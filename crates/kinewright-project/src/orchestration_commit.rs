//! Internal C1 publication validation for the S3 H1 publisher.
//!
//! Rust adaptation of <https://github.com/pingdotgg/t3code> at
//! 5d91ab1117ad51ef1288a94717d85c70e7af09fe, apps/server/src/orchestration-v2/
//! {EventSink,CommandReceiptStore,Orchestrator}.ts. Notice: LICENSES/t3code-MIT.txt.
//! Kinewright validates scope after reservation and at returned-result admission.
//! Publication uses the existing metadata owner and sealed store registry.
#![allow(clippy::missing_errors_doc)]

use super::{
    bundle_records::{ObjectRef, ProjectId, RecordKind},
    orchestration_records::{
        ATTEMPT_LIMIT, AttemptState, COMMAND_BYTES, CONTROL_BYTES, DeliveryState, EffectState,
        EventSegment, ITEM_LIMIT, OrchestrationEnvelope, OrchestrationPayload, OrchestrationRecord,
        Refusal, RunCleanup, RunOutcome, RunPhase, RunProjection, RunWait, RuntimeRequest,
        TASK_LIMIT, TaskBrief, TaskState, encoded, validate_reference,
    },
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[path = "orchestration_store.rs"]
mod store;
pub use store::{PublishedRunCut, RegisteredRunIndex, commit_orchestration};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordedRefusal {
    Unsupported,
    Capacity,
    StaleControl,
    InvalidState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "reason",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CommandDecision {
    Applied,
    Refused(RecordedRefusal),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestrationReceipt {
    pub envelope: OrchestrationEnvelope,
    pub generation: u64,
    pub sequence: u64,
    pub cut_digest: String,
    pub decision: CommandDecision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiptStatus {
    Absent,
    Pending,
    Committed(OrchestrationReceipt),
    Retired(OrchestrationReceipt),
    Unknown,
    RetiredUnresolved,
    UnresolvedOrigin,
}

/// The current host verifies project read access and S3 endpoint role first.
/// This function deliberately does not accept a caller-provided role string.
pub fn checked_receipt(
    envelope: &OrchestrationEnvelope,
    status: ReceiptStatus,
) -> Result<ReceiptStatus, Refusal> {
    envelope.validate()?;
    if let ReceiptStatus::Committed(receipt) | ReceiptStatus::Retired(receipt) = &status {
        // Repeat after reservation loss and again on the final returned result.
        if receipt.envelope != *envelope {
            return Err(Refusal::ScopeConflict);
        }
        if receipt.generation == 0 || receipt.cut_digest.len() != 64 {
            return Err(Refusal::InvalidState);
        }
    }
    Ok(status)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ValidatedRunUpdate {
    envelope: OrchestrationEnvelope,
    projection: RunProjection,
    segment: EventSegment,
    receipt: OrchestrationReceipt,
    dependencies: Vec<ObjectRef>,
}

impl ValidatedRunUpdate {
    pub fn new(
        project: &ProjectId,
        generation: u64,
        envelope: &OrchestrationEnvelope,
        projection: RunProjection,
        segment: EventSegment,
        decision: CommandDecision,
    ) -> Result<Self, Refusal> {
        envelope.validate()?;
        if envelope.project != *project {
            return Err(Refusal::ScopeConflict);
        }
        if envelope.expected_generation != generation {
            return Err(Refusal::StaleGeneration);
        }
        validate_projection(&projection)?;
        let snapshot = OrchestrationRecord {
            version: 1,
            payload: OrchestrationPayload::Snapshot(Box::new(projection.clone())),
        };
        let mut dependencies = snapshot.dependencies()?;
        let event_record = OrchestrationRecord {
            version: 1,
            payload: OrchestrationPayload::EventSegment(segment.clone()),
        };
        dependencies.extend(event_record.dependencies()?);
        dependencies.push(envelope.authority.grant_ref.clone());
        if segment.run != projection.run
            || segment
                .events
                .last()
                .is_none_or(|event| event.projection != projection || event.command != envelope.id)
        {
            return Err(Refusal::InvalidState);
        }
        dependencies.sort();
        dependencies.dedup();
        for reference in &dependencies {
            validate_reference(reference)?;
        }
        let cut_digest = kinewright_media::sha256_bytes(&encoded(
            &(envelope, &projection, &segment, &decision, &dependencies),
            CONTROL_BYTES,
        )?);
        let receipt = OrchestrationReceipt {
            envelope: envelope.clone(),
            generation: generation.checked_add(1).ok_or(Refusal::Capacity)?,
            sequence: projection.sequence,
            cut_digest,
            decision,
        };
        Ok(Self {
            envelope: envelope.clone(),
            projection,
            segment,
            receipt,
            dependencies,
        })
    }
    #[must_use]
    pub fn envelope(&self) -> &OrchestrationEnvelope {
        &self.envelope
    }
    #[must_use]
    pub fn projection(&self) -> &RunProjection {
        &self.projection
    }
    #[must_use]
    pub fn segment(&self) -> &EventSegment {
        &self.segment
    }
    #[must_use]
    pub fn receipt(&self) -> &OrchestrationReceipt {
        &self.receipt
    }
    #[must_use]
    pub fn dependencies(&self) -> &[ObjectRef] {
        &self.dependencies
    }

    /// Pure validation only. H1 must call this after the B5 durable acknowledgment,
    /// never on a prepared receipt or on an uncertain root publication.
    pub fn validate_returned_receipt(&self, receipt: &OrchestrationReceipt) -> Result<(), Refusal> {
        checked_receipt(&self.envelope, ReceiptStatus::Committed(receipt.clone()))?;
        if *receipt != self.receipt {
            return Err(Refusal::ScopeConflict);
        }
        Ok(())
    }
}

impl OrchestrationRecord {
    /// Exact dependency edges for H1's sealed registry, including cold/pending state.
    pub fn dependencies(&self) -> Result<Vec<ObjectRef>, Refusal> {
        if self.version != 1 {
            return Err(Refusal::Unsupported);
        }
        encoded(self, CONTROL_BYTES)?;
        let mut refs = Vec::new();
        match &self.payload {
            // S4 C5
            OrchestrationPayload::Accounting(record) => {
                refs.extend(record.dependencies().map_err(|_| Refusal::InvalidState)?);
            }
            OrchestrationPayload::AccountingArchive(record) => {
                refs.extend(record.dependencies().map_err(|_| Refusal::InvalidState)?);
            }
            OrchestrationPayload::RunIndex(index) => {
                if index.len() > ITEM_LIMIT {
                    return Err(Refusal::Capacity);
                }
                if index.iter().any(|r| r.record_kind != RecordKind::RunIndex) {
                    return Err(Refusal::InvalidState);
                }
                refs.extend(index.clone());
            }
            OrchestrationPayload::Snapshot(state) => projection_refs(state, &mut refs)?,
            OrchestrationPayload::EventSegment(segment) => {
                if segment.first_sequence == 0
                    || (segment.first_sequence > 1 && segment.previous.is_none())
                {
                    return Err(Refusal::InvalidState);
                }
                if segment.events.is_empty() || segment.events.len() > TASK_LIMIT {
                    return Err(Refusal::Capacity);
                }
                if let Some(previous) = &segment.previous {
                    if previous.record_kind != RecordKind::RunIndex {
                        return Err(Refusal::InvalidState);
                    }
                    refs.push(previous.clone());
                }
                for (offset, event) in segment.events.iter().enumerate() {
                    if segment.first_sequence.checked_add(offset as u64) != Some(event.sequence)
                        || event.sequence != event.projection.sequence
                        || event.projection.run != segment.run
                    {
                        return Err(Refusal::InvalidState);
                    }
                    projection_refs(&event.projection, &mut refs)?;
                }
            }
            OrchestrationPayload::Effect(effect) => refs.push(effect.payload.clone()),
            OrchestrationPayload::Request(request) => request_refs(request, &mut refs),
            OrchestrationPayload::Delivery(delivery) => refs.push(delivery.result.clone()),
            OrchestrationPayload::Result(result) => {
                if result.artifacts.len() > ITEM_LIMIT || result.evidence.len() > ITEM_LIMIT {
                    return Err(Refusal::Capacity);
                }
                if let Some(base) = &result.base {
                    refs.push(base.checkpoint.clone());
                }
                if let Some(head) = &result.head {
                    refs.push(head.checkpoint.clone());
                }
                refs.extend(result.delta_ref.clone());
                refs.extend(result.artifacts.clone());
                refs.extend(result.evidence.clone());
                refs.push(result.usage_ref.clone());
                refs.push(result.explanation.clone());
            }
            OrchestrationPayload::Usage(usage) => {
                if usage.version != 1 || usage.identity.event.len() > 256 {
                    return Err(Refusal::Unsupported);
                }
            }
        }
        for reference in &refs {
            validate_reference(reference)?;
        }
        refs.sort();
        refs.dedup();
        Ok(refs)
    }
}

fn brief_refs(brief: &TaskBrief, refs: &mut Vec<ObjectRef>) -> Result<(), Refusal> {
    encoded(brief, COMMAND_BYTES)?;
    for values in [
        &brief.scope.operations,
        &brief.scope.objects,
        &brief.scope.fields,
        &brief.scope.tracks,
        &brief.scope.bus_sidechain_closure,
    ] {
        if values.len() > ITEM_LIMIT || values.iter().any(|s| s.len() > 256) {
            return Err(Refusal::Capacity);
        }
    }
    if brief.scope.asset_roots.len() > ITEM_LIMIT
        || brief.scope.consent_refs.len() > ITEM_LIMIT
        || brief.provider.options.len() > 64
    {
        return Err(Refusal::Capacity);
    }
    if brief.scope.base.checkpoint.record_kind != RecordKind::BranchCheckpoint
        || brief.inputs.record_kind != RecordKind::InputManifest
    {
        return Err(Refusal::InvalidState);
    }
    refs.extend([
        brief.plan.clone(),
        brief.script.clone(),
        brief.inputs.clone(),
        brief.assignment.clone(),
        brief.scope.base.checkpoint.clone(),
    ]);
    refs.extend(brief.scope.asset_roots.clone());
    refs.extend(brief.scope.consent_refs.clone());
    Ok(())
}

fn request_refs(request: &RuntimeRequest, refs: &mut Vec<ObjectRef>) {
    let reference = match request {
        RuntimeRequest::Question(question) => {
            if let super::orchestration_records::QuestionStatus::Answered(answer) = &question.status
            {
                refs.push(answer.clone());
            }
            question.prompt.clone()
        }
        RuntimeRequest::Approval(attention) | RuntimeRequest::Secret(attention) => {
            attention.request_ref.clone()
        }
    };
    refs.push(reference);
}

fn projection_refs(state: &RunProjection, refs: &mut Vec<ObjectRef>) -> Result<(), Refusal> {
    validate_projection(state)?;
    for task in &state.tasks {
        brief_refs(&task.brief, refs)?;
        refs.extend(task.progress.feedback.clone());
        refs.extend(task.progress.decisions.clone());
        match &task.state {
            TaskState::Terminal(result) => refs.push(result.clone()),
            TaskState::CleanupBlocked(cleanup) => refs.push(cleanup.recovery_ref.clone()),
            _ => {}
        }
    }
    refs.extend(state.effects.iter().map(|effect| effect.payload.clone()));
    refs.extend(
        state
            .deliveries
            .iter()
            .map(|delivery| delivery.result.clone()),
    );
    for attempt in &state.attempts {
        refs.push(attempt.recovery_ref.clone());
        if let AttemptState::CleanupBlocked(cleanup) = &attempt.state {
            refs.push(cleanup.recovery_ref.clone());
        }
    }
    for request in &state.requests {
        request_refs(request, refs);
    }
    refs.extend(state.results.clone());
    refs.extend(state.usage.clone());
    match &state.outcome {
        RunOutcome::Failed(reason) | RunOutcome::Cancelled(reason) => {
            refs.extend(reason.detail_ref.clone());
        }
        _ => {}
    }
    match &state.wait {
        RunWait::Files(detail) | RunWait::Provider(detail) => {
            refs.push(detail.reason.clone());
            refs.extend(detail.missing.clone());
        }
        _ => {}
    }
    match &state.cleanup {
        RunCleanup::Pending(identities) | RunCleanup::Blocked(identities, _) => {
            refs.extend(
                identities
                    .iter()
                    .map(|identity| identity.recovery_ref.clone()),
            );
        }
        RunCleanup::Settled => {}
    }
    if let RunCleanup::Blocked(_, reason) = &state.cleanup {
        refs.push(reason.clone());
    }
    Ok(())
}

fn validate_attempt_ids(state: &RunProjection) -> Result<(), Refusal> {
    if state
        .attempts
        .iter()
        .map(|reservation| &reservation.identity.attempt)
        .collect::<BTreeSet<_>>()
        .len()
        != state.attempts.len()
    {
        return Err(Refusal::InvalidState);
    }
    Ok(())
}

fn validate_cleanup(state: &RunProjection) -> Result<(), Refusal> {
    use super::orchestration_records::CleanupIdentity;
    let mut required = Vec::new();
    let mut blocked = false;
    for reservation in &state.attempts {
        if matches!(reservation.state, AttemptState::Retired(_)) {
            continue;
        }
        let identity = CleanupIdentity {
            attempt: reservation.identity.attempt.clone(),
            nonce: reservation.identity.nonce.clone(),
            owner_instance: reservation.identity.owner_instance.clone(),
            recovery_ref: reservation.recovery_ref.clone(),
        };
        if let AttemptState::CleanupBlocked(detail) = &reservation.state {
            if *detail != identity {
                return Err(Refusal::ScopeConflict);
            }
            blocked = true;
        }
        required.push(identity);
    }
    for task in &state.tasks {
        if let TaskState::CleanupBlocked(identity) = &task.state {
            if !required.contains(identity)
                || !state.attempts.iter().any(|reservation| {
                    reservation.identity.attempt == identity.attempt
                        && reservation.identity.task == task.id
                })
            {
                return Err(Refusal::ScopeConflict);
            }
            blocked = true;
        }
    }
    match &state.cleanup {
        RunCleanup::Settled if required.is_empty() && !blocked => Ok(()),
        RunCleanup::Pending(identities) if !required.is_empty() && !blocked => {
            same_cleanup_set(&required, identities)
        }
        RunCleanup::Blocked(identities, _) if !required.is_empty() && blocked => {
            same_cleanup_set(&required, identities)
        }
        _ => Err(Refusal::InvalidState),
    }
}

fn same_cleanup_set(
    required: &[super::orchestration_records::CleanupIdentity],
    actual: &[super::orchestration_records::CleanupIdentity],
) -> Result<(), Refusal> {
    if required.len() != actual.len()
        || required.iter().any(|identity| {
            actual
                .iter()
                .filter(|candidate| *candidate == identity)
                .count()
                != 1
        })
    {
        return Err(Refusal::InvalidState);
    }
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "one complete projection invariant check"
)]
pub fn validate_projection(state: &RunProjection) -> Result<(), Refusal> {
    if state.tasks.len() > TASK_LIMIT
        || state.attempts.len() > ATTEMPT_LIMIT
        || [
            state.effects.len(),
            state.deliveries.len(),
            state.requests.len(),
            state.results.len(),
            state.usage.len(),
        ]
        .into_iter()
        .any(|n| n > ITEM_LIMIT)
    {
        return Err(Refusal::Capacity);
    }
    let native_slots = state
        .attempts
        .iter()
        .filter(|attempt| !matches!(attempt.state, AttemptState::Retired(_)))
        .count();
    let reserved_slots = state
        .tasks
        .iter()
        .filter(|task| {
            task.progress.capacity == super::orchestration_records::DependencyReservation::Reserved
                && !state.attempts.iter().any(|attempt| {
                    attempt.identity.task == task.id
                        && !matches!(attempt.state, AttemptState::Retired(_))
                })
        })
        .count();
    if native_slots + reserved_slots > ATTEMPT_LIMIT {
        return Err(Refusal::DependencyCapacity);
    }
    encoded(state, CONTROL_BYTES)?;
    validate_attempt_ids(state)?;
    validate_cleanup(state)?;
    let tasks: BTreeSet<_> = state.tasks.iter().map(|task| &task.id).collect();
    if tasks.len() != state.tasks.len()
        || state.tasks.iter().any(|task| {
            task.parent
                .as_ref()
                .is_some_and(|parent| !tasks.contains(parent))
        })
        || state
            .effects
            .iter()
            .any(|effect| !tasks.contains(&effect.task))
        || state
            .deliveries
            .iter()
            .any(|delivery| !tasks.contains(&delivery.task))
    {
        return Err(Refusal::InvalidState);
    }
    for task in &state.tasks {
        if let super::orchestration_records::DependencyReservation::Backed(id) =
            &task.progress.capacity
            && !state
                .attempts
                .iter()
                .any(|attempt| attempt.identity.task == task.id && &attempt.identity.attempt == id)
        {
            return Err(Refusal::ScopeConflict);
        }
        if task.progress.expired && task.progress.turn_ended {
            return Err(Refusal::InvalidState);
        }
        if task.progress.turn.as_ref().is_some_and(|turn| {
            turn.task != task.id
                || turn.ordinal == 0
                || turn.expected_head != task.brief.scope.base.head
        }) || task.progress.background > super::orchestration_records::BACKGROUND_LIMIT
            || task.progress.feedback.len() > ITEM_LIMIT
            || task.progress.decisions.len() > ITEM_LIMIT
        {
            return Err(Refusal::InvalidState);
        }
        let mut ancestors = BTreeSet::new();
        let mut parent = task.parent.as_ref();
        while let Some(id) = parent {
            if id == &task.id || !ancestors.insert(id) {
                return Err(Refusal::InvalidState);
            }
            parent = state
                .tasks
                .iter()
                .find(|task| &task.id == id)
                .and_then(|task| task.parent.as_ref());
        }
    }
    let question_ids = state
        .requests
        .iter()
        .filter_map(|request| {
            if let RuntimeRequest::Question(q) = request {
                Some(&q.id)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    if question_ids.iter().collect::<BTreeSet<_>>().len() != question_ids.len()
        || state.requests.iter().any(|request| matches!(request, RuntimeRequest::Question(q) if q.task != q.origin.task || !tasks.contains(&q.task))) { return Err(Refusal::InvalidState); }
    for attempt in &state.attempts {
        attempt.identity.validate()?;
        if attempt.identity.run != state.run || !tasks.contains(&attempt.identity.task) {
            return Err(Refusal::ScopeConflict);
        }
    }
    if state
        .effects
        .iter()
        .map(|e| &e.id)
        .collect::<BTreeSet<_>>()
        .len()
        != state.effects.len()
        || state
            .effects
            .iter()
            .map(|e| (&e.lane, e.order))
            .collect::<BTreeSet<_>>()
            .len()
            != state.effects.len()
        || state
            .deliveries
            .iter()
            .map(|d| &d.id)
            .collect::<BTreeSet<_>>()
            .len()
            != state.deliveries.len()
    {
        return Err(Refusal::InvalidState);
    }
    if matches!(&state.wait, RunWait::Children(ids) if ids.is_empty() || ids.len() > TASK_LIMIT || ids.iter().any(|id| !tasks.contains(id)))
        || state.tasks.iter().any(|task| {
            matches!(&task.state, TaskState::WaitingForChildren(ids)
            if ids.is_empty() || ids.len() > TASK_LIMIT || ids.iter().any(|id| !tasks.contains(id)))
        })
    {
        return Err(Refusal::InvalidState);
    }
    if state.outcome == RunOutcome::Ready
        && (state.phase != RunPhase::Finished
            || state.cleanup != RunCleanup::Settled
            || state.wait != RunWait::None
            || state
                .tasks
                .iter()
                .any(|task| !(matches!(task.state, TaskState::Terminal(_))
                    || task.parent.is_some() && task.state == TaskState::Cancelled))
            || state.effects.iter().any(|effect| {
                !matches!(effect.state, EffectState::Settled | EffectState::Disposed)
            })
            || state.deliveries.iter().any(|delivery| {
                !matches!(
                    delivery.state,
                    DeliveryState::Observed(_) | DeliveryState::Disposed
                )
            })
            || state.requests.iter().any(|request| !matches!(request, RuntimeRequest::Question(q) if q.status != super::orchestration_records::QuestionStatus::Pending)))
    {
        return Err(Refusal::InvalidState);
    }
    Ok(())
}
