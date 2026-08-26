//! CC6 §11.2: the core-owned colour QC and managed delivery fixtures.
//!
//! Every expected value here is written out analytically from CC6 §3-§6's
//! equations and transcribed **independently** in `f64` (rule 11.0.1). No
//! expected value is obtained by calling [`measure_color_qc`],
//! [`bt709_limited_ycbcr`], or [`encode_bt709_delivery`]; where source content
//! needs a transfer function, this file carries its own transcription, which
//! rule 11.0.1's transcription clause permits.
//!
//! Fixtures that need a GPU compositor, an encoder, or a decoded file are the
//! media crate's half of CC6 and are not duplicated here.

use std::{collections::BTreeMap, path::Path, sync::Arc};

use crossbeam_channel::Receiver;
use kinewright_core::{
    Analysis, AssetId, Clip, ClipContent, ClipId, ColorBitDepth, ColorContext, ColorDescription,
    ColorMatrix, ColorPrimaries, ColorProvenance, ColorQcCheck, ColorQcError, ColorQcRequest,
    ColorRange, ColorTransfer, ColorWhitePoint, DECODED_RANGE_EXCEPTION_BASIS_POINTS,
    DeliveryEncodeDepth, DeliveryProfile, DeliveryTagSource, Document, Effect, EffectId,
    ExportCancellation, ExportSettings, LinearRgbaImage, MATTE_COVERAGE_SCALE,
    MAX_QC_NODE_CONTRIBUTIONS, MatteRegionDescription, MatteRegionScope, MediaAsset, MediaError,
    MediaKind, MonitorProofMetadata, MonitorProofRenderKind, NODE_ATTRIBUTION_REMOVED,
    NormalizedRoi, ParamValue, PlaneLegalExcursion, QC_GAMUT_EXCEPTION_BASIS_POINTS,
    QC_RANGE_EXCEPTION_BASIS_POINTS, QaSeverity, Rational, RgbaImage,
    SKIN_BAND_CENTER_CENTIDEGREES, SKIN_BAND_HALF_WIDTH_CENTIDEGREES, SKIN_MAX_SPREAD_CENTIDEGREES,
    SKIN_MIN_CHROMA_MILLIONTHS, SKIN_PATCH_HUE_CENTIDEGREES, SceneStatus, SilenceStatus, TimeCode,
    TimelineSceneChange, TimelineSilenceSpan, TimelineTranscriptWord, Track, TrackId, TrackKind,
    TranscriptStatus, VisualAssetResult, WORKING_PROOF_ENCODING, WORKING_PROOF_STAGE, WorkingProof,
    WorkingProofMetadata, bt709_limited_ycbcr, delivery_color_for_depth, delivery_color_mismatch,
    delivery_color_mismatches, delivery_tag_check, encode_bt709_delivery, measure_color_qc,
    nodes::measure_node_contributions,
};

/// CC1's `SPEC_F64_TOLERANCE`, restated here because `cc1_fixtures.rs` lives in
/// the media crate and core cannot see it.
const SPEC_F64_TOLERANCE: f64 = 1e-6;

// ---------------------------------------------------------------------------
// Independent transcriptions. None of these calls the engine under test.
// ---------------------------------------------------------------------------

/// The BT.709 display transfer, transcribed in `f64` from CC6 §3.2.
fn reference_encode(linear: f64) -> f64 {
    if linear < 0.0 {
        -reference_encode(-linear)
    } else if linear < 0.018 {
        4.5 * linear
    } else {
        1.099 * linear.powf(0.45) - 0.099
    }
}

/// CC3's `grade709` decode, transcribed in `f64` so the CC5 chart patches can
/// be regenerated as scene-linear source content without calling the media
/// crate.
fn reference_grade709_decode(encoded: f64) -> f64 {
    const ALPHA: f64 = 1.099_296_8;
    const BETA_ENCODED: f64 = 0.081_242_86;
    const K: f64 = 0.099_296_8;
    const INVERSE_EXPONENT: f64 = 2.222_222_3;
    const SLOPE: f64 = 4.5;
    let sign = if encoded > 0.0 {
        1.0
    } else if encoded < 0.0 {
        -1.0
    } else {
        0.0
    };
    let magnitude = encoded.abs();
    if magnitude < BETA_ENCODED {
        sign * magnitude / SLOPE
    } else {
        sign * ((magnitude + K) / ALPHA).powf(INVERSE_EXPONENT)
    }
}

/// The forward BT.709 limited-range matrix, transcribed in `f64` from §3.4.
fn reference_ycbcr(encoded_rgb: [f64; 3], bits: u8) -> [f64; 3] {
    let [red, green, blue] = encoded_rgb;
    let luma = 0.2126 * red + 0.7152 * green + 0.0722 * blue;
    let cb = (blue - luma) / 1.8556;
    let cr = (red - luma) / 1.5748;
    let scale = f64::from(1u32 << (bits - 8));
    [
        16.0 * scale + 219.0 * scale * luma,
        128.0 * scale + 224.0 * scale * cb,
        128.0 * scale + 224.0 * scale * cr,
    ]
}

/// The inverse of [`reference_ycbcr`], transcribed with the rounded matrix
/// constants the media crate's `decode_bt709_ycbcr` already carries.
fn reference_ycbcr_inverse(normalized: [f64; 3], bits: u8) -> [f64; 3] {
    let max_code = f64::from((1u32 << bits) - 1);
    let scale = f64::from(1u32 << (bits - 8));
    let luma = (normalized[0] * max_code - 16.0 * scale) / (219.0 * scale);
    let cb = (normalized[1] * max_code - 128.0 * scale) / (224.0 * scale);
    let cr = (normalized[2] * max_code - 128.0 * scale) / (224.0 * scale);
    [
        luma + 1.5748 * cr,
        luma - 0.187_324 * cb - 0.468_124 * cr,
        luma + 1.8556 * cb,
    ]
}

/// `(Cb, Cr, chroma, hue centidegrees)` for one display-encoded triple.
fn reference_chroma(encoded_rgb: [f64; 3]) -> (f64, f64, f64, i32) {
    let luma = 0.2126 * encoded_rgb[0] + 0.7152 * encoded_rgb[1] + 0.0722 * encoded_rgb[2];
    let cb = (encoded_rgb[2] - luma) / 1.8556;
    let cr = (encoded_rgb[0] - luma) / 1.5748;
    let chroma = (cb * cb + cr * cr).sqrt();
    let degrees = cr.atan2(cb).to_degrees().rem_euclid(360.0);
    #[allow(clippy::cast_possible_truncation)]
    let centidegrees = (degrees * 100.0).round() as i32;
    (cb, cr, chroma, centidegrees)
}

/// `round(v · 1_000_000)`, half away from zero, transcribed independently.
#[allow(clippy::cast_possible_truncation)]
fn reference_millionths(value: f64) -> i64 {
    (value * 1_000_000.0).round() as i64
}

/// `floor(value · 10_000 / count)`, transcribed independently.
#[allow(clippy::cast_possible_truncation)]
fn reference_basis_points(value: u64, count: u64) -> u32 {
    (value * 10_000 / count) as u32
}

// ---------------------------------------------------------------------------
// §11.1: `cc6_qc_raster()` — 80 x 40 = 3200 with basis-point-exact rectangles.
// ---------------------------------------------------------------------------

const RASTER_WIDTH: u32 = 80;
const RASTER_HEIGHT: u32 = 40;
const RASTER_PIXELS: u64 = 3_200;

/// The CC5 chart patches, in `grade709`, transcribed from `cc5_fixtures.rs`.
const CHART_PATCHES: [(&str, [f64; 3]); 6] = [
    ("skin_light", [0.85, 0.68, 0.60]),
    ("skin_medium", [0.72, 0.53, 0.44]),
    ("skin_tan", [0.55, 0.38, 0.30]),
    ("skin_deep", [0.32, 0.20, 0.15]),
    ("product_red", [0.80, 0.10, 0.12]),
    ("product_cyan", [0.10, 0.65, 0.75]),
];

/// The neutral surround the patches sit in: `C = 0` exactly.
const CHART_SURROUND: [f64; 3] = [0.45, 0.45, 0.45];

/// One `grade709` triple as scene-linear light.
fn patch_linear(grade709: [f64; 3]) -> [f32; 3] {
    #[allow(clippy::cast_possible_truncation)]
    [
        reference_grade709_decode(grade709[0]) as f32,
        reference_grade709_decode(grade709[1]) as f32,
        reference_grade709_decode(grade709[2]) as f32,
    ]
}

/// The §11.1 QC raster's linear content at one pixel.
///
/// One pixel is exactly 125 basis points horizontally and 250 vertically, so
/// every region below is a basis-point-exact rectangle.
#[allow(clippy::cast_possible_truncation, clippy::cast_lossless)]
fn qc_raster_pixel(x: u32, y: u32) -> [f32; 3] {
    if y < 24 {
        if x < 48 {
            // In-range ramp, 48 x 24 = 1152: linear 0.0 ..= 1.0 in both axes.
            let horizontal = f64::from(x) / 47.0;
            let vertical = f64::from(y) / 23.0;
            return [
                horizontal as f32,
                vertical as f32,
                f64::midpoint(horizontal, vertical) as f32,
            ];
        }
        if x == 48 && y == 0 {
            // The isolated over pixel: one pixel is 3 bp of 3200, so it can
            // never trip the 10 bp threshold at whole-raster scope.
            return [1.2, 1.2, 1.2];
        }
        if x == 48 && y == 1 {
            // The below-black pixel: Y = -0.008189 < 0, so `d` is undefined.
            return [-0.02, -0.005, -0.005];
        }
        return patch_linear(CHART_SURROUND);
    }
    if y < 32 {
        if x < 36 {
            // Over block, 36 x 8 = 288: e = 1.0243960098942206.
            return [1.05, 1.05, 1.05];
        }
        if x < 72 {
            // Under block, 36 x 8 = 288: range under and gamut, bounded `d`.
            return [-0.01, 0.5, 0.5];
        }
        return patch_linear(CHART_SURROUND);
    }
    if x < 72 {
        return patch_linear(CHART_PATCHES[(x / 12) as usize].1);
    }
    patch_linear(CHART_SURROUND)
}

/// Renderer provenance for a raster this test built itself.
fn test_double_metadata(full_resolution: bool) -> MonitorProofMetadata {
    MonitorProofMetadata {
        render_kind: MonitorProofRenderKind::TestDouble,
        backend: "test_double".to_owned(),
        adapter: "test_double".to_owned(),
        software_fallback: true,
        gpu_claim: false,
        full_resolution,
    }
}

/// Wrap a linear raster as a working proof with honest provenance.
fn working_proof(image: LinearRgbaImage, full_resolution: bool) -> WorkingProof {
    let aspect = i64::from(image.width) * 1_000_000 / i64::from(image.height.max(1));
    WorkingProof {
        metadata: WorkingProofMetadata {
            render: test_double_metadata(full_resolution),
            stage: WORKING_PROOF_STAGE.to_owned(),
            encoding: WORKING_PROOF_ENCODING.to_owned(),
            raster_aspect_millionths: aspect,
        },
        image,
    }
}

/// The §11.1 QC raster.
fn cc6_qc_raster() -> WorkingProof {
    let mut pixels = Vec::with_capacity((RASTER_WIDTH * RASTER_HEIGHT * 4) as usize);
    for y in 0..RASTER_HEIGHT {
        for x in 0..RASTER_WIDTH {
            let [red, green, blue] = qc_raster_pixel(x, y);
            // §3.1: the working raster is opaque by construction.
            pixels.extend([red, green, blue, 1.0]);
        }
    }
    working_proof(
        LinearRgbaImage {
            width: RASTER_WIDTH,
            height: RASTER_HEIGHT,
            pixels,
        },
        true,
    )
}

/// A request measuring range and gamut over an optional region.
fn range_request(roi: Option<NormalizedRoi>) -> ColorQcRequest {
    ColorQcRequest {
        roi,
        checks: vec![ColorQcCheck::Range, ColorQcCheck::Gamut],
        ..ColorQcRequest::default()
    }
}

/// A request measuring the skin diagnostics over one region.
fn skin_request(roi: NormalizedRoi) -> ColorQcRequest {
    ColorQcRequest {
        roi: Some(roi),
        checks: vec![ColorQcCheck::Skin],
        ..ColorQcRequest::default()
    }
}

/// The 49 x 24 = 1176 sub-threshold region of §11.1.
const SUB_THRESHOLD_ROI: NormalizedRoi = NormalizedRoi::new(0, 0, 6_125, 6_000);
/// The 48 x 24 = 1152 ramp, which must trip nothing at all.
const RAMP_ROI: NormalizedRoi = NormalizedRoi::new(0, 0, 6_000, 6_000);
/// The 36 x 8 = 288 over block.
const OVER_BLOCK_ROI: NormalizedRoi = NormalizedRoi::new(0, 6_000, 4_500, 2_000);
/// The 36 x 8 = 288 under block.
const UNDER_BLOCK_ROI: NormalizedRoi = NormalizedRoi::new(4_500, 6_000, 4_500, 2_000);
/// The 8 x 8 = 64 achromatic surround corner beside the patch row.
const SURROUND_ROI: NormalizedRoi = NormalizedRoi::new(9_000, 8_000, 1_000, 2_000);

/// The 12 x 8 = 96 pixel region of one chart patch.
fn patch_roi(index: u32) -> NormalizedRoi {
    NormalizedRoi::new(index * 1_500, 8_000, 1_500, 2_000)
}

// ---------------------------------------------------------------------------
// §11.2.1
// ---------------------------------------------------------------------------

