//! FFmpeg decode, frame caching, and cpal audio-master playback.

mod audio;
mod cache;
mod clock;
mod decode;
mod engine;

use ffmpeg_next as ffmpeg;
use openreel_core::MediaError;

pub use cache::select_frame_for_position;
pub use clock::{frame_to_samples, samples_to_frame};
pub use engine::FfmpegMediaEngine;

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
