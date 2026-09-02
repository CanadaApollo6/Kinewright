//! Region tracker geometry tests.

use super::*;
use crate::server::mattes::{
    LayerTransform, layer_unit_to_basis_points, layer_unit_to_percent, matte_track_box_percent,
    matte_track_centre_basis_points, pixel_to_basis_points, pixel_to_percent,
    resolve_layer_transform_at, resolve_static_layer_transform, tracked_box_percent,
    tracked_centre_layer_unit,
};
use crate::server::tracking::{
    track_region, tracked_subject_focus_constraint, tracking_sample_frames,
};

fn box_frame(center: [u32; 2]) -> kinewright_core::RgbaImage {
    let width = 32;
    let height = 20;
    let mut pixels = vec![0_u8; usize::try_from(width * height * 4).unwrap()];
    for pixel in pixels.as_chunks_mut::<4>().0.iter_mut() {
        pixel[3] = 255;
    }
    for y in center[1] - 2..=center[1] + 2 {
        for x in center[0] - 2..=center[0] + 2 {
            let index = usize::try_from((y * width + x) * 4).unwrap();
            pixels[index..index + 4].copy_from_slice(&[220, 40, 10, 255]);
        }
    }
    kinewright_core::RgbaImage {
        width,
        height,
        pixels,
    }
}

/// CC5 §5.2's pixel → matte basis-point conversion, and the divergence
/// from the tracker's own `extent − 1` denominator.
///
/// The two are deliberately different functions: the tracker's names a
/// *sample position* on a lattice, the matte's names a *fraction of the
/// extent*. Asserting the divergence explicitly means a refactor cannot
/// quietly swap them.
#[test]
fn matte_centre_conversion_uses_the_pixel_centre_over_the_full_extent() {
    // round((pixel + 0.5) * 10000 / extent), hand-computed.
    // extent 512: (0 + 0.5) * 10000 / 512 = 9.765625 -> 10
    assert_eq!(matte_track_centre_basis_points(0, 512), 10);
    // (255 + 0.5) * 10000 / 512 = 4990.234375 -> 4990
    assert_eq!(matte_track_centre_basis_points(255, 512), 4990);
    // (256 + 0.5) * 10000 / 512 = 5009.765625 -> 5010
    assert_eq!(matte_track_centre_basis_points(256, 512), 5010);
    // (511 + 0.5) * 10000 / 512 = 9990.234375 -> 9990
    assert_eq!(matte_track_centre_basis_points(511, 512), 9990);
    // extent 288: (144 + 0.5) * 10000 / 288 = 5017.361 -> 5017
    assert_eq!(matte_track_centre_basis_points(144, 288), 5017);

    // The tracker's own conversion divides by `extent - 1` and adds no
    // half pixel. The two agree in the middle and diverge most at the
    // edges, by 10 bp on a 512-wide thumbnail and 17 bp on a 288-tall one
    // — the divergence CC5 §9.2.11 records so a refactor cannot quietly
    // swap them.
    for (pixel, extent, expected_divergence) in [
        (0_u32, 512_u32, 10_i64),
        (511, 512, 10),
        (0, 288, 17),
        (287, 288, 17),
    ] {
        let matte = matte_track_centre_basis_points(pixel, extent);
        let lattice = i64::from(pixel_to_basis_points(pixel, extent));
        assert_eq!(
            (matte - lattice).abs(),
            expected_divergence,
            "pixel {pixel} of {extent}: matte {matte} vs lattice {lattice}"
        );
    }
    // In the middle of the raster the two coincide, which is why only the
    // edges bound the error.
    assert_eq!(
        matte_track_centre_basis_points(255, 512),
        i64::from(pixel_to_basis_points(255, 512))
    );
}

