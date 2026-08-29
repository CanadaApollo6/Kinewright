//! CC7 §11.2 items 1–11: the core-owned scenario-authority fixtures.
//!
//! Every expected value here is written out analytically from CC7 §2's tables
//! or transcribed **independently** in `f64` in this file (rule 11.0.1). No
//! expected value is obtained by calling `measure_color_qc`, `match_parameters`
//! (`kinewright-agent::color_scopes`), `bt709_limited_ycbcr`, `encode_bt709` /
//! `decode_display709` / `grade709_decode` (`kinewright-media::color_pipeline`),
//! `matte_coverage_statistics`, the compositor, or swscale.
//!
//! Fixtures that need a GPU compositor, an encoder, or a decoded file are the
//! media crate's half of CC7 and are not duplicated here.

use std::{collections::BTreeMap, path::Path};

use kinewright_core::{
    AssetId, Clip, ClipContent, ClipId, ColorContext, Document, LutAsset, LutAssetKind,
    LutAssetSource, MediaAsset, MediaKind, NormalizedRoi, Operation, ParamValue, Rational,
    TimeCode, Track, TrackId, TrackKind, apply_batch,
    cc7_scenarios::{
        CC7_B1_RESIDUAL_SPREAD_MAX_CODE, CC7_BUDGETS, CC7_C2_MAX_OVER_EXCURSION_MILLIONTHS,
        CC7_CAM_A_LUMA_PERCENTILES_CODE8, CC7_CAM_A_LUMA_PERCENTILES_CODE16, CC7_CAMERA_ORDER,
        CC7_CAMERA_PATCH_CODES, CC7_CHART_BAND_ROI, CC7_CHART_LINEAR_MILLIONTHS,
        CC7_CHART_PATCH_COUNT, CC7_CHART_PATCHES, CC7_D2_CONTINUOUS_AREA_WRONG_MODEL_PIXELS_TENTHS,
        CC7_D2_FEATHER_COUNTS_PIXELS, CC7_D2_WINDOW_CENTRE_BASIS_POINTS,
        CC7_D2_WINDOW_HALF_EXTENTS_BASIS_POINTS, CC7_DEEP_SHADOW_ROI,
        CC7_DELIVERY_ALLOWED_INFO_CODES, CC7_DELIVERY_LEG_BUDGET_SECONDS_LINUX,
        CC7_DELIVERY_TEN_PSNR_MEASURED_HUNDREDTHS, CC7_FEATHER_BASIS_POINTS,
        CC7_FEATHER_PARTIAL_TOLERANCE_PIXELS, CC7_LOG_BLACK_PATCH_REPORTED_CODE,
        CC7_LOG_CARRIER_LUMA_PERCENTILES_CODE8, CC7_LOG_CARRIER_LUMA_PERCENTILES_CODE16,
        CC7_LOG_CHART_CODES, CC7_LOG_CHART_INVERSE_CODES, CC7_LOG_CHART_INVERSE_ERROR_CODES,
        CC7_LOG_CUBE_SIZE, CC7_LOG_CUBE_SIZE_LADDER, CC7_LOG_FIRST_PERCENTILE_MIN_CODE8_PROSE,
        CC7_LOG_FIRST_PERCENTILE_MIN_CODE16, CC7_LOG_INVERSE_MAX_CODE,
        CC7_LOG_MID_GREY_ANCHOR_CODE, CC7_LOG_MID_GREY_ANCHOR_MILLIONTHS, CC7_LOG_OFFSET_STOPS,
        CC7_LOG_P99_MAX_CODE8_PROSE, CC7_LOG_P99_MAX_CODE16, CC7_LOG_ROW_CODES, CC7_LOG_SPAN_STOPS,
        CC7_LOG_SURROUND_CODE, CC7_LOG_UNITY_ANCHOR_CODE, CC7_LOG_UNITY_ANCHOR_MILLIONTHS,
        CC7_LOOK_BLUE_ZERO_CROSSING_LINEAR_MILLIONTHS, CC7_LOOK_DEEP_SHADOW_OUT_OF_GAMUT_PIXELS,
        CC7_MATCH_LUMA_MEAN_MAX_CODE_MILLIONTHS, CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE,
        CC7_MATCH_PROPOSAL_C1, CC7_MATTE_OUTSIDE_CHANGED_PIXELS_MAX, CC7_MEASURED_DELIVERY_EIGHT,
        CC7_MEASURED_DELIVERY_TEN, CC7_MEASURED_UNMATCHED_B_SPREAD_CODE, CC7_PATCH_COUNT,
        CC7_PATCH_NAMES, CC7_PRIMARY_BAND_ROI, CC7_PRIMARY_PATCH_COUNT, CC7_PRIMARY_PATCHES,
        CC7_PRODUCT_RED_ROI, CC7_RAMP_ROI, CC7_RASTER_PIXELS, CC7_REGION_POPULATIONS,
        CC7_ROW_BAND_ROI, CC7_ROW_PATCH_COUNT, CC7_ROW_PATCHES, CC7_SCENARIO_SPECS, CC7_SCENARIOS,
        CC7_SCOPE_SIXTEEN_BIT_SCALE, CC7_SKIN_BAND_ROI, CC7_SOURCE_FPS, CC7_SOURCE_FRAMES,
        CC7_SOURCE_HEIGHT, CC7_SOURCE_WIDTH, CC7_SPEC_F64_TOLERANCE, CC7_SURROUND_CODE,
        CC7_TRACK_AMPLITUDE_X_PIXELS, CC7_TRACK_AMPLITUDE_Y_PIXELS,
        CC7_TRACK_ANALYTIC_CENTRES_BASIS_POINTS, CC7_TRACK_CENTRE_X_PIXELS,
        CC7_TRACK_CENTRE_Y_PIXELS, CC7_TRACK_CLEAN_CONFIDENCE_MIN_REPORTED,
        CC7_TRACK_CONFIDENCE_SEPARATION_MIN_BASIS_POINTS,
        CC7_TRACK_CONTAINMENT_REQUIRED_HALF_HEIGHT_PIXELS_REPORTED,
        CC7_TRACK_CONTAINMENT_REQUIRED_HALF_WIDTH_PIXELS_REPORTED,
        CC7_TRACK_CONTAINMENT_WORST_MARGIN_X_PIXELS_HUNDREDTHS,
        CC7_TRACK_CONTAINMENT_WORST_MARGIN_Y_PIXELS_HUNDREDTHS,
        CC7_TRACK_EXPECTED_LOW_CONFIDENCE_FRAMES, CC7_TRACK_F2_SAMPLE_FRAMES,
        CC7_TRACK_F2_STEP_FRAMES, CC7_TRACK_FRAMES, CC7_TRACK_MAX_STEP_BASIS_POINTS_RESTATED,
        CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS, CC7_TRACK_OBSERVATION_BUDGET_ROW,
        CC7_TRACK_OBSERVED_CENTRES_BASIS_POINTS, CC7_TRACK_OCCLUDED_CONFIDENCE_MAX_REPORTED,
        CC7_TRACK_RANGE_END_LOCAL_FRAME, CC7_TRACK_RANGE_START_LOCAL_FRAME, CC7_TRACK_SQUARE_SIZE,
        CC7_TRACK_STATIC_PATCH_BOTTOM, CC7_TRACK_STEP_FRAMES, CC7_TRACK_SURVIVING_SAMPLE_COUNT,
        CC7_TRACK_SURVIVING_SAMPLE_FRAMES, CC7_TRACK_TOLERANCE_BASIS_POINTS,
        CC7_TRACK_WINDOW_HALF_HEIGHT_BASIS_POINTS, CC7_TRACK_WINDOW_HALF_WIDTH_BASIS_POINTS,
        CC7_TRACK_WORST_RAW_OBSERVATION_ERROR_BASIS_POINTS_REPORTED, Cc7Budget, Cc7BudgetKind,
        Cc7Camera, Cc7PersonPath, Cc7Scenario, cc7_analytic_square_centre_basis_points,
        cc7_analytic_square_top_left, cc7_as_f64, cc7_b1_canonical_operations, cc7_camera_code,
        cc7_camera_patch_codes, cc7_canonical_operations, cc7_d2_canonical_operations,
        cc7_decode_display709, cc7_display_code, cc7_encode_bt709, cc7_grade709_decode,
        cc7_log_encode_code, cc7_log_inverse_display, cc7_lut_backed_canonical_operations,
        cc7_millionths, cc7_round_half_away_from_zero, cc7_spec, cc7_stabilized_centres,
        cc7_track_keyframe_centres, cc7_tracking_sample_frames, cc7_tracking_sample_frames_for,
    },
    effect_descriptor, stabilize_tracked_centres_basis_points,
};

// ---------------------------------------------------------------------------
// Neighbouring constants CC7 asserts distinctness from.
//
// Each is `pub(crate)` or private in a crate `kinewright-core` cannot see, so
// it is restated here with its owner named, exactly as R-M2 restates the three
// transfer functions. A transcription with a named owner is a boundary.
// ---------------------------------------------------------------------------

/// `MONITOR_CPU_GPU_MAX`, `kinewright-media::cc1_fixtures:62` (`pub(crate)`).
const NEIGHBOUR_MONITOR_CPU_GPU_MAX_CODE: i64 = 2;
/// `DELIVERY_CODEC_MAX`, `kinewright-media::cc1_fixtures:68` (private).
const NEIGHBOUR_DELIVERY_CODEC_MAX_CODE: i64 = 4;
/// `MATTE_TRACK_MAX_STEP_BASIS_POINTS`,
/// `kinewright-agent::server:11196` (`pub(crate)`).
const NEIGHBOUR_MATTE_TRACK_MAX_STEP_BASIS_POINTS: i64 = 800;
/// `DEFAULT_MATTE_TRACK_MINIMUM_CONFIDENCE_BASIS_POINTS`,
/// `kinewright-agent::server:11199` (private). Probe-2 measured that this
/// default drops nothing, which is why CC7 must not reuse it.
const NEIGHBOUR_DEFAULT_MATTE_TRACK_MINIMUM_CONFIDENCE_BASIS_POINTS: i64 = 5_000;
/// `MATTE_TRACK_DEAD_ZONE_BASIS_POINTS`, `kinewright-agent::server:11192`.
/// Deliberately **0**, so distinctness from it is trivially true for every
/// positive CC7 constant and CC7 does not assert it (CC7 §2.6, minor 4).
const NEIGHBOUR_MATTE_TRACK_DEAD_ZONE_BASIS_POINTS: i64 = 0;

// ---------------------------------------------------------------------------
// Independent transcriptions. None of these calls the module under test.
// ---------------------------------------------------------------------------

/// The BT.709 display transfer, transcribed a second time from CC1 §3.2's
/// equations so `cc7_scenarios`' own copy has something to be checked against.
fn reference_encode(linear: f64) -> f64 {
    if linear < 0.0 {
        -reference_encode(-linear)
    } else if linear < 0.018 {
        4.5 * linear
    } else {
        1.099 * linear.powf(0.45) - 0.099
    }
}

/// CC1's *source* decode `decode_bt709`
/// (`kinewright-media::color_pipeline:309-315`), which is **not**
/// `grade709_decode`: it carries the rounded broadcast constants and has no
/// sign extension. §11.2.3's failing direction swaps one for the other.
fn reference_decode_bt709(encoded: f64) -> f64 {
    if encoded < 0.081 {
        encoded / 4.5
    } else {
        ((encoded + 0.099) / 1.099).powf(1.0 / 0.45)
    }
}

/// CC3's `grade709` decode, transcribed a second time from CC3 §2.1.
fn reference_grade709_decode(encoded: f64) -> f64 {
    const ALPHA: f64 = 1.099_296_8;
    const BETA_ENCODED: f64 = 0.081_242_86;
    const K: f64 = 0.099_296_8;
    const INVERSE_EXPONENT: f64 = 2.222_222_3;
    if encoded.abs() < BETA_ENCODED {
        encoded / 4.5
    } else {
        ((encoded.abs() + K) / ALPHA).powf(INVERSE_EXPONENT) * encoded.signum()
    }
}

