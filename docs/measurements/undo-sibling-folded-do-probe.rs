#[test]
fn probe_sibling_folded_do_with_no_approval_fact() {
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
    assert_ne!(
        entry.fate,
        Fate::Discarded,
        "SIBLING_FOLDED_APPROVED_COMMAND_CANNOT_BE_CALLED_DISCARDED"
    );
}