/// CC5 §5.2: `box_percent` is the window bounding box *on the composite*,
/// so it is doubled and rescaled by the layer scale.
#[test]
fn matte_track_box_percent_rescales_the_window_by_the_layer_scale() {
    // hw = 2500 bp = 0.25 of the width; 2 * 0.25 * 1.0 * 100 = 50 percent.
    assert_eq!(matte_track_box_percent(2_500, 1.0), 50);
    // At scale 0.5 the same window covers half as much of the composite.
    assert_eq!(matte_track_box_percent(2_500, 0.5), 25);
    assert_eq!(matte_track_box_percent(1_300, 1.0), 26);
    assert_eq!(matte_track_box_percent(1_800, 0.5), 18);
}

/// CC5 §5.2's normative conversion, pinned against the *compositor*, not
/// against its own inverse.
///
/// `compositor.wgsl` places the layer quad at NDC
/// `p = q·scale + (offset_x, −offset_y)` and the fragment stage reads
/// `uv.y = (1 − ndc.y)/2`, so the shader's y negation and the uv flip
/// cancel and *both* axes carry `+offset/2`:
///
/// `u_composite = scale·(u_layer − 0.5) + offset/2 + 0.5`.
///
/// A round-trip test cannot see a sign error that appears in both
/// directions, so every case here is a hand-worked absolute value.
#[test]
fn layer_transform_offsets_move_the_window_the_way_the_compositor_does() {
    // At scale 1 an offset of 1.0 shifts the picture half a frame. The
    // composite point 10000 bp (the bottom edge) therefore came from the
    // layer's *centre*, 5000 bp — not from 15000 bp, which is what a
    // doubly-negated y produced.
    let vertical_shift = LayerTransform {
        scale: 1.0,
        offset_x: 0.0,
        offset_y: 1.0,
    };
    assert_eq!(
        vertical_shift.composite_to_layer_basis_points([5_000, 10_000]),
        [5_000, 5_000]
    );
    // Forward, the same fact: the layer centre lands on the bottom edge.
    let composite = vertical_shift.layer_to_composite([0.5, 0.5]);
    assert!((composite[0] - 0.5).abs() < 1e-12, "{composite:?}");
    assert!((composite[1] - 1.0).abs() < 1e-12, "{composite:?}");

    // The symmetric x case, which was already right and must stay right.
    let horizontal_shift = LayerTransform {
        scale: 1.0,
        offset_x: 1.0,
        offset_y: 0.0,
    };
    assert_eq!(
        horizontal_shift.composite_to_layer_basis_points([10_000, 5_000]),
        [5_000, 5_000]
    );
    let composite = horizontal_shift.layer_to_composite([0.5, 0.5]);
    assert!((composite[0] - 1.0).abs() < 1e-12, "{composite:?}");
    assert!((composite[1] - 0.5).abs() < 1e-12, "{composite:?}");

    // A negative y offset moves the picture the other way, by the same
    // half-frame: the layer centre lands on the top edge.
    let negative_y = LayerTransform {
        scale: 1.0,
        offset_x: 0.0,
        offset_y: -1.0,
    };
    assert_eq!(
        negative_y.composite_to_layer_basis_points([5_000, 0]),
        [5_000, 5_000]
    );

    // Hand-worked from the forward formula at scale 0.5 with both offsets
    // non-zero: offsets (0.4, -0.2).
    //   u_c.x = 0.5·(0.25 − 0.5) + 0.4/2 + 0.5 = 0.575  -> 5750 bp
    //   u_c.y = 0.5·(0.75 − 0.5) − 0.2/2 + 0.5 = 0.525  -> 5250 bp
    let both = LayerTransform {
        scale: 0.5,
        offset_x: 0.4,
        offset_y: -0.2,
    };
    let composite = both.layer_to_composite([0.25, 0.75]);
    assert!((composite[0] - 0.575).abs() < 1e-12, "{composite:?}");
    assert!((composite[1] - 0.525).abs() < 1e-12, "{composite:?}");
    assert_eq!(
        both.composite_to_layer_basis_points([5_750, 5_250]),
        [2_500, 7_500]
    );

    // And the two directions still compose to the identity.
    for (scale, offset_x, offset_y) in [(1.0, 0.0, 0.0), (0.5, 0.0, 0.0), (0.5, 0.4, -0.2)] {
        let transform = LayerTransform {
            scale,
            offset_x,
            offset_y,
        };
        for layer in [[0.5, 0.5], [0.25, 0.75], [0.1, 0.9]] {
            let composite = transform.layer_to_composite(layer);
            #[allow(clippy::cast_possible_truncation)]
            let basis_points = [
                (composite[0] * 10_000.0).round() as i64,
                (composite[1] * 10_000.0).round() as i64,
            ];
            let back = transform.composite_to_layer_basis_points(basis_points);
            #[allow(clippy::cast_possible_truncation)]
            let expected = [
                (layer[0] * 10_000.0).round() as i64,
                (layer[1] * 10_000.0).round() as i64,
            ];
            // One basis point of rounding at each of the two conversions.
            assert!(
                (back[0] - expected[0]).abs() <= 2 && (back[1] - expected[1]).abs() <= 2,
                "scale {scale} offset ({offset_x}, {offset_y}): {layer:?} -> {composite:?} -> {back:?}, expected {expected:?}"
            );
        }
    }
}

