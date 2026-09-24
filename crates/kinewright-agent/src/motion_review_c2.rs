//! Reviewer 2, MO1 Part C: `plan_motion` scenarios across every preset.
#![allow(clippy::unwrap_used)]

use kinewright_core::{
    AssetId, AudioMix, ClipContent, ColorContext, ColorDescription, MediaAsset, MediaCatalog,
    MediaKind, MediaSourceFingerprint, Rational, Track, TrackId,
};

use super::*;

fn doc(duration: i64, effects: Vec<Effect>) -> Document {
    Document {
        investigator: None,
        catalog: MediaCatalog::default(),
        audio_mix: AudioMix::default(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![Clip {
                enabled: true,
                enabled_curve: None,
                id: ClipId(1),
                asset: AssetId(1),
                source_range: TimeCode::ZERO..TimeCode(duration),
                content: ClipContent::Media,
                timeline_start: TimeCode::ZERO,
                effects,
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
                audio_gain_curve: None,
            }],
        }],
        media_pool: vec![MediaAsset {
            id: AssetId(1),
            path: "f.mp4".into(),
            name: "f".into(),
            duration: TimeCode(100_000),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::Video,
            resolution: Some((1920, 1080)),
            source_fingerprint: MediaSourceFingerprint::default(),
            color_description: ColorDescription::default(),
            assumed_from: None,
        }],
        markers: Vec::new(),
        fps: Rational::new(30, 1).unwrap(),
        resolution: (1920, 1080),
        duration: TimeCode(duration),
        color_context: ColorContext::default(),
        lut_assets: Vec::new(),
    }
}

fn args(preset: MotionPreset, replace: bool) -> MotionPlanArgs {
    MotionPlanArgs {
        expected_revision: TimelineRevision(3),
        clip_id: ClipId(1),
        preset,
        replace,
    }
}

#[test]
fn rc2_every_preset_applies_is_deterministic_and_fits_budget() {
    for duration in [2_i64, 60, 99_999] {
        for (preset, name) in MOTION_PRESETS {
            let document = doc(duration, Vec::new());
            let a = plan_motion(&document, TimelineRevision(3), &args(preset, false)).unwrap();
            let b =
                plan_motion(&document.clone(), TimelineRevision(3), &args(preset, false)).unwrap();
            let bytes_a = serde_json::to_string(&a.operations).unwrap();
            let bytes_b = serde_json::to_string(&b.operations).unwrap();
            assert_eq!(bytes_a, bytes_b, "{name} deterministic");
            let mut applied = document.clone();
            apply_batch(&mut applied, &a.operations).unwrap();
            let value = serde_json::to_value(&a.operations).unwrap();
            match motion_response(&a, value) {
                MotionProposal::Fits { text, body } => {
                    let total = text.len() + body.to_string().len();
                    assert!(total <= MOTION_RESPONSE_BUDGET_BYTES, "{name} {total}");
                }
                MotionProposal::OverBudget { .. } => panic!("{name} over budget"),
            }
            for (_, count) in &a.key_counts {
                assert!(*count <= MOTION_MAX_KEYS_PER_CURVE);
            }
            // Re-plan on the applied doc: refused without replace (except pip),
            // accepted with replace, and replace is idempotent.
            if preset != MotionPreset::Pip {
                assert!(matches!(
                    plan_motion(&applied, TimelineRevision(3), &args(preset, false)),
                    Err(MotionPlanError::ExistingCurves { .. })
                ));
                let again =
                    plan_motion(&applied, TimelineRevision(3), &args(preset, true)).unwrap();
                let mut twice = applied.clone();
                apply_batch(&mut twice, &again.operations).unwrap();
                assert_eq!(twice, applied, "{name} replace is idempotent");
            }
        }
    }
}

/// The planner writes absolute values: a clip whose transform already holds
/// a non-neutral fine static (a baked still fit, or a person's 150.00%)
/// jumps on frame 0.
#[test]
fn rc2_push_in_starts_from_the_existing_fine_static() {
    let descriptor = effect_descriptor("transform").unwrap();
    let mut parameters: BTreeMap<String, ParamValue> = descriptor
        .parameters
        .iter()
        .map(|p| (p.name.to_owned(), ParamValue::Integer(p.neutral)))
        .collect();
    parameters.insert(
        "scale_fine_hundredths".to_owned(),
        ParamValue::Integer(15_000),
    );
    let document = doc(
        60,
        vec![Effect {
            enabled: true,
            enabled_curve: None,
            id: EffectId(1),
            name: "transform".to_owned(),
            parameters,
            keyframes: BTreeMap::new(),
        }],
    );
    let plan = plan_motion(
        &document,
        TimelineRevision(3),
        &args(MotionPreset::PushIn, false),
    )
    .unwrap();
    let Operation::SetEffectKeyframes { curve, .. } = &plan.operations[0] else {
        panic!("{:?}", plan.operations);
    };
    assert_eq!(
        curve.keyframes[0].value, 15_000,
        "push-in starts where the clip is"
    );
}

/// Decimation is index-uniform with no tolerance bound: a feature on a
/// dropped key vanishes entirely.
#[test]
fn rc2_decimation_keeps_the_curve_within_tolerance() {
    let keys: Vec<Keyframe> = (0..20)
        .map(|index| Keyframe {
            at: TimeCode(index * 10),
            value: if index == 1 { 10_000 } else { 0 },
            interpolation: KeyframeInterpolation::Linear,
            tangent_in: 0,
            tangent_out: 0,
        })
        .collect();
    let original = AutomationCurve {
        keyframes: keys.clone(),
    };
    let thinned = AutomationCurve {
        keyframes: decimate_keys(keys),
    };
    assert!(thinned.keyframes.len() <= MOTION_MAX_KEYS_PER_CURVE);
    let worst = (0..=190)
        .map(|frame| {
            (original.value_at(TimeCode(frame)).unwrap()
                - thinned.value_at(TimeCode(frame)).unwrap())
            .abs()
        })
        .max()
        .unwrap();
    assert!(worst <= 100, "max deviation {worst} of a 10 000 range");
}
