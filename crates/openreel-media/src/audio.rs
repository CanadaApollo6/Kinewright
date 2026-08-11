use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ffmpeg_next as ffmpeg;
use openreel_core::{Document, ExportCancellation, MediaError, Rational, TimeCode};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::{
    clock::frame_to_samples,
    decode::{backend, ensure_decoder, media_error, media_input, stream_timestamp_to_global},
    timeline::timeline_audio_segments,
};

const AV_TIME_BASE: i64 = 1_000_000;
const BUFFER_SECONDS: usize = 2;
const MIX_CHUNK_SAMPLE_FRAMES: usize = 1_024;

pub(crate) fn decode_audio_range(
    path: &Path,
    source_fps: Rational,
    source_from: TimeCode,
    source_end: TimeCode,
    output_rate: u32,
    output_channels: u16,
    cancellation: &ExportCancellation,
) -> Result<Vec<f32>, MediaError> {
    if cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    let start_sample = frame_to_samples(source_from, output_rate, source_fps);
    let end_sample = frame_to_samples(source_end, output_rate, source_fps);
    let expected = usize::try_from(end_sample.saturating_sub(start_sample))
        .unwrap_or(usize::MAX)
        .saturating_mul(usize::from(output_channels));
    let mut decoder =
        AudioDecoder::open(path, output_rate, output_channels, start_sample, end_sample)?;
    let mut samples = Vec::with_capacity(expected);
    while let Some(chunk) = decoder.next_chunk()? {
        if cancellation.is_cancelled() {
            return Err(MediaError::Cancelled);
        }
        samples.extend_from_slice(&chunk);
    }
    samples.resize(expected, 0.0);
    samples.truncate(expected);
    Ok(samples)
}

/// Decode a mono peak envelope without retaining the source samples. The
/// decoder is opened once and each sample is reduced directly into a bounded
/// min/max bucket.
pub(crate) fn decode_audio_peaks(
    path: &Path,
    source_fps: Rational,
    source_end: TimeCode,
    output_rate: u32,
    maximum_peaks: usize,
) -> Result<Vec<(i16, i16)>, MediaError> {
    let end_sample = frame_to_samples(source_end, output_rate, source_fps);
    let bucket_count = usize::try_from(end_sample)
        .unwrap_or(usize::MAX)
        .min(maximum_peaks)
        .max(1);
    let mut accumulator = PeakAccumulator::new(end_sample, bucket_count);
    let mut decoder = AudioDecoder::open(path, output_rate, 1, 0, end_sample)?;
    while let Some(chunk) = decoder.next_chunk()? {
        accumulator.extend(&chunk);
    }
    Ok(accumulator.finish())
}

struct PeakAccumulator {
    total_samples: u64,
    next_sample: u64,
    minimums: Vec<f32>,
    maximums: Vec<f32>,
}

impl PeakAccumulator {
    fn new(total_samples: u64, bucket_count: usize) -> Self {
        Self {
            total_samples: total_samples.max(1),
            next_sample: 0,
            minimums: vec![1.0; bucket_count.max(1)],
            maximums: vec![-1.0; bucket_count.max(1)],
        }
    }

    fn extend(&mut self, samples: &[f32]) {
        let bucket_count = self.minimums.len() as u128;
        let total = u128::from(self.total_samples);
        for sample in samples {
            if self.next_sample >= self.total_samples {
                break;
            }
            let bucket =
                usize::try_from(u128::from(self.next_sample).saturating_mul(bucket_count) / total)
                    .unwrap_or(self.minimums.len().saturating_sub(1))
                    .min(self.minimums.len().saturating_sub(1));
            let sample = sample.clamp(-1.0, 1.0);
            self.minimums[bucket] = self.minimums[bucket].min(sample);
            self.maximums[bucket] = self.maximums[bucket].max(sample);
            self.next_sample = self.next_sample.saturating_add(1);
        }
    }

