//! CC5 secondaries core contracts.
//!
//! These tests hold the parts of `docs/CC5-SECONDARIES.md` that Core owns at
//! step 1: the §2.1 matte-capable kinds, the §2.2 generated parameter tables,
//! the §2.6 inactivity rules and degenerate bands, the §5.1 keyframing policy,
//! the §5.2 tracker-smoothing primitive, and the serialization and QA that
//! follow from them. Every expected value is transcribed from the document by
//! hand rather than read back out of the descriptor tables.

use std::{collections::BTreeMap, path::PathBuf};

use kinewright_core::{
    AssetId, AutomationCurve, COLOR_CURVES_DESCRIPTOR_PARAMETER_COUNT,
    COLOR_WHEELS_DESCRIPTOR_PARAMETER_COUNT, CREATIVE_LOOK_DESCRIPTOR_PARAMETER_COUNT, Clip,
    ClipContent, ClipId, ColorContext, ColorDescription, ColorNodeInactiveReason, ColorNodeKind,
    Command, Core, Document, Effect, EffectId, EffectUniform, Event, JournalCommand, Keyframe,
    KeyframeInterpolation, LutAsset, LutAssetId, LutAssetKind, LutAssetSource,
    MATTE_HUE_WIDTH_DISABLE_CENTIDEGREES, MATTE_LUMA_BAND, MATTE_MIX_BASIS_POINTS_MAX,
    MATTE_PARAMETER_COUNT, MATTE_SATURATION_BAND, MATTE_WINDOW_LIMIT, MATTE_WINDOW_PARAMETER_COUNT,
    MatteParams, MediaAsset, MediaKind, OpError, Operation,
    PRIMARY_CORRECTION_DESCRIPTOR_PARAMETER_COUNT, ParamValue, QaSeverity, Rational, TimeCode,
    Track, TrackId, TrackKind, active_color_nodes, color_node_inactive_reason, effect_descriptor,
    is_hold_only_matte_parameter, is_matte_capable_color_node, is_matte_parameter,
    managed_color_node_count, matte_capable, matte_parameter_names, matte_parameters,
    matte_window_parameter_names, matte_window_parameters, qa_document,
    stabilize_tracked_centres_basis_points,
};

/// The four kinds CC5 §2.1 gives a matte, in stage order.
const MATTE_CAPABLE: [&str; 4] = [
    "primary_correction",
    "color_wheels",
    "color_curves",
    "creative_look",
];

/// How many controls each matte-capable kind owned before CC5.
const BASE_PARAMETER_COUNTS: [(&str, usize); 4] = [
    ("primary_correction", 10),
    ("color_wheels", 13),
    ("color_curves", 133),
    ("creative_look", 4),
];

/// A path that always exists so `missing_media` never masks a colour issue.
fn present_media_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")
}

fn supported_source_description() -> ColorDescription {
    ColorContext::sdr_rec709().delivery
}

fn managed_document() -> Document {
    let asset = MediaAsset {
        id: AssetId(1),
        path: present_media_path(),
        name: "managed-source".to_owned(),
        duration: TimeCode(120),
        fps: Rational::new(30, 1).unwrap(),
        kind: MediaKind::Video,
        resolution: Some((1_920, 1_080)),
        source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
        color_description: supported_source_description(),
    };
    Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![Clip {
                id: ClipId(1),
                asset: AssetId(1),
                source_range: TimeCode(0)..TimeCode(30),
                content: ClipContent::Media,
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
            }],
        }],
        media_pool: vec![asset],
        duration: TimeCode(30),
        ..Document::default()
    }
}

/// A 64-character lowercase hexadecimal digest derived from an id.
fn digest(seed: u64) -> String {
    format!("{seed:064x}")
}

/// A registered look, so a LUT node has something legal to reference.
fn imported_asset(id: u64) -> LutAsset {
    LutAsset {
        id: LutAssetId(id),
        sha256: digest(id),
        title: format!("Look {id}"),
        kind: LutAssetKind::Cube3d,
        size: 33,
        byte_len: 1_174_896,
        domain_min_millionths: [0, 0, 0],
        domain_max_millionths: [1_000_000, 1_000_000, 1_000_000],
        source: LutAssetSource::Imported {
            source_path: format!("/looks/look{id}.cube"),
        },
    }
}

/// A managed document that already owns one registered LUT asset.
fn document_with_asset() -> Document {
    let mut document = managed_document();
    Operation::AddLutAsset {
        asset: imported_asset(1),
    }
    .apply(&mut document)
    .expect("a well-formed asset registers");
    document
}

/// A neutral node of `kind`, bound to the registered asset when it is a LUT.
fn neutral_node(id: u64, kind: &str) -> Effect {
    if kind == "technical_lut" || kind == "creative_look" {
        effect(id, kind, &[("lut_asset_id", 1)])
    } else {
        effect(id, kind, &[])
    }
}

fn effect(id: u64, name: &str, parameters: &[(&str, i64)]) -> Effect {
    Effect {
        id: EffectId(id),
        name: name.to_owned(),
        parameters: parameters
            .iter()
            .map(|(name, value)| ((*name).to_owned(), ParamValue::Integer(*value)))
            .collect(),
        keyframes: BTreeMap::new(),
    }
}

fn add(document: &mut Document, effect: Effect) -> Result<(), OpError> {
    Operation::AddEffect {
        clip: ClipId(1),
        effect,
    }
    .apply(document)
}

fn set_param(document: &mut Document, id: u64, name: &str, value: i64) -> Result<(), OpError> {
    Operation::SetEffectParam {
        clip: ClipId(1),
        effect: EffectId(id),
        name: name.to_owned(),
        value: ParamValue::Integer(value),
    }
    .apply(document)
}

fn set_keyframes(
    document: &mut Document,
    id: u64,
    name: &str,
    keyframes: &[(i64, i64, KeyframeInterpolation)],
) -> Result<(), OpError> {
    Operation::SetEffectKeyframes {
        clip: ClipId(1),
        effect: EffectId(id),
        name: name.to_owned(),
        curve: curve(keyframes),
    }
    .apply(document)
}

fn curve(keyframes: &[(i64, i64, KeyframeInterpolation)]) -> AutomationCurve {
    AutomationCurve {
        keyframes: keyframes
            .iter()
            .map(|(at, value, interpolation)| Keyframe {
                at: TimeCode(*at),
                value: *value,
                interpolation: *interpolation,
            })
            .collect(),
    }
}

fn hand_edited_document(effects: Vec<Effect>) -> Document {
    let mut document = managed_document();
    document.tracks[0].clips[0].effects = effects;
    document
}

