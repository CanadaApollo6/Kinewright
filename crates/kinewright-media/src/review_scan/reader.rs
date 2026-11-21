//! Decoded artifact streams on the output clock: the recorded streams only,
//! with the resampler drained at the end of the stream.
use std::fs::File;
use std::io::Seek as _;

use ffmpeg_next as ffmpeg;
use kinewright_core::{ExportCancellation, MediaError};

use crate::decode::backend;

#[allow(clippy::needless_pass_by_value, reason = "used as a map_err adapter")]
pub(super) fn io_error(error: std::io::Error) -> MediaError {
    MediaError::Backend(format!("review artifact read failed: {error}"))
}

/// Open the artifact from offset 0 through a fresh descriptor. Passes run
/// one at a time, so the shared file offset is never read concurrently.
pub(super) fn open_input(
    artifact: &File,
    cancellation: &ExportCancellation,
) -> Result<ffmpeg::format::context::Input, MediaError> {
    crate::quiet_ffmpeg_log();
    let mut file = artifact.try_clone().map_err(io_error)?;
    file.rewind().map_err(io_error)?;
    let io = ffmpeg::format::context::StreamIo::from_read_seek(file).map_err(backend)?;
    let cancel = cancellation.clone();
    ffmpeg::format::input_from_stream_with_interrupt(io, Some("review.mp4"), None, move || {
        cancel.is_cancelled()
    })
    .map_err(backend)
}

/// Exact PTS step of one output frame, if the stream time base allows one.
pub(super) fn frame_step(time_base: (i32, i32), fps: kinewright_core::Rational) -> Option<i64> {
    let numerator = i128::from(time_base.1) * i128::from(fps.denominator());
    let denominator = i128::from(time_base.0) * i128::from(fps.numerator());
    (denominator > 0 && numerator % denominator == 0)
        .then(|| i64::try_from(numerator / denominator).ok())
        .flatten()
        .filter(|step| *step > 0)
}

