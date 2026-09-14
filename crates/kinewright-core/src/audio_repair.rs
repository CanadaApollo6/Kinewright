//! AU5 §2.4: the repair-measurement report types and the pure exception rules.
//!
//! Media renders the stem and measures the percentile SNR and the clicks
//! (AU5 §3.9). The hum estimator lives here — a pure Goertzel, no FFT — so
//! the shoulder rule that stops a 60 Hz notch manufacturing a 50 Hz finding
//! (AU6 §13 / AU5 E7) is one implementation. Core then judges, under the same
//! module contract `audio_qc.rs` states: every threshold here is a `const`,
//! not a parameter, so two reports are comparable; every number is an integer
//! with its unit in the field name; and a report is evidence, never a gate —
//! it carries `evidence_only`, deliberately never `export_ready`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AudioQcException, MixSpectrumPoint, QaSeverity, TimeCode};

/// What to measure: a project range (omitted or `null` measures the whole
/// document) and the mix point to measure it at.
///
/// `deny_unknown_fields` so a misspelled argument is refused rather than
/// silently defaulted (AU5 §7 A15).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct AudioRepairRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub range: Option<std::ops::Range<TimeCode>>,
    /// AU5 §2.4 rule 25: the point type `mix_spectrum` already uses. AU5 adds
    /// no fourth spelling of "where in the graph".
    pub point: MixSpectrumPoint,
}

/// How an [`AudioRepairReport`] was computed, so the choices are auditable.
///
/// Follows [`AudioQcProvenance`](crate::AudioQcProvenance): fixed strings with
/// a manual `Default` (no derive) so the seven values are one literal each.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct AudioRepairProvenance {
    pub engine: String,
    pub measurement_rate: u32,
    /// The percentile pair the SNR is formed from, and the ranking rule.
    pub signal_to_noise: String,
    pub window: String,
    pub hum: String,
    pub clicks: String,
    pub exception_order: String,
}

/// The engine identity recorded in every [`AudioRepairProvenance`].
pub const AUDIO_REPAIR_ENGINE: &str = "kinewright_audio_repair_v1";

impl Default for AudioRepairProvenance {
    fn default() -> Self {
        Self {
            engine: AUDIO_REPAIR_ENGINE.to_owned(),
            measurement_rate: 48_000,
            signal_to_noise: "10th_and_90th_percentile_window_levels_nearest_rank".to_owned(),
            window: "10ms_rms_hop_equals_window_energetic_windows_only".to_owned(),
            hum: "goertzel_24000_frames_four_harmonics_sixth_octave_shoulders_excluding_notched_neighbours".to_owned(),
            clicks: "second_difference_detector_at_the_descriptor_neutral_threshold".to_owned(),
            exception_order: "severity_desc_code_asc_field_asc".to_owned(),
        }
    }
}