    fn finish(self) -> Vec<(i16, i16)> {
        self.minimums
            .into_iter()
            .zip(self.maximums)
            .map(|(minimum, maximum)| {
                if minimum > maximum {
                    (0, 0)
                } else {
                    (quantize_peak(minimum), quantize_peak(maximum))
                }
            })
            .collect()
    }
}

// The clamped, rounded sample is intentionally quantized to the i16 waveform format.
#[allow(clippy::cast_possible_truncation)]
fn quantize_peak(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16
}

pub(crate) fn limit_audio_mix(samples: &mut [f32]) {
    for sample in samples {
        *sample = sample.clamp(-1.0, 1.0);
    }
}

struct AudioMixSource {
    path: PathBuf,
    output_rate: u32,
    output_channels: u16,
    source_sample_start: u64,
    source_sample_end: u64,
    project_sample_start: u64,
    project_sample_end: u64,
    decoder: Option<AudioDecoder>,
    opened: bool,
    pending: Vec<f32>,
    pending_index: usize,
    finished: bool,
}

impl AudioMixSource {
    fn open_decoder(&mut self) -> Result<(), MediaError> {
        if self.opened {
            return Ok(());
        }
        self.opened = true;
        if self.source_sample_start >= self.source_sample_end {
            self.finished = true;
            return Ok(());
        }
        self.decoder = Some(AudioDecoder::open(
            &self.path,
            self.output_rate,
            self.output_channels,
            self.source_sample_start,
            self.source_sample_end,
        )?);
        Ok(())
    }

    fn add_samples(&mut self, destination: &mut [f32]) -> Result<(), MediaError> {
        if self.finished {
            return Ok(());
        }
        self.open_decoder()?;
        let mut destination_index = 0;
        while destination_index < destination.len() {
            while self.pending_index < self.pending.len() && destination_index < destination.len() {
                destination[destination_index] += self.pending[self.pending_index];
                destination_index += 1;
                self.pending_index += 1;
            }
            if destination_index == destination.len() {
                break;
            }
            self.pending.clear();
            self.pending_index = 0;
            let Some(decoder) = &mut self.decoder else {
                self.finished = true;
                break;
            };
            if let Some(chunk) = decoder.next_chunk()? {
                self.pending = chunk;
            } else {
                self.finished = true;
                break;
            }
        }
        Ok(())
    }

    fn retire(&mut self) {
        self.decoder = None;
        self.pending.clear();
        self.pending_index = 0;
        self.finished = true;
    }
}

struct AudioMixer {
    sources: Vec<AudioMixSource>,
    output_channels: usize,
    cursor_sample: u64,
    end_sample: u64,
}

impl AudioMixer {
    fn open(
        document: &Document,
        project_from: TimeCode,
        output_rate: u32,
        output_channels: u16,
    ) -> Result<Self, MediaError> {
        let project_end = document.duration;
        let segments = timeline_audio_segments(document, project_from..project_end)?;
        let mut sources = Vec::with_capacity(segments.len());
        for segment in segments {
            let clip = document.clip(segment.clip).ok_or_else(|| {
                MediaError::Backend(format!("timeline clip {} disappeared", segment.clip))
            })?;
            let asset = document.asset(segment.asset).ok_or_else(|| {
                MediaError::Backend(format!("timeline asset {} disappeared", segment.asset))
            })?;
            let clip_project_start =
                frame_to_samples(clip.timeline_start, output_rate, document.fps);
            let project_sample_start =
                frame_to_samples(segment.project.start, output_rate, document.fps);
            let project_sample_end =
                frame_to_samples(segment.project.end, output_rate, document.fps);
            let source_clip_start =
                frame_to_samples(clip.source_range.start, output_rate, asset.fps);
            let source_sample_end = frame_to_samples(clip.source_range.end, output_rate, asset.fps);
            let source_sample_start = source_clip_start
                .saturating_add(project_sample_start.saturating_sub(clip_project_start));
            sources.push(AudioMixSource {
                path: asset.path.clone(),
                output_rate,
                output_channels,
                source_sample_start,
                source_sample_end,
                project_sample_start,
                project_sample_end,
                decoder: None,
                opened: false,
                pending: Vec::new(),
                pending_index: 0,
                finished: false,
            });
        }
        Ok(Self {
            sources,
            output_channels: usize::from(output_channels),
            cursor_sample: frame_to_samples(project_from, output_rate, document.fps),
            end_sample: frame_to_samples(project_end, output_rate, document.fps),
        })
    }

