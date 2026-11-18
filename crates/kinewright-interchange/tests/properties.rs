use std::collections::BTreeSet;

use kinewright_core::{Document, InterchangeFormat, MediaKind, Rational, TimeCode, TrackKind};
use kinewright_interchange::{
    MediaTable, RefusalCode, Time, fcpxml,
    ir::{
        Anchor, Clip, ContainerTime, Item, Media, MediaId, ParseOptions, Placed, Playback, Source,
        Stack, Timeline, Track,
    },
    lower, otio,
};
use proptest::{
    prelude::*,
    test_runner::{Config, TestCaseError, TestCaseResult, TestRunner},
};

const OTIO_SEED: &[u8] = include_bytes!("fixtures/linked-dissolve.otio");
const XML_SEED: &[u8] = include_bytes!("fixtures/av-cross-dissolve.fcpxml");

type Parser =
    fn(&[u8], &str, ParseOptions<'_>) -> Result<Timeline, kinewright_interchange::Refusal>;

fn verdict(
    result: Result<(Document, kinewright_core::ImportRecord), kinewright_interchange::Refusal>,
) -> TestCaseResult {
    match result {
        Ok((document, record)) => {
            prop_assert_eq!(document.validate(), Ok(()));
            prop_assert_eq!(record.validate(), Ok(()));
            prop_assert_eq!(
                record.items,
                record.exact + record.approximated + record.dropped
            );
        }
        Err(error) => prop_assert_ne!(error.code, RefusalCode::LoweringFailed, "{:?}", error),
    }
    Ok(())
}

fn parser_fuzz(cases: u32, parse: Parser, seed: &[u8], format: InterchangeFormat) {
    let mut runner = TestRunner::new(Config {
        cases,
        failure_persistence: None,
        ..Config::default()
    });
    let name = match format {
        InterchangeFormat::Otio => "fuzz.otio",
        InterchangeFormat::Fcpxml => "fuzz.fcpxml",
    };
    runner
        .run(
            &(
                any::<bool>(),
                0_u8..=3,
                0_i16..1_000,
                prop::collection::vec((any::<u16>(), any::<u8>()), 1..16),
            ),
            |(structure, mode, amount, edits)| {
                let bytes = if structure {
                    match format {
                        InterchangeFormat::Otio => {
                            otio_n(24, i64::from(amount), i64::from(mode.max(1)))
                        }
                        InterchangeFormat::Fcpxml => {
                            xml_n(25, i64::from(amount), i64::from(mode.max(1)))
                        }
                    }
                    .into_bytes()
                } else {
                    let mut bytes = seed.to_owned();
                    for (position, byte) in edits {
                        let position = usize::from(position) % bytes.len().max(1);
                        match mode {
                            0 => bytes.insert(position, byte),
                            1 if !bytes.is_empty() => {
                                bytes.remove(position);
                            }
                            2 if !bytes.is_empty() => bytes[position] = byte,
                            _ => bytes.truncate(position),
                        }
                    }
                    bytes
                };
                let first = parse(&bytes, name, ParseOptions::default());
                let second = parse(&bytes, name, ParseOptions::default());
                prop_assert_eq!(&first, &second);
                match first {
                    Ok(timeline) => {
                        let first = lower(&timeline, &MediaTable::default());
                        let second = lower(&timeline, &MediaTable::default());
                        prop_assert_eq!(&first, &second);
                        verdict(first)?;
                    }
                    Err(error) => prop_assert_ne!(error.code, RefusalCode::LoweringFailed),
                }
                Ok(())
            },
        )
        .unwrap();
}

#[test]
fn otio_fuzz_fast() {
    parser_fuzz(256, otio::parse, OTIO_SEED, InterchangeFormat::Otio);
}

#[test]
fn fcpxml_fuzz_fast() {
    parser_fuzz(256, fcpxml::parse, XML_SEED, InterchangeFormat::Fcpxml);
}

#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn otio_fuzz_slow() {
    parser_fuzz(50_000, otio::parse, OTIO_SEED, InterchangeFormat::Otio);
}

