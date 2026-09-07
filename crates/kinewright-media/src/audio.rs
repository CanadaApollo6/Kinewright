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
use kinewright_core::{
    AudioBus, AudioBusId, Clip, Document, Effect, ExportCancellation, MediaError, MixPeaks,
    Rational, TimeCode, TrackId,
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
/// AU1 §5.3: the ring keeps [`BUFFER_SECONDS`] of capacity but the fill loop
/// stops once this much audio is already queued, so a live mixer edit is heard
/// about a second after the gesture instead of up to two.
const LIVE_FILL_MILLISECONDS: usize = 1_000;
/// AU1 §3.4: the per-channel ramp a live track-mix change is smoothed over.
const TRACK_MIX_RAMP_MILLISECONDS: u32 = 5;

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

/// One lock-free peak slot per mix point (AU1 §4.1).
///
/// Telemetry, not document state: every slot is overwritten per chunk exactly
/// as [`MeterState`] is, and the master slot *is* the engine's own meter, so
/// `Playback::output_peaks` keeps its existing behaviour.
#[derive(Debug)]
pub(crate) struct MixMeters {
    tracks: Vec<(TrackId, MeterState)>,
    buses: Vec<(AudioBusId, MeterState)>,
    master: Arc<MeterState>,
}

impl MixMeters {
    /// One slot per `document.tracks` entry in order, one per bus in order,
    /// plus the shared master slot (AU1 §4.1).
    pub(crate) fn for_document(document: &Document, master: Arc<MeterState>) -> Self {
        Self {
            tracks: document
                .tracks
                .iter()
                .map(|track| (track.id, MeterState::default()))
                .collect(),
            buses: document
                .audio_mix
                .buses
                .iter()
                .map(|bus| (bus.id, MeterState::default()))
                .collect(),
            master,
        }
    }

    /// The table installed whenever the worker is not playing (AU1 §4.1).
    pub(crate) fn empty(master: Arc<MeterState>) -> Self {
        Self {
            tracks: Vec::new(),
            buses: Vec::new(),
            master,
        }
    }

    pub(crate) fn peaks(&self) -> MixPeaks {
        MixPeaks {
            tracks: self
                .tracks
                .iter()
                .map(|(track, state)| (*track, state.peaks()))
                .collect(),
            buses: self
                .buses
                .iter()
                .map(|(bus, state)| (*bus, state.peaks()))
                .collect(),
            master: self.master.peaks(),
        }
    }

    /// Clear every slot, master included (AU1 §4.1).
    pub(crate) fn clear(&self) {
        for (_, state) in &self.tracks {
            state.clear();
        }
        for (_, state) in &self.buses {
            state.clear();
        }
        self.master.clear();
    }

    fn record_track(&self, track: TrackId, samples: &[f32], channel_count: usize) {
        if let Some((_, state)) = self.tracks.iter().find(|(id, _)| *id == track) {
            state.record_chunk(samples, channel_count);
        }
    }

    fn record_bus(&self, bus: AudioBusId, samples: &[f32], channel_count: usize) {
        if let Some((_, state)) = self.buses.iter().find(|(id, _)| *id == bus) {
            state.record_chunk(samples, channel_count);
        }
    }
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
            ("audio_limiter", AudioEffectState::Stateless) => {
                let ceiling = db_gain(audio_value(
                    &self.effect,
                    "ceiling_tenth_db",
                    project_at,
                    -10,
                ));
                for sample in samples {
                    *sample = sample.clamp(-ceiling, ceiling);
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
    id: AudioBusId,
    tracks: Vec<TrackId>,
    sidechain_tracks: Vec<TrackId>,
    effects: Vec<AudioEffectRuntime>,
}

impl AudioBusRuntime {
    fn new(bus: &AudioBus, channels: usize) -> Self {
        Self {
            id: bus.id,
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

/// AU1 §3.4: the ramp length in sample frames for one output rate.
fn track_mix_ramp_frames(sample_rate: u32) -> usize {
    usize::try_from(sample_rate.max(1))
        .unwrap_or(48_000)
        .saturating_mul(TRACK_MIX_RAMP_MILLISECONDS as usize)
        .saturating_div(1_000)
        .max(1)
}

/// AU1 §3.1 balance law: centre is the exact identity and the law never boosts.
#[allow(clippy::cast_precision_loss)]
fn pan_channel_ratios(pan_percent: i32) -> [f32; 2] {
    let pan = pan_percent as f32 / 100.0;
    [1.0 - pan.max(0.0), 1.0 + pan.min(0.0)]
}

/// AU1 §3.1: gate, gain, and the per-channel fold of gain and balance pan.
///
/// Returns `(audible, gain, per-channel target)`. A non-audible track targets
/// exact zeros; a one-channel device hears no pan.
fn track_stage_parameters(
    document: &Document,
    track: TrackId,
    channels: usize,
) -> (bool, f32, [f32; 2]) {
    if !document.track_audible(track) {
        return (false, 0.0, [0.0, 0.0]);
    }
    let mix = document.track_mix(track);
    let gain = db_gain(i64::from(mix.gain_tenth_db));
    if channels < 2 {
        return (true, gain, [gain, gain]);
    }
    let pan = pan_channel_ratios(mix.pan_percent);
    (true, gain, [gain * pan[0], gain * pan[1]])
}

/// One track's live stage state (AU1 §3.3).
#[derive(Debug)]
struct TrackStageRuntime {
    track: TrackId,
    audible: bool,
    /// Steady-state per-channel gain for channels 0 and 1; channels >= 2 use `gain`.
    target: [f32; 2],
    current: [f32; 2],
    ramp_start: [f32; 2],
    /// Equal to the processor's ramp length when settled (AU1 §3.4).
    ramp_index: usize,
    gain: f32,
}

impl TrackStageRuntime {
    /// A freshly built stage is settled, so export and a newly opened playback
    /// mixer never ramp (AU1 §3.4).
    fn new(document: &Document, track: TrackId, channels: usize, ramp_frames: usize) -> Self {
        let (audible, gain, target) = track_stage_parameters(document, track, channels);
        Self {
            track,
            audible,
            target,
            current: target,
            ramp_start: target,
            ramp_index: ramp_frames,
            gain,
        }
    }

    /// AU1 §3.4: retarget, ramping from the current interpolated value when
    /// the new target differs from it.
    // The comparison is an exact identity test, not a tolerance question: an
    // unchanged target must not restart the ramp.
    #[allow(clippy::float_cmp)]
    fn retarget(&mut self, document: &Document, channels: usize, ramp_frames: usize) {
        let (audible, gain, target) = track_stage_parameters(document, self.track, channels);
        self.audible = audible;
        self.gain = gain;
        self.target = target;
        if target == self.current {
            self.ramp_index = ramp_frames;
        } else {
            self.ramp_start = self.current;
            self.ramp_index = 0;
        }
    }

    fn channel_gain(&self, channel: usize) -> f32 {
        if channel < 2 {
            self.current[channel]
        } else {
            self.gain
        }
    }

    // The ramp position is an integer sample-frame index converted once per frame.
    #[allow(clippy::cast_precision_loss)]
    fn advance_ramp(&mut self, ramp_frames: usize) {
        if self.ramp_index >= ramp_frames {
            return;
        }
        let progress = (self.ramp_index + 1) as f32 / ramp_frames as f32;
        for channel in 0..2 {
            self.current[channel] = self.ramp_start[channel]
                + (self.target[channel] - self.ramp_start[channel]) * progress;
        }
        self.ramp_index += 1;
        if self.ramp_index >= ramp_frames {
            self.current = self.target;
        }
    }

    /// AU1 §3.4: advance the ramp across `frames` output sample frames in which
    /// this track carried no media.
    ///
    /// The silent and the audible arms of `mix` must advance the ramp by the
    /// same count for a given chunk: this takes `sample_frames`, while `apply`
    /// takes one step per source frame, and both production callers pass a
    /// buffer of exactly `sample_frames * channels` samples.
    ///
    /// The ramp is defined over output frames, not over frames in which this
    /// track has audio, so a live change made during a gap has settled by the
    /// time the next clip starts instead of blipping through its first 5 ms.
    // The ramp position is an integer sample-frame index converted once.
    #[allow(clippy::cast_precision_loss)]
    fn skip_ramp(&mut self, frames: usize, ramp_frames: usize) {
        if self.ramp_index >= ramp_frames {
            return;
        }
        let index = self.ramp_index.saturating_add(frames);
        if index >= ramp_frames {
            self.ramp_index = ramp_frames;
            self.current = self.target;
            return;
        }
        let progress = index as f32 / ramp_frames as f32;
        for channel in 0..2 {
            self.current[channel] = self.ramp_start[channel]
                + (self.target[channel] - self.ramp_start[channel]) * progress;
        }
        self.ramp_index = index;
    }

    /// Multiply one chunk of one track by its per-channel stage gain (AU1 §3.1).
    ///
    /// Every sample is multiplied exactly once. A settled neutral stage is an
    /// exact pass-through and a settled silenced stage writes exact zeros.
    ///
    /// While ramping this advances the ramp once per source frame,
    /// `ceil(source.len() / channels)`, which equals the `sample_frames` that
    /// [`Self::skip_ramp`] applies to a silent track for both production
    /// callers; keep the two equal for any new caller (AU1 §3.4).
    // The neutral fast path is an exact identity test: `x * 1.0 == x` in IEEE 754,
    // so a neutral track stays bit-identical to a pass-through.
    #[allow(clippy::float_cmp)]
    fn apply(
        &mut self,
        source: &[f32],
        destination: &mut [f32],
        channels: usize,
        ramp_frames: usize,
    ) {
        let channels = channels.max(1);
        if self.ramp_index >= ramp_frames {
            // AU1 §3.1: a gated track writes exact zeros. A settled stage has
            // `current == target`, and `!audible` targets `[0.0, 0.0]`.
            if !self.audible {
                destination.fill(0.0);
                return;
            }
            if self.gain == 1.0 && self.target == [1.0, 1.0] {
                destination.copy_from_slice(source);
                return;
            }
            for (index, (out, sample)) in destination.iter_mut().zip(source).enumerate() {
                *out = sample * self.channel_gain(index % channels);
            }
            return;
        }
        for (out_frame, source_frame) in destination
            .chunks_mut(channels)
            .zip(source.chunks(channels))
        {
            self.advance_ramp(ramp_frames);
            for (channel, (out, sample)) in out_frame.iter_mut().zip(source_frame).enumerate() {
                *out = sample * self.channel_gain(channel);
            }
        }
    }
}

/// AU1 §6: one chunk's post-stage, post-bus, and post-sum copies.
pub(crate) struct MixChunkStems {
    /// Parallel to the processor's document track order.
    pub(crate) tracks: Vec<Vec<f32>>,
    /// Parallel to the document's buses.
    pub(crate) buses: Vec<Vec<f32>>,
    pub(crate) master: Vec<f32>,
}

/// Stateful processor shared by real-time playback and export mixing.
pub(crate) struct AudioMixProcessor {
    /// `document.tracks` order: the deterministic summation order (AU1 §3.2).
    track_order: Vec<TrackId>,
    /// Parallel to `track_order`.
    stages: Vec<TrackStageRuntime>,
    /// Post-track-stage scratch, one per track, resized per chunk, never freed.
    staged: Vec<Vec<f32>>,
    buses: Vec<AudioBusRuntime>,
    routed_tracks: HashSet<TrackId>,
    /// `None` in export and measurement (AU1 §4.1).
    meters: Option<Arc<MixMeters>>,
    ramp_frames: usize,
    sample_rate: u32,
    channels: usize,
    project_fps: Rational,
}

impl AudioMixProcessor {
    pub(crate) fn new(
        document: &Document,
        sample_rate: u32,
        channels: usize,
        meters: Option<Arc<MixMeters>>,
    ) -> Self {
        let routed_tracks = document
            .audio_mix
            .buses
            .iter()
            .flat_map(|bus| bus.tracks.iter().copied())
            .collect();
        let ramp_frames = track_mix_ramp_frames(sample_rate);
        let track_order = document
            .tracks
            .iter()
            .map(|track| track.id)
            .collect::<Vec<_>>();
        let stages = track_order
            .iter()
            .map(|track| TrackStageRuntime::new(document, *track, channels, ramp_frames))
            .collect();
        let scratch = track_order.iter().map(|_| Vec::new()).collect();
        Self {
            track_order,
            stages,
            staged: scratch,
            buses: document
                .audio_mix
                .buses
                .iter()
                .map(|bus| AudioBusRuntime::new(bus, channels))
                .collect(),
            routed_tracks,
            meters,
            ramp_frames,
            sample_rate,
            channels,
            project_fps: document.fps,
        }
    }

    /// Attach the peak table once seek preroll has finished (AU1 §4.1).
    pub(crate) fn attach_meters(&mut self, meters: Option<Arc<MixMeters>>) {
        self.meters = meters;
    }

    /// AU1 §5.3: replace stage targets from a document whose tracks and buses
    /// are unchanged. Bus effect state is untouched and each changed stage
    /// ramps from its current value (AU1 §3.4).
    pub(crate) fn update_track_mix(&mut self, document: &Document) {
        let channels = self.channels;
        let ramp_frames = self.ramp_frames;
        for stage in &mut self.stages {
            stage.retarget(document, channels, ramp_frames);
        }
    }

    pub(crate) fn mix_chunk(
        &mut self,
        track_buffers: &HashMap<TrackId, Vec<f32>>,
        start_sample: u64,
        sample_frames: usize,
    ) -> Result<Vec<f32>, MediaError> {
        self.mix(track_buffers, start_sample, sample_frames, false)
            .map(|stems| stems.master)
    }

    /// AU1 §6: `mix_chunk` plus post-stage and post-bus copies for measurement.
    pub(crate) fn mix_chunk_with_stems(
        &mut self,
        track_buffers: &HashMap<TrackId, Vec<f32>>,
        start_sample: u64,
        sample_frames: usize,
    ) -> Result<MixChunkStems, MediaError> {
        self.mix(track_buffers, start_sample, sample_frames, true)
    }

    fn mix(
        &mut self,
        track_buffers: &HashMap<TrackId, Vec<f32>>,
        start_sample: u64,
        sample_frames: usize,
        collect_stems: bool,
    ) -> Result<MixChunkStems, MediaError> {
        let sample_count = sample_frames
            .checked_mul(self.channels)
            .ok_or_else(|| MediaError::Backend("audio mix chunk is too large".to_owned()))?;
        let channels = self.channels.max(1);
        let ramp_frames = self.ramp_frames;

        // Track stage (AU1 §3.1), in document order. Routing, bus, and
        // sidechain sums read `staged` below, never `track_buffers`.
        for index in 0..self.track_order.len() {
            let track = self.track_order[index];
            let Some(samples) = track_buffers.get(&track) else {
                // AU1 §3.4: the ramp runs on output frames, so it keeps running
                // while this track is silent.
                self.stages[index].skip_ramp(sample_frames, ramp_frames);
                if collect_stems {
                    let staged = &mut self.staged[index];
                    staged.clear();
                    staged.resize(sample_count, 0.0);
                }
                // AU1 §4.1 is overwrite semantics: a track that contributes
                // nothing this chunk reads zero, so its meter falls when its
                // last clip ends. Recording an empty slice allocates nothing.
                if let Some(meters) = &self.meters {
                    meters.record_track(track, &[], channels);
                }
                continue;
            };
            let length = sample_count.min(samples.len());
            {
                let stage = &mut self.stages[index];
                let staged = &mut self.staged[index];
                // `apply` writes every sample of `staged[..length]` exactly once
                // (AU1 §3.1), so only the tail past the source needs zeroing.
                staged.resize(sample_count, 0.0);
                stage.apply(
                    &samples[..length],
                    &mut staged[..length],
                    channels,
                    ramp_frames,
                );
                staged[length..].fill(0.0);
            }
            if let Some(meters) = &self.meters {
                meters.record_track(track, &self.staged[index], channels);
            }
        }

        let mut master = vec![0.0_f32; sample_count];
        for (index, track) in self.track_order.iter().enumerate() {
            if !self.routed_tracks.contains(track) && track_buffers.contains_key(track) {
                add_signal(&mut master, &self.staged[index]);
            }
        }

        let mut bus_stems = Vec::new();
        for bus in &mut self.buses {
            let mut signal = vec![0.0_f32; sample_count];
            for track in &bus.tracks {
                if track_buffers.contains_key(track)
                    && let Some(index) = self.track_order.iter().position(|id| id == track)
                {
                    add_signal(&mut signal, &self.staged[index]);
                }
            }
            let mut sidechain = vec![0.0_f32; sample_count];
            for track in &bus.sidechain_tracks {
                if track_buffers.contains_key(track)
                    && let Some(index) = self.track_order.iter().position(|id| id == track)
                {
                    add_signal(&mut sidechain, &self.staged[index]);
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
            if let Some(meters) = &self.meters {
                meters.record_bus(bus.id, &signal, channels);
            }
            add_signal(&mut master, &signal);
            if collect_stems {
                bus_stems.push(signal);
            }
        }

        Ok(MixChunkStems {
            tracks: if collect_stems {
                self.staged.clone()
            } else {
                Vec::new()
            },
            buses: bus_stems,
            master,
        })
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

/// AU1 §2.2: preroll is a property of stateful bus effects, not of the
/// stateless track stage, so a document carrying only track-mix entries seeks
/// straight to its target.
fn needs_seek_preroll(document: &Document, project_from: TimeCode) -> bool {
    !document.audio_mix.buses.is_empty() && project_from > TimeCode::ZERO
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
        let needs_preroll = needs_seek_preroll(document, project_from);
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
            // AU1 §4.1: the peak table is attached after preroll, exactly as
            // the master meter below is, so preroll chunks are not metered.
            processor: AudioMixProcessor::new(
                document,
                output_rate,
                usize::from(output_channels),
                None,
            ),
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

    /// AU1 §4.1: install the peak table once preroll has finished.
    fn attach_mix_meters(&mut self, meters: Arc<MixMeters>) {
        self.processor.attach_meters(Some(meters));
    }

    /// AU1 §5.3: apply new track-stage targets to a running mixer.
    fn update_track_mix(&mut self, document: &Document) {
        self.processor.update_track_mix(document);
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

/// AU1 §5.3: push mixed audio into the output ring until it holds
/// `target_samples`, the mix is exhausted, or the ring is full.
///
/// Device-free so the fill target is testable without an audio device.
/// Returns `false` when the mixer has no more audio to deliver.
fn fill_ring(
    producer: &mut Producer<f32>,
    pending: &mut Vec<f32>,
    pending_index: &mut usize,
    mixer: &mut AudioMixer,
    target_samples: usize,
) -> Result<bool, MediaError> {
    loop {
        let queued = producer
            .buffer()
            .capacity()
            .saturating_sub(producer.slots());
        if queued >= target_samples {
            return Ok(true);
        }
        while *pending_index < pending.len() {
            match producer.push(pending[*pending_index]) {
                Ok(()) => *pending_index += 1,
                Err(rtrb::PushError::Full(_)) => return Ok(true),
            }
        }
        pending.clear();
        *pending_index = 0;
        if let Some(chunk) = mixer.next_chunk()? {
            *pending = chunk;
        } else {
            return Ok(false);
        }
    }
}

pub(crate) struct AudioRuntime {
    stream: cpal::Stream,
    producer: Producer<f32>,
    mixer: AudioMixer,
    pending: Vec<f32>,
    pending_index: usize,
    /// AU1 §5.3: `LIVE_FILL_MILLISECONDS` of audio at the device's rate.
    target_samples: usize,
    pub(crate) error_flag: Arc<AtomicBool>,
}

impl AudioRuntime {
    pub(crate) fn open(
        document: &Document,
        project_from: TimeCode,
        position_samples: &Arc<AtomicU64>,
        sample_rate_atomic: &Arc<AtomicU32>,
        meter: Arc<MeterState>,
        mix_meters: Arc<MixMeters>,
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
        let mut mixer =
            AudioMixer::open(document, project_from, sample_rate, channels, Some(meter))?;
        mixer.attach_mix_meters(mix_meters);
        let target_samples = usize::try_from(sample_rate)
            .unwrap_or(48_000)
            .saturating_mul(usize::from(channels))
            .saturating_mul(LIVE_FILL_MILLISECONDS)
            .saturating_div(1_000)
            .max(1);
        let mut runtime = Self {
            stream,
            producer,
            mixer,
            pending: Vec::new(),
            pending_index: 0,
            target_samples,
            error_flag,
        };
        runtime.fill()?;
        Ok(runtime)
    }

    /// AU1 §5.3: apply new track-stage targets without stopping the stream.
    pub(crate) fn update_track_mix(&mut self, document: &Document) {
        self.mixer.update_track_mix(document);
    }

    pub(crate) fn play(&self) -> Result<(), MediaError> {
        self.stream.play().map_err(backend)
    }

    pub(crate) fn pause(&self) -> Result<(), MediaError> {
        self.stream.pause().map_err(backend)
    }

    pub(crate) fn fill(&mut self) -> Result<(), MediaError> {
        fill_ring(
            &mut self.producer,
            &mut self.pending,
            &mut self.pending_index,
            &mut self.mixer,
            self.target_samples,
        )?;
        Ok(())
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

    use kinewright_core::{
        AssetId, AudioBus, AudioBusId, AudioMix, AutomationCurve, Clip, ClipId, Effect, EffectId,
        ExportSettings, Keyframe, KeyframeInterpolation, MediaAsset, MediaKind, MixLevelRequest,
        ParamValue, Track, TrackId, TrackKind, TrackMix, Transition,
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
            audio_mix: AudioMix {
                buses,
                tracks: Vec::new(),
            },
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
        let output = AudioMixProcessor::new(&document, 10, 1, None)
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
        let output = AudioMixProcessor::new(&document, 1_000, 1, None)
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
    fn limiter_enforces_its_declared_sample_ceiling() {
        let document = processor_document(
            Rational::new(1_000, 1).unwrap(),
            2,
            vec![AudioBus {
                id: AudioBusId(1),
                name: "Delivery".to_owned(),
                tracks: vec![TrackId(1)],
                effects: vec![audio_effect(
                    1,
                    "audio_limiter",
                    &[("ceiling_tenth_db", -10)],
                )],
                ducking_sidechain_tracks: Vec::new(),
            }],
        );
        let tracks = HashMap::from([(TrackId(1), vec![2.0, -2.0])]);
        let output = AudioMixProcessor::new(&document, 1_000, 1, None)
            .mix_chunk(&tracks, 0, 2)
            .unwrap();
        let ceiling = 10.0_f32.powf(-1.0 / 20.0);

        assert_close(output[0], ceiling);
        assert_close(output[1], -ceiling);
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
            delivery_color: kinewright_core::ColorContext::sdr_rec709().delivery,
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
        let document = video_only_document(fps);

        let mut mixer = AudioMixer::open(&document, TimeCode::ZERO, 48_000, 2, None).unwrap();
        let rendered = mixer.render_remaining().unwrap();

        assert_eq!(rendered.len(), 19_200);
        assert!(rendered.iter().all(|sample| *sample == 0.0));
    }

    fn video_only_document(fps: Rational) -> Document {
        Document {
            catalog: kinewright_core::MediaCatalog::default(),
            audio_mix: kinewright_core::AudioMix::default(),
            color_context: kinewright_core::ColorContext::default(),
            lut_assets: Vec::new(),
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(1),
                    asset: AssetId(1),
                    source_range: TimeCode(0)..TimeCode(2),
                    content: kinewright_core::ClipContent::Media,
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
                source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
                color_description: kinewright_core::ColorDescription::default(),
            }],
            markers: Vec::new(),
            fps,
            resolution: (64, 64),
            duration: TimeCode(2),
        }
    }

    // ---------------------------------------------------------------- AU1 §7

    fn track_mix(
        track: u64,
        gain_tenth_db: i32,
        pan_percent: i32,
        mute: bool,
        solo: bool,
    ) -> TrackMix {
        TrackMix {
            track: TrackId(track),
            gain_tenth_db,
            pan_percent,
            mute,
            solo,
        }
    }

    /// Two clipless audio tracks (1 and 2) carrying AU1 mix state.
    fn mix_document(tracks: Vec<TrackMix>, buses: Vec<AudioBus>) -> Document {
        let mut document = processor_document(Rational::new(1_000, 1).unwrap(), 2_000, buses);
        document.audio_mix.tracks = tracks;
        document
    }

    fn parity_settings(fps: Rational) -> ExportSettings {
        ExportSettings {
            fps,
            resolution: (64, 64),
            delivery_color: kinewright_core::ColorContext::sdr_rec709().delivery,
            video_codec: "libx264".to_owned(),
            audio_codec: "aac".to_owned(),
            video_bitrate: 1_000_000,
            audio_bitrate: 128_000,
            cancellation: ExportCancellation::default(),
        }
    }

    /// AU1 §7 item 8.
    #[test]
    fn the_track_stage_gain_uses_the_shared_tenth_db_expression() {
        for (gain_tenth_db, expected) in [(-60, 10.0_f32.powf(-0.3)), (120, 10.0_f32.powf(0.6))] {
            let document = mix_document(
                vec![track_mix(1, gain_tenth_db, 0, false, false)],
                Vec::new(),
            );
            let output = AudioMixProcessor::new(&document, 1_000, 2, None)
                .mix_chunk(&HashMap::from([(TrackId(1), vec![1.0; 8])]), 0, 4)
                .unwrap();

            assert_eq!(output.len(), 8);
            for sample in output {
                assert_close(sample, expected);
            }
        }
    }

    /// AU1 §7 item 9.
    #[test]
    fn the_track_stage_pan_follows_the_balance_law() {
        for (pan_percent, expected) in [
            (0, [1.0_f32, 1.0]),
            (-100, [1.0, 0.0]),
            (100, [0.0, 1.0]),
            (50, [0.5, 1.0]),
        ] {
            let document =
                mix_document(vec![track_mix(1, 0, pan_percent, false, false)], Vec::new());
            let output = AudioMixProcessor::new(&document, 1_000, 2, None)
                .mix_chunk(&HashMap::from([(TrackId(1), vec![1.0; 4])]), 0, 2)
                .unwrap();

            assert_close(output[0], expected[0]);
            assert_close(output[1], expected[1]);
            assert_close(output[2], expected[0]);
            assert_close(output[3], expected[1]);
        }
    }

    /// AU1 §7 item 9: a one-channel device hears no pan and channels above two
    /// carry the unpanned gain.
    #[test]
    fn pan_touches_only_the_first_two_channels() {
        let document = mix_document(vec![track_mix(1, 0, 100, false, false)], Vec::new());

        let mono = AudioMixProcessor::new(&document, 1_000, 1, None)
            .mix_chunk(&HashMap::from([(TrackId(1), vec![1.0; 2])]), 0, 2)
            .unwrap();
        assert_close(mono[0], 1.0);
        assert_close(mono[1], 1.0);

        let surround = AudioMixProcessor::new(&document, 1_000, 4, None)
            .mix_chunk(&HashMap::from([(TrackId(1), vec![1.0; 8])]), 0, 2)
            .unwrap();
        for frame in 0..2 {
            assert_close(surround[frame * 4], 0.0);
            assert_close(surround[frame * 4 + 1], 1.0);
            assert_close(surround[frame * 4 + 2], 1.0);
            assert_close(surround[frame * 4 + 3], 1.0);
        }
    }

    fn ducked_bus() -> AudioBus {
        AudioBus {
            id: AudioBusId(1),
            name: "Bed".to_owned(),
            tracks: vec![TrackId(1)],
            effects: vec![audio_effect(
                1,
                "audio_ducking",
                &[
                    ("threshold_tenth_db", -300),
                    ("reduction_tenth_db", 200),
                    ("attack_milliseconds", 1),
                    ("release_milliseconds", 1),
                ],
            )],
            ducking_sidechain_tracks: vec![TrackId(2)],
        }
    }

    /// AU1 §7 item 10.
    #[test]
    // The gate must contribute exact zeros, so exact equality is the contract.
    #[allow(clippy::float_cmp)]
    fn a_muted_track_contributes_exact_zeros_to_master_and_to_a_sidechain() {
        let document = mix_document(vec![track_mix(1, 0, 0, true, false)], Vec::new());
        let master = AudioMixProcessor::new(&document, 1_000, 1, None)
            .mix_chunk(&HashMap::from([(TrackId(1), vec![-1.0; 8])]), 0, 8)
            .unwrap();
        assert!(
            master.iter().all(|sample| *sample == 0.0),
            "a muted track must contribute exact zeros: {master:?}"
        );

        let buffers = HashMap::from([
            (TrackId(1), vec![1.0; 2_000]),
            (TrackId(2), vec![1.0; 2_000]),
        ]);
        let bus = ducked_bus();
        let monitor = AudioBus {
            id: AudioBusId(2),
            name: "Sidechain monitor".to_owned(),
            tracks: vec![TrackId(2)],
            effects: vec![audio_effect(2, "audio_gain", &[("gain_tenth_db", -600)])],
            ducking_sidechain_tracks: Vec::new(),
        };

        let ducking = mix_document(Vec::new(), vec![bus.clone(), monitor.clone()]);
        let ducked = AudioMixProcessor::new(&ducking, 1_000, 1, None)
            .mix_chunk(&buffers, 0, 2_000)
            .unwrap();
        assert!(
            ducked[1_999] < 0.2,
            "an audible sidechain should duck the bed: {}",
            ducked[1_999]
        );

        let muted = mix_document(vec![track_mix(2, 0, 0, true, false)], vec![bus, monitor]);
        let released = AudioMixProcessor::new(&muted, 1_000, 1, None)
            .mix_chunk(&buffers, 0, 2_000)
            .unwrap();
        assert_close(released[1_999], 1.0);
    }

    /// AU1 §7 item 11.
    #[test]
    // The gate must contribute exact zeros, so exact equality is the contract.
    #[allow(clippy::float_cmp)]
    fn solo_gates_over_the_document_track_set_not_the_chunk() {
        let soloed = mix_document(vec![track_mix(1, 0, 0, false, true)], Vec::new());
        // Track 1 is soloed but absent from the chunk; track 2 is the only key.
        let output = AudioMixProcessor::new(&soloed, 1_000, 1, None)
            .mix_chunk(&HashMap::from([(TrackId(2), vec![1.0; 8])]), 0, 8)
            .unwrap();
        assert!(
            output.iter().all(|sample| *sample == 0.0),
            "another track's solo must silence track 2: {output:?}"
        );

        let both = mix_document(
            vec![
                track_mix(1, 0, 0, false, true),
                track_mix(2, 0, 0, false, true),
            ],
            Vec::new(),
        );
        let passed = AudioMixProcessor::new(&both, 1_000, 1, None)
            .mix_chunk(
                &HashMap::from([(TrackId(1), vec![1.0; 8]), (TrackId(2), vec![1.0; 8])]),
                0,
                8,
            )
            .unwrap();
        for sample in passed {
            assert_close(sample, 2.0);
        }
    }

    /// AU1 §7 item 12.
    ///
    /// Three tracks with order-sensitive magnitudes: f32 addition is
    /// commutative, so two tracks can never distinguish one summation order
    /// from another, but `(1.0 + -1.0) + 1e-8` and `(1.0 + 1e-8) + -1.0` differ.
    #[test]
    // Deterministic summation means the bytes must match exactly.
    #[allow(clippy::float_cmp)]
    fn unrouted_tracks_sum_in_document_order_whatever_the_map_order() {
        let mut document = processor_document(Rational::new(1_000, 1).unwrap(), 8, Vec::new());
        document.tracks.push(Track {
            id: TrackId(3),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: Vec::new(),
        });
        let sources = [
            (TrackId(1), [1.0_f32; 4]),
            (TrackId(2), [-1.0_f32; 4]),
            (TrackId(3), [1.0e-8_f32; 4]),
        ];
        // The fixture is only worth anything if the magnitudes really are
        // order-sensitive in f32.
        assert_ne!(
            (1.0_f32 + 1.0e-8) + -1.0,
            (1.0_f32 + -1.0) + 1.0e-8,
            "the fixture must distinguish summation orders"
        );

        // The document-order left fold, computed the way the master sum does it.
        let mut expected = vec![0.0_f32; 4];
        for (_, samples) in sources {
            for (slot, sample) in expected.iter_mut().zip(&samples) {
                *slot += *sample;
            }
        }
        assert_eq!(expected, vec![1.0e-8_f32; 4]);

        for order in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let mut buffers = HashMap::new();
            for index in order {
                let (track, samples) = sources[index];
                buffers.insert(track, samples.to_vec());
            }
            let mixed = AudioMixProcessor::new(&document, 1_000, 1, None)
                .mix_chunk(&buffers, 0, 4)
                .unwrap();
            assert_eq!(
                mixed, expected,
                "map insertion order {order:?} changed the sum"
            );
        }

        let mut buffers = HashMap::new();
        for (track, samples) in sources {
            buffers.insert(track, samples.to_vec());
        }
        let mut neutral = document.clone();
        neutral.audio_mix.tracks = vec![TrackMix::neutral(TrackId(1))];
        let with_neutral_entry = AudioMixProcessor::new(&neutral, 1_000, 1, None)
            .mix_chunk(&buffers, 0, 4)
            .unwrap();
        assert_eq!(with_neutral_entry, expected);
    }

    /// AU1 §7 item 13.
    #[test]
    // The settled ends of the ramp are exact by construction.
    #[allow(clippy::float_cmp)]
    fn a_live_track_mix_change_ramps_over_five_milliseconds() {
        let document = processor_document(Rational::new(10, 1).unwrap(), 10, Vec::new());
        let mut muted = document.clone();
        muted.audio_mix.tracks = vec![track_mix(1, 0, 0, true, false)];
        let buffers = HashMap::from([(TrackId(1), vec![1.0; 512])]);

        let mut processor = AudioMixProcessor::new(&document, 48_000, 1, None);
        processor.update_track_mix(&muted);
        let ramped = processor.mix_chunk(&buffers, 0, 512).unwrap();

        for index in 1..240 {
            assert!(
                ramped[index] < ramped[index - 1],
                "ramp sample {index} ({}) did not fall below {}",
                ramped[index],
                ramped[index - 1]
            );
        }
        assert_close(ramped[119], 0.5);
        assert!(
            ramped[239..].iter().all(|sample| *sample == 0.0),
            "the ramp must settle on exact zeros"
        );

        let settled = AudioMixProcessor::new(&muted, 48_000, 1, None)
            .mix_chunk(&buffers, 0, 512)
            .unwrap();
        assert!(
            settled.iter().all(|sample| *sample == 0.0),
            "a freshly opened mixer must never ramp"
        );
    }

    /// AU1 §3.4: the ramp is defined over output sample frames, not over frames
    /// in which this track has media, so a mute applied during a gap does not
    /// blip through the first 5 ms of the next clip.
    #[test]
    // The settled ends of the ramp are exact by construction.
    #[allow(clippy::float_cmp)]
    fn a_ramp_advances_while_the_track_is_absent_from_the_chunk() {
        let document = processor_document(Rational::new(10, 1).unwrap(), 10, Vec::new());
        let mut muted = document.clone();
        muted.audio_mix.tracks = vec![track_mix(1, 0, 0, true, false)];
        let buffers = HashMap::from([(TrackId(1), vec![1.0_f32; 512])]);

        // 512 absent frames is more than the 240-frame ramp: it settles in the gap.
        let mut processor = AudioMixProcessor::new(&document, 48_000, 1, None);
        processor.update_track_mix(&muted);
        let gap = processor.mix_chunk(&HashMap::new(), 0, 512).unwrap();
        assert!(
            gap.iter().all(|sample| *sample == 0.0),
            "a chunk with no media is silent"
        );
        let resumed = processor.mix_chunk(&buffers, 512, 512).unwrap();
        assert!(
            resumed[0] == 0.0,
            "the first frame after the gap must not blip: {}",
            resumed[0]
        );
        assert!(
            resumed.iter().all(|sample| *sample == 0.0),
            "the ramp settled during the gap, so the whole clip is muted"
        );

        // A gap shorter than the ramp advances it partway, continuously with
        // `advance_ramp`: frame 100 of the gap plus frame 0 of the clip is 101.
        let mut partial = AudioMixProcessor::new(&document, 48_000, 1, None);
        partial.update_track_mix(&muted);
        partial.mix_chunk(&HashMap::new(), 0, 100).unwrap();
        let after_gap = partial.mix_chunk(&buffers, 100, 512).unwrap();
        assert_close(after_gap[0], 1.0 - 101.0 / 240.0);
        assert!(
            after_gap[139..].iter().all(|sample| *sample == 0.0),
            "the ramp must still settle exactly 240 frames after the retarget"
        );
    }

    /// AU1 §3.4: a retarget mid-ramp restarts the ramp from the current
    /// interpolated value, so an unmute during a fade-out neither jumps nor
    /// takes the settled fast paths.
    #[test]
    // The settled end of the ramp is exact by construction.
    #[allow(clippy::float_cmp)]
    fn a_retarget_mid_ramp_restarts_from_the_current_value() {
        let document = processor_document(Rational::new(10, 1).unwrap(), 10, Vec::new());
        let mut muted = document.clone();
        muted.audio_mix.tracks = vec![track_mix(1, 0, 0, true, false)];

        let mut processor = AudioMixProcessor::new(&document, 48_000, 1, None);
        processor.update_track_mix(&muted);
        // 100 of the 240 ramp frames elapse before the editor changes its mind.
        let falling = processor
            .mix_chunk(&HashMap::from([(TrackId(1), vec![1.0_f32; 100])]), 0, 100)
            .unwrap();
        let start = 1.0 - 100.0 / 240.0;
        assert_close(falling[99], start);

        processor.update_track_mix(&document);
        let rising = processor
            .mix_chunk(&HashMap::from([(TrackId(1), vec![1.0_f32; 512])]), 100, 512)
            .unwrap();
        // Frame 0 continues from the interpolated value, it does not jump to
        // unity and it does not restart from zero.
        assert_close(rising[0], start + (1.0 - start) * (1.0 / 240.0));
        assert!(
            rising[0] > start && rising[0] < 1.0,
            "the retargeted ramp must rise from {start}: {}",
            rising[0]
        );
        for index in 1..240 {
            assert!(
                rising[index] > rising[index - 1],
                "ramp sample {index} ({}) did not rise above {}",
                rising[index],
                rising[index - 1]
            );
        }
        // Exactly 240 frames after the retarget the stage stores the target.
        assert!(
            rising[239..].iter().all(|sample| *sample == 1.0),
            "the ramp must settle on exact unity 240 frames after the retarget"
        );
        // `audible` flipped back, so the silent fast path is not taken.
        assert!(
            rising.iter().all(|sample| *sample != 0.0),
            "an unmuted track must not take the silent fast path"
        );
    }

    /// AU1 §7 item 15.
    #[test]
    fn mix_meters_record_track_bus_and_master_peaks_in_document_order() {
        let bus = AudioBus {
            id: AudioBusId(1),
            name: "Bed".to_owned(),
            tracks: vec![TrackId(2)],
            effects: vec![audio_effect(1, "audio_gain", &[("gain_tenth_db", -60)])],
            ducking_sidechain_tracks: Vec::new(),
        };
        let document = mix_document(vec![track_mix(1, -60, 0, false, false)], vec![bus]);
        let master = Arc::new(MeterState::default());
        let meters = Arc::new(MixMeters::for_document(&document, Arc::clone(&master)));
        let mut mixed = AudioMixProcessor::new(&document, 1_000, 2, Some(Arc::clone(&meters)))
            .mix_chunk(
                &HashMap::from([(TrackId(1), vec![0.5; 8]), (TrackId(2), vec![1.0; 8])]),
                0,
                4,
            )
            .unwrap();
        limit_and_meter_audio_mix(&mut mixed, 2, &master);

        let peaks = meters.peaks();
        let gain = 10.0_f32.powf(-0.3);
        assert_eq!(
            peaks
                .tracks
                .iter()
                .map(|(track, _)| *track)
                .collect::<Vec<_>>(),
            vec![TrackId(1), TrackId(2)]
        );
        assert_eq!(
            peaks.buses.iter().map(|(bus, _)| *bus).collect::<Vec<_>>(),
            vec![AudioBusId(1)]
        );
        assert_close(peaks.tracks[0].1[0], 0.5 * gain);
        assert_close(peaks.tracks[0].1[1], 0.5 * gain);
        assert_close(peaks.tracks[1].1[0], 1.0);
        assert_close(peaks.buses[0].1[0], gain);
        assert_close(peaks.master[0], 0.5 * gain + gain);

        let empty = MixMeters::empty(master).peaks();
        assert!(empty.tracks.is_empty() && empty.buses.is_empty());
    }

    /// AU1 §4.1 overwrite semantics: a track whose last clip has ended drops out
    /// of `track_buffers`, and its meter must fall rather than freeze at the
    /// last peak it recorded.
    #[test]
    // Meter slots are overwritten, so the zero is exact.
    #[allow(clippy::float_cmp)]
    fn an_absent_track_records_a_zero_meter_for_that_chunk() {
        let document = mix_document(Vec::new(), Vec::new());
        let master = Arc::new(MeterState::default());
        let meters = Arc::new(MixMeters::for_document(&document, Arc::clone(&master)));
        let mut processor = AudioMixProcessor::new(&document, 1_000, 2, Some(Arc::clone(&meters)));

        processor
            .mix_chunk(
                &HashMap::from([(TrackId(1), vec![0.5; 8]), (TrackId(2), vec![1.0; 8])]),
                0,
                4,
            )
            .unwrap();
        let peaks = meters.peaks();
        assert_eq!(peaks.tracks[0].1, [0.5, 0.5]);
        assert_eq!(peaks.tracks[1].1, [1.0, 1.0]);

        // Track 1's clip has ended; track 2 keeps playing.
        processor
            .mix_chunk(&HashMap::from([(TrackId(2), vec![1.0; 8])]), 4, 4)
            .unwrap();
        let peaks = meters.peaks();
        assert_eq!(peaks.tracks[0].1, [0.0, 0.0]);
        assert_eq!(peaks.tracks[1].1, [1.0, 1.0]);
    }

    /// AU1 §7 item 17.
    #[test]
    fn fill_ring_stops_at_the_live_target_and_tops_up_after_a_drain() {
        let fps = Rational::new(10, 1).unwrap();
        let mut document = video_only_document(fps);
        // Two seconds of silence at 48 kHz stereo: 192 000 samples in total.
        document.duration = TimeCode(20);
        let mut mixer = AudioMixer::open(&document, TimeCode::ZERO, 48_000, 2, None).unwrap();
        let (mut producer, mut consumer) = RingBuffer::<f32>::new(48_000 * 2 * 2);
        let capacity = producer.buffer().capacity();
        let target = 48_000 * 2;
        let chunk_samples = MIX_CHUNK_SAMPLE_FRAMES * 2;
        let mut pending = Vec::new();
        let mut pending_index = 0;

        assert!(
            fill_ring(
                &mut producer,
                &mut pending,
                &mut pending_index,
                &mut mixer,
                target
            )
            .unwrap()
        );
        let queued = capacity - producer.slots();
        assert!(
            (target..target + chunk_samples).contains(&queued),
            "first fill queued {queued}"
        );

        let mut delivered = 0_usize;
        for _ in 0..48_000 {
            consumer.pop().unwrap();
            delivered += 1;
        }
        assert!(
            fill_ring(
                &mut producer,
                &mut pending,
                &mut pending_index,
                &mut mixer,
                target
            )
            .unwrap()
        );
        let queued = capacity - producer.slots();
        assert!(
            (target..target + chunk_samples).contains(&queued),
            "top-up queued {queued}"
        );

        loop {
            while consumer.pop().is_ok() {
                delivered += 1;
            }
            if !fill_ring(
                &mut producer,
                &mut pending,
                &mut pending_index,
                &mut mixer,
                target,
            )
            .unwrap()
            {
                break;
            }
        }
        while consumer.pop().is_ok() {
            delivered += 1;
        }

        assert_eq!(delivered, 2 * 48_000 * 2);
    }

    /// AU1 §7 item 18.
    #[test]
    fn a_track_mix_only_document_needs_no_seek_preroll() {
        let fps = Rational::new(10, 1).unwrap();
        let mut document = processor_document(fps, 30, Vec::new());
        document.audio_mix.tracks = vec![track_mix(1, -60, 25, false, false)];

        assert!(!needs_seek_preroll(&document, TimeCode(5)));
        let mixer = AudioMixer::open(&document, TimeCode(5), 48_000, 2, None).unwrap();
        assert_eq!(
            mixer.cursor_sample,
            frame_to_samples(TimeCode(5), 48_000, fps)
        );

        let mut with_bus = document.clone();
        with_bus.audio_mix.buses = vec![AudioBus {
            id: AudioBusId(1),
            name: "Bed".to_owned(),
            tracks: vec![TrackId(1)],
            effects: vec![audio_effect(1, "audio_gain", &[("gain_tenth_db", -60)])],
            ducking_sidechain_tracks: Vec::new(),
        }];
        assert!(needs_seek_preroll(&with_bus, TimeCode(5)));
        assert!(!needs_seek_preroll(&with_bus, TimeCode::ZERO));
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
            catalog: kinewright_core::MediaCatalog::default(),
            audio_mix: AudioMix {
                tracks: Vec::new(),
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
            color_context: kinewright_core::ColorContext::default(),
            lut_assets: Vec::new(),
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

    /// AU1 §7 item 14: the same fixture as `parity_document` with a non-neutral
    /// track stage on every track.
    fn parity_document_with_track_mix(voice: &Path, bed: &Path, fps: Rational) -> Document {
        let mut document = parity_document(voice, bed, fps);
        document.audio_mix.tracks = vec![
            track_mix(1, -60, -30, false, false),
            track_mix(2, 0, 25, false, false),
            track_mix(3, 30, 0, false, false),
        ];
        document
    }

    fn transitioned(mut document: Document) -> Document {
        document.tracks[0].clips[1].transition_in = Some(Transition {
            name: "fade_from_black".to_owned(),
            duration: TimeCode(2),
        });
        document.validate().unwrap();
        document
    }

    /// The nine-window 1e-6 comparison plus the frame-5 seek comparison.
    fn assert_playback_matches_export(document: &Document, exported: &[f32], fps: Rational) {
        let mut playback = AudioMixer::open(document, TimeCode::ZERO, 48_000, 2, None).unwrap();
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

        let seek_sample = usize::try_from(frame_to_samples(TimeCode(5), 48_000, fps))
            .unwrap()
            .saturating_mul(2);
        let mut seeked_playback = AudioMixer::open(document, TimeCode(5), 48_000, 2, None).unwrap();
        let seeked = seeked_playback.render_remaining().unwrap();
        assert_eq!(seeked.len(), exported.len() - seek_sample);
        let seek_difference = exported[seek_sample..]
            .iter()
            .zip(&seeked)
            .map(|(exported, played)| (exported - played).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            seek_difference <= 1.0e-6,
            "coherent feeder seek differs by {seek_difference}"
        );
    }

    /// AU1 §7 item 14.
    #[test]
    fn playback_feeder_mix_matches_export_through_the_track_stage() {
        crate::initialize_ffmpeg().unwrap();
        let voice = loud_sine("au1-voice", 440);
        let bed = loud_sine("au1-bed", 660);
        let fps = Rational::new(10, 1).unwrap();
        let settings = parity_settings(fps);

        let neutral = transitioned(parity_document(voice.path(), bed.path(), fps));
        let exported_neutral = crate::export::mix_audio(&neutral, &settings).unwrap();

        let document = transitioned(parity_document_with_track_mix(
            voice.path(),
            bed.path(),
            fps,
        ));
        let exported = crate::export::mix_audio(&document, &settings).unwrap();
        assert_playback_matches_export(&document, &exported, fps);

        // Track 3's +3.0 dB stage lifts the only clip in 22..28 by 10^0.15.
        let expected = peak_in(&exported_neutral, 22..28, fps) * 10.0_f32.powf(0.15);
        let gained = peak_in(&exported, 22..28, fps);
        assert!(
            (gained - expected).abs() <= expected * 0.02,
            "+3.0 dB steady peak {gained} did not approximate {expected}"
        );

        let mut gated = document.clone();
        gated.audio_mix.tracks[0].solo = true;
        gated.audio_mix.tracks[2].mute = true;
        gated.validate().unwrap();
        let exported_gated = crate::export::mix_audio(&gated, &settings).unwrap();
        assert_playback_matches_export(&gated, &exported_gated, fps);
        assert!(
            peak_in(&exported_gated, 22..28, fps) == 0.0,
            "a muted track 3 must leave 22..28 silent"
        );
    }

    fn level_sine(label: &str, frequency: u16, volume: &str) -> GeneratedMedia {
        let source = format!("sine=frequency={frequency}:sample_rate=48000:duration=2");
        let filter = format!("volume={volume}");
        GeneratedMedia::ffmpeg(
            label,
            &[
                "-f",
                "lavfi",
                "-i",
                &source,
                "-filter:a",
                &filter,
                "-c:a",
                "pcm_f32le",
            ],
            "wav",
        )
    }

    /// Two audio tracks: a continuous sine on track 1 and a late one that only
    /// starts at frame 15 on track 2.
    fn levels_document(steady: &Path, late: &Path, fps: Rational) -> Document {
        Document {
            catalog: kinewright_core::MediaCatalog::default(),
            audio_mix: AudioMix::default(),
            color_context: kinewright_core::ColorContext::default(),
            lut_assets: Vec::new(),
            tracks: vec![
                Track {
                    id: TrackId(1),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: vec![audio_clip(1, 1, 0..20, 0)],
                },
                Track {
                    id: TrackId(2),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: vec![audio_clip(2, 2, 0..5, 15)],
                },
            ],
            media_pool: vec![
                audio_asset(1, steady, "steady", fps),
                audio_asset(2, late, "late", fps),
            ],
            markers: Vec::new(),
            fps,
            resolution: (64, 64),
            duration: TimeCode(20),
        }
    }

    /// AU1 §7 item 16.
    #[test]
    fn mix_levels_measures_stems_gain_mutes_and_ranges() {
        crate::initialize_ffmpeg().unwrap();
        let steady = level_sine("au1-levels-steady", 440, "0.4");
        let late = level_sine("au1-levels-late", 660, "0.2");
        let fps = Rational::new(10, 1).unwrap();
        let document = levels_document(steady.path(), late.path(), fps);
        document.validate().unwrap();
        let whole = MixLevelRequest { range: None };

        let neutral = crate::export::measure_mix_levels(&document, &whole).unwrap();
        assert_eq!(neutral.range, TimeCode::ZERO..TimeCode(20));
        assert!(!neutral.any_solo);
        assert_eq!(neutral.tracks.len(), 2);
        assert!(neutral.buses.is_empty());
        assert!(neutral.tracks.iter().all(|track| track.audible));

        // The master is the export mix, measured exactly as `timeline_loudness` does.
        let exported = crate::export::mix_audio(&document, &parity_settings(fps)).unwrap();
        assert_eq!(
            neutral.master,
            crate::loudness::measure_loudness(&exported, 48_000, 2).unwrap()
        );

        // A -6.0 dB track stage moves that track's stem by 600 LU-hundredths.
        let mut gained = document.clone();
        gained.audio_mix.tracks = vec![track_mix(1, -60, 0, false, false)];
        gained.validate().unwrap();
        let gained = crate::export::measure_mix_levels(&gained, &whole).unwrap();
        let before = neutral.tracks[0].levels.integrated_lufs_hundredths.unwrap();
        let after = gained.tracks[0].levels.integrated_lufs_hundredths.unwrap();
        assert!(
            (after - (before - 600)).abs() <= 5,
            "a -6.0 dB stage moved track 1 from {before} to {after}"
        );

        // A muted track reads silent.
        let mut muted = document.clone();
        muted.audio_mix.tracks = vec![track_mix(1, 0, 0, true, false)];
        muted.validate().unwrap();
        let muted = crate::export::measure_mix_levels(&muted, &whole).unwrap();
        assert!(!muted.tracks[0].audible);
        assert_eq!(muted.tracks[0].levels.integrated_lufs_hundredths, None);
        assert_eq!(muted.tracks[0].levels.sample_peak_dbfs_hundredths, None);
        assert!(muted.tracks[1].levels.integrated_lufs_hundredths.is_some());

        // A range measures only its own window.
        let window = crate::export::measure_mix_levels(
            &document,
            &MixLevelRequest {
                range: Some(TimeCode(10)..TimeCode(20)),
            },
        )
        .unwrap();
        assert_eq!(window.range, TimeCode(10)..TimeCode(20));
        assert_eq!(window.master.sample_frames, 48_000);
        for track in &window.tracks {
            assert_eq!(track.levels.sample_frames, 48_000);
        }
        let early = crate::export::measure_mix_levels(
            &document,
            &MixLevelRequest {
                range: Some(TimeCode::ZERO..TimeCode(10)),
            },
        )
        .unwrap();
        // The late sine starts at frame 15, so it is absent before frame 10 and
        // present in 10..20.
        assert_eq!(early.tracks[1].levels.integrated_lufs_hundredths, None);
        assert!(window.tracks[1].levels.integrated_lufs_hundredths.is_some());
        assert_eq!(
            early.tracks[0].levels.integrated_lufs_hundredths,
            window.tracks[0].levels.integrated_lufs_hundredths
        );

        // An empty clamped range is an error.
        assert!(
            crate::export::measure_mix_levels(
                &document,
                &MixLevelRequest {
                    range: Some(TimeCode(20)..TimeCode(30)),
                },
            )
            .is_err()
        );
    }

    fn audio_clip(id: u64, asset: u64, source: std::ops::Range<i64>, timeline_start: i64) -> Clip {
        Clip {
            id: ClipId(id),
            asset: AssetId(asset),
            source_range: TimeCode(source.start)..TimeCode(source.end),
            content: kinewright_core::ClipContent::Media,
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
            source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
            color_description: kinewright_core::ColorDescription::default(),
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
