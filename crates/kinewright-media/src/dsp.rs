//! AU2 §3.2 and §3.5 signal-processing primitives for the audio bus nodes.
//!
//! Kept beside [`crate::audio`] rather than inside it so the filter algebra,
//! the delay line, and the peak estimator can be read and tested without
//! wading through the mixer. Every coefficient is computed in `f64` from the
//! runtime sample rate; samples cross the boundary as `f32`.

use std::{collections::VecDeque, f64::consts::PI, sync::OnceLock};

/// AU2 §3.2: a state word smaller than this is squelched to zero.
///
/// **Not** a denormal flush: state is `f64`, whose subnormal threshold is about
/// `2.2e-308`, so true denormals are unreachable here. This is a cheap squelch
/// that keeps a settled section from running its state down toward subnormals.
/// Its cost is that a decay tail below about -600 dBFS is truncated, so a
/// filter's tail is not bit-reversible (§6.10).
const STATE_SQUELCH: f64 = 1.0e-30;

/// AU2 §3.2: the fraction of the sample rate every section frequency is
/// clamped to, so a 20 kHz band on a 44.1 kHz device becomes 19 845 Hz rather
/// than an unstable section.
const SECTION_HERTZ_CEILING: f64 = 0.45;

/// AU2 §3.2: the lowest frequency any section is designed at.
const SECTION_HERTZ_FLOOR: f64 = 20.0;

/// AU2 §3.2: clamp one section frequency into the stable band for one rate.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn clamp_section_hertz(hertz: f64, sample_rate: u32) -> f64 {
    let ceiling = (SECTION_HERTZ_CEILING * f64::from(sample_rate.max(1))).max(f64::MIN_POSITIVE);
    hertz.clamp(SECTION_HERTZ_FLOOR.min(ceiling), ceiling)
}