/// The §2.2 tables, transcribed by hand from the document and expanded from
/// the two published patterns rather than read out of the descriptors.
fn expected_matte_parameters() -> Vec<(String, i64, i64, i64)> {
    let mut expected: Vec<(String, i64, i64, i64)> = vec![
        ("matte_enabled".to_owned(), 0, 1, 0),
        ("matte_window_count".to_owned(), 0, 4, 0),
        ("matte_combine_token".to_owned(), 0, 1, 0),
        ("matte_invert".to_owned(), 0, 1, 0),
        ("matte_mix_basis_points".to_owned(), 0, 10_000, 10_000),
        ("matte_qualifier_enabled".to_owned(), 0, 1, 0),
        ("matte_hue_center_centidegrees".to_owned(), 0, 35_999, 0),
        ("matte_hue_width_centidegrees".to_owned(), 0, 18_000, 18_000),
        ("matte_hue_softness_centidegrees".to_owned(), 0, 18_000, 0),
        ("matte_saturation_low_basis_points".to_owned(), 0, 10_000, 0),
        (
            "matte_saturation_high_basis_points".to_owned(),
            0,
            10_000,
            10_000,
        ),
        (
            "matte_saturation_softness_basis_points".to_owned(),
            0,
            10_000,
            0,
        ),
        ("matte_luma_low_basis_points".to_owned(), 0, 10_000, 0),
        ("matte_luma_high_basis_points".to_owned(), 0, 10_000, 10_000),
        ("matte_luma_softness_basis_points".to_owned(), 0, 10_000, 0),
    ];
    for window in 0..4 {
        expected.push((format!("matte_window{window}_shape_token"), 1, 2, 1));
        expected.push((
            format!("matte_window{window}_center_x_basis_points"),
            -10_000,
            20_000,
            5_000,
        ));
        expected.push((
            format!("matte_window{window}_center_y_basis_points"),
            -10_000,
            20_000,
            5_000,
        ));
        expected.push((
            format!("matte_window{window}_half_width_basis_points"),
            1,
            10_000,
            2_500,
        ));
        expected.push((
            format!("matte_window{window}_half_height_basis_points"),
            1,
            10_000,
            2_500,
        ));
        expected.push((
            format!("matte_window{window}_rotation_centidegrees"),
            -18_000,
            18_000,
            0,
        ));
        expected.push((
            format!("matte_window{window}_feather_basis_points"),
            0,
            10_000,
            0,
        ));
        expected.push((format!("matte_window{window}_invert"), 0, 1, 0));
    }
    expected
}

// ---------------------------------------------------------------------------
// §2.1 / §2.2 descriptors
// ---------------------------------------------------------------------------

/// CC5 §2.2: 47 parameters generated from two patterns, identical on all four
/// matte-capable kinds and appended after each kind's own controls.
#[test]
fn every_matte_capable_kind_carries_the_forty_seven_generated_parameters() {
    let expected = expected_matte_parameters();
    assert_eq!(expected.len(), 47, "15 controls plus 4 x 8 window controls");
    assert_eq!(MATTE_PARAMETER_COUNT, 47);
    assert_eq!(MATTE_WINDOW_LIMIT, 4);
    assert_eq!(MATTE_WINDOW_PARAMETER_COUNT, 8);
    assert_eq!(MATTE_HUE_WIDTH_DISABLE_CENTIDEGREES, 18_000);
    assert_eq!(MATTE_MIX_BASIS_POINTS_MAX, 10_000);

    for (name, base) in BASE_PARAMETER_COUNTS {
        let descriptor = effect_descriptor(name).expect("a matte-capable kind is registered");
        assert_eq!(
            descriptor.parameters.len(),
            base + 47,
            "{name} keeps its own controls and gains 47"
        );
        for (parameter, (expected_name, min, max, neutral)) in
            descriptor.parameters[base..].iter().zip(&expected)
        {
            assert_eq!(
                (
                    parameter.name,
                    parameter.min,
                    parameter.max,
                    parameter.neutral,
                    parameter.uniform,
                ),
                (
                    expected_name.as_str(),
                    *min,
                    *max,
                    *neutral,
                    EffectUniform::ColorNode,
                ),
                "{name} / {expected_name}"
            );
        }
    }

    // The §2.2 descriptor sizes, written out.
    assert_eq!(PRIMARY_CORRECTION_DESCRIPTOR_PARAMETER_COUNT, 57);
    assert_eq!(COLOR_WHEELS_DESCRIPTOR_PARAMETER_COUNT, 60);
    assert_eq!(COLOR_CURVES_DESCRIPTOR_PARAMETER_COUNT, 180);
    assert_eq!(CREATIVE_LOOK_DESCRIPTOR_PARAMETER_COUNT, 51);
    for (name, size) in [
        ("primary_correction", 57),
        ("color_wheels", 60),
        ("color_curves", 180),
        ("creative_look", 51),
    ] {
        assert_eq!(
            effect_descriptor(name)
                .expect("registered")
                .parameters
                .len(),
            size
        );
    }

    // Spot checks written out in full so a pattern change cannot pass silently.
    let wheels = effect_descriptor("color_wheels").expect("color_wheels");
    let rotation = wheels
        .parameter("matte_window3_rotation_centidegrees")
        .expect("the fourth window owns a rotation");
    assert_eq!(
        (rotation.min, rotation.max, rotation.neutral),
        (-18_000, 18_000, 0)
    );
    let hue_width = wheels
        .parameter("matte_hue_width_centidegrees")
        .expect("hue width");
    assert_eq!(
        (hue_width.min, hue_width.max, hue_width.neutral),
        (0, 18_000, 18_000)
    );
    let mix = wheels
        .parameter("matte_mix_basis_points")
        .expect("matte mix");
    assert_eq!((mix.min, mix.max, mix.neutral), (0, 10_000, 10_000));
    let centre = wheels
        .parameter("matte_window0_center_x_basis_points")
        .expect("first window centre");
    assert_eq!(
        (centre.min, centre.max, centre.neutral),
        (-10_000, 20_000, 5_000)
    );
    let half_height = wheels
        .parameter("matte_window2_half_height_basis_points")
        .expect("third window half height");
    assert_eq!(
        (half_height.min, half_height.max, half_height.neutral),
        (1, 10_000, 2_500)
    );
    let shape = wheels
        .parameter("matte_window1_shape_token")
        .expect("second window shape");
    assert_eq!((shape.min, shape.max, shape.neutral), (1, 2, 1));
    assert!(wheels.parameter("matte_window4_shape_token").is_none());
    assert!(wheels.parameter("matte_blur_radius").is_none());
}

