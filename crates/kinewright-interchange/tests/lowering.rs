use kinewright_core::{
    InterchangeFormat, LossBound, LossCode, MediaKind, Rational, TimeCode, TrackKind,
};
use kinewright_interchange::{
    MediaTable, Time, fcpxml,
    ir::{
        self, Anchor, Clip, ContainerTime, Item, Media, MediaId, ParseOptions, Placed, Playback,
        Source, Stack, Timeline, Track,
    },
    lower, otio,
};

fn seconds(frames: i64, fps: u32) -> Time {
    Time::new(frames, u64::from(fps)).unwrap()
}

fn media(name: &str, fps: Rational) -> Media {
    Media {
        locator: format!("file:///Media/{name}.mov"),
        name: name.into(),
        kind: MediaKind::Video,
        fps: Some(fps),
        start: Some(Time::ZERO),
        duration: Some(Time::integer(300)),
        resolution: None,
    }
}

fn timeline(project_rate: u32, tracks: Vec<Track>, media: Vec<Media>) -> Timeline {
    Timeline {
        source: Source::new(InterchangeFormat::Otio, "Timeline.1", "grid.otio", b"grid"),
        name: "Grid".into(),
        start: Time::ZERO,
        rate_hint: Rational::new(project_rate, 1).unwrap(),
        resolution_hint: Some((1920, 1080)),
        root: Stack {
            tracks,
            ..Stack::default()
        },
        media,
        notes: Vec::new(),
    }
}

fn clip(site: &str, media: usize, at: Time, start: Time, span: Time, playback: Playback) -> Placed {
    let mut clip = Clip::new(MediaId(media), start, span);
    clip.time = playback;
    Placed::new(at, Item::Clip(clip), site)
}

#[test]
fn named_resolve_clip_stays_exact_through_lowering() {
    let bytes = include_bytes!("fixtures/named-resolve.otio");
    let timeline = otio::parse(bytes, "named-resolve.otio", ParseOptions::default()).unwrap();
    let (document, record) = lower(&timeline, &MediaTable::default()).unwrap();
    assert_eq!(
        (
            record.items,
            record.exact,
            record.approximated,
            record.dropped
        ),
        (1, 1, 0, 0)
    );
    assert_eq!(document.tracks[0].name.as_deref(), Some("V1"));
    assert_eq!(
        document.tracks[0].clips[0].source_range,
        TimeCode(0)..TimeCode(24)
    );
    assert_eq!(document.duration, TimeCode(24));
}

#[test]
fn windows_survive_lowering_without_unused_assets() {
    let bytes = include_bytes!("fixtures/track-window.otio");
    let timeline = otio::parse(bytes, "track-window.otio", ParseOptions::default()).unwrap();
    let (document, record) = lower(&timeline, &MediaTable::default()).unwrap();
    assert_eq!(document.media_pool.len(), 1);
    assert_eq!(document.media_pool[0].name, "Blue");
    assert_eq!((record.exact, record.dropped), (1, 1));
    assert_eq!(document.tracks[0].clips[0].timeline_start, TimeCode(0));

    let bytes = include_bytes!("fixtures/sync-window.fcpxml");
    let timeline = fcpxml::parse(bytes, "sync-window.fcpxml", ParseOptions::default()).unwrap();
    let (document, record) = lower(&timeline, &MediaTable::default()).unwrap();
    assert_eq!(document.tracks[0].clips.len(), 1);
    assert_eq!(document.tracks[0].clips[0].timeline_start, TimeCode(788));
    assert_eq!(record.dropped, 1);
}

#[test]
fn admissibility_beats_nearness_at_the_next_item_limit() {
    let mut video = Track::new(TrackKind::Video);
    video.items = vec![
        clip(
            "A",
            0,
            Time::ZERO,
            Time::ZERO,
            seconds(38, 60),
            Playback::Speed(Time::new(1, 10).unwrap()),
        ),
        clip(
            "B",
            1,
            seconds(38, 60),
            seconds(30, 60),
            seconds(1, 60),
            Playback::Normal,
        ),
    ];
    let timeline = timeline(
        60,
        vec![video],
        vec![
            media("A", Rational::default()),
            media("B", Rational::new(60, 1).unwrap()),
        ],
    );
    let (document, record) = lower(&timeline, &MediaTable::default()).unwrap();
    let [a, b] = document.tracks[0].clips.as_slice() else {
        panic!("expected a cut with two clips");
    };
    assert_eq!(clip_end(&document, a), TimeCode(20));
    assert_eq!(b.timeline_start, TimeCode(20));
    assert_eq!(b.source_range, TimeCode(12)..TimeCode(31));
    assert_eq!(document.duration, TimeCode(39));
    let snap = record
        .rows
        .iter()
        .find(|row| row.code == LossCode::CutSnapped)
        .unwrap();
    assert_eq!(snap.bound, Some(LossBound::Frames(18)));
}