/// One repair measurement of a mix point over a project range (AU5 §2.4).
///
/// **The noise floor is a percentile, not a detected floor, and its bias has a
/// direction.** Over [`REPAIR_WINDOW_MILLISECONDS`] windows,
/// `noise_floor_dbfs_hundredths` is the 10th-percentile window level and
/// `signal_dbfs_hundredths` the 90th. Percentiles rather than a silence
/// detector because there is no threshold to tune and they degrade gracefully
/// on material with no silence in it. On material with real silence the
/// 10th/90th split is right; on **continuous speech the 10th percentile is
/// quiet speech**, so the floor reads high and the SNR reads low — the safe
/// direction, but a planner that did not know it would refuse a repair that
/// worked. Every place this number is published says "percentile" for that
/// reason.
///
/// Like `AudioQcReport`, it deliberately does not reuse the name
/// `export_ready`: `QaReport::export_ready` gates an export, and an evidence
/// report must never gate one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct AudioRepairReport {
    pub range: std::ops::Range<TimeCode>,
    pub point: MixSpectrumPoint,
    pub sample_rate: u32,
    pub sample_frames: u64,
    /// The RMS window the percentiles are taken over (AU5 §3.8).
    pub window_milliseconds: u32,
    /// How many of those windows carried energy above `SILENCE_POWER`; only
    /// those enter the percentile population.
    pub windows: u32,
    /// The 10th percentile — **a percentile, not a detected floor**. `None`
    /// below [`REPAIR_MINIMUM_WINDOWS`] energetic windows, where the report
    /// says so through a `low_window_count` finding rather than inventing a
    /// number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub noise_floor_dbfs_hundredths: Option<i32>,
    /// The 90th percentile. `None` on the same rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub signal_dbfs_hundredths: Option<i32>,
    /// `signal_dbfs_hundredths - noise_floor_dbfs_hundredths`. `None` on the
    /// same rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub snr_db_hundredths: Option<i32>,
    /// Excess over the shoulder reference, summed over four harmonics.
    /// Each harmonic prefers the sixth-octave pair at `f * 2^(±1/6)` and
    /// drops a side that sits within a sixth-octave of the other mains
    /// family's harmonics; when both sides are neighbours the reference is
    /// their max (AU6 §13 / AU5 E7). `None` under one whole Goertzel block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub hum_50_excess_db_hundredths: Option<i32>,
    /// The same at 60 Hz.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub hum_60_excess_db_hundredths: Option<i32>,
    /// The four per-harmonic 50 Hz excesses, low to high: four entries or none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(default)]
    pub hum_50_harmonic_excess_db_hundredths: Vec<i32>,
    /// The four per-harmonic 60 Hz excesses, low to high: four entries or none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(default)]
    pub hum_60_harmonic_excess_db_hundredths: Vec<i32>,
    pub click_count: u32,
    /// `click_count * 60 * sample_rate / sample_frames`, rounded down.
    pub click_density_per_minute: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(default)]
    pub findings: Vec<AudioQcException>,
    /// Always `true`.
    pub evidence_only: bool,
    pub provenance: AudioRepairProvenance,
}

/// AU5 §3.8: the short RMS window the percentiles are taken over, hop = window.
///
/// Deliberately **not** BS.1770 — no 400 ms gating block, no K-weighting.
pub const REPAIR_WINDOW_MILLISECONDS: u32 = 10;

/// AU5 §2.4 rule 22: the fewest energetic windows a percentile means anything
/// over. Below it all three percentile fields are `None` and the report raises
/// `low_window_count` rather than inventing a number.
pub const REPAIR_MINIMUM_WINDOWS: u32 = 10;

/// An SNR under 12 dB raises `low_signal_to_noise` at `Warning`.
pub const REPAIR_LOW_SNR_HUNDREDTHS: i32 = 1_200;

/// A summed hum excess over 6 dB raises `mains_hum_present` at `Warning`.
pub const REPAIR_HUM_EXCESS_HUNDREDTHS: i32 = 600;

/// More than 30 clicks a minute raises `click_density_high` at `Warning`.
pub const REPAIR_CLICK_DENSITY_PER_MINUTE: u32 = 30;

/// The measured numbers media hands core to build the exception list
/// (AU5 §2.4 rule 24, §3.9).
///
/// The **non-serde** struct, exactly as
/// [`AudioQcMeasurements`](crate::AudioQcMeasurements) is: it crosses one
/// crate boundary and is never a wire type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioRepairMeasurements {
    /// Windows that carried energy above `SILENCE_POWER`.
    pub windows: u32,
    /// The 10th-percentile window level; `None` below
    /// [`REPAIR_MINIMUM_WINDOWS`].
    pub noise_floor_dbfs_hundredths: Option<i32>,
    /// The 90th-percentile window level; `None` on the same rule.
    pub signal_dbfs_hundredths: Option<i32>,
    /// Their difference; `None` on the same rule.
    pub snr_db_hundredths: Option<i32>,
    /// The 50 Hz excess summed over four harmonics; `None` under one block.
    pub hum_50_excess_db_hundredths: Option<i32>,
    /// The 60 Hz excess summed over four harmonics; `None` under one block.
    pub hum_60_excess_db_hundredths: Option<i32>,
    pub click_count: u32,
    pub click_density_per_minute: u32,
}

