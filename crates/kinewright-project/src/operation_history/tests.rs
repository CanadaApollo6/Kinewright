use super::*;
use kinewright_core::{
    Command, Document, Operation, Track, TrackId, TrackKind,
    undo::{History, PhaseCounters, RetentionPolicy, RoleBounds},
};

pub(super) fn origin(value: u64) -> CreationOrigin {
    CreationOrigin {
        branch: serde_json::from_str(&format!("\"{:032x}\"", 1)).unwrap(),
        batch: serde_json::from_str(&format!("\"{value:032x}\"")).unwrap(),
    }
}
pub(super) fn document() -> Document {
    Document {
        tracks: vec![Track {
            id: TrackId(81),
            kind: TrackKind::Video,
            sync_lock: false,
            clips: vec![],
        }],
        ..Document::default()
    }
}
fn owner() -> OwnerHistory {
    OwnerHistory::new(
        &HistoryPool::new(PoolLimits::default()),
        Role::Task,
        RetentionPolicy::NeverFold,
        &origin(1),
        || Ok(document()),
    )
    .unwrap()
}
pub(super) fn remove() -> Command {
    Command::DoBatch(vec![Operation::RemoveTrack { track: TrackId(81) }])
}
pub(super) fn approval() -> Approval {
    Approval {
        reference: crate::bundle_records::ObjectRef {
            sha256: "a".repeat(64),
            byte_length: 1,
            record_kind: crate::bundle_records::RecordKind::OwnedEvidence,
        },
        targets: vec![RemovedTarget::Track(TrackId(81))],
    }
}
#[test]
fn approval_fate_never_uses_a_later_removal() {
    let mut history = owner();
    let first = history
        .execute(remove(), origin(2), vec![], Some(approval()))
        .unwrap()
        .accepted;
    history
        .execute(Command::Undo, origin(3), vec![], None)
        .unwrap();
    assert_eq!(
        history.ledger().get(first).unwrap().fate,
        Fate::Undone,
        "APPROVAL_UNDONE"
    );
    history
        .execute(Command::Redo, origin(4), vec![], None)
        .unwrap();
    assert_eq!(
        history.ledger().get(first).unwrap().fate,
        Fate::Redone,
        "APPROVAL_REDONE"
    );
    history
        .execute(Command::Undo, origin(5), vec![], None)
        .unwrap();
    let second = history
        .execute(remove(), origin(6), vec![], Some(approval()))
        .unwrap()
        .accepted;
    assert_eq!(
        history.ledger().get(first).unwrap().fate,
        Fate::Discarded,
        "DISCARDED_APPROVAL_NEVER_KEPT"
    );
    assert_eq!(
        history.ledger().get(first).unwrap().answer(),
        ApprovalAnswer::Missing,
        "DISCARDED_APPROVAL_ANSWER"
    );
    assert_eq!(
        history.ledger().get(second).unwrap().answer(),
        ApprovalAnswer::Kept
    );
}
#[test]
fn coalesced_members_follow_undo_redo_and_discard() {
    let mut history = owner();
    history
        .execute(
            Command::Do(Operation::AddTrack {
                track: Track {
                    id: TrackId(82),
                    kind: TrackKind::Video,
                    sync_lock: false,
                    clips: vec![],
                },
            }),
            origin(2),
            vec![],
            None,
        )
        .unwrap();
    let mut keys = vec![];
    for id in [81, 82] {
        let mut approved = approval();
        approved.targets = vec![RemovedTarget::Track(TrackId(id))];
        keys.push(
            history
                .execute(
                    Command::DoBatchCoalesced {
                        operations: vec![Operation::RemoveTrack { track: TrackId(id) }],
                        coalesce_key: "remove".into(),
                    },
                    origin(id),
                    vec![],
                    Some(approved),
                )
                .unwrap()
                .accepted,
        );
    }
    for (command, fate) in [
        (Command::Undo, Fate::Undone),
        (Command::Redo, Fate::Redone),
        (Command::Undo, Fate::Undone),
    ] {
        history.execute(command, origin(100), vec![], None).unwrap();
        for key in &keys {
            assert_eq!(
                history.ledger().get(*key).unwrap().fate,
                fate,
                "EVERY_COALESCED_APPROVAL_FATE"
            );
        }
    }
    history
        .execute(
            Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(81),
                locked: true,
            }),
            origin(101),
            vec![],
            None,
        )
        .unwrap();
    for key in keys {
        assert_eq!(
            history.ledger().get(key).unwrap().fate,
            Fate::Discarded,
            "EVERY_COALESCED_APPROVAL_DISCARDED"
        );
    }
}
#[test]
fn owner_refusal_is_inside_the_document_transaction() {
    let mut history = owner();
    let before = history.history().snapshot_identity().unwrap();
    history.owner_bytes = Role::Task.envelope().owners;
    assert_eq!(
        history.execute(
            Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(81),
                locked: true
            }),
            origin(2),
            vec![],
            None
        ),
        Err(Error::Capacity("owner metadata")),
        "OWNER_TYPED_REFUSAL"
    );
    assert_eq!(
        history.history().snapshot_identity().unwrap(),
        before,
        "OWNER_REFUSAL_ATOMIC"
    );
    assert!(history.origins.0.is_empty());
}
#[test]
fn warm_owner_edit_has_no_document_or_exact_work() {
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../kinewright-core/tests/support/history_operation_cases.json"
    ))
    .unwrap();
    let document: Document =
        serde_json::from_value(cases["DeleteClip"]["document"].clone()).unwrap();
    let track = document.tracks[0].id;
    let clip = document.tracks[0].clips[0].id;
    let mut history = OwnerHistory::new(
        &HistoryPool::new(PoolLimits::default()),
        Role::Ordinary,
        RetentionPolicy::Fold,
        &origin(1),
        || Ok(document),
    )
    .unwrap();
    let mut approved = approval();
    approved.targets = vec![RemovedTarget::Clip(clip)];
    history
        .execute(
            Command::DoBatch(vec![Operation::DeleteClip { clip }]),
            origin(2),
            vec![],
            Some(approved),
        )
        .unwrap();
    history
        .execute(Command::Undo, origin(3), vec![], None)
        .unwrap();
    history
        .execute(
            Command::Do(Operation::SetTrackSyncLock {
                track,
                locked: true,
            }),
            origin(4),
            vec![],
            None,
        )
        .unwrap();
    assert!(
        history.ledger().entries().count() > 0
            && !history.seeds().occurrences.is_empty()
            && history.resources().values().count() > 0,
        "WARM_OWNER_NONEMPTY_ATTACHMENTS"
    );
    for n in 0..64 {
        let owner_before = OwnerCounters::read();
        let core_before = PhaseCounters::read();
        history
            .execute(
                Command::Do(Operation::SetTrackSyncLock {
                    track,
                    locked: n % 2 == 0,
                }),
                origin(5 + n),
                vec![],
                None,
            )
            .unwrap();
        let owner = OwnerCounters::read().since(owner_before);
        let core = PhaseCounters::read().since(core_before);
        assert_eq!(owner.document_scans, 0, "NO_WARM_OWNER_DOCUMENT_WORK");
        assert_eq!(
            owner.exact_measurements, 0,
            "NO_WARM_OWNER_EXACT_MEASUREMENT"
        );
        assert_eq!(core.document_copies, 1, "ONE_OWNER_ATOMIC_DOCUMENT_COPY");
        assert_eq!(
            core.exact_measurements, 0,
            "NO_OWNER_CORE_EXACT_MEASUREMENT"
        );
    }
}
#[test]
fn role_reservations_are_atomic_and_released() {
    let envelope = Role::Task.envelope();
    let pool = HistoryPool::new(PoolLimits {
        retained: [0, envelope.retained, 0],
        peak: [0, envelope.retained + envelope.workspace, 0],
        workspace: envelope.workspace,
    });
    let mut reservation = pool.reserve(Role::Task, envelope.retained - 1).unwrap();
    reservation.grow_to(envelope.retained).unwrap();
    assert_eq!(
        pool.usage(Role::Task).unwrap(),
        (envelope.retained, 0),
        "RESERVATION_EXACT_FIT"
    );
    assert!(
        matches!(pool.reserve(Role::Task, 1), Err(Error::Reservation)),
        "RESERVATION_ONE_BYTE_OVER"
    );
    assert_eq!(
        pool.usage(Role::Task).unwrap(),
        (envelope.retained, 0),
        "NO_PARTIAL_RESERVATION"
    );
    {
        let _workspace = reservation.workspace().unwrap();
        assert!(
            matches!(reservation.workspace(), Err(Error::Reservation)),
            "WORKSPACE_SUM_BOUND"
        );
    }
    drop(reservation);
    assert_eq!(
        pool.usage(Role::Task).unwrap(),
        (0, 0),
        "HISTORY_DROP_RELEASES_RESERVATION"
    );
    let mut called = false;
    let empty = HistoryPool::new(PoolLimits {
        retained: [0; 3],
        peak: [0; 3],
        workspace: 0,
    });
    assert!(matches!(
        OwnerHistory::new(
            &empty,
            Role::Task,
            RetentionPolicy::NeverFold,
            &origin(1),
            || {
                called = true;
                Ok(document())
            }
        ),
        Err(Error::Reservation)
    ));
    assert!(!called, "REFUSAL_PRECEDES_DOCUMENT_ALLOCATION");
}
#[test]
fn candidate_workspace_and_shared_peak_have_their_own_arithmetic() {
    let role = Role::Candidate.envelope();
    assert_eq!(role.retained, 2 << 20, "CANDIDATE_RETAINED_ENVELOPE");
    assert_eq!(role.workspace, 16 << 20, "CANDIDATE_WORKSPACE_ENVELOPE");
    let max_ops = (512_u64 << 10) / std::mem::size_of::<Operation>() as u64;
    let observations = max_ops
        * 4
        * (std::mem::size_of::<kinewright_core::OperationIdentityEffect>() as u64
            + std::mem::size_of::<kinewright_core::undo::CommandEffects>() as u64
            + 4 * 16);
    // Eight D kernel workspace, O+2A+8 KiB stored input, two A owner scratch,
    // inline observations and 64 KiB fixed headers; leave ten percent slack.
    let subtotal =
        (4 << 20) + Role::Candidate.persisted_shell_cap() + (1 << 20) + observations + (64 << 10);
    assert!(
        subtotal * 11 / 10 < role.workspace,
        "CANDIDATE_WORKSPACE_ARITHMETIC"
    );
    let pool = HistoryPool::new(PoolLimits::default());
    let task = pool.admit(Role::Task).unwrap();
    let candidate = pool.admit(Role::Candidate).unwrap();
    let window = task.workspace().unwrap();
    assert!(
        matches!(candidate.workspace(), Err(Error::Reservation)),
        "SHARED_WORKSPACE_WINDOW"
    );
    assert_eq!(
        pool.usage(Role::Candidate).unwrap(),
        (role.retained, 0),
        "SHARED_WINDOW_REFUSAL_ATOMIC"
    );
    drop(window);
    assert!(candidate.workspace().is_ok());
}
#[test]
fn folding_advances_seeds_and_resources() {
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../kinewright-core/tests/support/history_operation_cases.json"
    ))
    .unwrap();
    let document: Document =
        serde_json::from_value(cases["SplitClip"]["document"].clone()).unwrap();
    let operation: Operation =
        serde_json::from_value(cases["SplitClip"]["operation"].clone()).unwrap();
    let mut owner = OwnerHistory::new(
        &HistoryPool::new(PoolLimits::default()),
        Role::Ordinary,
        RetentionPolicy::Fold,
        &origin(1),
        || Ok(document.clone()),
    )
    .unwrap();
    owner.core = History::new(
        document,
        RetentionPolicy::Fold,
        RoleBounds {
            step_cap: 1,
            ..RoleBounds::ORDINARY
        },
    )
    .unwrap();
    let first = owner
        .execute(Command::Do(operation), origin(2), vec![], None)
        .unwrap()
        .accepted;
    owner
        .execute(
            Command::Do(Operation::SetTrackSyncLock {
                track: owner.history().document().tracks[0].id,
                locked: true,
            }),
            origin(3),
            vec![],
            None,
        )
        .unwrap();
    assert!(owner.origins.get(&first).is_none());
    assert!(
        owner
            .seeds
            .occurrences
            .values()
            .any(|id| id.creation_batch_id == origin(2).batch),
        "FOLD_ADVANCES_OCCURRENCE_SEEDS"
    );
    let boundary = owner.boundary(0).unwrap();
    let (baseline, lineage) = (&boundary.document, &boundary.lineage);
    lineage.verify(baseline).unwrap();
    let boundary = owner.boundary(1).unwrap();
    let (current, lineage) = (&boundary.document, &boundary.lineage);
    lineage.verify(current).unwrap();
}

