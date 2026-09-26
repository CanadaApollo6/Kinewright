//! MO2 Part A — core model contract tests (blend mode, adjustment and solid
//! content, the 12 geometric transition descriptors, span integrity,
//! survival, the adjustment support table, and the four operations).

use std::collections::BTreeMap;

use kinewright_core::{
    AssetId, AudioMix, AutomationCurve, BlendMode, Clip, ClipContent, ClipId, ColorContext,
    Command, Core, Document, Effect, EffectId, Event, Keyframe, KeyframeInterpolation, MediaAsset,
    MediaCatalog, MediaKind, MediaSourceFingerprint, OpError, Operation, ParamValue, Query,
    QueryResult, Rational, SolidColor, TRANSITION_DESCRIPTORS, TimeCode, Track, TrackId, TrackKind,
    Transition, TransitionAxis, TransitionShading, transition_descriptor,
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

// ------------------------------------------------------- A2 fixtures

fn effect(id: u64, name: &str) -> Effect {
    Effect {
        enabled: true,
        enabled_curve: None,
        id: EffectId(id),
        name: name.to_owned(),
        parameters: BTreeMap::new(),
        keyframes: BTreeMap::new(),
    }
}

fn curve(points: &[(i64, i64, KeyframeInterpolation)]) -> AutomationCurve {
    AutomationCurve {
        keyframes: points
            .iter()
            .map(|&(at, value, interpolation)| Keyframe {
                at: TimeCode(at),
                value,
                interpolation,
                tangent_in: 0,
                tangent_out: 0,
            })
            .collect(),
    }
}

/// Track 1: media clip 1 over 0..60. Track 2, butt-joined: adjustment 2
/// (0..20), solid 3 (20..40), adjustment 4 (40..60). Track 3 is audio.
fn stack() -> Document {
    let mut doc = empty();
    Operation::AddClip {
        track: TrackId(1),
        asset: AssetId(1),
        at: TimeCode(0),
        source: TimeCode(0)..TimeCode(60),
    }
    .apply(&mut doc)
    .unwrap();
    place(&mut doc, 1, generated(2, ClipContent::Adjustment, 0, 20));
    place(&mut doc, 1, generated(3, solid(), 20, 20));
    place(&mut doc, 1, generated(4, ClipContent::Adjustment, 40, 20));
    doc.validate().unwrap();
    doc
}

fn clip(doc: &Document, id: u64) -> &Clip {
    doc.clip(ClipId(id)).expect("clip")
}

fn clip_mut(doc: &mut Document, id: u64) -> &mut Clip {
    doc.tracks
        .iter_mut()
        .flat_map(|track| &mut track.clips)
        .find(|clip| clip.id == ClipId(id))
        .expect("clip")
}

fn span(doc: &Document, id: u64) -> (i64, i64, i64, i64) {
    let clip = clip(doc, id);
    (
        clip.timeline_start.0,
        clip.timeline_start.0 + doc.clip_duration(clip).unwrap().0,
        clip.source_range.start.0,
        clip.source_range.end.0,
    )
}

/// Apply `op` expecting `error`, and prove the refused op left the document
/// untouched.
#[allow(clippy::needless_pass_by_value)]
fn refuse(doc: &Document, op: Operation, error: &OpError) {
    let mut candidate = doc.clone();
    assert_eq!(op.apply(&mut candidate).as_ref(), Err(error), "{op:?}");
    assert_eq!(&candidate, doc, "a refused {op:?} must not mutate");
}

// ------------------------------------------------------------------ R5

/// R5: trim, split, roll, slide, move and ripple ride the span paths with
/// project-frame arithmetic on both generated kinds.
#[test]
fn r5_span_edits_ride_the_span_paths() {
    let base = stack();

    let mut doc = base.clone();
    Operation::TrimClip {
        clip: ClipId(2),
        new_source: TimeCode(5)..TimeCode(20),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(span(&doc, 2), (5, 20, 5, 20));

    let mut doc = base.clone();
    Operation::SplitClip {
        clip: ClipId(3),
        at: TimeCode(30),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(span(&doc, 3), (20, 30, 0, 10));
    let right = doc.tracks[1].clips[2].clone();
    assert_eq!(right.content, solid(), "the right half keeps its colour");
    assert_eq!(span(&doc, right.id.0), (30, 40, 10, 20));

    let mut doc = base.clone();
    Operation::RollEdit {
        left_clip: ClipId(2),
        right_clip: ClipId(3),
        to: TimeCode(25),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(span(&doc, 2), (0, 25, 0, 25));
    // MO1 R9: a span's origin never moves; the window resizes around it.
    assert_eq!(span(&doc, 3), (25, 40, 0, 15));

    let mut doc = base.clone();
    Operation::SlideClip {
        clip: ClipId(3),
        to: TimeCode(25),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(span(&doc, 2), (0, 25, 0, 25));
    assert_eq!(span(&doc, 3), (25, 45, 0, 20));
    assert_eq!(span(&doc, 4), (45, 60, 5, 20));

    let mut doc = base.clone();
    Operation::MoveClip {
        clip: ClipId(4),
        to_track: TrackId(2),
        to: TimeCode(90),
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(span(&doc, 4), (90, 110, 0, 20));
    assert_eq!(doc.duration, TimeCode(110));

    let mut doc = base.clone();
    Operation::RippleDeleteClip { clip: ClipId(2) }
        .apply(&mut doc)
        .unwrap();
    assert_eq!(span(&doc, 3), (0, 20, 0, 20));
    assert_eq!(span(&doc, 4), (20, 40, 0, 20));
}

/// R5 refusals: speed, the media-only editorial paths, audio-track moves
/// and the audio setters, each typed and each leaving the document as it
/// was.
#[test]
fn r5_generated_kinds_refuse_media_only_edits() {
    let doc = stack();
    for (id, audio_error, track_error) in [
        (
            2,
            OpError::AdjustmentClipHasNoAudio(ClipId(2)),
            OpError::AdjustmentOnAudioTrack(TrackId(3)),
        ),
        (
            3,
            OpError::SolidClipHasNoAudio(ClipId(3)),
            OpError::SolidOnAudioTrack(TrackId(3)),
        ),
    ] {
        let clip_id = ClipId(id);
        refuse(
            &doc,
            Operation::SetClipSpeed {
                clip: clip_id,
                speed_percent: 200,
            },
            &OpError::SpeedOnNonMediaClip(clip_id),
        );
        for op in [
            Operation::ReplaceClip {
                clip: clip_id,
                asset: AssetId(1),
                source: TimeCode(0)..TimeCode(20),
            },
            Operation::FitToFill {
                clip: clip_id,
                asset: AssetId(1),
                source: TimeCode(0)..TimeCode(40),
            },
            Operation::SlipClip {
                clip: clip_id,
                new_source_in: TimeCode(5),
            },
        ] {
            refuse(&doc, op, &OpError::EditorialRequiresMedia(clip_id));
        }
        refuse(
            &doc,
            Operation::MoveClip {
                clip: clip_id,
                to_track: TrackId(3),
                to: TimeCode(100),
            },
            &track_error,
        );
        refuse(
            &doc,
            Operation::SetClipAudio {
                clip: clip_id,
                gain_tenth_db: -60,
                fade_in_frames: TimeCode::ZERO,
                fade_out_frames: TimeCode::ZERO,
            },
            &audio_error,
        );
        refuse(
            &doc,
            Operation::SetClipGainEnvelope {
                clip: clip_id,
                curve: Some(curve(&[(0, -60, KeyframeInterpolation::Linear)])),
            },
            &audio_error,
        );
    }
}

/// R8: the *neutral* audio setters — a gain-envelope clear and an all-zero
/// `SetClipAudio` — are refused on both generated kinds with their typed
/// error, directly and through the Core actor, leaving document, revision
/// and both undo and redo histories exactly as they were.
#[test]
fn r8_neutral_audio_setters_refuse_generated_clips() {
    let neutral = |clip| {
        [
            Operation::SetClipGainEnvelope { clip, curve: None },
            Operation::SetClipAudio {
                clip,
                gain_tenth_db: 0,
                fade_in_frames: TimeCode::ZERO,
                fade_out_frames: TimeCode::ZERO,
            },
        ]
    };
    let cases = [
        (ClipId(2), OpError::AdjustmentClipHasNoAudio(ClipId(2))),
        (ClipId(3), OpError::SolidClipHasNoAudio(ClipId(3))),
    ];
    let initial = stack();
    for (clip_id, error) in &cases {
        for op in neutral(*clip_id) {
            refuse(&initial, op, error);
        }
    }

    // One undo and one redo entry, so a refusal that touched either history
    // shows up when both are walked afterwards.
    let core = Core::spawn(initial.clone()).unwrap();
    let mut done = Vec::new();
    for blend_mode in [BlendMode::Screen, BlendMode::Overlay] {
        let Event::DocumentChanged { doc, .. } = core
            .request(Command::Do(Operation::SetClipBlendMode {
                clip: ClipId(1),
                blend_mode,
            }))
            .unwrap()
        else {
            panic!("the blend lands");
        };
        done.push((*doc).clone());
    }
    let Event::DocumentChanged { doc, .. } = core.request(Command::Undo).unwrap() else {
        panic!("undo answers with the document");
    };
    assert_eq!(*doc, done[0]);
    let snapshot = || match core.request(Command::Query(Query::Snapshot)).unwrap() {
        Event::QueryResult(QueryResult::Snapshot { revision, document }) => (revision, document),
        other => panic!("{other:?}"),
    };
    let before = snapshot();
    for (clip_id, error) in cases {
        for op in neutral(clip_id) {
            let Event::OpRejected {
                error: rejected, ..
            } = core.request(Command::Do(op.clone())).unwrap()
            else {
                panic!("{op:?} on a generated clip is refused");
            };
            assert_eq!(rejected, error, "{op:?}");
            assert_eq!(snapshot(), before, "{op:?} left revision and document");
        }
    }
    for (command, expected) in [
        (Command::Redo, &done[1]),
        (Command::Undo, &done[0]),
        (Command::Undo, &initial),
    ] {
        let Event::DocumentChanged { doc, .. } = core.request(command).unwrap() else {
            panic!("history answers with the document");
        };
        assert_eq!(*doc, *expected, "both histories survive the refusals");
    }
}

// ------------------------------------------------------------------ R6

/// Generated clip 2 (of `content`) carrying a keyed look, a keyed effect
/// toggle and a keyed clip toggle — every keep-outside owner the generated
/// kinds have: effect values, effect enable and clip enable.
fn keyed_generated(content: ClipContent) -> Document {
    let mut doc = stack();
    let toggle = curve(&[
        (0, 1, KeyframeInterpolation::Hold),
        (10, 1, KeyframeInterpolation::Hold),
        (11, 0, KeyframeInterpolation::Hold),
        (19, 0, KeyframeInterpolation::Hold),
    ]);
    let mut look = effect(1, "brightness");
    look.parameters
        .insert("percent".to_owned(), ParamValue::Integer(0));
    look.keyframes.insert(
        "percent".to_owned(),
        curve(&[
            (0, 0, KeyframeInterpolation::Linear),
            (19, 40, KeyframeInterpolation::Linear),
        ]),
    );
    look.enabled_curve = Some(toggle.clone());
    let generated = clip_mut(&mut doc, 2);
    generated.content = content;
    generated.effects.push(look);
    generated.enabled_curve = Some(toggle);
    doc.validate().unwrap();
    doc
}

fn keyed_adjustment() -> Document {
    keyed_generated(ClipContent::Adjustment)
}

/// Both generated kinds: R6 survival holds for each, not only adjustments.
fn generated_kinds() -> [ClipContent; 2] {
    [ClipContent::Adjustment, solid()]
}

fn keys(curve: &AutomationCurve) -> Vec<(i64, i64)> {
    curve
        .keyframes
        .iter()
        .map(|key| (key.at.0, key.value))
        .collect()
}

/// The three keep-outside owners of a generated clip, in owner order:
/// effect values, effect enable, clip enable.
fn owners(clip: &Clip) -> [Vec<(i64, i64)>; 3] {
    [
        keys(&clip.effects[0].keyframes["percent"]),
        keys(
            clip.effects[0]
                .enabled_curve
                .as_ref()
                .expect("effect enable"),
        ),
        keys(clip.enabled_curve.as_ref().expect("clip enable")),
    ]
}

/// R6: trimming either generated kind in shifts every keep-outside curve
/// (negative keys legal), and trimming back out restores the bytes exactly.
#[test]
fn r6_generated_curves_survive_a_trim_cycle() {
    for content in generated_kinds() {
        let base = keyed_generated(content.clone());
        let mut doc = base.clone();
        Operation::TrimClip {
            clip: ClipId(2),
            new_source: TimeCode(5)..TimeCode(20),
        }
        .apply(&mut doc)
        .unwrap();
        let toggle = vec![(-5, 1), (5, 1), (6, 0), (14, 0)];
        assert_eq!(
            owners(clip(&doc, 2)),
            [vec![(-5, 0), (14, 40)], toggle.clone(), toggle],
            "{content:?} trimmed in"
        );
        assert_eq!(clip(&doc, 2).content, content);

        Operation::TrimClip {
            clip: ClipId(2),
            new_source: TimeCode(0)..TimeCode(20),
        }
        .apply(&mut doc)
        .unwrap();
        assert_eq!(doc, base, "{content:?} trimmed back out");
        assert_eq!(
            serde_json::to_string(clip(&doc, 2)).unwrap(),
            serde_json::to_string(clip(&base, 2)).unwrap()
        );
    }
}

/// R6: a split of either generated kind copies every curve to both halves,
/// each shifted to its own clip-local origin.
#[test]
fn r6_generated_curves_survive_a_split() {
    for content in generated_kinds() {
        let mut doc = keyed_generated(content.clone());
        Operation::SplitClip {
            clip: ClipId(2),
            at: TimeCode(8),
        }
        .apply(&mut doc)
        .unwrap();
        let toggle = vec![(0, 1), (10, 1), (11, 0), (19, 0)];
        assert_eq!(
            owners(clip(&doc, 2)),
            [vec![(0, 0), (19, 40)], toggle.clone(), toggle],
            "{content:?} left half"
        );
        let right = &doc.tracks[1].clips[1];
        assert_eq!(right.content, content);
        assert_eq!(clip(&doc, 2).content, content);
        let toggle = vec![(-8, 1), (2, 1), (3, 0), (11, 0)];
        assert_eq!(
            owners(right),
            [vec![(-8, 0), (11, 40)], toggle.clone(), toggle],
            "{content:?} right half"
        );
        doc.validate().unwrap();
    }
}

// ----------------------------------------------------------- R8 / R18

/// R8/R18: `chroma_key` is refused on an adjustment on every write path —
/// append, insert, attribute copy and load — even when disabled.
#[test]
fn r18_adjustment_refuses_chroma_key_on_every_path() {
    let doc = stack();
    let refused = OpError::EffectUnsupportedOnAdjustment {
        clip: ClipId(2),
        effect: "chroma_key".to_owned(),
    };
    let mut disabled = effect(1, "chroma_key");
    disabled.enabled = false;
    for key in [effect(1, "chroma_key"), disabled.clone()] {
        refuse(
            &doc,
            Operation::AddEffect {
                clip: ClipId(2),
                effect: key.clone(),
            },
            &refused,
        );
        refuse(
            &doc,
            Operation::InsertEffect {
                clip: ClipId(2),
                index: 0,
                effect: key,
            },
            &refused,
        );
    }

    let mut keyed_source = doc.clone();
    Operation::AddEffect {
        clip: ClipId(1),
        effect: disabled.clone(),
    }
    .apply(&mut keyed_source)
    .unwrap();
    refuse(
        &keyed_source,
        Operation::CopyClipAttributes {
            from_clip: ClipId(1),
            to_clip: ClipId(2),
            names: None,
            include_keyframes: false,
        },
        &refused,
    );

    let mut loaded = doc.clone();
    clip_mut(&mut loaded, 2).effects.push(disabled.clone());
    let text = serde_json::to_string(&loaded).unwrap();
    let parsed: Document = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed.validate(), Err(refused));

    // The restriction is the adjustment's alone: a solid may key.
    let mut solid_doc = doc.clone();
    Operation::AddEffect {
        clip: ClipId(3),
        effect: disabled,
    }
    .apply(&mut solid_doc)
    .unwrap();
}

/// R18: the colour fades are refused onto an adjustment (write and load);
/// crossfade and all twelve geometric transitions are its supported rows.
#[test]
fn r18_adjustment_refuses_colour_fades_and_admits_the_rest() {
    let doc = stack();
    for descriptor in TRANSITION_DESCRIPTORS {
        let transition = Transition {
            name: descriptor.name.to_owned(),
            duration: TimeCode(5),
        };
        let op = Operation::AddTransition {
            clip: ClipId(2),
            transition: transition.clone(),
        };
        if matches!(descriptor.shading, TransitionShading::FadeFromColor { .. }) {
            refuse(
                &doc,
                op,
                &OpError::TransitionUnsupportedOnAdjustment {
                    clip: ClipId(2),
                    transition: descriptor.name.to_owned(),
                },
            );
            let mut loaded = doc.clone();
            clip_mut(&mut loaded, 2).transition_in = Some(transition.clone());
            assert!(matches!(
                loaded.validate(),
                Err(OpError::TransitionUnsupportedOnAdjustment { .. })
            ));
            // A solid is an ordinary opaque layer: colour fades apply.
            let mut solid_doc = doc.clone();
            Operation::AddTransition {
                clip: ClipId(3),
                transition,
            }
            .apply(&mut solid_doc)
            .unwrap();
        } else {
            let mut accepted = doc.clone();
            op.apply(&mut accepted).unwrap();
        }
    }
}

/// R18 supported rows: the look, opacity and geometry effects, and a
/// non-normal blend, all land on an adjustment.
#[test]
fn r18_adjustment_accepts_its_supported_rows() {
    let mut doc = stack();
    for (id, name) in [
        (1, "brightness"),
        (2, "primary_correction"),
        (3, "opacity"),
        (4, "transform"),
        (5, "crop"),
    ] {
        Operation::AddEffect {
            clip: ClipId(2),
            effect: effect(id, name),
        }
        .apply(&mut doc)
        .unwrap_or_else(|error| panic!("{name}: {error}"));
    }
    clip_mut(&mut doc, 2).blend_mode = BlendMode::Screen;
    doc.validate().unwrap();
}

/// R8: a refused op through the Core actor leaves document, revision and
/// undo history exactly as they were.
#[test]
fn r8_refused_ops_leave_revision_and_history_unchanged() {
    let initial = stack();
    let core = Core::spawn(initial.clone()).unwrap();
    let Event::DocumentChanged { doc: after, .. } = core
        .request(Command::Do(Operation::AddEffect {
            clip: ClipId(2),
            effect: effect(1, "brightness"),
        }))
        .unwrap()
    else {
        panic!("a supported look is accepted");
    };
    let snapshot = || match core.request(Command::Query(Query::Snapshot)).unwrap() {
        Event::QueryResult(QueryResult::Snapshot { revision, document }) => (revision, document),
        other => panic!("{other:?}"),
    };
    let before = snapshot();
    let Event::OpRejected { error, .. } = core
        .request(Command::Do(Operation::AddEffect {
            clip: ClipId(2),
            effect: effect(2, "chroma_key"),
        }))
        .unwrap()
    else {
        panic!("chroma_key on an adjustment is refused");
    };
    assert!(matches!(
        error,
        OpError::EffectUnsupportedOnAdjustment { .. }
    ));
    assert_eq!(snapshot(), before);
    assert_eq!(*before.1, *after);
    let Event::DocumentChanged { doc, .. } = core.request(Command::Undo).unwrap() else {
        panic!("undo answers with the document");
    };
    assert_eq!(*doc, initial, "one undo step reaches the initial document");
}

// ----------------------------------------------------------------- R19

/// R19 model half: clip `enabled` / `enabled_curve` apply to generated kinds
/// as to any clip, keys clip-local; a disabled adjustment is a valid clip.
#[test]
fn r19_generated_kinds_carry_the_mo1_enable_model() {
    let mut doc = keyed_adjustment();
    let adjustment = clip(&doc, 2);
    assert!(adjustment.is_enabled_at(TimeCode(10)));
    assert!(!adjustment.is_enabled_at(TimeCode(11)));
    Operation::SetClipEnabled {
        clip: ClipId(3),
        enabled: false,
    }
    .apply(&mut doc)
    .unwrap();
    assert!(!clip(&doc, 3).is_enabled_at(TimeCode(0)));
    doc.validate().unwrap();
}

// ------------------------------------------------------------------ R8

/// R8: the four operations' wire shapes — the agent's generated tools and
/// the journal both carry exactly these.
#[test]
fn r8_operation_wire_shapes() {
    let cases = [
        (
            Operation::SetClipBlendMode {
                clip: ClipId(2),
                blend_mode: BlendMode::Multiply,
            },
            serde_json::json!({"SetClipBlendMode": {"clip": 2, "blend_mode": "multiply"}}),
        ),
        (
            Operation::AddAdjustmentClip {
                track: TrackId(2),
                timeline_start: TimeCode(60),
                duration: TimeCode(30),
                effects: Vec::new(),
            },
            serde_json::json!({"AddAdjustmentClip": {
                "track": 2, "timeline_start": 60, "duration": 30, "effects": []
            }}),
        ),
        (
            Operation::AddSolidClip {
                track: TrackId(2),
                timeline_start: TimeCode(60),
                duration: TimeCode(30),
                color: SolidColor { r: 1, g: 2, b: 3 },
            },
            serde_json::json!({"AddSolidClip": {
                "track": 2, "timeline_start": 60, "duration": 30,
                "color": {"r": 1, "g": 2, "b": 3}
            }}),
        ),
        (
            Operation::SetSolidColor {
                clip: ClipId(3),
                color: SolidColor { r: 9, g: 8, b: 7 },
            },
            serde_json::json!({"SetSolidColor": {"clip": 3, "color": {"r": 9, "g": 8, "b": 7}}}),
        ),
    ];
    for (op, json) in cases {
        assert_eq!(serde_json::to_value(&op).unwrap(), json);
        assert_eq!(serde_json::from_value::<Operation>(json).unwrap(), op);
    }
}

/// R8: creation ops address their track, the setters their clip (the
/// `AddTitle` precedent).
#[test]
fn r8_incident_subjects() {
    use kinewright_core::IncidentSubject;
    let track =
        |op: Operation| assert_eq!(op.incident_subject(), IncidentSubject::Track(TrackId(2)));
    track(Operation::AddAdjustmentClip {
        track: TrackId(2),
        timeline_start: TimeCode(0),
        duration: TimeCode(1),
        effects: Vec::new(),
    });
    track(Operation::AddSolidClip {
        track: TrackId(2),
        timeline_start: TimeCode(0),
        duration: TimeCode(1),
        color: SolidColor::default(),
    });
    let clip = |op: Operation| assert_eq!(op.incident_subject(), IncidentSubject::Clip(ClipId(3)));
    clip(Operation::SetClipBlendMode {
        clip: ClipId(3),
        blend_mode: BlendMode::Add,
    });
    clip(Operation::SetSolidColor {
        clip: ClipId(3),
        color: SolidColor::default(),
    });
}

/// R8: both creation ops place a fresh clip over `0..duration` of project
/// frames at `timeline_start`, with no asset and no audio, and extend the
/// project.
#[test]
fn r8_creation_places_generated_clips() {
    let mut doc = stack();
    let mut look = effect(7, "brightness");
    look.parameters
        .insert("percent".to_owned(), ParamValue::Integer(20));
    Operation::AddAdjustmentClip {
        track: TrackId(2),
        timeline_start: TimeCode(60),
        duration: TimeCode(30),
        effects: vec![look.clone()],
    }
    .apply(&mut doc)
    .unwrap();
    let adjustment = doc.tracks[1].clips.last().unwrap().clone();
    assert_eq!(adjustment.id, ClipId(5), "the next clip id");
    assert_eq!(adjustment.content, ClipContent::Adjustment);
    assert_eq!(adjustment.effects, vec![look]);
    assert_eq!(adjustment.blend_mode, BlendMode::Normal);
    assert_eq!(span(&doc, 5), (60, 90, 0, 30));
    assert_eq!(doc.duration, TimeCode(90));

    Operation::AddSolidClip {
        track: TrackId(1),
        timeline_start: TimeCode(60),
        duration: TimeCode(15),
        color: SolidColor { r: 1, g: 2, b: 3 },
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(
        clip(&doc, 6).content,
        ClipContent::Solid(SolidColor { r: 1, g: 2, b: 3 })
    );
    assert_eq!(span(&doc, 6), (60, 75, 0, 15));
    assert_eq!(doc.timeline_referenced_media_assets().len(), 1);
    doc.validate().unwrap();
}

/// R8's creation validation matrix, each refusal typed and each leaving the
/// document untouched.
#[test]
fn r8_creation_validation_matrix() {
    let doc = stack();
    let adjustment = |track: u64, start: i64, duration: i64, effects: Vec<Effect>| {
        Operation::AddAdjustmentClip {
            track: TrackId(track),
            timeline_start: TimeCode(start),
            duration: TimeCode(duration),
            effects,
        }
    };
    let solid = |track: u64, start: i64, duration: i64| Operation::AddSolidClip {
        track: TrackId(track),
        timeline_start: TimeCode(start),
        duration: TimeCode(duration),
        color: SolidColor::default(),
    };
    for (op, error) in [
        (
            adjustment(9, 60, 10, vec![]),
            OpError::MissingTrack(TrackId(9)),
        ),
        (solid(9, 60, 10), OpError::MissingTrack(TrackId(9))),
        (
            adjustment(2, -1, 10, vec![]),
            OpError::NegativeTimelinePosition(TimeCode(-1)),
        ),
        (
            solid(2, -1, 10),
            OpError::NegativeTimelinePosition(TimeCode(-1)),
        ),
        (
            adjustment(2, 60, 0, vec![]),
            OpError::InvalidSourceRange { start: 0, end: 0 },
        ),
        (
            solid(2, 60, -5),
            OpError::InvalidSourceRange { start: 0, end: -5 },
        ),
        (
            adjustment(2, i64::MAX - 1, 10, vec![]),
            OpError::TimeOverflow,
        ),
        (solid(2, i64::MAX - 1, 10), OpError::TimeOverflow),
        (
            adjustment(3, 60, 10, vec![]),
            OpError::AdjustmentOnAudioTrack(TrackId(3)),
        ),
        (solid(3, 60, 10), OpError::SolidOnAudioTrack(TrackId(3))),
        (
            adjustment(2, 50, 10, vec![]),
            OpError::ClipOverlap {
                track: TrackId(2),
                clip: ClipId(4),
                with: ClipId(5),
            },
        ),
        (
            adjustment(2, 60, 10, vec![effect(1, "chroma_key")]),
            OpError::EffectUnsupportedOnAdjustment {
                clip: ClipId(5),
                effect: "chroma_key".to_owned(),
            },
        ),
        (
            adjustment(2, 60, 10, vec![effect(1, "audio_gain")]),
            OpError::AudioEffectOnClip {
                clip: ClipId(5),
                effect: "audio_gain".to_owned(),
            },
        ),
        (
            adjustment(
                2,
                60,
                10,
                vec![effect(1, "brightness"), effect(1, "contrast")],
            ),
            OpError::DuplicateEffect {
                clip: ClipId(5),
                effect: EffectId(1),
            },
        ),
    ] {
        refuse(&doc, op, &error);
    }
}

/// R8: the two setters, including the solid-only refusal and a blend on an
/// audio clip (inert, accepted).
#[test]
fn r8_setters() {
    let mut doc = stack();
    for id in [1, 2, 3] {
        Operation::SetClipBlendMode {
            clip: ClipId(id),
            blend_mode: BlendMode::Overlay,
        }
        .apply(&mut doc)
        .unwrap();
        assert_eq!(clip(&doc, id).blend_mode, BlendMode::Overlay);
    }
    Operation::SetClipBlendMode {
        clip: ClipId(1),
        blend_mode: BlendMode::Normal,
    }
    .apply(&mut doc)
    .unwrap();
    assert!(
        !serde_json::to_string(clip(&doc, 1))
            .unwrap()
            .contains("blend_mode"),
        "setting normal restores the pre-MO2 bytes"
    );

    let color = SolidColor {
        r: 200,
        g: 100,
        b: 0,
    };
    Operation::SetSolidColor {
        clip: ClipId(3),
        color,
    }
    .apply(&mut doc)
    .unwrap();
    assert_eq!(clip(&doc, 3).content, ClipContent::Solid(color));
    for id in [1, 2] {
        refuse(
            &doc,
            Operation::SetSolidColor {
                clip: ClipId(id),
                color,
            },
            &OpError::SolidColorOnNonSolidClip(ClipId(id)),
        );
    }
    refuse(
        &doc,
        Operation::SetClipBlendMode {
            clip: ClipId(99),
            blend_mode: BlendMode::Add,
        },
        &OpError::MissingClip(ClipId(99)),
    );
}

/// R8: each operation is revision-gated and one undo step through the Core
/// actor.
#[test]
fn r8_operations_are_revision_gated_single_undo_steps() {
    let initial = stack();
    let core = Core::spawn(initial.clone()).unwrap();
    let revision = || match core.request(Command::Query(Query::Snapshot)).unwrap() {
        Event::QueryResult(QueryResult::Snapshot { revision, .. }) => revision,
        other => panic!("{other:?}"),
    };
    let ops = [
        Operation::AddAdjustmentClip {
            track: TrackId(2),
            timeline_start: TimeCode(60),
            duration: TimeCode(30),
            effects: vec![effect(1, "brightness")],
        },
        Operation::AddSolidClip {
            track: TrackId(1),
            timeline_start: TimeCode(60),
            duration: TimeCode(30),
            color: SolidColor::default(),
        },
        Operation::SetClipBlendMode {
            clip: ClipId(5),
            blend_mode: BlendMode::Screen,
        },
        Operation::SetSolidColor {
            clip: ClipId(6),
            color: SolidColor { r: 1, g: 1, b: 1 },
        },
    ];
    let mut history = vec![initial];
    for op in ops {
        let stale = revision();
        let Event::DocumentChanged { doc, .. } = core
            .request(Command::DoIfRevision {
                expected: stale,
                operation: op.clone(),
                token: None,
            })
            .unwrap()
        else {
            panic!("{op:?} lands at the current revision");
        };
        history.push((*doc).clone());
        let Event::RevisionConflict { .. } = core
            .request(Command::DoIfRevision {
                expected: stale,
                operation: op.clone(),
                token: None,
            })
            .unwrap()
        else {
            panic!("{op:?} at a stale revision is refused");
        };
    }
    history.pop();
    while let Some(expected) = history.pop() {
        let Event::DocumentChanged { doc, .. } = core.request(Command::Undo).unwrap() else {
            panic!("undo answers with the document");
        };
        assert_eq!(*doc, expected, "one undo step per operation");
    }
}
