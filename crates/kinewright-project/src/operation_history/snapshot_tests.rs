use super::tests::{approval, document, origin, remove};
use super::*;
use kinewright_core::{Command, Operation, TrackId, undo::RetentionPolicy};
fn history(pool: &HistoryPool) -> OwnerHistory {
    OwnerHistory::new(
        pool,
        Role::Task,
        RetentionPolicy::NeverFold,
        &origin(1),
        || Ok(document()),
    )
    .unwrap()
}
#[test]
fn snapshot_restores_the_same_approved_command_after_discard() {
    let old = kinewright_core::Core::spawn(document()).unwrap();
    old.request(remove()).unwrap();
    let selected = old.export_history().unwrap();
    old.request(Command::Undo).unwrap();
    old.request(remove()).unwrap();
    old.restore_history(selected).unwrap();
    let pool = HistoryPool::new(PoolLimits::default());
    let mut owner = history(&pool);
    let a = owner
        .execute(remove(), origin(2), vec![], Some(approval()))
        .unwrap()
        .accepted;
    let snapshot = owner.capture().unwrap();
    owner
        .execute(Command::Undo, origin(3), vec![], None)
        .unwrap();
    let mut second = approval();
    second.reference.sha256 = "b".repeat(64);
    let b = owner
        .execute(remove(), origin(4), vec![], Some(second))
        .unwrap()
        .accepted;
    assert_eq!(owner.ledger().get(a).unwrap().fate, Fate::Discarded);
    let key = owner
        .install_snapshot(snapshot.validate().unwrap())
        .unwrap();
    assert_eq!(
        owner.ledger().get(a).unwrap().fate,
        Fate::Applied,
        "SNAPSHOT_REAPPLIES_SAME_COMMAND"
    );
    assert_eq!(
        owner.ledger().get(a).unwrap().answer(),
        ApprovalAnswer::Kept,
        "SNAPSHOT_SAME_COMMAND_AUDIT_AGREES"
    );
    assert_eq!(
        owner.ledger().get(a).unwrap().answer(),
        old_answer(&old),
        "SNAPSHOT_SAME_COMMAND_EQUALS_ACTUAL_OLD_AUDIT"
    );
    assert_eq!(
        owner.ledger().get(b).unwrap().fate,
        Fate::Discarded,
        "INSTALL_DISCARDS_OTHER_LINE_COMMAND"
    );
    assert_eq!(
        owner.ledger().get(a).unwrap().install_epoch,
        Some(key.epoch),
        "INSTALL_FATE_PROVENANCE"
    );
    assert_eq!(
        owner.history().undo()[0].commands()[0].id,
        a,
        "INSTALL_PRESERVES_APPROVAL_IDENTITY"
    );
    owner
        .execute(Command::Undo, origin(5), vec![], None)
        .unwrap();
    assert_eq!(owner.ledger().get(a).unwrap().fate, Fate::Undone);
    owner
        .execute(Command::Redo, origin(6), vec![], None)
        .unwrap();
    assert_eq!(owner.ledger().get(a).unwrap().fate, Fate::Redone);
}
#[test]
fn snapshot_restores_the_same_approved_command_to_redo() {
    let old = kinewright_core::Core::spawn(document()).unwrap();
    old.request(remove()).unwrap();
    old.request(Command::Undo).unwrap();
    let selected = old.export_history().unwrap();
    old.request(Command::Redo).unwrap();
    old.restore_history(selected).unwrap();
    let pool = HistoryPool::new(PoolLimits::default());
    let mut owner = history(&pool);
    let a = owner
        .execute(remove(), origin(2), vec![], Some(approval()))
        .unwrap()
        .accepted;
    owner
        .execute(Command::Undo, origin(3), vec![], None)
        .unwrap();
    let snapshot = owner.capture().unwrap();
    owner
        .execute(Command::Redo, origin(4), vec![], None)
        .unwrap();
    owner
        .install_snapshot(snapshot.validate().unwrap())
        .unwrap();
    assert_eq!(
        owner.ledger().get(a).unwrap().fate,
        Fate::Undone,
        "SNAPSHOT_RESTORES_APPROVED_UNDONE_FATE"
    );
    assert_eq!(
        owner.ledger().get(a).unwrap().answer(),
        ApprovalAnswer::Missing,
        "SNAPSHOT_UNDONE_AUDIT_AGREES"
    );
    assert_eq!(
        owner.ledger().get(a).unwrap().answer(),
        old_answer(&old),
        "SNAPSHOT_UNDONE_EQUALS_ACTUAL_OLD_AUDIT"
    );
    assert_eq!(owner.history().document().tracks.len(), 1);
}
#[test]
fn conflicting_payload_install_is_typed_and_atomic() {
    let pool = HistoryPool::new(PoolLimits::default());
    let mut owner = history(&pool);
    let mut other = history(&pool);
    owner
        .execute(remove(), origin(2), vec![], Some(approval()))
        .unwrap();
    other
        .execute(
            Command::DoBatch(vec![
                Operation::RemoveTrack { track: TrackId(81) },
                Operation::AddMarker {
                    marker: kinewright_core::Marker {
                        id: kinewright_core::MarkerId(91),
                        position: kinewright_core::TimeCode(0),
                        label: "other".into(),
                        color_token: 0,
                    },
                },
            ]),
            origin(2),
            vec![],
            Some(approval()),
        )
        .unwrap();
    let identity = owner.history().snapshot_identity().unwrap();
    let ledger = owner.ledger().clone();
    assert_eq!(
        owner.install_snapshot(other.capture().unwrap().validate().unwrap()),
        Err(Error::CommandConflict),
        "INSTALL_KEY_DIGEST_CONFLICT_REFUSED"
    );
    assert_eq!(
        owner.history().snapshot_identity().unwrap(),
        identity,
        "CONFLICT_INSTALL_DOCUMENT_ATOMIC"
    );
    assert_eq!(owner.ledger(), &ledger, "CONFLICT_INSTALL_LEDGER_ATOMIC");
}
#[test]
fn malformed_owner_capture_refusals_leave_destination_unchanged() {
    let pool = HistoryPool::new(PoolLimits::default());
    let mut owner = history(&pool);
    owner
        .execute(remove(), origin(2), vec![], Some(approval()))
        .unwrap();
    let identity = owner.history().snapshot_identity().unwrap();
    for redo in [false, true] {
        let mut capture = owner.capture().unwrap();
        let empty = kinewright_core::undo::Step::unvalidated(vec![], None);
        if redo {
            capture.data.history.redo.push(empty);
        } else {
            capture.data.history.undo[0] = empty;
        }
        assert!(
            matches!(
                capture.validate(),
                Err(Error::History(
                    kinewright_core::undo::Error::InvalidMetadata
                ))
            ),
            "MALFORMED_EMPTY_OWNER_STEP_REFUSED"
        );
    }
    let mut capture = owner.capture().unwrap();
    capture
        .data
        .ledger
        .commands
        .values_mut()
        .next()
        .unwrap()
        .approval
        .targets = vec![RemovedTarget::Track(TrackId(999))];
    assert!(
        matches!(capture.validate(), Err(Error::InvalidLedger)),
        "OWNER_APPROVAL_TARGET_EFFECT_REFUSED"
    );
    let mut capture = owner.capture().unwrap();
    capture.data.origins[0].payload_digest[0] ^= 1;
    assert!(
        matches!(capture.validate(), Err(Error::InvalidOrigin)),
        "OWNER_ORIGIN_DIGEST_REFUSED"
    );
    let mut capture = owner.capture().unwrap();
    let entry = capture.data.ledger.commands.values_mut().next().unwrap();
    entry.fate = Fate::Undone;
    assert!(
        matches!(capture.validate(), Err(Error::InvalidLedger)),
        "OWNER_WRONG_STACK_FATE_REFUSED"
    );
    let mut capture = owner.capture().unwrap();
    capture.data.history.position = 10;
    let mut entry = capture
        .data
        .ledger
        .commands
        .values()
        .next()
        .unwrap()
        .clone();
    entry.key.position = 8;
    capture.data.ledger.commands.clear();
    capture.data.ledger.commands.insert(entry.key, entry);
    assert!(
        matches!(capture.validate(), Err(Error::InvalidLedger)),
        "OWNER_ABSENT_APPROVAL_COMMAND_REFUSED"
    );
    assert_eq!(
        owner.history().snapshot_identity().unwrap(),
        identity,
        "MALFORMED_OWNER_DESTINATION_UNCHANGED"
    );
}

