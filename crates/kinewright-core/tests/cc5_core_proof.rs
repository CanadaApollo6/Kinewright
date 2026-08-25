//! CC5 §4 matte proof and matte-scoped scope contracts owned by Core.
//!
//! These tests hold the §4.1 proof types, the §4.2 coverage statistics, and
//! the §4.3 matte-scoped scope metadata. Every expected value is derived by
//! hand from §4.2's equations in the comments below rather than read back out
//! of the implementation.

use kinewright_core::{
    ClipId, EffectId, MATTE_COVERAGE_ENCODING, MATTE_COVERAGE_HISTOGRAM_BUCKETS,
    MATTE_COVERAGE_SCALE, MATTE_SCOPE_THRESHOLD, MatteCoverageError, MatteCoverageStatistics,
    MatteProof, MatteProofError, MatteProofMetadata, MatteRegionDescription, MediaError,
    MonitorProofMetadata, MonitorProofRenderKind, NormalizedRoi, RgbaImage, ScopeComparisonError,
    ScopeError, ScopeEvidence, ScopeMeasurementMetadata, ScopeRequest, ScopeStage,
    matte_coverage_statistics, matte_scoped_frame, measure_scope,
};

/// Build a coverage raster from grey codes in row-major order.
///
/// The encoding is §4.1's: `R = G = B = round(255 · m)` with an opaque alpha.
fn coverage_image(width: u32, height: u32, codes: &[u8]) -> RgbaImage {
    let expected = usize::try_from(width).expect("test width fits")
        * usize::try_from(height).expect("test height fits");
    assert_eq!(codes.len(), expected, "test raster must be fully specified");
    let mut pixels = Vec::with_capacity(expected * 4);
    for code in codes {
        pixels.extend_from_slice(&[*code, *code, *code, 255]);
    }
    RgbaImage {
        width,
        height,
        pixels,
    }
}

/// The §9.2.9-style hand-built coverage raster: an 8 × 4 frame carrying a
/// 4 × 2 block of full coverage whose top-left pixel is `(2, 1)`, plus one
/// partial pixel of code 40 at `(6, 3)`.
fn hand_built_coverage() -> RgbaImage {
    let mut codes = [0_u8; 32];
    for y in 1..=2_usize {
        for x in 2..=5_usize {
            codes[y * 8 + x] = 255;
        }
    }
    codes[3 * 8 + 6] = 40;
    coverage_image(8, 4, &codes)
}

fn statistics(coverage: &RgbaImage) -> MatteCoverageStatistics {
    matte_coverage_statistics(coverage).expect("hand-built coverage is well formed")
}

#[test]
fn coverage_statistics_match_hand_derived_counts() {
    let stats = statistics(&hand_built_coverage());

    // 8 full pixels plus 1 partial pixel are covered out of 8 * 4 = 32.
    assert_eq!(stats.covered_pixel_count, 9);
    assert_eq!(stats.full_pixel_count, 8);
    assert_eq!(stats.partial_pixel_count, 1);
    assert_eq!(stats.total_pixel_count, 32);
    // floor(9 * 10000 / 32) = floor(2812.5) = 2812.
    assert_eq!(stats.covered_basis_points, 2812);
    assert!(stats.weighted_by_coverage);
}

#[test]
fn coverage_histogram_uses_the_cc2_bucketing_rule() {
    let stats = statistics(&hand_built_coverage());

    // bucket = min(15, floor(code * 16 / 256)): code 0 -> 0, code 40 -> 2,
    // code 255 -> 15.  Every pixel is counted, including the uncovered ones.
    let mut expected = [0_u64; MATTE_COVERAGE_HISTOGRAM_BUCKETS];
    expected[0] = 23;
    expected[2] = 1;
    expected[15] = 8;
    assert_eq!(stats.coverage_histogram, expected);
    assert_eq!(
        stats.coverage_histogram.iter().sum::<u64>(),
        stats.total_pixel_count
    );
}

