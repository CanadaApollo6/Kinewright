//! Frozen branch/checkpoint operations. Live writer fencing belongs to S2.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::{BundleError, BundleSaveReport, ProjectBundle},
    bundle_commit::BundleCommit,
    bundle_records::{
        BranchCheckpoint, BranchId, LineageRecord, ObjectRef, RecordKind, SpecialistBaseRecord,
    },
    receipts::HistoryRequest,
    storage_budget::StorageReservation,
};
use kinewright_core::{Command, Core, Event, HistorySnapshot, HistoryTransition};

pub(crate) fn replay_prefix(
    history: &HistorySnapshot,
    count: usize,
) -> Result<HistorySnapshot, BundleError> {
    let prefix = Core::spawn(std::sync::Arc::clone(&history.baseline))
        .map_err(|e| BundleError::Corrupt(e.to_string()))?;
    for batch in history
        .batches
        .get(..count)
        .ok_or_else(|| BundleError::Corrupt("history prefix cut".into()))?
    {
        if !matches!(
            prefix
                .request(Command::from(batch.command.clone()))
                .map_err(|e| BundleError::Corrupt(e.to_string()))?,
            Event::DocumentChanged { .. }
        ) {
            return Err(BundleError::Corrupt("history prefix replay".into()));
        }
    }
    prefix
        .export_history()
        .map_err(|e| BundleError::Corrupt(e.to_string()))
}

pub(crate) fn transition(
    request: &HistoryRequest,
    checkpoint: &BranchCheckpoint,
    before: HistorySnapshot,
) -> Result<HistoryTransition, BundleError> {
    let result = match request {
        HistoryRequest::Floor {
            discard_batches,
            preceding_checkpoint_generation,
            preceding_floor_generation,
            command_cut,
        } => {
            if checkpoint.checkpoint_generation != *preceding_checkpoint_generation
                || checkpoint.floor_generation != *preceding_floor_generation
                || checkpoint.command_cut.as_ref() != Some(command_cut)
            {
                return Err(BundleError::Unsupported(
                    "unmatched floor predecessor or cut".into(),
                ));
            }
            checkpoint
                .floor_generation
                .checked_add(1)
                .ok_or(BundleError::Capacity("floor generation"))?;
            HistoryTransition::floor(before, *discard_batches)
        }
        HistoryRequest::RecoveryClose {
            preceding_checkpoint_generation,
            reconciled_cut,
            suffix_digest,
        } => {
            if checkpoint.checkpoint_generation != *preceding_checkpoint_generation
                || checkpoint.command_cut.as_ref() != reconciled_cut.as_ref()
                || checkpoint.history_ref.sha256 != *suffix_digest
            {
                return Err(BundleError::Unsupported(
                    "unmatched recovery-close suffix or cut".into(),
                ));
            }
            HistoryTransition::recovery_close(before.clone(), &before)
        }
    };
    result.map_err(|e| BundleError::Corrupt(e.to_string()))
}

