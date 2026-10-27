//! C4 contract owner and the production session pump, pending H1 publication.
//!
//! Rust adaptation of <https://github.com/pingdotgg/t3code> at
//! 5d91ab1117ad51ef1288a94717d85c70e7af09fe, apps/server/src/orchestration-v2/
//! {Orchestrator,NotificationMailbox,DelegatedTaskQuestionReactor,ContextHandoffService}.ts.
//! Notice: LICENSES/t3code-MIT.txt. Kinewright tests exact identities and explicitly
//! separates contract acknowledgment from the missing durable H1 publisher.
#![allow(
    dead_code,
    reason = "exact C1 owner receiving ports compiled as fixture"
)]
#[path = "../src/orchestration/mod.rs"]
mod orchestration;
use kinewright_agent::{
    BudgetKind, ConfirmationPolicy, McpServer, PromptPoll, ScriptedDriver, ScriptedTurn,
    SessionCounters, SessionFlow, SessionLimits, SessionObserver, SessionStop, SharedCounters,
    pump_session,
};
use kinewright_core::{AgentDriver, AgentEvent, Core, Document, SessionConfig};
use kinewright_media::FfmpegMediaEngine;
use kinewright_project::{
    bundle_records::{ObjectRef, RecordKind},
    receipts,
};
use orchestration::{
    machine::Kernel, orchestration_commit::validate_projection, orchestration_records::*,
};
use std::{
    collections::VecDeque,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

fn id<T: serde::de::DeserializeOwned>(n: u64) -> T {
    serde_json::from_value(serde_json::json!(format!("{n:032x}"))).unwrap()
}
fn reference(n: u64, kind: RecordKind) -> ObjectRef {
    ObjectRef {
        sha256: format!("{n:064x}"),
        byte_length: n,
        record_kind: kind,
    }
}
fn initial() -> OrchestrationEnvelope {
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
            operations: std::collections::BTreeSet::default(),
            objects: std::collections::BTreeSet::default(),
            fields: std::collections::BTreeSet::default(),
            tracks: std::collections::BTreeSet::default(),
            bus_sidechain_closure: std::collections::BTreeSet::default(),
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
        digest: serde_json::from_value(serde_json::json!("0".repeat(64))).unwrap(),
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
struct ContractOwner {
    kernel: Kernel,
    envelope: OrchestrationEnvelope,
}
impl ContractOwner {
    fn new() -> Self {
        let envelope = initial();
        let mut kernel = Kernel::default();
        let receipt = kernel
            .propose(&envelope.project, 20, &envelope, None)
            .unwrap()
            .receipt()
            .clone();
        kernel.acknowledge(&receipt).unwrap();
        Self { kernel, envelope }
    }
    fn state(&self) -> &RunProjection {
        self.kernel.acknowledged().unwrap()
    }
    fn command_envelope(&self, command: OrchestrationCommand) -> OrchestrationEnvelope {
        let mut envelope = self.envelope.clone();
        envelope.id.sequence += 1;
        envelope.expected_generation += 1;
        envelope.authority.attempt = match &command {
            OrchestrationCommand::SettleExpiredBackground(_) => None,
            OrchestrationCommand::BeginTurn(turn)
            | OrchestrationCommand::EndTurn(turn)
            | OrchestrationCommand::BackgroundWork(turn, _) => Some(turn.attempt.clone()),
            OrchestrationCommand::Request(RuntimeRequest::Question(q)) => {
                Some(q.origin.attempt.clone())
            }
            _ => self.state().tasks[0]
                .progress
                .turn
                .as_ref()
                .map(|turn| turn.attempt.clone()),
        };
        envelope.command = command;
        envelope.digest = envelope.computed_digest().unwrap();
        envelope
    }
    fn apply(&mut self, command: OrchestrationCommand) -> Result<(), Refusal> {
        let envelope = self.command_envelope(command);
        let receipt = self
            .kernel
            .propose(
                &envelope.project,
                envelope.expected_generation,
                &envelope,
                Some(reference(88, RecordKind::RunIndex)),
            )?
            .receipt()
            .clone();
        self.kernel.acknowledge(&receipt)?;
        self.envelope = envelope;
        Ok(())
    }
    fn root(&self) -> TaskId {
        self.state().tasks[0].id.clone()
    }
    fn begin(&mut self, task: &TaskId, n: u64, purpose: TurnPurpose) -> TaskTurn {
        if !self.state().attempts.iter().any(|attempt| {
            &attempt.identity.task == task && !matches!(attempt.state, AttemptState::Retired(_))
        }) {
            // C2 fixture admission is separate from a C4 dependency reservation.
            let mut state = self.state().clone();
            state.attempts.push(AttemptReservation {
                identity: ExecutionIdentity {
                    version: 1,
                    project: self.envelope.project.clone(),
                    run: state.run.clone(),
                    task: task.clone(),
                    attempt: id(n),
                    nonce: id(n + 2000),
                    owner_instance: id(9000),
                },
                owner_epoch: 2,
                state: AttemptState::Reserved,
                recovery_ref: reference(n + 3000, RecordKind::OwnedData),
            });
            orchestration::tasks::refresh_cleanup(&mut state);
            self.kernel = Kernel::from_acknowledged(state).unwrap();
        }
        let record = self
            .state()
            .tasks
            .iter()
            .find(|record| &record.id == task)
            .unwrap();
        let attempt = self
            .state()
            .attempts
            .iter()
            .find(|a| &a.identity.task == task && !matches!(a.state, AttemptState::Retired(_)))
            .map_or_else(|| id(n), |a| a.identity.attempt.clone());
        let turn = TaskTurn {
            task: task.clone(),
            attempt,
            session: id(n + 1000),
            ordinal: record.progress.turn.as_ref().map_or(1, |t| t.ordinal + 1),
            owner_epoch: 2,
            expected_head: record.brief.scope.base.head.clone(),
            purpose,
        };
        self.apply(OrchestrationCommand::BeginTurn(turn.clone()))
            .unwrap();
        turn
    }
    fn assign(&mut self, parent: &TaskId, role: Role) -> Result<TaskId, Refusal> {
        let mut brief = self.state().tasks[0].brief.clone();
        brief.role = role;
        let command = OrchestrationCommand::AssignTask(Box::new(AssignTask {
            run: self.state().run.clone(),
            parent: parent.clone(),
            brief,
        }));
        let id = self.command_envelope(command.clone()).identity("task")?;
        self.apply(command)?;
        Ok(id)
    }
    fn result(&mut self, task: &TaskId, n: u64) -> Result<ObjectRef, Refusal> {
        let base = self
            .state()
            .tasks
            .iter()
            .find(|record| &record.id == task)
            .unwrap()
            .brief
            .scope
            .base
            .clone();
        let result = TaskResult {
            outcome: AttemptOutcome::Completed,
            base: Some(base.clone()),
            head: Some(base),
            delta_ref: None,
            artifacts: vec![reference(n, RecordKind::OwnedData)],
            evidence: vec![],
            usage_ref: reference(n + 1, RecordKind::RunIndex),
            explanation: reference(n + 2, RecordKind::OwnedData),
        };
        let bytes = encoded(
            &OrchestrationRecord {
                version: 1,
                payload: OrchestrationPayload::Result(Box::new(result.clone())),
            },
            CONTROL_BYTES,
        )?;
        let reference = ObjectRef {
            sha256: kinewright_media::sha256_bytes(&bytes),
            byte_length: bytes.len() as u64,
            record_kind: RecordKind::RunIndex,
        };
        let envelope = self.command_envelope(OrchestrationCommand::PublishResult(Box::new(
            PublishResult {
                task: task.clone(),
                result,
            },
        )));
        let receipt = self
            .kernel
            .propose_result(
                &envelope.project,
                envelope.expected_generation,
                &envelope,
                Some(crate::reference(89, RecordKind::RunIndex)),
                reference.clone(),
            )?
            .receipt()
            .clone();
        self.kernel.acknowledge(&receipt)?;
        self.envelope = envelope;
        Ok(reference)
    }
    fn question(&self, task: &TaskId, n: u64) -> QuestionRecord {
        QuestionRecord {
            id: id(n),
            task: task.clone(),
            prompt: reference(n, RecordKind::OwnedData),
            origin: self
                .state()
                .tasks
                .iter()
                .find(|record| &record.id == task)
                .unwrap()
                .progress
                .turn
                .clone()
                .unwrap(),
            status: QuestionStatus::Pending,
            wake: QuestionWake::Pending,
        }
    }
}
type Step = Box<dyn FnOnce(&mut ContractOwner)>;
struct Observer<'a> {
    owner: &'a mut ContractOwner,
    steps: VecDeque<Step>,
}
impl SessionObserver for Observer<'_> {
    fn prepare_studio_stop(
        &mut self,
        _: &SessionStop,
        _: Option<&kinewright_core::RuntimeRequestBinding>,
    ) -> Result<(), String> {
        validate_projection(self.owner.state()).map_err(|reason| format!("{reason:?}"))
    }
    fn on_event(&mut self, event: &AgentEvent) -> SessionFlow {
        if *event == AgentEvent::Done {
            self.steps.pop_front().unwrap()(self.owner);
        }
        SessionFlow::Continue
    }
    fn on_tick(&mut self, _: &SessionCounters) -> SessionFlow {
        SessionFlow::Continue
    }
    fn poll_prompt(&mut self) -> PromptPoll {
        if self.steps.is_empty() {
            PromptPoll::Closed
        } else {
            PromptPoll::Ready("task-linked follow-up".into())
        }
    }
}
fn drive(owner: &mut ContractOwner, steps: Vec<Step>) {
    let core = Core::spawn(Document::default()).unwrap();
    let media = Arc::new(FfmpegMediaEngine::new().unwrap());
    let server = McpServer::start(core, Arc::clone(&media) as _, media as _).unwrap();
    let script = (0..steps.len())
        .map(|i| ScriptedTurn::new(vec![], format!("intermediate-{i}")))
        .collect();
    let mut session = ScriptedDriver::new(script)
        .start_session(SessionConfig {
            mcp_url: Some(server.endpoint().into()),
            ..Default::default()
        })
        .unwrap();
    let events = session.events();
    let mut observer = Observer {
        owner,
        steps: steps.into(),
    };
    let (stop, counters) = pump_session(
        session.as_mut(),
        &events,
        &mut std::iter::once("start".into()),
        None,
        ConfirmationPolicy::Studio,
        &SessionLimits {
            max_turns: 80,
            max_wall_time: Duration::from_secs(10),
            max_tokens: None,
        },
        &Arc::new(SharedCounters::default()),
        &Arc::new(AtomicBool::new(false)),
        &mut observer,
    );
    assert_eq!(stop, SessionStop::Completed);
    assert!(counters.turns > 0);
    assert!(observer.steps.is_empty());
}

