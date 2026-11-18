use kinewright_core::{
    ClipContent, Document, ImportRecord, LossCode, MediaKind, TimeCode, TrackKind,
    clip_effective_fps, map_frames,
};
use kinewright_interchange::{
    Time,
    ir::{self, ContainerTime, Item, MediaId, Playback, Stack, Timeline},
};

enum Picture {
    Media {
        id: MediaId,
        time: Time,
        in_point: Time,
    },
    Title,
}

fn stack_picture(stack: &Stack, map: ContainerTime, at: Time) -> Option<Picture> {
    stack
        .tracks
        .iter()
        .filter_map(|track| track_picture(track, map, at))
        .next_back()
}

fn track_picture(track: &ir::Track, map: ContainerTime, at: Time) -> Option<Picture> {
    let mut anchors: Vec<_> = track
        .items
        .iter()
        .flat_map(|placed| &placed.anchors)
        .collect();
    anchors.sort_by_key(|anchor| anchor.lane);
    let below = anchors
        .iter()
        .filter(|anchor| anchor.lane < 0)
        .filter_map(|anchor| track_picture(&anchor.track, map.compose(anchor.time).unwrap(), at))
        .next_back();
    let mut by_time: Vec<_> = track.items.iter().collect();
    by_time.sort_by_key(|placed| placed.at);
    let body = by_time
        .into_iter()
        .filter_map(|placed| {
            let start = map.to_parent(placed.at).unwrap();
            let end = map
                .to_parent(placed.at.checked_add(placed.item.span()).unwrap())
                .unwrap();
            if !(start..end).contains(&at) {
                return None;
            }
            match &placed.item {
                Item::Clip(clip) if track.kind == TrackKind::Video && clip.enabled => {
                    let sampled = if let Playback::Freeze(held) = clip.time {
                        held
                    } else {
                        let local = at
                            .checked_sub(start)
                            .unwrap()
                            .checked_mul(map.speed)
                            .unwrap();
                        let rate = clip.time.average().unwrap();
                        clip.source_start
                            .checked_add(local.checked_mul(rate).unwrap())
                            .unwrap()
                    };
                    Some(Picture::Media {
                        id: clip.media,
                        time: sampled,
                        in_point: clip.source_start,
                    })
                }
                Item::Title { enabled: true, .. } if track.kind == TrackKind::Video => {
                    Some(Picture::Title)
                }
                Item::Nested { stack, .. } if stack.time.speed > Time::ZERO => {
                    let offset = start
                        .checked_add(stack.time.offset.checked_div(map.speed).unwrap())
                        .unwrap();
                    let inner = ContainerTime {
                        offset,
                        in_point: stack.time.in_point,
                        speed: map.speed.checked_mul(stack.time.speed).unwrap(),
                    };
                    stack_picture(stack, inner, at)
                }
                _ => None,
            }
        })
        .next_back();
    let above = anchors
        .iter()
        .filter(|anchor| anchor.lane >= 0)
        .filter_map(|anchor| track_picture(&anchor.track, map.compose(anchor.time).unwrap(), at))
        .next_back();
    above.or(body).or(below)
}

pub fn check(
    timeline: &Timeline,
    document: &Document,
    record: &ImportRecord,
    fixture: &str,
) -> usize {
    let changed = [
        LossCode::CutSnapped,
        LossCode::ClipTooShort,
        LossCode::TransitionLane,
        LossCode::TransitionHandlesShort,
        LossCode::TransitionFadeClamped,
        LossCode::TransitionTypeMapped,
        LossCode::ReverseDropped,
        LossCode::SpeedRounded,
        LossCode::SpeedClamped,
        LossCode::VariableSpeedAveraged,
    ];
    // These fixtures check the approximated timing and alpha in their golden
    // clips and independent cut tests. The opaque oracle uses the native order.
    if changed.iter().any(|code| {
        record.rows.iter().any(|row| row.code == *code) || record.omitted_codes.contains(code)
    }) || document
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .any(|clip| clip.transition_in.is_some() || !clip.effects.is_empty())
    {
        return 0;
    }
    let mut checked = 0;
    for frame in 0..document.duration.0 {
        let at = Time::from_frame(TimeCode(frame), document.fps).unwrap();
        let expected = stack_picture(&timeline.root, timeline.root.time, at);
        let actual = document
            .tracks
            .iter()
            .rev()
            .filter(|track| track.kind == TrackKind::Video)
            .flat_map(|track| &track.clips)
            .find(|clip| {
                clip.enabled
                    && clip.timeline_start.0 <= frame
                    && frame < clip.timeline_start.0 + document.clip_duration(clip).unwrap().0
            });
        match (expected, actual) {
            (None, None) => {}
            (Some(Picture::Title), Some(clip)) => assert!(
                matches!(clip.content, ClipContent::Title(_)),
                "{fixture} frame {frame}"
            ),
            (Some(Picture::Media { id, time, in_point }), Some(clip)) => {
                let source = &timeline.media[id.0];
                let asset = document.asset(clip.asset).unwrap();
                assert_eq!(
                    asset.path.to_str(),
                    Some(source.locator.as_str()),
                    "{fixture} frame {frame}"
                );
                let source_frame = if let ClipContent::Freeze(freeze) = &clip.content {
                    freeze.source_frame
                } else {
                    let effective = clip_effective_fps(asset.fps, clip).unwrap();
                    let local = TimeCode(frame - clip.timeline_start.0);
                    clip.source_range
                        .start
                        .checked_add(map_frames(local, document.fps, effective).unwrap())
                        .unwrap()
                };
                let represented = Time::from_frame(source_frame, asset.fps).unwrap();
                let time = if source.kind == MediaKind::Image {
                    in_point
                } else {
                    time
                };
                let expected = time
                    .checked_sub(source.start.unwrap_or(Time::ZERO))
                    .unwrap();
                let distance = represented.checked_sub(expected).unwrap().abs().unwrap();
                let half_frame = Time::new(
                    i64::from(asset.fps.denominator()),
                    u64::from(asset.fps.numerator()) * 2,
                )
                .unwrap();
                assert!(
                    distance <= half_frame,
                    "{fixture} frame {frame} source {source_frame:?} expected {expected:?}"
                );
            }
            _ => panic!("{fixture} frame {frame}: opaque image presence differs"),
        }
        checked += 1;
    }
    checked
}
