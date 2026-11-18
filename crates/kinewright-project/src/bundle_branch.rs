//! Frozen branch/checkpoint operations. Live writer fencing belongs to S2.
#![allow(clippy::missing_errors_doc)]
use crate::operation_actor::Core;
use crate::{
    bundle::{BundleError, BundleSaveReport, ProjectBundle},
    bundle_commit::BundleCommit,
    bundle_records::{BranchCheckpoint, BranchId, ObjectRef, RecordKind, SpecialistBaseRecord},
};

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
    ) -> Result<crate::operation_history::OwnerRead, BundleError> {
        self.checkpoint(id)?
            .open(&self.store, &self.history_pool)?
            .into_reader()
            .map_err(BundleError::from)
    }
    pub fn branch_lineage(
        &self,
        id: &BranchId,
    ) -> Result<crate::operation_history::OwnerBoundary, BundleError> {
        let reader = self.branch_history(id)?;
        let depth = reader.history().undo().len();
        reader.into_boundary(depth).map_err(BundleError::from)
    }
    pub fn fork_branch(
        &mut self,
        source: &BranchId,
        role: crate::operation_history::Role,
    ) -> Result<BranchId, BundleError> {
        self.fork_branch_as(source, BranchId::fresh()?, role)
    }
    pub(crate) fn fork_branch_as(
        &mut self,
        source: &BranchId,
        id: BranchId,
        role: crate::operation_history::Role,
    ) -> Result<BranchId, BundleError> {
        self.store.admit_run_write()?;
        if self.unsettled || self.pending_command.is_some() {
            return Err(BundleError::Unsettled);
        }
        if self.branches.contains_key(&id) {
            return Err(BundleError::HistoryChanged);
        }
        let inputs = self.checkpoint(source)?.input_manifest_ref.clone();
        let reader = self.branch_history(source)?;
        let mut owner = reader.fork(role)?;
        drop(reader);
        // The child owns an independent full lease; the cold source is gone
        // before live admission reserves the next capture window.
        owner.promote_live()?;
        let candidate = Core::spawn_owner(
            owner,
            id.clone(),
            kinewright_core::TimelineRevision::default(),
        )
        .map_err(crate::bundle::operation_error)?;
        // This private fresh child is quiescent until its first root exists.
        // Seal its frozen owner directly, leaving the third ordinary lease for
        // validating the publication rather than another in-memory capture.
        let attachment = self.attach_core(&candidate)?;
        let cut = attachment
            .freeze()
            .map_err(crate::bundle::operation_error)?;
        let sources =
            std::collections::BTreeSet::from([self.checkpoint(source)?.owner_ref.clone()]);
        let publication =
            BundleCommit::publish_frozen_branch(self, &attachment, &cut, &sources, &inputs, &id);
        // Publication failure cannot have accepted child work: the fork has not
        // been returned. Release synchronously before either answer is visible.
        attachment
            .retire(&cut)
            .map_err(crate::bundle::operation_error)?;
        publication?;
        Ok(id)
    }
    pub fn save_branch(
        &mut self,
        id: &BranchId,
        core: &Core,
    ) -> Result<BundleSaveReport, BundleError> {
        self.store.admit_run_write()?;
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
            .capture_state()
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        self.publish_core(core, before, &inputs, id)
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
        let _inputs = self.checkpoint(&id)?.input_manifest_ref.clone();
        BundleCommit::publish_maintenance(self)?;
        Ok(reference)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod fork_pool_tests {
    use super::*;
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    #[test]
    fn ordinary_fork_publishes_with_three_leases_and_releases_its_temporary_actor() {
        let temp = TempDirectory::new_in(&workspace_target_dir(), "hx-ordinary-fork-pool");
        let core = Core::for_test(kinewright_core::Document::default()).unwrap();
        let mut bundle = ProjectBundle::activate(
            &temp.path("project.kinewright"),
            crate::bundle::BundleWriteAdmission::new_document(),
            core.history_pool(),
        )
        .unwrap();
        let inputs = bundle.empty_inputs().unwrap().reference.clone();
        bundle.save(&core, &inputs).unwrap();
        let source = bundle.manual_branch_id().clone();
        let child = bundle
            .fork_branch(&source, crate::operation_history::Role::Ordinary)
            .expect("ORDINARY_FORK_FITS_THREE_LEASES");
        let counts = core
            .history_pool()
            .lease_counts(crate::operation_history::Role::Ordinary)
            .unwrap();
        assert!(
            counts.1 <= 3,
            "ORDINARY_FORK_PEAK_AT_MOST_THREE: {counts:?}"
        );
        assert_eq!(
            counts.0, 1,
            "ORDINARY_FORK_PUBLICATION_ACK_RELEASES_TEMPORARY_ACTOR"
        );
        let saved = bundle.branch_history(&child).unwrap();
        assert!(saved.history().undo().is_empty() && saved.history().redo().is_empty());
        assert_eq!(
            saved.history().document().as_ref(),
            &kinewright_core::Document::default(),
            "ORDINARY_FORK_PUBLISHES_ITS_EXACT_BASELINE"
        );
    }
}
