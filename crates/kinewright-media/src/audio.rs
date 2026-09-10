use std::{
    cell::Cell,
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ffmpeg_next as ffmpeg;
use kinewright_core::{
    AudioBus, AudioBusId, AudioChain, AudioMaster, AutomationCurve, ChainLookahead, Clip, ClipId,
    Document, Effect, EffectId, ExportCancellation, MediaError, MixPeaks, PanLaw, Rational,
    TRACK_MIX_PAN_MAX, TRACK_MIX_PAN_MIN, TimeCode, TrackId,
};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::{
    clock::{frame_to_samples, samples_to_frame},
    decode::{backend, ensure_decoder, media_error, media_input, stream_timestamp_to_global},
    dsp::{
        BiquadCoefficients, BiquadSection, Boxcar, DelayLine, MeanSquareWindow, SlidingMinimum,
        TRUE_PEAK_GROUP_DELAY_FRAMES, TruePeakEstimator,
    },
    loudness::LiveLoudnessMeter,
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

/// One lock-free gain-reduction slot (AU2 §3.8). Telemetry, not state.
#[derive(Debug, Default)]
pub(crate) struct GainReductionState(AtomicU32);

impl GainReductionState {
    fn decibels(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Acquire))
    }

    fn record(&self, decibels: f32) {
        self.0.store(decibels.to_bits(), Ordering::Release);
    }

    fn clear(&self) {
        self.0.store(0.0_f32.to_bits(), Ordering::Release);
    }
}

/// AU2 §3.8: the nodes that have a gain computer and therefore a slot.
fn has_gain_computer(name: &str) -> bool {
    matches!(
        name,
        "audio_compressor" | "audio_ducking" | "audio_gate" | "audio_true_peak_limiter"
    )
}

/// AU2 §3.8/§5.6: every gain-reduction slot key one document asks for, each
/// bus chain in document order and then the master chain.
fn gain_reduction_keys(document: &Document) -> impl Iterator<Item = (AudioChain, EffectId)> + '_ {
    document
        .audio_mix
        .buses
        .iter()
        .flat_map(|bus| {
            bus.effects
                .iter()
                .filter(|effect| has_gain_computer(&effect.name))
                .map(move |effect| (AudioChain::Bus(bus.id), effect.id))
        })
        .chain(
            document
                .audio_mix
                .master
                .effects
                .iter()
                .filter(|effect| has_gain_computer(&effect.name))
                .map(|effect| (AudioChain::Master, effect.id)),
        )
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
    /// AU2 §3.8: one slot per node with a gain computer, in chain order.
    gain_reduction: Vec<(AudioChain, EffectId, GainReductionState)>,
    master: Arc<MeterState>,
}

