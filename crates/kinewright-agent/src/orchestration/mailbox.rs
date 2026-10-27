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
    if task.state != TaskState::Freezing
        || publish.result.base.as_ref() != Some(&task.brief.scope.base)
        || publish.result.head.is_none()
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
    let delivery = if task.parent.is_some() {
        Some(DeliveryRecord {
            id: envelope.identity("delivery")?,
            task: publish.task.clone(),
            result: result.clone(),
            generation: state.sequence.checked_add(1).ok_or(Refusal::Capacity)?,
            message: envelope.identity("result-message")?,
            state: DeliveryState::Pending,
        })
    } else {
        None
    };
    let wake = if let Some(parent) = &task.parent {
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
            safety: EffectSafety::ReplaySafe,
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
