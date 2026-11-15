use super::tests::{approval, document, origin, remove};
use super::*;
use kinewright_core::{Command, Document, Operation, TrackId, undo::RoleBounds};

fn clip_document() -> Document {
    let mut cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../kinewright-core/tests/support/history_operation_cases.json"
    ))
    .unwrap();
    serde_json::from_value(cases["SplitClip"]["document"].take()).unwrap()
}
fn make(pool: &HistoryPool, role: Role, doc: Document) -> OwnerHistory {
    OwnerHistory::new(pool, role, role.policy(), &origin(1), || Ok(doc)).unwrap()
}
fn different(base: &OwnerSnapshot, changed: OwnerSnapshot, name: &str) {
    let a = base.identity().unwrap();
    let b = changed.identity().unwrap();
    assert_eq!(a.core, b.core, "CORE_DIGEST_FIXED {name}");
    assert_ne!(a.owner, b.owner, "{name}");
    assert_ne!(
        a.digest(),
        b.digest(),
        "COMBINED_IDENTITY_BINDS_OWNER {name}"
    );
    changed.validate().unwrap();
}
#[test]
fn owner_identity_binds_valid_origin_and_seed_with_core_fixed() {
    let pool = super::tests::story_pool();
    let mut owner = make(&pool, Role::Task, clip_document());
    let track = owner.history().document().tracks[0].id;
    owner
        .execute(
            Command::Do(Operation::SetTrackSyncLock {
                track,
                locked: true,
            }),
            origin(2),
            vec![],
            None,
        )
        .unwrap();
    let base = owner.capture().unwrap();
    let mut changed = owner.capture().unwrap();
    changed.data.origins[0].creation = origin(99);
    different(&base, changed, "OWNER_IDENTITY_BINDS_VALID_ORIGIN");
    let mut changed = owner.capture().unwrap();
    changed
        .data
        .seeds
        .occurrences
        .values_mut()
        .next()
        .unwrap()
        .creation_batch_id = origin(99).batch;
    different(&base, changed, "OWNER_IDENTITY_BINDS_VALID_SEED");
}
#[test]
fn owner_identity_binds_transition_direction_with_core_fixed() {
    let pool = super::tests::story_pool();
    let mut owner = make(&pool, Role::Task, document());
    owner.execute(remove(), origin(2), vec![], None).unwrap();
    owner
        .approve_transition(
            approval().reference,
            Direction::Undo,
            owner.transition_target(Direction::Undo).unwrap(),
        )
        .unwrap();
    let base = owner.capture().unwrap();
    let mut changed = owner.capture().unwrap();
    changed.data.ledger.transitions[0].direction = Direction::Redo;
    different(&base, changed, "OWNER_IDENTITY_BINDS_TRANSITION_DIRECTION");
}
#[test]
fn owner_identity_binds_fold_fact_with_core_fixed() {
    let pool = super::tests::story_pool();
    let mut snapshots = Vec::new();
    for coalesced in [false, true] {
        let bounds = RoleBounds {
            step_cap: 1,
            ..Role::Ordinary.bounds()
        };
        let mut owner = OwnerHistory::measured(
            &pool,
            Role::Ordinary,
            Role::Ordinary.policy(),
            &origin(1),
            bounds,
            || Ok(document()),
        )
        .unwrap();
        let operations = vec![Operation::SetTrackSyncLock {
            track: TrackId(81),
            locked: true,
        }];
        let first = if coalesced {
            Command::DoBatchCoalesced {
                operations,
                coalesce_key: "g".into(),
            }
        } else {
            Command::DoBatch(operations)
        };
        owner.execute(first, origin(2), vec![], None).unwrap();
        let operations = vec![Operation::RemoveTrack { track: TrackId(81) }];
        let second = if coalesced {
            Command::DoBatchCoalesced {
                operations,
                coalesce_key: "g".into(),
            }
        } else {
            Command::DoBatch(operations)
        };
        owner
            .execute(second, origin(3), vec![], Some(approval()))
            .unwrap();
        owner
            .execute(
                Command::Do(Operation::AddTrack {
                    track: document().tracks.remove(0),
                }),
                origin(4),
                vec![],
                None,
            )
            .unwrap();
        snapshots.push(owner.capture().unwrap());
    }
    let changed = snapshots.pop().unwrap();
    different(&snapshots[0], changed, "OWNER_IDENTITY_BINDS_FOLD_FACT");
}
#[test]
fn owner_identity_binds_role_with_core_fixed() {
    let pool = super::tests::story_pool();
    let task = make(&pool, Role::Task, document());
    let candidate = make(&pool, Role::Candidate, document());
    different(
        &task.capture().unwrap(),
        candidate.capture().unwrap(),
        "OWNER_IDENTITY_BINDS_ROLE",
    );
}

#[test]
fn borrowed_identity_matches_captures_without_copying_seed_maps_after_folds() {
    let pool = super::tests::story_pool();
    let bounds = RoleBounds {
        step_cap: 1,
        ..Role::Ordinary.bounds()
    };
    let mut owner = OwnerHistory::measured(
        &pool,
        Role::Ordinary,
        Role::Ordinary.policy(),
        &origin(1),
        bounds,
        || Ok(clip_document()),
    )
    .unwrap();
    let mut cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../kinewright-core/tests/support/history_operation_cases.json"
    ))
    .unwrap();
    let split = serde_json::from_value(cases["SplitClip"]["operation"].take()).unwrap();
    owner
        .execute(Command::Do(split), origin(2), vec![], None)
        .unwrap();
    let track = owner.history().document().tracks[0].id;
    for position in 3..7 {
        owner
            .execute(
                Command::Do(Operation::SetTrackSyncLock {
                    track,
                    locked: position % 2 == 0,
                }),
                origin(position),
                vec![],
                None,
            )
            .unwrap();
        let captured = owner.capture().unwrap();
        let expected = captured.identity().unwrap();
        let before = OwnerCounters::read();
        assert_eq!(
            owner.identity().unwrap(),
            expected,
            "BORROWED_IDENTITY_MATCHES_CAPTURE_AFTER_FOLD"
        );
        assert_eq!(
            OwnerCounters::read().since(before).seed_map_copies,
            0,
            "BORROWED_IDENTITY_NEVER_COPIES_SEED_MAPS"
        );
    }
}