#[test]
fn coverage_bounding_box_uses_the_roi_floor_ceil_rule() {
    let stats = statistics(&hand_built_coverage());

    // Covered pixels span x in 2..=6 and y in 1..=3, so the tightest half-open
    // pixel rect is [2, 7) x [1, 4).  Converted on an 8 x 4 raster:
    //   x      = floor(2 * 10000 / 8)          = 2500
    //   width  = ceil(7 * 10000 / 8)  - 2500   = 8750 - 2500 = 6250
    //   y      = floor(1 * 10000 / 4)          = 2500
    //   height = ceil(4 * 10000 / 4)  - 2500   = 10000 - 2500 = 7500
    assert_eq!(
        stats.bounding_box_basis_points,
        Some(NormalizedRoi::new(2500, 2500, 6250, 7500))
    );

    // The reported rectangle must survive CC2's ROI rasterization and still
    // contain every covered pixel.
    let pixels = stats
        .bounding_box_basis_points
        .expect("coverage is not empty")
        .to_pixels(8, 4)
        .expect("bounding box is a valid ROI");
    assert_eq!(
        (pixels.x, pixels.y, pixels.width, pixels.height),
        (2, 1, 5, 3)
    );
}

#[test]
fn coverage_centroid_is_weighted_and_rounded_half_away_from_zero() {
    let stats = statistics(&hand_built_coverage());

    // Sum m = 8 * 255 + 40 = 2080.
    // Sum m * (2x + 1) = 510 * (5 + 7 + 9 + 11) + 40 * 13 = 16320 + 520 = 16840.
    //   x = 16840 * 5000 / (8 * 2080) = 84_200_000 / 16_640 = 5060.096... -> 5060
    // Sum m * (2y + 1) = 1020 * 3 + 1020 * 5 + 40 * 7 = 3060 + 5100 + 280 = 8440.
    //   y = 8440 * 5000 / (4 * 2080) = 42_200_000 / 8_320 = 5072.115... -> 5072
    assert_eq!(stats.centroid_basis_points, Some((5060, 5072)));
}

#[test]
fn full_coverage_centroid_is_the_raster_centre() {
    // A fully covered 2 x 2 raster: sum m = 1020, sum m * (2x + 1) = 2040,
    // so x = 2040 * 5000 / (2 * 1020) = 5000 exactly, and likewise for y.
    let stats = statistics(&coverage_image(2, 2, &[255; 4]));

    assert_eq!(stats.covered_pixel_count, 4);
    assert_eq!(stats.full_pixel_count, 4);
    assert_eq!(stats.partial_pixel_count, 0);
    assert_eq!(stats.covered_basis_points, 10_000);
    assert_eq!(
        stats.bounding_box_basis_points,
        Some(NormalizedRoi::new(0, 0, 10_000, 10_000))
    );
    assert_eq!(stats.centroid_basis_points, Some((5000, 5000)));
}

#[test]
fn empty_coverage_reports_no_region() {
    let stats = statistics(&coverage_image(8, 4, &[0; 32]));

    assert_eq!(stats.covered_pixel_count, 0);
    assert_eq!(stats.full_pixel_count, 0);
    assert_eq!(stats.partial_pixel_count, 0);
    assert_eq!(stats.total_pixel_count, 32);
    assert_eq!(stats.covered_basis_points, 0);
    assert_eq!(stats.coverage_histogram[0], 32);
    assert_eq!(stats.bounding_box_basis_points, None);
    assert_eq!(stats.centroid_basis_points, None);
    assert!(stats.weighted_by_coverage);
}

#[test]
fn a_single_partial_pixel_is_covered_but_not_full() {
    // Code 1 is the whisper-of-coverage case the §4.3 threshold keeps.
    let stats = statistics(&coverage_image(2, 1, &[0, 1]));

    assert_eq!(stats.covered_pixel_count, 1);
    assert_eq!(stats.full_pixel_count, 0);
    assert_eq!(stats.partial_pixel_count, 1);
    // floor(1 * 10000 / 2) = 5000.
    assert_eq!(stats.covered_basis_points, 5000);
    assert_eq!(
        stats.bounding_box_basis_points,
        Some(NormalizedRoi::new(5000, 0, 5000, 10_000))
    );
    // The single sample sits at pixel centre ((1 + 0.5) / 2, 0.5) = (7500, 5000).
    assert_eq!(stats.centroid_basis_points, Some((7500, 5000)));
}