impl MixMeters {
    /// One slot per `document.tracks` entry in order, one per bus in order,
    /// plus the shared master slot (AU1 §4.1) and AU2 §3.8's per-node
    /// gain-reduction slots.
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
            gain_reduction: gain_reduction_keys(document)
                .map(|(chain, effect)| (chain, effect, GainReductionState::default()))
                .collect(),
            master,
        }
    }

    /// AU2 §5.8: whether the installed table's key set still describes this
    /// document, so a live update can skip the rebuild (A33).
    ///
    /// `MixMeters::for_document` allocates fresh zeroed slots, and the mixer UI
    /// only raises its meters while playing, so rebuilding on every update
    /// would zero every meter on every frame of a fader drag. A parameter-only
    /// retarget leaves the installed table and its accumulated peaks alone.
    pub(crate) fn matches_document(&self, document: &Document) -> bool {
        self.tracks.len() == document.tracks.len()
            && self
                .tracks
                .iter()
                .zip(&document.tracks)
                .all(|((installed, _), track)| *installed == track.id)
            && self.buses.len() == document.audio_mix.buses.len()
            && self
                .buses
                .iter()
                .zip(&document.audio_mix.buses)
                .all(|((installed, _), bus)| *installed == bus.id)
            && self.gain_reduction.len() == gain_reduction_keys(document).count()
            && self
                .gain_reduction
                .iter()
                .zip(gain_reduction_keys(document))
                .all(|((chain, effect, _), key)| (*chain, *effect) == key)
    }

    /// The table installed whenever the worker is not playing (AU1 §4.1).
    pub(crate) fn empty(master: Arc<MeterState>) -> Self {
        Self {
            tracks: Vec::new(),
            buses: Vec::new(),
            gain_reduction: Vec::new(),
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
            gain_reduction: self
                .gain_reduction
                .iter()
                .map(|(chain, effect, state)| (*chain, *effect, state.decibels()))
                .collect(),
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
        for (_, _, state) in &self.gain_reduction {
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

    /// AU2 §3.8: overwrite one node's slot with this chunk's reduction.
    fn record_gain_reduction(&self, chain: AudioChain, effect: EffectId, decibels: f32) {
        if let Some((_, _, state)) = self
            .gain_reduction
            .iter()
            .find(|(slot, id, _)| *slot == chain && *id == effect)
        {
            state.record(decibels);
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

/// AU4 §3.2: the clip's own gain envelope and its three multiplicative ramps.
///
/// Loses `Copy` (rule 50): the per-clip `Vec<Keyframe>` clone is once per clip
/// per export, not per sample.
#[derive(Debug, Clone)]
pub(crate) struct ClipAudioShaping {
    constant_gain: f32,
    fade_in: Option<AudioGainRamp>,
    fade_out: Option<AudioGainRamp>,
    transition: Option<AudioGainRamp>,
    /// AU4 §3.2: the envelope, keyed **clip-local**, so `origin` is the clip's
    /// `timeline_start`.
    curve: Option<AutomationCurve>,
    origin: TimeCode,
    /// The clip's own `audio_gain_tenth_db`, `automation_step`'s static
    /// fallback. When a curve exists it **replaces** the scalar, so
    /// `constant_gain` is 1.0 in that case (rule 52).
    gain_tenth_db: i64,
    /// AU4 §3.1 rule 46: bumped **unconditionally** by a rebuild.
    curve_epoch: u32,
    /// `gain_at` takes `&self` (rule 50), so the per-frame anchor memo lives
    /// behind a [`Cell`]. Nothing here crosses a thread: the mixer is driven by
    /// `fill_ring` from the worker and the cpal callback only drains the ring
    /// (rule 76).
    memo: Cell<AnchorMemo<f32>>,
    sample_rate: u32,
    project_fps: Rational,
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
        // AU4 §3.2 rule 52: a curve **replaces** the scalar rather than
        // multiplying with it, so the constant is exactly 1.0 whenever one
        // exists — the envelope is never gained twice (A13).
        #[allow(clippy::cast_precision_loss)]
        let constant_gain = if clip.audio_gain_curve.is_some() {
            1.0
        } else {
            10.0_f32.powf(clip.audio_gain_tenth_db as f32 / 200.0)
        };
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
            curve: clip.audio_gain_curve.clone(),
            origin: clip.timeline_start,
            gain_tenth_db: i64::from(clip.audio_gain_tenth_db),
            curve_epoch: 0,
            memo: Cell::new(AnchorMemo::new()),
            sample_rate,
            project_fps,
        }
    }

    /// AU4 §3.2 rule 51: **one** new factor, evaluated first so the three
    /// existing ramps multiply into it unchanged. With no curve the factor is
    /// skipped entirely and the returned float is bit-identical to pre-AU4.
    ///
    /// Rule 53: one implementation, two call sites — export `mix_pass` and live
    /// `AudioMixSource::add_samples` — and both already pass the same absolute
    /// project sample index, so identity is by construction, not by tolerance.
    pub(crate) fn gain_at(&self, project_sample: u64) -> f32 {
        let gain = self.envelope_at(project_sample);
        let gain = gain * self.constant_gain;
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

    fn envelope_at(&self, project_sample: u64) -> f32 {
        let Some(curve) = self.curve.as_ref() else {
            return 1.0;
        };
        let step = automation_step(
            Some(curve),
            self.gain_tenth_db,
            self.origin,
            project_sample,
            self.sample_rate,
            self.project_fps,
        );
        let frame = automation_frame(
            self.origin,
            project_sample,
            self.sample_rate,
            self.project_fps,
        );
        let mut memo = self.memo.get();
        let anchors = memo.anchors(frame, self.curve_epoch, || {
            (db_gain(step.v0), db_gain(step.v1))
        });
        self.memo.set(memo);
        automated_gain(&step, anchors)
    }

    /// AU4 §3.8 rule 73: rebuild one source's shaping in place from a new
    /// document, bumping the epoch so the memo cannot hold pre-edit anchors for
    /// the rest of the project frame (rule 46).
    fn rebuild(&mut self, clip: &Clip, clip_duration: TimeCode) {
        let epoch = self.curve_epoch.wrapping_add(1);
        *self = Self::new(clip, clip_duration, self.sample_rate, self.project_fps);
        self.curve_epoch = epoch;
    }
}

/// AU2 §3.6: one stage's latency in sample frames at one rate.
///
/// Truncating, and truncating is load-bearing: §3.6's alignment pad
/// `stage_latency_frames(L_bus) - Σ stage_latency_frames(node)` is non-negative
/// only because the floor function is superadditive and because every node's
/// own delay uses this same conversion.
pub(crate) fn stage_latency_frames(milliseconds: i64, sample_rate: u32) -> usize {
    let milliseconds = u64::try_from(milliseconds.max(0)).unwrap_or(0);
    usize::try_from(u64::from(sample_rate).saturating_mul(milliseconds) / 1_000)
        .unwrap_or(usize::MAX)
}

/// AU2 §3.6: the processor's total input-to-output latency in sample frames.
///
/// The **sum of the two truncations**, never a truncation of the sum: at
/// 44 100 Hz with a 5 ms bus stage and a 5 ms master stage the true latency is
/// `220 + 220 = 440`, while `stage_latency_frames(10, 44_100)` is 441. Every
/// compensation site uses this function and only this function.
pub(crate) fn graph_latency_frames(lookahead: &ChainLookahead, sample_rate: u32) -> usize {
    stage_latency_frames(lookahead.bus_stage, sample_rate)
        .saturating_add(stage_latency_frames(lookahead.master_stage, sample_rate))
}

/// AU2 §2.2: `lookahead_milliseconds`, spelled here because the runtime reads
/// it once at construction rather than through the automation curve.
const AUDIO_LOOKAHEAD_PARAMETER: &str = "lookahead_milliseconds";

/// AU2 §2.1: the shared bypass control of every audio node.
const AUDIO_BYPASS_PARAMETER: &str = "bypass";

/// AU2 §3.2: high-pass, low shelf, four peaking bands, high shelf.
const PARAMETRIC_EQ_SECTIONS: usize = 7;

/// AU2 §3.5/A27: how close to unity the limiter's release has to land before
/// it snaps to exactly `1.0`.
const RELEASE_IDENTITY_EPSILON: f32 = 1.0 / 1_048_576.0;

/// AU2 §3.2: the `audio_parametric_eq` controls at one project frame.
///
/// Read once per project frame rather than once per sample frame: `audio_value`
/// already returns a project-frame staircase, so the output is identical and
/// eighteen `BTreeMap` lookups per sample frame become eighteen per project
/// frame (pinned by A19).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ParametricEqSettings {
    high_pass_hertz: i64,
    low_shelf_hertz: i64,
    low_shelf_gain_tenth_db: i64,
    bands: [(i64, i64, i64); 4],
    high_shelf_hertz: i64,
    high_shelf_gain_tenth_db: i64,
    output_gain_tenth_db: i64,
}

impl ParametricEqSettings {
    fn read(effect: &Effect, at: TimeCode) -> Self {
        Self {
            high_pass_hertz: audio_value(effect, "high_pass_hertz", at, 0),
            low_shelf_hertz: audio_value(effect, "low_shelf_hertz", at, 100),
            low_shelf_gain_tenth_db: audio_value(effect, "low_shelf_gain_tenth_db", at, 0),
            bands: [
                (
                    audio_value(effect, "band1_hertz", at, 120),
                    audio_value(effect, "band1_gain_tenth_db", at, 0),
                    audio_value(effect, "band1_q_hundredths", at, 71),
                ),
                (
                    audio_value(effect, "band2_hertz", at, 500),
                    audio_value(effect, "band2_gain_tenth_db", at, 0),
                    audio_value(effect, "band2_q_hundredths", at, 71),
                ),
                (
                    audio_value(effect, "band3_hertz", at, 2_000),
                    audio_value(effect, "band3_gain_tenth_db", at, 0),
                    audio_value(effect, "band3_q_hundredths", at, 71),
                ),
                (
                    audio_value(effect, "band4_hertz", at, 8_000),
                    audio_value(effect, "band4_gain_tenth_db", at, 0),
                    audio_value(effect, "band4_q_hundredths", at, 71),
                ),
            ],
            high_shelf_hertz: audio_value(effect, "high_shelf_hertz", at, 8_000),
            high_shelf_gain_tenth_db: audio_value(effect, "high_shelf_gain_tenth_db", at, 0),
            output_gain_tenth_db: audio_value(effect, "output_gain_tenth_db", at, 0),
        }
    }

    /// The cascade in stage order: high-pass, low shelf, bands 1..4, high shelf.
    // Descriptor bounds keep every control exactly representable at audio
    // precision, so the integer-to-float conversions here cannot lose one.
    #[allow(clippy::cast_precision_loss)]
    fn coefficients(self, sample_rate: u32) -> [BiquadCoefficients; PARAMETRIC_EQ_SECTIONS] {
        let mut sections = [BiquadCoefficients::IDENTITY; PARAMETRIC_EQ_SECTIONS];
        if self.high_pass_hertz > 0 {
            sections[0] = BiquadCoefficients::high_pass(self.high_pass_hertz as f64, sample_rate);
        }
        sections[1] = BiquadCoefficients::low_shelf(
            self.low_shelf_hertz as f64,
            self.low_shelf_gain_tenth_db as f64 / 10.0,
            sample_rate,
        );
        for (index, (hertz, gain_tenth_db, q_hundredths)) in self.bands.into_iter().enumerate() {
            sections[2 + index] = BiquadCoefficients::peaking(
                hertz as f64,
                gain_tenth_db as f64 / 10.0,
                q_hundredths as f64 / 100.0,
                sample_rate,
            );
        }
        sections[6] = BiquadCoefficients::high_shelf(
            self.high_shelf_hertz as f64,
            self.high_shelf_gain_tenth_db as f64 / 10.0,
            sample_rate,
        );
        sections
    }
}

/// AU2 §6.7: the analytic magnitude of one `audio_parametric_eq` node.
///
/// The sum of the seven sections' transfer-function magnitudes plus the output
/// trim, evaluated from the same coefficient code the node runs, so the curve
/// the Mixer draws is the curve the mixer applies. `bypass` is deliberately
/// ignored: the curve describes the node's design, and the dock shows bypass
/// separately.
///
/// `at` resolves keyframes exactly as playback does; `sample_rate` is the rate
/// the response is designed at, which the app pins at 48 000 Hz because it
/// cannot know the device rate.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn parametric_eq_magnitude_db(
    effect: &Effect,
    at: TimeCode,
    hertz: f64,
    sample_rate: u32,
) -> f64 {
    let settings = ParametricEqSettings::read(effect, at);
    let cascade: f64 = settings
        .coefficients(sample_rate)
        .into_iter()
        .map(|section| section.magnitude_db(hertz, sample_rate))
        .sum();
    cascade + settings.output_gain_tenth_db as f64 / 10.0
}

/// AU2 §3.2: the seven sections, their output trim, and the parameter cache.
#[derive(Debug)]
struct ParametricEqState {
    sections: Vec<BiquadSection>,
    output_gain: f32,
    /// `(project frame, parameter epoch)` — the epoch is what makes a live
    /// retarget take effect on the next sample frame rather than at the next
    /// project-frame boundary (A13).
    cached: Option<(TimeCode, u64)>,
}

/// AU2 §3.3: the compressor's envelope, delay line, and RMS window.
#[derive(Debug)]
struct CompressorState {
    envelope: f32,
    delay: DelayLine,
    mean_square: MeanSquareWindow,
    minimum_gain: f32,
    /// AU2 §3.2: `(project frame, parameter epoch, knee_tenth_db, detector)`.
    /// AU2's two branch-selecting controls are resolved once per project frame
    /// on the same key as `bypass`, so a pre-AU2 document's compressor pays
    /// exactly AU1's five parameter reads per sample frame and no more. Both
    /// are project-frame staircases, so the output is identical.
    cached: Option<(TimeCode, u64, i64, i64)>,
}

/// AU2 §3.4: the gate's envelope and hold counter.
#[derive(Debug)]
struct GateState {
    envelope: f32,
    hold_counter: usize,
    minimum_gain: f32,
}

/// AU1 §3.1: the ducking envelope, with AU2 §3.8's chunk minimum beside it.
#[derive(Debug)]
struct DuckingState {
    envelope: f32,
    minimum_gain: f32,
}

/// AU2 §3.5: everything the true-peak limiter carries between sample frames.
#[derive(Debug)]
struct TruePeakLimiterState {
    /// The signal path, exactly `Lh` frames long.
    delay: DelayLine,
    estimator: TruePeakEstimator,
    /// The last `D` frames' sample peaks, so the sample-peak detector can also
    /// describe frame `i - D`.
    sample_peaks: VecDeque<f32>,
    minimum: SlidingMinimum,
    boxcar: Boxcar,
    /// `r[i-1]`.
    release_gain: f32,
    /// `D`: the detector's group delay, absorbed inside `Lh`. Zero when the
    /// node falls back to sample-peak detection.
    group_delay: usize,
    minimum_gain: f32,
}

#[derive(Debug)]
enum AudioEffectState {
    Stateless,
    Eq {
        low: Vec<f32>,
        high_pass_source: Vec<f32>,
    },
    ParametricEq(ParametricEqState),
    Compressor(CompressorState),
    Gate(GateState),
    Ducking(DuckingState),
    TruePeakLimiter(TruePeakLimiterState),
}

#[derive(Debug)]
struct AudioEffectRuntime {
    effect: Effect,
    state: AudioEffectState,
    /// AU2 §3.6: this node's own signal-path delay in sample frames, read once
    /// from `lookahead_milliseconds` and held for the runtime's life.
    latency_frames: usize,
    /// AU2 §3.2/A13: bumped whenever the runtime's parameters are retargeted
    /// live, so a cached project-frame tuple is discarded immediately. Part A
    /// has no live chain edit, so it never moves; Part B moves it.
    parameter_epoch: u64,
    /// AU2 §3.2: `bypass`, resolved once per project frame through the same
    /// `(project frame, parameter epoch)` key the parametric EQ uses. It is
    /// Hold-only, so it is a project-frame staircase by construction, and the
    /// pre-AU2 chunk path therefore gains no per-sample-frame map probe.
    bypass_cache: Option<(TimeCode, u64, bool)>,
}

impl AudioEffectRuntime {
    fn new(effect: &Effect, channels: usize, sample_rate: u32) -> Self {
        let latency_frames = stage_latency_frames(node_lookahead_milliseconds(effect), sample_rate);
        let state = match effect.name.as_str() {
            "audio_eq" => AudioEffectState::Eq {
                low: vec![0.0; channels],
                high_pass_source: vec![0.0; channels],
            },
            "audio_parametric_eq" => AudioEffectState::ParametricEq(ParametricEqState {
                sections: (0..PARAMETRIC_EQ_SECTIONS)
                    .map(|_| BiquadSection::new(channels))
                    .collect(),
                output_gain: 1.0,
                cached: None,
            }),
            "audio_compressor" => AudioEffectState::Compressor(CompressorState {
                envelope: 1.0,
                delay: DelayLine::new(latency_frames, channels),
                mean_square: MeanSquareWindow::new(
                    stage_latency_frames(
                        static_audio_value(effect, "rms_window_milliseconds", 10),
                        sample_rate,
                    )
                    .max(1),
                ),
                minimum_gain: 1.0,
                cached: None,
            }),
            "audio_gate" => AudioEffectState::Gate(GateState {
                envelope: 1.0,
                hold_counter: 0,
                minimum_gain: 1.0,
            }),
            "audio_ducking" => AudioEffectState::Ducking(DuckingState {
                envelope: 1.0,
                minimum_gain: 1.0,
            }),
            "audio_true_peak_limiter" => {
                // AU2 §3.5: `D` is absorbed inside `Lh`, so the node's total
                // signal delay stays exactly `Lh` whichever detector runs.
                // `Lh >= D + 1` is required; below that (only at test rates
                // under about 7 kHz) the node falls back to sample peak.
                let group_delay = if latency_frames > TRUE_PEAK_GROUP_DELAY_FRAMES {
                    TRUE_PEAK_GROUP_DELAY_FRAMES
                } else {
                    0
                };
                AudioEffectState::TruePeakLimiter(TruePeakLimiterState {
                    delay: DelayLine::new(latency_frames, channels),
                    estimator: TruePeakEstimator::new(channels),
                    sample_peaks: VecDeque::with_capacity(group_delay + 1),
                    minimum: SlidingMinimum::new(latency_frames - group_delay + 1),
                    boxcar: Boxcar::new((latency_frames - group_delay).max(1)),
                    release_gain: 1.0,
                    group_delay,
                    minimum_gain: 1.0,
                })
            }
            _ => AudioEffectState::Stateless,
        };
        Self {
            effect: effect.clone(),
            state,
            latency_frames,
            parameter_epoch: 0,
            bypass_cache: None,
        }
    }

    /// AU2 §5.8: adopt new parameter values for the same node, preserving
    /// every piece of DSP state — filter memories, envelopes, delay lines,
    /// RMS windows, and release state.
    ///
    /// The parameter epoch is bumped **unconditionally** (A13), which is what
    /// makes the edit audible on the next sample frame rather than at the next
    /// project-frame boundary: every per-project-frame cache in this module is
    /// keyed on `(project frame, parameter epoch)`.
    fn retarget(&mut self, effect: &Effect) {
        self.effect = effect.clone();
        self.parameter_epoch = self.parameter_epoch.wrapping_add(1);
    }

    /// AU2 §2.1: whether this node's gain computer is switched off at one
    /// project frame, read through the per-project-frame cache.
    fn bypassed(&mut self, project_at: TimeCode) -> bool {
        if let Some((at, epoch, bypassed)) = self.bypass_cache
            && at == project_at
            && epoch == self.parameter_epoch
        {
            return bypassed;
        }
        let bypassed = audio_value(&self.effect, AUDIO_BYPASS_PARAMETER, project_at, 0) != 0;
        self.bypass_cache = Some((project_at, self.parameter_epoch, bypassed));
        bypassed
    }

    /// AU2 §3.8: the chunk's largest gain reduction in decibels, reset for the
    /// next chunk. `None` for a node with no gain computer.
    fn take_gain_reduction(&mut self) -> Option<f32> {
        let minimum = match &mut self.state {
            AudioEffectState::Compressor(state) => &mut state.minimum_gain,
            AudioEffectState::Gate(state) => &mut state.minimum_gain,
            AudioEffectState::Ducking(state) => &mut state.minimum_gain,
            AudioEffectState::TruePeakLimiter(state) => &mut state.minimum_gain,
            AudioEffectState::Stateless
            | AudioEffectState::Eq { .. }
            | AudioEffectState::ParametricEq(_) => return None,
        };
        let gain = std::mem::replace(minimum, 1.0);
        Some((-20.0 * gain.max(f32::MIN_POSITIVE).log10()).max(0.0))
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
        // AU2 §2.1: `bypass` skips only the node's output multiply. The delay
        // line is still applied, so bypass never changes a chain's alignment,
        // and every dynamics node keeps its detector, envelope, window and
        // release state running, so un-bypassing cannot emit `Lh` frames of
        // material the gain computer never analysed.
        let bypassed = self.bypassed(project_at);
        let parameter_epoch = self.parameter_epoch;
        match (&*self.effect.name, &mut self.state) {
            ("audio_gain", AudioEffectState::Stateless) => {
                if bypassed {
                    return;
                }
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
                if bypassed {
                    return;
                }
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
            ("audio_parametric_eq", AudioEffectState::ParametricEq(state)) => {
                if bypassed {
                    return;
                }
                // AU2 §3.2: recompute only when the project frame or the
                // parameter epoch moves.
                if state.cached != Some((project_at, self.parameter_epoch)) {
                    let settings = ParametricEqSettings::read(&self.effect, project_at);
                    for (section, coefficients) in state
                        .sections
                        .iter_mut()
                        .zip(settings.coefficients(sample_rate))
                    {
                        section.set_coefficients(coefficients);
                    }
                    state.output_gain = db_gain(settings.output_gain_tenth_db);
                    state.cached = Some((project_at, self.parameter_epoch));
                }
                // AU2 A26: every section runs whenever the node is active, so
                // an automated gain crossing zero never throws away a tail.
                for section in &mut state.sections {
                    section.process(samples);
                }
                let output_gain = state.output_gain;
                for sample in samples {
                    *sample *= output_gain;
                }
            }
            ("audio_compressor", AudioEffectState::Compressor(state)) => {
                let threshold_tenth_db =
                    audio_value(&self.effect, "threshold_tenth_db", project_at, 0);
                let ratio =
                    audio_value(&self.effect, "ratio_hundredths", project_at, 100) as f32 / 100.0;
                let (knee_tenth_db, detector) = match state.cached {
                    Some((at, epoch, knee, detector))
                        if at == project_at && epoch == parameter_epoch =>
                    {
                        (knee, detector)
                    }
                    _ => {
                        let knee = audio_value(&self.effect, "knee_tenth_db", project_at, 0);
                        let detector = audio_value(&self.effect, "detector", project_at, 0);
                        state.cached = Some((project_at, parameter_epoch, knee, detector));
                        (knee, detector)
                    }
                };
                let level = if detector == 1 {
                    // AU2 §3.3: a boxcar running mean of the per-frame mean
                    // square across channels, on the undelayed input.
                    let channels = samples.len().max(1) as f64;
                    let mean_square = samples
                        .iter()
                        .map(|sample| f64::from(*sample) * f64::from(*sample))
                        .sum::<f64>()
                        / channels;
                    #[allow(clippy::cast_possible_truncation)]
                    let level = state.mean_square.push(mean_square).sqrt() as f32;
                    level
                } else {
                    samples
                        .iter()
                        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
                };
                let level_db = amplitude_db(level);
                let threshold_db = threshold_tenth_db as f32 / 10.0;
                let target = if knee_tenth_db == 0 {
                    // AU2 §3.3/R24: the AU1 expression, verbatim, so an
                    // existing-shaped document is bit-identical.
                    if level_db > threshold_db && ratio > 1.0 {
                        10.0_f32.powf(
                            ((threshold_db + (level_db - threshold_db) / ratio) - level_db) / 20.0,
                        )
                    } else {
                        1.0
                    }
                } else {
                    let knee_db = knee_tenth_db as f32 / 10.0;
                    let over = 2.0 * (level_db - threshold_db);
                    let output_db = if over < -knee_db {
                        level_db
                    } else if over.abs() <= knee_db {
                        let excess = level_db - threshold_db + knee_db / 2.0;
                        level_db + (1.0 / ratio - 1.0) * excess * excess / (2.0 * knee_db)
                    } else {
                        threshold_db + (level_db - threshold_db) / ratio
                    };
                    10.0_f32.powf((output_db - level_db) / 20.0)
                };
                smooth_gain(
                    &mut state.envelope,
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
                // AU2 §3.3: the detector read the undelayed input above; the
                // signal path is delayed here, and makeup multiplies after the
                // envelope, unsmoothed, as it always has.
                state.delay.process(samples);
                if bypassed {
                    // The envelope above kept tracking, so un-bypassing lands
                    // on a live envelope rather than a stale one; only the
                    // multiply and the telemetry are skipped, because a
                    // bypassed node applies no reduction.
                    return;
                }
                state.minimum_gain = state.minimum_gain.min(state.envelope.min(1.0));
                let envelope = state.envelope;
                for sample in samples {
                    *sample *= envelope * makeup;
                }
            }
            ("audio_gate", AudioEffectState::Gate(state)) => {
                let level = samples
                    .iter()
                    .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
                let level_db = amplitude_db(level);
                let threshold_db =
                    audio_value(&self.effect, "threshold_tenth_db", project_at, -600) as f32 / 10.0;
                let ratio =
                    audio_value(&self.effect, "ratio_hundredths", project_at, 100) as f32 / 100.0;
                let range_db =
                    audio_value(&self.effect, "range_tenth_db", project_at, 0) as f32 / 10.0;
                let target_db = if level_db >= threshold_db {
                    state.hold_counter = stage_latency_frames(
                        audio_value(&self.effect, "hold_milliseconds", project_at, 10),
                        sample_rate,
                    );
                    0.0
                } else if state.hold_counter > 0 {
                    state.hold_counter -= 1;
                    0.0
                } else {
                    // Downward expansion: `L - T` is negative below the
                    // threshold, so this falls to the range floor and stops.
                    ((ratio - 1.0) * (level_db - threshold_db)).max(-range_db)
                };
                let target = 10.0_f32.powf(target_db / 20.0);
                // AU2 §3.4: the first time argument is the falling-gain
                // constant, and a gate's falling gain is its release.
                smooth_gain(
                    &mut state.envelope,
                    target,
                    audio_value(&self.effect, "release_milliseconds", project_at, 100),
                    audio_value(&self.effect, "attack_milliseconds", project_at, 1),
                    sample_rate,
                );
                if bypassed {
                    return;
                }
                state.minimum_gain = state.minimum_gain.min(state.envelope.min(1.0));
                let envelope = state.envelope;
                for sample in samples {
                    *sample *= envelope;
                }
            }
            ("audio_limiter", AudioEffectState::Stateless) => {
                if bypassed {
                    return;
                }
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
            ("audio_true_peak_limiter", AudioEffectState::TruePeakLimiter(state)) => {
                let ceiling = db_gain(audio_value(&self.effect, "ceiling_tenth_db", project_at, 0));
                let sample_peak = samples
                    .iter()
                    .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
                // The history advances every frame so a hold keyframe on
                // `true_peak` can switch detectors without a warm-up.
                state.estimator.push(samples);
                state.sample_peaks.push_back(sample_peak);
                let delayed_sample_peak = if state.sample_peaks.len() > state.group_delay {
                    state.sample_peaks.pop_front().unwrap_or(0.0)
                } else {
                    0.0
                };
                // AU2 §3.5: either way the estimate describes frame `i - D`.
                // The 4x phase grid is offset by an eighth of a sample, so a
                // lone impulse reads about 0.23 dB *under* its own sample, and
                // an estimate below the sample peak would let the sample peak
                // through above the ceiling. Folding the sample peak of the
                // same frame in keeps the estimate an upper bound on both
                // (§0 errata).
                let estimate = if state.group_delay > 0
                    && audio_value(&self.effect, "true_peak", project_at, 1) == 1
                {
                    state.estimator.estimate().max(delayed_sample_peak)
                } else {
                    delayed_sample_peak
                };
                let required = (ceiling / estimate.max(1.0e-9)).min(1.0);
                let minimum = state.minimum.push(required);
                let release_samples =
                    audio_value(&self.effect, "release_milliseconds", project_at, 50).max(1) as f32
                        * sample_rate.max(1) as f32
                        / 1_000.0;
                let coefficient = (-1.0 / release_samples.max(1.0)).exp();
                // AU2 A27: the identity short-circuit is structural. Without
                // it, `c*1 + (1-c)*1` is exactly one only while `c >= 0.5`.
                // It is two-way: `r <- c*r + (1-c)` has a rounding fixed point
                // strictly below one — `1 - ulp/(2*(1-c))`, about 7.2e-5 at a
                // 50 ms release and 48 kHz — so without a downward snap a
                // limiter that has ever engaged attenuates the rest of the
                // programme and its gain-reduction slot never returns to
                // 0 dB. Snapping is only ever reached with `m >= 1`, which
                // forces `required[d] == 1` for every emitted sample the
                // boxcar covers, so the ceiling proof is untouched.
                state.release_gain = if minimum >= 1.0 {
                    if state.release_gain >= 1.0 {
                        1.0
                    } else {
                        let released =
                            coefficient * state.release_gain + (1.0 - coefficient) * minimum;
                        if released <= state.release_gain
                            || released >= 1.0 - RELEASE_IDENTITY_EPSILON
                        {
                            1.0
                        } else {
                            released.min(minimum)
                        }
                    }
                } else {
                    minimum.min(coefficient * state.release_gain + (1.0 - coefficient) * minimum)
                };
                let gain = state.boxcar.push(state.release_gain);
                // `out[i] = x[i - Lh] * g[i]`.
                state.delay.process(samples);
                if bypassed {
                    // The detector, the sliding minimum, the boxcar and the
                    // release above all kept running, so the frames that
                    // entered during the bypass window were analysed and
                    // un-bypassing cannot emit `Lh` frames over the ceiling.
                    return;
                }
                state.minimum_gain = state.minimum_gain.min(gain.min(1.0));
                for sample in samples {
                    *sample *= gain;
                }
            }
            ("audio_ducking", AudioEffectState::Ducking(state)) => {
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
                    &mut state.envelope,
                    target,
                    audio_value(&self.effect, "attack_milliseconds", project_at, 20),
                    audio_value(&self.effect, "release_milliseconds", project_at, 300),
                    sample_rate,
                );
                if bypassed {
                    return;
                }
                state.minimum_gain = state.minimum_gain.min(state.envelope.min(1.0));
                let envelope = state.envelope;
                for sample in samples {
                    *sample *= envelope;
                }
            }
            _ => {}
        }
    }
}

/// AU3 §5.6: run one static audio node over a whole interleaved buffer.
///
/// Builds a fresh [`AudioEffectRuntime`] from `effect`, calls `process_frame`
/// with `project_at = TimeCode::ZERO` on every frame — so every automation
/// curve is read at one project frame and the node behaves as a static one —
/// feeds `Lh` zero frames after the input so the lookahead delay line is
/// flushed, and drops the first `Lh` output frames so the returned buffer is
/// aligned with the input it was handed. The master carries no sidechain, so
/// the tap reads exact zeros, exactly as `AudioMixProcessor` does on the
/// master chain.
///
/// # Errors
///
/// Returns `MediaError::Backend` when `channels` is zero, or — per AU3 §5.6's
/// F11 bullet, which forbids a panic here — when the assembled output length
/// differs from the input's, which is what an unaligned `input` produces.
pub(crate) fn process_buffer_static(
    effect: &Effect,
    sample_rate: u32,
    channels: usize,
    input: &[f32],
) -> Result<Vec<f32>, MediaError> {
    if channels == 0 {
        return Err(MediaError::Backend(
            "a static audio node needs at least one channel".to_owned(),
        ));
    }
    let mut runtime = AudioEffectRuntime::new(effect, channels, sample_rate);
    let latency_frames = runtime.latency_frames;
    let silence = vec![0.0_f32; channels];
    let mut frame = vec![0.0_f32; channels];
    let mut output = Vec::with_capacity(input.len().saturating_add(latency_frames * channels));
    for source in input.chunks_exact(channels) {
        frame.copy_from_slice(source);
        runtime.process_frame(&mut frame, &silence, TimeCode::ZERO, sample_rate);
        output.extend_from_slice(&frame);
    }
    // AU2 §3.5: the node's total signal delay is exactly `Lh` frames, so `Lh`
    // zero frames flush the last real frame out of the delay line.
    for _ in 0..latency_frames {
        frame.fill(0.0);
        runtime.process_frame(&mut frame, &silence, TimeCode::ZERO, sample_rate);
        output.extend_from_slice(&frame);
    }
    let dropped = latency_frames.saturating_mul(channels);
    if dropped > output.len() || output.len() - dropped != input.len() {
        return Err(MediaError::Backend(format!(
            "a static audio node changed its buffer length: {} input samples became {}",
            input.len(),
            output.len().saturating_sub(dropped)
        )));
    }
    output.drain(..dropped);
    Ok(output)
}

/// AU2 §5.8: one node's contribution to a chain's **structure** — its id, its
/// effect name, and its static `lookahead_milliseconds`.
///
/// Two chains with the same ordered structure can be retargeted in place;
/// anything else rebuilds. `rms_window_milliseconds` is static too (AU2 §0
/// E16) but is deliberately not part of the key, because E16 defines the
/// window as read once when the chain is built.
fn node_structure(effect: &Effect) -> (EffectId, &str, i64) {
    (
        effect.id,
        effect.name.as_str(),
        node_lookahead_milliseconds(effect),
    )
}

/// AU2 §3.6: one node's declared lookahead in milliseconds, read statically.
fn node_lookahead_milliseconds(effect: &Effect) -> i64 {
    if kinewright_core::is_static_audio_parameter(&effect.name, AUDIO_LOOKAHEAD_PARAMETER) {
        let neutral = if effect.name == "audio_true_peak_limiter" {
            5
        } else {
            0
        };
        static_audio_value(effect, AUDIO_LOOKAHEAD_PARAMETER, neutral)
    } else {
        0
    }
}

/// AU2 §5.8: whether an installed chain and a document chain share a structure.
fn chain_structure_matches(runtimes: &[AudioEffectRuntime], effects: &[Effect]) -> bool {
    runtimes.len() == effects.len()
        && runtimes
            .iter()
            .zip(effects)
            .all(|(runtime, effect)| node_structure(&runtime.effect) == node_structure(effect))
}

/// AU2 §5.8: retarget an already-matching chain, node by node.
fn retarget_chain(runtimes: &mut [AudioEffectRuntime], effects: &[Effect]) {
    for (runtime, effect) in runtimes.iter_mut().zip(effects) {
        runtime.retarget(effect);
    }
}

/// AU2 §5.6: the master chain's runtime — the same machinery a bus carries,
/// with no routed tracks and no sidechain.
struct AudioMasterRuntime {
    /// AU2 §5.6: the master fader, applied to the summed mix before the chain.
    gain: GainRamp,
    effects: Vec<AudioEffectRuntime>,
    /// AU2 §3.6: the pad that brings the chain's own node delays up to
    /// `L_master`. Zero-length for every chain whose nodes already sum to it.
    pad: DelayLine,
    /// A scratch buffer of exact zeros: the master carries no sidechain, so
    /// `audio_ducking` is rejected on it (AU2 §5.4).
    silence: Vec<f32>,
}

impl AudioMasterRuntime {
    fn new(
        master: &AudioMaster,
        channels: usize,
        sample_rate: u32,
        project_fps: Rational,
        ramp_frames: usize,
        master_stage_frames: usize,
    ) -> Self {
        let effects = master
            .effects
            .iter()
            .map(|effect| AudioEffectRuntime::new(effect, channels, sample_rate))
            .collect::<Vec<_>>();
        let node_frames = effects
            .iter()
            .map(|effect| effect.latency_frames)
            .sum::<usize>();
        Self {
            gain: GainRamp::settled(
                master.gain_tenth_db,
                master.gain_curve.as_ref(),
                ramp_frames,
                sample_rate,
                project_fps,
            ),
            effects,
            pad: DelayLine::new(master_stage_frames.saturating_sub(node_frames), channels),
            silence: Vec::new(),
        }
    }

    /// AU2 §5.8: adopt a same-structure master chain in place.
    fn retarget(&mut self, master: &AudioMaster) {
        retarget_chain(&mut self.effects, &master.effects);
        self.gain
            .retarget(master.gain_tenth_db, master.gain_curve.as_ref());
    }
}

#[derive(Debug)]
struct AudioBusRuntime {
    id: AudioBusId,
    tracks: Vec<TrackId>,
    sidechain_tracks: Vec<TrackId>,
    effects: Vec<AudioEffectRuntime>,
    /// AU2 §5.6: the post-effects bus fader.
    gain: GainRamp,
    /// AU2 §3.6: the alignment pad that brings this chain's own node delays up
    /// to the bus stage's latency, so every bus leaves the stage aligned.
    pad: DelayLine,
}

impl AudioBusRuntime {
    fn new(
        bus: &AudioBus,
        channels: usize,
        sample_rate: u32,
        project_fps: Rational,
        ramp_frames: usize,
        bus_stage_frames: usize,
    ) -> Self {
        let effects = bus
            .effects
            .iter()
            .map(|effect| AudioEffectRuntime::new(effect, channels, sample_rate))
            .collect::<Vec<_>>();
        // AU2 §3.6: non-negative because the floor function is superadditive
        // and every node's delay uses the same truncating conversion.
        let node_frames = effects
            .iter()
            .map(|effect| effect.latency_frames)
            .sum::<usize>();
        Self {
            id: bus.id,
            tracks: bus.tracks.clone(),
            sidechain_tracks: bus.ducking_sidechain_tracks.clone(),
            effects,
            gain: GainRamp::settled(
                bus.gain_tenth_db,
                bus.gain_curve.as_ref(),
                ramp_frames,
                sample_rate,
                project_fps,
            ),
            pad: DelayLine::new(bus_stage_frames.saturating_sub(node_frames), channels),
        }
    }

    /// AU2 §5.8: adopt a same-structure bus without disturbing any DSP state.
    ///
    /// Routing and sidechain taps are re-read on every update, the chain is
    /// retargeted node by node, and the fader ramps over the 5 ms constant.
    fn retarget(&mut self, bus: &AudioBus) {
        self.tracks.clear();
        self.tracks.extend_from_slice(&bus.tracks);
        self.sidechain_tracks.clear();
        self.sidechain_tracks
            .extend_from_slice(&bus.ducking_sidechain_tracks);
        retarget_chain(&mut self.effects, &bus.effects);
        self.gain
            .retarget(bus.gain_tenth_db, bus.gain_curve.as_ref());
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

/// AU4 §3.1: the forward declick applied to a `Hold` step, in milliseconds.
///
/// Deliberately equal to AU1's `TRACK_MIX_RAMP_MILLISECONDS` and deliberately a
/// **separate** constant: AU1's ramp is a playback-only transient that export
/// never runs (AU1 §3.4), while this one is derived from the document and is
/// therefore present in export, in playback, and in every measurement.
pub(crate) const AUTOMATION_HOLD_DECLICK_MILLISECONDS: u32 = 5;

/// AU4 §3.1: 240 sample frames at 48 kHz.
fn automation_hold_declick_frames(sample_rate: u32) -> u64 {
    u64::from(sample_rate).saturating_mul(u64::from(AUTOMATION_HOLD_DECLICK_MILLISECONDS)) / 1_000
}

/// AU4 §3.1: the two integer frame anchors bracketing one absolute project
/// sample, the weight between them, and the `Hold` declick term.
///
/// Integer in, three floats out; the same call in both mix paths.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AutomationStep {
    /// The owner's value at the project frame containing `project_sample`.
    pub v0: i64,
    /// The value at the next frame, or `v0` when the segment holds.
    pub v1: i64,
    /// The weight between the two anchors, in `0.0..=1.0`.
    pub t: f32,
    /// `Some((held, w))` asks the consumer for one more lerp, from the amplitude
    /// of `held` toward the value it just computed, at weight `w` in `0.0..=1.0`.
    pub declick: Option<(i64, f32)>,
}

/// AU4 §3.1 rule 41: the one place automation becomes a float.
///
/// `origin` is the owner's time base — a clip envelope subtracts the clip's
/// `timeline_start`, every project-frame owner passes [`TimeCode::ZERO`].
// Rule 68: every boundary above this line is integral; `t` is the intentional
// integer-to-float conversion and the sample counts it divides are bounded by
// one project frame.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn automation_step(
    curve: Option<&AutomationCurve>,
    static_value: i64,
    origin: TimeCode,
    project_sample: u64,
    sample_rate: u32,
    project_fps: Rational,
) -> AutomationStep {
    // Rule 41.1: `frame_to_samples` returns 0 for a non-positive frame, so a
    // negative local frame is clamped *here* and never by it.
    let local = samples_to_frame(project_sample, sample_rate, project_fps)
        .0
        .saturating_sub(origin.0);
    let before_origin = local < 0;
    let frame = local.max(0);
    // Rule 41.2: both bounds are exact u128 floors.
    let s0 = frame_to_samples(
        TimeCode(frame.saturating_add(origin.0)),
        sample_rate,
        project_fps,
    );
    let s1 = frame_to_samples(
        TimeCode(frame.saturating_add(origin.0).saturating_add(1)),
        sample_rate,
        project_fps,
    );
    // Rule 41.3 and 41.4: `v1` is **segment-aware**, which is what makes a
    // `Hold` segment flat.
    let v0 = curve
        .and_then(|curve| curve.value_at(TimeCode(frame)))
        .unwrap_or(static_value);
    let v1 = if curve.is_none_or(|curve| curve.holds_at(TimeCode(frame))) {
        v0
    } else {
        curve
            .and_then(|curve| curve.value_at(TimeCode(frame.saturating_add(1))))
            .unwrap_or(static_value)
    };
    let offset = project_sample.saturating_sub(s0);
    let frame_span = s1.saturating_sub(s0);
    // Rule 41.5, with rule 41.1's `t = 0.0` for a negative local frame.
    let t = if before_origin || s1 <= s0 + 1 {
        0.0
    } else {
        (offset as f32 / frame_span as f32).clamp(0.0, 1.0)
    };
    // Rule 41.6: the forward declick.
    let declick = curve
        .and_then(|curve| curve.hold_step_at(TimeCode(frame)))
        .and_then(|step| {
            // Both extra `min` terms are **defensive**: keys are strictly
            // increasing, so `next_key >= at + 1` and `segment_span >=
            // s1 - s0` always, and one project frame is 1 600 sample frames at
            // 30 fps / 48 kHz. They are written down so the formula stays
            // correct above 200 fps, not because either binds today.
            let segment_span = match step.next_key {
                Some(next) => frame_to_samples(
                    TimeCode(next.0.saturating_add(origin.0)),
                    sample_rate,
                    project_fps,
                )
                .saturating_sub(s0),
                None => frame_span,
            };
            let declick_frames = automation_hold_declick_frames(sample_rate)
                .min(frame_span)
                .min(segment_span)
                .max(1);
            (offset < declick_frames).then(|| {
                (
                    step.previous_value,
                    (offset.saturating_add(1)) as f32 / declick_frames as f32,
                )
            })
        });
    AutomationStep { v0, v1, t, declick }
}

/// AU4 §3.1 rule 46: one **curve's** per-project-frame memo of its two anchor
/// values, keyed `(frame_index, curve_epoch)`.
///
/// The AU2 per-frame cache idiom (audio.rs:818-828) applied one level down: the
/// memo cannot be keyed per *owner* because a track stage owns **two** curves
/// and a frame-indexed per-owner memo would collide between gain and pan.
/// Export never retargets, so the epoch is constant there and the memo changes
/// nothing observable; live, it is what makes an edit audible on the next
/// sample frame rather than up to 1 599 samples later.
#[derive(Debug, Clone, Copy)]
struct AnchorMemo<T: Copy> {
    key: Option<(i64, u32)>,
    anchors: (T, T),
}

impl<T: Copy + Default> AnchorMemo<T> {
    fn new() -> Self {
        Self {
            key: None,
            anchors: (T::default(), T::default()),
        }
    }

    fn anchors(&mut self, frame: i64, epoch: u32, build: impl FnOnce() -> (T, T)) -> (T, T) {
        if self.key == Some((frame, epoch)) {
            return self.anchors;
        }
        let anchors = build();
        self.key = Some((frame, epoch));
        self.anchors = anchors;
        anchors
    }
}

/// AU4 §3.1 rule 46: the memo key — the owner-local frame index the two anchors
/// were read at, clamped exactly as [`automation_step`] clamps it.
fn automation_frame(
    origin: TimeCode,
    project_sample: u64,
    sample_rate: u32,
    project_fps: Rational,
) -> i64 {
    samples_to_frame(project_sample, sample_rate, project_fps)
        .0
        .saturating_sub(origin.0)
        .max(0)
}

/// AU4 §3.1 rule 42: the gain composition — `a + (b - a) * t`, then the forward
/// declick, whose `w >= 1.0` arm returns `target` **exactly**.
///
/// The two-rounding form `(1 - t) * a + t * b` is the one an implementer
/// reaches for first and it does **not** have the property that makes a
/// constant curve free: `a + (a - a) * t == a` bit-exactly for every finite
/// `t`. The `w >= 1.0` arm is normative, not an optimisation: `h + (target -
/// h)` is up to ~500 ulps from `target` on a -60 dB duck, and the value at the
/// end of a declick is *defined* to be the automated value exactly.
fn automated_gain(step: &AutomationStep, anchors: (f32, f32)) -> f32 {
    let (a0, a1) = anchors;
    let target = a0 + (a1 - a0) * step.t;
    match step.declick {
        Some((held, weight)) if weight < 1.0 => {
            let held = db_gain(held);
            held + (target - held) * weight
        }
        _ => target,
    }
}

/// AU4 §3.1 rule 42: the same two-step composition, per channel, over two
/// [`pan_channel_ratios`] pairs.
fn automated_pan(step: &AutomationStep, anchors: ([f32; 2], [f32; 2]), law: PanLaw) -> [f32; 2] {
    let (p0, p1) = anchors;
    let mut target = [
        p0[0] + (p1[0] - p0[0]) * step.t,
        p0[1] + (p1[1] - p0[1]) * step.t,
    ];
    if let Some((held, weight)) = step.declick
        && weight < 1.0
    {
        let held = pan_channel_ratios(law, pan_percent_of(held));
        for (channel, target) in target.iter_mut().enumerate() {
            *target = held[channel] + (*target - held[channel]) * weight;
        }
    }
    target
}

/// AU4 §3.1: a curve value read as a pan position. Validation bounds every
/// stored key to `TRACK_MIX_PAN_MIN..=TRACK_MIX_PAN_MAX`; a hand-edited
/// document that escaped it clamps rather than panicking.
fn pan_percent_of(value: i64) -> i32 {
    i32::try_from(value)
        .unwrap_or(if value < 0 {
            TRACK_MIX_PAN_MIN
        } else {
            TRACK_MIX_PAN_MAX
        })
        .clamp(TRACK_MIX_PAN_MIN, TRACK_MIX_PAN_MAX)
}

/// AU2 §5.7: one scalar gain with the [`TrackStageRuntime`] ramp discipline.
///
/// Construction is settled (`current == target`, `ramp_index == ramp_frames`),
/// so export and a freshly opened playback mixer never ramp and steady-state
/// parity is untouched. Playback-only transient state, not observable through
/// any facet.
#[derive(Debug)]
struct GainRamp {
    target: f32,
    current: f32,
    ramp_start: f32,
    ramp_index: usize,
    /// AU4 §3.4 rule 61: the parked scalar this fader's curve replaces, kept
    /// for rule 59's document comparison and as `automation_step`'s fallback.
    gain_tenth_db: i32,
    /// AU4 §3.4 rule 61: project-frame keyed, so the origin is
    /// [`TimeCode::ZERO`] — exactly what the chain nodes beside it already do.
    curve: Option<AutomationCurve>,
    /// AU4 §3.1 rule 46: bumped **unconditionally** by `retarget`.
    curve_epoch: u32,
    memo: AnchorMemo<f32>,
    sample_rate: u32,
    project_fps: Rational,
}

impl GainRamp {
    fn settled(
        gain_tenth_db: i32,
        curve: Option<&AutomationCurve>,
        ramp_frames: usize,
        sample_rate: u32,
        project_fps: Rational,
    ) -> Self {
        let target = db_gain(i64::from(gain_tenth_db));
        Self {
            target,
            current: target,
            ramp_start: target,
            ramp_index: ramp_frames,
            gain_tenth_db,
            curve: curve.cloned(),
            curve_epoch: 0,
            memo: AnchorMemo::new(),
            sample_rate,
            project_fps,
        }
    }

    /// AU2 §5.7: restart from the current interpolated value when the new
    /// target differs from it, exactly as [`TrackStageRuntime::retarget`] does.
    ///
    /// AU4 §3.4 rule 61 / §3.3 rule 59: the comparison is against the
    /// **document tuple** `(gain_tenth_db, curve)`, not against the derived
    /// amplitude, because `update_audio_mix` retargets every fader on every mix
    /// edit and an unconditional ramp would modulate faders the editor did not
    /// touch.
    // The comparison is an exact identity test, not a tolerance question: an
    // unchanged target must not restart the ramp.
    fn retarget(&mut self, gain_tenth_db: i32, curve: Option<&AutomationCurve>) {
        let unchanged = self.gain_tenth_db == gain_tenth_db && self.curve.as_ref() == curve;
        self.gain_tenth_db = gain_tenth_db;
        self.target = db_gain(i64::from(gain_tenth_db));
        if self.curve.as_ref() != curve {
            self.curve = curve.cloned();
        }
        // AU4 §3.1 rule 46: unconditional, exactly as `AudioEffectNode::retarget`
        // bumps `parameter_epoch`.
        self.curve_epoch = self.curve_epoch.wrapping_add(1);
        // AU4 §0 E24: the unchanged arm leaves the ramp **exactly as it is**
        // rather than forcing `ramp_index = ramp_frames`. AU1's arm could force
        // it because its condition was `target == self.current`, so settling
        // was a no-op; under rule 59's document comparison an unchanged owner
        // caught mid-ramp — every other track when one fader moves — would
        // freeze at the interpolated value forever, because the settled arm of
        // `apply` reads `current` and only `advance` ever assigns `target` to
        // it. Leaving it alone satisfies AU1's rule as written ("an unchanged
        // target must not **restart** the ramp") and lets the ramp finish.
        if !unchanged {
            self.ramp_start = self.current;
            self.ramp_index = 0;
        }
    }

    #[cfg(test)]
    fn is_settled(&self, ramp_frames: usize) -> bool {
        self.ramp_index >= ramp_frames
    }

    /// AU4 §3.4 rule 61: the automated target at one absolute input sample.
    fn automated_target(&mut self, project_sample: u64) -> f32 {
        let step = automation_step(
            self.curve.as_ref(),
            i64::from(self.gain_tenth_db),
            TimeCode::ZERO,
            project_sample,
            self.sample_rate,
            self.project_fps,
        );
        let frame = automation_frame(
            TimeCode::ZERO,
            project_sample,
            self.sample_rate,
            self.project_fps,
        );
        let epoch = self.curve_epoch;
        let anchors = self
            .memo
            .anchors(frame, epoch, || (db_gain(step.v0), db_gain(step.v1)));
        automated_gain(&step, anchors)
    }

    // The ramp position is an integer sample-frame index converted once per frame.
    #[allow(clippy::cast_precision_loss)]
    fn advance(&mut self, ramp_frames: usize) {
        if self.ramp_index >= ramp_frames {
            return;
        }
        let progress = (self.ramp_index + 1) as f32 / ramp_frames as f32;
        self.current = self.ramp_start + (self.target - self.ramp_start) * progress;
        self.ramp_index += 1;
        if self.ramp_index >= ramp_frames {
            self.current = self.target;
        }
    }

    /// AU4 §3.3 rule 57: the automated target is the *target*; the live ramp
    /// runs on top of it and `current` records the value last applied
    /// (rule 58).
    fn advance_automated(&mut self, target: f32, ramp_frames: usize) {
        if self.ramp_index >= ramp_frames {
            self.current = target;
            return;
        }
        // The ramp position is an integer sample-frame index converted once.
        #[allow(clippy::cast_precision_loss)]
        let progress = (self.ramp_index + 1) as f32 / ramp_frames as f32;
        self.current = self.ramp_start + (target - self.ramp_start) * progress;
        self.ramp_index += 1;
        if self.ramp_index >= ramp_frames {
            self.current = target;
        }
    }

    /// Multiply one interleaved chunk by this gain, advancing the ramp once per
    /// sample frame while it runs.
    ///
    /// AU4 §3.4 rule 61: `start_sample` is the chunk's absolute **input**
    /// sample index (§3.5 rule 64), and the automated arm is taken whenever a
    /// curve exists — so the two fast paths below stay bit-identical to pre-AU4
    /// for every curve-free document (A13).
    // A settled unity gain is an exact identity: `x * 1.0 == x` in IEEE 754, so
    // a neutral fader stays bit-identical to a pass-through.
    #[allow(clippy::float_cmp)]
    fn apply(
        &mut self,
        samples: &mut [f32],
        channels: usize,
        ramp_frames: usize,
        start_sample: u64,
    ) {
        if self.curve.is_some() {
            for (index, frame) in samples.chunks_mut(channels.max(1)).enumerate() {
                let project_sample =
                    start_sample.saturating_add(u64::try_from(index).unwrap_or(u64::MAX));
                let target = self.automated_target(project_sample);
                self.advance_automated(target, ramp_frames);
                let gain = self.current;
                for sample in frame.iter_mut() {
                    *sample *= gain;
                }
            }
            return;
        }
        if self.ramp_index >= ramp_frames {
            if self.current == 1.0 {
                return;
            }
            let gain = self.current;
            for sample in samples.iter_mut() {
                *sample *= gain;
            }
            return;
        }
        for frame in samples.chunks_mut(channels.max(1)) {
            self.advance(ramp_frames);
            let gain = self.current;
            for sample in frame.iter_mut() {
                *sample *= gain;
            }
        }
    }
}

/// AU2 §5.7: one pan position as a pair of channel ratios under one law.
///
/// `ConstantPower` computes in `f64` and casts once (R16). The three cardinal
/// positions are special-cased to exact ratios so it does not lose AU1's "hard
/// left yields the left channel at unity and the right silent": without the
/// special case `cos(PI/2)` in `f32` is `-4.371e-8`, which leaks -147 dBFS into
/// the far channel.
///
/// `Balance` keeps AU1's own `f32` expression **verbatim** (AU2 §0 E42): 82 of
/// the 201 integer positions round differently when the same expression is
/// evaluated in `f64` and cast once, so computing it in `f64` would change the
/// exported bytes of every pre-AU2 document carrying one of those pans, which
/// AU2 §5.10(f) and A14 forbid. Centre is the exact identity under it either
/// way.
// The pan position is an integer percent and the ratios are bounded by one:
// the single cast per channel is the intentional analysis-to-mix conversion.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn pan_channel_ratios(law: PanLaw, pan_percent: i32) -> [f32; 2] {
    match law {
        // AU1 §3.1 balance law: centre is the exact identity, and the law
        // never boosts.
        PanLaw::Balance => {
            let pan = pan_percent as f32 / 100.0;
            [1.0 - pan.max(0.0), 1.0 + pan.min(0.0)]
        }
        PanLaw::ConstantPower => match pan_percent {
            TRACK_MIX_PAN_MIN => [1.0, 0.0],
            TRACK_MIX_PAN_MAX => [0.0, 1.0],
            0 => [
                std::f32::consts::FRAC_1_SQRT_2,
                std::f32::consts::FRAC_1_SQRT_2,
            ],
            _ => {
                let theta = (f64::from(pan_percent) / 100.0 + 1.0) * std::f64::consts::FRAC_PI_4;
                [theta.cos() as f32, theta.sin() as f32]
            }
        },
    }
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
    let pan = pan_channel_ratios(document.audio_mix.pan_law, mix.pan_percent);
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
    /// AU4 §3.3 rule 55: the parked scalars a curve replaces, and the document
    /// inputs rule 59 compares. `pan_law` is in the tuple because
    /// `track_stage_parameters` reads it and `SetPanLaw` takes the live path.
    gain_tenth_db: i32,
    pan_percent: i32,
    pan_law: PanLaw,
    /// AU4 §2.1: **project**-frame keyed, so the origin is [`TimeCode::ZERO`].
    gain_curve: Option<AutomationCurve>,
    pan_curve: Option<AutomationCurve>,
    /// AU4 §3.1 rule 46: bumped **unconditionally** by `retarget`.
    curve_epoch: u32,
    gain_memo: AnchorMemo<f32>,
    pan_memo: AnchorMemo<[f32; 2]>,
    channels: usize,
    sample_rate: u32,
    project_fps: Rational,
}

impl TrackStageRuntime {
    /// A freshly built stage is settled, so export and a newly opened playback
    /// mixer never ramp (AU1 §3.4).
    fn new(
        document: &Document,
        track: TrackId,
        channels: usize,
        ramp_frames: usize,
        sample_rate: u32,
    ) -> Self {
        let (audible, gain, target) = track_stage_parameters(document, track, channels);
        let mix = document.track_mix(track);
        Self {
            track,
            audible,
            target,
            current: target,
            ramp_start: target,
            ramp_index: ramp_frames,
            gain,
            gain_tenth_db: mix.gain_tenth_db,
            pan_percent: mix.pan_percent,
            pan_law: document.audio_mix.pan_law,
            gain_curve: mix.gain_curve,
            pan_curve: mix.pan_curve,
            curve_epoch: 0,
            gain_memo: AnchorMemo::new(),
            pan_memo: AnchorMemo::new(),
            channels,
            sample_rate,
            project_fps: document.fps,
        }
    }

    /// AU4 §3.3 rule 56: whether this stage evaluates a curve at all. The AU1
    /// fast paths are taken only when both curves are `None`, so a curve-free
    /// document stays bit-identical (A13).
    const fn automated(&self) -> bool {
        self.gain_curve.is_some() || self.pan_curve.is_some()
    }

    /// AU1 §3.4: retarget, ramping from the current interpolated value when
    /// the new target differs from it.
    ///
    /// AU4 §3.3 rule 59: the comparison is against the **document tuple**
    /// `(audible, gain_tenth_db, pan_percent, gain_curve, pan_curve, pan_law)`
    /// — every input `track_stage_parameters` reads — because
    /// `update_audio_mix` retargets **every** stage on every mix edit, and an
    /// unconditional `pending_ramp` would restart a 5 ms ramp on every other
    /// automated track whenever one fader moved. `pan_law` is in the tuple
    /// because a law change moves the derived target without moving any
    /// per-track field; omitting it would make a live `Balance` / `ConstantPower`
    /// switch a no-op on every track.
    // The comparison is an exact identity test, not a tolerance question: an
    // unchanged target must not restart the ramp.
    fn retarget(&mut self, document: &Document) {
        let (audible, gain, target) = track_stage_parameters(document, self.track, self.channels);
        let mix = document.track_mix(self.track);
        let pan_law = document.audio_mix.pan_law;
        let unchanged = self.audible == audible
            && self.gain_tenth_db == mix.gain_tenth_db
            && self.pan_percent == mix.pan_percent
            && self.gain_curve == mix.gain_curve
            && self.pan_curve == mix.pan_curve
            && self.pan_law == pan_law;
        self.audible = audible;
        self.gain = gain;
        self.target = target;
        self.gain_tenth_db = mix.gain_tenth_db;
        self.pan_percent = mix.pan_percent;
        self.pan_law = pan_law;
        self.gain_curve = mix.gain_curve;
        self.pan_curve = mix.pan_curve;
        // AU4 §3.1 rule 46: unconditional, exactly as `AudioEffectNode::retarget`
        // bumps `parameter_epoch`.
        self.curve_epoch = self.curve_epoch.wrapping_add(1);
        // AU4 §0 E24: see [`GainRamp::retarget`] — the unchanged arm leaves the
        // ramp running rather than forcing it settled at a stale `current`.
        if !unchanged {
            self.ramp_start = self.current;
            self.ramp_index = 0;
        }
    }

    /// AU4 §3.3 rule 56: the automated per-channel target at one absolute input
    /// sample, plus the scalar gain channels >= 2 read.
    ///
    /// Rule 43 pins the multiply order: `track_stage_parameters` returns
    /// `[gain * pan[0], gain * pan[1]]`, so the automated arm composes
    /// `gain * pan[ch]` in that order — never `pan * gain`, never a fused
    /// three-way product.
    fn automated_target(&mut self, project_sample: u64) -> (f32, [f32; 2]) {
        if !self.audible {
            return (0.0, [0.0, 0.0]);
        }
        let frame = automation_frame(
            TimeCode::ZERO,
            project_sample,
            self.sample_rate,
            self.project_fps,
        );
        let epoch = self.curve_epoch;
        let gain_step = automation_step(
            self.gain_curve.as_ref(),
            i64::from(self.gain_tenth_db),
            TimeCode::ZERO,
            project_sample,
            self.sample_rate,
            self.project_fps,
        );
        let gain_anchors = self.gain_memo.anchors(frame, epoch, || {
            (db_gain(gain_step.v0), db_gain(gain_step.v1))
        });
        let gain = automated_gain(&gain_step, gain_anchors);
        if self.channels < 2 {
            return (gain, [gain, gain]);
        }
        let law = self.pan_law;
        let pan_step = automation_step(
            self.pan_curve.as_ref(),
            i64::from(self.pan_percent),
            TimeCode::ZERO,
            project_sample,
            self.sample_rate,
            self.project_fps,
        );
        let pan_anchors = self.pan_memo.anchors(frame, epoch, || {
            (
                pan_channel_ratios(law, pan_percent_of(pan_step.v0)),
                pan_channel_ratios(law, pan_percent_of(pan_step.v1)),
            )
        });
        let pan = automated_pan(&pan_step, pan_anchors, law);
        (gain, [gain * pan[0], gain * pan[1]])
    }

    /// AU4 §3.3 rules 57 and 58: the automated value is the *target*, the live
    /// ramp runs on top of it, and `current` records the value last applied.
    fn advance_automated(&mut self, target: [f32; 2], ramp_frames: usize) {
        if self.ramp_index >= ramp_frames {
            self.current = target;
            return;
        }
        // The ramp position is an integer sample-frame index converted once per frame.
        #[allow(clippy::cast_precision_loss)]
        let progress = (self.ramp_index + 1) as f32 / ramp_frames as f32;
        for (channel, current) in self.current.iter_mut().enumerate() {
            *current =
                self.ramp_start[channel] + (target[channel] - self.ramp_start[channel]) * progress;
        }
        self.ramp_index += 1;
        if self.ramp_index >= ramp_frames {
            self.current = target;
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
    ///
    /// AU4 §3.3 rule 60: on a **curve-bearing** stage the completion assignment
    /// becomes `automated_target(n_end)` — the automated value at the last
    /// output frame of the silent chunk — so rule 58's definition of `current`
    /// holds across gaps as well as across audible chunks. Parking `current` at
    /// the scalar during silence would make a live mix edit made in a gap ramp
    /// the first audible chunk of the next clip from a stale value: exactly the
    /// click rule 58 exists to prevent, moved into the gap. The silent arm still
    /// writes **no samples**; only `current` moves. `start_sample` is the
    /// chunk's absolute input sample index (AU4 §0 E22).
    // The ramp position is an integer sample-frame index converted once.
    #[allow(clippy::cast_precision_loss)]
    fn skip_ramp(&mut self, start_sample: u64, frames: usize, ramp_frames: usize) {
        if !self.automated() {
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
            return;
        }
        let last = u64::try_from(frames.max(1).saturating_sub(1)).unwrap_or(u64::MAX);
        let (_, target) = self.automated_target(start_sample.saturating_add(last));
        if self.ramp_index >= ramp_frames {
            self.current = target;
            return;
        }
        let index = self.ramp_index.saturating_add(frames);
        if index >= ramp_frames {
            self.ramp_index = ramp_frames;
            self.current = target;
            return;
        }
        let progress = index as f32 / ramp_frames as f32;
        for (channel, current) in self.current.iter_mut().enumerate() {
            *current =
                self.ramp_start[channel] + (target[channel] - self.ramp_start[channel]) * progress;
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
        ramp_frames: usize,
        start_sample: u64,
    ) {
        let channels = self.channels.max(1);
        // AU4 §3.3 rule 56: the automated arm, walked per source frame. The two
        // neutral fast paths below are taken **only** when both curves are
        // `None`, so a curve-free document stays bit-identical (A13).
        if self.automated() {
            // AU1 §3.1 is not a fast path but an invariant: a **gated** track
            // writes exact zeros. `sample * 0.0` is `-0.0` for a negative
            // sample, which is a different bit pattern from `fill(0.0)`, so a
            // silenced automated track would not be byte-identical to a
            // silenced un-automated one (AU4 §0 E23).
            if !self.audible && self.ramp_index >= ramp_frames {
                self.current = [0.0, 0.0];
                destination.fill(0.0);
                return;
            }
            for (index, (out_frame, source_frame)) in destination
                .chunks_mut(channels)
                .zip(source.chunks(channels))
                .enumerate()
            {
                let project_sample =
                    start_sample.saturating_add(u64::try_from(index).unwrap_or(u64::MAX));
                let (wide, target) = self.automated_target(project_sample);
                self.advance_automated(target, ramp_frames);
                for (channel, (out, sample)) in out_frame.iter_mut().zip(source_frame).enumerate() {
                    *out = sample
                        * if channel < 2 {
                            self.current[channel]
                        } else {
                            wide
                        };
                }
            }
            return;
        }
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
    /// AU2 §5.6: the master fader and the master chain, run on the summed mix.
    master: AudioMasterRuntime,
    routed_tracks: HashSet<TrackId>,
    /// AU2 §3.6: the unrouted path is padded to the bus stage's latency, so it
    /// reaches the master sum aligned with every bus.
    unrouted_delay: DelayLine,
    /// AU2 §3.6: `stage_latency_frames(L_bus, rate)`, computed once when the
    /// processor is built and held for its life.
    bus_stage_frames: usize,
    /// AU2 §5.6: `stage_latency_frames(L_master, rate)`, likewise fixed.
    master_stage_frames: usize,
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
            .map(|track| {
                TrackStageRuntime::new(document, *track, channels, ramp_frames, sample_rate)
            })
            .collect();
        let scratch = track_order.iter().map(|_| Vec::new()).collect();
        // AU2 §3.6: latency is derived from the document once, here, and held
        // constant for this processor's life. Zero for every pre-AU2 document.
        let lookahead = document.audio_mix.lookahead_milliseconds();
        let bus_stage_frames = stage_latency_frames(lookahead.bus_stage, sample_rate);
        let master_stage_frames = stage_latency_frames(lookahead.master_stage, sample_rate);
        Self {
            track_order,
            stages,
            staged: scratch,
            buses: document
                .audio_mix
                .buses
                .iter()
                .map(|bus| {
                    AudioBusRuntime::new(
                        bus,
                        channels,
                        sample_rate,
                        document.fps,
                        ramp_frames,
                        bus_stage_frames,
                    )
                })
                .collect(),
            master: AudioMasterRuntime::new(
                &document.audio_mix.master,
                channels,
                sample_rate,
                document.fps,
                ramp_frames,
                master_stage_frames,
            ),
            routed_tracks,
            unrouted_delay: DelayLine::new(bus_stage_frames, channels),
            bus_stage_frames,
            master_stage_frames,
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

    /// AU2 §3.6: `stage_latency_frames(L_bus, rate)` for this processor — the
    /// leading sample frames a bus stem carries before the requested range.
    pub(crate) const fn bus_stage_frames(&self) -> usize {
        self.bus_stage_frames
    }

    /// AU2 §5.8: apply a document that may differ in track mix, bus chains,
    /// the master chain, or the pan law, without stopping playback.
    ///
    /// The caller guarantees `AudioMix::lookahead_milliseconds()` is unchanged
    /// (the app predicate refuses the live path otherwise and the worker
    /// re-cues defensively), because `L` is fixed for a processor's life.
    ///
    /// A chain whose **structure** — the ordered list of `(effect id, effect
    /// name, static lookahead)` — is unchanged is retargeted in place, which
    /// preserves filter memories, envelopes, delay lines, and RMS windows and
    /// is audible on the next sample frame (A13). Any other change rebuilds
    /// just that chain and accepts a momentary discontinuity.
    pub(crate) fn update_audio_mix(&mut self, document: &Document) {
        let channels = self.channels;
        let ramp_frames = self.ramp_frames;
        let sample_rate = self.sample_rate;
        let project_fps = self.project_fps;
        // 1. Track stages, which now also fold the document's pan law.
        for stage in &mut self.stages {
            stage.retarget(document);
        }
        // 2. Routing, recomputed on every update.
        self.routed_tracks = document
            .audio_mix
            .buses
            .iter()
            .flat_map(|bus| bus.tracks.iter().copied())
            .collect();
        // 3. Buses, in the new document's order. A bus that disappears takes
        //    its runtime with it; a bus that arrives gets a fresh one.
        //
        //    The unrouted path's delay line is deliberately *not* cleared here,
        //    so when a track moves onto a new bus the old path's `L_bus` tail
        //    drains while the new chain's line fills and the join has no gap
        //    (AU2 §5.8, seamless bus creation).
        let bus_stage_frames = self.bus_stage_frames;
        let mut existing = std::mem::take(&mut self.buses);
        self.buses = document
            .audio_mix
            .buses
            .iter()
            .map(|bus| {
                let matched = existing
                    .iter()
                    .position(|runtime| runtime.id == bus.id)
                    .map(|index| {
                        (
                            index,
                            chain_structure_matches(&existing[index].effects, &bus.effects),
                        )
                    });
                match matched {
                    Some((index, true)) => {
                        let mut runtime = existing.remove(index);
                        runtime.retarget(bus);
                        runtime
                    }
                    // 5. The fader is continuous across a chain rebuild: only
                    //    the chain's own state resets.
                    Some((index, false)) => {
                        let gain = existing.remove(index).gain;
                        let mut runtime = AudioBusRuntime::new(
                            bus,
                            channels,
                            sample_rate,
                            project_fps,
                            ramp_frames,
                            bus_stage_frames,
                        );
                        runtime.gain = gain;
                        runtime
                            .gain
                            .retarget(bus.gain_tenth_db, bus.gain_curve.as_ref());
                        runtime
                    }
                    None => AudioBusRuntime::new(
                        bus,
                        channels,
                        sample_rate,
                        project_fps,
                        ramp_frames,
                        bus_stage_frames,
                    ),
                }
            })
            .collect();
        // 4. The master chain, by the same rule.
        if chain_structure_matches(&self.master.effects, &document.audio_mix.master.effects) {
            self.master.retarget(&document.audio_mix.master);
        } else {
            let mut master = AudioMasterRuntime::new(
                &document.audio_mix.master,
                channels,
                sample_rate,
                project_fps,
                ramp_frames,
                self.master_stage_frames,
            );
            master.gain = std::mem::replace(
                &mut self.master.gain,
                GainRamp::settled(0, None, ramp_frames, sample_rate, project_fps),
            );
            master.gain.retarget(
                document.audio_mix.master.gain_tenth_db,
                document.audio_mix.master.gain_curve.as_ref(),
            );
            self.master = master;
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

    #[allow(clippy::too_many_lines)]
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
                self.stages[index].skip_ramp(start_sample, sample_frames, ramp_frames);
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
                    ramp_frames,
                    start_sample,
                );
                staged[length..].fill(0.0);
            }
            if let Some(meters) = &self.meters {
                meters.record_track(track, &self.staged[index], channels);
            }
        }

        // AU2 §3.1/§3.6: the unrouted path is a plain delay to `L_bus`, so an
        // unrouted track stays aligned with every bus. The line is a no-op when
        // no chain declares lookahead, which is every pre-AU2 document.
        let mut master = vec![0.0_f32; sample_count];
        for (index, track) in self.track_order.iter().enumerate() {
            if !self.routed_tracks.contains(track) && track_buffers.contains_key(track) {
                add_signal(&mut master, &self.staged[index]);
            }
        }
        self.unrouted_delay.process_buffer(&mut master, channels);

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
            // AU2 §5.6: the bus fader sits after the chain and before the pad,
            // so the bus stem, the bus meter, and the master sum all read the
            // faded signal. Settled at unity it is an exact pass-through.
            bus.gain
                .apply(&mut signal, channels, ramp_frames, start_sample);
            // AU2 §3.6: the alignment pad brings this chain up to `L_bus`, so
            // the bus stem, the bus meter, and the master sum all see the same
            // stage latency whatever the chain's own split.
            bus.pad.process_buffer(&mut signal, channels);
            // AU2 §3.8: once per chunk with overwrite semantics, and never
            // during seek preroll, where `meters` is `None`.
            for effect in &mut bus.effects {
                if let Some(reduction) = effect.take_gain_reduction()
                    && let Some(meters) = &self.meters
                {
                    meters.record_gain_reduction(
                        AudioChain::Bus(bus.id),
                        effect.effect.id,
                        reduction,
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

        // AU2 §5.6: master sum -> master gain -> master effects in order ->
        // alignment pad to `L_master`. The callers' +/-1.0 clamp and the master
        // meter stay exactly where they were, after this.
        {
            let sample_rate = self.sample_rate;
            let project_fps = self.project_fps;
            let frame_channels = self.channels;
            let runtime = &mut self.master;
            runtime
                .gain
                .apply(&mut master, channels, ramp_frames, start_sample);
            if !runtime.effects.is_empty() {
                // The master carries no sidechain, so `audio_ducking` is
                // rejected on it (AU2 §5.4) and the tap reads exact zeros.
                runtime.silence.clear();
                runtime.silence.resize(frame_channels.max(1), 0.0);
                for frame in 0..sample_frames {
                    let start = frame * frame_channels;
                    let end = start + frame_channels;
                    let project_at = samples_to_frame(
                        start_sample.saturating_add(u64::try_from(frame).unwrap_or(u64::MAX)),
                        sample_rate,
                        project_fps,
                    );
                    for effect in &mut runtime.effects {
                        effect.process_frame(
                            &mut master[start..end],
                            &runtime.silence,
                            project_at,
                            sample_rate,
                        );
                    }
                }
            }
            runtime.pad.process_buffer(&mut master, channels);
            // AU2 §3.8: `AudioChain::Master` slots, once per chunk with
            // overwrite semantics and never during seek preroll.
            for effect in &mut runtime.effects {
                if let Some(reduction) = effect.take_gain_reduction()
                    && let Some(meters) = &self.meters
                {
                    meters.record_gain_reduction(AudioChain::Master, effect.effect.id, reduction);
                }
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

/// AU2 §2.2: one parameter read once when the runtime is built, ignoring any
/// curve. Every fallback here equals the descriptor neutral (pinned by A3).
fn static_audio_value(effect: &Effect, name: &str, neutral: i64) -> i64 {
    effect.static_integer_parameter(name).unwrap_or(neutral)
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

/// AU1 §3.1's one-pole envelope, unchanged in behaviour.
///
/// AU2 §3.3 renames the two time arguments for what the function actually
/// selects: the first applies while the gain is *falling* toward its target and
/// the second while it is rising. A compressor passes
/// `(attack, release)`; a gate passes `(release, attack)`, because a gate's
/// falling gain is its release (§3.4).
#[allow(clippy::cast_precision_loss)]
fn smooth_gain(
    current: &mut f32,
    target: f32,
    falling_milliseconds: i64,
    rising_milliseconds: i64,
    sample_rate: u32,
) {
    let milliseconds = if target < *current {
        falling_milliseconds
    } else {
        rising_milliseconds
    }
    .max(1) as f32;
    let samples = milliseconds * sample_rate.max(1) as f32 / 1_000.0;
    let coefficient = (-1.0 / samples.max(1.0)).exp();
    *current = coefficient * *current + (1.0 - coefficient) * target;
}

struct AudioMixSource {
    track: TrackId,
    /// AU4 §3.8 rule 73: the clip this segment came from, so a live shaping
    /// update can match the segment layout before rebuilding in place.
    clip: ClipId,
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

/// AU1 §2.2 / AU2 §5.6: preroll is a property of stateful chain nodes, not of
/// the stateless track stage, so a document carrying only track-mix entries
/// seeks straight to its target.
///
/// AU2 §5.6 widens the predicate to the master chain: a document with no buses
/// and one master true-peak limiter — the first document the "+ Effect" menu on
/// the master produces — has stateful nodes and must warm them.
fn needs_seek_preroll(document: &Document, project_from: TimeCode) -> bool {
    (!document.audio_mix.buses.is_empty() || !document.audio_mix.master.effects.is_empty())
        && project_from > TimeCode::ZERO
}

struct AudioMixer {
    sources: Vec<AudioMixSource>,
    output_channels: usize,
    cursor_sample: u64,
    end_sample: u64,
    /// AU2 §3.7: the processor's input-to-output delay in sample frames, so
    /// callers can convert the input cursor to the output position.
    latency: u64,
    meter: Option<Arc<MeterState>>,
    processor: AudioMixProcessor,
    /// AU4 §3.8 rule 73: the two inputs `open` derived the segment list from,
    /// so a live shaping update can re-derive exactly the same list.
    decode_from: TimeCode,
    output_rate: u32,
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
                clip: segment.clip,
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
        // AU2 §3.7: the processor holds `latency` sample frames of the mix, so
        // the mixer runs `latency` frames past the programme end to flush them
        // and discards `latency` frames past the seek target to fill them. Both
        // are zero for every pre-AU2 document, which is the point.
        let latency = u64::try_from(graph_latency_frames(
            &document.audio_mix.lookahead_milliseconds(),
            output_rate,
        ))
        .unwrap_or(0);
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
            end_sample: frame_to_samples(project_end, output_rate, document.fps)
                .saturating_add(latency),
            latency,
            meter: None,
            decode_from,
            output_rate,
        };
        // AU2 §3.7: unconditional, including a seek to zero, where the loop
        // body only executes at all once a chain declares lookahead.
        let discard_to = target_sample.saturating_add(latency);
        while mixer.cursor_sample < discard_to {
            let remaining = discard_to - mixer.cursor_sample;
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

    /// AU2 §3.7: the processor's input-to-output latency in sample frames.
    /// `cursor_sample` counts *input* frames consumed, so the newest **output**
    /// frame the mixer has produced is `cursor_sample − latency_frames()`.
    /// Zero for every document whose chains declare no lookahead.
    const fn latency_frames(&self) -> u64 {
        self.latency
    }

    /// AU2 §3.7: the project sample of the newest output frame produced —
    /// the position the preroll aligned to `frame_to_samples(project_from)`.
    const fn output_position_samples(&self) -> u64 {
        self.cursor_sample.saturating_sub(self.latency_frames())
    }

    /// AU1 §4.1: install the peak table once preroll has finished.
    fn attach_mix_meters(&mut self, meters: Arc<MixMeters>) {
        self.processor.attach_meters(Some(meters));
    }

    /// AU2 §5.8: apply a new audio mix — track stages, bus chains, the master
    /// chain, and the pan law — to a running mixer.
    fn update_audio_mix(&mut self, document: &Document) {
        self.processor.update_audio_mix(document);
    }

    /// AU4 §3.8 rule 73: rebuild each source's [`ClipAudioShaping`] **in
    /// place** from a new document, or refuse.
    ///
    /// The `chain_structure_matches` idiom one level over: the layout matches
    /// when the source count, the `clip` ids in order, every segment's asset
    /// path, and every segment's project and source sample bounds are
    /// unchanged. `false` asks the caller to fall back to stop-and-re-cue,
    /// exactly as the chain path already does.
    /// Neither `SetClipAudio` nor `SetClipGainEnvelope` can change a clip
    /// boundary, an asset, or the declared lookahead (rule 74), so a `false`
    /// here means the batch carried something else.
    ///
    /// Rule 76: this runs on the worker thread, not the audio callback, so the
    /// per-source `Vec<Keyframe>` clone is not a realtime allocation.
    fn update_clip_shaping(&mut self, document: &Document) -> bool {
        let output_rate = self.output_rate;
        let project_end = document.duration;
        let Ok(segments) = timeline_audio_segments(document, self.decode_from..project_end) else {
            return false;
        };
        if segments.len() != self.sources.len() {
            return false;
        }
        let mut rebuilt = Vec::with_capacity(segments.len());
        for (segment, source) in segments.iter().zip(&self.sources) {
            if segment.clip != source.clip || segment.track != source.track {
                return false;
            }
            let Some(clip) = document.clip(segment.clip) else {
                return false;
            };
            let Some(asset) = document.asset(segment.asset) else {
                return false;
            };
            // Defence in depth: rule 74 keeps a relink off the live path, and
            // the worker does not verify the app's predicate, so the layout
            // check compares the decoded file itself rather than trusting it.
            if asset.path != source.path {
                return false;
            }
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
            if project_sample_start != source.project_sample_start
                || project_sample_end != source.project_sample_end
                || source_sample_start != source.source_sample_start
                || source_sample_end != source.source_sample_end
            {
                return false;
            }
            let Ok(clip_duration) = document.clip_duration(clip) else {
                return false;
            };
            rebuilt.push((clip, clip_duration));
        }
        for (source, (clip, clip_duration)) in self.sources.iter_mut().zip(rebuilt) {
            source.shaping.rebuild(clip, clip_duration);
        }
        true
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
/// AU3 §3.9: every post-clamp chunk taken from the mixer is pushed to the
/// live `meter` (its first two channels) as it is taken — up to
/// `LIVE_FILL_MILLISECONDS` ahead of the loudspeaker — and the worker
/// publishes by audible position.
///
/// Device-free so the fill target is testable without an audio device.
/// Returns `false` when the mixer has no more audio to deliver.
fn fill_ring(
    producer: &mut Producer<f32>,
    pending: &mut Vec<f32>,
    pending_index: &mut usize,
    mixer: &mut AudioMixer,
    target_samples: usize,
    meter: &mut LiveLoudnessMeter,
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
            meter.push_chunk(&chunk, mixer.output_channels)?;
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
    sample_rate: u32,
    channels: u16,
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
        // AU3 §3.9: the initial fill is the worker's, through `fill(meter)`,
        // once it has matched its live meter to this stream's rate and layout.
        Ok(Self {
            stream,
            producer,
            mixer,
            pending: Vec::new(),
            pending_index: 0,
            target_samples,
            sample_rate,
            channels,
            error_flag,
        })
    }

    /// The device stream's rate.
    pub(crate) const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The device stream's channel count.
    pub(crate) const fn channels(&self) -> u16 {
        self.channels
    }

    /// AU3 §3.9: the absolute project sample (at the device rate) the mixer
    /// has rendered — and the live meter has been fed — up to.
    ///
    /// This is the *output* position, not the mixer's input cursor: AU2 §3.7's
    /// processor holds `latency_frames()` sample frames of the mix, and
    /// `AudioMixer::open` prerolls the input cursor that far past the seek
    /// target so the first frame *out* is `frame_to_samples(project_from)` —
    /// which is exactly what the clock's `position_samples` starts at. Keying a
    /// mid-play `reset_loudness` from the input cursor would put the ring
    /// origin one graph latency ahead of the audio it keys.
    pub(crate) const fn fed_position_samples(&self) -> u64 {
        self.mixer.output_position_samples()
    }

    /// AU2 §5.8: apply a new audio mix without stopping the stream.
    pub(crate) fn update_audio_mix(&mut self, document: &Document) {
        self.mixer.update_audio_mix(document);
    }

    /// AU4 §3.8 rule 73: apply new clip audio shaping without stopping the
    /// stream. `false` asks the worker to fall back to stop-and-re-cue.
    pub(crate) fn update_clip_shaping(&mut self, document: &Document) -> bool {
        self.mixer.update_clip_shaping(document)
    }

    /// AU2 §5.8: install a rebuilt peak table on a running stream, in the same
    /// call the worker publishes it.
    pub(crate) fn attach_mix_meters(&mut self, meters: Arc<MixMeters>) {
        self.mixer.attach_mix_meters(meters);
    }

    pub(crate) fn play(&self) -> Result<(), MediaError> {
        self.stream.play().map_err(backend)
    }

    pub(crate) fn pause(&self) -> Result<(), MediaError> {
        self.stream.pause().map_err(backend)
    }

    /// AU1 §5.3: top the ring up to the live target, metering as it goes
    /// (AU3 §3.9).
    pub(crate) fn fill(&mut self, meter: &mut LiveLoudnessMeter) -> Result<(), MediaError> {
        fill_ring(
            &mut self.producer,
            &mut self.pending,
            &mut self.pending_index,
            &mut self.mixer,
            self.target_samples,
            meter,
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

pub(crate) struct AudioDecoder {
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
    pub(crate) fn open(
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

    pub(crate) fn next_chunk(&mut self) -> Result<Option<Vec<f32>>, MediaError> {
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
                ..AudioMix::default()
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
                gain_tenth_db: 0,
                effects: vec![gain],
                ducking_sidechain_tracks: Vec::new(),
                gain_curve: None,
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
                    gain_tenth_db: 0,
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
                    gain_curve: None,
                },
                AudioBus {
                    id: AudioBusId(2),
                    name: "Sidechain monitor".to_owned(),
                    tracks: vec![TrackId(2)],
                    gain_tenth_db: 0,
                    effects: vec![audio_effect(4, "audio_gain", &[("gain_tenth_db", -600)])],
                    ducking_sidechain_tracks: Vec::new(),
                    gain_curve: None,
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
                gain_tenth_db: 0,
                effects: vec![audio_effect(
                    1,
                    "audio_limiter",
                    &[("ceiling_tenth_db", -10)],
                )],
                ducking_sidechain_tracks: Vec::new(),
                gain_curve: None,
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
            loudness_normalization: None,
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
                    audio_gain_curve: None,
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
            gain_curve: None,
            pan_curve: None,
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
            loudness_normalization: None,
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
            gain_tenth_db: 0,
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
            gain_curve: None,
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
            gain_tenth_db: 0,
            effects: vec![audio_effect(2, "audio_gain", &[("gain_tenth_db", -600)])],
            ducking_sidechain_tracks: Vec::new(),
            gain_curve: None,
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
        processor.update_audio_mix(&muted);
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
        processor.update_audio_mix(&muted);
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
        partial.update_audio_mix(&muted);
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
        processor.update_audio_mix(&muted);
        // 100 of the 240 ramp frames elapse before the editor changes its mind.
        let falling = processor
            .mix_chunk(&HashMap::from([(TrackId(1), vec![1.0_f32; 100])]), 0, 100)
            .unwrap();
        let start = 1.0 - 100.0 / 240.0;
        assert_close(falling[99], start);

        processor.update_audio_mix(&document);
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
            gain_tenth_db: 0,
            effects: vec![audio_effect(1, "audio_gain", &[("gain_tenth_db", -60)])],
            ducking_sidechain_tracks: Vec::new(),
            gain_curve: None,
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
        // AU3 §3.9: a scratch live meter; the fill pushes every chunk to it.
        let mut scratch_meter = LiveLoudnessMeter::new(48_000, 2).unwrap();

        assert!(
            fill_ring(
                &mut producer,
                &mut pending,
                &mut pending_index,
                &mut mixer,
                target,
                &mut scratch_meter,
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
                target,
                &mut scratch_meter,
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
                &mut scratch_meter,
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
            gain_tenth_db: 0,
            effects: vec![audio_effect(1, "audio_gain", &[("gain_tenth_db", -60)])],
            ducking_sidechain_tracks: Vec::new(),
            gain_curve: None,
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
                    gain_tenth_db: 0,
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
                    gain_curve: None,
                }],
                ..AudioMix::default()
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
            audio_gain_curve: None,
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

    // ---------------------------------------------------------------------
    // AU2 Part A (§3.9, A3, A5-A17, A19)
    // ---------------------------------------------------------------------

    /// One bus carrying `effects`, fed by track 1 with track 2 as its sidechain.
    fn chain_bus(effects: Vec<Effect>) -> AudioBus {
        AudioBus {
            id: AudioBusId(1),
            name: "Chain".to_owned(),
            tracks: vec![TrackId(1)],
            gain_tenth_db: 0,
            effects,
            ducking_sidechain_tracks: vec![TrackId(2)],
            gain_curve: None,
        }
    }

    fn chain_document(effects: Vec<Effect>) -> Document {
        processor_document(Rational::new(10, 1).unwrap(), 20, vec![chain_bus(effects)])
    }

    /// Mix one interleaved buffer through a single-bus document and return the
    /// **bus stem**, so the sidechain track — which is unrouted and would
    /// otherwise reach the master directly — cannot contaminate the reading.
    /// This is the "node level" A6 and A26 ask for.
    fn run_chain(
        document: &Document,
        rate: u32,
        channels: usize,
        input: &[f32],
        sidechain: Option<&[f32]>,
    ) -> Vec<f32> {
        let frames = input.len() / channels.max(1);
        let mut tracks = HashMap::from([(TrackId(1), input.to_vec())]);
        if let Some(sidechain) = sidechain {
            tracks.insert(TrackId(2), sidechain.to_vec());
        }
        let mut stems = AudioMixProcessor::new(document, rate, channels, None)
            .mix_chunk_with_stems(&tracks, 0, frames)
            .expect("the chunk should mix");
        stems.buses.remove(0)
    }

    /// The same mix, read at the master sum, so the unrouted path's delay and
    /// the master summation are exercised too.
    fn run_chain_master(
        document: &Document,
        rate: u32,
        channels: usize,
        input: &[f32],
    ) -> Vec<f32> {
        let frames = input.len() / channels.max(1);
        AudioMixProcessor::new(document, rate, channels, None)
            .mix_chunk(&HashMap::from([(TrackId(1), input.to_vec())]), 0, frames)
            .expect("the chunk should mix")
    }

    fn run_effect(effect: Effect, rate: u32, input: &[f32]) -> Vec<f32> {
        run_chain(&chain_document(vec![effect]), rate, 1, input, None)
    }

    /// AU2 §3.6: the latency tests' shared helper.
    fn latency_offset(document: &Document, rate: u32) -> usize {
        graph_latency_frames(&document.audio_mix.lookahead_milliseconds(), rate)
    }

    /// AU3 §7 B7: `process_buffer_static` is length-preserving, latency-
    /// compensated, a bit-exact identity on an under-ceiling buffer, and
    /// returns an error rather than panicking when the output length would
    /// differ from the input's.
    #[test]
    fn au3_process_buffer_static_preserves_length_and_alignment() {
        // AU2 §3.5: the node's total delay is exactly `Lh`, which is 240
        // frames for the export step's 5 ms lookahead at 48 kHz.
        assert_eq!(stage_latency_frames(5, 48_000), 240);
        let limiter = audio_effect(
            1,
            "audio_true_peak_limiter",
            &[
                ("ceiling_tenth_db", -30),
                ("lookahead_milliseconds", 5),
                ("release_milliseconds", 50),
                ("true_peak", 1),
            ],
        );
        // A 997 Hz tone at 0.5 with a 0.5 impulse on the very first frame:
        // every sample is under the node's `10^(-30/200) = 0.708` ceiling, so
        // the limiter is an identity, and the impulse's position is what a
        // wrong `Lh` drop would move.
        let mut input = Vec::new();
        for sample in tone(997.0, 0.5, 48_000, 48_000) {
            input.push(sample);
            input.push(sample);
        }
        input[0] = 0.5;
        input[1] = 0.5;
        let output = process_buffer_static(&limiter, 48_000, 2, &input)
            .expect("an aligned buffer must run through a static node");
        assert_eq!(output.len(), input.len());
        // The alignment claim, asserted independently of the identity below:
        // the tone's own frame 0 is `sin(0) = 0`, so only the impulse can put
        // 0.5 on the first frame. A buffer still carrying its `Lh` frames of
        // latency would read the node's silent lookahead priming here.
        assert!(
            (output[0] - 0.5).abs() < f32::EPSILON && (output[1] - 0.5).abs() < f32::EPSILON,
            "the first `Lh` = 240 output frames must have been dropped, leaving the impulse on \
             frame 0: {} {}",
            output[0],
            output[1]
        );
        assert_eq!(
            output, input,
            "AU2 A27: an under-ceiling buffer passes through the limiter bit-identically, and it \
             does so in place — the first `Lh` output frames were dropped, so the impulse is \
             still on frame 0"
        );
    }

    /// AU3 §7 B7 / §5.6's F11 bullet: a length change is an error, never a
    /// panic. An input that is not aligned to the channel count is the
    /// reachable case.
    #[test]
    fn au3_process_buffer_static_refuses_a_length_change_without_panicking() {
        let limiter = audio_effect(
            1,
            "audio_true_peak_limiter",
            &[("ceiling_tenth_db", -30), ("lookahead_milliseconds", 5)],
        );
        let unaligned = vec![0.25_f32; 5];
        let error = process_buffer_static(&limiter, 48_000, 2, &unaligned)
            .expect_err("an unaligned buffer cannot come back the length it went in");
        match &error {
            MediaError::Backend(message) => {
                assert!(message.contains("changed its buffer length"), "{message}");
            }
            other => panic!("a length change must be a typed backend error: {other}"),
        }
        let error = process_buffer_static(&limiter, 48_000, 0, &unaligned)
            .expect_err("a zero-channel buffer has no frames to run");
        match &error {
            MediaError::Backend(message) => {
                assert!(message.contains("at least one channel"), "{message}");
            }
            other => panic!("a zero channel count must be a typed backend error: {other}"),
        }
    }

    #[allow(clippy::cast_precision_loss)]
    fn tone(frequency: f64, amplitude: f32, rate: u32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|frame| {
                let phase = 2.0 * std::f64::consts::PI * frequency * frame as f64 / f64::from(rate);
                #[allow(clippy::cast_possible_truncation)]
                let sample = (phase.sin() as f32) * amplitude;
                sample
            })
            .collect()
    }

    /// A deterministic xorshift buffer, never a negative zero.
    #[allow(clippy::cast_precision_loss)]
    fn pseudo_random_amplitude(frames: usize, amplitude: f32) -> Vec<f32> {
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        (0..frames)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let unit = (state >> 40) as f32 / 8_388_608.0 - 1.0;
                unit * amplitude
            })
            .collect()
    }

    fn pseudo_random(frames: usize) -> Vec<f32> {
        pseudo_random_amplitude(frames, 0.25)
    }

    /// A `Hold` curve on `bypass` that switches the node on at `flip_at`,
    /// which §2.2 rule 1 explicitly permits.
    fn un_bypassing(mut effect: Effect, flip_at: TimeCode) -> Effect {
        effect.keyframes.insert(
            "bypass".to_owned(),
            AutomationCurve {
                keyframes: vec![
                    Keyframe {
                        at: TimeCode::ZERO,
                        value: 1,
                        interpolation: KeyframeInterpolation::Hold,
                    },
                    Keyframe {
                        at: flip_at,
                        value: 0,
                        interpolation: KeyframeInterpolation::Hold,
                    },
                ],
            },
        );
        effect
    }

    fn rms(samples: &[f32]) -> f64 {
        let sum: f64 = samples
            .iter()
            .map(|sample| f64::from(*sample) * f64::from(*sample))
            .sum();
        #[allow(clippy::cast_precision_loss)]
        let count = samples.len().max(1) as f64;
        (sum / count).sqrt()
    }

    fn sample_peak(samples: &[f32]) -> f32 {
        samples
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0_f32, f32::max)
    }

    /// AU2 §3.9(a): the response of one node measured through the real mixer,
    /// as an RMS ratio over a whole number of periods in the last 0.5 s of a
    /// 2 s tone at 48 kHz.
    fn measured_response_db(effect: Effect, frequency: f64) -> f64 {
        let rate = 48_000_u32;
        let frames = 96_000_usize;
        let input = tone(frequency, 0.5, rate, frames);
        let output = run_effect(effect, rate, &input);
        let window = 72_000..frames;
        20.0 * (rms(&output[window.clone()]) / rms(&input[window])).log10()
    }

    fn assert_response(effect: Effect, frequency: f64, expected_db: f64, tolerance: f64) {
        let measured = measured_response_db(effect, frequency);
        assert!(
            (measured - expected_db).abs() <= tolerance,
            "response at {frequency} Hz was {measured} dB, expected {expected_db} +/- {tolerance}"
        );
    }

    fn parametric_eq(id: u64, parameters: &[(&str, i64)]) -> Effect {
        audio_effect(id, "audio_parametric_eq", parameters)
    }

    /// AU2 §7 item A3: every runtime neutral fallback equals its descriptor
    /// neutral, so an `Effect` with an empty `parameters` map is bit-identical
    /// to one carrying every parameter at its neutral.
    #[test]
    #[allow(clippy::float_cmp)]
    fn empty_audio_parameters_are_bit_identical_to_the_descriptor_neutrals() {
        let input = pseudo_random(2_000);
        let sidechain = tone(220.0, 0.8, 48_000, 2_000);
        for descriptor in kinewright_core::EFFECT_DESCRIPTORS
            .iter()
            .filter(|descriptor| kinewright_core::is_audio_effect(descriptor.name))
        {
            let neutrals = descriptor
                .parameters
                .iter()
                .map(|parameter| (parameter.name, parameter.neutral))
                .collect::<Vec<_>>();
            let empty = run_chain(
                &chain_document(vec![audio_effect(1, descriptor.name, &[])]),
                48_000,
                1,
                &input,
                Some(&sidechain),
            );
            let explicit = run_chain(
                &chain_document(vec![audio_effect(1, descriptor.name, &neutrals)]),
                48_000,
                1,
                &input,
                Some(&sidechain),
            );
            assert_eq!(
                empty, explicit,
                "{}'s runtime fallbacks differ from its descriptor neutrals",
                descriptor.name
            );
        }
    }

    /// AU2 §7 item A5 (exit-gate clause 1): the filter magnitude response
    /// matches the analytic transfer function at pinned frequencies.
    #[test]
    fn filter_magnitude_matches_the_analytic_transfer_function() {
        // Peaking: RBJ gives |H(f0)| = A^2 = 10^(G/20) exactly, digitally.
        assert_response(
            parametric_eq(
                1,
                &[
                    ("band1_hertz", 1_000),
                    ("band1_gain_tenth_db", 60),
                    ("band1_q_hundredths", 100),
                ],
            ),
            1_000.0,
            6.00,
            0.01,
        );

        // Shelves with S = 1 give A = 10^(G/40) at f0 — exactly half the dB
        // gain — and the full gain at DC or Nyquist.
        let low_shelf = || {
            parametric_eq(
                2,
                &[("low_shelf_hertz", 250), ("low_shelf_gain_tenth_db", 80)],
            )
        };
        assert_response(low_shelf(), 20.0, 8.00, 0.02);
        assert_response(low_shelf(), 250.0, 4.00, 0.02);
        assert_response(low_shelf(), 20_000.0, 0.00, 0.02);

        let high_shelf = || {
            parametric_eq(
                3,
                &[
                    ("high_shelf_hertz", 4_000),
                    ("high_shelf_gain_tenth_db", -80),
                ],
            )
        };
        assert_response(high_shelf(), 20_000.0, -8.00, 0.02);
        assert_response(high_shelf(), 4_000.0, -4.00, 0.02);
        assert_response(high_shelf(), 20.0, 0.00, 0.02);

        // High-pass: H(e^{jw0}) = jQ exactly for every fc and every rate, so
        // the corner pin is frequency-independent.
        for corner in [100_u32, 1_000, 8_000] {
            assert_response(
                parametric_eq(4, &[("high_pass_hertz", i64::from(corner))]),
                f64::from(corner),
                -3.0103,
                0.01,
            );
        }
        // The half-corner figure is not frequency-independent: the warped
        // closed form is r = tan(PI*f/fs)/tan(PI*fc/fs), |H| = r^2/sqrt(1+r^4),
        // which is -12.322 dB at fc = 1 kHz and -13.53 dB at fc = 8 kHz.
        assert_response(
            parametric_eq(5, &[("high_pass_hertz", 1_000)]),
            500.0,
            -12.32,
            0.05,
        );

        // AU2 §6.7: the exported analytic helper the Mixer's EQ well samples
        // reads the same figures from the same coefficient code.
        for (effect, frequency, expected) in [
            (
                parametric_eq(
                    1,
                    &[
                        ("band1_hertz", 1_000),
                        ("band1_gain_tenth_db", 60),
                        ("band1_q_hundredths", 100),
                    ],
                ),
                1_000.0,
                6.00,
            ),
            (low_shelf(), 250.0, 4.00),
            (high_shelf(), 4_000.0, -4.00),
            (
                parametric_eq(4, &[("high_pass_hertz", 1_000)]),
                1_000.0,
                -3.0103,
            ),
            (
                parametric_eq(5, &[("high_pass_hertz", 1_000)]),
                500.0,
                -12.322,
            ),
            (
                parametric_eq(6, &[("output_gain_tenth_db", -30)]),
                1_000.0,
                -3.0,
            ),
        ] {
            let analytic =
                crate::parametric_eq_magnitude_db(&effect, TimeCode::ZERO, frequency, 48_000);
            assert!(
                (analytic - expected).abs() <= 0.01,
                "the analytic helper read {analytic} dB at {frequency} Hz, expected {expected}"
            );
        }
    }

    /// AU2 §7 item A6: a neutral `audio_parametric_eq` is bit-identical to its
    /// input at node level, and a bypassed node of every kind is bit-identical
    /// to its delayed input.
    #[test]
    #[allow(clippy::float_cmp)]
    fn a_neutral_parametric_eq_and_every_bypassed_node_are_identities() {
        let input = pseudo_random(1_000);
        let neutral = run_effect(parametric_eq(1, &[]), 48_000, &input);
        assert_eq!(neutral, input, "an all-neutral parametric EQ must be exact");

        let sidechain = tone(220.0, 0.9, 48_000, 1_000);
        for (name, parameters) in bypass_sweep_settings() {
            let mut with_bypass = parameters.clone();
            with_bypass.push(("bypass", 1));
            let document = chain_document(vec![audio_effect(1, name, &with_bypass)]);
            let delay = latency_offset(&document, 48_000);
            let output = run_chain(&document, 48_000, 1, &input, Some(&sidechain));
            assert_eq!(
                &output[delay..],
                &input[..input.len() - delay],
                "a bypassed {name} must pass its delayed input through"
            );
            assert!(
                output[..delay].iter().all(|sample| *sample == 0.0),
                "a bypassed {name} must open with silence for its delay"
            );
        }
    }

    /// Non-neutral settings for every one of the eight audio node names.
    fn bypass_sweep_settings() -> Vec<(&'static str, Vec<(&'static str, i64)>)> {
        vec![
            ("audio_gain", vec![("gain_tenth_db", -60)]),
            ("audio_eq", vec![("low_gain_tenth_db", -240)]),
            (
                "audio_compressor",
                vec![
                    ("threshold_tenth_db", -200),
                    ("ratio_hundredths", 1_000),
                    ("attack_milliseconds", 1),
                    ("lookahead_milliseconds", 2),
                ],
            ),
            (
                "audio_ducking",
                vec![("threshold_tenth_db", -600), ("reduction_tenth_db", 200)],
            ),
            ("audio_limiter", vec![("ceiling_tenth_db", -120)]),
            (
                "audio_parametric_eq",
                vec![("band1_hertz", 1_000), ("band1_gain_tenth_db", 240)],
            ),
            (
                "audio_gate",
                vec![
                    ("threshold_tenth_db", 0),
                    ("ratio_hundredths", 2_000),
                    ("range_tenth_db", 800),
                    ("hold_milliseconds", 0),
                ],
            ),
            (
                "audio_true_peak_limiter",
                vec![
                    ("ceiling_tenth_db", -60),
                    ("lookahead_milliseconds", 1),
                    ("true_peak", 1),
                ],
            ),
        ]
    }

    /// AU2 §7 item A13 (R25): every name `is_audio_effect` accepts changes a
    /// full-scale tone at a non-neutral setting. This is the one assertion
    /// `process_frame`'s fall-through arm cannot pass by accident.
    #[test]
    fn every_audio_effect_kind_changes_a_tone() {
        let input = tone(1_000.0, 0.5, 48_000, 4_800);
        let sidechain = tone(220.0, 0.9, 48_000, 4_800);
        let mut covered = 0_usize;
        for (name, parameters) in bypass_sweep_settings() {
            assert!(
                kinewright_core::is_audio_effect(name),
                "{name} is no longer an audio effect"
            );
            covered += 1;
            let output = run_chain(
                &chain_document(vec![audio_effect(1, name, &parameters)]),
                48_000,
                1,
                &input,
                Some(&sidechain),
            );
            let difference = output
                .iter()
                .zip(&input)
                .map(|(output, input)| (output - input).abs())
                .fold(0.0_f32, f32::max);
            assert!(difference > 1.0e-3, "{name} left the tone unchanged");
        }
        let registered = kinewright_core::EFFECT_DESCRIPTORS
            .iter()
            .filter(|descriptor| kinewright_core::is_audio_effect(descriptor.name))
            .count();
        assert_eq!(covered, registered, "the sweep must cover every audio node");
    }

    /// AU2 §7 item A8: the compressor's static curve — below-threshold
    /// identity, 12 dB over at 4:1 giving 3 dB over, and the knee midpoint at
    /// `(1 - 1/R) * W/8`.
    #[test]
    #[allow(clippy::float_cmp)]
    fn the_compressor_static_curve_holds_its_pins() {
        let rate = 48_000_u32;
        let frames = 24_000_usize;
        let settled = frames - 1;
        let compressor = |knee: i64| {
            audio_effect(
                1,
                "audio_compressor",
                &[
                    ("threshold_tenth_db", -200),
                    ("ratio_hundredths", 400),
                    ("attack_milliseconds", 1),
                    ("release_milliseconds", 10),
                    ("knee_tenth_db", knee),
                ],
            )
        };

        // Below threshold the envelope never leaves one, so the node is exact.
        let quiet = vec![0.01_f32; frames];
        assert_eq!(run_effect(compressor(0), rate, &quiet), quiet);

        // 12 dB over threshold at 4:1 settles 3 dB over threshold.
        let over = vec![10.0_f32.powf(-8.0 / 20.0); frames];
        let compressed = run_effect(compressor(0), rate, &over);
        let expected = 10.0_f32.powf(-17.0 / 20.0);
        assert!(
            (compressed[settled] - expected).abs() <= 1.0e-3,
            "12 dB over at 4:1 settled at {} rather than {expected}",
            compressed[settled]
        );

        // At the knee midpoint the reduction is (1 - 1/R) * W/8 dB.
        let at_threshold = vec![0.1_f32; frames];
        let kneed = run_effect(compressor(120), rate, &at_threshold);
        let reduction = -20.0 * (kneed[settled] / at_threshold[settled]).log10();
        assert!(
            (reduction - 1.125).abs() <= 1.0e-2,
            "the knee midpoint reduced by {reduction} dB rather than 1.125"
        );
    }

    /// AU2 §7 item A9 (R24): an existing-shaped compressor document — no knee,
    /// no detector, no RMS window, no lookahead — is bit-identical to the AU1
    /// expression evaluated by an independent reference here.
    #[test]
    #[allow(clippy::float_cmp)]
    fn an_existing_shaped_compressor_document_is_bit_identical_to_au1() {
        let rate = 48_000_u32;
        let input = tone(220.0, 0.9, rate, 4_800);
        let effect = audio_effect(
            1,
            "audio_compressor",
            &[("threshold_tenth_db", -120), ("ratio_hundredths", 400)],
        );
        let output = run_effect(effect, rate, &input);

        // The AU1 gain computer and envelope, written out.
        let threshold_db = -12.0_f32;
        let ratio = 4.0_f32;
        let mut envelope = 1.0_f32;
        let expected = input
            .iter()
            .map(|sample| {
                let level_db = amplitude_db(sample.abs());
                let target = if level_db > threshold_db && ratio > 1.0 {
                    10.0_f32.powf(
                        ((threshold_db + (level_db - threshold_db) / ratio) - level_db) / 20.0,
                    )
                } else {
                    1.0
                };
                smooth_gain(&mut envelope, target, 10, 250, rate);
                sample * (envelope * 1.0)
            })
            .collect::<Vec<_>>();
        assert_eq!(output, expected);
    }

    /// AU2 §7 item A10: RMS versus peak detection on a burst, the gate's range
    /// floor, its two independent identities, and an exact hold count.
    #[test]
    #[allow(clippy::float_cmp)]
    fn rms_detection_and_the_gate_follow_their_contracts() {
        let rate = 48_000_u32;
        // A 5 ms burst inside 100 ms of silence: the peak detector sees full
        // scale, the 10 ms RMS window at most sqrt(240/480) of it.
        let mut burst = vec![0.0_f32; 4_800];
        for sample in &mut burst[2_000..2_240] {
            *sample = 1.0;
        }
        let detector = |mode: i64| {
            audio_effect(
                1,
                "audio_compressor",
                &[
                    ("threshold_tenth_db", -60),
                    ("ratio_hundredths", 400),
                    ("attack_milliseconds", 1),
                    ("detector", mode),
                    ("rms_window_milliseconds", 10),
                ],
            )
        };
        let peak_mode = sample_peak(&run_effect(detector(0), rate, &burst));
        let rms_mode = sample_peak(&run_effect(detector(1), rate, &burst));
        assert!(
            peak_mode < rms_mode,
            "peak detection ({peak_mode}) must reduce more than RMS ({rms_mode})"
        );

        // The gate's range floor: -60 dBFS at T = -30, R = 4:1 wants -90 dB of
        // expansion and stops at the -24 dB floor.
        let quiet = vec![0.001_f32; 48_000];
        let gate = |ratio: i64, range: i64| {
            audio_effect(
                1,
                "audio_gate",
                &[
                    ("threshold_tenth_db", -300),
                    ("ratio_hundredths", ratio),
                    ("range_tenth_db", range),
                    ("attack_milliseconds", 1),
                    ("hold_milliseconds", 0),
                    ("release_milliseconds", 10),
                ],
            )
        };
        let floored = run_effect(gate(400, 240), rate, &quiet);
        let settled = floored[47_999] / quiet[47_999];
        let expected = 10.0_f32.powf(-24.0 / 20.0);
        assert!(
            (settled - expected).abs() <= 1.0e-4,
            "the range floor settled at {settled} rather than {expected}"
        );

        // Either a 1:1 ratio or a zero range alone is an exact identity.
        assert_eq!(run_effect(gate(100, 240), rate, &quiet), quiet);
        assert_eq!(run_effect(gate(400, 0), rate, &quiet), quiet);

        // The hold count is exact: 10 ms at 48 kHz is 480 sample frames, so a
        // single loud frame keeps the gate open through frame 480 and no
        // further.
        let mut held = vec![0.001_f32; 4_800];
        held[0] = 1.0;
        let output = run_effect(
            audio_effect(
                1,
                "audio_gate",
                &[
                    ("threshold_tenth_db", -300),
                    ("ratio_hundredths", 400),
                    ("range_tenth_db", 240),
                    ("attack_milliseconds", 1),
                    ("hold_milliseconds", 10),
                    ("release_milliseconds", 10),
                ],
            ),
            rate,
            &held,
        );
        assert_eq!(output[480], 0.001, "the gate must hold through frame 480");
        assert!(
            output[481] < 0.001,
            "the gate must start closing at frame 481, got {}",
            output[481]
        );
    }

    /// AU2 A11: an independent 16x windowed-sinc interpolator, written here so
    /// the limiter is never verified with its own 4x estimator.
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    fn reference_true_peak(samples: &[f32], channels: usize) -> f32 {
        const OVERSAMPLE: usize = 16;
        const PHASE_TAPS: usize = 12;
        const TAPS: usize = OVERSAMPLE * PHASE_TAPS;
        let mut taps = [0.0_f64; TAPS];
        for (index, tap) in taps.iter_mut().enumerate() {
            let position = index as f64;
            let argument = (position - (TAPS as f64 - 1.0) / 2.0) / OVERSAMPLE as f64;
            let phase = 2.0 * std::f64::consts::PI * (position + 0.5) / TAPS as f64;
            let window = 0.42 - 0.5 * phase.cos() + 0.08 * (2.0 * phase).cos();
            *tap = (std::f64::consts::PI * argument).sin() / (std::f64::consts::PI * argument)
                * window;
        }
        for phase in 0..OVERSAMPLE {
            let sum: f64 = (0..PHASE_TAPS).map(|k| taps[OVERSAMPLE * k + phase]).sum();
            for k in 0..PHASE_TAPS {
                taps[OVERSAMPLE * k + phase] /= sum;
            }
        }
        let channels = channels.max(1);
        let frames = samples.len() / channels;
        let mut peak = 0.0_f64;
        for channel in 0..channels {
            for frame in 0..frames {
                let mut phases = [0.0_f64; OVERSAMPLE];
                for k in 0..PHASE_TAPS.min(frame + 1) {
                    let sample = f64::from(samples[(frame - k) * channels + channel]);
                    for (phase, accumulator) in phases.iter_mut().enumerate() {
                        *accumulator += taps[OVERSAMPLE * k + phase] * sample;
                    }
                }
                for value in phases {
                    peak = peak.max(value.abs());
                }
            }
        }
        peak as f32
    }

    /// AU2 §7 item A11 (exit-gate clause 2): the true-peak limiter's ceiling is
    /// measured on synthetic material with an independent 16x interpolator.
    #[test]
    #[allow(clippy::float_cmp, clippy::too_many_lines)]
    fn the_true_peak_limiter_holds_its_ceiling_on_synthetic_material() {
        let rate = 48_000_u32;
        let ceiling = 10.0_f32.powf(-1.0 / 20.0);
        // OPEN-4: this contract's Blackman windowed-sinc reads within about
        // 0.3 dB of a 16x reference, so the reference is allowed that much.
        let detector_headroom = 10.0_f32.powf(0.3 / 20.0);
        let limiter = |id: u64, lookahead: i64, true_peak: i64, release: i64, ceiling: i64| {
            audio_effect(
                id,
                "audio_true_peak_limiter",
                &[
                    ("ceiling_tenth_db", ceiling),
                    ("lookahead_milliseconds", lookahead),
                    ("release_milliseconds", release),
                    ("true_peak", true_peak),
                ],
            )
        };

        // An over-level tone, under both detectors.
        let loud = tone(997.0, 1.4, rate, 9_600);
        for mode in [0_i64, 1] {
            let output = run_effect(limiter(1, 5, mode, 50, -10), rate, &loud);
            assert!(
                sample_peak(&output) <= ceiling * (1.0 + 1.0e-4),
                "the sample peak {} passed the ceiling {ceiling} with true_peak {mode}",
                sample_peak(&output)
            );
            if mode == 1 {
                let measured = reference_true_peak(&output, 1);
                assert!(
                    measured <= ceiling * detector_headroom,
                    "the 16x true peak {measured} passed the ceiling {ceiling}"
                );
            }
        }

        // Broadband material, which is where the 4x Blackman windowed-sinc
        // under-reads hardest: a tone sits far inside OPEN-4's +0.3 dB budget,
        // noise does not.
        let broadband = pseudo_random_amplitude(9_600, 1.8);
        let limited_noise = run_effect(limiter(1, 5, 1, 50, -10), rate, &broadband);
        assert!(
            sample_peak(&limited_noise) <= ceiling * (1.0 + 1.0e-4),
            "broadband material left a sample peak of {}",
            sample_peak(&limited_noise)
        );
        let broadband_true_peak = reference_true_peak(&limited_noise, 1);
        let overshoot_db = 20.0 * (broadband_true_peak / ceiling).log10();
        println!(
            "AU2 A11: broadband noise at x1.8 through a -1.0 dBFS ceiling reads \
             {overshoot_db:+.4} dB at 16x (OPEN-4 allows +0.3)"
        );
        assert!(
            broadband_true_peak <= ceiling * detector_headroom,
            "broadband material read {overshoot_db} dB over the ceiling at 16x"
        );

        // An isolated full-scale impulse with Lh at its minimum for the rate:
        // the case the detector's group delay would break if D were not
        // indexed out of the window.
        let mut impulse = vec![0.0_f32; 4_800];
        impulse[1_000] = 1.0;
        let squashed = run_effect(limiter(1, 1, 1, 50, -10), rate, &impulse);
        assert!(
            sample_peak(&squashed) <= ceiling * (1.0 + 1.0e-4),
            "the impulse left the node at {}",
            sample_peak(&squashed)
        );
        assert!(
            reference_true_peak(&squashed, 1) <= ceiling * detector_headroom,
            "the impulse's 16x true peak was {}",
            reference_true_peak(&squashed, 1)
        );

        // An under-ceiling signal is bit-exact after the Lh-frame delay, at
        // 48 kHz and at rate 1 000 with a 1 ms release, where the release
        // coefficient drops below 0.5 and only A27's short-circuit saves the
        // identity.
        let quiet = tone(997.0, 0.4, rate, 4_800);
        let document = chain_document(vec![limiter(1, 5, 1, 50, 0)]);
        let delay = latency_offset(&document, rate);
        assert_eq!(delay, 240);
        let passed = run_chain(&document, rate, 1, &quiet, None);
        assert_eq!(&passed[delay..], &quiet[..quiet.len() - delay]);

        let slow = tone(50.0, 0.4, 1_000, 400);
        let slow_document = chain_document(vec![limiter(1, 1, 1, 1, 0)]);
        let slow_delay = latency_offset(&slow_document, 1_000);
        assert_eq!(slow_delay, 1);
        let slow_passed = run_chain(&slow_document, 1_000, 1, &slow, None);
        assert_eq!(&slow_passed[slow_delay..], &slow[..slow.len() - slow_delay]);

        // The release constant is within 5 %: 50 ms at 48 kHz is 2 400 frames.
        let mut stepped = vec![0.4_f32; 19_200];
        for sample in &mut stepped[..4_800] {
            *sample = 2.0;
        }
        let released = run_effect(limiter(1, 1, 0, 50, -60), rate, &stepped);
        let start = 4_800 + 48 + 200;
        let gains = released[start..]
            .iter()
            .map(|sample| sample / 0.4)
            .collect::<Vec<_>>();
        let target = 1.0 - (1.0 - gains[0]) / std::f32::consts::E;
        let crossing = gains
            .iter()
            .position(|gain| *gain >= target)
            .expect("the gain must recover");
        #[allow(clippy::cast_precision_loss)]
        let error = (crossing as f32 - 2_400.0).abs() / 2_400.0;
        assert!(
            error <= 0.05,
            "the release reached 1-1/e after {crossing} frames rather than 2 400"
        );

        // The fs/4 pin, on the estimator itself: a 12 kHz sine at 48 kHz
        // sampled at 45 degrees has a sample peak of -3.010 dBFS and a true
        // peak of 0 dBFS.
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        let inter_sample = (0..512)
            .map(|frame| {
                (std::f64::consts::FRAC_PI_2 * f64::from(frame) + std::f64::consts::FRAC_PI_4).sin()
                    as f32
            })
            .collect::<Vec<_>>();
        let sample_peak_db = 20.0 * sample_peak(&inter_sample).log10();
        assert!(
            (sample_peak_db - (-3.010)).abs() <= 0.01,
            "the fs/4 sample peak read {sample_peak_db} dBFS"
        );
        let mut estimator = TruePeakEstimator::new(1);
        let mut estimated = 0.0_f32;
        for sample in &inter_sample {
            estimator.push(std::slice::from_ref(sample));
            estimated = estimated.max(estimator.estimate());
        }
        let estimated_db = 20.0 * estimated.log10();
        assert!(
            estimated_db.abs() <= 0.3,
            "the node's detector read {estimated_db} dBFS on an fs/4 tone"
        );
    }

    /// AU2 §7 item A12 (exit-gate clause 2): gain reduction is reported for
    /// every node with a gain computer, keyed by `(chain, effect id)`, and for
    /// nothing else.
    #[test]
    fn gain_reduction_is_reported_for_every_node_with_a_gain_computer() {
        let rate = 48_000_u32;
        let document = chain_document(vec![
            audio_effect(1, "audio_gain", &[("gain_tenth_db", 0)]),
            parametric_eq(2, &[("band1_gain_tenth_db", 60)]),
            audio_effect(
                3,
                "audio_compressor",
                &[
                    ("threshold_tenth_db", -300),
                    ("ratio_hundredths", 800),
                    ("attack_milliseconds", 1),
                ],
            ),
            audio_effect(
                4,
                "audio_gate",
                &[
                    ("threshold_tenth_db", 0),
                    ("ratio_hundredths", 800),
                    ("range_tenth_db", 240),
                    ("attack_milliseconds", 1),
                    ("hold_milliseconds", 0),
                ],
            ),
            audio_effect(
                5,
                "audio_ducking",
                &[
                    ("threshold_tenth_db", -600),
                    ("reduction_tenth_db", 200),
                    ("attack_milliseconds", 1),
                ],
            ),
            audio_effect(
                6,
                "audio_true_peak_limiter",
                &[
                    ("ceiling_tenth_db", -120),
                    ("lookahead_milliseconds", 1),
                    ("release_milliseconds", 10),
                    ("true_peak", 1),
                ],
            ),
        ]);
        let meters = Arc::new(MixMeters::for_document(
            &document,
            Arc::new(MeterState::default()),
        ));
        let input = tone(440.0, 0.9, rate, 9_600);
        let sidechain = tone(220.0, 0.9, rate, 9_600);
        let tracks = HashMap::from([(TrackId(1), input), (TrackId(2), sidechain)]);
        AudioMixProcessor::new(&document, rate, 1, Some(Arc::clone(&meters)))
            .mix_chunk(&tracks, 0, 9_600)
            .unwrap();

        let reported = meters.peaks().gain_reduction;
        assert_eq!(
            reported
                .iter()
                .map(|(chain, effect, _)| (*chain, *effect))
                .collect::<Vec<_>>(),
            vec![
                (AudioChain::Bus(AudioBusId(1)), EffectId(3)),
                (AudioChain::Bus(AudioBusId(1)), EffectId(4)),
                (AudioChain::Bus(AudioBusId(1)), EffectId(5)),
                (AudioChain::Bus(AudioBusId(1)), EffectId(6)),
            ],
            "only nodes with a gain computer take a slot, in chain order"
        );
        for (chain, effect, decibels) in reported {
            assert!(
                decibels > 0.0,
                "{chain:?} {effect} reported {decibels} dB of reduction"
            );
        }
    }

    /// AU2 §7 item A14: `L == 0` for every pre-AU2 document, so no existing
    /// fixture moves.
    #[test]
    fn pre_au2_documents_declare_no_processing_latency() {
        let legacy = chain_document(vec![
            audio_effect(1, "audio_eq", &[("low_gain_tenth_db", -30)]),
            audio_effect(
                2,
                "audio_compressor",
                &[("threshold_tenth_db", -120), ("ratio_hundredths", 400)],
            ),
            audio_effect(3, "audio_ducking", &[("reduction_tenth_db", 60)]),
            audio_effect(4, "audio_limiter", &[("ceiling_tenth_db", -10)]),
            audio_effect(5, "audio_gain", &[("gain_tenth_db", 120)]),
        ]);
        assert_eq!(
            legacy.audio_mix.lookahead_milliseconds(),
            ChainLookahead {
                bus_stage: 0,
                master_stage: 0,
            }
        );
        for rate in [1_000_u32, 10, 44_100, 48_000] {
            assert_eq!(latency_offset(&legacy, rate), 0);
        }
    }

    /// AU2 §3.6: the graph latency is the sum of the two truncations, and one
    /// bus stage's frame count is exact at both device rates.
    #[test]
    fn graph_latency_sums_the_stage_truncations() {
        assert_eq!(stage_latency_frames(10, 48_000), 480);
        assert_eq!(stage_latency_frames(10, 44_100), 441);
        assert_eq!(stage_latency_frames(5, 44_100), 220);
        assert_eq!(stage_latency_frames(1, 1_000), 1);
        assert_eq!(stage_latency_frames(1, 100), 0);
        let split = ChainLookahead {
            bus_stage: 5,
            master_stage: 5,
        };
        assert_eq!(graph_latency_frames(&split, 44_100), 440);
        assert_eq!(graph_latency_frames(&split, 48_000), 480);
    }

    /// AU2 §7 item A15: an impulse at input frame 0 emerges at output frame
    /// `graph_latency_frames` and nowhere else, at 48 000 Hz and 44 100 Hz.
    #[test]
    #[allow(clippy::float_cmp)]
    fn a_lookahead_chain_delays_an_impulse_by_exactly_the_graph_latency() {
        let document = chain_document(vec![audio_effect(
            1,
            "audio_true_peak_limiter",
            &[
                ("ceiling_tenth_db", 0),
                ("lookahead_milliseconds", 10),
                ("true_peak", 0),
            ],
        )]);
        for (rate, expected) in [(48_000_u32, 480_usize), (44_100, 441)] {
            let offset = latency_offset(&document, rate);
            assert_eq!(offset, expected, "graph latency at {rate} Hz");
            let mut impulse = vec![0.0_f32; 2_000];
            impulse[0] = 0.5;
            let output = run_chain_master(&document, rate, 1, &impulse);
            assert_eq!(output[offset], 0.5, "the impulse must land at {offset}");
            for (index, sample) in output.iter().enumerate() {
                if index != offset {
                    assert_eq!(*sample, 0.0, "sample {index} should be silent");
                }
            }
        }
    }

    /// AU2 §7 item A19: parameters read once per project frame are
    /// bit-identical to a per-sample-frame reference over a keyframed control.
    #[test]
    #[allow(clippy::float_cmp)]
    fn per_project_frame_evaluation_matches_a_per_sample_reference() {
        let rate = 48_000_u32;
        let fps = Rational::new(10, 1).unwrap();
        let mut effect = parametric_eq(1, &[("band1_hertz", 1_000), ("band1_q_hundredths", 100)]);
        effect.keyframes.insert(
            "band1_gain_tenth_db".to_owned(),
            AutomationCurve {
                keyframes: vec![
                    Keyframe {
                        at: TimeCode::ZERO,
                        value: -240,
                        interpolation: KeyframeInterpolation::Linear,
                    },
                    Keyframe {
                        at: TimeCode(10),
                        value: 240,
                        interpolation: KeyframeInterpolation::Linear,
                    },
                ],
            },
        );
        let input = pseudo_random(48_000);
        let output = run_effect(effect.clone(), rate, &input);

        let mut sections = (0..PARAMETRIC_EQ_SECTIONS)
            .map(|_| BiquadSection::new(1))
            .collect::<Vec<_>>();
        let reference = input
            .iter()
            .enumerate()
            .map(|(frame, sample)| {
                let at = samples_to_frame(u64::try_from(frame).unwrap(), rate, fps);
                let settings = ParametricEqSettings::read(&effect, at);
                for (section, coefficients) in sections.iter_mut().zip(settings.coefficients(rate))
                {
                    section.set_coefficients(coefficients);
                }
                let mut frame_samples = [*sample];
                for section in &mut sections {
                    section.process(&mut frame_samples);
                }
                frame_samples[0] * db_gain(settings.output_gain_tenth_db)
            })
            .collect::<Vec<_>>();
        assert_eq!(output, reference);
    }

    /// AU2 §7 item A19: the `0.45 * fs` clamp and the small-value squelch.
    #[test]
    fn the_frequency_clamp_and_the_state_squelch_hold() {
        // A 20 kHz band at 44.1 kHz is designed at 19 845 Hz and stays stable.
        let input = tone(1_000.0, 0.5, 44_100, 44_100);
        let output = run_effect(
            parametric_eq(
                1,
                &[
                    ("band4_hertz", 20_000),
                    ("band4_gain_tenth_db", 240),
                    ("band4_q_hundredths", 1_800),
                ],
            ),
            44_100,
            &input,
        );
        assert!(
            output.iter().all(|sample| sample.is_finite()),
            "a clamped 20 kHz band must stay stable at 44.1 kHz"
        );
        assert!(
            sample_peak(&output) < 10.0,
            "a clamped 20 kHz band must not blow up: {}",
            sample_peak(&output)
        );

        // A settled section's state reaches exactly zero.
        let mut section = BiquadSection::new(1);
        section.set_coefficients(BiquadCoefficients::peaking(1_000.0, 12.0, 0.71, 48_000));
        let mut frame = [1.0_f32];
        section.process(&mut frame);
        let mut settled_at = None;
        for index in 0..5_000 {
            let mut silence = [0.0_f32];
            section.process(&mut silence);
            if section.is_settled() {
                settled_at = Some(index);
                break;
            }
        }
        assert!(
            settled_at.is_some(),
            "the squelch must drive a settled section's state to exact zero"
        );
    }

    /// AU2 §7 item A19: the per-second cost of a worst-case chain, so
    /// `LIVE_FILL_MILLISECONDS` need not be revisited.
    #[test]
    fn a_worst_case_chain_records_its_per_second_cost() {
        let rate = 48_000_u32;
        let channels = 2_usize;
        let document = chain_document(vec![
            audio_effect(
                1,
                "audio_compressor",
                &[
                    ("threshold_tenth_db", -200),
                    ("ratio_hundredths", 400),
                    ("knee_tenth_db", 60),
                    ("detector", 1),
                    ("rms_window_milliseconds", 100),
                    ("lookahead_milliseconds", 10),
                ],
            ),
            audio_effect(
                2,
                "audio_true_peak_limiter",
                &[
                    ("ceiling_tenth_db", -10),
                    ("lookahead_milliseconds", 10),
                    ("release_milliseconds", 50),
                    ("true_peak", 1),
                ],
            ),
        ]);
        assert_eq!(
            document.audio_mix.lookahead_milliseconds().bus_stage,
            kinewright_core::CHAIN_LOOKAHEAD_MILLISECONDS,
            "the worst case is a chain at the whole budget"
        );
        let frames = usize::try_from(rate).unwrap();
        let mut source = Vec::with_capacity(frames * channels);
        for sample in tone(997.0, 0.9, rate, frames) {
            source.push(sample);
            source.push(sample * 0.8);
        }
        let mut processor = AudioMixProcessor::new(&document, rate, channels, None);
        let started = std::time::Instant::now();
        let mut offset = 0_usize;
        while offset < frames {
            let count = MIX_CHUNK_SAMPLE_FRAMES.min(frames - offset);
            let chunk = source[offset * channels..(offset + count) * channels].to_vec();
            processor
                .mix_chunk(
                    &HashMap::from([(TrackId(1), chunk)]),
                    u64::try_from(offset).unwrap(),
                    count,
                )
                .unwrap();
            offset += count;
        }
        let elapsed = started.elapsed();
        println!(
            "AU2 A19: a 10 ms true-peak limiter plus a 10 ms RMS lookahead compressor \
             mixed 1 s of 48 kHz stereo in {:.1} ms ({:.3} x real time)",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64()
        );
        assert!(
            elapsed.as_secs_f64() < 30.0,
            "the worst-case chain took {elapsed:?} for one second of audio"
        );
    }

    /// AU2 §3.9(d): the AU1 parity fixture with a bus chain carrying every AU2
    /// node kind. `parity_document` and `parity_document_with_track_mix` are
    /// untouched, so AU1's own assertions still read what they always read.
    fn parity_document_with_full_chain(voice: &Path, bed: &Path, fps: Rational) -> Document {
        let mut document = parity_document(voice, bed, fps);
        document.audio_mix.buses[0].effects = vec![
            audio_effect(
                10,
                "audio_parametric_eq",
                &[
                    ("high_pass_hertz", 80),
                    ("low_shelf_hertz", 200),
                    ("low_shelf_gain_tenth_db", -30),
                    ("band2_hertz", 1_000),
                    ("band2_gain_tenth_db", 40),
                    ("band2_q_hundredths", 150),
                    ("high_shelf_hertz", 6_000),
                    ("high_shelf_gain_tenth_db", 20),
                    ("output_gain_tenth_db", -10),
                ],
            ),
            audio_effect(
                11,
                "audio_gate",
                &[
                    ("threshold_tenth_db", -400),
                    ("ratio_hundredths", 200),
                    ("range_tenth_db", 60),
                    ("attack_milliseconds", 2),
                    ("hold_milliseconds", 20),
                    ("release_milliseconds", 120),
                ],
            ),
            audio_effect(
                12,
                "audio_compressor",
                &[
                    ("threshold_tenth_db", -120),
                    ("ratio_hundredths", 400),
                    ("knee_tenth_db", 60),
                    ("detector", 1),
                    ("rms_window_milliseconds", 10),
                    ("lookahead_milliseconds", 5),
                    ("makeup_gain_tenth_db", 20),
                ],
            ),
            audio_effect(
                13,
                "audio_ducking",
                &[("threshold_tenth_db", -300), ("reduction_tenth_db", 60)],
            ),
            audio_effect(14, "audio_gain", &[("gain_tenth_db", 120)]),
            audio_effect(
                15,
                "audio_true_peak_limiter",
                &[
                    ("ceiling_tenth_db", -10),
                    ("lookahead_milliseconds", 5),
                    ("release_milliseconds", 50),
                    ("true_peak", 1),
                ],
            ),
        ];
        document
    }

    /// The same fixture with the legacy pair kept alive on a second bus.
    fn parity_document_with_legacy_nodes(voice: &Path, bed: &Path, fps: Rational) -> Document {
        let mut document = parity_document_with_full_chain(voice, bed, fps);
        document.audio_mix.buses.push(AudioBus {
            id: AudioBusId(2),
            name: "Legacy".to_owned(),
            tracks: vec![TrackId(3)],
            gain_tenth_db: 0,
            effects: vec![
                audio_effect(20, "audio_eq", &[("high_gain_tenth_db", -30)]),
                audio_effect(21, "audio_limiter", &[("ceiling_tenth_db", -20)]),
            ],
            ducking_sidechain_tracks: Vec::new(),
            gain_curve: None,
        });
        document
    }

    /// AU2 §7 item A17 (exit-gate clause 3, bus half): playback/export parity
    /// through every bus node, with the graph latency compensated on both
    /// sides.
    #[test]
    fn playback_feeder_mix_matches_export_through_every_bus_node() {
        crate::initialize_ffmpeg().unwrap();
        let voice = loud_sine("au2-voice", 440);
        let bed = loud_sine("au2-bed", 660);
        let fps = Rational::new(10, 1).unwrap();
        let settings = parity_settings(fps);
        let expected_samples =
            usize::try_from(frame_to_samples(TimeCode(30), 48_000, fps)).unwrap() * 2;

        for document in [
            transitioned(parity_document_with_full_chain(
                voice.path(),
                bed.path(),
                fps,
            )),
            transitioned(parity_document_with_legacy_nodes(
                voice.path(),
                bed.path(),
                fps,
            )),
        ] {
            assert_eq!(
                document.audio_mix.lookahead_milliseconds(),
                ChainLookahead {
                    bus_stage: 10,
                    master_stage: 0,
                }
            );
            assert_eq!(latency_offset(&document, 48_000), 480);
            let exported = crate::export::mix_audio(&document, &settings).unwrap();
            assert_eq!(
                exported.len(),
                expected_samples,
                "the flush must not lengthen the export"
            );
            assert_playback_matches_export(&document, &exported, fps);
        }
    }

    /// A 32-bit float WAV, written by hand so a single-sample impulse survives
    /// exactly (AU2 §3.9(c)).
    fn wav_f32(samples: &[f32], rate: u32, channels: u16) -> Vec<u8> {
        let data_length = u32::try_from(samples.len() * 4).expect("the fixture should fit");
        let mut bytes = Vec::with_capacity(44 + samples.len() * 4);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_length).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&3_u16.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * u32::from(channels) * 4).to_le_bytes());
        bytes.extend_from_slice(&(channels * 4).to_le_bytes());
        bytes.extend_from_slice(&32_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_length.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        bytes
    }

    /// Impulses at project sample 0, at project frame 3, and at the very last
    /// sample of a two-second 48 kHz stereo source.
    fn impulse_media() -> GeneratedMedia {
        let mut samples = vec![0.0_f32; 96_000 * 2];
        for frame in [0_usize, 14_400, 95_999] {
            samples[frame * 2] = 0.5;
            samples[frame * 2 + 1] = 0.5;
        }
        GeneratedMedia::from_bytes("au2-impulse", "wav", &wav_f32(&samples, 48_000, 2))
    }

    /// One track routed to a bus carrying a 10 ms lookahead limiter.
    fn impulse_document(source: &Path, fps: Rational) -> Document {
        Document {
            catalog: kinewright_core::MediaCatalog::default(),
            audio_mix: AudioMix {
                tracks: Vec::new(),
                buses: vec![AudioBus {
                    id: AudioBusId(1),
                    name: "Delivery".to_owned(),
                    tracks: vec![TrackId(1)],
                    gain_tenth_db: 0,
                    effects: vec![audio_effect(
                        1,
                        "audio_true_peak_limiter",
                        &[
                            ("ceiling_tenth_db", 0),
                            ("lookahead_milliseconds", 10),
                            ("release_milliseconds", 50),
                            ("true_peak", 0),
                        ],
                    )],
                    ducking_sidechain_tracks: Vec::new(),
                    gain_curve: None,
                }],
                ..AudioMix::default()
            },
            color_context: kinewright_core::ColorContext::default(),
            lut_assets: Vec::new(),
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Audio,
                sync_lock: true,
                clips: vec![audio_clip(1, 1, 0..20, 0)],
            }],
            media_pool: vec![audio_asset(1, source, "impulses", fps)],
            markers: Vec::new(),
            fps,
            resolution: (64, 64),
            duration: TimeCode(20),
        }
    }

    /// AU2 §7 item A15/A29: export puts an impulse at project frame `k` at
    /// exported frame `k`, the programme tail survives the flush, and
    /// `AudioMixer::open` at frame 0, at frame 5, and at the last frame each
    /// emit their first output frame from the matching input frame.
    #[test]
    fn document_derived_latency_is_compensated_through_export_and_seek() {
        crate::initialize_ffmpeg().unwrap();
        let source = impulse_media();
        let fps = Rational::new(10, 1).unwrap();
        let document = impulse_document(source.path(), fps);
        document.validate().unwrap();
        assert_eq!(latency_offset(&document, 48_000), 480);

        let exported = crate::export::mix_audio(&document, &parity_settings(fps)).unwrap();
        assert_eq!(
            exported.len(),
            96_000 * 2,
            "the tail must survive the flush"
        );
        for frame in [0_usize, 14_400, 95_999] {
            assert!(
                (exported[frame * 2] - 0.5).abs() <= 1.0e-3,
                "the impulse at project frame {frame} exported as {}",
                exported[frame * 2]
            );
        }

        // `open` at frame 0, frame 5, and the last frame: the cursor lands
        // where AU1 says and output frame k still carries project frame
        // target + k.
        for target in [TimeCode::ZERO, TimeCode(5), TimeCode(19)] {
            let seek_sample = usize::try_from(frame_to_samples(target, 48_000, fps)).unwrap() * 2;
            let mut mixer = AudioMixer::open(&document, target, 48_000, 2, None).unwrap();
            let played = mixer.render_remaining().unwrap();
            assert_eq!(
                played.len(),
                exported.len() - seek_sample,
                "open at {target} emitted the wrong length"
            );
            let difference = exported[seek_sample..]
                .iter()
                .zip(&played)
                .map(|(exported, played)| (exported - played).abs())
                .fold(0.0_f32, f32::max);
            assert!(
                difference <= 1.0e-6,
                "open at {target} differs by {difference}"
            );
        }
    }

    /// AU2 §7 item A16 (A2): `measure_mix_levels` over a range containing one
    /// impulse reports it in the track stem, the bus stem, and the master, and
    /// all three report the same `sample_frames`. AU3 §3.3 / A11: the range is
    /// one gating block (`0..4` at 10 fps = 19 200 frames), the smallest a
    /// loudness measurement accepts; it still contains the frame-0 impulse.
    #[test]
    fn measure_mix_levels_trims_every_stem_family_to_the_requested_range() {
        crate::initialize_ffmpeg().unwrap();
        let source = impulse_media();
        let fps = Rational::new(10, 1).unwrap();
        let document = impulse_document(source.path(), fps);
        document.validate().unwrap();

        let report = crate::export::measure_mix_levels(
            &document,
            &MixLevelRequest {
                range: Some(TimeCode::ZERO..TimeCode(4)),
            },
        )
        .unwrap();
        assert_eq!(report.range, TimeCode::ZERO..TimeCode(4));
        assert_eq!(report.tracks.len(), 1);
        assert_eq!(report.buses.len(), 1);

        let track = report.tracks[0].levels;
        let bus = report.buses[0].levels;
        assert_eq!(track.sample_frames, 19_200);
        assert_eq!(bus.sample_frames, 19_200);
        assert_eq!(report.master.sample_frames, 19_200);
        for (name, peak) in [
            ("track stem", track.sample_peak_dbfs_hundredths),
            ("bus stem", bus.sample_peak_dbfs_hundredths),
            ("master", report.master.sample_peak_dbfs_hundredths),
        ] {
            let peak = peak.unwrap_or_else(|| panic!("the {name} must carry the impulse"));
            assert!(
                (peak + 602).abs() <= 10,
                "the {name} read {peak} rather than about -602"
            );
        }
    }

    /// AU2 §2.1 (review MAJOR 1): a `Hold` keyframe that un-bypasses a
    /// true-peak limiter mid-programme must not let `Lh` frames of unanalysed
    /// material through. Every dynamics node keeps its detector, window and
    /// release state running while bypassed and skips only the multiply.
    #[test]
    fn un_bypassing_a_limiter_mid_signal_never_breaks_its_ceiling() {
        let rate = 48_000_u32;
        let fps = Rational::new(10, 1).unwrap();
        let flip_at = TimeCode(5);
        let flip_frame =
            usize::try_from(frame_to_samples(flip_at, rate, fps)).expect("the flip should fit");
        let ceiling = 10.0_f32.powf(-1.0 / 20.0);
        let detector_headroom = 10.0_f32.powf(0.3 / 20.0);
        let limiter = || {
            audio_effect(
                1,
                "audio_true_peak_limiter",
                &[
                    ("ceiling_tenth_db", -10),
                    ("lookahead_milliseconds", 10),
                    ("release_milliseconds", 50),
                    ("true_peak", 1),
                ],
            )
        };

        // Broadband material at +5 dBFS across the whole buffer.
        let noise = pseudo_random_amplitude(48_000, 1.8);
        let output = run_effect(un_bypassing(limiter(), flip_at), rate, &noise);
        let after = &output[flip_frame..];
        assert!(
            sample_peak(after) <= ceiling * (1.0 + 1.0e-4),
            "un-bypassing let a sample peak of {} past the ceiling {ceiling}",
            sample_peak(after)
        );
        assert!(
            reference_true_peak(after, 1) <= ceiling * detector_headroom,
            "un-bypassing let a 16x true peak of {} past the ceiling {ceiling}",
            reference_true_peak(after, 1)
        );

        // The impulse case: full-scale impulses that enter *during* the bypass
        // window and are emitted after the flip.
        let mut impulses = vec![0.0_f32; 48_000];
        impulses[flip_frame - 200] = 1.0;
        impulses[flip_frame - 100] = 1.0;
        impulses[flip_frame + 100] = 1.0;
        let output = run_effect(un_bypassing(limiter(), flip_at), rate, &impulses);
        let after = &output[flip_frame..];
        assert!(
            sample_peak(after) <= ceiling * (1.0 + 1.0e-4),
            "an impulse that entered during bypass left at {}",
            sample_peak(after)
        );
        assert!(
            reference_true_peak(after, 1) <= ceiling * detector_headroom,
            "an impulse that entered during bypass read {} at 16x",
            reference_true_peak(after, 1)
        );
    }

    /// AU2 §2.1 (review MAJOR 1): a compressor's envelope keeps tracking while
    /// bypassed, so un-bypassing lands on a live envelope, not a stale one.
    #[test]
    #[allow(clippy::float_cmp)]
    fn un_bypassing_a_compressor_lands_on_a_live_envelope() {
        let rate = 48_000_u32;
        let fps = Rational::new(10, 1).unwrap();
        let flip_at = TimeCode(5);
        let flip_sample =
            usize::try_from(frame_to_samples(flip_at, rate, fps)).expect("the flip should fit");
        let compressor = || {
            audio_effect(
                1,
                "audio_compressor",
                &[
                    ("threshold_tenth_db", -200),
                    ("ratio_hundredths", 400),
                    ("attack_milliseconds", 1),
                    ("release_milliseconds", 100),
                    ("makeup_gain_tenth_db", 20),
                    ("lookahead_milliseconds", 5),
                ],
            )
        };
        let input = pseudo_random_amplitude(48_000, 0.9);
        let flipped = run_effect(un_bypassing(compressor(), flip_at), rate, &input);
        let never = run_effect(compressor(), rate, &input);
        assert_eq!(
            &flipped[flip_sample..],
            &never[flip_sample..],
            "the envelope after un-bypass must match a never-bypassed node's"
        );
        // And before the flip the bypassed node is its own delayed input.
        let delay = 240_usize;
        assert_eq!(
            &flipped[delay..flip_sample],
            &input[..flip_sample - delay],
            "a bypassed compressor must still pass its delayed input through"
        );
    }

    /// AU2 §3.5/A27 (review MINOR 5): the identity short-circuit is two-way, so
    /// gain reduction returns to exactly 0 dB once limiting stops and the
    /// release has elapsed.
    #[test]
    #[allow(clippy::float_cmp)]
    fn gain_reduction_returns_to_exactly_zero_after_the_release() {
        for (rate, release, loud_frames, quiet_frames, lookahead) in [
            (48_000_u32, 50_i64, 4_800_usize, 96_000_usize, 5_i64),
            (1_000, 1, 200, 4_000, 1),
        ] {
            let document = chain_document(vec![audio_effect(
                1,
                "audio_true_peak_limiter",
                &[
                    ("ceiling_tenth_db", -60),
                    ("lookahead_milliseconds", lookahead),
                    ("release_milliseconds", release),
                    ("true_peak", 1),
                ],
            )]);
            let meters = Arc::new(MixMeters::for_document(
                &document,
                Arc::new(MeterState::default()),
            ));
            let mut processor =
                AudioMixProcessor::new(&document, rate, 1, Some(Arc::clone(&meters)));
            let loud = tone(997.0, 0.9, rate, loud_frames);
            processor
                .mix_chunk(&HashMap::from([(TrackId(1), loud)]), 0, loud_frames)
                .unwrap();
            let engaged = meters.peaks().gain_reduction[0].2;
            assert!(
                engaged > 0.0,
                "the limiter must engage at {rate} Hz, read {engaged} dB"
            );

            // One chunk for the release to elapse in, then one settled chunk
            // whose reported minimum must be exactly unity.
            let mut cursor = u64::try_from(loud_frames).unwrap();
            for frames in [quiet_frames, quiet_frames / 4] {
                processor
                    .mix_chunk(
                        &HashMap::from([(TrackId(1), vec![0.001_f32; frames])]),
                        cursor,
                        frames,
                    )
                    .unwrap();
                cursor = cursor.saturating_add(u64::try_from(frames).unwrap());
            }
            let released = meters.peaks().gain_reduction[0].2;
            assert_eq!(
                released, 0.0,
                "the limiter must report exactly 0 dB at {rate} Hz once released"
            );
        }
    }

    /// An impulse just inside a sub-range, with a full-scale burst just after
    /// it, so the flush frames' gain depends on real programme audio.
    fn anticipation_media() -> GeneratedMedia {
        let mut samples = vec![0.0_f32; 96_000 * 2];
        samples[47_900 * 2] = 0.5;
        samples[47_900 * 2 + 1] = 0.5;
        for frame in 48_000..48_500 {
            samples[frame * 2] = 1.0;
            samples[frame * 2 + 1] = -1.0;
        }
        GeneratedMedia::from_bytes("au2-anticipation", "wav", &wav_f32(&samples, 48_000, 2))
    }

    /// AU2 §3.7 (review MINOR 2): a sub-range measurement's last `L` sample
    /// frames are limited against real programme audio, not against silence, so
    /// the sub-range master peak matches the full export restricted to the same
    /// range.
    #[test]
    fn a_sub_range_measurement_limits_against_the_real_programme() {
        crate::initialize_ffmpeg().unwrap();
        let source = anticipation_media();
        let fps = Rational::new(10, 1).unwrap();
        let mut document = impulse_document(source.path(), fps);
        document.audio_mix.buses[0].effects = vec![audio_effect(
            1,
            "audio_true_peak_limiter",
            &[
                ("ceiling_tenth_db", -60),
                ("lookahead_milliseconds", 10),
                ("release_milliseconds", 50),
                ("true_peak", 0),
            ],
        )];
        document.validate().unwrap();

        let whole = crate::export::mix_audio(&document, &parity_settings(fps)).unwrap();
        let expected = sample_peak(&whole[..48_000 * 2]);
        assert!(
            expected < 0.5 * (1.0 - 1.0e-3),
            "the burst must duck the impulse in a full export: {expected}"
        );

        let report = crate::export::measure_mix_levels(
            &document,
            &MixLevelRequest {
                range: Some(TimeCode::ZERO..TimeCode(10)),
            },
        )
        .unwrap();
        assert_eq!(report.master.sample_frames, 48_000);
        let measured_db = f64::from(
            report
                .master
                .sample_peak_dbfs_hundredths
                .expect("the sub-range must carry the impulse"),
        ) / 100.0;
        let expected_db = 20.0 * f64::from(expected).log10();
        assert!(
            (measured_db - expected_db).abs() <= 0.01,
            "the sub-range master read {measured_db} dBFS, the full export {expected_db} dBFS"
        );
    }

    /// AU2 §3.7 (review pass 2, finding 1): `mix_pass` enumerates exactly the
    /// project frames that contain the sample frames it mixes. The last one
    /// consumed is `total - 1`, so an `L == 0` pass — where `total` lands on a
    /// frame boundary — enumerates `range.end` and not one frame more.
    #[test]
    fn the_mix_pass_segment_enumeration_covers_only_the_mixed_frames() {
        let fps = Rational::new(10, 1).unwrap();
        let covering = |total: u64| samples_to_frame(total - 1, 48_000, fps).0 + 1;
        let end = frame_to_samples(TimeCode(10), 48_000, fps);
        assert_eq!(end, 48_000);
        assert_eq!(covering(end), 10, "L == 0 must not decode an extra frame");
        assert_eq!(
            covering(end + 480),
            11,
            "a 10 ms flush reaches into frame 10"
        );
        assert_eq!(
            covering(end + 4_800),
            11,
            "a whole extra frame still ends at 11"
        );
        assert_eq!(covering(end + 4_801), 12);
    }

    // ---- AU2 Part B -------------------------------------------------------

    /// One clipless bus routing track 1, with the given chain.
    fn part_b_bus(id: u64, tracks: Vec<u64>, effects: Vec<Effect>) -> AudioBus {
        AudioBus {
            id: AudioBusId(id),
            name: format!("Bus {id}"),
            tracks: tracks.into_iter().map(TrackId).collect(),
            gain_tenth_db: 0,
            effects,
            ducking_sidechain_tracks: Vec::new(),
            gain_curve: None,
        }
    }

    /// A non-neutral parametric EQ, so its biquad memories are worth
    /// preserving across a live edit.
    fn part_b_eq(id: u64, output_gain_tenth_db: i64) -> Effect {
        parametric_eq(
            id,
            &[
                ("low_shelf_hertz", 200),
                ("low_shelf_gain_tenth_db", -40),
                ("band2_hertz", 1_000),
                ("band2_gain_tenth_db", 60),
                ("band2_q_hundredths", 150),
                ("output_gain_tenth_db", output_gain_tenth_db),
            ],
        )
    }

    /// Mix one chunk of one track through a processor and return the master.
    ///
    /// `frames` is the chunk's sample-frame count, so a multi-channel caller
    /// passes `input.len() / channels` and never relies on `mix`'s zero-fill.
    fn part_b_chunk(
        processor: &mut AudioMixProcessor,
        track: u64,
        input: &[f32],
        frames: usize,
        start_sample: u64,
    ) -> Vec<f32> {
        processor
            .mix_chunk(
                &HashMap::from([(TrackId(track), input.to_vec())]),
                start_sample,
                frames,
            )
            .expect("the chunk should mix")
    }

    /// AU2 §7 item B7 / §5.6: a bus-free document carrying one master effect —
    /// the first document the "+ Effect" menu on the master produces — has
    /// stateful nodes, so a seek must warm them from zero.
    #[test]
    fn a_bus_free_document_with_one_master_effect_needs_seek_preroll() {
        let fps = Rational::new(10, 1).unwrap();
        let source = GeneratedMedia::from_bytes(
            "au2b-preroll",
            "wav",
            &wav_f32(&vec![0.25_f32; 96_000 * 2], 48_000, 2),
        );
        let mut document = processor_document(fps, 20, Vec::new());
        document.media_pool = vec![audio_asset(1, source.path(), "steady", fps)];
        document.tracks[0].clips = vec![audio_clip(1, 1, 0..20, 0)];
        document.audio_mix.master.effects = vec![audio_effect(
            1,
            "audio_true_peak_limiter",
            &[
                ("ceiling_tenth_db", -10),
                ("lookahead_milliseconds", 5),
                ("release_milliseconds", 50),
                ("true_peak", 1),
            ],
        )];
        document.validate().expect("the fixture should validate");

        assert!(document.audio_mix.buses.is_empty());
        assert!(needs_seek_preroll(&document, TimeCode(5)));
        assert!(!needs_seek_preroll(&document, TimeCode::ZERO));
        // `decode_from == TimeCode::ZERO`: the sources are built from frame 0,
        // not from the seek target, and the discard loop then runs the mix
        // forward to it.
        let mixer = AudioMixer::open(&document, TimeCode(5), 48_000, 2, None).unwrap();
        assert_eq!(mixer.sources.len(), 1);
        assert_eq!(mixer.sources[0].project_sample_start, 0);
        // AU2 §3.7: the discard loop runs `L` frames past the target, so output
        // frame `k` still carries project frame `target + k`.
        assert_eq!(latency_offset(&document, 48_000), 240);
        assert_eq!(
            mixer.cursor_sample,
            frame_to_samples(TimeCode(5), 48_000, fps) + 240
        );

        // The AU1 predicate is untouched for a master that carries only a
        // fader: a fader has no state to warm.
        let mut faded = document.clone();
        faded.audio_mix.master.effects.clear();
        faded.audio_mix.master.gain_tenth_db = -60;
        assert!(!needs_seek_preroll(&faded, TimeCode(5)));
    }

    /// AU2 §7 item B8 / §5.10(e): the two laws, their exact cardinals, and the
    /// channel rules neither law changes.
    // The cardinal ratios and the balance law's AU1 identity are exact claims.
    #[allow(clippy::float_cmp)]
    #[test]
    fn both_pan_laws_follow_their_channel_and_cardinal_rules() {
        // `L^2 + R^2 == 1` within 1e-6 at every integer position.
        for pan in TRACK_MIX_PAN_MIN..=TRACK_MIX_PAN_MAX {
            let [left, right] = pan_channel_ratios(PanLaw::ConstantPower, pan);
            let power =
                f64::from(left).mul_add(f64::from(left), f64::from(right) * f64::from(right));
            assert!(
                (power - 1.0).abs() <= 1.0e-6,
                "constant power at {pan} carries {power}"
            );
        }
        // The three cardinal positions are exact under `==`.
        assert_eq!(pan_channel_ratios(PanLaw::ConstantPower, -100), [1.0, 0.0]);
        assert_eq!(pan_channel_ratios(PanLaw::ConstantPower, 100), [0.0, 1.0]);
        assert_eq!(
            pan_channel_ratios(PanLaw::ConstantPower, 0),
            [
                std::f32::consts::FRAC_1_SQRT_2,
                std::f32::consts::FRAC_1_SQRT_2
            ]
        );
        assert_eq!(pan_channel_ratios(PanLaw::Balance, -100), [1.0, 0.0]);
        assert_eq!(pan_channel_ratios(PanLaw::Balance, 100), [0.0, 1.0]);
        assert_eq!(pan_channel_ratios(PanLaw::Balance, 0), [1.0, 1.0]);
        // Centre is -3.0103 dB +/- 0.001 under constant power.
        let centre = f64::from(pan_channel_ratios(PanLaw::ConstantPower, 0)[0]);
        let centre_db = 20.0 * centre.log10();
        assert!(
            (centre_db + 3.0103).abs() <= 0.001,
            "constant-power centre is {centre_db} dB"
        );
        // `Balance` is bit-identical to AU1's own expression at every integer
        // position (AU2 §0 E42: 82 of the 201 round differently in f64).
        for pan in TRACK_MIX_PAN_MIN..=TRACK_MIX_PAN_MAX {
            #[allow(clippy::cast_precision_loss)]
            let au1 = pan as f32 / 100.0;
            assert_eq!(
                pan_channel_ratios(PanLaw::Balance, pan),
                [1.0 - au1.max(0.0), 1.0 + au1.min(0.0)],
                "the balance law moved at {pan}"
            );
        }

        // AU1's channel rules, under both laws: a one-channel device applies no
        // pan, and channels beyond the first two take the unpanned gain.
        for law in [PanLaw::Balance, PanLaw::ConstantPower] {
            let mut document = mix_document(vec![track_mix(1, 30, -100, false, false)], Vec::new());
            document.audio_mix.pan_law = law;
            let gain = db_gain(30);
            assert_eq!(
                track_stage_parameters(&document, TrackId(1), 1),
                (true, gain, [gain, gain]),
                "a mono device must hear no pan under {law:?}"
            );
            let mixed = part_b_chunk(
                &mut AudioMixProcessor::new(&document, 1_000, 4, None),
                1,
                &[1.0; 32],
                8,
                0,
            );
            assert_eq!(mixed[0], gain, "hard left keeps channel 0 at unity gain");
            assert_eq!(mixed[1], 0.0, "hard left silences channel 1");
            assert_eq!(mixed[2], gain, "channel 2 takes the unpanned gain");
            assert_eq!(mixed[3], gain, "channel 3 takes the unpanned gain");
        }
    }

    /// AU2 §7 item B9 / §5.7: the bus and master faders ramp over 240 sample
    /// frames at 48 kHz and settle exactly; a fresh processor never ramps.
    // Ramp values are exact by construction, as AU1's ramp tests are.
    #[allow(clippy::float_cmp)]
    #[test]
    fn bus_and_master_gain_ramp_over_five_milliseconds_and_settle_exactly() {
        let fps = Rational::new(10, 1).unwrap();
        let document = processor_document(fps, 30, vec![part_b_bus(1, vec![1], Vec::new())]);
        let ramp_frames = track_mix_ramp_frames(48_000);
        assert_eq!(ramp_frames, 240);
        let ones = vec![1.0_f32; 300];

        for (label, retarget) in [
            (
                "bus",
                (|document: &mut Document, tenth_db: i32| {
                    document.audio_mix.buses[0].gain_tenth_db = tenth_db;
                }) as fn(&mut Document, i32),
            ),
            ("master", |document: &mut Document, tenth_db: i32| {
                document.audio_mix.master.gain_tenth_db = tenth_db;
            }),
        ] {
            let mut processor = AudioMixProcessor::new(&document, 48_000, 1, None);
            // A freshly built processor is settled, so export and a newly
            // opened playback mixer never ramp.
            assert!(processor.buses[0].gain.is_settled(ramp_frames));
            assert!(processor.master.gain.is_settled(ramp_frames));
            let settled = part_b_chunk(&mut processor, 1, &ones, ones.len(), 0);
            assert!(
                settled.iter().all(|sample| *sample == 1.0),
                "{label}: a fresh processor must not ramp"
            );

            let mut updated = document.clone();
            retarget(&mut updated, -60);
            processor.update_audio_mix(&updated);
            let ramped = part_b_chunk(&mut processor, 1, &ones, ones.len(), 300);
            let target = db_gain(-60);
            for (frame, sample) in ramped.iter().enumerate().take(ramp_frames - 1) {
                #[allow(clippy::cast_precision_loss)]
                let progress = (frame + 1) as f32 / ramp_frames as f32;
                assert_eq!(
                    *sample,
                    1.0 + (target - 1.0) * progress,
                    "{label}: sample frame {frame} is off the ramp"
                );
            }
            assert_eq!(
                ramped[ramp_frames - 1],
                target,
                "{label}: the ramp settles exactly at frame 239"
            );
            for sample in &ramped[ramp_frames..] {
                assert_eq!(*sample, target, "{label}: the settled gain must not drift");
            }

            // A second update to the same value does not restart the ramp.
            processor.update_audio_mix(&updated);
            let held = part_b_chunk(&mut processor, 1, &ones, ones.len(), 600);
            assert!(held.iter().all(|sample| *sample == target));
        }

        // Export builds a fresh processor per pass and never calls
        // `update_audio_mix`, so a stored fader is applied from sample zero.
        let mut geared = document.clone();
        geared.audio_mix.buses[0].gain_tenth_db = -60;
        geared.audio_mix.master.gain_tenth_db = 60;
        let mixed = part_b_chunk(
            &mut AudioMixProcessor::new(&geared, 48_000, 1, None),
            1,
            &ones,
            ones.len(),
            0,
        );
        let expected = db_gain(-60) * db_gain(60);
        assert!(mixed.iter().all(|sample| *sample == expected));
    }

    /// AU2 §7 item B9 / §5.7: retargeting a fader mid-ramp restarts from the
    /// **current interpolated value**, so a fader dragged through two positions
    /// inside 5 ms has no discontinuity. `TrackStageRuntime` carries AU1's own
    /// pin; `GainRamp` is a separate struct with its own copy of the arithmetic.
    // Ramp values are exact by construction, as AU1's ramp tests are.
    #[allow(clippy::float_cmp)]
    #[test]
    fn a_fader_retarget_mid_ramp_restarts_from_the_current_value() {
        let fps = Rational::new(10, 1).unwrap();
        let document = processor_document(fps, 30, vec![part_b_bus(1, vec![1], Vec::new())]);
        let ramp_frames = track_mix_ramp_frames(48_000);
        let ones = vec![1.0_f32; 300];
        let target = db_gain(-60);

        for (label, retarget) in [
            (
                "bus",
                (|document: &mut Document, tenth_db: i32| {
                    document.audio_mix.buses[0].gain_tenth_db = tenth_db;
                }) as fn(&mut Document, i32),
            ),
            ("master", |document: &mut Document, tenth_db: i32| {
                document.audio_mix.master.gain_tenth_db = tenth_db;
            }),
        ] {
            let mut processor = AudioMixProcessor::new(&document, 48_000, 1, None);
            let mut lowered = document.clone();
            retarget(&mut lowered, -60);
            processor.update_audio_mix(&lowered);

            // 100 of the 240 frames, so the ramp is still in flight.
            let partial = part_b_chunk(&mut processor, 1, &ones[..100], 100, 0);
            #[allow(clippy::cast_precision_loss)]
            let mid = 1.0 + (target - 1.0) * (100.0_f32 / ramp_frames as f32);
            assert_eq!(
                partial[99], mid,
                "{label}: the ramp must be 100/240 of the way down"
            );

            // Retarget back to unity from the interpolated value.
            processor.update_audio_mix(&document);
            let restarted = part_b_chunk(&mut processor, 1, &ones, ones.len(), 100);
            #[allow(clippy::cast_precision_loss)]
            let first_step = mid + (1.0 - mid) * (1.0 / ramp_frames as f32);
            assert_eq!(
                restarted[0], first_step,
                "{label}: the second ramp must start from the current value, not from unity"
            );
            assert!(
                (restarted[0] - mid).abs() < (1.0 - mid).abs(),
                "{label}: the restart must not step past its own target"
            );
            for (frame, sample) in restarted.iter().enumerate().take(ramp_frames - 1) {
                #[allow(clippy::cast_precision_loss)]
                let progress = (frame + 1) as f32 / ramp_frames as f32;
                assert_eq!(
                    *sample,
                    mid + (1.0 - mid) * progress,
                    "{label}: sample frame {frame} is off the restarted ramp"
                );
            }
            assert_eq!(
                restarted[ramp_frames - 1],
                1.0,
                "{label}: the restarted ramp settles exactly on unity"
            );
            for sample in &restarted[ramp_frames..] {
                assert_eq!(*sample, 1.0);
            }
        }
    }

    /// AU2 §3.9(d)/§5.10, the master half: the full-chain parity fixture with a
    /// master chain of one gain plus one true-peak limiter, and a non-neutral
    /// bus and master fader. `parity_document` and
    /// `parity_document_with_full_chain` are untouched.
    fn parity_document_with_master_chain(voice: &Path, bed: &Path, fps: Rational) -> Document {
        let mut document = parity_document_with_full_chain(voice, bed, fps);
        document.audio_mix.buses[0].gain_tenth_db = -30;
        document.audio_mix.master = kinewright_core::AudioMaster {
            gain_tenth_db: -20,
            effects: vec![
                audio_effect(30, "audio_gain", &[("gain_tenth_db", 40)]),
                audio_effect(
                    31,
                    "audio_true_peak_limiter",
                    &[
                        ("ceiling_tenth_db", -10),
                        ("lookahead_milliseconds", 5),
                        ("release_milliseconds", 50),
                        ("true_peak", 1),
                    ],
                ),
            ],
            gain_curve: None,
        };
        document.audio_mix.pan_law = PanLaw::ConstantPower;
        document.audio_mix.tracks = vec![track_mix(2, 0, 25, false, false)];
        document
    }

    /// AU2 §7 item B10 (exit-gate clause 3): playback/export parity through
    /// every node, the master chain included, with the graph latency
    /// compensated on both sides.
    #[test]
    fn playback_feeder_mix_matches_export_through_the_master_chain() {
        crate::initialize_ffmpeg().unwrap();
        let voice = loud_sine("au2b-voice", 440);
        let bed = loud_sine("au2b-bed", 660);
        let fps = Rational::new(10, 1).unwrap();
        let settings = parity_settings(fps);
        let document = transitioned(parity_document_with_master_chain(
            voice.path(),
            bed.path(),
            fps,
        ));

        assert_eq!(
            document.audio_mix.lookahead_milliseconds(),
            ChainLookahead {
                bus_stage: 10,
                master_stage: 5,
            }
        );
        // AU2 §3.6: the sum of the two truncations, never a truncation of the
        // sum.
        assert_eq!(latency_offset(&document, 48_000), 480 + 240);
        let exported = crate::export::mix_audio(&document, &settings).unwrap();
        assert_eq!(
            exported.len(),
            usize::try_from(frame_to_samples(TimeCode(30), 48_000, fps)).unwrap() * 2,
            "the master flush must not lengthen the export"
        );
        assert_playback_matches_export(&document, &exported, fps);
    }

    /// AU2 §7 item B11 / A13: a parameter-only edit keeps every filter memory
    /// and is audible on the **next sample frame**, not at the next
    /// project-frame boundary — 100 ms away at this fixture's 10 fps.
    // The scaling identity is exact: the chain's output gain is the last
    // multiply, so the two runs differ by exactly one factor.
    #[allow(clippy::float_cmp)]
    #[test]
    fn a_parameter_retarget_preserves_chain_state_and_is_audible_next_sample_frame() {
        let fps = Rational::new(10, 1).unwrap();
        let document = processor_document(
            fps,
            30,
            vec![part_b_bus(1, vec![1], vec![part_b_eq(10, 0)])],
        );
        let mut updated = document.clone();
        updated.audio_mix.buses[0].effects = vec![part_b_eq(10, -60)];
        // Both chunks sit inside project frame 0, which spans 4 800 sample
        // frames at 10 fps.
        let first = tone(440.0, 0.5, 48_000, 480);
        let second = tone(1_000.0, 0.5, 48_000, 480);

        let mut unchanged = AudioMixProcessor::new(&document, 48_000, 1, None);
        part_b_chunk(&mut unchanged, 1, &first, first.len(), 0);
        let steady = part_b_chunk(&mut unchanged, 1, &second, second.len(), 480);

        let mut retargeted = AudioMixProcessor::new(&document, 48_000, 1, None);
        part_b_chunk(&mut retargeted, 1, &first, first.len(), 0);
        retargeted.update_audio_mix(&updated);
        let edited = part_b_chunk(&mut retargeted, 1, &second, second.len(), 480);

        let trim = db_gain(-60);
        assert_eq!(
            edited[0],
            steady[0] * trim,
            "the retarget must be heard on the very next sample frame"
        );
        for (frame, (edited, steady)) in edited.iter().zip(&steady).enumerate() {
            assert_eq!(
                *edited,
                steady * trim,
                "sample frame {frame} lost the preserved filter state"
            );
        }

        // A rebuilt chain would have reset those biquads, which is audible.
        let mut rebuilt = AudioMixProcessor::new(&updated, 48_000, 1, None);
        let cold = part_b_chunk(&mut rebuilt, 1, &second, second.len(), 480);
        assert!(
            (cold[0] - edited[0]).abs() > 1.0e-6,
            "the retargeted chain must not read like a freshly built one"
        );
    }

    /// AU2 §7 item B11: an insertion, a removal, and a reorder each rebuild
    /// **only** the chain that changed; every other chain keeps its state.
    // The untouched bus is asserted bit-identical.
    #[allow(clippy::float_cmp)]
    #[test]
    fn a_structural_live_edit_rebuilds_only_the_chain_that_changed() {
        let fps = Rational::new(10, 1).unwrap();
        let document = processor_document(
            fps,
            30,
            vec![
                part_b_bus(1, vec![1], vec![part_b_eq(10, 0)]),
                part_b_bus(
                    2,
                    vec![2],
                    vec![
                        part_b_eq(20, 0),
                        audio_effect(21, "audio_gain", &[("gain_tenth_db", -20)]),
                    ],
                ),
            ],
        );
        let first = tone(440.0, 0.5, 48_000, 480);
        let second = tone(1_000.0, 0.5, 48_000, 480);
        let inputs = |samples: &[f32]| {
            HashMap::from([
                (TrackId(1), samples.to_vec()),
                (TrackId(2), samples.to_vec()),
            ])
        };

        for (label, mutate) in [
            (
                "insertion",
                (|effects: &mut Vec<Effect>| {
                    effects.insert(1, audio_effect(22, "audio_gain", &[("gain_tenth_db", -10)]));
                }) as fn(&mut Vec<Effect>),
            ),
            ("removal", |effects: &mut Vec<Effect>| {
                effects.pop();
            }),
            ("reorder", |effects: &mut Vec<Effect>| {
                effects.swap(0, 1);
            }),
        ] {
            let mut updated = document.clone();
            mutate(&mut updated.audio_mix.buses[1].effects);

            let mut unchanged = AudioMixProcessor::new(&document, 48_000, 1, None);
            unchanged
                .mix_chunk_with_stems(&inputs(&first), 0, 480)
                .unwrap();
            let steady = unchanged
                .mix_chunk_with_stems(&inputs(&second), 480, 480)
                .unwrap();

            let mut edited = AudioMixProcessor::new(&document, 48_000, 1, None);
            edited
                .mix_chunk_with_stems(&inputs(&first), 0, 480)
                .unwrap();
            edited.update_audio_mix(&updated);
            let after = edited
                .mix_chunk_with_stems(&inputs(&second), 480, 480)
                .unwrap();

            assert_eq!(
                after.buses[0], steady.buses[0],
                "{label}: the untouched bus must keep every filter memory"
            );
            let mut cold = AudioMixProcessor::new(&updated, 48_000, 1, None);
            let fresh = cold
                .mix_chunk_with_stems(&inputs(&second), 480, 480)
                .unwrap();
            assert_eq!(
                after.buses[1], fresh.buses[1],
                "{label}: the edited bus must be rebuilt, not retargeted"
            );
            assert_ne!(
                after.buses[1], steady.buses[1],
                "{label}: the edited bus must actually change"
            );
        }
    }

    /// AU2 §7 item B11: flipping `bypass` is a parameter change, so it
    /// retargets in place — the chain's other node keeps its filter memory —
    /// and takes effect on the next sample frame.
    // Bypass removes exactly one multiply, so the identity is exact.
    #[allow(clippy::float_cmp)]
    #[test]
    fn a_bypass_flip_retargets_the_chain_in_place() {
        let fps = Rational::new(10, 1).unwrap();
        let chain = vec![
            part_b_eq(10, 0),
            audio_effect(11, "audio_gain", &[("gain_tenth_db", -60)]),
        ];
        let document = processor_document(fps, 30, vec![part_b_bus(1, vec![1], chain.clone())]);
        let mut updated = document.clone();
        updated.audio_mix.buses[0].effects[1] =
            audio_effect(11, "audio_gain", &[("gain_tenth_db", -60), ("bypass", 1)]);
        let first = tone(440.0, 0.5, 48_000, 480);
        let second = tone(1_000.0, 0.5, 48_000, 480);

        let mut unchanged = AudioMixProcessor::new(&document, 48_000, 1, None);
        part_b_chunk(&mut unchanged, 1, &first, first.len(), 0);
        let steady = part_b_chunk(&mut unchanged, 1, &second, second.len(), 480);

        let mut flipped = AudioMixProcessor::new(&document, 48_000, 1, None);
        part_b_chunk(&mut flipped, 1, &first, first.len(), 0);
        flipped.update_audio_mix(&updated);
        let bypassed = part_b_chunk(&mut flipped, 1, &second, second.len(), 480);

        let trim = db_gain(-60);
        for (frame, (bypassed, steady)) in bypassed.iter().zip(&steady).enumerate() {
            assert_eq!(
                bypassed * trim,
                *steady,
                "sample frame {frame}: the EQ before the bypassed gain lost its state"
            );
        }
    }

    /// AU2 §7 item B11 / §5.8: creating a bus mid-play leaves the unrouted
    /// path's delay line to drain, so the join has no gap.
    // The drained tail and the new chain's pad sum to an exact unity.
    #[allow(clippy::float_cmp)]
    #[test]
    fn creating_a_bus_mid_play_drains_the_unrouted_path_without_a_gap() {
        let fps = Rational::new(10, 1).unwrap();
        // Bus 1 declares 10 ms, so `L_bus` is 480 sample frames at 48 kHz and
        // the unrouted path track 1 sits on is padded to match.
        let document = processor_document(
            fps,
            30,
            vec![part_b_bus(
                1,
                vec![2],
                vec![audio_effect(
                    10,
                    "audio_true_peak_limiter",
                    &[
                        ("ceiling_tenth_db", 0),
                        ("lookahead_milliseconds", 10),
                        ("release_milliseconds", 50),
                        ("true_peak", 0),
                    ],
                )],
            )],
        );
        let mut routed = document.clone();
        routed.audio_mix.buses.push(part_b_bus(
            2,
            vec![1],
            vec![audio_effect(20, "audio_gain", &[("gain_tenth_db", 0)])],
        ));
        // The latency guard: the live path is only legal because `L` is equal.
        assert_eq!(
            document.audio_mix.lookahead_milliseconds(),
            routed.audio_mix.lookahead_milliseconds()
        );

        let inputs = |frames: usize| {
            HashMap::from([
                (TrackId(1), vec![1.0_f32; frames]),
                (TrackId(2), vec![0.0_f32; frames]),
            ])
        };
        let mut processor = AudioMixProcessor::new(&document, 48_000, 1, None);
        // Fill the unrouted delay line, then confirm the steady state.
        processor.mix_chunk(&inputs(960), 0, 960).unwrap();
        let steady = processor.mix_chunk(&inputs(480), 960, 480).unwrap();
        assert!(steady.iter().all(|sample| *sample == 1.0));

        processor.update_audio_mix(&routed);
        let joined = processor.mix_chunk(&inputs(960), 1_440, 960).unwrap();
        for (frame, sample) in joined.iter().enumerate() {
            assert_eq!(
                *sample, 1.0,
                "sample frame {frame} gapped while the unrouted path drained"
            );
        }
    }

    /// AU2 §7 item B12 / R8: the installed peak table survives ten consecutive
    /// gain-only updates and is rebuilt only when its key set stops describing
    /// the document. The master chain contributes gain-reduction keys.
    #[test]
    fn the_meter_table_survives_gain_only_updates_and_rebuilds_on_a_structural_edit() {
        let fps = Rational::new(10, 1).unwrap();
        let mut document = processor_document(
            fps,
            30,
            vec![part_b_bus(
                1,
                vec![1],
                vec![
                    audio_effect(10, "audio_gain", &[("gain_tenth_db", 0)]),
                    audio_effect(11, "audio_compressor", &[("threshold_tenth_db", -120)]),
                ],
            )],
        );
        document.audio_mix.master.effects = vec![audio_effect(
            30,
            "audio_true_peak_limiter",
            &[("ceiling_tenth_db", -10)],
        )];
        let master_meter = Arc::new(MeterState::default());
        let installed = Arc::new(MixMeters::for_document(
            &document,
            Arc::clone(&master_meter),
        ));
        // AU2 §3.8/§5.6: one slot per node with a gain computer, buses in
        // document order and then the master chain.
        assert_eq!(
            installed
                .peaks()
                .gain_reduction
                .iter()
                .map(|(chain, effect, _)| (*chain, *effect))
                .collect::<Vec<_>>(),
            vec![
                (AudioChain::Bus(AudioBusId(1)), EffectId(11)),
                (AudioChain::Master, EffectId(30)),
            ]
        );

        let mut live = document.clone();
        for step in 1..=10 {
            live.audio_mix.buses[0].gain_tenth_db = -step;
            live.audio_mix.master.gain_tenth_db = step;
            live.audio_mix.tracks = vec![track_mix(1, -step, 0, false, false)];
            assert!(
                installed.matches_document(&live),
                "update {step} must not rebuild the table"
            );
        }

        for (label, mutate) in [
            (
                "a bus chain insertion",
                (|document: &mut Document| {
                    document.audio_mix.buses[0]
                        .effects
                        .push(audio_effect(12, "audio_gate", &[]));
                }) as fn(&mut Document),
            ),
            ("a master chain removal", |document: &mut Document| {
                document.audio_mix.master.effects.clear();
            }),
            ("a new bus", |document: &mut Document| {
                document
                    .audio_mix
                    .buses
                    .push(part_b_bus(2, vec![2], Vec::new()));
            }),
            ("a removed track", |document: &mut Document| {
                document.tracks.pop();
            }),
        ] {
            let mut structural = live.clone();
            mutate(&mut structural);
            assert!(
                !installed.matches_document(&structural),
                "{label} must force a rebuild"
            );
            let rebuilt = MixMeters::for_document(&structural, Arc::clone(&master_meter));
            assert!(rebuilt.matches_document(&structural));
        }
    }

    /// AU2 §7 item B13 / §5.9: the spectrum is measured through the real mix
    /// path — the same `mix_audio_stems` `measure_mix_levels` uses — at every
    /// mix point, and a range shorter than two Welch segments is refused.
    ///
    /// Placed here rather than in `export.rs`'s video-only test module, for
    /// AU2 §0 E19's reason: the audio fixtures live here.
    #[test]
    fn mix_spectrum_measures_every_mix_point_through_the_real_mix_path() {
        crate::initialize_ffmpeg().unwrap();
        let fps = Rational::new(10, 1).unwrap();
        // Two seconds of a full-scale 1 kHz sine, written by hand so the
        // amplitude survives exactly.
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        let samples = (0..96_000_usize)
            .flat_map(|frame| {
                let phase = 2.0 * std::f64::consts::PI * 1_000.0 * frame as f64 / 48_000.0;
                let sample = phase.sin() as f32;
                [sample, sample]
            })
            .collect::<Vec<_>>();
        let source =
            GeneratedMedia::from_bytes("au2b-spectrum", "wav", &wav_f32(&samples, 48_000, 2));
        let mut document = processor_document(
            fps,
            20,
            vec![part_b_bus(
                1,
                vec![1],
                vec![audio_effect(10, "audio_gain", &[("gain_tenth_db", 0)])],
            )],
        );
        document.media_pool = vec![audio_asset(1, source.path(), "kilohertz", fps)];
        document.tracks[0].clips = vec![audio_clip(1, 1, 0..20, 0)];
        document.validate().expect("the fixture should validate");

        for point in [
            kinewright_core::MixSpectrumPoint::Master,
            kinewright_core::MixSpectrumPoint::Track(TrackId(1)),
            kinewright_core::MixSpectrumPoint::Bus(AudioBusId(1)),
        ] {
            let report = crate::export::measure_mix_spectrum(
                &document,
                &kinewright_core::MixSpectrumRequest {
                    range: Some(TimeCode::ZERO..TimeCode(10)),
                    point,
                },
            )
            .expect("a one-second range is four Welch segments");
            assert_eq!(report.point, point);
            assert_eq!(report.sample_rate, 48_000);
            assert_eq!(report.sample_frames, 48_000);
            assert_eq!(report.segments, 4);
            assert_eq!(report.bands.len(), 31);
            assert_eq!(report.bands[17].center_hertz_tenths, 10_000);
            let peak = f64::from(
                report.bands[17]
                    .level_dbfs_hundredths
                    .expect("the 1 kHz band carries the tone"),
            ) / 100.0;
            assert!(
                peak.abs() <= 0.05,
                "{point:?}: the 1 kHz band read {peak} dBFS, not 0.00"
            );
            for index in [11_usize, 23] {
                let neighbour = report.bands[index]
                    .level_dbfs_hundredths
                    .map_or(f64::NEG_INFINITY, |hundredths| {
                        f64::from(hundredths) / 100.0
                    });
                assert!(
                    neighbour <= peak - 40.0,
                    "{point:?}: band {index} read {neighbour} dBFS, under 40 dB down"
                );
            }
            assert!(
                report.bands[..5].iter().all(|band| band.window_limited)
                    && report.bands[5..].iter().all(|band| !band.window_limited)
            );
        }

        // AU2 §5.9/A23: 400 ms is four project frames, 19 200 sample frames,
        // short of the 24 576 two segments need.
        let rejected = crate::export::measure_mix_spectrum(
            &document,
            &kinewright_core::MixSpectrumRequest {
                range: Some(TimeCode::ZERO..TimeCode(4)),
                point: kinewright_core::MixSpectrumPoint::Master,
            },
        )
        .expect_err("a 400 ms range must be refused");
        assert!(
            matches!(
                rejected,
                MediaError::MixSpectrumRangeTooShort {
                    sample_frames: 19_200,
                    required: 24_576,
                }
            ),
            "the rejection must be typed, not a formatted string: {rejected}"
        );
    }

    // ---- AU3 Part A -------------------------------------------------------

    use crate::{
        engine::{LiveLoudness, WorkerLoudness},
        export::{MixCollect, MixObserver, NoObserver, QcObserver, mix_audio_stems, mix_pass},
        loudness::{LIVE_LOUDNESS_RING_ENTRIES, LoudnessMeter},
    };
    use kinewright_core::{
        AudioMaster, AudioQcProvenance, AudioQcRequest, DeliveryProfile, LoudnessSnapshot,
        QaSeverity,
    };

    /// A stereo 1 kHz tone at `peak` per channel, hand-written in `f32` so the
    /// amplitude survives exactly.
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    fn stereo_tone(frequency: f64, peak: f64, frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|frame| {
                let sample = (peak
                    * (2.0 * std::f64::consts::PI * frequency * frame as f64 / 48_000.0).sin())
                    as f32;
                [sample, sample]
            })
            .collect()
    }

    /// One audio track carrying one clip of `frames` project frames at `fps`.
    fn one_clip_document(source: &Path, fps: Rational, frames: i64) -> Document {
        let mut asset = audio_asset(1, source, "au3", fps);
        asset.duration = TimeCode(frames);
        Document {
            catalog: kinewright_core::MediaCatalog::default(),
            audio_mix: AudioMix::default(),
            color_context: kinewright_core::ColorContext::default(),
            lut_assets: Vec::new(),
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Audio,
                sync_lock: true,
                clips: vec![audio_clip(1, 1, 0..frames, 0)],
            }],
            media_pool: vec![asset],
            markers: Vec::new(),
            fps,
            resolution: (64, 64),
            duration: TimeCode(frames),
        }
    }

    /// A14's walk: every leaf of a serialized report is an integer, bool,
    /// string, or null — never a float.
    fn assert_integer_leaves(value: &serde_json::Value, path: &str) {
        match value {
            serde_json::Value::Number(number) => assert!(
                number.is_i64() || number.is_u64(),
                "{path} is a non-integer number: {number}"
            ),
            serde_json::Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    assert_integer_leaves(item, &format!("{path}[{index}]"));
                }
            }
            serde_json::Value::Object(fields) => {
                for (key, item) in fields {
                    assert_integer_leaves(item, &format!("{path}.{key}"));
                }
            }
            serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::String(_) => {
            }
        }
    }

    fn qc(document: &Document, request: &AudioQcRequest) -> kinewright_core::AudioQcReport {
        crate::export::measure_audio_qc(document, request).expect("the QC fixture should measure")
    }

    fn exception_codes(report: &kinewright_core::AudioQcReport) -> Vec<&str> {
        report
            .exceptions
            .iter()
            .map(|exception| exception.code.as_str())
            .collect()
    }

    /// The observer-fed families equal `LoudnessMeter::measure` over the
    /// corresponding whole stem — the whole `AudioLoudness`, `assert_eq!`.
    fn assert_levels_equal_stems(document: &Document, range: std::ops::Range<TimeCode>) {
        let settings = parity_settings(document.fps);
        let report = crate::export::measure_mix_levels(
            document,
            &MixLevelRequest {
                range: Some(range.clone()),
            },
        )
        .unwrap();
        let stems = mix_audio_stems(document, range.clone(), &settings).unwrap();
        assert_eq!(report.range, range);
        for (levels, (track, stem)) in report.tracks.iter().zip(&stems.tracks) {
            assert_eq!(levels.track, *track);
            assert_eq!(
                levels.levels,
                LoudnessMeter::measure(stem, 48_000, 2).unwrap(),
                "track {} over {range:?}",
                track.0
            );
        }
        for (levels, (bus, stem)) in report.buses.iter().zip(&stems.buses) {
            assert_eq!(levels.bus, *bus);
            assert_eq!(
                levels.levels,
                LoudnessMeter::measure(stem, 48_000, 2).unwrap(),
                "bus {} over {range:?}",
                bus.0
            );
        }
        assert_eq!(
            report.master,
            LoudnessMeter::measure(&stems.master, 48_000, 2).unwrap(),
            "master over {range:?}"
        );
    }

    /// AU3 §7 item A11 (§3.8): through the observer, `measure_mix_levels` is
    /// `assert_eq!` to `LoudnessMeter::measure` over the corresponding
    /// `mix_audio_stems` family on the AU1 level fixture, on
    /// `impulse_document`, and on the AU2 master-chain parity document, over
    /// whole and windowed ranges.
    #[test]
    fn measure_mix_levels_equals_the_meter_over_every_stem_family() {
        crate::initialize_ffmpeg().unwrap();
        let fps = Rational::new(10, 1).unwrap();
        let steady = level_sine("au3-levels-steady", 440, "0.4");
        let late = level_sine("au3-levels-late", 660, "0.2");
        let levels = levels_document(steady.path(), late.path(), fps);
        levels.validate().unwrap();
        assert_levels_equal_stems(&levels, TimeCode::ZERO..TimeCode(20));
        assert_levels_equal_stems(&levels, TimeCode(10)..TimeCode(20));
        assert_levels_equal_stems(&levels, TimeCode(3)..TimeCode(17));

        let impulses = impulse_media();
        let impulse = impulse_document(impulses.path(), fps);
        impulse.validate().unwrap();
        assert_levels_equal_stems(&impulse, TimeCode::ZERO..TimeCode(20));
        assert_levels_equal_stems(&impulse, TimeCode(2)..TimeCode(9));

        let voice = loud_sine("au3-parity-voice", 440);
        let bed = loud_sine("au3-parity-bed", 660);
        let parity = transitioned(parity_document_with_master_chain(
            voice.path(),
            bed.path(),
            fps,
        ));
        parity.validate().unwrap();
        assert_levels_equal_stems(&parity, TimeCode::ZERO..TimeCode(30));
        assert_levels_equal_stems(&parity, TimeCode(7)..TimeCode(23));
    }

    /// A master-recording observer: the pre-clamp master chunks, in order.
    #[derive(Default)]
    struct MasterRecorder {
        master: Vec<f32>,
        chunks: usize,
    }

    impl MixObserver for MasterRecorder {
        fn master(&mut self, chunk: &[f32]) -> Result<(), MediaError> {
            self.master.extend_from_slice(chunk);
            self.chunks += 1;
            Ok(())
        }
    }

    /// AU3 §7 item A11: `mix_audio` on `parity_document_with_master_chain` is
    /// the `NoObserver` / `mix_chunk` path with no stems collected; the
    /// with-stems path and a clamp of the observer's pre-clamp master feed
    /// are sample-identical to it, and the observer receives exactly the
    /// document's length. (The output's SHA-256 was compared against the
    /// pre-AU3 code on the same machine when this landed: unchanged.)
    #[test]
    fn mix_audio_is_bit_identical_through_the_observer_feed() {
        crate::initialize_ffmpeg().unwrap();
        let fps = Rational::new(10, 1).unwrap();
        let voice = loud_sine("au3-identity-voice", 440);
        let bed = loud_sine("au3-identity-bed", 660);
        let settings = parity_settings(fps);
        let document = transitioned(parity_document_with_master_chain(
            voice.path(),
            bed.path(),
            fps,
        ));
        let exported = crate::export::mix_audio(&document, &settings).unwrap();
        assert_eq!(exported.len(), 30 * 4_800 * 2);
        let bytes = exported
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect::<Vec<_>>();
        println!(
            "AU3_MIX_AUDIO_SHA256 parity_master_chain {}",
            crate::sha256::sha256_bytes(&bytes)
        );

        let with_stems = mix_pass(
            &document,
            TimeCode::ZERO..document.duration,
            &settings,
            MixCollect {
                stems: true,
                master: true,
            },
            &mut NoObserver,
        )
        .unwrap();
        assert!(
            with_stems
                .master
                .iter()
                .zip(&exported)
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );

        let mut recorder = MasterRecorder::default();
        let uncollected = mix_pass(
            &document,
            TimeCode::ZERO..document.duration,
            &settings,
            MixCollect {
                stems: false,
                master: false,
            },
            &mut recorder,
        )
        .unwrap();
        assert!(uncollected.master.is_empty() && uncollected.tracks.is_empty());
        limit_audio_mix(&mut recorder.master);
        assert_eq!(recorder.master.len(), exported.len());
        assert!(
            recorder
                .master
                .iter()
                .zip(&exported)
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "the observer's clamped master feed must be the export"
        );
        // 144 000 frames plus 720 of latency: 142 chunks, the first split.
        assert_eq!(recorder.chunks, (144_000_usize + 720).div_ceil(1_024));
    }

    /// AU3 §7 item A11 / A13 (Q3): a range shorter than one gating block is
    /// refused typed **before decoding** — the fixture's asset path does not
    /// exist, so any decode would have failed with a different error, as the
    /// whole-range measurement shows.
    #[test]
    fn a_short_range_is_refused_before_anything_is_decoded() {
        crate::initialize_ffmpeg().unwrap();
        let missing = std::env::temp_dir().join("au3-does-not-exist-4f6cdd1d.wav");
        let at_10 = one_clip_document(&missing, Rational::new(10, 1).unwrap(), 20);
        let error = crate::export::measure_mix_levels(
            &at_10,
            &MixLevelRequest {
                range: Some(TimeCode::ZERO..TimeCode(1)),
            },
        )
        .expect_err("one frame at 10 fps is 4 800 frames");
        assert_eq!(
            error,
            MediaError::MixLoudnessRangeTooShort {
                sample_frames: 4_800,
                required: 19_200,
            }
        );
        let at_30 = one_clip_document(&missing, Rational::new(30, 1).unwrap(), 90);
        let error = crate::export::measure_audio_qc(
            &at_30,
            &AudioQcRequest {
                range: Some(TimeCode(10)..TimeCode(15)),
                profile: None,
            },
        )
        .expect_err("five frames at 30 fps are 8 000 frames");
        assert_eq!(
            error,
            MediaError::MixLoudnessRangeTooShort {
                sample_frames: 8_000,
                required: 19_200,
            }
        );
        // Exactly one block (12 frames at 30 fps) is not refused: it decodes,
        // and the decode is what fails.
        let error = crate::export::measure_audio_qc(
            &at_30,
            &AudioQcRequest {
                range: Some(TimeCode::ZERO..TimeCode(12)),
                profile: None,
            },
        )
        .expect_err("the asset does not exist");
        assert!(
            !matches!(error, MediaError::MixLoudnessRangeTooShort { .. }),
            "a one-block range reaches the decoder: {error}"
        );
        assert_eq!(error.recovery_code(), None);
    }

    /// AU3 §7 item A12: a device-free transport — the mixer, the ring, a
    /// consumer standing in for the loudspeaker, and the worker's loudness
    /// state — so the publish step runs against a simulated clock.
    struct SimulatedTransport {
        document: Document,
        mixer: Option<AudioMixer>,
        producer: Option<rtrb::Producer<f32>>,
        consumer: Option<rtrb::Consumer<f32>>,
        pending: Vec<f32>,
        pending_index: usize,
        /// The absolute project sample the loudspeaker has reached.
        heard: u64,
        loudness: WorkerLoudness,
        shared: Arc<LiveLoudness>,
    }

    impl SimulatedTransport {
        const TARGET_SAMPLES: usize = 48_000 * 2;

        fn new(document: Document) -> Self {
            let shared = Arc::new(LiveLoudness::default());
            Self {
                document,
                mixer: None,
                producer: None,
                consumer: None,
                pending: Vec::new(),
                pending_index: 0,
                heard: 0,
                loudness: WorkerLoudness::new(Arc::clone(&shared)),
                shared,
            }
        }

        /// `Worker::start_playback`: open at `from`, continue or reset, and
        /// run the initial fill.
        fn play(&mut self, from: TimeCode) {
            let fps = self.document.fps;
            let (producer, consumer) = RingBuffer::<f32>::new(48_000 * 2 * 2);
            self.mixer = Some(AudioMixer::open(&self.document, from, 48_000, 2, None).unwrap());
            self.producer = Some(producer);
            self.consumer = Some(consumer);
            self.pending.clear();
            self.pending_index = 0;
            self.heard = frame_to_samples(from, 48_000, fps);
            self.loudness.begin(from, 48_000, 2, fps).unwrap();
            self.fill();
        }

        fn fill(&mut self) -> bool {
            let meter = self.loudness.meter_mut().expect("playing");
            fill_ring(
                self.producer.as_mut().unwrap(),
                &mut self.pending,
                &mut self.pending_index,
                self.mixer.as_mut().unwrap(),
                Self::TARGET_SAMPLES,
                meter,
            )
            .unwrap()
        }

        /// The loudspeaker consumes `frames`, the worker tops up and publishes.
        fn advance(&mut self, frames: u64) {
            let consumer = self.consumer.as_mut().unwrap();
            let mut popped = 0_u64;
            for _ in 0..frames * 2 {
                if consumer.pop().is_ok() {
                    popped += 1;
                }
            }
            self.heard += popped / 2;
            self.fill();
            self.loudness.publish_at(self.heard);
        }

        /// `Worker::pause`: the clock position is the heard sample's frame.
        fn pause(&mut self) -> TimeCode {
            let position = samples_to_frame(self.heard, 48_000, self.document.fps);
            self.loudness.pause_at(position, self.document.fps);
            self.mixer = None;
            position
        }

        fn published(&self) -> LoudnessSnapshot {
            self.shared.load()
        }

        /// `AudioRuntime::fed_position_samples`: the newest *output* frame the
        /// meter has been fed, which trails the input cursor by AU2 §3.7's
        /// graph latency.
        fn fed_position(&self) -> u64 {
            self.mixer.as_ref().unwrap().output_position_samples()
        }

        fn input_cursor(&self) -> u64 {
            self.mixer.as_ref().unwrap().cursor_sample
        }
    }

    /// AU3 §7 item A12 (§3.9): a −20 LUFS tone converges to −2000 ±20 within
    /// 3 s of *heard* programme with the true peak at the amplitude ±1; the
    /// published entry never leads the clock and lags the meter's head by at
    /// most ten sub-blocks; `integrated == None` under 400 ms and
    /// `short_term == None` under 3 s of heard programme; `programme_seconds`
    /// follows the published key.
    #[test]
    fn the_live_meter_converges_on_the_heard_prefix_and_never_leads() {
        crate::initialize_ffmpeg().unwrap();
        let fps = Rational::new(10, 1).unwrap();
        let source = GeneratedMedia::from_bytes(
            "au3-live-tone",
            "wav",
            &wav_f32(&stereo_tone(1_000.0, 0.1, 48_000 * 6), 48_000, 2),
        );
        let document = one_clip_document(source.path(), fps, 60);
        document.validate().unwrap();
        let mut transport = SimulatedTransport::new(document);
        transport.play(TimeCode::ZERO);
        // The fill ran a second ahead — 47 whole chunks queued plus the one
        // `fill_ring` holds pending, all metered; nothing has been heard.
        assert_eq!(
            transport.fed_position(),
            (48_000_u64.div_ceil(1_024) + 1) * 1_024
        );
        assert_eq!(transport.published(), LoudnessSnapshot::default());

        // The fill leads the loudspeaker by the 1 s target plus at most two
        // 1 024-frame chunks (one over-fill, one pending), and the published
        // key is a block end up to one sub-block behind the heard position:
        // the head-to-published lag is therefore under 48 000 + 2 048 + 4 800
        // frames — eleven whole sub-blocks, well inside the 16-entry ring.
        let lag_bound = 48_000 + 2 * 1_024 + 4_800;
        let mut worst_lag = 0_u64;
        let mut heard = 0_u64;
        while heard < 48_000 * 6 {
            transport.advance(480);
            heard += 480;
            let published = transport.published();
            let key = transport.loudness.published_key();
            assert!(
                key.is_none_or(|key| key <= heard),
                "the published key {key:?} leads the clock at {heard}"
            );
            if let (Some(key), Some(head)) = (key, transport.loudness.fed_block_end()) {
                worst_lag = worst_lag.max(head - key);
                assert!(
                    head - key <= lag_bound,
                    "the lag {} exceeds {lag_bound} frames",
                    head - key
                );
                assert!(
                    (head - key) / 4_800 < LIVE_LOUDNESS_RING_ENTRIES as u64,
                    "the lag {} would outrun the ring",
                    head - key
                );
            }
            assert_eq!(
                published.programme_seconds,
                u32::try_from(key.unwrap_or(0) / 48_000).unwrap()
            );
            if heard < 19_200 {
                assert_eq!(published.integrated_lufs_hundredths, None, "at {heard}");
                assert_eq!(published.momentary_lufs_hundredths, None, "at {heard}");
            }
            if heard < 48_000 * 3 {
                assert_eq!(published.short_term_lufs_hundredths, None, "at {heard}");
            }
            if heard == 48_000 * 3 {
                let integrated = published.integrated_lufs_hundredths.expect("3 s heard");
                assert!(
                    (integrated + 2_000).abs() <= 20,
                    "integrated {integrated} at 3 s"
                );
                let true_peak = published.true_peak_dbtp_hundredths.expect("3 s heard");
                assert!(
                    (true_peak + 2_000).abs() <= 1,
                    "true peak {true_peak} at 3 s"
                );
                assert!(published.short_term_lufs_hundredths.is_some());
                assert!(published.momentary_lufs_hundredths.is_some());
                assert_eq!(published.programme_seconds, 3);
            }
        }
        let end = transport.published();
        assert_eq!(end.programme_seconds, 6);
        assert!((end.integrated_lufs_hundredths.unwrap() + 2_000).abs() <= 20);
        println!(
            "AU3_LIVE_LAG_WORST_FRAMES {worst_lag} ({} sub-blocks)",
            worst_lag / 4_800
        );
    }

    /// AU3 §7 item A12 (§3.9 pause/continue): pause after N seconds then
    /// continue at `paused_at`: `programme_seconds` and integrated exclude the
    /// pre-rendered second exactly once and equal an uninterrupted run
    /// within ±1 hundredth; `momentary == None` while paused; play elsewhere,
    /// a seek, and `reset_loudness` reset the published shape to `default()`.
    #[test]
    fn the_live_meter_pauses_continues_and_resets_per_the_contract() {
        crate::initialize_ffmpeg().unwrap();
        let fps = Rational::new(10, 1).unwrap();
        let source = GeneratedMedia::from_bytes(
            "au3-live-pause",
            "wav",
            &wav_f32(&stereo_tone(1_000.0, 0.1, 48_000 * 6), 48_000, 2),
        );
        let document = one_clip_document(source.path(), fps, 60);
        document.validate().unwrap();

        let mut uninterrupted = SimulatedTransport::new(document.clone());
        uninterrupted.play(TimeCode::ZERO);
        for _ in 0..600 {
            uninterrupted.advance(480);
        }
        let reference = uninterrupted.published();
        assert_eq!(reference.programme_seconds, 6);

        let mut paused = SimulatedTransport::new(document.clone());
        paused.play(TimeCode::ZERO);
        for _ in 0..250 {
            paused.advance(480);
        }
        // 2.5 s heard, 3.5 s fed.
        assert!(paused.fed_position() >= 48_000 * 3);
        let paused_at = paused.pause();
        assert_eq!(paused_at, TimeCode(25));
        assert_eq!(paused.loudness.paused_at(), Some(TimeCode(25)));
        let while_paused = paused.published();
        assert_eq!(while_paused.momentary_lufs_hundredths, None);
        assert_eq!(while_paused.short_term_lufs_hundredths, None);
        assert_eq!(while_paused.programme_seconds, 2);
        assert!(while_paused.integrated_lufs_hundredths.is_some());
        assert!(while_paused.true_peak_dbtp_hundredths.is_some());
        // Continue at `paused_at` (the app resumes from `Playback::position()`).
        paused.play(paused_at);
        assert_eq!(paused.loudness.paused_at(), None);
        for _ in 0..350 {
            paused.advance(480);
        }
        let resumed = paused.published();
        assert_eq!(resumed.programme_seconds, reference.programme_seconds);
        let (resumed_i, reference_i) = (
            resumed.integrated_lufs_hundredths.unwrap(),
            reference.integrated_lufs_hundredths.unwrap(),
        );
        assert!(
            (resumed_i - reference_i).abs() <= 1,
            "paused/continued read {resumed_i}, uninterrupted {reference_i}"
        );
        assert_eq!(
            resumed.true_peak_dbtp_hundredths,
            reference.true_peak_dbtp_hundredths
        );

        // Play elsewhere after a pause: reset.
        paused.pause();
        paused.play(TimeCode(10));
        assert_eq!(paused.published(), LoudnessSnapshot::default());
        assert_eq!(paused.loudness.published_key(), None);
        for _ in 0..150 {
            paused.advance(480);
        }
        assert_eq!(
            paused.published().programme_seconds,
            1,
            "counted from the new origin"
        );

        // A seek while playing re-enters `start_playback` with no `paused_at`.
        paused.play(TimeCode(30));
        assert_eq!(paused.published(), LoudnessSnapshot::default());
        for _ in 0..120 {
            paused.advance(480);
        }
        assert_eq!(paused.published().programme_seconds, 1);

        // `Playback::reset_loudness` while playing: the origin is the fed
        // position, so nothing is published until the sound reaches it.
        let fed = paused.fed_position();
        paused.loudness.reset(Some(fed), fps);
        assert_eq!(paused.published(), LoudnessSnapshot::default());
        let mut steps = 0_u64;
        while paused.published() == LoudnessSnapshot::default() && steps < 200 {
            paused.advance(480);
            steps += 1;
        }
        assert!(
            paused.heard >= fed + 4_800,
            "the first block after a reset is published once heard: heard {} fed {fed}",
            paused.heard
        );
        assert_eq!(paused.published().programme_seconds, 0);
    }

    /// AU3 §7 item A12 (§3.9 reset) against AU2 §3.7's graph latency: on a
    /// document whose master chain declares lookahead, a mid-play
    /// `reset_loudness` keys the ring from the **fed output** position —
    /// `cursor_sample − latency` — and not from the mixer's input cursor. The
    /// first published key after the reset therefore lands within one
    /// loudspeaker step of `heard − fed_output`; keying from the input cursor
    /// would publish a whole graph latency (480 frames here) late.
    #[test]
    fn a_mid_play_loudness_reset_keys_from_the_fed_output_position() {
        crate::initialize_ffmpeg().unwrap();
        let fps = Rational::new(10, 1).unwrap();
        let source = GeneratedMedia::from_bytes(
            "au3-live-reset-lookahead",
            "wav",
            &wav_f32(&stereo_tone(1_000.0, 0.1, 48_000 * 6), 48_000, 2),
        );
        let mut document = one_clip_document(source.path(), fps, 60);
        document.audio_mix.master.effects = vec![audio_effect(
            1,
            "audio_true_peak_limiter",
            &[("ceiling_tenth_db", -10), ("lookahead_milliseconds", 10)],
        )];
        document.validate().unwrap();
        let latency = u64::try_from(graph_latency_frames(
            &document.audio_mix.lookahead_milliseconds(),
            48_000,
        ))
        .unwrap();
        assert_eq!(latency, 480, "10 ms of master lookahead at 48 kHz");

        let mut transport = SimulatedTransport::new(document);
        transport.play(TimeCode::ZERO);
        assert_eq!(
            transport.mixer.as_ref().unwrap().latency_frames(),
            latency,
            "the mixer reports the graph latency it prerolled"
        );
        // The mixer prerolled `latency` frames past the seek target, so the
        // first frame *out* — and the first frame metered — is project sample
        // zero, exactly where the clock starts.
        assert_eq!(transport.fed_position() + latency, transport.input_cursor());

        for _ in 0..250 {
            transport.advance(480);
        }
        // The expected origin is derived from the input cursor so the final
        // assertion measures the published key against the audio, not
        // against `fed_position()` itself; that the graph really trails its
        // input cursor by `latency` is pinned against an impulse by
        // `a_lookahead_chain_delays_an_impulse_by_exactly_the_graph_latency`.
        let fed_output = transport.input_cursor() - latency;
        let fed = transport.fed_position();
        assert!(fed > transport.heard, "the fill leads the loudspeaker");

        transport.loudness.reset(Some(fed), fps);
        assert_eq!(transport.published(), LoudnessSnapshot::default());
        assert_eq!(transport.loudness.published_key(), None);

        // A loudspeaker step well under the 480-frame graph latency, so the
        // quantisation of `heard` cannot hide it.
        let step = 160_u64;
        let mut steps = 0_u64;
        while transport.loudness.published_key().is_none() && steps < 1_000 {
            transport.advance(step);
            steps += 1;
        }
        let key = transport
            .loudness
            .published_key()
            .expect("one sub-block past the reset origin is heard");
        assert_eq!(key, 4_800, "the first sub-block after the reset");
        let audible = transport.heard - fed_output;
        assert!(
            audible >= key && audible - key < step,
            "the first key {key} after a mid-play reset must land within one \
             loudspeaker step of heard−fed_output {audible} (heard {}, fed output \
             {fed_output}); keying from the input cursor would publish {latency} \
             frames late",
            transport.heard
        );
        assert_eq!(transport.published().programme_seconds, 0);
    }

    /// AU3 §7 item A12: the six atomics round-trip a snapshot, the sentinel
    /// reads as `None`, and `clear` restores `default()`.
    #[test]
    fn live_loudness_atomics_round_trip_every_field() {
        let shared = LiveLoudness::default();
        assert_eq!(shared.load(), LoudnessSnapshot::default());
        let snapshot = LoudnessSnapshot {
            momentary_lufs_hundredths: Some(-1_850),
            short_term_lufs_hundredths: None,
            integrated_lufs_hundredths: Some(-2_301),
            loudness_range_lu_hundredths: Some(0),
            true_peak_dbtp_hundredths: Some(-i32::MAX),
            programme_seconds: 7,
        };
        shared.publish(&snapshot);
        assert_eq!(shared.load(), snapshot);
        shared.clear();
        assert_eq!(shared.load(), LoudnessSnapshot::default());
    }

    /// AU3 §7 item A13 (§3.10): per-channel clipping on a full-scale 1 kHz
    /// tone with a +6 dB `audio_gain` on the master — both channels'
    /// `clipped_runs >= 1`, `audio_clipping` Errors, `technical_pass ==
    /// false`, and the post-clamp `sample_peak == 0`.
    #[test]
    fn audio_qc_counts_clipping_per_channel_on_the_pre_clamp_master() {
        crate::initialize_ffmpeg().unwrap();
        let fps = Rational::new(30, 1).unwrap();
        let source = GeneratedMedia::from_bytes(
            "au3-qc-clip",
            "wav",
            &wav_f32(&stereo_tone(1_000.0, 1.0, 48_000 * 2), 48_000, 2),
        );
        let mut document = one_clip_document(source.path(), fps, 60);
        document.audio_mix.master = AudioMaster {
            gain_tenth_db: 0,
            effects: vec![audio_effect(1, "audio_gain", &[("gain_tenth_db", 60)])],
            gain_curve: None,
        };
        document.validate().unwrap();
        let report = qc(&document, &AudioQcRequest::default());
        assert_eq!(report.range, TimeCode::ZERO..TimeCode(60));
        assert!(report.clipping.left.clipped_runs >= 1);
        assert!(report.clipping.right.clipped_runs >= 1);
        assert!(report.clipping.left.over_full_scale_samples > 0);
        assert!(report.clipping.left.basis_points > 0);
        assert_eq!(
            report.clipping.left, report.clipping.right,
            "a dual-mono tone clips alike"
        );
        assert_eq!(
            exception_codes(&report),
            ["audio_clipping", "audio_clipping"]
        );
        assert!(
            report
                .exceptions
                .iter()
                .all(|e| e.severity == QaSeverity::Error)
        );
        assert_eq!(
            report.exceptions[0].field.as_deref(),
            Some("clipping.left.clipped_runs")
        );
        assert!(!report.technical_pass);
        assert!(report.evidence_only);
        assert_eq!(report.master.sample_peak_dbfs_hundredths, Some(0));
        assert_eq!(
            report.master.true_peak_dbtp_hundredths.map(|p| p >= 0),
            Some(true)
        );
        assert_eq!(report.master.sample_frames, 96_000);
    }

    /// AU3 §7 item A13 (§3.10 F14): an L-only tone reads `Δ == None` with the
    /// `audio_channel_imbalance` Warning; L at −20 and R at −26 LUFS read
    /// `Δ == 600 ±5` and no imbalance.
    #[test]
    fn audio_qc_reports_channel_balance_over_the_gated_blocks() {
        crate::initialize_ffmpeg().unwrap();
        let fps = Rational::new(30, 1).unwrap();
        let mut left_only = stereo_tone(1_000.0, 0.2, 48_000 * 2);
        for sample in left_only.iter_mut().skip(1).step_by(2) {
            *sample = 0.0;
        }
        let source =
            GeneratedMedia::from_bytes("au3-qc-left-only", "wav", &wav_f32(&left_only, 48_000, 2));
        let document = one_clip_document(source.path(), fps, 60);
        document.validate().unwrap();
        let report = qc(&document, &AudioQcRequest::default());
        assert_eq!(report.channel_balance_lu_hundredths, None);
        assert_eq!(exception_codes(&report), ["audio_channel_imbalance"]);
        assert_eq!(report.exceptions[0].severity, QaSeverity::Warning);
        assert_eq!(
            report.exceptions[0].observed.as_deref(),
            Some("none (one channel silent)")
        );
        assert!(report.technical_pass);

        let mut unbalanced = stereo_tone(1_000.0, 0.2, 48_000 * 2);
        let right_gain = 10.0_f64.powf(-6.0 / 20.0);
        #[allow(clippy::cast_possible_truncation)]
        for sample in unbalanced.iter_mut().skip(1).step_by(2) {
            *sample = (f64::from(*sample) * right_gain) as f32;
        }
        let source = GeneratedMedia::from_bytes(
            "au3-qc-unbalanced",
            "wav",
            &wav_f32(&unbalanced, 48_000, 2),
        );
        let document = one_clip_document(source.path(), fps, 60);
        document.validate().unwrap();
        let report = qc(&document, &AudioQcRequest::default());
        let balance = report
            .channel_balance_lu_hundredths
            .expect("both sides carry energy");
        assert!((balance - 600).abs() <= 5, "Δ read {balance}");
        // Six LU is outside core's ±300 band, so the Warning is raised with
        // the measured ratio as its observed value.
        assert_eq!(exception_codes(&report), ["audio_channel_imbalance"]);
        assert_eq!(report.exceptions[0].severity, QaSeverity::Warning);
        assert_eq!(
            report.exceptions[0].observed.as_deref(),
            Some(balance.to_string().as_str())
        );
        assert_eq!(report.exceptions[0].allowed.as_deref(), Some("-300..=300"));
        assert!(report.technical_pass);
    }

    /// AU3 §7 item A13 (§3.10 F13): 1.5 s of silence before a tone at 30 fps
    /// reads `leading_silence_frames == 45` with the `audio_leading_silence`
    /// Info; a programme opening on a full-scale tone reads 0; trailing
    /// silence is symmetric.
    #[test]
    fn audio_qc_measures_leading_and_trailing_silence_in_the_sample_domain() {
        crate::initialize_ffmpeg().unwrap();
        let fps = Rational::new(30, 1).unwrap();
        let mut late = vec![0.0_f32; 48_000 * 3];
        late.extend(stereo_tone(1_000.0, 0.5, 48_000 * 3 / 2));
        let source = GeneratedMedia::from_bytes("au3-qc-late", "wav", &wav_f32(&late, 48_000, 2));
        let document = one_clip_document(source.path(), fps, 90);
        document.validate().unwrap();
        let report = qc(&document, &AudioQcRequest::default());
        assert_eq!(report.leading_silence_frames, TimeCode(45));
        assert_eq!(report.trailing_silence_frames, TimeCode::ZERO);
        assert_eq!(exception_codes(&report), ["audio_leading_silence"]);
        assert_eq!(report.exceptions[0].severity, QaSeverity::Info);
        assert_eq!(
            report.exceptions[0].observed.as_deref(),
            Some("45 frames (1500 ms)")
        );
        assert!(report.technical_pass);

        let mut early = stereo_tone(1_000.0, 1.0, 48_000 * 3 / 2);
        early.extend(vec![0.0_f32; 48_000 * 3]);
        let source = GeneratedMedia::from_bytes("au3-qc-early", "wav", &wav_f32(&early, 48_000, 2));
        let document = one_clip_document(source.path(), fps, 90);
        document.validate().unwrap();
        let report = qc(&document, &AudioQcRequest::default());
        assert_eq!(report.leading_silence_frames, TimeCode::ZERO);
        assert_eq!(report.trailing_silence_frames, TimeCode(45));
        assert_eq!(exception_codes(&report), ["audio_trailing_silence"]);
    }

    /// AU3 §7 item A13 (§3.10): with `profile: Some(Youtube1080p)` a −20 LUFS
    /// tone raises `audio_loudness_out_of_tolerance` (`observed "-2000"`,
    /// `allowed "-1500..=-1300"`) and a −0.50 dBTP peak raises
    /// `audio_true_peak_over_ceiling` as an Error; the target is published.
    #[test]
    fn audio_qc_judges_against_the_profile_target() {
        crate::initialize_ffmpeg().unwrap();
        let fps = Rational::new(30, 1).unwrap();
        let source = GeneratedMedia::from_bytes(
            "au3-qc-target",
            "wav",
            &wav_f32(&stereo_tone(997.0, 0.1, 48_000 * 2), 48_000, 2),
        );
        let document = one_clip_document(source.path(), fps, 60);
        document.validate().unwrap();
        let request = AudioQcRequest {
            range: None,
            profile: Some(DeliveryProfile::Youtube1080p),
        };
        let report = qc(&document, &request);
        assert_eq!(
            report.target,
            Some(DeliveryProfile::Youtube1080p.loudness_target())
        );
        assert_eq!(report.master.integrated_lufs_hundredths, Some(-2_000));
        assert_eq!(
            exception_codes(&report),
            ["audio_loudness_out_of_tolerance"]
        );
        assert_eq!(report.exceptions[0].severity, QaSeverity::Warning);
        assert_eq!(report.exceptions[0].observed.as_deref(), Some("-2000"));
        assert_eq!(
            report.exceptions[0].allowed.as_deref(),
            Some("-1500..=-1300")
        );
        assert!(report.technical_pass);

        let hot = GeneratedMedia::from_bytes(
            "au3-qc-hot",
            "wav",
            &wav_f32(
                &stereo_tone(1_000.0, 10.0_f64.powf(-0.5 / 20.0), 48_000 * 2),
                48_000,
                2,
            ),
        );
        let document = one_clip_document(hot.path(), fps, 60);
        document.validate().unwrap();
        let report = qc(&document, &request);
        assert_eq!(report.master.true_peak_dbtp_hundredths, Some(-50));
        assert!(
            exception_codes(&report).contains(&"audio_true_peak_over_ceiling"),
            "{:?}",
            exception_codes(&report)
        );
        assert_eq!(report.exceptions[0].code, "audio_true_peak_over_ceiling");
        assert_eq!(report.exceptions[0].severity, QaSeverity::Error);
        assert!(!report.technical_pass);
        // No profile: the same programme raises nothing against a target.
        let plain = qc(&document, &AudioQcRequest::default());
        assert_eq!(plain.target, None);
        assert!(plain.exceptions.is_empty());
    }

    /// AU3 §7 item A13 / A14 (§3.10): digital silence raises `audio_silent`
    /// alone, as a Warning, with `technical_pass == true`; the provenance is
    /// the eight §2.4 strings; every leaf of the serialized report is an
    /// integer, bool, string, or null; `evidence_only` is always true.
    #[test]
    fn audio_qc_reports_silence_as_evidence_with_integer_leaves() {
        crate::initialize_ffmpeg().unwrap();
        let fps = Rational::new(30, 1).unwrap();
        let source = GeneratedMedia::from_bytes(
            "au3-qc-silent",
            "wav",
            &wav_f32(&vec![0.0_f32; 48_000 * 4], 48_000, 2),
        );
        let document = one_clip_document(source.path(), fps, 60);
        document.validate().unwrap();
        let report = qc(
            &document,
            &AudioQcRequest {
                range: None,
                profile: Some(DeliveryProfile::SourceMaster),
            },
        );
        assert_eq!(exception_codes(&report), ["audio_silent"]);
        assert_eq!(report.exceptions[0].severity, QaSeverity::Warning);
        assert!(report.technical_pass);
        assert!(report.evidence_only);
        assert_eq!(report.master.integrated_lufs_hundredths, None);
        assert_eq!(report.master.sample_peak_dbfs_hundredths, None);
        assert_eq!(report.master.true_peak_dbtp_hundredths, None);
        assert_eq!(report.channel_balance_lu_hundredths, None);
        assert_eq!(report.leading_silence_frames, TimeCode(60));
        assert_eq!(report.trailing_silence_frames, TimeCode(60));
        assert_eq!(
            report.provenance,
            AudioQcProvenance {
                engine: "kinewright_audio_qc_v1".to_owned(),
                measurement_rate: 48_000,
                k_weighting: "bs1770_4_bilinear_from_prototypes".to_owned(),
                true_peak: "8x_polyphase_256_tap_blackman_harris".to_owned(),
                true_peak_bias: "exact_on_grid_at_most_-0.042_db_between_grid_points".to_owned(),
                gate: "bs1770_4_two_stage_complete_blocks".to_owned(),
                loudness_range: "ebu_tech_3342_nearest_rank".to_owned(),
                silence: "10ms_rms_-70_dbfs_post_clamp".to_owned(),
                exception_order: "severity_desc_code_asc_field_asc".to_owned(),
            }
        );

        let serialized = serde_json::to_value(&report).unwrap();
        assert_integer_leaves(&serialized, "report");
        assert_eq!(serialized["evidence_only"], serde_json::Value::Bool(true));
    }

    /// AU3 §7 item A13 (§3.10 F8): the silence window accumulator persists
    /// across `master()` callbacks — `1 024 = 2 × 480 + 64`, so every chunk
    /// boundary splits a window — and a window straddling a boundary is
    /// counted once: chunked and whole feeds agree, at 150 windows = 45 frames.
    #[test]
    fn a_silence_window_straddling_a_chunk_boundary_is_counted_once() {
        let fps = Rational::new(30, 1).unwrap();
        let mut programme = vec![0.0_f32; 48_000 * 3];
        programme.extend(stereo_tone(1_000.0, 0.5, 48_000 * 3 / 2));
        let range = TimeCode::ZERO..TimeCode(90);
        let mut chunked = QcObserver::new().unwrap();
        for chunk in programme.chunks(1_024 * 2) {
            chunked.master(chunk).unwrap();
        }
        let chunked = chunked.finish(range.clone(), fps, None).unwrap();
        let mut whole = QcObserver::new().unwrap();
        whole.master(&programme).unwrap();
        let whole = whole.finish(range, fps, None).unwrap();
        assert_eq!(chunked.leading_silence_frames, TimeCode(45));
        assert_eq!(chunked.leading_silence_frames, whole.leading_silence_frames);
        assert_eq!(
            chunked.trailing_silence_frames,
            whole.trailing_silence_frames
        );
        assert_eq!(chunked.master, whole.master);
        assert_eq!(chunked.clipping, whole.clipping);
        assert_eq!(chunked.exceptions, whole.exceptions);
        // Clipping runs also carry across chunk boundaries: a full-scale run
        // of three samples split 2 + 1 across two callbacks is one run.
        let mut split = QcObserver::new().unwrap();
        let mut first = vec![0.0_f32; 48_000 * 2];
        first[48_000 * 2 - 4] = 1.0;
        first[48_000 * 2 - 2] = 1.0;
        split.master(&first).unwrap();
        let mut second = vec![0.0_f32; 48_000 * 2];
        second[0] = 1.0;
        split.master(&second).unwrap();
        let split = split
            .finish(TimeCode::ZERO..TimeCode(60), fps, None)
            .unwrap();
        assert_eq!(split.clipping.left.over_full_scale_samples, 3);
        assert_eq!(split.clipping.left.clipped_runs, 1);
        assert_eq!(split.clipping.right.clipped_runs, 0);
    }

    // ---------------------------------------------------------------------
    // AU4 §3 Part A media
    // ---------------------------------------------------------------------

    fn au4_curve(points: &[(i64, i64)], interpolation: KeyframeInterpolation) -> AutomationCurve {
        AutomationCurve {
            keyframes: points
                .iter()
                .map(|(at, value)| Keyframe {
                    at: TimeCode(*at),
                    value: *value,
                    interpolation,
                })
                .collect(),
        }
    }

    /// A `Hold` step: `{first: a}` held, then `{second: b}`.
    fn au4_hold_step_curve(first: i64, a: i64, second: i64, b: i64) -> AutomationCurve {
        AutomationCurve {
            keyframes: vec![
                Keyframe {
                    at: TimeCode(first),
                    value: a,
                    interpolation: KeyframeInterpolation::Hold,
                },
                Keyframe {
                    at: TimeCode(second),
                    value: b,
                    interpolation: KeyframeInterpolation::Linear,
                },
            ],
        }
    }

    /// AU4 §7 item A9 (rules 41, 43): the two anchors, the weight, the negative
    /// local frame, the degenerate frame span, and the static fallback.
    #[test]
    #[allow(clippy::float_cmp)]
    fn automation_step_brackets_the_frame_and_falls_back_to_the_scalar() {
        let rate = 48_000_u32;
        let fps = Rational::new(30, 1).unwrap();
        // Rule 44's arithmetic, asserted rather than assumed.
        assert_eq!(automation_hold_declick_frames(rate), 240);
        assert_eq!(
            frame_to_samples(TimeCode(1), rate, fps) - frame_to_samples(TimeCode::ZERO, rate, fps),
            1_600
        );

        let curve = au4_curve(&[(0, 0), (10, -600)], KeyframeInterpolation::Linear);
        // The two anchors of frame 5 are `value_at(5)` and `value_at(6)`.
        let step = automation_step(Some(&curve), 0, TimeCode::ZERO, 5 * 1_600, rate, fps);
        assert_eq!(step.v0, curve.value_at(TimeCode(5)).unwrap());
        assert_eq!(step.v1, curve.value_at(TimeCode(6)).unwrap());
        assert_eq!(step.t, 0.0, "the first sample of a frame weighs zero");
        assert_eq!(step.declick, None);

        // Rule 41.5 over the frame's first, middle and last sample.
        let middle = automation_step(Some(&curve), 0, TimeCode::ZERO, 5 * 1_600 + 800, rate, fps);
        assert_eq!(middle.t, 0.5);
        let last = automation_step(
            Some(&curve),
            0,
            TimeCode::ZERO,
            5 * 1_600 + 1_599,
            rate,
            fps,
        );
        assert_eq!(last.t, 1_599.0 / 1_600.0);
        assert!(last.t < 1.0);

        // Rule 41.1: a negative local frame is clamped **before**
        // `frame_to_samples`, which would return 0 for it.
        let local = automation_step(Some(&curve), 0, TimeCode(10), 0, rate, fps);
        assert_eq!(local.t, 0.0);
        assert_eq!(local.v0, curve.value_at(TimeCode::ZERO).unwrap());
        assert_eq!(local.v1, curve.value_at(TimeCode(1)).unwrap());

        // Rule 41.5's `s1 <= s0 + 1` arm: one project frame is one sample.
        let dense = Rational::new(48_000, 1).unwrap();
        let degenerate = automation_step(Some(&curve), 0, TimeCode::ZERO, 5, rate, dense);
        assert_eq!(degenerate.t, 0.0);

        // No curve: both anchors are the scalar and nothing declicks, so the
        // weight is computed but multiplies a zero difference.
        let none = automation_step(None, -123, TimeCode::ZERO, 5 * 1_600 + 400, rate, fps);
        assert_eq!(
            (none.v0, none.v1, none.t, none.declick),
            (-123, -123, 0.25, None)
        );
        let scalar = db_gain(-123);
        assert_eq!(
            automated_gain(&none, (scalar, scalar)).to_bits(),
            scalar.to_bits()
        );

        // A curve whose `value_at` cannot answer falls back to the scalar too.
        let empty = AutomationCurve { keyframes: vec![] };
        let fallback = automation_step(Some(&empty), 77, TimeCode::ZERO, 0, rate, fps);
        assert_eq!((fallback.v0, fallback.v1), (77, 77));
    }

    /// AU4 §7 item A9 (rules 3, 44, 45): a `Hold` segment is flat, the value
    /// changes at the next key's first sample, and the forward declick has no
    /// seam.
    #[test]
    #[allow(clippy::float_cmp)]
    fn a_hold_segment_is_flat_and_its_step_declicks_forward_without_a_seam() {
        let rate = 48_000_u32;
        let fps = Rational::new(30, 1).unwrap();
        // Rule 44's pinned example: `{10: 0, Hold}, {100: -600}`.
        let curve = au4_hold_step_curve(10, 0, 100, -600);
        let boundary = frame_to_samples(TimeCode(100), rate, fps);
        assert_eq!(boundary, 160_000);

        // Every sample of frames 10..=99 reads the held value exactly.
        for frame in 10..100 {
            for offset in [0_u64, 1, 799, 1_599] {
                let sample = frame_to_samples(TimeCode(frame), rate, fps) + offset;
                let step = automation_step(Some(&curve), 0, TimeCode::ZERO, sample, rate, fps);
                assert_eq!(step.v0, 0);
                assert_eq!(step.v1, 0, "a Hold segment is flat at frame {frame}");
                assert_eq!(step.declick, None);
                let gain = automated_gain(&step, (db_gain(step.v0), db_gain(step.v1)));
                assert_eq!(gain.to_bits(), db_gain(0).to_bits());
            }
        }

        let held = db_gain(0);
        let target = db_gain(-600);
        let value = |sample: u64| {
            let step = automation_step(Some(&curve), 0, TimeCode::ZERO, sample, rate, fps);
            automated_gain(&step, (db_gain(step.v0), db_gain(step.v1)))
        };

        // The declick window: 240 frames, weights `1/240 ..= 240/240`.
        let mut previous = held;
        for offset in 0..239_u64 {
            let step = automation_step(
                Some(&curve),
                0,
                TimeCode::ZERO,
                boundary + offset,
                rate,
                fps,
            );
            let (declick_held, weight) = step.declick.expect("inside the declick window");
            assert_eq!(declick_held, 0);
            #[allow(clippy::cast_precision_loss)]
            let expected = (offset + 1) as f32 / 240.0;
            assert_eq!(weight, expected);
            let current = value(boundary + offset);
            assert!(
                current < previous && current > target,
                "the declick interior is strictly between {held} and {target} and monotone; \
                 sample {} read {current} after {previous}",
                boundary + offset
            );
            previous = current;
        }

        // Rule 30/45: the `w >= 1.0` arm returns `target` **exactly**, so the
        // last declick sample is `to_bits()`-identical to the first undeclicked
        // one and there is no seam to bound.
        let reached = boundary + 239;
        let exact = boundary + 240;
        let last_declick = automation_step(Some(&curve), 0, TimeCode::ZERO, reached, rate, fps);
        assert_eq!(last_declick.declick, Some((0, 1.0)));
        assert_eq!(
            automation_step(Some(&curve), 0, TimeCode::ZERO, exact, rate, fps).declick,
            None
        );
        assert_eq!(value(reached).to_bits(), value(exact).to_bits());
        assert_eq!(value(reached).to_bits(), target.to_bits());
        // The naive `h + (target - h)` is the form the arm exists to avoid.
        assert_ne!((held + (target - held)).to_bits(), target.to_bits());

        println!("AU4_HOLD_STEP boundary={boundary} reached={reached} exact={exact}");
        println!(
            "AU4_HOLD_SEAM reached={reached} bits_equal={}",
            value(reached).to_bits() == value(exact).to_bits()
        );
    }

    /// AU4 §7 item A9 (rules 7, 42, 43, 49): both lerps are `a + (b - a) * t`,
    /// the automated multiply order is `gain * pan[ch]`, and the constant-power
    /// centre is not unity.
    #[test]
    #[allow(clippy::float_cmp)]
    fn the_lerp_form_makes_a_constant_curve_free_and_pins_the_multiply_order() {
        let rate = 48_000_u32;
        let fps = Rational::new(30, 1).unwrap();
        assert_eq!(
            pan_channel_ratios(PanLaw::ConstantPower, 0),
            [
                std::f32::consts::FRAC_1_SQRT_2,
                std::f32::consts::FRAC_1_SQRT_2
            ]
        );
        assert_eq!(pan_channel_ratios(PanLaw::Balance, 0), [1.0, 1.0]);

        // `a + (a - a) * t == a` bit-exactly for every finite `t`, under both
        // laws, which is the property that makes a constant curve cost nothing.
        for anchor in [-600_i64, -237, -1, 0, 37, 120] {
            let amplitude = db_gain(anchor);
            for step_index in 0..=1_000_u32 {
                #[allow(clippy::cast_precision_loss)]
                let t = f32::from(u16::try_from(step_index).unwrap()) / 1_000.0;
                let step = AutomationStep {
                    v0: anchor,
                    v1: anchor,
                    t,
                    declick: None,
                };
                assert_eq!(
                    automated_gain(&step, (amplitude, amplitude)).to_bits(),
                    amplitude.to_bits()
                );
                for law in [PanLaw::Balance, PanLaw::ConstantPower] {
                    let ratios = pan_channel_ratios(law, 25);
                    let panned = automated_pan(&step, (ratios, ratios), law);
                    assert_eq!(panned[0].to_bits(), ratios[0].to_bits());
                    assert_eq!(panned[1].to_bits(), ratios[1].to_bits());
                }
            }
        }

        // Rule 43: at every project frame's first sample `t == 0.0`, so the
        // automated arm equals `db_gain(value_at(f))` bit-exactly.
        let curve = au4_curve(&[(0, -300), (20, 120)], KeyframeInterpolation::EaseInOut);
        for frame in 0..25_i64 {
            let sample = frame_to_samples(TimeCode(frame), rate, fps);
            let step = automation_step(Some(&curve), 0, TimeCode::ZERO, sample, rate, fps);
            assert_eq!(step.t, 0.0);
            let anchors = (db_gain(step.v0), db_gain(step.v1));
            assert_eq!(
                automated_gain(&step, anchors).to_bits(),
                db_gain(curve.value_at(TimeCode(frame)).unwrap()).to_bits()
            );
        }

        // Rule 43: `track_stage_parameters` returns `[gain * pan[0], gain *
        // pan[1]]`, so the automated arm composes in that order.
        let document = {
            let mut document = processor_document(fps, 30, Vec::new());
            document.audio_mix.pan_law = PanLaw::ConstantPower;
            document.audio_mix.tracks = vec![track_mix(1, -37, 41, false, false)];
            document
        };
        let (_, _, parked) = track_stage_parameters(&document, TrackId(1), 2);
        let mut stage = TrackStageRuntime::new(&document, TrackId(1), 2, 240, rate);
        stage.gain_curve = Some(au4_curve(&[(0, -37)], KeyframeInterpolation::Linear));
        stage.pan_curve = Some(au4_curve(&[(0, 41)], KeyframeInterpolation::Linear));
        let (_, automated) = stage.automated_target(0);
        assert_eq!(automated[0].to_bits(), parked[0].to_bits());
        assert_eq!(automated[1].to_bits(), parked[1].to_bits());
        let gain = db_gain(-37);
        let pan = pan_channel_ratios(PanLaw::ConstantPower, 41);
        assert_eq!(automated[0].to_bits(), (gain * pan[0]).to_bits());
        assert_eq!(automated[1].to_bits(), (gain * pan[1]).to_bits());
        // Multiplication is commutative in IEEE 754 but **not associative**,
        // which is why rule 43 forbids a fused three-way product: the stage
        // multiplies the sample by the *folded* `gain * pan[ch]`, never by
        // `gain` and then `pan[ch]`.
        let sample = 0.1_f32;
        assert_ne!(
            (sample * (gain * pan[0])).to_bits(),
            ((sample * gain) * pan[0]).to_bits(),
            "the fold order is observable, which is what makes it pinnable"
        );
    }

    /// AU4 §7 item A9 (rule 48, R50): the constant-power chord bound, swept over
    /// every integer pan step and printed.
    ///
    /// The sweep measures the **geometry** — the chord across the arc that
    /// rule 48 bounds — in `f64`, because at `delta = 1` the sagitta is
    /// `7.7e-6` and one `f32` ulp near `0.7` is `6.0e-8`, so a measurement made
    /// in `f32` reads the anchors' own representation error and not the chord.
    /// The `f32` path that ships is then checked against the same chord, which
    /// is the only thing `f32` can be held to here.
    #[test]
    fn the_constant_power_chord_stays_inside_its_sagitta_at_every_pan_step() {
        let theta = |percent: f64| (percent / 100.0 + 1.0) * std::f64::consts::FRAC_PI_4;
        for delta in 1..=200_i32 {
            let angle = f64::from(delta) * std::f64::consts::PI / 400.0;
            let bound = 1.0 - angle.cos();
            let sagitta = 1.0 - (angle / 2.0).cos();
            let mut measured = 0.0_f64;
            let mut float_error = 0.0_f64;
            for start in TRACK_MIX_PAN_MIN..=(TRACK_MIX_PAN_MAX - delta) {
                let end = start + delta;
                let exact_anchors = [theta(f64::from(start)), theta(f64::from(end))]
                    .map(|angle| [angle.cos(), angle.sin()]);
                let anchors = (
                    pan_channel_ratios(PanLaw::ConstantPower, start),
                    pan_channel_ratios(PanLaw::ConstantPower, end),
                );
                for index in 0..=100_u32 {
                    let t = f64::from(index) / 100.0;
                    let arc = theta(f64::from(start) + t * f64::from(delta));
                    let exact = [arc.cos(), arc.sin()];
                    #[allow(clippy::cast_possible_truncation)]
                    let step = AutomationStep {
                        v0: i64::from(start),
                        v1: i64::from(end),
                        t: t as f32,
                        declick: None,
                    };
                    let shipped = automated_pan(&step, anchors, PanLaw::ConstantPower);
                    for channel in 0..2 {
                        // The chord rule 48 bounds: `a + (b - a) * t` over the
                        // two exact arc endpoints.
                        let chord = exact_anchors[0][channel]
                            + (exact_anchors[1][channel] - exact_anchors[0][channel]) * t;
                        measured = measured.max((chord - exact[channel]).abs());
                        float_error = float_error.max((f64::from(shipped[channel]) - chord).abs());
                    }
                }
            }
            assert!(
                measured <= sagitta && sagitta <= bound,
                "delta={delta}: measured={measured} sagitta={sagitta} bound={bound}"
            );
            assert!(
                float_error <= 8.0 * f64::from(f32::EPSILON),
                "delta={delta}: the shipped f32 chord is {float_error} from the exact chord"
            );
            println!("AU4_PAN_CHORD delta={delta} bound={bound:.6e} measured={measured:.6e}");
        }
        // Rule 48's two pinned figures.
        let ten = 10.0_f64 * std::f64::consts::PI / 400.0;
        assert!(((1.0 - ten.cos()) - 3.083e-3).abs() < 1.0e-6);
        assert!(((1.0 - (ten / 2.0).cos()) - 7.710e-4).abs() < 1.0e-7);
        let hundred = 100.0_f64 * std::f64::consts::PI / 400.0;
        assert!(((1.0 - hundred.cos()) - 0.293).abs() < 1.0e-3);
        assert!(((1.0 - (hundred / 2.0).cos()) - 0.0761).abs() < 1.0e-4);
    }

    /// AU4 §7 item A10 (rules 18, 47): per-project-frame evaluation against a
    /// per-sample reference — bit-identity at every frame's first sample and
    /// monotonicity across its interior, for gain and for pan under both laws.
    #[test]
    fn per_project_frame_automation_matches_a_per_sample_reference() {
        let rate = 48_000_u32;
        let fps = Rational::new(30, 1).unwrap();
        let gain_curve = au4_curve(&[(0, -600), (20, 120)], KeyframeInterpolation::Linear);
        let pan_curve = au4_curve(&[(0, -100), (20, 100)], KeyframeInterpolation::Linear);

        for frame in 0..24_i64 {
            let first = frame_to_samples(TimeCode(frame), rate, fps);
            let next = frame_to_samples(TimeCode(frame + 1), rate, fps);

            // (b) part one: bit-identity at the frame's first sample.
            let step = automation_step(Some(&gain_curve), 0, TimeCode::ZERO, first, rate, fps);
            let anchors = (db_gain(step.v0), db_gain(step.v1));
            assert_eq!(
                automated_gain(&step, anchors).to_bits(),
                db_gain(gain_curve.value_at(TimeCode(frame)).unwrap()).to_bits(),
                "gain at the first sample of frame {frame}"
            );

            // (b) part two: monotone between the two anchors on the interior.
            let mut previous = automated_gain(&step, anchors);
            for sample in (first + 1)..next {
                let step = automation_step(Some(&gain_curve), 0, TimeCode::ZERO, sample, rate, fps);
                let current = automated_gain(&step, (db_gain(step.v0), db_gain(step.v1)));
                assert!(
                    current >= previous,
                    "a rising gain curve must not fall inside frame {frame}"
                );
                previous = current;
            }
            let end = automation_step(Some(&gain_curve), 0, TimeCode::ZERO, next - 1, rate, fps);
            let a1 = db_gain(end.v1);
            assert!(previous <= a1, "the interior stays inside its two anchors");

            for law in [PanLaw::Balance, PanLaw::ConstantPower] {
                let step = automation_step(Some(&pan_curve), 0, TimeCode::ZERO, first, rate, fps);
                let anchors = (
                    pan_channel_ratios(law, pan_percent_of(step.v0)),
                    pan_channel_ratios(law, pan_percent_of(step.v1)),
                );
                let exact = pan_channel_ratios(
                    law,
                    i32::try_from(pan_curve.value_at(TimeCode(frame)).unwrap()).unwrap(),
                );
                let read = automated_pan(&step, anchors, law);
                assert_eq!(read[0].to_bits(), exact[0].to_bits());
                assert_eq!(read[1].to_bits(), exact[1].to_bits());

                let mut previous = read;
                for sample in (first + 1)..next {
                    let step =
                        automation_step(Some(&pan_curve), 0, TimeCode::ZERO, sample, rate, fps);
                    let anchors = (
                        pan_channel_ratios(law, pan_percent_of(step.v0)),
                        pan_channel_ratios(law, pan_percent_of(step.v1)),
                    );
                    let current = automated_pan(&step, anchors, law);
                    assert!(
                        current[0] <= previous[0] && current[1] >= previous[1],
                        "a left-to-right pan sweep is monotone per channel inside frame {frame}"
                    );
                    previous = current;
                }
            }
        }
    }

    /// AU4 §7 items A11/A12: `parity_document` with **all five** owners
    /// carrying a curve — a clip envelope, both track curves, a bus fader curve
    /// and a master fader curve.
    fn parity_document_with_automation(voice: &Path, bed: &Path, fps: Rational) -> Document {
        let mut document = parity_document_with_track_mix(voice, bed, fps);
        // The clip envelope is keyed **clip-local**; clip 4 runs 10 frames from
        // `timeline_start` 20 and already carries `audio_gain_tenth_db = -60`,
        // which rule 52 says the curve replaces.
        document.tracks[2].clips[0].audio_gain_curve = Some(au4_curve(
            &[(0, -240), (5, 0), (9, -180)],
            KeyframeInterpolation::Linear,
        ));
        document.audio_mix.tracks[0].gain_curve = Some(au4_curve(
            &[(0, -120), (12, 60), (29, -300)],
            KeyframeInterpolation::Linear,
        ));
        document.audio_mix.tracks[0].pan_curve = Some(au4_curve(
            &[(0, -80), (14, 0), (29, 90)],
            KeyframeInterpolation::EaseInOut,
        ));
        document.audio_mix.buses[0].gain_curve = Some(AutomationCurve {
            keyframes: vec![
                Keyframe {
                    at: TimeCode::ZERO,
                    value: 0,
                    interpolation: KeyframeInterpolation::Hold,
                },
                Keyframe {
                    at: TimeCode(8),
                    value: -90,
                    interpolation: KeyframeInterpolation::Linear,
                },
                Keyframe {
                    at: TimeCode(29),
                    value: 30,
                    interpolation: KeyframeInterpolation::Linear,
                },
            ],
        });
        document.audio_mix.master.gain_curve = Some(au4_curve(
            &[(0, -60), (20, 0)],
            KeyframeInterpolation::Linear,
        ));
        document
    }

    /// AU4 §7 item A11 (rules 70, 71): a document carrying all five owners'
    /// curves mixes deterministically, the stems path and the clamped observer
    /// feed agree with `mix_audio` bit for bit, and the SHA-256 of the mix is
    /// **printed** beside AU3's family rather than pinned across systems.
    #[test]
    fn mix_audio_with_every_automated_owner_is_bit_identical_through_both_paths() {
        crate::initialize_ffmpeg().unwrap();
        let fps = Rational::new(10, 1).unwrap();
        let voice = loud_sine("au4-automation-voice", 440);
        let bed = loud_sine("au4-automation-bed", 660);
        let settings = parity_settings(fps);
        let document = transitioned(parity_document_with_automation(
            voice.path(),
            bed.path(),
            fps,
        ));

        let exported = crate::export::mix_audio(&document, &settings).unwrap();
        assert_eq!(exported.len(), 30 * 4_800 * 2);
        let again = crate::export::mix_audio(&document, &settings).unwrap();
        assert!(
            exported
                .iter()
                .zip(&again)
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "automation evaluation must be deterministic"
        );
        let bytes = exported
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect::<Vec<_>>();
        println!(
            "AU4_AUTOMATION_SHA256 parity_all_five_owners {}",
            crate::sha256::sha256_bytes(&bytes)
        );

        let with_stems = mix_pass(
            &document,
            TimeCode::ZERO..document.duration,
            &settings,
            MixCollect {
                stems: true,
                master: true,
            },
            &mut NoObserver,
        )
        .unwrap();
        assert!(
            with_stems
                .master
                .iter()
                .zip(&exported)
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "the stems path must be the export"
        );

        let mut recorder = MasterRecorder::default();
        mix_pass(
            &document,
            TimeCode::ZERO..document.duration,
            &settings,
            MixCollect {
                stems: false,
                master: false,
            },
            &mut recorder,
        )
        .unwrap();
        limit_audio_mix(&mut recorder.master);
        assert_eq!(recorder.master.len(), exported.len());
        assert!(
            recorder
                .master
                .iter()
                .zip(&exported)
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "the observer's clamped master feed must be the export"
        );
    }

    /// AU4 §7 item A12 (rules 67, 71): playback matches export on an
    /// automation-bearing fixture through AU1's nine windows and its frame-5
    /// seek arm, and a **windowed** measurement reads the same samples the
    /// full-range pass does over that window — rule 67's regression.
    #[test]
    fn playback_matches_export_and_a_windowed_pass_keys_automation_from_project_zero() {
        crate::initialize_ffmpeg().unwrap();
        let fps = Rational::new(10, 1).unwrap();
        let voice = loud_sine("au4-parity-voice", 440);
        let bed = loud_sine("au4-parity-bed", 660);
        let settings = parity_settings(fps);
        let document = transitioned(parity_document_with_automation(
            voice.path(),
            bed.path(),
            fps,
        ));

        let exported = crate::export::mix_audio(&document, &settings).unwrap();
        assert_playback_matches_export(&document, &exported, fps);

        // Rule 67: `mix_pass` always starts its chunk loop at project sample 0
        // and trims the head afterwards, so a windowed pass keys every
        // automated owner at the true project frame. Compared bit for bit
        // against the same window of the full-range mix.
        let window = TimeCode(10)..TimeCode(20);
        let windowed = mix_pass(
            &document,
            window.clone(),
            &settings,
            MixCollect {
                stems: false,
                master: true,
            },
            &mut NoObserver,
        )
        .unwrap()
        .master;
        let samples = interleaved_sample_range(10..20, fps, 48_000, 2);
        assert_eq!(windowed.len(), samples.end - samples.start);
        assert!(
            windowed
                .iter()
                .zip(&exported[samples.clone()])
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "a windowed automated pass must read the full-range pass restricted to that window"
        );

        // And the same statement one level up, through `measure_mix_levels`.
        let report = crate::export::measure_mix_levels(
            &document,
            &MixLevelRequest {
                range: Some(window),
            },
        )
        .unwrap();
        assert_eq!(
            report.master,
            crate::loudness::measure_loudness(&exported[samples], 48_000, 2).unwrap()
        );
    }

    /// Every scalar owner of `document` gains a one-key **constant** curve of
    /// exactly the value it already carries.
    fn au4_constant_curves(document: &Document) -> Document {
        let mut document = document.clone();
        for track in &mut document.tracks {
            for clip in &mut track.clips {
                clip.audio_gain_curve = Some(au4_curve(
                    &[(0, i64::from(clip.audio_gain_tenth_db))],
                    KeyframeInterpolation::Linear,
                ));
            }
        }
        for entry in &mut document.audio_mix.tracks {
            entry.gain_curve = Some(au4_curve(
                &[(0, i64::from(entry.gain_tenth_db))],
                KeyframeInterpolation::Linear,
            ));
            entry.pan_curve = Some(au4_curve(
                &[(0, i64::from(entry.pan_percent))],
                KeyframeInterpolation::Linear,
            ));
        }
        for bus in &mut document.audio_mix.buses {
            bus.gain_curve = Some(au4_curve(
                &[(0, i64::from(bus.gain_tenth_db))],
                KeyframeInterpolation::Linear,
            ));
        }
        document.audio_mix.master.gain_curve = Some(au4_curve(
            &[(0, i64::from(document.audio_mix.master.gain_tenth_db))],
            KeyframeInterpolation::Linear,
        ));
        document
    }

    /// AU4 §7 item A13 (rules 42, 51, 52, 56, 70): neutrality is bit-exact.
    #[test]
    fn a_curve_free_document_and_a_constant_curve_document_export_identically() {
        crate::initialize_ffmpeg().unwrap();
        let fps = Rational::new(10, 1).unwrap();
        let voice = loud_sine("au4-neutral-voice", 440);
        let bed = loud_sine("au4-neutral-bed", 660);
        let settings = parity_settings(fps);
        let document = transitioned(parity_document_with_track_mix(
            voice.path(),
            bed.path(),
            fps,
        ));

        // The fast paths still fire: a curve-free stage and a curve-free fader
        // never take the automated arm.
        let processor = AudioMixProcessor::new(&document, 48_000, 2, None);
        assert!(processor.stages.iter().all(|stage| !stage.automated()));
        assert!(processor.buses.iter().all(|bus| bus.gain.curve.is_none()));
        assert!(processor.master.gain.curve.is_none());

        let exported = crate::export::mix_audio(&document, &settings).unwrap();
        let bytes = exported
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect::<Vec<_>>();
        println!(
            "AU4_AUTOMATION_SHA256 parity_track_mix_curve_free {}",
            crate::sha256::sha256_bytes(&bytes)
        );

        // Rule 42's lerp form is what makes this exact: `a + (a - a) * t == a`
        // bit-exactly for every finite `t`, so replacing every scalar with a
        // one-key constant curve changes no byte of the export — the automated
        // arm and the scalar arm agree everywhere.
        let constant = au4_constant_curves(&document);
        constant.validate().unwrap();
        let automated = crate::export::mix_audio(&constant, &settings).unwrap();
        assert_eq!(automated.len(), exported.len());
        let differing = exported
            .iter()
            .zip(&automated)
            .filter(|(a, b)| a.to_bits() != b.to_bits())
            .count();
        assert_eq!(
            differing, 0,
            "a constant curve must cost nothing: {differing} samples differ"
        );

        // Rule 51: with no curve the envelope factor is skipped entirely and
        // `gain_at` is the pre-AU4 product exactly.
        let clip = &document.tracks[2].clips[0];
        let shaping = ClipAudioShaping::new(clip, TimeCode(10), 48_000, fps);
        assert!(shaping.curve.is_none());
        for sample in (0..48_000_u64).step_by(97) {
            let reference = shaping.constant_gain
                * shaping.fade_in.map_or(1.0, |ramp| ramp.gain_at(sample))
                * shaping
                    .fade_out
                    .map_or(1.0, |ramp| 1.0 - ramp.gain_at(sample))
                * shaping.transition.map_or(1.0, |ramp| ramp.gain_at(sample));
            assert_eq!(shaping.gain_at(sample).to_bits(), reference.to_bits());
        }
    }

    /// AU4 §7 item A13 (rule 52): the envelope is **not gained twice**.
    #[test]
    fn a_clip_envelope_replaces_the_clip_scalar_rather_than_multiplying_it() {
        let fps = Rational::new(10, 1).unwrap();
        let curve = au4_curve(&[(0, -180), (9, 0)], KeyframeInterpolation::Linear);
        let mut quiet = audio_clip(1, 1, 0..10, 0);
        quiet.audio_gain_tenth_db = -600;
        quiet.audio_gain_curve = Some(curve.clone());
        let mut unity = audio_clip(1, 1, 0..10, 0);
        unity.audio_gain_tenth_db = 0;
        unity.audio_gain_curve = Some(curve);

        let quiet = ClipAudioShaping::new(&quiet, TimeCode(10), 48_000, fps);
        let unity = ClipAudioShaping::new(&unity, TimeCode(10), 48_000, fps);
        assert_eq!(quiet.constant_gain.to_bits(), 1.0_f32.to_bits());
        assert_eq!(unity.constant_gain.to_bits(), 1.0_f32.to_bits());
        for sample in 0..48_000_u64 {
            assert_eq!(
                quiet.gain_at(sample).to_bits(),
                unity.gain_at(sample).to_bits(),
                "the scalar must not survive beside its own curve at sample {sample}"
            );
        }
        // And the envelope really is the curve, evaluated by `automation_step`.
        assert_eq!(unity.gain_at(0).to_bits(), db_gain(-180).to_bits());
        assert_eq!(
            unity
                .gain_at(frame_to_samples(TimeCode(9), 48_000, fps))
                .to_bits(),
            db_gain(0).to_bits()
        );
    }

    /// AU4 §7 item A14: mix one track through a processor in export's own
    /// 1 024-frame chunks and drop the whole graph latency from the head,
    /// exactly as `mix_pass` does, so the result is indexed by final position.
    fn au4_master_at_final_position(
        document: &Document,
        rate: u32,
        input: &[f32],
        frames: usize,
    ) -> Vec<f32> {
        let channels = 2_usize;
        let mut processor = AudioMixProcessor::new(document, rate, channels, None);
        let mut mixed = Vec::with_capacity(frames * channels);
        let mut start = 0_usize;
        while start < frames {
            let count = MIX_CHUNK_SAMPLE_FRAMES.min(frames - start);
            let chunk = input[start * channels..(start + count) * channels].to_vec();
            let out = processor
                .mix_chunk(
                    &HashMap::from([(TrackId(1), chunk)]),
                    u64::try_from(start).unwrap(),
                    count,
                )
                .expect("the chunk should mix");
            mixed.extend_from_slice(&out);
            start += count;
        }
        let latency = graph_latency_frames(&document.audio_mix.lookahead_milliseconds(), rate)
            .saturating_mul(channels)
            .min(mixed.len());
        mixed.drain(..latency);
        mixed
    }

    /// The final-position sample frame at which two runs first differ.
    fn au4_first_difference(reference: &[f32], probe: &[f32], channels: usize) -> u64 {
        let index = reference
            .iter()
            .zip(probe)
            .position(|(a, b)| a.to_bits() != b.to_bits())
            .expect("an automated run must differ from its un-automated reference");
        u64::try_from(index / channels).unwrap()
    }

    /// AU4 §7 item A14 (rules 25, 64, 65): the automation key is the **input**
    /// frame, so the offset against the audible result is **per owner** — 0 for
    /// the clip envelope and the track stage, the bus chain's own node latency
    /// for the bus fader, and `stage_latency_frames(L_bus)` for the master
    /// fader — and 0 for all four without declared lookahead.
    #[test]
    fn the_automation_offset_against_the_audible_result_is_per_owner() {
        let rate = 48_000_u32;
        let channels = 2_usize;
        let fps = Rational::new(10, 1).unwrap();
        let frames = 30_000_usize;
        let key = TimeCode(5);
        let boundary = frame_to_samples(key, rate, fps);
        let step = || au4_hold_step_curve(0, 0, key.0, -600);

        for declared in [0_i64, kinewright_core::CHAIN_LOOKAHEAD_MILLISECONDS] {
            let lookahead_node = |id: u64| {
                audio_effect(
                    id,
                    "audio_compressor",
                    &[
                        // Ratio 1:1 with the threshold at full scale: a pure
                        // delay, so the probe measures the fader's placement
                        // and nothing else.
                        ("threshold_tenth_db", 0),
                        ("ratio_hundredths", 100),
                        ("lookahead_milliseconds", declared),
                    ],
                )
            };
            let base_document = || {
                let mut document = processor_document(fps, 20, vec![]);
                document.audio_mix.buses = vec![AudioBus {
                    id: AudioBusId(1),
                    name: "Chain".to_owned(),
                    tracks: vec![TrackId(1)],
                    gain_tenth_db: 0,
                    effects: vec![lookahead_node(1)],
                    ducking_sidechain_tracks: Vec::new(),
                    gain_curve: None,
                }];
                document.audio_mix.master.effects = vec![lookahead_node(2)];
                document
            };
            let document = base_document();
            let lookahead = document.audio_mix.lookahead_milliseconds();
            let bus_node_frames = stage_latency_frames(declared, rate);
            let master_offset = stage_latency_frames(lookahead.bus_stage, rate);
            assert_eq!(bus_node_frames, if declared == 0 { 0 } else { 960 });
            assert_eq!(master_offset, if declared == 0 { 0 } else { 960 });

            let flat = vec![0.25_f32; frames * channels];
            let reference = au4_master_at_final_position(&document, rate, &flat, frames);

            let mut cases: Vec<(&str, Vec<f32>, Document, u64)> = Vec::new();

            // The clip envelope is applied to the raw input chunk **before**
            // any chain, in both mix paths, so it is probed exactly there.
            let mut clip = audio_clip(1, 1, 0..20, 0);
            clip.audio_gain_curve = Some(step());
            let shaping = ClipAudioShaping::new(&clip, TimeCode(20), rate, fps);
            let mut enveloped = flat.clone();
            for (index, frame) in enveloped.chunks_mut(channels).enumerate() {
                let gain = shaping.gain_at(u64::try_from(index).unwrap());
                for sample in frame.iter_mut() {
                    *sample *= gain;
                }
            }
            cases.push(("clip_envelope", enveloped, base_document(), 0));

            let mut track_gain = base_document();
            track_gain.audio_mix.tracks = vec![TrackMix {
                gain_curve: Some(step()),
                ..TrackMix::neutral(TrackId(1))
            }];
            cases.push(("track_gain", flat.clone(), track_gain, 0));

            let mut track_pan = base_document();
            track_pan.audio_mix.tracks = vec![TrackMix {
                pan_curve: Some(au4_hold_step_curve(0, 0, key.0, -100)),
                ..TrackMix::neutral(TrackId(1))
            }];
            cases.push(("track_pan", flat.clone(), track_pan, 0));

            let mut bus_fader = base_document();
            bus_fader.audio_mix.buses[0].gain_curve = Some(step());
            cases.push((
                "bus_fader",
                flat.clone(),
                bus_fader,
                u64::try_from(bus_node_frames).unwrap(),
            ));

            let mut master_fader = base_document();
            master_fader.audio_mix.master.gain_curve = Some(step());
            cases.push((
                "master_fader",
                flat.clone(),
                master_fader,
                u64::try_from(master_offset).unwrap(),
            ));

            for (owner, input, document, expected) in cases {
                let probe = au4_master_at_final_position(&document, rate, &input, frames);
                let first = au4_first_difference(&reference, &probe, channels);
                // A curve that became audible *late* must fail loudly here, not
                // saturate to an offset of 0 that the three zero-offset owners
                // would accept (review-pass2-media-a F1).
                assert!(
                    first <= boundary,
                    "{owner} at {declared} ms: the step keyed at sample {boundary} \
                     became audible LATE, at final position {first}"
                );
                let offset = boundary - first;
                assert_eq!(
                    offset, expected,
                    "{owner} at {declared} ms: the step keyed at sample {boundary} \
                     became audible at final position {first}"
                );
                println!("AU4_LATENCY owner={owner} declared_ms={declared} offset_frames={offset}");
            }
        }
    }

    /// AU4 §7 item A15 (rules 46, 59): a live curve edit retargets every stage,
    /// bumps the per-curve epoch, and is audible on the **next sample frame**
    /// rather than at the next project-frame boundary.
    #[test]
    fn a_mid_frame_curve_edit_is_audible_on_the_next_sample_frame() {
        let rate = 48_000_u32;
        let fps = Rational::new(10, 1).unwrap();
        let mut document = processor_document(fps, 20, Vec::new());
        document.audio_mix.tracks = vec![TrackMix {
            gain_curve: Some(au4_curve(
                &[(0, 0), (10, -600)],
                KeyframeInterpolation::Linear,
            )),
            ..TrackMix::neutral(TrackId(1))
        }];
        let mut stage = TrackStageRuntime::new(&document, TrackId(1), 2, 240, rate);

        // Read the anchors mid-project-frame, so the memo is warm for frame 2.
        let frame = frame_to_samples(TimeCode(2), rate, fps);
        let mid = frame + 1_000;
        let before = stage.automated_target(mid).0;
        assert_eq!(stage.gain_memo.key.unwrap().0, 2);
        let epoch = stage.curve_epoch;

        let mut edited = document.clone();
        edited.audio_mix.tracks[0].gain_curve = Some(au4_curve(
            &[(0, -600), (10, 0)],
            KeyframeInterpolation::Linear,
        ));
        stage.retarget(&edited);
        assert_eq!(stage.curve_epoch, epoch.wrapping_add(1));

        // The very next sample of the same project frame reads the new curve.
        let after = stage.automated_target(mid + 1).0;
        assert_ne!(before.to_bits(), after.to_bits());
        let expected = automation_step(
            edited.audio_mix.tracks[0].gain_curve.as_ref(),
            0,
            TimeCode::ZERO,
            mid + 1,
            rate,
            fps,
        );
        assert_eq!(
            after.to_bits(),
            automated_gain(&expected, (db_gain(expected.v0), db_gain(expected.v1))).to_bits()
        );

        // Rule 59: an unchanged document tuple must not **restart** the ramp,
        // and (AU4 §0 E24) must not cut it short either.
        let running = stage.ramp_index;
        stage.retarget(&edited);
        assert_eq!(
            stage.ramp_index, running,
            "an unchanged tuple leaves the ramp exactly where it was"
        );
        let mut moved = edited.clone();
        moved.audio_mix.pan_law = PanLaw::ConstantPower;
        stage.retarget(&moved);
        assert_eq!(stage.ramp_index, 0, "a pan law change must retarget");
    }

    /// AU4 §7 item A15 (rule 60): an edit made while an automated track is
    /// silent leaves `current` at the **automated** value, so the first audible
    /// chunk of the next clip does not ramp from the parked scalar.
    #[test]
    #[allow(clippy::float_cmp)]
    fn skip_ramp_completes_to_the_automated_value_across_a_gap() {
        let rate = 48_000_u32;
        let fps = Rational::new(10, 1).unwrap();
        let mut document = processor_document(fps, 20, Vec::new());
        document.audio_mix.tracks = vec![TrackMix {
            gain_tenth_db: -600,
            gain_curve: Some(au4_curve(
                &[(0, 0), (10, 120)],
                KeyframeInterpolation::Linear,
            )),
            ..TrackMix::neutral(TrackId(1))
        }];
        let mut stage = TrackStageRuntime::new(&document, TrackId(1), 2, 240, rate);
        // The parked scalar the pre-AU4 completion would have used.
        let parked = stage.target;
        assert_eq!(parked[0].to_bits(), db_gain(-600).to_bits());

        let start = frame_to_samples(TimeCode(4), rate, fps);
        let frames = 1_024_usize;
        stage.skip_ramp(start, frames, 240);
        let last = start + u64::try_from(frames - 1).unwrap();
        let mut probe = TrackStageRuntime::new(&document, TrackId(1), 2, 240, rate);
        let expected = probe.automated_target(last).1;
        assert_eq!(stage.current[0].to_bits(), expected[0].to_bits());
        assert_ne!(stage.current[0].to_bits(), parked[0].to_bits());

        // A curve-free stage keeps AU1's behaviour exactly.
        let plain = processor_document(fps, 20, Vec::new());
        let mut plain_stage = TrackStageRuntime::new(&plain, TrackId(1), 2, 240, rate);
        let before = plain_stage.current;
        plain_stage.skip_ramp(start, frames, 240);
        assert_eq!(plain_stage.current, before);
    }

    /// AU4 §7 item A15 (rules 28, 59): a live `SetPanLaw` retargets **both** an
    /// automated and a non-automated stage, and is audible in the next chunk
    /// without a re-cue.
    #[test]
    fn a_live_pan_law_change_retargets_an_automated_and_a_plain_stage() {
        let rate = 48_000_u32;
        let channels = 2_usize;
        let fps = Rational::new(10, 1).unwrap();
        let mut document = processor_document(fps, 20, Vec::new());
        document.audio_mix.tracks = vec![
            TrackMix {
                pan_curve: Some(au4_curve(
                    &[(0, -50), (10, 50)],
                    KeyframeInterpolation::Linear,
                )),
                ..TrackMix::neutral(TrackId(1))
            },
            TrackMix {
                pan_percent: 40,
                ..TrackMix::neutral(TrackId(2))
            },
        ];
        let frames = 2_048_usize;
        let signal = vec![0.25_f32; frames * channels];
        let tracks = HashMap::from([(TrackId(1), signal.clone()), (TrackId(2), signal.clone())]);

        let mut processor = AudioMixProcessor::new(&document, rate, channels, None);
        let before = processor
            .mix_chunk_with_stems(&tracks, 0, frames)
            .unwrap()
            .tracks;

        let mut switched = document.clone();
        switched.audio_mix.pan_law = PanLaw::ConstantPower;
        let mut retargeted = AudioMixProcessor::new(&document, rate, channels, None);
        // Prime the chunk the edit lands in, then retarget and read the next.
        retargeted.mix_chunk_with_stems(&tracks, 0, frames).unwrap();
        retargeted.update_audio_mix(&switched);
        let after = retargeted
            .mix_chunk_with_stems(&tracks, u64::try_from(frames).unwrap(), frames)
            .unwrap()
            .tracks;

        // Both stages moved: the ramp is running on both, so the very first
        // frame of the next chunk already differs from the Balance reading.
        for (index, name) in [(0_usize, "automated"), (1, "plain")] {
            assert_ne!(
                before[index][0].to_bits(),
                after[index][0].to_bits(),
                "the {name} stage must hear the pan law change"
            );
        }
        // And the pan law reaches the automated stage's own anchors.
        assert_eq!(retargeted.stages[0].pan_law, PanLaw::ConstantPower);
        assert_eq!(retargeted.stages[1].pan_law, PanLaw::ConstantPower);
    }

    /// AU4 §7 item A15 (rules 73, 74, 77): a running mixer retargets its clip
    /// shaping in place for both `SetClipGainEnvelope` and the AU1 clip-gain
    /// drag, and refuses — falling back to stop-and-re-cue — when the segment
    /// layout changed.
    #[test]
    fn update_clip_shaping_retargets_in_place_or_refuses_on_a_layout_change() {
        crate::initialize_ffmpeg().unwrap();
        let fps = Rational::new(10, 1).unwrap();
        let voice = loud_sine("au4-live-voice", 440);
        let bed = loud_sine("au4-live-bed", 660);
        let document = transitioned(parity_document(voice.path(), bed.path(), fps));

        let mut mixer = AudioMixer::open(&document, TimeCode::ZERO, 48_000, 2, None).unwrap();
        let plain = mixer.next_chunk().unwrap().unwrap();

        // Rule 77's AU1 debt: a plain `SetClipAudio` gain drag retargets.
        let mut gained = document.clone();
        gained.tracks[0].clips[0].audio_gain_tenth_db = -120;
        gained.validate().unwrap();
        assert!(mixer.update_clip_shaping(&gained));
        let after_gain = mixer.next_chunk().unwrap().unwrap();

        // A `SetClipGainEnvelope` on the same clip retargets too, and is
        // audible: the second chunk differs from the un-edited mixer's.
        let mut enveloped = gained.clone();
        enveloped.tracks[0].clips[0].audio_gain_curve = Some(au4_curve(
            &[(0, -600), (5, 0)],
            KeyframeInterpolation::Linear,
        ));
        enveloped.validate().unwrap();
        assert!(mixer.update_clip_shaping(&enveloped));

        let mut reference = AudioMixer::open(&document, TimeCode::ZERO, 48_000, 2, None).unwrap();
        let reference_first = reference.next_chunk().unwrap().unwrap();
        assert_eq!(plain.len(), reference_first.len());
        let reference_second = reference.next_chunk().unwrap().unwrap();
        assert_ne!(
            after_gain
                .iter()
                .map(|sample| sample.to_bits())
                .collect::<Vec<_>>(),
            reference_second
                .iter()
                .map(|sample| sample.to_bits())
                .collect::<Vec<_>>(),
            "a retargeted clip gain must be audible without a re-cue"
        );

        // Rule 73: a layout change is refused, and the caller re-cues.
        let mut moved = enveloped.clone();
        moved.tracks[0].clips[1].timeline_start = TimeCode(17);
        moved.validate().unwrap();
        assert!(!mixer.update_clip_shaping(&moved));

        let mut trimmed = enveloped.clone();
        trimmed.tracks[1].clips[0].source_range = TimeCode(4)..TimeCode(13);
        trimmed.validate().unwrap();
        assert!(!mixer.update_clip_shaping(&trimmed));
    }

    /// AU4 §0 E24 (rule 59): an owner whose document inputs did **not** change
    /// keeps its in-flight ramp instead of freezing at the interpolated value.
    ///
    /// `update_audio_mix` retargets every stage and every fader on every mix
    /// edit, so this is the ordinary case: one fader moves while another is
    /// still 3 ms into its own 5 ms ramp. AU1's arm could force `ramp_index =
    /// ramp_frames` because its condition was `target == self.current`; under
    /// rule 59's document comparison that would settle the ramp at a value
    /// `advance` never assigns `target` to, and the fader would stay there.
    #[test]
    #[allow(clippy::float_cmp)]
    fn an_unchanged_document_tuple_lets_an_in_flight_ramp_finish() {
        let rate = 48_000_u32;
        let fps = Rational::new(10, 1).unwrap();
        let ramp_frames = 240_usize;

        let mut fader = GainRamp::settled(0, None, ramp_frames, rate, fps);
        fader.retarget(-120, None);
        assert_eq!(fader.ramp_index, 0);
        let mut chunk = vec![1.0_f32; 100 * 2];
        fader.apply(&mut chunk, 2, ramp_frames, 0);
        assert!(!fader.is_settled(ramp_frames));
        let mid = fader.current;
        assert_ne!(mid.to_bits(), fader.target.to_bits());

        // A second edit elsewhere retargets this fader with the same inputs.
        fader.retarget(-120, None);
        assert_eq!(fader.current.to_bits(), mid.to_bits(), "no discontinuity");
        assert!(
            !fader.is_settled(ramp_frames),
            "the ramp must not be cut short"
        );
        let mut rest = vec![1.0_f32; 200 * 2];
        fader.apply(&mut rest, 2, ramp_frames, 200);
        assert!(fader.is_settled(ramp_frames));
        assert_eq!(
            fader.current.to_bits(),
            db_gain(-120).to_bits(),
            "the ramp must land on the document's own target"
        );

        // The same for a track stage.
        let mut document = processor_document(fps, 20, Vec::new());
        document.audio_mix.tracks = vec![track_mix(1, 0, 0, false, false)];
        let mut stage = TrackStageRuntime::new(&document, TrackId(1), 2, ramp_frames, rate);
        let mut moved = document.clone();
        moved.audio_mix.tracks = vec![track_mix(1, -120, 0, false, false)];
        stage.retarget(&moved);
        let mut source = vec![1.0_f32; 100 * 2];
        let mut destination = vec![0.0_f32; 100 * 2];
        stage.apply(&source, &mut destination, ramp_frames, 0);
        let mid = stage.current;
        assert_ne!(mid[0].to_bits(), stage.target[0].to_bits());
        stage.retarget(&moved);
        assert_eq!(stage.current[0].to_bits(), mid[0].to_bits());
        source.resize(200 * 2, 1.0);
        destination.resize(200 * 2, 0.0);
        stage.apply(&source, &mut destination, ramp_frames, 200);
        assert_eq!(stage.current[0].to_bits(), stage.target[0].to_bits());
    }
}
