//! The previous document-bearing shape remains an independent test oracle.
use kinewright_core::{Command, Core, Document, Event, HistorySnapshot, Operation, TrackId};
#[path = "support/history_oracle.rs"]
mod history_oracle;
use history_oracle::document_oracle as oracle;

fn fixture(clips: usize) -> Document {
    let mut document = Document::default();
    for track in 1..=clips / 10 {
        document.tracks.push(kinewright_core::Track {
            id: TrackId(track as u64),
            kind: kinewright_core::TrackKind::Video,
            sync_lock: true,
            clips: (0..10)
                .map(|clip| {
                    serde_json::from_value(serde_json::json!({
                        "id": track * 100 + clip + 1, "asset": 0,
                        "source_range": {"start": 0, "end": 90},
                        "content": {"solid": {"r": 0, "g": 0, "b": 0}},
                        "timeline_start": clip * 90, "effects": [], "transition_in": null
                    }))
                    .unwrap()
                })
                .collect(),
        });
    }
    document.duration = kinewright_core::TimeCode(900);
    document.validate().unwrap();
    document
}

#[test]
fn history_compact_restore_matches_boundary_oracle() {
    let core = Core::spawn(fixture(400)).unwrap();
    for edit in 0..200 {
        assert!(matches!(
            core.request(Command::DoBatch(vec![Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: edit % 2 == 0,
            }]))
            .unwrap(),
            Event::DocumentChanged { .. }
        ));
    }
    let history = core.export_history().unwrap();
    let expected = oracle(&history);
    let bytes = serde_json::to_vec(&history).unwrap();
    let restored: HistorySnapshot = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(oracle(&restored), expected);
    assert_eq!(restored, history);
    assert_eq!(restored.applied_operations(), history.applied_operations());
}

#[test]
fn history_compact_gestures_empty_steps_and_abandoned_redo_match_oracle() {
    let core = Core::spawn(fixture(40)).unwrap();
    let flag = |locked| Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked,
    };
    let gesture = |locked| Command::DoBatchCoalesced {
        operations: vec![flag(locked)],
        coalesce_key: "drag".into(),
    };
    for command in [
        Command::Undo,
        Command::Redo,
        Command::Do(flag(false)),
        gesture(true),
        gesture(false),
        Command::Undo,
        Command::Redo,
        gesture(true),
        Command::Undo,
        Command::DoBatch(vec![flag(false)]),
    ] {
        assert!(matches!(
            core.request(command).unwrap(),
            Event::DocumentChanged { .. }
        ));
        let original = core.export_history().unwrap();
        let wire = serde_json::to_vec(&original).unwrap();
        let restored: HistorySnapshot = serde_json::from_slice(&wire).unwrap();
        assert_eq!(oracle(&restored), oracle(&original));
        assert_eq!(restored, original);
        let replay = Core::spawn(Document::default()).unwrap();
        replay.restore_history(restored.clone()).unwrap();
        let shared = replay.export_history().unwrap();
        for (expected, actual) in restored
            .cursor
            .undo
            .iter()
            .zip(&shared.cursor.undo)
            .chain(restored.cursor.redo.iter().zip(&shared.cursor.redo))
        {
            assert!(
                std::sync::Arc::ptr_eq(&expected.document, &actual.document),
                "VERIFIED_BOUNDARIES_SHARE_STORAGE"
            );
        }
        for field in ["baseline_sha256", "document_sha256"] {
            let mut bad = serde_json::to_value(&original).unwrap();
            bad["journal"][field] = serde_json::Value::String("0".repeat(64));
            assert!(serde_json::from_value::<HistorySnapshot>(bad).is_err());
        }
        for (direction, entries) in [
            ("undo", &original.cursor.undo),
            ("redo", &original.cursor.redo),
        ] {
            for index in 0..entries.len() {
                let mut bad = serde_json::to_value(&original).unwrap();
                bad["journal"]["cursor"][direction][index] =
                    serde_json::Value::String("0".repeat(64));
                assert!(serde_json::from_value::<HistorySnapshot>(bad).is_err());
            }
        }
    }
    let before = core.export_history().unwrap();
    let floored = kinewright_core::HistoryTransition::floor(before, 1)
        .unwrap()
        .into_after();
    let restored: HistorySnapshot =
        serde_json::from_slice(&serde_json::to_vec(&floored).unwrap()).unwrap();
    assert_eq!(oracle(&restored), oracle(&floored));
}

