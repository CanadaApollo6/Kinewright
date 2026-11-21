//! Bounded sequential decode of a verified review artifact into a compact
//! per-frame and per-hop summary. No decoded picture or PCM is retained past
//! the frame or hop that produced it.
use std::fs::File;
use std::io::Seek as _;

use ffmpeg_next as ffmpeg;
use kinewright_core::{ColorDescription, ExportCancellation, MediaError};

use super::{MAX_SCAN_FRAMES, MAX_SCAN_HOPS, ReviewScanInputs, SCAN_GRID, TechnicalScanConfig};
use crate::decode::backend;

/// Irregular PTS, gaps and overlaps listed before only counting.
pub(crate) const MAX_TIMING_EVENTS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrameStat {
    /// Mean luma on the 8-bit scale.
    pub mean: u8,
    /// Brightest grid cell mean.
    pub max_cell: u8,
    /// Mean absolute luma difference from the previous frame, in 1/256
    /// codes; `u32::MAX` for the first frame.
    pub difference: u32,
}

#[derive(Debug)]
pub(crate) struct VideoSummary {
    pub width: u32,
    pub height: u32,
    pub color: Option<ColorDescription>,
    pub time_base: (i32, i32),
    pub first_pts: Option<i64>,
    /// Frames whose PTS is off `first + index × step`.
    pub off_grid: Vec<u64>,
    pub off_grid_count: u64,
    pub frames: Vec<FrameStat>,
    pub too_long: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HopStat {
    /// RMS of the channel mean in centidBFS, `i16::MIN` for digital zero.
    pub rms_cdb: i16,
    pub click: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TimingEvent {
    Gap { at: i64, samples: u64 },
    Overlap { at: i64, samples: u64 },
}

#[derive(Debug)]
pub(crate) struct AudioSummary {
    pub rate: u32,
    /// Output sample position of the first decoded sample.
    pub first: Option<i64>,
    /// Output sample position after the last decoded sample.
    pub end: i64,
    pub decoded_samples: u64,
    pub events: Vec<TimingEvent>,
    pub event_count: u64,
    /// One hop per `rate / 100` samples from output sample zero.
    pub hops: Vec<HopStat>,
    pub too_long: bool,
    pub loudness: Result<kinewright_core::AudioLoudness, String>,
}

#[derive(Debug)]
pub(crate) struct ScanSummary {
    pub video: VideoSummary,
    pub audio: Option<AudioSummary>,
}

pub(crate) fn scan(
    artifact: File,
    inputs: &ReviewScanInputs<'_>,
    config: &TechnicalScanConfig,
    cancellation: &ExportCancellation,
) -> Result<ScanSummary, MediaError> {
    let mut audio_file = artifact.try_clone().map_err(io_error)?;
    let video = scan_video(artifact, inputs, cancellation)?;
    let audio = if inputs.timing.audio.is_some() {
        audio_file.rewind().map_err(io_error)?;
        Some(scan_audio(audio_file, inputs, config, cancellation)?)
    } else {
        None
    };
    Ok(ScanSummary { video, audio })
}

#[allow(clippy::needless_pass_by_value, reason = "used as a map_err adapter")]
fn io_error(error: std::io::Error) -> MediaError {
    MediaError::Backend(format!("review artifact read failed: {error}"))
}

pub(crate) fn open_input(
    file: File,
    cancellation: &ExportCancellation,
) -> Result<ffmpeg::format::context::Input, MediaError> {
    crate::quiet_ffmpeg_log();
    let io = ffmpeg::format::context::StreamIo::from_read_seek(file).map_err(backend)?;
    let cancel = cancellation.clone();
    ffmpeg::format::input_from_stream_with_interrupt(io, Some("review.mp4"), None, move || {
        cancel.is_cancelled()
    })
    .map_err(backend)
}

/// Exact PTS step of one output frame, if the stream time base allows one.
pub(crate) fn frame_step(time_base: (i32, i32), fps: kinewright_core::Rational) -> Option<i64> {
    let numerator = i128::from(time_base.1) * i128::from(fps.denominator());
    let denominator = i128::from(time_base.0) * i128::from(fps.numerator());
    (denominator > 0 && numerator % denominator == 0)
        .then(|| i64::try_from(numerator / denominator).ok())
        .flatten()
        .filter(|step| *step > 0)
}

fn scan_video(
    artifact: File,
    inputs: &ReviewScanInputs<'_>,
    cancellation: &ExportCancellation,
) -> Result<VideoSummary, MediaError> {
    let mut input = open_input(artifact, cancellation)?;
    let stream = input
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| MediaError::Backend("review output has no video stream".into()))?;
    let index = stream.index();
    let time_base = (
        stream.time_base().numerator(),
        stream.time_base().denominator(),
    );
    let mut decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .and_then(|context| context.decoder().video())
        .map_err(backend)?;
    let mut state = VideoState {
        summary: VideoSummary {
            width: 0,
            height: 0,
            color: None,
            time_base,
            first_pts: None,
            off_grid: Vec::new(),
            off_grid_count: 0,
            frames: Vec::new(),
            too_long: false,
        },
        step: frame_step(time_base, inputs.expectations.output_fps),
        previous: Vec::new(),
        current: Vec::new(),
    };
    loop {
        if cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        let mut packet = ffmpeg::Packet::empty();
        match packet.read(&mut input) {
            Ok(()) if packet.stream() == index => {
                decoder.send_packet(&packet).map_err(backend)?;
                state.receive(&mut decoder, cancellation)?;
            }
            Ok(()) => {}
            Err(ffmpeg::Error::Eof) => break,
            Err(error) => return Err(backend(error)),
        }
        if state.summary.too_long {
            return Ok(state.summary);
        }
    }
    decoder.send_eof().map_err(backend)?;
    state.receive(&mut decoder, cancellation)?;
    Ok(state.summary)
}

struct VideoState {
    summary: VideoSummary,
    step: Option<i64>,
    previous: Vec<u8>,
    current: Vec<u8>,
}

impl VideoState {
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
                Ok(()) => self.frame(decoder, &frame)?,
                Err(
                    ffmpeg::Error::Eof
                    | ffmpeg::Error::Other {
                        errno: ffmpeg::error::EAGAIN,
                    },
                ) => return Ok(()),
                Err(error) => return Err(backend(error)),
            }
            if self.summary.too_long {
                return Ok(());
            }
        }
    }

    fn frame(
        &mut self,
        decoder: &ffmpeg::decoder::Video,
        frame: &ffmpeg::frame::Video,
    ) -> Result<(), MediaError> {
        let summary = &mut self.summary;
        let index = u64::try_from(summary.frames.len()).unwrap_or(u64::MAX);
        if index >= MAX_SCAN_FRAMES {
            summary.too_long = true;
            return Ok(());
        }
        if summary.color.is_none() {
            summary.width = frame.width();
            summary.height = frame.height();
            summary.color = Some(crate::decode::color_description_from_decoder(
                decoder,
                Some(frame.format()),
            ));
        }
        let pts = frame.pts().or_else(|| frame.timestamp());
        let first = *summary.first_pts.get_or_insert(pts.unwrap_or(0));
        let on_grid = match (pts, self.step) {
            (Some(pts), Some(step)) => {
                i64::try_from(index)
                    .ok()
                    .and_then(|index| index.checked_mul(step))
                    .and_then(|offset| first.checked_add(offset))
                    == Some(pts)
            }
            _ => false,
        };
        if !on_grid {
            summary.off_grid_count += 1;
            if summary.off_grid.len() < MAX_TIMING_EVENTS {
                summary.off_grid.push(index);
            }
        }
        let stat = luma_stat(frame, &mut self.current, &self.previous)?;
        std::mem::swap(&mut self.previous, &mut self.current);
        summary.frames.push(stat);
        Ok(())
    }
}

