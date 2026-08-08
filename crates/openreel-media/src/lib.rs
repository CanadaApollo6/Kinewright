use std::{path::Path, sync::Arc};

use crossbeam_channel::{Receiver, Sender, unbounded};
use ffmpeg_next as ffmpeg;
use openreel_core::{
    Document, ExportSettings, FrameTexture, MediaAsset, MediaEngine, MediaError, ProgressSink,
    RgbaImage, TimeCode,
};

/// Initialize the linked FFmpeg libraries.
pub fn initialize_ffmpeg() -> Result<(), MediaError> {
    ffmpeg::init().map_err(|error| MediaError::Backend(error.to_string()))
}

/// M0 implementation. Decode, playback, and export arrive in later milestones.
pub struct StubMediaEngine {
    frames_tx: Sender<(TimeCode, FrameTexture)>,
    frames_rx: Receiver<(TimeCode, FrameTexture)>,
}

impl StubMediaEngine {
    #[must_use]
    pub fn new() -> Self {
        let (frames_tx, frames_rx) = unbounded();
        Self {
            frames_tx,
            frames_rx,
        }
    }
}

impl Default for StubMediaEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl MediaEngine for StubMediaEngine {
    fn probe(&self, _path: &Path) -> Result<MediaAsset, MediaError> {
        Err(MediaError::NotImplemented)
    }

    fn set_document(&self, _doc: Arc<Document>) {}

    fn request_frame(&self, _t: TimeCode) {
        let _ = &self.frames_tx;
    }

    fn frames(&self) -> Receiver<(TimeCode, FrameTexture)> {
        self.frames_rx.clone()
    }

    fn play(&self, _from: TimeCode) {}

    fn pause(&self) {}

    fn thumbnail_at(&self, _t: TimeCode, _max_w: u32) -> Result<RgbaImage, MediaError> {
        Err(MediaError::NotImplemented)
    }

    fn export(
        &self,
        _out: &Path,
        _settings: ExportSettings,
        _progress: ProgressSink,
    ) -> Result<(), MediaError> {
        Err(MediaError::NotImplemented)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linked_ffmpeg_initializes() {
        initialize_ffmpeg().expect("the linked FFmpeg libraries should initialize");
    }
}

