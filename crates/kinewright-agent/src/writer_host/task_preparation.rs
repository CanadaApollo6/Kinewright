//! Preparation chooses an accepted run-owned base; assignment receipts stay immutable.
use super::{HostCommandOwner, HostError, HostRefusal};
use crate::orchestration::orchestration_records::{BranchBinding, Role, RunProjection, TaskScope};
#[cfg(target_os = "linux")]
use crate::orchestration::orchestration_records::{
    EffectId, EffectRecord, EffectSafety, EffectState, ExecutionIdentity, ITEM_LIMIT, RunOutcome,
    RunPhase, RunWait, TaskState, WaitDetail,
};
use kinewright_project::{
    bundle_records::BranchCheckpoint,
    receipts::{ReceiptLookup, ReceiptResult},
};
fn refused() -> HostError {
    HostRefusal::new(
        "task_scope",
        "candidate is not descended from this run's admitted base",
    )
    .into()
}
impl HostCommandOwner {
    /// Descent is proved by this run's acknowledged merge receipts, never by a
    /// model-supplied head or a same-project branch name alone.
    pub(super) fn run_candidate_bases(
        &self,
        state: &RunProjection,
    ) -> Result<Vec<BranchBinding>, HostError> {
        let root = state
            .tasks
            .iter()
            .find(|t| t.parent.is_none() && t.brief.role == Role::Director)
            .ok_or_else(refused)?;
        let origin = &root.brief.scope.base;
        for run in self.owner()?.orchestration_runs()? {
            if run != state.run
                && self
                    .owner()?
                    .orchestration_projection(&run)?
                    .is_some_and(|other| {
                        other.tasks.iter().any(|t| {
                            t.parent.is_none() && t.brief.scope.base.branch == origin.branch
                        })
                    })
            {
                return Err(refused());
            }
        }
        let mut edges = Vec::new();
        for task in &state.tasks {
            let Some(history) = self.saved_task_history(state, &task.id)? else {
                continue;
            };
            let Some(envelope) = &history.merge_envelope else {
                continue;
            };
            if history.base.branch != origin.branch
                || envelope.target_branch != origin.branch
                || envelope.expected_head.as_ref() != Some(&history.base.head)
            {
                return Err(refused());
            }
            let (ReceiptLookup::Committed(receipt) | ReceiptLookup::Retired(receipt)) =
                self.owner()?.manual_transition_receipt(envelope)?
            else {
                continue;
            };
            let ReceiptResult::Edit { checkpoint_ref, .. } = &receipt.result else {
                return Err(refused());
            };
            let checkpoint = self
                .owner()?
                .store()?
                .read_record::<BranchCheckpoint>(checkpoint_ref)?;
            if receipt.target_branch != origin.branch
                || checkpoint.branch_id != origin.branch
                || checkpoint.head_id != receipt.head_id
                || checkpoint.predecessor.as_ref() != Some(&history.base.head)
            {
                return Err(refused());
            }
            edges.push((
                history.base.clone(),
                BranchBinding {
                    branch: checkpoint.branch_id.clone(),
                    head: checkpoint.head_id.clone(),
                    checkpoint: checkpoint_ref.clone(),
                    document_hash: checkpoint.document_hash.clone(),
                },
            ));
        }
        let mut bases = vec![origin.clone()];
        for _ in 0..edges.len() {
            let next = edges
                .iter()
                .filter(|(base, _)| Some(base) == bases.last())
                .map(|(_, next)| next)
                .collect::<Vec<_>>();
            if next.is_empty() {
                break;
            }
            if next.iter().any(|base| *base != next[0]) || bases.contains(next[0]) {
                return Err(refused());
            }
            bases.push(next[0].clone());
        }
        Ok(bases)
    }
    pub(super) fn preparation_base(
        &self,
        state: &RunProjection,
    ) -> Result<BranchBinding, HostError> {
        let bases = self.run_candidate_bases(state)?;
        let current = self.owner()?.candidate_binding(&bases[0].branch)?;
        if bases.last() != Some(&current) {
            return Err(refused());
        }
        Ok(current)
    }
    pub(super) fn prepared_child_scope(
        &self,
        state: &RunProjection,
        parent: &TaskScope,
        child: &TaskScope,
    ) -> bool {
        if super::orchestration::scope_contains(parent, child) {
            return true;
        }
        let mut limits = child.clone();
        limits.base = parent.base.clone();
        super::orchestration::scope_contains(parent, &limits)
            && self
                .run_candidate_bases(state)
                .is_ok_and(|bases| bases.contains(&child.base))
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct PreparationRefusal(pub &'static str);
#[cfg(target_os = "linux")]
pub(crate) fn preparation_refusal(error: HostError) -> std::io::Error {
    let words = match error {
        HostError::Refused(refusal) if refusal.code == "history_changed" => {
            "The task could not start because its candidate changed. This start sent nothing. The Director must replan the task; Continue cannot repair it."
        }
        HostError::Refused(refusal)
            if matches!(
                refusal.code.as_str(),
                "task_scope" | "task_history" | "orchestration_refused" | "role_refused"
            ) =>
        {
            "The task could not start because its scope is invalid. This start sent nothing. The Director must replan the task; Continue cannot repair it."
        }
        _ => {
            "The writer host could not prepare the task. This start sent nothing. The Director must replan the task; Continue cannot repair it."
        }
    };
    std::io::Error::other(PreparationRefusal(words))
}
#[cfg(target_os = "linux")]
impl HostCommandOwner {
    pub(crate) fn preparation_failed(
        &mut self,
        producer: &super::HostAuth,
        identity: &ExecutionIdentity,
        reason: &str,
    ) -> Result<(), HostError> {
        let mut state = self.spending_scope(producer, &identity.run)?;
        if state.outcome != RunOutcome::Pending {
            return Ok(());
        }
        if state.effects.len() >= ITEM_LIMIT {
            return Err(HostRefusal::new("task_preparation", "notification capacity").into());
        }
        let parent = state
            .tasks
            .iter()
            .find(|t| t.id == identity.task)
            .ok_or_else(refused)?
            .parent
            .clone();
        let reason = if parent.is_none() {
            "The writer host could not prepare the Director. This start sent nothing. Stop this run and start a new one after reviewing the task. Continue cannot repair it."
        } else {
            reason
        };
        let reference = self.owner_mut()?.admit_host_grant(reason.as_bytes())?;
        for effect in state
            .effects
            .iter_mut()
            .filter(|e| e.task == identity.task && e.state == EffectState::Pending)
        {
            effect.state = EffectState::Disposed;
        }
        let child = state
            .tasks
            .iter_mut()
            .find(|t| t.id == identity.task)
            .ok_or_else(refused)?;
        child.state = TaskState::Cancelled;
        child.progress.decisions.push(reference.clone());
        let index = self.resolve_attachment(Some(&producer.bearer()))?;
        let issued = self.issue_metadata(index, None)?;
        state.wait = RunWait::Replan(WaitDetail {
            reason: reference.clone(),
            missing: vec![],
            attempt: Some(identity.attempt.clone()),
        });
        if let Some(parent) = parent {
            let parent = state
                .tasks
                .iter_mut()
                .find(|t| t.id == parent)
                .ok_or_else(refused)?;
            parent.progress.decisions.push(reference.clone());
            let task = parent.id.clone();
            let lane = parent.brief.scope.base.branch.clone();
            let order = state
                .effects
                .iter()
                .filter(|e| e.lane == lane)
                .map(|e| e.order)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or_else(refused)?;
            state.effects.push(EffectRecord {
                id: EffectId(serde_json::from_value(serde_json::json!(
                    &kinewright_media::sha256_bytes(&serde_json::to_vec(&(
                        "preparation-replan",
                        &issued.envelope.id
                    ))?)[..32]
                ))?),
                task,
                lane,
                order,
                safety: EffectSafety::ProcessBound,
                state: EffectState::Pending,
                payload: reference.clone(),
            });
            if matches!(state.phase, RunPhase::Queued | RunPhase::Running) {
                state.phase = RunPhase::Running;
            }
        } else {
            state.phase = RunPhase::Interrupted;
        }
        crate::orchestration::tasks::refresh_cleanup(&mut state);
        self.publish_native_issued(producer, state, identity, vec![reference], issued)?;
        Ok(())
    }
}
