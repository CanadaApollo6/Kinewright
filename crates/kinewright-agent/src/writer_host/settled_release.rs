//! Settled runs keep their journal identities, not dormant actors.
use super::{BundleError, HostCommandOwner};
use crate::orchestration::orchestration_records::{AttemptState, RunCleanup, RunId, RunOutcome};
use std::collections::BTreeSet;

pub(super) fn reopen_words(task: &str) -> String {
    format!(
        "Task {task}'s saved result could not be reopened. The project's other work is unaffected."
    )
}

impl HostCommandOwner {
    pub(super) fn sweep_settled_run_actors(&mut self) {
        if self.hooks.pending.is_some() || self.recovering || self.switch_frozen {
            return;
        }
        if self
            .hooks
            .runs
            .values()
            .all(|run| run.tasks.is_empty() && run.settled_candidates.is_empty())
        {
            return;
        }
        let Some(owner) = self.owner.as_mut() else {
            return;
        };
        // Retain only branch identities from the temporary projections, never
        // their actors or full decoded run records.
        let mut active_candidates = BTreeSet::new();
        for run in self.hooks.runs.keys() {
            let Ok(Some(state)) = owner.orchestration_projection(run) else {
                return;
            };
            if state.outcome == RunOutcome::Pending || state.cleanup != RunCleanup::Settled {
                active_candidates.extend(
                    state
                        .tasks
                        .into_iter()
                        .map(|task| task.brief.scope.base.branch),
                );
            }
        }
        for (run, runtime) in &mut self.hooks.runs {
            if runtime.tasks.is_empty() && runtime.settled_candidates.is_empty() {
                continue;
            }
            let Ok(Some(state)) = owner.orchestration_projection(run) else {
                continue;
            };
            if state.outcome == RunOutcome::Pending
                || state.cleanup != RunCleanup::Settled
                || state
                    .attempts
                    .iter()
                    .any(|attempt| !matches!(attempt.state, AttemptState::Retired(_)))
            {
                continue;
            }
            runtime.settled_candidates.extend(
                state
                    .tasks
                    .into_iter()
                    .map(|task| task.brief.scope.base.branch)
                    .filter(|branch| owner.is_registered_run_candidate(branch)),
            );
            let super::orchestration::HostRun {
                tasks,
                media_reads,
                release_notice,
                settled_candidates,
                ..
            } = runtime;
            tasks.retain(|id, actor| match actor.release_checkpointed(owner) {
                Ok(true) => {
                    media_reads.remove(id);
                    false
                }
                Err(BundleError::Corrupt(reason)) => {
                    release_notice.get_or_insert_with(|| {
                        format!(
                            "{} Its live work is kept in this session. {reason}",
                            reopen_words(id.0.as_str()),
                        )
                    });
                    true
                }
                _ => true,
            });
            settled_candidates.retain(|branch| {
                if active_candidates.contains(branch) {
                    return true;
                }
                match owner.release_checkpointed_run_candidate(branch) {
                    Ok(true) => false,
                    Err(BundleError::Corrupt(reason)) => {
                        release_notice.get_or_insert_with(|| format!(
                            "Candidate {} could not be reopened. Its live work is kept in this session. The project's other work is unaffected. {reason}",
                            branch.as_str(),
                        ));
                        true
                    }
                    _ => true,
                }
            });
        }
    }

    pub(super) fn record_release_notice(&mut self, run: &RunId, message: String) {
        if let Some(runtime) = self.hooks.runs.get_mut(run) {
            runtime.release_notice.get_or_insert(message);
        }
    }

    pub(super) fn task_reopen_error(
        &mut self,
        run: &RunId,
        task: &crate::orchestration::orchestration_records::TaskId,
        error: super::HostError,
    ) -> super::HostError {
        if let super::HostError::Refused(ref reason) = error
            && reason.code == "damaged_bundle"
        {
            let words = reopen_words(task.0.as_str());
            if reason.message.contains(&words) {
                return error;
            }
            let message = format!("{words} {}", reason.message);
            self.record_release_notice(run, message.clone());
            return BundleError::Corrupt(message).into();
        }
        error
    }
}
