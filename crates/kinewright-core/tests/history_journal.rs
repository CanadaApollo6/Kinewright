//! The previous document-bearing shape remains an independent test oracle.
use kinewright_core::{Command, Core, Document, Event, HistorySnapshot, Operation, TrackId};
#[path = "support/history_oracle.rs"]
mod history_oracle;
use history_oracle::document_oracle as oracle;

fn history_effect(keys: i64) -> kinewright_core::Effect {
    serde_json::from_value(serde_json::json!({
        "id":1,"name":"primary_correction","enabled":true,"enabled_curve":null,
        "parameters":{"exposure_milli_stops":0},
        "keyframes":{"exposure_milli_stops":{"keyframes":(0..keys).map(|at| serde_json::json!({
            "at":at,"value":0,"interpolation":"linear","tangent_in":0,"tangent_out":0,
        })).collect::<Vec<_>>()}}
    }))
    .unwrap()
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
fn history_transport_refuses_before_exceeding_replay_window() {
    let document = std::sync::Arc::new(fixture(1200));
    let hash = kinewright_core::history_document_hash(&document).unwrap();
    let batches = (0..4096)
        .map(|order| kinewright_core::JournalBatch {
            order,
            command: kinewright_core::JournalCommand::DoBatch(vec![Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: order % 2 == 0,
            }]),
            identity_effects: vec![kinewright_core::JournalIdentityEffect {
                created: vec![],
                deleted: vec![],
                split_parent: None,
                split_results: vec![],
            }],
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
    // Identities are valid so replay reaches real document amplification.
    // It must stop within the reserved window, before reaching the intentionally
    // incomplete final cursor. Declared counts alone cannot authorize allocation.
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
    let bound = kinewright_core::history_size::measure_replay_peak(
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
    let charge = kinewright_core::history_size::measure_replay_peak(
        &wire.journal,
        &original.baseline,
        &original.document,
    )
    .unwrap();
    let decoded_charge = kinewright_core::history_size::measure_replay_peak(
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
        let source = Core::spawn(fixture(10)).unwrap();
        let effect = history_effect(10);
        assert!(matches!(
            source
                .request(Command::Do(Operation::AddEffect {
                    clip: kinewright_core::ClipId(101),
                    effect,
                }))
                .unwrap(),
            Event::DocumentChanged { .. }
        ));
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
            } else if step == 2 {
                Command::Do(Operation::CopyClipAttributes {
                    from_clip: kinewright_core::ClipId(101),
                    to_clip: kinewright_core::ClipId(102),
                    names: None,
                    include_keyframes: true,
                })
            } else if step == 3 {
                Command::Do(Operation::RemoveEffect {
                    clip: kinewright_core::ClipId(102),
                    effect: kinewright_core::EffectId(1),
                })
            } else if step == 4 {
                Command::Do(Operation::SplitClip {
                    clip: kinewright_core::ClipId(101),
                    at: kinewright_core::TimeCode(45),
                })
            } else if step == 5 {
                Command::Do(Operation::DeleteClip {
                    clip: kinewright_core::ClipId(111),
                })
            } else if step == 6 {
                Command::Undo
            } else if step == 7 {
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

#[test]
fn history_restore_charge_covers_replay_array_capacity() {
    fn check_charge(core: &Core, case: &str) {
        let captured = core.export_history().unwrap();
        let estimate =
            kinewright_core::history_size::restored_history_live_bytes(&captured).unwrap();
        let restored: HistorySnapshot =
            serde_json::from_slice(&serde_json::to_vec(&captured).unwrap()).unwrap();
        assert_eq!(restored, captured);
        let actual = kinewright_core::history_size::history_live_bytes(&restored).unwrap();
        assert!(
            estimate >= actual,
            "REPLAY_CAPACITIES_CHARGED case={case} estimate={estimate} actual={actual}"
        );
    }

    let core = Core::spawn(fixture(40)).unwrap();
    for edit in 0..203 {
        core.request(Command::DoBatch(vec![Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: edit % 2 == 0,
        }]))
        .unwrap();
    }
    for undo_count in [0, 190] {
        for _ in 0..undo_count {
            core.request(Command::Undo).unwrap();
        }
        check_charge(&core, &format!("undo-{undo_count}"));
    }
    for count in [20, 1] {
        core.request(Command::DoBatchCoalesced {
            operations: vec![
                Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: true
                };
                count
            ],
            coalesce_key: "capacity-growth".into(),
        })
        .unwrap();
    }
    check_charge(&core, "coalesced-arbitrary-start");
}

#[test]
fn history_transport_requires_its_current_typed_fields() {
    let core = Core::spawn(fixture(10)).unwrap();
    core.request(Command::DoBatch(vec![Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked: false,
    }]))
    .unwrap();
    let canonical = serde_json::to_value(core.export_history().unwrap()).unwrap();
    kinewright_core::history_journal_support_fields(&canonical["journal"]).unwrap();
    for path in [
        "",
        "/journal",
        "/journal/cursor",
        "/journal/batches/0",
        "/journal/batches/0/identity_effects/0",
        "/journal/batches/0/command",
    ] {
        let mut malformed = canonical.clone();
        malformed
            .pointer_mut(path)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unexpected".into(), serde_json::json!(true));
        assert!(
            serde_json::from_value::<HistorySnapshot>(malformed).is_err(),
            "UNKNOWN_HISTORY_FIELD path={path}"
        );
    }
    let mut missing = canonical;
    missing
        .pointer_mut("/journal/batches/0/identity_effects/0")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("split_parent")
        .unwrap();
    assert!(
        kinewright_core::history_journal_support_fields(&missing["journal"]).is_err(),
        "NULLABLE_IDENTITY_FIELD_IS_REQUIRED_BY_SCHEMA"
    );
    assert!(
        serde_json::from_value::<HistorySnapshot>(missing).is_err(),
        "NULLABLE_IDENTITY_FIELD_IS_REQUIRED"
    );
}

#[test]
fn history_replay_reservation_covers_implicit_splits_and_removals() {
    use kinewright_core::{AssetId, ClipId, MediaAsset, MediaKind, TimeCode};
    for patched in [false, true] {
        let mut document = fixture(10);
        document.tracks[0].clips.truncate(1);
        document.duration = TimeCode(90);
        document.media_pool.push(MediaAsset {
            id: AssetId(1),
            path: "replay-only.mp4".into(),
            name: "replay-only".into(),
            duration: TimeCode(10000),
            fps: document.fps,
            kind: MediaKind::Video,
            resolution: Some((1920, 1080)),
            source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
            color_description: kinewright_core::ColorDescription::default(),
            assumed_from: None,
        });
        let core = Core::spawn(document).unwrap();
        assert!(matches!(
            core.request(Command::Do(Operation::AddEffect {
                clip: ClipId(101),
                effect: history_effect(10),
            }))
            .unwrap(),
            Event::DocumentChanged { .. }
        ));
        for cut in 0..60 {
            let command = if patched {
                Operation::PatchedThreePointEdit {
                    asset: AssetId(1),
                    source_in: Some(TimeCode(0)),
                    source_out: Some(TimeCode(1)),
                    timeline_in: Some(TimeCode(cut * 2 + 1)),
                    timeline_out: None,
                    mode: kinewright_core::ThreePointMode::Insert,
                    video_track: Some(TrackId(1)),
                    audio_track: None,
                }
            } else {
                Operation::ThreePointEdit {
                    track: TrackId(1),
                    asset: AssetId(1),
                    source_in: Some(TimeCode(0)),
                    source_out: Some(TimeCode(1)),
                    timeline_in: Some(TimeCode(cut * 2 + 1)),
                    timeline_out: None,
                    mode: kinewright_core::ThreePointMode::Insert,
                }
            };
            let event = core.request(Command::Do(command)).unwrap();
            assert!(matches!(event, Event::DocumentChanged { .. }), "{event:?}");
        }
        let deletes = core.export_history().unwrap().document.tracks[0]
            .clips
            .iter()
            .filter(|clip| clip.id != ClipId(101))
            .map(|clip| Operation::DeleteClip { clip: clip.id })
            .collect();
        assert!(matches!(
            core.request(Command::DoBatch(deletes)).unwrap(),
            Event::DocumentChanged { .. }
        ));
        let history = core.export_history().unwrap();
        let journal = kinewright_core::HistoryJournal::capture(&history).unwrap();
        let reserved = kinewright_core::history_size::measure_replay_peak(
            &journal,
            &history.baseline,
            &history.document,
        )
        .unwrap();
        let retained =
            kinewright_core::history_size::restored_history_live_bytes(&history).unwrap();
        println!("IMPLICIT_SPLIT_REPLAY patched={patched} retained={retained} peak={reserved}");
        assert_eq!(
            journal.resolve_with_budget(
                std::sync::Arc::clone(&history.baseline),
                &history.document,
                retained
            ),
            Err(kinewright_core::HistoryError::Capacity),
            "IMPLICIT_SPLITS_NEED_TRANSIENT_ROOM_BEYOND_FINAL_RETAINED_STATE"
        );
        history_oracle::assert_compact_roundtrip(&history);
    }
}

fn oracle_operation_kind(operation: &Operation) -> &'static str {
    match operation {
        Operation::AddAsset { .. } => "AddAsset",
        Operation::RelinkAsset { .. } => "RelinkAsset",
        Operation::SetAssetColorDescription { .. } => "SetAssetColorDescription",
        Operation::SetColorContext { .. } => "SetColorContext",
        Operation::UpsertBin { .. } => "UpsertBin",
        Operation::RemoveBin { .. } => "RemoveBin",
        Operation::SetAssetBin { .. } => "SetAssetBin",
        Operation::UpsertStringOut { .. } => "UpsertStringOut",
        Operation::RemoveStringOut { .. } => "RemoveStringOut",
        Operation::UpsertSyncGroup { .. } => "UpsertSyncGroup",
        Operation::RemoveSyncGroup { .. } => "RemoveSyncGroup",
        Operation::UpsertAudioBus { .. } => "UpsertAudioBus",
        Operation::RemoveAudioBus { .. } => "RemoveAudioBus",
        Operation::SetAudioMaster { .. } => "SetAudioMaster",
        Operation::SetPanLaw { .. } => "SetPanLaw",
        Operation::AddTrack { .. } => "AddTrack",
        Operation::RemoveTrack { .. } => "RemoveTrack",
        Operation::SetTrackSyncLock { .. } => "SetTrackSyncLock",
        Operation::SetTrackMix { .. } => "SetTrackMix",
        Operation::SetTrackAutomation { .. } => "SetTrackAutomation",
        Operation::AddClip { .. } => "AddClip",
        Operation::AddTitle { .. } => "AddTitle",
        Operation::SplitClip { .. } => "SplitClip",
        Operation::TrimClip { .. } => "TrimClip",
        Operation::MoveClip { .. } => "MoveClip",
        Operation::ThreePointEdit { .. } => "ThreePointEdit",
        Operation::PatchedThreePointEdit { .. } => "PatchedThreePointEdit",
        Operation::SlipClip { .. } => "SlipClip",
        Operation::RollEdit { .. } => "RollEdit",
        Operation::SlideClip { .. } => "SlideClip",
        Operation::ReplaceClip { .. } => "ReplaceClip",
        Operation::FitToFill { .. } => "FitToFill",
        Operation::DeleteClip { .. } => "DeleteClip",
        Operation::RippleDeleteClip { .. } => "RippleDeleteClip",
        Operation::RippleInsertGap { .. } => "RippleInsertGap",
        Operation::LinkClips { .. } => "LinkClips",
        Operation::UnlinkClips { .. } => "UnlinkClips",
        Operation::AddMarker { .. } => "AddMarker",
        Operation::RemoveMarker { .. } => "RemoveMarker",
        Operation::MoveMarker { .. } => "MoveMarker",
        Operation::AddEffect { .. } => "AddEffect",
        Operation::InsertEffect { .. } => "InsertEffect",
        Operation::RemoveEffect { .. } => "RemoveEffect",
        Operation::SetEffectParam { .. } => "SetEffectParam",
        Operation::SetEffectKeyframes { .. } => "SetEffectKeyframes",
        Operation::ClearEffectKeyframes { .. } => "ClearEffectKeyframes",
        Operation::UpsertEffectKeyframe { .. } => "UpsertEffectKeyframe",
        Operation::RemoveEffectKeyframe { .. } => "RemoveEffectKeyframe",
        Operation::SetEffectEnabled { .. } => "SetEffectEnabled",
        Operation::SetClipEnabled { .. } => "SetClipEnabled",
        Operation::SetClipEnabledCurve { .. } => "SetClipEnabledCurve",
        Operation::CopyClipAttributes { .. } => "CopyClipAttributes",
        Operation::ConvertLegacyLook { .. } => "ConvertLegacyLook",
        Operation::AddLutAsset { .. } => "AddLutAsset",
        Operation::RemoveLutAsset { .. } => "RemoveLutAsset",
        Operation::SetTitleParam { .. } => "SetTitleParam",
        Operation::SetClipAudio { .. } => "SetClipAudio",
        Operation::SetClipGainEnvelope { .. } => "SetClipGainEnvelope",
        Operation::AddTransition { .. } => "AddTransition",
        Operation::RemoveTransition { .. } => "RemoveTransition",
        Operation::SetMarkerParam { .. } => "SetMarkerParam",
        Operation::AddFreezeFrame { .. } => "AddFreezeFrame",
        Operation::SetClipSpeed { .. } => "SetClipSpeed",
        Operation::SetClipBlendMode { .. } => "SetClipBlendMode",
        Operation::AddAdjustmentClip { .. } => "AddAdjustmentClip",
        Operation::AddSolidClip { .. } => "AddSolidClip",
        Operation::SetSolidColor { .. } => "SetSolidColor",
    }
}

#[test]
fn every_operation_kind_has_a_complete_history_oracle_case() {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Case {
        document: Document,
        operation: Operation,
    }
    let cases: std::collections::BTreeMap<String, Case> =
        serde_json::from_str(include_str!("support/history_operation_cases.json")).unwrap();
    let schema = serde_json::to_value(schemars::schema_for!(Operation)).unwrap();
    let expected: std::collections::BTreeSet<_> = schema["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|variant| variant["properties"].as_object().unwrap().keys().cloned())
        .collect();
    let mut hits = std::collections::BTreeSet::new();
    for (kind, mut case) in cases {
        assert_eq!(oracle_operation_kind(&case.operation), kind);
        history_oracle::ApplyHistoryOracle::apply_with_history_oracle(
            &case.operation,
            &mut case.document,
        )
        .unwrap();
        assert!(hits.insert(kind));
    }
    assert_eq!(hits, expected, "EVERY_OPERATION_REQUIRES_AN_ORACLE_HIT");
}

#[test]
fn history_replay_same_window_covers_cold_nested_vector_capacity() {
    let mut document = fixture(10);
    document.tracks[0].clips.truncate(1);
    document.duration = kinewright_core::TimeCode(90);
    document.tracks[0].clips[0].effects = (1..=129)
        .map(|id| kinewright_core::Effect {
            id: kinewright_core::EffectId(id),
            name: "opacity".into(),
            enabled: true,
            enabled_curve: None,
            parameters: std::collections::BTreeMap::from([(
                "percent".into(),
                kinewright_core::ParamValue::Integer(100),
            )]),
            keyframes: std::collections::BTreeMap::new(),
        })
        .collect();
    let core = Core::spawn(document).unwrap();
    core.request(Command::Do(Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked: false,
    }))
    .unwrap();
    let original = core.export_history().unwrap();
    let wire: kinewright_core::HistoryTransport =
        serde_json::from_slice(&serde_json::to_vec(&original).unwrap()).unwrap();
    assert!(
        wire.baseline.tracks[0].clips[0].effects.capacity()
            > original.baseline.tracks[0].clips[0].effects.capacity()
    );
    let charge = kinewright_core::history_size::measure_replay_peak(
        &wire.journal,
        &original.baseline,
        &original.document,
    )
    .unwrap();
    let (cold, cold_peak) = wire
        .journal
        .resolve_with_budget(
            std::sync::Arc::clone(&wire.baseline),
            &wire.document,
            charge,
        )
        .expect("THE_SAME_ADMISSION_WINDOW_MUST_OPEN_A_COLD_DECODE");
    assert_eq!(cold, original);
    assert!(cold_peak <= charge);
}

#[test]
fn history_replay_bounds_large_intermediate_copy_removed_in_the_same_batch() {
    use kinewright_core::{ClipId, EffectId, HistoryError};
    let mut document = fixture(10);
    document.tracks[0].clips[0].effects = vec![history_effect(1000)];
    let core = Core::spawn(document).unwrap();
    let mut operations: Vec<_> = (102..=110)
        .map(|id| Operation::CopyClipAttributes {
            from_clip: ClipId(101),
            to_clip: ClipId(id),
            names: None,
            include_keyframes: true,
        })
        .collect();
    operations.extend((102..=110).map(|id| Operation::RemoveEffect {
        clip: ClipId(id),
        effect: EffectId(1),
    }));
    assert!(matches!(
        core.request(Command::DoBatch(operations)).unwrap(),
        Event::DocumentChanged { .. }
    ));
    let original = core.export_history().unwrap();
    assert_eq!(
        original.document, original.baseline,
        "THE_FINAL_DOCUMENT_HIDES_THE_TEMPORARY_COPIES"
    );
    let wire: kinewright_core::HistoryTransport =
        serde_json::from_slice(&serde_json::to_vec(&original).unwrap()).unwrap();
    let window = kinewright_core::history_size::document_live_bytes(&wire.baseline).unwrap() * 8;
    assert!(
        kinewright_core::history_size::restored_history_live_bytes(&original).unwrap() < window
    );
    assert_eq!(
        wire.journal.resolve_with_budget(
            std::sync::Arc::clone(&wire.baseline),
            &wire.document,
            window
        ),
        Err(HistoryError::Capacity),
        "CHECK_THE_INTERMEDIATE_STATE_BEFORE_COPYING_AGAIN"
    );
    assert_eq!(wire.resolve().unwrap(), original);
    assert_eq!(core.export_history().unwrap(), original);
}

#[test]
fn history_replay_releases_discarded_coalesced_documents_inside_the_window() {
    let core = Core::spawn(fixture(400)).unwrap();
    for edit in 0..1000 {
        assert!(matches!(
            core.request(Command::DoBatchCoalesced {
                operations: vec![Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: edit % 2 == 0
                }],
                coalesce_key: "one-gesture".into(),
            })
            .unwrap(),
            Event::DocumentChanged { .. }
        ));
    }
    let original = core.export_history().unwrap();
    assert_eq!(original.cursor.undo.len(), 1);
    let window = 32 << 20;
    assert!(
        kinewright_core::history_size::restored_history_live_bytes(&original).unwrap() < window
    );
    let journal = kinewright_core::HistoryJournal::capture(&original).unwrap();
    let (restored, _) = journal
        .resolve_with_budget(
            std::sync::Arc::clone(&original.baseline),
            &original.document,
            window,
        )
        .expect("A_FINISHED_GESTURE_DOES_NOT_RETAIN_ONE_DOCUMENT_PER_MOUSE_EVENT");
    assert_eq!(restored, original);
    let reopened = Core::spawn(Document::default()).unwrap();
    reopened.restore_history(restored).unwrap();
    reopened.request(Command::Undo).unwrap();
    assert_eq!(
        reopened.export_history().unwrap().document,
        original.baseline
    );
    reopened.request(Command::Redo).unwrap();
    assert_eq!(
        reopened.export_history().unwrap().document,
        original.document
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one sequence compares every prefix against independent cold replay and both capacity thresholds"
)]
fn history_incremental_proof_equals_cold_peak_and_refusals_at_every_cut() {
    use kinewright_core::{HistoryError, HistoryJournal, Track, TrackKind};
    use std::sync::Arc;
    let core = Core::spawn(Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        ..Document::default()
    })
    .unwrap();
    let mut proof = None;
    let mut commands = vec![
        Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: false,
        }),
        Command::DoBatchCoalesced {
            operations: vec![Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: true,
            }],
            coalesce_key: "proof".into(),
        },
        Command::DoBatchCoalesced {
            operations: vec![Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: false,
            }],
            coalesce_key: "proof".into(),
        },
        Command::Undo,
        Command::Redo,
        Command::Undo,
    ];
    for index in 0..32 {
        commands.push(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: index % 2 == 0,
        }));
    }
    commands.extend([
        Command::Do(Operation::AddSolidClip {
            track: TrackId(1),
            timeline_start: kinewright_core::TimeCode(0),
            duration: kinewright_core::TimeCode(90),
            color: kinewright_core::SolidColor { r: 0, g: 0, b: 0 },
        }),
        Command::Do(Operation::AddSolidClip {
            track: TrackId(1),
            timeline_start: kinewright_core::TimeCode(90),
            duration: kinewright_core::TimeCode(90),
            color: kinewright_core::SolidColor { r: 0, g: 0, b: 0 },
        }),
        Command::Do(Operation::AddEffect {
            clip: kinewright_core::ClipId(1),
            effect: history_effect(10),
        }),
        Command::Do(Operation::CopyClipAttributes {
            from_clip: kinewright_core::ClipId(1),
            to_clip: kinewright_core::ClipId(2),
            names: None,
            include_keyframes: true,
        }),
        Command::Do(Operation::SplitClip {
            clip: kinewright_core::ClipId(2),
            at: kinewright_core::TimeCode(120),
        }),
        Command::DoBatch(vec![
            Operation::DeleteClip {
                clip: kinewright_core::ClipId(3),
            },
            Operation::DeleteClip {
                clip: kinewright_core::ClipId(2),
            },
        ]),
        Command::Undo,
        Command::Redo,
    ]);
    for command in commands {
        assert!(matches!(
            core.request(command).unwrap(),
            Event::DocumentChanged { .. }
        ));
        let history = core.export_history().unwrap();
        let journal = HistoryJournal::capture(&history).unwrap();
        let cold = journal
            .resolve_verified_with_budget(
                Arc::clone(&history.baseline),
                &history.document,
                128 << 20,
                None,
            )
            .unwrap();
        let wire: kinewright_core::HistoryTransport = serde_json::from_slice(
            &serde_json::to_vec(&kinewright_core::HistoryTransport::capture(&history).unwrap())
                .unwrap(),
        )
        .unwrap();
        let wire_proof = wire
            .journal
            .resolve_verified_with_budget(
                Arc::clone(&wire.baseline),
                &wire.document,
                cold.replay_peak(),
                None,
            )
            .unwrap();
        assert_eq!(
            wire_proof.replay_peak(),
            cold.replay_peak(),
            "WARM_AND_COLD_ADMISSION_HAVE_THE_SAME_WINDOW"
        );
        let incremental = journal
            .resolve_verified_with_budget(
                Arc::clone(&history.baseline),
                &history.document,
                128 << 20,
                proof.as_ref(),
            )
            .unwrap();
        assert_eq!(incremental.snapshot(), cold.snapshot());
        assert_eq!(
            incremental.replay_peak(),
            cold.replay_peak(),
            "PREFIX_PROOF_HAS_THE_SAME_PEAK_AS_COLD_REPLAY at {} batches",
            history.batches.len()
        );
        let just_too_small = cold.replay_peak() - 1;
        assert!(matches!(
            journal.resolve_verified_with_budget(
                Arc::clone(&history.baseline),
                &history.document,
                just_too_small,
                None
            ),
            Err(HistoryError::Capacity)
        ));
        assert!(matches!(
            journal.resolve_verified_with_budget(
                Arc::clone(&history.baseline),
                &history.document,
                just_too_small,
                proof.as_ref()
            ),
            Err(HistoryError::Capacity)
        ));
        let shared = incremental.share_documents(history.clone()).unwrap();
        assert!(Arc::ptr_eq(&shared.snapshot().document, &history.document));
        assert!(Arc::ptr_eq(&shared.snapshot().baseline, &history.baseline));
        for (proved, live) in shared
            .snapshot()
            .cursor
            .undo
            .iter()
            .chain(&shared.snapshot().cursor.redo)
            .zip(history.cursor.undo.iter().chain(&history.cursor.redo))
        {
            assert!(
                Arc::ptr_eq(&proved.document, &live.document),
                "PROOF_SHARES_EVERY_LIVE_BOUNDARY_DOCUMENT"
            );
        }
        proof = Some(shared);
    }
    let mut larger = core.export_history().unwrap();
    Arc::make_mut(&mut larger.document).tracks.reserve(10_000);
    let shared = proof
        .as_ref()
        .unwrap()
        .clone()
        .share_documents(larger.clone())
        .unwrap();
    assert!(
        !Arc::ptr_eq(&shared.snapshot().document, &larger.document),
        "EQUAL_BYTES_WITH_LARGER_SPARE_CAPACITY_CANNOT_REPLACE_A_PROVEN_ALLOCATION"
    );
    let mut forged = core.export_history().unwrap();
    forged.cursor.undo.last_mut().unwrap().coalesce_key = Some("forged".into());
    assert!(matches!(
        proof.unwrap().share_documents(forged),
        Err(HistoryError::StaleExpectation)
    ));
}

