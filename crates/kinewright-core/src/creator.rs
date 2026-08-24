use std::{cmp::Reverse, ops::Range};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    AssetId, BeatStatus, ClipId, Document, OpError, Operation, ThreePointMode, TimeCode,
    TimelineBeat, TrackId, TrackKind, map_source_range_to_project,
};

/// Completeness of the timeline-beat snapshot supplied to a creator plan.
///
/// Timeline analysis can legitimately return cached beats while other assets
/// are still running. Plans reject that partial state so an agent never
/// presents a provisional cut list as complete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TimelineBeatAnalysisState {
    Ready,
    Pending {
        asset_ids: Vec<AssetId>,
    },
    Unavailable {
        asset_ids: Vec<AssetId>,
        reason: String,
    },
}

/// One musical onset selected to pace the target clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BeatPacingPoint {
    pub beat_asset: AssetId,
    pub beat_track: TrackId,
    pub beat_clip: ClipId,
    pub source_frame: TimeCode,
    pub project_frame: TimeCode,
    pub strength_basis_points: u16,
    pub estimated_bpm_milli: u32,
}

impl From<TimelineBeat> for BeatPacingPoint {
    fn from(beat: TimelineBeat) -> Self {
        Self {
            beat_asset: beat.asset,
            beat_track: beat.track,
            beat_clip: beat.clip,
            source_frame: beat.source_frame,
            project_frame: beat.project_frame,
            strength_basis_points: beat.strength_basis_points,
            estimated_bpm_milli: beat.estimated_bpm_milli,
        }
    }
}

/// A deterministic set of cuts chosen from mapped timeline beats.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BeatPacingPlan {
    pub target_clip: ClipId,
    pub range: Range<TimeCode>,
    pub minimum_strength_basis_points: u16,
    pub minimum_spacing_frames: TimeCode,
    /// Selected musical onsets in ascending timeline order for inspection.
    pub selected_beats: Vec<BeatPacingPoint>,
    /// Atomic apply order. Splits are deliberately newest-to-oldest so every
    /// operation can continue to address the original left-hand clip id.
    pub operations: Vec<Operation>,
}

/// One model-selected source shot, in the exact order it should appear.
///
/// `source_range` is an allowed source envelope. The planner preserves its
/// earliest feasible source subrange and reports the exact subset consumed by
/// the resulting real-time edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BeatMontageSelect {
    pub asset: AssetId,
    pub source_range: Range<TimeCode>,
}

/// A musical boundary chosen between two adjacent montage shots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BeatMontageCutAnchor {
    /// Zero-based index of the shot immediately before this cut.
    pub after_shot_index: usize,
    pub beat: BeatPacingPoint,
}

/// One fully resolved, real-time shot in a beat montage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BeatMontageShot {
    pub select_index: usize,
    pub asset: AssetId,
    /// The caller-provided source limits used when validating the edit.
    pub source_envelope: Range<TimeCode>,
    /// The exact source frames consumed by the planned operation.
    pub source_range: Range<TimeCode>,
    pub timeline_range: Range<TimeCode>,
}

/// An inspectable, executable hard-cut montage paced to one music asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BeatMontagePlan {
    pub target_track: TrackId,
    pub music_asset: AssetId,
    pub timeline_range: Range<TimeCode>,
    pub minimum_strength_basis_points: u16,
    pub minimum_shot_frames: TimeCode,
    pub maximum_shot_frames: TimeCode,
    pub mode: ThreePointMode,
    pub shots: Vec<BeatMontageShot>,
    pub cut_anchors: Vec<BeatMontageCutAnchor>,
    pub operations: Vec<Operation>,
}

/// The observable cadence contract for a hard-cut beat montage.
///
/// Durations are measured in project frames. A cadence passes when it uses at
/// least `minimum_duration_buckets` distinct rounded duration buckets and no
/// consecutive run contains more than `maximum_similar_run` shots whose
/// adjacent durations differ by at most `similar_tolerance_frames`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BeatMontageCadenceContract {
    pub minimum_duration_buckets: usize,
    pub duration_bucket_frames: TimeCode,
    pub maximum_similar_run: usize,
    pub similar_tolerance_frames: TimeCode,
}

/// Computed evidence for a beat-montage cadence contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BeatMontageCadenceSummary {
    /// Project-frame duration of each shot in montage order.
    pub durations: Vec<TimeCode>,
    /// Rounded bucket for each shot, using the configured bucket size.
    pub rounded_buckets: Vec<i64>,
    /// Distinct rounded buckets in ascending order.
    pub distinct_buckets: Vec<i64>,
    /// Longest consecutive run whose adjacent durations are within tolerance.
    pub longest_similar_run: usize,
}

/// Inspectable evidence for a repaired beat-montage anchor schedule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BeatMontageAnchorRepair {
    pub preferred_anchors: Vec<TimeCode>,
    pub resolved_anchors: Vec<TimeCode>,
    /// Signed project-frame movement for each anchor (`resolved - preferred`).
    pub signed_deltas: Vec<i64>,
    pub absolute_deltas: Vec<u64>,
    pub maximum_absolute_delta: u64,
    pub total_absolute_delta: u64,
}

/// How a music-fit plan uses the selected material.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MusicFitStrategy {
    /// Start on one detected beat and make a straight, real-time edit whose
    /// duration exactly matches the requested project range.
    BeatAnchoredStraightCut,
    /// Start on one detected beat, then choose that beat because the
    /// resulting real-time out point best satisfies an explicit source-end
    /// target. This remains a straight cut: it never loops or retimes music.
    EndAnchoredStraightCut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MusicDurationFit {
    ExactProjectRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MusicPlaybackMode {
    RealTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MusicRepeatMode {
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", content = "frames", rename_all = "snake_case")]
pub enum MusicEndBeatAlignment {
    Exact,
    /// Signed source-frame distance from the nearest detected beat to the
    /// selected out point (`source_out - beat`).
    Offset(i64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MusicBeatAnchor {
    pub source_frame: TimeCode,
    pub strength_basis_points: u16,
}

/// Caller-supplied source-end target for a fixed-duration music fit.
///
/// The planner may resolve the out point within `maximum_drift_frames`, but
/// never broadens that limit or silently falls back to a start-only fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MusicEndAnchor {
    pub preferred_source_end: TimeCode,
    pub maximum_drift_frames: TimeCode,
}

/// Inspectable evidence for an end-anchored music fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MusicEndAnchorEvidence {
    /// The caller's requested half-open source end boundary.
    pub target_source_end: TimeCode,
    /// The exact source end produced by the selected real-time range.
    pub resolved_source_end: TimeCode,
    /// Signed source-frame movement (`resolved_source_end - target_source_end`).
    pub signed_offset_frames: i64,
    /// The inclusive drift bound enforced while selecting the beat-anchored
    /// source start.
    pub maximum_drift_frames: TimeCode,
}

/// A transparent, executable music fit. It explicitly records the features
/// that were *not* used so callers cannot mistake a straight cut for looping
/// or time stretching.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MusicFitPlan {
    pub target_track: TrackId,
    pub asset: AssetId,
    pub timeline_range: Range<TimeCode>,
    pub source_range: Range<TimeCode>,
    pub anchor_beat: MusicBeatAnchor,
    pub strategy: MusicFitStrategy,
    pub duration_fit: MusicDurationFit,
    pub playback: MusicPlaybackMode,
    pub repeat: MusicRepeatMode,
    pub source_end_alignment: MusicEndBeatAlignment,
    /// Present only when the caller asked for an explicit source-end target.
    #[serde(default)]
    pub end_anchor: Option<MusicEndAnchorEvidence>,
    pub operations: Vec<Operation>,
}

/// The default meter used by callers that do not have a meter hypothesis.
///
/// This is a useful starting point for common music, not a claim that every
/// recording is in 4/4. Callers should expose the meter as an explicit input
/// when they have better evidence.
pub const MUSIC_STRUCTURE_DEFAULT_METER_BEATS: u8 = 4;
/// The default phrase length used by callers that do not have a phrase
/// hypothesis. It is intentionally a caller-level default rather than a
/// hidden assumption in [`music_structure_analysis`].
pub const MUSIC_STRUCTURE_DEFAULT_PHRASE_BARS: u8 = 4;
/// Upper bound accepted by the heuristic structure analyser for one meter.
pub const MUSIC_STRUCTURE_MAX_METER_BEATS: u8 = 32;
/// Upper bound accepted for one phrase length in bars.
pub const MUSIC_STRUCTURE_MAX_PHRASE_BARS: u8 = 32;

/// A heuristic role assigned to one eligible onset.
///
/// Every eligible onset is retained as a `Beat`. Onsets inferred to be bar or
/// phrase downbeats are promoted to the corresponding higher-level role. This
/// is deliberately a compact heuristic classification, not a claim of full
/// music-theory understanding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MusicStructureRole {
    Beat,
    Bar,
    Phrase,
}

/// Inferred structure parameters and their strength provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MusicStructureParameters {
    pub project_fps: crate::Rational,
    pub meter_beats: u8,
    pub phrase_bars: u8,
    /// Zero-based eligible-beat phase selected as the bar downbeat.
    pub bar_phase: u8,
    /// Zero-based bar phase selected as the phrase downbeat.
    pub phrase_phase: u8,
    pub estimated_bpm_milli: u32,
    pub bar_phase_strength: u64,
    pub total_beat_strength: u64,
    pub bar_phase_confidence_basis_points: u16,
    pub phrase_phase_strength: u64,
    pub total_bar_strength: u64,
    pub phrase_phase_confidence_basis_points: u16,
}

/// One exact, inspectable onset in the requested project range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MusicStructureCandidate {
    pub asset: AssetId,
    pub track: TrackId,
    pub clip: ClipId,
    pub source_frame: TimeCode,
    pub project_frame: TimeCode,
    /// Zero-based position in the sorted, deduplicated eligible onset list.
    pub beat_index: usize,
    pub beat_in_bar: u8,
    pub bar_in_phrase: u8,
    pub role: MusicStructureRole,
    /// Original onset strength from the beat detector.
    pub strength_basis_points: u16,
    /// For bar/phrase roles this is the phase-share confidence. Generic beats
    /// have no inferred higher-level phase and therefore report zero.
    pub confidence_basis_points: u16,
    pub estimated_bpm_milli: u32,
}

/// A deterministic, read-only heuristic analysis of musical structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MusicStructureAnalysis {
    pub music_asset: AssetId,
    pub timeline_range: Range<TimeCode>,
    pub minimum_strength_basis_points: u16,
    pub parameters: MusicStructureParameters,
    pub candidates: Vec<MusicStructureCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CreatorPlanError {
    #[error("clip {0} does not exist")]
    MissingClip(ClipId),
    #[error("clip {0} is not media")]
    NonMediaClip(ClipId),
    #[error("asset {0} does not exist")]
    MissingAsset(AssetId),
    #[error("target track {0} does not exist")]
    MissingTargetTrack(TrackId),
    #[error("target track {0} is not a video track")]
    MontageTargetNotVideo(TrackId),
    #[error("music asset {0} does not contain audio")]
    MontageMusicNotAudio(AssetId),
    #[error("a beat montage requires at least two ordered selects, got {0}")]
    TooFewMontageSelects(usize),
    #[error("montage select {index} uses non-video asset {asset}")]
    MontageSelectNotVideo { index: usize, asset: AssetId },
    #[error(
        "montage select {index} has invalid source envelope {start}..{end} for asset {asset} with duration {asset_duration}"
    )]
    InvalidMontageSourceEnvelope {
        index: usize,
        asset: AssetId,
        start: TimeCode,
        end: TimeCode,
        asset_duration: TimeCode,
    },
    #[error(
        "montage shot constraints must have non-negative minimum, positive maximum, and minimum <= maximum; got {minimum}..{maximum}"
    )]
    InvalidMontageShotConstraints {
        minimum: TimeCode,
        maximum: TimeCode,
    },
    #[error(
        "timeline duration {duration} cannot fit {shots} shots constrained to {minimum}..{maximum} frames"
    )]
    MontageDurationConstraintsUnsatisfied {
        shots: usize,
        duration: TimeCode,
        minimum: TimeCode,
        maximum: TimeCode,
    },
    #[error(
        "invalid beat montage cadence contract: minimum_duration_buckets={minimum_duration_buckets}, duration_bucket_frames={duration_bucket_frames}, maximum_similar_run={maximum_similar_run}, similar_tolerance_frames={similar_tolerance_frames}; require minimum buckets > 0, bucket frames > 0, maximum run > 0, and non-negative tolerance"
    )]
    InvalidMontageCadenceContract {
        minimum_duration_buckets: usize,
        duration_bucket_frames: TimeCode,
        maximum_similar_run: usize,
        similar_tolerance_frames: TimeCode,
    },
    #[error("beat montage cadence shot {index} has an invalid negative duration {duration}")]
    InvalidMontageCadenceDuration { index: usize, duration: TimeCode },
    #[error("beat montage cadence shot {index} has an invalid timeline range {start}..{end}")]
    InvalidMontageCadenceShotRange {
        index: usize,
        start: TimeCode,
        end: TimeCode,
    },
    #[error(
        "beat montage cadence failed: observed durations {observed_durations:?}, rounded buckets {observed_buckets:?} using {duration_bucket_frames} frames, longest similar run {observed_longest_similar_run}; requires at least {minimum_duration_buckets} distinct buckets and at most {maximum_similar_run} consecutive similar shots at tolerance {similar_tolerance_frames}; vary shot durations or choose different cut anchors"
    )]
    MontageCadenceContractUnsatisfied {
        minimum_duration_buckets: usize,
        duration_bucket_frames: TimeCode,
        maximum_similar_run: usize,
        similar_tolerance_frames: TimeCode,
        observed_durations: Vec<TimeCode>,
        observed_buckets: Vec<i64>,
        observed_longest_similar_run: usize,
    },
    #[error(
        "invalid beat montage anchor-repair settings: maximum_anchor_movement_frames={maximum_anchor_movement_frames:?}, locked_anchor_indices={locked_anchor_indices:?}, anchor_count={anchor_count}; movement must be non-negative and locked indices must be unique and in range"
    )]
    InvalidMontageAnchorRepairSettings {
        maximum_anchor_movement_frames: Option<TimeCode>,
        locked_anchor_indices: Vec<usize>,
        anchor_count: usize,
    },
    #[error(
        "beat montage needs {required} cut anchors but only {eligible} eligible music beats remain"
    )]
    InsufficientMontageBeats { required: usize, eligible: usize },
    #[error(
        "no set of {required} eligible beat anchors satisfies shot constraints {minimum}..{maximum}"
    )]
    MontageBeatConstraintsUnsatisfied {
        required: usize,
        minimum: TimeCode,
        maximum: TimeCode,
    },
    #[error(
        "montage select {index} envelope {start}..{end} for asset {asset} can supply at most {maximum_project_frames} project frames in real time, but needs {required_project_frames}; reassign this select to a shorter slot or select a larger source envelope"
    )]
    MontageSourceEnvelopeTooShort {
        index: usize,
        asset: AssetId,
        start: TimeCode,
        end: TimeCode,
        maximum_project_frames: TimeCode,
        required_project_frames: TimeCode,
    },
    #[error(
        "explicit beat montage requires {expected} cut anchors for {shots} shots, got {actual}"
    )]
    MontageExplicitAnchorCountMismatch {
        expected: usize,
        actual: usize,
        shots: usize,
    },
    #[error(
        "explicit montage anchor {index} at project frame {project_frame} is outside the interior of timeline range {start}..{end}"
    )]
    MontageExplicitAnchorOutsideRange {
        index: usize,
        project_frame: TimeCode,
        start: TimeCode,
        end: TimeCode,
    },
    #[error(
        "explicit montage anchor {index} at project frame {project_frame} is not strictly after prior anchor {previous}"
    )]
    MontageExplicitAnchorUnordered {
        index: usize,
        previous: TimeCode,
        project_frame: TimeCode,
    },
    #[error(
        "explicit montage anchor {index} at project frame {project_frame} is not an eligible beat for music asset {music_asset} at minimum strength {minimum_strength_basis_points}"
    )]
    MontageExplicitAnchorNotEligible {
        index: usize,
        music_asset: AssetId,
        project_frame: TimeCode,
        minimum_strength_basis_points: u16,
    },
    #[error(
        "explicit montage shot {shot_index} spans {start}..{end} ({duration} frames), outside constraints {minimum}..{maximum}"
    )]
    MontageExplicitShotDurationUnsatisfied {
        shot_index: usize,
        start: TimeCode,
        end: TimeCode,
        duration: TimeCode,
        minimum: TimeCode,
        maximum: TimeCode,
    },
    #[error("timeline range must be non-empty and non-negative: {start}..{end}")]
    InvalidTimelineRange { start: TimeCode, end: TimeCode },
    #[error(
        "requested range {start}..{end} is outside clip {clip}'s project range {clip_start}..{clip_end}"
    )]
    RangeOutsideClip {
        clip: ClipId,
        start: TimeCode,
        end: TimeCode,
        clip_start: TimeCode,
        clip_end: TimeCode,
    },
    #[error("minimum beat spacing cannot be negative: {0}")]
    NegativeMinimumSpacing(TimeCode),
    #[error("minimum beat strength must be at most 10000 basis points, got {0}")]
    InvalidMinimumStrength(u16),
    #[error("music structure meter must contain 1..={max} beats, got {value}")]
    InvalidMusicStructureMeter { value: u8, max: u8 },
    #[error("music structure phrase must contain 1..={max} bars, got {value}")]
    InvalidMusicStructurePhraseBars { value: u8, max: u8 },
    #[error("music structure asset {0} does not contain audio")]
    MusicStructureAssetNotAudio(AssetId),
    #[error("no eligible music structure beats occur for asset {asset} inside {start}..{end}")]
    NoEligibleMusicStructureBeats {
        asset: AssetId,
        start: TimeCode,
        end: TimeCode,
    },
    #[error("timeline beat analysis is still pending for assets {asset_ids:?}")]
    TimelineBeatAnalysisPending { asset_ids: Vec<AssetId> },
    #[error("timeline beat analysis is unavailable for assets {asset_ids:?}: {reason}")]
    TimelineBeatAnalysisUnavailable {
        asset_ids: Vec<AssetId>,
        reason: String,
    },
    #[error("no eligible timeline beats occur strictly inside {start}..{end}")]
    NoEligibleTimelineBeats { start: TimeCode, end: TimeCode },
    #[error("beat analysis has not been requested for asset {0}")]
    BeatAnalysisNotRequested(AssetId),
    #[error("beat analysis for asset {asset} is still {phase}")]
    BeatAnalysisPending { asset: AssetId, phase: &'static str },
    #[error("asset {0} has no audio stream to analyze")]
    NoAudio(AssetId),
    #[error("beat analysis for asset {0} was cancelled")]
    BeatAnalysisCancelled(AssetId),
    #[error("beat analysis for asset {asset} failed: {reason}")]
    BeatAnalysisFailed { asset: AssetId, reason: String },
    #[error("beat analysis belongs to asset {actual}, not requested asset {expected}")]
    BeatAnalysisAssetMismatch { expected: AssetId, actual: AssetId },
    #[error("no detected beat for asset {asset} meets the requested source and strength limits")]
    NoEligibleMusicBeat { asset: AssetId },
    #[error("preferred source frame {preferred} is outside asset {asset}'s range 0..{duration}")]
    PreferredSourceOutsideAsset {
        asset: AssetId,
        preferred: TimeCode,
        duration: TimeCode,
    },
    #[error(
        "preferred music source end {target} is outside asset {asset}'s half-open range 0..={duration}"
    )]
    PreferredMusicEndOutsideAsset {
        asset: AssetId,
        target: TimeCode,
        duration: TimeCode,
    },
    #[error("music end-anchor maximum drift cannot be negative: {0}")]
    NegativeMusicEndAnchorDrift(TimeCode),
    #[error(
        "no eligible beat in asset {asset} leaves enough real-time source for the target range"
    )]
    InsufficientMusicSource { asset: AssetId },
    #[error(
        "no eligible beat in asset {asset} can fill the target range and resolve within {maximum_drift_frames} source frames of requested end {target_source_end}"
    )]
    MusicEndAnchorUnsatisfied {
        asset: AssetId,
        target_source_end: TimeCode,
        maximum_drift_frames: TimeCode,
    },
    #[error("creator plan produced an invalid operation: {0}")]
    InvalidOperation(#[from] OpError),
}