/// Build the AU5 §2.4 exception table from measured numbers, sorted by
/// `(severity desc, code asc, field asc)` — `audio_qc_exceptions`' comparator,
/// shared rather than re-spelled.
///
/// Four thresholds and nothing else. A percentile that is `None` raises
/// nothing but `low_window_count`, because rule 22 makes "not enough windows"
/// and "a bad SNR" different answers; a hum excess that is `None` raises
/// nothing at all, because a range under one Goertzel block was never
/// measured.
#[must_use]
pub fn audio_repair_exceptions(measured: &AudioRepairMeasurements) -> Vec<AudioQcException> {
    let mut exceptions = Vec::new();

    if measured.windows < REPAIR_MINIMUM_WINDOWS {
        exceptions.push(AudioQcException {
            code: "low_window_count".to_owned(),
            severity: QaSeverity::Info,
            message: format!(
                "Only {windows} windows of {REPAIR_WINDOW_MILLISECONDS} ms carried energy, under the {REPAIR_MINIMUM_WINDOWS} windows a percentile needs, so the noise floor, signal and SNR are not reported.",
                windows = measured.windows,
            ),
            field: Some("windows".to_owned()),
            observed: Some(measured.windows.to_string()),
            allowed: Some(format!(">= {REPAIR_MINIMUM_WINDOWS}")),
        });
    }

    if let Some(snr) = measured.snr_db_hundredths
        && snr < REPAIR_LOW_SNR_HUNDREDTHS
    {
        exceptions.push(AudioQcException {
            code: "low_signal_to_noise".to_owned(),
            severity: QaSeverity::Warning,
            message: format!(
                "The 90th-percentile window level is {snr} hundredths of a dB over the 10th; the house floor is {REPAIR_LOW_SNR_HUNDREDTHS}. The floor is a percentile, so on continuous speech it reads high and this reads low."
            ),
            field: Some("snr_db_hundredths".to_owned()),
            observed: Some(snr.to_string()),
            allowed: Some(format!(">= {REPAIR_LOW_SNR_HUNDREDTHS}")),
        });
    }

    for (hertz, field, excess) in [
        (
            50,
            "hum_50_excess_db_hundredths",
            measured.hum_50_excess_db_hundredths,
        ),
        (
            60,
            "hum_60_excess_db_hundredths",
            measured.hum_60_excess_db_hundredths,
        ),
    ] {
        if let Some(excess) = excess
            && excess > REPAIR_HUM_EXCESS_HUNDREDTHS
        {
            exceptions.push(AudioQcException {
                code: "mains_hum_present".to_owned(),
                severity: QaSeverity::Warning,
                message: format!(
                    "The {hertz} Hz fundamental and its first three harmonics sit {excess} hundredths of a dB over their sixth-octave shoulders in total; the house band is {REPAIR_HUM_EXCESS_HUNDREDTHS}."
                ),
                field: Some(field.to_owned()),
                observed: Some(excess.to_string()),
                allowed: Some(format!("<= {REPAIR_HUM_EXCESS_HUNDREDTHS}")),
            });
        }
    }

    if measured.click_density_per_minute > REPAIR_CLICK_DENSITY_PER_MINUTE {
        exceptions.push(AudioQcException {
            code: "click_density_high".to_owned(),
            severity: QaSeverity::Warning,
            message: format!(
                "The range holds {count} flagged click(s), {density} a minute; the house band is {REPAIR_CLICK_DENSITY_PER_MINUTE}.",
                count = measured.click_count,
                density = measured.click_density_per_minute,
            ),
            field: Some("click_density_per_minute".to_owned()),
            observed: Some(measured.click_density_per_minute.to_string()),
            allowed: Some(format!("<= {REPAIR_CLICK_DENSITY_PER_MINUTE}")),
        });
    }

    crate::audio_qc::sort_exceptions(&mut exceptions);
    exceptions
}

/// AU5 §3.9 / R64: the Goertzel block, in sample frames.
///
/// R18's floor is "≥ 4 800". 24 000 frames (500 ms at 48 kHz) is what makes
/// the sixth-octave shoulders sit far enough from the centre that a real hum
/// can clear [`REPAIR_HUM_EXCESS_HUNDREDTHS`].
pub const HUM_GOERTZEL_BLOCK_FRAMES: usize = 24_000;

/// AU5 §3.9: how many harmonics of each mains frequency are summed.
pub const HUM_HARMONICS: u32 = 4;

