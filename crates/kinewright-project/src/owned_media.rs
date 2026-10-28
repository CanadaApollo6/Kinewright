//! Canonical source identity and bounded descriptor certification for production media.
#![allow(clippy::missing_errors_doc)]
use crate::{
    bundle::BundleError,
    bundle_records::{ObjectRef, RecordKind},
    owned_inputs::{OwnedInputEntry, OwnedInputManifest, OwnedInputRef},
    storage_budget::{StorageLimits, StorageReservation},
    store::{BundleRecord, ProjectStore, SealedObject, StorePin},
};
use kinewright_core::{AssetId, ColorDescription, MediaAsset, MediaKind, Rational, TimeCode};
use kinewright_hash::Sha256;
use kinewright_media::owned_io::{CertifiedSource, CertifiedStream};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

pub const INPUT_CHUNK_BYTES: u64 = 1_048_576;
const MAX_ENCODED_CHUNK_DIGEST_BYTES: u64 = 129;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputChunkTable {
    pub version: u32,
    pub algorithm: String,
    pub chunk_size: u64,
    pub source_sha256: String,
    pub source_length: u64,
    pub chunk_sha256: Vec<[u8; 32]>,
}
impl InputChunkTable {
    pub(crate) fn checked_count(length: u64, limits: &StorageLimits) -> Result<u64, BundleError> {
        let count = length
            .checked_add(INPUT_CHUNK_BYTES - 1)
            .ok_or(BundleError::Capacity("chunk count overflow"))?
            / INPUT_CHUNK_BYTES;
        let encoded = count
            .checked_mul(MAX_ENCODED_CHUNK_DIGEST_BYTES)
            .and_then(|n| n.checked_add(1024))
            .ok_or(BundleError::Capacity("chunk table overflow"))?;
        let decoded = count
            .checked_mul(32 * 32 + 128)
            .and_then(|n| n.checked_add(4096))
            .ok_or(BundleError::Capacity("chunk metadata overflow"))?;
        if length == 0
            || length > limits.owned_bytes
            || encoded > limits.decoded_record_bytes
            || decoded > limits.decoded_record_bytes
            || usize::try_from(count).is_err()
        {
            return Err(BundleError::Capacity("chunk table bounds"));
        }
        Ok(count)
    }
    pub fn validate_binding(
        &self,
        source: &ObjectRef,
        limits: &StorageLimits,
    ) -> Result<(), BundleError> {
        self.validate(limits)?;
        if source.record_kind != RecordKind::OwnedMedia
            || self.source_sha256 != source.sha256
            || self.source_length != source.byte_length
        {
            return Err(BundleError::Corrupt("chunk table source binding".into()));
        }
        Ok(())
    }
}
impl crate::store::record_sealed::Sealed for InputChunkTable {}
impl BundleRecord for InputChunkTable {
    const KIND: RecordKind = RecordKind::InputChunkTable;
    fn validate(&self, limits: &StorageLimits) -> Result<(), BundleError> {
        if self.version != 1
            || self.algorithm != "sha256"
            || self.chunk_size != INPUT_CHUNK_BYTES
            || self.chunk_sha256.len() as u64 != Self::checked_count(self.source_length, limits)?
        {
            return Err(BundleError::Corrupt(
                "unsupported or malformed chunk table".into(),
            ));
        }
        parse_digest(&self.source_sha256)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamInterpretation {
    pub index: u32,
    pub time_base: (i64, i64),
    pub timestamp_origin: i64,
}
impl StreamInterpretation {
    fn from_probe(stream: &CertifiedStream) -> Result<Self, BundleError> {
        let divisor = gcd(i64::from(stream.time_base.0), i64::from(stream.time_base.1));
        if divisor <= 0 {
            return Err(BundleError::Corrupt("invalid stream timebase".into()));
        }
        let result = Self {
            index: u32::try_from(stream.index)
                .map_err(|_| BundleError::Capacity("stream index"))?,
            time_base: (
                i64::from(stream.time_base.0) / divisor,
                i64::from(stream.time_base.1) / divisor,
            ),
            timestamp_origin: stream.timestamp_origin,
        };
        result.validate()?;
        Ok(result)
    }
    fn validate(&self) -> Result<(), BundleError> {
        if self.time_base.0 <= 0
            || self.time_base.1 <= 0
            || gcd(self.time_base.0, self.time_base.1) != 1
        {
            return Err(BundleError::Corrupt("noncanonical stream timebase".into()));
        }
        Ok(())
    }
}
fn gcd(mut a: i64, mut b: i64) -> i64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaInterpretation {
    pub version: u32,
    pub video: Option<StreamInterpretation>,
    pub audio: Option<StreamInterpretation>,
    pub fps: Rational,
    pub duration: TimeCode,
    pub kind: MediaKind,
    pub resolution: Option<(u32, u32)>,
    pub effective_color: ColorDescription,
    pub probed_color: ColorDescription,
    pub source_profile_assumption: Option<kinewright_core::ColorSourceProfileAssumption>,
}
impl MediaInterpretation {
    fn validate(&self) -> Result<(), BundleError> {
        let video = matches!(
            self.kind,
            MediaKind::Video | MediaKind::AudioVideo | MediaKind::Image
        );
        let audio = matches!(self.kind, MediaKind::Audio | MediaKind::AudioVideo);
        if self.version != 1
            || video != self.video.is_some()
            || audio != self.audio.is_some()
            || self
                .video
                .as_ref()
                .zip(self.audio.as_ref())
                .is_some_and(|(v, a)| v.index == a.index)
            || self.duration.0 <= 0
            || self.fps.numerator() == 0
            || self.fps.denominator() == 0
        {
            return Err(BundleError::Corrupt("media interpretation".into()));
        }
        for stream in self.video.iter().chain(&self.audio) {
            stream.validate()?;
        }
        let expected_assumption = if video {
            kinewright_media::source_profile_assumption(&self.effective_color)
        } else {
            None
        };
        if self.source_profile_assumption != expected_assumption {
            return Err(BundleError::Corrupt(
                "noncanonical source profile assumption".into(),
            ));
        }
        if video {
            kinewright_core::classify_source_with_assumption(
                &self.effective_color,
                self.source_profile_assumption,
            )
            .map_err(|error| {
                BundleError::Unsupported(format!("unusable video colour interpretation: {error:?}"))
            })?;
        }
        Ok(())
    }
    #[must_use]
    pub fn matches_asset(&self, asset: &MediaAsset) -> bool {
        self.fps == asset.fps
            && self.duration == asset.duration
            && self.kind == asset.kind
            && self.resolution == asset.resolution
            && self.effective_color == asset.color_description
    }
    fn matches_probe(
        &self,
        probe: &kinewright_media::owned_io::CertifiedProbe,
    ) -> Result<bool, BundleError> {
        let selected = |index: Option<usize>| -> Result<Option<StreamInterpretation>, BundleError> {
            index
                .map(|index| {
                    probe
                        .streams
                        .iter()
                        .find(|stream| stream.index == index)
                        .ok_or_else(|| BundleError::Corrupt("missing selected stream".into()))
                        .and_then(StreamInterpretation::from_probe)
                })
                .transpose()
        };
        let explicit_override = matches!(
            self.effective_color.provenance,
            kinewright_core::ColorProvenance::UserOverride
                | kinewright_core::ColorProvenance::AgentAssumption
        );
        Ok(self.video == selected(probe.video_stream)?
            && self.audio == selected(probe.audio_stream)?
            && self.fps == probe.fps
            && self.duration == probe.duration
            && self.kind == probe.kind
            && self.resolution == probe.resolution
            && self.probed_color == probe.color_description
            && (self.effective_color == probe.color_description || explicit_override))
    }
    pub fn canonical(&self) -> Result<String, BundleError> {
        self.validate()?;
        serde_json::to_string(self).map_err(|e| BundleError::Corrupt(e.to_string()))
    }
}
impl OwnedInputEntry {
    pub fn media_interpretation(&self) -> Result<MediaInterpretation, BundleError> {
        if self.object.record_kind != RecordKind::OwnedMedia
            || self.selected_stream.is_some()
            || self.time_base.is_some()
            || self.origin.is_some()
        {
            return Err(BundleError::Corrupt(
                "conflicting media interpretation".into(),
            ));
        }
        let encoded = self
            .profile
            .as_deref()
            .ok_or_else(|| BundleError::Corrupt("missing media interpretation".into()))?;
        if encoded.len() > 8192 {
            return Err(BundleError::Capacity("media interpretation bytes"));
        }
        let result: MediaInterpretation =
            serde_json::from_str(encoded).map_err(|e| BundleError::Corrupt(e.to_string()))?;
        if result.canonical()? != encoded {
            return Err(BundleError::Corrupt(
                "noncanonical media interpretation".into(),
            ));
        }
        Ok(result)
    }
    pub fn chunk_table_ref(&self) -> Result<&ObjectRef, BundleError> {
        let mut tables = self
            .dependencies
            .iter()
            .filter(|r| r.record_kind == RecordKind::InputChunkTable);
        let table = tables
            .next()
            .ok_or_else(|| BundleError::Corrupt("missing chunk table".into()))?;
        if tables.next().is_some() {
            return Err(BundleError::Corrupt("ambiguous chunk table".into()));
        }
        Ok(table)
    }
    #[must_use]
    pub fn matches_media_asset(&self, asset: &MediaAsset) -> bool {
        let Ok(interpretation) = self.media_interpretation() else {
            return false;
        };
        self.object.record_kind == RecordKind::OwnedMedia
            && asset.source_fingerprint.content_sha256.as_deref()
                == Some(self.object.sha256.as_str())
            && asset.source_fingerprint.byte_len == Some(self.object.byte_length)
            && interpretation.matches_asset(asset)
            && self.entry_id
                == media_entry_id(
                    asset.id,
                    &self.object,
                    self.profile.as_deref().unwrap_or_default(),
                )
            && self.chunk_table_ref().is_ok()
    }
}
fn media_entry_id(id: AssetId, source: &ObjectRef, interpretation: &str) -> String {
    format!(
        "media:{}:{}:{}:{}",
        id.0,
        source.sha256,
        source.byte_length,
        kinewright_media::sha256_bytes(interpretation.as_bytes())
    )
}

pub(crate) fn stream_media(
    reader: &mut impl Read,
    output: &mut File,
    length: u64,
    count: u64,
) -> Result<InputChunkTable, BundleError> {
    let mut chunks = Vec::new();
    chunks
        .try_reserve_exact(
            usize::try_from(count).map_err(|_| BundleError::Capacity("chunk address space"))?,
        )
        .map_err(|_| BundleError::Capacity("chunk allocation"))?;
    let mut buffer = vec![0; usize::try_from(INPUT_CHUNK_BYTES).expect("bounded chunk size")];
    let mut full = Sha256::new();
    let mut remaining = length;
    while remaining != 0 {
        let size = usize::try_from(remaining.min(INPUT_CHUNK_BYTES))
            .map_err(|_| BundleError::Capacity("chunk size"))?;
        reader.read_exact(&mut buffer[..size])?;
        output.write_all(&buffer[..size])?;
        #[cfg(any(test, feature = "test-util"))]
        crate::io_observer::at_test_hook("import_chunk_streamed");
        full.update(&buffer[..size]);
        let mut chunk = Sha256::new();
        chunk.update(&buffer[..size]);
        chunks.push(chunk.finalize());
        remaining -= size as u64;
    }
    let mut extra = [0];
    if reader.read(&mut extra)? != 0 {
        return Err(BundleError::Corrupt("source grew during import".into()));
    }
    Ok(InputChunkTable {
        version: 1,
        algorithm: "sha256".into(),
        chunk_size: INPUT_CHUNK_BYTES,
        source_sha256: hex_digest(full.finalize()),
        source_length: length,
        chunk_sha256: chunks,
    })
}
pub(crate) fn hex_digest(bytes: [u8; 32]) -> String {
    use std::fmt::Write as _;
    let mut result = String::with_capacity(64);
    for byte in bytes {
        let _ = write!(result, "{byte:02x}");
    }
    result
}
pub(crate) fn parse_digest(value: &str) -> Result<[u8; 32], BundleError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(BundleError::Corrupt("noncanonical SHA-256".into()));
    }
    let mut result = [0; 32];
    for (index, slot) in result.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|e| BundleError::Corrupt(e.to_string()))?;
    }
    Ok(result)
}

