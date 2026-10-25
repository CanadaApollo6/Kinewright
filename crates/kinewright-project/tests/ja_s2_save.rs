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
    let (result, trace) = io_observer::observe(|| {
        ProjectBundle::activate(&path, BundleWriteAdmission::new_document())
    });
    assert!(matches!(result, Err(BundleError::DurabilityUnsupported)));
    assert!(trace.events.is_empty());
    assert!(!path.exists());
}

#[cfg(target_os = "linux")]
mod linux {
    use kinewright_core::{
        Command, Core, Document, Event, HistorySnapshot, JournalCommand, Operation, Query,
        QueryResult, TimelineRevision, Track, TrackId, TrackKind,
    };
    use kinewright_media::test_support::TempDirectory;
    use kinewright_project::{
        bundle::{
            BundleError, BundleWriteAdmission, LoadedBundle, ProjectBundle, PublicationState,
        },
        bundle_commit::{BundleCommit, CommitOutcome, bound_history_probe, history_cost_probe},
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
        let mut owner =
            ProjectBundle::activate(path, BundleWriteAdmission::new_document()).unwrap();
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
    fn unchanged(core: &Core, history: &HistorySnapshot) {
        assert!(
            core.export_history().unwrap() == *history,
            "live history changed"
        );
        assert!(core.pending_history_save().unwrap().is_none());
    }
    #[test]
    fn w12_post_b5_floor_error_keeps_publication_phase() {
        let temp = temp("s2-b5-phase");
        let path = temp.path("a.kinewright");
        let initial = Core::spawn(document(200)).unwrap();
        let (owner, inputs) = start(&path, &initial);
        drop(owner);
        let mut owner = LoadedBundle::read_with_limits_probe(&path, small_limits())
            .unwrap()
            .into_writer()
            .unwrap();
        // Revision exhaustion reaches the real actor's CommitInvariant after B5, without a fake error.
        let core = Core::spawn_at(document(200), TimelineRevision(u64::MAX - 40)).unwrap();
        grow(&core, 40);
        let before = core.export_history().unwrap();
        assert!(
            bound_history_probe(&before, &small_limits())
                .unwrap()
                .floor_offset
                > 0
        );
        let outcome = BundleCommit::run(&mut owner, &core, &inputs);
        assert!(
            matches!(
                outcome,
                CommitOutcome::CommittedWithError(BundleError::HistoryCommitInvariant)
            ),
            "{outcome:?}"
        );
        assert_eq!(owner.publication_state(), PublicationState::Committed);
        unchanged(&core, &before);
        let loaded = LoadedBundle::read_with_limits_probe(&path, small_limits()).unwrap();
        assert_eq!(
            loaded.admission().inspection_document(),
            Some(&*before.document)
        );
        assert!(loaded.manual_checkpoint().unwrap().floor_generation > 0);
        let mut session = SessionStore::create(temp.root(), StorageLimits::default()).unwrap();
        let source_inputs = session_inputs(&mut session);
        let mut copied = session
            .first_save_with_limits(
                &temp.path("copy.kinewright"),
                &core,
                &source_inputs,
                small_limits(),
            )
            .unwrap();
        assert!(
            matches!(copied.status(), CopyStatus::CommittedWithError { .. }),
            "{:?}",
            copied.status()
        );
        unchanged(&core, &before);
        let copy_path = temp.path("copy.kinewright");
        let committed_root = fs::read(&copy_path).unwrap();
        fs::remove_file(&copy_path).unwrap();
        assert!(matches!(copied.reconcile(), Err(BundleError::Corrupt(_))));
        assert!(matches!(
            copied.status(),
            CopyStatus::CommittedWithError { .. }
        ));
        unchanged(&core, &before);
        fs::write(&copy_path, committed_root).unwrap();
        assert!(copied.reconcile().unwrap().is_some());
    }
    #[test]
    fn w12_nonpermission_rename_retains_charged_temporary() {
        let temp = temp("s2-rename-io");
        let path = temp.path("a.kinewright");
        let core = Core::spawn(document(2)).unwrap();
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
        let core = Core::spawn(document(2)).unwrap();
        let (mut owner, inputs) = start(&path, &core);
        let saved = core.export_history().unwrap();
        grow(&core, 2);
        let live = core.export_history().unwrap();
        io_observer::fail_once("root_file_sync", || {
            io_observer::fail_path("root_cleanup_unlink", &path, || owner.save(&core, &inputs))
        })
        .unwrap_err();
        assert!(owner.cleanup_pending());
        assert_eq!(owner.publication_state(), PublicationState::NeverAttempted);
        let charge = owner.store().pending_control_bytes_probe().unwrap();
        assert!(charge > 0);
        let hidden = temp.path("unreadable-a");
        fs::rename(&a, &hidden).unwrap();
        let b = temp.path("b.kinewright");
        let b_owner = copy(&mut owner, &b, StorageLimits::default());
        unchanged(&core, &live);
        assert!(owner.cleanup_pending());
        assert_eq!(owner.store().pending_control_bytes_probe().unwrap(), charge);
        assert_ne!(owner.project_id(), b_owner.project_id());
        let reopened = Core::spawn(Document::default()).unwrap();
        LoadedBundle::read(&b)
            .unwrap()
            .restore_manual(&reopened)
            .unwrap();
        assert_eq!(
            reopened.export_history().unwrap(),
            saved,
            "ordinary copy uses acknowledged graph, not an implicit rescue of live edits"
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
        let core = Core::spawn(document(2)).unwrap();
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
        let core = Core::spawn(document(2)).unwrap();
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
        assert!(core.pending_history_save().unwrap().is_none());
        assert!(copy.reconcile().unwrap().is_none());
        assert!(staged(temp.root()).is_empty());
    }
    #[test]
    fn m05_tag_appears_only_after_floor_tagging_succeeds() {
        for fault in [
            "root_before_publish",
            "floor_tag_prepare",
            "floor_tag_complete",
        ] {
            let temp = temp("s2-tag-order");
            let path = temp.path("a.kinewright");
            let core = Core::spawn(document(200)).unwrap();
            let (owner, inputs) = start(&path, &core);
            drop(owner);
            let mut owner = LoadedBundle::read_with_limits_probe(&path, small_limits())
                .unwrap()
                .into_writer()
                .unwrap();
            let prior = fs::read(&path).unwrap();
            grow(&core, 40);
            let before = core.export_history().unwrap();
            assert!(
                bound_history_probe(&before, &small_limits())
                    .unwrap()
                    .floor_offset
                    > 0
            );
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
        let core = Core::spawn(document(200)).unwrap();
        let (owner, inputs) = start(&path, &core);
        drop(owner);
        let mut owner = LoadedBundle::read_with_limits_probe(&path, small_limits())
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
        assert_eq!(core.export_history().unwrap().cursor, before.cursor);
        owner.reconcile().unwrap();
        owner.save(&core, &inputs).unwrap();
        assert!(staged(temp.root()).is_empty());
        assert!(core.pending_history_save().unwrap().is_none());
        let saved = core.export_history().unwrap();
        assert_eq!(saved.document, before.document);
        assert_eq!(saved.cursor.redo, before.cursor.redo);
        let b = temp.path("b.kinewright");
        drop(copy(&mut owner, &b, small_limits()));
        LoadedBundle::read_with_limits_probe(&b, small_limits()).unwrap();
    }
    #[test]
    fn w01_ordinary_first_save_phase_and_owned_bytes_control() {
        let temp = temp("s2-first-save-bytes");
        let core = Core::spawn(document(2)).unwrap();
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
        LoadedBundle::read(&b).unwrap();
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
            let core = Core::spawn(document(2)).unwrap();
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
                .install_reconciled_command(&core, core.export_history().unwrap())
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
        let core = Core::spawn(document(4)).unwrap();
        grow(&core, 8);
        let (mut owner, inputs) = start(&path, &core);
        let before = core.export_history().unwrap();
        let assets = temp.path("a.kinewright-assets");
        let retained = temp.path("retained-assets");
        fs::rename(&assets, &retained).unwrap();
        fs::create_dir(&assets).unwrap();
        assert!(owner.save(&core, &inputs).is_err());
        fs::remove_dir(&assets).unwrap();
        fs::rename(&retained, &assets).unwrap();
        let decoded = io_observer::history_decode_count();
        let (saved, trace) = io_observer::observe(|| owner.save(&core, &inputs));
        saved.unwrap();
        assert!(
            trace
                .events
                .iter()
                .any(|e| e.operation == "publication_memo_miss"),
            "namespace discontinuity retained the memo"
        );
        assert!(
            io_observer::history_decode_count() > decoded,
            "restored namespace was not reverified"
        );
        unchanged(&core, &before);
        LoadedBundle::read(&path).unwrap();
    }

    #[test]
    fn w13_memo_rejects_wrong_object_ref_identity() {
        for wrong_kind in [false, true] {
            let temp = temp("s2-memo-object-ref");
            let path = temp.path("a.kinewright");
            let core = Core::spawn(document(4)).unwrap();
            grow(&core, 8);
            let (mut owner, inputs) = start(&path, &core);
            let reference = owner
                .checkpoint(owner.manual_branch_id())
                .unwrap()
                .history_ref
                .clone();
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
            LoadedBundle::read(&path).unwrap();
        }
    }

    #[test]
    fn w13_memo_matches_reader_after_changed_history_and_cache_misses() {
        let temp = temp("s2-memo-costs");
        let path = temp.path("a.kinewright");
        let core = Core::spawn(document(8)).unwrap();
        grow(&core, 8);
        let (mut owner, inputs) = start(&path, &core);
        let manual = owner.manual_branch_id().clone();
        for n in [12, 16] {
            let id = owner.fork_branch(&manual).unwrap();
            let other =
                Core::spawn((*owner.branch_history(&id).unwrap().document).clone()).unwrap();
            grow(&other, n);
            owner.save_branch(&id, &other).unwrap();
        }
        for (id, checkpoint) in owner.branches_probe() {
            if *id != manual {
                owner
                    .store()
                    .evict_verified_cost_probe(&checkpoint.history_ref)
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
        let loaded = LoadedBundle::read(&path).unwrap();
        let actual = loaded.usage_probe().unwrap();
        assert_eq!(
            modeled.live_history_bytes, actual.live_history_bytes,
            "memo presence decided current-branch membership"
        );
        assert_eq!(
            modeled.peak_live_history_bytes, actual.peak_live_history_bytes,
            "stale history identity supplied a charge"
        );
        assert_eq!(
            modeled.peak_decoded_bytes, actual.peak_decoded_bytes,
            "memo omitted reader scratch/decode overlap"
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
        use kinewright_core::HistoryTransition;
        use std::collections::BTreeMap;
        let temp = temp("s2-fresh-open");
        let path = temp.path("a.kinewright");
        let core = Core::spawn(document(8)).unwrap();
        grow(&core, 8);
        let (mut source, _inputs) = start(&path, &core);
        let manual = source.manual_branch_id().clone();
        let first = source.fork_branch(&manual).unwrap();
        let second = source.fork_branch(&manual).unwrap();
        assert_eq!(
            source.checkpoint(&first).unwrap().history_ref,
            source.checkpoint(&second).unwrap().history_ref
        );
        let shared = LoadedBundle::read(&path)
            .unwrap()
            .usage_probe()
            .unwrap()
            .peak_live_history_bytes;
        let shared_model_path = temp.path("shared-model.kinewright");
        let shared_model = copy(&mut source, &shared_model_path, StorageLimits::default());
        let shared_loaded = LoadedBundle::read(&shared_model_path).unwrap();
        let shared_usage = shared_loaded.usage_probe().unwrap();
        let expected_residents = history_cost_probe(
            &source.branch_history(&manual).unwrap(),
            &StorageLimits::default(),
        )
        .unwrap()
            + history_cost_probe(
                &source.branch_history(&first).unwrap(),
                &StorageLimits::default(),
            )
            .unwrap();
        assert_eq!(
            shared_usage.live_history_bytes, expected_residents,
            "matching current history hashes share exactly one resident charge"
        );
        let shared_profile = StorageLimits {
            live_history_bytes: shared_usage.peak_live_history_bytes,
            ..StorageLimits::default()
        };
        LoadedBundle::read_with_limits_probe(&shared_model_path, shared_profile.clone()).unwrap();
        drop(shared_loaded);
        drop(shared_model);
        let shared_exact_path = temp.path("shared-exact.kinewright");
        let shared_exact = source
            .save_as_with_limits(&shared_exact_path, shared_profile.clone())
            .unwrap_or_else(|error| {
                panic!("shared-history exact-fit publication refused: {error}")
            });
        let SaveAsOutcome::Copied(shared_exact) = shared_exact else {
            panic!("fresh shared-history copy")
        };
        assert!(matches!(shared_exact.status(), CopyStatus::Committed(_)));
        drop(destination(*shared_exact));
        LoadedBundle::read_with_limits_probe(&shared_exact_path, shared_profile).unwrap();
        let branch_core =
            Core::spawn((*source.branch_history(&first).unwrap().document).clone()).unwrap();
        grow(&branch_core, 20);
        source.save_branch(&first, &branch_core).unwrap();
        let distinct = LoadedBundle::read(&path)
            .unwrap()
            .usage_probe()
            .unwrap()
            .peak_live_history_bytes;
        assert!(distinct > shared);
        LoadedBundle::read_with_limits_probe(
            &path,
            StorageLimits {
                live_history_bytes: distinct,
                ..StorageLimits::default()
            },
        )
        .unwrap();
        assert!(matches!(
            LoadedBundle::read_with_limits_probe(
                &path,
                StorageLimits {
                    live_history_bytes: distinct - 1,
                    ..StorageLimits::default()
                }
            ),
            Err(BundleError::Capacity(_))
        ));
        let mut ids = vec![first, second];
        for n in 0..21 {
            let id = source.fork_branch(&manual).unwrap();
            let other =
                Core::spawn((*source.branch_history(&id).unwrap().document).clone()).unwrap();
            grow(&other, 24 + n);
            source.save_branch(&id, &other).unwrap();
            ids.push(id);
        }
        let mut charges = BTreeMap::new();
        for id in &ids {
            let checkpoint = source.checkpoint(id).unwrap();
            charges
                .entry(checkpoint.history_ref.sha256.clone())
                .or_insert_with(|| {
                    history_cost_probe(
                        &source.branch_history(id).unwrap(),
                        &StorageLimits::default(),
                    )
                    .unwrap()
                });
        }
        let other_residents: u64 = charges.values().sum();
        // A genuinely larger immutable current document makes even the cheapest legal floor
        // exceed the later limit; ordinary trimming cannot hide target-only accounting.
        let additions: Vec<_> = (100..1900)
            .map(|id| Operation::AddTrack {
                track: Track {
                    id: TrackId(id),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![],
                },
            })
            .collect();
        let model_path = temp.path("model.kinewright");
        let mut model = copy(&mut source, &model_path, StorageLimits::default());
        let initial_copy_peak = LoadedBundle::read(&model_path)
            .unwrap()
            .usage_probe()
            .unwrap()
            .peak_live_history_bytes;
        let model_input = model.root().unwrap().input_manifest_ref.clone();
        grow(&core, 20);
        model.save(&core, &model_input).unwrap();
        let first_write_cost =
            history_cost_probe(&core.export_history().unwrap(), &StorageLimits::default()).unwrap();
        apply(&core, Command::DoBatch(additions.clone()));
        let candidate = core.export_history().unwrap();
        let minimum = (0..=candidate.batches.len())
            .filter_map(|cut| {
                let history = if cut == 0 {
                    candidate.clone()
                } else {
                    HistoryTransition::floor(candidate.clone(), cut)
                        .ok()?
                        .after()
                        .clone()
                };
                Some(history_cost_probe(&history, &StorageLimits::default()).unwrap())
            })
            .min()
            .unwrap();
        model.save(&core, &model_input).unwrap();
        let peak = LoadedBundle::read(&model_path)
            .unwrap()
            .usage_probe()
            .unwrap()
            .peak_live_history_bytes;
        let irreducible = other_residents + minimum;
        let actual_write_overlap = first_write_cost
            + 2 * history_cost_probe(&candidate, &StorageLimits::default()).unwrap();
        assert!(
            irreducible > actual_write_overlap,
            "the real sparse writer must fit before the independent reader binds"
        );
        assert!(peak >= irreducible);
        assert!(
            irreducible > initial_copy_peak,
            "setup copy must fit below the unavoidable later candidate"
        );
        eprintln!(
            "fresh-open peak={peak}; unavoidable current histories={irreducible}; other distinct current histories={other_residents}; cheapest legal target floor={minimum}"
        );
        let profiles = [peak, irreducible - 1, peak - 1];
        let mut writers = Vec::new();
        for (index, limit) in profiles.into_iter().enumerate() {
            let path = temp.path(&format!("candidate-{index}.kinewright"));
            let profile = StorageLimits {
                live_history_bytes: limit,
                ..StorageLimits::default()
            };
            let writer = copy(&mut source, &path, profile.clone());
            assert_eq!(writer.cached_branch_count_probe(), 1);
            writers.push((path, profile, writer));
        }
        drop(model);
        drop(source);
        drop(branch_core);
        drop(core);
        assert_eq!(
            charges.len(),
            ids.len(),
            "different current branch histories have distinct verified hashes"
        );
        for (index, (path, profile, mut writer)) in writers.into_iter().enumerate() {
            let input = writer.root().unwrap().input_manifest_ref.clone();
            let live = Core::spawn(document(8)).unwrap();
            grow(&live, 8);
            grow(&live, 20);
            writer.save(&live, &input).unwrap();
            assert_eq!(writer.cached_branch_count_probe(), 1);
            let prior = fs::read(&path).unwrap();
            apply(&live, Command::DoBatch(additions.clone()));
            let before = live.export_history().unwrap();
            for (branch, checkpoint) in writer.branches_probe() {
                if branch != writer.manual_branch_id() {
                    writer
                        .store()
                        .evict_verified_cost_probe(&checkpoint.history_ref)
                        .unwrap();
                }
            }
            eprintln!(
                "candidate-{index} limit={} actual live={} target cost={}",
                profile.live_history_bytes,
                writer.store().usage().unwrap().live_history_bytes,
                history_cost_probe(&before, &profile).unwrap()
            );
            let (result, trace) = io_observer::observe(|| writer.save(&live, &input));
            assert!(!trace.overflowed);
            assert!(
                !trace
                    .events
                    .iter()
                    .any(|e| e.operation == "bundle_reconcile")
            );
            if index == 0 {
                result.expect("exact fresh-open peak must publish");
                LoadedBundle::read_with_limits_probe(&path, profile)
                    .expect("exact candidate independently opens");
            } else {
                if result.is_ok() {
                    // Keep this oracle first: the target-only mutant must reach publication,
                    // then fail independent Open, rather than some structural/hash assertion.
                    LoadedBundle::read_with_limits_probe(&path, profile.clone()).expect(
                        "published candidate cannot exceed independent fresh-open capacity",
                    );
                    panic!("one-byte-over candidate was published");
                }
                assert!(
                    matches!(result, Err(BundleError::Capacity(_))),
                    "{result:?}"
                );
                assert_eq!(fs::read(&path).unwrap(), prior);
                unchanged(&live, &before);
                assert!(
                    trace
                        .events
                        .iter()
                        .any(|e| e.operation == "publication_reopen_fit"),
                    "actual writer reservations masked the fresh-open bound: {result:?}; events={:?}",
                    trace.events.iter().map(|e| e.operation).collect::<Vec<_>>()
                );
                assert!(
                    !trace
                        .events
                        .iter()
                        .any(|e| e.operation == "root_rename_attempt")
                );
                LoadedBundle::read_with_limits_probe(&path, profile).unwrap();
            }
        }
    }
}
