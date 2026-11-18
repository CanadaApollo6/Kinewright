//! Verified checkpoint rescue retains failed history and original occurrence identities.
use super::*;
use crate::operation_actor::Core;
use kinewright_core::{Command, Operation, Track, TrackId, TrackKind};
use kinewright_media::test_support::{TempDirectory, workspace_target_dir};

fn fixture() -> (TempDirectory, Core, ProjectBundle) {
    fixture_with(1)
}
fn fixture_with(toggles: usize) -> (TempDirectory, Core, ProjectBundle) {
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
    let core = Core::for_test(document).unwrap();
    for toggle in 0..toggles {
        core.request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: toggle % 2 == 1,
        }))
        .unwrap();
    }
    let mut bundle = ProjectBundle::activate(
        &temp.path("source.kinewright"),
        BundleWriteAdmission::new_document(),
        core.history_pool(),
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
    let core = Core::for_test(Document {
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
    let core = Core::for_test(Document {
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

fn reopen(path: &Path, pool: &crate::operation_history::HistoryPool) -> Core {
    let mut loaded = crate::bundle::LoadedBundle::read(path, pool).unwrap();
    let checkpoint = loaded.manual_checkpoint().unwrap().clone();
    Core::open(
        loaded.take_manual_snapshot().unwrap(),
        checkpoint.branch_id,
        checkpoint.revision,
    )
    .unwrap()
}
fn roundtrip(core: &Core, label: &str) {
    let temp = TempDirectory::new_in(&workspace_target_dir(), label);
    let path = temp.path("source.kinewright");
    let pool = core.history_pool().clone();
    let expected = core.export_history().unwrap();
    let mut bundle =
        ProjectBundle::activate(&path, BundleWriteAdmission::new_document(), &pool).unwrap();
    let inputs = bundle.empty_inputs().unwrap();
    bundle.save(core, &inputs.reference).unwrap();
    let authority = bundle.attach_core(core).unwrap();
    let undo_depth = expected.history().undo.len();
    let cut = authority.freeze().unwrap();
    authority.retire(&cut).unwrap();
    drop(authority);
    drop(bundle);
    let restored = reopen(&path, &pool);
    assert_eq!(
        restored.snapshot_identity().unwrap(),
        expected.identity().unwrap(),
        "COLD_OPEN_PRESERVES_COMPLETE_OWNER"
    );
    drop(expected);
    let baseline = restored.boundary(0).unwrap().document_arc();
    for _ in 0..undo_depth {
        restored.request(Command::Undo).unwrap();
    }
    assert_eq!(
        restored.export_history().unwrap().history().document,
        baseline,
        "FULL_UNDO_REACHES_SAVED_BASELINE"
    );
}
#[test]
fn history_admission_copy_remove_refused_save_retry_cold_reopen_keeps_complete_undo() {
    roundtrip(&history_admission_copy_core(), "copy-owner-roundtrip");
}
#[test]
fn history_admission_split_delete_save_cold_reopen_keeps_complete_undo() {
    roundtrip(&history_admission_split_core(), "split-owner-roundtrip");
}
fn damage_journal(bundle: &mut ProjectBundle, reject_operation: bool) -> BranchCheckpoint {
    let mut checkpoint = bundle.branches[&bundle.manual_branch_id].clone();
    let record = bundle
        .store
        .read_record::<crate::operation_history::StoredOwnerRecord>(&checkpoint.owner_ref)
        .unwrap();
    let mut encoded = serde_json::to_value(&*record).unwrap();
    drop(record);
    if reject_operation {
        encoded["body"]["position"] = 0.into();
    } else {
        encoded["body"]["undo"][0][1][0][2] = serde_json::Value::String("0".repeat(64));
    }
    let invalid: crate::operation_history::StoredOwnerRecord =
        serde_json::from_value(encoded).unwrap();
    let history = bundle.store.admit_record(&invalid).unwrap();
    checkpoint.owner_ref = history.reference.clone();
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
        let pool = core.history_pool().clone();
        let expected = core.export_history().unwrap().history().document.clone();
        let damaged = damage_journal(&mut bundle, reject_operation);
        drop(bundle);
        drop(core);
        let source = temp.path("source.kinewright");
        let original_root = std::fs::read(&source).unwrap();
        let history_path = temp.path(&format!(
            "source.kinewright-assets/studio/objects/{}",
            damaged.owner_ref.sha256
        ));
        let original_history = std::fs::read(&history_path).unwrap();
        assert!(
            crate::bundle::LoadedBundle::read(&source, &pool).is_err(),
            "ORDINARY_OPEN_STAYS_STRICT"
        );
        let recovery = VerifiedCheckpointRecovery::open(&source, &pool)
            .unwrap()
            .unwrap();
        assert_eq!(
            recovery.document(),
            &*expected,
            "RECOVERY_CHECKPOINT_DOCUMENT_EXACT"
        );
        assert_eq!(recovery.disclosure().history_ref, damaged.owner_ref);
        let destination = temp.path("recovered.kinewright");
        recovery.save_copy(&destination).unwrap();
        let restored = reopen(&destination, &pool);
        let history = restored.export_history().unwrap();
        assert_eq!(
            &history.history().document,
            &expected,
            "DOCUMENT_ONLY_RESET_PRESERVES_DOCUMENT"
        );
        assert!(
            history.history().undo.is_empty() && history.history().redo.is_empty(),
            "DOCUMENT_ONLY_RESET_HAS_NO_INVENTED_STEPS"
        );
        assert_eq!(
            history.ledger().entries().count(),
            0,
            "DOCUMENT_ONLY_RESET_INVENTS_NO_APPROVAL_FATES"
        );
        assert!(
            history.history().epoch > damaged.position.epoch,
            "DOCUMENT_ONLY_RESET_STARTS_NEW_EPOCH"
        );
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
            recovery.save_copy(&source).is_err(),
            "RECOVERY_CANNOT_REPLACE_ITS_SOURCE"
        );
        assert!(
            recovery.save_copy(&destination).is_err(),
            "RECOVERY_CANNOT_OVERWRITE_AN_OCCUPIED_DESTINATION"
        );
    }
}
#[test]
fn checkpoint_recovery_requires_an_independently_verified_document() {
    let (temp, core, mut bundle) = fixture();
    let pool = core.history_pool().clone();
    let checkpoint = damage_journal(&mut bundle, true);
    drop(bundle);
    drop(core);
    std::fs::write(
        temp.path(&format!(
            "source.kinewright-assets/studio/objects/{}",
            checkpoint.document_ref.sha256
        )),
        b"bad document",
    )
    .unwrap();
    assert!(
        VerifiedCheckpointRecovery::open(&temp.path("source.kinewright"), &pool).is_err(),
        "CORRUPT_CURRENT_DOCUMENT_GRANTS_NO_RESET"
    );
}
#[test]
fn checkpoint_current_lineage_does_not_replay_unavailable_undo_maps() {
    let (temp, core, mut bundle) = fixture();
    let pool = core.history_pool().clone();
    damage_journal(&mut bundle, true);
    drop(bundle);
    drop(core);
    let recovery = VerifiedCheckpointRecovery::open(&temp.path("source.kinewright"), &pool)
        .unwrap()
        .unwrap();
    recovery.save_copy(&temp.path("reset.kinewright")).unwrap();
    let restored = reopen(&temp.path("reset.kinewright"), &pool);
    let baseline = restored.boundary(0).unwrap();
    assert_eq!(
        baseline.lineage().occurrences.len(),
        1,
        "RESET_CREATES_ONLY_CURRENT_DOCUMENT_SEEDS"
    );
    assert!(
        baseline.lineage().split_ancestry.is_empty(),
        "RESET_INVENTS_NO_OLD_ANCESTRY"
    );
}
#[test]
fn checkpoint_rescue_does_not_read_unreachable_history_resources() {
    let (temp, core, mut bundle) = fixture();
    let pool = core.history_pool().clone();
    let bytes = b"unreachable historical bytes";
    let reference = ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::OwnedData,
    };
    let pin = bundle
        .store
        .admit_blob(&mut bytes.as_slice(), &reference, &[])
        .unwrap();
    damage_journal(&mut bundle, false);
    drop(pin);
    drop(bundle);
    drop(core);
    let unused = temp.path(&format!(
        "source.kinewright-assets/studio/objects/{}",
        reference.sha256
    ));
    std::fs::write(&unused, b"unavailable").unwrap();
    let recovery = VerifiedCheckpointRecovery::open(&temp.path("source.kinewright"), &pool)
        .unwrap()
        .unwrap();
    assert!(recovery.inputs.entries.is_empty());
    recovery.save_copy(&temp.path("reset.kinewright")).unwrap();
    assert!(crate::bundle::LoadedBundle::read(&temp.path("reset.kinewright"), &pool).is_ok());
    assert_eq!(
        std::fs::read(unused).unwrap(),
        b"unavailable",
        "UNAVAILABLE_HISTORY_RESOURCES_REMAIN_WITH_THE_ORIGINAL"
    );
}
#[test]
fn history_admission_reopen_capacity_never_authorizes_empty_undo_recovery() {
    let (temp, core, bundle) = fixture();
    let path = temp.path("source.kinewright");
    drop(bundle);
    drop(core);
    let original = std::fs::read(&path).unwrap();
    let mut limits = crate::operation_history::PoolLimits::default();
    limits.retained[0] = 0;
    let pool = crate::operation_history::HistoryPool::new(limits);
    assert!(
        crate::bundle::LoadedBundle::read(&path, &pool).is_err(),
        "REOPEN_SIZE_IS_CAPACITY"
    );
    assert!(
        matches!(
            VerifiedCheckpointRecovery::open(&path, &pool),
            Err(BundleError::Capacity(_))
        ),
        "CAPACITY_GRANTS_NO_RESCUE"
    );
    assert_eq!(std::fs::read(path).unwrap(), original);
}
fn owner_object(temp: &TempDirectory, bundle: &ProjectBundle) -> (std::path::PathBuf, ObjectRef) {
    let owner = bundle.branches[&bundle.manual_branch_id].owner_ref.clone();
    (
        temp.path(&format!(
            "source.kinewright-assets/studio/objects/{}",
            owner.sha256
        )),
        owner,
    )
}
/// A history read that fails for a reason other than its stored bytes is a
/// refusal: no rescue capability, nothing written, and the same project opens
/// once the cause is gone.
fn assert_retryable_refusal(
    refused: Result<Option<VerifiedCheckpointRecovery>, BundleError>,
    expected: impl FnOnce(&BundleError) -> bool,
    path: &Path,
    original: &[u8],
    context: &str,
) {
    match refused {
        Err(error) => assert!(expected(&error), "{context}: unexpected refusal {error:?}"),
        Ok(Some(recovery)) => panic!(
            "{context}: STORE_ERROR_GRANTS_NO_EMPTY_HISTORY_RESCUE ({})",
            recovery.disclosure().reason
        ),
        Ok(None) => panic!("{context}: the failing read was not reached"),
    }
    assert_eq!(
        std::fs::read(path).unwrap(),
        original,
        "{context}: ROOT_UNTOUCHED"
    );
}
#[test]
fn unreadable_history_refuses_rescue_and_stays_recoverable() {
    use std::os::unix::fs::PermissionsExt;
    let (temp, core, bundle) = fixture();
    let pool = core.history_pool().clone();
    let path = temp.path("source.kinewright");
    let (object, _) = owner_object(&temp, &bundle);
    drop(bundle);
    drop(core);
    let original = std::fs::read(&path).unwrap();
    let history = std::fs::read(&object).unwrap();
    let mode = std::fs::metadata(&object).unwrap().permissions();
    std::fs::set_permissions(&object, std::fs::Permissions::from_mode(0o000)).unwrap();
    assert!(
        std::fs::File::open(&object).is_err(),
        "EACCES_FIXTURE_NEEDS_A_NON_ROOT_RUNNER"
    );
    let refused = VerifiedCheckpointRecovery::open(&path, &pool);
    std::fs::set_permissions(&object, mode).unwrap();
    assert_retryable_refusal(
        refused,
        |error| {
            matches!(
                error,
                BundleError::Operation(crate::operation_actor::Error::History(
                    crate::operation_history::Error::Store(reason)
                )) if reason.contains("Permission denied")
            )
        },
        &path,
        &original,
        "EACCES",
    );
    assert_eq!(
        std::fs::read(&object).unwrap(),
        history,
        "HISTORY_UNTOUCHED"
    );
    let restored = reopen(&path, &pool);
    assert!(
        !restored.export_history().unwrap().history().undo.is_empty(),
        "EACCES_RETRY_KEEPS_UNDO"
    );
}
#[test]
fn checkpoint_overlong_history_offers_empty_history_rescue() {
    use std::io::Write as _;
    let (temp, core, bundle) = fixture();
    let pool = core.history_pool().clone();
    let path = temp.path("source.kinewright");
    let (object, owner) = owner_object(&temp, &bundle);
    drop(bundle);
    drop(core);
    let original = std::fs::read(&path).unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&object)
        .unwrap()
        .write_all(b"!")
        .unwrap();
    let overlong = std::fs::read(&object).unwrap();
    assert_eq!(overlong.len() as u64, owner.byte_length + 1);
    let recovery = VerifiedCheckpointRecovery::open(&path, &pool)
        .unwrap()
        .expect("an over-long history object must allow checkpoint rescue");
    assert_eq!(recovery.disclosure().history_ref, owner);
    assert!(
        recovery
            .disclosure()
            .reason
            .contains("owned byte length changed")
    );
    let destination = temp.path("recovered.kinewright");
    recovery.save_copy(&destination).unwrap();
    let restored = reopen(&destination, &pool);
    let history = restored.export_history().unwrap();
    assert!(history.history().undo.is_empty() && history.history().redo.is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(std::fs::read(&object).unwrap(), overlong);
}

#[test]
fn checkpoint_history_read_failure_refuses_empty_history_rescue() {
    let (temp, core, bundle) = fixture();
    let pool = core.history_pool().clone();
    let path = temp.path("source.kinewright");
    let (object, owner) = owner_object(&temp, &bundle);
    let expected = core.snapshot_identity().unwrap();
    drop(bundle);
    drop(core);
    let original = std::fs::read(&path).unwrap();
    let history = std::fs::read(&object).unwrap();
    let refused =
        crate::io_observer::fail_path("owned_verification_read", Path::new(&owner.sha256), || {
            VerifiedCheckpointRecovery::open(&path, &pool)
        });
    assert_retryable_refusal(
        refused,
        |error| {
            matches!(
                error,
                BundleError::Operation(crate::operation_actor::Error::History(
                    crate::operation_history::Error::Store(reason)
                )) if reason == "injected owned_verification_read"
            )
        },
        &path,
        &original,
        "mid-object read failure",
    );
    assert_eq!(std::fs::read(&object).unwrap(), history);
    let restored = reopen(&path, &pool);
    assert_eq!(restored.snapshot_identity().unwrap(), expected);
    assert!(!restored.export_history().unwrap().history().undo.is_empty());
}

#[test]
fn history_store_capacity_refuses_rescue_and_stays_recoverable() {
    // A long Undo over a tiny document: the owner record is the only object
    // whose encoded or decoded size reaches the cap below.
    let (temp, core, bundle) = fixture_with(8000);
    let pool = core.history_pool().clone();
    let path = temp.path("source.kinewright");
    let (_, owner) = owner_object(&temp, &bundle);
    let largest_other = bundle
        .store
        .manifest()
        .unwrap()
        .objects
        .iter()
        .filter(|reference| reference.sha256 != owner.sha256)
        .map(|reference| reference.byte_length)
        .max()
        .unwrap();
    drop(bundle);
    drop(core);
    let original = std::fs::read(&path).unwrap();
    // Every other record still fits; only the owner record is over the cap.
    let limits = StorageLimits {
        decoded_record_bytes: owner.byte_length - 1,
        ..StorageLimits::default()
    };
    assert!(
        largest_other < limits.decoded_record_bytes,
        "STORE_CAPACITY_FIXTURE_TARGETS_ONLY_THE_OWNER_RECORD"
    );
    assert_retryable_refusal(
        VerifiedCheckpointRecovery::open_with_limits(&path, &limits, &pool),
        |error| matches!(error, BundleError::Capacity("encoded metadata record")),
        &path,
        &original,
        "STORE_CAPACITY",
    );
    let restored = reopen(&path, &pool);
    assert!(
        !restored.export_history().unwrap().history().undo.is_empty(),
        "STORE_CAPACITY_RETRY_KEEPS_UNDO"
    );
}
