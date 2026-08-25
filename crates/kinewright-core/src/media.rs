use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crossbeam_channel::{Receiver, Sender};
use thiserror::Error;

use crate::{
    AssetId, ClipId, ColorDescription, Document, EffectId, LutAsset, LutAssetId, MediaAsset,
    MediaSourceFingerprint, Rational, TimeCode, TrackId,
};

/// The runtime truth about whether an imported source can currently be read.
///
/// This is deliberately not persisted in [`MediaAsset`]: availability depends
/// on the current machine and can change while a project is open. A verified
/// source has the same SHA-256 and byte length as its persisted fingerprint;
/// an unverified source is readable but belongs to a legacy asset without a
/// fingerprint. `Changed` means the path is readable but no longer matches the
/// source identity. `Unreadable` preserves the backend reason for failures that
/// are more specific than a missing path.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum MediaAvailabilityKind {
    OnlineVerified,
    OnlineUnverified,
    OfflineMissing,
    Changed,
    Unreadable,
}

/// A typed, machine-readable media availability observation.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct MediaAvailabilityStatus {
    pub kind: MediaAvailabilityKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub observed_fingerprint: Option<MediaSourceFingerprint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub reason: Option<String>,
}

/// One live source-availability failure that blocks export.
///
/// This keeps the availability observation out of persisted project state:
/// source status belongs to the machine that is about to render, while the
/// imported fingerprint remains part of the project document.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct ExportMediaPreflightIssue {
    pub asset: AssetId,
    pub asset_name: String,
    pub availability: MediaAvailabilityStatus,
}

/// Live source-identity result for the timeline-referenced portion of one
/// immutable export document.
///
/// `OnlineUnverified` deliberately blocks export. It identifies a legacy
/// source that has no persisted fingerprint, so a readable path alone cannot
/// prove it is the media that was edited. Re-import or explicitly relink the
/// source to record a verified identity before delivery. This policy applies
/// equally to video and audio assets and is intentionally stricter than
/// ordinary preview availability.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct ExportMediaPreflightReport {
    /// Asset ids checked in deterministic media-pool order. Unused media-bin
    /// entries are intentionally absent.
    pub checked_assets: Vec<AssetId>,
    /// Every status other than `online_verified`, retained with its typed
    /// backend reason for UI and agent recovery surfaces.
    pub issues: Vec<ExportMediaPreflightIssue>,
}

impl ExportMediaPreflightReport {
    /// Return whether every timeline-referenced source was live and matched
    /// its persisted fingerprint at preflight time.
    #[must_use]
    pub const fn export_ready(&self) -> bool {
        self.issues.is_empty()
    }

    /// Produce a concise human-readable summary while preserving the full
    /// typed report for callers that need individual recovery actions.
    #[must_use]
    pub fn summary(&self) -> String {
        if self.issues.is_empty() {
            return format!(
                "{} timeline-referenced source(s) are online and fingerprint-verified",
                self.checked_assets.len()
            );
        }
        let details = self
            .issues
            .iter()
            .map(|issue| {
                let reason = issue
                    .availability
                    .reason
                    .as_deref()
                    .unwrap_or("no backend reason was provided");
                format!(
                    "{} ({:?}): {reason}",
                    issue.asset_name, issue.availability.kind
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        format!(
            "Export blocked: {} timeline-referenced source(s) need relink or recovery: {details}",
            self.issues.len()
        )
    }
}

/// The runtime truth about whether a project-owned LUT asset can currently be
/// read from the store (CC4 §2.3).
///
/// Availability is runtime state, never project state: it depends on the
/// current machine and is never serialized into a document. Core has no
/// filesystem or project-directory concept, so it never computes these values
/// — the media layer observes them against the derived store root and passes
/// them in.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum LutAvailabilityKind {
    /// The store file exists, is a regular file, and hashes to `sha256`.
    Verified,
    /// The store file is absent or is not a regular file.
    Missing,
    /// A file exists but its bytes hash to something else.
    Changed,
    /// The path exists but bytes or metadata cannot be read.
    Unreadable,
}

/// A typed, machine-readable LUT availability observation (CC4 §2.3).
///
/// M41's `online_unverified` has no CC4 equivalent: a LUT asset can only be
/// created with a hash, so there is no legacy unverified state.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct LutAvailabilityStatus {
    pub kind: LutAvailabilityKind,
    /// The hash actually observed, present when a file was read and hashed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub observed_sha256: Option<String>,
    /// The backend reason, preserved verbatim for recovery surfaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub reason: Option<String>,
    /// The expected store path, so a human can be told where to look.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub path: Option<PathBuf>,
}

/// One live LUT-availability failure that blocks proof and export (CC4 §2.3).
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct ExportLutPreflightIssue {
    pub lut_asset: LutAssetId,
    pub title: String,
    /// The hash the project records, which is the identity being looked for.
    pub sha256: String,
    pub kind: LutAvailabilityKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub path: Option<PathBuf>,
    /// Every node that could evaluate this asset on some frame, in document
    /// order. An asset referenced only by nodes that can never evaluate does
    /// not appear in the report at all.
    pub referenced_by: Vec<(ClipId, EffectId)>,
}

/// Live LUT-identity result over the assets referenced by possibly-active
/// nodes in one immutable export document (CC4 §2.3).
///
/// The mirror of [`ExportMediaPreflightReport`] for looks. A `missing`,
/// `changed`, or `unreadable` asset referenced by a node that could evaluate
/// blocks managed proof and export; an asset referenced only by bypassed or
/// `mix = 0` nodes does not block, because those nodes are never evaluated.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct ExportLutPreflightReport {
    /// Asset ids checked, in `Document::lut_assets` order. Assets no
    /// possibly-active node references are intentionally absent.
    pub checked_lut_assets: Vec<LutAssetId>,
    /// Every status other than `verified`, retained with its typed backend
    /// reason for UI and agent recovery surfaces.
    pub issues: Vec<ExportLutPreflightIssue>,
}

