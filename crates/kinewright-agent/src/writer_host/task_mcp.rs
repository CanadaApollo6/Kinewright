//! Versioned task tools use durable roles and separately refreshed live controls.
pub(crate) mod manifest;
mod reply;
pub(crate) mod schema;
use super::{HostAuth, HostCommandOwner, HostError, HostRefusal};
use crate::orchestration::{
    orchestration_commit::CommandDecision,
    orchestration_records::{
        AuthorityBinding, OrchestrationCommand, OrchestrationEnvelope, Role, RunId, RunProjection,
        TaskId,
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
        "finish_task",
        "Freeze and publish the current revision with one durable completion identity. Retry the original id and payload. Frozen-only work is pending; completion also requires any parent delivery to be observed.",
    ),
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
    terminal_state: Arc<Mutex<Option<Value>>>,
    #[cfg(target_os = "linux")]
    measurements: Arc<Mutex<kinewright_project::usage_measurement::Measurements>>,
    #[cfg(all(target_os = "linux", any(test, feature = "test-util")))]
    stop_gate: Option<Arc<super::qualification_gate::MergeStopGate>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Control {
    id: CommandId,
    attempt: crate::orchestration::orchestration_records::AttemptId,
    expected_generation: u64,
    owner_epoch: u64,
    expected_head: HeadId,
    command: OrchestrationCommand,
}
fn refused(reason: &str) -> HostError {
    HostRefusal::new("task_scope", reason).into()
}

/// C5 fix 10: the product's own name of an MCP call, for its refusal record.
/// A task control invoked through `invoke_capability` keeps its control name;
/// anything not on the product's fixed lists is `unknown_tool`.
pub(crate) fn product_tool_name(name: &str, capability: Option<&str>) -> &'static str {
    if name == "invoke_capability"
        && let Some((control, _)) =
            capability.and_then(|capability| CAPABILITIES.iter().find(|(c, _)| *c == capability))
    {
        return control;
    }
    if let Some((tool, _)) = CAPABILITIES.iter().find(|(tool, _)| *tool == name) {
        return tool;
    }
    match name {
        "read_task_evidence" => return "read_task_evidence",
        "read_task_result" => return "read_task_result",
        _ => {}
    }
    crate::runtime::COMPACT_TOOL_NAMES
        .iter()
        .find(|tool| **tool == name)
        .copied()
        .unwrap_or("unknown_tool")
}

/// C5 fix 10: the fixed code of a refused product MCP call, read from the
/// product's own refusal text. Only the code is kept, never the text.
pub(crate) fn refusal_code(
    text: &str,
) -> kinewright_project::orchestration_accounting::ToolRefusalCode {
    use kinewright_project::orchestration_accounting::ToolRefusalCode as Code;
    let tokens = text
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    let token = |names: &[&str]| tokens.iter().any(|token| names.contains(token));
    let phrase = |phrases: &[&str]| phrases.iter().any(|phrase| text.contains(phrase));
    if phrase(&["result differs from the frozen task"]) {
        Code::StaleFreeze
    } else if phrase(&["task is not frozen"]) {
        Code::NotFrozen
    } else if phrase(&["published result changed"]) {
        Code::PublishedChanged
    } else if token(&["StaleGeneration", "StaleControl"])
        || phrase(&["control changed", "stale control"])
    {
        Code::StaleControl
    } else if phrase(&["timeline revision", "prepared edit plan"]) {
        Code::StaleRevision
    } else if token(&[
        "storage_backpressure",
        "durability_unknown",
        "unsettled",
        "save_in_progress",
        "PublicationUnknown",
    ]) || phrase(&["another publication must reconcile"])
    {
        Code::Storage
    } else if token(&[
        "Sends",
        "Tokens",
        "Time",
        "Replan",
        "Capacity",
        "DependencyCapacity",
        "run_limit",
    ]) {
        Code::Budget
    } else if token(&["RetiredUnresolved", "task_freezing"])
        || phrase(&["already terminal", "run is not pending", "terminal slot"])
    {
        Code::TerminalReserved
    } else if token(&["InvalidState"]) {
        Code::InvalidState
    } else if token(&[
        "task_scope",
        "task_authority_expired",
        "ScopeConflict",
        "root_refused",
        "identity_refused",
        "Unsupported",
    ]) || phrase(&[
        "internal capability",
        "cannot be invoked",
        "is not available",
    ]) {
        Code::NotAdmitted
    } else if token(&["invalid_request", "invalid_params"])
        || phrase(&["invalid bounded protocol value"])
    {
        Code::InvalidArguments
    } else {
        Code::Other
    }
}