    fn next_chunk(&mut self) -> Result<Option<Vec<f32>>, MediaError> {
        if self.cursor_sample >= self.end_sample {
            return Ok(None);
        }
        let remaining = self.end_sample.saturating_sub(self.cursor_sample);
        let sample_frames = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(MIX_CHUNK_SAMPLE_FRAMES);
        let chunk_end = self
            .cursor_sample
            .saturating_add(u64::try_from(sample_frames).unwrap_or(u64::MAX));
        let mut mixed = vec![
            0.0;
            sample_frames
                .checked_mul(self.output_channels)
                .ok_or_else(|| MediaError::Backend(
                    "audio mix chunk is too large".to_owned()
                ))?
        ];
        for source in &mut self.sources {
            let overlap_start = self.cursor_sample.max(source.project_sample_start);
            let overlap_end = chunk_end.min(source.project_sample_end);
            if overlap_end <= overlap_start {
                continue;
            }
            let start = usize::try_from(overlap_start.saturating_sub(self.cursor_sample))
                .unwrap_or(usize::MAX)
                .saturating_mul(self.output_channels);
            let end = usize::try_from(overlap_end.saturating_sub(self.cursor_sample))
                .unwrap_or(usize::MAX)
                .saturating_mul(self.output_channels)
                .min(mixed.len());
            source.add_samples(&mut mixed[start..end])?;
            if overlap_end >= source.project_sample_end {
                source.retire();
            }
        }
        limit_audio_mix(&mut mixed);
        self.cursor_sample = chunk_end;
        Ok(Some(mixed))
    }

    #[cfg(test)]
    fn render_remaining(&mut self) -> Result<Vec<f32>, MediaError> {
        let sample_frames = self.end_sample.saturating_sub(self.cursor_sample);
        let capacity = usize::try_from(sample_frames)
            .unwrap_or(usize::MAX)
            .saturating_mul(self.output_channels);
        let mut rendered = Vec::with_capacity(capacity);
        while let Some(chunk) = self.next_chunk()? {
            rendered.extend_from_slice(&chunk);
        }
        Ok(rendered)
    }
}

pub(crate) struct AudioRuntime {
    stream: cpal::Stream,
    producer: Producer<f32>,
    mixer: AudioMixer,
    pending: Vec<f32>,
    pending_index: usize,
    pub(crate) error_flag: Arc<AtomicBool>,
}