/// One named region: its manifest name, its ROI, and the pixel rect it must
/// resolve to.
type Cc7NamedRegion = (&'static str, NormalizedRoi, (u32, u32, u32, u32));

/// `round(255 · v)` half away from zero, clamped, transcribed here.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn reference_code(display: f64) -> u8 {
    let scaled = display * 255.0;
    let rounded = if scaled >= 0.0 {
        (scaled + 0.5).floor()
    } else {
        -((-scaled + 0.5).floor())
    };
    rounded.clamp(0.0, 255.0) as u8
}

/// CC7 §2.4.2's curve, transcribed here so the module's own copy is checked
/// rather than trusted.
fn reference_log_value(linear: f64) -> f64 {
    if linear <= 0.0 {
        return 0.0;
    }
    ((linear.log2() + 8.0) / 12.0).clamp(0.0, 1.0)
}

/// CC7 §2.4.2's curve **without the clamp**: the failing direction of
/// §11.2.5. `log2(0) = -inf`, so black inverts to `0` rather than to `4`.
fn reference_log_value_unclamped(linear: f64) -> f64 {
    (linear.log2() + 8.0) / 12.0
}

/// The exact inverse of the curve, back to a monitoring code.
fn reference_log_inverse_code(value: f64) -> u8 {
    reference_code(reference_encode(2.0_f64.powf(12.0 * value - 8.0)))
}

// ===========================================================================
// §11.2.1 — key `raster.regions`.
// ===========================================================================

/// Every §2.3.3 and §2.5 rectangle resolves to the pixel rect it claims.
///
/// **The `ceil` on the start is load-bearing** (A19). The failing direction is
/// asserted, not described: the naive `4222` for the patch row's `y 76` —
/// `76 · 10000/180 = 4222.2̄` truncated — resolves to `y 75, h 17`, which is
/// **204** pixels rather than 192, because the floored start lands one row
/// early. `10_000/180 = 55.5̄` is exact only on 9-pixel boundaries and the row
/// boundaries 20, 36, 52, 56, 72, 76 and 92 are not multiples of 9, so the
/// round trip is asserted rather than a divisibility that does not hold.
#[test]
#[allow(clippy::too_many_lines)]
fn cc7_scenario_geometry_round_trips_through_normalized_roi() {
    let resolve = |roi: NormalizedRoi| {
        roi.to_pixels(CC7_SOURCE_WIDTH, CC7_SOURCE_HEIGHT)
            .expect("every CC7 ROI resolves on the 320 x 180 raster")
    };

    // The five named regions of §2.3.3, plus the derived region ROIs the gates
    // use, plus (d2)'s window patch.
    let regions: [Cc7NamedRegion; 8] = [
        ("neutral_ramp_band", CC7_RAMP_ROI, (0, 0, 320, 20)),
        ("achromatic_chart_band", CC7_CHART_BAND_ROI, (0, 36, 96, 16)),
        ("primaries_band", CC7_PRIMARY_BAND_ROI, (0, 56, 40, 16)),
        ("patch_row", CC7_ROW_BAND_ROI, (0, 76, 84, 16)),
        ("skin_band", CC7_SKIN_BAND_ROI, (0, 76, 48, 16)),
        ("product_red", CC7_PRODUCT_RED_ROI, (48, 76, 12, 16)),
        ("deep_shadow", CC7_DEEP_SHADOW_ROI, (72, 76, 12, 16)),
        // §2.3.3's own worked example: the primaries band's `y 56..72`.
        (
            "primaries_band_bp",
            NormalizedRoi::new(0, 3_112, 1_250, 888),
            (0, 56, 40, 16),
        ),
    ];
    for (name, roi, expected) in regions {
        let pixels = resolve(roi);
        assert_eq!(
            (pixels.x, pixels.y, pixels.width, pixels.height),
            expected,
            "{name}: {roi:?} must resolve to {expected:?}"
        );
    }

    // Every patch rect, chart then primaries then row.
    for patch in CC7_CHART_PATCHES
        .iter()
        .chain(CC7_PRIMARY_PATCHES.iter())
        .chain(CC7_ROW_PATCHES.iter())
    {
        let pixels = resolve(patch.roi);
        assert_eq!(
            (pixels.x, pixels.y, pixels.width, pixels.height),
            (
                patch.rect.x,
                patch.rect.y,
                patch.rect.width,
                patch.rect.height
            ),
            "{}: {:?} must resolve to its pixel rect",
            patch.name,
            patch.roi
        );
    }

    // Failing direction: the naive floored start misses by one pixel row.
    let naive_deep_shadow = NormalizedRoi::new(2_250, 4_222, 375, 888);
    let naive = resolve(naive_deep_shadow);
    assert_eq!(
        (naive.y, naive.height),
        (75, 17),
        "the naive floored start must resolve one row early"
    );
    assert_eq!(
        naive.width * naive.height,
        204,
        "the naive rect must measure 204 pixels, not 192"
    );
    assert_ne!(
        naive.height,
        CC7_DEEP_SHADOW_ROI
            .to_pixels(CC7_SOURCE_WIDTH, CC7_SOURCE_HEIGHT)
            .expect("the ceil'ed deep_shadow ROI resolves")
            .height
    );

    // The population table and its sum.
    let total: u32 = CC7_REGION_POPULATIONS.iter().map(|(_, count)| count).sum();
    assert_eq!(total, CC7_RASTER_PIXELS);
    assert_eq!(CC7_RASTER_PIXELS, 57_600);
    assert_eq!(CC7_RASTER_PIXELS, CC7_SOURCE_WIDTH * CC7_SOURCE_HEIGHT);
    let named: [(u32, u32); 4] = [
        (CC7_REGION_POPULATIONS[0].1, 320 * 20),
        (CC7_REGION_POPULATIONS[1].1, 96 * 16),
        (CC7_REGION_POPULATIONS[2].1, 40 * 16),
        (CC7_REGION_POPULATIONS[3].1, 84 * 16),
    ];
    for (declared, derived) in named {
        assert_eq!(declared, derived);
    }
}

// ===========================================================================
// §11.2.2 — keys `patches.chart`, `patches.primaries`.
// ===========================================================================

/// The twelve achromatic codes and the five primaries, with no pure red (A1).
#[test]
#[allow(clippy::too_many_lines)]
fn cc7_chart_and_primary_codes_are_the_contract_table() {
    const EXPECTED: [u8; CC7_CHART_PATCH_COUNT] =
        [0, 11, 24, 48, 72, 104, 128, 152, 180, 208, 242, 255];
    const PRIMARIES: [[u8; 3]; CC7_PRIMARY_PATCH_COUNT] = [
        [0, 255, 0],
        [0, 0, 255],
        [0, 255, 255],
        [255, 0, 255],
        [255, 255, 0],
    ];
    assert_eq!(CC7_CHART_PATCHES.len(), CC7_CHART_PATCH_COUNT);
    for (patch, expected) in CC7_CHART_PATCHES.iter().zip(EXPECTED) {
        assert_eq!(
            patch.display_code_cam_a,
            [expected, expected, expected],
            "{} must carry CC1's step {expected}",
            patch.name
        );
        let [red, green, blue] = patch.display_code_cam_a;
        assert!(
            red == green && green == blue,
            "{} must be achromatic; a chart patch that is not makes (a)'s spread statistic meaningless",
            patch.name
        );
        assert_eq!(patch.grade709, None, "a chart patch is authored as a code");
        assert_eq!(patch.rect.pixels(), 128);
    }
    assert!(
        EXPECTED.windows(2).all(|pair| pair[0] < pair[1]),
        "the chart band must be strictly increasing"
    );

    assert_eq!(CC7_PRIMARY_PATCHES.len(), CC7_PRIMARY_PATCH_COUNT);
    assert_eq!(CC7_PRIMARY_PATCH_COUNT, 5);
    for (patch, expected) in CC7_PRIMARY_PATCHES.iter().zip(PRIMARIES) {
        assert_eq!(patch.display_code_cam_a, expected, "{}", patch.name);
        assert_eq!(patch.rect.pixels(), 128);
    }
    for patch in &CC7_PRIMARY_PATCHES {
        assert_ne!(
            patch.display_code_cam_a,
            [255, 0, 0],
            "the pure red primary is deliberately absent (A1): the derived product_red qualifier captures it and (d)'s exact containment could not pass"
        );
    }

    // The names are the manifest's, and the whole set is 24 patches.
    assert_eq!(CC7_PATCH_NAMES.len(), CC7_PATCH_COUNT);
    assert_eq!(
        CC7_PATCH_COUNT,
        CC7_CHART_PATCH_COUNT + CC7_PRIMARY_PATCH_COUNT + CC7_ROW_PATCH_COUNT
    );
    let names = CC7_CHART_PATCHES
        .iter()
        .chain(CC7_PRIMARY_PATCHES.iter())
        .chain(CC7_ROW_PATCHES.iter())
        .map(|patch| patch.name)
        .collect::<Vec<_>>();
    assert_eq!(names, CC7_PATCH_NAMES.to_vec());
}

// ===========================================================================
// §11.2.3 — key `patches.cam_a`.
// ===========================================================================

