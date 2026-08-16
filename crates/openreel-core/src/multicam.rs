use std::{collections::BTreeMap, ops::Range};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    AssetId, AssetTranscript, AutomationCurve, ClipContent, ClipId, Document, EffectId, Keyframe,
    KeyframeInterpolation, Operation, SyncGroupId, ThreePointMode, TimeCode, TimeMappingError,
    TrackId, TrackKind, apply_batch, map_frames,
};

/// One explicit mapping from a diarization label to a named sync-group angle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SpeakerAngleAssignment {
    pub speaker: String,
    pub angle_name: String,
}

/// Inputs for deterministic speaker-aware multicam planning.
///
/// Sync-group positions and offsets are project frames. `group_start` is mapped
/// onto `record_start`; the source transcript supplies the switch timing, while
/// each assignment chooses the source angle to place on the target video track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SpeakerMulticamSettings {
    pub sync_group: SyncGroupId,
    pub target_track: TrackId,
    pub group_start: TimeCode,
    pub group_end: TimeCode,
    pub record_start: TimeCode,
    /// Merge words assigned to the same angle across gaps no larger than this.
    pub maximum_word_gap_frames: TimeCode,
    /// Suppress shots shorter than this rather than producing rapid cuts.
    pub minimum_shot_frames: TimeCode,
    pub assignments: Vec<SpeakerAngleAssignment>,
}

/// One chronological shot selected by a speaker-aware multicam plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SpeakerMulticamCut {
    pub speakers: Vec<String>,
    pub angle_name: String,
    pub asset: AssetId,
    pub group_start: TimeCode,
    pub group_end: TimeCode,
    pub timeline_start: TimeCode,
    pub timeline_end: TimeCode,
    pub source_in: TimeCode,
}

/// A non-mutating, directly applicable speaker-aware multicam edit plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SpeakerMulticamPlan {
    pub sync_group: SyncGroupId,
    pub target_track: TrackId,
    pub reference_asset: AssetId,
    pub group_start: TimeCode,
    pub group_end: TimeCode,
    pub record_start: TimeCode,
    pub suppressed_short_shots: usize,
    /// Cuts stay chronological for inspection.
    pub cuts: Vec<SpeakerMulticamCut>,
    /// Overwrites run latest-first so generated clip ids cannot affect earlier ranges.
    pub operations: Vec<Operation>,
}

/// Failures that prevent an honest speaker-aware multicam plan.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SpeakerMulticamError {
    #[error("sync group {0} does not exist")]
    MissingSyncGroup(SyncGroupId),
    #[error("target track {0} does not exist")]
    MissingTargetTrack(TrackId),
    #[error("speaker-aware multicam requires a video target track, but track {0} is audio")]
    TargetTrackIsAudio(TrackId),
    #[error("group range must be non-empty, got {start}..{end}")]
    InvalidGroupRange { start: TimeCode, end: TimeCode },
    #[error("record start must be non-negative, got {0}")]
    InvalidRecordStart(TimeCode),
    #[error("maximum word gap and minimum shot length must be non-negative")]
    InvalidTimingPolicy,
    #[error("at least one speaker-to-angle assignment is required")]
    MissingAssignments,
    #[error("speaker and angle names in assignments must be non-empty")]
    EmptyAssignment,
    #[error("speaker {0:?} is assigned more than once")]
    DuplicateSpeakerAssignment(String),
    #[error("sync group {group} contains duplicate normalized angle name {angle:?}")]
    DuplicateAngleName { group: SyncGroupId, angle: String },
    #[error("angle {0:?} does not exist in the sync group")]
    UnknownAngle(String),
    #[error("asset {0} does not exist")]
    MissingAsset(AssetId),
    #[error("angle {angle:?} uses asset {asset}, which has no video stream")]
    AngleHasNoVideo { angle: String, asset: AssetId },
    #[error("transcript asset {0} is not a member of the sync group")]
    ReferenceAssetNotInGroup(AssetId),
    #[error("transcript and asset frame rates do not match")]
    TranscriptRateMismatch,
    #[error("transcript word {index} has invalid source range {start}..{end}")]
    InvalidTranscriptWord {
        index: usize,
        start: TimeCode,
        end: TimeCode,
    },
    #[error("transcript word {index} in the requested range has no diarization label")]
    MissingSpeakerLabel { index: usize },
    #[error("speaker {0:?} has no angle assignment")]
    UnmappedSpeaker(String),
    #[error("different assigned angles overlap in the transcript near group frame {0}")]
    OverlappingSpeakers(TimeCode),
    #[error("no speaker words overlap the requested group range")]
    NoSpeakerWords,
    #[error("all detected speaker shots were shorter than the minimum shot length")]
    AllShotsSuppressed,
    #[error(
        "angle {angle:?} does not cover group frame {group_frame}; its source begins at group frame {angle_offset}"
    )]
    AngleDoesNotCover {
        angle: String,
        group_frame: TimeCode,
        angle_offset: TimeCode,
    },
    #[error("multicam time calculation overflowed")]
    TimeOverflow,
    #[error("the generated multicam plan is not applicable: {0}")]
    PlanNotApplicable(String),
    #[error(transparent)]
    TimeMapping(#[from] TimeMappingError),
}

#[derive(Debug, Clone)]
struct RawTurn {
    speakers: Vec<String>,
    angle_key: String,
    angle_name: String,
    start: TimeCode,
    end: TimeCode,
}