impl ExportLutPreflightReport {
    /// Return whether every look a frame could need hashed to its recorded
    /// identity at preflight time.
    #[must_use]
    pub const fn export_ready(&self) -> bool {
        self.issues.is_empty()
    }

    /// Produce a concise human-readable summary while preserving the full
    /// typed report for callers that need individual recovery actions.
    #[must_use]
    pub fn summary(&self) -> String {
        if self.issues.is_empty() {
            return format!(
                "{} referenced LUT asset(s) are present and hash-verified",
                self.checked_lut_assets.len()
            );
        }
        let details = self
            .issues
            .iter()
            .map(|issue| {
                let reason = issue
                    .reason
                    .as_deref()
                    .unwrap_or("no backend reason was provided");
                format!("{} ({:?}): {reason}", issue.title, issue.kind)
            })
            .collect::<Vec<_>>()
            .join("; ");
        format!(
            "Export blocked: {} LUT asset(s) need restore or replacement: {details}",
            self.issues.len()
        )
    }
}

/// Recheck every LUT asset a frame could need immediately before a proof or an
/// export is accepted (CC4 §2.3).
///
/// The observation comes from the caller's store-backed probe rather than the
/// persisted document, for the same reason M41's media preflight does: the
/// bytes are machine-local and can change while a project is open. Core
/// supplies only the document-side half — which assets are referenced by nodes
/// that could evaluate — because it has no filesystem concept.
///
/// Assets referenced only by nodes that can never evaluate (bypassed on every
/// frame, `mix = 0` on every frame, or unbound) are skipped entirely, so a
/// look an operator has switched off cannot block a delivery.
#[must_use]
pub fn export_lut_preflight_with(
    document: &Document,
    availability_for: &dyn Fn(&LutAsset) -> LutAvailabilityStatus,
) -> ExportLutPreflightReport {
    let mut checked_lut_assets = Vec::new();
    let mut issues = Vec::new();
    for asset in &document.lut_assets {
        let referenced_by = possibly_active_lut_references(document, asset.id);
        if referenced_by.is_empty() {
            continue;
        }
        checked_lut_assets.push(asset.id);
        let availability = availability_for(asset);
        if availability.kind == LutAvailabilityKind::Verified {
            continue;
        }
        issues.push(ExportLutPreflightIssue {
            lut_asset: asset.id,
            title: asset.title.clone(),
            sha256: asset.sha256.clone(),
            kind: availability.kind,
            reason: availability.reason,
            path: availability.path,
            referenced_by,
        });
    }
    ExportLutPreflightReport {
        checked_lut_assets,
        issues,
    }
}

/// Every node that references one asset and could evaluate on some frame.
fn possibly_active_lut_references(document: &Document, id: LutAssetId) -> Vec<(ClipId, EffectId)> {
    let referencing = document.lut_asset_references(id);
    document
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .flat_map(|clip| clip.effects.iter().map(move |effect| (clip.id, effect)))
        .filter(|(clip, effect)| {
            referencing.contains(&(*clip, effect.id)) && crate::lut_node_may_be_active(effect)
        })
        .map(|(clip, effect)| (clip, effect.id))
        .collect()
}

/// Fixed cache families exposed by the media runtime. The generated-proxy
/// family is present in the contract so clients can distinguish an intentional
/// unsupported feature from an empty cache; M41 does not generate proxies.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum MediaCacheFamily {
    PreviewMemory,
    VisualAssets,
    DerivedAnalysis,
    Transcripts,
    GeneratedProxy,
}

/// Inventory for one owned or explicitly unsupported cache family.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct MediaCacheFamilyStatus {
    pub family: MediaCacheFamily,
    pub supported: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub root: Option<PathBuf>,
    pub file_count: u64,
    pub bytes: u64,
    /// The family can be repopulated by an active worker or normal preview
    /// activity after a clear. This avoids promising permanence to callers.
    pub may_repopulate: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub note: Option<String>,
}

/// Snapshot of the fixed media-cache families.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct MediaCacheInventory {
    pub families: Vec<MediaCacheFamilyStatus>,
}

/// Result of a scoped cache clear. Clearing is idempotent: absent roots and
/// already-removed files report zero removed files and bytes.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct MediaCacheClearResult {
    pub family: MediaCacheFamily,
    pub supported: bool,
    pub removed_file_count: u64,
    pub removed_bytes: u64,
    pub may_repopulate: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameTexture {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// The renderer implementation that produced a managed monitor proof.
///
/// This is intentionally a core-owned vocabulary: proof manifests must not
/// expose a `wgpu` type or make a backend-specific claim that a test double
/// cannot support.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum MonitorProofRenderKind {
    GpuPreview,
    TestDouble,
}

