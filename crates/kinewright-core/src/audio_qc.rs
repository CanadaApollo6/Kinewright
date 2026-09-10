//! AU3 §2.4: the audio QC report types and the pure exception rules.
//!
//! Media measures (AU3 §3.10) and core judges: every threshold here is a
//! `const`, not a parameter, so two reports are comparable (CC6's
//! `_BASIS_POINTS` rule, `color_qc.rs`), and every number is an integer with
//! its unit in the field name. A report is evidence, never a gate: it carries
//! `technical_pass`, deliberately not `export_ready`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AudioLoudness, DeliveryProfile, LoudnessTarget, QaSeverity, TimeCode};

/// What to measure: a project range (omitted or `null` measures the whole
/// document) and an optional delivery profile whose loudness target the
/// report is judged against.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AudioQcRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub range: Option<std::ops::Range<TimeCode>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub profile: Option<DeliveryProfile>,
}

/// Clipping counted on one channel of the pre-clamp master.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AudioChannelClipping {
    /// Samples of this channel at `|x| >= 1.0` on the pre-clamp master.
    pub over_full_scale_samples: u64,
    /// Runs of at least [`AUDIO_QC_CLIPPED_RUN_SAMPLES`] consecutive such
    /// samples on this channel.
    pub clipped_runs: u32,
    /// `floor(over_full_scale_samples * 10_000 / this channel's sample
    /// count)`; `0` when empty.
    pub basis_points: u32,
}

/// Per-channel clipping for the stereo master.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AudioClipping {
    pub left: AudioChannelClipping,
    pub right: AudioChannelClipping,
}

/// One reportable audio QC finding: `ColorQcException`'s shape
/// (`color_qc.rs`) under its own name, without `clip` and `effect`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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

/// How an [`AudioQcReport`] was computed, so the choices are auditable.
///
/// Fixed strings; a manual `impl Default` (no derive) so the eight values are
/// one literal each.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AudioQcProvenance {
    pub engine: String,
    pub measurement_rate: u32,
    pub k_weighting: String,
    pub true_peak: String,
    pub true_peak_bias: String,
    pub gate: String,
    pub loudness_range: String,
    pub silence: String,
    pub exception_order: String,
}

/// The engine identity recorded in every [`AudioQcProvenance`].
pub const AUDIO_QC_ENGINE: &str = "kinewright_audio_qc_v1";

impl Default for AudioQcProvenance {
    fn default() -> Self {
        Self {
            engine: AUDIO_QC_ENGINE.to_owned(),
            measurement_rate: 48_000,
            k_weighting: "bs1770_4_bilinear_from_prototypes".to_owned(),
            true_peak: "8x_polyphase_256_tap_blackman_harris".to_owned(),
            true_peak_bias: "exact_on_grid_at_most_-0.042_db_between_grid_points".to_owned(),
            gate: "bs1770_4_two_stage_complete_blocks".to_owned(),
            loudness_range: "ebu_tech_3342_nearest_rank".to_owned(),
            silence: "10ms_rms_-70_dbfs_post_clamp".to_owned(),
            exception_order: "severity_desc_code_asc_field_asc".to_owned(),
        }
    }
}

/// One audio QC measurement of the master over a project range.
///
/// Like `ColorQcReport`, it deliberately does not reuse the name
/// `export_ready`: `QaReport::export_ready` gates an export, and a QC report
/// must never gate one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AudioQcReport {
    pub range: std::ops::Range<TimeCode>,
    /// The post-clamp master: the signal export encodes.
    pub master: AudioLoudness,
    /// `10·log10(E_L/E_R)` over the integrated gate's block set, clamped to
    /// ±[`AUDIO_QC_CHANNEL_BALANCE_CLAMP_LU_HUNDREDTHS`] (AU3 §3.10). `None`
    /// for mono, for an empty gated set, or when one channel's gated energy
    /// is zero.
    pub channel_balance_lu_hundredths: Option<i32>,
    pub clipping: AudioClipping,
    pub leading_silence_frames: TimeCode,
    pub trailing_silence_frames: TimeCode,
    pub target: Option<LoudnessTarget>,
    pub exceptions: Vec<AudioQcException>,
    /// No `Error`-severity exception.
    pub technical_pass: bool,
    /// Always `true`.
    pub evidence_only: bool,
    pub provenance: AudioQcProvenance,
}