/// Build an overwrite plan from real diarization labels and existing sync metadata.
///
/// This function performs no analysis and never claims to infer speakers. It
/// requires a transcript with populated labels, validates the complete plan on
/// a clone, and leaves `document` unchanged.
///
/// # Errors
///
/// Returns an error for missing diarization or sync data, ambiguous mappings,
/// uncovered source ranges, invalid timing, or operations the current timeline
/// cannot apply atomically.
#[allow(clippy::too_many_lines)]
pub fn plan_speaker_multicam(
    document: &Document,
    transcript: &AssetTranscript,
    settings: &SpeakerMulticamSettings,
) -> Result<SpeakerMulticamPlan, SpeakerMulticamError> {
    if settings.group_end <= settings.group_start {
        return Err(SpeakerMulticamError::InvalidGroupRange {
            start: settings.group_start,
            end: settings.group_end,
        });
    }
    if settings.record_start < TimeCode::ZERO {
        return Err(SpeakerMulticamError::InvalidRecordStart(
            settings.record_start,
        ));
    }
    if settings.maximum_word_gap_frames < TimeCode::ZERO
        || settings.minimum_shot_frames < TimeCode::ZERO
    {
        return Err(SpeakerMulticamError::InvalidTimingPolicy);
    }
    let group = document
        .catalog
        .sync_groups
        .iter()
        .find(|group| group.id == settings.sync_group)
        .ok_or(SpeakerMulticamError::MissingSyncGroup(settings.sync_group))?;
    let target = document
        .tracks
        .iter()
        .find(|track| track.id == settings.target_track)
        .ok_or(SpeakerMulticamError::MissingTargetTrack(
            settings.target_track,
        ))?;
    if target.kind != TrackKind::Video {
        return Err(SpeakerMulticamError::TargetTrackIsAudio(target.id));
    }

    let mut angles = BTreeMap::new();
    for member in &group.members {
        let key = normalized_name(&member.angle_name);
        if angles.insert(key.clone(), member).is_some() {
            return Err(SpeakerMulticamError::DuplicateAngleName {
                group: group.id,
                angle: member.angle_name.clone(),
            });
        }
        document
            .asset(member.asset)
            .ok_or(SpeakerMulticamError::MissingAsset(member.asset))?;
    }
    let reference = group
        .members
        .iter()
        .find(|member| member.asset == transcript.asset)
        .ok_or(SpeakerMulticamError::ReferenceAssetNotInGroup(
            transcript.asset,
        ))?;
    let reference_asset = document
        .asset(reference.asset)
        .ok_or(SpeakerMulticamError::MissingAsset(reference.asset))?;
    if transcript.source_fps != reference_asset.fps {
        return Err(SpeakerMulticamError::TranscriptRateMismatch);
    }

    if settings.assignments.is_empty() {
        return Err(SpeakerMulticamError::MissingAssignments);
    }
    let mut assignments = BTreeMap::new();
    for assignment in &settings.assignments {
        let speaker_key = normalized_name(&assignment.speaker);
        let angle_key = normalized_name(&assignment.angle_name);
        if speaker_key.is_empty() || angle_key.is_empty() {
            return Err(SpeakerMulticamError::EmptyAssignment);
        }
        if !angles.contains_key(&angle_key) {
            return Err(SpeakerMulticamError::UnknownAngle(
                assignment.angle_name.clone(),
            ));
        }
        let member = angles
            .get(&angle_key)
            .ok_or_else(|| SpeakerMulticamError::UnknownAngle(assignment.angle_name.clone()))?;
        let asset = document
            .asset(member.asset)
            .ok_or(SpeakerMulticamError::MissingAsset(member.asset))?;
        if !asset.kind.supports(TrackKind::Video) {
            return Err(SpeakerMulticamError::AngleHasNoVideo {
                angle: member.angle_name.clone(),
                asset: member.asset,
            });
        }
        if assignments
            .insert(
                speaker_key,
                (angle_key, assignment.speaker.trim().to_owned()),
            )
            .is_some()
        {
            return Err(SpeakerMulticamError::DuplicateSpeakerAssignment(
                assignment.speaker.clone(),
            ));
        }
    }

    let requested = settings.group_start..settings.group_end;
    // Build turns per angle first. Real diarization contains short backchannels
    // and overlapping acknowledgements; merging globally would make one brief
    // interjection split an otherwise continuous primary-speaker turn before
    // the minimum-shot policy gets a chance to suppress it.
    let mut words_by_angle: BTreeMap<String, Vec<RawTurn>> = BTreeMap::new();
    for (index, word) in transcript.words.iter().enumerate() {
        if word.source_start < TimeCode::ZERO
            || word.source_end <= word.source_start
            || word.source_end > reference_asset.duration
        {
            return Err(SpeakerMulticamError::InvalidTranscriptWord {
                index,
                start: word.source_start,
                end: word.source_end,
            });
        }
        let mapped_start = map_frames(word.source_start, transcript.source_fps, document.fps)?;
        let mapped_end = map_frames(word.source_end, transcript.source_fps, document.fps)?;
        let group_word =
            add_offset(mapped_start, reference.offset)?..add_offset(mapped_end, reference.offset)?;
        let Some(intersection) = intersect(group_word, requested.clone()) else {
            continue;
        };
        let speaker = word
            .speaker
            .as_deref()
            .map(str::trim)
            .filter(|speaker| !speaker.is_empty())
            .ok_or(SpeakerMulticamError::MissingSpeakerLabel { index })?;
        let speaker_key = normalized_name(speaker);
        let (angle_key, display_speaker) = assignments
            .get(&speaker_key)
            .ok_or_else(|| SpeakerMulticamError::UnmappedSpeaker(speaker.to_owned()))?;
        let member = angles
            .get(angle_key)
            .ok_or_else(|| SpeakerMulticamError::UnknownAngle(angle_key.clone()))?;
        let raw = RawTurn {
            speakers: vec![display_speaker.clone()],
            angle_key: angle_key.clone(),
            angle_name: member.angle_name.clone(),
            start: intersection.start,
            end: intersection.end,
        };
        let angle_turns = words_by_angle.entry(angle_key.clone()).or_default();
        if let Some(previous) = angle_turns.last_mut() {
            let merge_limit = add_offset(previous.end, settings.maximum_word_gap_frames)?;
            if raw.start <= merge_limit {
                previous.end = previous.end.max(raw.end);
                for speaker in raw.speakers {
                    if !previous.speakers.contains(&speaker) {
                        previous.speakers.push(speaker);
                    }
                }
                continue;
            }
        }
        angle_turns.push(raw);
    }
    if words_by_angle.is_empty() {
        return Err(SpeakerMulticamError::NoSpeakerWords);
    }
    let mut turns = words_by_angle.into_values().flatten().collect::<Vec<_>>();
    turns.sort_by(|left, right| {
        (left.start, left.end, &left.angle_key, &left.speakers).cmp(&(
            right.start,
            right.end,
            &right.angle_key,
            &right.speakers,
        ))
    });

    let before_suppression = turns.len();
    turns.retain(|turn| {
        turn.end
            .checked_sub(turn.start)
            .is_some_and(|duration| duration >= settings.minimum_shot_frames)
    });
    let suppressed_short_shots = before_suppression - turns.len();
    if turns.is_empty() {
        return Err(SpeakerMulticamError::AllShotsSuppressed);
    }

    // Once short backchannels are gone, retained cross-angle overlap is a real
    // ambiguity and remains an explicit error. Adjacent retained turns on the
    // same angle can safely absorb their intervening silence as one shot.
    let mut retained: Vec<RawTurn> = Vec::with_capacity(turns.len());
    for turn in turns {
        if let Some(previous) = retained.last_mut() {
            if turn.start < previous.end && turn.angle_key != previous.angle_key {
                return Err(SpeakerMulticamError::OverlappingSpeakers(turn.start));
            }
            if turn.angle_key == previous.angle_key {
                previous.end = previous.end.max(turn.end);
                for speaker in turn.speakers {
                    if !previous.speakers.contains(&speaker) {
                        previous.speakers.push(speaker);
                    }
                }
                continue;
            }
        }
        retained.push(turn);
    }

    // Cover the complete requested range. Cuts land halfway through silence
    // between speakers, so the plan never flashes back to the placeholder
    // angle during ordinary conversational gaps.
    let mut coverage_start = settings.group_start;
    for index in 0..retained.len() {
        let coverage_end = retained.get(index + 1).map_or(settings.group_end, |next| {
            let silence = next.start.0.saturating_sub(retained[index].end.0);
            TimeCode(retained[index].end.0.saturating_add(silence / 2))
        });
        retained[index].start = coverage_start;
        retained[index].end = coverage_end;
        coverage_start = coverage_end;
    }

    let mut cuts = Vec::with_capacity(retained.len());
    for turn in retained {
        let member = angles
            .get(&turn.angle_key)
            .ok_or_else(|| SpeakerMulticamError::UnknownAngle(turn.angle_name.clone()))?;
        let asset = document
            .asset(member.asset)
            .ok_or(SpeakerMulticamError::MissingAsset(member.asset))?;
        let angle_project_frame = turn
            .start
            .checked_sub(member.offset)
            .ok_or(SpeakerMulticamError::TimeOverflow)?;
        if angle_project_frame < TimeCode::ZERO {
            return Err(SpeakerMulticamError::AngleDoesNotCover {
                angle: member.angle_name.clone(),
                group_frame: turn.start,
                angle_offset: member.offset,
            });
        }
        let source_in = map_frames(angle_project_frame, document.fps, asset.fps)?;
        if source_in >= asset.duration {
            return Err(SpeakerMulticamError::AngleDoesNotCover {
                angle: member.angle_name.clone(),
                group_frame: turn.start,
                angle_offset: member.offset,
            });
        }
        let record_offset = turn
            .start
            .checked_sub(settings.group_start)
            .ok_or(SpeakerMulticamError::TimeOverflow)?;
        let timeline_start = add_offset(settings.record_start, record_offset)?;
        let shot_duration = turn
            .end
            .checked_sub(turn.start)
            .ok_or(SpeakerMulticamError::TimeOverflow)?;
        let timeline_end = add_offset(timeline_start, shot_duration)?;
        cuts.push(SpeakerMulticamCut {
            speakers: turn.speakers,
            angle_name: turn.angle_name,
            asset: member.asset,
            group_start: turn.start,
            group_end: turn.end,
            timeline_start,
            timeline_end,
            source_in,
        });
    }

    let operations = cuts
        .iter()
        .rev()
        .map(|cut| Operation::ThreePointEdit {
            track: settings.target_track,
            asset: cut.asset,
            source_in: Some(cut.source_in),
            source_out: None,
            timeline_in: Some(cut.timeline_start),
            timeline_out: Some(cut.timeline_end),
            mode: ThreePointMode::Overwrite,
        })
        .collect::<Vec<_>>();
    let mut candidate = document.clone();
    apply_batch(&mut candidate, &operations)
        .map_err(|error| SpeakerMulticamError::PlanNotApplicable(error.to_string()))?;

    Ok(SpeakerMulticamPlan {
        sync_group: settings.sync_group,
        target_track: settings.target_track,
        reference_asset: reference.asset,
        group_start: settings.group_start,
        group_end: settings.group_end,
        record_start: settings.record_start,
        suppressed_short_shots,
        cuts,
        operations,
    })
}