#[test]
#[cfg_attr(
    not(feature = "slow-tests"),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn fcpxml_fuzz_slow() {
    parser_fuzz(50_000, fcpxml::parse, XML_SEED, InterchangeFormat::Fcpxml);
}

fn time(frames: i64, rate: u32) -> Time {
    Time::new(frames, u64::from(rate)).unwrap()
}

fn media(name: &str, fps: Rational, kind: MediaKind) -> Media {
    Media {
        locator: format!("file:///Media/{name}"),
        name: name.into(),
        fps: Some(fps),
        kind,
        start: Some(Time::ZERO),
        duration: Some(Time::integer(300)),
        resolution: None,
    }
}

fn timeline(
    format: InterchangeFormat,
    source: &[u8],
    name: &str,
    fps: u32,
    tracks: Vec<Track>,
    media: Vec<Media>,
) -> Timeline {
    let version = match format {
        InterchangeFormat::Otio => "Timeline.1",
        InterchangeFormat::Fcpxml => "1.13",
    };
    Timeline {
        source: Source::new(format, version, name, source),
        name: name.into(),
        start: Time::ZERO,
        rate_hint: Rational::new(fps, 1).unwrap(),
        resolution_hint: Some((1920, 1080)),
        root: Stack {
            tracks,
            ..Stack::default()
        },
        media,
        notes: Vec::new(),
    }
}

fn on_track(kind: TrackKind, items: Vec<Placed>) -> Track {
    let mut track = Track::new(kind);
    track.items = items;
    track
}

fn clip(site: &str, asset: usize, at: Time, source: Time, span: Time, speed: Playback) -> Placed {
    let mut clip = Clip::new(MediaId(asset), source, span);
    clip.time = speed;
    Placed::new(at, Item::Clip(clip), site)
}

fn end(document: &Document, clip: &kinewright_core::Clip) -> i64 {
    clip.timeline_start.0 + document.clip_duration(clip).unwrap().0
}

fn rate(index: usize) -> Rational {
    let (n, d) = [(24_000, 1001), (25, 1), (30, 1), (60, 1)][index];
    Rational::new(n, d).unwrap()
}

fn unwind_site(p: (Result<Timeline, kinewright_interchange::Refusal>, Timeline)) -> TestCaseResult {
    let (parsed, model) = p;
    let parsed = parsed.map_err(|error| TestCaseError::fail(format!("parser: {error:?}")))?;
    let (from_file, _) = lower(&parsed, &MediaTable::default()).unwrap();
    let (expected, _) = lower(&model, &MediaTable::default()).unwrap();
    prop_assert_eq!(from_file, expected);
    Ok(())
}