/// CC5 §2.2: the generated name tables agree with the descriptors.
#[test]
fn the_matte_name_helpers_list_exactly_the_generated_parameters() {
    let expected = expected_matte_parameters();
    assert_eq!(matte_parameter_names().len(), 47);
    assert_eq!(matte_parameters().len(), 47);
    for (name, (expected_name, _, _, _)) in matte_parameter_names().iter().zip(&expected) {
        assert_eq!(*name, expected_name.as_str());
    }
    for name in matte_parameter_names() {
        assert!(is_matte_parameter(name), "{name} is a matte parameter");
    }
    for window in 0..4 {
        let names = matte_window_parameter_names(window).expect("windows 0..=3 exist");
        let descriptors = matte_window_parameters(window).expect("windows 0..=3 exist");
        assert_eq!(names.len(), 8);
        assert_eq!(names[0], format!("matte_window{window}_shape_token"));
        assert_eq!(names[7], format!("matte_window{window}_invert"));
        for (name, descriptor) in names.iter().zip(descriptors) {
            assert_eq!(*name, descriptor.name);
            assert_eq!(descriptor.uniform, EffectUniform::ColorNode);
        }
    }
    assert!(matte_window_parameter_names(4).is_none());
    assert!(matte_window_parameters(4).is_none());

    for name in ["bypass", "mix_basis_points", "lut_asset_id", "master_x0"] {
        assert!(!is_matte_parameter(name), "{name} is not a matte parameter");
    }
}

/// CC5 §2.1: `technical_lut` carries no matte, and naming one is the ordinary
/// unknown-parameter rejection.
#[test]
fn technical_lut_carries_no_matte_parameter() {
    assert!(!ColorNodeKind::TechnicalLut.supports_matte());
    assert!(!matte_capable(ColorNodeKind::TechnicalLut));
    assert!(!is_matte_capable_color_node("technical_lut"));
    for kind in MATTE_CAPABLE {
        let node = ColorNodeKind::from_effect_name(kind).expect("registered kind");
        assert!(node.supports_matte(), "{kind} carries a matte");
        assert!(matte_capable(node));
        assert!(is_matte_capable_color_node(kind));
    }
    assert!(!is_matte_capable_color_node("mask"));

    let descriptor = effect_descriptor("technical_lut").expect("technical_lut");
    assert_eq!(descriptor.parameters.len(), 4);
    for name in matte_parameter_names() {
        assert!(
            descriptor.parameter(name).is_none(),
            "technical_lut must not declare {name}"
        );
    }

    let mut document = document_with_asset();
    add(&mut document, neutral_node(1, "technical_lut")).expect("a technical LUT node is legal");
    let before = document.clone();
    let error = set_param(&mut document, 1, "matte_enabled", 1)
        .expect_err("a matte parameter on technical_lut must be rejected");
    assert_eq!(
        error,
        OpError::UnknownEffectParam {
            effect: "technical_lut".to_owned(),
            name: "matte_enabled".to_owned(),
        }
    );
    assert_eq!(document, before, "a rejected SetEffectParam must be atomic");

    // The same name on a matte-capable kind is accepted, so the rejection is
    // about the kind and not about the name.
    let mut document = document_with_asset();
    add(&mut document, neutral_node(1, "creative_look")).expect("a look node is legal");
    set_param(&mut document, 1, "matte_enabled", 1).expect("creative_look carries a matte");
}

// ---------------------------------------------------------------------------
// §2.2 bounds and typed resolution
// ---------------------------------------------------------------------------

/// CC5 §2.2: every bound accepts its extremes and rejects one step beyond, on
/// every matte-capable kind.
#[test]
fn matte_bounds_accept_the_extremes_and_reject_one_step_beyond() {
    for kind in MATTE_CAPABLE {
        let mut document = document_with_asset();
        add(&mut document, neutral_node(1, kind)).expect("a neutral node is legal");
        for (name, min, max, _) in expected_matte_parameters() {
            for value in [min, max] {
                set_param(&mut document, 1, &name, value)
                    .unwrap_or_else(|error| panic!("{kind} / {name} = {value}: {error}"));
            }
            for actual in [min - 1, max + 1] {
                let before = document.clone();
                let error = set_param(&mut document, 1, &name, actual)
                    .expect_err("a value outside the descriptor range must be rejected");
                assert_eq!(
                    error,
                    OpError::EffectParamOutOfRange {
                        effect: kind.to_owned(),
                        name: name.clone(),
                        min,
                        max,
                        actual,
                    }
                );
                assert_eq!(document, before, "a rejected edit must be atomic");
            }
            // Leave the parameter at its maximum, which is where the document
            // invariant is exercised below.
            set_param(&mut document, 1, &name, max).expect("the maximum is legal");
        }
        document
            .validate()
            .expect("every matte parameter at its maximum is a valid document");
    }

    // The minima are equally valid, including `matte_window_count = 0` with
    // four fully populated windows behind it.
    for kind in MATTE_CAPABLE {
        let mut node = neutral_node(1, kind);
        for (name, min, _, _) in expected_matte_parameters() {
            node.parameters.insert(name, ParamValue::Integer(min));
        }
        let mut document = document_with_asset();
        document.tracks[0].clips[0].effects = vec![node];
        document
            .validate()
            .expect("every matte parameter at its minimum is a valid document");
    }
}

