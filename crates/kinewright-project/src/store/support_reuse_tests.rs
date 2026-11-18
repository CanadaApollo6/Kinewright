//! Immutable owner proofs save replay work, while cold open always validates.
use super::*;
use crate::{
    bundle::{BundleWriteAdmission, LoadedBundle, ProjectBundle},
    operation_actor::Core,
};
use kinewright_core::{Command, Document, Operation, Track, TrackId, TrackKind};
use kinewright_media::test_support::{TempDirectory, workspace_target_dir};

fn fixture() -> (TempDirectory, Core, ProjectBundle, ObjectRef) {
    let temp = TempDirectory::new_in(&workspace_target_dir(), "owner-publication-proof");
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
    let mut bundle = ProjectBundle::activate(
        &temp.path("p.kinewright"),
        BundleWriteAdmission::new_document(),
        core.history_pool(),
    )
    .unwrap();
    let inputs = bundle.empty_inputs().unwrap().reference.clone();
    bundle.save(&core, &inputs).unwrap();
    (temp, core, bundle, inputs)
}
#[test]
fn fixtures_and_cc7_grading_write_the_same_bytes_and_inventory() {
    let (_temp, core, mut bundle, inputs) = fixture();
    for locked in [false, true, false] {
        core.request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked,
        }))
        .unwrap();
        let report = bundle.save(&core, &inputs).unwrap();
        assert_eq!(
            report.history_sha256,
            core.snapshot_identity().unwrap().to_string(),
            "SAVE_PUBLISHES_THE_EXACT_COMBINED_IDENTITY"
        );
        let checkpoint = bundle.checkpoint(bundle.manual_branch_id()).unwrap();
        let cold = checkpoint.open(&bundle.store, core.history_pool()).unwrap();
        assert_eq!(
            cold.identity(),
            checkpoint.identity,
            "COLD_VALIDATION_EQUALS_THE_WARM_PROOF"
        );
    }
}
#[test]
fn long_history_writes_the_same_bytes_and_inventory() {
    let (temp, core, mut bundle, inputs) = fixture();
    for i in 0_u32..128 {
        core.request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: i.is_multiple_of(2),
        }))
        .unwrap();
    }
    bundle.save(&core, &inputs).unwrap();
    let proof = bundle
        .checkpoint(bundle.manual_branch_id())
        .unwrap()
        .clone();
    bundle.store.clear_verified_memo().unwrap();
    let counts = core
        .history_pool()
        .lease_counts(crate::operation_history::Role::Ordinary)
        .unwrap();
    bundle.save(&core, &inputs).unwrap();
    assert_eq!(
        core.history_pool()
            .lease_counts(crate::operation_history::Role::Ordinary)
            .unwrap(),
        counts,
        "WARM_PUBLICATION_DOES_NOT_OPEN_ANOTHER_HISTORY"
    );
    assert_eq!(
        bundle
            .checkpoint(bundle.manual_branch_id())
            .unwrap()
            .identity,
        proof.identity,
        "RAW_MEMO_RESET_PRESERVES_IMMUTABLE_OWNER_PROOF"
    );
    drop(bundle);
    let loaded = LoadedBundle::read(&temp.path("p.kinewright"), core.history_pool()).unwrap();
    assert_eq!(
        loaded.manual_checkpoint().unwrap().identity,
        proof.identity,
        "COLD_OPEN_VALIDATES_THE_SAME_OWNER_IDENTITY"
    );
}
#[test]
fn bounded_history_at_the_cap_writes_the_same_bytes_and_inventory() {
    let (_temp, core, mut bundle, inputs) = fixture();
    let snapshot = core.export_history().unwrap();
    let reservation = core.export_history().unwrap();
    let counts = core
        .history_pool()
        .lease_counts(crate::operation_history::Role::Ordinary)
        .unwrap();
    assert!(
        core.export_history().is_err(),
        "FULL_POOL_REFUSES_AN_EXTRA_CAPTURE"
    );
    // Full pool cannot authorize a read/validation of another history here.
    // The already validated immutable cut is sufficient for metadata publication.
    bundle.store.clear_verified_memo().unwrap();
    crate::bundle_commit::BundleCommit::publish_maintenance(&mut bundle).unwrap();
    assert_eq!(
        core.history_pool()
            .lease_counts(crate::operation_history::Role::Ordinary)
            .unwrap(),
        counts,
        "METADATA_PUBLICATION_USES_FIXED_PROOFS_AT_A_FULL_POOL"
    );
    drop(reservation);
    drop(snapshot);
    bundle.save(&core, &inputs).unwrap();
}
#[test]
fn fields_checked_for_other_bytes_are_not_used() {
    let (_temp, core, mut bundle, _inputs) = fixture();
    let mut wrong = bundle
        .checkpoint(bundle.manual_branch_id())
        .unwrap()
        .clone();
    let other = bundle.store.admit_document(&Document::default()).unwrap();
    wrong.document_ref = other.reference.clone();
    assert!(
        wrong.open(&bundle.store, core.history_pool()).is_err(),
        "OWNER_PROOF_IS_BOUND_TO_ITS_EXACT_DOCUMENT_PROJECTION"
    );
    let mut wrong = bundle
        .checkpoint(bundle.manual_branch_id())
        .unwrap()
        .clone();
    wrong.identity.owner[0] ^= 1;
    assert!(
        wrong.open(&bundle.store, core.history_pool()).is_err(),
        "OWNER_PROOF_REQUIRES_THE_COMBINED_IDENTITY"
    );
}
#[test]
fn exact_live_history_limit_refuses_the_same_with_and_without_reuse() {
    let (temp, core, bundle, _) = fixture();
    let root = std::fs::read(temp.path("p.kinewright")).unwrap();
    drop(bundle);
    let mut limits = crate::operation_history::PoolLimits::default();
    limits.retained[0] = crate::operation_history::Role::Ordinary.envelope().retained - 1;
    let pool = crate::operation_history::HistoryPool::new(limits);
    assert!(
        LoadedBundle::read(&temp.path("p.kinewright"), &pool).is_err(),
        "COLD_OPEN_CANNOT_BORROW_A_WARM_PROOF_TO_BYPASS_ADMISSION"
    );
    assert_eq!(
        std::fs::read(temp.path("p.kinewright")).unwrap(),
        root,
        "REFUSAL_LEAVES_THE_DURABLE_ROOT_UNCHANGED"
    );
    assert!(LoadedBundle::read(&temp.path("p.kinewright"), core.history_pool()).is_ok());
}
