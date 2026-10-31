//! The seven MCP tools carry controls bound to one H3 task attachment.
pub(crate) mod schema;
use super::{HostAuth, HostCommandOwner, HostError, HostRefusal};
use crate::orchestration::{
    orchestration_commit::CommandDecision,
    orchestration_records::{
        AuthorityBinding, OrchestrationCommand, OrchestrationEnvelope, RunId, RunProjection, TaskId,
    },
    tasks::TaskEndpoint,
};
use kinewright_project::{
    bundle_records::HeadId,
    receipts::{CommandId, IssuerId},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, Weak};

pub(crate) const CAPABILITIES: &[(&str, &str)] = &[
    (
        "undo_task_edit",
        "Undo a scoped edit while retaining the exact redo history.",
    ),
    (
        "redo_task_edit",
        "Redo a scoped edit from the retained history.",
    ),
    (
        "assign_task",
        "Assign a scoped child and reserve dependency capacity before waiting.",
    ),
    (
        "freeze_task",
        "Drain a task and durably freeze its exact checkpoint once.",
    ),
    (
        "publish_result",
        "Publish the exact frozen task result and its parent delivery.",
    ),
    (
        "merge_task",
        "Merge a frozen child into its exact candidate once, retaining the original receipt.",
    ),
    (
        "observe_delivery",
        "Acknowledge a delivery offered to this exact live Director turn.",
    ),
    (
        "cancel_task",
        "Cancel this task or an authorized child, retaining results until cleanup.",
    ),
    (
        "answer_question",
        "Answer a pending question as the live Director.",
    ),
    (
        "task_status",
        "Read durable run status and the next control envelope identity.",
    ),
    (
        "wait_task",
        "Read status after a sequence without blocking the host; unchanged status means waiting.",
    ),
    (
        "send_message",
        "Queue a Director message through durable send accounting, never into an active turn.",
    ),
];
pub(crate) fn is_capability(name: &str) -> bool {
    CAPABILITIES.iter().any(|(candidate, _)| *candidate == name)
}

