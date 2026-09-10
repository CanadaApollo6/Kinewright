//! AU3 §3: the streaming ITU-R BS.1770-4 / EBU R128 loudness meter.
//!
//! One type, [`LoudnessMeter`], measures everything the programme reports —
//! integrated loudness (§3.3), momentary and short-term maxima (§3.4), the
//! EBU Tech 3342 loudness range (§3.5), and the sample and true peaks (§3.6)
//! — from interleaved PCM pushed in chunks of any size. Every accumulation is
//! sequential per sample in `f64` with filter and FIR state carried across
//! `push`, so the result is independent of how the programme was chunked
//! (§3.1, A5). Outputs are integer hundredths through [`hundredths`] and
//! `Option` for "unmeasured" (§3.7).

use std::{collections::VecDeque, f64::consts::PI, sync::OnceLock};

use kinewright_core::{AudioLoudness, LoudnessSnapshot, MediaError};

/// BS.1770-4 absolute gate.
const ABSOLUTE_GATE_LUFS: f64 = -70.0;
/// BS.1770-4 relative gate for the integrated value.
const RELATIVE_GATE_LU: f64 = 10.0;
/// EBU Tech 3342 relative gate for the loudness range.
const LOUDNESS_RANGE_GATE_LU: f64 = 20.0;
const LOUDNESS_OFFSET: f64 = -0.691;

/// AU3 §2.5 / §3.3: one 400 ms gating block at the 48 kHz measurement rate.
/// A `mix_levels` or `audio_qc` range shorter than this is refused typed
/// before anything is decoded.
pub const LOUDNESS_GATING_BLOCK_FRAMES: u64 = 19_200;

/// BS.1770-4 Table 3 channel weights (L, R, C, Ls, Rs), applied by
/// [`LoudnessMeter`] to every block and short-term energy sum. Unity for the
/// first two channels — the only ones this meter measures, because export is
/// stereo (§1.3) — so the constant records the rule without changing a number.
pub(crate) const BS1770_CHANNEL_WEIGHTS: [f64; 5] = [1.0, 1.0, 1.0, 1.41, 1.41];

/// §3.3's `G_i`: `BS1770_CHANNEL_WEIGHTS[i]`, unity past the table.
fn channel_weight(channel: usize) -> f64 {
    BS1770_CHANNEL_WEIGHTS.get(channel).copied().unwrap_or(1.0)
}

/// Sub-blocks per 400 ms gating block (75 % overlap at a 100 ms hop).
const SUB_BLOCKS_PER_BLOCK: usize = 4;
/// Sub-blocks per 3 s short-term window.
const SUB_BLOCKS_PER_SHORT_TERM: usize = 30;
/// The smallest rate a meter accepts; below it a 100 ms sub-block is under
/// 100 frames and the K-weighting prototypes fold over Nyquist.
const MINIMUM_SAMPLE_RATE: u32 = 1_000;

/// AU3 §3.6: the true-peak meter's oversampling factor.
pub(crate) const TRUE_PEAK_METER_OVERSAMPLE: usize = 8;
/// AU3 §3.6: taps per polyphase branch.
pub(crate) const TRUE_PEAK_METER_PHASE_TAPS: usize = 32;
/// AU3 §3.6: the prototype length, `OVERSAMPLE × PHASE_TAPS`.
pub(crate) const TRUE_PEAK_METER_TAPS: usize =
    TRUE_PEAK_METER_OVERSAMPLE * TRUE_PEAK_METER_PHASE_TAPS;
/// AU3 §3.6: the group delay in input frames — exact on this grid, because
/// `x_n = (n − 128) / 8` puts the prototype's centre tap on a sample.
pub(crate) const TRUE_PEAK_METER_GROUP_DELAY_FRAMES: usize = 16;
const _: () = assert!(TRUE_PEAK_METER_GROUP_DELAY_FRAMES == TRUE_PEAK_METER_PHASE_TAPS / 2);

/// AU3 §3.9: how many `(block end, snapshot)` pairs the live ring holds. The
/// fill leads the loudspeaker by at most 1 s = 10 sub-blocks; 16 leaves
/// headroom for a slow tick.
pub(crate) const LIVE_LOUDNESS_RING_ENTRIES: usize = 16;

#[derive(Clone, Copy, Debug)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 3],
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Biquad {
    const fn new(b: [f64; 3], a: [f64; 3]) -> Self {
        Self {
            b,
            a,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn process(&mut self, input: f64) -> f64 {
        let output = self.b[0] * input + self.b[1] * self.x1 + self.b[2] * self.x2
            - self.a[1] * self.y1
            - self.a[2] * self.y2;
        self.x2 = self.x1;
        self.x1 = input;
        self.y2 = self.y1;
        self.y1 = output;
        output
    }

    fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }
}

/// AU3 §3.2: the two K-weighting stages for one sample rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct KWeighting {
    pub(crate) shelf_b: [f64; 3],
    pub(crate) shelf_a: [f64; 3],
    pub(crate) high_pass_b: [f64; 3],
    pub(crate) high_pass_a: [f64; 3],
}

/// AU3 §3.2 (N1): the bilinear transform of the BS.1770-4 analogue prototypes
/// in the De Man parametrisation — not the RBJ cookbook shelf, which misses
/// Table 1 by up to 0.056, and not the cookbook's normalised high-pass
/// numerator: the RLB stage keeps `[1, −2, 1]` as the ITU table does.
#[must_use]
pub(crate) fn k_weighting(sample_rate: u32) -> KWeighting {
    let fs = f64::from(sample_rate);
    // Stage 1 — the pre-filter (high shelf).
    let f0 = 1_681.974_450_955_533;
    let q = 0.707_175_236_955_419_6;
    let gain_db = 3.999_843_853_973_347;
    let k = (PI * f0 / fs).tan();
    let vh = 10.0_f64.powf(gain_db / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let a0 = 1.0 + k / q + k * k;
    let shelf_b = [
        (vh + vb * k / q + k * k) / a0,
        2.0 * (k * k - vh) / a0,
        (vh - vb * k / q + k * k) / a0,
    ];
    let shelf_a = [1.0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0];
    // Stage 2 — the RLB weighting (high-pass).
    let f0 = 38.135_470_876_024_44;
    let q = 0.500_327_037_323_877_3;
    let k = (PI * f0 / fs).tan();
    let a0 = 1.0 + k / q + k * k;
    let high_pass_b = [1.0, -2.0, 1.0];
    let high_pass_a = [1.0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0];
    KWeighting {
        shelf_b,
        shelf_a,
        high_pass_b,
        high_pass_a,
    }
}

/// One channel's K-weighting chain.
#[derive(Clone, Copy, Debug)]
struct KWeightingChain {
    shelf: Biquad,
    high_pass: Biquad,
}

impl KWeightingChain {
    fn new(coefficients: &KWeighting) -> Self {
        Self {
            shelf: Biquad::new(coefficients.shelf_b, coefficients.shelf_a),
            high_pass: Biquad::new(coefficients.high_pass_b, coefficients.high_pass_a),
        }
    }

    fn process(&mut self, input: f64) -> f64 {
        self.high_pass.process(self.shelf.process(input))
    }

    fn reset(&mut self) {
        self.shelf.reset();
        self.high_pass.reset();
    }
}

fn energy_loudness(energy: f64) -> f64 {
    LOUDNESS_OFFSET + 10.0 * energy.log10()
}

/// Round half away from zero to `i32` hundredths, range-checked.
///
/// The lower bound is **exclusive**: `i32::MIN` is `LIVE_LOUDNESS_NONE`, the
/// live path's "unmeasured" sentinel (§3.9), so no measurement may ever
/// produce it. (No real energy comes close — f64's denormal floor bounds a
/// loudness at about −323 300 hundredths — but the sentinel's disjointness is
/// a range check here rather than an argument about denormals.)
fn hundredths(value: f64) -> Result<i32, MediaError> {
    let scaled = (value * 100.0).round();
    if !scaled.is_finite() || scaled <= f64::from(i32::MIN) || scaled > f64::from(i32::MAX) {
        return Err(MediaError::Backend(
            "audio loudness result is outside the supported range".to_owned(),
        ));
    }
    #[allow(clippy::cast_possible_truncation)]
    Ok(scaled as i32)
}

/// `hundredths` of a loudness, or `None` for a zero (or otherwise
/// unrepresentable) energy — the live sentinel's meaning (§3.9).
fn loudness_hundredths(energy: f64) -> Option<i32> {
    if energy > 0.0 {
        hundredths(energy_loudness(energy)).ok()
    } else {
        None
    }
}

// Block counts are memory-bounded and far below 2^53.
#[allow(clippy::cast_precision_loss)]
fn mean(values: impl Iterator<Item = f64>) -> Option<f64> {
    let (sum, count) = values.fold((0.0_f64, 0_usize), |(sum, count), value| {
        (sum + value, count + 1)
    });
    (count > 0).then(|| sum / count as f64)
}

/// CC6 §10.4's nearest rank in integer arithmetic: `min(n − 1, ceil(p·n) − 1)`
/// with `p` in percent, so `0.10 × 200` is rank 20 and not `20.000…04`.
///
/// `pub(crate)` per AU5 §0 R21: AU5 §3.7 rule 61(ii)'s profile percentile and
/// §3.9's 10th/90th window percentiles are the same ranking rule, read from
/// `spectrum.rs` and `export.rs`.
pub(crate) fn nearest_rank_index(count: usize, percent: usize) -> usize {
    let rank = (count * percent).div_ceil(100);
    rank.max(1).min(count) - 1
}

// ---- True peak -------------------------------------------------------------

/// The prototype and its per-phase normalisation (§3.6): a 4-term
/// Blackman-Harris windowed sinc on `x_n = (n − 128) / 8`, each phase scaled
/// so its 32 taps sum to one. Returned phase-major and time-reversed so a
/// phase's output is one straight dot product with the history window.
fn true_peak_meter_phases()
-> &'static [[f64; TRUE_PEAK_METER_PHASE_TAPS]; TRUE_PEAK_METER_OVERSAMPLE] {
    static PHASES: OnceLock<[[f64; TRUE_PEAK_METER_PHASE_TAPS]; TRUE_PEAK_METER_OVERSAMPLE]> =
        OnceLock::new();
    PHASES.get_or_init(|| {
        let taps = true_peak_meter_taps();
        let mut phases = [[0.0_f64; TRUE_PEAK_METER_PHASE_TAPS]; TRUE_PEAK_METER_OVERSAMPLE];
        for (phase, reversed) in phases.iter_mut().enumerate() {
            for k in 0..TRUE_PEAK_METER_PHASE_TAPS {
                reversed[TRUE_PEAK_METER_PHASE_TAPS - 1 - k] =
                    taps[TRUE_PEAK_METER_OVERSAMPLE * k + phase];
            }
        }
        phases
    })
}

