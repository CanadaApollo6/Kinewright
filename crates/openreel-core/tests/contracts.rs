use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use openreel_core::{
    AssetId, Clip, ClipId, Command, Core, Document, Effect, EffectId, Event, MediaAsset,
    MediaKind, OpError, Operation, ParamValue, Rational, TimeCode, Track, TrackId, TrackKind,
    Transition,
};

fn asset(id: u64, fps: Rational, duration: i64) -> MediaAsset {
    MediaAsset {
        id: AssetId(id),
        path: PathBuf::from(format!("asset-{id}.mp4")),
        name: format!("asset-{id}"),
        duration: TimeCode(duration),
        fps,
        kind: MediaKind::Video,
        resolution: Some((1_920, 1_080)),
    }
}

fn empty_timeline(fps: Rational) -> Document {
    Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            clips: Vec::new(),
        }],
        media_pool: Vec::new(),
        fps,
        resolution: (1_920, 1_080),
        duration: TimeCode::ZERO,
    }
}

fn document_with_one_clip() -> Document {
    let fps = Rational::new(30, 1).unwrap();
    let mut doc = empty_timeline(fps);
    Operation::AddAsset {
        asset: asset(1, fps, 300),
    }
    .apply(&mut doc)
    .unwrap();
    Operation::AddClip {
        track: TrackId(1),
        asset: AssetId(1),
        at: TimeCode(10),
        source: TimeCode(0)..TimeCode(30),
    }
    .apply(&mut doc)
    .unwrap();
    doc
}

#[test]
fn document_and_every_operation_variant_round_trip_through_json() {
    let doc = document_with_one_clip();
    let encoded = serde_json::to_string(&doc).unwrap();
    let decoded: Document = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, doc);

    let operations = vec![
        Operation::AddAsset {
            asset: asset(2, Rational::new(24_000, 1_001).unwrap(), 240),
        },
        Operation::AddTrack {
            track: Track {
                id: TrackId(2),
                kind: TrackKind::Audio,
                clips: Vec::new(),
            },
        },
        Operation::RemoveTrack { track: TrackId(2) },
        Operation::AddClip {
            track: TrackId(1),
            asset: AssetId(1),
            at: TimeCode(100),
            source: TimeCode(5)..TimeCode(25),
        },
        Operation::SplitClip {
            clip: ClipId(1),
            at: TimeCode(25),
        },
        Operation::TrimClip {
            clip: ClipId(1),
            new_source: TimeCode(1)..TimeCode(20),
        },
        Operation::MoveClip {
            clip: ClipId(1),
            to_track: TrackId(1),
            to: TimeCode(60),
        },
        Operation::DeleteClip { clip: ClipId(1) },
        Operation::AddEffect {
            clip: ClipId(1),
            effect: Effect {
                id: EffectId(1),
                name: "brightness".to_owned(),
                parameters: BTreeMap::new(),
            },
        },
        Operation::RemoveEffect {
            clip: ClipId(1),
            effect: EffectId(1),
        },
        Operation::SetEffectParam {
            clip: ClipId(1),
            effect: EffectId(1),
            name: "percent".to_owned(),
            value: ParamValue::Integer(25),
        },
        Operation::AddTransition {
            clip: ClipId(1),
            transition: Transition {
                name: "crossfade".to_owned(),
                duration: TimeCode(10),
            },
        },
        Operation::RemoveTransition { clip: ClipId(1) },
    ];

    for operation in operations {
        let encoded = serde_json::to_string(&operation).unwrap();
        let decoded: Operation = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, operation);
    }
}

