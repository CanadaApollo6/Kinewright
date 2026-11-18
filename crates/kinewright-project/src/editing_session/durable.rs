//! Registration belongs to the project owner; dropping a caller's guard does not close its actor.
use super::{BundleError, Core, Ja1EditingSession, ObjectRef, StorageReservation};
use crate::bundle_records::{BranchId, SpecialistBaseRecord};
use crate::operation_history::Role;
use kinewright_core::{Command, Document, Event};
use std::sync::Arc;

/// Exact durable identity. Runtime revision counters never substitute for this binding.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchBinding {
    pub branch: BranchId,
    pub head: crate::bundle_records::HeadId,
    pub checkpoint: ObjectRef,
    pub document_hash: String,
}

/// Sessions grow per edit; native run candidates grow per accepted merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableBranchRole {
    Session,
    RunCandidate,
}
impl DurableBranchRole {
    const fn history_role(self) -> Role {
        match self {
            Self::Session => Role::Task,
            Self::RunCandidate => Role::Candidate,
        }
    }
}

/// Constructed only by freezing a registered, persisted specialist.
#[derive(Debug, Clone)]
pub struct FrozenTaskResult {
    base: BranchBinding,
    source: BranchBinding,
    base_ref: ObjectRef,
    captured_history: crate::edit_audit::CapturedHistory,
}

/// Exact checkpoint and accepted cut captured under the same Core freeze.
pub struct PersistedTaskCheckpoint {
    pub base_ref: ObjectRef,
    pub captured_history: crate::edit_audit::CapturedHistory,
}
impl FrozenTaskResult {
    #[must_use]
    pub const fn base(&self) -> &BranchBinding {
        &self.base
    }
    #[must_use]
    pub const fn source(&self) -> &BranchBinding {
        &self.source
    }
    #[must_use]
    pub const fn base_ref(&self) -> &ObjectRef {
        &self.base_ref
    }

    pub fn delta(
        &self,
        session: &Ja1EditingSession,
    ) -> Result<crate::operation_history::OwnerOperations, BundleError> {
        let bundle = session.active_bundle()?;
        let checkpoint = bundle
            .store
            .read_record::<crate::bundle_records::BranchCheckpoint>(&self.source.checkpoint)?;
        if checkpoint.branch_id != self.source.branch
            || checkpoint.head_id != self.source.head
            || checkpoint.document_ref.sha256 != self.source.document_hash
            || checkpoint.role == Role::Ordinary
        {
            return Err(BundleError::HistoryChanged);
        }
        let reader = checkpoint
            .open(&bundle.store, bundle.history_pool())?
            .into_reader()?;
        reader.into_operations().map_err(BundleError::from)
    }
    #[must_use]
    pub const fn captured_history(&self) -> crate::edit_audit::CapturedHistory {
        self.captured_history
    }
}

pub(super) struct PendingCandidateCommit {
    pub(super) core: Core,
    pub(super) envelope: crate::receipts::CommandEnvelope,
}

