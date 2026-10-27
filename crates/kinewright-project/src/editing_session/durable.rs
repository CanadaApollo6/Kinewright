//! Registration belongs to the project owner; dropping a caller's guard does not close its actor.
use super::{BundleError, Core, Ja1EditingSession, ObjectRef, StorageReservation};
use crate::bundle_records::{BranchId, SpecialistBaseRecord};
use kinewright_core::{Command, Document, Event, HistorySnapshot};
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

/// Constructed only by freezing a registered, persisted specialist.
#[derive(Debug, Clone)]
pub struct FrozenTaskResult {
    base: BranchBinding,
    source: BranchBinding,
    base_ref: ObjectRef,
    delta: Vec<kinewright_core::Operation>,
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
    #[must_use]
    pub fn delta(&self) -> &[kinewright_core::Operation] {
        &self.delta
    }
}

pub(super) struct PendingCandidateCommit {
    core: Core,
    cut: kinewright_core::AcceptedCut,
    envelope: crate::receipts::CommandEnvelope,
}

#[derive(Clone)]
pub struct DurableBranchSessionGuard {
    pub(super) branch: BranchId,
    pub(super) base: Arc<StorageReservation<SpecialistBaseRecord>>,
    core: Core,
    pub(super) epoch: u64,
    floor_source: Arc<StorageReservation<HistorySnapshot>>,
}
pub(super) struct RegisteredSession {
    pub(super) digest_cache: std::cell::RefCell<super::HistoryDigestCache>,
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
        self.store()?.reserve(0, name.len() as u64 + 512, 0, false)
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
            document_hash: checkpoint.document_hash.clone(),
        })
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
            .export_history()
            .map_err(crate::bundle::history_error)?
            != *bundle.branch_history(&binding.branch)?
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
        self.candidate_core(base)?;
        let charge = self.registration_capacity(name)?;
        let bundle = self.active_bundle_mut()?;
        let checkpoint = bundle.checkpoint(&base.branch)?.clone();
        let history = bundle.branch_history(&base.branch)?;
        let core = Core::spawn((*history.document).clone())
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let frozen = core
            .export_history()
            .map_err(crate::bundle::history_error)?;
        let specialist = SpecialistBaseRecord {
            target_branch: base.branch.clone(),
            base_head: base.head.clone(),
            base_document_hash: base.document_hash.clone(),
            original_baseline_ref: checkpoint.document_ref,
            frozen_checkpoint_ref: base.checkpoint.clone(),
            full_applied_delta: vec![],
        };
        let pin = bundle.store.pin(std::slice::from_ref(&base.checkpoint))?;
        let encoded =
            serde_json::to_vec(&specialist).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let specialist = Arc::new(
            bundle
                .store
                .reserve_retained_capture(&encoded)?
                .hold(specialist),
        );
        let encoded =
            serde_json::to_vec(&frozen).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let floor_source = Arc::new(
            bundle
                .store
                .reserve_retained_capture(&encoded)?
                .hold(frozen),
        );
        let branch = bundle.fork_branch(&base.branch)?;
        bundle.attach_core(&core)?;
        let guard = DurableBranchSessionGuard {
            branch,
            base: specialist,
            core,
            epoch: bundle.store.owner_epoch(),
            floor_source,
        };
        self.durable_sessions.push(RegisteredSession {
            digest_cache: std::cell::RefCell::new(None),
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
        self.reopen_durable_branch(branch, base, "candidate")
    }
    pub fn freeze_registered_candidate(
        &mut self,
        guard: &mut DurableBranchSessionGuard,
    ) -> Result<FrozenTaskResult, BundleError> {
        let base_ref = self.persist_durable_branch_inner(guard, false)?;
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
            source: self.candidate_binding(guard.branch_id())?,
            base_ref,
            delta: base.full_applied_delta.clone(),
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
                frozen.delta.clone(),
            )),
            switch: None,
            manual: None,
            lease: None,
            comment_text: None,
            comment_strokes: None,
            evidence: vec![],
            transition: None,
        })
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
            self.settle_pending_candidate(true)?;
        }
        let bundle = self.active_bundle()?;
        let previous = bundle.receipt_lookup(envelope)?;
        if previous != crate::receipts::ReceiptLookup::Absent {
            return Ok(previous);
        }
        let core = self.candidate_core(frozen.base())?;
        if self.candidate_binding(&frozen.source.branch)? != frozen.source
            || *envelope != self.candidate_merge_envelope(frozen, envelope.id.clone())?
        {
            return Err(BundleError::HistoryChanged);
        }
        let target = frozen.base.branch.clone();
        let attachment = self.active_bundle()?.attach_core(&core)?;
        let cut = attachment.freeze().map_err(crate::bundle::history_error)?;
        let encoded = serde_json::to_vec(&(cut.history(), envelope))
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let charge = match self.store()?.reserve_retained_capture(&encoded) {
            Ok(charge) => charge,
            Err(error) => {
                attachment
                    .resume(&cut)
                    .map_err(crate::bundle::history_error)?;
                return Err(error);
            }
        };
        self.pending_candidate = Some(charge.hold(PendingCandidateCommit {
            core: core.clone(),
            cut,
            envelope: envelope.clone(),
        }));
        let bundle = self.active_bundle_mut()?;
        let inputs = bundle.checkpoint(&target)?.input_manifest_ref.clone();
        let result = crate::bundle_commit::BundleCommit::command_target(
            bundle,
            envelope,
            &core,
            &inputs,
            &[],
            false,
            &target,
        );
        if result.is_ok() {
            self.settle_pending_candidate(false)?;
        }
        result
    }
    pub(super) fn settle_pending_candidate(&mut self, reconcile: bool) -> Result<(), BundleError> {
        let Some(pending) = &self.pending_candidate else {
            return Ok(());
        };
        let core = pending.core.clone();
        let cut = pending.cut.clone();
        let envelope = pending.envelope.clone();
        let bundle = self.active_bundle_mut()?;
        if reconcile {
            bundle.reconcile_command(&core)?;
        }
        let attachment = bundle.attach_core(&core)?;
        let cut = match bundle.receipt_lookup(&envelope)? {
            crate::receipts::ReceiptLookup::Committed(_) => {
                let after = bundle.branch_history(&envelope.target_branch)?;
                attachment
                    .install_frozen_checkpoint(&cut, (*after).clone())
                    .map_err(crate::bundle::history_error)?
            }
            crate::receipts::ReceiptLookup::Absent => cut,
            _ => return Err(BundleError::Unsettled),
        };
        attachment
            .resume(&cut)
            .map_err(crate::bundle::history_error)?;
        self.pending_candidate = None;
        Ok(())
    }
    pub fn fork_durable_branch(
        &mut self,
        name: &str,
    ) -> Result<DurableBranchSessionGuard, BundleError> {
        if self
            .core
            .attachment_state()
            .map_err(crate::bundle::history_error)?
            != kinewright_core::AttachmentState::Live
        {
            return Err(BundleError::Unsettled);
        }
        let charge = self.registration_capacity(name)?;
        let live = self
            .core
            .export_history()
            .map_err(crate::bundle::history_error)?;
        if self.dirty()? {
            return Err(BundleError::HistoryChanged);
        }
        let core = Core::spawn((*live.document).clone())
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let bundle = self.active_bundle_mut()?;
        let target = bundle.manual_branch_id().clone();
        if live != *bundle.branch_history(&target)? {
            return Err(BundleError::HistoryChanged);
        }
        let checkpoint = bundle.checkpoint(&target)?.clone();
        let base = SpecialistBaseRecord {
            target_branch: target.clone(),
            base_head: checkpoint.head_id,
            base_document_hash: checkpoint.document_hash,
            original_baseline_ref: checkpoint.document_ref,
            frozen_checkpoint_ref: bundle.checkpoint_reference(&target)?,
            full_applied_delta: vec![],
        };
        let pin = bundle
            .store
            .pin(std::slice::from_ref(&base.frozen_checkpoint_ref))?;
        let encoded = serde_json::to_vec(&base).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let base = Arc::new(bundle.store.reserve_retained_capture(&encoded)?.hold(base));
        let attachment = bundle.attach_core(&core)?;
        let candidate_cut = attachment.freeze().map_err(crate::bundle::history_error)?;
        if !candidate_cut.history().batches.is_empty() {
            attachment
                .resume(&candidate_cut)
                .map_err(crate::bundle::history_error)?;
            return Err(BundleError::HistoryChanged);
        }
        let encoded = serde_json::to_vec(candidate_cut.history())
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let floor_source = Arc::new(
            bundle
                .store
                .reserve_retained_capture(&encoded)?
                .hold(candidate_cut.history().clone()),
        );
        let branch = match bundle.fork_branch(&target) {
            Ok(branch) => branch,
            Err(error) => {
                attachment
                    .seal(&candidate_cut)
                    .map_err(crate::bundle::history_error)?;
                return Err(error);
            }
        };
        let guard = DurableBranchSessionGuard {
            branch,
            floor_source,
            base,
            core: core.clone(),
            epoch: bundle.store.owner_epoch(),
        };
        self.durable_sessions.push(RegisteredSession {
            digest_cache: std::cell::RefCell::new(None),
            guard: guard.clone(),
            name: name.into(),
            pin,
            _charge: charge,
        });
        attachment
            .resume(&candidate_cut)
            .map_err(crate::bundle::history_error)?;
        Ok(guard)
    }
    pub fn reopen_durable_branch(
        &mut self,
        branch: BranchId,
        base_ref: &ObjectRef,
        name: &str,
    ) -> Result<DurableBranchSessionGuard, BundleError> {
        if self
            .core
            .attachment_state()
            .map_err(crate::bundle::history_error)?
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
        let base = bundle.store.read_record::<SpecialistBaseRecord>(base_ref)?;
        let checkpoint = bundle
            .store
            .read_record::<crate::bundle_records::BranchCheckpoint>(&base.frozen_checkpoint_ref)?;
        if checkpoint.branch_id != base.target_branch
            || checkpoint.head_id != base.base_head
            || checkpoint.document_hash != base.base_document_hash
            || checkpoint.document_ref != base.original_baseline_ref
            || !bundle.branches.contains_key(&base.target_branch)
        {
            return Err(BundleError::HistoryChanged);
        }
        let history = bundle.branch_history(&branch)?;
        let core =
            Core::spawn(Document::default()).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        bundle
            .attach_core(&core)?
            .restore_history((*history).clone())
            .map_err(crate::bundle::history_error)?;
        let attachment = bundle.attach_core(&core)?;
        let candidate_cut = attachment.freeze().map_err(crate::bundle::history_error)?;
        let pin = bundle
            .store
            .pin(&[base_ref.clone(), bundle.checkpoint_reference(&branch)?])?;
        let encoded = serde_json::to_vec(candidate_cut.history())
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let floor_source = Arc::new(
            bundle
                .store
                .reserve_retained_capture(&encoded)?
                .hold(candidate_cut.history().clone()),
        );
        let guard = DurableBranchSessionGuard {
            branch,
            floor_source,
            base: Arc::new(base),
            core: core.clone(),
            epoch: bundle.store.owner_epoch(),
        };
        self.durable_sessions.push(RegisteredSession {
            digest_cache: std::cell::RefCell::new(None),
            guard: guard.clone(),
            name: name.into(),
            pin,
            _charge: charge,
        });
        attachment
            .resume(&candidate_cut)
            .map_err(crate::bundle::history_error)?;
        Ok(guard)
    }
    fn registered(&self, guard: &DurableBranchSessionGuard) -> Result<usize, BundleError> {
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
        Ok(Arc::new(
            (*self
                .registered_bundle(guard)?
                .store()
                .read_record::<Document>(&guard.base.original_baseline_ref)?)
            .clone(),
        ))
    }
    pub fn durable_branch_history(
        &self,
        guard: &DurableBranchSessionGuard,
    ) -> Result<StorageReservation<HistorySnapshot>, BundleError> {
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
        self.persist_durable_branch_inner(guard, true)
    }
    fn persist_durable_branch_inner(
        &mut self,
        guard: &mut DurableBranchSessionGuard,
        resume: bool,
    ) -> Result<ObjectRef, BundleError> {
        let index = self.registered(guard)?;
        let canonical = self.durable_sessions[index].guard.clone();
        if self
            .core
            .attachment_state()
            .map_err(crate::bundle::history_error)?
            != kinewright_core::AttachmentState::Live
            || self.registered_bundle(&canonical)?.unsettled
        {
            return Err(BundleError::Unsettled);
        }
        let owner = self
            .registered_bundle(&canonical)?
            .attach_core(&canonical.core)?;
        let cut = owner.freeze().map_err(crate::bundle::history_error)?;
        let result = (|| {
            let discarded = usize::try_from(
                cut.history()
                    .floor_offset
                    .checked_sub(canonical.floor_source.floor_offset)
                    .ok_or(BundleError::HistoryChanged)?,
            )
            .map_err(|_| BundleError::Capacity("specialist floor offset"))?;
            let original = if discarded == 0 {
                (**canonical.floor_source).clone()
            } else {
                kinewright_core::HistoryTransition::floor(
                    (**canonical.floor_source).clone(),
                    discarded,
                )
                .map_err(crate::bundle::history_error)?
                .after()
                .clone()
            };
            if original.baseline != cut.history().baseline {
                return Err(BundleError::HistoryChanged);
            }
            let mut full_delta = specialist_floor_delta_from_baseline(
                canonical.base(),
                &original,
                &*self.durable_branch_baseline(&canonical)?,
            )?;
            full_delta.extend(cut.history().applied_operations());
            let mut base = canonical.base().clone();
            base.full_applied_delta = full_delta;
            let base_bytes =
                serde_json::to_vec(&base).map_err(|e| BundleError::Corrupt(e.to_string()))?;
            let history_bytes = serde_json::to_vec(cut.history())
                .map_err(|e| BundleError::Corrupt(e.to_string()))?;
            let source = self.registered_bundle(&canonical)?.store();
            let base = Arc::new(source.reserve_retained_capture(&base_bytes)?.hold(base));
            let floor_source = Arc::new(
                source
                    .reserve_retained_capture(&history_bytes)?
                    .hold(cut.history().clone()),
            );
            *guard = DurableBranchSessionGuard {
                base,
                floor_source,
                ..canonical.clone()
            };
            self.durable_sessions[index].guard = guard.clone();
            let bundle = self.registered_bundle_mut(guard)?;
            let before = (bundle.evidence.len(), bundle.pending_evidence.len());
            let reference = bundle.stage_specialist_base(guard.base())?;
            let inputs = bundle.checkpoint(&guard.branch)?.input_manifest_ref.clone();
            if let Err(error) = bundle.save_frozen_branch(&guard.core, &inputs, &cut, &guard.branch)
            {
                if !bundle.unsettled {
                    bundle.evidence.truncate(before.0);
                    bundle.pending_evidence.truncate(before.1);
                }
                return Err(error);
            }
            let pin = bundle.store.pin(&[
                reference.clone(),
                bundle.checkpoint_reference(&guard.branch)?,
            ])?;
            self.durable_sessions[index].pin = pin;
            Ok(reference)
        })();
        let current = owner.capture().map_err(crate::bundle::history_error)?;
        if resume || result.is_err() {
            owner
                .resume(&current)
                .map_err(crate::bundle::history_error)?;
        }
        result
    }
    /// Fence a persisted session. The agent close route persists its full proposal first.
    pub fn close_durable_branch(
        &mut self,
        guard: &DurableBranchSessionGuard,
    ) -> Result<(), BundleError> {
        let index = self.registered(guard)?;
        if self
            .core
            .attachment_state()
            .map_err(crate::bundle::history_error)?
            != kinewright_core::AttachmentState::Live
            || self.registered_bundle(guard)?.unsettled
        {
            return Err(BundleError::Unsettled);
        }
        let bundle = self.registered_bundle_mut(guard)?;
        let owner = bundle.attach_core(&guard.core)?;
        let cut = owner.freeze().map_err(crate::bundle::history_error)?;
        if cut.history() != &*bundle.branch_history(&guard.branch)? {
            owner.resume(&cut).map_err(crate::bundle::history_error)?;
            return Err(BundleError::Identity(
                "persist this session before closing it".into(),
            ));
        }
        let inputs = bundle.checkpoint(&guard.branch)?.input_manifest_ref.clone();
        if let Err(error) = bundle.save_frozen_branch(&guard.core, &inputs, &cut, &guard.branch) {
            if !bundle.unsettled {
                owner.resume(&cut).map_err(crate::bundle::history_error)?;
            }
            return Err(error);
        }
        let settled = owner.capture().map_err(crate::bundle::history_error)?;
        owner.seal(&settled).map_err(crate::bundle::history_error)?;
        self.durable_sessions.remove(index);
        Ok(())
    }
}