impl AudioRuntime {
    pub(crate) fn open(
        document: &Document,
        project_from: TimeCode,
        position_samples: &Arc<AtomicU64>,
        sample_rate_atomic: &Arc<AtomicU32>,
    ) -> Result<Self, MediaError> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| MediaError::Backend("no default audio output device".to_owned()))?;
        let supported = device.default_output_config().map_err(backend)?;
        let sample_format = supported.sample_format();
        let config = supported.config();
        let sample_rate = config.sample_rate;
        let channels = config.channels;
        let capacity = usize::try_from(sample_rate)
            .unwrap_or(48_000)
            .saturating_mul(usize::from(channels))
            .saturating_mul(BUFFER_SECONDS);
        let (producer, consumer) = RingBuffer::new(capacity.max(1));
        let start_sample = frame_to_samples(project_from, sample_rate, document.fps);
        position_samples.store(start_sample, Ordering::Release);
        sample_rate_atomic.store(sample_rate, Ordering::Release);
        let error_flag = Arc::new(AtomicBool::new(false));
        let stream = build_stream(
            &device,
            &config,
            sample_format,
            consumer,
            channels,
            Arc::clone(position_samples),
            Arc::clone(&error_flag),
        )?;
        let mixer = AudioMixer::open(document, project_from, sample_rate, channels)?;
        let mut runtime = Self {
            stream,
            producer,
            mixer,
            pending: Vec::new(),
            pending_index: 0,
            error_flag,
        };
        runtime.fill()?;
        Ok(runtime)
    }

    pub(crate) fn play(&self) -> Result<(), MediaError> {
        self.stream.play().map_err(backend)
    }

    pub(crate) fn pause(&self) -> Result<(), MediaError> {
        self.stream.pause().map_err(backend)
    }

    pub(crate) fn fill(&mut self) -> Result<(), MediaError> {
        loop {
            while self.pending_index < self.pending.len() {
                let sample = self.pending[self.pending_index];
                match self.producer.push(sample) {
                    Ok(()) => self.pending_index += 1,
                    Err(rtrb::PushError::Full(_)) => return Ok(()),
                }
            }
            self.pending.clear();
            self.pending_index = 0;
            if let Some(chunk) = self.mixer.next_chunk()? {
                self.pending = chunk;
            } else {
                return Ok(());
            }
        }
    }
}

fn build_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    format: cpal::SampleFormat,
    consumer: Consumer<f32>,
    channels: u16,
    position: Arc<AtomicU64>,
    error_flag: Arc<AtomicBool>,
) -> Result<cpal::Stream, MediaError> {
    match format {
        cpal::SampleFormat::F32 => {
            build_typed_stream::<f32>(device, config, consumer, channels, position, error_flag)
        }
        cpal::SampleFormat::I16 => {
            build_typed_stream::<i16>(device, config, consumer, channels, position, error_flag)
        }
        cpal::SampleFormat::U16 => {
            build_typed_stream::<u16>(device, config, consumer, channels, position, error_flag)
        }
        unsupported => Err(MediaError::Backend(format!(
            "unsupported audio device sample format {unsupported}"
        ))),
    }
}

fn build_typed_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut consumer: Consumer<f32>,
    channels: u16,
    position: Arc<AtomicU64>,
    error_flag: Arc<AtomicBool>,
) -> Result<cpal::Stream, MediaError>
where
    T: cpal::SizedSample + cpal::Sample + cpal::FromSample<f32>,
{
    let callback_channels = usize::from(channels).max(1);
    device
        .build_output_stream(
            *config,
            move |output: &mut [T], _| {
                render_output(&mut consumer, output, callback_channels, &position);
            },
            move |_error| {
                error_flag.store(true, Ordering::Release);
            },
            None,
        )
        .map_err(backend)
}

fn render_output<T>(
    consumer: &mut Consumer<f32>,
    output: &mut [T],
    channels: usize,
    position: &AtomicU64,
) where
    T: cpal::Sample + cpal::FromSample<f32>,
{
    let sample_frames = output.len() / channels.max(1);
    for destination in output {
        let sample = consumer.pop().unwrap_or(0.0);
        *destination = T::from_sample(sample);
    }
    position.fetch_add(
        u64::try_from(sample_frames).unwrap_or(u64::MAX),
        Ordering::Release,
    );
}

struct AudioDecoder {
    path: PathBuf,
    input: ffmpeg::format::context::Input,
    decoder: ffmpeg::decoder::Audio,
    resampler: Option<ffmpeg::software::resampling::Context>,
    stream_index: usize,
    stream_time_base: ffmpeg::Rational,
    stream_start: i64,
    output_rate: u32,
    output_channels: usize,
    target_sample: u64,
    end_sample: u64,
    started: bool,
    finished: bool,
    eof_sent: bool,
}