#[test]
fn coverage_rejects_a_non_opaque_alpha() {
    let mut coverage = coverage_image(2, 2, &[255; 4]);
    // Pixel (1, 1) is the fourth pixel; its alpha byte is index 15.
    coverage.pixels[15] = 254;

    let error = matte_coverage_statistics(&coverage).expect_err("alpha must be 255 everywhere");
    assert_eq!(
        error,
        MatteCoverageError::AlphaNotOpaque {
            x: 1,
            y: 1,
            observed: 254,
            allowed: 255,
        }
    );
    assert_eq!(error.code(), "matte_coverage_alpha_not_opaque");
}

#[test]
fn coverage_rejects_a_non_grey_pixel() {
    let mut coverage = coverage_image(2, 1, &[10, 10]);
    // Pixel (0, 0): green becomes 20 while red and blue stay 10.
    coverage.pixels[1] = 20;

    let error = matte_coverage_statistics(&coverage).expect_err("coverage must be grey");
    assert_eq!(
        error,
        MatteCoverageError::NotGrey {
            x: 0,
            y: 0,
            red: 10,
            green: 20,
            blue: 10,
            allowed: "red, green, and blue must be equal",
        }
    );
    assert_eq!(error.code(), "matte_coverage_not_grey");
}

#[test]
fn coverage_rejects_malformed_rasters() {
    let empty = RgbaImage {
        width: 0,
        height: 4,
        pixels: Vec::new(),
    };
    let error = matte_coverage_statistics(&empty).expect_err("zero width is rejected");
    assert_eq!(
        error,
        MatteCoverageError::InvalidDimensions {
            observed: "0x4".to_owned(),
            allowed: "width and height must both be non-zero",
        }
    );
    assert_eq!(error.code(), "matte_coverage_invalid_dimensions");

    let mut truncated = coverage_image(2, 2, &[255; 4]);
    truncated.pixels.truncate(12);
    let error = matte_coverage_statistics(&truncated).expect_err("short buffer is rejected");
    assert_eq!(
        error,
        MatteCoverageError::BufferLengthMismatch {
            observed: 12,
            allowed: 16,
        }
    );
    assert_eq!(error.code(), "matte_coverage_buffer_length_mismatch");
}

#[test]
fn coverage_statistics_round_trip_through_json() {
    let stats = statistics(&hand_built_coverage());
    let json = serde_json::to_string(&stats).expect("statistics serialize");
    let parsed: MatteCoverageStatistics = serde_json::from_str(&json).expect("statistics parse");
    assert_eq!(parsed, stats);
}

/// A 4 x 2 monitor frame whose pixels are all opaque and distinguishable.
fn monitor_frame() -> RgbaImage {
    let mut pixels = Vec::with_capacity(32);
    for index in 0..8_u8 {
        pixels.extend_from_slice(&[index * 10 + 1, index * 10 + 2, index * 10 + 3, 255]);
    }
    RgbaImage {
        width: 4,
        height: 2,
        pixels,
    }
}

#[test]
fn matte_scoped_frame_keeps_only_covered_pixels() {
    let frame = monitor_frame();
    // A 2 x 1 covered block at (1, 0): one full pixel and one at code 1, which
    // the pinned `m > 0` threshold keeps.
    let coverage = coverage_image(4, 2, &[0, 255, 1, 0, 0, 0, 0, 0]);

    let scoped = matte_scoped_frame(&frame, &coverage).expect("dimensions agree");
    assert_eq!(scoped.width, 4);
    assert_eq!(scoped.height, 2);
    let (scoped_pixels, _) = scoped.pixels.as_chunks::<4>();
    let alphas = scoped_pixels
        .iter()
        .map(|pixel| pixel[3])
        .collect::<Vec<_>>();
    assert_eq!(alphas, vec![0, 255, 255, 0, 0, 0, 0, 0]);
    // RGB is copied verbatim: only the analysis alpha changes.
    let (source_pixels, _) = frame.pixels.as_chunks::<4>();
    for (scoped_pixel, source_pixel) in scoped_pixels.iter().zip(source_pixels) {
        assert_eq!(scoped_pixel[0..3], source_pixel[0..3]);
    }
    // The source frame is untouched.
    assert_eq!(frame, monitor_frame());
    assert_eq!(MATTE_SCOPE_THRESHOLD, "coverage_greater_than_zero");
}