pub(crate) struct CertifiedChunkTable {
    pub(crate) chunks: Arc<[[u8; 32]]>,
    pub(crate) table: StorageReservation<InputChunkTable>,
    pub(crate) _copy_charge: StorageReservation,
    pub(crate) _pin: StorePin,
}

/// Sealed staging keeps all bytes available until publication or explicit cancellation.
pub struct PreparedOwnedMedia {
    pub asset: MediaAsset,
    pub inputs_ref: ObjectRef,
    pub manifest: OwnedInputManifest,
    pub(crate) _pins: Vec<SealedObject>,
    pub(crate) _charge: StorageReservation,
}

/// Decoder access is narrowed to the admitted stream interpretation.
#[derive(Clone)]
pub struct CertifiedMediaInput {
    source: CertifiedSource,
    interpretation: MediaInterpretation,
}
impl CertifiedMediaInput {
    pub(crate) fn playback_source(&self) -> CertifiedSource {
        self.source.clone()
    }
    fn from_admitted_source(
        source: CertifiedSource,
        interpretation: MediaInterpretation,
    ) -> Result<Self, BundleError> {
        let source = source
            .with_selected_streams(
                interpretation
                    .video
                    .as_ref()
                    .map(|stream| stream.index as usize),
                interpretation
                    .audio
                    .as_ref()
                    .map(|stream| stream.index as usize),
            )
            .map_err(io_error)?;
        Ok(Self {
            source,
            interpretation,
        })
    }
    pub(crate) fn from_verified_source(
        source: CertifiedSource,
        interpretation: MediaInterpretation,
    ) -> Result<Self, BundleError> {
        let probe = source
            .probe(Arc::new(AtomicBool::new(false)))
            .map_err(io_error)?;
        if !interpretation.matches_probe(&probe)? {
            return Err(BundleError::Corrupt("probe/interpretation conflict".into()));
        }
        let source = source
            .with_selected_streams(probe.video_stream, probe.audio_stream)
            .map_err(io_error)?;
        Ok(Self {
            source,
            interpretation,
        })
    }
    #[must_use]
    pub fn interpretation(&self) -> &MediaInterpretation {
        &self.interpretation
    }
    #[must_use]
    pub fn identity(&self) -> kinewright_media::owned_io::CertifiedIdentity {
        self.source.identity()
    }
    pub fn cursor(
        &self,
        cancel: Arc<AtomicBool>,
    ) -> Result<kinewright_media::owned_io::VerifiedCursor, kinewright_media::owned_io::OwnedIoError>
    {
        self.source.cursor(cancel)
    }
    pub fn probe(
        &self,
        cancel: Arc<AtomicBool>,
    ) -> Result<kinewright_media::owned_io::CertifiedProbe, BundleError> {
        let result = self.source.probe(cancel).map_err(io_error)?;
        if !self.interpretation.matches_probe(&result)? {
            return Err(BundleError::Corrupt("probe/interpretation conflict".into()));
        }
        Ok(result)
    }
    pub fn open_video(
        &self,
        cancel: Arc<AtomicBool>,
    ) -> Result<kinewright_media::owned_io::CertifiedVideoDecoder, BundleError> {
        let selected = self
            .interpretation
            .video
            .as_ref()
            .ok_or_else(|| BundleError::Unsupported("no admitted video stream".into()))?;
        self.source
            .open_video(selected.index as usize, cancel)
            .map_err(io_error)
    }
    pub fn open_audio(
        &self,
        cancel: Arc<AtomicBool>,
    ) -> Result<kinewright_media::owned_io::CertifiedAudioDecoder, BundleError> {
        let selected = self
            .interpretation
            .audio
            .as_ref()
            .ok_or_else(|| BundleError::Unsupported("no admitted audio stream".into()))?;
        self.source
            .open_audio(selected.index as usize, cancel)
            .map_err(io_error)
    }
}
impl ProjectStore {
    /// Admit another interpretation of the same recording without replacing undo's entries.
    pub fn prepare_media_interpretation(
        &mut self,
        previous: &ObjectRef,
        asset: &MediaAsset,
    ) -> Result<PreparedOwnedMedia, BundleError> {
        let prior = self.read_input_manifest(previous)?;
        let charge = self.reserve(
            0,
            prior
                .decoded_charge()
                .checked_add(16_384)
                .ok_or(BundleError::Capacity("interpretation manifest charge"))?,
            0,
            false,
        )?;
        if (prior.project_id.clone(), prior.session_id.clone()) != self.input_owner() {
            return Err(BundleError::Identity(
                "interpretation manifest owner".into(),
            ));
        }
        let mut manifest = (*prior).clone();
        drop(prior);
        if !manifest
            .entries
            .iter()
            .any(|entry| entry.matches_media_asset(asset))
        {
            let original = manifest.entries.iter().find(|entry| {
                let Ok(interpretation) = entry.media_interpretation() else { return false; };
                let mut original_asset = asset.clone();
                original_asset.color_description = interpretation.effective_color;
                entry.matches_media_asset(&original_asset)
            }).ok_or_else(|| BundleError::Identity("interpretation requires the admitted source fingerprint and technical identity".into()))?;
            let mut interpretation = original.media_interpretation()?;
            if asset.color_description != interpretation.probed_color
                && !matches!(
                    asset.color_description.provenance,
                    kinewright_core::ColorProvenance::UserOverride
                        | kinewright_core::ColorProvenance::AgentAssumption
                )
            {
                return Err(BundleError::Identity(
                    "interpretation requires explicit colour provenance".into(),
                ));
            }
            interpretation
                .effective_color
                .clone_from(&asset.color_description);
            interpretation.source_profile_assumption = if interpretation.video.is_some() {
                kinewright_media::source_profile_assumption(&asset.color_description)
            } else {
                None
            };
            let profile = interpretation.canonical()?;
            let mut entry = original.clone();
            entry.entry_id = media_entry_id(asset.id, &entry.object, &profile);
            entry.profile = Some(profile);
            manifest.entries.push(entry);
        }
        manifest.entries.sort_by(|a, b| a.entry_id.cmp(&b.entry_id));
        let sealed = self.admit_record(&manifest)?;
        Ok(PreparedOwnedMedia {
            asset: asset.clone(),
            inputs_ref: sealed.reference.clone(),
            manifest,
            _pins: vec![sealed],
            _charge: charge,
        })
    }

