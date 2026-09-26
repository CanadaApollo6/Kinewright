use std::ops::Range;

use kinewright_core::{
    AssetId, BlendMode, Clip, ClipContent, ClipId, Document, Effect, FrameRounding, MediaError,
    MediaKind, SolidColor, TimeCode, Title, Track, TrackId, TrackKind, TransitionAxis,
    TransitionShading, map_frames_with_rounding, map_source_range_to_project,
    transition_descriptor,
};

/// The source frame selected by a project-frame position on the first video track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimelineSource {
    pub track: TrackId,
    pub clip: ClipId,
    pub asset: AssetId,
    pub source_at: TimeCode,
    pub source_end: TimeCode,
    pub timeline_end: TimeCode,
}

/// One active video layer at a project frame. Layers are returned in document
/// track order, which is the project's bottom-to-top z-order.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineVideoLayer {
    pub source: TimelineSource,
    pub effects: Vec<Effect>,
    pub transition: TransitionRenderParams,
    /// MO2 R1: how the layer blends onto the composite below it.
    pub blend_mode: BlendMode,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimelineTitleLayer {
    pub track: TrackId,
    pub clip: ClipId,
    pub title: Title,
    pub effects: Vec<Effect>,
    pub transition: TransitionRenderParams,
    pub blend_mode: BlendMode,
}

/// MO2 R3: a solid-colour clip. Generated content, like a title: the fill
/// enters working space through the shared display-frame conversion.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineSolidLayer {
    pub track: TrackId,
    pub clip: ClipId,
    pub color: SolidColor,
    pub effects: Vec<Effect>,
    pub transition: TransitionRenderParams,
    pub blend_mode: BlendMode,
}

/// MO2 R18: an adjustment instruction. It has no pixels of its own: the
/// compositor grades the composite of the layers below it (R17).
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineAdjustmentLayer {
    pub track: TrackId,
    pub clip: ClipId,
    pub effects: Vec<Effect>,
    pub transition: TransitionRenderParams,
    pub blend_mode: BlendMode,
}

/// Per-layer transition shading evaluated for one project frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransitionRenderParams {
    pub alpha: f32,
    pub fade_mix: f32,
    pub fade_white: f32,
    /// MO2 R21: the entering layer's displacement in screen fractions,
    /// positive right/down (Push, Slide).
    pub offset: [f32; 2],
    /// MO2 R21: output-space coverage while a geometric transition is active.
    pub coverage: Option<TransitionCoverage>,
    /// MO2 R13: the Push backdrop displacement `q` in screen fractions;
    /// `Some` only while a Push is active.
    pub backdrop: Option<[f32; 2]>,
}

/// MO2 R21: the revealed region, tested at output pixel centres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransitionCoverage {
    pub axis: TransitionAxis,
    /// Keep `coordinate < edge` (entering from the left/top); otherwise keep
    /// `coordinate >= edge`.
    pub below_edge: bool,
    pub edge: f32,
}

impl Default for TransitionRenderParams {
    fn default() -> Self {
        Self {
            alpha: 1.0,
            fade_mix: 0.0,
            fade_white: 0.0,
            offset: [0.0; 2],
            coverage: None,
            backdrop: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TimelineVisualLayer {
    Video(TimelineVideoLayer),
    Title(TimelineTitleLayer),
    Solid(TimelineSolidLayer),
    Adjustment(TimelineAdjustmentLayer),
}

impl TimelineVisualLayer {
    /// MO2 R32: the clip this layer was resolved from, whatever its kind.
    #[must_use]
    pub const fn clip(&self) -> ClipId {
        match self {
            Self::Video(layer) => layer.source.clip,
            Self::Title(layer) => layer.clip,
            Self::Solid(layer) => layer.clip,
            Self::Adjustment(layer) => layer.clip,
        }
    }

    /// The keyframe-evaluated, enabled effects of this layer.
    #[must_use]
    pub fn effects(&self) -> &[Effect] {
        match self {
            Self::Video(layer) => &layer.effects,
            Self::Title(layer) => &layer.effects,
            Self::Solid(layer) => &layer.effects,
            Self::Adjustment(layer) => &layer.effects,
        }
    }
}

/// One audio-bearing portion of a timeline clip within a requested project range.
///
/// The project and source ranges are half-open. Source boundaries use floor at
/// the start and ceil at the end so the integer source range covers the entire
/// requested project segment without crossing the clip's trim boundaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineAudioSegment {
    pub track: TrackId,
    pub clip: ClipId,
    pub asset: AssetId,
    pub project: Range<TimeCode>,
    pub source: Range<TimeCode>,
}

/// Map a project frame to its active clip and source frame.
///
/// Clip intervals are half-open. A position in a gap, before the first clip, or
/// at/after the document duration maps to `None`.
///
/// # Errors
///
/// Returns a media error when exact source/project frame mapping fails.
pub fn timeline_source_at(
    document: &Document,
    project_at: TimeCode,
) -> Result<Option<TimelineSource>, MediaError> {
    if project_at < TimeCode::ZERO {
        return Ok(None);
    }
    let Some(track) = document
        .tracks
        .iter()
        .find(|track| track.kind == TrackKind::Video)
    else {
        return Ok(None);
    };

    source_on_track(document, track, project_at)
}

/// Resolve every active video track at a project frame, bottom-to-top.
///
/// # Errors
///
/// Returns a media error when exact source/project frame mapping fails.
pub fn video_layers_at(
    document: &Document,
    project_at: TimeCode,
) -> Result<Vec<TimelineVideoLayer>, MediaError> {
    if project_at < TimeCode::ZERO {
        return Ok(Vec::new());
    }
    let mut layers = Vec::new();
    for track in document
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Video)
    {
        if let Some(source) = source_on_track(document, track, project_at)? {
            let clip = track
                .clips
                .iter()
                .find(|clip| clip.id == source.clip)
                .ok_or_else(|| {
                    MediaError::Backend("active timeline clip disappeared".to_owned())
                })?;
            layers.push(TimelineVideoLayer {
                source,
                effects: evaluated_effects(clip, project_at),
                transition: transition_render_params(clip, project_at),
                blend_mode: clip.blend_mode,
            });
        }
    }
    Ok(layers)
}

/// Resolve every active visual layer at a project frame in bottom-to-top track order.
///
/// # Errors
///
/// Returns a media error when exact source/project frame mapping fails.
pub fn visual_layers_at(
    document: &Document,
    project_at: TimeCode,
) -> Result<Vec<TimelineVisualLayer>, MediaError> {
    if project_at < TimeCode::ZERO {
        return Ok(Vec::new());
    }
    let mut layers = Vec::new();
    for track in document
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Video)
    {
        let Some(clip) = active_clip_on_track(document, track, project_at)? else {
            continue;
        };
        let effects = evaluated_effects(clip, project_at);
        let transition = transition_render_params(clip, project_at);
        let blend_mode = clip.blend_mode;
        match &clip.content {
            ClipContent::Media => {
                let source = media_source_for_clip(document, track.id, clip, project_at)?;
                layers.push(TimelineVisualLayer::Video(TimelineVideoLayer {
                    source,
                    effects,
                    transition,
                    blend_mode,
                }));
            }
            ClipContent::Title(title) => {
                let mut transition = transition;
                transition.alpha *= title_alpha(document, clip, title, project_at)?;
                layers.push(TimelineVisualLayer::Title(TimelineTitleLayer {
                    track: track.id,
                    clip: clip.id,
                    title: title.clone(),
                    effects,
                    transition,
                    blend_mode,
                }));
            }
            ClipContent::Freeze(freeze) => {
                let duration = document
                    .clip_duration(clip)
                    .map_err(|error| MediaError::Backend(error.to_string()))?;
                let timeline_end = clip.timeline_start.checked_add(duration).ok_or_else(|| {
                    MediaError::Backend("timeline position overflowed".to_owned())
                })?;
                let source_end = freeze
                    .source_frame
                    .checked_add(TimeCode(1))
                    .ok_or_else(|| MediaError::Backend("source position overflowed".to_owned()))?;
                layers.push(TimelineVisualLayer::Video(TimelineVideoLayer {
                    source: TimelineSource {
                        track: track.id,
                        clip: clip.id,
                        asset: clip.asset,
                        source_at: freeze.source_frame,
                        source_end,
                        timeline_end,
                    },
                    effects,
                    transition,
                    blend_mode,
                }));
            }
            ClipContent::Solid(color) => {
                layers.push(TimelineVisualLayer::Solid(TimelineSolidLayer {
                    track: track.id,
                    clip: clip.id,
                    color: *color,
                    effects,
                    transition,
                    blend_mode,
                }));
            }
            ClipContent::Adjustment => {
                layers.push(TimelineVisualLayer::Adjustment(TimelineAdjustmentLayer {
                    track: track.id,
                    clip: clip.id,
                    effects,
                    transition,
                    blend_mode,
                }));
            }
        }
    }
    Ok(layers)
}

