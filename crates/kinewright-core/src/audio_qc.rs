//! AD0 — the audio delivery contract and its pure QC engine.
//!
//! Colour delivery (CC6) reads the written file back and reports typed,
//! integer-valued exceptions against named budgets. AD0 gives audio the same
//! shape: a typed [`AudioDeliveryTarget`] chosen per job, a typed
//! [`AudioDeliveryMeasurement`] taken from the *decoded* file, and one pure
//! function, [`measure_audio_qc`], that compares them and reports. Nothing here
//! decodes, normalizes, or changes a document: the engine is evidence only,
//! and the numbers it reports are the numbers a person or an agent decides on.
//!
//! Units follow `AudioLoudness`: LUFS, LU, dBTP, and dB are carried as signed
//! hundredths so every value is an integer on the wire.

use serde::{Deserialize, Serialize};

use crate::{AudioLoudness, QaSeverity};

/// The only analysis rate the measurement path accepts (BS.1770 K-weighting
/// coefficients are fixed for 48 kHz).
pub const AUDIO_DELIVERY_ANALYSIS_SAMPLE_RATE: u32 = 48_000;

/// The oversampling factor the true-peak measurement uses (BS.1770-4 Annex 2).
pub const TRUE_PEAK_OVERSAMPLING: u32 = 4;

/// Named delivery presets. Each is a [`AudioDeliveryTarget`] whose numbers are
/// stated once, in [`AudioDeliveryPreset::target`], and nowhere else.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AudioDeliveryPreset {
    /// Measure and report everything; gate nothing. The default for a
    /// mezzanine (`source_master`) export.
    #[default]
    MeasureOnly,
    /// Platform loudness normalization (YouTube, Spotify, Apple Music, TikTok):
    /// −14 LUFS ± 1 LU, −1 dBTP.
    Streaming,
    /// Spoken-word delivery (Apple Podcasts, Spotify podcasts):
    /// −16 LUFS ± 1 LU, −1 dBTP.
    Podcast,
    /// EBU R 128: −23 LUFS ± 1 LU, −1 dBTP, loudness range advised ≤ 20 LU.
    BroadcastEbuR128,
    /// ATSC A/85: −24 LKFS ± 2 LU, −2 dBTP.
    BroadcastAtscA85,
}

impl AudioDeliveryPreset {
    pub const ALL: [Self; 5] = [
        Self::MeasureOnly,
        Self::Streaming,
        Self::Podcast,
        Self::BroadcastEbuR128,
        Self::BroadcastAtscA85,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MeasureOnly => "measure_only",
            Self::Streaming => "streaming",
            Self::Podcast => "podcast",
            Self::BroadcastEbuR128 => "broadcast_ebu_r128",
            Self::BroadcastAtscA85 => "broadcast_atsc_a85",
        }
    }

    /// A short human label for a radio button or a report line.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::MeasureOnly => "Measure only",
            Self::Streaming => "Streaming −14 LUFS",
            Self::Podcast => "Podcast −16 LUFS",
            Self::BroadcastEbuR128 => "Broadcast EBU R 128 −23 LUFS",
            Self::BroadcastAtscA85 => "Broadcast ATSC A/85 −24 LKFS",
        }
    }

    /// The numbers behind the preset. This is the single authority.
    #[must_use]
    pub const fn target(self) -> AudioDeliveryTarget {
        match self {
            Self::MeasureOnly => AudioDeliveryTarget {
                preset: self,
                integrated_lufs_hundredths: None,
                tolerance_lu_hundredths: 0,
                maximum_true_peak_dbtp_hundredths: None,
                maximum_loudness_range_lu_hundredths: None,
            },
            Self::Streaming => AudioDeliveryTarget {
                preset: self,
                integrated_lufs_hundredths: Some(-1_400),
                tolerance_lu_hundredths: 100,
                maximum_true_peak_dbtp_hundredths: Some(-100),
                maximum_loudness_range_lu_hundredths: None,
            },
            Self::Podcast => AudioDeliveryTarget {
                preset: self,
                integrated_lufs_hundredths: Some(-1_600),
                tolerance_lu_hundredths: 100,
                maximum_true_peak_dbtp_hundredths: Some(-100),
                maximum_loudness_range_lu_hundredths: None,
            },
            Self::BroadcastEbuR128 => AudioDeliveryTarget {
                preset: self,
                integrated_lufs_hundredths: Some(-2_300),
                tolerance_lu_hundredths: 100,
                maximum_true_peak_dbtp_hundredths: Some(-100),
                maximum_loudness_range_lu_hundredths: Some(2_000),
            },
            Self::BroadcastAtscA85 => AudioDeliveryTarget {
                preset: self,
                integrated_lufs_hundredths: Some(-2_400),
                tolerance_lu_hundredths: 200,
                maximum_true_peak_dbtp_hundredths: Some(-200),
                maximum_loudness_range_lu_hundredths: None,
            },
        }
    }
}

