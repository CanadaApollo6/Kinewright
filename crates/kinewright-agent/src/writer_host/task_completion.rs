//! A durable intent owns fixed freeze/publication identities, never authority.
use super::{HostCommandOwner, HostError, HostRefusal, orchestration::ScopedOrchestrationRequest};
use crate::orchestration::{
    orchestration_commit::{CommandDecision, OrchestrationReceipt},
    orchestration_records::{
        AuthorityBinding, CompletionIntent, CompletionStep, DeliveryState, FinishTask,
        OrchestrationCommand, OrchestrationEnvelope, PublishResult, RunOutcome, RunProjection,
        TaskId, TaskResult,
    },
};
use kinewright_project::receipts::CommandId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionState {
    Pending,
    Blocked,
    Completed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionPhase {
    Freeze,
    Publication,
    Delivery,
    Settled,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionStatus {
    pub task: TaskId,
    pub id: CommandId,
    pub state: CompletionState,
    pub phase: CompletionPhase,
    pub reason: Option<String>,
    pub freeze_receipt: Option<OrchestrationReceipt>,
    pub publication_receipt: Option<OrchestrationReceipt>,
    pub result: Option<TaskResult>,
}

/// A trusted restore or a permanent live failure records a recovery cause.
/// It carries no publication capability and cannot revive a task endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RecoveryBlock {
    Completion(TaskId),
    Merge(TaskId),
}
fn refused(reason: &str) -> HostError {
    HostRefusal::new("task_scope", reason).into()
}
impl HostCommandOwner {
    pub(super) fn inspect_recovery_blocks(
        &self,
        state: &RunProjection,
    ) -> Result<Vec<RecoveryBlock>, HostError> {
        use crate::orchestration::orchestration_records::OrchestrationPayload;
        use kinewright_project::{bundle_records::RecordKind, receipts::ReceiptLookup};
        let mut blocks = Vec::new();
        for task in &state.tasks {
            if let Some(intent) = task.progress.completion.as_deref() {
                let id = intent
                    .step_id(CompletionStep::Publication)
                    .map_err(|_| refused("completion identity capacity"))?;
                if self
                    .owner()?
                    .orchestration_receipt(&state.run, &id)?
                    .is_none_or(|receipt| receipt.decision != CommandDecision::Applied)
                {
                    blocks.push(RecoveryBlock::Completion(task.id.clone()));
                    continue;
                }
            }
            for reference in task
                .progress
                .decisions
                .iter()
                .rev()
                .filter(|r| r.record_kind == RecordKind::RunIndex)
            {
                let record = self.owner()?.store()?.read_run_record(reference)?;
                let OrchestrationPayload::TaskHistory(history) = &record.payload else {
                    continue;
                };
                if history.project != self.owner()?.project_id()?
                    || history.run != state.run
                    || history.task != task.id
                {
                    return Err(refused("recovery history scope changed"));
                }
                if let Some(envelope) = &history.merge_envelope
                    && !matches!(
                        self.owner()?.manual_transition_receipt(envelope)?,
                        ReceiptLookup::Committed(_) | ReceiptLookup::Retired(_)
                    )
                {
                    blocks.push(RecoveryBlock::Merge(task.id.clone()));
                }
                break;
            }
        }
        Ok(blocks)
    }
    pub(super) fn run_recovery_blocks(
        &self,
        state: &RunProjection,
    ) -> Result<Vec<RecoveryBlock>, HostError> {
        if state.outcome != RunOutcome::Pending {
            return Ok(Vec::new());
        }
        if let Some(run) = self.hooks.runs.get(&state.run) {
            if run.recovery_blocks.is_empty() {
                return Ok(Vec::new());
            }
            let current = self.inspect_recovery_blocks(state)?;
            return Ok(run
                .recovery_blocks
                .iter()
                .filter(|block| current.contains(block))
                .cloned()
                .collect());
        }
        self.inspect_recovery_blocks(state)
    }
    pub(super) fn run_recovery_message(
        &self,
        state: &RunProjection,
    ) -> Result<Option<String>, HostError> {
        let blocks = if state.outcome == RunOutcome::Pending {
            self.run_recovery_blocks(state)?
        } else {
            self.inspect_recovery_blocks(state)?
        };
        let Some(block) = blocks.first() else {
            if state.outcome == RunOutcome::Pending {
                return Ok(None);
            }
            return Ok(self
                .owner()?
                .retained_task_work()?
                .into_iter()
                .filter(|work| work.run == state.run)
                .min_by_key(|work| (work.open_on_timeline, !work.frozen))
                .map(|work| format!("retained_task_work: {}", work.disclosure)));
        };
        let (task, code) = match block {
            RecoveryBlock::Completion(task) => (task, "completion_recovery_requires_person"),
            RecoveryBlock::Merge(task) => (task, "merge_recovery_requires_person"),
        };
        let retained = self
            .owner()?
            .retained_task_work()?
            .into_iter()
            .find(|work| work.run == state.run && &work.task == task);
        Ok(Some(if let Some(work) = retained {
            format!("{code}: {}", work.disclosure)
        } else {
            format!(
                "{code}: task {} has retained work. Stop and wait for cleanup before reviewing its saved task branch.",
                task.0.as_str()
            )
        }))
    }
    /// Reserve the fixed identities on every public Producer route, not only MCP.
    pub(super) fn authorize_completion_command(
        state: &RunProjection,
        envelope: &OrchestrationEnvelope,
    ) -> Result<(), HostError> {
        for task in &state.tasks {
            let Some(intent) = task.progress.completion.as_deref() else {
                continue;
            };
            let freeze = intent
                .step_id(CompletionStep::Freeze)
                .map_err(|_| refused("completion identity capacity"))?;
            let publish = intent
                .step_id(CompletionStep::Publication)
                .map_err(|_| refused("completion identity capacity"))?;
            let correct = match &envelope.command {
                OrchestrationCommand::FreezeTask(target) if *target == task.id => {
                    envelope.id == freeze
                }
                OrchestrationCommand::PublishResult(result) if result.task == task.id => {
                    envelope.id == publish
                }
                _ => envelope.id != freeze && envelope.id != publish,
            };
            if !correct {
                return Err(refused("completion substep identity reserved"));
            }
        }
        Ok(())
    }
    pub(super) fn begin_finish_ingress(
        &self,
        request: &ScopedOrchestrationRequest,
    ) -> Result<Option<crate::orchestration::tasks::FinishDrain>, HostError> {
        let OrchestrationCommand::FinishTask(finish) = &request.envelope.command else {
            return Ok(None);
        };
        let run = request.run.as_ref().ok_or_else(|| refused("run missing"))?;
        let endpoint = self
            .hooks
            .runs
            .get(run)
            .and_then(|run| run.tasks.get(&finish.task))
            .ok_or_else(|| refused("task actor missing"))?
            .endpoint();
        let owner = self.owner()?;
        let state = owner
            .orchestration_projection(run)?
            .ok_or_else(|| refused("run missing"))?;
        if !owner.has_unresolved_publication()
            && state
                .tasks
                .iter()
                .find(|task| task.id == finish.task)
                .is_some_and(|task| task.progress.completion.is_none())
        {
            endpoint.reconcile_unapplied_finish();
        }
        endpoint
            .begin_finish()
            .map(Some)
            .map_err(|reason| refused(&reason))
    }
    pub(super) fn finish_revision_matches(
        &self,
        request: &ScopedOrchestrationRequest,
    ) -> Result<bool, HostError> {
        let OrchestrationCommand::FinishTask(finish) = &request.envelope.command else {
            return Ok(true);
        };
        let actor = self
            .hooks
            .runs
            .get(request.run.as_ref().ok_or_else(|| refused("run missing"))?)
            .and_then(|run| run.tasks.get(&finish.task))
            .ok_or_else(|| refused("task actor missing"))?;
        let kinewright_core::Event::QueryResult(kinewright_core::QueryResult::Snapshot {
            revision,
            ..
        }) = actor
            .core()
            .request(kinewright_core::Command::Query(
                kinewright_core::Query::Snapshot,
            ))
            .map_err(|_| refused("task revision unavailable"))?
        else {
            return Err(refused("task revision unavailable"));
        };
        Ok(revision == finish.expected_revision)
    }
    /// Never let a later failure replace the already committed immutable outcome.
    pub(super) fn drive_completion(
        &mut self,
        index: usize,
        request: &ScopedOrchestrationRequest,
        receipt: &OrchestrationReceipt,
    ) {
        if receipt.decision != CommandDecision::Applied
            || !matches!(
                request.envelope.command,
                OrchestrationCommand::FinishTask(_)
            )
        {
            return;
        }
        self.hooks.completion_failure = None;
        if let Err(error) = self.drive_completion_inner(index, request) {
            let retryable = Self::completion_failure_retryable(&error);
            if !retryable
                && let (Some(run), OrchestrationCommand::FinishTask(finish)) =
                    (&request.run, &request.envelope.command)
                && let Some(runtime) = self.hooks.runs.get_mut(run)
            {
                let block = RecoveryBlock::Completion(finish.task.clone());
                if !runtime.recovery_blocks.contains(&block) {
                    runtime.recovery_blocks.push(block);
                }
            }
            self.hooks.completion_failure = Some((
                receipt.envelope.id.clone(),
                format!(
                    "{}:{:?}",
                    if retryable {
                        "retryable_substep_refused"
                    } else {
                        "permanent_substep_refused"
                    },
                    super::task_mcp::refusal_code(&error.to_string())
                ),
            ));
        }
    }
    fn completion_failure_retryable(error: &HostError) -> bool {
        let refusal = error.refusal();
        #[cfg(test)]
        if refusal.message.contains("fixture lost reply") {
            return true;
        }
        matches!(
            refusal.code.as_str(),
            "io" | "durability_unknown"
                | "unsettled"
                | "save_in_progress"
                | "storage_backpressure"
                | "contention"
                | "cleanup_pending"
        )
    }
    fn drive_completion_inner(
        &mut self,
        index: usize,
        request: &ScopedOrchestrationRequest,
    ) -> Result<(), HostError> {
        #[cfg(test)]
        if std::mem::take(&mut self.hooks.completion_before_freeze) {
            return Err(refused("fixture lost reply before freeze"));
        }
        let run = request.run.as_ref().ok_or_else(|| refused("run missing"))?;
        let OrchestrationCommand::FinishTask(finish) = &request.envelope.command else {
            return Err(refused("not a completion"));
        };
        let state = self
            .owner()?
            .orchestration_projection(run)?
            .ok_or_else(|| refused("run missing"))?;
        let intent = state
            .tasks
            .iter()
            .find(|task| task.id == finish.task)
            .and_then(|task| task.progress.completion.as_deref())
            .ok_or_else(|| refused("completion intent missing"))?
            .clone();
        if intent.id != request.envelope.id || intent.request != *finish {
            return Err(refused("completion payload changed"));
        }
        let frozen = self.completion_step(
            index,
            run,
            &intent,
            CompletionStep::Freeze,
            OrchestrationCommand::FreezeTask(finish.task.clone()),
        )?;
        #[cfg(test)]
        self.completion_test_cut(CompletionStep::Freeze)?;
        let frozen_state = self.owner()?.orchestration_receipt_projection(&frozen)?;
        let result = self
            .saved_task_history(&frozen_state, &finish.task)?
            .and_then(|history| history.frozen_result)
            .ok_or_else(|| refused("frozen result missing"))?;
        self.completion_step(
            index,
            run,
            &intent,
            CompletionStep::Publication,
            OrchestrationCommand::PublishResult(Box::new(PublishResult {
                task: finish.task.clone(),
                result,
            })),
        )?;
        #[cfg(test)]
        self.completion_test_cut(CompletionStep::Publication)?;
        Ok(())
    }
    #[cfg(test)]
    fn completion_test_cut(&mut self, step: CompletionStep) -> Result<(), HostError> {
        if self.hooks.completion_cut == Some(step) {
            self.hooks.completion_cut = None;
            return Err(refused("fixture lost reply"));
        }
        Ok(())
    }
    fn completion_step(
        &mut self,
        index: usize,
        run: &crate::orchestration::orchestration_records::RunId,
        intent: &CompletionIntent,
        step: CompletionStep,
        command: OrchestrationCommand,
    ) -> Result<OrchestrationReceipt, HostError> {
        let id = intent
            .step_id(step)
            .map_err(|_| refused("completion identity capacity"))?;
        // A replay uses its original generation, grant, result and accounting ref.
        if let Some(receipt) = self.owner()?.orchestration_receipt(run, &id)? {
            let same_command = match (&command, &receipt.envelope.command) {
                (
                    OrchestrationCommand::PublishResult(expected),
                    OrchestrationCommand::PublishResult(stored),
                ) => {
                    let mut expected = expected.clone();
                    expected.result.usage_ref = stored.result.usage_ref.clone();
                    expected == *stored
                }
                _ => command == receipt.envelope.command,
            };
            if receipt.decision != CommandDecision::Applied
                || !same_command
                || receipt.envelope.authority.attempt.as_ref() != Some(&intent.attempt)
            {
                return Err(refused("completion substep refused"));
            }
            return Ok(receipt);
        }
        let endpoint = self
            .hooks
            .runs
            .get(run)
            .and_then(|run| run.tasks.get(&intent.request.task))
            .ok_or_else(|| refused("completion task actor missing"))?
            .endpoint();
        endpoint
            .check_live()
            .map_err(|_| refused("task authority expired"))?;
        if endpoint.authority().attempt != intent.attempt {
            return Err(refused("completion attempt changed"));
        }
        let mut envelope = OrchestrationEnvelope {
            id,
            digest: intent.digest.clone(),
            project: self.owner()?.active_manual_project()?,
            expected_generation: self.owner()?.metadata_generation()?,
            authority: AuthorityBinding {
                grant_ref: self.expected_grant(index)?,
                owner_epoch: self.owner()?.store()?.owner_epoch(),
                endpoint: Some(endpoint.authority().endpoint.clone()),
                attempt: Some(intent.attempt.clone()),
            },
            command,
        };
        envelope.digest = envelope
            .computed_digest()
            .map_err(|_| refused("invalid completion substep"))?;
        let step = ScopedOrchestrationRequest {
            run: Some(run.clone()),
            envelope,
            consume_batch: None,
        };
        self.authorize_orchestration(index, &step)?;
        let receipt = self.dispatch_orchestration(index, &step)?;
        if receipt.decision != CommandDecision::Applied {
            return Err(refused("completion substep refused"));
        }
        Ok(receipt)
    }
    /// A later progress read cannot erase an acknowledged intent receipt.
    pub(super) fn completion_reply_status(
        &mut self,
        run: &crate::orchestration::orchestration_records::RunId,
        finish: &FinishTask,
        id: &CommandId,
    ) -> CompletionStatus {
        let read = (|| -> Result<CompletionStatus, HostError> {
            #[cfg(test)]
            if std::mem::take(&mut self.hooks.completion_status_read_failure) {
                return Err(refused("fixture progress read unavailable"));
            }
            let current = self
                .owner()?
                .orchestration_projection(run)?
                .ok_or_else(|| refused("run missing"))?;
            let intent = current
                .tasks
                .iter()
                .find(|task| task.id == finish.task)
                .and_then(|task| task.progress.completion.as_deref())
                .ok_or_else(|| refused("completion intent missing"))?;
            self.completion_status(&current, intent)
        })();
        read.unwrap_or_else(|_| CompletionStatus {
            task: finish.task.clone(),
            id: id.clone(),
            state: CompletionState::Blocked,
            phase: CompletionPhase::Freeze,
            reason: Some("completion_state_unavailable".into()),
            freeze_receipt: None,
            publication_receipt: None,
            result: None,
        })
    }
    pub(super) fn completion_statuses(
        &self,
        state: &RunProjection,
    ) -> Result<Vec<CompletionStatus>, HostError> {
        state
            .tasks
            .iter()
            .filter_map(|task| task.progress.completion.as_deref())
            .map(|intent| self.completion_status(state, intent))
            .collect()
    }
    pub(super) fn completion_status(
        &self,
        state: &RunProjection,
        intent: &CompletionIntent,
    ) -> Result<CompletionStatus, HostError> {
        let freeze_receipt = self.owner()?.orchestration_receipt(
            &state.run,
            &intent
                .step_id(CompletionStep::Freeze)
                .map_err(|_| refused("completion identity capacity"))?,
        )?;
        let publication_receipt = self.owner()?.orchestration_receipt(
            &state.run,
            &intent
                .step_id(CompletionStep::Publication)
                .map_err(|_| refused("completion identity capacity"))?,
        )?;
        let published = publication_receipt
            .as_ref()
            .filter(|r| r.decision == CommandDecision::Applied);
        let task = state
            .tasks
            .iter()
            .find(|task| task.id == intent.request.task)
            .ok_or_else(|| refused("task missing"))?;
        let result = if let Some(receipt) = published {
            let OrchestrationCommand::PublishResult(result) = &receipt.envelope.command else {
                return Err(refused("completion publication changed"));
            };
            Some(result.result.clone())
        } else if let Some(receipt) = freeze_receipt
            .as_ref()
            .filter(|receipt| receipt.decision == CommandDecision::Applied)
        {
            let original = self.owner()?.orchestration_receipt_projection(receipt)?;
            self.saved_task_history(&original, &task.id)?
                .and_then(|history| history.frozen_result)
        } else {
            None
        };
        let delivery = state
            .deliveries
            .iter()
            .find(|delivery| delivery.task == task.id);
        let settled = task.parent.is_none()
            || delivery.is_some_and(|d| matches!(d.state, DeliveryState::Observed(_)));
        let (status, phase, reason) = if published.is_some() && settled {
            (CompletionState::Completed, CompletionPhase::Settled, None)
        } else if published.is_some() {
            if delivery.is_some_and(|d| d.state == DeliveryState::Disposed) {
                (
                    CompletionState::Blocked,
                    CompletionPhase::Delivery,
                    Some("delivery_disposed".into()),
                )
            } else {
                (CompletionState::Pending, CompletionPhase::Delivery, None)
            }
        } else {
            let phase = if freeze_receipt
                .as_ref()
                .is_some_and(|receipt| receipt.decision == CommandDecision::Applied)
            {
                CompletionPhase::Publication
            } else {
                CompletionPhase::Freeze
            };
            let retained_block = self.hooks.runs.get(&state.run).is_some_and(|run| {
                run.recovery_blocks
                    .contains(&RecoveryBlock::Completion(task.id.clone()))
            });
            let reason = if state.outcome != RunOutcome::Pending {
                Some("run_stopped".into())
            } else if !self.completion_authority_live(state, intent)? {
                Some("task_authority_unavailable".into())
            } else {
                self.hooks
                    .completion_failure
                    .as_ref()
                    .filter(|(id, _)| *id == intent.id)
                    .map(|(_, reason)| reason.clone())
                    .or_else(|| {
                        retained_block.then(|| "completion_recovery_requires_person".into())
                    })
            };
            (
                if reason.is_some() {
                    CompletionState::Blocked
                } else {
                    CompletionState::Pending
                },
                phase,
                reason,
            )
        };
        Ok(CompletionStatus {
            task: task.id.clone(),
            id: intent.id.clone(),
            state: status,
            phase,
            reason,
            freeze_receipt,
            publication_receipt,
            result,
        })
    }
    fn completion_authority_live(
        &self,
        state: &RunProjection,
        intent: &CompletionIntent,
    ) -> Result<bool, HostError> {
        let subject = self.owner()?.metadata_issuer_subject(&intent.id.issuer)?;
        let endpoint = self
            .hooks
            .runs
            .get(&state.run)
            .and_then(|run| run.tasks.get(&intent.request.task))
            .map(crate::orchestration::tasks::EditorTask::endpoint);
        Ok(endpoint.is_some_and(|endpoint| {
            endpoint.check_live().is_ok()
                && self.attachments.iter().any(|attachment| {
                    subject.as_deref() == Some(attachment.subject.as_str())
                        && attachment.producer_scope.as_ref().is_some_and(|scope| {
                            scope.task.as_ref() == Some(&intent.request.task)
                                && scope.run == state.run
                                && scope.attempt.as_ref() == Some(&intent.attempt)
                                && scope.endpoint.as_ref() == Some(&endpoint.authority().endpoint)
                                && !scope.expired()
                                && scope.owner_epoch == endpoint.authority().owner_epoch
                        })
                })
        }))
    }
}