fn evaluated_effects(clip: &Clip, project_at: TimeCode) -> Vec<Effect> {
    let local_at = project_at
        .checked_sub(clip.timeline_start)
        .unwrap_or(TimeCode::ZERO);
    clip.effects
        .iter()
        // MO1 R4: a disabled effect is absent from the resolved layer.
        .filter(|effect| effect.is_enabled_at(local_at))
        .map(|effect| effect.evaluated_at(local_at))
        .collect()
}

/// Enumerate every audio-bearing clip portion intersecting a project range.
///
/// Both audio tracks and video tracks backed by audio/video assets participate.
/// Results preserve document track and clip order, which is also the mix order.
///
/// # Errors
///
/// Returns a media error for an invalid requested range, a missing timeline
/// asset, or an exact source/project mapping failure.
pub fn timeline_audio_segments(
    document: &Document,
    project: Range<TimeCode>,
) -> Result<Vec<TimelineAudioSegment>, MediaError> {
    if project.start < TimeCode::ZERO || project.end <= project.start {
        return Err(MediaError::Backend(format!(
            "timeline audio range must be non-empty and non-negative: {}..{}",
            project.start.0, project.end.0
        )));
    }
    let requested_end = project.end.min(document.duration);
    if project.start >= requested_end {
        return Ok(Vec::new());
    }

    let mut segments = Vec::new();
    for track in &document.tracks {
        for clip in &track.clips {
            if !clip.content.is_media() {
                continue;
            }
            let asset = document.asset(clip.asset).ok_or_else(|| {
                MediaError::Backend(format!(
                    "timeline clip {} references missing asset {}",
                    clip.id, clip.asset
                ))
            })?;
            if !matches!(asset.kind, MediaKind::Audio | MediaKind::AudioVideo) {
                continue;
            }
            if clip.speed_percent != 100 {
                continue;
            }
            let duration =
                map_source_range_to_project(clip.source_range.clone(), asset.fps, document.fps)
                    .map_err(|error| MediaError::Backend(error.to_string()))?;
            let clip_end = clip.timeline_start.checked_add(duration).ok_or_else(|| {
                MediaError::Backend("timeline audio position overflowed".to_owned())
            })?;
            let project_start = clip.timeline_start.max(project.start);
            let project_end = clip_end.min(requested_end);
            if project_end <= project_start {
                continue;
            }

            let start_offset = project_start
                .checked_sub(clip.timeline_start)
                .ok_or_else(|| MediaError::Backend("timeline position underflowed".to_owned()))?;
            let end_offset = project_end
                .checked_sub(clip.timeline_start)
                .ok_or_else(|| MediaError::Backend("timeline position underflowed".to_owned()))?;
            // MO1 R5: a statically disabled clip contributes silence (no
            // segment); a keyframed `enabled_curve` splits the intersection
            // into maximal enabled runs, one segment each. Without a curve
            // the single run below is exactly the pre-MO1 segment.
            if clip.enabled_curve.is_none() && !clip.enabled {
                continue;
            }
            let runs = match &clip.enabled_curve {
                None => vec![(start_offset, end_offset)],
                Some(_) => enabled_clip_runs(clip, start_offset, end_offset),
            };
            for (run_start, run_end) in runs {
                let run_project_start =
                    clip.timeline_start.checked_add(run_start).ok_or_else(|| {
                        MediaError::Backend("timeline position overflowed".to_owned())
                    })?;
                let run_project_end =
                    clip.timeline_start.checked_add(run_end).ok_or_else(|| {
                        MediaError::Backend("timeline position overflowed".to_owned())
                    })?;
                let source_start_offset = map_frames_with_rounding(
                    run_start,
                    document.fps,
                    asset.fps,
                    FrameRounding::Floor,
                )
                .map_err(|error| MediaError::Backend(error.to_string()))?;
                let source_end_offset =
                    map_frames_with_rounding(run_end, document.fps, asset.fps, FrameRounding::Ceil)
                        .map_err(|error| MediaError::Backend(error.to_string()))?;
                let source_start = clip
                    .source_range
                    .start
                    .checked_add(source_start_offset)
                    .ok_or_else(|| MediaError::Backend("source position overflowed".to_owned()))?;
                let source_end = clip
                    .source_range
                    .start
                    .checked_add(source_end_offset)
                    .ok_or_else(|| MediaError::Backend("source position overflowed".to_owned()))?;
                segments.push(TimelineAudioSegment {
                    track: track.id,
                    clip: clip.id,
                    asset: clip.asset,
                    project: run_project_start..run_project_end,
                    source: TimeCode(source_start.0.min(clip.source_range.end.0))
                        ..TimeCode(source_end.0.min(clip.source_range.end.0)),
                });
            }
        }
    }
    Ok(segments)
}

/// Maximal contiguous clip-local runs of `[start, end)` where
/// [`Clip::is_enabled_at`] holds (MO1 R5).
///
/// Per-frame evaluation is the exact semantics: the ≥ 1 test resolves every
/// frame, including mid-ramp frames of non-`Hold` enable curves. Only called
/// when the clip carries an `enabled_curve`; the static case never walks.
fn enabled_clip_runs(clip: &Clip, start: TimeCode, end: TimeCode) -> Vec<(TimeCode, TimeCode)> {
    let mut runs = Vec::new();
    let mut cursor = start.0;
    while cursor < end.0 {
        if !clip.is_enabled_at(TimeCode(cursor)) {
            cursor += 1;
            continue;
        }
        let run_start = cursor;
        cursor += 1;
        while cursor < end.0 && clip.is_enabled_at(TimeCode(cursor)) {
            cursor += 1;
        }
        runs.push((TimeCode(run_start), TimeCode(cursor)));
    }
    runs
}

