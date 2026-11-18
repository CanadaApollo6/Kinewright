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
        time: ContainerTime::default(),
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

#[test]
fn available_media_can_force_a_cut_farther_than_one_source_frame() {
    let bytes = include_bytes!("fixtures/snap-media-ended.otio");
    let timeline = otio::parse(bytes, "snap-media-ended.otio", ParseOptions::default()).unwrap();
    let (document, record) = lower(&timeline, &MediaTable::default()).unwrap();
    assert_eq!(document.duration, TimeCode(24));
    let snap = record
        .rows
        .iter()
        .find(|row| row.code == LossCode::CutSnapped)
        .unwrap();
    assert_eq!(snap.bound, Some(LossBound::Frames(216)));
}

#[test]
fn omitted_codes_retain_dropped_items_when_more_than_eight_codes_compete() {
    let bytes = include_bytes!("fixtures/row-cap.otio");
    let timeline = otio::parse(bytes, "row-cap.otio", ParseOptions::default()).unwrap();
    let (_, record) = lower(&timeline, &MediaTable::default()).unwrap();
    assert_eq!(record.rows.len(), 8);
    assert!(record.omitted_codes.contains(&LossCode::UnknownItem));
    assert_eq!(record.dropped, 9);
}

#[test]
fn otio_audio_pairing_distinguishes_split_edits_and_collapses_dual_mono() {
    for (bytes, matched, code) in [
        (
            include_bytes!("fixtures/split-edit.otio").as_slice(),
            false,
            LossCode::AvLinkUnpaired,
        ),
        (
            include_bytes!("fixtures/dual-mono.otio").as_slice(),
            true,
            LossCode::ChannelMapDropped,
        ),
    ] {
        let timeline = otio::parse(bytes, "sound.otio", ParseOptions::default()).unwrap();
        let (document, record) = lower(&timeline, &MediaTable::default()).unwrap();
        let original = &document.tracks[0].clips[0];
        let audio: Vec<_> = document
            .tracks
            .iter()
            .filter(|track| track.kind == TrackKind::Audio)
            .flat_map(|track| &track.clips)
            .collect();
        assert_eq!(audio.len(), 1);
        if matched {
            assert!(original.link.is_some());
            assert_eq!(original.link, audio[0].link);
        } else {
            assert_eq!((original.link, audio[0].link), (None, None));
        }
        assert!(record.rows.iter().any(|row| row.code == code));
    }
}

#[test]
fn linked_transition_heads_and_bodies_have_separate_groups_in_both_formats() {
    for timeline in [
        otio::parse(
            include_bytes!("fixtures/linked-dissolve.otio"),
            "av.otio",
            ParseOptions::default(),
        )
        .unwrap(),
        fcpxml::parse(
            include_bytes!("fixtures/av-cross-dissolve.fcpxml"),
            "av.fcpxml",
            ParseOptions::default(),
        )
        .unwrap(),
    ] {
        let (document, _) = lower(&timeline, &MediaTable::default()).unwrap();
        let main_video = &document.tracks[0];
        let video_head = &document.tracks[1].clips[0];
        let main_audio = &document.tracks[2];
        let audio_head = &document.tracks[3].clips[0];
        assert!(video_head.link.is_some());
        assert_eq!(video_head.link, audio_head.link);
        assert!(main_video.clips[1].link.is_some());
        assert_eq!(main_video.clips[1].link, main_audio.clips[1].link);
        assert_ne!(video_head.link, main_video.clips[1].link);
    }
}

#[test]
fn hostile_decoded_file_locators_become_offline_assets() {
    let timeline = otio::parse(
        include_bytes!("fixtures/remote-locators.otio"),
        "remote.otio",
        ParseOptions::default(),
    )
    .unwrap();
    let (document, record) = lower(&timeline, &MediaTable::default()).unwrap();
    assert_eq!(
        (
            record.media.assets,
            record.media.offline,
            record.media.remote
        ),
        (2, 2, 2)
    );
    assert!(
        document
            .media_pool
            .iter()
            .all(|asset| asset.source_fingerprint.is_unknown())
    );
    assert_eq!(
        document.media_pool[0].path.to_str(),
        Some("file://localhost//host.invalid/share/a.mov")
    );
    assert!(
        record
            .rows
            .iter()
            .any(|row| row.code == LossCode::RemoteUrl && row.count == 2)
    );
}