#[test]
fn effect_operations_validate_names_ids_parameters_and_are_atomic() {
    let mut doc = document_with_one_clip();
    let effect = Effect {
        id: EffectId(7),
        name: "brightness".to_owned(),
        parameters: BTreeMap::new(),
    };
    Operation::AddEffect {
        clip: ClipId(1),
        effect: effect.clone(),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(doc.clip(ClipId(1)).unwrap().effects, vec![effect]);

    let before_duplicate = doc.clone();
    assert_eq!(
        Operation::AddEffect {
            clip: ClipId(1),
            effect: Effect {
                id: EffectId(7),
                name: "opacity".to_owned(),
                parameters: BTreeMap::new(),
            },
        }
        .apply(&mut doc),
        Err(OpError::DuplicateEffect {
            clip: ClipId(1),
            effect: EffectId(7),
        })
    );
    assert_eq!(doc, before_duplicate);

    Operation::SetEffectParam {
        clip: ClipId(1),
        effect: EffectId(7),
        name: "percent".to_owned(),
        value: ParamValue::Integer(40),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(
        doc.clip(ClipId(1)).unwrap().effects[0]
            .parameters
            .get("percent"),
        Some(&ParamValue::Integer(40))
    );

    let before_invalid = doc.clone();
    assert_eq!(
        Operation::SetEffectParam {
            clip: ClipId(1),
            effect: EffectId(7),
            name: "percent".to_owned(),
            value: ParamValue::Integer(101),
        }
        .apply(&mut doc),
        Err(OpError::EffectParamOutOfRange {
            effect: "brightness".to_owned(),
            name: "percent".to_owned(),
            min: -100,
            max: 100,
            actual: 101,
        })
    );
    assert_eq!(doc, before_invalid);

    Operation::RemoveEffect {
        clip: ClipId(1),
        effect: EffectId(7),
    }
    .apply(&mut doc)
    .unwrap();
    assert!(doc.clip(ClipId(1)).unwrap().effects.is_empty());
    assert_eq!(
        Operation::AddEffect {
            clip: ClipId(1),
            effect: Effect {
                id: EffectId(8),
                name: "blur".to_owned(),
                parameters: BTreeMap::new(),
            },
        }
        .apply(&mut doc),
        Err(OpError::UnknownEffect("blur".to_owned()))
    );
}

#[test]
fn transition_operations_validate_crossfade_duration_and_are_atomic() {
    let mut doc = document_with_one_clip();
    Operation::AddTransition {
        clip: ClipId(1),
        transition: Transition {
            name: "crossfade".to_owned(),
            duration: TimeCode(12),
        },
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(
        doc.clip(ClipId(1)).unwrap().transition_in,
        Some(Transition {
            name: "crossfade".to_owned(),
            duration: TimeCode(12),
        })
    );

    let before_duplicate = doc.clone();
    assert_eq!(
        Operation::AddTransition {
            clip: ClipId(1),
            transition: Transition {
                name: "crossfade".to_owned(),
                duration: TimeCode(4),
            },
        }
        .apply(&mut doc),
        Err(OpError::DuplicateTransition(ClipId(1)))
    );
    assert_eq!(doc, before_duplicate);

    Operation::RemoveTransition { clip: ClipId(1) }
        .apply(&mut doc)
        .unwrap();
    assert_eq!(
        Operation::AddTransition {
            clip: ClipId(1),
            transition: Transition {
                name: "crossfade".to_owned(),
                duration: TimeCode(31),
            },
        }
        .apply(&mut doc),
        Err(OpError::TransitionTooLong {
            clip: ClipId(1),
            duration: TimeCode(31),
            clip_duration: TimeCode(30),
        })
    );
}

#[test]
fn add_and_remove_track_are_validated_and_atomic() {
    let mut doc = Document::default();
    let video = Track {
        id: TrackId(1),
        kind: TrackKind::Video,
        clips: Vec::new(),
    };
    Operation::AddTrack {
        track: video.clone(),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(doc.tracks, vec![video.clone()]);

    let before_duplicate = doc.clone();
    assert_eq!(
        Operation::AddTrack {
            track: video.clone(),
        }
        .apply(&mut doc),
        Err(OpError::DuplicateTrack(TrackId(1)))
    );
    assert_eq!(doc, before_duplicate);

    let non_empty = Track {
        id: TrackId(2),
        kind: TrackKind::Video,
        clips: vec![Clip {
            id: ClipId(99),
            asset: AssetId(99),
            source_range: TimeCode(0)..TimeCode(1),
            timeline_start: TimeCode::ZERO,
            effects: Vec::new(),
            transition_in: None,
        }],
    };
    assert_eq!(
        Operation::AddTrack { track: non_empty }.apply(&mut doc),
        Err(OpError::NewTrackNotEmpty(TrackId(2)))
    );

    Operation::RemoveTrack { track: TrackId(1) }
        .apply(&mut doc)
        .unwrap();
    assert!(doc.tracks.is_empty());
    assert_eq!(
        Operation::RemoveTrack { track: TrackId(404) }.apply(&mut doc),
        Err(OpError::MissingTrack(TrackId(404)))
    );
}

#[test]
fn remove_track_cascades_clips_and_recomputes_duration() {
    let mut doc = document_with_one_clip();
    Operation::RemoveTrack { track: TrackId(1) }
        .apply(&mut doc)
        .unwrap();
    assert!(doc.tracks.is_empty());
    assert_eq!(doc.duration, TimeCode::ZERO);
    assert_eq!(doc.media_pool.len(), 1);
}

#[test]
fn left_edge_trim_moves_the_timeline_start_as_one_atomic_edit() {
    let mut doc = document_with_one_clip();
    Operation::TrimClip {
        clip: ClipId(1),
        new_source: TimeCode(5)..TimeCode(30),
    }
    .apply(&mut doc)
    .unwrap();

    assert_eq!(doc.tracks[0].clips[0].timeline_start, TimeCode(15));
    assert_eq!(doc.tracks[0].clips[0].source_range, TimeCode(5)..TimeCode(30));
    assert_eq!(doc.duration, TimeCode(40));
}

#[test]
fn project_json_round_trip_preserves_exact_document_equality() {
    let doc = document_with_one_clip();
    let json = serde_json::to_string_pretty(&doc).unwrap();
    let loaded: Document = serde_json::from_str(&json).unwrap();
    loaded.validate().unwrap();
    assert_eq!(loaded, doc);
}

#[test]
fn public_actor_builds_undoes_redoes_saves_and_reopens_a_rough_cut() {
    let core = Core::spawn(Document::default()).unwrap();
    let events = core.subscribe().unwrap();
    let _initial = events.recv_timeout(Duration::from_secs(1)).unwrap();
    let fps = Rational::new(30, 1).unwrap();
    let operations = [
        Operation::AddTrack {
            track: Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                clips: Vec::new(),
            },
        },
        Operation::AddAsset {
            asset: asset(1, fps, 120),
        },
        Operation::AddAsset {
            asset: asset(2, fps, 120),
        },
        Operation::AddClip {
            track: TrackId(1),
            asset: AssetId(1),
            at: TimeCode(0),
            source: TimeCode(0)..TimeCode(120),
        },
        Operation::AddClip {
            track: TrackId(1),
            asset: AssetId(2),
            at: TimeCode(120),
            source: TimeCode(0)..TimeCode(120),
        },
        Operation::SplitClip {
            clip: ClipId(1),
            at: TimeCode(30),
        },
        Operation::TrimClip {
            clip: ClipId(1),
            new_source: TimeCode(5)..TimeCode(30),
        },
        Operation::MoveClip {
            clip: ClipId(2),
            to_track: TrackId(1),
            to: TimeCode(130),
        },
        Operation::DeleteClip { clip: ClipId(3) },
    ];

    let mut latest = None;
    for operation in operations {
        core.send(Command::Do(operation)).unwrap();
        let Event::DocumentChanged { doc, .. } =
            events.recv_timeout(Duration::from_secs(1)).unwrap()
        else {
            panic!("expected accepted edit");
        };
        latest = Some(doc);
    }
    core.send(Command::Undo).unwrap();
    let Event::DocumentChanged { doc: restored, .. } =
        events.recv_timeout(Duration::from_secs(1)).unwrap()
    else {
        panic!("expected undo snapshot");
    };
    assert!(restored.clip(ClipId(3)).is_some());
    core.send(Command::Redo).unwrap();
    let Event::DocumentChanged { doc: redone, .. } =
        events.recv_timeout(Duration::from_secs(1)).unwrap()
    else {
        panic!("expected redo snapshot");
    };
    assert_eq!(redone, latest.unwrap());
    redone.validate().unwrap();

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("openreel-m2-{nonce}.openreel"));
    fs::write(&path, serde_json::to_string_pretty(&*redone).unwrap()).unwrap();
    let reopened: Document = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let _ = fs::remove_file(path);
    reopened.validate().unwrap();
    assert_eq!(reopened, *redone);
}

#[test]
fn missing_asset_is_rejected_atomically() {
    let fps = Rational::new(30, 1).unwrap();
    let mut doc = empty_timeline(fps);
    let before = doc.clone();
    let error = Operation::AddClip {
        track: TrackId(1),
        asset: AssetId(404),
        at: TimeCode(0),
        source: TimeCode(0)..TimeCode(10),
    }
    .apply(&mut doc)
    .unwrap_err();

    assert_eq!(error, OpError::MissingAsset(AssetId(404)));
    assert_eq!(doc, before);
}

#[test]
fn overlapping_clip_is_rejected_atomically() {
    let mut doc = document_with_one_clip();
    let before = doc.clone();
    let error = Operation::AddClip {
        track: TrackId(1),
        asset: AssetId(1),
        at: TimeCode(20),
        source: TimeCode(0)..TimeCode(30),
    }
    .apply(&mut doc)
    .unwrap_err();

    assert!(matches!(error, OpError::ClipOverlap { .. }));
    assert_eq!(doc, before);
}

#[test]
fn unsorted_input_document_is_rejected() {
    let fps = Rational::new(30, 1).unwrap();
    let media = asset(1, fps, 300);
    let later = Clip {
        id: ClipId(1),
        asset: AssetId(1),
        source_range: TimeCode(0)..TimeCode(10),
        timeline_start: TimeCode(20),
        effects: Vec::new(),
        transition_in: None,
    };
    let earlier = Clip {
        id: ClipId(2),
        timeline_start: TimeCode(0),
        ..later.clone()
    };
    let doc = Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            clips: vec![later, earlier],
        }],
        media_pool: vec![media],
        fps,
        resolution: (1_920, 1_080),
        duration: TimeCode(30),
    };

    assert!(matches!(doc.validate(), Err(OpError::ClipsUnsorted { .. })));
}

