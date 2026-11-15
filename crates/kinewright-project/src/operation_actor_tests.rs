use super::*;
use crate::operation_history::{CreationSizing, PoolLimits};
use kinewright_core::{
    Operation, Track, TrackId, TrackKind,
    undo::{Charge, RoleBounds},
};

struct Proof;
fn origin() -> CreationOrigin {
    CreationOrigin {
        branch: BranchId::fresh().unwrap(),
        batch: BatchId::fresh().unwrap(),
    }
}
fn document() -> Document {
    Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Audio,
            sync_lock: false,
            clips: vec![],
        }],
        ..Document::default()
    }
}
fn lock(value: bool) -> Operation {
    Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked: value,
    }
}
fn locked(core: &Core) -> bool {
    match core.request(Command::Query(Query::Document)).unwrap() {
        Event::QueryResult(QueryResult::Document(document)) => document.tracks[0].sync_lock,
        other => panic!("DOCUMENT_QUERY_MUST_RETURN_DOCUMENT: {other:?}"),
    }
}
fn small(pool: &HistoryPool, role: Role) -> Core {
    let origin = origin();
    let owner = OwnerHistory::measured(
        pool,
        role,
        role.policy(),
        &origin,
        RoleBounds {
            step_cap: 1,
            ..role.bounds()
        },
        || Ok(document()),
    )
    .unwrap();
    Core::spawn_owner(owner, origin.branch, TimelineRevision::default()).unwrap()
}
#[test]
fn capture_keeps_its_state_without_stopping_live_folds() {
    let pool = HistoryPool::new(PoolLimits::default());
    let core = small(&pool, Role::Ordinary);
    core.request(Command::Do(lock(true))).unwrap();
    let before = core.export_history().unwrap();
    let identity = before.identity().unwrap();
    for i in 0..12 {
        core.request(Command::Do(lock(i % 2 != 0))).unwrap();
    }
    assert_eq!(
        before.identity().unwrap(),
        identity,
        "INFLIGHT_CAPTURE_STAYS_IMMUTABLE_ACROSS_FOLDS"
    );
    assert_eq!(
        core.export_history().unwrap().history().undo.len(),
        1,
        "CAPTURE_NEVER_BLOCKS_FOLD"
    );
    assert_eq!(
        pool.lease_counts(Role::Ordinary).unwrap().1,
        3,
        "CAPTURE_AND_INDEPENDENT_READ_ARE_CHARGED"
    );
}
#[test]
fn revision_token_survives_a_history_capacity_refusal() {
    let pool = HistoryPool::new(PoolLimits::default());
    let core = small(&pool, Role::Task);
    core.request(Command::Do(lock(true))).unwrap();
    let before = core.export_history().unwrap();
    let token = Some(CommandToken(71));
    let refused = core
        .request(Command::DoIfRevision {
            expected: TimelineRevision(1),
            operation: lock(false),
            token,
        })
        .unwrap_err();
    assert_eq!(
        refused.token, token,
        "HISTORY_REFUSAL_PRESERVES_CORRELATION_TOKEN"
    );
    assert!(
        matches!(
            refused.error,
            Error::History(HistoryError::History(
                kinewright_core::undo::Error::HistoryFull
            ))
        ),
        "TASK_LIMIT_IS_A_TYPED_REFUSAL"
    );
    assert_eq!(
        core.export_history().unwrap().identity().unwrap(),
        before.identity().unwrap(),
        "ACTOR_CAPACITY_REFUSAL_CHANGES_NOTHING"
    );
}
#[test]
fn no_op_close_advances_position_and_default_recorder_is_disabled() {
    let pool = HistoryPool::new(PoolLimits::default());
    let core = small(&pool, Role::Ordinary);
    let before = core.change_id().unwrap();
    core.close_gesture().unwrap();
    let after = core.change_id().unwrap();
    assert_eq!(
        after.position.position,
        before.position.position + 1,
        "ACTOR_NOOP_CLOSE_ADVANCES_POSITION"
    );
    assert_eq!(
        after.revision, before.revision,
        "ACTOR_NOOP_CLOSE_DOES_NOT_CHANGE_TIMELINE"
    );
    assert_eq!(
        core.export_history().unwrap().history().undo.len(),
        0,
        "ACTOR_NOOP_CLOSE_LEAVES_STACKS_EMPTY"
    );
    assert!(
        matches!(
            core.request(Command::Query(Query::OpLog)),
            Err(Refusal {
                error: Error::RecorderDisabled,
                ..
            })
        ),
        "PRODUCT_ACTOR_HAS_NO_IMPLICIT_RECORDER"
    );
}
#[test]
fn frozen_installs_preserve_original_command_keys_and_invalidate_old_cuts() {
    let pool = HistoryPool::new(PoolLimits::default());
    let core = small(&pool, Role::Task);
    core.request(Command::Do(lock(true))).unwrap();
    let selected = core.export_history().unwrap();
    let original_key = selected.history().undo[0].commands()[0].id;
    core.request(Command::Undo).unwrap();
    core.request(Command::Do(lock(false))).unwrap();
    let attachment = core.attach(&Proof).unwrap();
    let cut = attachment.freeze(&Proof).unwrap();
    attachment
        .install_snapshot(&Proof, cut, selected.validate().unwrap())
        .unwrap();
    assert!(
        matches!(attachment.resume(&Proof, cut), Err(Error::CaptureChanged)),
        "INSTALL_INVALIDATES_OLD_ATTACHMENT_CUT"
    );
    let current = attachment.freeze(&Proof).unwrap();
    assert!(
        current.change.position.epoch > original_key.epoch,
        "ACTOR_INSTALL_STARTS_A_NEW_EPOCH"
    );
    assert_eq!(
        core.export_history().unwrap().history().undo[0].commands()[0].id,
        original_key,
        "ACTOR_INSTALL_DOES_NOT_REKEY_RETAINED_COMMANDS"
    );
    attachment.resume(&Proof, current).unwrap();
    assert!(locked(&core), "ACTOR_INSTALLED_SELECTED_DOCUMENT");
    core.request(Command::Undo).unwrap();
    assert!(
        !locked(&core),
        "ACTOR_UNDO_AFTER_INSTALL_USES_SELECTED_HISTORY"
    );
}
#[test]
fn retiring_acknowledges_actual_lease_release_with_stale_handles_alive() {
    let sizing = CreationSizing {
        kind: crate::operation_history::CreationKind::Task,
        baseline_live: kinewright_core::undo::document_capacity_bytes(&document()).unwrap(),
        baseline_owners: 0,
        initial_batch: Charge::default(),
    };
    // Determine the actual baseline owner's charge through the same factory.
    let probe_pool = HistoryPool::new(PoolLimits::default());
    let probe = OwnerHistory::new(
        &probe_pool,
        Role::Task,
        Role::Task.policy(),
        &origin(),
        || Ok(document()),
    )
    .unwrap();
    let sizing = CreationSizing {
        baseline_owners: probe.creation_sizing().baseline_owners,
        ..sizing
    };
    let retained = sizing.limits().unwrap().envelope.retained;
    drop(probe);
    let pool = HistoryPool::new(PoolLimits {
        retained: [0, 2 * retained, 0],
        peak: [0, 2 * retained + (168 << 20), 0],
        workspace: 168 << 20,
    });
    let core = Core::spawn(&pool, Role::Task, origin(), document()).unwrap();
    let stale = core.clone();
    let retained_document = match core.request(Command::Query(Query::Document)).unwrap() {
        Event::QueryResult(QueryResult::Document(document)) => document,
        other => panic!("DOCUMENT_QUERY_MUST_RETURN_DOCUMENT: {other:?}"),
    };
    let weak_document = Arc::downgrade(&retained_document);
    drop(retained_document);
    assert!(
        matches!(
            Core::spawn(&pool, Role::Task, origin(), document()),
            Err(Error::History(HistoryError::PoolFull(Role::Task)))
        ),
        "FULL_TASK_POOL_REFUSES_BEFORE_START"
    );
    let attachment = core.attach(&Proof).unwrap();
    let cut = attachment.freeze(&Proof).unwrap();
    attachment.retire(&Proof, cut).unwrap();
    assert!(
        weak_document.upgrade().is_none(),
        "RETIRE_ACK_DROPS_TASK_DOCUMENTS"
    );
    assert_eq!(
        pool.usage(Role::Task).unwrap(),
        (0, 0),
        "RETIRE_ACK_FOLLOWS_HISTORY_LEASE_DESTRUCTION"
    );
    assert!(
        matches!(
            stale.request(Command::Query(Query::Document)),
            Err(Refusal {
                error: Error::Retired,
                ..
            })
        ),
        "STALE_TASK_HANDLES_RETAIN_NO_HISTORY"
    );
    let next = Core::spawn(&pool, Role::Task, origin(), document())
        .expect("NEXT_TASK_ADMITTED_IMMEDIATELY_AFTER_RETIRE_ACK");
    let next_attachment = next.attach(&Proof).unwrap();
    let cut = next_attachment.freeze(&Proof).unwrap();
    next_attachment.retire(&Proof, cut).unwrap();
}