/// CC5 §5.2, worked by hand at `scale = 0.5`.
///
/// A layer centre of `(0.25, 0.75)` sits at composite
/// `(0.25 − 0.5)·0.5 + 0.5 = 0.375` and `(0.75 − 0.5)·0.5 + 0.5 = 0.625`,
/// i.e. 3750 and 6250 basis points. Converting back must recover 2500 and
/// 7500 exactly.
#[test]
fn layer_transform_matches_the_hand_worked_half_scale_case() {
    let transform = LayerTransform {
        scale: 0.5,
        offset_x: 0.0,
        offset_y: 0.0,
    };
    assert_eq!(
        transform.composite_to_layer_basis_points([3_750, 6_250]),
        [2_500, 7_500]
    );
    // An off-frame composite centre stays legal: CC5 §2.2's centre range
    // is deliberately wide so a tracked window may leave and re-enter.
    assert_eq!(
        transform.composite_to_layer_basis_points([0, 10_000]),
        [-5_000, 15_000]
    );
    // And the bounds clamp rather than wrapping.
    assert_eq!(
        transform.composite_to_layer_basis_points([-20_000, 30_000]),
        [
            kinewright_core::MATTE_WINDOW_CENTER_MIN_BASIS_POINTS,
            kinewright_core::MATTE_WINDOW_CENTER_MAX_BASIS_POINTS
        ]
    );
}

/// CC5 §5.2 / M40: the smoothing constants are pinned here, and the
/// three-sample median filter's last-sample lag is reproduced by hand.
#[test]
fn matte_track_smoothing_uses_the_pinned_m40_constants_and_lags_the_last_sample() {
    // A dead zone deliberately lags, which is wrong for a matte.
    assert_eq!(MATTE_TRACK_DEAD_ZONE_BASIS_POINTS, 0);
    // 8 % of the frame between samples.
    assert_eq!(MATTE_TRACK_MAX_STEP_BASIS_POINTS, 800);

    // A steadily moving subject, 200 bp per sample.
    let observations = [1_000_i64, 1_200, 1_400, 1_600, 1_800];
    let smoothed = kinewright_core::stabilize_tracked_centres_basis_points(
        &observations,
        kinewright_core::MATTE_WINDOW_CENTER_MIN_BASIS_POINTS,
        kinewright_core::MATTE_WINDOW_CENTER_MAX_BASIS_POINTS,
        MATTE_TRACK_DEAD_ZONE_BASIS_POINTS,
        MATTE_TRACK_MAX_STEP_BASIS_POINTS,
    );
    assert_eq!(smoothed.len(), observations.len());
    // CC5 §5.2's stated systematic lag: the filter replaces the final
    // sample with median(o[n-3], o[n-2], o[n-1]) = median(1400, 1600,
    // 1800) = 1600, one inter-sample displacement behind the true 1800.
    assert!(
        smoothed[4] <= 1_600,
        "the last smoothed value must lag by one inter-sample displacement, was {}",
        smoothed[4]
    );
    assert_eq!(observations[4] - smoothed[4], 200);

    // One-sample noise is rejected, which was M40's first fix.
    let noisy = [5_000_i64, 5_000, 9_000, 5_000, 5_000];
    let filtered = kinewright_core::stabilize_tracked_centres_basis_points(
        &noisy,
        kinewright_core::MATTE_WINDOW_CENTER_MIN_BASIS_POINTS,
        kinewright_core::MATTE_WINDOW_CENTER_MAX_BASIS_POINTS,
        MATTE_TRACK_DEAD_ZONE_BASIS_POINTS,
        MATTE_TRACK_MAX_STEP_BASIS_POINTS,
    );
    assert!(
        filtered.iter().all(|value| *value == 5_000),
        "a single 9000 spike must not survive the median filter: {filtered:?}"
    );
}

