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
    pub full_refresh: bool,
    pub allowed_actions: Vec<RunActionKind>,
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
    pub producer: HostAuth,
    pub inputs: super::continuation::HostOwnedInputClosure,
    pub tasks: BTreeMap<TaskId, EditorTask>,
    pub continuable: bool,
}
#[derive(Default)]
pub(super) struct Hooks {
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
}

fn refused(reason: &str) -> HostError {
    HostRefusal::new("orchestration_refused", reason).into()
}
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
        HostCommand::IssueMetadataIssuer { .. }
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
            if !binding.live_in(projection, epoch) {
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
        issuer: Option<IssuerId>,
    ) -> Result<MetadataIssuer, HostError> {
        if self.hooks.pending.is_some() {
            return Err(BundleError::Unsettled.into());
        }
        let issuer = if let Some(issuer) = issuer {
            self.require_metadata_issuer(index, &issuer)?;
            issuer
        } else {
            let subject = self.metadata_subject(index);
            self.owner_mut()?.issue_metadata_issuer_for(&subject)?
        };
        let sequence = self.owner()?.metadata_issuer_sequence(&issuer)?;
        let envelope = self
            .owner()?
            .metadata_envelope(CommandId { issuer, sequence })?;
        let grant = serde_json::to_vec(&(
            &self.instance,
            &self.attachments[index].subject,
            self.attachments[index].role,
            &self.attachments[index].roots,
        ))?;
        let grant_ref = self.owner_mut()?.admit_host_grant(&grant)?;
        Ok(MetadataIssuer {
            envelope,
            root_generation: self.owner()?.metadata_generation()?,
            grant_ref,
        })
    }
    fn expected_grant(&self, index: usize) -> Result<ObjectRef, HostError> {
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
            HostCommand::ObserveRun { .. } | HostCommand::DetachForContinuation
        );
        let result = (|| {
            if !read {
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
                HostCommand::IssueMetadataIssuer { issuer } => {
                    reply.metadata_issuer = Some(self.issue_metadata(index, issuer)?);
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
    fn authorize_orchestration(
        &self,
        index: usize,
        request: &ScopedOrchestrationRequest,
    ) -> Result<(), HostError> {
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
        let allowed = match &request.envelope.command {
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
            OrchestrationCommand::RecordContext { task, .. }
            | OrchestrationCommand::FreezeTask(task) => own_task(task),
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
        Ok(())
    }
    pub(super) fn dispatch_orchestration(
        &mut self,
        index: usize,
        request: &ScopedOrchestrationRequest,
    ) -> Result<OrchestrationReceipt, HostError> {
        if let Some(pending) = &self.hooks.pending {
            // An immutable attachment instance admitted this exact command before B4.
            // Scope records are unreadable while the namespace is uncertain; only that
            // still-live instance may reconcile, then all current policy runs again.
            if !matches!(pending, PendingPublication::Wire(original, actor) if &**original == request && *actor == self.attachments[index].id)
            {
                return Err(refused("original uncertain command must reconcile first"));
            }
            self.owner_mut()?.reconcile_metadata()?;
        }
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
            return Ok(receipt);
        }
        if request.envelope.authority.owner_epoch != self.owner()?.store()?.owner_epoch()
            || request.envelope.authority.grant_ref != self.expected_grant(index)?
        {
            return Err(refused("fresh epoch or attachment grant changed"));
        }
        let owner = self.owner()?;
        let previous = request
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
        let generation = self.owner()?.metadata_generation()?;
        let attention = self.live_attention_admission(request)?;
        let transition = machine::prepare_with_attention(
            previous.as_ref(),
            &self.owner()?.active_manual_project()?,
            generation,
            &request.envelope,
            previous_segment,
            attention.as_ref().map(|(proof, _)| proof),
        )
        .map_err(scheduler_error)?;
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
            Box::new(request.clone()),
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
        let allowed_actions = if self.attachments[index].role == AttachmentRole::Editor {
            [
                RunAction::Pause,
                RunAction::Stop,
                RunAction::Continue,
                RunAction::SendMessage {
                    text_ref: projection.tasks[0].brief.assignment.clone(),
                },
            ]
            .into_iter()
            .filter(|action| machine::eligible(&projection, action))
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
        Ok(RunViewPage {
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
        let index = self.resolve_attachment(Some(&producer.bearer()))?;
        let root = self.attachments[index]
            .producer_scope
            .as_ref()
            .ok_or_else(|| refused("missing run Producer"))?;
        if self.attachments[index].role != AttachmentRole::Producer
            || &root.run != run
            || root.task.is_some()
            || root.expired()
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
            if &authority.attempt != attempt || endpoint.check_live().is_err() {
                return Err(refused(
                    "task endpoint already issued for another or revoked attempt",
                ));
            }
            let attachment =
                self.attachments
                    .iter()
                    .find(|attachment| {
                        attachment.producer_scope.as_ref().is_some_and(|scope| {
                            scope.endpoint.as_ref() == Some(&authority.endpoint)
                        })
                    })
                    .ok_or_else(|| refused("task endpoint attachment was revoked"))?;
            return Ok((attachment.auth.clone(), endpoint));
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
        let editor_task =
            EditorTask::prepare(self.owner_mut()?, task_record.brief.clone(), authority)?;
        let endpoint = editor_task.endpoint();
        let roots = self.attachments[index].roots.clone();
        let auth = self.issue_attachment(
            format!("task:{}", task.0.as_str()),
            AttachmentRole::Producer,
            roots,
        )?;
        let index = self.resolve_attachment(Some(&auth.bearer()))?;
        self.attachments[index].producer_scope = Some(scope);
        self.hooks
            .runs
            .get_mut(run)
            .ok_or_else(|| refused("run retention missing"))?
            .tasks
            .insert(task.clone(), editor_task);
        Ok((auth, endpoint))
    }
    pub fn start_task_server(
        &self,
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
        let media = std::sync::Arc::new(super::LazyHostMedia::default());
        crate::McpServer::start_task_isolated(
            task.core().clone(),
            media.clone(),
            media,
            &self.config.project,
            self.attachments[index].roots.clone(),
            endpoint,
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