fn source_on_track(
    document: &Document,
    track: &Track,
    project_at: TimeCode,
) -> Result<Option<TimelineSource>, MediaError> {
    let Some(clip) = active_clip_on_track(document, track, project_at)? else {
        return Ok(None);
    };
    if !clip.content.is_media() {
        return Ok(None);
    }
    media_source_for_clip(document, track.id, clip, project_at).map(Some)
}

fn active_clip_on_track<'a>(
    document: &Document,
    track: &'a Track,
    project_at: TimeCode,
) -> Result<Option<&'a Clip>, MediaError> {
    for clip in &track.clips {
        if project_at < clip.timeline_start {
            break;
        }
        let duration = document
            .clip_duration(clip)
            .map_err(|error| MediaError::Backend(error.to_string()))?;
        let timeline_end = clip
            .timeline_start
            .checked_add(duration)
            .ok_or_else(|| MediaError::Backend("timeline position overflowed".to_owned()))?;
        if project_at < timeline_end {
            // MO1 R5: a disabled clip is absent from every layer resolution
            // (`timeline_source_at`, `video_layers_at`, `visual_layers_at` all
            // route through here). `continue`, not `None`, so an overlapping
            // later clip could still cover the frame.
            let local = project_at
                .checked_sub(clip.timeline_start)
                .unwrap_or(TimeCode::ZERO);
            if !clip.is_enabled_at(local) {
                continue;
            }
            return Ok(Some(clip));
        }
    }
    Ok(None)
}

fn media_source_for_clip(
    document: &Document,
    track: TrackId,
    clip: &Clip,
    project_at: TimeCode,
) -> Result<TimelineSource, MediaError> {
    let asset = document.asset(clip.asset).ok_or_else(|| {
        MediaError::Backend(format!(
            "timeline clip {} references missing asset {}",
            clip.id, clip.asset
        ))
    })?;
    let effective_fps = kinewright_core::clip_effective_fps(asset.fps, clip)
        .map_err(|error| MediaError::Backend(error.to_string()))?;
    let duration =
        map_source_range_to_project(clip.source_range.clone(), effective_fps, document.fps)
            .map_err(|error| MediaError::Backend(error.to_string()))?;
    let timeline_end = clip
        .timeline_start
        .checked_add(duration)
        .ok_or_else(|| MediaError::Backend("timeline position overflowed".to_owned()))?;
    let project_offset = project_at
        .checked_sub(clip.timeline_start)
        .ok_or_else(|| MediaError::Backend("timeline position underflowed".to_owned()))?;
    let source_offset = map_frames_with_rounding(
        project_offset,
        document.fps,
        effective_fps,
        FrameRounding::Floor,
    )
    .map_err(|error| MediaError::Backend(error.to_string()))?;
    let source_at = clip
        .source_range
        .start
        .checked_add(source_offset)
        .ok_or_else(|| MediaError::Backend("source position overflowed".to_owned()))?;
    Ok(TimelineSource {
        track,
        clip: clip.id,
        asset: clip.asset,
        source_at: TimeCode(source_at.0.min(clip.source_range.end.0.saturating_sub(1))),
        source_end: clip.source_range.end,
        timeline_end,
    })
}

// GPU shading is f32; projecting integer frame offsets is the intended final conversion.
#[allow(clippy::cast_precision_loss)]
fn transition_render_params(clip: &Clip, project_at: TimeCode) -> TransitionRenderParams {
    let Some(transition) = &clip.transition_in else {
        return TransitionRenderParams::default();
    };
    if transition.duration.0 <= 1 {
        return TransitionRenderParams::default();
    }
    let Some(descriptor) = transition_descriptor(&transition.name) else {
        return TransitionRenderParams::default();
    };
    let offset = project_at.0.saturating_sub(clip.timeline_start.0);
    let progress = (offset as f32 / (transition.duration.0 - 1) as f32).clamp(0.0, 1.0);
    match descriptor.shading {
        TransitionShading::CrossfadeAlpha => TransitionRenderParams {
            alpha: progress,
            ..TransitionRenderParams::default()
        },
        TransitionShading::FadeFromColor { white } => TransitionRenderParams {
            fade_mix: 1.0 - progress,
            fade_white: if white { 1.0 } else { 0.0 },
            ..TransitionRenderParams::default()
        },
        TransitionShading::Push { axis, sign }
        | TransitionShading::Slide { axis, sign }
        | TransitionShading::Wipe { axis, sign } => {
            // MO2 R20: active only before `offset = d − 1`; from there on the
            // layer is its ordinary authored contribution.
            if offset >= transition.duration.0 - 1 {
                return TransitionRenderParams::default();
            }
            let sign = f32::from(sign);
            let along = |value: f32| match axis {
                TransitionAxis::Horizontal => [value, 0.0],
                TransitionAxis::Vertical => [0.0, value],
            };
            let push = matches!(descriptor.shading, TransitionShading::Push { .. });
            let wipe = matches!(descriptor.shading, TransitionShading::Wipe { .. });
            // MO2 R21: entering `sign·(1−p)`, backdrop `−sign·p`; coverage
            // keeps `x < p` from the left/top, `x ≥ 1 − p` from the right/bottom.
            TransitionRenderParams {
                offset: if wipe {
                    [0.0; 2]
                } else {
                    along(sign * (1.0 - progress))
                },
                coverage: Some(TransitionCoverage {
                    axis,
                    below_edge: sign < 0.0,
                    edge: if sign < 0.0 { progress } else { 1.0 - progress },
                }),
                backdrop: push.then(|| along(-sign * progress)),
                ..TransitionRenderParams::default()
            }
        }
    }
}

