//! The previous document-bearing shape remains an independent test oracle.
use kinewright_core::{Command, Core, Document, Event, HistorySnapshot, Operation, TrackId};
use serde::Serialize;

#[derive(Serialize)]
struct BoundaryOracle<'a> {
    floor_offset: u64,
    baseline: &'a Document,
    document: &'a Document,
    batches: &'a [kinewright_core::HistoryBatch],
    cursor: &'a kinewright_core::HistoryCursor,
}
fn oracle(history: &HistorySnapshot) -> serde_json::Value {
    let mut value = serde_json::to_value(BoundaryOracle {
        floor_offset: history.floor_offset,
        baseline: &history.baseline,
        document: &history.document,
        batches: &history.batches,
        cursor: &history.cursor,
    })
    .unwrap();
    // The old format also stored every surviving clip ID. Reconstruct that
    // redundant field independently for the test-only legacy value.
    let mut current: std::collections::BTreeSet<_> = history
        .baseline
        .tracks
        .iter()
        .flat_map(|track| track.clips.iter().map(|clip| clip.id))
        .collect();
    let mut undo = Vec::new();
    let mut redo = Vec::new();
    for (batch, encoded) in history
        .batches
        .iter()
        .zip(value["batches"].as_array_mut().unwrap())
    {
        match &batch.command {
            kinewright_core::JournalCommand::Undo => {
                if let Some(previous) = undo.pop() {
                    redo.push(current);
                    current = previous;
                }
            }
            kinewright_core::JournalCommand::Redo => {
                if let Some(next) = redo.pop() {
                    undo.push(current);
                    current = next;
                }
            }
            _ => {
                let before = current.clone();
                for (effect, encoded) in batch
                    .identity_effects
                    .iter()
                    .zip(encoded["identity_effects"].as_array_mut().unwrap())
                {
                    let retained: Vec<_> = current
                        .iter()
                        .filter(|id| !effect.deleted.contains(id))
                        .copied()
                        .collect();
                    encoded["retained"] = serde_json::to_value(retained).unwrap();
                    for id in &effect.deleted {
                        current.remove(id);
                    }
                    current.extend(&effect.created);
                }
                if batch.undo_entries > undo.len() {
                    undo.push(before);
                }
                redo.clear();
            }
        }
    }
    value
}

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
        let mut bad = serde_json::to_value(&original).unwrap();
        bad["journal"]["document_sha256"] = serde_json::Value::String("0".repeat(64));
        assert!(serde_json::from_value::<HistorySnapshot>(bad).is_err());
        for index in 0..original.cursor.undo.len() {
            let mut bad = serde_json::to_value(&original).unwrap();
            bad["journal"]["cursor"]["undo"][index] = serde_json::Value::String("0".repeat(64));
            assert!(serde_json::from_value::<HistorySnapshot>(bad).is_err());
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
