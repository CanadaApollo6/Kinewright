//! CC3 curves-and-wheels core contracts.
//!
//! These tests hold the parts of `docs/CC3-CURVES-AND-WHEELS.md` that Core
//! owns: the §4 control tables, the §2.3 point representation and its
//! validation, the §3.1 node limit, the §3.3 inactivity rules, the §3.4
//! truncation rule, the §6 keyframing policy, and the QA reporting that
//! follows from them. Expected values are transcribed from the document by
//! hand rather than read back out of the descriptor tables.

use std::{collections::BTreeMap, path::PathBuf};

use kinewright_core::{
    AssetId, AutomationCurve, COLOR_CURVE_PARAMETER_COUNT, COLOR_CURVES_PARAMETER_COUNT,
    COLOR_NODE_LIMIT_PER_LAYER, Clip, ClipContent, ClipId, ColorContext, ColorCurveChannel,
    ColorDescription, ColorNodeInactiveReason, ColorNodeKind, ColorWheelsParams, Command, Core,
    CurvePoints, DeliveryProfile, Document, Effect, EffectId, EffectUniform, Event, JournalCommand,
    Keyframe, KeyframeInterpolation, MANAGED_COLOR_NODE_NAMES, MediaAsset, MediaKind, OpError,
    Operation, ParamValue, QaSeverity, Rational, ResolvedCurves, TimeCode, Track, TrackId,
    TrackKind, active_color_nodes, classify_color_node, color_curve_parameter_names,
    color_node_inactive_reason, delivery_conformance, effect_compatibility_stage,
    effect_descriptor, is_managed_color_node, managed_color_node_count, qa_document,
};

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
        curve: AutomationCurve {
            keyframes: keyframes
                .iter()
                .map(|(at, value, interpolation)| Keyframe {
                    at: TimeCode(*at),
                    value: *value,
                    interpolation: *interpolation,
                })
                .collect(),
        },
    }
    .apply(document)
}

/// CC3 §4.1, transcribed by hand from the control table.
#[test]
fn color_wheels_descriptor_matches_the_published_control_table() {
    let descriptor = effect_descriptor("color_wheels").expect("color_wheels must be registered");
    let expected: [(&str, i64, i64, i64); 13] = [
        ("lift_master_basis_points", -2_000, 2_000, 0),
        ("lift_red_basis_points", -2_000, 2_000, 0),
        ("lift_green_basis_points", -2_000, 2_000, 0),
        ("lift_blue_basis_points", -2_000, 2_000, 0),
        ("gamma_master_thousandths", 100, 4_000, 1_000),
        ("gamma_red_thousandths", 100, 4_000, 1_000),
        ("gamma_green_thousandths", 100, 4_000, 1_000),
        ("gamma_blue_thousandths", 100, 4_000, 1_000),
        ("gain_master_thousandths", 0, 4_000, 1_000),
        ("gain_red_thousandths", 0, 4_000, 1_000),
        ("gain_green_thousandths", 0, 4_000, 1_000),
        ("gain_blue_thousandths", 0, 4_000, 1_000),
        ("bypass", 0, 1, 0),
    ];

    assert_eq!(descriptor.parameters.len(), expected.len());
    for (parameter, (name, min, max, neutral)) in descriptor.parameters.iter().zip(expected) {
        assert_eq!(
            (
                parameter.name,
                parameter.min,
                parameter.max,
                parameter.neutral
            ),
            (name, min, max, neutral)
        );
        assert_eq!(
            parameter.uniform,
            EffectUniform::ColorNode,
            "{name} must be consumed by the ordered colour-node buffer",
        );
    }
}

/// CC3 §4.2: 133 parameters generated from three patterns.
#[test]
fn color_curves_descriptor_expands_the_published_patterns() {
    let descriptor = effect_descriptor("color_curves").expect("color_curves must be registered");
    assert_eq!(COLOR_CURVE_PARAMETER_COUNT, 33);
    assert_eq!(COLOR_CURVES_PARAMETER_COUNT, 133);
    assert_eq!(descriptor.parameters.len(), 133);

    let mut expected: Vec<(String, i64, i64, i64)> = Vec::new();
    for curve in ["master", "red", "green", "blue"] {
        expected.push((format!("{curve}_point_count"), 2, 16, 2));
        for index in 0..=15 {
            let neutral = if index == 0 { 0 } else { 10_000 };
            expected.push((format!("{curve}_x{index}"), -2_000, 12_000, neutral));
            expected.push((format!("{curve}_y{index}"), -2_000, 12_000, neutral));
        }
    }
    expected.push(("bypass".to_owned(), 0, 1, 0));
    assert_eq!(expected.len(), 133);

    for (parameter, (name, min, max, neutral)) in descriptor.parameters.iter().zip(&expected) {
        assert_eq!(
            (
                parameter.name,
                parameter.min,
                parameter.max,
                parameter.neutral,
                parameter.uniform
            ),
            (
                name.as_str(),
                *min,
                *max,
                *neutral,
                EffectUniform::ColorNode
            )
        );
    }

    // Spot checks written out in full so a pattern change cannot pass silently.
    let x0 = descriptor.parameter("master_x0").expect("master_x0");
    assert_eq!((x0.min, x0.max, x0.neutral), (-2_000, 12_000, 0));
    let y15 = descriptor.parameter("blue_y15").expect("blue_y15");
    assert_eq!((y15.min, y15.max, y15.neutral), (-2_000, 12_000, 10_000));
    assert!(descriptor.parameter("master_x16").is_none());
    assert!(descriptor.parameter("luma_point_count").is_none());
}

