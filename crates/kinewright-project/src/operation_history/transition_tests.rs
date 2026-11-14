use super::tests::{document, origin};
use super::*;
use kinewright_core::{
    Command, Document, Marker, MarkerId, Operation, TimeCode, Track, TrackId, TrackKind,
    undo::{History, RetentionPolicy, RoleBounds},
};
fn id(value: u64) -> crate::bundle_records::ObjectRef {
    crate::bundle_records::ObjectRef {
        sha256: format!("{value:064x}"),
        byte_length: 1,
        record_kind: crate::bundle_records::RecordKind::OwnedEvidence,
    }
}
fn owner(pool: &HistoryPool, fold: bool, empty: bool) -> OwnerHistory {
    let role = if fold { Role::Ordinary } else { Role::Task };
    let policy = if fold {
        RetentionPolicy::Fold
    } else {
        RetentionPolicy::NeverFold
    };
    let doc = if empty {
        Document::default()
    } else {
        document()
    };
    // The independent factory value makes the tiny folding profile exercise
    // the same owner transaction without waiting for thousands of steps.
    let mut owner = OwnerHistory::new(pool, role, policy, &origin(1), || Ok(doc)).unwrap();
    if fold {
        owner.core = History::new(
            if empty {
                Document::default()
            } else {
                document()
            },
            policy,
            RoleBounds {
                step_cap: 1,
                ..role.bounds()
            },
        )
        .unwrap();
    }
    owner
}
fn add() -> Command {
    Command::Do(Operation::AddTrack {
        track: Track {
            id: TrackId(81),
            kind: TrackKind::Video,
            sync_lock: false,
            clips: vec![],
        },
    })
}
fn marker(value: u64) -> Operation {
    Operation::AddMarker {
        marker: Marker {
            id: MarkerId(value),
            position: TimeCode(0),
            label: "marker".into(),
            color_token: 0,
        },
    }
}
fn approved(owner: &mut OwnerHistory, direction: Direction, value: u64) {
    let target = owner.transition_target(direction).unwrap();
    owner
        .approve_transition(id(value), direction, target)
        .unwrap();
}
#[test]
fn approved_destructive_undo_then_install_of_its_snapshot_holds() {
    let pool = HistoryPool::new(PoolLimits::default());
    let mut history = owner(&pool, false, true);
    history.execute(add(), origin(2), vec![], None).unwrap();
    let target = history.transition_target(Direction::Undo).unwrap();
    approved(&mut history, Direction::Undo, 1);
    let snapshot = history.capture().unwrap();
    history
        .execute(Command::Redo, origin(3), vec![], None)
        .unwrap();
    assert_eq!(
        history.transition_fate(&id(1)).unwrap(),
        Some(TransitionFate::Reversed)
    );
    history
        .install_snapshot(snapshot.validate().unwrap())
        .unwrap();
    assert_eq!(
        history.transition_fate(&id(1)).unwrap(),
        Some(TransitionFate::Holds),
        "APPROVED_UNDO_INSTALL_HOLDS"
    );
    assert_eq!(
        history.history().redo()[0].evidence(),
        Some(target),
        "CONTROL_APPROVAL_ADDS_NO_CORE_COMMAND"
    );
    let entry = history.ledger().transition_entries().next().unwrap();
    let value = serde_json::to_value(entry).unwrap();
    assert_eq!(
        value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["direction", "id", "target"],
        "TRANSITION_RECORD_HAS_NO_FATE_FLAG"
    );
}
#[test]
fn approved_undo_then_redo_is_reversed() {
    let pool = HistoryPool::new(PoolLimits::default());
    let mut history = owner(&pool, false, true);
    history.execute(add(), origin(2), vec![], None).unwrap();
    approved(&mut history, Direction::Undo, 1);
    history
        .execute(Command::Redo, origin(3), vec![], None)
        .unwrap();
    assert_eq!(
        history.transition_fate(&id(1)).unwrap(),
        Some(TransitionFate::Reversed),
        "APPROVED_UNDO_REDO_REVERSED"
    );
}
#[test]
fn approved_redo_then_undo_then_new_do_is_discarded() {
    let pool = HistoryPool::new(PoolLimits::default());
    let mut history = owner(&pool, false, false);
    history
        .execute(
            Command::Do(Operation::RemoveTrack { track: TrackId(81) }),
            origin(2),
            vec![],
            None,
        )
        .unwrap();
    history
        .execute(Command::Undo, origin(3), vec![], None)
        .unwrap();
    approved(&mut history, Direction::Redo, 1);
    history
        .execute(Command::Undo, origin(4), vec![], None)
        .unwrap();
    history
        .execute(
            Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(81),
                locked: true,
            }),
            origin(5),
            vec![],
            None,
        )
        .unwrap();
    assert_eq!(
        history.transition_fate(&id(1)).unwrap(),
        Some(TransitionFate::Discarded),
        "APPROVED_REDO_DROP_DISCARDED"
    );
    let captured = history.capture().unwrap();
    let opened = captured.validate().unwrap().open().unwrap();
    assert_eq!(
        opened.transition_fate(&id(1)).unwrap(),
        Some(TransitionFate::Discarded),
        "APPROVED_REDO_DROP_OPEN_DISCARDED"
    );
}
#[test]
fn approved_undo_of_a_dropped_step_holds_even_behind_frontier() {
    let pool = HistoryPool::new(PoolLimits::default());
    let mut history = owner(&pool, true, true);
    history.execute(add(), origin(2), vec![], None).unwrap();
    approved(&mut history, Direction::Undo, 1);
    for n in 1..=2 {
        history
            .execute(Command::Do(marker(n)), origin(n + 2), vec![], None)
            .unwrap();
    }
    assert_eq!(
        history.transition_fate(&id(1)).unwrap(),
        Some(TransitionFate::Holds),
        "APPROVED_UNDO_DROP_HOLDS"
    );
    assert!(history.fold_frontier.is_some());
    let opened = history
        .capture()
        .unwrap()
        .validate()
        .unwrap()
        .open()
        .unwrap();
    assert_eq!(
        opened.transition_fate(&id(1)).unwrap(),
        Some(TransitionFate::Holds),
        "KNOWN_DISCARD_OVERRIDES_FOLD_FRONTIER"
    );
}
#[test]
fn redo_closes_the_approved_gesture_before_the_next_edit() {
    let pool = HistoryPool::new(PoolLimits::default());
    let mut history = owner(&pool, false, false);
    for (n, operation) in [Operation::RemoveTrack { track: TrackId(81) }, marker(1)]
        .into_iter()
        .enumerate()
    {
        history
            .execute(
                Command::DoBatchCoalesced {
                    operations: vec![operation],
                    coalesce_key: "gesture".into(),
                },
                origin(n as u64 + 2),
                vec![],
                None,
            )
            .unwrap();
    }
    history
        .execute(Command::Undo, origin(4), vec![], None)
        .unwrap();
    approved(&mut history, Direction::Redo, 1);
    assert!(
        history
            .history()
            .undo()
            .back()
            .unwrap()
            .coalesce_key()
            .is_none(),
        "REDO_CLOSES_APPROVED_GESTURE"
    );
    history
        .execute(
            Command::DoBatchCoalesced {
                operations: vec![marker(2)],
                coalesce_key: "gesture".into(),
            },
            origin(5),
            vec![],
            None,
        )
        .unwrap();
    assert_eq!(
        history.history().undo().len(),
        2,
        "REDO_TARGET_CANNOT_EXTEND_BY_EDIT"
    );
    assert_eq!(
        history.transition_fate(&id(1)).unwrap(),
        Some(TransitionFate::Holds)
    );
}
#[test]
fn installed_exact_prefix_holds_and_an_incompatible_prefix_is_uncertain() {
    let pool = HistoryPool::new(PoolLimits::default());
    let mut history = owner(&pool, false, false);
    for (n, operation) in [Operation::RemoveTrack { track: TrackId(81) }, marker(1)]
        .into_iter()
        .enumerate()
    {
        history
            .execute(
                Command::DoBatchCoalesced {
                    operations: vec![operation],
                    coalesce_key: "gesture".into(),
                },
                origin(n as u64 + 2),
                vec![],
                None,
            )
            .unwrap();
    }
    let mut fork = history
        .capture()
        .unwrap()
        .validate()
        .unwrap()
        .open()
        .unwrap();
    history
        .execute(Command::Undo, origin(4), vec![], None)
        .unwrap();
    approved(&mut history, Direction::Redo, 1);
    fork.execute(
        Command::DoBatchCoalesced {
            operations: vec![marker(2)],
            coalesce_key: "gesture".into(),
        },
        origin(5),
        vec![],
        None,
    )
    .unwrap();
    history
        .install_snapshot(fork.capture().unwrap().validate().unwrap())
        .unwrap();
    assert_eq!(
        history.transition_fate(&id(1)).unwrap(),
        Some(TransitionFate::Holds),
        "INSTALLED_APPROVED_EXACT_PREFIX_HOLDS"
    );
    let mut altered = fork.capture().unwrap();
    // An independent wire record with the middle command removed. The first
    // key/digest still matches, but the approved two-command prefix does not.
    let mut wire = serde_json::to_value(&altered.data.history).unwrap();
    wire["undo"][0]["commands"]
        .as_array_mut()
        .unwrap()
        .remove(1);
    altered.data.history = serde_json::from_value(wire).unwrap();
    altered.data.origins.remove(1);
    let mut expected = (*altered.data.history.baseline).clone();
    for operation in altered
        .data
        .history
        .undo
        .iter()
        .flat_map(kinewright_core::undo::Step::operations)
    {
        operation.apply(&mut expected).unwrap();
    }
    altered.data.history.document = std::sync::Arc::new(expected);
    history
        .install_snapshot(altered.validate().unwrap())
        .unwrap();
    assert_eq!(
        history.transition_fate(&id(1)).unwrap(),
        Some(TransitionFate::Uncertain(Uncertainty::ShapeMismatch)),
        "INSTALLED_NONPREFIX_IS_UNCERTAIN"
    );
    assert_eq!(
        history.transition_counts().unwrap(),
        TransitionCounts {
            uncertain: 1,
            ..TransitionCounts::default()
        },
        "SHAPE_UNCERTAIN_NEVER_COUNTS_AS_HOLDS"
    );
}
#[test]
fn folded_snapshot_before_the_transition_approval_reports_only_uncertain() {
    let pool = HistoryPool::new(PoolLimits::default());
    let mut history = owner(&pool, true, true);
    history.execute(add(), origin(2), vec![], None).unwrap();
    let before_fold = history.capture().unwrap();
    history
        .execute(
            Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(81),
                locked: true,
            }),
            origin(3),
            vec![],
            None,
        )
        .unwrap();
    let folded = history.capture().unwrap();
    history
        .install_snapshot(before_fold.validate().unwrap())
        .unwrap();
    approved(&mut history, Direction::Undo, 1);
    history
        .install_snapshot(folded.validate().unwrap())
        .unwrap();
    let fate = history.transition_fate(&id(1)).unwrap().unwrap();
    assert_eq!(
        fate,
        TransitionFate::Uncertain(Uncertainty::MissingFoldProvenance),
        "PREAPPROVAL_FOLDED_SNAPSHOT_IS_UNCERTAIN"
    );
    assert_eq!(
        fate.answer(),
        ApprovalAnswer::Uncertain,
        "UNCERTAIN_NEVER_PRESENTED_AS_KEPT_OR_MISSING"
    );
    assert_eq!(
        history.transition_counts().unwrap(),
        TransitionCounts {
            uncertain: 1,
            ..TransitionCounts::default()
        },
        "UNCERTAIN_NEVER_COUNTS_AS_HOLDS_REVERSED_OR_DISCARDED"
    );
}
#[test]
fn known_folded_transition_target_keeps_its_exact_answer() {
    let pool = HistoryPool::new(PoolLimits::default());
    let mut history = owner(&pool, true, true);
    history.execute(add(), origin(2), vec![], None).unwrap();
    approved(&mut history, Direction::Undo, 1);
    history
        .execute(Command::Redo, origin(3), vec![], None)
        .unwrap();
    history
        .execute(Command::Do(marker(1)), origin(4), vec![], None)
        .unwrap();
    assert_eq!(
        history.transition_fate(&id(1)).unwrap(),
        Some(TransitionFate::Reversed),
        "KNOWN_FOLD_REVERSES_APPROVED_UNDO"
    );
    let opened = history
        .capture()
        .unwrap()
        .validate()
        .unwrap()
        .open()
        .unwrap();
    assert_eq!(
        opened.transition_fate(&id(1)).unwrap(),
        Some(TransitionFate::Reversed),
        "KNOWN_FOLD_FACT_SURVIVES_OPEN"
    );
}