/// The 256 normalised taps in prototype order, `h[8k + p]` being phase `p`'s
/// `k`-th tap. `h[128] == 1.0` and every other `h[8k]` is under 1e-15, so
/// phase 0 is the input delayed by [`TRUE_PEAK_METER_GROUP_DELAY_FRAMES`]
/// and the sample peak is structurally part of the estimate (R5).
pub(crate) fn true_peak_meter_taps() -> &'static [f64; TRUE_PEAK_METER_TAPS] {
    static TAPS: OnceLock<[f64; TRUE_PEAK_METER_TAPS]> = OnceLock::new();
    TAPS.get_or_init(|| {
        let mut taps = raw_true_peak_meter_taps();
        let sums = raw_true_peak_meter_phase_sums();
        for (index, tap) in taps.iter_mut().enumerate() {
            *tap /= sums[index % TRUE_PEAK_METER_OVERSAMPLE];
        }
        taps
    })
}

/// The unnormalised prototype.
#[allow(clippy::cast_precision_loss)]
fn raw_true_peak_meter_taps() -> [f64; TRUE_PEAK_METER_TAPS] {
    let mut taps = [0.0_f64; TRUE_PEAK_METER_TAPS];
    let length = TRUE_PEAK_METER_TAPS as f64;
    let centre = (TRUE_PEAK_METER_TAPS / 2) as f64;
    for (index, tap) in taps.iter_mut().enumerate() {
        let n = index as f64;
        let x = (n - centre) / TRUE_PEAK_METER_OVERSAMPLE as f64;
        let window = 0.358_75 - 0.488_29 * (2.0 * PI * n / length).cos()
            + 0.141_28 * (4.0 * PI * n / length).cos()
            - 0.011_68 * (6.0 * PI * n / length).cos();
        let sinc = if x == 0.0 {
            1.0
        } else {
            (PI * x).sin() / (PI * x)
        };
        *tap = sinc * window;
    }
    taps
}

/// The eight raw phase sums, each within 1.2e-6 of one before normalisation.
pub(crate) fn raw_true_peak_meter_phase_sums() -> [f64; TRUE_PEAK_METER_OVERSAMPLE] {
    let taps = raw_true_peak_meter_taps();
    let mut sums = [0.0_f64; TRUE_PEAK_METER_OVERSAMPLE];
    for (index, tap) in taps.iter().enumerate() {
        sums[index % TRUE_PEAK_METER_OVERSAMPLE] += tap;
    }
    sums
}

/// AU3 §3.6: the 8× polyphase true-peak meter, per channel, `f64`.
///
/// The history is a doubled ring so the 32 most recent frames are always one
/// contiguous window; each input frame costs 256 multiply-adds per channel.
#[derive(Clone, Debug)]
struct TruePeakMeter {
    history: Vec<[f64; 2 * TRUE_PEAK_METER_PHASE_TAPS]>,
    write: usize,
    peak: f64,
}

impl TruePeakMeter {
    fn new(channels: usize) -> Self {
        Self {
            history: vec![[0.0; 2 * TRUE_PEAK_METER_PHASE_TAPS]; channels.max(1)],
            write: 0,
            peak: 0.0,
        }
    }

    /// Record one frame and fold its eight interpolated values into the peak.
    fn push(&mut self, frame: &[f64]) {
        let phases = true_peak_meter_phases();
        let write = self.write;
        let mut peak = self.peak;
        for (history, sample) in self.history.iter_mut().zip(frame) {
            history[write] = *sample;
            history[write + TRUE_PEAK_METER_PHASE_TAPS] = *sample;
            let window: &[f64; TRUE_PEAK_METER_PHASE_TAPS] = history
                [write + 1..write + 1 + TRUE_PEAK_METER_PHASE_TAPS]
                .try_into()
                .expect("the doubled history always holds a whole window");
            for phase in phases {
                let mut accumulator = 0.0_f64;
                for m in 0..TRUE_PEAK_METER_PHASE_TAPS {
                    accumulator += phase[m] * window[m];
                }
                let magnitude = accumulator.abs();
                if magnitude > peak {
                    peak = magnitude;
                }
            }
        }
        self.peak = peak;
        self.write = (write + 1) % TRUE_PEAK_METER_PHASE_TAPS;
    }

    /// §3.6 edges: feed 32 zero frames so every real frame has been seen by
    /// all eight phases.
    fn flush(&mut self) {
        let zeros = [0.0_f64; 2];
        let channels = self.history.len().min(zeros.len());
        for _ in 0..TRUE_PEAK_METER_PHASE_TAPS {
            self.push(&zeros[..channels]);
        }
    }

    /// Forget the signal history; the peak is kept (§3.1 `truncate_to`).
    fn restart(&mut self) {
        self.history.fill([0.0; 2 * TRUE_PEAK_METER_PHASE_TAPS]);
        self.write = 0;
    }

    fn reset(&mut self) {
        self.restart();
        self.peak = 0.0;
    }
}

// ---- The meter -------------------------------------------------------------

/// AU3 §3.1: the streaming loudness meter.
///
/// State: two DF-I `f64` biquads per channel, one running square sum per
/// channel for the open 100 ms sub-block, one `f64` per completed sub-block
/// per channel (96 KB at ten minutes of stereo), the two derived series
/// (`blocks`, `short_term`) appended as each sub-block closes, the true-peak
/// history, and the running sample and true peaks. Only complete sub-blocks
/// exist; a programme shorter than 400 ms has no gating block and reads
/// `integrated == None` with both peaks `Some` (§3.3, F5).
///
/// The two series are maintained **incrementally**: closing a sub-block costs
/// four plus thirty adds, so `snapshot` (§3.9, ten times a second while
/// playing) is the gate scan plus the Tech 3342 sort and nothing else. The
/// direct forms are kept under `cfg(test)` and pinned against these.
#[derive(Clone, Debug)]
pub struct LoudnessMeter {
    sample_rate: u32,
    channels: usize,
    sub_block_frames: usize,
    filters: Vec<KWeightingChain>,
    open_sums: Vec<f64>,
    open_frames: usize,
    /// Per channel: the mean square of each completed sub-block.
    energies: Vec<Vec<f64>>,
    /// End sample (frames since reset) of each completed sub-block.
    ends: Vec<u64>,
    /// §3.3: `m_j` for every complete 400 ms gating block, indexed `j − 3`.
    blocks: Vec<f64>,
    /// §3.4: `s_j` for every complete 3 s window, indexed `j − 29`.
    short_term: Vec<f64>,
    total_frames: u64,
    sample_peak: f64,
    true_peak: TruePeakMeter,
}

impl LoudnessMeter {
    /// A meter for interleaved PCM at `sample_rate` with `channels` channels.
    ///
    /// # Errors
    ///
    /// `sample_rate < 1_000` or a channel count other than 1 or 2 is refused
    /// with `MediaError::Backend`.
    pub fn new(sample_rate: u32, channels: u16) -> Result<Self, MediaError> {
        if sample_rate < MINIMUM_SAMPLE_RATE {
            return Err(MediaError::Backend(format!(
                "loudness measurement needs at least {MINIMUM_SAMPLE_RATE} Hz PCM, got {sample_rate}"
            )));
        }
        let channels = usize::from(channels);
        if channels == 0 || channels > 2 {
            return Err(MediaError::Backend(
                "loudness measurement currently supports mono or stereo PCM".to_owned(),
            ));
        }
        let coefficients = k_weighting(sample_rate);
        Ok(Self {
            sample_rate,
            channels,
            sub_block_frames: usize::try_from(sample_rate / 10).unwrap_or(usize::MAX),
            filters: vec![KWeightingChain::new(&coefficients); channels],
            open_sums: vec![0.0; channels],
            open_frames: 0,
            energies: vec![Vec::new(); channels],
            ends: Vec::new(),
            blocks: Vec::new(),
            short_term: Vec::new(),
            total_frames: 0,
            sample_peak: 0.0,
            true_peak: TruePeakMeter::new(channels),
        })
    }

    /// Measure a whole buffer in one push and finish.
    ///
    /// # Errors
    ///
    /// As [`LoudnessMeter::new`], [`LoudnessMeter::push`], and
    /// [`LoudnessMeter::finish`].
    pub fn measure(
        samples: &[f32],
        sample_rate: u32,
        channels: u16,
    ) -> Result<AudioLoudness, MediaError> {
        let mut meter = Self::new(sample_rate, channels)?;
        meter.push(samples)?;
        meter.finish()
    }

    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    #[must_use]
    pub fn channels(&self) -> u16 {
        u16::try_from(self.channels).unwrap_or(0)
    }

    /// Frames pushed since the last reset (or truncation point).
    #[must_use]
    pub const fn sample_frames(&self) -> u64 {
        self.total_frames
    }

    /// End sample (frames since reset) of the last completed sub-block;
    /// `None` before the first.
    #[must_use]
    pub fn last_block_end(&self) -> Option<u64> {
        self.ends.last().copied()
    }

