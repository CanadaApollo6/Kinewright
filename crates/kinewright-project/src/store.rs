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

struct Ledger {
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

struct StoreInner {
    namespace: Mutex<ProjectNamespace>,
    ledger: Mutex<Ledger>,
    usage: Arc<Mutex<StorageUsage>>,
    limits: StorageLimits,
    root_bytes: Mutex<u64>,
    inventory_bytes: Mutex<u64>,
    project_id: Option<ProjectId>,
    session_id: Option<SessionId>,
    owner_epoch: u64,
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
        let mut epoch_bytes = [0_u8; 8];
        getrandom::fill(&mut epoch_bytes).map_err(|e| BundleError::Identity(e.to_string()))?;
        let owner_epoch = u64::from_le_bytes(epoch_bytes).max(1);
        let root_bytes = namespace.read_root()?.map_or(0, |b| b.len() as u64);
        let inventory_bytes = namespace
            .read(
                &crate::bundle_recovery::inventory_path()?,
                limits.control_bytes,
            )?
            .map_or(0, |b| b.len() as u64);
        let control_bytes = root_bytes
            .checked_add(inventory_bytes)
            .ok_or(BundleError::Capacity("control files"))?;
        let can_collect = root_bytes == 0;
        Ok(Self {
            inner: Arc::new(StoreInner {
                namespace: Mutex::new(namespace),
                ledger: Mutex::new(Ledger {
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
                    .and_then(|n| n.checked_add(usage.decoded_bytes))
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
                (reference.clone(), dependencies.clone()),
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
    pub fn closure(&self, roots: &[ObjectRef]) -> Result<Vec<ObjectRef>, BundleError> {
        self.closure_with_inventory(roots, true)
    }
    fn closure_with_inventory(
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
        // Opening archived bytes does not promote their automatic inventory into live roots.
        self.closure_with_inventory(std::slice::from_ref(reference), false)?;
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
        #[cfg(any(test, feature = "test-util"))]
        crate::io_observer::check_failure("gc_before_unlink", path.as_path())?;
        inner
            .namespace
            .lock()
            .map_err(|_| BundleError::Identity("namespace poisoned".into()))?
            .unlink_owned(&path)?;
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
        inner
            .namespace
            .lock()
            .map_err(|_| BundleError::Identity("namespace poisoned".into()))?
            .retire_temporary(&file)?;
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
        let path = PortablePath::new(&Path::new("studio/objects").join(&hash))?;
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
    Ok(retired)
}
impl Drop for StoreInner {
    fn drop(&mut self) {
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
        Ok(sealed)
    }
    pub fn read_record<T: BundleRecord>(
        &self,
        reference: &ObjectRef,
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
        let _encoded = self.reserve(0, reference.byte_length, 0, false)?;
        let mut file = self.open_verified(reference)?;
        let bytes = crate::bundle_recovery::bounded_bytes(&mut file, reference.byte_length)?;
        if bytes.len() as u64 != reference.byte_length {
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
            let charge = 256_u64
                .checked_add(
                    (dependencies.len() as u64)
                        .checked_mul(160)
                        .ok_or(BundleError::Capacity("edge overflow"))?,
                )
                .and_then(|n| {
                    n.checked_add(if reference.record_kind.is_owned_bytes() {
                        0
                    } else {
                        reference.byte_length
                    })
                })
                .ok_or(BundleError::Capacity("metadata overflow"))?;
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
        self.publish_control(&bytes, false)
    }
    pub(crate) fn publish_root(&mut self, bytes: &[u8]) -> Result<(), BundleError> {
        self.publish_control(bytes, true)
    }
    fn publish_control(&mut self, bytes: &[u8], root: bool) -> Result<(), BundleError> {
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
            self.with_namespace(|n| n.replace_root(bytes))
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
                    fields.extend(cached.iter().cloned());
                    continue;
                }
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
                let _raw = crate::bundle_recovery::decode::<serde_json::Value>(
                    &bytes,
                    &self.inner.usage,
                    reference.record_kind,
                    &mut object_fields,
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
            .checked_add(prior.metadata_bytes)
            .is_none_or(|n| n > inner.limits.metadata_bytes)
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
        if reference.record_kind != RecordKind::OwnedEvidence
            || reference.byte_length > self.limits().control_bytes
        {
            return Err(BundleError::Capacity("evidence snapshot"));
        }
        let _pin = self.pin(std::slice::from_ref(reference))?;
        let charge = self.reserve(0, reference.byte_length, 0, false)?;
        let file = self.open_verified(reference)?;
        let bytes = crate::bundle_recovery::bounded_bytes(file, reference.byte_length)?;
        if bytes.len() as u64 != reference.byte_length {
            return Err(BundleError::Corrupt(
                "evidence changed while reading".into(),
            ));
        }
        Ok(charge.hold(bytes))
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
    pub(crate) fn begin_copy(
        &self,
        roots: &[ObjectRef],
        bytes: u64,
    ) -> Result<SourceCopyHold, BundleError> {
        self.with_namespace(|n| n.verify())?;
        let overlap = self.reserve_copy(bytes)?;
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
        let _working = source.reserve(0, source.usage()?.metadata_bytes, 0, false)?;
        let selected = source.closure(roots)?;
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
                let mut file = source.open_verified(&reference)?;
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
                .checked_add(usage.decoded_bytes)
                .is_none_or(|n| n > limits.metadata_bytes)
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