/// Floor matching media's `SILENCE_POWER`: a zero Goertzel bin is written as
/// this power before the dB conversion.
const HUM_SILENCE_POWER: f64 = 1e-12;

/// A shoulder within this many octaves of the other family's harmonics is a
/// notched neighbour and is dropped from the reference.
const HUM_NEIGHBOUR_OCTAVES: f64 = 1.0 / 6.0;

/// The other IEC mains family: 50 Hz estimates ignore 60/120/180/240 Hz
/// neighbours, and 60 Hz estimates ignore 50/100/150/200 Hz.
fn other_mains_family(fundamental: f64) -> f64 {
    if (fundamental - 50.0).abs() <= (fundamental - 60.0).abs() {
        60.0
    } else {
        50.0
    }
}

fn near_other_family(hertz: f64, other: f64) -> bool {
    (1..=HUM_HARMONICS).any(|harmonic| {
        let notch = other * f64::from(harmonic);
        (hertz / notch).log2().abs() < HUM_NEIGHBOUR_OCTAVES
    })
}

fn pair_at_ratio(centre: f64, ratio: f64) -> [f64; 2] {
    [centre / ratio, centre * ratio]
}

fn sixth_octave_pair(fundamental: f64, harmonic: u32) -> [f64; 2] {
    pair_at_ratio(fundamental * f64::from(harmonic), 2.0_f64.powf(1.0 / 6.0))
}

/// The sixth-octave frequencies that enter one harmonic's reference.
///
/// A side within a sixth-octave of the other mains family is a notched
/// neighbour and is omitted. When *both* sides are neighbours (150 Hz and
/// 200 Hz against 60 Hz) both frequencies are still read and combined by
/// `max`, so a hole on either side cannot pull the reference down.
#[must_use]
pub fn hum_shoulder_hertz(fundamental: f64, harmonic: u32) -> Vec<f64> {
    let [low, high] = sixth_octave_pair(fundamental, harmonic);
    let other = other_mains_family(fundamental);
    match (
        near_other_family(low, other),
        near_other_family(high, other),
    ) {
        (false, false) | (true, true) => vec![low, high],
        (true, false) => vec![high],
        (false, true) => vec![low],
    }
}

fn combine_shoulders(low: f64, high: f64, low_near: bool, high_near: bool) -> f64 {
    match (low_near, high_near) {
        (false, false) => f64::midpoint(low, high),
        (true, false) => high,
        (false, true) => low,
        (true, true) => low.max(high),
    }
}

/// AU5 §3.9: the mean-square power at one frequency, by Goertzel over as many
/// whole [`HUM_GOERTZEL_BLOCK_FRAMES`] blocks as the range holds.
///
/// A sine of amplitude `A` exactly on the analysis frequency gives
/// `|X| = A*N/2`, so the mean-square power is `2|X|²/N²`.
#[allow(clippy::cast_precision_loss)]
fn goertzel_power(samples: &[f32], channels: usize, rate: u32, hertz: f64) -> Option<f64> {
    let channels = channels.max(1);
    let frames = samples.len() / channels;
    let block = HUM_GOERTZEL_BLOCK_FRAMES;
    if frames < block {
        return None;
    }
    let blocks = frames / block;
    let omega = 2.0 * std::f64::consts::PI * hertz / f64::from(rate);
    let coefficient = 2.0 * omega.cos();
    let mut total = 0.0_f64;
    for index in 0..blocks {
        for channel in 0..channels {
            let (mut s1, mut s2) = (0.0_f64, 0.0_f64);
            for frame in 0..block {
                let sample = f64::from(samples[(index * block + frame) * channels + channel]);
                let s0 = coefficient.mul_add(s1, sample) - s2;
                s2 = s1;
                s1 = s0;
            }
            let real = s1 - s2 * omega.cos();
            let imaginary = s2 * omega.sin();
            let magnitude_squared = real.mul_add(real, imaginary * imaginary);
            total += 2.0 * magnitude_squared / (block as f64 * block as f64);
        }
    }
    Some(total / (blocks * channels) as f64)
}

fn level_db(samples: &[f32], channels: usize, rate: u32, hertz: f64) -> Option<f64> {
    goertzel_power(samples, channels, rate, hertz)
        .map(|power| 10.0 * power.max(HUM_SILENCE_POWER).log10())
}