/// Backend provenance attached to one full-resolution managed proof.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct MonitorProofMetadata {
    pub render_kind: MonitorProofRenderKind,
    /// Stable backend identifier, such as `vulkan` or `dx12`.
    pub backend: String,
    /// Adapter/device name reported by the active renderer.
    pub adapter: String,
    /// True when the renderer is known to be a software fallback.
    pub software_fallback: bool,
    /// True only when the renderer can honestly claim a GPU compositor path.
    pub gpu_claim: bool,
    /// Full-raster proof marker. Managed proof must always set this to true.
    pub full_resolution: bool,
}

impl MonitorProofMetadata {
    /// Metadata for deterministic analysis/test doubles. It never claims GPU
    /// rendering and makes the non-production backend explicit.
    #[must_use]
    pub fn test_double() -> Self {
        Self {
            render_kind: MonitorProofRenderKind::TestDouble,
            backend: "test_double".to_owned(),
            adapter: "test_double".to_owned(),
            software_fallback: true,
            gpu_claim: false,
            full_resolution: true,
        }
    }
}

/// One full-resolution managed monitor proof and its renderer provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorProof {
    pub image: RgbaImage,
    pub metadata: MonitorProofMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WaveformPeak {
    pub minimum: i16,
    pub maximum: i16,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WaveformData {
    pub asset: AssetId,
    /// Monotonic request generation supplied by the caller. This is runtime
    /// delivery metadata only and is deliberately excluded from the
    /// content-addressed disk cache serialization.
    #[serde(skip, default)]
    pub request_generation: u64,
    /// The requesting asset's media path. Asset ids are per-document, so
    /// with several projects open the path is the identity consumers key
    /// by. Not persisted: the disk cache is content-addressed, and the
    /// serving worker rebinds the path per request.
    #[serde(skip)]
    pub path: std::path::PathBuf,
    pub content_sha256: String,
    pub source_fps: Rational,
    pub source_frames: TimeCode,
    pub peaks: Vec<WaveformPeak>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ThumbnailKey {
    pub asset: AssetId,
    pub source_at: TimeCode,
    pub max_width: u32,
}

#[derive(Debug, Clone)]
pub struct ThumbnailFrame {
    pub key: ThumbnailKey,
    /// Monotonic request generation supplied by the caller. This is runtime
    /// delivery metadata only and is not part of the thumbnail cache key.
    pub request_generation: u64,
    /// See [`WaveformData::path`]: the content identity behind the id.
    pub path: std::path::PathBuf,
    pub image: Arc<RgbaImage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualRequestKind {
    Waveform,
    Thumbnail(ThumbnailKey),
}

#[derive(Debug, Clone)]
pub enum VisualAssetResult {
    Waveform(Arc<WaveformData>),
    Thumbnail(ThumbnailFrame),
    Failed {
        asset: AssetId,
        /// Monotonic request generation supplied by the caller. This remains
        /// available when a worker cannot produce a waveform or thumbnail.
        request_generation: u64,
        /// See [`WaveformData::path`]: the content identity behind the id.
        path: std::path::PathBuf,
        request: VisualRequestKind,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportSettings {
    pub fps: Rational,
    pub resolution: (u32, u32),
    /// Colour metadata declared for the encoded delivery.
    ///
    /// This is an output-tag contract only. The current export path does not
    /// perform a colour transform; managed pixel transforms belong to CC1.
    pub delivery_color: ColorDescription,
    pub video_codec: String,
    pub audio_codec: String,
    pub video_bitrate: u64,
    pub audio_bitrate: u64,
    pub cancellation: ExportCancellation,
}

/// Delivery loudness measured from decoded PCM using the ITU-R BS.1770
/// K-weighting and gating model. Decibel values use hundredths so agent and
/// project contracts remain deterministic and JSON-safe.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct AudioLoudness {
    /// Gated programme loudness. `None` means the decoded signal was silent.
    pub integrated_lufs_hundredths: Option<i32>,
    /// Highest decoded sample before loudness weighting. `None` means silence.
    pub sample_peak_dbfs_hundredths: Option<i32>,
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_frames: u64,
}

#[derive(Debug, Clone, Default)]
pub struct ExportCancellation(Arc<AtomicBool>);

impl ExportCancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

impl PartialEq for ExportCancellation {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for ExportCancellation {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportProgress {
    pub completed_frames: u64,
    pub total_frames: u64,
}

pub type ProgressSink = Sender<ExportProgress>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Paused,
    Playing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaEvent {
    Position(TimeCode),
    PlaybackStateChanged(PlaybackState),
    Error(MediaError),
}

/// One recognized word. Both boundaries are half-open source-frame positions
/// in the owning asset's exact frame rate.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TranscriptWord {
    pub text: String,
    pub source_start: TimeCode,
    pub source_end: TimeCode,
    /// Optional stable diarization label. Plain Whisper transcripts leave it
    /// unset; speaker-aware backends can populate it without changing edits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

/// Derived, reproducible speech data for one media asset. This deliberately
/// does not live in `Document` or the operation journal.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AssetTranscript {
    pub asset: AssetId,
    pub content_sha256: String,
    pub source_fps: Rational,
    pub words: Vec<TranscriptWord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptStatus {
    NotRequested,
    Queued,
    Hashing,
    DownloadingModel {
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    Transcribing {
        progress_percent: u8,
    },
    Ready(Arc<AssetTranscript>),
    NoSpeech,
    Cancelled,
    Failed(String),
}

impl TranscriptStatus {
    #[must_use]
    pub const fn is_running(&self) -> bool {
        matches!(
            self,
            Self::Queued
                | Self::Hashing
                | Self::DownloadingModel { .. }
                | Self::Transcribing { .. }
        )
    }
}

/// A source word mapped through a clip onto the project timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineTranscriptWord {
    pub text: String,
    pub speaker: Option<String>,
    pub asset: AssetId,
    pub track: TrackId,
    pub clip: ClipId,
    pub source_start: TimeCode,
    pub source_end: TimeCode,
    pub project_start: TimeCode,
    pub project_end: TimeCode,
}

/// A half-open silent range in one asset's exact source-frame time base.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SilenceSpan {
    pub source_start: TimeCode,
    pub source_end: TimeCode,
}

/// Derived, reproducible audio-energy analysis for one media asset.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AssetSilences {
    pub asset: AssetId,
    pub content_sha256: String,
    pub source_fps: Rational,
    pub source_frames: TimeCode,
    /// Detection threshold in hundredths of a dBFS (for example, -4000 is -40 dBFS).
    pub threshold_dbfs_hundredths: i32,
    pub window_milliseconds: u32,
    pub spans: Vec<SilenceSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SilenceStatus {
    NotRequested,
    Queued,
    Hashing,
    Analyzing,
    Ready(Arc<AssetSilences>),
    NoAudio,
    Cancelled,
    Failed(String),
}

impl SilenceStatus {
    #[must_use]
    pub const fn is_running(&self) -> bool {
        matches!(self, Self::Queued | Self::Hashing | Self::Analyzing)
    }
}

/// One candidate scene boundary. Confidence is stored as basis points from
/// 0.00% through 100.00% so the derived contract remains deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SceneChange {
    pub source_frame: TimeCode,
    pub confidence_basis_points: u16,
}

/// Derived, reproducible scene-difference analysis for one media asset.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AssetSceneChanges {
    pub asset: AssetId,
    pub content_sha256: String,
    pub source_fps: Rational,
    pub source_frames: TimeCode,
    pub proxy_width: u32,
    pub changes: Vec<SceneChange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SceneStatus {
    NotRequested,
    Queued,
    Hashing,
    Analyzing,
    Ready(Arc<AssetSceneChanges>),
    NoVideo,
    Cancelled,
    Failed(String),
}

impl SceneStatus {
    #[must_use]
    pub const fn is_running(&self) -> bool {
        matches!(self, Self::Queued | Self::Hashing | Self::Analyzing)
    }
}

/// A source silence span mapped through one clip onto project frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimelineSilenceSpan {
    pub asset: AssetId,
    pub track: TrackId,
    pub clip: ClipId,
    pub source_start: TimeCode,
    pub source_end: TimeCode,
    pub project_start: TimeCode,
    pub project_end: TimeCode,
}

/// A source scene boundary mapped through one clip onto a project frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimelineSceneChange {
    pub asset: AssetId,
    pub track: TrackId,
    pub clip: ClipId,
    pub source_frame: TimeCode,
    pub project_frame: TimeCode,
    pub confidence_basis_points: u16,
}

/// One locally detected rhythmic onset in an asset's exact source-frame grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BeatMarker {
    pub source_frame: TimeCode,
    /// Relative onset strength from 0.00% through 100.00%.
    pub strength_basis_points: u16,
}

/// Derived, reproducible rhythmic analysis for one audio-capable asset.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AssetBeats {
    pub asset: AssetId,
    pub content_sha256: String,
    pub source_fps: Rational,
    pub source_frames: TimeCode,
    /// Robust interval estimate in thousandths of a beat per minute.
    pub estimated_bpm_milli: u32,
    pub beats: Vec<BeatMarker>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeatStatus {
    NotRequested,
    Queued,
    Hashing,
    Analyzing { progress_percent: Option<u8> },
    Ready(Arc<AssetBeats>),
    NoAudio,
    Cancelled,
    Failed(String),
}

impl BeatStatus {
    #[must_use]
    pub const fn is_running(&self) -> bool {
        matches!(self, Self::Queued | Self::Hashing | Self::Analyzing { .. })
    }
}

/// One source beat mapped through a real-time media clip onto project frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TimelineBeat {
    pub asset: AssetId,
    pub track: TrackId,
    pub clip: ClipId,
    pub source_frame: TimeCode,
    pub project_frame: TimeCode,
    pub strength_basis_points: u16,
    pub estimated_bpm_milli: u32,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisKind {
    Transcript,
    Silence,
    Scene,
    Beat,
}

impl AnalysisKind {
    pub const ALL: [Self; 4] = [Self::Transcript, Self::Silence, Self::Scene, Self::Beat];
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisPhase {
    NotRequested,
    Queued,
    Hashing,
    Downloading,
    Analyzing,
    Ready,
    Unavailable,
    Cancelled,
    Failed,
}

/// Provider-neutral lifecycle record for one derived-media job.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct AnalysisJobStatus {
    pub asset: AssetId,
    pub kind: AnalysisKind,
    pub phase: AnalysisPhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress_percent: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MediaError {
    #[error("media operation is not implemented")]
    NotImplemented,
    #[error("export was cancelled")]
    Cancelled,
    /// The managed renderer cannot prove that a decoder's native format is a
    /// supported integer source surface. This remains structured so proof
    /// callers can offer a stable recovery action instead of parsing text.
    #[error(
        "unsupported_decoder_format: {reason} (path={path}, format={format}, declared_bit_depth={declared_bit_depth:?}, decoder_bit_depth={decoder_bit_depth:?})"
    )]
    UnsupportedDecoderFormat {
        /// The source path involved in the failed managed decode.
        path: PathBuf,
        /// The `FFmpeg` pixel-format name when available.
        format: String,
        /// The declared source integer depth, if it was valid enough to read.
        declared_bit_depth: Option<u8>,
        /// The decoder's native integer depth, if it was recognized.
        decoder_bit_depth: Option<u8>,
        /// A stable human-readable reason for recovery surfaces.
        reason: String,
    },
    #[error("media backend error: {0}")]
    Backend(String),
}

