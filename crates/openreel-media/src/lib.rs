//! `FFmpeg` decode, frame caching, and cpal audio-master playback.

mod analysis;
mod audio;
mod cache;
mod clock;
mod compositor;
mod decode;
mod derived;
mod derived_cache;
mod engine;
mod export;
mod render;
mod sha256;
mod timeline;
mod transcript;

#[cfg(any(test, feature = "test-util"))]
pub mod test_support;

#[cfg(test)]
mod media_matrix_tests;

use ffmpeg_next as ffmpeg;
use openreel_core::MediaError;

pub use analysis::{MAX_THUMBNAIL_BYTES, MAX_THUMBNAIL_FILES, MAX_WAVEFORM_PEAKS};
pub use cache::select_frame_for_position;
pub use clock::{frame_to_samples, samples_to_frame};
pub use compositor::{Compositor, CompositorLayer, GpuContext};
pub use derived::{
    DEFAULT_MINIMUM_SILENCE_FRAMES, DEFAULT_SCENE_CONFIDENCE_BASIS_POINTS,
    DEFAULT_SCENE_PROXY_WIDTH, DEFAULT_SILENCE_THRESHOLD_DBFS_HUNDREDTHS,
    DEFAULT_SILENCE_WINDOW_MILLISECONDS, DerivedAnalysisConfig, SceneDetectionConfig,
    SilenceDetectionConfig,
};
pub use engine::FfmpegMediaEngine;
pub use openreel_core::{
    ThumbnailFrame, ThumbnailKey, VisualAssetResult, VisualRequestKind, WaveformData, WaveformPeak,
};
pub use timeline::{TimelineSource, TimelineVideoLayer, timeline_source_at, video_layers_at};
pub use transcript::{
    WHISPER_MODEL_LICENSE, WHISPER_MODEL_NAME, WHISPER_MODEL_SHA256, WHISPER_MODEL_URL,
    default_data_dir,
};

/// Initialize the linked `FFmpeg` libraries once for the current process.
///
/// # Errors
///
/// Returns a media error when `FFmpeg` initialization fails.
pub fn initialize_ffmpeg() -> Result<(), MediaError> {
    ffmpeg::init().map_err(|error| MediaError::Backend(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linked_ffmpeg_initializes() {
        initialize_ffmpeg().expect("the linked FFmpeg libraries should initialize");
    }
}