/// CC5 §2.2: an omitted parameter resolves to its neutral, and the resolved
/// type carries the stored integers unchanged.
#[test]
#[allow(clippy::too_many_lines)]
fn matte_params_resolve_omitted_parameters_to_their_neutrals() {
    let bare = effect(1, "color_wheels", &[]);
    let resolved = MatteParams::from_effect(&bare);
    assert_eq!(resolved, MatteParams::NEUTRAL);
    assert_eq!(resolved.enabled, 0);
    assert_eq!(resolved.window_count, 0);
    assert_eq!(resolved.combine_token, 0);
    assert_eq!(resolved.invert, 0);
    assert_eq!(resolved.mix_bp, 10_000);
    assert_eq!(resolved.qualifier.enabled, 0);
    assert_eq!(resolved.qualifier.hue_width_cd, 18_000);
    assert!(resolved.qualifier.hue_leg_disabled());
    assert_eq!(resolved.qualifier.sat_high_bp, 10_000);
    assert_eq!(resolved.qualifier.luma_high_bp, 10_000);
    for window in resolved.windows {
        assert_eq!(window.shape_token, 1);
        assert_eq!(window.center_x_bp, 5_000);
        assert_eq!(window.center_y_bp, 5_000);
        assert_eq!(window.half_width_bp, 2_500);
        assert_eq!(window.half_height_bp, 2_500);
        assert_eq!(window.rotation_cd, 0);
        assert_eq!(window.feather_bp, 0);
        assert_eq!(window.invert, 0);
        assert!(!window.is_ellipse());
    }
    assert!(!resolved.has_matte(), "a neutral matte is inactive");
    assert_eq!(resolved.active_windows().count(), 0);
    assert!(resolved.window(0).is_some());
    assert!(resolved.window(4).is_none());
    assert!(resolved.degenerate_bands().is_empty());
    assert!((resolved.mix() - 1.0).abs() < f32::EPSILON);

    // A populated sample, read back field by field.
    let populated = effect(
        2,
        "color_curves",
        &[
            ("matte_enabled", 1),
            ("matte_window_count", 2),
            ("matte_combine_token", 1),
            ("matte_invert", 1),
            ("matte_mix_basis_points", 6_000),
            ("matte_qualifier_enabled", 1),
            ("matte_hue_center_centidegrees", 35_000),
            ("matte_hue_width_centidegrees", 1_000),
            ("matte_hue_softness_centidegrees", 1_000),
            ("matte_saturation_low_basis_points", 8_000),
            ("matte_saturation_high_basis_points", 10_000),
            ("matte_saturation_softness_basis_points", 1_000),
            ("matte_luma_low_basis_points", 2_000),
            ("matte_luma_high_basis_points", 9_000),
            ("matte_luma_softness_basis_points", 500),
            ("matte_window0_shape_token", 2),
            ("matte_window0_center_x_basis_points", -10_000),
            ("matte_window0_center_y_basis_points", 20_000),
            ("matte_window0_half_width_basis_points", 1_125),
            ("matte_window0_half_height_basis_points", 2_000),
            ("matte_window0_rotation_centidegrees", 4_500),
            ("matte_window0_feather_basis_points", 4_000),
            ("matte_window0_invert", 1),
            ("matte_window1_center_x_basis_points", 7_500),
        ],
    );
    let resolved = MatteParams::from_effect(&populated);
    assert!(resolved.is_enabled());
    assert!(resolved.is_inverted());
    assert!(resolved.intersects());
    assert_eq!(resolved.window_count, 2);
    assert_eq!(resolved.mix_bp, 6_000);
    assert!((resolved.mix() - 0.6).abs() < 1e-6);
    assert_eq!(resolved.qualifier.hue_center_cd, 35_000);
    assert_eq!(resolved.qualifier.hue_width_cd, 1_000);
    assert!(!resolved.qualifier.hue_leg_disabled());
    assert_eq!(resolved.qualifier.sat_low_bp, 8_000);
    assert_eq!(resolved.qualifier.luma_softness_bp, 500);
    let first = resolved.windows[0];
    assert!(first.is_ellipse());
    assert!(first.is_inverted());
    assert_eq!(first.center_x_bp, -10_000);
    assert_eq!(first.center_y_bp, 20_000);
    assert_eq!(first.half_width_bp, 1_125);
    assert_eq!(first.half_height_bp, 2_000);
    assert_eq!(first.rotation_cd, 4_500);
    assert_eq!(first.feather_bp, 4_000);
    // The second window keeps its neutrals except the centre that was stored.
    assert_eq!(resolved.windows[1].center_x_bp, 7_500);
    assert_eq!(resolved.windows[1].center_y_bp, 5_000);
    assert_eq!(resolved.active_windows().count(), 2);
    assert_eq!(
        resolved
            .active_windows()
            .map(|window| window.center_x_bp)
            .collect::<Vec<_>>(),
        vec![-10_000, 7_500]
    );

    // A hostile stored value is clamped defensively rather than failing a
    // render, and a kind that carries no matte resolves to the neutral matte
    // whatever its file says.
    let hostile = effect(
        3,
        "color_wheels",
        &[
            ("matte_enabled", 9),
            ("matte_window_count", 11),
            ("matte_window0_half_width_basis_points", -4),
        ],
    );
    let resolved = MatteParams::from_effect(&hostile);
    assert_eq!(resolved.enabled, 1);
    assert_eq!(resolved.window_count, 4);
    assert_eq!(resolved.windows[0].half_width_bp, 1);
    let technical = effect(4, "technical_lut", &[("matte_enabled", 1)]);
    assert_eq!(MatteParams::from_effect(&technical), MatteParams::NEUTRAL);
    let unmanaged = effect(5, "mask", &[("matte_enabled", 1)]);
    assert_eq!(MatteParams::from_effect(&unmanaged), MatteParams::NEUTRAL);
}

// ---------------------------------------------------------------------------
// §5.1 keyframe policy
// ---------------------------------------------------------------------------

/// CC5 §5.1: matte tokens and counts accept only `Hold` keyframes.
#[test]
fn matte_tokens_and_counts_accept_only_hold_keyframes() {
    let hold_only: Vec<String> = {
        let mut names = vec![
            "matte_enabled".to_owned(),
            "matte_window_count".to_owned(),
            "matte_combine_token".to_owned(),
            "matte_invert".to_owned(),
            "matte_qualifier_enabled".to_owned(),
        ];
        for window in 0..4 {
            names.push(format!("matte_window{window}_shape_token"));
            names.push(format!("matte_window{window}_invert"));
        }
        names
    };
    assert_eq!(hold_only.len(), 13);
    for name in &hold_only {
        assert!(is_hold_only_matte_parameter(name), "{name} is Hold-only");
    }

    for kind in MATTE_CAPABLE {
        let mut document = document_with_asset();
        add(&mut document, neutral_node(1, kind)).expect("a neutral node is legal");
        for name in &hold_only {
            let (_, min, max, _) = expected_matte_parameters()
                .into_iter()
                .find(|(parameter, _, _, _)| parameter == name)
                .expect("every Hold-only name is a matte parameter");
            set_keyframes(
                &mut document,
                1,
                name,
                &[
                    (0, min, KeyframeInterpolation::Hold),
                    (10, max, KeyframeInterpolation::Hold),
                ],
            )
            .unwrap_or_else(|error| panic!("{kind} / {name} accepts Hold: {error}"));

            let before = document.clone();
            let error = set_keyframes(
                &mut document,
                1,
                name,
                &[
                    (0, min, KeyframeInterpolation::Hold),
                    (10, max, KeyframeInterpolation::Linear),
                ],
            )
            .expect_err("an interpolated token or count must be rejected");
            assert_eq!(
                error,
                OpError::NonHoldKeyframeParameter {
                    effect: kind.to_owned(),
                    name: name.clone(),
                }
            );
            assert_eq!(document, before, "a rejected keyframe edit must be atomic");
        }

        // The mix, the window geometry, and every qualifier scalar keep every
        // interpolation.
        for name in [
            "matte_mix_basis_points",
            "matte_window0_center_x_basis_points",
            "matte_window3_center_y_basis_points",
            "matte_window1_half_width_basis_points",
            "matte_window2_rotation_centidegrees",
            "matte_window0_feather_basis_points",
            "matte_hue_center_centidegrees",
            "matte_hue_width_centidegrees",
            "matte_hue_softness_centidegrees",
            "matte_saturation_low_basis_points",
            "matte_saturation_high_basis_points",
            "matte_saturation_softness_basis_points",
            "matte_luma_low_basis_points",
            "matte_luma_high_basis_points",
            "matte_luma_softness_basis_points",
        ] {
            assert!(!is_hold_only_matte_parameter(name), "{name} is keyframable");
            set_keyframes(
                &mut document,
                1,
                name,
                &[
                    (0, 1_000, KeyframeInterpolation::Linear),
                    (10, 2_000, KeyframeInterpolation::EaseInOut),
                ],
            )
            .unwrap_or_else(|error| panic!("{kind} / {name} accepts Linear: {error}"));
        }
    }
}