#[test]
fn s4_nested_capacity_refusal() {
    let mut owner = ContractOwner::new();
    let root = owner.root();
    owner.begin(&root, 21, TurnPurpose::Work);
    let child = owner.assign(&root, Role::Editor).unwrap();
    owner.begin(&child, 22, TurnPurpose::Work);
    let mut brief = owner.state().tasks[0].brief.clone();
    brief.role = Role::Motion;
    brief.scope.base.branch = id(14);
    owner
        .apply(OrchestrationCommand::AssignTask(Box::new(AssignTask {
            run: owner.state().run.clone(),
            parent: child.clone(),
            brief,
        })))
        .unwrap();
    let grandchild = owner.state().tasks[2].id.clone();
    owner.begin(&grandchild, 23, TurnPurpose::Work);
    let mut retained = owner.state().clone();
    retained.usage.push(reference(402, RecordKind::RunIndex));
    owner.kernel = Kernel::from_acknowledged(retained).unwrap();
    let before = owner.state().clone();
    drive(
        &mut owner,
        vec![Box::new(move |owner| {
            assert_eq!(
                owner.assign(&grandchild, Role::Reviewer),
                Err(Refusal::DependencyCapacity)
            );
            assert_eq!(
                owner.assign(&child, Role::Reviewer),
                Err(Refusal::DependencyCapacity)
            );
            assert_eq!(owner.state(), &before);
        })],
    );
}

#[test]
fn s4_nested_followup_readiness() {
    let mut owner = ContractOwner::new();
    let root = owner.root();
    owner.begin(&root, 21, TurnPurpose::Work);
    let child = owner.assign(&root, Role::Editor).unwrap();
    let turn = owner.begin(&child, 22, TurnPurpose::Work);
    let grandchild = owner.assign(&child, Role::Reviewer).unwrap();
    let grandturn = owner.begin(&grandchild, 23, TurnPurpose::Work);
    drive(
        &mut owner,
        vec![Box::new(move |owner| {
            owner.apply(OrchestrationCommand::EndTurn(turn)).unwrap();
            assert!(!orchestration::tasks::ready(owner.state(), &child));
            assert_eq!(
                owner.apply(OrchestrationCommand::FreezeTask(child.clone())),
                Err(Refusal::InvalidState)
            );
            let followup = owner.begin(&child, 24, TurnPurpose::Work);
            owner
                .apply(OrchestrationCommand::EndTurn(followup))
                .unwrap();
            assert!(!orchestration::tasks::ready(owner.state(), &child));
            owner
                .apply(OrchestrationCommand::EndTurn(grandturn))
                .unwrap();
            owner
                .apply(OrchestrationCommand::FreezeTask(grandchild.clone()))
                .unwrap();
            let result = owner.result(&grandchild, 110).unwrap();
            assert_eq!(
                owner
                    .state()
                    .tasks
                    .iter()
                    .find(|t| t.id == grandchild)
                    .unwrap()
                    .state,
                TaskState::Terminal(result)
            );
            assert!(!orchestration::tasks::ready(owner.state(), &child));
            let followup = owner.begin(&child, 25, TurnPurpose::Work);
            owner
                .apply(OrchestrationCommand::OfferDeliveries(
                    child.clone(),
                    followup.attempt.clone(),
                ))
                .unwrap();
            let delivery = owner.state().deliveries[0].id.clone();
            // An observation is authorized by the receiving parent, rather than the Director.
            let mut envelope =
                owner.command_envelope(OrchestrationCommand::ObserveDelivery(delivery));
            envelope.authority.attempt = Some(followup.attempt.clone());
            envelope.digest = envelope.computed_digest().unwrap();
            let receipt = owner
                .kernel
                .propose(
                    &envelope.project,
                    envelope.expected_generation,
                    &envelope,
                    Some(reference(88, RecordKind::RunIndex)),
                )
                .unwrap()
                .receipt()
                .clone();
            owner.kernel.acknowledge(&receipt).unwrap();
            owner.envelope = envelope;
            owner
                .apply(OrchestrationCommand::EndTurn(followup))
                .unwrap();
            assert!(orchestration::tasks::ready(owner.state(), &child));
            owner
                .apply(OrchestrationCommand::FreezeTask(child.clone()))
                .unwrap();
            owner.result(&child, 120).unwrap();
            assert_eq!(owner.state().tasks.len(), 3);
        })],
    );
}

