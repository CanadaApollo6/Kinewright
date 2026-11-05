//! Person-owned recovery views derived from retained task records, including stopped runs.
use super::{BundleError, Ja1EditingSession};
use crate::{
    bundle_recovery::RecoverySelection,
    orchestration_commit::CommandDecision,
    orchestration_records::{
        BranchBinding, CompletionStep, OrchestrationPayload, RunCleanup, RunId, RunOutcome, TaskId,
    },
    receipts::ReceiptLookup,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetainedWorkKind {
    Task,
    Completion,
    Merge,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent retained-record, cleanup and timeline facts"
)]
pub struct RetainedTaskWork {
    pub run: RunId,
    pub task: TaskId,
    pub kind: RetainedWorkKind,
    pub source: BranchBinding,
    pub frozen: bool,
    pub stopped: bool,
    pub cleanup_settled: bool,
    pub available: bool,
    pub open_on_timeline: bool,
    pub disclosure: String,
}
impl Ja1EditingSession {
    pub fn retained_task_work(&self) -> Result<Vec<RetainedTaskWork>, BundleError> {
        Ok(self
            .retained_task_sources()?
            .into_iter()
            .filter(|work| work.stopped || work.frozen || work.kind != RetainedWorkKind::Task)
            .collect())
    }
    #[allow(
        clippy::too_many_lines,
        reason = "derive one bounded source view from authenticated task histories"
    )]
    fn retained_task_sources(&self) -> Result<Vec<RetainedTaskWork>, BundleError> {
        let _reading = crate::run_admission::reading(self.store()?);
        let bundle = self.active_bundle()?;
        let index = bundle.run_index()?;
        let cuts = index
            .orchestration
            .iter()
            .filter_map(|(run, cuts)| cuts.last().map(|cut| (run, &cut.snapshot)))
            .collect::<Vec<_>>();
        let key = kinewright_media::sha256_bytes(
            &serde_json::to_vec(&(
                bundle.store.owner_epoch(),
                bundle
                    .root
                    .as_ref()
                    .map(|root| (&root.branch_index_ref, &root.manual_branch_id)),
                &cuts,
            ))
            .map_err(|e| BundleError::Corrupt(e.to_string()))?,
        );
        if let Some((previous, work)) = &*self.retained_task_work_cache.borrow()
            && previous == &key
        {
            return Ok(work.clone());
        }
        let mut work = Vec::new();
        for (run, _) in cuts {
            let Some(state) = self.orchestration_projection(run)? else {
                continue;
            };
            for task in &state.tasks {
                let mut history = None;
                for reference in task
                    .progress
                    .decisions
                    .iter()
                    .rev()
                    .filter(|r| r.record_kind == crate::bundle_records::RecordKind::RunIndex)
                {
                    let record = self.store()?.read_run_record(reference)?;
                    if let OrchestrationPayload::TaskHistory(saved) = &record.payload {
                        if saved.project != self.project_id()?
                            || saved.run != *run
                            || saved.task != task.id
                            || saved.base != task.brief.scope.base
                        {
                            return Err(BundleError::Identity(
                                "retained task history scope changed".into(),
                            ));
                        }
                        history = Some((**saved).clone());
                        break;
                    }
                }
                let Some(history) = history else {
                    continue;
                };
                let incomplete = if let Some(intent) = task.progress.completion.as_deref() {
                    let id = intent
                        .step_id(CompletionStep::Publication)
                        .map_err(|_| BundleError::HistoryChanged)?;
                    self.orchestration_receipt(run, &id)?
                        .is_none_or(|r| r.decision != CommandDecision::Applied)
                } else {
                    false
                };
                let merge_pending = history
                    .merge_envelope
                    .as_ref()
                    .map(|envelope| self.manual_transition_receipt(envelope))
                    .transpose()?
                    .is_some_and(|receipt| {
                        !matches!(
                            receipt,
                            ReceiptLookup::Committed(_) | ReceiptLookup::Retired(_)
                        )
                    });
                let kind = if incomplete {
                    RetainedWorkKind::Completion
                } else if merge_pending {
                    RetainedWorkKind::Merge
                } else {
                    RetainedWorkKind::Task
                };
                let source = history.source;
                let available = bundle
                    .branches
                    .get(&source.branch)
                    .is_some_and(|checkpoint| {
                        checkpoint.head_id == source.head
                            && checkpoint.document_hash == source.document_hash
                    });
                let manual = bundle.checkpoint(bundle.manual_branch_id())?;
                let open_on_timeline = bundle.manual_branch_id() == &source.branch
                    || bundle
                        .branches
                        .get(&source.branch)
                        .is_some_and(|checkpoint| checkpoint.history_ref == manual.history_ref);
                let stopped = state.outcome != RunOutcome::Pending;
                let cleanup_settled = state.cleanup == RunCleanup::Settled;
                let disclosure = if open_on_timeline {
                    format!(
                        "Task {} work is open on your timeline. Review it here, or keep this working branch for later.",
                        task.id.0.as_str()
                    )
                } else if !available {
                    format!(
                        "Task {} retained work is bound to working branch {} at head {}. That branch has changed or was removed. Keep the original task history for review; this selection cannot replace it with another version.",
                        task.id.0.as_str(),
                        source.branch.as_str(),
                        source.head.as_str()
                    )
                } else if !stopped || !cleanup_settled {
                    format!(
                        "Task {} work is kept on working branch {}. Stop this run and wait for cleanup. Then choose Save current edit, then open {} to review it on your timeline. Your current edit is saved first.",
                        task.id.0.as_str(),
                        source.branch.as_str(),
                        source.branch.as_str()
                    )
                } else {
                    format!(
                        "Task {} work is kept on working branch {}. Choose Save current edit, then open {} to review it on your timeline. Your current edit is saved first. You can also leave this branch for later review.",
                        task.id.0.as_str(),
                        source.branch.as_str(),
                        source.branch.as_str()
                    )
                };
                work.push(RetainedTaskWork {
                    run: run.clone(),
                    task: task.id.clone(),
                    kind,
                    source,
                    frozen: history.frozen,
                    stopped,
                    cleanup_settled,
                    available,
                    open_on_timeline,
                    disclosure,
                });
            }
        }
        *self.retained_task_work_cache.borrow_mut() = Some((key, work.clone()));
        Ok(work)
    }
    pub(super) fn authorize_retained_selection(
        &mut self,
        target: &RecoverySelection,
    ) -> Result<bool, BundleError> {
        let branch = match target {
            RecoverySelection::WorkingBranch(branch)
            | RecoverySelection::Checkpoint { branch, .. } => branch,
        };
        let retained = self
            .retained_task_sources()?
            .into_iter()
            .filter(|work| &work.source.branch == branch)
            .collect::<Vec<_>>();
        if retained.is_empty() {
            return Ok(false);
        }
        if retained
            .iter()
            .any(|work| !work.stopped || !work.cleanup_settled)
        {
            return Err(BundleError::Identity(
                "Stop the run and wait for cleanup before reviewing retained task work.".into(),
            ));
        }
        if retained.iter().any(|work| !work.available) {
            return Err(BundleError::HistoryChanged);
        }
        // Close only the already-acknowledged task registration. This preserves
        // its checkpoint and refuses any unsaved history or current-owner mismatch.
        let registrations = self
            .durable_sessions()
            .into_iter()
            .filter(|(_, guard)| guard.branch_id() == branch)
            .map(|(_, guard)| guard)
            .collect::<Vec<_>>();
        for guard in registrations {
            self.close_registered_task_checkpoint(&guard)?;
        }
        Ok(matches!(target, RecoverySelection::WorkingBranch(_)))
    }
}
