//! One authority owns object admission, verification, pins and retirement.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::BundleError,
    bundle_records::{ObjectRef, ProjectId, RecordKind, SessionId},
    namespace::{PortablePath, ProjectNamespace},
    storage_budget::{StorageLimits, StorageReservation, StorageUsage},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreManifest {
    pub project_id: Option<ProjectId>,
    pub session_id: Option<SessionId>,
    pub generation: u64,
    pub objects: Vec<ObjectRef>,
    pub dependencies: BTreeMap<String, Vec<ObjectRef>>,
    pub inventory_dependencies: BTreeMap<String, Vec<ObjectRef>>,
    pub durable_pins: Vec<ObjectRef>,
    pub recovery: RecoveryInventory,
    pub required_fields: BTreeSet<String>,
    pub required_apis: BTreeMap<String, u32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub evidence_roles: BTreeMap<String, String>,
    pub required_kinds: BTreeSet<RecordKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mirrors: Vec<(ObjectRef, PortablePath)>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryInventory {
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation_project_id: Option<ProjectId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_branch_id: Option<crate::bundle_records::BranchId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace_root_file_name: Option<String>,
    pub predecessors: Vec<ObjectRef>,
    pub staging: Option<ObjectRef>,
    pub pre_migration: Option<ObjectRef>,
}

#[derive(Debug, Default, Clone)]
pub struct CollectionStatus {
    /// At most 32 paths; totals cover every failed file in the latest pass.
    pub failures: Vec<CollectionFailure>,
    pub failed_files: u64,
    pub pending_bytes: u64,
}
#[derive(Debug, Clone)]
pub struct CollectionFailure {
    pub path: std::path::PathBuf,
    pub attempts: u64,
    pub error: String,
}

struct Ledger {
    // A leased reader receives the host's authenticated graph declarations.
    // This admits graph traversal only: local byte reads still hash the object
    // or certify its complete media fingerprint and chunks before exposure.
    authenticated_read_graph: bool,
    collection: CollectionStatus,
    objects: BTreeMap<String, (ObjectRef, Vec<ObjectRef>)>,
    pins: BTreeMap<String, u64>,
    inventory_dependencies: BTreeMap<String, Vec<ObjectRef>>,
    verified: BTreeSet<String>,
    support_fields: BTreeMap<String, StorageReservation<BTreeSet<String>>>,
    committed: Vec<ObjectRef>,
    acknowledged_objects: BTreeSet<String>,
    mirrors: Vec<(ObjectRef, PortablePath)>,
    can_collect: bool,
    metadata_sizes: BTreeMap<String, u64>,
    pending: Vec<StorageReservation>,
    pending_controls: Vec<StorageReservation>,
    copy_holds: u64,
    unclassified: BTreeMap<String, u64>,
    temporaries: Vec<crate::namespace::PhysicalFile>,
}

struct WeakCacheEntry<T> {
    value: std::sync::Weak<T>,
    _charge: StorageReservation,
}

struct MediaAdmission {
    binding: kinewright_media::owned_io::SourceAdmissionMemo,
    _charge: StorageReservation,
}

struct StoreInner {
    read_authority: Option<kinewright_media::owned_io::ReadAuthority>,
    read_graph_charge: Option<snapshot_reader::GraphCharge>,
    namespace: Mutex<ProjectNamespace>,
    verified_memo: Mutex<crate::bundle_recovery::VerifiedIdentityMemo>,
    chunk_tables:
        Mutex<BTreeMap<ObjectRef, WeakCacheEntry<crate::owned_media::CertifiedChunkTable>>>,
    resources:
        Mutex<BTreeMap<ObjectRef, WeakCacheEntry<crate::certified_resources::ResourceBytes>>>,
    media_admissions: Mutex<BTreeMap<(ObjectRef, ObjectRef, String), MediaAdmission>>,
    ledger: Mutex<Ledger>,
    usage: Arc<Mutex<StorageUsage>>,
    limits: StorageLimits,
    root_bytes: Mutex<u64>,
    inventory_bytes: Mutex<u64>,
    project_id: Option<ProjectId>,
    session_id: Option<SessionId>,
    owner_epoch: u64,
    session_closed: std::sync::atomic::AtomicBool,
    session_closing: std::sync::atomic::AtomicBool,
}

pub struct ProjectStore {
    inner: Arc<StoreInner>,
}
pub struct StorePin {
    inner: Arc<StoreInner>,
    objects: Vec<String>,
}
pub struct SealedObject {
    pub reference: ObjectRef,
    pin: StorePin,
}

/// Unforgeable access to a verified, pinned closure under its original store owner.
/// No caller locator or external hint is accepted by reads.
pub struct RetainedReadAdmission {
    hold: Arc<SourceCopyHold>,
    objects: StorageReservation<Vec<ObjectRef>>,
}
pub(crate) struct RetainedReadLease {
    file: File,
    _hold: Arc<SourceCopyHold>,
    _charge: StorageReservation,
}
impl Read for RetainedReadLease {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(buffer)
    }
}
impl Seek for RetainedReadLease {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.file.seek(position)
    }
}
impl RetainedReadAdmission {
    pub(crate) fn reserve_snapshot_growth(
        &self,
        overlap: &StorageReservation,
        encoded_bytes: u64,
    ) -> Result<StorageReservation, BundleError> {
        let store = ProjectStore {
            inner: Arc::clone(&self.hold.pin.inner),
        };
        if store
            .inner
            .session_closing
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(BundleError::Identity(
                "originating source is closing".into(),
            ));
        }
        let free = store.with_namespace(|n| n.available_bytes())?;
        overlap.reserve_copy_growth(encoded_bytes, store.limits(), free)
    }
    pub(crate) fn reserve_snapshot_scratch(
        &self,
        encoded_bytes: u64,
    ) -> Result<StorageReservation, BundleError> {
        let store = ProjectStore {
            inner: Arc::clone(&self.hold.pin.inner),
        };
        store.reserve_copy_read(0, encoded_bytes, 0, false, true)
    }
    pub(crate) fn inspect_destination(
        &self,
        path: &Path,
    ) -> Result<crate::namespace::DestinationInspection, BundleError> {
        let store = ProjectStore {
            inner: Arc::clone(&self.hold.pin.inner),
        };
        ProjectStore::inspect_borrowed_destination(path, &[&store])
    }
    #[must_use]
    pub fn objects(&self) -> &[ObjectRef] {
        &self.objects
    }
    pub fn open(&self, reference: &ObjectRef) -> Result<impl Read + Seek + use<>, BundleError> {
        self.open_lease(reference)
    }
    pub(crate) fn open_lease(
        &self,
        reference: &ObjectRef,
    ) -> Result<RetainedReadLease, BundleError> {
        if !self.objects.contains(reference) {
            return Err(BundleError::Identity(
                "object outside retained admission".into(),
            ));
        }
        let store = ProjectStore {
            inner: Arc::clone(&self.hold.pin.inner),
        };
        let path = ProjectStore::object_path(reference)?;
        crate::namespace::note("rescue_permission_read", path.as_path());
        let charge = store.reserve_copy_read(0, 256, 0, false, true)?;
        let file = store.open_copy_source(reference, true)?;
        Ok(RetainedReadLease {
            file,
            _hold: Arc::clone(&self.hold),
            _charge: charge,
        })
    }
}
impl ProjectStore {
    /// Explicit originating Close. Keep this owner for retry on failure or while
    /// readers still hold the real pin; no host descriptor can call this method.
    pub(crate) fn close_temporary_source(&mut self) -> Result<bool, BundleError> {
        use std::sync::atomic::Ordering;
        if self.inner.session_closed.load(Ordering::SeqCst) {
            return Ok(true);
        }
        if self.inner.session_id.is_none() {
            return Err(BundleError::Identity(
                "only the originating temporary owner can close its source".into(),
            ));
        }
        self.inner.session_closing.store(true, Ordering::SeqCst);
        if Arc::strong_count(&self.inner) != 1 {
            return Ok(false);
        }
        let needs_collection = {
            let mut ledger = self
                .inner
                .ledger
                .lock()
                .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
            if ledger.copy_holds != 0 || ledger.pins.values().any(|n| *n != 0) {
                return Ok(false);
            }
            if !ledger.pending.is_empty() || !ledger.pending_controls.is_empty() {
                return Err(BundleError::Unsettled);
            }
            ledger.can_collect = true;
            ledger.committed.clear();
            !ledger.objects.is_empty()
                || !ledger.unclassified.is_empty()
                || !ledger.temporaries.is_empty()
        };
        let result = (|| {
            if needs_collection {
                collect(&self.inner)?;
            }
            self.with_namespace(ProjectNamespace::cleanup_session)?;
            self.inner.session_closed.store(true, Ordering::SeqCst);
            Ok(true)
        })();
        self.inner
            .usage
            .lock()
            .map_err(|_| BundleError::Capacity("usage poisoned"))?
            .unsettled = result.is_err();
        result
    }