#[test]
fn out_of_bounds_trim_is_rejected_atomically() {
    let mut doc = document_with_one_clip();
    let before = doc.clone();
    let error = Operation::TrimClip {
        clip: ClipId(1),
        new_source: TimeCode(0)..TimeCode(301),
    }
    .apply(&mut doc)
    .unwrap_err();

    assert!(matches!(error, OpError::SourceOutOfBounds { .. }));
    assert_eq!(doc, before);
}

#[test]
fn mixed_rate_split_preserves_the_clip_boundary_and_total_duration() {
    let source_fps = Rational::new(24_000, 1_001).unwrap();
    let project_fps = Rational::new(30, 1).unwrap();
    let mut doc = empty_timeline(project_fps);
    Operation::AddAsset {
        asset: asset(1, source_fps, 48),
    }
    .apply(&mut doc)
    .unwrap();
    Operation::AddClip {
        track: TrackId(1),
        asset: AssetId(1),
        at: TimeCode(0),
        source: TimeCode(1)..TimeCode(48),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(doc.duration, TimeCode(59));

    Operation::SplitClip {
        clip: ClipId(1),
        at: TimeCode(29),
    }
    .apply(&mut doc)
    .unwrap();

    assert_eq!(doc.tracks[0].clips.len(), 2);
    assert_eq!(doc.tracks[0].clips[0].source_range.end, TimeCode(24));
    assert_eq!(doc.tracks[0].clips[1].source_range.start, TimeCode(24));
    assert_eq!(doc.tracks[0].clips[1].timeline_start, TimeCode(29));
    assert_eq!(doc.duration, TimeCode(59));
    doc.validate().unwrap();
}

#[test]
fn mixed_rate_split_rejects_a_non_source_frame_boundary() {
    let source_fps = Rational::new(24_000, 1_001).unwrap();
    let project_fps = Rational::new(30, 1).unwrap();
    let mut doc = empty_timeline(project_fps);
    Operation::AddAsset {
        asset: asset(1, source_fps, 48),
    }
    .apply(&mut doc)
    .unwrap();
    Operation::AddClip {
        track: TrackId(1),
        asset: AssetId(1),
        at: TimeCode(0),
        source: TimeCode(0)..TimeCode(48),
    }
    .apply(&mut doc)
    .unwrap();
    let before = doc.clone();

    let error = Operation::SplitClip {
        clip: ClipId(1),
        at: TimeCode(2),
    }
    .apply(&mut doc)
    .unwrap_err();

    assert_eq!(
        error,
        OpError::UnrepresentableSplit {
            clip: ClipId(1),
            at: TimeCode(2),
        }
    );
    assert_eq!(doc, before);
}