impl TaskMcp {
    pub(crate) fn record_size(&self, tool: &'static str, bytes: Option<u64>) -> bool {
        #[cfg(target_os = "linux")]
        {
            use kinewright_project::usage_measurement::ToolResult;
            let authority = self.endpoint.authority();
            // Retain this task's bounded observer even during host/run teardown.
            // Check host first to preserve the host -> measurement lock order.
            let attached = self.host.upgrade().is_some_and(|owner| {
                owner.lock().is_ok_and(|host| {
                    host.hooks.runs.get(&authority.run).is_some_and(|run| {
                        run.tasks.contains_key(&authority.task)
                            && run
                                .measurements
                                .get(&authority.task)
                                .is_some_and(|observer| Arc::ptr_eq(observer, &self.measurements))
                    })
                })
            });
            let mut measurements = self.measurements.lock().unwrap_or_else(|error| {
                let mut measurements = error.into_inner();
                measurements.incomplete = true;
                measurements
            });
            measurements.incomplete |= !attached;
            if let Some(bytes) = bytes {
                measurements.result(ToolResult {
                    attempt: authority.attempt.0.clone(),
                    turn: None,
                    provider_call: None,
                    host_call: 0,
                    tool: tool.into(),
                    serialized_bytes: bytes,
                });
            } else {
                measurements.incomplete = true;
            }
            !measurements.incomplete
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (self, tool, bytes);
            false
        }
    }
    /// C5 fix 10: count one refused product MCP call in this task's durable
    /// accounting. Best effort: it never changes the call's reply, and a
    /// refusal the store itself cannot take is not retried.
    pub(crate) fn record_refusal(
        &self,
        tool: &'static str,
        code: kinewright_project::orchestration_accounting::ToolRefusalCode,
    ) {
        #[cfg(target_os = "linux")]
        if let Some(host) = self.host.upgrade()
            && let Ok(mut host) = host.lock()
        {
            let _ = host.record_tool_refusal(self.endpoint.authority(), tool, code);
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (self, tool, code);
    }
    pub(super) fn new(
        host: Weak<Mutex<HostCommandOwner>>,
        auth: HostAuth,
        endpoint: TaskEndpoint,
        #[cfg(target_os = "linux")] measurements: Arc<
            Mutex<kinewright_project::usage_measurement::Measurements>,
        >,
        #[cfg(all(target_os = "linux", any(test, feature = "test-util")))] stop_gate: Option<
            Arc<super::qualification_gate::MergeStopGate>,
        >,
    ) -> Self {
        Self {
            host,
            auth,
            endpoint,
            issuer: Arc::default(),
            terminal_state: Arc::default(),
            #[cfg(target_os = "linux")]
            measurements,
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
        let mut host = host.lock().map_err(|_| refused("host owner unavailable"))?;
        host.prove_retention()?;
        let (index, state, task) = host.resolve_task_mcp(&self.auth, &self.endpoint)?;
        let role = state
            .tasks
            .iter()
            .find(|record| record.id == task)
            .ok_or_else(|| refused("task missing"))?
            .brief
            .role;
        if !manifest::names(role).contains(&name) {
            return Err(refused("tool hidden for durable task role"));
        }
        if matches!(name, "read_task_result" | "read_task_evidence") {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Read {
                reference: kinewright_project::bundle_records::ObjectRef,
            }
            let read: Read = serde_json::from_value(arguments)?;
            let record = state.tasks.iter().find(|record| record.id == task).unwrap();
            let result_allowed = |reference: &kinewright_project::bundle_records::ObjectRef| {
                state.deliveries.iter().any(|delivery| {
                    &delivery.result == reference
                        && state.tasks.iter().any(|child| {
                            child.id == delivery.task && child.parent.as_ref() == Some(&task)
                        })
                }) || matches!(&record.state, crate::orchestration::orchestration_records::TaskState::Terminal(result) if result == reference)
            };
            let allowed = if name == "read_task_result" {
                result_allowed(&read.reference)
            } else {
                [
                    &record.brief.plan,
                    &record.brief.script,
                    &record.brief.inputs,
                    &record.brief.assignment,
                ]
                .contains(&&read.reference)
                    || record.brief.scope.asset_roots.contains(&read.reference)
                    || record.brief.scope.consent_refs.contains(&read.reference)
                    || state.results.iter().filter(|reference| result_allowed(reference)).any(|reference| {
                        host.owner().and_then(|owner| Ok(owner.store()?.read_run_record(reference)?))
                            .is_ok_and(|record| match &record.payload {
                                crate::orchestration::orchestration_records::OrchestrationPayload::Result(result) => {
                                    result.evidence.contains(&read.reference) || result.artifacts.contains(&read.reference)
                                        || result.delta_ref.as_ref() == Some(&read.reference)
                                        || result.usage_ref == read.reference || result.explanation == read.reference
                                }, _ => false,
                            })
                    })
            };
            if !allowed {
                return Err(refused("read reference outside task scope"));
            }
            return host.read_task_reference(&read.reference);
        }
        if matches!(name, "task_status" | "wait_task") {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Observe {
                #[serde(default)]
                after_sequence: u64,
            }
            let observe: Observe = serde_json::from_value(arguments)?;
            return Ok(
                json!({"run": host.observe_run(index, &state.run, observe.after_sequence)?,
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
        if control.attempt != self.endpoint.authority().attempt
            || control.owner_epoch != self.endpoint.authority().owner_epoch
            || control.expected_head != *self.endpoint.head()
        {
            return Err(refused("control epoch or candidate head changed"));
        }
        let command_name = match &control.command {
            OrchestrationCommand::AssignTask(_) => "assign_task",
            OrchestrationCommand::FreezeTask(_) => "freeze_task",
            OrchestrationCommand::PublishResult(_) => "publish_result",
            OrchestrationCommand::FinishTask(_) => "finish_task",
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
        let bound = host.bind_publication(&request)?;
        let replay = host.owner()?.orchestration_replay(&bound.envelope, None)?;
        let merge_replay = if let OrchestrationCommand::MergeTask(merge) = &request.envelope.command
        {
            host.saved_task_history(&state, &merge.task)?
                .and_then(|history| history.merge_envelope)
                .is_some_and(|original| original.id == request.envelope.id)
        } else {
            false
        };
        if control.expected_generation != host.owner()?.metadata_generation()?
            && replay.is_none()
            && !merge_replay
        {
            return Err(refused("stale control generation"));
        }
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
            // C5 fix 10: the task, base, head and delta identify a re-sent
            // publication; its host-owned usage reference never does. The
            // re-sent command is rebound to the stored result, so a replay of
            // the original control finds its receipt and nothing new is written.
            let reference = reference.clone();
            let bound = host.bind_publication(&request)?;
            return Ok(
                json!({"publication_status":{"already_published":true,"result":reference},"receipt":host.owner()?.orchestration_replay(&bound.envelope,None)?}),
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
            let reconciliation = host.bind_merge_receipt(
                &state,
                &merge.task,
                &request.envelope,
                self.endpoint.authority(),
            )?;
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
            return Ok(json!({"receipt": receipt,"reconciliation":reconciliation}));
        }
        let freeze = if let OrchestrationCommand::FreezeTask(task) = &request.envelope.command {
            Some(task.clone())
        } else {
            None
        };
        let receipt = host.dispatch_orchestration(index, &request)?;
        self.remember_terminal_state(&host);
        if receipt.decision == CommandDecision::Applied
            && let OrchestrationCommand::FinishTask(finish) = &request.envelope.command
        {
            let status = host.completion_reply_status(&state.run, finish, &receipt.envelope.id);
            return Ok(json!({"receipt":receipt,"completion_status":status}));
        }
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
            let record = self.owner()?.store()?.read_run_record(reference)?;
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
        #[cfg(target_os = "linux")]
        if self.native_recovery_pending || self.recovery_only {
            return Err(HostRefusal::new("recovery_frozen", "native recovery is pending").into());
        }
        endpoint.check_live().map_err(|_| {
            HostError::from(HostRefusal::new(
                "task_authority_expired",
                "task authority expired",
            ))
        })?;
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
            return Err(HostError::from(HostRefusal::new(
                "task_authority_expired",
                "task no longer live",
            )));
        }
        Ok((index, state, authority.task.clone()))
    }
}

impl HostCommandOwner {
    pub(super) fn stage_completion_history(
        &mut self,
        update: crate::orchestration::orchestration_commit::ValidatedRunUpdate,
    ) -> Result<crate::orchestration::orchestration_commit::ValidatedRunUpdate, HostError> {
        if update.receipt().decision != CommandDecision::Applied {
            return Ok(update);
        }
        let OrchestrationCommand::FinishTask(finish) = &update.envelope().command else {
            return Ok(update);
        };
        let identity = update
            .projection()
            .attempts
            .iter()
            .find(|attempt| {
                attempt.identity.task == finish.task
                    && update.envelope().authority.attempt.as_ref()
                        == Some(&attempt.identity.attempt)
            })
            .ok_or_else(|| refused("completion attempt missing"))?
            .identity
            .clone();
        let mut state = update.projection().clone();
        self.persist_runtime_task(&mut state, &identity, false)?;
        let mut segment = update.segment().clone();
        segment
            .events
            .last_mut()
            .ok_or_else(|| refused("completion event missing"))?
            .projection = state.clone();
        crate::orchestration::orchestration_commit::ValidatedRunUpdate::new(
            &update.envelope().project,
            update.envelope().expected_generation,
            update.envelope(),
            state,
            segment,
            update.receipt().decision.clone(),
        )
        .map_err(|_| refused("invalid completion checkpoint"))
    }
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
        let expected = update
            .projection()
            .tasks
            .iter()
            .find(|task| task.id == *target)
            .and_then(|task| task.progress.completion.as_deref())
            .map(|intent| intent.request.expected_revision);
        let frozen = runtime
            .freeze_at_revision(self.owner_mut()?, expected)
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
        let state = if let Some(receipt) = receipt {
            self.owner()?.orchestration_receipt_projection(receipt)?
        } else {
            self.owner()?
                .orchestration_projection(run)?
                .ok_or_else(|| refused("run missing"))?
        };
        self.restore_frozen_task(&state, task)?;
        let history = self
            .saved_task_history(&state, task)?
            .ok_or_else(|| refused("frozen history missing"))?;
        if let Some(result) = &history.frozen_result {
            return Ok(json!({"receipt":receipt,"history":history,"result":result}));
        }
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
            explanation: self
                .owner_mut()?
                .admit_host_grant(super::orchestration::TASK_RESULT_EXPLANATION)?,
        };
        Ok(json!({"receipt":receipt,"history":history,"result":result}))
    }
}

impl HostCommandOwner {
    fn read_task_reference(
        &self,
        reference: &kinewright_project::bundle_records::ObjectRef,
    ) -> Result<Value, HostError> {
        use kinewright_project::bundle_records::RecordKind;
        if reference.byte_length > (crate::protocol::limits::FRAME_BYTES / 2) as u64 {
            return Err(refused("task evidence capacity"));
        }
        let store = self.owner()?.store()?;
        match reference.record_kind {
            RecordKind::OwnedEvidence => {
                let bytes = store.read_evidence(reference)?;
                Ok(serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                    match std::str::from_utf8(&bytes) {
                        Ok(text) => Value::String(text.into()),
                        Err(_) => json!({"encoding":"bytes","contents":bytes.to_vec()}),
                    }
                }))
            }
            RecordKind::InputManifest => Ok(serde_json::to_value(
                &*store.read_record::<kinewright_project::owned_inputs::OwnedInputManifest>(
                    reference,
                )?,
            )?),
            RecordKind::RunIndex => Ok(serde_json::to_value(&*store.read_run_record(reference)?)?),
            _ => Err(refused("unsupported task evidence record")),
        }
    }
}
