//! Owner-bound byte admission and chunk-verified decoder certification.
#![allow(clippy::missing_errors_doc)]
pub use crate::certified_resources::CertifiedResource;
pub use crate::owned_media::{
    CertifiedMediaInput, INPUT_CHUNK_BYTES, InputChunkTable, MediaInterpretation,
    PreparedOwnedMedia, StreamInterpretation,
};
use crate::{
    bundle::BundleError,
    bundle_records::{ObjectRef, ProjectId, SessionId},
    store::{ProjectStore, SealedObject, StorePin},
};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputRequest {
    pub project_id: Option<ProjectId>,
    pub session_id: Option<SessionId>,
    pub owner_epoch: u64,
    pub job: String,
    pub object: ObjectRef,
    pub source_locator_hint: Option<PathBuf>,
    pub dependencies: Vec<ObjectRef>,
    pub selected_stream: Option<String>,
    pub time_base: Option<(i64, i64)>,
    pub origin: Option<i64>,
    pub profile: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedInputEntry {
    pub entry_id: String,
    pub object: ObjectRef,
    pub dependencies: Vec<ObjectRef>,
    pub selected_stream: Option<String>,
    pub time_base: Option<(i64, i64)>,
    pub origin: Option<i64>,
    pub profile: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedInputManifest {
    pub project_id: Option<ProjectId>,
    pub session_id: Option<SessionId>,
    pub owner_epoch: u64,
    pub entries: Vec<OwnedInputEntry>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedInputRef {
    pub manifest_ref: ObjectRef,
    pub entry: OwnedInputEntry,
}
/// Retains the actual store authority and physical charge. It grants no decoder certification.
pub struct OwnedInputLease {
    file: File,
    _pin: StorePin,
    identity: OwnedInputRef,
    owner_epoch: u64,
    source: Option<CertifiedMediaInput>,
    cursor: Option<kinewright_media::owned_io::VerifiedCursor>,
}
impl Read for OwnedInputLease {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.source.is_some() {
            self.verified_cursor()?
                .read(buffer)
                .map_err(std::io::Error::other)
        } else {
            self.file.read(buffer)
        }
    }
}
impl Seek for OwnedInputLease {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        if self.source.is_some() {
            self.verified_cursor()?
                .seek(position)
                .map_err(std::io::Error::other)
        } else {
            self.file.seek(position)
        }
    }
}
impl OwnedInputLease {
    #[must_use]
    pub fn identity(&self) -> &OwnedInputRef {
        &self.identity
    }
    #[must_use]
    pub const fn owner_epoch(&self) -> u64 {
        self.owner_epoch
    }

    pub(crate) fn new(
        file: File,
        pin: StorePin,
        identity: OwnedInputRef,
        owner_epoch: u64,
        source: Option<CertifiedMediaInput>,
    ) -> Self {
        Self {
            file,
            _pin: pin,
            identity,
            owner_epoch,
            source,
            cursor: None,
        }
    }
    pub fn certify_decoder_open(&self) -> Result<DecoderOpenReceipt, BundleError> {
        self.certified_source()?;
        let entry = &self.identity.entry;
        Ok(DecoderOpenReceipt {
            manifest_ref: self.identity.manifest_ref.clone(),
            entry_id: entry.entry_id.clone(),
            opened_object: entry.object.clone(),
            selected_stream: entry.selected_stream.clone(),
            time_base: entry.time_base,
            origin: entry.origin,
            profile: entry.profile.clone(),
            use_outcome: "chunk-verified-through-use".into(),
        })
    }
    pub fn certified_source(&self) -> Result<CertifiedMediaInput, BundleError> {
        self.source
            .clone()
            .ok_or_else(|| BundleError::Unsupported("input is not certified media".into()))
    }
    fn verified_cursor(
        &mut self,
    ) -> std::io::Result<&mut kinewright_media::owned_io::VerifiedCursor> {
        if self.cursor.is_none() {
            let source = self
                .source
                .as_ref()
                .ok_or_else(|| std::io::Error::other("missing media certificate"))?;
            self.cursor = Some(
                source
                    .cursor(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
                        false,
                    )))
                    .map_err(std::io::Error::other)?,
            );
        }
        self.cursor
            .as_mut()
            .ok_or_else(|| std::io::Error::other("missing verified cursor"))
    }
}
/// A receipt describes certification. Actual reads require the retained capability.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecoderOpenReceipt {
    manifest_ref: ObjectRef,
    entry_id: String,
    opened_object: ObjectRef,
    selected_stream: Option<String>,
    time_base: Option<(i64, i64)>,
    origin: Option<i64>,
    profile: Option<String>,
    use_outcome: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DerivedResult {
    pub project_id: Option<ProjectId>,
    pub session_id: Option<SessionId>,
    pub owner_epoch: u64,
    pub job: String,
    pub source: OwnedInputRef,
    pub recipe: String,
    pub intent: String,
    pub measured_bytes: u64,
    pub outputs: Vec<ObjectRef>,
    pub coverage: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuccessorEvidence {
    pub previous: OwnedInputRef,
    pub successor: OwnedInputRef,
    pub reason: String,
    pub invalidated_dependencies: Vec<ObjectRef>,
    pub expected_head: crate::bundle_records::HeadId,
    pub expected_bound_resource: ObjectRef,
}

impl ProjectStore {
    pub fn admit_input(
        &mut self,
        request: &InputRequest,
        reader: &mut impl Read,
    ) -> Result<(OwnedInputRef, SealedObject), BundleError> {
        if (request.project_id.clone(), request.session_id.clone()) != self.input_owner()
            || request.owner_epoch != self.owner_epoch()
        {
            return Err(BundleError::Identity("input owner/epoch mismatch".into()));
        }
        if request.job.is_empty() || request.job.len() > 256 {
            return Err(BundleError::Capacity("input job identity"));
        }
        let blob = self.admit_blob(reader, &request.object, &request.dependencies)?;
        let entry = OwnedInputEntry {
            entry_id: request.object.sha256.clone(),
            object: request.object.clone(),
            dependencies: request.dependencies.clone(),
            selected_stream: request.selected_stream.clone(),
            time_base: request.time_base,
            origin: request.origin,
            profile: request.profile.clone(),
        };
        let manifest = OwnedInputManifest {
            project_id: request.project_id.clone(),
            session_id: request.session_id.clone(),
            owner_epoch: request.owner_epoch,
            entries: vec![entry.clone()],
        };
        let sealed = self.admit_record(&manifest)?;
        let reference = OwnedInputRef {
            manifest_ref: sealed.reference.clone(),
            entry,
        };
        drop(blob);
        Ok((reference, sealed))
    }
    pub fn read_input_manifest(
        &self,
        reference: &ObjectRef,
    ) -> Result<crate::storage_budget::StorageReservation<OwnedInputManifest>, BundleError> {
        self.read_record(reference)
    }
    pub fn lease_input(&self, reference: &OwnedInputRef) -> Result<OwnedInputLease, BundleError> {
        self.lease_input_for_owner(reference, &self.input_owner())
    }
    /// Only a verified checkpoint preparation can admit a historical payload owner.
    pub(crate) fn lease_input_for_owner(
        &self,
        reference: &OwnedInputRef,
        owner: &(Option<ProjectId>, Option<SessionId>),
    ) -> Result<OwnedInputLease, BundleError> {
        let manifest = self.read_record::<OwnedInputManifest>(&reference.manifest_ref)?;
        if &(manifest.project_id.clone(), manifest.session_id.clone()) != owner
            || !manifest.entries.contains(&reference.entry)
        {
            return Err(BundleError::Identity(
                "input manifest binding mismatch".into(),
            ));
        }
        let pin = self.pin(std::slice::from_ref(&reference.manifest_ref))?;
        let source = if reference.entry.object.record_kind
            == crate::bundle_records::RecordKind::OwnedMedia
        {
            Some(self.certify_media_input(reference)?)
        } else {
            None
        };
        let file = if source.is_some() {
            self.open_input_descriptor(&reference.entry.object)?
        } else {
            self.open_verified(&reference.entry.object)?
        };
        Ok(OwnedInputLease::new(
            file,
            pin,
            reference.clone(),
            self.owner_epoch(),
            source,
        ))
    }
}
