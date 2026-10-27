//! Registered task ingress and exact frozen results through the existing project owner.
//!
//! Rust adaptation of <https://github.com/pingdotgg/t3code> at
//! 5d91ab1117ad51ef1288a94717d85c70e7af09fe, apps/server/src/orchestration-v2/
//! Orchestrator.ts and apps/server/src/mcp/OrchestratorMcpService.ts.
//! Notice: LICENSES/t3code-MIT.txt. Kinewright drains a registered Core and merges
//! its applied delta at an exact durable candidate instead of restoring Git files.
#![allow(clippy::missing_errors_doc)]
use super::{
    orchestration_records::{AttemptId, EndpointId, RunId, TaskBrief, TaskId},
    policy::TaskPolicy,
};
use kinewright_core::{Core, Document, Operation};
use kinewright_project::{
    bundle::BundleError,
    editing_session::{DurableBranchSessionGuard, FrozenTaskResult, Ja1EditingSession},
    receipts::{CommandEnvelope, CommandId, ReceiptLookup},
};
use std::{
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

/// H3 narrows its resolved Producer into this runtime-only identity.
#[derive(Debug, Clone)]
pub struct RuntimeTaskAuthority {
    pub project: super::bundle_records::ProjectId,
    pub run: RunId,
    pub task: TaskId,
    pub attempt: AttemptId,
    pub endpoint: EndpointId,
    pub owner_epoch: u64,
    pub expires_at: Instant,
}

struct Ingress {
    gate: Mutex<()>,
    closed: AtomicBool,
    revoked: AtomicBool,
    authority: RuntimeTaskAuthority,
    actor: kinewright_core::ActorIdentity,
    policy: TaskPolicy,
}

#[derive(Clone)]
pub struct TaskEndpoint(Arc<Ingress>);
impl TaskEndpoint {
    /// Receiving port only. H3 must resolve attachment, epoch and scope before calling.
    #[allow(dead_code, reason = "H3 owns authenticated endpoint construction")]
    pub(crate) fn from_authorized_owner(
        policy: TaskPolicy,
        authority: RuntimeTaskAuthority,
        core: &Core,
    ) -> Self {
        Self(Arc::new(Ingress {
            gate: Mutex::new(()),
            closed: AtomicBool::new(false),
            revoked: AtomicBool::new(false),
            policy,
            authority,
            actor: core.actor_identity(),
        }))
    }
    #[cfg(any(test, feature = "eval-harness"))]
    #[must_use]
    pub fn for_fixture(policy: TaskPolicy, authority: RuntimeTaskAuthority, core: &Core) -> Self {
        Self::from_authorized_owner(policy, authority, core)
    }
    #[must_use]
    pub fn authority(&self) -> &RuntimeTaskAuthority {
        &self.0.authority
    }
    pub fn revoke(&self) {
        self.0.revoked.store(true, Ordering::SeqCst);
    }
    pub(crate) fn matches_core(&self, core: &Core) -> bool {
        self.0.actor == core.actor_identity()
    }
    pub(crate) fn check_live(&self) -> Result<(), String> {
        if self.0.revoked.load(Ordering::SeqCst) || Instant::now() >= self.0.authority.expires_at {
            return Err("task_authority_expired".into());
        }
        Ok(())
    }
    pub(crate) fn admit(&self) -> Result<MutexGuard<'_, ()>, String> {
        let guard = self.0.gate.lock().map_err(|_| "task_ingress_poisoned")?;
        self.check_live()?;
        if self.0.closed.load(Ordering::SeqCst) {
            return Err("task_freezing".into());
        }
        Ok(guard)
    }
    #[cfg(test)]
    pub(crate) fn is_closed_for_fixture(&self) -> bool {
        self.0.closed.load(Ordering::SeqCst)
    }
    pub(crate) fn close_and_drain(&self) -> Result<MutexGuard<'_, ()>, String> {
        self.0.closed.store(true, Ordering::SeqCst);
        self.0
            .gate
            .lock()
            .map_err(|_| "task_ingress_poisoned".into())
    }
    pub(crate) fn validate_batch(
        &self,
        document: &Document,
        operations: &[Operation],
    ) -> Result<(), String> {
        self.check_live()?;
        self.0.policy.validate_batch(document, operations)
    }
    pub(crate) fn admit_capability(&self, name: &str) -> Result<(), String> {
        self.check_live()?;
        // Evidence-only timeline inspection has no filesystem, cancellation or task effects.
        // Other inspectors/planners need a classified adapter before Studio can expose them.
        if matches!(
            name,
            "get_timeline_state"
                | "search_capabilities"
                | "get_capability"
                | "prepare_edit_plan"
                | "commit_edit_plan"
                | "discard_edit_plan"
                | "invoke_capability"
                | "plan_motion"
        ) {
            Ok(())
        } else {
            Err(format!("task_scope: capability {name} is unavailable"))
        }
    }
}

