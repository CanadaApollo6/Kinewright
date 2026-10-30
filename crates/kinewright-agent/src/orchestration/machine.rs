//! Pure Studio command decisions, with no provider waits or project writes.
//!
//! Rust adaptation of <https://github.com/pingdotgg/t3code> at
//! 5d91ab1117ad51ef1288a94717d85c70e7af09fe, apps/server/src/orchestration-v2/
//! {Orchestrator,EventSink,CommandReceiptStore}.ts. Notice: LICENSES/t3code-MIT.txt.
//! Kinewright binds identities to scoped envelopes and requires the existing
//! host to authenticate S3 access before receipt lookup or fresh admission.
#![allow(clippy::missing_errors_doc)]

use super::{
    orchestration_commit::{
        CommandDecision, RecordedRefusal, ValidatedRunUpdate, validate_projection,
    },
    orchestration_records::{
        EffectRecord, EffectSafety, EffectState, EventSegment, OpaqueRevision,
        OrchestrationCommand, OrchestrationEnvelope, OrchestrationEvent, OutcomeDetail, Refusal,
        Role, RunAction, RunCleanup, RunOutcome, RunPhase, RunProjection, RunWait, TaskRecord,
        TaskState,
    },
};

/// One serialized pending publication. Uncertainty retains the original envelope
/// and prevents a fresh decision until the owner reconciles that exact cut.
#[derive(Debug, Default)]
pub struct Kernel {
    acknowledged: Option<RunProjection>,
    pending: Option<ValidatedRunUpdate>,
    uncertain: bool,
}

impl Kernel {
    #[must_use]
    pub fn acknowledged(&self) -> Option<&RunProjection> {
        self.acknowledged.as_ref()
    }

    #[must_use]
    pub fn original_envelope(&self) -> Option<&OrchestrationEnvelope> {
        self.pending.as_ref().map(ValidatedRunUpdate::envelope)
    }

    pub fn propose(
        &mut self,
        project: &super::bundle_records::ProjectId,
        generation: u64,
        envelope: &OrchestrationEnvelope,
        previous_segment: Option<super::bundle_records::ObjectRef>,
    ) -> Result<&ValidatedRunUpdate, Refusal> {
        self.require_available()?;
        let next = prepare(
            self.acknowledged.as_ref(),
            project,
            generation,
            envelope,
            previous_segment,
        )?;
        self.pending = Some(next);
        self.pending.as_ref().ok_or(Refusal::InvalidState)
    }

    /// H1 has staged the typed result whose reference is included in this same cut.
    pub fn propose_result(
        &mut self,
        project: &super::bundle_records::ProjectId,
        generation: u64,
        envelope: &OrchestrationEnvelope,
        previous_segment: Option<super::bundle_records::ObjectRef>,
        result: super::bundle_records::ObjectRef,
    ) -> Result<&ValidatedRunUpdate, Refusal> {
        self.require_available()?;
        let state = self.acknowledged.as_ref().ok_or(Refusal::InvalidState)?;
        self.pending = Some(super::mailbox::prepare_result_update(
            state,
            project,
            generation,
            envelope,
            previous_segment,
            result,
        )?);
        self.pending.as_ref().ok_or(Refusal::InvalidState)
    }
    fn require_available(&self) -> Result<(), Refusal> {
        if self.uncertain || self.pending.is_some() {
            return Err(Refusal::PublicationUnknown);
        }
        Ok(())
    }

    /// H1 calls after loading the acknowledged cut, never a tentative snapshot.
    #[allow(dead_code, reason = "H1 supplies trusted recovery admission")]
    pub(crate) fn from_acknowledged(state: RunProjection) -> Result<Self, Refusal> {
        validate_projection(&state)?;
        Ok(Self {
            acknowledged: Some(state),
            pending: None,
            uncertain: false,
        })
    }

    pub fn publication_unknown(&mut self) -> Result<(), Refusal> {
        if self.pending.is_none() {
            return Err(Refusal::InvalidState);
        }
        self.uncertain = true;
        Ok(())
    }

