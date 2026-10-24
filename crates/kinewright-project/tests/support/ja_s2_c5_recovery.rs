use kinewright_core::{
    AttachmentState, Command, Core, Document, Event, Operation, Track, TrackId, TrackKind,
};
use kinewright_media::test_support::TempDirectory;
use kinewright_project::{
    bundle::LoadedBundle,
    bundle_records::{BranchId, ObjectRef},
    bundle_recovery::RecoverySelection,
    bundle_switch::ProjectSelectionResult,
    editing_session::Ja1EditingSession,
    io_observer,
    storage_budget::StorageLimits,
};
use std::path::Path;
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
    let event = core
        .request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: true,
        }))
        .unwrap();
    assert!(
        matches!(event, Event::MutationRefused { .. }),
        "post-cut edit was accepted: {event:?}"
    );
    assert_eq!(core.attachment_state().unwrap(), AttachmentState::Frozen);
}
fn fixture() -> (
    TempDirectory,
    Ja1EditingSession,
    RecoverySelection,
    BranchId,
) {
    let temp = TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "s2-c5-local");
    let core = Core::spawn(Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
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
    let initial_cost = kinewright_project::bundle_commit::history_cost_probe(
        &core.export_history().unwrap(),
        &StorageLimits::default(),
    )
    .unwrap();
    let profile = StorageLimits {
        branch_history_bytes: initial_cost * 3,
        ..StorageLimits::default()
    };
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
fn w13_checkpoint_recovery_preserves_dirty_b_before_activation() {
    let (temp, mut session, _, original_branch) = fixture();
    let target = session
        .list_recovery_points()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    let target_core = {
        let loaded = LoadedBundle::read(&temp.path("a.kinewright")).unwrap();
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
    let history = original.export_history().unwrap();
    let oracle = Core::spawn((*history.baseline).clone()).unwrap();
    oracle.restore_history(history).unwrap();
    std::fs::create_dir_all(temp.path("ordinary-oracle")).unwrap();
    let mut ordinary = Ja1EditingSession::new(
        &temp.path("ordinary-oracle"),
        &oracle,
        StorageLimits::default(),
    )
    .unwrap();
    ordinary
        .first_save(
            &temp.path("ordinary.kinewright"),
            session.store().unwrap().limits().clone(),
        )
        .unwrap();
    let expected_l = oracle.export_history().unwrap();
    drop(ordinary);
    drop(oracle);
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
    let expected_target = (*session
        .store()
        .unwrap()
        .read_record::<kinewright_core::HistorySnapshot>(&archived_checkpoint.history_ref)
        .unwrap())
    .clone();
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
    assert_eq!(original.export_history().unwrap(), expected_l);
    let selected = session.core().clone();
    let expected_r = selected.export_history().unwrap();
    assert_eq!(expected_r, expected_target);
    assert!(!session.dirty().unwrap());
    let publications = trace
        .events
        .iter()
        .filter(|e| e.operation == "root_rename")
        .count();
    assert_eq!(publications, 2, "{trace:?}");
    assert!(session.store().unwrap().verify(&target_core).is_ok());
    let branch_lineage = |session: &Ja1EditingSession, id: &BranchId| {
        let loaded = LoadedBundle::read(session.association().unwrap().path()).unwrap();
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
        let lineage = (*session
            .store()
            .unwrap()
            .read_record::<kinewright_project::bundle_records::LineageRecord>(&cp.lineage_ref)
            .unwrap())
        .clone();
        (cp.batch_ids.clone(), lineage)
    };
    let (source_ids, source_lineage) = branch_lineage(&session, &original_branch);
    assert!(!source_ids.is_empty());
    assert!(!source_lineage.split_ancestry.is_empty());
    let target_ids = branch_lineage(
        &session,
        &LoadedBundle::read(&temp.path("a.kinewright"))
            .unwrap()
            .root()
            .unwrap()
            .manual_branch_id,
    )
    .0;
    let right = LoadedBundle::read(&temp.path("a.kinewright"))
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
        for (clip, occurrence) in &current.baseline {
            if let Some(original) = source_lineage.current.get(clip) {
                assert_eq!(occurrence, original);
            }
        }
        assert_eq!(current.current, source_lineage.current);
        assert_eq!(current.split_ancestry, source_lineage.split_ancestry);
    }
    assert!(selected.export_history().unwrap().floor_offset > expected_r.floor_offset);
    let later_r = selected.export_history().unwrap();
    session
        .prepare_recovery(RecoverySelection::WorkingBranch(original_branch.clone()))
        .unwrap();
    assert!(matches!(
        session.select_local(),
        ProjectSelectionResult::Activated(_)
    ));
    assert_eq!(session.core().export_history().unwrap(), expected_l);
    for i in 0..5 {
        edit(session.core(), i % 2 == 0);
    }
    session.save_active().unwrap();
    let (later_ids, later_lineage) = branch_lineage(&session, &original_branch);
    let (_, right_lineage) = branch_lineage(&session, &right);
    for (clip, occurrence) in &right_lineage.baseline {
        if let Some(original) = later_lineage.current.get(clip) {
            assert_eq!(occurrence, original);
        }
    }
    for shared in later_ids.iter().filter(|id| target_ids.contains(id)) {
        assert!(source_ids.contains(shared));
    }
    assert_eq!(later_lineage.current, source_lineage.current);
    assert_eq!(later_lineage.split_ancestry, source_lineage.split_ancestry);
    let later_l = session.core().export_history().unwrap();
    drop(session);
    drop(original);
    drop(selected);
    let loaded = LoadedBundle::read(&temp.path("a.kinewright")).unwrap();
    let fresh = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&fresh).unwrap();
    assert_eq!(fresh.export_history().unwrap(), later_l);
    drop(loaded);
    let mut fresh_owner = Ja1EditingSession::open(&temp.path("a.kinewright")).unwrap();
    fresh_owner
        .prepare_recovery(RecoverySelection::WorkingBranch(right))
        .unwrap();
    assert!(matches!(
        fresh_owner.select_local(),
        ProjectSelectionResult::Activated(_)
    ));
    assert_eq!(fresh_owner.core().export_history().unwrap(), later_r);
    assert_ne!(expected_r, later_r);
}
#[test]
fn w13_preliminary_save_faults_keep_exact_cut_and_do_not_select() {
    for fault in ["root_before_publish", "root_rename_io", "root_parent_sync"] {
        let (_temp, mut session, _, _) = fixture();
        let target = session.list_recovery_points().unwrap()[0].clone();
        edit(session.core(), false);
        let core = session.core().clone();
        let history = core.export_history().unwrap();
        session.prepare_recovery(target).unwrap();
        let result = io_observer::fail_once(fault, || session.select_local());
        assert_eq!(core.export_history().unwrap(), history);
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
                core.request(Command::Undo).unwrap(),
                Event::MutationRefused { .. }
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
fn w13_one_level_retention_and_explicit_retirement() {
    let (_temp, mut session, _, branch) = fixture();
    let mut guard = session.fork_durable_branch("worker X").unwrap();
    let specialist_core = guard.core().clone();
    edit(&specialist_core, false);
    let root = session.root_sha256().unwrap();
    let history = specialist_core.export_history().unwrap();
    assert!(
        session
            .retire_working_branch(guard.branch_id(), &root, true)
            .is_err()
    );
    assert_eq!(session.root_sha256().unwrap(), root);
    assert_eq!(specialist_core.export_history().unwrap(), history);
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
fn w13_prepared_existing_target_is_never_a_retirement_offer() {
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
fn w13_frozen_cut_rescue_at_both_unknown_publications_and_late_a_isolation() {
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
        let frozen = core.export_history().unwrap();
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
        assert!(matches!(
            core.request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: true
            }))
            .unwrap(),
            Event::MutationRefused { .. }
        ));
        assert!(matches!(
            core.request(Command::Undo).unwrap(),
            Event::MutationRefused { .. }
        ));
        assert!(matches!(
            core.request(Command::Redo).unwrap(),
            Event::MutationRefused { .. }
        ));
        assert_eq!(core.export_history().unwrap(), frozen);
        let unknown =
            attempt(&mut session, "unknown.kinewright", Some("root_parent_sync")).unwrap();
        assert!(!unknown.activated);
        frozen_refuses_input(&core);
        let outcome = attempt(&mut session, "success.kinewright", None).unwrap();
        assert!(outcome.activated, "{outcome:?}");
        assert!(outcome.disclosure.contains("Abandon") || publication == 1);
        let active = session.core().clone();
        assert_eq!(active.export_history().unwrap(), frozen);
        assert_ne!(active.actor_identity(), core.actor_identity());
        edit(&active, true);
        let newer = active.export_history().unwrap();
        let association = session.association().unwrap().path().to_path_buf();
        let dirty = session.dirty().unwrap();
        std::fs::rename(&hidden, &parent).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), old_root);
        session.reconcile_retained_destination(&a).unwrap();
        assert_eq!(core.attachment_state().unwrap(), AttachmentState::Sealed);
        assert_eq!(active.export_history().unwrap(), newer);
        assert_eq!(session.core().actor_identity(), active.actor_identity());
        assert_eq!(active.attachment_state().unwrap(), AttachmentState::Live);
        assert_eq!(session.association().unwrap().path(), association);
        assert_eq!(session.dirty().unwrap(), dirty);
        session.save_active().unwrap();
        let saved = active.export_history().unwrap();
        drop(session);
        drop(core);
        drop(active);
        let loaded = LoadedBundle::read(&b_parent.join("success.kinewright")).unwrap();
        let fresh = Core::spawn(Document::default()).unwrap();
        loaded.restore_manual(&fresh).unwrap();
        assert_eq!(fresh.export_history().unwrap(), saved);
        drop(loaded);
        if publication == 2 && !landed {
            let mut collector = LoadedBundle::read(&a).unwrap().into_writer().unwrap();
            collector.collect_after_ack().unwrap();
            drop(collector);
            let reopened = LoadedBundle::read(&a).unwrap();
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
fn w13_existing_target_retry_requires_current_membership_and_head() {
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
        let history = session.core().export_history().unwrap();
        assert!(matches!(
            session.select_local(),
            ProjectSelectionResult::Refused(_)
        ));
        assert_eq!(session.root_sha256().unwrap(), root);
        assert_eq!(session.core().export_history().unwrap(), history);
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
fn w13_selection_refusal_and_absence_keep_p2_for_same_target_retry() {
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
        let loaded = LoadedBundle::read(&temp.path("a.kinewright")).unwrap();
        assert_eq!(loaded.root().unwrap().manual_switch_generation, 1);
    }
}

#[test]
fn w13_preliminary_save_uses_ordinary_floor_with_redo_or_gesture() {
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
        let before = original.export_history().unwrap();
        let profile = session.store().unwrap().limits();
        let expected = (1..=before.batches.len())
            .find_map(|cut| {
                let after = kinewright_core::HistoryTransition::floor(before.clone(), cut)
                    .ok()?
                    .after()
                    .clone();
                let cost =
                    kinewright_project::bundle_commit::history_cost_probe(&after, profile).ok()?;
                (after.encoded_bytes().ok()? <= profile.branch_history_bytes
                    && cost <= profile.branch_history_bytes)
                    .then_some(after)
            })
            .expect("ordinary legal earliest floor");
        assert!(expected.floor_offset > before.floor_offset);
        let disclosure = session.recovery_save_disclosure().unwrap();
        assert!(disclosure.contains(&format!(
            "{} oldest Undo",
            before.cursor.undo.len() - expected.cursor.undo.len()
        )));
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
        assert_eq!(original.export_history().unwrap(), expected);
        assert_eq!(expected.document, before.document);
        assert_eq!(expected.cursor.redo, before.cursor.redo);
        assert!(!session.dirty().unwrap());
        assert!(matches!(
            session.select_local(),
            ProjectSelectionResult::Activated(_)
        ));
    }
}
