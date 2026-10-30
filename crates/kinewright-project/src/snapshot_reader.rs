//! Borrowed immutable snapshot objects. The host owns files, pins and collection.
#![allow(clippy::missing_errors_doc)]
use super::{
    BundleError, BundleRecord, ObjectRef, ProjectNamespace, ProjectStore, RecoveryInventory,
    StorageLimits, StorageReservation, StorageUsage, StoreManifest,
};
use crate::{
    studio_records::{SnapshotDescriptor, SnapshotKey},
    studio_store::{FrozenContext, PinnedSnapshot},
};
use kinewright_media::owned_io::ReadAuthority;
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotLease {
    pub id: String,
    pub descriptor: SnapshotDescriptor,
    pub manifest: StoreManifest,
    pub payload_sha256: String,
}
impl SnapshotLease {
    pub fn digest(&self) -> Result<String, BundleError> {
        Ok(kinewright_media::sha256_bytes(
            &serde_json::to_vec(&(&self.id, &self.descriptor, &self.manifest))
                .map_err(|e| BundleError::Corrupt(e.to_string()))?,
        ))
    }
}

/// The host's current edit input graph: the media and resources Current-edit
/// playback reads. The document is the GUI's accepted view, checked against
/// these inputs; the host ends the lease when its owner or inputs move (S3 C11).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurrentEditLease {
    pub id: String,
    pub owner_epoch: u64,
    pub inputs: ObjectRef,
    pub manifest: StoreManifest,
    pub payload_sha256: String,
}
impl CurrentEditLease {
    pub fn digest(&self) -> Result<String, BundleError> {
        Ok(kinewright_media::sha256_bytes(
            &serde_json::to_vec(&(&self.id, self.owner_epoch, &self.inputs, &self.manifest))
                .map_err(|e| BundleError::Corrupt(e.to_string()))?,
        ))
    }
}

