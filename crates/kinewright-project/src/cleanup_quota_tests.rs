//! A task's immutable base uses one version slot, independent of its tail.
use crate::operation_actor::Core;
use crate::{
    bundle::BundleError,
    editing_session::{DurableBranchRole, Ja1EditingSession},
    storage_budget::StorageLimits,
};
use kinewright_core::{Command, Document, Operation, Track, TrackId, TrackKind};

#[test]
fn run_limit_cleanup_reserves_specialist_versions_and_reuses_a_full_raw_slot() {
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "fixed-base-version-room",
    );
    let core = Core::for_test(Document {
        tracks: vec![Track {
            name: None,
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: false,
            clips: vec![],
        }],
        ..Document::default()
    })
    .unwrap();
    let mut owner = Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
    owner
        .first_save(&temp.path("p.kinewright"), StorageLimits::default())
        .unwrap();
    owner
        .restrict_limits_probe(StorageLimits {
            versions: 1,
            ..StorageLimits::default()
        })
        .unwrap();
    let mut branch = owner
        .fork_durable_branch("quota witness", DurableBranchRole::Session)
        .unwrap();
    let base = owner.persist_durable_branch(&mut branch).unwrap();
    let bytes = owner
        .store()
        .unwrap()
        .read_record::<crate::bundle_records::SpecialistBaseRecord>(&base)
        .unwrap();
    let original = serde_json::to_vec(&*bytes).unwrap();
    drop(bytes);
    for edit in 0_u32..32 {
        branch
            .core()
            .request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: edit.is_multiple_of(2),
            }))
            .unwrap();
        assert_eq!(
            owner.persist_durable_branch(&mut branch).unwrap(),
            base,
            "FULL_VERSION_SLOT_REUSES_THE_IMMUTABLE_BASE"
        );
    }
    assert_eq!(
        branch.core().export_history().unwrap().history().undo.len(),
        32,
        "IMMUTABLE_BASE_NEVER_TRIMS_THE_TASK_TAIL"
    );
    let current = owner
        .store()
        .unwrap()
        .read_record::<crate::bundle_records::SpecialistBaseRecord>(&base)
        .unwrap();
    assert_eq!(
        serde_json::to_vec(&*current).unwrap(),
        original,
        "BASE_CONTAINS_NO_DUPLICATED_GROWING_DELTA"
    );
    drop(current);
    core.request(Command::Do(Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked: true,
    }))
    .unwrap();
    owner.save_active().unwrap();
    let root = owner.root_sha256().unwrap();
    let before = core.change_id().unwrap();
    assert!(
        matches!(
            owner.fork_durable_branch("different base", DurableBranchRole::Session),
            Err(BundleError::Capacity(_) | BundleError::RunLimit)
        ),
        "A_DIFFERENT_BASE_CANNOT_BYPASS_THE_VERSION_CAP"
    );
    assert_eq!(
        owner.root_sha256().unwrap(),
        root,
        "BASE_CAPACITY_REFUSAL_PUBLISHES_NOTHING"
    );
    assert_eq!(
        core.change_id().unwrap(),
        before,
        "BASE_CAPACITY_REFUSAL_KEEPS_SOURCE_WORK"
    );
    assert_eq!(
        branch.core().export_history().unwrap().history().undo.len(),
        32
    );
}