    /// Frames still needed to complete the open sub-block.
    fn frames_to_sub_block_end(&self) -> usize {
        self.sub_block_frames - self.open_frames
    }

    /// Push whole interleaved frames. Returns the number of 100 ms
    /// sub-blocks this push completed.
    ///
    /// # Errors
    ///
    /// A slice not aligned to the channel count is refused with
    /// `MediaError::Backend("interleaved PCM is not aligned to its channel
    /// count")` and nothing is consumed.
    pub fn push(&mut self, interleaved: &[f32]) -> Result<usize, MediaError> {
        if !interleaved.len().is_multiple_of(self.channels) {
            return Err(MediaError::Backend(
                "interleaved PCM is not aligned to its channel count".to_owned(),
            ));
        }
        let mut completed = 0_usize;
        let mut frame_f64 = [0.0_f64; 2];
        for frame in interleaved.chunks_exact(self.channels) {
            for (channel, sample) in frame.iter().enumerate() {
                let x = f64::from(*sample);
                let magnitude = x.abs();
                if magnitude > self.sample_peak {
                    self.sample_peak = magnitude;
                }
                frame_f64[channel] = x;
                let y = self.filters[channel].process(x);
                self.open_sums[channel] += y * y;
            }
            self.true_peak.push(&frame_f64[..self.channels]);
            self.open_frames += 1;
            self.total_frames += 1;
            if self.open_frames == self.sub_block_frames {
                self.close_sub_block();
                completed += 1;
            }
        }
        Ok(completed)
    }

    #[allow(clippy::cast_precision_loss)]
    fn close_sub_block(&mut self) {
        let frames = self.sub_block_frames as f64;
        for (energies, sum) in self.energies.iter_mut().zip(&mut self.open_sums) {
            energies.push(*sum / frames);
            *sum = 0.0;
        }
        self.ends.push(self.total_frames);
        self.open_frames = 0;
        // §3.3 / §3.4: the two windows that just became complete, if any.
        if let Some(energy) = self.newest_window_energy(SUB_BLOCKS_PER_BLOCK) {
            self.blocks.push(energy);
        }
        if let Some(energy) = self.newest_window_energy(SUB_BLOCKS_PER_SHORT_TERM) {
            self.short_term.push(energy);
        }
    }

    /// The overlapping-window energy ending at the newest completed sub-block:
    /// `Σ_i G_i · (1/span) Σ_s z_{i,s}` (§3.3 with `span = 4`, §3.4 with
    /// `span = 30`). `None` until `span` sub-blocks have completed.
    #[allow(clippy::cast_precision_loss)]
    fn newest_window_energy(&self, span: usize) -> Option<f64> {
        let count = self.ends.len();
        if count < span {
            return None;
        }
        Some(
            self.energies
                .iter()
                .enumerate()
                .map(|(channel, energies)| {
                    channel_weight(channel)
                        * (energies[count - span..count].iter().sum::<f64>() / span as f64)
                })
                .sum::<f64>(),
        )
    }

    /// Restart the filters and the true-peak history (not its peak).
    fn restart_signal_state(&mut self) {
        for filter in &mut self.filters {
            filter.reset();
        }
        self.true_peak.restart();
    }

    /// AU3 §3.9 pause: drop completed sub-blocks whose end sample is past
    /// `frames`, discard the open sub-block, and restart the filter and
    /// oversampler state. Peaks are kept. The frame count becomes `frames`
    /// (never more than what was pushed), so subsequent sub-blocks are keyed
    /// from the truncation point.
    pub fn truncate_to(&mut self, frames: u64) {
        while self.ends.last().is_some_and(|end| *end > frames) {
            self.ends.pop();
            for energies in &mut self.energies {
                energies.pop();
            }
        }
        self.blocks
            .truncate(self.ends.len().saturating_sub(SUB_BLOCKS_PER_BLOCK - 1));
        self.short_term.truncate(
            self.ends
                .len()
                .saturating_sub(SUB_BLOCKS_PER_SHORT_TERM - 1),
        );
        self.open_sums.fill(0.0);
        self.open_frames = 0;
        self.total_frames = frames.min(self.total_frames);
        self.restart_signal_state();
    }

    /// Forget everything, peaks included.
    pub fn reset(&mut self) {
        self.ends.clear();
        for energies in &mut self.energies {
            energies.clear();
        }
        self.blocks.clear();
        self.short_term.clear();
        self.open_sums.fill(0.0);
        self.open_frames = 0;
        self.total_frames = 0;
        self.sample_peak = 0.0;
        self.true_peak.reset();
        self.restart_signal_state();
    }

    /// Test-only: the whole overlapping-window series rebuilt from
    /// `self.energies`, the direct form of what `close_sub_block` appends
    /// incrementally to `blocks` / `short_term`.
    #[cfg(test)]
    #[allow(clippy::cast_precision_loss)]
    fn direct_window_energies(&self, span: usize) -> Vec<f64> {
        let count = self.ends.len();
        if count < span {
            return Vec::new();
        }
        (span - 1..count)
            .map(|j| {
                self.energies
                    .iter()
                    .enumerate()
                    .map(|(channel, energies)| {
                        channel_weight(channel)
                            * (energies[j + 1 - span..=j].iter().sum::<f64>() / span as f64)
                    })
                    .sum::<f64>()
            })
            .collect()
    }

    /// Test-only: §3.3's `m_j` for every complete 400 ms block, rebuilt.
    #[cfg(test)]
    fn block_energies(&self) -> Vec<f64> {
        self.direct_window_energies(SUB_BLOCKS_PER_BLOCK)
    }

    /// Test-only: §3.4's `s_j` for every complete 3 s window, rebuilt.
    #[cfg(test)]
    fn short_term_energies(&self) -> Vec<f64> {
        self.direct_window_energies(SUB_BLOCKS_PER_SHORT_TERM)
    }

    /// §3.3: the block set `J` — indices into `blocks` that pass the absolute
    /// gate and then the relative gate, both strict.
    fn gated_blocks(blocks: &[f64]) -> Vec<usize> {
        let absolute = blocks
            .iter()
            .enumerate()
            .filter(|(_, energy)| **energy > 0.0 && energy_loudness(**energy) > ABSOLUTE_GATE_LUFS)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let Some(ungated) = mean(absolute.iter().map(|index| blocks[*index])) else {
            return Vec::new();
        };
        let relative_gate = energy_loudness(ungated) - RELATIVE_GATE_LU;
        absolute
            .into_iter()
            .filter(|index| energy_loudness(blocks[*index]) > relative_gate)
            .collect()
    }

    fn integrated_energy(blocks: &[f64], gated: &[usize]) -> Option<f64> {
        mean(gated.iter().map(|index| blocks[*index]))
    }

    /// §3.5: Tech 3342 on the short-term series. `relative_gate == false` is
    /// the test-only ungated figure (absolute gate only).
    fn loudness_range_lu(short_term: &[f64], relative_gate: bool) -> Option<f64> {
        let absolute = short_term
            .iter()
            .copied()
            .filter(|energy| *energy > 0.0 && energy_loudness(*energy) > ABSOLUTE_GATE_LUFS)
            .collect::<Vec<_>>();
        let gated = if relative_gate {
            let gate = energy_loudness(mean(absolute.iter().copied())?) - LOUDNESS_RANGE_GATE_LU;
            absolute
                .into_iter()
                .filter(|energy| energy_loudness(*energy) > gate)
                .collect::<Vec<_>>()
        } else {
            absolute
        };
        let count = gated.len();
        if count < 2 {
            return None;
        }
        let mut loudness = gated.into_iter().map(energy_loudness).collect::<Vec<_>>();
        loudness.sort_by(f64::total_cmp);
        Some(loudness[nearest_rank_index(count, 95)] - loudness[nearest_rank_index(count, 10)])
    }

    fn true_peak_hundredths(&self) -> Option<i32> {
        (self.true_peak.peak > 0.0).then(|| hundredths(20.0 * self.true_peak.peak.log10()).ok())?
    }

    /// AU3 §3.10 (F14): `10·log10(E_L / E_R)` over the integrated gate's block
    /// set, clamped to ±6 000; `None` for mono, for an empty gated set, or
    /// when either side's gated energy is zero.
    #[must_use]
    pub fn channel_balance_lu_hundredths(&self) -> Option<i32> {
        if self.channels < 2 {
            return None;
        }
        let gated = Self::gated_blocks(&self.blocks);
        if gated.is_empty() {
            return None;
        }
        let side = |channel: &Vec<f64>| {
            gated
                .iter()
                .map(|index| {
                    let j = index + SUB_BLOCKS_PER_BLOCK - 1;
                    channel[j + 1 - SUB_BLOCKS_PER_BLOCK..=j]
                        .iter()
                        .sum::<f64>()
                })
                .sum::<f64>()
        };
        let left = side(&self.energies[0]);
        let right = side(&self.energies[1]);
        if left <= 0.0 || right <= 0.0 {
            return None;
        }
        hundredths(10.0 * (left / right).log10())
            .ok()
            .map(|balance| balance.clamp(-6_000, 6_000))
    }

    /// The live shape over every completed sub-block: the latest momentary
    /// and short-term values, the integrated value, the range, the running
    /// true peak, and whole seconds to the last completed sub-block.
    #[must_use]
    pub fn snapshot(&self) -> LoudnessSnapshot {
        let blocks = &self.blocks;
        let short_term = &self.short_term;
        let gated = Self::gated_blocks(blocks);
        LoudnessSnapshot {
            momentary_lufs_hundredths: blocks.last().copied().and_then(loudness_hundredths),
            short_term_lufs_hundredths: short_term.last().copied().and_then(loudness_hundredths),
            integrated_lufs_hundredths: Self::integrated_energy(blocks, &gated)
                .and_then(loudness_hundredths),
            loudness_range_lu_hundredths: Self::loudness_range_lu(short_term, true)
                .and_then(|range| hundredths(range).ok()),
            true_peak_dbtp_hundredths: self.true_peak_hundredths(),
            programme_seconds: u32::try_from(
                self.last_block_end().unwrap_or(0) / u64::from(self.sample_rate),
            )
            .unwrap_or(u32::MAX),
        }
    }