/// One GUI attachment shares this budget across all its open snapshots.
pub struct SnapshotReadSession {
    usage: Arc<Mutex<StorageUsage>>,
    limits: StorageLimits,
}
impl SnapshotReadSession {
    pub fn new(limits: StorageLimits) -> Result<Self, BundleError> {
        limits.validate()?;
        Ok(Self {
            usage: Arc::default(),
            limits,
        })
    }
    pub fn open(
        &self,
        path: &Path,
        lease: &SnapshotLease,
        authority: ReadAuthority,
    ) -> Result<HostSnapshotReader, BundleError> {
        let store = self.open_graph(
            path,
            (&lease.id, &lease.payload_sha256, lease.digest()?),
            &lease.manifest,
            &lease.descriptor.references(),
            authority,
        )?;
        Ok(HostSnapshotReader {
            store,
            descriptor: lease.descriptor.clone(),
        })
    }
    /// Opens the current edit's leased input graph (S3 C11).
    pub fn open_current(
        &self,
        path: &Path,
        lease: &CurrentEditLease,
        authority: ReadAuthority,
    ) -> Result<CurrentEditReader, BundleError> {
        let store = self.open_graph(
            path,
            (&lease.id, &lease.payload_sha256, lease.digest()?),
            &lease.manifest,
            std::slice::from_ref(&lease.inputs),
            authority,
        )?;
        Ok(CurrentEditReader {
            store,
            inputs: lease.inputs.clone(),
        })
    }
    fn open_graph(
        &self,
        path: &Path,
        (id, payload_sha256, digest): (&str, &str, String),
        manifest: &StoreManifest,
        roots: &[ObjectRef],
        authority: ReadAuthority,
    ) -> Result<ProjectStore, BundleError> {
        if authority.binding() != payload_sha256 {
            return Err(BundleError::Identity(
                "snapshot authority belongs to another lease".into(),
            ));
        }
        authority
            .check()
            .map_err(|e| BundleError::Identity(e.to_string()))?;
        if id.is_empty()
            || id.len() > 128
            || digest != payload_sha256
            || manifest.project_id.is_none()
            || manifest.session_id.is_some()
        {
            return Err(BundleError::Identity("snapshot lease binding".into()));
        }
        let mut namespace = ProjectNamespace::inspect(path)?;
        namespace.lend_read_authority(authority)?;
        let mut store = ProjectStore::new(
            namespace,
            manifest.project_id.clone(),
            None,
            self.limits.clone(),
        )?;
        let inner = Arc::get_mut(&mut store.inner).ok_or(BundleError::Unsettled)?;
        inner.usage = Arc::clone(&self.usage);
        if manifest.objects.len() as u64 > self.limits.objects
            || manifest.dependencies.len() != manifest.objects.len()
        {
            return Err(BundleError::Capacity("snapshot graph"));
        }
        let mut metadata = 0_u64;
        let mut edges = 0_u64;
        let ledger = inner
            .ledger
            .get_mut()
            .map_err(|_| BundleError::Capacity("reader ledger poisoned"))?;
        for reference in &manifest.objects {
            reference.validate()?;
            let dependencies = manifest
                .dependencies
                .get(&reference.sha256)
                .ok_or_else(|| BundleError::Corrupt("snapshot dependency row".into()))?;
            metadata = metadata
                .checked_add(crate::storage_budget::object_metadata_charge(
                    reference,
                    dependencies.len(),
                )?)
                .ok_or(BundleError::Capacity("snapshot metadata"))?;
            edges = edges
                .checked_add(dependencies.len() as u64)
                .ok_or(BundleError::Capacity("snapshot edges"))?;
            if ledger
                .objects
                .insert(
                    reference.sha256.clone(),
                    (reference.clone(), dependencies.clone()),
                )
                .is_some()
            {
                return Err(BundleError::Corrupt("duplicate snapshot object".into()));
            }
        }
        ledger.inventory_dependencies = manifest.inventory_dependencies.clone();
        ledger.authenticated_read_graph = true;
        inner.read_graph_charge = Some(GraphCharge::new(
            Arc::clone(&self.usage),
            &self.limits,
            metadata,
            manifest.objects.len() as u64,
            edges,
        )?);
        // Validate the entire closed graph, not just the first media object used.
        let selected = store.closure(roots)?;
        if selected.len() != manifest.objects.len()
            || !selected.iter().all(|r| manifest.objects.contains(r))
        {
            return Err(BundleError::Corrupt(
                "snapshot graph is not the selected closure".into(),
            ));
        }
        store.check_read_authority()?;
        Ok(store)
    }
    pub fn usage(&self) -> Result<StorageUsage, BundleError> {
        self.usage
            .lock()
            .map(|u| u.clone())
            .map_err(|_| BundleError::Capacity("reader usage poisoned"))
    }
}

pub struct HostSnapshotReader {
    store: ProjectStore,
    descriptor: SnapshotDescriptor,
}
impl HostSnapshotReader {
    #[must_use]
    pub fn descriptor(&self) -> &SnapshotDescriptor {
        &self.descriptor
    }
    pub fn frozen_context(&self) -> Result<FrozenContext, BundleError> {
        self.store.leased_frozen_context(&self.descriptor)
    }
    pub fn read_record<T: BundleRecord>(
        &self,
        reference: &ObjectRef,
    ) -> Result<StorageReservation<T>, BundleError> {
        self.store.read_record(reference)
    }
}

pub struct CurrentEditReader {
    store: ProjectStore,
    inputs: ObjectRef,
}
impl CurrentEditReader {
    #[must_use]
    pub fn inputs(&self) -> &ObjectRef {
        &self.inputs
    }
    /// Admits the accepted document's playback inputs from the leased graph.
    pub fn playback_inputs(
        &self,
        document: Arc<kinewright_core::Document>,
    ) -> Result<Arc<kinewright_media::owned_io::PlaybackInputs>, BundleError> {
        self.store.check_read_authority()?;
        self.store.manual_playback_inputs(document, &self.inputs)
    }
}