impl AudioDecoder {
    fn open(
        path: &Path,
        output_rate: u32,
        output_channels: u16,
        target_sample: u64,
        end_sample: u64,
    ) -> Result<Self, MediaError> {
        let mut input = media_input(path)?;
        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Audio)
            .ok_or_else(|| {
                MediaError::Backend(format!("media {} has no audio stream", path.display()))
            })?;
        let stream_index = stream.index();
        let stream_time_base = stream.time_base();
        let stream_start = normalized_start(stream.start_time());
        ensure_decoder(&stream, "audio", path)?;
        let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .map_err(|error| media_error(path, "could not read audio codec parameters", error))?;
        let decoder = context
            .decoder()
            .audio()
            .map_err(|error| media_error(path, "could not open the audio decoder", error))?;
        let target_us = i64::try_from(
            u128::from(target_sample).saturating_mul(u128::from(AV_TIME_BASE as u64))
                / u128::from(output_rate),
        )
        .unwrap_or(i64::MAX)
        .saturating_add(stream_timestamp_to_global(stream_start, stream_time_base));
        input
            .seek(target_us, ..target_us)
            .map_err(|error| media_error(path, "audio seek failed", error))?;
        Ok(Self {
            path: path.to_path_buf(),
            input,
            decoder,
            resampler: None,
            stream_index,
            stream_time_base,
            stream_start,
            output_rate,
            output_channels: usize::from(output_channels),
            target_sample,
            end_sample,
            started: false,
            finished: false,
            eof_sent: false,
        })
    }

    fn next_chunk(&mut self) -> Result<Option<Vec<f32>>, MediaError> {
        loop {
            if self.finished {
                return Ok(None);
            }
            if let Some(chunk) = self.receive_frame()?
                && !chunk.is_empty()
            {
                return Ok(Some(chunk));
            }
            if self.finished || self.eof_sent {
                return Ok(None);
            }
            let next = self
                .input
                .packets()
                .next()
                .map(|(stream, packet)| (stream.index(), packet));
            if let Some((stream_index, packet)) = next {
                if stream_index != self.stream_index {
                    continue;
                }
                self.decoder
                    .send_packet(&packet)
                    .map_err(|error| media_error(&self.path, "audio decode failed", error))?;
            } else {
                self.decoder.send_eof().map_err(|error| {
                    media_error(&self.path, "audio decoder flush failed", error)
                })?;
                self.eof_sent = true;
            }
        }
    }

    // FFmpeg frame receipt, resampling, and clipping form one ordered state transition.
    #[allow(clippy::too_many_lines)]
    fn receive_frame(&mut self) -> Result<Option<Vec<f32>>, MediaError> {
        let mut decoded = ffmpeg::frame::Audio::empty();
        if self.decoder.receive_frame(&mut decoded).is_err() {
            return Ok(None);
        }
        let pts = decoded.timestamp();
        let decoded_format = decoded.format();
        let mut decoded_layout = decoded.channel_layout();
        if decoded_layout.is_empty() {
            decoded_layout = ffmpeg::ChannelLayout::default(i32::from(decoded.channels()));
            decoded.set_channel_layout(decoded_layout);
        }
        let decoded_rate = decoded.rate();
        if self.resampler.is_none() {
            let output_layout =
                ffmpeg::ChannelLayout::default(i32::try_from(self.output_channels).unwrap_or(1));
            self.resampler = Some(
                ffmpeg::software::resampling::Context::get(
                    decoded_format,
                    decoded_layout,
                    decoded_rate,
                    ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Planar),
                    output_layout,
                    self.output_rate,
                )
                .map_err(|error| {
                    media_error(&self.path, "could not create audio resampler", error)
                })?,
            );
        }
        let output = *self
            .resampler
            .as_ref()
            .expect("resampler is initialized from the first decoded frame")
            .output();
        let output_samples = u64::try_from(decoded.samples())
            .unwrap_or(u64::MAX)
            .saturating_mul(u64::from(output.rate))
            .saturating_add(u64::from(decoded_rate).saturating_sub(1))
            / u64::from(decoded_rate.max(1));
        let output_samples =
            usize::try_from(output_samples.saturating_add(64)).unwrap_or(usize::MAX);
        let mut converted =
            ffmpeg::frame::Audio::new(output.format, output_samples, output.channel_layout);
        converted.set_rate(output.rate);
        self.resampler
            .as_mut()
            .expect("resampler is initialized from the first decoded frame")
            .run(&decoded, &mut converted)
            .map_err(|error| {
                MediaError::Backend(format!(
                    "audio resampling failed for {} ({decoded_format:?}, {decoded_layout:?}, {decoded_rate} Hz): {error}",
                    self.path.display()
                ))
            })?;
        let samples = converted.samples();
        if samples == 0 {
            return Ok(Some(Vec::new()));
        }
        let chunk_start = pts.map_or(self.target_sample, |timestamp| {
            timestamp_to_samples(
                timestamp.saturating_sub(self.stream_start),
                self.stream_time_base,
                self.output_rate,
            )
        });
        let chunk_end = chunk_start.saturating_add(u64::try_from(samples).unwrap_or(u64::MAX));
        if chunk_start >= self.end_sample {
            self.finished = true;
            return Ok(None);
        }
        if !self.started && chunk_end <= self.target_sample {
            return Ok(Some(Vec::new()));
        }
        let wanted_start = chunk_start.max(self.target_sample);
        let wanted_end = chunk_end.min(self.end_sample);
        if wanted_end <= wanted_start {
            self.finished = chunk_end >= self.end_sample;
            return Ok(Some(Vec::new()));
        }
        let skip = usize::try_from(wanted_start.saturating_sub(chunk_start))
            .unwrap_or(samples)
            .min(samples);
        let gap = if self.started {
            0
        } else {
            chunk_start.saturating_sub(self.target_sample)
        };
        let gap = usize::try_from(gap)
            .unwrap_or_default()
            .min(usize::try_from(self.output_rate).unwrap_or_default());
        let remaining = usize::try_from(wanted_end.saturating_sub(wanted_start))
            .unwrap_or(samples.saturating_sub(skip))
            .min(samples.saturating_sub(skip));
        let mut interleaved = Vec::with_capacity(
            gap.saturating_add(remaining)
                .saturating_mul(self.output_channels),
        );
        interleaved.resize(gap.saturating_mul(self.output_channels), 0.0);
        for sample_index in skip..skip.saturating_add(remaining) {
            for channel in 0..self.output_channels {
                interleaved.push(converted.plane::<f32>(channel)[sample_index]);
            }
        }
        self.started = true;
        if wanted_end >= self.end_sample {
            self.finished = true;
        }
        Ok(Some(interleaved))
    }
}