#[test]
fn transition_entries_share_the_256_approval_cap_and_have_fixed_evidence() {
    let pool = HistoryPool::new(PoolLimits::default());
    let mut history = owner(&pool, false, true);
    history.execute(add(), origin(2), vec![], None).unwrap();
    for value in 1..=256 {
        approved(
            &mut history,
            if value % 2 == 1 {
                Direction::Undo
            } else {
                Direction::Redo
            },
            value,
        );
    }
    assert_eq!(history.ledger().len(), 256, "COMBINED_APPROVAL_LIMIT");
    assert!(
        std::mem::size_of::<TransitionApproval>() <= 160,
        "FIXED_TRANSITION_ENTRY_LAYOUT"
    );
    assert!(
        history.owner_bytes() <= 300 << 10,
        "TRANSITION_256_LIVE_BYTES"
    );
    assert_eq!(
        history.transition_counts().unwrap(),
        TransitionCounts {
            holds: 128,
            reversed: 128,
            discarded: 0,
            uncertain: 0
        },
        "TRANSITION_COUNTS_DERIVE_FROM_STACKS"
    );
    let before = history.history().snapshot_identity().unwrap();
    let target = history.transition_target(Direction::Undo).unwrap();
    assert_eq!(
        history.approve_transition(id(257), Direction::Undo, target),
        Err(Error::Capacity("approval count")),
        "TRANSITION_LIMIT_REFUSAL"
    );
    assert_eq!(
        history.history().snapshot_identity().unwrap(),
        before,
        "TRANSITION_LIMIT_REFUSAL_ATOMIC"
    );
    let bounds = history.history().bounds();
    let opened = history
        .capture()
        .unwrap()
        .validate_bounded(bounds)
        .unwrap()
        .open()
        .unwrap();
    assert_eq!(
        opened.transition_counts().unwrap(),
        history.transition_counts().unwrap(),
        "TRANSITION_CAP_RECORD_ROUNDTRIP"
    );
}

