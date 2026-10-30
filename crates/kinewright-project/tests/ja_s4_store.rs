//! Isolated H1 contracts, not B1-B5 or bundle recovery acceptance.
//! Rust adaptation of <https://github.com/pingdotgg/t3code> at
//! 5d91ab1117ad51ef1288a94717d85c70e7af09fe, apps/server/src/orchestration-v2/
//! {EventSink,CommandReceiptStore}.ts. Notice: LICENSES/t3code-MIT.txt.
//! Kinewright adds cross-scope reservation-loser and complete-cut checks.
#![allow(clippy::missing_errors_doc)]
#![allow(
    dead_code,
    reason = "project contracts compile in isolation pending S3 H1 registration"
)]

use kinewright_project::{bundle_records, orchestration_commit, orchestration_records, receipts};

use bundle_records::{ObjectRef, RecordKind};
use orchestration_commit::{CommandDecision, ReceiptStatus, ValidatedRunUpdate, checked_receipt};
use orchestration_records::*;
use serde::de::DeserializeOwned;
use std::sync::{Arc, Barrier, Mutex};

fn id<T: DeserializeOwned>(value: u64) -> T {
    serde_json::from_value(serde_json::Value::String(format!("{value:032x}"))).unwrap()
}
fn reference(value: u64, kind: RecordKind) -> ObjectRef {
    ObjectRef {
        sha256: format!("{value:064x}"),
        byte_length: value,
        record_kind: kind,
    }
}
fn envelope(project: u64, assignment: u64) -> OrchestrationEnvelope {
    let brief = TaskBrief {
        plan: reference(1, RecordKind::OwnedScript),
        script: reference(2, RecordKind::OwnedScript),
        inputs: reference(3, RecordKind::InputManifest),
        assignment: reference(assignment, RecordKind::OwnedData),
        role: Role::Director,
        provider: ProviderSelectionRecord {
            connector: "scripted".into(),
            model: "fixture".into(),
            options: vec![],
        },
        scope: TaskScope {
            operations: std::collections::BTreeSet::new(),
            objects: std::collections::BTreeSet::new(),
            fields: std::collections::BTreeSet::new(),
            tracks: std::collections::BTreeSet::new(),
            bus_sidechain_closure: std::collections::BTreeSet::new(),
            asset_roots: vec![reference(5, RecordKind::OwnedMedia)],
            base: BranchBinding {
                branch: id(11),
                head: id(12),
                checkpoint: reference(6, RecordKind::BranchCheckpoint),
                document_hash: format!("{:064x}", 13),
            },
            consent_refs: vec![reference(7, RecordKind::OwnedData)],
        },
    };
    let mut envelope = OrchestrationEnvelope {
        id: receipts::CommandId {
            issuer: id(8),
            sequence: 1,
        },
        digest: id_digest(),
        project: id(project),
        expected_generation: 20,
        authority: AuthorityBinding {
            grant_ref: reference(9, RecordKind::OwnedData),
            owner_epoch: 2,
            endpoint: None,
            attempt: None,
        },
        command: OrchestrationCommand::StartRun(Box::new(StartRun { director: brief })),
    };
    envelope.digest = envelope.computed_digest().unwrap();
    envelope
}
fn id_digest() -> receipts::CommandDigest {
    serde_json::from_value(serde_json::Value::String("0".repeat(64))).unwrap()
}
fn projection(envelope: &OrchestrationEnvelope) -> RunProjection {
    let OrchestrationCommand::StartRun(start) = &envelope.command else {
        unreachable!()
    };
    let task: TaskId = envelope.identity("task").unwrap();
    RunProjection {
        run: envelope.identity("run").unwrap(),
        sequence: 1,
        control_revision: id(14),
        phase: RunPhase::Queued,
        outcome: RunOutcome::Pending,
        wait: RunWait::None,
        cleanup: RunCleanup::Settled,
        tasks: vec![TaskRecord {
            id: task.clone(),
            parent: None,
            brief: start.director.clone(),
            state: TaskState::Queued,
            progress: TaskProgress::default(),
        }],
        effects: vec![EffectRecord {
            id: envelope.identity("effect.prepare").unwrap(),
            task,
            lane: id(11),
            order: 1,
            safety: EffectSafety::ReplaySafe,
            state: EffectState::Pending,
            payload: reference(30, RecordKind::OwnedData),
        }],
        attempts: vec![],
        deliveries: vec![],
        requests: vec![],
        results: vec![],
        usage: vec![],
    }
}
fn update(envelope: &OrchestrationEnvelope, projection: RunProjection) -> ValidatedRunUpdate {
    let segment = EventSegment {
        run: projection.run.clone(),
        first_sequence: projection.sequence,
        previous: None,
        events: vec![OrchestrationEvent {
            command: envelope.id.clone(),
            sequence: projection.sequence,
            projection: projection.clone(),
        }],
    };
    ValidatedRunUpdate::new(
        &envelope.project,
        20,
        envelope,
        projection,
        segment,
        CommandDecision::Applied,
    )
    .unwrap()
}

