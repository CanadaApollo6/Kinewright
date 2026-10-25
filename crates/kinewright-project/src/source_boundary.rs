//! A borrowed source carries identities and bounded reads, never a source owner or cleanup right.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::BundleError,
    bundle_records::{ObjectRef, ProjectId, SessionId},
    storage_budget::StorageLimits,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

pub(crate) struct RetainedBorrowedSource {
    pub(crate) descriptor: BorrowedSourceDescriptor,
    pub(crate) reads: crate::store::RetainedReadAdmission,
    pub(crate) _metadata: crate::storage_budget::StorageReservation,
    pub(crate) overlap: Mutex<BorrowedSnapshotOverlap>,
    pub(crate) accepting_reads: AtomicBool,
    pub(crate) reader: Mutex<Option<(ObjectRef, crate::store::RetainedReadLease)>>,
}

pub(crate) struct BorrowedSnapshotOverlap {
    pub(crate) reservation: crate::storage_budget::StorageReservation,
    pub(crate) snapshot: Option<BorrowedSnapshotBinding>,
}

/// Exact foreground snapshot admitted by the originating source owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BorrowedSnapshotBinding {
    pub history_sha256: String,
    pub encoded_bytes: u64,
}
impl BorrowedSnapshotBinding {
    pub fn from_history(history: &kinewright_core::HistorySnapshot) -> Result<Self, BundleError> {
        let bytes = serde_json::to_vec(history).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        Ok(Self {
            history_sha256: kinewright_media::sha256_bytes(&bytes),
            encoded_bytes: bytes.len() as u64,
        })
    }
}

/// Cloneable GUI callback endpoint. Weak access cannot outlive originating Close
/// or become an independent pin/reservation owner. Authentication is external.
#[derive(Clone)]
pub struct BorrowedSourceService {
    retained: Weak<RetainedBorrowedSource>,
}
impl BorrowedSourceService {
    /// GUI-only admission: reserve a checked delta on the actual source ledger,
    /// then commit one exact binding without replacing any existing hold or pin.
    pub fn ensure_snapshot(
        &self,
        source: &BorrowedSourceDescriptor,
        history: &kinewright_core::HistorySnapshot,
    ) -> Result<BorrowedSnapshotBinding, BundleError> {
        let retained = self
            .retained
            .upgrade()
            .filter(|r| r.accepting_reads.load(Ordering::SeqCst) && r.descriptor == *source)
            .ok_or_else(|| {
                BundleError::Identity("source retention was not re-established".into())
            })?;
        let encoded_bytes = history
            .encoded_bytes()
            .map_err(crate::bundle::history_error)?;
        let mut overlap = retained
            .overlap
            .lock()
            .map_err(|_| BundleError::Capacity("snapshot overlap poisoned"))?;
        let extension = retained
            .reads
            .reserve_snapshot_growth(&overlap.reservation, encoded_bytes)?;
        let _scratch = retained.reads.reserve_snapshot_scratch(encoded_bytes)?;
        let snapshot = BorrowedSnapshotBinding::from_history(history)?;
        if snapshot.encoded_bytes != encoded_bytes {
            return Err(BundleError::Identity(
                "snapshot encoded length changed during admission".into(),
            ));
        }
        if !retained.accepting_reads.load(Ordering::SeqCst) {
            return Err(BundleError::Identity("originating source is closed".into()));
        }
        overlap.reservation.absorb_copy_growth(extension)?;
        overlap.snapshot = Some(snapshot.clone());
        Ok(snapshot)
    }

    /// Read-only protocol proof; it cannot grow, release or replace a reservation.
    pub fn prove_snapshot(
        &self,
        source: &BorrowedSourceDescriptor,
        snapshot: &BorrowedSnapshotBinding,
    ) -> Result<(), BundleError> {
        let retained = self
            .retained
            .upgrade()
            .filter(|r| r.accepting_reads.load(Ordering::SeqCst) && r.descriptor == *source)
            .ok_or_else(|| {
                BundleError::Identity("source retention was not re-established".into())
            })?;
        let overlap = retained
            .overlap
            .lock()
            .map_err(|_| BundleError::Capacity("snapshot overlap poisoned"))?;
        if overlap.snapshot.as_ref() != Some(snapshot) {
            return Err(BundleError::Identity(
                "foreground snapshot overlap was not admitted".into(),
            ));
        }
        overlap
            .reservation
            .verify_copy_charge(snapshot.encoded_bytes)?;
        Ok(())
    }