fn timestamp_to_samples(timestamp: i64, time_base: ffmpeg::Rational, output_rate: u32) -> u64 {
    if timestamp <= 0 {
        return 0;
    }
    let numerator = i128::from(timestamp)
        .saturating_mul(i128::from(time_base.numerator()))
        .saturating_mul(i128::from(output_rate));
    let denominator = i128::from(time_base.denominator());
    u64::try_from(numerator / denominator).unwrap_or(u64::MAX)
}

fn normalized_start(start: i64) -> i64 {
    if start < -1_000_000_000_000 { 0 } else { start }
}

#[cfg(test)]
mod tests {
    use openreel_core::{
        AssetId, Clip, ClipId, ExportSettings, MediaAsset, MediaKind, Track, TrackId, TrackKind,
    };

    use crate::test_support::GeneratedMedia;

    use super::*;

    #[test]
    // These values are exact binary fractions and zeros, so exact equality is the contract.
    #[allow(clippy::float_cmp)]
    fn callback_consumes_ring_then_writes_silence_and_accounts_frames() {
        let (mut producer, mut consumer) = RingBuffer::new(4);
        producer.push(0.25).unwrap();
        producer.push(-0.5).unwrap();
        let position = AtomicU64::new(10);
        let mut output = [1.0_f32; 4];

        render_output(&mut consumer, &mut output, 2, &position);

        assert_eq!(output, [0.25, -0.5, 0.0, 0.0]);
        assert_eq!(position.load(Ordering::Acquire), 12);
    }