/// Audio streams in the container.
pub(super) fn audio_streams(input: &ffmpeg::format::context::Input) -> u32 {
    let count = input
        .streams()
        .filter(|stream| stream.parameters().medium() == ffmpeg::media::Type::Audio)
        .count();
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// Pull decoded audio of the recorded stream, converted to planar F32 at its
/// own rate and channel count, with PTS mapped to output samples from the
/// first picture.
pub(super) struct AudioReader {
    input: ffmpeg::format::context::Input,
    stream: usize,
    time_base: ffmpeg::Rational,
    decoder: ffmpeg::decoder::Audio,
    resampler: Option<ffmpeg::software::resampling::Context>,
    pub rate: u32,
    pub channels: u16,
    /// Samples per coded frame, when the codec has a fixed one.
    pub frame_size: u32,
    pub streams: u32,
    /// The opened decoder's name, for the timing record check.
    pub decoder_name: String,
    origin: i128,
    eof_sent: bool,
    flushed: bool,
}

/// One converted frame: its sample count and PTS position, if any.
pub(super) struct AudioChunk {
    pub pts_position: Option<i64>,
    pub frames: usize,
    /// The resampler's drained tail at the end of the stream.
    pub flushed: bool,
}

impl AudioReader {
    pub(super) fn open(
        artifact: &File,
        timing: &crate::ReviewOutputTiming,
        stream: usize,
        rate: Option<u32>,
        cancellation: &ExportCancellation,
    ) -> Result<Self, MediaError> {
        let input = open_input(artifact, cancellation)?;
        let streams = audio_streams(&input);
        let selected = input
            .stream(stream)
            .filter(|selected| selected.parameters().medium() == ffmpeg::media::Type::Audio)
            .ok_or_else(|| MediaError::Backend("recorded audio stream is missing".into()))?;
        let time_base = selected.time_base();
        if time_base.numerator() <= 0 || time_base.denominator() <= 0 {
            return Err(MediaError::Backend("invalid review audio time base".into()));
        }
        let decoder = ffmpeg::codec::context::Context::from_parameters(selected.parameters())
            .and_then(|context| context.decoder().audio())
            .map_err(backend)?;
        let rate = rate.unwrap_or_else(|| decoder.rate());
        if rate < 1_000 {
            return Err(MediaError::Backend("review audio rate is unusable".into()));
        }
        let channels = decoder.channels();
        if !(1..=2).contains(&channels) {
            return Err(MediaError::Backend(format!(
                "review audio has {channels} channels; the scan judges mono or stereo"
            )));
        }
        let (video_num, video_den) = timing.video.time_base;
        let origin = i128::from(timing.video_first_pts) * i128::from(video_num) * i128::from(rate)
            / i128::from(video_den.max(1));
        let decoder_name = decoder
            .codec()
            .map(|codec| codec.name().to_owned())
            .unwrap_or_default();
        Ok(Self {
            input,
            stream,
            time_base,
            decoder_name,
            frame_size: decoder.frame_size(),
            decoder,
            resampler: None,
            rate,
            channels,
            streams,
            origin,
            eof_sent: false,
            flushed: false,
        })
    }

    pub(super) fn time_base(&self) -> (i32, i32) {
        (self.time_base.numerator(), self.time_base.denominator())
    }

    /// Replace `out` with the next converted frame's interleaved samples.
    pub(super) fn next(
        &mut self,
        out: &mut Vec<f32>,
        cancellation: &ExportCancellation,
    ) -> Result<Option<AudioChunk>, MediaError> {
        loop {
            if cancellation.is_cancelled() {
                return Err(MediaError::Cancelled);
            }
            if self.flushed {
                return Ok(None);
            }
            if let Some(chunk) = self.receive(out)? {
                return Ok(Some(chunk));
            }
            if self.eof_sent {
                // F24: samples the resampler still holds belong to the end
                // of the stream.
                return self.flush(out);
            }
            let mut packet = ffmpeg::Packet::empty();
            match packet.read(&mut self.input) {
                Ok(()) if packet.stream() == self.stream => {
                    self.decoder.send_packet(&packet).map_err(backend)?;
                }
                Ok(()) => {}
                Err(ffmpeg::Error::Eof) => {
                    self.decoder.send_eof().map_err(backend)?;
                    self.eof_sent = true;
                }
                Err(error) => return Err(backend(error)),
            }
        }
    }

    fn interleave(&self, converted: &ffmpeg::frame::Audio, out: &mut Vec<f32>) -> usize {
        let frames = converted.samples();
        let channels = usize::from(self.channels);
        out.clear();
        out.reserve(frames * channels);
        for sample in 0..frames {
            for channel in 0..channels {
                out.push(converted.plane::<f32>(channel)[sample]);
            }
        }
        frames
    }

    fn flush(&mut self, out: &mut Vec<f32>) -> Result<Option<AudioChunk>, MediaError> {
        let Some(resampler) = self.resampler.as_mut() else {
            self.flushed = true;
            return Ok(None);
        };
        let output = *resampler.output();
        let mut converted = ffmpeg::frame::Audio::new(output.format, 4_096, output.channel_layout);
        converted.set_rate(self.rate);
        resampler.flush(&mut converted).map_err(backend)?;
        if converted.samples() == 0 {
            self.flushed = true;
            return Ok(None);
        }
        let frames = self.interleave(&converted, out);
        Ok(Some(AudioChunk {
            pts_position: None,
            frames,
            flushed: true,
        }))
    }

    fn receive(&mut self, out: &mut Vec<f32>) -> Result<Option<AudioChunk>, MediaError> {
        let mut decoded = ffmpeg::frame::Audio::empty();
        match self.decoder.receive_frame(&mut decoded) {
            Ok(()) => {}
            Err(
                ffmpeg::Error::Eof
                | ffmpeg::Error::Other {
                    errno: ffmpeg::error::EAGAIN,
                },
            ) => return Ok(None),
            Err(error) => return Err(backend(error)),
        }
        if decoded.channel_layout().is_empty() {
            decoded.set_channel_layout(ffmpeg::ChannelLayout::default(i32::from(
                decoded.channels(),
            )));
        }
        if decoded.channels() != self.channels {
            return Err(MediaError::Backend(
                "review audio channel count changed".into(),
            ));
        }
        let decoded_rate = decoded.rate().max(1);
        if self.resampler.is_none() {
            self.resampler = Some(
                ffmpeg::software::resampling::Context::get(
                    decoded.format(),
                    decoded.channel_layout(),
                    decoded_rate,
                    ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Planar),
                    decoded.channel_layout(),
                    self.rate,
                )
                .map_err(backend)?,
            );
        }
        let resampler = self.resampler.as_mut().expect("resampler set above");
        let output = *resampler.output();
        let capacity = u64::try_from(decoded.samples())
            .unwrap_or(u64::MAX)
            .saturating_mul(u64::from(self.rate))
            .div_ceil(u64::from(decoded_rate))
            .saturating_add(64);
        let mut converted = ffmpeg::frame::Audio::new(
            output.format,
            usize::try_from(capacity).unwrap_or(usize::MAX),
            output.channel_layout,
        );
        converted.set_rate(self.rate);
        resampler.run(&decoded, &mut converted).map_err(backend)?;
        let frames = self.interleave(&converted, out);
        let pts_position = decoded.pts().or_else(|| decoded.timestamp()).map(|pts| {
            let at =
                i128::from(pts) * i128::from(self.time_base.numerator()) * i128::from(self.rate)
                    / i128::from(self.time_base.denominator());
            i64::try_from(at - self.origin).unwrap_or(i64::MAX)
        });
        Ok(Some(AudioChunk {
            pts_position,
            frames,
            flushed: false,
        }))
    }
}

/// Places consecutive chunks on the output sample clock.
#[derive(Debug)]
pub(super) struct Placement {
    pub cursor: Option<i64>,
    pub tolerance: u64,
}

pub(super) struct Placed {
    pub start: i64,
    /// Silence the PTS jump leaves before this chunk.
    pub gap: u64,
    /// Samples this chunk's PTS overlaps the previous chunk by.
    pub overlap: u64,
    /// A decoded frame without a PTS after the first.
    pub missing_pts: bool,
}

impl Placement {
    pub(super) fn place(&mut self, chunk: &AudioChunk) -> Placed {
        let frames = i64::try_from(chunk.frames).unwrap_or(i64::MAX);
        let placed = match (self.cursor, chunk.pts_position) {
            (None, at) => Placed {
                start: at.unwrap_or(0),
                gap: 0,
                overlap: 0,
                missing_pts: at.is_none(),
            },
            (Some(cursor), None) => Placed {
                start: cursor,
                gap: 0,
                overlap: 0,
                missing_pts: !chunk.flushed,
            },
            (Some(cursor), Some(at)) => {
                let delta = at.saturating_sub(cursor);
                let tolerance = i64::try_from(self.tolerance).unwrap_or(i64::MAX);
                if delta > tolerance {
                    Placed {
                        start: at,
                        gap: delta.unsigned_abs(),
                        overlap: 0,
                        missing_pts: false,
                    }
                } else if delta < -tolerance {
                    Placed {
                        start: cursor,
                        gap: 0,
                        overlap: delta.unsigned_abs(),
                        missing_pts: false,
                    }
                } else {
                    Placed {
                        start: cursor,
                        gap: 0,
                        overlap: 0,
                        missing_pts: false,
                    }
                }
            }
        };
        self.cursor = Some(placed.start.saturating_add(frames));
        placed
    }
}