#[test]
fn s4_one_root_cut_and_replay_contract() {
    let envelope = envelope(10, 4);
    let update = update(&envelope, projection(&envelope));
    let received: orchestration_commit::OrchestrationReceipt =
        serde_json::from_slice(&serde_json::to_vec(update.receipt()).unwrap()).unwrap();
    update.validate_returned_receipt(&received).unwrap();
    assert_eq!(
        checked_receipt(&envelope, ReceiptStatus::Committed(received.clone())).unwrap(),
        ReceiptStatus::Committed(received.clone())
    );
    assert_eq!(
        update.projection().tasks[0].id,
        envelope.identity::<TaskId>("task").unwrap()
    );
    assert_eq!(
        update.projection().effects[0].id,
        envelope.identity::<EffectId>("effect.prepare").unwrap()
    );
    let mut wrong_cut = received;
    wrong_cut.sequence += 1;
    assert_eq!(
        update.validate_returned_receipt(&wrong_cut),
        Err(Refusal::ScopeConflict)
    );
    for status in [
        ReceiptStatus::Unknown,
        ReceiptStatus::RetiredUnresolved,
        ReceiptStatus::UnresolvedOrigin,
        ReceiptStatus::Pending,
    ] {
        assert_eq!(checked_receipt(&envelope, status.clone()).unwrap(), status);
    }
}

#[test]
fn s4_receipt_scope_collision_forced_reservation_loser_contract() {
    for loser_project in [10, 99] {
        let a = envelope(10, 4);
        let b = envelope(loser_project, 41);
        let reservation = Arc::new(Mutex::new(None));
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = [a, b]
            .into_iter()
            .map(|envelope| {
                let reservation = Arc::clone(&reservation);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let planned = update(&envelope, projection(&envelope));
                    barrier.wait();
                    let winner = {
                        let mut reservation = reservation.lock().unwrap();
                        reservation
                            .get_or_insert_with(|| planned.receipt().clone())
                            .clone()
                    };
                    let returned = checked_receipt(&envelope, ReceiptStatus::Committed(winner));
                    (envelope, returned)
                })
            })
            .collect();
        let outcomes: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(
            outcomes.iter().filter(|(_, result)| result.is_ok()).count(),
            1
        );
        for (scope, result) in outcomes {
            match result {
                Ok(ReceiptStatus::Committed(receipt)) => assert_eq!(receipt.envelope, scope),
                Err(error) => assert_eq!(error, Refusal::ScopeConflict),
                other => panic!("unexpected reservation result {other:?}"),
            }
        }
    }
}

