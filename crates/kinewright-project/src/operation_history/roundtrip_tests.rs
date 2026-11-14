use super::tests::{approval, document, origin, remove};
use super::*;
use kinewright_core::{Command, Operation, TrackId, undo::RetentionPolicy};

pub(super) fn saved_open(owner: &OwnerHistory) {
    if let Some(state) = &owner.fold {
        state.assert_matches(
            &owner.seeds,
            owner.history().baseline(),
            &owner.resources.baseline,
        );
    }
    let OwnerSnapshot { data, reservation } = owner.capture().unwrap();
    let wire = super::stored::codec_tests::encoded(&OwnerSnapshot { data, reservation });
    let shell: super::StoredOwnerRecord = serde_json::from_slice(&wire).unwrap();
    super::stored::assert_decode_forecast(&shell, owner.decode_bound);
    let OwnerSnapshot { data, reservation } = owner.capture().unwrap();
    let bytes = serde_json::to_vec(&data).unwrap();
    drop(data);
    let data = serde_json::from_slice(&bytes).expect("SAVED_OWNER_BYTES_DECODE");
    let opened = OwnerSnapshot { data, reservation }
        .validate()
        .expect("WHAT_CAN_BE_SAVED_CAN_BE_OPENED")
        .open()
        .unwrap();
    assert_same(owner, &opened);
}

pub(super) fn assert_same(owner: &OwnerHistory, opened: &OwnerHistory) {
    assert_eq!(
        opened.history().document(),
        owner.history().document(),
        "SAVED_OPEN_DOCUMENT"
    );
    assert_eq!(
        opened.history().snapshot_identity().unwrap(),
        owner.history().snapshot_identity().unwrap(),
        "SAVED_OPEN_BOTH_STACKS_AND_COUNTERS"
    );
    let expected_identity = owner.capture().unwrap().snapshot_identity().unwrap();
    let opened_identity = opened.capture().unwrap().snapshot_identity().unwrap();
    assert_eq!(
        opened_identity, expected_identity,
        "SAVED_OPEN_COMPLETE_OWNER_IDENTITY"
    );
    assert_eq!(opened.ledger(), owner.ledger(), "SAVED_OPEN_COMMAND_LEDGER");
    assert_eq!(
        opened.transition_counts().unwrap(),
        owner.transition_counts().unwrap(),
        "SAVED_OPEN_TRANSITION_COUNTS"
    );
    for entry in owner.ledger().transition_entries() {
        assert_eq!(
            opened.transition_fate(&entry.id).unwrap(),
            owner.transition_fate(&entry.id).unwrap(),
            "SAVED_OPEN_TRANSITION_ANSWER"
        );
    }
    assert!(
        opened.resources().values().eq(owner.resources().values()),
        "SAVED_OPEN_RESOURCES"
    );
    assert_eq!(
        opened.resources(),
        owner.resources(),
        "SAVED_OPEN_RESOURCE_COUNTS"
    );
    for depth in 0..=owner.history().undo().len() + owner.history().redo().len() {
        // The expected lineage is an independent test value. Drop its boundary
        // lease before asking for the other boundary in a four-holder pool.
        let expected = owner.boundary(depth).unwrap().lineage().clone();
        assert_eq!(
            opened.boundary(depth).unwrap().lineage(),
            &expected,
            "SAVED_OPEN_EVERY_BOUNDARY_LINEAGE"
        );
    }
}

#[derive(Clone, Copy)]
pub(super) enum InstallCase {
    Single,
    Twice,
    Undo,
}

pub(super) fn older_install(
    pool: &HistoryPool,
    case: InstallCase,
    reference: crate::bundle_records::ObjectRef,
) -> OwnerHistory {
    let mut owner = OwnerHistory::new(
        pool,
        Role::Task,
        RetentionPolicy::NeverFold,
        &origin(1),
        || Ok(document()),
    )
    .unwrap();
    let first = owner.capture().unwrap();
    let second = matches!(case, InstallCase::Twice).then(|| owner.capture().unwrap());
    for n in 0..4 {
        owner
            .execute(
                Command::Do(Operation::SetTrackSyncLock {
                    track: TrackId(81),
                    locked: n % 2 == 0,
                }),
                origin(n + 2),
                vec![],
                None,
            )
            .unwrap();
    }
    match case {
        InstallCase::Single | InstallCase::Twice => {
            let mut approval = approval();
            approval.reference = reference;
            owner
                .execute(remove(), origin(6), vec![], Some(approval))
                .unwrap();
        }
        InstallCase::Undo => {
            let target = owner.transition_target(Direction::Undo).unwrap();
            owner
                .approve_transition(reference, Direction::Undo, target)
                .unwrap();
        }
    }
    owner.install_snapshot(first.validate().unwrap()).unwrap();
    saved_open(&owner);
    if let Some(second) = second {
        owner.install_snapshot(second.validate().unwrap()).unwrap();
        saved_open(&owner);
    }
    owner
        .execute(
            Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(81),
                locked: true,
            }),
            origin(7),
            vec![],
            None,
        )
        .unwrap();
    owner
}

