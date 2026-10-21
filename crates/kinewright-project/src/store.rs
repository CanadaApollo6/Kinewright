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
    pub objects: Vec<ObjectRef>,
    pub dependencies: BTreeMap<String, Vec<ObjectRef>>,
    pub durable_pins: Vec<ObjectRef>,
    pub required_kinds: BTreeSet<RecordKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mirrors: Vec<(ObjectRef, PortablePath)>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryInventory {
    pub generation: u64,
    pub predecessors: Vec<ObjectRef>,
    pub staging: Option<ObjectRef>,
    pub pre_migration: Option<ObjectRef>,
}

struct Ledger {
    objects: BTreeMap<String, (ObjectRef, Vec<ObjectRef>)>,
    pins: BTreeMap<String, u64>,
    verified: BTreeSet<String>,
    committed: Vec<ObjectRef>,
    mirrors: Vec<(ObjectRef, PortablePath)>,
    can_collect: bool,
    metadata_sizes: BTreeMap<String, u64>,
    pending: Vec<StorageReservation>,
}

struct StoreInner {
    namespace: Mutex<ProjectNamespace>,
    ledger: Mutex<Ledger>,
    usage: Arc<Mutex<StorageUsage>>,
    limits: StorageLimits,
    project_id: Option<ProjectId>,
    session_id: Option<SessionId>,
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

impl SealedObject {
    #[must_use]
    pub fn into_pin(self) -> StorePin {
        self.pin
    }
}
impl ProjectStore {
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
        namespace.verify()?;
        let can_collect = namespace.read_root()?.is_none();
        Ok(Self {
            inner: Arc::new(StoreInner {
                namespace: Mutex::new(namespace),
                ledger: Mutex::new(Ledger {
                    objects: BTreeMap::new(),
                    pins: BTreeMap::new(),
                    verified: BTreeSet::new(),
                    committed: vec![],
                    mirrors: vec![],
                    can_collect,
                    metadata_sizes: BTreeMap::new(),
                    pending: vec![],
                }),
                usage: Arc::new(Mutex::new(StorageUsage::default())),
                limits,
                project_id,
                session_id,
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
        run(&mut namespace)
    }
    pub(crate) fn reserve(
        &self,
        bytes: u64,
        decoded: u64,
        history: u64,
        copy: bool,
    ) -> Result<StorageReservation, BundleError> {
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
    #[allow(clippy::too_many_lines)]
    pub(crate) fn admit(
        &mut self,
        reader: &mut impl Read,
        reference: &ObjectRef,
        dependencies: &[ObjectRef],
    ) -> Result<SealedObject, BundleError> {
        reference.validate()?;
        let _working = self.reserve(0, (1 << 20) + 65_536, 0, false)?;
        let mut dependencies = dependencies.to_vec();
        dependencies.sort();
        dependencies.dedup();
        self.closure(&dependencies)?;
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
                let usage = self.usage()?;
                let metadata_charge = 256_u64
                    .checked_add(
                        (dependencies.len() as u64)
                            .checked_mul(160)
                            .ok_or(BundleError::Capacity("edge metadata overflow"))?,
                    )
                    .and_then(|n| {
                        n.checked_add(if reference.record_kind.is_blob() {
                            0
                        } else {
                            reference.byte_length
                        })
                    })
                    .ok_or(BundleError::Capacity("metadata overflow"))?;
                if usage
                    .metadata_bytes
                    .checked_add(metadata_charge)
                    .is_none_or(|n| n > self.inner.limits.metadata_bytes)
                    || (reference.record_kind.is_blob()
                        && usage
                            .owned_bytes
                            .checked_add(reference.byte_length)
                            .is_none_or(|n| n > self.inner.limits.owned_bytes))
                {
                    return Err(BundleError::Capacity("object/edge metadata or owned bytes"));
                }
                if ledger.objects.len() as u64 >= self.inner.limits.objects {
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
        let reservation = self.reserve(reference.byte_length, 0, 0, false)?;
        let path = Self::object_path(reference)?;
        let published = self.with_namespace(|namespace| {
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
                Ok(())
            })
        });
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
        reservation.seal(reference.byte_length)?;
        {
            let mut ledger = self
                .inner
                .ledger
                .lock()
                .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
            ledger.objects.insert(
                reference.sha256.clone(),
                (reference.clone(), dependencies.clone()),
            );
            ledger.verified.insert(reference.sha256.clone());
            let metadata_charge = 256
                + (dependencies.len() as u64) * 160
                + if reference.record_kind.is_blob() {
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
        usage.objects = usage
            .objects
            .checked_add(1)
            .ok_or(BundleError::Capacity("object accounting overflow"))?;
        usage.edges = usage
            .edges
            .checked_add(dependencies.len() as u64)
            .ok_or(BundleError::Capacity("edge accounting overflow"))?;
        let metadata_charge = 256
            + (dependencies.len() as u64) * 160
            + if reference.record_kind.is_blob() {
                0
            } else {
                reference.byte_length
            };
        usage.metadata_bytes = usage
            .metadata_bytes
            .checked_add(metadata_charge)
            .ok_or(BundleError::Capacity("metadata accounting overflow"))?;
        if reference.record_kind.is_blob() {
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
    pub fn closure(&self, roots: &[ObjectRef]) -> Result<Vec<ObjectRef>, BundleError> {
        let ledger = self
            .inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
        let mut found = BTreeMap::new();
        let mut active = BTreeSet::new();
        for root in roots {
            walk(
                root,
                &ledger,
                &self.inner.limits,
                &mut found,
                &mut active,
                0,
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
        let _working = self.reserve(0, 1 << 20, 0, false)?;
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
    pub const fn owner_epoch(&self) -> u64 {
        1
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
        collect(&self.inner)
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
            objects: ledger.objects.values().map(|(r, _)| r.clone()).collect(),
            dependencies: ledger
                .objects
                .iter()
                .map(|(h, (_, d))| (h.clone(), d.clone()))
                .collect(),
            durable_pins: ledger.committed.clone(),
            required_kinds: ledger
                .objects
                .values()
                .map(|(r, _)| r.record_kind)
                .collect(),
            mirrors: ledger.mirrors.clone(),
        })
    }
}

fn walk(
    reference: &ObjectRef,
    ledger: &Ledger,
    limits: &StorageLimits,
    found: &mut BTreeMap<String, ObjectRef>,
    active: &mut BTreeSet<String>,
    depth: u64,
) -> Result<(), BundleError> {
    reference.validate()?;
    if depth > limits.graph_depth {
        return Err(BundleError::Capacity("graph depth"));
    }
    if active.contains(&reference.sha256) {
        return Err(BundleError::Corrupt("dependency cycle".into()));
    }
    if let Some(known) = found.get(&reference.sha256) {
        if known != reference {
            return Err(BundleError::Corrupt(
                "inconsistent reference identity".into(),
            ));
        }
        return Ok(());
    }
    let (known, dependencies) = ledger
        .objects
        .get(&reference.sha256)
        .ok_or_else(|| BundleError::Corrupt("missing dependency declaration".into()))?;
    if known != reference || !ledger.verified.contains(&reference.sha256) {
        return Err(BundleError::Corrupt(
            "unverified dependency identity".into(),
        ));
    }
    if found.len() as u64 >= limits.objects {
        return Err(BundleError::Capacity("closure object count"));
    }
    active.insert(reference.sha256.clone());
    for dependency in dependencies {
        walk(dependency, ledger, limits, found, active, depth + 1)?;
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
            .filter(|n| *n <= self.limit)
            .ok_or_else(|| std::io::Error::other("owned bytes exceed reservation"))?;
        Ok(count)
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

fn collect(inner: &StoreInner) -> Result<u64, BundleError> {
    let victims = {
        let ledger = inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
        if !ledger.can_collect {
            return Err(BundleError::Unsettled);
        }
        ledger
            .objects
            .iter()
            .filter(|(h, _)| ledger.pins.get(*h).copied().unwrap_or(0) == 0)
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
        inner
            .namespace
            .lock()
            .map_err(|_| BundleError::Identity("namespace poisoned".into()))?
            .unlink_owned(&path)?;
        let mut ledger = inner
            .ledger
            .lock()
            .map_err(|_| BundleError::Capacity("ledger poisoned"))?;
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
        if reference.record_kind.is_blob() {
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
        ledger.metadata_sizes.remove(&hash);
        retired = retired
            .checked_add(reference.byte_length)
            .ok_or(BundleError::Capacity("retirement overflow"))?;
    }
    Ok(retired)
}
impl Drop for StoreInner {
    fn drop(&mut self) {
        let temporary = self.session_id.is_some();
        let mut collectable = false;
        if let Ok(ledger) = self.ledger.get_mut() {
            if !ledger.pending.is_empty() {
                for reservation in ledger.pending.drain(..) {
                    reservation.retain_charge();
                }
                if let Ok(mut usage) = self.usage.lock() {
                    usage.unsettled = true;
                }
            }
            if temporary {
                ledger.can_collect = true;
            }
            collectable = ledger.can_collect;
        }
        // No StorePin exists once this final Arc is dropped. Named committed roots persist.
        let retired = if collectable { collect(self) } else { Ok(0) };
        let closed = self
            .namespace
            .get_mut()
            .map_err(|_| BundleError::Capacity("namespace poisoned"))
            .and_then(|namespace| {
                if temporary {
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

mod record_sealed {
    pub trait Sealed {}
}
/// Only production record types can use the typed-record ingress.
pub(crate) trait BundleRecord:
    record_sealed::Sealed + Serialize + serde::de::DeserializeOwned
{
    const KIND: RecordKind;
    fn dependencies(&self) -> Vec<ObjectRef> {
        vec![]
    }
    fn validate(&self, _limits: &StorageLimits) -> Result<(), BundleError> {
        Ok(())
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
            for dep in &entry.dependencies {
                dep.validate()?;
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
record!(crate::bundle_records::LineageRecord, Lineage);
impl record_sealed::Sealed for StoreManifest {}
impl BundleRecord for StoreManifest {
    const KIND: RecordKind = RecordKind::StoreManifest;
    fn dependencies(&self) -> Vec<ObjectRef> {
        self.objects.clone()
    }
}

impl ProjectStore {
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
        self.admit(&mut bytes.as_slice(), &reference, &record.dependencies())
    }
    pub(crate) fn read_record<T: BundleRecord>(
        &self,
        reference: &ObjectRef,
    ) -> Result<StorageReservation<T>, BundleError> {
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
        let _encoded = self.reserve(0, reference.byte_length, 0, false)?;
        let mut file = self.open_verified(reference)?;
        let capacity = usize::try_from(reference.byte_length)
            .map_err(|_| BundleError::Capacity("record size"))?;
        let mut bytes = Vec::with_capacity(capacity);
        (&mut file)
            .take(
                reference
                    .byte_length
                    .checked_add(1)
                    .ok_or(BundleError::Capacity("record overflow"))?,
            )
            .read_to_end(&mut bytes)?;
        if bytes.len() != capacity {
            return Err(BundleError::Corrupt("record changed while decoding".into()));
        }
        let preflight = self.reserve(0, self.limits().decoded_record_bytes, 0, false)?;
        let decoded = preflight_record(&bytes, self.limits())?;
        drop(preflight);
        if T::KIND == RecordKind::History && decoded > self.limits().branch_history_bytes {
            return Err(BundleError::Capacity("live branch history"));
        }
        let decoded_reservation = self.reserve(
            0,
            decoded,
            if T::KIND == RecordKind::History {
                decoded
            } else {
                0
            },
            false,
        )?;
        let value: T =
            serde_json::from_slice(&bytes).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        value.validate(self.limits())?;
        Ok(decoded_reservation.hold(value))
    }
}
struct BoundedCounter {
    bytes: u64,
    limit: u64,
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
fn preflight_record(bytes: &[u8], limits: &StorageLimits) -> Result<u64, BundleError> {
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
            // Counts a conservative native variant slot at every node. Measurements are separate.
            let node = std::mem::size_of::<kinewright_core::Operation>()
                .max(std::mem::size_of::<kinewright_core::HistoryBatch>())
                .max(128);
            self.charge(
                u64::try_from(node)
                    .map_err(D::Error::custom)?
                    .checked_mul(2)
                    .ok_or_else(|| D::Error::custom("native slot overflow"))?,
            )?;
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
