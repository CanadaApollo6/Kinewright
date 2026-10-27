//! Round-six outcome boundaries and copy-source settlement through public save paths.
#![cfg(target_os = "linux")]
#![allow(clippy::too_many_lines)]
use kinewright_core::*;
use kinewright_media::test_support::TempDirectory;
use kinewright_project::{
    bundle::{BundleWriteAdmission, ProjectBundle},
    bundle_commit::bound_history_probe,
    bundle_copy::{CopyStatus, SaveAsOutcome, SessionStore},
    bundle_records::ObjectRef,
    io_observer,
    owned_inputs::OwnedInputManifest,
    storage_budget::StorageLimits,
};
use std::{fs, path::Path};
fn temp(label: &str) -> TempDirectory {
    TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), label)
}
fn limits() -> StorageLimits {
    StorageLimits {
        branch_history_bytes: 1 << 20,
        ..StorageLimits::default()
    }
}
fn core() -> Core {
    Core::spawn(Document {
        tracks: (1..=200)
            .map(|id| Track {
                id: TrackId(id),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            })
            .collect(),
        ..Document::default()
    })
    .unwrap()
}
fn apply(core: &Core, command: Command) {
    assert!(matches!(
        core.request(command).unwrap(),
        Event::DocumentChanged { .. }
    ));
}
fn grow(core: &Core) {
    for i in 0..40 {
        apply(
            core,
            Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: i % 2 == 0,
            }),
        );
    }
}
fn inputs(session: &mut SessionStore) -> ObjectRef {
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
fn every_entry(core: &Core, before: &HistorySnapshot) {
    for entry in before.cursor.undo.iter().rev() {
        apply(core, Command::Undo);
        assert_eq!(
            core.export_history().unwrap().document,
            entry.document,
            "original Undo target unavailable"
        );
    }
    for _ in &before.cursor.undo {
        apply(core, Command::Redo);
    }
    assert_eq!(core.export_history().unwrap().document, before.document);
    for entry in before.cursor.redo.iter().rev() {
        apply(core, Command::Redo);
        assert_eq!(
            core.export_history().unwrap().document,
            entry.document,
            "original Redo target unavailable"
        );
    }
}
#[test]
fn root_staging_and_cleanup_failure_releases_first_save_to_another_destination() {
    let temp = temp("r6-root-stage");
    let a_dir = temp.path("a");
    fs::create_dir(&a_dir).unwrap();
    let path = a_dir.join("prior.kinewright");
    let original = kinewright_project::serialize_project_document(&Document::default())
        .unwrap()
        .into_bytes();
    fs::write(&path, &original).unwrap();
    let core = core();
    grow(&core);
    for _ in 0..3 {
        apply(&core, Command::Undo);
    }
    let before = core.export_history().unwrap();
    assert!(
        bound_history_probe(&before, &limits())
            .unwrap()
            .floor_offset
            > 0
    );
    let mut session = SessionStore::create(temp.root(), StorageLimits::default()).unwrap();
    let input = inputs(&mut session);
    let (copy, trace) = io_observer::observe(|| {
        io_observer::fail_once("root_file_sync", || {
            io_observer::fail_path("root_cleanup_unlink", &path, || {
                session.first_save_with_limits(&path, &core, &input, limits())
            })
        })
    });
    let mut copy = copy.unwrap();
    assert!(matches!(copy.status(), CopyStatus::CleanupPending { .. }));
    assert!(!trace.overflowed);
    assert_eq!(
        trace
            .events
            .iter()
            .filter(|e| e.operation == "injected_failure")
            .count(),
        2
    );
    assert!(
        !trace
            .events
            .iter()
            .any(|e| e.operation == "root_rename_attempt" || e.operation == "root_rename")
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    let staged: Vec<_> = fs::read_dir(&a_dir)
        .unwrap()
        .map(Result::unwrap)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert_eq!(staged.len(), 1, "unresolved staging must stay with A");
    let charged = copy.destination_usage_probe().unwrap();
    assert!(charged.unsettled);
    assert!(
        charged.reserved_bytes >= staged[0].metadata().unwrap().len(),
        "unresolved staging lost its charge"
    );
    assert!(
        core.pending_history_save().unwrap().is_none(),
        "prepublication staging cleanup retained Core's ticket"
    );
    assert_eq!(core.export_history().unwrap(), before);
    every_entry(&core, &before);
    let hidden = temp.path("a-unavailable");
    fs::rename(&a_dir, &hidden).unwrap();
    let b = temp.path("b.kinewright");
    let next = session
        .first_save_with_limits(&b, &core, &input, limits())
        .expect("A cleanup must not gate Core or the copy source");
    assert!(matches!(next.status(), CopyStatus::Committed(_)));
    assert_eq!(fs::read(hidden.join("prior.kinewright")).unwrap(), original);
    // A's owner, rather than Core, retains the charge and retries the temporary cleanup.
    fs::rename(&hidden, &a_dir).unwrap();
    assert!(copy.reconcile().unwrap().is_none());
    assert!(
        !staged[0].path().exists(),
        "A's cleanup must retry its unresolved temporary"
    );
}
#[test]
fn reconciled_first_copy_does_not_gate_the_session_on_its_old_destination() {
    let temp = temp("r6-session-copy");
    let a = temp.path("a");
    fs::create_dir(&a).unwrap();
    let path = a.join("first.kinewright");
    let core = core();
    grow(&core);
    let mut session = SessionStore::create(temp.root(), StorageLimits::default()).unwrap();
    let input = inputs(&mut session);
    let mut copy = io_observer::fail_once("root_parent_sync", || {
        session.first_save_with_limits(&path, &core, &input, limits())
    })
    .unwrap();
    assert!(matches!(copy.status(), CopyStatus::DurabilityUnknown));
    assert!(copy.reconcile().unwrap().is_some());
    assert!(core.pending_history_save().unwrap().is_none());
    fs::rename(&a, temp.path("gone")).unwrap();
    let next = session
        .first_save_with_limits(&temp.path("b.kinewright"), &core, &input, limits())
        .expect("settled first copy left a stale source tag");
    assert!(matches!(next.status(), CopyStatus::Committed(_)));
}
#[test]
fn reconciled_save_as_does_not_gate_the_source_owners_next_save() {
    let temp = temp("r6-named-copy");
    let path = temp.path("source.kinewright");
    let mut source = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    let core = core();
    grow(&core);
    let input = source.empty_inputs().unwrap().reference.clone();
    source.save(&core, &input).unwrap();
    let b = temp.path("b");
    fs::create_dir(&b).unwrap();
    let SaveAsOutcome::Copied(mut copy) = io_observer::fail_once("root_parent_sync", || {
        source.save_as(&b.join("copy.kinewright"))
    })
    .unwrap() else {
        panic!("copied destination")
    };
    assert!(matches!(copy.status(), CopyStatus::DurabilityUnknown));
    assert!(copy.reconcile().unwrap().is_some());
    fs::rename(&b, temp.path("gone")).unwrap();
    source
        .save(&core, &input)
        .expect("settled Save As left a stale source tag");
}

#[test]
fn durable_floor_uses_only_the_current_checkpoint_history_cache() {
    use kinewright_project::bundle::LoadedBundle;
    let temp = temp("r6-history-cache");
    let path = temp.path("cached.kinewright");
    let core = core();
    grow(&core);
    let mut owner = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    let input = owner.empty_inputs().unwrap().reference.clone();
    owner.save(&core, &input).unwrap();
    for expected_reads in [0, 0, 1, 0] {
        if expected_reads == 1 {
            owner.invalidate_history_cache_binding_probe();
        }
        apply(
            &core,
            Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: expected_reads == 1,
            }),
        );
        let count = io_observer::history_decode_count();
        io_observer::at_barrier(
            "history_selected",
            move || {
                assert_eq!(
                    io_observer::history_decode_count() - count,
                    expected_reads,
                    "durable-floor cache must match the current immutable checkpoint reference"
                );
            },
            || owner.save(&core, &input),
        )
        .unwrap();
    }
    drop(owner);
    let mut owner = LoadedBundle::read(&path).unwrap().into_writer().unwrap();
    let input = owner
        .checkpoint(owner.manual_branch_id())
        .unwrap()
        .input_manifest_ref
        .clone();
    let count = io_observer::history_decode_count();
    io_observer::at_barrier(
        "history_selected",
        move || {
            assert_eq!(
                io_observer::history_decode_count(),
                count,
                "reopen did not bind its verified cache"
            );
        },
        || owner.save(&core, &input),
    )
    .unwrap();
    assert_eq!(
        core.export_history().unwrap(),
        *owner.branch_history(owner.manual_branch_id()).unwrap()
    );
}

#[test]
fn pending_copy_source_reconciles_before_the_next_first_save() {
    let temp = temp("r6-pending-copy");
    let core = core();
    grow(&core);
    let mut session = SessionStore::create(temp.root(), StorageLimits::default()).unwrap();
    let input = inputs(&mut session);
    let copy = io_observer::fail_once("root_parent_sync", || {
        session.first_save_with_limits(&temp.path("a.kinewright"), &core, &input, limits())
    })
    .unwrap();
    assert!(matches!(copy.status(), CopyStatus::DurabilityUnknown));
    let next = session
        .first_save_with_limits(&temp.path("b.kinewright"), &core, &input, limits())
        .expect("pending source did not reconcile before retry");
    assert!(matches!(next.status(), CopyStatus::Committed(_)));
    assert!(!session.store().usage().unwrap().unsettled);
}
