//! MO2 Part A — core model contract tests (blend mode, adjustment and solid
//! content, the 12 geometric transition descriptors, span integrity,
//! survival, the adjustment support table, and the four operations).

use kinewright_core::{
    AssetId, AudioMix, BlendMode, Clip, ClipContent, ClipId, ColorContext, Document, MediaAsset,
    MediaCatalog, MediaKind, MediaSourceFingerprint, OpError, Operation, Rational, SolidColor,
    TRANSITION_DESCRIPTORS, TimeCode, Track, TrackId, TrackKind, TransitionAxis, TransitionShading,
    transition_descriptor,
};

fn fps(n: u32) -> Rational {
    Rational::new(n, 1).unwrap()
}

fn asset(id: u64, kind: MediaKind, duration: i64) -> MediaAsset {
    MediaAsset {
        id: AssetId(id),
        path: std::path::PathBuf::from(format!("a{id}.mp4")),
        name: format!("a{id}"),
        duration: TimeCode(duration),
        fps: fps(30),
        kind,
        resolution: Some((1_920, 1_080)),
        source_fingerprint: MediaSourceFingerprint::default(),
        color_description: kinewright_core::ColorDescription::default(),
        assumed_from: None,
    }
}

/// Video track 1, video track 2, audio track 3; asset 1 is audio/video.
fn empty() -> Document {
    let track = |id, kind| Track {
        id: TrackId(id),
        kind,
        sync_lock: true,
        clips: Vec::new(),
    };
    let mut doc = Document {
        investigator: None,
        catalog: MediaCatalog::default(),
        audio_mix: AudioMix::default(),
        color_context: ColorContext::default(),
        lut_assets: Vec::new(),
        tracks: vec![
            track(1, TrackKind::Video),
            track(2, TrackKind::Video),
            track(3, TrackKind::Audio),
        ],
        media_pool: Vec::new(),
        markers: Vec::new(),
        fps: fps(30),
        resolution: (1_920, 1_080),
        duration: TimeCode::ZERO,
    };
    Operation::AddAsset {
        asset: asset(1, MediaKind::AudioVideo, 600),
    }
    .apply(&mut doc)
    .unwrap();
    doc
}

/// A generated clip literal over a project-frame span, as a loaded file
/// would carry it.
fn generated(id: u64, content: ClipContent, at: i64, duration: i64) -> Clip {
    Clip {
        id: ClipId(id),
        asset: AssetId::default(),
        source_range: TimeCode(0)..TimeCode(duration),
        content,
        timeline_start: TimeCode(at),
        effects: Vec::new(),
        transition_in: None,
        link: None,
        audio_gain_tenth_db: 0,
        audio_fade_in_frames: TimeCode::ZERO,
        audio_fade_out_frames: TimeCode::ZERO,
        audio_gain_curve: None,
        speed_percent: 100,
        enabled: true,
        enabled_curve: None,
        blend_mode: BlendMode::Normal,
    }
}

/// Place a clip literal on a track and fix the document duration, the way a
/// hand-written file would.
fn place(doc: &mut Document, track: usize, clip: Clip) {
    let end = clip.timeline_start.0 + (clip.source_range.end.0 - clip.source_range.start.0);
    doc.tracks[track].clips.push(clip);
    doc.tracks[track]
        .clips
        .sort_by_key(|clip| (clip.timeline_start, clip.id));
    doc.duration = TimeCode(doc.duration.0.max(end));
}

fn solid() -> ClipContent {
    ClipContent::Solid(SolidColor {
        r: 12,
        g: 34,
        b: 56,
    })
}

// ------------------------------------------------------------------ R1

/// R1: `Normal` serialises away, so a pre-MO2 clip's bytes never move, and
/// every mode round-trips under its snake-case wire name.
#[test]
fn r1_blend_mode_serde_identity() {
    let mut doc = empty();
    Operation::AddClip {
        track: TrackId(1),
        asset: AssetId(1),
        at: TimeCode(0),
        source: TimeCode(0)..TimeCode(30),
    }
    .apply(&mut doc)
    .unwrap();
    let clip = doc.clip(ClipId(1)).unwrap().clone();
    let bytes = serde_json::to_string(&clip).unwrap();
    assert!(!bytes.contains("blend_mode"), "Normal must skip: {bytes}");
    let parsed: Clip = serde_json::from_str(&bytes).unwrap();
    assert_eq!(parsed.blend_mode, BlendMode::Normal, "absent reads Normal");
    assert_eq!(serde_json::to_string(&parsed).unwrap(), bytes);

    let names = [
        "normal", "multiply", "screen", "overlay", "darken", "lighten", "add",
    ];
    for (mode, name) in BlendMode::ALL.into_iter().zip(names) {
        assert_eq!(serde_json::to_value(mode).unwrap(), name);
        let mut blended = clip.clone();
        blended.blend_mode = mode;
        let text = serde_json::to_string(&blended).unwrap();
        assert_eq!(
            text.contains(&format!("\"blend_mode\":\"{name}\"")),
            mode != BlendMode::Normal
        );
        assert_eq!(serde_json::from_str::<Clip>(&text).unwrap(), blended);
    }
    assert!(serde_json::from_str::<BlendMode>("\"hard_light\"").is_err());
}