    /// Flush the oversampler and convert. Consumes the meter.
    ///
    /// # Errors
    ///
    /// Returns `MediaError::Backend` when a value falls outside the `i32`
    /// hundredths range.
    pub fn finish(mut self) -> Result<AudioLoudness, MediaError> {
        self.true_peak.flush();
        let blocks = &self.blocks;
        let short_term = &self.short_term;
        let gated = Self::gated_blocks(blocks);
        let convert = |energy: Option<f64>| -> Result<Option<i32>, MediaError> {
            energy
                .filter(|energy| *energy > 0.0)
                .map(|energy| hundredths(energy_loudness(energy)))
                .transpose()
        };
        let maximum = |energies: &[f64]| {
            energies.iter().copied().fold(None, |best: Option<f64>, e| {
                Some(best.map_or(e, |best| best.max(e)))
            })
        };
        Ok(AudioLoudness {
            integrated_lufs_hundredths: convert(Self::integrated_energy(blocks, &gated))?,
            sample_peak_dbfs_hundredths: (self.sample_peak > 0.0)
                .then(|| hundredths(20.0 * self.sample_peak.log10()))
                .transpose()?,
            sample_rate: self.sample_rate,
            channels: self.channels(),
            sample_frames: self.total_frames,
            momentary_max_lufs_hundredths: convert(maximum(blocks))?,
            short_term_max_lufs_hundredths: convert(maximum(short_term))?,
            loudness_range_lu_hundredths: Self::loudness_range_lu(short_term, true)
                .map(hundredths)
                .transpose()?,
            true_peak_dbtp_hundredths: (self.true_peak.peak > 0.0)
                .then(|| hundredths(20.0 * self.true_peak.peak.log10()))
                .transpose()?,
        })
    }

    /// Test-only: the momentary series `l_j` in LUFS, `sub_blocks − 3` long.
    /// Reads the incremental series, the one the report is built from.
    #[cfg(test)]
    pub(crate) fn momentary_series_lufs(&self) -> Vec<f64> {
        self.blocks.iter().copied().map(energy_loudness).collect()
    }

    /// Test-only: the short-term series `l^s_j` in LUFS, `sub_blocks − 29` long.
    #[cfg(test)]
    pub(crate) fn short_term_series_lufs(&self) -> Vec<f64> {
        self.short_term
            .iter()
            .copied()
            .map(energy_loudness)
            .collect()
    }

    /// Test-only: the loudness range with the absolute gate only.
    #[cfg(test)]
    pub(crate) fn loudness_range_ungated_lu_hundredths(&self) -> Option<i32> {
        Self::loudness_range_lu(&self.short_term, false).and_then(|range| hundredths(range).ok())
    }
}

/// Measure interleaved PCM in one pass: [`LoudnessMeter::measure`].
///
/// # Errors
///
/// As [`LoudnessMeter::measure`].
pub fn measure_loudness(
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
) -> Result<AudioLoudness, MediaError> {
    LoudnessMeter::measure(samples, sample_rate, channels)
}

// ---- The live meter --------------------------------------------------------

/// AU3 §3.9: the worker's device-rate meter with its 16-entry snapshot ring.
///
/// `fill_ring` pushes every post-clamp chunk it takes from the mixer — up to
/// a second ahead of the loudspeaker — and each completed sub-block records
/// `(block end, snapshot)` here. The worker publishes the entry at or behind
/// the audible position, never the meter's own head.
#[derive(Clone, Debug)]
pub(crate) struct LiveLoudnessMeter {
    meter: LoudnessMeter,
    ring: VecDeque<(u64, LoudnessSnapshot)>,
    fold: Vec<f32>,
}

impl LiveLoudnessMeter {
    /// A meter at the device rate over `min(output_channels, 2)` channels: a
    /// one-channel device is measured as mono, channels beyond the second
    /// are ignored.
    ///
    /// # Errors
    ///
    /// As [`LoudnessMeter::new`].
    pub(crate) fn new(sample_rate: u32, output_channels: u16) -> Result<Self, MediaError> {
        Ok(Self {
            meter: LoudnessMeter::new(sample_rate, output_channels.clamp(1, 2))?,
            ring: VecDeque::with_capacity(LIVE_LOUDNESS_RING_ENTRIES),
            fold: Vec::new(),
        })
    }

    pub(crate) const fn sample_rate(&self) -> u32 {
        self.meter.sample_rate()
    }

    pub(crate) fn channels(&self) -> u16 {
        self.meter.channels()
    }

    #[cfg(test)]
    pub(crate) const fn meter(&self) -> &LoudnessMeter {
        &self.meter
    }

    /// Push one post-clamp chunk of `output_channels` interleaved channels,
    /// keeping the first `min(output_channels, 2)`, and record a ring entry
    /// for every sub-block it completes.
    ///
    /// # Errors
    ///
    /// As [`LoudnessMeter::push`].
    pub(crate) fn push_chunk(
        &mut self,
        chunk: &[f32],
        output_channels: usize,
    ) -> Result<(), MediaError> {
        let Self { meter, ring, fold } = self;
        let output_channels = output_channels.max(1);
        let meter_channels = meter.channels;
        let folded: &[f32] = if output_channels == meter_channels {
            chunk
        } else {
            fold.clear();
            for frame in chunk.chunks_exact(output_channels) {
                fold.extend_from_slice(&frame[..meter_channels.min(frame.len())]);
            }
            fold
        };
        let frames = folded.len() / meter_channels;
        let mut offset = 0_usize;
        while offset < frames {
            let take = meter.frames_to_sub_block_end().min(frames - offset);
            let completed =
                meter.push(&folded[offset * meter_channels..(offset + take) * meter_channels])?;
            if completed > 0 {
                let Some(end) = meter.last_block_end() else {
                    break;
                };
                if ring.len() == LIVE_LOUDNESS_RING_ENTRIES {
                    ring.pop_front();
                }
                ring.push_back((end, meter.snapshot()));
            }
            offset += take;
        }
        Ok(())
    }

    /// The ring entry with the greatest block end at or behind
    /// `audible_frames` (frames since reset), with its key.
    pub(crate) fn published(&self, audible_frames: u64) -> Option<(u64, LoudnessSnapshot)> {
        self.ring
            .iter()
            .rev()
            .find(|(end, _)| *end <= audible_frames)
            .copied()
    }

    /// §3.9 pause: truncate the integration to blocks ending at or before
    /// `frames` and forget the ring entries past it.
    pub(crate) fn truncate_to(&mut self, frames: u64) {
        self.meter.truncate_to(frames);
        self.ring.retain(|(end, _)| *end <= frames);
    }

    /// The snapshot published while paused: the integration as truncated,
    /// with momentary and short-term `None` because their windows describe
    /// audio no longer playing.
    pub(crate) fn paused_snapshot(&self) -> LoudnessSnapshot {
        LoudnessSnapshot {
            momentary_lufs_hundredths: None,
            short_term_lufs_hundredths: None,
            ..self.meter.snapshot()
        }
    }

    pub(crate) fn reset(&mut self) {
        self.meter.reset();
        self.ring.clear();
    }

