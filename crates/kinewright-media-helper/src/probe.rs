#[cfg(test)]
#[path = "../../kinewright-job-sys/test_support/deadline.rs"]
mod deadline;

use crate::{avio::Input, metadata, still};
use ffmpeg_next as ffmpeg;
use kinewright_core::{ColorDescription, Rational};
use kinewright_job_sys::media_boundary::MediaFile;
use kinewright_media_ipc::{
    Color, Kind, MediaSandboxError as Error, ProbeResult, Stream, StreamKind,
};
use std::sync::{Arc, atomic::AtomicBool};

/// Metadata and the live native buffers which produced it. Keeping them until
/// Close permits host measurement at Complete and makes teardown observable.
pub struct Probed {
    pub metadata: ProbeResult,
    pub retention: Retention,
}
impl std::fmt::Debug for Probed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Probed")
            .field("metadata", &self.metadata)
            .finish_non_exhaustive()
    }
}
pub struct Retention {
    // Field order releases accepted frames/decoders before their format/AVIO.
    _video: Option<(ffmpeg::frame::Video, ffmpeg::decoder::Video)>,
    _audio: Option<ffmpeg::decoder::Audio>,
    _input: Input,
}

#[allow(
    clippy::too_many_lines,
    reason = "one source-bound admission probe through first-frame decode"
)]
pub fn probe(
    file: MediaFile,
    length: u64,
    selection: [Option<u32>; 2],
    budget: u64,
    cancel: Arc<AtomicBool>,
    mut decoder_checkpoint: impl FnMut(),
) -> Result<Probed, Error> {
    let mut input = Input::open(file, length, budget, cancel)?;
    let original: Vec<_> = input
        .format
        .streams()
        .map(|s| s.parameters().medium())
        .collect();
    if original.is_empty()
        || original.len() > 16
        || original.iter().any(|kind| {
            !matches!(
                kind,
                ffmpeg::media::Type::Video | ffmpeg::media::Type::Audio
            )
        })
    {
        return Err(Error::UnsupportedMedia);
    }
    input.find_stream_info(budget)?;
    if input
        .format
        .streams()
        .map(|s| s.parameters().medium())
        .ne(original)
    {
        return Err(Error::UnsupportedMedia);
    }
    let choose = |requested: Option<u32>, kind| -> Result<Option<usize>, Error> {
        if selection != [None, None] {
            return requested
                .map(|index| {
                    let index = usize::try_from(index).map_err(|_| Error::UnsupportedMedia)?;
                    if input
                        .format
                        .stream(index)
                        .is_none_or(|s| s.parameters().medium() != kind)
                    {
                        return Err(Error::UnsupportedMedia);
                    }
                    Ok(index)
                })
                .transpose();
        }
        Ok(input.format.streams().best(kind).map(|s| s.index()))
    };
    let video = choose(selection[0], ffmpeg::media::Type::Video)?;
    let audio = choose(selection[1], ffmpeg::media::Type::Audio)?;
    let mut kind = match (video, audio) {
        (Some(_), Some(_)) => Kind::AudioVideo,
        (Some(_), None) => Kind::Video,
        (None, Some(_)) => Kind::Audio,
        _ => return Err(Error::UnsupportedMedia),
    };
    let streams = input
        .format
        .streams()
        .map(|s| {
            let (kind, rate) = if s.parameters().medium() == ffmpeg::media::Type::Video {
                (StreamKind::Video, stream_fps(s.avg_frame_rate()))
            } else {
                (StreamKind::Audio, [0, 1])
            };
            Stream {
                index: u32::try_from(s.index()).unwrap_or(u32::MAX),
                kind,
                time_base: [s.time_base().numerator(), s.time_base().denominator()],
                timestamp_origin: if s.start_time() == i64::MIN {
                    0
                } else {
                    s.start_time()
                },
                duration: s.duration().max(0),
                average_frame_rate: rate,
            }
        })
        .collect();
    let mut fps = Rational::default();
    let mut duration_microseconds = input.format.duration();
    let mut duration = metadata::duration_us_to_frames(duration_microseconds, fps);
    let mut resolution = None;
    let mut color = ColorDescription::unknown();
    let mut retained_video = None;
    let mut retained_audio = None;
    if let Some(index) = video {
        let stream = input.format.stream(index).ok_or(Error::UnsupportedMedia)?;
        let time_base = stream.time_base();
        let stream_duration = stream.duration();
        let mut rotation =
            metadata::stream_rotation(&stream).map_err(|_| Error::UnsupportedMedia)?;
        let rate = [stream.avg_frame_rate(), stream.rate()]
            .into_iter()
            .find_map(|rate| {
                Rational::new(
                    u32::try_from(rate.numerator()).ok()?,
                    u32::try_from(rate.denominator()).ok()?,
                )
                .ok()
            });
        let context = Input::decoder_context(stream.parameters(), budget)?;
        let mut decoder = context
            .decoder()
            .video()
            .map_err(|_| Error::UnsupportedMedia)?;
        let still_image = audio.is_none() && metadata::is_still_image_codec(decoder.id());
        let mut frame = ffmpeg::frame::Video::empty();
        let mut eof = false;
        loop {
            match decoder.receive_frame(&mut frame) {
                Ok(()) => break,
                Err(ffmpeg::Error::Other {
                    errno: ffmpeg::error::EAGAIN,
                }) if !eof => {}
                _ => return Err(Error::UnsupportedMedia),
            }
            if let Some(packet) = input.packet(index)? {
                decoder
                    .send_packet(&packet)
                    .map_err(|_| Error::UnsupportedMedia)?;
                decoder_checkpoint();
            } else {
                decoder.send_eof().map_err(|_| Error::UnsupportedMedia)?;
                eof = true;
            }
        }
        input.check_frame(index, &decoder, &frame, budget)?;
        if still_image {
            if let Some(orientation) = still::still_bytes_orientation(&input.header) {
                rotation = orientation.rotation;
            }
            kind = Kind::Image;
            duration = 1;
            // A still occupies one frame on the probe's nominal grid. File
            // demuxers may report no duration for it.
            let divisor = i64::from(fps.numerator());
            duration_microseconds =
                (1_000_000 * i64::from(fps.denominator()) + divisor - 1) / divisor;
        } else {
            fps = rate.ok_or(Error::UnsupportedMedia)?;
            duration = metadata::timestamp_to_grid_ceil(stream_duration, time_base, fps);
            if duration <= 0 {
                duration = metadata::duration_us_to_frames(duration_microseconds, fps);
            }
        }
        let (w, h) = rotation.display_dimensions(frame.width(), frame.height());
        resolution = Some([w, h]);
        color = metadata::color_description_from_decoder(&decoder, Some(frame.format()));
        retained_video = Some((frame, decoder));
    } else if let Some(index) = audio {
        let stream = input.format.stream(index).ok_or(Error::UnsupportedMedia)?;
        retained_audio = Some(
            Input::decoder_context(stream.parameters(), budget)?
                .decoder()
                .audio()
                .map_err(|_| Error::UnsupportedMedia)?,
        );
    }
    input.check()?;
    let result = ProbeResult {
        streams,
        duration_microseconds,
        video_stream: video.and_then(|v| u32::try_from(v).ok()),
        audio_stream: audio.and_then(|a| u32::try_from(a).ok()),
        duration_frames: duration,
        fps: [fps.numerator(), fps.denominator()],
        kind,
        resolution,
        color: Color {
            primaries: color.primaries,
            transfer: color.transfer,
            matrix: color.matrix,
            range: color.range,
            white_point: color.white_point,
            bit_depth: color.bit_depth,
            confidence_basis_points: color.confidence_basis_points,
            provenance: color.provenance,
        },
    };
    let metadata = kinewright_media_ipc::ValidatedProbe::new(result)
        .map(kinewright_media_ipc::ValidatedProbe::into_metadata)
        .map_err(|_| Error::UnsupportedMedia)?;
    Ok(Probed {
        metadata,
        retention: Retention {
            _video: retained_video,
            _audio: retained_audio,
            _input: input,
        },
    })
}

