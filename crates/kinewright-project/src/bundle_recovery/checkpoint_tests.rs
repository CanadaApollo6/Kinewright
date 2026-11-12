//! Verified checkpoint rescue retains failed history and original occurrence identities.
use super::*;
use kinewright_core::{Command, Core, Operation, Track, TrackId, TrackKind};
use kinewright_media::test_support::{TempDirectory, workspace_target_dir};

fn fixture() -> (TempDirectory, Core, ProjectBundle) {
    let temp = TempDirectory::new_in(&workspace_target_dir(), "checkpoint-undo-recovery");
    let mut document = Document::default();
    document.tracks.push(Track {
        id: TrackId(1),
        kind: TrackKind::Video,
        sync_lock: true,
        clips: vec![
            serde_json::from_value(serde_json::json!({
                "id":1,"asset":0,"source_range":{"start":0,"end":90},
                "content":{"solid":{"r":0,"g":0,"b":0}},
                "timeline_start":0,"effects":[],"transition_in":null
            }))
            .unwrap(),
        ],
    });
    document.duration = kinewright_core::TimeCode(90);
    let core = Core::spawn(document).unwrap();
    core.request(Command::Do(Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked: false,
    }))
    .unwrap();
    let mut bundle = ProjectBundle::activate(
        &temp.path("source.kinewright"),
        BundleWriteAdmission::new_document(),
    )
    .unwrap();
    let inputs = bundle.empty_inputs().unwrap();
    bundle.save(&core, &inputs.reference).unwrap();
    (temp, core, bundle)
}

/// Build an authentic content-addressed but semantically unreplayable graph.
/// Only this adversarial fixture replaces a root outside the ordinary publisher.
fn damage_journal(bundle: &mut ProjectBundle, reject_operation: bool) -> BranchCheckpoint {
    let mut checkpoint = bundle.branches[&bundle.manual_branch_id].clone();
    let mut record = (*bundle
        .store
        .read_record::<crate::history_record::HistoryRecord>(&checkpoint.history_ref)
        .unwrap())
    .clone();
    if reject_operation {
        record.journal.batches[0].command =
            kinewright_core::JournalCommand::Do(Operation::SetTrackSyncLock {
                track: TrackId(999),
                locked: false,
            });
    } else {
        record.journal.cursor.undo[0] = "0".repeat(64);
    }
    let history = bundle.store.admit_record(&record).unwrap();
    checkpoint.history_ref = history.reference.clone();
    let sealed = bundle.store.admit_record(&checkpoint).unwrap();
    let index = bundle
        .store
        .admit_record(&BTreeMap::from([(
            checkpoint.branch_id.clone(),
            sealed.reference.clone(),
        )]))
        .unwrap();
    let mut root = bundle.root.as_ref().unwrap().clone();
    root.branch_index_ref = index.reference.clone();
    let manifest = bundle.store.manifest().unwrap();
    let sealed_manifest = bundle.store.admit_record(&manifest).unwrap();
    root.store_manifest_ref = sealed_manifest.reference.clone();
    let retained = root.references();
    let mut file = bundle.admission.data.file.as_ref().unwrap().clone();
    file.studio = Some(root);
    let bytes = serde_json::to_vec_pretty(&file).unwrap();
    let _pin = bundle.store.mark_committed(&retained).unwrap();
    bundle
        .store
        .with_namespace(|n| n.replace_root(&bytes).map_err(|e| e.error))
        .unwrap();
    checkpoint
}

#[test]
fn verified_checkpoint_recovery_refuses_to_drift_on_bad_boundary_or_command() {
    for reject_operation in [false, true] {
        let (temp, core, mut bundle) = fixture();
        let expected = core.export_history().unwrap().document;
        let old = bundle.branches[&bundle.manual_branch_id].clone();
        let current = bundle
            .store
            .read_record::<LineageRecord>(&old.lineage_ref)
            .unwrap()
            .current
            .clone();
        let damaged = damage_journal(&mut bundle, reject_operation);
        drop(bundle);
        drop(core);
        let source = temp.path("source.kinewright");
        let original_root = std::fs::read(&source).unwrap();
        let history_path = temp.path(&format!(
            "source.kinewright-assets/studio/objects/{}",
            damaged.history_ref.sha256
        ));
        let original_history = std::fs::read(&history_path).unwrap();
        assert!(
            crate::bundle::LoadedBundle::read(&source).is_err(),
            "ORDINARY_OPEN_STAYS_STRICT"
        );
        let recovery = VerifiedCheckpointRecovery::open(&source).unwrap().unwrap();
        assert_eq!(
            recovery.document(),
            &*expected,
            "RECOVERY_CHECKPOINT_DOCUMENT_EXACT"
        );
        assert_eq!(recovery.disclosure().history_ref, damaged.history_ref);
        assert!(recovery.lineage.undo.is_empty() && recovery.lineage.redo.is_empty());
        assert_eq!(
            recovery.lineage.current, current,
            "RECOVERY_OCCURRENCES_ARE_NOT_REMINTED"
        );
        let destination = temp.path("recovered.kinewright");
        recovery.save_copy(&destination).unwrap();
        let loaded = crate::bundle::LoadedBundle::read(&destination).unwrap();
        let restored = Core::spawn(Document::default()).unwrap();
        loaded.restore_manual(&restored).unwrap();
        let history = restored.export_history().unwrap();
        assert_eq!(history.document, expected);
        assert!(
            history.batches.is_empty()
                && history.cursor.undo.is_empty()
                && history.cursor.redo.is_empty()
        );
        let checkpoint = loaded.manual_checkpoint().unwrap().clone();
        drop(loaded);
        let mut writer = ProjectBundle::activate(
            &destination,
            BundleWriteAdmission::read(&destination).unwrap(),
        )
        .unwrap();
        assert_eq!(
            writer
                .store
                .read_record::<LineageRecord>(&checkpoint.lineage_ref)
                .unwrap()
                .current,
            current
        );
        let inputs = writer.root.as_ref().unwrap().input_manifest_ref.clone();
        writer.save(&restored, &inputs).unwrap();
        writer.collect_after_ack().unwrap();
        drop(writer);
        drop(restored);
        assert!(crate::bundle::LoadedBundle::read(&destination).is_ok());
        assert_eq!(
            std::fs::read(&source).unwrap(),
            original_root,
            "QUARANTINED_ROOT_RETAINED"
        );
        assert_eq!(
            std::fs::read(&history_path).unwrap(),
            original_history,
            "QUARANTINED_HISTORY_RETAINED"
        );
        assert!(
            recovery.save_copy(&destination).is_err(),
            "RECOVERY_CANNOT_OVERWRITE_AN_OCCUPIED_DESTINATION"
        );
        assert!(
            recovery.save_copy(&source).is_err(),
            "RECOVERY_CANNOT_REPLACE_ITS_SOURCE"
        );
    }
}

