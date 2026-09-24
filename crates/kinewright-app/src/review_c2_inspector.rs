//! Reviewer 2, MO1 Part C: adversarial scenarios over the inspector's MOTION
//! surface, the stills import, copy/paste and the plan Apply path.
#![allow(clippy::unwrap_used, clippy::too_many_lines)]

use std::collections::BTreeMap;

use eframe::egui;
use kinewright_core::{
    AssetId, AutomationCurve, Clip, ClipContent, ClipId, Command, Core, Document, Effect, EffectId,
    Event, Keyframe, KeyframeInterpolation, MediaAsset, MediaKind, Operation, ParamValue, Rational,
    TimeCode, Track, TrackId, TrackKind, apply_batch,
};

use super::*;

fn key(at: i64, value: i64, interpolation: KeyframeInterpolation) -> Keyframe {
    Keyframe {
        at: TimeCode(at),
        value,
        interpolation,
        tangent_in: 0,
        tangent_out: 0,
    }
}

fn clip(id: u64, start: i64, source: std::ops::Range<i64>, effects: Vec<Effect>) -> Clip {
    Clip {
        enabled: true,
        enabled_curve: None,
        id: ClipId(id),
        asset: AssetId(1),
        source_range: TimeCode(source.start)..TimeCode(source.end),
        content: ClipContent::Media,
        timeline_start: TimeCode(start),
        effects,
        transition_in: None,
        link: None,
        audio_gain_tenth_db: 0,
        audio_fade_in_frames: TimeCode::ZERO,
        audio_fade_out_frames: TimeCode::ZERO,
        speed_percent: 100,
        audio_gain_curve: None,
    }
}

fn video_asset() -> MediaAsset {
    MediaAsset {
        id: AssetId(1),
        path: "fixture.mp4".into(),
        name: "fixture".into(),
        duration: TimeCode(600),
        fps: Rational::new(30, 1).unwrap(),
        kind: MediaKind::Video,
        resolution: Some((1920, 1080)),
        source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
        color_description: kinewright_core::ColorDescription::default(),
        assumed_from: None,
    }
}

fn document(clips: Vec<Clip>) -> Document {
    let end = clips
        .iter()
        .map(|clip| clip.timeline_start.0 + clip.source_range.end.0 - clip.source_range.start.0)
        .max()
        .unwrap_or(0);
    let mut document = Document {
        investigator: None,
        catalog: kinewright_core::MediaCatalog::default(),
        audio_mix: kinewright_core::AudioMix::default(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips,
        }],
        media_pool: vec![video_asset()],
        markers: Vec::new(),
        fps: Rational::new(30, 1).unwrap(),
        resolution: (1920, 1080),
        duration: TimeCode(end),
        color_context: kinewright_core::ColorContext::default(),
        lut_assets: Vec::new(),
    };
    // Settle any derived fields through one no-op batch.
    apply_batch(&mut document, &[]).ok();
    document
}

fn transform(id: u64, statics: &[(&str, i64)], curves: &[(&str, Vec<Keyframe>)]) -> Effect {
    let descriptor = kinewright_core::effect_descriptor("transform").unwrap();
    let mut parameters: BTreeMap<String, ParamValue> = descriptor
        .parameters
        .iter()
        .map(|parameter| {
            (
                parameter.name.to_owned(),
                ParamValue::Integer(parameter.neutral),
            )
        })
        .collect();
    for (name, value) in statics {
        parameters.insert((*name).to_owned(), ParamValue::Integer(*value));
    }
    Effect {
        enabled: true,
        enabled_curve: None,
        id: EffectId(id),
        name: "transform".to_owned(),
        parameters,
        keyframes: curves
            .iter()
            .map(|(name, keys)| {
                (
                    (*name).to_owned(),
                    AutomationCurve {
                        keyframes: keys.clone(),
                    },
                )
            })
            .collect(),
    }
}

fn opacity(keys: Vec<Keyframe>, parked: i64) -> Effect {
    Effect {
        enabled: true,
        enabled_curve: None,
        id: EffectId(5),
        name: "opacity".to_owned(),
        parameters: BTreeMap::from([("percent".to_owned(), ParamValue::Integer(parked))]),
        keyframes: if keys.is_empty() {
            BTreeMap::new()
        } else {
            BTreeMap::from([("percent".to_owned(), AutomationCurve { keyframes: keys })])
        },
    }
}

