//! AU1 manual mix — core contract tests (AU1 §7 items 2–7).

use kinewright_core::{
    AudioMix, Command, Core, Document, Event, MixLevelRequest, OpError, Operation, Rational,
    TRACK_MIX_GAIN_MAX, TRACK_MIX_GAIN_MIN, TRACK_MIX_PAN_MAX, TRACK_MIX_PAN_MIN, TimeCode, Track,
    TrackId, TrackKind, TrackMix, effect_descriptor,
};

fn empty_timeline(fps: Rational) -> Document {
    Document {
        catalog: kinewright_core::MediaCatalog::default(),
        audio_mix: AudioMix::default(),
        color_context: kinewright_core::ColorContext::default(),
        lut_assets: Vec::new(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: Vec::new(),
        }],
        media_pool: Vec::new(),
        markers: Vec::new(),
        fps,
        resolution: (1_920, 1_080),
        duration: TimeCode::ZERO,
    }
}

fn two_track_document() -> Document {
    let fps = Rational::new(30, 1).unwrap();
    let mut doc = empty_timeline(fps);
    Operation::AddTrack {
        track: Track {
            id: TrackId(2),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: Vec::new(),
        },
    }
    .apply(&mut doc)
    .unwrap();
    doc
}

const fn set_mix(
    track: u64,
    gain_tenth_db: i32,
    pan_percent: i32,
    mute: bool,
    solo: bool,
) -> Operation {
    Operation::SetTrackMix {
        track: TrackId(track),
        gain_tenth_db,
        pan_percent,
        mute,
        solo,
    }
}

/// AU1 §7 item 2.
#[test]
fn set_track_mix_validates_bounds_and_track_existence_atomically() {
    let base = two_track_document();

    for gain_tenth_db in [TRACK_MIX_GAIN_MIN, TRACK_MIX_GAIN_MAX] {
        let mut doc = base.clone();
        set_mix(1, gain_tenth_db, 0, false, false)
            .apply(&mut doc)
            .unwrap();
        assert_eq!(doc.track_mix(TrackId(1)).gain_tenth_db, gain_tenth_db);
    }
    for pan_percent in [TRACK_MIX_PAN_MIN, TRACK_MIX_PAN_MAX] {
        let mut doc = base.clone();
        set_mix(1, 0, pan_percent, false, false)
            .apply(&mut doc)
            .unwrap();
        assert_eq!(doc.track_mix(TrackId(1)).pan_percent, pan_percent);
    }

    let rejections = [
        (
            set_mix(1, TRACK_MIX_GAIN_MIN - 1, 0, false, false),
            OpError::TrackMixGainOutOfRange {
                track: TrackId(1),
                gain_tenth_db: -601,
            },
        ),
        (
            set_mix(1, TRACK_MIX_GAIN_MAX + 1, 0, false, false),
            OpError::TrackMixGainOutOfRange {
                track: TrackId(1),
                gain_tenth_db: 121,
            },
        ),
        (
            set_mix(1, 0, TRACK_MIX_PAN_MIN - 1, false, false),
            OpError::TrackMixPanOutOfRange {
                track: TrackId(1),
                pan_percent: -101,
            },
        ),
        (
            set_mix(1, 0, TRACK_MIX_PAN_MAX + 1, false, false),
            OpError::TrackMixPanOutOfRange {
                track: TrackId(1),
                pan_percent: 101,
            },
        ),
        (
            set_mix(9, -60, 0, false, false),
            OpError::MissingTrack(TrackId(9)),
        ),
    ];
    // Every rejection is run twice: once against an empty mix table, and once
    // against a document that already carries a non-neutral entry, so
    // `doc == before` means "the surviving entry is untouched" rather than only
    // "nothing was added". `ApplyOp::apply` gets its atomicity from a
    // clone-then-swap, and this pins that the guarantee covers a
    // partially-populated mix table too.
    let mut occupied = base.clone();
    set_mix(1, -60, 25, false, false)
        .apply(&mut occupied)
        .unwrap();
    assert_eq!(occupied.audio_mix.tracks.len(), 1);
    for before in [&base, &occupied] {
        for (operation, expected) in &rejections {
            let mut doc = before.clone();
            assert_eq!(operation.apply(&mut doc).unwrap_err(), *expected);
            assert_eq!(doc, *before);
        }
    }

    assert_eq!(
        OpError::TrackMixGainOutOfRange {
            track: TrackId(1),
            gain_tenth_db: -601,
        }
        .to_string(),
        "track mix gain on track 1 is -601 tenth-dB, outside the inclusive range -600..=120"
    );
    assert_eq!(
        OpError::TrackMixPanOutOfRange {
            track: TrackId(1),
            pan_percent: 101,
        }
        .to_string(),
        "track mix pan on track 1 is 101 percent, outside the inclusive range -100..=100"
    );
    assert_eq!(
        OpError::DuplicateTrackMix(TrackId(2)).to_string(),
        "track 2 has more than one mix entry"
    );
    assert_eq!(
        OpError::TrackMixUnsorted.to_string(),
        "track mix entries are not sorted by track id"
    );
}

