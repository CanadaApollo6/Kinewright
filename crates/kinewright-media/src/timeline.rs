use std::ops::Range;

use kinewright_core::{
    AssetId, Clip, ClipContent, ClipId, Document, Effect, FrameRounding, MediaError, MediaKind,
    TimeCode, Title, Track, TrackId, TrackKind, TransitionShading, map_frames_with_rounding,
    map_source_range_to_project, transition_descriptor,
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
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimelineTitleLayer {
    pub track: TrackId,
    pub clip: ClipId,
    pub title: Title,
    pub effects: Vec<Effect>,
    pub transition: TransitionRenderParams,
}

/// Per-layer transition shading evaluated for one project frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransitionRenderParams {
    pub alpha: f32,
    pub fade_mix: f32,
    pub fade_white: f32,
}

impl Default for TransitionRenderParams {
    fn default() -> Self {
        Self {
            alpha: 1.0,
            fade_mix: 0.0,
            fade_white: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TimelineVisualLayer {
    Video(TimelineVideoLayer),
    Title(TimelineTitleLayer),
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
        match &clip.content {
            ClipContent::Media => {
                let source = media_source_for_clip(document, track.id, clip, project_at)?;
                layers.push(TimelineVisualLayer::Video(TimelineVideoLayer {
                    source,
                    effects: evaluated_effects(clip, project_at),
                    transition: transition_render_params(clip, project_at),
                }));
            }
            ClipContent::Title(title) => {
                let mut transition = transition_render_params(clip, project_at);
                transition.alpha *= title_alpha(document, clip, title, project_at)?;
                layers.push(TimelineVisualLayer::Title(TimelineTitleLayer {
                    track: track.id,
                    clip: clip.id,
                    title: title.clone(),
                    effects: evaluated_effects(clip, project_at),
                    transition,
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
                    effects: evaluated_effects(clip, project_at),
                    transition: transition_render_params(clip, project_at),
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
            // Speed-changed clips are muted in v1: varispeed shifts pitch and
            // pitch-preserving stretch is deferred, and silence is the honest
            // middle ground. Remaining clips are all real time, so the asset
            // rate below is already the effective rate.
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
            let source_start_offset = map_frames_with_rounding(
                start_offset,
                document.fps,
                asset.fps,
                FrameRounding::Floor,
            )
            .map_err(|error| MediaError::Backend(error.to_string()))?;
            let source_end_offset =
                map_frames_with_rounding(end_offset, document.fps, asset.fps, FrameRounding::Ceil)
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
                project: project_start..project_end,
                source: TimeCode(source_start.0.min(clip.source_range.end.0))
                    ..TimeCode(source_end.0.min(clip.source_range.end.0)),
            });
        }
    }
    Ok(segments)
}

fn source_on_track(
    document: &Document,
    track: &Track,
    project_at: TimeCode,
) -> Result<Option<TimelineSource>, MediaError> {
    let Some(clip) = active_clip_on_track(document, track, project_at)? else {
        return Ok(None);
    };
    // Freeze clips deliberately stay invisible here: this media-only lookup is
    // used by split-at-playhead and freeze creation to resolve moving footage.
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
        AssetId, AutomationCurve, Clip, ClipId, Document, Effect, EffectId, FreezeFrame, Keyframe,
        KeyframeInterpolation, MediaAsset, MediaKind, ParamValue, Rational, TimeCode, Track,
        TrackId, TrackKind, Transition,
    };

    use super::*;

    fn fixture() -> Document {
        Document {
            catalog: kinewright_core::MediaCatalog::default(),
            audio_mix: kinewright_core::AudioMix::default(),
            color_context: kinewright_core::ColorContext::default(),
            tracks: vec![Track {
                id: TrackId(7),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![
                    Clip {
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
                    },
                    Clip {
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
                    color_description: kinewright_core::ColorDescription::default(),
                },
                MediaAsset {
                    id: AssetId(2),
                    path: PathBuf::from("two.mp4"),
                    name: "two".to_owned(),
                    duration: TimeCode(60),
                    fps: Rational::new(30, 1).unwrap(),
                    kind: MediaKind::AudioVideo,
                    resolution: Some((320, 180)),
                    color_description: kinewright_core::ColorDescription::default(),
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
                        },
                        Keyframe {
                            at: TimeCode(9),
                            value: 90,
                            interpolation: KeyframeInterpolation::Linear,
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
                id: ClipId(3),
                asset: AssetId(1),
                source_range: TimeCode(0)..TimeCode(10),
                content: ClipContent::Freeze(FreezeFrame {
                    source_frame: TimeCode(17),
                }),
                timeline_start: TimeCode::ZERO,
                effects: vec![Effect {
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
                color_description: kinewright_core::ColorDescription::default(),
            },
            MediaAsset {
                id: AssetId(4),
                path: PathBuf::from("silent.mp4"),
                name: "silent".to_owned(),
                duration: TimeCode(30),
                fps: Rational::new(30, 1).unwrap(),
                kind: MediaKind::Video,
                resolution: Some((320, 180)),
                color_description: kinewright_core::ColorDescription::default(),
            },
        ]);
        document.tracks.extend([
            Track {
                id: TrackId(8),
                kind: TrackKind::Audio,
                sync_lock: true,
                clips: vec![Clip {
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
                }],
            },
            Track {
                id: TrackId(9),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
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

        // Source 10..20 at an effective 60 fps in a 30 fps project: the clip
        // now covers 5 project frames, consuming two source frames per one.
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
}
