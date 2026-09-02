//! CC5 matte inspector and matte-window tracker tests.

use super::tracking::matte_noise_frame;
use super::*;
use crate::server::tracking::RegionTrackingRequest;

// -----------------------------------------------------------------------
// CC5 §4.2 / §5.2 / §7 — the matte agent surface
// -----------------------------------------------------------------------

/// A `width × height` coverage raster whose codes come from `code(x, y)`.
///
/// Built as a plain RGBA buffer, so no CC5 code path can prove its own
/// statistics.
fn matte_coverage_raster(width: u32, height: u32, code: impl Fn(u32, u32) -> u8) -> RgbaImage {
    let mut pixels = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for y in 0..height {
        for x in 0..width {
            let value = code(x, y);
            pixels.extend_from_slice(&[value, value, value, 255]);
        }
    }
    RgbaImage {
        width,
        height,
        pixels,
    }
}

/// A service whose clip carries one matted `color_wheels` node.
fn matte_service(coverage: Option<RgbaImage>) -> (KinewrightMcp, Core) {
    matte_service_with(coverage, BTreeMap::new(), Vec::new())
}

fn matte_service_with(
    coverage: Option<RgbaImage>,
    extra_matte_parameters: BTreeMap<String, i64>,
    extra_effects: Vec<Effect>,
) -> (KinewrightMcp, Core) {
    let (seed, playback, _) = fixture();
    let Event::QueryResult(QueryResult::Document(seed_document)) =
        seed.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected fixture document");
    };
    let mut document = (*seed_document).clone();
    // A managed CC1 source: an unknown-primaries fixture is refused before
    // any proof or matte work happens.
    document.media_pool[0].color_description = ColorDescription {
        primaries: ColorPrimaries::Bt709,
        transfer: ColorTransfer::Bt709,
        matrix: ColorMatrix::Bt709,
        range: ColorRange::Limited,
        white_point: ColorWhitePoint::D65,
        bit_depth: ColorBitDepth::Eight,
        confidence_basis_points: 10_000,
        provenance: ColorProvenance::StreamMetadata,
    };
    document.lut_assets.push(kinewright_core::LutAsset {
        id: kinewright_core::LutAssetId(1),
        sha256: "b".repeat(64),
        title: "transform".to_owned(),
        kind: kinewright_core::LutAssetKind::Cube3d,
        size: 17,
        byte_len: 1_024,
        domain_min_millionths: [0; 3],
        domain_max_millionths: [1_000_000; 3],
        source: kinewright_core::LutAssetSource::Builtin {
            name: "neutral".to_owned(),
        },
    });
    let mut parameters = BTreeMap::from([
        (
            "gain_red_thousandths".to_owned(),
            ParamValue::Integer(1_200),
        ),
        ("matte_enabled".to_owned(), ParamValue::Integer(1)),
        ("matte_window_count".to_owned(), ParamValue::Integer(1)),
    ]);
    for (name, value) in extra_matte_parameters {
        parameters.insert(name, ParamValue::Integer(value));
    }
    // Extras go first so an Input-stage node such as `technical_lut` sits
    // ahead of the Correction-stage wheels node Core's ordering rule
    // requires (CC4 §3.2).
    document.tracks[0].clips[0].effects = extra_effects
        .into_iter()
        .chain(std::iter::once(Effect {
            id: EffectId(1),
            name: "color_wheels".to_owned(),
            parameters,
            keyframes: BTreeMap::new(),
        }))
        .collect();
    let media = Arc::new(NoopMedia {
        matte_coverage: coverage,
        ..NoopMedia::default()
    });
    let core = Core::spawn(document).unwrap();
    let service = KinewrightMcp::new(core.clone(), playback, media, ConfirmationBroker::default());
    (service, core)
}

/// A service whose clip carries one matted `color_wheels` node and whose
/// analysis backend answers thumbnails from `frames`.
pub(super) fn matte_track_service(
    frames: BTreeMap<TimeCode, RgbaImage>,
    extra_matte_parameters: BTreeMap<String, i64>,
    extra_effects: Vec<Effect>,
) -> (KinewrightMcp, Core) {
    let (seed, playback, _) = fixture();
    let Event::QueryResult(QueryResult::Document(seed_document)) =
        seed.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected fixture document");
    };
    let mut document = (*seed_document).clone();
    let mut parameters = BTreeMap::from([
        ("matte_enabled".to_owned(), ParamValue::Integer(1)),
        ("matte_window_count".to_owned(), ParamValue::Integer(1)),
    ]);
    for (name, value) in extra_matte_parameters {
        parameters.insert(name, ParamValue::Integer(value));
    }
    document.tracks[0].clips[0].effects = std::iter::once(Effect {
        id: EffectId(1),
        name: "color_wheels".to_owned(),
        parameters,
        keyframes: BTreeMap::new(),
    })
    .chain(extra_effects)
    .collect();
    let media = Arc::new(NoopMedia {
        thumbnail_frames: frames,
        ..NoopMedia::default()
    });
    let core = Core::spawn(document).unwrap();
    let service = KinewrightMcp::new(core.clone(), playback, media, ConfirmationBroker::default());
    (service, core)
}

/// CC5 §4.2: the statistics are measured off a hand-built coverage, so
/// every expected value below is derived by hand rather than by the code
/// that produced it.
#[test]
fn inspect_grade_matte_reports_the_cc5_coverage_statistics() {
    // A 4 × 2 coverage:
    //   row 0: 255 255 128   0
    //   row 1: 255 255 128   0
    // Hand-derived: 6 covered (m > 0), 4 full (code 255), 2 partial,
    // 8 total, floor(6 * 10000 / 8) = 7500 basis points. The bounding box
    // of the covered set is columns 0..3 of rows 0..2, i.e. x 0..7500 of
    // the width and the whole height. Buckets are
    // min(15, floor(code * 16 / 256)): code 0 -> 0, 128 -> 8, 255 -> 15.
    let coverage = matte_coverage_raster(4, 2, |x, _| match x {
        0 | 1 => 255,
        2 => 128,
        _ => 0,
    });
    let (service, _core) = matte_service(Some(coverage));

    let result = service
        .inspect_grade_matte(&InspectGradeMatteArgs {
            expected_revision: Some(TimelineRevision(0)),
            clip_id: ClipId(1),
            effect_id: EffectId(1),
            timecode: TimeCode(10),
            include_image: None,
        })
        .unwrap();
    assert_ne!(result.is_error, Some(true));
    let structured = result.structured_content.clone().unwrap();

    let statistics = &structured["statistics"];
    assert_eq!(statistics["covered_pixel_count"], 6);
    assert_eq!(statistics["full_pixel_count"], 4);
    assert_eq!(statistics["partial_pixel_count"], 2);
    assert_eq!(statistics["total_pixel_count"], 8);
    assert_eq!(statistics["covered_basis_points"], 7_500);
    assert_eq!(statistics["weighted_by_coverage"], true);
    let histogram = statistics["coverage_histogram"].as_array().unwrap();
    assert_eq!(histogram.len(), 16);
    assert_eq!(histogram[0], 2, "the two code-0 pixels land in bucket 0");
    assert_eq!(histogram[8], 2, "the two code-128 pixels land in bucket 8");
    assert_eq!(
        histogram[15], 4,
        "the four code-255 pixels land in bucket 15"
    );
    let total = histogram
        .iter()
        .map(|count| count.as_u64().unwrap())
        .sum::<u64>();
    assert_eq!(total, 8, "the buckets cover every pixel, code 0 included");

    // CC5 §4.3's threshold is reported at the level a caller reads it.
    assert_eq!(structured["matte_threshold"], "coverage_greater_than_zero");
    assert_eq!(structured["covered_pixel_count"], 6);
    assert_eq!(structured["raster"], json!({"width": 4, "height": 2}));
    assert_eq!(structured["coverage_encoding"], "linear_coverage_u8");
    assert_eq!(structured["coverage_scale"], 255);
    assert_eq!(structured["kind"], "color_wheels");
    assert_eq!(structured["active"], true);
    assert_eq!(structured["inactive_reason"], serde_json::Value::Null);
    // CC5 §1: the two coverage concepts are named apart.
    assert_eq!(structured["surface"], "Matte (this correction)");
    assert!(
        structured["distinct_from"]
            .as_str()
            .unwrap()
            .contains("Mask (layer alpha)")
    );
    // The full 47 integers, as a compact object.
    let resolved = &structured["resolved_matte_parameters"];
    assert_eq!(resolved["matte_enabled"], 1);
    assert_eq!(resolved["matte_window_count"], 1);
    assert_eq!(resolved["matte_mix_basis_points"], 10_000);
    assert_eq!(resolved["matte_hue_width_centidegrees"], 18_000);
    assert_eq!(resolved["windows"].as_array().unwrap().len(), 1);
    assert_eq!(resolved["windows"][0]["half_width_basis_points"], 2_500);
    // Renderer provenance rides along, unchanged from the monitor proof.
    assert_eq!(structured["provenance"]["node_kind"], "color_wheels");
    assert_eq!(structured["provenance"]["window_count"], 1);
    assert_eq!(
        structured["provenance"]["render"]["render_kind"],
        "test_double"
    );

    // A PNG is attached by default and suppressed on request.
    assert_eq!(structured["image_included"], true);
    assert!(
        result
            .content
            .iter()
            .any(|block| block.as_image().is_some()),
        "include_image defaults to true"
    );
    let without = service
        .inspect_grade_matte(&InspectGradeMatteArgs {
            expected_revision: None,
            clip_id: ClipId(1),
            effect_id: EffectId(1),
            timecode: TimeCode(10),
            include_image: Some(false),
        })
        .unwrap();
    assert!(
        without
            .content
            .iter()
            .all(|block| block.as_image().is_none())
    );
}