/// CC5 §5.2: a layer whose scale or offset moves across the tracked range
/// cannot be expressed as one affine map, so the tool refuses.
#[test]
fn static_layer_transform_refuses_a_keyframed_scale_or_offset() {
    let frames = [TimeCode(0), TimeCode(5), TimeCode(10)];

    // No transform effect at all is the identity.
    assert_eq!(
        resolve_static_layer_transform(&[], &frames).ok(),
        Some(LayerTransform::IDENTITY)
    );

    // A static transform resolves once and is accepted.
    let static_transform = [Effect {
        id: EffectId(2),
        name: "transform".to_owned(),
        parameters: BTreeMap::from([("scale_percent".to_owned(), ParamValue::Integer(50))]),
        keyframes: BTreeMap::new(),
    }];
    let resolved = resolve_static_layer_transform(&static_transform, &frames)
        .unwrap_or_else(|_| panic!("a static transform is one affine map"));
    assert!((resolved.scale - 0.5).abs() < f64::EPSILON);

    // A keyframe curve that resolves to one constant value is *also*
    // accepted: the rule is about the values the renderer uses, not about
    // the presence of automation.
    let mut constant = static_transform[0].clone();
    constant.keyframes.insert(
        "scale_percent".to_owned(),
        AutomationCurve {
            keyframes: vec![Keyframe {
                at: TimeCode(0),
                value: 50,
                interpolation: KeyframeInterpolation::Hold,
            }],
        },
    );
    assert!(resolve_static_layer_transform(&[constant], &frames).is_ok());

    // A moving scale is refused, with the field and both observed values.
    let mut moving = static_transform[0].clone();
    moving.keyframes.insert(
        "scale_percent".to_owned(),
        AutomationCurve {
            keyframes: vec![
                Keyframe {
                    at: TimeCode(0),
                    value: 50,
                    interpolation: KeyframeInterpolation::Linear,
                },
                Keyframe {
                    at: TimeCode(10),
                    value: 100,
                    interpolation: KeyframeInterpolation::Linear,
                },
            ],
        },
    );
    let unsupported = resolve_static_layer_transform(&[moving], &frames)
        .expect_err("a moving scale cannot be one affine map");
    assert_eq!(unsupported.field, "scale");
    assert_eq!(unsupported.observed["parameter"], "scale");
    assert_eq!(unsupported.observed["at_first_sample"], 0.5);
    assert_eq!(unsupported.observed["at_frame"], 5);

    // A moving offset is refused the same way.
    let mut moving_offset = static_transform[0].clone();
    moving_offset.keyframes.insert(
        "x_percent".to_owned(),
        AutomationCurve {
            keyframes: vec![
                Keyframe {
                    at: TimeCode(0),
                    value: 0,
                    interpolation: KeyframeInterpolation::Linear,
                },
                Keyframe {
                    at: TimeCode(10),
                    value: 20,
                    interpolation: KeyframeInterpolation::Linear,
                },
            ],
        },
    );
    assert_eq!(
        resolve_static_layer_transform(&[moving_offset], &frames)
            .err()
            .map(|unsupported| unsupported.field),
        Some("offset_x")
    );
}

