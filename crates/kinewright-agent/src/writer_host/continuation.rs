//! Intentional GUI detach retains the host's certified input closure and Producer.
use super::{
    AttachmentRole, HostCommandOwner, HostError, HostRefusal,
    orchestration::{HostRun, ProducerScope},
};
use crate::orchestration::orchestration_records::{RunId, TaskBrief};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

/// Constructed only from the host owner's admitted and charged frozen context.
/// Neither the wire nor a GUI callback can construct this value.
pub struct HostOwnedInputClosure {
    pub(super) context: Arc<kinewright_project::studio_store::FrozenContext>,
    pub(super) inputs: Arc<kinewright_media::owned_io::PlaybackInputs>,
}
impl HostOwnedInputClosure {
    fn ready(&self) -> Result<(), HostError> {
        let _verified = self.context.playback_inputs()?;
        if self.inputs.document() != &self.context.document {
            return Err(
                HostRefusal::new("continuation_inputs", "retained document changed").into(),
            );
        }
        self.context
            .verify_retained_inputs()
            .map_err(|error| HostRefusal::new("continuation_inputs", &error.to_string()))?;
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetachOutcome {
    pub retained_runs: Vec<RunId>,
    /// JA-1 S6 C3: an active review job or a running sealed save is retained.
    #[serde(default)]
    pub retained_review: bool,
}

impl HostCommandOwner {
    pub(super) fn retain_run(&mut self, run: &RunId, brief: &TaskBrief) -> Result<(), HostError> {
        if self.hooks.runs.contains_key(run) {
            return Ok(());
        }
        if self.hooks.runs.len() >= 64 {
            return Err(HostRefusal::new("capacity", "64 retained runs maximum").into());
        }
        self.owner()?
            .validate_orchestration_scope(&self.owner()?.active_manual_project()?, brief)?;
        let descriptor = self.run_input_descriptor(&brief.scope.base.checkpoint)?;
        let prepared = if let Some(prepared) = self.hooks.prepared_run_inputs.take() {
            prepared
        } else {
            let manifest = self.owner()?.store()?.read_input_manifest(&brief.inputs)?;
            if !manifest.entries.is_empty() {
                return Err(HostRefusal::new(
                    "continuation_inputs",
                    "Run inputs are not ready. Try again from the project window.",
                )
                .into());
            }
            self.prepare_run_inputs(&brief.scope.base.checkpoint)?
                .open()?
        };
        prepared.recheck(&descriptor, self.owner()?.store()?.owner_epoch())?;
        let HostOwnedInputClosure { context, inputs } = prepared;
        let producer = self.issue_attachment(
            format!("host-run:{}", run.0.as_str()),
            AttachmentRole::Producer,
            self.config.roots.clone(),
        )?;
        let index = self.resolve_attachment(Some(&producer.bearer()))?;
        self.attachments[index].producer_scope = Some(ProducerScope {
            run: run.clone(),
            task: None,
            attempt: None,
            endpoint: None,
            owner_epoch: self.owner()?.store()?.owner_epoch(),
            expires_at: None,
            scope: brief.scope.clone(),
        });
        let continue_floor = self
            .owner()?
            .orchestration_projection(run)?
            .map_or(0, |state| state.sequence);
        let mut settled_candidates = std::collections::BTreeSet::new();
        if self
            .owner()?
            .is_registered_run_candidate(&brief.scope.base.branch)
        {
            settled_candidates.insert(brief.scope.base.branch.clone());
        }
        self.hooks.runs.insert(
            run.clone(),
            HostRun {
                recovery_blocks: vec![],
                #[cfg(target_os = "linux")]
                measurements: BTreeMap::new(),
                producer,
                inputs: HostOwnedInputClosure { context, inputs },
                tasks: BTreeMap::new(),
                settled_candidates,
                release_notice: None,
                media_reads: BTreeMap::new(),
                continuable: true,
                last_continue: Some(continue_floor),
                #[cfg(target_os = "linux")]
                pending_provider_wait: None,
            },
        );
        Ok(())
    }
    pub(super) fn detach_for_continuation(
        &mut self,
        index: usize,
    ) -> Result<DetachOutcome, HostError> {
        if self.attachments[index].role != AttachmentRole::Editor
            || self.recovering
            || self.switch_frozen
            || self.hooks.pending.is_some()
            || self.prepared_owned_media.is_some()
        {
            return Err(HostRefusal::new(
                "continuation_refused",
                "the attachment remains paused until its current work is settled",
            )
            .into());
        }
        let mut retained_runs = vec![];
        for (id, run) in &self.hooks.runs {
            if !run.continuable {
                continue;
            }
            run.inputs.ready()?;
            let producer = self.resolve_attachment(Some(&run.producer.bearer()))?;
            if self.attachments[producer].role != AttachmentRole::Producer
                || self.attachments[producer]
                    .producer_scope
                    .as_ref()
                    .is_none_or(ProducerScope::expired)
            {
                return Err(HostRefusal::new(
                    "continuation_refused",
                    "independent host Producer is not live",
                )
                .into());
            }
            retained_runs.push(id.clone());
        }
        // Review jobs exist only on Linux; elsewhere a review never retains.
        #[cfg(target_os = "linux")]
        let retained_review = self.review_retained();
        #[cfg(not(target_os = "linux"))]
        let retained_review = false;
        if retained_runs.is_empty() && !retained_review {
            return Err(HostRefusal::new(
                "continuation_refused",
                "no independently retained run or review",
            )
            .into());
        }
        self.remote = None;
        self.detached = true;
        Ok(DetachOutcome {
            retained_runs,
            retained_review,
        })
    }
    /// An active review job or a running sealed save keeps the host alive.
    #[cfg(target_os = "linux")]
    fn review_retained(&self) -> bool {
        self.review_active().unwrap_or(true)
            || matches!(
                self.review_runtime.sealed_save_status(),
                Some(kinewright_project::review_job::delivery::SealedSaveStatus::Saving { .. })
            )
    }
    /// JA-1 S6 C3: a retained run still has work in flight: a live attempt,
    /// a task whose first attempt has not launched yet, unsettled cleanup, a
    /// provider wait or an edit awaiting application. A Pending run whose
    /// attempts all retired and whose cleanup settled is durable; like a
    /// Paused review it waits for the next open. A read error counts as work.
    pub(super) fn retained_run_working(&self) -> bool {
        use crate::orchestration::orchestration_records::{AttemptState, RunCleanup, RunOutcome};
        if !self.hooks.pending_edit_applications.is_empty() {
            return true;
        }
        self.hooks.runs.iter().any(|(id, run)| {
            #[cfg(target_os = "linux")]
            if run.continuable && run.pending_provider_wait.is_some() {
                return true;
            }
            run.continuable
                && self
                    .owner()
                    .ok()
                    .and_then(|owner| owner.orchestration_projection(id).ok().flatten())
                    .is_none_or(|projection| {
                        projection.cleanup != RunCleanup::Settled
                            || (projection.outcome == RunOutcome::Pending
                                && (projection.attempts.iter().any(|attempt| {
                                    !matches!(attempt.state, AttemptState::Retired(_))
                                }) || run_launch_pending(&projection)))
                    })
        })
    }
    /// JA-1 S6 C3: work a detached host must finish before it exits.
    fn detached_work_remains(&self) -> bool {
        let retained = self.retained_run_working()
            || self.hooks.pending.is_some()
            || self.recovering
            || self.switch_frozen
            || self.prepared_owned_media.is_some();
        #[cfg(target_os = "linux")]
        let retained = retained || self.review_work_remains();
        retained
    }
    /// JA-1 S6 C3: called wherever detached work ends (seal, pause, stop
    /// retirement, run retirement, save end). Once the GUI explicitly
    /// detached and nothing remains, settle the root, then fence exactly as
    /// `Shutdown` does; `WriterHost::wait` finishes the exit and the project
    /// lock goes with the owner. An attached GUI is never idle.
    pub(super) fn exit_when_detached_work_ends(&mut self) {
        let attached = self
            .attachments
            .iter()
            .any(|attachment| attachment.role == AttachmentRole::Editor);
        if !self.detached || attached || self.lifecycle.stopping() || self.detached_work_remains() {
            return;
        }
        #[cfg(target_os = "linux")]
        if self.settle_root_for_exit().is_err() {
            return;
        }
        self.begin_shutdown();
        self.lifecycle.request_detached_exit();
    }
    /// JA-1 S6 C3: a detached host has no GUI request to retire a finished
    /// encode worker or persist a Stop, so `WriterHost::wait` and every
    /// `/host` request run this step, then the exit check.
    pub(super) fn poll_detached(host: &std::sync::Mutex<Self>) {
        let Ok(mut owner) = host.lock() else {
            return;
        };
        if !owner.detached || owner.lifecycle.stopping() {
            return;
        }
        #[cfg(target_os = "linux")]
        if !owner.review_frozen() && !owner.settlement_owned_elsewhere() {
            // Stop retirement: a finished worker's Stopping job retires here.
            let _ = owner.retire_review_worker();
            let _ = owner.persist_pending_stops();
        }
        owner.exit_when_detached_work_ends();
    }
    #[cfg(target_os = "linux")]
    fn settle_root_for_exit(&mut self) -> Result<(), HostError> {
        if self.settlement_owned_elsewhere() {
            return Err(kinewright_project::bundle::BundleError::Unsettled.into());
        }
        let Some(session) = self.owner.as_mut() else {
            return Ok(());
        };
        let Ok(project) = session.project_id() else {
            return Ok(());
        };
        session
            .review_source_bundle_mut(&project)?
            .settle_review_root()?;
        Ok(())
    }
    /// Only the trusted issuer marks an attachment as the person-facing holder.
    pub fn issue_person_facing_editor(
        &mut self,
        subject: String,
        roots: super::SessionRoots,
    ) -> Result<super::HostAuth, HostError> {
        let auth = self.issue_attachment(subject, AttachmentRole::Editor, roots)?;
        let index = self.resolve_attachment(Some(&auth.bearer()))?;
        self.attachments[index].person_facing = true;
        Ok(auth)
    }
}

/// JA-1 S6 C3: an active run has an open task that never had an attempt, so
/// its first launch is still to come. A task requeued after an interrupted
/// attempt is not: it waits for the next open, as a Paused run does.
fn run_launch_pending(
    projection: &crate::orchestration::orchestration_records::RunProjection,
) -> bool {
    use crate::orchestration::orchestration_records::{RunPhase, TaskState};
    matches!(
        projection.phase,
        RunPhase::Queued | RunPhase::Running | RunPhase::Reviewing
    ) && projection.tasks.iter().any(|task| {
        matches!(task.state, TaskState::Queued | TaskState::Preparing)
            && !projection
                .attempts
                .iter()
                .any(|attempt| attempt.identity.task == task.id)
    })
}
