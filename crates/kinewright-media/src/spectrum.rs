//! AU2 §5.9: the in-house Welch third-octave spectrum.
//!
//! No new dependency: the workspace pins every version with `=` and the media
//! crate carries no DSP crate, so the transform is an iterative radix-2
//! Cooley-Tukey over `f64` complex pairs with a bit-reversal permutation.
//!
//! The transform is the **unnormalised forward DFT**,
//! `X_k = sum_n w[n] x[n] e^{-2 pi i k n / N}`, with no `1/N` in the forward
//! direction. Under it Parseval gives `sum_k |X_k|^2 = N * sum_n (w[n]x[n])^2`,
//! which is why the one-sided power estimate divides by `N * sum(w^2)`: without
//! the `N` every band reads `10*log10(N) = +42.14 dB` high.

use std::sync::OnceLock;

use kinewright_core::SpectrumBand;

use crate::loudness::nearest_rank_index;

/// AU2 §5.9: the Welch segment length in sample frames.
pub(crate) const SPECTRUM_SEGMENT_FRAMES: usize = 16_384;

/// AU2 §5.9: the hop between segments, 50 % overlap.
pub(crate) const SPECTRUM_HOP_FRAMES: usize = SPECTRUM_SEGMENT_FRAMES / 2;

/// AU2 §5.9/A23: Welch needs two segments, so a measurement needs
/// `2 * 16 384 - 8 192` sample frames — 512 ms at 48 kHz. A shorter range is
/// rejected rather than returning a degenerate spectrum.
pub(crate) const SPECTRUM_MINIMUM_FRAMES: u64 =
    (2 * SPECTRUM_SEGMENT_FRAMES - SPECTRUM_HOP_FRAMES) as u64;

/// AU5 §3.7 rule 61: the noise profile's analysis window, in sample frames.
///
/// 4 096 rather than AU2's 16 384 so ten windows fit inside half a second of
/// found silence. The mismatch between this window and the denoiser's 512-point
/// runtime window is absorbed entirely by AU5 §3.3 rule 44, because a band
/// *level* is window-independent by construction.
pub(crate) const NOISE_PROFILE_SEGMENT_FRAMES: usize = 4_096;

/// AU5 §3.7 rule 61: the profile's hop, 50 % overlap.
pub(crate) const NOISE_PROFILE_HOP_FRAMES: usize = NOISE_PROFILE_SEGMENT_FRAMES / 2;

/// AU5 §3.7 rule 61: `SEGMENT + 9 * HOP` — 469.3 ms at 48 kHz, exactly ten
/// windows, the fewest a percentile means anything over.
///
/// [`SPECTRUM_MINIMUM_FRAMES`] is 24 576 and would refuse this range outright,
/// which is why AU5 §0 R33 parameterises [`third_octave_spectrum`] rather than
/// reusing it as it stood.
pub(crate) const NOISE_PROFILE_MINIMUM_FRAMES: u64 =
    (NOISE_PROFILE_SEGMENT_FRAMES + 9 * NOISE_PROFILE_HOP_FRAMES) as u64;

/// AU5 §3.7 rule 61(ii): the profile reduces over windows with the
/// **20th-percentile** band level rather than a Welch mean, so a stray word or
/// a door close inside the learned range cannot raise the floor.
pub(crate) const NOISE_PROFILE_PERCENT: usize = 20;

/// AU2 §5.9: the 31 ISO nominal third-octave centres, in tenths of a hertz,
/// low to high — nominally 20 Hz through 20 kHz.
pub(crate) const THIRD_OCTAVE_CENTERS_TENTHS: [u32; 31] = [
    200, 250, 315, 400, 500, 630, 800, 1_000, 1_250, 1_600, 2_000, 2_500, 3_150, 4_000, 5_000,
    6_300, 8_000, 10_000, 12_500, 16_000, 20_000, 25_000, 31_500, 40_000, 50_000, 63_000, 80_000,
    100_000, 125_000, 160_000, 200_000,
];

/// AU2 §5.9: a band power below this reads as silence and reports `None`.
///
/// AU5 §0 R21/R34 promoted it to `pub(crate)`: AU5 §3.8 rule 65 and §3.9 rule
/// 68 both name it from `export.rs`, a different module in the same crate.
pub(crate) const SILENCE_POWER: f64 = 1e-12;

/// AU2 §5.9: the Hann main lobe is four bins wide.
const MAIN_LOBE_BINS: f64 = 4.0;

/// One complex value of the in-house transform.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Complex {
    pub(crate) re: f64,
    pub(crate) im: f64,
}

impl Complex {
    pub(crate) const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    fn magnitude_squared(self) -> f64 {
        self.re.mul_add(self.re, self.im * self.im)
    }
}

