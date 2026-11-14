use super::tests::{approval, document, origin, remove};
use super::*;
use kinewright_core::{Command, Marker, MarkerId, Operation, TimeCode, undo::RetentionPolicy};
fn marker(id: u64) -> Command {
    Command::Do(Operation::AddMarker {
        marker: Marker {
            id: MarkerId(id),
            position: TimeCode(0),
            label: format!("marker {id}"),
            color_token: 0,
        },
    })
}
fn make(pool: &HistoryPool) -> OwnerHistory {
    OwnerHistory::measured(
        pool,
        Role::Ordinary,
        RetentionPolicy::Fold,
        &origin(1),
        kinewright_core::undo::RoleBounds {
            step_cap: 1,
            ..Role::Ordinary.bounds()
        },
        || Ok(document()),
    )
    .unwrap()
}
#[test]
fn sibling_folded_do_with_no_approval_fact_is_uncertain() {
    let pool = HistoryPool::new(PoolLimits::default());
    let bounds = kinewright_core::undo::RoleBounds {
        step_cap: 1,
        ..Role::Ordinary.bounds()
    };
    let make = || {
        OwnerHistory::measured(
            &pool,
            Role::Ordinary,
            RetentionPolicy::Fold,
            &origin(1),
            bounds,
            || Ok(document()),
        )
        .unwrap()
    };
    let mut approved = make();
    let first = approved.capture().unwrap();
    let second = approved.capture().unwrap();
    let mut sibling = make();
    approved
        .install_snapshot(first.validate_bounded(bounds).unwrap())
        .unwrap();
    sibling
        .install_snapshot(second.validate_bounded(bounds).unwrap())
        .unwrap();
    let b = sibling
        .execute(remove(), origin(2), vec![], None)
        .unwrap()
        .accepted;
    let b_digest = sibling.history().undo()[0].commands()[0].payload_digest;
    let folded = sibling
        .execute(marker(99), origin(3), vec![], None)
        .unwrap();
    assert!(
        folded.folded.iter().any(|command| command.key == b),
        "SIBLING_EVENT_PROVES_ORIGINAL_COMMAND_FOLDED"
    );
    let selected = sibling.capture().unwrap();
    assert!(
        selected.data.history.baseline.tracks.is_empty(),
        "SIBLING_FOLDED_BASELINE_HAS_REMOVAL"
    );
    assert!(
        selected.data.ledger.is_empty() && selected.data.folded_approvals.is_empty(),
        "SIBLING_SNAPSHOT_PREDATES_APPROVAL_EVIDENCE"
    );
    assert!(
        selected
            .data
            .fold_frontier
            .is_some_and(|frontier| b < frontier),
        "SIBLING_COMMAND_OLDER_THAN_FOLD_FRONTIER"
    );
    // The approval is accepted only after the sibling snapshot already exists.
    let a = approved
        .execute(remove(), origin(2), vec![], Some(approval()))
        .unwrap()
        .accepted;
    assert_eq!(a, b, "SIBLING_SAME_ORIGINAL_COMMAND_KEY");
    assert_eq!(
        approved.history().undo()[0].commands()[0].payload_digest,
        b_digest,
        "SIBLING_SAME_ORIGINAL_PAYLOAD_DIGEST"
    );
    approved
        .install_snapshot(selected.validate_bounded(bounds).unwrap())
        .expect("SIBLING_FOLDED_SNAPSHOT_INSTALL_ALLOWED");
    let entry = approved.ledger().get(a).unwrap();
    println!(
        "SIBLING_FOLDED_DO key={a:?} trusted_fold_event=true selected_baseline_removed_track=true fate={:?} answer={:?}",
        entry.fate,
        entry.answer()
    );
    assert_eq!(
        entry.answer(),
        ApprovalAnswer::Uncertain,
        "SIBLING_FOLDED_DO_IS_UNCERTAIN"
    );
    super::roundtrip_tests::saved_open(&approved);
}

#[test]
fn future_epoch_absent_from_capture_is_certainly_missing() {
    let pool = HistoryPool::new(PoolLimits::default());
    let mut owner = make(&pool);
    let selected = owner.capture().unwrap();
    owner
        .install_snapshot(owner.capture().unwrap().validate().unwrap())
        .unwrap();
    let key = owner
        .execute(remove(), origin(2), vec![], Some(approval()))
        .unwrap()
        .accepted;
    owner
        .install_snapshot(selected.validate().unwrap())
        .unwrap();
    assert_eq!(
        owner.ledger().get(key).unwrap().answer(),
        ApprovalAnswer::Missing,
        "FUTURE_EPOCH_ABSENT_COMMAND_IS_MISSING"
    );
    super::roundtrip_tests::saved_open(&owner);
}
#[test]
fn same_epoch_after_frontier_and_counter_is_certainly_missing() {
    let pool = HistoryPool::new(PoolLimits::default());
    let mut owner = make(&pool);
    owner.execute(marker(1), origin(2), vec![], None).unwrap();
    owner.execute(marker(2), origin(3), vec![], None).unwrap();
    let selected = owner.capture().unwrap();
    let key = owner
        .execute(remove(), origin(4), vec![], Some(approval()))
        .unwrap()
        .accepted;
    owner
        .install_snapshot(selected.validate().unwrap())
        .unwrap();
    assert_eq!(
        owner.ledger().get(key).unwrap().answer(),
        ApprovalAnswer::Missing,
        "SAME_EPOCH_AFTER_FRONTIER_IS_MISSING"
    );
    super::roundtrip_tests::saved_open(&owner);
}
#[test]
fn older_epoch_without_snapshot_evidence_is_uncertain() {
    let pool = HistoryPool::new(PoolLimits::default());
    let mut owner = make(&pool);
    let mut sibling = make(&pool);
    // The sibling abandons epoch zero; the record deliberately cannot prove
    // that absence to a different line whose approval was not in its ledger.
    sibling
        .install_snapshot(sibling.capture().unwrap().validate().unwrap())
        .unwrap();
    let selected = sibling.capture().unwrap();
    let key = owner
        .execute(remove(), origin(2), vec![], Some(approval()))
        .unwrap()
        .accepted;
    owner
        .install_snapshot(selected.validate().unwrap())
        .unwrap();
    assert_eq!(
        owner.ledger().get(key).unwrap().answer(),
        ApprovalAnswer::Uncertain,
        "OLDER_EPOCH_WITHOUT_SNAPSHOT_EVIDENCE_IS_UNCERTAIN"
    );
    assert_eq!(
        owner
            .ledger()
            .entries()
            .filter(|entry| entry.answer() == ApprovalAnswer::Kept)
            .count(),
        0,
        "UNCERTAIN_DO_NOT_COUNTED_KEPT"
    );
    assert_eq!(
        owner
            .ledger()
            .entries()
            .filter(|entry| entry.answer() == ApprovalAnswer::Missing)
            .count(),
        0,
        "UNCERTAIN_DO_NOT_COUNTED_MISSING"
    );
    super::roundtrip_tests::saved_open(&owner);
}
