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

fn producer_lease(
    core: &Core,
    branch: BranchId,
    sequence: u64,
) -> crate::operation_protocol::Lease {
    use crate::{
        bundle_records::{HeadId, ProjectId},
        operation_protocol::{Lease, Seed, StreamState},
        receipts::IssuerId,
    };
    let snapshot = core.export_history().unwrap();
    let identity = snapshot.identity().unwrap();
    Lease {
        state: StreamState::installed(Seed {
            project: ProjectId::fresh().unwrap(),
            branch,
            attachment_epoch: sequence,
            installed: identity,
        })
        .unwrap(),
        snapshot: identity,
        next_key: core.change_id().unwrap().position,
        issuer: IssuerId(format!("{sequence:032x}")),
        next_issuer_sequence: 1,
        generation: sequence,
        client: "gui".into(),
        token: "test-authority".into(),
        switch_generation: 1,
        owner_epoch: 1,
        anchor_head: HeadId::fresh().unwrap(),
        inputs: crate::operation_inputs::ManualInputs::fixture()
            .reference()
            .clone(),
        parent_identity: None,
        namespace_name: None,
    }
}
fn pending_receipts(permit: &ManualPermit) -> Vec<crate::operation_protocol::Receipt> {
    let mut stream = permit.pending().unwrap().into_iter();
    stream
        .by_ref()
        .map(|encoded| {
            let message = encoded.decode().unwrap();
            crate::operation_protocol::Receipt {
                id: message.id.clone(),
                sequence: message.sequence,
                accepted: message.accepted,
                tip: message.tip,
                command: message.digest().unwrap(),
            }
        })
        .collect()
}
#[test]
fn manual_actor_fences_raw_handles_and_stale_permits_without_changing_pending_work() {
    let pool = HistoryPool::new(PoolLimits::default());
    let origin = origin();
    let core = Core::spawn(&pool, Role::Ordinary, origin.clone(), document()).unwrap();
    let attachment = core.attach(&Proof).unwrap();
    let live = Arc::new(AtomicBool::new(true));
    let permit = attachment
        .manual_permit(
            &Proof,
            None,
            producer_lease(&core, origin.branch.clone(), 1),
            crate::operation_inputs::ManualInputs::fixture(),
            Arc::clone(&live),
        )
        .unwrap();
    let token = Some(CommandToken(19));
    assert_eq!(
        core.request(Command::DoIfRevision {
            expected: TimelineRevision(0),
            operation: lock(true),
            token
        })
        .unwrap_err(),
        Refusal {
            error: Error::MutationRefused(CoreMutationRefusal::ManualPermitRequired),
            token,
        },
        "RAW_HANDLE_CANNOT_BYPASS_ACTOR_PRODUCER"
    );
    permit.request(Command::Do(lock(true))).unwrap();
    let receipts = pending_receipts(&permit);
    let cut = attachment.freeze(&Proof).unwrap();
    assert!(
        matches!(
            permit.request(Command::Undo).unwrap_err().error,
            Error::MutationRefused(CoreMutationRefusal::Frozen)
        ),
        "FREEZE_FENCES_CURRENT_PERMIT"
    );
    assert_eq!(
        pending_receipts(&permit),
        receipts,
        "FREEZE_PRESERVES_ORIGINAL_PENDING_IDENTITIES"
    );
    attachment
        .acknowledge_manual(&Proof, &permit, receipts)
        .unwrap();
    let replacement = attachment
        .manual_permit(
            &Proof,
            Some(cut),
            producer_lease(&core, origin.branch, 2),
            crate::operation_inputs::ManualInputs::fixture(),
            Arc::clone(&live),
        )
        .unwrap();
    assert!(
        matches!(
            permit.request(Command::Undo).unwrap_err().error,
            Error::MutationRefused(CoreMutationRefusal::ManualPermitRequired)
        ),
        "REBIND_INVALIDATES_EVERY_OLD_PERMIT"
    );
    replacement.request(Command::Undo).unwrap();
    live.store(false, Ordering::Release);
    assert!(
        matches!(
            replacement.request(Command::Redo).unwrap_err().error,
            Error::MutationRefused(CoreMutationRefusal::ManualPermitRequired)
        ),
        "REVOKED_SOURCE_CANNOT_ACCEPT_ANOTHER_EDIT"
    );
    assert_eq!(
        replacement.pending().unwrap().len(),
        1,
        "REVOKED_SOURCE_RETAINS_RECOVERABLE_WORK"
    );
    assert!(!locked(&core));
}
#[test]
fn manual_actor_records_only_normalised_successes_and_explicit_close() {
    let pool = HistoryPool::new(PoolLimits::default());
    let origin = origin();
    let core = Core::spawn(&pool, Role::Ordinary, origin.clone(), document()).unwrap();
    let attachment = core.attach(&Proof).unwrap();
    let permit = attachment
        .manual_permit(
            &Proof,
            None,
            producer_lease(&core, origin.branch, 1),
            crate::operation_inputs::ManualInputs::fixture(),
            Arc::new(AtomicBool::new(true)),
        )
        .unwrap();
    let before = core.change_id().unwrap();
    assert!(matches!(
        permit
            .request(Command::DoIfRevision {
                expected: TimelineRevision(17),
                operation: lock(true),
                token: None
            })
            .unwrap(),
        Event::RevisionConflict { .. }
    ));
    assert!(matches!(
        permit
            .request(Command::Do(Operation::RemoveTrack {
                track: TrackId(404)
            }))
            .unwrap(),
        Event::OpRejected { .. }
    ));
    assert_eq!(
        core.change_id().unwrap(),
        before,
        "REFUSED_PRODUCER_EDITS_ADVANCE_NO_COUNTER"
    );
    assert!(
        permit.pending().unwrap().is_empty(),
        "REFUSED_PRODUCER_EDITS_ENTER_NO_OUTBOX"
    );
    permit
        .request(Command::DoIfRevision {
            expected: TimelineRevision(0),
            operation: lock(true),
            token: Some(CommandToken(55)),
        })
        .unwrap();
    permit.close_gesture().unwrap();
    let pending = permit.pending().unwrap();
    assert_eq!(pending.len(), 2);
    assert!(
        matches!(
            pending[0].decode().unwrap().action,
            crate::operation_protocol::Action::Do { .. }
        ),
        "ACCEPTED_CONDITIONAL_IS_RECORDED_AS_PLAIN_DO"
    );
    assert!(
        matches!(
            pending[1].decode().unwrap().action,
            crate::operation_protocol::Action::Close
        ),
        "GESTURE_CLOSE_IS_AN_EXPLICIT_ORDERED_MESSAGE"
    );
    assert_eq!(pending[1].decode().unwrap().accepted.position, 1);
}