/// AU2 §3.2: one RBJ biquad's coefficients, already divided through by `a0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BiquadCoefficients {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl BiquadCoefficients {
    /// The exact pass-through: `y = x`, and a zero state stays zero.
    pub(crate) const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    fn normalized(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Self {
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    /// A 2nd-order Butterworth high-pass, `Q = 1/sqrt(2)` (AU2 §3.2).
    pub(crate) fn high_pass(hertz: f64, sample_rate: u32) -> Self {
        let w0 = section_omega(hertz, sample_rate);
        let cw = w0.cos();
        let alpha = w0.sin() / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        Self::normalized(
            f64::midpoint(1.0, cw),
            -(1.0 + cw),
            f64::midpoint(1.0, cw),
            1.0 + alpha,
            -2.0 * cw,
            1.0 - alpha,
        )
    }

    /// A low shelf with `S = 1` (AU2 §3.2).
    pub(crate) fn low_shelf(hertz: f64, gain_db: f64, sample_rate: u32) -> Self {
        let w0 = section_omega(hertz, sample_rate);
        let cw = w0.cos();
        let amplitude = shelf_amplitude(gain_db);
        let alpha = w0.sin() / 2.0 * std::f64::consts::SQRT_2;
        let two_root_alpha = 2.0 * amplitude.sqrt() * alpha;
        Self::normalized(
            amplitude * ((amplitude + 1.0) - (amplitude - 1.0) * cw + two_root_alpha),
            2.0 * amplitude * ((amplitude - 1.0) - (amplitude + 1.0) * cw),
            amplitude * ((amplitude + 1.0) - (amplitude - 1.0) * cw - two_root_alpha),
            (amplitude + 1.0) + (amplitude - 1.0) * cw + two_root_alpha,
            -2.0 * ((amplitude - 1.0) + (amplitude + 1.0) * cw),
            (amplitude + 1.0) + (amplitude - 1.0) * cw - two_root_alpha,
        )
    }

    /// A high shelf with `S = 1` (AU2 §3.2).
    pub(crate) fn high_shelf(hertz: f64, gain_db: f64, sample_rate: u32) -> Self {
        let w0 = section_omega(hertz, sample_rate);
        let cw = w0.cos();
        let amplitude = shelf_amplitude(gain_db);
        let alpha = w0.sin() / 2.0 * std::f64::consts::SQRT_2;
        let two_root_alpha = 2.0 * amplitude.sqrt() * alpha;
        Self::normalized(
            amplitude * ((amplitude + 1.0) + (amplitude - 1.0) * cw + two_root_alpha),
            -2.0 * amplitude * ((amplitude - 1.0) + (amplitude + 1.0) * cw),
            amplitude * ((amplitude + 1.0) + (amplitude - 1.0) * cw - two_root_alpha),
            (amplitude + 1.0) - (amplitude - 1.0) * cw + two_root_alpha,
            2.0 * ((amplitude - 1.0) - (amplitude + 1.0) * cw),
            (amplitude + 1.0) - (amplitude - 1.0) * cw - two_root_alpha,
        )
    }

    /// A peaking band (AU2 §3.2).
    pub(crate) fn peaking(hertz: f64, gain_db: f64, q: f64, sample_rate: u32) -> Self {
        let w0 = section_omega(hertz, sample_rate);
        let cw = w0.cos();
        let amplitude = shelf_amplitude(gain_db);
        let alpha = w0.sin() / (2.0 * q.max(f64::MIN_POSITIVE));
        Self::normalized(
            1.0 + alpha * amplitude,
            -2.0 * cw,
            1.0 - alpha * amplitude,
            1.0 + alpha / amplitude,
            -2.0 * cw,
            1.0 - alpha / amplitude,
        )
    }

    /// The section's analytic magnitude in decibels at one frequency.
    ///
    /// `H(z) = (b0 + b1 z^-1 + b2 z^-2) / (1 + a1 z^-1 + a2 z^-2)` evaluated on
    /// the unit circle. This is the transfer function itself, so a test that
    /// compares a measured tone against it is comparing against the design.
    pub(crate) fn magnitude_db(self, hertz: f64, sample_rate: u32) -> f64 {
        let w = 2.0 * PI * hertz / f64::from(sample_rate.max(1));
        let (sin1, cos1) = w.sin_cos();
        let (sin2, cos2) = (2.0 * w).sin_cos();
        let numerator_real = self.b0 + self.b1 * cos1 + self.b2 * cos2;
        let numerator_imaginary = -(self.b1 * sin1 + self.b2 * sin2);
        let denominator_real = 1.0 + self.a1 * cos1 + self.a2 * cos2;
        let denominator_imaginary = -(self.a1 * sin1 + self.a2 * sin2);
        let numerator = numerator_real.hypot(numerator_imaginary);
        let denominator = denominator_real.hypot(denominator_imaginary);
        20.0 * (numerator / denominator.max(f64::MIN_POSITIVE))
            .max(1.0e-30)
            .log10()
    }
}

fn section_omega(hertz: f64, sample_rate: u32) -> f64 {
    2.0 * PI * clamp_section_hertz(hertz, sample_rate) / f64::from(sample_rate.max(1))
}

/// `A = 10^(dB/40)`, the RBJ shelf and peaking amplitude.
fn shelf_amplitude(gain_db: f64) -> f64 {
    if gain_db == 0.0 {
        1.0
    } else {
        10.0_f64.powf(gain_db / 40.0)
    }
}

/// AU2 §3.2: one transposed-direct-form-II section with `f64` state per
/// channel.
#[derive(Debug, Clone)]
pub(crate) struct BiquadSection {
    coefficients: BiquadCoefficients,
    state: Vec<[f64; 2]>,
}

impl BiquadSection {
    pub(crate) fn new(channels: usize) -> Self {
        Self {
            coefficients: BiquadCoefficients::IDENTITY,
            state: vec![[0.0; 2]; channels.max(1)],
        }
    }

    pub(crate) fn set_coefficients(&mut self, coefficients: BiquadCoefficients) {
        self.coefficients = coefficients;
    }

    /// `y = b0*x + s1; s1 = b1*x - a1*y + s2; s2 = b2*x - a2*y`, then the
    /// small-value squelch.
    #[allow(clippy::cast_possible_truncation)]
    pub(crate) fn process(&mut self, frame: &mut [f32]) {
        let coefficients = self.coefficients;
        for (state, sample) in self.state.iter_mut().zip(frame.iter_mut()) {
            let x = f64::from(*sample);
            let y = coefficients.b0 * x + state[0];
            state[0] = coefficients.b1 * x - coefficients.a1 * y + state[1];
            state[1] = coefficients.b2 * x - coefficients.a2 * y;
            if state[0].abs() < STATE_SQUELCH {
                state[0] = 0.0;
            }
            if state[1].abs() < STATE_SQUELCH {
                state[1] = 0.0;
            }
            *sample = y as f32;
        }
    }

    /// Whether every state word of every channel is exactly zero (AU2 A19).
    #[cfg(test)]
    #[allow(clippy::float_cmp)]
    pub(crate) fn is_settled(&self) -> bool {
        self.state
            .iter()
            .all(|state| state[0] == 0.0 && state[1] == 0.0)
    }
}

/// AU2 §3.3/§3.5: a whole-frame delay line of a fixed length.
///
/// A zero-length line is an exact no-op, which is what every pre-AU2 document
/// gets.
#[derive(Debug, Clone)]
pub(crate) struct DelayLine {
    buffer: Vec<f32>,
    frames: usize,
    channels: usize,
    cursor: usize,
}

impl DelayLine {
    pub(crate) fn new(frames: usize, channels: usize) -> Self {
        let channels = channels.max(1);
        Self {
            buffer: vec![0.0; frames.saturating_mul(channels)],
            frames,
            channels,
            cursor: 0,
        }
    }

    /// Swap one sample frame with the frame written `frames` frames ago.
    pub(crate) fn process(&mut self, frame: &mut [f32]) {
        if self.frames == 0 {
            return;
        }
        let base = self.cursor * self.channels;
        for (channel, sample) in frame.iter_mut().enumerate().take(self.channels) {
            std::mem::swap(&mut self.buffer[base + channel], sample);
        }
        self.cursor = (self.cursor + 1) % self.frames;
    }

    /// Delay a whole interleaved buffer in place.
    pub(crate) fn process_buffer(&mut self, samples: &mut [f32], channels: usize) {
        if self.frames == 0 {
            return;
        }
        let channels = channels.max(1);
        for frame in samples.chunks_mut(channels) {
            self.process(frame);
        }
    }
}

/// AU2 §3.5: the oversampling factor of the inter-sample peak estimator.
pub(crate) const TRUE_PEAK_OVERSAMPLE: usize = 4;

/// AU2 §3.5: the estimator's total tap count.
pub(crate) const TRUE_PEAK_TAPS: usize = 48;

/// AU2 §3.5: taps per polyphase branch.
pub(crate) const TRUE_PEAK_PHASE_TAPS: usize = TRUE_PEAK_TAPS / TRUE_PEAK_OVERSAMPLE;

/// AU2 §3.5: the estimator's group delay in input sample frames,
/// `round((TAPS - 1) / 2 / OVERSAMPLE)` = round(5.875).
pub(crate) const TRUE_PEAK_GROUP_DELAY_FRAMES: usize = 6;

/// The Blackman windowed-sinc prototype, normalised per phase.
///
/// Four phases of twelve taps. Each phase sums to one, so a constant input
/// reads back as that constant under every phase.
fn true_peak_taps() -> &'static [f64; TRUE_PEAK_TAPS] {
    static TAPS: OnceLock<[f64; TRUE_PEAK_TAPS]> = OnceLock::new();
    TAPS.get_or_init(|| {
        let mut taps = [0.0_f64; TRUE_PEAK_TAPS];
        for (index, tap) in taps.iter_mut().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let position = index as f64;
            let argument = (position - 23.5) / 4.0;
            let phase = 2.0 * PI * (position + 0.5) / 48.0;
            let window = 0.42 - 0.5 * phase.cos() + 0.08 * (2.0 * phase).cos();
            *tap = (PI * argument).sin() / (PI * argument) * window;
        }
        for phase in 0..TRUE_PEAK_OVERSAMPLE {
            let sum: f64 = (0..TRUE_PEAK_PHASE_TAPS)
                .map(|k| taps[TRUE_PEAK_OVERSAMPLE * k + phase])
                .sum();
            if sum.abs() > f64::MIN_POSITIVE {
                for k in 0..TRUE_PEAK_PHASE_TAPS {
                    taps[TRUE_PEAK_OVERSAMPLE * k + phase] /= sum;
                }
            }
        }
        taps
    })
}

