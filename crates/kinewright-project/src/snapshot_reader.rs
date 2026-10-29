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
        if authority.binding() != lease.payload_sha256 {
            return Err(BundleError::Identity(
                "snapshot authority belongs to another lease".into(),
            ));
        }
        authority
            .check()
            .map_err(|e| BundleError::Identity(e.to_string()))?;
        if lease.id.is_empty()
            || lease.id.len() > 128
            || lease.digest()? != lease.payload_sha256
            || lease.manifest.project_id.is_none()
            || lease.manifest.session_id.is_some()
        {
            return Err(BundleError::Identity("snapshot lease binding".into()));
        }
        let mut namespace = ProjectNamespace::inspect(path)?;
        namespace.lend_read_authority(authority)?;
        let mut store = ProjectStore::new(
            namespace,
            lease.manifest.project_id.clone(),
            None,
            self.limits.clone(),
        )?;
        let inner = Arc::get_mut(&mut store.inner).ok_or(BundleError::Unsettled)?;
        inner.usage = Arc::clone(&self.usage);
        let manifest = &lease.manifest;
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
        let selected = store.closure(&lease.descriptor.references())?;
        if selected.len() != manifest.objects.len()
            || !selected.iter().all(|r| manifest.objects.contains(r))
        {
            return Err(BundleError::Corrupt(
                "snapshot graph is not the selected closure".into(),
            ));
        }
        store.check_read_authority()?;
        Ok(HostSnapshotReader {
            store,
            descriptor: lease.descriptor.clone(),
        })
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