/// CC5 §4.1: a backend that cannot proof fails typed. It never returns a
/// blank frame, and it never invents a coverage number.
#[test]
fn inspect_grade_matte_surfaces_an_unavailable_proof_as_a_typed_refusal() {
    let (service, _core) = matte_service(None);
    let result = service
        .inspect_grade_matte(&InspectGradeMatteArgs {
            expected_revision: None,
            clip_id: ClipId(1),
            effect_id: EffectId(1),
            timecode: TimeCode(10),
            include_image: None,
        })
        .unwrap();

    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.unwrap();
    assert_eq!(structured["code"], "matte_proof_unavailable");
    assert_eq!(structured["applied"], false);
    let details = &structured["details"];
    assert_eq!(details["field"], "effect_id");
    assert_eq!(details["observed"]["effect_id"], 1);
    assert_eq!(details["observed"]["node_kind"], "color_wheels");
    assert_eq!(details["observed"]["has_matte"], true);
    assert!(
        details["recovery_action"]
            .as_str()
            .unwrap()
            .contains("no coverage is invented here")
    );
    // The resolved matte is still published: the refusal is about the
    // render, not about the request.
    assert_eq!(details["resolved_matte"]["matte_enabled"], 1);
}

/// CC5 §2.1: `technical_lut` carries no matte, and the layer `mask` effect
/// is a compositing alpha operation, not a colour node.
#[test]
fn inspect_grade_matte_refuses_nodes_that_cannot_carry_a_matte() {
    let (service, _core) = matte_service_with(
        None,
        BTreeMap::new(),
        vec![
            Effect {
                id: EffectId(2),
                name: "mask".to_owned(),
                parameters: BTreeMap::new(),
                keyframes: BTreeMap::new(),
            },
            Effect {
                id: EffectId(3),
                name: "technical_lut".to_owned(),
                parameters: BTreeMap::from([("lut_asset_id".to_owned(), ParamValue::Integer(1))]),
                keyframes: BTreeMap::new(),
            },
        ],
    );

    let mask = service
        .inspect_grade_matte(&InspectGradeMatteArgs {
            expected_revision: None,
            clip_id: ClipId(1),
            effect_id: EffectId(2),
            timecode: TimeCode(10),
            include_image: None,
        })
        .unwrap();
    let structured = mask.structured_content.unwrap();
    assert_eq!(structured["code"], "matte_effect_not_a_color_node");
    assert!(
        structured["details"]["recovery_action"]
            .as_str()
            .unwrap()
            .contains("compositing alpha operation")
    );

    let technical = service
        .inspect_grade_matte(&InspectGradeMatteArgs {
            expected_revision: None,
            clip_id: ClipId(1),
            effect_id: EffectId(3),
            timecode: TimeCode(10),
            include_image: None,
        })
        .unwrap();
    let structured = technical.structured_content.unwrap();
    assert_eq!(structured["code"], "matte_unsupported_node_kind");
    assert_eq!(structured["details"]["observed"], "technical_lut");
    assert_eq!(
        structured["details"]["allowed"],
        json!(crate::color_status::MATTE_CAPABLE_NODE_NAMES)
    );
}

/// CC5 §7: `matte_comparison` is valid only alongside `effect_id`, is
/// mutually exclusive with `look_comparison`, and needs a node that both
/// may carry a matte and actually does. Every check runs before any render.
#[test]
fn render_color_proof_validates_matte_comparison_before_rendering() {
    let (service, _core) = matte_service_with(
        None,
        BTreeMap::new(),
        vec![Effect {
            id: EffectId(3),
            name: "color_curves".to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::new(),
        }],
    );
    let proof = |effect_id: Option<EffectId>,
                 matte: Option<MatteComparison>,
                 look: Option<LookComparison>| {
        service
            .render_color_proof(&RenderColorProofArgs {
                expected_revision: TimelineRevision(0),
                clip_id: ClipId(1),
                timecode: TimeCode(10),
                profile_assumption: None,
                parameters: BTreeMap::new(),
                effect_id,
                look_comparison: look,
                matte_comparison: matte,
            })
            .unwrap()
            .structured_content
            .unwrap()
    };

    let without_effect = proof(None, Some(MatteComparison::Coverage), None);
    assert_eq!(
        without_effect["code"],
        "matte_comparison_requires_effect_id"
    );
    assert_eq!(without_effect["details"]["field"], "matte_comparison");

    let both = proof(
        Some(EffectId(1)),
        Some(MatteComparison::InsideOnly),
        Some(LookComparison::Before),
    );
    assert_eq!(
        both["code"],
        "matte_comparison_conflicts_with_look_comparison"
    );
    assert!(
        both["details"]["allowed"]
            .as_str()
            .unwrap()
            .contains("exactly one")
    );

    // A matte-capable node that carries no matte has no coverage to
    // partition, so the proof refuses rather than rendering a blank frame.
    let no_matte = proof(Some(EffectId(3)), Some(MatteComparison::OutsideOnly), None);
    assert_eq!(no_matte["code"], "matte_proof_no_matte");
    assert_eq!(no_matte["details"]["observed"]["has_matte"], false);
    assert!(
        no_matte["details"]["recovery_action"]
            .as_str()
            .unwrap()
            .contains("plan_secondary_correction")
    );
}

