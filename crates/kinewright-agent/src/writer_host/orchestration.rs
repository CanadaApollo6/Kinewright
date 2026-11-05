//! H3 resolves a live attachment before either receipt lookup or scheduler admission.
#![allow(clippy::missing_errors_doc)]
use super::{
    AttachmentRole, HostAuth, HostCommand, HostCommandOwner, HostError, HostRefusal, HostReply,
    SessionRoots,
};
use crate::orchestration::{
    machine,
    orchestration_commit::{self, OrchestrationReceipt, ValidatedRunUpdate},
    orchestration_records::{
        AttemptId, AttemptState, EndpointId, ExecutionIdentity, OrchestrationCommand,
        OrchestrationEnvelope, Refusal, Role, RunAction, RunActionKind, RunCleanup, RunId,
        RunOutcome, RunPhase, RunProjection, RuntimeRequest, TaskId, TaskScope, TaskState,
    },
    tasks::{EditorTask, RuntimeTaskAuthority, TaskEndpoint},
};
use kinewright_project::{
    bundle::BundleError,
    bundle_records::ObjectRef,
    receipts::{CommandId, IssuerId},
    studio_records::{FeedbackBatchId, MetadataEnvelope},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopedOrchestrationRequest {
    pub run: Option<RunId>,
    pub envelope: OrchestrationEnvelope,
    pub consume_batch: Option<FeedbackBatchId>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataIssuer {
    pub envelope: MetadataEnvelope,
    pub root_generation: u64,
    pub grant_ref: ObjectRef,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunViewPage {
    pub projection: RunProjection,
    #[serde(default)]
    pub accounting: Option<DurableRunAccounting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_status: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub completions: Vec<super::CompletionStatus>,
    pub full_refresh: bool,
    pub allowed_actions: Vec<RunActionKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableRunAccounting {
    pub stop_reason: Option<String>,
    pub start_unix_ms: u64,
    pub deadline_unix_ms: u64,
    pub last_unix_ms: u64,
    pub elapsed_ms: u64,
    pub clock_uncertain: bool,
    pub tasks: Vec<DurableTaskAccounting>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableTaskAccounting {
    pub task: TaskId,
    pub sends: usize,
    pub provider: Option<kinewright_project::orchestration_accounting::ProviderMetadata>,
    pub totals: kinewright_project::orchestration_accounting::UsageTotals,
    pub wait_reason: Option<String>,
    pub pause: Option<kinewright_project::orchestration_accounting::AccountingRefusal>,
    /// C5 fix 10: refused product MCP calls, by tool and fixed code.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refusals: Vec<kinewright_project::orchestration_accounting::ToolRefusal>,
}
#[derive(Clone)]
pub(super) struct ProducerScope {
    pub run: RunId,
    pub task: Option<TaskId>,
    pub attempt: Option<AttemptId>,
    pub endpoint: Option<EndpointId>,
    pub owner_epoch: u64,
    // The private run Producer ends with its host/epoch. Task grants have deadlines.
    pub expires_at: Option<Instant>,
    pub scope: TaskScope,
}
impl ProducerScope {
    pub(super) fn expired(&self) -> bool {
        self.expires_at
            .is_some_and(|deadline| Instant::now() >= deadline)
    }
}
/// Immutable provenance for every attempt-derived runtime authorization.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct RuntimeGrantBinding {
    pub run: RunId,
    pub task: TaskId,
    pub attempt: AttemptId,
    pub owner_epoch: u64,
}
impl RuntimeGrantBinding {
    pub(super) fn live_in(&self, projection: &RunProjection, epoch: Option<u64>) -> bool {
        self.run == projection.run
            && Some(self.owner_epoch) == epoch
            && projection.outcome == RunOutcome::Pending
            && projection.phase != RunPhase::Finished
            && projection.tasks.iter().any(|task| {
                task.id == self.task
                    && !task.progress.expired
                    && !matches!(
                        task.state,
                        TaskState::Terminal(_)
                            | TaskState::Cancelled
                            | TaskState::Cancelling
                            | TaskState::CleanupBlocked(_)
                    )
            })
            && projection.attempts.iter().any(|attempt| {
                attempt.identity.run == self.run
                    && attempt.identity.task == self.task
                    && attempt.identity.attempt == self.attempt
                    && attempt.owner_epoch == self.owner_epoch
                    && !matches!(
                        attempt.state,
                        AttemptState::Stopping
                            | AttemptState::CleanupBlocked(_)
                            | AttemptState::Retired(_)
                    )
            })
    }
}
pub(super) struct HostRun {
    pub recovery_blocks: Vec<super::task_completion::RecoveryBlock>,
    #[cfg(target_os = "linux")]
    pub measurements:
        BTreeMap<TaskId, Arc<Mutex<kinewright_project::usage_measurement::Measurements>>>,
    pub producer: HostAuth,
    pub inputs: super::continuation::HostOwnedInputClosure,
    pub tasks: BTreeMap<TaskId, EditorTask>,
    pub continuable: bool,
}
#[derive(Default)]
pub(super) struct Hooks {
    pub completion_failure: Option<(CommandId, String)>,
    #[cfg(test)]
    pub completion_cut: Option<crate::orchestration::orchestration_records::CompletionStep>,
    #[cfg(test)]
    pub completion_before_freeze: bool,
    #[cfg(test)]
    pub completion_status_read_failure: bool,
    pub runs: BTreeMap<RunId, HostRun>,
    pub pending: Option<PendingPublication>,
    pub attention: Arc<Mutex<super::attention::AttentionState>>,
    #[cfg(target_os = "linux")]
    pub native: BTreeMap<(RunId, AttemptId), super::native_invocation::RegisteredNativeAttempt>,
}
impl Drop for Hooks {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        for attempt in self.native.values() {
            attempt.control.close();
        }
        for run in self.runs.values() {
            for task in run.tasks.values() {
                task.endpoint().revoke();
            }
        }
        self.attention
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .revoke_grants();
    }
}
#[derive(Clone, PartialEq, Eq, Serialize)]
pub(super) enum PendingPublication {
    Wire(Box<ScopedOrchestrationRequest>, super::auth::AttachmentId),
    Owner(Box<ValidatedRunUpdate>),
    #[cfg(target_os = "linux")]
    Cumulative(
        Box<crate::orchestration::ExecutionIdentity>,
        Box<kinewright_project::orchestration_accounting::AccountingRecord>,
    ),
}

fn refused(reason: &str) -> HostError {
    HostRefusal::new("orchestration_refused", reason).into()
}
/// The fixed explanation grant of every frozen task result.
pub(super) const TASK_RESULT_EXPLANATION: &[u8] = b"task result";
#[expect(
    clippy::needless_pass_by_value,
    reason = "Result::map_err supplies the owned refusal"
)]
fn scheduler_error(reason: Refusal) -> HostError {
    refused(&format!("{reason:?}"))
}
pub(super) fn is_hook(command: &HostCommand) -> bool {
    matches!(
        command,
        HostCommand::ReconcileTaskReceipt { .. }
            | HostCommand::IssueMetadataIssuer { .. }
            | HostCommand::DispatchOrchestration { .. }
            | HostCommand::ObserveRun { .. }
            | HostCommand::ResolveAttention { .. }
            | HostCommand::DetachForContinuation
            | HostCommand::ReviseCut { .. }
    )
}

impl HostCommandOwner {
    fn sync_runtime_grants(&mut self, projection: &RunProjection) {
        let epoch = self
            .owner()
            .and_then(|owner| Ok(owner.store()?.owner_epoch()))
            .ok();
        #[cfg(target_os = "linux")]
        self.hooks.native.retain(|_, attempt| {
            if attempt.identity.run != projection.run {
                return true;
            }
            let binding = RuntimeGrantBinding {
                run: attempt.identity.run.clone(),
                task: attempt.identity.task.clone(),
                attempt: attempt.identity.attempt.clone(),
                owner_epoch: attempt.owner_epoch,
            };
            let draining = Some(attempt.owner_epoch) == epoch
                && projection.outcome == RunOutcome::Pending
                && projection.tasks.iter().any(|task| {
                    task.id == attempt.identity.task
                        && matches!(task.state, TaskState::Terminal(_))
                        && !task.progress.expired
                })
                && projection.attempts.iter().any(|reservation| {
                    reservation.identity == attempt.identity
                        && reservation.state == AttemptState::Executing
                });
            if !binding.live_in(projection, epoch) && !draining {
                attempt.control.close();
            }
            !attempt.control.closed()
        });
        for attachment in &self.attachments {
            if let Some(scope) = &attachment.producer_scope
                && scope.run == projection.run
                && let (Some(task), Some(attempt)) = (&scope.task, &scope.attempt)
                && (scope.expired()
                    || !(RuntimeGrantBinding {
                        run: scope.run.clone(),
                        task: task.clone(),
                        attempt: attempt.clone(),
                        owner_epoch: scope.owner_epoch,
                    })
                    .live_in(projection, epoch))
            {
                attachment
                    .alive
                    .store(false, std::sync::atomic::Ordering::Release);
            }
        }
        if let Some(run) = self.hooks.runs.get(&projection.run) {
            for (id, task) in &run.tasks {
                let endpoint = task.endpoint();
                let authority = endpoint.authority();
                let binding = RuntimeGrantBinding {
                    run: authority.run.clone(),
                    task: authority.task.clone(),
                    attempt: authority.attempt.clone(),
                    owner_epoch: authority.owner_epoch,
                };
                let current = projection.tasks.iter().find(|record| &record.id == id);
                if !binding.live_in(projection, epoch)
                    || Instant::now() >= authority.expires_at
                    || current.is_none_or(|record| !endpoint.matches_brief(&record.brief))
                {
                    endpoint.revoke();
                }
                endpoint.set_paused(
                    matches!(projection.phase, RunPhase::Paused | RunPhase::Interrupted)
                        || matches!(projection.cleanup, RunCleanup::Blocked(_, _)),
                );
            }
        }
        match self.hooks.attention.lock() {
            Ok(mut attention) => attention.sync_grants(projection, epoch),
            Err(poisoned) => poisoned.into_inner().revoke_grants(),
        }
    }
    pub(super) fn check_producer_attachment(
        &self,
        index: usize,
        command: &HostCommand,
    ) -> Result<(), HostError> {
        let Some(scope) = &self.attachments[index].producer_scope else {
            return Ok(());
        };
        if matches!(command, HostCommand::Detach) {
            return Ok(());
        }
        if scope.expired() || scope.owner_epoch != self.owner()?.store()?.owner_epoch() {
            return Err(HostRefusal::new(
                "producer_expired",
                "the task Producer must be freshly authorized",
            )
            .into());
        }
        let allowed = match command {
            HostCommand::Status
            | HostCommand::IssueMetadataIssuer { .. }
            | HostCommand::DispatchOrchestration { .. }
            | HostCommand::ObserveRun { .. }
            | HostCommand::SnapshotLeaseStatus { .. }
            | HostCommand::ReleaseSnapshot { .. } => true,
            HostCommand::ReadSnapshot { key } | HostCommand::LeaseSnapshot { key } => {
                key.head == scope.scope.base.head
            }
            HostCommand::ReadSubmittedBatch { batch } => self
                .owner()?
                .orchestration_cut(&scope.run)?
                .is_some_and(|cut| cut.consumed_batch.as_ref() == Some(batch)),
            _ => false,
        };
        if !allowed {
            return Err(HostRefusal::new(
                "producer_scope",
                "task Producer cannot use the general host command path",
            )
            .into());
        }
        Ok(())
    }
    pub(super) fn metadata_subject(&self, index: usize) -> String {
        self.attachments[index].subject.clone()
    }
    pub(super) fn require_metadata_issuer(
        &self,
        index: usize,
        issuer: &IssuerId,
    ) -> Result<(), HostError> {
        if self.owner()?.metadata_issuer_subject(issuer)?.as_deref()
            != Some(self.metadata_subject(index).as_str())
        {
            return Err(HostRefusal::new(
                "metadata_issuer",
                "issuer belongs to another attachment subject",
            )
            .into());
        }
        Ok(())
    }
    pub(super) fn issue_metadata(
        &mut self,
        index: usize,
        issuer: Option<&IssuerId>,
    ) -> Result<MetadataIssuer, HostError> {
        if self.hooks.pending.is_some() {
            return Err(BundleError::Unsettled.into());
        }
        if let Some(issuer) = issuer {
            self.require_metadata_issuer(index, issuer)?;
        }
        let grant = serde_json::to_vec(&(
            &self.instance,
            &self.attachments[index].subject,
            self.attachments[index].role,
            &self.attachments[index].roots,
        ))?;
        let subject = self.metadata_subject(index);
        // Ordinary admission publishes the issuer and grant at once, so a
        // receipt query after a host restart can still prove absence. Only a
        // closed ordinary admission leaves them as an unpublished description,
        // which the first validated publication commits under its own admission.
        let owner = self.owner_mut()?;
        let published = owner.issue_metadata_authority(&subject, issuer, &grant);
        let (issuer, grant_ref) = match published {
            Ok(published) => published,
            Err(BundleError::RunLimit) => {
                owner.prepare_metadata_authority(&subject, issuer, &grant)?
            }
            Err(error) => return Err(error.into()),
        };
        let sequence = self.owner()?.metadata_issuer_sequence(&issuer)?;
        let envelope = self
            .owner()?
            .metadata_envelope(CommandId { issuer, sequence })?;
        Ok(MetadataIssuer {
            envelope,
            root_generation: self.owner()?.metadata_generation()?,
            grant_ref,
        })
    }
    pub(super) fn expected_grant(&self, index: usize) -> Result<ObjectRef, HostError> {
        let bytes = serde_json::to_vec(&(
            &self.instance,
            &self.attachments[index].subject,
            self.attachments[index].role,
            &self.attachments[index].roots,
        ))?;
        Ok(ObjectRef {
            sha256: kinewright_media::sha256_bytes(&bytes),
            byte_length: bytes.len() as u64,
            record_kind: kinewright_project::bundle_records::RecordKind::OwnedEvidence,
        })
    }
    pub(super) fn execute_hook(
        &mut self,
        index: usize,
        command: HostCommand,
        roots: &SessionRoots,
    ) -> HostReply {
        let mut reply = HostReply::default();
        let read = matches!(
            command,
            HostCommand::ObserveRun { .. }
                | HostCommand::ReconcileTaskReceipt { .. }
                | HostCommand::DetachForContinuation
        );
        let result = (|| {
            if !read {
                #[cfg(target_os = "linux")]
                if !matches!(command, HostCommand::IssueMetadataIssuer { .. })
                    && (self.native_recovery_pending || self.recovery_only)
                {
                    let status = self
                        .native_recovery_status
                        .as_deref()
                        .unwrap_or("cleanup pending");
                    return Err(HostRefusal::new("recovery_frozen", status).into());
                }
                self.prove_retention()?;
                if self.recovering || self.switch_frozen {
                    return Err(HostRefusal::new(
                        "recovery_frozen",
                        "orchestration writes wait for recovery confirmation",
                    )
                    .into());
                }
            }
            for pin in &self.pins {
                if !roots.permits(&pin.anchor, !read) {
                    return Err(
                        HostRefusal::roots("forwarded roots exclude the named project").into(),
                    );
                }
            }
            match command {
                HostCommand::ReconcileTaskReceipt { query } => {
                    reply.task_receipt = Some(self.reconcile_task_receipt(index, &query)?);
                }
                HostCommand::IssueMetadataIssuer { issuer } => {
                    reply.metadata_issuer = Some(self.issue_metadata(index, issuer.as_ref())?);
                }
                HostCommand::DispatchOrchestration { request } => {
                    reply.orchestration = Some(self.dispatch_orchestration(index, &request)?);
                }
                HostCommand::ObserveRun {
                    run,
                    after_sequence,
                } => {
                    reply.run_view = Some(self.observe_run(index, &run, after_sequence)?);
                    reply.attention = self.attention_prompts(&run);
                }
                HostCommand::ResolveAttention { response } => {
                    reply.attention_outcome = Some(self.resolve_attention(index, response)?);
                }
                HostCommand::DetachForContinuation => {
                    reply.detached = Some(self.detach_for_continuation(index)?);
                }
                HostCommand::ReviseCut { request } => {
                    reply.revise_result = Some(self.revise_cut(index, &request)?);
                }
                _ => return Err(refused("not an orchestration command")),
            }
            Ok::<(), HostError>(())
        })();
        if let Err(error) = result {
            reply.error = Some(error.refusal());
        }
        reply.status = self.status();
        reply
    }
    pub(super) fn authorize_orchestration(
        &self,
        index: usize,
        request: &ScopedOrchestrationRequest,
    ) -> Result<(), HostError> {
        #[cfg(target_os = "linux")]
        if self.native_recovery_pending || self.recovery_only {
            let status = self
                .native_recovery_status
                .as_deref()
                .unwrap_or("cleanup pending");
            return Err(HostRefusal::new("recovery_frozen", status).into());
        }
        let attachment = &self.attachments[index];
        self.require_metadata_issuer(index, &request.envelope.id.issuer)?;
        request.envelope.validate().map_err(scheduler_error)?;
        if request.envelope.project != self.owner()?.active_manual_project()? {
            return Err(refused("project changed"));
        }
        match attachment.role {
            AttachmentRole::Viewer => {
                return Err(HostRefusal::new(
                    "role_refused",
                    "Viewer cannot start or control runs",
                )
                .into());
            }
            AttachmentRole::Editor => match &request.envelope.command {
                OrchestrationCommand::StartRun(start) if request.run.is_none() => {
                    if request.envelope.authority.endpoint.is_some()
                        || request.envelope.authority.attempt.is_some()
                    {
                        return Err(refused("Editor cannot impersonate a task endpoint"));
                    }
                    self.owner()?
                        .validate_orchestration_scope(&request.envelope.project, &start.director)?;
                }
                OrchestrationCommand::ControlRun(intent)
                    if request.run.as_ref() == Some(&intent.run_id) =>
                {
                    if request.consume_batch.is_some()
                        || request.envelope.authority.endpoint.is_some()
                        || request.envelope.authority.attempt.is_some()
                    {
                        return Err(refused("control scope changed"));
                    }
                    let state = self
                        .owner()?
                        .orchestration_projection(&intent.run_id)?
                        .ok_or_else(|| refused("unknown run"))?;
                    if intent.action == RunAction::Continue
                        && !self.run_recovery_blocks(&state)?.is_empty()
                    {
                        return Err(refused(
                            "recovery_requires_person: unfinished task completion or merge",
                        ));
                    }
                    for task in &state.tasks {
                        self.owner()?
                            .validate_orchestration_scope(&request.envelope.project, &task.brief)?;
                    }
                }
                _ => {
                    return Err(HostRefusal::new(
                        "role_refused",
                        "Editor endpoint only starts and controls admitted runs",
                    )
                    .into());
                }
            },
            AttachmentRole::Producer => self.authorize_producer(index, request)?,
        }
        let _ = self.live_attention_admission(request)?;
        Ok(())
    }
    fn authorize_producer(
        &self,
        index: usize,
        request: &ScopedOrchestrationRequest,
    ) -> Result<(), HostError> {
        let scope = self.attachments[index]
            .producer_scope
            .as_ref()
            .ok_or_else(|| refused("Producer has no run scope"))?;
        if scope.expired()
            || request.run.as_ref() != Some(&scope.run)
            || request.envelope.authority.owner_epoch != scope.owner_epoch
            || request.envelope.authority.endpoint != scope.endpoint
            || request.envelope.authority.attempt != scope.attempt
            || scope.task.is_none()
            || scope.endpoint.is_none()
            || scope.expires_at.is_none()
            || request.consume_batch.is_some()
        {
            return Err(refused("Producer scope or expiry"));
        }
        let state = self
            .owner()?
            .orchestration_projection(&scope.run)?
            .ok_or_else(|| refused("unknown run"))?;
        let task = state
            .tasks
            .iter()
            .find(|task| Some(&task.id) == scope.task.as_ref())
            .ok_or_else(|| refused("unknown task"))?;
        if task.brief.scope != scope.scope
            || task.progress.expired
            || matches!(
                task.state,
                TaskState::Terminal(_)
                    | TaskState::Cancelled
                    | TaskState::Cancelling
                    | TaskState::CleanupBlocked(_)
            )
            || !state.attempts.iter().any(|attempt| {
                Some(&attempt.identity.attempt) == scope.attempt.as_ref()
                    && attempt.identity.task == task.id
                    && attempt.owner_epoch == scope.owner_epoch
                    && !matches!(
                        attempt.state,
                        AttemptState::Retired(_)
                            | AttemptState::Stopping
                            | AttemptState::CleanupBlocked(_)
                    )
            })
        {
            return Err(refused("Producer task or attempt is no longer live"));
        }
        let own_task = |id: &TaskId| Some(id) == scope.task.as_ref();
        let child = |id: &TaskId| {
            task.brief.role == Role::Director
                && state.tasks.iter().any(|child| {
                    &child.id == id
                        && child.parent.as_ref() == Some(&task.id)
                        && scope_contains(&scope.scope, &child.brief.scope)
                })
        };
        let allowed = match &request.envelope.command {
            OrchestrationCommand::FinishTask(finish) => own_task(&finish.task),
            OrchestrationCommand::AssignTask(assignment) => {
                assignment.run == scope.run
                    && own_task(&assignment.parent)
                    && task.brief.role == Role::Director
                    && scope_contains(&scope.scope, &assignment.brief.scope)
            }
            OrchestrationCommand::BeginTurn(turn)
            | OrchestrationCommand::EndTurn(turn)
            | OrchestrationCommand::BackgroundWork(turn, _) => {
                own_task(&turn.task)
                    && Some(&turn.attempt) == scope.attempt.as_ref()
                    && turn.owner_epoch == scope.owner_epoch
            }
            OrchestrationCommand::RecordContext { task, .. } => own_task(task),
            OrchestrationCommand::FreezeTask(target) | OrchestrationCommand::CancelTask(target) => {
                own_task(target) || child(target)
            }
            OrchestrationCommand::PublishResult(result) => {
                own_task(&result.task) || child(&result.task)
            }
            OrchestrationCommand::MergeTask(merge) => {
                task.brief.role == Role::Director && (own_task(&merge.task) || child(&merge.task))
            }
            OrchestrationCommand::ObserveDelivery(id) => state.deliveries.iter().any(|delivery| {
                &delivery.id == id
                    && state.tasks.iter().any(|child| {
                        child.id == delivery.task && child.parent.as_ref() == Some(&task.id)
                    })
            }),
            OrchestrationCommand::ControlRun(intent) => {
                task.brief.role == Role::Director
                    && intent.run_id == scope.run
                    && matches!(
                        intent.action,
                        crate::orchestration::orchestration_records::RunAction::SendMessage { .. }
                    )
            }
            OrchestrationCommand::Request(RuntimeRequest::Question(question)) => {
                own_task(&question.task) && Some(&question.origin.attempt) == scope.attempt.as_ref()
            }
            OrchestrationCommand::Request(
                RuntimeRequest::Approval(attention) | RuntimeRequest::Secret(attention),
            ) => own_task(&attention.task) && Some(&attention.attempt) == scope.attempt.as_ref(),
            OrchestrationCommand::AnswerQuestion(_) => task.brief.role == Role::Director,
            _ => false,
        };
        if !allowed {
            return Err(HostRefusal::new(
                "role_refused",
                "Producer cannot mint authority or escape its task scope",
            )
            .into());
        }
        Self::authorize_completion_command(&state, &request.envelope)?;
        Ok(())
    }
    /// C5 fix 10: bind a publication's host-owned `usage_ref`. The model may
    /// echo any value; it is never compared or trusted. A first publication
    /// names the accounting record current in this admission. A re-sent
    /// publication of an already published task is identified by its task,
    /// base, head and delta, and is rebound to the stored result, so it
    /// replays the same publication however many accounting records followed.
    pub(super) fn bind_publication(
        &self,
        request: &ScopedOrchestrationRequest,
    ) -> Result<ScopedOrchestrationRequest, HostError> {
        let OrchestrationCommand::PublishResult(publish) = &request.envelope.command else {
            return Ok(request.clone());
        };
        let Some(run) = &request.run else {
            return Ok(request.clone());
        };
        let Some(state) = self.owner()?.orchestration_projection(run)? else {
            return Ok(request.clone());
        };
        let stored = state
            .tasks
            .iter()
            .find(|task| task.id == publish.task)
            .and_then(|task| {
                if let TaskState::Terminal(reference) = &task.state {
                    Some(reference.clone())
                } else {
                    None
                }
            });
        let result = if let Some(reference) = stored {
            let record = self.owner()?.store()?.read_run_record(&reference)?;
            let crate::orchestration::orchestration_records::OrchestrationPayload::Result(stored) =
                record.payload
            else {
                return Err(refused("published result changed"));
            };
            // Deltas are content-addressed, so equal references mean equal deltas.
            let mut original = publish.result.clone();
            original.usage_ref = stored.usage_ref.clone();
            if *stored != original {
                return Err(refused("published result changed"));
            }
            *stored
        } else {
            let mut result = publish.result.clone();
            result.usage_ref = self.current_accounting_reference(&state)?;
            result
        };
        let mut bound = request.clone();
        bound.envelope.command = OrchestrationCommand::PublishResult(Box::new(
            crate::orchestration::orchestration_records::PublishResult {
                task: publish.task.clone(),
                result,
            },
        ));
        bound.envelope.digest = bound
            .envelope
            .computed_digest()
            .map_err(|_| refused("invalid publication"))?;
        Ok(bound)
    }
    pub(super) fn dispatch_orchestration(
        &mut self,
        index: usize,
        incoming: &ScopedOrchestrationRequest,
    ) -> Result<OrchestrationReceipt, HostError> {
        // Bounded owner reads/replay finish before ordinary staging begins.
        // Long-lived snapshot readers never receive this allowance.
        if let Some(pending) = &self.hooks.pending {
            // An immutable attachment instance admitted this exact command before B4.
            // Scope records are unreadable while the namespace is uncertain; only that
            // still-live instance may reconcile, then all current policy runs again.
            if !matches!(pending, PendingPublication::Wire(original, actor) if &**original == incoming && *actor == self.attachments[index].id)
            {
                return Err(refused("original uncertain command must reconcile first"));
            }
            self.owner_mut()?.reconcile_metadata()?;
        }
        // A publication's usage reference is host bookkeeping, bound here in
        // the same admission that writes it. Pending publication blocks every
        // accounting change, so a reconciled retry binds the same reference.
        let bound = self.bind_publication(incoming)?;
        let request = &bound;
        self.authorize_orchestration(index, request)?;
        if let Some(receipt) = self
            .owner()?
            .orchestration_replay(&request.envelope, request.consume_batch.as_ref())?
        {
            let run = match &request.run {
                Some(run) => run.clone(),
                None => receipt.envelope.identity("run").map_err(scheduler_error)?,
            };
            if let Some(current) = self.owner()?.orchestration_projection(&run)? {
                self.sync_runtime_grants(&current);
                if let Some(runtime) = self.hooks.runs.get_mut(&run) {
                    runtime.continuable = current.outcome == RunOutcome::Pending
                        || current.cleanup != RunCleanup::Settled;
                }
            }
            self.hooks.pending = None;
            self.drive_completion(index, request, &receipt);
            return Ok(receipt);
        }
        if request.envelope.authority.owner_epoch != self.owner()?.store()?.owner_epoch()
            || request.envelope.authority.grant_ref != self.expected_grant(index)?
        {
            return Err(refused("fresh epoch or attachment grant changed"));
        }
        let owner = self.owner()?;
        let mut previous = request
            .run
            .as_ref()
            .map(|run| owner.orchestration_projection(run))
            .transpose()?
            .flatten();
        let previous_segment = request
            .run
            .as_ref()
            .map(|run| owner.orchestration_cut(run))
            .transpose()?
            .flatten()
            .map(|cut| cut.segment);
        // Freeze is an explicit completion request. H3, rather than a provider
        // turn command, ends the owning turn in the same cut as its frozen history.
        if let OrchestrationCommand::FreezeTask(target) = &request.envelope.command
            && let Some(state) = &mut previous
        {
            let mut ended = state.clone();
            if let Some(task) = ended.tasks.iter_mut().find(|task| &task.id == target)
                && task.progress.turn.as_ref().is_some_and(|turn| {
                    turn.purpose == crate::orchestration::orchestration_records::TurnPurpose::Work
                })
            {
                task.progress.turn_ended = true;
            }
            if crate::orchestration::tasks::decide_task(&ended, &request.envelope.command)
                .is_ok_and(|transition| transition.is_some())
            {
                *state = ended;
            }
        }
        let generation = self.owner()?.metadata_generation()?;
        self.check_finish_revision(request)?;
        let attention = self.live_attention_admission(request)?;
        let transition = if let OrchestrationCommand::PublishResult(publish) =
            &request.envelope.command
        {
            let state = previous
                .as_ref()
                .ok_or_else(|| refused("result run missing"))?;
            let history = self
                .saved_task_history(state, &publish.task)?
                .ok_or_else(|| refused("result history missing"))?;
            // A host restart between freeze and publish reloads the frozen task.
            if history.frozen {
                self.restore_frozen_task(state, &publish.task)?;
            }
            let runtime = self
                .hooks
                .runs
                .get(&state.run)
                .and_then(|run| run.tasks.get(&publish.task))
                .ok_or_else(|| refused("result task missing"))?;
            let delta = runtime
                .frozen_delta()
                .ok_or_else(|| refused("task is not frozen"))?
                .to_vec();
            let result = &publish.result;
            // `usage_ref` was bound by the host above; only base, head and
            // delta identify the frozen task.
            if !history.frozen
                || result.base.as_ref() != Some(&history.base)
                || result.head.as_ref() != Some(&history.source)
                || match &result.delta_ref {
                    // The delta is content-addressed: compare it with the
                    // frozen delta by identity, then admit those bytes so
                    // the publication's dependency is declared even when
                    // a restart dropped the freeze-time grant.
                    Some(reference) => {
                        let bytes = serde_json::to_vec(&delta)?;
                        reference.sha256 != kinewright_media::sha256_bytes(&bytes)
                            || reference.byte_length != bytes.len() as u64
                            || self.owner_mut()?.admit_host_grant(&bytes)? != *reference
                    }
                    None => !delta.is_empty(),
                }
            {
                return Err(refused("result differs from the frozen task"));
            }
            // A frozen result's explanation is the host's own fixed grant;
            // declare it again so a restart since the freeze cannot leave
            // the publication with a missing dependency.
            if result.explanation.sha256 == kinewright_media::sha256_bytes(TASK_RESULT_EXPLANATION)
                && result.explanation.byte_length == TASK_RESULT_EXPLANATION.len() as u64
            {
                self.owner_mut()?
                    .admit_host_grant(TASK_RESULT_EXPLANATION)?;
            }
            let record = self.owner_mut()?.admit_scheduler_record(
                &crate::orchestration::orchestration_records::OrchestrationRecord {
                    version: 1,
                    payload:
                        crate::orchestration::orchestration_records::OrchestrationPayload::Result(
                            Box::new(result.clone()),
                        ),
                },
            )?;
            crate::orchestration::mailbox::prepare_result_update(
                state,
                &request.envelope.project,
                generation,
                &request.envelope,
                previous_segment,
                record.reference,
            )
            .map_err(scheduler_error)?
        } else {
            machine::prepare_with_attention(
                previous.as_ref(),
                &self.owner()?.active_manual_project()?,
                generation,
                &request.envelope,
                previous_segment,
                attention.as_ref().map(|(proof, _)| proof),
            )
            .map_err(scheduler_error)?
        };
        self.owner_mut()?.run_transition_admission(&transition)?;
        let transition = self.seed_run_spending(transition)?;
        let transition = self.stage_control_freeze(transition)?;
        if let Some((_, bytes)) = attention {
            self.owner_mut()?.admit_host_grant(&bytes)?;
        }
        let newly_retained = !self.hooks.runs.contains_key(&transition.projection().run);
        if let OrchestrationCommand::StartRun(start) = &request.envelope.command {
            self.retain_run(&transition.projection().run, &start.director)?;
        }
        let run_id = transition.projection().run.clone();
        let published = transition.projection().clone();
        let continuable = transition.projection().outcome == RunOutcome::Pending
            || transition.projection().cleanup != RunCleanup::Settled;
        self.hooks.pending = Some(PendingPublication::Wire(
            Box::new(incoming.clone()),
            self.attachments[index].id,
        ));
        let result = orchestration_commit::commit_orchestration(
            self.owner_mut()?,
            transition,
            request.consume_batch.clone(),
        );
        match result {
            Ok(receipt) => {
                self.sync_runtime_grants(&published);
                self.hooks.pending = None;
                if let Some(run) = self.hooks.runs.get_mut(&run_id) {
                    run.continuable = continuable;
                }
                self.drive_completion(index, request, &receipt);
                Ok(receipt)
            }
            Err(error) => {
                if !matches!(
                    error,
                    BundleError::DurabilityUnknown | BundleError::Unsettled
                ) {
                    self.hooks.pending = None;
                    if newly_retained && let Some(runtime) = self.hooks.runs.remove(&run_id) {
                        self.attachments.retain(|attachment| {
                            !attachment.auth.accepts(Some(&runtime.producer.bearer()))
                        });
                    }
                }
                Err(error.into())
            }
        }
    }
    pub(super) fn observe_run(
        &self,
        index: usize,
        run: &RunId,
        after_sequence: u64,
    ) -> Result<RunViewPage, HostError> {
        let projection = self
            .owner()?
            .orchestration_projection(run)?
            .ok_or_else(|| refused("unknown run"))?;
        if let Some(scope) = &self.attachments[index].producer_scope {
            if &scope.run != run || scope.expired() {
                return Err(refused("Producer observation scope or expiry"));
            }
        } else if self.attachments[index].role == AttachmentRole::Producer {
            return Err(refused("Producer has no observation scope"));
        }
        #[cfg(target_os = "linux")]
        let runtime_status = self.native_recovery_status.clone();
        #[cfg(not(target_os = "linux"))]
        let runtime_status: Option<String> = None;
        #[cfg(target_os = "linux")]
        let recovery_ready = !self.native_recovery_pending;
        #[cfg(not(target_os = "linux"))]
        let recovery_ready = true;
        let allowed_actions =
            if recovery_ready && self.attachments[index].role == AttachmentRole::Editor {
                [
                    RunAction::Pause,
                    RunAction::Stop,
                    RunAction::Continue,
                    RunAction::SendMessage {
                        text_ref: projection.tasks[0].brief.assignment.clone(),
                    },
                ]
                .into_iter()
                .filter(|action| {
                    machine::eligible(&projection, action)
                        && (!matches!(action, RunAction::Continue)
                            || self
                                .run_recovery_blocks(&projection)
                                .is_ok_and(|blocks| blocks.is_empty()))
                })
                .map(|action| match action {
                    RunAction::Pause => RunActionKind::Pause,
                    RunAction::Stop => RunActionKind::Stop,
                    RunAction::SendMessage { .. } => RunActionKind::SendMessage,
                    RunAction::Continue => RunActionKind::Continue,
                })
                .collect()
            } else {
                vec![]
            };
        // C5 fix 10: the invariant witness. A stall the pump has not yet
        // surfaced is still reported, never shown as a running run.
        let runtime_status = runtime_status
            .or(self.owner()?.run_publication_status(run)?)
            .or(self.run_recovery_message(&projection)?)
            .or_else(|| {
                machine::silently_stalled(&projection).then(|| machine::SILENT_STALL.into())
            });
        let accounting = self.durable_run_accounting(&projection)?;
        Ok(RunViewPage {
            completions: self.completion_statuses(&projection)?,
            accounting,
            runtime_status,
            full_refresh: after_sequence == 0
                || after_sequence > projection.sequence
                || after_sequence.saturating_add(1) < projection.sequence,
            projection,
            allowed_actions,
        })
    }
    pub fn run_producer(&self, run: &RunId) -> Result<HostAuth, HostError> {
        Ok(self
            .hooks
            .runs
            .get(run)
            .ok_or_else(|| refused("run is not retained by this host"))?
            .producer
            .clone())
    }
    pub fn publish_orchestration_transition(
        &mut self,
        producer: &HostAuth,
        transition: &ValidatedRunUpdate,
        reservation: Option<&ExecutionIdentity>,
    ) -> Result<
        (
            OrchestrationReceipt,
            Option<crate::orchestration::AdmittedAttempt>,
        ),
        HostError,
    > {
        let index = self.resolve_attachment(Some(&producer.bearer()))?;
        let scope = self.attachments[index]
            .producer_scope
            .as_ref()
            .ok_or_else(|| refused("missing run Producer"))?;
        self.require_metadata_issuer(index, &transition.envelope().id.issuer)?;
        if self.attachments[index].role != AttachmentRole::Producer
            || scope.task.is_some()
            || scope.run != transition.projection().run
            || scope.expired()
            || scope.owner_epoch != self.owner()?.store()?.owner_epoch()
            || transition.envelope().authority.owner_epoch != scope.owner_epoch
            || self.recovering
            || transition.envelope().authority.grant_ref != self.expected_grant(index)?
        {
            return Err(refused("run Producer authority changed"));
        }
        if let Some(pending) = &self.hooks.pending {
            if !matches!(pending, PendingPublication::Owner(original) if &**original == transition)
            {
                return Err(refused(
                    "original uncertain owner transition must reconcile first",
                ));
            }
            self.owner_mut()?.reconcile_metadata()?;
        }
        self.hooks.pending = Some(PendingPublication::Owner(Box::new(transition.clone())));
        let result =
            orchestration_commit::commit_orchestration(self.owner_mut()?, transition.clone(), None);
        let receipt = match result {
            Ok(receipt) => {
                self.hooks.pending = None;
                receipt
            }
            Err(error) => {
                if !matches!(
                    error,
                    BundleError::DurabilityUnknown | BundleError::Unsettled
                ) {
                    self.hooks.pending = None;
                }
                return Err(error.into());
            }
        };
        let current = self
            .owner()?
            .orchestration_projection(&transition.projection().run)?
            .ok_or_else(|| refused("committed run disappeared"))?;
        self.sync_runtime_grants(&current);
        if let Some(run) = self.hooks.runs.get_mut(&current.run) {
            run.continuable =
                current.outcome == RunOutcome::Pending || current.cleanup != RunCleanup::Settled;
        }
        let admitted = reservation
            .filter(|identity| {
                current.attempts.iter().any(|attempt| {
                    attempt.identity == **identity
                        && attempt.state == AttemptState::Reserved
                        && attempt.owner_epoch == transition.envelope().authority.owner_epoch
                })
            })
            .map(|identity| {
                crate::orchestration::acknowledge_attempt_reservation(
                    transition, &receipt, identity,
                )
            })
            .transpose()
            .map_err(scheduler_error)?;
        Ok((receipt, admitted))
    }
    pub(super) fn revoke_task_endpoints(&mut self, removed: Option<&ProducerScope>) {
        for run in self.hooks.runs.values() {
            for task in run.tasks.values() {
                let endpoint = task.endpoint();
                if removed.is_some_and(|scope| {
                    scope.endpoint.as_ref() == Some(&endpoint.authority().endpoint)
                }) {
                    endpoint.revoke();
                }
            }
        }
    }
    pub fn prepare_task_endpoint(
        &mut self,
        producer: &HostAuth,
        run: &RunId,
        task: &TaskId,
        attempt: &AttemptId,
        ttl: Duration,
    ) -> Result<(HostAuth, TaskEndpoint), HostError> {
        // Retired task grants cannot consume attachment capacity across all 80 sends.
        if self.hooks.pending.is_none() {
            self.attachments.retain(|attachment| {
                attachment.alive.load(std::sync::atomic::Ordering::Acquire)
                    || attachment
                        .producer_scope
                        .as_ref()
                        .is_none_or(|scope| scope.task.is_none())
            });
        }
        let index = self.resolve_attachment(Some(&producer.bearer()))?;
        let root = self.attachments[index]
            .producer_scope
            .as_ref()
            .ok_or_else(|| refused("missing run Producer"))?;
        if self.attachments[index].role != AttachmentRole::Producer
            || &root.run != run
            || root.task.is_some()
            || root.expired()
            || root.owner_epoch != self.owner()?.store()?.owner_epoch()
            || self.recovering
            || self.switch_frozen
            || self.hooks.pending.is_some()
            || ttl.is_zero()
            || ttl > Duration::from_secs(3600)
        {
            return Err(refused(
                "task issuance requires the independent run Producer",
            ));
        }
        let state = self
            .owner()?
            .orchestration_projection(run)?
            .ok_or_else(|| refused("unknown run"))?;
        if state.outcome != RunOutcome::Pending
            || matches!(
                state.phase,
                RunPhase::Paused | RunPhase::Interrupted | RunPhase::Finished
            )
            || self.attachments.len() >= super::auth::ATTACHMENT_CAPACITY
        {
            return Err(refused("task issuance is paused or at capacity"));
        }
        let task_record = state
            .tasks
            .iter()
            .find(|record| &record.id == task)
            .ok_or_else(|| refused("unknown task"))?;
        let epoch = self.owner()?.store()?.owner_epoch();
        if !(RuntimeGrantBinding {
            run: run.clone(),
            task: task.clone(),
            attempt: attempt.clone(),
            owner_epoch: epoch,
        })
        .live_in(&state, Some(epoch))
        {
            return Err(refused("task attempt is no longer live"));
        }
        if !state.attempts.iter().any(|reservation| {
            &reservation.identity.task == task
                && &reservation.identity.attempt == attempt
                && reservation.owner_epoch == epoch
                && reservation.state == AttemptState::Reserved
        }) {
            return Err(refused("task has no acknowledged reservation"));
        }
        if let Some(existing) = self
            .hooks
            .runs
            .get(run)
            .and_then(|runtime| runtime.tasks.get(task))
        {
            let endpoint = existing.endpoint();
            let authority = endpoint.authority();
            if &authority.attempt == attempt {
                if endpoint.check_live().is_err() {
                    return Err(refused("task authority revoked"));
                }
                let attachment = self
                    .attachments
                    .iter()
                    .find(|attachment| {
                        attachment.producer_scope.as_ref().is_some_and(|scope| {
                            scope.endpoint.as_ref() == Some(&authority.endpoint)
                        })
                    })
                    .ok_or_else(|| refused("task endpoint attachment was revoked"))?;
                return Ok((attachment.auth.clone(), endpoint));
            }
            if !self.retired_task_authority(&state, task, &authority.attempt)? {
                return Err(refused("previous task authority is not retired"));
            }
        }
        let authority = RuntimeTaskAuthority {
            project: self.owner()?.active_manual_project()?,
            run: run.clone(),
            task: task.clone(),
            attempt: attempt.clone(),
            endpoint: EndpointId(kinewright_project::bundle_records::SessionId::fresh()?),
            owner_epoch: epoch,
            expires_at: Instant::now() + ttl,
        };
        let scope = ProducerScope {
            run: run.clone(),
            task: Some(task.clone()),
            attempt: Some(attempt.clone()),
            endpoint: Some(authority.endpoint.clone()),
            owner_epoch: epoch,
            expires_at: Some(authority.expires_at),
            scope: task_record.brief.scope.clone(),
        };
        let saved = self.saved_task_history(&state, task)?;
        let previous = self
            .hooks
            .runs
            .get_mut(run)
            .and_then(|r| r.tasks.remove(task));
        let editor_task = if let Some(previous) = previous {
            previous.continue_authority(self.owner()?, &task_record.brief, authority)?
        } else if let Some(history) = &saved {
            if history.attempt == *attempt
                || !self.retired_task_authority(&state, task, &history.attempt)?
            {
                return Err(refused("previous task authority is not retired"));
            }
            self.restore_task_candidate(history)?;
            let mut reopened = EditorTask::reopen(
                self.owner_mut()?,
                task_record.brief.clone(),
                authority,
                history.source.branch.clone(),
                &history.base_ref,
                history.merge_envelope.clone(),
            )?;
            if self.owner()?.candidate_binding(&history.source.branch)? != history.source {
                return Err(refused("task checkpoint changed"));
            }
            if history.frozen && reopened.freeze(self.owner_mut()?)?.source() != &history.source {
                return Err(refused("frozen task checkpoint changed"));
            }
            reopened
        } else {
            if state
                .attempts
                .iter()
                .any(|a| a.identity.task == *task && a.identity.attempt != *attempt)
                || self.retired_task_authority_exists(&state, task)?
            {
                return Err(refused("prior task history is unavailable"));
            }
            self.restore_initial_run_candidate(&state, &task_record.brief.scope.base.branch)?;
            EditorTask::prepare(self.owner_mut()?, task_record.brief.clone(), authority)?
        };
        let endpoint = editor_task.endpoint();
        self.hooks
            .runs
            .get_mut(run)
            .ok_or_else(|| refused("run missing"))?
            .tasks
            .insert(task.clone(), editor_task);
        let identity = state
            .attempts
            .iter()
            .find(|a| a.identity.task == *task && a.identity.attempt == *attempt)
            .ok_or_else(|| refused("reservation missing"))?
            .identity
            .clone();
        let mut state = state;
        self.persist_runtime_task(&mut state, &identity, false)?;
        self.publish_native(producer, state, &identity, vec![])?;
        let roots = self.attachments[index].roots.clone();
        let auth = self.issue_attachment(
            format!("task:{}", task.0.as_str()),
            AttachmentRole::Producer,
            roots,
        )?;
        let index = self.resolve_attachment(Some(&auth.bearer()))?;
        self.attachments[index].producer_scope = Some(scope);
        Ok((auth, endpoint))
    }
    pub fn start_task_server(
        &mut self,
        producer: &HostAuth,
        run: &RunId,
        task: &TaskId,
    ) -> Result<crate::McpServer, HostError> {
        let index = self.resolve_attachment(Some(&producer.bearer()))?;
        let scope = self.attachments[index]
            .producer_scope
            .as_ref()
            .ok_or_else(|| refused("missing run Producer"))?;
        if self.attachments[index].role != AttachmentRole::Producer
            || &scope.run != run
            || scope.task.is_some()
            || scope.expired()
        {
            return Err(refused("independent run Producer required"));
        }
        let task = self
            .hooks
            .runs
            .get(run)
            .and_then(|run| run.tasks.get(task))
            .ok_or_else(|| refused("task is not retained"))?;
        let endpoint = task.endpoint();
        endpoint.check_live().map_err(|error| refused(&error))?;
        let core = task.core().clone();
        #[cfg(target_os = "linux")]
        let measurements = {
            let authority = endpoint.authority();
            let retained = self
                .hooks
                .runs
                .get(run)
                .ok_or_else(|| refused("size observation run missing"))?;
            let previous = if retained.measurements.contains_key(&authority.task) {
                None
            } else {
                let state = self
                    .owner()?
                    .orchestration_projection(run)?
                    .ok_or_else(|| refused("size observation run missing"))?;
                self.native_accounting(&state)?
                    .and_then(|r| r.tasks.into_iter().find(|t| t.task == authority.task.0))
                    .and_then(|t| t.measurements)
            };
            self.hooks
                .runs
                .get_mut(run)
                .ok_or_else(|| refused("size observation run missing"))?
                .measurements
                .entry(authority.task.clone())
                .or_insert_with(|| Arc::new(Mutex::new(previous.unwrap_or_default())))
                .clone()
        };
        let media = std::sync::Arc::new(super::LazyHostMedia::default());
        crate::McpServer::start_task_isolated(
            core,
            media.clone(),
            media,
            &self.config.project,
            self.attachments[index].roots.clone(),
            endpoint.clone(),
            Some(super::task_mcp::TaskMcp::new(
                self.runtime_owner.clone(),
                self.attachments
                    .iter()
                    .find(|attachment| {
                        attachment.producer_scope.as_ref().is_some_and(|scope| {
                            scope.endpoint.as_ref() == Some(&endpoint.authority().endpoint)
                        })
                    })
                    .ok_or_else(|| refused("task attachment missing"))?
                    .auth
                    .clone(),
                endpoint,
                #[cfg(target_os = "linux")]
                measurements,
                #[cfg(all(target_os = "linux", any(test, feature = "test-util")))]
                self.qualification_stop.clone(),
            )),
        )
    }
}

fn scope_contains(parent: &TaskScope, child: &TaskScope) -> bool {
    child.base == parent.base
        && child.operations.is_subset(&parent.operations)
        && child.objects.is_subset(&parent.objects)
        && child.fields.is_subset(&parent.fields)
        && child.tracks.is_subset(&parent.tracks)
        && child
            .bus_sidechain_closure
            .is_subset(&parent.bus_sidechain_closure)
        && child
            .asset_roots
            .iter()
            .all(|reference| parent.asset_roots.contains(reference))
        && child
            .consent_refs
            .iter()
            .all(|reference| parent.consent_refs.contains(reference))
}