/// Copy the luma plane to `current` as 8-bit codes and measure it against
/// `previous`.
fn luma_stat(
    frame: &ffmpeg::frame::Video,
    current: &mut Vec<u8>,
    previous: &[u8],
) -> Result<FrameStat, MediaError> {
    use ffmpeg::format::Pixel;
    let wide = match frame.format() {
        Pixel::YUV420P
        | Pixel::YUVJ420P
        | Pixel::YUV422P
        | Pixel::YUVJ422P
        | Pixel::YUV444P
        | Pixel::YUVJ444P
        | Pixel::NV12 => false,
        Pixel::YUV420P10LE | Pixel::YUV422P10LE | Pixel::YUV444P10LE | Pixel::P010LE => true,
        other => {
            return Err(MediaError::Backend(format!(
                "review scan cannot read luma from {other:?}"
            )));
        }
    };
    let width = usize::try_from(frame.width()).unwrap_or(0);
    let height = usize::try_from(frame.height()).unwrap_or(0);
    if width == 0 || height == 0 {
        return Err(MediaError::Backend("review frame has no picture".into()));
    }
    let stride = frame.stride(0);
    let data = frame.data(0);
    current.clear();
    current.reserve(width * height);
    for row in 0..height {
        let line = &data[row * stride..];
        if wide {
            current.extend(line[..width * 2].as_chunks::<2>().0.iter().map(|pair| {
                let code = u16::from_le_bytes(*pair) >> 2;
                u8::try_from(code.min(255)).unwrap_or(u8::MAX)
            }));
        } else {
            current.extend_from_slice(&line[..width]);
        }
    }
    let (columns, rows) = SCAN_GRID;
    let mut cells = [(0_u64, 0_u64); SCAN_GRID.0 * SCAN_GRID.1];
    let mut total = 0_u64;
    for (row, line) in current.chunks_exact(width).enumerate() {
        let cell_row = row * rows / height;
        for (column, &code) in line.iter().enumerate() {
            let cell = &mut cells[cell_row * columns + column * columns / width];
            cell.0 += u64::from(code);
            cell.1 += 1;
            total += u64::from(code);
        }
    }
    let count = u64::try_from(current.len()).unwrap_or(u64::MAX);
    let max_cell = cells
        .iter()
        .filter(|cell| cell.1 > 0)
        .map(|cell| cell.0 / cell.1)
        .max()
        .unwrap_or(0);
    let difference = if previous.len() == current.len() {
        let sum: u64 = previous
            .iter()
            .zip(current.iter())
            .map(|(old, new)| u64::from(old.abs_diff(*new)))
            .sum();
        u32::try_from(sum * 256 / count.max(1)).unwrap_or(u32::MAX)
    } else {
        u32::MAX
    };
    Ok(FrameStat {
        mean: u8::try_from(total / count.max(1)).unwrap_or(u8::MAX),
        max_cell: u8::try_from(max_cell).unwrap_or(u8::MAX),
        difference,
    })
}