/// CC5 §5.1: keyframed window motion resolves per frame through
/// `Effect::evaluated_at`.
#[test]
fn keyframed_window_motion_resolves_per_frame() {
    let mut document = managed_document();
    add(
        &mut document,
        effect(
            1,
            "color_wheels",
            &[
                ("matte_enabled", 1),
                ("matte_window_count", 1),
                ("gain_master_thousandths", 1_500),
            ],
        ),
    )
    .expect("a windowed wheels node is legal");
    set_keyframes(
        &mut document,
        1,
        "matte_window0_center_x_basis_points",
        &[
            (0, 2_500, KeyframeInterpolation::Linear),
            (20, 7_500, KeyframeInterpolation::Linear),
        ],
    )
    .expect("a window centre is fully keyframable");

    let stored = document.clip(ClipId(1)).expect("clip").effects[0].clone();
    // 2500 -> 7500 over twenty frames: a quarter of the way is 3750, half is
    // 5000, and the value holds at each end.
    for (frame, expected) in [
        (0, 2_500),
        (5, 3_750),
        (10, 5_000),
        (20, 7_500),
        (29, 7_500),
    ] {
        let resolved = MatteParams::from_effect(&stored.evaluated_at(TimeCode(frame)));
        assert_eq!(
            resolved.windows[0].center_x_bp, expected,
            "frame {frame} resolves the window centre"
        );
        assert_eq!(resolved.window_count, 1);
        assert!(resolved.has_matte());
    }
}

// ---------------------------------------------------------------------------
// §2.6 inactive mattes and inactive nodes
// ---------------------------------------------------------------------------

/// CC5 §2.6: the inactivity truth table, read off the stored integers.
#[test]
fn the_matte_inactivity_truth_table_is_read_off_the_stored_integers() {
    // enabled = 0 makes every other control irrelevant, including mix 0.
    let disabled = effect(
        1,
        "color_wheels",
        &[
            ("matte_enabled", 0),
            ("matte_mix_basis_points", 0),
            ("matte_window_count", 4),
            ("matte_invert", 1),
        ],
    );
    let resolved = MatteParams::from_effect(&disabled);
    assert!(resolved.is_inactive_matte());
    assert!(!resolved.has_matte());
    assert!(!resolved.node_excluded_by_matte());

    // enabled = 1, mix = 0: the node is excluded.
    let zero_mix = effect(
        2,
        "color_wheels",
        &[
            ("matte_enabled", 1),
            ("matte_window_count", 1),
            ("matte_mix_basis_points", 0),
        ],
    );
    let resolved = MatteParams::from_effect(&zero_mix);
    assert!(!resolved.is_inactive_matte());
    assert!(resolved.node_excluded_by_matte());

    // enabled = 1, no window, no qualifier, invert = 1: m = 0 everywhere.
    let inverted_empty = effect(
        3,
        "color_wheels",
        &[("matte_enabled", 1), ("matte_invert", 1)],
    );
    let resolved = MatteParams::from_effect(&inverted_empty);
    assert!(!resolved.is_inactive_matte());
    assert!(resolved.node_excluded_by_matte());

    // enabled = 1 with one window: an ordinary active matte.
    let windowed = effect(
        4,
        "color_wheels",
        &[("matte_enabled", 1), ("matte_window_count", 1)],
    );
    let resolved = MatteParams::from_effect(&windowed);
    assert!(resolved.has_matte());
    assert!(!resolved.node_excluded_by_matte());

    // enabled = 1 with a qualifier and nothing else is active too.
    let qualified = effect(
        5,
        "color_wheels",
        &[("matte_enabled", 1), ("matte_qualifier_enabled", 1)],
    );
    assert!(MatteParams::from_effect(&qualified).has_matte());

    // enabled = 1 but neutral: no window, no qualifier, invert 0, full mix.
    let neutral_but_enabled = effect(
        6,
        "color_wheels",
        &[
            ("matte_enabled", 1),
            ("matte_invert", 0),
            ("matte_mix_basis_points", 10_000),
        ],
    );
    let resolved = MatteParams::from_effect(&neutral_but_enabled);
    assert!(
        resolved.is_inactive_matte(),
        "an enabled matte that selects everything at full strength is inactive"
    );
    assert!(!resolved.node_excluded_by_matte());

    // A partial mix alone is an active matte: it is the node's strength.
    let partial = effect(
        7,
        "color_wheels",
        &[("matte_enabled", 1), ("matte_mix_basis_points", 5_000)],
    );
    let resolved = MatteParams::from_effect(&partial);
    assert!(resolved.has_matte());
    assert!(!resolved.node_excluded_by_matte());
}