#[derive(Clone)]
pub(crate) struct TaskMcp {
    host: Weak<Mutex<HostCommandOwner>>,
    auth: HostAuth,
    endpoint: TaskEndpoint,
    issuer: Arc<Mutex<Option<IssuerId>>>,
    #[cfg(all(target_os = "linux", any(test, feature = "test-util")))]
    stop_gate: Option<Arc<super::qualification_gate::MergeStopGate>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Control {
    id: CommandId,
    expected_generation: u64,
    owner_epoch: u64,
    expected_head: HeadId,
    command: OrchestrationCommand,
}
fn refused(reason: &str) -> HostError {
    HostRefusal::new("task_scope", reason).into()
}
impl TaskMcp {
    pub(super) fn new(
        host: Weak<Mutex<HostCommandOwner>>,
        auth: HostAuth,
        endpoint: TaskEndpoint,
        #[cfg(all(target_os = "linux", any(test, feature = "test-util")))] stop_gate: Option<
            Arc<super::qualification_gate::MergeStopGate>,
        >,
    ) -> Self {
        Self {
            host,
            auth,
            endpoint,
            issuer: Arc::default(),
            #[cfg(all(target_os = "linux", any(test, feature = "test-util")))]
            stop_gate,
        }
    }
    pub(crate) fn call(&self, name: &str, arguments: Value) -> Result<Value, HostError> {
        #[cfg(all(target_os = "linux", any(test, feature = "test-util")))]
        if let Some(gate) = &self.stop_gate {
            gate.before_request(&self.endpoint.authority().task)
                .map_err(refused)?;
        }
        let reply = self.call_inner(name, arguments)?;
        #[cfg(all(target_os = "linux", any(test, feature = "test-util")))]
        if name == "merge_task"
            && let Some(gate) = &self.stop_gate
        {
            gate.after_merge(self.endpoint.authority().task.clone());
        }
        Ok(reply)
    }
    fn call_inner(&self, name: &str, arguments: Value) -> Result<Value, HostError> {
        let host = self.host.upgrade().ok_or_else(|| refused("host closed"))?;
        let mut issuer = self
            .issuer
            .lock()
            .map_err(|_| refused("control issuer unavailable"))?;
        let mut host = host.lock().map_err(|_| refused("host owner unavailable"))?;
        host.prove_retention()?;
        let (index, state, task) = host.resolve_task_mcp(&self.auth, &self.endpoint)?;
        if matches!(name, "task_status" | "wait_task") {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Observe {
                #[serde(default)]
                after_sequence: u64,
            }
            let observe: Observe = serde_json::from_value(arguments)?;
            let metadata = host.issue_metadata(index, issuer.clone())?;
            *issuer = Some(metadata.envelope.id.issuer.clone());
            return Ok(
                json!({"run": host.observe_run(index, &state.run, observe.after_sequence)?,
                "control": {"id": metadata.envelope.id, "expected_generation": metadata.root_generation,
                    "owner_epoch": metadata.envelope.owner_epoch, "expected_head": self.endpoint.head()},
                "waiting": state.sequence <= observe.after_sequence}),
            );
        }
        if matches!(name, "undo_task_edit" | "redo_task_edit") {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Step {
                expected_revision: kinewright_core::TimelineRevision,
            }
            let step: Step = serde_json::from_value(arguments)?;
            let core = host.hooks.runs[&state.run].tasks[&task].core();
            let history = self
                .endpoint
                .history_step(core, step.expected_revision, name == "redo_task_edit")
                .map_err(|_| refused("history step is outside task authority"))?;
            return Ok(json!({"history":history}));
        }
        let control: Control = serde_json::from_value(arguments)?;
        if control.owner_epoch != self.endpoint.authority().owner_epoch
            || control.expected_head != *self.endpoint.head()
        {
            return Err(refused("control epoch or candidate head changed"));
        }
        let command_name = match &control.command {
            OrchestrationCommand::AssignTask(_) => "assign_task",
            OrchestrationCommand::FreezeTask(_) => "freeze_task",
            OrchestrationCommand::PublishResult(_) => "publish_result",
            OrchestrationCommand::MergeTask(_) => "merge_task",
            OrchestrationCommand::ObserveDelivery(_) => "observe_delivery",
            OrchestrationCommand::CancelTask(_) => "cancel_task",
            OrchestrationCommand::AnswerQuestion(_) => "answer_question",
            OrchestrationCommand::ControlRun(intent)
                if matches!(
                    intent.action,
                    crate::orchestration::orchestration_records::RunAction::SendMessage { .. }
                ) =>
            {
                "send_message"
            }
            _ => return Err(refused("host-only transition")),
        };
        if name != command_name {
            return Err(refused("capability/command mismatch"));
        }
        let mut envelope = OrchestrationEnvelope {
            id: control.id,
            digest: serde_json::from_value(json!("0".repeat(64)))?,
            project: self.endpoint.authority().project.clone(),
            expected_generation: control.expected_generation,
            authority: AuthorityBinding {
                grant_ref: host.expected_grant(index)?,
                owner_epoch: control.owner_epoch,
                endpoint: Some(self.endpoint.authority().endpoint.clone()),
                attempt: Some(self.endpoint.authority().attempt.clone()),
            },
            command: control.command,
        };
        envelope.digest = envelope
            .computed_digest()
            .map_err(|_| refused("invalid control"))?;
        let request = super::orchestration::ScopedOrchestrationRequest {
            run: Some(state.run.clone()),
            envelope,
            consume_batch: None,
        };
        // Authorize before receipt lookup or any freeze/merge side effect.
        host.authorize_orchestration(index, &request)?;
        if let OrchestrationCommand::FreezeTask(target) = &request.envelope.command
            && let Some(history) = host.saved_task_history(&state, target)?
            && history.frozen
        {
            let receipt = host
                .owner()?
                .orchestration_replay(&request.envelope, None)?;
            return host.frozen_mcp_result(&state.run, target, receipt.as_ref());
        }
        if let OrchestrationCommand::PublishResult(publish) = &request.envelope.command
            && let Some(reference) = state
                .tasks
                .iter()
                .find(|task| task.id == publish.task)
                .and_then(|task| {
                    if let crate::orchestration::orchestration_records::TaskState::Terminal(
                        reference,
                    ) = &task.state
                    {
                        Some(reference)
                    } else {
                        None
                    }
                })
        {
            let record = host
                .owner()?
                .store()?
                .read_record::<crate::orchestration::orchestration_records::OrchestrationRecord>(
                reference,
            )?;
            if record.payload
                != crate::orchestration::orchestration_records::OrchestrationPayload::Result(
                    Box::new(publish.result.clone()),
                )
            {
                return Err(refused("published result changed"));
            }
            return Ok(
                json!({"already_published":true,"result":reference,"receipt":host.owner()?.orchestration_replay(&request.envelope,None)?}),
            );
        }
        if let OrchestrationCommand::MergeTask(merge) = &request.envelope.command {
            host.require_metadata_issuer(index, &request.envelope.id.issuer)?;
            let history = host
                .saved_task_history(&state, &merge.task)?
                .ok_or_else(|| refused("task history missing"))?;
            let result = host
                .owner()?
                .store()?
                .read_record::<crate::orchestration::orchestration_records::OrchestrationRecord>(
                &merge.result,
            )?;
            let crate::orchestration::orchestration_records::OrchestrationPayload::Result(result) =
                &result.payload
            else {
                return Err(refused("not a task result"));
            };
            if !state.results.contains(&merge.result)
                || merge.target != history.base
                || result.head.as_ref() != Some(&history.source)
                || !history.frozen
                || (history.merge_envelope.is_none()
                    && control.expected_generation != host.owner()?.metadata_generation()?)
            {
                return Err(refused("merge target/result changed"));
            }
            host.restore_frozen_task(&state, &merge.task)?;
            let producer = host.run_producer(&state.run)?;
            let receipt = host.merge_task_history(
                &producer,
                &state.run,
                &merge.task,
                request.envelope.id.clone(),
            )?;
            let (kinewright_project::receipts::ReceiptLookup::Committed(receipt)
            | kinewright_project::receipts::ReceiptLookup::Retired(receipt)) = receipt
            else {
                return Err(refused("merge outcome is not settled"));
            };
            return Ok(json!({"receipt": receipt}));
        }
        let freeze = if let OrchestrationCommand::FreezeTask(task) = &request.envelope.command {
            Some(task.clone())
        } else {
            None
        };
        let receipt = host.dispatch_orchestration(index, &request)?;
        if receipt.decision == CommandDecision::Applied
            && let Some(task) = freeze
        {
            return host.frozen_mcp_result(&state.run, &task, Some(&receipt));
        }
        let _ = task;
        Ok(json!({"receipt":receipt}))
    }
}
impl HostCommandOwner {
    pub(super) fn current_accounting_reference(
        &self,
        state: &RunProjection,
    ) -> Result<kinewright_project::bundle_records::ObjectRef, HostError> {
        for reference in state.usage.iter().rev() {
            let record = self
                .owner()?
                .store()?
                .read_record::<crate::orchestration::orchestration_records::OrchestrationRecord>(
                reference,
            )?;
            if matches!(
                record.payload,
                crate::orchestration::orchestration_records::OrchestrationPayload::Accounting(_)
            ) {
                return Ok(reference.clone());
            }
        }
        Err(refused("accounting missing"))
    }
    fn resolve_task_mcp(
        &self,
        auth: &HostAuth,
        endpoint: &TaskEndpoint,
    ) -> Result<(usize, RunProjection, TaskId), HostError> {
        endpoint
            .check_live()
            .map_err(|_| refused("task authority expired"))?;
        let index = self.resolve_attachment(Some(&auth.bearer()))?;
        let authority = endpoint.authority();
        let scope = self.attachments[index]
            .producer_scope
            .as_ref()
            .ok_or_else(|| refused("missing task scope"))?;
        if self.attachments[index].role != super::AttachmentRole::Producer
            || scope.run != authority.run
            || scope.task.as_ref() != Some(&authority.task)
            || scope.attempt.as_ref() != Some(&authority.attempt)
            || scope.endpoint.as_ref() != Some(&authority.endpoint)
            || scope.owner_epoch != authority.owner_epoch
            || scope.expired()
            || scope.owner_epoch != self.owner()?.store()?.owner_epoch()
            || authority.project != self.owner()?.active_manual_project()?
            || self.recovering
            || self.switch_frozen
        {
            return Err(refused("task attachment changed"));
        }
        for pin in &self.pins {
            if !self.attachments[index].roots.permits(&pin.anchor, true) {
                return Err(refused("task roots changed"));
            }
        }
        let state = self
            .owner()?
            .orchestration_projection(&scope.run)?
            .ok_or_else(|| refused("run missing"))?;
        let record = state
            .tasks
            .iter()
            .find(|t| t.id == authority.task)
            .ok_or_else(|| refused("task missing"))?;
        if !endpoint.matches_brief(&record.brief)
            || !(super::orchestration::RuntimeGrantBinding {
                run: authority.run.clone(),
                task: authority.task.clone(),
                attempt: authority.attempt.clone(),
                owner_epoch: authority.owner_epoch,
            })
            .live_in(&state, Some(scope.owner_epoch))
        {
            return Err(refused("task no longer live"));
        }
        Ok((index, state, authority.task.clone()))
    }
}

impl HostCommandOwner {
    pub(super) fn stage_control_freeze(
        &mut self,
        update: crate::orchestration::orchestration_commit::ValidatedRunUpdate,
    ) -> Result<crate::orchestration::orchestration_commit::ValidatedRunUpdate, HostError> {
        if update.receipt().decision != CommandDecision::Applied {
            return Ok(update);
        }
        let OrchestrationCommand::FreezeTask(target) = &update.envelope().command else {
            return Ok(update);
        };
        let mut runtime = self
            .hooks
            .runs
            .get_mut(&update.projection().run)
            .and_then(|run| run.tasks.remove(target))
            .ok_or_else(|| refused("freeze actor missing"))?;
        let identity = update
            .projection()
            .attempts
            .iter()
            .find(|attempt| {
                attempt.identity.task == *target
                    && attempt.identity.attempt == runtime.endpoint().authority().attempt
            })
            .ok_or_else(|| refused("freeze attempt missing"))?
            .identity
            .clone();
        let frozen = runtime
            .freeze(self.owner_mut()?)
            .map(|_| ())
            .map_err(HostError::from);
        self.hooks
            .runs
            .get_mut(&update.projection().run)
            .ok_or_else(|| refused("run missing"))?
            .tasks
            .insert(target.clone(), runtime);
        frozen?;
        let mut state = update.projection().clone();
        self.persist_runtime_task(&mut state, &identity, true)?;
        let mut segment = update.segment().clone();
        segment
            .events
            .last_mut()
            .ok_or_else(|| refused("freeze event missing"))?
            .projection = state.clone();
        crate::orchestration::orchestration_commit::ValidatedRunUpdate::new(
            &update.envelope().project,
            update.envelope().expected_generation,
            update.envelope(),
            state,
            segment,
            update.receipt().decision.clone(),
        )
        .map_err(|_| refused("invalid frozen transition"))
    }
    fn frozen_mcp_result(
        &mut self,
        run: &RunId,
        task: &TaskId,
        receipt: Option<&crate::orchestration::orchestration_commit::OrchestrationReceipt>,
    ) -> Result<Value, HostError> {
        let state = self
            .owner()?
            .orchestration_projection(run)?
            .ok_or_else(|| refused("run missing"))?;
        self.restore_frozen_task(&state, task)?;
        let history = self
            .saved_task_history(&state, task)?
            .ok_or_else(|| refused("frozen history missing"))?;
        let delta = self.hooks.runs[run].tasks[task]
            .frozen_delta()
            .ok_or_else(|| refused("frozen delta missing"))?
            .to_vec();
        let delta_ref = self
            .owner_mut()?
            .admit_host_grant(&serde_json::to_vec(&delta)?)?;
        let result = crate::orchestration::orchestration_records::TaskResult {
            outcome: crate::orchestration::orchestration_records::AttemptOutcome::Completed,
            base: Some(history.base.clone()),
            head: Some(history.source.clone()),
            delta_ref: Some(delta_ref),
            artifacts: vec![],
            evidence: vec![],
            usage_ref: self.current_accounting_reference(&state)?,
            explanation: self.owner_mut()?.admit_host_grant(b"task result")?,
        };
        Ok(json!({"receipt":receipt,"history":history,"result":result}))
    }
}