/// AU2 §3.5: a 4x polyphase windowed-sinc inter-sample peak estimator.
///
/// This has the *structure* of ITU-R BS.1770-4 Annex 2 — 48 taps in four
/// phases of twelve — with this contract's own Blackman windowed-sinc
/// coefficients. It is not the ITU coefficient table and reads within about
/// 0.3 dB of a 16x reference (§6.10). It is the limiter's detector only: the
/// measurement path in `loudness.rs` is an 8× meter conformant by Tech 3341
/// tolerance (AU3 §3.6).
#[derive(Debug, Clone)]
pub(crate) struct TruePeakEstimator {
    history: Vec<[f64; TRUE_PEAK_PHASE_TAPS]>,
    cursor: usize,
}

impl TruePeakEstimator {
    pub(crate) fn new(channels: usize) -> Self {
        Self {
            history: vec![[0.0; TRUE_PEAK_PHASE_TAPS]; channels.max(1)],
            cursor: 0,
        }
    }

    /// Record one sample frame; the estimate then describes the continuous
    /// signal around frame `i - TRUE_PEAK_GROUP_DELAY_FRAMES`.
    pub(crate) fn push(&mut self, frame: &[f32]) {
        self.cursor = (self.cursor + 1) % TRUE_PEAK_PHASE_TAPS;
        for (history, sample) in self.history.iter_mut().zip(frame) {
            history[self.cursor] = f64::from(*sample);
        }
    }

