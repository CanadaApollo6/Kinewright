use kinewright_core::{LossCode, Rational};
use kinewright_interchange::{
    RefusalCode, Time, fcpxml,
    ir::{Item, ParseOptions, Playback},
};

const WINDOW: &[u8] = include_bytes!("fixtures/sync-window.fcpxml");
const ANCHORS: &[u8] = include_bytes!("fixtures/anchors-and-retime.fcpxml");

#[test]
fn sync_window_cuts_the_inner_spine_and_counts_the_outside_clip() {
    let timeline = fcpxml::parse(WINDOW, "sync-window.fcpxml", ParseOptions::default()).unwrap();
    assert_eq!(timeline.rate_hint, Rational::new(25, 1).unwrap());
    let Item::Nested { stack, duration } = &timeline.root.tracks[0].items[0].item else {
        panic!("expected a sync container");
    };
    assert_eq!(*duration, Time::new(788, 25).unwrap());
    let [gap, clip] = stack.tracks[0].items.as_slice() else {
        panic!("expected two report items");
    };
    assert_eq!(gap.item.span(), *duration);
    assert!(
        matches!(&clip.item, Item::Discarded { count: 1, loss } if loss.code == LossCode::OutsideWindow)
    );
}

#[test]
fn normal_and_retimed_anchors_use_parent_local_time() {
    let timeline = fcpxml::parse(ANCHORS, "anchors.fcpxml", ParseOptions::default()).unwrap();
    let [normal, retimed] = timeline.root.tracks[0].items.as_slice() else {
        panic!("expected two clips");
    };
    assert_eq!(normal.anchors[0].track.items[0].at, Time::integer(12));
    let connected = &retimed.anchors[0].track.items[0];
    assert_eq!(connected.at, Time::integer(15));
    assert!(
        connected
            .notes
            .iter()
            .any(|loss| loss.code == LossCode::AnchorRetimeReading)
    );
    let Item::Clip(clip) = &retimed.item else {
        panic!("expected a clip");
    };
    assert_eq!(clip.source_start, Time::integer(3604));
    assert_eq!(clip.time, Playback::Speed(Time::integer(2)));
}

#[test]
fn document_declarations_cannot_enable_entities_or_external_subsets() {
    for xml in [
        r#"<!DOCTYPE fcpxml [<!ENTITY a "A"><!ENTITY b "&a;&a;&a;">]><fcpxml version="1.13">&b;</fcpxml>"#,
        r#"<!DOCTYPE fcpxml [<!ENTITY external SYSTEM "file:///outside.txt">]><fcpxml version="1.13">&external;</fcpxml>"#,
        r#"<!DOCTYPE fcpxml SYSTEM "file:///outside.dtd"><fcpxml version="1.13"/>"#,
    ] {
        let error =
            fcpxml::parse(xml.as_bytes(), "unsafe.fcpxml", ParseOptions::default()).unwrap_err();
        assert_eq!(error.code, RefusalCode::DtdNotAllowed);
        assert_eq!(error.at.as_deref(), Some("1:1"));
    }
}

#[test]
fn duplicate_ids_and_resource_cycles_are_typed() {
    for xml in [
        r#"<fcpxml version="1.13"><resources><asset id="a"/><asset id="a"/></resources></fcpxml>"#,
        r#"<fcpxml version="1.13"><resources><media id="a"><sequence><spine><ref-clip ref="b"/></spine></sequence></media><media id="b"><sequence><spine><ref-clip ref="a"/></spine></sequence></media></resources></fcpxml>"#,
    ] {
        let error =
            fcpxml::parse(xml.as_bytes(), "bad.fcpxml", ParseOptions::default()).unwrap_err();
        assert_eq!(error.code, RefusalCode::Malformed);
        assert!(error.at.as_deref().unwrap().starts_with("1:"));
    }
}

#[test]
fn deeply_nested_markup_is_refused_before_dom_allocation() {
    let xml = "<n>".repeat(1_000_000);
    let error = fcpxml::parse(xml.as_bytes(), "deep.fcpxml", ParseOptions::default()).unwrap_err();
    assert_eq!(error.code, RefusalCode::TooDeep);
}

#[test]
fn format_rates_over_the_cap_and_magnitudes_over_i64_are_typed() {
    for (time, code) in [
        ("1/1001s", RefusalCode::UnsupportedRate),
        ("-1/25s", RefusalCode::UnsupportedRate),
        ("0s", RefusalCode::UnsupportedRate),
        ("9223372036854775808s", RefusalCode::Unrepresentable),
    ] {
        let xml = format!(
            r#"<fcpxml version="1.13"><resources><format id="f" frameDuration="{time}"/></resources></fcpxml>"#
        );
        assert_eq!(
            fcpxml::parse(xml.as_bytes(), "bad.fcpxml", ParseOptions::default())
                .unwrap_err()
                .code,
            code
        );
    }
}