pub(crate) fn texts(output: &egui::FullOutput) -> Vec<(String, egui::Rect)> {
    fn collect(shape: &egui::epaint::Shape, into: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::epaint::Shape::Text(text) => into.push((
                text.galley.text().to_owned(),
                text.galley.rect.translate(text.pos.to_vec2()),
            )),
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, into);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    for clipped in &output.shapes {
        collect(&clipped.shape, &mut found);
    }
    found
}

/// Run one frame of `motion_param_row` for `effect.name`/`param`.
pub(crate) fn param_row_frame(
    ctx: &egui::Context,
    clip: ClipId,
    effect: &Effect,
    param: &str,
    at: i64,
    events: Vec<egui::Event>,
    time: f64,
) -> (egui::FullOutput, InspectorEdits) {
    let mut pending = InspectorEdits::default();
    let output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(500.0, 600.0),
            )),
            time: Some(time),
            events,
            ..Default::default()
        },
        |ui| {
            let descriptor = EFFECT_DESCRIPTORS
                .iter()
                .find(|descriptor| descriptor.name == effect.name)
                .unwrap();
            let parameter = descriptor
                .parameters
                .iter()
                .find(|parameter| parameter.name == param)
                .unwrap();
            motion_param_row(
                ui,
                clip,
                effect,
                parameter,
                TimeCode(at),
                TimeCode(30),
                &mut pending,
            );
        },
    );
    (output, pending)
}

