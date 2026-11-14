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

#[test]
fn selected_fold_fact_with_other_digest_discards_only_the_destination_approval() {
    let pool = HistoryPool::new(PoolLimits::default());
    let mut destination = make(&pool);
    let mut sibling = make(&pool);
    let a = approval();
    let mut b = approval();
    b.reference.sha256 = "b".repeat(64);
    sibling
        .execute(
            Command::DoBatch(vec![
                Operation::RemoveTrack {
                    track: kinewright_core::TrackId(81),
                },
                Operation::AddMarker {
                    marker: Marker {
                        id: MarkerId(99),
                        position: TimeCode(0),
                        label: "different payload".into(),
                        color_token: 0,
                    },
                },
            ]),
            origin(2),
            vec![],
            Some(b.clone()),
        )
        .unwrap();
    sibling.execute(marker(1), origin(3), vec![], None).unwrap();
    let selected = sibling.capture().unwrap();
    destination
        .execute(remove(), origin(2), vec![], Some(a.clone()))
        .unwrap();
    destination
        .install_snapshot(selected.validate().unwrap())
        .expect("DIFFERENT_FOLDED_DIGEST_IS_MISSING_NOT_AN_INSTALL_REFUSAL");
    for (reference, expected) in [
        (&a.reference, ApprovalAnswer::Missing),
        (&b.reference, ApprovalAnswer::Kept),
    ] {
        let entry = destination
            .ledger()
            .entries()
            .find(|entry| &entry.approval.reference == reference)
            .unwrap();
        assert_eq!(
            entry.answer(),
            expected,
            "FOLD_FACT_DIGEST_MATCH_DECIDES_EACH_APPROVAL"
        );
    }
    let key = destination.ledger().entries().next().unwrap().key;
    assert!(
        destination.ledger().get(key).is_none(),
        "AMBIGUOUS_KEY_NEVER_PICKS_AN_APPROVAL"
    );
    for entry in destination.ledger().entries() {
        assert_eq!(
            destination.ledger().get_exact(key, entry.payload_digest),
            Some(entry),
            "AUDIT_LOOKUP_USES_EXACT_IDENTITY"
        );
    }
    assert_eq!(
        destination.ledger().len(),
        2,
        "BOTH_APPROVAL_IDENTITIES_SURVIVE_INSTALL"
    );
    super::roundtrip_tests::saved_open(&destination);
}

#[test]
fn selected_matching_fold_fact_is_certainly_kept() {
    let pool = HistoryPool::new(PoolLimits::default());
    let mut destination = make(&pool);
    let mut sibling = make(&pool);
    sibling
        .execute(remove(), origin(2), vec![], Some(approval()))
        .unwrap();
    sibling.execute(marker(1), origin(3), vec![], None).unwrap();
    let selected = sibling.capture().unwrap();
    let key = destination
        .execute(remove(), origin(2), vec![], Some(approval()))
        .unwrap()
        .accepted;
    destination
        .install_snapshot(selected.validate().unwrap())
        .unwrap();
    assert_eq!(
        destination.ledger().get(key).unwrap().fate,
        Fate::Folded,
        "MATCHING_SELECTED_FOLD_FACT_IS_KEPT"
    );
    super::roundtrip_tests::saved_open(&destination);
}

#[test]
fn folded_sibling_payload_collision_preserves_exact_transition_evidence_in_both_digest_orders() {
    for swapped in [false, true] {
        let pool = HistoryPool::new(PoolLimits::default());
        let mut destination = make(&pool);
        let mut sibling = make(&pool);
        let payload = |extra: bool| {
            let mut ops = vec![Operation::RemoveTrack {
                track: kinewright_core::TrackId(81),
            }];
            if extra {
                ops.push(Operation::AddMarker {
                    marker: Marker {
                        id: MarkerId(99),
                        position: TimeCode(0),
                        label: "other payload".into(),
                        color_token: 0,
                    },
                });
            }
            Command::DoBatch(ops)
        };
        let mut b = approval();
        b.reference.sha256 = "b".repeat(64);
        sibling
            .execute(payload(swapped), origin(2), vec![], Some(b))
            .unwrap();
        let subject = sibling.transition_target(Direction::Undo).unwrap();
        let mut transition = approval().reference;
        transition.sha256 = "c".repeat(64);
        sibling
            .approve_transition(transition.clone(), Direction::Undo, subject)
            .unwrap();
        sibling
            .execute(Command::Redo, origin(3), vec![], None)
            .unwrap();
        sibling.execute(marker(1), origin(4), vec![], None).unwrap();
        let selected = sibling.capture().unwrap();
        destination
            .execute(payload(!swapped), origin(2), vec![], Some(approval()))
            .unwrap();
        destination
            .install_snapshot(selected.validate().unwrap())
            .unwrap();
        assert_eq!(
            destination.transition_fate(&transition).unwrap(),
            Some(TransitionFate::Reversed),
            "HISTORIC_DO_ALIAS_CANNOT_CONFLICT_WITH_EXACT_TRANSITION"
        );
        destination
            .execute(Command::Undo, origin(5), vec![], None)
            .unwrap();
        destination
            .execute(Command::Redo, origin(6), vec![], None)
            .unwrap();
        assert_eq!(
            destination.transition_fate(&transition).unwrap(),
            Some(TransitionFate::Reversed),
            "UNRELATED_MOVEMENT_DOES_NOT_CHANGE_FOLDED_SIBLING_ANSWERS"
        );
        super::roundtrip_tests::saved_open(&destination);
    }
}

#[test]
fn fixed_do_charge_covers_the_digest_index_and_fold_fact() {
    use std::mem::size_of;
    let a = approval();
    let inline = 3
        * (size_of::<(kinewright_core::undo::CommandKey, [u8; 32])>()
            + size_of::<ApprovalEntry>()
            + size_of::<kinewright_core::undo::CommandKey>()
            + size_of::<super::ledger::FoldedApproval>())
        + 64;
    let heap = a.reference.sha256.capacity() + a.targets.capacity() * size_of::<RemovedTarget>();
    assert!(
        super::owner::approval_charge(&a).unwrap() >= (inline + heap) as u64,
        "DO_APPROVAL_CHARGE_COVERS_BOTH_INDEX_NODES"
    );
}