/// AU5 §3.1 rule 28: the largest stage the twiddle table serves, `2^17`.
///
/// Comfortably over the 16 384-point Welch window `third_octave_spectrum` uses
/// and the 1 024-point window the denoiser reaches at 96 kHz. A longer
/// transform still runs — it falls back to computing its own stage table — so
/// the ceiling is a memory bound, never a correctness one.
const FFT_MAX_STAGE_LOG2: usize = 17;

/// AU5 §3.1 rule 28: per-stage twiddles, computed once per stage size.
static STAGE_TWIDDLES: [OnceLock<Vec<Complex>>; FFT_MAX_STAGE_LOG2 + 1] =
    [const { OnceLock::new() }; FFT_MAX_STAGE_LOG2 + 1];

/// AU5 §3.1 rule 29, normative on bit-identity: `e^(-2 pi i k / m)` for
/// `k in 0..m/2`, computed by the **identical expression** the butterfly loop
/// used before AU5 — `(-2.0 * PI / m as f64) * k as f64`, then `sin_cos`.
///
/// Keyed by **stage size**, not by transform length, precisely so the `f64`
/// argument handed to `sin_cos` is unchanged. A table keyed by transform length
/// would have to compute `e^(-2 pi i (k N / m) / N)`, whose argument differs
/// from `e^(-2 pi i k / m)` in the last ulp for some `k`, and that would move
/// `measure_mix_spectrum`'s goldens.
#[allow(clippy::cast_precision_loss)]
fn compute_stage_twiddles(stage: usize) -> Vec<Complex> {
    let angle = -2.0 * std::f64::consts::PI / stage as f64;
    (0..stage / 2)
        .map(|offset| {
            let theta = angle * offset as f64;
            let (sin, cos) = theta.sin_cos();
            Complex::new(cos, sin)
        })
        .collect()
}

/// AU5 §3.1 rule 28: the cached table for one stage size.
fn stage_twiddles(stage: usize) -> &'static [Complex] {
    let log2 = stage.trailing_zeros() as usize;
    STAGE_TWIDDLES[log2].get_or_init(|| compute_stage_twiddles(stage))
}

/// AU2 §5.9: the unnormalised forward DFT, in place.
///
/// `data.len()` must be a power of two; anything else is left untouched, which
/// only a caller bug can produce.
///
/// AU5 §3.1 rule 28: the per-butterfly `sin_cos` is served from
/// [`stage_twiddles`], which measured 17.1 us per 512-point transform before
/// the table — 25.6 ms of CPU per second of 48 kHz stereo through a denoiser,
/// or about 15 s of pure FFT for a seek at minute 10, because AU2 §3.7's
/// preroll runs every prerolled frame through the STFT. The arithmetic is
/// unchanged, so every transform in the workspace is bit-identical to its
/// pre-AU5 result (rule 29, A5).
pub(crate) fn forward_fft(data: &mut [Complex]) {
    let length = data.len();
    if length < 2 || !length.is_power_of_two() {
        return;
    }
    // Bit-reversal permutation.
    let mut target = 0_usize;
    for source in 1..length {
        let mut bit = length >> 1;
        while target & bit != 0 {
            target ^= bit;
            bit >>= 1;
        }
        target |= bit;
        if source < target {
            data.swap(source, target);
        }
    }
    // Iterative Cooley-Tukey butterflies, smallest stage first.
    let mut stage = 2_usize;
    while stage <= length {
        let overflow;
        let twiddles: &[Complex] = if stage.trailing_zeros() as usize <= FFT_MAX_STAGE_LOG2 {
            stage_twiddles(stage)
        } else {
            overflow = compute_stage_twiddles(stage);
            &overflow
        };
        for block in (0..length).step_by(stage) {
            for (offset, twiddle) in twiddles.iter().enumerate() {
                let (sin, cos) = (twiddle.im, twiddle.re);
                let even = data[block + offset];
                let odd = data[block + offset + stage / 2];
                let rotated = Complex::new(
                    cos.mul_add(odd.re, -(sin * odd.im)),
                    sin.mul_add(odd.re, cos * odd.im),
                );
                data[block + offset] = Complex::new(even.re + rotated.re, even.im + rotated.im);
                data[block + offset + stage / 2] =
                    Complex::new(even.re - rotated.re, even.im - rotated.im);
            }
        }
        stage <<= 1;
    }
}