#[test]
fn curve_parameter_name_helper_lists_every_parameter_one_curve_owns() {
    for curve in ColorCurveChannel::ALL {
        let names = color_curve_parameter_names(curve);
        assert_eq!(names.len(), 33);
        assert_eq!(names[0], format!("{}_point_count", curve.name()));
        assert_eq!(curve.point_count_parameter(), names[0]);
        for index in 0..16 {
            assert_eq!(
                curve.x_parameter(index).expect("point index below 16"),
                format!("{}_x{index}", curve.name())
            );
            assert_eq!(
                curve.y_parameter(index).expect("point index below 16"),
                format!("{}_y{index}", curve.name())
            );
        }
        assert!(curve.x_parameter(16).is_none());
        assert_eq!(ColorCurveChannel::owning(names[7]), Some(curve));
    }
    assert_eq!(ColorCurveChannel::owning("bypass"), None);
    assert_eq!(ColorCurveChannel::from_name("luma"), None);
}

/// CC3 §3.1 and §9: the new nodes are managed, never compatibility stages.
#[test]
fn the_new_nodes_are_managed_colour_nodes_and_never_compatibility_stages() {
    assert_eq!(
        MANAGED_COLOR_NODE_NAMES,
        ["primary_correction", "color_wheels", "color_curves"]
    );
    for name in MANAGED_COLOR_NODE_NAMES {
        assert!(is_managed_color_node(name));
        assert!(
            effect_compatibility_stage(name).is_none(),
            "{name} is inside the managed conformance claim"
        );
        assert_eq!(
            ColorNodeKind::from_effect_name(name)
                .expect("managed node")
                .effect_name(),
            name
        );
    }
    for name in ["brightness", "look_lut", "cube_lut", "transform"] {
        assert!(!is_managed_color_node(name));
    }
    assert_eq!(ColorNodeKind::Primary.storage_buffer_tag(), 1);
    assert_eq!(ColorNodeKind::Wheels.storage_buffer_tag(), 2);
    assert_eq!(ColorNodeKind::Curves.storage_buffer_tag(), 3);
}

/// CC3 §4: a value at a bound is valid; one step beyond is rejected with the
/// field, the observed value, and the allowed range.
#[test]
fn control_bounds_accept_the_extremes_and_reject_one_step_beyond() {
    let mut document = managed_document();
    add(&mut document, effect(1, "color_wheels", &[])).expect("a neutral wheels node is legal");
    add(&mut document, effect(2, "color_curves", &[])).expect("a neutral curves node is legal");

    let accepted: [(u64, &str, i64); 10] = [
        (1, "lift_master_basis_points", -2_000),
        (1, "lift_blue_basis_points", 2_000),
        (1, "gamma_red_thousandths", 100),
        (1, "gamma_green_thousandths", 4_000),
        (1, "gain_blue_thousandths", 0),
        (1, "gain_master_thousandths", 4_000),
        (1, "bypass", 1),
        (2, "master_point_count", 2),
        (2, "red_x15", 12_000),
        (2, "green_y0", -2_000),
    ];
    for (id, name, value) in accepted {
        set_param(&mut document, id, name, value)
            .unwrap_or_else(|error| panic!("{name}={value} is a legal bound: {error}"));
    }

    let rejected: [(u64, &str, i64, i64, i64); 10] = [
        (1, "lift_master_basis_points", -2_001, -2_000, 2_000),
        (1, "lift_blue_basis_points", 2_001, -2_000, 2_000),
        (1, "gamma_red_thousandths", 99, 100, 4_000),
        (1, "gamma_green_thousandths", 4_001, 100, 4_000),
        (1, "gain_blue_thousandths", -1, 0, 4_000),
        (1, "bypass", 2, 0, 1),
        (2, "master_point_count", 1, 2, 16),
        (2, "master_point_count", 17, 2, 16),
        (2, "red_x15", 12_001, -2_000, 12_000),
        (2, "green_y0", -2_001, -2_000, 12_000),
    ];
    for (id, name, value, min, max) in rejected {
        let before = document.clone();
        let error = set_param(&mut document, id, name, value)
            .expect_err("a value outside the descriptor range must be rejected");
        assert_eq!(
            error,
            OpError::EffectParamOutOfRange {
                effect: if id == 1 {
                    "color_wheels"
                } else {
                    "color_curves"
                }
                .to_owned(),
                name: name.to_owned(),
                min,
                max,
                actual: value,
            }
        );
        assert_eq!(document, before, "a rejected edit must not mutate anything");
    }

    // CC3 §4.2: `bypass` is an integer token, not a `ParamValue::Boolean`.
    let error = Operation::SetEffectParam {
        clip: ClipId(1),
        effect: EffectId(2),
        name: "bypass".to_owned(),
        value: ParamValue::Boolean(true),
    }
    .apply(&mut document)
    .expect_err("bypass is a 0/1 integer token");
    assert_eq!(
        error,
        OpError::InvalidEffectParamType {
            effect: "color_curves".to_owned(),
            name: "bypass".to_owned(),
        }
    );

    // The maximum point count is legal when the sixteen points are distinct,
    // which is the only way to declare them: the descriptor neutrals collide.
    let ramp: Vec<(String, i64)> = std::iter::once(("blue_point_count".to_owned(), 16))
        .chain((0..16).flat_map(|index| {
            [
                (format!("blue_x{index}"), index * 800 - 2_000),
                (format!("blue_y{index}"), index * 800 - 2_000),
            ]
        }))
        .collect();
    let ramp: Vec<(&str, i64)> = ramp
        .iter()
        .map(|(name, value)| (name.as_str(), *value))
        .collect();
    add(&mut document, effect(3, "color_curves", &ramp))
        .expect("sixteen distinct points are legal");
    assert_eq!(
        ResolvedCurves::from_effect(&document.clip(ClipId(1)).expect("clip").effects[2])
            .blue
            .points
            .len(),
        16
    );

    let error = set_param(&mut document, 2, "master_x16", 100)
        .expect_err("point 16 does not exist on a 16-point curve");
    assert_eq!(
        error,
        OpError::UnknownEffectParam {
            effect: "color_curves".to_owned(),
            name: "master_x16".to_owned(),
        }
    );
}