/// AU1 §7 item 3.
#[test]
fn neutral_track_mix_removes_the_entry_and_serializes_byte_identically() {
    let mut doc = two_track_document();
    let before = serde_json::to_string(&doc).unwrap();
    assert!(!before.contains("audio_mix"));

    set_mix(2, -60, 25, false, true).apply(&mut doc).unwrap();
    assert_eq!(
        doc.audio_mix.tracks,
        vec![TrackMix {
            track: TrackId(2),
            gain_tenth_db: -60,
            pan_percent: 25,
            mute: false,
            solo: true,
        }]
    );
    assert!(serde_json::to_string(&doc).unwrap().contains("audio_mix"));

    set_mix(2, 0, 0, false, false).apply(&mut doc).unwrap();
    assert!(doc.audio_mix.tracks.is_empty());
    assert!(doc.audio_mix.is_empty());
    assert_eq!(serde_json::to_string(&doc).unwrap(), before);
    // The pre-AU1 bytes still load, and load back to this document.
    assert_eq!(serde_json::from_str::<Document>(&before).unwrap(), doc);

    // A never-set track stays neutral and writes nothing.
    let mut fresh = two_track_document();
    set_mix(1, 0, 0, false, false).apply(&mut fresh).unwrap();
    assert_eq!(
        serde_json::to_string(&fresh).unwrap(),
        serde_json::to_string(&two_track_document()).unwrap()
    );
    assert_eq!(fresh.track_mix(TrackId(1)), TrackMix::neutral(TrackId(1)));
    assert!(fresh.track_mix(TrackId(1)).is_neutral());
    assert!(fresh.track_audible(TrackId(1)));
}

