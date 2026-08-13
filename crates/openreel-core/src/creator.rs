use std::{cmp::Reverse, ops::Range};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    AssetId, BeatStatus, ClipId, Document, OpError, Operation, ThreePointMode, TimeCode,
    TimelineBeat, TrackId, map_source_range_to_project,
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

/// How a music-fit plan uses the selected material.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MusicFitStrategy {
    /// Start on one detected beat and make a straight, real-time edit whose
    /// duration exactly matches the requested project range.
    BeatAnchoredStraightCut,
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
    pub operations: Vec<Operation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CreatorPlanError {
    #[error("clip {0} does not exist")]
    MissingClip(ClipId),
    #[error("clip {0} is not media")]
    NonMediaClip(ClipId),
    #[error("asset {0} does not exist")]
    MissingAsset(AssetId),
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
        "no eligible beat in asset {asset} leaves enough real-time source for the target range"
    )]
    InsufficientMusicSource { asset: AssetId },
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

    let project_duration = timeline_range
        .end
        .checked_sub(timeline_range.start)
        .ok_or(OpError::TimeOverflow)?;
    let (anchor_beat, source_out) = select_music_source(
        media,
        beats,
        preferred,
        minimum_strength_basis_points,
        document.fps,
        project_duration,
    )?;
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
        strategy: MusicFitStrategy::BeatAnchoredStraightCut,
        duration_fit: MusicDurationFit::ExactProjectRange,
        playback: MusicPlaybackMode::RealTime,
        repeat: MusicRepeatMode::None,
        source_end_alignment,
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
}