/// CC5 §7: `matte_invert` is Hold-only but keyframable, so `outside_only`
/// must get the curve out of the way on its scratch copy — otherwise the
/// static write is dead, the "outside" cell renders the *inside*, and the
/// manifest says `outside_only` about a picture that is not.
#[test]
fn render_color_proof_outside_only_clears_a_keyframed_matte_invert_on_the_scratch_copy() {
    let (service, core) = matte_service(None);
    // A Hold curve that turns the matte inversion *on* from frame 0. The
    // stored static value stays 0, so a planner reading only the static
    // value would toggle to 1 and render exactly the inside cell.
    let Event::DocumentChanged { revision, .. } = core
        .request(Command::Do(Operation::SetEffectKeyframes {
            clip: ClipId(1),
            effect: EffectId(1),
            name: "matte_invert".to_owned(),
            curve: kinewright_core::AutomationCurve {
                keyframes: vec![kinewright_core::Keyframe {
                    at: TimeCode(0),
                    value: 1,
                    interpolation: kinewright_core::KeyframeInterpolation::Hold,
                }],
            },
        }))
        .unwrap()
    else {
        panic!("expected the keyframe to apply");
    };
    assert_eq!(revision, TimelineRevision(1));

    let result = service
        .render_color_proof(&RenderColorProofArgs {
            expected_revision: TimelineRevision(1),
            clip_id: ClipId(1),
            timecode: TimeCode(10),
            profile_assumption: None,
            parameters: BTreeMap::new(),
            effect_id: Some(EffectId(1)),
            look_comparison: None,
            matte_comparison: Some(MatteComparison::OutsideOnly),
        })
        .unwrap();
    let manifest = result.structured_content.unwrap();
    assert_ne!(
        result.is_error,
        Some(true),
        "outside_only proof refused: {manifest}"
    );
    let comparison = &manifest["matte_comparison"];
    assert_eq!(comparison["variant"], "outside_only");
    // The curve is cleared first, then the complement of the value the
    // curve renders at this frame is written. The rendered value is 1, so
    // the outside cell writes 0 — not 1, which is what complementing the
    // stored static value would have produced.
    assert_eq!(
        comparison["after_operations"],
        json!([
            {"ClearEffectKeyframes": {"clip": 1, "effect": 1, "name": "matte_invert"}},
            {"SetEffectParam": {
                "clip": 1,
                "effect": 1,
                "name": "matte_invert",
                "value": 0,
            }},
        ])
    );
    assert_eq!(comparison["cleared_keyframes"], json!(["matte_invert"]));

    // Scratch only: the live document keeps its automation untouched.
    let Event::QueryResult(QueryResult::Document(document)) =
        core.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected a document");
    };
    let effect = &document.clip(ClipId(1)).unwrap().effects[0];
    assert_eq!(
        effect.keyframes["matte_invert"].keyframes[0].value, 1,
        "the live document must still carry the curve"
    );
    assert!(!effect.parameters.contains_key("matte_invert"));

    // A node with no `matte_invert` automation is byte-unchanged: one
    // operation, and an empty `cleared_keyframes`.
    let (plain, _) = matte_service(None);
    let plain = plain
        .render_color_proof(&RenderColorProofArgs {
            expected_revision: TimelineRevision(0),
            clip_id: ClipId(1),
            timecode: TimeCode(10),
            profile_assumption: None,
            parameters: BTreeMap::new(),
            effect_id: Some(EffectId(1)),
            look_comparison: None,
            matte_comparison: Some(MatteComparison::OutsideOnly),
        })
        .unwrap()
        .structured_content
        .unwrap();
    assert_eq!(
        plain["matte_comparison"]["cleared_keyframes"],
        json!([]),
        "a node with no matte_invert curve clears nothing"
    );
}

/// CC5 §7: `outside_only` renders a scratch copy with `matte_invert`
/// toggled, and the manifest states exactly which variant it rendered.
#[test]
fn render_color_proof_outside_only_toggles_matte_invert_on_a_scratch_copy() {
    let (service, core) = matte_service(None);
    let result = service
        .render_color_proof(&RenderColorProofArgs {
            expected_revision: TimelineRevision(0),
            clip_id: ClipId(1),
            timecode: TimeCode(10),
            profile_assumption: None,
            parameters: BTreeMap::new(),
            effect_id: Some(EffectId(1)),
            look_comparison: None,
            matte_comparison: Some(MatteComparison::OutsideOnly),
        })
        .unwrap();
    let manifest = result.structured_content.unwrap();
    assert_ne!(
        result.is_error,
        Some(true),
        "outside_only proof refused: {manifest}"
    );
    let comparison = &manifest["matte_comparison"];
    assert_eq!(comparison["variant"], "outside_only");
    assert_eq!(comparison["effect_id"], 1);
    assert_eq!(comparison["kind"], "color_wheels");
    assert!(
        comparison["after_cell"]
            .as_str()
            .unwrap()
            .contains("matte_invert toggled")
    );
    // The exact scratch operation, hand-written.
    assert_eq!(
        comparison["after_operations"],
        json!([{"SetEffectParam": {
            "clip": 1,
            "effect": 1,
            "name": "matte_invert",
            "value": 1,
        }}])
    );
    // `inside_only` renders the document exactly as stored, so it has no
    // scratch operation at all.
    let inside = service
        .render_color_proof(&RenderColorProofArgs {
            expected_revision: TimelineRevision(0),
            clip_id: ClipId(1),
            timecode: TimeCode(10),
            profile_assumption: None,
            parameters: BTreeMap::new(),
            effect_id: Some(EffectId(1)),
            look_comparison: None,
            matte_comparison: Some(MatteComparison::InsideOnly),
        })
        .unwrap()
        .structured_content
        .unwrap();
    assert_eq!(inside["matte_comparison"]["variant"], "inside_only");
    assert_eq!(inside["matte_comparison"]["after_operations"], json!([]));
    assert_eq!(inside["applied"], false);

    // Read-only: the live document never gained `matte_invert`.
    let Event::QueryResult(QueryResult::Document(document)) =
        core.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected a document");
    };
    assert_eq!(
        document.clip(ClipId(1)).unwrap().effects[0]
            .parameters
            .get("matte_invert"),
        None
    );
}

/// CC5 §7: `coverage` replaces the AFTER cell with the §4.1 proof image
/// itself, and reports the measured coverage next to it.
#[test]
fn render_color_proof_coverage_returns_the_matte_proof_image() {
    // 320 × 180 is the fixture raster; the left third is covered.
    let coverage = matte_coverage_raster(320, 180, |x, _| u8::from(x < 106) * 255);
    let (service, _core) = matte_service(Some(coverage));
    let manifest = service
        .render_color_proof(&RenderColorProofArgs {
            expected_revision: TimelineRevision(0),
            clip_id: ClipId(1),
            timecode: TimeCode(10),
            profile_assumption: None,
            parameters: BTreeMap::new(),
            effect_id: Some(EffectId(1)),
            look_comparison: None,
            matte_comparison: Some(MatteComparison::Coverage),
        })
        .unwrap()
        .structured_content
        .unwrap();

    let comparison = &manifest["matte_comparison"];
    assert_eq!(comparison["variant"], "coverage");
    // 106 columns of 320, over 180 rows: 106 * 180 = 19080 of 57600, and
    // floor(19080 * 10000 / 57600) = 3312 basis points.
    assert_eq!(comparison["coverage"]["covered_pixel_count"], 19_080);
    assert_eq!(
        comparison["coverage"]["statistics"]["covered_basis_points"],
        3_312
    );
    assert_eq!(
        comparison["coverage"]["matte_threshold"],
        "coverage_greater_than_zero"
    );
    assert_eq!(comparison["coverage"]["coverage_scale"], 255);
}

/// CC5 §7: a CC4 proof is byte-unchanged — no `matte_comparison` key at all
/// when none was requested.
#[test]
fn render_color_proof_omits_matte_comparison_when_none_was_requested() {
    let (service, _core) = matte_service(None);
    let manifest = service
        .render_color_proof(&RenderColorProofArgs {
            expected_revision: TimelineRevision(0),
            clip_id: ClipId(1),
            timecode: TimeCode(10),
            profile_assumption: None,
            parameters: BTreeMap::new(),
            effect_id: Some(EffectId(1)),
            look_comparison: None,
            matte_comparison: None,
        })
        .unwrap()
        .structured_content
        .unwrap();
    assert!(manifest.get("matte_comparison").is_none());
}

/// A 320 × 180 frame carrying one 40 × 40 bright box centred on `centre`.
///
/// The `box_frame` pattern from `mod tracking_tests`, at the fixture
/// raster: a static dark background with one high-contrast subject, which
/// is what pins a normalized SAD template match at zero displacement error.
pub(super) fn matte_box_frame(centre: [u32; 2]) -> RgbaImage {
    let (width, height) = (320_u32, 180_u32);
    let mut pixels = vec![0_u8; (width * height * 4) as usize];
    for pixel in pixels.as_chunks_mut::<4>().0.iter_mut() {
        *pixel = [48, 48, 48, 255];
    }
    for y in centre[1].saturating_sub(20)..(centre[1] + 20).min(height) {
        for x in centre[0].saturating_sub(20)..(centre[0] + 20).min(width) {
            let index = ((y * width + x) * 4) as usize;
            pixels[index..index + 4].copy_from_slice(&[235, 235, 235, 255]);
        }
    }
    RgbaImage {
        width,
        height,
        pixels,
    }
}

