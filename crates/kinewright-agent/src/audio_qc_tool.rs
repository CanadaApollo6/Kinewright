//! AU3 §4.1: `get_audio_qc`, the agent's read-only audio QC surface.
//!
//! The audio twin of [`crate::color_qc_tool`]: media measures the post-clamp
//! master over one project range through the export mix path
//! (`Analysis::audio_qc`), core judges it against fixed thresholds and an
//! optional delivery profile's loudness target, and this module publishes the
//! result as evidence. It constructs no [`kinewright_core::Operation`],
//! commits nothing, gates no export, and leaves the timeline revision exactly
//! where it found it.
//!
//! **A range shorter than one 400 ms gating block is refused, not measured.**
//! `Analysis::audio_qc` raises the typed
//! [`MediaError::MixLoudnessRangeTooShort`] before decoding anything, so after
//! a refusal-free measurement `integrated_lufs_hundredths == null` means the
//! range was gated out entirely — silent — and nothing else.

use std::fmt::Write as _;

use kinewright_core::{
    Analysis, AudioQcReport, AudioQcRequest, DeliveryProfile, Document, MediaError, TimeCode,
    TimelineRevision,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{color_scopes::ScopeError, server::render_optional_hundredths};

/// The `get_audio_qc` tool description, kept under M36's 1 KB budget.
pub(crate) const AUDIO_QC_DESCRIPTION: &str = "Measure evidence-only audio QC of the master mix over an optional half-open project-frame window: \
integrated, momentary-max and short-term-max loudness, loudness range, 8x-oversampled true peak, sample peak, \
per-channel clipping on the pre-clamp master, channel balance, and leading and trailing silence, every value an \
integer in hundredths. Loudness follows ITU-R BS.1770-4 and the range EBU Tech 3342. Give profile to judge the report against \
that delivery profile's published loudness target (get_delivery_profiles); without one the target checks are \
skipped. Omit both frame bounds to measure the whole timeline; a range shorter than one 400 ms gating block \
(19200 sample frames) is refused, and a silent range reads integrated null with an audio_silent warning. \
Read-only: it constructs no operation, changes no document, and never gates an export; technical_pass is not \
export_ready.";

/// Canonical request envelope for `get_audio_qc`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AudioQcArgs {
    /// Optional revision returned by `get_timeline_state`. This is an
    /// inspector, not a planner, so omitting it succeeds; supplying a stale
    /// one fails with the uniform `stale_revision`.
    #[serde(default)]
    pub expected_revision: Option<TimelineRevision>,
    /// Optional inclusive first project frame. Defaults to 0 when only
    /// `end_frame` is given, and to the whole timeline when both are omitted.
    #[serde(default)]
    pub start_frame: Option<TimeCode>,
    /// Optional exclusive last project frame. Defaults to the timeline
    /// duration when only `start_frame` is given.
    #[serde(default)]
    pub end_frame: Option<TimeCode>,
    /// Optional delivery profile whose loudness target the report is judged
    /// against. Omitted, the report carries no target and raises no target
    /// exception.
    #[serde(default)]
    pub profile: Option<DeliveryProfile>,
}

/// Why one `get_audio_qc` call published no report.
#[derive(Debug)]
pub(crate) enum AudioQcRefusal {
    /// A stale `expected_revision`: the uniform `stale_revision` envelope.
    Stale(ScopeError),
    /// An inverted range, a range shorter than one gating block, or a
    /// measurement failure, as tool-call text (AU1's `get_audio_levels`
    /// idiom).
    Text(String),
}

/// One published measurement: the human-readable lines and the envelope.
#[derive(Debug)]
pub(crate) struct AudioQcResponse {
    pub text: String,
    pub value: Value,
}