#[test]
fn manual_warm_edit_and_full_outbox_keep_the_actor_atomic() {
    let pool = HistoryPool::new(PoolLimits::default());
    let origin = origin();
    let core = Core::spawn(&pool, Role::Ordinary, origin.clone(), document()).unwrap();
    let attachment = core.attach(&Proof).unwrap();
    let permit = attachment
        .manual_permit(
            &Proof,
            None,
            producer_lease(&core, origin.branch, 1),
            crate::operation_inputs::ManualInputs::fixture(),
            Arc::new(AtomicBool::new(true)),
        )
        .unwrap();
    let before = core.counters().unwrap();
    permit.request(Command::Do(lock(true))).unwrap();
    let after = core.counters().unwrap();
    let kernel = after.0.since(before.0);
    let owner = after.1.since(before.1);
    assert_eq!(
        kernel.document_copies, 1,
        "PERMITTED_WARM_EDIT_COPIES_ONE_DOCUMENT"
    );
    assert_eq!(
        kernel.exact_measurements, 0,
        "PERMITTED_WARM_EDIT_MEASURES_NO_DOCUMENT"
    );
    assert_eq!(
        owner.owner_identity_hashes + owner.document_scans + owner.seed_map_copies,
        0,
        "PERMITTED_WARM_EDIT_HAS_NO_OWNER_WHOLE_STATE_WORK"
    );
    for index in 1..crate::operation_protocol::PENDING_COMMANDS {
        permit
            .request(Command::Do(lock(index.is_multiple_of(2))))
            .unwrap();
    }
    let cut = core.change_id().unwrap();
    let token = Some(CommandToken(29));
    assert_eq!(
        permit
            .request(Command::DoIfRevision {
                expected: cut.revision,
                operation: lock(false),
                token
            })
            .unwrap_err(),
        Refusal {
            error: Error::Protocol(crate::operation_protocol::Conflict::Backpressure),
            token,
        },
        "OUTBOX_CAPACITY_REFUSAL_RETAINS_CORRELATION_TOKEN"
    );
    assert!(
        matches!(
            permit.request(Command::DoBatch(vec![])).unwrap(),
            Event::BatchRejected {
                error: kinewright_core::BatchError::Empty,
                ..
            }
        ),
        "EMPTY_BATCH_PRECEDES_FULL_OUTBOX"
    );
    assert_eq!(
        core.change_id().unwrap(),
        cut,
        "FULL_OUTBOX_REFUSAL_CHANGES_NO_ACTOR_STATE"
    );
    assert_eq!(
        permit.pending().unwrap().len(),
        crate::operation_protocol::PENDING_COMMANDS
    );
}