#[test]
fn checkpoint_recovery_requires_an_independently_verified_document() {
    let (temp, core, mut bundle) = fixture();
    let checkpoint = damage_journal(&mut bundle, false);
    drop(bundle);
    drop(core);
    let document_path = temp.path(&format!(
        "source.kinewright-assets/studio/objects/{}",
        checkpoint.document_ref.sha256
    ));
    std::fs::write(&document_path, b"damaged document").unwrap();
    assert!(
        VerifiedCheckpointRecovery::open(&temp.path("source.kinewright")).is_err(),
        "NO_DOCUMENT_FALLBACK_FROM_UNVERIFIED_BYTES"
    );
}

#[test]
fn checkpoint_current_lineage_does_not_replay_unavailable_undo_maps() {
    let (_temp, _core, bundle) = fixture();
    let checkpoint = &bundle.branches[&bundle.manual_branch_id];
    let original = bundle
        .store
        .read_record::<LineageRecord>(&checkpoint.lineage_ref)
        .unwrap();
    let mut raw = serde_json::to_value(&*original).unwrap();
    raw["undo"][0]["removed"] = serde_json::json!([999]);
    let bytes = serde_json::to_vec(&raw).unwrap();
    assert!(
        serde_json::from_slice::<LineageRecord>(&bytes).is_err(),
        "ORDINARY_LINEAGE_READER_STAYS_STRICT"
    );
    let recovered = crate::lineage_journal::restore_current(&bytes, |live| {
        bundle.store.reserve_copy_read(0, live, 0, false, true)
    })
    .unwrap();
    assert_eq!(recovered.current, original.current);
    assert_eq!(recovered.baseline, original.current);
    assert!(recovered.undo.is_empty() && recovered.redo.is_empty());
    raw["unexpected"] = serde_json::json!(true);
    assert!(
        crate::lineage_journal::restore_current(&serde_json::to_vec(&raw).unwrap(), |live| {
            bundle.store.reserve_copy_read(0, live, 0, false, true)
        })
        .is_err(),
        "CURRENT_LINEAGE_READER_REQUIRES_THE_EXACT_CURRENT_SHAPE"
    );
}

#[test]
fn checkpoint_rescue_does_not_read_unreachable_history_resources() {
    use crate::owned_inputs::OwnedInputEntry;
    let (temp, core, mut bundle) = fixture();
    let bytes = b"resource retained only by unavailable history";
    let reference = ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::OwnedData,
    };
    let held_blob = bundle
        .store
        .admit_blob(&mut bytes.as_slice(), &reference, &[])
        .unwrap();
    let mut inputs = (*bundle
        .store
        .read_input_manifest(&bundle.root.as_ref().unwrap().input_manifest_ref)
        .unwrap())
    .clone();
    inputs.entries.push(OwnedInputEntry {
        entry_id: "history-only-resource".into(),
        object: reference.clone(),
        dependencies: vec![],
        selected_stream: None,
        time_base: None,
        origin: None,
        profile: None,
    });
    let sealed = bundle.store.admit_record(&inputs).unwrap();
    bundle
        .branches
        .get_mut(&bundle.manual_branch_id)
        .unwrap()
        .input_manifest_ref = sealed.reference.clone();
    bundle.root.as_mut().unwrap().input_manifest_ref = sealed.reference.clone();
    damage_journal(&mut bundle, false);
    drop(sealed);
    drop(held_blob);
    drop(bundle);
    drop(core);
    let unused = temp.path(&format!(
        "source.kinewright-assets/studio/objects/{}",
        reference.sha256
    ));
    std::fs::write(&unused, b"damaged unrelated history resource").unwrap();
    let recovery = VerifiedCheckpointRecovery::open(&temp.path("source.kinewright"))
        .unwrap()
        .unwrap();
    assert!(recovery.inputs.entries.is_empty());
    let copy = temp.path("recovered.kinewright");
    recovery.save_copy(&copy).unwrap();
    assert!(crate::bundle::LoadedBundle::read(&copy).is_ok());
    assert_eq!(
        std::fs::read(unused).unwrap(),
        b"damaged unrelated history resource",
        "UNAVAILABLE_HISTORY_RESOURCES_REMAIN_WITH_THE_ORIGINAL"
    );
}