    pub(crate) fn new(retained: &Arc<RetainedBorrowedSource>) -> Self {
        Self {
            retained: Arc::downgrade(retained),
        }
    }
    pub fn open_borrowed_source(
        &self,
        source: &BorrowedSourceDescriptor,
        object: &ObjectRef,
    ) -> Result<impl Read + Seek + use<>, BundleError> {
        let retained = self
            .retained
            .upgrade()
            .filter(|r| r.accepting_reads.load(Ordering::SeqCst) && r.descriptor == *source)
            .ok_or_else(|| {
                BundleError::Identity("source retention was not re-established".into())
            })?;
        let reader = retained.reads.open(object)?;
        Ok(BorrowedSourceLease {
            reader,
            _retained: retained,
        })
    }
    /// The GUI's actual namespace checks separation without granting any source cleanup rights.
    pub fn inspect_destination(
        &self,
        source: &BorrowedSourceDescriptor,
        path: &std::path::Path,
    ) -> Result<crate::namespace::NamespaceIdentity, BundleError> {
        let retained = self
            .retained
            .upgrade()
            .filter(|r| r.accepting_reads.load(Ordering::SeqCst) && r.descriptor == *source)
            .ok_or_else(|| {
                BundleError::Identity("source retention was not re-established".into())
            })?;
        let inspection = retained.reads.inspect_destination(path)?;
        inspection.identity()
    }
    pub fn read_borrowed_source(
        &self,
        source: &BorrowedSourceDescriptor,
        object: &ObjectRef,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<usize, BundleError> {
        if buffer.len() > SOURCE_READ_CHUNK_BYTES || offset > object.byte_length {
            return Err(BundleError::Capacity("borrowed source read range"));
        }
        let retained = self
            .retained
            .upgrade()
            .filter(|r| r.accepting_reads.load(Ordering::SeqCst) && r.descriptor == *source)
            .ok_or_else(|| {
                BundleError::Identity("source retention was not re-established".into())
            })?;
        let mut slot = retained
            .reader
            .lock()
            .map_err(|_| BundleError::Capacity("borrowed reader poisoned"))?;
        if slot
            .as_ref()
            .is_none_or(|(reference, _)| reference != object)
        {
            *slot = Some((object.clone(), retained.reads.open_lease(object)?));
        }
        let (_, reader) = slot.as_mut().ok_or(BundleError::HistoryBarrierInvariant)?;
        reader.seek(SeekFrom::Start(offset))?;
        let count = buffer
            .len()
            .min(usize::try_from(object.byte_length - offset).unwrap_or(usize::MAX));
        Ok(reader.read(&mut buffer[..count])?)
    }
}
impl BorrowedSourceReader for BorrowedSourceService {
    fn prove_snapshot(
        &mut self,
        source: &BorrowedSourceDescriptor,
        snapshot: &BorrowedSnapshotBinding,
    ) -> Result<(), BundleError> {
        Self::prove_snapshot(self, source, snapshot)
    }
    fn inspect_destination(
        &mut self,
        source: &BorrowedSourceDescriptor,
        path: &std::path::Path,
    ) -> Result<crate::namespace::NamespaceIdentity, BundleError> {
        Self::inspect_destination(self, source, path)
    }
    fn read_source(
        &mut self,
        source: &BorrowedSourceDescriptor,
        object: &ObjectRef,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<usize, BundleError> {
        self.read_borrowed_source(source, object, offset, buffer)
    }
}

pub(crate) struct BorrowedSourceLease<R> {
    pub(crate) reader: R,
    pub(crate) _retained: Arc<RetainedBorrowedSource>,
}
impl<R: Read> Read for BorrowedSourceLease<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.reader.read(buffer)
    }
}
impl<R: Seek> Seek for BorrowedSourceLease<R> {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.reader.seek(position)
    }
}

pub const SOURCE_READ_CHUNK_BYTES: usize = 65_536;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BorrowedSourceObject {
    pub(crate) reference: ObjectRef,
    pub(crate) dependencies: Vec<ObjectRef>,
    pub(crate) inventory_dependencies: Vec<ObjectRef>,
}

/// Wire data only. Authentication belongs to the attachment protocol; the originating
/// owner additionally matches this entire value to its still-live admitted closure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BorrowedSourceDescriptor {
    pub(crate) admission: SessionId,
    pub(crate) project_id: Option<ProjectId>,
    pub(crate) session_id: Option<SessionId>,
    pub(crate) owner_epoch: u64,
    pub(crate) extra_overlap_bytes: u64,
    pub(crate) roots: Vec<ObjectRef>,
    pub(crate) objects: Vec<BorrowedSourceObject>,
}