    #[cfg(test)]
    pub(crate) fn ring_len(&self) -> usize {
        self.ring.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    /// `(peak dBFS, seconds)` segments of a stepped 1 kHz fixture.
    type Segments = Vec<(f64, f64)>;

    fn dbfs(db: f64) -> f64 {
        10.0_f64.powf(db / 20.0)
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn frames(seconds: f64, rate: u32) -> usize {
        (seconds * f64::from(rate)).round() as usize
    }

    /// A sine at `peak` amplitude in every channel, `f64` phase, `f32` output.
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    fn tone(frequency: f64, peak: f64, frames: usize, rate: u32, channels: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|frame| {
                let sample =
                    (peak * (2.0 * PI * frequency * frame as f64 / f64::from(rate)).sin()) as f32;
                std::iter::repeat_n(sample, channels)
            })
            .collect()
    }

    /// A stereo 1 kHz tone whose peak level per channel steps through
    /// `(dBFS, seconds)` segments, phase-continuous (Tech 3341/3342 fixtures).
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    fn stepped_tone(segments: &[(f64, f64)], rate: u32) -> Vec<f32> {
        let mut samples = Vec::new();
        let mut frame = 0_usize;
        for (level_dbfs, seconds) in segments {
            let peak = dbfs(*level_dbfs);
            for _ in 0..frames(*seconds, rate) {
                let sample =
                    (peak * (2.0 * PI * 1_000.0 * frame as f64 / f64::from(rate)).sin()) as f32;
                samples.push(sample);
                samples.push(sample);
                frame += 1;
            }
        }
        samples
    }

    /// R1: a 10 ms raised-cosine fade in and out, applied per frame.
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    fn faded(mut samples: Vec<f32>, rate: u32, channels: usize, fade_ms: f64) -> Vec<f32> {
        let fade = frames(fade_ms / 1_000.0, rate);
        let total = samples.len() / channels;
        for frame in 0..total {
            let position = frame.min(total - 1 - frame);
            if position < fade {
                let gain = 0.5 - 0.5 * (PI * position as f64 / fade as f64).cos();
                for channel in 0..channels {
                    samples[frame * channels + channel] =
                        (f64::from(samples[frame * channels + channel]) * gain) as f32;
                }
            }
        }
        samples
    }

    /// A deterministic xorshift buffer.
    #[allow(clippy::cast_precision_loss)]
    fn noise(frames: usize, channels: usize, amplitude: f32) -> Vec<f32> {
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        (0..frames * channels)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let unit = (state >> 40) as f32 / 8_388_608.0 - 1.0;
                unit * amplitude
            })
            .collect()
    }

    fn measure(samples: &[f32], rate: u32, channels: u16) -> AudioLoudness {
        LoudnessMeter::measure(samples, rate, channels).expect("the fixture should measure")
    }

    fn assert_close(label: &str, observed: Option<i32>, expected: i32, tolerance: i32) {
        let observed = observed.unwrap_or_else(|| panic!("{label} must be measured"));
        assert!(
            (observed - expected).abs() <= tolerance,
            "{label} read {observed}, expected {expected} ±{tolerance}"
        );
    }

    // ---- A5 --------------------------------------------------------------

    /// AU3 §7 item A5: one push, 1 024-frame pushes, and 7-frame pushes of the
    /// same programme produce bit-identical `finish()` output.
    #[test]
    fn the_meter_is_independent_of_frame_aligned_chunking() {
        let mut programme = noise(frames(2.5, RATE), 2, 0.3);
        let tone = tone(997.0, 0.2, frames(2.5, RATE), RATE, 2);
        for (sample, tone) in programme.iter_mut().zip(tone) {
            *sample += tone;
        }
        let once = measure(&programme, RATE, 2);
        for chunk_frames in [1_024_usize, 7] {
            let mut meter = LoudnessMeter::new(RATE, 2).unwrap();
            for chunk in programme.chunks(chunk_frames * 2) {
                meter.push(chunk).unwrap();
            }
            assert_eq!(meter.finish().unwrap(), once, "{chunk_frames}-frame pushes");
        }
        assert!(once.integrated_lufs_hundredths.is_some());
        assert!(
            once.loudness_range_lu_hundredths.is_none(),
            "under two short-term windows"
        );
        assert_eq!(once, measure_loudness(&programme, RATE, 2).unwrap());
    }

    /// AU3 §7 item A5 (F8): a misaligned push is refused and consumes nothing.
    #[test]
    fn a_misaligned_push_is_refused_and_consumes_nothing() {
        let programme = tone(1_000.0, 0.5, frames(0.5, RATE), RATE, 2);
        let mut meter = LoudnessMeter::new(RATE, 2).unwrap();
        meter.push(&programme).unwrap();
        let before = meter.clone();
        let error = meter
            .push(&programme[..3])
            .expect_err("three samples are not stereo frames");
        assert_eq!(
            error,
            MediaError::Backend("interleaved PCM is not aligned to its channel count".to_owned())
        );
        assert_eq!(meter.sample_frames(), before.sample_frames());
        assert_eq!(meter.finish().unwrap(), before.finish().unwrap());
    }

    /// AU3 §7 item A5: the constructor's refusals.
    #[test]
    fn the_meter_refuses_unsupported_rates_and_layouts() {
        assert!(LoudnessMeter::new(999, 2).is_err());
        assert!(LoudnessMeter::new(1_000, 1).is_ok());
        assert!(LoudnessMeter::new(RATE, 0).is_err());
        assert!(LoudnessMeter::new(RATE, 3).is_err());
    }

    #[test]
    fn reports_silence_without_inventing_a_decibel_floor() {
        let result = measure(&vec![0.0; 96_000], RATE, 2);
        assert_eq!(result.integrated_lufs_hundredths, None);
        assert_eq!(result.sample_peak_dbfs_hundredths, None);
        assert_eq!(result.true_peak_dbtp_hundredths, None);
        assert_eq!(result.momentary_max_lufs_hundredths, None);
        assert_eq!(result.short_term_max_lufs_hundredths, None);
        assert_eq!(result.loudness_range_lu_hundredths, None);
        assert_eq!(result.sample_frames, 48_000);
    }

    #[test]
    fn six_decibels_of_gain_moves_both_measurements_six_decibels() {
        let quiet = measure(&tone(1_000.0, 0.1, 96_000, RATE, 2), RATE, 2);
        let loud = measure(&tone(1_000.0, 0.2, 96_000, RATE, 2), RATE, 2);
        let loudness_delta =
            loud.integrated_lufs_hundredths.unwrap() - quiet.integrated_lufs_hundredths.unwrap();
        let peak_delta =
            loud.sample_peak_dbfs_hundredths.unwrap() - quiet.sample_peak_dbfs_hundredths.unwrap();
        let true_peak_delta =
            loud.true_peak_dbtp_hundredths.unwrap() - quiet.true_peak_dbtp_hundredths.unwrap();
        assert!((600..=605).contains(&loudness_delta));
        assert!((600..=605).contains(&peak_delta));
        assert!((600..=605).contains(&true_peak_delta));
    }

    // ---- A6 --------------------------------------------------------------

    fn assert_coefficients(label: &str, observed: [f64; 3], expected: [f64; 3]) {
        for (index, (observed, expected)) in observed.iter().zip(expected).enumerate() {
            assert!(
                (observed - expected).abs() <= 1.0e-6,
                "{label}[{index}] = {observed}, expected {expected}"
            );
        }
    }

    /// AU3 §7 item A6 (N1): the derivation reproduces BS.1770-4 Tables 1–2 at
    /// 48 kHz within 1e-6 (achieved ~1e-15) and N1's 44.1 / 96 kHz values.
    #[test]
    // The RLB numerator is constructed as exactly `[1, −2, 1]`.
    #[allow(clippy::float_cmp)]
    fn k_weighting_reproduces_the_bs1770_tables_and_the_secondary_rates() {
        let at_48k = k_weighting(48_000);
        assert_coefficients(
            "48k shelf b",
            at_48k.shelf_b,
            [
                1.535_124_859_586_97,
                -2.691_696_189_406_38,
                1.198_392_810_852_85,
            ],
        );
        assert_coefficients(
            "48k shelf a",
            at_48k.shelf_a,
            [1.0, -1.690_659_293_182_41, 0.732_480_774_215_85],
        );
        assert_eq!(at_48k.high_pass_b, [1.0, -2.0, 1.0]);
        assert_coefficients(
            "48k high-pass a",
            at_48k.high_pass_a,
            [1.0, -1.990_047_454_833_98, 0.990_072_250_366_21],
        );
        let at_44k = k_weighting(44_100);
        assert_coefficients(
            "44.1k shelf b",
            at_44k.shelf_b,
            [1.530_841_23, -2.650_980_00, 1.169_079_08],
        );
        assert_coefficients(
            "44.1k shelf a",
            at_44k.shelf_a,
            [1.0, -1.663_655_11, 0.712_595_43],
        );
        assert_coefficients(
            "44.1k high-pass a",
            at_44k.high_pass_a,
            [1.0, -1.989_169_67, 0.989_199_04],
        );
        let at_96k = k_weighting(96_000);
        assert_coefficients(
            "96k shelf b",
            at_96k.shelf_b,
            [1.559_714_23, -2.926_741_58, 1.378_261_20],
        );
        assert_coefficients(
            "96k shelf a",
            at_96k.shelf_a,
            [1.0, -1.844_609_47, 0.855_843_32],
        );
        assert_coefficients(
            "96k high-pass a",
            at_96k.high_pass_a,
            [1.0, -1.995_017_54, 0.995_023_76],
        );
    }

    /// AU3 §7 item A6 (R3): a stereo 0 dBFS 997 Hz sine reads exactly 0 / 0 /
    /// −2 hundredths at 48 / 44.1 / 96 kHz; one channel only reads −301; a
    /// stereo tone at peak 0.0708 reads −2300 ±10.
    #[test]
    fn the_997_hz_calibration_tone_reads_its_pinned_values() {
        for (rate, expected) in [(48_000_u32, 0_i32), (44_100, 0), (96_000, -2)] {
            let programme = tone(997.0, 1.0, frames(10.0, rate), rate, 2);
            let measured = measure(&programme, rate, 2);
            assert_eq!(
                measured.integrated_lufs_hundredths,
                Some(expected),
                "stereo 0 dBFS 997 Hz at {rate} Hz"
            );
            assert_eq!(measured.sample_peak_dbfs_hundredths, Some(0));
        }
        let mut one_channel = tone(997.0, 1.0, frames(10.0, RATE), RATE, 2);
        for sample in one_channel.iter_mut().skip(1).step_by(2) {
            *sample = 0.0;
        }
        assert_eq!(
            measure(&one_channel, RATE, 2).integrated_lufs_hundredths,
            Some(-301)
        );
        let programme = tone(997.0, 0.0708, frames(10.0, RATE), RATE, 2);
        assert_close(
            "stereo 997 Hz at 0.0708",
            measure(&programme, RATE, 2).integrated_lufs_hundredths,
            -2_300,
            10,
        );
    }

    // ---- A7 --------------------------------------------------------------

    /// AU3 §7 item A7, per EBU Tech 3341 v3: integrated loudness cases 1–5 at
    /// 1 kHz stereo read −2300 / −3300 / −2300 / −2300 / −2300 ±10.
    #[test]
    fn tech_3341_integrated_cases_1_to_5_read_their_pinned_values() {
        let cases: [(&str, Segments, i32); 5] = [
            ("case 1", vec![(-23.0, 20.0)], -2_300),
            ("case 2", vec![(-33.0, 20.0)], -3_300),
            (
                "case 3",
                vec![(-36.0, 10.0), (-23.0, 60.0), (-36.0, 10.0)],
                -2_300,
            ),
            (
                "case 4",
                vec![
                    (-72.0, 10.0),
                    (-36.0, 10.0),
                    (-23.0, 60.0),
                    (-36.0, 10.0),
                    (-72.0, 10.0),
                ],
                -2_300,
            ),
            (
                "case 5",
                vec![(-26.0, 20.0), (-20.0, 20.1), (-26.0, 20.0)],
                -2_300,
            ),
        ];
        for (label, segments, expected) in cases {
            let measured = measure(&stepped_tone(&segments, RATE), RATE, 2);
            assert_close(label, measured.integrated_lufs_hundredths, expected, 10);
        }
    }

    /// AU3 §7 item A7 (F5): 399 ms has no gating block and 400 ms has one;
    /// both report the sample and true peak.
    #[test]
    fn a_programme_needs_one_complete_gating_block_to_read_integrated() {
        let short = measure(&tone(1_000.0, 0.5, 19_152, RATE, 2), RATE, 2);
        assert_eq!(short.integrated_lufs_hundredths, None);
        assert_eq!(short.momentary_max_lufs_hundredths, None);
        assert_eq!(short.sample_frames, 19_152);
        assert_eq!(short.sample_peak_dbfs_hundredths, Some(-602));
        assert!(short.true_peak_dbtp_hundredths.is_some());
        let block = measure(&tone(1_000.0, 0.5, 19_200, RATE, 2), RATE, 2);
        assert!(block.integrated_lufs_hundredths.is_some());
        assert!(block.momentary_max_lufs_hundredths.is_some());
        assert_eq!(block.sample_frames, 19_200);
        assert_eq!(block.sample_peak_dbfs_hundredths, Some(-602));
        assert!(block.true_peak_dbtp_hundredths.is_some());
    }

    // ---- A8 --------------------------------------------------------------

    /// AU3 §7 item A8, per EBU Tech 3341 v3: case 12 (momentary, alternating
    /// −20 dBFS 0.18 s / −30 dBFS 0.22 s) reads −2300 ±10 on every complete
    /// window; case 9 (short-term, −20 dBFS 1.34 s / −30 dBFS 1.66 s) reads
    /// −2300 ±10 on every complete window.
    #[test]
    fn tech_3341_cases_12_and_9_hold_on_every_complete_window() {
        let mut segments = Vec::new();
        for _ in 0..15 {
            segments.push((-20.0, 0.18));
            segments.push((-30.0, 0.22));
        }
        let mut meter = LoudnessMeter::new(RATE, 2).unwrap();
        meter.push(&stepped_tone(&segments, RATE)).unwrap();
        let momentary = meter.momentary_series_lufs();
        assert_eq!(momentary.len(), 60 - 3);
        for (index, value) in momentary.iter().enumerate() {
            assert!(
                (value + 23.0).abs() <= 0.1,
                "case 12 momentary window {index} read {value}"
            );
        }

        let mut segments = Vec::new();
        for _ in 0..3 {
            segments.push((-20.0, 1.34));
            segments.push((-30.0, 1.66));
        }
        let mut meter = LoudnessMeter::new(RATE, 2).unwrap();
        meter.push(&stepped_tone(&segments, RATE)).unwrap();
        let short_term = meter.short_term_series_lufs();
        assert_eq!(short_term.len(), 90 - 29);
        for (index, value) in short_term.iter().enumerate() {
            assert!(
                (value + 23.0).abs() <= 0.1,
                "case 9 short-term window {index} read {value}"
            );
        }
    }

    /// AU3 §7 item A8 (§3.4): 1 s of silence then a −20 LUFS tone: momentary
    /// `j = 12` reads −2125 ±1 and `j = 13` −2000 ±1; short-term `j = 38`
    /// reads −2015 ±1 and `j = 39` −2000 ±1; the series are `sub_blocks − 3`
    /// and `sub_blocks − 29` long.
    #[test]
    fn the_window_and_hop_step_pins_hold() {
        let mut programme = vec![0.0_f32; frames(1.0, RATE) * 2];
        programme.extend(tone(1_000.0, 0.1, frames(5.0, RATE), RATE, 2));
        let mut meter = LoudnessMeter::new(RATE, 2).unwrap();
        meter.push(&programme).unwrap();
        let momentary = meter.momentary_series_lufs();
        let short_term = meter.short_term_series_lufs();
        assert_eq!(momentary.len(), 60 - 3);
        assert_eq!(short_term.len(), 60 - 29);
        let at = |series: &[f64], j: usize, offset: usize| hundredths(series[j - offset]).unwrap();
        assert_close("momentary j = 12", Some(at(&momentary, 12, 3)), -2_125, 1);
        assert_close("momentary j = 13", Some(at(&momentary, 13, 3)), -2_000, 1);
        assert_close(
            "short-term j = 38",
            Some(at(&short_term, 38, 29)),
            -2_015,
            1,
        );
        assert_close(
            "short-term j = 39",
            Some(at(&short_term, 39, 29)),
            -2_000,
            1,
        );
        let finished = meter.finish().unwrap();
        assert_close(
            "momentary max",
            finished.momentary_max_lufs_hundredths,
            -2_000,
            1,
        );
        assert_close(
            "short-term max",
            finished.short_term_max_lufs_hundredths,
            -2_000,
            1,
        );
    }

    // ---- A9 --------------------------------------------------------------

    /// AU3 §7 item A9, per EBU Tech 3342 v3: loudness range cases 1–4 read
    /// 1 000 / 500 / 2 000 / 1 500 ±10 (case 4 ungated 3 000).
    #[test]
    fn tech_3342_loudness_range_cases_1_to_4_read_their_pinned_values() {
        let cases: [(&str, Segments, i32, Option<i32>); 4] = [
            ("case 1", vec![(-20.0, 20.0), (-30.0, 20.0)], 1_000, None),
            ("case 2", vec![(-20.0, 20.0), (-15.0, 20.0)], 500, None),
            ("case 3", vec![(-40.0, 20.0), (-20.0, 20.0)], 2_000, None),
            (
                "case 4",
                vec![
                    (-50.0, 20.0),
                    (-35.0, 20.0),
                    (-20.0, 20.0),
                    (-35.0, 20.0),
                    (-50.0, 20.0),
                ],
                1_500,
                Some(3_000),
            ),
        ];
        for (label, segments, expected, ungated) in cases {
            let mut meter = LoudnessMeter::new(RATE, 2).unwrap();
            meter.push(&stepped_tone(&segments, RATE)).unwrap();
            if let Some(ungated) = ungated {
                assert_close(
                    &format!("{label} ungated"),
                    meter.loudness_range_ungated_lu_hundredths(),
                    ungated,
                    10,
                );
            }
            assert_close(
                label,
                meter.finish().unwrap().loudness_range_lu_hundredths,
                expected,
                10,
            );
        }
    }

    /// AU3 §7 item A9 (§3.5): the −60 / −15 relative-gate case reads 176 ±10
    /// gated and 4 500 ungated; a single gated window reads `None`.
    #[test]
    fn the_relative_gate_case_and_the_one_window_floor_hold() {
        let mut meter = LoudnessMeter::new(RATE, 2).unwrap();
        meter
            .push(&stepped_tone(&[(-60.0, 20.0), (-15.0, 20.0)], RATE))
            .unwrap();
        assert_close(
            "−60/−15 ungated",
            meter.loudness_range_ungated_lu_hundredths(),
            4_500,
            10,
        );
        assert_close(
            "−60/−15 gated",
            meter.finish().unwrap().loudness_range_lu_hundredths,
            176,
            10,
        );

        // Exactly 30 sub-blocks: one short-term window.
        let one_window = measure(&tone(1_000.0, 0.1, 30 * 4_800, RATE, 2), RATE, 2);
        assert!(one_window.short_term_max_lufs_hundredths.is_some());
        assert_eq!(one_window.loudness_range_lu_hundredths, None);
    }

    // ---- A10 -------------------------------------------------------------

    /// A 16× / 512-tap reference interpolator written here so the meter is
    /// never verified with its own taps (the audio.rs `reference_true_peak`
    /// pattern): the same window family on `x_n = (n − 256) / 16`.
    #[allow(clippy::cast_precision_loss)]
    fn reference_true_peak_16x(samples: &[f32], channels: usize) -> f64 {
        const OVERSAMPLE: usize = 16;
        const PHASE_TAPS: usize = 32;
        const TAPS: usize = OVERSAMPLE * PHASE_TAPS;
        let mut taps = [0.0_f64; TAPS];
        for (index, tap) in taps.iter_mut().enumerate() {
            let n = index as f64;
            let x = (n - (TAPS / 2) as f64) / OVERSAMPLE as f64;
            let window = 0.358_75 - 0.488_29 * (2.0 * PI * n / TAPS as f64).cos()
                + 0.141_28 * (4.0 * PI * n / TAPS as f64).cos()
                - 0.011_68 * (6.0 * PI * n / TAPS as f64).cos();
            let sinc = if x == 0.0 {
                1.0
            } else {
                (PI * x).sin() / (PI * x)
            };
            *tap = sinc * window;
        }
        for phase in 0..OVERSAMPLE {
            let sum: f64 = (0..PHASE_TAPS).map(|k| taps[OVERSAMPLE * k + phase]).sum();
            for k in 0..PHASE_TAPS {
                taps[OVERSAMPLE * k + phase] /= sum;
            }
        }
        let frames = samples.len() / channels;
        let mut peak = 0.0_f64;
        for channel in 0..channels {
            // Emit past the end so the tail is interpolated too.
            for frame in 0..frames + PHASE_TAPS {
                let mut phases = [0.0_f64; OVERSAMPLE];
                for k in 0..PHASE_TAPS {
                    let Some(index) = frame.checked_sub(k) else {
                        break;
                    };
                    if index >= frames {
                        continue;
                    }
                    let sample = f64::from(samples[index * channels + channel]);
                    for (phase, accumulator) in phases.iter_mut().enumerate() {
                        *accumulator += taps[OVERSAMPLE * k + phase] * sample;
                    }
                }
                for value in phases {
                    peak = peak.max(value.abs());
                }
            }
        }
        peak
    }

    /// A 50 ms mono tone at 48 kHz with the R1 fades, at `peak` amplitude and
    /// an initial phase in degrees.
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    fn conformance_tone(frequency: f64, phase_degrees: f64, peak: f64) -> Vec<f32> {
        let phase = phase_degrees.to_radians();
        let samples = (0..frames(0.05, RATE))
            .map(|frame| {
                (peak * (2.0 * PI * frequency * frame as f64 / f64::from(RATE) + phase).sin())
                    as f32
            })
            .collect();
        faded(samples, RATE, 1, 10.0)
    }

    fn true_peak(samples: &[f32], channels: u16) -> i32 {
        measure(samples, RATE, channels)
            .true_peak_dbtp_hundredths
            .expect("a non-silent programme has a true peak")
    }

    /// AU3 §7 item A10, per EBU Tech 3341 v3: cases 15–18 at amplitude 0.5 —
    /// fs/4 at 0°, fs/4 at 45°, fs/6 at 60°, fs/8 at 67.5° — plus 997 Hz and
    /// 20 kHz read −602 within +20 / −40 (design check: −6.0206 on all).
    #[test]
    fn tech_3341_true_peak_cases_15_to_18_read_minus_six_dbtp() {
        let fs = f64::from(RATE);
        for (label, frequency, phase) in [
            ("case 15 fs/4 0°", fs / 4.0, 0.0),
            ("case 16 fs/4 45°", fs / 4.0, 45.0),
            ("case 17 fs/6 60°", fs / 6.0, 60.0),
            ("case 18 fs/8 67.5°", fs / 8.0, 67.5),
            ("997 Hz", 997.0, 0.0),
            ("20 kHz", 20_000.0, 0.0),
        ] {
            let observed = true_peak(&conformance_tone(frequency, phase, 0.5), 1);
            assert!(
                (-642..=-582).contains(&observed),
                "{label} read {observed} dBTP hundredths, expected −602 within +20/−40"
            );
        }
    }

    /// AU3 §7 item A10 (§3.6 edges): a full-scale impulse at the first,
    /// middle, and last sample reads exactly 0 dBTP — the last one only
    /// because `finish()` flushes the oversampler.
    #[test]
    fn a_full_scale_impulse_reads_zero_dbtp_wherever_it_sits() {
        let frames = 4_800_usize;
        for position in [0_usize, frames / 2, frames - 1] {
            let mut programme = vec![0.0_f32; frames * 2];
            programme[position * 2] = 1.0;
            programme[position * 2 + 1] = -1.0;
            let measured = measure(&programme, RATE, 2);
            assert_eq!(
                measured.true_peak_dbtp_hundredths,
                Some(0),
                "impulse at {position}"
            );
            assert_eq!(measured.sample_peak_dbfs_hundredths, Some(0));
        }
    }

    /// AU3 §7 item A10 (§3.6 bias): an fs/4 tone swept over sample phases
    /// 0°–87° in 3° steps never reads under −5 hundredths at full scale.
    #[test]
    fn the_phase_sweep_floor_holds_at_fs_over_four() {
        let mut worst = i32::MAX;
        for step in 0..30 {
            let phase = f64::from(step) * 3.0;
            let observed = true_peak(&conformance_tone(f64::from(RATE) / 4.0, phase, 1.0), 1);
            worst = worst.min(observed);
            assert!(observed >= -5, "fs/4 at {phase}° read {observed}");
            assert!(
                observed <= 0,
                "fs/4 at {phase}° read over full scale: {observed}"
            );
        }
        println!("AU3_TRUE_PEAK_SWEEP_FLOOR {worst}");
    }

    /// AU3 §7 item A10: broadband noise reads within 2 hundredths of the 16×
    /// reference, with the bias printed.
    #[test]
    fn broadband_noise_agrees_with_the_16x_reference() {
        let programme = noise(RATE as usize, 2, 0.5);
        let observed = true_peak(&programme, 2);
        let reference = hundredths(20.0 * reference_true_peak_16x(&programme, 2).log10()).unwrap();
        println!("AU3_TRUE_PEAK_NOISE meter {observed} reference {reference}");
        assert!(
            (observed - reference).abs() <= 2,
            "the 8× meter read {observed}, the 16× reference {reference}"
        );
    }

    /// AU3 §7 item A10 (R5, F7): the eight raw phase sums are within 2e-6 of
    /// one; after normalisation `h[128] == 1.0` and `|h[8k]| < 1e-15` for
    /// `k ≠ 16`, so phase 0 is the identity delayed 16 frames.
    #[test]
    #[allow(clippy::float_cmp)]
    fn the_oversampler_prototype_has_the_pinned_structure() {
        for (phase, sum) in raw_true_peak_meter_phase_sums().iter().enumerate() {
            assert!(
                (sum - 1.0).abs() <= 2.0e-6,
                "raw phase {phase} sums to {sum}"
            );
        }
        let taps = true_peak_meter_taps();
        assert_eq!(taps[128], 1.0);
        for k in 0..TRUE_PEAK_METER_PHASE_TAPS {
            if k != TRUE_PEAK_METER_GROUP_DELAY_FRAMES {
                assert!(
                    taps[8 * k].abs() < 1.0e-15,
                    "phase-0 tap {k} is {}",
                    taps[8 * k]
                );
            }
        }
        for phase in 0..TRUE_PEAK_METER_OVERSAMPLE {
            let sum: f64 = (0..TRUE_PEAK_METER_PHASE_TAPS)
                .map(|k| taps[TRUE_PEAK_METER_OVERSAMPLE * k + phase])
                .sum();
            assert!(
                (sum - 1.0).abs() <= 1.0e-12,
                "normalised phase {phase} sums to {sum}"
            );
        }
        // The group delay is exact: a step through phase 0 arrives 16 frames later.
        let mut meter = TruePeakMeter::new(1);
        for _ in 0..TRUE_PEAK_METER_GROUP_DELAY_FRAMES {
            meter.push(&[0.25]);
        }
        assert!(
            meter.peak < 0.25,
            "phase 0 must not see the step before its delay"
        );
        meter.push(&[0.25]);
        assert!(meter.peak >= 0.25 - 1.0e-12);
    }

    // ---- The live shape --------------------------------------------------

    /// §3.1 / §3.9: `truncate_to` keeps the blocks ending at or before the
    /// position, discards the open sub-block, keeps the peaks, and keys new
    /// sub-blocks from the truncation point; `reset` forgets everything.
    #[test]
    fn truncation_keeps_heard_blocks_and_peaks_only() {
        let mut meter = LoudnessMeter::new(RATE, 2).unwrap();
        meter
            .push(&tone(1_000.0, 0.5, 4_800 * 10 + 1_000, RATE, 2))
            .unwrap();
        assert_eq!(meter.last_block_end(), Some(48_000));
        assert_eq!(meter.sample_frames(), 49_000);
        meter.truncate_to(4_800 * 6 + 100);
        assert_eq!(meter.last_block_end(), Some(4_800 * 6));
        assert_eq!(meter.sample_frames(), 4_800 * 6 + 100);
        let snapshot = meter.snapshot();
        assert_eq!(snapshot.true_peak_dbtp_hundredths, Some(-602));
        assert!(snapshot.integrated_lufs_hundredths.is_some());
        // §3.9: the 100 open frames of the −6 dBFS tone were discarded, not
        // carried into the next sub-block. One block of a 20 dB quieter tone
        // must therefore read exactly what a fresh meter reads for it; a
        // leaked open sum would inflate the first sub-block by 3× (+1.76 dB
        // on the block mean).
        let quiet = tone(1_000.0, 0.05, 4_800 * 4, RATE, 2);
        assert_eq!(meter.push(&quiet).unwrap(), 4);
        assert_eq!(meter.last_block_end(), Some(4_800 * 10 + 100));
        let mut fresh = LoudnessMeter::new(RATE, 2).unwrap();
        fresh.push(&quiet).unwrap();
        let continued = meter.snapshot().momentary_lufs_hundredths.unwrap();
        let expected = fresh.snapshot().momentary_lufs_hundredths.unwrap();
        assert!(
            (continued - expected).abs() <= 1,
            "after truncation the next block read {continued}, a fresh meter {expected}"
        );
        meter.reset();
        assert_eq!(meter.last_block_end(), None);
        assert_eq!(meter.sample_frames(), 0);
        assert_eq!(meter.snapshot(), LoudnessSnapshot::default());
    }

    /// §3.9: the live wrapper records one ring entry per completed sub-block
    /// (however the chunks fall), publishes by audible position, never
    /// leads it, folds a four-channel device to its first two channels, and
    /// truncates the ring with the meter.
    #[test]
    fn the_live_meter_publishes_by_audible_position() {
        let mut live = LiveLoudnessMeter::new(RATE, 2).unwrap();
        let programme = tone(1_000.0, 0.1, frames(2.0, RATE), RATE, 2);
        for chunk in programme.chunks(1_024 * 2) {
            live.push_chunk(chunk, 2).unwrap();
        }
        // Twenty sub-blocks completed; the ring keeps the last sixteen.
        assert_eq!(live.ring_len(), LIVE_LOUDNESS_RING_ENTRIES);
        assert_eq!(live.meter().last_block_end(), Some(96_000));
        assert_eq!(
            live.published(4_800 * 5 - 1),
            None,
            "evicted or not yet audible"
        );
        assert_eq!(
            live.published(4_800 * 5).map(|(end, _)| end),
            Some(4_800 * 5)
        );
        let (end, snapshot) = live.published(50_000).unwrap();
        assert_eq!(end, 48_000);
        assert_eq!(snapshot.programme_seconds, 1);
        assert_close(
            "published integrated",
            snapshot.integrated_lufs_hundredths,
            -2_000,
            5,
        );
        assert_close(
            "published true peak",
            snapshot.true_peak_dbtp_hundredths,
            -2_000,
            1,
        );
        live.truncate_to(50_000);
        assert_eq!(live.published(u64::MAX).map(|(end, _)| end), Some(48_000));
        let paused = live.paused_snapshot();
        assert_eq!(paused.momentary_lufs_hundredths, None);
        assert_eq!(paused.short_term_lufs_hundredths, None);
        assert_eq!(
            paused.integrated_lufs_hundredths,
            snapshot.integrated_lufs_hundredths
        );

        // A single large push still yields one entry per sub-block.
        let mut whole = LiveLoudnessMeter::new(RATE, 2).unwrap();
        whole.push_chunk(&programme[..4_800 * 2 * 5], 2).unwrap();
        assert_eq!(whole.ring_len(), 5);

        // Four device channels: the first two are measured.
        let mut quad = LiveLoudnessMeter::new(RATE, 4).unwrap();
        assert_eq!(quad.channels(), 2);
        let four = programme
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|frame| [frame[0], frame[1], 0.0, 0.0])
            .collect::<Vec<_>>();
        quad.push_chunk(&four[..4_800 * 4 * 5], 4).unwrap();
        assert_eq!(quad.meter().sample_frames(), 4_800 * 5);
        assert_close(
            "quad momentary",
            quad.meter().snapshot().momentary_lufs_hundredths,
            -2_000,
            5,
        );
        quad.reset();
        assert_eq!(quad.ring_len(), 0);
        assert_eq!(quad.meter().sample_frames(), 0);
    }

    /// §3.10 (F14): the channel balance over the gated block set.
    #[test]
    fn channel_balance_follows_the_gated_energy_ratio() {
        let mut programme = tone(1_000.0, 0.2, frames(3.0, RATE), RATE, 2);
        let right = 0.2 * dbfs(-6.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        for (index, sample) in programme.iter_mut().enumerate().skip(1).step_by(2) {
            let frame = (index / 2) as f64;
            *sample = (right * (2.0 * PI * 1_000.0 * frame / f64::from(RATE)).sin()) as f32;
        }
        let mut meter = LoudnessMeter::new(RATE, 2).unwrap();
        meter.push(&programme).unwrap();
        assert_close("−6 dB right", meter.channel_balance_lu_hundredths(), 600, 5);

        let mut left_only = tone(1_000.0, 0.2, frames(3.0, RATE), RATE, 2);
        for sample in left_only.iter_mut().skip(1).step_by(2) {
            *sample = 0.0;
        }
        let mut meter = LoudnessMeter::new(RATE, 2).unwrap();
        meter.push(&left_only).unwrap();
        assert_eq!(meter.channel_balance_lu_hundredths(), None);
        assert!(meter.snapshot().integrated_lufs_hundredths.is_some());

        let mut silent = LoudnessMeter::new(RATE, 2).unwrap();
        silent.push(&vec![0.0; 96_000]).unwrap();
        assert_eq!(silent.channel_balance_lu_hundredths(), None);
        let mut mono = LoudnessMeter::new(RATE, 1).unwrap();
        mono.push(&tone(1_000.0, 0.2, frames(1.0, RATE), RATE, 1))
            .unwrap();
        assert_eq!(mono.channel_balance_lu_hundredths(), None);
    }

    #[test]
    fn nearest_rank_matches_the_contract_indices() {
        assert_eq!(nearest_rank_index(371, 10), 37);
        assert_eq!(nearest_rank_index(371, 95), 352);
        assert_eq!(nearest_rank_index(627, 10), 62);
        assert_eq!(nearest_rank_index(627, 95), 595);
        assert_eq!(nearest_rank_index(200, 10), 19);
        assert_eq!(nearest_rank_index(200, 95), 189);
        assert_eq!(nearest_rank_index(2, 10), 0);
        assert_eq!(nearest_rank_index(2, 95), 1);
    }

    /// §3.9: `hundredths` refuses `i32::MIN` itself, so no measurement can
    /// ever collide with the live path's "unmeasured" sentinel
    /// (`LIVE_LOUDNESS_NONE`), and still accepts one hundredth above it.
    #[test]
    fn the_hundredths_range_check_excludes_the_live_sentinel() {
        assert!(hundredths(f64::from(i32::MIN) / 100.0).is_err());
        assert_eq!(
            hundredths(f64::from(i32::MIN + 1) / 100.0),
            Ok(i32::MIN + 1)
        );
        assert!(hundredths(f64::from(i32::MAX) / 100.0).is_ok());
        assert!(hundredths(f64::from(i32::MAX) / 100.0 + 1.0).is_err());
        assert!(hundredths(f64::NAN).is_err());
        assert!(hundredths(f64::NEG_INFINITY).is_err());
        // The floor a real energy can reach is nowhere near the sentinel.
        assert!(energy_loudness(f64::MIN_POSITIVE / 2.0) > -400_000.0);
    }

    /// §3.3 / §3.4 (§3.1): the `blocks` and `short_term` series maintained
    /// incrementally by `close_sub_block` are bit-identical to rebuilding
    /// them from `energies` — after a long, unevenly chunked programme, and
    /// again after a `truncate_to` pause.
    #[test]
    fn the_incremental_window_series_match_the_direct_computation() {
        let programme = stepped_tone(
            &[
                (-23.0, 2.5),
                (-33.0, 1.7),
                (-13.0, 3.3),
                (-70.0, 2.0),
                (-23.0, 3.5),
            ],
            RATE,
        );
        let mut meter = LoudnessMeter::new(RATE, 2).unwrap();
        // Chunk sizes (in frames) that land short of, exactly on, and well
        // past a 4 800-frame sub-block boundary.
        let sizes = [1_usize, 7, 4_800, 4_799, 9_601, 13, 2, 19_200, 331];
        let mut sizes = sizes.iter().copied().cycle();
        let mut offset = 0_usize;
        while offset < programme.len() {
            let take = (sizes.next().unwrap() * 2).min(programme.len() - offset);
            meter.push(&programme[offset..offset + take]).unwrap();
            offset += take;
        }
        assert_eq!(meter.ends.len(), 130, "13 s of complete sub-blocks");
        assert_eq!(meter.blocks.len(), 130 - (SUB_BLOCKS_PER_BLOCK - 1));
        assert_eq!(
            meter.short_term.len(),
            130 - (SUB_BLOCKS_PER_SHORT_TERM - 1)
        );
        assert_eq!(meter.blocks, meter.block_energies());
        assert_eq!(meter.short_term, meter.short_term_energies());

        // A pause truncation drops the series alongside `ends`.
        meter.truncate_to(37_000);
        assert_eq!(meter.ends.len(), 7);
        assert_eq!(meter.blocks.len(), 4);
        assert_eq!(meter.short_term.len(), 0);
        assert_eq!(meter.blocks, meter.block_energies());
        assert_eq!(meter.short_term, meter.short_term_energies());

        // And the continuation keeps appending against the same origin.
        meter
            .push(&tone(1_000.0, 0.2, frames(4.0, RATE), RATE, 2))
            .unwrap();
        assert_eq!(meter.ends.len(), 47);
        assert_eq!(meter.blocks, meter.block_energies());
        assert_eq!(meter.short_term, meter.short_term_energies());

        // Truncating below one gating block empties both series.
        meter.truncate_to(4_800);
        assert_eq!(meter.blocks, Vec::<f64>::new());
        assert_eq!(meter.short_term, Vec::<f64>::new());
        assert_eq!(meter.blocks, meter.block_energies());

        // And `reset` clears them outright.
        meter.reset();
        assert!(meter.blocks.is_empty() && meter.short_term.is_empty());
        assert_eq!(meter.snapshot(), LoudnessSnapshot::default());
    }

    /// §3.3: `BS1770_CHANNEL_WEIGHTS` is the `G_i` the block and short-term
    /// sums actually apply — unity for the two channels this meter measures,
    /// so mono and stereo are bit-identical to the unweighted sum.
    #[test]
    // The weights are exact constants and the point of the test is that the
    // measured channels multiply by exactly one, bit for bit.
    #[allow(clippy::float_cmp)]
    fn the_channel_weights_are_unity_over_the_measured_channels() {
        assert_eq!(BS1770_CHANNEL_WEIGHTS, [1.0, 1.0, 1.0, 1.41, 1.41]);
        assert_eq!(channel_weight(0), 1.0);
        assert_eq!(channel_weight(1), 1.0);
        assert_eq!(channel_weight(4), 1.41);
        assert_eq!(channel_weight(9), 1.0, "unity past the table");

        for channels in [1_u16, 2] {
            let mut meter = LoudnessMeter::new(RATE, channels).unwrap();
            meter
                .push(&tone(
                    1_000.0,
                    0.5,
                    frames(4.0, RATE),
                    RATE,
                    usize::from(channels),
                ))
                .unwrap();
            let unweighted = (SUB_BLOCKS_PER_BLOCK - 1..meter.ends.len())
                .map(|j| {
                    meter
                        .energies
                        .iter()
                        .map(|energy| {
                            #[allow(clippy::cast_precision_loss)]
                            let span = SUB_BLOCKS_PER_BLOCK as f64;
                            energy[j + 1 - SUB_BLOCKS_PER_BLOCK..=j].iter().sum::<f64>() / span
                        })
                        .sum::<f64>()
                })
                .collect::<Vec<_>>();
            assert_eq!(meter.blocks, unweighted, "{channels} channel(s)");
        }
    }

    /// AU3 §7 item A19 (E14): `dsp.rs`'s module doc records that the AU2
    /// limiter's 4x detector is not the measurement path — that pin is
    /// media's, and this is it.
    #[test]
    fn the_true_peak_detector_note_names_the_au3_meter() {
        // Unwrap the doc comment so the pin is on the sentence, not the
        // line breaks.
        let dsp = include_str!("dsp.rs").replace("\n/// ", " ");
        assert!(
            dsp.contains(
                "It is the limiter's detector only: the measurement path in \
                 `loudness.rs` is an 8× meter conformant by Tech 3341 tolerance \
                 (AU3 §3.6)."
            ),
            "dsp.rs must keep AU3 §7 A19's note pointing at loudness.rs"
        );
    }
}
