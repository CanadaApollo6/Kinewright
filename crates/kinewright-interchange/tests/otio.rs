use kinewright_core::{LossCode, Rational};
use kinewright_interchange::{
    RefusalCode, Time,
    ir::{Item, ParseOptions},
    otio,
};

const NAMED: &[u8] = include_bytes!("fixtures/named-resolve.otio");
const WINDOW: &[u8] = include_bytes!("fixtures/track-window.otio");

#[test]
fn parses_a_named_resolve_clip_at_exact_snapped_rate() {
    let timeline = otio::parse(NAMED, "reel2.otio", ParseOptions::default()).unwrap();
    assert_eq!(timeline.rate_hint, Rational::new(24_000, 1_001).unwrap());
    assert_eq!(timeline.start, Time::new(18_018, 5).unwrap());
    let placed = &timeline.root.tracks[0].items[0];
    let Item::Clip(clip) = &placed.item else {
        panic!("expected a clip");
    };
    assert_eq!(clip.name.as_deref(), Some("Interview \"A\""));
    assert!(placed.notes.iter().all(|note| !note.code.is_item()));
}

#[test]
fn track_window_retains_blue_and_rebases_it_to_zero() {
    let timeline = otio::parse(WINDOW, "track-window.otio", ParseOptions::default()).unwrap();
    let [red, blue] = timeline.root.tracks[0].items.as_slice() else {
        panic!("expected two report items");
    };
    assert!(
        matches!(&red.item, Item::Discarded { count: 1, loss } if loss.code == LossCode::OutsideWindow)
    );
    assert_eq!(blue.at, Time::ZERO);
    let Item::Clip(clip) = &blue.item else {
        panic!("expected a clip");
    };
    assert_eq!(clip.name.as_deref(), Some("Blue"));
}

#[test]
fn malformed_numbers_report_the_json_pointer() {
    for value in ["NaN", "Infinity"] {
        let input = format!(
            r#"{{"OTIO_SCHEMA":"Timeline.1","global_start_time":{{"rate":24,"value":{value}}}}}"#
        );
        let error = otio::parse(input.as_bytes(), "bad.otio", ParseOptions::default()).unwrap_err();
        assert_eq!(error.code, RefusalCode::Malformed);
        assert_eq!(error.at.as_deref(), Some("/global_start_time/value"));
    }
}

#[test]
fn massive_time_and_invalid_frame_rates_are_typed() {
    for (value, rate, code) in [
        ("1e300", 24, RefusalCode::Unrepresentable),
        ("0", 1001, RefusalCode::UnsupportedRate),
        ("0", 0, RefusalCode::UnsupportedRate),
        ("0", -25, RefusalCode::UnsupportedRate),
    ] {
        let input = format!(
            r#"{{"OTIO_SCHEMA":"Timeline.1","global_start_time":{{"rate":{rate},"value":{value}}}}}"#
        );
        let error = otio::parse(input.as_bytes(), "bad.otio", ParseOptions::default()).unwrap_err();
        assert_eq!(error.code, code);
    }
}

#[test]
fn nesting_is_bounded_before_serde_builds_the_tree() {
    let input = "[".repeat(1_000_000);
    let error = otio::parse(input.as_bytes(), "bad.otio", ParseOptions::default()).unwrap_err();
    assert_eq!(error.code, RefusalCode::TooDeep);
}