#[derive(Clone)]
pub struct DurableBranchSessionGuard {
    pub(super) branch: BranchId,
    pub(super) base: Arc<StorageReservation<SpecialistBaseRecord>>,
    core: Core,
    pub(super) epoch: u64,
}
pub(super) struct RegisteredSession {
    pub(super) guard: DurableBranchSessionGuard,
    pub(super) name: String,
    pin: crate::store::StorePin,
    _charge: StorageReservation,
}
impl DurableBranchSessionGuard {
    #[must_use]
    pub const fn branch_id(&self) -> &BranchId {
        &self.branch
    }
    #[must_use]
    pub fn base(&self) -> &SpecialistBaseRecord {
        &self.base
    }
    #[must_use]
    pub const fn core(&self) -> &Core {
        &self.core
    }
}
impl Ja1EditingSession {
    pub(super) fn registered_bundle(
        &self,
        guard: &DurableBranchSessionGuard,
    ) -> Result<&crate::bundle::ProjectBundle, BundleError> {
        self.registered(guard)?;
        if let Ok(bundle) = self.active_bundle()
            && bundle.store.owner_epoch() == guard.epoch
        {
            return Ok(bundle);
        }
        self.retained_destinations
            .iter()
            .find_map(|(owner, _)| {
                owner
                    .bundle()
                    .ok()
                    .filter(|b| b.store.owner_epoch() == guard.epoch)
            })
            .ok_or_else(|| {
                BundleError::Identity("registered source owner is no longer retained".into())
            })
    }
    fn registered_bundle_mut(
        &mut self,
        guard: &DurableBranchSessionGuard,
    ) -> Result<&mut crate::bundle::ProjectBundle, BundleError> {
        self.registered(guard)?;
        if self
            .active_bundle()
            .is_ok_and(|b| b.store.owner_epoch() == guard.epoch)
        {
            return self.active_bundle_mut();
        }
        self.retained_destinations
            .iter_mut()
            .find(|(owner, _)| {
                owner
                    .bundle()
                    .is_ok_and(|b| b.store.owner_epoch() == guard.epoch)
            })
            .ok_or_else(|| {
                BundleError::Identity("registered source owner is no longer retained".into())
            })?
            .0
            .bundle_mut()
    }
    #[must_use]
    pub fn durable_sessions(&self) -> Vec<(String, DurableBranchSessionGuard)> {
        self.durable_sessions
            .iter()
            .map(|r| (r.name.clone(), r.guard.clone()))
            .collect()
    }
    fn registration_capacity(&mut self, name: &str) -> Result<StorageReservation, BundleError> {
        if self.durable_sessions.len() as u64 >= self.store()?.limits().objects {
            return Err(BundleError::Capacity("durable session registrations"));
        }
        self.durable_sessions
            .try_reserve(1)
            .map_err(|_| BundleError::Capacity("durable session registrations"))?;
        self.store()?.reserve(0, name.len() as u64 + 512, false)
    }
    pub fn candidate_binding(&self, branch: &BranchId) -> Result<BranchBinding, BundleError> {
        let bundle = self.active_bundle()?;
        if branch == bundle.manual_branch_id() {
            return Err(BundleError::HistoryChanged);
        }
        let checkpoint = bundle.checkpoint(branch)?;
        Ok(BranchBinding {
            branch: branch.clone(),
            head: checkpoint.head_id.clone(),
            checkpoint: bundle.checkpoint_reference(branch)?,
            document_hash: checkpoint.document_ref.sha256.clone(),
        })
    }
    /// A prepared binding is published only with its matching H1 history record.
    pub fn task_checkpoint_binding(&self, branch: &BranchId) -> Result<BranchBinding, BundleError> {
        let bundle = self.active_bundle()?;
        if let Some(staged) = bundle.task_checkpoints.get(branch) {
            if bundle.checkpoint_reference(branch)? != staged.before {
                return Err(BundleError::HistoryChanged);
            }
            return Ok(staged.binding());
        }
        self.candidate_binding(branch)
    }
    fn candidate_core(&self, binding: &BranchBinding) -> Result<Core, BundleError> {
        if self.candidate_binding(&binding.branch)? != *binding {
            return Err(BundleError::HistoryChanged);
        }
        let bundle = self.active_bundle()?;
        let registered = self
            .durable_sessions
            .iter()
            .find(|r| {
                r.guard.branch == binding.branch && r.guard.epoch == bundle.store.owner_epoch()
            })
            .ok_or(BundleError::HistoryChanged)?;
        let core = registered.guard.core.clone();
        if core
            .change_id()
            .map_err(crate::bundle::operation_error)?
            .position
            != bundle.checkpoint(&binding.branch)?.position
        {
            return Err(BundleError::HistoryChanged);
        }
        Ok(core)
    }
    pub fn fork_registered_from_candidate(
        &mut self,
        base: &BranchBinding,
        name: &str,
    ) -> Result<DurableBranchSessionGuard, BundleError> {
        self.fork_registered_from_candidate_inner(base, name, None)
    }
    pub fn fork_registered_from_candidate_retained(
        &mut self,
        base: &BranchBinding,
        name: &str,
        branch: BranchId,
    ) -> Result<DurableBranchSessionGuard, BundleError> {
        self.fork_registered_from_candidate_inner(base, name, Some(branch))
    }
    fn retained_registered_fork(
        &mut self,
        base: &BranchBinding,
        name: &str,
        branch: &BranchId,
    ) -> Result<Option<DurableBranchSessionGuard>, BundleError> {
        if let Some(registered) = self
            .durable_sessions()
            .into_iter()
            .find(|(_, g)| g.branch_id() == branch)
            .map(|(_, g)| g)
        {
            if registered.base().target_branch != base.branch
                || registered.base().base_head != base.head
            {
                return Err(BundleError::HistoryChanged);
            }
            return Ok(Some(registered));
        }
        if self.active_bundle()?.branches.contains_key(branch) {
            let bundle = self.active_bundle_mut()?;
            let checkpoint = bundle
                .store()
                .read_record::<crate::bundle_records::BranchCheckpoint>(&base.checkpoint)?;
            let history = bundle.branch_history(branch)?;
            let retained = bundle.checkpoint(branch)?;
            if !history.history().undo().is_empty()
                || !history.history().redo().is_empty()
                || retained.role != Role::Task
                || retained.document_ref.sha256 != base.document_hash
                || history.history().baseline() != history.history().document()
                || retained.input_manifest_ref != checkpoint.input_manifest_ref
            {
                return Err(BundleError::HistoryChanged);
            }
            let specialist = SpecialistBaseRecord {
                history_role: Role::Task,
                target_branch: base.branch.clone(),
                base_head: base.head.clone(),
                base_document_hash: base.document_hash.clone(),
                original_baseline_ref: checkpoint.document_ref.clone(),
                frozen_checkpoint_ref: base.checkpoint.clone(),
            };
            let reference = bundle.stage_specialist_base(&specialist)?;
            return self
                .reopen_branch(branch.clone(), &reference, name, Role::Task)
                .map(Some);
        }
        Ok(None)
    }
    fn fork_registered_from_candidate_inner(
        &mut self,
        base: &BranchBinding,
        name: &str,
        branch: Option<BranchId>,
    ) -> Result<DurableBranchSessionGuard, BundleError> {
        if let Some(branch) = branch.as_ref()
            && let Some(guard) = self.retained_registered_fork(base, name, branch)?
        {
            return Ok(guard);
        }
        let parent = self.candidate_core(base)?;
        let charge = self.registration_capacity(name)?;
        let branch = branch.unwrap_or(BranchId::fresh()?);
        let (core, _) = parent
            .fork(
                Role::Task,
                crate::operation_history::CreationOrigin {
                    branch: branch.clone(),
                    batch: crate::bundle_records::BatchId::fresh()?,
                },
                false,
            )
            .map_err(crate::bundle::operation_error)?;
        self.register_fork(core, branch, base, Role::Task, name, charge)
    }
    fn register_fork(
        &mut self,
        core: Core,
        branch: BranchId,
        base: &BranchBinding,
        role: Role,
        name: &str,
        charge: StorageReservation,
    ) -> Result<DurableBranchSessionGuard, BundleError> {
        let bundle = self.active_bundle_mut()?;
        let checkpoint = bundle
            .store
            .read_record::<crate::bundle_records::BranchCheckpoint>(&base.checkpoint)?;
        if checkpoint.branch_id != base.branch
            || checkpoint.head_id != base.head
            || checkpoint.document_ref.sha256 != base.document_hash
            || bundle.branches.contains_key(&branch)
        {
            return Err(BundleError::HistoryChanged);
        }
        let specialist = SpecialistBaseRecord {
            history_role: role,
            target_branch: base.branch.clone(),
            base_head: base.head.clone(),
            base_document_hash: base.document_hash.clone(),
            original_baseline_ref: checkpoint.document_ref.clone(),
            frozen_checkpoint_ref: base.checkpoint.clone(),
        };
        // Reserve the immutable base's version slot before a task can accept
        // work. Its growing tail never consumes another base slot.
        bundle.stage_specialist_base(&specialist)?;
        let pin = bundle.store.pin(std::slice::from_ref(&base.checkpoint))?;
        let bytes = serde_json::to_vec(&specialist).map_err(crate::bundle::encoding_error)?;
        let base = Arc::new(
            bundle
                .store
                .reserve_retained_capture(&bytes)?
                .hold(specialist),
        );
        let capture = core
            .capture_state()
            .map_err(crate::bundle::operation_error)?;
        crate::bundle_commit::BundleCommit::publish_branch(
            bundle,
            &capture,
            &checkpoint.input_manifest_ref,
            &branch,
        )?;
        drop(capture);
        bundle.attach_core(&core)?;
        let guard = DurableBranchSessionGuard {
            branch,
            base,
            core,
            epoch: bundle.store.owner_epoch(),
        };
        self.durable_sessions.push(RegisteredSession {
            guard: guard.clone(),
            name: name.into(),
            pin,
            _charge: charge,
        });
        Ok(guard)
    }
    pub fn reopen_registered_candidate(
        &mut self,
        branch: BranchId,
        base: &ObjectRef,
    ) -> Result<DurableBranchSessionGuard, BundleError> {
        let role = self
            .store()?
            .read_record::<SpecialistBaseRecord>(base)?
            .history_role;
        if role == Role::Ordinary {
            return Err(BundleError::HistoryChanged);
        }
        self.reopen_branch(branch, base, "native branch", role)
    }
    pub fn freeze_registered_candidate(
        &mut self,
        guard: &mut DurableBranchSessionGuard,
    ) -> Result<FrozenTaskResult, BundleError> {
        self.freeze_registered_candidate_inner(guard, false)
    }
    /// Reconstruct a frozen task without advancing an unchanged checkpoint.
    pub fn freeze_registered_task_checkpoint(
        &mut self,
        guard: &mut DurableBranchSessionGuard,
    ) -> Result<FrozenTaskResult, BundleError> {
        self.freeze_registered_candidate_inner(guard, true)
    }
    fn freeze_registered_candidate_inner(
        &mut self,
        guard: &mut DurableBranchSessionGuard,
        retain_checkpoint: bool,
    ) -> Result<FrozenTaskResult, BundleError> {
        let persisted = self.persist_durable_branch_inner(guard, false, retain_checkpoint)?;
        let base_ref = persisted.base_ref;
        #[cfg(any(test, feature = "test-util"))]
        crate::test_hook("candidate_frozen_persisted");
        let base = guard.base();
        Ok(FrozenTaskResult {
            base: BranchBinding {
                branch: base.target_branch.clone(),
                head: base.base_head.clone(),
                checkpoint: base.frozen_checkpoint_ref.clone(),
                document_hash: base.base_document_hash.clone(),
            },
            source: if retain_checkpoint {
                self.task_checkpoint_binding(guard.branch_id())?
            } else {
                self.candidate_binding(guard.branch_id())?
            },
            base_ref,
            captured_history: persisted.captured_history,
        })
    }
    pub fn candidate_merge_envelope(
        &self,
        frozen: &FrozenTaskResult,
        id: crate::receipts::CommandId,
    ) -> Result<crate::receipts::CommandEnvelope, BundleError> {
        use kinewright_core::{Query, QueryResult};
        let core = self.candidate_core(frozen.base())?;
        let Event::QueryResult(QueryResult::Snapshot { revision, .. }) = core
            .request(Command::Query(Query::Snapshot))
            .map_err(|e| BundleError::Corrupt(e.to_string()))?
        else {
            return Err(BundleError::HistoryChanged);
        };
        let bundle = self.active_bundle()?;
        let context = self.candidate_merge_context(frozen, &id)?;
        Ok(crate::receipts::CommandEnvelope {
            version: 1,
            action: "edit".into(),
            id,
            project_id: bundle.project_id().clone(),
            target_branch: frozen.base.branch.clone(),
            owner_epoch: bundle.store.owner_epoch(),
            expected_head: Some(frozen.base.head.clone()),
            expected_revision: revision,
            command: Some(kinewright_core::JournalCommand::DoBatch(
                frozen.delta(self)?.iter().cloned().collect(),
            )),
            switch: None,
            context: Some(context),
            lease: None,
            comment_text: None,
            comment_strokes: None,
            evidence: vec![],
        })
    }
    fn candidate_merge_context(
        &self,
        frozen: &FrozenTaskResult,
        id: &crate::receipts::CommandId,
    ) -> Result<crate::operation_actor::CommandContext, BundleError> {
        let bundle = self.active_bundle()?;
        let checkpoint = bundle
            .store
            .read_record::<crate::bundle_records::BranchCheckpoint>(&frozen.source.checkpoint)?;
        let record = bundle
            .store
            .read_record::<crate::operation_history::StoredOwnerRecord>(&checkpoint.owner_ref)?;
        let mut inputs: std::collections::BTreeSet<_> = record
            .edges
            .iter()
            .filter(|r| r.record_kind == crate::bundle_records::RecordKind::InputManifest)
            .cloned()
            .collect();
        inputs.insert(checkpoint.input_manifest_ref.clone());
        Ok(crate::operation_actor::CommandContext {
            creation: crate::operation_history::CreationOrigin {
                branch: frozen.base.branch.clone(),
                batch: crate::bundle_records::BatchId::command(id)?,
            },
            inputs: inputs.into_iter().collect(),
            approval: None,
            transition: None,
        })
    }
    /// Admit a merge before the task retains its pending envelope or checkpoint.
    /// The commit rechecks the same head and revision before it mutates Core.
    pub fn admit_candidate_merge(
        &self,
        frozen: &FrozenTaskResult,
        envelope: &crate::receipts::CommandEnvelope,
    ) -> Result<(), BundleError> {
        let core = self.candidate_core(frozen.base())?;
        self.preview_candidate_merge(&core, frozen, envelope)?;
        Ok(())
    }
    fn preview_candidate_merge(
        &self,
        core: &Core,
        frozen: &FrozenTaskResult,
        envelope: &crate::receipts::CommandEnvelope,
    ) -> Result<(), BundleError> {
        let owner = self.active_bundle()?.attach_core(core)?;
        let cut = owner.freeze().map_err(crate::bundle::operation_error)?;
        let result = (|| {
            if cut.change().revision != envelope.expected_revision {
                return Err(BundleError::HistoryChanged);
            }
            let context = crate::operation_actor::CommandContext {
                creation: crate::operation_history::CreationOrigin {
                    branch: frozen.base.branch.clone(),
                    batch: crate::bundle_records::BatchId::fresh()?,
                },
                inputs: vec![],
                approval: None,
                transition: None,
            };
            let update = owner
                .prepare_merge(&cut, frozen.delta(self)?.iter().cloned().collect(), context)
                .map_err(|error| {
                    use crate::{operation_actor::Error, operation_history::Error as HistoryError};
                    if matches!(
                        error,
                        Error::History(
                            HistoryError::History(
                                kinewright_core::undo::Error::StepTooLarge
                                    | kinewright_core::undo::Error::HistoryFull
                                    | kinewright_core::undo::Error::Capacity
                                    | kinewright_core::undo::Error::DocumentCapacity
                            ) | HistoryError::Capacity(_)
                        )
                    ) {
                        BundleError::RunAdmission(
                            "Candidate history limit reached. Keep this result frozen and use publish_result to report the limit.",
                        )
                    } else {
                        // Temporary pool/workspace pressure stays typed and
                        // retryable; only this fixed history budget is exhausted.
                        crate::bundle::operation_error(error)
                    }
                })?;
            drop(update);
            envelope.encoded_bytes()?;
            Ok(())
        })();
        owner.resume(&cut).map_err(crate::bundle::operation_error)?;
        result
    }
    pub fn commit_candidate(
        &mut self,
        frozen: &FrozenTaskResult,
        envelope: &crate::receipts::CommandEnvelope,
    ) -> Result<crate::receipts::ReceiptLookup, BundleError> {
        if let Some(pending) = &self.pending_candidate {
            if pending.envelope != *envelope {
                return Err(BundleError::Unsettled);
            }
            self.reconcile_active()?;
        }
        let bundle = self.active_bundle()?;
        let previous = bundle.receipt_lookup(envelope)?;
        if previous != crate::receipts::ReceiptLookup::Absent {
            if matches!(
                previous,
                crate::receipts::ReceiptLookup::Committed(_)
                    | crate::receipts::ReceiptLookup::Retired(_)
            ) {
                self.active_bundle_mut()?
                    .settle_matching_receipt(envelope)?;
            }
            return Ok(previous);
        }
        if bundle.unsettled || bundle.pending_command.is_some() {
            return Err(BundleError::Unsettled);
        }
        let core = self.candidate_core(frozen.base())?;
        let mut expected = self.candidate_merge_envelope(frozen, envelope.id.clone())?;
        if self.active_bundle()?.retained_candidate_merge(envelope)? {
            expected.owner_epoch = envelope.owner_epoch;
            expected.expected_revision = envelope.expected_revision;
        }
        if self.candidate_binding(&frozen.source.branch)? != frozen.source || *envelope != expected
        {
            return Err(BundleError::HistoryChanged);
        }
        let target = frozen.base.branch.clone();
        let inputs = self
            .active_bundle()?
            .checkpoint(&target)?
            .input_manifest_ref
            .clone();
        self.preview_candidate_merge(&core, frozen, envelope)?;
        let bundle = self.active_bundle_mut()?;
        let result = crate::bundle_commit::BundleCommit::command_target(
            bundle,
            envelope,
            &core,
            &inputs,
            &[],
            true,
            &target,
        );
        if result.is_ok() {
            // Settlement can retry through reconcile_active without hiding a durable receipt.
            let _ = self.settle_pending_candidate();
        } else if !bundle.unsettled && bundle.pending_command.is_none() {
            self.settle_pending_candidate()?;
        }
        result
    }
    pub(super) fn settle_pending_candidate(&mut self) -> Result<(), BundleError> {
        if self.active_bundle()?.unsettled {
            return Err(BundleError::Unsettled);
        }
        self.pending_candidate = None;
        Ok(())
    }
    pub fn fork_durable_branch(
        &mut self,
        name: &str,
        role: DurableBranchRole,
    ) -> Result<DurableBranchSessionGuard, BundleError> {
        if self
            .core
            .attachment_state()
            .map_err(crate::bundle::operation_error)?
            != kinewright_core::AttachmentState::Live
        {
            return Err(BundleError::Unsettled);
        }
        let charge = self.registration_capacity(name)?;
        if self.dirty()? {
            return Err(BundleError::HistoryChanged);
        }
        let bundle = self.active_bundle()?;
        let target = bundle.manual_branch_id().clone();
        let checkpoint = bundle.checkpoint(&target)?;
        let base = BranchBinding {
            branch: target.clone(),
            head: checkpoint.head_id.clone(),
            checkpoint: bundle.checkpoint_reference(&target)?,
            document_hash: checkpoint.document_ref.sha256.clone(),
        };
        let branch = BranchId::fresh()?;
        let (core, _) = self
            .core
            .fork(
                role.history_role(),
                crate::operation_history::CreationOrigin {
                    branch: branch.clone(),
                    batch: crate::bundle_records::BatchId::fresh()?,
                },
                false,
            )
            .map_err(crate::bundle::operation_error)?;
        self.register_fork(core, branch, &base, role.history_role(), name, charge)
    }
    pub fn reopen_durable_branch(
        &mut self,
        branch: BranchId,
        base_ref: &ObjectRef,
        name: &str,
        role: DurableBranchRole,
    ) -> Result<DurableBranchSessionGuard, BundleError> {
        self.reopen_branch(branch, base_ref, name, role.history_role())
    }
    fn reopen_branch(
        &mut self,
        branch: BranchId,
        base_ref: &ObjectRef,
        name: &str,
        role: Role,
    ) -> Result<DurableBranchSessionGuard, BundleError> {
        let session = self.prepare_reopened_branch(branch, base_ref, name, role)?;
        let guard = session.guard.clone();
        self.durable_sessions.push(session);
        Ok(guard)
    }
    /// Register a native task and its candidate only after both journals replay.
    pub fn reopen_native_task_pair(
        &mut self,
        task: BranchId,
        task_base: &ObjectRef,
        candidate: BranchId,
        candidate_base: &ObjectRef,
    ) -> Result<(), BundleError> {
        if task == candidate {
            return Err(BundleError::HistoryChanged);
        }
        self.durable_sessions
            .try_reserve(2)
            .map_err(|_| BundleError::Capacity("durable session registrations"))?;
        let mut prepared = Vec::new();
        for (branch, base, role) in [
            (task, task_base, Role::Task),
            (candidate, candidate_base, Role::Candidate),
        ] {
            if !self
                .durable_sessions
                .iter()
                .any(|session| session.guard.branch == branch)
            {
                if u64::try_from(self.durable_sessions.len() + prepared.len())
                    .map_err(|_| BundleError::Capacity("durable session registrations"))?
                    >= self.store()?.limits().objects
                {
                    return Err(BundleError::Capacity("durable session registrations"));
                }
                prepared.push(self.prepare_reopened_branch(branch, base, "native branch", role)?);
            }
        }
        self.durable_sessions.extend(prepared);
        Ok(())
    }
    fn prepare_reopened_branch(
        &mut self,
        branch: BranchId,
        base_ref: &ObjectRef,
        name: &str,
        role: Role,
    ) -> Result<RegisteredSession, BundleError> {
        if self
            .core
            .attachment_state()
            .map_err(crate::bundle::operation_error)?
            != kinewright_core::AttachmentState::Live
            || self.active_bundle()?.unsettled
        {
            return Err(BundleError::Unsettled);
        }
        if self.active_bundle()?.manual_branch_id() == &branch
            || self
                .durable_sessions
                .iter()
                .any(|r| r.guard.branch == branch)
        {
            return Err(BundleError::Contention(
                "durable branch already has a live session".into(),
            ));
        }
        let charge = self.registration_capacity(name)?;
        let bundle = self.active_bundle()?;
        let base_ref = bundle.current_candidate_base(&branch, base_ref)?;
        let base = bundle
            .store
            .read_record::<SpecialistBaseRecord>(&base_ref)?;
        if base.history_role != role || bundle.checkpoint(&branch)?.role != role {
            return Err(BundleError::HistoryChanged);
        }
        let checkpoint = bundle
            .store
            .read_record::<crate::bundle_records::BranchCheckpoint>(&base.frozen_checkpoint_ref)?;
        if checkpoint.branch_id != base.target_branch
            || checkpoint.head_id != base.base_head
            || checkpoint.document_ref.sha256 != base.base_document_hash
            || checkpoint.document_ref != base.original_baseline_ref
            || !bundle.branches.contains_key(&base.target_branch)
        {
            return Err(BundleError::HistoryChanged);
        }
        let current = bundle.checkpoint(&branch)?;
        let record = bundle
            .store
            .read_record::<crate::operation_history::StoredOwnerRecord>(&current.owner_ref)?;
        if record.baseline != base.original_baseline_ref {
            return Err(BundleError::HistoryChanged);
        }
        drop(record);
        let snapshot = current.open(&bundle.store, bundle.history_pool())?;
        let core = Core::open(snapshot, branch.clone(), current.revision)
            .map_err(crate::bundle::operation_error)?;
        bundle.attach_core(&core)?;
        let pin = bundle
            .store
            .pin(&[base_ref.clone(), bundle.checkpoint_reference(&branch)?])?;
        Ok(RegisteredSession {
            guard: DurableBranchSessionGuard {
                branch,
                base: Arc::new(base),
                core,
                epoch: bundle.store.owner_epoch(),
            },
            name: name.into(),
            pin,
            _charge: charge,
        })
    }
    pub(super) fn registered(
        &self,
        guard: &DurableBranchSessionGuard,
    ) -> Result<usize, BundleError> {
        self.durable_sessions
            .iter()
            .position(|r| {
                r.guard.branch == guard.branch
                    && r.guard.core.actor_identity() == guard.core.actor_identity()
                    && r.guard.epoch == guard.epoch
            })
            .ok_or_else(|| BundleError::Identity("unregistered durable session".into()))
    }
    pub fn durable_branch_baseline(
        &self,
        guard: &DurableBranchSessionGuard,
    ) -> Result<Arc<Document>, BundleError> {
        self.registered(guard)?;
        guard
            .core
            .baseline_document()
            .map_err(|error| BundleError::Corrupt(error.to_string()))
    }
    pub fn durable_branch_history(
        &self,
        guard: &DurableBranchSessionGuard,
    ) -> Result<crate::operation_history::OwnerRead, BundleError> {
        self.registered(guard)?;
        let bundle = self.registered_bundle(guard)?;
        if bundle.unsettled {
            return Err(BundleError::Unsettled);
        }
        bundle.branch_history(&guard.branch)
    }
    pub fn persist_durable_branch(
        &mut self,
        guard: &mut DurableBranchSessionGuard,
    ) -> Result<ObjectRef, BundleError> {
        self.persist_durable_branch_inner(guard, true, false)
            .map(|saved| saved.base_ref)
    }
    /// Stage a task checkpoint for the matching H1 root transaction.
    /// Unchanged checkpoints retain their exact identity.
    pub fn persist_registered_task_checkpoint(
        &mut self,
        guard: &mut DurableBranchSessionGuard,
    ) -> Result<PersistedTaskCheckpoint, BundleError> {
        self.persist_durable_branch_inner(guard, true, true)
    }
    /// Measure the exact history/base projection without admitting or publishing it.
    #[cfg(any(test, feature = "test-util"))]
    pub fn pending_task_history_profile(
        &self,
        core: &Core,
    ) -> Result<crate::orchestration_retention::PendingTaskHistoryProfile, BundleError> {
        let _reading = crate::run_admission::reading(self.store()?);
        let guard = &self
            .durable_sessions
            .iter()
            .find(|session| session.guard.core.actor_identity() == core.actor_identity())
            .ok_or(BundleError::HistoryChanged)?
            .guard;
        let history = core
            .export_history()
            .map_err(crate::bundle::operation_error)?;
        let base = guard.base();
        crate::orchestration_retention::measure_pending_task_history(
            &history,
            base,
            self.store()?.limits(),
        )
    }
    #[allow(
        clippy::too_many_lines,
        reason = "one registered Core fence spans checkpoint reuse, publication and resume"
    )]
    fn persist_durable_branch_inner(
        &mut self,
        guard: &mut DurableBranchSessionGuard,
        resume: bool,
        retain_checkpoint: bool,
    ) -> Result<PersistedTaskCheckpoint, BundleError> {
        let index = self.registered(guard)?;
        if !retain_checkpoint {
            self.registered_bundle(guard)?
                .refuse_unpaired_task_checkpoint(guard.branch_id())?;
        }
        let canonical = self.durable_sessions[index].guard.clone();
        let owner = self
            .registered_bundle(&canonical)?
            .attach_core(&canonical.core)?;
        let cut = owner.freeze().map_err(crate::bundle::operation_error)?;
        let captured_history = crate::edit_audit::CapturedHistory::from_cut(&cut);
        let result = (|| {
            let bundle = self.registered_bundle_mut(&canonical)?;
            if bundle.unsettled {
                return Err(BundleError::Unsettled);
            }
            let reference = bundle.stage_specialist_base(canonical.base())?;
            let checkpoint = bundle.checkpoint(&canonical.branch)?;
            let inputs = checkpoint.input_manifest_ref.clone();
            let same = if let Some(staged) = bundle.task_checkpoints.get(&canonical.branch) {
                staged.checkpoint.identity == cut.identity() && staged.base_ref == reference
            } else {
                checkpoint.identity == cut.identity()
                    && bundle.root().is_some_and(|root| {
                        root.candidate_recovery
                            .get(&canonical.branch)
                            .is_some_and(|binding| binding.base_ref == reference)
                    })
            };
            if !same {
                if retain_checkpoint {
                    crate::bundle_commit::BundleCommit::stage_task_checkpoint(
                        bundle,
                        &canonical
                            .core
                            .capture_state()
                            .map_err(crate::bundle::operation_error)?,
                        &inputs,
                        &canonical.branch,
                        &reference,
                    )?;
                } else {
                    bundle.save_frozen_branch_with_base(
                        &canonical.core,
                        &inputs,
                        &cut,
                        &canonical.branch,
                        Some(&reference),
                    )?;
                }
            }
            let checkpoint = bundle.task_checkpoints.get(&canonical.branch).map_or_else(
                || bundle.checkpoint_reference(&canonical.branch),
                |staged| Ok(staged.reference.clone()),
            )?;
            let pin = bundle.store.pin(&[reference.clone(), checkpoint])?;
            self.durable_sessions[index].pin = pin;
            *guard = canonical.clone();
            Ok(reference)
        })();
        if (resume || result.is_err()) && !self.registered_bundle(&canonical)?.unsettled {
            owner.resume(&cut).map_err(crate::bundle::operation_error)?;
        }
        result.map(|base_ref| PersistedTaskCheckpoint {
            base_ref,
            captured_history,
        })
    }
    /// Fence a persisted session. The agent close route persists its full proposal first.
    pub fn close_durable_branch(
        &mut self,
        guard: &DurableBranchSessionGuard,
    ) -> Result<(), BundleError> {
        self.close_durable_branch_inner(guard, false)
    }
    /// Seal an already persisted task without replacing its frozen checkpoint.
    pub fn close_registered_task_checkpoint(
        &mut self,
        guard: &DurableBranchSessionGuard,
    ) -> Result<(), BundleError> {
        self.close_durable_branch_inner(guard, true)
    }
    fn close_durable_branch_inner(
        &mut self,
        guard: &DurableBranchSessionGuard,
        retain_checkpoint: bool,
    ) -> Result<(), BundleError> {
        let index = self.registered(guard)?;
        if self
            .core
            .attachment_state()
            .map_err(crate::bundle::operation_error)?
            != kinewright_core::AttachmentState::Live
            || self.registered_bundle(guard)?.unsettled
        {
            return Err(BundleError::Unsettled);
        }
        let bundle = self.registered_bundle_mut(guard)?;
        let owner = bundle.attach_core(&guard.core)?;
        let cut = owner.freeze().map_err(crate::bundle::operation_error)?;
        if cut.identity() != bundle.checkpoint(&guard.branch)?.identity {
            owner.resume(&cut).map_err(crate::bundle::operation_error)?;
            return Err(BundleError::Identity(
                "persist this session before closing it".into(),
            ));
        }
        let inputs = bundle.checkpoint(&guard.branch)?.input_manifest_ref.clone();
        if !retain_checkpoint
            && let Err(error) = bundle.save_frozen_branch(&guard.core, &inputs, &cut, &guard.branch)
        {
            if !bundle.unsettled {
                owner.resume(&cut).map_err(crate::bundle::operation_error)?;
            }
            return Err(error);
        }
        let settled = owner.capture().map_err(crate::bundle::operation_error)?;
        owner
            .retire(&settled)
            .map_err(crate::bundle::operation_error)?;
        self.durable_sessions.remove(index);
        Ok(())
    }
}

