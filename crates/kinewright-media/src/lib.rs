//! `FFmpeg` decode, frame caching, and cpal audio-master playback.

mod analysis;
mod audio;
mod builtin_looks;
mod cache;
mod clock;
pub mod color_pipeline;
mod compositor;
mod decode;
mod derived;
mod derived_cache;
mod engine;
mod export;
mod frame;
mod loudness;
mod lut;
mod lut_store;
mod render;
mod sha256;
mod timeline;
mod title;
mod transcript;

#[cfg(any(test, feature = "test-util"))]
pub mod test_support;

#[cfg(test)]
mod media_matrix_tests;

#[cfg(test)]
mod cc1_fixtures;

#[cfg(test)]
mod cc3_fixtures;

#[cfg(test)]
mod cc4_fixtures;

#[cfg(test)]
mod gpu_test_support;

use ffmpeg_next as ffmpeg;
use kinewright_core::MediaError;

pub use analysis::{MAX_THUMBNAIL_BYTES, MAX_THUMBNAIL_FILES, MAX_WAVEFORM_PEAKS};
pub use builtin_looks::{
    BUILTIN_IDENTITY_SIZE, BUILTIN_LOOK_DOMAIN_MAX, BUILTIN_LOOK_DOMAIN_MIN, BUILTIN_LOOK_SHA256,
    BUILTIN_LOOK_SIZE, BuiltinLook,
};
pub use cache::select_frame_for_position;
pub use clock::{frame_to_samples, samples_to_frame};
pub use compositor::{
    COMPOSITOR_LEGACY_LUT_SLOT, COMPOSITOR_LUT_ATLAS_SLOTS, COMPOSITOR_LUT_SLOTS_PER_LAYER,
    COMPOSITOR_REQUIRED_STORAGE_BUFFER_BINDING_SIZE,
    COMPOSITOR_REQUIRED_STORAGE_BUFFERS_PER_SHADER_STAGE, COMPOSITOR_REQUIRED_TEXTURE_DIMENSION_3D,
    Compositor, CompositorLayer, DeliveryFrame, GpuContext, compositor_required_limits,
};
pub use derived::{
    BeatDetectionConfig, DEFAULT_BEAT_MINIMUM_INTERVAL_MILLISECONDS,
    DEFAULT_BEAT_WINDOW_MILLISECONDS, DEFAULT_MINIMUM_SILENCE_FRAMES,
    DEFAULT_SCENE_CONFIDENCE_BASIS_POINTS, DEFAULT_SCENE_PROXY_WIDTH,
    DEFAULT_SILENCE_THRESHOLD_DBFS_HUNDREDTHS, DEFAULT_SILENCE_WINDOW_MILLISECONDS,
    DerivedAnalysisConfig, SceneDetectionConfig, SilenceDetectionConfig,
};
pub use engine::FfmpegMediaEngine;
pub use kinewright_core::{
    LutAvailabilityKind, LutAvailabilityStatus, MediaAvailabilityKind, MediaAvailabilityStatus,
    MediaCacheClearResult, MediaCacheFamily, MediaCacheFamilyStatus, MediaCacheInventory,
    ThumbnailFrame, ThumbnailKey, VisualAssetResult, VisualRequestKind, WaveformData, WaveformPeak,
};
pub use loudness::measure_loudness;
pub use lut::{
    CubeLut, LutParseError, LutParseErrorCode, MAX_CUBE_SIZE, MIN_CUBE_SIZE, parse_cube_lut,
    parse_cube_lut_bytes, parse_cube_lut_typed,
};
pub use lut_store::{
    LUT_MAX_FILE_BYTES, LUT_STORE_LUTS_DIRECTORY, LUT_STORE_SUFFIX, LutAssetImport, LutLibrary,
    LutStore, LutStoreError, LutStoreErrorCode, metadata_mismatch,
};
pub use sha256::{sha256_bytes, sha256_file, source_fingerprint};
pub use timeline::{
    TimelineAudioSegment, TimelineSource, TimelineTitleLayer, TimelineVideoLayer,
    TimelineVisualLayer, TransitionRenderParams, timeline_audio_segments, timeline_source_at,
    video_layers_at, visual_layers_at,
};
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
