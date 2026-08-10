use std::collections::HashSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    AssetId, Clip, ClipId, Document, Effect, EffectId, MediaAsset, ParamValue, TimeCode,
    TimeMappingError, Track, TrackId, Transition, map_source_range_to_project,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Operation {
    AddAsset {
        asset: MediaAsset,
    },
    AddTrack {
        track: Track,
    },
    RemoveTrack {
        track: TrackId,
    },
    AddClip {
        track: TrackId,
        asset: AssetId,
        at: TimeCode,
        source: std::ops::Range<TimeCode>,
    },
    SplitClip {
        clip: ClipId,
        /// Split position in project frames.
        at: TimeCode,
    },
    TrimClip {
        clip: ClipId,
        new_source: std::ops::Range<TimeCode>,
    },
    MoveClip {
        clip: ClipId,
        to_track: TrackId,
        to: TimeCode,
    },
    DeleteClip {
        clip: ClipId,
    },
    AddEffect {
        clip: ClipId,
        effect: Effect,
    },
    RemoveEffect {
        clip: ClipId,
        effect: EffectId,
    },
    SetEffectParam {
        clip: ClipId,
        effect: EffectId,
        name: String,
        value: ParamValue,
    },
    AddTransition {
        clip: ClipId,
        transition: Transition,
    },
    RemoveTransition {
        clip: ClipId,
    },
}

pub trait ApplyOp {
    /// Validate and apply to `doc`. This method performs no I/O.
    fn apply(&self, doc: &mut Document) -> Result<(), OpError>;
}

impl Operation {
    pub fn apply(&self, doc: &mut Document) -> Result<(), OpError> {
        <Self as ApplyOp>::apply(self, doc)
    }
}

/// Validate and apply an ordered edit plan without exposing partial state.
///
/// The caller's document is replaced only after every operation succeeds. Each
/// operation is applied to the result of the previous one, so generated ids and
/// later validations have the same semantics as sequential `Operation::apply`
/// calls.
pub fn apply_batch(doc: &mut Document, operations: &[Operation]) -> Result<(), BatchError> {
    if operations.is_empty() {
        return Err(BatchError::Empty);
    }
    let mut candidate = doc.clone();
    for (index, operation) in operations.iter().enumerate() {
        operation
            .apply(&mut candidate)
            .map_err(|error| BatchError::OperationFailed {
                op_number: index + 1,
                error,
            })?;
    }
    *doc = candidate;
    Ok(())
}