/// CC5 §5.2: track one window on a synthetic moving subject and return a
/// prepared plan of exactly two keyframe operations, committing nothing.
#[test]
#[allow(clippy::too_many_lines)]
fn track_matte_window_prepares_two_keyframe_operations_without_committing() {
    // The subject travels from x = 80 to x = 240 across frames 0..=40,
    // 4 pixels per frame, at a constant y = 90.
    let frames = (0..=40)
        .map(|frame| {
            (
                TimeCode(frame),
                matte_box_frame([u32::try_from(80 + frame * 4).unwrap(), 90]),
            )
        })
        .collect::<BTreeMap<_, _>>();
    // Seed the window on the subject at frame 0: pixel 80 of 320 is 2500
    // basis points of the width, and pixel 90 of 180 is 5000 of the height.
    let (service, core) = matte_track_service(
        frames,
        BTreeMap::from([("matte_window0_center_x_basis_points".to_owned(), 2_500)]),
        Vec::new(),
    );

    let result = service
        .track_matte_window(&TrackMatteWindowArgs {
            expected_revision: Some(TimelineRevision(0)),
            clip_id: ClipId(1),
            effect_id: EffectId(1),
            window_index: 0,
            start_local_frame: Some(TimeCode(0)),
            end_local_frame: Some(TimeCode(41)),
            step_frames: Some(10),
            search_radius_percent: Some(25),
            max_width: Some(320),
            minimum_confidence_basis_points: None,
        })
        .unwrap();
    let structured = result.structured_content.clone().unwrap();
    assert_ne!(
        result.is_error,
        Some(true),
        "tracking refused: {structured}"
    );

    // Five samples at 0, 10, 20, 30, 40.
    let observations = structured["observations"].as_array().unwrap();
    assert_eq!(observations.len(), 5);
    assert_eq!(
        observations
            .iter()
            .map(|observation| observation["local_frame"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![0, 10, 20, 30, 40]
    );
    // Raw centres, hand-derived through CC5 §5.2's conversion at scale 1:
    // the subject centre is pixel x = 80 + 4 * frame, and
    // round((pixel + 0.5) * 10000 / 320) gives 2516, 3766, 5016, 6266,
    // 7516. The tracker seeds from the window centre, so the first sample
    // is the seeded position and the rest are matched.
    let raw = observations
        .iter()
        .map(|observation| observation["center_x_basis_points"].as_i64().unwrap())
        .collect::<Vec<_>>();
    for (index, expected) in [2_516_i64, 3_766, 5_016, 6_266, 7_516].iter().enumerate() {
        assert!(
            (raw[index] - expected).abs() <= 200,
            "sample {index}: raw {} against the analytic {expected}",
            raw[index]
        );
    }
    // A static subject on the vertical axis: every raw y stays at the
    // seeded centre, round((89.5 + 0.5) * 10000 / 180) = 5000.
    for observation in observations {
        assert!((observation["center_y_basis_points"].as_i64().unwrap() - 5_000).abs() <= 200);
        assert!(observation["confidence_basis_points"].as_u64().unwrap() >= 5_000);
    }

    // The smoothed curve differs from the raw observations, and the last
    // sample lags by one inter-sample displacement exactly as CC5 §5.2
    // states.
    let smoothed = structured["curves"]["matte_window0_center_x_basis_points"]["keyframes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|keyframe| keyframe["value"].as_i64().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(smoothed.len(), 5);
    assert!(
        smoothed[4] < raw[4],
        "the median filter must lag the final sample: smoothed {} against raw {}",
        smoothed[4],
        raw[4]
    );
    assert!(
        (raw[4] - smoothed[4]) <= 2 * (raw[4] - raw[3]),
        "the lag is bounded by one inter-sample displacement"
    );
    // Every keyframe is Linear: sustained movement gets continuous
    // velocity, and M40 rejected eased per-segment curves.
    for keyframe in structured["curves"]["matte_window0_center_x_basis_points"]["keyframes"]
        .as_array()
        .unwrap()
    {
        assert_eq!(keyframe["interpolation"], "linear");
    }

    // The pinned M40 constants ride in the response.
    let stabilization = &structured["window_stabilization"];
    assert_eq!(stabilization["median_filter"], true);
    assert_eq!(stabilization["dead_zone_basis_points"], 0);
    assert_eq!(stabilization["maximum_step_basis_points"], 800);
    assert_eq!(stabilization["minimum_basis_points"], -10_000);
    assert_eq!(stabilization["maximum_basis_points"], 20_000);
    assert_eq!(stabilization["interpolation"], "Linear");

    // CC5 §5.2's conversion is stated, not inferred.
    let space = &structured["coordinate_space"];
    assert_eq!(
        space["pixel_to_basis_points"],
        "centre_bp = round((pixel + 0.5) * 10000 / extent)"
    );
    assert_eq!(
        space["composite_to_layer"],
        "u_layer = (u_composite - 0.5) / scale - (offset_x, offset_y) / (2 * scale) + 0.5"
    );
    assert_eq!(space["layer_scale"], 1.0);
    // hw = 2500 bp at scale 1 is a 50 percent template.
    assert_eq!(space["box_percent"], json!([50, 50]));

    // CC5 §5.2's provenance marker.
    let boundary = structured["tracking_boundary"].as_str().unwrap();
    assert!(boundary.contains("normalized SAD template match"));
    assert!(boundary.contains("no learned object, face, or skin detection"));
    assert!(boundary.contains("rotation_centidegrees"));

    // The prepared plan carries exactly the two keyframe operations, and
    // neither is destructive.
    let preview = &structured["prepared_edit_plan"]["preview"];
    assert_eq!(preview["operation_count"], 2);
    assert_eq!(preview["destructive_operations"], json!([]));
    assert_eq!(preview["expected_revision"], 0);
    assert_eq!(preview["before_clips"], preview["after_clips"]);
    // The two parameters CC5 §5.2 writes, and no others: rotation and the
    // half extents are never written.
    assert_eq!(
        structured["parameters"],
        json!([
            "matte_window0_center_x_basis_points",
            "matte_window0_center_y_basis_points"
        ])
    );
    assert_eq!(
        structured["curves"].as_object().unwrap().len(),
        2,
        "exactly two curves are proposed"
    );
    assert_eq!(structured["applied"], false);

    // Nothing was committed: the live node still carries no automation.
    let Event::QueryResult(QueryResult::Document(document)) =
        core.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected a document");
    };
    assert!(
        document
            .clip(ClipId(1))
            .unwrap()
            .effects
            .iter()
            .all(|effect| effect.keyframes.is_empty()),
        "track_matte_window commits nothing"
    );
}

// -----------------------------------------------------------------------
// CC5 §9.2.11, agent half: the tracked shot.
//
// The media crate owns the generated clip and proves containment for a
// *simulated* smoother; the real `track_matte_window` lives here, so the
// same containment gate is run against the curve the tool actually emits.
// The shot is the media crate's recipe, restated because its generator and
// its analytic helpers are `pub(crate)` to `kinewright-media`.
// -----------------------------------------------------------------------

/// The §9.2.11 tracked shot's raster and subject, from the media recipe.
const TRACKED_SHOT_WIDTH: u32 = 640;
const TRACKED_SHOT_HEIGHT: u32 = 360;
const TRACKED_SHOT_FRAMES: i64 = 100;
const TRACKED_SHOT_FPS: i64 = 25;
/// The white subject is 80 × 80 pixels.
const TRACKED_SHOT_BOX: i64 = 80;
/// §9.2.11's window half extents, in basis points of width and of height.
const TRACKED_SHOT_HALF_WIDTH_BASIS_POINTS: i64 = 1_300;
const TRACKED_SHOT_HALF_HEIGHT_BASIS_POINTS: i64 = 1_800;
/// The subject's own half extents in the same units: half of the 80 px box
/// is `40 / 640 = 625` bp of the width and `40 / 360 = 1111.1` bp of the
/// height, which is the `625` / `1111` §9.2.11 states. The exact fraction
/// is used on the vertical axis because it is the stricter of the two.
const TRACKED_SHOT_SUBJECT_HALF_WIDTH_BASIS_POINTS: f64 = 625.0;
const TRACKED_SHOT_SUBJECT_HALF_HEIGHT_BASIS_POINTS: f64 = 40.0 * 10_000.0 / 360.0;
/// §9.2.11's derived margin budget: `1300 − 625` and `1800 − 1111`.
const TRACKED_SHOT_MARGIN_BUDGET_X_BASIS_POINTS: f64 = 675.0;
const TRACKED_SHOT_MARGIN_BUDGET_Y_BASIS_POINTS: f64 = 689.0;
/// §9.2.11's tolerances: the raw observations may miss the analytic centre
/// by 200 bp, and the smoothed curve — which pays the median filter's lag
/// on top of that error — by 600 bp.
const TRACKED_SHOT_RAW_TOLERANCE_BASIS_POINTS: f64 = 200.0;
const TRACKED_SHOT_SMOOTHED_TOLERANCE_BASIS_POINTS: f64 = 600.0;
/// The measured worst `|raw − analytic|` of the real tracker on this shot,
/// in basis points, at samples 9 (x) and 19 (y).
const TRACKED_SHOT_WORST_RAW_X_BASIS_POINTS: f64 = 55.0;
const TRACKED_SHOT_WORST_RAW_Y_BASIS_POINTS: f64 = 180.888_888_888_888_7;
/// The measured worst `|smoothed − analytic|`, both at sample 99.
const TRACKED_SHOT_WORST_SMOOTHED_X_BASIS_POINTS: f64 = 366.75;
const TRACKED_SHOT_WORST_SMOOTHED_Y_BASIS_POINTS: f64 = 292.0;
/// The measured worst containment margin over all 100 frames, both at
/// frame 99: `675 − 366.75` and `688.9 − 292`.
const TRACKED_SHOT_WORST_MARGIN_X_BASIS_POINTS: f64 = 308.25;
const TRACKED_SHOT_WORST_MARGIN_Y_BASIS_POINTS: f64 = 396.888_888_888_888_7;

/// The analytic top-left corner of the subject at clip-local `frame`.
///
/// The media crate generates the shot with
/// `overlay=x='320+120*sin(2*PI*t/8)-40':y='180+60*sin(2*PI*t/8)-40'` over
/// a solid `0x303030` 640 × 360 background at 25 fps. `overlay` exposes
/// `t` as *time*, so `t = frame / 25`, and the realised box snaps to even
/// pixel offsets because that clip is muxed `yuv420p`; the expectation is
/// therefore `2·floor(edge / 2)`. Restated rather than imported:
/// `cc5_fixtures.rs::analytic_box_corner` is `pub(crate)` to the media
/// crate and cannot be called from here.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn tracked_shot_box_corner(frame: i64) -> (i64, i64) {
    let seconds = frame as f64 / TRACKED_SHOT_FPS as f64;
    let phase = (2.0 * std::f64::consts::PI * seconds / 8.0).sin();
    let snap = |value: f64| 2 * (value / 2.0).floor() as i64;
    (
        snap(320.0 + 120.0 * phase - 40.0),
        snap(180.0 + 60.0 * phase - 40.0),
    )
}

/// The exact window centre, in basis points, that centres the analytic box
/// in the window at `frame`. Kept fractional: rounding it to the integer
/// the tool emits would hide up to half a basis point of the error this
/// test measures.
#[allow(clippy::cast_precision_loss)]
fn tracked_shot_centre_basis_points(frame: i64) -> [f64; 2] {
    let (x, y) = tracked_shot_box_corner(frame);
    [
        (x + TRACKED_SHOT_BOX / 2) as f64 * 10_000.0 / f64::from(TRACKED_SHOT_WIDTH),
        (y + TRACKED_SHOT_BOX / 2) as f64 * 10_000.0 / f64::from(TRACKED_SHOT_HEIGHT),
    ]
}

/// One frame of the tracked shot as an RGBA thumbnail.
///
/// The background is solid on purpose: §5.2's box rule makes the SAD
/// template *window* sized rather than subject sized, and a featureless
/// background is what pins the match on the subject instead of on some
/// other piece of texture inside the template.
fn tracked_shot_frame(frame: i64) -> RgbaImage {
    let (width, height) = (TRACKED_SHOT_WIDTH, TRACKED_SHOT_HEIGHT);
    let mut pixels = vec![0_u8; (width * height * 4) as usize];
    for pixel in pixels.as_chunks_mut::<4>().0.iter_mut() {
        *pixel = [0x30, 0x30, 0x30, 255];
    }
    let (left, top) = tracked_shot_box_corner(frame);
    for y in top..top + TRACKED_SHOT_BOX {
        for x in left..left + TRACKED_SHOT_BOX {
            let y = u32::try_from(y).expect("the subject stays inside the raster");
            let x = u32::try_from(x).expect("the subject stays inside the raster");
            let index = ((y * width + x) * 4) as usize;
            pixels[index..index + 4].copy_from_slice(&[255, 255, 255, 255]);
        }
    }
    RgbaImage {
        width,
        height,
        pixels,
    }
}

/// A service whose only clip is the §9.2.11 tracked shot — 100 frames of
/// 640 × 360 at 25 fps — carrying one matted `color_wheels` node whose
/// window 0 is the contract's 1300 × 1800 bp rect, and whose analysis
/// backend answers thumbnails with the generated frames.
///
/// The window centre is left at its neutral 5000 / 5000, which *is* the
/// analytic centre at frame 0 — the tracker seeds from the stored centre,
/// so seeding it anywhere else would inject an error the shot does not
/// have. The clip starts at timeline frame 0, so clip-local and project
/// frames coincide and the thumbnail map is keyed by either.
fn tracked_shot_service() -> (KinewrightMcp, Core) {
    let asset = MediaAsset {
        id: AssetId(1),
        path: PathBuf::from("cc5-tracked-shot.mp4"),
        name: "cc5-tracked-shot".to_owned(),
        duration: TimeCode(TRACKED_SHOT_FRAMES),
        fps: Rational::new(u32::try_from(TRACKED_SHOT_FPS).unwrap(), 1).unwrap(),
        kind: MediaKind::Video,
        resolution: Some((TRACKED_SHOT_WIDTH, TRACKED_SHOT_HEIGHT)),
        source_fingerprint: MediaSourceFingerprint::default(),
        color_description: ColorDescription::default(),
    };
    let document = Document {
        catalog: kinewright_core::MediaCatalog::default(),
        audio_mix: kinewright_core::AudioMix::default(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![Clip {
                id: ClipId(1),
                asset: asset.id,
                source_range: TimeCode::ZERO..TimeCode(TRACKED_SHOT_FRAMES),
                content: ClipContent::Media,
                timeline_start: TimeCode::ZERO,
                effects: vec![Effect {
                    id: EffectId(1),
                    name: "color_wheels".to_owned(),
                    parameters: BTreeMap::from([
                        ("matte_enabled".to_owned(), ParamValue::Integer(1)),
                        ("matte_window_count".to_owned(), ParamValue::Integer(1)),
                        (
                            "matte_window0_half_width_basis_points".to_owned(),
                            ParamValue::Integer(TRACKED_SHOT_HALF_WIDTH_BASIS_POINTS),
                        ),
                        (
                            "matte_window0_half_height_basis_points".to_owned(),
                            ParamValue::Integer(TRACKED_SHOT_HALF_HEIGHT_BASIS_POINTS),
                        ),
                    ]),
                    keyframes: BTreeMap::new(),
                }],
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
            }],
        }],
        media_pool: vec![asset],
        markers: Vec::new(),
        fps: Rational::new(u32::try_from(TRACKED_SHOT_FPS).unwrap(), 1).unwrap(),
        resolution: (TRACKED_SHOT_WIDTH, TRACKED_SHOT_HEIGHT),
        duration: TimeCode(TRACKED_SHOT_FRAMES),
        color_context: kinewright_core::ColorContext::default(),
        lut_assets: Vec::new(),
    };
    let media = Arc::new(NoopMedia {
        thumbnail_frames: (0..TRACKED_SHOT_FRAMES)
            .map(|frame| (TimeCode(frame), tracked_shot_frame(frame)))
            .collect(),
        ..NoopMedia::default()
    });
    let playback: Arc<dyn Playback> = media.clone();
    let analysis: Arc<dyn Analysis> = media;
    let core = Core::spawn(document).unwrap();
    let service = KinewrightMcp::new(
        core.clone(),
        playback,
        analysis,
        ConfirmationBroker::default(),
    );
    (service, core)
}