#[test]
fn target_digest_conflict_is_refused_even_after_the_step_was_dropped() {
    let pool = HistoryPool::new(PoolLimits::default());
    let mut history = owner(&pool, false, false);
    history
        .execute(
            Command::Do(Operation::RemoveTrack { track: TrackId(81) }),
            origin(2),
            vec![],
            None,
        )
        .unwrap();
    history
        .execute(Command::Undo, origin(3), vec![], None)
        .unwrap();
    approved(&mut history, Direction::Redo, 1);
    history
        .execute(Command::Undo, origin(4), vec![], None)
        .unwrap();
    history
        .execute(
            Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(81),
                locked: true,
            }),
            origin(5),
            vec![],
            None,
        )
        .unwrap();
    let mut other = owner(&pool, false, false);
    other
        .execute(
            Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(81),
                locked: false,
            }),
            origin(2),
            vec![],
            None,
        )
        .unwrap();
    let before = history.history().snapshot_identity().unwrap();
    assert_eq!(
        history.install_snapshot(other.capture().unwrap().validate().unwrap()),
        Err(Error::CommandConflict),
        "TRANSITION_TARGET_DIGEST_CONFLICT_REFUSED"
    );
    assert_eq!(
        history.history().snapshot_identity().unwrap(),
        before,
        "TRANSITION_DIGEST_CONFLICT_ATOMIC"
    );
}
#[test]
fn malformed_fold_frontier_is_refused_before_install() {
    let pool = HistoryPool::new(PoolLimits::default());
    let mut history = owner(&pool, false, true);
    history.execute(add(), origin(2), vec![], None).unwrap();
    let before = history.history().snapshot_identity().unwrap();
    let mut record = history.capture().unwrap();
    record.data.fold_frontier = Some(kinewright_core::undo::CommandKey {
        epoch: 0,
        position: 100,
    });
    assert!(
        matches!(record.validate(), Err(Error::InvalidFrontier)),
        "MALFORMED_FOLD_FRONTIER_REFUSED"
    );
    assert_eq!(
        history.history().snapshot_identity().unwrap(),
        before,
        "FRONTIER_REFUSAL_ATOMIC"
    );
}