impl MediaError {
    /// Return the machine-readable recovery code, when this error has one.
    #[must_use]
    pub const fn recovery_code(&self) -> Option<&'static str> {
        match self {
            Self::UnsupportedDecoderFormat { .. } => Some("unsupported_decoder_format"),
            Self::NotImplemented | Self::Cancelled | Self::Backend(_) => None,
        }
    }
}

pub trait Playback: Send + Sync {
    fn set_document(&self, doc: Arc<Document>);
    fn request_frame(&self, t: TimeCode);
    fn frames(&self) -> Receiver<(TimeCode, FrameTexture)>;
    /// Non-blocking playback status stream. Implementations may coalesce ticks.
    fn events(&self) -> Receiver<MediaEvent>;
    fn play(&self, from: TimeCode);
    fn pause(&self);
    /// Seek transport and audio to an exact project frame without blocking the caller.
    fn seek(&self, to: TimeCode);
    /// Read the atomically published audio-master position.
    fn position(&self) -> TimeCode;
    /// Read the current post-limiter master output peaks for left and right.
    fn output_peaks(&self) -> [f32; 2];
}

pub trait Analysis: Send + Sync {
    /// Inspect a media file and return its project metadata.
    ///
    /// # Errors
    ///
    /// Returns a media error when the file cannot be probed.
    fn probe(&self, path: &Path) -> Result<MediaAsset, MediaError>;
    /// Inspect whether the source path is currently available and, when the
    /// file can be read, compare its fingerprint with the imported identity.
    /// Stateful media backends should override this to return verified,
    /// changed, and unreadable observations rather than the conservative
    /// default.
    fn media_availability(&self, asset: &MediaAsset) -> MediaAvailabilityStatus {
        if !asset.path.is_file() {
            return MediaAvailabilityStatus {
                kind: MediaAvailabilityKind::OfflineMissing,
                observed_fingerprint: None,
                reason: Some(format!(
                    "media path is missing or not a regular file: {}",
                    asset.path.display()
                )),
            };
        }
        MediaAvailabilityStatus {
            kind: MediaAvailabilityKind::OnlineUnverified,
            observed_fingerprint: None,
            reason: Some("source identity is not available for verification".to_owned()),
        }
    }
    /// Decode a thumbnail at an exact project frame.
    ///
    /// # Errors
    ///
    /// Returns a media error when decoding or compositing fails.
    fn thumbnail_at(&self, t: TimeCode, max_w: u32) -> Result<RgbaImage, MediaError>;
    /// Decode a thumbnail against an explicit immutable document without
    /// changing the live playback document.
    ///
    /// Backends that do not maintain playback state may use the default.
    /// Stateful backends should override this method so branch proofs cannot
    /// disturb the user's monitor or transport.
    ///
    /// # Errors
    ///
    /// Returns a media error when decoding or compositing fails.
    fn thumbnail_for_document(
        &self,
        _document: Arc<Document>,
        t: TimeCode,
        max_w: u32,
    ) -> Result<RgbaImage, MediaError> {
        self.thumbnail_at(t, max_w)
    }
    /// Render one exact project frame at the document's full resolution for
    /// managed colour proof. This is intentionally separate from thumbnail
    /// rendering: proxy dimensions cannot establish full-raster conformance.
    ///
    /// Stateful backends should use a branch-scoped renderer and must not
    /// mutate the live playback document or reuse a stale proxy surface.
    ///
    /// # Errors
    ///
    /// Returns a media error when the backend cannot provide a managed
    /// full-resolution proof frame.
    fn monitor_proof_for_document(
        &self,
        _document: Arc<Document>,
        _t: TimeCode,
    ) -> Result<MonitorProof, MediaError> {
        Err(MediaError::NotImplemented)
    }
    /// Queue derived speech recognition without blocking the caller. Repeated
    /// requests for the same asset are coalesced by the implementation.
    fn request_transcription(&self, asset: MediaAsset);
    /// Queue speech recognition with an optional ISO 639-1 language hint.
    /// Backends that do not support hints fall back to ordinary transcription.
    fn request_transcription_with_language(&self, asset: MediaAsset, _language: Option<&str>) {
        self.request_transcription(asset);
    }
    /// Return the latest state for an asset's derived transcript.
    ///
    /// Takes the full asset, not just its id: asset ids are per-document, so
    /// with several projects open the same id can name different files. The
    /// asset's path is the identity derived data is keyed by.
    fn transcript_status(&self, asset: &MediaAsset) -> TranscriptStatus;
    /// Return words currently audible on the timeline, optionally restricted
    /// to a half-open range of project frames.
    ///
    /// # Errors
    ///
    /// Returns a media error when timeline/source frame mapping fails.
    fn timeline_transcript(
        &self,
        document: &Document,
        range: Option<std::ops::Range<TimeCode>>,
    ) -> Result<Vec<TimelineTranscriptWord>, MediaError>;
    /// Queue windowed-RMS silence analysis without blocking the caller.
    fn request_silence_detection(&self, asset: MediaAsset);
    /// See [`Analysis::transcript_status`] for why this takes the full asset.
    fn silence_status(&self, asset: &MediaAsset) -> SilenceStatus;
    /// Return detected silence spans mapped into project time.
    ///
    /// # Errors
    ///
    /// Returns a media error when timeline/source frame mapping fails.
    fn timeline_silences(
        &self,
        document: &Document,
        range: Option<std::ops::Range<TimeCode>>,
        minimum_source_frames: TimeCode,
    ) -> Result<Vec<TimelineSilenceSpan>, MediaError>;
    /// Queue proxy-resolution scene analysis without blocking the caller.
    fn request_scene_detection(&self, asset: MediaAsset);
    /// See [`Analysis::transcript_status`] for why this takes the full asset.
    fn scene_status(&self, asset: &MediaAsset) -> SceneStatus;
    /// Return scene changes mapped into project time.
    ///
    /// # Errors
    ///
    /// Returns a media error when timeline/source frame mapping fails.
    fn timeline_scene_changes(
        &self,
        document: &Document,
        range: Option<std::ops::Range<TimeCode>>,
        minimum_confidence_basis_points: u16,
    ) -> Result<Vec<TimelineSceneChange>, MediaError>;
    /// Measure a decoded source asset using the delivery loudness contract.
    ///
    /// # Errors
    ///
    /// Returns a media error when audio cannot be decoded or measured.
    fn asset_loudness(&self, _asset: &MediaAsset) -> Result<AudioLoudness, MediaError> {
        Err(MediaError::NotImplemented)
    }
    /// Render the current audio graph in memory and measure its delivery loudness.
    ///
    /// # Errors
    ///
    /// Returns a media error when timeline audio cannot be rendered or measured.
    fn timeline_loudness(&self, _document: &Document) -> Result<AudioLoudness, MediaError> {
        Err(MediaError::NotImplemented)
    }
    /// Queue deterministic beat/onset analysis without blocking the caller.
    fn request_beat_detection(&self, _asset: MediaAsset) {}
    /// Return the latest beat-analysis state for an asset.
    fn beat_status(&self, _asset: &MediaAsset) -> BeatStatus {
        BeatStatus::NotRequested
    }
    /// Return detected beats mapped into project time.
    ///
    /// # Errors
    ///
    /// Returns a media error when timeline/source frame mapping fails.
    fn timeline_beats(
        &self,
        _document: &Document,
        _range: Option<std::ops::Range<TimeCode>>,
        _minimum_strength_basis_points: u16,
    ) -> Result<Vec<TimelineBeat>, MediaError> {
        Ok(Vec::new())
    }
    /// Return a uniform lifecycle view over every analysis family.
    fn analysis_jobs(&self, asset: &MediaAsset) -> Vec<AnalysisJobStatus> {
        analysis_job_statuses(self, asset)
    }
    /// Cooperatively cancel queued or running work. Repeated cancellation is harmless.
    fn cancel_analysis(&self, _asset: &MediaAsset, _kind: AnalysisKind) -> bool {
        false
    }
    /// Queue a content-addressed waveform extraction without blocking the caller.
    fn request_waveform(&self, asset: MediaAsset, request_generation: u64) -> bool;
    /// Queue one source-frame thumbnail without blocking the caller.
    fn request_thumbnail(
        &self,
        asset: MediaAsset,
        source_at: TimeCode,
        max_width: u32,
        request_generation: u64,
    ) -> bool;
    /// Bounded stream of ready waveform and thumbnail data.
    fn visual_asset_results(&self) -> Receiver<VisualAssetResult>;
    /// Return inventory for the fixed media-cache families. The default is an
    /// empty inventory for stateless test backends.
    fn cache_inventory(&self) -> MediaCacheInventory {
        MediaCacheInventory {
            families: Vec::new(),
        }
    }
    /// Clear one owned cache family. Implementations must not interpret this
    /// as permission to delete project files, source media, or model weights.
    ///
    /// # Errors
    ///
    /// Returns a media error when the cache family cannot be inspected or
    /// cleared safely, or when the implementation does not own that family.
    fn clear_cache(&self, _family: MediaCacheFamily) -> Result<MediaCacheClearResult, MediaError> {
        Err(MediaError::NotImplemented)
    }
}