/// The ten §3.2 anchors, hand-derived: `(linear, f64 e, millionths)`.
const RANGE_ANCHORS: [(f64, f64, i64); 10] = [
    // Sign-preserving odd extension of the **power** branch: |−0.02| ≥ 0.018,
    // so this is −(1.099·0.02^0.45 − 0.099) = −0.089_999_732_924_536_89, not
    // the −0.090_000 the linear branch would give. It rounds to −90_000
    // millionths, which is why the wrong pin survived: the pin is the
    // function, not the rounding.
    (-0.02, -0.089_999_732_924_536_89, -90_000),
    (-0.01, -0.045_000, -45_000),
    (-0.005, -0.022_500, -22_500),
    (0.0, 0.0, 0),
    (0.018, 0.081_247_944_035_140_46, 81_248),
    (0.5, 0.705_515_089_922_121_2, 705_515),
    (1.0, 1.000_000, 1_000_000),
    (1.05, 1.024_396_009_894_220_6, 1_024_396),
    (1.2, 1.093_969_260_201_581, 1_093_969),
    (2.0, 1.402_278_242_173_080_6, 1_402_278),
];

#[test]
fn cc6_negative_range_anchors_take_the_power_branch() {
    // The odd extension takes the *power* branch whenever `|linear| >= 0.018`,
    // and the pinned value has to say so. `-0.02` is checked to 1e-12 rather
    // than to `SPEC_F64_TOLERANCE`, because the linear branch's
    // `-4.5 · 0.02 = -0.090_000` sits 2.67e-7 away — inside the looser
    // tolerance, and inside the millionths rounding as well, which is exactly
    // how a wrong pin sits unnoticed in a passing table.
    let (linear, pinned, millionths) = RANGE_ANCHORS[0];
    assert!((linear - -0.02).abs() < f64::EPSILON);
    assert!((reference_encode(linear) - pinned).abs() < 1e-12);
    assert!(
        (pinned - -0.090_000).abs() > 2.6e-7,
        "the pin is the power branch, not the linear branch's -0.090_000"
    );
    assert_eq!(
        millionths, -90_000,
        "both branches round to the same integer, which is why only the \
         encoded pin catches this"
    );
    // The two anchors below the seam are the linear branch, exactly.
    for (linear, pinned, _) in [RANGE_ANCHORS[1], RANGE_ANCHORS[2]] {
        assert!((pinned - 4.5 * linear).abs() < 1e-15);
    }
}

#[test]
fn cc6_range_anchors_match_the_hand_derived_delivery_encode() {
    for (linear, expected_encoded, expected_millionths) in RANGE_ANCHORS {
        // The independent f64 transcription agrees with the pinned table.
        let transcribed = reference_encode(linear);
        assert!(
            (transcribed - expected_encoded).abs() < SPEC_F64_TOLERANCE,
            "transcribed e({linear}) = {transcribed}, pinned {expected_encoded}"
        );
        #[allow(clippy::cast_possible_truncation)]
        let engine = f64::from(encode_bt709_delivery(linear as f32));
        assert!(
            (engine - expected_encoded).abs() < SPEC_F64_TOLERANCE,
            "engine e({linear}) = {engine}, pinned {expected_encoded}"
        );
        assert_eq!(
            reference_millionths(engine),
            expected_millionths,
            "millionths of e({linear})"
        );
    }

    // The boundary is strict: `e(1.0) == 1.0` exactly, in both precisions, and
    // is therefore not an excursion.
    assert!((reference_encode(1.0) - 1.0).abs() < f64::EPSILON);
    assert!((encode_bt709_delivery(1.0) - 1.0).abs() < f32::EPSILON);
    assert!(encode_bt709_delivery(1.0) <= 1.0);

    // The 0.018 seam takes the *power* branch in both precisions, because the
    // f32 literal 0.018 is 0.0179999992251396179 and `linear < 0.018` compares
    // that value to itself.
    let linear_branch = 4.5 * 0.018_f64;
    let power_branch = 1.099 * 0.018_f64.powf(0.45) - 0.099;
    assert!(reference_encode(0.018) > linear_branch);
    assert!((reference_encode(0.018) - power_branch).abs() < SPEC_F64_TOLERANCE);
    assert!(f64::from(encode_bt709_delivery(0.018)) > linear_branch);
    // BT.709's rounded constants make the transfer discontinuous there by
    // 2.479e-4, which is 0.0543 eight-bit codes. Recorded so a future edit to
    // the seam is visible.
    let discontinuity = power_branch - linear_branch;
    assert!(
        (discontinuity - 2.479e-4).abs() < 1e-6,
        "seam discontinuity {discontinuity}"
    );
    // 0.0543 delivery code units on the limited-range 219-code luma span.
    assert!((discontinuity * 219.0 - 0.0543).abs() < 1e-3);

    let proof = cc6_qc_raster();
    let report = measure_color_qc(&proof, &range_request(None)).expect("the raster measures");
    assert_eq!(report.raster, (RASTER_WIDTH, RASTER_HEIGHT));
    assert_eq!(report.visible_pixel_count, RASTER_PIXELS);
    assert_eq!(report.transparent_pixel_count, 0);
    assert_eq!(report.stage, WORKING_PROOF_STAGE);
    assert!(report.evidence_only);

    // §11.1's whole-raster table, on the 3200 denominator.
    for channel in [&report.range.red, &report.range.green, &report.range.blue] {
        assert_eq!(channel.over_pixel_count, 289);
        assert_eq!(channel.over_basis_points, 903);
        assert_eq!(channel.maximum_over_excursion_millionths, 93_969);
    }
    assert_eq!(report.range.red.under_pixel_count, 289);
    assert_eq!(report.range.red.under_basis_points, 903);
    assert_eq!(report.range.red.minimum_under_excursion_millionths, -90_000);
    for channel in [&report.range.green, &report.range.blue] {
        assert_eq!(channel.under_pixel_count, 1);
        assert_eq!(channel.under_basis_points, 3);
        assert_eq!(channel.minimum_under_excursion_millionths, -22_500);
    }
    assert_eq!(report.range.clamped_pixel_count, 578);
    assert_eq!(report.range.clamped_basis_points, 1_806);
    assert_eq!(reference_basis_points(289, RASTER_PIXELS), 903);
    assert_eq!(reference_basis_points(578, RASTER_PIXELS), 1_806);
    assert_eq!(reference_basis_points(1, RASTER_PIXELS), 3);

    // Failing direction: 903 bp is at or above the 10 bp threshold.
    const { assert!(903 >= QC_RANGE_EXCEPTION_BASIS_POINTS) };
    let excursions: Vec<_> = report
        .exceptions
        .iter()
        .filter(|exception| exception.code == "delivery_range_excursion")
        .collect();
    assert!(!excursions.is_empty());
    assert!(
        excursions
            .iter()
            .all(|exception| exception.severity == QaSeverity::Warning)
    );
    // A blown highlight is a creative choice: a warning does not clear the pass.
    assert!(report.technical_pass);

    // Passing direction: the 1176-pixel sub-threshold ROI.
    let sub = measure_color_qc(&proof, &range_request(Some(SUB_THRESHOLD_ROI)))
        .expect("the sub-threshold region measures");
    assert_eq!(sub.region.pixel_roi.width, 49);
    assert_eq!(sub.region.pixel_roi.height, 24);
    assert_eq!(sub.visible_pixel_count, 1_176);
    assert_eq!(sub.range.red.over_basis_points, 8);
    for channel in [&sub.range.red, &sub.range.green, &sub.range.blue] {
        assert_eq!(channel.over_pixel_count, 1);
        assert_eq!(channel.under_pixel_count, 1);
        assert_eq!(channel.under_basis_points, 8);
    }
    assert_eq!(reference_basis_points(1, 1_176), 8);
    const { assert!(8 < QC_RANGE_EXCEPTION_BASIS_POINTS) };
    assert!(
        !sub.exceptions
            .iter()
            .any(|exception| exception.code == "delivery_range_excursion")
    );

    // And the ramp alone, which must trip nothing at all.
    let ramp =
        measure_color_qc(&proof, &range_request(Some(RAMP_ROI))).expect("the ramp region measures");
    assert_eq!(ramp.visible_pixel_count, 1_152);
    for channel in [&ramp.range.red, &ramp.range.green, &ramp.range.blue] {
        assert_eq!(channel.over_pixel_count, 0);
        assert_eq!(channel.under_pixel_count, 0);
        assert_eq!(channel.over_basis_points, 0);
        assert_eq!(channel.under_basis_points, 0);
        assert_eq!(channel.maximum_over_excursion_millionths, 0);
        assert_eq!(channel.minimum_under_excursion_millionths, 0);
    }
    assert!(ramp.exceptions.is_empty());
}

// ---------------------------------------------------------------------------
// §11.2.2
// ---------------------------------------------------------------------------

#[test]
fn cc6_gamut_and_range_under_describe_the_same_pixel_set() {
    let proof = cc6_qc_raster();
    let report = measure_color_qc(&proof, &range_request(None)).expect("the raster measures");

    // The out-of-gamut set is exactly the set of pixels with at least one
    // under-range channel: the under block (288, red only) plus the
    // below-black pixel (1, all three).
    assert_eq!(report.gamut.out_of_gamut_pixel_count, 289);
    assert_eq!(
        report.gamut.out_of_gamut_pixel_count,
        report.range.red.under_pixel_count
    );
    assert_eq!(report.gamut.out_of_gamut_basis_points, 903);
    assert!(
        report.gamut.definition.contains("must not be summed"),
        "the report states the set relation itself"
    );

    // The over block contributes zero to gamut: an over-range positive value
    // is inside the chromaticity triangle and merely brighter than white.
    let over = measure_color_qc(&proof, &range_request(Some(OVER_BLOCK_ROI)))
        .expect("the over block measures");
    assert_eq!(over.visible_pixel_count, 288);
    assert_eq!(over.range.red.over_pixel_count, 288);
    assert_eq!(over.range.red.over_basis_points, 10_000);
    assert_eq!(over.gamut.out_of_gamut_pixel_count, 0);
    assert_eq!(over.gamut.out_of_gamut_basis_points, 0);
    assert_eq!(over.gamut.minimum_linear_millionths, 0);
    assert_eq!(over.gamut.maximum_desaturation_millionths, 0);

    // `d = -m / (Y - m)`, hand-computed for the under block's
    // `(-0.01, 0.5, 0.5)`: Y = 0.391574, so d = 0.01 / 0.401574 = 0.0249021.
    let minimum = -0.01_f64;
    let luma = 0.2126 * minimum + 0.7152 * 0.5 + 0.0722 * 0.5;
    assert!((luma - 0.391_574).abs() < SPEC_F64_TOLERANCE);
    let desaturation = -minimum / (luma - minimum);
    assert!((desaturation - 0.024_902).abs() < SPEC_F64_TOLERANCE);
    assert_eq!(reference_millionths(desaturation), 24_902);
    assert_eq!(report.gamut.maximum_desaturation_millionths, 24_902);
    assert_eq!(report.gamut.minimum_linear_millionths, -20_000);

    // The below-black pixel: Y = -0.008189 < 0, so `d` is undefined for it. It
    // is counted as out of gamut and excluded from the maximum.
    let below_black_luma = 0.2126_f64 * -0.02 + 0.7152 * -0.005 + 0.0722 * -0.005;
    assert!((below_black_luma - -0.008_189).abs() < SPEC_F64_TOLERANCE);
    assert!(below_black_luma < 0.0);
    assert_eq!(report.gamut.below_black_pixel_count, 1);
    // Were it not excluded, `d` for that pixel would exceed 1 and diverge.
    let unbounded: f64 = 0.02 / (below_black_luma + 0.02);
    assert!(unbounded > 1.0);
    assert!((unbounded - 1.693_340).abs() < 1e-5);

    // A pixel with `m < 0 < Y` small gives `d` approaching but not exceeding 1.
    // `d = |m| / (Y + |m|)` with `Y = 0.7874·t − 0.2126·a`: choosing
    // `t / a = 0.28286` puts `Y` just above zero, so `d = 0.98998`.
    let near_black = [-0.5_f32, 0.141_43, 0.141_43];
    let near_luma = 0.2126_f64 * -0.5 + 0.7874 * 0.141_43;
    assert!(near_luma > 0.0);
    let near_desaturation = 0.5 / (near_luma + 0.5);
    assert!((near_desaturation - 0.989_98).abs() < 1e-4);
    let near = measure_color_qc(&single_pixel_proof(near_black), &range_request(None))
        .expect("the single pixel measures");
    assert_eq!(near.gamut.out_of_gamut_pixel_count, 1);
    assert_eq!(near.gamut.below_black_pixel_count, 0);
    assert!(near.gamut.maximum_desaturation_millionths < 1_000_000);
    assert!(near.gamut.maximum_desaturation_millionths > 900_000);
    assert!(
        (near.gamut.maximum_desaturation_millionths - reference_millionths(near_desaturation))
            .abs()
            <= 2
    );

    // Failing direction on the whole raster, passing direction on the ROI.
    const { assert!(903 >= QC_GAMUT_EXCEPTION_BASIS_POINTS) };
    assert!(
        report
            .exceptions
            .iter()
            .any(|exception| exception.code == "delivery_gamut_excursion"
                && exception.severity == QaSeverity::Warning)
    );
    let sub = measure_color_qc(&proof, &range_request(Some(SUB_THRESHOLD_ROI)))
        .expect("the sub-threshold region measures");
    assert_eq!(sub.gamut.out_of_gamut_pixel_count, 1);
    assert_eq!(sub.gamut.out_of_gamut_basis_points, 8);
    const { assert!(8 < QC_GAMUT_EXCEPTION_BASIS_POINTS) };
    assert!(
        !sub.exceptions
            .iter()
            .any(|exception| exception.code == "delivery_gamut_excursion")
    );
    // The under block alone: every pixel is out of gamut and none is below
    // black, so both directions of the same measurement are exercised.
    let under = measure_color_qc(&proof, &range_request(Some(UNDER_BLOCK_ROI)))
        .expect("the under block measures");
    assert_eq!(under.gamut.out_of_gamut_pixel_count, 288);
    assert_eq!(under.gamut.out_of_gamut_basis_points, 10_000);
    assert_eq!(under.gamut.below_black_pixel_count, 0);
    assert_eq!(under.gamut.maximum_desaturation_millionths, 24_902);
}