/// AU5 §3.1 rule 26: the normalised inverse of [`forward_fft`], by conjugation.
///
/// `ifft(X)[n] = conj(fft(conj(X))[n]) / N`. Exact up to rounding because
/// `forward_fft` is unnormalised: the only arithmetic added is a sign flip on
/// the imaginary parts and one division per bin. No new transform and no new
/// crate — the workspace pins every dependency with `=` and this module's own
/// header states the no-new-dependency rule.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn inverse_fft(data: &mut [Complex]) {
    let length = data.len();
    if length < 2 || !length.is_power_of_two() {
        return;
    }
    for value in data.iter_mut() {
        value.im = -value.im;
    }
    forward_fft(data);
    let scale = 1.0 / length as f64;
    for value in data.iter_mut() {
        value.re *= scale;
        value.im = -(value.im * scale);
    }
}

/// AU2 §5.9: one Welch measurement of one interleaved stem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpectrumMeasurement {
    /// How many Welch segments the average covers.
    pub(crate) segments: u32,
    /// The 31 ISO third-octave bands, low to high.
    pub(crate) bands: Vec<SpectrumBand>,
}

/// AU2 §5.9: the exact third-octave centre `f_c = 1000 * 2^((k - 17)/3)`.
///
/// `pub(crate)` for AU5 §3.3 rule 44, whose runtime interpolation is against
/// `log10(f)` at these exact centres rather than at the nominal ones.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn exact_band_center(index: usize) -> f64 {
    1000.0 * 2.0_f64.powf((index as f64 - 17.0) / 3.0)
}

/// AU2 §5.9: the exact third-octave edges around `f_c = 1000 * 2^(k/3)` for `k`
/// in `-17..=13`: `[f_c * 2^(-1/6), f_c * 2^(1/6))`.
fn band_edges(index: usize) -> (f64, f64) {
    let exact_center = exact_band_center(index);
    let ratio = 2.0_f64.powf(1.0 / 6.0);
    (exact_center / ratio, exact_center * ratio)
}

/// AU5 §3.3 rule 44: the same edges, for the runtime's `bins_in_band` count.
pub(crate) fn third_octave_band_edges(index: usize) -> (f64, f64) {
    band_edges(index)
}

/// AU2 §5.9: the third-octave spectrum of one interleaved stem.
///
/// Hann window of `segment_frames`, hop `hop_frames`, trailing partial segment
/// discarded, power averaged across every segment and every channel. Returns
/// `None` when the stem holds fewer than `minimum_frames` sample frames, which
/// the caller rejects with
/// [`kinewright_core::MediaError::MixSpectrumRangeTooShort`] before it ever
/// gets here.
///
/// **AU5 §0 R33 parameterised the three window figures.** AU2's callers pass
/// [`SPECTRUM_SEGMENT_FRAMES`] / [`SPECTRUM_HOP_FRAMES`] /
/// [`SPECTRUM_MINIMUM_FRAMES`] and their goldens are pinned byte-unchanged
/// (A4); AU5's profile passes 4 096 / 2 048 / 22 528, which
/// `SPECTRUM_MINIMUM_FRAMES` would otherwise refuse. The arithmetic below is
/// unchanged in shape and in **summation order**, so the parameterisation
/// cannot move a golden.
pub(crate) fn third_octave_spectrum(
    samples: &[f32],
    channels: usize,
    sample_rate: u32,
    segment_frames: usize,
    hop_frames: usize,
    minimum_frames: u64,
) -> Option<SpectrumMeasurement> {
    let channels = channels.max(1);
    let frames = samples.len() / channels;
    if u64::try_from(frames).unwrap_or(0) < minimum_frames || frames < segment_frames {
        return None;
    }
    let segment_count = (frames - segment_frames) / hop_frames.max(1) + 1;
    // The Hann window and the `sum(w^2)` its normalisation needs.
    let window = hann_window(segment_frames);
    let window_power: f64 = window.iter().map(|weight| weight * weight).sum();
    // The transform is unnormalised, so the estimate divides by `N * sum(w^2)`.
    #[allow(clippy::cast_precision_loss)]
    let normalisation = segment_frames as f64 * window_power;

    let mut power = vec![0.0_f64; segment_frames / 2 + 1];
    let mut scratch = vec![Complex::default(); segment_frames];
    for segment in 0..segment_count {
        let start = segment * hop_frames;
        for channel in 0..channels {
            for (index, slot) in scratch.iter_mut().enumerate() {
                let sample = samples[(start + index) * channels + channel];
                *slot = Complex::new(f64::from(sample) * window[index], 0.0);
            }
            forward_fft(&mut scratch);
            for (bin, accumulated) in power.iter_mut().enumerate() {
                let magnitude = scratch[bin].magnitude_squared();
                let one_sided = if bin == 0 || bin == segment_frames / 2 {
                    magnitude
                } else {
                    2.0 * magnitude
                };
                *accumulated += one_sided / normalisation;
            }
        }
    }
    // The segment and channel counts are small integers.
    #[allow(clippy::cast_precision_loss)]
    let averages = (segment_count * channels) as f64;
    for accumulated in &mut power {
        *accumulated /= averages;
    }

    let bin_width = f64::from(sample_rate) / normalised_length(segment_frames);
    let main_lobe = MAIN_LOBE_BINS * bin_width;
    let nyquist = f64::from(sample_rate) / 2.0;
    let bands = THIRD_OCTAVE_CENTERS_TENTHS
        .iter()
        .enumerate()
        .map(|(index, center)| {
            let (low, high) = band_edges(index);
            SpectrumBand {
                center_hertz_tenths: *center,
                level_dbfs_hundredths: band_level_hundredths(
                    &power,
                    low,
                    high.min(nyquist),
                    bin_width,
                ),
                window_limited: high - low < main_lobe,
            }
        })
        .collect();
    Some(SpectrumMeasurement {
        // The segment count is far inside `u32`.
        segments: u32::try_from(segment_count).unwrap_or(u32::MAX),
        bands,
    })
}

