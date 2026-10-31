//! Frozen task results and parent delivery share one proposed orchestration cut.
//!
//! Rust adaptation of <https://github.com/pingdotgg/t3code> at
//! 5d91ab1117ad51ef1288a94717d85c70e7af09fe, apps/server/src/orchestration-v2/
//! NotificationMailbox.ts and Orchestrator.ts.
//! Notice: LICENSES/t3code-MIT.txt. Kinewright retains explicit result and
//! observation identities. H1 alone can publish or acknowledge the proposal.
#![allow(clippy::missing_errors_doc)]
use super::{
    bundle_records::ObjectRef,
    orchestration_records::{
        CONTROL_BYTES, DeliveryRecord, DeliveryState, EffectRecord, EffectSafety, EffectState,
        ITEM_LIMIT, OrchestrationCommand, OrchestrationEnvelope, OrchestrationPayload,
        OrchestrationRecord, Refusal, RunProjection, TaskState, encoded,
    },
};

/// H1 stages this typed record with the returned projection, in the same cut.
/// A provider's final text cannot call this port or make a task ready.
/// A coalesced wake delivers every `DeliveryRecord` whose `wake` equals the effect
/// ID. Its `payload` names only the first result, not the full delivery set.
#[allow(
    clippy::too_many_lines,
    reason = "one atomic result and generation proposal"
)]
pub fn result_and_delivery(
    state: &RunProjection,
    envelope: &OrchestrationEnvelope,
    result: ObjectRef,
) -> Result<RunProjection, Refusal> {
    envelope.validate()?;
    let OrchestrationCommand::PublishResult(publish) = &envelope.command else {
        return Err(Refusal::Unsupported);
    };
    if state.results.len() >= ITEM_LIMIT
        || state.deliveries.len() >= ITEM_LIMIT
        || state.effects.len() >= ITEM_LIMIT
    {
        return Err(Refusal::Capacity);
    }
    let task = state
        .tasks
        .iter()
        .find(|task| task.id == publish.task)
        .ok_or(Refusal::ScopeConflict)?;
    require_publishable(state, task)?;
    if publish.result.base.as_ref() != Some(&task.brief.scope.base) || publish.result.head.is_none()
    {
        return Err(Refusal::InvalidState);
    }
    let record = OrchestrationRecord {
        version: 1,
        payload: OrchestrationPayload::Result(Box::new(publish.result.clone())),
    };
    let bytes = encoded(&record, CONTROL_BYTES)?;
    if result.sha256 != kinewright_media::sha256_bytes(&bytes)
        || result.byte_length != bytes.len() as u64
    {
        return Err(Refusal::ScopeConflict);
    }
    let siblings = state
        .deliveries
        .iter()
        .filter(|delivery| {
            state
                .tasks
                .iter()
                .any(|other| other.id == delivery.task && other.parent == task.parent)
        })
        .collect::<Vec<_>>();
    let coalesced = siblings.iter().find(|delivery| {
        delivery.state == DeliveryState::Pending
            && state
                .effects
                .iter()
                .any(|effect| effect.id == delivery.wake && effect.state == EffectState::Pending)
    });
    let generation = coalesced.map_or_else(
        || {
            siblings
                .iter()
                .map(|delivery| delivery.generation)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or(Refusal::Capacity)
        },
        |delivery| Ok(delivery.generation),
    )?;
    let delivery = if task.parent.is_some() {
        Some(DeliveryRecord {
            id: envelope.identity("delivery")?,
            task: publish.task.clone(),
            result: result.clone(),
            generation,
            wake: coalesced.map_or_else(
                || envelope.identity("effect.deliver-result"),
                |delivery| Ok(delivery.wake.clone()),
            )?,
            message: coalesced.map_or_else(
                || envelope.identity("result-message"),
                |delivery| Ok(delivery.message.clone()),
            )?,
            state: DeliveryState::Pending,
        })
    } else {
        None
    };
    let wake = if let Some(parent) = task.parent.as_ref().filter(|_| coalesced.is_none()) {
        let parent = state
            .tasks
            .iter()
            .find(|candidate| &candidate.id == parent)
            .ok_or(Refusal::InvalidState)?;
        let lane = parent.brief.scope.base.branch.clone();
        let order = state
            .effects
            .iter()
            .filter(|effect| effect.lane == lane)
            .map(|effect| effect.order)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(Refusal::Capacity)?;
        Some(EffectRecord {
            id: envelope.identity("effect.deliver-result")?,
            task: parent.id.clone(),
            lane,
            order,
            safety: EffectSafety::ProcessBound,
            state: EffectState::Pending,
            payload: result.clone(),
        })
    } else {
        None
    };
    let mut next = state.clone();
    let task = next
        .tasks
        .iter_mut()
        .find(|task| task.id == publish.task)
        .ok_or(Refusal::ScopeConflict)?;
    task.state = TaskState::Terminal(result.clone());
    next.results.push(result);
    if let Some(delivery) = delivery {
        next.deliveries.push(delivery);
    }
    if let Some(wake) = wake {
        next.effects.push(wake);
    }
    super::tasks::refresh_wait(&mut next);
    Ok(next)
}

