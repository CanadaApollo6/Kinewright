use super::tests::{document, origin};
use super::*;
use crate::bundle_records::{ObjectRef, RecordKind};
use kinewright_core::{
    Command, Marker, MarkerId, Operation, TimeCode, TrackId,
    undo::{RetentionPolicy, RoleBounds},
};

pub(super) fn consent_bytes(line: u64, transition: bool) -> Vec<u8> {
    format!("sibling consent {line} {transition}").into_bytes()
}
pub(super) fn consent(line: u64, transition: bool) -> ObjectRef {
    let bytes = consent_bytes(line, transition);
    ObjectRef {
        sha256: kinewright_media::sha256_bytes(&bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::OwnedEvidence,
    }
}
pub(super) fn bounds() -> RoleBounds {
    RoleBounds {
        step_cap: 1,
        ..Role::Ordinary.bounds()
    }
}
pub(super) fn make(pool: &HistoryPool) -> OwnerHistory {
    OwnerHistory::measured(
        pool,
        Role::Ordinary,
        RetentionPolicy::Fold,
        &origin(1),
        bounds(),
        || Ok(document()),
    )
    .unwrap()
}
fn marker(id: u64) -> Operation {
    Operation::AddMarker {
        marker: Marker {
            id: MarkerId(id),
            position: TimeCode(0),
            label: format!("marker {id}"),
            color_token: 0,
        },
    }
}
pub(super) fn sibling_captures(pool: &HistoryPool) -> [Vec<u8>; 2] {
    [0, 1].map(|line| {
        let mut owner = make(pool);
        owner
            .execute(
                Command::DoBatch(vec![
                    Operation::RemoveTrack { track: TrackId(81) },
                    marker(10 + line),
                ]),
                origin(2),
                vec![],
                Some(Approval {
                    reference: consent(line, false),
                    targets: vec![RemovedTarget::Track(TrackId(81))],
                }),
            )
            .unwrap();
        owner
            .approve_transition(
                consent(line, true),
                Direction::Undo,
                owner.transition_target(Direction::Undo).unwrap(),
            )
            .unwrap();
        owner
            .execute(Command::Redo, origin(3), vec![], None)
            .unwrap();
        owner
            .execute(Command::Do(marker(20)), origin(4), vec![], None)
            .unwrap();
        serde_json::to_vec(&owner.capture().unwrap().data).unwrap()
    })
}
pub(super) fn install_line(
    owner: &mut OwnerHistory,
    pool: &HistoryPool,
    bytes: &[u8],
    selected: u64,
) {
    let data: super::snapshot::SnapshotData = serde_json::from_slice(bytes).unwrap();
    let snapshot = OwnerSnapshot {
        reservation: pool.admit_sized(data.sizing).unwrap(),
        data,
    };
    owner
        .install_snapshot(snapshot.validate_bounded(bounds()).unwrap())
        .expect("FOLDED_SIBLING_TRANSITIONS_CANNOT_VETO_INSTALL");
    for entry in owner.ledger().entries() {
        let active = entry.approval.reference == consent(selected, false);
        assert_eq!(
            entry.fate,
            if active {
                Fate::Folded
            } else {
                Fate::Discarded
            },
            "SIBLING_DO_FATE_USES_SELECTED_DIGEST"
        );
    }
    for entry in owner.ledger().transition_entries() {
        let active = entry.id == consent(selected, true);
        let expected = if active {
            TransitionFate::Reversed
        } else {
            TransitionFate::Uncertain(Uncertainty::MissingFoldProvenance)
        };
        assert_eq!(
            owner.transition_fate(&entry.id).unwrap(),
            Some(expected),
            "SIBLING_TRANSITION_USES_SELECTED_EXACT_EVIDENCE"
        );
    }
    let counts = owner.transition_counts().unwrap();
    assert_eq!(counts.reversed, 1, "SELECTED_FOLDED_UNDO_IS_REVERSED");
    assert_eq!(
        counts.holds + counts.discarded,
        0,
        "UNKNOWN_SIBLING_IS_NOT_A_CERTAIN_ANSWER"
    );
    assert_eq!(
        counts.uncertain,
        owner.ledger().transition_entries().count() - 1,
        "ONLY_UNMATCHED_SIBLING_IS_UNCERTAIN"
    );
}
#[test]
fn folded_sibling_transition_approvals_survive_repeated_installs() {
    let pool = super::tests::story_pool();
    let captures = sibling_captures(&pool);
    let mut owner = make(&pool);
    for selected in [0_usize, 1, 0, 1] {
        install_line(&mut owner, &pool, &captures[selected], selected as u64);
        super::roundtrip_tests::saved_open(&owner);
    }
}
#[test]
fn surplus_resources_are_refused_before_the_install_token() {
    let pool = super::tests::story_pool();
    let owner = make(&pool);
    let before = owner.capture().unwrap().snapshot_identity().unwrap();
    let mut captured = owner.capture().unwrap();
    captured.data.resources.insert(Resource::Media {
        asset: 999,
        sha256: None,
        byte_len: None,
    });
    assert!(
        matches!(captured.validate(), Err(Error::InvalidResource)),
        "SURPLUS_RESOURCES_REFUSED_AS_INVALID_RESOURCE"
    );
    assert_eq!(
        owner.capture().unwrap().snapshot_identity().unwrap(),
        before,
        "SURPLUS_RESOURCE_REFUSAL_LEAVES_DESTINATION_UNCHANGED"
    );
}

#[test]
fn sibling_approvals_of_the_same_command_identity_survive_install_and_movements() {
    use super::tests::{approval, document, origin, remove};
    let pool = super::tests::story_pool();
    let mut left = OwnerHistory::new(&pool, Role::Task, Role::Task.policy(), &origin(1), || {
        Ok(document())
    })
    .unwrap();
    let mut right = OwnerHistory::new(&pool, Role::Task, Role::Task.policy(), &origin(1), || {
        Ok(document())
    })
    .unwrap();
    let a = approval();
    let mut b = approval();
    b.reference.sha256 = "b".repeat(64);
    left.execute(remove(), origin(2), vec![], Some(a)).unwrap();
    right.execute(remove(), origin(2), vec![], Some(b)).unwrap();
    left.install_snapshot(right.capture().unwrap().validate().unwrap())
        .expect("SAME_COMMAND_DIFFERENT_APPROVAL_NEVER_REFUSES_SIBLING_INSTALL");
    assert_eq!(
        left.ledger().len(),
        2,
        "EACH_CONSENT_KEEPS_ITS_OWN_LEDGER_ENTRY"
    );
    for (command, expected) in [
        (kinewright_core::Command::Undo, ApprovalAnswer::Missing),
        (kinewright_core::Command::Redo, ApprovalAnswer::Kept),
    ] {
        left.execute(command, origin(3), vec![], None).unwrap();
        assert!(
            left.ledger()
                .entries()
                .all(|entry| entry.answer() == expected),
            "ALL_APPROVALS_FOLLOW_THE_EXACT_TARGET_COMMAND_FATE"
        );
    }
    super::roundtrip_tests::saved_open(&left);
}

#[test]
fn selected_command_fact_settles_an_alias_approval_without_fold_provenance() {
    use super::tests::{approval, document, origin, remove};
    let pool = super::tests::story_pool();
    let mut destination =
        OwnerHistory::new(&pool, Role::Task, Role::Task.policy(), &origin(1), || {
            Ok(document())
        })
        .unwrap();
    let mut sibling = OwnerHistory::new(&pool, Role::Task, Role::Task.policy(), &origin(1), || {
        Ok(document())
    })
    .unwrap();
    let empty = sibling.capture().unwrap();
    let a = approval();
    let mut b = approval();
    b.reference.sha256 = "b".repeat(64);
    destination
        .execute(remove(), origin(2), vec![], Some(a))
        .unwrap();
    sibling
        .execute(remove(), origin(2), vec![], Some(b))
        .unwrap();
    sibling.install_snapshot(empty.validate().unwrap()).unwrap();
    destination
        .install_snapshot(sibling.capture().unwrap().validate().unwrap())
        .unwrap();
    assert_eq!(
        destination.ledger().len(),
        2,
        "KNOWN_COMMAND_FACT_PRESERVES_BOTH_CONSENT_IDENTITIES"
    );
    assert!(
        destination
            .ledger()
            .entries()
            .all(|entry| entry.fate == Fate::Discarded),
        "SELECTED_COMMAND_FACT_SETTLES_EVERY_APPROVAL_OF_THAT_COMMAND"
    );
    super::roundtrip_tests::saved_open(&destination);
}
#[test]
fn a_record_cannot_give_two_approvals_of_one_command_different_fates() {
    use super::tests::{approval, document, origin, remove};
    let pool = super::tests::story_pool();
    let mut owner = OwnerHistory::new(&pool, Role::Task, Role::Task.policy(), &origin(1), || {
        Ok(document())
    })
    .unwrap();
    owner
        .execute(remove(), origin(2), vec![], Some(approval()))
        .unwrap();
    let mut captured = owner.capture().unwrap();
    let mut other = captured.data.ledger.entries().next().unwrap().clone();
    other.approval.reference.sha256 = "b".repeat(64);
    other.fate = Fate::Redone;
    captured.data.ledger.commands.insert(
        (
            other.key,
            other.payload_digest,
            super::ledger::reference_key(&other.approval.reference).unwrap(),
        ),
        other,
    );
    assert!(
        matches!(captured.validate(), Err(Error::InvalidLedger)),
        "ONE_COMMAND_CANNOT_CARRY_CONTRADICTORY_APPROVAL_FATES"
    );
}

#[test]
fn forwarding_a_selected_snapshot_keeps_its_epoch_and_recomputed_approval_fate() {
    use super::tests::{approval, remove};
    for captured_undone in [false, true] {
        let pool = super::tests::story_pool();
        let make = || {
            OwnerHistory::new(&pool, Role::Task, Role::Task.policy(), &origin(1), || {
                Ok(document())
            })
            .unwrap()
        };
        let mut selected = make();
        let mut peer = make();
        for owner in [&mut selected, &mut peer] {
            owner
                .execute(remove(), origin(2), vec![], Some(approval()))
                .unwrap();
            if captured_undone {
                owner
                    .execute(Command::Undo, origin(3), vec![], None)
                    .unwrap();
            }
        }
        let saved = selected.capture().unwrap().validate().unwrap();
        for owner in [&mut selected, &mut peer] {
            owner
                .execute(
                    if captured_undone {
                        Command::Redo
                    } else {
                        Command::Undo
                    },
                    origin(4),
                    vec![],
                    None,
                )
                .unwrap();
        }
        selected.install_snapshot(saved).unwrap();
        let epoch = selected.history().position().epoch;
        peer.install_snapshot_at_epoch(selected.capture().unwrap().validate().unwrap(), epoch)
            .unwrap();
        assert_eq!(
            peer.identity().unwrap(),
            selected.identity().unwrap(),
            "FORWARDED_OWNER_INSTALL_HAS_THE_SAME_COMBINED_IDENTITY"
        );
        let approved = kinewright_core::undo::CommandKey {
            epoch: 0,
            position: 0,
        };
        let expected = if captured_undone {
            Fate::Undone
        } else {
            Fate::Applied
        };
        assert_eq!(
            peer.ledger().get(approved).unwrap().fate,
            expected,
            "FORWARDED_INSTALL_RECOMPUTES_THE_SELECTED_COMMAND_FATE"
        );
        assert_eq!(
            peer.ledger().get(approved).unwrap().install_epoch,
            Some(epoch),
            "FORWARDED_INSTALL_STAMPS_THE_AGREED_EPOCH"
        );
        super::roundtrip_tests::saved_open(&peer);
        let before = peer.identity().unwrap();
        let invalid = peer.capture().unwrap().validate().unwrap();
        assert_eq!(
            peer.install_snapshot_at_epoch(invalid, epoch),
            Err(Error::History(
                kinewright_core::undo::Error::InvalidMetadata
            ))
        );
        assert_eq!(
            peer.identity().unwrap(),
            before,
            "AGREED_EPOCH_REFUSAL_IS_ATOMIC_FOR_THE_WHOLE_OWNER"
        );
    }
}