/// AU5 §3.7: one learned noise profile, in `band_level_hundredths`' own unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoiseProfileMeasurement {
    /// How many whole analysis windows the percentile is taken over.
    pub(crate) windows: u32,
    /// The 31 ISO third-octave bands, low to high, in **hundredths** of a dB on
    /// AU5 §3.3 rule 43's scale. `None` for a band that read silence in every
    /// window, which rule 43(i) writes as the neutral.
    pub(crate) bands: Vec<Option<i32>>,
}

/// AU5 §3.7 rule 61(ii): the **20th-percentile** band level over the windows.
///
/// The one thing this does not share with [`third_octave_spectrum`] is the
/// reduction: a Welch mean over windows would let one door close inside the
/// learned range raise the floor for the whole recording, so each window's 31
/// band levels are computed on their own and each band's `NOISE_PROFILE_PERCENT`
/// percentile is taken with [`nearest_rank_index`], AU3's ranking rule. A band
/// that read silence in a window is simply absent from that band's population.
pub(crate) fn third_octave_band_percentile(
    samples: &[f32],
    channels: usize,
    sample_rate: u32,
    segment_frames: usize,
    hop_frames: usize,
    minimum_frames: u64,
    percent: usize,
) -> Option<NoiseProfileMeasurement> {
    let channels = channels.max(1);
    let frames = samples.len() / channels;
    if u64::try_from(frames).unwrap_or(0) < minimum_frames || frames < segment_frames {
        return None;
    }
    let segment_count = (frames - segment_frames) / hop_frames.max(1) + 1;
    let window = hann_window(segment_frames);
    let window_power: f64 = window.iter().map(|weight| weight * weight).sum();
    #[allow(clippy::cast_precision_loss)]
    let normalisation = segment_frames as f64 * window_power;
    let bin_width = f64::from(sample_rate) / normalised_length(segment_frames);
    let nyquist = f64::from(sample_rate) / 2.0;

    let mut populations: Vec<Vec<i32>> = vec![Vec::new(); THIRD_OCTAVE_CENTERS_TENTHS.len()];
    let mut power = vec![0.0_f64; segment_frames / 2 + 1];
    let mut scratch = vec![Complex::default(); segment_frames];
    for segment in 0..segment_count {
        let start = segment * hop_frames;
        power.fill(0.0);
        for channel in 0..channels {
            for (index, slot) in scratch.iter_mut().enumerate() {
                let sample = samples[(start + index) * channels + channel];
                *slot = Complex::new(f64::from(sample) * window[index], 0.0);
            }
            forward_fft(&mut scratch);
            for (bin, accumulated) in power.iter_mut().enumerate() {
                let magnitude = scratch[bin].magnitude_squared();
                let one_sided = if bin == 0 || bin == segment_frames / 2 {
                    magnitude
                } else {
                    2.0 * magnitude
                };
                *accumulated += one_sided / normalisation;
            }
        }
        #[allow(clippy::cast_precision_loss)]
        let averages = channels as f64;
        for accumulated in &mut power {
            *accumulated /= averages;
        }
        for (index, population) in populations.iter_mut().enumerate() {
            let (low, high) = band_edges(index);
            if let Some(level) = band_level_hundredths(&power, low, high.min(nyquist), bin_width) {
                population.push(level);
            }
        }
    }

    let bands = populations
        .into_iter()
        .map(|mut population| {
            if population.is_empty() {
                return None;
            }
            population.sort_unstable();
            Some(population[nearest_rank_index(population.len(), percent)])
        })
        .collect();
    Some(NoiseProfileMeasurement {
        windows: u32::try_from(segment_count).unwrap_or(u32::MAX),
        bands,
    })
}