/// §2.4.1's table, transcribed independently, within `SPEC_F64_TOLERANCE`.
///
/// The stated path is grade709 → linear → display709 → round, and
/// `decode_display709` does **not** appear in it. The free cross-check is that
/// the two encodings differ only in the fourth decimal, so every patch's code
/// equals `round(255 · g)` as well; the fixture computes the stated path and
/// asserts the agreement rather than substituting it.
///
/// *Fails:* a transcription that swaps `decode_bt709` for `grade709_decode`
/// differs on **`skin_light`**, whose grade709 `0.85` is in the power segment.
/// `deep_shadow` is deliberately not the failing patch (R-M14): its `0.05`
/// sits below `GRADE709_BETA_ENCODED = 0.081_242_86` and below
/// `decode_display709`'s `0.081`, so all three decodes return `0.05/4.5` and
/// agree exactly — a failing direction placed there would pass.
#[test]
#[allow(clippy::too_many_lines)]
fn cc7_camera_a_patch_codes_are_the_hand_derived_display_encoding() {
    const LINEAR: [[f64; 3]; 7] = [
        [0.721_798, 0.465_557, 0.365_963],
        [0.520_332, 0.289_498, 0.205_445],
        [0.310_342, 0.158_076, 0.105_347],
        [0.117_434, 0.055_516, 0.036_983],
        [0.640_023, 0.022_489, 0.027_814],
        [0.022_489, 0.426_664, 0.563_622],
        [0.011_111, 0.011_111, 0.011_111],
    ];
    const DISPLAY: [[f64; 3]; 7] = [
        [0.850_040, 0.680_086, 0.600_108],
        [0.720_076, 0.530_127, 0.440_151],
        [0.550_121, 0.380_167, 0.300_189],
        [0.320_184, 0.200_216, 0.150_229],
        [0.800_054, 0.100_243, 0.120_238],
        [0.100_243, 0.650_094, 0.750_067],
        [0.050_000, 0.050_000, 0.050_000],
    ];
    const CODES: [[u8; 3]; 7] = [
        [217, 173, 153],
        [184, 135, 112],
        [140, 97, 77],
        [82, 51, 38],
        [204, 26, 31],
        [26, 166, 191],
        [13, 13, 13],
    ];
    // The contract's table is printed to six decimals, so the comparison
    // tolerance is a printed half-ulp rather than `SPEC_F64_TOLERANCE`.
    const PRINTED_TOLERANCE: f64 = 5e-7;

    assert_eq!(CC7_ROW_PATCHES.len(), CC7_ROW_PATCH_COUNT);
    for (index, patch) in CC7_ROW_PATCHES.iter().enumerate() {
        let grade709 = patch.grade709.expect("a row patch carries its grade709");
        for channel in 0..3 {
            let g = cc7_as_f64(grade709[channel]) / 1_000_000.0;
            let linear = reference_grade709_decode(g);
            let display = reference_encode(linear);
            assert!(
                (linear - LINEAR[index][channel]).abs() <= PRINTED_TOLERANCE,
                "{} channel {channel}: linear {linear} against the table's {}",
                patch.name,
                LINEAR[index][channel]
            );
            assert!(
                (display - DISPLAY[index][channel]).abs() <= PRINTED_TOLERANCE,
                "{} channel {channel}: display {display} against the table's {}",
                patch.name,
                DISPLAY[index][channel]
            );
            assert_eq!(
                reference_code(display),
                CODES[index][channel],
                "{}",
                patch.name
            );
            // The module's own value is the same number.
            assert!(
                (cc7_as_f64(patch.linear_millionths_cam_a[channel]) / 1_000_000.0 - linear).abs()
                    <= CC7_SPEC_F64_TOLERANCE,
                "{}: the module's linear millionths must be the transcribed value",
                patch.name
            );
            // The free cross-check: `round(255 · g)` agrees on every patch.
            assert_eq!(
                reference_code(g),
                CODES[index][channel],
                "{} channel {channel}: the two encodings differ only in the fourth decimal",
                patch.name
            );
        }
        assert_eq!(patch.display_code_cam_a, CODES[index], "{}", patch.name);
        assert_eq!(patch.rect.pixels(), 192);
    }

    // The surround, stated in §2.3.3 rather than in the row table.
    assert_eq!(
        reference_code(reference_encode(reference_grade709_decode(0.45))),
        CC7_SURROUND_CODE
    );

    // Failing direction: `decode_bt709` is not `grade709_decode`, and the
    // difference is visible on `skin_light` and invisible on `deep_shadow`.
    let skin_light = 0.85_f64;
    assert!(
        (reference_decode_bt709(skin_light) - reference_grade709_decode(skin_light)).abs()
            > CC7_SPEC_F64_TOLERANCE,
        "the two decodes must disagree on skin_light, or the failing direction is vacuous"
    );
    let deep_shadow = 0.05_f64;
    assert!(
        (reference_decode_bt709(deep_shadow) - reference_grade709_decode(deep_shadow)).abs()
            <= CC7_SPEC_F64_TOLERANCE,
        "the two decodes must agree on deep_shadow, which is why R-M14 moved the failing direction"
    );

    // The module's own transcriptions agree with this file's second ones.
    for step in 0..=1_000 {
        let value = f64::from(step) / 1_000.0;
        assert!(
            (cc7_encode_bt709(value) - reference_encode(value)).abs() <= CC7_SPEC_F64_TOLERANCE
        );
        assert!(
            (cc7_grade709_decode(value) - reference_grade709_decode(value)).abs()
                <= CC7_SPEC_F64_TOLERANCE
        );
        assert!(
            (cc7_decode_display709(value) - reference_decode_bt709(value)).abs()
                <= CC7_SPEC_F64_TOLERANCE
        );
    }
    assert!((cc7_encode_bt709(0.0)).abs() <= CC7_SPEC_F64_TOLERANCE);
    assert!((cc7_grade709_decode(0.0)).abs() <= CC7_SPEC_F64_TOLERANCE);
    assert!((cc7_decode_display709(0.0)).abs() <= CC7_SPEC_F64_TOLERANCE);
    assert!(
        (cc7_encode_bt709(-0.5) + cc7_encode_bt709(0.5)).abs() <= CC7_SPEC_F64_TOLERANCE,
        "the monitor encoding must be sign preserving"
    );
    assert_eq!(cc7_display_code(1.5), 255, "the display code clamps");
    assert_eq!(cc7_round_half_away_from_zero(-0.5), -1);
    assert_eq!(cc7_round_half_away_from_zero(0.5), 1);
    assert_eq!(cc7_millionths(0.450_148), 450_148);
}

// ===========================================================================
// §11.2.4 — key `log.curve`.
// ===========================================================================

/// §2.4.2's anchors, its twelve stored codes, its seven row patches, and the
/// **unit** the (c) signature gate is stated in.
///
/// `ChannelStatistics::{first_percentile, ninety_ninth_percentile}` are 16-bit
/// codes — the 8-bit value × 257 (`scopes.rs:576-586`, produced at
/// `:1330-1339`) — while `mean_code_values.luma` is an 8-bit *mean* and is the
/// wrong field. Left in 8 bits the p1 gate would have passed on every source
/// and the p99 gate failed on every source (A21).
///
/// *Fails:* the base scene's own codes differ from the carrier's on at least
/// eight chart patches, asserted.
#[test]
#[allow(clippy::too_many_lines)]
fn cc7_log_curve_anchors_and_patch_codes_are_the_contract_table() {
    assert_eq!(CC7_LOG_OFFSET_STOPS, 8);
    assert_eq!(CC7_LOG_SPAN_STOPS, 12);

    // Anchors. `v(1.0) = 2/3`; the brief's `0.4589` for 18 % grey did not
    // satisfy its own formula and is superseded by the formula.
    assert!(
        (reference_log_value(1.0) - 2.0 / 3.0).abs() <= CC7_SPEC_F64_TOLERANCE,
        "v(1.0) must be exactly two thirds"
    );
    assert_eq!(
        cc7_millionths(reference_log_value(1.0)),
        CC7_LOG_UNITY_ANCHOR_MILLIONTHS
    );
    assert_eq!(
        reference_code(reference_log_value(1.0)),
        CC7_LOG_UNITY_ANCHOR_CODE
    );
    assert_eq!(
        cc7_millionths(reference_log_value(0.18)),
        CC7_LOG_MID_GREY_ANCHOR_MILLIONTHS
    );
    assert_eq!(
        reference_code(reference_log_value(0.18)),
        CC7_LOG_MID_GREY_ANCHOR_CODE
    );
    assert_ne!(
        CC7_LOG_MID_GREY_ANCHOR_MILLIONTHS, 458_900,
        "the brief's 0.4589 did not satisfy its own formula"
    );

    // The floor: every linear below `2^-8` stores `v = 0`.
    assert_eq!(cc7_millionths(2.0_f64.powi(-8)), 3_906);
    assert_eq!(cc7_log_encode_code(3_905), 0);
    assert_eq!(cc7_log_encode_code(0), 0);

    // The twelve stored codes, from the chart patches' analytic linear.
    let mut differing = 0;
    for (index, patch) in CC7_CHART_PATCHES.iter().enumerate() {
        let linear = reference_decode_bt709(f64::from(patch.display_code_cam_a[0]) / 255.0);
        assert!(
            (cc7_as_f64(CC7_CHART_LINEAR_MILLIONTHS[index]) / 1_000_000.0 - linear).abs()
                <= CC7_SPEC_F64_TOLERANCE,
            "{}: the declared linear must be the transcribed one",
            patch.name
        );
        let stored = reference_code(reference_log_value(linear));
        assert_eq!(
            stored, CC7_LOG_CHART_CODES[index],
            "{}: the stored log code must be the contract table's",
            patch.name
        );
        assert_eq!(
            stored,
            cc7_log_encode_code(CC7_CHART_LINEAR_MILLIONTHS[index])
        );
        if stored != patch.display_code_cam_a[0] {
            differing += 1;
        }
    }
    assert!(
        differing >= 8,
        "the carrier must differ from the base scene on at least eight chart patches, not {differing}"
    );

    // The seven row patches through the same curve, fed the analytic grade709
    // linear rather than the decoded 8-bit code: feeding the code instead
    // gives skin_light 160,146,139 and deep_shadow 33, which is wrong.
    for (index, patch) in CC7_ROW_PATCHES.iter().enumerate() {
        let grade709 = patch.grade709.expect("a row patch carries its grade709");
        for channel in 0..3 {
            let linear = reference_grade709_decode(cc7_as_f64(grade709[channel]) / 1_000_000.0);
            assert_eq!(
                reference_code(reference_log_value(linear)),
                CC7_LOG_ROW_CODES[index][channel],
                "{} channel {channel}",
                patch.name
            );
        }
    }
    // Feeding the decoded 8-bit code instead of the analytic linear gives a
    // different picture. On `skin_light` the difference is one code on green
    // — 160,146,139 against the analytic 160,147,139 — so the assertion is
    // over the whole triple, never over the red channel alone, which agrees.
    let wrong_path = CC7_ROW_PATCHES[0].display_code_cam_a.map(|code| {
        reference_code(reference_log_value(reference_decode_bt709(
            f64::from(code) / 255.0,
        )))
    });
    assert_eq!(wrong_path, [160, 146, 139], "the wrong path's own value");
    assert_ne!(
        wrong_path, CC7_LOG_ROW_CODES[0],
        "feeding the decoded 8-bit code instead of the analytic linear must be visibly different"
    );
    assert_eq!(
        wrong_path[0], CC7_LOG_ROW_CODES[0][0],
        "the red channel agrees, which is why the comparison is over the triple"
    );
    let wrong_deep_shadow = CC7_ROW_PATCHES[6].display_code_cam_a.map(|code| {
        reference_code(reference_log_value(reference_decode_bt709(
            f64::from(code) / 255.0,
        )))
    });
    assert_eq!(
        wrong_deep_shadow,
        [33, 33, 33],
        "the wrong path's deep_shadow"
    );
    assert_ne!(wrong_deep_shadow, CC7_LOG_ROW_CODES[6]);
    assert_eq!(
        reference_code(reference_log_value(reference_grade709_decode(0.45))),
        CC7_LOG_SURROUND_CODE
    );

    // The unit, so no implementer compares an 8-bit constant against a 16-bit
    // JSON number.
    assert_eq!(CC7_SCOPE_SIXTEEN_BIT_SCALE, 257);
    assert_eq!(
        CC7_LOG_FIRST_PERCENTILE_MIN_CODE16,
        CC7_LOG_FIRST_PERCENTILE_MIN_CODE8_PROSE * CC7_SCOPE_SIXTEEN_BIT_SCALE
    );
    assert_eq!(
        CC7_LOG_P99_MAX_CODE16,
        CC7_LOG_P99_MAX_CODE8_PROSE * CC7_SCOPE_SIXTEEN_BIT_SCALE
    );
    for index in 0..3 {
        assert_eq!(
            CC7_LOG_CARRIER_LUMA_PERCENTILES_CODE16[index],
            CC7_LOG_CARRIER_LUMA_PERCENTILES_CODE8[index] * CC7_SCOPE_SIXTEEN_BIT_SCALE
        );
        assert_eq!(
            CC7_CAM_A_LUMA_PERCENTILES_CODE16[index],
            CC7_CAM_A_LUMA_PERCENTILES_CODE8[index] * CC7_SCOPE_SIXTEEN_BIT_SCALE
        );
    }
    // The gate separates the carrier from cam A in the 16-bit unit …
    assert!(CC7_LOG_CARRIER_LUMA_PERCENTILES_CODE16[0] >= CC7_LOG_FIRST_PERCENTILE_MIN_CODE16);
    assert!(CC7_LOG_CARRIER_LUMA_PERCENTILES_CODE16[2] <= CC7_LOG_P99_MAX_CODE16);
    assert!(CC7_CAM_A_LUMA_PERCENTILES_CODE16[0] < CC7_LOG_FIRST_PERCENTILE_MIN_CODE16);
    assert!(CC7_CAM_A_LUMA_PERCENTILES_CODE16[2] > CC7_LOG_P99_MAX_CODE16);
    // … and would have been vacuous in 8 bits, which is A21's whole point.
    assert!(
        CC7_CAM_A_LUMA_PERCENTILES_CODE16[0] > CC7_LOG_FIRST_PERCENTILE_MIN_CODE8_PROSE,
        "an 8-bit p1 floor would have passed on every source"
    );
    assert!(
        CC7_LOG_CARRIER_LUMA_PERCENTILES_CODE16[2] > CC7_LOG_P99_MAX_CODE8_PROSE,
        "an 8-bit p99 ceiling would have failed on every source"
    );
}

// ===========================================================================
// §11.2.5 — key `log.round_trip`.
// ===========================================================================

