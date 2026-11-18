//! Full owner-state checks at public installation boundaries.
#![cfg(all(feature = "test-util", target_os = "linux"))]
use kinewright_core::{
    AssetId, ColorDescription, Command, Document, Event, MediaAsset, MediaKind,
    MediaSourceFingerprint, Operation, Rational, SolidColor, TimeCode, Track, TrackId, TrackKind,
};
use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
use kinewright_project::{
    bundle_copy::CopyStatus,
    bundle_records::{BatchId, ObjectRef, RecordKind},
    bundle_recovery::RecoverySelection,
    bundle_switch::ProjectSelectionResult,
    editing_session::Ja1EditingSession,
    operation_actor::{CommandContext, Core, PortableHistory},
    operation_history::{
        Approval, ApprovalAnswer, CreationOrigin, HistoryPool, PoolLimits, RemovedTarget,
    },
    storage_budget::StorageLimits,
};
use serde_json::Value;
const EVIDENCE: &[u8] = b"door audit approval";
fn evidence() -> ObjectRef {
    ObjectRef {
        sha256: kinewright_media::sha256_bytes(EVIDENCE),
        byte_length: EVIDENCE.len() as u64,
        record_kind: RecordKind::OwnedEvidence,
    }
}
fn baseline() -> Document {
    let mut doc = Document {
        tracks: vec![Track {
            name: None,
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        media_pool: vec![MediaAsset {
            remote_locator: None,
            id: AssetId(50),
            path: "external-unhashed.wav".into(),
            name: "external".into(),
            duration: TimeCode(60),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::Audio,
            resolution: None,
            source_fingerprint: MediaSourceFingerprint::default(),
            color_description: ColorDescription::default(),
            assumed_from: None,
        }],
        ..Document::default()
    };
    for start in [0, 30] {
        Operation::AddSolidClip {
            track: TrackId(1),
            timeline_start: TimeCode(start),
            duration: TimeCode(30),
            color: SolidColor { r: 0, g: 0, b: 0 },
        }
        .apply(&mut doc)
        .unwrap();
    }
    doc
}
fn admit_evidence(owner: &mut Ja1EditingSession) {
    let mut bytes = EVIDENCE;
    owner
        .source_mut()
        .unwrap()
        .store_mut()
        .admit_blob(&mut bytes, &evidence(), &[])
        .unwrap();
}
fn populated(root: &std::path::Path) -> Ja1EditingSession {
    let core = Core::for_test(baseline()).unwrap();
    let mut owner = Ja1EditingSession::new(root, &core, StorageLimits::default()).unwrap();
    admit_evidence(&mut owner);
    populate_history(&core, kinewright_core::ClipId(1));
    let snapshot = core.export_history().unwrap();
    assert_eq!(
        snapshot.history().undo.len(),
        1,
        "DOOR_FIXTURE_UNDO_NONEMPTY"
    );
    assert_eq!(
        snapshot.history().redo.len(),
        1,
        "DOOR_FIXTURE_REDO_NONEMPTY"
    );
    assert!(
        !snapshot.resources().is_empty(),
        "DOOR_FIXTURE_RESOURCE_NONEMPTY"
    );
    assert_eq!(
        core.approval_answer(evidence()).unwrap(),
        Some(ApprovalAnswer::Kept),
        "DOOR_FIXTURE_APPROVAL_APPLIED"
    );
    owner
}
fn populate_history(core: &Core, clip: kinewright_core::ClipId) {
    assert!(matches!(
        core.request_with_context(
            Command::Do(Operation::DeleteClip { clip }),
            CommandContext {
                creation: CreationOrigin {
                    branch: core.creation_branch().unwrap(),
                    batch: BatchId::fresh().unwrap()
                },
                inputs: vec![],
                approval: Some(Approval {
                    reference: evidence(),
                    targets: vec![RemovedTarget::Clip(clip)]
                }),
                transition: None,
            }
        )
        .unwrap(),
        Event::DocumentChanged { .. }
    ));
    core.request(Command::Do(Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked: false,
    }))
    .unwrap();
    core.request(Command::Undo).unwrap();
}
fn state(core: &Core) -> Value {
    serde_json::to_value(core.export_history().unwrap()).unwrap()
}
fn same_owner(core: &Core, expected: &Value, door: &str) {
    let actual = state(core);
    let mut expected = expected.clone();
    let old = expected["history"]["epoch"].as_u64().unwrap();
    let new = actual["history"]["epoch"].as_u64().unwrap();
    assert!(new >= old, "DOOR_EPOCH_NEVER_DECREASES: {door}");
    if new != old {
        expected["history"]["epoch"] = new.into();
    }
    assert!(
        actual == expected,
        "DOOR_PRESERVES_DOCUMENT_STACKS_KEYS_LEDGER_LINEAGE_RESOURCES: {door}: {}",
        difference(&actual, &expected, "owner")
    );
    assert_eq!(
        core.approval_answer(evidence()).unwrap(),
        Some(ApprovalAnswer::Kept),
        "DOOR_APPROVAL_IS_COMMAND_FATE: {door}"
    );
}
fn difference(actual: &Value, expected: &Value, path: &str) -> String {
    if actual == expected {
        return String::new();
    }
    if let (Some(a), Some(b)) = (actual.as_object(), expected.as_object()) {
        for key in a.keys().chain(b.keys()) {
            if a.get(key) != b.get(key) {
                return difference(&actual[key], &expected[key], &format!("{path}.{key}"));
            }
        }
    }
    if let (Some(a), Some(b)) = (actual.as_array(), expected.as_array()) {
        if a.len() != b.len() {
            return format!("{path}: lengths {} != {}", a.len(), b.len());
        }
        for (index, (left, right)) in a.iter().zip(b).enumerate() {
            if left != right {
                return difference(left, right, &format!("{path}[{index}]"));
            }
        }
    }
    format!("{path}: {actual} != {expected}")
}
fn first_save(owner: &mut Ja1EditingSession, path: &std::path::Path) {
    assert!(matches!(
        owner.first_save(path, StorageLimits::default()).unwrap(),
        CopyStatus::Committed(_)
    ));
}
#[test]
fn first_save_save_and_cold_open_preserve_the_complete_owner() {
    let temp = TempDirectory::new_in(&workspace_target_dir(), "door-first-save");
    let path = temp.path("project.kinewright");
    let mut owner = populated(temp.root());
    let expected = state(owner.core());
    first_save(&mut owner, &path);
    same_owner(owner.core(), &expected, "first save rebound");
    owner.save_active().unwrap();
    same_owner(owner.core(), &expected, "ordinary save");
    let saved = state(owner.core());
    drop(owner);
    let pool = HistoryPool::new(PoolLimits::default());
    let reopened = Ja1EditingSession::open(&path, &pool).unwrap();
    same_owner(reopened.core(), &saved, "cold open");
}
#[test]
fn save_as_checkpoint_and_working_branch_preserve_the_complete_owner() {
    let temp = TempDirectory::new_in(&workspace_target_dir(), "door-save-as");
    let mut owner = populated(temp.root());
    first_save(&mut owner, &temp.path("original.kinewright"));
    let expected = state(owner.core());
    assert!(matches!(
        owner
            .save_as(&temp.path("copy.kinewright"), StorageLimits::default())
            .unwrap(),
        CopyStatus::Committed(_)
    ));
    same_owner(owner.core(), &expected, "Save As");
    let (target, _, _) = owner
        .list_working_branches()
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let checkpoint = owner
        .list_recovery_points()
        .unwrap()
        .into_iter()
        .find(|point| matches!(point, RecoverySelection::Checkpoint { .. }))
        .unwrap();
    owner.prepare_recovery(checkpoint).unwrap();
    let selected = owner.select_local();
    assert!(
        matches!(selected, ProjectSelectionResult::Activated(_)),
        "PERSON_CHECKPOINT_ACTIVATES: {selected:?}"
    );
    same_owner(owner.core(), &expected, "person selected checkpoint");
    owner.core().request(Command::Redo).unwrap();
    owner
        .prepare_recovery(RecoverySelection::WorkingBranch(target))
        .unwrap();
    let selected = owner.select_local();
    assert!(
        matches!(selected, ProjectSelectionResult::Activated(_)),
        "PERSON_WORKING_BRANCH_ACTIVATES: {selected:?}"
    );
    same_owner(owner.core(), &expected, "person selected working branch");
}
#[test]
fn host_shadow_install_preserves_both_stacks_and_exact_approval() {
    let temp = TempDirectory::new_in(&workspace_target_dir(), "door-host-shadow");
    let mut source = populated(temp.root());
    first_save(&mut source, &temp.path("source.kinewright"));
    let mut expected = state(source.core());
    // A peer which had no such approval changes its fate from absent to applied.
    let epoch = expected["history"]["epoch"].clone();
    for entry in expected["ledger"]["commands"].as_array_mut().unwrap() {
        entry["install_epoch"] = epoch.clone();
    }
    // Distinct process simulation: only bytes cross into the receiving pool.
    let wire = serde_json::to_vec(&source.core().portable_history().unwrap()).unwrap();
    let receiver = Core::for_test(Document::default()).unwrap();
    let mut shadow =
        Ja1EditingSession::new(temp.root(), &receiver, StorageLimits::default()).unwrap();
    admit_evidence(&mut shadow);
    let portable: PortableHistory = serde_json::from_slice(&wire).unwrap();
    shadow.install_host_snapshot(portable).unwrap();
    same_owner(shadow.core(), &expected, "host shadow adoption");
}

#[test]
fn task_reopen_installs_only_the_current_persisted_branch_with_its_fixed_policy() {
    use kinewright_project::editing_session::DurableBranchRole;
    let temp = TempDirectory::new_in(&workspace_target_dir(), "door-task-reopen");
    let mut owner = populated(temp.root());
    first_save(&mut owner, &temp.path("project.kinewright"));
    let mut guard = owner
        .fork_durable_branch("task", DurableBranchRole::Session)
        .unwrap();
    populate_history(guard.core(), kinewright_core::ClipId(2));
    let expected = state(guard.core());
    assert_eq!(
        expected["history"]["policy"], "NeverFold",
        "TASK_REOPEN_SOURCE_NEVER_FOLDS"
    );
    let branch = guard.branch_id().clone();
    let base = owner.persist_durable_branch(&mut guard).unwrap();
    owner.close_durable_branch(&guard).unwrap();
    drop(guard);
    let reopened = owner
        .reopen_durable_branch(branch, &base, "reopened", DurableBranchRole::Session)
        .unwrap();
    same_owner(reopened.core(), &expected, "own current persisted task");
    assert_eq!(
        state(reopened.core())["history"]["policy"],
        "NeverFold",
        "TASK_REOPEN_PRESERVES_NEVER_FOLD"
    );
    let preview = reopened.core().boundary(0).unwrap();
    assert_eq!(
        preview.document().tracks[0].clips.len(),
        1,
        "TASK_PREVIEW_USES_ITS_OWN_BASELINE"
    );
}

#[test]
fn rescue_after_parent_loss_preserves_the_complete_owner_and_approval() {
    let temp = TempDirectory::new_in(&workspace_target_dir(), "door-rescue");
    let drive = temp.path("drive");
    std::fs::create_dir(&drive).unwrap();
    let mut owner = populated(temp.root());
    first_save(&mut owner, &drive.join("project.kinewright"));
    owner.core().close_gesture().unwrap();
    let expected = state(owner.core());
    assert!(
        kinewright_project::io_observer::fail_once("root_parent_sync", || owner.save_active())
            .is_err()
    );
    std::fs::rename(&drive, temp.path("missing-drive")).unwrap();
    let offer = owner.probe_active().unwrap().unwrap();
    let capture = owner.prepare_rescue(&offer).unwrap().unwrap();
    let selected = owner
        .preview_rescue(capture, StorageLimits::default())
        .unwrap();
    let consent = selected.consent();
    let destination = owner
        .inspect_rescue_destination(&temp.path("rescued.kinewright"))
        .unwrap();
    let outcome = owner.rescue(selected, &consent, &destination).unwrap();
    assert!(
        outcome.activated,
        "PERSON_RESCUE_ACTIVATES_WITHOUT_REAPPROVING_COMMAND"
    );
    same_owner(owner.core(), &expected, "rescue with full history");
}