fn stream_fps(rate: ffmpeg::Rational) -> [i32; 2] {
    let [n, d] = [rate.numerator(), rate.denominator()];
    if n == 0 && d >= 0 {
        [0, 1]
    } else if n > 0 && d > 0 {
        let reduced = rate.reduce();
        [reduced.numerator(), reduced.denominator()]
    } else {
        [n, d]
    }
}

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use super::*;
    use kinewright_job_sys::media_boundary::{self as boundary, Mode};
    use std::{
        fs,
        io::{Seek, SeekFrom},
        os::fd::AsRawFd,
        process::{Command, Stdio},
    };

    #[test]
    fn cancellation_fixture() {
        let Ok(case) = std::env::var("KW_PROBE_CANCEL_FIXTURE") else {
            return;
        };
        // SAFETY: the fixture launcher alone installs the inherited source at fd 6.
        let source = unsafe { boundary::inherited_file(boundary::SOURCE) };
        let length = boundary::source_length().unwrap();
        ffmpeg::init().unwrap();
        ffmpeg::log::set_level(ffmpeg::log::Level::Quiet);
        let cancelled = case == "cancelled";
        let cancel = Arc::new(AtomicBool::new(cancelled));
        boundary::install_filter(Mode::Degraded).unwrap();
        let result = probe(
            source,
            length,
            [None, None],
            2 * 1024 * 1024 * 1024,
            cancel,
            || {},
        );
        if cancelled {
            assert!(matches!(result, Err(Error::Cancelled)));
        } else {
            assert!(result.is_ok(), "{result:?}");
        }
        // libtest writes to stdout after a test; the real response-only policy
        // forbids that. Exit only after probe/decoder/source teardown has finished.
        boundary::exit_helper(0);
    }

    #[test]
    fn final_filter_cancels_native_open_and_control_decodes_and_closes() {
        let _deadline = deadline::test();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ms2-s1a")
            .join(format!("cancel-{}.png", std::process::id()));
        let fixture = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=size=64x32:rate=30",
                "-frames:v",
                "1",
                "-c:v",
                "png",
                "-threads",
                "1",
            ])
            .arg(&root)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(fixture.status.success(), "{fixture:?}");
        let mut source = fs::File::open(&root).unwrap();
        let end = source.seek(SeekFrom::End(0)).unwrap();
        let (command_read, _command_write) =
            rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
        let (_response_read, response_write) =
            rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
        let (lifetime_read, _lifetime_write) =
            rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
        let (release_read, _release_write) =
            rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
        for case in ["cancelled", "control"] {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "probe::tests::cancellation_fixture",
                    "--nocapture",
                ])
                .env_clear()
                .env("KW_PROBE_CANCEL_FIXTURE", case)
                .stdin(Stdio::null());
            // SAFETY: these originals stay owned through spawn; the audited wrapper
            // installs child-only copies and never transfers the parent's ownership.
            unsafe {
                boundary::inherit_media_fds(
                    &mut command,
                    [
                        command_read.as_raw_fd(),
                        response_write.as_raw_fd(),
                        lifetime_read.as_raw_fd(),
                        source.as_raw_fd(),
                        release_read.as_raw_fd(),
                    ],
                    None,
                );
            }
            let output = command.output().unwrap();
            assert!(output.status.success(), "{case}: {output:?}");
            assert_eq!(
                source.stream_position().unwrap(),
                end,
                "helper changed shared OFD offset"
            );
        }
        fs::remove_file(root).unwrap();
    }
}
