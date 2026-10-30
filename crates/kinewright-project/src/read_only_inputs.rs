//! Explicit read-only admission of byte-preserved owned inputs. No namespace writes.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::BundleError,
    bundle_records::{ObjectRef, ProjectId, RecordKind, SessionId},
    namespace::FileIdentity,
    owned_inputs::{OwnedInputManifest, OwnedInputRef},
    owned_media::{CertifiedMediaInput, INPUT_CHUNK_BYTES, InputChunkTable, parse_digest},
    storage_budget::{StorageLimits, StorageReservation, StorageUsage},
    store::{BundleRecord, ProjectStore, preflight_record},
};
use kinewright_hash::Sha256;
use kinewright_media::owned_io::{CertifiedSource, CursorReservation, LifetimeGuard};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    sync::{Arc, Mutex, atomic::AtomicBool},
};

/// Expected identities supplied by the already-admitted bundle or fixture inventory.
/// These are content references, never source locator hints.
#[derive(Clone)]
pub struct ReadOnlyInputBinding {
    pub document_ref: ObjectRef,
    pub input: OwnedInputRef,
    pub project_id: Option<ProjectId>,
    pub session_id: Option<SessionId>,
    pub owner_epoch: u64,
}

/// Open owned objects. Admission verifies the bytes and retains these exact handles.
pub struct ReadOnlyInputFiles {
    pub document: File,
    pub manifest: File,
    pub media: File,
    pub table: File,
}

type UsageReader = dyn Fn() -> Result<StorageUsage, BundleError> + Send + Sync;

/// A read capability with one shared lifetime and the owner's existing budget semantics.
/// It cannot import, publish, save, collect, or grant durable namespace authority.
#[derive(Clone)]
pub struct ReadOnlyInputs {
    media: CertifiedMediaInput,
    usage: Arc<UsageReader>,
}

impl ReadOnlyInputs {
    /// Explicit admission of an already-produced fixture using its exact trusted inventory.
    /// The sole ledger here belongs to this read-only owner; live stores use the adapter below.
    pub fn admit_fixture(
        binding: &ReadOnlyInputBinding,
        files: ReadOnlyInputFiles,
        limits: &StorageLimits,
    ) -> Result<Self, BundleError> {
        limits.validate()?;
        let usage = Arc::new(Mutex::new(StorageUsage::default()));
        let metadata_usage = Arc::clone(&usage);
        let metadata_limits = limits.clone();
        let metadata_reserve: Arc<CursorReservation> = Arc::new(move |bytes| {
            StorageReservation::reserve_for_retained_read(
                &metadata_usage,
                &metadata_limits,
                0,
                bytes,
                0,
                false,
                u64::MAX,
                true,
            )
            .map(|charge| Arc::new(charge) as LifetimeGuard)
            .map_err(|error| error.to_string())
        });
        let reservation_usage = Arc::clone(&usage);
        let reservation_limits = limits.clone();
        let reserve: Arc<CursorReservation> = Arc::new(move |bytes| {
            StorageReservation::reserve_decoder(
                &reservation_usage,
                &reservation_limits,
                bytes,
                u64::MAX,
            )
            .map(|charge| Arc::new(charge) as LifetimeGuard)
            .map_err(|error| error.to_string())
        });
        let usage: Arc<UsageReader> = Arc::new(move || {
            usage
                .lock()
                .map(|value| value.clone())
                .map_err(|_| BundleError::Capacity("read-only usage poisoned"))
        });
        Self::admit(
            binding,
            files,
            limits,
            &metadata_reserve,
            reserve,
            usage,
            Arc::new(()),
        )
    }

    /// Fresh probing reuses the pinned descriptor and admitted immutable table.
    pub fn certify_media_input(&self) -> Result<CertifiedMediaInput, BundleError> {
        self.media.probe(Arc::new(AtomicBool::new(false)))?;
        Ok(self.media.clone())
    }

    pub fn usage(&self) -> Result<StorageUsage, BundleError> {
        (self.usage)()
    }