/// Complete a proposed root cut after H1 stages the exact typed result record.
/// Staging alone does not acknowledge this update or expose its wake effect.
pub fn prepare_result_update(
    state: &RunProjection,
    project: &super::bundle_records::ProjectId,
    generation: u64,
    envelope: &OrchestrationEnvelope,
    previous_segment: Option<ObjectRef>,
    result: ObjectRef,
) -> Result<super::orchestration_commit::ValidatedRunUpdate, Refusal> {
    use super::{
        orchestration_commit::{CommandDecision, ValidatedRunUpdate, validate_projection},
        orchestration_records::{EventSegment, OrchestrationEvent},
    };
    validate_projection(state)?;
    let mut next = result_and_delivery(state, envelope, result)?;
    next.sequence = next.sequence.checked_add(1).ok_or(Refusal::Capacity)?;
    next.control_revision = envelope.identity("control-revision")?;
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
    ValidatedRunUpdate::new(
        project,
        generation,
        envelope,
        next,
        segment,
        CommandDecision::Applied,
    )
}

pub fn apply(state: &mut RunProjection, envelope: &OrchestrationEnvelope) -> Result<bool, Refusal> {
    use super::orchestration_records::{AttemptState, RunOutcome, RunPhase};
    match &envelope.command {
        OrchestrationCommand::OfferDeliveries(parent, attempt) => {
            if state.outcome != RunOutcome::Pending
                || state.phase == RunPhase::Paused
                || !state.attempts.iter().any(|reservation| {
                    reservation.identity.task == *parent
                        && reservation.identity.attempt == *attempt
                        && !matches!(
                            reservation.state,
                            AttemptState::Stopping
                                | AttemptState::Retired(_)
                                | AttemptState::CleanupBlocked(_)
                        )
                })
                || !state.tasks.iter().any(|task| {
                    task.id == *parent
                        && !matches!(
                            task.state,
                            TaskState::Terminal(_) | TaskState::Cancelled | TaskState::Cancelling
                        )
                })
            {
                return Err(Refusal::InvalidState);
            }
            for delivery in &mut state.deliveries {
                if state
                    .tasks
                    .iter()
                    .any(|task| task.id == delivery.task && task.parent.as_ref() == Some(parent))
                    && matches!(
                        delivery.state,
                        DeliveryState::Pending | DeliveryState::Offered(_, _)
                    )
                {
                    delivery.state =
                        DeliveryState::Offered(attempt.clone(), delivery.message.clone());
                }
            }
        }
        OrchestrationCommand::ObserveDelivery(id) => {
            let index = state
                .deliveries
                .iter()
                .position(|delivery| &delivery.id == id)
                .ok_or(Refusal::ScopeConflict)?;
            let delivery = state.deliveries[index].clone();
            let parent = state
                .tasks
                .iter()
                .find(|task| task.id == delivery.task)
                .and_then(|task| task.parent.as_ref())
                .ok_or(Refusal::ScopeConflict)?;
            let turn = state
                .tasks
                .iter()
                .find(|task| &task.id == parent)
                .and_then(|task| task.progress.turn.as_ref())
                .ok_or(Refusal::InvalidState)?;
            super::requests::live_turn(state, turn)?;
            if envelope.authority.attempt.as_ref() != Some(&turn.attempt)
                || !matches!(&delivery.state, DeliveryState::Offered(attempt, message) if attempt == &turn.attempt && message == &delivery.message)
            {
                return Err(Refusal::ScopeConflict);
            }
            state.deliveries[index].state = DeliveryState::Observed(state.run.clone());
            settle_generations(state);
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn require_publishable(
    state: &RunProjection,
    task: &super::orchestration_records::TaskRecord,
) -> Result<(), Refusal> {
    if !super::tasks::ready(state, &task.id)
        || state.outcome != super::orchestration_records::RunOutcome::Pending
        || task.state != TaskState::Freezing
    {
        return Err(Refusal::InvalidState);
    }
    Ok(())
}

pub fn settle_generations(state: &mut RunProjection) {
    for effect in &mut state.effects {
        let deliveries = state
            .deliveries
            .iter()
            .filter(|delivery| delivery.wake == effect.id)
            .collect::<Vec<_>>();
        if !deliveries.is_empty()
            && deliveries.iter().all(|delivery| {
                matches!(
                    delivery.state,
                    DeliveryState::Observed(_) | DeliveryState::Disposed
                )
            })
        {
            effect.state = if deliveries
                .iter()
                .any(|delivery| matches!(delivery.state, DeliveryState::Observed(_)))
            {
                EffectState::Settled
            } else {
                EffectState::Disposed
            };
        }
    }
}

/// The host offers only results whose exact wake payload was charged for this attempt.
pub(crate) fn offer_attempt_deliveries(
    state: &mut RunProjection,
    parent: &super::orchestration_records::TaskId,
    attempt: &super::orchestration_records::AttemptId,
    wake: &super::orchestration_records::EffectId,
) -> Result<(), Refusal> {
    let turn = state
        .tasks
        .iter()
        .find(|task| &task.id == parent)
        .and_then(|task| task.progress.turn.as_ref())
        .ok_or(Refusal::InvalidState)?;
    super::requests::live_turn(state, turn)?;
    if &turn.attempt != attempt {
        return Err(Refusal::ScopeConflict);
    }
    for delivery in &mut state.deliveries {
        if &delivery.wake == wake
            && state
                .tasks
                .iter()
                .any(|task| task.id == delivery.task && task.parent.as_ref() == Some(parent))
            && matches!(delivery.state, DeliveryState::Pending)
        {
            delivery.state = DeliveryState::Offered(attempt.clone(), delivery.message.clone());
        }
    }
    Ok(())
}