/// CC5 §5.2, hand-worked: a tracked composite pixel becomes a *layer*
/// control value through a fraction-of-extent read and the inverse of the
/// compositor's placement, never through the tracker's `extent − 1`
/// lattice.
///
/// At `scale = 0.5` with `x_percent = y_percent = 20` the compositor
/// accumulates `offset = 20 / 50 = 0.4` on both axes, so the forward map is
/// `u_c = 0.5·(u_l − 0.5) + 0.2 + 0.5 = 0.5·u_l + 0.45` and its inverse is
/// `u_l = 2·u_c − 0.9`.
#[test]
fn tracked_centre_converts_to_layer_space_as_a_fraction_of_the_extent() {
    let transform = LayerTransform {
        scale: 0.5,
        offset_x: 0.4,
        offset_y: 0.4,
    };

    // Pixel 160 of 320: u_c = 160.5 / 320 = 0.5015625,
    // u_l = 2 · 0.5015625 − 0.9 = 0.103125.
    let layer = tracked_centre_layer_unit([160, 125], 320, 180, transform);
    assert!(
        (layer[0] - 0.103_125).abs() < 1e-12,
        "x converted to {}",
        layer[0]
    );
    // Pixel 125 of 180: u_c = 125.5 / 180 = 0.697222…,
    // u_l = 2 · 0.697222… − 0.9 = 0.494444….
    assert!(
        (layer[1] - 0.494_444_444_444_444_4).abs() < 1e-12,
        "y converted to {}",
        layer[1]
    );
    // 10.3125 percent rounds to 10; 1031.25 bp rounds to 1031.
    assert_eq!(layer_unit_to_percent(layer[0]), 10);
    assert_eq!(layer_unit_to_basis_points(layer[0]), 1_031);
    // 49.4444 percent rounds to 49; 4944.44 bp rounds to 4944.
    assert_eq!(layer_unit_to_percent(layer[1]), 49);
    assert_eq!(layer_unit_to_basis_points(layer[1]), 4_944);

    // Pixel 224 of 320: u_c = 224.5 / 320 = 0.7015625, u_l = 0.503125.
    let layer = tracked_centre_layer_unit([224, 125], 320, 180, transform);
    assert_eq!(layer_unit_to_percent(layer[0]), 50);
    assert_eq!(layer_unit_to_basis_points(layer[0]), 5_031);

    // The composite value the *unconverted* code wrote is nowhere near it:
    // round(224 · 100 / 319) = 70 percent against the layer's 50.
    assert_eq!(pixel_to_percent(224, 320), 70);

    // At the identity the conversion is the fraction-of-extent read alone,
    // which is the deliberate ≤ 1 unit correction over the old `extent − 1`
    // lattice: pixel 0 of 320 is 0.15625 percent of the extent, not 0.
    let identity = tracked_centre_layer_unit([0, 0], 320, 180, LayerTransform::IDENTITY);
    assert!((identity[0] - 0.001_562_5).abs() < 1e-12);
    assert_eq!(layer_unit_to_percent(identity[0]), 0);
    assert_eq!(layer_unit_to_basis_points(identity[0]), 16);
    // The middle of the raster agrees with the lattice to the percent.
    let middle = tracked_centre_layer_unit([160, 90], 320, 180, LayerTransform::IDENTITY);
    assert_eq!(layer_unit_to_percent(middle[0]), 50);
    assert_eq!(layer_unit_to_percent(middle[1]), 50);
    assert_eq!(
        layer_unit_to_percent(middle[0]),
        i64::from(pixel_to_percent(160, 320))
    );

    // Both writers clamp: a layer coordinate outside the layer's own quad
    // is a real possibility at scale < 1, and neither control accepts it.
    assert_eq!(layer_unit_to_percent(-0.02), 0);
    assert_eq!(layer_unit_to_percent(1.4), 100);
    assert_eq!(layer_unit_to_basis_points(-0.02), 0);
    assert_eq!(layer_unit_to_basis_points(1.4), 10_000);
}