/// AU1 §7 item 4.
#[test]
fn hand_edited_track_mix_entries_are_rejected_during_document_validation() {
    let mut valid = two_track_document();
    set_mix(1, -60, 0, false, false).apply(&mut valid).unwrap();
    set_mix(2, 30, -20, true, false).apply(&mut valid).unwrap();
    assert_eq!(valid.validate(), Ok(()));

    let cases: Vec<(Vec<serde_json::Value>, OpError)> = vec![
        (
            vec![
                serde_json::json!({"track": 1, "gain_tenth_db": -60}),
                serde_json::json!({"track": 1, "gain_tenth_db": 30}),
            ],
            OpError::DuplicateTrackMix(TrackId(1)),
        ),
        (
            vec![
                serde_json::json!({"track": 2, "gain_tenth_db": 30}),
                serde_json::json!({"track": 1, "gain_tenth_db": -60}),
            ],
            OpError::TrackMixUnsorted,
        ),
        (
            vec![serde_json::json!({"track": 9, "gain_tenth_db": -60})],
            OpError::MissingTrack(TrackId(9)),
        ),
        (
            vec![serde_json::json!({"track": 1, "gain_tenth_db": -601})],
            OpError::TrackMixGainOutOfRange {
                track: TrackId(1),
                gain_tenth_db: -601,
            },
        ),
        (
            vec![serde_json::json!({"track": 1, "pan_percent": 101})],
            OpError::TrackMixPanOutOfRange {
                track: TrackId(1),
                pan_percent: 101,
            },
        ),
    ];
    for (tracks, expected) in cases {
        let mut value = serde_json::to_value(&valid).unwrap();
        value["audio_mix"]["tracks"] = serde_json::Value::Array(tracks);
        let loaded: Document = serde_json::from_value(value).unwrap();
        assert_eq!(loaded.validate(), Err(expected));
    }

    // A hand-written neutral entry loads and validates.
    let mut value = serde_json::to_value(&valid).unwrap();
    value["audio_mix"]["tracks"] = serde_json::json!([{"track": 1}]);
    let loaded: Document = serde_json::from_value(value).unwrap();
    assert_eq!(loaded.validate(), Ok(()));
    assert_eq!(loaded.audio_mix.tracks, vec![TrackMix::neutral(TrackId(1))]);

    // A pre-AU1 `audio_mix` object carries only `buses`, and a pre-AU1 document
    // has no `audio_mix` key at all; both load to an empty table.
    let mut value = serde_json::to_value(&valid).unwrap();
    value["audio_mix"] = serde_json::json!({"buses": []});
    let loaded: Document = serde_json::from_value(value).unwrap();
    assert!(loaded.audio_mix.tracks.is_empty());
    assert_eq!(loaded.validate(), Ok(()));

    let mut value = serde_json::to_value(&valid).unwrap();
    value.as_object_mut().unwrap().remove("audio_mix");
    let loaded: Document = serde_json::from_value(value).unwrap();
    assert!(loaded.audio_mix.is_empty());
    assert_eq!(loaded.validate(), Ok(()));
}

/// AU1 §7 item 5.
#[test]
fn remove_track_drops_the_track_mix_entry() {
    let mut doc = two_track_document();
    // Written in descending track order so the upsert's sort is exercised.
    set_mix(2, 30, 40, false, true).apply(&mut doc).unwrap();
    set_mix(1, -60, 0, false, false).apply(&mut doc).unwrap();
    assert_eq!(
        doc.audio_mix
            .tracks
            .iter()
            .map(|entry| entry.track)
            .collect::<Vec<_>>(),
        vec![TrackId(1), TrackId(2)]
    );
    assert!(doc.audio_mix.any_solo());
    // Track 2 is soloed, so track 1 is suppressed until the solo goes away.
    assert!(!doc.track_audible(TrackId(1)));
    assert!(doc.track_audible(TrackId(2)));

    Operation::RemoveTrack { track: TrackId(2) }
        .apply(&mut doc)
        .unwrap();

    assert_eq!(
        doc.audio_mix.tracks,
        vec![TrackMix {
            track: TrackId(1),
            gain_tenth_db: -60,
            pan_percent: 0,
            mute: false,
            solo: false,
        }]
    );
    assert!(!doc.audio_mix.any_solo());
    assert!(doc.track_audible(TrackId(1)));
    assert_eq!(doc.validate(), Ok(()));
}

