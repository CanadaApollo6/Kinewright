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