/// CC3 §2.3: `x` must be strictly increasing over the active prefix; equal or
/// descending `x` is rejected atomically on both `AddEffect` and
/// `SetEffectParam`.
#[test]
fn curve_points_must_be_strictly_increasing_in_x_over_the_active_prefix() {
    let mut document = managed_document();

    let equal = effect(
        1,
        "color_curves",
        &[
            ("master_point_count", 3),
            ("master_x0", 0),
            ("master_x1", 5_000),
            ("master_y1", 6_000),
            ("master_x2", 5_000),
        ],
    );
    let before = document.clone();
    let error = add(&mut document, equal).expect_err("equal x must be rejected");
    assert_eq!(
        error,
        OpError::InvalidCurvePoints {
            effect: "color_curves".to_owned(),
            curve: "master".to_owned(),
            index: 2,
            previous_x: 5_000,
            x: 5_000,
        }
    );
    assert_eq!(document, before, "a rejected AddEffect must be atomic");

    let descending = effect(
        1,
        "color_curves",
        &[("red_point_count", 3), ("red_x1", 5_000), ("red_x2", 4_999)],
    );
    let error = add(&mut document, descending).expect_err("descending x must be rejected");
    assert_eq!(
        error,
        OpError::InvalidCurvePoints {
            effect: "color_curves".to_owned(),
            curve: "red".to_owned(),
            index: 2,
            previous_x: 5_000,
            x: 4_999,
        }
    );
    assert_eq!(document, before, "a rejected AddEffect must be atomic");

    // Points at index >= point_count are ignored, so their colliding
    // (10000, 10000) neutrals - and any stored value there - are legal.
    add(
        &mut document,
        effect(
            1,
            "color_curves",
            &[
                ("master_point_count", 2),
                ("master_x1", 5_000),
                ("master_x2", 5_000),
                ("master_x3", 100),
            ],
        ),
    )
    .expect("inactive points are ignored even when they collide");

    // Raising the count activates the colliding points, and that edit is
    // rejected against the map the change would produce.
    let before = document.clone();
    let error = set_param(&mut document, 1, "master_point_count", 4)
        .expect_err("activating colliding points must be rejected");
    assert_eq!(
        error,
        OpError::InvalidCurvePoints {
            effect: "color_curves".to_owned(),
            curve: "master".to_owned(),
            index: 2,
            previous_x: 5_000,
            x: 5_000,
        }
    );
    assert_eq!(document, before, "a rejected SetEffectParam must be atomic");

    // Separating the third point first makes the same activation legal, which
    // is what a curve editor does when it adds a point.
    set_param(&mut document, 1, "master_x2", 8_000).expect("an inactive point may move freely");
    set_param(&mut document, 1, "master_point_count", 3).expect("three separated points are legal");

    // Moving an active point onto its neighbour is rejected the same way.
    let before = document.clone();
    let error =
        set_param(&mut document, 1, "master_x1", 8_000).expect_err("x1 may not reach x2 at 8000");
    assert_eq!(
        error,
        OpError::InvalidCurvePoints {
            effect: "color_curves".to_owned(),
            curve: "master".to_owned(),
            index: 2,
            previous_x: 8_000,
            x: 8_000,
        }
    );
    assert_eq!(document, before);
    set_param(&mut document, 1, "master_x1", 7_999)
        .expect("one basis point of separation is legal");
}