/// Press one recorded control of a param row; return the release frame's ops.
pub(crate) fn press_param(
    ctx: &egui::Context,
    effect: &Effect,
    param: &str,
    button: &str,
) -> Vec<Operation> {
    let _ = crate::mixer_ui::take_strip_rects();
    let _ = param_row_frame(ctx, ClipId(1), effect, param, 9, Vec::new(), 0.0);
    let rects = crate::mixer_ui::take_strip_rects();
    let target = rects
        .iter()
        .find(|(name, _)| name == button)
        .unwrap_or_else(|| panic!("{button} laid out: {rects:?}"))
        .1
        .center();
    let mut all = Vec::new();
    for (events, time) in [
        (
            vec![
                egui::Event::PointerMoved(target),
                egui::Event::PointerButton {
                    pos: target,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            0.04,
        ),
        (
            vec![egui::Event::PointerButton {
                pos: target,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            0.06,
        ),
    ] {
        let (_, pending) = param_row_frame(ctx, ClipId(1), effect, param, 9, events, time);
        all.extend(pending.operations.clone());
    }
    let _ = crate::mixer_ui::take_strip_rects();
    all
}

/// Drag from `from` by `steps` along x, returning each frame's edits.
pub(crate) fn drag_frames(
    mut frame: impl FnMut(Vec<egui::Event>, f64) -> InspectorEdits,
    from: egui::Pos2,
    steps: &[f32],
) -> Vec<InspectorEdits> {
    let mut out = Vec::new();
    let mut time = 0.1;
    out.push(frame(
        vec![
            egui::Event::PointerMoved(from),
            egui::Event::PointerButton {
                pos: from,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
        time,
    ));
    let mut last = from;
    for dx in steps {
        time += 0.05;
        last = egui::pos2(from.x + dx, from.y);
        out.push(frame(vec![egui::Event::PointerMoved(last)], time));
    }
    time += 0.05;
    out.push(frame(
        vec![egui::Event::PointerButton {
            pos: last,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
        time,
    ));
    time += 0.05;
    out.push(frame(Vec::new(), time));
    out
}

/// Route frames exactly as `submit_inspector_edits` does (gesture counter,
/// `{key}#{gesture}` coalescing) into a real Core; return the final doc.
fn route(core: &Core, frames: Vec<InspectorEdits>) -> Document {
    let mut gesture = 0u64;
    let mut last = None;
    for edits in frames {
        if edits.gesture_started {
            gesture += 1;
        }
        if edits.operations.is_empty() {
            continue;
        }
        let command = match edits.coalesce_key.clone() {
            Some(key) => Command::DoBatchCoalesced {
                operations: edits.operations.clone(),
                coalesce_key: format!("{key}#{gesture}"),
            },
            None => Command::DoBatch(edits.operations.clone()),
        };
        match core.request(command).unwrap() {
            Event::DocumentChanged { doc, .. } => last = Some(doc.as_ref().clone()),
            other => panic!("routed frame rejected: {other:?}"),
        }
    }
    last.expect("the gesture wrote something")
}

fn undo(core: &Core) -> Document {
    match core.request(Command::Undo).unwrap() {
        Event::DocumentChanged { doc, .. } => doc.as_ref().clone(),
        other => panic!("undo failed: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// (1) auto-key after a head trim lands at the right clip-local frame
// ---------------------------------------------------------------------------

#[test]
fn rc2_auto_key_after_head_trim_lands_at_the_playhead() {
    let scale = vec![
        key(0, 100, KeyframeInterpolation::Linear),
        key(59, 150, KeyframeInterpolation::Linear),
    ];
    let mut doc = document(vec![clip(
        1,
        0,
        0..60,
        vec![transform(1, &[], &[("scale_percent", scale)])],
    )]);
    apply_batch(
        &mut doc,
        &[Operation::TrimClip {
            clip: ClipId(1),
            new_source: TimeCode(20)..TimeCode(60),
        }],
    )
    .unwrap();
    let trimmed = doc.tracks[0].clips[0].clone();
    let curve = trimmed.effects[0].keyframes.get("scale_percent").unwrap();
    assert_eq!(curve.keyframes[0].at, TimeCode(-20), "keep-outside shift");
    // Playhead at project frame P; the inspector computes P - timeline_start.
    let position = trimmed.timeline_start.0 + 10;
    let playhead_local = position - trimmed.timeline_start.0;
    let op = motion_static_operation(
        ClipId(1),
        EffectId(1),
        "scale_percent",
        130,
        Some(curve),
        TimeCode(playhead_local),
        1..=400,
    );
    apply_batch(&mut doc, std::slice::from_ref(&op)).unwrap();
    let after = doc.tracks[0].clips[0].effects[0]
        .keyframes
        .get("scale_percent")
        .unwrap();
    assert_eq!(after.value_at(TimeCode(10)), Some(130));
    assert_eq!(after.keyframes.len(), 3);
    assert_eq!(
        after.keyframes[0].at,
        TimeCode(-20),
        "kept-outside key untouched"
    );
}

// ---------------------------------------------------------------------------
// (2) one undo per slider drag gesture on a keyframed param (widget level)
// ---------------------------------------------------------------------------

#[test]
fn rc2_one_slider_drag_on_a_keyed_param_is_one_undo_entry() {
    let effect = opacity(
        vec![
            key(0, 50, KeyframeInterpolation::Linear),
            key(29, 100, KeyframeInterpolation::Linear),
        ],
        80,
    );
    let doc = document(vec![clip(1, 0, 0..30, vec![effect.clone()])]);
    let core = Core::spawn(doc.clone()).unwrap();
    let ctx = egui::Context::default();
    crate::theme::install(&ctx);
    // N5 K4: the slider shows the value at the playhead (linear 50→100 at
    // frame 9), not the parked static — locate its box to aim at the bar.
    let shown = effect
        .keyframes
        .get("percent")
        .unwrap()
        .value_at(TimeCode(9))
        .unwrap()
        .to_string();
    assert_ne!(
        shown, "80",
        "the parked static is not what the control shows"
    );
    let (output, _) = param_row_frame(&ctx, ClipId(1), &effect, "percent", 9, Vec::new(), 0.0);
    let found = texts(&output);
    let value_box = found
        .iter()
        .find(|(text, _)| text == &shown)
        .unwrap_or_else(|| panic!("the slider shows {shown}: {found:?}"))
        .1;
    let press = egui::pos2(value_box.left() - 60.0, value_box.center().y);
    // The document the widget sees stays the one it started from (as in the
    // app, where the next frame's clip is the updated one — close enough for
    // a slider, whose value is absolute).
    let frames = drag_frames(
        |events, time| param_row_frame(&ctx, ClipId(1), &effect, "percent", 9, events, time).1,
        press,
        &[5.0, 15.0, 25.0],
    );
    let written: Vec<_> = frames
        .iter()
        .map(|edits| (edits.operations.len(), edits.coalesce_key.clone()))
        .collect();
    assert!(
        frames.iter().flat_map(|edits| edits.operations.iter()).all(
            |op| matches!(op, Operation::UpsertEffectKeyframe { key, .. } if key.at == TimeCode(9))
        ),
        "every drag frame auto-keys at the playhead: {written:?}"
    );
    let _ = route(&core, frames);
    let undone = undo(&core);
    assert_eq!(
        undone, doc,
        "one undo restores the pre-drag document; frames {written:?}"
    );
}

// ---------------------------------------------------------------------------
// (3) editing a static param creates no key; (4) reset keeps keys
// ---------------------------------------------------------------------------

#[test]
fn rc2_static_edit_writes_no_key_and_reset_keeps_keys() {
    let op = motion_static_operation(
        ClipId(1),
        EffectId(1),
        "scale_percent",
        130,
        None,
        TimeCode(10),
        1..=400,
    );
    let mut doc = document(vec![clip(1, 0, 0..60, vec![transform(1, &[], &[])])]);
    apply_batch(&mut doc, std::slice::from_ref(&op)).unwrap();
    let effect = &doc.tracks[0].clips[0].effects[0];
    assert!(effect.keyframes.is_empty());
    assert_eq!(
        effect.parameters.get("scale_percent"),
        Some(&ParamValue::Integer(130))
    );

    // Reset on a keyed row: static to neutral, keys kept (R22).
    let keyed = opacity(
        vec![
            key(0, 50, KeyframeInterpolation::Linear),
            key(15, 100, KeyframeInterpolation::Linear),
        ],
        80,
    );
    let mut doc = document(vec![clip(1, 0, 0..30, vec![keyed.clone()])]);
    let ctx = egui::Context::default();
    crate::theme::install(&ctx);
    let pressed = press_param(&ctx, &keyed, "percent", "motion_reset_param");
    apply_batch(&mut doc, &pressed).unwrap();
    let effect = &doc.tracks[0].clips[0].effects[0];
    assert_eq!(
        effect.parameters.get("percent"),
        Some(&ParamValue::Integer(100))
    );
    assert_eq!(effect.keyframes.get("percent").unwrap().keyframes.len(), 2);
}

// ---------------------------------------------------------------------------
// (5) last-key removal through the row writes the value as static (R16)
// ---------------------------------------------------------------------------

#[test]
fn rc2_last_key_row_remove_writes_the_static() {
    let keyed = opacity(vec![key(7, 33, KeyframeInterpolation::Linear)], 80);
    let mut doc = document(vec![clip(1, 0, 0..30, vec![keyed.clone()])]);
    let ctx = egui::Context::default();
    crate::theme::install(&ctx);
    let pressed = press_param(&ctx, &keyed, "percent", "keyframe_row_remove");
    assert_eq!(pressed.len(), 1, "{pressed:?}");
    apply_batch(&mut doc, &pressed).unwrap();
    let effect = &doc.tracks[0].clips[0].effects[0];
    assert!(!effect.keyframes.contains_key("percent"));
    assert_eq!(
        effect.parameters.get("percent"),
        Some(&ParamValue::Integer(33))
    );
}

// ---------------------------------------------------------------------------
// (6) prev/next nav with keys outside the clip range
// ---------------------------------------------------------------------------

#[test]
fn rc2_nav_neighbours_stay_inside_the_clip() {
    // N5 K4: nav stays inside the clip. Clip 30 frames; keys at -20
    // (head-trimmed out), 5, 25, 45 (tail-trimmed out). From local 5, prev
    // finds nothing inside and next lands on 25 — the kept-outside keys are
    // preserved, not navigated to.
    let (previous, next) = motion_neighbor_frames(&[-20, 5, 25, 45], 5, 30);
    assert_eq!((previous, next), (None, Some(25)));
    let (previous, next) = motion_neighbor_frames(&[-20, 5, 25, 45], 25, 30);
    assert_eq!((previous, next), (Some(5), None));
}

// ---------------------------------------------------------------------------
// (7) a value edit on a kept-outside (negative) key must not move it
// ---------------------------------------------------------------------------

#[test]
fn rc2_value_edit_on_a_kept_outside_key_keeps_its_frame() {
    let keyed = opacity(
        vec![
            key(-20, 0, KeyframeInterpolation::Hold),
            key(0, 50, KeyframeInterpolation::Linear),
            key(15, 100, KeyframeInterpolation::EaseInOut),
        ],
        80,
    );
    let ctx = egui::Context::default();
    crate::theme::install(&ctx);
    let (output, _) = param_row_frame(&ctx, ClipId(1), &keyed, "percent", 9, Vec::new(), 0.0);
    let found = texts(&output);
    let frame_box = found
        .iter()
        .find(|(text, _)| text == "-20")
        .unwrap_or_else(|| panic!("row 0 paints -20: {found:?}"))
        .1;
    let value_box = found
        .iter()
        .filter(|(text, rect)| {
            text == "0"
                && (rect.center().y - frame_box.center().y).abs() < 4.0
                && rect.left() > frame_box.right()
        })
        .map(|(_, rect)| *rect)
        .next()
        .unwrap_or_else(|| panic!("row 0 paints its value 0: {found:?}"));
    let frames = drag_frames(
        |events, time| param_row_frame(&ctx, ClipId(1), &keyed, "percent", 9, events, time).1,
        value_box.center(),
        &[10.0, 20.0, 30.0],
    );
    let writes: Vec<&Operation> = frames
        .iter()
        .flat_map(|edits| edits.operations.iter())
        .collect();
    assert!(!writes.is_empty(), "the value drag wrote something");
    for write in writes {
        let Operation::SetEffectKeyframes { curve, .. } = write else {
            panic!("a row edit is a whole-curve Set: {write:?}");
        };
        assert_eq!(
            curve.keyframes[0].at,
            TimeCode(-20),
            "editing the VALUE of the kept-outside key moved it: {:?}",
            curve.keyframes
        );
    }
}

// ---------------------------------------------------------------------------
// (8) portrait still import → Freeze over Image with baked fit; then the
//     agent's ken_burns / push_in on that still must keep the fit
// ---------------------------------------------------------------------------

fn still_doc(resolution: (u32, u32)) -> (Document, MediaAsset) {
    let mut doc = document(Vec::new());
    let still = MediaAsset {
        id: AssetId(9),
        path: "portrait.png".into(),
        name: "portrait.png".into(),
        duration: TimeCode(1),
        fps: Rational::default(),
        kind: MediaKind::Image,
        resolution: Some(resolution),
        source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
        color_description: kinewright_core::ColorDescription::default(),
        assumed_from: None,
    };
    doc.media_pool.push(still.clone());
    doc.duration = TimeCode::ZERO;
    (doc, still)
}

#[test]
fn rc2_portrait_still_import_is_a_fitted_freeze() {
    let (mut doc, still) = still_doc((1080, 1920));
    let operations = crate::media_bin::tests_still_placement(&doc, &still).unwrap();
    apply_batch(&mut doc, &operations).unwrap();
    let placed = &doc.tracks[0].clips[0];
    assert!(matches!(placed.content, ClipContent::Freeze(_)));
    assert_eq!(doc.clip_duration(placed).unwrap(), TimeCode(150));
    let fit = kinewright_core::scale_to_frame_fit((1080, 1920), (1920, 1080));
    let t = &placed.effects[0];
    assert_eq!(
        (
            t.parameters.get("scale_x_percent"),
            t.parameters.get("scale_y_percent"),
            t.parameters.get("scale_fine_hundredths"),
        ),
        (
            Some(&ParamValue::Integer(fit.0)),
            Some(&ParamValue::Integer(fit.1)),
            Some(&ParamValue::Integer(fit.2)),
        )
    );
    assert_ne!(fit.2, 10_000, "a portrait still bakes a non-neutral fine");
}

#[test]
fn rc2_ken_burns_on_a_fitted_still_keeps_the_baked_fit() {
    for preset in [
        kinewright_agent::MotionPreset::KenBurns,
        kinewright_agent::MotionPreset::PushIn,
        kinewright_agent::MotionPreset::PullOut,
    ] {
        let (mut doc, still) = still_doc((1080, 1920));
        let operations = crate::media_bin::tests_still_placement(&doc, &still).unwrap();
        apply_batch(&mut doc, &operations).unwrap();
        let baked = match doc.tracks[0].clips[0].effects[0]
            .parameters
            .get("scale_fine_hundredths")
        {
            Some(ParamValue::Integer(value)) => *value,
            other => panic!("{other:?}"),
        };
        let plan = kinewright_agent::plan_motion(
            &doc,
            kinewright_core::TimelineRevision(0),
            &kinewright_agent::MotionPlanArgs {
                expected_revision: kinewright_core::TimelineRevision(0),
                clip_id: doc.tracks[0].clips[0].id,
                preset,
                replace: false,
            },
        )
        .unwrap();
        apply_batch(&mut doc, &plan.operations).unwrap();
        let effect = &doc.tracks[0].clips[0].effects[0];
        let first = effect
            .keyframes
            .get("scale_fine_hundredths")
            .unwrap()
            .value_at(TimeCode::ZERO)
            .unwrap();
        // The move must start from the fitted framing (or ramp relative to
        // it). A start at 10 000 over a 3 906 bake scales the still ×2.56 on
        // frame 0 — the fitted portrait overflows the frame vertically.
        assert_eq!(
            first, baked,
            "{preset:?} frame 0 fine {first} vs baked fit {baked}"
        );
    }
}

// ---------------------------------------------------------------------------
// (9) copy/paste attributes with-keys vs values-only onto a shorter clip
// ---------------------------------------------------------------------------

#[test]
fn rc2_paste_with_keys_and_values_only_onto_a_shorter_clip() {
    let source_keys = vec![
        key(0, 100, KeyframeInterpolation::Linear),
        key(59, 150, KeyframeInterpolation::EaseInOut),
    ];
    let long = clip(
        1,
        0,
        0..60,
        vec![transform(
            1,
            &[("x_percent", 7)],
            &[("scale_percent", source_keys.clone())],
        )],
    );
    let short = clip(2, 60, 100..120, vec![transform(3, &[], &[])]);
    let doc = document(vec![long, short]);
    let pastes = crate::timeline_ui::tests_paste_operations(Some(ClipId(1)), ClipId(2));
    assert_eq!(pastes.len(), 2);
    let mut with_keys = doc.clone();
    apply_batch(&mut with_keys, &pastes[0..1]).unwrap();
    let target = &with_keys.tracks[0].clips[1].effects[0];
    assert_eq!(target.id, EffectId(3), "target id kept");
    assert_eq!(
        target.keyframes.get("scale_percent").unwrap().keyframes,
        source_keys,
        "with keys: verbatim, even past the 20-frame target"
    );
    assert_eq!(
        target.parameters.get("x_percent"),
        Some(&ParamValue::Integer(7))
    );
    let mut values_only = doc.clone();
    apply_batch(&mut values_only, &pastes[1..2]).unwrap();
    let target = &values_only.tracks[0].clips[1].effects[0];
    assert!(
        !target.keyframes.contains_key("scale_percent"),
        "values only: no keys"
    );
    assert_eq!(
        target.parameters.get("x_percent"),
        Some(&ParamValue::Integer(7))
    );
    // Same-clip paste is not offered.
    assert!(crate::timeline_ui::tests_paste_operations(Some(ClipId(1)), ClipId(1)).is_empty());
}

// ---------------------------------------------------------------------------
// (10) plan Apply lands as one undo
// ---------------------------------------------------------------------------

#[test]
fn rc2_plan_apply_is_one_undo_entry() {
    let doc = document(vec![clip(1, 0, 0..60, Vec::new())]);
    let core = Core::spawn(doc.clone()).unwrap();
    let plan = kinewright_agent::plan_motion(
        &doc,
        kinewright_core::TimelineRevision(0),
        &kinewright_agent::MotionPlanArgs {
            expected_revision: kinewright_core::TimelineRevision(0),
            clip_id: ClipId(1),
            preset: kinewright_agent::MotionPreset::KenBurns,
            replace: false,
        },
    )
    .unwrap();
    assert!(plan.operations.len() >= 2, "AddEffect plus curves");
    let Event::DocumentChanged { doc: applied, .. } = core
        .request(Command::DoBatch(plan.operations.clone()))
        .unwrap()
    else {
        panic!("the plan applies");
    };
    assert_eq!(applied.tracks[0].clips[0].effects.len(), 1);
    assert_eq!(undo(&core), doc, "one undo reverts the whole plan");
}