proptest! {
    #![proptest_config(Config { cases: 256, failure_persistence: None, ..Config::default() })]

    #[test]
    fn windows_reserved_components_never_classify_as_local(
        prefix in "[a-zA-Z0-9_-]{1,16}",
        leaf in "[a-zA-Z0-9_-]{1,16}",
        extension in "[a-zA-Z0-9]{1,8}",
        device in prop::sample::select(vec![
            "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$",
            "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
            "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
            "COM¹", "COM²", "COM³", "LPT¹", "LPT²", "LPT³",
        ]),
        lower_case in any::<bool>(),
    ) {
        use kinewright_interchange::locator::{Locator, Platform, classify};

        let stem = if lower_case { device.to_ascii_lowercase() } else { device.into() };
        for locator in [
            format!(r"D:\{prefix}\{stem}  .{extension}\{leaf}.mov"),
            format!("file:///D:/{prefix}/{stem}.{extension}"),
            format!("file:///D:/{prefix}%20/{leaf}.mov"),
            format!(r"D:\{prefix}.\{leaf}.mov"),
            format!(r"D:{prefix}\{leaf}.mov"),
        ] {
            prop_assert_eq!(classify(&locator, Platform::Windows), Locator::Invalid, "{}", locator);
        }
    }

    #[test]
    fn windows_local_paths_never_contain_a_colon_outside_the_drive(
        stem in prop_oneof![Just("NUL".into()), "[a-zA-Z0-9_-]{1,16}\\.mov"],
        stream in "[a-zA-Z0-9_-]{1,16}",
        suffix in prop::sample::select(vec!["", "/clip.mov"]),
    ) {
        use kinewright_interchange::locator::{Locator, Platform, classify};

        for locator in [
            format!(r"D:\Media\{stem}:{stream}{suffix}"),
            format!(r"\\?\D:\Media\{stem}:{stream}{suffix}"),
            format!("file:///D:/Media/{stem}%3A{stream}{suffix}"),
        ] {
            prop_assert_eq!(classify(&locator, Platform::Windows), Locator::Invalid, "{}", locator);
        }
    }

    #[test]
    fn mixed_rates_keep_abutting_cuts_and_the_admitted_bound(
        rate_index in 0_usize..4, project_index in 0_usize..3,
        leading in 0_i64..120, a_duration in 30_i64..160, b_duration in 30_i64..160,
        audio in any::<bool>(), percent in prop_oneof![Just(100_u32), Just(10), Just(50), Just(125), Just(250)],
    ) {
        let project = [24, 50, 60][project_index];
        let fps = Rational::new(project, 1).unwrap();
        let source_fps = if audio { Rational::default() } else { rate(rate_index) };
        let kind = if audio { MediaKind::Audio } else { MediaKind::Video };
        let track_kind = if audio { TrackKind::Audio } else { TrackKind::Video };
        let start = Time::new(4 * leading + 1, u64::from(4 * project)).unwrap();
        let duration = time(a_duration, project);
        let desired_cut = start.checked_add(duration).unwrap();
        let b_len = time(b_duration, project);
        let model = timeline(InterchangeFormat::Otio, b"model", "property.otio", project, vec![on_track(track_kind, vec![
            clip("first", 0, start, Time::integer(5), duration, Playback::Speed(Time::new(i64::from(percent), 100).unwrap())),
            clip("second", 1, desired_cut, Time::integer(8), b_len, Playback::Normal),
        ])], vec![media("first.mov", source_fps, kind), media("second.mov", fps, kind)]);
        let (document, record) = lower(&model, &MediaTable::default()).unwrap();
        let clips = &document.tracks[0].clips;
        prop_assert_eq!(clips.len(), 2);
        prop_assert_eq!(document.validate(), Ok(()));
        prop_assert_eq!(record.validate(), Ok(()));
        let cut = end(&document, &clips[0]);
        prop_assert_eq!(cut, clips[1].timeline_start.0);
        let project_time = |at| Time::from_frame(TimeCode(at), fps).unwrap();
        prop_assert!(project_time(clips[0].timeline_start.0).checked_sub(start).unwrap().abs().unwrap() <= Time::new(1, u64::from(2 * project)).unwrap());
        let error = project_time(cut).checked_sub(desired_cut).unwrap().abs().unwrap();

        let effective = kinewright_core::speed_scaled_fps(source_fps, percent).unwrap();
        let source_start = clips[0].source_range.start;
        let duration_at = |source_end| kinewright_core::map_source_range_to_project(source_start..TimeCode(source_end), effective, fps).unwrap().0;
        let raw_winner = (source_start.0+1..=source_start.0+1_500).filter_map(|source_end| {
            let span = duration_at(source_end);
            (span > 0).then(|| {
                let frame = clips[0].timeline_start.0 + span;
                (project_time(frame).checked_sub(desired_cut).unwrap().abs().unwrap(), frame)
            })
        }).min().unwrap();
        let cap = desired_cut.checked_add(b_len).unwrap();
        let divisor = if project_time(raw_winner.1) <= cap { 2 } else { 1 };
        let bound = Time::new(1, u64::from(2 * project)).unwrap()
            .checked_add(Time::new(i64::from(effective.denominator()), u64::from(effective.numerator()) * divisor).unwrap()).unwrap();
        prop_assert!(error <= bound, "cut {cut}, error {error:?}, bound {bound:?}");
        if percent == 100 {
            let actual = Time::from_frame(clips[0].source_range.end, source_fps).unwrap();
            let ideal = Time::integer(5).checked_add(duration).unwrap();
            let bound = Time::new(i64::from(source_fps.denominator()), u64::from(source_fps.numerator())).unwrap()
                .checked_add(time(1, project)).unwrap();
            prop_assert!(actual.checked_sub(ideal).unwrap().abs().unwrap() <= bound);
        }
    }

    #[test]
    fn mixed_rate_dissolves_always_validate(
        a in 0_usize..4, b in 0_usize..4, project_idx in 0_usize..3,
        cut in 30_i64..150, after in 1_i64..20, before in 1_i64..20,
    ) {
        let project = [24,50,60][project_idx];
        let cut_time = time(cut, project);
        let model = timeline(InterchangeFormat::Otio, b"model", "dissolve.otio", project, vec![on_track(TrackKind::Video, vec![
            clip("A",0, Time::ZERO, Time::integer(5),cut_time,Playback::Normal),
            Placed::new(cut_time, Item::Transition {
                kind:"SMPTE_Dissolve".into(), before:time(before,project), after:time(after,project),
            }, "transition"),
            clip("B",1,cut_time,Time::integer(5),time(50,project),Playback::Normal),
        ])], vec![media("A.mov",rate(a),MediaKind::Video),media("B.mov",rate(b),MediaKind::Video)]);
        let result = lower(&model, &MediaTable::default());
        prop_assert!(result.is_ok(), "{result:?}");
        verdict(result)?;
    }

    #[test]
    fn both_test_writers_round_trip_the_exact_subset(fps in 1_u32..121, start in 0_i64..1000, span in 1_i64..240) {
        for (format, parser, bytes, name, site) in [
            (InterchangeFormat::Otio, otio::parse as Parser, otio_n(fps,start,span), "exact.otio", "/tracks/children/0/children/0"),
            (InterchangeFormat::Fcpxml, fcpxml::parse as Parser, xml_n(fps,start,span), "exact.fcpxml", "1:1"),
        ] {
            let fps_rational = Rational::new(fps,1).unwrap();
            let (at, in_point) = if format == InterchangeFormat::Otio { (Time::ZERO, time(start,fps)) } else { (time(start,fps), Time::ZERO) };
            let mut source = media("source.mov", fps_rational, MediaKind::Video);
            source.resolution = (format == InterchangeFormat::Fcpxml).then_some((1920,1080));
            let model = timeline(format, bytes.as_bytes(), name, fps, vec![on_track(TrackKind::Video, vec![clip(site,0,at,in_point,time(span,fps),Playback::Normal)])], vec![source]);
            unwind_site((parser(bytes.as_bytes(),name,ParseOptions { resolution:Some((1920,1080)), ..ParseOptions::default() }), model))?;
        }
    }

    #[test]
    fn generated_nests_keep_topmost_opaque_source_with_concurrent_anchors(
        anchor_start in 0_i64..12, anchor_length in 1_i64..12, spill_start in 0_i64..12,
    ) {
        let fps = Rational::new(24, 1).unwrap();
        let mut inner_lower = on_track(TrackKind::Video, vec![
            clip("base",0,Time::ZERO,Time::ONE,Time::ONE,Playback::Normal),
            clip("spill", 1, time(spill_start,24), Time::ONE,time(6,24),Playback::Normal),
        ]);
        inner_lower.name = Some("Inner".into());
        let upper = on_track(TrackKind::Video, vec![
            clip("upper", 2, time(4,24),Time::ONE,time(16,24),Playback::Normal)
        ]);
        let mut nest = Placed::new(Time::ZERO, Item::Nested { stack:Stack { tracks:vec![inner_lower,upper], ..Stack::default() },duration:Time::ONE }, "nest");
        nest.anchors.push(Anchor { lane:1,time:ContainerTime::default(),track:on_track(TrackKind::Video,vec![clip("anchor",3,time(anchor_start,24),Time::ONE,time(anchor_length,24),Playback::Normal)]) });
        let inputs = (0..4).map(|index| media(&format!("{index}.mov"),fps,MediaKind::Video)).collect();
        let model = timeline(InterchangeFormat::Fcpxml,b"model","nest.fcpxml",24,vec![on_track(TrackKind::Video,vec![nest])],inputs);
        let (document, _) = lower(&model,&MediaTable::default()).unwrap();
        for frame in 0..24 {
            let (layer,start) = if (anchor_start..anchor_start+anchor_length).contains(&frame) {(3,anchor_start)}
                else if (4..20).contains(&frame) {(2,4)}
                else if (spill_start..spill_start+6).contains(&frame) {(1,spill_start)}
                else {(0,0)};
            let actual = document.tracks.iter().rev().flat_map(|track| &track.clips)
                .find(|clip| clip.timeline_start.0 <= frame && frame < end(&document,clip)).unwrap();
            let asset = document.asset(actual.asset).unwrap();
            prop_assert_eq!(&asset.name, &format!("{layer}.mov"));
            let source_frame = actual.source_range.start.0 + frame - actual.timeline_start.0;
            prop_assert_eq!(source_frame, 24 + frame - start);
        }
    }
}