impl ProjectBundle {
    pub fn checkpoint(&self, id: &BranchId) -> Result<&BranchCheckpoint, BundleError> {
        self.branches
            .get(id)
            .ok_or_else(|| BundleError::Unsupported("unknown branch".into()))
    }
    pub fn checkpoint_reference(&self, id: &BranchId) -> Result<ObjectRef, BundleError> {
        let _reading = crate::run_admission::reading(&self.store);
        let root = self
            .root
            .as_ref()
            .ok_or_else(|| BundleError::Corrupt("no root".into()))?;
        self.store
            .read_record::<std::collections::BTreeMap<BranchId, ObjectRef>>(&root.branch_index_ref)?
            .get(id)
            .cloned()
            .ok_or_else(|| BundleError::Corrupt("branch index omitted checkpoint".into()))
    }
    pub fn branch_history(
        &self,
        id: &BranchId,
    ) -> Result<StorageReservation<HistorySnapshot>, BundleError> {
        let reference = &self.checkpoint(id)?.history_ref;
        if id == &self.manual_branch_id {
            self.store.resolve_manual_history(reference)
        } else {
            self.store.resolve_history(reference)
        }
    }
    pub fn branch_lineage(
        &self,
        id: &BranchId,
    ) -> Result<StorageReservation<LineageRecord>, BundleError> {
        self.store.read_record(&self.checkpoint(id)?.lineage_ref)
    }
    pub fn fork_branch(
        &mut self,
        source: &BranchId,
        role: crate::history_record::HistoryRole,
    ) -> Result<BranchId, BundleError> {
        self.fork_branch_as(source, BranchId::fresh()?, role)
    }
    pub(crate) fn fork_branch_as(
        &mut self,
        source: &BranchId,
        id: BranchId,
        role: crate::history_record::HistoryRole,
    ) -> Result<BranchId, BundleError> {
        if self.unsettled || self.pending_command.is_some() {
            return Err(BundleError::Unsettled);
        }
        let inputs = self.checkpoint(source)?.input_manifest_ref.clone();
        let source_history = self.branch_history(source)?;
        let original = self.branch_lineage(source)?;
        if self.branches.contains_key(&id) {
            return Err(BundleError::HistoryChanged);
        }
        let seed = LineageRecord {
            branch_origin: id.clone(),
            baseline: original.current.clone(),
            current: original.current.clone(),
            undo: vec![],
            redo: vec![],
            split_ancestry: original.split_ancestry.clone(),
        };
        let candidate = Core::spawn(std::sync::Arc::clone(&source_history.document))
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        let frozen = candidate
            .export_history()
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        BundleCommit::publish_branch(
            self,
            &frozen,
            &inputs,
            &id,
            Some(crate::bundle_commit::BranchSeed {
                lineage: &seed,
                role,
            }),
        )?;
        Ok(id)
    }
    pub fn save_branch(
        &mut self,
        id: &BranchId,
        core: &Core,
    ) -> Result<BundleSaveReport, BundleError> {
        crate::bundle::reconcile_core_save(core)?;
        crate::bundle_copy::settle_copy_source(&mut self.pending_copy_source)?;
        if self.unsettled {
            self.reconcile()?;
        }
        if self.unsettled || self.pending_command.is_some() {
            return Err(BundleError::Unsettled);
        }
        let inputs = self.checkpoint(id)?.input_manifest_ref.clone();
        let before = core
            .export_history()
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        self.publish_core(core, &before, &inputs, id)
    }
    /// Persist the full frozen delta independently of subsequent manual floors.
    pub(crate) fn stage_specialist_base(
        &mut self,
        base: &SpecialistBaseRecord,
    ) -> Result<ObjectRef, BundleError> {
        self.store_mut()?;
        let bytes =
            serde_json::to_vec(base).map_err(|error| BundleError::Corrupt(error.to_string()))?;
        let expected = ObjectRef {
            sha256: kinewright_media::sha256_bytes(&bytes),
            byte_length: bytes.len() as u64,
            record_kind: RecordKind::SpecialistBase,
        };
        // A matching retry consumes no new version slot, including at the raw
        // limit after a prior attempt already staged this exact base.
        if !self.evidence.contains(&expected)
            && self
                .evidence
                .iter()
                .filter(|reference| reference.record_kind == RecordKind::SpecialistBase)
                .count() as u64
                >= self.store.specialist_version_limit()
        {
            return Err(self
                .store
                .run_error(BundleError::Capacity("retained specialist versions")));
        }
        let sealed = self.store.admit_record(base)?;
        let reference = sealed.reference.clone();
        if !self.evidence.contains(&reference) {
            self.evidence.push(reference.clone());
            self.pending_evidence.push(sealed.into_pin());
        }
        Ok(reference)
    }
    pub fn retain_specialist_base(
        &mut self,
        base: &SpecialistBaseRecord,
    ) -> Result<ObjectRef, BundleError> {
        let reference = self.stage_specialist_base(base)?;
        let id = self.manual_branch_id.clone();
        let inputs = self.checkpoint(&id)?.input_manifest_ref.clone();
        let history = self.branch_history(&id)?;
        BundleCommit::publish(self, &history, &inputs)?;
        Ok(reference)
    }
}