/// Inclusive focus bounds for subject-aware reframe automation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReframeFocusBounds {
    pub min_x_percent: i64,
    pub max_x_percent: i64,
    pub min_y_percent: i64,
    pub max_y_percent: i64,
}

impl Default for ReframeFocusBounds {
    fn default() -> Self {
        Self {
            min_x_percent: 0,
            max_x_percent: 100,
            min_y_percent: 0,
            max_y_percent: 100,
        }
    }
}

/// One externally observed subject center in clip-local coordinates.
///
/// The planner consumes these observations; it does not detect or identify a
/// subject itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubjectCenterSample {
    pub at: TimeCode,
    pub x_percent: i64,
    pub y_percent: i64,
    pub confidence_basis_points: u16,
}

/// One externally observed subject center with 0.01 percent coordinate precision.
///
/// Coordinate basis points span 0..=10000 across each frame axis. Observations
/// outside that range are accepted and clamped to the configured safe region by
/// [`plan_subject_reframe_basis_points`], matching the legacy percent planner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubjectCenterBasisPointSample {
    pub at: TimeCode,
    pub x_basis_points: i64,
    pub y_basis_points: i64,
    pub confidence_basis_points: u16,
}

/// Inclusive virtual-camera focus interval that keeps one tracked subject box
/// inside the delivery crop at a sample frame.
///
/// This is deliberately expressed in basis points and supplied by the caller:
/// the core planner remains media-backend agnostic while a tracker can account
/// for its source aspect, crop geometry, and subject bounds exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubjectFocusBasisPointConstraint {
    pub at: TimeCode,
    pub min_x_basis_points: i64,
    pub max_x_basis_points: i64,
    pub min_y_basis_points: i64,
    pub max_y_basis_points: i64,
}

/// Configuration for converting subject observations into editable reframe curves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubjectReframeSettings {
    pub clip: ClipId,
    pub effect: EffectId,
    pub bounds: ReframeFocusBounds,
    pub minimum_confidence_basis_points: u16,
    /// Subject movement tolerated before the virtual camera follows, in frame percent.
    #[serde(default = "default_focus_dead_zone_percent")]
    #[schemars(default = "default_focus_dead_zone_percent")]
    pub focus_dead_zone_percent: u8,
    /// Largest focus change allowed between adjacent observations, in frame percent.
    #[serde(default = "default_maximum_focus_step_percent")]
    #[schemars(default = "default_maximum_focus_step_percent")]
    pub maximum_focus_step_percent: u8,
}

const fn default_focus_dead_zone_percent() -> u8 {
    6
}

const fn default_maximum_focus_step_percent() -> u8 {
    2
}

/// A validated, non-mutating reframe automation plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubjectReframePlan {
    pub clip: ClipId,
    pub effect: EffectId,
    pub samples: Vec<SubjectCenterSample>,
    pub clamped_samples: usize,
    pub focus_x_curve: AutomationCurve,
    pub focus_y_curve: AutomationCurve,
    pub operations: Vec<Operation>,
}

/// A precise, validated reframe plan whose samples and curves use basis points.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubjectReframeBasisPointPlan {
    pub clip: ClipId,
    pub effect: EffectId,
    pub samples: Vec<SubjectCenterBasisPointSample>,
    pub clamped_samples: usize,
    pub focus_x_curve: AutomationCurve,
    pub focus_y_curve: AutomationCurve,
    pub operations: Vec<Operation>,
}

/// Failures that prevent subject observations from becoming reframe automation.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SubjectReframeError {
    #[error("clip {0} does not exist")]
    MissingClip(ClipId),
    #[error("subject-aware reframe requires a media clip")]
    RequiresMediaClip,
    #[error("effect {effect} does not exist on clip {clip}")]
    MissingEffect { clip: ClipId, effect: EffectId },
    #[error("effect {effect} is {actual:?}; subject automation requires reframe")]
    WrongEffect { effect: EffectId, actual: String },
    #[error("at least one subject center sample is required")]
    MissingSamples,
    #[error("focus bounds must be ordered and within 0..=100")]
    InvalidBounds,
    #[error("minimum confidence must be in 0..=10000 basis points")]
    InvalidConfidenceThreshold,
    #[error("focus dead zone must be in 0..=25 percent")]
    InvalidDeadZone,
    #[error("maximum focus step must be in 1..=25 percent")]
    InvalidMaximumStep,
    #[error("sample confidence must be in 0..=10000 basis points at frame {0}")]
    InvalidSampleConfidence(TimeCode),
    #[error("sample at frame {frame} has confidence {actual}, below the required {minimum}")]
    LowConfidence {
        frame: TimeCode,
        actual: u16,
        minimum: u16,
    },
    #[error("sample frame {frame} is outside clip-local range 0..{duration}")]
    SampleOutsideClip { frame: TimeCode, duration: TimeCode },
    #[error("more than one subject sample targets clip-local frame {0}")]
    DuplicateSampleFrame(TimeCode),
    #[error("more than one containment constraint targets clip-local frame {0}")]
    DuplicateContainmentConstraint(TimeCode),
    #[error("no containment constraint was supplied for tracked sample frame {0}")]
    MissingContainmentConstraint(TimeCode),
    #[error("containment constraint targets untracked clip-local frame {0}")]
    UnexpectedContainmentConstraint(TimeCode),
    #[error(
        "containment constraint at frame {frame} has invalid {axis} interval {minimum}..={maximum}"
    )]
    InvalidContainmentConstraint {
        frame: TimeCode,
        axis: &'static str,
        minimum: i64,
        maximum: i64,
    },
    #[error(
        "containment constraint at frame {frame} does not fit the configured {axis} focus bounds"
    )]
    ContainmentOutsideFocusBounds { frame: TimeCode, axis: &'static str },
    #[error(
        "full tracked-subject containment is infeasible on {axis} at frame {frame}: allowed focus {allowed_minimum}..={allowed_maximum}, reachable with the configured maximum step {reachable_minimum}..={reachable_maximum}"
    )]
    ContainmentInfeasible {
        frame: TimeCode,
        axis: &'static str,
        allowed_minimum: i64,
        allowed_maximum: i64,
        reachable_minimum: i64,
        reachable_maximum: i64,
    },
    #[error("the generated reframe plan is not applicable: {0}")]
    PlanNotApplicable(String),
}