/// Recheck every timeline-referenced source immediately before an export is
/// accepted or started.
///
/// The observation comes from the active [`Analysis`] backend rather than the
/// persisted document, because file availability and content identity are
/// machine-local and can change while a project is open. Only
/// [`MediaAvailabilityKind::OnlineVerified`] is accepted. In particular,
/// `OnlineUnverified` legacy sources are blocked until they have been
/// verified through import/relink, preventing a same-path replacement from
/// silently entering delivery. Audio sources use the same rule as visual
/// sources, while unused media-pool entries are excluded.
#[must_use]
pub fn export_media_preflight(
    document: &Document,
    analysis: &dyn Analysis,
) -> ExportMediaPreflightReport {
    export_media_preflight_with(document, |asset| analysis.media_availability(asset))
}

fn export_media_preflight_with(
    document: &Document,
    mut availability_for: impl FnMut(&MediaAsset) -> MediaAvailabilityStatus,
) -> ExportMediaPreflightReport {
    let mut checked_assets = Vec::new();
    let mut issues = Vec::new();
    for asset in document.timeline_referenced_media_assets() {
        checked_assets.push(asset.id);
        let availability = availability_for(asset);
        if availability.kind != MediaAvailabilityKind::OnlineVerified {
            issues.push(ExportMediaPreflightIssue {
                asset: asset.id,
                asset_name: asset.name.clone(),
                availability,
            });
        }
    }
    ExportMediaPreflightReport {
        checked_assets,
        issues,
    }
}