    fn admit(
        binding: &ReadOnlyInputBinding,
        files: ReadOnlyInputFiles,
        limits: &StorageLimits,
        reserve: &Arc<CursorReservation>,
        decoder_reserve: Arc<CursorReservation>,
        usage: Arc<UsageReader>,
        owner: LifetimeGuard,
    ) -> Result<Self, BundleError> {
        let admission_charge = charge(reserve, 4096)?;
        if binding.project_id.is_some() == binding.session_id.is_some() || binding.owner_epoch == 0
        {
            return Err(BundleError::Identity("read-only input owner".into()));
        }
        let mut document = PinnedObject::new(files.document, &binding.document_ref)?;
        let mut manifest = PinnedObject::new(files.manifest, &binding.input.manifest_ref)?;
        let mut media = PinnedObject::new(files.media, &binding.input.entry.object)?;
        let mut table = PinnedObject::new(files.table, binding.input.entry.chunk_table_ref()?)?;
        if media.reference.record_kind != RecordKind::OwnedMedia {
            return Err(BundleError::Identity("read-only source kind".into()));
        }
        let (manifest_value, manifest_charge) =
            read_record::<OwnedInputManifest>(&mut manifest, limits, reserve)?;
        if manifest_value.project_id != binding.project_id
            || manifest_value.session_id != binding.session_id
            || manifest_value.owner_epoch != binding.owner_epoch
            || !manifest_value.entries.contains(&binding.input.entry)
        {
            return Err(BundleError::Identity("read-only manifest binding".into()));
        }
        let (document_value, document_charge) =
            read_record::<kinewright_core::Document>(&mut document, limits, reserve)?;
        crate::bundle_commit::validate_document_inputs(&document_value, &manifest_value)?;
        if !document_value
            .media_pool
            .iter()
            .any(|asset| binding.input.entry.matches_media_asset(asset))
        {
            return Err(BundleError::Identity(
                "read-only document input binding".into(),
            ));
        }
        let (table_value, table_charge) =
            read_record::<InputChunkTable>(&mut table, limits, reserve)?;
        table_value.validate_binding(&media.reference, limits)?;
        InputChunkTable::checked_count(media.reference.byte_length, limits)?;
        media.verify_digest(reserve)?;
        let chunks_charge = charge(
            reserve,
            (table_value.chunk_sha256.len() as u64)
                .checked_mul(32)
                .ok_or(BundleError::Capacity("read-only chunk allocation"))?,
        )?;
        let chunks: Arc<[[u8; 32]]> = Arc::from(table_value.chunk_sha256.as_slice());
        let interpretation = binding.input.entry.media_interpretation()?;
        let source_digest = parse_digest(&media.reference.sha256)?;
        let source_length = media.reference.byte_length;
        let table_digest = parse_digest(&table.reference.sha256)?;
        let file = media.clone_checked()?;
        let lifetime: LifetimeGuard = Arc::new((
            owner,
            document,
            manifest,
            media,
            table,
            document_value,
            manifest_value,
            table_value,
            document_charge,
            manifest_charge,
            table_charge,
            chunks_charge,
            admission_charge,
        ));
        let source = CertifiedSource::verify(
            file,
            source_digest,
            source_length,
            table_digest,
            chunks,
            lifetime,
            decoder_reserve,
        )
        .map_err(|error| BundleError::Corrupt(error.to_string()))?;
        Ok(Self {
            media: CertifiedMediaInput::from_verified_source(source, interpretation)?,
            usage,
        })
    }
}

impl ProjectStore {
    /// A live owner lends only its own pinned objects and charges its existing ledger.
    pub fn admit_read_only_inputs(
        &self,
        binding: &ReadOnlyInputBinding,
        files: ReadOnlyInputFiles,
    ) -> Result<ReadOnlyInputs, BundleError> {
        if self.input_owner() != (binding.project_id.clone(), binding.session_id.clone()) {
            return Err(BundleError::Identity("read-only store owner".into()));
        }
        let roots = [
            binding.document_ref.clone(),
            binding.input.manifest_ref.clone(),
        ];
        let pin = self.pin(&roots)?;
        for (reference, file) in [
            (&binding.document_ref, &files.document),
            (&binding.input.manifest_ref, &files.manifest),
            (&binding.input.entry.object, &files.media),
            (binding.input.entry.chunk_table_ref()?, &files.table),
        ] {
            let owned = self.open_input_descriptor(reference)?;
            if FileIdentity::of_file(&owned)? != FileIdentity::of_file(file)? {
                return Err(BundleError::Identity(
                    "read-only owned handle mismatch".into(),
                ));
            }
        }
        ReadOnlyInputs::admit(
            binding,
            files,
            self.limits(),
            &self.metadata_reserver(),
            self.decoder_reserver(),
            Arc::new(self.usage_handle()),
            Arc::new(pin),
        )
    }
}

