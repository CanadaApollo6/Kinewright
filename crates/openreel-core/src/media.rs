use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crossbeam_channel::{Receiver, Sender};
use thiserror::Error;

use crate::{Document, MediaAsset, Rational, TimeCode};

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
    fn thumbnail_at(&self, t: TimeCode, max_w: u32) -> Result<RgbaImage, MediaError>;
    fn export(
        &self,
        out: &Path,
        settings: ExportSettings,
        progress: ProgressSink,
    ) -> Result<(), MediaError>;
}

