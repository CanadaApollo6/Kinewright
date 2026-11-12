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

fn history_admission_effect(keys: i64) -> kinewright_core::Effect {
    use kinewright_core::{
        AutomationCurve, Effect, EffectId, Keyframe, KeyframeInterpolation, ParamValue, TimeCode,
    };
    Effect {
        id: EffectId(1),
        name: "primary_correction".into(),
        enabled: true,
        enabled_curve: None,
        parameters: BTreeMap::from([("exposure_milli_stops".into(), ParamValue::Integer(0))]),
        keyframes: BTreeMap::from([(
            "exposure_milli_stops".into(),
            AutomationCurve {
                keyframes: (0..keys)
                    .map(|at| Keyframe {
                        at: TimeCode(at),
                        value: 0,
                        interpolation: KeyframeInterpolation::Linear,
                        tangent_in: 0,
                        tangent_out: 0,
                    })
                    .collect(),
            },
        )]),
    }
}

fn history_admission_split_core() -> Core {
    use kinewright_core::{ClipId, Event, SolidColor, TimeCode};
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
    let apply = |operation| {
        let event = core.request(Command::Do(operation)).unwrap();
        assert!(matches!(event, Event::DocumentChanged { .. }), "{event:?}");
    };
    apply(Operation::AddSolidClip {
        track: TrackId(1),
        timeline_start: TimeCode(0),
        duration: TimeCode(90),
        color: SolidColor { r: 0, g: 0, b: 0 },
    });
    apply(Operation::AddEffect {
        clip: ClipId(1),
        effect: history_admission_effect(10),
    });
    for split in 1..=60 {
        apply(Operation::SplitClip {
            clip: ClipId(split),
            at: TimeCode(i64::try_from(split).unwrap()),
        });
    }
    let event = core
        .request(Command::DoBatch(
            (2..=61)
                .map(|id| Operation::DeleteClip { clip: ClipId(id) })
                .collect(),
        ))
        .unwrap();
    assert!(matches!(event, Event::DocumentChanged { .. }), "{event:?}");
    core
}

fn history_admission_copy_core() -> Core {
    use kinewright_core::{ClipId, EffectId, Event, SolidColor, TimeCode};
    let effect = history_admission_effect(1000);
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
    for at in 0..10 {
        assert!(matches!(
            core.request(Command::Do(Operation::AddSolidClip {
                track: TrackId(1),
                timeline_start: TimeCode(at * 1000),
                duration: TimeCode(1000),
                color: SolidColor { r: 0, g: 0, b: 0 },
            }))
            .unwrap(),
            Event::DocumentChanged { .. }
        ));
    }
    assert!(matches!(
        core.request(Command::Do(Operation::AddEffect {
            clip: ClipId(1),
            effect
        }))
        .unwrap(),
        Event::DocumentChanged { .. }
    ));
    for operations in [
        (2..=10)
            .map(|id| Operation::CopyClipAttributes {
                from_clip: ClipId(1),
                to_clip: ClipId(id),
                names: None,
                include_keyframes: true,
            })
            .collect(),
        (2..=10)
            .map(|id| Operation::RemoveEffect {
                clip: ClipId(id),
                effect: EffectId(1),
            })
            .collect(),
    ] {
        assert!(matches!(
            core.request(Command::DoBatch(operations)).unwrap(),
            Event::DocumentChanged { .. }
        ));
    }
    core
}

#[test]
fn history_admission_copy_remove_refused_save_retry_cold_reopen_keeps_complete_undo() {
    let temp = TempDirectory::new_in(&workspace_target_dir(), "history-admission-copy");
    let path = temp.path("source.kinewright");
    let core = history_admission_copy_core();
    let before = core.export_history().unwrap();
    let mut bundle = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    let inputs = bundle.empty_inputs().unwrap();
    let refused = crate::io_observer::fail_once("root_before_publish", || {
        bundle.save(&core, &inputs.reference)
    });
    assert!(
        refused
            .unwrap_err()
            .to_string()
            .contains("injected root_before_publish"),
        "SAVE_REACHED_THE_REAL_PUBLICATION_REFUSAL"
    );
    assert_eq!(core.export_history().unwrap(), before);
    let usage = bundle.store.usage().unwrap();
    let available = bundle.store.limits().metadata_bytes
        - usage.metadata_bytes
        - (usage.decoded_bytes - usage.decoder_bytes);
    let mut pressure = Vec::new();
    let mut remaining = available - 1;
    while remaining != 0 {
        let chunk = remaining.min(bundle.store.limits().decoded_record_bytes);
        pressure.push(
            bundle
                .store
                .reserve_copy_read(0, chunk, 0, false, true)
                .unwrap(),
        );
        remaining -= chunk;
    }
    for _ in 0..2 {
        assert!(
            matches!(
                bundle.save(&core, &inputs.reference),
                Err(BundleError::Capacity(_))
            ),
            "RETRY_CANNOT_BYPASS_STILL_PRESENT_CAPACITY_REFUSAL"
        );
        assert_eq!(core.export_history().unwrap(), before);
    }
    drop(pressure);
    bundle.save(&core, &inputs.reference).unwrap();
    assert_eq!(
        core.export_history().unwrap(),
        before,
        "RETRY_KEEPS_ALL_UNDO"
    );
    drop(inputs);
    drop(bundle);
    let root = std::fs::read(&path).unwrap();
    let loaded = crate::bundle::LoadedBundle::read(&path).unwrap();
    let restored = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&restored).unwrap();
    assert_eq!(restored.export_history().unwrap(), before);
    history_admission_assert_cursor_ends(&restored, &before);
    drop(loaded);
    assert!(VerifiedCheckpointRecovery::open(&path).unwrap().is_none());
    assert_eq!(std::fs::read(path).unwrap(), root);
}