/// §2.4.2's exact-inverse error column, and the two structural floors.
///
/// *Fails:* a curve **without** the clamp round-trips black to `0`, proving
/// the `+4` is the clamp at `v = 0` and not the arithmetic (A2).
#[test]
#[allow(clippy::too_many_lines)]
fn cc7_log_inverse_error_floors_are_properties_of_the_curve() {
    for (index, patch) in CC7_CHART_PATCHES.iter().enumerate() {
        let source = patch.display_code_cam_a[0];
        let linear = reference_decode_bt709(f64::from(source) / 255.0);
        let stored = reference_code(reference_log_value(linear));
        let back = reference_log_inverse_code(f64::from(stored) / 255.0);
        assert_eq!(stored, CC7_LOG_CHART_CODES[index], "{}", patch.name);
        assert_eq!(back, CC7_LOG_CHART_INVERSE_CODES[index], "{}", patch.name);
        let error = i64::from(back) - i64::from(source);
        assert_eq!(
            error, CC7_LOG_CHART_INVERSE_ERROR_CODES[index],
            "{}: the error column is the contract's",
            patch.name
        );
        // The module's own inverse agrees with this file's.
        assert_eq!(
            cc7_display_code(
                cc7_as_f64(cc7_log_inverse_display(cc7_millionths(
                    f64::from(stored) / 255.0
                ))) / 1_000_000.0
            ),
            back,
            "{}",
            patch.name
        );
        if index == 0 {
            assert_eq!(
                error, CC7_LOG_BLACK_PATCH_REPORTED_CODE,
                "black's +4 is the clamped channel"
            );
        } else {
            assert!(
                error.abs() <= 2,
                "{}: every unclamped chart channel must land within two codes, not {error}",
                patch.name
            );
        }
    }

    // Failing direction: without the clamp, `log2(0) = -inf` and black inverts
    // to 0 rather than to 4, so the +4 is the clamp and not the arithmetic.
    let unclamped_black = reference_log_value_unclamped(0.0);
    assert!(unclamped_black.is_infinite() && unclamped_black.is_sign_negative());
    assert_eq!(
        reference_log_inverse_code(unclamped_black),
        0,
        "the unclamped curve must round-trip black to zero"
    );
    assert_ne!(
        i64::from(reference_log_inverse_code(unclamped_black)),
        CC7_LOG_BLACK_PATCH_REPORTED_CODE
    );

    // The size ladder is monotone non-increasing, size 17 genuinely fails, and
    // the pinned size is 65 rather than the size a selection rule would pick.
    assert_eq!(CC7_LOG_CUBE_SIZE, 65);
    let ladder = CC7_LOG_CUBE_SIZE_LADDER;
    assert!(
        ladder
            .windows(2)
            .all(|pair| pair[0].0 < pair[1].0 && pair[0].1 >= pair[1].1),
        "the sweep must be monotone non-increasing with size"
    );
    assert!(ladder[0].1 > CC7_LOG_INVERSE_MAX_CODE, "size 17 must fail");
    assert!(ladder[1].1 <= CC7_LOG_INVERSE_MAX_CODE);
    assert!(ladder[2].1 < ladder[1].1);
    assert_eq!(ladder[2].0, CC7_LOG_CUBE_SIZE);
    assert!(
        CC7_LOG_INVERSE_MAX_CODE < 2 * ladder[1].1,
        "read as a selection rule the sweep would choose 33 at a 1.7x margin, which is why the size is pinned"
    );
}

// ===========================================================================
// §11.2.6 — key `patches.cameras`.
// ===========================================================================

/// §2.4.3's measured codes against an independent `f64` transcription, and the
/// luma-preserving property of the saturation leg.
///
/// *Fails:* the same transform applied in **display code space** differs on
/// every non-neutral patch, asserted.
#[test]
#[allow(clippy::too_many_lines)]
fn cc7_camera_transforms_are_applied_in_linear_light() {
    // The six reference steps and `deep_shadow` are §2.4.3's printed table.
    const PRINTED: [[[u8; 3]; 7]; 4] = [
        [
            [0, 0, 0],
            [11, 11, 11],
            [104, 104, 104],
            [180, 180, 180],
            [242, 242, 242],
            [255, 255, 255],
            [13, 13, 13],
        ],
        [
            [0, 0, 0],
            [8, 8, 7],
            [88, 85, 83],
            [154, 150, 146],
            [209, 204, 198],
            [220, 215, 209],
            [10, 9, 9],
        ],
        [
            [0, 0, 0],
            [4, 4, 4],
            [53, 56, 59],
            [99, 103, 108],
            [136, 142, 148],
            [144, 150, 156],
            [4, 5, 5],
        ],
        [
            [0, 0, 0],
            [2, 2, 2],
            [28, 34, 40],
            [60, 69, 79],
            [86, 97, 110],
            [91, 103, 117],
            [2, 2, 3],
        ],
    ];
    // chart indices of the six CC1 reference steps, then `deep_shadow`.
    const REFERENCE_STEPS: [usize; 7] = [0, 1, 5, 8, 10, 11, CC7_PATCH_COUNT - 1];
    /// A second, independent transcription of §2.4.3's pipeline.
    fn reference_camera_code(camera: Cc7Camera, rgb: [u8; 3]) -> [u8; 3] {
        let (gain, exposure, saturation) = match camera {
            Cc7Camera::A | Cc7Camera::LogLike => ([1.0, 1.0, 1.0], 0.0, 1.0),
            Cc7Camera::B => ([1.06, 1.0, 0.94], -500.0, 0.85),
            Cc7Camera::C1 => ([0.92, 1.0, 1.08], -1_500.0, 1.0),
            Cc7Camera::C2 => ([0.80, 1.0, 1.25], -2_500.0, 1.0),
        };
        let scale = 2.0_f64.powf(exposure / 1_000.0);
        let mut linear = [0.0_f64; 3];
        for channel in 0..3 {
            linear[channel] =
                reference_decode_bt709(f64::from(rgb[channel]) / 255.0) * gain[channel] * scale;
        }
        let luma = 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2];
        let mut out = [0_u8; 3];
        for channel in 0..3 {
            out[channel] = reference_code(reference_encode(
                luma + saturation * (linear[channel] - luma),
            ));
        }
        out
    }

    /// The wrong model: the same transform applied to the display **codes**.
    fn display_space_camera_code(camera: Cc7Camera, rgb: [u8; 3]) -> [u8; 3] {
        let (gain, exposure, saturation) = match camera {
            Cc7Camera::A | Cc7Camera::LogLike => ([1.0, 1.0, 1.0], 0.0, 1.0),
            Cc7Camera::B => ([1.06, 1.0, 0.94], -500.0, 0.85),
            Cc7Camera::C1 => ([0.92, 1.0, 1.08], -1_500.0, 1.0),
            Cc7Camera::C2 => ([0.80, 1.0, 1.25], -2_500.0, 1.0),
        };
        let scale = 2.0_f64.powf(exposure / 1_000.0);
        let mut encoded = [0.0_f64; 3];
        for channel in 0..3 {
            encoded[channel] = f64::from(rgb[channel]) / 255.0 * gain[channel] * scale;
        }
        let luma = 0.2126 * encoded[0] + 0.7152 * encoded[1] + 0.0722 * encoded[2];
        let mut out = [0_u8; 3];
        for channel in 0..3 {
            out[channel] = reference_code(luma + saturation * (encoded[channel] - luma));
        }
        out
    }

    let patches = CC7_CHART_PATCHES
        .iter()
        .chain(CC7_PRIMARY_PATCHES.iter())
        .chain(CC7_ROW_PATCHES.iter())
        .collect::<Vec<_>>();
    assert_eq!(patches.len(), CC7_PATCH_COUNT);

    for (camera_index, camera) in CC7_CAMERA_ORDER.into_iter().enumerate() {
        let table = cc7_camera_patch_codes(camera).expect("the four cameras have a code table");
        assert_eq!(table, &CC7_CAMERA_PATCH_CODES[camera_index]);
        let mut differing_in_display_space = 0;
        for (patch_index, patch) in patches.iter().enumerate() {
            let source = patch.display_code_cam_a;
            let expected = reference_camera_code(camera, source);
            assert_eq!(
                table[patch_index], expected,
                "camera {camera:?} patch {}: the pinned table must be the transcribed value",
                patch.name
            );
            assert_eq!(
                cc7_camera_code(camera, source),
                expected,
                "camera {camera:?} patch {}: the module's own function must agree",
                patch.name
            );
            if camera != Cc7Camera::A && display_space_camera_code(camera, source) != expected {
                differing_in_display_space += 1;
            }
        }
        if camera == Cc7Camera::A {
            // The identity leaves the base scene's own codes alone.
            for (patch_index, patch) in patches.iter().enumerate() {
                assert_eq!(
                    table[patch_index], patch.display_code_cam_a,
                    "{}",
                    patch.name
                );
            }
        } else {
            assert!(
                differing_in_display_space >= 20,
                "camera {camera:?}: the display-space model must differ on nearly every patch, not {differing_in_display_space}"
            );
        }
    }
    assert_eq!(cc7_camera_patch_codes(Cc7Camera::LogLike), None);

    // The saturation leg preserves BT.709 luma on the achromatic patches.
    for patch in &CC7_CHART_PATCHES {
        for camera in CC7_CAMERA_ORDER {
            let linear = reference_decode_bt709(f64::from(patch.display_code_cam_a[0]) / 255.0);
            let transform = kinewright_core::cc7_camera_transform(camera);
            let scale = 2.0_f64.powf(cc7_as_f64(transform.exposure_milli_stops) / 1_000.0);
            let gained: [f64; 3] = std::array::from_fn(|channel| {
                linear * (cc7_as_f64(transform.gain_millionths[channel]) / 1_000_000.0) * scale
            });
            let luma = 0.2126 * gained[0] + 0.7152 * gained[1] + 0.0722 * gained[2];
            let saturation = cc7_as_f64(transform.saturation_millionths) / 1_000_000.0;
            let mixed = [
                luma + saturation * (gained[0] - luma),
                luma + saturation * (gained[1] - luma),
                luma + saturation * (gained[2] - luma),
            ];
            let mixed_luma = 0.2126 * mixed[0] + 0.7152 * mixed[1] + 0.0722 * mixed[2];
            assert!(
                (mixed_luma - luma).abs() <= 1e-9,
                "camera {camera:?} on {}: the saturation mix must preserve BT.709 luma",
                patch.name
            );
        }
    }

    for (camera_index, camera) in CC7_CAMERA_ORDER.into_iter().enumerate() {
        let table = cc7_camera_patch_codes(camera).expect("a code table");
        for (column, patch_index) in REFERENCE_STEPS.into_iter().enumerate() {
            assert_eq!(
                table[patch_index], PRINTED[camera_index][column],
                "camera {camera:?} column {column} must be §2.4.3's printed value"
            );
        }
    }
}

// ===========================================================================
// §11.2.7 — key `tracking.path`.
// ===========================================================================