fn analysis_job_statuses<A: Analysis + ?Sized>(
    analysis: &A,
    asset: &MediaAsset,
) -> Vec<AnalysisJobStatus> {
    vec![
        transcript_job_status(asset.id, analysis.transcript_status(asset)),
        silence_job_status(asset.id, analysis.silence_status(asset)),
        scene_job_status(asset.id, analysis.scene_status(asset)),
        beat_job_status(asset.id, analysis.beat_status(asset)),
    ]
}

fn job(
    asset: AssetId,
    kind: AnalysisKind,
    phase: AnalysisPhase,
    progress_percent: Option<u8>,
    error: Option<String>,
) -> AnalysisJobStatus {
    AnalysisJobStatus {
        asset,
        kind,
        phase,
        progress_percent,
        error,
    }
}

fn transcript_job_status(asset: AssetId, status: TranscriptStatus) -> AnalysisJobStatus {
    match status {
        TranscriptStatus::NotRequested => job(
            asset,
            AnalysisKind::Transcript,
            AnalysisPhase::NotRequested,
            None,
            None,
        ),
        TranscriptStatus::Queued => job(
            asset,
            AnalysisKind::Transcript,
            AnalysisPhase::Queued,
            Some(0),
            None,
        ),
        TranscriptStatus::Hashing => job(
            asset,
            AnalysisKind::Transcript,
            AnalysisPhase::Hashing,
            None,
            None,
        ),
        TranscriptStatus::DownloadingModel {
            downloaded_bytes,
            total_bytes,
        } => job(
            asset,
            AnalysisKind::Transcript,
            AnalysisPhase::Downloading,
            total_bytes.and_then(|total| percent(downloaded_bytes, total)),
            None,
        ),
        TranscriptStatus::Transcribing { progress_percent } => job(
            asset,
            AnalysisKind::Transcript,
            AnalysisPhase::Analyzing,
            Some(progress_percent),
            None,
        ),
        TranscriptStatus::Ready(_) => job(
            asset,
            AnalysisKind::Transcript,
            AnalysisPhase::Ready,
            Some(100),
            None,
        ),
        TranscriptStatus::NoSpeech => job(
            asset,
            AnalysisKind::Transcript,
            AnalysisPhase::Unavailable,
            Some(100),
            None,
        ),
        TranscriptStatus::Cancelled => job(
            asset,
            AnalysisKind::Transcript,
            AnalysisPhase::Cancelled,
            None,
            None,
        ),
        TranscriptStatus::Failed(error) => job(
            asset,
            AnalysisKind::Transcript,
            AnalysisPhase::Failed,
            None,
            Some(error),
        ),
    }
}