#[test]
fn s4_run_graph_pending_dependencies_contract() {
    let envelope = envelope(10, 4);
    let mut projection = projection(&envelope);
    let task = projection.tasks[0].id.clone();
    projection.tasks[0].state = TaskState::Terminal(reference(31, RecordKind::RunIndex));
    projection.deliveries.push(DeliveryRecord {
        id: id(32),
        task: task.clone(),
        result: reference(33, RecordKind::RunIndex),
        generation: 1,
        wake: id(46),
        message: id(34),
        state: DeliveryState::Pending,
    });
    projection
        .requests
        .push(RuntimeRequest::Question(QuestionRecord {
            id: id(35),
            task: task.clone(),
            origin: TaskTurn {
                task,
                attempt: id(41),
                session: id(42),
                ordinal: 1,
                owner_epoch: 1,
                expected_head: id(43),
                purpose: TurnPurpose::Work,
            },
            status: QuestionStatus::Pending,
            wake: QuestionWake::Pending,
            prompt: reference(36, RecordKind::OwnedData),
        }));
    projection.results.push(reference(37, RecordKind::RunIndex));
    projection.usage.push(reference(38, RecordKind::RunIndex));
    projection.wait = RunWait::Files(WaitDetail {
        reason: reference(39, RecordKind::OwnedData),
        missing: vec![reference(40, RecordKind::OwnedData)],
        attempt: None,
    });
    let update = update(&envelope, projection);
    for (value, kind) in [
        (1, RecordKind::OwnedScript),
        (2, RecordKind::OwnedScript),
        (3, RecordKind::InputManifest),
        (4, RecordKind::OwnedData),
        (5, RecordKind::OwnedMedia),
        (6, RecordKind::BranchCheckpoint),
        (7, RecordKind::OwnedData),
        (9, RecordKind::OwnedData),
        (30, RecordKind::OwnedData),
        (31, RecordKind::RunIndex),
        (33, RecordKind::RunIndex),
        (36, RecordKind::OwnedData),
        (37, RecordKind::RunIndex),
        (38, RecordKind::RunIndex),
        (39, RecordKind::OwnedData),
        (40, RecordKind::OwnedData),
    ] {
        assert!(
            update.dependencies().contains(&reference(value, kind)),
            "missing dependency {value}"
        );
    }
}

#[test]
fn s4_support_and_original_envelope_contract() {
    let envelope = envelope(10, 4);
    let mut wrong = envelope.clone();
    wrong.authority.owner_epoch += 1;
    assert_eq!(wrong.validate(), Err(Refusal::ScopeConflict));
    let mut record = OrchestrationRecord {
        version: 2,
        payload: OrchestrationPayload::Snapshot(Box::new(projection(&envelope))),
    };
    assert_eq!(record.dependencies(), Err(Refusal::Unsupported));
    record.version = 1;
    record.dependencies().unwrap();
    let bad: Result<ExecutionIdentity, _> = serde_json::from_value(serde_json::json!({
        "version":1, "project":"0".repeat(32), "run":"0".repeat(32), "task":"0".repeat(32),
        "attempt":"0".repeat(32), "nonce":"A".repeat(32), "owner_instance":"0".repeat(32)
    }));
    assert!(bad.is_err());
    let mut oversized = envelope.clone();
    let OrchestrationCommand::StartRun(start) = &mut oversized.command else {
        unreachable!()
    };
    start.director.provider.model = "x".repeat(COMMAND_BYTES + 1);
    assert_eq!(oversized.computed_digest(), Err(Refusal::Capacity));
}

#[test]
fn s4_stale_generation_and_mismatched_event_contract() {
    let envelope = envelope(10, 4);
    let update = update(&envelope, projection(&envelope));
    assert!(matches!(
        ValidatedRunUpdate::new(
            &id(99),
            20,
            &envelope,
            update.projection().clone(),
            update.segment().clone(),
            CommandDecision::Applied
        ),
        Err(Refusal::ScopeConflict)
    ));
    assert!(matches!(
        ValidatedRunUpdate::new(
            &envelope.project,
            21,
            &envelope,
            update.projection().clone(),
            update.segment().clone(),
            CommandDecision::Applied
        ),
        Err(Refusal::StaleGeneration)
    ));
    let mut segment = update.segment().clone();
    segment.events[0].sequence += 1;
    assert!(matches!(
        ValidatedRunUpdate::new(
            &envelope.project,
            20,
            &envelope,
            update.projection().clone(),
            segment,
            CommandDecision::Applied
        ),
        Err(Refusal::InvalidState)
    ));
}

#[test]
fn s4_large_owned_input_reference_contract() {
    let mut media = reference(70, RecordKind::OwnedMedia);
    media.byte_length = 1024 * 1024 * 1024;
    validate_reference(&media).unwrap();
    media.record_kind = RecordKind::RunIndex;
    assert_eq!(validate_reference(&media), Err(Refusal::InvalidState));
}