#[test]
fn origin_spare_capacity_is_charged_before_commit() {
    let mut value = String::with_capacity(1 << 20);
    value.push_str("00000000000000000000000000000002");
    let branch = serde_json::from_value(serde_json::Value::String(value)).unwrap();
    let creation = CreationOrigin {
        branch,
        batch: origin(2).batch,
    };
    assert!(
        creation.branch.allocated_capacity() >= 1 << 20,
        "RESERVED_ORIGIN_FIXTURE"
    );
    let pool = HistoryPool::new(PoolLimits::default());
    let mut owner = OwnerHistory::new(
        &pool,
        Role::Candidate,
        RetentionPolicy::NeverFold,
        &origin(1),
        || Ok(document()),
    )
    .unwrap();
    let identity = owner.history().snapshot_identity().unwrap();
    assert_eq!(
        owner.execute(
            Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(81),
                locked: true
            }),
            creation,
            vec![],
            None
        ),
        Err(Error::Capacity("owner metadata")),
        "RESERVED_ORIGIN_CAPACITY_REFUSED"
    );
    assert_eq!(
        owner.history().snapshot_identity().unwrap(),
        identity,
        "ORIGIN_REFUSAL_ATOMIC"
    );
}

fn old_answer(old: &kinewright_core::Core) -> ApprovalAnswer {
    let Command::DoBatch(operations) = remove() else {
        unreachable!()
    };
    crate::edit_audit::owner_differential_proof(
        &old.export_history().unwrap(),
        Some(0),
        Some(&operations),
        &approval().targets,
        true,
    )
}