#[test]
fn s4_wait_timeout_and_delivery_retry() {
    let mut owner = ContractOwner::new();
    let root = owner.root();
    let director = owner.begin(&root, 21, TurnPurpose::Work);
    let a = owner.assign(&root, Role::Reviewer).unwrap();
    let ta = owner.begin(&a, 22, TurnPurpose::Work);
    let b = owner.assign(&root, Role::Reviewer).unwrap();
    let tb = owner.begin(&b, 23, TurnPurpose::Work);
    drive(
        &mut owner,
        vec![Box::new(move |owner| {
            let before = owner.state().clone();
            owner
                .apply(OrchestrationCommand::WaitTimeout(root.clone()))
                .unwrap();
            assert_eq!(owner.state().tasks, before.tasks);
            assert_eq!(owner.state().deliveries, before.deliveries);
            assert_eq!(owner.state().attempts, before.attempts);
            for (task, turn, n) in [(&a, ta, 130), (&b, tb, 140)] {
                owner.apply(OrchestrationCommand::EndTurn(turn)).unwrap();
                owner
                    .apply(OrchestrationCommand::FreezeTask(task.clone()))
                    .unwrap();
                owner.result(task, n).unwrap();
            }
            let deliveries = owner.state().deliveries.clone();
            assert_eq!(deliveries[0].generation, deliveries[1].generation);
            assert_eq!(deliveries[0].message, deliveries[1].message);
            assert_ne!(deliveries[0].result, deliveries[1].result);
            owner
                .apply(OrchestrationCommand::OfferDeliveries(
                    root.clone(),
                    director.attempt.clone(),
                ))
                .unwrap();
            let offered = owner.state().deliveries.clone();
            owner
                .apply(OrchestrationCommand::OfferDeliveries(
                    root.clone(),
                    director.attempt,
                ))
                .unwrap();
            assert_eq!(owner.state().deliveries, offered);
            for delivery in deliveries {
                owner
                    .apply(OrchestrationCommand::ObserveDelivery(delivery.id))
                    .unwrap();
            }
            assert!(
                owner
                    .state()
                    .deliveries
                    .iter()
                    .all(|d| matches!(d.state, DeliveryState::Observed(_)))
            );
        })],
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "stale admission and execution share a production pump witness"
)]
fn s4_stale_question_and_approval() {
    let mut owner = ContractOwner::new();
    let root = owner.root();
    owner.begin(&root, 21, TurnPurpose::Work);
    let child = owner.assign(&root, Role::Editor).unwrap();
    let turn = owner.begin(&child, 22, TurnPurpose::Work);
    drive(
        &mut owner,
        vec![Box::new(move |owner| {
            let q = owner.question(&child, 161);
            owner
                .apply(OrchestrationCommand::Request(RuntimeRequest::Question(
                    q.clone(),
                )))
                .unwrap();
            owner
                .apply(OrchestrationCommand::AnswerQuestion(AnswerQuestion {
                    question: q.id.clone(),
                    answer: reference(162, RecordKind::OwnedData),
                }))
                .unwrap();
            assert_eq!(
                owner.apply(OrchestrationCommand::AdmitQuestionWake(q.id)),
                Err(Refusal::InvalidState)
            );
            let q = owner.question(&child, 163);
            owner
                .apply(OrchestrationCommand::Request(RuntimeRequest::Question(
                    q.clone(),
                )))
                .unwrap();
            owner
                .apply(OrchestrationCommand::AdmitQuestionWake(q.id.clone()))
                .unwrap();
            owner
                .apply(OrchestrationCommand::AnswerQuestion(AnswerQuestion {
                    question: q.id.clone(),
                    answer: reference(164, RecordKind::OwnedData),
                }))
                .unwrap();
            assert_eq!(
                owner.apply(OrchestrationCommand::ExecuteQuestionWake(q.id)),
                Err(Refusal::InvalidState)
            );
            let live = owner.question(&child, 168);
            owner
                .apply(OrchestrationCommand::Request(RuntimeRequest::Question(
                    live.clone(),
                )))
                .unwrap();
            owner
                .apply(OrchestrationCommand::AdmitQuestionWake(live.id.clone()))
                .unwrap();
            owner
                .apply(OrchestrationCommand::ExecuteQuestionWake(live.id.clone()))
                .unwrap();
            assert!(owner.state().requests.iter().any(|request| matches!(request, RuntimeRequest::Question(q) if q.id == live.id && q.wake == QuestionWake::Executed)));
            owner
                .apply(OrchestrationCommand::AnswerQuestion(AnswerQuestion {
                    question: live.id,
                    answer: reference(169, RecordKind::OwnedData),
                }))
                .unwrap();
            assert!(
                owner.state().tasks[0]
                    .progress
                    .decisions
                    .contains(&reference(169, RecordKind::OwnedData))
            );
            let q = owner.question(&child, 165);
            owner
                .apply(OrchestrationCommand::Request(RuntimeRequest::Question(
                    q.clone(),
                )))
                .unwrap();
            owner
                .apply(OrchestrationCommand::AdmitQuestionWake(q.id.clone()))
                .unwrap();
            owner
                .apply(OrchestrationCommand::CancelTask(child.clone()))
                .unwrap();
            assert_eq!(
                owner.apply(OrchestrationCommand::ExecuteQuestionWake(q.id)),
                Err(Refusal::InvalidState)
            );
            assert_eq!(
                owner.apply(OrchestrationCommand::EndTurn(turn.clone())),
                Err(Refusal::InvalidState)
            );
            for request in [
                RuntimeRequest::Approval(AttentionReference {
                    task: child.clone(),
                    attempt: turn.attempt.clone(),
                    request_ref: reference(166, RecordKind::OwnedData),
                }),
                RuntimeRequest::Secret(AttentionReference {
                    task: child.clone(),
                    attempt: turn.attempt.clone(),
                    request_ref: reference(167, RecordKind::OwnedData),
                }),
            ] {
                assert_eq!(
                    owner.apply(OrchestrationCommand::Request(request)),
                    Err(Refusal::H3AttentionUnavailable)
                );
            }
        })],
    );
}

#[test]
fn s4_background_cancel_reopen() {
    let mut owner = ContractOwner::new();
    let root = owner.root();
    let turn = owner.begin(&root, 21, TurnPurpose::Work);
    drive(
        &mut owner,
        vec![Box::new(move |owner| {
            owner
                .apply(OrchestrationCommand::BackgroundWork(turn.clone(), 1))
                .unwrap();
            owner
                .apply(OrchestrationCommand::EndTurn(turn.clone()))
                .unwrap();
            assert!(!orchestration::tasks::ready(owner.state(), &root));
            owner
                .apply(OrchestrationCommand::CancelTask(root.clone()))
                .unwrap();
            let bytes = encoded(owner.state(), CONTROL_BYTES).unwrap();
            let state: RunProjection = serde_json::from_slice(&bytes).unwrap();
            validate_projection(&state).unwrap();
            owner.kernel = Kernel::from_acknowledged(state).unwrap();
            assert_eq!(owner.state().tasks[0].state, TaskState::Cancelling);
            assert_eq!(owner.state().attempts[0].state, AttemptState::Stopping);
            assert_ne!(owner.state().cleanup, RunCleanup::Settled);
            let before = owner.state().clone();
            assert_eq!(
                owner.apply(OrchestrationCommand::BackgroundWork(turn, 0)),
                Err(Refusal::InvalidState)
            );
            assert_eq!(owner.state(), &before);
        })],
    );
}

#[test]
fn s4_question_attention_and_late_result_preserve_identity() {
    let mut owner = ContractOwner::new();
    let root = owner.root();
    owner.begin(&root, 21, TurnPurpose::Work);
    let child = owner.assign(&root, Role::Editor).unwrap();
    let old = owner.begin(&child, 22, TurnPurpose::QuestionAttention);
    drive(
        &mut owner,
        vec![Box::new(move |owner| {
            owner
                .apply(OrchestrationCommand::EndTurn(old.clone()))
                .unwrap();
            assert!(!orchestration::tasks::ready(owner.state(), &child));
            assert_eq!(
                owner.apply(OrchestrationCommand::FreezeTask(child.clone())),
                Err(Refusal::InvalidState)
            );
            let current = owner.begin(&child, 23, TurnPurpose::Work);
            let before = owner.state().clone();
            assert_eq!(
                owner.apply(OrchestrationCommand::EndTurn(old)),
                Err(Refusal::InvalidState)
            );
            assert_eq!(owner.state(), &before);
            owner.apply(OrchestrationCommand::EndTurn(current)).unwrap();
            owner
                .apply(OrchestrationCommand::FreezeTask(child.clone()))
                .unwrap();
            let result = owner.result(&child, 180).unwrap();
            let before = owner.state().clone();
            assert_eq!(owner.result(&child, 190), Err(Refusal::InvalidState));
            assert_eq!(owner.state(), &before);
            assert_eq!(owner.state().tasks[1].state, TaskState::Terminal(result));
        })],
    );
}