/// Measure the master over the requested range and publish the AU3 §4.1
/// report.
///
/// # Errors
///
/// Returns [`AudioQcRefusal::Stale`] for a stale revision and
/// [`AudioQcRefusal::Text`] for `start_frame >= end_frame`, for a range
/// shorter than one 400 ms gating block, and for any other measurement error.
pub(crate) fn get_audio_qc(
    document: &Document,
    revision: TimelineRevision,
    analysis: &dyn Analysis,
    args: &AudioQcArgs,
) -> Result<AudioQcResponse, AudioQcRefusal> {
    if let Some(expected) = args.expected_revision
        && expected != revision
    {
        return Err(AudioQcRefusal::Stale(ScopeError::stale(expected, revision)));
    }
    let range = match (args.start_frame, args.end_frame) {
        (None, None) => None,
        (start, end) => {
            let start = start.unwrap_or(TimeCode::ZERO);
            let end = end.unwrap_or(document.duration);
            if start >= end {
                return Err(AudioQcRefusal::Text(format!(
                    "get_audio_qc needs start_frame < end_frame; got {start}..{end}"
                )));
            }
            Some(start..end)
        }
    };
    let request = AudioQcRequest {
        range,
        profile: args.profile,
    };
    let report = analysis
        .audio_qc(document, &request)
        .map_err(|error| match error {
            MediaError::MixLoudnessRangeTooShort {
                sample_frames,
                required,
            } => AudioQcRefusal::Text(format!(
                "get_audio_qc needs at least one 400 ms gating block ({required} sample frames); got {sample_frames}"
            )),
            other => AudioQcRefusal::Text(format!("could not measure audio qc: {other}")),
        })?;
    Ok(AudioQcResponse {
        text: summary(&report, args.profile),
        value: response(revision, &report, report.target.is_some()),
    })
}

/// The AU3 §4.1 response envelope: CC6's shape minus `stage` and
/// `full_resolution`, which a mix measurement does not have.
fn response(revision: TimelineRevision, report: &AudioQcReport, with_profile: bool) -> Value {
    json!({
        "timeline_revision": revision.0,
        "evidence_only": true,
        "applied": false,
        "report": report,
        "assumptions": assumptions(with_profile),
        "exceptions": report.exceptions,
    })
}

/// The stated assumptions behind one measurement, in a fixed order.
fn assumptions(with_profile: bool) -> Vec<String> {
    let mut assumptions = vec![
        "Measured at 48 kHz stereo through the export mix path with the document's buses, master chain, and pan law applied: the same signal export encodes.".to_owned(),
        "Clipping is counted per channel on the summed master before the single clamp; loudness, true peak, balance, and silence are measured after it. A clipped run is three or more consecutive full-scale samples on one channel.".to_owned(),
        "Loudness follows ITU-R BS.1770-4 gating on complete 400 ms blocks and a range shorter than one block is refused; loudness range follows EBU Tech 3342; true peak is an 8x oversampled measurement conformant to EBU Tech 3341 tolerances and reads at most 0.04 dB under between grid points.".to_owned(),
    ];
    if with_profile {
        assumptions.push(
            "The target is the profile's published delivery contract (get_delivery_profiles). Distance from it is evidence; normalization is a separate, explicit export step, and monitoring is not delivery.".to_owned(),
        );
    }
    assumptions.push(
        "Evidence only. This tool constructs no operation, changes no document, and never gates an export: technical_pass is not export_ready.".to_owned(),
    );
    assumptions
}

