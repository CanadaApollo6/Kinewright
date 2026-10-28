//! A person-facing holder can answer only a request freshly checked by the sole pump.
use super::{AttachmentRole, HostAuth, HostCommandOwner, HostError, HostRefusal};
use crate::orchestration::orchestration_records::{AttemptId, RunId, TaskId};
use crate::{
    LiveResponseOutcome, PromptPoll, SessionCounters, SessionFlow, SessionObserver, SessionStop,
    StudioResponseQueue, StudioResponseSender,
};
use kinewright_core::{
    AgentEvent, RuntimeRequest, RuntimeRequestBinding, RuntimeRequestKind, RuntimeResponse,
    RuntimeResponseControl,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum AttentionAnswer {
    Approval(bool),
    SecretReference(String),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveAttentionResponse {
    pub run: RunId,
    pub request_id: String,
    pub binding_digest: String,
    pub command: u64,
    pub answer: AttentionAnswer,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttentionPrompt {
    pub run: RunId,
    pub request_id: String,
    pub binding_digest: String,
    pub kind: String,
    pub text: String,
    pub request_ref: kinewright_project::bundle_records::ObjectRef,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionOutcome {
    Queued,
}
struct PendingAttention {
    request: RuntimeRequest,
    digest: String,
    expires: Instant,
    sender: StudioResponseSender,
    submitted: Option<(super::auth::AttachmentId, LiveAttentionResponse)>,
}
impl PendingAttention {
    fn facts(&self, run: &RunId) -> Vec<u8> {
        serde_json::to_vec(&(run, self.request.id(), &self.digest, self.request.text()))
            .expect("bounded nonsecret attention facts")
    }
    fn reference(&self, run: &RunId) -> kinewright_project::bundle_records::ObjectRef {
        let bytes = self.facts(run);
        kinewright_project::bundle_records::ObjectRef {
            sha256: kinewright_media::sha256_bytes(&bytes),
            byte_length: bytes.len() as u64,
            record_kind: kinewright_project::bundle_records::RecordKind::OwnedEvidence,
        }
    }
}
#[derive(Default)]
pub(super) struct AttentionState {
    current: BTreeMap<(RunId, String), RuntimeRequestBinding>,
    requests: BTreeMap<(RunId, String), PendingAttention>,
    consumed: BTreeMap<(RunId, String), RuntimeRequestBinding>,
}
pub struct HostRunObserver {
    run: RunId,
    task: TaskId,
    attempt: AttemptId,
    epoch: u64,
    state: Arc<Mutex<AttentionState>>,
    sender: StudioResponseSender,
    queue: StudioResponseQueue,
    delegate: Box<dyn SessionObserver + Send>,
}
fn digest(request: &RuntimeRequest) -> String {
    let b = request.binding();
    kinewright_media::sha256_bytes(
        format!(
            "{}\0{}\0{}\0{}\0{}\0{}\0{:?}\0{}",
            request.id(),
            b.task,
            b.attempt,
            b.session,
            b.turn,
            b.owner_epoch,
            request.kind(),
            request.text()
        )
        .as_bytes(),
    )
}
impl SessionObserver for HostRunObserver {
    fn supports_live_attention(&self) -> bool {
        true
    }
    fn on_event(&mut self, event: &AgentEvent) -> SessionFlow {
        self.delegate.on_event(event)
    }
    fn on_tick(&mut self, counters: &SessionCounters) -> SessionFlow {
        self.delegate.on_tick(counters)
    }
    fn poll_prompt(&mut self) -> PromptPoll {
        self.delegate.poll_prompt()
    }
    fn prepare_studio_stop(
        &mut self,
        stop: &SessionStop,
        binding: Option<&RuntimeRequestBinding>,
    ) -> Result<(), String> {
        self.delegate.prepare_studio_stop(stop, binding)
    }
    fn on_confirmation_error(&mut self, message: String) {
        self.delegate.on_confirmation_error(message);
    }
    fn on_runtime_binding(&mut self, binding: Option<&RuntimeRequestBinding>) {
        if let Ok(mut state) = self.state.lock() {
            let key = (self.run.clone(), self.task.0.as_str().to_owned());
            let binding = binding.filter(|b| {
                b.task == self.task.0.as_str()
                    && b.attempt == self.attempt.0.as_str()
                    && b.owner_epoch == self.epoch
            });
            state.requests.retain(|(run, _), pending| {
                run != &self.run
                    || pending.request.binding().task != self.task.0.as_str()
                    || binding == Some(pending.request.binding())
            });
            state.consumed.retain(|(run, _), prior| {
                run != &self.run || prior.task != self.task.0.as_str() || binding == Some(prior)
            });
            if let Some(binding) = binding {
                state.current.insert(key, binding.clone());
            } else {
                state.current.remove(&key);
            }
        }
        self.delegate.on_runtime_binding(binding);
    }
    fn on_runtime_request(&mut self, request: &RuntimeRequest) -> RuntimeResponse {
        if request.kind() == RuntimeRequestKind::Question {
            return self.delegate.on_runtime_request(request);
        }
        let Ok(mut state) = self.state.lock() else {
            return RuntimeResponse::H3AttentionUnavailable;
        };
        if state
            .consumed
            .get(&(self.run.clone(), request.id().to_owned()))
            == Some(request.binding())
        {
            return RuntimeResponse::Pending;
        }
        if state
            .current
            .get(&(self.run.clone(), request.binding().task.clone()))
            != Some(request.binding())
            || state.requests.len() + state.consumed.len() >= 64
        {
            return RuntimeResponse::Expired;
        }
        state
            .requests
            .entry((self.run.clone(), request.id().to_owned()))
            .or_insert_with(|| PendingAttention {
                request: request.clone(),
                digest: digest(request),
                expires: Instant::now() + Duration::from_secs(300),
                sender: self.sender.clone(),
                submitted: None,
            });
        RuntimeResponse::Pending
    }
    fn poll_runtime_response(&mut self) -> Option<RuntimeResponseControl> {
        self.queue.poll_response().or_else(|| {
            self.delegate
                .poll_runtime_response()
                .filter(|control| control.request().kind() == RuntimeRequestKind::Question)
        })
    }
    fn on_runtime_response_result(
        &mut self,
        request: &RuntimeRequest,
        outcome: LiveResponseOutcome,
    ) {
        if let Ok(mut state) = self.state.lock() {
            let key = (self.run.clone(), request.id().to_owned());
            if outcome == LiveResponseOutcome::Revoked {
                if let Some(pending) = state.requests.get_mut(&key) {
                    pending.submitted = None;
                }
            } else if let Some(pending) = state.requests.remove(&key) {
                state
                    .consumed
                    .insert(key, pending.request.binding().clone());
            }
        }
        self.delegate.on_runtime_response_result(request, outcome);
    }
}
impl Drop for HostRunObserver {
    fn drop(&mut self) {
        self.on_runtime_binding(None);
    }
}
impl HostCommandOwner {
    pub(super) fn live_attention_admission(
        &self,
        request: &super::ScopedOrchestrationRequest,
    ) -> Result<
        Option<(
            crate::orchestration::requests::LiveAttentionAdmission,
            Vec<u8>,
        )>,
        HostError,
    > {
        use crate::orchestration::orchestration_records::{
            OrchestrationCommand, RuntimeRequest as RecordedRequest,
        };
        let OrchestrationCommand::Request(
            recorded @ (RecordedRequest::Approval(attention) | RecordedRequest::Secret(attention)),
        ) = &request.envelope.command
        else {
            return Ok(None);
        };
        let run = request
            .run
            .as_ref()
            .ok_or_else(|| HostRefusal::new("attention_expired", "run scope required"))?;
        let state = self.hooks.attention.lock().map_err(|_| {
            HostRefusal::new("attention_expired", "live request registry unavailable")
        })?;
        let pending = state
            .requests
            .iter()
            .find(|((id, _), pending)| {
                id == run
                    && pending.reference(run) == attention.request_ref
                    && pending.request.binding().task == attention.task.0.as_str()
                    && pending.request.binding().attempt == attention.attempt.0.as_str()
            })
            .map(|(_, pending)| pending)
            .ok_or_else(|| {
                HostRefusal::new(
                    "attention_expired",
                    "the request has no verified live callback",
                )
            })?;
        if Instant::now() >= pending.expires
            || state
                .current
                .get(&(run.clone(), pending.request.binding().task.clone()))
                != Some(pending.request.binding())
            || matches!(recorded, RecordedRequest::Approval(_))
                != (pending.request.kind() == RuntimeRequestKind::Approval)
        {
            return Err(
                HostRefusal::new("attention_expired", "the request callback changed").into(),
            );
        }
        Ok(Some((
            crate::orchestration::requests::LiveAttentionAdmission::from_host(recorded.clone()),
            pending.facts(run),
        )))
    }
    pub fn run_observer(
        &self,
        producer: &HostAuth,
        run: &RunId,
        task: &TaskId,
        attempt: &AttemptId,
        delegate: Box<dyn SessionObserver + Send>,
    ) -> Result<HostRunObserver, HostError> {
        let index = self.resolve_attachment(Some(&producer.bearer()))?;
        let scope = self.attachments[index]
            .producer_scope
            .as_ref()
            .ok_or_else(|| HostRefusal::new("role_refused", "independent run Producer required"))?;
        let state = self
            .owner()?
            .orchestration_projection(run)?
            .ok_or_else(|| HostRefusal::new("orchestration_refused", "unknown run"))?;
        if self.attachments[index].role != AttachmentRole::Producer
            || &scope.run != run
            || scope.task.is_some()
            || state.outcome != crate::orchestration::orchestration_records::RunOutcome::Pending
            || scope.expired()
            || !state.attempts.iter().any(|reservation| {
                &reservation.identity.task == task
                    && &reservation.identity.attempt == attempt
                    && reservation.owner_epoch == scope.owner_epoch
                    && !matches!(
                        reservation.state,
                        crate::orchestration::orchestration_records::AttemptState::Retired(_)
                    )
            })
        {
            return Err(HostRefusal::new("role_refused", "live run attempt required").into());
        }
        let (sender, queue) = crate::studio_response_queue();
        Ok(HostRunObserver {
            run: run.clone(),
            task: task.clone(),
            attempt: attempt.clone(),
            epoch: scope.owner_epoch,
            state: Arc::clone(&self.hooks.attention),
            sender,
            queue,
            delegate,
        })
    }
    pub(super) fn attention_prompts(&self, run: &RunId) -> Vec<AttentionPrompt> {
        self.hooks.attention.lock().map_or_else(
            |_| vec![],
            |state| {
                state
                    .requests
                    .iter()
                    .filter(|((id, _), pending)| id == run && Instant::now() < pending.expires)
                    .map(|((id, request), pending)| AttentionPrompt {
                        run: id.clone(),
                        request_id: request.clone(),
                        binding_digest: pending.digest.clone(),
                        kind: format!("{:?}", pending.request.kind()).to_ascii_lowercase(),
                        text: pending.request.text().to_owned(),
                        request_ref: pending.reference(id),
                    })
                    .collect()
            },
        )
    }
    pub(super) fn resolve_attention(
        &mut self,
        index: usize,
        response: LiveAttentionResponse,
    ) -> Result<AttentionOutcome, HostError> {
        let attachment = &self.attachments[index];
        if attachment.role != AttachmentRole::Editor || !attachment.person_facing {
            return Err(HostRefusal::new(
                "attention_holder",
                "the person-facing Editor holder is required",
            )
            .into());
        }
        let projection = self
            .owner()?
            .orchestration_projection(&response.run)?
            .ok_or_else(|| HostRefusal::new("attention_expired", "unknown run"))?;
        let mut state = self.hooks.attention.lock().map_err(|_| {
            HostRefusal::new("attention_expired", "live request registry unavailable")
        })?;
        let key = (response.run.clone(), response.request_id.clone());
        let pending = state
            .requests
            .get(&key)
            .ok_or_else(|| HostRefusal::new("attention_expired", "no live request"))?;
        let binding = pending.request.binding();
        if response.command == 0
            || response.binding_digest != pending.digest
            || Instant::now() >= pending.expires
            || state
                .current
                .get(&(response.run.clone(), binding.task.clone()))
                != Some(binding)
            || !projection.tasks.iter().any(|task| {
                task.id.0.as_str() == binding.task
                    && !task.progress.expired
                    && task.progress.turn.as_ref().is_some_and(|turn| {
                        crate::orchestration::requests::live_turn(&projection, turn).is_ok()
                            && turn.session.as_str() == binding.session
                            && turn.ordinal == binding.turn
                            && turn.owner_epoch == binding.owner_epoch
                            && turn.attempt.0.as_str() == binding.attempt
                    })
            })
        {
            return Err(HostRefusal::new(
                "attention_expired",
                "the exact live request must be verified again",
            )
            .into());
        }
        let runtime = match &response.answer {
            AttentionAnswer::Approval(approved) => RuntimeResponse::ApprovalDecision(*approved),
            AttentionAnswer::SecretReference(reference) => {
                RuntimeResponse::SecretReference(reference.clone())
            }
        };
        let control = RuntimeResponseControl::new(pending.request.clone(), runtime)
            .map_err(|_| {
                HostRefusal::new(
                    "attention_kind",
                    "ordinary answers cannot approve or provide a secret",
                )
            })?
            .with_live_authority(Arc::clone(&attachment.alive));
        if let Some((holder, previous)) = &pending.submitted {
            if holder != &attachment.id || previous != &response {
                return Err(HostRefusal::new(
                    "attention_changed",
                    "an answer is already queued for this request",
                )
                .into());
            }
            return Ok(AttentionOutcome::Queued);
        }
        pending.sender.try_send(control).map_err(|_| {
            HostRefusal::new("attention_capacity", "live response queue is unavailable")
        })?;
        state
            .requests
            .get_mut(&key)
            .ok_or_else(|| HostRefusal::new("attention_expired", "request disappeared"))?
            .submitted = Some((attachment.id, response));
        Ok(AttentionOutcome::Queued)
    }
}