#[test]
fn history_replay_prior_mix_curves_and_relink_clip_copies_have_bounded_windows() {
    use kinewright_core::{
        AutomationCurve, HistoryError, HistoryTransport, Keyframe, KeyframeInterpolation,
        SolidColor, TimeCode, TrackKind,
    };
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("support/history_operation_cases.json")).unwrap();
    for name in ["RelinkAsset", "SetTrackMix", "SetTrackAutomation"] {
        let mut document: Document =
            serde_json::from_value(cases[name]["document"].clone()).unwrap();
        let operation: Operation =
            serde_json::from_value(cases[name]["operation"].clone()).unwrap();
        match &operation {
            Operation::RelinkAsset { .. } => {
                Operation::AddEffect {
                    clip: document.tracks[0].clips[0].id,
                    effect: {
                        let mut effect = history_effect(1000);
                        effect.id = kinewright_core::EffectId(1000);
                        effect
                    },
                }
                .apply(&mut document)
                .unwrap();
            }
            Operation::SetTrackMix { track, .. } | Operation::SetTrackAutomation { track, .. } => {
                let video = document
                    .tracks
                    .iter()
                    .find(|track| track.kind == TrackKind::Video)
                    .unwrap()
                    .id;
                Operation::AddSolidClip {
                    track: video,
                    timeline_start: document.duration,
                    duration: TimeCode(2000),
                    color: SolidColor { r: 0, g: 0, b: 0 },
                }
                .apply(&mut document)
                .unwrap();
                Operation::SetTrackAutomation {
                    track: *track,
                    parameter: "pan_percent".into(),
                    curve: Some(AutomationCurve {
                        keyframes: (0..1000)
                            .map(|at| Keyframe {
                                at: TimeCode(at),
                                value: 0,
                                interpolation: KeyframeInterpolation::Linear,
                                tangent_in: 0,
                                tangent_out: 0,
                            })
                            .collect(),
                    }),
                }
                .apply(&mut document)
                .unwrap();
            }
            _ => unreachable!("three prior-state-copy fixtures"),
        }
        let core = Core::spawn(document).unwrap();
        assert!(
            matches!(
                core.request(Command::Do(operation)).unwrap(),
                Event::DocumentChanged { .. }
            ),
            "{name}"
        );
        let history = core.export_history().unwrap();
        let transport = HistoryTransport::capture(&history).unwrap();
        let retained =
            kinewright_core::history_size::restored_history_live_bytes(&history).unwrap();
        assert!(
            matches!(
                transport.journal.resolve_with_budget(
                    std::sync::Arc::clone(&transport.baseline),
                    &transport.document,
                    retained
                ),
                Err(HistoryError::Capacity)
            ),
            "{name}: final retained size alone cannot reserve the prior-state copy peak"
        );
        let resolved = transport.resolve_verified().unwrap();
        assert_eq!(resolved.snapshot(), &history, "{name}");
        let restored = Core::spawn(Document::default()).unwrap();
        restored.restore_resolved_history(resolved).unwrap();
        restored.request(Command::Undo).unwrap();
        assert_eq!(
            restored.export_history().unwrap().document,
            history.baseline
        );
        restored.request(Command::Redo).unwrap();
        assert_eq!(
            restored.export_history().unwrap().document,
            history.document
        );
    }
}

