use super::*;
use kinewright_core::{
    Command, Document, Operation, Track, TrackId, TrackKind,
    undo::{History, PhaseCounters, RetentionPolicy, RoleBounds},
};

/// Adversarial stories retain several independent sibling lines at once.
/// This fixture is not a product process admission test; `session_model` uses
/// the unmodified two-Ordinary default pool and asserts its observed peak.
pub(super) fn story_pool() -> HistoryPool {
    let mut limits = PoolLimits::default();
    limits.retained[0] = 8 * Role::Ordinary.envelope().retained;
    limits.peak[0] = limits.retained[0] + limits.workspace;
    HistoryPool::new(limits)
}
pub(super) fn origin(value: u64) -> CreationOrigin {
    CreationOrigin {
        branch: serde_json::from_str(&format!("\"{:032x}\"", 1)).unwrap(),
        batch: serde_json::from_str(&format!("\"{value:032x}\"")).unwrap(),
    }
}
pub(super) fn document() -> Document {
    Document {
        tracks: vec![Track {
            name: None,
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
    let mut second_approval = approval();
    second_approval.reference.sha256 = "b".repeat(64);
    let second = history
        .execute(remove(), origin(6), vec![], Some(second_approval))
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
                    name: None,
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
        approved.reference.sha256 = format!("{id:064x}");
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
        assert_eq!(
            owner.owner_identity_hashes, 0,
            "NO_WARM_OWNER_IDENTITY_HASH"
        );
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
    let reservation = pool.admit(Role::Task).unwrap();
    assert_eq!(
        pool.usage(Role::Task).unwrap(),
        (envelope.retained, 0),
        "RESERVATION_EXACT_FIT"
    );
    assert!(
        matches!(pool.admit(Role::Task), Err(Error::PoolFull(Role::Task))),
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
            matches!(reservation.workspace(), Err(Error::PoolFull(Role::Task))),
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
        Err(Error::PoolFull(Role::Task))
    ));
    assert!(!called, "REFUSAL_PRECEDES_DOCUMENT_ALLOCATION");
}
#[test]
fn candidate_workspace_and_shared_peak_have_their_own_arithmetic() {
    let role = Role::Candidate.envelope();
    assert_eq!(
        role.retained,
        72 << 20,
        "CANDIDATE_MAXIMUM_RETAINED_ENVELOPE"
    );
    assert_eq!(role.workspace, 168 << 20, "CANDIDATE_WORKSPACE_ENVELOPE");
    // Raw ingress includes the structural decoder's 64 MiB ceiling and
    // one untrusted document at its separate 64 MiB decoder ceiling. The
    // previous document was already checked against D. The structural charge
    // includes twice the encoded input; replay's 10% margin is separate.
    let ingress = Role::Candidate.persisted_shell_cap()
        + 2 * (64 << 20)
        + Role::Candidate.bounds().document_live_bytes
        + (64 << 10);
    assert!(
        ingress < role.retained + role.workspace,
        "CANDIDATE_WORKSPACE_ARITHMETIC"
    );
    // The 20,000-clip ripple uses seven D in the kernel reservation; the
    // Ordinary executor also reserves three A, with the same 10% margin.
    let ripple = 7 * Role::Ordinary.bounds().document_live_bytes
        + 3 * Role::Ordinary.envelope().owners
        + (64 << 10);
    assert!(
        ripple + ripple.div_ceil(10) < role.workspace,
        "ORDINARY_RIPPLE_WORKSPACE_ARITHMETIC"
    );
    let pool = HistoryPool::new(PoolLimits::default());
    let task = pool.admit(Role::Task).unwrap();
    let candidate = pool.admit(Role::Candidate).unwrap();
    let window = task.workspace().unwrap();
    assert!(
        matches!(candidate.workspace(), Err(Error::PoolFull(Role::Candidate))),
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
    let required = Resources::payload(std::slice::from_ref(&operation), &[]);
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
fn approved_command_answers_only_its_fate_after_recreation() {
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
    assert_eq!(
        history.ledger().get(key).unwrap().answer(),
        ApprovalAnswer::Kept,
        "APPLIED_COMMAND_STAYS_APPLIED_AFTER_RECREATION"
    );
    history
        .execute(Command::Undo, origin(4), vec![], None)
        .unwrap();
    history
        .execute(Command::Undo, origin(5), vec![], None)
        .unwrap();
    assert_eq!(
        history.ledger().get(key).unwrap().answer(),
        ApprovalAnswer::Missing,
        "UNDONE_COMMAND_IS_NOT_APPLIED"
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

#[test]
fn forked_baseline_without_parent_approval_facts_never_claims_a_newer_epoch_is_missing() {
    let pool = story_pool();
    let mut parent = OwnerHistory::new(
        &pool,
        Role::Ordinary,
        RetentionPolicy::Fold,
        &origin(1),
        || Ok(document()),
    )
    .unwrap();
    parent.core.begin_install_epoch_at(3).unwrap();
    let approved = parent
        .execute(remove(), origin(2), vec![], Some(approval()))
        .unwrap()
        .accepted;
    let child = parent.fork_current(Role::Ordinary, true).unwrap();
    let selected = child.capture().unwrap().validate().unwrap();
    parent
        .execute(Command::Undo, origin(3), vec![], None)
        .unwrap();
    parent
        .execute(
            Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(81),
                locked: true,
            }),
            origin(4),
            vec![],
            None,
        )
        .unwrap();
    parent.install_snapshot(selected).unwrap();
    assert_eq!(
        parent.approval_answer(&approval().reference).unwrap(),
        Some(ApprovalAnswer::Uncertain),
        "FORKED_BASELINE_WITHOUT_FOLD_FACT_IS_UNCERTAIN_NOT_MISSING"
    );
    assert!(
        child.history().position().epoch > approved.epoch,
        "FORK_CAPTURE_EPOCH_IS_AFTER_ITS_PARENT_LINE"
    );
    let capture = parent.capture().unwrap();
    let restored = capture.validate().unwrap().open().unwrap();
    assert_eq!(
        restored.approval_answer(&approval().reference).unwrap(),
        Some(ApprovalAnswer::Uncertain),
        "FORK_BASELINE_UNCERTAINTY_SURVIVES_REOPEN"
    );
}
