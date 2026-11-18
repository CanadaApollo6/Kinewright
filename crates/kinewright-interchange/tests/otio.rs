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

#[test]
fn item_cap_is_checked_before_deserializing_all_items() {
    let mut node = String::from("[");
    for _ in 0..=kinewright_interchange::MAX_ITEMS {
        node.push_str(r#"{"OTIO_SCHEMA":"Clip.1"},"#);
    }
    node.push(']');
    let error = otio::parse(node.as_bytes(), "items.otio", ParseOptions::default()).unwrap_err();
    assert_eq!(error.code, RefusalCode::TooManyItems);
}

#[test]
fn a_window_edge_turns_a_dissolve_into_a_reported_cut() {
    let mut input: serde_json::Value = serde_json::from_slice(WINDOW).unwrap();
    let children = input["tracks"]["children"][0]["children"]
        .as_array_mut()
        .unwrap();
    children.insert(
        1,
        serde_json::json!({
            "OTIO_SCHEMA": "Transition.1", "transition_type": "SMPTE_Dissolve",
            "in_offset": { "value": 2, "rate": 24 }, "out_offset": { "value": 2, "rate": 24 }
        }),
    );
    let bytes = serde_json::to_vec(&input).unwrap();
    let timeline = otio::parse(&bytes, "window.otio", ParseOptions::default()).unwrap();
    let affected = &timeline.root.tracks[0].items[1];
    assert!(matches!(
        affected.item,
        Item::Gap {
            duration: Time::ZERO
        }
    ));
    assert!(
        affected
            .notes
            .iter()
            .any(|loss| loss.code == LossCode::OutsideWindow)
    );
    let (document, record) =
        kinewright_interchange::lower(&timeline, &kinewright_interchange::MediaTable::default())
            .unwrap();
    assert!(document.tracks[0].clips[0].transition_in.is_none());
    assert!(
        record
            .rows
            .iter()
            .any(|row| row.code == LossCode::TransitionAtNestCut)
    );
}

#[test]
fn otio_clip_enabled_state_is_inherited_from_each_composition() {
    use serde_json::{Value, json};

    let source: Value = serde_json::from_slice(WINDOW).unwrap();
    let mut enabled_clip = source["tracks"]["children"][0]["children"][0].clone();
    enabled_clip["enabled"] = json!(true);
    let timeline = |timeline_enabled, stack_enabled, track_enabled, clip: &Value| {
        json!({
            "OTIO_SCHEMA": "Timeline.1",
            "enabled": timeline_enabled,
            "global_start_time": {"rate": 24, "value": 0},
            "tracks": {
                "OTIO_SCHEMA": "Stack.1",
                "enabled": stack_enabled,
                "children": [{
                    "OTIO_SCHEMA": "Track.1",
                    "kind": "Video",
                    "enabled": track_enabled,
                    "children": [clip]
                }]
            }
        })
    };
    for (timeline_enabled, stack_enabled, track_enabled, expected) in [
        (true, true, false, false),
        (true, false, true, false),
        (false, true, true, false),
        (true, true, true, true),
    ] {
        let bytes = serde_json::to_vec(&timeline(
            timeline_enabled,
            stack_enabled,
            track_enabled,
            &enabled_clip,
        ))
        .unwrap();
        let parsed = otio::parse(&bytes, "disabled.otio", ParseOptions::default()).unwrap();
        let (document, _) =
            kinewright_interchange::lower(&parsed, &kinewright_interchange::MediaTable::default())
                .unwrap();
        assert_eq!(document.tracks[0].clips[0].enabled, expected);
    }
}
