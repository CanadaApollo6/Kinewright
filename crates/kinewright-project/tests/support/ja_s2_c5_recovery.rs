use super::Ja1EditingSession;
use kinewright_core::{
    AttachmentState, Command, Document, Event, Operation, Track, TrackId, TrackKind,
};
use kinewright_media::test_support::TempDirectory;
use kinewright_project::operation_actor::Core;
use kinewright_project::{
    bundle::LoadedBundle,
    bundle_records::{BranchId, ObjectRef},
    bundle_recovery::RecoverySelection,
    bundle_switch::ProjectSelectionResult,
    io_observer,
    storage_budget::StorageLimits,
};
fn edit(core: &Core, locked: bool) {
    assert!(matches!(
        core.request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked
        }))
        .unwrap(),
        Event::DocumentChanged { .. }
    ));
}
fn frozen_refuses_input(core: &Core) {
    let result = core.request(Command::Do(Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked: true,
    }));
    assert!(
        matches!(
            result,
            Err(kinewright_project::operation_actor::Refusal {
                error: kinewright_project::operation_actor::Error::MutationRefused(
                    kinewright_core::CoreMutationRefusal::Frozen
                ),
                ..
            })
        ),
        "FROZEN_SELECTION_REFUSES_POST_CUT_INPUT {result:?}"
    );
    assert_eq!(core.attachment_state().unwrap(), AttachmentState::Frozen);
}
// The expected selected contents outlive several publications. JSON is an
// independent test oracle, not another resident history holder. Installation
// changes the capture epoch; every command key and all owner fields stay checked.
pub(super) fn contents(
    snapshot: &kinewright_project::operation_history::OwnerSnapshot,
) -> serde_json::Value {
    let mut value = serde_json::to_value(snapshot).unwrap();
    value["history"].as_object_mut().unwrap().remove("epoch");
    for stack in ["undo", "redo"] {
        for step in value["history"][stack].as_array_mut().unwrap() {
            step.as_object_mut().unwrap().remove("charge");
        }
    }
    value
}
#[track_caller]
pub(super) fn state(core: &Core) -> serde_json::Value {
    let snapshot = match core.export_history() {
        Ok(snapshot) => snapshot,
        Err(error) => panic!(
            "STATE_CAPTURE {error}; pool={:?}",
            core.history_pool()
                .lease_counts(kinewright_project::operation_history::Role::Ordinary)
        ),
    };
    contents(&snapshot)
}