#[test]
fn warm_actor_edit_copies_one_document_and_measures_neither_document_nor_owner() {
    let pool = HistoryPool::new(PoolLimits::default());
    let core = Core::spawn(&pool, Role::Ordinary, origin(), document()).unwrap();
    core.request(Command::Do(lock(true))).unwrap();
    let (core_before, owner_before) = core.counters().unwrap();
    core.request(Command::Do(lock(false))).unwrap();
    let (core_after, owner_after) = core.counters().unwrap();
    let phases = core_after.since(core_before);
    let owners = owner_after.since(owner_before);
    assert_eq!(
        phases.document_copies, 1,
        "ACTOR_WARM_EDIT_ONE_DOCUMENT_COPY"
    );
    assert_eq!(
        phases.exact_measurements, 0,
        "ACTOR_WARM_EDIT_ZERO_EXACT_MEASUREMENTS"
    );
    assert_eq!(
        owners.owner_identity_hashes, 0,
        "ACTOR_WARM_EDIT_NO_OWNER_HASH"
    );
    assert_eq!(
        owners.document_scans, 0,
        "ACTOR_WARM_EDIT_NO_OWNER_DOCUMENT_SCAN"
    );
    assert_eq!(
        owners.seed_map_copies, 0,
        "ACTOR_WARM_EDIT_NO_SEED_MAP_COPY"
    );
}

#[test]
fn explicit_activity_recorder_and_applied_delta_have_separate_leased_readers() {
    let pool = HistoryPool::new(PoolLimits::default());
    let core = Core::spawn_recording(&pool, Role::Task, origin(), document()).unwrap();
    core.request(Command::Do(lock(true))).unwrap();
    core.request(Command::Do(lock(false))).unwrap();
    core.request(Command::Undo).unwrap();
    let activity = core.operations(true).unwrap();
    let applied = core.operations(false).unwrap();
    assert_eq!(
        &*activity,
        &[lock(true), lock(false)],
        "RECORDER_KEEPS_COMPLETE_ACTIVITY"
    );
    assert_eq!(
        &*applied,
        &[lock(true)],
        "APPLIED_READER_EXCLUDES_UNDONE_COMMAND"
    );
    assert_eq!(
        pool.lease_counts(Role::Task).unwrap().0,
        3,
        "RETAINED_OPERATION_READERS_KEEP_THEIR_OWN_LEASES"
    );
}
