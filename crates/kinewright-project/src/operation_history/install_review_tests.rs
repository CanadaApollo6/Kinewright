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
    let snapshot = OwnerSnapshot {
        data: serde_json::from_slice(bytes).unwrap(),
        reservation: pool.admit(Role::Ordinary).unwrap(),
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
    let pool = HistoryPool::new(PoolLimits::default());
    let captures = sibling_captures(&pool);
    let mut owner = make(&pool);
    for selected in [0_usize, 1, 0, 1] {
        install_line(&mut owner, &pool, &captures[selected], selected as u64);
        super::roundtrip_tests::saved_open(&owner);
    }
}
#[test]
fn surplus_resources_are_refused_before_the_install_token() {
    let pool = HistoryPool::new(PoolLimits::default());
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
