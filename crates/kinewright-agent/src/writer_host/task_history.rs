//! Task actor checkpoints and retired attempts use the same acknowledged H1 root.
#![allow(clippy::missing_errors_doc)]
use super::{AttachmentRole, HostAuth, HostCommandOwner, HostError, HostRefusal};
use crate::orchestration::{
    ExecutionIdentity,
    orchestration_records::{
        AttemptId, AttemptState, OrchestrationPayload, OrchestrationRecord, RetiredAttemptArchive,
        RunId, RunOutcome, RunProjection, TaskHistoryRecord, TaskId,
    },
};
use kinewright_project::{
    bundle_records::{ObjectRef, RecordKind},
    receipts::{CommandId, ReceiptLookup},
};

fn refused(reason: impl std::fmt::Debug) -> HostError {
    HostRefusal::new("task_history", &format!("{reason:?}")).into()
}
impl HostCommandOwner {
    fn history_scope(
        &mut self,
        producer: &HostAuth,
        run: &RunId,
    ) -> Result<RunProjection, HostError> {
        let index = self.resolve_attachment(Some(&producer.bearer()))?;
        let scope = self.attachments[index]
            .producer_scope
            .as_ref()
            .ok_or_else(|| refused("run Producer missing"))?;
        if self.attachments[index].role != AttachmentRole::Producer
            || scope.run != *run
            || scope.task.is_some()
            || scope.owner_epoch != self.owner()?.store()?.owner_epoch()
            || scope.expired()
            || self.hooks.pending.is_some()
            || self.recovering
            || self.switch_frozen
        {
            return Err(refused("run authority changed"));
        }
        self.owner()?
            .orchestration_projection(run)?
            .ok_or_else(|| refused("run missing"))
    }
    pub(super) fn saved_task_history(
        &self,
        state: &RunProjection,
        task: &TaskId,
    ) -> Result<Option<TaskHistoryRecord>, HostError> {
        let task = state
            .tasks
            .iter()
            .find(|t| t.id == *task)
            .ok_or_else(|| refused("task missing"))?;
        for reference in task
            .progress
            .decisions
            .iter()
            .rev()
            .filter(|r| r.record_kind == RecordKind::RunIndex)
        {
            let record = self
                .owner()?
                .store()?
                .read_record::<OrchestrationRecord>(reference)?;
            if let OrchestrationPayload::TaskHistory(history) = &record.payload {
                if history.project != self.owner()?.project_id()?
                    || history.run != state.run
                    || history.task != task.id
                    || history.base != task.brief.scope.base
                {
                    return Err(refused("history scope changed"));
                }
                return Ok(Some((**history).clone()));
            }
        }
        Ok(None)
    }
    fn attempt_archive(
        &self,
        state: &RunProjection,
    ) -> Result<(Option<ObjectRef>, RetiredAttemptArchive), HostError> {
        for reference in state.usage.iter().rev() {
            let record = self
                .owner()?
                .store()?
                .read_record::<OrchestrationRecord>(reference)?;
            if let OrchestrationPayload::AttemptArchive(archive) = &record.payload {
                if archive.run != state.run {
                    return Err(refused("archive run changed"));
                }
                return Ok((Some(reference.clone()), (**archive).clone()));
            }
        }
        Ok((
            None,
            RetiredAttemptArchive {
                run: state.run.clone(),
                attempts: vec![],
            },
        ))
    }
    #[cfg(target_os = "linux")]
    pub(crate) fn recovery_attempt(
        &self,
        state: &RunProjection,
        native: &crate::job_guardian::protocol::AttemptIdentity,
    ) -> Result<Option<crate::orchestration::orchestration_records::AttemptReservation>, HostError>
    {
        let archive = self.attempt_archive(state)?.1;
        Ok(state
            .attempts
            .iter()
            .chain(&archive.attempts)
            .find(|a| {
                let id = &a.identity;
                id.project.as_str() == native.project
                    && id.run.0.as_str() == native.run
                    && id.task.0.as_str() == native.task
                    && id.attempt.0.as_str() == native.attempt
                    && id.nonce.as_str() == native.nonce
                    && id.owner_instance.as_str() == native.owner_instance
            })
            .cloned())
    }
    pub(super) fn retired_task_authority_exists(
        &self,
        state: &RunProjection,
        task: &TaskId,
    ) -> Result<bool, HostError> {
        Ok(self
            .attempt_archive(state)?
            .1
            .attempts
            .iter()
            .any(|a| a.identity.task == *task))
    }
    pub(super) fn retired_task_authority(
        &self,
        state: &RunProjection,
        task: &TaskId,
        attempt: &AttemptId,
    ) -> Result<bool, HostError> {
        let (_, archive) = self.attempt_archive(state)?;
        Ok(state.attempts.iter().chain(&archive.attempts).any(|a| {
            a.identity.task == *task
                && a.identity.attempt == *attempt
                && matches!(a.state, AttemptState::Retired(_))
        }))
    }
    #[cfg(target_os = "linux")]
    pub(super) fn archive_retired_attempts(
        &mut self,
        state: &mut RunProjection,
    ) -> Result<(), HostError> {
        if !state
            .attempts
            .iter()
            .any(|a| matches!(a.state, AttemptState::Retired(_)))
        {
            return Ok(());
        }
        let (previous, mut archive) = self.attempt_archive(state)?;
        archive.attempts.extend(
            state
                .attempts
                .iter()
                .filter(|a| matches!(a.state, AttemptState::Retired(_)))
                .cloned(),
        );
        let sealed = self
            .owner_mut()?
            .admit_scheduler_record(&OrchestrationRecord {
                version: 1,
                payload: OrchestrationPayload::AttemptArchive(Box::new(archive)),
            })?;
        state
            .usage
            .retain(|reference| Some(reference) != previous.as_ref());
        state.usage.push(sealed.reference);
        state
            .attempts
            .retain(|a| !matches!(a.state, AttemptState::Retired(_)));
        Ok(())
    }
    pub(super) fn restore_task_candidate(
        &mut self,
        history: &TaskHistoryRecord,
    ) -> Result<(), HostError> {
        if !self
            .owner()?
            .durable_sessions()
            .iter()
            .any(|(_, g)| g.branch_id() == &history.base.branch)
        {
            self.owner_mut()?.reopen_registered_candidate(
                history.base.branch.clone(),
                &history.candidate_base_ref,
            )?;
        }
        Ok(())
    }
    pub(super) fn persist_runtime_task(
        &mut self,
        state: &mut RunProjection,
        identity: &ExecutionIdentity,
        fence: bool,
    ) -> Result<(), HostError> {
        let Some(mut runtime) = self
            .hooks
            .runs
            .get_mut(&state.run)
            .and_then(|r| r.tasks.remove(&identity.task))
        else {
            return Ok(());
        };
        let result = (|| {
            if runtime.endpoint().authority().attempt != identity.attempt {
                if self.retired_task_authority(
                    state,
                    &identity.task,
                    &runtime.endpoint().authority().attempt,
                )? {
                    return Ok(());
                }
                return Err(refused("task authority changed"));
            }
            let brief = state
                .tasks
                .iter()
                .find(|t| t.id == identity.task)
                .ok_or_else(|| refused("task missing"))?
                .brief
                .clone();
            let previous = self.saved_task_history(state, &identity.task)?;
            let (source, base_ref, frozen) = runtime.persisted_history(self.owner_mut()?, fence)?;
            let mut candidate = self
                .owner()?
                .durable_sessions()
                .into_iter()
                .find(|(_, g)| g.branch_id() == &brief.scope.base.branch)
                .ok_or_else(|| refused("candidate actor missing"))?
                .1;
            let candidate_base_ref = self
                .owner_mut()?
                .persist_registered_task_checkpoint(&mut candidate)?;
            let history = TaskHistoryRecord {
                project: identity.project.clone(),
                run: state.run.clone(),
                task: identity.task.clone(),
                attempt: identity.attempt.clone(),
                base: brief.scope.base,
                source,
                base_ref,
                candidate_base_ref,
                frozen,
                merge_envelope: runtime.original_merge_envelope().cloned(),
            };
            if previous.as_ref().is_some_and(|old| {
                old.source.branch != history.source.branch
                    || old.base != history.base
                    || (old.frozen && !history.frozen)
                    || (old.merge_envelope.is_some()
                        && old.merge_envelope != history.merge_envelope)
            }) {
                return Err(refused("original task history or receipts changed"));
            }
            let sealed = self
                .owner_mut()?
                .admit_scheduler_record(&OrchestrationRecord {
                    version: 1,
                    payload: OrchestrationPayload::TaskHistory(Box::new(history)),
                })?;
            let decisions = state
                .tasks
                .iter_mut()
                .find(|t| t.id == identity.task)
                .ok_or_else(|| refused("task missing"))?
                .progress
                .decisions
                .clone();
            let mut keep = Vec::new();
            for reference in decisions {
                let history = reference.record_kind == RecordKind::RunIndex
                    && matches!(
                        self.owner()?
                            .store()?
                            .read_record::<OrchestrationRecord>(&reference)?
                            .payload,
                        OrchestrationPayload::TaskHistory(_)
                    );
                if !history {
                    keep.push(reference);
                }
            }
            keep.push(sealed.reference);
            state
                .tasks
                .iter_mut()
                .find(|t| t.id == identity.task)
                .ok_or_else(|| refused("task missing"))?
                .progress
                .decisions = keep;
            Ok(())
        })();
        self.hooks
            .runs
            .get_mut(&state.run)
            .ok_or_else(|| refused("run missing"))?
            .tasks
            .insert(identity.task.clone(), runtime);
        result
    }
    /// Drain edits and acknowledge the task checkpoint before recording turn exit.
    pub fn checkpoint_task_history(
        &mut self,
        producer: &HostAuth,
        run: &RunId,
        task: &TaskId,
    ) -> Result<TaskHistoryRecord, HostError> {
        let mut state = self.history_scope(producer, run)?;
        if state.outcome != RunOutcome::Pending {
            return Err(refused("run retired"));
        }
        let attempt = self
            .hooks
            .runs
            .get(run)
            .and_then(|r| r.tasks.get(task))
            .ok_or_else(|| refused("task actor missing"))?
            .endpoint()
            .authority()
            .attempt
            .clone();
        let identity = state
            .attempts
            .iter()
            .find(|a| a.identity.task == *task && a.identity.attempt == attempt)
            .ok_or_else(|| refused("task reservation missing"))?
            .identity
            .clone();
        self.persist_runtime_task(&mut state, &identity, true)?;
        self.publish_native(producer, state.clone(), &identity, vec![])?;
        self.saved_task_history(&state, task)?
            .ok_or_else(|| refused("task checkpoint missing"))
    }
    pub fn freeze_task_history(
        &mut self,
        producer: &HostAuth,
        run: &RunId,
        task: &TaskId,
    ) -> Result<TaskHistoryRecord, HostError> {
        let _state = self.history_scope(producer, run)?;
        let mut runtime = self
            .hooks
            .runs
            .get_mut(run)
            .and_then(|r| r.tasks.remove(task))
            .ok_or_else(|| refused("task actor missing"))?;
        let result = runtime.freeze(self.owner_mut()?);
        let result = result.map(|_| ()).map_err(HostError::from);
        self.hooks
            .runs
            .get_mut(run)
            .ok_or_else(|| refused("run missing"))?
            .tasks
            .insert(task.clone(), runtime);
        result?;
        self.checkpoint_task_history(producer, run, task)
    }
    pub fn merge_task_history(
        &mut self,
        producer: &HostAuth,
        run: &RunId,
        task: &TaskId,
        id: CommandId,
    ) -> Result<ReceiptLookup, HostError> {
        let state = self.history_scope(producer, run)?;
        if let Some(original) = self
            .saved_task_history(&state, task)?
            .and_then(|h| h.merge_envelope)
        {
            if original.id != id {
                return Err(refused("merge identity changed"));
            }
            let receipt = self.owner()?.manual_transition_receipt(&original)?;
            if receipt != ReceiptLookup::Absent {
                return Ok(receipt);
            }
        }
        self.freeze_task_history(producer, run, task)?;
        let mut runtime = self
            .hooks
            .runs
            .get_mut(run)
            .and_then(|r| r.tasks.remove(task))
            .ok_or_else(|| refused("task actor missing"))?;
        let result = runtime
            .prepare_merge(self.owner()?, id.clone())
            .map_err(HostError::from);
        self.hooks
            .runs
            .get_mut(run)
            .ok_or_else(|| refused("run missing"))?
            .tasks
            .insert(task.clone(), runtime);
        result?;
        // Persist the ORIGINAL merge envelope before attempting its side effect.
        self.checkpoint_task_history(producer, run, task)?;
        let mut runtime = self
            .hooks
            .runs
            .get_mut(run)
            .and_then(|r| r.tasks.remove(task))
            .ok_or_else(|| refused("task actor missing"))?;
        let result = runtime
            .merge(self.owner_mut()?, id)
            .map_err(HostError::from);
        self.hooks
            .runs
            .get_mut(run)
            .ok_or_else(|| refused("run missing"))?
            .tasks
            .insert(task.clone(), runtime);
        let receipt = result?;
        self.checkpoint_task_history(producer, run, task)?;
        Ok(receipt)
    }
}