#[test]
fn manual_input_admission_is_atomic_after_kernel_validation_and_keeps_external_locators() {
    use kinewright_core::{
        AssetId, MediaAsset, MediaKind, MediaSourceFingerprint, Rational, TimeCode,
    };
    let pool = HistoryPool::new(PoolLimits::default());
    let origin = origin();
    let core = Core::spawn(&pool, Role::Ordinary, origin.clone(), document()).unwrap();
    let attachment = core.attach(&Proof).unwrap();
    let permit = attachment
        .manual_permit(
            &Proof,
            None,
            producer_lease(&core, origin.branch, 1),
            crate::operation_inputs::ManualInputs::fixture(),
            Arc::new(AtomicBool::new(true)),
        )
        .unwrap();
    let owned = || MediaAsset {
        id: AssetId(2),
        path: "external.mp4".into(),
        name: "test".into(),
        duration: TimeCode(24),
        fps: Rational::new(24, 1).unwrap(),
        kind: MediaKind::Video,
        resolution: Some((1920, 1080)),
        source_fingerprint: MediaSourceFingerprint {
            content_sha256: Some("a".repeat(64)),
            byte_len: Some(100),
        },
        color_description: kinewright_core::ColorDescription::default(),
        assumed_from: None,
    };
    let before = core.change_id().unwrap();
    let before_identity = core.export_history().unwrap().identity().unwrap();
    assert_eq!(
        permit
            .request(Command::Do(Operation::AddAsset { asset: owned() }))
            .unwrap_err()
            .error,
        Error::MutationRefused(CoreMutationRefusal::ManualInputRequired),
        "UNCERTIFIED_OWNED_PAYLOAD_IS_REFUSED_BEFORE_ACCEPTANCE"
    );
    assert_eq!(
        core.change_id().unwrap(),
        before,
        "INPUT_REFUSAL_PRESERVES_COMMAND_AND_REVISION_COUNTERS"
    );
    assert_eq!(
        core.export_history().unwrap().identity().unwrap(),
        before_identity,
        "INPUT_REFUSAL_PRESERVES_COMPLETE_HISTORY_AND_OWNER"
    );
    assert!(
        permit.pending().unwrap().is_empty(),
        "INPUT_REFUSAL_PRESERVES_THE_OUTBOX"
    );
    let mut invalid = owned();
    invalid.source_fingerprint.content_sha256 = Some("not-a-digest".into());
    assert!(
        matches!(
            permit
                .request(Command::Do(Operation::AddAsset { asset: invalid }))
                .unwrap(),
            Event::OpRejected { .. }
        ),
        "KERNEL_REFUSAL_PRECEDES_SOURCE_INPUT_ADMISSION"
    );
    let mut external = owned();
    external.source_fingerprint = MediaSourceFingerprint::default();
    assert!(
        matches!(
            permit
                .request(Command::Do(Operation::AddAsset { asset: external }))
                .unwrap(),
            Event::DocumentChanged { .. }
        ),
        "UNHASHED_MEDIA_REMAINS_AN_EXTERNAL_LOCATOR"
    );
    let accepted = permit.pending().unwrap().remove(0).decode().unwrap();
    assert_eq!(
        accepted.context.inputs,
        vec![
            crate::operation_inputs::ManualInputs::fixture()
                .reference()
                .clone()
        ],
        "ACCEPTED_COMMAND_OWNS_ITS_CERTIFIED_INPUT_CONTEXT"
    );
}