/// Convert observed subject centers into bounded, editable reframe keyframes.
///
/// Samples are sorted by clip-local frame, confidence-gated, median-filtered,
/// clamped to the requested safe focus region, and converted into dead-zone,
/// speed-limited linear camera motion. Two normal `SetEffectKeyframes`
/// operations are returned for revision-gated application by the caller.
///
/// # Errors
///
/// Returns an error for a missing reframe effect, invalid bounds, duplicate or
/// out-of-range samples, low confidence, or an inapplicable generated plan.
#[allow(clippy::too_many_lines)]
pub fn plan_subject_reframe(
    document: &Document,
    settings: SubjectReframeSettings,
    samples: &[SubjectCenterSample],
) -> Result<SubjectReframePlan, SubjectReframeError> {
    let samples = samples
        .iter()
        .map(|sample| ScaledSubjectCenterSample {
            at: sample.at,
            x: sample.x_percent,
            y: sample.y_percent,
            confidence_basis_points: sample.confidence_basis_points,
        })
        .collect::<Vec<_>>();
    let plan = plan_subject_reframe_scaled(
        document,
        settings,
        &samples,
        1,
        ["focus_x_percent", "focus_y_percent"],
        None,
    )?;

    Ok(SubjectReframePlan {
        clip: settings.clip,
        effect: settings.effect,
        samples: plan
            .samples
            .into_iter()
            .map(|sample| SubjectCenterSample {
                at: sample.at,
                x_percent: sample.x,
                y_percent: sample.y,
                confidence_basis_points: sample.confidence_basis_points,
            })
            .collect(),
        clamped_samples: plan.clamped_samples,
        focus_x_curve: plan.focus_x_curve,
        focus_y_curve: plan.focus_y_curve,
        operations: plan.operations,
    })
}

/// Convert precise subject centers into bounded basis-point reframe keyframes.
///
/// This is the non-quantizing counterpart to [`plan_subject_reframe`]. The
/// existing percent settings remain the public tuning surface and are scaled by
/// 100 internally, so a six-percent dead zone becomes 600 basis points and a
/// two-percent maximum step becomes 200 basis points. Generated operations
/// target `focus_x_basis_points` and `focus_y_basis_points`; renderers therefore
/// retain every 0.01 percent of tracker precision.
///
/// # Errors
///
/// Returns the same validation failures as [`plan_subject_reframe`].
pub fn plan_subject_reframe_basis_points(
    document: &Document,
    settings: SubjectReframeSettings,
    samples: &[SubjectCenterBasisPointSample],
) -> Result<SubjectReframeBasisPointPlan, SubjectReframeError> {
    let samples = samples
        .iter()
        .map(|sample| ScaledSubjectCenterSample {
            at: sample.at,
            x: sample.x_basis_points,
            y: sample.y_basis_points,
            confidence_basis_points: sample.confidence_basis_points,
        })
        .collect::<Vec<_>>();
    let plan = plan_subject_reframe_scaled(
        document,
        settings,
        &samples,
        100,
        ["focus_x_basis_points", "focus_y_basis_points"],
        None,
    )?;

    Ok(SubjectReframeBasisPointPlan {
        clip: settings.clip,
        effect: settings.effect,
        samples: plan
            .samples
            .into_iter()
            .map(|sample| SubjectCenterBasisPointSample {
                at: sample.at,
                x_basis_points: sample.x,
                y_basis_points: sample.y,
                confidence_basis_points: sample.confidence_basis_points,
            })
            .collect(),
        clamped_samples: plan.clamped_samples,
        focus_x_curve: plan.focus_x_curve,
        focus_y_curve: plan.focus_y_curve,
        operations: plan.operations,
    })
}

/// Plan precise, editable reframe automation with hard per-sample containment.
///
/// The controller knows every supplied future interval before emitting a curve.
/// It therefore begins a bounded move before a future crop edge would exclude
/// the subject, rather than reacting only after the tracker has already moved.
/// A configured maximum step remains a hard nominal speed limit whenever a
/// full-containment path exists. When no such path exists, this returns an
/// explicit error instead of silently generating an edge-clipped crop.
///
/// # Errors
///
/// Returns [`SubjectReframeError::ContainmentInfeasible`] when the supplied
/// intervals cannot be reached under the configured maximum focus step.
#[allow(clippy::too_many_lines)]
pub fn plan_subject_reframe_basis_points_with_containment(
    document: &Document,
    settings: SubjectReframeSettings,
    samples: &[SubjectCenterBasisPointSample],
    containment: &[SubjectFocusBasisPointConstraint],
) -> Result<SubjectReframeBasisPointPlan, SubjectReframeError> {
    let samples = samples
        .iter()
        .map(|sample| ScaledSubjectCenterSample {
            at: sample.at,
            x: sample.x_basis_points,
            y: sample.y_basis_points,
            confidence_basis_points: sample.confidence_basis_points,
        })
        .collect::<Vec<_>>();
    let plan = plan_subject_reframe_scaled(
        document,
        settings,
        &samples,
        100,
        ["focus_x_basis_points", "focus_y_basis_points"],
        Some(containment),
    )?;

    Ok(SubjectReframeBasisPointPlan {
        clip: settings.clip,
        effect: settings.effect,
        samples: plan
            .samples
            .into_iter()
            .map(|sample| SubjectCenterBasisPointSample {
                at: sample.at,
                x_basis_points: sample.x,
                y_basis_points: sample.y,
                confidence_basis_points: sample.confidence_basis_points,
            })
            .collect(),
        clamped_samples: plan.clamped_samples,
        focus_x_curve: plan.focus_x_curve,
        focus_y_curve: plan.focus_y_curve,
        operations: plan.operations,
    })
}

#[derive(Debug, Clone, Copy)]
struct ScaledSubjectCenterSample {
    at: TimeCode,
    x: i64,
    y: i64,
    confidence_basis_points: u16,
}

#[derive(Debug, Clone, Copy)]
struct ScaledFocusConstraint {
    min_x: i64,
    max_x: i64,
    min_y: i64,
    max_y: i64,
}

struct ScaledSubjectReframePlan {
    samples: Vec<ScaledSubjectCenterSample>,
    clamped_samples: usize,
    focus_x_curve: AutomationCurve,
    focus_y_curve: AutomationCurve,
    operations: Vec<Operation>,
}