#[test]
fn history_transport_refuses_replay_amplification_before_execution() {
    let document = std::sync::Arc::new(fixture(1200));
    let hash = kinewright_core::history_document_hash(&document).unwrap();
    let batches = (0..4096)
        .map(|order| kinewright_core::JournalBatch {
            order,
            command: kinewright_core::JournalCommand::DoBatch(vec![Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: order % 2 == 0,
            }]),
            identity_effects: vec![],
            undo_entries: usize::try_from(order + 1).unwrap(),
            redo_entries: 0,
        })
        .collect();
    let transport = kinewright_core::HistoryTransport {
        baseline: std::sync::Arc::clone(&document),
        document,
        journal: kinewright_core::HistoryJournal {
            floor_offset: 0,
            baseline_sha256: hash.clone(),
            document_sha256: hash,
            batches,
            cursor: kinewright_core::JournalCursor {
                undo: vec![],
                redo: vec![],
            },
        },
    };
    // Invalid effects would be Corrupt if replay began; capacity wins first.
    assert_eq!(
        transport.resolve(),
        Err(kinewright_core::HistoryError::Capacity)
    );
    let bytes = serde_json::to_vec(&transport).unwrap();
    assert!(
        serde_json::from_slice::<HistorySnapshot>(&bytes)
            .unwrap_err()
            .to_string()
            .contains("bounded admission")
    );
}

#[test]
fn replay_reservation_covers_reallocating_clip_arrays() {
    let mut document = fixture(400);
    let mut clips = document
        .tracks
        .drain(..)
        .flat_map(|track| track.clips)
        .collect::<Vec<_>>()
        .into_iter();
    for track in 1..=2 {
        let mut values: Vec<_> = clips.by_ref().take(200).collect();
        for (index, clip) in values.iter_mut().enumerate() {
            clip.timeline_start = kinewright_core::TimeCode(i64::try_from(index).unwrap() * 90);
        }
        document.tracks.push(kinewright_core::Track {
            id: TrackId(track),
            kind: kinewright_core::TrackKind::Video,
            sync_lock: true,
            clips: values,
        });
    }
    document.duration = kinewright_core::TimeCode(18_000);
    let core = Core::spawn(document).unwrap();
    for index in 0..20 {
        let (track, at) = if index % 2 == 0 { (2, 18_000) } else { (1, 0) };
        assert!(matches!(
            core.request(Command::Do(Operation::MoveClip {
                clip: kinewright_core::ClipId(101),
                to_track: TrackId(track),
                to: kinewright_core::TimeCode(at)
            }))
            .unwrap(),
            Event::DocumentChanged { .. }
        ));
    }
    let history = core.export_history().unwrap();
    let bytes = serde_json::to_vec(&history).unwrap();
    let transport: kinewright_core::HistoryTransport = serde_json::from_slice(&bytes).unwrap();
    let bound = kinewright_core::history_size::replay_live_bound(
        &transport.journal,
        &transport.baseline,
        &transport.document,
    )
    .unwrap();
    let restored = transport.resolve().unwrap();
    assert_eq!(restored, history);
    assert!(
        kinewright_core::history_size::history_live_bytes(&restored).unwrap() <= bound,
        "REPLAY_RESERVATION_COVERS_VEC_GROWTH"
    );
}

#[test]
fn small_vector_transport_growth_is_reserved_before_replay() {
    let core = Core::spawn(Document {
        tracks: vec![kinewright_core::Track {
            id: TrackId(1),
            kind: kinewright_core::TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        markers: vec![kinewright_core::Marker {
            id: kinewright_core::MarkerId(1),
            position: kinewright_core::TimeCode(0),
            label: "single marker".into(),
            color_token: 0,
        }],
        ..Document::default()
    })
    .unwrap();
    for locked in [false, true, false] {
        core.request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked,
        }))
        .unwrap();
    }
    let original = core.export_history().unwrap();
    let wire: kinewright_core::HistoryTransport =
        serde_json::from_slice(&serde_json::to_vec(&original).unwrap()).unwrap();
    assert!(wire.baseline.tracks.capacity() > original.baseline.tracks.capacity());
    assert!(wire.baseline.markers.capacity() > original.baseline.markers.capacity());
    let charge = kinewright_core::history_size::replay_live_bound(
        &wire.journal,
        &original.baseline,
        &original.document,
    )
    .unwrap();
    let decoded_charge = kinewright_core::history_size::replay_live_bound(
        &wire.journal,
        &wire.baseline,
        &wire.document,
    )
    .unwrap();
    assert_eq!(
        charge, decoded_charge,
        "CAPTURE_MUST_RESERVE_THE_WIRE_ALLOCATION"
    );
    assert_eq!(wire.resolve().unwrap(), original);
}