impl BorrowedSourceDescriptor {
    #[must_use]
    pub fn admission_id(&self) -> &SessionId {
        &self.admission
    }
    #[must_use]
    pub fn roots(&self) -> &[ObjectRef] {
        &self.roots
    }
    pub fn objects(&self) -> impl Iterator<Item = &ObjectRef> {
        self.objects.iter().map(|o| &o.reference)
    }
    #[must_use]
    pub fn session_id(&self) -> Option<&SessionId> {
        self.session_id.as_ref()
    }
    #[must_use]
    pub const fn owner_epoch(&self) -> u64 {
        self.owner_epoch
    }
    pub(crate) fn metadata_bytes(&self) -> Result<u64, BundleError> {
        self.objects.iter().try_fold(
            (self.roots.len() as u64)
                .checked_mul(160)
                .and_then(|n| n.checked_add(512))
                .ok_or(BundleError::Capacity("borrowed source metadata"))?,
            |total, object| {
                let edges =
                    object.dependencies.len() as u64 + object.inventory_dependencies.len() as u64;
                edges
                    .checked_mul(160)
                    .and_then(|n| n.checked_add(256))
                    .and_then(|n| n.checked_add(total))
                    .ok_or(BundleError::Capacity("borrowed source metadata"))
            },
        )
    }
    /// Validate untrusted wire metadata before opening or mutating a destination.
    pub fn validate(&self, limits: &StorageLimits) -> Result<(), BundleError> {
        limits.validate()?;
        if self.owner_epoch == 0
            || self.project_id.is_some() == self.session_id.is_some()
            || self.roots.len() as u64 > limits.objects
            || self.objects.len() as u64 > limits.objects
            || self.metadata_bytes()? > limits.metadata_bytes
        {
            return Err(BundleError::Capacity("borrowed source descriptor"));
        }
        let mut known = BTreeMap::new();
        for object in &self.objects {
            object.reference.validate()?;
            let mut depth = 1_u64;
            for dependency in &object.dependencies {
                let Some((reference, prior_depth)) = known.get(&dependency.sha256) else {
                    return Err(BundleError::Identity(
                        "borrowed graph is not ordered and closed".into(),
                    ));
                };
                if reference != dependency {
                    return Err(BundleError::Identity(
                        "borrowed dependency identity differs".into(),
                    ));
                }
                depth = depth.max(prior_depth + 1);
            }
            if depth > limits.graph_depth
                || known
                    .insert(
                        object.reference.sha256.clone(),
                        (object.reference.clone(), depth),
                    )
                    .is_some()
            {
                return Err(BundleError::Capacity("borrowed graph depth or duplicate"));
            }
        }
        for reference in self
            .roots
            .iter()
            .chain(self.objects.iter().flat_map(|o| &o.inventory_dependencies))
        {
            if known
                .get(&reference.sha256)
                .is_none_or(|(r, _)| r != reference)
            {
                return Err(BundleError::Identity(
                    "borrowed graph reference outside admission".into(),
                ));
            }
        }
        Ok(())
    }
}

/// Implemented by the authenticated host transport. A callback cannot retire the source.
/// Each invocation must return at most the supplied buffer length, from the exact object.
pub trait BorrowedSourceReader {
    fn prove_snapshot(
        &mut self,
        source: &BorrowedSourceDescriptor,
        snapshot: &BorrowedSnapshotBinding,
    ) -> Result<(), BundleError>;
    fn inspect_destination(
        &mut self,
        source: &BorrowedSourceDescriptor,
        path: &std::path::Path,
    ) -> Result<crate::namespace::NamespaceIdentity, BundleError>;
    fn read_source(
        &mut self,
        source: &BorrowedSourceDescriptor,
        object: &ObjectRef,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<usize, BundleError>;
}

pub(crate) struct BorrowedObjectReader<'a> {
    pub(crate) source: &'a BorrowedSourceDescriptor,
    pub(crate) object: &'a ObjectRef,
    pub(crate) reader: &'a mut dyn BorrowedSourceReader,
    offset: u64,
}
impl<'a> BorrowedObjectReader<'a> {
    pub(crate) fn new(
        source: &'a BorrowedSourceDescriptor,
        object: &'a ObjectRef,
        reader: &'a mut dyn BorrowedSourceReader,
    ) -> Self {
        Self {
            source,
            object,
            reader,
            offset: 0,
        }
    }
}
impl Read for BorrowedObjectReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let remaining = self.object.byte_length - self.offset;
        let count = buffer
            .len()
            .min(SOURCE_READ_CHUNK_BYTES)
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        if count == 0 {
            return Ok(0);
        }
        let read = self
            .reader
            .read_source(self.source, self.object, self.offset, &mut buffer[..count])
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        if read == 0 || read > count {
            return Err(std::io::Error::other(
                "short or oversized borrowed source response",
            ));
        }
        self.offset += read as u64;
        Ok(read)
    }
}