fn fixture() -> (
    TempDirectory,
    Ja1EditingSession,
    RecoverySelection,
    BranchId,
) {
    let temp = TempDirectory::new_in(super::durable_test_root(), "s2-c5-local");
    let core = Core::for_test(Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            name: None,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    })
    .unwrap();
    for command in [
        Command::Do(Operation::AddSolidClip {
            track: TrackId(1),
            timeline_start: kinewright_core::TimeCode(0),
            duration: kinewright_core::TimeCode(30),
            color: kinewright_core::SolidColor::default(),
        }),
        Command::Do(Operation::SplitClip {
            clip: kinewright_core::ClipId(1),
            at: kinewright_core::TimeCode(15),
        }),
        Command::Undo,
        Command::Redo,
    ] {
        assert!(matches!(
            core.request(command).unwrap(),
            Event::DocumentChanged { .. }
        ));
    }
    let profile = StorageLimits::default();
    let mut session = Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
    assert!(matches!(
        session
            .first_save(&temp.path("a.kinewright"), profile)
            .unwrap(),
        kinewright_project::bundle_copy::CopyStatus::Committed(_)
    ));
    let first = std::fs::read(temp.path("a.kinewright")).unwrap();
    let branch = session.list_working_branches().unwrap()[0].0.clone();
    edit(&core, false);
    session.save_active().unwrap();
    edit(&core, true);
    session.save_active().unwrap();
    let point = ObjectRef {
        sha256: kinewright_media::sha256_bytes(&first),
        byte_length: first.len() as u64,
        record_kind: kinewright_project::bundle_records::RecordKind::RootCheckpoint,
    };
    let target = RecoverySelection::Checkpoint {
        root: point,
        branch: branch.clone(),
    };
    session.save_active().unwrap();
    (temp, session, target, branch)
}
#[test]
pub(super) fn w13_checkpoint_recovery_preserves_dirty_b_before_activation() {
    let (temp, mut session, _, original_branch) = fixture();
    let target = session
        .list_recovery_points()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    let target_core = {
        let loaded = LoadedBundle::read(
            &temp.path("a.kinewright"),
            &kinewright_project::operation_history::HistoryPool::new(
                kinewright_project::operation_history::PoolLimits::default(),
            ),
        )
        .unwrap();
        let writer_root = loaded.root().unwrap();
        assert_eq!(writer_root.manual_branch_id, original_branch);
        let RecoverySelection::Checkpoint { root, .. } = &target else {
            panic!("checkpoint");
        };
        assert!(session.store().unwrap().verify(root).is_ok());
        root.clone()
    };
    let original = session.core().clone();
    edit(&original, false);
    edit(&original, true);
    assert!(matches!(
        original.request(Command::Undo).unwrap(),
        Event::DocumentChanged { .. }
    ));
    let expected_l = state(&original);
    let raw = std::fs::read(temp.path(&format!(
        "a.kinewright-assets/studio/objects/{}",
        target_core.sha256
    )))
    .unwrap();
    assert_eq!(kinewright_media::sha256_bytes(&raw), target_core.sha256);
    let archived: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let archived: kinewright_project::bundle::StudioRoot =
        serde_json::from_value(archived["studio"].clone()).unwrap();
    let archived_index = session
        .store()
        .unwrap()
        .read_record::<std::collections::BTreeMap<BranchId, ObjectRef>>(&archived.branch_index_ref)
        .unwrap();
    let archived_checkpoint = session
        .store()
        .unwrap()
        .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(
            &archived_index[&archived.manual_branch_id],
        )
        .unwrap();
    let expected_target = archived_checkpoint
        .open(
            session.store().unwrap(),
            &kinewright_project::operation_history::HistoryPool::new(
                kinewright_project::operation_history::PoolLimits::default(),
            ),
        )
        .unwrap()
        .into_reader()
        .unwrap()
        .into_capture()
        .unwrap();
    drop(archived_checkpoint);
    drop(archived_index);
    session.prepare_recovery(target.clone()).unwrap();
    assert!(
        session
            .selection_availability()
            .unwrap()
            .contains("will remove")
    );
    let before = session.root_sha256().unwrap();
    let (result, trace) = io_observer::observe(|| session.select_local());
    assert!(
        matches!(result, ProjectSelectionResult::Activated(_)),
        "{result:?}"
    );
    assert_ne!(session.root_sha256().unwrap(), before);
    assert_eq!(
        original.attachment_state().unwrap(),
        AttachmentState::Sealed
    );
    assert!(
        matches!(
            original.export_history(),
            Err(kinewright_project::operation_actor::Error::Retired)
        ),
        "SELECTED_SOURCE_RELEASES_ITS_RESIDENT_HISTORY"
    );
    let selected = session.core().clone();
    let expected_r = state(&selected);
    assert_eq!(expected_r, contents(&expected_target));
    drop(expected_target);
    assert!(!session.dirty().unwrap());
    let publications = trace
        .events
        .iter()
        .filter(|e| e.operation == "root_rename")
        .count();
    assert_eq!(publications, 2, "{trace:?}");
    assert!(session.store().unwrap().verify(&target_core).is_ok());
    let branch_lineage = |session: &Ja1EditingSession, id: &BranchId| {
        let loaded = LoadedBundle::read(
            session.association().unwrap().path(),
            &kinewright_project::operation_history::HistoryPool::new(
                kinewright_project::operation_history::PoolLimits::default(),
            ),
        )
        .unwrap();
        let index = session
            .store()
            .unwrap()
            .read_record::<std::collections::BTreeMap<BranchId, ObjectRef>>(
                &loaded.root().unwrap().branch_index_ref,
            )
            .unwrap();
        let cp = session
            .store()
            .unwrap()
            .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(&index[id])
            .unwrap();
        let capture = cp
            .open(
                session.store().unwrap(),
                &kinewright_project::operation_history::HistoryPool::new(
                    kinewright_project::operation_history::PoolLimits::default(),
                ),
            )
            .unwrap()
            .into_reader()
            .unwrap()
            .into_capture()
            .unwrap();
        let keys = capture
            .history()
            .undo
            .iter()
            .chain(capture.history().redo.iter().rev())
            .flat_map(|step| {
                step.commands()
                    .iter()
                    .map(|command| (command.id, command.payload_digest))
            })
            .collect::<Vec<_>>();
        let depth = capture.history().undo.len();
        let lineage = capture
            .validate()
            .unwrap()
            .into_reader()
            .unwrap()
            .into_boundary(depth)
            .unwrap()
            .lineage()
            .clone();
        (keys, lineage)
    };
    let (source_ids, source_lineage) = branch_lineage(&session, &original_branch);
    assert!(!source_ids.is_empty());
    assert!(!source_lineage.split_ancestry.is_empty());
    let target_ids = branch_lineage(
        &session,
        &LoadedBundle::read(
            &temp.path("a.kinewright"),
            &kinewright_project::operation_history::HistoryPool::new(
                kinewright_project::operation_history::PoolLimits::default(),
            ),
        )
        .unwrap()
        .root()
        .unwrap()
        .manual_branch_id,
    )
    .0;
    let right = LoadedBundle::read(
        &temp.path("a.kinewright"),
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .unwrap()
    .root()
    .unwrap()
    .manual_branch_id
    .clone();
    for n in 0..3 {
        for i in 0..4 {
            edit(&selected, (n + i) % 2 == 0);
        }
        session.save_active().unwrap();
        let (right_ids, current) = branch_lineage(&session, &right);
        if n == 0 {
            for shared in source_ids.iter().filter(|id| target_ids.contains(id)) {
                assert!(
                    right_ids.contains(shared),
                    "a shared checkpoint batch was reminted"
                );
            }
        }
        assert_eq!(
            current.occurrences, source_lineage.occurrences,
            "INSTALL_AND_LOCK_EDITS_PRESERVE_OCCURRENCE_IDENTITIES"
        );
        assert_eq!(current.split_ancestry, source_lineage.split_ancestry);
    }
    assert_eq!(
        serde_json::to_value(selected.export_history().unwrap().fold_frontier()).unwrap(),
        expected_r["fold_frontier"],
        "SAVE_DOES_NOT_CREATE_A_FOLD"
    );
    let later_r = state(&selected);
    let (_, expected_right_lineage) = branch_lineage(&session, &right);
    session
        .prepare_recovery(RecoverySelection::WorkingBranch(original_branch.clone()))
        .unwrap();
    assert!(matches!(
        session.select_local(),
        ProjectSelectionResult::Activated(_)
    ));
    assert_eq!(state(session.core()), expected_l);
    for i in 0..5 {
        edit(session.core(), i % 2 == 0);
    }
    session.save_active().unwrap();
    let (later_ids, later_lineage) = branch_lineage(&session, &original_branch);
    let (_, right_lineage) = branch_lineage(&session, &right);
    assert_eq!(right_lineage, expected_right_lineage);
    for shared in later_ids.iter().filter(|id| target_ids.contains(id)) {
        assert!(source_ids.contains(shared));
    }
    assert_eq!(later_lineage.occurrences, source_lineage.occurrences);
    assert_eq!(later_lineage.split_ancestry, source_lineage.split_ancestry);
    let later_l = state(session.core());
    drop(session);
    drop(original);
    drop(selected);
    let mut loaded = LoadedBundle::read(
        &temp.path("a.kinewright"),
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .unwrap();
    let fresh = loaded.open_manual().unwrap();
    assert_eq!(state(&fresh), later_l);
    drop(loaded);
    let mut fresh_owner = Ja1EditingSession::open(
        &temp.path("a.kinewright"),
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .unwrap();
    fresh_owner
        .prepare_recovery(RecoverySelection::WorkingBranch(right))
        .unwrap();
    assert!(matches!(
        fresh_owner.select_local(),
        ProjectSelectionResult::Activated(_)
    ));
    assert_eq!(state(fresh_owner.core()), later_r);
    assert_ne!(expected_r, later_r);
}
#[test]
pub(super) fn w13_preliminary_save_faults_keep_exact_cut_and_do_not_select() {
    for fault in ["root_before_publish", "root_rename_io", "root_parent_sync"] {
        let (_temp, mut session, _, _) = fixture();
        let target = session.list_recovery_points().unwrap()[0].clone();
        edit(session.core(), false);
        let core = session.core().clone();
        let history = state(&core);
        session.prepare_recovery(target).unwrap();
        let result = io_observer::fail_once(fault, || session.select_local());
        assert_eq!(state(&core), history);
        if fault == "root_before_publish" {
            assert!(
                matches!(result, ProjectSelectionResult::Refused(_)),
                "{result:?}"
            );
            assert_eq!(core.attachment_state().unwrap(), AttachmentState::Live);
            assert!(session.dirty().unwrap());
            session.save_active().unwrap();
        } else {
            assert!(
                matches!(result, ProjectSelectionResult::Unknown),
                "{result:?}"
            );
            assert_eq!(core.attachment_state().unwrap(), AttachmentState::Frozen);
            assert!(matches!(
                core.request(Command::Undo),
                Err(kinewright_project::operation_actor::Refusal {
                    error: kinewright_project::operation_actor::Error::MutationRefused(
                        kinewright_core::CoreMutationRefusal::Frozen
                    ),
                    ..
                })
            ));
            let result = session.select_local();
            if fault == "root_rename_io" {
                assert!(
                    matches!(result, ProjectSelectionResult::Refused(_)),
                    "{result:?}"
                );
                assert_eq!(core.attachment_state().unwrap(), AttachmentState::Live);
            } else {
                assert!(
                    matches!(result, ProjectSelectionResult::Activated(_)),
                    "{result:?}"
                );
                assert_eq!(core.attachment_state().unwrap(), AttachmentState::Sealed);
            }
        }
    }
}
#[test]
pub(super) fn w13_one_level_retention_and_explicit_retirement() {
    let (_temp, mut session, _, branch) = fixture();
    let mut guard = session
        .fork_durable_branch(
            "worker X",
            kinewright_project::editing_session::DurableBranchRole::Session,
        )
        .unwrap();
    let specialist_core = guard.core().clone();
    edit(&specialist_core, false);
    let root = session.root_sha256().unwrap();
    let history = state(&specialist_core);
    assert!(
        session
            .retire_working_branch(guard.branch_id(), &root, true)
            .is_err()
    );
    assert_eq!(session.root_sha256().unwrap(), root);
    assert_eq!(state(&specialist_core), history);
    assert!(session.retire_working_branch(&branch, &root, true).is_err());
    assert!(session.close_durable_branch(&guard).is_err());
    session.persist_durable_branch(&mut guard).unwrap();
    session.close_durable_branch(&guard).unwrap();
    assert_eq!(
        specialist_core.attachment_state().unwrap(),
        AttachmentState::Sealed
    );
    let root = session.root_sha256().unwrap();
    assert!(
        session
            .retire_working_branch(guard.branch_id(), &root, false)
            .is_err()
    );
    assert_eq!(session.root_sha256().unwrap(), root);
    session
        .retire_working_branch(guard.branch_id(), &root, true)
        .unwrap();
    assert!(
        !session
            .list_working_branches()
            .unwrap()
            .iter()
            .any(|(id, _, _)| id == guard.branch_id())
    );
    assert!(
        session
            .retire_working_branch(guard.branch_id(), &session.root_sha256().unwrap(), true)
            .is_err()
    );
}
#[test]
pub(super) fn w13_prepared_existing_target_is_never_a_retirement_offer() {
    let (_temp, mut session, _, branch) = fixture();
    let target = session.list_recovery_points().unwrap()[0].clone();
    session.prepare_recovery(target).unwrap();
    assert!(matches!(
        session.select_local(),
        ProjectSelectionResult::Activated(_)
    ));
    session
        .prepare_recovery(RecoverySelection::WorkingBranch(branch.clone()))
        .unwrap();
    let before = session.root_sha256().unwrap();
    assert!(!session.retirement_candidates().unwrap().contains(&branch));
    assert!(
        session
            .retire_working_branch(&branch, &before, true)
            .is_err()
    );
    assert_eq!(session.root_sha256().unwrap(), before);
    assert!(
        session
            .check_destination_change()
            .unwrap_err()
            .to_string()
            .contains("Abandon")
    );
    session.abandon_recovery().unwrap();
    session.check_destination_change().unwrap();
}

#[test]
pub(super) fn w13_frozen_cut_rescue_at_both_unknown_publications_and_late_a_isolation() {
    for (publication, landed) in [(1, false), (1, true), (2, false), (2, true)] {
        let (temp, mut session, _, _) = fixture();
        let a = temp.path("a.kinewright");
        // Move the complete private fixture under an independently renameable parent.
        // The owning namespace remains the original parent; the probe fault models unavailability.
        let target = session
            .list_recovery_points()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        let RecoverySelection::Checkpoint {
            root: selected_root,
            ..
        } = &target
        else {
            panic!("checkpoint");
        };
        let selected_root = selected_root.clone();
        let core = session.core().clone();
        edit(&core, false);
        session.prepare_recovery(target).unwrap();
        let result = io_observer::fail_nth(
            if landed {
                "root_parent_sync"
            } else {
                "root_rename_io"
            },
            publication,
            || session.select_local(),
        );
        assert!(
            matches!(result, ProjectSelectionResult::Unknown),
            "{result:?}"
        );
        assert_eq!(
            session.dirty().unwrap(),
            publication == 1,
            "only a definitely successful preliminary Save acknowledges O"
        );
        let frozen_identity = core.snapshot_identity().unwrap();
        let frozen = state(&core);
        frozen_refuses_input(&core);
        let old_root = std::fs::read(&a).unwrap();
        let parent = temp.root().to_path_buf();
        let hidden = parent.with_extension(format!("hidden-{publication}-{landed}"));
        std::fs::rename(&parent, &hidden).unwrap();
        let b_parent = parent.parent().unwrap().join(format!(
            "c5-rescue-{publication}-{landed}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&b_parent).unwrap();
        let attempt = |session: &mut Ja1EditingSession, name: &str, fault: Option<&'static str>| {
            let offer = session.probe_active().unwrap().unwrap();
            let capture = session.prepare_rescue(&offer).unwrap().unwrap();
            let selection = session
                .preview_rescue(capture, StorageLimits::default())
                .unwrap();
            let consent = selection.consent();
            let destination = session
                .inspect_rescue_destination(&b_parent.join(name))
                .unwrap();
            if let Some(fault) = fault {
                io_observer::fail_once(fault, || session.rescue(selection, &consent, &destination))
            } else {
                session.rescue(selection, &consent, &destination)
            }
        };
        let offer = session.probe_active().unwrap().unwrap();
        let capture = session.prepare_rescue(&offer).unwrap().unwrap();
        session.cancel_rescue(capture).unwrap();
        frozen_refuses_input(&core);
        let offer = session.probe_active().unwrap().unwrap();
        let capture = session.prepare_rescue(&offer).unwrap().unwrap();
        let tiny = StorageLimits {
            objects: 1,
            ..StorageLimits::default()
        };
        assert!(session.preview_rescue(capture, tiny).is_err());
        frozen_refuses_input(&core);
        let failed_creation = attempt(
            &mut session,
            "missing/failed-creation.kinewright",
            Some("destination_child_sync"),
        );
        assert!(failed_creation.is_err());
        frozen_refuses_input(&core);
        let failed = attempt(
            &mut session,
            "failed.kinewright",
            Some("root_before_publish"),
        );
        assert!(failed.is_err() || failed.as_ref().is_ok_and(|o| !o.activated));
        frozen_refuses_input(&core);
        for command in [Command::Undo, Command::Redo] {
            assert!(matches!(
                core.request(command),
                Err(kinewright_project::operation_actor::Refusal {
                    error: kinewright_project::operation_actor::Error::MutationRefused(
                        kinewright_core::CoreMutationRefusal::Frozen
                    ),
                    ..
                })
            ));
        }
        assert_eq!(core.snapshot_identity().unwrap(), frozen_identity);
        let unknown =
            attempt(&mut session, "unknown.kinewright", Some("root_parent_sync")).unwrap();
        assert!(!unknown.activated);
        frozen_refuses_input(&core);
        let outcome = attempt(&mut session, "success.kinewright", None).unwrap();
        assert!(outcome.activated, "{outcome:?}");
        assert!(outcome.disclosure.contains("Abandon") || publication == 1);
        let active = session.core().clone();
        assert_eq!(state(&active), frozen);
        assert_ne!(active.actor_identity(), core.actor_identity());
        edit(&active, true);
        let newer = state(&active);
        let association = session.association().unwrap().path().to_path_buf();
        let dirty = session.dirty().unwrap();
        std::fs::rename(&hidden, &parent).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), old_root);
        session.reconcile_retained_destination(&a).unwrap();
        assert_eq!(core.attachment_state().unwrap(), AttachmentState::Sealed);
        assert_eq!(state(&active), newer);
        assert_eq!(session.core().actor_identity(), active.actor_identity());
        assert_eq!(active.attachment_state().unwrap(), AttachmentState::Live);
        assert_eq!(session.association().unwrap().path(), association);
        assert_eq!(session.dirty().unwrap(), dirty);
        session.save_active().unwrap();
        let saved = state(&active);
        drop(session);
        drop(core);
        drop(active);
        let mut loaded = LoadedBundle::read(
            &b_parent.join("success.kinewright"),
            &kinewright_project::operation_history::HistoryPool::new(
                kinewright_project::operation_history::PoolLimits::default(),
            ),
        )
        .unwrap();
        let fresh = loaded.open_manual().unwrap();
        assert_eq!(state(&fresh), saved);
        drop(loaded);
        if publication == 2 && !landed {
            let mut collector = LoadedBundle::read(
                &a,
                &kinewright_project::operation_history::HistoryPool::new(
                    kinewright_project::operation_history::PoolLimits::default(),
                ),
            )
            .unwrap()
            .into_writer()
            .unwrap();
            collector.collect_after_ack().unwrap();
            drop(collector);
            let reopened = LoadedBundle::read(
                &a,
                &kinewright_project::operation_history::HistoryPool::new(
                    kinewright_project::operation_history::PoolLimits::default(),
                ),
            )
            .unwrap();
            drop(reopened);
            let objects = temp.path("a.kinewright-assets/studio/objects");
            assert!(
                !objects.join(&selected_root.sha256).exists(),
                "session-only P2 must be collectible after A absence and context teardown"
            );
        }
        std::fs::remove_dir_all(&b_parent).unwrap();
    }
}

#[test]
pub(super) fn w13_existing_target_retry_requires_current_membership_and_head() {
    for head in [false, true] {
        let (_temp, mut session, _, left) = fixture();
        let target = session.list_recovery_points().unwrap()[0].clone();
        session.prepare_recovery(target).unwrap();
        assert!(matches!(
            session.select_local(),
            ProjectSelectionResult::Activated(_)
        ));
        session
            .prepare_recovery(RecoverySelection::WorkingBranch(left.clone()))
            .unwrap();
        session.change_prepared_target_probe(head).unwrap();
        let root = session.root_sha256().unwrap();
        let history = state(session.core());
        assert!(matches!(
            session.select_local(),
            ProjectSelectionResult::Refused(_)
        ));
        assert_eq!(session.root_sha256().unwrap(), root);
        assert_eq!(state(session.core()), history);
        assert_eq!(
            session.core().attachment_state().unwrap(),
            AttachmentState::Live
        );
        assert_eq!(
            session
                .list_working_branches()
                .unwrap()
                .iter()
                .any(|(id, _, _)| id == &left),
            head
        );
    }
}

#[test]
pub(super) fn w13_selection_refusal_and_absence_keep_p2_for_same_target_retry() {
    for fault in ["root_before_publish", "root_rename_io", "root_parent_sync"] {
        let (temp, mut session, _, _) = fixture();
        edit(session.core(), false);
        let original = session.core().clone();
        let target = session
            .list_recovery_points()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        session.prepare_recovery(target.clone()).unwrap();
        let initial_count = session.list_working_branches().unwrap().len();
        let result = io_observer::fail_nth(fault, 2, || session.select_local());
        if fault == "root_before_publish" {
            assert!(matches!(result, ProjectSelectionResult::Refused(_)));
            assert_eq!(original.attachment_state().unwrap(), AttachmentState::Live);
        } else {
            assert!(matches!(result, ProjectSelectionResult::Unknown));
            assert_eq!(
                original.attachment_state().unwrap(),
                AttachmentState::Frozen
            );
            if fault == "root_rename_io" {
                assert!(matches!(
                    session.select_local(),
                    ProjectSelectionResult::Refused(_)
                ));
                assert_eq!(original.attachment_state().unwrap(), AttachmentState::Live);
            }
        }
        if fault != "root_parent_sync" {
            assert!(
                !session.dirty().unwrap(),
                "preliminary ordinary Save is definite"
            );
            let disclosure = session.selection_availability().unwrap();
            assert!(disclosure.contains("only for this selection in this session"));
            let root = session.root_sha256().unwrap();
            assert!(
                session
                    .save_as(&temp.path("forbidden.kinewright"), StorageLimits::default())
                    .is_err()
            );
            assert!(session.open_project(&temp.path("a.kinewright")).is_err());
            assert_eq!(session.root_sha256().unwrap(), root);
            assert!(!temp.path("forbidden.kinewright").exists());
            session.save_active().unwrap();
            session.prepare_recovery(target.clone()).unwrap();
        }
        assert!(matches!(
            session.select_local(),
            ProjectSelectionResult::Activated(_)
        ));
        let root = session.root_sha256().unwrap();
        session.prepare_recovery(target).unwrap();
        assert_eq!(
            session.list_working_branches().unwrap().len(),
            initial_count + 1
        );
        assert!(matches!(
            session.select_local(),
            ProjectSelectionResult::Activated(_)
        ));
        assert_eq!(session.root_sha256().unwrap(), root);
        drop(session);
        drop(original);
        let loaded = LoadedBundle::read(
            &temp.path("a.kinewright"),
            &kinewright_project::operation_history::HistoryPool::new(
                kinewright_project::operation_history::PoolLimits::default(),
            ),
        )
        .unwrap();
        assert_eq!(loaded.root().unwrap().manual_switch_generation, 1);
    }
}

#[test]
pub(super) fn w13_preliminary_save_uses_ordinary_floor_with_redo_or_gesture() {
    for gesture in [false, true] {
        let (_temp, mut session, _, _) = fixture();
        for i in 0..24 {
            edit(session.core(), i % 2 == 0);
        }
        if gesture {
            assert!(matches!(
                session
                    .core()
                    .request(Command::DoBatchCoalesced {
                        operations: vec![Operation::SetTrackSyncLock {
                            track: TrackId(1),
                            locked: false
                        }],
                        coalesce_key: "recovery-open-gesture".into(),
                    })
                    .unwrap(),
                Event::DocumentChanged { .. }
            ));
        } else {
            assert!(matches!(
                session.core().request(Command::Undo).unwrap(),
                Event::DocumentChanged { .. }
            ));
        }
        let original = session.core().clone();
        let before = state(&original);
        let disclosure = session.recovery_save_disclosure().unwrap();
        assert!(
            !disclosure.contains("oldest Undo"),
            "PRELIMINARY_SAVE_NEVER_TRIMS_HISTORY"
        );
        let target = session
            .list_recovery_points()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        session.prepare_recovery(target).unwrap();
        assert!(matches!(
            io_observer::fail_nth("root_before_publish", 2, || session.select_local()),
            ProjectSelectionResult::Refused(_)
        ));
        assert_eq!(original.attachment_state().unwrap(), AttachmentState::Live);
        assert_eq!(
            state(&original),
            before,
            "PRELIMINARY_SAVE_PRESERVES_EVERY_REACHABLE_STATE"
        );
        assert!(!session.dirty().unwrap());
        assert!(matches!(
            session.select_local(),
            ProjectSelectionResult::Activated(_)
        ));
    }
}