/// CC5 §2.6: `MatteExcluded` joins the inactive reasons after bypass, neutral,
/// and unbound.
#[test]
#[allow(clippy::too_many_lines)]
fn a_matte_excluded_node_reports_the_new_inactive_reason() {
    assert_eq!(
        ColorNodeInactiveReason::MatteExcluded.as_str(),
        "matte_excluded"
    );
    assert_eq!(ColorNodeInactiveReason::Bypassed.as_str(), "bypassed");
    assert_eq!(ColorNodeInactiveReason::Neutral.as_str(), "neutral");
    assert_eq!(ColorNodeInactiveReason::Unbound.as_str(), "unbound");

    // A graded node with a zero-mix matte is excluded on every matte-capable
    // kind, `primary_correction` included.
    let cases: [(&str, Effect); 4] = [
        (
            "primary_correction",
            effect(
                1,
                "primary_correction",
                &[
                    ("exposure_milli_stops", 250),
                    ("matte_enabled", 1),
                    ("matte_window_count", 1),
                    ("matte_mix_basis_points", 0),
                ],
            ),
        ),
        (
            "color_wheels",
            effect(
                2,
                "color_wheels",
                &[
                    ("gain_master_thousandths", 1_500),
                    ("matte_enabled", 1),
                    ("matte_invert", 1),
                ],
            ),
        ),
        (
            "color_curves",
            effect(
                3,
                "color_curves",
                &[
                    ("master_x1", 4_000),
                    ("matte_enabled", 1),
                    ("matte_mix_basis_points", 0),
                ],
            ),
        ),
        (
            "creative_look",
            effect(
                4,
                "creative_look",
                &[
                    ("lut_asset_id", 7),
                    ("matte_enabled", 1),
                    ("matte_mix_basis_points", 0),
                ],
            ),
        ),
    ];
    for (name, node) in cases {
        assert_eq!(
            color_node_inactive_reason(&node),
            Some(ColorNodeInactiveReason::MatteExcluded),
            "{name} is excluded by its matte"
        );
    }

    // Bypass wins over the matte rule, so an already-identity node keeps
    // reporting the cause it had before CC5.
    let bypassed = effect(
        5,
        "color_wheels",
        &[
            ("gain_master_thousandths", 1_500),
            ("bypass", 1),
            ("matte_enabled", 1),
            ("matte_mix_basis_points", 0),
        ],
    );
    assert_eq!(
        color_node_inactive_reason(&bypassed),
        Some(ColorNodeInactiveReason::Bypassed)
    );
    // A neutral node stays `Neutral` rather than becoming `MatteExcluded`.
    let neutral = effect(
        6,
        "color_wheels",
        &[("matte_enabled", 1), ("matte_mix_basis_points", 0)],
    );
    assert_eq!(
        color_node_inactive_reason(&neutral),
        Some(ColorNodeInactiveReason::Neutral)
    );
    // An unbound look stays `Unbound`.
    let unbound = effect(
        7,
        "creative_look",
        &[("matte_enabled", 1), ("matte_mix_basis_points", 0)],
    );
    assert_eq!(
        color_node_inactive_reason(&unbound),
        Some(ColorNodeInactiveReason::Unbound)
    );

    // A matte-capable node with a live matte stays active, and an excluded
    // node drops out of the renderer's list while keeping its slot.
    let active = effect(
        8,
        "color_wheels",
        &[
            ("gain_master_thousandths", 1_500),
            ("matte_enabled", 1),
            ("matte_window_count", 1),
        ],
    );
    assert_eq!(color_node_inactive_reason(&active), None);
    let excluded = effect(
        9,
        "color_wheels",
        &[
            ("gain_master_thousandths", 1_500),
            ("matte_enabled", 1),
            ("matte_mix_basis_points", 0),
        ],
    );
    let stack = vec![active.clone(), excluded];
    assert_eq!(
        active_color_nodes(&stack),
        vec![(0, ColorNodeKind::Wheels)],
        "an excluded node is not written to the buffer"
    );
    assert_eq!(
        managed_color_node_count(&stack),
        2,
        "an excluded node still occupies one of the sixteen slots"
    );

    // `matte_enabled = 0` with `matte_mix = 0` renders unmasked at full
    // strength: every other matte control is ignored when the switch is off.
    let switched_off = effect(
        10,
        "color_wheels",
        &[
            ("gain_master_thousandths", 1_500),
            ("matte_enabled", 0),
            ("matte_mix_basis_points", 0),
        ],
    );
    assert_eq!(color_node_inactive_reason(&switched_off), None);
    assert!(!MatteParams::from_effect(&switched_off).has_matte());
}

/// CC5 §2.6: a band whose low edge resolves above its high edge is reported,
/// never clamped or reordered.
#[test]
fn degenerate_bands_are_reported_without_clamping() {
    let inverted = effect(
        1,
        "color_wheels",
        &[
            ("matte_enabled", 1),
            ("matte_qualifier_enabled", 1),
            ("matte_saturation_low_basis_points", 8_000),
            ("matte_saturation_high_basis_points", 2_000),
        ],
    );
    let resolved = MatteParams::from_effect(&inverted);
    assert_eq!(resolved.degenerate_bands(), vec![MATTE_SATURATION_BAND]);
    assert!(resolved.qualifier.saturation_band_inverted());
    assert!(!resolved.qualifier.luma_band_inverted());
    // The stored integers are untouched: no clamping, no reordering.
    assert_eq!(resolved.qualifier.sat_low_bp, 8_000);
    assert_eq!(resolved.qualifier.sat_high_bp, 2_000);

    let both = effect(
        2,
        "color_curves",
        &[
            ("matte_enabled", 1),
            ("matte_qualifier_enabled", 1),
            ("matte_saturation_low_basis_points", 10_000),
            ("matte_saturation_high_basis_points", 0),
            ("matte_luma_low_basis_points", 6_000),
            ("matte_luma_high_basis_points", 5_999),
        ],
    );
    assert_eq!(
        MatteParams::from_effect(&both).degenerate_bands(),
        vec![MATTE_SATURATION_BAND, MATTE_LUMA_BAND]
    );

    // Equal edges are a legal one-point band, not a degenerate one.
    let touching = effect(
        3,
        "color_wheels",
        &[
            ("matte_enabled", 1),
            ("matte_qualifier_enabled", 1),
            ("matte_saturation_low_basis_points", 5_000),
            ("matte_saturation_high_basis_points", 5_000),
        ],
    );
    assert!(
        MatteParams::from_effect(&touching)
            .degenerate_bands()
            .is_empty()
    );
}

// ---------------------------------------------------------------------------
// QA
// ---------------------------------------------------------------------------

