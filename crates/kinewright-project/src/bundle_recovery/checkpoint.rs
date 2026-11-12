//! Independently verified checkpoint rescue. Failed history grants no writer authority.
#![allow(clippy::missing_errors_doc)]
use super::{RecoveryAssessment, compare_edges, read_bytes, read_record, walk};
use crate::{
    bundle::{BundleError, BundleSaveReport, BundleWriteAdmission, ProjectBundle},
    bundle_commit::{BranchSeed, BundleCommit},
    bundle_records::{BranchCheckpoint, BranchId, LineageRecord, ObjectRef, RecordKind},
    history_record::HistoryRole,
    namespace::{DestinationInspection, ProjectNamespace},
    owned_inputs::OwnedInputManifest,
    owned_media::InputChunkTable,
    storage_budget::{StorageLimits, StorageReservation},
    store::{BundleRecord, ProjectStore, StoreManifest},
};
use kinewright_core::{Document, HistoryCursor, HistorySnapshot};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoHistoryUnavailable {
    pub history_ref: ObjectRef,
    pub reason: String,
}

/// Keeps the damaged source locked and untouched. Only a fresh rescue destination
/// can receive its independently verified document, occurrences and current inputs.
pub struct VerifiedCheckpointRecovery {
    namespace: ProjectNamespace,
    admission: BundleWriteAdmission,
    document: StorageReservation<Arc<Document>>,
    lineage: StorageReservation<LineageRecord>,
    inputs: StorageReservation<OwnedInputManifest>,
    manifest: StorageReservation<StoreManifest>,
    disclosure: UndoHistoryUnavailable,
}

fn load<T: BundleRecord>(
    namespace: &ProjectNamespace,
    reference: &ObjectRef,
    admission: &BundleWriteAdmission,
) -> Result<StorageReservation<T>, BundleError> {
    read_record(
        namespace,
        reference,
        &admission.data.usage,
        &mut BTreeSet::new(),
        &StorageLimits::default(),
    )
}

impl VerifiedCheckpointRecovery {
    #[must_use]
    pub fn document(&self) -> &Document {
        &self.document
    }
    #[must_use]
    pub const fn disclosure(&self) -> &UndoHistoryUnavailable {
        &self.disclosure
    }