/// Pull decoded audio, converted to planar F32 at one rate and channel count,
/// with its PTS mapped to output samples from the first picture.
pub(crate) struct AudioReader {
    input: ffmpeg::format::context::Input,
    stream: usize,
    time_base: ffmpeg::Rational,
    decoder: ffmpeg::decoder::Audio,
    resampler: Option<ffmpeg::software::resampling::Context>,
    pub rate: u32,
    pub channels: u16,
    origin: i128,
    eof_sent: bool,
    finished: bool,
}

/// One converted frame: interleaved samples and its PTS position, if any.
pub(crate) struct AudioChunk {
    pub pts_position: Option<i64>,
    pub frames: usize,
}

impl AudioReader {
    /// `rate`/`channels` of `None` keep the stream's own (channels capped at 2).
    pub(crate) fn open(
        file: File,
        timing: &crate::ReviewOutputTiming,
        rate: Option<u32>,
        channels: Option<u16>,
        cancellation: &ExportCancellation,
    ) -> Result<Self, MediaError> {
        let input = open_input(file, cancellation)?;
        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Audio)
            .ok_or_else(|| MediaError::Backend("review output has no audio stream".into()))?;
        let index = stream.index();
        let time_base = stream.time_base();
        if time_base.numerator() <= 0 || time_base.denominator() <= 0 {
            return Err(MediaError::Backend("invalid review audio time base".into()));
        }
        let decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .and_then(|context| context.decoder().audio())
            .map_err(backend)?;
        let rate = rate.unwrap_or_else(|| decoder.rate());
        if rate < 1_000 {
            return Err(MediaError::Backend("review audio rate is unusable".into()));
        }
        let channels = channels.unwrap_or_else(|| decoder.channels().clamp(1, 2));
        let (video_num, video_den) = timing.video.time_base;
        let origin = i128::from(timing.video_first_pts) * i128::from(video_num) * i128::from(rate)
            / i128::from(video_den.max(1));
        Ok(Self {
            input,
            stream: index,
            time_base,
            decoder,
            resampler: None,
            rate,
            channels,
            origin,
            eof_sent: false,
            finished: false,
        })
    }

    /// Replace `out` with the next converted frame's interleaved samples.
    pub(crate) fn next(
        &mut self,
        out: &mut Vec<f32>,
        cancellation: &ExportCancellation,
    ) -> Result<Option<AudioChunk>, MediaError> {
        loop {
            if cancellation.is_cancelled() {
                return Err(MediaError::Cancelled);
            }
            if self.finished {
                return Ok(None);
            }
            if let Some(chunk) = self.receive(out)? {
                return Ok(Some(chunk));
            }
            if self.eof_sent {
                self.finished = true;
                return Ok(None);
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
        let decoded_rate = decoded.rate().max(1);
        if self.resampler.is_none() {
            self.resampler = Some(
                ffmpeg::software::resampling::Context::get(
                    decoded.format(),
                    decoded.channel_layout(),
                    decoded_rate,
                    ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Planar),
                    ffmpeg::ChannelLayout::default(i32::from(self.channels)),
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
        let frames = converted.samples();
        let channels = usize::from(self.channels);
        out.clear();
        out.reserve(frames * channels);
        for sample in 0..frames {
            for channel in 0..channels {
                out.push(converted.plane::<f32>(channel)[sample]);
            }
        }
        let pts_position = decoded.pts().or_else(|| decoded.timestamp()).map(|pts| {
            let at =
                i128::from(pts) * i128::from(self.time_base.numerator()) * i128::from(self.rate)
                    / i128::from(self.time_base.denominator());
            i64::try_from(at - self.origin).unwrap_or(i64::MAX)
        });
        Ok(Some(AudioChunk {
            pts_position,
            frames,
        }))
    }
}

/// Places consecutive chunks on the output sample clock.
#[derive(Debug)]
pub(crate) struct Placement {
    pub cursor: Option<i64>,
    pub tolerance: u64,
}

pub(crate) struct Placed {
    pub start: i64,
    /// Silence the PTS jump leaves before this chunk.
    pub gap: u64,
    /// Samples this chunk's PTS overlaps the previous chunk by.
    pub overlap: u64,
}

impl Placement {
    pub(crate) fn place(&mut self, chunk: &AudioChunk) -> Placed {
        let frames = i64::try_from(chunk.frames).unwrap_or(i64::MAX);
        let placed = match (self.cursor, chunk.pts_position) {
            (None, at) => Placed {
                start: at.unwrap_or(0),
                gap: 0,
                overlap: 0,
            },
            (Some(cursor), None) => Placed {
                start: cursor,
                gap: 0,
                overlap: 0,
            },
            (Some(cursor), Some(at)) => {
                let delta = at.saturating_sub(cursor);
                let tolerance = i64::try_from(self.tolerance).unwrap_or(i64::MAX);
                if delta > tolerance {
                    Placed {
                        start: at,
                        gap: delta.unsigned_abs(),
                        overlap: 0,
                    }
                } else if delta < -tolerance {
                    Placed {
                        start: cursor,
                        gap: 0,
                        overlap: delta.unsigned_abs(),
                    }
                } else {
                    Placed {
                        start: cursor,
                        gap: 0,
                        overlap: 0,
                    }
                }
            }
        };
        self.cursor = Some(placed.start.saturating_add(frames));
        placed
    }
}

fn scan_audio(
    file: File,
    inputs: &ReviewScanInputs<'_>,
    config: &TechnicalScanConfig,
    cancellation: &ExportCancellation,
) -> Result<AudioSummary, MediaError> {
    let mut reader = AudioReader::open(file, inputs.timing, None, None, cancellation)?;
    let rate = reader.rate;
    let channels = reader.channels;
    if rate % 100 != 0 {
        return Err(MediaError::Backend(format!(
            "review audio at {rate} Hz has no whole 10 ms hop"
        )));
    }
    let mut meter =
        crate::loudness::LoudnessMeter::new(rate, channels).map_err(|error| error.to_string());
    let mut hops = HopBuilder::new(rate / 100, config);
    let mut placement = Placement {
        cursor: None,
        tolerance: config.audio_pts_tolerance_samples,
    };
    let mut summary = AudioSummary {
        rate,
        first: None,
        end: 0,
        decoded_samples: 0,
        events: Vec::new(),
        event_count: 0,
        hops: Vec::new(),
        too_long: false,
        loudness: Err("not measured".into()),
    };
    let mut samples = Vec::new();
    while let Some(chunk) = reader.next(&mut samples, cancellation)? {
        if chunk.frames == 0 {
            continue;
        }
        let placed = placement.place(&chunk);
        summary.first.get_or_insert(placed.start);
        let event = if placed.gap > 0 {
            Some(TimingEvent::Gap {
                at: placed.start,
                samples: placed.gap,
            })
        } else if placed.overlap > 0 {
            Some(TimingEvent::Overlap {
                at: placed.start,
                samples: placed.overlap,
            })
        } else {
            None
        };
        if let Some(event) = event {
            summary.event_count += 1;
            if summary.events.len() < MAX_TIMING_EVENTS {
                summary.events.push(event);
            }
        }
        if let Ok(active) = meter.as_mut()
            && let Err(error) = active.push(&samples)
        {
            meter = Err(error.to_string());
        }
        summary.decoded_samples += u64::try_from(chunk.frames).unwrap_or(u64::MAX);
        hops.push(placed.start, &samples, usize::from(channels));
        summary.end = placed
            .start
            .saturating_add(i64::try_from(chunk.frames).unwrap_or(0));
        if u64::try_from(hops.hops.len()).unwrap_or(u64::MAX) > MAX_SCAN_HOPS {
            summary.too_long = true;
            break;
        }
    }
    summary.hops = hops.finish();
    summary.loudness = meter.and_then(|meter| meter.finish().map_err(|error| error.to_string()));
    Ok(summary)
}

/// Builds 10 ms hops of the channel mean from output sample zero.
struct HopBuilder {
    hop: u64,
    click_min: f32,
    click_crest: f32,
    /// Output sample position the next sample lands on.
    position: i64,
    sum_squares: f64,
    sd_squares: f64,
    sd_peak: f32,
    count: u64,
    /// The two previous samples, newest last, reset at a PTS gap.
    older: Option<f32>,
    old: Option<f32>,
    hops: Vec<HopStat>,
}

impl HopBuilder {
    fn new(hop: u32, config: &TechnicalScanConfig) -> Self {
        Self {
            hop: u64::from(hop),
            click_min: config.click_min_second_difference,
            click_crest: config.click_crest,
            position: 0,
            sum_squares: 0.0,
            sd_squares: 0.0,
            sd_peak: 0.0,
            count: 0,
            older: None,
            old: None,
            hops: Vec::new(),
        }
    }

    fn push(&mut self, start: i64, interleaved: &[f32], channels: usize) {
        let mut skip = 0_usize;
        if start > self.position {
            // A PTS gap is silence on the output clock.
            let silence = start - self.position;
            for _ in 0..silence {
                self.sample(0.0);
            }
            self.older = None;
            self.old = None;
        } else if start < self.position {
            // Samples before the picture or overlapped samples are not
            // placed twice.
            skip = usize::try_from(self.position - start).unwrap_or(usize::MAX);
        }
        let scale = 1.0 / f32::from(u16::try_from(channels).unwrap_or(1).max(1));
        for frame in interleaved.chunks_exact(channels.max(1)).skip(skip) {
            self.sample(frame.iter().sum::<f32>() * scale);
        }
    }

    fn sample(&mut self, value: f32) {
        if self.position >= 0 {
            self.sum_squares += f64::from(value) * f64::from(value);
            if let (Some(older), Some(old)) = (self.older, self.old) {
                let second = value - 2.0 * old + older;
                self.sd_squares += f64::from(second) * f64::from(second);
                self.sd_peak = self.sd_peak.max(second.abs());
            }
            self.count += 1;
            if self.count == self.hop {
                self.close();
            }
        }
        self.older = self.old;
        self.old = Some(value);
        self.position += 1;
    }

    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "hop sample counts and RMS levels are far inside f32 precision"
    )]
    fn close(&mut self) {
        let count = self.count.max(1) as f64;
        let rms = (self.sum_squares / count).sqrt();
        let sd_rms = (self.sd_squares / count).sqrt() as f32;
        self.hops.push(HopStat {
            rms_cdb: centi_db(rms),
            click: self.sd_peak >= self.click_min && self.sd_peak >= self.click_crest * sd_rms,
        });
        self.sum_squares = 0.0;
        self.sd_squares = 0.0;
        self.sd_peak = 0.0;
        self.count = 0;
    }

    fn finish(mut self) -> Vec<HopStat> {
        if self.count > 0 {
            self.close();
        }
        self.hops
    }
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "the level is clamped to the i16 range first"
)]
pub(crate) fn centi_db(rms: f64) -> i16 {
    if rms <= 0.0 {
        i16::MIN
    } else {
        (2_000.0 * rms.log10()).round().clamp(-30_000.0, 0.0) as i16
    }
}
