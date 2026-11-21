//! Inspect the selected streams of an installed owned encode through its file handle.
use std::fs::File;

use ffmpeg_next as ffmpeg;
use kinewright_core::{ExportCancellation, MediaError};
use serde::{Deserialize, Serialize};

use crate::decode::backend;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewOutputStream {
    pub index: usize,
    pub decoder: String,
    pub time_base: (i32, i32),
    pub start_pts: i64,
    pub duration_pts: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewOutputTiming {
    pub decoder_version: u32,
    pub video: ReviewOutputStream,
    pub audio: Option<ReviewOutputStream>,
    pub video_first_pts: i64,
    /// The owned delivery encode must present on one constant frame grid.
    pub video_pts_step: Option<i64>,
    pub video_frames: u64,
}

/// Open the decoders and record the presented video PTS grid and selected streams.
///
/// # Errors
/// Refuses unreadable streams, decode failures, cancellation and irregular video timing.
pub fn inspect_review_output(
    file: File,
    cancellation: &ExportCancellation,
) -> Result<ReviewOutputTiming, MediaError> {
    crate::quiet_ffmpeg_log();
    let io = ffmpeg::format::context::StreamIo::from_read_seek(file).map_err(backend)?;
    let cancel = cancellation.clone();
    let mut input =
        ffmpeg::format::input_from_stream_with_interrupt(io, Some("review.mp4"), None, move || {
            cancel.is_cancelled()
        })
        .map_err(backend)?;

    let stream = input
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| MediaError::Backend("review output has no video stream".into()))?;
    let mut decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .and_then(|context| context.decoder().video())
        .map_err(backend)?;
    let video = stream_identity(&stream, decoder.codec())?;
    let audio = input
        .streams()
        .best(ffmpeg::media::Type::Audio)
        .map(|stream| {
            let decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
                .and_then(|context| context.decoder().audio())
                .map_err(backend)?;
            stream_identity(&stream, decoder.codec())
        })
        .transpose()?;

    let mut timing = VideoTiming::default();
    loop {
        if cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        let mut packet = ffmpeg::Packet::empty();
        match packet.read(&mut input) {
            Ok(()) if packet.stream() == video.index => {
                decoder.send_packet(&packet).map_err(backend)?;
                timing.receive(&mut decoder, cancellation)?;
            }
            Ok(()) => {}
            Err(ffmpeg::Error::Eof) => break,
            Err(error) => return Err(backend(error)),
        }
    }
    decoder.send_eof().map_err(backend)?;
    timing.receive(&mut decoder, cancellation)?;
    Ok(ReviewOutputTiming {
        decoder_version: ffmpeg::codec::version(),
        video,
        audio,
        video_first_pts: timing.first.ok_or_else(invalid_timing)?,
        video_pts_step: timing.step,
        video_frames: timing.frames,
    })
}

fn stream_identity(
    stream: &ffmpeg::Stream<'_>,
    codec: Option<ffmpeg::Codec>,
) -> Result<ReviewOutputStream, MediaError> {
    let codec = codec.ok_or_else(|| MediaError::Backend("review decoder unavailable".into()))?;
    let time_base = stream.time_base();
    if time_base.numerator() <= 0 || time_base.denominator() <= 0 {
        return Err(invalid_timing());
    }
    Ok(ReviewOutputStream {
        index: stream.index(),
        decoder: codec.name().to_owned(),
        time_base: (time_base.numerator(), time_base.denominator()),
        start_pts: stream.start_time(),
        duration_pts: stream.duration(),
    })
}

#[derive(Default)]
struct VideoTiming {
    first: Option<i64>,
    last: Option<i64>,
    step: Option<i64>,
    frames: u64,
}

impl VideoTiming {
    fn receive(
        &mut self,
        decoder: &mut ffmpeg::decoder::Video,
        cancellation: &ExportCancellation,
    ) -> Result<(), MediaError> {
        let mut frame = ffmpeg::frame::Video::empty();
        loop {
            if cancellation.is_cancelled() {
                return Err(MediaError::Cancelled);
            }
            match decoder.receive_frame(&mut frame) {
                Ok(()) => {
                    let pts = frame
                        .pts()
                        .or_else(|| frame.timestamp())
                        .ok_or_else(invalid_timing)?;
                    if let Some(last) = self.last {
                        let step = pts
                            .checked_sub(last)
                            .filter(|step| *step > 0)
                            .ok_or_else(invalid_timing)?;
                        if *self.step.get_or_insert(step) != step {
                            return Err(invalid_timing());
                        }
                    }
                    self.first.get_or_insert(pts);
                    self.last = Some(pts);
                    self.frames += 1;
                }
                Err(
                    ffmpeg::Error::Eof
                    | ffmpeg::Error::Other {
                        errno: ffmpeg::error::EAGAIN,
                    },
                ) => {
                    return Ok(());
                }
                Err(error) => return Err(backend(error)),
            }
        }
    }
}

fn invalid_timing() -> MediaError {
    MediaError::Backend("invalid review output presentation timing".into())
}