/// Build a beat-aware cut plan for one existing timeline clip.
///
/// Beats may come from any audible timeline clip, such as a music bed beneath
/// the visual target. Duplicate onsets at the same project frame collapse to
/// the strongest deterministic representative. The returned operations are
/// safe to submit as one edit plan in the order supplied.
///
/// # Errors
///
/// Returns an explicit analysis-state error for incomplete/unavailable beats,
/// or a validation error when the target and requested range cannot be split.
#[allow(clippy::too_many_arguments)]
pub fn beat_pacing_plan(
    document: &Document,
    target_clip: ClipId,
    range: Option<Range<TimeCode>>,
    timeline_beats: &[TimelineBeat],
    analysis_state: &TimelineBeatAnalysisState,
    minimum_strength_basis_points: u16,
    minimum_spacing_frames: TimeCode,
) -> Result<BeatPacingPlan, CreatorPlanError> {
    require_complete_timeline_beats(analysis_state)?;
    if minimum_strength_basis_points > 10_000 {
        return Err(CreatorPlanError::InvalidMinimumStrength(
            minimum_strength_basis_points,
        ));
    }
    if minimum_spacing_frames < TimeCode::ZERO {
        return Err(CreatorPlanError::NegativeMinimumSpacing(
            minimum_spacing_frames,
        ));
    }

    let clip = document
        .clip(target_clip)
        .ok_or(CreatorPlanError::MissingClip(target_clip))?;
    if !clip.content.is_media() {
        return Err(CreatorPlanError::NonMediaClip(target_clip));
    }
    let clip_end = clip
        .timeline_start
        .checked_add(document.clip_duration(clip)?)
        .ok_or(OpError::TimeOverflow)?;
    let range = range.unwrap_or(clip.timeline_start..clip_end);
    validate_timeline_range(&range)?;
    if range.start < clip.timeline_start || range.end > clip_end {
        return Err(CreatorPlanError::RangeOutsideClip {
            clip: target_clip,
            start: range.start,
            end: range.end,
            clip_start: clip.timeline_start,
            clip_end,
        });
    }

    let mut candidates = timeline_beats
        .iter()
        .copied()
        .filter(|beat| {
            beat.project_frame > range.start
                && beat.project_frame < range.end
                && beat.strength_basis_points >= minimum_strength_basis_points
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|beat| {
        (
            beat.project_frame,
            Reverse(beat.strength_basis_points),
            beat.track,
            beat.clip,
            beat.asset,
            beat.source_frame,
        )
    });
    candidates.dedup_by_key(|beat| beat.project_frame);

    let mut selected_beats = Vec::new();
    for beat in candidates {
        let sufficiently_spaced = selected_beats.last().is_none_or(|previous: &TimelineBeat| {
            beat.project_frame.0 - previous.project_frame.0 >= minimum_spacing_frames.0
        });
        if sufficiently_spaced {
            selected_beats.push(beat);
        }
    }
    if selected_beats.is_empty() {
        return Err(CreatorPlanError::NoEligibleTimelineBeats {
            start: range.start,
            end: range.end,
        });
    }

    let operations = selected_beats
        .iter()
        .rev()
        .map(|beat| Operation::SplitClip {
            clip: target_clip,
            at: beat.project_frame,
        })
        .collect::<Vec<_>>();
    validate_operations(document, &operations)?;

    Ok(BeatPacingPlan {
        target_clip,
        range,
        minimum_strength_basis_points,
        minimum_spacing_frames,
        selected_beats: selected_beats
            .into_iter()
            .map(BeatPacingPoint::from)
            .collect(),
        operations,
    })
}

/// Infer a compact musical hierarchy from one asset's mapped timeline beats.
///
/// The result is deliberately heuristic. It preserves every eligible onset,
/// labels the inferred bar and phrase downbeats, and records the aggregate
/// strength behind each phase choice so an agent can inspect or reject the
/// hypothesis. Beats are sorted by project frame and duplicate project-frame
/// onsets collapse to the strongest deterministic representative.
///
/// `meter_beats` and `phrase_bars` are explicit inputs. Callers may use
/// [`MUSIC_STRUCTURE_DEFAULT_METER_BEATS`] and
/// [`MUSIC_STRUCTURE_DEFAULT_PHRASE_BARS`] as starting values, but this
/// function never silently assumes them.
///
/// No timeline operations are produced or applied.
///
/// # Errors
///
/// Returns an explicit lifecycle error for pending or unavailable beat
/// analysis, or a validation error for the requested asset, range, strength,
/// meter, phrase length, or absence of eligible onsets.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
pub fn music_structure_analysis(
    document: &Document,
    music_asset: AssetId,
    timeline_range: Range<TimeCode>,
    timeline_beats: &[TimelineBeat],
    analysis_state: &TimelineBeatAnalysisState,
    minimum_strength_basis_points: u16,
    meter_beats: u8,
    phrase_bars: u8,
) -> Result<MusicStructureAnalysis, CreatorPlanError> {
    require_complete_timeline_beats(analysis_state)?;
    validate_timeline_range(&timeline_range)?;
    if minimum_strength_basis_points > 10_000 {
        return Err(CreatorPlanError::InvalidMinimumStrength(
            minimum_strength_basis_points,
        ));
    }
    if !(1..=MUSIC_STRUCTURE_MAX_METER_BEATS).contains(&meter_beats) {
        return Err(CreatorPlanError::InvalidMusicStructureMeter {
            value: meter_beats,
            max: MUSIC_STRUCTURE_MAX_METER_BEATS,
        });
    }
    if !(1..=MUSIC_STRUCTURE_MAX_PHRASE_BARS).contains(&phrase_bars) {
        return Err(CreatorPlanError::InvalidMusicStructurePhraseBars {
            value: phrase_bars,
            max: MUSIC_STRUCTURE_MAX_PHRASE_BARS,
        });
    }

    let media = document
        .asset(music_asset)
        .ok_or(CreatorPlanError::MissingAsset(music_asset))?;
    if !media.kind.supports(TrackKind::Audio) {
        return Err(CreatorPlanError::MusicStructureAssetNotAudio(music_asset));
    }

    let eligible = music_structure_beat_candidates(
        timeline_beats,
        music_asset,
        &timeline_range,
        media.duration,
        minimum_strength_basis_points,
    );
    if eligible.is_empty() {
        return Err(CreatorPlanError::NoEligibleMusicStructureBeats {
            asset: music_asset,
            start: timeline_range.start,
            end: timeline_range.end,
        });
    }

    let bar_phase_strengths = phase_strengths(&eligible, usize::from(meter_beats));
    let (bar_phase, bar_phase_strength) = strongest_phase(&bar_phase_strengths);
    let total_beat_strength = bar_phase_strengths.iter().sum::<u64>();
    let bar_phase_confidence_basis_points =
        phase_confidence(bar_phase_strength, total_beat_strength);

    let bar_candidates = eligible
        .iter()
        .enumerate()
        .filter(|(index, _)| index % usize::from(meter_beats) == bar_phase)
        .collect::<Vec<_>>();
    let phrase_phase_strengths = (0..usize::from(phrase_bars))
        .map(|phase| {
            bar_candidates
                .iter()
                .filter(|(index, _)| {
                    music_bar_index(*index, bar_phase, usize::from(meter_beats))
                        .rem_euclid(i64::from(phrase_bars))
                        == i64::try_from(phase).unwrap_or(i64::MAX)
                })
                .map(|(_, beat)| u64::from(beat.strength_basis_points))
                .sum::<u64>()
        })
        .collect::<Vec<_>>();
    let (phrase_phase, phrase_phase_strength) = strongest_phase(&phrase_phase_strengths);
    let total_bar_strength = phrase_phase_strengths.iter().sum::<u64>();
    let phrase_phase_confidence_basis_points =
        phase_confidence(phrase_phase_strength, total_bar_strength);

    let estimated_bpms = estimated_bpm_values(&eligible, document.fps);
    let parameters = MusicStructureParameters {
        project_fps: document.fps,
        meter_beats,
        phrase_bars,
        bar_phase: u8::try_from(bar_phase).unwrap_or(u8::MAX),
        phrase_phase: u8::try_from(phrase_phase).unwrap_or(u8::MAX),
        estimated_bpm_milli: median_u32(&estimated_bpms),
        bar_phase_strength,
        total_beat_strength,
        bar_phase_confidence_basis_points,
        phrase_phase_strength,
        total_bar_strength,
        phrase_phase_confidence_basis_points,
    };

    let candidates = eligible
        .iter()
        .enumerate()
        .map(|(beat_index, beat)| {
            let meter_beats = usize::from(meter_beats);
            let beat_in_bar =
                u8::try_from((beat_index % meter_beats + meter_beats - bar_phase) % meter_beats)
                    .unwrap_or(u8::MAX);
            let bar_in_phrase = u8::try_from(
                (music_bar_index(beat_index, bar_phase, meter_beats)
                    - i64::try_from(phrase_phase).unwrap_or(i64::MAX))
                .rem_euclid(i64::from(phrase_bars)),
            )
            .unwrap_or(u8::MAX);
            let role = if beat_in_bar == 0 && bar_in_phrase == 0 {
                MusicStructureRole::Phrase
            } else if beat_in_bar == 0 {
                MusicStructureRole::Bar
            } else {
                MusicStructureRole::Beat
            };
            let confidence_basis_points = match role {
                MusicStructureRole::Beat => 0,
                MusicStructureRole::Bar => bar_phase_confidence_basis_points,
                MusicStructureRole::Phrase => phrase_phase_confidence_basis_points,
            };
            MusicStructureCandidate {
                asset: beat.asset,
                track: beat.track,
                clip: beat.clip,
                source_frame: beat.source_frame,
                project_frame: beat.project_frame,
                beat_index,
                beat_in_bar,
                bar_in_phrase,
                role,
                strength_basis_points: beat.strength_basis_points,
                confidence_basis_points,
                estimated_bpm_milli: estimated_bpms[beat_index],
            }
        })
        .collect();

    Ok(MusicStructureAnalysis {
        music_asset,
        timeline_range,
        minimum_strength_basis_points,
        parameters,
        candidates,
    })
}

/// Build a deterministic hard-cut montage from ordered source selects.
///
/// Eligible cut anchors are strong-enough beats from `music_asset` strictly
/// inside `timeline_range`. The planner chooses exactly one fewer anchors than
/// shots. It first minimizes aggregate distance from evenly spaced boundaries,
/// then maximizes aggregate beat strength, then chooses the lexicographically
/// earliest boundary frames. No semantic selection, transition, or retiming is
/// introduced.
///
/// # Errors
///
/// Returns explicit errors for incomplete beat analysis, incompatible tracks
/// or assets, invalid source envelopes, infeasible cut constraints, and edits
/// that cannot be represented exactly at the project frame rate.
#[allow(clippy::too_many_arguments)]
pub fn beat_montage_plan(
    document: &Document,
    target_track: TrackId,
    music_asset: AssetId,
    timeline_range: Range<TimeCode>,
    selects: &[BeatMontageSelect],
    timeline_beats: &[TimelineBeat],
    analysis_state: &TimelineBeatAnalysisState,
    minimum_strength_basis_points: u16,
    minimum_shot_frames: TimeCode,
    maximum_shot_frames: TimeCode,
    mode: ThreePointMode,
) -> Result<BeatMontagePlan, CreatorPlanError> {
    validate_beat_montage_inputs(
        document,
        target_track,
        music_asset,
        &timeline_range,
        selects,
        analysis_state,
        minimum_strength_basis_points,
        minimum_shot_frames,
        maximum_shot_frames,
    )?;
    let candidates = montage_beat_candidates(
        timeline_beats,
        music_asset,
        &timeline_range,
        minimum_strength_basis_points,
    );
    let selected_beats = choose_montage_beats(
        document,
        selects,
        &candidates,
        &timeline_range,
        minimum_shot_frames,
        maximum_shot_frames,
    )?;

    let mut boundaries = Vec::with_capacity(selects.len() + 1);
    boundaries.push(timeline_range.start);
    boundaries.extend(selected_beats.iter().map(|beat| beat.project_frame));
    boundaries.push(timeline_range.end);
    let (shots, operations) =
        resolve_montage_shots(document, target_track, selects, &boundaries, mode)?;
    validate_operations(document, &operations)?;

    Ok(BeatMontagePlan {
        target_track,
        music_asset,
        timeline_range,
        minimum_strength_basis_points,
        minimum_shot_frames,
        maximum_shot_frames,
        mode,
        shots,
        cut_anchors: selected_beats
            .into_iter()
            .enumerate()
            .map(|(after_shot_index, beat)| BeatMontageCutAnchor {
                after_shot_index,
                beat: BeatPacingPoint::from(beat),
            })
            .collect(),
        operations,
    })
}

/// Build a hard-cut montage while preserving caller-selected beat anchors.
///
/// Unlike [`beat_montage_plan`], this entry point never chooses, moves, or
/// regularizes a cut. `anchors` contains the exact project-frame boundaries
/// requested by the caller. Each frame must identify one eligible beat from
/// `music_asset`; the returned plan carries the canonical [`TimelineBeat`]
/// metadata for that frame while retaining the requested frame unchanged.
/// Selects remain in their supplied order and are resolved as real-time,
/// source-envelope-bounded edits.
///
/// # Errors
///
/// Returns explicit errors for an anchor-count mismatch, anchors outside the
/// requested range, unordered anchors, frames that do not identify an
/// eligible beat, shot durations outside the requested constraints, source
/// envelopes that cannot fill a shot at mixed frame rates, and operations that
/// fail normal document validation.
#[allow(clippy::too_many_arguments)]
pub fn beat_montage_plan_with_anchors(
    document: &Document,
    target_track: TrackId,
    music_asset: AssetId,
    timeline_range: Range<TimeCode>,
    selects: &[BeatMontageSelect],
    anchors: &[TimeCode],
    timeline_beats: &[TimelineBeat],
    analysis_state: &TimelineBeatAnalysisState,
    minimum_strength_basis_points: u16,
    minimum_shot_frames: TimeCode,
    maximum_shot_frames: TimeCode,
    mode: ThreePointMode,
) -> Result<BeatMontagePlan, CreatorPlanError> {
    validate_beat_montage_inputs(
        document,
        target_track,
        music_asset,
        &timeline_range,
        selects,
        analysis_state,
        minimum_strength_basis_points,
        minimum_shot_frames,
        maximum_shot_frames,
    )?;
    let expected_anchors = selects.len() - 1;
    if anchors.len() != expected_anchors {
        return Err(CreatorPlanError::MontageExplicitAnchorCountMismatch {
            expected: expected_anchors,
            actual: anchors.len(),
            shots: selects.len(),
        });
    }
    let candidates = montage_beat_candidates(
        timeline_beats,
        music_asset,
        &timeline_range,
        minimum_strength_basis_points,
    );
    let selected_beats = validate_explicit_montage_anchors(
        anchors,
        &candidates,
        music_asset,
        &timeline_range,
        minimum_strength_basis_points,
    )?;

    let mut boundaries = Vec::with_capacity(selects.len() + 1);
    boundaries.push(timeline_range.start);
    boundaries.extend(anchors.iter().copied());
    boundaries.push(timeline_range.end);
    validate_explicit_montage_durations(&boundaries, minimum_shot_frames, maximum_shot_frames)?;
    let (shots, operations) =
        resolve_montage_shots(document, target_track, selects, &boundaries, mode)?;
    validate_operations(document, &operations)?;

    Ok(BeatMontagePlan {
        target_track,
        music_asset,
        timeline_range,
        minimum_strength_basis_points,
        minimum_shot_frames,
        maximum_shot_frames,
        mode,
        shots,
        cut_anchors: selected_beats
            .into_iter()
            .enumerate()
            .map(|(after_shot_index, beat)| BeatMontageCutAnchor {
                after_shot_index,
                beat: BeatPacingPoint::from(beat),
            })
            .collect(),
        operations,
    })
}

impl BeatMontageCadenceContract {
    /// Compute cadence evidence without requiring the contract to pass.
    ///
    /// This validates the contract settings and input durations, then returns
    /// the rounded duration buckets and longest similar run for inspection.
    ///
    /// # Errors
    ///
    /// Returns an error when the contract settings are invalid or a duration
    /// is negative.
    pub fn summarize(
        &self,
        durations: &[TimeCode],
    ) -> Result<BeatMontageCadenceSummary, CreatorPlanError> {
        validate_cadence_contract_settings(self)?;
        for (index, duration) in durations.iter().copied().enumerate() {
            if duration < TimeCode::ZERO {
                return Err(CreatorPlanError::InvalidMontageCadenceDuration { index, duration });
            }
        }

        let rounded_buckets = durations
            .iter()
            .map(|duration| {
                duration
                    .0
                    .saturating_add(self.duration_bucket_frames.0 / 2)
                    .div_euclid(self.duration_bucket_frames.0)
            })
            .collect::<Vec<_>>();
        let mut distinct_buckets = rounded_buckets.clone();
        distinct_buckets.sort_unstable();
        distinct_buckets.dedup();

        let mut current_run = usize::from(!durations.is_empty());
        let mut longest_similar_run = current_run;
        let tolerance = self.similar_tolerance_frames.0.unsigned_abs();
        for pair in durations.windows(2) {
            if pair[0].0.abs_diff(pair[1].0) <= tolerance {
                current_run += 1;
                longest_similar_run = longest_similar_run.max(current_run);
            } else {
                current_run = 1;
            }
        }

        Ok(BeatMontageCadenceSummary {
            durations: durations.to_vec(),
            rounded_buckets,
            distinct_buckets,
            longest_similar_run,
        })
    }

    /// Validate cadence and return the computed evidence on success.
    ///
    /// # Errors
    ///
    /// Returns an error when the contract settings are invalid, a duration is
    /// negative, or the observed cadence does not satisfy the contract.
    pub fn validate(
        &self,
        durations: &[TimeCode],
    ) -> Result<BeatMontageCadenceSummary, CreatorPlanError> {
        let summary = self.summarize(durations)?;
        if summary.distinct_buckets.len() < self.minimum_duration_buckets
            || summary.longest_similar_run > self.maximum_similar_run
        {
            return Err(CreatorPlanError::MontageCadenceContractUnsatisfied {
                minimum_duration_buckets: self.minimum_duration_buckets,
                duration_bucket_frames: self.duration_bucket_frames,
                maximum_similar_run: self.maximum_similar_run,
                similar_tolerance_frames: self.similar_tolerance_frames,
                observed_durations: summary.durations,
                observed_buckets: summary.distinct_buckets,
                observed_longest_similar_run: summary.longest_similar_run,
            });
        }
        Ok(summary)
    }
}

/// Validate project-frame shot durations against a beat-montage cadence
/// contract and return inspectable evidence on success.
///
/// # Errors
///
/// Returns an error when the contract settings are invalid, a duration is
/// negative, or the observed cadence does not satisfy the contract.
pub fn validate_beat_montage_cadence(
    durations: &[TimeCode],
    contract: BeatMontageCadenceContract,
) -> Result<BeatMontageCadenceSummary, CreatorPlanError> {
    contract.validate(durations)
}

/// Compute and validate cadence directly from a resolved montage plan.
///
/// # Errors
///
/// Returns an error when a shot range is invalid, the contract settings are
/// invalid, or the observed cadence does not satisfy the contract.
pub fn validate_beat_montage_plan_cadence(
    plan: &BeatMontagePlan,
    contract: BeatMontageCadenceContract,
) -> Result<BeatMontageCadenceSummary, CreatorPlanError> {
    let durations = plan
        .shots
        .iter()
        .enumerate()
        .map(|(index, shot)| {
            shot.timeline_range
                .end
                .checked_sub(shot.timeline_range.start)
                .ok_or(CreatorPlanError::InvalidMontageCadenceShotRange {
                    index,
                    start: shot.timeline_range.start,
                    end: shot.timeline_range.end,
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    contract.validate(&durations)
}

/// Build a beat montage by repairing preferred anchor frames to the nearest
/// globally feasible detected-beat schedule.
///
/// This is explicitly opt-in. The ordinary explicit-anchor planner remains
/// strict and never moves caller-selected anchors. Selects stay in their
/// supplied order and source envelopes remain hard limits. When supplied,
/// `locked_anchor_indices` keeps those preferred anchor ordinals exact and
/// `maximum_anchor_movement_frames` bounds every repair.
///
/// # Errors
///
/// Returns the normal montage planning errors when no ordered schedule can
/// satisfy beat eligibility, shot durations, source envelopes, movement
/// limits, or the optional cadence contract.
#[allow(clippy::too_many_arguments)]
pub fn beat_montage_plan_near_anchors(
    document: &Document,
    target_track: TrackId,
    music_asset: AssetId,
    timeline_range: Range<TimeCode>,
    selects: &[BeatMontageSelect],
    preferred_anchors: &[TimeCode],
    timeline_beats: &[TimelineBeat],
    analysis_state: &TimelineBeatAnalysisState,
    minimum_strength_basis_points: u16,
    minimum_shot_frames: TimeCode,
    maximum_shot_frames: TimeCode,
    mode: ThreePointMode,
    maximum_anchor_movement_frames: Option<TimeCode>,
    locked_anchor_indices: &[usize],
    cadence_contract: Option<BeatMontageCadenceContract>,
) -> Result<BeatMontagePlan, CreatorPlanError> {
    beat_montage_plan_near_anchors_with_report(
        document,
        target_track,
        music_asset,
        timeline_range,
        selects,
        preferred_anchors,
        timeline_beats,
        analysis_state,
        minimum_strength_basis_points,
        minimum_shot_frames,
        maximum_shot_frames,
        mode,
        maximum_anchor_movement_frames,
        locked_anchor_indices,
        cadence_contract,
    )
    .map(|(plan, _repair)| plan)
}

/// Build a repaired montage and return requested/resolved anchor movement
/// evidence alongside the normal executable plan.
///
/// # Errors
///
/// Returns the normal montage planning errors when no ordered schedule can
/// satisfy beat eligibility, shot durations, source envelopes, movement
/// limits, or the optional cadence contract.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
pub fn beat_montage_plan_near_anchors_with_report(
    document: &Document,
    target_track: TrackId,
    music_asset: AssetId,
    timeline_range: Range<TimeCode>,
    selects: &[BeatMontageSelect],
    preferred_anchors: &[TimeCode],
    timeline_beats: &[TimelineBeat],
    analysis_state: &TimelineBeatAnalysisState,
    minimum_strength_basis_points: u16,
    minimum_shot_frames: TimeCode,
    maximum_shot_frames: TimeCode,
    mode: ThreePointMode,
    maximum_anchor_movement_frames: Option<TimeCode>,
    locked_anchor_indices: &[usize],
    cadence_contract: Option<BeatMontageCadenceContract>,
) -> Result<(BeatMontagePlan, BeatMontageAnchorRepair), CreatorPlanError> {
    validate_beat_montage_inputs(
        document,
        target_track,
        music_asset,
        &timeline_range,
        selects,
        analysis_state,
        minimum_strength_basis_points,
        minimum_shot_frames,
        maximum_shot_frames,
    )?;
    let required_anchors = selects.len() - 1;
    validate_near_anchor_preferences(
        preferred_anchors,
        required_anchors,
        &timeline_range,
        maximum_anchor_movement_frames,
        locked_anchor_indices,
    )?;
    if let Some(contract) = cadence_contract.as_ref() {
        validate_cadence_contract_settings(contract)?;
    }

    let candidates = montage_beat_candidates(
        timeline_beats,
        music_asset,
        &timeline_range,
        minimum_strength_basis_points,
    );
    if candidates.len() < required_anchors {
        return Err(CreatorPlanError::InsufficientMontageBeats {
            required: required_anchors,
            eligible: candidates.len(),
        });
    }
    for &index in locked_anchor_indices {
        if !candidates
            .iter()
            .any(|beat| beat.project_frame == preferred_anchors[index])
        {
            return Err(CreatorPlanError::MontageExplicitAnchorNotEligible {
                index,
                music_asset,
                project_frame: preferred_anchors[index],
                minimum_strength_basis_points,
            });
        }
    }

    let source_aware = select_montage_boundaries_near_anchors(
        &candidates,
        &timeline_range,
        selects.len(),
        minimum_shot_frames,
        maximum_shot_frames,
        preferred_anchors,
        maximum_anchor_movement_frames,
        locked_anchor_indices,
        cadence_contract.as_ref(),
        |shot_index, duration| montage_select_can_fill(document, &selects[shot_index], duration),
    );
    let path = source_aware
        .or_else(|| {
            cadence_contract.as_ref().and_then(|_| {
                select_montage_boundaries_near_anchors(
                    &candidates,
                    &timeline_range,
                    selects.len(),
                    minimum_shot_frames,
                    maximum_shot_frames,
                    preferred_anchors,
                    maximum_anchor_movement_frames,
                    locked_anchor_indices,
                    None,
                    |shot_index, duration| {
                        montage_select_can_fill(document, &selects[shot_index], duration)
                    },
                )
            })
        })
        .or_else(|| {
            select_montage_boundaries_near_anchors(
                &candidates,
                &timeline_range,
                selects.len(),
                minimum_shot_frames,
                maximum_shot_frames,
                preferred_anchors,
                maximum_anchor_movement_frames,
                locked_anchor_indices,
                cadence_contract.as_ref(),
                |_, _| true,
            )
        })
        .or_else(|| {
            cadence_contract.as_ref().and_then(|_| {
                select_montage_boundaries_near_anchors(
                    &candidates,
                    &timeline_range,
                    selects.len(),
                    minimum_shot_frames,
                    maximum_shot_frames,
                    preferred_anchors,
                    maximum_anchor_movement_frames,
                    locked_anchor_indices,
                    None,
                    |_, _| true,
                )
            })
        })
        .ok_or(CreatorPlanError::MontageBeatConstraintsUnsatisfied {
            required: required_anchors,
            minimum: minimum_shot_frames,
            maximum: maximum_shot_frames,
        })?;
    let selected_beats = path
        .candidate_indices
        .iter()
        .map(|&index| candidates[index])
        .collect::<Vec<_>>();
    let mut boundaries = Vec::with_capacity(selects.len() + 1);
    boundaries.push(timeline_range.start);
    boundaries.extend(selected_beats.iter().map(|beat| beat.project_frame));
    boundaries.push(timeline_range.end);
    let (shots, operations) =
        resolve_montage_shots(document, target_track, selects, &boundaries, mode)?;
    if let Some(contract) = cadence_contract {
        let durations = boundaries
            .windows(2)
            .map(|pair| pair[1].checked_sub(pair[0]).ok_or(OpError::TimeOverflow))
            .collect::<Result<Vec<_>, _>>()?;
        contract.validate(&durations)?;
    }
    validate_operations(document, &operations)?;

    let plan = BeatMontagePlan {
        target_track,
        music_asset,
        timeline_range,
        minimum_strength_basis_points,
        minimum_shot_frames,
        maximum_shot_frames,
        mode,
        shots,
        cut_anchors: selected_beats
            .iter()
            .enumerate()
            .map(|(after_shot_index, beat)| BeatMontageCutAnchor {
                after_shot_index,
                beat: BeatPacingPoint::from(*beat),
            })
            .collect(),
        operations,
    };
    let repair = anchor_repair_report(preferred_anchors, &selected_beats);
    Ok((plan, repair))
}

fn validate_near_anchor_preferences(
    preferred_anchors: &[TimeCode],
    required_anchors: usize,
    timeline_range: &Range<TimeCode>,
    maximum_anchor_movement_frames: Option<TimeCode>,
    locked_anchor_indices: &[usize],
) -> Result<(), CreatorPlanError> {
    if preferred_anchors.len() != required_anchors {
        return Err(CreatorPlanError::MontageExplicitAnchorCountMismatch {
            expected: required_anchors,
            actual: preferred_anchors.len(),
            shots: required_anchors + 1,
        });
    }
    let mut sorted_locked_anchor_indices = locked_anchor_indices.to_vec();
    sorted_locked_anchor_indices.sort_unstable();
    if maximum_anchor_movement_frames.is_some_and(|value| value < TimeCode::ZERO)
        || sorted_locked_anchor_indices
            .windows(2)
            .any(|pair| pair[0] == pair[1])
        || locked_anchor_indices
            .iter()
            .any(|&index| index >= required_anchors)
    {
        return Err(CreatorPlanError::InvalidMontageAnchorRepairSettings {
            maximum_anchor_movement_frames,
            locked_anchor_indices: locked_anchor_indices.to_vec(),
            anchor_count: required_anchors,
        });
    }
    for (index, &project_frame) in preferred_anchors.iter().enumerate() {
        if project_frame <= timeline_range.start || project_frame >= timeline_range.end {
            return Err(CreatorPlanError::MontageExplicitAnchorOutsideRange {
                index,
                project_frame,
                start: timeline_range.start,
                end: timeline_range.end,
            });
        }
        if index > 0 {
            let previous = preferred_anchors[index - 1];
            if project_frame <= previous {
                return Err(CreatorPlanError::MontageExplicitAnchorUnordered {
                    index,
                    previous,
                    project_frame,
                });
            }
        }
    }
    Ok(())
}

fn anchor_repair_report(
    preferred_anchors: &[TimeCode],
    selected_beats: &[TimelineBeat],
) -> BeatMontageAnchorRepair {
    let resolved_anchors = selected_beats
        .iter()
        .map(|beat| beat.project_frame)
        .collect::<Vec<_>>();
    let signed_deltas = resolved_anchors
        .iter()
        .zip(preferred_anchors)
        .map(|(resolved, preferred)| resolved.0.saturating_sub(preferred.0))
        .collect::<Vec<_>>();
    let absolute_deltas = signed_deltas
        .iter()
        .map(|delta| delta.unsigned_abs())
        .collect::<Vec<_>>();
    BeatMontageAnchorRepair {
        preferred_anchors: preferred_anchors.to_vec(),
        resolved_anchors,
        signed_deltas,
        maximum_absolute_delta: absolute_deltas.iter().copied().max().unwrap_or(0),
        total_absolute_delta: absolute_deltas.iter().copied().fold(0, u64::saturating_add),
        absolute_deltas,
    }
}

fn validate_cadence_contract_settings(
    contract: &BeatMontageCadenceContract,
) -> Result<(), CreatorPlanError> {
    if contract.minimum_duration_buckets == 0
        || contract.duration_bucket_frames <= TimeCode::ZERO
        || contract.maximum_similar_run == 0
        || contract.similar_tolerance_frames < TimeCode::ZERO
    {
        return Err(CreatorPlanError::InvalidMontageCadenceContract {
            minimum_duration_buckets: contract.minimum_duration_buckets,
            duration_bucket_frames: contract.duration_bucket_frames,
            maximum_similar_run: contract.maximum_similar_run,
            similar_tolerance_frames: contract.similar_tolerance_frames,
        });
    }
    Ok(())
}

/// Fit a straight section of music exactly into one project range, starting
/// on the eligible detected beat nearest `preferred_source_start`.
///
/// This deliberately does not promise a beat-aligned out point, looping, or
/// time stretching. Those facts are carried in the returned plan. Candidates
/// without enough remaining source are skipped rather than silently retimed.
///
/// # Errors
///
/// Returns explicit lifecycle errors until beat analysis is ready, and a
/// validation error when no beat can fill the target at real time.
#[allow(clippy::too_many_arguments)]
pub fn music_fit_plan(
    document: &Document,
    target_track: TrackId,
    asset: AssetId,
    timeline_range: Range<TimeCode>,
    preferred_source_start: Option<TimeCode>,
    beat_status: &BeatStatus,
    minimum_strength_basis_points: u16,
    mode: ThreePointMode,
) -> Result<MusicFitPlan, CreatorPlanError> {
    music_fit_plan_with_end_anchor(
        document,
        target_track,
        asset,
        timeline_range,
        preferred_source_start,
        None,
        beat_status,
        minimum_strength_basis_points,
        mode,
    )
}

/// Fit a straight, fixed-duration section of music while targeting its source
/// out point within an explicit bounded drift.
///
/// Eligible source starts remain detected beats. The planner evaluates each
/// real-time source range that can fill `timeline_range`, then selects by
/// endpoint distance first, preferred-start distance second, beat strength
/// third, and source position last. It fails closed when no eligible start can
/// satisfy `end_anchor.maximum_drift_frames`.
///
/// Pass `None` for `end_anchor` to retain the start-only behavior exposed by
/// [`music_fit_plan`].
///
/// # Errors
///
/// Returns an error when the target range, media, track, beat analysis, or
/// endpoint contract is invalid or cannot be satisfied without retiming.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn music_fit_plan_with_end_anchor(
    document: &Document,
    target_track: TrackId,
    asset: AssetId,
    timeline_range: Range<TimeCode>,
    preferred_source_start: Option<TimeCode>,
    end_anchor: Option<MusicEndAnchor>,
    beat_status: &BeatStatus,
    minimum_strength_basis_points: u16,
    mode: ThreePointMode,
) -> Result<MusicFitPlan, CreatorPlanError> {
    validate_timeline_range(&timeline_range)?;
    if minimum_strength_basis_points > 10_000 {
        return Err(CreatorPlanError::InvalidMinimumStrength(
            minimum_strength_basis_points,
        ));
    }
    let media = document
        .asset(asset)
        .ok_or(CreatorPlanError::MissingAsset(asset))?;
    let beats = ready_asset_beats(asset, beat_status)?;
    if beats.asset != asset {
        return Err(CreatorPlanError::BeatAnalysisAssetMismatch {
            expected: asset,
            actual: beats.asset,
        });
    }
    let preferred = preferred_source_start.unwrap_or(TimeCode::ZERO);
    if preferred < TimeCode::ZERO || preferred >= media.duration {
        return Err(CreatorPlanError::PreferredSourceOutsideAsset {
            asset,
            preferred,
            duration: media.duration,
        });
    }
    if let Some(end_anchor) = end_anchor {
        if end_anchor.preferred_source_end < TimeCode::ZERO
            || end_anchor.preferred_source_end > media.duration
        {
            return Err(CreatorPlanError::PreferredMusicEndOutsideAsset {
                asset,
                target: end_anchor.preferred_source_end,
                duration: media.duration,
            });
        }
        if end_anchor.maximum_drift_frames < TimeCode::ZERO {
            return Err(CreatorPlanError::NegativeMusicEndAnchorDrift(
                end_anchor.maximum_drift_frames,
            ));
        }
    }

    let project_duration = timeline_range
        .end
        .checked_sub(timeline_range.start)
        .ok_or(OpError::TimeOverflow)?;
    let (anchor_beat, source_out) = match end_anchor {
        Some(end_anchor) => select_end_anchored_music_source(
            media,
            beats,
            preferred,
            end_anchor,
            minimum_strength_basis_points,
            document.fps,
            project_duration,
        )?,
        None => select_music_source(
            media,
            beats,
            preferred,
            minimum_strength_basis_points,
            document.fps,
            project_duration,
        )?,
    };
    let source_range = anchor_beat.source_frame..source_out;
    let source_end_alignment = music_end_alignment(beats, media.duration, source_out);
    let operations = vec![Operation::ThreePointEdit {
        track: target_track,
        asset,
        source_in: Some(anchor_beat.source_frame),
        source_out: None,
        timeline_in: Some(timeline_range.start),
        timeline_out: Some(timeline_range.end),
        mode,
    }];
    validate_operations(document, &operations)?;

    Ok(MusicFitPlan {
        target_track,
        asset,
        timeline_range,
        source_range,
        anchor_beat: MusicBeatAnchor {
            source_frame: anchor_beat.source_frame,
            strength_basis_points: anchor_beat.strength_basis_points,
        },
        strategy: if end_anchor.is_some() {
            MusicFitStrategy::EndAnchoredStraightCut
        } else {
            MusicFitStrategy::BeatAnchoredStraightCut
        },
        duration_fit: MusicDurationFit::ExactProjectRange,
        playback: MusicPlaybackMode::RealTime,
        repeat: MusicRepeatMode::None,
        source_end_alignment,
        end_anchor: end_anchor.map(|end_anchor| MusicEndAnchorEvidence {
            target_source_end: end_anchor.preferred_source_end,
            resolved_source_end: source_out,
            signed_offset_frames: source_out
                .0
                .saturating_sub(end_anchor.preferred_source_end.0),
            maximum_drift_frames: end_anchor.maximum_drift_frames,
        }),
        operations,
    })
}

fn require_complete_timeline_beats(
    state: &TimelineBeatAnalysisState,
) -> Result<(), CreatorPlanError> {
    match state {
        TimelineBeatAnalysisState::Ready => Ok(()),
        TimelineBeatAnalysisState::Pending { asset_ids } => {
            Err(CreatorPlanError::TimelineBeatAnalysisPending {
                asset_ids: sorted_asset_ids(asset_ids),
            })
        }
        TimelineBeatAnalysisState::Unavailable { asset_ids, reason } => {
            Err(CreatorPlanError::TimelineBeatAnalysisUnavailable {
                asset_ids: sorted_asset_ids(asset_ids),
                reason: reason.clone(),
            })
        }
    }
}

fn ready_asset_beats(
    asset: AssetId,
    status: &BeatStatus,
) -> Result<&crate::AssetBeats, CreatorPlanError> {
    match status {
        BeatStatus::NotRequested => Err(CreatorPlanError::BeatAnalysisNotRequested(asset)),
        BeatStatus::Queued => Err(CreatorPlanError::BeatAnalysisPending {
            asset,
            phase: "queued",
        }),
        BeatStatus::Hashing => Err(CreatorPlanError::BeatAnalysisPending {
            asset,
            phase: "hashing",
        }),
        BeatStatus::Analyzing { .. } => Err(CreatorPlanError::BeatAnalysisPending {
            asset,
            phase: "analyzing",
        }),
        BeatStatus::Ready(beats) => Ok(beats),
        BeatStatus::NoAudio => Err(CreatorPlanError::NoAudio(asset)),
        BeatStatus::Cancelled => Err(CreatorPlanError::BeatAnalysisCancelled(asset)),
        BeatStatus::Failed(reason) => Err(CreatorPlanError::BeatAnalysisFailed {
            asset,
            reason: reason.clone(),
        }),
    }
}

fn validate_timeline_range(range: &Range<TimeCode>) -> Result<(), CreatorPlanError> {
    if range.start < TimeCode::ZERO || range.end <= range.start {
        return Err(CreatorPlanError::InvalidTimelineRange {
            start: range.start,
            end: range.end,
        });
    }
    Ok(())
}

fn validate_operations(
    document: &Document,
    operations: &[Operation],
) -> Result<(), CreatorPlanError> {
    let mut candidate = document.clone();
    for operation in operations {
        operation.apply(&mut candidate)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_beat_montage_inputs(
    document: &Document,
    target_track: TrackId,
    music_asset: AssetId,
    timeline_range: &Range<TimeCode>,
    selects: &[BeatMontageSelect],
    analysis_state: &TimelineBeatAnalysisState,
    minimum_strength_basis_points: u16,
    minimum_shot_frames: TimeCode,
    maximum_shot_frames: TimeCode,
) -> Result<(), CreatorPlanError> {
    require_complete_timeline_beats(analysis_state)?;
    validate_timeline_range(timeline_range)?;
    if minimum_strength_basis_points > 10_000 {
        return Err(CreatorPlanError::InvalidMinimumStrength(
            minimum_strength_basis_points,
        ));
    }
    if minimum_shot_frames < TimeCode::ZERO
        || maximum_shot_frames <= TimeCode::ZERO
        || minimum_shot_frames > maximum_shot_frames
    {
        return Err(CreatorPlanError::InvalidMontageShotConstraints {
            minimum: minimum_shot_frames,
            maximum: maximum_shot_frames,
        });
    }
    if selects.len() < 2 {
        return Err(CreatorPlanError::TooFewMontageSelects(selects.len()));
    }

    let track = document
        .tracks
        .iter()
        .find(|track| track.id == target_track)
        .ok_or(CreatorPlanError::MissingTargetTrack(target_track))?;
    if track.kind != TrackKind::Video {
        return Err(CreatorPlanError::MontageTargetNotVideo(target_track));
    }
    let music = document
        .asset(music_asset)
        .ok_or(CreatorPlanError::MissingAsset(music_asset))?;
    if !music.kind.supports(TrackKind::Audio) {
        return Err(CreatorPlanError::MontageMusicNotAudio(music_asset));
    }

    for (index, select) in selects.iter().enumerate() {
        validate_montage_select(document, select, index)?;
    }
    let timeline_duration = timeline_range
        .end
        .checked_sub(timeline_range.start)
        .ok_or(OpError::TimeOverflow)?;
    if !montage_total_duration_is_feasible(
        timeline_duration,
        selects.len(),
        minimum_shot_frames,
        maximum_shot_frames,
    ) {
        return Err(CreatorPlanError::MontageDurationConstraintsUnsatisfied {
            shots: selects.len(),
            duration: timeline_duration,
            minimum: minimum_shot_frames,
            maximum: maximum_shot_frames,
        });
    }
    Ok(())
}

fn validate_montage_select(
    document: &Document,
    select: &BeatMontageSelect,
    index: usize,
) -> Result<(), CreatorPlanError> {
    let media = document
        .asset(select.asset)
        .ok_or(CreatorPlanError::MissingAsset(select.asset))?;
    if !media.kind.supports(TrackKind::Video) {
        return Err(CreatorPlanError::MontageSelectNotVideo {
            index,
            asset: select.asset,
        });
    }
    if select.source_range.start < TimeCode::ZERO
        || select.source_range.end <= select.source_range.start
        || select.source_range.end > media.duration
    {
        return Err(CreatorPlanError::InvalidMontageSourceEnvelope {
            index,
            asset: select.asset,
            start: select.source_range.start,
            end: select.source_range.end,
            asset_duration: media.duration,
        });
    }
    Ok(())
}

fn music_structure_beat_candidates(
    timeline_beats: &[TimelineBeat],
    music_asset: AssetId,
    timeline_range: &Range<TimeCode>,
    asset_duration: TimeCode,
    minimum_strength_basis_points: u16,
) -> Vec<TimelineBeat> {
    let mut candidates = timeline_beats
        .iter()
        .copied()
        .filter(|beat| {
            beat.asset == music_asset
                && beat.source_frame >= TimeCode::ZERO
                && beat.source_frame < asset_duration
                && beat.project_frame >= timeline_range.start
                && beat.project_frame < timeline_range.end
                && beat.strength_basis_points >= minimum_strength_basis_points
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|beat| {
        (
            beat.project_frame,
            Reverse(beat.strength_basis_points),
            beat.track,
            beat.clip,
            beat.source_frame,
            beat.estimated_bpm_milli,
        )
    });
    candidates.dedup_by_key(|beat| beat.project_frame);
    candidates
}

fn phase_strengths(beats: &[TimelineBeat], phase_count: usize) -> Vec<u64> {
    let mut strengths = vec![0_u64; phase_count];
    for (index, beat) in beats.iter().enumerate() {
        strengths[index % phase_count] =
            strengths[index % phase_count].saturating_add(u64::from(beat.strength_basis_points));
    }
    strengths
}

/// Select the first phase with the maximum aggregate strength. Iterating in
/// phase order makes ties stable and reproducible across runs.
fn strongest_phase(strengths: &[u64]) -> (usize, u64) {
    let mut best = (0, 0_u64);
    for (phase, &strength) in strengths.iter().enumerate() {
        if strength > best.1 {
            best = (phase, strength);
        }
    }
    best
}

fn phase_confidence(winner: u64, total: u64) -> u16 {
    if total == 0 {
        return 0;
    }
    let scaled = (u128::from(winner) * 10_000) / u128::from(total);
    u16::try_from(scaled.min(10_000)).unwrap_or(10_000)
}

fn music_bar_index(beat_index: usize, bar_phase: usize, meter_beats: usize) -> i64 {
    let beat_index = i128::try_from(beat_index).unwrap_or(i128::MAX);
    let bar_phase = i128::try_from(bar_phase).unwrap_or(i128::MAX);
    let meter_beats = i128::try_from(meter_beats).unwrap_or(i128::MAX).max(1);
    let bar_index = (beat_index - bar_phase).div_euclid(meter_beats);
    i64::try_from(bar_index).unwrap_or(if bar_index.is_negative() {
        i64::MIN
    } else {
        i64::MAX
    })
}

fn estimated_bpm_values(beats: &[TimelineBeat], project_fps: crate::Rational) -> Vec<u32> {
    beats
        .iter()
        .enumerate()
        .map(|(index, beat)| {
            if beat.estimated_bpm_milli != 0 {
                return beat.estimated_bpm_milli;
            }
            let previous = index
                .checked_sub(1)
                .and_then(|previous| beats.get(previous));
            let next = index.checked_add(1).and_then(|next| beats.get(next));
            previous
                .and_then(|previous| {
                    project_bpm_from_interval(
                        previous.project_frame,
                        beat.project_frame,
                        project_fps,
                    )
                })
                .or_else(|| {
                    next.and_then(|next| {
                        project_bpm_from_interval(
                            beat.project_frame,
                            next.project_frame,
                            project_fps,
                        )
                    })
                })
                .unwrap_or(0)
        })
        .collect()
}

fn project_bpm_from_interval(
    previous: TimeCode,
    current: TimeCode,
    project_fps: crate::Rational,
) -> Option<u32> {
    let interval = current.0.checked_sub(previous.0)?;
    if interval <= 0 || !project_fps.is_valid() {
        return None;
    }
    let numerator = i128::from(60_000_u32).checked_mul(i128::from(project_fps.numerator()))?;
    let denominator = i128::from(project_fps.denominator()).checked_mul(i128::from(interval))?;
    let rounded = (numerator + denominator / 2) / denominator;
    u32::try_from(rounded).ok()
}

fn median_u32(values: &[u32]) -> u32 {
    let mut sorted = values
        .iter()
        .copied()
        .filter(|value| *value != 0)
        .collect::<Vec<_>>();
    sorted.sort_unstable();
    sorted
        .get((sorted.len().saturating_sub(1)) / 2)
        .copied()
        .unwrap_or(0)
}

fn montage_beat_candidates(
    timeline_beats: &[TimelineBeat],
    music_asset: AssetId,
    timeline_range: &Range<TimeCode>,
    minimum_strength_basis_points: u16,
) -> Vec<TimelineBeat> {
    let mut candidates = timeline_beats
        .iter()
        .copied()
        .filter(|beat| {
            beat.asset == music_asset
                && beat.project_frame > timeline_range.start
                && beat.project_frame < timeline_range.end
                && beat.strength_basis_points >= minimum_strength_basis_points
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|beat| {
        (
            beat.project_frame,
            Reverse(beat.strength_basis_points),
            beat.track,
            beat.clip,
            beat.source_frame,
            beat.estimated_bpm_milli,
        )
    });
    candidates.dedup_by_key(|beat| beat.project_frame);
    candidates
}

fn validate_explicit_montage_anchors(
    anchors: &[TimeCode],
    candidates: &[TimelineBeat],
    music_asset: AssetId,
    timeline_range: &Range<TimeCode>,
    minimum_strength_basis_points: u16,
) -> Result<Vec<TimelineBeat>, CreatorPlanError> {
    let mut selected = Vec::with_capacity(anchors.len());
    for (index, &project_frame) in anchors.iter().enumerate() {
        if project_frame <= timeline_range.start || project_frame >= timeline_range.end {
            return Err(CreatorPlanError::MontageExplicitAnchorOutsideRange {
                index,
                project_frame,
                start: timeline_range.start,
                end: timeline_range.end,
            });
        }
        if index > 0 {
            let previous = anchors[index - 1];
            if project_frame <= previous {
                return Err(CreatorPlanError::MontageExplicitAnchorUnordered {
                    index,
                    previous,
                    project_frame,
                });
            }
        }
        let Some(&beat) = candidates
            .iter()
            .find(|beat| beat.project_frame == project_frame)
        else {
            return Err(CreatorPlanError::MontageExplicitAnchorNotEligible {
                index,
                music_asset,
                project_frame,
                minimum_strength_basis_points,
            });
        };
        selected.push(beat);
    }
    Ok(selected)
}

fn validate_explicit_montage_durations(
    boundaries: &[TimeCode],
    minimum: TimeCode,
    maximum: TimeCode,
) -> Result<(), CreatorPlanError> {
    for (shot_index, boundary_pair) in boundaries.windows(2).enumerate() {
        let start = boundary_pair[0];
        let end = boundary_pair[1];
        let duration = end.checked_sub(start).ok_or(OpError::TimeOverflow)?;
        if duration < minimum || duration > maximum {
            return Err(CreatorPlanError::MontageExplicitShotDurationUnsatisfied {
                shot_index,
                start,
                end,
                duration,
                minimum,
                maximum,
            });
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn choose_montage_beats(
    document: &Document,
    selects: &[BeatMontageSelect],
    candidates: &[TimelineBeat],
    timeline_range: &Range<TimeCode>,
    minimum_shot_frames: TimeCode,
    maximum_shot_frames: TimeCode,
) -> Result<Vec<TimelineBeat>, CreatorPlanError> {
    let required_anchors = selects.len() - 1;
    if candidates.len() < required_anchors {
        return Err(CreatorPlanError::InsufficientMontageBeats {
            required: required_anchors,
            eligible: candidates.len(),
        });
    }
    let source_aware = select_montage_boundaries(
        candidates,
        timeline_range,
        selects.len(),
        minimum_shot_frames,
        maximum_shot_frames,
        |shot_index, duration| montage_select_can_fill(document, &selects[shot_index], duration),
    );
    if let Some(boundaries) = source_aware {
        return Ok(boundaries);
    }

    // Preserve a useful source-envelope error when beats and shot lengths are
    // otherwise feasible. Shot resolution identifies the exact bad envelope.
    select_montage_boundaries(
        candidates,
        timeline_range,
        selects.len(),
        minimum_shot_frames,
        maximum_shot_frames,
        |_, _| true,
    )
    .ok_or(CreatorPlanError::MontageBeatConstraintsUnsatisfied {
        required: required_anchors,
        minimum: minimum_shot_frames,
        maximum: maximum_shot_frames,
    })
}

fn resolve_montage_shots(
    document: &Document,
    target_track: TrackId,
    selects: &[BeatMontageSelect],
    boundaries: &[TimeCode],
    mode: ThreePointMode,
) -> Result<(Vec<BeatMontageShot>, Vec<Operation>), CreatorPlanError> {
    let mut shots = Vec::with_capacity(selects.len());
    let mut operations = Vec::with_capacity(selects.len());
    for (index, (select, boundary_pair)) in selects.iter().zip(boundaries.windows(2)).enumerate() {
        let shot_timeline = boundary_pair[0]..boundary_pair[1];
        let shot_duration = shot_timeline
            .end
            .checked_sub(shot_timeline.start)
            .ok_or(OpError::TimeOverflow)?;
        let media = document
            .asset(select.asset)
            .ok_or(CreatorPlanError::MissingAsset(select.asset))?;
        // Match the operation's boundary derivation exactly, including mixed
        // frame-rate rounding, so this is the source range apply will consume.
        // A source envelope is an allowed region, not a required source_in.
        // When absolute-boundary rounding makes the requested duration
        // unavailable from the first frame, preserve the earliest feasible
        // source subrange within the envelope instead of rejecting it.
        let source_range = resolve_montage_source_range(
            &select.source_range,
            media.fps,
            document.fps,
            shot_duration,
        )
        .ok_or(CreatorPlanError::MontageSourceEnvelopeTooShort {
            index,
            asset: select.asset,
            start: select.source_range.start,
            end: select.source_range.end,
            maximum_project_frames: map_source_range_to_project(
                select.source_range.clone(),
                media.fps,
                document.fps,
            )
            .unwrap_or(TimeCode::ZERO),
            required_project_frames: shot_duration,
        })?;
        shots.push(BeatMontageShot {
            select_index: index,
            asset: select.asset,
            source_envelope: select.source_range.clone(),
            source_range: source_range.clone(),
            timeline_range: shot_timeline.clone(),
        });
        operations.push(Operation::ThreePointEdit {
            track: target_track,
            asset: select.asset,
            source_in: Some(source_range.start),
            source_out: None,
            timeline_in: Some(shot_timeline.start),
            timeline_out: Some(shot_timeline.end),
            mode,
        });
    }
    Ok((shots, operations))
}

#[derive(Debug, Clone)]
struct MontageBoundaryPath {
    candidate_indices: Vec<usize>,
    ideal_distance: u128,
    total_strength: u64,
}

#[derive(Debug, Clone)]
struct NearAnchorPath {
    candidate_indices: Vec<usize>,
    maximum_distance: u64,
    total_distance: u128,
    total_strength: u64,
    last_duration: TimeCode,
    current_similar_run: usize,
    cadence_satisfied: bool,
    cadence_buckets: Vec<i64>,
}

fn montage_total_duration_is_feasible(
    duration: TimeCode,
    shots: usize,
    minimum: TimeCode,
    maximum: TimeCode,
) -> bool {
    let Ok(shots) = i128::try_from(shots) else {
        return false;
    };
    let duration = i128::from(duration.0);
    let minimum_total = i128::from(minimum.0) * shots;
    let maximum_total = i128::from(maximum.0) * shots;
    duration >= minimum_total && duration <= maximum_total
}

fn select_montage_boundaries(
    candidates: &[TimelineBeat],
    range: &Range<TimeCode>,
    shot_count: usize,
    minimum: TimeCode,
    maximum: TimeCode,
    shot_can_fill: impl Fn(usize, TimeCode) -> bool,
) -> Option<Vec<TimelineBeat>> {
    let required = shot_count.checked_sub(1)?;
    let total_frames = u128::try_from(range.end.0.checked_sub(range.start.0)?).ok()?;
    let scaled_shot_count = u128::try_from(shot_count).ok()?;
    let mut states: Vec<Option<MontageBoundaryPath>> = vec![None; candidates.len()];

    for anchor_number in 1..=required {
        let mut next = vec![None; candidates.len()];
        for (candidate_index, candidate) in candidates.iter().enumerate() {
            let distance = montage_ideal_distance(
                candidate.project_frame,
                range.start,
                total_frames,
                scaled_shot_count,
                anchor_number,
            )?;
            if anchor_number == 1 {
                let opening_duration = candidate.project_frame.0.checked_sub(range.start.0)?;
                if montage_shot_duration_is_valid(opening_duration, minimum, maximum)
                    && shot_can_fill(0, TimeCode(opening_duration))
                {
                    next[candidate_index] = Some(MontageBoundaryPath {
                        candidate_indices: vec![candidate_index],
                        ideal_distance: distance,
                        total_strength: u64::from(candidate.strength_basis_points),
                    });
                }
                continue;
            }

            for (previous_index, previous_path) in states
                .iter()
                .take(candidate_index)
                .enumerate()
                .filter_map(|(index, path)| path.as_ref().map(|path| (index, path)))
            {
                let previous_frame = candidates[previous_index].project_frame;
                let shot_duration = candidate.project_frame.0.checked_sub(previous_frame.0)?;
                if !montage_shot_duration_is_valid(shot_duration, minimum, maximum)
                    || !shot_can_fill(anchor_number - 1, TimeCode(shot_duration))
                {
                    continue;
                }
                let mut path = previous_path.clone();
                path.candidate_indices.push(candidate_index);
                path.ideal_distance = path.ideal_distance.checked_add(distance)?;
                path.total_strength = path
                    .total_strength
                    .checked_add(u64::from(candidate.strength_basis_points))?;
                if next[candidate_index]
                    .as_ref()
                    .is_none_or(|current| montage_path_is_better(&path, current))
                {
                    next[candidate_index] = Some(path);
                }
            }
        }
        states = next;
    }

    let best = states
        .into_iter()
        .enumerate()
        .filter_map(|(candidate_index, path)| {
            let closing_duration = range
                .end
                .0
                .checked_sub(candidates[candidate_index].project_frame.0)?;
            (montage_shot_duration_is_valid(closing_duration, minimum, maximum)
                && shot_can_fill(required, TimeCode(closing_duration)))
            .then_some(path?)
        })
        .reduce(|current, candidate| {
            if montage_path_is_better(&candidate, &current) {
                candidate
            } else {
                current
            }
        })?;

    Some(
        best.candidate_indices
            .into_iter()
            .map(|index| candidates[index])
            .collect(),
    )
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
fn select_montage_boundaries_near_anchors(
    candidates: &[TimelineBeat],
    range: &Range<TimeCode>,
    shot_count: usize,
    minimum: TimeCode,
    maximum: TimeCode,
    preferred_anchors: &[TimeCode],
    maximum_anchor_movement_frames: Option<TimeCode>,
    locked_anchor_indices: &[usize],
    cadence_contract: Option<&BeatMontageCadenceContract>,
    shot_can_fill: impl Fn(usize, TimeCode) -> bool,
) -> Option<NearAnchorPath> {
    let required = shot_count.checked_sub(1)?;
    if preferred_anchors.len() != required || candidates.is_empty() {
        return None;
    }
    let mut states: Vec<Vec<NearAnchorPath>> = vec![Vec::new(); candidates.len()];

    for (candidate_index, candidate) in candidates.iter().enumerate() {
        if !anchor_movement_is_allowed(
            candidate.project_frame,
            preferred_anchors[0],
            maximum_anchor_movement_frames,
        ) || (locked_anchor_indices.contains(&0)
            && candidate.project_frame != preferred_anchors[0])
        {
            continue;
        }
        let opening_duration = candidate.project_frame.0.checked_sub(range.start.0)?;
        if !montage_shot_duration_is_valid(opening_duration, minimum, maximum)
            || !shot_can_fill(0, TimeCode(opening_duration))
        {
            continue;
        }
        let duration = TimeCode(opening_duration);
        let Some((cadence_satisfied, cadence_buckets, current_similar_run)) =
            advance_cadence_state(duration, None, 0, false, &[], cadence_contract)
        else {
            continue;
        };
        let distance = candidate.project_frame.0.abs_diff(preferred_anchors[0].0);
        states[candidate_index].push(NearAnchorPath {
            candidate_indices: vec![candidate_index],
            maximum_distance: distance,
            total_distance: u128::from(distance),
            total_strength: u64::from(candidate.strength_basis_points),
            last_duration: duration,
            current_similar_run,
            cadence_satisfied,
            cadence_buckets,
        });
    }

    for (ordinal, preferred_anchor) in preferred_anchors.iter().enumerate().take(required).skip(1) {
        let mut next: Vec<Vec<NearAnchorPath>> = vec![Vec::new(); candidates.len()];
        for (candidate_index, candidate) in candidates.iter().enumerate() {
            if !anchor_movement_is_allowed(
                candidate.project_frame,
                *preferred_anchor,
                maximum_anchor_movement_frames,
            ) || (locked_anchor_indices.contains(&ordinal)
                && candidate.project_frame != *preferred_anchor)
            {
                continue;
            }
            for previous_index in 0..candidate_index {
                for previous_path in &states[previous_index] {
                    let shot_duration = candidate
                        .project_frame
                        .0
                        .checked_sub(candidates[previous_index].project_frame.0)?;
                    if !montage_shot_duration_is_valid(shot_duration, minimum, maximum)
                        || !shot_can_fill(ordinal - 1, TimeCode(shot_duration))
                    {
                        continue;
                    }
                    let duration = TimeCode(shot_duration);
                    let Some((cadence_satisfied, cadence_buckets, current_similar_run)) =
                        advance_cadence_state(
                            duration,
                            Some(previous_path.last_duration),
                            previous_path.current_similar_run,
                            previous_path.cadence_satisfied,
                            &previous_path.cadence_buckets,
                            cadence_contract,
                        )
                    else {
                        continue;
                    };
                    let distance = candidate.project_frame.0.abs_diff(preferred_anchor.0);
                    let mut path = previous_path.clone();
                    path.candidate_indices.push(candidate_index);
                    path.maximum_distance = path.maximum_distance.max(distance);
                    path.total_distance = path.total_distance.saturating_add(u128::from(distance));
                    path.total_strength = path
                        .total_strength
                        .saturating_add(u64::from(candidate.strength_basis_points));
                    path.last_duration = duration;
                    path.current_similar_run = current_similar_run;
                    path.cadence_satisfied = cadence_satisfied;
                    path.cadence_buckets = cadence_buckets;
                    insert_near_anchor_path(&mut next[candidate_index], path);
                }
            }
        }
        states = next;
    }

    let mut best: Option<NearAnchorPath> = None;
    for (candidate_index, paths) in states.into_iter().enumerate() {
        let Some(closing_duration) = range
            .end
            .0
            .checked_sub(candidates[candidate_index].project_frame.0)
            .map(TimeCode)
        else {
            continue;
        };
        if !montage_shot_duration_is_valid(closing_duration.0, minimum, maximum)
            || !shot_can_fill(required, closing_duration)
        {
            continue;
        }
        for path in paths {
            let Some((cadence_satisfied, _, _)) = advance_cadence_state(
                closing_duration,
                Some(path.last_duration),
                path.current_similar_run,
                path.cadence_satisfied,
                &path.cadence_buckets,
                cadence_contract,
            ) else {
                continue;
            };
            if !cadence_satisfied {
                continue;
            }
            if best
                .as_ref()
                .is_none_or(|current| near_anchor_path_is_better(&path, current))
            {
                best = Some(path);
            }
        }
    }
    best
}

fn anchor_movement_is_allowed(
    candidate: TimeCode,
    preferred: TimeCode,
    maximum: Option<TimeCode>,
) -> bool {
    maximum.is_none_or(|limit| candidate.0.abs_diff(preferred.0) <= limit.0.unsigned_abs())
}

fn advance_cadence_state(
    duration: TimeCode,
    previous_duration: Option<TimeCode>,
    previous_run: usize,
    cadence_satisfied: bool,
    previous_buckets: &[i64],
    contract: Option<&BeatMontageCadenceContract>,
) -> Option<(bool, Vec<i64>, usize)> {
    let Some(contract) = contract else {
        return Some((true, Vec::new(), 0));
    };
    let current_run = match previous_duration {
        Some(previous)
            if previous.0.abs_diff(duration.0)
                <= contract.similar_tolerance_frames.0.unsigned_abs() =>
        {
            previous_run.checked_add(1)?
        }
        Some(_) | None => 1,
    };
    if current_run > contract.maximum_similar_run {
        return None;
    }
    if cadence_satisfied {
        return Some((true, Vec::new(), current_run));
    }
    let bucket = cadence_bucket(duration, contract.duration_bucket_frames);
    let mut buckets = previous_buckets.to_vec();
    if buckets.binary_search(&bucket).is_err() {
        buckets.push(bucket);
        buckets.sort_unstable();
    }
    if buckets.len() >= contract.minimum_duration_buckets {
        return Some((true, Vec::new(), current_run));
    }
    Some((false, buckets, current_run))
}

fn cadence_bucket(duration: TimeCode, bucket_frames: TimeCode) -> i64 {
    duration
        .0
        .saturating_add(bucket_frames.0 / 2)
        .div_euclid(bucket_frames.0)
}

fn insert_near_anchor_path(paths: &mut Vec<NearAnchorPath>, candidate: NearAnchorPath) {
    if let Some(index) = paths.iter().position(|current| {
        current.last_duration == candidate.last_duration
            && current.current_similar_run == candidate.current_similar_run
            && current.cadence_satisfied == candidate.cadence_satisfied
            && current.cadence_buckets == candidate.cadence_buckets
    }) {
        if near_anchor_path_is_better(&candidate, &paths[index]) {
            paths[index] = candidate;
        }
    } else {
        paths.push(candidate);
    }
}

fn near_anchor_path_is_better(candidate: &NearAnchorPath, current: &NearAnchorPath) -> bool {
    candidate.maximum_distance < current.maximum_distance
        || (candidate.maximum_distance == current.maximum_distance
            && (candidate.total_distance < current.total_distance
                || (candidate.total_distance == current.total_distance
                    && (candidate.total_strength > current.total_strength
                        || (candidate.total_strength == current.total_strength
                            && candidate.candidate_indices < current.candidate_indices)))))
}

fn montage_select_can_fill(
    document: &Document,
    select: &BeatMontageSelect,
    project_duration: TimeCode,
) -> bool {
    document.asset(select.asset).is_some_and(|media| {
        resolve_montage_source_range(
            &select.source_range,
            media.fps,
            document.fps,
            project_duration,
        )
        .is_some()
    })
}

fn montage_ideal_distance(
    frame: TimeCode,
    start: TimeCode,
    total_frames: u128,
    shot_count: u128,
    anchor_number: usize,
) -> Option<u128> {
    let relative = u128::try_from(frame.0.checked_sub(start.0)?).ok()?;
    let anchor_number = u128::try_from(anchor_number).ok()?;
    Some(
        relative
            .checked_mul(shot_count)?
            .abs_diff(total_frames.checked_mul(anchor_number)?),
    )
}

fn montage_shot_duration_is_valid(
    duration_frames: i64,
    minimum: TimeCode,
    maximum: TimeCode,
) -> bool {
    duration_frames >= minimum.0 && duration_frames <= maximum.0
}

fn montage_path_is_better(candidate: &MontageBoundaryPath, current: &MontageBoundaryPath) -> bool {
    candidate.ideal_distance < current.ideal_distance
        || (candidate.ideal_distance == current.ideal_distance
            && (candidate.total_strength > current.total_strength
                || (candidate.total_strength == current.total_strength
                    && candidate.candidate_indices < current.candidate_indices)))
}

fn select_music_source(
    media: &crate::MediaAsset,
    beats: &crate::AssetBeats,
    preferred: TimeCode,
    minimum_strength_basis_points: u16,
    project_fps: crate::Rational,
    project_duration: TimeCode,
) -> Result<(crate::BeatMarker, TimeCode), CreatorPlanError> {
    let mut candidates = beats
        .beats
        .iter()
        .copied()
        .filter(|beat| {
            beat.source_frame >= TimeCode::ZERO
                && beat.source_frame < media.duration
                && beat.strength_basis_points >= minimum_strength_basis_points
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|beat| {
        (
            absolute_frame_distance(beat.source_frame, preferred),
            Reverse(beat.strength_basis_points),
            beat.source_frame,
        )
    });
    if candidates.is_empty() {
        return Err(CreatorPlanError::NoEligibleMusicBeat { asset: media.id });
    }

    candidates
        .iter()
        .find_map(|beat| {
            source_end_for_project_duration(
                beat.source_frame,
                media.duration,
                media.fps,
                project_fps,
                project_duration,
            )
            .map(|source_out| (*beat, source_out))
        })
        .ok_or(CreatorPlanError::InsufficientMusicSource { asset: media.id })
}

fn select_end_anchored_music_source(
    media: &crate::MediaAsset,
    beats: &crate::AssetBeats,
    preferred_start: TimeCode,
    end_anchor: MusicEndAnchor,
    minimum_strength_basis_points: u16,
    project_fps: crate::Rational,
    project_duration: TimeCode,
) -> Result<(crate::BeatMarker, TimeCode), CreatorPlanError> {
    let mut candidates = beats
        .beats
        .iter()
        .copied()
        .filter(|beat| {
            beat.source_frame >= TimeCode::ZERO
                && beat.source_frame < media.duration
                && beat.strength_basis_points >= minimum_strength_basis_points
        })
        .filter_map(|beat| {
            let source_out = source_end_for_project_duration(
                beat.source_frame,
                media.duration,
                media.fps,
                project_fps,
                project_duration,
            )?;
            let endpoint_distance =
                absolute_frame_distance(source_out, end_anchor.preferred_source_end);
            (endpoint_distance <= end_anchor.maximum_drift_frames.0.unsigned_abs()).then_some((
                endpoint_distance,
                absolute_frame_distance(beat.source_frame, preferred_start),
                Reverse(beat.strength_basis_points),
                beat.source_frame,
                beat,
                source_out,
            ))
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(
        |(endpoint_distance, start_distance, strength, source_frame, _, _)| {
            (
                *endpoint_distance,
                *start_distance,
                *strength,
                *source_frame,
            )
        },
    );
    candidates
        .into_iter()
        .next()
        .map(|(_, _, _, _, beat, source_out)| (beat, source_out))
        .ok_or(CreatorPlanError::MusicEndAnchorUnsatisfied {
            asset: media.id,
            target_source_end: end_anchor.preferred_source_end,
            maximum_drift_frames: end_anchor.maximum_drift_frames,
        })
}

fn music_end_alignment(
    beats: &crate::AssetBeats,
    media_duration: TimeCode,
    source_out: TimeCode,
) -> MusicEndBeatAlignment {
    let offset = beats
        .beats
        .iter()
        .filter(|beat| beat.source_frame >= TimeCode::ZERO && beat.source_frame < media_duration)
        .min_by_key(|beat| {
            (
                absolute_frame_distance(beat.source_frame, source_out),
                beat.source_frame,
            )
        })
        .map_or(0, |beat| source_out.0.saturating_sub(beat.source_frame.0));
    if offset == 0 {
        MusicEndBeatAlignment::Exact
    } else {
        MusicEndBeatAlignment::Offset(offset)
    }
}

fn sorted_asset_ids(asset_ids: &[AssetId]) -> Vec<AssetId> {
    let mut sorted = asset_ids.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    sorted
}

fn absolute_frame_distance(left: TimeCode, right: TimeCode) -> u64 {
    left.0.abs_diff(right.0)
}

/// Resolve the earliest source subrange inside an allowed envelope that maps
/// to an exact project duration. Mixed frame-rate mapping rounds absolute
/// boundaries, so a requested duration can be unavailable from the envelope's
/// first source frame even though a one-frame phase shift can represent it.
/// The reduced denominator of the project/source rate ratio is the complete
/// phase period; searching one period is sufficient to find the earliest
/// feasible subrange.
fn resolve_montage_source_range(
    envelope: &Range<TimeCode>,
    source_fps: crate::Rational,
    project_fps: crate::Rational,
    project_duration: TimeCode,
) -> Option<Range<TimeCode>> {
    let phase_period = source_mapping_phase_period(source_fps, project_fps).unwrap_or(1);
    let phase_end = envelope
        .start
        .checked_add(TimeCode(phase_period))
        .unwrap_or(envelope.end)
        .min(envelope.end);
    let mut source_start = envelope.start;
    while source_start < phase_end {
        if let Some(source_out) = source_end_for_project_duration(
            source_start,
            envelope.end,
            source_fps,
            project_fps,
            project_duration,
        ) {
            return Some(source_start..source_out);
        }
        source_start = source_start.checked_add(TimeCode(1))?;
    }
    None
}

fn source_mapping_phase_period(
    source_fps: crate::Rational,
    project_fps: crate::Rational,
) -> Option<i64> {
    if !source_fps.is_valid() || !project_fps.is_valid() {
        return None;
    }
    let ratio_numerator =
        u64::from(project_fps.numerator()).checked_mul(u64::from(source_fps.denominator()))?;
    let ratio_denominator =
        u64::from(project_fps.denominator()).checked_mul(u64::from(source_fps.numerator()))?;
    let divisor = greatest_common_divisor(ratio_numerator, ratio_denominator);
    i64::try_from(ratio_denominator / divisor)
        .ok()
        .filter(|period| *period > 0)
}

fn greatest_common_divisor(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

fn source_end_for_project_duration(
    source_start: TimeCode,
    maximum_end: TimeCode,
    source_fps: crate::Rational,
    project_fps: crate::Rational,
    project_duration: TimeCode,
) -> Option<TimeCode> {
    let mut low = source_start.0.checked_add(1)?;
    let mut high = maximum_end.0;
    while low <= high {
        let middle = low + (high - low) / 2;
        let candidate = TimeCode(middle);
        let mapped =
            map_source_range_to_project(source_start..candidate, source_fps, project_fps).ok()?;
        match mapped.cmp(&project_duration) {
            std::cmp::Ordering::Less => low = middle.checked_add(1)?,
            std::cmp::Ordering::Greater => high = middle.checked_sub(1)?,
            std::cmp::Ordering::Equal => return Some(candidate),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc};

    use crate::{
        AssetBeats, BeatMarker, ClipContent, MediaAsset, MediaKind, Rational, Track, TrackKind,
        apply_batch,
    };

    use super::*;

    fn asset(id: u64, kind: MediaKind, duration: i64, fps: Rational) -> MediaAsset {
        MediaAsset {
            id: AssetId(id),
            path: PathBuf::from(format!("asset-{id}.mov")),
            name: format!("asset-{id}"),
            duration: TimeCode(duration),
            fps,
            kind,
            resolution: kind.supports(TrackKind::Video).then_some((1_920, 1_080)),
            color_description: crate::ColorDescription::default(),
        }
    }

    fn clip(id: u64, asset: u64, start: i64, source: Range<i64>) -> crate::Clip {
        crate::Clip {
            id: ClipId(id),
            asset: AssetId(asset),
            source_range: TimeCode(source.start)..TimeCode(source.end),
            content: ClipContent::Media,
            timeline_start: TimeCode(start),
            effects: Vec::new(),
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
        }
    }

    fn pacing_document() -> Document {
        let fps = Rational::new(30, 1).unwrap();
        Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![clip(10, 1, 0, 0..300)],
            }],
            media_pool: vec![asset(1, MediaKind::Video, 300, fps)],
            fps,
            resolution: (1_920, 1_080),
            duration: TimeCode(300),
            ..Document::default()
        }
    }

    fn timeline_beat(project: i64, strength: u16, source_clip: u64) -> TimelineBeat {
        TimelineBeat {
            asset: AssetId(2),
            track: TrackId(2),
            clip: ClipId(source_clip),
            source_frame: TimeCode(project),
            project_frame: TimeCode(project),
            strength_basis_points: strength,
            estimated_bpm_milli: 120_000,
        }
    }

    fn montage_document() -> Document {
        let fps = Rational::new(30, 1).unwrap();
        Document {
            tracks: vec![
                Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: Vec::new(),
                },
                Track {
                    id: TrackId(2),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: vec![clip(90, 9, 0, 0..300)],
                },
            ],
            media_pool: vec![
                asset(1, MediaKind::Video, 300, fps),
                asset(2, MediaKind::Video, 300, fps),
                asset(3, MediaKind::Video, 300, fps),
                asset(9, MediaKind::AudioVideo, 300, fps),
                asset(10, MediaKind::Audio, 300, fps),
            ],
            fps,
            resolution: (1_920, 1_080),
            duration: TimeCode(300),
            ..Document::default()
        }
    }

    fn montage_select(asset: u64, start: i64, end: i64) -> BeatMontageSelect {
        BeatMontageSelect {
            asset: AssetId(asset),
            source_range: TimeCode(start)..TimeCode(end),
        }
    }

    fn nonzero_mixed_montage_document() -> Document {
        let mut document = montage_document();
        document.fps = Rational::new(25, 1).unwrap();
        document.media_pool[0].fps = Rational::new(24, 1).unwrap();
        document.media_pool[0].duration = TimeCode(1_253);
        document.duration = TimeCode(250);
        document
    }

    fn montage_beat(asset: u64, project: i64, strength: u16, clip: u64) -> TimelineBeat {
        TimelineBeat {
            asset: AssetId(asset),
            track: TrackId(2),
            clip: ClipId(clip),
            source_frame: TimeCode(project),
            project_frame: TimeCode(project),
            strength_basis_points: strength,
            estimated_bpm_milli: 120_000,
        }
    }

    fn structure_document() -> Document {
        let fps = Rational::new(30, 1).unwrap();
        Document {
            media_pool: vec![
                asset(9, MediaKind::Audio, 1_000, fps),
                asset(10, MediaKind::Video, 1_000, fps),
            ],
            fps,
            resolution: (1_920, 1_080),
            duration: TimeCode(1_000),
            ..Document::default()
        }
    }

    fn structure_beat(
        asset: u64,
        project: i64,
        strength: u16,
        clip: u64,
        bpm: u32,
    ) -> TimelineBeat {
        TimelineBeat {
            asset: AssetId(asset),
            track: TrackId(2),
            clip: ClipId(clip),
            source_frame: TimeCode(project),
            project_frame: TimeCode(project),
            strength_basis_points: strength,
            estimated_bpm_milli: bpm,
        }
    }

    fn beat_status(asset_id: u64, duration: i64, beats: &[(i64, u16)]) -> BeatStatus {
        BeatStatus::Ready(Arc::new(AssetBeats {
            asset: AssetId(asset_id),
            content_sha256: "test".to_owned(),
            source_fps: Rational::new(30, 1).unwrap(),
            source_frames: TimeCode(duration),
            estimated_bpm_milli: 120_000,
            beats: beats
                .iter()
                .map(|(frame, strength)| BeatMarker {
                    source_frame: TimeCode(*frame),
                    strength_basis_points: *strength,
                })
                .collect(),
        }))
    }

    #[test]
    fn music_structure_infers_bar_and_phrase_phases_and_roles() {
        let document = structure_document();
        let beats = (0..12)
            .map(|index| {
                let strength = if index % 4 == 1 {
                    if index % 8 == 1 { 9_000 } else { 8_000 }
                } else {
                    1_000
                };
                structure_beat(9, i64::from(index * 30), strength, 20, 120_000)
            })
            .collect::<Vec<_>>();

        let analysis = music_structure_analysis(
            &document,
            AssetId(9),
            TimeCode::ZERO..TimeCode(360),
            &beats,
            &TimelineBeatAnalysisState::Ready,
            0,
            4,
            2,
        )
        .unwrap();

        assert_eq!(analysis.parameters.bar_phase, 1);
        assert_eq!(analysis.parameters.phrase_phase, 0);
        assert_eq!(analysis.parameters.estimated_bpm_milli, 120_000);
        assert_eq!(analysis.parameters.bar_phase_strength, 26_000);
        assert_eq!(analysis.parameters.total_beat_strength, 35_000);
        assert_eq!(analysis.parameters.phrase_phase_strength, 18_000);
        assert_eq!(analysis.parameters.total_bar_strength, 26_000);
        assert_eq!(analysis.candidates.len(), 12);
        assert_eq!(analysis.candidates[1].project_frame, TimeCode(30));
        assert_eq!(analysis.candidates[1].beat_in_bar, 0);
        assert_eq!(analysis.candidates[1].bar_in_phrase, 0);
        assert_eq!(analysis.candidates[1].role, MusicStructureRole::Phrase);
        assert_eq!(analysis.candidates[5].role, MusicStructureRole::Bar);
        assert_eq!(analysis.candidates[9].role, MusicStructureRole::Phrase);
        assert!(
            analysis
                .candidates
                .iter()
                .enumerate()
                .all(|(index, candidate)| candidate.beat_index == index)
        );
    }

    #[test]
    fn music_structure_filters_range_asset_strength_and_deduplicates_stably() {
        let document = structure_document();
        let beats = vec![
            structure_beat(10, 30, 9_000, 1, 120_000),
            structure_beat(9, 0, 9_000, 1, 120_000),
            structure_beat(9, 30, 2_000, 1, 120_000),
            structure_beat(9, 30, 8_000, 2, 120_000),
            structure_beat(9, 60, 1_000, 1, 120_000),
            structure_beat(9, 90, 8_000, 1, 120_000),
            structure_beat(9, 120, 8_000, 1, 120_000),
            structure_beat(9, 150, 8_000, 1, 120_000),
            structure_beat(9, 180, 8_000, 1, 120_000),
        ];
        // A source frame outside the asset is ignored even if its project
        // frame and strength would otherwise make it eligible.
        let mut invalid_source = structure_beat(9, 210, 9_000, 1, 120_000);
        invalid_source.source_frame = TimeCode(1_000);
        let mut beats = beats;
        beats.push(invalid_source);

        let analysis = music_structure_analysis(
            &document,
            AssetId(9),
            TimeCode(30)..TimeCode(181),
            &beats,
            &TimelineBeatAnalysisState::Ready,
            5_000,
            4,
            4,
        )
        .unwrap();

        assert_eq!(
            analysis
                .candidates
                .iter()
                .map(|candidate| (candidate.project_frame, candidate.strength_basis_points))
                .collect::<Vec<_>>(),
            vec![
                (TimeCode(30), 8_000),
                (TimeCode(90), 8_000),
                (TimeCode(120), 8_000),
                (TimeCode(150), 8_000),
                (TimeCode(180), 8_000),
            ]
        );
    }

    #[test]
    fn music_structure_ties_choose_lowest_phase_and_fallback_bpm_uses_project_fps() {
        let document = structure_document();
        let beats = [0, 30, 60, 90].map(|project| structure_beat(9, project, 1_000, 1, 0));
        let analysis = music_structure_analysis(
            &document,
            AssetId(9),
            TimeCode::ZERO..TimeCode(120),
            &beats,
            &TimelineBeatAnalysisState::Ready,
            0,
            4,
            1,
        )
        .unwrap();

        assert_eq!(analysis.parameters.bar_phase, 0);
        assert_eq!(analysis.parameters.phrase_phase, 0);
        assert_eq!(analysis.parameters.estimated_bpm_milli, 60_000);
        assert!(
            analysis
                .candidates
                .iter()
                .all(|candidate| candidate.estimated_bpm_milli == 60_000)
        );
    }

    #[test]
    fn music_structure_rejects_invalid_inputs_and_analysis_lifecycle() {
        let document = structure_document();
        let beats = [structure_beat(9, 30, 8_000, 1, 120_000)];
        let invalid_meter = music_structure_analysis(
            &document,
            AssetId(9),
            TimeCode::ZERO..TimeCode(100),
            &beats,
            &TimelineBeatAnalysisState::Ready,
            0,
            0,
            4,
        )
        .unwrap_err();
        assert!(matches!(
            invalid_meter,
            CreatorPlanError::InvalidMusicStructureMeter { value: 0, .. }
        ));

        let invalid_phrase = music_structure_analysis(
            &document,
            AssetId(9),
            TimeCode::ZERO..TimeCode(100),
            &beats,
            &TimelineBeatAnalysisState::Ready,
            0,
            4,
            MUSIC_STRUCTURE_MAX_PHRASE_BARS + 1,
        )
        .unwrap_err();
        assert!(matches!(
            invalid_phrase,
            CreatorPlanError::InvalidMusicStructurePhraseBars { .. }
        ));

        let pending = music_structure_analysis(
            &document,
            AssetId(9),
            TimeCode::ZERO..TimeCode(100),
            &beats,
            &TimelineBeatAnalysisState::Pending {
                asset_ids: vec![AssetId(9)],
            },
            0,
            4,
            4,
        )
        .unwrap_err();
        assert!(matches!(
            pending,
            CreatorPlanError::TimelineBeatAnalysisPending { .. }
        ));

        let unavailable = music_structure_analysis(
            &document,
            AssetId(9),
            TimeCode::ZERO..TimeCode(100),
            &beats,
            &TimelineBeatAnalysisState::Unavailable {
                asset_ids: vec![AssetId(9)],
                reason: "decoder".to_owned(),
            },
            0,
            4,
            4,
        )
        .unwrap_err();
        assert!(matches!(
            unavailable,
            CreatorPlanError::TimelineBeatAnalysisUnavailable { .. }
        ));

        let no_beats = music_structure_analysis(
            &document,
            AssetId(9),
            TimeCode::ZERO..TimeCode(100),
            &[],
            &TimelineBeatAnalysisState::Ready,
            0,
            4,
            4,
        )
        .unwrap_err();
        assert!(matches!(
            no_beats,
            CreatorPlanError::NoEligibleMusicStructureBeats { .. }
        ));
    }

    #[test]
    fn music_structure_analysis_round_trips_through_json() {
        let document = structure_document();
        let beats = [
            structure_beat(9, 0, 8_000, 1, 120_000),
            structure_beat(9, 30, 8_000, 1, 120_000),
            structure_beat(9, 60, 8_000, 1, 120_000),
            structure_beat(9, 90, 8_000, 1, 120_000),
        ];
        let analysis = music_structure_analysis(
            &document,
            AssetId(9),
            TimeCode::ZERO..TimeCode(120),
            &beats,
            &TimelineBeatAnalysisState::Ready,
            0,
            4,
            1,
        )
        .unwrap();
        let encoded = serde_json::to_string(&analysis).unwrap();
        assert!(encoded.contains("\"music_asset\":9"));
        assert!(encoded.contains("\"role\":\"phrase\""));
        let decoded: MusicStructureAnalysis = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, analysis);
    }

    #[test]
    fn pacing_splits_target_to_other_track_beats_in_descending_apply_order() {
        let document = pacing_document();
        let beats = vec![
            timeline_beat(180, 7_000, 21),
            timeline_beat(60, 8_000, 21),
            timeline_beat(120, 4_000, 21),
            timeline_beat(120, 9_000, 22),
        ];

        let plan = beat_pacing_plan(
            &document,
            ClipId(10),
            None,
            &beats,
            &TimelineBeatAnalysisState::Ready,
            5_000,
            TimeCode(30),
        )
        .unwrap();

        assert_eq!(
            plan.selected_beats
                .iter()
                .map(|beat| (beat.project_frame.0, beat.strength_basis_points))
                .collect::<Vec<_>>(),
            vec![(60, 8_000), (120, 9_000), (180, 7_000)]
        );
        assert_eq!(
            plan.operations,
            vec![
                Operation::SplitClip {
                    clip: ClipId(10),
                    at: TimeCode(180),
                },
                Operation::SplitClip {
                    clip: ClipId(10),
                    at: TimeCode(120),
                },
                Operation::SplitClip {
                    clip: ClipId(10),
                    at: TimeCode(60),
                },
            ]
        );

        let mut applied = document;
        apply_batch(&mut applied, &plan.operations).unwrap();
        assert_eq!(
            applied.tracks[0]
                .clips
                .iter()
                .map(|clip| clip.timeline_start.0)
                .collect::<Vec<_>>(),
            vec![0, 60, 120, 180]
        );
    }

    #[test]
    fn pacing_excludes_boundaries_and_applies_spacing_from_earliest_onset() {
        let document = pacing_document();
        let beats = [0, 50, 75, 100, 150].map(|frame| timeline_beat(frame, 8_000, 21));

        let plan = beat_pacing_plan(
            &document,
            ClipId(10),
            Some(TimeCode(50)..TimeCode(150)),
            &beats,
            &TimelineBeatAnalysisState::Ready,
            0,
            TimeCode(30),
        )
        .unwrap();

        assert_eq!(
            plan.selected_beats
                .iter()
                .map(|beat| beat.project_frame.0)
                .collect::<Vec<_>>(),
            vec![75]
        );
    }

    #[test]
    fn pacing_accepts_retimed_targets_when_each_split_is_representable() {
        let mut document = pacing_document();
        document.tracks[0].clips[0].speed_percent = 200;
        document.duration = TimeCode(150);
        let beats = [50, 100].map(|frame| timeline_beat(frame, 8_000, 21));

        let plan = beat_pacing_plan(
            &document,
            ClipId(10),
            None,
            &beats,
            &TimelineBeatAnalysisState::Ready,
            0,
            TimeCode::ZERO,
        )
        .unwrap();

        let mut applied = document;
        apply_batch(&mut applied, &plan.operations).unwrap();
        assert_eq!(
            applied.tracks[0]
                .clips
                .iter()
                .map(|clip| (clip.timeline_start.0, clip.source_range.clone()))
                .collect::<Vec<_>>(),
            vec![
                (0, TimeCode(0)..TimeCode(99)),
                (50, TimeCode(99)..TimeCode(199)),
                (100, TimeCode(199)..TimeCode(300)),
            ]
        );
    }

    #[test]
    fn pacing_rejects_partial_and_unavailable_analysis_explicitly() {
        let document = pacing_document();
        let pending = beat_pacing_plan(
            &document,
            ClipId(10),
            None,
            &[timeline_beat(60, 8_000, 21)],
            &TimelineBeatAnalysisState::Pending {
                asset_ids: vec![AssetId(3), AssetId(2), AssetId(3)],
            },
            0,
            TimeCode::ZERO,
        )
        .unwrap_err();
        assert_eq!(
            pending,
            CreatorPlanError::TimelineBeatAnalysisPending {
                asset_ids: vec![AssetId(2), AssetId(3)],
            }
        );

        let unavailable = beat_pacing_plan(
            &document,
            ClipId(10),
            None,
            &[],
            &TimelineBeatAnalysisState::Unavailable {
                asset_ids: vec![AssetId(2)],
                reason: "no audio stream".to_owned(),
            },
            0,
            TimeCode::ZERO,
        )
        .unwrap_err();
        assert!(matches!(
            unavailable,
            CreatorPlanError::TimelineBeatAnalysisUnavailable { .. }
        ));
    }

    #[test]
    fn montage_plans_av_music_and_applies_ordered_shots_gaplessly() {
        let document = montage_document();
        let selects = vec![
            montage_select(1, 10, 200),
            montage_select(2, 20, 200),
            montage_select(3, 30, 200),
        ];
        let beats = vec![
            montage_beat(9, 60, 8_000, 90),
            montage_beat(9, 30, 7_000, 91),
            montage_beat(9, 30, 9_000, 90),
        ];

        let plan = beat_montage_plan(
            &document,
            TrackId(1),
            AssetId(9),
            TimeCode::ZERO..TimeCode(90),
            &selects,
            &beats,
            &TimelineBeatAnalysisState::Ready,
            5_000,
            TimeCode(20),
            TimeCode(40),
            ThreePointMode::Overwrite,
        )
        .unwrap();

        assert_eq!(
            plan.cut_anchors
                .iter()
                .map(|anchor| {
                    (
                        anchor.after_shot_index,
                        anchor.beat.project_frame.0,
                        anchor.beat.strength_basis_points,
                    )
                })
                .collect::<Vec<_>>(),
            vec![(0, 30, 9_000), (1, 60, 8_000)]
        );
        assert_eq!(
            plan.shots
                .iter()
                .map(|shot| {
                    (
                        shot.asset,
                        shot.timeline_range.clone(),
                        shot.source_range.clone(),
                    )
                })
                .collect::<Vec<_>>(),
            vec![
                (
                    AssetId(1),
                    TimeCode(0)..TimeCode(30),
                    TimeCode(10)..TimeCode(40),
                ),
                (
                    AssetId(2),
                    TimeCode(30)..TimeCode(60),
                    TimeCode(20)..TimeCode(50),
                ),
                (
                    AssetId(3),
                    TimeCode(60)..TimeCode(90),
                    TimeCode(30)..TimeCode(60),
                ),
            ]
        );

        let mut applied = document;
        apply_batch(&mut applied, &plan.operations).unwrap();
        let clips = &applied.tracks[0].clips;
        assert_eq!(clips.len(), 3);
        assert_eq!(
            clips
                .iter()
                .map(|clip| (clip.asset, clip.timeline_start, clip.source_range.clone()))
                .collect::<Vec<_>>(),
            vec![
                (AssetId(1), TimeCode(0), TimeCode(10)..TimeCode(40),),
                (AssetId(2), TimeCode(30), TimeCode(20)..TimeCode(50),),
                (AssetId(3), TimeCode(60), TimeCode(30)..TimeCode(60),),
            ]
        );
        for pair in clips.windows(2) {
            let left_end = pair[0]
                .timeline_start
                .checked_add(applied.clip_duration(&pair[0]).unwrap())
                .unwrap();
            assert_eq!(left_end, pair[1].timeline_start);
        }
    }

    #[test]
    fn montage_ignores_beats_from_non_target_music_assets() {
        let document = montage_document();
        let selects = [montage_select(1, 0, 200), montage_select(2, 0, 200)];
        let beats = [
            montage_beat(10, 50, 10_000, 100),
            montage_beat(9, 40, 7_000, 90),
        ];

        let plan = beat_montage_plan(
            &document,
            TrackId(1),
            AssetId(9),
            TimeCode::ZERO..TimeCode(100),
            &selects,
            &beats,
            &TimelineBeatAnalysisState::Ready,
            0,
            TimeCode(20),
            TimeCode(80),
            ThreePointMode::Overwrite,
        )
        .unwrap();

        assert_eq!(plan.cut_anchors[0].beat.project_frame, TimeCode(40));
        assert_eq!(plan.cut_anchors[0].beat.beat_asset, AssetId(9));
    }

    #[test]
    fn montage_ties_resolve_to_stable_earliest_frames() {
        let document = montage_document();
        let selects = [
            montage_select(1, 0, 200),
            montage_select(2, 0, 200),
            montage_select(3, 0, 200),
        ];
        let beats = [85, 45, 75, 35]
            .map(|frame| montage_beat(9, frame, 8_000, u64::try_from(frame).unwrap()));

        let plan = beat_montage_plan(
            &document,
            TrackId(1),
            AssetId(9),
            TimeCode::ZERO..TimeCode(120),
            &selects,
            &beats,
            &TimelineBeatAnalysisState::Ready,
            0,
            TimeCode(20),
            TimeCode(60),
            ThreePointMode::Overwrite,
        )
        .unwrap();

        assert_eq!(
            plan.cut_anchors
                .iter()
                .map(|anchor| anchor.beat.project_frame.0)
                .collect::<Vec<_>>(),
            vec![35, 75]
        );
    }

    #[test]
    fn montage_distinguishes_insufficient_beats_from_infeasible_constraints() {
        let document = montage_document();
        let selects = [
            montage_select(1, 0, 200),
            montage_select(2, 0, 200),
            montage_select(3, 0, 200),
        ];

        let insufficient = beat_montage_plan(
            &document,
            TrackId(1),
            AssetId(9),
            TimeCode::ZERO..TimeCode(120),
            &selects,
            &[montage_beat(9, 40, 8_000, 90)],
            &TimelineBeatAnalysisState::Ready,
            0,
            TimeCode(20),
            TimeCode(60),
            ThreePointMode::Overwrite,
        )
        .unwrap_err();
        assert_eq!(
            insufficient,
            CreatorPlanError::InsufficientMontageBeats {
                required: 2,
                eligible: 1,
            }
        );

        let infeasible = beat_montage_plan(
            &document,
            TrackId(1),
            AssetId(9),
            TimeCode::ZERO..TimeCode(120),
            &selects,
            &[
                montage_beat(9, 10, 8_000, 90),
                montage_beat(9, 110, 8_000, 90),
            ],
            &TimelineBeatAnalysisState::Ready,
            0,
            TimeCode(20),
            TimeCode(60),
            ThreePointMode::Overwrite,
        )
        .unwrap_err();
        assert_eq!(
            infeasible,
            CreatorPlanError::MontageBeatConstraintsUnsatisfied {
                required: 2,
                minimum: TimeCode(20),
                maximum: TimeCode(60),
            }
        );
    }

    #[test]
    fn montage_reports_the_short_source_envelope() {
        let document = montage_document();
        let error = beat_montage_plan(
            &document,
            TrackId(1),
            AssetId(9),
            TimeCode::ZERO..TimeCode(60),
            &[montage_select(1, 0, 20), montage_select(2, 0, 100)],
            &[montage_beat(9, 30, 8_000, 90)],
            &TimelineBeatAnalysisState::Ready,
            0,
            TimeCode(20),
            TimeCode(40),
            ThreePointMode::Overwrite,
        )
        .unwrap_err();

        assert_eq!(
            error,
            CreatorPlanError::MontageSourceEnvelopeTooShort {
                index: 0,
                asset: AssetId(1),
                start: TimeCode(0),
                end: TimeCode(20),
                maximum_project_frames: TimeCode(20),
                required_project_frames: TimeCode(30),
            }
        );
        assert_eq!(
            error.to_string(),
            "montage select 0 envelope 0..20 for asset 1 can supply at most 20 project frames in real time, but needs 30; reassign this select to a shorter slot or select a larger source envelope"
        );
    }

    #[test]
    fn montage_rejects_non_video_targets_and_select_assets() {
        let document = montage_document();
        let target_error = beat_montage_plan(
            &document,
            TrackId(2),
            AssetId(9),
            TimeCode::ZERO..TimeCode(60),
            &[montage_select(1, 0, 100), montage_select(2, 0, 100)],
            &[montage_beat(9, 30, 8_000, 90)],
            &TimelineBeatAnalysisState::Ready,
            0,
            TimeCode(20),
            TimeCode(40),
            ThreePointMode::Overwrite,
        )
        .unwrap_err();
        assert_eq!(
            target_error,
            CreatorPlanError::MontageTargetNotVideo(TrackId(2))
        );

        let asset_error = beat_montage_plan(
            &document,
            TrackId(1),
            AssetId(9),
            TimeCode::ZERO..TimeCode(60),
            &[montage_select(10, 0, 100), montage_select(2, 0, 100)],
            &[montage_beat(9, 30, 8_000, 90)],
            &TimelineBeatAnalysisState::Ready,
            0,
            TimeCode(20),
            TimeCode(40),
            ThreePointMode::Overwrite,
        )
        .unwrap_err();
        assert_eq!(
            asset_error,
            CreatorPlanError::MontageSelectNotVideo {
                index: 0,
                asset: AssetId(10),
            }
        );
    }

    #[test]
    fn montage_dp_skips_nearest_beat_when_mixed_rate_envelope_cannot_fill_it() {
        let mut document = montage_document();
        document.media_pool[0].fps = Rational::new(24, 1).unwrap();
        document.media_pool[1].fps = Rational::new(25, 1).unwrap();
        let selects = [montage_select(1, 0, 23), montage_select(2, 0, 100)];
        let beats = [
            montage_beat(9, 30, 9_000, 90),
            montage_beat(9, 29, 8_000, 90),
        ];

        let plan = beat_montage_plan(
            &document,
            TrackId(1),
            AssetId(9),
            TimeCode::ZERO..TimeCode(60),
            &selects,
            &beats,
            &TimelineBeatAnalysisState::Ready,
            0,
            TimeCode(20),
            TimeCode(40),
            ThreePointMode::Overwrite,
        )
        .unwrap();

        assert_eq!(plan.cut_anchors[0].beat.project_frame, TimeCode(29));
        assert_eq!(plan.shots[0].source_range, TimeCode(0)..TimeCode(23));
        assert_eq!(plan.shots[1].source_range, TimeCode(0)..TimeCode(26));
    }

    #[test]
    fn explicit_montage_preserves_nonuniform_anchors_and_validates_operations() {
        let document = montage_document();
        let selects = [
            montage_select(1, 0, 200),
            montage_select(2, 0, 200),
            montage_select(3, 0, 200),
        ];
        let anchors = [TimeCode(25), TimeCode(70)];
        let beats = [
            montage_beat(9, 25, 8_000, 90),
            montage_beat(9, 70, 9_000, 90),
            montage_beat(9, 40, 10_000, 90),
        ];

        let plan = beat_montage_plan_with_anchors(
            &document,
            TrackId(1),
            AssetId(9),
            TimeCode::ZERO..TimeCode(120),
            &selects,
            &anchors,
            &beats,
            &TimelineBeatAnalysisState::Ready,
            5_000,
            TimeCode(20),
            TimeCode(60),
            ThreePointMode::Overwrite,
        )
        .unwrap();

        assert_eq!(
            plan.cut_anchors
                .iter()
                .map(|anchor| anchor.beat.project_frame)
                .collect::<Vec<_>>(),
            anchors
        );
        assert_eq!(
            plan.shots
                .iter()
                .map(|shot| shot.timeline_range.clone())
                .collect::<Vec<_>>(),
            vec![
                TimeCode(0)..TimeCode(25),
                TimeCode(25)..TimeCode(70),
                TimeCode(70)..TimeCode(120),
            ]
        );

        let mut applied = document;
        apply_batch(&mut applied, &plan.operations).unwrap();
        assert_eq!(
            applied.tracks[0]
                .clips
                .iter()
                .map(|clip| clip.timeline_start)
                .collect::<Vec<_>>(),
            vec![TimeCode(0), TimeCode(25), TimeCode(70)]
        );
    }

    #[test]
    fn explicit_montage_rejects_anchor_count_order_and_range_errors() {
        let document = montage_document();
        let selects = [
            montage_select(1, 0, 200),
            montage_select(2, 0, 200),
            montage_select(3, 0, 200),
        ];
        let beats = [
            montage_beat(9, 25, 8_000, 90),
            montage_beat(9, 70, 8_000, 90),
        ];

        let count_error = beat_montage_plan_with_anchors(
            &document,
            TrackId(1),
            AssetId(9),
            TimeCode::ZERO..TimeCode(120),
            &selects,
            &[TimeCode(25)],
            &beats,
            &TimelineBeatAnalysisState::Ready,
            0,
            TimeCode(20),
            TimeCode(60),
            ThreePointMode::Overwrite,
        )
        .unwrap_err();
        assert_eq!(
            count_error,
            CreatorPlanError::MontageExplicitAnchorCountMismatch {
                expected: 2,
                actual: 1,
                shots: 3,
            }
        );

        let unordered_error = beat_montage_plan_with_anchors(
            &document,
            TrackId(1),
            AssetId(9),
            TimeCode::ZERO..TimeCode(120),
            &selects,
            &[TimeCode(70), TimeCode(25)],
            &beats,
            &TimelineBeatAnalysisState::Ready,
            0,
            TimeCode(20),
            TimeCode(60),
            ThreePointMode::Overwrite,
        )
        .unwrap_err();
        assert_eq!(
            unordered_error,
            CreatorPlanError::MontageExplicitAnchorUnordered {
                index: 1,
                previous: TimeCode(70),
                project_frame: TimeCode(25),
            }
        );

        let outside_error = beat_montage_plan_with_anchors(
            &document,
            TrackId(1),
            AssetId(9),
            TimeCode::ZERO..TimeCode(120),
            &selects,
            &[TimeCode(0), TimeCode(70)],
            &beats,
            &TimelineBeatAnalysisState::Ready,
            0,
            TimeCode(20),
            TimeCode(60),
            ThreePointMode::Overwrite,
        )
        .unwrap_err();
        assert_eq!(
            outside_error,
            CreatorPlanError::MontageExplicitAnchorOutsideRange {
                index: 0,
                project_frame: TimeCode(0),
                start: TimeCode(0),
                end: TimeCode(120),
            }
        );
    }

    #[test]
    fn explicit_montage_rejects_ineligible_beats_and_invalid_shot_duration() {
        let document = montage_document();
        let selects = [
            montage_select(1, 0, 200),
            montage_select(2, 0, 200),
            montage_select(3, 0, 200),
        ];
        let beats = [montage_beat(9, 25, 8_000, 90)];

        let ineligible_error = beat_montage_plan_with_anchors(
            &document,
            TrackId(1),
            AssetId(9),
            TimeCode::ZERO..TimeCode(120),
            &selects,
            &[TimeCode(25), TimeCode(70)],
            &beats,
            &TimelineBeatAnalysisState::Ready,
            0,
            TimeCode(20),
            TimeCode(60),
            ThreePointMode::Overwrite,
        )
        .unwrap_err();
        assert_eq!(
            ineligible_error,
            CreatorPlanError::MontageExplicitAnchorNotEligible {
                index: 1,
                music_asset: AssetId(9),
                project_frame: TimeCode(70),
                minimum_strength_basis_points: 0,
            }
        );

        let duration_error = beat_montage_plan_with_anchors(
            &document,
            TrackId(1),
            AssetId(9),
            TimeCode::ZERO..TimeCode(120),
            &selects,
            &[TimeCode(10), TimeCode(70)],
            &[
                montage_beat(9, 10, 8_000, 90),
                montage_beat(9, 70, 8_000, 90),
            ],
            &TimelineBeatAnalysisState::Ready,
            0,
            TimeCode(20),
            TimeCode(60),
            ThreePointMode::Overwrite,
        )
        .unwrap_err();
        assert_eq!(
            duration_error,
            CreatorPlanError::MontageExplicitShotDurationUnsatisfied {
                shot_index: 0,
                start: TimeCode(0),
                end: TimeCode(10),
                duration: TimeCode(10),
                minimum: TimeCode(20),
                maximum: TimeCode(60),
            }
        );
    }

    #[test]
    fn explicit_montage_reports_source_infeasibility_and_maps_mixed_fps() {
        let document = montage_document();
        let source_error = beat_montage_plan_with_anchors(
            &document,
            TrackId(1),
            AssetId(9),
            TimeCode::ZERO..TimeCode(120),
            &[
                montage_select(1, 0, 20),
                montage_select(2, 0, 200),
                montage_select(3, 0, 200),
            ],
            &[TimeCode(25), TimeCode(70)],
            &[
                montage_beat(9, 25, 8_000, 90),
                montage_beat(9, 70, 8_000, 90),
            ],
            &TimelineBeatAnalysisState::Ready,
            0,
            TimeCode(20),
            TimeCode(60),
            ThreePointMode::Overwrite,
        )
        .unwrap_err();
        assert_eq!(
            source_error,
            CreatorPlanError::MontageSourceEnvelopeTooShort {
                index: 0,
                asset: AssetId(1),
                start: TimeCode(0),
                end: TimeCode(20),
                maximum_project_frames: TimeCode(20),
                required_project_frames: TimeCode(25),
            }
        );

        let mut mixed_document = montage_document();
        mixed_document.media_pool[0].fps = Rational::new(24, 1).unwrap();
        mixed_document.media_pool[1].fps = Rational::new(25, 1).unwrap();
        let mixed_plan = beat_montage_plan_with_anchors(
            &mixed_document,
            TrackId(1),
            AssetId(9),
            TimeCode::ZERO..TimeCode(60),
            &[montage_select(1, 0, 23), montage_select(2, 0, 100)],
            &[TimeCode(29)],
            &[
                montage_beat(9, 29, 8_000, 90),
                montage_beat(9, 30, 9_000, 90),
            ],
            &TimelineBeatAnalysisState::Ready,
            0,
            TimeCode(20),
            TimeCode(40),
            ThreePointMode::Overwrite,
        )
        .unwrap();
        assert_eq!(mixed_plan.cut_anchors[0].beat.project_frame, TimeCode(29));
        assert_eq!(mixed_plan.shots[0].source_range, TimeCode(0)..TimeCode(23));
        assert_eq!(mixed_plan.shots[1].source_range, TimeCode(0)..TimeCode(26));
    }

    #[test]
    fn mixed_fps_montage_shifts_nonzero_source_start_to_represent_duration() {
        let mut document = montage_document();
        document.fps = Rational::new(25, 1).unwrap();
        document.media_pool[0].fps = Rational::new(24, 1).unwrap();
        document.media_pool[0].duration = TimeCode(1_253);
        document.duration = TimeCode(250);

        let plan = beat_montage_plan_with_anchors(
            &document,
            TrackId(1),
            AssetId(9),
            TimeCode::ZERO..TimeCode(100),
            &[montage_select(1, 780, 832), montage_select(2, 0, 100)],
            &[TimeCode(49)],
            &[montage_beat(9, 49, 8_000, 90)],
            &TimelineBeatAnalysisState::Ready,
            0,
            TimeCode(40),
            TimeCode(60),
            ThreePointMode::Overwrite,
        )
        .unwrap();

        assert_eq!(plan.shots[0].source_range, TimeCode(781)..TimeCode(828));
        assert_eq!(
            plan.operations[0],
            Operation::ThreePointEdit {
                track: TrackId(1),
                asset: AssetId(1),
                source_in: Some(TimeCode(781)),
                source_out: None,
                timeline_in: Some(TimeCode::ZERO),
                timeline_out: Some(TimeCode(49)),
                mode: ThreePointMode::Overwrite,
            }
        );
    }

    #[test]
    fn near_anchor_montage_preserves_exact_preferred_path_without_repair() {
        let document = montage_document();
        let (plan, repair) = beat_montage_plan_near_anchors_with_report(
            &document,
            TrackId(1),
            AssetId(9),
            TimeCode::ZERO..TimeCode(60),
            &[montage_select(1, 0, 100), montage_select(2, 0, 100)],
            &[TimeCode(30)],
            &[montage_beat(9, 30, 8_000, 90)],
            &TimelineBeatAnalysisState::Ready,
            0,
            TimeCode(20),
            TimeCode(40),
            ThreePointMode::Overwrite,
            Some(TimeCode::ZERO),
            &[],
            None,
        )
        .unwrap();

        assert_eq!(plan.cut_anchors[0].beat.project_frame, TimeCode(30));
        assert_eq!(repair.preferred_anchors, vec![TimeCode(30)]);
        assert_eq!(repair.resolved_anchors, vec![TimeCode(30)]);
        assert_eq!(repair.signed_deltas, vec![0]);
        assert_eq!(repair.maximum_absolute_delta, 0);
        assert_eq!(repair.total_absolute_delta, 0);
    }

    #[test]
    fn near_anchor_montage_repairs_final_capacity_minimally_and_passes_cadence() {
        let document = nonzero_mixed_montage_document();
        let contract = BeatMontageCadenceContract {
            minimum_duration_buckets: 2,
            duration_bucket_frames: TimeCode(20),
            maximum_similar_run: 2,
            similar_tolerance_frames: TimeCode(8),
        };
        let (plan, repair) = beat_montage_plan_near_anchors_with_report(
            &document,
            TrackId(1),
            AssetId(9),
            TimeCode::ZERO..TimeCode(100),
            &[montage_select(1, 780, 827), montage_select(2, 0, 100)],
            &[TimeCode(49)],
            &[
                montage_beat(9, 48, 8_000, 90),
                montage_beat(9, 49, 9_000, 90),
                montage_beat(9, 50, 8_000, 90),
            ],
            &TimelineBeatAnalysisState::Ready,
            0,
            TimeCode(40),
            TimeCode(60),
            ThreePointMode::Overwrite,
            Some(TimeCode(2)),
            &[],
            Some(contract),
        )
        .unwrap();

        assert_eq!(plan.cut_anchors[0].beat.project_frame, TimeCode(48));
        assert_eq!(plan.shots[0].source_range, TimeCode(780)..TimeCode(827));
        assert_eq!(repair.signed_deltas, vec![-1]);
        assert_eq!(repair.absolute_deltas, vec![1]);
        assert_eq!(repair.maximum_absolute_delta, 1);
        assert_eq!(repair.total_absolute_delta, 1);
        let cadence = validate_beat_montage_plan_cadence(&plan, contract).unwrap();
        assert_eq!(cadence.distinct_buckets, vec![2, 3]);
        assert_eq!(cadence.longest_similar_run, 2);
    }

    #[test]
    fn near_anchor_montage_reports_source_error_when_repair_is_impossible() {
        let document = nonzero_mixed_montage_document();
        let error = beat_montage_plan_near_anchors(
            &document,
            TrackId(1),
            AssetId(9),
            TimeCode::ZERO..TimeCode(100),
            &[montage_select(1, 780, 827), montage_select(2, 0, 100)],
            &[TimeCode(49)],
            &[montage_beat(9, 49, 9_000, 90)],
            &TimelineBeatAnalysisState::Ready,
            0,
            TimeCode(40),
            TimeCode(60),
            ThreePointMode::Overwrite,
            Some(TimeCode(0)),
            &[],
            None,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            CreatorPlanError::MontageSourceEnvelopeTooShort {
                maximum_project_frames: TimeCode(48),
                required_project_frames: TimeCode(49),
                ..
            }
        ));
        assert!(error.to_string().contains("at most 48 project frames"));
    }

    #[test]
    fn cadence_contract_reports_observed_failure_evidence_and_repair_hint() {
        let durations = [69, 50, 70, 40, 40, 40, 40, 47, 63, 74, 67]
            .map(TimeCode)
            .to_vec();
        let contract = BeatMontageCadenceContract {
            minimum_duration_buckets: 3,
            duration_bucket_frames: TimeCode(20),
            maximum_similar_run: 3,
            similar_tolerance_frames: TimeCode(8),
        };

        let error = validate_beat_montage_cadence(&durations, contract).unwrap_err();
        assert_eq!(
            error,
            CreatorPlanError::MontageCadenceContractUnsatisfied {
                minimum_duration_buckets: 3,
                duration_bucket_frames: TimeCode(20),
                maximum_similar_run: 3,
                similar_tolerance_frames: TimeCode(8),
                observed_durations: durations,
                observed_buckets: vec![2, 3, 4],
                observed_longest_similar_run: 5,
            }
        );
        let message = error.to_string();
        assert!(message.contains("rounded buckets [2, 3, 4] using 20 frames"));
        assert!(message.contains("longest similar run 5"));
        assert!(message.contains("vary shot durations or choose different cut anchors"));
    }

    #[test]
    fn cadence_contract_accepts_varied_fixture_and_computes_plan_durations() {
        let durations = [120, 80, 40, 60, 40, 80, 120].map(TimeCode).to_vec();
        let contract = BeatMontageCadenceContract {
            minimum_duration_buckets: 3,
            duration_bucket_frames: TimeCode(20),
            maximum_similar_run: 3,
            similar_tolerance_frames: TimeCode(8),
        };
        let summary = validate_beat_montage_cadence(&durations, contract).unwrap();
        assert_eq!(summary.durations, durations);
        assert_eq!(summary.rounded_buckets, vec![6, 4, 2, 3, 2, 4, 6]);
        assert_eq!(summary.distinct_buckets, vec![2, 3, 4, 6]);
        assert_eq!(summary.longest_similar_run, 1);

        let mut timeline_start = TimeCode::ZERO;
        let shots = durations
            .iter()
            .enumerate()
            .map(|(index, duration)| {
                let timeline_range = timeline_start..timeline_start.checked_add(*duration).unwrap();
                timeline_start = timeline_range.end;
                BeatMontageShot {
                    select_index: index,
                    asset: AssetId(1),
                    source_envelope: TimeCode::ZERO..TimeCode(200),
                    source_range: TimeCode::ZERO..*duration,
                    timeline_range,
                }
            })
            .collect();
        let plan = BeatMontagePlan {
            target_track: TrackId(1),
            music_asset: AssetId(9),
            timeline_range: TimeCode::ZERO..timeline_start,
            minimum_strength_basis_points: 0,
            minimum_shot_frames: TimeCode(1),
            maximum_shot_frames: TimeCode(200),
            mode: ThreePointMode::Overwrite,
            shots,
            cut_anchors: Vec::new(),
            operations: Vec::new(),
        };
        let plan_summary = validate_beat_montage_plan_cadence(&plan, contract).unwrap();
        assert_eq!(plan_summary.durations, durations);
    }

    #[test]
    fn cadence_contract_rejects_invalid_settings() {
        let error = validate_beat_montage_cadence(
            &[TimeCode(40)],
            BeatMontageCadenceContract {
                minimum_duration_buckets: 0,
                duration_bucket_frames: TimeCode(0),
                maximum_similar_run: 0,
                similar_tolerance_frames: TimeCode(-1),
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            CreatorPlanError::InvalidMontageCadenceContract { .. }
        ));
    }

    #[test]
    fn music_fit_starts_on_beat_and_fills_exact_range_without_hidden_retime() {
        let fps = Rational::new(30, 1).unwrap();
        let document = Document {
            tracks: vec![Track {
                id: TrackId(7),
                kind: TrackKind::Audio,
                sync_lock: true,
                clips: Vec::new(),
            }],
            media_pool: vec![asset(2, MediaKind::Audio, 600, fps)],
            fps,
            resolution: (1_920, 1_080),
            ..Document::default()
        };
        let status = beat_status(2, 600, &[(30, 9_000), (180, 8_000)]);

        let plan = music_fit_plan(
            &document,
            TrackId(7),
            AssetId(2),
            TimeCode(100)..TimeCode(220),
            Some(TimeCode(20)),
            &status,
            5_000,
            ThreePointMode::Overwrite,
        )
        .unwrap();

        assert_eq!(plan.source_range, TimeCode(30)..TimeCode(150));
        assert_eq!(plan.anchor_beat.source_frame, TimeCode(30));
        assert_eq!(plan.strategy, MusicFitStrategy::BeatAnchoredStraightCut);
        assert_eq!(plan.end_anchor, None);
        assert_eq!(plan.duration_fit, MusicDurationFit::ExactProjectRange);
        assert_eq!(plan.playback, MusicPlaybackMode::RealTime);
        assert_eq!(plan.repeat, MusicRepeatMode::None);
        assert_eq!(
            plan.source_end_alignment,
            MusicEndBeatAlignment::Offset(-30)
        );

        let mut applied = document;
        apply_batch(&mut applied, &plan.operations).unwrap();
        let inserted = &applied.tracks[0].clips[0];
        assert_eq!(inserted.timeline_start, TimeCode(100));
        assert_eq!(inserted.source_range, TimeCode(30)..TimeCode(150));
        assert_eq!(inserted.speed_percent, 100);
    }

    #[test]
    fn music_fit_skips_preferred_beat_when_it_cannot_fill_at_real_time() {
        let fps = Rational::new(30, 1).unwrap();
        let document = Document {
            tracks: vec![Track {
                id: TrackId(7),
                kind: TrackKind::Audio,
                sync_lock: true,
                clips: Vec::new(),
            }],
            media_pool: vec![asset(2, MediaKind::Audio, 300, fps)],
            fps,
            resolution: (1_920, 1_080),
            ..Document::default()
        };
        let status = beat_status(2, 300, &[(30, 7_000), (270, 9_000)]);

        let plan = music_fit_plan(
            &document,
            TrackId(7),
            AssetId(2),
            TimeCode::ZERO..TimeCode(90),
            Some(TimeCode(270)),
            &status,
            0,
            ThreePointMode::Overwrite,
        )
        .unwrap();

        assert_eq!(plan.anchor_beat.source_frame, TimeCode(30));
        assert_eq!(plan.source_range, TimeCode(30)..TimeCode(120));
    }

    #[test]
    fn music_fit_reports_analysis_lifecycle_instead_of_empty_plan() {
        let fps = Rational::new(30, 1).unwrap();
        let document = Document {
            tracks: vec![Track {
                id: TrackId(7),
                kind: TrackKind::Audio,
                sync_lock: true,
                clips: Vec::new(),
            }],
            media_pool: vec![asset(2, MediaKind::Audio, 300, fps)],
            fps,
            ..Document::default()
        };

        let error = music_fit_plan(
            &document,
            TrackId(7),
            AssetId(2),
            TimeCode::ZERO..TimeCode(90),
            None,
            &BeatStatus::Analyzing {
                progress_percent: Some(50),
            },
            0,
            ThreePointMode::Overwrite,
        )
        .unwrap_err();

        assert_eq!(
            error,
            CreatorPlanError::BeatAnalysisPending {
                asset: AssetId(2),
                phase: "analyzing",
            }
        );
    }

    #[test]
    fn music_fit_maps_source_and_project_frame_rates_exactly() {
        let source_fps = Rational::new(24, 1).unwrap();
        let project_fps = Rational::new(30, 1).unwrap();
        let document = Document {
            tracks: vec![Track {
                id: TrackId(7),
                kind: TrackKind::Audio,
                sync_lock: true,
                clips: Vec::new(),
            }],
            media_pool: vec![asset(2, MediaKind::Audio, 480, source_fps)],
            fps: project_fps,
            ..Document::default()
        };
        let status = BeatStatus::Ready(Arc::new(AssetBeats {
            asset: AssetId(2),
            content_sha256: "test".to_owned(),
            source_fps,
            source_frames: TimeCode(480),
            estimated_bpm_milli: 120_000,
            beats: vec![BeatMarker {
                source_frame: TimeCode(24),
                strength_basis_points: 10_000,
            }],
        }));

        let plan = music_fit_plan(
            &document,
            TrackId(7),
            AssetId(2),
            TimeCode(30)..TimeCode(180),
            None,
            &status,
            0,
            ThreePointMode::Overwrite,
        )
        .unwrap();

        assert_eq!(plan.source_range, TimeCode(24)..TimeCode(144));
        assert_eq!(
            map_source_range_to_project(plan.source_range, source_fps, project_fps).unwrap(),
            TimeCode(150)
        );
    }

    #[test]
    fn end_anchored_music_fit_prefers_exact_natural_tail_over_nearer_start() {
        let source_fps = Rational::new(30, 1).unwrap();
        let project_fps = Rational::new(25, 1).unwrap();
        let document = Document {
            tracks: vec![Track {
                id: TrackId(7),
                kind: TrackKind::Audio,
                sync_lock: true,
                clips: Vec::new(),
            }],
            media_pool: vec![asset(2, MediaKind::Audio, 6_170, source_fps)],
            fps: project_fps,
            resolution: (1_920, 1_080),
            ..Document::default()
        };
        let status = beat_status(2, 6_170, &[(5_160, 5_638), (5_161, 10_000)]);

        let plan = music_fit_plan_with_end_anchor(
            &document,
            TrackId(7),
            AssetId(2),
            TimeCode::ZERO..TimeCode(700),
            Some(TimeCode(5_161)),
            Some(MusicEndAnchor {
                preferred_source_end: TimeCode(6_000),
                maximum_drift_frames: TimeCode::ZERO,
            }),
            &status,
            0,
            ThreePointMode::Overwrite,
        )
        .unwrap();

        assert_eq!(plan.source_range, TimeCode(5_160)..TimeCode(6_000));
        assert_eq!(plan.anchor_beat.source_frame, TimeCode(5_160));
        assert_eq!(plan.strategy, MusicFitStrategy::EndAnchoredStraightCut);
        assert_eq!(
            plan.end_anchor,
            Some(MusicEndAnchorEvidence {
                target_source_end: TimeCode(6_000),
                resolved_source_end: TimeCode(6_000),
                signed_offset_frames: 0,
                maximum_drift_frames: TimeCode::ZERO,
            })
        );
        assert_eq!(
            map_source_range_to_project(plan.source_range, source_fps, project_fps).unwrap(),
            TimeCode(700)
        );
    }

    #[test]
    fn end_anchored_music_fit_fails_closed_when_target_cannot_be_reached() {
        let fps = Rational::new(30, 1).unwrap();
        let document = Document {
            tracks: vec![Track {
                id: TrackId(7),
                kind: TrackKind::Audio,
                sync_lock: true,
                clips: Vec::new(),
            }],
            media_pool: vec![asset(2, MediaKind::Audio, 6_170, fps)],
            fps,
            ..Document::default()
        };
        let status = beat_status(2, 6_170, &[(5_160, 5_638)]);

        let error = music_fit_plan_with_end_anchor(
            &document,
            TrackId(7),
            AssetId(2),
            TimeCode::ZERO..TimeCode(700),
            Some(TimeCode(5_160)),
            Some(MusicEndAnchor {
                preferred_source_end: TimeCode(6_001),
                maximum_drift_frames: TimeCode::ZERO,
            }),
            &status,
            0,
            ThreePointMode::Overwrite,
        )
        .unwrap_err();

        assert_eq!(
            error,
            CreatorPlanError::MusicEndAnchorUnsatisfied {
                asset: AssetId(2),
                target_source_end: TimeCode(6_001),
                maximum_drift_frames: TimeCode::ZERO,
            }
        );
    }

    #[test]
    fn end_anchored_music_fit_rejects_negative_drift() {
        let fps = Rational::new(30, 1).unwrap();
        let document = Document {
            tracks: vec![Track {
                id: TrackId(7),
                kind: TrackKind::Audio,
                sync_lock: true,
                clips: Vec::new(),
            }],
            media_pool: vec![asset(2, MediaKind::Audio, 600, fps)],
            fps,
            ..Document::default()
        };
        let status = beat_status(2, 600, &[(30, 9_000)]);

        let error = music_fit_plan_with_end_anchor(
            &document,
            TrackId(7),
            AssetId(2),
            TimeCode::ZERO..TimeCode(60),
            None,
            Some(MusicEndAnchor {
                preferred_source_end: TimeCode(90),
                maximum_drift_frames: TimeCode(-1),
            }),
            &status,
            0,
            ThreePointMode::Overwrite,
        )
        .unwrap_err();

        assert_eq!(
            error,
            CreatorPlanError::NegativeMusicEndAnchorDrift(TimeCode(-1))
        );
    }
}
