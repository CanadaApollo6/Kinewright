//! FFmpeg decode, frame caching, and cpal audio-master playback.

mod audio;
mod cache;
mod clock;
mod compositor;
mod decode;
mod engine;
mod export;
mod render;
mod sha256;
mod timeline;
mod transcript;

use ffmpeg_next as ffmpeg;
use openreel_core::MediaError;

pub use cache::select_frame_for_position;
pub use clock::{frame_to_samples, samples_to_frame};
pub use engine::FfmpegMediaEngine;
pub use compositor::{Compositor, CompositorLayer, GpuContext};
pub use timeline::{TimelineSource, TimelineVideoLayer, timeline_source_at, video_layers_at};
pub use transcript::{
    WHISPER_MODEL_LICENSE, WHISPER_MODEL_NAME, WHISPER_MODEL_SHA256, WHISPER_MODEL_URL,
    default_data_dir,
};

/// Initialize the linked FFmpeg libraries.
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
