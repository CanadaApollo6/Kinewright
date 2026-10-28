//! Verified project-owned recovery payloads and the restricted L1 linkage projection.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::{BundleError, BundleWriteAdmission, ProjectBundle, SupportInventory},
    bundle_records::{BranchCheckpoint, BranchId, LineageRecord, ObjectRef, RecordKind},
    owned_inputs::OwnedInputManifest,
    rescue_record::{RescueCopyV1, RescueRecoveryPoint},
    store::StoreManifest,
};
use kinewright_core::{Core, HistorySnapshot};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoverySelection {
    WorkingBranch(BranchId),
    Checkpoint { root: ObjectRef, branch: BranchId },
}
pub struct PreparedCheckpointRestore {
    pub(crate) attachment: crate::ProjectAttachment,
    pub(crate) history_digest: String,
    pub(crate) core: Core,
    pub(crate) checkpoint: BranchCheckpoint,
    pub(crate) history: std::sync::Arc<crate::storage_budget::StorageReservation<HistorySnapshot>>,
    pub(crate) lineage: crate::storage_budget::StorageReservation<LineageRecord>,
    pub(crate) cut: kinewright_core::AcceptedCut,
    pub(crate) input_leases: Vec<crate::owned_inputs::OwnedInputLease>,
}
impl PreparedCheckpointRestore {
    #[must_use]
    pub fn core(&self) -> Core {
        self.core.clone()
    }
    pub fn input_leases_mut(&mut self) -> &mut [crate::owned_inputs::OwnedInputLease] {
        &mut self.input_leases
    }
    #[must_use]
    pub const fn checkpoint(&self) -> &BranchCheckpoint {
        &self.checkpoint
    }
}
impl ProjectBundle {
    #[allow(
        clippy::too_many_lines,
        reason = "one verified checkpoint and resource preparation before any publication"
    )]
    pub fn prepare_checkpoint_restore(
        &self,
        selection: &RecoverySelection,
    ) -> Result<PreparedCheckpointRestore, BundleError> {
        let reference = match selection {
            RecoverySelection::WorkingBranch(branch) => self.checkpoint_reference(branch)?,
            RecoverySelection::Checkpoint { root, branch } => {
                if root.record_kind == RecordKind::BranchCheckpoint {
                    let versions = self.version_index()?;
                    let descriptor = versions
                        .snapshots
                        .values()
                        .find(|d| d.checkpoint_ref == *root)
                        .ok_or_else(|| {
                            BundleError::Identity("unindexed Studio checkpoint".into())
                        })?;
                    self.validate_snapshot(descriptor)?;
                    let checkpoint = self.store.read_record::<BranchCheckpoint>(root)?;
                    if checkpoint.branch_id != *branch {
                        return Err(BundleError::Identity("Studio checkpoint branch".into()));
                    }
                    root.clone()
                } else {
                    if root.record_kind != RecordKind::RootCheckpoint {
                        return Err(BundleError::Corrupt("recovery root kind".into()));
                    }
                    let bytes = self.store.read_checkpoint_bytes(root)?;
                    let admission = BundleWriteAdmission::from_bytes_with_limits(
                        None,
                        bytes,
                        self.store.usage_arc(),
                        self.store.limits(),
                    )?;
                    admission.require_supported()?;
                    let root = admission
                        .data
                        .file
                        .as_ref()
                        .and_then(|f| f.studio.as_ref())
                        .ok_or(BundleError::Unsettled)?;
                    if root.project_id != self.project_id
                        && !self.origin_project_ids.contains(&root.project_id)
                    {
                        return Err(BundleError::Identity("recovery origin".into()));
                    }
                    let manifest = self
                        .store
                        .read_record::<StoreManifest>(&root.store_manifest_ref)?;
                    super::validate_manifest_apis(&manifest, root.manual_switch_generation)?;
                    self.store
                        .read_record::<BTreeMap<BranchId, ObjectRef>>(&root.branch_index_ref)?
                        .get(branch)
                        .cloned()
                        .ok_or_else(|| BundleError::Corrupt("recovery branch".into()))?
                }
            }
        };
        let closure = self.store.closure(std::slice::from_ref(&reference))?;
        for object in &closure {
            self.store.verify(object)?;
        }
        let checkpoint = self.store.read_record::<BranchCheckpoint>(&reference)?;
        let history = self
            .store
            .read_record::<HistorySnapshot>(&checkpoint.history_ref)?;
        let document = self
            .store
            .read_record::<kinewright_core::Document>(&checkpoint.document_ref)?;
        let baseline = self
            .store
            .read_record::<kinewright_core::Document>(&checkpoint.baseline_ref)?;
        if *document != *history.document
            || *baseline != *history.baseline
            || checkpoint.document_hash != checkpoint.document_ref.sha256
            || checkpoint.applied_operations_digest
                != kinewright_media::sha256_bytes(
                    &serde_json::to_vec(&history.applied_operations())
                        .map_err(|e| BundleError::Corrupt(e.to_string()))?,
                )
            || checkpoint.batch_ids.len() != history.batches.len()
            || checkpoint.history_input_manifest_refs.len() != history.batches.len() + 1
        {
            return Err(BundleError::Corrupt("recovery payload agreement".into()));
        }
        let inputs = self
            .store
            .read_record::<OwnedInputManifest>(&checkpoint.input_manifest_ref)?;
        let mut retained = (*inputs).clone();
        let mut input_leases = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for manifest_ref in std::iter::once(&checkpoint.input_manifest_ref)
            .chain(&checkpoint.history_input_manifest_refs)
        {
            if !seen.insert(manifest_ref.clone()) {
                continue;
            }
            let manifest = self.store.read_input_manifest(manifest_ref)?;
            if manifest.session_id.is_some()
                || !manifest.project_id.as_ref().is_some_and(|id| {
                    id == &self.project_id || self.origin_project_ids.contains(id)
                })
            {
                return Err(BundleError::Identity("recovery input owner".into()));
            }
            let owner = (manifest.project_id.clone(), None);
            for entry in &manifest.entries {
                input_leases.push(self.store.lease_input_for_owner(
                    &crate::owned_inputs::OwnedInputRef {
                        manifest_ref: manifest_ref.clone(),
                        entry: entry.clone(),
                    },
                    &owner,
                )?);
            }
        }
        for reference in &checkpoint.history_input_manifest_refs {
            retained.entries.extend(
                self.store
                    .read_record::<OwnedInputManifest>(reference)?
                    .entries
                    .clone(),
            );
        }
        crate::bundle_commit::validate_document_inputs(&history.document, &inputs)?;
        crate::bundle_commit::validate_inputs(&history, &retained)?;
        let core = Core::spawn((*history.baseline).clone())
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        self.attach_core(&core)?
            .restore_history((*history).clone())
            .map_err(crate::bundle::history_error)?;
        let lineage = self
            .store
            .read_record::<LineageRecord>(&checkpoint.lineage_ref)?;
        let cut = self
            .attach_core(&core)?
            .freeze()
            .map_err(crate::bundle::history_error)?;
        let attachment = self.attach_core(&core)?;
        let history_digest = kinewright_media::sha256_bytes(
            &serde_json::to_vec(&*history).map_err(|e| BundleError::Corrupt(e.to_string()))?,
        );
        Ok(PreparedCheckpointRestore {
            attachment,
            history_digest,
            core,
            cut,
            input_leases,
            checkpoint: (*checkpoint).clone(),
            history: std::sync::Arc::new(history),
            lineage,
        })
    }

    /// Remove only inherited rescue linkage. Payload refs, independent roots and receipt refs stay exact.
    pub fn project_recovery_root(
        &mut self,
        source: &ObjectRef,
    ) -> Result<(ObjectRef, RescueRecoveryPoint), BundleError> {
        let bytes = self.store.read_checkpoint_bytes(source)?;
        let admission = BundleWriteAdmission::from_bytes_with_limits(
            None,
            bytes,
            self.store.usage_arc(),
            self.store.limits(),
        )?;
        admission.require_supported()?;
        let mut root = admission
            .data
            .file
            .as_ref()
            .and_then(|f| f.studio.clone())
            .ok_or(BundleError::Unsettled)?;
        let manifest = self
            .store
            .read_record::<StoreManifest>(&root.store_manifest_ref)?;
        let mut pins = manifest.durable_pins.clone();
        let mut seals = Vec::new();
        let mut changed = false;
        for pin in &mut pins {
            if manifest.evidence_roles.get(&pin.sha256).map(String::as_str)
                != Some(crate::rescue_record::RESCUE_ROLE)
            {
                continue;
            }
            let original = self.store.read_record::<RescueCopyV1>(pin)?;
            let mut record = (*original).clone();
            if let Some(inherited) = record.acknowledged_root.take() {
                record.acknowledged_absence =
                    Some("inherited recovery linkage is provenance".into());
                record.recovery_origin = Some(RescueRecoveryPoint {
                    source_root: inherited,
                    rebound: true,
                });
                let sealed = self.store.admit_record(&record)?;
                *pin = sealed.reference.clone();
                seals.push(sealed);
                changed = true;
            }
        }
        let origin = RescueRecoveryPoint {
            source_root: source.clone(),
            rebound: changed,
        };
        if !changed {
            return Ok((source.clone(), origin));
        }
        let roots = root
            .references()
            .into_iter()
            .filter(|reference| reference != &root.store_manifest_ref)
            .collect::<Vec<_>>();
        let inventory = SupportInventory {
            required_fields: manifest.required_fields.clone(),
            required_kinds: manifest.required_kinds.clone(),
            required_apis: manifest.required_apis.clone(),
        };
        let mut projected = self.store.selected_manifest(
            &roots,
            &pins,
            crate::store::RecoveryInventory::default(),
            &inventory,
            root.generation,
        )?;
        projected.project_id.clone_from(&manifest.project_id);
        projected.session_id.clone_from(&manifest.session_id);
        let sealed = self.store.admit_record(&projected)?;
        root.store_manifest_ref = sealed.reference.clone();
        seals.push(sealed);
        let document = admission
            .inspection_document()
            .ok_or(BundleError::Unsettled)?;
        let (bytes, _charge) = crate::bundle_commit::serialize_root(&self.store, document, &root)?;
        let sealed = self.admit_root_checkpoint(&bytes, &root.references())?;
        let reference = sealed.reference.clone();
        // Carry the new root's complete closure until the caller attaches its rescue record.
        self.pending_evidence.push(sealed.into_pin());
        drop(seals);
        verify_projection_payloads(self, source, &reference)?;
        Ok((reference, origin))
    }
}
impl crate::store::ProjectStore {
    pub(crate) fn read_checkpoint_bytes(
        &self,
        reference: &ObjectRef,
    ) -> Result<Vec<u8>, BundleError> {
        if reference.record_kind != RecordKind::RootCheckpoint
            || reference.byte_length > self.limits().control_bytes
        {
            return Err(BundleError::Corrupt("checkpoint kind/length".into()));
        }
        let _charge = self.reserve(0, reference.byte_length, 0, false)?;
        let bytes = super::bounded_bytes(self.open_verified(reference)?, reference.byte_length)?;
        let admission = BundleWriteAdmission::from_bytes_with_limits(
            None,
            bytes.clone(),
            self.usage_arc(),
            self.limits(),
        )?;
        admission.require_supported()?;
        let mut expected = admission
            .data
            .file
            .as_ref()
            .and_then(|f| f.studio.as_ref())
            .ok_or(BundleError::Unsettled)?
            .references();
        expected.sort();
        expected.dedup();
        let mut actual = self
            .manifest()?
            .dependencies
            .get(&reference.sha256)
            .cloned()
            .ok_or_else(|| BundleError::Corrupt("checkpoint declaration".into()))?;
        actual.sort();
        actual.dedup();
        if expected != actual {
            return Err(BundleError::Corrupt("root checkpoint content edges".into()));
        }
        Ok(bytes)
    }
}