#[test]
fn an_install_requires_draining_the_original_outbox_and_rebinding_its_new_epoch() {
    let pool = HistoryPool::new(PoolLimits::default());
    let origin = origin();
    let core = Core::spawn(&pool, Role::Ordinary, origin.clone(), document()).unwrap();
    let attachment = core.attach(&Proof).unwrap();
    let permit = attachment
        .manual_permit(
            &Proof,
            None,
            producer_lease(&core, origin.branch.clone(), 1),
            crate::operation_inputs::ManualInputs::fixture(),
            Arc::new(AtomicBool::new(true)),
        )
        .unwrap();
    permit.request(Command::Do(lock(true))).unwrap();
    let cut = attachment.freeze(&Proof).unwrap();
    let selected = core.export_history().unwrap().validate().unwrap();
    assert_eq!(
        attachment.install_snapshot(&Proof, cut, selected),
        Err(Error::Protocol(
            crate::operation_protocol::Conflict::Backpressure
        )),
        "INSTALL_CANNOT_ABANDON_AN_UNACKNOWLEDGED_ORIGINAL"
    );
    assert_eq!(core.change_id().unwrap(), cut.change());
    assert_eq!(permit.pending().unwrap().len(), 1);
    attachment
        .acknowledge_manual(&Proof, &permit, pending_receipts(&permit))
        .unwrap();
    let selected = core.export_history().unwrap().validate().unwrap();
    attachment.install_snapshot(&Proof, cut, selected).unwrap();
    let installed = attachment.freeze(&Proof).unwrap();
    assert_eq!(
        attachment.resume(&Proof, installed),
        Err(Error::Protocol(
            crate::operation_protocol::Conflict::AcceptedKey
        )),
        "INSTALL_CANNOT_RESUME_THE_PREVIOUS_EPOCH_STREAM"
    );
    let replacement = attachment
        .manual_permit(
            &Proof,
            Some(installed),
            producer_lease(&core, origin.branch, 2),
            crate::operation_inputs::ManualInputs::fixture(),
            Arc::new(AtomicBool::new(true)),
        )
        .unwrap();
    replacement.request(Command::Undo).unwrap();
    let message = replacement.pending().unwrap().remove(0).decode().unwrap();
    assert_eq!(
        message.sequence, 1,
        "INSTALL_RESTARTS_THE_ACCEPTED_COMMAND_SEQUENCE"
    );
    assert_eq!(
        message.accepted.epoch, 1,
        "INSTALL_RESTARTS_THE_CORE_ACCEPTANCE_EPOCH"
    );
    assert!(!locked(&core));
}