/// What one delivery is expected to measure. A job parameter, never a
/// document edit (the same rule CC6 gives the delivery bit depth).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct AudioDeliveryTarget {
    /// The preset these numbers came from, for the report line. A custom
    /// target carries the preset it was derived from.
    #[serde(default)]
    #[schemars(default)]
    pub preset: AudioDeliveryPreset,
    /// Integrated (programme) loudness target. `None` gates nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub integrated_lufs_hundredths: Option<i32>,
    /// Symmetric tolerance around the integrated target, in hundredths of an LU.
    #[serde(default)]
    #[schemars(default)]
    pub tolerance_lu_hundredths: u16,
    /// True-peak ceiling. `None` gates nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub maximum_true_peak_dbtp_hundredths: Option<i32>,
    /// Loudness-range advisory ceiling (EBU Tech 3342). Always `Warning`
    /// severity: a wide range is a creative property until a broadcaster says
    /// otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub maximum_loudness_range_lu_hundredths: Option<i32>,
}

impl Default for AudioDeliveryTarget {
    fn default() -> Self {
        AudioDeliveryPreset::MeasureOnly.target()
    }
}

impl AudioDeliveryTarget {
    /// `true` when at least one of the gated quantities has a bound.
    #[must_use]
    pub const fn gates_anything(&self) -> bool {
        self.integrated_lufs_hundredths.is_some()
            || self.maximum_true_peak_dbtp_hundredths.is_some()
            || self.maximum_loudness_range_lu_hundredths.is_some()
    }
}

/// The decoded-file measurement AD0 adds on top of [`AudioLoudness`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct AudioDeliveryMeasurement {
    /// BS.1770 integrated loudness and the raw sample peak.
    pub loudness: AudioLoudness,
    /// BS.1770-4 true peak after [`TRUE_PEAK_OVERSAMPLING`]× oversampling.
    /// `None` means the decoded signal was silent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub true_peak_dbtp_hundredths: Option<i32>,
    /// EBU Tech 3342 loudness range (95th − 10th percentile of gated 3 s
    /// short-term loudness). `None` when fewer than two short-term windows
    /// survived the gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub loudness_range_lu_hundredths: Option<i32>,
}

/// One reportable audio QC finding. The same shape as `ColorQcException`
/// without the clip/effect attribution, which audio delivery does not have yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AudioQcException {
    pub code: String,
    pub severity: QaSeverity,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub field: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub observed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub allowed: Option<String>,
}

/// The typed codes [`measure_audio_qc`] can publish. Listed so a caller can
/// match on them rather than on prose.
pub const AUDIO_QC_CODES: [&str; 7] = [
    "audio_programme_silent",
    "audio_integrated_loudness_below_target",
    "audio_integrated_loudness_above_target",
    "audio_true_peak_over_ceiling",
    "audio_true_peak_unmeasured",
    "audio_loudness_range_over_limit",
    "audio_analysis_rate_unexpected",
];

/// One measured delivery against one target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AudioQcReport {
    pub target: AudioDeliveryTarget,
    pub measured: AudioDeliveryMeasurement,
    /// The constant gain, in hundredths of a dB, that would place the measured
    /// integrated loudness exactly on the target. `None` when the target has
    /// no integrated bound or the programme is silent. Evidence for a
    /// normalization proposal, never applied here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub gain_to_target_db_hundredths: Option<i32>,
    /// `true` when applying `gain_to_target_db_hundredths` would push the
    /// measured true peak above the ceiling, i.e. gain alone cannot conform
    /// this programme and a limiter or a mix change is needed.
    #[serde(default)]
    #[schemars(default)]
    pub gain_would_exceed_peak_ceiling: bool,
    pub exceptions: Vec<AudioQcException>,
    /// No `Error`-severity entry in `exceptions`.
    pub technical_pass: bool,
}

/// Render signed hundredths as a decimal with two places, without a float.
#[must_use]
pub fn hundredths_to_string(hundredths: i32) -> String {
    let sign = if hundredths < 0 { "-" } else { "" };
    let magnitude = hundredths.unsigned_abs();
    format!("{sign}{}.{:02}", magnitude / 100, magnitude % 100)
}