/// §2.3.6's four generator bounds over all 100 frames at amplitude `(100, 40)`.
///
/// *Fails:* an amplitude of 130 px leaves the raster, asserted.
#[test]
#[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
fn cc7_analytic_square_path_stays_in_frame_and_clears_the_patch_row() {
    /// The path at an arbitrary amplitude, transcribed here so the failing
    /// direction can move the amplitude without moving the module's constant.
    #[allow(clippy::cast_possible_truncation)]
    fn top_left_at(frame: i64, amplitude_x: f64, amplitude_y: f64) -> (i64, i64) {
        let angle = 2.0 * std::f64::consts::PI * cc7_as_f64(frame) / f64::from(CC7_TRACK_FRAMES);
        let sine = angle.sin();
        let round = |value: f64| {
            if value >= 0.0 {
                (value + 0.5).floor() as i64
            } else {
                -((-value + 0.5).floor() as i64)
            }
        };
        (
            round(cc7_as_f64(CC7_TRACK_CENTRE_X_PIXELS) + amplitude_x * sine),
            round(cc7_as_f64(CC7_TRACK_CENTRE_Y_PIXELS) + amplitude_y * sine),
        )
    }

    let size = CC7_TRACK_SQUARE_SIZE;
    let width = i64::from(CC7_SOURCE_WIDTH);
    let height = i64::from(CC7_SOURCE_HEIGHT);
    for frame in 0..i64::from(CC7_TRACK_FRAMES) {
        let (x, y) = cc7_analytic_square_top_left(frame);
        assert_eq!(
            (x, y),
            top_left_at(
                frame,
                cc7_as_f64(CC7_TRACK_AMPLITUDE_X_PIXELS),
                cc7_as_f64(CC7_TRACK_AMPLITUDE_Y_PIXELS)
            ),
            "frame {frame}: the module's path must be the transcribed one"
        );
        assert!(x >= 0, "frame {frame}: x {x} must not be negative");
        assert!(x + size <= width, "frame {frame}: x {x} must stay in frame");
        assert!(
            y >= size,
            "frame {frame}: y {y} must clear the static patch rows at y {}..{}",
            0,
            CC7_TRACK_STATIC_PATCH_BOTTOM
        );
        assert!(
            y + size <= height,
            "frame {frame}: y {y} must stay in frame"
        );
        assert!(
            y > CC7_TRACK_STATIC_PATCH_BOTTOM,
            "frame {frame}: the square must never cover the static patch row"
        );
    }

    // The analytic centre table, and §10.1's half-away-from-zero rounding,
    // which is load-bearing at frames 18, 28 and 32.
    for (index, frame) in cc7_tracking_sample_frames().into_iter().enumerate() {
        let (cx, cy) = cc7_analytic_square_centre_basis_points(frame);
        assert_eq!(
            [cx, cy],
            CC7_TRACK_ANALYTIC_CENTRES_BASIS_POINTS[index],
            "frame {frame}"
        );
    }
    assert_eq!(cc7_analytic_square_centre_basis_points(0), (5_000, 5_000));
    for frame in [18_i64, 28, 32] {
        let (x, _) = cc7_analytic_square_top_left(frame);
        let exact = cc7_as_f64(x + 12) * 10_000.0 / f64::from(CC7_SOURCE_WIDTH);
        assert!(
            (exact.fract() - 0.5).abs() <= 1e-9,
            "frame {frame} is one of the exact half-basis-point cases"
        );
        assert_eq!(
            cc7_analytic_square_centre_basis_points(frame).0,
            (exact + 0.5).floor() as i64,
            "frame {frame} must round half away from zero"
        );
    }

    // Failing direction: 130 px of amplitude leaves the raster — on the **y**
    // axis. §11.2.7's "an amplitude of 130 px leaves the raster" is true of y
    // (`78 + 130 + 24 = 232 > 180`) and false of x, where the square only
    // leaves once the amplitude passes 148 (`148 + 148 + 24 = 320`); both
    // directions are asserted so neither reading is left as a claim.
    let escapes_in_y = (0..i64::from(CC7_TRACK_FRAMES)).any(|frame| {
        let (_, y) = top_left_at(frame, cc7_as_f64(CC7_TRACK_AMPLITUDE_X_PIXELS), 130.0);
        y < size || y + size > height
    });
    assert!(
        escapes_in_y,
        "an amplitude of 130 px must leave the raster in y"
    );
    let holds_in_x_at_130 = (0..i64::from(CC7_TRACK_FRAMES)).all(|frame| {
        let (x, _) = top_left_at(frame, 130.0, cc7_as_f64(CC7_TRACK_AMPLITUDE_Y_PIXELS));
        x >= 0 && x + size <= width
    });
    assert!(
        holds_in_x_at_130,
        "130 px of x amplitude still fits; the x bound only fails past 148"
    );
    let escapes_in_x = (0..i64::from(CC7_TRACK_FRAMES)).any(|frame| {
        let (x, _) = top_left_at(frame, 149.0, cc7_as_f64(CC7_TRACK_AMPLITUDE_Y_PIXELS));
        x < 0 || x + size > width
    });
    assert!(
        escapes_in_x,
        "an x amplitude past 148 must leave the raster"
    );
}

// ===========================================================================
// §11.2.8 — key `tracking.sample_frames`.
// ===========================================================================

/// The transcribed `tracking_sample_frames` distribution reproduces the tool's
/// list, and the naive stepping does **not** — which is the recipe error A12
/// corrects, made checkable.
#[test]
#[allow(clippy::too_many_lines)]
fn cc7_tracking_sample_frames_are_the_closed_form_distribution() {
    let frames = cc7_tracking_sample_frames_for(
        CC7_TRACK_RANGE_START_LOCAL_FRAME,
        CC7_TRACK_RANGE_END_LOCAL_FRAME,
        CC7_TRACK_STEP_FRAMES,
    );
    assert_eq!(frames, vec![0, 4, 9, 14, 18, 23, 28, 32, 37, 42, 47]);
    assert_eq!(frames, cc7_tracking_sample_frames().to_vec());
    assert_eq!(frames.len(), 11);

    let f2 = cc7_tracking_sample_frames_for(
        CC7_TRACK_RANGE_START_LOCAL_FRAME,
        CC7_TRACK_RANGE_END_LOCAL_FRAME,
        CC7_TRACK_F2_STEP_FRAMES,
    );
    assert_eq!(f2, CC7_TRACK_F2_SAMPLE_FRAMES.to_vec());
    assert_eq!(f2, vec![0, 47]);

    // The naive `start + k · step` stepping is a different list.
    let naive = (0..CC7_TRACK_RANGE_END_LOCAL_FRAME)
        .map(|k| CC7_TRACK_RANGE_START_LOCAL_FRAME + k * CC7_TRACK_STEP_FRAMES)
        .take_while(|frame| *frame < CC7_TRACK_RANGE_END_LOCAL_FRAME)
        .collect::<Vec<_>>();
    assert_eq!(naive[..3], [0, 5, 10]);
    assert_ne!(
        naive, frames,
        "the tool distributes evenly; it does not step, and A12 corrects the recipe that assumed it did"
    );

    // Exactly one sampled frame is inside the occlusion, and it is 47.
    let occluded = frames
        .iter()
        .copied()
        .filter(|frame| !kinewright_core::cc7_scenarios::cc7_square_is_drawn(*frame))
        .collect::<Vec<_>>();
    assert_eq!(occluded, CC7_TRACK_EXPECTED_LOW_CONFIDENCE_FRAMES.to_vec());
    assert_eq!(
        frames.len() - occluded.len(),
        CC7_TRACK_SURVIVING_SAMPLE_COUNT
    );
    assert_eq!(
        CC7_TRACK_SURVIVING_SAMPLE_FRAMES.to_vec(),
        frames[..CC7_TRACK_SURVIVING_SAMPLE_COUNT].to_vec()
    );

    // Degenerate ranges do not panic and do not invent a sample.
    assert!(cc7_tracking_sample_frames_for(0, 0, 5).is_empty());
    assert_eq!(cc7_tracking_sample_frames_for(3, 4, 5), vec![3]);
}

// ===========================================================================
// §11.2.9 — key `thresholds.distinctness`.
// ===========================================================================

