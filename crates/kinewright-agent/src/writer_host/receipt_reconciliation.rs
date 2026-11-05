//! Project-read receipt recovery is independent of live task dispatch.
use super::{
    AttachmentRole, HostCommandOwner, HostError, HostRefusal, orchestration::PendingPublication,
};
use kinewright_project::{
    bundle_records::ProjectId,
    orchestration_records::{AttemptId, OrchestrationCommand, RunId, TaskId, task_command_digest},
    receipts::{CommandDigest, CommandId, ReceiptLookup},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptQuery {
    pub project: ProjectId,
    pub run: RunId,
    pub task: TaskId,
    pub attempt: AttemptId,
    pub id: CommandId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<CommandDigest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<OrchestrationCommand>,
}
impl ReceiptQuery {
    fn command_digest(&self) -> Result<CommandDigest, HostError> {
        let computed = self
            .command
            .as_ref()
            .map(task_command_digest)
            .transpose()
            .map_err(|_| refused())?;
        match (&self.digest, computed) {
            (Some(expected), Some(actual)) if *expected == actual => Ok(actual),
            (None, Some(actual)) => Ok(actual),
            (Some(expected), None) => Ok(expected.clone()),
            _ => Err(refused()),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReceiptReply {
    Stored {
        outcome: serde_json::Value,
        receipt: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        completion: Option<Box<super::CompletionStatus>>,
    },
    Pending,
    Unknown,
}
fn refused() -> HostError {
    HostRefusal::new(
        "receipt_scope",
        "original project/run/task/attempt/command identity required",
    )
    .into()
}
impl HostCommandOwner {
    pub(super) fn reconcile_task_receipt(
        &self,
        index: usize,
        query: &ReceiptQuery,
    ) -> Result<ReceiptReply, HostError> {
        // Resolved by the foreground bearer route, with project roots checked in
        // read mode. Task Producers cannot turn a dead endpoint into authority.
        if !matches!(
            self.attachments[index].role,
            AttachmentRole::Editor | AttachmentRole::Viewer
        ) || query.project != self.owner()?.active_manual_project()?
        {
            return Err(refused());
        }
        let digest = query.command_digest()?;
        // An uncertain namespace may not serve a current projection. Pending
        // identity is checked from the already authenticated retained request.
        let pending = match &self.hooks.pending {
            Some(PendingPublication::Wire(request, actor)) => Some((
                &request.envelope,
                request.run.as_ref(),
                self.attachments
                    .iter()
                    .find(|attachment| attachment.id == *actor)
                    .and_then(|attachment| attachment.producer_scope.as_ref())
                    .and_then(|scope| scope.task.as_ref()),
            )),
            Some(PendingPublication::Owner(update)) => Some((
                update.envelope(),
                Some(&update.projection().run),
                update
                    .projection()
                    .attempts
                    .iter()
                    .find(|attempt| {
                        Some(&attempt.identity.attempt)
                            == update.envelope().authority.attempt.as_ref()
                    })
                    .map(|attempt| &attempt.identity.task),
            )),
            _ => None,
        };
        if let Some((envelope, run, task)) = pending
            && envelope.id == query.id
        {
            if run != Some(&query.run)
                || task != Some(&query.task)
                || envelope.project != query.project
                || task_command_digest(&envelope.command).map_err(|_| refused())? != digest
                || envelope.authority.attempt.as_ref() != Some(&query.attempt)
            {
                return Err(refused());
            }
            return Ok(ReceiptReply::Pending);
        }
        let state = self
            .owner()?
            .orchestration_projection(&query.run)?
            .ok_or_else(refused)?;
        let original_attempt =
            |a: &kinewright_project::orchestration_records::AttemptReservation| {
                a.identity.project == query.project
                    && a.identity.run == query.run
                    && a.identity.task == query.task
                    && a.identity.attempt == query.attempt
            };
        if !state.attempts.iter().any(original_attempt)
            && !self
                .attempt_archive(&state)?
                .1
                .attempts
                .iter()
                .any(original_attempt)
        {
            return Err(refused());
        }
        if let Some(receipt) = self.owner()?.orchestration_receipt(&query.run, &query.id)? {
            if task_command_digest(&receipt.envelope.command).map_err(|_| refused())? != digest
                || receipt.envelope.project != query.project
                || receipt.envelope.authority.attempt.as_ref() != Some(&query.attempt)
            {
                return Err(refused());
            }
            let outcome = self.stored_task_outcome(&receipt)?;
            return Ok(ReceiptReply::Stored {
                outcome,
                completion: if let OrchestrationCommand::FinishTask(finish) =
                    &receipt.envelope.command
                {
                    let intent = state
                        .tasks
                        .iter()
                        .find(|task| task.id == finish.task)
                        .and_then(|task| task.progress.completion.as_deref())
                        .ok_or_else(refused)?;
                    if intent.id != query.id {
                        return Err(refused());
                    }
                    Some(Box::new(self.completion_status(&state, intent)?))
                } else {
                    None
                },
                receipt: serde_json::to_value(receipt)?,
            });
        }
        // Candidate merge receipts live in the manual-history journal. The
        // original durable task history binds the command to its Director.
        for original_run in self.owner()?.orchestration_runs()? {
            let original_state = self
                .owner()?
                .orchestration_projection(&original_run)?
                .ok_or_else(refused)?;
            for task in &original_state.tasks {
                // Read the original retained history, not its current edit scope:
                // merging can legitimately advance the parent's candidate head.
                for reference in task.progress.decisions.iter().rev().filter(|r| {
                    r.record_kind == kinewright_project::bundle_records::RecordKind::RunIndex
                }) {
                    let record = self.owner()?.store()?.read_run_record(reference)?;
                    let kinewright_project::orchestration_records::OrchestrationPayload::TaskHistory(history) = &record.payload else { continue; };
                    let Some(envelope) = &history.merge_envelope else {
                        continue;
                    };
                    if envelope.id != query.id {
                        continue;
                    }
                    let origin = history.merge_authority.as_ref().ok_or_else(refused)?;
                    if original_run != query.run
                        || history.run != original_run
                        || history.task != task.id
                        || history.project != query.project
                        || task.parent.as_ref() != Some(&query.task)
                        || envelope.project_id != query.project
                        || origin.caller.project != query.project
                        || origin.caller.run != query.run
                        || origin.caller.task != query.task
                        || origin.caller.attempt != query.attempt
                        || origin.id != query.id
                        || origin.digest != digest
                    {
                        return Err(refused());
                    }
                    return match self.owner()?.manual_transition_receipt(envelope)? {
                        ReceiptLookup::Committed(receipt) | ReceiptLookup::Retired(receipt) => {
                            let receipt = serde_json::to_value(receipt)?;
                            Ok(ReceiptReply::Stored {
                                completion: None,
                                outcome: serde_json::json!({"receipt":receipt,"reconciliation":ReceiptQuery {
                                    project:origin.caller.project.clone(),run:origin.caller.run.clone(),task:origin.caller.task.clone(),attempt:origin.caller.attempt.clone(),id:origin.id.clone(),digest:Some(origin.digest.clone()),command:None,
                                }}),
                                receipt,
                            })
                        }
                        ReceiptLookup::Pending => Ok(ReceiptReply::Pending),
                        _ => Ok(ReceiptReply::Unknown),
                    };
                }
            }
        }
        Ok(ReceiptReply::Unknown)
    }
}

impl HostCommandOwner {
    /// Bind the manual journal receipt to the authenticated original Director
    /// before any merge side effect, retaining the binding across recovery.
    pub(super) fn bind_merge_receipt(
        &mut self,
        state: &kinewright_project::orchestration_records::RunProjection,
        child: &TaskId,
        envelope: &kinewright_project::orchestration_records::OrchestrationEnvelope,
        authority: &crate::orchestration::tasks::RuntimeTaskAuthority,
    ) -> Result<ReceiptQuery, HostError> {
        use kinewright_project::orchestration_records::{
            OrchestrationPayload, OrchestrationRecord, TaskMergeAuthority,
        };
        let caller = state
            .attempts
            .iter()
            .find(|attempt| {
                attempt.identity.task == authority.task
                    && attempt.identity.attempt == authority.attempt
            })
            .ok_or_else(refused)?
            .identity
            .clone();
        let binding = TaskMergeAuthority {
            caller: caller.clone(),
            id: envelope.id.clone(),
            digest: task_command_digest(&envelope.command).map_err(|_| refused())?,
        };
        let mut history = self.saved_task_history(state, child)?.ok_or_else(refused)?;
        if let Some(original) = &history.merge_authority {
            if *original != binding {
                return Err(refused());
            }
        } else {
            if history.merge_envelope.is_some() {
                // Older committed merges have no durable caller-attempt proof.
                // A current caller cannot manufacture that original authority.
                return Err(refused());
            }
            history.merge_authority = Some(binding);
            let sealed = self
                .owner_mut()?
                .admit_scheduler_record(&OrchestrationRecord {
                    version: 1,
                    payload: OrchestrationPayload::TaskHistory(Box::new(history)),
                })?;
            let mut next = state.clone();
            let child = next
                .tasks
                .iter_mut()
                .find(|task| &task.id == child)
                .ok_or_else(refused)?;
            if child.progress.decisions.len()
                >= kinewright_project::orchestration_records::ITEM_LIMIT
            {
                return Err(refused());
            }
            child.progress.decisions.push(sealed.reference.clone());
            let producer = self.run_producer(&state.run)?;
            self.publish_native(&producer, next, &caller, vec![])?;
        }
        Ok(ReceiptQuery {
            project: caller.project,
            run: caller.run,
            task: caller.task,
            attempt: caller.attempt,
            id: envelope.id.clone(),
            digest: Some(task_command_digest(&envelope.command).map_err(|_| refused())?),
            command: None,
        })
    }
}

impl HostCommandOwner {
    fn stored_task_outcome(
        &self,
        receipt: &kinewright_project::orchestration_commit::OrchestrationReceipt,
    ) -> Result<serde_json::Value, HostError> {
        use kinewright_project::orchestration_records::OrchestrationCommand;
        if receipt.decision == kinewright_project::orchestration_commit::CommandDecision::Applied
            && let OrchestrationCommand::FreezeTask(task) = &receipt.envelope.command
        {
            let state = self.owner()?.orchestration_receipt_projection(receipt)?;
            if let Some(history) = self.saved_task_history(&state, task)?
                && let Some(result) = &history.frozen_result
            {
                return Ok(
                    serde_json::json!({"receipt":receipt,"history":history,"result":result}),
                );
            }
        }
        Ok(serde_json::json!({"receipt":receipt}))
    }
}