/// CC3 §3.1: at most sixteen managed colour nodes per layer.
#[test]
fn the_seventeenth_managed_colour_node_is_a_typed_error() {
    let mut document = managed_document();
    let kinds = ["primary_correction", "color_wheels", "color_curves"];
    for index in 0..COLOR_NODE_LIMIT_PER_LAYER {
        add(
            &mut document,
            effect(index as u64 + 1, kinds[index % kinds.len()], &[]),
        )
        .expect("sixteen managed colour nodes are legal");
    }
    let clip = document.clip(ClipId(1)).expect("clip");
    assert_eq!(managed_color_node_count(&clip.effects), 16);

    // An unmanaged effect does not consume a colour-node slot.
    add(&mut document, effect(100, "transform", &[])).expect("transform is not a colour node");

    let before = document.clone();
    let error = add(&mut document, effect(17, "color_wheels", &[]))
        .expect_err("the seventeenth colour node must be rejected");
    assert_eq!(
        error,
        OpError::TooManyColorNodes {
            clip: ClipId(1),
            limit: 16,
            actual: 17,
        }
    );
    assert_eq!(document, before, "a rejected AddEffect must be atomic");

    // A bypassed node still occupies its slot.
    set_param(&mut document, 2, "bypass", 1).expect("bypass is an ordinary parameter");
    let error = add(&mut document, effect(18, "color_curves", &[]))
        .expect_err("a bypassed node keeps its slot");
    assert_eq!(
        error,
        OpError::TooManyColorNodes {
            clip: ClipId(1),
            limit: 16,
            actual: 17,
        }
    );
}

/// CC3 §6: `{curve}_point_count` accepts only `Hold` keyframes.
#[test]
fn curve_point_count_accepts_only_hold_keyframes() {
    let mut document = managed_document();
    add(&mut document, effect(1, "color_curves", &[])).expect("a neutral curves node is legal");

    set_keyframes(
        &mut document,
        1,
        "master_point_count",
        &[
            (0, 2, KeyframeInterpolation::Hold),
            (10, 5, KeyframeInterpolation::Hold),
        ],
    )
    .expect("whole-curve steps are policy 1");

    let before = document.clone();
    let error = set_keyframes(
        &mut document,
        1,
        "red_point_count",
        &[
            (0, 2, KeyframeInterpolation::Hold),
            (10, 5, KeyframeInterpolation::Linear),
        ],
    )
    .expect_err("an interpolated point count must be rejected");
    assert_eq!(
        error,
        OpError::NonHoldKeyframeParameter {
            effect: "color_curves".to_owned(),
            name: "red_point_count".to_owned(),
        }
    );
    assert_eq!(document, before, "a rejected keyframe edit must be atomic");

    // Coordinates and bypass keep every interpolation.
    set_keyframes(
        &mut document,
        1,
        "blue_x1",
        &[
            (0, 4_000, KeyframeInterpolation::Linear),
            (10, 6_000, KeyframeInterpolation::EaseInOut),
        ],
    )
    .expect("point-wise interpolation is policy 2");
}

/// CC3 §6: an animated point count and animated coordinates are mutually
/// exclusive on the same curve, whichever operation arrives second.
#[test]
fn animated_point_count_and_animated_coordinates_are_mutually_exclusive() {
    let mut document = managed_document();
    add(&mut document, effect(1, "color_curves", &[])).expect("a neutral curves node is legal");
    set_keyframes(
        &mut document,
        1,
        "master_point_count",
        &[
            (0, 2, KeyframeInterpolation::Hold),
            (10, 3, KeyframeInterpolation::Hold),
        ],
    )
    .expect("whole-curve steps are policy 1");

    let before = document.clone();
    let error = set_keyframes(
        &mut document,
        1,
        "master_x1",
        &[(0, 4_000, KeyframeInterpolation::Linear)],
    )
    .expect_err("a coordinate may not animate under a stepped point count");
    assert_eq!(
        error,
        OpError::CurvePointCountAnimatedWithPoints {
            effect: "color_curves".to_owned(),
            curve: "master".to_owned(),
        }
    );
    assert_eq!(document, before, "a rejected keyframe edit must be atomic");

    // A different curve is unaffected: the policy is per curve.
    set_keyframes(
        &mut document,
        1,
        "green_y1",
        &[(0, 4_000, KeyframeInterpolation::Linear)],
    )
    .expect("policies are enforced per curve");

    // The other order is rejected too.
    let before = document.clone();
    let error = set_keyframes(
        &mut document,
        1,
        "green_point_count",
        &[
            (0, 2, KeyframeInterpolation::Hold),
            (10, 3, KeyframeInterpolation::Hold),
        ],
    )
    .expect_err("a point count may not step while coordinates animate");
    assert_eq!(
        error,
        OpError::CurvePointCountAnimatedWithPoints {
            effect: "color_curves".to_owned(),
            curve: "green".to_owned(),
        }
    );
    assert_eq!(document, before);

    // One point-count keyframe is policy 2's constant point count and stays
    // legal alongside animated coordinates.
    set_keyframes(
        &mut document,
        1,
        "green_point_count",
        &[(0, 2, KeyframeInterpolation::Hold)],
    )
    .expect("a single point-count keyframe is a constant point count");
}

