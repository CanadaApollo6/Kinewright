//! Round-2 regression scenarios through the real bundle owner.
#![allow(
    clippy::too_many_lines,
    reason = "each witness retains its full scenario"
)]
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
    store::{RecoveryInventory, StoreManifest},
};
use std::{fs, path::Path};
fn temp(label: &str) -> TempDirectory {
    TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), label)
}
fn apply(core: &Core, command: Command) {
    assert!(matches!(
        core.request(command).unwrap(),
        Event::DocumentChanged { .. }
    ));
}
fn lock(locked: bool) -> Operation {
    Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked,
    }
}
fn document() -> Document {
    Document {
        tracks: (1..=200)
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
fn setup(path: &Path) -> (Core, ProjectBundle, ObjectRef) {
    let core = actor(document());
    let mut owner = ProjectBundle::activate(
        path,
        BundleWriteAdmission::new_document(),
        core.history_pool(),
    )
    .unwrap();
    let inputs = owner.empty_inputs().unwrap().reference.clone();
    owner.save(&core, &inputs).unwrap();
    (core, owner, inputs)
}
fn actor(document: impl Into<std::sync::Arc<Document>>) -> Core {
    Core::for_test_with_bounds(
        document,
        kinewright_core::undo::RoleBounds {
            step_cap: 4,
            ..kinewright_core::undo::RoleBounds::ORDINARY
        },
    )
    .unwrap()
}
fn object(path: &Path, r: &ObjectRef) -> std::path::PathBuf {
    path.with_extension("kinewright-assets")
        .join("studio/objects")
        .join(&r.sha256)
}
#[test]
fn ordinary_save_installs_floor_preserving_redo_gesture_and_reopen() {
    let temp = temp("r2-live-floor");
    let path = temp.path("edit.kinewright");
    let (core, mut owner, inputs) = setup(&path);
    for i in 0..40 {
        apply(&core, Command::Do(lock(i % 2 == 0)));
        owner.save(&core, &inputs).unwrap();
        assert_eq!(
            core.export_history().unwrap(),
            owner
                .branch_history(owner.manual_branch_id())
                .unwrap()
                .into_capture()
                .unwrap(),
            "save acknowledged a different live history"
        );
    }
    assert!(
        core.export_history()
            .unwrap()
            .fold_frontier()
            .map_or(0, |key| key.position + 1)
            > 0
    );
    apply(&core, Command::Undo);
    apply(&core, Command::Undo);
    let before_save = core.export_history().unwrap();
    let redo = serde_json::to_value(&before_save.history().redo).unwrap();
    drop(before_save);
    owner.save(&core, &inputs).unwrap();
    assert_eq!(
        serde_json::to_value(&core.export_history().unwrap().history().redo).unwrap(),
        redo
    );
    apply(&core, Command::Redo);
    apply(&core, Command::Redo);
    apply(
        &core,
        Command::DoBatchCoalesced {
            coalesce_key: "r2-open".into(),
            operations: vec![lock(false)],
        },
    );
    owner.save(&core, &inputs).unwrap();
    let open = core.export_history().unwrap();
    assert_eq!(
        open.history().undo.back().unwrap().coalesce_key(),
        Some("r2-open")
    );
    assert_eq!(
        open,
        owner
            .branch_history(owner.manual_branch_id())
            .unwrap()
            .into_capture()
            .unwrap()
    );
    let count = open.history().undo.len();
    let open_document = std::sync::Arc::clone(&open.history().document);
    drop(open);
    for _ in 0..count {
        apply(&core, Command::Undo);
    }
    let at_floor = core.export_history().unwrap();
    assert!(at_floor.history().undo.is_empty());

    core.request(Command::Undo).unwrap();
    let stopped = core.export_history().unwrap();
    assert_eq!(
        stopped.history().document,
        at_floor.history().document,
        "undo crossed the durable floor"
    );
    assert_eq!(stopped.history().undo, at_floor.history().undo);
    assert_eq!(stopped.history().redo, at_floor.history().redo);
    assert_eq!(
        stopped.history().position,
        at_floor.history().position + 1,
        "EMPTY_UNDO_ADVANCES_ONLY_THE_ACCEPTED_COUNTER"
    );
    assert_eq!(
        stopped.fold_frontier().map_or(0, |key| key.position + 1),
        at_floor.fold_frontier().map_or(0, |key| key.position + 1)
    );
    drop(at_floor);
    drop(stopped);
    owner.save(&core, &inputs).unwrap();
    assert_eq!(
        core.export_history().unwrap(),
        owner
            .branch_history(owner.manual_branch_id())
            .unwrap()
            .into_capture()
            .unwrap()
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
    assert_eq!(
        reopened.export_history().unwrap(),
        core.export_history().unwrap()
    );
    for _ in 0..count {
        apply(&reopened, Command::Redo);
        apply(&core, Command::Redo);
    }
    assert_eq!(
        reopened.export_history().unwrap(),
        core.export_history().unwrap()
    );
    assert_eq!(
        reopened.export_history().unwrap().history().document,
        open_document
    );
    core.request(Command::Redo).unwrap();
    reopened.request(Command::Redo).unwrap();
    let mut owner = loaded.into_writer().unwrap();
    owner.save(&reopened, &inputs).unwrap();
    assert_eq!(
        reopened.export_history().unwrap(),
        core.export_history().unwrap()
    );
}
#[test]
fn first_save_installs_the_same_floor_before_handoff() {
    let temp = temp("r2-first-floor");
    let core = actor(document());
    for i in 0..40 {
        apply(&core, Command::Do(lock(i % 2 == 0)));
    }
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
    let copy = session
        .first_save_with_limits(
            &path,
            &core,
            &inputs.reference,
            StorageLimits {
                branch_history_bytes: 1 << 20,
                ..StorageLimits::default()
            },
        )
        .unwrap();
    let owner = copy
        .into_destination()
        .unwrap_or_else(|_| panic!("handoff pending"));
    assert!(
        core.export_history()
            .unwrap()
            .fold_frontier()
            .map_or(0, |key| key.position + 1)
            > 0
    );
    let selected = owner
        .branch_history(owner.manual_branch_id())
        .unwrap()
        .into_capture()
        .unwrap();
    assert_selected_epoch(&core.export_history().unwrap(), &selected);
    drop(owner);
    let mut loaded = LoadedBundle::read(
        &path,
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .unwrap();
    let reopened = loaded.open_manual().unwrap();
    assert_selected_epoch(
        &core.export_history().unwrap(),
        &reopened.export_history().unwrap(),
    );
}
#[test]
fn floor_retains_lut_context_across_multi_edit_save_reuse_and_retirement() {
    let temp = temp("r2-lut-floor");
    let path = temp.path("edit.kinewright");
    let (core, mut owner, _) = setup(&path);
    let lut = |bytes: &[u8]| LutAsset {
        id: LutAssetId(1),
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_len: bytes.len() as u64,
        title: "LUT".into(),
        kind: LutAssetKind::Cube3d,
        size: 2,
        domain_min_millionths: [0; 3],
        domain_max_millionths: [1_000_000; 3],
        source: LutAssetSource::Imported {
            source_path: "unavailable.cube".into(),
        },
    };
    let mut entries = vec![];
    let mut seals = vec![];
    for bytes in [b"LUT A bytes".as_slice(), b"LUT B different".as_slice()] {
        let r = ObjectRef {
            sha256: kinewright_media::sha256_bytes(bytes),
            byte_length: bytes.len() as u64,
            record_kind: RecordKind::OwnedLut,
        };
        let seal = owner
            .store_mut()
            .unwrap()
            .admit_blob(&mut &bytes[..], &r, &[])
            .unwrap();
        entries.push(OwnedInputEntry {
            entry_id: format!("lut:1:{}", r.sha256),
            object: r,
            dependencies: vec![],
            selected_stream: None,
            time_base: None,
            origin: None,
            profile: None,
        });
        seals.push(seal);
    }
    let manifest = |owner: &ProjectBundle, entries| OwnedInputManifest {
        project_id: Some(owner.project_id().clone()),
        session_id: None,
        owner_epoch: owner.store().owner_epoch(),
        entries,
    };
    let a = owner
        .admit_inputs(&manifest(&owner, vec![entries[0].clone()]))
        .unwrap();
    apply(
        &core,
        Command::Do(Operation::AddLutAsset {
            asset: lut(b"LUT A bytes"),
        }),
    );
    owner.save(&core, &a.reference).unwrap();
    apply(&core, Command::Do(lock(false)));
    apply(
        &core,
        Command::Do(Operation::RemoveLutAsset {
            lut_asset: LutAssetId(1),
        }),
    );
    let empty = owner.empty_inputs().unwrap();
    owner.save(&core, &empty.reference).unwrap();
    // Six steps in a four-step executor fold exactly the Add and the lock.
    for locked in [true, false, true] {
        apply(&core, Command::Do(lock(locked)));
    }
    assert_eq!(
        core.export_history()
            .unwrap()
            .fold_frontier()
            .unwrap()
            .position,
        2
    );

    owner.save(&core, &empty.reference).unwrap();
    drop(a);
    drop(empty);
    drop(seals);
    drop(owner);
    let loaded = LoadedBundle::read(&path, core.history_pool())
        .expect("floor lost the LUT required by its baseline and retained undo entry");
    assert_eq!(
        fs::read(object(&path, &entries[0].object)).unwrap(),
        b"LUT A bytes",
        "FOLDED_BASELINE_KEEPS_LUT_UNTIL_REMOVAL_IS_FOLDED"
    );
    let mut owner = loaded.into_writer().unwrap();
    let b_bytes = b"LUT B different".as_slice();
    let b_seal = owner
        .store_mut()
        .unwrap()
        .admit_blob(&mut &*b_bytes, &entries[1].object, &[])
        .unwrap();
    apply(
        &core,
        Command::Do(Operation::AddLutAsset {
            asset: lut(b"LUT B different"),
        }),
    );
    let b = owner
        .admit_inputs(&manifest(&owner, vec![entries[1].clone()]))
        .unwrap();
    owner.save(&core, &b.reference).unwrap();
    for _ in 0..4 {
        owner.save(&core, &b.reference).unwrap();
        owner.collect_after_ack().unwrap();
    }
    // Adding B folds A's removal. Four later roots retire the old automatic
    // recovery points, so no retained baseline, Undo or Redo needs A now.
    assert!(
        !object(&path, &entries[0].object).exists(),
        "FOLDED_REMOVAL_RELEASES_UNREACHABLE_LUT"
    );
    assert_eq!(
        fs::read(object(&path, &entries[1].object)).unwrap(),
        b"LUT B different"
    );
    drop(b);
    drop(b_seal);
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
    let depth = reopened.export_history().unwrap().history().undo.len();
    for _ in 0..depth {
        apply(&reopened, Command::Undo);
    }
    for _ in 0..depth {
        apply(&reopened, Command::Redo);
    }
    assert_eq!(
        reopened.export_history().unwrap().history().document,
        core.export_history().unwrap().history().document,
        "EVERY_REACHABLE_LUT_STATE_REPLAYS_AFTER_COLLECTION"
    );
    assert_eq!(
        reopened.export_history().unwrap().history().undo.len(),
        depth
    );
}
#[test]
fn newly_admitted_unpinned_input_survives_previous_ack_cleanup() {
    let temp = temp("r2-new-input");
    let (core, mut owner, _) = setup(&temp.path("edit.kinewright"));
    let bytes = b"new evidence admitted after acknowledgment";
    let r = ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::OwnedEvidence,
    };
    drop(
        owner
            .store_mut()
            .unwrap()
            .admit_blob(&mut &bytes[..], &r, &[])
            .unwrap(),
    );
    let inputs = owner
        .admit_inputs(&OwnedInputManifest {
            project_id: Some(owner.project_id().clone()),
            session_id: None,
            owner_epoch: owner.store().owner_epoch(),
            entries: vec![OwnedInputEntry {
                entry_id: "new-evidence".into(),
                object: r.clone(),
                dependencies: vec![],
                selected_stream: None,
                time_base: None,
                origin: None,
                profile: None,
            }],
        })
        .unwrap()
        .reference
        .clone();
    owner
        .save(&core, &inputs)
        .expect("pre-save cleanup deleted newly admitted inputs");
    owner.store().verify(&r).unwrap();
    drop(owner);
    LoadedBundle::read(&temp.path("edit.kinewright"), core.history_pool()).unwrap();
}
#[test]
fn persistent_collection_failure_is_recorded_and_does_not_block_saves() {
    let temp = temp("r2-gc-retry");
    let path = temp.path("edit.kinewright");
    let (core, mut owner, inputs) = setup(&path);
    for i in 0..5 {
        apply(&core, Command::Do(lock(i % 2 == 0)));
        owner.save(&core, &inputs).unwrap();
    }
    for i in 0..3 {
        apply(&core, Command::Do(lock(i % 2 != 0)));
        let (result, trace) = io_observer::observe(|| {
            io_observer::fail_once("gc_before_unlink", || owner.save(&core, &inputs))
        });
        assert!(
            trace
                .events
                .iter()
                .any(|e| e.operation == "injected_failure"),
            "collection failure was not exercised"
        );
        result.expect("maintenance failure blocked a new save");
        assert!(
            owner.collection_error().is_some(),
            "maintenance failure was not recorded"
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
    owner.collect_after_ack().unwrap();
    assert!(owner.collection_error().is_none());
    LoadedBundle::read(
        &path,
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .unwrap();
}
#[test]
fn over_quota_reopen_keeps_inventory_only_owned_graph_until_retirement() {
    let temp = temp("r2-inventory-owned");
    let path = temp.path("edit.kinewright");
    let (core, mut owner, inputs) = setup(&path);
    let bytes = vec![37_u8; 16 << 20];
    let r = ObjectRef {
        sha256: kinewright_media::sha256_bytes(&bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::OwnedLut,
    };
    let seal = owner
        .store_mut()
        .unwrap()
        .admit_blob(&mut &bytes[..], &r, &[])
        .unwrap();
    let a = owner
        .admit_inputs(&OwnedInputManifest {
            project_id: Some(owner.project_id().clone()),
            session_id: None,
            owner_epoch: owner.store().owner_epoch(),
            entries: vec![OwnedInputEntry {
                entry_id: "lut:1".into(),
                object: r.clone(),
                dependencies: vec![],
                selected_stream: None,
                time_base: None,
                origin: None,
                profile: None,
            }],
        })
        .unwrap();
    apply(
        &core,
        Command::Do(Operation::AddLutAsset {
            asset: LutAsset {
                id: LutAssetId(1),
                sha256: r.sha256.clone(),
                byte_len: r.byte_length,
                title: "inventory LUT".into(),
                kind: LutAssetKind::Cube3d,
                size: 2,
                domain_min_millionths: [0; 3],
                domain_max_millionths: [1_000_000; 3],
                source: LutAssetSource::Imported {
                    source_path: "gone.cube".into(),
                },
            },
        }),
    );
    owner.save(&core, &a.reference).unwrap();
    apply(
        &core,
        Command::Do(Operation::RemoveLutAsset {
            lut_asset: LutAssetId(1),
        }),
    );
    owner.save(&core, &inputs).unwrap();
    for locked in [true, false, true, false] {
        apply(&core, Command::Do(lock(locked)));
    }
    assert_eq!(
        core.export_history()
            .unwrap()
            .fold_frontier()
            .unwrap()
            .position,
        2
    );

    owner.save(&core, &inputs).unwrap();
    owner.save(&core, &inputs).unwrap();
    owner.save(&core, &inputs).unwrap();
    let manifest: StoreManifest = serde_json::from_slice(
        &fs::read(object(&path, &owner.root().unwrap().store_manifest_ref)).unwrap(),
    )
    .unwrap();
    assert!(
        !manifest.objects.contains(&r),
        "object must be visible only through the B3 inventory"
    );
    let inventory_path = path
        .with_extension("kinewright-assets")
        .join("studio/recovery/inventory");
    let before = fs::read(&inventory_path).unwrap();
    let inventory: RecoveryInventory = serde_json::from_slice(&before).unwrap();
    assert!(
        inventory
            .predecessors
            .iter()
            .any(|p| !manifest.recovery.predecessors.contains(p))
    );
    drop(a);
    drop(seal);
    drop(owner);
    let limits = StorageLimits {
        metadata_bytes: 8 << 20,
        // Fresh open now uses this effective profile, including bounded decode scratch.
        decoded_record_bytes: 4 << 20,
        ..StorageLimits::default()
    };
    let (writer, trace) = io_observer::observe(|| {
        LoadedBundle::read(
            &path,
            &kinewright_project::operation_history::HistoryPool::new(
                kinewright_project::operation_history::PoolLimits::default(),
            ),
        )
        .unwrap()
        .into_writer_with_limits_probe(limits)
        .unwrap()
    });
    assert_eq!(fs::read(&inventory_path).unwrap(), before);
    assert!(
        r.byte_length > writer.store().limits().metadata_bytes,
        "the old residual charge must exceed the real reopen bound"
    );
    assert_eq!(
        fs::read(object(&path, &r)).unwrap(),
        bytes,
        "reopen unlinked a durable-inventory-owned object"
    );
    assert!(!trace.events.iter().any(|e| e.operation == "object_unlink"));
    let mut writer = writer;
    let (_, retirement) = io_observer::observe(|| writer.collect_after_ack().unwrap());
    let unlink = retirement
        .events
        .iter()
        .position(|e| e.operation == "object_unlink")
        .unwrap();
    let sync = retirement
        .events
        .iter()
        .position(|e| e.operation == "inventory_rename")
        .unwrap();
    assert!(sync < unlink);
    assert!(!object(&path, &r).exists());
    LoadedBundle::read(
        &path,
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .unwrap();
    super::durable_bundle::model_inventory_retirement_cuts();
}
#[test]
fn no_edit_comment_preserves_the_unsaved_actor_cut_and_the_published_snapshot() {
    use kinewright_project::receipts::{CommandEnvelope, CommandId, ReceiptLookup};
    let temp = temp("r2-comment-floor");
    let (core, mut owner, inputs) = setup(&temp.path("edit.kinewright"));
    let issuer = owner.issue_issuer(&core, &inputs, 1).unwrap();
    for i in 0..40 {
        apply(&core, Command::Do(lock(i % 2 == 0)));
    }
    apply(
        &core,
        Command::DoBatchCoalesced {
            coalesce_key: "comment-gesture".into(),
            operations: vec![lock(false)],
        },
    );
    owner.save(&core, &inputs).unwrap();
    let durable = owner.checkpoint(owner.manual_branch_id()).unwrap().identity;
    apply(&core, Command::Do(lock(true)));
    let before = core.snapshot_identity().unwrap();
    assert_ne!(before, durable);
    let Event::QueryResult(QueryResult::Snapshot { revision, .. }) =
        core.request(Command::Query(Query::Snapshot)).unwrap()
    else {
        panic!("snapshot")
    };
    let text = b"exact comment".as_slice();
    let strokes = b"[]".as_slice();
    let reference = |bytes: &[u8]| ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::OwnedEvidence,
    };
    let text_ref = reference(text);
    let strokes_ref = reference(strokes);
    let request = CommandEnvelope {
        switch: None,
        manual: None,
        context: None,
        version: 1,
        action: "comment".into(),
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
        command: None,
        comment_text: Some(text_ref.clone()),
        comment_strokes: Some(strokes_ref.clone()),
        evidence: vec![],
        transition: None,
    };
    assert!(matches!(
        owner
            .command(
                &request,
                &core,
                &inputs,
                &[(&text_ref, text), (&strokes_ref, strokes)]
            )
            .unwrap(),
        ReceiptLookup::Committed(_)
    ));
    assert_eq!(
        core.snapshot_identity().unwrap(),
        before,
        "COMMENT_DOES_NOT_REPLACE_UNSAVED_HISTORY"
    );
    assert_eq!(
        owner.checkpoint(owner.manual_branch_id()).unwrap().identity,
        durable,
        "COMMENT_DOES_NOT_PUBLISH_UNRELATED_UNSAVED_EDITS"
    );
}
#[test]
fn wrong_lut_length_refuses_even_when_sha_and_entry_id_match() {
    let temp = temp("r2-lut-length");
    let path = temp.path("edit.kinewright");
    let core = Core::for_test(Document::default()).unwrap();
    let mut owner = ProjectBundle::activate(
        &path,
        BundleWriteAdmission::new_document(),
        core.history_pool(),
    )
    .unwrap();
    let bytes = b"exact LUT bytes".as_slice();
    let r = ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::OwnedLut,
    };
    let seal = owner
        .store_mut()
        .unwrap()
        .admit_blob(&mut &*bytes, &r, &[])
        .unwrap();
    let inputs = owner
        .admit_inputs(&OwnedInputManifest {
            project_id: Some(owner.project_id().clone()),
            session_id: None,
            owner_epoch: owner.store().owner_epoch(),
            entries: vec![OwnedInputEntry {
                entry_id: "lut:1".into(),
                object: r.clone(),
                dependencies: vec![],
                selected_stream: None,
                time_base: None,
                origin: None,
                profile: None,
            }],
        })
        .unwrap();
    let lut = LutAsset {
        id: LutAssetId(1),
        sha256: r.sha256.clone(),
        byte_len: r.byte_length + 1,
        title: "wrong length".into(),
        kind: LutAssetKind::Cube3d,
        size: 2,
        domain_min_millionths: [0; 3],
        domain_max_millionths: [1_000_000; 3],
        source: LutAssetSource::Imported {
            source_path: "gone.cube".into(),
        },
    };
    apply(&core, Command::Do(Operation::AddLutAsset { asset: lut }));
    let error = owner.save(&core, &inputs.reference).unwrap_err();
    assert!(
        matches!(
            error,
            kinewright_project::bundle::BundleError::Operation(
                kinewright_project::operation_actor::Error::History(
                    kinewright_project::operation_history::Error::MissingResource
                )
            )
        ),
        "{error}"
    );
    assert!(!path.exists());
    drop(seal);
}
#[test]
fn save_as_preserves_the_published_floor_and_refuses_silent_copy_trim() {
    use kinewright_project::bundle_copy::SaveAsOutcome;
    let temp = temp("r2-copy-floor");
    let path = temp.path("edit.kinewright");
    let (core, mut owner, inputs) = setup(&path);
    for i in 0..40 {
        apply(&core, Command::Do(lock(i % 2 == 0)));
        owner.save(&core, &inputs).unwrap();
    }
    let history = core.export_history().unwrap();
    assert!(history.fold_frontier().map_or(0, |key| key.position + 1) > 0);
    let copied = match owner.save_as(&temp.path("copied.kinewright")).unwrap() {
        SaveAsOutcome::Copied(copy) => copy
            .into_destination()
            .unwrap_or_else(|_| panic!("copy pending")),
        SaveAsOutcome::SameProject(_) => panic!("not a copy"),
    };
    let selected = copied
        .branch_history(copied.manual_branch_id())
        .unwrap()
        .into_capture()
        .unwrap();
    assert_selected_epoch(&history, &selected);
    drop(selected);
    let narrow = temp.path("narrow.kinewright");
    let result = owner.save_as_with_limits(
        &narrow,
        StorageLimits {
            metadata_bytes: 1,
            ..StorageLimits::default()
        },
    );
    assert!(
        result.is_err(),
        "copy silently trimmed a snapshot without a live Core transition"
    );
    assert!(!narrow.exists());
    assert_eq!(core.export_history().unwrap(), history);
}

pub(super) fn assert_selected_epoch(
    source: &kinewright_project::operation_history::OwnerSnapshot,
    selected: &kinewright_project::operation_history::OwnerSnapshot,
) {
    assert!(
        selected.history().epoch > source.history().epoch,
        "COPY_STARTS_A_FRESH_INSTALL_EPOCH"
    );
    let mut expected: kinewright_core::undo::Record =
        serde_json::from_value(serde_json::to_value(source.history()).unwrap()).unwrap();
    expected.epoch = selected.history().epoch;
    let source_identity = source.identity().unwrap();
    let selected_identity = selected.identity().unwrap();
    assert_eq!(
        selected_identity.core,
        expected.snapshot_identity().unwrap(),
        "COPY_PRESERVES_ALL_COMMANDS_AND_BOTH_STACKS"
    );
    assert_eq!(
        selected_identity.owner, source_identity.owner,
        "COPY_PRESERVES_COMPLETE_CANONICAL_OWNER"
    );
}