    #[test]
    fn ring_buffer_is_bounded_and_non_overwriting() {
        let (mut producer, mut consumer) = RingBuffer::new(2);
        assert!(producer.push(1.0_f32).is_ok());
        assert!(producer.push(2.0_f32).is_ok());
        assert!(matches!(
            producer.push(3.0_f32),
            Err(rtrb::PushError::Full(3.0))
        ));
        assert_eq!(consumer.pop(), Ok(1.0));
        assert_eq!(consumer.pop(), Ok(2.0));
        assert!(matches!(consumer.pop(), Err(rtrb::PopError::Empty)));
    }

    #[test]
    fn peak_accumulator_reduces_samples_into_bounded_min_max_pairs() {
        let mut accumulator = PeakAccumulator::new(8, 2);
        accumulator.extend(&[-1.0, -0.5, 0.25]);
        accumulator.extend(&[0.5, -0.25, 0.0, 0.75, 1.0]);

        assert_eq!(
            accumulator.finish(),
            vec![(i16::MIN + 1, 16_384), (-8_192, i16::MAX)]
        );
    }

    #[test]
    fn peak_accumulator_zero_fills_missing_buckets() {
        let mut accumulator = PeakAccumulator::new(8, 4);
        accumulator.extend(&[0.5, -0.5]);

        assert_eq!(accumulator.finish()[1..], [(0, 0), (0, 0), (0, 0)]);
    }

    #[test]
    fn playback_feeder_mix_matches_export_across_overlap_trim_gap_and_clamp() {
        crate::initialize_ffmpeg().unwrap();
        let voice = loud_sine("m12-voice", 440);
        let bed = loud_sine("m12-bed", 660);
        let fps = Rational::new(10, 1).unwrap();
        let document = parity_document(voice.path(), bed.path(), fps);
        document.validate().unwrap();
        let settings = ExportSettings {
            fps,
            resolution: (64, 64),
            video_codec: "libx264".to_owned(),
            audio_codec: "aac".to_owned(),
            video_bitrate: 1_000_000,
            audio_bitrate: 128_000,
            cancellation: ExportCancellation::default(),
        };

        let exported = crate::export::mix_audio(&document, &settings).unwrap();
        let mut playback = AudioMixer::open(&document, TimeCode::ZERO, 48_000, 2).unwrap();
        let played = playback.render_remaining().unwrap();

        assert_eq!(played.len(), exported.len());
        for (name, frames) in [
            ("single source", 0..4),
            ("overlap", 4..6),
            ("trimmed source", 6..14),
            ("silence", 14..16),
            ("post-gap source", 16..20),
        ] {
            let samples = interleaved_sample_range(frames, fps, 48_000, 2);
            let maximum_difference = exported[samples.clone()]
                .iter()
                .zip(&played[samples])
                .map(|(exported, played)| (exported - played).abs())
                .fold(0.0_f32, f32::max);
            assert!(
                maximum_difference <= 1.0e-6,
                "{name} differs by {maximum_difference}"
            );
        }

        let overlap = interleaved_sample_range(4..6, fps, 48_000, 2);
        assert!(
            exported[overlap]
                .iter()
                .any(|sample| sample.abs() >= 1.0 - f32::EPSILON),
            "fixture did not exercise the hard-clamp limiter"
        );
        let silence = interleaved_sample_range(14..16, fps, 48_000, 2);
        assert!(
            exported[silence]
                .iter()
                .all(|sample| sample.abs() <= 1.0e-7),
            "fixture gap was not silent"
        );

        let seek_sample = usize::try_from(frame_to_samples(TimeCode(5), 48_000, fps))
            .unwrap()
            .saturating_mul(2);
        let mut seeked_playback = AudioMixer::open(&document, TimeCode(5), 48_000, 2).unwrap();
        let seeked = seeked_playback.render_remaining().unwrap();
        let seek_difference = exported[seek_sample..]
            .iter()
            .zip(&seeked)
            .map(|(exported, played)| (exported - played).abs())
            .fold(0.0_f32, f32::max);
        assert_eq!(seeked.len(), exported.len() - seek_sample);
        assert!(
            seek_difference <= 1.0e-6,
            "coherent feeder seek differs by {seek_difference}"
        );
    }