/// A one-pixel opaque working proof, for anchors that need no raster.
fn single_pixel_proof(linear: [f32; 3]) -> WorkingProof {
    working_proof(
        LinearRgbaImage {
            width: 1,
            height: 1,
            pixels: vec![linear[0], linear[1], linear[2], 1.0],
        },
        true,
    )
}

// ---------------------------------------------------------------------------
// §11.2.3
// ---------------------------------------------------------------------------

/// The §3.4 anchor table: `(R'G'B', [Y, Cb, Cr] at 8 bits)`.
///
/// The bold values of the contract's table sit exactly on a legal chroma
/// bound: maxima from red and blue, minima from yellow and cyan. They are the
/// sharpest possible check that neither bound is off by one.
const YCBCR_ANCHORS_8BIT: [([f64; 3], [f64; 3]); 8] = [
    ([1.0, 1.0, 1.0], [235.0, 128.0, 128.0]),
    ([0.0, 0.0, 0.0], [16.0, 128.0, 128.0]),
    ([0.5, 0.5, 0.5], [125.5, 128.0, 128.0]),
    ([1.0, 0.0, 0.0], [62.559_400, 102.335_848, 240.0]),
    ([0.0, 1.0, 0.0], [172.628_800, 41.664_152, 26.269_749]),
    ([0.0, 0.0, 1.0], [31.811_800, 240.0, 117.730_251]),
    ([1.0, 1.0, 0.0], [219.188_200, 16.0, 138.269_749]),
    ([0.0, 1.0, 1.0], [188.440_600, 153.664_152, 16.0]),
];

/// The same anchors at 10 bits.
const YCBCR_ANCHORS_10BIT: [([f64; 3], [f64; 3]); 8] = [
    ([1.0, 1.0, 1.0], [940.0, 512.0, 512.0]),
    ([0.0, 0.0, 0.0], [64.0, 512.0, 512.0]),
    ([0.5, 0.5, 0.5], [502.0, 512.0, 512.0]),
    ([1.0, 0.0, 0.0], [250.237_600, 409.343_393, 960.0]),
    ([0.0, 1.0, 0.0], [690.515_200, 166.656_607, 105.078_994]),
    ([0.0, 0.0, 1.0], [127.247_200, 960.0, 470.921_006]),
    ([1.0, 1.0, 0.0], [876.752_800, 64.0, 553.078_994]),
    ([0.0, 1.0, 1.0], [753.762_400, 614.656_607, 64.0]),
];

#[test]
fn cc6_bt709_forward_ycbcr_matches_the_spec_at_eight_and_ten_bits() {
    for (bits, anchors) in [(8_u8, YCBCR_ANCHORS_8BIT), (10, YCBCR_ANCHORS_10BIT)] {
        let scale = f64::from(1u32 << (bits - 8));
        for (rgb, expected) in anchors {
            let transcribed = reference_ycbcr(rgb, bits);
            let engine = bt709_limited_ycbcr(rgb, bits);
            for plane in 0..3 {
                assert!(
                    (transcribed[plane] - expected[plane]).abs() < SPEC_F64_TOLERANCE,
                    "transcription of {rgb:?} plane {plane} at {bits} bits"
                );
                assert!(
                    (engine[plane] - expected[plane]).abs() < SPEC_F64_TOLERANCE,
                    "engine {rgb:?} plane {plane} at {bits} bits: {} vs {}",
                    engine[plane],
                    expected[plane]
                );
            }
            // Passing direction: an in-range R'G'B' is never an excursion, and
            // the values sitting exactly on a bound are asserted not to be one
            // under the strict `>` / `<` tests.
            assert!(engine[0] >= 16.0 * scale && engine[0] <= 235.0 * scale);
            for chroma in [engine[1], engine[2]] {
                assert!(chroma >= 16.0 * scale && chroma <= 240.0 * scale);
            }

            // The round trip through the inverse matrix, after dividing the
            // codes by `2^bits - 1` because the decode takes normalized
            // samples.
            let max_code = f64::from((1u32 << bits) - 1);
            let normalized = [
                engine[0] / max_code,
                engine[1] / max_code,
                engine[2] / max_code,
            ];
            let recovered = reference_ycbcr_inverse(normalized, bits);
            for channel in 0..3 {
                assert!(
                    (recovered[channel] - rgb[channel]).abs() < SPEC_F64_TOLERANCE,
                    "round trip of {rgb:?} channel {channel} at {bits} bits: {}",
                    recovered[channel]
                );
            }
        }
    }

    // The forward constants are the exact inverses of the four the media crate
    // already carries for the decode direction.
    let blue_axis_to_green: f64 = 0.0722 * 1.8556 / 0.7152;
    let red_axis_to_green: f64 = 0.2126 * 1.5748 / 0.7152;
    assert!((blue_axis_to_green - 0.187_324_272_930_648_8).abs() < 1e-12);
    assert!((red_axis_to_green - 0.468_124_272_930_648_84).abs() < 1e-12);
    assert!((blue_axis_to_green - 0.187_324).abs() < SPEC_F64_TOLERANCE);
    assert!((red_axis_to_green - 0.468_124).abs() < SPEC_F64_TOLERANCE);

    // `linear = 1.05` on all three channels predicts a luma-only excursion.
    let encoded = reference_encode(1.05);
    assert!((encoded - 1.024_396_009_894_220_6).abs() < SPEC_F64_TOLERANCE);
    let luma_8 = reference_ycbcr([encoded; 3], 8);
    let luma_10 = reference_ycbcr([encoded; 3], 10);
    assert!((luma_8[0] - 240.342_726).abs() < SPEC_F64_TOLERANCE);
    assert!((luma_10[0] - 961.370_905).abs() < SPEC_F64_TOLERANCE);
    // Cb and Cr stay exactly at their offsets: the excursion is luma-only, and
    // that attribution is what the RGB test cannot see.
    assert!((luma_8[1] - 128.0).abs() < SPEC_F64_TOLERANCE);
    assert!((luma_10[1] - 512.0).abs() < SPEC_F64_TOLERANCE);

    // Failing direction: a synthetic R'G'B' outside [0, 1] produces codes
    // outside the legal box, in both directions and on both plane kinds.
    let over = bt709_limited_ycbcr([1.2, 1.2, 1.2], 8);
    assert!(over[0] > 235.0);
    let under = bt709_limited_ycbcr([-0.1, -0.1, -0.1], 8);
    assert!(under[0] < 16.0);
    let chroma_over = bt709_limited_ycbcr([-0.2, 0.0, 1.2], 8);
    assert!(chroma_over[1] > 240.0);
    let chroma_under = bt709_limited_ycbcr([1.2, 1.0, -0.2], 8);
    assert!(chroma_under[1] < 16.0);

    // And the engine's own predicted report agrees, at both depths.
    for depth in DeliveryEncodeDepth::ALL {
        let request = ColorQcRequest {
            delivery_bit_depth: depth,
            ..range_request(Some(OVER_BLOCK_ROI))
        };
        let report = measure_color_qc(&cc6_qc_raster(), &request).expect("the over block measures");
        let predicted = &report.range.predicted_ycbcr;
        assert_eq!(predicted.bit_depth, depth.bits());
        assert_eq!(predicted.luma.above_count, 288);
        assert_eq!(predicted.luma.above_basis_points, 10_000);
        assert_eq!(predicted.luma.below_count, 0);
        assert_eq!(predicted.cb.above_count, 0);
        assert_eq!(predicted.cb.below_count, 0);
        assert_eq!(predicted.cr.above_count, 0);
        assert_eq!(predicted.cr.below_count, 0);
        let expected_luma = if depth == DeliveryEncodeDepth::Eight {
            240.342_726
        } else {
            961.370_905
        };
        #[allow(clippy::cast_possible_truncation)]
        let expected_hundredths = (expected_luma * 100.0_f64).round() as i64;
        assert_eq!(predicted.luma.maximum_code_hundredths, expected_hundredths);
    }

    // A legal source predicts exactly zero excursions on every plane.
    let ramp = measure_color_qc(&cc6_qc_raster(), &range_request(Some(RAMP_ROI)))
        .expect("the ramp region measures");
    for plane in [
        &ramp.range.predicted_ycbcr.luma,
        &ramp.range.predicted_ycbcr.cb,
        &ramp.range.predicted_ycbcr.cr,
    ] {
        assert_eq!(plane.below_count, 0);
        assert_eq!(plane.above_count, 0);
    }
}

// ---------------------------------------------------------------------------
// §11.2.4
// ---------------------------------------------------------------------------

#[test]
fn cc6_skin_band_constants_are_derived_from_the_cc5_patches() {
    // Each CC5 patch, transcribed independently and transformed
    // grade709_decode -> encode -> Cb/Cr -> atan2.
    let mut hues = Vec::new();
    let mut chromas = Vec::new();
    for (name, grade709) in CHART_PATCHES {
        let encoded = [
            reference_encode(reference_grade709_decode(grade709[0])),
            reference_encode(reference_grade709_decode(grade709[1])),
            reference_encode(reference_grade709_decode(grade709[2])),
        ];
        let (_, _, chroma, centidegrees) = reference_chroma(encoded);
        hues.push((name, centidegrees));
        chromas.push((name, chroma));
    }
    let skin_hues: Vec<i32> = hues[..4].iter().map(|(_, hue)| *hue).collect();
    assert_eq!(skin_hues, SKIN_PATCH_HUE_CENTIDEGREES.to_vec());

    // `skin_light` and `skin_tan` genuinely share an angle and a chroma.
    assert_eq!(hues[0].1, hues[2].1);
    assert!((chromas[0].1 - chromas[2].1).abs() < 1e-6);

    // The centre is the circular mean of the four, with R = 0.999885.
    let sine: f64 = skin_hues
        .iter()
        .map(|hue| (f64::from(*hue) / 100.0).to_radians().sin())
        .sum();
    let cosine: f64 = skin_hues
        .iter()
        .map(|hue| (f64::from(*hue) / 100.0).to_radians().cos())
        .sum();
    let mean = sine.atan2(cosine).to_degrees().rem_euclid(360.0);
    #[allow(clippy::cast_possible_truncation)]
    let mean_centidegrees = (mean * 100.0).round() as i32;
    assert_eq!(mean_centidegrees, SKIN_BAND_CENTER_CENTIDEGREES);
    assert_eq!(SKIN_BAND_CENTER_CENTIDEGREES, 12_339);
    let resultant = (cosine * cosine + sine * sine).sqrt() / 4.0;
    assert_eq!(reference_millionths(resultant), 999_885);

    // Every patch sits at least 1049 centidegrees inside a band edge and at
    // most 1154.
    let half = SKIN_BAND_HALF_WIDTH_CENTIDEGREES;
    assert_eq!(half, 1_200);
    let margin = |hue: i32| half - (hue - SKIN_BAND_CENTER_CENTIDEGREES).abs();
    assert_eq!(margin(12_188), 1_049, "skin_deep is the tightest patch");
    assert_eq!(margin(12_396), 1_143, "skin_medium");
    assert_eq!(margin(12_385), 1_154, "skin_light and skin_tan");
    for hue in skin_hues {
        assert!(margin(hue) >= 1_049 && margin(hue) <= 1_154);
    }

    // The derived NTSC +I axis at exactly 123.0000 degrees is corroboration,
    // not the source of the centre.
    let ntsc_i = (33.0_f64)
        .to_radians()
        .cos()
        .atan2(-(33.0_f64).to_radians().sin());
    #[allow(clippy::cast_possible_truncation)]
    let ntsc_i_centidegrees = (ntsc_i.to_degrees().rem_euclid(360.0) * 100.0).round() as i32;
    assert_eq!(ntsc_i_centidegrees, 12_300);
    assert!(margin(ntsc_i_centidegrees) > 0, "the +I axis is inside");
    assert!((SKIN_BAND_CENTER_CENTIDEGREES - ntsc_i_centidegrees).abs() <= 39);

    // Failing directions: the Rec.709 red primary and the two product patches.
    let (_, _, _, red_primary) = reference_chroma([1.0, 0.0, 0.0]);
    assert_eq!(red_primary, 10_291);
    assert_eq!(margin(red_primary), -848, "848 cd outside the lower edge");
    assert_eq!(hues[4].1, 10_137, "product_red");
    assert_eq!(margin(hues[4].1), -1_002);
    assert_eq!(hues[5].1, 29_201, "product_cyan");
    assert!(margin(hues[5].1) < 0);

    // The chroma floor: skin_deep is 3.67x it, and the surround is exactly 0.
    let deep_chroma = reference_millionths(chromas[3].1);
    assert_eq!(deep_chroma, 73_341);
    assert_eq!(SKIN_MIN_CHROMA_MILLIONTHS, 20_000);
    #[allow(clippy::cast_precision_loss)]
    let ratio = deep_chroma as f64 / SKIN_MIN_CHROMA_MILLIONTHS as f64;
    assert!((ratio - 3.67).abs() < 0.01);
    // 0.02 * 224 = 4.48 eight-bit code units from 128: a few codes, not a
    // fraction of one.
    assert!((0.02 * 224.0 - 4.48_f64).abs() < SPEC_F64_TOLERANCE);
    let surround_encoded = reference_encode(reference_grade709_decode(0.45));
    let (_, _, surround_chroma, _) = reference_chroma([surround_encoded; 3]);
    assert_eq!(reference_millionths(surround_chroma), 0);
}

