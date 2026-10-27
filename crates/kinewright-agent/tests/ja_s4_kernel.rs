//! Isolated kernel and guardian-admission contracts, not durable H1 acceptance.
//! Rust adaptation of <https://github.com/pingdotgg/t3code> at
//! 5d91ab1117ad51ef1288a94717d85c70e7af09fe, apps/server/src/orchestration-v2/
//! {Orchestrator,EffectOutbox,EffectWorker}.ts. Notice: LICENSES/t3code-MIT.txt.
//! Kinewright adds original-envelope checks and opaque attempt reservation proof.
#![allow(
    dead_code,
    reason = "the H1 owner-only constructor is exercised with its source module"
)]

#[path = "../src/orchestration/mod.rs"]
mod orchestration;

use kinewright_project::{
    bundle_records::{ObjectRef, RecordKind},
    receipts,
};
use orchestration::{machine, orchestration_commit::*, orchestration_records::*, outbox};
use serde::de::DeserializeOwned;

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
fn envelope() -> OrchestrationEnvelope {
    let brief = TaskBrief {
        plan: reference(1, RecordKind::OwnedScript),
        script: reference(2, RecordKind::OwnedScript),
        inputs: reference(3, RecordKind::InputManifest),
        assignment: reference(4, RecordKind::OwnedData),
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
            asset_roots: vec![],
            consent_refs: vec![],
            base: BranchBinding {
                branch: id(11),
                head: id(12),
                checkpoint: reference(6, RecordKind::BranchCheckpoint),
                document_hash: format!("{:064x}", 13),
            },
        },
    };
    let mut envelope = OrchestrationEnvelope {
        id: receipts::CommandId {
            issuer: id(8),
            sequence: 1,
        },
        digest: serde_json::from_value(serde_json::Value::String("0".repeat(64))).unwrap(),
        project: id(10),
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
fn start(envelope: &OrchestrationEnvelope) -> ValidatedRunUpdate {
    machine::prepare(None, &envelope.project, 20, envelope, None).unwrap()
}
fn control(
    envelope: &OrchestrationEnvelope,
    state: &RunProjection,
    action: RunAction,
) -> OrchestrationEnvelope {
    let mut next = envelope.clone();
    next.id.sequence += 1;
    next.expected_generation += 1;
    next.command = OrchestrationCommand::ControlRun(RunIntent {
        command_id: next.id.clone(),
        run_id: state.run.clone(),
        expected_control_revision: state.control_revision.clone(),
        action,
    });
    next.digest = next.computed_digest().unwrap();
    next
}

#[test]
fn s4_kernel_start_and_control_contract() {
    let envelope = envelope();
    let first = start(&envelope);
    let replay = start(&envelope);
    assert_eq!(first.projection(), replay.projection());
    assert_eq!(first.receipt(), replay.receipt());
    let state = first.projection();
    let pause = control(&envelope, state, RunAction::Pause);
    let paused = machine::prepare(
        Some(state),
        &envelope.project,
        21,
        &pause,
        Some(reference(87, RecordKind::RunIndex)),
    )
    .unwrap();
    assert_eq!(paused.projection().phase, RunPhase::Paused);
    assert_ne!(paused.projection().control_revision, state.control_revision);
    assert_eq!(paused.projection().tasks, state.tasks);
    assert_eq!(paused.projection().effects, state.effects);
    let mut outdated = control(&pause, paused.projection(), RunAction::Continue);
    let OrchestrationCommand::ControlRun(intent) = &mut outdated.command else {
        unreachable!()
    };
    intent.expected_control_revision = state.control_revision.clone();
    outdated.digest = outdated.computed_digest().unwrap();
    let refused = machine::prepare(
        Some(paused.projection()),
        &envelope.project,
        22,
        &outdated,
        Some(reference(88, RecordKind::RunIndex)),
    )
    .unwrap();
    assert_eq!(
        refused.receipt().decision,
        CommandDecision::Refused(RecordedRefusal::StaleControl)
    );
    assert_eq!(refused.projection().phase, RunPhase::Paused);
    assert_eq!(
        refused.projection().control_revision,
        paused.projection().control_revision
    );
    let mut folded = state.clone();
    machine::fold(&mut folded, &paused.segment().events[0]).unwrap();
    assert_eq!(&folded, paused.projection());
    assert_eq!(
        machine::fold(&mut folded, &paused.segment().events[0]),
        Err(Refusal::InvalidState)
    );
}

#[test]
fn s4_unknown_publication_freezes_original_envelope_contract() {
    let envelope = envelope();
    let mut kernel = machine::Kernel::default();
    let receipt = kernel
        .propose(&envelope.project, 20, &envelope, None)
        .unwrap()
        .receipt()
        .clone();
    assert!(kernel.acknowledged().is_none());
    kernel.publication_unknown().unwrap();
    assert_eq!(kernel.original_envelope(), Some(&envelope));
    assert!(matches!(
        kernel.propose(&envelope.project, 20, &envelope, None),
        Err(Refusal::PublicationUnknown)
    ));
    let mut wrong = receipt.clone();
    wrong.cut_digest = "f".repeat(64);
    assert_eq!(kernel.acknowledge(&wrong), Err(Refusal::ScopeConflict));
    assert!(kernel.acknowledged().is_none());
    kernel.acknowledge(&receipt).unwrap();
    assert!(kernel.original_envelope().is_none());
    assert_eq!(
        kernel.acknowledged().unwrap().effects[0].id,
        envelope.identity::<EffectId>("effect.prepare").unwrap()
    );
    assert_eq!(kernel.acknowledge(&receipt), Err(Refusal::Unknown));
}

#[test]
fn s4_outbox_order_unknown_and_reconcile_contract() {
    let envelope = envelope();
    let start = start(&envelope);
    let mut state = start.projection().clone();
    let first = state.effects[0].clone();
    let mut later = first.clone();
    later.id = id(90);
    later.order += 1;
    state.effects.push(later.clone());
    let claimed = outbox::claim(&state, &first.id, &id(20)).unwrap();
    assert!(outbox::next_effect(&claimed, &first.lane).is_none());
    assert!(outbox::claim(&claimed, &first.id, &id(21)).is_err());
    assert!(outbox::claim(&claimed, &later.id, &id(21)).is_err());
    let unknown = outbox::outcome_unknown(&claimed, &first.id, &id(20)).unwrap();
    assert!(outbox::next_effect(&unknown, &first.lane).is_none());
    let retried = outbox::retry_reconciled_safe(&unknown, &first.id).unwrap();
    assert_eq!(
        outbox::next_effect(&retried, &first.lane).unwrap().id,
        first.id
    );
    let mut process_bound = unknown;
    process_bound.effects[0].safety = EffectSafety::ProcessBound;
    assert_eq!(
        outbox::retry_reconciled_safe(&process_bound, &first.id),
        Err(Refusal::InvalidState)
    );
    assert_eq!(
        outbox::outcome_unknown(&claimed, &first.id, &id(21)),
        Err(Refusal::ScopeConflict)
    );
    let mut other_lane = later;
    other_lane.id = id(91);
    other_lane.lane = id(92);
    process_bound.effects.push(other_lane.clone());
    assert_eq!(
        outbox::next_effect(&process_bound, &other_lane.lane).unwrap(),
        &other_lane
    );
}

#[test]
fn s4_durable_before_commit_attempt_contract() {
    let envelope = envelope();
    let start = start(&envelope);
    let mut state = start.projection().clone();
    let identity = ExecutionIdentity {
        version: 1,
        project: envelope.project.clone(),
        run: state.run.clone(),
        task: state.tasks[0].id.clone(),
        attempt: id(50),
        nonce: id(51),
        owner_instance: id(52),
    };
    state.attempts.push(AttemptReservation {
        identity: identity.clone(),
        owner_epoch: 2,
        state: AttemptState::Reserved,
        recovery_ref: reference(53, RecordKind::OwnedData),
    });
    state.cleanup = RunCleanup::Pending(vec![CleanupIdentity {
        attempt: identity.attempt.clone(),
        nonce: identity.nonce.clone(),
        owner_instance: identity.owner_instance.clone(),
        recovery_ref: reference(53, RecordKind::OwnedData),
    }]);
    let segment = EventSegment {
        run: state.run.clone(),
        first_sequence: 1,
        previous: None,
        events: vec![OrchestrationEvent {
            command: envelope.id.clone(),
            sequence: 1,
            projection: state.clone(),
        }],
    };
    let reserved = ValidatedRunUpdate::new(
        &envelope.project,
        20,
        &envelope,
        state,
        segment,
        CommandDecision::Applied,
    )
    .unwrap();
    let admitted =
        orchestration::acknowledge_attempt_reservation(&reserved, reserved.receipt(), &identity)
            .unwrap();
    assert_eq!(admitted.identity(), &identity);
    assert_eq!(admitted.reservation_receipt(), reserved.receipt());
    assert!(
        orchestration::acknowledge_attempt_reservation(&reserved, start.receipt(), &identity)
            .is_err()
    );
    let mut wrong_identity = identity.clone();
    wrong_identity.nonce = id(54);
    assert!(
        orchestration::acknowledge_attempt_reservation(
            &reserved,
            reserved.receipt(),
            &wrong_identity
        )
        .is_err()
    );
    let denied = ValidatedRunUpdate::new(
        &envelope.project,
        20,
        &envelope,
        reserved.projection().clone(),
        reserved.segment().clone(),
        CommandDecision::Refused(RecordedRefusal::InvalidState),
    )
    .unwrap();
    assert!(
        orchestration::acknowledge_attempt_reservation(&denied, denied.receipt(), &identity)
            .is_err()
    );
    let encoded = serde_json::to_vec(&identity).unwrap();
    assert!(encoded.len() < 1024);
    assert_eq!(
        serde_json::from_slice::<ExecutionIdentity>(&encoded).unwrap(),
        identity
    );
}

#[test]
fn s4_stop_retains_cleanup_and_never_continues_failed_work_contract() {
    let envelope = envelope();
    let start = start(&envelope);
    let mut state = start.projection().clone();
    let cleanup = CleanupIdentity {
        attempt: id(80),
        owner_instance: id(81),
        nonce: id(82),
        recovery_ref: reference(83, RecordKind::OwnedData),
    };
    let identity = ExecutionIdentity {
        version: 1,
        project: envelope.project.clone(),
        run: state.run.clone(),
        task: state.tasks[0].id.clone(),
        attempt: cleanup.attempt.clone(),
        nonce: cleanup.nonce.clone(),
        owner_instance: cleanup.owner_instance.clone(),
    };
    state.attempts.push(AttemptReservation {
        identity,
        owner_epoch: 2,
        state: AttemptState::CleanupBlocked(cleanup.clone()),
        recovery_ref: cleanup.recovery_ref.clone(),
    });
    state.tasks[0].state = TaskState::CleanupBlocked(cleanup.clone());
    state.cleanup = RunCleanup::Blocked(vec![cleanup], reference(84, RecordKind::OwnedData));
    state.wait = RunWait::Provider(WaitDetail {
        reason: reference(85, RecordKind::OwnedData),
        missing: vec![],
        attempt: Some(id(80)),
    });
    let stop = control(&envelope, &state, RunAction::Stop);
    let stopped = machine::prepare(
        Some(&state),
        &envelope.project,
        21,
        &stop,
        Some(reference(89, RecordKind::RunIndex)),
    )
    .unwrap();
    assert_eq!(stopped.projection().cleanup, state.cleanup);
    assert_eq!(stopped.projection().wait, state.wait);
    assert!(matches!(
        stopped.projection().outcome,
        RunOutcome::Cancelled(_)
    ));
    assert!(!machine::eligible(
        stopped.projection(),
        &RunAction::Continue
    ));
    assert!(!machine::eligible(
        stopped.projection(),
        &RunAction::SendMessage {
            text_ref: reference(86, RecordKind::OwnedData)
        }
    ));
    assert_eq!(stopped.projection().effects[0].state, EffectState::Disposed);
    let mut contradictory = state.clone();
    contradictory.phase = RunPhase::Paused;
    contradictory.wait = RunWait::None;
    contradictory.cleanup = RunCleanup::Settled;
    let resume = control(&envelope, &contradictory, RunAction::Continue);
    assert!(matches!(
        machine::prepare(
            Some(&contradictory),
            &envelope.project,
            21,
            &resume,
            Some(reference(90, RecordKind::RunIndex))
        ),
        Err(Refusal::InvalidState)
    ));
}
