#![cfg(feature = "test-util")]
#![expect(
    clippy::too_many_lines,
    reason = "Stateful witnesses keep one owner and its complete publication sequence visible"
)]
use kinewright_project::{
    bundle::BundleError, receipts::CommandId, studio_records::*, studio_store::RunProjectionCursor,
};
fn page<T>(sequence: u64) -> Page<T> {
    Page {
        sequence,
        rows: vec![],
        next: None,
    }
}
fn view(run_id: RunId) -> RunView {
    RunView {
        run_id,
        sequence: 1,
        control_revision: OpaqueRevision("control-a".into()),
        phase: RunPhase::Queued,
        outcome: RunOutcome::Pending,
        wait: RunWait::None,
        cleanup: RunCleanup::Settled,
        allowed_actions: vec![RunActionKind::Pause, RunActionKind::Stop],
        display_label: "Queued".into(),
        current_task: None,
        completed: page(1),
        queued: page(1),
        assumptions: page(1),
        review_state: ReviewView {
            state: "unreviewed".into(),
            evidence_refs: vec![],
        },
    }
}
#[test]
fn s3_run_projection_contract() {
    let run_id = RunId::fresh().unwrap();
    let reason = OutcomeDetail {
        reason: "explicit failure".into(),
        evidence_refs: vec![],
    };
    let wait = RunWait::Provider(WaitDetail {
        reason: "approval pending".into(),
        provider_reason: Some(ProviderWaitReason::Approval),
        attempt: Some(AttemptId::fresh().unwrap()),
        missing_refs: vec![],
    });
    let cleanup = RunCleanup::Blocked {
        identities: vec!["attempt:retirement".into()],
        reason: "proof missing".into(),
    };
    let mut fixtures = vec![];
    for phase in [
        RunPhase::Queued,
        RunPhase::Running,
        RunPhase::Reviewing,
        RunPhase::Paused,
        RunPhase::Interrupted,
        RunPhase::Finished,
    ] {
        for outcome in [
            RunOutcome::Pending,
            RunOutcome::Ready,
            RunOutcome::Failed(reason.clone()),
            RunOutcome::Cancelled(reason.clone()),
        ] {
            let mut v = view(run_id.clone());
            v.phase = phase.clone();
            v.outcome = outcome;
            if v.phase != RunPhase::Finished {
                v.outcome = RunOutcome::Pending;
            }
            if matches!(v.outcome, RunOutcome::Failed(_) | RunOutcome::Cancelled(_)) {
                v.wait = wait.clone();
                v.cleanup = cleanup.clone();
                v.allowed_actions.clear();
            }
            if v.phase == RunPhase::Paused || v.phase == RunPhase::Interrupted {
                v.allowed_actions = vec![RunActionKind::Continue];
            }
            v.validate().unwrap();
            let bytes = serde_json::to_vec(&v).unwrap();
            assert_eq!(
                serde_json::from_slice::<RunView>(&bytes).unwrap(),
                v,
                "S3_PROJECTION_FACTS"
            );
            fixtures.push(v);
        }
    }
    for fixture in &fixtures {
        if matches!(fixture.cleanup, RunCleanup::Blocked { .. })
            || matches!(
                fixture.outcome,
                RunOutcome::Ready | RunOutcome::Failed(_) | RunOutcome::Cancelled(_)
            )
        {
            assert!(
                !fixture.allowed_actions.contains(&RunActionKind::Continue),
                "S3_PROJECTION_EXCLUSIONS"
            );
        }
    }
    let record_ref = kinewright_project::bundle_records::ObjectRef {
        sha256: "a".repeat(64),
        byte_length: 3,
        record_kind: kinewright_project::bundle_records::RecordKind::OwnedEvidence,
    };
    let task_id = TaskId::fresh().unwrap();
    let attempt_id = AttemptId::fresh().unwrap();
    let question_id = QuestionId::fresh().unwrap();
    let task_states = vec![
        TaskState::Queued,
        TaskState::Preparing,
        TaskState::Running(attempt_id.clone()),
        TaskState::WaitingForQuestion(question_id.clone()),
        TaskState::WaitingForChildren(vec![TaskId::fresh().unwrap()]),
        TaskState::Freezing,
        TaskState::Cancelling,
        TaskState::CleanupBlocked("guardian".into()),
        TaskState::Terminal(record_ref.clone()),
    ];
    let attempts = [
        AttemptState::Reserved,
        AttemptState::Owned,
        AttemptState::Contained,
        AttemptState::Executing,
        AttemptState::Waiting,
        AttemptState::Stopping,
        AttemptState::CleanupBlocked("guardian".into()),
        AttemptState::Retired(reason.clone()),
    ];
    for state in task_states {
        let mut projection = view(run_id.clone());
        let task = TaskView {
            id: task_id.clone(),
            state,
            base: Some(record_ref.clone()),
            attempts: attempts
                .iter()
                .map(|state| AttemptView {
                    id: AttemptId::fresh().unwrap(),
                    state: state.clone(),
                    session: Some(AgentThreadId::fresh().unwrap()),
                    turn: 7,
                    guardian: Some(record_ref.clone()),
                })
                .collect(),
            effects: [
                EffectState::Pending,
                EffectState::Claimed(attempt_id.clone()),
                EffectState::OutcomeUnknown,
                EffectState::Settled,
                EffectState::Disposed,
            ]
            .into_iter()
            .enumerate()
            .map(|(i, state)| EffectView {
                id: EffectId::fresh().unwrap(),
                state,
                safety: [
                    EffectSafety::ReplaySafe,
                    EffectSafety::ProcessBound,
                    EffectSafety::ReconcileExternal,
                ][i % 3]
                    .clone(),
                record_ref: record_ref.clone(),
            })
            .collect(),
            deliveries: [
                DeliveryState::Pending,
                DeliveryState::Offered {
                    attempt: attempt_id.clone(),
                    message_id: "stable-message".into(),
                },
                DeliveryState::Observed {
                    run: run_id.clone(),
                },
                DeliveryState::Disposed,
            ]
            .into_iter()
            .map(|state| DeliveryView {
                id: DeliveryId::fresh().unwrap(),
                generation: 9,
                state,
                result_ref: record_ref.clone(),
            })
            .collect(),
            requests: vec![
                RuntimeRequest::Question(record_ref.clone()),
                RuntimeRequest::Approval(record_ref.clone()),
                RuntimeRequest::Secret(record_ref.clone()),
            ],
            frozen_result: Some(record_ref.clone()),
            terminal_result: Some(record_ref.clone()),
        };
        projection.current_task = Some(ActiveTaskViews {
            focused: task_id.clone(),
            tasks: Page {
                sequence: 1,
                rows: vec![task],
                next: Some("next-at-same-sequence".into()),
            },
        });
        projection.validate().unwrap();
        assert_eq!(
            serde_json::from_slice::<RunView>(&serde_json::to_vec(&projection).unwrap()).unwrap(),
            projection,
            "S3_TASK_MACHINE_FACTS"
        );
    }
    for waiting in [
        RunWait::Files(WaitDetail {
            reason: "missing source".into(),
            provider_reason: None,
            attempt: None,
            missing_refs: vec![record_ref],
        }),
        RunWait::Provider(WaitDetail {
            reason: "authentication".into(),
            provider_reason: Some(ProviderWaitReason::Authentication),
            attempt: Some(attempt_id),
            missing_refs: vec![],
        }),
        RunWait::Question(question_id),
        RunWait::Children(vec![task_id]),
    ] {
        let mut projection = view(run_id.clone());
        projection.wait = waiting;
        projection.cleanup = RunCleanup::Pending {
            identities: vec!["proof".into()],
        };
        assert_eq!(
            serde_json::from_slice::<RunView>(&serde_json::to_vec(&projection).unwrap()).unwrap(),
            projection,
            "S3_WAIT_CLEANUP_FACTS"
        );
    }
    let mut cursor = RunProjectionCursor::default();
    let mut paused = view(run_id.clone());
    paused.phase = RunPhase::Paused;
    paused.allowed_actions = vec![RunActionKind::Continue];
    cursor.refresh(paused.clone()).unwrap();
    let id: CommandId = serde_json::from_value(
        serde_json::json!({"issuer":"0123456789abcdef0123456789abcdef","sequence":1}),
    )
    .unwrap();
    let intent = RunIntent {
        command_id: id,
        run_id: run_id.clone(),
        expected_control_revision: paused.control_revision.clone(),
        action: RunAction::Continue,
    };
    assert!(cursor.can_submit(&intent));
    let mut next = paused.clone();
    next.sequence = 3;
    next.completed.sequence = 3;
    next.queued.sequence = 3;
    next.assumptions.sequence = 3;
    assert!(matches!(
        cursor.receive(RunEvent {
            run_id: run_id.clone(),
            sequence: 3,
            delta: RunViewDelta { view: next.clone() }
        }),
        Err(BundleError::HistoryChanged)
    ));
    assert!(!cursor.can_submit(&intent), "S3_PROJECTION_GAP");
    next.control_revision = OpaqueRevision("control-b".into());
    cursor.refresh(next).unwrap();
    assert!(!cursor.can_submit(&intent), "S3_PROJECTION_STALE");
    let mut blocked = paused.clone();
    blocked.cleanup = cleanup;
    blocked.outcome = RunOutcome::Ready;
    blocked.display_label = "Ready".into();
    cursor.refresh(blocked).unwrap();
    assert!(!cursor.can_submit(&intent), "S3_BLOCKED_CONTINUE_RECEIVER");
    cursor.refresh(paused.clone()).unwrap();
    let mut invalid = paused.clone();
    invalid.sequence = 2;
    invalid.control_revision = OpaqueRevision(String::new());
    invalid.completed.sequence = 2;
    invalid.queued.sequence = 2;
    invalid.assumptions.sequence = 2;
    assert!(
        cursor
            .receive(RunEvent {
                run_id,
                sequence: 2,
                delta: RunViewDelta { view: invalid }
            })
            .is_err()
    );
    assert!(!cursor.can_submit(&intent), "S3_INVALID_EVENT_REFRESH");
    cursor.refresh(paused.clone()).unwrap();
    assert!(cursor.can_submit(&intent));
    let mut too_many = paused;
    too_many.queued.rows = (0..201)
        .map(|_| TaskView {
            id: TaskId::fresh().unwrap(),
            state: TaskState::Preparing,
            base: None,
            attempts: vec![],
            effects: vec![],
            deliveries: vec![],
            requests: vec![],
            frozen_result: None,
            terminal_result: None,
        })
        .collect();
    assert!(too_many.validate().is_err());
}

#[path = "support/ja_s3_store_linux.rs"]
mod linux;