#[allow(clippy::too_many_lines)]
fn plan_subject_reframe_scaled(
    document: &Document,
    settings: SubjectReframeSettings,
    samples: &[ScaledSubjectCenterSample],
    coordinate_scale: i64,
    focus_parameters: [&str; 2],
    containment: Option<&[SubjectFocusBasisPointConstraint]>,
) -> Result<ScaledSubjectReframePlan, SubjectReframeError> {
    let clip = document
        .clip(settings.clip)
        .ok_or(SubjectReframeError::MissingClip(settings.clip))?;
    if !matches!(clip.content, ClipContent::Media) {
        return Err(SubjectReframeError::RequiresMediaClip);
    }
    let effect = clip
        .effects
        .iter()
        .find(|effect| effect.id == settings.effect)
        .ok_or(SubjectReframeError::MissingEffect {
            clip: settings.clip,
            effect: settings.effect,
        })?;
    if effect.name != "reframe" {
        return Err(SubjectReframeError::WrongEffect {
            effect: settings.effect,
            actual: effect.name.clone(),
        });
    }
    if samples.is_empty() {
        return Err(SubjectReframeError::MissingSamples);
    }
    let bounds = settings.bounds;
    if bounds.min_x_percent < 0
        || bounds.max_x_percent > 100
        || bounds.min_y_percent < 0
        || bounds.max_y_percent > 100
        || bounds.min_x_percent > bounds.max_x_percent
        || bounds.min_y_percent > bounds.max_y_percent
    {
        return Err(SubjectReframeError::InvalidBounds);
    }
    if settings.minimum_confidence_basis_points > 10_000 {
        return Err(SubjectReframeError::InvalidConfidenceThreshold);
    }
    if settings.focus_dead_zone_percent > 25 {
        return Err(SubjectReframeError::InvalidDeadZone);
    }
    if !(1..=25).contains(&settings.maximum_focus_step_percent) {
        return Err(SubjectReframeError::InvalidMaximumStep);
    }
    let duration = document
        .clip_duration(clip)
        .map_err(|error| SubjectReframeError::PlanNotApplicable(error.to_string()))?;
    let mut normalized = samples.to_vec();
    normalized.sort_by_key(|sample| sample.at);
    let scaled = |value: i64| value.saturating_mul(coordinate_scale);
    let min_x = scaled(bounds.min_x_percent);
    let max_x = scaled(bounds.max_x_percent);
    let min_y = scaled(bounds.min_y_percent);
    let max_y = scaled(bounds.max_y_percent);
    let mut clamped_samples = 0;
    for index in 0..normalized.len() {
        if index > 0 && normalized[index - 1].at == normalized[index].at {
            return Err(SubjectReframeError::DuplicateSampleFrame(
                normalized[index].at,
            ));
        }
        let sample = &mut normalized[index];
        if sample.confidence_basis_points > 10_000 {
            return Err(SubjectReframeError::InvalidSampleConfidence(sample.at));
        }
        if sample.confidence_basis_points < settings.minimum_confidence_basis_points {
            return Err(SubjectReframeError::LowConfidence {
                frame: sample.at,
                actual: sample.confidence_basis_points,
                minimum: settings.minimum_confidence_basis_points,
            });
        }
        if sample.at < TimeCode::ZERO || sample.at >= duration {
            return Err(SubjectReframeError::SampleOutsideClip {
                frame: sample.at,
                duration,
            });
        }
        let bounded_x = sample.x.clamp(min_x, max_x);
        let bounded_y = sample.y.clamp(min_y, max_y);
        if bounded_x != sample.x || bounded_y != sample.y {
            clamped_samples += 1;
        }
        sample.x = bounded_x;
        sample.y = bounded_y;
    }
    let normalized_containment = containment
        .map(|constraints| {
            normalize_containment_constraints(&normalized, constraints, min_x, max_x, min_y, max_y)
        })
        .transpose()?;
    let frames = normalized
        .iter()
        .map(|sample| sample.at)
        .collect::<Vec<_>>();
    let horizontal_observations = normalized.iter().map(|sample| sample.x).collect::<Vec<_>>();
    let vertical_observations = normalized.iter().map(|sample| sample.y).collect::<Vec<_>>();
    let dead_zone = scaled(i64::from(settings.focus_dead_zone_percent));
    let maximum_step = scaled(i64::from(settings.maximum_focus_step_percent));
    let horizontal_values = if let Some(constraints) = &normalized_containment {
        let intervals = constraints
            .iter()
            .map(|constraint| (constraint.min_x, constraint.max_x))
            .collect::<Vec<_>>();
        containment_aware_focus_values(
            &horizontal_observations,
            &intervals,
            &frames,
            "horizontal",
            min_x,
            max_x,
            dead_zone,
            maximum_step,
        )?
    } else {
        stabilized_focus_values(
            &horizontal_observations,
            min_x,
            max_x,
            dead_zone,
            maximum_step,
        )
    };
    let vertical_values = if let Some(constraints) = &normalized_containment {
        let intervals = constraints
            .iter()
            .map(|constraint| (constraint.min_y, constraint.max_y))
            .collect::<Vec<_>>();
        containment_aware_focus_values(
            &vertical_observations,
            &intervals,
            &frames,
            "vertical",
            min_y,
            max_y,
            dead_zone,
            maximum_step,
        )?
    } else {
        stabilized_focus_values(
            &vertical_observations,
            min_y,
            max_y,
            dead_zone,
            maximum_step,
        )
    };
    let curve_for = |values: Vec<i64>| AutomationCurve {
        keyframes: normalized
            .iter()
            .zip(values)
            .map(|(sample, value)| Keyframe {
                at: sample.at,
                value,
                interpolation: KeyframeInterpolation::Linear,
            })
            .collect(),
    };
    let horizontal_curve = curve_for(horizontal_values);
    let vertical_curve = curve_for(vertical_values);
    let operations = vec![
        Operation::SetEffectKeyframes {
            clip: settings.clip,
            effect: settings.effect,
            name: focus_parameters[0].to_owned(),
            curve: horizontal_curve.clone(),
        },
        Operation::SetEffectKeyframes {
            clip: settings.clip,
            effect: settings.effect,
            name: focus_parameters[1].to_owned(),
            curve: vertical_curve.clone(),
        },
    ];
    let mut candidate = document.clone();
    apply_batch(&mut candidate, &operations)
        .map_err(|error| SubjectReframeError::PlanNotApplicable(error.to_string()))?;

    Ok(ScaledSubjectReframePlan {
        samples: normalized,
        clamped_samples,
        focus_x_curve: horizontal_curve,
        focus_y_curve: vertical_curve,
        operations,
    })
}

fn normalize_containment_constraints(
    samples: &[ScaledSubjectCenterSample],
    constraints: &[SubjectFocusBasisPointConstraint],
    minimum_x: i64,
    maximum_x: i64,
    minimum_y: i64,
    maximum_y: i64,
) -> Result<Vec<ScaledFocusConstraint>, SubjectReframeError> {
    let mut by_frame = BTreeMap::new();
    for constraint in constraints {
        if by_frame.insert(constraint.at, *constraint).is_some() {
            return Err(SubjectReframeError::DuplicateContainmentConstraint(
                constraint.at,
            ));
        }
        for (axis, minimum, maximum) in [
            (
                "horizontal",
                constraint.min_x_basis_points,
                constraint.max_x_basis_points,
            ),
            (
                "vertical",
                constraint.min_y_basis_points,
                constraint.max_y_basis_points,
            ),
        ] {
            if minimum < 0 || maximum > 10_000 || minimum > maximum {
                return Err(SubjectReframeError::InvalidContainmentConstraint {
                    frame: constraint.at,
                    axis,
                    minimum,
                    maximum,
                });
            }
        }
    }
    let mut normalized = Vec::with_capacity(samples.len());
    for sample in samples {
        let constraint = by_frame
            .remove(&sample.at)
            .ok_or(SubjectReframeError::MissingContainmentConstraint(sample.at))?;
        let min_x = constraint.min_x_basis_points.max(minimum_x);
        let max_x = constraint.max_x_basis_points.min(maximum_x);
        let min_y = constraint.min_y_basis_points.max(minimum_y);
        let max_y = constraint.max_y_basis_points.min(maximum_y);
        if min_x > max_x {
            return Err(SubjectReframeError::ContainmentOutsideFocusBounds {
                frame: sample.at,
                axis: "horizontal",
            });
        }
        if min_y > max_y {
            return Err(SubjectReframeError::ContainmentOutsideFocusBounds {
                frame: sample.at,
                axis: "vertical",
            });
        }
        normalized.push(ScaledFocusConstraint {
            min_x,
            max_x,
            min_y,
            max_y,
        });
    }
    if let Some((at, _)) = by_frame.into_iter().next() {
        return Err(SubjectReframeError::UnexpectedContainmentConstraint(at));
    }
    Ok(normalized)
}