/// CC3 §3.4: a resolved curve is truncated to its longest strictly increasing
/// prefix; a prefix shorter than two points resolves to the identity.
#[test]
fn degenerate_resolved_curves_truncate_to_the_longest_increasing_prefix() {
    // Four declared points whose third and fourth collide at 10000.
    let stepped = effect(
        1,
        "color_curves",
        &[
            ("master_point_count", 4),
            ("master_x1", 5_000),
            ("master_y1", 6_000),
        ],
    );
    let resolved = ResolvedCurves::from_effect(&stepped);
    assert_eq!(
        resolved.master.points,
        vec![(0, 0), (5_000, 6_000), (10_000, 10_000)]
    );
    assert_eq!(resolved.master.declared_point_count, 4);
    assert!(resolved.master.truncated);
    assert_eq!(
        resolved.truncated_curves(),
        vec![ColorCurveChannel::Master],
        "only the offending curve is reported"
    );
    assert!(resolved.truncated());
    assert!(!resolved.red.truncated);
    assert!(resolved.red.is_structural_identity());

    // A first pair that already violates the rule leaves no usable prefix.
    let collapsed = effect(
        1,
        "color_curves",
        &[
            ("blue_point_count", 3),
            ("blue_x0", 6_000),
            ("blue_x1", 1_000),
            ("blue_x2", 2_000),
        ],
    );
    let resolved = ResolvedCurves::from_effect(&collapsed);
    assert_eq!(resolved.blue.points, vec![(0, 0), (10_000, 10_000)]);
    assert!(resolved.blue.truncated);
    assert!(resolved.blue.is_structural_identity());
    assert_eq!(resolved.blue.declared_point_count, 3);

    // The same node reached through keyframe evaluation, which is how §3.4
    // actually happens: the point count steps up past the authored points.
    let mut animated = effect(
        1,
        "color_curves",
        &[("master_x1", 5_000), ("master_y1", 6_000)],
    );
    animated.keyframes.insert(
        "master_point_count".to_owned(),
        AutomationCurve {
            keyframes: vec![
                Keyframe {
                    at: TimeCode(0),
                    value: 2,
                    interpolation: KeyframeInterpolation::Hold,
                },
                Keyframe {
                    at: TimeCode(10),
                    value: 4,
                    interpolation: KeyframeInterpolation::Hold,
                },
            ],
        },
    );
    let at_zero = ResolvedCurves::from_effect(&animated.evaluated_at(TimeCode(0)));
    assert_eq!(at_zero.master.points, vec![(0, 0), (5_000, 6_000)]);
    assert!(!at_zero.truncated());
    let at_ten = ResolvedCurves::from_effect(&animated.evaluated_at(TimeCode(10)));
    assert_eq!(
        at_ten.master.points,
        vec![(0, 0), (5_000, 6_000), (10_000, 10_000)]
    );
    assert!(at_ten.truncated());
}

/// CC3 §3.3: inactivity is decided on the stored integers, never on floats.
#[test]
fn neutral_and_bypassed_nodes_are_inactive_on_the_stored_integers() {
    let neutral_wheels = effect(1, "color_wheels", &[]);
    let params = ColorWheelsParams::from_effect(&neutral_wheels);
    assert_eq!(params, ColorWheelsParams::NEUTRAL);
    assert!(params.is_neutral());
    assert!(!params.bypass());
    assert_eq!(
        params.inactive_reason(),
        Some(ColorNodeInactiveReason::Neutral)
    );
    assert_eq!(ColorNodeInactiveReason::Neutral.as_str(), "neutral");

    // One integer step away from neutral activates the node.
    let nudged = effect(1, "color_wheels", &[("gain_red_thousandths", 1_001)]);
    let params = ColorWheelsParams::from_effect(&nudged);
    assert_eq!(params.gain_thousandths.red, 1_001);
    assert_eq!(params.gain_thousandths.master, 1_000);
    assert!(!params.is_neutral());
    assert!(params.is_active());

    // Bypass wins over a non-neutral control and reports its own reason.
    let bypassed = effect(
        1,
        "color_wheels",
        &[("gain_red_thousandths", 1_200), ("bypass", 1)],
    );
    assert_eq!(
        color_node_inactive_reason(&bypassed),
        Some(ColorNodeInactiveReason::Bypassed)
    );
    assert_eq!(ColorNodeInactiveReason::Bypassed.as_str(), "bypassed");

    // Curves: structural identity only. A collinear 16-point curve is
    // mathematically identity but must still be evaluated (§2.3, §10.3.6).
    let neutral_curves = effect(2, "color_curves", &[]);
    let resolved = ResolvedCurves::from_effect(&neutral_curves);
    assert!(resolved.is_neutral());
    assert_eq!(
        resolved.master.points,
        vec![(0, 0), (10_000, 10_000)],
        "every parameter at neutral is the structural identity"
    );
    let mut collinear = vec![("master_point_count", 3)];
    collinear.push(("master_x1", 5_000));
    collinear.push(("master_y1", 5_000));
    let collinear = effect(2, "color_curves", &collinear);
    let resolved = ResolvedCurves::from_effect(&collinear);
    assert!(!resolved.master.is_structural_identity());
    assert!(!resolved.is_neutral());
    assert_eq!(color_node_inactive_reason(&collinear), None);

    let bypassed_curves = effect(
        2,
        "color_curves",
        &[
            ("master_point_count", 3),
            ("master_x1", 5_000),
            ("bypass", 1),
        ],
    );
    assert_eq!(
        color_node_inactive_reason(&bypassed_curves),
        Some(ColorNodeInactiveReason::Bypassed)
    );
    assert!(CurvePoints::identity().is_structural_identity());
}