/// R1: blend is honoured for visual layers only and silently inert on audio
/// ones — an audio-track clip carrying a blend validates (the linked A/V
/// batch is never stranded).
#[test]
fn r1_blend_on_an_audio_clip_is_inert_and_valid() {
    let mut doc = empty();
    Operation::AddClip {
        track: TrackId(3),
        asset: AssetId(1),
        at: TimeCode(0),
        source: TimeCode(0)..TimeCode(30),
    }
    .apply(&mut doc)
    .unwrap();
    doc.tracks[2].clips[0].blend_mode = BlendMode::Screen;
    doc.validate().unwrap();
}

// ------------------------------------------------------------ R2 / R3

/// R2/R3: the wire shapes — a unit `adjustment` and `solid` with display
/// bytes — round-trip and validate on a video track.
#[test]
fn r2_r3_generated_content_serde_shapes() {
    let adjustment = generated(1, ClipContent::Adjustment, 0, 10);
    let value = serde_json::to_value(&adjustment).unwrap();
    assert_eq!(value["content"], "adjustment");
    assert_eq!(serde_json::from_value::<Clip>(value).unwrap(), adjustment);

    let solid = generated(2, solid(), 0, 10);
    let value = serde_json::to_value(&solid).unwrap();
    assert_eq!(
        value["content"],
        serde_json::json!({ "solid": { "r": 12, "g": 34, "b": 56 } })
    );
    assert_eq!(serde_json::from_value::<Clip>(value).unwrap(), solid);

    let mut doc = empty();
    place(&mut doc, 0, adjustment);
    place(&mut doc, 1, solid);
    doc.validate().unwrap();
    let text = serde_json::to_string(&doc).unwrap();
    let reloaded: Document = serde_json::from_str(&text).unwrap();
    reloaded.validate().unwrap();
    assert_eq!(reloaded, doc);
    assert_eq!(
        doc.clip_duration(doc.clip(ClipId(1)).unwrap()),
        Ok(TimeCode(10))
    );
}

/// R2/R3: loading refuses the generated kinds on an audio track and a
/// malformed span, with the typed placement / range errors.
#[test]
fn r2_r3_loaded_generated_clips_validate_track_and_span() {
    let mut doc = empty();
    place(&mut doc, 2, generated(1, ClipContent::Adjustment, 0, 10));
    assert_eq!(
        doc.validate(),
        Err(OpError::AdjustmentOnAudioTrack(TrackId(3)))
    );

    let mut doc = empty();
    place(&mut doc, 2, generated(1, solid(), 0, 10));
    assert_eq!(doc.validate(), Err(OpError::SolidOnAudioTrack(TrackId(3))));

    for content in [ClipContent::Adjustment, solid()] {
        let mut doc = empty();
        let mut clip = generated(1, content, 0, 10);
        clip.source_range = TimeCode(-1)..TimeCode(9);
        place(&mut doc, 0, clip);
        assert_eq!(
            doc.validate(),
            Err(OpError::InvalidSourceRange { start: -1, end: 9 })
        );
    }
}

/// R2/R3: the generated kinds reference no asset, so export preflight and
/// source availability never see the ignored default id.
#[test]
fn r2_r3_generated_kinds_reference_no_asset() {
    let mut doc = empty();
    place(&mut doc, 0, generated(1, ClipContent::Adjustment, 0, 10));
    place(&mut doc, 1, generated(2, solid(), 0, 10));
    doc.validate().unwrap();
    assert!(doc.timeline_referenced_media_assets().is_empty());
}

// ------------------------------------------------------------------ R4

/// R4: the 12 rows map name → shading with the entry edge as the sign
/// (−1 enters from the left/top, +1 from the right/bottom).
#[test]
fn r4_geometric_descriptors_name_their_entry_edge() {
    assert_eq!(TRANSITION_DESCRIPTORS.len(), 15);
    let mut names = std::collections::BTreeSet::new();
    for descriptor in TRANSITION_DESCRIPTORS {
        assert!(names.insert(descriptor.name), "unique {}", descriptor.name);
        assert!(!descriptor.description.is_empty());
    }
    for (direction, axis, sign) in [
        ("left", TransitionAxis::Horizontal, -1),
        ("right", TransitionAxis::Horizontal, 1),
        ("up", TransitionAxis::Vertical, -1),
        ("down", TransitionAxis::Vertical, 1),
    ] {
        let shading = |kind: &str| {
            transition_descriptor(&format!("{kind}_{direction}"))
                .unwrap()
                .shading
        };
        assert_eq!(shading("push"), TransitionShading::Push { axis, sign });
        assert_eq!(shading("slide"), TransitionShading::Slide { axis, sign });
        assert_eq!(shading("wipe"), TransitionShading::Wipe { axis, sign });
    }
}