#[test]
fn s4_event_predecessor_retention_contract() {
    let envelope = envelope(10, 4);
    let mut state = projection(&envelope);
    state.sequence = 2;
    let mut record = OrchestrationRecord {
        version: 1,
        payload: OrchestrationPayload::EventSegment(EventSegment {
            run: state.run.clone(),
            first_sequence: 2,
            previous: None,
            events: vec![OrchestrationEvent {
                command: envelope.id.clone(),
                sequence: 2,
                projection: state,
            }],
        }),
    };
    assert_eq!(record.dependencies(), Err(Refusal::InvalidState));
    let OrchestrationPayload::EventSegment(segment) = &mut record.payload else {
        unreachable!()
    };
    segment.previous = Some(reference(60, RecordKind::RunIndex));
    assert!(
        record
            .dependencies()
            .unwrap()
            .contains(&reference(60, RecordKind::RunIndex))
    );
    let OrchestrationPayload::EventSegment(segment) = &mut record.payload else {
        unreachable!()
    };
    segment.first_sequence = 0;
    segment.events[0].sequence = 0;
    segment.events[0].projection.sequence = 0;
    assert_eq!(record.dependencies(), Err(Refusal::InvalidState));
}

fn reservation_fixture(
    envelope: &OrchestrationEnvelope,
    state: &RunProjection,
) -> AttemptReservation {
    AttemptReservation {
        identity: ExecutionIdentity {
            version: 1,
            project: envelope.project.clone(),
            run: state.run.clone(),
            task: state.tasks[0].id.clone(),
            attempt: id(61),
            nonce: id(62),
            owner_instance: id(63),
        },
        owner_epoch: 2,
        state: AttemptState::Reserved,
        recovery_ref: reference(64, RecordKind::OwnedData),
    }
}
fn cleanup_identity(reservation: &AttemptReservation) -> CleanupIdentity {
    CleanupIdentity {
        attempt: reservation.identity.attempt.clone(),
        nonce: reservation.identity.nonce.clone(),
        owner_instance: reservation.identity.owner_instance.clone(),
        recovery_ref: reservation.recovery_ref.clone(),
    }
}

#[test]
fn s4_attempt_identity_unique_contract() {
    let envelope = envelope(10, 4);
    let mut state = projection(&envelope);
    let first = reservation_fixture(&envelope, &state);
    let mut second = first.clone();
    second.identity.nonce = id(65);
    second.identity.owner_instance = id(66);
    state.cleanup = RunCleanup::Pending(vec![cleanup_identity(&first), cleanup_identity(&second)]);
    state.attempts = vec![first.clone(), second];
    assert_eq!(
        orchestration_commit::validate_projection(&state),
        Err(Refusal::InvalidState)
    );
    state.attempts.pop();
    state.cleanup = RunCleanup::Pending(vec![cleanup_identity(&first)]);
    orchestration_commit::validate_projection(&state).unwrap();
}

#[test]
fn s4_cleanup_matches_attempt_facts_contract() {
    let envelope = envelope(10, 4);
    let mut state = projection(&envelope);
    state.phase = RunPhase::Paused;
    let mut reservation = reservation_fixture(&envelope, &state);
    let cleanup = cleanup_identity(&reservation);
    reservation.state = AttemptState::CleanupBlocked(cleanup.clone());
    state.attempts.push(reservation);
    assert_eq!(
        orchestration_commit::validate_projection(&state),
        Err(Refusal::InvalidState)
    );
    state.cleanup =
        RunCleanup::Blocked(vec![cleanup.clone()], reference(67, RecordKind::OwnedData));
    orchestration_commit::validate_projection(&state).unwrap();
    state.cleanup = RunCleanup::Pending(vec![cleanup.clone()]);
    assert_eq!(
        orchestration_commit::validate_projection(&state),
        Err(Refusal::InvalidState)
    );
    state.attempts[0].state = AttemptState::Executing;
    state.cleanup = RunCleanup::Settled;
    state.tasks[0].state = TaskState::Terminal(reference(68, RecordKind::RunIndex));
    state.effects[0].state = EffectState::Settled;
    state.phase = RunPhase::Finished;
    state.outcome = RunOutcome::Ready;
    assert_eq!(
        orchestration_commit::validate_projection(&state),
        Err(Refusal::InvalidState)
    );
    state.outcome = RunOutcome::Pending;
    state.phase = RunPhase::Paused;
    state.cleanup = RunCleanup::Pending(vec![cleanup]);
    orchestration_commit::validate_projection(&state).unwrap();
}