#[test]
fn a_present_unprobed_asset_is_inspected_without_a_fingerprint_or_estimated_range() {
    let timeline = otio::parse(
        include_bytes!("fixtures/named-resolve.otio"),
        "inspect.otio",
        ParseOptions::default(),
    )
    .unwrap();
    let table = MediaTable {
        entries: vec![kinewright_interchange::Availability::Unprobed {
            path: "media/relinked.mov".into(),
            relinked_by_name: true,
        }],
    };
    let (document, record) = lower(&timeline, &table).unwrap();
    assert_eq!(
        (
            record.media.assets,
            record.media.online,
            record.media.offline
        ),
        (1, 1, 0)
    );
    assert!(record.estimated_assets.is_empty());
    assert!(document.media_pool[0].source_fingerprint.is_unknown());
    assert!(
        record
            .rows
            .iter()
            .any(|row| row.code == LossCode::RelinkedByName)
    );
}

#[test]
fn fcpxml_nest_edge_is_a_hard_cut_for_mirrored_audio() {
    let timeline = fcpxml::parse(
        include_bytes!("fixtures/av-transition-at-nest.fcpxml"),
        "av-transition-at-nest.fcpxml",
        ParseOptions::default(),
    )
    .unwrap();
    assert_av_hard_cut(&timeline);
}

#[test]
fn otio_nest_edge_is_one_hard_cut_for_video_and_audio() {
    let timeline = otio::parse(
        include_bytes!("fixtures/transition-at-nest.otio"),
        "transition-at-nest.otio",
        ParseOptions::default(),
    )
    .unwrap();
    assert_av_hard_cut(&timeline);
}

fn assert_av_hard_cut(timeline: &Timeline) {
    let (document, record) = lower(timeline, &MediaTable::default()).unwrap();
    let mut audio: Vec<_> = document
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Audio)
        .flat_map(|track| &track.clips)
        .collect();
    audio.sort_by_key(|clip| clip.timeline_start);
    let [outgoing, entering] = audio.as_slice() else {
        panic!(
            "expected only the two original audio clips, got {}",
            audio.len()
        );
    };
    assert_eq!(outgoing.audio_fade_out_frames, TimeCode::ZERO);
    assert_eq!(entering.audio_fade_in_frames, TimeCode::ZERO);
    assert_eq!(clip_end(&document, outgoing), TimeCode(24));
    assert_eq!(entering.timeline_start, TimeCode(24));
    assert!(
        document
            .tracks
            .iter()
            .flat_map(|track| &track.clips)
            .all(|clip| clip.transition_in.is_none())
    );
    let row = record
        .rows
        .iter()
        .find(|row| row.code == LossCode::TransitionAtNestCut)
        .unwrap();
    assert_eq!(row.count, 1);
    assert_eq!(row.bound, Some(LossBound::Frames(6)));
    assert!(!record.rows.iter().any(|row| matches!(
        row.code,
        LossCode::AudioCrossfadeLinear | LossCode::TransitionLane
    )));
}

#[test]
fn fcpxml_nest_edge_mirroring_requires_an_applied_video_transition() {
    let xml = include_str!("fixtures/av-cross-dissolve.fcpxml")
        .replace("hasVideo=\"1\"", "hasVideo=\"0\"");
    let timeline =
        fcpxml::parse(xml.as_bytes(), "audio-only.fcpxml", ParseOptions::default()).unwrap();
    let (document, record) = lower(&timeline, &MediaTable::default()).unwrap();
    let clips: Vec<_> = document
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .collect();
    let [outgoing, entering] = clips.as_slice() else {
        panic!("expected the two unmodified audio clips");
    };
    assert_eq!(outgoing.audio_fade_out_frames, TimeCode::ZERO);
    assert_eq!(entering.audio_fade_in_frames, TimeCode::ZERO);
    assert_eq!(clip_end(&document, outgoing), TimeCode(25));
    assert_eq!(entering.timeline_start, TimeCode(25));
    assert!(
        !record
            .rows
            .iter()
            .any(|row| row.code == LossCode::AudioCrossfadeLinear)
    );
}