// ---------------------------------------------------------------------------
// §11.2.5
// ---------------------------------------------------------------------------

/// The display-encoded triple of one chart patch, transcribed independently.
fn patch_encoded(index: usize) -> [f64; 3] {
    let grade709 = CHART_PATCHES[index].1;
    [
        reference_encode(reference_grade709_decode(grade709[0])),
        reference_encode(reference_grade709_decode(grade709[1])),
        reference_encode(reference_grade709_decode(grade709[2])),
    ]
}

#[test]
fn cc6_skin_diagnostics_report_circular_statistics_on_a_chosen_region() {
    let proof = cc6_qc_raster();

    // Passing direction: an ROI covering exactly one skin patch.
    for index in 0..4_u32 {
        let report = measure_color_qc(&proof, &skin_request(patch_roi(index)))
            .expect("the patch region measures");
        let skin = report.skin.as_ref().expect("skin was requested");
        assert_eq!(skin.region_pixel_count, 96);
        assert_eq!(skin.considered_pixel_count, 96);
        assert_eq!(skin.excluded_achromatic_pixel_count, 0);

        let (_, _, chroma, centidegrees) = reference_chroma(patch_encoded(index as usize));
        assert_eq!(skin.mean_hue_centidegrees, Some(centidegrees));
        assert_eq!(centidegrees, SKIN_PATCH_HUE_CENTIDEGREES[index as usize]);
        // A uniform patch has R = 1 after the clamp, so the spread is exactly
        // zero rather than NaN.
        assert_eq!(skin.hue_concentration_millionths, 1_000_000);
        assert_eq!(skin.circular_spread_centidegrees, 0);
        let expected_chroma = reference_millionths(chroma);
        assert!(
            (skin.median_chroma_millionths - expected_chroma).abs() <= 1,
            "{}: median chroma {} vs {expected_chroma}",
            CHART_PATCHES[index as usize].0,
            skin.median_chroma_millionths
        );
        assert_eq!(skin.in_band_basis_points, 10_000);
        assert_eq!(skin.band_center_centidegrees, SKIN_BAND_CENTER_CENTIDEGREES);
        assert_eq!(
            skin.band_half_width_centidegrees,
            SKIN_BAND_HALF_WIDTH_CENTIDEGREES
        );
        assert!(skin.boundary.contains("not a skin detector"));
        assert!(
            !report
                .exceptions
                .iter()
                .any(|exception| exception.code == "skin_region_outside_band")
        );
    }

    // Failing direction: a product patch is chromatic but outside the band.
    for index in 4..6_u32 {
        let report = measure_color_qc(&proof, &skin_request(patch_roi(index)))
            .expect("the product region measures");
        let skin = report.skin.as_ref().expect("skin was requested");
        assert_eq!(skin.considered_pixel_count, 96);
        assert_eq!(skin.in_band_basis_points, 0);
        let exception = report
            .exceptions
            .iter()
            .find(|exception| exception.code == "skin_region_outside_band")
            .expect("a chromatic region outside the band is reported");
        // Info, not Warning: a chosen region that is not skin is a user choice.
        assert_eq!(exception.severity, QaSeverity::Info);
        assert!(report.technical_pass);
    }

    // The 0/0 rule: an all-achromatic region produces no hue evidence at all,
    // and reporting it as "outside the band" would be a fabricated finding.
    let surround = measure_color_qc(&proof, &skin_request(SURROUND_ROI))
        .expect("the surround region measures");
    let skin = surround.skin.as_ref().expect("skin was requested");
    assert_eq!(skin.region_pixel_count, 64);
    assert_eq!(skin.considered_pixel_count, 0);
    assert_eq!(skin.excluded_achromatic_pixel_count, 64);
    assert_eq!(skin.mean_hue_centidegrees, None);
    assert_eq!(skin.hue_concentration_millionths, 0);
    assert_eq!(
        skin.circular_spread_centidegrees,
        SKIN_MAX_SPREAD_CENTIDEGREES
    );
    assert_eq!(skin.circular_spread_centidegrees, 18_000);
    assert_eq!(skin.median_chroma_millionths, 0);
    assert_eq!(skin.in_band_basis_points, 0);
    assert!(
        !surround
            .exceptions
            .iter()
            .any(|exception| exception.code == "skin_region_outside_band")
    );

    // Wrap-around: two synthetic hues straddling 0/360 average to 0, not to
    // 18000, because the mean is taken from f64 sums of cos and sin.
    let wrap = wrap_around_proof();
    let report = measure_color_qc(&wrap, &skin_request(NormalizedRoi::full_frame()))
        .expect("the wrap raster measures");
    let skin = report.skin.as_ref().expect("skin was requested");
    assert_eq!(skin.considered_pixel_count, 2);
    let mean = skin.mean_hue_centidegrees.expect("two chromatic pixels");
    // Exactly 0, not "0 or nearly 360": `centidegrees` takes `rem_euclid`
    // after rounding, so an angle a hair under 360° rounds to 36000 and wraps
    // to 0. A two-branch assertion would have accepted a genuinely wrong mean
    // of 359.99°.
    assert_eq!(
        mean, 0,
        "circular mean of 35900 and 100 centidegrees is 0, got {mean}"
    );
}

/// A two-pixel proof carrying synthetic hues at 35900 and 100 centidegrees.
///
/// The chroma vectors are built directly, then inverted through the BT.709
/// matrix to the linear light that produces them, so the hue is chosen rather
/// than discovered.
fn wrap_around_proof() -> WorkingProof {
    let mut pixels = Vec::new();
    for centidegrees in [35_900.0_f64, 100.0] {
        let radians = (centidegrees / 100.0).to_radians();
        let chroma = 0.10;
        let cb = chroma * radians.cos();
        let cr = chroma * radians.sin();
        // Choose Y' = 0.5 and invert: R' = Y' + 1.5748 Cr, B' = Y' + 1.8556 Cb,
        // G' = Y' - 0.187324 Cb - 0.468124 Cr.
        let luma = 0.5;
        let encoded = [
            luma + 1.5748 * cr,
            luma - 0.187_324 * cb - 0.468_124 * cr,
            luma + 1.8556 * cb,
        ];
        for value in encoded {
            // Invert the display transfer to scene-linear light.
            let linear = if value < 0.081 {
                value / 4.5
            } else {
                ((value + 0.099) / 1.099).powf(1.0 / 0.45)
            };
            #[allow(clippy::cast_possible_truncation)]
            pixels.push(linear as f32);
        }
        pixels.push(1.0);
    }
    working_proof(
        LinearRgbaImage {
            width: 2,
            height: 1,
            pixels,
        },
        true,
    )
}

// ---------------------------------------------------------------------------
// §11.2.8
// ---------------------------------------------------------------------------

#[test]
fn cc6_working_proof_refuses_a_claim_that_is_not_full_resolution() {
    // A proof whose provenance cannot claim the document raster is refused
    // before any measurement runs. The two legs of the compositor's
    // `full_resolution` conjunction — a scale that is not full resolution, and
    // a full scale whose rendered raster differs from `document.resolution` —
    // are both derived on the media side and both arrive here as `false`.
    let mut proof = cc6_qc_raster();
    proof.metadata.render.full_resolution = false;
    let error = measure_color_qc(&proof, &range_request(None))
        .expect_err("a proxy claim must be refused, not measured");
    assert_eq!(error.code(), "color_qc_proxy_proof_refused");
    assert_eq!(error.field(), "full_resolution");
    assert_eq!(error.observed(), "false");
    assert!(error.allowed_values().contains("true"));
    assert!(error.recovery_action().contains("no proxy working proof"));

    // Passing direction, one step away: the real full-resolution proof.
    let honest = cc6_qc_raster();
    assert!(honest.metadata.render.full_resolution);
    let report = measure_color_qc(&honest, &range_request(None)).expect("a full proof measures");
    assert!(report.full_resolution);
}

// ---------------------------------------------------------------------------
// §11.2.9
// ---------------------------------------------------------------------------

/// The description `probe_path` produces for a written H.264 delivery file.
fn probed_h264_description(depth: DeliveryEncodeDepth) -> ColorDescription {
    ColorDescription {
        primaries: ColorPrimaries::Bt709,
        transfer: ColorTransfer::Bt709,
        matrix: ColorMatrix::Bt709,
        range: ColorRange::Limited,
        // H.264/AVC has no white-point field at all.
        white_point: ColorWhitePoint::Unknown,
        bit_depth: depth.color_bit_depth(),
        confidence_basis_points: 10_000,
        provenance: ColorProvenance::StreamMetadata,
    }
}

// One fixture per contract clause: §11.2.9 states both modes, both lanes,
// the not-representable rule, and the two functions' non-interchangeability
// as one requirement, and splitting it would hide which half regressed.
#[allow(clippy::too_many_lines)]
#[test]
fn cc6_delivery_tag_check_covers_both_modes_and_marks_white_point_not_representable() {
    let document = managed_document();

    // Pre-export mode, both lanes.
    for depth in DeliveryEncodeDepth::ALL {
        let expected = delivery_color_for_depth(&document, depth);
        assert_eq!(expected.bit_depth, depth.color_bit_depth());
        let check = delivery_tag_check(
            &expected,
            &expected,
            DeliveryTagSource::MaterialisedExportSettings,
        );
        assert_eq!(check.tag_source, "materialised_export_settings");
        assert!(check.conforming);
        assert!(check.mismatches.is_empty());
        assert!(check.not_representable.is_empty());
    }

    // Four wrong fields produce four mismatches in the fixed check order,
    // while `delivery_color_mismatch` still returns exactly the first.
    let mut wrong = ColorContext::sdr_rec709().delivery;
    wrong.primaries = ColorPrimaries::Bt2020;
    wrong.transfer = ColorTransfer::Smpte2084;
    wrong.matrix = ColorMatrix::Rgb;
    wrong.range = ColorRange::Full;
    let mismatches = delivery_color_mismatches(&wrong);
    let fields: Vec<&str> = mismatches
        .iter()
        .map(|mismatch| mismatch.field.as_str())
        .collect();
    assert_eq!(fields, vec!["primaries", "transfer", "matrix", "range"]);
    assert_eq!(
        delivery_color_mismatch(&wrong).map(|mismatch| mismatch.field),
        Some("primaries".to_owned())
    );
    let check = delivery_tag_check(
        &wrong,
        &wrong,
        DeliveryTagSource::MaterialisedExportSettings,
    );
    assert_eq!(check.mismatches.len(), 4);
    assert!(!check.conforming);

    // Post-export mode: a probed description produces zero mismatches and
    // exactly one not-representable entry, for both lanes.
    for depth in DeliveryEncodeDepth::ALL {
        let expected = delivery_color_for_depth(&document, depth);
        let probed = probed_h264_description(depth);
        let check = delivery_tag_check(&expected, &probed, DeliveryTagSource::ProbedOutputFile);
        assert_eq!(check.tag_source, "probed_output_file");
        assert!(
            check.mismatches.is_empty(),
            "a correctly written file must not mismatch: {:?}",
            check.mismatches
        );
        assert!(check.conforming);
        assert_eq!(check.not_representable.len(), 1);
        let entry = &check.not_representable[0];
        assert_eq!(entry.field, "white_point");
        assert_eq!(entry.expected, "d65");
        assert!(entry.reason.contains("no white-point field"));

        // The two functions are not interchangeable: `delivery_color_mismatch`
        // applied to the same probed description *does* reject it, on the
        // white point, which is exactly why it must never be applied to one.
        let rejected = delivery_color_mismatch(&probed).expect("a probe cannot satisfy the gate");
        assert_eq!(rejected.field, "white_point");
    }

    // A genuinely mis-tagged file is still caught in post-export mode.
    let expected = delivery_color_for_depth(&document, DeliveryEncodeDepth::Eight);
    let mut mistagged = probed_h264_description(DeliveryEncodeDepth::Eight);
    mistagged.primaries = ColorPrimaries::Bt2020;
    mistagged.range = ColorRange::Full;
    let check = delivery_tag_check(&expected, &mistagged, DeliveryTagSource::ProbedOutputFile);
    assert!(!check.conforming);
    let fields: Vec<&str> = check
        .mismatches
        .iter()
        .map(|mismatch| mismatch.field.as_str())
        .collect();
    assert_eq!(fields, vec!["primaries", "range"]);

    // A tag mismatch is an Error and clears `technical_pass`; nothing else in
    // the report does.
    let proof = cc6_qc_raster();
    let request = ColorQcRequest {
        checks: vec![ColorQcCheck::Tags],
        expected_delivery: Some(expected.clone()),
        observed_delivery: Some(mistagged),
        ..ColorQcRequest::default()
    };
    let report = measure_color_qc(&proof, &request).expect("the raster measures");
    assert!(!report.technical_pass);
    assert_eq!(
        report
            .exceptions
            .iter()
            .filter(|exception| exception.code == "delivery_tag_mismatch")
            .count(),
        2
    );
    // Exceptions are ordered severity descending, so the Errors come first.
    assert_eq!(report.exceptions[0].severity, QaSeverity::Error);
    assert_eq!(
        report.exceptions.last().map(|exception| exception.severity),
        Some(QaSeverity::Info)
    );
    assert!(
        report
            .exceptions
            .iter()
            .any(|exception| exception.code == "delivery_tag_not_representable")
    );

    // Passing direction one step away: the conforming probe.
    let conforming = ColorQcRequest {
        observed_delivery: Some(probed_h264_description(DeliveryEncodeDepth::Eight)),
        ..request
    };
    let report = measure_color_qc(&proof, &conforming).expect("the raster measures");
    assert!(report.technical_pass);
    assert!(
        !report
            .exceptions
            .iter()
            .any(|exception| exception.code == "delivery_tag_mismatch")
    );

    // The conformance message names both accepted depths, not the pre-CC6
    // 8-bit-only string.
    let mut rejected_document = managed_document();
    rejected_document.color_context.delivery.transfer = ColorTransfer::Smpte2084;
    let report = kinewright_core::delivery_conformance(
        &rejected_document,
        DeliveryProfile::SourceMaster,
        DeliveryEncodeDepth::Eight,
        50,
        50,
    )
    .expect("an unsupported delivery colour is reported, not returned as an error");
    let issue = report
        .issues
        .iter()
        .find(|issue| issue.code == "unsupported_delivery_color")
        .expect("the transfer must block export");
    assert!(issue.message.contains("8-bit or 10-bit"));
    assert!(!issue.message.contains("explicit 8-bit SDR"));
}