    pub fn admit_chunk_table(
        &mut self,
        table: &InputChunkTable,
    ) -> Result<SealedObject, BundleError> {
        self.admit_record(table)
    }
    pub fn import_media(
        &mut self,
        reader: &mut impl Read,
        length: u64,
        id: AssetId,
        locator: &Path,
        effective_color: Option<ColorDescription>,
    ) -> Result<(MediaAsset, OwnedInputEntry, Vec<SealedObject>), BundleError> {
        let (source, table) = self.admit_measured_media(reader, length)?;
        let sealed_table = self.admit_record(&table)?;
        let certificate =
            self.certify_media_parts(&source.reference, &sealed_table.reference, None)?;
        let cancel = Arc::new(AtomicBool::new(false));
        let probe = certificate.probe(Arc::clone(&cancel)).map_err(io_error)?;
        let probed_color = probe.color_description.clone();
        let asset = MediaAsset {
            id,
            path: locator.to_path_buf(),
            name: locator
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("media")
                .into(),
            duration: probe.duration,
            fps: probe.fps,
            kind: probe.kind,
            resolution: probe.resolution,
            source_fingerprint: kinewright_core::MediaSourceFingerprint {
                content_sha256: Some(source.reference.sha256.clone()),
                byte_len: Some(length),
            },
            color_description: effective_color.unwrap_or(probe.color_description),
            assumed_from: None,
        };
        let video = probe
            .video_stream
            .map(|index| {
                probe
                    .streams
                    .iter()
                    .find(|s| s.index == index)
                    .ok_or_else(|| BundleError::Corrupt("missing selected video stream".into()))
                    .and_then(StreamInterpretation::from_probe)
            })
            .transpose()?;
        let audio = probe
            .audio_stream
            .map(|index| {
                probe
                    .streams
                    .iter()
                    .find(|s| s.index == index)
                    .ok_or_else(|| BundleError::Corrupt("missing selected audio stream".into()))
                    .and_then(StreamInterpretation::from_probe)
            })
            .transpose()?;
        let interpretation = MediaInterpretation {
            version: 1,
            video,
            audio,
            fps: asset.fps,
            duration: asset.duration,
            kind: asset.kind,
            resolution: asset.resolution,
            effective_color: asset.color_description.clone(),
            probed_color,
            source_profile_assumption: kinewright_media::source_profile_assumption(
                &asset.color_description,
            ),
        };
        let profile = interpretation.canonical()?;
        let entry = OwnedInputEntry {
            entry_id: media_entry_id(id, &source.reference, &profile),
            object: source.reference.clone(),
            dependencies: vec![sealed_table.reference.clone()],
            selected_stream: None,
            time_base: None,
            origin: None,
            profile: Some(profile),
        };
        self.remember_media_admission(&entry)?;
        Ok((asset, entry, vec![source, sealed_table]))
    }
    pub fn certify_media_input(
        &self,
        reference: &OwnedInputRef,
    ) -> Result<CertifiedMediaInput, BundleError> {
        let manifest = self.read_input_manifest(&reference.manifest_ref)?;
        if !manifest.entries.contains(&reference.entry) {
            return Err(BundleError::Identity("media manifest binding".into()));
        }
        self.certify_media_entry_in_context(&reference.entry, Some(&reference.manifest_ref))
    }
    pub(crate) fn certify_media_entry(
        &self,
        entry: &OwnedInputEntry,
    ) -> Result<CertifiedMediaInput, BundleError> {
        self.certify_media_entry_in_context(entry, None)
    }
    fn certify_media_entry_in_context(
        &self,
        entry: &OwnedInputEntry,
        context: Option<&ObjectRef>,
    ) -> Result<CertifiedMediaInput, BundleError> {
        let interpretation = entry.media_interpretation()?;
        let source = self.certify_media_parts(&entry.object, entry.chunk_table_ref()?, context)?;
        if self.media_admission_verified(entry)? {
            return CertifiedMediaInput::from_admitted_source(source, interpretation);
        }
        let certificate = CertifiedMediaInput::from_verified_source(source, interpretation)?;
        self.remember_media_admission(entry)?;
        Ok(certificate)
    }
    fn certify_media_parts(
        &self,
        source: &ObjectRef,
        table_ref: &ObjectRef,
        context: Option<&ObjectRef>,
    ) -> Result<CertifiedSource, BundleError> {
        let table = self.shared_chunk_table(table_ref)?;
        table.table.validate_binding(source, self.limits())?;
        let file = self.open_input_descriptor(source)?;
        let mut roots = vec![source.clone(), table_ref.clone()];
        roots.extend(context.cloned());
        let pin = self.pin(&roots)?;
        CertifiedSource::verify(
            file,
            parse_digest(&source.sha256)?,
            source.byte_length,
            parse_digest(&table_ref.sha256)?,
            Arc::clone(&table.chunks),
            Arc::new((pin, table)),
            self.decoder_reserver(),
        )
        .map_err(io_error)
    }
}
#[allow(
    clippy::needless_pass_by_value,
    reason = "map_err consumes the owned decoder error"
)]
fn io_error(error: kinewright_media::owned_io::OwnedIoError) -> BundleError {
    BundleError::Corrupt(error.to_string())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::bundle_copy::SessionStore;
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    use std::io::Cursor;
    fn session(label: &str) -> (TempDirectory, SessionStore) {
        let root = TempDirectory::new_in(&workspace_target_dir(), label);
        let owner = SessionStore::create(root.root(), StorageLimits::default()).unwrap();
        (root, owner)
    }
    fn wav(sample: i16) -> Vec<u8> {
        let length = 9600_u32;
        let mut out = Vec::new();
        out.extend(b"RIFF");
        out.extend((length + 36).to_le_bytes());
        out.extend(b"WAVEfmt ");
        out.extend(16_u32.to_le_bytes());
        out.extend(1_u16.to_le_bytes());
        out.extend(1_u16.to_le_bytes());
        out.extend(48000_u32.to_le_bytes());
        out.extend(96000_u32.to_le_bytes());
        out.extend(2_u16.to_le_bytes());
        out.extend(16_u16.to_le_bytes());
        out.extend(b"data");
        out.extend(length.to_le_bytes());
        for _ in 0..length / 2 {
            out.extend(sample.to_le_bytes());
        }
        out
    }
    fn import(
        owner: &mut SessionStore,
        id: u64,
        sample: i16,
    ) -> (MediaAsset, OwnedInputEntry, Vec<SealedObject>) {
        let bytes = wav(sample);
        owner
            .store_mut()
            .import_media(
                &mut bytes.as_slice(),
                bytes.len() as u64,
                AssetId(id),
                Path::new("provenance-only.wav"),
                None,
            )
            .unwrap()
    }
    fn manifest(owner: &SessionStore, entries: Vec<OwnedInputEntry>) -> OwnedInputManifest {
        OwnedInputManifest {
            project_id: None,
            session_id: Some(owner.session_id().clone()),
            owner_epoch: owner.store().owner_epoch(),
            entries,
        }
    }

    #[test]
    fn s3_fix1_reused_crash_residue_keeps_bytes_and_one_charge() {
        use crate::bundle::{BundleWriteAdmission, LoadedBundle};
        use crate::editing_session::Ja1EditingSession;
        use kinewright_core::{Core, Document};
        const CHILD_ROOT: &str = "KINEWRIGHT_C0_FIX1_CRASH_ROOT";
        let bytes = wav(5432);
        if let Some(path) = std::env::var_os(CHILD_ROOT) {
            let mut owner = LoadedBundle::read(Path::new(&path))
                .unwrap()
                .into_writer()
                .unwrap();
            owner
                .store_mut()
                .unwrap()
                .admit_measured_media(&mut bytes.as_slice(), bytes.len() as u64)
                .unwrap();
            std::process::exit(0);
        }
        let temp = TempDirectory::new_in(&workspace_target_dir(), "c0-fix1-crash-residue");
        let path = temp.path("crash.kinewright");
        let core = Core::spawn(Document::default()).unwrap();
        let mut initial =
            Ja1EditingSession::activate(&path, BundleWriteAdmission::new_document(), &core, None)
                .unwrap();
        initial.save_active().unwrap();
        drop(initial);
        // Capture the child's output: an inherited "running 1 test" line would corrupt the
        // parent's libtest stream, which CI's slow-test accounting parses.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "owned_media::tests::s3_fix1_reused_crash_residue_keeps_bytes_and_one_charge",
                "--test-threads=1",
            ])
            .env(CHILD_ROOT, &path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "crash child failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let mut owner = LoadedBundle::read(&path).unwrap().into_writer().unwrap();
        let before = owner.store().usage().unwrap();
        assert_eq!(before.unclassified_bytes, bytes.len() as u64);
        let (asset, entry, pins) = owner
            .store_mut()
            .unwrap()
            .import_media(
                &mut bytes.as_slice(),
                bytes.len() as u64,
                AssetId(1),
                Path::new("external.wav"),
                None,
            )
            .unwrap();
        assert!(entry.matches_media_asset(&asset));
        let after = owner.store().usage().unwrap();
        assert_eq!(after.unclassified_bytes, 0, "C0_FIX1_ORPHAN_RECLASSIFIED");
        let table = entry.chunk_table_ref().unwrap();
        assert_eq!(
            after.physical_bytes,
            before.physical_bytes + table.byte_length
        );
        assert_eq!(after.owned_bytes, before.owned_bytes);
        assert_eq!(after.objects, before.objects + 1);
        assert_eq!(after.reserved_bytes, 0);
        owner.collect_after_ack().unwrap();
        let mut descriptor = owner.store().open_input_descriptor(&entry.object).unwrap();
        let mut actual = Vec::new();
        descriptor.read_to_end(&mut actual).unwrap();
        assert_eq!(actual, bytes, "C0_FIX1_REUSED_BYTES_REOPEN");
        for _ in 0..2 {
            let source = owner.store().certify_media_entry(&entry).unwrap();
            let mut decoder = source.open_audio(Arc::new(AtomicBool::new(false))).unwrap();
            assert!(decoder.next_frame().unwrap().is_some());
        }
        assert_eq!(owner.store().usage().unwrap().unclassified_bytes, 0);
        drop(pins);
    }

    #[test]
    fn s3_import_stream_hashes_full_and_final_short_chunk_from_written_bytes() {
        let (_root, mut owner) = session("c0-stream-hashes");
        let bytes = (0..INPUT_CHUNK_BYTES + 93)
            .map(|n| (n % 251) as u8)
            .collect::<Vec<_>>();
        let (sealed, table) = owner
            .store_mut()
            .admit_measured_media(&mut bytes.as_slice(), bytes.len() as u64)
            .unwrap();
        assert_eq!(table.source_sha256, kinewright_media::sha256_bytes(&bytes));
        assert_eq!(table.chunk_sha256.len(), 2);
        for (chunk, digest) in bytes
            .chunks(usize::try_from(INPUT_CHUNK_BYTES).unwrap())
            .zip(&table.chunk_sha256)
        {
            assert_eq!(&hex_digest(*digest), &kinewright_media::sha256_bytes(chunk));
        }
        let mut file = owner
            .store()
            .open_input_descriptor(&sealed.reference)
            .unwrap();
        let mut actual = Vec::new();
        file.read_to_end(&mut actual).unwrap();
        assert_eq!(actual, bytes);
    }
    #[test]
    fn s3_import_refuses_short_growing_and_overflow_without_leaked_charges() {
        let (_root, mut owner) = session("c0-short-import");
        let before = owner.store().usage().unwrap();
        for (bytes, length, marker) in [
            (vec![1, 2], 3, "C0_IMPORT_SHORT_READ_REFUSED"),
            (vec![1, 2, 3], 2, "C0_IMPORT_GROWTH_REFUSED"),
            (vec![], u64::MAX, "C0_IMPORT_OVERFLOW_REFUSED"),
        ] {
            assert!(
                owner
                    .store_mut()
                    .admit_measured_media(&mut bytes.as_slice(), length)
                    .is_err(),
                "{marker}"
            );
            let after = owner.store().usage().unwrap();
            assert_eq!(after.physical_bytes, before.physical_bytes);
            assert_eq!(after.decoded_bytes, before.decoded_bytes);
            assert_eq!(after.reserved_bytes, before.reserved_bytes);
        }
    }
    #[test]
    fn s3_fingerprint_digest_length_and_interpretation_admission() {
        let (_root, mut owner) = session("c0-identity-admission");
        let (asset, entry, _pins) = import(&mut owner, 1, 1234);
        let inputs = manifest(&owner, vec![entry.clone()]);
        let mut document = kinewright_core::Document {
            media_pool: vec![asset.clone()],
            ..Default::default()
        };
        crate::bundle_commit::validate_document_inputs(&document, &inputs).unwrap();
        document.media_pool[0].source_fingerprint.content_sha256 = Some("0".repeat(64));
        assert!(
            crate::bundle_commit::validate_document_inputs(&document, &inputs).is_err(),
            "C0_SOURCE_DIGEST_REFUSED"
        );
        document.media_pool[0] = asset.clone();
        document.media_pool[0].source_fingerprint.byte_len = Some(1);
        assert!(
            crate::bundle_commit::validate_document_inputs(&document, &inputs).is_err(),
            "C0_SOURCE_LENGTH_REFUSED"
        );
        document.media_pool[0] = asset.clone();
        document.media_pool[0].duration.0 += 1;
        assert!(
            crate::bundle_commit::validate_document_inputs(&document, &inputs).is_err(),
            "C0_SOURCE_INTERPRETATION_REFUSED"
        );
        let mut altered = entry.media_interpretation().unwrap();
        altered.audio.as_mut().unwrap().timestamp_origin += 1;
        let mut wrong = entry.clone();
        wrong.profile = Some(altered.canonical().unwrap());
        wrong.entry_id = media_entry_id(asset.id, &wrong.object, wrong.profile.as_deref().unwrap());
        assert!(
            owner.admit_inputs(&manifest(&owner, vec![wrong])).is_err(),
            "C0_PROBE_ORIGIN_REFUSED"
        );
        let mut video = entry.media_interpretation().unwrap();
        video.kind = MediaKind::Video;
        video.video = video.audio.take();
        video.resolution = Some((32, 24));
        assert!(
            video.canonical().is_err(),
            "C0_UNKNOWN_VIDEO_PROFILE_REFUSED"
        );
        let mut missing = entry;
        missing.dependencies.clear();
        assert!(
            owner
                .admit_inputs(&manifest(&owner, vec![missing]))
                .is_err()
        );
    }
    #[test]
    fn s3_historical_same_asset_id_preserves_complete_input_identity() {
        let (_root, mut owner) = session("c0-historical-id");
        let (a, first, _a_pins) = import(&mut owner, 1, 1111);
        let (b, second, _b_pins) = import(&mut owner, 1, -2222);
        assert_ne!(first.entry_id, second.entry_id);
        let core = kinewright_core::Core::spawn(kinewright_core::Document {
            media_pool: vec![a.clone()],
            ..Default::default()
        })
        .unwrap();
        let history = core.export_history().unwrap();
        let other = kinewright_core::Core::spawn(kinewright_core::Document {
            media_pool: vec![b.clone()],
            ..Default::default()
        })
        .unwrap()
        .export_history()
        .unwrap();
        let inputs = manifest(&owner, vec![first.clone(), second.clone()]);
        let mut retained = inputs.clone();
        crate::bundle_commit::retain_history_inputs(&history, &mut retained);
        assert_eq!(
            retained.entries,
            vec![first.clone()],
            "C0_HISTORICAL_COMPLETE_IDENTITY_PRESERVED"
        );
        let mut retained = inputs.clone();
        crate::bundle_commit::retain_history_inputs(&other, &mut retained);
        assert_eq!(
            retained.entries,
            vec![second.clone()],
            "C0_HISTORICAL_COMPLETE_IDENTITY_PRESERVED"
        );
        crate::bundle_commit::validate_inputs(&history, &inputs).unwrap();
        crate::bundle_commit::validate_inputs(&other, &inputs).unwrap();
        let sealed = owner.admit_inputs(&inputs).unwrap();
        for (asset, entry) in [(a, first), (b, second)] {
            assert!(entry.matches_media_asset(&asset));
            let reference = OwnedInputRef {
                manifest_ref: sealed.reference.clone(),
                entry,
            };
            let certified = owner.store().certify_media_input(&reference).unwrap();
            assert_eq!(
                certified.identity().source_sha256,
                parse_digest(asset.source_fingerprint.content_sha256.as_deref().unwrap()).unwrap()
            );
        }
    }
    #[test]
    fn s3_table_binding_and_cursor_pin_charges_survive_last_consumer() {
        let (_root, mut owner) = session("c0-table-charges");
        let (_asset, entry, pins) = import(&mut owner, 1, 4321);
        let sealed = owner
            .admit_inputs(&manifest(&owner, vec![entry.clone()]))
            .unwrap();
        let reference = OwnedInputRef {
            manifest_ref: sealed.reference.clone(),
            entry: entry.clone(),
        };
        let source = owner.store().certify_media_input(&reference).unwrap();
        let baseline = owner.store().usage().unwrap().decoded_bytes;
        let second = owner.store().certify_media_input(&reference).unwrap();
        assert_eq!(
            owner.store().usage().unwrap().decoded_bytes,
            baseline,
            "shared table charged once"
        );
        let mut cursor = source.cursor(Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!(
            owner.store().usage().unwrap().decoded_bytes - baseline,
            kinewright_media::owned_io::CURSOR_RESERVATION_BYTES
        );
        drop(pins);
        drop(sealed);
        drop(source);
        drop(second);
        owner.store_mut().collect_unpublished().unwrap();
        assert!(
            owner
                .store()
                .open_input_descriptor(&reference.entry.object)
                .is_ok(),
            "C0_SOURCE_PIN_THROUGH_LAST_CURSOR"
        );
        assert!(
            owner.store().usage().unwrap().physical_bytes >= reference.entry.object.byte_length
        );
        let mut header = [0; 4];
        assert_eq!(cursor.read(&mut header).unwrap(), 4);
        assert_eq!(&header, b"RIFF");
        drop(cursor);
        owner.store_mut().collect_unpublished().unwrap();
        assert_eq!(owner.store().usage().unwrap().decoded_bytes, 0);
        let (blob, table) = owner
            .store_mut()
            .admit_measured_media(&mut Cursor::new(wav(5432)), wav(5432).len() as u64)
            .unwrap();
        let mut false_table = table;
        false_table.source_sha256 = "0".repeat(64);
        let hashed = owner.store_mut().admit_chunk_table(&false_table).unwrap();
        assert!(
            owner
                .store()
                .certify_media_parts(&blob.reference, &hashed.reference, None)
                .is_err(),
            "C0_CONFLICTING_TABLE_BINDING_REFUSED"
        );
    }
    #[test]
    fn s3_unsaved_source_capacity_refuses_before_document_install() {
        let (root, _owner) = session("c0-unsaved-capacity");
        let source = root.path("source.wav");
        std::fs::write(&source, wav(3333)).unwrap();
        let core = kinewright_core::Core::spawn(kinewright_core::Document::default()).unwrap();
        let limits = StorageLimits {
            pending_commands: 2,
            ..StorageLimits::default()
        };
        let mut edit =
            crate::editing_session::Ja1EditingSession::new(root.root(), &core, limits).unwrap();
        let launch = edit.prepare_host_attachment().unwrap();
        assert!(launch.source.is_some());
        edit.import_owned_media(&source).unwrap();
        let prepared = edit.refresh_unsaved_host_source().unwrap();
        assert_eq!(&prepared.inputs, edit.current_inputs_ref());
        let before = core.export_history().unwrap();
        let inputs = edit.current_inputs_ref().clone();
        let Err(error) = edit.import_owned_media(&source) else {
            panic!("C0_CAPACITY_VARIANT: import exceeded retained source capacity");
        };
        assert!(
            matches!(
                error,
                BundleError::Capacity("retained borrowed source attempts")
            ),
            "C0_CAPACITY_VARIANT: expected retained borrowed source Capacity, got {error:?}"
        );
        assert_eq!(
            core.export_history().unwrap(),
            before,
            "C0_CAPACITY_DOCUMENT_PRESERVED"
        );
        assert_eq!(
            edit.current_inputs_ref(),
            &inputs,
            "C0_CAPACITY_MANIFEST_PRESERVED"
        );
        assert!(
            edit.refresh_unsaved_host_source().is_err(),
            "C0_CAPACITY_NO_PENDING_TRANSFER"
        );
    }
    #[test]
    fn s3_nofollow_import_refuses_alias_and_preserves_document() {
        let (root, mut owner) = session("c0-nofollow");
        let bytes = wav(777);
        let actual = root.path("actual.wav");
        std::fs::write(&actual, &bytes).unwrap();
        let link = root.path("link.wav");
        std::os::unix::fs::symlink(&actual, &link).unwrap();
        let core = kinewright_core::Core::spawn(kinewright_core::Document::default()).unwrap();
        let mut session = crate::editing_session::Ja1EditingSession::new(
            root.root(),
            &core,
            StorageLimits::default(),
        )
        .unwrap();
        let history = core.export_history().unwrap();
        assert!(
            session.prepare_owned_media(&link, AssetId(1)).is_err(),
            "C0_NOFOLLOW_SOURCE_REFUSED"
        );
        assert_eq!(
            core.export_history().unwrap(),
            history,
            "C0_NOFOLLOW_DOCUMENT_PRESERVED"
        );
        let (_asset, mut entry, _pins) = import(&mut owner, 1, 777);
        entry.dependencies.clear();
        assert!(owner.admit_inputs(&manifest(&owner, vec![entry])).is_err());
    }

    #[test]
    fn s3_manifest_reads_do_not_probe_or_reject_an_unrelated_damaged_source() {
        use std::io::{Seek, SeekFrom};
        let (_root, mut owner) = session("c0-structural-manifest-read");
        let (_a, first, first_pins) = import(&mut owner, 1, 1111);
        let (_b, second, second_pins) = import(&mut owner, 2, 2222);
        let sealed = owner
            .admit_inputs(&manifest(&owner, vec![first.clone(), second.clone()]))
            .unwrap();
        let reference = OwnedInputRef {
            manifest_ref: sealed.reference.clone(),
            entry: first,
        };
        let root = owner.store().resolved_root().unwrap();
        let assets = root.with_file_name(format!(
            "{}.kinewright-assets",
            root.file_stem().unwrap().to_string_lossy()
        ));
        let mut writer = std::fs::OpenOptions::new()
            .write(true)
            .open(assets.join("studio/objects").join(&second.object.sha256))
            .unwrap();
        writer.seek(SeekFrom::Start(100)).unwrap();
        writer.write_all(&[0xff, 0xff]).unwrap();
        writer.flush().unwrap();
        let reserve = owner.store().decoder_reserver();
        let all_decoder_bytes = reserve(StorageLimits::default().decoder_bytes).unwrap();
        let typed = owner
            .store()
            .read_input_manifest(&sealed.reference)
            .expect("C0_TYPED_MANIFEST_READ_DOES_NOT_PROBE");
        assert_eq!(typed.entries.len(), 2);
        let source = owner
            .store()
            .certify_media_input(&reference)
            .expect("C0_CERTIFICATE_USES_COMPLETE_IDENTITY_MEMO");
        drop(all_decoder_bytes);
        let mut decoder = source
            .open_audio(Arc::new(AtomicBool::new(false)))
            .expect("C0_UNRELATED_DAMAGE_DOES_NOT_BLOCK_SELECTED_CERTIFICATE");
        assert!(decoder.next_frame().unwrap().is_some());
        let damaged = owner
            .store()
            .certify_media_input(&OwnedInputRef {
                manifest_ref: sealed.reference.clone(),
                entry: second,
            })
            .unwrap();
        assert!(
            damaged
                .open_audio(Arc::new(AtomicBool::new(false)))
                .is_err(),
            "C0_MEMO_DOES_NOT_BYPASS_ACTUAL_USE_CHUNK_CHECKS"
        );
        drop((first_pins, second_pins));
    }

    #[test]
    fn s3_manifest_chunk_table_dependency_survives_collection() {
        let (_root, mut owner) = session("c0-table-edge-retention");
        let (_asset, entry, pins) = import(&mut owner, 1, 2468);
        let table = entry.chunk_table_ref().unwrap().clone();
        let sealed = owner.admit_inputs(&manifest(&owner, vec![entry])).unwrap();
        drop(pins);
        owner.store_mut().collect_unpublished().unwrap();
        assert!(
            owner.store().open_input_descriptor(&table).is_ok(),
            "C0_MANIFEST_TABLE_EDGE_PRESERVED"
        );
        assert!(
            owner
                .store()
                .closure(std::slice::from_ref(&sealed.reference))
                .unwrap()
                .contains(&table),
            "C0_MANIFEST_TABLE_GRAPH_PRESERVED"
        );
    }

    #[test]
    fn s3_unsaved_local_colour_override_preserves_undo_interpretation() {
        let (root, _owner) = session("c0-unsaved-local-colour");
        let path = root.path("source.wav");
        std::fs::write(&path, wav(1234)).unwrap();
        let core = kinewright_core::Core::spawn(kinewright_core::Document::default()).unwrap();
        let mut edit = crate::editing_session::Ja1EditingSession::new(
            root.root(),
            &core,
            StorageLimits::default(),
        )
        .unwrap();
        let (original, _) = edit.import_owned_media(&path).unwrap();
        let mut changed = original.clone();
        changed.color_description.provenance = kinewright_core::ColorProvenance::UserOverride;
        changed.color_description.confidence_basis_points = 10_000;
        edit.apply_owned_media_interpretation(&changed).unwrap();
        let manifest = edit
            .store()
            .unwrap()
            .read_input_manifest(edit.current_inputs_ref())
            .unwrap();
        assert!(
            manifest
                .entries
                .iter()
                .any(|entry| entry.matches_media_asset(&original)),
            "C0_UNSAVED_OVERRIDE_RETAINS_UNDO_INPUT"
        );
        assert!(
            manifest
                .entries
                .iter()
                .any(|entry| entry.matches_media_asset(&changed)),
            "C0_UNSAVED_OVERRIDE_ADMITS_NEW_INPUT"
        );
        let history = core.export_history().unwrap();
        crate::bundle_commit::validate_inputs(&history, &manifest).unwrap();
        assert_eq!(
            history.document.media_pool[0].color_description,
            changed.color_description
        );
    }
}