#[test]
fn s4_pause_stop_and_restart_brief() {
    use orchestration::context::{NativeInjection, RemainingLimits, RestartBrief};
    let mut owner = ContractOwner::new();
    let root = owner.root();
    let turn = owner.begin(&root, 21, TurnPurpose::Work);
    drive(
        &mut owner,
        vec![Box::new(move |owner| {
            let intent = RunIntent {
                command_id: owner
                    .command_envelope(OrchestrationCommand::WaitTimeout(root.clone()))
                    .id,
                run_id: owner.state().run.clone(),
                expected_control_revision: owner.state().control_revision.clone(),
                action: RunAction::Pause,
            };
            owner
                .apply(OrchestrationCommand::ControlRun(intent))
                .unwrap();
            assert!(
                orchestration::outbox::next_effect(
                    owner.state(),
                    &owner.state().tasks[0].brief.scope.base.branch
                )
                .is_none()
            );
            assert_eq!(
                owner.assign(&root, Role::Editor),
                Err(Refusal::InvalidState)
            );
            owner.apply(OrchestrationCommand::EndTurn(turn)).unwrap();
            assert_eq!(owner.state().phase, RunPhase::Paused);
            let remaining = RemainingLimits {
                sends: 79,
                elapsed_millis: 42,
                token_ceiling: Some(1234),
                repairs: 2,
                rate_waits: 1,
            };
            owner
                .apply(OrchestrationCommand::RecordContext {
                    task: root.clone(),
                    feedback: vec![reference(201, RecordKind::OwnedData)],
                    decisions: vec![reference(202, RecordKind::OwnedData)],
                })
                .unwrap();
            let brief = RestartBrief::build(owner.state(), &root, remaining.clone()).unwrap();
            assert_eq!(brief.assignment, owner.state().tasks[0].brief);
            assert_eq!(brief.remaining, remaining);
            assert_eq!(brief.feedback[0], reference(201, RecordKind::OwnedData));
            assert_eq!(
                owner.apply(OrchestrationCommand::RecordContext {
                    task: root.clone(),
                    feedback: vec![reference(201, RecordKind::OwnedData); ITEM_LIMIT + 1],
                    decisions: vec![]
                }),
                Err(Refusal::Capacity)
            );
            let mut injection = NativeInjection::Fresh;
            injection.begin().unwrap();
            injection.finish(false).unwrap();
            assert_eq!(injection.begin(), Err(Refusal::InvalidState));
            let intent = RunIntent {
                command_id: owner
                    .command_envelope(OrchestrationCommand::WaitTimeout(root.clone()))
                    .id,
                run_id: owner.state().run.clone(),
                expected_control_revision: owner.state().control_revision.clone(),
                action: RunAction::Stop,
            };
            owner
                .apply(OrchestrationCommand::ControlRun(intent))
                .unwrap();
            assert!(matches!(owner.state().outcome, RunOutcome::Cancelled(_)));
            assert_ne!(owner.state().cleanup, RunCleanup::Settled);
            assert!(!orchestration::machine::eligible(
                owner.state(),
                &RunAction::Continue
            ));
        })],
    );
}

#[test]
fn s4_cancel_preserves_published_output_and_expires_session() {
    let mut owner = ContractOwner::new();
    let root = owner.root();
    owner.begin(&root, 21, TurnPurpose::Work);
    let child = owner.assign(&root, Role::Reviewer).unwrap();
    let turn = owner.begin(&child, 22, TurnPurpose::Work);
    drive(
        &mut owner,
        vec![Box::new(move |owner| {
            let question = owner.question(&child, 210);
            owner
                .apply(OrchestrationCommand::Request(RuntimeRequest::Question(
                    question.clone(),
                )))
                .unwrap();
            owner
                .apply(OrchestrationCommand::ExpireSession(turn.session))
                .unwrap();
            assert_eq!(
                owner.apply(OrchestrationCommand::AdmitQuestionWake(question.id)),
                Err(Refusal::InvalidState)
            );
            let turn = owner.begin(&child, 23, TurnPurpose::Work);
            owner.apply(OrchestrationCommand::EndTurn(turn)).unwrap();
            owner
                .apply(OrchestrationCommand::FreezeTask(child.clone()))
                .unwrap();
            let result = owner.result(&child, 220).unwrap();
            owner.apply(OrchestrationCommand::CancelTask(root)).unwrap();
            assert_eq!(
                owner.state().tasks[1].state,
                TaskState::Terminal(result.clone())
            );
            assert!(owner.state().results.contains(&result));
            assert_eq!(owner.state().deliveries[0].state, DeliveryState::Disposed);
        })],
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "real pump exercises each live response boundary"
)]
fn s4_runtime_request_freshness() {
    use kinewright_core::{
        AgentError, AgentSession, RuntimeRequest as LiveRequest, RuntimeRequestBinding,
        RuntimeRequestKind, RuntimeResponse,
    };
    use std::sync::Mutex;
    struct Session {
        binding: Arc<Mutex<Option<RuntimeRequestBinding>>>,
        requests: VecDeque<LiveRequest>,
        responses: Vec<RuntimeResponse>,
        tx: crossbeam_channel::Sender<AgentEvent>,
        rx: crossbeam_channel::Receiver<AgentEvent>,
        interrupts: usize,
    }
    impl AgentSession for Session {
        fn send_user_message(&mut self, _: String) -> Result<(), AgentError> {
            self.tx.send(AgentEvent::Done).unwrap();
            Ok(())
        }
        fn events(&self) -> crossbeam_channel::Receiver<AgentEvent> {
            self.rx.clone()
        }
        fn interrupt(&mut self) {
            self.interrupts += 1;
        }
        fn poll_runtime_request(&mut self) -> Result<Option<LiveRequest>, AgentError> {
            Ok(self.requests.pop_front())
        }
        fn runtime_request_binding(&self) -> Option<RuntimeRequestBinding> {
            self.binding.lock().unwrap().clone()
        }
        fn respond_runtime_request(
            &mut self,
            request: &LiveRequest,
            response: RuntimeResponse,
        ) -> Result<(), AgentError> {
            response.validate(request)?;
            self.responses.push(response);
            Ok(())
        }
    }
    struct Answer {
        binding: Arc<Mutex<Option<RuntimeRequestBinding>>>,
        revoke: bool,
        called: usize,
    }
    impl SessionObserver for Answer {
        fn prepare_studio_stop(
            &mut self,
            _: &SessionStop,
            _: Option<&RuntimeRequestBinding>,
        ) -> Result<(), String> {
            Ok(())
        }
        fn on_event(&mut self, _: &AgentEvent) -> SessionFlow {
            SessionFlow::Continue
        }
        fn on_tick(&mut self, _: &SessionCounters) -> SessionFlow {
            SessionFlow::Continue
        }
        fn on_runtime_request(&mut self, _: &LiveRequest) -> RuntimeResponse {
            self.called += 1;
            if self.revoke {
                *self.binding.lock().unwrap() = None;
            }
            RuntimeResponse::QuestionAnswer("reversible recorded assumption".into())
        }
    }
    for (kind, revoke) in [
        (RuntimeRequestKind::Question, false),
        (RuntimeRequestKind::Question, true),
        (RuntimeRequestKind::Approval, false),
        (RuntimeRequestKind::Secret, false),
    ] {
        let binding = RuntimeRequestBinding {
            task: format!("{:032x}", 1),
            attempt: format!("{:032x}", 2),
            session: format!("{:032x}", 3),
            turn: 1,
            owner_epoch: 2,
        };
        let request = LiveRequest::new(
            "request-1".into(),
            binding.clone(),
            kind,
            "nonsecret reference".into(),
        )
        .unwrap();
        assert!(
            RuntimeResponse::QuestionAnswer("x".repeat(16 * 1024 + 1))
                .validate(&request)
                .is_err()
        );
        if kind != RuntimeRequestKind::Question {
            assert!(
                RuntimeResponse::QuestionAnswer("yes".into())
                    .validate(&request)
                    .is_err()
            );
        }
        let binding = Arc::new(Mutex::new(Some(binding)));
        let (tx, rx) = crossbeam_channel::bounded(4);
        let mut session = Session {
            binding: binding.clone(),
            requests: [request].into(),
            responses: vec![],
            tx,
            rx,
            interrupts: 0,
        };
        let events = session.events();
        let mut observer = Answer {
            binding,
            revoke,
            called: 0,
        };
        let (stop, _) = pump_session(
            &mut session,
            &events,
            &mut std::iter::once("run".into()),
            None,
            ConfirmationPolicy::Studio,
            &SessionLimits {
                max_turns: 2,
                max_wall_time: Duration::from_secs(2),
                max_tokens: None,
            },
            &Arc::new(SharedCounters::default()),
            &Arc::new(AtomicBool::new(false)),
            &mut observer,
        );
        assert_eq!(stop, SessionStop::Completed);
        assert_eq!(session.interrupts, 1);
        if revoke {
            assert!(session.responses.is_empty());
        } else if kind == RuntimeRequestKind::Question {
            assert_eq!(
                session.responses,
                vec![RuntimeResponse::QuestionAnswer(
                    "reversible recorded assumption".into()
                )]
            );
        } else {
            assert_eq!(observer.called, 0);
            assert_eq!(
                session.responses,
                vec![RuntimeResponse::H3AttentionUnavailable]
            );
        }
    }
}