/// The RMS level at or under which a 10 ms window counts as silence, on the
/// post-clamp master; the same −70 dBFS as the BS.1770 absolute gate.
pub const AUDIO_QC_SILENCE_DBFS_HUNDREDTHS: i32 = -7_000;
/// The silence window: `detect_silences`' 10 ms RMS window, sample domain.
pub const AUDIO_QC_SILENCE_WINDOW_MILLISECONDS: u32 = 10;
/// Leading or trailing silence longer than this raises an `Info`.
pub const AUDIO_QC_SILENCE_INFO_MILLISECONDS: u32 = 1_000;
/// Consecutive samples at or over full scale that close one clipped run.
pub const AUDIO_QC_CLIPPED_RUN_SAMPLES: u32 = 3;
/// A channel balance beyond ±3 LU raises a `Warning`.
pub const AUDIO_QC_CHANNEL_IMBALANCE_LU_HUNDREDTHS: i32 = 300;
/// The channel balance is clamped to ±60 LU so a near-silent side stays an
/// integer rather than an infinity.
pub const AUDIO_QC_CHANNEL_BALANCE_CLAMP_LU_HUNDREDTHS: i32 = 6_000;

/// The measured numbers media hands core to build the exception list
/// (AU3 §2.4, §3.10). Silence arrives both as report frames and as the
/// millisecond run length the window count fixes, so the 1 000 ms rule never
/// depends on a frame-rate rounding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioQcMeasurements {
    /// The post-clamp master.
    pub master: AudioLoudness,
    /// See [`AudioQcReport::channel_balance_lu_hundredths`].
    pub channel_balance_lu_hundredths: Option<i32>,
    pub clipping: AudioClipping,
    pub leading_silence_frames: TimeCode,
    /// The leading silent run in whole silence windows, in milliseconds.
    pub leading_silence_milliseconds: u64,
    pub trailing_silence_frames: TimeCode,
    /// The trailing silent run in whole silence windows, in milliseconds.
    pub trailing_silence_milliseconds: u64,
    pub target: Option<LoudnessTarget>,
}