#[allow(clippy::cast_precision_loss)]
fn title_alpha(
    document: &Document,
    clip: &Clip,
    title: &Title,
    project_at: TimeCode,
) -> Result<f32, MediaError> {
    let duration = document
        .clip_duration(clip)
        .map_err(|error| MediaError::Backend(error.to_string()))?;
    let offset = project_at.0.saturating_sub(clip.timeline_start.0);
    let remaining = duration.0.saturating_sub(offset).saturating_sub(1);
    let fade_in = if title.fade_in_frames.0 <= 1 {
        1.0
    } else {
        (offset as f32 / (title.fade_in_frames.0 - 1) as f32).clamp(0.0, 1.0)
    };
    let fade_out = if title.fade_out_frames.0 <= 1 {
        1.0
    } else {
        (remaining as f32 / (title.fade_out_frames.0 - 1) as f32).clamp(0.0, 1.0)
    };
    Ok(fade_in.min(fade_out))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use kinewright_core::{
        AssetId, AutomationCurve, BlendMode, Clip, ClipId, Document, Effect, EffectId, FreezeFrame,
        Keyframe, KeyframeInterpolation, MediaAsset, MediaKind, ParamValue, Rational, TimeCode,
        Track, TrackId, TrackKind, Transition,
    };

    use super::*;

    fn fixture() -> Document {
        Document {
            investigator: None,
            catalog: kinewright_core::MediaCatalog::default(),
            audio_mix: kinewright_core::AudioMix::default(),
            color_context: kinewright_core::ColorContext::default(),
            lut_assets: Vec::new(),
            tracks: vec![Track {
                id: TrackId(7),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![
                    Clip {
                        enabled: true,
                        enabled_curve: None,
                        id: ClipId(1),
                        asset: AssetId(1),
                        source_range: TimeCode(10)..TimeCode(20),
                        content: ClipContent::Media,
                        timeline_start: TimeCode(0),
                        effects: Vec::new(),
                        transition_in: None,
                        link: None,
                        audio_gain_tenth_db: 0,
                        audio_fade_in_frames: TimeCode::ZERO,
                        audio_fade_out_frames: TimeCode::ZERO,
                        speed_percent: 100,
                        audio_gain_curve: None,
                        blend_mode: BlendMode::Normal,
                    },
                    Clip {
                        enabled: true,
                        enabled_curve: None,
                        id: ClipId(2),
                        asset: AssetId(2),
                        source_range: TimeCode(30)..TimeCode(40),
                        content: ClipContent::Media,
                        timeline_start: TimeCode(15),
                        effects: Vec::new(),
                        transition_in: None,
                        link: None,
                        audio_gain_tenth_db: 0,
                        audio_fade_in_frames: TimeCode::ZERO,
                        audio_fade_out_frames: TimeCode::ZERO,
                        speed_percent: 100,
                        audio_gain_curve: None,
                        blend_mode: BlendMode::Normal,
                    },
                ],
            }],
            media_pool: vec![
                MediaAsset {
                    id: AssetId(1),
                    path: PathBuf::from("one.mp4"),
                    name: "one".to_owned(),
                    duration: TimeCode(60),
                    fps: Rational::new(30, 1).unwrap(),
                    kind: MediaKind::AudioVideo,
                    resolution: Some((320, 180)),
                    source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
                    color_description: kinewright_core::ColorDescription::default(),
                    assumed_from: None,
                },
                MediaAsset {
                    id: AssetId(2),
                    path: PathBuf::from("two.mp4"),
                    name: "two".to_owned(),
                    duration: TimeCode(60),
                    fps: Rational::new(30, 1).unwrap(),
                    kind: MediaKind::AudioVideo,
                    resolution: Some((320, 180)),
                    source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
                    color_description: kinewright_core::ColorDescription::default(),
                    assumed_from: None,
                },
            ],
            markers: Vec::new(),
            fps: Rational::new(30, 1).unwrap(),
            resolution: (320, 180),
            duration: TimeCode(25),
        }
    }

    #[test]
    fn visual_layers_resolve_effect_automation_at_clip_local_frames() {
        let mut document = fixture();
        document.tracks[0].clips[0].effects.push(Effect {
            enabled: true,
            enabled_curve: None,
            id: EffectId(1),
            name: "brightness".to_owned(),
            parameters: std::collections::BTreeMap::from([(
                "percent".to_owned(),
                ParamValue::Integer(-10),
            )]),
            keyframes: std::collections::BTreeMap::from([(
                "percent".to_owned(),
                AutomationCurve {
                    keyframes: vec![
                        Keyframe {
                            at: TimeCode::ZERO,
                            value: 0,
                            interpolation: KeyframeInterpolation::Linear,
                            tangent_in: 0,
                            tangent_out: 0,
                        },
                        Keyframe {
                            at: TimeCode(9),
                            value: 90,
                            interpolation: KeyframeInterpolation::Linear,
                            tangent_in: 0,
                            tangent_out: 0,
                        },
                    ],
                },
            )]),
        });
        document.validate().unwrap();

        let layers = visual_layers_at(&document, TimeCode(3)).unwrap();
        let TimelineVisualLayer::Video(layer) = &layers[0] else {
            panic!("media clip must resolve to a video layer");
        };
        assert_eq!(
            layer.effects[0].parameters.get("percent"),
            Some(&ParamValue::Integer(30))
        );
        assert!(layer.effects[0].keyframes.is_empty());
    }

    #[test]
    fn selects_sources_across_clip_boundaries_and_gap() {
        let document = fixture();
        let cases = [
            (TimeCode(0), Some((ClipId(1), AssetId(1), TimeCode(10)))),
            (TimeCode(9), Some((ClipId(1), AssetId(1), TimeCode(19)))),
            (TimeCode(10), None),
            (TimeCode(14), None),
            (TimeCode(15), Some((ClipId(2), AssetId(2), TimeCode(30)))),
            (TimeCode(24), Some((ClipId(2), AssetId(2), TimeCode(39)))),
            (TimeCode(25), None),
        ];

        for (position, expected) in cases {
            let actual = timeline_source_at(&document, position).unwrap();
            assert_eq!(
                actual.map(|source| (source.clip, source.asset, source.source_at)),
                expected,
                "wrong mapping at project frame {position}"
            );
        }
    }

    #[test]
    fn mixed_rates_use_integer_floor_mapping_inside_the_clip() {
        let mut document = fixture();
        document.fps = Rational::new(30, 1).unwrap();
        document.media_pool[0].fps = Rational::new(24, 1).unwrap();
        document.tracks[0].clips[0].source_range = TimeCode(10)..TimeCode(34);
        document.tracks[0].clips[1].timeline_start = TimeCode(35);
        document.duration = TimeCode(45);

        assert_eq!(
            timeline_source_at(&document, TimeCode(5))
                .unwrap()
                .unwrap()
                .source_at,
            TimeCode(14)
        );
    }

    #[test]
    fn overlapping_video_tracks_keep_document_bottom_to_top_order() {
        let mut document = fixture();
        document.tracks.push(Track {
            id: TrackId(8),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![Clip {
                enabled: true,
                enabled_curve: None,
                id: ClipId(3),
                asset: AssetId(2),
                source_range: TimeCode(0)..TimeCode(10),
                content: ClipContent::Media,
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
                audio_gain_curve: None,
                blend_mode: BlendMode::Normal,
            }],
        });

        let layers = video_layers_at(&document, TimeCode::ZERO).unwrap();
        assert_eq!(
            layers
                .iter()
                .map(|layer| layer.source.track)
                .collect::<Vec<_>>(),
            [TrackId(7), TrackId(8)]
        );
    }

    #[test]
    // These transition endpoints and midpoint are exact binary fractions.
    #[allow(clippy::float_cmp)]
    fn media_crossfade_ramps_alpha_across_integer_frames() {
        let mut document = fixture();
        document.tracks[0].clips[0].transition_in = Some(Transition {
            name: "crossfade".to_owned(),
            duration: TimeCode(3),
        });

        let alphas = [0, 1, 2].map(|frame| {
            video_layers_at(&document, TimeCode(frame)).unwrap()[0]
                .transition
                .alpha
        });
        assert_eq!(alphas, [0.0, 0.5, 1.0]);
    }

    #[test]
    // These transition endpoints and midpoint are exact binary fractions.
    #[allow(clippy::float_cmp)]
    fn media_color_fades_ramp_mix_down_and_keep_layer_alpha_opaque() {
        for (name, fade_white) in [("fade_from_black", 0.0), ("fade_from_white", 1.0)] {
            let mut document = fixture();
            document.tracks[0].clips[0].transition_in = Some(Transition {
                name: name.to_owned(),
                duration: TimeCode(3),
            });

            let shading = [0, 1, 2]
                .map(|frame| video_layers_at(&document, TimeCode(frame)).unwrap()[0].transition);
            assert_eq!(shading.map(|value| value.alpha), [1.0, 1.0, 1.0]);
            assert_eq!(shading.map(|value| value.fade_mix), [1.0, 0.5, 0.0]);
            assert_eq!(shading.map(|value| value.fade_white), [fade_white; 3]);
        }
    }

    #[test]
    fn one_frame_transition_is_a_fully_visible_no_op() {
        for name in ["crossfade", "fade_from_black", "fade_from_white"] {
            let mut document = fixture();
            document.tracks[0].clips[0].transition_in = Some(Transition {
                name: name.to_owned(),
                duration: TimeCode(1),
            });

            assert_eq!(
                video_layers_at(&document, TimeCode::ZERO).unwrap()[0].transition,
                TransitionRenderParams::default()
            );
        }
    }

    #[test]
    fn title_layers_keep_track_order_and_map_integer_fades_to_layer_alpha() {
        let mut document = fixture();
        document.tracks.push(Track {
            id: TrackId(8),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![Clip {
                enabled: true,
                enabled_curve: None,
                id: ClipId(3),
                asset: AssetId::default(),
                source_range: TimeCode(0)..TimeCode(10),
                content: ClipContent::Title(Title {
                    text: "Overlay".to_owned(),
                    fade_in_frames: TimeCode(3),
                    fade_out_frames: TimeCode(3),
                    ..Title::default()
                }),
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
                audio_gain_curve: None,
                blend_mode: BlendMode::Normal,
            }],
        });
        document.validate().unwrap();

        let start = visual_layers_at(&document, TimeCode(0)).unwrap();
        assert!(matches!(start[0], TimelineVisualLayer::Video(_)));
        let TimelineVisualLayer::Title(title) = &start[1] else {
            panic!("top track must resolve to a title layer");
        };
        assert_eq!(title.track, TrackId(8));
        assert!(title.transition.alpha.abs() < f32::EPSILON);

        let middle = visual_layers_at(&document, TimeCode(5)).unwrap();
        let TimelineVisualLayer::Title(title) = &middle[1] else {
            panic!("top track must resolve to a title layer");
        };
        assert!((title.transition.alpha - 1.0).abs() < f32::EPSILON);

        let end = visual_layers_at(&document, TimeCode(9)).unwrap();
        let TimelineVisualLayer::Title(title) = &end[1] else {
            panic!("top track must resolve to a title layer");
        };
        assert!(title.transition.alpha.abs() < f32::EPSILON);
    }

    #[test]
    fn freeze_visual_layer_holds_one_source_window_and_carries_shading() {
        let mut document = fixture();
        document.tracks = vec![Track {
            id: TrackId(8),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![Clip {
                enabled: true,
                enabled_curve: None,
                id: ClipId(3),
                asset: AssetId(1),
                source_range: TimeCode(0)..TimeCode(10),
                content: ClipContent::Freeze(FreezeFrame {
                    source_frame: TimeCode(17),
                }),
                timeline_start: TimeCode::ZERO,
                effects: vec![Effect {
                    enabled: true,
                    enabled_curve: None,
                    id: EffectId(1),
                    name: "brightness".to_owned(),
                    parameters: std::collections::BTreeMap::from([(
                        "percent".to_owned(),
                        ParamValue::Integer(20),
                    )]),
                    keyframes: std::collections::BTreeMap::new(),
                }],
                transition_in: Some(Transition {
                    name: "crossfade".to_owned(),
                    duration: TimeCode(3),
                }),
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
                audio_gain_curve: None,
                blend_mode: BlendMode::Normal,
            }],
        }];
        document.duration = TimeCode(10);
        document.validate().unwrap();

        let layers = visual_layers_at(&document, TimeCode(1)).unwrap();
        let TimelineVisualLayer::Video(layer) = &layers[0] else {
            panic!("freeze must use the normal video render path");
        };
        assert_eq!(layer.source.track, TrackId(8));
        assert_eq!(layer.source.clip, ClipId(3));
        assert_eq!(layer.source.asset, AssetId(1));
        assert_eq!(layer.source.source_at, TimeCode(17));
        assert_eq!(layer.source.source_end, TimeCode(18));
        assert_eq!(layer.source.timeline_end, TimeCode(10));
        assert_eq!(layer.effects[0].name, "brightness");
        assert!((layer.transition.alpha - 0.5).abs() < f32::EPSILON);
        assert!(
            timeline_source_at(&document, TimeCode(1))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn timeline_audio_segments_exclude_freeze_clips() {
        let mut document = fixture();
        document.tracks = vec![Track {
            id: TrackId(7),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![Clip {
                enabled: true,
                enabled_curve: None,
                id: ClipId(3),
                asset: AssetId(1),
                source_range: TimeCode(0)..TimeCode(10),
                content: ClipContent::Freeze(FreezeFrame {
                    source_frame: TimeCode(12),
                }),
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
                audio_gain_curve: None,
                blend_mode: BlendMode::Normal,
            }],
        }];
        document.duration = TimeCode(10);
        document.validate().unwrap();
        assert!(
            timeline_audio_segments(&document, TimeCode(0)..TimeCode(10))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn enumerates_audio_from_every_track_and_maps_the_requested_portions() {
        let mut document = fixture();
        document.media_pool.extend([
            MediaAsset {
                id: AssetId(3),
                path: PathBuf::from("bed.wav"),
                name: "bed".to_owned(),
                duration: TimeCode(30),
                fps: Rational::new(30, 1).unwrap(),
                kind: MediaKind::Audio,
                resolution: None,
                source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
                color_description: kinewright_core::ColorDescription::default(),
                assumed_from: None,
            },
            MediaAsset {
                id: AssetId(4),
                path: PathBuf::from("silent.mp4"),
                name: "silent".to_owned(),
                duration: TimeCode(30),
                fps: Rational::new(30, 1).unwrap(),
                kind: MediaKind::Video,
                resolution: Some((320, 180)),
                source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
                color_description: kinewright_core::ColorDescription::default(),
                assumed_from: None,
            },
        ]);
        document.tracks.extend([
            Track {
                id: TrackId(8),
                kind: TrackKind::Audio,
                sync_lock: true,
                clips: vec![Clip {
                    enabled: true,
                    enabled_curve: None,
                    id: ClipId(3),
                    asset: AssetId(3),
                    source_range: TimeCode(4)..TimeCode(14),
                    content: ClipContent::Media,
                    timeline_start: TimeCode(8),
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                    audio_gain_curve: None,
                    blend_mode: BlendMode::Normal,
                }],
            },
            Track {
                id: TrackId(9),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    enabled: true,
                    enabled_curve: None,
                    id: ClipId(4),
                    asset: AssetId(4),
                    source_range: TimeCode(0)..TimeCode(20),
                    content: ClipContent::Media,
                    timeline_start: TimeCode::ZERO,
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                    audio_gain_curve: None,
                    blend_mode: BlendMode::Normal,
                }],
            },
        ]);

        let segments = timeline_audio_segments(&document, TimeCode(5)..TimeCode(18)).unwrap();

        assert_eq!(
            segments,
            vec![
                TimelineAudioSegment {
                    track: TrackId(7),
                    clip: ClipId(1),
                    asset: AssetId(1),
                    project: TimeCode(5)..TimeCode(10),
                    source: TimeCode(15)..TimeCode(20),
                },
                TimelineAudioSegment {
                    track: TrackId(7),
                    clip: ClipId(2),
                    asset: AssetId(2),
                    project: TimeCode(15)..TimeCode(18),
                    source: TimeCode(30)..TimeCode(33),
                },
                TimelineAudioSegment {
                    track: TrackId(8),
                    clip: ClipId(3),
                    asset: AssetId(3),
                    project: TimeCode(8)..TimeCode(18),
                    source: TimeCode(4)..TimeCode(14),
                },
            ]
        );
    }

    #[test]
    fn audio_segment_mapping_uses_floor_start_and_ceil_end_at_mixed_rates() {
        let mut document = fixture();
        document.tracks.truncate(1);
        document.media_pool.truncate(1);
        document.media_pool[0].fps = Rational::new(24, 1).unwrap();
        document.tracks[0].clips.truncate(1);
        document.tracks[0].clips[0].source_range = TimeCode(10)..TimeCode(34);
        document.duration = TimeCode(30);

        let segments = timeline_audio_segments(&document, TimeCode(5)..TimeCode(20)).unwrap();

        assert_eq!(segments[0].project, TimeCode(5)..TimeCode(20));
        assert_eq!(segments[0].source, TimeCode(14)..TimeCode(26));
    }

    #[test]
    fn audio_segment_range_must_be_forward_and_non_negative() {
        let document = fixture();
        assert!(timeline_audio_segments(&document, TimeCode(-1)..TimeCode(1)).is_err());
        assert!(timeline_audio_segments(&document, TimeCode(4)..TimeCode(4)).is_err());
        assert!(
            timeline_audio_segments(&document, TimeCode(30)..TimeCode(40))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn speeded_clip_maps_project_offsets_through_effective_fps() {
        let mut document = fixture();
        document.tracks[0].clips[0].speed_percent = 200;

        let layers = video_layers_at(&document, TimeCode(2)).unwrap();
        assert_eq!(layers.len(), 1);
        let source = &layers[0].source;
        assert_eq!(source.source_at, TimeCode(14));
        assert_eq!(source.timeline_end, TimeCode(5));

        document.tracks[0].clips[0].speed_percent = 50;
        // Effective 15 fps: 20 project frames, one source frame per two.
        let layers = video_layers_at(&document, TimeCode(6)).unwrap();
        let source = &layers[0].source;
        assert_eq!(source.source_at, TimeCode(13));
        assert_eq!(source.timeline_end, TimeCode(20));
    }

    #[test]
    fn speeded_clips_are_muted_in_audio_segments() {
        let mut document = fixture();
        document.tracks[0].clips[0].speed_percent = 200;
        document.duration = TimeCode(40);

        let segments = timeline_audio_segments(&document, TimeCode(0)..TimeCode(40)).unwrap();
        assert!(
            segments.iter().all(|segment| segment.clip != ClipId(1)),
            "speed-changed clip must not contribute audio"
        );
        assert!(
            segments.iter().any(|segment| segment.clip == ClipId(2)),
            "real-time clip must still contribute audio"
        );
    }

    fn brightness_effect(id: u64, enabled: bool) -> Effect {
        Effect {
            enabled,
            enabled_curve: None,
            id: EffectId(id),
            name: "brightness".to_owned(),
            parameters: std::collections::BTreeMap::from([(
                "percent".to_owned(),
                ParamValue::Integer(25),
            )]),
            keyframes: std::collections::BTreeMap::new(),
        }
    }

    fn hold_curve(keys: &[(i64, i64)]) -> AutomationCurve {
        AutomationCurve {
            keyframes: keys
                .iter()
                .map(|(at, value)| Keyframe {
                    at: TimeCode(*at),
                    value: *value,
                    interpolation: KeyframeInterpolation::Hold,
                    tangent_in: 0,
                    tangent_out: 0,
                })
                .collect(),
        }
    }

    /// §10 gate 5, GPU-free half: toggling an effect off equals removing it
    /// in `visual_layers_at` (and `video_layers_at`, same `evaluated_effects`
    /// filter). The lavapipe half lives with the Part B goldens.
    #[test]
    fn disabled_effect_renders_through() {
        let mut document = fixture();
        document.tracks[0].clips[0]
            .effects
            .push(brightness_effect(1, true));
        document.validate().unwrap();

        let mut disabled = document.clone();
        disabled.tracks[0].clips[0].effects[0].enabled = false;
        let mut removed = document.clone();
        removed.tracks[0].clips[0].effects.clear();

        for at in [0, 3, 9] {
            let off = visual_layers_at(&disabled, TimeCode(at)).unwrap();
            let gone = visual_layers_at(&removed, TimeCode(at)).unwrap();
            assert_eq!(off, gone, "disabled must equal removed at frame {at}");
            assert!(
                off.iter().all(|layer| layer.effects().is_empty()),
                "no resolved layer may carry the disabled effect at frame {at}"
            );
            assert_eq!(
                video_layers_at(&disabled, TimeCode(at)).unwrap(),
                video_layers_at(&removed, TimeCode(at)).unwrap(),
                "video_layers_at must agree at frame {at}"
            );
        }

        // Sanity: the enabled original still resolves the effect.
        let on = visual_layers_at(&document, TimeCode(3)).unwrap();
        let TimelineVisualLayer::Video(layer) = &on[0] else {
            panic!("media clip must resolve to a video layer");
        };
        assert_eq!(layer.effects.len(), 1);
    }

    /// A keyframed `enabled` cuts at the key's first frame: the effect is
    /// present on every frame before the disabling key and absent from it on.
    #[test]
    fn keyframed_effect_enable_cuts_at_the_key_frame() {
        let mut document = fixture();
        let mut effect = brightness_effect(1, true);
        effect.enabled_curve = Some(hold_curve(&[(0, 1), (5, 0)]));
        document.tracks[0].clips[0].effects.push(effect);
        document.validate().unwrap();

        for at in 0..10 {
            let layers = visual_layers_at(&document, TimeCode(at)).unwrap();
            let TimelineVisualLayer::Video(layer) = &layers[0] else {
                panic!("media clip must resolve to a video layer at frame {at}");
            };
            assert_eq!(
                layer.effects.len(),
                usize::from(at < 5),
                "the effect must cut exactly at frame 5 (probed {at})"
            );
        }
    }

    /// §10 gate 6, GPU-free half: disabling a clip removes it from every
    /// layer resolution and silences it in the segments — byte-identical to
    /// the clip removed. The lavapipe half lives with the Part B goldens.
    #[test]
    fn disabled_clip_renders_through() {
        let mut document = fixture();
        document.tracks[0].clips[0]
            .effects
            .push(brightness_effect(1, true));
        document.validate().unwrap();

        let mut disabled = document.clone();
        disabled.tracks[0].clips[0].enabled = false;
        let mut removed = document.clone();
        removed.tracks[0].clips.remove(0);

        for at in [0, 3, 9, 15, 20] {
            assert_eq!(
                visual_layers_at(&disabled, TimeCode(at)).unwrap(),
                visual_layers_at(&removed, TimeCode(at)).unwrap(),
                "visual layers must match removal at frame {at}"
            );
            assert_eq!(
                video_layers_at(&disabled, TimeCode(at)).unwrap(),
                video_layers_at(&removed, TimeCode(at)).unwrap(),
                "video layers must match removal at frame {at}"
            );
            assert_eq!(
                timeline_source_at(&disabled, TimeCode(at)).unwrap(),
                timeline_source_at(&removed, TimeCode(at)).unwrap(),
                "source lookup must match removal at frame {at}"
            );
        }
        assert!(
            visual_layers_at(&disabled, TimeCode(3)).unwrap().is_empty(),
            "the disabled clip must leave a gap, not a layer"
        );
        // Clip 2 (frames 15+) still resolves through the disabled neighbour.
        assert_eq!(visual_layers_at(&disabled, TimeCode(15)).unwrap().len(), 1);

        let off_segments = timeline_audio_segments(&disabled, TimeCode(0)..TimeCode(25)).unwrap();
        let gone_segments = timeline_audio_segments(&removed, TimeCode(0)..TimeCode(25)).unwrap();
        assert_eq!(off_segments, gone_segments);
        assert!(
            off_segments.iter().all(|segment| segment.clip != ClipId(1)),
            "the disabled clip must contribute silence"
        );
        assert!(
            off_segments.iter().any(|segment| segment.clip == ClipId(2)),
            "the enabled clip must still contribute audio"
        );
    }

    /// A keyframed clip `enabled_curve` cuts layers at the key's first frame,
    /// in both directions.
    #[test]
    fn keyframed_clip_enable_cuts_layers_at_the_key_frame() {
        let mut document = fixture();
        document.tracks[0].clips[0].enabled_curve = Some(hold_curve(&[(0, 1), (5, 0)]));
        document.validate().unwrap();

        for at in 0..10 {
            let layers = visual_layers_at(&document, TimeCode(at)).unwrap();
            assert_eq!(
                layers.len(),
                usize::from(at < 5),
                "the clip must cut exactly at frame 5 (probed {at})"
            );
            assert_eq!(
                timeline_source_at(&document, TimeCode(at))
                    .unwrap()
                    .is_some(),
                at < 5,
                "source lookup must cut with the clip (probed {at})"
            );
        }

        document.tracks[0].clips[0].enabled_curve = Some(hold_curve(&[(0, 0), (5, 1)]));
        document.validate().unwrap();
        for at in 0..10 {
            assert_eq!(
                visual_layers_at(&document, TimeCode(at)).unwrap().len(),
                usize::from(at >= 5),
                "the clip must return exactly at frame 5 (probed {at})"
            );
        }
    }

    /// MO1 N4 G8 (R2 S3): the layer cut above, on a clip starting past
    /// zero — clip 2 at 15 with local keys [(0,1),(5,0)] cuts at project
    /// 20, not 5. Evaluating `is_enabled_at` at project time leaves no
    /// layers at 15..20 and the test reds.
    #[test]
    fn keyframed_clip_enable_cuts_offset_layers_at_the_local_key_frame() {
        let mut document = fixture();
        document.tracks[0].clips[1].enabled_curve = Some(hold_curve(&[(0, 1), (5, 0)]));
        document.validate().unwrap();

        for at in 15..25 {
            let layers = visual_layers_at(&document, TimeCode(at)).unwrap();
            assert_eq!(
                layers.len(),
                usize::from(at < 20),
                "offset clip must cut at project 20 (probed {at})"
            );
            assert_eq!(
                timeline_source_at(&document, TimeCode(at))
                    .unwrap()
                    .is_some(),
                at < 20,
                "offset source lookup must cut with the clip (probed {at})"
            );
        }

        document.tracks[0].clips[1].enabled_curve = Some(hold_curve(&[(0, 0), (5, 1)]));
        document.validate().unwrap();
        for at in 15..25 {
            assert_eq!(
                visual_layers_at(&document, TimeCode(at)).unwrap().len(),
                usize::from(at >= 20),
                "offset clip must return at project 20 (probed {at})"
            );
        }
    }

    /// MO1 N4 G8 (R2 S2): the audio run-split above, on a clip starting
    /// past zero — clip 2 at 15 with local keys [(0,1),(4,0),(7,1)]
    /// splits at project (15,19) and (22,25). Runs evaluated in project
    /// time never split (or split at the wrong frames) and the test reds.
    #[test]
    fn keyframed_clip_enable_splits_offset_audio_into_enabled_runs() {
        let mut document = fixture();
        document.tracks[0].clips[1].enabled_curve = Some(hold_curve(&[(0, 1), (4, 0), (7, 1)]));
        document.validate().unwrap();

        let segments = timeline_audio_segments(&document, TimeCode(15)..TimeCode(25)).unwrap();
        let clip: Vec<_> = segments
            .iter()
            .filter(|segment| segment.clip == ClipId(2))
            .collect();
        assert_eq!(
            clip.iter()
                .map(|segment| (segment.project.start.0, segment.project.end.0))
                .collect::<Vec<_>>(),
            vec![(15, 19), (22, 25)],
            "local Hold 1->0->1 on a start-15 clip must cut at project 19 and 22"
        );
        // Same-rate mapping: source runs track the project runs off source 30.
        assert_eq!(
            clip.iter()
                .map(|segment| (segment.source.start.0, segment.source.end.0))
                .collect::<Vec<_>>(),
            vec![(30, 34), (37, 40)]
        );

        for at in 15..25 {
            let local = TimeCode(at - 15);
            let covered = segments.iter().any(|segment| {
                segment.clip == ClipId(2)
                    && segment.project.start.0 <= at
                    && at < segment.project.end.0
            });
            assert_eq!(
                covered,
                document.tracks[0].clips[1].is_enabled_at(local),
                "offset segment coverage must equal is_enabled_at(local) at frame {at}"
            );
        }
    }

    /// A keyframed clip `enabled_curve` splits audio into maximal enabled
    /// runs; every covered project frame is enabled at its clip-local frame
    /// and every uncovered one is not.
    #[test]
    fn keyframed_clip_enable_splits_audio_into_enabled_runs() {
        let mut document = fixture();
        document.tracks[0].clips[0].enabled_curve = Some(hold_curve(&[(0, 1), (4, 0), (7, 1)]));
        document.validate().unwrap();

        let segments = timeline_audio_segments(&document, TimeCode(0)..TimeCode(10)).unwrap();
        let clip: Vec<_> = segments
            .iter()
            .filter(|segment| segment.clip == ClipId(1))
            .collect();
        assert_eq!(
            clip.iter()
                .map(|segment| (segment.project.start.0, segment.project.end.0))
                .collect::<Vec<_>>(),
            vec![(0, 4), (7, 10)],
            "Hold 1→0→1 must cut exactly at frames 4 and 7"
        );
        // Same-rate mapping: source runs track the project runs off source 10.
        assert_eq!(
            clip.iter()
                .map(|segment| (segment.source.start.0, segment.source.end.0))
                .collect::<Vec<_>>(),
            vec![(10, 14), (17, 20)]
        );

        // Frame-by-frame equivalence with `is_enabled_at`, whatever the
        // interpolation between the keys resolves to.
        for at in 0..10 {
            let local = TimeCode(at);
            let covered = segments.iter().any(|segment| {
                segment.clip == ClipId(1)
                    && segment.project.start.0 <= at
                    && at < segment.project.end.0
            });
            assert_eq!(
                covered,
                document.tracks[0].clips[0].is_enabled_at(local),
                "segment coverage must equal is_enabled_at at frame {at}"
            );
        }
    }

    /// An always-enabled curve resolves the identical single segment the
    /// curve-free path always produced.
    #[test]
    fn always_enabled_curve_matches_the_curve_free_segment() {
        let plain = fixture();
        let expected = timeline_audio_segments(&plain, TimeCode(0)..TimeCode(25)).unwrap();

        let mut document = fixture();
        document.tracks[0].clips[0].enabled_curve = Some(hold_curve(&[(0, 1)]));
        document.tracks[0].clips[1].enabled_curve = Some(hold_curve(&[(0, 1)]));
        document.validate().unwrap();
        let actual = timeline_audio_segments(&document, TimeCode(0)..TimeCode(25)).unwrap();
        assert_eq!(actual, expected);
    }

    /// MO2 R1–R4, R20, R21 (Part B1 replaces Part A's fail-closed seam):
    /// every MO2 layer resolves in both resolvers with its blend, its role,
    /// and the R21 geometry — entering/backdrop offsets and coverage at the
    /// exact midpoint of an odd duration, an invisible start, and the
    /// ordinary layer from `offset = d − 1` on.
    #[test]
    #[allow(clippy::float_cmp, clippy::too_many_lines)]
    fn mo2_layers_resolve_with_blend_role_and_geometry() {
        for mode in BlendMode::ALL {
            let mut document = fixture();
            document.tracks[0].clips[0].blend_mode = mode;
            let TimelineVisualLayer::Video(layer) =
                &visual_layers_at(&document, TimeCode(0)).unwrap()[0]
            else {
                panic!("a media clip resolves as video");
            };
            assert_eq!(layer.blend_mode, mode);
            assert_eq!(
                video_layers_at(&document, TimeCode(0)).unwrap()[0].blend_mode,
                mode
            );
        }
        // R21's table, in screen fractions (positive right/down).
        let directions = [
            ("left", [-1.0_f32, 0.0], true),
            ("right", [1.0, 0.0], false),
            ("up", [0.0, -1.0], true),
            ("down", [0.0, 1.0], false),
        ];
        for descriptor in &kinewright_core::TRANSITION_DESCRIPTORS[3..] {
            let mut document = fixture();
            document.tracks[0].clips[0].transition_in = Some(Transition {
                name: descriptor.name.to_owned(),
                duration: TimeCode(5),
            });
            let (_, direction, below) = directions
                .iter()
                .find(|(suffix, _, _)| descriptor.name.ends_with(suffix))
                .copied()
                .unwrap();
            let kind = descriptor.name.split('_').next().unwrap();
            let at = |document: &Document, frame: i64| {
                video_layers_at(document, TimeCode(frame)).unwrap()[0].transition
            };
            for (frame, progress) in [(0_i64, 0.0_f32), (2, 0.5)] {
                let params = at(&document, frame);
                let coverage = params.coverage.expect("geometric coverage is active");
                assert_eq!(coverage.below_edge, below, "{}", descriptor.name);
                assert_eq!(
                    coverage.edge,
                    if below { progress } else { 1.0 - progress },
                    "{}",
                    descriptor.name
                );
                let entering = direction.map(|value| value * (1.0 - progress));
                assert_eq!(
                    params.offset,
                    if kind == "wipe" { [0.0; 2] } else { entering },
                    "{} at {frame}",
                    descriptor.name
                );
                let backdrop = direction.map(|value| -value * progress);
                assert_eq!(
                    params.backdrop,
                    (kind == "push").then_some(backdrop),
                    "{} at {frame}",
                    descriptor.name
                );
                assert_eq!(params.alpha, 1.0);
            }
            for frame in [4, 5] {
                let ordinary = at(&document, frame);
                assert_eq!(
                    ordinary,
                    TransitionRenderParams::default(),
                    "{}",
                    descriptor.name
                );
            }
            document.tracks[0].clips[0]
                .transition_in
                .as_mut()
                .unwrap()
                .duration = TimeCode(1);
            let identity = at(&document, 0);
            assert_eq!(
                identity,
                TransitionRenderParams::default(),
                "duration 1 is an identity"
            );
        }
        for (content, solid) in [
            (ClipContent::Adjustment, false),
            (
                ClipContent::Solid(kinewright_core::SolidColor { r: 1, g: 2, b: 3 }),
                true,
            ),
        ] {
            let mut document = fixture();
            let clip = &mut document.tracks[0].clips[0];
            clip.content = content;
            clip.source_range = TimeCode(0)..TimeCode(10);
            clip.blend_mode = BlendMode::Screen;
            document.validate().unwrap();
            let layers = visual_layers_at(&document, TimeCode(0)).unwrap();
            match (&layers[0], solid) {
                (TimelineVisualLayer::Solid(layer), true) => {
                    assert_eq!(
                        layer.color,
                        kinewright_core::SolidColor { r: 1, g: 2, b: 3 }
                    );
                    assert_eq!(layer.blend_mode, BlendMode::Screen);
                }
                (TimelineVisualLayer::Adjustment(layer), false) => {
                    assert_eq!(layer.clip, ClipId(1));
                    assert_eq!(layer.blend_mode, BlendMode::Screen);
                }
                (other, _) => panic!("unexpected layer {other:?}"),
            }
            // `video_layers_at` resolves media only.
            assert!(video_layers_at(&document, TimeCode(0)).unwrap().is_empty());
        }
    }

    /// MO2 R1/R2/R3: blend is inert on audio — segments are identical
    /// whatever the mode — and the generated kinds contribute no audio.
    #[test]
    fn mo2_blend_is_inert_on_audio_and_generated_kinds_are_silent() {
        let expected = timeline_audio_segments(&fixture(), TimeCode(0)..TimeCode(25)).unwrap();
        for mode in BlendMode::ALL {
            let mut document = fixture();
            document.tracks[0].clips[0].blend_mode = mode;
            document.tracks[0].clips[1].blend_mode = mode;
            let actual = timeline_audio_segments(&document, TimeCode(0)..TimeCode(25)).unwrap();
            assert_eq!(actual, expected, "{mode:?}");
        }
        let mut document = fixture();
        let clip = &mut document.tracks[0].clips[0];
        clip.content = ClipContent::Adjustment;
        clip.source_range = TimeCode(0)..TimeCode(10);
        document.validate().unwrap();
        let segments = timeline_audio_segments(&document, TimeCode(0)..TimeCode(25)).unwrap();
        assert!(segments.iter().all(|segment| segment.clip != ClipId(1)));
    }
}
