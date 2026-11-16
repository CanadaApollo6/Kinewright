//! C1: ordinary publication/cleanup controls. No rescue or host availability is claimed.
#![cfg(feature = "test-util")]
#![allow(
    clippy::too_many_lines,
    reason = "each fault witness keeps controls and independent oracles together"
)]

#[cfg(windows)]
#[test]
fn native_windows_still_refuses_before_store_mutation() {
    use kinewright_project::{
        bundle::{BundleError, BundleWriteAdmission, ProjectBundle},
        io_observer,
    };
    let temp = kinewright_media::test_support::TempDirectory::new("s2-native-refusal");
    let path = temp.path("refused.kinewright");
    let pool = kinewright_project::operation_history::HistoryPool::new(
        kinewright_project::operation_history::PoolLimits::default(),
    );
    let (result, trace) = io_observer::observe(|| {
        ProjectBundle::activate(&path, BundleWriteAdmission::new_document(), &pool)
    });
    assert!(matches!(result, Err(BundleError::DurabilityUnsupported)));
    assert!(trace.events.is_empty());
    assert!(!path.exists());
}

#[cfg(target_os = "linux")]
mod linux {
    use kinewright_core::{
        Command, Document, Event, JournalCommand, Operation, Query, QueryResult, TimelineRevision,
        Track, TrackId, TrackKind,
    };
    use kinewright_media::test_support::TempDirectory;
    use kinewright_project::operation_actor::Core;
    use kinewright_project::{
        bundle::{
            BundleError, BundleWriteAdmission, LoadedBundle, ProjectBundle, PublicationState,
        },
        bundle_commit::{BundleCommit, CommitOutcome},
        bundle_copy::{BundleCopy, CopyStatus, SaveAsOutcome, SessionStore},
        bundle_records::{ObjectRef, RecordKind},
        io_observer,
        owned_inputs::{OwnedInputEntry, OwnedInputManifest},
        receipts::{CommandEnvelope, CommandId, ReceiptLookup},
        storage_budget::StorageLimits,
    };
    use std::{
        fs,
        path::{Path, PathBuf},
    };
    fn temp(label: &str) -> TempDirectory {
        TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), label)
    }
    fn document(tracks: u64) -> Document {
        Document {
            tracks: (1..=tracks)
                .map(|id| Track {
                    id: TrackId(id),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![],
                })
                .collect(),
            ..Document::default()
        }
    }
    fn apply(core: &Core, command: Command) {
        assert!(matches!(
            core.request(command).unwrap(),
            Event::DocumentChanged { .. }
        ));
    }
    fn grow(core: &Core, n: u64) {
        for i in 0..n {
            apply(
                core,
                Command::Do(Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: i.is_multiple_of(2),
                }),
            );
        }
    }
    fn small_limits() -> StorageLimits {
        StorageLimits {
            branch_history_bytes: 1 << 20,
            ..StorageLimits::default()
        }
    }
    fn start(path: &Path, core: &Core) -> (ProjectBundle, ObjectRef) {
        let mut owner = ProjectBundle::activate(
            path,
            BundleWriteAdmission::new_document(),
            core.history_pool(),
        )
        .unwrap();
        let inputs = owner.empty_inputs().unwrap().reference.clone();
        owner.save(core, &inputs).unwrap();
        (owner, inputs)
    }
    fn staged(parent: &Path) -> Vec<PathBuf> {
        fs::read_dir(parent)
            .unwrap()
            .map(Result::unwrap)
            .filter(|e| {
                e.file_name().to_string_lossy().starts_with(".studio-")
                    && e.file_name().to_string_lossy().ends_with(".tmp")
            })
            .map(|e| e.path())
            .collect()
    }
    fn session_inputs(session: &mut SessionStore) -> ObjectRef {
        session
            .admit_inputs(&OwnedInputManifest {
                project_id: None,
                session_id: Some(session.session_id().clone()),
                owner_epoch: session.store().owner_epoch(),
                entries: vec![],
            })
            .unwrap()
            .reference
            .clone()
    }
    fn destination(copy: BundleCopy) -> ProjectBundle {
        match copy.into_destination() {
            Ok(owner) => owner,
            Err(pending) => panic!("copy must be definitely committed: {:?}", pending.status()),
        }
    }
    fn copy(owner: &mut ProjectBundle, path: &Path, limits: StorageLimits) -> ProjectBundle {
        let SaveAsOutcome::Copied(copy) = owner
            .save_as_with_limits(path, limits)
            .unwrap_or_else(|e| panic!("copy {}: {e}", path.display()))
        else {
            panic!("fresh copy")
        };
        assert!(matches!(copy.status(), CopyStatus::Committed(_)));
        destination(*copy)
    }
    fn unchanged(core: &Core, history: &kinewright_project::operation_history::OwnerSnapshot) {
        assert!(
            core.export_history().unwrap() == *history,
            "live history changed"
        );
        assert_eq!(
            core.attachment_state().unwrap(),
            kinewright_core::AttachmentState::Live
        );
    }
    #[test]
    fn w12_post_b5_floor_error_keeps_publication_phase() {
        let temp = temp("s2-max-revision-save");
        let path = temp.path("a.kinewright");
        let core =
            Core::for_test_at_revision(document(200), TimelineRevision(u64::MAX - 40)).unwrap();
        grow(&core, 40);
        let (mut owner, inputs) = start(&path, &core);
        let before = core.snapshot_identity().unwrap();
        assert!(
            matches!(
                BundleCommit::run(&mut owner, &core, &inputs),
                CommitOutcome::Committed(_)
            ),
            "PLAIN_SAVE_NEVER_MINTS_A_REVISION_OR_AN_IMPLICIT_FLOOR"
        );
        assert_eq!(owner.publication_state(), PublicationState::Committed);
        assert_eq!(core.snapshot_identity().unwrap(), before);
        let root = fs::read(&path).unwrap();
        let refusal = core
            .request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: true,
            }))
            .unwrap_err();
        assert_eq!(
            refusal.error,
            kinewright_project::operation_actor::Error::CounterExhausted,
            "REVISION_EXHAUSTION_REFUSES_BEFORE_ACCEPTANCE"
        );
        assert_eq!(core.snapshot_identity().unwrap(), before);
        assert_eq!(fs::read(&path).unwrap(), root);
        drop(owner);
        let mut loaded = LoadedBundle::read(
            &path,
            &kinewright_project::operation_history::HistoryPool::new(
                kinewright_project::operation_history::PoolLimits::default(),
            ),
        )
        .unwrap();
        let cold = loaded.open_manual().unwrap();
        assert_eq!(
            cold.snapshot_identity().unwrap(),
            before,
            "MAX_REVISION_PLAIN_SAVE_COLD_OPENS_EXACTLY"
        );
    }
    #[test]
    fn w12_nonpermission_rename_retains_charged_temporary() {
        let temp = temp("s2-rename-io");
        let path = temp.path("a.kinewright");
        let core = Core::for_test(document(2)).unwrap();
        let (mut owner, inputs) = start(&path, &core);
        let original = fs::read(&path).unwrap();
        grow(&core, 2);
        assert!(io_observer::fail_once("root_rename_io", || owner.save(&core, &inputs)).is_err());
        assert_eq!(owner.publication_state(), PublicationState::Unknown);
        let pending = staged(temp.root());
        assert_eq!(pending.len(), 1);
        assert_eq!(
            owner.store().pending_control_bytes_probe().unwrap(),
            fs::metadata(&pending[0]).unwrap().len()
        );
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(owner.reconcile().unwrap().is_some());
        assert!(
            !pending[0].exists(),
            "non-EACCES rename left an untracked temporary"
        );
        assert_eq!(owner.store().pending_control_bytes_probe().unwrap(), 0);
    }
    #[test]
    fn w12_named_owner_cleanup_allows_ordinary_fresh_copy() {
        let temp = temp("s2-named-cleanup");
        let a = temp.path("a");
        fs::create_dir(&a).unwrap();
        let path = a.join("a.kinewright");
        let core = Core::for_test(document(2)).unwrap();
        let (mut owner, inputs) = start(&path, &core);
        let mut saved = serde_json::to_value(core.export_history().unwrap()).unwrap();
        grow(&core, 2);
        let live = core.snapshot_identity().unwrap();
        let failure = io_observer::fail_once("root_file_sync", || {
            io_observer::fail_path("root_cleanup_unlink", &path, || owner.save(&core, &inputs))
        })
        .unwrap_err();
        assert!(
            owner.cleanup_pending(),
            "CLEANUP_ONLY_FAILURE_HAS_NO_PUBLICATION_TAG: {failure:?}; phase={:?}; tag={}",
            owner.publication_state(),
            owner.has_publication_tag_probe()
        );
        assert_eq!(owner.publication_state(), PublicationState::NeverAttempted);
        let charge = owner.store().pending_control_bytes_probe().unwrap();
        assert!(charge > 0);
        let hidden = temp.path("unreadable-a");
        fs::rename(&a, &hidden).unwrap();
        let b = temp.path("b.kinewright");
        let b_owner = copy(&mut owner, &b, StorageLimits::default());
        assert_eq!(core.snapshot_identity().unwrap(), live);
        assert!(owner.cleanup_pending());
        assert_eq!(owner.store().pending_control_bytes_probe().unwrap(), charge);
        assert_ne!(owner.project_id(), b_owner.project_id());
        let mut loaded = LoadedBundle::read(
            &b,
            &kinewright_project::operation_history::HistoryPool::new(
                kinewright_project::operation_history::PoolLimits::default(),
            ),
        )
        .unwrap();
        let reopened = loaded.open_manual().unwrap();
        let selected = reopened.export_history().unwrap();
        saved["history"]["epoch"] = serde_json::json!(selected.history().epoch);
        assert!(
            serde_json::to_value(&selected).unwrap() == saved,
            "ORDINARY_COPY_USES_ACKNOWLEDGED_GRAPH_WITHOUT_UNSAVED_LIVE_EDITS"
        );
        let reference = &owner.root().unwrap().branch_index_ref;
        let missing = hidden
            .join("a.kinewright-assets/studio/objects")
            .join(&reference.sha256);
        let held_missing = hidden.join("temporarily-missing-index");
        fs::rename(&missing, &held_missing).unwrap();
        let refused = temp.path("missing-bytes.kinewright");
        assert!(owner.save_as(&refused).is_err());
        assert!(
            !refused.exists(),
            "missing selected owned bytes cannot publish a partial copy"
        );
        assert_eq!(owner.store().pending_control_bytes_probe().unwrap(), charge);
        fs::rename(&held_missing, &missing).unwrap();
        fs::rename(&hidden, &a).unwrap();
        owner.reconcile().unwrap();
        assert!(staged(&a).is_empty());
        owner.save(&core, &inputs).unwrap();
        // An actual unknown publication must still reconcile A before another ordinary copy.
        io_observer::fail_once("root_parent_sync", || owner.save(&core, &inputs)).unwrap_err();
        fs::rename(&a, &hidden).unwrap();
        assert!(owner.save_as(&temp.path("forbidden.kinewright")).is_err());
        assert!(!temp.path("forbidden.kinewright").exists());
        fs::rename(&hidden, &a).unwrap();
        owner.reconcile().unwrap();
    }
    #[test]
    fn m03_pending_controls_have_an_exact_independent_charge() {
        let temp = temp("s2-exact-control-charge");
        let path = temp.path("a.kinewright");
        let core = Core::for_test(document(2)).unwrap();
        let (mut owner, inputs) = start(&path, &core);
        let held = owner.store().reserve_probe(1234, 0).unwrap();
        let unrelated_copy = owner.store().reserve_copy(1 << 20).unwrap();
        let before = owner.store().pending_control_bytes_probe().unwrap();
        assert_eq!(before, 0);
        io_observer::fail_once("root_file_sync", || {
            io_observer::fail_path("root_cleanup_unlink", &path, || owner.save(&core, &inputs))
        })
        .unwrap_err();
        let paths = staged(temp.root());
        assert_eq!(paths.len(), 1);
        let length = fs::metadata(&paths[0]).unwrap().len();
        assert_eq!(
            owner.store().pending_control_bytes_probe().unwrap() - before,
            length,
            "staged root's independent charge, not aggregate reservations"
        );
        assert!(owner.store().usage().unwrap().decoded_bytes >= 1234);
        assert_eq!(owner.store().usage().unwrap().reserved_copy_bytes, 1 << 20);
        assert!(
            length < (1 << 20),
            "unrelated reservation must mask a weak aggregate assertion"
        );
        drop(unrelated_copy);
        drop(held);
        owner.reconcile().unwrap();
        assert_eq!(owner.store().pending_control_bytes_probe().unwrap(), 0);
    }
    #[test]
    fn m04_cleanup_only_status_does_not_claim_unknown_root() {
        let temp = temp("s2-cleanup-status");
        let path = temp.path("a.kinewright");
        let core = Core::for_test(document(2)).unwrap();
        let mut session = SessionStore::create(temp.root(), StorageLimits::default()).unwrap();
        let inputs = session_inputs(&mut session);
        let mut copy = io_observer::fail_once("root_file_sync", || {
            io_observer::fail_path("root_cleanup_unlink", &path, || {
                session.first_save(&path, &core, &inputs)
            })
        })
        .unwrap();
        assert!(
            matches!(copy.status(), CopyStatus::CleanupPending { diagnostic } if diagnostic.contains("save failed; cleanup pending") && !diagnostic.contains("durability_unknown")),
            "{:?}",
            copy.status()
        );
        assert!(!path.exists());
        assert_eq!(
            core.attachment_state().unwrap(),
            kinewright_core::AttachmentState::Live
        );
        assert!(copy.reconcile().unwrap().is_none());
        assert!(staged(temp.root()).is_empty());
    }
    #[test]
    fn m05_tag_appears_only_after_floor_tagging_succeeds() {
        for fault in ["root_before_publish", "root_file_sync"] {
            let temp = temp("s2-tag-order");
            let path = temp.path("a.kinewright");
            let core = Core::for_test(document(200)).unwrap();
            let (owner, inputs) = start(&path, &core);
            drop(owner);
            let mut owner =
                LoadedBundle::read_with_limits_probe(&path, small_limits(), core.history_pool())
                    .unwrap()
                    .into_writer()
                    .unwrap();
            let prior = fs::read(&path).unwrap();
            grow(&core, 40);
            let before = core.export_history().unwrap();
            assert_eq!(before.history().undo.len(), 40);
            io_observer::fail_once(fault, || owner.save(&core, &inputs)).unwrap_err();
            assert!(
                !owner.has_publication_tag_probe(),
                "phantom publication tag after {fault}"
            );
            assert_eq!(owner.publication_state(), PublicationState::NeverAttempted);
            assert_eq!(fs::read(&path).unwrap(), prior);
            unchanged(&core, &before);
            owner.save(&core, &inputs).unwrap();
        }
    }
    #[test]
    fn m08_ordinary_save_staging_and_cleanup_fault_preserves_history() {
        let temp = temp("s2-ordinary-stage");
        let path = temp.path("a.kinewright");
        let core = Core::for_test(document(200)).unwrap();
        let (owner, inputs) = start(&path, &core);
        drop(owner);
        let mut owner =
            LoadedBundle::read_with_limits_probe(&path, small_limits(), core.history_pool())
                .unwrap()
                .into_writer()
                .unwrap();
        grow(&core, 40);
        apply(&core, Command::Undo);
        let before = core.export_history().unwrap();
        let prior = fs::read(&path).unwrap();
        let (result, trace) = io_observer::observe(|| {
            io_observer::fail_once("root_file_sync", || {
                io_observer::fail_path("root_cleanup_unlink", &path, || owner.save(&core, &inputs))
            })
        });
        assert!(result.is_err());
        assert!(!trace.overflowed);
        assert!(
            !trace
                .events
                .iter()
                .any(|e| e.operation == "root_rename_attempt")
        );
        unchanged(&core, &before);
        assert_eq!(fs::read(&path).unwrap(), prior);
        apply(&core, Command::Undo);
        apply(&core, Command::Redo);
        let roundtrip = core.export_history().unwrap();
        assert_eq!(roundtrip.history().document, before.history().document);
        assert_eq!(
            serde_json::to_value(&roundtrip.history().undo).unwrap(),
            serde_json::to_value(&before.history().undo).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&roundtrip.history().redo).unwrap(),
            serde_json::to_value(&before.history().redo).unwrap()
        );
        drop(roundtrip);
        owner.reconcile().unwrap();
        owner.save(&core, &inputs).unwrap();
        assert!(staged(temp.root()).is_empty());
        assert_eq!(
            core.attachment_state().unwrap(),
            kinewright_core::AttachmentState::Live
        );
        let saved = core.export_history().unwrap();
        assert_eq!(saved.history().document, before.history().document);
        assert_eq!(saved.history().redo, before.history().redo);
        drop(saved);
        drop(before);
        let b = temp.path("b.kinewright");
        drop(copy(&mut owner, &b, small_limits()));
        LoadedBundle::read_with_limits_probe(
            &b,
            small_limits(),
            &kinewright_project::operation_history::HistoryPool::new(
                kinewright_project::operation_history::PoolLimits::default(),
            ),
        )
        .unwrap();
    }
    #[test]
    fn w01_ordinary_first_save_phase_and_owned_bytes_control() {
        let temp = temp("s2-first-save-bytes");
        let core = Core::for_test(document(2)).unwrap();
        let mut session = SessionStore::create(temp.root(), StorageLimits::default()).unwrap();
        let bytes = b"independent owned source\0\r\n";
        let object = ObjectRef {
            sha256: kinewright_media::sha256_bytes(bytes),
            byte_length: bytes.len() as u64,
            record_kind: RecordKind::OwnedData,
        };
        let sealed = session
            .store_mut()
            .admit_blob(&mut &bytes[..], &object, &[])
            .unwrap();
        let inputs = session
            .admit_inputs(&OwnedInputManifest {
                project_id: None,
                session_id: Some(session.session_id().clone()),
                owner_epoch: session.store().owner_epoch(),
                entries: vec![OwnedInputEntry {
                    entry_id: "data".into(),
                    object: object.clone(),
                    dependencies: vec![],
                    selected_stream: None,
                    time_base: None,
                    origin: None,
                    profile: None,
                }],
            })
            .unwrap();
        let path = temp.path("a.kinewright");
        let mut unknown = io_observer::fail_once("root_parent_sync", || {
            session.first_save(&path, &core, &inputs.reference)
        })
        .unwrap();
        assert!(matches!(unknown.status(), CopyStatus::DurabilityUnknown));
        assert!(unknown.reconcile().unwrap().is_some());
        let mut owner = destination(unknown);
        assert!(
            owner
                .store()
                .read_input_manifest(&owner.root().unwrap().input_manifest_ref)
                .unwrap()
                .entries
                .iter()
                .any(|entry| entry.object == object),
            "first-save owned input binding missing"
        );
        let b = temp.path("b.kinewright");
        drop(copy(&mut owner, &b, StorageLimits::default()));
        let actual = fs::read(temp.path(&format!(
            "b.kinewright-assets/studio/objects/{}",
            object.sha256
        )))
        .unwrap();
        assert_eq!(actual, bytes);
        drop(sealed);
        drop(inputs);
        drop(session);
        drop(owner);
        LoadedBundle::read(
            &b,
            &kinewright_project::operation_history::HistoryPool::new(
                kinewright_project::operation_history::PoolLimits::default(),
            ),
        )
        .unwrap();
        let refused = temp.path("refused.kinewright");
        let mut session = SessionStore::create(temp.root(), StorageLimits::default()).unwrap();
        let input = session_inputs(&mut session);
        assert!(
            io_observer::fail_once("root_file_sync", || session
                .first_save(&refused, &core, &input))
            .is_err()
        );
        assert!(!refused.exists());
    }
    #[test]
    fn w10_pending_publication_phase_control() {
        for committed in [false, true] {
            let temp = temp("s2-pending-command");
            let path = temp.path("a.kinewright");
            let core = Core::for_test(document(2)).unwrap();
            let (mut owner, inputs) = start(&path, &core);
            let issuer = owner.issue_issuer(&core, &inputs, 1).unwrap();
            let Event::QueryResult(QueryResult::Snapshot { revision, .. }) =
                core.request(Command::Query(Query::Snapshot)).unwrap()
            else {
                panic!("snapshot")
            };
            let envelope = CommandEnvelope {
                switch: None,
                manual: None,
                context: None,
                version: 1,
                action: "edit".into(),
                id: CommandId {
                    issuer,
                    sequence: 1,
                },
                project_id: owner.project_id().clone(),
                target_branch: owner.manual_branch_id().clone(),
                owner_epoch: owner.store().owner_epoch(),
                lease: None,
                expected_head: owner.manual_head().cloned(),
                expected_revision: revision,
                command: Some(JournalCommand::Do(Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: false,
                })),
                comment_text: None,
                comment_strokes: None,
                evidence: vec![],
                transition: None,
            };
            let fault = if committed {
                "root_parent_sync"
            } else {
                "root_rename_io"
            };
            assert!(
                io_observer::fail_once(fault, || owner.command(&envelope, &core, &inputs, &[]))
                    .is_err()
            );
            assert_eq!(owner.publication_state(), PublicationState::Unknown);
            owner
                .install_reconciled_command(
                    &core,
                    owner.attach_core(&core).unwrap().capture().unwrap(),
                )
                .unwrap();
            let lookup = owner.receipt_lookup(&envelope).unwrap();
            assert_eq!(
                matches!(lookup, ReceiptLookup::Committed(_)),
                committed,
                "{lookup:?}"
            );
            assert!(staged(temp.root()).is_empty());
        }
    }

    #[test]
    fn w13_memo_is_discarded_after_namespace_continuity_failure() {
        let temp = temp("s2-memo-continuity");
        let path = temp.path("a.kinewright");
        let core = Core::for_test(document(4)).unwrap();
        grow(&core, 8);
        let (mut owner, inputs) = start(&path, &core);
        let before = core.export_history().unwrap();
        let assets = temp.path("a.kinewright-assets");
        let retained = temp.path("retained-assets");
        fs::rename(&assets, &retained).unwrap();
        fs::create_dir(&assets).unwrap();
        let (failed, invalidation) = io_observer::observe(|| owner.save(&core, &inputs));
        assert!(failed.is_err());
        assert!(
            invalidation
                .events
                .iter()
                .any(|event| event.operation == "publication_memo_reset"),
            "NAMESPACE_DISCONTINUITY_RESETS_RAW_IDENTITY_COSTS"
        );
        fs::remove_dir(&assets).unwrap();
        fs::rename(&retained, &assets).unwrap();
        let decoded = io_observer::history_decode_count();
        owner.save(&core, &inputs).unwrap();
        assert_eq!(
            io_observer::history_decode_count(),
            decoded,
            "IMMUTABLE_OWNER_PROOF_SURVIVES_RAW_MEMO_RESET_AFTER_BYTE_REVERIFICATION"
        );
        unchanged(&core, &before);
        LoadedBundle::read(
            &path,
            &kinewright_project::operation_history::HistoryPool::new(
                kinewright_project::operation_history::PoolLimits::default(),
            ),
        )
        .unwrap();
        assert!(
            io_observer::history_decode_count() > decoded,
            "INDEPENDENT_COLD_OPEN_STILL_VALIDATES_ALL_OWNER_SEMANTICS"
        );
    }

    #[test]
    fn w13_memo_rejects_wrong_object_ref_identity() {
        for wrong_kind in [false, true] {
            let temp = temp("s2-memo-object-ref");
            let path = temp.path("a.kinewright");
            let core = Core::for_test(document(4)).unwrap();
            grow(&core, 8);
            let (mut owner, inputs) = start(&path, &core);
            let manual = owner.manual_branch_id().clone();
            let archived = owner
                .fork_branch(
                    &manual,
                    kinewright_project::operation_history::Role::Ordinary,
                )
                .unwrap();
            let reference = owner.checkpoint(&archived).unwrap().owner_ref.clone();
            let mut wrong = reference.clone();
            if wrong_kind {
                wrong.record_kind = RecordKind::Document;
            } else {
                wrong.byte_length += 1;
            }
            owner
                .store()
                .misbind_verified_cost_probe(&reference, wrong)
                .unwrap();
            let decoded = io_observer::history_decode_count();
            let (saved, trace) = io_observer::observe(|| owner.save(&core, &inputs));
            saved.unwrap();
            assert!(
                io_observer::history_decode_count() > decoded,
                "wrong ObjectRef identity supplied a history charge"
            );
            assert!(
                trace
                    .events
                    .iter()
                    .any(|e| e.operation == "publication_memo_miss")
            );
            LoadedBundle::read(
                &path,
                &kinewright_project::operation_history::HistoryPool::new(
                    kinewright_project::operation_history::PoolLimits::default(),
                ),
            )
            .unwrap();
        }
    }

    #[test]
    fn w13_memo_matches_reader_after_changed_history_and_cache_misses() {
        let temp = temp("s2-memo-costs");
        let path = temp.path("a.kinewright");
        let core = Core::for_test(document(8)).unwrap();
        grow(&core, 8);
        let (mut owner, inputs) = start(&path, &core);
        let manual = owner.manual_branch_id().clone();
        for n in [12, 16] {
            let id = owner
                .fork_branch(
                    &manual,
                    kinewright_project::operation_history::Role::Ordinary,
                )
                .unwrap();
            let other = Core::for_test(
                (owner
                    .branch_history(&id)
                    .unwrap()
                    .into_capture()
                    .unwrap()
                    .history()
                    .document)
                    .clone(),
            )
            .unwrap();
            grow(&other, n);
            owner.save_branch(&id, &other).unwrap();
        }
        for (id, checkpoint) in owner.branches_probe() {
            if *id != manual {
                owner
                    .store()
                    .evict_verified_cost_probe(&checkpoint.owner_ref)
                    .unwrap();
            }
        }
        grow(&core, 4);
        let (saved, trace) = io_observer::observe(|| owner.save(&core, &inputs));
        saved.unwrap();
        assert!(
            trace
                .events
                .iter()
                .any(|e| e.operation == "publication_memo_miss")
        );
        assert!(
            trace
                .events
                .iter()
                .any(|e| e.operation == "publication_memo_hit")
        );
        let modeled = io_observer::publication_usage();
        let loaded = LoadedBundle::read(
            &path,
            &kinewright_project::operation_history::HistoryPool::new(
                kinewright_project::operation_history::PoolLimits::default(),
            ),
        )
        .unwrap();
        let actual = loaded.usage_probe().unwrap();
        assert_eq!(
            modeled.live_history_bytes, actual.live_history_bytes,
            "memo presence decided current-branch membership"
        );
        assert_eq!(
            modeled.peak_live_history_bytes, actual.peak_live_history_bytes,
            "stale history identity supplied a charge"
        );
        assert!(
            modeled.peak_decoded_bytes >= actual.peak_decoded_bytes,
            "PUBLICATION_PREFLIGHT_BOUNDS_REAL_COLD_METADATA_AND_DECODER_OVERLAP"
        );
        drop(loaded);
        let decoded = io_observer::history_decode_count();
        owner.save(&core, &inputs).unwrap();
        assert_eq!(
            io_observer::history_decode_count(),
            decoded,
            "warm unchanged save decoded histories"
        );
    }

    // The full fresh-open boundary and sparse-cache scenario lives here so one exact witness
    // can be rerun for each independent publisher-accounting mutation.
    #[test]
    fn w13_every_publication_fits_fresh_open() {
        fresh_open_boundaries();
    }

    fn fresh_open_boundaries() {
        use kinewright_project::operation_history::{HistoryPool, PoolLimits, Role};
        let temp = temp("s2-fresh-open-pool");
        let path = temp.path("project.kinewright");
        let core = Core::for_test(document(8)).unwrap();
        grow(&core, 8);
        let (mut source, inputs) = start(&path, &core);
        let manual = source.manual_branch_id().clone();
        let attachment = source.attach_core(&core).unwrap();
        let cut = attachment.freeze().unwrap();
        attachment.retire(&cut).unwrap();
        // Keep the original 24-branch/archive fixture with one active ordinary actor.
        let mut ids = vec![];
        for n in 0..24 {
            let id = source.fork_branch(&manual, Role::Ordinary).unwrap();
            let checkpoint = source.checkpoint(&id).unwrap();
            let other = Core::open(
                checkpoint
                    .open(source.store(), source.history_pool())
                    .unwrap(),
                id.clone(),
                checkpoint.revision,
            )
            .unwrap();
            grow(&other, 24 + n);
            source.save_branch(&id, &other).unwrap();
            let attachment = source.attach_core(&other).unwrap();
            let cut = attachment.freeze().unwrap();
            attachment.retire(&cut).unwrap();
            ids.push(id);
        }
        assert_eq!(ids.len(), 24);
        assert_eq!(
            source.cached_branch_count_probe(),
            0,
            "ARCHIVED_BRANCHES_KEEP_NO_RESIDENT_HISTORIES"
        );
        let checkpoint = source.checkpoint(&manual).unwrap();
        let live = Core::open(
            checkpoint
                .open(source.store(), source.history_pool())
                .unwrap(),
            manual,
            checkpoint.revision,
        )
        .unwrap();
        grow(&live, 20);
        apply(
            &live,
            Command::Do(Operation::AddMarker {
                marker: kinewright_core::Marker {
                    id: kinewright_core::MarkerId(9000),
                    position: kinewright_core::TimeCode(0),
                    label: "m".repeat(1 << 19),
                    color_token: 0,
                },
            }),
        );
        source.save(&live, &inputs).unwrap();
        let expected = live.snapshot_identity().unwrap();
        let root = fs::read(&path).unwrap();
        let ordinary = Role::Ordinary.envelope().retained;
        let mut short_limits = PoolLimits::default();
        short_limits.retained[0] = ordinary - 1;
        assert!(
            matches!(
                LoadedBundle::read(&path, &HistoryPool::new(short_limits)),
                Err(BundleError::Operation(
                    kinewright_project::operation_actor::Error::History(
                        kinewright_project::operation_history::Error::PoolFull(Role::Ordinary)
                    )
                ))
            ),
            "COLD_OPEN_REFUSES_BEFORE_EXCEEDING_ONE_FIXED_HOLDER"
        );
        assert_eq!(
            fs::read(&path).unwrap(),
            root,
            "REFUSED_COLD_OPEN_LEAVES_EVERY_SAVED_BRANCH_REACHABLE"
        );
        let mut two = PoolLimits::default();
        two.retained[0] = 2 * ordinary;
        two.peak[0] = two.retained[0] + two.workspace;
        let pool = HistoryPool::new(two);
        let mut loaded = LoadedBundle::read(&path, &pool).unwrap();
        let cold = loaded.open_manual().unwrap();
        assert_eq!(
            cold.snapshot_identity().unwrap(),
            expected,
            "PUBLISHED_OWNER_FITS_INDEPENDENT_FRESH_OPEN"
        );
        assert!(
            pool.lease_counts(Role::Ordinary).unwrap().1 <= 2,
            "COLD_OPEN_REPLAYS_ARCHIVED_HISTORIES_SEQUENTIALLY"
        );
        assert_eq!(loaded.root().unwrap().candidate_branch_ids.len(), 24);
        assert_eq!(fs::read(&path).unwrap(), root);
    }
}