// ---------------------------------------------------------------------------
// §11.2.15 (core half)
// ---------------------------------------------------------------------------

/// An opaque coverage raster with the given code in every colour channel.
fn coverage_raster(width: u32, height: u32, code: u8) -> RgbaImage {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for _ in 0..width * height {
        pixels.extend([code, code, code, MATTE_COVERAGE_SCALE.min(255) as u8]);
    }
    RgbaImage {
        width,
        height,
        pixels,
    }
}

/// A matte scope carrying one coverage raster.
fn matte_scope(coverage: RgbaImage) -> MatteRegionScope {
    let covered = u64::from(coverage.width) * u64::from(coverage.height);
    MatteRegionScope {
        description: MatteRegionDescription::new(ClipId(1), EffectId(1), covered),
        coverage,
    }
}

#[test]
fn cc6_typed_qc_refusals_carry_code_field_observed_and_allowed() {
    // color_qc_raster_length_mismatch: the buffer shortened by one pixel.
    let mut short = cc6_qc_raster();
    short.image.pixels.truncate(short.image.pixels.len() - 4);
    let error = measure_color_qc(&short, &range_request(None)).expect_err("a short buffer refuses");
    assert_eq!(error.code(), "color_qc_raster_length_mismatch");
    assert_eq!(error.field(), "pixels");
    assert!(error.observed().contains("12796"));
    assert!(error.allowed_values().contains("12800"));
    assert!(error.allowed_values().contains("80x40"));
    // Passing neighbour: the correct length.
    assert!(measure_color_qc(&cc6_qc_raster(), &range_request(None)).is_ok());

    // color_qc_region_empty: a matte whose coverage is empty everywhere.
    let empty_region = ColorQcRequest {
        matte_region: Some(matte_scope(coverage_raster(RASTER_WIDTH, RASTER_HEIGHT, 0))),
        ..range_request(None)
    };
    let error = measure_color_qc(&cc6_qc_raster(), &empty_region)
        .expect_err("an empty region refuses rather than dividing by zero");
    assert_eq!(error.code(), "color_qc_region_empty");
    assert_eq!(error.field(), "region");
    assert_eq!(error.observed(), "0 pixels");
    assert!(error.allowed_values().contains("at least one pixel"));
    // Passing neighbour, one step away: coverage above the threshold.
    let covered = ColorQcRequest {
        matte_region: Some(matte_scope(coverage_raster(RASTER_WIDTH, RASTER_HEIGHT, 1))),
        ..range_request(None)
    };
    let report = measure_color_qc(&cc6_qc_raster(), &covered).expect("a covered region measures");
    assert_eq!(report.visible_pixel_count, RASTER_PIXELS);
    assert_eq!(
        report
            .region
            .matte_region
            .as_ref()
            .map(|region| region.clip),
        Some(ClipId(1))
    );

    // color_qc_node_budget_exceeded: both directions, 0 and 17.
    for budget in [0_u8, 17] {
        let request = ColorQcRequest {
            max_nodes: budget,
            ..range_request(None)
        };
        let error = measure_color_qc(&cc6_qc_raster(), &request)
            .expect_err("a budget outside 1..=16 refuses");
        assert_eq!(error.code(), "color_qc_node_budget_exceeded");
        assert_eq!(error.field(), "max_nodes");
        assert_eq!(error.observed(), budget.to_string());
        assert_eq!(error.allowed_values(), "1..=16");
    }
    // Passing neighbours, one step away on each side.
    for budget in [1_u8, 16] {
        let request = ColorQcRequest {
            max_nodes: budget,
            ..range_request(None)
        };
        assert!(measure_color_qc(&cc6_qc_raster(), &request).is_ok());
    }
    assert_eq!(MAX_QC_NODE_CONTRIBUTIONS, 16);

    // color_qc_matte_region_raster_mismatch: a coverage raster one pixel wider.
    let mismatched = ColorQcRequest {
        matte_region: Some(matte_scope(coverage_raster(
            RASTER_WIDTH + 1,
            RASTER_HEIGHT,
            255,
        ))),
        ..range_request(None)
    };
    let error = measure_color_qc(&cc6_qc_raster(), &mismatched)
        .expect_err("a coverage raster of the wrong size refuses");
    assert_eq!(error.code(), "color_qc_matte_region_raster_mismatch");
    assert_eq!(error.field(), "coverage");
    assert_eq!(error.observed(), "81x40 with 12960 u8 samples");
    assert_eq!(error.allowed_values(), "80x40 with 12800 u8 samples");

    // L6: dimensions that agree while the buffer does not. Without the length
    // check this measured a silently smaller region — every pixel past the end
    // of the coverage buffer reads as coverage 0 and is skipped — and reported
    // it under the requested region's name.
    let mut short_coverage = coverage_raster(RASTER_WIDTH, RASTER_HEIGHT, 255);
    short_coverage
        .pixels
        .truncate(short_coverage.pixels.len() - 4);
    let short_scope = ColorQcRequest {
        matte_region: Some(matte_scope(short_coverage)),
        ..range_request(None)
    };
    let error = measure_color_qc(&cc6_qc_raster(), &short_scope)
        .expect_err("a coverage buffer shorter than its own raster refuses");
    assert_eq!(error.code(), "color_qc_matte_region_raster_mismatch");
    assert_eq!(error.field(), "coverage");
    assert_eq!(error.observed(), "80x40 with 12796 u8 samples");
    assert_eq!(error.allowed_values(), "80x40 with 12800 u8 samples");
    // Passing neighbour: the matching raster.
    let matching = ColorQcRequest {
        matte_region: Some(matte_scope(coverage_raster(
            RASTER_WIDTH,
            RASTER_HEIGHT,
            255,
        ))),
        ..range_request(None)
    };
    assert!(measure_color_qc(&cc6_qc_raster(), &matching).is_ok());

    // Every refusal renders one actionable message carrying all four facts.
    let error = ColorQcError::NodeBudgetExceeded {
        observed: "0".to_owned(),
        allowed: "1..=16",
    };
    let message = error.actionable_message();
    for expected in ["field=max_nodes", "observed=0", "allowed=1..=16"] {
        assert!(message.contains(expected), "{message}");
    }
}

/// Refusals compose into `MediaError` structurally (errata E32): the typed
/// code survives `?` instead of being flattened into a `Backend` string.
#[test]
fn cc6_qc_refusals_keep_their_code_through_media_error() {
    let error = ColorQcError::NodeBudgetExceeded {
        observed: "0".to_owned(),
        allowed: "1..=16",
    };
    let media: MediaError = error.clone().into();
    assert_eq!(media, MediaError::ColorQc(error));
    assert_eq!(media.recovery_code(), Some("color_qc_node_budget_exceeded"));
}

/// §6.4 (a): the strict-box excursion rate counts **both** directions.
///
/// The rate the 100-basis-point threshold is compared against is
/// `below_count + above_count` over the plane's own population, not
/// `max(below, above)`. The distinction is invisible on a plane that only ever
/// leaves the box one way and is exactly the plane that matters when it leaves
/// both ways, so it is pinned here and used by both the media verifier's gate
/// and the CC6 fixture's prediction.
#[test]
fn cc6_plane_excursion_basis_points_count_both_directions() {
    let excursion = |below: u64, above: u64| PlaneLegalExcursion {
        below_count: below,
        above_count: above,
        below_basis_points: 0,
        above_basis_points: 0,
        minimum_code_hundredths: 0,
        maximum_code_hundredths: 0,
    };

    // The discriminating case: neither direction reaches the threshold on its
    // own, and together they cross it. `max(below, above)` would answer 51.
    let both = excursion(50, 51);
    assert_eq!(both.excursion_basis_points(10_000), 101);
    assert!(both.excursion_basis_points(10_000) > DECODED_RANGE_EXCEPTION_BASIS_POINTS);
    for alone in [excursion(50, 0), excursion(0, 51)] {
        assert!(
            alone.excursion_basis_points(10_000) < DECODED_RANGE_EXCEPTION_BASIS_POINTS,
            "neither direction reaches the threshold on its own: {alone:?}"
        );
    }

    // Integer floor, never rounding: 1 sample in 10 001 is 0 basis points, and
    // the next one is 1.
    assert_eq!(excursion(1, 0).excursion_basis_points(10_001), 0);
    assert_eq!(excursion(2, 0).excursion_basis_points(10_001), 1);
    assert_eq!(excursion(0, 1).excursion_basis_points(10_001), 0);
    // Every sample outside the box is the whole population.
    assert_eq!(excursion(3, 7).excursion_basis_points(10), 10_000);
    // An empty population is 0, the same answer the two per-direction rates
    // carry for one, and never a division.
    assert_eq!(excursion(0, 0).excursion_basis_points(0), 0);
    assert_eq!(excursion(5, 5).excursion_basis_points(0), 0);
    // A plane nothing was measured on: no excursions and no rate.
    let unseen = PlaneLegalExcursion {
        minimum_code_hundredths: PlaneLegalExcursion::UNSEEN_MINIMUM_CODE_HUNDREDTHS,
        maximum_code_hundredths: PlaneLegalExcursion::UNSEEN_MAXIMUM_CODE_HUNDREDTHS,
        ..excursion(0, 0)
    };
    assert!(!unseen.samples_seen());
    assert_eq!(unseen.excursion_basis_points(0), 0);
}

// ---------------------------------------------------------------------------
// §11.2.17 (core half; the `ExportJobRecord` half is the agent's)
// ---------------------------------------------------------------------------

#[test]
fn cc6_export_settings_and_job_records_serialize_deterministically() {
    let document = managed_document();
    for depth in DeliveryEncodeDepth::ALL {
        let settings = DeliveryProfile::Youtube1080p.export_settings(
            &document,
            depth,
            ExportCancellation::default(),
        );
        let first = serde_json::to_string(&settings).expect("settings serialize");
        let second = serde_json::to_string(&settings).expect("settings serialize");
        assert_eq!(first, second, "two serializations are byte-identical");
        assert!(
            !first.contains("cancellation"),
            "the runtime token is not a setting: {first}"
        );

        let restored: ExportSettings = serde_json::from_str(&first).expect("settings deserialize");
        assert_eq!(restored.fps, settings.fps);
        assert_eq!(restored.resolution, settings.resolution);
        assert_eq!(restored.delivery_color, settings.delivery_color);
        assert_eq!(restored.video_codec, settings.video_codec);
        assert_eq!(restored.audio_codec, settings.audio_codec);
        assert_eq!(restored.video_bitrate, settings.video_bitrate);
        assert_eq!(restored.audio_bitrate, settings.audio_bitrate);
        // The cancellation token is reconstructed as a fresh, uncancelled one.
        assert!(!restored.cancellation.is_cancelled());
        settings.cancellation.cancel();
        assert!(!restored.cancellation.is_cancelled());

        assert_eq!(restored.delivery_color.bit_depth, depth.color_bit_depth());
    }

    // `DeliveryEncodeDepth` defaults to the 8-bit lane, so a pre-CC6 value
    // deserializes to what it meant.
    assert_eq!(DeliveryEncodeDepth::default(), DeliveryEncodeDepth::Eight);
    assert_eq!(
        serde_json::from_str::<DeliveryEncodeDepth>("\"eight\"").unwrap(),
        DeliveryEncodeDepth::Eight
    );
    assert_eq!(
        serde_json::from_str::<DeliveryEncodeDepth>("\"ten\"").unwrap(),
        DeliveryEncodeDepth::Ten
    );
    assert_eq!(DeliveryEncodeDepth::Eight.as_str(), "eight");
    assert_eq!(DeliveryEncodeDepth::Ten.as_str(), "ten");
    assert_eq!(DeliveryEncodeDepth::Eight.bits(), 8);
    assert_eq!(DeliveryEncodeDepth::Ten.bits(), 10);
    assert_eq!(DeliveryEncodeDepth::Eight.pixel_format(), "yuv420p");
    assert_eq!(DeliveryEncodeDepth::Ten.pixel_format(), "yuv420p10le");
    assert_eq!(
        DeliveryEncodeDepth::Ten.color_bit_depth(),
        ColorBitDepth::Ten
    );

    // A conformance report recorded before CC6 carries no `delivery_bit_depth`
    // and deserializes as the 8-bit lane.
    let report = kinewright_core::delivery_conformance(
        &document,
        DeliveryProfile::SourceMaster,
        DeliveryEncodeDepth::Ten,
        50,
        50,
    )
    .expect("the fixture conforms");
    assert_eq!(report.delivery_bit_depth, DeliveryEncodeDepth::Ten);
    let mut value = serde_json::to_value(&report).expect("the report serializes");
    value
        .as_object_mut()
        .expect("the report is an object")
        .remove("delivery_bit_depth");
    let legacy: kinewright_core::DeliveryConformanceReport =
        serde_json::from_value(value).expect("a pre-CC6 report still loads");
    assert_eq!(legacy.delivery_bit_depth, DeliveryEncodeDepth::Eight);

    // The four `DeliveryProfile` wire strings are byte-identical. Note that
    // the crate has carried two spellings since CC0: `as_str` is the agent and
    // manifest vocabulary the contract names, while serde's `rename_all =
    // "snake_case"` produces `youtube1080p` for the one profile whose variant
    // ends in a digit. CC6 changes neither, and both are pinned here so a
    // future edit to either cannot pass unnoticed.
    let named: Vec<&str> = DeliveryProfile::ALL
        .iter()
        .map(|profile| profile.as_str())
        .collect();
    assert_eq!(
        named,
        vec![
            "source_master",
            "youtube_1080p",
            "vertical_short",
            "square_social",
        ]
    );
    let wire: Vec<String> = DeliveryProfile::ALL
        .iter()
        .map(|profile| serde_json::to_string(profile).expect("profiles serialize"))
        .collect();
    assert_eq!(
        wire,
        vec![
            "\"source_master\"",
            "\"youtube1080p\"",
            "\"vertical_short\"",
            "\"square_social\"",
        ]
    );

    // The QC report round-trips as data, so the agent envelope and the
    // manifest see the same shape.
    let qc = measure_color_qc(&cc6_qc_raster(), &range_request(None)).expect("the raster measures");
    let encoded = serde_json::to_string(&qc).expect("the report serializes");
    assert_eq!(encoded, serde_json::to_string(&qc).unwrap());
    let restored: kinewright_core::ColorQcReport =
        serde_json::from_str(&encoded).expect("the report deserializes");
    assert_eq!(restored, qc);
}