#[test]
fn ordinary_origins_fit_ten_thousand_steps() {
    let mut owner = OwnerHistory::new(
        &HistoryPool::new(PoolLimits::default()),
        Role::Ordinary,
        RetentionPolicy::Fold,
        &origin(1),
        || Ok(document()),
    )
    .unwrap();
    for n in 0..10_000 {
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
    assert_eq!(
        owner.history().undo().len(),
        10_000,
        "OWNER_TEN_THOUSAND_STEPS"
    );
    assert!(
        owner.owner_bytes() < 5 << 20,
        "OWNER_ORDINARY_METADATA_ENVELOPE"
    );
}

#[test]
fn resources_include_payloads_in_the_live_redo_tail() {
    let mut cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../kinewright-core/tests/support/history_operation_cases.json"
    ))
    .unwrap();
    let document: Document =
        serde_json::from_value(cases["AddLutAsset"]["document"].take()).unwrap();
    let operation: Operation =
        serde_json::from_value(cases["AddLutAsset"]["operation"].take()).unwrap();
    let required = Resources::payload(std::slice::from_ref(&operation));
    assert!(!required.is_empty());
    let mut owner = OwnerHistory::new(
        &HistoryPool::new(PoolLimits::default()),
        Role::Ordinary,
        RetentionPolicy::Fold,
        &origin(1),
        || Ok(document),
    )
    .unwrap();
    owner
        .execute(Command::Do(operation), origin(2), vec![], None)
        .unwrap();
    owner
        .execute(Command::Undo, origin(3), vec![], None)
        .unwrap();
    for resource in &required {
        assert!(
            owner.resources().contains(resource),
            "REDO_PAYLOAD_RESOURCE_SUPERSET"
        );
    }
    for depth in 0..=1 {
        let boundary = owner.boundary(depth).unwrap();
        let document = boundary.document();
        for resource in Resources::document(document) {
            assert!(
                owner.resources().contains(&resource),
                "EVERY_REACHABLE_RESOURCE"
            );
        }
    }
}