/// The four text lines of AU3 §4.1, hundredths throughout and `none` for an
/// unmeasurable value, so the summary and the structured report agree.
fn summary(report: &AudioQcReport, profile: Option<DeliveryProfile>) -> String {
    let mut text = String::new();
    let target = render_optional_hundredths(report.target.map(|t| t.integrated_lufs_hundredths));
    let tolerance = render_optional_hundredths(report.target.map(|t| t.tolerance_lu_hundredths));
    let ceiling =
        render_optional_hundredths(report.target.map(|t| t.true_peak_ceiling_dbtp_hundredths));
    let _ = writeln!(
        text,
        "audio_qc range={}..{} profile={} target={target}±{tolerance} ceiling={ceiling} technical_pass={}",
        report.range.start,
        report.range.end,
        profile.map_or("none", DeliveryProfile::as_str),
        report.technical_pass,
    );
    let master = &report.master;
    let _ = writeln!(
        text,
        "master lufs={} momentary_max={} short_term_max={} lra={} true_peak={} peak={} frames={}",
        render_optional_hundredths(master.integrated_lufs_hundredths),
        render_optional_hundredths(master.momentary_max_lufs_hundredths),
        render_optional_hundredths(master.short_term_max_lufs_hundredths),
        render_optional_hundredths(master.loudness_range_lu_hundredths),
        render_optional_hundredths(master.true_peak_dbtp_hundredths),
        render_optional_hundredths(master.sample_peak_dbfs_hundredths),
        master.sample_frames,
    );
    let clipping = &report.clipping;
    let _ = write!(
        text,
        "balance={} clipping L samples={} runs={} bp={} R samples={} runs={} bp={} leading_silence={} trailing_silence={}",
        render_optional_hundredths(report.channel_balance_lu_hundredths),
        clipping.left.over_full_scale_samples,
        clipping.left.clipped_runs,
        clipping.left.basis_points,
        clipping.right.over_full_scale_samples,
        clipping.right.clipped_runs,
        clipping.right.basis_points,
        report.leading_silence_frames,
        report.trailing_silence_frames,
    );
    for exception in &report.exceptions {
        let _ = write!(
            text,
            "\nexception {:?} {} {} observed={} allowed={}",
            exception.severity,
            exception.code,
            exception.field.as_deref().unwrap_or("none"),
            exception.observed.as_deref().unwrap_or("none"),
            exception.allowed.as_deref().unwrap_or("none"),
        );
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AU3 §4.2 / A16: the description stays inside M36's kilobyte budget.
    /// The registered descriptor is measured again in `server.rs`; this pin
    /// catches an edit to the literal before it reaches the registry.
    #[test]
    fn au3_get_audio_qc_description_is_under_the_m36_budget() {
        assert!(
            AUDIO_QC_DESCRIPTION.len() < 1_024,
            "{} bytes",
            AUDIO_QC_DESCRIPTION.len()
        );
        assert!(AUDIO_QC_DESCRIPTION.contains("400 ms gating block"));
        // The block length in the prose is media's constant, not a second number.
        assert!(
            AUDIO_QC_DESCRIPTION
                .contains(&kinewright_media::LOUDNESS_GATING_BLOCK_FRAMES.to_string())
        );
        assert!(AUDIO_QC_DESCRIPTION.ends_with("technical_pass is not export_ready."));
    }

    /// AU3 §4.1: `deny_unknown_fields`, so a resolution knob of any spelling
    /// or a misspelt bound is a malformed request, not a silently ignored one.
    #[test]
    fn au3_get_audio_qc_args_reject_unknown_fields() {
        let parsed: Result<AudioQcArgs, _> = serde_json::from_value(
            json!({"start_frame": 0, "end_frame": 30, "profile": "youtube1080p"}),
        );
        let parsed = parsed.unwrap();
        assert_eq!(parsed.start_frame, Some(TimeCode(0)));
        assert_eq!(parsed.end_frame, Some(TimeCode(30)));
        assert_eq!(parsed.profile, Some(DeliveryProfile::Youtube1080p));
        for unknown in [
            json!({"resolution": "proxy"}),
            json!({"start": 0}),
            json!({"range": {"start": 0, "end": 30}}),
        ] {
            let refused: Result<AudioQcArgs, _> = serde_json::from_value(unknown.clone());
            assert!(refused.is_err(), "{unknown} must be refused");
        }
    }

    /// AU3 §4.1: the five assumptions in their fixed order with a profile,
    /// four without, and the evidence-only boundary last either way.
    #[test]
    fn au3_get_audio_qc_assumptions_are_fixed_and_end_with_the_boundary() {
        let with = assumptions(true);
        let without = assumptions(false);
        assert_eq!(with.len(), 5);
        assert_eq!(without.len(), 4);
        assert!(with[0].starts_with("Measured at 48 kHz stereo"));
        assert!(with[1].starts_with("Clipping is counted per channel"));
        assert!(with[2].starts_with("Loudness follows ITU-R BS.1770-4"));
        assert!(with[3].contains("get_delivery_profiles"));
        for list in [&with, &without] {
            assert!(
                list.last()
                    .unwrap()
                    .ends_with("technical_pass is not export_ready.")
            );
        }
        assert!(
            !without
                .iter()
                .any(|assumption| assumption.contains("get_delivery_profiles"))
        );
    }
}