#[cfg(target_os = "linux")]
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one store admission, permitted edit and cold-open ownership sequence"
)]
fn a_store_admitted_payload_survives_the_permitted_actor_and_cold_record_open() {
    use crate::{
        bundle_copy::SessionStore,
        bundle_records::RecordKind,
        operation_history::{Resource, StoredOwnerRecord},
        operation_inputs::ManualInputs,
        owned_inputs::{OwnedInputEntry, OwnedInputManifest},
        storage_budget::StorageLimits,
    };
    use kinewright_core::{LutAsset, LutAssetId, LutAssetKind, LutAssetSource};
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "hx-admitted-source-input",
    );
    let mut session = SessionStore::create(temp.root(), StorageLimits::default()).unwrap();
    let bytes = b"owned LUT bytes";
    let resource = ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::OwnedLut,
    };
    let _blob = session
        .store_mut()
        .admit_blob(&mut bytes.as_slice(), &resource, &[])
        .unwrap();
    let manifest = OwnedInputManifest {
        project_id: None,
        session_id: Some(session.session_id().clone()),
        owner_epoch: session.store().owner_epoch(),
        entries: vec![OwnedInputEntry {
            entry_id: "lut:1".into(),
            object: resource.clone(),
            dependencies: vec![],
            selected_stream: None,
            time_base: None,
            origin: None,
            profile: None,
        }],
    };
    let context = session.admit_inputs(&manifest).unwrap();
    let certificate = ManualInputs::read(session.store(), &context.reference).unwrap();
    let pool = HistoryPool::new(PoolLimits::default());
    let origin = origin();
    let core = Core::spawn(&pool, Role::Ordinary, origin.clone(), document()).unwrap();
    let attachment = core.attach(&Proof).unwrap();
    let mut lease = producer_lease(&core, origin.branch, 1);
    lease.inputs = context.reference.clone();
    let permit = attachment
        .manual_permit(
            &Proof,
            None,
            lease,
            certificate,
            Arc::new(AtomicBool::new(true)),
        )
        .unwrap();
    let asset = LutAsset {
        id: LutAssetId(1),
        sha256: resource.sha256.clone(),
        title: "owned LUT".into(),
        kind: LutAssetKind::Cube3d,
        size: 2,
        byte_len: resource.byte_length,
        domain_min_millionths: [0; 3],
        domain_max_millionths: [1_000_000; 3],
        source: LutAssetSource::Imported {
            source_path: "owned.cube".into(),
        },
    };
    assert!(
        matches!(
            permit
                .request(Command::Do(Operation::AddLutAsset { asset }))
                .unwrap(),
            Event::DocumentChanged { .. }
        ),
        "CERTIFIED_PAYLOAD_IS_ACCEPTED_THROUGH_THE_ACTOR_PERMIT"
    );
    permit.request(Command::Undo).unwrap();
    let capture = core.export_history().unwrap();
    let identity = capture.identity().unwrap();
    let record = capture
        .persist(session.store_mut(), |r| {
            Ok(matches!(r, Resource::Lut { .. }).then(|| context.reference.clone()))
        })
        .unwrap();
    drop(capture);
    let reopened = StoredOwnerRecord::read(
        session.store(),
        &record.reference,
        &HistoryPool::new(PoolLimits::default()),
        Role::Ordinary,
    )
    .unwrap()
    .open()
    .unwrap();
    assert_eq!(
        reopened.identity().unwrap(),
        identity,
        "ADMITTED_SOURCE_OWNER_IDENTITY_SURVIVES_COLD_OPEN"
    );
    assert_eq!(
        reopened.history().redo().len(),
        1,
        "ADMITTED_REDO_PAYLOAD_SURVIVES_COLD_OPEN"
    );
    let closure = session
        .store()
        .closure(std::slice::from_ref(&record.reference))
        .unwrap();
    assert!(
        closure.contains(&resource) && closure.contains(&context.reference),
        "ADMITTED_REDO_PAYLOAD_RETAINS_ITS_MANIFEST_AND_BYTES"
    );
}

