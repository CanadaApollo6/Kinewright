use std::path::PathBuf;

use openreel_core::{
    AssetId, Clip, ClipId, Document, MediaAsset, MediaKind, OpError, Operation, Rational,
    TimeCode, Track, TrackId, TrackKind,
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
    ];

    for operation in operations {
        let encoded = serde_json::to_string(&operation).unwrap();
        let decoded: Operation = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, operation);
    }
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