    /// Called only by the trusted owner after B5 or exact-envelope reconciliation.
    /// No prepared cut or externally deserialized receipt grants effect dispatch.
    #[allow(
        dead_code,
        reason = "H1 supplies the production post-acknowledgment caller"
    )]
    pub(crate) fn acknowledge(
        &mut self,
        returned: &super::orchestration_commit::OrchestrationReceipt,
    ) -> Result<(), Refusal> {
        let pending = self.pending.as_ref().ok_or(Refusal::Unknown)?;
        pending.validate_returned_receipt(returned)?;
        self.acknowledged = Some(pending.projection().clone());
        self.pending = None;
        self.uncertain = false;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transition {
    Start,
    Control(RunAction),
    Task(super::tasks::TaskTransition),
    Lifecycle,
    Refused(RecordedRefusal),
}

pub fn decide(
    state: Option<&RunProjection>,
    command: &OrchestrationCommand,
) -> Result<Transition, Refusal> {
    match (state, command) {
        (None, OrchestrationCommand::StartRun(start)) if start.director.role == Role::Director => {
            Ok(Transition::Start)
        }
        (Some(state), OrchestrationCommand::ControlRun(intent)) => {
            if state.run != intent.run_id {
                return Err(Refusal::ScopeConflict);
            }
            if state.control_revision != intent.expected_control_revision {
                return Ok(Transition::Refused(RecordedRefusal::StaleControl));
            }
            if !eligible(state, &intent.action) {
                return Ok(Transition::Refused(RecordedRefusal::InvalidState));
            }
            Ok(Transition::Control(intent.action.clone()))
        }
        (
            Some(state),
            OrchestrationCommand::RecordContext { .. }
            | OrchestrationCommand::SettleExpiredBackground(_)
            | OrchestrationCommand::WaitTimeout(_)
            | OrchestrationCommand::CancelTask(_),
        ) if state.outcome != RunOutcome::Pending => Err(Refusal::InvalidState),
        (
            Some(_),
            OrchestrationCommand::RecordContext { .. }
            | OrchestrationCommand::BeginTurn(_)
            | OrchestrationCommand::EndTurn(_)
            | OrchestrationCommand::BackgroundWork(_, _)
            | OrchestrationCommand::SettleExpiredBackground(_)
            | OrchestrationCommand::Request(_)
            | OrchestrationCommand::AdmitQuestionWake(_)
            | OrchestrationCommand::ExecuteQuestionWake(_)
            | OrchestrationCommand::AnswerQuestion(_)
            | OrchestrationCommand::ExpireSession(_)
            | OrchestrationCommand::OfferDeliveries(_, _)
            | OrchestrationCommand::ObserveDelivery(_)
            | OrchestrationCommand::WaitTimeout(_)
            | OrchestrationCommand::CancelTask(_),
        ) => Ok(Transition::Lifecycle),
        (Some(state), command) => Ok(super::tasks::decide_task(state, command)?.map_or(
            Transition::Refused(RecordedRefusal::Unsupported),
            Transition::Task,
        )),
        _ => Ok(Transition::Refused(RecordedRefusal::Unsupported)),
    }
}

#[must_use]
pub fn eligible(state: &RunProjection, action: &RunAction) -> bool {
    if state.outcome != RunOutcome::Pending {
        return false;
    }
    match action {
        RunAction::Pause => matches!(
            state.phase,
            RunPhase::Queued | RunPhase::Running | RunPhase::Reviewing
        ),
        RunAction::Stop => state.phase != RunPhase::Finished,
        RunAction::Continue => {
            matches!(state.phase, RunPhase::Paused | RunPhase::Interrupted)
                && state.cleanup == RunCleanup::Settled
                && state.wait == RunWait::None
        }
        RunAction::SendMessage { .. } => {
            super::requests::director(state).is_ok()
                && state.effects.len() < super::orchestration_records::ITEM_LIMIT
        }
    }
}

/// Prepare one complete cut. Replay and fresh endpoint authorization belong to
/// H1/H3 in the owner, before this function, including on unknown-outcome retry.
#[allow(clippy::too_many_lines, reason = "single envelope-to-cut transition")]
pub fn prepare(
    state: Option<&RunProjection>,
    project: &super::bundle_records::ProjectId,
    generation: u64,
    envelope: &OrchestrationEnvelope,
    previous_segment: Option<super::bundle_records::ObjectRef>,
) -> Result<ValidatedRunUpdate, Refusal> {
    prepare_with_attention(state, project, generation, envelope, previous_segment, None)
}
#[allow(clippy::too_many_lines, reason = "single envelope-to-cut transition")]
pub(crate) fn prepare_with_attention(
    state: Option<&RunProjection>,
    project: &super::bundle_records::ProjectId,
    generation: u64,
    envelope: &OrchestrationEnvelope,
    previous_segment: Option<super::bundle_records::ObjectRef>,
    attention: Option<&super::requests::LiveAttentionAdmission>,
) -> Result<ValidatedRunUpdate, Refusal> {
    envelope.validate()?;
    if let Some(state) = state {
        validate_projection(state)?;
    }
    let transition = decide(state, &envelope.command)?;
    let revision = envelope.identity::<OpaqueRevision>("control-revision")?;
    let mut next = if let Some(state) = state {
        state.clone()
    } else {
        let OrchestrationCommand::StartRun(start) = &envelope.command else {
            return Err(Refusal::Unsupported);
        };
        if transition != Transition::Start {
            return Err(Refusal::Unsupported);
        }
        let task = TaskRecord {
            id: envelope.identity("task")?,
            parent: None,
            brief: start.director.clone(),
            state: TaskState::Queued,
            progress: super::orchestration_records::TaskProgress {
                capacity: super::orchestration_records::DependencyReservation::Reserved,
                ..super::orchestration_records::TaskProgress::default()
            },
        };
        RunProjection {
            run: envelope.identity("run")?,
            sequence: 0,
            control_revision: revision.clone(),
            phase: RunPhase::Queued,
            outcome: RunOutcome::Pending,
            wait: RunWait::None,
            cleanup: RunCleanup::Settled,
            tasks: vec![task],
            effects: Vec::new(),
            attempts: Vec::new(),
            deliveries: Vec::new(),
            requests: Vec::new(),
            results: Vec::new(),
            usage: Vec::new(),
        }
    };
    let decision = match transition {
        Transition::Start => {
            let task = &next.tasks[0];
            next.effects.push(EffectRecord {
                id: envelope.identity("effect.prepare")?,
                task: task.id.clone(),
                lane: task.brief.scope.base.branch.clone(),
                order: 1,
                safety: EffectSafety::ReplaySafe,
                state: EffectState::Pending,
                payload: task.brief.assignment.clone(),
            });
            CommandDecision::Applied
        }
        Transition::Control(action) => {
            match action {
                RunAction::Pause => next.phase = RunPhase::Paused,
                RunAction::Stop => {
                    next.phase = RunPhase::Finished;
                    next.outcome = RunOutcome::Cancelled(OutcomeDetail {
                        code: "stopped_by_person".into(),
                        detail_ref: None,
                    });
                    let roots = next
                        .tasks
                        .iter()
                        .filter(|task| task.parent.is_none())
                        .map(|task| task.id.clone())
                        .collect::<Vec<_>>();
                    for id in roots {
                        super::tasks::cancel(&mut next, &id)?;
                    }
                    for effect in &mut next.effects {
                        if effect.state == EffectState::Pending {
                            effect.state = EffectState::Disposed;
                        }
                    }
                    // Actual attempt retirement is never inferred from Stop.
                }
                RunAction::Continue => next.phase = RunPhase::Queued,
                RunAction::SendMessage { text_ref } => {
                    let task = super::requests::director(&next)?;
                    let order = next
                        .effects
                        .iter()
                        .filter(|effect| effect.lane == task.brief.scope.base.branch)
                        .map(|effect| effect.order)
                        .max()
                        .unwrap_or(0)
                        .checked_add(1)
                        .ok_or(Refusal::Capacity)?;
                    next.effects.push(EffectRecord {
                        id: envelope.identity("effect.control-message")?,
                        task: task.id.clone(),
                        lane: task.brief.scope.base.branch.clone(),
                        order,
                        safety: EffectSafety::ProcessBound,
                        state: EffectState::Pending,
                        payload: text_ref,
                    });
                }
            }
            next.control_revision = revision;
            CommandDecision::Applied
        }
        Transition::Lifecycle => {
            if !super::tasks::apply_lifecycle(&mut next, envelope)?
                && !super::requests::apply_authorized(&mut next, envelope, attention)?
                && !super::mailbox::apply(&mut next, envelope)?
            {
                return Err(Refusal::Unsupported);
            }
            next.control_revision = revision;
            CommandDecision::Applied
        }
        Transition::Task(task) => {
            super::tasks::apply_task_transition(&mut next, envelope, task)?;
            next.control_revision = revision;
            CommandDecision::Applied
        }
        Transition::Refused(reason) => CommandDecision::Refused(reason),
    };
    if decision == CommandDecision::Applied {
        super::tasks::refresh_cleanup(&mut next);
        super::tasks::refresh_wait(&mut next);
    }
    next.sequence = next.sequence.checked_add(1).ok_or(Refusal::Capacity)?;
    let event = OrchestrationEvent {
        command: envelope.id.clone(),
        sequence: next.sequence,
        projection: next.clone(),
    };
    let segment = EventSegment {
        run: next.run.clone(),
        first_sequence: next.sequence,
        previous: previous_segment,
        events: vec![event],
    };
    ValidatedRunUpdate::new(project, generation, envelope, next, segment, decision)
}

pub fn fold(state: &mut RunProjection, event: &OrchestrationEvent) -> Result<(), Refusal> {
    validate_projection(&event.projection)?;
    if state.run != event.projection.run
        || state.sequence.checked_add(1) != Some(event.sequence)
        || event.sequence != event.projection.sequence
    {
        return Err(Refusal::InvalidState);
    }
    *state = event.projection.clone();
    Ok(())
}