#[test]
fn s4_idle_pump_keeps_deadline_ticks() {
    struct Pending;
    impl SessionObserver for Pending {
        fn prepare_studio_stop(
            &mut self,
            _: &SessionStop,
            _: Option<&kinewright_core::RuntimeRequestBinding>,
        ) -> Result<(), String> {
            Ok(())
        }
        fn on_event(&mut self, _: &AgentEvent) -> SessionFlow {
            SessionFlow::Continue
        }
        fn on_tick(&mut self, _: &SessionCounters) -> SessionFlow {
            SessionFlow::Continue
        }
        fn poll_prompt(&mut self) -> PromptPoll {
            PromptPoll::Pending
        }
    }
    struct Idle(crossbeam_channel::Receiver<AgentEvent>, usize);
    impl kinewright_core::AgentSession for Idle {
        fn send_user_message(&mut self, _: String) -> Result<(), kinewright_core::AgentError> {
            panic!("idle source sent a turn")
        }
        fn events(&self) -> crossbeam_channel::Receiver<AgentEvent> {
            self.0.clone()
        }
        fn interrupt(&mut self) {
            self.1 += 1;
        }
    }
    let (_tx, rx) = crossbeam_channel::bounded(1);
    let mut session = Idle(rx.clone(), 0);
    let (stop, spent) = pump_session(
        &mut session,
        &rx,
        &mut std::iter::empty(),
        None,
        ConfirmationPolicy::Studio,
        &SessionLimits {
            max_turns: 80,
            max_wall_time: Duration::from_millis(20),
            max_tokens: None,
        },
        &Arc::new(SharedCounters::default()),
        &Arc::new(AtomicBool::new(false)),
        &mut Pending,
    );
    assert_eq!(stop, SessionStop::Budget(BudgetKind::WallTime));
    assert_eq!(spent.turns, 0);
    assert_eq!(session.1, 1);
}

#[test]
#[ignore = "H1 durable owner publication and reopen are not merged"]
fn s4_durable_owner_lifecycle_reopen() {
    s4_background_cancel_reopen();
    panic!("H1 prerequisite missing: contract replay cannot certify durable owner reopen");
}
#[test]
#[ignore = "H3 person-facing holder and attachment roles are not merged"]
fn s4_person_holder_approval_and_secret() {
    s4_runtime_request_freshness();
    panic!("H3 prerequisite missing: live holder callback cannot be certified");
}

#[test]
fn s4_control_source_is_bounded_and_nonblocking() {
    let (sender, mut queue) = kinewright_agent::studio_prompt_queue();
    assert!(matches!(queue.poll_prompt(), PromptPoll::Pending));
    for _ in 0..64 {
        sender.try_send("after the active turn".into()).unwrap();
    }
    assert!(sender.try_send("over capacity".into()).is_err());
    assert!(sender.try_send("x".repeat(COMMAND_BYTES + 1)).is_err());
    for _ in 0..64 {
        assert!(
            matches!(queue.poll_prompt(), PromptPoll::Ready(text) if text == "after the active turn")
        );
    }
    assert!(matches!(queue.poll_prompt(), PromptPoll::Pending));
    drop(sender);
    assert!(matches!(queue.poll_prompt(), PromptPoll::Closed));
}

#[test]
fn s4_cancel_intent_precedes_native_stop() {
    use std::sync::atomic::Ordering;
    struct CancelOwner {
        owner: ContractOwner,
        acknowledged: Arc<AtomicBool>,
        unknown: bool,
    }
    impl SessionObserver for CancelOwner {
        fn on_event(&mut self, _: &AgentEvent) -> SessionFlow {
            SessionFlow::Continue
        }
        fn on_tick(&mut self, _: &SessionCounters) -> SessionFlow {
            SessionFlow::Continue
        }
        fn prepare_studio_stop(
            &mut self,
            _: &SessionStop,
            _: Option<&kinewright_core::RuntimeRequestBinding>,
        ) -> Result<(), String> {
            if self.unknown {
                return Err("publication_unknown".into());
            }
            self.owner
                .apply(OrchestrationCommand::CancelTask(self.owner.root()))
                .unwrap();
            assert_eq!(self.owner.state().tasks[0].state, TaskState::Cancelling);
            self.acknowledged.store(true, Ordering::SeqCst);
            Ok(())
        }
    }
    struct NativeStop {
        acknowledged: Arc<AtomicBool>,
        interrupts: usize,
        events: crossbeam_channel::Receiver<AgentEvent>,
    }
    impl kinewright_core::AgentSession for NativeStop {
        fn send_user_message(&mut self, _: String) -> Result<(), kinewright_core::AgentError> {
            panic!("cancelled session sent a turn");
        }
        fn events(&self) -> crossbeam_channel::Receiver<AgentEvent> {
            self.events.clone()
        }
        fn interrupt(&mut self) {
            assert!(
                self.acknowledged.load(Ordering::SeqCst),
                "native stop preceded acknowledged intent"
            );
            self.interrupts += 1;
        }
    }
    for unknown in [false, true] {
        let mut owner = ContractOwner::new();
        let root = owner.root();
        owner.begin(&root, 21, TurnPurpose::Work);
        let acknowledged = Arc::new(AtomicBool::new(false));
        let (_tx, events) = crossbeam_channel::bounded(1);
        let mut session = NativeStop {
            acknowledged: acknowledged.clone(),
            interrupts: 0,
            events: events.clone(),
        };
        let mut observer = CancelOwner {
            owner,
            acknowledged,
            unknown,
        };
        let (stop, _) = pump_session(
            &mut session,
            &events,
            &mut std::iter::empty(),
            None,
            ConfirmationPolicy::Studio,
            &SessionLimits {
                max_turns: 80,
                max_wall_time: Duration::from_secs(2),
                max_tokens: None,
            },
            &Arc::new(SharedCounters::default()),
            &Arc::new(AtomicBool::new(true)),
            &mut observer,
        );
        assert!(matches!(stop, SessionStop::Cancelled(_)));
        assert_eq!(session.interrupts, usize::from(!unknown));
        assert_ne!(observer.owner.state().cleanup, RunCleanup::Settled);
    }
}