#[test]
fn a_peer_actor_adopts_the_selected_epoch_at_its_own_frozen_cut() {
    let pool = HistoryPool::new(PoolLimits::default());
    let origin = origin();
    let source = Core::spawn(&pool, Role::Task, origin.clone(), document()).unwrap();
    source.request(Command::Do(lock(true))).unwrap();
    let before = source.change_id().unwrap();
    let peer = Core::open(
        source.export_history().unwrap().validate().unwrap(),
        origin.branch,
        before.revision,
    )
    .unwrap();
    assert_eq!(
        peer.change_id().unwrap().revision,
        before.revision,
        "COLD_ACTOR_OPENS_AT_ITS_SAVED_REVISION"
    );
    let source_attachment = source.attach(&Proof).unwrap();
    let source_cut = source_attachment.freeze(&Proof).unwrap();
    source_attachment
        .install_snapshot(
            &Proof,
            source_cut,
            source.export_history().unwrap().validate().unwrap(),
        )
        .unwrap();
    let selected_epoch = source.change_id().unwrap().position.epoch;
    let peer_attachment = peer.attach(&Proof).unwrap();
    let peer_cut = peer_attachment.freeze(&Proof).unwrap();
    peer_attachment
        .install_snapshot_at_epoch(
            &Proof,
            peer_cut,
            source.export_history().unwrap().validate().unwrap(),
            selected_epoch,
        )
        .unwrap();
    let source_identity = source.export_history().unwrap().identity().unwrap();
    let peer_identity = peer.export_history().unwrap().identity().unwrap();
    assert_eq!(
        peer_identity, source_identity,
        "ACTOR_FORWARDING_USES_THE_SAME_COMBINED_SNAPSHOT_IDENTITY"
    );
    assert_eq!(
        peer.change_id().unwrap().position,
        source.change_id().unwrap().position,
        "ACTOR_FORWARDING_STARTS_EXACTLY_ONE_SHARED_EPOCH"
    );
    assert_eq!(
        peer.change_id().unwrap().revision,
        source.change_id().unwrap().revision,
        "ACTOR_FORWARDING_PRESERVES_REPLICA_REVISION_AGREEMENT"
    );
    assert_eq!(
        peer_attachment.resume(&Proof, peer_cut),
        Err(Error::CaptureChanged),
        "PEER_INSTALL_INVALIDATES_ITS_OLD_FROZEN_CUT"
    );
}