#[test]
fn the_unchanged_cc2_engine_measures_only_covered_pixels() {
    let frame = monitor_frame();
    let coverage = coverage_image(4, 2, &[0, 255, 1, 0, 0, 0, 0, 0]);
    let scoped = matte_scoped_frame(&frame, &coverage).expect("dimensions agree");

    let evidence = measure_scope(&scoped, 0, &ScopeRequest::default()).expect("scoped measurement");
    assert_eq!(evidence.metadata.roi_pixel_count, 8);
    assert_eq!(evidence.metadata.transparent_pixel_count, 6);
    assert_eq!(evidence.metadata.visible_pixel_count, 2);
    // The stage is untouched: matte scoping changes the region, not the
    // pipeline boundary.
    assert_eq!(evidence.metadata.stage, ScopeStage::MonitoringPostComposite);
    assert_eq!(evidence.metadata.matte_region, None);
}

#[test]
fn matte_scoped_frame_rejects_a_raster_mismatch() {
    let frame = monitor_frame();
    let coverage = coverage_image(2, 1, &[255, 255]);

    let error = matte_scoped_frame(&frame, &coverage).expect_err("dimensions differ");
    match error {
        ScopeError::MatteRegionRasterMismatch { observed, allowed } => {
            assert_eq!((observed.width, observed.height), (2, 1));
            assert_eq!((allowed.width, allowed.height), (4, 2));
        }
        other => panic!("expected a raster mismatch, got {other:?}"),
    }
}

fn matte_region(clip: u64, effect: u64) -> MatteRegionDescription {
    MatteRegionDescription::new(ClipId(clip), EffectId(effect), 2)
}

fn scoped_evidence(region: Option<MatteRegionDescription>) -> ScopeEvidence {
    let frame = monitor_frame();
    let coverage = coverage_image(4, 2, &[0, 255, 1, 0, 0, 0, 0, 0]);
    let scoped = matte_scoped_frame(&frame, &coverage).expect("dimensions agree");
    let mut evidence = measure_scope(&scoped, 0, &ScopeRequest::default()).expect("measurement");
    evidence.metadata.matte_region = region;
    evidence
}

#[test]
fn comparison_accepts_matching_matte_regions() {
    let reference = scoped_evidence(Some(matte_region(7, 3)));
    let candidate = scoped_evidence(Some(matte_region(7, 3)));

    let comparison = reference.compare(&candidate).expect("regions match");
    assert_eq!(comparison.stage, ScopeStage::MonitoringPostComposite);
    assert_eq!(comparison.visible_pixel_count.delta, 0);

    // Two unscoped results still compare, exactly as before CC5.
    let unscoped = scoped_evidence(None);
    unscoped
        .compare(&scoped_evidence(None))
        .expect("both unscoped");
}

#[test]
fn comparison_rejects_scoped_against_unscoped() {
    let reference = scoped_evidence(Some(matte_region(7, 3)));
    let candidate = scoped_evidence(None);

    let error = reference
        .compare(&candidate)
        .expect_err("a matte-scoped result is not comparable with an unscoped one");
    assert_eq!(
        error,
        ScopeComparisonError::MatteRegionMismatch {
            reference: Some(matte_region(7, 3)),
            candidate: None,
        }
    );

    let error = candidate
        .compare(&reference)
        .expect_err("the rejection is symmetric");
    assert_eq!(
        error,
        ScopeComparisonError::MatteRegionMismatch {
            reference: None,
            candidate: Some(matte_region(7, 3)),
        }
    );
}