/// Metadata belongs to the GUI budget; immutable file bytes stay charged to the host.
pub(super) struct GraphCharge {
    usage: Arc<Mutex<StorageUsage>>,
    bytes: u64,
    objects: u64,
    edges: u64,
}
impl GraphCharge {
    fn new(
        usage: Arc<Mutex<StorageUsage>>,
        limits: &StorageLimits,
        bytes: u64,
        objects: u64,
        edges: u64,
    ) -> Result<Self, BundleError> {
        {
            let mut u = usage
                .lock()
                .map_err(|_| BundleError::Capacity("reader usage poisoned"))?;
            let next_bytes = u
                .metadata_bytes
                .checked_add(bytes)
                .ok_or(BundleError::Capacity("reader metadata"))?;
            let next_objects = u
                .objects
                .checked_add(objects)
                .ok_or(BundleError::Capacity("reader objects"))?;
            let next_edges = u
                .edges
                .checked_add(edges)
                .ok_or(BundleError::Capacity("reader edges"))?;
            if next_bytes > limits.metadata_bytes
                || next_objects > limits.objects
                || next_edges > limits.objects.saturating_mul(limits.objects)
            {
                return Err(BundleError::Capacity("shared snapshot graph budget"));
            }
            u.metadata_bytes = next_bytes;
            u.objects = next_objects;
            u.edges = next_edges;
        }
        Ok(Self {
            usage,
            bytes,
            objects,
            edges,
        })
    }
}
impl Drop for GraphCharge {
    fn drop(&mut self) {
        if let Ok(mut u) = self.usage.lock() {
            u.metadata_bytes -= self.bytes;
            u.objects -= self.objects;
            u.edges -= self.edges;
        }
    }
}
impl crate::editing_session::Ja1EditingSession {
    /// Pins the descriptor's complete graph before lending immutable read access.
    pub fn lease_snapshot(
        &self,
        key: &SnapshotKey,
        id: String,
    ) -> Result<(SnapshotLease, PinnedSnapshot), BundleError> {
        let bundle = self.active_bundle()?;
        let pinned = bundle.read_snapshot(key)?;
        let manifest = bundle.store.selected_manifest(
            &pinned.roots,
            &[],
            RecoveryInventory::default(),
            &bundle.admission.data.inventory,
            bundle.generation,
        )?;
        let mut lease = SnapshotLease {
            id,
            descriptor: pinned.descriptor.clone(),
            manifest,
            payload_sha256: String::new(),
        };
        lease.payload_sha256 = lease.digest()?;
        Ok((lease, pinned))
    }
}

impl crate::editing_session::Ja1EditingSession {
    /// Pins the current edit's input graph. Only the inputs the session holds
    /// now can be leased; an older context is refused.
    pub fn lease_current_edit(
        &self,
        inputs: &ObjectRef,
        id: String,
    ) -> Result<(CurrentEditLease, super::StorePin), BundleError> {
        let bundle = self.active_bundle()?;
        if inputs != self.current_inputs_ref() {
            return Err(BundleError::HistoryChanged);
        }
        let roots = vec![inputs.clone()];
        let pin = bundle.store.pin(&roots)?;
        let manifest = bundle.store.selected_manifest(
            &roots,
            &[],
            RecoveryInventory::default(),
            &bundle.admission.data.inventory,
            bundle.generation,
        )?;
        let mut lease = CurrentEditLease {
            id,
            owner_epoch: bundle.store.owner_epoch(),
            inputs: inputs.clone(),
            manifest,
            payload_sha256: String::new(),
        };
        lease.payload_sha256 = lease.digest()?;
        Ok((lease, pin))
    }
}

impl crate::editing_session::Ja1EditingSession {
    pub fn reserve_snapshot_lease(&self, bytes: u64) -> Result<StorageReservation, BundleError> {
        self.active_bundle()?.store.reserve(0, bytes, 0, false)
    }
}

#[cfg(all(target_os = "linux", any(test, feature = "test-util")))]
impl crate::editing_session::Ja1EditingSession {
    /// The actual named-writer retirement boundary, without the GUI handoff's
    /// fail-closed attachment state, for held-reader witnesses.
    pub fn retire_snapshot_writer_probe(&mut self) -> Result<(), BundleError> {
        let bundle = self.active_bundle_mut()?;
        bundle.store.release_named_writer_for_host(
            bundle.admission.raw_bytes(),
            usize::from(bundle.durable.is_some()),
        )
    }
    pub fn collect_snapshot_objects_probe(&mut self) -> Result<u64, BundleError> {
        self.active_bundle_mut()?.collect_after_ack()
    }
}

impl crate::editing_session::Ja1EditingSession {
    pub fn check_snapshot_authority(&self) -> Result<(), BundleError> {
        self.active_bundle()?
            .store
            .with_namespace(|namespace| namespace.verify_read_authority())
    }
}
