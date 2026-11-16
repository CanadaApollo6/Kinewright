//! Prepared local selection. C5 composes the preliminary ordinary Save and lifecycle.
#![allow(clippy::missing_errors_doc)]
use crate::operation_actor::AcceptedCut;
use crate::operation_actor::Core;
use crate::{
    bundle::{BundleError, BundleSaveReport, ProjectBundle, PublicationState},
    bundle_commit::BundleCommit,
    bundle_records::{BranchId, HeadId, ProjectId},
    bundle_recovery::{PreparedCheckpointRestore, RecoverySelection},
    storage_budget::StorageReservation,
    store::StorePin,
};
use kinewright_core::AttachmentState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SwitchTarget {
    WorkingBranch {
        branch: BranchId,
    },
    Checkpoint {
        root: crate::bundle_records::ObjectRef,
        branch: BranchId,
    },
    Snapshot {
        root: crate::bundle_records::ObjectRef,
        branch: BranchId,
        version: String,
    },
}
impl SwitchTarget {
    #[must_use]
    pub fn selection(&self) -> RecoverySelection {
        match self {
            Self::WorkingBranch { branch } => RecoverySelection::WorkingBranch(branch.clone()),
            Self::Checkpoint { root, branch } | Self::Snapshot { root, branch, .. } => {
                RecoverySelection::Checkpoint {
                    root: root.clone(),
                    branch: branch.clone(),
                }
            }
        }
    }
    #[must_use]
    pub fn version(&self) -> Option<String> {
        if let Self::Snapshot { version, .. } = self {
            Some(version.clone())
        } else {
            None
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwitchRequest {
    pub target: SwitchTarget,
    pub source_checkpoint: crate::bundle_records::ObjectRef,
    pub checkpoint_generation: u64,
    pub switch_generation: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwitchBranch {
    pub branch: BranchId,
    pub head: HeadId,
    pub checkpoint: crate::bundle_records::ObjectRef,
    pub history: crate::bundle_records::ObjectRef,
    pub history_role: crate::operation_history::Role,
    pub history_digest: crate::operation_history::SnapshotIdentity,
    pub inputs: crate::bundle_records::ObjectRef,
    pub revision: kinewright_core::TimelineRevision,
}
impl SwitchBranch {
    pub(crate) fn from_checkpoint(
        checkpoint: crate::bundle_records::ObjectRef,
        value: &crate::bundle_records::BranchCheckpoint,
    ) -> Self {
        Self {
            branch: value.branch_id.clone(),
            head: value.head_id.clone(),
            checkpoint,
            history: value.owner_ref.clone(),
            history_role: value.role,
            history_digest: value.identity,
            inputs: value.input_manifest_ref.clone(),
            revision: value.revision,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwitchResult {
    pub old: SwitchBranch,
    pub new: SwitchBranch,
    pub target_version: Option<String>,
    pub switch_generation: u64,
    pub root_generation: u64,
}

/// Exact current committed attachment. Historical receipt results never select it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwitchAttachment {
    pub project: ProjectId,
    pub root_sha256: String,
    pub root_generation: u64,
    pub switch_generation: u64,
    pub current: SwitchBranch,
    // The retained reply shares the same immutable wire buffer and its one capture lease.
    pub history: crate::operation_actor::PortableHistory,
    pub inputs: crate::owned_inputs::OwnedInputManifest,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwitchAttachmentIdentity {
    pub project: ProjectId,
    pub root_sha256: String,
    pub root_generation: u64,
    pub switch_generation: u64,
    pub current: SwitchBranch,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwitchPayload {
    pub history: crate::operation_actor::PortableHistory,
    pub inputs: crate::owned_inputs::OwnedInputManifest,
}
impl SwitchAttachment {
    pub fn identity(&self) -> Result<SwitchAttachmentIdentity, BundleError> {
        self.verify_header()?;
        Ok(SwitchAttachmentIdentity {
            project: self.project.clone(),
            root_sha256: self.root_sha256.clone(),
            root_generation: self.root_generation,
            switch_generation: self.switch_generation,
            current: self.current.clone(),
        })
    }
    fn verify_header(&self) -> Result<(), BundleError> {
        if self.history.branch != self.current.branch
            || self.history.revision != self.current.revision
            || self.history.identity() != self.current.history_digest
            || self.history.role() != self.current.history_role
        {
            return Err(BundleError::HistoryChanged);
        }
        let bytes = serde_json::to_vec(&self.inputs).map_err(crate::bundle::encoding_error)?;
        if self.current.inputs.byte_length != bytes.len() as u64
            || self.current.inputs.sha256 != kinewright_media::sha256_bytes(&bytes)
        {
            return Err(BundleError::HistoryChanged);
        }
        Ok(())
    }
    /// Header equality never grants installation: full replay creates the token.
    pub fn verify(
        &self,
        pool: &crate::operation_history::HistoryPool,
    ) -> Result<crate::operation_history::ValidatedOwnerSnapshot, BundleError> {
        self.verify_header()?;
        self.history
            .snapshot
            .validate_shared(pool, self.current.history_role)
            .map_err(BundleError::from)
    }
}

pub struct PreparedProjectSelection {
    pub(crate) envelope: Option<crate::receipts::CommandEnvelope>,
    pub(crate) restore: PreparedCheckpointRestore,
    pub(crate) target_branch: BranchId,
    source_project: ProjectId,
    pub(crate) source_branch: BranchId,
    source_generation: u64,
    source_cut: AcceptedCut,
    source_head: HeadId,
    existing_head: Option<HeadId>,
    target: RecoverySelection,
    pin: Option<StorePin>,
    pin_charge: Option<StorageReservation>,
    published_root: Option<String>,
    completed: bool,
    result: Option<BundleSaveReport>,
}
#[derive(Debug)]
pub enum ProjectSelectionResult {
    Activated(BundleSaveReport),
    Refused(BundleError),
    Unknown,
    CommittedWithError(BundleError),
}
impl PreparedProjectSelection {
    pub(crate) const fn publication_started(&self) -> bool {
        self.published_root.is_some()
    }
    pub(crate) fn protects_branch(&self, branch: &BranchId) -> bool {
        self.existing_head.is_some()
            && matches!(&self.target, RecoverySelection::WorkingBranch(source) if source == branch)
    }
    pub(crate) fn fork_retained_target(&mut self) -> Result<(), BundleError> {
        if self.publication_started() || self.existing_head.is_none() {
            return Err(BundleError::HistoryChanged);
        }
        let branch = BranchId::fresh()?;
        self.restore.cut = self
            .restore
            .attachment
            .retarget_selection(&self.restore.cut, branch.clone())
            .map_err(crate::bundle::operation_error)?;
        self.target_branch = branch;
        Ok(())
    }
    pub(crate) fn reuses_checkpoint(&self) -> bool {
        matches!(&self.target, RecoverySelection::WorkingBranch(source) if source == &self.target_branch)
    }
    #[must_use]
    pub const fn target_branch(&self) -> &BranchId {
        &self.target_branch
    }
    /// # Panics
    /// Panics if this is an existing branch or publication has already started.
    #[cfg(any(test, feature = "test-util"))]
    pub fn reuse_new_target_id_probe(&mut self, id: BranchId) {
        assert!(self.existing_head.is_none() && self.published_root.is_none());
        self.restore.cut = self
            .restore
            .attachment
            .retarget_selection(&self.restore.cut, id.clone())
            .unwrap();
        self.target_branch = id;
    }
    #[must_use]
    pub fn core(&self) -> Core {
        self.restore.core.clone()
    }
    pub fn input_leases_mut(&mut self) -> &mut [crate::owned_inputs::OwnedInputLease] {
        &mut self.restore.input_leases
    }
    #[must_use]
    pub const fn target(&self) -> &RecoverySelection {
        &self.target
    }
}
impl ProjectBundle {
    /// Verify and pin before the caller starts its ordinary Save. Preparation publishes nothing.
    pub fn prepare_project_selection(
        &self,
        core: &Core,
        target: RecoverySelection,
    ) -> Result<PreparedProjectSelection, BundleError> {
        if self.unsettled || self.pending_command.is_some() {
            return Err(BundleError::Unsettled);
        }
        let reference = match &target {
            RecoverySelection::WorkingBranch(id) => self.checkpoint_reference(id)?,
            RecoverySelection::Checkpoint { root, .. } => root.clone(),
        };
        let closure = self.store.closure(std::slice::from_ref(&reference))?;
        let charge = self.store.reserve(
            0,
            (closure.len() as u64)
                .checked_mul(128)
                .ok_or(BundleError::Capacity("selection pin metadata"))?,
            0,
            false,
        )?;
        for object in &closure {
            self.store.verify(object)?;
        }
        let pin = self.store.pin(std::slice::from_ref(&reference))?;
        let existing_head = match &target {
            RecoverySelection::WorkingBranch(id) => Some(self.checkpoint(id)?.head_id.clone()),
            RecoverySelection::Checkpoint { .. } => None,
        };
        let target_branch = if let RecoverySelection::WorkingBranch(id) = &target {
            id.clone()
        } else {
            BranchId::fresh()?
        };
        let restore = self.prepare_checkpoint_restore(core, &target, target_branch.clone())?;
        let source_cut = self
            .attach_core(core)?
            .capture()
            .map_err(crate::bundle::operation_error)?;
        Ok(PreparedProjectSelection {
            envelope: None,
            restore,
            target_branch,
            source_project: self.project_id.clone(),
            source_branch: self.manual_branch_id.clone(),
            source_generation: self
                .root
                .as_ref()
                .ok_or(BundleError::Unsettled)?
                .manual_switch_generation,
            source_cut,
            source_head: self.manual_head().cloned().ok_or(BundleError::Unsettled)?,
            existing_head,
            target,
            pin: Some(pin),
            pin_charge: Some(charge),
            published_root: None,
            completed: false,
            result: None,
        })
    }
    /// Bind the exact post-Save cut. C5 supplies the ordinary Save's settled floor result.
    pub fn refresh_selection_source(
        &self,
        prepared: &mut PreparedProjectSelection,
        core: &Core,
    ) -> Result<(), BundleError> {
        if self.unsettled
            || self.project_id != prepared.source_project
            || self.manual_branch_id != prepared.source_branch
            || self.root.as_ref().map(|r| r.manual_switch_generation)
                != Some(prepared.source_generation)
            || core.actor_identity() != prepared.source_cut.change().actor
        {
            return Err(BundleError::HistoryBarrierInvariant);
        }
        let cut = self
            .attach_core(core)?
            .capture()
            .map_err(crate::bundle::operation_error)?;
        if !self.selection_source_history_matches(&self.manual_branch_id, cut.identity())? {
            return Err(BundleError::HistoryBarrierInvariant);
        }
        prepared.source_cut = cut;
        prepared.source_head = self.manual_head().cloned().ok_or(BundleError::Unsettled)?;
        Ok(())
    }
    fn selection_source_history_matches(
        &self,
        branch: &BranchId,
        identity: crate::operation_history::SnapshotIdentity,
    ) -> Result<bool, BundleError> {
        Ok(self.checkpoint(branch)?.identity == identity)
    }
    /// Publish exactly one target and activate only after definite root proof.
    pub fn publish_project_selection(
        &mut self,
        prepared: &mut PreparedProjectSelection,
        core: &Core,
    ) -> ProjectSelectionResult {
        if prepared.result.is_none() && prepared.published_root.is_none() && !self.unsettled {
            self.publication_state = PublicationState::NeverAttempted;
            self.publication_tag = None;
        }
        match self.select_prepared(prepared, core) {
            Ok(report) => ProjectSelectionResult::Activated(report),
            Err(error) if self.publication_state == PublicationState::Committed => {
                self.unsettled = true;
                ProjectSelectionResult::CommittedWithError(error)
            }
            Err(_) if self.unsettled || self.publication_state == PublicationState::Unknown => {
                ProjectSelectionResult::Unknown
            }
            Err(error) => ProjectSelectionResult::Refused(error),
        }
    }
    fn select_prepared(
        &mut self,
        prepared: &mut PreparedProjectSelection,
        core: &Core,
    ) -> Result<BundleSaveReport, BundleError> {
        if let Some(report) = &prepared.result {
            return Ok(report.clone());
        }
        if core.actor_identity() != prepared.source_cut.change().actor
            || self.project_id != prepared.source_project
        {
            return Err(BundleError::HistoryBarrierInvariant);
        }
        if let Some(hash) = &prepared.published_root {
            if self.unsettled {
                self.reconcile_destination(None)?;
            }
            if kinewright_media::sha256_bytes(self.admission.raw_bytes()) == *hash {
                return self.finish_selection(prepared, core);
            }
            prepared.published_root = None;
            self.attach_core(core)?
                .resume(&prepared.source_cut)
                .map_err(crate::bundle::operation_error)?;
            return Err(BundleError::HistoryBarrierInvariant);
        }
        if prepared.completed {
            return Err(BundleError::HistoryBarrierInvariant);
        }
        if self.unsettled || self.pending_command.is_some() {
            return Err(BundleError::Unsettled);
        }
        let owner = self.attach_core(core)?;
        let cut = owner.freeze().map_err(crate::bundle::operation_error)?;
        let valid = cut == prepared.source_cut
            && self.selection_source_history_matches(&prepared.source_branch, cut.identity())?
            && self.manual_branch_id == prepared.source_branch
            && self.manual_head() == Some(&prepared.source_head)
            && self.root.as_ref().map(|r| r.manual_switch_generation)
                == Some(prepared.source_generation)
            && prepared.existing_head.as_ref().is_none_or(|head| {
                self.branches
                    .get(match &prepared.target {
                        RecoverySelection::WorkingBranch(source) => source,
                        RecoverySelection::Checkpoint { branch, .. } => branch,
                    })
                    .is_some_and(|c| c.head_id == *head)
            });
        if !valid {
            owner.resume(&cut).map_err(crate::bundle::operation_error)?;
            return Err(BundleError::HistoryBarrierInvariant);
        }
        let target_ref = match &prepared.target {
            RecoverySelection::WorkingBranch(id) => self.checkpoint_reference(id)?,
            RecoverySelection::Checkpoint { root, .. } => root.clone(),
        };
        // Verify the original identity even after its automatic slot has rotated away.
        let verification = (|| {
            for object in self.store.closure(std::slice::from_ref(&target_ref))? {
                self.store.verify(&object)?;
            }
            if self
                .attach_core(&prepared.restore.core)?
                .capture()
                .map_err(crate::bundle::operation_error)?
                != prepared.restore.cut
            {
                return Err(BundleError::HistoryBarrierInvariant);
            }
            Ok(())
        })();
        if let Err(error) = verification {
            owner.resume(&cut).map_err(crate::bundle::operation_error)?;
            return Err(error);
        }
        let result = BundleCommit::publish_selection(self, prepared);
        if let Some(tag) = &self.publication_tag {
            prepared.published_root = Some(tag.root_sha256.clone());
        }
        match result {
            Ok(_) => self.finish_selection(prepared, core),
            Err(error) => {
                if !self.unsettled && self.publication_state != PublicationState::Unknown {
                    owner.resume(&cut).map_err(crate::bundle::operation_error)?;
                }
                Err(error)
            }
        }
    }
    fn finish_selection(
        &self,
        prepared: &mut PreparedProjectSelection,
        core: &Core,
    ) -> Result<BundleSaveReport, BundleError> {
        if self.manual_branch_id != prepared.target_branch
            || self.root.as_ref().map(|r| r.manual_switch_generation)
                != prepared.source_generation.checked_add(1)
        {
            return Err(BundleError::HistoryBarrierInvariant);
        }
        #[cfg(any(test, feature = "test-util"))]
        crate::io_observer::check_failure(
            "selection_activation_prepare",
            &self.store.resolved_root()?,
        )?;
        let report = BundleSaveReport {
            position: self.checkpoint(&self.manual_branch_id)?.position,
            history_sha256: self
                .checkpoint(&self.manual_branch_id)?
                .identity
                .to_string(),
            collection: self.store.collection_status()?,
            path: self.store.resolved_root()?,
            project_id: self.project_id.clone(),
            generation: self.generation,
            head_id: self.manual_head().cloned().ok_or(BundleError::Unsettled)?,
            root_sha256: kinewright_media::sha256_bytes(self.admission.raw_bytes()),
            byte_length: self.admission.raw_bytes().len() as u64,
        };
        if !prepared.completed {
            match core
                .attachment_state()
                .map_err(crate::bundle::operation_error)?
            {
                AttachmentState::Frozen => self
                    .attach_core(core)?
                    .retire(&prepared.source_cut)
                    .map_err(crate::bundle::operation_error)?,
                // A committed switch may be retried after retiring A but before
                // B could resume. A's complete state is already in this root.
                AttachmentState::Sealed
                    if core.actor_identity() == prepared.source_cut.change().actor => {}
                _ => return Err(BundleError::HistoryBarrierInvariant),
            }
            self.attach_core(&prepared.restore.core)?
                .resume(&prepared.restore.cut)
                .map_err(crate::bundle::operation_error)?;
            prepared.completed = true;
            prepared.pin.take();
            prepared.pin_charge.take();
        }
        prepared.result = Some(report.clone());
        Ok(report)
    }
}
