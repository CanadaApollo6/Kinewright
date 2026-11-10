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
        atomic::{AtomicBool, AtomicU8, Ordering},
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

const INGRESS_OPEN: u8 = 0;
const INGRESS_FINISH_DRAINING: u8 = 1;
const INGRESS_SEALED: u8 = 2;
const INGRESS_FINISH_UNCERTAIN: u8 = 3;

/// Only a not-yet-accepted finish may release its own temporary ingress fence.
/// The gate is quiescent before construction. Keeping this token blocks future
/// edits without holding the mutex across nested host checkpoint operations.
pub(crate) struct FinishDrain {
    endpoint: TaskEndpoint,
}
impl FinishDrain {
    pub(crate) fn accept(&mut self) {
        self.endpoint
            .0
            .phase
            .store(INGRESS_SEALED, Ordering::SeqCst);
    }
    pub(crate) fn publication_unknown(&mut self) {
        let _ = self.endpoint.0.phase.compare_exchange(
            INGRESS_FINISH_DRAINING,
            INGRESS_FINISH_UNCERTAIN,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }
    fn reopen_refused_finish(&self) {
        let _ = self.endpoint.0.phase.compare_exchange(
            INGRESS_FINISH_DRAINING,
            INGRESS_OPEN,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }
}
impl Drop for FinishDrain {
    fn drop(&mut self) {
        self.reopen_refused_finish();
    }
}

struct Ingress {
    gate: Mutex<()>,
    #[cfg(test)]
    admission_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    #[cfg(all(test, target_os = "linux"))]
    owner_wait: Mutex<Option<Arc<std::sync::Barrier>>>,
    phase: AtomicU8,
    revoked: AtomicBool,
    paused: AtomicBool,
    media_read_busy: AtomicBool,
    authority: RuntimeTaskAuthority,
    actor: kinewright_core::ActorIdentity,
    policy: TaskPolicy,
}

#[derive(Clone)]
pub struct TaskEndpoint(Arc<Ingress>);
#[derive(Debug, Clone, Copy)]
pub(crate) enum MediaReadStartError {
    Unavailable,
    Busy,
}
pub(crate) struct MediaReadPermit(TaskEndpoint);
impl Drop for MediaReadPermit {
    fn drop(&mut self) {
        self.0.0.media_read_busy.store(false, Ordering::Release);
    }
}
pub(crate) fn is_liveness_refusal(reason: &str) -> bool {
    matches!(
        reason,
        "task_authority_expired"
            | "task_authority_paused"
            | "task_freezing"
            | "task_ingress_poisoned"
    )
}
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
            #[cfg(test)]
            admission_hook: Mutex::new(None),
            #[cfg(all(test, target_os = "linux"))]
            owner_wait: Mutex::new(None),
            phase: AtomicU8::new(INGRESS_OPEN),
            revoked: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            media_read_busy: AtomicBool::new(false),
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
    pub(crate) fn begin_media_read(&self) -> Result<MediaReadPermit, MediaReadStartError> {
        self.check_live()
            .map_err(|_| MediaReadStartError::Unavailable)?;
        if self.0.phase.load(Ordering::Acquire) != INGRESS_OPEN {
            return Err(MediaReadStartError::Unavailable);
        }
        self.0
            .media_read_busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| MediaReadStartError::Busy)?;
        Ok(MediaReadPermit(self.clone()))
    }
    pub(crate) fn media_read_live(&self) -> bool {
        self.check_live().is_ok() && self.0.phase.load(Ordering::Acquire) == INGRESS_OPEN
    }
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn observe_next_owner_wait_for_fixture(&self, gate: Arc<std::sync::Barrier>) {
        *self.0.owner_wait.lock().unwrap() = Some(gate);
    }
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn signal_owner_wait_for_fixture(&self) {
        if let Some(gate) = self.0.owner_wait.lock().unwrap().take() {
            gate.wait();
        }
    }
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn media_read_pending_for_fixture(&self) -> bool {
        self.0.media_read_busy.load(Ordering::Acquire)
    }
    pub fn revoke(&self) {
        self.0.revoked.store(true, Ordering::SeqCst);
    }
    pub(crate) fn set_paused(&self, paused: bool) {
        self.0.paused.store(paused, Ordering::SeqCst);
    }
    pub(crate) fn matches_brief(&self, brief: &TaskBrief) -> bool {
        self.0.policy.scope == brief.scope && self.0.policy.role == brief.role
    }
    pub(crate) fn head(&self) -> &super::bundle_records::HeadId {
        &self.0.policy.scope.base.head
    }
    pub(crate) fn matches_core(&self, core: &Core) -> bool {
        self.0.actor == core.actor_identity()
    }
    pub(crate) fn check_live(&self) -> Result<(), String> {
        if self.0.revoked.load(Ordering::SeqCst) || Instant::now() >= self.0.authority.expires_at {
            return Err("task_authority_expired".into());
        }
        if self.0.paused.load(Ordering::SeqCst) {
            return Err("task_authority_paused".into());
        }
        Ok(())
    }
    pub(crate) fn admit(&self) -> Result<MutexGuard<'_, ()>, String> {
        let guard = self.0.gate.lock().map_err(|_| "task_ingress_poisoned")?;
        self.check_live()?;
        if self.0.phase.load(Ordering::SeqCst) != INGRESS_OPEN {
            return Err("task_freezing".into());
        }
        #[cfg(test)]
        if let Some(hook) = self.0.admission_hook.lock().unwrap().take() {
            hook();
        }
        Ok(guard)
    }
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn hold_next_admission_for_fixture(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        *self.0.admission_hook.lock().unwrap() = Some(hook);
    }
    #[cfg(test)]
    pub(crate) fn is_closed_for_fixture(&self) -> bool {
        self.0.phase.load(Ordering::SeqCst) != INGRESS_OPEN
    }
    /// Called only after exact-envelope reconciliation proved no accepted intent.
    pub(crate) fn reconcile_unapplied_finish(&self) {
        let _ = self.0.phase.compare_exchange(
            INGRESS_FINISH_UNCERTAIN,
            INGRESS_OPEN,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }
    pub(crate) fn seal_accepted_finish(&self) {
        self.0.phase.store(INGRESS_SEALED, Ordering::SeqCst);
    }
    pub(crate) fn begin_finish(&self) -> Result<FinishDrain, String> {
        // Host dispatch owns the host mutex. Waiting here could deadlock an
        // admitted edit whose reply still needs that owner; refuse and retry.
        let _gate = self.0.gate.try_lock().map_err(|_| "task_ingress_busy")?;
        self.check_live()?;
        if self.0.phase.load(Ordering::SeqCst) == INGRESS_SEALED {
            return Ok(FinishDrain {
                endpoint: self.clone(),
            });
        }
        self.0
            .phase
            .compare_exchange(
                INGRESS_OPEN,
                INGRESS_FINISH_DRAINING,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .map_err(|_| "task_freezing")?;
        Ok(FinishDrain {
            endpoint: self.clone(),
        })
    }
    pub(crate) fn close_and_drain(&self) -> Result<MutexGuard<'_, ()>, String> {
        self.0.phase.store(INGRESS_SEALED, Ordering::SeqCst);
        self.0
            .gate
            .lock()
            .map_err(|_| "task_ingress_poisoned".into())
    }
    pub(crate) fn preview_history_step(
        &self,
        core: &Core,
        expected: kinewright_core::TimelineRevision,
        redo: bool,
    ) -> Result<
        (
            kinewright_core::HistorySnapshot,
            kinewright_core::HistorySnapshot,
            Vec<Operation>,
        ),
        String,
    > {
        self.check_live()?;
        if !self.matches_core(core) {
            return Err("task actor changed".into());
        }
        let kinewright_core::Event::QueryResult(kinewright_core::QueryResult::Snapshot {
            revision,
            ..
        }) = core
            .request(kinewright_core::Command::Query(
                kinewright_core::Query::Snapshot,
            ))
            .map_err(|e| e.to_string())?
        else {
            return Err("task revision unavailable".into());
        };
        if revision != expected {
            return Err("task revision changed".into());
        }
        let history = core.export_history().map_err(|e| e.to_string())?;
        let boundary = if redo {
            history.cursor.redo.last()
        } else {
            history.cursor.undo.last()
        }
        .ok_or("history cursor is empty")?;
        self.0.policy.validate_batch(
            if redo {
                &history.document
            } else {
                &boundary.document
            },
            &boundary.operations,
        )?;
        let operations = boundary.operations.clone();
        let private =
            Core::spawn(std::sync::Arc::clone(&history.baseline)).map_err(|e| e.to_string())?;
        private
            .restore_history(history.clone())
            .map_err(|e| e.to_string())?;
        private
            .request(if redo {
                kinewright_core::Command::Redo
            } else {
                kinewright_core::Command::Undo
            })
            .map_err(|e| e.to_string())?;
        let after = private.export_history().map_err(|e| e.to_string())?;
        self.0
            .policy
            .validate_history_change(&history.document, &after.document)?;
        Ok((history, after, operations))
    }
    pub(crate) fn history_step(
        &self,
        core: &Core,
        expected: kinewright_core::TimelineRevision,
        redo: bool,
    ) -> Result<kinewright_core::HistorySnapshot, String> {
        let _gate = self.admit()?;
        let (_, after, _) = self.preview_history_step(core, expected, redo)?;
        core.request(if redo {
            kinewright_core::Command::Redo
        } else {
            kinewright_core::Command::Undo
        })
        .map_err(|e| e.to_string())?;
        let actual = core.export_history().map_err(|e| e.to_string())?;
        if actual != after {
            return Err("task history differs from validated transition".into());
        }
        Ok(actual)
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
        Self::prepare_inner(owner, brief, authority, None)
    }
    pub(crate) fn prepare_retained(
        owner: &mut Ja1EditingSession,
        brief: TaskBrief,
        authority: RuntimeTaskAuthority,
        branch: super::bundle_records::BranchId,
    ) -> Result<Self, BundleError> {
        Self::prepare_inner(owner, brief, authority, Some(branch))
    }
    fn prepare_inner(
        owner: &mut Ja1EditingSession,
        brief: TaskBrief,
        authority: RuntimeTaskAuthority,
        branch: Option<super::bundle_records::BranchId>,
    ) -> Result<Self, BundleError> {
        validate_owner_authority(owner, &authority)?;
        let registration = match branch {
            Some(branch) => owner.fork_registered_from_candidate_retained(
                &brief.scope.base,
                "Studio task",
                branch,
            )?,
            None => owner.fork_registered_from_candidate(&brief.scope.base, "Studio task")?,
        };
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
        let registered = owner
            .durable_sessions()
            .into_iter()
            .find(|(_, g)| g.branch_id() == &branch)
            .map(|(_, g)| g);
        let registration = if let Some(guard) = registered {
            if guard.base() != &*base {
                return Err(BundleError::HistoryChanged);
            }
            guard
        } else {
            owner.reopen_registered_candidate(branch, base_ref)?
        };
        drop(base);
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
    /// Replace only retired authority. The registered actor and all receipts stay.
    pub(crate) fn continue_authority(
        mut self,
        owner: &Ja1EditingSession,
        brief: &TaskBrief,
        authority: RuntimeTaskAuthority,
    ) -> Result<Self, BundleError> {
        validate_owner_authority(owner, &authority)?;
        if !self.endpoint.matches_brief(brief) {
            return Err(BundleError::HistoryChanged);
        }
        {
            let _drain = self
                .endpoint
                .close_and_drain()
                .map_err(BundleError::Unsupported)?;
            self.endpoint.revoke();
        }
        self.endpoint = TaskEndpoint::from_authorized_owner(
            TaskPolicy {
                role: brief.role,
                scope: brief.scope.clone(),
            },
            authority,
            self.registration.core(),
        );
        if self.frozen.is_some() {
            self.endpoint
                .0
                .phase
                .store(INGRESS_SEALED, Ordering::SeqCst);
        }
        Ok(self)
    }
    pub(crate) fn persisted_history(
        &mut self,
        owner: &mut Ja1EditingSession,
        fence: bool,
    ) -> Result<
        (
            super::orchestration_records::BranchBinding,
            super::bundle_records::ObjectRef,
            bool,
        ),
        BundleError,
    > {
        if let Some(frozen) = &self.frozen {
            return Ok((frozen.source().clone(), frozen.base_ref().clone(), true));
        }
        if fence {
            drop(
                self.endpoint
                    .close_and_drain()
                    .map_err(BundleError::Unsupported)?,
            );
        }
        let _gate = self
            .endpoint
            .0
            .gate
            .lock()
            .map_err(|_| BundleError::HistoryChanged)?;
        let base_ref = owner.persist_registered_task_checkpoint(&mut self.registration)?;
        Ok((
            owner.task_checkpoint_binding(self.registration.branch_id())?,
            base_ref,
            false,
        ))
    }
    pub(crate) fn prepare_merge(
        &mut self,
        owner: &Ja1EditingSession,
        id: CommandId,
    ) -> Result<(), BundleError> {
        if let Some(envelope) = &self.merge_envelope {
            return if envelope.id == id {
                Ok(())
            } else {
                Err(BundleError::HistoryChanged)
            };
        }
        validate_owner_authority(owner, self.endpoint.authority())?;
        let frozen = self.frozen.as_ref().ok_or(BundleError::HistoryChanged)?;
        self.merge_envelope = Some(owner.candidate_merge_envelope(frozen, id)?);
        Ok(())
    }
    pub(crate) fn frozen_delta(&self) -> Option<&[Operation]> {
        self.frozen.as_ref().map(FrozenTaskResult::delta)
    }
    #[must_use]
    pub fn endpoint(&self) -> TaskEndpoint {
        self.endpoint.clone()
    }
    #[must_use]
    pub fn core(&self) -> &Core {
        self.registration.core()
    }
    #[cfg(all(target_os = "linux", any(test, feature = "eval-harness")))]
    pub fn freeze_fixture(
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
    pub fn freeze(
        &mut self,
        owner: &mut Ja1EditingSession,
    ) -> Result<&FrozenTaskResult, BundleError> {
        self.freeze_at_revision(owner, None)
    }
    pub(crate) fn freeze_at_revision(
        &mut self,
        owner: &mut Ja1EditingSession,
        expected: Option<kinewright_core::TimelineRevision>,
    ) -> Result<&FrozenTaskResult, BundleError> {
        let _drain = self
            .endpoint
            .close_and_drain()
            .map_err(BundleError::Unsupported)?;
        if let Some(expected) = expected {
            let kinewright_core::Event::QueryResult(kinewright_core::QueryResult::Snapshot {
                revision,
                ..
            }) = self
                .core()
                .request(kinewright_core::Command::Query(
                    kinewright_core::Query::Snapshot,
                ))
                .map_err(|error| BundleError::Corrupt(error.to_string()))?
            else {
                return Err(BundleError::HistoryChanged);
            };
            if revision != expected {
                return Err(BundleError::Unsupported("timeline revision changed".into()));
            }
        }
        if self.frozen.is_none() {
            self.frozen = Some(owner.freeze_registered_task_checkpoint(&mut self.registration)?);
        }
        self.frozen.as_ref().ok_or(BundleError::HistoryChanged)
    }
    /// Replay the original receipt before admitting and validating a fresh merge.
    /// H3 must authenticate the current attachment and project before calling.
    /// `Ok` alone does not confirm a committed merge. Callers must handle
    /// `Pending`, `Retired`, `RetiredUnresolved`, and `IdPayloadConflict` explicitly.
    pub fn merge(
        &mut self,
        owner: &mut Ja1EditingSession,
        id: CommandId,
    ) -> Result<ReceiptLookup, BundleError> {
        if let Some(envelope) = &self.merge_envelope {
            if envelope.id != id {
                return Err(BundleError::HistoryChanged);
            }
            let receipt = owner.manual_transition_receipt(envelope)?;
            if receipt != ReceiptLookup::Absent {
                return Ok(receipt);
            }
        }
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
            .0
            .policy
            .validate_batch(&document, frozen.delta())
            .map_err(BundleError::Unsupported)?;
        if self.merge_envelope.is_none() {
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
    /// Persist terminal work, seal its Core and release the owner's registration.
    pub fn close(&mut self, owner: &mut Ja1EditingSession) -> Result<(), BundleError> {
        self.freeze(owner)?;
        owner.close_registered_task_checkpoint(&self.registration)?;
        self.endpoint.revoke();
        Ok(())
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
                || parent.brief.role == Role::Reviewer
                || matches!(
                    parent.state,
                    TaskState::Freezing
                        | TaskState::Terminal(_)
                        | TaskState::Cancelled
                        | TaskState::Cancelling
                        | TaskState::CleanupBlocked(_)
                )
                || state.outcome != RunOutcome::Pending
            {
                return Err(Refusal::InvalidState);
            }
            if dependency_slots(state) >= super::orchestration_records::ATTEMPT_LIMIT {
                return Err(Refusal::DependencyCapacity);
            }
            if state.phase == super::orchestration_records::RunPhase::Paused {
                return Err(Refusal::InvalidState);
            }
            if state.tasks.len() >= TASK_LIMIT {
                return Err(Refusal::Capacity);
            }
            if assignment.brief.role != Role::Reviewer
                && state.tasks.iter().any(|task| {
                    task.parent.is_some()
                        && task.brief.role != Role::Reviewer
                        && !matches!(task.state, TaskState::Terminal(_) | TaskState::Cancelled)
                        && task.brief.scope.base.branch == assignment.brief.scope.base.branch
                })
            {
                return Err(Refusal::DependencyCapacity);
            }
            Ok(Some(TaskTransition::Assign(assignment.clone())))
        }
        OrchestrationCommand::FreezeTask(id) => {
            let task = state
                .tasks
                .iter()
                .find(|task| &task.id == id)
                .ok_or(Refusal::ScopeConflict)?;
            if !ready(state, id)
                || !matches!(
                    task.state,
                    TaskState::Queued
                        | TaskState::Preparing
                        | TaskState::Running(_)
                        | TaskState::WaitingForChildren(_)
                )
            {
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
                progress: super::orchestration_records::TaskProgress {
                    capacity: super::orchestration_records::DependencyReservation::Reserved,
                    ..super::orchestration_records::TaskProgress::default()
                },
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
                safety: EffectSafety::ProcessBound,
                state: EffectState::Pending,
                payload: task.brief.assignment.clone(),
            });
            state.tasks.push(task);
            if matches!(state.wait, super::orchestration_records::RunWait::Replan(_)) {
                if let super::orchestration_records::RunWait::Replan(detail) = &state.wait
                    && let Some(attempt) = &detail.attempt
                {
                    let key = replan_effect_key(attempt);
                    for effect in &mut state.effects {
                        if effect.id.0.as_str() == key
                            && effect.state == super::orchestration_records::EffectState::Pending
                        {
                            effect.state = super::orchestration_records::EffectState::Disposed;
                        }
                    }
                }
                state.wait = super::orchestration_records::RunWait::None;
            }
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

#[must_use]
pub fn ready(state: &super::orchestration_records::RunProjection, id: &TaskId) -> bool {
    ready_for_completion(state, id, false)
}
fn ready_for_completion(
    state: &super::orchestration_records::RunProjection,
    id: &TaskId,
    explicit_end_turn: bool,
) -> bool {
    use super::orchestration_records::{
        DeliveryState, QuestionStatus, RuntimeRequest, TaskState, TurnPurpose,
    };
    let Some(task) = state.tasks.iter().find(|task| &task.id == id) else {
        return false;
    };
    (task.progress.turn_ended || explicit_end_turn)
        && !task.progress.expired
        && task.progress.turn.as_ref().is_some_and(|turn| turn.purpose == TurnPurpose::Work)
        && task.progress.background == 0
        && !matches!(task.state, TaskState::Terminal(_) | TaskState::Cancelled | TaskState::Cancelling | TaskState::CleanupBlocked(_))
        && state.tasks.iter().filter(|child| child.parent.as_ref() == Some(id)).all(|child| {
            matches!(child.state, TaskState::Terminal(_) | TaskState::Cancelled)
                && state.deliveries.iter().filter(|delivery| delivery.task == child.id)
                    .all(|delivery| matches!(delivery.state, DeliveryState::Observed(_) | DeliveryState::Disposed))
        })
        && !state.requests.iter().any(|request| matches!(request, RuntimeRequest::Question(q) if &q.task == id && q.status == QuestionStatus::Pending))
}

pub(crate) fn replan_effect_key(attempt: &super::orchestration_records::AttemptId) -> String {
    kinewright_media::sha256_bytes(format!("preparation-replan:{}", attempt.0.as_str()).as_bytes())
        [..32]
        .into()
}

pub fn refresh_wait(state: &mut super::orchestration_records::RunProjection) {
    use super::orchestration_records::{
        DeliveryState, QuestionStatus, RunWait, RuntimeRequest, TaskState,
    };
    if let RunWait::Replan(detail) = &state.wait {
        let delivered = detail.attempt.as_ref().is_some_and(|attempt| {
            let key = replan_effect_key(attempt);
            state.effects.iter().any(|effect| {
                effect.id.0.as_str() == key
                    && effect.state != super::orchestration_records::EffectState::Pending
            })
        });
        if !delivered {
            return;
        }
        state.wait = RunWait::None;
    }
    // Only the file/provider owner can settle these external facts.
    if matches!(
        state.wait,
        RunWait::Files(_) | RunWait::Provider(_) | RunWait::ProviderSettings(_)
    ) {
        return;
    }
    if let Some(question) = state.requests.iter().find_map(|request| match request {
        RuntimeRequest::Question(q) if q.status == QuestionStatus::Pending => Some(q),
        _ => None,
    }) {
        state.wait = RunWait::Question(question.id.clone());
        return;
    }
    let mut children = std::collections::BTreeSet::new();
    for task in &state.tasks {
        if let TaskState::WaitingForChildren(ids) = &task.state {
            children.extend(
                ids.iter()
                    .filter(|id| {
                        state.tasks.iter().any(|child| {
                            child.id == **id
                                && !matches!(
                                    child.state,
                                    TaskState::Terminal(_) | TaskState::Cancelled
                                )
                        })
                    })
                    .cloned(),
            );
        }
    }
    children.extend(
        state
            .deliveries
            .iter()
            .filter(|delivery| {
                !matches!(
                    delivery.state,
                    DeliveryState::Observed(_) | DeliveryState::Disposed
                )
            })
            .map(|delivery| delivery.task.clone()),
    );
    state.wait = if children.is_empty() {
        RunWait::None
    } else {
        RunWait::Children(children.into_iter().collect())
    };
}

pub fn refresh_cleanup(state: &mut super::orchestration_records::RunProjection) {
    use super::orchestration_records::{AttemptState, CleanupIdentity, RunCleanup};
    let identities = state
        .attempts
        .iter()
        .filter(|attempt| !matches!(attempt.state, AttemptState::Retired(_)))
        .map(|attempt| CleanupIdentity {
            attempt: attempt.identity.attempt.clone(),
            nonce: attempt.identity.nonce.clone(),
            owner_instance: attempt.identity.owner_instance.clone(),
            recovery_ref: attempt.recovery_ref.clone(),
        })
        .collect::<Vec<_>>();
    state.cleanup = if identities.is_empty() {
        RunCleanup::Settled
    } else if let RunCleanup::Blocked(_, reason) = &state.cleanup {
        RunCleanup::Blocked(identities, reason.clone())
    } else {
        RunCleanup::Pending(identities)
    };
}

#[must_use]
pub fn dependency_slots(state: &super::orchestration_records::RunProjection) -> usize {
    use super::orchestration_records::{AttemptState, DependencyReservation};
    state
        .attempts
        .iter()
        .filter(|attempt| !matches!(attempt.state, AttemptState::Retired(_)))
        .count()
        + state
            .tasks
            .iter()
            .filter(|task| {
                task.progress.capacity == DependencyReservation::Reserved
                    && !state.attempts.iter().any(|attempt| {
                        attempt.identity.task == task.id
                            && !matches!(attempt.state, AttemptState::Retired(_))
                    })
            })
            .count()
}

pub fn cancel(
    state: &mut super::orchestration_records::RunProjection,
    id: &TaskId,
) -> Result<(), super::orchestration_records::Refusal> {
    use super::orchestration_records::{
        AttemptState, DeliveryState, EffectState, QuestionStatus, QuestionWake, Refusal,
        RuntimeRequest, TaskState,
    };
    if !state.tasks.iter().any(|task| &task.id == id) {
        return Err(Refusal::ScopeConflict);
    }
    let mut ids = std::collections::BTreeSet::from([id.clone()]);
    for _ in 0..state.tasks.len() {
        for task in &state.tasks {
            if task
                .parent
                .as_ref()
                .is_some_and(|parent| ids.contains(parent))
            {
                ids.insert(task.id.clone());
            }
        }
    }
    for task in &mut state.tasks {
        if ids.contains(&task.id)
            && !matches!(task.state, TaskState::Terminal(_) | TaskState::Cancelled)
        {
            task.state = if state.attempts.iter().any(|attempt| {
                attempt.identity.task == task.id
                    && !matches!(attempt.state, AttemptState::Retired(_))
            }) {
                TaskState::Cancelling
            } else {
                task.progress.capacity = super::orchestration_records::DependencyReservation::Free;
                TaskState::Cancelled
            };
        }
    }
    for attempt in &mut state.attempts {
        if ids.contains(&attempt.identity.task)
            && !matches!(
                attempt.state,
                AttemptState::Retired(_) | AttemptState::CleanupBlocked(_)
            )
        {
            attempt.state = AttemptState::Stopping;
        }
    }
    for effect in &mut state.effects {
        if ids.contains(&effect.task) && effect.state == EffectState::Pending {
            effect.state = EffectState::Disposed;
        }
    }
    for delivery in &mut state.deliveries {
        let recipient_cancelled = state
            .tasks
            .iter()
            .find(|task| task.id == delivery.task)
            .and_then(|task| task.parent.as_ref())
            .is_some_and(|parent| ids.contains(parent));
        if (ids.contains(&delivery.task) || recipient_cancelled)
            && !matches!(delivery.state, DeliveryState::Observed(_))
        {
            delivery.state = DeliveryState::Disposed;
        }
    }
    super::mailbox::settle_generations(state);
    for request in &mut state.requests {
        if let RuntimeRequest::Question(q) = request
            && ids.contains(&q.task)
            && q.status == QuestionStatus::Pending
        {
            q.status = QuestionStatus::Cancelled;
            q.wake = QuestionWake::Disposed;
        }
    }
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "one task lifecycle match keeps guards beside mutations"
)]
pub fn apply_lifecycle(
    state: &mut super::orchestration_records::RunProjection,
    envelope: &super::orchestration_records::OrchestrationEnvelope,
) -> Result<bool, super::orchestration_records::Refusal> {
    use super::orchestration_records::{
        AttemptState, BACKGROUND_LIMIT, ITEM_LIMIT, OrchestrationCommand, QuestionStatus, Refusal,
        RunOutcome, RunPhase, RuntimeRequest, TaskState, validate_reference,
    };
    match &envelope.command {
        OrchestrationCommand::FinishTask(request) => {
            // Do not reserve completion identities while the parent still needs
            // ordinary commands to settle children, questions or background work.
            if !ready_for_completion(state, &request.task, true) {
                return Err(Refusal::InvalidState);
            }
            let task = state
                .tasks
                .iter_mut()
                .find(|task| task.id == request.task)
                .ok_or(Refusal::ScopeConflict)?;
            if task.progress.completion.is_some()
                || task.progress.expired
                || matches!(
                    task.state,
                    TaskState::Freezing
                        | TaskState::Terminal(_)
                        | TaskState::Cancelled
                        | TaskState::Cancelling
                        | TaskState::CleanupBlocked(_)
                )
            {
                return Err(Refusal::InvalidState);
            }
            let intent = super::orchestration_records::CompletionIntent {
                id: envelope.id.clone(),
                digest: super::orchestration_records::task_command_digest(&envelope.command)?,
                attempt: envelope
                    .authority
                    .attempt
                    .clone()
                    .ok_or(Refusal::ScopeConflict)?,
                request: request.clone(),
            };
            intent.validate(&task.id)?;
            task.progress.completion = Some(Box::new(intent));
        }
        OrchestrationCommand::BeginTurn(turn) => {
            let task = state
                .tasks
                .iter()
                .find(|task| task.id == turn.task)
                .ok_or(Refusal::ScopeConflict)?;
            if state.phase == RunPhase::Paused
                || state.outcome != RunOutcome::Pending
                || matches!(
                    task.state,
                    TaskState::Terminal(_)
                        | TaskState::Cancelled
                        | TaskState::Cancelling
                        | TaskState::Freezing
                        | TaskState::CleanupBlocked(_)
                )
                || turn.expected_head != task.brief.scope.base.head
                || task.progress.background != 0
                || turn.ordinal == 0
                || task.progress.turn.as_ref().is_some_and(|previous| {
                    (!task.progress.turn_ended && !task.progress.expired) || turn.ordinal <= previous.ordinal || (task.progress.expired && turn.session == previous.session)
                })
                || task.progress.turn.as_ref().is_some_and(|previous| previous.attempt != turn.attempt && state.attempts.iter().any(|reservation| reservation.identity.attempt == previous.attempt && !matches!(reservation.state, AttemptState::Retired(_))))
                || state.requests.iter().any(|request| matches!(request, RuntimeRequest::Question(q) if q.task == turn.task && q.status == QuestionStatus::Pending))
                || turn.owner_epoch != envelope.authority.owner_epoch
            {
                return Err(Refusal::InvalidState);
            }
            if let Some(reservation) = state
                .attempts
                .iter()
                .find(|attempt| attempt.identity.attempt == turn.attempt)
            {
                if reservation.identity.task != turn.task
                    || reservation.owner_epoch != turn.owner_epoch
                    || matches!(
                        reservation.state,
                        AttemptState::Retired(_)
                            | AttemptState::Stopping
                            | AttemptState::CleanupBlocked(_)
                    )
                {
                    return Err(Refusal::ScopeConflict);
                }
            } else {
                return Err(Refusal::ScopeConflict);
            }
            let task = state
                .tasks
                .iter_mut()
                .find(|task| task.id == turn.task)
                .ok_or(Refusal::ScopeConflict)?;
            task.progress.capacity =
                super::orchestration_records::DependencyReservation::Backed(turn.attempt.clone());
            task.progress.turn = Some(turn.clone());
            task.progress.turn_ended = false;
            task.progress.expired = false;
            task.state = TaskState::Running(turn.attempt.clone());
            state.phase = RunPhase::Running;
        }
        OrchestrationCommand::EndTurn(turn) | OrchestrationCommand::BackgroundWork(turn, _) => {
            if envelope.authority.attempt.as_ref() != Some(&turn.attempt)
                || envelope.authority.owner_epoch != turn.owner_epoch
            {
                return Err(Refusal::ScopeConflict);
            }
            if matches!(envelope.command, OrchestrationCommand::BackgroundWork(_, _)) {
                super::requests::live_identity(state, turn)?;
            } else {
                super::requests::live_turn(state, turn)?;
            }
            let task = state
                .tasks
                .iter_mut()
                .find(|task| task.id == turn.task)
                .ok_or(Refusal::ScopeConflict)?;
            if let OrchestrationCommand::BackgroundWork(_, count) = envelope.command {
                if count > BACKGROUND_LIMIT {
                    return Err(Refusal::Capacity);
                }
                task.progress.background = count;
            } else {
                task.progress.turn_ended = true;
                let children = state
                    .tasks
                    .iter()
                    .filter(|task| {
                        task.parent.as_ref() == Some(&turn.task)
                            && !matches!(task.state, TaskState::Terminal(_) | TaskState::Cancelled)
                    })
                    .map(|task| task.id.clone())
                    .collect::<Vec<_>>();
                let task = state
                    .tasks
                    .iter_mut()
                    .find(|task| task.id == turn.task)
                    .ok_or(Refusal::ScopeConflict)?;
                if !children.is_empty() {
                    task.state = TaskState::WaitingForChildren(children);
                }
            }
        }
        OrchestrationCommand::SettleExpiredBackground(turn) => {
            let task = state
                .tasks
                .iter_mut()
                .find(|task| task.id == turn.task)
                .ok_or(Refusal::ScopeConflict)?;
            if envelope.authority.attempt.is_some()
                || envelope.authority.endpoint.is_some()
                || envelope.authority.owner_epoch != turn.owner_epoch
                || !task.progress.expired
                || task.progress.turn.as_ref() != Some(turn)
                || !state.attempts.iter().any(|attempt| {
                    attempt.identity.task == turn.task
                        && attempt.identity.attempt == turn.attempt
                        && attempt.owner_epoch == turn.owner_epoch
                        && matches!(attempt.state, AttemptState::Retired(_))
                })
            {
                return Err(Refusal::InvalidState);
            }
            task.progress.background = 0;
        }
        OrchestrationCommand::CancelTask(id) => cancel(state, id)?,
        OrchestrationCommand::RecordContext {
            task,
            feedback,
            decisions,
        } => {
            let record = state
                .tasks
                .iter_mut()
                .find(|record| &record.id == task)
                .ok_or(Refusal::ScopeConflict)?;
            if record.progress.feedback.len() + feedback.len() > ITEM_LIMIT
                || record.progress.decisions.len() + decisions.len() > ITEM_LIMIT
            {
                return Err(Refusal::Capacity);
            }
            for reference in feedback.iter().chain(decisions) {
                validate_reference(reference)?;
            }
            record.progress.feedback.extend(feedback.clone());
            record.progress.decisions.extend(decisions.clone());
        }
        OrchestrationCommand::WaitTimeout(id) => {
            if !state.tasks.iter().any(|task| &task.id == id) {
                return Err(Refusal::ScopeConflict);
            }
            // Timeout records a receipt; dependency and delivery identities remain intact.
        }
        _ => return Ok(false),
    }
    Ok(true)
}