impl ApplyOp for Operation {
    fn apply(&self, doc: &mut Document) -> Result<(), OpError> {
        // Applying to a clone makes rejection atomic: the caller's document is
        // unchanged if either the operation or final invariant check fails.
        validate_document(doc)?;
        let mut candidate = doc.clone();
        apply_unchecked(self, &mut candidate)?;
        candidate.recompute_duration()?;
        validate_document(&candidate)?;
        *doc = candidate;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OpError {
    #[error("asset {0} already exists")]
    DuplicateAsset(AssetId),
    #[error("track {0} occurs more than once")]
    DuplicateTrack(TrackId),
    #[error("new track {0} must be empty")]
    NewTrackNotEmpty(TrackId),
    #[error("clip {0} occurs more than once")]
    DuplicateClip(ClipId),
    #[error("asset {0} does not exist")]
    MissingAsset(AssetId),
    #[error("track {0} does not exist")]
    MissingTrack(TrackId),
    #[error("clip {0} does not exist")]
    MissingClip(ClipId),
    #[error("asset {asset} cannot be placed on {track} track {track_id}")]
    IncompatibleTrack {
        asset: AssetId,
        track: &'static str,
        track_id: TrackId,
    },
    #[error("source range must be non-empty and non-negative: {start}..{end}")]
    InvalidSourceRange { start: i64, end: i64 },
    #[error("source range ends at {end}, beyond asset {asset}'s duration {duration}")]
    SourceOutOfBounds {
        asset: AssetId,
        end: TimeCode,
        duration: TimeCode,
    },
    #[error("timeline positions must be non-negative: {0}")]
    NegativeTimelinePosition(TimeCode),
    #[error("clip {0} maps to zero project frames")]
    ZeroProjectDuration(ClipId),
    #[error("clip {clip} overlaps clip {with} on track {track}")]
    ClipOverlap {
        track: TrackId,
        clip: ClipId,
        with: ClipId,
    },
    #[error("clips on track {track} are not sorted: {previous} appears before {next}")]
    ClipsUnsorted {
        track: TrackId,
        previous: ClipId,
        next: ClipId,
    },
    #[error("split at project frame {at} is outside clip {clip}")]
    SplitOutsideClip { clip: ClipId, at: TimeCode },
    #[error("project frame {at} inside clip {clip} is not an integer source-frame boundary")]
    UnrepresentableSplit { clip: ClipId, at: TimeCode },
    #[error("document frame rate is invalid")]
    InvalidProjectRate,
    #[error("asset {0} has an invalid frame rate")]
    InvalidAssetRate(AssetId),
    #[error("asset {0} has a non-positive duration")]
    InvalidAssetDuration(AssetId),
    #[error("document resolution must be non-zero")]
    InvalidResolution,
    #[error("document duration {actual:?} does not match calculated duration {expected:?}")]
    IncorrectDocumentDuration {
        expected: TimeCode,
        actual: TimeCode,
    },
    #[error("clip id space is exhausted")]
    ClipIdExhausted,
    #[error("effect {effect} already exists on clip {clip}")]
    DuplicateEffect { clip: ClipId, effect: EffectId },
    #[error("effect {effect} does not exist on clip {clip}")]
    MissingEffect { clip: ClipId, effect: EffectId },
    #[error("unknown effect name {0:?}")]
    UnknownEffect(String),
    #[error("effect {effect:?} has no parameter {name:?}")]
    UnknownEffectParam { effect: String, name: String },
    #[error("effect {effect:?} parameter {name:?} requires an integer")]
    InvalidEffectParamType { effect: String, name: String },
    #[error(
        "effect {effect:?} parameter {name:?} is {actual}, outside the inclusive range {min}..={max}"
    )]
    EffectParamOutOfRange {
        effect: String,
        name: String,
        min: i64,
        max: i64,
        actual: i64,
    },
    #[error("clip {0} already has a transition_in")]
    DuplicateTransition(ClipId),
    #[error("clip {0} has no transition_in")]
    MissingTransition(ClipId),
    #[error("unknown transition name {0:?}")]
    UnknownTransition(String),
    #[error("transition duration on clip {clip} must be positive, got {duration}")]
    InvalidTransitionDuration { clip: ClipId, duration: TimeCode },
    #[error("transition duration {duration} exceeds clip {clip} duration {clip_duration}")]
    TransitionTooLong {
        clip: ClipId,
        duration: TimeCode,
        clip_duration: TimeCode,
    },
    #[error("time calculation overflowed")]
    TimeOverflow,
    #[error(transparent)]
    TimeMapping(#[from] TimeMappingError),
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BatchError {
    #[error("edit plan must contain at least one operation")]
    Empty,
    #[error("edit plan operation {op_number} failed: {error}")]
    OperationFailed {
        /// One-based operation number as presented to users and agents.
        op_number: usize,
        #[source]
        error: OpError,
    },
}

fn apply_unchecked(operation: &Operation, doc: &mut Document) -> Result<(), OpError> {
    match operation {
        Operation::AddAsset { asset } => add_asset(doc, asset.clone()),
        Operation::AddTrack { track } => add_track(doc, track.clone()),
        Operation::RemoveTrack { track } => remove_track(doc, *track),
        Operation::AddClip {
            track,
            asset,
            at,
            source,
        } => add_clip(doc, *track, *asset, *at, source.clone()),
        Operation::SplitClip { clip, at } => split_clip(doc, *clip, *at),
        Operation::TrimClip { clip, new_source } => trim_clip(doc, *clip, new_source.clone()),
        Operation::MoveClip { clip, to_track, to } => move_clip(doc, *clip, *to_track, *to),
        Operation::DeleteClip { clip } => delete_clip(doc, *clip),
        Operation::AddEffect { clip, effect } => add_effect(doc, *clip, effect.clone()),
        Operation::RemoveEffect { clip, effect } => remove_effect(doc, *clip, *effect),
        Operation::SetEffectParam {
            clip,
            effect,
            name,
            value,
        } => set_effect_param(doc, *clip, *effect, name, value.clone()),
        Operation::AddTransition { clip, transition } => {
            add_transition(doc, *clip, transition.clone())
        }
        Operation::RemoveTransition { clip } => remove_transition(doc, *clip),
    }
}

fn add_track(doc: &mut Document, track: Track) -> Result<(), OpError> {
    if doc.tracks.iter().any(|existing| existing.id == track.id) {
        return Err(OpError::DuplicateTrack(track.id));
    }
    if !track.clips.is_empty() {
        return Err(OpError::NewTrackNotEmpty(track.id));
    }
    doc.tracks.push(track);
    Ok(())
}

fn remove_track(doc: &mut Document, track_id: TrackId) -> Result<(), OpError> {
    let index = doc
        .tracks
        .iter()
        .position(|track| track.id == track_id)
        .ok_or(OpError::MissingTrack(track_id))?;
    doc.tracks.remove(index);
    Ok(())
}

fn add_asset(doc: &mut Document, asset: MediaAsset) -> Result<(), OpError> {
    if doc.asset(asset.id).is_some() {
        return Err(OpError::DuplicateAsset(asset.id));
    }
    validate_asset(&asset)?;
    doc.media_pool.push(asset);
    Ok(())
}

fn add_clip(
    doc: &mut Document,
    track_id: TrackId,
    asset_id: AssetId,
    at: TimeCode,
    source: std::ops::Range<TimeCode>,
) -> Result<(), OpError> {
    if at < TimeCode::ZERO {
        return Err(OpError::NegativeTimelinePosition(at));
    }
    let asset = doc.asset(asset_id).ok_or(OpError::MissingAsset(asset_id))?;
    validate_source_range(asset, &source)?;
    let track_index = doc
        .tracks
        .iter()
        .position(|track| track.id == track_id)
        .ok_or(OpError::MissingTrack(track_id))?;
    validate_track_compatibility(asset, &doc.tracks[track_index])?;

    let clip = Clip {
        id: next_clip_id(doc)?,
        asset: asset_id,
        source_range: source,
        timeline_start: at,
        effects: Vec::new(),
        transition_in: None,
    };
    doc.tracks[track_index].clips.push(clip);
    doc.tracks[track_index]
        .clips
        .sort_by_key(|clip| (clip.timeline_start, clip.id));
    Ok(())
}

fn split_clip(doc: &mut Document, clip_id: ClipId, at: TimeCode) -> Result<(), OpError> {
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    let original = doc.tracks[track_index].clips[clip_index].clone();
    let end = doc.clip_end(&original)?;
    if at <= original.timeline_start || at >= end {
        return Err(OpError::SplitOutsideClip { clip: clip_id, at });
    }

    let asset = doc
        .asset(original.asset)
        .ok_or(OpError::MissingAsset(original.asset))?;
    let offset = at
        .checked_sub(original.timeline_start)
        .ok_or(OpError::TimeOverflow)?;
    let source_split =
        find_source_boundary(original.source_range.clone(), offset, asset.fps, doc.fps)
            .ok_or(OpError::UnrepresentableSplit { clip: clip_id, at })?;
    let new_id = next_clip_id(doc)?;

    doc.tracks[track_index].clips[clip_index].source_range.end = source_split;
    let mut right = original;
    right.id = new_id;
    right.source_range.start = source_split;
    right.timeline_start = at;
    right.transition_in = None;
    doc.tracks[track_index].clips.push(right);
    doc.tracks[track_index]
        .clips
        .sort_by_key(|clip| (clip.timeline_start, clip.id));
    Ok(())
}

fn trim_clip(
    doc: &mut Document,
    clip_id: ClipId,
    new_source: std::ops::Range<TimeCode>,
) -> Result<(), OpError> {
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    let original = doc.tracks[track_index].clips[clip_index].clone();
    let asset_id = original.asset;
    let asset = doc.asset(asset_id).ok_or(OpError::MissingAsset(asset_id))?;
    validate_source_range(asset, &new_source)?;
    let shifted_start = match new_source.start.cmp(&original.source_range.start) {
        std::cmp::Ordering::Greater => {
            let offset = map_source_range_to_project(
                original.source_range.start..new_source.start,
                asset.fps,
                doc.fps,
            )?;
            original
                .timeline_start
                .checked_add(offset)
                .ok_or(OpError::TimeOverflow)?
        }
        std::cmp::Ordering::Less => {
            let offset = map_source_range_to_project(
                new_source.start..original.source_range.start,
                asset.fps,
                doc.fps,
            )?;
            original
                .timeline_start
                .checked_sub(offset)
                .ok_or(OpError::TimeOverflow)?
        }
        std::cmp::Ordering::Equal => original.timeline_start,
    };
    if shifted_start < TimeCode::ZERO {
        return Err(OpError::NegativeTimelinePosition(shifted_start));
    }
    doc.tracks[track_index].clips[clip_index].timeline_start = shifted_start;
    doc.tracks[track_index].clips[clip_index].source_range = new_source;
    Ok(())
}

fn move_clip(
    doc: &mut Document,
    clip_id: ClipId,
    target_track_id: TrackId,
    to: TimeCode,
) -> Result<(), OpError> {
    if to < TimeCode::ZERO {
        return Err(OpError::NegativeTimelinePosition(to));
    }
    let (source_track_index, clip_index) = find_clip(doc, clip_id)?;
    let target_track_index = doc
        .tracks
        .iter()
        .position(|track| track.id == target_track_id)
        .ok_or(OpError::MissingTrack(target_track_id))?;
    let asset_id = doc.tracks[source_track_index].clips[clip_index].asset;
    let asset = doc.asset(asset_id).ok_or(OpError::MissingAsset(asset_id))?;
    validate_track_compatibility(asset, &doc.tracks[target_track_index])?;

    let mut clip = doc.tracks[source_track_index].clips.remove(clip_index);
    clip.timeline_start = to;
    doc.tracks[target_track_index].clips.push(clip);
    doc.tracks[target_track_index]
        .clips
        .sort_by_key(|clip| (clip.timeline_start, clip.id));
    Ok(())
}

fn delete_clip(doc: &mut Document, clip_id: ClipId) -> Result<(), OpError> {
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    doc.tracks[track_index].clips.remove(clip_index);
    Ok(())
}

fn add_effect(doc: &mut Document, clip_id: ClipId, effect: Effect) -> Result<(), OpError> {
    validate_effect(&effect)?;
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    let clip = &mut doc.tracks[track_index].clips[clip_index];
    if clip.effects.iter().any(|existing| existing.id == effect.id) {
        return Err(OpError::DuplicateEffect {
            clip: clip_id,
            effect: effect.id,
        });
    }
    clip.effects.push(effect);
    Ok(())
}

fn remove_effect(doc: &mut Document, clip_id: ClipId, effect_id: EffectId) -> Result<(), OpError> {
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    let clip = &mut doc.tracks[track_index].clips[clip_index];
    let effect_index = clip
        .effects
        .iter()
        .position(|effect| effect.id == effect_id)
        .ok_or(OpError::MissingEffect {
            clip: clip_id,
            effect: effect_id,
        })?;
    clip.effects.remove(effect_index);
    Ok(())
}

fn set_effect_param(
    doc: &mut Document,
    clip_id: ClipId,
    effect_id: EffectId,
    name: &str,
    value: ParamValue,
) -> Result<(), OpError> {
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    let effect = doc.tracks[track_index].clips[clip_index]
        .effects
        .iter_mut()
        .find(|effect| effect.id == effect_id)
        .ok_or(OpError::MissingEffect {
            clip: clip_id,
            effect: effect_id,
        })?;
    validate_effect_parameter(&effect.name, name, &value)?;
    effect.parameters.insert(name.to_owned(), value);
    Ok(())
}

fn add_transition(
    doc: &mut Document,
    clip_id: ClipId,
    transition: Transition,
) -> Result<(), OpError> {
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    let clip = &doc.tracks[track_index].clips[clip_index];
    if clip.transition_in.is_some() {
        return Err(OpError::DuplicateTransition(clip_id));
    }
    validate_transition(doc, clip, &transition)?;
    doc.tracks[track_index].clips[clip_index].transition_in = Some(transition);
    Ok(())
}

fn remove_transition(doc: &mut Document, clip_id: ClipId) -> Result<(), OpError> {
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    let transition = doc.tracks[track_index].clips[clip_index]
        .transition_in
        .take();
    if transition.is_none() {
        return Err(OpError::MissingTransition(clip_id));
    }
    Ok(())
}

fn find_clip(doc: &Document, id: ClipId) -> Result<(usize, usize), OpError> {
    doc.tracks
        .iter()
        .enumerate()
        .find_map(|(track_index, track)| {
            track
                .clips
                .iter()
                .position(|clip| clip.id == id)
                .map(|clip_index| (track_index, clip_index))
        })
        .ok_or(OpError::MissingClip(id))
}

fn next_clip_id(doc: &Document) -> Result<ClipId, OpError> {
    doc.tracks
        .iter()
        .flat_map(|track| &track.clips)
        .map(|clip| clip.id.0)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .map(ClipId)
        .ok_or(OpError::ClipIdExhausted)
}

fn find_source_boundary(
    source: std::ops::Range<TimeCode>,
    project_offset: TimeCode,
    source_fps: crate::Rational,
    project_fps: crate::Rational,
) -> Option<TimeCode> {
    let mut low = source.start.0.checked_add(1)?;
    let mut high = source.end.0.checked_sub(1)?;
    while low <= high {
        let middle = low + (high - low) / 2;
        let candidate = TimeCode(middle);
        let mapped =
            map_source_range_to_project(source.start..candidate, source_fps, project_fps).ok()?;
        match mapped.cmp(&project_offset) {
            std::cmp::Ordering::Less => low = middle.checked_add(1)?,
            std::cmp::Ordering::Greater => high = middle.checked_sub(1)?,
            std::cmp::Ordering::Equal => return Some(candidate),
        }
    }
    None
}

fn validate_asset(asset: &MediaAsset) -> Result<(), OpError> {
    if !asset.fps.is_valid() {
        return Err(OpError::InvalidAssetRate(asset.id));
    }
    if asset.duration <= TimeCode::ZERO {
        return Err(OpError::InvalidAssetDuration(asset.id));
    }
    if asset
        .resolution
        .is_some_and(|(width, height)| width == 0 || height == 0)
    {
        return Err(OpError::InvalidResolution);
    }
    Ok(())
}

fn validate_effect(effect: &Effect) -> Result<(), OpError> {
    match effect.name.as_str() {
        "brightness" | "contrast" | "saturation" | "opacity" | "transform" => {}
        _ => return Err(OpError::UnknownEffect(effect.name.clone())),
    }
    for (name, value) in &effect.parameters {
        validate_effect_parameter(&effect.name, name, value)?;
    }
    Ok(())
}

fn validate_effect_parameter(effect: &str, name: &str, value: &ParamValue) -> Result<(), OpError> {
    let (min, max) = match (effect, name) {
        ("brightness" | "contrast" | "saturation", "percent") => (-100, 100),
        ("opacity", "percent") => (0, 100),
        ("transform", "scale_percent") => (1, 400),
        ("transform", "x_percent" | "y_percent") => (-100, 100),
        ("brightness" | "contrast" | "saturation" | "opacity" | "transform", _) => {
            return Err(OpError::UnknownEffectParam {
                effect: effect.to_owned(),
                name: name.to_owned(),
            });
        }
        _ => return Err(OpError::UnknownEffect(effect.to_owned())),
    };
    let ParamValue::Integer(actual) = value else {
        return Err(OpError::InvalidEffectParamType {
            effect: effect.to_owned(),
            name: name.to_owned(),
        });
    };
    if !(min..=max).contains(actual) {
        return Err(OpError::EffectParamOutOfRange {
            effect: effect.to_owned(),
            name: name.to_owned(),
            min,
            max,
            actual: *actual,
        });
    }
    Ok(())
}

fn validate_transition(
    doc: &Document,
    clip: &Clip,
    transition: &Transition,
) -> Result<(), OpError> {
    if transition.name != "crossfade" {
        return Err(OpError::UnknownTransition(transition.name.clone()));
    }
    if transition.duration <= TimeCode::ZERO {
        return Err(OpError::InvalidTransitionDuration {
            clip: clip.id,
            duration: transition.duration,
        });
    }
    let clip_duration = doc.clip_duration(clip)?;
    if transition.duration > clip_duration {
        return Err(OpError::TransitionTooLong {
            clip: clip.id,
            duration: transition.duration,
            clip_duration,
        });
    }
    Ok(())
}

fn validate_source_range(
    asset: &MediaAsset,
    source: &std::ops::Range<TimeCode>,
) -> Result<(), OpError> {
    if source.start < TimeCode::ZERO || source.end <= source.start {
        return Err(OpError::InvalidSourceRange {
            start: source.start.0,
            end: source.end.0,
        });
    }
    if source.end > asset.duration {
        return Err(OpError::SourceOutOfBounds {
            asset: asset.id,
            end: source.end,
            duration: asset.duration,
        });
    }
    Ok(())
}

fn validate_track_compatibility(asset: &MediaAsset, track: &crate::Track) -> Result<(), OpError> {
    if asset.kind.supports(track.kind) {
        Ok(())
    } else {
        Err(OpError::IncompatibleTrack {
            asset: asset.id,
            track: match track.kind {
                crate::TrackKind::Video => "video",
                crate::TrackKind::Audio => "audio",
            },
            track_id: track.id,
        })
    }
}

pub(crate) fn validate_document(doc: &Document) -> Result<(), OpError> {
    if !doc.fps.is_valid() {
        return Err(OpError::InvalidProjectRate);
    }
    if doc.resolution.0 == 0 || doc.resolution.1 == 0 {
        return Err(OpError::InvalidResolution);
    }

    let mut asset_ids = HashSet::new();
    for asset in &doc.media_pool {
        if !asset_ids.insert(asset.id) {
            return Err(OpError::DuplicateAsset(asset.id));
        }
        validate_asset(asset)?;
    }

    let mut track_ids = HashSet::new();
    let mut clip_ids = HashSet::new();
    let mut expected_duration = TimeCode::ZERO;
    for track in &doc.tracks {
        if !track_ids.insert(track.id) {
            return Err(OpError::DuplicateTrack(track.id));
        }
        let mut previous: Option<(&Clip, TimeCode)> = None;
        for clip in &track.clips {
            if !clip_ids.insert(clip.id) {
                return Err(OpError::DuplicateClip(clip.id));
            }
            if clip.timeline_start < TimeCode::ZERO {
                return Err(OpError::NegativeTimelinePosition(clip.timeline_start));
            }
            let asset = doc
                .asset(clip.asset)
                .ok_or(OpError::MissingAsset(clip.asset))?;
            validate_track_compatibility(asset, track)?;
            validate_source_range(asset, &clip.source_range)?;
            let clip_duration = doc.clip_duration(clip)?;
            if clip_duration <= TimeCode::ZERO {
                return Err(OpError::ZeroProjectDuration(clip.id));
            }
            let clip_end = clip
                .timeline_start
                .checked_add(clip_duration)
                .ok_or(OpError::TimeOverflow)?;
            let mut effect_ids = HashSet::new();
            for effect in &clip.effects {
                if !effect_ids.insert(effect.id) {
                    return Err(OpError::DuplicateEffect {
                        clip: clip.id,
                        effect: effect.id,
                    });
                }
                validate_effect(effect)?;
            }
            if let Some(transition) = &clip.transition_in {
                validate_transition(doc, clip, transition)?;
            }
            if let Some((previous_clip, previous_end)) = previous {
                if clip.timeline_start < previous_clip.timeline_start {
                    return Err(OpError::ClipsUnsorted {
                        track: track.id,
                        previous: previous_clip.id,
                        next: clip.id,
                    });
                }
                if clip.timeline_start < previous_end {
                    return Err(OpError::ClipOverlap {
                        track: track.id,
                        clip: previous_clip.id,
                        with: clip.id,
                    });
                }
            }
            previous = Some((clip, clip_end));
            expected_duration = expected_duration.max(clip_end);
        }
    }

    if doc.duration != expected_duration {
        return Err(OpError::IncorrectDocumentDuration {
            expected: expected_duration,
            actual: doc.duration,
        });
    }
    Ok(())
}