/// CC3 §3.1: the ordered stack keeps its serialized order, and inactive nodes
/// are omitted from the render.
#[test]
fn active_color_nodes_reports_stage_indices_in_serialized_order() {
    let effects = vec![
        effect(1, "transform", &[]),
        effect(
            2,
            "color_curves",
            &[("master_point_count", 3), ("master_x1", 5_000)],
        ),
        effect(3, "primary_correction", &[]),
        effect(4, "color_wheels", &[("lift_master_basis_points", -500)]),
        effect(5, "color_wheels", &[]),
        effect(6, "color_curves", &[("bypass", 1), ("red_x1", 1_000)]),
    ];
    assert_eq!(
        active_color_nodes(&effects),
        vec![
            (1, ColorNodeKind::Curves),
            (2, ColorNodeKind::Primary),
            (3, ColorNodeKind::Wheels),
        ]
    );
    assert_eq!(managed_color_node_count(&effects), 5);
    assert_eq!(classify_color_node(&effects[0]), None);
    assert_eq!(
        classify_color_node(&effects[3]),
        Some(ColorNodeKind::Wheels)
    );
}

/// CC3 §2.4 and §9.3: an omitted parameter resolves to its neutral, and the
/// sparse node survives save/reopen, journal replay, undo, and redo unchanged.
#[test]
fn a_sparse_curves_node_resolves_to_neutral_and_survives_history_byte_for_byte() {
    let initial = managed_document();
    initial.validate().expect("fixture must be valid");
    let sparse = effect(
        7,
        "color_curves",
        &[
            ("master_point_count", 3),
            ("master_x1", 5_000),
            ("master_y1", 6_000),
        ],
    );
    let operation = Operation::AddEffect {
        clip: ClipId(1),
        effect: sparse.clone(),
    };

    let mut applied = initial.clone();
    operation.apply(&mut applied).expect("sparse node is legal");
    let stored = &applied.clip(ClipId(1)).expect("clip").effects[0];
    assert_eq!(
        stored.parameters.len(),
        3,
        "only the touched parameters are stored"
    );
    let resolved = ResolvedCurves::from_effect(stored);
    assert_eq!(
        resolved.master.points,
        vec![(0, 0), (5_000, 6_000), (10_000, 10_000)],
        "the omitted first and last points resolve to their neutrals"
    );
    assert!(resolved.red.is_structural_identity());
    assert!(!resolved.master.truncated);

    let saved = serde_json::to_string(&applied).expect("document should save");
    let reopened: Document = serde_json::from_str(&saved).expect("document should reopen");
    assert_eq!(reopened, applied);
    assert_eq!(
        serde_json::to_string(&reopened.clip(ClipId(1)).expect("clip").effects).unwrap(),
        serde_json::to_string(&applied.clip(ClipId(1)).expect("clip").effects).unwrap(),
    );

    let core = Core::spawn(initial.clone()).expect("core should spawn");
    let Event::DocumentChanged {
        doc: live,
        journal_command: Some(JournalCommand::Do(journaled)),
        ..
    } = core.request(Command::Do(operation.clone())).unwrap()
    else {
        panic!("adding a curves node should be accepted and journaled");
    };
    assert_eq!(journaled, operation);
    assert_eq!(&*live, &applied);

    let encoded = serde_json::to_string(&JournalCommand::Do(operation)).unwrap();
    let parsed: JournalCommand = serde_json::from_str(&encoded).unwrap();
    let replay = Core::spawn(initial.clone()).expect("core should spawn");
    let Event::DocumentChanged { doc: replayed, .. } = replay.request(parsed.into()).unwrap()
    else {
        panic!("the journaled node should replay");
    };
    assert_eq!(&*replayed, &*live);

    let Event::DocumentChanged { doc: undone, .. } = core.request(Command::Undo).unwrap() else {
        panic!("adding a node should undo");
    };
    assert_eq!(&*undone, &initial);
    let Event::DocumentChanged { doc: redone, .. } = core.request(Command::Redo).unwrap() else {
        panic!("adding a node should redo");
    };
    assert_eq!(&*redone, &*live);
    assert_eq!(
        serde_json::to_string(&*redone).unwrap(),
        serde_json::to_string(&applied).unwrap(),
        "undo and redo must restore the node byte-for-byte"
    );
}

