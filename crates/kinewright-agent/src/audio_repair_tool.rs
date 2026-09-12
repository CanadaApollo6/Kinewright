//! AU5 §4.1: `get_audio_repair`, the agent's read-only repair-measurement
//! surface.
//!
//! The third member of the audio evidence family, on
//! [`crate::audio_qc_tool`]'s exact shape: media measures one mix point over
//! one project range through the export mix path (`Analysis::audio_repair`),
//! core judges the numbers against four fixed thresholds
//! (`audio_repair_exceptions`), and this module publishes the result. It
//! constructs no [`kinewright_core::Operation`], commits nothing, gates no
//! export, and leaves the timeline revision exactly where it found it.
//!
//! **The noise floor is a percentile, not a detected silence** (AU5 §2.4 rule
//! 21). It is the 10th-percentile 10 ms window level and the signal is the
//! 90th, so on material with real silence the split is right, while on
//! continuous speech the 10th percentile is *quiet speech*: the floor reads
//! high and the SNR reads low. That is the safe direction, but a planner that
//! did not know it would refuse a repair that would have worked, which is why
//! the bias is stated in the tool description's first sentence, in the
//! rendered text and in the report's own doc comment rather than in any one of
//! them alone.

use std::fmt::Write as _;

use kinewright_core::{
    Analysis, AudioBusId, AudioRepairReport, AudioRepairRequest, Document, MediaError,
    MixSpectrumPoint, TimeCode, TimelineRevision, TrackId,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{color_scopes::ScopeError, server::render_optional_hundredths};

/// The `get_audio_repair` tool description, kept under M36's 1 KB budget.
///
/// AU5 §4.1 rule 76: `get_capability` and `search_capabilities` publish only
/// `first_sentence(description)`, so the percentile clause and the direction
/// of its bias ride in the FIRST sentence, where a compact surface reaches
/// them; everything a caller needs only once it has decided to call the tool
/// follows.
pub(crate) const AUDIO_REPAIR_DESCRIPTION: &str = "Measures a percentile noise floor, percentile SNR, 50/60 Hz hum excess and click density over one timeline \
range at one point; the floor is the 10th-percentile short window, not a detected silence, so continuous speech \
reads a higher floor and a lower SNR; evidence only. The signal is the 90th percentile of the same 10 ms RMS \
windows and the SNR is their difference, all three reported as null under ten energetic windows with a \
low_window_count finding rather than a made-up number. Hum excess is each mains fundamental and its first three \
harmonics over their sixth-octave shoulders, summed, with the four per-harmonic values also reported. Every value \
is an integer in hundredths. Measure the master by default, or one track's post-track-stage stem, or one bus's \
post-chain stem. Omit both frame bounds to measure the whole timeline. Read-only: it constructs no operation, \
changes no document, and never gates an export.";

/// Canonical request envelope for `get_audio_repair`.
///
/// `deny_unknown_fields` so a misspelled bound or an invented resolution knob
/// is refused by name rather than silently defaulted (AU5 §7 A15).
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AudioRepairArgs {
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
    /// Measure this track's post-track-stage stem instead of the master.
    #[serde(default)]
    pub track: Option<TrackId>,
    /// Measure this bus's post-chain stem instead of the master.
    #[serde(default)]
    pub bus: Option<AudioBusId>,
}

/// Why one `get_audio_repair` call published no report.
#[derive(Debug)]
pub(crate) enum AudioRepairRefusal {
    /// A stale `expected_revision`: the uniform `stale_revision` envelope.
    Stale(ScopeError),
    /// Both `track` and `bus`, an inverted range, a range too short to
    /// measure, or a measurement failure, as tool-call text (AU1's
    /// `get_audio_levels` idiom).
    Text(String),
}

/// One published measurement: the human-readable lines and the envelope.
#[derive(Debug)]
pub(crate) struct AudioRepairResponse {
    pub text: String,
    pub value: Value,
}

/// Measure one mix point over the requested range and publish the AU5 §4.1
/// report.
///
/// # Errors
///
/// Returns [`AudioRepairRefusal::Stale`] for a stale revision and
/// [`AudioRepairRefusal::Text`] for both `track` and `bus`, for
/// `start_frame >= end_frame`, and for any measurement error.
pub(crate) fn get_audio_repair(
    document: &Document,
    revision: TimelineRevision,
    analysis: &dyn Analysis,
    args: &AudioRepairArgs,
) -> Result<AudioRepairResponse, AudioRepairRefusal> {
    if let Some(expected) = args.expected_revision
        && expected != revision
    {
        return Err(AudioRepairRefusal::Stale(ScopeError::stale(
            expected, revision,
        )));
    }
    let point = match (args.track, args.bus) {
        (Some(_), Some(_)) => {
            return Err(AudioRepairRefusal::Text(
                "get_audio_repair takes at most one of track and bus".to_owned(),
            ));
        }
        (Some(track), None) => MixSpectrumPoint::Track(track),
        (None, Some(bus)) => MixSpectrumPoint::Bus(bus),
        (None, None) => MixSpectrumPoint::Master,
    };
    let range = match (args.start_frame, args.end_frame) {
        (None, None) => None,
        (start, end) => {
            let start = start.unwrap_or(TimeCode::ZERO);
            let end = end.unwrap_or(document.duration);
            if start >= end {
                return Err(AudioRepairRefusal::Text(format!(
                    "get_audio_repair needs start_frame < end_frame; got {start}..{end}"
                )));
            }
            Some(start..end)
        }
    };
    let request = AudioRepairRequest { range, point };
    let report = analysis
        .audio_repair(document, &request)
        .map_err(|error| match error {
            MediaError::MixLoudnessRangeTooShort {
                sample_frames,
                required,
            } => AudioRepairRefusal::Text(format!(
                "get_audio_repair needs at least {required} sample frames to measure; got {sample_frames}"
            )),
            other => AudioRepairRefusal::Text(format!("could not measure audio repair: {other}")),
        })?;
    Ok(AudioRepairResponse {
        text: summary(&report),
        value: response(revision, &report),
    })
}

/// The AU5 §4.1 response envelope: exactly the two keys A15 names.
///
/// The report carries its own `evidence_only` flag and its own `findings`, so
/// republishing either beside it would be a second place for them to disagree.
fn response(revision: TimelineRevision, report: &AudioRepairReport) -> Value {
    json!({
        "timeline_revision": revision.0,
        "report": report,
    })
}

/// The rendered lines of AU5 §4.1 rule 77: every figure with its unit,
/// `none` for an absent one, and the percentile clause repeated in prose so a
/// reader of the text — not just of the schema — knows what the floor is.
fn summary(report: &AudioRepairReport) -> String {
    let mut text = String::new();
    let _ = writeln!(
        text,
        "audio_repair range={}..{} point={} sample_rate={} sample_frames={} window={}ms windows={}",
        report.range.start,
        report.range.end,
        crate::server::render_mix_spectrum_point(report.point),
        report.sample_rate,
        report.sample_frames,
        report.window_milliseconds,
        report.windows,
    );
    let _ = writeln!(
        text,
        "noise_floor={} signal={} snr={} in dBFS hundredths; the floor is the 10th-percentile {} ms window and the signal the 90th, not a detected silence, so continuous speech reads a higher floor and a lower snr",
        render_optional_hundredths(report.noise_floor_dbfs_hundredths),
        render_optional_hundredths(report.signal_dbfs_hundredths),
        render_optional_hundredths(report.snr_db_hundredths),
        report.window_milliseconds,
    );
    let _ = writeln!(
        text,
        "hum_50={} harmonics={} hum_60={} harmonics={} in dB hundredths over the sixth-octave shoulders, summed over four harmonics",
        render_optional_hundredths(report.hum_50_excess_db_hundredths),
        render_harmonics(&report.hum_50_harmonic_excess_db_hundredths),
        render_optional_hundredths(report.hum_60_excess_db_hundredths),
        render_harmonics(&report.hum_60_harmonic_excess_db_hundredths),
    );
    let _ = write!(
        text,
        "clicks={} density={} a minute evidence_only={}",
        report.click_count, report.click_density_per_minute, report.evidence_only,
    );
    for finding in &report.findings {
        let _ = write!(
            text,
            "\nfinding {:?} {} {} observed={} allowed={}",
            finding.severity,
            finding.code,
            finding.field.as_deref().unwrap_or("none"),
            finding.observed.as_deref().unwrap_or("none"),
            finding.allowed.as_deref().unwrap_or("none"),
        );
    }
    text
}

/// The four per-harmonic excesses, low to high, or `none` when the range held
/// under one Goertzel block and the report published no harmonic at all.
fn render_harmonics(harmonics: &[i32]) -> String {
    if harmonics.is_empty() {
        return "none".to_owned();
    }
    let mut rendered = String::from("[");
    for (index, excess) in harmonics.iter().enumerate() {
        if index != 0 {
            rendered.push(',');
        }
        let _ = write!(rendered, "{excess}");
    }
    rendered.push(']');
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinewright_core::{AudioRepairProvenance, REPAIR_WINDOW_MILLISECONDS};

    /// AU5 §4.1 / A15: the description stays inside M36's kilobyte budget and
    /// carries the percentile clause plus the direction of its bias in the
    /// FIRST sentence, which is the only part `get_capability` and
    /// `search_capabilities` publish. The registered descriptor is measured
    /// again in `server.rs`; this pin catches an edit to the literal before it
    /// reaches the registry.
    #[test]
    fn au5_get_audio_repair_description_is_under_the_m36_budget() {
        assert!(
            AUDIO_REPAIR_DESCRIPTION.len() < 1_024,
            "{} bytes",
            AUDIO_REPAIR_DESCRIPTION.len()
        );
        let first = crate::runtime::first_sentence(AUDIO_REPAIR_DESCRIPTION);
        let first = first.as_str();
        assert!(first.contains("percentile"), "{first}");
        assert!(first.contains("10th-percentile short window"), "{first}");
        assert!(
            first.contains("higher floor and a lower SNR"),
            "the direction of the bias is rule 21's load-bearing clause: {first}"
        );
        assert!(first.contains("evidence only"), "{first}");
    }

    /// AU5 §4.1 / A15: `deny_unknown_fields`, so a misspelled bound is a
    /// malformed request, not a silently ignored one.
    #[test]
    fn au5_get_audio_repair_args_reject_unknown_fields() {
        let parsed: AudioRepairArgs =
            serde_json::from_value(json!({"start_frame": 0, "end_frame": 30, "bus": 2})).unwrap();
        assert_eq!(parsed.start_frame, Some(TimeCode(0)));
        assert_eq!(parsed.end_frame, Some(TimeCode(30)));
        assert_eq!(parsed.bus, Some(AudioBusId(2)));
        assert!(parsed.track.is_none());
        for unknown in [
            json!({"threshold_dbfs_hundreths": -4_000}),
            json!({"start": 0}),
            json!({"range": {"start": 0, "end": 30}}),
            json!({"point": "master"}),
        ] {
            let refused: Result<AudioRepairArgs, _> = serde_json::from_value(unknown.clone());
            assert!(refused.is_err(), "{unknown} must be refused");
        }
    }

    /// AU5 §4.1 rule 77: the rendered text names every figure with its unit,
    /// prints `none` for an absent one, and repeats the percentile clause in
    /// prose.
    #[test]
    fn au5_get_audio_repair_text_names_every_figure_and_the_percentile() {
        let report = AudioRepairReport {
            range: TimeCode::ZERO..TimeCode(60),
            point: MixSpectrumPoint::Master,
            sample_rate: 48_000,
            sample_frames: 96_000,
            window_milliseconds: REPAIR_WINDOW_MILLISECONDS,
            windows: 200,
            noise_floor_dbfs_hundredths: Some(-5_400),
            signal_dbfs_hundredths: Some(-1_200),
            snr_db_hundredths: Some(4_200),
            hum_50_excess_db_hundredths: Some(1_150),
            hum_60_excess_db_hundredths: None,
            hum_50_harmonic_excess_db_hundredths: vec![800, 250, 100, 0],
            hum_60_harmonic_excess_db_hundredths: Vec::new(),
            click_count: 19,
            click_density_per_minute: 570,
            findings: Vec::new(),
            evidence_only: true,
            provenance: AudioRepairProvenance::default(),
        };
        let text = summary(&report);
        assert!(
            text.contains("audio_repair range=0..60 point=master sample_rate=48000 sample_frames=96000 window=10ms windows=200"),
            "{text}"
        );
        assert!(
            text.contains("noise_floor=-5400 signal=-1200 snr=4200 in dBFS hundredths"),
            "{text}"
        );
        assert!(
            text.contains(
                "10th-percentile 10 ms window and the signal the 90th, not a detected silence"
            ),
            "the prose reader learns what the floor is: {text}"
        );
        assert!(
            text.contains("hum_50=1150 harmonics=[800,250,100,0] hum_60=none harmonics=none"),
            "{text}"
        );
        assert!(
            text.contains("clicks=19 density=570 a minute evidence_only=true"),
            "{text}"
        );
    }

    /// AU5 §2.4 rule 22: an unmeasurable percentile prints `none` in all three
    /// places, and the `low_window_count` finding is what says why.
    #[test]
    fn au5_get_audio_repair_text_prints_none_for_an_unmeasurable_percentile() {
        let report = AudioRepairReport {
            range: TimeCode::ZERO..TimeCode(2),
            point: MixSpectrumPoint::Track(TrackId(3)),
            sample_rate: 48_000,
            sample_frames: 3_200,
            window_milliseconds: REPAIR_WINDOW_MILLISECONDS,
            windows: 4,
            noise_floor_dbfs_hundredths: None,
            signal_dbfs_hundredths: None,
            snr_db_hundredths: None,
            hum_50_excess_db_hundredths: None,
            hum_60_excess_db_hundredths: None,
            hum_50_harmonic_excess_db_hundredths: Vec::new(),
            hum_60_harmonic_excess_db_hundredths: Vec::new(),
            click_count: 0,
            click_density_per_minute: 0,
            findings: kinewright_core::audio_repair_exceptions(
                &kinewright_core::AudioRepairMeasurements {
                    windows: 4,
                    noise_floor_dbfs_hundredths: None,
                    signal_dbfs_hundredths: None,
                    snr_db_hundredths: None,
                    hum_50_excess_db_hundredths: None,
                    hum_60_excess_db_hundredths: None,
                    click_count: 0,
                    click_density_per_minute: 0,
                },
            ),
            evidence_only: true,
            provenance: AudioRepairProvenance::default(),
        };
        let text = summary(&report);
        assert!(text.contains("point=track 3"), "{text}");
        assert!(
            text.contains("noise_floor=none signal=none snr=none"),
            "{text}"
        );
        assert!(
            text.contains("finding Info low_window_count windows observed=4 allowed=>= 10"),
            "{text}"
        );
    }
}
