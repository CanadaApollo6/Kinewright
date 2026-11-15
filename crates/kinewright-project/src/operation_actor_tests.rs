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

fn context() -> CommandContext {
    CommandContext {
        creation: origin(),
        inputs: vec![],
        approval: None,
        transition: None,
    }
}
#[test]
fn prepared_task_update_is_atomic_and_releases_its_working_lease_before_ack() {
    let pool = HistoryPool::new(PoolLimits::default());
    let core = Core::spawn(&pool, Role::Task, origin(), document()).unwrap();
    let live_usage = pool.usage(Role::Task).unwrap().0;
    let attachment = core.attach(&Proof).unwrap();
    let cut = attachment.freeze(&Proof).unwrap();
    let action = crate::operation_protocol::Action::Do {
        operations: vec![lock(true)],
        gesture: None,
    };
    let prepared = attachment.prepare(&Proof, cut, action, context()).unwrap();
    assert_eq!(
        pool.lease_counts(Role::Task).unwrap().0,
        2,
        "PREPARATION_USES_ONE_WORKING_HOLDER"
    );
    assert!(!locked(&core), "PREPARATION_LEAVES_LIVE_DOCUMENT_UNCHANGED");
    assert_eq!(
        attachment.freeze(&Proof).unwrap(),
        cut,
        "PREPARATION_LEAVES_LIVE_IDENTITY_UNCHANGED"
    );
    let selected = prepared.identity().unwrap();
    let change = attachment.commit_prepared(&Proof, cut, prepared).unwrap();
    assert_eq!(
        change.position.epoch, cut.change.position.epoch,
        "PUBLISHED_EDIT_DOES_NOT_INSTALL_ANOTHER_EPOCH"
    );
    assert_eq!(
        pool.usage(Role::Task).unwrap().0,
        live_usage,
        "COMMIT_ACK_FOLLOWS_WORKING_LEASE_RELEASE"
    );
    let installed = attachment.freeze(&Proof).unwrap();
    assert_eq!(
        installed.identity(),
        selected,
        "COMMITTED_IDENTITY_EQUALS_PUBLISHED_PROSPECTIVE_IDENTITY"
    );
    attachment.resume(&Proof, installed).unwrap();
    assert!(locked(&core), "PUBLISHED_PREPARED_DOCUMENT_BECOMES_LIVE");
}
#[test]
fn candidate_staging_preserves_earlier_commands_and_the_live_creation_room() {
    let pool = HistoryPool::new(PoolLimits::default());
    let core = Core::spawn(&pool, Role::Candidate, origin(), document()).unwrap();
    let original = core.export_history().unwrap().creation_sizing();
    let live_usage = pool.usage(Role::Candidate).unwrap().0;
    core.request(Command::Do(lock(true))).unwrap();
    let first = core.export_history().unwrap().history().undo[0].commands()[0].id;
    let attachment = core.attach(&Proof).unwrap();
    for value in [false, true] {
        let cut = attachment.freeze(&Proof).unwrap();
        let prepared = attachment
            .prepare_merge(&Proof, cut, vec![lock(value)], context())
            .unwrap();
        assert!(
            pool.usage(Role::Candidate).unwrap().0 < 2 * live_usage,
            "CANDIDATE_STAGING_USES_MEASURED_DELTA_ROOM"
        );
        let identity = prepared.identity().unwrap();
        attachment.commit_prepared(&Proof, cut, prepared).unwrap();
        assert_eq!(
            pool.usage(Role::Candidate).unwrap().0,
            live_usage,
            "CANDIDATE_LIVE_LEASE_NEVER_GROWS"
        );
        let current = attachment.freeze(&Proof).unwrap();
        assert_eq!(
            current.identity(),
            identity,
            "CANDIDATE_PUBLISHED_IDENTITY_USES_LIVE_DESCRIPTOR"
        );
        attachment.resume(&Proof, current).unwrap();
    }
    let snapshot = core.export_history().unwrap();
    assert_eq!(
        snapshot.creation_sizing(),
        original,
        "CANDIDATE_COMMIT_KEEPS_IMMUTABLE_CREATION_ROOM"
    );
    let keys: Vec<_> = snapshot
        .history()
        .undo
        .iter()
        .flat_map(|s| s.commands().iter().map(|c| c.id))
        .collect();
    assert_eq!(keys.len(), 3, "CANDIDATE_MERGE_KEEPS_FULL_APPLIED_PREFIX");
    assert_eq!(
        keys[0], first,
        "CANDIDATE_MERGE_NEVER_REKEYS_PRIOR_COMMANDS"
    );
    assert!(
        keys.windows(2).all(|pair| pair[0] < pair[1]),
        "CANDIDATE_MERGE_ACCEPTS_FRESH_KEYS_IN_ORDER"
    );
}
#[test]
fn refused_private_update_changes_nothing_and_releases_its_lease() {
    let pool = HistoryPool::new(PoolLimits::default());
    let core = Core::spawn(&pool, Role::Task, origin(), document()).unwrap();
    let attachment = core.attach(&Proof).unwrap();
    let cut = attachment.freeze(&Proof).unwrap();
    let before = pool.usage(Role::Task).unwrap();
    let action = crate::operation_protocol::Action::Do {
        operations: vec![Operation::RemoveTrack { track: TrackId(99) }],
        gesture: None,
    };
    assert!(
        attachment.prepare(&Proof, cut, action, context()).is_err(),
        "INVALID_PREPARED_EDIT_IS_REFUSED"
    );
    assert_eq!(
        pool.usage(Role::Task).unwrap(),
        before,
        "REFUSED_PREVIEW_RELEASES_ALL_WORKING_MEMORY"
    );
    assert_eq!(
        attachment.freeze(&Proof).unwrap(),
        cut,
        "REFUSED_PREPARED_EDIT_PRESERVES_COMPLETE_LIVE_STATE"
    );
}
#[cfg(target_os = "linux")]
#[test]
fn prospective_candidate_record_opens_with_the_original_live_descriptor_before_commit() {
    use crate::{
        bundle_copy::SessionStore,
        operation_history::{CreationKind, StoredOwnerRecord},
        storage_budget::StorageLimits,
    };
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "hx-prepared-owner",
    );
    let mut session = SessionStore::create(temp.root(), StorageLimits::default()).unwrap();
    let pool = HistoryPool::new(PoolLimits::default());
    let core = Core::spawn(&pool, Role::Candidate, origin(), document()).unwrap();
    core.request(Command::Do(lock(true))).unwrap();
    let descriptor = core.export_history().unwrap().creation_sizing();
    let attachment = core.attach(&Proof).unwrap();
    let cut = attachment.freeze(&Proof).unwrap();
    let prepared = attachment
        .prepare_merge(&Proof, cut, vec![lock(false)], context())
        .unwrap();
    let expected = prepared.identity().unwrap();
    let record = prepared.persist(session.store_mut(), |_| Ok(None)).unwrap();
    let reader_pool = HistoryPool::new(PoolLimits::default());
    let opened = StoredOwnerRecord::read(
        session.store(),
        &record.reference,
        &reader_pool,
        Role::Candidate,
    )
    .unwrap()
    .open()
    .unwrap();
    assert_eq!(
        opened.creation_sizing(),
        descriptor,
        "PREPARED_RECORD_REOPENS_WITH_ORIGINAL_LIVE_SIZING"
    );
    assert_eq!(
        opened.creation_sizing().kind,
        CreationKind::LiveCandidate,
        "STAGING_RECORD_CANNOT_SHRINK_LIVE_CANDIDATE_ROOM"
    );
    assert_eq!(
        opened.identity().unwrap(),
        expected,
        "PREPARED_RECORD_IDENTITY_SURVIVES_COLD_OPEN"
    );
    assert!(
        locked(&core),
        "PERSISTING_PREVIEW_DOES_NOT_MUTATE_LIVE_CORE"
    );
    attachment.commit_prepared(&Proof, cut, prepared).unwrap();
    assert_eq!(
        attachment.freeze(&Proof).unwrap().identity(),
        expected,
        "PUBLICATION_AND_COMMIT_NAME_THE_SAME_OWNER_STATE"
    );
}