/// Build the AU3 §2.4 exception table from measured numbers, sorted by
/// `(severity desc, code asc, field asc)`.
///
/// After a refusal-free measurement (`MediaError::MixLoudnessRangeTooShort`
/// is raised before decoding) `integrated == None` means the range was gated
/// out entirely — silent — and nothing else, so `audio_silent` suppresses the
/// target, balance, and silence entries. Clipping is counted on the pre-clamp
/// master and is raised regardless.
#[must_use]
pub fn audio_qc_exceptions(measured: &AudioQcMeasurements) -> Vec<AudioQcException> {
    let mut exceptions = Vec::new();
    for (side, channel) in [
        ("left", measured.clipping.left),
        ("right", measured.clipping.right),
    ] {
        if channel.clipped_runs > 0 {
            exceptions.push(AudioQcException {
                code: "audio_clipping".to_owned(),
                severity: QaSeverity::Error,
                message: format!(
                    "The {side} channel of the pre-clamp master holds {runs} run(s) of at least {AUDIO_QC_CLIPPED_RUN_SAMPLES} consecutive samples at or over full scale ({over} samples, {rate} basis points); the export clamp flattens them.",
                    runs = channel.clipped_runs,
                    over = channel.over_full_scale_samples,
                    rate = channel.basis_points,
                ),
                field: Some(format!("clipping.{side}.clipped_runs")),
                observed: Some(channel.clipped_runs.to_string()),
                allowed: Some("0".to_owned()),
            });
        }
    }

    if measured.master.integrated_lufs_hundredths.is_none() {
        exceptions.push(AudioQcException {
            code: "audio_silent".to_owned(),
            severity: QaSeverity::Warning,
            message: "The master is silent over the measured range: no 400 ms block passed the -70 LUFS absolute gate, so the target, channel-balance, and silence checks are not raised.".to_owned(),
            field: Some("integrated_lufs_hundredths".to_owned()),
            observed: Some("none".to_owned()),
            allowed: Some(format!("> {AUDIO_QC_SILENCE_DBFS_HUNDREDTHS}")),
        });
        sort_exceptions(&mut exceptions);
        return exceptions;
    }

    if let Some(target) = measured.target {
        exceptions.extend(loudness_target_exceptions(
            &measured.master,
            target,
            "audio",
        ));
    }

    let balance_allowed = format!(
        "{}..={}",
        -AUDIO_QC_CHANNEL_IMBALANCE_LU_HUNDREDTHS, AUDIO_QC_CHANNEL_IMBALANCE_LU_HUNDREDTHS
    );
    match measured.channel_balance_lu_hundredths {
        Some(balance) if balance.abs() > AUDIO_QC_CHANNEL_IMBALANCE_LU_HUNDREDTHS => {
            exceptions.push(AudioQcException {
                code: "audio_channel_imbalance".to_owned(),
                severity: QaSeverity::Warning,
                message: format!(
                    "The left channel is {balance} hundredths of an LU louder than the right over the gated blocks; the house band is ±{AUDIO_QC_CHANNEL_IMBALANCE_LU_HUNDREDTHS}."
                ),
                field: Some("channel_balance_lu_hundredths".to_owned()),
                observed: Some(balance.to_string()),
                allowed: Some(balance_allowed),
            });
        }
        // The gate passed blocks (integrated is `Some`) on a stereo master,
        // yet no ratio could be formed: one side's gated energy is zero.
        None if measured.master.channels >= 2 => {
            exceptions.push(AudioQcException {
                code: "audio_channel_imbalance".to_owned(),
                severity: QaSeverity::Warning,
                message: "One channel is silent over the gated blocks while the other is not, so no balance can be formed.".to_owned(),
                field: Some("channel_balance_lu_hundredths".to_owned()),
                observed: Some("none (one channel silent)".to_owned()),
                allowed: Some(balance_allowed),
            });
        }
        _ => {}
    }

    for (edge, verb, frames, milliseconds) in [
        (
            "leading",
            "opens with",
            measured.leading_silence_frames,
            measured.leading_silence_milliseconds,
        ),
        (
            "trailing",
            "ends with",
            measured.trailing_silence_frames,
            measured.trailing_silence_milliseconds,
        ),
    ] {
        if milliseconds > u64::from(AUDIO_QC_SILENCE_INFO_MILLISECONDS) {
            exceptions.push(AudioQcException {
                code: format!("audio_{edge}_silence"),
                severity: QaSeverity::Info,
                message: format!(
                    "The programme {verb} {frames} frames ({milliseconds} ms) of silence under {AUDIO_QC_SILENCE_DBFS_HUNDREDTHS} hundredths of a dBFS."
                ),
                field: Some(format!("{edge}_silence_frames")),
                observed: Some(format!("{frames} frames ({milliseconds} ms)")),
                allowed: Some("<= 1 s".to_owned()),
            });
        }
    }

    sort_exceptions(&mut exceptions);
    exceptions
}