// ---------------------------------------------------------------------------
// §11.2.14 (the pure half; the GPU and z-order halves are the media crate's)
// ---------------------------------------------------------------------------

/// A deterministic [`Analysis`] double that renders from a gain table.
///
/// It has no GPU, no decoder, and no clock. Its working proof is
/// `base · product(gain(effect))` over every colour node present in the
/// document, so removing a node has an effect the fixture can hand-compute,
/// and a node whose gain is exactly `1.0` genuinely produces a zero delta
/// rather than one asserted by construction.
struct GainAnalysis {
    base: f32,
    gains: BTreeMap<EffectId, f32>,
    full_resolution: bool,
    renders: std::sync::atomic::AtomicU32,
    visual_results: Receiver<VisualAssetResult>,
}

impl GainAnalysis {
    fn new(base: f32, gains: BTreeMap<EffectId, f32>) -> Self {
        let (_sender, visual_results) = crossbeam_channel::unbounded();
        Self {
            base,
            gains,
            full_resolution: true,
            renders: std::sync::atomic::AtomicU32::new(0),
            visual_results,
        }
    }

    fn render_count(&self) -> u32 {
        self.renders.load(std::sync::atomic::Ordering::Acquire)
    }
}

impl Analysis for GainAnalysis {
    fn probe(&self, _path: &Path) -> Result<MediaAsset, MediaError> {
        Err(MediaError::NotImplemented)
    }

    fn thumbnail_at(&self, _t: TimeCode, _max_w: u32) -> Result<RgbaImage, MediaError> {
        Err(MediaError::NotImplemented)
    }

    fn working_proof_for_document(
        &self,
        document: Arc<Document>,
        _at: TimeCode,
    ) -> Result<WorkingProof, MediaError> {
        self.renders
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let mut value = self.base;
        for track in &document.tracks {
            for clip in &track.clips {
                for effect in &clip.effects {
                    if kinewright_core::classify_color_node(effect).is_some() {
                        value *= self.gains.get(&effect.id).copied().unwrap_or(1.0);
                    }
                }
            }
        }
        Ok(working_proof(
            LinearRgbaImage {
                width: 2,
                height: 2,
                pixels: (0..4).flat_map(|_| [value, value, value, 1.0]).collect(),
            },
            self.full_resolution,
        ))
    }

    fn request_transcription(&self, _asset: MediaAsset) {}

    fn transcript_status(&self, _asset: &MediaAsset) -> TranscriptStatus {
        TranscriptStatus::NotRequested
    }

    fn timeline_transcript(
        &self,
        _document: &Document,
        _range: Option<std::ops::Range<TimeCode>>,
    ) -> Result<Vec<TimelineTranscriptWord>, MediaError> {
        Ok(Vec::new())
    }

    fn request_silence_detection(&self, _asset: MediaAsset) {}

    fn silence_status(&self, _asset: &MediaAsset) -> SilenceStatus {
        SilenceStatus::NotRequested
    }

    fn timeline_silences(
        &self,
        _document: &Document,
        _range: Option<std::ops::Range<TimeCode>>,
        _minimum_source_frames: TimeCode,
    ) -> Result<Vec<TimelineSilenceSpan>, MediaError> {
        Ok(Vec::new())
    }

    fn request_scene_detection(&self, _asset: MediaAsset) {}

    fn scene_status(&self, _asset: &MediaAsset) -> SceneStatus {
        SceneStatus::NotRequested
    }

    fn timeline_scene_changes(
        &self,
        _document: &Document,
        _range: Option<std::ops::Range<TimeCode>>,
        _minimum_confidence_basis_points: u16,
    ) -> Result<Vec<TimelineSceneChange>, MediaError> {
        Ok(Vec::new())
    }

    fn request_waveform(&self, _asset: MediaAsset, _request_generation: u64) -> bool {
        false
    }

    fn request_thumbnail(
        &self,
        _asset: MediaAsset,
        _source_at: TimeCode,
        _max_width: u32,
        _request_generation: u64,
    ) -> bool {
        false
    }

    fn visual_asset_results(&self) -> Receiver<VisualAssetResult> {
        self.visual_results.clone()
    }
}

/// One managed video asset the fixture documents reference.
fn fixture_asset() -> MediaAsset {
    MediaAsset {
        id: AssetId(1),
        path: Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
        name: "cc6".to_owned(),
        duration: TimeCode(30),
        fps: Rational::new(30, 1).unwrap(),
        kind: MediaKind::Video,
        resolution: Some((1_920, 1_080)),
        source_fingerprint: kinewright_core::MediaSourceFingerprint::default(),
        color_description: ColorContext::sdr_rec709().delivery,
    }
}

/// A managed SDR document with one video clip per requested track.
fn managed_document_with_tracks(effects_per_track: &[Vec<Effect>]) -> Document {
    let asset = fixture_asset();
    let tracks = effects_per_track
        .iter()
        .enumerate()
        .map(|(index, effects)| {
            #[allow(clippy::cast_possible_truncation)]
            let identifier = (index + 1) as u64;
            Track {
                id: TrackId(identifier),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(identifier),
                    asset: asset.id,
                    source_range: TimeCode(0)..TimeCode(30),
                    content: ClipContent::Media,
                    timeline_start: TimeCode::ZERO,
                    effects: effects.clone(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                }],
            }
        })
        .collect();
    Document {
        tracks,
        media_pool: vec![asset],
        duration: TimeCode(30),
        ..Document::default()
    }
}

/// A managed SDR document with a single empty video clip.
fn managed_document() -> Document {
    managed_document_with_tracks(&[Vec::new()])
}

/// A `primary_correction` node, which has no `bypass` control and is therefore
/// always active: the node a bypass-based method could never attribute.
fn primary_node(id: u64) -> Effect {
    Effect {
        id: EffectId(id),
        name: "primary_correction".to_owned(),
        parameters: BTreeMap::new(),
        keyframes: BTreeMap::new(),
    }
}

/// A `color_wheels` node whose controls are all neutral, so
/// `color_node_inactive_reason` reports it inactive.
fn inactive_wheels_node(id: u64) -> Effect {
    Effect {
        id: EffectId(id),
        name: "color_wheels".to_owned(),
        parameters: BTreeMap::new(),
        keyframes: BTreeMap::new(),
    }
}

/// A `color_wheels` node with a real gain, so it is active.
fn active_wheels_node(id: u64) -> Effect {
    Effect {
        id: EffectId(id),
        name: "color_wheels".to_owned(),
        parameters: BTreeMap::from([(
            "gain_master_thousandths".to_owned(),
            ParamValue::Integer(1_500),
        )]),
        keyframes: BTreeMap::new(),
    }
}

// One fixture per contract clause; see §11.2.14.
#[allow(clippy::too_many_lines)]
#[test]
fn cc6_per_node_contribution_attributes_clipping_to_the_node_that_causes_it() {
    // Node A is a neutral `primary_correction`; node B is the `color_wheels`
    // node carrying the clipping gain. The base is 0.9, so with B the raster
    // encodes over 1.0 and without it does not.
    let neutral = primary_node(1);
    let clipping = active_wheels_node(2);
    let document = Arc::new(managed_document_with_tracks(&[vec![
        neutral.clone(),
        clipping.clone(),
    ]]));
    let before = document.as_ref().clone();
    let analysis = GainAnalysis::new(
        0.9,
        BTreeMap::from([(EffectId(1), 1.0), (EffectId(2), 1.5)]),
    );
    let request = range_request(None);

    // Hand-computed: 0.9 x 1.5 = 1.35 encodes to e > 1 on all three channels,
    // so all four pixels clamp; 0.9 alone encodes to e < 1, so none do.
    assert!(reference_encode(1.35) > 1.0);
    assert!(reference_encode(0.9) < 1.0);

    let contributions =
        measure_node_contributions(&analysis, Arc::clone(&document), TimeCode::ZERO, &request)
            .expect("the double renders");
    assert_eq!(contributions.attribution, NODE_ATTRIBUTION_REMOVED);
    assert_eq!(contributions.baseline_range_basis_points, 10_000);
    assert_eq!(contributions.baseline_gamut_basis_points, 0);
    assert_eq!(contributions.considered_node_count, 2);
    assert!(!contributions.truncated);
    assert_eq!(contributions.nodes.len(), 2);

    // A `primary_correction` node is attributed normally by removal, which is
    // the whole reason removal is the method: it has no `bypass` parameter.
    let node_a = &contributions.nodes[0];
    assert_eq!(node_a.effect, EffectId(1));
    assert_eq!(node_a.node_kind, "primary_correction");
    assert!(node_a.active);
    assert_eq!(node_a.inactive_reason, None);
    assert_eq!(node_a.range_basis_points_delta, 0);
    assert_eq!(node_a.gamut_basis_points_delta, 0);

    let node_b = &contributions.nodes[1];
    assert_eq!(node_b.effect, EffectId(2));
    assert_eq!(node_b.node_kind, "color_wheels");
    assert!(node_b.active);
    // With-all minus with-this-node-removed: 10000 - 0.
    assert_eq!(node_b.range_basis_points_delta, 10_000);
    assert_eq!(node_b.gamut_basis_points_delta, 0);

    // Clipping is not additive: the deltas are deliberately not asserted to
    // sum to the baseline, and here they happen to, which proves nothing.
    assert_eq!(analysis.render_count(), 3, "one baseline plus two scratch");

    // The live document is byte-identical afterwards.
    assert_eq!(document.as_ref(), &before);

    // An inactive node reports its reason and both deltas exactly zero — and
    // that zero is measured by actually removing it, not assumed.
    let document = Arc::new(managed_document_with_tracks(&[vec![
        inactive_wheels_node(3),
        clipping.clone(),
    ]]));
    let analysis = GainAnalysis::new(
        0.9,
        BTreeMap::from([(EffectId(3), 1.0), (EffectId(2), 1.5)]),
    );
    let contributions = measure_node_contributions(&analysis, document, TimeCode::ZERO, &request)
        .expect("the double renders");
    let inactive = &contributions.nodes[0];
    assert!(!inactive.active);
    assert_eq!(inactive.inactive_reason.as_deref(), Some("neutral"));
    assert_eq!(inactive.range_basis_points_delta, 0);
    assert_eq!(inactive.gamut_basis_points_delta, 0);

    // Ordering is track, then clip, then effect-chain order, and it is core's
    // own: nothing here consults the compositor.
    let document = Arc::new(managed_document_with_tracks(&[
        vec![primary_node(1), primary_node(2)],
        vec![primary_node(3)],
        vec![primary_node(4), primary_node(5)],
    ]));
    let analysis = GainAnalysis::new(0.5, BTreeMap::new());
    let contributions =
        measure_node_contributions(&analysis, Arc::clone(&document), TimeCode::ZERO, &request)
            .expect("the double renders");
    let order: Vec<(u64, u64)> = contributions
        .nodes
        .iter()
        .map(|node| (node.clip.0, node.effect.0))
        .collect();
    assert_eq!(order, vec![(1, 1), (1, 2), (2, 3), (3, 4), (3, 5)]);

    // Seventeen candidates truncate to sixteen in the stated order. The
    // per-clip colour-node limit is sixteen, so seventeen needs two clips.
    let first: Vec<Effect> = (1..=9).map(primary_node).collect();
    let second: Vec<Effect> = (10..=17).map(primary_node).collect();
    let document = Arc::new(managed_document_with_tracks(&[first, second]));
    let analysis = GainAnalysis::new(0.5, BTreeMap::new());
    let contributions =
        measure_node_contributions(&analysis, Arc::clone(&document), TimeCode::ZERO, &request)
            .expect("the double renders");
    assert_eq!(contributions.considered_node_count, 17);
    assert!(contributions.truncated);
    assert_eq!(contributions.nodes.len(), MAX_QC_NODE_CONTRIBUTIONS);
    let effects: Vec<u64> = contributions
        .nodes
        .iter()
        .map(|node| node.effect.0)
        .collect();
    assert_eq!(effects, (1..=16).collect::<Vec<_>>());
    // Seventeen scratch renders never happen: the bound is a cost bound.
    assert_eq!(analysis.render_count(), 17, "one baseline plus sixteen");

    // The truncation is reported, in core, as one Info exception.
    let mut report = measure_color_qc(
        &analysis
            .working_proof_for_document(Arc::clone(&document), TimeCode::ZERO)
            .expect("the double renders"),
        &request,
    )
    .expect("the double's raster measures");
    kinewright_core::attach_node_contributions(&mut report, contributions);
    let exception = report
        .exceptions
        .iter()
        .find(|exception| exception.code == "qc_per_node_truncated")
        .expect("a truncated list states the omission");
    assert_eq!(exception.severity, QaSeverity::Info);
    assert_eq!(exception.observed.as_deref(), Some("17"));
    assert_eq!(exception.allowed.as_deref(), Some("16"));
    assert!(report.technical_pass);
    assert_eq!(
        report.nodes.as_ref().map(|nodes| nodes.nodes.len()),
        Some(16)
    );

    // Passing neighbour: sixteen candidates are not truncated and raise no
    // exception, so the check is known to be able to stay silent.
    let document = Arc::new(managed_document_with_tracks(&[
        (1..=9).map(primary_node).collect(),
        (10..=16).map(primary_node).collect(),
    ]));
    let analysis = GainAnalysis::new(0.5, BTreeMap::new());
    let contributions = measure_node_contributions(&analysis, document, TimeCode::ZERO, &request)
        .expect("the double renders");
    assert_eq!(contributions.considered_node_count, 16);
    assert!(!contributions.truncated);

    // The node budget is refused before any render happens.
    let document = Arc::new(managed_document_with_tracks(&[vec![primary_node(1)]]));
    let analysis = GainAnalysis::new(0.5, BTreeMap::new());
    let budget = ColorQcRequest {
        max_nodes: 0,
        ..range_request(None)
    };
    let error = measure_node_contributions(&analysis, document, TimeCode::ZERO, &budget)
        .expect_err("an out-of-range budget refuses");
    // CC6 errata E32: the refusal travels structurally as MediaError::ColorQc,
    // so a caller recovers the code without parsing the rendered message.
    let MediaError::ColorQc(ref refusal) = error else {
        panic!("a node budget refusal must arrive as MediaError::ColorQc, not {error:?}")
    };
    assert_eq!(refusal.code(), "color_qc_node_budget_exceeded");
    assert_eq!(error.recovery_code(), Some("color_qc_node_budget_exceeded"));
    assert!(error.to_string().contains("color_qc_node_budget_exceeded"));
    assert_eq!(analysis.render_count(), 0);
}