#[allow(clippy::too_many_arguments)]
fn containment_aware_focus_values(
    observations: &[i64],
    containment: &[(i64, i64)],
    frames: &[TimeCode],
    axis: &'static str,
    minimum: i64,
    maximum: i64,
    dead_zone: i64,
    maximum_step: i64,
) -> Result<Vec<i64>, SubjectReframeError> {
    debug_assert_eq!(observations.len(), containment.len());
    debug_assert_eq!(observations.len(), frames.len());
    if observations.is_empty() {
        return Ok(Vec::new());
    }
    let desired = stabilized_focus_values(observations, minimum, maximum, dead_zone, maximum_step);
    let mut reachable: Vec<(i64, i64)> = Vec::with_capacity(containment.len());
    for (index, &(allowed_minimum, allowed_maximum)) in containment.iter().enumerate() {
        let (reachable_minimum, reachable_maximum) =
            if let Some((previous_minimum, previous_maximum)) = reachable.last() {
                (
                    previous_minimum.saturating_sub(maximum_step),
                    previous_maximum.saturating_add(maximum_step),
                )
            } else {
                (minimum, maximum)
            };
        let lower = allowed_minimum.max(reachable_minimum).max(minimum);
        let upper = allowed_maximum.min(reachable_maximum).min(maximum);
        if lower > upper {
            return Err(SubjectReframeError::ContainmentInfeasible {
                frame: frames[index],
                axis,
                allowed_minimum,
                allowed_maximum,
                reachable_minimum,
                reachable_maximum,
            });
        }
        reachable.push((lower, upper));
    }
    let mut values = vec![0; desired.len()];
    let last = values.len() - 1;
    values[last] = desired[last].clamp(reachable[last].0, reachable[last].1);
    for index in (0..last).rev() {
        let (reachable_minimum, reachable_maximum) = reachable[index];
        let lower = reachable_minimum.max(values[index + 1].saturating_sub(maximum_step));
        let upper = reachable_maximum.min(values[index + 1].saturating_add(maximum_step));
        debug_assert!(lower <= upper);
        values[index] = desired[index].clamp(lower, upper);
    }
    Ok(values)
}

fn stabilized_focus_values(
    observations: &[i64],
    minimum: i64,
    maximum: i64,
    dead_zone: i64,
    maximum_step: i64,
) -> Vec<i64> {
    let filtered = median_filtered_observations(observations);
    reactive_focus_values(&filtered, minimum, maximum, dead_zone, maximum_step)
}

fn median_filtered_observations(observations: &[i64]) -> Vec<i64> {
    let mut filtered = observations.to_vec();
    for index in 1..observations.len().saturating_sub(1) {
        let mut window = [
            observations[index - 1],
            observations[index],
            observations[index + 1],
        ];
        window.sort_unstable();
        filtered[index] = window[1];
    }
    if observations.len() >= 3 {
        let last = observations.len() - 1;
        let mut window = [
            observations[last - 2],
            observations[last - 1],
            observations[last],
        ];
        window.sort_unstable();
        filtered[last] = window[1];
    }
    filtered
}

fn reactive_focus_values(
    filtered: &[i64],
    minimum: i64,
    maximum: i64,
    dead_zone: i64,
    maximum_step: i64,
) -> Vec<i64> {
    let Some(first) = filtered.first().copied() else {
        return Vec::new();
    };
    let mut focus = first.clamp(minimum, maximum);
    filtered
        .iter()
        .copied()
        .map(|subject| {
            let subject = subject.clamp(minimum, maximum);
            let desired = if subject < focus.saturating_sub(dead_zone) {
                subject.saturating_add(dead_zone)
            } else if subject > focus.saturating_add(dead_zone) {
                subject.saturating_sub(dead_zone)
            } else {
                focus
            };
            focus = focus
                .saturating_add((desired - focus).clamp(-maximum_step, maximum_step))
                .clamp(minimum, maximum);
            focus
        })
        .collect()
}

fn normalized_name(value: &str) -> String {
    value.trim().to_lowercase()
}

fn add_offset(left: TimeCode, right: TimeCode) -> Result<TimeCode, SpeakerMulticamError> {
    left.checked_add(right)
        .ok_or(SpeakerMulticamError::TimeOverflow)
}