/// No CC7 constant equals a compositor, delivery, or tracking constant it
/// could be silently substituted for (CC7 §2.6).
///
/// `MATTE_TRACK_DEAD_ZONE_BASIS_POINTS` is deliberately **0**, so distinctness
/// from it is trivially true for every positive constant and CC7 does not
/// assert it (minor 4).
#[test]
#[allow(clippy::too_many_lines)]
fn cc7_budgets_are_distinct_from_every_neighbouring_constant() {
    // Distinctness is asserted **within a unit**. "Could be silently
    // substituted for" is the contract's own test (CC7 §2.6), and a pixel
    // count cannot be substituted for an 8-bit code tolerance: the two
    // coincidences below are recorded rather than asserted away.
    let cc7: [(&str, &str, i64); 12] = [
        (
            "CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE",
            "code",
            CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE,
        ),
        (
            "CC7_B1_RESIDUAL_SPREAD_MAX_CODE",
            "code",
            CC7_B1_RESIDUAL_SPREAD_MAX_CODE,
        ),
        (
            "CC7_MATCH_LUMA_MEAN_MAX_CODE_MILLIONTHS",
            "code_millionths",
            CC7_MATCH_LUMA_MEAN_MAX_CODE_MILLIONTHS,
        ),
        (
            "CC7_LOG_FIRST_PERCENTILE_MIN_CODE16",
            "code16",
            CC7_LOG_FIRST_PERCENTILE_MIN_CODE16,
        ),
        ("CC7_LOG_P99_MAX_CODE16", "code16", CC7_LOG_P99_MAX_CODE16),
        ("CC7_LOG_INVERSE_MAX_CODE", "code", CC7_LOG_INVERSE_MAX_CODE),
        (
            "CC7_FEATHER_PARTIAL_TOLERANCE_PIXELS",
            "pixels",
            CC7_FEATHER_PARTIAL_TOLERANCE_PIXELS,
        ),
        (
            "CC7_LOOK_DEEP_SHADOW_OUT_OF_GAMUT_PIXELS",
            "pixels",
            CC7_LOOK_DEEP_SHADOW_OUT_OF_GAMUT_PIXELS,
        ),
        (
            "CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS",
            "basis_points",
            CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS,
        ),
        (
            "CC7_TRACK_TOLERANCE_BASIS_POINTS",
            "basis_points",
            CC7_TRACK_TOLERANCE_BASIS_POINTS,
        ),
        (
            "CC7_DELIVERY_LEG_BUDGET_SECONDS_LINUX",
            "seconds",
            CC7_DELIVERY_LEG_BUDGET_SECONDS_LINUX,
        ),
        // §4(b)(3)'s excursion depth. It is reported rather than gated, but it
        // is asserted *exactly* by two fixtures, so a collision with a
        // neighbouring constant would be as silent a substitution as a
        // budget's.
        (
            "CC7_C2_MAX_OVER_EXCURSION_MILLIONTHS",
            "linear_millionths",
            CC7_C2_MAX_OVER_EXCURSION_MILLIONTHS,
        ),
    ];
    let neighbours: [(&str, &str, i64); 12] = [
        (
            "MONITOR_CPU_GPU_MAX",
            "code",
            NEIGHBOUR_MONITOR_CPU_GPU_MAX_CODE,
        ),
        (
            "DELIVERY_CODEC_MAX",
            "code",
            NEIGHBOUR_DELIVERY_CODEC_MAX_CODE,
        ),
        (
            "MATTE_TRACK_MAX_STEP_BASIS_POINTS",
            "basis_points",
            NEIGHBOUR_MATTE_TRACK_MAX_STEP_BASIS_POINTS,
        ),
        (
            "DEFAULT_MATTE_TRACK_MINIMUM_CONFIDENCE_BASIS_POINTS",
            "basis_points",
            NEIGHBOUR_DEFAULT_MATTE_TRACK_MINIMUM_CONFIDENCE_BASIS_POINTS,
        ),
        (
            "DELIVERY_LUMA_MAX_CODE_8BIT",
            "code",
            i64::from(kinewright_core::DELIVERY_LUMA_MAX_CODE_8BIT),
        ),
        (
            "DELIVERY_LUMA_MAX_CODE_10BIT",
            "code",
            i64::from(kinewright_core::DELIVERY_LUMA_MAX_CODE_10BIT),
        ),
        (
            "DELIVERY_LUMA_P99_CODE_8BIT_MILLIONTHS",
            "code_millionths",
            kinewright_core::DELIVERY_LUMA_P99_CODE_8BIT_MILLIONTHS,
        ),
        (
            "DELIVERY_LUMA_P99_CODE_10BIT_MILLIONTHS",
            "code_millionths",
            kinewright_core::DELIVERY_LUMA_P99_CODE_10BIT_MILLIONTHS,
        ),
        (
            "DELIVERY_LUMA_MEAN_CODE_8BIT_MILLIONTHS",
            "code_millionths",
            kinewright_core::DELIVERY_LUMA_MEAN_CODE_8BIT_MILLIONTHS,
        ),
        (
            "DELIVERY_LUMA_MEAN_CODE_10BIT_MILLIONTHS",
            "code_millionths",
            kinewright_core::DELIVERY_LUMA_MEAN_CODE_10BIT_MILLIONTHS,
        ),
        (
            "DELIVERY_RGB_MEAN_CODE_8BIT_MILLIONTHS",
            "code_millionths",
            kinewright_core::DELIVERY_RGB_MEAN_CODE_8BIT_MILLIONTHS,
        ),
        (
            "DELIVERY_RGB_MEAN_CODE_10BIT_MILLIONTHS",
            "code_millionths",
            kinewright_core::DELIVERY_RGB_MEAN_CODE_10BIT_MILLIONTHS,
        ),
    ];
    let mut compared = 0;
    for (cc7_name, cc7_unit, cc7_value) in cc7 {
        for (neighbour_name, neighbour_unit, neighbour_value) in neighbours {
            if cc7_unit != neighbour_unit {
                continue;
            }
            compared += 1;
            assert_ne!(
                cc7_value, neighbour_value,
                "{cc7_name} must not equal {neighbour_name}: a constant that equals its neighbour in the same unit can be silently substituted for it"
            );
        }
    }
    assert!(
        compared >= 22,
        "the same-unit comparison must not be vacuous: only {compared} pairs were checked"
    );

    // The one cross-unit coincidence, recorded rather than asserted away:
    // `CC7_FEATHER_PARTIAL_TOLERANCE_PIXELS` is 4 **pixels** and
    // `DELIVERY_CODEC_MAX` is 4 **8-bit codes**. Neither can stand in for the
    // other, and the equality is stated here so it cannot be discovered later
    // as a surprise.
    assert_eq!(CC7_FEATHER_PARTIAL_TOLERANCE_PIXELS, 4);
    assert_eq!(NEIGHBOUR_DELIVERY_CODEC_MAX_CODE, 4);

    // Asserted by name, because probe-2 measured that the tracker default
    // drops nothing at all on this recipe.
    assert_ne!(
        CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS,
        NEIGHBOUR_DEFAULT_MATTE_TRACK_MINIMUM_CONFIDENCE_BASIS_POINTS
    );
    assert_ne!(
        CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS,
        NEIGHBOUR_MATTE_TRACK_MAX_STEP_BASIS_POINTS
    );
    // C-E7: the (b1) budget is a SECOND code-unit budget, one code above the
    // (a) one. Asserted by name in both directions, because the whole reason
    // it exists is that 5 cannot be widened to 6 — 6 is what unmatched cam B
    // measures — and because a (b1) budget equal to the (a) one would be a
    // silent substitution of the very constant it was split away from.
    assert_ne!(
        CC7_B1_RESIDUAL_SPREAD_MAX_CODE, CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE,
        "the (b1) residual budget must not collapse back into the (a) match budget"
    );
    assert_eq!(
        CC7_B1_RESIDUAL_SPREAD_MAX_CODE,
        CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE + 1,
        "(b1) is exactly one code above (a): a harder recovery, not a free hand"
    );
    assert_ne!(
        CC7_B1_RESIDUAL_SPREAD_MAX_CODE, CC7_LOG_INVERSE_MAX_CODE,
        "the two code-unit budgets must stay distinct"
    );
    assert_ne!(
        CC7_B1_RESIDUAL_SPREAD_MAX_CODE, NEIGHBOUR_DELIVERY_CODEC_MAX_CODE,
        "the (b1) budget must not equal DELIVERY_CODEC_MAX"
    );
    assert_ne!(
        CC7_B1_RESIDUAL_SPREAD_MAX_CODE, NEIGHBOUR_MONITOR_CPU_GPU_MAX_CODE,
        "the (b1) budget must not equal MONITOR_CPU_GPU_MAX"
    );
    // A15's reason the (a) budget stays at 5, restated where the split is made:
    // the unmatched cam B measurement is exactly the (b1) budget, so the two
    // rows could not have shared one constant.
    assert_eq!(
        CC7_MEASURED_UNMATCHED_B_SPREAD_CODE, CC7_B1_RESIDUAL_SPREAD_MAX_CODE,
        "a shared budget of 6 would have admitted (a)'s own failing direction"
    );
    // The excursion depth is the one **linear**-millionths quantity in the
    // list, and the two constants it could be mistaken for are the look's
    // blue zero crossing (the other linear-millionths figure in
    // `cc7_scenarios`) and the chart luma mean budget, which is code
    // millionths and not linear at all. Both are asserted by name, because a
    // reader who drops "linear" from the unit is exactly the reader who would
    // substitute one for the other.
    assert_ne!(
        CC7_C2_MAX_OVER_EXCURSION_MILLIONTHS, CC7_LOOK_BLUE_ZERO_CROSSING_LINEAR_MILLIONTHS,
        "the two linear-millionths constants must stay distinct"
    );
    assert_ne!(
        CC7_C2_MAX_OVER_EXCURSION_MILLIONTHS, CC7_MATCH_LUMA_MEAN_MAX_CODE_MILLIONTHS,
        "a linear excursion depth must not equal a code-millionths budget"
    );

    // The dead zone is zero, so CC7 states rather than asserts distinctness.
    assert_eq!(NEIGHBOUR_MATTE_TRACK_DEAD_ZONE_BASIS_POINTS, 0);
    assert!(cc7.iter().all(|(_, _, value)| *value != 0));

    // One constant per term, and never a per-OS constant (R5).
    let mut names = cc7.iter().map(|(name, _, _)| *name).collect::<Vec<_>>();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), cc7.len());
    assert!(
        names
            .iter()
            .all(|name| !name.contains("WINDOWS") && !name.contains("MACOS")),
        "there is one constant per term and never a per-OS constant"
    );
    assert_eq!(CC7_DELIVERY_ALLOWED_INFO_CODES.len(), 1);
    assert_eq!(
        CC7_DELIVERY_ALLOWED_INFO_CODES[0],
        "delivery_tag_not_representable"
    );
}

// ===========================================================================
// §11.2.10 — key `budgets`.
// ===========================================================================