pub struct EditorTask {
    endpoint: TaskEndpoint,
    registration: DurableBranchSessionGuard,
    frozen: Option<FrozenTaskResult>,
    merge_envelope: Option<CommandEnvelope>,
}
impl EditorTask {
    #[allow(dead_code, reason = "H3 supplies the authenticated preparation caller")]
    pub(crate) fn prepare(
        owner: &mut Ja1EditingSession,
        brief: TaskBrief,
        authority: RuntimeTaskAuthority,
    ) -> Result<Self, BundleError> {
        validate_owner_authority(owner, &authority)?;
        let registration =
            owner.fork_registered_from_candidate(&brief.scope.base, "Studio task")?;
        let endpoint = TaskEndpoint::from_authorized_owner(
            TaskPolicy {
                role: brief.role,
                scope: brief.scope,
            },
            authority,
            registration.core(),
        );
        Ok(Self {
            endpoint,
            registration,
            frozen: None,
            merge_envelope: None,
        })
    }
    #[cfg(any(test, feature = "eval-harness"))]
    pub fn prepare_fixture(
        owner: &mut Ja1EditingSession,
        brief: TaskBrief,
        authority: RuntimeTaskAuthority,
    ) -> Result<Self, BundleError> {
        Self::prepare(owner, brief, authority)
    }
    /// Reconstructs exact branch/base identity; a runtime counter never replaces it.
    #[allow(
        dead_code,
        clippy::too_many_arguments,
        reason = "H3 supplies persisted task and merge identities"
    )]
    pub(crate) fn reopen(
        owner: &mut Ja1EditingSession,
        brief: TaskBrief,
        authority: RuntimeTaskAuthority,
        branch: super::bundle_records::BranchId,
        base_ref: &super::bundle_records::ObjectRef,
        original_merge: Option<CommandEnvelope>,
    ) -> Result<Self, BundleError> {
        validate_owner_authority(owner, &authority)?;
        let base = owner
            .store()?
            .read_record::<kinewright_project::bundle_records::SpecialistBaseRecord>(base_ref)?;
        if base.target_branch != brief.scope.base.branch
            || base.base_head != brief.scope.base.head
            || base.frozen_checkpoint_ref != brief.scope.base.checkpoint
            || base.base_document_hash != brief.scope.base.document_hash
        {
            return Err(BundleError::HistoryChanged);
        }
        drop(base);
        let registration = owner.reopen_registered_candidate(branch, base_ref)?;
        let endpoint = TaskEndpoint::from_authorized_owner(
            TaskPolicy {
                role: brief.role,
                scope: brief.scope,
            },
            authority,
            registration.core(),
        );
        Ok(Self {
            endpoint,
            registration,
            frozen: None,
            merge_envelope: original_merge,
        })
    }
    #[must_use]
    pub fn endpoint(&self) -> TaskEndpoint {
        self.endpoint.clone()
    }
    #[must_use]
    pub fn core(&self) -> &Core {
        self.registration.core()
    }
    pub fn freeze(
        &mut self,
        owner: &mut Ja1EditingSession,
    ) -> Result<&FrozenTaskResult, BundleError> {
        if self.frozen.is_none() {
            let _drain = self
                .endpoint
                .close_and_drain()
                .map_err(BundleError::Unsupported)?;
            self.frozen = Some(owner.freeze_registered_candidate(&mut self.registration)?);
        }
        self.frozen.as_ref().ok_or(BundleError::HistoryChanged)
    }
    /// Revalidate the full frozen delta at the original durable base before merge/replay.
    pub fn merge(
        &mut self,
        owner: &mut Ja1EditingSession,
        id: CommandId,
    ) -> Result<ReceiptLookup, BundleError> {
        self.endpoint
            .check_live()
            .map_err(BundleError::Unsupported)?;
        validate_owner_authority(owner, self.endpoint.authority())?;
        let frozen = self.frozen.as_ref().ok_or(BundleError::HistoryChanged)?;
        let checkpoint = owner
            .store()?
            .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(
                &frozen.base().checkpoint,
            )?;
        let document = owner
            .store()?
            .read_record::<Document>(&checkpoint.document_ref)?;
        self.endpoint
            .validate_batch(&document, frozen.delta())
            .map_err(BundleError::Unsupported)?;
        if let Some(envelope) = &self.merge_envelope {
            if envelope.id != id {
                return Err(BundleError::HistoryChanged);
            }
        } else {
            self.merge_envelope = Some(owner.candidate_merge_envelope(frozen, id)?);
        }
        let envelope = self
            .merge_envelope
            .as_ref()
            .ok_or(BundleError::HistoryChanged)?;
        if envelope.project_id != owner.active_manual_project()?
            || envelope.target_branch != frozen.base().branch
            || envelope.expected_head.as_ref() != Some(&frozen.base().head)
            || envelope.action != "edit"
            || envelope.command
                != Some(kinewright_core::JournalCommand::DoBatch(
                    frozen.delta().to_vec(),
                ))
        {
            return Err(BundleError::HistoryChanged);
        }
        owner.commit_candidate(
            frozen,
            self.merge_envelope
                .as_ref()
                .ok_or(BundleError::HistoryChanged)?,
        )
    }
    #[must_use]
    pub const fn original_merge_envelope(&self) -> Option<&CommandEnvelope> {
        self.merge_envelope.as_ref()
    }
}