#[test]
fn the_dom_refuses_a_node_count_over_its_cap() {
    let mut xml = String::from(r#"<fcpxml version="1.13">"#);
    for _ in 0..kinewright_interchange::MAX_XML_NODES {
        xml.push_str("<z/>");
    }
    xml.push_str("</fcpxml>");
    let error = fcpxml::parse(xml.as_bytes(), "nodes.fcpxml", ParseOptions::default()).unwrap_err();
    assert_eq!(error.code, RefusalCode::TooManyItems);
}

#[test]
fn distinct_active_multicam_picture_and_sound_angles_are_both_lowered() {
    let input = br#"<fcpxml version="1.13">
      <resources>
        <format id="fmt" frameDuration="1/25s" width="1920" height="1080"/>
        <asset id="a" format="fmt" name="Camera" duration="4s" hasVideo="1" hasAudio="1">
          <media-rep kind="original-media" src="file:///camera.mov"/>
        </asset>
        <asset id="b" format="fmt" name="Recorder" duration="4s" hasVideo="1" hasAudio="1">
          <media-rep kind="original-media" src="file:///recorder.mov"/>
        </asset>
        <media id="mc"><multicam>
          <mc-angle angleID="a"><asset-clip ref="a" duration="4s"/></mc-angle>
          <mc-angle angleID="b"><asset-clip ref="b" duration="4s"/></mc-angle>
        </multicam></media>
      </resources>
      <project><sequence format="fmt"><spine>
        <mc-clip ref="mc" start="1s" duration="1s">
          <mc-source angleID="a" srcEnable="video"/><mc-source angleID="b" srcEnable="audio"/>
        </mc-clip>
      </spine></sequence></project>
    </fcpxml>"#;
    let timeline = fcpxml::parse(input, "mc.fcpxml", ParseOptions::default()).unwrap();
    let (document, record) =
        kinewright_interchange::lower(&timeline, &kinewright_interchange::MediaTable::default())
            .unwrap();
    let kinds: Vec<_> = document
        .tracks
        .iter()
        .flat_map(|track| {
            track.clips.iter().map(|clip| {
                (
                    track.kind,
                    document.asset(clip.asset).unwrap().name.as_str(),
                )
            })
        })
        .collect();
    assert_eq!(
        kinds,
        [
            (kinewright_core::TrackKind::Video, "Camera"),
            (kinewright_core::TrackKind::Audio, "Recorder")
        ]
    );
    assert!(
        record
            .rows
            .iter()
            .any(|row| row.code == LossCode::MulticamActiveAngle && row.count == 2)
    );
}

#[test]
fn fcpxml_clip_enabled_state_is_inherited_inside_and_across_resources() {
    let xml = br#"<fcpxml version="1.13">
      <resources>
        <format id="f" frameDuration="1/24s" width="1920" height="1080"/>
        <asset id="a" format="f" start="0s" duration="4s" hasVideo="1" hasAudio="1">
          <media-rep kind="original-media" src="file:///a.mov"/>
        </asset>
        <media id="c"><sequence format="f"><spine>
          <asset-clip ref="a" duration="1s" enabled="1"/>
        </spine></sequence></media>
      </resources>
      <project><sequence format="f"><spine>
        <sync-clip enabled="0" offset="0s" duration="1s"><spine>
          <asset-clip ref="a" duration="1s" enabled="1"/>
        </spine></sync-clip>
        <ref-clip ref="c" enabled="0" offset="1s" duration="1s"/>
        <ref-clip ref="c" offset="2s" duration="1s"/>
        <asset-clip ref="a" enabled="0" offset="3s" duration="1s"/>
      </spine></sequence></project>
    </fcpxml>"#;
    let timeline = fcpxml::parse(xml, "disabled.fcpxml", ParseOptions::default()).unwrap();
    let (document, _) =
        kinewright_interchange::lower(&timeline, &kinewright_interchange::MediaTable::default())
            .unwrap();
    for kind in [
        kinewright_core::TrackKind::Video,
        kinewright_core::TrackKind::Audio,
    ] {
        let mut clips: Vec<_> = document
            .tracks
            .iter()
            .filter(|track| track.kind == kind)
            .flat_map(|track| &track.clips)
            .map(|clip| (clip.timeline_start.0, clip.enabled))
            .collect();
        clips.sort_unstable();
        assert_eq!(clips, [(0, false), (24, false), (48, true), (72, false)]);
    }
}