#[cfg(all(test, target_os = "linux"))]
mod resource_tests {
    use super::*;
    use crate::bundle_copy::SessionStore;
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    use std::io::{Seek, SeekFrom, Write};
    #[test]
    fn s3_certified_resources_are_private_deduplicated_and_pinned() {
        let root = TempDirectory::new_in(&workspace_target_dir(), "c0-resource-copy");
        let mut owner = SessionStore::create(root.root(), StorageLimits::default()).unwrap();
        let bytes = kinewright_core::title_font_bytes();
        let object = ObjectRef {
            sha256: kinewright_media::sha256_bytes(bytes),
            byte_length: bytes.len() as u64,
            record_kind: RecordKind::OwnedFont,
        };
        let mut reader = bytes;
        let blob = owner
            .store_mut()
            .admit_blob(&mut reader, &object, &[])
            .unwrap();
        let entry = OwnedInputEntry {
            entry_id: "font:Inter".into(),
            object: object.clone(),
            dependencies: vec![],
            selected_stream: None,
            time_base: None,
            origin: None,
            profile: None,
        };
        let manifest = OwnedInputManifest {
            project_id: None,
            session_id: Some(owner.session_id().clone()),
            owner_epoch: owner.store().owner_epoch(),
            entries: vec![entry.clone()],
        };
        let sealed = owner.admit_inputs(&manifest).unwrap();
        let reference = OwnedInputRef {
            manifest_ref: sealed.reference.clone(),
            entry,
        };
        let resource = owner.store().certify_resource_input(&reference).unwrap();
        let baseline = owner.store().usage().unwrap().decoded_bytes;
        let second = owner.store().certify_resource_input(&reference).unwrap();
        assert_eq!(owner.store().usage().unwrap().decoded_bytes, baseline);
        let store_root = owner.store().resolved_root().unwrap();
        let assets = store_root.with_file_name(format!(
            "{}.kinewright-assets",
            store_root.file_stem().unwrap().to_string_lossy()
        ));
        let object_path = assets.join("studio/objects").join(&object.sha256);
        let mut alias = std::fs::OpenOptions::new()
            .write(true)
            .open(object_path)
            .unwrap();
        alias.seek(SeekFrom::Start(0)).unwrap();
        alias.write_all(&[bytes[0] ^ 1]).unwrap();
        alias.flush().unwrap();
        assert_eq!(resource.bytes(), bytes);
        assert_eq!(second.bytes(), bytes);
        drop(blob);
        drop(sealed);
        drop(second);
        owner.store_mut().collect_unpublished().unwrap();
        assert!(owner.store().open_input_descriptor(&object).is_ok());
        assert_eq!(resource.bytes(), bytes);
        drop(resource);
        owner.store_mut().collect_unpublished().unwrap();
        assert_eq!(owner.store().usage().unwrap().decoded_bytes, 0);
    }
}