/// Pure C3 decisions. H3 authenticates the Director before asking for a decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskTransition {
    Assign(Box<super::orchestration_records::AssignTask>),
    Freeze(TaskId),
}

pub fn decide_task(
    state: &super::orchestration_records::RunProjection,
    command: &super::orchestration_records::OrchestrationCommand,
) -> Result<Option<TaskTransition>, super::orchestration_records::Refusal> {
    use super::orchestration_records::{
        OrchestrationCommand, Refusal, Role, RunOutcome, TASK_LIMIT, TaskState,
    };
    match command {
        OrchestrationCommand::AssignTask(assignment) => {
            if assignment.run != state.run {
                return Err(Refusal::ScopeConflict);
            }
            let parent = state
                .tasks
                .iter()
                .find(|task| task.id == assignment.parent)
                .ok_or(Refusal::ScopeConflict)?;
            if assignment.brief.role == Role::Director
                || parent.brief.role != Role::Director
                || matches!(
                    parent.state,
                    TaskState::Terminal(_) | TaskState::Cancelling | TaskState::CleanupBlocked(_)
                )
                || state.outcome != RunOutcome::Pending
            {
                return Err(Refusal::InvalidState);
            }
            if state.tasks.len() >= TASK_LIMIT {
                return Err(Refusal::Capacity);
            }
            if assignment.brief.role != Role::Reviewer
                && state.tasks.iter().any(|task| {
                    task.parent.is_some()
                        && task.brief.role != Role::Reviewer
                        && !matches!(task.state, TaskState::Terminal(_))
                        && task.brief.scope.base.branch == assignment.brief.scope.base.branch
                })
            {
                return Err(Refusal::InvalidState);
            }
            Ok(Some(TaskTransition::Assign(assignment.clone())))
        }
        OrchestrationCommand::FreezeTask(id) => {
            let task = state
                .tasks
                .iter()
                .find(|task| &task.id == id)
                .ok_or(Refusal::ScopeConflict)?;
            if !matches!(task.state, TaskState::Preparing | TaskState::Running(_)) {
                return Err(Refusal::InvalidState);
            }
            Ok(Some(TaskTransition::Freeze(id.clone())))
        }
        _ => Ok(None),
    }
}

pub fn apply_task_transition(
    state: &mut super::orchestration_records::RunProjection,
    envelope: &super::orchestration_records::OrchestrationEnvelope,
    transition: TaskTransition,
) -> Result<(), super::orchestration_records::Refusal> {
    use super::orchestration_records::{
        EffectRecord, EffectSafety, EffectState, Refusal, TaskRecord, TaskState,
    };
    match transition {
        TaskTransition::Assign(assignment) => {
            let task = TaskRecord {
                id: envelope.identity("task")?,
                parent: Some(assignment.parent),
                brief: assignment.brief,
                state: TaskState::Queued,
            };
            let order = state
                .effects
                .iter()
                .filter(|effect| effect.lane == task.brief.scope.base.branch)
                .map(|effect| effect.order)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or(Refusal::Capacity)?;
            state.effects.push(EffectRecord {
                id: envelope.identity("effect.prepare")?,
                task: task.id.clone(),
                lane: task.brief.scope.base.branch.clone(),
                order,
                safety: EffectSafety::ReplaySafe,
                state: EffectState::Pending,
                payload: task.brief.assignment.clone(),
            });
            state.tasks.push(task);
        }
        TaskTransition::Freeze(id) => {
            let task = state
                .tasks
                .iter_mut()
                .find(|task| task.id == id)
                .ok_or(Refusal::ScopeConflict)?;
            task.state = TaskState::Freezing;
        }
    }
    Ok(())
}

fn validate_owner_authority(
    owner: &Ja1EditingSession,
    authority: &RuntimeTaskAuthority,
) -> Result<(), BundleError> {
    if authority.project != owner.active_manual_project()?
        || authority.owner_epoch != owner.store()?.owner_epoch()
        || Instant::now() >= authority.expires_at
    {
        return Err(BundleError::Unsupported("task project/epoch/expiry".into()));
    }
    Ok(())
}