#[test]
fn recreation_is_bounded_evidence_and_never_resurrects_a_discarded_approval() {
    let mut history = owner();
    let key = history
        .execute(remove(), origin(2), vec![], Some(approval()))
        .unwrap()
        .accepted;
    history
        .execute(
            Command::Do(Operation::AddTrack {
                track: document().tracks.remove(0),
            }),
            origin(3),
            vec![],
            None,
        )
        .unwrap();
    assert!(
        history.ledger().get(key).unwrap().recreated,
        "RECREATION_COMES_FROM_LATER_IDENTITY_EFFECTS"
    );
    history
        .execute(Command::Undo, origin(4), vec![], None)
        .unwrap();
    history
        .execute(Command::Undo, origin(5), vec![], None)
        .unwrap();
    assert_eq!(
        history.ledger().get(key).unwrap().answer(),
        ApprovalAnswer::Uncertain,
        "UNDONE_RECREATION_STAYS_UNCERTAIN"
    );
    history
        .execute(
            Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(81),
                locked: true,
            }),
            origin(6),
            vec![],
            None,
        )
        .unwrap();
    let entry = history.ledger().get(key).unwrap();
    assert!(entry.recreated, "RECREATION_EVIDENCE_IS_ONE_STICKY_BIT");
    assert_eq!(
        entry.answer(),
        ApprovalAnswer::Missing,
        "DISCARDED_RECREATION_NEVER_RESURRECTS_APPROVAL"
    );
}

#[test]
fn empty_owner_batch_precedes_workspace_capacity() {
    let pool = HistoryPool::new(PoolLimits::default());
    let mut owner = OwnerHistory::new(
        &pool,
        Role::Task,
        RetentionPolicy::NeverFold,
        &origin(1),
        || Ok(document()),
    )
    .unwrap();
    let blocker = pool.admit(Role::Task).unwrap();
    let _window = blocker.workspace().unwrap();
    let before = owner.history().snapshot_identity().unwrap();
    for command in [
        Command::DoBatch(vec![]),
        Command::DoBatchCoalesced {
            operations: vec![],
            coalesce_key: "drag".into(),
        },
    ] {
        assert_eq!(
            owner.execute(command, origin(2), vec![], None),
            Err(Error::History(kinewright_core::undo::Error::Batch(
                kinewright_core::BatchError::Empty
            ))),
            "OWNER_EMPTY_PRECEDES_SHARED_CAPACITY"
        );
        assert_eq!(
            owner.history().snapshot_identity().unwrap(),
            before,
            "OWNER_EMPTY_REFUSAL_UNCHANGED"
        );
    }
}