/// CC5 §5.2: the mask and the reframe subject state a *full* extent in
/// layer percent, so the composite template is that extent times the scale.
/// Cross-checked against [`matte_track_box_percent`], whose input is a half
/// extent in basis points: a 50 percent region is `hw = 2500 bp`.
#[test]
fn tracked_box_percent_rescales_the_template_by_the_layer_scale() {
    assert_eq!(tracked_box_percent(40, 1.0), 40);
    assert_eq!(tracked_box_percent(40, 0.5), 20);
    assert_eq!(tracked_box_percent(70, 0.5), 35);
    // Out of the tracker's 1..=75 band in both directions.
    assert_eq!(tracked_box_percent(50, 2.0), 100);
    assert_eq!(tracked_box_percent(4, 0.1), 0);
    // The same rule the matte window already uses.
    assert_eq!(
        tracked_box_percent(50, 0.5),
        matte_track_box_percent(2_500, 0.5)
    );
}

/// CC5 §5.2: per-frame resolution is what lets a *keyframed* transform be
/// converted instead of refused. The same effect that
/// `resolve_static_layer_transform` rejects resolves cleanly at each frame.
#[test]
fn resolve_layer_transform_at_follows_a_keyframed_scale() {
    let mut moving = Effect {
        id: EffectId(2),
        name: "transform".to_owned(),
        parameters: BTreeMap::from([("scale_percent".to_owned(), ParamValue::Integer(100))]),
        keyframes: BTreeMap::new(),
    };
    moving.keyframes.insert(
        "scale_percent".to_owned(),
        AutomationCurve {
            keyframes: vec![
                Keyframe {
                    at: TimeCode(0),
                    value: 100,
                    interpolation: KeyframeInterpolation::Linear,
                },
                Keyframe {
                    at: TimeCode(40),
                    value: 50,
                    interpolation: KeyframeInterpolation::Linear,
                },
            ],
        },
    );
    let effects = [moving];
    // Linear from 100 to 50 over 40 frames: 100, 75, 50 at 0, 20, 40.
    for (frame, expected) in [(0_i64, 1.0_f64), (20, 0.75), (40, 0.5)] {
        let resolved = resolve_layer_transform_at(&effects, TimeCode(frame));
        assert!(
            (resolved.scale - expected).abs() < 1e-12,
            "frame {frame} resolved scale {}",
            resolved.scale
        );
        assert!(resolved.offset_x.abs() < f64::EPSILON);
        assert!(resolved.offset_y.abs() < f64::EPSILON);
    }
    // The static resolver still refuses it, which is why the two exist.
    assert!(
        resolve_static_layer_transform(&effects, &[TimeCode(0), TimeCode(40)]).is_err(),
        "a moving scale is not one affine map"
    );

    // A static chain resolves to the same values at every frame, and the
    // offsets are the compositor's own `percent / 50`.
    let static_chain = [Effect {
        id: EffectId(3),
        name: "transform".to_owned(),
        parameters: BTreeMap::from([
            ("scale_percent".to_owned(), ParamValue::Integer(50)),
            ("x_percent".to_owned(), ParamValue::Integer(20)),
            ("y_percent".to_owned(), ParamValue::Integer(20)),
        ]),
        keyframes: BTreeMap::new(),
    }];
    let resolved = resolve_layer_transform_at(&static_chain, TimeCode(7));
    assert!((resolved.scale - 0.5).abs() < f64::EPSILON);
    assert!((resolved.offset_x - 0.4).abs() < f64::EPSILON);
    assert!((resolved.offset_y - 0.4).abs() < f64::EPSILON);
}

