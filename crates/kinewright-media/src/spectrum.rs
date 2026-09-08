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

use kinewright_core::SpectrumBand;

/// AU2 §5.9: the Welch segment length in sample frames.
pub(crate) const SPECTRUM_SEGMENT_FRAMES: usize = 16_384;

/// AU2 §5.9: the hop between segments, 50 % overlap.
pub(crate) const SPECTRUM_HOP_FRAMES: usize = SPECTRUM_SEGMENT_FRAMES / 2;

/// AU2 §5.9/A23: Welch needs two segments, so a measurement needs
/// `2 * 16 384 - 8 192` sample frames — 512 ms at 48 kHz. A shorter range is
/// rejected rather than returning a degenerate spectrum.
pub(crate) const SPECTRUM_MINIMUM_FRAMES: u64 =
    (2 * SPECTRUM_SEGMENT_FRAMES - SPECTRUM_HOP_FRAMES) as u64;

/// AU2 §5.9: the 31 ISO nominal third-octave centres, in tenths of a hertz,
/// low to high — nominally 20 Hz through 20 kHz.
pub(crate) const THIRD_OCTAVE_CENTERS_TENTHS: [u32; 31] = [
    200, 250, 315, 400, 500, 630, 800, 1_000, 1_250, 1_600, 2_000, 2_500, 3_150, 4_000, 5_000,
    6_300, 8_000, 10_000, 12_500, 16_000, 20_000, 25_000, 31_500, 40_000, 50_000, 63_000, 80_000,
    100_000, 125_000, 160_000, 200_000,
];

/// AU2 §5.9: a band power below this reads as silence and reports `None`.
const SILENCE_POWER: f64 = 1e-12;

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

/// AU2 §5.9: the unnormalised forward DFT, in place.
///
/// `data.len()` must be a power of two; anything else is left untouched, which
/// only a caller bug can produce.
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
        // The stage length is a power of two well inside `f64`'s exact range.
        #[allow(clippy::cast_precision_loss)]
        let angle = -2.0 * std::f64::consts::PI / stage as f64;
        for block in (0..length).step_by(stage) {
            for offset in 0..stage / 2 {
                // Likewise an exact small-integer conversion.
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

/// AU2 §5.9: one Welch measurement of one interleaved stem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpectrumMeasurement {
    /// How many Welch segments the average covers.
    pub(crate) segments: u32,
    /// The 31 ISO third-octave bands, low to high.
    pub(crate) bands: Vec<SpectrumBand>,
}

/// AU2 §5.9: the exact third-octave edges around `f_c = 1000 * 2^(k/3)` for `k`
/// in `-17..=13`: `[f_c * 2^(-1/6), f_c * 2^(1/6))`.
fn band_edges(index: usize) -> (f64, f64) {
    // The band index is a small integer; the conversion is exact.
    #[allow(clippy::cast_precision_loss)]
    let exact_center = 1000.0 * 2.0_f64.powf((index as f64 - 17.0) / 3.0);
    let ratio = 2.0_f64.powf(1.0 / 6.0);
    (exact_center / ratio, exact_center * ratio)
}

/// AU2 §5.9: the third-octave spectrum of one interleaved stem.
///
/// 16 384-point Hann window, hop 8 192, trailing partial segment discarded,
/// power averaged across every segment and every channel. Returns `None` when
/// the stem holds fewer than [`SPECTRUM_MINIMUM_FRAMES`] sample frames — Welch
/// needs two segments — which the caller rejects with
/// [`kinewright_core::MediaError::MixSpectrumRangeTooShort`] before it ever
/// gets here.
pub(crate) fn third_octave_spectrum(
    samples: &[f32],
    channels: usize,
    sample_rate: u32,
) -> Option<SpectrumMeasurement> {
    let channels = channels.max(1);
    let frames = samples.len() / channels;
    if u64::try_from(frames).unwrap_or(0) < SPECTRUM_MINIMUM_FRAMES {
        return None;
    }
    let segment_count = (frames - SPECTRUM_SEGMENT_FRAMES) / SPECTRUM_HOP_FRAMES + 1;
    // The Hann window and the `sum(w^2)` its normalisation needs.
    let window = hann_window(SPECTRUM_SEGMENT_FRAMES);
    let window_power: f64 = window.iter().map(|weight| weight * weight).sum();
    // The transform is unnormalised, so the estimate divides by `N * sum(w^2)`.
    #[allow(clippy::cast_precision_loss)]
    let normalisation = SPECTRUM_SEGMENT_FRAMES as f64 * window_power;

    let mut power = vec![0.0_f64; SPECTRUM_SEGMENT_FRAMES / 2 + 1];
    let mut scratch = vec![Complex::default(); SPECTRUM_SEGMENT_FRAMES];
    for segment in 0..segment_count {
        let start = segment * SPECTRUM_HOP_FRAMES;
        for channel in 0..channels {
            for (index, slot) in scratch.iter_mut().enumerate() {
                let sample = samples[(start + index) * channels + channel];
                *slot = Complex::new(f64::from(sample) * window[index], 0.0);
            }
            forward_fft(&mut scratch);
            for (bin, accumulated) in power.iter_mut().enumerate() {
                let magnitude = scratch[bin].magnitude_squared();
                let one_sided = if bin == 0 || bin == SPECTRUM_SEGMENT_FRAMES / 2 {
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

    let bin_width = f64::from(sample_rate) / normalised_length();
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
                // AU2 §5.9: true for the five bands narrower than the analysis
                // window's main lobe — 20, 25, 31.5, 40, and 50 Hz at 48 kHz.
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

/// The segment length as an `f64`; a power of two, so the conversion is exact.
#[allow(clippy::cast_precision_loss)]
fn normalised_length() -> f64 {
    SPECTRUM_SEGMENT_FRAMES as f64
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
fn hann_window(length: usize) -> Vec<f64> {
    // The window length is a power of two well inside `f64`'s exact range.
    #[allow(clippy::cast_precision_loss)]
    let denominator = length as f64;
    (0..length)
        .map(|index| {
            // Likewise exact.
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
        let measured = third_octave_spectrum(&tone(1_000.0, 48_000, 48_000), 1, 48_000)
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
        let measured = third_octave_spectrum(&tone(80.0, 48_000, 48_000), 1, 48_000)
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
        let measured = third_octave_spectrum(&vec![0.0_f32; 24_576], 1, 48_000)
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
        assert!(third_octave_spectrum(&vec![0.0_f32; 24_575 * 2], 2, 48_000).is_none());
        let measured = third_octave_spectrum(&vec![0.0_f32; 24_576 * 2], 2, 48_000)
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
}