fn shoulder_reference_db(
    samples: &[f32],
    channels: usize,
    rate: u32,
    fundamental: f64,
    harmonic: u32,
    exclude_neighbours: bool,
) -> Option<f64> {
    let [low_hz, high_hz] = sixth_octave_pair(fundamental, harmonic);
    let low = level_db(samples, channels, rate, low_hz)?;
    let high = level_db(samples, channels, rate, high_hz)?;
    if !exclude_neighbours {
        return Some(f64::midpoint(low, high));
    }
    let other = other_mains_family(fundamental);
    Some(combine_shoulders(
        low,
        high,
        near_other_family(low_hz, other),
        near_other_family(high_hz, other),
    ))
}

fn harmonic_excess_db(
    samples: &[f32],
    channels: usize,
    rate: u32,
    fundamental: f64,
    harmonic: u32,
    exclude_neighbours: bool,
) -> Option<f64> {
    let centre = level_db(samples, channels, rate, fundamental * f64::from(harmonic))?;
    let reference = shoulder_reference_db(
        samples,
        channels,
        rate,
        fundamental,
        harmonic,
        exclude_neighbours,
    )?;
    Some(centre - reference)
}

/// AU5 §3.9: the summed excess over four harmonics, and the four values.
///
/// Sixth-octave sides that sit on the other family's notches are excluded
/// so a 60 Hz cascade does not manufacture a 50 Hz
/// [`mains_hum_present`](audio_repair_exceptions).
#[must_use]
pub fn hum_excess_hundredths(
    samples: &[f32],
    channels: usize,
    rate: u32,
    fundamental: f64,
) -> (Option<i32>, Vec<i32>) {
    hum_excess_hundredths_with(samples, channels, rate, fundamental, true)
}

#[allow(clippy::cast_possible_truncation)]
fn hum_excess_hundredths_with(
    samples: &[f32],
    channels: usize,
    rate: u32,
    fundamental: f64,
    exclude_neighbours: bool,
) -> (Option<i32>, Vec<i32>) {
    let mut per_harmonic = Vec::with_capacity(4);
    let mut summed = 0.0_f64;
    for harmonic in 1..=HUM_HARMONICS {
        let Some(excess) = harmonic_excess_db(
            samples,
            channels,
            rate,
            fundamental,
            harmonic,
            exclude_neighbours,
        ) else {
            return (None, Vec::new());
        };
        per_harmonic.push((excess * 100.0).round() as i32);
        summed += excess.max(0.0);
    }
    (Some((summed * 100.0).round() as i32), per_harmonic)
}

#[cfg(test)]
mod hum_shoulders {
    use super::*;

    #[test]
    fn fifty_hertz_drops_the_high_sixth_that_sits_on_sixty() {
        let shoulders = hum_shoulder_hertz(50.0, 1);
        assert_eq!(shoulders.len(), 1);
        let expected = 50.0 / 2.0_f64.powf(1.0 / 6.0);
        assert!((shoulders[0] - expected).abs() < 1e-9, "{shoulders:?}");
    }

    #[test]
    fn sixty_hertz_drops_the_low_sixth_that_sits_on_fifty() {
        let shoulders = hum_shoulder_hertz(60.0, 1);
        assert_eq!(shoulders.len(), 1);
        let expected = 60.0 * 2.0_f64.powf(1.0 / 6.0);
        assert!((shoulders[0] - expected).abs() < 1e-9, "{shoulders:?}");
    }

    #[test]
    fn one_hundred_keeps_only_the_low_sixth() {
        let shoulders = hum_shoulder_hertz(50.0, 2);
        assert_eq!(shoulders.len(), 1);
        let expected = 100.0 / 2.0_f64.powf(1.0 / 6.0);
        assert!((shoulders[0] - expected).abs() < 1e-9, "{shoulders:?}");
    }

    #[test]
    fn one_fifty_keeps_both_sixths_and_combines_them_by_max() {
        let shoulders = hum_shoulder_hertz(50.0, 3);
        assert_eq!(shoulders.len(), 2, "{shoulders:?}");
        let [low, high] = super::sixth_octave_pair(50.0, 3);
        assert!((shoulders[0] - low).abs() < 1e-9, "{shoulders:?}");
        assert!((shoulders[1] - high).abs() < 1e-9, "{shoulders:?}");
    }