/// The segment length as an `f64`; a power of two, so the conversion is exact.
#[allow(clippy::cast_precision_loss)]
fn normalised_length(segment_frames: usize) -> f64 {
    segment_frames as f64
}

/// AU2 §5.9: the **overlap-weighted** sum of the one-sided estimate over one
/// band's edges — each bin contributes in proportion to the fraction of its
/// `bin_width` that falls inside the band.
///
/// Never a "bin centre inside the band" test, which would leave the 20 Hz and
/// 40 Hz bands empty. `level = 10*log10(P / 0.5)`, so a full-scale sine inside
/// one band reads exactly `0.00 dBFS`.
fn band_level_hundredths(power: &[f64], low: f64, high: f64, bin_width: f64) -> Option<i32> {
    if high <= low {
        return None;
    }
    let mut band_power = 0.0_f64;
    // Bin `k` is centred on `k * bin_width` and half a bin wide either side.
    let first = ((low / bin_width) - 0.5).floor().max(0.0);
    let last = ((high / bin_width) + 0.5).ceil();
    // Both bounds come from a bounded frequency range and a positive bin width.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let range = (first as usize)..=(last as usize).min(power.len().saturating_sub(1));
    for bin in range {
        // The bin index is small; the conversion is exact.
        #[allow(clippy::cast_precision_loss)]
        let center = bin as f64 * bin_width;
        let overlap = (center + bin_width / 2.0).min(high) - (center - bin_width / 2.0).max(low);
        if overlap > 0.0 {
            band_power += power[bin] * (overlap / bin_width).min(1.0);
        }
    }
    if band_power < SILENCE_POWER {
        return None;
    }
    // A full-scale sine carries 0.5 of one-sided power, which is 0 dBFS.
    let level = 10.0 * (band_power / 0.5).log10();
    // Levels are bounded well inside `i32` at hundredths of a decibel.
    #[allow(clippy::cast_possible_truncation)]
    Some((level * 100.0).round() as i32)
}