// ---------------------------------------------------------------------------
// §6.2/§6.3: the sampling rule and the per-lane budgets.
//
// Not a numbered §11.2 fixture — the numbered ones for these live with the
// encoded round trip in the media crate — but the sampling rule and
// `DeliveryBudgets::for_depth` are core code and rule 11.0.5 applies to them.
// ---------------------------------------------------------------------------

#[test]
fn cc6_delivery_verification_sampling_is_the_closed_form_integer_rule() {
    let request = |frame_count: u8| kinewright_core::DeliveryVerificationRequest {
        frame_count,
        budgets: kinewright_core::DeliveryBudgets::for_depth(DeliveryEncodeDepth::Eight),
        expected_delivery: ColorContext::sdr_rec709().delivery,
    };

    // On the CC6 source (T = 60, n = 5) the samples are 0, 14, 29, 44, 59:
    // `f_i = floor(i · (T − 1) / (n − 1))`, hand-evaluated.
    assert_eq!(
        request(kinewright_core::DELIVERY_VERIFICATION_FRAME_COUNT).sample_frames(60),
        vec![0, 14, 29, 44, 59]
    );
    for (index, expected) in [0_u64, 14, 29, 44, 59].into_iter().enumerate() {
        assert_eq!(u64::try_from(index).unwrap() * 59 / 4, expected);
    }
    assert_eq!(kinewright_core::DELIVERY_VERIFICATION_FRAME_COUNT, 5);
    assert_eq!(kinewright_core::DELIVERY_VERIFICATION_MAX_FRAMES, 16);

    // `n == 1` samples frame 0 only, and says so rather than claiming both
    // ends.
    assert_eq!(request(1).sample_frames(60), vec![0]);
    // `T <= n` samples every frame.
    assert_eq!(request(5).sample_frames(3), vec![0, 1, 2]);
    // For `n >= 2` the sample always includes frame 0 and frame T-1.
    for frames in [2_u64, 7, 60, 601] {
        for count in 2..=kinewright_core::DELIVERY_VERIFICATION_MAX_FRAMES {
            let sampled = request(count).sample_frames(frames);
            assert_eq!(sampled.first(), Some(&0));
            assert_eq!(sampled.last(), Some(&(frames - 1)));
            assert!(sampled.len() <= usize::from(count));
            // Deterministic, ascending, and duplicate-free.
            assert!(sampled.windows(2).all(|pair| pair[0] < pair[1]));
            assert_eq!(sampled, request(count).sample_frames(frames));
        }
    }
    // An empty file samples nothing rather than inventing frame 0.
    assert!(request(5).sample_frames(0).is_empty());

    // The two lanes' budgets are separately baselined, never scaled from each
    // other: scaling the 8-bit numbers by four would reuse a compositor
    // tolerance as a codec tolerance.
    let eight = kinewright_core::DeliveryBudgets::for_depth(DeliveryEncodeDepth::Eight);
    let ten = kinewright_core::DeliveryBudgets::for_depth(DeliveryEncodeDepth::Ten);
    // Every value re-baselined against `cc6_delivery_source()`'s own
    // measurement before the fixture landed (§6.3), not widened afterwards to
    // make a red build green; the media fixtures assert the >= 2x margin each
    // one keeps.
    assert_eq!(eight.luma_max_code, 8);
    assert_eq!(eight.luma_p99_code_millionths, 3_000_000);
    assert_eq!(eight.luma_mean_code_millionths, 400_000);
    assert_eq!(eight.rgb_mean_code_millionths, 1_750_000);
    assert_eq!(eight.psnr_floor_db_hundredths, 3_300);
    assert_eq!(ten.luma_max_code, 16);
    assert_eq!(ten.luma_p99_code_millionths, 4_000_000);
    assert_eq!(ten.luma_mean_code_millionths, 1_000_000);
    // The 10-bit lane's RGB mean budget is *tighter*, not four times looser.
    // Both are 8-bit-equivalent (§6.3), which is the unit the comparison
    // reports them in, so the two numbers are directly comparable.
    assert_eq!(ten.rgb_mean_code_millionths, 1_000_000);
    assert!(ten.rgb_mean_code_millionths < eight.rgb_mean_code_millionths);
    assert_eq!(ten.psnr_floor_db_hundredths, 3_300);
    // Not a scaling of the 8-bit lane by `s = 4` in any term (§6.3: the
    // 10-bit budget is baselined, never derived).
    assert_ne!(ten.luma_max_code, eight.luma_max_code * 4);
    assert_ne!(
        ten.luma_p99_code_millionths,
        eight.luma_p99_code_millionths * 4
    );
    assert_ne!(
        ten.luma_mean_code_millionths,
        eight.luma_mean_code_millionths * 4
    );
    assert_ne!(eight, ten);
    assert_eq!(kinewright_core::DECODED_RANGE_EXCEPTION_BASIS_POINTS, 100);

    // The RGB extremes note travels with the numbers it explains.
    assert!(kinewright_core::DELIVERY_RGB_EXTREMES_NOTE.contains("evidence, not a gate"));
}

// ---------------------------------------------------------------------------
// §3.1/§3.4 (core review round): non-finite samples, the strict legal bounds
// measured through the engine, and the refusals and orderings around them.
//
// Not numbered §11.2 fixtures — rule 11.0.5 covers them as core code — but
// every expected value below is still transcribed independently.
// ---------------------------------------------------------------------------

/// A range-and-gamut request at one delivery lane.
fn depth_request(depth: DeliveryEncodeDepth) -> ColorQcRequest {
    ColorQcRequest {
        delivery_bit_depth: depth,
        ..range_request(None)
    }
}

/// An opaque `n`-pixel row of the given linear triples.
fn row_proof(pixels: &[[f32; 3]]) -> WorkingProof {
    let mut samples = Vec::with_capacity(pixels.len() * 4);
    for [red, green, blue] in pixels {
        samples.extend([*red, *green, *blue, 1.0]);
    }
    working_proof(
        LinearRgbaImage {
            #[allow(clippy::cast_possible_truncation)]
            width: pixels.len() as u32,
            height: 1,
            pixels: samples,
        },
        true,
    )
}

/// The one `color_qc_non_finite_sample` exception a report carries, if any.
fn non_finite_exception(
    report: &kinewright_core::ColorQcReport,
) -> Option<&kinewright_core::ColorQcException> {
    report
        .exceptions
        .iter()
        .find(|exception| exception.code == "color_qc_non_finite_sample")
}

/// Grey `linear = 0.5` predicts `Y = 16 + 219·e(0.5)` with `Cb = Cr = 128`.
///
/// `e(0.5) = 1.099·0.5^0.45 − 0.099 = 0.705515`, so
/// `Y = 16 + 219·0.705515 = 170.5078`, which is 17051 hundredths after the
/// half-away-from-zero rounding. Transcribed here, not measured.
const GREY_HALF_LUMA_HUNDREDTHS: i64 = 17_051;
const NEUTRAL_CHROMA_HUNDREDTHS: i64 = 12_800;

#[test]
fn cc6_non_finite_samples_are_counted_and_never_classified() {
    // The transcription this fixture pins its plane codes against.
    let encoded = reference_encode(0.5);
    assert!((encoded - 0.705_515_089_922_121_2).abs() < SPEC_F64_TOLERANCE);
    let luma_code = 16.0 + 219.0 * encoded;
    assert!((luma_code - 170.507_804_688_694_5).abs() < SPEC_F64_TOLERANCE);
    #[allow(clippy::cast_possible_truncation)]
    let transcribed = (luma_code * 100.0).round() as i64;
    assert_eq!(transcribed, GREY_HALF_LUMA_HUNDREDTHS);

    // (a) One NaN pixel beside one finite neighbour.
    let tripped = row_proof(&[[f32::NAN, 0.5, 0.5], [0.5, 0.5, 0.5]]);
    let report = measure_color_qc(&tripped, &range_request(None)).expect("the raster measures");
    assert_eq!(report.visible_pixel_count, 2);
    assert_eq!(report.non_finite_pixel_count, 1);
    assert_eq!(report.region.non_finite_pixel_count, 1);
    assert_eq!(report.transparent_pixel_count, 0);

    // The pixel is counted, and it reaches no accumulator: every classified
    // count belongs to the finite neighbour alone.
    assert_eq!(report.range.clamped_pixel_count, 0);
    assert_eq!(report.range.red.over_pixel_count, 0);
    assert_eq!(report.range.red.under_pixel_count, 0);
    assert_eq!(report.gamut.out_of_gamut_pixel_count, 0);
    assert_eq!(report.gamut.minimum_linear_millionths, 0);
    let luma = report.range.predicted_ycbcr.luma;
    assert!(luma.samples_seen());
    assert_eq!(luma.minimum_code_hundredths, GREY_HALF_LUMA_HUNDREDTHS);
    assert_eq!(luma.maximum_code_hundredths, GREY_HALF_LUMA_HUNDREDTHS);
    assert_eq!(
        report.range.predicted_ycbcr.cb.minimum_code_hundredths,
        NEUTRAL_CHROMA_HUNDREDTHS
    );
    assert_eq!(luma.below_count, 0);
    assert_eq!(luma.above_count, 0);

    // It is an Error, and an Error clears `technical_pass`.
    let exception = non_finite_exception(&report).expect("the NaN pixel is reported");
    assert_eq!(exception.severity, QaSeverity::Error);
    assert_eq!(exception.field.as_deref(), Some("non_finite_pixel_count"));
    assert_eq!(exception.observed.as_deref(), Some("1"));
    assert_eq!(exception.allowed.as_deref(), Some("0"));
    assert!(exception.message.contains("non-finite"));
    assert!(!report.technical_pass);

    // (b) The finite neighbour on its own passes, so the Error above is the
    // NaN and nothing else.
    let clean = row_proof(&[[0.5, 0.5, 0.5], [0.5, 0.5, 0.5]]);
    let report = measure_color_qc(&clean, &range_request(None)).expect("the raster measures");
    assert_eq!(report.non_finite_pixel_count, 0);
    assert_eq!(non_finite_exception(&report), None);
    assert!(report.technical_pass);
    assert_eq!(
        report.range.predicted_ycbcr.luma.maximum_code_hundredths,
        GREY_HALF_LUMA_HUNDREDTHS
    );

    // (c) A NaN with no finite neighbour anywhere: the planes saw nothing, and
    // an unseen plane reports the empty interval rather than a fabricated
    // `minimum_code_hundredths = 0` that reads as a legal black.
    let only_nan = single_pixel_proof([f32::NAN, f32::NAN, f32::NAN]);
    let report = measure_color_qc(&only_nan, &range_request(None)).expect("the raster measures");
    assert_eq!(report.visible_pixel_count, 1);
    assert_eq!(report.non_finite_pixel_count, 1);
    assert!(!report.technical_pass);
    for plane in [
        report.range.predicted_ycbcr.luma,
        report.range.predicted_ycbcr.cb,
        report.range.predicted_ycbcr.cr,
    ] {
        assert!(!plane.samples_seen());
        assert_eq!(
            plane.minimum_code_hundredths,
            kinewright_core::PlaneLegalExcursion::UNSEEN_MINIMUM_CODE_HUNDREDTHS
        );
        assert_eq!(
            plane.maximum_code_hundredths,
            kinewright_core::PlaneLegalExcursion::UNSEEN_MAXIMUM_CODE_HUNDREDTHS
        );
        assert_eq!(plane.below_count, 0);
        assert_eq!(plane.above_count, 0);
    }

    // (d) `+inf`, which used to saturate the plane extreme to `i64::MAX` and
    // be counted as an over-range pixel on the strength of a comparison
    // against infinity.
    for sample in [f32::INFINITY, f32::NEG_INFINITY] {
        let infinite = single_pixel_proof([sample, 0.5, 0.5]);
        let report =
            measure_color_qc(&infinite, &range_request(None)).expect("the raster measures");
        assert_eq!(report.non_finite_pixel_count, 1);
        assert_eq!(report.range.red.over_pixel_count, 0);
        assert_eq!(report.range.red.under_pixel_count, 0);
        assert_eq!(report.range.red.maximum_over_excursion_millionths, 0);
        assert_eq!(report.range.red.minimum_under_excursion_millionths, 0);
        assert_eq!(report.range.clamped_pixel_count, 0);
        assert_eq!(report.gamut.out_of_gamut_pixel_count, 0);
        let plane = report.range.predicted_ycbcr.luma;
        assert!(!plane.samples_seen());
        assert_ne!(plane.maximum_code_hundredths, i64::MAX);
        assert!(!report.technical_pass);
    }

    // (e) A NaN alpha is not visible: `NaN <= 0.0` is false, so the old
    // spelling of the visibility test called it visible and then measured it.
    let mut nan_alpha = single_pixel_proof([0.5, 0.5, 0.5]);
    nan_alpha.image.pixels[3] = f32::NAN;
    let error = measure_color_qc(&nan_alpha, &range_request(None))
        .expect_err("a raster with no visible pixel refuses");
    assert_eq!(error.code(), "color_qc_region_empty");
    assert!(error.observed().contains("0 visible pixels"));
}