#[cfg(all(test, target_os = "linux"))]
mod fu2_tests {
    use super::*;
    use crate::{
        receipts::{CommandId, ReceiptLookup},
        storage_budget::StorageLimits,
    };
    use kinewright_core::{AttachmentState, Operation, Track, TrackId, TrackKind};

    #[test]
    fn session_history_uses_the_ordinary_pool_above_the_candidate_bound() {
        let temp = kinewright_media::test_support::TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "ordinary-session-history",
        );
        let core = Core::for_test(Document {
            tracks: vec![Track {
                name: None,
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            }],
            markers: vec![kinewright_core::Marker {
                id: kinewright_core::MarkerId(1),
                position: kinewright_core::TimeCode(0),
                label: "x".repeat(150_000),
                color_token: 0,
            }],
            ..Document::default()
        })
        .unwrap();
        let mut owner =
            Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
        owner
            .first_save(&temp.path("project.kinewright"), StorageLimits::default())
            .unwrap();
        let mut session = owner
            .fork_durable_branch("session", DurableBranchRole::Session)
            .unwrap();
        for edit in 0..20 {
            session
                .core()
                .request(Command::Do(Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: edit % 2 == 0,
                }))
                .unwrap();
        }
        let expected = session.core().export_history().unwrap();
        assert_eq!(
            expected.history().policy,
            kinewright_core::undo::RetentionPolicy::NeverFold,
            "SESSION_DELTA_NEVER_FOLDS"
        );
        assert_eq!(
            expected.creation_sizing().kind,
            crate::operation_history::CreationKind::Task
        );
        owner.persist_durable_branch(&mut session).unwrap();
        let bundle = owner.active_bundle().unwrap();
        let checkpoint = bundle.checkpoint(session.branch_id()).unwrap();
        assert_eq!(checkpoint.role, Role::Task);
        assert_eq!(
            bundle
                .branch_history(session.branch_id())
                .unwrap()
                .into_capture()
                .unwrap(),
            expected,
            "SESSION_PERSIST_KEEPS_THE_COMPLETE_OWNER"
        );
    }

    #[test]
    fn native_task_keeps_admitted_history_above_ordinary_live_limit() {
        let temp = kinewright_media::test_support::TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "native-history-floor",
        );
        let document = Document {
            tracks: vec![Track {
                name: None,
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            }],
            markers: vec![kinewright_core::Marker {
                id: kinewright_core::MarkerId(1),
                position: kinewright_core::TimeCode(0),
                label: "x".repeat(150_000),
                color_token: 0,
            }],
            ..Document::default()
        };
        let core = Core::for_test(document).unwrap();
        let mut owner =
            Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
        owner
            .first_save(&temp.path("project.kinewright"), StorageLimits::default())
            .unwrap();
        let mut candidate = owner
            .fork_durable_branch(
                "candidate",
                crate::editing_session::DurableBranchRole::RunCandidate,
            )
            .unwrap();
        owner.persist_durable_branch(&mut candidate).unwrap();
        let base = owner.candidate_binding(candidate.branch_id()).unwrap();
        let mut task = owner.fork_registered_from_candidate(&base, "task").unwrap();
        for edit in 0..70 {
            assert!(matches!(
                task.core()
                    .request(Command::DoBatch(vec![Operation::SetTrackSyncLock {
                        track: TrackId(1),
                        locked: edit % 2 == 0,
                    }]))
                    .unwrap(),
                Event::DocumentChanged { .. }
            ));
        }
        let expected = task.core().export_history().unwrap();
        assert_eq!(
            expected.history().undo.len(),
            70,
            "NATIVE_ADMISSION_MUST_NOT_FLOOR"
        );
        assert_eq!(
            expected.history().policy,
            kinewright_core::undo::RetentionPolicy::NeverFold
        );
        owner.persist_registered_task_checkpoint(&mut task).unwrap();
        let staged = &owner.active_bundle().unwrap().task_checkpoints[task.branch_id()];
        assert_eq!(
            staged.checkpoint.identity,
            expected.identity().unwrap(),
            "NATIVE_STAGED_OWNER_IS_THE_EXACT_TASK"
        );
        assert_eq!(staged.checkpoint.role, Role::Task);
        assert_eq!(task.core().export_history().unwrap(), expected);
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one candidate settlement and manual-history oracle across issuer retirement"
    )]
    fn fu2_candidate_settlement_recovers_retired_receipt_without_masking_commit() {
        let root = kinewright_media::test_support::workspace_target_dir().join("fu2-unit");
        let temp =
            kinewright_media::test_support::TempDirectory::new_in(&root, "retired-candidate");
        let core = Core::for_test(Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                name: None,
                sync_lock: true,
                clips: vec![],
            }],
            ..Document::default()
        })
        .unwrap();
        let mut owner =
            Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
        owner
            .first_save(&temp.path("project.kinewright"), StorageLimits::default())
            .unwrap();
        let mut candidate = owner
            .fork_durable_branch(
                "candidate",
                crate::editing_session::DurableBranchRole::RunCandidate,
            )
            .unwrap();
        owner.persist_durable_branch(&mut candidate).unwrap();
        let binding = owner.candidate_binding(candidate.branch_id()).unwrap();
        let mut specialist = owner
            .fork_registered_from_candidate(&binding, "Editor")
            .unwrap();
        specialist
            .core()
            .request(Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: false,
            }))
            .unwrap();
        let frozen = owner.freeze_registered_candidate(&mut specialist).unwrap();
        let bundle = owner.active_bundle_mut().unwrap();
        let inputs = bundle
            .checkpoint(bundle.manual_branch_id())
            .unwrap()
            .input_manifest_ref
            .clone();
        let issuer = bundle.issue_issuer(&core, &inputs, 1).unwrap();
        let envelope = owner
            .candidate_merge_envelope(
                &frozen,
                CommandId {
                    issuer: issuer.clone(),
                    sequence: 1,
                },
            )
            .unwrap();
        core.request(Command::Do(Operation::AddSolidClip {
            track: TrackId(1),
            timeline_start: kinewright_core::TimeCode(0),
            duration: kinewright_core::TimeCode(100),
            color: kinewright_core::SolidColor::default(),
        }))
        .unwrap();
        let manual = core.snapshot_identity().unwrap();
        assert_eq!(
            core.export_history().unwrap().history().document.tracks[0]
                .clips
                .len(),
            1
        );
        let result = crate::io_observer::fail_once("candidate_settlement", || {
            owner.commit_candidate(&frozen, &envelope)
        });
        let retry = crate::io_observer::fail_once("candidate_settlement", || {
            owner.commit_candidate(&frozen, &envelope)
        });
        assert_eq!(
            candidate.core().attachment_state().unwrap(),
            AttachmentState::Frozen
        );
        assert!(owner.active_bundle().unwrap().pending_publication.is_some());
        assert_eq!(core.snapshot_identity().unwrap(), manual);
        let expected = owner
            .active_bundle()
            .unwrap()
            .branch_history(candidate.branch_id())
            .unwrap();
        assert!(expected.history().document().tracks[0].clips.is_empty());
        assert_ne!(expected.identity().unwrap(), manual);
        owner
            .reconcile_active()
            .expect("committed private update must settle before metadata can retire its receipt");
        let bundle = owner.active_bundle_mut().unwrap();
        let inputs = bundle
            .checkpoint(bundle.manual_branch_id())
            .unwrap()
            .input_manifest_ref
            .clone();
        bundle
            .acknowledge_issuer(&issuer, 1, &core, &inputs)
            .unwrap();
        bundle
            .retire_issuer(&issuer, false, &core, &inputs)
            .unwrap();
        let ReceiptLookup::Retired(retired) = bundle.receipt_lookup(&envelope).unwrap() else {
            panic!("retired receipt");
        };
        owner
            .reconcile_active()
            .expect("a retired committed receipt must settle");
        assert_eq!(
            candidate.core().attachment_state().unwrap(),
            AttachmentState::Live
        );
        assert!(owner.active_bundle().unwrap().pending_publication.is_none());
        assert_eq!(
            candidate.core().snapshot_identity().unwrap(),
            expected.identity().unwrap()
        );
        assert_eq!(core.snapshot_identity().unwrap(), manual);
        owner.save_active().unwrap();
        assert_eq!(core.snapshot_identity().unwrap(), manual);
        assert!(result.is_err(), "ACK_WAITS_FOR_PRIVATE_SHADOW_SETTLEMENT");
        assert!(
            matches!(retry, Err(_) | Ok(ReceiptLookup::Pending)),
            "ACK_RETRY_WAITS_FOR_PRIVATE_SHADOW_SETTLEMENT: {retry:?}"
        );
        assert_eq!(
            owner.commit_candidate(&frozen, &envelope).unwrap(),
            ReceiptLookup::Retired(retired),
            "SETTLED_RETRY_RECOVERS_THE_ORIGINAL_RETIRED_RECEIPT"
        );
    }
}