    /// Only the bundle's own durable pin may remain at a named writer handover.
    /// Check and release under the actual pinned namespace, before a new host acquires it.
    pub(crate) fn release_named_writer_for_host(
        &mut self,
        expected_root: &[u8],
        own_pins: usize,
    ) -> Result<(), BundleError> {
        if self.inner.session_id.is_some() || Arc::strong_count(&self.inner) != 1 + own_pins {
            return Err(BundleError::Identity(
                "named writer still has source holds or readers".into(),
            ));
        }
        let mut ledger = self
            .inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
        if ledger.copy_holds != 0
            || !ledger.pending.is_empty()
            || !ledger.pending_controls.is_empty()
        {
            return Err(BundleError::Unsettled);
        }
        let mut namespace = self
            .inner
            .namespace
            .lock()
            .map_err(|_| BundleError::Capacity("namespace poisoned"))?;
        namespace.verify()?;
        if namespace.read_root()?.as_deref() != Some(expected_root) {
            return Err(BundleError::Identity(
                "named handover checkpoint changed".into(),
            ));
        }
        // The released owner must do no collection after giving up the lock.
        ledger.can_collect = false;
        namespace.release()?;
        Ok(())
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn retention_probe(&self) -> Result<(u64, BTreeMap<String, u64>), BundleError> {
        let ledger = self
            .inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
        Ok((ledger.copy_holds, ledger.pins.clone()))
    }
    pub(crate) fn reserve_retained_capture(
        &self,
        bytes: &[u8],
    ) -> Result<StorageReservation, BundleError> {
        let decoded = preflight_record(bytes, self.limits())?;
        self.reserve_copy_read(0, decoded, decoded, false, true)
    }
    pub(crate) fn reserve_capture_scratch(&self) -> Result<StorageReservation, BundleError> {
        self.reserve_copy_read(0, self.limits().decoded_record_bytes, 0, false, true)
    }
    pub fn inspect_rescue_destination(
        path: &Path,
        stores: &[&Self],
    ) -> Result<crate::namespace::DestinationInspection, BundleError> {
        Self::inspect_copy_destination(path, stores, false)
    }
    pub fn inspect_borrowed_destination(
        path: &Path,
        stores: &[&Self],
    ) -> Result<crate::namespace::DestinationInspection, BundleError> {
        Self::inspect_copy_destination(path, stores, true)
    }
    fn inspect_copy_destination(
        path: &Path,
        stores: &[&Self],
        borrowed: bool,
    ) -> Result<crate::namespace::DestinationInspection, BundleError> {
        let mut stores = stores.to_vec();
        stores.sort_by_key(|store| Arc::as_ptr(&store.inner));
        stores.dedup_by(|a, b| Arc::ptr_eq(&a.inner, &b.inner));
        let guards = stores
            .iter()
            .map(|s| {
                s.inner
                    .namespace
                    .lock()
                    .map_err(|_| BundleError::Capacity("namespace poisoned"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let namespaces = guards.iter().map(|g| &**g).collect::<Vec<_>>();
        if borrowed {
            crate::namespace::DestinationInspection::inspect_borrowed(path, &namespaces)
        } else {
            crate::namespace::DestinationInspection::inspect(path, &namespaces)
        }
    }
    pub fn revalidate_rescue_destination(
        inspection: &crate::namespace::DestinationInspection,
        stores: &[&Self],
    ) -> Result<(), BundleError> {
        let mut stores = stores.to_vec();
        stores.sort_by_key(|store| Arc::as_ptr(&store.inner));
        stores.dedup_by(|a, b| Arc::ptr_eq(&a.inner, &b.inner));
        let guards = stores
            .iter()
            .map(|s| {
                s.inner
                    .namespace
                    .lock()
                    .map_err(|_| BundleError::Capacity("namespace poisoned"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        inspection.revalidate(&guards.iter().map(|g| &**g).collect::<Vec<_>>())
    }
    /// Diagnose each retaining role independently, then page a bounded frozen result.
    pub fn diagnose_retained_reads(
        &self,
        roots: &[(ObjectRef, crate::bundle_rescue::MissingOwnedUse)],
        offset: usize,
        page: usize,
    ) -> Result<crate::bundle_rescue::RescueMissingOwnedBytes, BundleError> {
        self.diagnose_retained_selection(roots, offset, page, true)
    }
    pub(crate) fn diagnose_retained_content_reads(
        &self,
        roots: &[(ObjectRef, crate::bundle_rescue::MissingOwnedUse)],
        offset: usize,
        page: usize,
    ) -> Result<crate::bundle_rescue::RescueMissingOwnedBytes, BundleError> {
        self.diagnose_retained_selection(roots, offset, page, false)
    }
    fn diagnose_retained_selection(
        &self,
        roots: &[(ObjectRef, crate::bundle_rescue::MissingOwnedUse)],
        offset: usize,
        page: usize,
        inventory: bool,
    ) -> Result<crate::bundle_rescue::RescueMissingOwnedBytes, BundleError> {
        use crate::bundle_rescue::RescueMissingOwnedBytes;
        if roots.len() as u64 > self.limits().objects || page == 0 || page > 32 {
            return Err(BundleError::Capacity("rescue diagnostic page"));
        }
        let mut uses = BTreeMap::<ObjectRef, BTreeSet<_>>::new();
        let mut working = Vec::new();
        let mut complete = true;
        let mut missing = BTreeMap::new();
        for (root, role) in roots {
            match self.closure_with_inventory(std::slice::from_ref(root), inventory) {
                Ok(objects) => {
                    let additions = objects
                        .iter()
                        .filter(|object| {
                            !uses.get(*object).is_some_and(|roles| roles.contains(role))
                        })
                        .count();
                    working.push(self.reserve_copy_read(
                        0,
                        additions as u64 * 1024,
                        0,
                        false,
                        true,
                    )?);
                    for object in objects {
                        uses.entry(object).or_default().insert(role.clone());
                    }
                }
                Err(error) => {
                    working.push(self.reserve_copy_read(0, 1024, 0, false, true)?);
                    complete = false;
                    uses.entry(root.clone()).or_default().insert(role.clone());
                    missing.insert(root.clone(), error.to_string());
                }
            }
        }
        for object in uses.keys() {
            if missing.contains_key(object) {
                continue;
            }
            if let Err(error) = self.open_copy_source(object, true) {
                if !object.record_kind.is_owned_bytes() {
                    complete = false;
                }
                missing.insert(object.clone(), format!("retained object read: {error}"));
            }
        }
        let total = missing.len();
        let entries = missing
            .into_iter()
            .skip(offset)
            .take(page)
            .map(|(object, cause)| {
                let roles = uses
                    .remove(&object)
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
                (object, roles, cause)
            })
            .collect::<Vec<_>>();
        let end = offset.saturating_add(entries.len());
        Ok(RescueMissingOwnedBytes {
            omitted: total.saturating_sub(entries.len()),
            next: (end < total).then_some(end),
            entries,
            dependencies_complete: complete,
        })
    }
    pub fn admit_retained_reads(
        &self,
        roots: &[(ObjectRef, crate::bundle_rescue::MissingOwnedUse)],
    ) -> Result<RetainedReadAdmission, BundleError> {
        self.admit_retained_selection(roots, true)
    }
    pub(crate) fn admit_retained_content_reads(
        &self,
        roots: &[(ObjectRef, crate::bundle_rescue::MissingOwnedUse)],
    ) -> Result<RetainedReadAdmission, BundleError> {
        self.admit_retained_selection(roots, false)
    }
    fn admit_retained_selection(
        &self,
        roots: &[(ObjectRef, crate::bundle_rescue::MissingOwnedUse)],
        inventory: bool,
    ) -> Result<RetainedReadAdmission, BundleError> {
        let diagnostic = self.diagnose_retained_selection(roots, 0, 32, inventory)?;
        if !diagnostic.entries.is_empty() || !diagnostic.dependencies_complete {
            return Err(BundleError::RescueMissingOwnedBytes(Box::new(diagnostic)));
        }
        let roots = roots.iter().map(|(r, _)| r.clone()).collect::<Vec<_>>();
        let objects = self.closure_with_inventory(&roots, inventory)?;
        let bytes = objects.iter().try_fold(0_u64, |n, r| {
            n.checked_add(r.byte_length)
                .ok_or(BundleError::Capacity("retained source bytes"))
        })?;
        let objects = self
            .reserve_copy_read(0, objects.len() as u64 * 256, 0, false, true)?
            .hold(objects);
        let hold = self.begin_copy_source(&roots, bytes, true)?;
        Ok(RetainedReadAdmission {
            hold: Arc::new(hold),
            objects,
        })
    }
}

impl SealedObject {
    #[must_use]
    pub fn into_pin(self) -> StorePin {
        self.pin
    }
}
impl ProjectStore {
    #[cfg(any(test, feature = "test-util"))]
    pub fn reset_decode_peaks_probe(&self) -> Result<(), BundleError> {
        let mut usage = self
            .inner
            .usage
            .lock()
            .map_err(|_| BundleError::Capacity("usage poisoned"))?;
        usage.peak_decoded_bytes = usage.decoded_bytes;
        usage.peak_decoder_bytes = usage.decoder_bytes;
        usage.peak_live_history_bytes = usage.live_history_bytes;
        Ok(())
    }
    pub fn collection_status(&self) -> Result<CollectionStatus, BundleError> {
        Ok(self
            .inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?
            .collection
            .clone())
    }
    pub fn new(
        namespace: ProjectNamespace,
        project_id: Option<ProjectId>,
        session_id: Option<SessionId>,
        limits: StorageLimits,
    ) -> Result<Self, BundleError> {
        limits.validate()?;
        if project_id.is_some() == session_id.is_some() {
            return Err(BundleError::Identity(
                "exactly one project/session owner is required".into(),
            ));
        }
        let read_authority = namespace.read_authority();
        if read_authority.is_some() {
            namespace.verify_read_authority()?;
        } else {
            namespace.verify()?;
        }
        let mut epoch_bytes = [0_u8; 8];
        getrandom::fill(&mut epoch_bytes).map_err(|e| BundleError::Identity(e.to_string()))?;
        let owner_epoch = u64::from_le_bytes(epoch_bytes).max(1);
        let root_bytes = if read_authority.is_some() {
            0
        } else {
            namespace.read_root()?.map_or(0, |b| b.len() as u64)
        };
        let inventory_bytes = if read_authority.is_some() {
            0
        } else {
            namespace
                .read(
                    &crate::bundle_recovery::inventory_path()?,
                    limits.control_bytes,
                )?
                .map_or(0, |b| b.len() as u64)
        };
        let control_bytes = root_bytes
            .checked_add(inventory_bytes)
            .ok_or(BundleError::Capacity("control files"))?;
        let can_collect = read_authority.is_none() && root_bytes == 0;
        Ok(Self {
            inner: Arc::new(StoreInner {
                read_authority,
                read_graph_charge: None,
                namespace: Mutex::new(namespace),
                verified_memo: Mutex::new(crate::bundle_recovery::VerifiedIdentityMemo::default()),
                chunk_tables: Mutex::new(BTreeMap::new()),
                resources: Mutex::new(BTreeMap::new()),
                media_admissions: Mutex::new(BTreeMap::new()),
                ledger: Mutex::new(Ledger {
                    authenticated_read_graph: false,
                    collection: CollectionStatus::default(),
                    objects: BTreeMap::new(),
                    pins: BTreeMap::new(),
                    inventory_dependencies: BTreeMap::new(),
                    unclassified: BTreeMap::new(),
                    temporaries: vec![],
                    verified: BTreeSet::new(),
                    support_fields: BTreeMap::new(),
                    committed: vec![],
                    acknowledged_objects: BTreeSet::new(),
                    mirrors: vec![],
                    can_collect,
                    metadata_sizes: BTreeMap::new(),
                    pending: vec![],
                    pending_controls: vec![],
                    copy_holds: 0,
                }),
                usage: Arc::new(Mutex::new(StorageUsage {
                    physical_bytes: control_bytes,
                    peak_physical_bytes: control_bytes,
                    metadata_bytes: control_bytes,
                    ..StorageUsage::default()
                })),
                root_bytes: Mutex::new(root_bytes),
                inventory_bytes: Mutex::new(inventory_bytes),
                limits,
                project_id,
                session_id,
                owner_epoch,
                session_closed: std::sync::atomic::AtomicBool::new(false),
                session_closing: std::sync::atomic::AtomicBool::new(false),
            }),
        })
    }
    pub fn usage(&self) -> Result<StorageUsage, BundleError> {
        self.inner
            .usage
            .lock()
            .map(|u| u.clone())
            .map_err(|_| BundleError::Capacity("usage poisoned"))
    }
    pub fn resolved_root(&self) -> Result<std::path::PathBuf, BundleError> {
        self.with_namespace(|n| Ok(n.identity().resolved_root.clone()))
    }
    pub fn reserve_copy(&self, bytes: u64) -> Result<StorageReservation, BundleError> {
        self.reserve(bytes, 0, 0, true)
    }
    #[cfg(any(test, feature = "test-util"))]
    pub fn reserve_probe(
        &self,
        decoded: u64,
        history: u64,
    ) -> Result<StorageReservation, BundleError> {
        self.reserve(0, decoded, history, false)
    }
    pub fn reserve_history_shadow(&self, bytes: u64) -> Result<StorageReservation, BundleError> {
        if bytes > self.limits().branch_history_bytes {
            return Err(BundleError::Capacity("branch history shadow"));
        }
        self.reserve(0, bytes, bytes, false)
    }
    #[must_use]
    pub fn limits(&self) -> &StorageLimits {
        &self.inner.limits
    }
    pub(crate) fn with_namespace<T>(
        &self,
        run: impl FnOnce(&mut ProjectNamespace) -> Result<T, BundleError>,
    ) -> Result<T, BundleError> {
        let mut namespace = self
            .inner
            .namespace
            .lock()
            .map_err(|_| BundleError::Identity("namespace poisoned".into()))?;
        let result = run(&mut namespace);
        if namespace.is_uncertain()
            || matches!(
                &result,
                Err(BundleError::Identity(_)
                    | BundleError::Unsettled
                    | BundleError::DurabilityUnknown
                    | BundleError::Corrupt(_))
            )
        {
            self.clear_verified_memo()?;
        }
        result
    }
    pub(crate) fn clear_verified_memo(&self) -> Result<(), BundleError> {
        self.inner
            .verified_memo
            .lock()
            .map_err(|_| BundleError::Capacity("memo poisoned"))?
            .clear();
        Ok(())
    }
    pub(crate) fn replace_verified_memo(
        &self,
        memo: crate::bundle_recovery::VerifiedIdentityMemo,
    ) -> Result<(), BundleError> {
        *self
            .inner
            .verified_memo
            .lock()
            .map_err(|_| BundleError::Capacity("memo poisoned"))? = memo;
        Ok(())
    }
    #[cfg(any(test, feature = "test-util"))]
    pub fn clear_verified_memo_probe(&self) -> Result<(), BundleError> {
        self.clear_verified_memo()
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn evict_verified_cost_probe(&self, reference: &ObjectRef) -> Result<(), BundleError> {
        self.inner
            .verified_memo
            .lock()
            .map_err(|_| BundleError::Capacity("memo poisoned"))?
            .forget(reference);
        Ok(())
    }
    #[cfg(any(test, feature = "test-util"))]
    pub fn misbind_verified_cost_probe(
        &self,
        reference: &ObjectRef,
        wrong: ObjectRef,
    ) -> Result<(), BundleError> {
        self.inner
            .verified_memo
            .lock()
            .map_err(|_| BundleError::Capacity("memo poisoned"))?
            .misbind_probe(reference, wrong);
        Ok(())
    }
    pub(crate) fn reserve(
        &self,
        bytes: u64,
        decoded: u64,
        history: u64,
        copy: bool,
    ) -> Result<StorageReservation, BundleError> {
        if self
            .inner
            .session_closing
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(BundleError::Identity("originating source is closed".into()));
        }
        let free = self.with_namespace(|n| n.available_bytes())?;
        StorageReservation::reserve(
            &self.inner.usage,
            &self.inner.limits,
            bytes,
            decoded,
            history,
            copy,
            free,
        )
    }
    fn object_path(reference: &ObjectRef) -> Result<PortablePath, BundleError> {
        reference.validate()?;
        PortablePath::new(&Path::new("studio/objects").join(&reference.sha256))
    }
    /// Stream measured bytes. Declared size/digest are constraints, never observations.
    pub fn admit_blob(
        &mut self,
        reader: &mut impl Read,
        reference: &ObjectRef,
        dependencies: &[ObjectRef],
    ) -> Result<SealedObject, BundleError> {
        if !reference.record_kind.is_blob() {
            return Err(BundleError::Unsupported(
                "typed records require typed record admission".into(),
            ));
        }
        self.admit(reader, reference, dependencies)
    }
    pub(crate) fn admit(
        &mut self,
        reader: &mut impl Read,
        reference: &ObjectRef,
        dependencies: &[ObjectRef],
    ) -> Result<SealedObject, BundleError> {
        self.admit_internal(reader, reference, dependencies, false)
    }
    pub(crate) fn admit_recovered(
        &mut self,
        reader: &mut impl Read,
        reference: &ObjectRef,
        dependencies: &[ObjectRef],
    ) -> Result<SealedObject, BundleError> {
        self.admit_internal(reader, reference, dependencies, true)
    }
    #[allow(clippy::too_many_lines)]
    fn admit_internal(
        &mut self,
        reader: &mut impl Read,
        reference: &ObjectRef,
        dependencies: &[ObjectRef],
        recover: bool,
    ) -> Result<SealedObject, BundleError> {
        reference.validate()?;
        let _working = self.reserve(0, (1 << 20) + 65_536, 0, false)?;
        let mut dependencies = dependencies.to_vec();
        dependencies.sort();
        dependencies.dedup();
        self.closure_at(
            &dependencies,
            reference.record_kind != RecordKind::RootCheckpoint,
            1,
        )?;
        let unclassified = self
            .inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?
            .unclassified
            .get(&reference.sha256)
            .copied();
        if unclassified.is_some_and(|length| length != reference.byte_length) {
            return Err(BundleError::Corrupt(
                "unclassified object length differs".into(),
            ));
        }
        let existing = {
            let ledger = self
                .inner
                .ledger
                .lock()
                .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
            if let Some((known, known_dependencies)) = ledger.objects.get(&reference.sha256) {
                if known != reference || known_dependencies != &dependencies {
                    return Err(BundleError::Corrupt(
                        "same object identity has different kind/edges".into(),
                    ));
                }
                true
            } else {
                let mut usage = self.usage()?;
                if let Some(length) = unclassified {
                    usage.metadata_bytes = usage
                        .metadata_bytes
                        .checked_sub(
                            length
                                .checked_add(256)
                                .ok_or(BundleError::Capacity("unclassified metadata overflow"))?,
                        )
                        .ok_or(BundleError::Capacity("unclassified metadata underflow"))?;
                    usage.owned_bytes = usage
                        .owned_bytes
                        .checked_sub(length)
                        .ok_or(BundleError::Capacity("unclassified owned underflow"))?;
                }
                let metadata_charge = 256_u64
                    .checked_add(
                        (dependencies.len() as u64)
                            .checked_mul(160)
                            .ok_or(BundleError::Capacity("edge metadata overflow"))?,
                    )
                    .and_then(|n| {
                        n.checked_add(if reference.record_kind.is_owned_bytes() {
                            0
                        } else {
                            reference.byte_length
                        })
                    })
                    .ok_or(BundleError::Capacity("metadata overflow"))?;
                if usage
                    .metadata_bytes
                    .checked_add(metadata_charge)
                    .and_then(|n| {
                        n.checked_add(usage.decoded_bytes.checked_sub(usage.decoder_bytes)?)
                    })
                    .is_none_or(|n| n > self.inner.limits.metadata_bytes)
                    || (reference.record_kind.is_owned_bytes()
                        && usage
                            .owned_bytes
                            .checked_add(reference.byte_length)
                            .is_none_or(|n| n > self.inner.limits.owned_bytes))
                {
                    return Err(BundleError::Capacity("object/edge metadata or owned bytes"));
                }
                if usage.objects >= self.inner.limits.objects && unclassified.is_none() {
                    return Err(BundleError::Capacity("object count"));
                }
                false
            }
        };
        if existing {
            let mut counted = CountingReader {
                inner: reader,
                bytes: 0,
                limit: reference.byte_length,
            };
            let digest = kinewright_media::sha256_reader(&mut counted)
                .map_err(|e| BundleError::Corrupt(e.to_string()))?;
            if counted.bytes != reference.byte_length || digest != reference.sha256 {
                return Err(BundleError::Corrupt(
                    "duplicate input differs from owned bytes".into(),
                ));
            }
            self.verify(reference)?;
            return Ok(SealedObject {
                reference: reference.clone(),
                pin: self.pin(std::slice::from_ref(reference))?,
            });
        }
        let reservation = self.reserve(
            if unclassified.is_some() {
                0
            } else {
                reference.byte_length
            },
            0,
            0,
            false,
        )?;
        let path = Self::object_path(reference)?;
        let recovered = (recover || unclassified.is_some())
            && self.with_namespace(|n| n.open_owned(&path))?.is_some();
        let published = if recovered {
            self.with_namespace(|n| {
                Self::verify_in_namespace(n, reference, &self.inner.usage, &self.inner.limits)
            })?;
            let mut measured = CountingReader {
                inner: reader,
                bytes: 0,
                limit: reference.byte_length,
            };
            let digest = kinewright_media::sha256_reader(&mut measured)?;
            if digest == reference.sha256 && measured.bytes == reference.byte_length {
                #[cfg(any(test, feature = "test-util"))]
                crate::io_observer::note_digest(
                    "object_stage",
                    path.as_path(),
                    digest,
                    usize::try_from(measured.bytes)
                        .map_err(|_| BundleError::Capacity("object address space"))?,
                );
                self.with_namespace(|n| n.seal_existing(&path))
            } else {
                Err(BundleError::Corrupt(
                    "recovered input differs from owned object".into(),
                ))
            }
        } else {
            self.with_namespace(|namespace| {
                namespace.publish_with(&path, |file| {
                    let mut bytes = 0_u64;
                    let mut buffer = vec![0_u8; 65_536];
                    loop {
                        let count = reader.read(&mut buffer)?;
                        if count == 0 {
                            break;
                        }
                        bytes = bytes
                            .checked_add(count as u64)
                            .filter(|n| *n <= reference.byte_length)
                            .ok_or(BundleError::Capacity("stream exceeded its reservation"))?;
                        file.write_all(&buffer[..count])?;
                    }
                    if bytes != reference.byte_length {
                        return Err(BundleError::Corrupt("short owned input".into()));
                    }
                    file.seek(SeekFrom::Start(0))?;
                    let digest = kinewright_media::sha256_reader(file)
                        .map_err(|e| BundleError::Corrupt(e.to_string()))?;
                    if digest != reference.sha256 {
                        return Err(BundleError::Corrupt("owned input digest mismatch".into()));
                    }
                    #[cfg(any(test, feature = "test-util"))]
                    crate::io_observer::note_digest(
                        "object_stage",
                        path.as_path(),
                        digest,
                        usize::try_from(bytes)
                            .map_err(|_| BundleError::Capacity("object address space"))?,
                    );
                    Ok(())
                })
            })
        };
        if let Err(error) = published {
            if self.with_namespace(|namespace| Ok(namespace.is_uncertain()))? {
                self.inner
                    .ledger
                    .lock()
                    .map_err(|_| BundleError::Capacity("ledger poisoned"))?
                    .pending
                    .push(reservation);
                self.inner
                    .usage
                    .lock()
                    .map_err(|_| BundleError::Capacity("usage poisoned"))?
                    .unsettled = true;
            }
            return Err(error);
        }
        self.register_published(reference, &dependencies, reservation, unclassified)
    }
    fn register_published(
        &self,
        reference: &ObjectRef,
        dependencies: &[ObjectRef],
        reservation: StorageReservation,
        unclassified: Option<u64>,
    ) -> Result<SealedObject, BundleError> {
        reservation.seal(if unclassified.is_some() {
            0
        } else {
            reference.byte_length
        })?;
        {
            let mut ledger = self
                .inner
                .ledger
                .lock()
                .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
            ledger.objects.insert(
                reference.sha256.clone(),
                (reference.clone(), dependencies.to_vec()),
            );
            ledger.verified.insert(reference.sha256.clone());
            if unclassified.is_some() {
                ledger.unclassified.remove(&reference.sha256);
            }
            let metadata_charge = 256
                + (dependencies.len() as u64) * 160
                + if reference.record_kind.is_owned_bytes() {
                    0
                } else {
                    reference.byte_length
                };
            ledger
                .metadata_sizes
                .insert(reference.sha256.clone(), metadata_charge);
        }
        let mut usage = self
            .inner
            .usage
            .lock()
            .map_err(|_| BundleError::Capacity("usage poisoned"))?;
        if let Some(length) = unclassified {
            usage.owned_bytes = usage
                .owned_bytes
                .checked_sub(length)
                .ok_or(BundleError::Capacity("unclassified owned underflow"))?;
            usage.metadata_bytes = usage
                .metadata_bytes
                .checked_sub(
                    length
                        .checked_add(256)
                        .ok_or(BundleError::Capacity("unclassified metadata overflow"))?,
                )
                .ok_or(BundleError::Capacity("unclassified metadata underflow"))?;
            usage.unclassified_bytes = usage
                .unclassified_bytes
                .checked_sub(length)
                .ok_or(BundleError::Capacity("unclassified bytes underflow"))?;
        }
        usage.objects = usage
            .objects
            .checked_add(u64::from(unclassified.is_none()))
            .ok_or(BundleError::Capacity("object accounting overflow"))?;
        usage.edges = usage
            .edges
            .checked_add(dependencies.len() as u64)
            .ok_or(BundleError::Capacity("edge accounting overflow"))?;
        let metadata_charge = 256
            + (dependencies.len() as u64) * 160
            + if reference.record_kind.is_owned_bytes() {
                0
            } else {
                reference.byte_length
            };
        usage.metadata_bytes = usage
            .metadata_bytes
            .checked_add(metadata_charge)
            .ok_or(BundleError::Capacity("metadata accounting overflow"))?;
        if reference.record_kind.is_owned_bytes() {
            usage.owned_bytes = usage
                .owned_bytes
                .checked_add(reference.byte_length)
                .ok_or(BundleError::Capacity("owned accounting overflow"))?;
        }
        drop(usage);
        Ok(SealedObject {
            reference: reference.clone(),
            pin: self.pin(std::slice::from_ref(reference))?,
        })
    }
    pub(crate) fn admit_measured_media(
        &mut self,
        reader: &mut impl Read,
        length: u64,
    ) -> Result<(SealedObject, crate::owned_inputs::InputChunkTable), BundleError> {
        let count = crate::owned_inputs::InputChunkTable::checked_count(length, self.limits())?;
        let _scratch = self.reserve(0, (1 << 20) + count * 32 + 1024, 0, false)?;
        let usage = self.usage()?;
        if usage.objects >= self.limits().objects
            || usage
                .owned_bytes
                .checked_add(length)
                .is_none_or(|n| n > self.limits().owned_bytes)
            || usage
                .metadata_bytes
                .checked_add(256)
                .is_none_or(|n| n > self.limits().metadata_bytes)
        {
            return Err(BundleError::Capacity("import object admission"));
        }
        let reservation = self.reserve(length, 0, 0, false)?;
        let mut table = None;
        let published = self.with_namespace(|namespace| {
            namespace.publish_hashed(|file| {
                let result = crate::owned_media::stream_media(reader, file, length, count)?;
                let reference = ObjectRef {
                    sha256: result.source_sha256.clone(),
                    byte_length: length,
                    record_kind: RecordKind::OwnedMedia,
                };
                table = Some(result);
                Ok(reference)
            })
        });
        let reference = match published {
            Ok(reference) => reference,
            Err(error) => {
                if self.with_namespace(|n| Ok(n.is_uncertain()))? {
                    self.inner
                        .ledger
                        .lock()
                        .map_err(|_| BundleError::Capacity("ledger poisoned"))?
                        .pending
                        .push(reservation);
                    self.inner
                        .usage
                        .lock()
                        .map_err(|_| BundleError::Capacity("usage poisoned"))?
                        .unsettled = true;
                }
                return Err(error);
            }
        };
        let existing = self
            .inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?
            .objects
            .get(&reference.sha256)
            .cloned();
        let sealed = if let Some((known, dependencies)) = existing {
            if known != reference || !dependencies.is_empty() {
                return Err(BundleError::Corrupt(
                    "import object identity conflict".into(),
                ));
            }
            drop(reservation);
            SealedObject {
                reference: reference.clone(),
                pin: self.pin(std::slice::from_ref(&reference))?,
            }
        } else {
            let unclassified = self
                .inner
                .ledger
                .lock()
                .map_err(|_| BundleError::Capacity("ledger poisoned"))?
                .unclassified
                .get(&reference.sha256)
                .copied();
            if unclassified.is_some_and(|length| length != reference.byte_length) {
                return Err(BundleError::Corrupt(
                    "unclassified import length differs".into(),
                ));
            }
            self.register_published(&reference, &[], reservation, unclassified)?
        };
        Ok((sealed, table.ok_or(BundleError::HistoryBarrierInvariant)?))
    }

    pub(crate) fn open_input_descriptor(&self, reference: &ObjectRef) -> Result<File, BundleError> {
        self.closure_with_inventory(std::slice::from_ref(reference), false)?;
        let file = self
            .with_namespace(|n| n.open_owned(&Self::object_path(reference)?))?
            .ok_or_else(|| BundleError::Corrupt("missing owned source".into()))?;
        if !file.metadata()?.is_file() || file.metadata()?.len() != reference.byte_length {
            return Err(BundleError::Corrupt("owned source length changed".into()));
        }
        Ok(file)
    }

    pub(crate) fn shared_chunk_table(
        &self,
        reference: &ObjectRef,
    ) -> Result<Arc<crate::owned_media::CertifiedChunkTable>, BundleError> {
        let mut cache = self
            .inner
            .chunk_tables
            .lock()
            .map_err(|_| BundleError::Capacity("table cache poisoned"))?;
        cache.retain(|_, entry| entry.value.strong_count() != 0);
        if let Some(table) = cache.get(reference).and_then(|entry| entry.value.upgrade()) {
            return Ok(table);
        }
        let table =
            self.read_copy_record::<crate::owned_inputs::InputChunkTable>(reference, true)?;
        let _encoded = self.reserve_copy_read(0, reference.byte_length, 0, false, true)?;
        let canonical =
            serde_json::to_vec(&*table).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        if canonical.len() as u64 != reference.byte_length
            || kinewright_media::sha256_bytes(&canonical) != reference.sha256
        {
            return Err(BundleError::Corrupt(
                "noncanonical chunk table object".into(),
            ));
        }
        let charge = self.reserve_copy_read(
            0,
            (table.chunk_sha256.len() as u64)
                .checked_mul(32)
                .ok_or(BundleError::Capacity("table allocation"))?,
            0,
            false,
            true,
        )?;
        let chunks = Arc::from(table.chunk_sha256.as_slice());
        let value = Arc::new(crate::owned_media::CertifiedChunkTable {
            chunks,
            table,
            _copy_charge: charge,
            _pin: self.pin(std::slice::from_ref(reference))?,
        });
        if cache.len() as u64 >= self.limits().objects {
            return Err(BundleError::Capacity("table cache entries"));
        }
        cache.insert(
            reference.clone(),
            WeakCacheEntry {
                value: Arc::downgrade(&value),
                _charge: self.reserve_copy_read(0, 256, 0, false, true)?,
            },
        );
        Ok(value)
    }

    pub(crate) fn shared_resource(
        &self,
        reference: &ObjectRef,
    ) -> Result<Arc<crate::certified_resources::ResourceBytes>, BundleError> {
        if !matches!(
            reference.record_kind,
            RecordKind::OwnedLut | RecordKind::OwnedFont
        ) {
            return Err(BundleError::Unsupported(
                "resource has no certified private-copy reader".into(),
            ));
        }
        if reference.byte_length > self.limits().decoded_record_bytes {
            return Err(BundleError::Capacity("private resource bytes"));
        }
        let mut cache = self
            .inner
            .resources
            .lock()
            .map_err(|_| BundleError::Capacity("resource cache poisoned"))?;
        cache.retain(|_, entry| entry.value.strong_count() != 0);
        if let Some(value) = cache.get(reference).and_then(|entry| entry.value.upgrade()) {
            return Ok(value);
        }
        let charge = self.reserve_copy_read(
            0,
            reference
                .byte_length
                .checked_add(256)
                .ok_or(BundleError::Capacity("resource charge"))?,
            0,
            false,
            true,
        )?;
        let mut file = self.open_input_descriptor(reference)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(
                usize::try_from(reference.byte_length)
                    .map_err(|_| BundleError::Capacity("resource address space"))?,
            )
            .map_err(|_| BundleError::Capacity("resource allocation"))?;
        bytes.resize(
            usize::try_from(reference.byte_length)
                .map_err(|_| BundleError::Capacity("resource address space"))?,
            0,
        );
        file.read_exact(&mut bytes)?;
        let mut extra = [0];
        if file.read(&mut extra)? != 0
            || file.metadata()?.len() != reference.byte_length
            || kinewright_media::sha256_bytes(&bytes) != reference.sha256
        {
            return Err(BundleError::Corrupt(
                "private resource identity changed".into(),
            ));
        }
        let value = Arc::new(crate::certified_resources::ResourceBytes {
            bytes: charge.hold(bytes),
            _pin: self.pin(std::slice::from_ref(reference))?,
        });
        if cache.len() as u64 >= self.limits().objects {
            return Err(BundleError::Capacity("resource cache entries"));
        }
        cache.insert(
            reference.clone(),
            WeakCacheEntry {
                value: Arc::downgrade(&value),
                _charge: self.reserve_copy_read(0, 256, 0, false, true)?,
            },
        );
        Ok(value)
    }

    pub(crate) fn metadata_reserver(&self) -> Arc<kinewright_media::owned_io::CursorReservation> {
        let store = Self {
            inner: Arc::clone(&self.inner),
        };
        Arc::new(move |bytes| {
            store
                .reserve_copy_read(0, bytes, 0, false, true)
                .map(|charge| Arc::new(charge) as kinewright_media::owned_io::LifetimeGuard)
                .map_err(|error| error.to_string())
        })
    }

    pub(crate) fn decoder_reserver(&self) -> Arc<kinewright_media::owned_io::CursorReservation> {
        let store = Self {
            inner: Arc::clone(&self.inner),
        };
        Arc::new(move |bytes| {
            store.check_read_authority().map_err(|e| e.to_string())?;
            if store
                .inner
                .session_closing
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                return Err("originating source is closing".into());
            }
            StorageReservation::reserve_decoder(&store.inner.usage, store.limits(), bytes, 0)
                .map(|charge| Arc::new(charge) as kinewright_media::owned_io::LifetimeGuard)
                .map_err(|error| error.to_string())
        })
    }

    pub(crate) fn media_admission_verified(
        &self,
        entry: &crate::owned_inputs::OwnedInputEntry,
    ) -> Result<bool, BundleError> {
        Ok(self.media_admission_memo(entry)?.is_some())
    }

    pub(crate) fn media_admission_memo(
        &self,
        entry: &crate::owned_inputs::OwnedInputEntry,
    ) -> Result<Option<kinewright_media::owned_io::SourceAdmissionMemo>, BundleError> {
        let memo = self
            .inner
            .media_admissions
            .lock()
            .map_err(|_| BundleError::Capacity("media admission memo poisoned"))?;
        Ok(memo
            .get(&(
                entry.object.clone(),
                entry.chunk_table_ref()?.clone(),
                entry
                    .profile
                    .clone()
                    .ok_or_else(|| BundleError::Corrupt("missing media interpretation".into()))?,
            ))
            .map(|admission| admission.binding.clone()))
    }

    pub(crate) fn remember_media_admission(
        &self,
        entry: &crate::owned_inputs::OwnedInputEntry,
        binding: kinewright_media::owned_io::SourceAdmissionMemo,
    ) -> Result<(), BundleError> {
        let mut memo = self
            .inner
            .media_admissions
            .lock()
            .map_err(|_| BundleError::Capacity("media admission memo poisoned"))?;
        let profile = entry
            .profile
            .as_deref()
            .ok_or_else(|| BundleError::Corrupt("missing media interpretation".into()))?;
        let charge = (profile.len() as u64)
            .checked_add(1024)
            .ok_or(BundleError::Capacity("media admission memo charge"))?;
        let key = (
            entry.object.clone(),
            entry.chunk_table_ref()?.clone(),
            profile.to_owned(),
        );
        if memo.contains_key(&key) {
            return Ok(());
        }
        if self.limits().objects == 0 {
            return Err(BundleError::Capacity("media admission memo entries"));
        }
        if memo.len() as u64 >= self.limits().objects {
            memo.pop_first();
        }
        memo.insert(
            key,
            MediaAdmission {
                binding,
                _charge: self.reserve_copy_read(0, charge, 0, false, true)?,
            },
        );
        Ok(())
    }

    /// Measure the complete committed graph, including retained recovery inventories.
    #[cfg(any(test, feature = "test-util"))]
    #[doc(hidden)]
    pub fn maximum_committed_depth(&self) -> Result<u64, BundleError> {
        fn depth(
            ledger: &Ledger,
            reference: &ObjectRef,
            inventory: bool,
            memo: &mut BTreeMap<(String, bool), u64>,
            active: &mut BTreeSet<String>,
        ) -> Result<u64, BundleError> {
            let inventory = inventory && reference.record_kind != RecordKind::RootCheckpoint;
            let key = (reference.sha256.clone(), inventory);
            if let Some(depth) = memo.get(&key) {
                return Ok(*depth);
            }
            if !active.insert(reference.sha256.clone()) {
                return Err(BundleError::Corrupt("dependency cycle".into()));
            }
            let (_, edges) = ledger
                .objects
                .get(&reference.sha256)
                .ok_or_else(|| BundleError::Corrupt("missing graph node".into()))?;
            let mut maximum = 0;
            for edge in edges.iter().chain(
                ledger
                    .inventory_dependencies
                    .get(&reference.sha256)
                    .filter(|_| inventory)
                    .into_iter()
                    .flatten(),
            ) {
                maximum = maximum.max(1 + depth(ledger, edge, inventory, memo, active)?);
            }
            active.remove(&reference.sha256);
            memo.insert(key, maximum);
            Ok(maximum)
        }
        let ledger = self
            .inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
        let mut memo = BTreeMap::new();
        let mut active = BTreeSet::new();
        let mut maximum = 0;
        for root in &ledger.committed {
            maximum = maximum.max(depth(&ledger, root, true, &mut memo, &mut active)?);
        }
        Ok(maximum)
    }

    pub fn closure(&self, roots: &[ObjectRef]) -> Result<Vec<ObjectRef>, BundleError> {
        self.closure_with_inventory(roots, true)
    }
    pub(crate) fn closure_with_inventory(
        &self,
        roots: &[ObjectRef],
        include_inventory: bool,
    ) -> Result<Vec<ObjectRef>, BundleError> {
        self.closure_at(roots, include_inventory, 0)
    }
    fn closure_at(
        &self,
        roots: &[ObjectRef],
        include_inventory: bool,
        depth: u64,
    ) -> Result<Vec<ObjectRef>, BundleError> {
        let ledger = self
            .inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
        let mut found = BTreeMap::new();
        let mut active = BTreeSet::new();
        let mut visited = BTreeSet::new();
        for root in roots {
            walk(
                root,
                &ledger,
                &self.inner.limits,
                &mut found,
                &mut active,
                &mut visited,
                include_inventory,
                depth,
            )?;
        }
        Ok(found.into_values().collect())
    }
    pub fn pin(&self, roots: &[ObjectRef]) -> Result<StorePin, BundleError> {
        let objects = self.closure(roots)?;
        let mut ledger = self
            .inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
        for object in &objects {
            if ledger.pins.get(&object.sha256).copied().unwrap_or(0) == u64::MAX {
                return Err(BundleError::Capacity("pin overflow"));
            }
        }
        let keys = objects.into_iter().map(|o| o.sha256).collect::<Vec<_>>();
        for key in &keys {
            *ledger.pins.entry(key.clone()).or_default() += 1;
        }
        Ok(StorePin {
            inner: Arc::clone(&self.inner),
            objects: keys,
        })
    }
    pub fn verify(&self, reference: &ObjectRef) -> Result<(), BundleError> {
        self.open_verified(reference).map(drop)
    }
    pub(crate) fn open_verified(&self, reference: &ObjectRef) -> Result<File, BundleError> {
        self.open_copy_source(reference, false)
    }
    fn open_copy_source(
        &self,
        reference: &ObjectRef,
        cleanup_only: bool,
    ) -> Result<File, BundleError> {
        self.inner
            .verified_memo
            .lock()
            .map_err(|_| BundleError::Capacity("memo poisoned"))?
            .forget(reference);
        // Opening archived bytes does not promote their automatic inventory into live roots.
        self.closure_with_inventory(std::slice::from_ref(reference), false)?;
        let _working = self.reserve_copy_read(0, 1 << 20, 0, false, cleanup_only)?;
        let path = Self::object_path(reference)?;
        let mut file = self
            .with_namespace(|namespace| namespace.open_owned(&path))?
            .ok_or_else(|| BundleError::Corrupt("missing owned object".into()))?;
        let mut reader = CountingReader {
            inner: &mut file,
            bytes: 0,
            limit: reference.byte_length,
        };
        let digest = kinewright_media::sha256_reader(&mut reader);
        let bytes = reader.bytes;
        let mut usage = self
            .inner
            .usage
            .lock()
            .map_err(|_| BundleError::Capacity("usage poisoned"))?;
        usage.verified_calls = usage
            .verified_calls
            .checked_add(1)
            .ok_or(BundleError::Capacity("verification count overflow"))?;
        usage.verified_bytes = usage
            .verified_bytes
            .checked_add(bytes)
            .ok_or(BundleError::Capacity("verification byte overflow"))?;
        drop(usage);
        if file.metadata()?.len() != reference.byte_length
            || !digest.as_ref().is_ok_and(|h| h == &reference.sha256)
        {
            self.inner
                .ledger
                .lock()
                .map_err(|_| BundleError::Capacity("ledger poisoned"))?
                .verified
                .remove(&reference.sha256);
            return Err(BundleError::Corrupt("owned identity changed".into()));
        }
        file.seek(SeekFrom::Start(0))?;
        self.inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?
            .verified
            .insert(reference.sha256.clone());
        Ok(file)
    }
    #[must_use]
    pub fn input_owner(&self) -> (Option<ProjectId>, Option<SessionId>) {
        (self.inner.project_id.clone(), self.inner.session_id.clone())
    }
    #[must_use]
    pub fn owner_epoch(&self) -> u64 {
        self.inner.owner_epoch
    }
    /// This handle observes accounting after the final reader/owner retires.
    pub fn usage_handle(&self) -> impl Fn() -> Result<StorageUsage, BundleError> + use<> {
        let usage = Arc::clone(&self.inner.usage);
        move || {
            usage
                .lock()
                .map(|u| u.clone())
                .map_err(|_| BundleError::Capacity("usage poisoned"))
        }
    }
    pub fn collect_unpublished(&mut self) -> Result<u64, BundleError> {
        if self.inner.read_authority.is_some() {
            return Err(BundleError::Identity("read-only snapshot".into()));
        }
        self.inner
            .chunk_tables
            .lock()
            .map_err(|_| BundleError::Capacity("table cache poisoned"))?
            .retain(|_, entry| entry.value.strong_count() != 0);
        self.inner
            .resources
            .lock()
            .map_err(|_| BundleError::Capacity("resource cache poisoned"))?
            .retain(|_, entry| entry.value.strong_count() != 0);
        let collected = collect(&self.inner)?;
        let ledger = self
            .inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
        self.inner
            .media_admissions
            .lock()
            .map_err(|_| BundleError::Capacity("media admission memo poisoned"))?
            .retain(|(source, table, _), _| {
                ledger
                    .objects
                    .get(&source.sha256)
                    .is_some_and(|(reference, _)| reference == source)
                    && ledger
                        .objects
                        .get(&table.sha256)
                        .is_some_and(|(reference, _)| reference == table)
            });
        Ok(collected)
    }
    pub fn manifest(&self) -> Result<StoreManifest, BundleError> {
        let ledger = self
            .inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
        Ok(StoreManifest {
            project_id: self.inner.project_id.clone(),
            session_id: self.inner.session_id.clone(),
            generation: 0,
            objects: ledger.objects.values().map(|(r, _)| r.clone()).collect(),
            dependencies: ledger
                .objects
                .iter()
                .map(|(h, (_, d))| (h.clone(), d.clone()))
                .collect(),
            inventory_dependencies: ledger.inventory_dependencies.clone(),
            durable_pins: ledger.committed.clone(),
            recovery: RecoveryInventory::default(),
            required_fields: BTreeSet::new(),
            required_apis: BTreeMap::new(),
            evidence_roles: BTreeMap::new(),
            required_kinds: ledger
                .objects
                .values()
                .map(|(r, _)| r.record_kind)
                .collect(),
            mirrors: ledger.mirrors.clone(),
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn walk(
    reference: &ObjectRef,
    ledger: &Ledger,
    limits: &StorageLimits,
    found: &mut BTreeMap<String, ObjectRef>,
    active: &mut BTreeSet<String>,
    visited: &mut BTreeSet<(String, bool)>,
    include_inventory: bool,
    depth: u64,
) -> Result<(), BundleError> {
    reference.validate()?;
    if depth > limits.graph_depth {
        return Err(BundleError::Capacity("graph depth"));
    }
    if active.contains(&reference.sha256) {
        return Err(BundleError::Corrupt("dependency cycle".into()));
    }
    if found
        .get(&reference.sha256)
        .is_some_and(|known| known != reference)
    {
        return Err(BundleError::Corrupt(
            "inconsistent reference identity".into(),
        ));
    }
    let include_inventory =
        include_inventory && reference.record_kind != RecordKind::RootCheckpoint;
    if !visited.insert((reference.sha256.clone(), include_inventory)) {
        return Ok(());
    }
    let (known, dependencies) = ledger.objects.get(&reference.sha256).ok_or_else(|| {
        BundleError::Corrupt(format!("missing dependency declaration: {reference:?}"))
    })?;
    if known != reference
        || (!ledger.authenticated_read_graph && !ledger.verified.contains(&reference.sha256))
    {
        return Err(BundleError::Corrupt(
            "unverified dependency identity".into(),
        ));
    }
    if found.len() as u64 >= limits.objects {
        return Err(BundleError::Capacity("closure object count"));
    }
    active.insert(reference.sha256.clone());
    for dependency in dependencies {
        walk(
            dependency,
            ledger,
            limits,
            found,
            active,
            visited,
            include_inventory,
            depth + 1,
        )?;
    }
    if include_inventory
        && let Some(dependencies) = ledger.inventory_dependencies.get(&reference.sha256)
    {
        for dependency in dependencies {
            walk(
                dependency,
                ledger,
                limits,
                found,
                active,
                visited,
                true,
                depth + 1,
            )?;
        }
    }
    active.remove(&reference.sha256);
    found.insert(reference.sha256.clone(), reference.clone());
    Ok(())
}

struct CountingReader<'a, R> {
    inner: &'a mut R,
    bytes: u64,
    limit: u64,
}
impl<R: Read> Read for CountingReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = self.inner.read(buffer)?;
        self.bytes = self
            .bytes
            .checked_add(count as u64)
            .ok_or_else(|| std::io::Error::other("read counter overflow"))?;
        if self.bytes > self.limit {
            return Err(std::io::Error::other("owned bytes exceed reservation"));
        }
        Ok(count)
    }
}
impl StorePin {
    /// Pins may be shared only within the same IO owner's ledger.
    #[must_use]
    pub fn shares_owner_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}
impl Drop for StorePin {
    fn drop(&mut self) {
        if let Ok(mut ledger) = self.inner.ledger.lock() {
            for key in &self.objects {
                match ledger.pins.get_mut(key) {
                    Some(count) if *count > 0 => *count -= 1,
                    _ => {
                        if let Ok(mut usage) = self.inner.usage.lock() {
                            usage.damaged_accounting = true;
                        }
                    }
                }
            }
        }
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "one authority retires known objects and owned temporary charges after durable publication"
)]
fn collect(inner: &StoreInner) -> Result<u64, BundleError> {
    inner
        .namespace
        .lock()
        .map_err(|_| BundleError::Identity("namespace poisoned".into()))?
        .begin_object_batch()?;
    let result = collect_objects(inner);
    let barrier = inner
        .namespace
        .lock()
        .map_err(|_| BundleError::Identity("namespace poisoned".into()))?
        .finish_object_batch();
    barrier?;
    result
}

#[allow(clippy::too_many_lines)]
fn collect_objects(inner: &StoreInner) -> Result<u64, BundleError> {
    let previous = inner
        .ledger
        .lock()
        .map_err(|_| BundleError::Capacity("ledger poisoned"))?
        .collection
        .clone();
    let mut collection = CollectionStatus::default();
    let mut retire = |path: &Path,
                      length: u64,
                      run: &mut dyn FnMut() -> Result<(), BundleError>|
     -> Result<bool, BundleError> {
        #[cfg(any(test, feature = "test-util"))]
        let result = crate::io_observer::check_failure("gc_before_unlink", path)
            .map_err(BundleError::from)
            .and_then(|()| run());
        #[cfg(not(any(test, feature = "test-util")))]
        let result = run();
        match result {
            Ok(()) => Ok(true),
            Err(error) => {
                // An ordinary file refusal must not prevent unrelated retirement. Uncertain
                // namespace operations still stop collection and fence publication.
                if matches!(
                    error,
                    BundleError::Unsettled
                        | BundleError::DurabilityUnknown
                        | BundleError::Identity(_)
                ) {
                    return Err(error);
                }
                collection.failed_files += 1;
                collection.pending_bytes = collection
                    .pending_bytes
                    .checked_add(length)
                    .ok_or(BundleError::Capacity("failed collection bytes"))?;
                if collection.failures.len() < 32 {
                    let attempts = previous
                        .failures
                        .iter()
                        .find(|f| f.path == path)
                        .map_or(1, |f| f.attempts.saturating_add(1));
                    collection.failures.push(CollectionFailure {
                        path: path.to_path_buf(),
                        attempts,
                        error: error.to_string(),
                    });
                }
                Ok(false)
            }
        }
    };
    let victims = {
        let ledger = inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
        if !ledger.can_collect {
            return Err(BundleError::Unsettled);
        }
        let mut durable = BTreeMap::new();
        let mut active = BTreeSet::new();
        let mut visited = BTreeSet::new();
        for root in &ledger.committed {
            walk(
                root,
                &ledger,
                &inner.limits,
                &mut durable,
                &mut active,
                &mut visited,
                true,
                0,
            )?;
        }
        ledger
            .objects
            .iter()
            .filter(|(h, _)| {
                ledger.pins.get(*h).copied().unwrap_or(0) == 0
                    && !durable.contains_key(*h)
                    && (ledger.committed.is_empty() || ledger.acknowledged_objects.contains(*h))
            })
            .map(|(h, (r, d))| {
                (
                    h.clone(),
                    r.clone(),
                    d.len() as u64,
                    *ledger.metadata_sizes.get(h).unwrap_or(&0),
                )
            })
            .collect::<Vec<_>>()
    };
    let mut retired = 0_u64;
    for (hash, reference, edges, metadata) in victims {
        let path = ProjectStore::object_path(&reference)?;
        if !retire(path.as_path(), reference.byte_length, &mut || {
            inner
                .namespace
                .lock()
                .map_err(|_| BundleError::Identity("namespace poisoned".into()))?
                .unlink_owned(&path)
        })? {
            continue;
        }
        let mut ledger = inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
        drop(ledger.support_fields.remove(&hash));
        let mut usage = inner
            .usage
            .lock()
            .map_err(|_| BundleError::Capacity("usage poisoned"))?;
        let Some(physical) = usage.physical_bytes.checked_sub(reference.byte_length) else {
            usage.damaged_accounting = true;
            return Err(BundleError::Capacity("retirement underflow"));
        };
        let Some(next_metadata) = usage.metadata_bytes.checked_sub(metadata) else {
            usage.damaged_accounting = true;
            return Err(BundleError::Capacity("metadata retirement underflow"));
        };
        let Some(next_edges) = usage.edges.checked_sub(edges) else {
            usage.damaged_accounting = true;
            return Err(BundleError::Capacity("edge retirement underflow"));
        };
        let Some(objects) = usage.objects.checked_sub(1) else {
            usage.damaged_accounting = true;
            return Err(BundleError::Capacity("object retirement underflow"));
        };
        if reference.record_kind.is_owned_bytes() {
            usage.owned_bytes = usage
                .owned_bytes
                .checked_sub(reference.byte_length)
                .ok_or(BundleError::Capacity("owned retirement underflow"))?;
        }
        usage.physical_bytes = physical;
        usage.metadata_bytes = next_metadata;
        usage.edges = next_edges;
        usage.objects = objects;
        ledger.objects.remove(&hash);
        ledger.pins.remove(&hash);
        ledger.verified.remove(&hash);
        inner
            .verified_memo
            .lock()
            .map_err(|_| BundleError::Capacity("memo poisoned"))?
            .forget(&reference);

        ledger.metadata_sizes.remove(&hash);
        ledger.inventory_dependencies.remove(&hash);
        retired = retired
            .checked_add(reference.byte_length)
            .ok_or(BundleError::Capacity("retirement overflow"))?;
        #[cfg(any(test, feature = "test-util"))]
        crate::io_observer::check_failure("gc_after_unlink", path.as_path())?;
    }
    let temporaries = inner
        .ledger
        .lock()
        .map_err(|_| BundleError::Capacity("ledger poisoned"))?
        .temporaries
        .clone();
    for file in temporaries {
        if !retire(file.relative.as_path(), file.length, &mut || {
            inner
                .namespace
                .lock()
                .map_err(|_| BundleError::Identity("namespace poisoned".into()))?
                .retire_temporary(&file)
        })? {
            continue;
        }
        let mut ledger = inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
        ledger
            .temporaries
            .retain(|f| f.relative != file.relative || f.root_parent != file.root_parent);
        let mut usage = inner
            .usage
            .lock()
            .map_err(|_| BundleError::Capacity("usage poisoned"))?;
        usage.physical_bytes = usage
            .physical_bytes
            .checked_sub(file.length)
            .ok_or(BundleError::Capacity("temporary physical underflow"))?;
        usage.owned_bytes = usage
            .owned_bytes
            .checked_sub(file.length)
            .ok_or(BundleError::Capacity("temporary owned underflow"))?;
        usage.metadata_bytes = usage
            .metadata_bytes
            .checked_sub(
                file.length
                    .checked_add(256)
                    .ok_or(BundleError::Capacity("temporary metadata overflow"))?,
            )
            .ok_or(BundleError::Capacity("temporary metadata underflow"))?;
        usage.unclassified_bytes = usage
            .unclassified_bytes
            .checked_sub(file.length)
            .ok_or(BundleError::Capacity("temporary unclassified underflow"))?;
        usage.objects = usage
            .objects
            .checked_sub(1)
            .ok_or(BundleError::Capacity("temporary count underflow"))?;
        retired = retired
            .checked_add(file.length)
            .ok_or(BundleError::Capacity("temporary retirement overflow"))?;
    }
    // The claim and verified committed closure exclude these generated names from every retained root.
    let orphans = inner
        .ledger
        .lock()
        .map_err(|_| BundleError::Capacity("ledger poisoned"))?
        .unclassified
        .clone();
    for (hash, length) in orphans {
        let mut ledger = inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
        if ledger.objects.contains_key(&hash) || ledger.pins.get(&hash).copied().unwrap_or(0) != 0 {
            continue;
        }
        let path = PortablePath::new(&Path::new("studio/objects").join(&hash))?;
        if !retire(path.as_path(), length, &mut || {
            inner
                .namespace
                .lock()
                .map_err(|_| BundleError::Identity("namespace poisoned".into()))?
                .unlink_owned(&path)
        })? {
            continue;
        }
        let mut usage = inner
            .usage
            .lock()
            .map_err(|_| BundleError::Capacity("usage poisoned"))?;
        usage.physical_bytes = usage
            .physical_bytes
            .checked_sub(length)
            .ok_or(BundleError::Capacity("orphan physical underflow"))?;
        usage.owned_bytes = usage
            .owned_bytes
            .checked_sub(length)
            .ok_or(BundleError::Capacity("orphan owned underflow"))?;
        usage.metadata_bytes = usage
            .metadata_bytes
            .checked_sub(length + 256)
            .ok_or(BundleError::Capacity("orphan metadata underflow"))?;
        usage.unclassified_bytes = usage
            .unclassified_bytes
            .checked_sub(length)
            .ok_or(BundleError::Capacity("orphan bytes underflow"))?;
        usage.objects = usage
            .objects
            .checked_sub(1)
            .ok_or(BundleError::Capacity("orphan object underflow"))?;
        ledger.unclassified.remove(&hash);
        retired = retired
            .checked_add(length)
            .ok_or(BundleError::Capacity("orphan retirement overflow"))?;
    }
    inner
        .ledger
        .lock()
        .map_err(|_| BundleError::Capacity("ledger poisoned"))?
        .collection = collection.clone();
    if let Some(failure) = collection.failures.first() {
        return Err(std::io::Error::other(failure.error.clone()).into());
    }
    Ok(retired)
}

impl Drop for StoreInner {
    fn drop(&mut self) {
        if self.read_authority.is_some() {
            return;
        }
        if self
            .session_closed
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let temporary = self.session_id.is_some();
        let mut collectable = false;
        if let Ok(ledger) = self.ledger.get_mut() {
            if !ledger.pending.is_empty() || !ledger.pending_controls.is_empty() {
                for reservation in ledger
                    .pending
                    .drain(..)
                    .chain(ledger.pending_controls.drain(..))
                {
                    reservation.retain_charge();
                }
                if let Ok(mut usage) = self.usage.lock() {
                    usage.unsettled = true;
                }
            }
            if temporary && ledger.copy_holds == 0 {
                ledger.can_collect = true;
                ledger.committed.clear();
            }
            collectable = ledger.can_collect && ledger.copy_holds == 0;
        }
        // No StorePin exists once this final Arc is dropped. Named committed roots persist.
        if temporary
            && !collectable
            && let Ok(mut usage) = self.usage.lock()
        {
            usage.unsettled = true;
        }
        let retired = if collectable { collect(self) } else { Ok(0) };
        let closed = self
            .namespace
            .get_mut()
            .map_err(|_| BundleError::Capacity("namespace poisoned"))
            .and_then(|namespace| {
                if temporary && collectable {
                    namespace.cleanup_session()
                } else {
                    namespace.release()
                }
            });
        if (retired.is_err() || closed.is_err())
            && let Ok(mut usage) = self.usage.lock()
        {
            usage.unsettled = true;
        }
    }
}

pub(crate) mod record_sealed {
    pub trait Sealed {}
}
/// Only production record types can use the typed-record ingress.
pub trait BundleRecord: record_sealed::Sealed + Serialize + serde::de::DeserializeOwned {
    const KIND: RecordKind;
    fn dependencies(&self) -> Vec<ObjectRef> {
        vec![]
    }
    fn inventory_dependencies(&self) -> Vec<ObjectRef> {
        vec![]
    }
    fn validate(&self, _limits: &StorageLimits) -> Result<(), BundleError> {
        Ok(())
    }
    fn validate_store(&self, _store: &ProjectStore) -> Result<(), BundleError> {
        Ok(())
    }
    fn validate_admission(&self, store: &ProjectStore) -> Result<(), BundleError> {
        self.validate_store(store)
    }
}
macro_rules! record {
    ($ty:ty, $kind:ident) => {
        impl record_sealed::Sealed for $ty {}
        impl BundleRecord for $ty {
            const KIND: RecordKind = RecordKind::$kind;
        }
    };
}
impl record_sealed::Sealed for kinewright_core::Document {}
impl BundleRecord for kinewright_core::Document {
    const KIND: RecordKind = RecordKind::Document;
    fn validate(&self, _: &StorageLimits) -> Result<(), BundleError> {
        self.validate()
            .map_err(|e| BundleError::Corrupt(e.to_string()))
    }
}
impl record_sealed::Sealed for kinewright_core::HistorySnapshot {}
impl BundleRecord for kinewright_core::HistorySnapshot {
    const KIND: RecordKind = RecordKind::History;
    fn validate(&self, limits: &StorageLimits) -> Result<(), BundleError> {
        let bytes = self.encoded_bytes().map_err(|e| {
            BundleError::Capacity(match e {
                kinewright_core::HistoryError::Capacity => "history capacity",
                _ => "invalid history",
            })
        })?;
        if bytes > limits.branch_history_bytes {
            return Err(BundleError::Capacity("branch history"));
        }
        Ok(())
    }
}
impl record_sealed::Sealed for crate::owned_inputs::OwnedInputManifest {}
impl BundleRecord for crate::owned_inputs::OwnedInputManifest {
    const KIND: RecordKind = RecordKind::InputManifest;
    fn dependencies(&self) -> Vec<ObjectRef> {
        self.entries
            .iter()
            .flat_map(|e| std::iter::once(e.object.clone()).chain(e.dependencies.clone()))
            .collect()
    }
    fn validate(&self, limits: &StorageLimits) -> Result<(), BundleError> {
        let mut ids = BTreeSet::new();
        if self.entries.len() as u64 > limits.objects {
            return Err(BundleError::Capacity("input entries"));
        }
        for entry in &self.entries {
            if entry.entry_id.is_empty()
                || entry.entry_id.len() > 256
                || !ids.insert(&entry.entry_id)
                || !entry.object.record_kind.is_blob()
            {
                return Err(BundleError::Corrupt("invalid input entry".into()));
            }
            entry.object.validate()?;
            if entry.object.record_kind == RecordKind::OwnedMedia {
                entry.media_interpretation()?;
                entry.chunk_table_ref()?;
            }
            for dep in &entry.dependencies {
                dep.validate()?;
            }
        }
        Ok(())
    }
    fn validate_store(&self, store: &ProjectStore) -> Result<(), BundleError> {
        for entry in &self.entries {
            if entry.object.record_kind == RecordKind::OwnedMedia {
                let table = store.shared_chunk_table(entry.chunk_table_ref()?)?;
                table
                    .table
                    .validate_binding(&entry.object, store.limits())?;
            }
        }
        Ok(())
    }
    fn validate_admission(&self, store: &ProjectStore) -> Result<(), BundleError> {
        self.validate_store(store)?;
        for entry in &self.entries {
            if entry.object.record_kind == RecordKind::OwnedMedia
                && !store.media_admission_verified(entry)?
            {
                store.certify_media_entry(entry)?;
            }
        }
        Ok(())
    }
}
impl record_sealed::Sealed for crate::bundle_records::BranchCheckpoint {}
impl BundleRecord for crate::bundle_records::BranchCheckpoint {
    const KIND: RecordKind = RecordKind::BranchCheckpoint;
    fn dependencies(&self) -> Vec<ObjectRef> {
        let mut refs = vec![
            self.document_ref.clone(),
            self.baseline_ref.clone(),
            self.history_ref.clone(),
            self.lineage_ref.clone(),
            self.input_manifest_ref.clone(),
        ];
        refs.extend(self.history_input_manifest_refs.clone());
        refs.extend(self.take_map_revision_ref.clone());
        refs
    }
}
impl record_sealed::Sealed for BTreeMap<crate::bundle_records::BranchId, ObjectRef> {}
impl BundleRecord for BTreeMap<crate::bundle_records::BranchId, ObjectRef> {
    const KIND: RecordKind = RecordKind::BranchIndex;
    fn dependencies(&self) -> Vec<ObjectRef> {
        self.values().cloned().collect()
    }
    fn validate(&self, limits: &StorageLimits) -> Result<(), BundleError> {
        if self.len() as u64 > limits.objects {
            return Err(BundleError::Capacity("branch index"));
        }
        if self
            .values()
            .any(|r| r.record_kind != RecordKind::BranchCheckpoint)
        {
            return Err(BundleError::Corrupt("branch index kind".into()));
        }
        Ok(())
    }
}
impl record_sealed::Sealed for crate::bundle_records::SpecialistBaseRecord {}
impl BundleRecord for crate::bundle_records::SpecialistBaseRecord {
    const KIND: RecordKind = RecordKind::SpecialistBase;
    fn dependencies(&self) -> Vec<ObjectRef> {
        vec![
            self.original_baseline_ref.clone(),
            self.frozen_checkpoint_ref.clone(),
        ]
    }
    fn validate(&self, limits: &StorageLimits) -> Result<(), BundleError> {
        if self.base_document_hash.len() != 64
            || self.full_applied_delta.len() as u64 > kinewright_core::HISTORY_COMMANDS as u64
        {
            return Err(BundleError::Capacity("specialist delta"));
        }
        let mut counter = BoundedCounter {
            bytes: 0,
            limit: limits.branch_history_bytes,
        };
        serde_json::to_writer(&mut counter, self)
            .map_err(|_| BundleError::Capacity("specialist delta bytes"))?;
        Ok(())
    }
}
impl record_sealed::Sealed for crate::bundle_records::HistoryFloorCheckpoint {}
impl BundleRecord for crate::bundle_records::HistoryFloorCheckpoint {
    const KIND: RecordKind = RecordKind::HistoryFloor;
    fn dependencies(&self) -> Vec<ObjectRef> {
        vec![
            self.old_baseline_ref.clone(),
            self.new_baseline_ref.clone(),
            self.checkpoint_ref.clone(),
        ]
    }
    fn validate(&self, _: &StorageLimits) -> Result<(), BundleError> {
        if self.preceding_floor_generation.checked_add(1) != Some(self.floor_generation)
            || self.equivalence_digest.len() != 64
        {
            return Err(BundleError::Corrupt("floor generation/equivalence".into()));
        }
        Ok(())
    }
}
impl record_sealed::Sealed for crate::bundle_records::RecoveryCloseCheckpoint {}
impl BundleRecord for crate::bundle_records::RecoveryCloseCheckpoint {
    const KIND: RecordKind = RecordKind::RecoveryClose;
    fn dependencies(&self) -> Vec<ObjectRef> {
        vec![
            self.preceding_checkpoint_ref.clone(),
            self.closed_checkpoint_ref.clone(),
        ]
    }
    fn validate(&self, _: &StorageLimits) -> Result<(), BundleError> {
        if self.suffix_digest.len() != 64 {
            return Err(BundleError::Corrupt("recovery suffix digest".into()));
        }
        Ok(())
    }
}
record!(crate::bundle_records::LineageRecord, Lineage);
impl record_sealed::Sealed for StoreManifest {}
impl BundleRecord for StoreManifest {
    const KIND: RecordKind = RecordKind::StoreManifest;
    fn dependencies(&self) -> Vec<ObjectRef> {
        self.durable_pins.clone()
    }
    fn inventory_dependencies(&self) -> Vec<ObjectRef> {
        self.recovery.references()
    }
    fn validate(&self, limits: &StorageLimits) -> Result<(), BundleError> {
        self.recovery.validate()?;
        if !self.mirrors.is_empty() {
            return Err(BundleError::Unsupported(
                "inactive physical mirror producer".into(),
            ));
        }
        if self.objects.len() as u64 >= limits.objects {
            return Err(BundleError::Capacity("manifest object count"));
        }
        Ok(())
    }
}

impl ProjectStore {
    /// Uses typed admission for projected-manifest oracle fixtures.
    #[cfg(feature = "test-util")]
    pub fn admit_manifest_probe(
        &mut self,
        manifest: &StoreManifest,
    ) -> Result<SealedObject, BundleError> {
        self.admit_record(manifest)
    }

    pub fn admit_document(
        &mut self,
        document: &kinewright_core::Document,
    ) -> Result<SealedObject, BundleError> {
        self.admit_record(document)
    }
    pub fn admit_history(
        &mut self,
        history: &kinewright_core::HistorySnapshot,
    ) -> Result<SealedObject, BundleError> {
        self.admit_record(history)
    }
    pub(crate) fn admit_record<T: BundleRecord>(
        &mut self,
        record: &T,
    ) -> Result<SealedObject, BundleError> {
        record.validate(self.limits())?;
        record.validate_admission(self)?;
        if !T::KIND.supported() {
            return Err(BundleError::Unsupported("inactive record kind".into()));
        }
        let limit = if T::KIND == RecordKind::History {
            self.limits().branch_history_bytes
        } else {
            self.limits().decoded_record_bytes
        };
        let mut counter = BoundedCounter { bytes: 0, limit };
        serde_json::to_writer(&mut counter, record)
            .map_err(|_| BundleError::Capacity("encoded record"))?;
        let _encoded = self.reserve(0, counter.bytes, 0, false)?;
        let capacity =
            usize::try_from(counter.bytes).map_err(|_| BundleError::Capacity("record size"))?;
        let mut bytes = Vec::with_capacity(capacity);
        serde_json::to_writer(&mut bytes, record)
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
        if bytes.len() != capacity {
            return Err(BundleError::Corrupt("unstable record serialization".into()));
        }
        let preflight = self.reserve(0, self.limits().decoded_record_bytes, 0, false)?;
        let decoded = preflight_record(&bytes, self.limits())?;
        drop(preflight);
        if T::KIND == RecordKind::History && decoded > self.limits().branch_history_bytes {
            return Err(BundleError::Capacity("live branch history"));
        }
        let _decoded = self.reserve(
            0,
            decoded,
            if T::KIND == RecordKind::History {
                decoded
            } else {
                0
            },
            false,
        )?;
        let reference = ObjectRef {
            sha256: kinewright_media::sha256_bytes(&bytes),
            byte_length: counter.bytes,
            record_kind: T::KIND,
        };
        let inventory_dependencies = record.inventory_dependencies();
        self.closure(&inventory_dependencies)?;
        let mut sealed =
            self.admit_recovered(&mut bytes.as_slice(), &reference, &record.dependencies())?;
        if !inventory_dependencies.is_empty() {
            self.inner
                .ledger
                .lock()
                .map_err(|_| BundleError::Capacity("ledger poisoned"))?
                .inventory_dependencies
                .insert(reference.sha256, inventory_dependencies);
        }
        sealed.pin = self.pin(std::slice::from_ref(&sealed.reference))?;
        self.inner
            .verified_memo
            .lock()
            .map_err(|_| BundleError::Capacity("memo poisoned"))?
            .remember(&sealed.reference, record, decoded, None, self.limits());
        Ok(sealed)
    }
    pub fn read_record<T: BundleRecord>(
        &self,
        reference: &ObjectRef,
    ) -> Result<StorageReservation<T>, BundleError> {
        self.read_copy_record(reference, false)
    }
    pub(crate) fn read_copy_record<T: BundleRecord>(
        &self,
        reference: &ObjectRef,
        cleanup_only: bool,
    ) -> Result<StorageReservation<T>, BundleError> {
        #[cfg(any(test, feature = "test-util"))]
        if T::KIND == RecordKind::History {
            crate::io_observer::note_history_decode();
        }
        if reference.record_kind != T::KIND {
            return Err(BundleError::Corrupt("typed record kind mismatch".into()));
        }
        let limit = if T::KIND == RecordKind::History {
            self.limits().branch_history_bytes
        } else {
            self.limits().decoded_record_bytes
        };
        if reference.byte_length > limit {
            return Err(BundleError::Capacity("encoded record"));
        }
        let _encoded = self.reserve_copy_read(0, reference.byte_length, 0, false, cleanup_only)?;
        let mut file = self.open_copy_source(reference, cleanup_only)?;
        let bytes = crate::bundle_recovery::bounded_bytes(&mut file, reference.byte_length)?;
        if bytes.len() as u64 != reference.byte_length
            || kinewright_media::sha256_bytes(&bytes) != reference.sha256
        {
            return Err(BundleError::Corrupt("record changed while decoding".into()));
        }
        let preflight = self.reserve_copy_read(
            0,
            self.limits().decoded_record_bytes,
            0,
            false,
            cleanup_only,
        )?;
        let decoded = preflight_record(&bytes, self.limits())?;
        drop(preflight);
        if T::KIND == RecordKind::History && decoded > self.limits().branch_history_bytes {
            return Err(BundleError::Capacity("live branch history"));
        }
        let decoded_reservation = self.reserve_copy_read(
            0,
            decoded,
            if T::KIND == RecordKind::History {
                decoded
            } else {
                0
            },
            false,
            cleanup_only,
        )?;
        let value: T =
            serde_json::from_slice(&bytes).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        value.validate(self.limits())?;
        value.validate_store(self)?;
        self.inner
            .verified_memo
            .lock()
            .map_err(|_| BundleError::Capacity("memo poisoned"))?
            .remember(reference, &value, decoded, None, self.limits());
        self.check_read_authority()?;
        Ok(decoded_reservation.hold(value))
    }
}
pub(crate) struct BoundedCounter {
    pub(crate) bytes: u64,
    pub(crate) limit: u64,
}
impl Write for BoundedCounter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len() as u64)
            .filter(|n| *n <= self.limit)
            .ok_or_else(|| std::io::Error::other("encoded record cap"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Admission bound, not a measured heap/RSS value. Runs before typed allocations.
#[allow(clippy::too_many_lines)]
pub(crate) fn preflight_record(bytes: &[u8], limits: &StorageLimits) -> Result<u64, BundleError> {
    use serde::de::{DeserializeSeed, Error, MapAccess, SeqAccess, Visitor};
    use std::{cell::Cell, fmt};
    struct Scan<'a> {
        charge: &'a Cell<u64>,
        limits: &'a StorageLimits,
        depth: u64,
    }
    impl Scan<'_> {
        fn charge<E: Error>(&self, bytes: u64) -> Result<(), E> {
            let next = self
                .charge
                .get()
                .checked_add(bytes)
                .filter(|n| *n <= self.limits.decoded_record_bytes)
                .ok_or_else(|| E::custom("decoded record cap"))?;
            self.charge.set(next);
            Ok(())
        }
        fn child(&self) -> Self {
            Self {
                charge: self.charge,
                limits: self.limits,
                depth: self.depth + 1,
            }
        }
    }
    impl<'de> DeserializeSeed<'de> for Scan<'_> {
        type Value = ();
        fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
            if self.depth > self.limits.graph_depth.saturating_mul(4) {
                return Err(D::Error::custom("JSON depth cap"));
            }
            // Format-defined slots, independent of Rust enum layout and target ABI.
            self.charge(64)?;
            d.deserialize_any(self)
        }
    }
    impl<'de> Visitor<'de> for Scan<'_> {
        type Value = ();
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("bounded record JSON")
        }
        fn visit_bool<E: Error>(self, _: bool) -> Result<(), E> {
            Ok(())
        }
        fn visit_i64<E: Error>(self, _: i64) -> Result<(), E> {
            Ok(())
        }
        fn visit_u64<E: Error>(self, _: u64) -> Result<(), E> {
            Ok(())
        }
        fn visit_f64<E: Error>(self, _: f64) -> Result<(), E> {
            Ok(())
        }
        fn visit_unit<E: Error>(self) -> Result<(), E> {
            Ok(())
        }
        fn visit_none<E: Error>(self) -> Result<(), E> {
            Ok(())
        }
        fn visit_str<E: Error>(self, value: &str) -> Result<(), E> {
            self.charge(value.len() as u64)
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
            while seq.next_element_seed(self.child())?.is_some() {}
            Ok(())
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
            let mut keys = BTreeSet::new();
            while let Some(key) = map.next_key::<String>()? {
                self.charge(
                    (key.len() as u64)
                        .checked_add(128)
                        .ok_or_else(|| A::Error::custom("key cap"))?,
                )?;
                if !keys.insert(key) {
                    return Err(A::Error::custom("duplicate record key"));
                }
                map.next_value_seed(self.child())?;
            }
            Ok(())
        }
    }
    if bytes.len() as u64 > limits.decoded_record_bytes {
        return Err(BundleError::Capacity("encoded record cap"));
    }
    // serde_json's unescape scratch may grow geometrically; reserve it before typed parsing.
    let scratch = (bytes.len() as u64)
        .checked_mul(2)
        .ok_or(BundleError::Capacity("parser scratch overflow"))?;
    if scratch > limits.decoded_record_bytes {
        return Err(BundleError::Capacity("parser scratch"));
    }
    let charge = Cell::new(scratch);
    let mut d = serde_json::Deserializer::from_slice(bytes);
    Scan {
        charge: &charge,
        limits,
        depth: 0,
    }
    .deserialize(&mut d)
    .map_err(|e| {
        if e.to_string().contains("cap") {
            BundleError::Capacity("decoded record or JSON depth")
        } else {
            BundleError::Corrupt(e.to_string())
        }
    })?;
    d.end().map_err(|e| BundleError::Corrupt(e.to_string()))?;
    Ok(charge.get())
}

impl ProjectStore {
    pub(crate) fn verify_in_namespace(
        namespace: &ProjectNamespace,
        reference: &ObjectRef,
        usage: &Arc<Mutex<StorageUsage>>,
        limits: &StorageLimits,
    ) -> Result<File, BundleError> {
        reference.validate()?;
        let _working = StorageReservation::reserve(usage, limits, 0, 1 << 20, 0, false, u64::MAX)?;
        let mut file = namespace
            .open_owned(&Self::object_path(reference)?)?
            .ok_or_else(|| BundleError::Corrupt("missing owned object".into()))?;
        if file.metadata()?.len() != reference.byte_length {
            return Err(BundleError::Corrupt("owned byte length changed".into()));
        }
        let mut counted = CountingReader {
            inner: &mut file,
            bytes: 0,
            limit: reference.byte_length,
        };
        let digest = kinewright_media::sha256_reader(&mut counted);
        let measured = counted.bytes;
        let mut state = usage
            .lock()
            .map_err(|_| BundleError::Capacity("usage poisoned"))?;
        state.verified_calls = state
            .verified_calls
            .checked_add(1)
            .ok_or(BundleError::Capacity("verification calls"))?;
        state.verified_bytes = state
            .verified_bytes
            .checked_add(measured)
            .ok_or(BundleError::Capacity("verification bytes"))?;
        drop(state);
        if measured != reference.byte_length
            || !digest.as_ref().is_ok_and(|h| h == &reference.sha256)
        {
            return Err(BundleError::Corrupt("owned identity changed".into()));
        }
        file.seek(SeekFrom::Start(0))?;
        Ok(file)
    }
    #[allow(
        clippy::too_many_lines,
        reason = "adopt a verified graph and its physical charges together"
    )]
    pub(crate) fn from_verified(
        namespace: ProjectNamespace,
        project_id: ProjectId,
        manifest: &StoreManifest,
        manifest_ref: &ObjectRef,
        verified: &BTreeSet<String>,
        usage: Arc<Mutex<StorageUsage>>,
        limits: StorageLimits,
    ) -> Result<Self, BundleError> {
        let mut retained = manifest.clone();
        let mut verified = verified.clone();
        crate::bundle_recovery::retain_inventory_graph(
            &namespace,
            &mut retained,
            &mut verified,
            &usage,
            &limits,
        )?;
        let manifest = &retained;
        if manifest.project_id.as_ref() != Some(&project_id) || manifest.session_id.is_some() {
            return Err(BundleError::Identity(
                "store owner differs from root".into(),
            ));
        }
        let mut store = Self::new(namespace, Some(project_id), None, limits)?;
        let inner = Arc::get_mut(&mut store.inner)
            .ok_or_else(|| BundleError::Identity("store already shared".into()))?;
        inner.usage = usage;
        let ledger = inner
            .ledger
            .get_mut()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
        let mut references = manifest.objects.clone();
        references.push(manifest_ref.clone());
        if references.len() as u64 > inner.limits.objects {
            return Err(BundleError::Capacity("restored object count"));
        }
        let controls = *inner
            .root_bytes
            .get_mut()
            .map_err(|_| BundleError::Capacity("root accounting"))?
            + *inner
                .inventory_bytes
                .get_mut()
                .map_err(|_| BundleError::Capacity("inventory accounting"))?;
        let mut physical = controls;
        let mut owned = 0_u64;
        let mut metadata = controls;
        let mut edges = 0_u64;
        for reference in references {
            reference.validate()?;
            if !verified.contains(&reference.sha256)
                || ledger.objects.contains_key(&reference.sha256)
            {
                return Err(BundleError::Corrupt(
                    "duplicate or unverified declaration".into(),
                ));
            }
            let dependencies = if reference == *manifest_ref {
                manifest.durable_pins.clone()
            } else {
                manifest
                    .dependencies
                    .get(&reference.sha256)
                    .cloned()
                    .ok_or_else(|| BundleError::Corrupt("missing dependency row".into()))?
            };
            let charge =
                crate::storage_budget::object_metadata_charge(&reference, dependencies.len())?;
            physical = physical
                .checked_add(reference.byte_length)
                .ok_or(BundleError::Capacity("physical overflow"))?;
            if reference.record_kind.is_owned_bytes() {
                owned = owned
                    .checked_add(reference.byte_length)
                    .ok_or(BundleError::Capacity("owned overflow"))?;
            }
            metadata = metadata
                .checked_add(charge)
                .ok_or(BundleError::Capacity("metadata overflow"))?;
            edges = edges
                .checked_add(dependencies.len() as u64)
                .ok_or(BundleError::Capacity("edge overflow"))?;
            ledger
                .metadata_sizes
                .insert(reference.sha256.clone(), charge);
            ledger.verified.insert(reference.sha256.clone());
            ledger
                .objects
                .insert(reference.sha256.clone(), (reference, dependencies));
        }
        if owned > inner.limits.owned_bytes || metadata > inner.limits.metadata_bytes {
            return Err(BundleError::Capacity("restored graph quota"));
        }
        ledger.inventory_dependencies = manifest.inventory_dependencies.clone();
        ledger
            .inventory_dependencies
            .insert(manifest_ref.sha256.clone(), manifest.recovery.references());
        ledger.mirrors.clone_from(&manifest.mirrors);
        let mut state = inner
            .usage
            .lock()
            .map_err(|_| BundleError::Capacity("usage poisoned"))?;
        if state.physical_bytes != 0 {
            return Err(BundleError::Identity(
                "physical accounting already adopted".into(),
            ));
        }
        state.physical_bytes = physical;
        state.peak_physical_bytes = physical;
        state.owned_bytes = owned;
        state.metadata_bytes = metadata;
        state.edges = edges;
        state.objects = ledger.objects.len() as u64;
        drop(state);
        store.account_residuals(true)?;
        Ok(store)
    }
    pub(crate) fn has_host_switch_receipts(
        &self,
        objects: &[ObjectRef],
    ) -> Result<bool, BundleError> {
        for reference in objects
            .iter()
            .filter(|r| r.record_kind == RecordKind::ReceiptIndex)
        {
            if self
                .read_record::<crate::receipts::ReceiptLedger>(reference)?
                .has_switch()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub(crate) fn has_archived_manual_switch(
        &self,
        objects: &[ObjectRef],
    ) -> Result<bool, BundleError> {
        #[derive(Deserialize)]
        struct RootCapabilities {
            studio: Option<StudioCapabilities>,
        }
        #[derive(Deserialize)]
        struct StudioCapabilities {
            manual_switch_generation: u64,
        }
        for reference in objects
            .iter()
            .filter(|r| r.record_kind == RecordKind::RootCheckpoint)
        {
            let _encoded = self.reserve(0, reference.byte_length, 0, false)?;
            let bytes = crate::bundle_recovery::bounded_bytes(
                self.open_verified(reference)?,
                reference.byte_length,
            )?;
            // Derive the raw requirement without allocating another Document. The shared
            // publication reader still fully admits every archived root before B5.
            let capabilities: RootCapabilities =
                serde_json::from_slice(&bytes).map_err(|e| BundleError::Corrupt(e.to_string()))?;
            if capabilities
                .studio
                .as_ref()
                .is_some_and(|r| r.manual_switch_generation != 0)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub(crate) fn evidence_roles(
        &self,
        objects: &[ObjectRef],
    ) -> Result<BTreeMap<String, String>, BundleError> {
        let mut roles = BTreeMap::new();
        for reference in objects.iter().filter(|r| {
            r.record_kind == RecordKind::OwnedEvidence
                && r.byte_length <= crate::rescue_record::PROVENANCE_BYTES as u64
        }) {
            if let Some(record) = crate::rescue_record::parse(&self.read_evidence(reference)?)? {
                roles.insert(reference.sha256.clone(), record.role);
            }
        }
        Ok(roles)
    }
    pub(crate) fn selected_manifest(
        &self,
        roots: &[ObjectRef],
        pins: &[ObjectRef],
        recovery: RecoveryInventory,
        inventory: &crate::bundle::SupportInventory,
        generation: u64,
    ) -> Result<StoreManifest, BundleError> {
        let mut all = roots.to_vec();
        all.extend_from_slice(pins);
        all.extend(recovery.references());
        let objects = self.closure(&all)?;
        let evidence_roles = self.evidence_roles(&objects)?;
        let ledger = self
            .inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
        let hashes: BTreeSet<_> = objects.iter().map(|r| r.sha256.as_str()).collect();
        Ok(StoreManifest {
            project_id: self.inner.project_id.clone(),
            session_id: self.inner.session_id.clone(),
            generation,
            objects: objects.clone(),
            dependencies: ledger
                .objects
                .iter()
                .filter(|(h, _)| hashes.contains(h.as_str()))
                .map(|(h, (_, d))| (h.clone(), d.clone()))
                .collect(),
            inventory_dependencies: ledger
                .inventory_dependencies
                .iter()
                .filter(|(h, _)| hashes.contains(h.as_str()))
                .map(|(h, d)| (h.clone(), d.clone()))
                .collect(),
            durable_pins: pins.to_vec(),
            recovery,
            required_fields: inventory.required_fields.clone(),
            required_kinds: objects
                .iter()
                .map(|r| r.record_kind)
                .chain(std::iter::once(RecordKind::StoreManifest))
                .collect(),
            required_apis: inventory.required_apis.clone(),
            evidence_roles,
            mirrors: ledger
                .mirrors
                .iter()
                .filter(|(r, _)| hashes.contains(r.sha256.as_str()))
                .cloned()
                .collect(),
        })
    }
    pub(crate) fn begin_publication(&mut self) -> Result<(), BundleError> {
        self.with_namespace(crate::namespace::ProjectNamespace::begin_object_batch)?;
        self.inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?
            .can_collect = false;
        Ok(())
    }
    pub(crate) fn mark_committed(&mut self, roots: &[ObjectRef]) -> Result<StorePin, BundleError> {
        let pin = self.pin(roots)?;
        let mut ledger = self
            .inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
        ledger.committed = roots.to_vec();
        ledger.acknowledged_objects = ledger
            .objects
            .keys()
            .chain(ledger.unclassified.keys())
            .cloned()
            .collect();
        ledger.can_collect = false;
        Ok(pin)
    }
    pub(crate) fn allow_collection(&mut self) -> Result<(), BundleError> {
        self.with_namespace(|n| n.verify())?;
        self.inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?
            .can_collect = true;
        Ok(())
    }
}
impl RecoveryInventory {
    pub(crate) fn references(&self) -> Vec<ObjectRef> {
        self.predecessors
            .iter()
            .cloned()
            .chain(self.staging.clone())
            .chain(self.pre_migration.clone())
            .collect()
    }
    pub(crate) fn validate(&self) -> Result<(), BundleError> {
        if self.predecessors.len() > 2 {
            return Err(BundleError::Capacity("automatic predecessors"));
        }
        for reference in self.references() {
            reference.validate()?;
            if reference.record_kind != RecordKind::RootCheckpoint {
                return Err(BundleError::Corrupt(
                    "recovery reference is not exact root bytes".into(),
                ));
            }
        }
        Ok(())
    }
}

impl ProjectStore {
    pub(crate) fn usage_arc(&self) -> Arc<Mutex<StorageUsage>> {
        Arc::clone(&self.inner.usage)
    }
    pub(crate) fn publish_inventory(
        &mut self,
        inventory: &RecoveryInventory,
    ) -> Result<(), BundleError> {
        inventory.validate()?;
        let mut count = BoundedCounter {
            bytes: 0,
            limit: self.limits().control_bytes,
        };
        serde_json::to_writer(&mut count, inventory)
            .map_err(|_| BundleError::Capacity("recovery inventory"))?;
        let _encoded = self.reserve(0, count.bytes, 0, false)?;
        let bytes =
            serde_json::to_vec(inventory).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        self.publish_control(
            &bytes,
            false,
            &mut crate::namespace::RootReplaceBoundary::NeverAttempted,
        )
    }
    pub(crate) fn publish_root(
        &mut self,
        bytes: &[u8],
    ) -> Result<(), crate::namespace::RootReplaceFailure> {
        let mut boundary = crate::namespace::RootReplaceBoundary::NeverAttempted;
        let result = self.with_namespace(|n| {
            crate::namespace::note("publication_reopen_fit", &n.identity().resolved_root);
            n.verify()?;
            crate::bundle_recovery::validate_publication_reopen_fit(
                n,
                bytes,
                self.limits(),
                &mut *self
                    .inner
                    .verified_memo
                    .lock()
                    .map_err(|_| BundleError::Capacity("memo poisoned"))?,
            )
        });
        result
            .and_then(|()| self.publish_control(bytes, true, &mut boundary))
            .map_err(|error| crate::namespace::RootReplaceFailure { boundary, error })
    }
    fn publish_control(
        &mut self,
        bytes: &[u8],
        root: bool,
        boundary: &mut crate::namespace::RootReplaceBoundary,
    ) -> Result<(), BundleError> {
        let length = bytes.len() as u64;
        if length > self.limits().control_bytes
            || self
                .usage()?
                .metadata_bytes
                .checked_add(length)
                .is_none_or(|n| n > self.limits().metadata_bytes)
        {
            return Err(BundleError::Capacity("control publication"));
        }
        let reservation = self.reserve(length, 0, 0, false)?;
        let result = if root {
            self.with_namespace(|n| match n.replace_root(bytes) {
                Ok(()) => {
                    // Any later accounting failure follows a completed root replacement.
                    *boundary = crate::namespace::RootReplaceBoundary::Unknown;
                    Ok(())
                }
                Err(failure) => {
                    *boundary = failure.boundary;
                    Err(failure.error)
                }
            })
        } else {
            self.with_namespace(|n| {
                n.replace_relative(&crate::bundle_recovery::inventory_path()?, bytes)
            })
        };
        if let Err(error) = result {
            if self.with_namespace(|n| Ok(n.is_uncertain()))? {
                self.inner
                    .ledger
                    .lock()
                    .map_err(|_| BundleError::Capacity("ledger poisoned"))?
                    .pending_controls
                    .push(reservation);
                self.inner
                    .usage
                    .lock()
                    .map_err(|_| BundleError::Capacity("usage poisoned"))?
                    .unsettled = true;
            }
            return Err(error);
        }
        reservation.seal(length)?;
        let mut prior = if root {
            self.inner.root_bytes.lock()
        } else {
            self.inner.inventory_bytes.lock()
        }
        .map_err(|_| BundleError::Capacity("control accounting"))?;
        let mut usage = self
            .inner
            .usage
            .lock()
            .map_err(|_| BundleError::Capacity("usage poisoned"))?;
        usage.physical_bytes = usage
            .physical_bytes
            .checked_sub(*prior)
            .ok_or(BundleError::Capacity("control retirement underflow"))?;
        usage.metadata_bytes = usage
            .metadata_bytes
            .checked_sub(*prior)
            .and_then(|n| n.checked_add(length))
            .ok_or(BundleError::Capacity("control metadata overflow"))?;
        *prior = length;
        Ok(())
    }
    fn memoize_support_decode(
        &self,
        reference: &ObjectRef,
        decoded: u64,
        fields: BTreeSet<String>,
    ) -> Result<(), BundleError> {
        let ledger = self
            .inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
        let (identity, dependencies) = ledger
            .objects
            .get(&reference.sha256)
            .ok_or_else(|| BundleError::Corrupt("support identity absent from store".into()))?;
        if identity != reference {
            return Err(BundleError::Corrupt("support identity changed".into()));
        }
        let inventory = ledger
            .inventory_dependencies
            .get(&reference.sha256)
            .cloned()
            .unwrap_or_default();
        self.inner
            .verified_memo
            .lock()
            .map_err(|_| BundleError::Capacity("memo poisoned"))?
            .remember_raw(
                reference,
                decoded,
                fields,
                dependencies.clone(),
                inventory,
                self.limits(),
            );
        Ok(())
    }
    pub(crate) fn support_fields(
        &self,
        roots: &[ObjectRef],
    ) -> Result<BTreeSet<String>, BundleError> {
        let mut fields = BTreeSet::new();
        for reference in self.closure(roots)? {
            if !matches!(
                reference.record_kind,
                RecordKind::Document
                    | RecordKind::SpecialistBase
                    | RecordKind::History
                    | RecordKind::Lineage
                    | RecordKind::RootCheckpoint
            ) {
                continue;
            }
            {
                let ledger = self
                    .inner
                    .ledger
                    .lock()
                    .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
                if let Some(cached) = ledger.support_fields.get(&reference.sha256) {
                    self.inner
                        .verified_memo
                        .lock()
                        .map_err(|_| BundleError::Capacity("memo poisoned"))?
                        .remember_fields(&reference, cached, self.limits());
                    fields.extend(cached.iter().cloned());
                    continue;
                }
            }
            if let Some(cached) = self
                .inner
                .verified_memo
                .lock()
                .map_err(|_| BundleError::Capacity("memo poisoned"))?
                .cached_fields(&reference, self.limits())
            {
                fields.extend(cached);
                continue;
            }
            let mut object_fields = BTreeSet::new();
            if reference.byte_length > self.limits().control_bytes {
                return Err(BundleError::Capacity("support record bytes"));
            }
            let _encoded = self.reserve(0, reference.byte_length, 0, false)?;
            let mut file = self.open_verified(&reference)?;
            let mut bytes = Vec::new();
            bytes
                .try_reserve_exact(
                    usize::try_from(reference.byte_length)
                        .map_err(|_| BundleError::Capacity("record address space"))?,
                )
                .map_err(|_| BundleError::Capacity("support bytes allocation"))?;
            (&mut file)
                .take(reference.byte_length + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() as u64 != reference.byte_length {
                return Err(BundleError::Corrupt("support bytes changed".into()));
            }
            if reference.record_kind == RecordKind::RootCheckpoint {
                let admission = crate::bundle::BundleWriteAdmission::from_bytes(None, bytes)?;
                admission.require_supported()?;
                object_fields.extend(admission.data.inventory.required_fields.iter().cloned());
            } else {
                let raw = crate::bundle_recovery::decode::<serde_json::Value>(
                    &bytes,
                    &self.inner.usage,
                    reference.record_kind,
                    &mut object_fields,
                    self.limits(),
                )?;
                self.memoize_support_decode(
                    &reference,
                    raw.decoded_charge(),
                    object_fields.clone(),
                )?;
            }
            let cache_charge = object_fields.iter().try_fold(128_u64, |total, field| {
                total
                    .checked_add(field.len() as u64 + 64)
                    .ok_or(BundleError::Capacity("support field cache"))
            })?;
            fields.extend(object_fields.iter().cloned());
            let cached = self.reserve(0, cache_charge, 0, false)?.hold(object_fields);
            self.inner
                .ledger
                .lock()
                .map_err(|_| BundleError::Capacity("ledger poisoned"))?
                .support_fields
                .insert(reference.sha256, cached);
        }
        Ok(fields)
    }
}
impl ProjectStore {
    /// Finish uncertain object namespace barriers and account residual bytes before
    /// releasing their staging reservations. This does not decide any Core floor.
    pub(crate) fn settle_object_writes(&mut self) -> Result<(), BundleError> {
        if !self.has_unsettled_objects()? {
            return Ok(());
        }
        self.with_namespace(|n| {
            let root = n.read_root()?;
            n.reconcile_copy_root(root.as_deref())?;
            // Retry the object-directory syncs whose outcome was unknown.
            n.finish_object_batch()
        })?;
        self.account_residuals(false)?;
        self.inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?
            .pending
            .clear();
        self.settle_controls()
    }
    pub(crate) fn settle_controls(&mut self) -> Result<(), BundleError> {
        let mut ledger = self
            .inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
        if !ledger.pending.is_empty() {
            return Err(BundleError::Unsettled);
        }
        ledger.pending_controls.clear();
        let (root, inventory) = self.with_namespace(|n| {
            Ok((
                n.read_root()?.map_or(0, |b| b.len() as u64),
                n.read(
                    &crate::bundle_recovery::inventory_path()?,
                    self.limits().control_bytes,
                )?
                .map_or(0, |b| b.len() as u64),
            ))
        })?;
        let mut old_root = self
            .inner
            .root_bytes
            .lock()
            .map_err(|_| BundleError::Capacity("root accounting"))?;
        let mut old_inventory = self
            .inner
            .inventory_bytes
            .lock()
            .map_err(|_| BundleError::Capacity("inventory accounting"))?;
        let mut usage = self
            .inner
            .usage
            .lock()
            .map_err(|_| BundleError::Capacity("usage poisoned"))?;
        let old = old_root
            .checked_add(*old_inventory)
            .ok_or(BundleError::Capacity("old controls overflow"))?;
        let new = root
            .checked_add(inventory)
            .ok_or(BundleError::Capacity("new controls overflow"))?;
        usage.physical_bytes = usage
            .physical_bytes
            .checked_sub(old)
            .and_then(|n| n.checked_add(new))
            .ok_or(BundleError::Capacity("control reconciliation accounting"))?;
        usage.metadata_bytes = usage
            .metadata_bytes
            .checked_sub(old)
            .and_then(|n| n.checked_add(new))
            .ok_or(BundleError::Capacity("control reconciliation metadata"))?;
        usage.peak_physical_bytes = usage.peak_physical_bytes.max(usage.physical_bytes);
        usage.unsettled = false;
        *old_root = root;
        *old_inventory = inventory;
        Ok(())
    }
}
impl ProjectStore {
    #[cfg(target_os = "linux")]
    pub(crate) fn adopt_usage(
        &mut self,
        usage: Arc<Mutex<StorageUsage>>,
    ) -> Result<(), BundleError> {
        let inner = Arc::get_mut(&mut self.inner)
            .ok_or_else(|| BundleError::Identity("store already shared".into()))?;
        let prior = inner
            .usage
            .lock()
            .map_err(|_| BundleError::Capacity("usage poisoned"))?
            .clone();
        let mut state = usage
            .lock()
            .map_err(|_| BundleError::Capacity("admission usage poisoned"))?;
        if state.physical_bytes != 0 || prior.objects != 0 {
            return Err(BundleError::Identity("store usage already adopted".into()));
        }
        if state
            .decoded_bytes
            .checked_sub(state.decoder_bytes)
            .and_then(|bytes| bytes.checked_add(prior.metadata_bytes))
            .is_none_or(|n| n > inner.limits.metadata_bytes)
            || state.decoder_bytes > inner.limits.decoder_bytes
        {
            return Err(BundleError::Capacity("admission/store overlap"));
        }
        state.physical_bytes = prior.physical_bytes;
        state.metadata_bytes = prior.metadata_bytes;
        state.peak_physical_bytes = state.peak_physical_bytes.max(prior.peak_physical_bytes);
        drop(state);
        inner.usage = usage;
        Ok(())
    }
}
impl ProjectStore {
    /// Read a bounded opaque evidence snapshot. This is no decoder-use certification or lease grant.
    pub fn read_evidence(
        &self,
        reference: &ObjectRef,
    ) -> Result<StorageReservation<Vec<u8>>, BundleError> {
        self.read_copy_evidence(reference, false)
    }
    pub(crate) fn read_copy_evidence(
        &self,
        reference: &ObjectRef,
        retained: bool,
    ) -> Result<StorageReservation<Vec<u8>>, BundleError> {
        if self.inner.read_authority.is_some() {
            return Err(BundleError::Identity(
                "private evidence requires the host evidence route".into(),
            ));
        }
        if reference.record_kind != RecordKind::OwnedEvidence
            || reference.byte_length > self.limits().control_bytes
        {
            return Err(BundleError::Capacity("evidence snapshot"));
        }
        let _pin = self.pin(std::slice::from_ref(reference))?;
        let charge = self.reserve_copy_read(0, reference.byte_length, 0, false, retained)?;
        let file = self.open_copy_source(reference, retained)?;
        let bytes = crate::bundle_recovery::bounded_bytes(file, reference.byte_length)?;
        if bytes.len() as u64 != reference.byte_length {
            return Err(BundleError::Corrupt(
                "evidence changed while reading".into(),
            ));
        }
        Ok(charge.hold(bytes))
    }
    #[cfg(any(test, feature = "test-util"))]
    pub fn pending_control_bytes_probe(&self) -> Result<u64, BundleError> {
        self.inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?
            .pending_controls
            .iter()
            .try_fold(0_u64, |sum, charge| {
                sum.checked_add(charge.reserved_bytes_probe())
                    .ok_or(BundleError::Capacity("control charge overflow"))
            })
    }
    pub(crate) fn has_unsettled_objects(&self) -> Result<bool, BundleError> {
        Ok(!self
            .inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?
            .pending
            .is_empty())
    }
}

/// One frozen source closure and its distinct destination-overlap reservation.
pub(crate) struct SourceCopyHold {
    pin: StorePin,
    overlap: Option<StorageReservation>,
    uncertain: bool,
    previous_collection: bool,
}
impl ProjectStore {
    /// Retain the real source objects under this ledger; only the descriptor may cross processes.
    pub(crate) fn retain_borrowed_source(
        &self,
        roots: &[ObjectRef],
        extra_overlap_bytes: u64,
    ) -> Result<crate::source_boundary::RetainedBorrowedSource, BundleError> {
        use crate::source_boundary::{
            BorrowedSnapshotOverlap, BorrowedSourceDescriptor, BorrowedSourceObject,
            RetainedBorrowedSource,
        };
        if roots.len() as u64 > self.limits().objects {
            return Err(BundleError::Capacity("borrowed source roots"));
        }
        let _working = self.reserve_copy_read(0, self.usage()?.metadata_bytes, 0, false, true)?;
        let roots_with_roles = roots
            .iter()
            .cloned()
            .map(|r| (r, crate::bundle_rescue::MissingOwnedUse::Manifest))
            .collect::<Vec<_>>();
        let admission = self.admit_retained_reads(&roots_with_roles)?;
        let manifest = self.manifest()?;
        let mut remaining = admission
            .objects()
            .iter()
            .cloned()
            .map(|r| (r.sha256.clone(), r))
            .collect::<BTreeMap<_, _>>();
        let mut ordered = Vec::new();
        let mut copied = BTreeSet::new();
        while !remaining.is_empty() {
            let ready = remaining
                .iter()
                .filter(|(hash, _)| {
                    manifest.dependencies[*hash]
                        .iter()
                        .all(|r| copied.contains(&r.sha256))
                })
                .map(|(hash, _)| hash.clone())
                .collect::<Vec<_>>();
            if ready.is_empty() {
                return Err(BundleError::Corrupt(
                    "borrowed source graph is not closed".into(),
                ));
            }
            for hash in ready {
                let reference = remaining
                    .remove(&hash)
                    .ok_or_else(|| BundleError::Corrupt("borrowed object disappeared".into()))?;
                ordered.push(BorrowedSourceObject {
                    reference,
                    dependencies: manifest.dependencies[&hash].clone(),
                    inventory_dependencies: manifest
                        .inventory_dependencies
                        .get(&hash)
                        .cloned()
                        .unwrap_or_default(),
                });
                copied.insert(hash);
            }
        }
        let (project_id, session_id) = self.input_owner();
        let descriptor = BorrowedSourceDescriptor {
            admission: SessionId::fresh()?,
            project_id,
            session_id,
            owner_epoch: self.owner_epoch(),
            extra_overlap_bytes,
            roots: roots.to_vec(),
            objects: ordered,
        };
        descriptor.validate(self.limits())?;
        let metadata = self.reserve_copy_read(0, descriptor.metadata_bytes()?, 0, false, true)?;
        let overlap = self.reserve_copy_read(extra_overlap_bytes, 0, 0, true, true)?;
        Ok(RetainedBorrowedSource {
            descriptor,
            reads: admission,
            _metadata: metadata,
            overlap: Mutex::new(BorrowedSnapshotOverlap {
                reservation: overlap,
                snapshot: None,
            }),
            accepting_reads: std::sync::atomic::AtomicBool::new(true),
            reader: Mutex::new(None),
        })
    }

    /// A host destination uses its own normal ledger and verifies every streamed object.
    /// This method never opens a source namespace or holds a source reservation.
    pub fn copy_graph_from_borrowed(
        &mut self,
        source: &crate::source_boundary::BorrowedSourceDescriptor,
        reader: &mut dyn crate::source_boundary::BorrowedSourceReader,
    ) -> Result<(), BundleError> {
        source.validate(self.limits())?;
        let path = self.resolved_root()?;
        let inspection = Self::inspect_borrowed_destination(&path, &[])?;
        if reader.inspect_destination(source, inspection.path())? != inspection.identity()? {
            return Err(BundleError::Identity(
                "borrowed destination identity differs".into(),
            ));
        }
        let _working = self.reserve(0, source.metadata_bytes()?, 0, false)?;
        for object in &source.objects {
            let mut stream = crate::source_boundary::BorrowedObjectReader::new(
                source,
                &object.reference,
                reader,
            );
            let sealed =
                self.admit_recovered(&mut stream, &object.reference, &object.dependencies)?;
            if !object.inventory_dependencies.is_empty() {
                self.inner
                    .ledger
                    .lock()
                    .map_err(|_| BundleError::Capacity("ledger poisoned"))?
                    .inventory_dependencies
                    .insert(
                        object.reference.sha256.clone(),
                        object.inventory_dependencies.clone(),
                    );
            }
            drop(sealed);
        }
        Ok(())
    }

    fn reserve_copy_read(
        &self,
        bytes: u64,
        decoded: u64,
        history: u64,
        copy: bool,
        cleanup_only: bool,
    ) -> Result<StorageReservation, BundleError> {
        if self
            .inner
            .session_closing
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(BundleError::Identity(
                "originating source is closing".into(),
            ));
        }
        let free = self.with_namespace(|n| n.available_bytes())?;
        StorageReservation::reserve_for_retained_read(
            &self.inner.usage,
            self.limits(),
            bytes,
            decoded,
            history,
            copy,
            free,
            cleanup_only,
        )
    }
    pub(crate) fn begin_copy(
        &self,
        roots: &[ObjectRef],
        bytes: u64,
    ) -> Result<SourceCopyHold, BundleError> {
        self.begin_copy_source(roots, bytes, false)
    }
    pub(crate) fn begin_copy_source(
        &self,
        roots: &[ObjectRef],
        bytes: u64,
        cleanup_only: bool,
    ) -> Result<SourceCopyHold, BundleError> {
        if !cleanup_only {
            self.with_namespace(|n| n.verify())?;
        }
        let overlap = self.reserve_copy_read(bytes, 0, 0, true, cleanup_only)?;
        let pin = self.pin(roots)?;
        let mut ledger = self
            .inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
        let previous_collection = ledger.can_collect;
        ledger.copy_holds = ledger
            .copy_holds
            .checked_add(1)
            .ok_or(BundleError::Capacity("copy holds"))?;
        Ok(SourceCopyHold {
            pin,
            overlap: Some(overlap),
            uncertain: false,
            previous_collection,
        })
    }
    pub(crate) fn rebind_unshared_copy_owner(&mut self, id: ProjectId) -> Result<(), BundleError> {
        let inner = Arc::get_mut(&mut self.inner).ok_or_else(|| {
            BundleError::Identity("destination store already has live readers".into())
        })?;
        let mut bytes = [0; 8];
        getrandom::fill(&mut bytes).map_err(|e| BundleError::Identity(e.to_string()))?;
        inner.project_id = Some(id);
        inner.session_id = None;
        inner.owner_epoch = u64::from_le_bytes(bytes).max(1);
        Ok(())
    }
    pub(crate) fn copy_graph_from(
        &mut self,
        source: &ProjectStore,
        roots: &[ObjectRef],
    ) -> Result<(), BundleError> {
        self.copy_graph_from_source(source, roots, false)
    }
    pub(crate) fn copy_graph_from_source(
        &mut self,
        source: &ProjectStore,
        roots: &[ObjectRef],
        cleanup_only: bool,
    ) -> Result<(), BundleError> {
        self.copy_selected_graph(source, roots, cleanup_only, true)
    }
    /// Check the destination profile while the retained source is still read-only.
    pub(crate) fn admit_rescue_record_profile(
        &self,
        reference: &ObjectRef,
        limits: &StorageLimits,
    ) -> Result<(), BundleError> {
        let cap = match reference.record_kind {
            RecordKind::History => limits.branch_history_bytes,
            RecordKind::RootCheckpoint | RecordKind::OwnedEvidence => limits.control_bytes,
            _ if reference.record_kind.is_owned_bytes() => return Ok(()),
            _ => limits.decoded_record_bytes,
        };
        if reference.byte_length > cap {
            return Err(BundleError::Capacity("rescue encoded record"));
        }
        if reference.record_kind == RecordKind::OwnedEvidence {
            return Ok(());
        }
        let _encoded = self.reserve_copy_read(0, reference.byte_length, 0, false, true)?;
        let bytes = crate::bundle_recovery::bounded_bytes(
            self.open_copy_source(reference, true)?,
            reference.byte_length,
        )?;
        let _decoded =
            self.reserve_copy_read(0, self.limits().decoded_record_bytes, 0, false, true)?;
        let charge = preflight_record(&bytes, limits)?;
        if reference.record_kind == RecordKind::History && charge > limits.branch_history_bytes {
            return Err(BundleError::Capacity("rescue decoded branch history"));
        }
        Ok(())
    }
    pub(crate) fn copy_rescue_graph(
        &mut self,
        source: &ProjectStore,
        roots: &[ObjectRef],
    ) -> Result<(), BundleError> {
        self.copy_selected_graph(source, roots, true, false)
    }
    fn copy_selected_graph(
        &mut self,
        source: &ProjectStore,
        roots: &[ObjectRef],
        cleanup_only: bool,
        inventory: bool,
    ) -> Result<(), BundleError> {
        let _working =
            source.reserve_copy_read(0, source.usage()?.metadata_bytes, 0, false, cleanup_only)?;
        let selected = source.closure_with_inventory(roots, inventory)?;
        let manifest = source.manifest()?;
        let mut remaining: BTreeMap<_, _> = selected
            .into_iter()
            .map(|r| (r.sha256.clone(), r))
            .collect();
        let mut copied = BTreeSet::new();
        while !remaining.is_empty() {
            let ready: Vec<_> = remaining
                .iter()
                .filter(|(hash, _)| {
                    manifest.dependencies[*hash]
                        .iter()
                        .all(|r| copied.contains(&r.sha256))
                })
                .map(|(h, _)| h.clone())
                .collect();
            if ready.is_empty() {
                return Err(BundleError::Corrupt(
                    "copy graph is not topologically closed".into(),
                ));
            }
            for hash in ready {
                let reference = remaining
                    .remove(&hash)
                    .ok_or_else(|| BundleError::Corrupt("copy selection disappeared".into()))?;
                let dependencies = &manifest.dependencies[&hash];
                let mut file = source.open_copy_source(&reference, cleanup_only)?;
                #[cfg(any(test, feature = "test-util"))]
                crate::io_observer::check_failure(
                    "copy_object",
                    Self::object_path(&reference)?.as_path(),
                )?;
                let object = self.admit_recovered(&mut file, &reference, dependencies)?;
                if let Some(inventory) = manifest.inventory_dependencies.get(&hash) {
                    self.inner
                        .ledger
                        .lock()
                        .map_err(|_| BundleError::Capacity("ledger poisoned"))?
                        .inventory_dependencies
                        .insert(hash.clone(), inventory.clone());
                }
                copied.insert(hash);
                drop(object);
            }
        }
        Ok(())
    }
}
impl SourceCopyHold {
    /// Context teardown ends source reuse, without proving any destination outcome.
    pub(crate) fn close_originating_context(&mut self) {
        self.uncertain = false;
    }

    pub(crate) const fn requires_reconciliation(&self) -> bool {
        self.uncertain
    }
    pub(crate) fn mark_uncertain(&mut self) -> Result<(), BundleError> {
        self.uncertain = true;
        self.pin
            .inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?
            .can_collect = false;
        self.pin
            .inner
            .usage
            .lock()
            .map_err(|_| BundleError::Capacity("usage poisoned"))?
            .unsettled = true;
        Ok(())
    }
    pub(crate) fn reconciled(&mut self) {
        self.uncertain = false;
        if let Ok(ledger) = self.pin.inner.ledger.lock()
            && ledger.pending.is_empty()
            && ledger.pending_controls.is_empty()
            && let Ok(mut usage) = self.pin.inner.usage.lock()
        {
            usage.unsettled = false;
        }
    }
}
impl Drop for SourceCopyHold {
    fn drop(&mut self) {
        if self.uncertain {
            if let Some(reservation) = self.overlap.take() {
                reservation.retain_charge();
            }
            return;
        }
        if let Ok(mut ledger) = self.pin.inner.ledger.lock() {
            if let Some(next) = ledger.copy_holds.checked_sub(1) {
                ledger.copy_holds = next;
                if next == 0 {
                    ledger.can_collect = self.previous_collection;
                    if ledger.pending.is_empty()
                        && ledger.pending_controls.is_empty()
                        && let Ok(mut usage) = self.pin.inner.usage.lock()
                    {
                        usage.unsettled = false;
                    }
                }
            } else if let Ok(mut usage) = self.pin.inner.usage.lock() {
                usage.damaged_accounting = true;
            }
        }
    }
}

impl ProjectStore {
    /// Charge all residuals until the claimed, verified closure permits unreachable-name retirement.
    #[allow(
        clippy::case_sensitive_file_extension_comparisons,
        reason = "exact generated names define temporary ownership"
    )]
    #[allow(
        clippy::too_many_lines,
        reason = "one owner classifies, accounts and retires residual physical files"
    )]
    pub(crate) fn account_residuals(&mut self, verified_graph: bool) -> Result<(), BundleError> {
        let files = self.with_namespace(|n| n.physical_files(self.inner.limits.objects))?;
        let mut ledger = self
            .inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
        let mut usage = self
            .inner
            .usage
            .lock()
            .map_err(|_| BundleError::Capacity("usage poisoned"))?;
        for file in files {
            let name = file
                .relative
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| BundleError::Identity("physical filename".into()))?;
            if !file.root_parent && file.relative.parent() == Some(Path::new("studio/objects")) {
                if let Some((r, _)) = ledger.objects.get(name) {
                    if r.byte_length != file.length {
                        return Err(BundleError::Corrupt("physical object length".into()));
                    }
                    continue;
                }
                if ledger.unclassified.contains_key(name) {
                    continue;
                }
            } else if ledger
                .temporaries
                .iter()
                .any(|f| f.relative == file.relative && f.root_parent == file.root_parent)
            {
                continue;
            }
            let metadata_charge = file
                .length
                .checked_add(256)
                .ok_or(BundleError::Capacity("unclassified metadata overflow"))?;
            let owned = usage
                .owned_bytes
                .checked_add(file.length)
                .ok_or(BundleError::Capacity("unclassified owned overflow"))?;
            let metadata = usage
                .metadata_bytes
                .checked_add(metadata_charge)
                .ok_or(BundleError::Capacity("unclassified metadata overflow"))?;
            let count = usage
                .objects
                .checked_add(1)
                .ok_or(BundleError::Capacity("unclassified object overflow"))?;
            if owned > self.inner.limits.owned_bytes
                || metadata > self.inner.limits.metadata_bytes
                || count > self.inner.limits.objects
            {
                let generated_orphan = !file.root_parent
                    && file.relative.parent() == Some(Path::new("studio/objects"))
                    && name.len() == 64
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
                if verified_graph && generated_orphan {
                    // The fully verified root/retained inventory excludes this generated name.
                    // Retire it before an old crash's orphan can prevent reopening for cleanup.
                    drop(usage);
                    drop(ledger);
                    self.with_namespace(|namespace| {
                        namespace.unlink_owned(&PortablePath::new(&file.relative)?)
                    })?;
                    ledger = self
                        .inner
                        .ledger
                        .lock()
                        .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
                    usage = self
                        .inner
                        .usage
                        .lock()
                        .map_err(|_| BundleError::Capacity("usage poisoned"))?;
                    continue;
                }
                return Err(BundleError::Capacity("unclassified physical files"));
            }
            usage.physical_bytes = usage
                .physical_bytes
                .checked_add(file.length)
                .ok_or(BundleError::Capacity("unclassified physical overflow"))?;
            usage.unclassified_bytes = usage
                .unclassified_bytes
                .checked_add(file.length)
                .ok_or(BundleError::Capacity("unclassified bytes overflow"))?;
            usage.peak_physical_bytes = usage.peak_physical_bytes.max(usage.physical_bytes);
            usage.owned_bytes = owned;
            usage.metadata_bytes = metadata;
            usage.objects = count;
            if name.starts_with(".studio-") && name.ends_with(".tmp") {
                ledger.temporaries.push(file);
            } else if !file.root_parent
                && file.relative.parent() == Some(Path::new("studio/objects"))
                && name.len() == 64
                && name
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                ledger.unclassified.insert(name.to_owned(), file.length);
            } else {
                return Err(BundleError::Identity(
                    "unclassified non-object managed file".into(),
                ));
            }
        }
        Ok(())
    }
    pub(crate) fn restrict_copy_limits(
        &mut self,
        limits: StorageLimits,
    ) -> Result<(), BundleError> {
        limits.validate()?;
        let usage = self.usage()?;
        if usage.objects > limits.objects
            || usage.owned_bytes > limits.owned_bytes
            || usage
                .metadata_bytes
                .checked_add(
                    usage
                        .decoded_bytes
                        .checked_sub(usage.decoder_bytes)
                        .ok_or(BundleError::Capacity("decoder accounting subset"))?,
                )
                .is_none_or(|n| n > limits.metadata_bytes)
            || usage.decoder_bytes > limits.decoder_bytes
            || usage.live_history_bytes > limits.live_history_bytes
        {
            return Err(BundleError::Capacity("destination quota"));
        }
        Arc::get_mut(&mut self.inner)
            .ok_or_else(|| BundleError::Identity("destination already shared".into()))?
            .limits = limits;
        Ok(())
    }
    pub(crate) fn rollback_copy(
        &mut self,
        root: Option<&[u8]>,
        inventory: Option<&[u8]>,
        had_assets: bool,
    ) -> Result<(), BundleError> {
        if self.has_unsettled_objects()? {
            return Err(BundleError::Unsettled);
        }
        self.with_namespace(|n| n.reconcile_copy_root(root))?;
        let path = crate::bundle_recovery::inventory_path()?;
        self.with_namespace(|n| {
            if let Some(bytes) = inventory {
                n.replace_relative(&path, bytes)
            } else {
                n.unlink_owned(&path)
            }
        })?;
        self.settle_controls()?;
        self.allow_collection()?;
        self.collect_copy_staging()?;
        if !had_assets {
            self.with_namespace(ProjectNamespace::cleanup_empty_copy_store)?;
        }
        Ok(())
    }
    fn collect_copy_staging(&mut self) -> Result<(), BundleError> {
        let (acknowledged, unclassified, temporaries) = {
            let mut ledger = self
                .inner
                .ledger
                .lock()
                .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
            let acknowledged = std::mem::take(&mut ledger.acknowledged_objects);
            // Only this failed copy's new names may disappear. Even an older
            // unreachable file may still belong to the restored B3 inventory.
            ledger.acknowledged_objects = ledger
                .objects
                .keys()
                .filter(|hash| !acknowledged.contains(*hash))
                .cloned()
                .collect();
            (
                acknowledged,
                std::mem::take(&mut ledger.unclassified),
                std::mem::take(&mut ledger.temporaries),
            )
        };
        let result = self.collect_unpublished();
        let mut ledger = self
            .inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
        ledger.acknowledged_objects = acknowledged;
        ledger.unclassified = unclassified;
        ledger.temporaries = temporaries;
        // Releasing the failed copy owner must not run ordinary maintenance on
        // the exact prior inventory that rollback just restored.
        ledger.can_collect = false;
        result.map(|_| ())
    }
}