#[test]
fn a_transition_can_consume_the_whole_body() {
    let fps = Rational::new(24, 1).unwrap();
    let mut video = Track::new(TrackKind::Video);
    video.items = vec![
        clip("A", 0, Time::ZERO, Time::ONE, Time::ONE, Playback::Normal),
        Placed::new(
            Time::ONE,
            Item::Transition {
                kind: "SMPTE_Dissolve".into(),
                before: seconds(2, 24),
                after: seconds(4, 24),
            },
            "transition",
        ),
        clip(
            "B",
            1,
            Time::ONE,
            Time::ONE,
            seconds(4, 24),
            Playback::Normal,
        ),
    ];
    let timeline = timeline(24, vec![video], vec![media("A", fps), media("B", fps)]);
    let (document, record) = lower(&timeline, &MediaTable::default()).unwrap();
    assert_eq!(document.tracks.len(), 2);
    assert_eq!(
        document
            .tracks
            .iter()
            .map(|track| track.clips.len())
            .sum::<usize>(),
        2
    );
    assert_eq!(
        document.tracks[0].clips[0].source_range,
        TimeCode(24)..TimeCode(52)
    );
    let head = &document.tracks[1].clips[0];
    assert_eq!(head.timeline_start, TimeCode(22));
    assert_eq!(head.transition_in.as_ref().unwrap().duration, TimeCode(6));
    assert!(
        record
            .rows
            .iter()
            .any(|row| row.code == LossCode::TransitionLane)
    );
}

#[test]
fn a_dissolve_next_to_a_nest_is_a_cut_and_an_anchor_stays_above_its_layers() {
    let fps = Rational::new(24, 1).unwrap();
    let track = |site, asset, at, span| {
        let mut track = Track::new(TrackKind::Video);
        track
            .items
            .push(clip(site, asset, at, Time::ONE, span, Playback::Normal));
        track
    };
    let mut base = track("A", 0, Time::ZERO, Time::ONE);
    base.items.push(Placed::new(
        Time::ONE,
        Item::Transition {
            kind: "SMPTE_Dissolve".into(),
            before: seconds(4, 24),
            after: seconds(4, 24),
        },
        "transition",
    ));
    let mut nested = Placed::new(
        Time::ONE,
        Item::Nested {
            stack: Stack {
                tracks: vec![
                    track("B", 1, Time::ZERO, Time::ONE),
                    track("C", 2, Time::ZERO, Time::ONE),
                ],
                ..Stack::default()
            },
            duration: Time::ONE,
        },
        "nest",
    );
    nested.anchors.push(Anchor {
        lane: 1,
        track: track("D", 3, Time::ONE, seconds(12, 24)),
    });
    base.items.push(nested);
    let timeline = timeline(
        24,
        vec![base],
        ["A", "B", "C", "D"].map(|name| media(name, fps)).into(),
    );
    let (document, record) = lower(&timeline, &MediaTable::default()).unwrap();
    assert!(
        record
            .rows
            .iter()
            .any(|row| row.code == LossCode::TransitionAtNestCut)
    );
    assert_eq!(
        clip_end(&document, &document.tracks[0].clips[0]),
        TimeCode(24)
    );
    let at = |frame| {
        document
            .tracks
            .iter()
            .rev()
            .find_map(|track| {
                track.clips.iter().find(|clip| {
                    clip.timeline_start <= TimeCode(frame)
                        && clip_end(&document, clip) > TimeCode(frame)
                })
            })
            .and_then(|clip| document.asset(clip.asset))
            .unwrap()
            .name
            .as_str()
    };
    assert_eq!(at(12), "A");
    assert_eq!(at(30), "D");
    assert_eq!(at(40), "C");
}

#[test]
fn dropped_container_counts_its_entire_subtree_and_the_trailing_gap_is_timeline_scoped() {
    let fps = Rational::new(24, 1).unwrap();
    let mut video = Track::new(TrackKind::Video);
    let mut unavailable = Track::new(TrackKind::Video);
    unavailable.items.push(clip(
        "inside",
        0,
        Time::ZERO,
        Time::ZERO,
        Time::ONE,
        Playback::Normal,
    ));
    unavailable.decorations.effects.push(ir::NamedEffect {
        name: "Blur".into(),
        site: "inside.effect".into(),
        code: LossCode::EffectDropped,
    });
    video.items = vec![
        Placed::new(
            Time::ZERO,
            Item::Nested {
                stack: Stack {
                    time: ContainerTime {
                        speed: Time::ZERO,
                        ..ContainerTime::default()
                    },
                    tracks: vec![unavailable],
                    ..Stack::default()
                },
                duration: Time::ONE,
            },
            "nest",
        ),
        clip(
            "visible",
            1,
            Time::ONE,
            Time::ZERO,
            Time::ONE,
            Playback::Normal,
        ),
        Placed::new(
            Time::integer(2),
            Item::Gap {
                duration: Time::ONE,
            },
            "tail",
        ),
    ];
    let timeline = timeline(
        24,
        vec![video],
        vec![media("inside", fps), media("visible", fps)],
    );
    let (document, record) = lower(&timeline, &MediaTable::default()).unwrap();
    assert_eq!(document.duration, TimeCode(48));
    assert_eq!((record.items, record.exact, record.dropped), (4, 1, 3));
    assert_eq!(
        record
            .rows
            .iter()
            .find(|row| row.code == LossCode::NestRetimeUnsupported)
            .unwrap()
            .count,
        3
    );
    assert!(
        record
            .rows
            .iter()
            .any(|row| row.code == LossCode::TrailingGapTrimmed
                && row.bound == Some(LossBound::Frames(24)))
    );
}

fn clip_end(document: &kinewright_core::Document, clip: &kinewright_core::Clip) -> TimeCode {
    clip.timeline_start
        .checked_add(document.clip_duration(clip).unwrap())
        .unwrap()
}