    /// The largest interpolated magnitude over every phase and channel.
    // The estimate is a control value compared against an `f32` ceiling.
    #[allow(clippy::cast_possible_truncation)]
    pub(crate) fn estimate(&self) -> f32 {
        let taps = true_peak_taps();
        let mut peak = 0.0_f64;
        for history in &self.history {
            let mut phases = [0.0_f64; TRUE_PEAK_OVERSAMPLE];
            for k in 0..TRUE_PEAK_PHASE_TAPS {
                let sample =
                    history[(self.cursor + TRUE_PEAK_PHASE_TAPS - k) % TRUE_PEAK_PHASE_TAPS];
                for (phase, accumulator) in phases.iter_mut().enumerate() {
                    *accumulator += taps[TRUE_PEAK_OVERSAMPLE * k + phase] * sample;
                }
            }
            for value in phases {
                peak = peak.max(value.abs());
            }
        }
        peak as f32
    }
}

/// AU2 §3.5: the sliding minimum over a trailing window, as a monotonic deque.
///
/// Each frame pushes once and pops amortised once, so the running minimum is
/// amortised O(1) rather than an O(window) rescan.
#[derive(Debug, Clone)]
pub(crate) struct SlidingMinimum {
    window: usize,
    values: VecDeque<(u64, f32)>,
    index: u64,
}

impl SlidingMinimum {
    pub(crate) fn new(window: usize) -> Self {
        let window = window.max(1);
        Self {
            window,
            values: VecDeque::with_capacity(window + 1),
            index: 0,
        }
    }

    /// Push one value and return the minimum of the trailing `window` values.
    ///
    /// Values before the first push are treated as absent rather than as one,
    /// which is sound because every pushed value is already at most one.
    pub(crate) fn push(&mut self, value: f32) -> f32 {
        while let Some(&(_, back)) = self.values.back() {
            if back >= value {
                self.values.pop_back();
            } else {
                break;
            }
        }
        self.values.push_back((self.index, value));
        let expiry = self
            .index
            .saturating_add(1)
            .saturating_sub(self.window as u64);
        while let Some(&(index, _)) = self.values.front() {
            if index < expiry {
                self.values.pop_front();
            } else {
                break;
            }
        }
        self.index = self.index.saturating_add(1);
        self.values.front().map_or(value, |&(_, minimum)| minimum)
    }
}

/// AU2 §3.5: a running boxcar mean of a fixed length, primed with ones.
///
/// The running sum is kept in `f64`, and a window holding nothing but exact
/// ones short-circuits to exactly `1.0` rather than to the accumulated sum, so
/// the under-ceiling identity is bit-exact at every rate and stays exact for
/// the life of the processor however long it has been running.
#[derive(Debug, Clone)]
pub(crate) struct Boxcar {
    values: Vec<f32>,
    cursor: usize,
    sum: f64,
    /// How many stored values are exactly `1.0`.
    unity: usize,
}

impl Boxcar {
    pub(crate) fn new(length: usize) -> Self {
        let length = length.max(1);
        Self {
            values: vec![1.0; length],
            cursor: 0,
            #[allow(clippy::cast_precision_loss)]
            sum: length as f64,
            unity: length,
        }
    }

