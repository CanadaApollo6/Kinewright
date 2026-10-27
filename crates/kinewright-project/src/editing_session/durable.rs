//! Registration belongs to the project owner; dropping a caller's guard does not close its actor.
use super::{BundleError, Core, Ja1EditingSession, ObjectRef, StorageReservation};
use crate::bundle_records::{BranchId, SpecialistBaseRecord};
use kinewright_core::{Command, Document, Event, HistorySnapshot};
use std::sync::Arc;

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
        owner
            .resume(&current)
            .map_err(crate::bundle::history_error)?;
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