#[test]
fn comparison_rejects_a_different_matte() {
    let reference = scoped_evidence(Some(matte_region(7, 3)));

    let other_clip = scoped_evidence(Some(matte_region(8, 3)));
    assert!(matches!(
        reference.compare(&other_clip),
        Err(ScopeComparisonError::MatteRegionMismatch { .. })
    ));

    let other_effect = scoped_evidence(Some(matte_region(7, 4)));
    assert!(matches!(
        reference.compare(&other_effect),
        Err(ScopeComparisonError::MatteRegionMismatch { .. })
    ));

    // The covered population is *reported*, not part of the region identity:
    // a qualifier matte's coverage depends on the colour entering the node, so
    // a before/after pair legitimately differs in count (CC5 §4.3).
    let mut other_count = scoped_evidence(Some(matte_region(7, 3)));
    let reference_count = reference
        .metadata
        .matte_region
        .as_ref()
        .expect("region was set")
        .covered_pixel_count;
    other_count
        .metadata
        .matte_region
        .as_mut()
        .expect("region was set")
        .covered_pixel_count = reference_count + 3;
    let comparison = reference
        .compare(&other_count)
        .expect("a different covered population is still comparable");
    let delta = comparison
        .matte_covered_pixel_delta
        .expect("both sides carried a matte region");
    assert_eq!(delta.delta, 3);
    assert!(
        scoped_evidence(None)
            .compare(&scoped_evidence(None))
            .expect("unscoped pairs compare")
            .matte_covered_pixel_delta
            .is_none()
    );
}

#[test]
fn matte_region_description_pins_the_threshold_token() {
    let region = matte_region(7, 3);
    assert_eq!(region.threshold, MATTE_SCOPE_THRESHOLD);
    assert_eq!(region.clip, ClipId(7));
    assert_eq!(region.effect, EffectId(3));
    assert_eq!(region.covered_pixel_count, 2);
}

#[test]
fn scope_metadata_serializes_the_matte_region_only_when_scoped() {
    let unscoped = scoped_evidence(None);
    let json = serde_json::to_value(&unscoped.metadata).expect("metadata serializes");
    assert!(
        json.get("matte_region").is_none(),
        "unscoped evidence must not carry the key: {json}"
    );
    let parsed: ScopeMeasurementMetadata =
        serde_json::from_value(json).expect("unscoped metadata parses");
    assert_eq!(parsed, unscoped.metadata);

    let scoped = scoped_evidence(Some(matte_region(7, 3)));
    let json = serde_json::to_value(&scoped.metadata).expect("metadata serializes");
    assert_eq!(
        json.get("matte_region")
            .and_then(|region| region.get("threshold"))
            .and_then(serde_json::Value::as_str),
        Some(MATTE_SCOPE_THRESHOLD)
    );
    let parsed: ScopeMeasurementMetadata =
        serde_json::from_value(json).expect("scoped metadata parses");
    assert_eq!(parsed, scoped.metadata);
}

#[test]
fn scope_metadata_recorded_before_cc5_still_loads() {
    // Evidence JSON written before matte scoping existed has no
    // `matte_region` key at all.
    let legacy = r#"{
        "stage": "monitoring_post_composite",
        "source_resolution": { "width": 4, "height": 2 },
        "measurement_resolution": { "width": 4, "height": 2 },
        "full_resolution": true,
        "normalized_roi": {
            "x_basis_points": 0,
            "y_basis_points": 0,
            "width_basis_points": 10000,
            "height_basis_points": 10000
        },
        "pixel_roi": { "x": 0, "y": 0, "width": 4, "height": 2 },
        "project_frames": [0],
        "roi_pixel_count": 8,
        "transparent_pixel_count": 6,
        "visible_pixel_count": 2
    }"#;

    let parsed: ScopeMeasurementMetadata =
        serde_json::from_str(legacy).expect("legacy evidence still loads");
    assert_eq!(parsed.matte_region, None);
    assert_eq!(parsed.stage, ScopeStage::MonitoringPostComposite);
    assert_eq!(parsed.visible_pixel_count, 2);
    assert_eq!(parsed, scoped_evidence(None).metadata);
}