#[cfg(all(test, target_os = "linux"))]
mod c0_fix1_tests {
    use super::*;

    #[test]
    fn s3_fix1_reclassification_refuses_a_different_orphan_length_before_registration() {
        let temp = kinewright_media::test_support::TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "c0-fix1-orphan-length",
        );
        let mut session =
            crate::bundle_copy::SessionStore::create(temp.root(), StorageLimits::default())
                .unwrap();
        let bytes = vec![17; 4096];
        let (sealed, _) = session
            .store_mut()
            .admit_measured_media(&mut bytes.as_slice(), bytes.len() as u64)
            .unwrap();
        let reference = sealed.reference.clone();
        {
            let mut ledger = session.store().inner.ledger.lock().unwrap();
            ledger.objects.remove(&reference.sha256);
            ledger
                .unclassified
                .insert(reference.sha256.clone(), reference.byte_length + 1);
        }
        let before = session.store().usage().unwrap();
        let result = session
            .store_mut()
            .admit_measured_media(&mut bytes.as_slice(), bytes.len() as u64);
        assert!(result.is_err(), "C0_FIX1_ORPHAN_LENGTH_REFUSED");
        let mut ledger = session.store().inner.ledger.lock().unwrap();
        assert!(
            !ledger.objects.contains_key(&reference.sha256),
            "C0_FIX1_NO_INVALID_REGISTRATION"
        );
        assert_eq!(
            ledger.unclassified.get(&reference.sha256),
            Some(&(reference.byte_length + 1))
        );
        ledger.unclassified.remove(&reference.sha256);
        ledger
            .objects
            .insert(reference.sha256.clone(), (reference, vec![]));
        drop(ledger);
        let after = session.store().usage().unwrap();
        assert_eq!(before.physical_bytes, after.physical_bytes);
        assert_eq!(after.reserved_bytes, 0);
        drop(sealed);
    }

    #[test]
    fn s3_fix1_orphan_cleanup_preserves_admitted_and_pinned_identities() {
        for admitted in [true, false] {
            let temp = kinewright_media::test_support::TempDirectory::new_in(
                &kinewright_media::test_support::workspace_target_dir(),
                "c0-fix1-orphan-guard",
            );
            let mut session =
                crate::bundle_copy::SessionStore::create(temp.root(), StorageLimits::default())
                    .unwrap();
            let bytes = vec![17; 4096];
            let (sealed, _) = session
                .store_mut()
                .admit_measured_media(&mut bytes.as_slice(), bytes.len() as u64)
                .unwrap();
            let reference = sealed.reference.clone();
            {
                let mut ledger = session.store().inner.ledger.lock().unwrap();
                ledger
                    .unclassified
                    .insert(reference.sha256.clone(), reference.byte_length);
                if admitted {
                    ledger.pins.remove(&reference.sha256);
                    ledger.committed.push(reference.clone());
                } else {
                    ledger.objects.remove(&reference.sha256);
                }
            }
            // Model the stale dual classification left by the pre-fix reuse path.
            let result = session.store_mut().collect_unpublished();
            let actual = session.store().with_namespace(|namespace| {
                namespace.read(
                    &ProjectStore::object_path(&reference)?,
                    reference.byte_length,
                )
            });
            assert_eq!(
                actual.unwrap().unwrap_or_default(),
                bytes,
                "C0_FIX1_ORPHAN_CLEANUP_GUARD"
            );
            result.unwrap();
            let mut ledger = session.store().inner.ledger.lock().unwrap();
            ledger.unclassified.remove(&reference.sha256);
            if admitted {
                ledger.pins.insert(reference.sha256, 1);
                ledger.committed.clear();
            } else {
                ledger
                    .objects
                    .insert(reference.sha256.clone(), (reference, vec![]));
            }
            drop(ledger);
            drop(sealed);
        }
    }
}

#[path = "snapshot_reader.rs"]
pub mod snapshot_reader;
impl ProjectStore {
    pub(crate) fn read_authority(&self) -> Option<kinewright_media::owned_io::ReadAuthority> {
        self.inner.read_authority.clone()
    }
    pub(crate) fn check_read_authority(&self) -> Result<(), BundleError> {
        self.inner.read_authority.as_ref().map_or(Ok(()), |a| {
            a.check().map_err(|e| BundleError::Identity(e.to_string()))
        })
    }
}
impl StorePin {
    pub(crate) fn read_store(&self) -> ProjectStore {
        ProjectStore {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl ProjectStore {
    pub(crate) fn snapshot_project_id(&self) -> Result<ProjectId, BundleError> {
        self.check_read_authority()?;
        self.inner
            .project_id
            .clone()
            .ok_or_else(|| BundleError::Identity("snapshot has no named owner".into()))
    }
}