#[test]
fn transition_evidence_digest_has_literal_ordered_identity_bytes() {
    use kinewright_core::undo::{CommandKey, StepEvidence};
    let first = CommandKey {
        epoch: 2,
        position: 7,
    };
    let second = CommandKey {
        epoch: 3,
        position: 9,
    };
    let evidence =
        StepEvidence::from_commands([(first, [17; 32]), (second, [34; 32])].into_iter()).unwrap();
    assert_eq!(
        evidence.first_key, first,
        "EVIDENCE_PRESERVES_ORIGINAL_FIRST_KEY"
    );
    assert_eq!(evidence.first_digest, [17; 32]);
    assert_eq!(evidence.commands, 2);
    assert_eq!(
        evidence.digest,
        [
            40, 33, 108, 48, 236, 34, 199, 98, 106, 231, 158, 145, 240, 232, 129, 236, 180, 183,
            124, 73, 255, 194, 23, 240, 226, 248, 213, 202, 236, 44, 126, 119
        ],
        "ORDERED_STEP_DIGEST_LITERAL"
    );
}

#[test]
fn preapproval_folded_do_fact_settles_a_later_redo_approval() {
    let pool = HistoryPool::new(PoolLimits::default());
    let mut history = owner(&pool, true, false);
    let bounds = history.history().bounds();
    history
        .execute(
            super::tests::remove(),
            origin(2),
            vec![],
            Some(super::tests::approval()),
        )
        .unwrap();
    history
        .execute(Command::Undo, origin(3), vec![], None)
        .unwrap();
    let t = history.capture().unwrap();
    history
        .execute(Command::Redo, origin(4), vec![], None)
        .unwrap();
    history
        .execute(Command::Do(marker(9)), origin(5), vec![], None)
        .unwrap();
    let s = history.capture().unwrap();
    assert!(
        s.data.terminal_facts.is_empty(),
        "S_PREDATES_THE_TRANSITION_APPROVAL"
    );
    assert_eq!(
        s.data.folded_approvals.len(),
        1,
        "S_CARRIES_MATCHING_DO_FOLD_EVIDENCE"
    );
    history
        .install_snapshot(t.validate_bounded(bounds).unwrap())
        .unwrap();
    approved(&mut history, Direction::Redo, 99);
    history
        .install_snapshot(s.validate_bounded(bounds).unwrap())
        .unwrap();
    assert_eq!(
        history.transition_fate(&id(99)).unwrap(),
        Some(TransitionFate::Holds),
        "APPROVED_REDO_USES_INSTALLED_DO_FOLD_FACT"
    );
    assert_eq!(
        history.transition_counts().unwrap().uncertain,
        0,
        "MATCHING_FOLD_EVIDENCE_IS_EXACT"
    );
}

