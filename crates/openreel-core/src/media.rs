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
            Self::Queued | Self::Hashing | Self::DownloadingModel { .. } | Self::Transcribing { .. }
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

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MediaError {
    #[error("media operation is not implemented")]
    NotImplemented,
    #[error("export was cancelled")]
    Cancelled,
    #[error("media backend error: {0}")]
    Backend(String),
}

pub trait MediaEngine: Send + Sync {
    fn probe(&self, path: &Path) -> Result<MediaAsset, MediaError>;
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
    /// Queue derived speech recognition without blocking the caller. Repeated
    /// requests for the same asset are coalesced by the implementation.
    fn request_transcription(&self, asset: MediaAsset);
    /// Return the latest state for an asset's derived transcript.
    fn transcript_status(&self, asset: AssetId) -> TranscriptStatus;
    /// Return words currently audible on the timeline, optionally restricted
    /// to a half-open range of project frames.
    fn timeline_transcript(
        &self,
        document: &Document,
        range: Option<std::ops::Range<TimeCode>>,
    ) -> Result<Vec<TimelineTranscriptWord>, MediaError>;
    fn thumbnail_at(&self, t: TimeCode, max_w: u32) -> Result<RgbaImage, MediaError>;
    fn export(
        &self,
        out: &Path,
        settings: ExportSettings,
        progress: ProgressSink,
    ) -> Result<(), MediaError>;
}