#[test]
fn history_runtime_replay_evidence_is_not_input_authority_or_a_document_owner() {
    use kinewright_core::{HistoryError, HistoryTransport};
    use std::sync::Arc;
    let core = Core::spawn(fixture(10)).unwrap();
    core.request(Command::Do(Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked: false,
    }))
    .unwrap();
    let transport = HistoryTransport::capture(&core.export_history().unwrap()).unwrap();
    let decoded = transport.resolve().unwrap();
    assert!(decoded.replay_proof.is_some());
    let bytes = serde_json::to_value(&decoded).unwrap();
    assert!(bytes.get("replay_proof").is_none());
    let mut forged_wire = bytes;
    forged_wire["replay_proof"] = serde_json::json!({"trusted":true});
    assert!(
        serde_json::from_value::<HistorySnapshot>(forged_wire).is_err(),
        "WIRE_CANNOT_SUPPLY_REPLAY_EVIDENCE"
    );
    let restored = Core::spawn(Document::default()).unwrap();
    restored.restore_history(decoded.clone()).unwrap();
    assert_eq!(restored.export_history().unwrap(), decoded);
    let mut changed = decoded.clone();
    changed.cursor.undo.last_mut().unwrap().operations.clear();
    assert_eq!(
        restored.restore_history(changed),
        Err(HistoryError::Corrupt),
        "CHANGED_CURSOR_METADATA_CANNOT_REUSE_THE_EVIDENCE"
    );
    assert_eq!(restored.export_history().unwrap(), decoded);
    let independent = transport.resolve().unwrap();
    let evidence = independent.replay_proof.as_ref().unwrap().clone();
    let document = Arc::downgrade(&independent.document);
    drop(independent);
    assert!(
        document.upgrade().is_none(),
        "REPLAY_CARRIER_DOES_NOT_RETAIN_DOCUMENT_PAYLOADS"
    );
    drop(evidence);
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the same paths check intermediate peaks, allocation parity and residual reservations"
)]
fn history_redo_reservation_covers_every_cut_without_growing_on_redo() {
    use kinewright_core::{ClipId, EffectId, HistoryJournal};
    for shape in 0..4 {
        let core = Core::spawn(fixture(10)).unwrap();
        for index in 0..17 {
            let operation = if shape > 0 && index == 2 {
                Operation::AddEffect {
                    clip: ClipId(101),
                    effect: history_effect(if shape == 2 { 400 } else { 60 }),
                }
            } else if shape > 0 && index == 12 {
                Operation::RemoveEffect {
                    clip: ClipId(101),
                    effect: EffectId(1),
                }
            } else {
                Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: index % 2 == 0,
                }
            };
            assert!(matches!(
                core.request(Command::Do(operation)).unwrap(),
                Event::DocumentChanged { .. }
            ));
        }
        for _ in 0..(if shape == 2 { 16 } else { 9 }) {
            core.request(Command::Undo).unwrap();
        }
        let mut start = core.export_history().unwrap();
        if shape == 3 {
            let document = &mut start.cursor.redo.last_mut().unwrap().document;
            std::sync::Arc::make_mut(document)
                .tracks
                .reserve_exact(4000);
            core.restore_history(start.clone()).unwrap();
        }
        let resolve = |history: &HistorySnapshot| {
            HistoryJournal::capture(history)
                .unwrap()
                .resolve_verified_with_budget(
                    std::sync::Arc::clone(&history.baseline),
                    &history.document,
                    kinewright_core::history_size::TRANSPORT_LIVE_BYTES,
                    None,
                )
                .unwrap()
        };
        let proof = resolve(&start);
        let bound = proof
            .redo_capacity_bound(&HistoryJournal::capture(proof.snapshot()).unwrap(), &start)
            .unwrap();
        let bytes_before = serde_json::to_vec(&HistoryJournal::capture(&start).unwrap().batches)
            .unwrap()
            .len();
        let mut peak_during_return = 0;
        let mut final_peak = 0;
        for step in 0..start.cursor.redo.len() {
            assert!(matches!(
                core.request(Command::Redo).unwrap(),
                Event::DocumentChanged { .. }
            ));
            let now = core.export_history().unwrap();
            let proof = resolve(&now);
            final_peak = proof.replay_peak();
            peak_during_return = peak_during_return.max(final_peak);
            assert!(
                proof.replay_peak() <= bound.replay_bytes,
                "REDO_PEAK_BOUND shape={shape} step={step}"
            );
            assert!(
                kinewright_core::history_size::restored_history_live_bytes(proof.snapshot())
                    .unwrap()
                    <= bound.retained_bytes,
                "REDO_RETAINED_BOUND"
            );
            if now.cursor.redo.is_empty() {
                let bytes_after =
                    serde_json::to_vec(&HistoryJournal::capture(&now).unwrap().batches)
                        .unwrap()
                        .len();
                assert_eq!(
                    (bytes_after - bytes_before) as u64,
                    bound.encoded_batch_bytes,
                    "REDO_BATCH_CLOSED_FORM_IS_EXACT"
                );
            } else {
                let remaining = proof
                    .redo_capacity_bound(&HistoryJournal::capture(proof.snapshot()).unwrap(), &now)
                    .unwrap();
                assert!(
                    remaining.replay_bytes <= bound.replay_bytes,
                    "REDO_RESERVATION_MUST_NOT_GROW shape={shape} step={step}: {remaining:?} > {bound:?}"
                );
                assert!(
                    remaining.retained_bytes <= bound.retained_bytes,
                    "REDO_RETAINED_RESERVATION_MUST_NOT_GROW"
                );
            }
        }
        if shape == 2 {
            assert!(
                peak_during_return > final_peak,
                "INTERMEDIATE_REDO_IS_MORE_EXPENSIVE_THAN_THE_END"
            );
            println!(
                "REDO_INTERMEDIATE_PEAK maximum={peak_during_return} final={final_peak} reserved={}",
                bound.replay_bytes
            );
        }
    }
}

