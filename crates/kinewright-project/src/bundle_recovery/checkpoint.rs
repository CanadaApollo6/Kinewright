//! Independently verified checkpoint rescue. Failed history grants no writer authority.
#![allow(clippy::missing_errors_doc)]
use super::{RecoveryAssessment, compare_edges, read_record, walk};
use crate::{
    bundle::{BundleError, BundleSaveReport, BundleWriteAdmission, ProjectBundle},
    bundle_records::{BranchCheckpoint, BranchId, ObjectRef, RecordKind},
    namespace::{DestinationInspection, ProjectNamespace},
    owned_inputs::OwnedInputManifest,
    owned_media::InputChunkTable,
    storage_budget::{StorageLimits, StorageReservation},
    store::{BundleRecord, ProjectStore, StoreManifest},
};
use kinewright_core::Document;
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
    pool: crate::operation_history::HistoryPool,
    epoch: u64,
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

/// Whether a failed owner read proves the stored history unusable: a decode or
/// identity failure of the bytes the store holds under that reference. Only
/// that authorizes a rescue with empty history; every other failure is a
/// refusal that leaves the record untouched and retryable. Exhaustive, so a
/// new error variant does not compile until it is classified here.
fn history_bytes_unusable(error: &crate::operation_history::Error) -> bool {
    use crate::operation_history::Error as H;
    use kinewright_core::undo::Error as C;
    match error {
        H::History(
            C::Batch(_)
            | C::InvalidDepth
            | C::InvalidRecord
            | C::InvalidMetadata
            | C::InvalidDocument(_)
            | C::DocumentMismatch
            | C::InvalidUndo(_)
            | C::InvalidRedo(_)
            | C::UnsupportedCommand,
        )
        | H::InvalidLedger
        | H::InvalidFrontier
        | H::InvalidPolicy
        | H::InvalidOrigin
        | H::InvalidSeeds
        | H::MissingResource
        | H::InvalidResource
        | H::InvalidRecord
        | H::IdentityMismatch
        // Decode errors, digest or length mismatches, and a content address
        // the store does not hold.
        | H::StoreCorrupt(_) => true,
        H::History(
            C::StepTooLarge
            | C::HistoryFull
            | C::Capacity
            | C::DocumentCapacity
            | C::CounterExhausted
            | C::Encoding,
        )
        | H::Capacity(_)
        | H::PoolFull(_)
        | H::Reservation
        | H::StoreCapacity(_)
        | H::StoreUnsettled
        | H::StoreRunLimit
        // I/O (EACCES and the like), identity of the session, contention.
        | H::Store(_)
        | H::CommandConflict
        | H::NoTargetStep
        | H::StaleTransition
        | H::InputRequired => false,
    }
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
    pub fn open(
        path: &Path,
        pool: &crate::operation_history::HistoryPool,
    ) -> Result<Option<Self>, BundleError> {
        Self::open_with_limits(path, &StorageLimits::default(), pool)
    }
    #[allow(
        clippy::too_many_lines,
        reason = "one independently verified source cut before granting a rescue capability"
    )]
    fn open_with_limits(
        path: &Path,
        limits: &StorageLimits,
        pool: &crate::operation_history::HistoryPool,
    ) -> Result<Option<Self>, BundleError> {
        let RecoveryAssessment::Quarantined {
            admission,
            branches,
            verified_document_bytes,
            ..
        } = super::assess_with_limits(path, limits, pool)?
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
            || checkpoint.owner_ref != failed.history_ref
            || !verified_document_bytes.contains(&checkpoint.document_ref)
        {
            return Err(BundleError::Corrupt("recovery checkpoint identity".into()));
        }
        // Only unusable stored bytes authorize discarding Undo. Capacity,
        // permission and other store failures leave the record retryable.
        let reason = match crate::operation_history::StoredOwnerRecord::read_namespace(
            &namespace,
            &checkpoint,
            pool,
            &admission.data.usage,
            limits,
        ) {
            Err(error) if history_bytes_unusable(&error) => error.to_string(),
            Err(
                crate::operation_history::Error::Capacity(_)
                | crate::operation_history::Error::PoolFull(_),
            ) => {
                return Err(BundleError::Capacity("checkpoint recovery admission"));
            }
            Err(error) => return Err(error.into()),
            Ok(_) => return Err(BundleError::HistoryChanged),
        };
        let document = load::<Document>(&namespace, &checkpoint.document_ref, &admission)?;
        if admission.inspection_document() != Some(&*document) {
            return Err(BundleError::Corrupt(
                "recovery checkpoint document mismatch".into(),
            ));
        }
        let epoch = checkpoint
            .position
            .epoch
            .checked_add(1)
            .ok_or(BundleError::Capacity("install epoch"))?;
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
        let inputs = inputs.map(|mut inputs| {
            crate::bundle_commit::retain_document_inputs(&document, &mut inputs);
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
        let history_ref = checkpoint.owner_ref.clone();
        namespace.verify_admitted_root(admission.raw_bytes())?;
        Ok(Some(Self {
            namespace,
            admission,
            document,
            pool: pool.clone(),
            epoch,
            inputs,
            manifest,
            disclosure: UndoHistoryUnavailable {
                history_ref,
                reason: format!(
                    "{reason}. Rescue keeps the current document; previous Undo steps, approvals and comment links cannot be recovered."
                ),
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
            &self.pool,
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
        let branch = target.manual_branch_id.clone();
        let origin = crate::operation_history::CreationOrigin {
            branch: branch.clone(),
            batch: crate::bundle_records::BatchId::fresh()?,
        };
        let owner = crate::operation_history::OwnerHistory::new(
            &self.pool,
            crate::operation_history::Role::Ordinary,
            kinewright_core::undo::RetentionPolicy::Fold,
            &origin,
            || Ok(Arc::clone(&self.document)),
        )?;
        let selected = owner.capture()?.into_install_at(self.epoch)?;
        drop(owner);
        let snapshot = selected.validate()?;
        let core = crate::operation_actor::Core::open(
            snapshot,
            branch,
            kinewright_core::TimelineRevision(1),
        )
        .map_err(crate::bundle::operation_error)?;
        let attachment = target.attach_core(&core)?;
        let cut = attachment
            .freeze()
            .map_err(crate::bundle::operation_error)?;
        self.namespace
            .verify_admitted_root(self.admission.raw_bytes())?;
        let result =
            target.with_person_publication(|target| target.save_frozen(&core, &inputs, &cut));
        attachment
            .retire(&cut)
            .map_err(crate::bundle::operation_error)?;
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