    // The mean is a control value multiplied into an `f32` mix.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::float_cmp
    )]
    pub(crate) fn push(&mut self, value: f32) -> f32 {
        let replaced = self.values[self.cursor];
        self.sum -= f64::from(replaced);
        self.unity -= usize::from(replaced == 1.0);
        self.values[self.cursor] = value;
        self.sum += f64::from(value);
        self.unity += usize::from(value == 1.0);
        self.cursor = (self.cursor + 1) % self.values.len();
        if self.unity == self.values.len() {
            return 1.0;
        }
        (self.sum / self.values.len() as f64) as f32
    }
}

/// AU2 §3.3: a boxcar running mean square over a fixed window of sample
/// frames, allocated once when the runtime is built.
#[derive(Debug, Clone)]
pub(crate) struct MeanSquareWindow {
    values: Vec<f64>,
    cursor: usize,
    sum: f64,
}

impl MeanSquareWindow {
    pub(crate) fn new(frames: usize) -> Self {
        let frames = frames.max(1);
        Self {
            values: vec![0.0; frames],
            cursor: 0,
            sum: 0.0,
        }
    }

    /// Push one frame's mean square and return the window's running mean.
    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn push(&mut self, mean_square: f64) -> f64 {
        self.sum -= self.values[self.cursor];
        self.values[self.cursor] = mean_square;
        self.sum += mean_square;
        self.cursor = (self.cursor + 1) % self.values.len();
        (self.sum / self.values.len() as f64).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AU2 §3.5: every phase of the prototype sums to one after normalisation.
    #[test]
    fn every_true_peak_phase_sums_to_one() {
        let taps = true_peak_taps();
        for phase in 0..TRUE_PEAK_OVERSAMPLE {
            let sum: f64 = (0..TRUE_PEAK_PHASE_TAPS)
                .map(|k| taps[TRUE_PEAK_OVERSAMPLE * k + phase])
                .sum();
            assert!(
                (sum - 1.0).abs() < 1.0e-12,
                "phase {phase} summed to {sum} rather than 1"
            );
        }
    }

    /// AU2 §3.5: the monotonic deque agrees with a naive trailing rescan.
    #[test]
    #[allow(clippy::float_cmp)]
    fn the_sliding_minimum_agrees_with_a_rescan() {
        let values = [0.9_f32, 0.4, 0.7, 1.0, 0.2, 0.8, 0.8, 0.1, 1.0, 1.0, 0.5];
        for window in 1..=6 {
            let mut sliding = SlidingMinimum::new(window);
            for (index, value) in values.iter().enumerate() {
                let start = (index + 1).saturating_sub(window);
                let expected = values[start..=index]
                    .iter()
                    .copied()
                    .fold(f32::INFINITY, f32::min);
                assert_eq!(sliding.push(*value), expected, "window {window} at {index}");
            }
        }
    }

    /// AU2 §3.5: a boxcar holding only ones means exactly one.
    #[test]
    #[allow(clippy::float_cmp)]
    fn a_boxcar_of_ones_means_exactly_one() {
        for length in [1_usize, 3, 42, 480] {
            let mut boxcar = Boxcar::new(length);
            for _ in 0..=(2 * length) {
                assert_eq!(boxcar.push(1.0), 1.0, "boxcar of {length}");
            }
        }
    }

    /// AU2 §3.2: a zero-length delay line is an exact no-op.
    #[test]
    #[allow(clippy::float_cmp)]
    fn a_zero_length_delay_line_passes_samples_through() {
        let mut line = DelayLine::new(0, 2);
        let mut frame = [0.25_f32, -0.5];
        line.process(&mut frame);
        assert_eq!(frame, [0.25, -0.5]);
    }

    /// AU2 §3.2: the frequency clamp keeps a 20 kHz request stable at 44.1 kHz.
    #[test]
    #[allow(clippy::float_cmp)]
    fn the_section_frequency_clamp_holds_at_forty_five_percent() {
        assert_eq!(clamp_section_hertz(20_000.0, 44_100), 19_845.0);
        assert_eq!(clamp_section_hertz(20_000.0, 48_000), 20_000.0);
        assert_eq!(clamp_section_hertz(5.0, 48_000), 20.0);
    }
}