/// Every §4.1 row: `budget / measured ≥ 2`, and every `measured` strictly
/// inside its budget.
///
/// The two-sided track confidence floor and the containment half-extents are
/// checked by their own rule (§4.1 notes 3 and 5): the ≥ 2× rule does not
/// apply to a value pinned between two populations, and a term measured at
/// zero records its failing-direction fixture rather than a fabricated ratio.
#[test]
#[allow(clippy::too_many_lines)]
fn cc7_every_budget_carries_the_declared_margin() {
    fn check(row: Cc7Budget) {
        match row.kind {
            Cc7BudgetKind::RatioAtLeastTwo => {
                let measured = row.measured.abs();
                assert!(
                    measured > 0,
                    "{}: a ratio row measured zero; it belongs in MeasuredZero",
                    row.term
                );
                assert!(
                    measured < row.budget,
                    "{} ({}): measured {measured} must be strictly inside the budget {}",
                    row.term,
                    row.constant,
                    row.budget
                );
                assert!(
                    row.budget >= 2 * measured,
                    "{} ({}): budget {} over measured {measured} is below the 2x bar",
                    row.term,
                    row.constant,
                    row.budget
                );
            }
            Cc7BudgetKind::Exact => assert_eq!(
                row.measured, row.budget,
                "{}: an exact term must measure its budget",
                row.term
            ),
            Cc7BudgetKind::MeasuredZero => assert_eq!(
                row.measured, 0,
                "{}: a MeasuredZero row must measure exactly zero, so its bound is its failing-direction fixture",
                row.term
            ),
            Cc7BudgetKind::Floor => assert!(
                row.measured > row.budget,
                "{}: measured {} must clear the floor {}",
                row.term,
                row.measured,
                row.budget
            ),
            Cc7BudgetKind::Ceiling => assert!(
                row.measured < row.budget,
                "{}: measured {} must stay under the ceiling {}",
                row.term,
                row.measured,
                row.budget
            ),
            // R4-M2: a budget CC7 does not own and may not move, whose
            // amended-scene measurement does not clear the 2x bar. The margin
            // is recorded, not asserted — but the *classification* is
            // asserted in both directions, so this cannot become a quiet
            // waiver for a row that would have passed the rule.
            Cc7BudgetKind::RecordedMargin => {
                let measured = row.measured.abs();
                assert!(
                    measured > 0,
                    "{}: a recorded-margin row measured zero; it belongs in MeasuredZero",
                    row.term
                );
                assert!(
                    measured < row.budget,
                    "{} ({}): measured {measured} must be strictly inside the budget {}",
                    row.term,
                    row.constant,
                    row.budget
                );
                assert!(
                    row.budget < 2 * measured,
                    "{} ({}): budget {} over measured {measured} clears the 2x bar, so it belongs in RatioAtLeastTwo rather than recording its margin",
                    row.term,
                    row.constant,
                    row.budget
                );
            }
        }
    }

    for row in CC7_BUDGETS {
        check(row);
    }
    check(CC7_TRACK_OBSERVATION_BUDGET_ROW);
    assert!(
        CC7_DELIVERY_TEN_PSNR_MEASURED_HUNDREDTHS
            > kinewright_core::DELIVERY_PSNR_FLOOR_DB_HUNDREDTHS_10BIT.into()
    );

    // The worst CC6 delivery margin is the 8-bit luma mean, and on the
    // amended scene it is **1.06x**, not the 2.16x probe-1 measured on the
    // pre-A1 scene: scenario (e) — the warm look — measures 377 538 against
    // CC6's 400 000 (Implementer C erratum C-E8, R4-M2). CC7 never
    // re-baselines a constant it does not own, so the row records its margin
    // instead of asserting a ratio it does not have.
    let eight_bit_luma_mean = CC7_BUDGETS
        .iter()
        .find(|row| row.constant == "DELIVERY_LUMA_MEAN_CODE_8BIT_MILLIONTHS")
        .expect("the 8-bit luma mean row");
    assert_eq!(
        eight_bit_luma_mean.kind,
        Cc7BudgetKind::RecordedMargin,
        "the worst delivery row records its margin"
    );
    assert_eq!(eight_bit_luma_mean.measured, CC7_MEASURED_DELIVERY_EIGHT[2]);
    assert!(
        eight_bit_luma_mean.budget < 2 * eight_bit_luma_mean.measured,
        "if this row ever clears 2x again it must go back to RatioAtLeastTwo"
    );
    // It is the *only* row that does not clear the bar: every other delivery
    // term is still a ratio, floor or measured-zero row, and the loop above
    // asserts each by its own rule.
    assert_eq!(
        CC7_BUDGETS
            .iter()
            .filter(|row| row.kind == Cc7BudgetKind::RecordedMargin)
            .count(),
        1,
        "exactly one CC7 budget row falls short of the 2x bar"
    );
    // The delivery measurements are the worst of the six scenarios on the
    // amended scene; `assert_cc7_delivery_lane` asserts every lane against the
    // manifest's per-scenario triples, which is what keeps them true.
    assert_eq!(
        CC7_DELIVERY_TEN_PSNR_MEASURED_HUNDREDTHS, CC7_MEASURED_DELIVERY_TEN[4],
        "the 10-bit PSNR row is the fifth term of the same measured table"
    );

    // §4.1 note 5: the confidence floor is a two-sided bound, not a ratio.
    const {
        assert!(
            CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS - CC7_TRACK_OCCLUDED_CONFIDENCE_MAX_REPORTED
                > CC7_TRACK_CONFIDENCE_SEPARATION_MIN_BASIS_POINTS,
            "the floor must sit more than 1 000 bp above the measured occluded maximum"
        );
    }
    const {
        assert!(
            CC7_TRACK_CLEAN_CONFIDENCE_MIN_REPORTED - CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS
                > CC7_TRACK_CONFIDENCE_SEPARATION_MIN_BASIS_POINTS,
            "the floor must sit more than 1 000 bp below the measured clean minimum"
        );
    }
    assert_eq!(
        CC7_TRACK_CLEAN_CONFIDENCE_MIN_REPORTED - CC7_TRACK_OCCLUDED_CONFIDENCE_MAX_REPORTED,
        2_329,
        "the separation the floor is pinned inside"
    );

    // §4.1's containment row: the 1.5x window clears the measured requirement,
    // and the seeded 1.0x window does not.
    let required_x = CC7_TRACK_CONTAINMENT_REQUIRED_HALF_WIDTH_PIXELS_REPORTED;
    let required_y = CC7_TRACK_CONTAINMENT_REQUIRED_HALF_HEIGHT_PIXELS_REPORTED;
    // The 1.5x window's own half-extents, derived from the basis-point
    // constants rather than restated as `18 * 100`: the x window resolves to
    // 18.016 px, not 18.000, so the two axes do not share a literal.
    let window_x = cc7_round_half_away_from_zero(
        f64::from(CC7_SOURCE_WIDTH) * cc7_as_f64(CC7_TRACK_WINDOW_HALF_WIDTH_BASIS_POINTS) / 100.0,
    );
    let window_y = cc7_round_half_away_from_zero(
        f64::from(CC7_SOURCE_HEIGHT) * cc7_as_f64(CC7_TRACK_WINDOW_HALF_HEIGHT_BASIS_POINTS)
            / 100.0,
    );
    // The reported margins are C-E6's float measurements (3.232 / 5.118 px)
    // rounded to hundredths, so they agree with the integer difference to
    // within the same two hundredths the media containment gate allows
    // (`cc7_tracked_window_contains_the_square_at_every_sampled_frame`).
    let margin_x = CC7_TRACK_CONTAINMENT_WORST_MARGIN_X_PIXELS_HUNDREDTHS;
    let margin_y = CC7_TRACK_CONTAINMENT_WORST_MARGIN_Y_PIXELS_HUNDREDTHS;
    assert!(
        (window_x - required_x - margin_x).abs() <= 2,
        "the reported x margin {margin_x} is not the 1.5x window {window_x} less the required \
         {required_x}"
    );
    assert!(
        (window_y - required_y - margin_y).abs() <= 2,
        "the reported y margin {margin_y} is not the 1.5x window {window_y} less the required \
         {required_y}"
    );
    assert!(
        required_x > 12 * 100,
        "the seeded 12 px window is short in x"
    );
    assert_eq!(
        CC7_TRACK_WINDOW_HALF_WIDTH_BASIS_POINTS,
        cc7_round_half_away_from_zero(18.0 * 10_000.0 / f64::from(CC7_SOURCE_WIDTH))
    );
    assert_eq!(
        CC7_TRACK_WINDOW_HALF_HEIGHT_BASIS_POINTS,
        cc7_round_half_away_from_zero(18.0 * 10_000.0 / f64::from(CC7_SOURCE_HEIGHT))
    );

    // §4(d)(4)'s discrete pixel-centre model, and the wrong continuous model.
    let [full, covered, partial] = CC7_D2_FEATHER_COUNTS_PIXELS;
    assert_eq!(covered - full, partial);
    assert_eq!([full, covered, partial], [140, 252, 112]);
    let centre_x =
        cc7_as_f64(CC7_D2_WINDOW_CENTRE_BASIS_POINTS[0]) * f64::from(CC7_SOURCE_WIDTH) / 10_000.0;
    let centre_y =
        cc7_as_f64(CC7_D2_WINDOW_CENTRE_BASIS_POINTS[1]) * f64::from(CC7_SOURCE_HEIGHT) / 10_000.0;
    let half_width = cc7_as_f64(CC7_D2_WINDOW_HALF_EXTENTS_BASIS_POINTS[0])
        * f64::from(CC7_SOURCE_WIDTH)
        / 10_000.0;
    let half_height = cc7_as_f64(CC7_D2_WINDOW_HALF_EXTENTS_BASIS_POINTS[1])
        * f64::from(CC7_SOURCE_HEIGHT)
        / 10_000.0;
    let feather = cc7_as_f64(CC7_FEATHER_BASIS_POINTS) / 10_000.0;
    let count = |scale: f64, strict: bool| {
        let mut inside = 0_i64;
        for y in 0..CC7_SOURCE_HEIGHT {
            for x in 0..CC7_SOURCE_WIDTH {
                let dx = (f64::from(x) + 0.5 - centre_x).abs();
                let dy = (f64::from(y) + 0.5 - centre_y).abs();
                let hit = if strict {
                    dx < scale * half_width && dy < scale * half_height
                } else {
                    dx <= scale * half_width && dy <= scale * half_height
                };
                if hit {
                    inside += 1;
                }
            }
        }
        inside
    };
    let inner = count(1.0 - feather, false);
    let outer = count(1.0 + feather, true);
    assert_eq!(inner, full, "the discrete inner count is 10 x 14");
    assert_eq!(outer, covered, "the discrete outer count is 14 x 18");
    assert_eq!(outer - inner, partial);
    // The continuous-area formula is the wrong model, by 35 pixels (31 %).
    // §4(d)(4) states it at the window's **nominal** 6 x 8 px half-extents,
    // where `4 * 6 * 8 * ((1.1)^2 - (0.9)^2) = 76.8`; at the
    // basis-point-quantized 5.984 x 7.992 it is 76.5. Both are asserted not to
    // match, so neither reading of the wrong model can be reintroduced.
    let span = (1.0 + feather).powi(2) - (1.0 - feather).powi(2);
    let nominal = 4.0 * 6.0 * 8.0 * span;
    assert_eq!(
        cc7_round_half_away_from_zero(nominal * 10.0),
        CC7_D2_CONTINUOUS_AREA_WRONG_MODEL_PIXELS_TENTHS
    );
    let quantized = 4.0 * half_width * half_height * span;
    assert_eq!(cc7_round_half_away_from_zero(quantized * 10.0), 765);
    for wrong in [nominal, quantized] {
        assert!(
            (cc7_as_f64(partial) - wrong).abs() > cc7_as_f64(CC7_FEATHER_PARTIAL_TOLERANCE_PIXELS),
            "the continuous-area formula must not match, or the wrong model could be reintroduced"
        );
    }
    assert_eq!(
        partial - cc7_round_half_away_from_zero(nominal),
        35,
        "the wrong model is wrong by 35 pixels on this window"
    );
    assert_eq!(CC7_MATTE_OUTSIDE_CHANGED_PIXELS_MAX, 0);
}

// ===========================================================================
// §11.2.11 — key `canonical_documents`.
// ===========================================================================

/// One managed video asset the CC7 scenario documents reference.
fn cc7_asset() -> MediaAsset {
    MediaAsset {
        id: AssetId(1),
        path: Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
        name: "cc7".to_owned(),
        duration: TimeCode(i64::from(CC7_TRACK_FRAMES)),
        fps: Rational::new(25, 1).unwrap(),
        kind: MediaKind::Video,
        resolution: Some((CC7_SOURCE_WIDTH, CC7_SOURCE_HEIGHT)),
        source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
        color_description: ColorContext::sdr_rec709().delivery,
    }
}

fn cc7_clip(id: u64, timeline_start: i64, frames: i64) -> Clip {
    Clip {
        id: ClipId(id),
        asset: AssetId(1),
        source_range: TimeCode(0)..TimeCode(frames),
        content: ClipContent::Media,
        timeline_start: TimeCode(timeline_start),
        effects: Vec::new(),
        transition_in: None,
        link: None,
        audio_gain_tenth_db: 0,
        audio_fade_in_frames: TimeCode::ZERO,
        audio_fade_out_frames: TimeCode::ZERO,
        speed_percent: 100,
    }
}

/// The initial document a scenario's canonical batch is applied to.
fn cc7_initial_document(scenario: Cc7Scenario) -> Document {
    let spec = cc7_spec(scenario);
    let per_clip = i64::from(if scenario == Cc7Scenario::TrackedSecondary {
        CC7_TRACK_FRAMES
    } else {
        CC7_SOURCE_FRAMES
    });
    let clips = spec
        .clips
        .iter()
        .enumerate()
        .map(|(index, clip)| {
            cc7_clip(
                clip.clip_id,
                i64::try_from(index).unwrap_or(0) * per_clip,
                per_clip,
            )
        })
        .collect::<Vec<_>>();
    Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips,
        }],
        media_pool: vec![cc7_asset()],
        // The project runs at the source's own rate, so a clip's timeline
        // duration is its source range and the two (a)/(b) clips abut rather
        // than overlap.
        fps: Rational::new(CC7_SOURCE_FPS, 1).expect("25 fps"),
        resolution: (CC7_SOURCE_WIDTH, CC7_SOURCE_HEIGHT),
        duration: TimeCode(i64::from(spec.frames)),
        ..Document::default()
    }
}

/// A LUT asset record shaped like the one `import_lut_asset` registers. The
/// hash is a placeholder: §11.2.11 proves core **accepts the batch in order**,
/// and no measurement is compared against this record.
fn cc7_placeholder_lut_asset(size: u32) -> LutAsset {
    LutAsset {
        id: kinewright_core::LutAssetId(1),
        sha256: "0".repeat(64),
        title: "CC7 log inverse".to_owned(),
        kind: LutAssetKind::Cube3d,
        size,
        byte_len: 7_414_990,
        domain_min_millionths: [0, 0, 0],
        domain_max_millionths: [1_000_000, 1_000_000, 1_000_000],
        source: LutAssetSource::Builtin {
            name: "cc7-log-inverse".to_owned(),
        },
    }
}