/// Exact L1 oracle. A graph-valid replacement of any non-linkage payload still refuses.
pub fn verify_projection_payloads(
    bundle: &ProjectBundle,
    original: &ObjectRef,
    projected: &ObjectRef,
) -> Result<(), BundleError> {
    let before = BundleWriteAdmission::from_bytes_with_limits(
        None,
        bundle.store.read_checkpoint_bytes(original)?,
        bundle.store.usage_arc(),
        bundle.store.limits(),
    )?;
    let after = BundleWriteAdmission::from_bytes_with_limits(
        None,
        bundle.store.read_checkpoint_bytes(projected)?,
        bundle.store.usage_arc(),
        bundle.store.limits(),
    )?;
    before.require_supported()?;
    after.require_supported()?;
    let left = before
        .data
        .file
        .as_ref()
        .and_then(|f| f.studio.as_ref())
        .ok_or(BundleError::Unsettled)?;
    let right = after
        .data
        .file
        .as_ref()
        .and_then(|f| f.studio.as_ref())
        .ok_or(BundleError::Unsettled)?;
    if before.inspection_document() != after.inspection_document()
        || left.branch_index_ref != right.branch_index_ref
        || left.input_manifest_ref != right.input_manifest_ref
        || left.version_index_ref != right.version_index_ref
        || left.feedback_index_ref != right.feedback_index_ref
        || left.run_index_ref != right.run_index_ref
        || left.claim_index_ref != right.claim_index_ref
        || left.receipt_index_ref != right.receipt_index_ref
        || left.issuer_state_ref != right.issuer_state_ref
    {
        return Err(BundleError::Corrupt(
            "L1 non-linkage payload changed".into(),
        ));
    }
    let lm = bundle
        .store
        .read_record::<StoreManifest>(&left.store_manifest_ref)?;
    let rm = bundle
        .store
        .read_record::<StoreManifest>(&right.store_manifest_ref)?;
    if right
        .references()
        .iter()
        .filter(|reference| *reference != &right.store_manifest_ref)
        .any(|reference| !rm.objects.contains(reference))
    {
        return Err(BundleError::Corrupt(
            "L1 projected manifest omits root reference".into(),
        ));
    }
    let independent = |manifest: &StoreManifest| {
        manifest
            .durable_pins
            .iter()
            .filter(|r| !manifest.evidence_roles.contains_key(&r.sha256))
            .cloned()
            .collect::<Vec<_>>()
    };
    if independent(&lm) != independent(&rm) {
        return Err(BundleError::Corrupt(
            "L1 independent evidence changed".into(),
        ));
    }
    Ok(())
}
