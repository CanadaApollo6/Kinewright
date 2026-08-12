use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crossbeam_channel::{Receiver, Sender};
use thiserror::Error;

use crate::{AssetId, ClipId, Document, MediaAsset, Rational, TimeCode, TrackId};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WaveformPeak {
    pub minimum: i16,
    pub maximum: i16,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WaveformData {
    pub asset: AssetId,
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
    pub video_codec: String,
    pub audio_codec: String,
    pub video_bitrate: u64,
    pub audio_bitrate: u64,
    pub cancellation: ExportCancellation,
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

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MediaError {
    #[error("media operation is not implemented")]
    NotImplemented,
    #[error("export was cancelled")]
    Cancelled,
    #[error("media backend error: {0}")]
    Backend(String),
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
    /// Decode a thumbnail at an exact project frame.
    ///
    /// # Errors
    ///
    /// Returns a media error when decoding or compositing fails.
    fn thumbnail_at(&self, t: TimeCode, max_w: u32) -> Result<RgbaImage, MediaError>;
    /// Queue derived speech recognition without blocking the caller. Repeated
    /// requests for the same asset are coalesced by the implementation.
    fn request_transcription(&self, asset: MediaAsset);
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
    /// Queue a content-addressed waveform extraction without blocking the caller.
    fn request_waveform(&self, asset: MediaAsset) -> bool;
    /// Queue one source-frame thumbnail without blocking the caller.
    fn request_thumbnail(&self, asset: MediaAsset, source_at: TimeCode, max_width: u32) -> bool;
    /// Bounded stream of ready waveform and thumbnail data.
    fn visual_asset_results(&self) -> Receiver<VisualAssetResult>;
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
}