/// CC5 §9.2.11, agent half. The *smoothed* curve `track_matte_window`
/// prepares, linearly interpolated between its sample keyframes, keeps the
/// analytic subject box inside the 1300 × 1800 bp window at **every**
/// frame `0..=99` — not only at the 21 frames the tracker sampled.
///
/// The media crate's
/// `cc5_tracked_shot_window_contains_the_subject_at_every_frame` runs this
/// gate against ground truth and against a *simulated* smoother; the real
/// tool lives here, so this runs it against the curve the tool actually
/// emitted for the same shot. Interpolation is
/// [`AutomationCurve::value_at`], which is precisely the evaluator
/// `Effect::evaluated_at` calls and therefore precisely the rule the media
/// gate uses — a hand-rolled lerp here could agree with the contract and
/// disagree with the timeline.
#[test]
#[allow(clippy::too_many_lines, clippy::cast_precision_loss)]
fn track_matte_window_smoothed_curve_contains_the_subject_at_every_frame() {
    let (service, _core) = tracked_shot_service();

    // The media recipe's tracking call: step 5, radius 25, max_width 512.
    // The analysis double answers thumbnails at the frames' own raster, so
    // the tracker measures 640 × 360 and `max_width` only records the
    // recipe.
    let result = service
        .track_matte_window(&TrackMatteWindowArgs {
            expected_revision: Some(TimelineRevision(0)),
            clip_id: ClipId(1),
            effect_id: EffectId(1),
            window_index: 0,
            start_local_frame: Some(TimeCode(0)),
            end_local_frame: Some(TimeCode(TRACKED_SHOT_FRAMES)),
            step_frames: Some(5),
            search_radius_percent: Some(25),
            max_width: Some(512),
            minimum_confidence_basis_points: None,
        })
        .unwrap();
    let structured = result.structured_content.clone().unwrap();
    assert_ne!(
        result.is_error,
        Some(true),
        "tracking refused: {structured}"
    );
    // The window really is the contract's: `2 · hw · scale · 100` is 26
    // percent of the width and 36 percent of the height at scale 1.
    assert_eq!(
        structured["coordinate_space"]["box_percent"],
        json!([26, 36])
    );
    assert_eq!(
        structured["coordinate_space"]["thumbnail"],
        json!({"width": TRACKED_SHOT_WIDTH, "height": TRACKED_SHOT_HEIGHT})
    );

    // `tracking_sample_frames(0..100, 5)` distributes 20 even intervals
    // across the 99-frame span: 0, 4, 9, …, 94, 99. Not multiples of five,
    // and the media fixture's sequence exactly.
    let observations = structured["observations"].as_array().unwrap();
    let sample_frames = observations
        .iter()
        .map(|observation| observation["local_frame"].as_i64().unwrap())
        .collect::<Vec<_>>();
    let expected_samples = std::iter::once(0)
        .chain((4..TRACKED_SHOT_FRAMES).step_by(5))
        .collect::<Vec<_>>();
    assert_eq!(expected_samples.len(), 21);
    assert_eq!(*expected_samples.last().unwrap(), 99);
    assert_eq!(
        sample_frames, expected_samples,
        "every sample must survive the confidence floor, at the media fixture's own frames"
    );

    // --- §9.2.11: the raw observations stay within 200 bp --------------
    let mut worst_raw = [0.0_f64; 2];
    let mut worst_raw_frame = [0_i64; 2];
    for observation in observations {
        let frame = observation["local_frame"].as_i64().unwrap();
        let analytic = tracked_shot_centre_basis_points(frame);
        for (axis, name) in [
            (0_usize, "center_x_basis_points"),
            (1, "center_y_basis_points"),
        ] {
            let observed = observation[name].as_i64().unwrap() as f64;
            let error = (observed - analytic[axis]).abs();
            assert!(
                error <= TRACKED_SHOT_RAW_TOLERANCE_BASIS_POINTS,
                "frame {frame}, axis {axis}: the raw observation {observed} bp misses the \
                 analytic {} bp by {error} bp, past §9.2.11's {} bp raw tolerance",
                analytic[axis],
                TRACKED_SHOT_RAW_TOLERANCE_BASIS_POINTS
            );
            if error > worst_raw[axis] {
                worst_raw[axis] = error;
                worst_raw_frame[axis] = frame;
            }
        }
        assert!(observation["confidence_basis_points"].as_u64().unwrap() >= 5_000);
    }

    // --- the smoothed curves the tool prepared -------------------------
    let curve_for = |axis: usize| {
        let name = if axis == 0 {
            "matte_window0_center_x_basis_points"
        } else {
            "matte_window0_center_y_basis_points"
        };
        serde_json::from_value::<AutomationCurve>(structured["curves"][name].clone())
            .expect("the tool publishes ordinary automation curves")
    };
    let curves = [curve_for(0), curve_for(1)];
    let mut worst_smoothed = [0.0_f64; 2];
    let mut worst_smoothed_frame = [0_i64; 2];
    for (axis, curve) in curves.iter().enumerate() {
        curve.validate().expect("a valid curve");
        assert_eq!(
            curve
                .keyframes
                .iter()
                .map(|keyframe| keyframe.at.0)
                .collect::<Vec<_>>(),
            expected_samples,
            "axis {axis}: one keyframe per surviving sample"
        );
        for keyframe in &curve.keyframes {
            assert_eq!(keyframe.interpolation, KeyframeInterpolation::Linear);
            let analytic = tracked_shot_centre_basis_points(keyframe.at.0);
            let error = (keyframe.value as f64 - analytic[axis]).abs();
            assert!(
                error <= TRACKED_SHOT_SMOOTHED_TOLERANCE_BASIS_POINTS,
                "frame {}, axis {axis}: the smoothed centre {} bp misses the analytic {} bp \
                 by {error} bp, past §9.2.11's {} bp smoothed tolerance",
                keyframe.at.0,
                keyframe.value,
                analytic[axis],
                TRACKED_SHOT_SMOOTHED_TOLERANCE_BASIS_POINTS
            );
            if error > worst_smoothed[axis] {
                worst_smoothed[axis] = error;
                worst_smoothed_frame[axis] = keyframe.at.0;
            }
        }
    }
    // The smoother is not a pass-through: it costs lag, and the lag is
    // what the margin budget below is spent on.
    assert!(
        worst_smoothed[0] > 0.0 && worst_smoothed[1] > 0.0,
        "a smoothed curve identical to ground truth would not exercise the margin budget"
    );

    // --- §9.2.11: containment at EVERY frame, not only the samples -----
    //
    // The window is `[cx ± 1300, cy ± 1800]` and the subject box is
    // `[analytic ± (625, 1111.1)]`, both in basis points of the frame
    // extent, so the margin on each axis collapses to
    // `half_extent − subject_half_extent − |centre error|`. The four edge
    // comparisons are written out anyway: containment is the assertion the
    // contract makes, and the margin is the evidence.
    let half_extent = [
        TRACKED_SHOT_HALF_WIDTH_BASIS_POINTS as f64,
        TRACKED_SHOT_HALF_HEIGHT_BASIS_POINTS as f64,
    ];
    let subject_half_extent = [
        TRACKED_SHOT_SUBJECT_HALF_WIDTH_BASIS_POINTS,
        TRACKED_SHOT_SUBJECT_HALF_HEIGHT_BASIS_POINTS,
    ];
    let budget = [
        TRACKED_SHOT_MARGIN_BUDGET_X_BASIS_POINTS,
        TRACKED_SHOT_MARGIN_BUDGET_Y_BASIS_POINTS,
    ];
    let mut worst_margin = [f64::INFINITY; 2];
    let mut worst_margin_frame = [0_i64; 2];
    let mut frames_asserted = 0_i64;
    for frame in 0..TRACKED_SHOT_FRAMES {
        let analytic = tracked_shot_centre_basis_points(frame);
        for axis in 0..2 {
            let centre = curves[axis]
                .value_at(TimeCode(frame))
                .expect("the curve covers the whole clip") as f64;
            let window = [centre - half_extent[axis], centre + half_extent[axis]];
            let subject = [
                analytic[axis] - subject_half_extent[axis],
                analytic[axis] + subject_half_extent[axis],
            ];
            assert!(
                subject[0] >= window[0] && subject[1] <= window[1],
                "frame {frame}, axis {axis}: the subject {subject:?} leaves the tracked \
                 window {window:?}"
            );
            let margin = (subject[0] - window[0]).min(window[1] - subject[1]);
            if margin < worst_margin[axis] {
                worst_margin[axis] = margin;
                worst_margin_frame[axis] = frame;
            }
        }
        frames_asserted += 1;
    }
    assert_eq!(
        frames_asserted, TRACKED_SHOT_FRAMES,
        "containment is asserted at every frame, interpolated between the 21 samples"
    );
    for axis in 0..2 {
        assert!(
            worst_margin[axis] > 0.0 && worst_margin[axis] <= budget[axis],
            "axis {axis}: the measured worst margin {} bp at frame {} must be positive and \
             inside §9.2.11's {} bp budget",
            worst_margin[axis],
            worst_margin_frame[axis],
            budget[axis]
        );
    }
    // The measured evidence, pinned. Every number below is a measurement
    // of the real tool on the real shot rather than arithmetic on the
    // contract's constants, so a regression in the tracker or in the
    // smoother moves it. The tracker is integer SAD over synthetic frames
    // and the curve evaluator is integer, so the run is exactly
    // reproducible and an exact comparison is honest.
    //
    // Both smoothed peaks and both margin minima land on frame 99, which
    // is §5.2's stated last-sample median substitution: the filter
    // replaces `o[n-1]` with `median(o[n-3], o[n-2], o[n-1])`, so the last
    // value lags a moving subject and spends the most margin.
    for (label, measured, recorded, frame, expected_frame) in [
        (
            "raw_x",
            worst_raw[0],
            TRACKED_SHOT_WORST_RAW_X_BASIS_POINTS,
            worst_raw_frame[0],
            9,
        ),
        (
            "raw_y",
            worst_raw[1],
            TRACKED_SHOT_WORST_RAW_Y_BASIS_POINTS,
            worst_raw_frame[1],
            19,
        ),
        (
            "smoothed_x",
            worst_smoothed[0],
            TRACKED_SHOT_WORST_SMOOTHED_X_BASIS_POINTS,
            worst_smoothed_frame[0],
            99,
        ),
        (
            "smoothed_y",
            worst_smoothed[1],
            TRACKED_SHOT_WORST_SMOOTHED_Y_BASIS_POINTS,
            worst_smoothed_frame[1],
            99,
        ),
        (
            "margin_x",
            worst_margin[0],
            TRACKED_SHOT_WORST_MARGIN_X_BASIS_POINTS,
            worst_margin_frame[0],
            99,
        ),
        (
            "margin_y",
            worst_margin[1],
            TRACKED_SHOT_WORST_MARGIN_Y_BASIS_POINTS,
            worst_margin_frame[1],
            99,
        ),
    ] {
        assert!(
            (measured - recorded).abs() <= 1.0e-6,
            "{label}: the measured value is {measured} bp, not the recorded {recorded} bp"
        );
        assert_eq!(
            frame, expected_frame,
            "{label}: the worst frame moved from {expected_frame} to {frame}"
        );
    }
    // The margin and the lag are one measurement seen twice, not two
    // independent literals: the sample frame carrying the worst lag is one
    // of the hundred frames checked above, so the worst margin can never
    // exceed the budget less that lag. Here the two are equal to the bp,
    // because the worst lag falls on sample frame 99 rather than on an
    // interpolated frame between two samples.
    for axis in 0..2 {
        assert!(
            worst_margin[axis] <= budget[axis] - worst_smoothed[axis] + 1.0e-6,
            "axis {axis}: a worst margin of {} bp is larger than the {} bp budget less the \
             {} bp worst lag, which no frame can be",
            worst_margin[axis],
            budget[axis],
            worst_smoothed[axis]
        );
    }
}