/// A periodic Hann window of `length` points.
///
/// `pub(crate)` per AU5 §0 R21: the denoiser's analysis and synthesis window is
/// literally these numbers, so `third_octave_spectrum`'s window and the
/// runtime's cannot drift apart.
pub(crate) fn hann_window(length: usize) -> Vec<f64> {
    // The window length is a power of two well inside `f64`'s exact range.
    #[allow(clippy::cast_precision_loss)]
    let denominator = length as f64;
    (0..length)
        .map(|index| {
            #[allow(clippy::cast_precision_loss)]
            let phase = 2.0 * std::f64::consts::PI * index as f64 / denominator;
            0.5 * (1.0 - phase.cos())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An O(N^2) direct transform, written from the definition.
    #[allow(clippy::cast_precision_loss)]
    fn direct_dft(input: &[Complex]) -> Vec<Complex> {
        let length = input.len();
        (0..length)
            .map(|bin| {
                let mut sum = Complex::default();
                for (index, value) in input.iter().enumerate() {
                    let theta =
                        -2.0 * std::f64::consts::PI * bin as f64 * index as f64 / length as f64;
                    let (sin, cos) = theta.sin_cos();
                    sum.re += value.re * cos - value.im * sin;
                    sum.im += value.re * sin + value.im * cos;
                }
                sum
            })
            .collect()
    }

    fn pseudo_random(count: usize) -> Vec<Complex> {
        let mut state = 0x2545_F491_4F6C_DD1D_u64;
        (0..count)
            .map(|_| {
                let mut next = || {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    #[allow(clippy::cast_precision_loss)]
                    let unit = (state >> 11) as f64 / (1_u64 << 53) as f64;
                    unit.mul_add(2.0, -1.0)
                };
                Complex::new(next(), next())
            })
            .collect()
    }

    /// AU2 §7 item B13.
    #[test]
    fn the_in_house_fft_matches_a_direct_dft_at_sixty_four_points() {
        let input = pseudo_random(64);
        let expected = direct_dft(&input);
        let mut actual = input;
        forward_fft(&mut actual);
        let difference = expected
            .iter()
            .zip(&actual)
            .map(|(expected, actual)| {
                (expected.re - actual.re)
                    .abs()
                    .max((expected.im - actual.im).abs())
            })
            .fold(0.0_f64, f64::max);
        assert!(
            difference <= 1.0e-9,
            "the radix-2 transform differs from the direct DFT by {difference}"
        );
    }

    /// A mono full-scale tone, exact at 48 kHz.
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    fn tone(frequency: f64, rate: u32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|frame| {
                let phase = 2.0 * std::f64::consts::PI * frequency * frame as f64 / f64::from(rate);
                phase.sin() as f32
            })
            .collect()
    }

    fn level_db(bands: &[SpectrumBand], index: usize) -> Option<f64> {
        bands[index]
            .level_dbfs_hundredths
            .map(|hundredths| f64::from(hundredths) / 100.0)
    }

    /// AU2 §7 item B13: a full-scale 1 kHz sine reads `0.00 dBFS` in the
    /// 1 000 Hz band — which fails by 42.14 dB without the `1/N` of A5 — and is
    /// at least 40 dB down two octaves either way.
    #[test]
    fn a_full_scale_kilohertz_sine_reads_zero_dbfs_in_its_own_band() {
        let measured = third_octave_spectrum(
            &tone(1_000.0, 48_000, 48_000),
            1,
            48_000,
            SPECTRUM_SEGMENT_FRAMES,
            SPECTRUM_HOP_FRAMES,
            SPECTRUM_MINIMUM_FRAMES,
        )
        .expect("one second is four Welch segments");
        assert_eq!(measured.segments, 4);
        assert_eq!(measured.bands.len(), 31);
        assert_eq!(measured.bands[17].center_hertz_tenths, 10_000);
        let peak = level_db(&measured.bands, 17).expect("the 1 kHz band carries the tone");
        assert!(
            peak.abs() <= 0.05,
            "the 1 kHz band read {peak} dBFS, not 0.00"
        );
        for (index, name) in [(11_usize, "250 Hz"), (23, "4 kHz")] {
            let neighbour = level_db(&measured.bands, index).unwrap_or(f64::NEG_INFINITY);
            assert!(
                neighbour <= peak - 40.0,
                "the {name} band read {neighbour} dBFS, under 40 dB below the tone"
            );
        }
    }

    /// AU2 §7 item B13: the same at 80 Hz, so the low end is demonstrably not
    /// empty — the overlap-weighted band sum is what makes it read at all.
    #[test]
    fn an_eighty_hertz_tone_lands_in_the_eighty_hertz_band() {
        let measured = third_octave_spectrum(
            &tone(80.0, 48_000, 48_000),
            1,
            48_000,
            SPECTRUM_SEGMENT_FRAMES,
            SPECTRUM_HOP_FRAMES,
            SPECTRUM_MINIMUM_FRAMES,
        )
        .expect("one second is four Welch segments");
        assert_eq!(measured.bands[6].center_hertz_tenths, 800);
        assert!(
            !measured.bands[6].window_limited,
            "the 80 Hz band is 18.2 Hz wide, wider than the 11.72 Hz main lobe"
        );
        let peak = level_db(&measured.bands, 6).expect("the 80 Hz band carries the tone");
        assert!(
            peak.abs() <= 0.05,
            "the 80 Hz band read {peak} dBFS, not 0.00"
        );
        let loudest = (0..31)
            .max_by(|left, right| {
                level_db(&measured.bands, *left)
                    .unwrap_or(f64::NEG_INFINITY)
                    .total_cmp(&level_db(&measured.bands, *right).unwrap_or(f64::NEG_INFINITY))
            })
            .expect("the table is not empty");
        assert_eq!(loudest, 6, "the 80 Hz tone must peak in the 80 Hz band");
    }

    /// AU2 §7 item B13: exactly the 20, 25, 31.5, 40, and 50 Hz bands are
    /// narrower than the analysis window's main lobe.
    #[test]
    fn exactly_five_bands_are_window_limited() {
        let measured = third_octave_spectrum(
            &vec![0.0_f32; 24_576],
            1,
            48_000,
            SPECTRUM_SEGMENT_FRAMES,
            SPECTRUM_HOP_FRAMES,
            SPECTRUM_MINIMUM_FRAMES,
        )
        .expect("the minimum range is two segments");
        let flagged = measured
            .bands
            .iter()
            .enumerate()
            .filter(|(_, band)| band.window_limited)
            .map(|(index, band)| (index, band.center_hertz_tenths))
            .collect::<Vec<_>>();
        assert_eq!(
            flagged,
            vec![(0, 200), (1, 250), (2, 315), (3, 400), (4, 500)]
        );
    }

    /// AU2 §7 item B13/A23: Welch needs two segments, so 24 575 sample frames
    /// are refused and 24 576 are accepted. Silence reports no band at all.
    #[test]
    fn a_measurement_needs_two_whole_welch_segments() {
        assert!(
            third_octave_spectrum(
                &vec![0.0_f32; 24_575 * 2],
                2,
                48_000,
                SPECTRUM_SEGMENT_FRAMES,
                SPECTRUM_HOP_FRAMES,
                SPECTRUM_MINIMUM_FRAMES,
            )
            .is_none()
        );
        let measured = third_octave_spectrum(
            &vec![0.0_f32; 24_576 * 2],
            2,
            48_000,
            SPECTRUM_SEGMENT_FRAMES,
            SPECTRUM_HOP_FRAMES,
            SPECTRUM_MINIMUM_FRAMES,
        )
        .expect("24 576 sample frames are exactly two segments");
        assert_eq!(measured.segments, 2);
        assert!(
            measured
                .bands
                .iter()
                .all(|band| band.level_dbfs_hundredths.is_none()),
            "a silent stem reports no band level"
        );
    }

    /// AU2 §5.9: the ISO nominal centres, in order.
    #[test]
    fn the_band_table_is_thirty_one_iso_third_octave_centres() {
        assert_eq!(THIRD_OCTAVE_CENTERS_TENTHS.len(), 31);
        assert_eq!(THIRD_OCTAVE_CENTERS_TENTHS[0], 200);
        assert_eq!(THIRD_OCTAVE_CENTERS_TENTHS[17], 10_000);
        assert_eq!(THIRD_OCTAVE_CENTERS_TENTHS[30], 200_000);
        // The exact edges bracket each nominal centre.
        for (index, center) in THIRD_OCTAVE_CENTERS_TENTHS.iter().enumerate() {
            let (low, high) = band_edges(index);
            let nominal = f64::from(*center) / 10.0;
            assert!(
                low < nominal && nominal < high,
                "band {index} does not contain its nominal centre {nominal}"
            );
        }
        assert_eq!(SPECTRUM_MINIMUM_FRAMES, 24_576);
    }

    /// AU5 §3.1 rule 29: the **pre-AU5 butterfly loop**, verbatim, so the
    /// bit-identity claim is checked against the arithmetic it replaced rather
    /// than against a re-derivation of it.
    fn forward_fft_reference(data: &mut [Complex]) {
        let length = data.len();
        if length < 2 || !length.is_power_of_two() {
            return;
        }
        let mut target = 0_usize;
        for source in 1..length {
            let mut bit = length >> 1;
            while target & bit != 0 {
                target ^= bit;
                bit >>= 1;
            }
            target |= bit;
            if source < target {
                data.swap(source, target);
            }
        }
        let mut stage = 2_usize;
        while stage <= length {
            #[allow(clippy::cast_precision_loss)]
            let angle = -2.0 * std::f64::consts::PI / stage as f64;
            for block in (0..length).step_by(stage) {
                for offset in 0..stage / 2 {
                    #[allow(clippy::cast_precision_loss)]
                    let theta = angle * offset as f64;
                    let (sin, cos) = theta.sin_cos();
                    let even = data[block + offset];
                    let odd = data[block + offset + stage / 2];
                    let rotated = Complex::new(
                        cos.mul_add(odd.re, -(sin * odd.im)),
                        sin.mul_add(odd.re, cos * odd.im),
                    );
                    data[block + offset] = Complex::new(even.re + rotated.re, even.im + rotated.im);
                    data[block + offset + stage / 2] =
                        Complex::new(even.re - rotated.re, even.im - rotated.im);
                }
            }
            stage <<= 1;
        }
    }

    /// AU5 §7 A5 / §3.1 rule 29: the twiddle-table transform is
    /// **`to_bits()`-identical** to the pre-AU5 routine at 512, 4 096 and
    /// 16 384 points, which is what keeps AU2's spectrum goldens byte-unchanged.
    #[test]
    fn au5_the_twiddle_table_is_bit_identical_to_the_pre_au5_transform() {
        for length in [512_usize, 4_096, 16_384] {
            let input = pseudo_random(length);
            let mut tabled = input.clone();
            let mut reference = input;
            forward_fft(&mut tabled);
            forward_fft_reference(&mut reference);
            for (bin, (tabled, reference)) in tabled.iter().zip(&reference).enumerate() {
                assert_eq!(
                    (tabled.re.to_bits(), tabled.im.to_bits()),
                    (reference.re.to_bits(), reference.im.to_bits()),
                    "bin {bin} of a {length}-point transform is not bit-identical"
                );
            }
        }
    }

    /// AU5 §7 A5 / §3.1 rule 26: `inverse_fft(forward_fft(x))` returns `x`
    /// within 1e-12 at 512 / 4 096 / 16 384.
    #[test]
    fn au5_the_inverse_transform_round_trips() {
        for length in [512_usize, 4_096, 16_384] {
            let input = pseudo_random(length);
            let mut round_trip = input.clone();
            forward_fft(&mut round_trip);
            inverse_fft(&mut round_trip);
            let difference = input
                .iter()
                .zip(&round_trip)
                .map(|(input, round_trip)| {
                    (input.re - round_trip.re)
                        .abs()
                        .max((input.im - round_trip.im).abs())
                })
                .fold(0.0_f64, f64::max);
            assert!(
                difference <= 1.0e-12,
                "a {length}-point round trip differs by {difference}"
            );
        }
    }

    /// AU5 §3.1 rule 30: the preroll lane is **printed, not gated**.
    ///
    /// One second of 48 kHz stereo through a 512-point STFT at a 128-frame hop
    /// is 375 blocks per channel, and AU2 §3.7's seek preroll runs every
    /// prerolled frame through it — which is why a seek at minute 10 was
    /// measured at about 15 s of pure FFT before the table. There is **no timing
    /// assert**: a timing gate would be flaky or OS-conditioned, and N0 forbids
    /// OS-conditional tolerances, so the seek cliff is evidence in the log
    /// rather than a support ticket.
    #[test]
    fn au5_the_preroll_lane_prints_its_measurement() {
        let blocks = (48_000 / 128) * 2;
        let input = pseudo_random(512);
        // Warm the table so the measurement is steady-state, not first-touch.
        let mut warm = input.clone();
        forward_fft(&mut warm);

        let started = std::time::Instant::now();
        for _ in 0..blocks {
            let mut scratch = input.clone();
            forward_fft(&mut scratch);
            std::hint::black_box(&scratch);
        }
        let tabled = started.elapsed().as_secs_f64();

        let started = std::time::Instant::now();
        for _ in 0..blocks {
            let mut scratch = input.clone();
            forward_fft_reference(&mut scratch);
            std::hint::black_box(&scratch);
        }
        let untabled = started.elapsed().as_secs_f64();

        let speedup = untabled / tabled.max(f64::MIN_POSITIVE);
        let profile = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };
        println!(
            "AU5_PREROLL seconds=1.000 stft_milliseconds={:.3} ratio={:.5} speedup={speedup:.2} \
             profile={profile}",
            tabled * 1_000.0,
            tabled
        );
        assert!(tabled > 0.0 && untabled > 0.0);
    }

    /// AU5 §7 A4 / §0 R33: the profile's window is accepted where AU2's
    /// `SPECTRUM_MINIMUM_FRAMES` refuses it, and the 20th-percentile reduction
    /// reads a flat band.
    #[test]
    fn au5_the_profile_window_is_accepted_where_the_welch_window_is_refused() {
        let samples = vec![0.0_f32; 22_528 * 2];
        assert!(
            third_octave_spectrum(
                &samples,
                2,
                48_000,
                SPECTRUM_SEGMENT_FRAMES,
                SPECTRUM_HOP_FRAMES,
                SPECTRUM_MINIMUM_FRAMES,
            )
            .is_none(),
            "22 528 frames is under AU2's 24 576"
        );
        let measured = third_octave_band_percentile(
            &samples,
            2,
            48_000,
            NOISE_PROFILE_SEGMENT_FRAMES,
            NOISE_PROFILE_HOP_FRAMES,
            NOISE_PROFILE_MINIMUM_FRAMES,
            NOISE_PROFILE_PERCENT,
        )
        .expect("22 528 frames is exactly ten profile windows");
        assert_eq!(measured.windows, 10);
        assert_eq!(NOISE_PROFILE_MINIMUM_FRAMES, 22_528);
        assert!(
            measured.bands.iter().all(Option::is_none),
            "digital silence learns no band at all, which rule 43(i) writes as the neutral"
        );

        let noise = crate::test_support::pseudo_random_amplitude(22_528 * 2, 0.1);
        let spectrum = third_octave_spectrum(
            &noise,
            2,
            48_000,
            NOISE_PROFILE_SEGMENT_FRAMES,
            NOISE_PROFILE_HOP_FRAMES,
            NOISE_PROFILE_MINIMUM_FRAMES,
        )
        .expect("ten profile windows");
        let limited = spectrum
            .bands
            .iter()
            .filter(|band| band.window_limited)
            .count();
        assert_eq!(limited, 11, "eleven bands are narrower than a 46.9 Hz lobe");
        let learned = third_octave_band_percentile(
            &noise,
            2,
            48_000,
            NOISE_PROFILE_SEGMENT_FRAMES,
            NOISE_PROFILE_HOP_FRAMES,
            NOISE_PROFILE_MINIMUM_FRAMES,
            NOISE_PROFILE_PERCENT,
        )
        .expect("ten profile windows");
        assert!(
            learned.bands[..11].iter().all(Option::is_some),
            "the eleven window-limited bands are still reported as learned"
        );
    }
}
