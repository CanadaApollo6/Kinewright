//! Versioned task tools use durable roles and separately refreshed live controls.
pub(crate) use crate::orchestration::task_manifest as manifest;
#[path = "task_consent.rs"]
pub(crate) mod consent;
mod reply;
pub(crate) mod schema;
use super::{HostAuth, HostCommandOwner, HostError, HostRefusal};
use crate::orchestration::{
    orchestration_commit::CommandDecision,
    orchestration_records::{
        AuthorityBinding, OrchestrationCommand, OrchestrationEnvelope, OrchestrationPayload, Role,
        RunId, RunProjection, TaskId,
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
    confirmations: Option<crate::ConfirmationBroker>,
    auth: HostAuth,
    endpoint: TaskEndpoint,
    issuer: Arc<Mutex<Option<IssuerId>>>,
    terminal_state: Arc<Mutex<Option<Value>>>,
    #[cfg(all(feature = "te1-eval", target_os = "linux"))]
    evaluation_observer: crate::te1_eval::observe::ObserverSlot,
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
fn task_authority_error(reason: &str) -> HostError {
    HostRefusal::new(
        reason,
        if reason == "task_authority_paused" {
            "the run is paused; retry after it resumes"
        } else {
            "task authority expired"
        },
    )
    .into()
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
        "read_child_result" => return "read_child_result",
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
        "evidence_capacity",
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
        "task_authority_paused",
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
    pub(super) fn lock_owner<'a>(
        &self,
        owner: &'a Mutex<HostCommandOwner>,
    ) -> Result<std::sync::MutexGuard<'a, HostCommandOwner>, HostError> {
        loop {
            self.endpoint
                .check_live()
                .map_err(|why| task_authority_error(&why))?;
            match owner.try_lock() {
                Ok(host) => {
                    self.endpoint
                        .check_live()
                        .map_err(|why| task_authority_error(&why))?;
                    return Ok(host);
                }
                Err(std::sync::TryLockError::Poisoned(_)) => {
                    return Err(refused("host owner unavailable"));
                }
                Err(std::sync::TryLockError::WouldBlock) => {
                    #[cfg(all(test, target_os = "linux"))]
                    self.endpoint.signal_owner_wait_for_fixture();
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
        }
    }
    fn lock_reply_owner<'a>(
        &self,
        owner: &'a Mutex<HostCommandOwner>,
    ) -> Result<std::sync::MutexGuard<'a, HostCommandOwner>, HostError> {
        if self.endpoint.check_live().is_err() {
            owner
                .try_lock()
                .map_err(|_| refused("host owner unavailable"))
        } else {
            self.lock_owner(owner)
        }
    }
    pub(crate) fn with_confirmations(mut self, broker: crate::ConfirmationBroker) -> Self {
        self.confirmations = Some(broker);
        self
    }
    pub(crate) fn record_size(&self, tool: &'static str, bytes: Option<u64>) -> bool {
        #[cfg(target_os = "linux")]
        {
            use kinewright_project::usage_measurement::ToolResult;
            let authority = self.endpoint.authority();
            // Retain this task's bounded observer even during host/run teardown.
            // Check host first to preserve the host -> measurement lock order.
            let attached = self.host.upgrade().is_some_and(|owner| {
                self.lock_reply_owner(&owner).is_ok_and(|host| {
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
            && let Ok(mut host) = self.lock_reply_owner(&host)
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
        #[cfg(all(feature = "te1-eval", target_os = "linux"))]
        evaluation_observer: crate::te1_eval::observe::ObserverSlot,
        #[cfg(target_os = "linux")] measurements: Arc<
            Mutex<kinewright_project::usage_measurement::Measurements>,
        >,
        #[cfg(all(target_os = "linux", any(test, feature = "test-util")))] stop_gate: Option<
            Arc<super::qualification_gate::MergeStopGate>,
        >,
    ) -> Self {
        Self {
            host,
            confirmations: None,
            auth,
            endpoint,
            issuer: Arc::default(),
            terminal_state: Arc::default(),
            #[cfg(all(feature = "te1-eval", target_os = "linux"))]
            evaluation_observer,
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
        let owner = self.host.upgrade().ok_or_else(|| refused("host closed"))?;
        let mut host = self.lock_owner(&owner)?;
        // Uncertain storage cannot serve resolve_task_mcp's projection. Only
        // the retained original finish may reach exact-envelope reconciliation.
        if name == "finish_task" && host.hooks.pending.is_some() {
            return self.retry_pending_finish(&mut host, arguments);
        }
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
        if matches!(
            name,
            "get_timeline_state" | "read_task_evidence" | "read_child_result"
        ) {
            let record = state
                .tasks
                .iter()
                .find(|record| record.id == task)
                .ok_or_else(|| refused("task missing"))?;
            return match name {
                "get_timeline_state" => {
                    if arguments != json!({}) {
                        return Err(refused("overview takes no caller scope"));
                    }
                    let (revision, document) = host.context_snapshot(&state.run, &task)?;
                    host.context_overview(&state.run, record, revision, &document)
                }
                "read_task_evidence" => {
                    let read: super::task_context::EvidenceRead =
                        serde_json::from_value(arguments)?;
                    if matches!(
                        read.evidence,
                        super::task_context::EvidenceKind::Audio { .. }
                            | super::task_context::EvidenceKind::Frame { .. }
                    ) {
                        let prepared = host.prepare_media_context(index, &state, record, &read)?;
                        let rendered = {
                            drop(host);
                            match prepared.render(&self.endpoint, &owner) {
                                Ok(rendered) => rendered,
                                Err(error) => {
                                    if prepared.failed()?
                                        && let Ok(mut host) = self.lock_owner(&owner)
                                    {
                                        prepared.discard_failed(&mut host, &state.run, &task)?;
                                    }
                                    return Err(error);
                                }
                            }
                        };
                        let mut host = self.lock_owner(&owner)?;
                        let (index, state, task) =
                            host.resolve_task_mcp(&self.auth, &self.endpoint)?;
                        let record = state
                            .tasks
                            .iter()
                            .find(|record| record.id == task)
                            .ok_or_else(|| refused("task missing"))?;
                        host.finish_media_context(index, &state, record, &read, &rendered)
                    } else {
                        let result = host.read_context(index, &state, record, &read);
                        #[cfg(test)]
                        if let Some(gate) = host.hooks.context_reply_gate.take() {
                            *gate.outcome.lock().unwrap() = result.as_ref().ok().cloned();
                            drop(host);
                            gate.gate.0.wait();
                            gate.gate.1.wait();
                        }
                        result
                    }
                }
                _ => host.read_child_context(
                    index,
                    &state,
                    record,
                    &serde_json::from_value(arguments)?,
                ),
            };
        }
        if name == "read_task_result" {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Read {
                reference: kinewright_project::bundle_records::ObjectRef,
            }
            let read: Read = serde_json::from_value(arguments)?;
            let record = state
                .tasks
                .iter()
                .find(|record| record.id == task)
                .ok_or_else(|| refused("task missing"))?;
            let allowed = state.deliveries.iter().any(|delivery| {
                delivery.result == read.reference
                    && state.tasks.iter().any(|child| {
                        child.id == delivery.task && child.parent.as_ref() == Some(&task)
                    })
            }) || matches!(&record.state,crate::orchestration::orchestration_records::TaskState::Terminal(result) if *result==read.reference);
            if !allowed {
                return Err(refused("read reference outside task scope"));
            }
            return host.read_legacy_context(index, &state, record, &read.reference);
        } else if matches!(name, "task_status" | "wait_task") {
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
            let core = host.hooks.runs[&state.run].tasks[&task].core().clone();
            drop(host);
            let history = self.history_with_edit_consent(
                &core,
                step.expected_revision,
                name == "redo_task_edit",
            )?;
            return Ok(json!({"history":history}));
        }
        let mut arguments = arguments;
        if name == "assign_task" {
            let brief = arguments
                .pointer_mut("/command/value/brief")
                .and_then(Value::as_object_mut)
                .ok_or_else(|| refused("assignment brief required"))?;
            brief.insert(
                "provider".into(),
                serde_json::to_value(&state.tasks[0].brief.provider)?,
            );
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
        #[cfg(target_os = "linux")]
        if matches!(
            request.envelope.command,
            OrchestrationCommand::AssignTask(_)
        ) && let Some(receipt) = replay.as_ref()
        {
            // A broken new choice can end this turn without replacing its stored outcome.
            let outcome = json!({"receipt":receipt});
            drop(host);
            let _ = super::provider_readiness::check_run(&owner, &state.run);
            return Ok(outcome);
        }
        #[cfg(target_os = "linux")]
        if matches!(
            request.envelope.command,
            OrchestrationCommand::AssignTask(_)
        ) && replay.is_none()
        {
            drop(host);
            super::provider_readiness::check_run(&owner, &state.run).map_err(|why| {
                let reason = if super::provider_readiness::setup_failure(&why) {
                    super::provider_readiness::model_wait()
                } else {
                    super::provider_readiness::MODEL_CHECK_WAIT
                };
                HostRefusal::new("provider_wait", reason)
            })?;
            host = self.lock_owner(&owner)?;
            host.resolve_task_mcp(&self.auth, &self.endpoint)?;
            if host.bind_publication(&request)? != bound {
                let why = "Your provider choices changed while they were checked. Review Director and Specialists in Settings, then try again.";
                host.wait_for_provider(&state.run, why)?;
                return Err(HostRefusal::new(
                    "provider_wait",
                    super::provider_readiness::model_wait(),
                )
                .into());
            }
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
    /// Before role refresh, resume an uncertain exact finish or read its stored
    /// outcome. The slot-free route never drives completion or grants mutation.
    pub(crate) fn retry_pending_completion(
        &self,
        arguments: Value,
    ) -> Option<Result<Value, HostError>> {
        let host = self.host.upgrade()?;
        let mut host = match self.lock_reply_owner(&host) {
            Ok(host) => host,
            Err(error) => return Some(Err(error)),
        };
        if host.hooks.pending.is_some() {
            return Some(self.retry_pending_finish(&mut host, arguments));
        }
        // Live calls retain normal admission, refusal accounting and completion
        // continuation. This extra route is only a read after that authority ends.
        if host.resolve_task_mcp(&self.auth, &self.endpoint).is_ok() {
            return None;
        }
        let (index, request) = match self.original_finish_request(&host, arguments) {
            Ok(request) => request,
            Err(error) => return Some(Err(error)),
        };
        let replay = (|| {
            let scope = host.attachments[index]
                .producer_scope
                .as_ref()
                .ok_or_else(|| refused("original finish attachment changed"))?;
            let authority = self.endpoint.authority();
            if host.attachments[index].role != super::AttachmentRole::Producer
                || scope.run != authority.run
                || scope.task.as_ref() != Some(&authority.task)
                || scope.attempt.as_ref() != Some(&authority.attempt)
                || scope.endpoint.as_ref() != Some(&authority.endpoint)
                || scope.owner_epoch != authority.owner_epoch
                || scope.owner_epoch != host.owner()?.store()?.owner_epoch()
                || authority.project != host.owner()?.active_manual_project()?
            {
                return Err(refused("original finish attachment changed"));
            }
            for pin in &host.pins {
                if !host.attachments[index].roots.permits(&pin.anchor, false) {
                    return Err(refused("original finish roots changed"));
                }
            }
            host.require_metadata_issuer(index, &request.envelope.id.issuer)?;
            host.owner()?
                .orchestration_replay(&request.envelope, None)
                .map_err(Into::into)
        })();
        match replay {
            Ok(None) => None,
            Err(error) => Some(Err(error)),
            Ok(Some(receipt)) => {
                self.remember_terminal_state(&host);
                let OrchestrationCommand::FinishTask(finish) = &request.envelope.command else {
                    unreachable!()
                };
                let status = host.completion_reply_status(
                    &self.endpoint.authority().run,
                    finish,
                    &receipt.envelope.id,
                );
                Some(Ok(json!({"receipt":receipt,"completion_status":status})))
            }
        }
    }
    fn retry_pending_finish(
        &self,
        host: &mut HostCommandOwner,
        arguments: Value,
    ) -> Result<Value, HostError> {
        let (index, request) = self.original_finish_request(host, arguments)?;
        let receipt = host.dispatch_orchestration(index, &request)?;
        self.remember_terminal_state(host);
        if receipt.decision == CommandDecision::Applied
            && let OrchestrationCommand::FinishTask(finish) = &request.envelope.command
        {
            let status = host.completion_reply_status(
                &self.endpoint.authority().run,
                finish,
                &receipt.envelope.id,
            );
            return Ok(json!({"receipt":receipt,"completion_status":status}));
        }
        Ok(json!({"receipt":receipt}))
    }
    fn original_finish_request(
        &self,
        host: &HostCommandOwner,
        arguments: Value,
    ) -> Result<(usize, super::orchestration::ScopedOrchestrationRequest), HostError> {
        let retry_control: Control = serde_json::from_value(arguments)?;
        let authority = self.endpoint.authority();
        if retry_control.attempt != authority.attempt
            || retry_control.owner_epoch != authority.owner_epoch
            || retry_control.expected_head != *self.endpoint.head()
            || !matches!(&retry_control.command, OrchestrationCommand::FinishTask(finish) if finish.task == authority.task)
        {
            return Err(refused("original uncertain finish changed"));
        }
        let index = host.resolve_attachment(Some(&self.auth.bearer()))?;
        let mut envelope = OrchestrationEnvelope {
            id: retry_control.id,
            digest: serde_json::from_value(json!("0".repeat(64)))?,
            project: authority.project.clone(),
            expected_generation: retry_control.expected_generation,
            authority: AuthorityBinding {
                grant_ref: host.expected_grant(index)?,
                owner_epoch: retry_control.owner_epoch,
                endpoint: Some(authority.endpoint.clone()),
                attempt: Some(authority.attempt.clone()),
            },
            command: retry_control.command,
        };
        envelope.digest = envelope
            .computed_digest()
            .map_err(|_| refused("invalid original finish"))?;
        let request = super::orchestration::ScopedOrchestrationRequest {
            run: Some(authority.run.clone()),
            envelope,
            consume_batch: None,
        };
        Ok((index, request))
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
    pub(super) fn resolve_task_mcp(
        &self,
        auth: &HostAuth,
        endpoint: &TaskEndpoint,
    ) -> Result<(usize, RunProjection, TaskId), HostError> {
        #[cfg(target_os = "linux")]
        if self.native_recovery_pending || self.recovery_only {
            return Err(HostRefusal::new("recovery_frozen", "native recovery is pending").into());
        }
        endpoint
            .check_live()
            .map_err(|why| task_authority_error(&why))?;
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
    pub(super) fn read_task_reference(
        &self,
        reference: &kinewright_project::bundle_records::ObjectRef,
    ) -> Result<Value, HostError> {
        use kinewright_project::bundle_records::RecordKind;
        if reference.record_kind != RecordKind::RunIndex
            && reference.byte_length > (super::task_context::PAGE_BYTES * 8) as u64
        {
            return Err(super::task_context::record_too_large());
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
            RecordKind::RunIndex => {
                let mut record = (*store.read_run_record(reference)?).clone();
                if let OrchestrationPayload::Accounting(accounting) = &mut record.payload {
                    **accounting = super::provider_usage::provider_accounting_view(accounting)?;
                }
                Ok(serde_json::to_value(record)?)
            }
            RecordKind::SpecialistBase => Ok(serde_json::to_value(
                &*store.read_record::<kinewright_project::bundle_records::SpecialistBaseRecord>(
                    reference,
                )?,
            )?),
            _ => Err(refused("unsupported task evidence record")),
        }
    }
}

#[cfg(all(feature = "te1-eval", target_os = "linux"))]
impl TaskMcp {
    /// Called after the production handler has released its endpoint permit.
    pub(crate) fn eval_observe(
        &self,
        name: &str,
        args: Value,
        result: &rmcp::model::CallToolResult,
    ) -> std::io::Result<()> {
        let Some(observer) = self.evaluation_observer.get() else {
            return Ok(());
        };
        let result = observer.call(&self.endpoint, name, args, result);
        if result.is_err() {
            self.measurements
                .lock()
                .map_err(|e| std::io::Error::other(e.to_string()))?
                .incomplete = true;
        }
        result
    }
}