    /// Ordinary/native open stays strict. This capability offers a checkpoint cut,
    /// never a resolved value under the failed history reference.
    pub fn open(path: &Path) -> Result<Option<Self>, BundleError> {
        Self::open_with_limits(path, &StorageLimits::default())
    }
    #[allow(
        clippy::too_many_lines,
        reason = "one independently verified source cut before granting a rescue capability"
    )]
    fn open_with_limits(path: &Path, limits: &StorageLimits) -> Result<Option<Self>, BundleError> {
        let RecoveryAssessment::Quarantined {
            admission,
            branches,
            verified_document_bytes,
            ..
        } = super::assess_with_limits(path, limits)?
        else {
            return Ok(None);
        };
        let root = admission
            .data
            .file
            .as_ref()
            .and_then(|f| f.studio.as_ref())
            .ok_or(BundleError::Unsettled)?;
        let Some(failed) = branches
            .iter()
            .find(|b| b.branch_id == root.manual_branch_id)
        else {
            return Ok(None);
        };
        let namespace = ProjectNamespace::acquire(path)?;
        namespace.verify_admitted_root(admission.raw_bytes())?;
        let index =
            load::<BTreeMap<BranchId, ObjectRef>>(&namespace, &root.branch_index_ref, &admission)?;
        let checkpoint = load::<BranchCheckpoint>(
            &namespace,
            index
                .get(&root.manual_branch_id)
                .ok_or_else(|| BundleError::Corrupt("recovery manual checkpoint missing".into()))?,
            &admission,
        )?;
        if checkpoint.branch_id != root.manual_branch_id
            || checkpoint.history_ref != failed.history_ref
            || !verified_document_bytes.contains(&checkpoint.document_ref)
        {
            return Err(BundleError::Corrupt("recovery checkpoint identity".into()));
        }
        // Capacity and authority failures do not authorize discarding Undo.
        let reason = match crate::history_record::resolve_namespace_history(
            &namespace,
            &checkpoint.history_ref,
            &admission.data.usage,
            &mut BTreeSet::new(),
            limits,
            false,
        ) {
            Err(BundleError::Corrupt(reason)) => reason,
            Err(error) => return Err(error),
            Ok(_) => return Err(BundleError::HistoryChanged),
        };
        let document = load::<Document>(&namespace, &checkpoint.document_ref, &admission)?;
        if checkpoint.document_hash != checkpoint.document_ref.sha256
            || admission.inspection_document() != Some(&*document)
        {
            return Err(BundleError::Corrupt(
                "recovery checkpoint document mismatch".into(),
            ));
        }
        if checkpoint.lineage_ref.record_kind != RecordKind::Lineage {
            return Err(BundleError::Corrupt("recovery lineage kind".into()));
        }
        let lineage = {
            let limits = StorageLimits::default();
            let (bytes, _encoded) = read_bytes(
                &namespace,
                &checkpoint.lineage_ref,
                &admission.data.usage,
                &limits,
            )?;
            let charge = crate::store::preflight_record(&bytes, &limits)?;
            let _wire = StorageReservation::reserve(
                &admission.data.usage,
                &limits,
                0,
                charge,
                0,
                false,
                u64::MAX,
            )?;
            crate::lineage_journal::restore_current(&bytes, |live| {
                StorageReservation::reserve(
                    &admission.data.usage,
                    &limits,
                    0,
                    live,
                    0,
                    false,
                    u64::MAX,
                )
            })?
        };
        if lineage.current.keys().copied().collect::<BTreeSet<_>>()
            != document
                .tracks
                .iter()
                .flat_map(|t| t.clips.iter().map(|c| c.id))
                .collect()
        {
            return Err(BundleError::Corrupt(
                "recovery current occurrences mismatch".into(),
            ));
        }
        let inputs =
            load::<OwnedInputManifest>(&namespace, &checkpoint.input_manifest_ref, &admission)?;
        if inputs.session_id.is_some()
            || !inputs
                .project_id
                .as_ref()
                .is_some_and(|id| id == &root.project_id || root.origin_project_ids.contains(id))
        {
            return Err(BundleError::Identity("recovery current input owner".into()));
        }
        crate::bundle_commit::validate_document_inputs(&document, &inputs)?;
        let manifest = load::<StoreManifest>(&namespace, &root.store_manifest_ref, &admission)?;
        compare_edges(&checkpoint.input_manifest_ref, &*inputs, &manifest)?;
        // Only the checkpoint document is being recovered. Redo-only resources
        // remain with the original quarantined history, not the empty new history.
        let document = document.map(Arc::new);
        let history = HistorySnapshot {
            floor_offset: 0,
            baseline: Arc::clone(&document),
            document: Arc::clone(&document),
            batches: vec![],
            cursor: HistoryCursor {
                undo: vec![],
                redo: vec![],
            },
        };
        let inputs = inputs.map(|mut inputs| {
            crate::bundle_commit::retain_history_inputs(&history, &mut inputs);
            inputs
        });
        // Bound the SHA lookup and the walk's owned visited/found keys.
        let _inspection = StorageReservation::reserve(
            &admission.data.usage,
            &StorageLimits::default(),
            0,
            (manifest.objects.len() as u64)
                .checked_mul(512)
                .ok_or(BundleError::Capacity("recovery graph inspection"))?,
            0,
            false,
            u64::MAX,
        )?;
        let objects: BTreeMap<_, _> = manifest
            .objects
            .iter()
            .map(|r| (r.sha256.as_str(), r))
            .collect();
        let mut closure = BTreeSet::new();
        let mut visited = BTreeSet::new();
        let mut active = BTreeSet::new();
        for reference in inputs
            .entries
            .iter()
            .flat_map(|entry| std::iter::once(&entry.object).chain(&entry.dependencies))
        {
            walk(
                reference,
                &objects,
                &manifest,
                &mut visited,
                &mut active,
                &mut closure,
                false,
                0,
                &StorageLimits::default(),
            )?;
        }
        // Reject an over-cap selected closure before hashing any of its blobs.
        let limits = StorageLimits::default();
        manifest
            .objects
            .iter()
            .filter(|reference| {
                closure.contains(&reference.sha256) && reference.record_kind.is_owned_bytes()
            })
            .try_fold(0_u64, |bytes, reference| {
                bytes
                    .checked_add(reference.byte_length)
                    .filter(|bytes| *bytes <= limits.owned_bytes)
                    .ok_or(BundleError::Capacity("recovery owned bytes"))
            })?;
        for reference in manifest
            .objects
            .iter()
            .filter(|r| closure.contains(&r.sha256))
        {
            match reference.record_kind {
                RecordKind::InputChunkTable => {
                    let table = load::<InputChunkTable>(&namespace, reference, &admission)?;
                    compare_edges(reference, &*table, &manifest)?;
                }
                kind if kind.is_owned_bytes() => {
                    drop(ProjectStore::verify_in_namespace(
                        &namespace,
                        reference,
                        &admission.data.usage,
                        &StorageLimits::default(),
                    )?);
                }
                _ => {
                    return Err(BundleError::Corrupt(
                        "unexpected recovery input dependency".into(),
                    ));
                }
            }
        }
        for entry in &inputs.entries {
            if entry.object.record_kind == RecordKind::OwnedMedia {
                load::<InputChunkTable>(&namespace, entry.chunk_table_ref()?, &admission)?
                    .validate_binding(&entry.object, &StorageLimits::default())?;
            }
        }
        let history_ref = checkpoint.history_ref.clone();
        namespace.verify_admitted_root(admission.raw_bytes())?;
        Ok(Some(Self {
            namespace,
            admission,
            document,
            lineage,
            inputs,
            manifest,
            disclosure: UndoHistoryUnavailable {
                history_ref,
                reason,
            },
        }))
    }

    fn copy_input(
        &self,
        reference: &ObjectRef,
        target: &mut ProjectStore,
        copied: &mut BTreeSet<String>,
        depth: u64,
    ) -> Result<(), BundleError> {
        if copied.contains(&reference.sha256) {
            return Ok(());
        }
        if depth > target.limits().graph_depth {
            return Err(BundleError::Capacity("recovery input depth"));
        }
        let dependencies = self
            .manifest
            .dependencies
            .get(&reference.sha256)
            .ok_or_else(|| BundleError::Corrupt("recovery input edges missing".into()))?;
        for child in dependencies {
            self.copy_input(child, target, copied, depth + 1)?;
        }
        let actual = if reference.record_kind == RecordKind::InputChunkTable {
            let table = load::<InputChunkTable>(&self.namespace, reference, &self.admission)?;
            target.admit_chunk_table(&table)?.reference
        } else if reference.record_kind.is_owned_bytes() {
            let mut file = ProjectStore::verify_in_namespace(
                &self.namespace,
                reference,
                &self.admission.data.usage,
                &StorageLimits::default(),
            )?;
            target
                .admit_blob(&mut file, reference, dependencies)?
                .reference
        } else {
            return Err(BundleError::Corrupt(
                "unexpected recovery copy dependency".into(),
            ));
        };
        if actual != *reference {
            return Err(BundleError::Corrupt(
                "recovery copy identity changed".into(),
            ));
        }
        copied.insert(reference.sha256.clone());
        Ok(())
    }

    pub fn save_copy(&self, path: &Path) -> Result<BundleSaveReport, BundleError> {
        self.namespace
            .verify_admitted_root(self.admission.raw_bytes())?;
        let inspection = DestinationInspection::inspect(path, &[&self.namespace])?;
        inspection.revalidate(&[&self.namespace])?;
        let prepared = ProjectNamespace::prepare_rescue_destination(&inspection, &mut || Ok(()))?;
        let destination = ProjectNamespace::inspect(&prepared.path)?;
        destination.matches_destination(&prepared)?;
        if destination.read_root()?.is_some() || destination.has_asset_store()? {
            return Err(BundleError::Identity(
                "recovery destination is occupied".into(),
            ));
        }
        destination.probe_copy_contention()?;
        let mut target = ProjectBundle::activate_copy(
            &prepared.path,
            BundleWriteAdmission::new_document(),
            &prepared,
        )?;
        target
            .store
            .with_namespace(|n| n.seal_destination_ancestors())?;
        let mut copied = BTreeSet::new();
        for entry in &self.inputs.entries {
            for dependency in &entry.dependencies {
                self.copy_input(dependency, &mut target.store, &mut copied, 0)?;
            }
            self.copy_input(&entry.object, &mut target.store, &mut copied, 0)?;
        }
        let inputs = target
            .admit_inputs(&OwnedInputManifest {
                project_id: Some(target.project_id.clone()),
                session_id: None,
                owner_epoch: target.store.owner_epoch(),
                entries: self.inputs.entries.clone(),
            })?
            .reference;
        let history = HistorySnapshot {
            floor_offset: 0,
            baseline: Arc::clone(&self.document),
            document: Arc::clone(&self.document),
            batches: vec![],
            cursor: HistoryCursor {
                undo: vec![],
                redo: vec![],
            },
        };
        self.namespace
            .verify_admitted_root(self.admission.raw_bytes())?;
        let branch = target.manual_branch_id.clone();
        let result = target.with_person_publication(|target| {
            BundleCommit::publish_branch(
                target,
                &history,
                &inputs,
                &branch,
                Some(BranchSeed {
                    lineage: &self.lineage,
                    role: HistoryRole::Ordinary,
                }),
            )
        });
        match result {
            Err(BundleError::DurabilityUnknown) => {
                // The diagnostic keeps the admitted identity after its parent guard drops.
                Err(BundleError::DurabilityUnknownAt(prepared.path.clone()))
            }
            result => result,
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "checkpoint_tests.rs"]
mod tests;
