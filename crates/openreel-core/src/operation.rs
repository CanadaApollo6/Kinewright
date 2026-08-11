use std::collections::HashSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    AssetId, Clip, ClipContent, ClipId, Document, Effect, EffectId, LinkId,
    MARKER_COLOR_TOKEN_COUNT, Marker, MarkerId, MediaAsset, ParamValue, TimeCode, TimeMappingError,
    Title, TitleParameterKind, TitlePosition, Track, TrackId, TrackKind, Transition,
    map_source_range_to_project, title_parameter_descriptor,
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
    /// Enable or disable cross-track ripple participation for one track.
    SetTrackSyncLock {
        track: TrackId,
        locked: bool,
    },
    AddClip {
        track: TrackId,
        asset: AssetId,
        at: TimeCode,
        source: std::ops::Range<TimeCode>,
    },
    AddTitle {
        track: TrackId,
        at: TimeCode,
        duration: TimeCode,
        title: Title,
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
    /// Delete one clip and close its duration on that clip's track and every
    /// other sync-locked track.
    ///
    /// The ripple point is the deleted clip's pre-edit end. On participating
    /// tracks, only clips whose start is at or after that point shift left.
    /// A clip that starts before the point is not shifted or trimmed, even
    /// when it straddles the point. Normal validation atomically rejects any
    /// overlap caused by shifting a later clip beside that unchanged clip.
    /// Project markers at or after the ripple point shift left regardless of
    /// track sync locks. A shifted marker is clamped to frame zero.
    RippleDeleteClip {
        clip: ClipId,
    },
    /// Insert empty time by shifting clips that start at or after `at` on the
    /// target track and every other sync-locked track. Clips that start before
    /// `at` remain unchanged. Project markers at or after `at` shift right
    /// regardless of track sync locks.
    RippleInsertGap {
        track: TrackId,
        at: TimeCode,
        duration: TimeCode,
    },
    /// Assign one fresh link id to the listed clips.
    ///
    /// Link-follow enforcement intentionally does not live in core apply:
    /// core operations stay independently pure, while the UI and agent build
    /// one `DoBatch` containing the corresponding edits for every member.
    LinkClips {
        clips: Vec<ClipId>,
    },
    /// Clear the link id on each listed clip. This is per-clip data mutation;
    /// remaining members of a group are not rewritten implicitly.
    UnlinkClips {
        clips: Vec<ClipId>,
    },
    AddMarker {
        marker: Marker,
    },
    RemoveMarker {
        marker: MarkerId,
    },
    MoveMarker {
        marker: MarkerId,
        to: TimeCode,
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
    SetTitleParam {
        clip: ClipId,
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
    SetMarkerParam {
        marker: MarkerId,
        name: String,
        value: ParamValue,
    },
}

pub trait ApplyOp {
    /// Validate and apply to `doc`. This method performs no I/O.
    ///
    /// # Errors
    ///
    /// Returns an operation error without mutating `doc` when validation fails.
    fn apply(&self, doc: &mut Document) -> Result<(), OpError>;
}

impl Operation {
    /// Validate and atomically apply this operation to a document.
    ///
    /// # Errors
    ///
    /// Returns an operation error without mutating `doc` when validation fails.
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
///
/// # Errors
///
/// Returns the failing operation index and error, leaving `doc` unchanged.
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
    #[error("title clips can only be placed on video track {0}")]
    TitleOnAudioTrack(TrackId),
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
    #[error("link id space is exhausted")]
    LinkIdExhausted,
    #[error("link operation requires at least {minimum} clip(s)")]
    TooFewLinkedClips { minimum: usize },
    #[error("clip {0} occurs more than once in the link operation")]
    DuplicateClipSelection(ClipId),
    #[error("marker {0} already exists")]
    DuplicateMarker(MarkerId),
    #[error("marker {0} does not exist")]
    MissingMarker(MarkerId),
    #[error("markers are not sorted: {previous} appears before {next}")]
    MarkersUnsorted { previous: MarkerId, next: MarkerId },
    #[error("marker positions must be non-negative: {0}")]
    NegativeMarkerPosition(TimeCode),
    #[error("marker color token {actual} is outside the supported range 0..{maximum_exclusive}")]
    InvalidMarkerColor { actual: u8, maximum_exclusive: u8 },
    #[error("ripple gap duration must be positive: {0}")]
    InvalidRippleDuration(TimeCode),
    #[error("title duration must be positive: {0}")]
    InvalidTitleDuration(TimeCode),
    #[error("clip {0} is not a title")]
    NotTitleClip(ClipId),
    #[error("unknown title parameter {0:?}")]
    UnknownTitleParam(String),
    #[error("title parameter {name:?} has the wrong value type")]
    InvalidTitleParamType { name: String },
    #[error("title parameter {name:?} is {actual}, outside the inclusive range {min}..={max}")]
    TitleParamOutOfRange {
        name: String,
        min: i64,
        max: i64,
        actual: i64,
    },
    #[error("title text exceeds the maximum length of {maximum} characters")]
    TitleTextTooLong { maximum: usize },
    #[error("title fade {name:?} ({frames} frames) exceeds clip {clip} duration {duration}")]
    TitleFadeTooLong {
        clip: ClipId,
        name: &'static str,
        frames: TimeCode,
        duration: TimeCode,
    },
    #[error("marker {marker} has no parameter {name:?}")]
    UnknownMarkerParam { marker: MarkerId, name: String },
    #[error("marker {marker} parameter {name:?} has the wrong value type")]
    InvalidMarkerParamType { marker: MarkerId, name: String },
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
        Operation::SetTrackSyncLock { track, locked } => set_track_sync_lock(doc, *track, *locked),
        Operation::AddClip {
            track,
            asset,
            at,
            source,
        } => add_clip(doc, *track, *asset, *at, source.clone()),
        Operation::AddTitle {
            track,
            at,
            duration,
            title,
        } => add_title(doc, *track, *at, *duration, title.clone()),
        Operation::SplitClip { clip, at } => split_clip(doc, *clip, *at),
        Operation::TrimClip { clip, new_source } => trim_clip(doc, *clip, new_source.clone()),
        Operation::MoveClip { clip, to_track, to } => move_clip(doc, *clip, *to_track, *to),
        Operation::DeleteClip { clip } => delete_clip(doc, *clip),
        Operation::RippleDeleteClip { clip } => ripple_delete_clip(doc, *clip),
        Operation::RippleInsertGap {
            track,
            at,
            duration,
        } => ripple_insert_gap(doc, *track, *at, *duration),
        Operation::LinkClips { clips } => link_clips(doc, clips),
        Operation::UnlinkClips { clips } => unlink_clips(doc, clips),
        Operation::AddMarker { marker } => add_marker(doc, marker.clone()),
        Operation::RemoveMarker { marker } => remove_marker(doc, *marker),
        Operation::MoveMarker { marker, to } => move_marker(doc, *marker, *to),
        Operation::AddEffect { clip, effect } => add_effect(doc, *clip, effect.clone()),
        Operation::RemoveEffect { clip, effect } => remove_effect(doc, *clip, *effect),
        Operation::SetEffectParam {
            clip,
            effect,
            name,
            value,
        } => set_effect_param(doc, *clip, *effect, name, value.clone()),
        Operation::SetTitleParam { clip, name, value } => {
            set_title_param(doc, *clip, name, value.clone())
        }
        Operation::AddTransition { clip, transition } => {
            add_transition(doc, *clip, transition.clone())
        }
        Operation::RemoveTransition { clip } => remove_transition(doc, *clip),
        Operation::SetMarkerParam {
            marker,
            name,
            value,
        } => set_marker_param(doc, *marker, name, value.clone()),
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

fn set_track_sync_lock(doc: &mut Document, track_id: TrackId, locked: bool) -> Result<(), OpError> {
    let track = doc
        .tracks
        .iter_mut()
        .find(|track| track.id == track_id)
        .ok_or(OpError::MissingTrack(track_id))?;
    track.sync_lock = locked;
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
        content: ClipContent::Media,
        timeline_start: at,
        effects: Vec::new(),
        transition_in: None,
        link: None,
    };
    doc.tracks[track_index].clips.push(clip);
    doc.tracks[track_index]
        .clips
        .sort_by_key(|clip| (clip.timeline_start, clip.id));
    Ok(())
}

fn add_title(
    doc: &mut Document,
    track_id: TrackId,
    at: TimeCode,
    duration: TimeCode,
    title: Title,
) -> Result<(), OpError> {
    if at < TimeCode::ZERO {
        return Err(OpError::NegativeTimelinePosition(at));
    }
    if duration <= TimeCode::ZERO {
        return Err(OpError::InvalidTitleDuration(duration));
    }
    let track_index = doc
        .tracks
        .iter()
        .position(|track| track.id == track_id)
        .ok_or(OpError::MissingTrack(track_id))?;
    if doc.tracks[track_index].kind != TrackKind::Video {
        return Err(OpError::TitleOnAudioTrack(track_id));
    }
    let clip_id = next_clip_id(doc)?;
    validate_title(clip_id, &title, duration)?;
    doc.tracks[track_index].clips.push(Clip {
        id: clip_id,
        asset: AssetId::default(),
        source_range: TimeCode::ZERO..duration,
        content: ClipContent::Title(title),
        timeline_start: at,
        effects: Vec::new(),
        transition_in: None,
        link: None,
    });
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

    let offset = at
        .checked_sub(original.timeline_start)
        .ok_or(OpError::TimeOverflow)?;
    let source_split = match &original.content {
        ClipContent::Media => {
            let asset = doc
                .asset(original.asset)
                .ok_or(OpError::MissingAsset(original.asset))?;
            find_source_boundary(original.source_range.clone(), offset, asset.fps, doc.fps)
                .ok_or(OpError::UnrepresentableSplit { clip: clip_id, at })?
        }
        ClipContent::Title(_) => original
            .source_range
            .start
            .checked_add(offset)
            .ok_or(OpError::TimeOverflow)?,
    };
    let new_id = next_clip_id(doc)?;

    doc.tracks[track_index].clips[clip_index].source_range.end = source_split;
    let mut right = original;
    right.id = new_id;
    right.source_range.start = source_split;
    right.timeline_start = at;
    right.transition_in = None;
    if let ClipContent::Title(title) = &mut doc.tracks[track_index].clips[clip_index].content {
        title.fade_in_frames = title.fade_in_frames.min(offset);
        title.fade_out_frames = TimeCode::ZERO;
    }
    if let ClipContent::Title(title) = &mut right.content {
        title.fade_in_frames = TimeCode::ZERO;
        let right_duration = end.checked_sub(at).ok_or(OpError::TimeOverflow)?;
        title.fade_out_frames = title.fade_out_frames.min(right_duration);
    }
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
    let source_fps = match &original.content {
        ClipContent::Media => {
            let asset = doc
                .asset(original.asset)
                .ok_or(OpError::MissingAsset(original.asset))?;
            validate_source_range(asset, &new_source)?;
            asset.fps
        }
        ClipContent::Title(_) => {
            validate_title_range(&new_source)?;
            doc.fps
        }
    };
    let shifted_start = match new_source.start.cmp(&original.source_range.start) {
        std::cmp::Ordering::Greater => {
            let offset = map_source_range_to_project(
                original.source_range.start..new_source.start,
                source_fps,
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
                source_fps,
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
    let duration = new_source
        .end
        .checked_sub(new_source.start)
        .ok_or(OpError::TimeOverflow)?;
    doc.tracks[track_index].clips[clip_index].timeline_start = shifted_start;
    doc.tracks[track_index].clips[clip_index].source_range = new_source;
    if let ClipContent::Title(title) = &mut doc.tracks[track_index].clips[clip_index].content {
        title.fade_in_frames = title.fade_in_frames.min(duration);
        title.fade_out_frames = title.fade_out_frames.min(duration);
    }
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
    match &doc.tracks[source_track_index].clips[clip_index].content {
        ClipContent::Media => {
            let asset_id = doc.tracks[source_track_index].clips[clip_index].asset;
            let asset = doc.asset(asset_id).ok_or(OpError::MissingAsset(asset_id))?;
            validate_track_compatibility(asset, &doc.tracks[target_track_index])?;
        }
        ClipContent::Title(_) => {
            if doc.tracks[target_track_index].kind != TrackKind::Video {
                return Err(OpError::TitleOnAudioTrack(target_track_id));
            }
        }
    }

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

fn ripple_delete_clip(doc: &mut Document, clip_id: ClipId) -> Result<(), OpError> {
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    let removed = doc.tracks[track_index].clips[clip_index].clone();
    let duration = doc.clip_duration(&removed)?;
    let ripple_point = removed
        .timeline_start
        .checked_add(duration)
        .ok_or(OpError::TimeOverflow)?;
    let source_track = doc.tracks[track_index].id;
    doc.tracks[track_index].clips.remove(clip_index);
    for track in &mut doc.tracks {
        if track.id != source_track && !track.sync_lock {
            continue;
        }
        for clip in track
            .clips
            .iter_mut()
            .filter(|clip| clip.timeline_start >= ripple_point)
        {
            clip.timeline_start = clip
                .timeline_start
                .checked_sub(duration)
                .ok_or(OpError::TimeOverflow)?;
        }
    }
    shift_markers_left(&mut doc.markers, ripple_point, duration)?;
    Ok(())
}

fn ripple_insert_gap(
    doc: &mut Document,
    track_id: TrackId,
    at: TimeCode,
    duration: TimeCode,
) -> Result<(), OpError> {
    if at < TimeCode::ZERO {
        return Err(OpError::NegativeTimelinePosition(at));
    }
    if duration <= TimeCode::ZERO {
        return Err(OpError::InvalidRippleDuration(duration));
    }
    if !doc.tracks.iter().any(|track| track.id == track_id) {
        return Err(OpError::MissingTrack(track_id));
    }
    for track in &mut doc.tracks {
        if track.id != track_id && !track.sync_lock {
            continue;
        }
        for clip in track
            .clips
            .iter_mut()
            .filter(|clip| clip.timeline_start >= at)
        {
            clip.timeline_start = clip
                .timeline_start
                .checked_add(duration)
                .ok_or(OpError::TimeOverflow)?;
        }
    }
    shift_markers_right(&mut doc.markers, at, duration)?;
    Ok(())
}

fn shift_markers_left(
    markers: &mut [Marker],
    boundary: TimeCode,
    duration: TimeCode,
) -> Result<(), OpError> {
    for marker in markers
        .iter_mut()
        .filter(|marker| marker.position >= boundary)
    {
        marker.position = marker
            .position
            .checked_sub(duration)
            .ok_or(OpError::TimeOverflow)?
            .max(TimeCode::ZERO);
    }
    markers.sort_by_key(|marker| (marker.position, marker.id));
    Ok(())
}

fn shift_markers_right(
    markers: &mut [Marker],
    boundary: TimeCode,
    duration: TimeCode,
) -> Result<(), OpError> {
    for marker in markers
        .iter_mut()
        .filter(|marker| marker.position >= boundary)
    {
        marker.position = marker
            .position
            .checked_add(duration)
            .ok_or(OpError::TimeOverflow)?;
    }
    markers.sort_by_key(|marker| (marker.position, marker.id));
    Ok(())
}

fn validate_clip_selection(
    doc: &Document,
    clips: &[ClipId],
    minimum: usize,
) -> Result<(), OpError> {
    if clips.len() < minimum {
        return Err(OpError::TooFewLinkedClips { minimum });
    }
    let mut selected = HashSet::new();
    for clip in clips {
        if !selected.insert(*clip) {
            return Err(OpError::DuplicateClipSelection(*clip));
        }
        find_clip(doc, *clip)?;
    }
    Ok(())
}

fn link_clips(doc: &mut Document, clips: &[ClipId]) -> Result<(), OpError> {
    validate_clip_selection(doc, clips, 2)?;
    let link = next_link_id(doc)?;
    let selected = clips.iter().copied().collect::<HashSet<_>>();
    for clip in doc.tracks.iter_mut().flat_map(|track| &mut track.clips) {
        if selected.contains(&clip.id) {
            clip.link = Some(link);
        }
    }
    Ok(())
}

fn unlink_clips(doc: &mut Document, clips: &[ClipId]) -> Result<(), OpError> {
    validate_clip_selection(doc, clips, 1)?;
    let selected = clips.iter().copied().collect::<HashSet<_>>();
    for clip in doc.tracks.iter_mut().flat_map(|track| &mut track.clips) {
        if selected.contains(&clip.id) {
            clip.link = None;
        }
    }
    Ok(())
}

fn add_marker(doc: &mut Document, marker: Marker) -> Result<(), OpError> {
    if doc.marker(marker.id).is_some() {
        return Err(OpError::DuplicateMarker(marker.id));
    }
    validate_marker(&marker)?;
    doc.markers.push(marker);
    doc.markers
        .sort_by_key(|marker| (marker.position, marker.id));
    Ok(())
}

fn remove_marker(doc: &mut Document, marker_id: MarkerId) -> Result<(), OpError> {
    let index = doc
        .markers
        .iter()
        .position(|marker| marker.id == marker_id)
        .ok_or(OpError::MissingMarker(marker_id))?;
    doc.markers.remove(index);
    Ok(())
}

fn move_marker(doc: &mut Document, marker_id: MarkerId, to: TimeCode) -> Result<(), OpError> {
    if to < TimeCode::ZERO {
        return Err(OpError::NegativeMarkerPosition(to));
    }
    let marker = doc
        .markers
        .iter_mut()
        .find(|marker| marker.id == marker_id)
        .ok_or(OpError::MissingMarker(marker_id))?;
    marker.position = to;
    doc.markers
        .sort_by_key(|marker| (marker.position, marker.id));
    Ok(())
}

fn set_marker_param(
    doc: &mut Document,
    marker_id: MarkerId,
    name: &str,
    value: ParamValue,
) -> Result<(), OpError> {
    let marker = doc
        .markers
        .iter_mut()
        .find(|marker| marker.id == marker_id)
        .ok_or(OpError::MissingMarker(marker_id))?;
    match (name, value) {
        ("label", ParamValue::Text(label)) => marker.label = label,
        ("color_token", ParamValue::Integer(token)) => {
            marker.color_token = u8::try_from(token).map_err(|_| OpError::InvalidMarkerColor {
                actual: u8::MAX,
                maximum_exclusive: MARKER_COLOR_TOKEN_COUNT,
            })?;
        }
        ("position", ParamValue::Integer(position)) => marker.position = TimeCode(position),
        ("label" | "color_token" | "position", _) => {
            return Err(OpError::InvalidMarkerParamType {
                marker: marker_id,
                name: name.to_owned(),
            });
        }
        _ => {
            return Err(OpError::UnknownMarkerParam {
                marker: marker_id,
                name: name.to_owned(),
            });
        }
    }
    validate_marker(marker)?;
    doc.markers
        .sort_by_key(|marker| (marker.position, marker.id));
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

fn set_title_param(
    doc: &mut Document,
    clip_id: ClipId,
    name: &str,
    value: ParamValue,
) -> Result<(), OpError> {
    let (track_index, clip_index) = find_clip(doc, clip_id)?;
    let duration = doc.clip_duration(&doc.tracks[track_index].clips[clip_index])?;
    let ClipContent::Title(title) = &mut doc.tracks[track_index].clips[clip_index].content else {
        return Err(OpError::NotTitleClip(clip_id));
    };
    validate_title_parameter(name, &value)?;
    match (name, value) {
        ("text", ParamValue::Text(value)) => title.text = value,
        ("font_size_token", ParamValue::Integer(value)) => {
            title.font_size_token =
                u8::try_from(value).map_err(|_| OpError::TitleParamOutOfRange {
                    name: name.to_owned(),
                    min: 0,
                    max: i64::from(u8::MAX),
                    actual: value,
                })?;
        }
        ("color_token", ParamValue::Integer(value)) => {
            title.color_token = u8::try_from(value).map_err(|_| OpError::TitleParamOutOfRange {
                name: name.to_owned(),
                min: 0,
                max: i64::from(u8::MAX),
                actual: value,
            })?;
        }
        ("position", ParamValue::Text(value)) => {
            title.position = value.parse().map_err(|()| OpError::InvalidTitleParamType {
                name: name.to_owned(),
            })?;
        }
        ("background_scrim", ParamValue::Boolean(value)) => title.background_scrim = value,
        ("fade_in_frames", ParamValue::Integer(value)) => {
            title.fade_in_frames = TimeCode(value);
        }
        ("fade_out_frames", ParamValue::Integer(value)) => {
            title.fade_out_frames = TimeCode(value);
        }
        _ => {
            return Err(OpError::InvalidTitleParamType {
                name: name.to_owned(),
            });
        }
    }
    validate_title(clip_id, title, duration)
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

fn next_link_id(doc: &Document) -> Result<LinkId, OpError> {
    doc.tracks
        .iter()
        .flat_map(|track| &track.clips)
        .filter_map(|clip| clip.link)
        .map(|link| link.0)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .map(LinkId)
        .ok_or(OpError::LinkIdExhausted)
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
    let Some(descriptor) = crate::effect_descriptor(&effect.name) else {
        return Err(OpError::UnknownEffect(effect.name.clone()));
    };
    for (name, value) in &effect.parameters {
        validate_described_effect_parameter(descriptor, name, value)?;
    }
    Ok(())
}

fn validate_effect_parameter(effect: &str, name: &str, value: &ParamValue) -> Result<(), OpError> {
    let Some(descriptor) = crate::effect_descriptor(effect) else {
        return Err(OpError::UnknownEffect(effect.to_owned()));
    };
    validate_described_effect_parameter(descriptor, name, value)
}

fn validate_described_effect_parameter(
    effect: crate::EffectDescriptor,
    name: &str,
    value: &ParamValue,
) -> Result<(), OpError> {
    let Some(parameter) = effect.parameter(name) else {
        return Err(OpError::UnknownEffectParam {
            effect: effect.name.to_owned(),
            name: name.to_owned(),
        });
    };
    let ParamValue::Integer(actual) = value else {
        return Err(OpError::InvalidEffectParamType {
            effect: effect.name.to_owned(),
            name: name.to_owned(),
        });
    };
    if !(parameter.min..=parameter.max).contains(actual) {
        return Err(OpError::EffectParamOutOfRange {
            effect: effect.name.to_owned(),
            name: name.to_owned(),
            min: parameter.min,
            max: parameter.max,
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

fn validate_marker(marker: &Marker) -> Result<(), OpError> {
    if marker.position < TimeCode::ZERO {
        return Err(OpError::NegativeMarkerPosition(marker.position));
    }
    if marker.color_token >= MARKER_COLOR_TOKEN_COUNT {
        return Err(OpError::InvalidMarkerColor {
            actual: marker.color_token,
            maximum_exclusive: MARKER_COLOR_TOKEN_COUNT,
        });
    }
    Ok(())
}

fn validate_title_parameter(name: &str, value: &ParamValue) -> Result<(), OpError> {
    let descriptor = title_parameter_descriptor(name)
        .ok_or_else(|| OpError::UnknownTitleParam(name.to_owned()))?;
    match (descriptor.kind, value) {
        (TitleParameterKind::Text { maximum_characters }, ParamValue::Text(text)) => {
            if text.chars().count() > maximum_characters {
                return Err(OpError::TitleTextTooLong {
                    maximum: maximum_characters,
                });
            }
        }
        (TitleParameterKind::Integer { min, max }, ParamValue::Integer(actual)) => {
            if !(min..=max).contains(actual) {
                return Err(OpError::TitleParamOutOfRange {
                    name: name.to_owned(),
                    min,
                    max,
                    actual: *actual,
                });
            }
        }
        (TitleParameterKind::Boolean, ParamValue::Boolean(_)) => {}
        (TitleParameterKind::Position, ParamValue::Text(position)) => {
            if position.parse::<TitlePosition>().is_err() {
                return Err(OpError::InvalidTitleParamType {
                    name: name.to_owned(),
                });
            }
        }
        _ => {
            return Err(OpError::InvalidTitleParamType {
                name: name.to_owned(),
            });
        }
    }
    Ok(())
}

fn validate_title(clip: ClipId, title: &Title, duration: TimeCode) -> Result<(), OpError> {
    for descriptor in crate::TITLE_PARAMETER_DESCRIPTORS {
        let value = crate::title_parameter_value(title, descriptor.name)
            .expect("every title descriptor has a typed value");
        validate_title_parameter(descriptor.name, &value)?;
    }
    for (name, frames) in [
        ("fade_in_frames", title.fade_in_frames),
        ("fade_out_frames", title.fade_out_frames),
    ] {
        if frames > duration {
            return Err(OpError::TitleFadeTooLong {
                clip,
                name,
                frames,
                duration,
            });
        }
    }
    Ok(())
}

fn validate_title_range(source: &std::ops::Range<TimeCode>) -> Result<(), OpError> {
    if source.start < TimeCode::ZERO || source.end <= source.start {
        return Err(OpError::InvalidSourceRange {
            start: source.start.0,
            end: source.end.0,
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

#[allow(clippy::too_many_lines)]
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
            match &clip.content {
                ClipContent::Media => {
                    let asset = doc
                        .asset(clip.asset)
                        .ok_or(OpError::MissingAsset(clip.asset))?;
                    validate_track_compatibility(asset, track)?;
                    validate_source_range(asset, &clip.source_range)?;
                }
                ClipContent::Title(title) => {
                    if track.kind != TrackKind::Video {
                        return Err(OpError::TitleOnAudioTrack(track.id));
                    }
                    validate_title_range(&clip.source_range)?;
                    let duration = doc.clip_duration(clip)?;
                    validate_title(clip.id, title, duration)?;
                }
            }
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

    let mut marker_ids = HashSet::new();
    let mut previous_marker: Option<&Marker> = None;
    for marker in &doc.markers {
        if !marker_ids.insert(marker.id) {
            return Err(OpError::DuplicateMarker(marker.id));
        }
        validate_marker(marker)?;
        if let Some(previous) = previous_marker
            && marker.position < previous.position
        {
            return Err(OpError::MarkersUnsorted {
                previous: previous.id,
                next: marker.id,
            });
        }
        previous_marker = Some(marker);
    }

    if doc.duration != expected_duration {
        return Err(OpError::IncorrectDocumentDuration {
            expected: expected_duration,
            actual: doc.duration,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_shift_left_clamps_negative_results_to_zero() {
        let mut markers = [Marker {
            id: MarkerId(1),
            position: TimeCode(5),
            label: String::new(),
            color_token: 0,
        }];

        shift_markers_left(&mut markers, TimeCode(5), TimeCode(10)).unwrap();

        assert_eq!(markers[0].position, TimeCode::ZERO);
    }
}