#[test]
fn captured_state_pairs_its_revision_and_position_with_the_same_immutable_record() {
    let pool = HistoryPool::new(PoolLimits::default());
    let core = small(&pool, Role::Ordinary);
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let writer = core.clone();
    let writer_barrier = Arc::clone(&barrier);
    let worker = std::thread::spawn(move || {
        for index in 0..64 {
            writer_barrier.wait();
            assert!(matches!(
                writer.request(Command::Do(lock(index % 2 == 0))).unwrap(),
                Event::DocumentChanged { .. }
            ));
            writer_barrier.wait();
        }
    });
    for round in 0..64 {
        barrier.wait();
        let captured = core.capture_state().unwrap();
        let record = captured.snapshot.history();
        assert_eq!(
            captured.change.position.position, record.position,
            "CAPTURE_IDENTIFIER_NAMES_ITS_EXACT_IMMUTABLE_RECORD"
        );
        assert_eq!(captured.change.position.epoch, record.epoch);
        assert_eq!(
            captured.change.revision.0, record.position,
            "CAPTURE_REVISION_AND_RECORD_HAVE_ONE_ACTOR_CUT"
        );
        assert!((round..=round + 1).contains(&record.position));
        assert_eq!(
            record.document.tracks[0].sync_lock,
            record.position % 2 == 1,
            "CAPTURE_DOCUMENT_MATCHES_ITS_ACCEPTED_POSITION"
        );
        barrier.wait();
    }
    worker.join().unwrap();
    assert_eq!(core.change_id().unwrap().position.position, 64);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one approved command and Undo lifecycle through discard and retirement"
)]
fn audit_answers_read_command_fate_without_holding_a_task_history() {
    use crate::{
        bundle_records::RecordKind,
        operation_history::{ApprovalAnswer, Direction, RemovedTarget},
    };
    let pool = HistoryPool::new(PoolLimits::default());
    let origin = origin();
    let core = Core::spawn(&pool, Role::Task, origin.clone(), document()).unwrap();
    let do_ref = ObjectRef {
        sha256: "a".repeat(64),
        byte_length: 1,
        record_kind: RecordKind::OwnedEvidence,
    };
    let undo_ref = ObjectRef {
        sha256: "b".repeat(64),
        byte_length: 1,
        record_kind: RecordKind::OwnedEvidence,
    };
    let context = |approval, transition| CommandContext {
        creation: origin.clone(),
        inputs: vec![],
        approval,
        transition,
    };
    core.request_with_context(
        Command::Do(Operation::RemoveTrack { track: TrackId(1) }),
        context(
            Some(Approval {
                reference: do_ref.clone(),
                targets: vec![RemovedTarget::Track(TrackId(1))],
            }),
            None,
        ),
    )
    .unwrap();
    assert_eq!(
        core.approval_answer(do_ref.clone()).unwrap(),
        Some(ApprovalAnswer::Kept)
    );
    let target = core
        .export_history()
        .unwrap()
        .history()
        .undo
        .back()
        .unwrap()
        .evidence()
        .unwrap();
    core.request_with_context(
        Command::Undo,
        context(
            None,
            Some(TransitionApproval {
                id: undo_ref.clone(),
                direction: Direction::Undo,
                target,
            }),
        ),
    )
    .unwrap();
    assert_eq!(
        core.approval_answer(do_ref.clone()).unwrap(),
        Some(ApprovalAnswer::Missing)
    );
    assert_eq!(
        core.approval_answer(undo_ref.clone()).unwrap(),
        Some(ApprovalAnswer::Kept)
    );
    core.request(Command::Redo).unwrap();
    assert_eq!(
        core.approval_answer(do_ref.clone()).unwrap(),
        Some(ApprovalAnswer::Kept)
    );
    assert_eq!(
        core.approval_answer(undo_ref.clone()).unwrap(),
        Some(ApprovalAnswer::Missing)
    );
    core.request(Command::Undo).unwrap();
    core.request(Command::Do(Operation::RemoveTrack { track: TrackId(1) }))
        .unwrap();
    let before = core.counters().unwrap();
    let accounting = pool.usage(Role::Task).unwrap();
    assert_eq!(
        core.approval_answer(do_ref).unwrap(),
        Some(ApprovalAnswer::Missing),
        "AUDIT_QUERY_NEVER_CREDITS_A_DIFFERENT_REMOVAL"
    );
    let held_answer = core.approval_answer(undo_ref).unwrap();
    assert_eq!(
        held_answer,
        Some(ApprovalAnswer::Kept),
        "APPROVED_UNDO_HOLDS_AFTER_ITS_REDO_TARGET_IS_DROPPED"
    );
    assert_eq!(
        pool.usage(Role::Task).unwrap(),
        accounting,
        "AUDIT_QUERY_RETAINS_NO_INDEPENDENT_HISTORY_LEASE"
    );
    let after = core.counters().unwrap();
    let kernel = after.0.since(before.0);
    let owner = after.1.since(before.1);
    assert_eq!(
        (
            kernel.document_copies,
            kernel.exact_measurements,
            owner.owner_identity_hashes,
            owner.document_scans,
            owner.seed_map_copies
        ),
        (0, 0, 0, 0, 0),
        "AUDIT_QUERY_COPIES_AND_HASHES_NO_DOCUMENT_OR_OWNER"
    );
    let attachment = core.attach(&Proof).unwrap();
    let cut = attachment.freeze(&Proof).unwrap();
    attachment.retire(&Proof, cut).unwrap();
    assert_eq!(held_answer, Some(ApprovalAnswer::Kept));
    assert_eq!(
        pool.usage(Role::Task).unwrap(),
        (0, 0),
        "FIXED_AUDIT_ANSWER_DOES_NOT_DELAY_TASK_RELEASE"
    );
}