#[test]
fn s4_candidate_writer_dependency_refuses_before_edge() {
    let mut owner = ContractOwner::new();
    let root = owner.root();
    owner.begin(&root, 21, TurnPurpose::Work);
    let child = owner.assign(&root, Role::Editor).unwrap();
    owner.begin(&child, 22, TurnPurpose::Work);
    let before = owner.state().clone();
    assert_eq!(
        owner.assign(&child, Role::Audio),
        Err(Refusal::DependencyCapacity)
    );
    assert_eq!(owner.state(), &before);
}

#[test]
fn s4_expired_turn_never_manufactures_readiness() {
    let mut owner = ContractOwner::new();
    let root = owner.root();
    owner.begin(&root, 21, TurnPurpose::Work);
    let child = owner.assign(&root, Role::Editor).unwrap();
    let turn = owner.begin(&child, 22, TurnPurpose::Work);
    drive(
        &mut owner,
        vec![Box::new(move |owner| {
            owner
                .apply(OrchestrationCommand::ExpireSession(turn.session.clone()))
                .unwrap();
            let before = owner.state().clone();
            assert!(!orchestration::tasks::ready(owner.state(), &child));
            assert_eq!(
                owner.apply(OrchestrationCommand::FreezeTask(child.clone())),
                Err(Refusal::InvalidState)
            );
            assert_eq!(
                owner.apply(OrchestrationCommand::BackgroundWork(turn.clone(), 0)),
                Err(Refusal::InvalidState)
            );
            assert_eq!(
                owner.apply(OrchestrationCommand::EndTurn(turn)),
                Err(Refusal::InvalidState)
            );
            assert_eq!(owner.state(), &before);
            let fresh = owner.begin(&child, 23, TurnPurpose::Work);
            assert!(!orchestration::tasks::ready(owner.state(), &child));
            owner.apply(OrchestrationCommand::EndTurn(fresh)).unwrap();
            owner
                .apply(OrchestrationCommand::FreezeTask(child.clone()))
                .unwrap();
            owner.result(&child, 250).unwrap();
        })],
    );
}

#[test]
fn s4_followup_waits_for_identified_background_completion() {
    let mut owner = ContractOwner::new();
    let root = owner.root();
    let turn = owner.begin(&root, 21, TurnPurpose::Work);
    drive(
        &mut owner,
        vec![Box::new(move |owner| {
            owner
                .apply(OrchestrationCommand::BackgroundWork(turn.clone(), 1))
                .unwrap();
            owner
                .apply(OrchestrationCommand::EndTurn(turn.clone()))
                .unwrap();
            let before = owner.state().clone();
            let mut next = turn.clone();
            next.ordinal += 1;
            next.session = id(31);
            assert_eq!(
                owner.apply(OrchestrationCommand::BeginTurn(next)),
                Err(Refusal::InvalidState)
            );
            assert_eq!(owner.state(), &before);
            owner
                .apply(OrchestrationCommand::BackgroundWork(turn, 0))
                .unwrap();
            let next = owner.begin(&root, 32, TurnPurpose::Work);
            owner.apply(OrchestrationCommand::EndTurn(next)).unwrap();
            assert!(orchestration::tasks::ready(owner.state(), &root));
        })],
    );
}

#[test]
fn s4_observed_and_cancelled_siblings_settle_one_wake() {
    let mut owner = ContractOwner::new();
    let root = owner.root();
    let director = owner.begin(&root, 21, TurnPurpose::Work);
    let a = owner.assign(&root, Role::Reviewer).unwrap();
    let ta = owner.begin(&a, 22, TurnPurpose::Work);
    let b = owner.assign(&root, Role::Reviewer).unwrap();
    let tb = owner.begin(&b, 23, TurnPurpose::Work);
    drive(
        &mut owner,
        vec![Box::new(move |owner| {
            for (task, turn, n) in [(&a, ta, 270), (&b, tb, 280)] {
                owner.apply(OrchestrationCommand::EndTurn(turn)).unwrap();
                owner
                    .apply(OrchestrationCommand::FreezeTask(task.clone()))
                    .unwrap();
                owner.result(task, n).unwrap();
            }
            let deliveries = owner.state().deliveries.clone();
            assert_eq!(deliveries[0].wake, deliveries[1].wake);
            owner
                .apply(OrchestrationCommand::OfferDeliveries(
                    root.clone(),
                    director.attempt,
                ))
                .unwrap();
            owner
                .apply(OrchestrationCommand::ObserveDelivery(
                    deliveries[0].id.clone(),
                ))
                .unwrap();
            owner.apply(OrchestrationCommand::CancelTask(b)).unwrap();
            assert!(matches!(
                owner.state().deliveries[0].state,
                DeliveryState::Observed(_)
            ));
            assert_eq!(owner.state().deliveries[1].state, DeliveryState::Disposed);
            let wake = owner
                .state()
                .effects
                .iter()
                .find(|effect| effect.id == deliveries[0].wake)
                .unwrap();
            assert_eq!(wake.state, EffectState::Settled);
            let mut state = owner.state().clone();
            for effect in &mut state.effects {
                if effect.id != wake.id {
                    effect.state = EffectState::Settled;
                }
            }
            let mut later = wake.clone();
            later.id = id(300);
            later.order += 1;
            later.state = EffectState::Pending;
            state.effects.push(later.clone());
            assert_eq!(
                orchestration::outbox::next_effect(&state, &later.lane),
                Some(&later)
            );
        })],
    );
}

#[test]
fn s4_async_question_response_uses_acknowledged_owner() {
    async_question_response_cases(&[0, 1, 2, 3]);
}

#[test]
fn fu2_repeated_pending_request_uses_acknowledged_owner() {
    async_question_response_cases(&[4]);
}