/// CC3 §9 and §3.1: managed nodes produce no QA or delivery findings.
#[test]
fn qa_and_delivery_report_nothing_for_managed_colour_nodes() {
    let mut document = managed_document();
    add(
        &mut document,
        effect(1, "primary_correction", &[("exposure_milli_stops", 250)]),
    )
    .expect("a primary node is legal");
    add(
        &mut document,
        effect(2, "color_wheels", &[("gain_red_thousandths", 1_200)]),
    )
    .expect("a wheels node is legal");
    add(
        &mut document,
        effect(
            3,
            "color_curves",
            &[
                ("master_point_count", 3),
                ("master_x1", 5_000),
                ("master_y1", 6_000),
            ],
        ),
    )
    .expect("a curves node is legal");

    let report = qa_document(&document);
    let colour_codes = [
        "legacy_colour_semantics",
        "legacy_lut_stage",
        "curve_truncated_by_automation",
    ];
    for code in colour_codes {
        assert!(
            !report.issues.iter().any(|issue| issue.code == code),
            "managed colour nodes must not report {code}: {:?}",
            report.issues
        );
    }
    assert!(report.export_ready());

    let delivery = delivery_conformance(&document, DeliveryProfile::SourceMaster, 50, 50)
        .expect("delivery conformance must produce a report");
    for code in colour_codes {
        assert!(
            !delivery.issues.iter().any(|issue| issue.code == code),
            "managed colour nodes must not block delivery with {code}"
        );
    }
    assert!(delivery.export_ready());
}

/// CC3 §3.4: the inspector reports `curve_truncated_by_automation` for a node
/// whose evaluated curves lose points.
#[test]
fn qa_reports_a_curve_truncated_by_automation() {
    let mut document = managed_document();
    add(
        &mut document,
        effect(
            1,
            "color_curves",
            &[
                ("master_point_count", 2),
                ("master_x1", 5_000),
                ("master_y1", 6_000),
            ],
        ),
    )
    .expect("two authored points are legal");
    assert!(
        !qa_document(&document)
            .issues
            .iter()
            .any(|issue| issue.code == "curve_truncated_by_automation"),
        "a static, valid curve is never truncated"
    );

    set_keyframes(
        &mut document,
        1,
        "master_point_count",
        &[
            (0, 2, KeyframeInterpolation::Hold),
            (10, 4, KeyframeInterpolation::Hold),
        ],
    )
    .expect("whole-curve steps are policy 1");

    let report = qa_document(&document);
    let issue = report
        .issues
        .iter()
        .find(|issue| issue.code == "curve_truncated_by_automation")
        .expect("a curve truncated by automation must be reported");
    assert_eq!(issue.severity, QaSeverity::Warning);
    assert_eq!(issue.clip, Some(ClipId(1)));
    assert!(
        issue.message.contains("the master curve"),
        "{}",
        issue.message
    );
    assert!(issue.message.contains("frame 10"), "{}", issue.message);
    assert!(
        report.export_ready(),
        "truncation is reportable, not an export blocker"
    );

    // A bypassed node is the exact identity, so its truncation is not
    // reported.
    set_param(&mut document, 1, "bypass", 1).expect("bypass is an ordinary parameter");
    assert!(
        !qa_document(&document)
            .issues
            .iter()
            .any(|issue| issue.code == "curve_truncated_by_automation")
    );
}

/// Insert a hand-authored effect straight into the document, bypassing the
/// operations that would have rejected it. This is the shape a project file
/// edited outside Kinewright arrives in.
fn hand_edited_document(effects: Vec<Effect>) -> Document {
    let mut document = managed_document();
    document.tracks[0].clips[0].effects = effects;
    document
}

/// CC3 §2.3 is a *document* invariant, not merely an `AddEffect` precondition.
///
/// A hand-edited project whose stored `x` is not strictly increasing used to
/// load with `Ok` and then lock its own colour node: every later
/// `SetEffectParam` re-validates the whole effect and fails on parameters the
/// user never touched, so the node could be neither graded nor reset.
#[test]
fn validate_document_rejects_stored_curve_points_that_are_not_strictly_increasing() {
    let document = hand_edited_document(vec![effect(
        1,
        "color_curves",
        &[
            ("master_point_count", 3),
            ("master_x0", 0),
            ("master_x1", 5_000),
            ("master_x2", 4_000),
        ],
    )]);
    assert_eq!(
        document.validate(),
        Err(OpError::InvalidCurvePoints {
            effect: "color_curves".to_owned(),
            curve: "master".to_owned(),
            index: 2,
            previous_x: 5_000,
            x: 4_000,
        }),
        "the same points `AddEffect` rejects must not load",
    );

    // Points at index >= point_count keep their colliding neutrals and stay
    // legal, exactly as on the operation path.
    hand_edited_document(vec![effect(
        1,
        "color_curves",
        &[
            ("master_point_count", 2),
            ("master_x1", 5_000),
            ("master_x2", 5_000),
            ("master_x3", 100),
        ],
    )])
    .validate()
    .expect("inactive points are ignored by the document invariant too");
}