    #[test]
    fn video_only_timeline_feeds_silence_for_the_audio_master_clock() {
        let fps = Rational::new(10, 1).unwrap();
        let document = Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(1),
                    asset: AssetId(1),
                    source_range: TimeCode(0)..TimeCode(2),
                    content: openreel_core::ClipContent::Media,
                    timeline_start: TimeCode::ZERO,
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                }],
            }],
            media_pool: vec![MediaAsset {
                id: AssetId(1),
                path: PathBuf::from("video-only.mp4"),
                name: "video only".to_owned(),
                duration: TimeCode(2),
                fps,
                kind: MediaKind::Video,
                resolution: Some((64, 64)),
            }],
            markers: Vec::new(),
            fps,
            resolution: (64, 64),
            duration: TimeCode(2),
        };

        let mut mixer = AudioMixer::open(&document, TimeCode::ZERO, 48_000, 2).unwrap();
        let rendered = mixer.render_remaining().unwrap();

        assert_eq!(rendered.len(), 19_200);
        assert!(rendered.iter().all(|sample| *sample == 0.0));
    }

    fn loud_sine(label: &str, frequency: u16) -> GeneratedMedia {
        let source = format!("sine=frequency={frequency}:sample_rate=48000:duration=2");
        GeneratedMedia::ffmpeg(
            label,
            &[
                "-f",
                "lavfi",
                "-i",
                &source,
                "-filter:a",
                "volume=6",
                "-c:a",
                "pcm_f32le",
            ],
            "wav",
        )
    }

    fn parity_document(voice: &Path, bed: &Path, fps: Rational) -> Document {
        Document {
            tracks: vec![
                Track {
                    id: TrackId(1),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: vec![audio_clip(1, 1, 0..6, 0), audio_clip(2, 1, 16..20, 16)],
                },
                Track {
                    id: TrackId(2),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: vec![audio_clip(3, 2, 4..14, 4)],
                },
            ],
            media_pool: vec![
                audio_asset(1, voice, "voice-440", fps),
                audio_asset(2, bed, "bed-660", fps),
            ],
            markers: Vec::new(),
            fps,
            resolution: (64, 64),
            duration: TimeCode(20),
        }
    }

    fn audio_clip(id: u64, asset: u64, source: std::ops::Range<i64>, timeline_start: i64) -> Clip {
        Clip {
            id: ClipId(id),
            asset: AssetId(asset),
            source_range: TimeCode(source.start)..TimeCode(source.end),
            content: openreel_core::ClipContent::Media,
            timeline_start: TimeCode(timeline_start),
            effects: Vec::new(),
            transition_in: None,
            link: None,
        }
    }

    fn audio_asset(id: u64, path: &Path, name: &str, fps: Rational) -> MediaAsset {
        MediaAsset {
            id: AssetId(id),
            path: path.to_path_buf(),
            name: name.to_owned(),
            duration: TimeCode(20),
            fps,
            kind: MediaKind::Audio,
            resolution: None,
        }
    }

    fn interleaved_sample_range(
        project_frames: std::ops::Range<i64>,
        fps: Rational,
        sample_rate: u32,
        channels: usize,
    ) -> std::ops::Range<usize> {
        let start = usize::try_from(frame_to_samples(
            TimeCode(project_frames.start),
            sample_rate,
            fps,
        ))
        .unwrap()
        .saturating_mul(channels);
        let end = usize::try_from(frame_to_samples(
            TimeCode(project_frames.end),
            sample_rate,
            fps,
        ))
        .unwrap()
        .saturating_mul(channels);
        start..end
    }
}
