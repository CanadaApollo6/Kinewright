//! Round-3 scenarios use the public storage and Core boundaries.
#![allow(clippy::too_many_lines)]
use kinewright_core::*;
use kinewright_media::test_support::TempDirectory;
use kinewright_project::operation_actor::Core;
use kinewright_project::{
    bundle::{BundleWriteAdmission, LoadedBundle, ProjectBundle},
    bundle_copy::SessionStore,
    bundle_records::{ObjectRef, RecordKind},
    io_observer,
    owned_inputs::{OwnedInputEntry, OwnedInputManifest},
    storage_budget::StorageLimits,
};
use std::{fs, path::Path, time::Instant};

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
fn lock(locked: bool) -> Command {
    Command::Do(Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked,
    })
}
fn manifest(owner: &ProjectBundle, entries: Vec<OwnedInputEntry>) -> OwnedInputManifest {
    OwnedInputManifest {
        project_id: Some(owner.project_id().clone()),
        session_id: None,
        owner_epoch: owner.store().owner_epoch(),
        entries,
    }
}
fn first_save_over_limit(batch_limit: bool) {
    let temp = temp("r3-first-cap");
    let core = Core::for_test_with_bounds(
        document(if batch_limit { 1 } else { 2000 }),
        kinewright_core::undo::RoleBounds {
            step_cap: 8,
            ..kinewright_core::undo::RoleBounds::ORDINARY
        },
    )
    .unwrap();
    let count = if batch_limit { 9 } else { 500 };
    for i in 0..count {
        apply(&core, lock(i % 2 == 0));
    }
    let original = core.export_history().unwrap();
    assert!(
        original.fold_frontier().is_some(),
        "ONLY_THE_EXECUTOR_HAS_ALREADY_FOLDED_THE_PREFIX"
    );
    let mut session = SessionStore::create(temp.root(), StorageLimits::default()).unwrap();
    let inputs = session
        .admit_inputs(&OwnedInputManifest {
            project_id: None,
            session_id: Some(session.session_id().clone()),
            owner_epoch: session.store().owner_epoch(),
            entries: vec![],
        })
        .unwrap();
    let path = temp.path("first.kinewright");
    // A definite failure must preserve the original unsaved stream, including below its future floor.
    let failed = io_observer::fail_once("copy_verify", || {
        session.first_save(&path, &core, &inputs.reference)
    });
    assert!(failed.is_err());
    assert_eq!(core.export_history().unwrap(), original);
    assert!(!path.exists());
    let original_document = std::sync::Arc::clone(&original.history().document);
    let original_position = original.history().position;
    drop(original);
    let started = Instant::now();
    let copy = session
        .first_save(&path, &core, &inputs.reference)
        .expect("history growth must not refuse first save");
    let elapsed = started.elapsed();
    let owner = copy
        .into_destination()
        .unwrap_or_else(|_| panic!("first-save handoff pending"));
    let saved = core.export_history().unwrap();
    assert!(saved.fold_frontier().map_or(0, |key| key.position + 1) > 0);
    assert_eq!(saved.history().document, original_document);
    super::round_two::assert_selected_epoch(
        &saved,
        &owner
            .branch_history(owner.manual_branch_id())
            .unwrap()
            .into_capture()
            .unwrap(),
    );
    eprintln!(
        "R3_FIRST_SAVE batch_limit={batch_limit} original_position={} retained_steps={} fold_frontier={} elapsed_ms={:.3}",
        original_position,
        saved.history().undo.len(),
        saved.fold_frontier().map_or(0, |key| key.position + 1),
        elapsed.as_secs_f64() * 1000.0
    );
    drop(owner);
    let mut loaded = LoadedBundle::read(
        &path,
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .unwrap();
    let reopened = loaded.open_manual().unwrap();
    super::round_two::assert_selected_epoch(&saved, &reopened.export_history().unwrap());
    for _ in 0..saved.history().undo.len() {
        apply(&reopened, Command::Undo);
    }
    let floor = reopened.export_history().unwrap();
    apply(&reopened, Command::Undo);
    let stopped = reopened.export_history().unwrap();
    assert_eq!(floor.history().document, stopped.history().document);
    assert_eq!(floor.history().undo, stopped.history().undo);
    assert_eq!(floor.history().redo, stopped.history().redo);
    assert_eq!(stopped.history().position, floor.history().position + 1);
}
#[test]
fn first_save_floors_history_above_default_live_limit() {
    first_save_over_limit(false);
}
#[test]
fn first_save_floors_history_above_default_batch_limit() {
    first_save_over_limit(true);
}

#[test]
fn first_save_unknown_floor_handoff_reconciles_once() {
    let temp = temp("r3-first-unknown");
    let core = Core::for_test(document(200)).unwrap();
    for i in 0..40 {
        apply(&core, lock(i % 2 == 0));
    }
    let before = core.export_history().unwrap();
    let mut session = SessionStore::create(temp.root(), StorageLimits::default()).unwrap();
    let inputs = session
        .admit_inputs(&OwnedInputManifest {
            project_id: None,
            session_id: Some(session.session_id().clone()),
            owner_epoch: session.store().owner_epoch(),
            entries: vec![],
        })
        .unwrap();
    let path = temp.path("first.kinewright");
    let mut copy = io_observer::fail_once("root_parent_sync", || {
        session.first_save_with_limits(
            &path,
            &core,
            &inputs.reference,
            StorageLimits {
                ..StorageLimits::default()
            },
        )
    })
    .unwrap();
    assert!(matches!(
        copy.status(),
        kinewright_project::bundle_copy::CopyStatus::DurabilityUnknown
    ));
    assert!(
        path.exists(),
        "the uncertain root must actually have landed"
    );
    assert_eq!(core.export_history().unwrap(), before);
    drop(before);
    let report = copy
        .reconcile()
        .expect("floor handoff must install exactly once")
        .unwrap();
    let retry = copy.reconcile().unwrap().unwrap();
    assert_eq!(retry.root_sha256, report.root_sha256);
    assert_eq!(retry.generation, report.generation);
    let mut owner = copy
        .into_destination()
        .unwrap_or_else(|_| panic!("handoff remained uncertain"));
    let saved = core.export_history().unwrap();
    assert!(
        saved.fold_frontier().is_none(),
        "SAVE_DOES_NOT_CUT_A_SHORT_HISTORY"
    );
    super::round_two::assert_selected_epoch(
        &saved,
        &owner
            .branch_history(owner.manual_branch_id())
            .unwrap()
            .into_capture()
            .unwrap(),
    );
    let depth = saved.history().undo.len();
    drop(saved);
    for _ in 0..depth {
        apply(&core, Command::Undo);
    }
    let floor = core.export_history().unwrap();
    apply(&core, Command::Undo);
    let stopped = core.export_history().unwrap();
    assert_eq!(stopped.history().undo, floor.history().undo);
    assert_eq!(stopped.history().redo, floor.history().redo);
    assert_eq!(stopped.history().position, floor.history().position + 1);
    assert_eq!(stopped.history().document, floor.history().document);
    drop(floor);
    drop(stopped);
    apply(&core, lock(false));
    let inputs = owner
        .checkpoint(owner.manual_branch_id())
        .unwrap()
        .input_manifest_ref
        .clone();
    owner.save(&core, &inputs).unwrap();
    drop(owner);
    let mut loaded = LoadedBundle::read(
        &path,
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .unwrap();
    let reopened = loaded.open_manual().unwrap();
    assert_eq!(
        reopened.export_history().unwrap(),
        core.export_history().unwrap()
    );
}

#[test]
fn short_reused_lut_ids_floor_save_and_reopen_both_contents() {
    let temp = temp("r3-lut-collision");
    let path = temp.path("edit.kinewright");
    let core = Core::for_test_with_bounds(
        document(1),
        kinewright_core::undo::RoleBounds {
            step_cap: 2,
            ..kinewright_core::undo::RoleBounds::ORDINARY
        },
    )
    .unwrap();
    let mut owner = ProjectBundle::activate(
        &path,
        BundleWriteAdmission::new_document(),
        core.history_pool(),
    )
    .unwrap();
    let mut entries = vec![];
    for bytes in [
        b"LUT A bytes".as_slice(),
        b"different LUT B bytes".as_slice(),
    ] {
        let object = ObjectRef {
            sha256: kinewright_media::sha256_bytes(bytes),
            byte_length: bytes.len() as u64,
            record_kind: RecordKind::OwnedLut,
        };
        let seal = owner
            .store_mut()
            .unwrap()
            .admit_blob(&mut &*bytes, &object, &[])
            .unwrap();
        let entry = OwnedInputEntry {
            entry_id: "lut:1".into(),
            object: object.clone(),
            dependencies: vec![],
            selected_stream: None,
            time_base: None,
            origin: None,
            profile: None,
        };
        let inputs = owner
            .admit_inputs(&manifest(&owner, vec![entry.clone()]))
            .unwrap();
        let asset = LutAsset {
            id: LutAssetId(1),
            sha256: object.sha256.clone(),
            byte_len: object.byte_length,
            title: "LUT".into(),
            kind: LutAssetKind::Cube3d,
            size: 2,
            domain_min_millionths: [0; 3],
            domain_max_millionths: [1_000_000; 3],
            source: LutAssetSource::Imported {
                source_path: "missing.cube".into(),
            },
        };
        apply(&core, Command::Do(Operation::AddLutAsset { asset }));
        owner.save(&core, &inputs.reference).unwrap();
        entries.push(entry);
        drop(inputs);
        drop(seal);
        if entries.len() == 1 {
            apply(&core, lock(false));
            apply(
                &core,
                Command::Do(Operation::RemoveLutAsset {
                    lut_asset: LutAssetId(1),
                }),
            );
            let empty = owner.empty_inputs().unwrap();
            owner.save(&core, &empty.reference).unwrap();
        }
    }
    // The baseline needs A and the suffix/current head needs B at the same local ID.
    assert!(
        core.export_history().unwrap().fold_frontier().is_some(),
        "EXECUTOR_FOLDED_THE_OLD_LUT_INTO_ITS_BASELINE"
    );
    let b = owner
        .admit_inputs(&manifest(&owner, vec![entries[1].clone()]))
        .unwrap();
    for _ in 0..5 {
        owner
            .save(&core, &b.reference)
            .expect("synthesized LUT IDs must remain unique");
        owner.collect_after_ack().unwrap();
    }
    drop(b);
    drop(owner);
    let mut loaded = LoadedBundle::read(
        &path,
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .unwrap();
    let reopened = loaded.open_manual().unwrap();
    assert_eq!(
        reopened.export_history().unwrap(),
        core.export_history().unwrap()
    );
    for (entry, bytes) in entries.iter().zip([
        b"LUT A bytes".as_slice(),
        b"different LUT B bytes".as_slice(),
    ]) {
        assert_eq!(
            fs::read(
                path.with_extension("kinewright-assets")
                    .join("studio/objects")
                    .join(&entry.object.sha256)
            )
            .unwrap(),
            bytes
        );
    }
}

fn reconcile_floor(landed: bool, branch_save: bool) {
    let temp = temp("r3-reconcile-floor");
    let path = temp.path("edit.kinewright");
    let core = Core::for_test(document(200)).unwrap();
    let mut owner = ProjectBundle::activate(
        &path,
        BundleWriteAdmission::new_document(),
        core.history_pool(),
    )
    .unwrap();
    let inputs = owner.empty_inputs().unwrap().reference.clone();
    owner.save(&core, &inputs).unwrap();
    let branch = if branch_save {
        owner
            .fork_branch(
                &owner.manual_branch_id().clone(),
                kinewright_project::operation_history::Role::Ordinary,
            )
            .unwrap()
    } else {
        owner.manual_branch_id().clone()
    };
    owner
        .restrict_limits_probe(StorageLimits {
            ..StorageLimits::default()
        })
        .unwrap();
    for i in 0..40 {
        apply(&core, lock(i % 2 == 0));
    }
    let before = core.snapshot_identity().unwrap();
    let old = fs::read(&path).unwrap();
    let failure = if landed {
        "root_parent_sync"
    } else {
        "root_rename_io"
    };
    let result = io_observer::fail_once(failure, || {
        if branch_save {
            owner.save_branch(&branch, &core)
        } else {
            owner.save(&core, &inputs)
        }
    });
    assert!(matches!(
        result,
        Err(kinewright_project::bundle::BundleError::DurabilityUnknown)
    ));
    assert_eq!(core.snapshot_identity().unwrap(), before);
    if landed {
        assert_ne!(fs::read(&path).unwrap(), old);
    } else {
        assert_eq!(fs::read(&path).unwrap(), old);
    }
    owner.reconcile().unwrap();
    if landed {
        let published = owner
            .branch_history(&branch)
            .unwrap()
            .into_capture()
            .unwrap();
        assert_eq!(
            published.identity().unwrap(),
            before,
            "RECONCILIATION_PRESERVES_THE_FULL_PUBLISHED_CUT"
        );
        assert_eq!(
            core.export_history().unwrap(),
            published,
            "reconcile acknowledged a floor without installing it"
        );
        let depth = published.history().undo.len();
        drop(published);
        for _ in 0..depth {
            apply(&core, Command::Undo);
        }
        let floor = core.export_history().unwrap();
        apply(&core, Command::Undo);
        let stopped = core.export_history().unwrap();
        assert_eq!(stopped.history().undo, floor.history().undo);
        assert_eq!(stopped.history().redo, floor.history().redo);
        assert_eq!(stopped.history().position, floor.history().position + 1);
        assert_eq!(stopped.history().document, floor.history().document);
    } else {
        assert_eq!(
            core.snapshot_identity().unwrap(),
            before,
            "a different confirmed root discarded unsaved edits"
        );
    }
    apply(&core, lock(false));
    if branch_save {
        owner.save_branch(&branch, &core).unwrap();
    } else {
        owner.save(&core, &inputs).unwrap();
    }
    assert_eq!(
        core.export_history().unwrap(),
        owner
            .branch_history(&branch)
            .unwrap()
            .into_capture()
            .unwrap()
    );
}
#[test]
fn reconcile_own_floor_installs_manual_core() {
    reconcile_floor(true, false);
}
#[test]
fn reconcile_older_root_preserves_manual_unsaved_edits() {
    reconcile_floor(false, false);
}
#[test]
fn reconcile_own_floor_installs_specialist_core() {
    reconcile_floor(true, true);
}
#[test]
fn reconcile_older_root_preserves_specialist_unsaved_edits() {
    reconcile_floor(false, true);
}

#[test]
fn persistent_file_refusal_collects_other_garbage_and_reports_pending_bytes() {
    let temp = temp("r3-stuck-gc");
    let path = temp.path("edit.kinewright");
    let core = Core::for_test(document(1)).unwrap();
    let mut owner = ProjectBundle::activate(
        &path,
        BundleWriteAdmission::new_document(),
        core.history_pool(),
    )
    .unwrap();
    let inputs = owner.empty_inputs().unwrap().reference.clone();
    owner.save(&core, &inputs).unwrap();
    drop(owner);
    let objects = path
        .with_extension("kinewright-assets")
        .join("studio/objects");
    let stuck_bytes = b"permanently refused orphan";
    let stuck_hash = kinewright_media::sha256_bytes(stuck_bytes);
    let stuck = objects.join(&stuck_hash);
    fs::write(&stuck, stuck_bytes).unwrap();
    let other = objects.join(kinewright_media::sha256_bytes(b"ordinary orphan"));
    fs::write(&other, b"ordinary orphan").unwrap();
    let temporary = objects.join(format!(
        ".studio-{}-{}.tmp",
        kinewright_media::sha256_bytes(b"edit.kinewright"),
        "0".repeat(32)
    ));
    fs::write(&temporary, b"abandoned staging bytes").unwrap();
    let mut owner = LoadedBundle::read(&path, core.history_pool())
        .unwrap()
        .into_writer()
        .unwrap();
    let relative = Path::new("studio/objects").join(&stuck_hash);
    io_observer::fail_path("gc_before_unlink", &relative, || {
        let first = owner.save(&core, &inputs).unwrap();
        assert_eq!(first.collection.failed_files, 1);
        assert!(!other.exists());
        assert!(!temporary.exists());
        // Adopt the same refused file as a known, acknowledged object, then keep the
        // refusal armed across every later save and its known-object sweep.
        let reference = ObjectRef {
            sha256: stuck_hash.clone(),
            byte_length: stuck_bytes.len() as u64,
            record_kind: RecordKind::OwnedEvidence,
        };
        let pin = owner
            .store_mut()
            .unwrap()
            .admit_blob(&mut &stuck_bytes[..], &reference, &[])
            .unwrap();
        owner.save(&core, &inputs).unwrap();
        drop(pin);
        let mut previous = vec![];
        for iteration in 1..=8 {
            let mut next = vec![];
            for index in 0..3 {
                let bytes = format!("unreferenced blob {iteration}/{index}").into_bytes();
                let reference = ObjectRef {
                    sha256: kinewright_media::sha256_bytes(&bytes),
                    byte_length: bytes.len() as u64,
                    record_kind: RecordKind::OwnedEvidence,
                };
                let seal = owner
                    .store_mut()
                    .unwrap()
                    .admit_blob(&mut &*bytes, &reference, &[])
                    .unwrap();
                next.push(objects.join(&reference.sha256));
                drop(seal);
            }
            apply(&core, lock(iteration % 2 == 0));
            let report = owner.save(&core, &inputs).unwrap();
            assert_eq!(report.collection.failed_files, 1);
            assert_eq!(report.collection.pending_bytes, stuck_bytes.len() as u64);
            assert_eq!(report.collection.failures[0].path, relative);
            assert_eq!(report.collection.failures[0].attempts, iteration);
            assert!(owner.collection_error().is_some());
            assert_eq!(fs::read(&stuck).unwrap(), stuck_bytes);
            assert!(!other.exists(), "a stuck file stopped orphan cleanup");
            assert!(
                !temporary.exists(),
                "a stuck file stopped temporary cleanup"
            );
            for file in previous {
                assert!(
                    !fs::exists(file).unwrap(),
                    "a stuck file stopped known-object cleanup"
                );
            }
            previous = next;
            let usage = owner.store().usage().unwrap();
            assert_eq!(usage.unclassified_bytes, 0);
            assert!(
                usage.objects < 100,
                "garbage accumulated across saves: {}",
                usage.objects
            );
            assert_eq!(
                core.export_history().unwrap(),
                owner
                    .branch_history(owner.manual_branch_id())
                    .unwrap()
                    .into_capture()
                    .unwrap()
            );
        }
    });
    owner.collect_after_ack().unwrap();
    assert!(!stuck.exists());
    assert!(owner.collection_error().is_none());
    let report = owner.save(&core, &inputs).unwrap();
    assert_eq!(report.collection.failed_files, 0);
    assert_eq!(report.collection.pending_bytes, 0);
}
