use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ffmpeg_next as ffmpeg;
use openreel_core::{
    AudioBus, Clip, Document, Effect, ExportCancellation, MediaError, Rational, TimeCode, TrackId,
};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::{
    clock::{frame_to_samples, samples_to_frame},
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

#[derive(Debug)]
pub(crate) struct MeterState {
    peaks: [AtomicU32; 2],
}

impl Default for MeterState {
    fn default() -> Self {
        Self {
            peaks: std::array::from_fn(|_| AtomicU32::new(0.0_f32.to_bits())),
        }
    }
}

impl MeterState {
    pub(crate) fn peaks(&self) -> [f32; 2] {
        self.peaks
            .each_ref()
            .map(|peak| f32::from_bits(peak.load(Ordering::Acquire)))
    }

    pub(crate) fn clear(&self) {
        for peak in &self.peaks {
            peak.store(0.0_f32.to_bits(), Ordering::Release);
        }
    }

    fn record_chunk(&self, samples: &[f32], channel_count: usize) {
        let mut peaks = [0.0_f32; 2];
        let channel_count = channel_count.max(1);
        for (sample_index, sample) in samples.iter().enumerate() {
            let channel = sample_index % channel_count;
            if channel < peaks.len() {
                peaks[channel] = peaks[channel].max(sample.abs());
            }
        }
        for (state, peak) in self.peaks.iter().zip(peaks) {
            state.store(peak.to_bits(), Ordering::Release);
        }
    }
}

fn limit_and_meter_audio_mix(samples: &mut [f32], channel_count: usize, meter: &MeterState) {
    limit_audio_mix(samples);
    meter.record_chunk(samples, channel_count);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AudioGainRamp {
    start_sample: u64,
    end_sample: u64,
}

impl AudioGainRamp {
    // Per-sample gain is the intentional final float conversion after integer boundaries.
    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn gain_at(self, project_sample: u64) -> f32 {
        let sample_span = self.end_sample.saturating_sub(self.start_sample);
        if sample_span <= 1 || project_sample >= self.end_sample.saturating_sub(1) {
            return 1.0;
        }
        let offset = project_sample.saturating_sub(self.start_sample);
        (offset as f32 / (sample_span - 1) as f32).clamp(0.0, 1.0)
    }
}

pub(crate) fn transition_audio_ramp(
    clip: &Clip,
    sample_rate: u32,
    project_fps: Rational,
) -> Option<AudioGainRamp> {
    let transition = clip.transition_in.as_ref()?;
    audio_gain_ramp(
        clip.timeline_start,
        transition.duration,
        sample_rate,
        project_fps,
    )
}

fn audio_gain_ramp(
    start_frame: TimeCode,
    duration: TimeCode,
    sample_rate: u32,
    project_fps: Rational,
) -> Option<AudioGainRamp> {
    if duration.0 <= 1 {
        return None;
    }
    let end_frame = start_frame.checked_add(duration)?;
    let start_sample = frame_to_samples(start_frame, sample_rate, project_fps);
    let end_sample = frame_to_samples(end_frame, sample_rate, project_fps);
    (end_sample > start_sample).then_some(AudioGainRamp {
        start_sample,
        end_sample,
    })
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ClipAudioShaping {
    constant_gain: f32,
    fade_in: Option<AudioGainRamp>,
    fade_out: Option<AudioGainRamp>,
    transition: Option<AudioGainRamp>,
}

impl ClipAudioShaping {
    pub(crate) fn new(
        clip: &Clip,
        clip_duration: TimeCode,
        sample_rate: u32,
        project_fps: Rational,
    ) -> Self {
        let clip_end = clip
            .timeline_start
            .checked_add(clip_duration)
            .unwrap_or(clip.timeline_start);
        let fade_out_start = clip_end
            .checked_sub(clip.audio_fade_out_frames)
            .unwrap_or(clip.timeline_start);
        #[allow(clippy::cast_precision_loss)]
        let constant_gain = 10.0_f32.powf(clip.audio_gain_tenth_db as f32 / 200.0);
        Self {
            constant_gain,
            fade_in: audio_gain_ramp(
                clip.timeline_start,
                clip.audio_fade_in_frames,
                sample_rate,
                project_fps,
            ),
            fade_out: audio_gain_ramp(
                fade_out_start,
                clip.audio_fade_out_frames,
                sample_rate,
                project_fps,
            ),
            transition: transition_audio_ramp(clip, sample_rate, project_fps),
        }
    }

    pub(crate) fn gain_at(self, project_sample: u64) -> f32 {
        let gain = self.constant_gain;
        let gain = gain
            * self
                .fade_in
                .map_or(1.0, |ramp| ramp.gain_at(project_sample));
        let gain = gain
            * self
                .fade_out
                .map_or(1.0, |ramp| 1.0 - ramp.gain_at(project_sample));
        gain * self
            .transition
            .map_or(1.0, |ramp| ramp.gain_at(project_sample))
    }
}

#[derive(Debug)]
enum AudioEffectState {
    Stateless,
    Eq {
        low: Vec<f32>,
        high_pass_source: Vec<f32>,
    },
    GainEnvelope(f32),
}

#[derive(Debug)]
struct AudioEffectRuntime {
    effect: Effect,
    state: AudioEffectState,
}

impl AudioEffectRuntime {
    fn new(effect: &Effect, channels: usize) -> Self {
        let state = match effect.name.as_str() {
            "audio_eq" => AudioEffectState::Eq {
                low: vec![0.0; channels],
                high_pass_source: vec![0.0; channels],
            },
            "audio_compressor" | "audio_ducking" => AudioEffectState::GainEnvelope(1.0),
            _ => AudioEffectState::Stateless,
        };
        Self {
            effect: effect.clone(),
            state,
        }
    }

    // Descriptor bounds keep every integer conversion exactly representable at
    // audio-control precision; keeping the ordered DSP chain together makes
    // its execution order auditable.
    #[allow(clippy::cast_precision_loss, clippy::too_many_lines)]
    fn process_frame(
        &mut self,
        samples: &mut [f32],
        sidechain: &[f32],
        project_at: TimeCode,
        sample_rate: u32,
    ) {
        match (&*self.effect.name, &mut self.state) {
            ("audio_gain", AudioEffectState::Stateless) => {
                let gain = db_gain(audio_value(&self.effect, "gain_tenth_db", project_at, 0));
                for sample in samples {
                    *sample *= gain;
                }
            }
            (
                "audio_eq",
                AudioEffectState::Eq {
                    low,
                    high_pass_source,
                },
            ) => {
                let low_gain = db_gain(audio_value(
                    &self.effect,
                    "low_gain_tenth_db",
                    project_at,
                    0,
                ));
                let mid_gain = db_gain(audio_value(
                    &self.effect,
                    "mid_gain_tenth_db",
                    project_at,
                    0,
                ));
                let high_gain = db_gain(audio_value(
                    &self.effect,
                    "high_gain_tenth_db",
                    project_at,
                    0,
                ));
                let low_coefficient = low_pass_coefficient(200.0, sample_rate);
                let high_coefficient = low_pass_coefficient(4_000.0, sample_rate);
                for (channel, sample) in samples.iter_mut().enumerate() {
                    let input = *sample;
                    low[channel] += low_coefficient * (input - low[channel]);
                    high_pass_source[channel] +=
                        high_coefficient * (input - high_pass_source[channel]);
                    let low_band = low[channel];
                    let high_band = input - high_pass_source[channel];
                    let mid_band = high_pass_source[channel] - low_band;
                    *sample = low_band * low_gain + mid_band * mid_gain + high_band * high_gain;
                }
            }
            ("audio_compressor", AudioEffectState::GainEnvelope(envelope)) => {
                let threshold_tenth_db =
                    audio_value(&self.effect, "threshold_tenth_db", project_at, 0);
                let ratio =
                    audio_value(&self.effect, "ratio_hundredths", project_at, 100) as f32 / 100.0;
                let level = samples
                    .iter()
                    .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
                let level_db = amplitude_db(level);
                let threshold_db = threshold_tenth_db as f32 / 10.0;
                let target = if level_db > threshold_db && ratio > 1.0 {
                    10.0_f32.powf(
                        ((threshold_db + (level_db - threshold_db) / ratio) - level_db) / 20.0,
                    )
                } else {
                    1.0
                };
                smooth_gain(
                    envelope,
                    target,
                    audio_value(&self.effect, "attack_milliseconds", project_at, 10),
                    audio_value(&self.effect, "release_milliseconds", project_at, 250),
                    sample_rate,
                );
                let makeup = db_gain(audio_value(
                    &self.effect,
                    "makeup_gain_tenth_db",
                    project_at,
                    0,
                ));
                for sample in samples {
                    *sample *= *envelope * makeup;
                }
            }
            ("audio_ducking", AudioEffectState::GainEnvelope(envelope)) => {
                let sidechain_level = sidechain
                    .iter()
                    .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
                let threshold = db_gain(audio_value(
                    &self.effect,
                    "threshold_tenth_db",
                    project_at,
                    -300,
                ));
                let target = if sidechain_level >= threshold {
                    db_gain(-audio_value(
                        &self.effect,
                        "reduction_tenth_db",
                        project_at,
                        120,
                    ))
                } else {
                    1.0
                };
                smooth_gain(
                    envelope,
                    target,
                    audio_value(&self.effect, "attack_milliseconds", project_at, 20),
                    audio_value(&self.effect, "release_milliseconds", project_at, 300),
                    sample_rate,
                );
                for sample in samples {
                    *sample *= *envelope;
                }
            }
            _ => {}
        }
    }
}

#[derive(Debug)]
struct AudioBusRuntime {
    tracks: Vec<TrackId>,
    sidechain_tracks: Vec<TrackId>,
    effects: Vec<AudioEffectRuntime>,
}

impl AudioBusRuntime {
    fn new(bus: &AudioBus, channels: usize) -> Self {
        Self {
            tracks: bus.tracks.clone(),
            sidechain_tracks: bus.ducking_sidechain_tracks.clone(),
            effects: bus
                .effects
                .iter()
                .map(|effect| AudioEffectRuntime::new(effect, channels))
                .collect(),
        }
    }
}

/// Stateful processor shared by real-time playback and export mixing.
pub(crate) struct AudioMixProcessor {
    buses: Vec<AudioBusRuntime>,
    routed_tracks: HashSet<TrackId>,
    sample_rate: u32,
    channels: usize,
    project_fps: Rational,
}

impl AudioMixProcessor {
    pub(crate) fn new(document: &Document, sample_rate: u32, channels: usize) -> Self {
        let routed_tracks = document
            .audio_mix
            .buses
            .iter()
            .flat_map(|bus| bus.tracks.iter().copied())
            .collect();
        Self {
            buses: document
                .audio_mix
                .buses
                .iter()
                .map(|bus| AudioBusRuntime::new(bus, channels))
                .collect(),
            routed_tracks,
            sample_rate,
            channels,
            project_fps: document.fps,
        }
    }

    pub(crate) fn mix_chunk(
        &mut self,
        track_buffers: &HashMap<TrackId, Vec<f32>>,
        start_sample: u64,
        sample_frames: usize,
    ) -> Result<Vec<f32>, MediaError> {
        let sample_count = sample_frames
            .checked_mul(self.channels)
            .ok_or_else(|| MediaError::Backend("audio mix chunk is too large".to_owned()))?;
        let mut master = vec![0.0_f32; sample_count];
        for (track, samples) in track_buffers {
            if !self.routed_tracks.contains(track) {
                add_signal(&mut master, samples);
            }
        }
        for bus in &mut self.buses {
            let mut signal = vec![0.0_f32; sample_count];
            for track in &bus.tracks {
                if let Some(samples) = track_buffers.get(track) {
                    add_signal(&mut signal, samples);
                }
            }
            let mut sidechain = vec![0.0_f32; sample_count];
            for track in &bus.sidechain_tracks {
                if let Some(samples) = track_buffers.get(track) {
                    add_signal(&mut sidechain, samples);
                }
            }
            for frame in 0..sample_frames {
                let start = frame * self.channels;
                let end = start + self.channels;
                let project_at = samples_to_frame(
                    start_sample.saturating_add(u64::try_from(frame).unwrap_or(u64::MAX)),
                    self.sample_rate,
                    self.project_fps,
                );
                for effect in &mut bus.effects {
                    effect.process_frame(
                        &mut signal[start..end],
                        &sidechain[start..end],
                        project_at,
                        self.sample_rate,
                    );
                }
            }
            add_signal(&mut master, &signal);
        }
        Ok(master)
    }
}

fn add_signal(destination: &mut [f32], source: &[f32]) {
    for (destination, source) in destination.iter_mut().zip(source) {
        *destination += source;
    }
}

fn audio_value(effect: &Effect, name: &str, at: TimeCode, neutral: i64) -> i64 {
    effect.integer_parameter_at(name, at).unwrap_or(neutral)
}

#[allow(clippy::cast_precision_loss)]
fn db_gain(tenth_db: i64) -> f32 {
    10.0_f32.powf(tenth_db as f32 / 200.0)
}

fn amplitude_db(amplitude: f32) -> f32 {
    20.0 * amplitude.max(0.000_001).log10()
}

#[allow(clippy::cast_precision_loss)]
fn low_pass_coefficient(frequency: f32, sample_rate: u32) -> f32 {
    1.0 - (-2.0 * std::f32::consts::PI * frequency / sample_rate.max(1) as f32).exp()
}

#[allow(clippy::cast_precision_loss)]
fn smooth_gain(
    current: &mut f32,
    target: f32,
    attack_milliseconds: i64,
    release_milliseconds: i64,
    sample_rate: u32,
) {
    let milliseconds = if target < *current {
        attack_milliseconds
    } else {
        release_milliseconds
    }
    .max(1) as f32;
    let samples = milliseconds * sample_rate.max(1) as f32 / 1_000.0;
    let coefficient = (-1.0 / samples.max(1.0)).exp();
    *current = coefficient * *current + (1.0 - coefficient) * target;
}

struct AudioMixSource {
    track: TrackId,
    path: PathBuf,
    output_rate: u32,
    output_channels: u16,
    source_sample_start: u64,
    source_sample_end: u64,
    project_sample_start: u64,
    project_sample_end: u64,
    shaping: ClipAudioShaping,
    next_project_sample: u64,
    next_channel: usize,
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
                let gain = self.shaping.gain_at(self.next_project_sample);
                destination[destination_index] += self.pending[self.pending_index] * gain;
                destination_index += 1;
                self.pending_index += 1;
                self.next_channel = self.next_channel.saturating_add(1);
                if self.next_channel >= usize::from(self.output_channels).max(1) {
                    self.next_channel = 0;
                    self.next_project_sample = self.next_project_sample.saturating_add(1);
                }
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
    meter: Option<Arc<MeterState>>,
    processor: AudioMixProcessor,
}

impl AudioMixer {
    fn open(
        document: &Document,
        project_from: TimeCode,
        output_rate: u32,
        output_channels: u16,
        meter: Option<Arc<MeterState>>,
    ) -> Result<Self, MediaError> {
        let project_end = document.duration;
        let needs_preroll = !document.audio_mix.is_empty() && project_from > TimeCode::ZERO;
        let decode_from = if needs_preroll {
            TimeCode::ZERO
        } else {
            project_from
        };
        let segments = timeline_audio_segments(document, decode_from..project_end)?;
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
            let clip_duration = document
                .clip_duration(clip)
                .map_err(|error| MediaError::Backend(error.to_string()))?;
            sources.push(AudioMixSource {
                track: segment.track,
                path: asset.path.clone(),
                output_rate,
                output_channels,
                source_sample_start,
                source_sample_end,
                project_sample_start,
                project_sample_end,
                shaping: ClipAudioShaping::new(clip, clip_duration, output_rate, document.fps),
                next_project_sample: project_sample_start,
                next_channel: 0,
                decoder: None,
                opened: false,
                pending: Vec::new(),
                pending_index: 0,
                finished: false,
            });
        }
        let target_sample = frame_to_samples(project_from, output_rate, document.fps);
        let mut mixer = Self {
            processor: AudioMixProcessor::new(document, output_rate, usize::from(output_channels)),
            sources,
            output_channels: usize::from(output_channels),
            cursor_sample: frame_to_samples(decode_from, output_rate, document.fps),
            end_sample: frame_to_samples(project_end, output_rate, document.fps),
            meter: None,
        };
        while mixer.cursor_sample < target_sample {
            let remaining = target_sample - mixer.cursor_sample;
            let limit = usize::try_from(remaining)
                .unwrap_or(usize::MAX)
                .min(MIX_CHUNK_SAMPLE_FRAMES);
            if mixer.next_chunk_limited(limit)?.is_none() {
                break;
            }
        }
        mixer.meter = meter;
        Ok(mixer)
    }

    fn next_chunk(&mut self) -> Result<Option<Vec<f32>>, MediaError> {
        self.next_chunk_limited(MIX_CHUNK_SAMPLE_FRAMES)
    }

    fn next_chunk_limited(
        &mut self,
        maximum_sample_frames: usize,
    ) -> Result<Option<Vec<f32>>, MediaError> {
        if self.cursor_sample >= self.end_sample {
            return Ok(None);
        }
        let remaining = self.end_sample.saturating_sub(self.cursor_sample);
        let sample_frames = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(maximum_sample_frames.max(1));
        let chunk_end = self
            .cursor_sample
            .saturating_add(u64::try_from(sample_frames).unwrap_or(u64::MAX));
        let sample_count = sample_frames
            .checked_mul(self.output_channels)
            .ok_or_else(|| MediaError::Backend("audio mix chunk is too large".to_owned()))?;
        let mut track_buffers = HashMap::<TrackId, Vec<f32>>::new();
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
                .min(sample_count);
            let track = track_buffers
                .entry(source.track)
                .or_insert_with(|| vec![0.0; sample_count]);
            source.add_samples(&mut track[start..end])?;
            if overlap_end >= source.project_sample_end {
                source.retire();
            }
        }
        let mut mixed =
            self.processor
                .mix_chunk(&track_buffers, self.cursor_sample, sample_frames)?;
        if let Some(meter) = &self.meter {
            limit_and_meter_audio_mix(&mut mixed, self.output_channels, meter);
        } else {
            limit_audio_mix(&mut mixed);
        }
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
        meter: Arc<MeterState>,
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
        let mixer = AudioMixer::open(document, project_from, sample_rate, channels, Some(meter))?;
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
    use std::collections::BTreeMap;

    use openreel_core::{
        AssetId, AudioBus, AudioBusId, AudioMix, AutomationCurve, Clip, ClipId, Effect, EffectId,
        ExportSettings, Keyframe, KeyframeInterpolation, MediaAsset, MediaKind, ParamValue, Track,
        TrackId, TrackKind, Transition,
    };

    use crate::test_support::GeneratedMedia;

    use super::*;

    fn audio_effect(id: u64, name: &str, parameters: &[(&str, i64)]) -> Effect {
        Effect {
            id: EffectId(id),
            name: name.to_owned(),
            parameters: parameters
                .iter()
                .map(|(name, value)| ((*name).to_owned(), ParamValue::Integer(*value)))
                .collect(),
            keyframes: BTreeMap::new(),
        }
    }

    fn processor_document(fps: Rational, duration: i64, buses: Vec<AudioBus>) -> Document {
        Document {
            fps,
            duration: TimeCode(duration),
            tracks: vec![
                Track {
                    id: TrackId(1),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: Vec::new(),
                },
                Track {
                    id: TrackId(2),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: Vec::new(),
                },
            ],
            audio_mix: AudioMix { buses },
            ..Document::default()
        }
    }

    #[test]
    fn bus_gain_automation_uses_exact_project_frames() {
        let mut gain = audio_effect(1, "audio_gain", &[("gain_tenth_db", -600)]);
        gain.keyframes.insert(
            "gain_tenth_db".to_owned(),
            AutomationCurve {
                keyframes: vec![
                    Keyframe {
                        at: TimeCode::ZERO,
                        value: -600,
                        interpolation: KeyframeInterpolation::Linear,
                    },
                    Keyframe {
                        at: TimeCode(10),
                        value: 0,
                        interpolation: KeyframeInterpolation::Linear,
                    },
                ],
            },
        );
        let document = processor_document(
            Rational::new(10, 1).unwrap(),
            11,
            vec![AudioBus {
                id: AudioBusId(1),
                name: "Music".to_owned(),
                tracks: vec![TrackId(1)],
                effects: vec![gain],
                ducking_sidechain_tracks: Vec::new(),
            }],
        );
        let tracks = HashMap::from([(TrackId(1), vec![1.0; 11])]);
        let output = AudioMixProcessor::new(&document, 10, 1)
            .mix_chunk(&tracks, 0, 11)
            .unwrap();

        assert_close(output[0], 0.001);
        assert_close(output[5], 10.0_f32.powf(-30.0 / 20.0));
        assert_close(output[10], 1.0);
    }

    #[test]
    fn eq_compressor_and_ducking_execute_in_order_on_real_samples() {
        let document = processor_document(
            Rational::new(1_000, 1).unwrap(),
            2_000,
            vec![
                AudioBus {
                    id: AudioBusId(1),
                    name: "Bed".to_owned(),
                    tracks: vec![TrackId(1)],
                    effects: vec![
                        audio_effect(1, "audio_eq", &[("low_gain_tenth_db", -240)]),
                        audio_effect(
                            2,
                            "audio_compressor",
                            &[
                                ("threshold_tenth_db", -200),
                                ("ratio_hundredths", 1_000),
                                ("attack_milliseconds", 1),
                            ],
                        ),
                        audio_effect(
                            3,
                            "audio_ducking",
                            &[
                                ("threshold_tenth_db", -300),
                                ("reduction_tenth_db", 200),
                                ("attack_milliseconds", 1),
                            ],
                        ),
                    ],
                    ducking_sidechain_tracks: vec![TrackId(2)],
                },
                AudioBus {
                    id: AudioBusId(2),
                    name: "Sidechain monitor".to_owned(),
                    tracks: vec![TrackId(2)],
                    effects: vec![audio_effect(4, "audio_gain", &[("gain_tenth_db", -600)])],
                    ducking_sidechain_tracks: Vec::new(),
                },
            ],
        );
        let tracks = HashMap::from([
            (TrackId(1), vec![1.0; 2_000]),
            (TrackId(2), vec![1.0; 2_000]),
        ]);
        let output = AudioMixProcessor::new(&document, 1_000, 1)
            .mix_chunk(&tracks, 0, 2_000)
            .unwrap();

        assert!(output[1_999] > 0.001, "processed bed must remain audible");
        assert!(
            output[1_999] < 0.03,
            "EQ, compression, and ducking should reduce steady full-scale input: {}",
            output[1_999]
        );
    }

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
    fn clip_audio_shaping_applies_constant_tenth_db_gain() {
        let mut clip = audio_clip(1, 1, 0..10, 10);
        clip.audio_gain_tenth_db = -60;
        let shaping =
            ClipAudioShaping::new(&clip, TimeCode(10), 100, Rational::new(10, 1).unwrap());

        assert_close(shaping.gain_at(150), 10.0_f32.powf(-60.0 / 200.0));
    }

    #[test]
    fn clip_audio_shaping_fades_linearly_and_anchors_fade_out_to_clip_end() {
        let mut clip = audio_clip(1, 1, 0..10, 10);
        clip.audio_fade_in_frames = TimeCode(2);
        clip.audio_fade_out_frames = TimeCode(2);
        let shaping =
            ClipAudioShaping::new(&clip, TimeCode(10), 100, Rational::new(10, 1).unwrap());

        assert_close(shaping.gain_at(100), 0.0);
        assert_close(shaping.gain_at(119), 1.0);
        assert_close(shaping.gain_at(150), 1.0);
        assert_close(shaping.gain_at(179), 1.0);
        assert_close(shaping.gain_at(180), 1.0);
        assert_close(shaping.gain_at(199), 0.0);
    }

    #[test]
    fn clip_audio_shaping_multiplies_gain_fade_and_transition_in_fixed_order() {
        let mut clip = audio_clip(1, 1, 0..10, 10);
        clip.audio_gain_tenth_db = -60;
        clip.audio_fade_in_frames = TimeCode(2);
        clip.transition_in = Some(Transition {
            name: "crossfade".to_owned(),
            duration: TimeCode(4),
        });
        let shaping =
            ClipAudioShaping::new(&clip, TimeCode(10), 100, Rational::new(10, 1).unwrap());
        let constant = 10.0_f32.powf(-60.0 / 200.0);
        let expected = (constant * (10.0 / 19.0)) * (10.0 / 39.0);

        assert_close(shaping.gain_at(110), expected);
    }

    #[test]
    fn one_frame_audio_fades_and_transition_are_no_ops() {
        let mut clip = audio_clip(1, 1, 0..10, 10);
        clip.audio_fade_in_frames = TimeCode(1);
        clip.audio_fade_out_frames = TimeCode(1);
        clip.transition_in = Some(Transition {
            name: "crossfade".to_owned(),
            duration: TimeCode(1),
        });
        let shaping =
            ClipAudioShaping::new(&clip, TimeCode(10), 100, Rational::new(10, 1).unwrap());

        assert_close(shaping.gain_at(100), 1.0);
        assert_close(shaping.gain_at(199), 1.0);
    }

    #[test]
    fn meter_state_records_post_limiter_stereo_chunk_peaks() {
        let meter = MeterState::default();
        let mut chunk = [1.5, -0.25, -0.5, 0.75];

        limit_and_meter_audio_mix(&mut chunk, 2, &meter);

        for (actual, expected) in chunk.into_iter().zip([1.0, -0.25, -0.5, 0.75]) {
            assert_close(actual, expected);
        }
        for (actual, expected) in meter.peaks().into_iter().zip([1.0, 0.75]) {
            assert_close(actual, expected);
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn playback_feeder_mix_matches_export_across_overlap_trim_gap_and_clamp() {
        crate::initialize_ffmpeg().unwrap();
        let voice = loud_sine("m12-voice", 440);
        let bed = loud_sine("m12-bed", 660);
        let fps = Rational::new(10, 1).unwrap();
        let mut document = parity_document(voice.path(), bed.path(), fps);
        document.tracks[0].clips[1].transition_in = Some(Transition {
            name: "fade_from_black".to_owned(),
            duration: TimeCode(2),
        });
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
        let mut playback = AudioMixer::open(&document, TimeCode::ZERO, 48_000, 2, None).unwrap();
        let played = playback.render_remaining().unwrap();

        assert_eq!(played.len(), exported.len());
        for (name, frames) in [
            ("single source", 0..4),
            ("overlap", 4..6),
            ("trimmed source", 6..14),
            ("silence", 14..16),
            ("transition fade-in", 16..18),
            ("post-transition steady state", 18..20),
            ("clip fade-in", 20..22),
            ("gained clip steady state", 22..28),
            ("clip fade-out", 28..30),
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
        // Stateful EQ is allowed to ring briefly after the cut at frame 14;
        // the second half of the gap must settle below -80 dBFS.
        let silence = interleaved_sample_range(15..16, fps, 48_000, 2);
        let silence_peak = exported[silence]
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0_f32, f32::max);
        assert!(
            silence_peak <= 1.0e-4,
            "processed filter tail {silence_peak} exceeded -80 dBFS in the fixture gap"
        );
        let transition_start = interleaved_sample_range(16..17, fps, 48_000, 2);
        let steady_state = interleaved_sample_range(18..20, fps, 48_000, 2);
        let transition_peak = exported[transition_start]
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0_f32, f32::max);
        let steady_peak = exported[steady_state]
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0_f32, f32::max);
        assert!(
            transition_peak < steady_peak * 0.75,
            "transition first-frame peak {transition_peak} was not attenuated below steady {steady_peak}"
        );

        let gained_head = peak_in(&exported, 20..21, fps);
        let gained_steady = peak_in(&exported, 22..28, fps);
        let gained_tail = peak_in(&exported, 29..30, fps);
        assert!(
            gained_head < gained_steady * 0.75,
            "clip fade-in head {gained_head} was not attenuated below steady {gained_steady}"
        );
        assert!(
            gained_tail < gained_steady * 0.75,
            "clip fade-out tail {gained_tail} was not attenuated below steady {gained_steady}"
        );
        let expected_gain = 10.0_f32.powf(-60.0 / 200.0);
        let gained_ratio = gained_steady / steady_peak;
        assert!(
            (gained_ratio - expected_gain).abs() <= 0.02,
            "-6.0 dB steady peak ratio {gained_ratio} did not approximate {expected_gain}"
        );

        let seek_sample = usize::try_from(frame_to_samples(TimeCode(5), 48_000, fps))
            .unwrap()
            .saturating_mul(2);
        let mut seeked_playback =
            AudioMixer::open(&document, TimeCode(5), 48_000, 2, None).unwrap();
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
            catalog: openreel_core::MediaCatalog::default(),
            audio_mix: openreel_core::AudioMix::default(),
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
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
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

        let mut mixer = AudioMixer::open(&document, TimeCode::ZERO, 48_000, 2, None).unwrap();
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
            catalog: openreel_core::MediaCatalog::default(),
            audio_mix: AudioMix {
                buses: vec![AudioBus {
                    id: AudioBusId(1),
                    name: "Bed duck".to_owned(),
                    tracks: vec![TrackId(2)],
                    effects: vec![
                        audio_effect(10, "audio_eq", &[("high_gain_tenth_db", -30)]),
                        audio_effect(
                            11,
                            "audio_compressor",
                            &[("threshold_tenth_db", -120), ("ratio_hundredths", 400)],
                        ),
                        audio_effect(
                            12,
                            "audio_ducking",
                            &[("threshold_tenth_db", -300), ("reduction_tenth_db", 60)],
                        ),
                        audio_effect(13, "audio_gain", &[("gain_tenth_db", 120)]),
                    ],
                    ducking_sidechain_tracks: vec![TrackId(1)],
                }],
            },
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
                Track {
                    id: TrackId(3),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: vec![Clip {
                        audio_gain_tenth_db: -60,
                        audio_fade_in_frames: TimeCode(2),
                        audio_fade_out_frames: TimeCode(2),
                        ..audio_clip(4, 2, 0..10, 20)
                    }],
                },
            ],
            media_pool: vec![
                audio_asset(1, voice, "voice-440", fps),
                audio_asset(2, bed, "bed-660", fps),
            ],
            markers: Vec::new(),
            fps,
            resolution: (64, 64),
            duration: TimeCode(30),
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
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
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

    fn peak_in(samples: &[f32], frames: std::ops::Range<i64>, fps: Rational) -> f32 {
        samples[interleaved_sample_range(frames, fps, 48_000, 2)]
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0_f32, f32::max)
    }

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() <= 1.0e-6,
            "expected {expected}, got {actual}"
        );
    }
}