#[test]
fn owner_role_policy_and_fold_frontier_follow_accepted_position_order() {
    let pool = HistoryPool::new(PoolLimits::default());
    let mut called = false;
    assert!(
        matches!(
            OwnerHistory::new(&pool, Role::Task, RetentionPolicy::Fold, &origin(1), || {
                called = true;
                Ok(document())
            }),
            Err(Error::InvalidPolicy)
        ),
        "TASK_POLICY_CANNOT_FOLD_THE_PINNED_DELTA"
    );
    assert!(!called, "INVALID_POLICY_PRECEDES_DOCUMENT_ALLOCATION");
    let mut owner = history(&pool);
    owner
        .execute(
            Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(81),
                locked: true,
            }),
            origin(2),
            vec![],
            None,
        )
        .unwrap();
    let mut capture = owner.capture().unwrap();
    capture.data.history.policy = RetentionPolicy::Fold;
    assert!(
        matches!(capture.validate(), Err(Error::InvalidPolicy)),
        "OPEN_REFUSES_ROLE_POLICY_MISMATCH"
    );
    let mut capture = owner.capture().unwrap();
    let key = kinewright_core::undo::CommandKey {
        epoch: 1,
        position: 2,
    };
    let mut command = capture.data.history.undo[0].commands()[0].clone();
    command.id = key;
    command.position = 2;
    capture.data.history.undo[0] = kinewright_core::undo::Step::unvalidated(vec![command], None);
    capture.data.origins[0].key = key;
    capture.data.history.epoch = 2;
    capture.data.history.position = 10;
    capture.data.fold_frontier = Some(kinewright_core::undo::CommandKey {
        epoch: 0,
        position: 3,
    });
    assert!(
        matches!(capture.validate(), Err(Error::InvalidFrontier)),
        "FOLD_FRONTIER_POSITION_PRECEDES_RETAINED_COMMANDS"
    );
    let target = owner.transition_target(Direction::Undo).unwrap();
    owner
        .approve_transition(approval().reference, Direction::Undo, target)
        .unwrap();
    let mut capture = owner.capture().unwrap();
    capture.data.ledger.transitions[0].target.commands = 4;
    assert!(
        matches!(capture.validate(), Err(Error::InvalidLedger)),
        "TRANSITION_EVIDENCE_CANNOT_EXCEED_ACCEPTED_POSITION"
    );
}