#[test]
fn older_snapshot_install_with_later_do_can_reopen() {
    let owner = older_install(
        &HistoryPool::new(PoolLimits::default()),
        InstallCase::Single,
        approval().reference,
    );
    saved_open(&owner);
}
#[test]
fn two_older_snapshot_installs_with_later_do_can_reopen() {
    let owner = older_install(
        &HistoryPool::new(PoolLimits::default()),
        InstallCase::Twice,
        approval().reference,
    );
    saved_open(&owner);
}
#[test]
fn older_snapshot_install_with_approved_undo_can_reopen() {
    let owner = older_install(
        &HistoryPool::new(PoolLimits::default()),
        InstallCase::Undo,
        approval().reference,
    );
    saved_open(&owner);
}

#[test]
fn historic_approval_positions_still_refuse_overflow() {
    let owner = older_install(
        &HistoryPool::new(PoolLimits::default()),
        InstallCase::Single,
        approval().reference,
    );
    let mut snapshot = owner.capture().unwrap();
    let (_, mut entry) = snapshot.data.ledger.commands.pop_first().unwrap();
    entry.key.position = u64::MAX;
    snapshot
        .data
        .ledger
        .commands
        .insert((entry.key, entry.payload_digest), entry);
    assert_eq!(
        snapshot.validate().err(),
        Some(Error::InvalidLedger),
        "HISTORIC_APPROVAL_POSITION_MUST_NOT_OVERFLOW"
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one deterministic transition grammar and its positive coverage assertions"
)]
fn generated_owner_states_can_be_saved_and_opened() {
    let mut checks = 0;
    let mut folds = 0;
    let mut historic_reopens = 0;
    let mut invalid_reference_refusals = 0;
    for seed in 1..=4_u64 {
        for role in [Role::Ordinary, Role::Task] {
            let pool = HistoryPool::new(PoolLimits::default());
            let bounds = kinewright_core::undo::RoleBounds {
                step_cap: 3,
                ..role.bounds()
            };
            let mut owner =
                OwnerHistory::measured(&pool, role, role.policy(), &origin(1), bounds, || {
                    Ok(document())
                })
                .unwrap();
            let mut selected = Some(owner.capture().unwrap());
            let mut random = seed;
            for i in 0..64 {
                let before_frontier = owner.fold_frontier;
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                let mut installed = false;
                match random % 8 {
                    0 | 7 => {
                        let _ = owner.execute(
                            Command::Do(Operation::SetTrackSyncLock {
                                track: TrackId(81),
                                locked: i % 2 == 0,
                            }),
                            origin(i + 2),
                            vec![],
                            None,
                        );
                    }
                    1 => {
                        let _ = owner.execute(Command::Undo, origin(i + 2), vec![], None);
                    }
                    2 => {
                        let _ = owner.execute(Command::Redo, origin(i + 2), vec![], None);
                    }
                    3 => {
                        invalid_reference_refusals +=
                            usize::from(generated_removal(&mut owner, seed, i, random));
                    }
                    4 => {
                        let _ = owner.execute(
                            Command::Do(Operation::AddTrack {
                                track: document().tracks.remove(0),
                            }),
                            origin(i + 2),
                            vec![],
                            None,
                        );
                    }
                    5 => {
                        if selected.is_none() {
                            selected = Some(owner.capture().unwrap());
                        }
                    }
                    6 => {
                        if let Some(snapshot) = selected.take() {
                            owner
                                .install_snapshot(snapshot.validate_bounded(bounds).unwrap())
                                .unwrap();
                            assert!(
                                owner
                                    .ledger()
                                    .entries()
                                    .all(|entry| entry.answer() != ApprovalAnswer::Uncertain),
                                "SAME_LINE_INSTALL_NEVER_MAKES_DO_UNCERTAIN"
                            );
                            installed = true;
                        }
                    }
                    _ => unreachable!(),
                }
                let folded = !installed && owner.fold_frontier != before_frontier;
                folds += usize::from(folded);
                if installed || folded || random.is_multiple_of(5) {
                    let next = owner.history().position();
                    historic_reopens += usize::from(owner.ledger().entries().any(|entry| {
                        entry.key.epoch < next.epoch && entry.key.position >= next.position
                    }));
                    saved_open(&owner);
                    checks += 1;
                }
            }
        }
    }
    assert!(checks > 0, "GENERATED_SAVE_OPEN_INVARIANT_EXERCISED");
    assert!(folds > 0, "GENERATED_SAVE_OPEN_AFTER_FOLD_EXERCISED");
    assert!(
        historic_reopens > 0,
        "GENERATED_HISTORIC_POSITION_EXERCISED"
    );
    assert!(
        invalid_reference_refusals > 0,
        "GENERATED_MALFORMED_APPROVAL_REFERENCE_REFUSED"
    );
    println!(
        "HX_OWNER_PROPERTY_SAVE_OPEN={checks} FOLDS={folds} HISTORIC={historic_reopens} INVALID_REFERENCES={invalid_reference_refusals}"
    );
}

fn generated_removal(owner: &mut OwnerHistory, seed: u64, index: u64, random: u64) -> bool {
    let invalid = (random >> 8).is_multiple_of(5);
    let mut consent = approval();
    consent.reference.sha256 = if invalid {
        "bad".into()
    } else {
        format!("{seed:032x}{index:032x}")
    };
    let result = owner.execute(remove(), origin(index + 2), vec![], Some(consent));
    if invalid {
        assert!(
            result.is_err(),
            "GENERATED_MALFORMED_APPROVAL_MUST_NOT_BE_ADMITTED {result:?}"
        );
        matches!(result, Err(Error::InvalidLedger))
    } else {
        false
    }
}
