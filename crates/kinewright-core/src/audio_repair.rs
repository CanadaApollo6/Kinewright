//! AU5 §2.4: the repair-measurement report types and the pure exception rules.
//!
//! Media measures (AU5 §3.9) and core judges, under the same module contract
//! `audio_qc.rs` states: every threshold here is a `const`, not a parameter,
//! so two reports are comparable; every number is an integer with its unit in
//! the field name; and a report is evidence, never a gate — it carries
//! `evidence_only`, deliberately never `export_ready`.

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
            hum: "goertzel_4800_frames_four_harmonics_sixth_octave_shoulders".to_owned(),
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
    /// Excess over the sixth-octave shoulders at `f * 2^(±1/6)`, summed over
    /// four harmonics. `None` under one whole Goertzel block.
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