#[test]
fn generated_command_prefixes_equal_document_bearing_oracle() {
    const SEED: u64 = 0x71e1_8a5e_f32a_b0ac;
    let mut random = SEED;
    for case in 0..32 {
        let source = Core::spawn(Document {
            tracks: vec![kinewright_core::Track {
                id: TrackId(1),
                kind: kinewright_core::TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            }],
            ..Document::default()
        })
        .unwrap();
        let mut resumed = Core::spawn(Document::default()).unwrap();
        resumed
            .restore_history(source.export_history().unwrap())
            .unwrap();
        for step in 0..40 {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            let operation = Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: random & 1 == 0,
            };
            let command = if step == 0 {
                Command::Undo
            } else if step == 1 {
                Command::Redo
            } else {
                match random % 6 {
                    0 => Command::Undo,
                    1 => Command::Redo,
                    2 => Command::DoBatch(vec![
                        operation,
                        Operation::SetTrackSyncLock {
                            track: TrackId(1),
                            locked: true,
                        },
                    ]),
                    3 | 4 => Command::DoBatchCoalesced {
                        operations: vec![operation],
                        coalesce_key: format!("gesture-{}", random % 3),
                    },
                    _ => Command::Do(operation),
                }
            };
            for actor in [&source, &resumed] {
                assert!(matches!(
                    actor.request(command.clone()).unwrap(),
                    Event::DocumentChanged { .. }
                ));
            }
            let expected = source.export_history().unwrap();
            assert_eq!(
                resumed.export_history().unwrap(),
                expected,
                "seed={SEED:#x} case={case} step={step}"
            );
            history_oracle::assert_compact_roundtrip(&expected);
            let encoded = serde_json::to_vec(&expected).unwrap();
            let restored: HistorySnapshot = serde_json::from_slice(&encoded).unwrap();
            resumed = Core::spawn(Document::default()).unwrap();
            resumed.restore_history(restored).unwrap();
        }
    }
}

#[test]
fn compact_applied_operations_contains_only_the_live_path() {
    let core = Core::spawn(Document {
        tracks: vec![kinewright_core::Track {
            id: TrackId(1),
            kind: kinewright_core::TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    })
    .unwrap();
    let a = Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked: false,
    };
    let marker = |id| Operation::AddMarker {
        marker: kinewright_core::Marker {
            id: kinewright_core::MarkerId(id),
            position: kinewright_core::TimeCode(0),
            label: format!("marker {id}"),
            color_token: 0,
        },
    };
    let c = marker(7);
    for command in [
        Command::Do(a.clone()),
        Command::Do(marker(3)),
        Command::Undo,
        Command::Do(c.clone()),
    ] {
        assert!(matches!(
            core.request(command).unwrap(),
            Event::DocumentChanged { .. }
        ));
    }
    let history = core.export_history().unwrap();
    assert_eq!(history.applied_operations(), [a, c]);
    assert_eq!(history.document.markers[0].id, kinewright_core::MarkerId(7));
    history_oracle::assert_compact_roundtrip(&history);
}

#[test]
fn compact_roundtrip_preserves_created_ids_when_an_undone_id_is_reused() {
    let core = Core::spawn(Document {
        tracks: vec![kinewright_core::Track {
            id: TrackId(2),
            kind: kinewright_core::TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    })
    .unwrap();
    let solid = |red| Operation::AddSolidClip {
        track: TrackId(2),
        timeline_start: kinewright_core::TimeCode(0),
        duration: kinewright_core::TimeCode(90),
        color: kinewright_core::SolidColor { r: red, g: 0, b: 0 },
    };
    for command in [
        Command::Do(solid(0)),
        Command::Undo,
        Command::Do(solid(255)),
    ] {
        let event = core.request(command).unwrap();
        assert!(matches!(event, Event::DocumentChanged { .. }), "{event:?}");
        history_oracle::assert_compact_roundtrip(&core.export_history().unwrap());
    }
    let original = core.export_history().unwrap();
    assert_eq!(
        original.batches[0].identity_effects[0].created,
        [kinewright_core::ClipId(1)]
    );
    assert_eq!(
        original.batches[2].identity_effects[0].created,
        [kinewright_core::ClipId(1)]
    );
    let restored: HistorySnapshot =
        serde_json::from_slice(&serde_json::to_vec(&original).unwrap()).unwrap();
    let reopened = Core::spawn(Document::default()).unwrap();
    reopened.restore_history(restored).unwrap();
    for command in [Command::Undo, Command::Redo] {
        for actor in [&core, &reopened] {
            assert!(matches!(
                actor.request(command.clone()).unwrap(),
                Event::DocumentChanged { .. }
            ));
        }
        assert_eq!(
            core.export_history().unwrap(),
            reopened.export_history().unwrap()
        );
    }
    assert!(matches!(
        &core.export_history().unwrap().document.tracks[0].clips[0].content,
        kinewright_core::ClipContent::Solid(color) if color.r == 255
    ));
}