/// CC5 §5.2: fewer than two samples above the confidence floor is the
/// roadmap's manual fallback, reported typed with field/observed/allowed.
#[test]
fn track_matte_window_refuses_when_confidence_is_too_low() {
    // Every frame carries a completely different deterministic pattern, so
    // no template matches its successor and the confidence floor rejects
    // every sample after the seeded first one. This is the shape of a real
    // failure: the tracker has no occlusion handling, so a subject that
    // vanishes leaves nothing to match.
    let frames = (0..=40)
        .map(|frame| (TimeCode(frame), matte_noise_frame(frame)))
        .collect::<BTreeMap<_, _>>();
    let (service, _core) = matte_track_service(frames, BTreeMap::new(), Vec::new());

    let result = service
        .track_matte_window(&TrackMatteWindowArgs {
            expected_revision: None,
            clip_id: ClipId(1),
            effect_id: EffectId(1),
            window_index: 0,
            start_local_frame: Some(TimeCode(0)),
            end_local_frame: Some(TimeCode(41)),
            step_frames: Some(10),
            search_radius_percent: Some(25),
            max_width: Some(320),
            // Only a perfect match survives, which the seeded first sample
            // alone reports.
            minimum_confidence_basis_points: Some(10_000),
        })
        .unwrap();

    let structured = result.structured_content.unwrap();
    assert_eq!(
        result.is_error,
        Some(true),
        "expected a refusal: {structured}"
    );
    assert_eq!(structured["code"], "tracking_confidence_too_low");
    let details = &structured["details"];
    assert_eq!(details["field"], "minimum_confidence_basis_points");
    assert_eq!(
        details["observed"]["minimum_confidence_basis_points"],
        10_000
    );
    assert_eq!(details["allowed"], json!({"minimum_surviving_samples": 2}));
    assert!(details["observed"]["surviving_samples"].as_u64().unwrap() < 2);
    assert!(
        details["recovery_action"]
            .as_str()
            .unwrap()
            .contains("will not invent a position")
    );
}