/// AU1 §7 item 6.
#[test]
fn coalesced_track_mix_drag_is_one_undo_entry() {
    let initial = two_track_document();
    let core = Core::spawn(initial.clone()).unwrap();

    let mut tenth = None;
    for step in 1..=10 {
        let Event::DocumentChanged { doc, .. } = core
            .request(Command::DoBatchCoalesced {
                operations: vec![set_mix(1, -step * 10, 0, false, false)],
                coalesce_key: "track_mix:1#1".to_owned(),
            })
            .unwrap()
        else {
            panic!("coalesced track mix batch should be accepted");
        };
        assert_eq!(doc.track_mix(TrackId(1)).gain_tenth_db, -step * 10);
        tenth = Some(doc);
    }
    let tenth = tenth.unwrap();

    let Event::DocumentChanged {
        doc,
        revision: first,
        ..
    } = core.request(Command::Undo).unwrap()
    else {
        panic!("the gesture should be undoable");
    };
    assert_eq!(&*doc, &initial);

    let Event::DocumentChanged {
        doc,
        revision: second,
        ..
    } = core.request(Command::Undo).unwrap()
    else {
        panic!("a second undo should still report the document");
    };
    assert_eq!(&*doc, &initial);
    assert_eq!(first, second);

    let Event::DocumentChanged { doc, .. } = core.request(Command::Redo).unwrap() else {
        panic!("the gesture should be redoable");
    };
    assert_eq!(&*doc, &*tenth);
    assert_eq!(doc.track_mix(TrackId(1)).gain_tenth_db, -100);
}

/// AU1 §7 item 7.
#[test]
fn track_mix_gain_range_matches_the_audio_gain_descriptor() {
    let descriptor = effect_descriptor("audio_gain").expect("audio_gain is registered");
    let parameter = descriptor
        .parameter("gain_tenth_db")
        .expect("audio_gain exposes gain_tenth_db");
    assert_eq!(parameter.min, i64::from(TRACK_MIX_GAIN_MIN));
    assert_eq!(parameter.max, i64::from(TRACK_MIX_GAIN_MAX));
}

/// AU1 §3.1 in the false direction. Core owns `track_audible`; both the media
/// track stage and the level report call it rather than re-deriving the gate.
#[test]
fn track_audible_is_false_when_muted_or_solo_suppressed() {
    // A muted track is inaudible even when nothing is soloed.
    let mut muted = two_track_document();
    set_mix(1, 0, 0, true, false).apply(&mut muted).unwrap();
    assert!(!muted.audio_mix.any_solo());
    assert!(!muted.track_audible(TrackId(1)));
    assert!(muted.track_audible(TrackId(2)));

    // Another track soloed suppresses every non-soloed track.
    let mut soloed = two_track_document();
    set_mix(2, 0, 0, false, true).apply(&mut soloed).unwrap();
    assert!(soloed.audio_mix.any_solo());
    assert!(!soloed.track_audible(TrackId(1)));
    assert!(soloed.track_audible(TrackId(2)));

    // Both soloed: both audible.
    set_mix(1, 0, 0, false, true).apply(&mut soloed).unwrap();
    assert!(soloed.track_audible(TrackId(1)));
    assert!(soloed.track_audible(TrackId(2)));

    // Mute wins over the track's own solo.
    set_mix(1, 0, 0, true, true).apply(&mut soloed).unwrap();
    assert!(!soloed.track_audible(TrackId(1)));
    assert!(soloed.track_audible(TrackId(2)));
}

/// AU1 §6.1: `MixLevelRequest` is a public JSON input type, so the
/// whole-document request must be the empty object on the wire.
#[test]
fn mix_level_request_omits_an_absent_range_on_the_wire() {
    let whole = serde_json::from_str::<MixLevelRequest>("{}").unwrap();
    assert_eq!(whole, MixLevelRequest { range: None });
    assert_eq!(serde_json::to_string(&whole).unwrap(), "{}");

    let windowed = MixLevelRequest {
        range: Some(TimeCode(30)..TimeCode(60)),
    };
    assert_eq!(
        serde_json::from_str::<MixLevelRequest>(r#"{"range":{"start":30,"end":60}}"#).unwrap(),
        windowed
    );
    assert_eq!(
        serde_json::to_string(&windowed).unwrap(),
        r#"{"range":{"start":30,"end":60}}"#
    );
}
