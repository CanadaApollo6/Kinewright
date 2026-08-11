use std::ops::Range;

use openreel_core::{
    AssetId, Clip, ClipId, Document, Effect, FrameRounding, MediaError, MediaKind, TimeCode, Track,
    TrackId, TrackKind, map_frames_with_rounding, map_source_range_to_project,
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
    pub transition_alpha: f32,
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
                effects: clip.effects.clone(),
                transition_alpha: transition_alpha(clip, project_at),
            });
        }
    }
    Ok(layers)
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
            let asset = document.asset(clip.asset).ok_or_else(|| {
                MediaError::Backend(format!(
                    "timeline clip {} references missing asset {}",
                    clip.id, clip.asset
                ))
            })?;
            if !matches!(asset.kind, MediaKind::Audio | MediaKind::AudioVideo) {
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
    for clip in &track.clips {
        if project_at < clip.timeline_start {
            break;
        }
        let asset = document.asset(clip.asset).ok_or_else(|| {
            MediaError::Backend(format!(
                "timeline clip {} references missing asset {}",
                clip.id, clip.asset
            ))
        })?;
        let duration =
            map_source_range_to_project(clip.source_range.clone(), asset.fps, document.fps)
                .map_err(|error| MediaError::Backend(error.to_string()))?;
        let timeline_end = clip
            .timeline_start
            .checked_add(duration)
            .ok_or_else(|| MediaError::Backend("timeline position overflowed".to_owned()))?;
        if project_at >= timeline_end {
            continue;
        }
        let project_offset = project_at
            .checked_sub(clip.timeline_start)
            .ok_or_else(|| MediaError::Backend("timeline position underflowed".to_owned()))?;
        let source_offset = map_frames_with_rounding(
            project_offset,
            document.fps,
            asset.fps,
            FrameRounding::Floor,
        )
        .map_err(|error| MediaError::Backend(error.to_string()))?;
        let source_at = clip
            .source_range
            .start
            .checked_add(source_offset)
            .ok_or_else(|| MediaError::Backend("source position overflowed".to_owned()))?;
        return Ok(Some(TimelineSource {
            track: track.id,
            clip: clip.id,
            asset: clip.asset,
            source_at: TimeCode(source_at.0.min(clip.source_range.end.0.saturating_sub(1))),
            source_end: clip.source_range.end,
            timeline_end,
        }));
    }
    Ok(None)
}

// GPU alpha is f32; projecting integer frame offsets is the intended final conversion.
#[allow(clippy::cast_precision_loss)]
fn transition_alpha(clip: &Clip, project_at: TimeCode) -> f32 {
    let Some(transition) = &clip.transition_in else {
        return 1.0;
    };
    if transition.name != "crossfade" || transition.duration.0 <= 1 {
        return 1.0;
    }
    let offset = project_at.0.saturating_sub(clip.timeline_start.0);
    (offset as f32 / (transition.duration.0 - 1) as f32).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use openreel_core::{
        AssetId, Clip, ClipId, Document, MediaAsset, MediaKind, Rational, TimeCode, Track, TrackId,
        TrackKind,
    };

    use super::*;

    fn fixture() -> Document {
        Document {
            tracks: vec![Track {
                id: TrackId(7),
                kind: TrackKind::Video,
                clips: vec![
                    Clip {
                        id: ClipId(1),
                        asset: AssetId(1),
                        source_range: TimeCode(10)..TimeCode(20),
                        timeline_start: TimeCode(0),
                        effects: Vec::new(),
                        transition_in: None,
                        link: None,
                    },
                    Clip {
                        id: ClipId(2),
                        asset: AssetId(2),
                        source_range: TimeCode(30)..TimeCode(40),
                        timeline_start: TimeCode(15),
                        effects: Vec::new(),
                        transition_in: None,
                        link: None,
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
                },
                MediaAsset {
                    id: AssetId(2),
                    path: PathBuf::from("two.mp4"),
                    name: "two".to_owned(),
                    duration: TimeCode(60),
                    fps: Rational::new(30, 1).unwrap(),
                    kind: MediaKind::AudioVideo,
                    resolution: Some((320, 180)),
                },
            ],
            markers: Vec::new(),
            fps: Rational::new(30, 1).unwrap(),
            resolution: (320, 180),
            duration: TimeCode(25),
        }
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
            clips: vec![Clip {
                id: ClipId(3),
                asset: AssetId(2),
                source_range: TimeCode(0)..TimeCode(10),
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: None,
                link: None,
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
            },
            MediaAsset {
                id: AssetId(4),
                path: PathBuf::from("silent.mp4"),
                name: "silent".to_owned(),
                duration: TimeCode(30),
                fps: Rational::new(30, 1).unwrap(),
                kind: MediaKind::Video,
                resolution: Some((320, 180)),
            },
        ]);
        document.tracks.extend([
            Track {
                id: TrackId(8),
                kind: TrackKind::Audio,
                clips: vec![Clip {
                    id: ClipId(3),
                    asset: AssetId(3),
                    source_range: TimeCode(4)..TimeCode(14),
                    timeline_start: TimeCode(8),
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                }],
            },
            Track {
                id: TrackId(9),
                kind: TrackKind::Video,
                clips: vec![Clip {
                    id: ClipId(4),
                    asset: AssetId(4),
                    source_range: TimeCode(0)..TimeCode(20),
                    timeline_start: TimeCode::ZERO,
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
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
}
