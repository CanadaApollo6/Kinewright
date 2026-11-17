//! The MS1 profile is shared with the source builder. Refuse before probe decoding.
use crate::ffmpeg;
use kinewright_core::MediaError;
use std::{ffi::CString, sync::LazyLock};

#[derive(serde::Deserialize)]
struct Profile {
    decoders: Vec<String>,
    demuxers: Vec<String>,
}
static PROFILE: LazyLock<Profile> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../scripts/ffmpeg-profile.json"))
        .expect("compiled FFmpeg profile")
});
pub(crate) static CODECS: LazyLock<CString> =
    LazyLock::new(|| CString::new(PROFILE.decoders.join(",")).expect("codec names"));
pub(crate) static FORMATS: LazyLock<CString> = LazyLock::new(|| {
    // libavformat's registered name differs from its configure component name.
    CString::new(
        PROFILE
            .demuxers
            .iter()
            .map(|name| match name.as_str() {
                "image_png_pipe" => "png_pipe",
                "image_jpeg_pipe" => "jpeg_pipe",
                "matroska" => "matroska,webm",
                name => name,
            })
            .collect::<Vec<_>>()
            .join(","),
    )
    .expect("format names")
});

pub(crate) fn check(id: ffmpeg::codec::Id) -> Result<(), MediaError> {
    let name = id.name();
    if PROFILE
        .decoders
        .iter()
        .any(|decoder| decoder == name || (decoder == "libdav1d" && name == "av1"))
    {
        Ok(())
    } else {
        Err(MediaError::UnsupportedCodec { codec: name.into() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{TempDirectory, run_ffmpeg};

    #[test]
    fn ms1_fixture_video_codecs_decode() {
        crate::initialize_ffmpeg().unwrap();
        let directory = TempDirectory::new("ms1-video-codecs");
        for (encoder, container) in [
            ("libx264", "mov"),
            ("libx265", "mov"),
            ("ffv1", "mkv"),
            ("mpeg4", "avi"),
            ("mpeg2video", "mkv"),
            ("mjpeg", "mov"),
            ("png", "mov"),
            ("rawvideo", "nut"),
            ("libvpx-vp9", "mkv"),
        ] {
            let path = directory.path(&format!("{encoder}.{container}"));
            run_ffmpeg(
                &[
                    "-f",
                    "lavfi",
                    "-i",
                    "color=s=32x32:r=5:d=0.4",
                    "-c:v",
                    encoder,
                    "-threads",
                    "1",
                ],
                &path,
            );
            let mut input = crate::decode::media_input(&path).unwrap();
            let stream = input.streams().best(ffmpeg::media::Type::Video).unwrap();
            let index = stream.index();
            let mut decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
                .unwrap()
                .decoder()
                .video()
                .unwrap();
            let mut frame = ffmpeg::frame::Video::empty();
            let mut produced_frame = false;
            for (stream, packet) in input.packets() {
                if stream.index() == index {
                    decoder.send_packet(&packet).unwrap();
                    produced_frame |= decoder.receive_frame(&mut frame).is_ok();
                }
            }
            decoder.send_eof().unwrap();
            produced_frame |= decoder.receive_frame(&mut frame).is_ok();
            assert!(produced_frame, "{encoder} must actually produce a frame");
        }
    }

    #[test]
    fn ms1_fixture_audio_codecs_decode() {
        crate::initialize_ffmpeg().unwrap();
        let directory = TempDirectory::new("ms1-audio-codecs");
        for codec in ["aac", "pcm_s16le", "pcm_f32le"] {
            let path = directory.path(&format!("{codec}.mov"));
            run_ffmpeg(
                &["-f", "lavfi", "-i", "sine=duration=0.1", "-c:a", codec],
                &path,
            );
            let mut input = crate::decode::media_input(&path).unwrap();
            let stream = input.streams().best(ffmpeg::media::Type::Audio).unwrap();
            let index = stream.index();
            let mut decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
                .unwrap()
                .decoder()
                .audio()
                .unwrap();
            let mut frame = ffmpeg::frame::Audio::empty();
            let mut produced_frame = false;
            for (stream, packet) in input.packets() {
                if stream.index() == index {
                    decoder.send_packet(&packet).unwrap();
                    produced_frame |= decoder.receive_frame(&mut frame).is_ok();
                }
            }
            assert!(produced_frame, "{codec} must actually produce samples");
        }
    }
}