/// CC5 §2.6: QA reports an inverted band as a non-blocking warning, at frame
/// zero or at any keyframe frame of the band parameters.
#[test]
fn qa_reports_a_matte_band_inverted_by_automation() {
    let document = hand_edited_document(vec![effect(
        1,
        "color_wheels",
        &[
            ("gain_master_thousandths", 1_500),
            ("matte_enabled", 1),
            ("matte_qualifier_enabled", 1),
            ("matte_saturation_low_basis_points", 8_000),
            ("matte_saturation_high_basis_points", 2_000),
        ],
    )]);
    let report = qa_document(&document);
    let warning = report
        .issues
        .iter()
        .find(|issue| issue.code == "matte_band_inverted_by_automation")
        .expect("a statically inverted band is reported");
    assert_eq!(warning.severity, QaSeverity::Warning);
    assert_eq!(warning.clip, Some(ClipId(1)));
    assert_eq!(warning.track, Some(TrackId(1)));
    assert_eq!(
        warning.range,
        Some(TimeCode(0)..TimeCode(1)),
        "the static document is reported at frame zero"
    );
    assert!(warning.message.contains("saturation"));
    assert!(
        report
            .issues
            .iter()
            .all(|issue| issue.severity != QaSeverity::Error),
        "an inverted band is never blocking"
    );

    // Automation that crosses the edges later is caught at the keyframe frame.
    let mut animated = hand_edited_document(vec![effect(
        1,
        "color_curves",
        &[
            ("master_x1", 4_000),
            ("matte_enabled", 1),
            ("matte_qualifier_enabled", 1),
        ],
    )]);
    set_keyframes(
        &mut animated,
        1,
        "matte_luma_low_basis_points",
        &[
            (0, 0, KeyframeInterpolation::Linear),
            (12, 9_000, KeyframeInterpolation::Linear),
        ],
    )
    .expect("a band edge is fully keyframable");
    set_keyframes(
        &mut animated,
        1,
        "matte_luma_high_basis_points",
        &[
            (0, 10_000, KeyframeInterpolation::Linear),
            (12, 1_000, KeyframeInterpolation::Linear),
        ],
    )
    .expect("a band edge is fully keyframable");
    let warning = qa_document(&animated)
        .issues
        .into_iter()
        .find(|issue| issue.code == "matte_band_inverted_by_automation")
        .expect("automation that inverts a band is reported");
    assert_eq!(warning.range, Some(TimeCode(12)..TimeCode(13)));
    assert!(warning.message.contains("luma"));
}

/// CC5 §2.6: nothing is reported for a bypassed node, an inactive matte, a
/// disabled qualifier, or a well-ordered band.
#[test]
fn qa_stays_silent_for_inactive_mattes_and_ordered_bands() {
    let inverted_band: [(&str, i64); 2] = [
        ("matte_saturation_low_basis_points", 8_000),
        ("matte_saturation_high_basis_points", 2_000),
    ];
    let cases: [(&str, Vec<(&str, i64)>); 4] = [
        (
            "a bypassed node is the identity",
            [
                ("bypass", 1),
                ("matte_enabled", 1),
                ("matte_qualifier_enabled", 1),
            ]
            .into_iter()
            .chain(inverted_band)
            .collect(),
        ),
        (
            "an inactive matte is never evaluated",
            [("matte_enabled", 0), ("matte_qualifier_enabled", 1)]
                .into_iter()
                .chain(inverted_band)
                .collect(),
        ),
        (
            "a disabled qualifier has no bands",
            [("matte_enabled", 1), ("matte_qualifier_enabled", 0)]
                .into_iter()
                .chain(inverted_band)
                .collect(),
        ),
        (
            "a well-ordered band is ordinary",
            vec![
                ("matte_enabled", 1),
                ("matte_qualifier_enabled", 1),
                ("matte_saturation_low_basis_points", 2_000),
                ("matte_saturation_high_basis_points", 8_000),
            ],
        ),
    ];
    for (reason, parameters) in cases {
        let document = hand_edited_document(vec![effect(1, "color_wheels", &parameters)]);
        assert!(
            qa_document(&document)
                .issues
                .iter()
                .all(|issue| issue.code != "matte_band_inverted_by_automation"),
            "{reason}"
        );
    }
}

// ---------------------------------------------------------------------------
// §9.2.14 serialization and history
// ---------------------------------------------------------------------------