fn history_admission_assert_cursor_ends(core: &Core, before: &HistorySnapshot) {
    use kinewright_core::Event;
    for expected in before.cursor.undo.iter().rev() {
        assert!(matches!(
            core.request(Command::Undo).unwrap(),
            Event::DocumentChanged { .. }
        ));
        assert_eq!(core.export_history().unwrap().document, expected.document);
    }
    assert!(core.export_history().unwrap().cursor.undo.is_empty());
    for expected in before
        .cursor
        .undo
        .iter()
        .skip(1)
        .map(|entry| &entry.document)
        .chain(std::iter::once(&before.document))
    {
        assert!(matches!(
            core.request(Command::Redo).unwrap(),
            Event::DocumentChanged { .. }
        ));
        assert_eq!(&core.export_history().unwrap().document, expected);
    }
    assert!(core.export_history().unwrap().cursor.redo.is_empty());
}

#[test]
fn history_admission_split_delete_save_cold_reopen_keeps_complete_undo() {
    let temp = TempDirectory::new_in(&workspace_target_dir(), "history-admission-split");
    let path = temp.path("source.kinewright");
    let core = history_admission_split_core();
    let before = core.export_history().unwrap();
    assert_eq!(before.batches.len(), 63);
    println!(
        "HISTORY_SPLIT_ADMISSION replay={} retained={} encoded={}",
        kinewright_core::history_size::measure_replay_peak(
            &kinewright_core::HistoryJournal::capture(&before).unwrap(),
            &before.baseline,
            &before.document,
        )
        .unwrap(),
        kinewright_core::history_size::history_live_bytes(&before).unwrap(),
        before.encoded_bytes().unwrap(),
    );
    let mut bundle = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    let inputs = bundle.empty_inputs().unwrap();
    bundle.save(&core, &inputs.reference).unwrap();
    assert_eq!(
        core.export_history().unwrap(),
        before,
        "SAVE_KEEPS_ALL_UNDO"
    );
    drop(inputs);
    drop(bundle);
    let root_bytes = std::fs::read(&path).unwrap();
    let loaded =
        crate::bundle::LoadedBundle::read(&path).expect("ADMITTED_SPLIT_HISTORY_COLD_REOPEN");
    let restored = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&restored).unwrap();
    assert_eq!(restored.export_history().unwrap(), before);
    history_admission_assert_cursor_ends(&restored, &before);
    drop(loaded);
    assert!(VerifiedCheckpointRecovery::open(&path).unwrap().is_none());
    assert_eq!(std::fs::read(path).unwrap(), root_bytes);
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

#[test]
fn history_admission_reopen_capacity_never_authorizes_empty_undo_recovery() {
    let (temp, core, bundle) = fixture();
    let before = core.export_history().unwrap();
    let path = temp.path("source.kinewright");
    drop(bundle);
    let root = std::fs::read(&path).unwrap();
    let limits = StorageLimits {
        live_history_bytes: 1,
        ..StorageLimits::default()
    };
    assert!(
        matches!(
            crate::bundle_recovery::read_with_limits(&path, limits.clone()),
            Err(BundleError::Capacity(_))
        ),
        "REOPEN_SIZE_IS_CAPACITY"
    );
    assert!(
        matches!(
            VerifiedCheckpointRecovery::open_with_limits(&path, &limits),
            Err(BundleError::Capacity(_))
        ),
        "CAPACITY_GRANTS_NO_RESCUE"
    );
    assert_eq!(std::fs::read(&path).unwrap(), root);
    let loaded = crate::bundle::LoadedBundle::read(&path).unwrap();
    let restored = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&restored).unwrap();
    assert_eq!(restored.export_history().unwrap(), before);
    history_admission_assert_cursor_ends(&restored, &before);
}