fn proof_metadata() -> MatteProofMetadata {
    MatteProofMetadata {
        render: MonitorProofMetadata::test_double(),
        clip: ClipId(7),
        effect: EffectId(3),
        node_kind: "color_wheels".to_owned(),
        coverage_encoding: MATTE_COVERAGE_ENCODING.to_owned(),
        coverage_scale: MATTE_COVERAGE_SCALE,
        raster_aspect_millionths: 1_777_778,
        matte_enabled: true,
        window_count: 1,
        qualifier_enabled: false,
    }
}

#[test]
fn matte_proof_metadata_round_trips_through_json() {
    let metadata = proof_metadata();
    let json = serde_json::to_value(&metadata).expect("metadata serializes");

    // Provenance is composed, not replaced: the render kind still names the
    // renderer implementation rather than an output target.
    assert_eq!(
        json.get("render")
            .and_then(|render| render.get("render_kind"))
            .and_then(serde_json::Value::as_str),
        Some("test_double")
    );
    assert_eq!(
        json.get("coverage_encoding")
            .and_then(serde_json::Value::as_str),
        Some("linear_coverage_u8")
    );
    assert_eq!(
        json.get("coverage_scale")
            .and_then(serde_json::Value::as_u64),
        Some(255)
    );

    let parsed: MatteProofMetadata = serde_json::from_value(json).expect("metadata parses");
    assert_eq!(parsed, metadata);
    assert_eq!(
        parsed.render.render_kind,
        MonitorProofRenderKind::TestDouble
    );
    assert_eq!(MATTE_COVERAGE_ENCODING, "linear_coverage_u8");
    assert_eq!(MATTE_COVERAGE_SCALE, 255);
}

#[test]
fn matte_proof_carries_the_coverage_raster() {
    let proof = MatteProof {
        coverage: hand_built_coverage(),
        metadata: proof_metadata(),
    };

    let stats = statistics(&proof.coverage);
    assert_eq!(stats.covered_pixel_count, 9);
    assert_eq!(proof.coverage.width, 8);
    assert_eq!(proof.coverage.height, 4);
    assert_eq!(proof.metadata.coverage_scale, MATTE_COVERAGE_SCALE);
}

#[test]
fn matte_proof_failures_carry_stable_codes() {
    let inactive = MatteProofError::NodeInactive {
        reason: "matte_excluded".to_owned(),
    };
    assert_eq!(inactive.code(), "matte_proof_node_inactive");
    assert_eq!(MatteProofError::NoMatte.code(), "matte_proof_no_matte");
    assert_eq!(
        MatteProofError::EffectNotFound {
            clip: ClipId(7),
            effect: EffectId(3),
        }
        .code(),
        "matte_proof_effect_not_found"
    );
    assert_eq!(
        MatteProofError::NotAColorNode {
            clip: ClipId(7),
            effect: EffectId(3),
            name: "gaussian_blur".to_owned(),
        }
        .code(),
        "matte_proof_not_a_color_node"
    );

    // Converting into a media error keeps the code as the message prefix.
    let media: MediaError = inactive.clone().into();
    match &media {
        MediaError::Backend(message) => {
            assert!(
                message.starts_with("matte_proof_node_inactive: "),
                "unexpected message: {message}"
            );
            assert!(message.contains("matte_excluded"), "{message}");
        }
        other => panic!("expected a backend error, got {other:?}"),
    }
    assert_eq!(media.recovery_code(), None);
}

#[test]
fn coverage_failures_convert_into_media_errors() {
    let error = MatteCoverageError::ArithmeticOverflow {
        operation: "centroid_basis_points",
    };
    assert_eq!(error.code(), "matte_coverage_overflow");
    let media: MediaError = error.into();
    match media {
        MediaError::Backend(message) => assert!(
            message.starts_with("matte_coverage_overflow: "),
            "unexpected message: {message}"
        ),
        other => panic!("expected a backend error, got {other:?}"),
    }
}