#[test]
fn fcpxml_dissolve_preserves_gain_curves_on_the_head_and_body() {
    let xml = br#"<fcpxml version="1.13">
      <resources>
        <format id="f" frameDuration="1/25s" width="1920" height="1080"/>
        <asset id="a" format="f" start="0s" duration="6s" hasVideo="1" hasAudio="1">
          <media-rep kind="original-media" src="file:///before.mov"/>
        </asset>
        <asset id="b" format="f" start="0s" duration="6s" hasVideo="1" hasAudio="1">
          <media-rep kind="original-media" src="file:///after.mov"/>
        </asset>
      </resources>
      <project><sequence format="f"><spine>
        <asset-clip ref="a" start="1s" duration="1s"/>
        <transition offset="23/25s" duration="6/25s"/>
        <asset-clip ref="b" offset="1s" start="1s" duration="2s">
          <adjust-volume amount="0dB"><param name="amount">
            <keyframeAnimation>
              <keyframe time="1s" value="-60dB"/>
              <keyframe time="2s" value="0dB"/>
            </keyframeAnimation>
          </param></adjust-volume>
        </asset-clip>
      </spine></sequence></project>
    </fcpxml>"#;
    let parsed = fcpxml::parse(xml, "gain-dissolve.fcpxml", ParseOptions::default()).unwrap();
    let (document, _) = lower(&parsed, &MediaTable::default()).unwrap();
    let audio_at = |frame| {
        document
            .tracks
            .iter()
            .filter(|track| track.kind == TrackKind::Audio)
            .flat_map(|track| &track.clips)
            .find(|clip| clip.timeline_start == TimeCode(frame))
            .unwrap()
    };
    let head = audio_at(23)
        .audio_gain_curve
        .as_ref()
        .expect("head gain curve");
    let body = audio_at(29)
        .audio_gain_curve
        .as_ref()
        .expect("body gain curve");
    assert_eq!(head.keyframes.first().unwrap().at, TimeCode::ZERO);
    assert_eq!(head.value_at(TimeCode::ZERO), Some(-600));
    assert_eq!(head.value_at(TimeCode(2)), Some(-600));
    assert_eq!(head.value_at(TimeCode(5)), Some(-528));
    assert_eq!(body.keyframes.first().unwrap().at, TimeCode::ZERO);
    assert_eq!(body.value_at(TimeCode::ZERO), Some(-504));
    assert_eq!(body.keyframes.last().unwrap().at, TimeCode(21));
    assert_eq!(body.value_at(TimeCode(21)), Some(0));
}

#[test]
fn transition_fade_from_gap_is_counted_as_approximated() {
    assert_video_fade_counts(
        include_bytes!("fixtures/fade-from-gap.otio"),
        "fade-from-gap.otio",
        2,
    );
}

#[test]
fn transition_fade_to_gap_is_counted_as_approximated() {
    assert_video_fade_counts(
        include_bytes!("fixtures/fade-to-gap.otio"),
        "fade-to-gap.otio",
        1,
    );
}

fn assert_video_fade_counts(bytes: &[u8], name: &str, exact: u32) {
    let timeline = otio::parse(bytes, name, ParseOptions::default()).unwrap();
    let (_, record) = lower(&timeline, &MediaTable::default()).unwrap();
    assert_eq!(
        (
            record.items,
            record.exact,
            record.approximated,
            record.dropped
        ),
        (exact + 1, exact, 1, 0),
        "{name}"
    );
    let fade = record
        .rows
        .iter()
        .find(|row| row.code == LossCode::FadeShapeLinear)
        .unwrap();
    assert_eq!(fade.count, 1, "{name}");
    assert_eq!(
        fade.fidelity,
        kinewright_core::Fidelity::Approximated,
        "{name}"
    );
}
