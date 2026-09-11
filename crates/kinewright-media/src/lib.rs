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
mod dsp;
mod engine;
mod export;
mod frame;
mod loudness;
mod lut;
mod lut_store;
mod render;
mod room_tone_store;
mod sha256;
mod spectrum;
mod timeline;
mod title;
mod transcript;
mod verify;

#[cfg(any(test, feature = "test-util"))]
pub mod test_support;

/// CC7 §3's source generators. Gated exactly as [`test_support`] is, and for
/// the same reason: it depends on `test_support`'s `GeneratedMedia` and
/// `run_ffmpeg`, which are themselves behind this feature.
#[cfg(any(test, feature = "test-util"))]
pub mod cc7_sources;

/// AU6 §3's source generators, gated exactly as [`cc7_sources`] is and for the
/// same reason: every buffer is written through `test_support`'s
/// `GeneratedMedia`, which is itself behind this feature.
#[cfg(any(test, feature = "test-util"))]
pub mod au6_sources;

#[cfg(test)]
mod media_matrix_tests;

#[cfg(test)]
mod cc1_fixtures;

#[cfg(test)]
mod cc3_fixtures;

#[cfg(test)]
mod cc4_fixtures;

#[cfg(test)]
mod cc5_fixtures;

#[cfg(test)]
mod cc6_fixtures;

#[cfg(test)]
mod cc7_fixtures;

/// AU5 §5.4's seam lanes. They live in `src/` rather than in
/// `tests/au5_fixtures.rs` for exactly AU5 §0 R62's reason and are recorded as
/// R100: the seam pin compares `export::mix_audio`'s samples against a
/// per-piece decode, and `mix_audio` is `pub(crate)` (export.rs:1078), behind
/// the `test-util` feature no default build enables (AU6 §12.1 item 9(b):
/// `test_support` is a `pub mod`, and the clause that said otherwise is gone).
#[cfg(test)]
mod au5b_fixtures;

/// AU6 §11's media gates and the audio programme's first fixture manifest.
/// In `src/` for A9/E9's reason: `mix_audio_stems` and `MixStems` are
/// `pub(crate)` (export.rs:999, :1099), so the stem-identity gates of §4(a)(5)
/// and §4(d)(1) are unreachable from `tests/`, and AU6 does not widen the
/// public surface to reach them.
#[cfg(test)]
mod au6_fixtures;

#[cfg(test)]
mod gpu_test_support;

use ffmpeg_next as ffmpeg;
use kinewright_core::MediaError;

pub use analysis::{MAX_THUMBNAIL_BYTES, MAX_THUMBNAIL_FILES, MAX_WAVEFORM_PEAKS};
pub use audio::{decode_audio_range, hum_removal_magnitude_db, parametric_eq_magnitude_db};
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
    Compositor, CompositorLayer, DeliveryFrame, GpuContext, MatteRenderTarget,
    compositor_required_limits,
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
pub use loudness::{LOUDNESS_GATING_BLOCK_FRAMES, LoudnessMeter, measure_loudness};
pub use lut::{
    CubeLut, LutParseError, LutParseErrorCode, MAX_CUBE_SIZE, MIN_CUBE_SIZE, parse_cube_lut,
    parse_cube_lut_bytes, parse_cube_lut_typed,
};
pub use lut_store::{
    LUT_MAX_FILE_BYTES, LUT_STORE_LUTS_DIRECTORY, LUT_STORE_SUFFIX, LutAssetImport, LutLibrary,
    LutStore, LutStoreError, LutStoreErrorCode, metadata_mismatch,
};
pub use room_tone_store::{
    ROOM_TONE_ASSET_FRAMES_PER_SECOND, ROOM_TONE_CHANNELS, ROOM_TONE_MAX_CAPTURE_MILLISECONDS,
    ROOM_TONE_MAX_FILE_BYTES, ROOM_TONE_MINIMUM_CAPTURE_MILLISECONDS,
    ROOM_TONE_SAMPLE_FRAMES_PER_ASSET_FRAME, ROOM_TONE_SAMPLE_RATE, ROOM_TONE_STORE_DIRECTORY,
    RoomToneAvailabilityKind, RoomToneAvailabilityStatus, RoomToneCapture, RoomToneStore,
    RoomToneStoreError, RoomToneStoreErrorCode,
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
pub use verify::{DELIVERY_REFERENCE_DENOMINATOR, EBU_R103_TOLERANCE_CODES_8BIT, NativePlaneFrame};

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