#[allow(
    clippy::too_many_lines,
    reason = "asynchronous owner acknowledgment and repeated polls share the real pump fixture"
)]
fn async_question_response_cases(modes: &[u8]) {
    use kinewright_core::{
        AgentError, AgentSession, RuntimeRequest as LiveRequest, RuntimeRequestBinding,
        RuntimeRequestKind, RuntimeResponse, RuntimeResponseControl,
    };
    use std::sync::{Mutex, atomic::Ordering};
    struct Session {
        binding: Arc<Mutex<Option<RuntimeRequestBinding>>>,
        requests: VecDeque<LiveRequest>,
        polls: usize,
        acknowledged: Arc<AtomicBool>,
        responses: usize,
        tx: crossbeam_channel::Sender<AgentEvent>,
        rx: crossbeam_channel::Receiver<AgentEvent>,
    }
    impl AgentSession for Session {
        fn send_user_message(&mut self, _: String) -> Result<(), AgentError> {
            Ok(())
        }
        fn events(&self) -> crossbeam_channel::Receiver<AgentEvent> {
            self.rx.clone()
        }
        fn interrupt(&mut self) {
            assert!(self.acknowledged.load(Ordering::SeqCst));
        }
        fn poll_runtime_request(&mut self) -> Result<Option<LiveRequest>, AgentError> {
            let request = self.requests.pop_front();
            self.polls += usize::from(request.is_some());
            Ok(request)
        }
        fn runtime_request_binding(&self) -> Option<RuntimeRequestBinding> {
            self.binding.lock().unwrap().clone()
        }
        fn respond_runtime_request(
            &mut self,
            request: &LiveRequest,
            response: RuntimeResponse,
        ) -> Result<(), AgentError> {
            assert!(
                self.acknowledged.load(Ordering::SeqCst),
                "response preceded the C1 owner acknowledgment"
            );
            response.validate(request)?;
            self.responses += 1;
            self.tx.send(AgentEvent::Done).unwrap();
            Ok(())
        }
    }
    struct Director {
        owner: ContractOwner,
        child: TaskId,
        request: Option<LiveRequest>,
        ticks: u32,
        mode: u8,
        acknowledged: Arc<AtomicBool>,
        binding: Arc<Mutex<Option<RuntimeRequestBinding>>>,
        cancel: Arc<AtomicBool>,
        sender: kinewright_agent::StudioResponseSender,
        queue: kinewright_agent::StudioResponseQueue,
    }
    impl SessionObserver for Director {
        fn on_event(&mut self, _: &AgentEvent) -> SessionFlow {
            SessionFlow::Continue
        }
        fn on_runtime_request(&mut self, request: &LiveRequest) -> RuntimeResponse {
            let question = self.owner.question(&self.child, 320);
            self.owner
                .apply(OrchestrationCommand::Request(RuntimeRequest::Question(
                    question,
                )))
                .unwrap();
            self.request = Some(request.clone());
            RuntimeResponse::Pending
        }
        fn on_tick(&mut self, _: &SessionCounters) -> SessionFlow {
            self.ticks += 1;
            if self.ticks == 3
                && let Some(request) = self.request.take()
            {
                if self.mode == 1 {
                    self.cancel.store(true, Ordering::SeqCst);
                } else if self.mode != 3 {
                    self.owner
                        .apply(OrchestrationCommand::AnswerQuestion(AnswerQuestion {
                            question: id(320),
                            answer: reference(321, RecordKind::OwnedData),
                        }))
                        .unwrap();
                    self.acknowledged.store(true, Ordering::SeqCst);
                    self.sender
                        .try_send(
                            RuntimeResponseControl::new(
                                request,
                                RuntimeResponse::QuestionAnswer("recorded assumption".into()),
                            )
                            .unwrap(),
                        )
                        .unwrap();
                    if self.mode == 2 {
                        *self.binding.lock().unwrap() = None;
                    }
                }
            }
            if self.mode == 2 && self.ticks >= 5 {
                self.cancel.store(true, Ordering::SeqCst);
            }
            SessionFlow::Continue
        }
        fn poll_runtime_response(&mut self) -> Option<RuntimeResponseControl> {
            self.queue.poll_response()
        }
        fn prepare_studio_stop(
            &mut self,
            _: &SessionStop,
            _: Option<&RuntimeRequestBinding>,
        ) -> Result<(), String> {
            self.owner
                .apply(OrchestrationCommand::CancelTask(self.owner.root()))
                .unwrap();
            self.acknowledged.store(true, Ordering::SeqCst);
            Ok(())
        }
    }
    fn text_id<T: serde::Serialize>(id: &T) -> String {
        serde_json::to_value(id).unwrap().as_str().unwrap().into()
    }
    for &mode in modes {
        let mut owner = ContractOwner::new();
        let root = owner.root();
        owner.begin(&root, 21, TurnPurpose::Work);
        let child = owner.assign(&root, Role::Editor).unwrap();
        let turn = owner.begin(&child, 22, TurnPurpose::Work);
        let binding = RuntimeRequestBinding {
            task: text_id(&child),
            attempt: text_id(&turn.attempt),
            session: text_id(&turn.session),
            turn: turn.ordinal,
            owner_epoch: turn.owner_epoch,
        };
        let request = LiveRequest::new(
            "question-320".into(),
            binding.clone(),
            RuntimeRequestKind::Question,
            "choose a reversible assumption".into(),
        )
        .unwrap();
        let binding = Arc::new(Mutex::new(Some(binding)));
        let acknowledged = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = crossbeam_channel::bounded(4);
        let (sender, queue) = kinewright_agent::studio_response_queue();
        let mut session = Session {
            binding: binding.clone(),
            requests: if mode == 4 {
                VecDeque::from([request.clone(), request])
            } else {
                VecDeque::from([request])
            },
            polls: 0,
            acknowledged: acknowledged.clone(),
            responses: 0,
            tx,
            rx: rx.clone(),
        };
        let mut observer = Director {
            owner,
            child,
            request: None,
            ticks: 0,
            mode,
            acknowledged,
            binding,
            cancel: cancel.clone(),
            sender,
            queue,
        };
        let (stop, _) = pump_session(
            &mut session,
            &rx,
            &mut std::iter::once("work".into()),
            None,
            ConfirmationPolicy::Studio,
            &SessionLimits {
                max_turns: 80,
                max_wall_time: if mode == 3 {
                    Duration::from_millis(50)
                } else {
                    Duration::from_secs(2)
                },
                max_tokens: None,
            },
            &Arc::new(SharedCounters::default()),
            &cancel,
            &mut observer,
        );
        if mode == 0 || mode == 4 {
            assert_eq!(stop, SessionStop::Completed);
            assert_eq!(session.responses, 1);
            assert_eq!(session.polls, if mode == 4 { 2 } else { 1 });
            assert!(
                observer.owner.state().tasks[0]
                    .progress
                    .decisions
                    .contains(&reference(321, RecordKind::OwnedData))
            );
        } else {
            assert_eq!(session.responses, 0);
            assert!(matches!(
                stop,
                SessionStop::Cancelled(_) | SessionStop::Budget(BudgetKind::WallTime)
            ));
        }
    }
}

#[test]
fn fu1_cancelled_reviewer_allows_completed_root_ready() {
    let mut owner = ContractOwner::new();
    let root = owner.root();
    let turn = owner.begin(&root, 21, TurnPurpose::Work);
    let reviewer = owner.assign(&root, Role::Reviewer).unwrap();
    drive(
        &mut owner,
        vec![Box::new(move |owner| {
            owner
                .apply(OrchestrationCommand::CancelTask(reviewer.clone()))
                .unwrap();
            owner.apply(OrchestrationCommand::EndTurn(turn)).unwrap();
            owner
                .apply(OrchestrationCommand::FreezeTask(root.clone()))
                .unwrap();
            owner
                .result(&root, 700)
                .expect("a settled cancelled reviewer must not prevent Ready");
            let mut final_cut = owner.state().clone();
            final_cut.phase = RunPhase::Finished;
            final_cut.outcome = RunOutcome::Ready;
            assert_eq!(validate_projection(&final_cut), Err(Refusal::InvalidState));
            for attempt in &mut final_cut.attempts {
                attempt.state = AttemptState::Retired(AttemptOutcome::Completed);
            }
            for effect in &mut final_cut.effects {
                effect.state = EffectState::Settled;
            }
            orchestration::tasks::refresh_cleanup(&mut final_cut);
            owner.kernel = Kernel::from_acknowledged(final_cut)
                .expect("final Ready validation must accept a settled cancelled descendant");
            assert_eq!(owner.state().outcome, RunOutcome::Ready);
            assert_eq!(owner.state().cleanup, RunCleanup::Settled);
            assert_eq!(
                owner
                    .state()
                    .tasks
                    .iter()
                    .find(|task| task.id == reviewer)
                    .unwrap()
                    .state,
                TaskState::Cancelled
            );
            let mut invalid = owner.state().clone();
            invalid.tasks[0].state = TaskState::Cancelled;
            assert_eq!(validate_projection(&invalid), Err(Refusal::InvalidState));
            let mut invalid = owner.state().clone();
            invalid.tasks[0].state = TaskState::Preparing;
            assert_eq!(validate_projection(&invalid), Err(Refusal::InvalidState));
        })],
    );
}

