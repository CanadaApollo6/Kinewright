use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ffmpeg_next as ffmpeg;
use openreel_core::{ExportCancellation, MediaError, Rational, TimeCode};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::{
    clock::frame_to_samples,
    decode::{backend, ensure_decoder, media_error, media_input, stream_timestamp_to_global},
};

const AV_TIME_BASE: i64 = 1_000_000;
const BUFFER_SECONDS: usize = 2;

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

fn quantize_peak(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16
}

pub(crate) struct AudioRuntime {
    stream: cpal::Stream,
    producer: Producer<f32>,
    decoder: Option<AudioDecoder>,
    pending: Vec<f32>,
    pending_index: usize,
    pub(crate) error_flag: Arc<AtomicBool>,
    exhausted: bool,
}

impl AudioRuntime {
    pub(crate) fn open(
        path: &Path,
        source_fps: Rational,
        project_fps: Rational,
        source_from: TimeCode,
        source_end: TimeCode,
        project_from: TimeCode,
        position_samples: Arc<AtomicU64>,
        sample_rate_atomic: Arc<AtomicU32>,
    ) -> Result<Self, MediaError> {
        Self::open_output(
            Some((path, source_fps, source_from, source_end)),
            project_fps,
            project_from,
            position_samples,
            sample_rate_atomic,
        )
    }

    pub(crate) fn open_silence(
        project_fps: Rational,
        project_from: TimeCode,
        position_samples: Arc<AtomicU64>,
        sample_rate_atomic: Arc<AtomicU32>,
    ) -> Result<Self, MediaError> {
        Self::open_output(
            None,
            project_fps,
            project_from,
            position_samples,
            sample_rate_atomic,
        )
    }

    fn open_output(
        source: Option<(&Path, Rational, TimeCode, TimeCode)>,
        project_fps: Rational,
        project_from: TimeCode,
        position_samples: Arc<AtomicU64>,
        sample_rate_atomic: Arc<AtomicU32>,
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
        let start_sample = frame_to_samples(project_from, sample_rate, project_fps);
        position_samples.store(start_sample, Ordering::Release);
        sample_rate_atomic.store(sample_rate, Ordering::Release);
        let error_flag = Arc::new(AtomicBool::new(false));
        let stream = build_stream(
            &device,
            &config,
            sample_format,
            consumer,
            channels,
            Arc::clone(&position_samples),
            Arc::clone(&error_flag),
        )?;
        let decoder = source
            .map(|(path, source_fps, source_from, source_end)| {
                let source_start = frame_to_samples(source_from, sample_rate, source_fps);
                let source_end = frame_to_samples(source_end, sample_rate, source_fps);
                AudioDecoder::open(path, sample_rate, channels, source_start, source_end)
            })
            .transpose()?;
        let mut runtime = Self {
            stream,
            producer,
            decoder,
            pending: Vec::new(),
            pending_index: 0,
            error_flag,
            exhausted: false,
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
            let Some(decoder) = &mut self.decoder else {
                self.exhausted = true;
                return Ok(());
            };
            if let Some(chunk) = decoder.next_chunk()? { self.pending = chunk } else {
                self.exhausted = true;
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
    queued: VecDeque<Vec<f32>>,
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
            queued: VecDeque::new(),
        })
    }

    fn next_chunk(&mut self) -> Result<Option<Vec<f32>>, MediaError> {
        loop {
            if let Some(chunk) = self.queued.pop_front()
                && !chunk.is_empty() {
                    return Ok(Some(chunk));
                }
            if self.finished {
                return Ok(None);
            }
            if self.eof_sent {
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
                self.receive_frames()?;
            } else {
                self.decoder.send_eof().map_err(|error| {
                    media_error(&self.path, "audio decoder flush failed", error)
                })?;
                self.eof_sent = true;
                self.receive_frames()?;
            }
        }
    }

    fn receive_frames(&mut self) -> Result<(), MediaError> {
        let mut decoded = ffmpeg::frame::Audio::empty();
        while self.decoder.receive_frame(&mut decoded).is_ok() {
            let pts = decoded.timestamp();
            let decoded_format = decoded.format();
            let mut decoded_layout = decoded.channel_layout();
            if decoded_layout.is_empty() {
                decoded_layout = ffmpeg::ChannelLayout::default(i32::from(decoded.channels()));
                decoded.set_channel_layout(decoded_layout);
            }
            let decoded_rate = decoded.rate();
            if self.resampler.is_none() {
                let output_layout = ffmpeg::ChannelLayout::default(
                    i32::try_from(self.output_channels).unwrap_or(1),
                );
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
                continue;
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
                return Ok(());
            }
            if !self.started && chunk_end <= self.target_sample {
                continue;
            }
            let wanted_start = chunk_start.max(self.target_sample);
            let wanted_end = chunk_end.min(self.end_sample);
            if wanted_end <= wanted_start {
                self.finished = chunk_end >= self.end_sample;
                continue;
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
            self.queued.push_back(interleaved);
            if wanted_end >= self.end_sample {
                self.finished = true;
            }
        }
        Ok(())
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
    use super::*;

    #[test]
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
}