#[test]
fn history_canonical_admission_ignores_a_foreign_allocation_prefix() {
    let core = Core::spawn(kinewright_core::Document {
        tracks: vec![kinewright_core::Track {
            id: TrackId(1),
            kind: kinewright_core::TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
        ..kinewright_core::Document::default()
    })
    .unwrap();
    core.request(Command::Do(Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked: false,
    }))
    .unwrap();
    let history = core.export_history().unwrap();
    let journal = kinewright_core::HistoryJournal::capture(&history).unwrap();
    let mut enlarged = (*history.baseline).clone();
    enlarged.tracks.reserve(100_000);
    let canonical = journal
        .resolve_canonical_with_budget(&enlarged, &history.document, 32 << 20, None)
        .unwrap();
    let holder = journal
        .resolve_verified_with_budget(
            std::sync::Arc::new(enlarged),
            &history.document,
            32 << 20,
            None,
        )
        .unwrap();
    assert!(holder.replay_peak() > canonical.replay_peak());
    let with_foreign = journal
        .resolve_canonical_with_budget(
            &history.baseline,
            &history.document,
            canonical.replay_peak(),
            Some(&holder),
        )
        .unwrap();
    assert_eq!(with_foreign.snapshot(), canonical.snapshot());
    assert_eq!(
        with_foreign.replay_peak(),
        canonical.replay_peak(),
        "CANONICAL_PREFIX_PROVENANCE"
    );
}