#[test]
fn fu1_expired_background_settles_only_after_native_cleanup() {
    let mut owner = ContractOwner::new();
    let root = owner.root();
    let turn = owner.begin(&root, 21, TurnPurpose::Work);
    drive(
        &mut owner,
        vec![Box::new(move |owner| {
            owner
                .apply(OrchestrationCommand::BackgroundWork(turn.clone(), 2))
                .unwrap();
            owner
                .apply(OrchestrationCommand::ExpireSession(turn.session.clone()))
                .unwrap();
            let before = owner.state().clone();
            assert_eq!(
                owner.apply(OrchestrationCommand::SettleExpiredBackground(turn.clone())),
                Err(Refusal::InvalidState)
            );
            assert_eq!(
                owner.apply(OrchestrationCommand::BackgroundWork(turn.clone(), 0)),
                Err(Refusal::InvalidState)
            );
            let mut next = turn.clone();
            next.ordinal += 1;
            next.session = id(800);
            assert_eq!(
                owner.apply(OrchestrationCommand::BeginTurn(next.clone())),
                Err(Refusal::InvalidState)
            );
            assert_eq!(owner.state(), &before);
            // C2 owns native proof. This fixture supplies its acknowledged retirement fact.
            let mut cleaned = owner.state().clone();
            cleaned.attempts[0].state = AttemptState::Retired(AttemptOutcome::Interrupted);
            orchestration::tasks::refresh_cleanup(&mut cleaned);
            owner.kernel = Kernel::from_acknowledged(cleaned).unwrap();
            let before = owner.state().clone();
            let mut wrong = turn.clone();
            wrong.ordinal += 1;
            assert_eq!(
                owner.apply(OrchestrationCommand::SettleExpiredBackground(wrong)),
                Err(Refusal::InvalidState)
            );
            assert_eq!(owner.state(), &before);
            owner
                .apply(OrchestrationCommand::SettleExpiredBackground(turn.clone()))
                .unwrap();
            assert_eq!(owner.state().tasks[0].progress.background, 0);
            assert_eq!(owner.state().tasks[0].progress.turn.as_ref(), Some(&turn));
            assert!(owner.state().tasks[0].progress.expired);
            assert_eq!(
                owner.apply(OrchestrationCommand::BeginTurn(next)),
                Err(Refusal::ScopeConflict)
            );
            let fresh = owner.begin(&root, 22, TurnPurpose::Work);
            assert_ne!(fresh.attempt, turn.attempt);
            assert_ne!(fresh.session, turn.session);
            assert_eq!(
                owner.apply(OrchestrationCommand::BackgroundWork(turn, 0)),
                Err(Refusal::InvalidState)
            );
            owner.apply(OrchestrationCommand::EndTurn(fresh)).unwrap();
            assert!(orchestration::tasks::ready(owner.state(), &root));
        })],
    );
}

#[test]
fn fu2_late_expiry_preserves_replacement_and_finished_phase() {
    let mut owner = ContractOwner::new();
    let root = owner.root();
    let first = owner.begin(&root, 21, TurnPurpose::Work);
    owner
        .apply(OrchestrationCommand::ExpireSession(first.session.clone()))
        .unwrap();
    let mut cleaned = owner.state().clone();
    cleaned.attempts[0].state = AttemptState::Retired(AttemptOutcome::Interrupted);
    orchestration::tasks::refresh_cleanup(&mut cleaned);
    owner.kernel = Kernel::from_acknowledged(cleaned).unwrap();
    let replacement = owner.begin(&root, 22, TurnPurpose::Work);
    let before = owner.state().clone();
    let effect = orchestration::outbox::next_effect(&before, &before.effects[0].lane).cloned();
    assert!(effect.is_some());
    assert_eq!(
        owner.apply(OrchestrationCommand::ExpireSession(first.session)),
        Err(Refusal::InvalidState)
    );
    assert_eq!(owner.state(), &before);
    assert_eq!(
        owner.state().tasks[0].progress.turn.as_ref(),
        Some(&replacement)
    );
    assert_eq!(
        orchestration::outbox::next_effect(owner.state(), &before.effects[0].lane),
        effect.as_ref()
    );
    owner
        .apply(OrchestrationCommand::ControlRun(RunIntent {
            command_id: owner
                .command_envelope(OrchestrationCommand::WaitTimeout(root.clone()))
                .id,
            run_id: owner.state().run.clone(),
            expected_control_revision: owner.state().control_revision.clone(),
            action: RunAction::Stop,
        }))
        .unwrap();
    let finished = owner.state().clone();
    assert_eq!(
        owner.apply(OrchestrationCommand::ExpireSession(replacement.session)),
        Err(Refusal::InvalidState)
    );
    assert_eq!(owner.state(), &finished);
    assert_eq!(owner.state().phase, RunPhase::Finished);
}

#[test]
fn fu2_terminal_runs_refuse_owner_lifecycle_commands() {
    let mut owner = ContractOwner::new();
    let root = owner.root();
    let turn = owner.begin(&root, 21, TurnPurpose::Work);
    owner
        .apply(OrchestrationCommand::ExpireSession(turn.session.clone()))
        .unwrap();
    let mut terminal = owner.state().clone();
    terminal.attempts[0].state = AttemptState::Retired(AttemptOutcome::Interrupted);
    orchestration::tasks::refresh_cleanup(&mut terminal);
    terminal.phase = RunPhase::Finished;
    for outcome in [
        RunOutcome::Failed(OutcomeDetail {
            code: "fixture_failure".into(),
            detail_ref: None,
        }),
        RunOutcome::Cancelled(OutcomeDetail {
            code: "fixture_cancel".into(),
            detail_ref: None,
        }),
        RunOutcome::Ready,
    ] {
        terminal.outcome = outcome;
        terminal.tasks[0].state = TaskState::Terminal(reference(920, RecordKind::RunIndex));
        for effect in &mut terminal.effects {
            effect.state = EffectState::Disposed;
        }
        validate_projection(&terminal).unwrap();
        owner.kernel = Kernel::from_acknowledged(terminal.clone()).unwrap();
        for command in [
            OrchestrationCommand::SettleExpiredBackground(turn.clone()),
            OrchestrationCommand::RecordContext {
                task: root.clone(),
                feedback: vec![reference(921, RecordKind::OwnedData)],
                decisions: vec![],
            },
            OrchestrationCommand::WaitTimeout(root.clone()),
            OrchestrationCommand::CancelTask(root.clone()),
        ] {
            assert_eq!(owner.apply(command), Err(Refusal::InvalidState));
            assert_eq!(owner.state(), &terminal);
        }
    }
}

#[test]
fn fu2_wait_tracks_questions_children_and_deliveries() {
    let mut owner = ContractOwner::new();
    let root = owner.root();
    let director = owner.begin(&root, 21, TurnPurpose::Work);
    let child = owner.assign(&root, Role::Reviewer).unwrap();
    let turn = owner.begin(&child, 22, TurnPurpose::Work);
    let question = owner.question(&child, 930);
    owner
        .apply(OrchestrationCommand::Request(RuntimeRequest::Question(
            question.clone(),
        )))
        .unwrap();
    assert_eq!(owner.state().wait, RunWait::Question(question.id.clone()));
    owner
        .apply(OrchestrationCommand::AnswerQuestion(AnswerQuestion {
            question: question.id,
            answer: reference(931, RecordKind::OwnedData),
        }))
        .unwrap();
    assert_eq!(owner.state().wait, RunWait::None);
    owner
        .apply(OrchestrationCommand::EndTurn(director.clone()))
        .unwrap();
    assert_eq!(owner.state().wait, RunWait::Children(vec![child.clone()]));
    owner.apply(OrchestrationCommand::EndTurn(turn)).unwrap();
    owner
        .apply(OrchestrationCommand::FreezeTask(child.clone()))
        .unwrap();
    let followup = owner.begin(&root, 23, TurnPurpose::Work);
    assert_eq!(owner.state().wait, RunWait::None);
    owner.result(&child, 940).unwrap();
    assert_eq!(owner.state().wait, RunWait::Children(vec![child]));
    owner
        .apply(OrchestrationCommand::OfferDeliveries(
            root,
            followup.attempt,
        ))
        .unwrap();
    owner
        .apply(OrchestrationCommand::ObserveDelivery(
            owner.state().deliveries[0].id.clone(),
        ))
        .unwrap();
    assert_eq!(owner.state().wait, RunWait::None);
    assert!(matches!(owner.state().cleanup, RunCleanup::Pending(_)));
    validate_projection(owner.state()).unwrap();
}