/// CC5 §5.2: the composite → layer conversion is a single affine map, so a
/// layer whose transform moves across the range is a typed refusal.
#[test]
fn track_matte_window_refuses_a_keyframed_layer_transform() {
    let mut transform = Effect {
        id: EffectId(2),
        name: "transform".to_owned(),
        parameters: BTreeMap::new(),
        keyframes: BTreeMap::new(),
    };
    transform.keyframes.insert(
        "scale_percent".to_owned(),
        AutomationCurve {
            keyframes: vec![
                Keyframe {
                    at: TimeCode(0),
                    value: 50,
                    interpolation: KeyframeInterpolation::Linear,
                },
                Keyframe {
                    at: TimeCode(40),
                    value: 100,
                    interpolation: KeyframeInterpolation::Linear,
                },
            ],
        },
    );
    let frames = BTreeMap::from([(TimeCode(0), matte_box_frame([160, 90]))]);
    let (service, _core) = matte_track_service(frames, BTreeMap::new(), vec![transform]);

    let result = service
        .track_matte_window(&TrackMatteWindowArgs {
            expected_revision: None,
            clip_id: ClipId(1),
            effect_id: EffectId(1),
            window_index: 0,
            start_local_frame: Some(TimeCode(0)),
            end_local_frame: Some(TimeCode(41)),
            step_frames: Some(10),
            search_radius_percent: None,
            max_width: None,
            minimum_confidence_basis_points: None,
        })
        .unwrap();

    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.unwrap();
    assert_eq!(
        structured["code"],
        "matte_track_layer_transform_unsupported"
    );
    let details = &structured["details"];
    assert_eq!(details["field"], "scale");
    assert_eq!(details["observed"]["at_first_sample"], 0.5);
    assert!(
        details["allowed"]
            .as_str()
            .unwrap()
            .contains("one value across the whole tracked range")
    );
    // LOW C: the window is tracked with one fixed-size template, so the
    // contract asks for a static transform to keep the window
    // reproducible. It is *not* that a per-frame conversion is impossible
    // — `track_mask_region` and `track_reframe_subject` both do one.
    let recovery = details["recovery_action"].as_str().unwrap();
    assert!(
        recovery.contains("one template of one fixed size"),
        "the rationale must name the fixed template: {recovery}"
    );
    assert!(
        recovery.contains("reproducible"),
        "the rationale must name reproducibility: {recovery}"
    );
    assert!(
        !recovery.contains("single affine map"),
        "the false rationale must be gone: {recovery}"
    );
}

/// CC5 §2.2: a window at index >= `matte_window_count` is stored but never
/// rendered, so tracking it would animate geometry that affects no pixel.
#[test]
fn track_matte_window_refuses_a_window_past_the_active_count() {
    let frames = BTreeMap::from([(TimeCode(0), matte_box_frame([160, 90]))]);
    let (service, _core) = matte_track_service(frames, BTreeMap::new(), Vec::new());

    let result = service
        .track_matte_window(&TrackMatteWindowArgs {
            expected_revision: None,
            clip_id: ClipId(1),
            effect_id: EffectId(1),
            // The fixture node resolves `matte_window_count = 1`.
            window_index: 2,
            start_local_frame: None,
            end_local_frame: None,
            step_frames: None,
            search_radius_percent: None,
            max_width: None,
            minimum_confidence_basis_points: None,
        })
        .unwrap();

    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.unwrap();
    assert_eq!(structured["code"], "matte_window_not_active");
    assert_eq!(structured["details"]["field"], "window_index");
    assert_eq!(structured["details"]["observed"], 2);
    assert_eq!(structured["details"]["allowed"]["window_count"], 1);
}