fn otio_n(fps: u32, start: i64, span: i64) -> String {
    let rt = |value| serde_json::json!({"OTIO_SCHEMA":"RationalTime.1","value":value,"rate":fps});
    let range = |start, end| serde_json::json!({"OTIO_SCHEMA":"TimeRange.1","start_time":rt(start),"duration":rt(end)});
    serde_json::json!({"OTIO_SCHEMA":"Timeline.1","global_start_time":rt(0),"tracks":{
        "OTIO_SCHEMA":"Stack.1","children":[{"OTIO_SCHEMA":"Track.1","kind":"Video","children":[{
            "OTIO_SCHEMA":"Clip.1","source_range":range(start,span),"media_reference":{
                "OTIO_SCHEMA":"ExternalReference.1","target_url":"file:///Media/source.mov","available_range":range(0,300*i64::from(fps))
            }
        }]}]
    }}).to_string()
}

fn xml_n(fps: u32, start: i64, span: i64) -> String {
    format!(
        r#"<fcpxml version="1.13"><resources><format id="f" frameDuration="1/{fps}s" width="1920" height="1080"/><asset id="a" format="f" start="0s" duration="300s" hasVideo="1" name="source.mov"><media-rep kind="original-media" src="file:///Media/source.mov"/></asset></resources><project><sequence format="f"><spine><asset-clip ref="a" offset="{start}/{fps}s" duration="{span}/{fps}s"/></spine></sequence></project></fcpxml>"#
    )
}

#[test]
fn shipping_dependency_graph_links_no_network_client() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let tree = std::process::Command::new(env!("CARGO"))
        .args([
            "tree",
            "--offline",
            "--locked",
            "-e",
            "normal",
            "--manifest-path",
        ])
        .arg(manifest)
        .output()
        .unwrap();
    assert!(
        tree.status.success(),
        "{}",
        String::from_utf8_lossy(&tree.stderr)
    );
    let stdout = String::from_utf8(tree.stdout).unwrap();
    let packages: BTreeSet<_> = stdout
        .lines()
        .filter_map(|line| {
            line.trim_start_matches(|c: char| !c.is_ascii_alphabetic())
                .split_whitespace()
                .next()
        })
        .collect();
    for banned in [
        "reqwest", "hyper", "tokio", "socket2", "rustls", "url", "curl", "mio",
    ] {
        assert!(
            !packages.contains(banned),
            "network package {banned} linked"
        );
    }
}