fn exception(
    code: &str,
    severity: QaSeverity,
    message: String,
    field: &str,
    observed: Option<String>,
    allowed: Option<String>,
) -> AudioQcException {
    AudioQcException {
        code: code.to_owned(),
        severity,
        message,
        field: Some(field.to_owned()),
        observed,
        allowed,
    }
}

/// Compare one decoded-file measurement against one target.
///
/// Pure and total: every input produces a report, and a report never carries
/// a value the measurement did not.
#[must_use]
pub fn measure_audio_qc(
    target: AudioDeliveryTarget,
    measured: AudioDeliveryMeasurement,
) -> AudioQcReport {
    let mut exceptions = Vec::new();
    let loudness = measured.loudness;

    if loudness.sample_rate != AUDIO_DELIVERY_ANALYSIS_SAMPLE_RATE {
        exceptions.push(exception(
            "audio_analysis_rate_unexpected",
            QaSeverity::Warning,
            format!(
                "the loudness measurement was taken at {} Hz; the BS.1770 weighting is defined here for {} Hz",
                loudness.sample_rate, AUDIO_DELIVERY_ANALYSIS_SAMPLE_RATE
            ),
            "sample_rate",
            Some(loudness.sample_rate.to_string()),
            Some(AUDIO_DELIVERY_ANALYSIS_SAMPLE_RATE.to_string()),
        ));
    }

    let mut gain_to_target_db_hundredths = None;
    let mut gain_would_exceed_peak_ceiling = false;

    match (target.integrated_lufs_hundredths, loudness.integrated_lufs_hundredths) {
        (Some(wanted), None) => exceptions.push(exception(
            "audio_programme_silent",
            QaSeverity::Error,
            "the decoded programme is silent (no block passed the −70 LUFS absolute gate), so it cannot meet an integrated loudness target".to_owned(),
            "integrated_lufs_hundredths",
            None,
            Some(format!("{} LUFS", hundredths_to_string(wanted))),
        )),
        (None, None) => exceptions.push(exception(
            "audio_programme_silent",
            QaSeverity::Info,
            "the decoded programme is silent; no loudness target is set".to_owned(),
            "integrated_lufs_hundredths",
            None,
            None,
        )),
        (Some(wanted), Some(got)) => {
            let tolerance = i32::from(target.tolerance_lu_hundredths);
            let low = wanted.saturating_sub(tolerance);
            let high = wanted.saturating_add(tolerance);
            let allowed = format!(
                "{}..={} LUFS",
                hundredths_to_string(low),
                hundredths_to_string(high)
            );
            let gain = wanted.saturating_sub(got);
            gain_to_target_db_hundredths = Some(gain);
            if got < low {
                exceptions.push(exception(
                    "audio_integrated_loudness_below_target",
                    QaSeverity::Error,
                    format!(
                        "integrated loudness {} LUFS is {} LU below the tolerance band; {} dB of gain would reach the target",
                        hundredths_to_string(got),
                        hundredths_to_string(low.saturating_sub(got)),
                        hundredths_to_string(gain)
                    ),
                    "integrated_lufs_hundredths",
                    Some(format!("{} LUFS", hundredths_to_string(got))),
                    Some(allowed),
                ));
            } else if got > high {
                exceptions.push(exception(
                    "audio_integrated_loudness_above_target",
                    QaSeverity::Error,
                    format!(
                        "integrated loudness {} LUFS is {} LU above the tolerance band; {} dB of gain would reach the target",
                        hundredths_to_string(got),
                        hundredths_to_string(got.saturating_sub(high)),
                        hundredths_to_string(gain)
                    ),
                    "integrated_lufs_hundredths",
                    Some(format!("{} LUFS", hundredths_to_string(got))),
                    Some(allowed),
                ));
            }
            if let (Some(ceiling), Some(true_peak)) = (
                target.maximum_true_peak_dbtp_hundredths,
                measured.true_peak_dbtp_hundredths,
            ) {
                gain_would_exceed_peak_ceiling = true_peak.saturating_add(gain) > ceiling;
            }
        }
        (None, Some(_)) => {}
    }

    match (
        target.maximum_true_peak_dbtp_hundredths,
        measured.true_peak_dbtp_hundredths,
    ) {
        (Some(ceiling), Some(true_peak)) if true_peak > ceiling => {
            exceptions.push(exception(
                "audio_true_peak_over_ceiling",
                QaSeverity::Error,
                format!(
                    "true peak {} dBTP exceeds the {} dBTP ceiling by {} dB",
                    hundredths_to_string(true_peak),
                    hundredths_to_string(ceiling),
                    hundredths_to_string(true_peak.saturating_sub(ceiling))
                ),
                "true_peak_dbtp_hundredths",
                Some(format!("{} dBTP", hundredths_to_string(true_peak))),
                Some(format!("<= {} dBTP", hundredths_to_string(ceiling))),
            ));
        }
        (Some(ceiling), None) if loudness.integrated_lufs_hundredths.is_some() => {
            exceptions.push(exception(
                "audio_true_peak_unmeasured",
                QaSeverity::Warning,
                "a true-peak ceiling is set but the measurement carries no true peak".to_owned(),
                "true_peak_dbtp_hundredths",
                None,
                Some(format!("<= {} dBTP", hundredths_to_string(ceiling))),
            ));
        }
        _ => {}
    }

    if let (Some(limit), Some(range)) = (
        target.maximum_loudness_range_lu_hundredths,
        measured.loudness_range_lu_hundredths,
    ) && range > limit
    {
        exceptions.push(exception(
            "audio_loudness_range_over_limit",
            QaSeverity::Warning,
            format!(
                "loudness range {} LU exceeds the advised {} LU",
                hundredths_to_string(range),
                hundredths_to_string(limit)
            ),
            "loudness_range_lu_hundredths",
            Some(format!("{} LU", hundredths_to_string(range))),
            Some(format!("<= {} LU", hundredths_to_string(limit))),
        ));
    }

    let technical_pass = !exceptions
        .iter()
        .any(|exception| exception.severity == QaSeverity::Error);

    AudioQcReport {
        target,
        measured,
        gain_to_target_db_hundredths,
        gain_would_exceed_peak_ceiling,
        exceptions,
        technical_pass,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measurement(
        integrated: Option<i32>,
        true_peak: Option<i32>,
        range: Option<i32>,
    ) -> AudioDeliveryMeasurement {
        AudioDeliveryMeasurement {
            loudness: AudioLoudness {
                integrated_lufs_hundredths: integrated,
                sample_peak_dbfs_hundredths: true_peak.map(|peak| peak - 30),
                sample_rate: 48_000,
                channels: 2,
                sample_frames: 480_000,
            },
            true_peak_dbtp_hundredths: true_peak,
            loudness_range_lu_hundredths: range,
        }
    }

    fn codes(report: &AudioQcReport) -> Vec<&str> {
        report
            .exceptions
            .iter()
            .map(|exception| exception.code.as_str())
            .collect()
    }

    #[test]
    fn every_preset_is_its_own_authority_and_round_trips() {
        for preset in AudioDeliveryPreset::ALL {
            let target = preset.target();
            assert_eq!(target.preset, preset);
            let json = serde_json::to_string(&target).unwrap();
            let back: AudioDeliveryTarget = serde_json::from_str(&json).unwrap();
            assert_eq!(back, target);
            assert_eq!(
                preset == AudioDeliveryPreset::MeasureOnly,
                !target.gates_anything()
            );
        }
        assert_eq!(AudioDeliveryTarget::default().preset, AudioDeliveryPreset::MeasureOnly);
    }

    #[test]
    fn measure_only_reports_and_gates_nothing() {
        let report = measure_audio_qc(
            AudioDeliveryPreset::MeasureOnly.target(),
            measurement(Some(-3_990), Some(-172), Some(1_200)),
        );
        assert!(report.technical_pass);
        assert!(report.exceptions.is_empty());
        assert_eq!(report.gain_to_target_db_hundredths, None);
    }

    #[test]
    fn the_m40_event_cut_fails_streaming_by_the_measured_amount() {
        // The event/multicam baseline shipped at −39.9 LUFS (M40 §event).
        let report = measure_audio_qc(
            AudioDeliveryPreset::Streaming.target(),
            measurement(Some(-3_990), Some(-2_500), None),
        );
        assert!(!report.technical_pass);
        assert_eq!(codes(&report), ["audio_integrated_loudness_below_target"]);
        assert_eq!(report.gain_to_target_db_hundredths, Some(2_590));
        // −25.00 + 25.90 = +0.90 dBTP > −1.00: gain alone would clip the ceiling.
        assert!(report.gain_would_exceed_peak_ceiling);
        let exception = &report.exceptions[0];
        assert_eq!(exception.observed.as_deref(), Some("-39.90 LUFS"));
        assert_eq!(exception.allowed.as_deref(), Some("-15.00..=-13.00 LUFS"));
        assert_eq!(exception.severity, QaSeverity::Error);
    }

    #[test]
    fn inside_the_tolerance_band_passes_at_both_edges() {
        for got in [-1_500, -1_400, -1_300] {
            // A true peak 3 dB under the ceiling leaves room for the ±1 LU of
            // gain the band edges imply.
            let report = measure_audio_qc(
                AudioDeliveryPreset::Streaming.target(),
                measurement(Some(got), Some(-300), None),
            );
            assert!(report.technical_pass, "{got}");
            assert!(report.exceptions.is_empty(), "{got}");
            assert_eq!(report.gain_to_target_db_hundredths, Some(-1_400 - got));
            assert!(!report.gain_would_exceed_peak_ceiling);
        }
        let report = measure_audio_qc(
            AudioDeliveryPreset::Streaming.target(),
            measurement(Some(-1_299), Some(-100), None),
        );
        assert_eq!(codes(&report), ["audio_integrated_loudness_above_target"]);
    }

    #[test]
    fn true_peak_over_the_ceiling_is_an_error_even_when_loudness_conforms() {
        let report = measure_audio_qc(
            AudioDeliveryPreset::Streaming.target(),
            measurement(Some(-1_400), Some(-50), None),
        );
        assert!(!report.technical_pass);
        assert_eq!(codes(&report), ["audio_true_peak_over_ceiling"]);
        assert_eq!(
            report.exceptions[0].allowed.as_deref(),
            Some("<= -1.00 dBTP")
        );
    }

    #[test]
    fn silence_is_an_error_with_a_target_and_information_without() {
        let with_target = measure_audio_qc(
            AudioDeliveryPreset::Podcast.target(),
            measurement(None, None, None),
        );
        assert!(!with_target.technical_pass);
        assert_eq!(codes(&with_target), ["audio_programme_silent"]);
        assert_eq!(with_target.exceptions[0].severity, QaSeverity::Error);

        let without = measure_audio_qc(
            AudioDeliveryPreset::MeasureOnly.target(),
            measurement(None, None, None),
        );
        assert!(without.technical_pass);
        assert_eq!(codes(&without), ["audio_programme_silent"]);
        assert_eq!(without.exceptions[0].severity, QaSeverity::Info);
    }

    #[test]
    fn loudness_range_and_analysis_rate_only_ever_warn() {
        let mut wide = measurement(Some(-2_300), Some(-100), Some(2_550));
        wide.loudness.sample_rate = 44_100;
        let report = measure_audio_qc(AudioDeliveryPreset::BroadcastEbuR128.target(), wide);
        assert!(report.technical_pass);
        assert_eq!(
            codes(&report),
            ["audio_analysis_rate_unexpected", "audio_loudness_range_over_limit"]
        );
        assert!(
            report
                .exceptions
                .iter()
                .all(|exception| exception.severity == QaSeverity::Warning)
        );
    }

    #[test]
    fn a_missing_true_peak_under_a_ceiling_is_a_warning_not_a_pass() {
        let report = measure_audio_qc(
            AudioDeliveryPreset::Streaming.target(),
            measurement(Some(-1_400), None, None),
        );
        assert!(report.technical_pass);
        assert_eq!(codes(&report), ["audio_true_peak_unmeasured"]);
    }

    #[test]
    fn every_published_code_is_reachable_and_nothing_else_is_published() {
        let reports = [
            measure_audio_qc(
                AudioDeliveryPreset::Streaming.target(),
                measurement(None, None, None),
            ),
            measure_audio_qc(
                AudioDeliveryPreset::Streaming.target(),
                measurement(Some(-2_000), Some(0), None),
            ),
            measure_audio_qc(
                AudioDeliveryPreset::Streaming.target(),
                measurement(Some(-1_000), None, None),
            ),
            {
                let mut wide = measurement(Some(-2_300), Some(-100), Some(9_900));
                wide.loudness.sample_rate = 96_000;
                measure_audio_qc(AudioDeliveryPreset::BroadcastEbuR128.target(), wide)
            },
        ];
        let mut seen = reports
            .iter()
            .flat_map(|report| report.exceptions.iter().map(|e| e.code.clone()))
            .collect::<Vec<_>>();
        seen.sort_unstable();
        seen.dedup();
        let mut published = AUDIO_QC_CODES.map(str::to_owned).to_vec();
        published.sort_unstable();
        assert_eq!(seen, published);
    }

    #[test]
    fn hundredths_render_without_dropping_the_sign_of_small_values() {
        assert_eq!(hundredths_to_string(-50), "-0.50");
        assert_eq!(hundredths_to_string(-1_400), "-14.00");
        assert_eq!(hundredths_to_string(5), "0.05");
        assert_eq!(hundredths_to_string(0), "0.00");
    }
}