#[test]
fn delta_fork_preserves_the_current_occurrences_without_a_parent_capture() {
    let pool = HistoryPool::new(PoolLimits::default());
    let mut cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../kinewright-core/tests/support/history_operation_cases.json"
    ))
    .unwrap();
    let document: Document = serde_json::from_value(cases["SplitClip"]["document"].take()).unwrap();
    let operation: Operation =
        serde_json::from_value(cases["SplitClip"]["operation"].take()).unwrap();
    let parent = Core::spawn(&pool, Role::Ordinary, origin(), document).unwrap();
    parent.request(Command::Do(operation)).unwrap();
    let boundary = parent.boundary(1).unwrap();
    let expected = serde_json::to_value(boundary.lineage()).unwrap();
    let expected_document = serde_json::to_value(boundary.document()).unwrap();
    // Leave the existing command applied and an unrelated accepted Close in
    // the parent's counter. Neither parent's history nor counter is inherited.
    parent.close_gesture().unwrap();
    let before = parent.change_id().unwrap();
    let child_origin = origin();
    let child_branch = child_origin.branch.clone();
    let (child, revision) = parent.fork(Role::Task, child_origin, false).unwrap();
    assert_eq!(
        revision, before.revision,
        "FORK_OBSERVES_ONE_PARENT_REVISION"
    );
    assert_eq!(
        parent.change_id().unwrap(),
        before,
        "FORK_LEAVES_PARENT_STATE_UNCHANGED"
    );
    assert_eq!(
        pool.lease_counts(Role::Ordinary).unwrap(),
        (2, 2),
        "FORK_DOES_NOT_TAKE_AN_EXTRA_ORDINARY_CAPTURE"
    );
    drop(boundary);
    let snapshot = child.export_history().unwrap();
    assert_eq!(
        serde_json::to_value(snapshot.seeds()).unwrap(),
        expected,
        "DELTA_FORK_PRESERVES_PARENT_OCCURRENCE_IDENTITIES"
    );
    assert_eq!(
        serde_json::to_value(&snapshot.history().document).unwrap(),
        expected_document,
        "DELTA_FORK_BASELINE_IS_CURRENT_PARENT_DOCUMENT"
    );
    assert!(
        snapshot.history().undo.is_empty() && snapshot.history().redo.is_empty(),
        "DELTA_FORK_STARTS_WITH_EMPTY_STACKS"
    );
    assert_eq!(child.change_id().unwrap().revision, TimelineRevision(0));
    drop(snapshot);
    let track = match child.request(Command::Query(Query::Document)).unwrap() {
        Event::QueryResult(QueryResult::Document(document)) => document.tracks[0].id,
        _ => unreachable!(),
    };
    child
        .request(Command::Do(Operation::SetTrackSyncLock {
            track,
            locked: true,
        }))
        .unwrap();
    assert_eq!(
        child.export_history().unwrap().origins()[0].creation.branch,
        child_branch,
        "FORK_COMMANDS_BELONG_TO_THE_CHILD_BRANCH"
    );
}

#[test]
fn attachment_capture_is_a_fixed_cut_and_shares_the_actor_identity_domain() {
    let pool = HistoryPool::new(PoolLimits::default());
    let parent = Core::spawn(&pool, Role::Ordinary, origin(), document()).unwrap();
    let old_proof_actor = kinewright_core::Core::spawn(document()).unwrap();
    assert_ne!(
        parent.actor_identity(),
        old_proof_actor.actor_identity(),
        "ACTOR_IDENTITIES_CANNOT_COLLIDE_DURING_THE_CUTOVER"
    );
    let owner = parent.attach(&Proof).unwrap();
    let cut = owner.capture(&Proof).unwrap();
    assert_eq!(cut.change(), parent.change_id().unwrap());
    assert_eq!(parent.attachment_state().unwrap(), AttachmentState::Live);
    assert_eq!(
        pool.lease_counts(Role::Ordinary).unwrap(),
        (1, 1),
        "ATTACHMENT_CAPTURE_RETAINS_NO_HISTORY_LEASE"
    );
    let frozen = owner.freeze(&Proof).unwrap();
    assert_eq!(cut, frozen);
    assert_eq!(parent.attachment_state().unwrap(), AttachmentState::Frozen);
    owner.resume(&Proof, frozen).unwrap();
}
