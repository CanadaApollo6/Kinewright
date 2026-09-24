//! Reviewer 2, MO1 Part C: timeline key lane and opacity band scenarios.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use kinewright_core::{
    AssetId, AutomationCurve, Clip, ClipContent, ClipId, Effect, EffectId, Keyframe,
    KeyframeInterpolation, Operation, ParamValue, TimeCode, apply_batch,
};

use super::*;

fn key(at: i64, value: i64) -> Keyframe {
    Keyframe {
        at: TimeCode(at),
        value,
        interpolation: KeyframeInterpolation::Linear,
        tangent_in: 0,
        tangent_out: 0,
    }
}

fn opacity_clip(keys: Vec<Keyframe>) -> Clip {
    Clip {
        enabled: true,
        enabled_curve: None,
        id: ClipId(1),
        asset: AssetId(1),
        source_range: TimeCode(0)..TimeCode(30),
        content: ClipContent::Media,
        timeline_start: TimeCode(0),
        effects: vec![Effect {
            enabled: true,
            enabled_curve: None,
            id: EffectId(1),
            name: "opacity".to_owned(),
            parameters: BTreeMap::from([("percent".to_owned(), ParamValue::Integer(80))]),
            keyframes: BTreeMap::from([(
                "percent".to_owned(),
                AutomationCurve { keyframes: keys },
            )]),
        }],
        transition_in: None,
        link: None,
        audio_gain_tenth_db: 0,
        audio_fade_in_frames: TimeCode::ZERO,
        audio_fade_out_frames: TimeCode::ZERO,
        speed_percent: 100,
        audio_gain_curve: None,
    }
}

/// R23: negatives are clipped from the lane; a tail-trimmed kept-outside key
/// (frame ≥ clip duration) should be clipped the same way, or its diamond is
/// painted past the clip's right edge over the neighbouring clip.
#[test]
fn rc2_lane_clips_keys_past_the_clip_end() {
    let clip = opacity_clip(vec![key(-20, 0), key(5, 50), key(45, 100)]);
    let diamonds = key_lane_diamonds(&clip, TimeCode(30));
    let clip_rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(30.0, 60.0));
    let points = key_lane_points(key_lane_strip(clip_rect), 0.0, &diamonds, 1.0);
    assert!(
        points.iter().all(|point| point.x <= clip_rect.right()),
        "diamonds painted outside the 30-frame clip: frames {diamonds:?}"
    );
}

/// Gate 10 with kept-outside keys on both sides: the band drag of an
/// interior key lands the same document as remove + upsert.
#[test]
fn rc2_band_drag_with_kept_outside_keys_matches_the_op_path() {
    let stored = vec![key(-20, 0), key(0, 50), key(15, 100), key(45, 30)];
    let mut document = linked_fixture_like(opacity_clip(stored.clone()));
    let duration = TimeCode(30);
    let moved = opacity_move_key(&stored, 2, TimeCode(29), 80, duration);
    let mut banded = document.clone();
    apply_batch(
        &mut banded,
        std::slice::from_ref(&opacity_operation(ClipId(1), EffectId(1), moved)),
    )
    .unwrap();
    apply_batch(
        &mut document,
        &[
            Operation::RemoveEffectKeyframe {
                clip: ClipId(1),
                effect: EffectId(1),
                name: "percent".to_owned(),
                at: TimeCode(15),
            },
            Operation::UpsertEffectKeyframe {
                clip: ClipId(1),
                effect: EffectId(1),
                name: "percent".to_owned(),
                key: key(29, 80),
            },
        ],
    )
    .unwrap();
    assert_eq!(banded, document);
}

fn linked_fixture_like(clip: Clip) -> kinewright_core::Document {
    let mut document = kinewright_core::Document {
        investigator: None,
        catalog: kinewright_core::MediaCatalog::default(),
        audio_mix: kinewright_core::AudioMix::default(),
        tracks: vec![kinewright_core::Track {
            id: kinewright_core::TrackId(1),
            kind: kinewright_core::TrackKind::Video,
            sync_lock: true,
            clips: vec![clip],
        }],
        media_pool: vec![kinewright_core::MediaAsset {
            id: AssetId(1),
            path: "fixture.mp4".into(),
            name: "fixture".into(),
            duration: TimeCode(600),
            fps: kinewright_core::Rational::new(30, 1).unwrap(),
            kind: kinewright_core::MediaKind::Video,
            resolution: Some((1920, 1080)),
            source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
            color_description: kinewright_core::ColorDescription::default(),
            assumed_from: None,
        }],
        markers: Vec::new(),
        fps: kinewright_core::Rational::new(30, 1).unwrap(),
        resolution: (1920, 1080),
        duration: TimeCode(30),
        color_context: kinewright_core::ColorContext::default(),
        lut_assets: Vec::new(),
    };
    apply_batch(&mut document, &[]).ok();
    document
}