#[test]
fn folded_do_fact_does_not_turn_a_nonprefix_transition_into_holds() {
    let pool = HistoryPool::new(PoolLimits::default());
    let mut doc = document();
    doc.tracks.push(Track {
        id: TrackId(82),
        kind: TrackKind::Video,
        sync_lock: false,
        clips: vec![],
    });
    let bounds = RoleBounds {
        step_cap: 1,
        ..Role::Ordinary.bounds()
    };
    let mut source = OwnerHistory::measured(
        &pool,
        Role::Ordinary,
        RetentionPolicy::Fold,
        &origin(1),
        bounds,
        || Ok(doc.clone()),
    )
    .unwrap();
    let mut dest = OwnerHistory::measured(
        &pool,
        Role::Ordinary,
        RetentionPolicy::Fold,
        &origin(1),
        bounds,
        || Ok(doc),
    )
    .unwrap();
    for (index, track) in [TrackId(82), TrackId(81)].into_iter().enumerate() {
        let ops = vec![Operation::RemoveTrack { track }];
        let approval = (index == 1).then(super::tests::approval);
        source
            .execute(
                Command::DoBatchCoalesced {
                    operations: ops.clone(),
                    coalesce_key: "cut".into(),
                },
                origin(index as u64 + 2),
                vec![],
                approval.clone(),
            )
            .unwrap();
        dest.execute(
            Command::DoBatch(ops),
            origin(index as u64 + 2),
            vec![],
            approval,
        )
        .unwrap();
    }
    source
        .execute(Command::Do(marker(50)), origin(4), vec![], None)
        .unwrap();
    let s = source.capture().unwrap();
    dest.execute(Command::Undo, origin(4), vec![], None)
        .unwrap();
    approved(&mut dest, Direction::Redo, 99);
    dest.install_snapshot(s.validate_bounded(bounds).unwrap())
        .unwrap();
    assert_eq!(
        dest.transition_fate(&id(99)).unwrap(),
        Some(TransitionFate::Uncertain(Uncertainty::ShapeMismatch)),
        "FOLDED_COMMAND_FACT_MUST_PROVE_THE_APPROVED_PREFIX"
    );
    assert_eq!(
        dest.transition_counts().unwrap().holds,
        0,
        "NONPREFIX_FOLD_NEVER_COUNTS_AS_HOLDS"
    );
}