    #[allow(clippy::cast_precision_loss)]
    fn tone(hertz: f64, amplitude: f32, rate: u32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|frame| {
                let phase = 2.0 * std::f64::consts::PI * hertz * frame as f64 / f64::from(rate);
                #[allow(clippy::cast_possible_truncation)]
                {
                    (phase.sin() as f32) * amplitude
                }
            })
            .collect()
    }

    #[allow(clippy::cast_precision_loss)]
    fn noise(frames: usize, amplitude: f32) -> Vec<f32> {
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        (0..frames)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                #[allow(clippy::cast_possible_truncation)]
                let unit = (state >> 40) as f32 / 8_388_608.0 - 1.0;
                unit * amplitude
            })
            .collect()
    }

    fn add(into: &mut [f32], extra: &[f32]) {
        for (sample, addend) in into.iter_mut().zip(extra) {
            *sample += *addend;
        }
    }

    /// A unity-gain RBJ notch at Q 12 — deeper than the product's −30 dB
    /// peaking cut, so a manufactured 50 Hz rise here is the stronger case.
    fn notch(samples: &[f32], rate: u32, hertz: f64) -> Vec<f32> {
        let w0 = 2.0 * std::f64::consts::PI * hertz / f64::from(rate);
        let alpha = w0.sin() / (2.0 * 12.0);
        let cos = w0.cos();
        let (b0, b1, b2) = (1.0_f64, -2.0 * cos, 1.0);
        let (a0, a1, a2) = (1.0 + alpha, -2.0 * cos, 1.0 - alpha);
        let mut out = vec![0.0; samples.len()];
        let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
        for (index, &sample) in samples.iter().enumerate() {
            let x = f64::from(sample);
            let y = (b0.mul_add(x, b1.mul_add(x1, b2 * x2)) - a1 * y1 - a2 * y2) / a0;
            #[allow(clippy::cast_possible_truncation)]
            {
                out[index] = y as f32;
            }
            x2 = x1;
            x1 = x;
            y2 = y1;
            y1 = y;
        }
        out
    }

    fn cascade_60(samples: &[f32], rate: u32) -> Vec<f32> {
        let mut out = samples.to_vec();
        for hertz in [60.0, 120.0, 180.0] {
            out = notch(&out, rate, hertz);
        }
        out
    }

    const RATE: u32 = 48_000;
    const FRAMES: usize = 48_000;

    fn sixty_family() -> Vec<f32> {
        let mut samples = noise(FRAMES, 0.010);
        add(&mut samples, &tone(60.0, 0.100, RATE, FRAMES));
        add(&mut samples, &tone(120.0, 0.050, RATE, FRAMES));
        add(&mut samples, &tone(180.0, 0.025, RATE, FRAMES));
        samples
    }

    fn both_families() -> Vec<f32> {
        let mut samples = sixty_family();
        add(&mut samples, &tone(50.0, 0.100, RATE, FRAMES));
        add(&mut samples, &tone(100.0, 0.050, RATE, FRAMES));
        add(&mut samples, &tone(150.0, 0.025, RATE, FRAMES));
        samples
    }

    #[test]
    fn a_fifty_hertz_tone_reads_large_and_sixty_stays_quiet() {
        let mut samples = noise(FRAMES, 0.010);
        add(&mut samples, &tone(50.0, 0.100, RATE, FRAMES));
        let (hum_50, harmonics_50) = hum_excess_hundredths(&samples, 1, RATE, 50.0);
        let (hum_60, _) = hum_excess_hundredths(&samples, 1, RATE, 60.0);
        let hum_50 = hum_50.expect("one whole block");
        let hum_60 = hum_60.expect("one whole block");
        assert_eq!(harmonics_50.len(), 4);
        assert!(hum_50 > 1_500, "hum_50={hum_50}");
        assert!(hum_60 < 300, "hum_60={hum_60}");
    }

    /// Notch the 50 Hz family's high sixth-octave sides — the E7 mechanism —
    /// without a 50 Hz tone. The old mean-of-both reference then rises;
    /// excluding those neighbours must not.
    fn dipped_fifty_high_shoulders() -> Vec<f32> {
        let mut samples = noise(FRAMES, 0.010);
        for harmonic in 1..=3_u32 {
            let [_low, high] = super::sixth_octave_pair(50.0, harmonic);
            samples = notch(&samples, RATE, high);
        }
        samples
    }

    #[test]
    fn a_sixty_cascade_does_not_manufacture_a_fifty_hertz_finding() {
        let dipped = dipped_fifty_high_shoulders();
        let (old_50, _) = hum_excess_hundredths_with(&dipped, 1, RATE, 50.0, false);
        let (new_50, _) = hum_excess_hundredths(&dipped, 1, RATE, 50.0);
        let old_50 = old_50.expect("one whole block");
        let new_50 = new_50.expect("one whole block");
        assert!(
            old_50 > REPAIR_HUM_EXCESS_HUNDREDTHS,
            "dipping the 50 Hz high shoulders must manufacture the old rise, old_50={old_50}"
        );
        assert!(
            new_50 <= REPAIR_HUM_EXCESS_HUNDREDTHS,
            "excluding notched neighbours must keep 50 Hz under the house band, new_50={new_50}"
        );

        let before = sixty_family();
        let after = cascade_60(&before, RATE);
        let (new_60_before, harmonics_before) = hum_excess_hundredths(&before, 1, RATE, 60.0);
        let (new_60_after, harmonics_after) = hum_excess_hundredths(&after, 1, RATE, 60.0);
        let drop = new_60_before.expect("60 before") - new_60_after.expect("60 after");
        assert!(
            drop >= 2_400,
            "the 60 Hz drop must still clear AU6's floor, drop={drop}"
        );
        for index in 0..3 {
            let harmonic_drop = harmonics_before[index] - harmonics_after[index];
            assert!(
                harmonic_drop >= 500,
                "harmonic {index} drop={harmonic_drop}"
            );
        }
        let measured = AudioRepairMeasurements {
            windows: 600,
            noise_floor_dbfs_hundredths: Some(-6_000),
            signal_dbfs_hundredths: Some(-1_800),
            snr_db_hundredths: Some(4_200),
            hum_50_excess_db_hundredths: Some(new_50),
            hum_60_excess_db_hundredths: new_60_after,
            click_count: 0,
            click_density_per_minute: 0,
        };
        assert!(
            !audio_repair_exceptions(&measured)
                .iter()
                .any(|finding| finding.code == "mains_hum_present"
                    && finding.field.as_deref() == Some("hum_50_excess_db_hundredths")),
            "{:?}",
            audio_repair_exceptions(&measured)
        );
    }

    #[test]
    fn both_families_keep_a_real_fifty_hertz_finding_after_a_sixty_cascade() {
        let before = both_families();
        let after = cascade_60(&before, RATE);
        let (hum_50, _) = hum_excess_hundredths(&after, 1, RATE, 50.0);
        let (hum_60_before, _) = hum_excess_hundredths(&before, 1, RATE, 60.0);
        let (hum_60_after, _) = hum_excess_hundredths(&after, 1, RATE, 60.0);
        let hum_50 = hum_50.expect("one whole block");
        assert!(
            hum_50 > REPAIR_HUM_EXCESS_HUNDREDTHS,
            "a real 50 Hz family must survive a 60 Hz cascade, hum_50={hum_50}"
        );
        let drop = hum_60_before.expect("60 before") - hum_60_after.expect("60 after");
        assert!(drop >= 2_400, "60 Hz drop={drop}");
        let measured = AudioRepairMeasurements {
            windows: 600,
            noise_floor_dbfs_hundredths: Some(-6_000),
            signal_dbfs_hundredths: Some(-1_800),
            snr_db_hundredths: Some(4_200),
            hum_50_excess_db_hundredths: Some(hum_50),
            hum_60_excess_db_hundredths: hum_60_after,
            click_count: 0,
            click_density_per_minute: 0,
        };
        assert!(
            audio_repair_exceptions(&measured).iter().any(|finding| {
                finding.code == "mains_hum_present"
                    && finding.field.as_deref() == Some("hum_50_excess_db_hundredths")
            }),
            "{:?}",
            audio_repair_exceptions(&measured)
        );
    }
}