/// The three target checks, shared by the QC report (prefix `audio`) and the
/// delivery audio verification (prefix `delivery`, AU3 §5.3): true peak over
/// the ceiling (`Error`), integrated loudness outside the tolerance
/// (`Warning`), and loudness range over the target's maximum (`Warning`).
/// A measurement that is `None` raises nothing here.
#[must_use]
pub fn loudness_target_exceptions(
    measured: &AudioLoudness,
    target: LoudnessTarget,
    prefix: &str,
) -> Vec<AudioQcException> {
    let mut exceptions = Vec::new();
    let ceiling = target.true_peak_ceiling_dbtp_hundredths;
    if let Some(true_peak) = measured.true_peak_dbtp_hundredths
        && true_peak > ceiling
    {
        exceptions.push(AudioQcException {
            code: format!("{prefix}_true_peak_over_ceiling"),
            severity: QaSeverity::Error,
            message: format!(
                "The true peak of {true_peak} hundredths of a dBTP is over the target ceiling of {ceiling}."
            ),
            field: Some("true_peak_dbtp_hundredths".to_owned()),
            observed: Some(true_peak.to_string()),
            allowed: Some(format!("<= {ceiling}")),
        });
    }
    let integrated_target = target.integrated_lufs_hundredths;
    let tolerance = target.tolerance_lu_hundredths;
    if let Some(integrated) = measured.integrated_lufs_hundredths
        && (integrated - integrated_target).abs() > tolerance
    {
        exceptions.push(AudioQcException {
            code: format!("{prefix}_loudness_out_of_tolerance"),
            severity: QaSeverity::Warning,
            message: format!(
                "The integrated loudness of {integrated} hundredths of a LUFS is outside the target of {integrated_target} ±{tolerance}."
            ),
            field: Some("integrated_lufs_hundredths".to_owned()),
            observed: Some(integrated.to_string()),
            allowed: Some(format!(
                "{}..={}",
                integrated_target - tolerance,
                integrated_target + tolerance
            )),
        });
    }
    if let (Some(range), Some(maximum)) = (
        measured.loudness_range_lu_hundredths,
        target.loudness_range_max_lu_hundredths,
    ) && range > maximum
    {
        exceptions.push(AudioQcException {
            code: format!("{prefix}_loudness_range_over_maximum"),
            severity: QaSeverity::Warning,
            message: format!(
                "The loudness range of {range} hundredths of an LU is over the target maximum of {maximum}."
            ),
            field: Some("loudness_range_lu_hundredths".to_owned()),
            observed: Some(range.to_string()),
            allowed: Some(format!("<= {maximum}")),
        });
    }
    sort_exceptions(&mut exceptions);
    exceptions
}

/// The delivery audio verification's exceptions (AU3 §5.3).
///
/// With `None` the verification is a reference measurement and raises
/// nothing. With a target it is [`loudness_target_exceptions`] under the
/// `delivery` prefix, plus `delivery_audio_silent` (`Warning`) when the
/// decoded file has no gated block at all. Unlike the QC report, a delivery
/// measurement is never length-refused, so that reading is ambiguous between a
/// silent programme and one shorter than a single gating block; the message
/// says both.
#[must_use]
pub fn delivery_audio_exceptions(
    measured: &AudioLoudness,
    target: Option<LoudnessTarget>,
) -> Vec<AudioQcException> {
    let Some(target) = target else {
        return Vec::new();
    };
    let mut exceptions = loudness_target_exceptions(measured, target, "delivery");
    if measured.integrated_lufs_hundredths.is_none() {
        exceptions.push(AudioQcException {
            code: "delivery_audio_silent".to_owned(),
            severity: QaSeverity::Warning,
            message: "The written file's audio reported no gated loudness: it is silent, or shorter than one 400 ms gating block.".to_owned(),
            field: Some("integrated_lufs_hundredths".to_owned()),
            observed: Some("none".to_owned()),
            allowed: Some(format!("> {AUDIO_QC_SILENCE_DBFS_HUNDREDTHS}")),
        });
    }
    sort_exceptions(&mut exceptions);
    exceptions
}

/// `technical_pass`: no `Error`-severity exception. Warnings do not clear it.
#[must_use]
pub fn audio_qc_technical_pass(exceptions: &[AudioQcException]) -> bool {
    !exceptions
        .iter()
        .any(|exception| exception.severity == QaSeverity::Error)
}

/// `(severity desc, code asc, field asc)`: `color_qc.rs`'s comparator without
/// the clip and effect terms. A missing field sorts as the empty string.
///
/// `pub(crate)` since AU5 §0 R50: `audio_repair_exceptions` must sort by the
/// same rule, and sharing the one comparator is the only way two modules
/// cannot drift apart on it.
pub(crate) fn sort_exceptions(exceptions: &mut [AudioQcException]) {
    exceptions.sort_by(|left, right| {
        severity_rank(left.severity)
            .cmp(&severity_rank(right.severity))
            .then_with(|| left.code.cmp(&right.code))
            .then_with(|| {
                left.field
                    .as_deref()
                    .unwrap_or_default()
                    .cmp(right.field.as_deref().unwrap_or_default())
            })
    });
}

/// `Error` first, then `Warning`, then `Info`: severity descending.
const fn severity_rank(severity: QaSeverity) -> u8 {
    match severity {
        QaSeverity::Error => 0,
        QaSeverity::Warning => 1,
        QaSeverity::Info => 2,
    }
}