/// A declared `point_count` of sixteen with the coordinates omitted resolves
/// every point past the first to the colliding `(10000, 10000)` neutral, so it
/// is not strictly increasing and must be rejected on load.
///
/// This is also what removed the *false* static
/// `curve_truncated_by_automation`: before the invariant existed, such a node
/// loaded cleanly and QA reported a truncation the user could not act on,
/// because no operation could reach the state that produced it.
#[test]
fn validate_document_rejects_a_declared_sixteen_point_curve_with_omitted_coordinates() {
    let document = hand_edited_document(vec![effect(
        1,
        "color_curves",
        &[("master_point_count", 16)],
    )]);
    assert_eq!(
        document.validate(),
        Err(OpError::InvalidCurvePoints {
            effect: "color_curves".to_owned(),
            curve: "master".to_owned(),
            index: 2,
            previous_x: 10_000,
            x: 10_000,
        }),
    );
    // The state QA used to describe is now simply unreachable.
    assert!(
        qa_document(&document)
            .issues
            .iter()
            .any(|issue| issue.code == "curve_truncated_by_automation"),
        "this is the static state that produced the unactionable warning",
    );
    assert!(
        add_effect_would_reject(&document),
        "no operation can reach or leave this state",
    );
}

/// Whether every operation on a document fails, which is what an unenforced
/// load-time invariant produces: a document nothing can edit.
fn add_effect_would_reject(document: &Document) -> bool {
    let mut candidate = document.clone();
    Operation::SetEffectParam {
        clip: ClipId(1),
        effect: EffectId(1),
        name: "master_point_count".to_owned(),
        value: ParamValue::Integer(2),
    }
    .apply(&mut candidate)
    .is_err()
}

/// CC3 §3.1: the sixteen-node limit is a document invariant as well. `AddEffect`
/// enforces it, so a seventeenth node can only have arrived by hand.
#[test]
fn validate_document_rejects_more_than_sixteen_managed_colour_nodes() {
    let sixteen: Vec<Effect> = (0..COLOR_NODE_LIMIT_PER_LAYER)
        .map(|index| effect(index as u64 + 1, "color_wheels", &[]))
        .collect();
    hand_edited_document(sixteen.clone())
        .validate()
        .expect("sixteen managed colour nodes are legal");

    let mut seventeen = sixteen;
    seventeen.push(effect(17, "color_wheels", &[]));
    assert_eq!(
        hand_edited_document(seventeen.clone()).validate(),
        Err(OpError::TooManyColorNodes {
            clip: ClipId(1),
            limit: 16,
            actual: 17,
        }),
    );

    // Unmanaged effects never consume a slot, however many there are.
    let mut mixed = seventeen;
    mixed.pop();
    for index in 0..8 {
        mixed.push(effect(200 + index, "transform", &[]));
    }
    hand_edited_document(mixed)
        .validate()
        .expect("only managed colour nodes count against the limit");
}

/// CC3 §3.4: `bypass` is node-owned rather than curve-owned, so a truncation
/// scan built from curve keyframes alone would only ever look at frame zero.
/// A node bypassed at frame zero and live from frame ten must still report.
#[test]
fn a_keyframed_bypass_is_part_of_the_qa_truncation_scan() {
    let mut document = managed_document();
    add(&mut document, effect(1, "color_curves", &[])).expect("a neutral curves node is legal");
    // A single Hold keyframe holds sixteen points at every frame, with the
    // coordinates omitted so each one resolves to the colliding neutral. The
    // static value stays at two, so the document itself remains legal.
    set_keyframes(
        &mut document,
        1,
        "master_point_count",
        &[(0, 16, KeyframeInterpolation::Hold)],
    )
    .expect("a single whole-curve step is policy 1");
    set_keyframes(
        &mut document,
        1,
        "bypass",
        &[
            (0, 1, KeyframeInterpolation::Hold),
            (10, 0, KeyframeInterpolation::Hold),
        ],
    )
    .expect("bypass is keyframable");

    let report = qa_document(&document);
    let issue = report
        .issues
        .iter()
        .find(|issue| issue.code == "curve_truncated_by_automation")
        .expect("frame 10 releases the bypass and must be scanned");
    assert_eq!(issue.severity, QaSeverity::Warning);
    assert!(issue.message.contains("frame 10"), "{}", issue.message);

    // Holding the bypass down for the whole clip still reports nothing.
    set_keyframes(
        &mut document,
        1,
        "bypass",
        &[(0, 1, KeyframeInterpolation::Hold)],
    )
    .expect("bypass is keyframable");
    assert!(
        !qa_document(&document)
            .issues
            .iter()
            .any(|issue| issue.code == "curve_truncated_by_automation"),
        "a node that is bypassed at every frame is the exact identity",
    );
}