struct PinnedObject {
    file: File,
    reference: ObjectRef,
    identity: FileIdentity,
}

impl PinnedObject {
    fn new(file: File, reference: &ObjectRef) -> Result<Self, BundleError> {
        reference.validate()?;
        let object = Self {
            identity: FileIdentity::of_file(&file)?,
            file,
            reference: reference.clone(),
        };
        object.check()?;
        Ok(object)
    }

    fn check(&self) -> Result<(), BundleError> {
        let metadata = self.file.metadata()?;
        if !metadata.is_file()
            || metadata.len() != self.reference.byte_length
            || FileIdentity::of_file(&self.file)? != self.identity
        {
            return Err(BundleError::Identity(
                "read-only pinned object changed".into(),
            ));
        }
        Ok(())
    }

    fn clone_checked(&self) -> Result<File, BundleError> {
        self.check()?;
        let file = self.file.try_clone()?;
        if FileIdentity::of_file(&file)? != self.identity {
            return Err(BundleError::Identity(
                "read-only cloned handle changed".into(),
            ));
        }
        Ok(file)
    }

    fn verify_digest(&mut self, reserve: &Arc<CursorReservation>) -> Result<(), BundleError> {
        self.check()?;
        let _scratch = charge(reserve, INPUT_CHUNK_BYTES)?;
        let size = usize::try_from(INPUT_CHUNK_BYTES)
            .map_err(|_| BundleError::Capacity("read-only scratch address space"))?;
        let mut buffer = vec![0; size];
        self.file.seek(SeekFrom::Start(0))?;
        let mut hash = Sha256::new();
        let mut remaining = self.reference.byte_length;
        while remaining != 0 {
            let length = usize::try_from(remaining.min(INPUT_CHUNK_BYTES))
                .map_err(|_| BundleError::Capacity("read-only hash chunk"))?;
            self.file.read_exact(&mut buffer[..length])?;
            hash.update(&buffer[..length]);
            remaining -= length as u64;
        }
        self.check()?;
        if hash.finalize() != parse_digest(&self.reference.sha256)? {
            return Err(BundleError::Corrupt("read-only source digest".into()));
        }
        Ok(())
    }
}

fn charge(reserve: &Arc<CursorReservation>, bytes: u64) -> Result<LifetimeGuard, BundleError> {
    reserve(bytes).map_err(|error| BundleError::Unsupported(format!("read-only budget: {error}")))
}

fn read_record<T: BundleRecord>(
    object: &mut PinnedObject,
    limits: &StorageLimits,
    reserve: &Arc<CursorReservation>,
) -> Result<(T, LifetimeGuard), BundleError> {
    if object.reference.record_kind != T::KIND {
        return Err(BundleError::Identity("read-only typed record kind".into()));
    }
    if object.reference.byte_length > limits.decoded_record_bytes {
        return Err(BundleError::Capacity("read-only encoded record"));
    }
    object.check()?;
    let _encoded = charge(reserve, object.reference.byte_length)?;
    let size = usize::try_from(object.reference.byte_length)
        .map_err(|_| BundleError::Capacity("read-only record address space"))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(size)
        .map_err(|_| BundleError::Capacity("read-only encoded allocation"))?;
    bytes.resize(size, 0);
    object.file.seek(SeekFrom::Start(0))?;
    object.file.read_exact(&mut bytes)?;
    object.check()?;
    let mut hash = Sha256::new();
    hash.update(&bytes);
    if hash.finalize() != parse_digest(&object.reference.sha256)? {
        return Err(BundleError::Corrupt("read-only record digest".into()));
    }
    let preflight = charge(reserve, limits.decoded_record_bytes)?;
    let decoded = preflight_record(&bytes, limits)?;
    drop(preflight);
    let decoded = charge(reserve, decoded)?;
    let value: T =
        serde_json::from_slice(&bytes).map_err(|error| BundleError::Corrupt(error.to_string()))?;
    value.validate(limits)?;
    let _canonical = charge(reserve, object.reference.byte_length)?;
    if serde_json::to_vec(&value).map_err(|error| BundleError::Corrupt(error.to_string()))? != bytes
    {
        return Err(BundleError::Corrupt("read-only noncanonical record".into()));
    }
    Ok((value, decoded))
}