/// CC5 §9.2.14: a two-window matte with a qualifier survives save, reopen,
/// journal replay, undo, and redo byte-for-byte.
#[test]
#[allow(clippy::too_many_lines)]
fn a_two_window_matte_survives_save_reopen_replay_and_undo() {
    let initial = managed_document();
    let operations = vec![
        Operation::AddEffect {
            clip: ClipId(1),
            effect: effect(
                1,
                "color_wheels",
                &[
                    ("gain_master_thousandths", 1_500),
                    ("matte_enabled", 1),
                    ("matte_window_count", 2),
                ],
            ),
        },
        Operation::SetEffectParam {
            clip: ClipId(1),
            effect: EffectId(1),
            name: "matte_combine_token".to_owned(),
            value: ParamValue::Integer(1),
        },
        Operation::SetEffectParam {
            clip: ClipId(1),
            effect: EffectId(1),
            name: "matte_window0_shape_token".to_owned(),
            value: ParamValue::Integer(2),
        },
        Operation::SetEffectParam {
            clip: ClipId(1),
            effect: EffectId(1),
            name: "matte_window1_center_x_basis_points".to_owned(),
            value: ParamValue::Integer(7_500),
        },
        Operation::SetEffectParam {
            clip: ClipId(1),
            effect: EffectId(1),
            name: "matte_qualifier_enabled".to_owned(),
            value: ParamValue::Integer(1),
        },
        Operation::SetEffectParam {
            clip: ClipId(1),
            effect: EffectId(1),
            name: "matte_hue_center_centidegrees".to_owned(),
            value: ParamValue::Integer(35_000),
        },
        Operation::SetEffectParam {
            clip: ClipId(1),
            effect: EffectId(1),
            name: "matte_hue_width_centidegrees".to_owned(),
            value: ParamValue::Integer(1_000),
        },
        Operation::SetEffectKeyframes {
            clip: ClipId(1),
            effect: EffectId(1),
            name: "matte_window0_center_x_basis_points".to_owned(),
            curve: curve(&[
                (0, 2_500, KeyframeInterpolation::Linear),
                (20, 7_500, KeyframeInterpolation::Linear),
            ]),
        },
        Operation::SetEffectKeyframes {
            clip: ClipId(1),
            effect: EffectId(1),
            name: "matte_window0_invert".to_owned(),
            curve: curve(&[
                (0, 0, KeyframeInterpolation::Hold),
                (15, 1, KeyframeInterpolation::Hold),
            ]),
        },
        Operation::ClearEffectKeyframes {
            clip: ClipId(1),
            effect: EffectId(1),
            name: "matte_window0_invert".to_owned(),
        },
    ];

    let core = Core::spawn(initial.clone()).expect("core should spawn");
    let mut journaled = Vec::new();
    let mut live = None;
    for operation in &operations {
        let Event::DocumentChanged {
            doc,
            journal_command: Some(command),
            ..
        } = core.request(Command::Do(operation.clone())).unwrap()
        else {
            panic!("every CC5 operation must be accepted and journaled");
        };
        assert_eq!(command, JournalCommand::Do(operation.clone()));
        journaled.push(command);
        live = Some(doc);
    }
    let live = live.expect("operations produce a document");

    let stored = &live.clip(ClipId(1)).expect("clip").effects[0];
    let resolved = MatteParams::from_effect(&stored.evaluated_at(TimeCode(0)));
    assert_eq!(resolved.window_count, 2);
    assert!(resolved.intersects());
    assert!(resolved.windows[0].is_ellipse());
    assert_eq!(resolved.windows[1].center_x_bp, 7_500);
    assert_eq!(resolved.qualifier.hue_center_cd, 35_000);
    assert!(
        !stored.keyframes.contains_key("matte_window0_invert"),
        "ClearEffectKeyframes removes the curve"
    );
    assert!(
        stored
            .keyframes
            .contains_key("matte_window0_center_x_basis_points")
    );

    // Save and reopen: the JSON is the whole state.
    let saved = serde_json::to_string(&*live).expect("document serializes");
    let reopened: Document = serde_json::from_str(&saved).expect("document reopens");
    assert_eq!(&reopened, &*live);
    assert_eq!(
        serde_json::to_string(&reopened).unwrap(),
        saved,
        "reopening and re-saving is byte-for-byte identical"
    );

    // Journal replay reproduces the same document from the same commands.
    let replay = Core::spawn(initial.clone()).expect("core should spawn");
    let mut replayed = None;
    for command in &journaled {
        let encoded = serde_json::to_string(command).expect("journal command serializes");
        let parsed: JournalCommand = serde_json::from_str(&encoded).expect("and parses back");
        let Event::DocumentChanged { doc, .. } = replay
            .request(parsed.into())
            .expect("a journaled CC5 operation replays")
        else {
            panic!("replay should reach the same state");
        };
        replayed = Some(doc);
    }
    assert_eq!(
        serde_json::to_string(&*replayed.expect("replay produces a document")).unwrap(),
        saved,
        "replay is byte-for-byte identical"
    );

    // Undo every applied step, then redo every one of them.
    let mut undone = None;
    for _ in 0..operations.len() {
        let Event::DocumentChanged { doc, .. } = core.request(Command::Undo).unwrap() else {
            panic!("every applied step must undo");
        };
        undone = Some(doc);
    }
    assert_eq!(
        &*undone.expect("undo produces a document"),
        &initial,
        "undo returns to the opening document"
    );
    let mut redone = None;
    for _ in 0..operations.len() {
        let Event::DocumentChanged { doc, .. } = core.request(Command::Redo).unwrap() else {
            panic!("every undone step must redo");
        };
        redone = Some(doc);
    }
    assert_eq!(
        serde_json::to_string(&*redone.expect("redo produces a document")).unwrap(),
        saved,
        "undo and redo restore the matte byte-for-byte"
    );
}

// ---------------------------------------------------------------------------
// §5.2 tracker smoothing
// ---------------------------------------------------------------------------

/// CC5 §5.2: the promoted smoothing primitive is the M40 algorithm unchanged.
///
/// The expectations are computed by hand from the two published stages: a
/// three-sample median filter whose *final* sample is
/// `median(o[n-3], o[n-2], o[n-1])`, then a reactive controller that steps by
/// at most `maximum_step` toward the filtered observation.
#[test]
fn stabilize_tracked_centres_matches_the_m40_algorithm() {
    // Raw observations with a one-sample spike at index 2 and a sustained move
    // afterwards.
    let observations = [1_000, 1_000, 9_000, 1_000, 5_000, 5_200, 5_400];
    // Median filter, by hand:
    //   [0] untouched                                       -> 1000
    //   [1] median(1000, 1000, 9000)                        -> 1000
    //   [2] median(1000, 9000, 1000)                        -> 1000  (spike gone)
    //   [3] median(9000, 1000, 5000)                        -> 5000
    //   [4] median(1000, 5000, 5200)                        -> 5000
    //   [5] median(5000, 5200, 5400)                        -> 5200
    //   [6] median(o[4], o[5], o[6]) = median(5000, 5200, 5400) -> 5200
    // Reactive controller from focus = 1000, dead_zone = 0, max_step = 800:
    //   1000, 1000, 1000, 1800, 2600, 3400, 4200
    let smoothed = stabilize_tracked_centres_basis_points(&observations, -10_000, 20_000, 0, 800);
    assert_eq!(
        smoothed,
        vec![1_000, 1_000, 1_000, 1_800, 2_600, 3_400, 4_200]
    );
    assert_ne!(
        smoothed[6], observations[6],
        "the last sample lags by one inter-sample displacement (the median rule)"
    );
    assert!(
        smoothed
            .windows(2)
            .all(|pair| pair[0].abs_diff(pair[1]) <= 800),
        "no step exceeds MATTE_TRACK_MAX_STEP_BASIS_POINTS"
    );

    // The bounds clamp both the observations and the controller state.
    assert_eq!(
        stabilize_tracked_centres_basis_points(&observations, 0, 2_000, 0, 800),
        vec![1_000, 1_000, 1_000, 1_800, 2_000, 2_000, 2_000]
    );

    // The multicam call, with M40's dead zone, is byte-identical across the
    // rename: this is the existing `subject_reframe_rejects_a_last_frame_
    // tracking_outlier` expectation, now asserted through the public name.
    assert_eq!(
        stabilize_tracked_centres_basis_points(&[50, 50, 34, 35, 80], 25, 75, 6, 25),
        vec![50, 50, 41, 41, 41]
    );

    // Degenerate inputs stay total.
    assert!(stabilize_tracked_centres_basis_points(&[], 0, 10_000, 0, 800).is_empty());
    assert_eq!(
        stabilize_tracked_centres_basis_points(&[4_200], 0, 10_000, 0, 800),
        vec![4_200]
    );
    assert_eq!(
        stabilize_tracked_centres_basis_points(&[4_200, 9_000], 0, 10_000, 0, 800),
        vec![4_200, 5_000],
        "fewer than three samples skip the median filter entirely"
    );
}