fn silence_job_status(asset: AssetId, status: SilenceStatus) -> AnalysisJobStatus {
    let (phase, progress, error) = match status {
        SilenceStatus::NotRequested => (AnalysisPhase::NotRequested, None, None),
        SilenceStatus::Queued => (AnalysisPhase::Queued, Some(0), None),
        SilenceStatus::Hashing => (AnalysisPhase::Hashing, None, None),
        SilenceStatus::Analyzing => (AnalysisPhase::Analyzing, None, None),
        SilenceStatus::Ready(_) => (AnalysisPhase::Ready, Some(100), None),
        SilenceStatus::NoAudio => (AnalysisPhase::Unavailable, Some(100), None),
        SilenceStatus::Cancelled => (AnalysisPhase::Cancelled, None, None),
        SilenceStatus::Failed(error) => (AnalysisPhase::Failed, None, Some(error)),
    };
    job(asset, AnalysisKind::Silence, phase, progress, error)
}

fn scene_job_status(asset: AssetId, status: SceneStatus) -> AnalysisJobStatus {
    let (phase, progress, error) = match status {
        SceneStatus::NotRequested => (AnalysisPhase::NotRequested, None, None),
        SceneStatus::Queued => (AnalysisPhase::Queued, Some(0), None),
        SceneStatus::Hashing => (AnalysisPhase::Hashing, None, None),
        SceneStatus::Analyzing => (AnalysisPhase::Analyzing, None, None),
        SceneStatus::Ready(_) => (AnalysisPhase::Ready, Some(100), None),
        SceneStatus::NoVideo => (AnalysisPhase::Unavailable, Some(100), None),
        SceneStatus::Cancelled => (AnalysisPhase::Cancelled, None, None),
        SceneStatus::Failed(error) => (AnalysisPhase::Failed, None, Some(error)),
    };
    job(asset, AnalysisKind::Scene, phase, progress, error)
}