/// CC5 §5.2: `excluded_effect` narrows the tracker's exclusion from *every*
/// effect sharing a name to exactly the one being tracked.
///
/// Two `mask` effects on one clip: tracking the first must leave the
/// second's alpha in the tracking thumbnails, which is the correct
/// behaviour and the delta CC5 §9.2.12 asserts.
#[test]
fn region_tracking_excludes_exactly_one_effect_by_id() {
    let frames = BTreeMap::from([
        (TimeCode(0), matte_box_frame([160, 90])),
        (TimeCode(10), matte_box_frame([160, 90])),
    ]);
    let masks = vec![
        Effect {
            id: EffectId(7),
            name: "mask".to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::new(),
        },
        Effect {
            id: EffectId(8),
            name: "mask".to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::new(),
        },
    ];
    let (service, _core) = matte_track_service(frames, BTreeMap::new(), masks);
    let (_, document) = service.snapshot().unwrap();

    let request = |excluded: EffectId| RegionTrackingRequest {
        document: &document,
        clip_id: ClipId(1),
        clip_timeline_start: TimeCode::ZERO,
        sample_frames: &[TimeCode(0), TimeCode(10)],
        center_percent: [50, 50],
        box_percent: [25, 25],
        search_radius_percent: 25,
        max_width: 320,
        excluded_effect: excluded,
    };
    // Both calls succeed; the point is that the *identity* selects which
    // effect is removed, so a second effect of the same name survives.
    assert!(service.track_clip_region(&request(EffectId(7))).is_ok());
    assert!(service.track_clip_region(&request(EffectId(8))).is_ok());
    // The document itself is never touched by tracking isolation.
    assert_eq!(
        document
            .clip(ClipId(1))
            .unwrap()
            .effects
            .iter()
            .filter(|effect| effect.name == "mask")
            .count(),
        2
    );
}

/// CC5 §2.6: a qualifier band whose low edge resolved above its high edge
/// selects nothing, and `get_qa_report` surfaces Core's
/// `matte_band_inverted_by_automation` issue for it.
#[test]
fn qa_report_surfaces_an_inverted_matte_band() {
    let (service, _core) = matte_service_with(
        None,
        BTreeMap::from([
            ("matte_qualifier_enabled".to_owned(), 1),
            ("matte_saturation_low_basis_points".to_owned(), 9_000),
            ("matte_saturation_high_basis_points".to_owned(), 1_000),
        ]),
        Vec::new(),
    );

    let report = service.qa_report().unwrap();
    let text = report.content[0].as_text().unwrap().text.clone();
    let report: serde_json::Value = serde_json::from_str(&text).unwrap();
    let issue = report["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|issue| issue["code"] == "matte_band_inverted_by_automation")
        .unwrap_or_else(|| panic!("the inverted band must be reported: {report}"));
    assert!(
        issue["message"]
            .as_str()
            .unwrap()
            .contains("selects nothing")
    );

    // A band that is not inverted produces no issue at all.
    let (clean, _core) = matte_service_with(
        None,
        BTreeMap::from([
            ("matte_qualifier_enabled".to_owned(), 1),
            ("matte_saturation_low_basis_points".to_owned(), 1_000),
            ("matte_saturation_high_basis_points".to_owned(), 9_000),
        ]),
        Vec::new(),
    );
    let text = clean.qa_report().unwrap().content[0]
        .as_text()
        .unwrap()
        .text
        .clone();
    let clean_report: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(
        !clean_report["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["code"] == "matte_band_inverted_by_automation")
    );
}

/// CC5 §7: the three matte tools are registered, read-only, and
/// `inspect_grade_matte` is an Inspector by explicit override because the
/// `inspect_` prefix matches no inference rule.
///
/// CC6 §7 extends the same registry bookkeeping to `get_color_qc`, which
/// needs **no** `CAPABILITY_KIND_OVERRIDES` entry because `get_` already
/// infers `Inspector` — asserted below so the omission is a decision.
#[test]
#[allow(clippy::too_many_lines)]
fn cc5_matte_tools_are_registered_read_only_inspectors() {
    let tools = KinewrightMcp::tools().unwrap();
    let names = tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<BTreeSet<_>>();
    for name in [
        "inspect_grade_matte",
        "track_matte_window",
        "plan_secondary_correction",
        "get_color_qc",
    ] {
        assert!(names.contains(name), "missing CC5 tool {name}");
        let tool = tools.iter().find(|tool| tool.name == name).unwrap();
        assert_eq!(
            tool.annotations.as_ref().unwrap().read_only_hint,
            Some(true),
            "{name} must be read-only"
        );
    }
    assert_eq!(crate::schema::INSPECTOR_TOOL_NAMES.len(), 75);

    // M36: every colour planner and every CC5 tool stays inside the
    // kilobyte description budget, measured on the *registered* descriptor
    // rather than on a copy of the literal, so a descriptor-derived legend
    // that grows is caught here. `plan_secondary_correction` carries a
    // pointer to the matte legend, not the legend itself; the four other
    // planners carry only `matte_parameter_pointer`.
    for name in [
        "plan_primary_correction",
        "plan_color_wheels",
        "plan_color_curves",
        "plan_creative_look",
        "plan_technical_lut",
        "plan_secondary_correction",
        "inspect_grade_matte",
        "track_matte_window",
        "track_mask_region",
        "track_reframe_subject",
        "get_color_qc",
    ] {
        let tool = tools.iter().find(|tool| tool.name == name).unwrap();
        let description = tool.description.as_deref().unwrap_or_default();
        assert!(
            description.len() < 1_024,
            "{name} description is {} bytes, over the M36 1 KB budget",
            description.len()
        );
    }
    // The matte's own planner must not repeat the 47-parameter legend, and
    // must not recommend itself.
    let secondary = tools
        .iter()
        .find(|tool| tool.name == "plan_secondary_correction")
        .unwrap();
    let secondary = secondary.description.as_deref().unwrap_or_default();
    assert!(
        !secondary.contains("matte_window{j}_*"),
        "plan_secondary_correction must not carry the full matte legend"
    );
    assert!(
        !secondary.contains("Prefer plan_secondary_correction"),
        "plan_secondary_correction must not recommend itself"
    );
    assert!(secondary.contains("details.matte_parameters"));
    // The legend itself is still served, in full, by the two enumerating
    // surfaces the pointer names.
    for name in ["add_effect", "set_effect_param"] {
        let tool = tools.iter().find(|tool| tool.name == name).unwrap();
        assert!(
            tool.description
                .as_deref()
                .unwrap_or_default()
                .contains("matte_window{j}_*"),
            "{name} must still enumerate the matte legend"
        );
    }

    let capabilities = crate::runtime::capabilities(&tools);
    let kind = |name: &str| {
        capabilities
            .iter()
            .find(|capability| capability.name == name)
            .unwrap_or_else(|| panic!("{name} must be a capability"))
            .kind
    };
    assert_eq!(
        kind("inspect_grade_matte"),
        crate::runtime::CapabilityKind::Inspector
    );
    // These two are inferred correctly by their name prefixes and need no
    // override entry.
    assert_eq!(
        kind("track_matte_window"),
        crate::runtime::CapabilityKind::Inspector
    );
    assert_eq!(
        kind("plan_secondary_correction"),
        crate::runtime::CapabilityKind::Planner
    );
    // CC6 §7: `get_` infers Inspector with no override entry.
    assert_eq!(
        kind("get_color_qc"),
        crate::runtime::CapabilityKind::Inspector
    );
    let color_qc = tools
        .iter()
        .find(|tool| tool.name == "get_color_qc")
        .unwrap();
    let annotations = color_qc.annotations.as_ref().unwrap();
    assert_eq!(annotations.read_only_hint, Some(true));
    assert_eq!(annotations.destructive_hint, Some(false));
    assert_eq!(annotations.idempotent_hint, Some(true));
    assert_eq!(annotations.open_world_hint, Some(false));
    // CC6 R13: a working-stage measurement is full-resolution or refused,
    // so the tool must not offer a resolution knob of any spelling.
    let schema = serde_json::to_value(color_qc.input_schema.as_ref()).unwrap();
    let properties = schema["properties"].as_object().unwrap();
    for absent in ["resolution", "proxy_sampling", "max_width"] {
        assert!(
            !properties.contains_key(absent),
            "get_color_qc must not carry {absent}"
        );
    }
    assert_eq!(schema["additionalProperties"], serde_json::json!(false));
}