#[test]
fn deterministic_tracker_follows_a_translated_region_exactly() {
    let previous = box_frame([8, 8]);
    let current = box_frame([13, 11]);
    let tracked = track_region(&previous, &current, [8, 8], [2, 2], 25);

    assert_eq!(tracked.center, [13, 11]);
    assert_eq!(tracked.confidence_basis_points, 10_000);
}

#[test]
fn tracking_samples_include_the_exact_last_visible_frame() {
    assert_eq!(
        tracking_sample_frames(TimeCode(3)..TimeCode(15), 5),
        vec![TimeCode(3), TimeCode(6), TimeCode(10), TimeCode(14)]
    );
}

#[test]
fn tracking_samples_distribute_non_divisible_spans_without_a_short_tail() {
    let frames = tracking_sample_frames(TimeCode(0)..TimeCode(12), 5);

    assert_eq!(
        frames,
        vec![TimeCode(0), TimeCode(3), TimeCode(7), TimeCode(11)]
    );
    let gaps = frames
        .windows(2)
        .map(|pair| pair[1].0 - pair[0].0)
        .collect::<Vec<_>>();
    assert_eq!(gaps, vec![3, 4, 4]);
    assert!(gaps.iter().all(|gap| *gap <= 5));
    assert!(gaps.windows(2).all(|pair| pair[0].abs_diff(pair[1]) <= 1));
}

#[test]
fn tracking_samples_handle_short_ranges_and_exact_division() {
    assert_eq!(
        tracking_sample_frames(TimeCode(7)..TimeCode(10), 10),
        vec![TimeCode(7), TimeCode(9)]
    );
    assert_eq!(
        tracking_sample_frames(TimeCode(4)..TimeCode(15), 5),
        vec![TimeCode(4), TimeCode(9), TimeCode(14)]
    );
    assert_eq!(
        tracking_sample_frames(TimeCode(6)..TimeCode(7), 5),
        vec![TimeCode(6)]
    );
}

#[test]
fn tracking_samples_are_unique_and_in_visible_range() {
    let frames = tracking_sample_frames(TimeCode(10)..TimeCode(31), 6);

    assert_eq!(frames.first(), Some(&TimeCode(10)));
    assert_eq!(frames.last(), Some(&TimeCode(30)));
    assert!(frames.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(frames.iter().all(|frame| (10..31).contains(&frame.0)));
}

#[test]
fn tracked_subject_constraints_match_the_failed_vertical_crop_edges() {
    let constraint = tracked_subject_focus_constraint(
        TrackedSubjectBounds {
            at: TimeCode(69),
            left_basis_points: 2_392,
            right_basis_points: 4_902,
            top_basis_points: 1_442,
            bottom_basis_points: 4_520,
        },
        352,
        288,
        5_625,
    )
    .unwrap();

    assert_eq!(constraint.min_x_basis_points, 2_600);
    assert_eq!(constraint.max_x_basis_points, 4_693);
    assert_eq!(constraint.min_y_basis_points, 0);
    assert_eq!(constraint.max_y_basis_points, 10_000);
}

#[test]
fn tracked_subject_constraints_preserve_the_right_edge_focus_plateau() {
    let constraint = tracked_subject_focus_constraint(
        TrackedSubjectBounds {
            at: TimeCode(235),
            left_basis_points: 1_921,
            right_basis_points: 4_432,
            top_basis_points: 3_557,
            bottom_basis_points: 6_635,
        },
        352,
        288,
        5_625,
    )
    .unwrap();

    assert_eq!(constraint.min_x_basis_points, 0);
    assert_eq!(constraint.max_x_basis_points, 4_222);
    assert_eq!(constraint.min_y_basis_points, 0);
    assert_eq!(constraint.max_y_basis_points, 10_000);
}
