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
pub(super) fn validate_merge_checkpoint(
    origin: &BranchBinding,
    base: &BranchBinding,
    receipt: &kinewright_project::receipts::CommandReceipt,
    checkpoint: &BranchCheckpoint,
) -> Result<(), HostError> {
    if receipt.target_branch != origin.branch
        || checkpoint.branch_id != origin.branch
        || checkpoint.head_id != receipt.head_id
        || checkpoint.predecessor.as_ref() != Some(&base.head)
    {
        return Err(refused());
    }
    Ok(())
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
            validate_merge_checkpoint(origin, &history.base, &receipt, &checkpoint)?;
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
pub(crate) const HOST_RETRY: &str =
    "The writer host could not start the task yet. This start sent nothing. Continue to try again.";
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum PreparationKind {
    History,
    Scope,
    Paused,
    Retry,
    FrozenCheckpoint,
    MissingHistory,
    LegacyPreparation,
    HistoryBase,
}
#[cfg(target_os = "linux")]
impl PreparationKind {
    pub(crate) fn deterministic(self) -> bool {
        matches!(self, Self::History | Self::Scope)
    }
    pub(crate) fn permanent(self) -> bool {
        matches!(
            self,
            Self::FrozenCheckpoint
                | Self::MissingHistory
                | Self::LegacyPreparation
                | Self::HistoryBase
        )
    }
    pub(crate) fn words(self) -> &'static str {
        match self {
            Self::FrozenCheckpoint => super::task_preparation_words::FROZEN_RECOVERY,
            Self::MissingHistory => super::task_preparation_words::MISSING_RECOVERY,
            Self::LegacyPreparation => super::task_preparation_words::LEGACY_RECOVERY,
            Self::HistoryBase => super::task_preparation_words::BASE_RECOVERY,
            Self::History => {
                "The task could not start because its candidate changed. This start sent nothing. The Director must replan the task; Continue cannot repair it."
            }
            Self::Scope => {
                "The task could not start because its scope is invalid. This start sent nothing. The Director must replan the task; Continue cannot repair it."
            }
            Self::Paused | Self::Retry => {
                "The writer host could not start the task yet. This start sent nothing. Continue to try again."
            }
        }
    }
}
#[cfg(target_os = "linux")]
#[derive(Debug, thiserror::Error)]
#[error("{source}")]
pub(crate) struct PreparationRefusal {
    pub(crate) kind: PreparationKind,
    source: HostError,
}
#[cfg(target_os = "linux")]
pub(crate) fn preparation_refusal(error: HostError) -> std::io::Error {
    let kind = match &error {
        HostError::Refused(refusal) => match refusal.code.as_str() {
            "history_changed" => PreparationKind::History,
            "task_preparation_frozen" => PreparationKind::FrozenCheckpoint,
            "task_preparation_missing_history" => PreparationKind::MissingHistory,
            "task_preparation_legacy" => PreparationKind::LegacyPreparation,
            "task_preparation_history_base" => PreparationKind::HistoryBase,
            "task_scope" | "task_preparation_scope" | "task_preparation_inputs" => {
                PreparationKind::Scope
            }
            "task_preparation_paused" => PreparationKind::Paused,
            _ => PreparationKind::Retry,
        },
        _ => PreparationKind::Retry,
    };
    std::io::Error::other(PreparationRefusal {
        kind,
        source: error,
    })
}
#[cfg(target_os = "linux")]
pub(crate) use super::task_preparation_words::{DIRECTOR_RECOVERY, SENT_TASK_RECOVERY};
#[cfg(target_os = "linux")]
impl HostCommandOwner {
    pub(super) fn preparation_block(
        &self,
        state: &RunProjection,
    ) -> Result<Option<PreparationKind>, HostError> {
        let archive = self.attempt_archive(state)?.1;
        for attempt in state.attempts.iter().chain(&archive.attempts) {
            let bytes = self
                .owner()?
                .store()?
                .read_evidence(&attempt.recovery_ref)?;
            if let Ok(intent) =
                serde_json::from_slice::<super::native_invocation::NativeLaunchIntent>(&bytes)
            {
                if intent.identity != attempt.identity {
                    return Err(refused());
                }
                if let Some(kind) = intent
                    .permanent_preparation_failure
                    .filter(|kind| kind.permanent())
                {
                    return Ok(Some(kind));
                }
            }
        }
        Ok(None)
    }
    pub(super) fn task_has_sent(
        &self,
        state: &RunProjection,
        id: &crate::orchestration::orchestration_records::TaskId,
    ) -> Result<bool, HostError> {
        Ok(self.native_accounting(state)?.is_some_and(|accounting| {
            accounting.tasks.iter().any(|task| {
                task.task == id.0
                    && task.sends.iter().any(|send| {
                        !task.usage.iter().any(|sample| {
                            sample.attempt == send.attempt
                                && sample.turn == send.turn
                                && sample.event == "host.not_dispatched"
                        })
                    })
            })
        }))
    }
    pub(super) fn unprepared_retry_safe(
        &self,
        state: &RunProjection,
        id: &crate::orchestration::orchestration_records::TaskId,
        attempt: &crate::orchestration::orchestration_records::AttemptId,
    ) -> Result<bool, HostError> {
        if self.task_has_sent(state, id)? {
            return Ok(false);
        }
        let mut prior = state.attempts.clone();
        prior.extend(self.attempt_archive(state)?.1.attempts);
        for entry in prior
            .iter()
            .filter(|entry| &entry.identity.task == id && &entry.identity.attempt != attempt)
        {
            if !matches!(
                entry.state,
                crate::orchestration::orchestration_records::AttemptState::Retired(_)
            ) {
                return Err(HostRefusal::new(
                    "task_preparation_busy",
                    "previous task authority is not retired",
                )
                .into());
            }
            let intent = self.native_launch_intent(state, &entry.identity)?;
            if intent
                .as_ref()
                .is_none_or(|intent| intent.prepared.is_none())
            {
                return Err(HostRefusal::new(
                    "task_preparation_legacy",
                    "previous task preparation cannot be verified",
                )
                .into());
            }
            if !intent.is_some_and(|intent| {
                intent.prepared == Some(false) && !intent.dispatched && intent.native.is_none()
            }) {
                return Ok(false);
            }
        }
        Ok(true)
    }
    pub(crate) fn preparation_failed(
        &mut self,
        producer: &super::HostAuth,
        identity: &ExecutionIdentity,
        kind: PreparationKind,
    ) -> Result<(), HostError> {
        let mut state = self.spending_scope(producer, &identity.run)?;
        if state.outcome != RunOutcome::Pending {
            return Ok(());
        }
        let task = state
            .tasks
            .iter()
            .find(|t| t.id == identity.task)
            .ok_or_else(refused)?;
        let parent = task.parent.clone();
        let sent = self.task_has_sent(&state, &identity.task)?;
        if kind.permanent() {
            let mut intent = self
                .native_launch_intent(&state, identity)?
                .ok_or_else(refused)?;
            intent.permanent_preparation_failure = Some(kind);
            let reference = self
                .owner_mut()?
                .admit_host_grant(&serde_json::to_vec(&intent)?)?;
            state
                .attempts
                .iter_mut()
                .find(|a| a.identity == *identity)
                .ok_or_else(refused)?
                .recovery_ref = reference;
            let reason = self
                .owner_mut()?
                .admit_host_grant(kind.words().as_bytes())?;
            state.wait = RunWait::Provider(WaitDetail {
                reason,
                missing: vec![],
                attempt: Some(identity.attempt.clone()),
            });
            state.phase = RunPhase::Interrupted;
            crate::orchestration::tasks::refresh_cleanup(&mut state);
            return self.publish_native(producer, state, identity, vec![]);
        }
        if !kind.deterministic() {
            // A person Pause wins even if a different transient error raced it.
            if state.phase != RunPhase::Paused {
                self.wait_for_provider(&identity.run, kind.words())?;
            }
            return Ok(());
        }
        if parent.is_some() && sent {
            if matches!(
                state.wait,
                RunWait::Files(_) | RunWait::Provider(_) | RunWait::ProviderSettings(_)
            ) {
                return Ok(());
            }
            return self.record_runtime_wait(
                producer,
                &identity.run,
                SENT_TASK_RECOVERY,
                false,
                RunWait::Provider,
            );
        }
        if state.effects.len() >= ITEM_LIMIT {
            return Err(HostRefusal::new("task_preparation", "notification capacity").into());
        }
        let reason = if parent.is_none() {
            DIRECTOR_RECOVERY
        } else {
            kind.words()
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
        child.progress.capacity =
            crate::orchestration::orchestration_records::DependencyReservation::Free;
        child.progress.decisions.push(reference.clone());
        let index = self.resolve_attachment(Some(&producer.bearer()))?;
        let issued = self.issue_metadata(index, None)?;
        if !matches!(
            state.wait,
            RunWait::Files(_) | RunWait::Provider(_) | RunWait::ProviderSettings(_)
        ) {
            state.wait = RunWait::Replan(WaitDetail {
                reason: reference.clone(),
                missing: vec![],
                attempt: Some(identity.attempt.clone()),
            });
        }
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
                    crate::orchestration::tasks::replan_effect_key(&identity.attempt)
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

#[cfg(all(test, target_os = "linux"))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PreparationCheckpoint {
    History,
    Publish,
}
#[cfg(all(test, target_os = "linux"))]
impl HostCommandOwner {
    pub(super) fn preparation_checkpoint(
        &mut self,
        point: PreparationCheckpoint,
    ) -> Result<(), HostError> {
        if self.preparation_fault == Some(point) {
            self.preparation_fault = None;
            return Err(std::io::Error::other("fixture preparation store error").into());
        }
        Ok(())
    }
}