pub fn specialist_floor_delta_from_baseline(
    base: &SpecialistBaseRecord,
    history: &kinewright_core::HistorySnapshot,
    baseline: &Document,
) -> Result<Vec<kinewright_core::Operation>, BundleError> {
    let retained = history.applied_operations();
    let split = base
        .full_applied_delta
        .len()
        .checked_sub(retained.len())
        .filter(|split| base.full_applied_delta[*split..] == retained)
        .ok_or_else(|| {
            BundleError::Corrupt("frozen specialist delta differs from checkpoint".into())
        })?;
    let check = Core::spawn(baseline.clone()).map_err(|e| BundleError::Corrupt(e.to_string()))?;
    for (ops, expected) in [
        (&base.full_applied_delta[..split], &history.baseline),
        (&retained[..], &history.document),
    ] {
        if !ops.is_empty()
            && !matches!(
                check
                    .request(Command::DoBatch(ops.to_vec()))
                    .map_err(|e| BundleError::Corrupt(e.to_string()))?,
                Event::DocumentChanged { .. }
            )
        {
            return Err(BundleError::Corrupt(
                "specialist proposal replay refused".into(),
            ));
        }
        if check
            .export_history()
            .map_err(|e| BundleError::Corrupt(e.to_string()))?
            .document
            != *expected
        {
            return Err(BundleError::Corrupt(
                "specialist proposal differs from checkpoint".into(),
            ));
        }
    }
    Ok(base.full_applied_delta[..split].to_vec())
}