/// Each scenario's batch through `apply_batch` on its initial document, in
/// order, with every stored parameter a real descriptor control that is not at
/// its neutral.
///
/// *Fails:* a reordered batch is rejected with the typed core error —
/// asserted for (c), whose `InsertEffect` names an asset `AddLutAsset` has not
/// registered yet, and for (f), whose `SetEffectKeyframes` names an effect
/// `InsertEffect` has not created yet.
#[test]
#[allow(clippy::too_many_lines)]
fn cc7_canonical_operations_are_accepted_by_core_in_order() {
    assert_eq!(CC7_SCENARIOS.len(), 6);
    assert_eq!(CC7_SCENARIO_SPECS.len(), CC7_SCENARIOS.len());
    for (index, scenario) in CC7_SCENARIOS.into_iter().enumerate() {
        assert_eq!(cc7_spec(scenario).scenario, scenario);
        assert_eq!(CC7_SCENARIO_SPECS[index].scenario, scenario);
    }

    let apply = |scenario: Cc7Scenario, operations: &[Operation]| -> Document {
        let mut document = cc7_initial_document(scenario);
        apply_batch(&mut document, operations)
            .unwrap_or_else(|error| panic!("{scenario:?}: core must accept the batch: {error}"));
        document
    };

    for scenario in CC7_SCENARIOS {
        let spec = cc7_spec(scenario);
        let operations = match scenario {
            Cc7Scenario::LogLike | Cc7Scenario::CreativeLook => {
                cc7_lut_backed_canonical_operations(
                    scenario,
                    cc7_placeholder_lut_asset(CC7_LOG_CUBE_SIZE),
                )
            }
            _ => cc7_canonical_operations(scenario),
        };
        let document = apply(scenario, &operations);

        // The node landed on the clip the scenario names, and nowhere else.
        let target = kinewright_core::cc7_scenarios::cc7_target_clip(scenario);
        for track in &document.tracks {
            for clip in &track.clips {
                let expected = usize::from(clip.id == target);
                assert_eq!(
                    clip.effects.len(),
                    expected,
                    "{scenario:?}: clip {} must carry {expected} effect(s)",
                    clip.id.0
                );
            }
        }
        let clip = document.clip(target).expect("the target clip survives");
        let effect = &clip.effects[0];
        let node = spec.canonical_operations[0];
        assert_eq!(effect.name, node.effect_name);

        // Every stored parameter is a real descriptor control, off its neutral,
        // and the stored set is exactly the canonical one.
        let descriptor =
            effect_descriptor(node.effect_name).expect("every canonical effect is a descriptor");
        let mut expected = BTreeMap::new();
        for (name, value) in node.parameters {
            let parameter = descriptor.parameter(name).unwrap_or_else(|| {
                panic!("{scenario:?}: {name} is not a {} control", node.effect_name)
            });
            assert_ne!(
                parameter.neutral, *value,
                "{scenario:?}: {name} is stored at its descriptor neutral, which a commit never does"
            );
            assert!(
                (parameter.min..=parameter.max).contains(value),
                "{scenario:?}: {name} = {value} is outside {}..={}",
                parameter.min,
                parameter.max
            );
            expected.insert((*name).to_owned(), ParamValue::Integer(*value));
        }
        assert_eq!(
            effect.parameters, expected,
            "{scenario:?}: the committed parameters must be the canonical set exactly"
        );

        // The neutral controls the planner did not move are absent.
        for parameter in descriptor.parameters {
            if !expected.contains_key(parameter.name) {
                assert!(
                    !effect.parameters.contains_key(parameter.name),
                    "{scenario:?}: {} must be absent, not stored at its neutral",
                    parameter.name
                );
            }
        }

        // Only (f) carries keyframes, and it carries exactly two curves.
        if scenario == Cc7Scenario::TrackedSecondary {
            assert_eq!(effect.keyframes.len(), 2);
            for (axis, name) in kinewright_core::cc7_scenarios::CC7_F_KEYFRAMED_PARAMETERS
                .into_iter()
                .enumerate()
            {
                let curve = effect
                    .keyframes
                    .get(name)
                    .unwrap_or_else(|| panic!("the (f) commit writes a curve for {name}"));
                let values = cc7_track_keyframe_centres(axis);
                assert_eq!(curve.keyframes.len(), CC7_TRACK_SURVIVING_SAMPLE_COUNT);
                for (index, keyframe) in curve.keyframes.iter().enumerate() {
                    assert_eq!(keyframe.at.0, CC7_TRACK_SURVIVING_SAMPLE_FRAMES[index]);
                    assert_eq!(keyframe.value, values[index]);
                }
            }
        } else {
            assert!(
                effect.keyframes.is_empty(),
                "{scenario:?} keyframes nothing"
            );
        }

        // Scenario (b) commits (b2)'s document; (b1) is a second document.
        assert_eq!(spec.canonical_operations.len(), 1);
    }

    // (b1) and (d2) are second documents of their scenarios, not seventh and
    // eighth scenarios.
    let b1 = apply(Cc7Scenario::WhiteBalance, &cc7_b1_canonical_operations());
    let b1_effect = &b1.clip(ClipId(2)).expect("clip 2").effects[0];
    assert_eq!(
        b1_effect.parameters.get("exposure_milli_stops"),
        Some(&ParamValue::Integer(
            CC7_MATCH_PROPOSAL_C1.exposure_milli_stops
        )),
        "the (b1) exposure is the planner's proposal, never a literal (§2.1)"
    );
    // D-E5: the tint delta rounds to zero on the amended scene, so the planner
    // omits the control and the canonical (b1) node stores two parameters.
    assert_eq!(b1_effect.parameters.get("tint_percent"), None);
    let b2 = apply(
        Cc7Scenario::WhiteBalance,
        &cc7_canonical_operations(Cc7Scenario::WhiteBalance),
    );
    assert_ne!(
        b1.clip(ClipId(2)).expect("clip 2").effects,
        b2.clip(ClipId(2)).expect("clip 2").effects,
        "(b1) and (b2) are two documents"
    );

    let d2 = apply(Cc7Scenario::ProductAndSkin, &cc7_d2_canonical_operations());
    let window_only = &d2.clip(ClipId(1)).expect("clip 1").effects[0];
    assert!(
        !window_only
            .parameters
            .contains_key("matte_qualifier_enabled"),
        "(d2) is window-only: a node carrying both legs would measure 192 / 140 / 52"
    );
    assert!(
        window_only
            .parameters
            .contains_key("matte_window0_feather_basis_points")
    );
    let d = apply(
        Cc7Scenario::ProductAndSkin,
        &cc7_canonical_operations(Cc7Scenario::ProductAndSkin),
    );
    let qualifier_only = &d.clip(ClipId(1)).expect("clip 1").effects[0];
    assert!(
        qualifier_only
            .parameters
            .contains_key("matte_qualifier_enabled"),
        "(d) is qualifier-only"
    );
    assert!(
        !qualifier_only
            .parameters
            .keys()
            .any(|name| name.starts_with("matte_window")),
        "(d) carries no window"
    );

    // Failing direction 1: (c) reordered puts the node before its asset.
    let mut reordered = cc7_lut_backed_canonical_operations(
        Cc7Scenario::LogLike,
        cc7_placeholder_lut_asset(CC7_LOG_CUBE_SIZE),
    );
    reordered.reverse();
    let mut document = cc7_initial_document(Cc7Scenario::LogLike);
    let error = apply_batch(&mut document, &reordered)
        .expect_err("a node that names an unregistered asset must be rejected");
    assert!(
        format!("{error}").contains("LUT asset") || format!("{error:?}").contains("LutAsset"),
        "the rejection must name the asset: {error}"
    );

    // Failing direction 2: (f) reordered writes a curve before the node exists.
    let mut reordered = cc7_canonical_operations(Cc7Scenario::TrackedSecondary);
    reordered.reverse();
    let mut document = cc7_initial_document(Cc7Scenario::TrackedSecondary);
    apply_batch(&mut document, &reordered)
        .expect_err("a curve written before its node must be rejected");

    // Person-path bookkeeping: five of six, and (f) says why in the code's own
    // words rather than in a comment.
    let expressible = CC7_SCENARIOS
        .into_iter()
        .filter(|scenario| cc7_spec(*scenario).person_path == Cc7PersonPath::Expressible)
        .count();
    assert_eq!(expressible, 5);
    match cc7_spec(Cc7Scenario::TrackedSecondary).person_path {
        Cc7PersonPath::NotApplicable { reason } => {
            assert!(reason.contains("track_matte_window"));
        }
        Cc7PersonPath::Expressible => panic!("scenario (f) is person-N/A by construction"),
    }
    // Only scenario (c) is objective-only.
    let questioned = CC7_SCENARIOS
        .into_iter()
        .filter(|scenario| cc7_spec(*scenario).human_question.is_some())
        .count();
    assert_eq!(questioned, 5);
    assert!(cc7_spec(Cc7Scenario::LogLike).human_question.is_none());
    let ids = CC7_SCENARIOS
        .into_iter()
        .map(|scenario| cc7_spec(scenario).id)
        .collect::<Vec<_>>();
    assert_eq!(ids, ["a", "b", "c", "d", "e", "f"]);
}

// ===========================================================================
// The keyframe smoother's transcription, held honest against its owner.
// ===========================================================================

/// `cc7_stabilized_centres` is an independent transcription of
/// `stabilize_tracked_centres_basis_points` (`multicam.rs:1171-1236`), which
/// `track_matte_window` reaches at `server.rs:4611-4620`. A transcription
/// nobody cross-checks is a second definition.
#[test]
#[allow(clippy::too_many_lines)]
fn cc7_the_keyframe_smoother_transcription_matches_core() {
    for axis in 0..2 {
        let raw = CC7_TRACK_OBSERVED_CENTRES_BASIS_POINTS
            .iter()
            .take(CC7_TRACK_SURVIVING_SAMPLE_COUNT)
            .map(|centre| centre[axis])
            .collect::<Vec<_>>();
        let owner = stabilize_tracked_centres_basis_points(
            &raw,
            kinewright_core::MATTE_WINDOW_CENTER_MIN_BASIS_POINTS,
            kinewright_core::MATTE_WINDOW_CENTER_MAX_BASIS_POINTS,
            0,
            CC7_TRACK_MAX_STEP_BASIS_POINTS_RESTATED,
        );
        assert_eq!(
            cc7_stabilized_centres(&raw, CC7_TRACK_MAX_STEP_BASIS_POINTS_RESTATED),
            owner,
            "axis {axis}: the transcription must agree with its owner"
        );
        assert_eq!(cc7_track_keyframe_centres(axis), owner);
    }

    // The published `known_systematic_lag`: the median filter replaces the
    // final value, so frame 42 is written as the previous sample's `7 246`
    // rather than its own `6 465`, which is 746 bp from the **analytic**
    // centre `6 500`. The lag is an error against the subject, not against the
    // observation, which is why a gate written on `curves` would fail on the
    // smoother (A17).
    let x = cc7_track_keyframe_centres(0);
    let last = CC7_TRACK_SURVIVING_SAMPLE_COUNT - 1;
    assert_eq!(
        x[last],
        CC7_TRACK_OBSERVED_CENTRES_BASIS_POINTS[last - 1][0]
    );
    assert_eq!(
        x[last] - CC7_TRACK_ANALYTIC_CENTRES_BASIS_POINTS[last][0],
        kinewright_core::cc7_scenarios::CC7_TRACK_FINAL_KEYFRAME_LAG_BASIS_POINTS_REPORTED
    );
    // Every surviving raw observation is inside the tolerance, which the
    // curve's final keyframe is not.
    for index in 0..CC7_TRACK_SURVIVING_SAMPLE_COUNT {
        for axis in 0..2 {
            let error = CC7_TRACK_OBSERVED_CENTRES_BASIS_POINTS[index][axis]
                - CC7_TRACK_ANALYTIC_CENTRES_BASIS_POINTS[index][axis];
            assert!(
                error.abs() <= CC7_TRACK_TOLERANCE_BASIS_POINTS,
                "sample {index} axis {axis}: raw error {error} must stay inside the tolerance"
            );
            assert!(error.abs() <= CC7_TRACK_WORST_RAW_OBSERVATION_ERROR_BASIS_POINTS_REPORTED);
        }
    }
    const {
        assert!(
            kinewright_core::cc7_scenarios::CC7_TRACK_FINAL_KEYFRAME_LAG_BASIS_POINTS_REPORTED
                > CC7_TRACK_TOLERANCE_BASIS_POINTS,
            "a gate written against the smoothed curve would fail on the smoother"
        );
    }

    // `MATTE_TRACK_MAX_STEP_BASIS_POINTS = 800` is reached exactly once on
    // this path by an **observed** step: the `4 -> 9` segment, raw dx 898. The
    // clamp then self-corrects over the following samples at a net cost of no
    // more than 98 bp to the smoothed curve, which is two orders of magnitude
    // smaller than a containment failure (A12). The count is over the raw
    // steps, not over the smoothed differences: holding a sample back makes
    // the *next* desired step 801, which the clamp trims again while the curve
    // is catching up, and counting that as a second clamp would misread the
    // self-correction as a second excursion.
    let raw = CC7_TRACK_OBSERVED_CENTRES_BASIS_POINTS
        .iter()
        .take(CC7_TRACK_SURVIVING_SAMPLE_COUNT)
        .map(|centre| centre[0])
        .collect::<Vec<_>>();
    let over_the_step = (1..CC7_TRACK_SURVIVING_SAMPLE_COUNT)
        .filter(|index| {
            (raw[*index] - raw[index - 1]).abs() > CC7_TRACK_MAX_STEP_BASIS_POINTS_RESTATED
        })
        .collect::<Vec<_>>();
    assert_eq!(
        over_the_step,
        vec![2],
        "only the 4 -> 9 segment exceeds the step"
    );
    assert_eq!(raw[2] - raw[1], 898, "the raw step the clamp trims");
    assert_eq!(x[2] - x[1], CC7_TRACK_MAX_STEP_BASIS_POINTS_RESTATED);
    let worst_clamp_cost = (0..CC7_TRACK_SURVIVING_SAMPLE_COUNT - 1)
        .map(|index| (x[index] - raw[index]).abs())
        .max()
        .expect("a non-empty curve");
    assert!(
        worst_clamp_cost <= 98,
        "the clamp must self-correct within 98 bp, not {worst_clamp_cost}"
    );
}