fn beat_job_status(asset: AssetId, status: BeatStatus) -> AnalysisJobStatus {
    let (phase, progress, error) = match status {
        BeatStatus::NotRequested => (AnalysisPhase::NotRequested, None, None),
        BeatStatus::Queued => (AnalysisPhase::Queued, Some(0), None),
        BeatStatus::Hashing => (AnalysisPhase::Hashing, None, None),
        BeatStatus::Analyzing { progress_percent } => {
            (AnalysisPhase::Analyzing, progress_percent, None)
        }
        BeatStatus::Ready(_) => (AnalysisPhase::Ready, Some(100), None),
        BeatStatus::NoAudio => (AnalysisPhase::Unavailable, Some(100), None),
        BeatStatus::Cancelled => (AnalysisPhase::Cancelled, None, None),
        BeatStatus::Failed(error) => (AnalysisPhase::Failed, None, Some(error)),
    };
    job(asset, AnalysisKind::Beat, phase, progress, error)
}

fn percent(value: u64, total: u64) -> Option<u8> {
    if total == 0 {
        return None;
    }
    Some(u8::try_from(value.saturating_mul(100).saturating_div(total).min(100)).unwrap_or(100))
}

pub trait Export: Send + Sync {
    /// Export the current document to a media file.
    ///
    /// # Errors
    ///
    /// Returns a media error when export fails or is cancelled.
    fn export(
        &self,
        out: &Path,
        settings: ExportSettings,
        progress: ProgressSink,
    ) -> Result<(), MediaError>;

    /// Export an explicit immutable document without replacing live playback.
    ///
    /// The default preserves compatibility for stateless test backends.
    /// Stateful production backends should override it.
    ///
    /// # Errors
    ///
    /// Returns a media error when export fails or is cancelled.
    fn export_document(
        &self,
        _document: Arc<Document>,
        out: &Path,
        settings: ExportSettings,
        progress: ProgressSink,
    ) -> Result<(), MediaError> {
        self.export(out, settings, progress)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Clip, ClipContent, MediaKind, Rational, Track, TrackKind};

    fn asset(id: u64, kind: MediaKind) -> MediaAsset {
        MediaAsset {
            id: AssetId(id),
            path: format!("fixture-{id}.mov").into(),
            name: format!("fixture-{id}"),
            duration: TimeCode(30),
            fps: Rational::new(30, 1).unwrap(),
            kind,
            resolution: Some((1920, 1080)),
            source_fingerprint: MediaSourceFingerprint::unknown(),
            color_description: ColorDescription::default(),
        }
    }

    fn media_clip(id: u64) -> Clip {
        Clip {
            id: ClipId(id),
            asset: AssetId(id),
            source_range: TimeCode::ZERO..TimeCode(30),
            content: ClipContent::Media,
            timeline_start: TimeCode::ZERO,
            effects: Vec::new(),
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
        }
    }

    fn document_with_video_audio_and_unused() -> Document {
        Document {
            tracks: vec![
                Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![media_clip(1)],
                },
                Track {
                    id: TrackId(2),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: vec![media_clip(2)],
                },
            ],
            media_pool: vec![
                asset(1, MediaKind::Video),
                asset(2, MediaKind::Audio),
                asset(3, MediaKind::AudioVideo),
            ],
            duration: TimeCode(30),
            ..Document::default()
        }
    }

    fn status(kind: MediaAvailabilityKind) -> MediaAvailabilityStatus {
        MediaAvailabilityStatus {
            kind,
            observed_fingerprint: None,
            reason: Some("fixture availability".to_owned()),
        }
    }

    #[test]
    fn export_preflight_checks_only_timeline_video_and_audio_sources() {
        let document = document_with_video_audio_and_unused();
        let mut observed = Vec::new();
        let report = export_media_preflight_with(&document, |asset| {
            observed.push(asset.id);
            status(MediaAvailabilityKind::OnlineVerified)
        });

        assert!(report.export_ready());
        assert_eq!(report.checked_assets, vec![AssetId(1), AssetId(2)]);
        assert_eq!(observed, vec![AssetId(1), AssetId(2)]);
    }

    #[test]
    fn export_preflight_blocks_changed_and_legacy_unverified_sources() {
        let document = document_with_video_audio_and_unused();
        let report = export_media_preflight_with(&document, |asset| match asset.id {
            AssetId(1) => status(MediaAvailabilityKind::Changed),
            AssetId(2) => status(MediaAvailabilityKind::OnlineUnverified),
            _ => unreachable!("unused assets are not observed"),
        });

        assert!(!report.export_ready());
        assert_eq!(report.issues.len(), 2);
        assert_eq!(report.issues[0].asset, AssetId(1));
        assert_eq!(
            report.issues[0].availability.kind,
            MediaAvailabilityKind::Changed
        );
        assert_eq!(report.issues[1].asset, AssetId(2));
        assert_eq!(
            report.issues[1].availability.kind,
            MediaAvailabilityKind::OnlineUnverified
        );
        assert!(report.summary().contains("relink or recovery"));
    }
}