#[test]
fn cc6_ycbcr_legal_bounds_are_strict_through_the_measurement() {
    // Rec.709 red at exactly the ceiling. `e(1.0) = 1.099 · 1^0.45 − 0.099 =
    // 1.000000`, and for a red-only pixel `Y' = Kr·R'`, so
    // `Cr = 128·s + 224·s·(1 − Kr)/1.5748 = 128·s + 112·s·R' = 240·s` exactly.
    // The comparison is strict, so the bound itself is legal.
    for (depth, ceiling) in [
        (DeliveryEncodeDepth::Eight, 24_000_i64),
        (DeliveryEncodeDepth::Ten, 96_000),
    ] {
        let report = measure_color_qc(&single_pixel_proof([1.0, 0.0, 0.0]), &depth_request(depth))
            .expect("one pixel measures");
        let cr = report.range.predicted_ycbcr.cr;
        assert_eq!(report.range.predicted_ycbcr.bit_depth, depth.bits());
        assert!(cr.samples_seen());
        assert_eq!(cr.maximum_code_hundredths, ceiling);
        assert_eq!(cr.above_count, 0);
        assert_eq!(cr.above_basis_points, 0);
    }

    // Yellow at exactly the floor: `Y' = (Kr + Kg)·v` with `B' = 0`, so
    // `Cb = 128·s − 112·s·v = 16·s` at `v = 1`.
    for (depth, floor) in [
        (DeliveryEncodeDepth::Eight, 1_600_i64),
        (DeliveryEncodeDepth::Ten, 6_400),
    ] {
        let report = measure_color_qc(&single_pixel_proof([1.0, 1.0, 0.0]), &depth_request(depth))
            .expect("one pixel measures");
        let cb = report.range.predicted_ycbcr.cb;
        assert!(cb.samples_seen());
        assert_eq!(cb.minimum_code_hundredths, floor);
        assert_eq!(cb.below_count, 0);
        assert_eq!(cb.below_basis_points, 0);
    }

    // The failing neighbours, one code away. `e(1.02) = 1.009837`, so the
    // red-only pixel predicts `Cr = 128 + 112·1.009837 = 241.10` and the
    // yellow pixel predicts `Cb = 128 − 112·1.009837 = 14.90`: one code past
    // each bound, and each one counts.
    let neighbour_encoded = reference_encode(1.02);
    assert!((neighbour_encoded - 1.009_837_150_573_730_5).abs() < SPEC_F64_TOLERANCE);
    assert!((128.0 + 112.0 * neighbour_encoded - 241.101_760_864_257_8).abs() < 1e-4);

    let over = measure_color_qc(
        &single_pixel_proof([1.02, 0.0, 0.0]),
        &depth_request(DeliveryEncodeDepth::Eight),
    )
    .expect("one pixel measures");
    let cr = over.range.predicted_ycbcr.cr;
    assert_eq!(cr.above_count, 1);
    assert_eq!(cr.above_basis_points, 10_000);
    assert_eq!(cr.maximum_code_hundredths, 24_110);

    let under = measure_color_qc(
        &single_pixel_proof([1.02, 1.02, 0.0]),
        &depth_request(DeliveryEncodeDepth::Eight),
    )
    .expect("one pixel measures");
    let cb = under.range.predicted_ycbcr.cb;
    assert_eq!(cb.below_count, 1);
    assert_eq!(cb.below_basis_points, 10_000);
    assert_eq!(cb.minimum_code_hundredths, 1_490);
}

/// The two delivery lanes are the whole domain, and a third depth is a
/// development fault rather than evidence.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "8-bit or 10-bit delivery depth")]
fn cc6_bt709_limited_ycbcr_refuses_a_depth_that_is_not_a_delivery_lane() {
    let _ = bt709_limited_ycbcr([0.5, 0.5, 0.5], 12);
}

#[test]
fn cc6_exceptions_sort_by_severity_then_code_then_field() {
    // One over pixel, one under pixel, one NaN. Each channel is over once and
    // under once out of three visible pixels, which is 3333 basis points and
    // well past the 10 basis-point threshold, so all six range warnings and
    // the gamut warning are raised together with the non-finite Error.
    let proof = row_proof(&[
        [1.2, 1.2, 1.2],
        [-0.1, -0.1, -0.1],
        [f32::NAN, f32::NAN, f32::NAN],
    ]);
    let report = measure_color_qc(&proof, &range_request(None)).expect("the raster measures");
    assert_eq!(report.visible_pixel_count, 3);
    assert_eq!(report.non_finite_pixel_count, 1);
    assert_eq!(report.range.red.over_basis_points, 3_333);
    assert_eq!(reference_basis_points(1, 3), 3_333);

    let order: Vec<(&str, &str)> = report
        .exceptions
        .iter()
        .map(|exception| {
            (
                exception.code.as_str(),
                exception.field.as_deref().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        order,
        vec![
            ("color_qc_non_finite_sample", "non_finite_pixel_count"),
            ("delivery_gamut_excursion", "out_of_gamut_basis_points"),
            ("delivery_range_excursion", "blue.over_basis_points"),
            ("delivery_range_excursion", "blue.under_basis_points"),
            ("delivery_range_excursion", "green.over_basis_points"),
            ("delivery_range_excursion", "green.under_basis_points"),
            ("delivery_range_excursion", "red.over_basis_points"),
            ("delivery_range_excursion", "red.under_basis_points"),
        ],
        "severity descending, then code ascending, then the field tiebreak"
    );
    assert!(!report.technical_pass);
}

#[test]
fn cc6_delivery_verification_refuses_a_frame_count_outside_the_sampled_range() {
    let request = |frame_count: u8| kinewright_core::DeliveryVerificationRequest {
        frame_count,
        budgets: kinewright_core::DeliveryBudgets::for_depth(DeliveryEncodeDepth::Eight),
        expected_delivery: ColorContext::sdr_rec709().delivery,
    };

    for count in [0_u8, 17, 255] {
        let error = request(count)
            .validate()
            .expect_err("a frame count outside 1..=16 refuses");
        assert_eq!(
            error.code(),
            "delivery_verification_frame_count_out_of_range"
        );
        assert_eq!(error.field(), "frame_count");
        assert_eq!(error.observed(), count.to_string());
        assert_eq!(error.allowed_values(), "1..=16");
        let message = error.actionable_message();
        for expected in ["field=frame_count", "allowed=1..=16"] {
            assert!(message.contains(expected), "{message}");
        }
        // The reason the refusal exists: sampling stays total and silently
        // measures a different number of frames than the one requested.
        let clamped = request(count).sample_frames(60);
        assert_eq!(clamped.len(), if count == 0 { 1 } else { 16 });
    }

    // Passing neighbours on both edges, and the default.
    for count in [
        1_u8,
        kinewright_core::DELIVERY_VERIFICATION_FRAME_COUNT,
        kinewright_core::DELIVERY_VERIFICATION_MAX_FRAMES,
    ] {
        assert!(request(count).validate().is_ok());
    }
}

/// One video track carrying the given `(clip id, timeline start)` clips, each
/// with a 15-frame duration and one `primary_correction` node whose effect id
/// is the clip id.
fn track_document(clips: &[(u64, TimeCode)]) -> Document {
    let asset = fixture_asset();
    let clips = clips
        .iter()
        .map(|(id, start)| Clip {
            id: ClipId(*id),
            asset: asset.id,
            source_range: TimeCode(0)..TimeCode(15),
            content: ClipContent::Media,
            timeline_start: *start,
            effects: vec![primary_node(*id)],
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
        })
        .collect();
    Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips,
        }],
        media_pool: vec![asset],
        duration: TimeCode(30),
        ..Document::default()
    }
}

#[test]
fn cc6_per_node_candidates_find_the_on_screen_clip_whatever_the_clip_order() {
    let request = range_request(None);
    // The ordinary path: two clips in timeline order, each attributed on its
    // own frames and neither attributed on the other's.
    let sorted = Arc::new(track_document(&[(1, TimeCode::ZERO), (2, TimeCode(15))]));
    for (at, expected_clip, expected_effect) in [
        (TimeCode::ZERO, ClipId(1), EffectId(1)),
        (TimeCode(14), ClipId(1), EffectId(1)),
        (TimeCode(15), ClipId(2), EffectId(2)),
        (TimeCode(20), ClipId(2), EffectId(2)),
    ] {
        let analysis = GainAnalysis::new(0.5, BTreeMap::new());
        let contributions =
            measure_node_contributions(&analysis, Arc::clone(&sorted), at, &request)
                .expect("the double renders");
        assert_eq!(
            contributions.considered_node_count, 1,
            "exactly the on-screen clip's colour nodes are candidates at {at:?}"
        );
        assert_eq!(contributions.nodes[0].clip, expected_clip);
        assert_eq!(contributions.nodes[0].effect, expected_effect);
    }
    // Past the last clip there is genuinely nothing on screen.
    let analysis = GainAnalysis::new(0.5, BTreeMap::new());
    let contributions =
        measure_node_contributions(&analysis, Arc::clone(&sorted), TimeCode(30), &request)
            .expect("the double renders");
    assert_eq!(contributions.considered_node_count, 0);
    assert!(contributions.nodes.is_empty());

    // The same two clips listed out of timeline order. The editing operations
    // refuse such a document — `OpError::ClipsUnsorted` — so the invariant
    // holds for anything that reached the tree through an operation, but the
    // type does not enforce it and a fixture or a deserialized document can
    // present this order. Scanning with an early `break` reported the track as
    // carrying no colour node at all, which is a missing measurement dressed
    // as a clean one; finding the on-screen clip surfaces the document model's
    // own refusal instead.
    let unsorted = Arc::new(track_document(&[(2, TimeCode(15)), (1, TimeCode::ZERO)]));
    let analysis = GainAnalysis::new(0.5, BTreeMap::new());
    let error = measure_node_contributions(&analysis, unsorted, TimeCode::ZERO, &request)
        .expect_err("the on-screen clip is found, and removing from it is refused");
    let message = error.to_string();
    assert!(
        message.contains("color_qc_node_removal_rejected"),
        "{message}"
    );
    assert!(message.contains("not sorted"), "{message}");
    // CC6 §3.8 / errata E32: it is a *typed* refusal, not a flattened backend
    // string. A surface reporting it must be able to name what happened —
    // the removal was rejected — rather than falling back on "the working
    // proof was unavailable", which is not what went wrong.
    assert_eq!(
        error.recovery_code(),
        Some("color_qc_node_removal_rejected"),
        "{message}"
    );
    let MediaError::ColorQc(ColorQcError::NodeRemovalRejected { clip, effect, .. }) = &error else {
        panic!("the rejection travels as a typed colour QC refusal: {error:?}");
    };
    assert_eq!(
        (*clip, *effect),
        (ClipId(1), EffectId(1)),
        "and it names the clip and the effect it could not remove"
    );
}