fn intersect(left: Range<TimeCode>, right: Range<TimeCode>) -> Option<Range<TimeCode>> {
    let start = left.start.max(right.start);
    let end = left.end.min(right.end);
    (end > start).then_some(start..end)
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf};

    use super::*;
    use crate::{
        Clip, Effect, MediaAsset, MediaCatalog, MediaKind, ParamValue, Rational, SyncGroup,
        SyncGroupMember, Track, apply_batch,
    };

    fn fps() -> Rational {
        Rational::new(30, 1).unwrap()
    }

    fn asset(id: u64, kind: MediaKind) -> MediaAsset {
        MediaAsset {
            id: AssetId(id),
            path: PathBuf::from(format!("angle-{id}.mp4")),
            name: format!("Angle {id}"),
            duration: TimeCode(300),
            fps: fps(),
            kind,
            resolution: Some((1_920, 1_080)),
        }
    }

    fn multicam_document() -> Document {
        Document {
            tracks: vec![Track {
                id: TrackId(7),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: Vec::new(),
            }],
            media_pool: vec![asset(1, MediaKind::AudioVideo), asset(2, MediaKind::Video)],
            catalog: MediaCatalog {
                sync_groups: vec![SyncGroup {
                    id: SyncGroupId(4),
                    name: "Ceremony".to_owned(),
                    members: vec![
                        SyncGroupMember {
                            asset: AssetId(1),
                            offset: TimeCode::ZERO,
                            angle_name: "Wide".to_owned(),
                        },
                        SyncGroupMember {
                            asset: AssetId(2),
                            offset: TimeCode(-3),
                            angle_name: "Close".to_owned(),
                        },
                    ],
                }],
                ..MediaCatalog::default()
            },
            fps: fps(),
            resolution: (1_920, 1_080),
            ..Document::default()
        }
    }

    fn transcript(words: Vec<(TimeCode, TimeCode, Option<&str>)>) -> AssetTranscript {
        AssetTranscript {
            asset: AssetId(1),
            content_sha256: "fixture".to_owned(),
            source_fps: fps(),
            words: words
                .into_iter()
                .map(
                    |(source_start, source_end, speaker)| crate::TranscriptWord {
                        text: "word".to_owned(),
                        source_start,
                        source_end,
                        speaker: speaker.map(str::to_owned),
                    },
                )
                .collect(),
        }
    }

    fn multicam_settings() -> SpeakerMulticamSettings {
        SpeakerMulticamSettings {
            sync_group: SyncGroupId(4),
            target_track: TrackId(7),
            group_start: TimeCode::ZERO,
            group_end: TimeCode(60),
            record_start: TimeCode(100),
            maximum_word_gap_frames: TimeCode(3),
            minimum_shot_frames: TimeCode(5),
            assignments: vec![
                SpeakerAngleAssignment {
                    speaker: "Alice".to_owned(),
                    angle_name: "Close".to_owned(),
                },
                SpeakerAngleAssignment {
                    speaker: "Bob".to_owned(),
                    angle_name: "Wide".to_owned(),
                },
            ],
        }
    }

    #[test]
    fn speaker_plan_maps_offsets_and_returns_applicable_latest_first_overwrites() {
        let document = multicam_document();
        let source = transcript(vec![
            (TimeCode(10), TimeCode(15), Some("Alice")),
            (TimeCode(16), TimeCode(20), Some("Alice")),
            (TimeCode(30), TimeCode(45), Some("Bob")),
        ]);

        let plan = plan_speaker_multicam(&document, &source, &multicam_settings()).unwrap();

        assert_eq!(plan.cuts.len(), 2);
        assert_eq!(plan.cuts[0].angle_name, "Close");
        assert_eq!(plan.cuts[0].source_in, TimeCode(3));
        assert_eq!(plan.cuts[0].timeline_start, TimeCode(100));
        assert_eq!(plan.cuts[0].timeline_end, TimeCode(125));
        assert_eq!(plan.cuts[1].angle_name, "Wide");
        assert_eq!(plan.cuts[1].timeline_start, TimeCode(125));
        assert_eq!(plan.cuts[1].timeline_end, TimeCode(160));
        assert!(matches!(
            &plan.operations[0],
            Operation::ThreePointEdit {
                asset: AssetId(1),
                timeline_in: Some(TimeCode(125)),
                ..
            }
        ));
        let mut applied = document;
        apply_batch(&mut applied, &plan.operations).unwrap();
        assert_eq!(applied.tracks[0].clips.len(), 2);
    }

    #[test]
    fn speaker_plan_suppresses_short_backchannels_before_resolving_overlap() {
        let document = multicam_document();
        let source = transcript(vec![
            (TimeCode(10), TimeCode(30), Some("Alice")),
            (TimeCode(20), TimeCode(22), Some("Bob")),
            (TimeCode(40), TimeCode(55), Some("Bob")),
        ]);
        let mut settings = multicam_settings();
        settings.minimum_shot_frames = TimeCode(5);

        let plan = plan_speaker_multicam(&document, &source, &settings).unwrap();

        assert_eq!(plan.suppressed_short_shots, 1);
        assert_eq!(plan.cuts.len(), 2);
        assert_eq!(plan.cuts[0].angle_name, "Close");
        assert_eq!(plan.cuts[0].group_start, TimeCode::ZERO);
        assert_eq!(plan.cuts[0].group_end, TimeCode(35));
        assert_eq!(plan.cuts[1].angle_name, "Wide");
        assert_eq!(plan.cuts[1].group_start, TimeCode(35));
        assert_eq!(plan.cuts[1].group_end, TimeCode(60));
    }

    #[test]
    fn speaker_plan_refuses_to_invent_missing_diarization() {
        let error = plan_speaker_multicam(
            &multicam_document(),
            &transcript(vec![(TimeCode(10), TimeCode(20), None)]),
            &multicam_settings(),
        )
        .unwrap_err();

        assert_eq!(
            error,
            SpeakerMulticamError::MissingSpeakerLabel { index: 0 }
        );
    }

    fn reframe_document() -> Document {
        let reframe = Effect {
            id: EffectId(9),
            name: "reframe".to_owned(),
            parameters: BTreeMap::from([
                (
                    "target_aspect_basis_points".to_owned(),
                    ParamValue::Integer(5_625),
                ),
                ("focus_x_percent".to_owned(), ParamValue::Integer(50)),
                ("focus_y_percent".to_owned(), ParamValue::Integer(50)),
            ]),
            keyframes: BTreeMap::new(),
        };
        Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(2),
                    asset: AssetId(1),
                    source_range: TimeCode::ZERO..TimeCode(100),
                    content: ClipContent::Media,
                    timeline_start: TimeCode::ZERO,
                    effects: vec![reframe],
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                }],
            }],
            media_pool: vec![asset(1, MediaKind::Video)],
            duration: TimeCode(100),
            fps: fps(),
            resolution: (1_080, 1_920),
            ..Document::default()
        }
    }

    #[test]
    fn subject_reframe_sorts_clamps_and_builds_editable_curves() {
        let document = reframe_document();
        let settings = SubjectReframeSettings {
            clip: ClipId(2),
            effect: EffectId(9),
            bounds: ReframeFocusBounds {
                min_x_percent: 10,
                max_x_percent: 90,
                min_y_percent: 20,
                max_y_percent: 80,
            },
            minimum_confidence_basis_points: 7_000,
            focus_dead_zone_percent: 0,
            maximum_focus_step_percent: 25,
        };
        let samples = [
            SubjectCenterSample {
                at: TimeCode(99),
                x_percent: 130,
                y_percent: 90,
                confidence_basis_points: 9_000,
            },
            SubjectCenterSample {
                at: TimeCode::ZERO,
                x_percent: -12,
                y_percent: 40,
                confidence_basis_points: 10_000,
            },
        ];

        let plan = plan_subject_reframe(&document, settings, &samples).unwrap();

        assert_eq!(plan.clamped_samples, 2);
        assert_eq!(plan.samples[0].at, TimeCode::ZERO);
        assert_eq!(plan.samples[1].x_percent, 90);
        assert_eq!(plan.samples[1].y_percent, 80);
        assert_eq!(plan.focus_x_curve.keyframes[0].value, 10);
        assert_eq!(plan.focus_x_curve.keyframes[1].value, 35);
        assert_eq!(plan.focus_y_curve.keyframes[1].value, 65);
        let mut applied = document;
        apply_batch(&mut applied, &plan.operations).unwrap();
        assert_eq!(
            applied.clip(ClipId(2)).unwrap().effects[0].keyframes.len(),
            2
        );
    }

    #[test]
    fn subject_reframe_rejects_low_confidence_observations() {
        let error = plan_subject_reframe(
            &reframe_document(),
            SubjectReframeSettings {
                clip: ClipId(2),
                effect: EffectId(9),
                bounds: ReframeFocusBounds::default(),
                minimum_confidence_basis_points: 8_000,
                focus_dead_zone_percent: 6,
                maximum_focus_step_percent: 2,
            },
            &[SubjectCenterSample {
                at: TimeCode(10),
                x_percent: 50,
                y_percent: 50,
                confidence_basis_points: 7_999,
            }],
        )
        .unwrap_err();

        assert!(matches!(error, SubjectReframeError::LowConfidence { .. }));
    }

    #[test]
    fn subject_reframe_dead_zone_filters_jitter_and_rate_limits_camera_motion() {
        let document = reframe_document();
        let samples = [50, 52, 48, 53, 47, 37, 35, 34, 34, 34]
            .into_iter()
            .enumerate()
            .map(|(index, x_percent)| SubjectCenterSample {
                at: TimeCode(i64::try_from(index).unwrap() * 10),
                x_percent,
                y_percent: 50,
                confidence_basis_points: 10_000,
            })
            .collect::<Vec<_>>();

        let plan = plan_subject_reframe(
            &document,
            SubjectReframeSettings {
                clip: ClipId(2),
                effect: EffectId(9),
                bounds: ReframeFocusBounds {
                    min_x_percent: 25,
                    max_x_percent: 75,
                    min_y_percent: 20,
                    max_y_percent: 80,
                },
                minimum_confidence_basis_points: 0,
                focus_dead_zone_percent: 6,
                maximum_focus_step_percent: 2,
            },
            &samples,
        )
        .unwrap();
        let horizontal = &plan.focus_x_curve.keyframes;

        assert_eq!(
            horizontal
                .iter()
                .map(|keyframe| keyframe.value)
                .collect::<Vec<_>>(),
            vec![50, 50, 50, 50, 50, 48, 46, 44, 42, 40]
        );
        assert!(
            horizontal
                .iter()
                .all(|keyframe| keyframe.interpolation == KeyframeInterpolation::Linear)
        );
        assert!(
            horizontal
                .windows(2)
                .all(|pair| pair[0].value.abs_diff(pair[1].value) <= 2)
        );
    }

    #[test]
    fn subject_reframe_rejects_a_last_frame_tracking_outlier() {
        let values = stabilized_focus_values(&[50, 50, 34, 35, 80], 25, 75, 6, 25);

        assert_eq!(values, vec![50, 50, 41, 41, 41]);
    }

    #[test]
    fn basis_point_subject_reframe_preserves_subpercent_tracker_precision() {
        let document = reframe_document();
        let plan = plan_subject_reframe_basis_points(
            &document,
            SubjectReframeSettings {
                clip: ClipId(2),
                effect: EffectId(9),
                bounds: ReframeFocusBounds::default(),
                minimum_confidence_basis_points: 0,
                focus_dead_zone_percent: 0,
                maximum_focus_step_percent: 25,
            },
            &[
                SubjectCenterBasisPointSample {
                    at: TimeCode::ZERO,
                    x_basis_points: 5_001,
                    y_basis_points: 4_999,
                    confidence_basis_points: 10_000,
                },
                SubjectCenterBasisPointSample {
                    at: TimeCode(10),
                    x_basis_points: 5_002,
                    y_basis_points: 4_998,
                    confidence_basis_points: 10_000,
                },
            ],
        )
        .unwrap();

        assert_eq!(
            plan.focus_x_curve
                .keyframes
                .iter()
                .map(|keyframe| keyframe.value)
                .collect::<Vec<_>>(),
            vec![5_001, 5_002]
        );
        assert!(matches!(
            &plan.operations[0],
            Operation::SetEffectKeyframes { name, .. } if name == "focus_x_basis_points"
        ));
        let mut applied = document;
        apply_batch(&mut applied, &plan.operations).unwrap();
        let effect = &applied.clip(ClipId(2)).unwrap().effects[0];
        assert_eq!(
            effect.keyframes["focus_x_basis_points"].keyframes[0].value,
            5_001
        );
        assert_eq!(
            effect.keyframes["focus_y_basis_points"].keyframes[1].value,
            4_998
        );
    }

    #[test]
    fn containment_controller_moves_before_a_future_subject_edge_reaches_the_crop() {
        let document = reframe_document();
        let settings = SubjectReframeSettings {
            clip: ClipId(2),
            effect: EffectId(9),
            bounds: ReframeFocusBounds {
                min_x_percent: 25,
                max_x_percent: 75,
                min_y_percent: 20,
                max_y_percent: 80,
            },
            minimum_confidence_basis_points: 0,
            focus_dead_zone_percent: 6,
            maximum_focus_step_percent: 2,
        };
        let samples = [TimeCode::ZERO, TimeCode(10), TimeCode(20)]
            .into_iter()
            .map(|at| SubjectCenterBasisPointSample {
                at,
                x_basis_points: 5_020,
                y_basis_points: 5_000,
                confidence_basis_points: 10_000,
            })
            .collect::<Vec<_>>();
        let containment = [
            SubjectFocusBasisPointConstraint {
                at: TimeCode::ZERO,
                min_x_basis_points: 2_500,
                max_x_basis_points: 7_500,
                min_y_basis_points: 2_000,
                max_y_basis_points: 8_000,
            },
            SubjectFocusBasisPointConstraint {
                at: TimeCode(10),
                min_x_basis_points: 2_500,
                max_x_basis_points: 7_500,
                min_y_basis_points: 2_000,
                max_y_basis_points: 8_000,
            },
            SubjectFocusBasisPointConstraint {
                at: TimeCode(20),
                min_x_basis_points: 2_500,
                max_x_basis_points: 4_693,
                min_y_basis_points: 2_000,
                max_y_basis_points: 8_000,
            },
        ];

        let plan = plan_subject_reframe_basis_points_with_containment(
            &document,
            settings,
            &samples,
            &containment,
        )
        .unwrap();
        let horizontal = plan
            .focus_x_curve
            .keyframes
            .iter()
            .map(|keyframe| keyframe.value)
            .collect::<Vec<_>>();

        assert_eq!(horizontal, vec![5_020, 4_893, 4_693]);
        assert!(
            horizontal
                .windows(2)
                .all(|pair| { pair[0].abs_diff(pair[1]) <= 200 })
        );
        assert!(
            plan.focus_x_curve
                .keyframes
                .iter()
                .all(|keyframe| keyframe.interpolation == KeyframeInterpolation::Linear)
        );
    }

    #[test]
    fn containment_controller_returns_an_error_when_the_speed_bound_cannot_reach_the_subject() {
        let document = reframe_document();
        let settings = SubjectReframeSettings {
            clip: ClipId(2),
            effect: EffectId(9),
            bounds: ReframeFocusBounds {
                min_x_percent: 25,
                max_x_percent: 75,
                min_y_percent: 20,
                max_y_percent: 80,
            },
            minimum_confidence_basis_points: 0,
            focus_dead_zone_percent: 6,
            maximum_focus_step_percent: 2,
        };
        let samples = [TimeCode::ZERO, TimeCode(10)]
            .into_iter()
            .map(|at| SubjectCenterBasisPointSample {
                at,
                x_basis_points: 5_000,
                y_basis_points: 5_000,
                confidence_basis_points: 10_000,
            })
            .collect::<Vec<_>>();
        let containment = [
            SubjectFocusBasisPointConstraint {
                at: TimeCode::ZERO,
                min_x_basis_points: 5_000,
                max_x_basis_points: 5_000,
                min_y_basis_points: 5_000,
                max_y_basis_points: 5_000,
            },
            SubjectFocusBasisPointConstraint {
                at: TimeCode(10),
                min_x_basis_points: 7_000,
                max_x_basis_points: 7_000,
                min_y_basis_points: 5_000,
                max_y_basis_points: 5_000,
            },
        ];

        let error = plan_subject_reframe_basis_points_with_containment(
            &document,
            settings,
            &samples,
            &containment,
        )
        .unwrap_err();

        assert_eq!(
            error,
            SubjectReframeError::ContainmentInfeasible {
                frame: TimeCode(10),
                axis: "horizontal",
                allowed_minimum: 7_000,
                allowed_maximum: 7_000,
                reachable_minimum: 4_800,
                reachable_maximum: 5_200,
            }
        );
    }
}
