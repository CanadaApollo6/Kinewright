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
        if self.uncertain || self.pending.is_some() {
            return Err(Refusal::PublicationUnknown);
        }
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
        // C3/C4 add task, merge and live-request authority. C1 cannot execute them.
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
        // Live/bounded Director message admission arrives with C4, not a label.
        RunAction::SendMessage { .. } => false,
    }
}

/// Prepare one complete cut. Replay and fresh endpoint authorization belong to
/// H1/H3 in the owner, before this function, including on unknown-outcome retry.
pub fn prepare(
    state: Option<&RunProjection>,
    project: &super::bundle_records::ProjectId,
    generation: u64,
    envelope: &OrchestrationEnvelope,
    previous_segment: Option<super::bundle_records::ObjectRef>,
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
                    for effect in &mut next.effects {
                        if effect.state == EffectState::Pending {
                            effect.state = EffectState::Disposed;
                        }
                    }
                    // Actual attempt retirement is never inferred from Stop.
                }
                RunAction::Continue => next.phase = RunPhase::Queued,
                RunAction::SendMessage { .. } => return Err(Refusal::Unsupported),
            }
            next.control_revision = revision;
            CommandDecision::Applied
        }
        Transition::Refused(reason) => CommandDecision::Refused(reason),
    };
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
