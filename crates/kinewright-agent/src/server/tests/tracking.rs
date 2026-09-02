//! CC5 §5.2 mask and reframe tracking tests.

use super::mattes::{matte_box_frame, matte_track_service};
use super::*;
use crate::server::mattes::{LayerTransform, tracked_box_percent};
use crate::server::tracking::layer_subject_bounds;

// -----------------------------------------------------------------------
// CC5 §5.2, the mask and reframe halves.
//
// `track_mask_region` and `track_reframe_subject` measure the *composited*
// thumbnail and write controls the compositor evaluates in *layer* uv, so
// both need the same composite → layer conversion `track_matte_window`
// already does. The analysis double answers the composited thumbnail the
// real compositor would produce, with the subject drawn at the position the
// shader's forward map puts it — the double ignores the document, so the
// placement is stated here by hand rather than rendered.
// -----------------------------------------------------------------------

/// A 320 × 180 frame carrying one white box of half extent `half` pixels
/// centred on `centre`, over `matte_box_frame`'s dark background.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
fn transform_box_frame(centre: [i64; 2], half: i64) -> RgbaImage {
    let (width, height) = (320_i64, 180_i64);
    let mut pixels = vec![0_u8; (width * height * 4) as usize];
    for pixel in pixels.as_chunks_mut::<4>().0.iter_mut() {
        *pixel = [48, 48, 48, 255];
    }
    for y in (centre[1] - half).max(0)..(centre[1] + half).min(height) {
        for x in (centre[0] - half).max(0)..(centre[0] + half).min(width) {
            let index = ((y * width + x) * 4) as usize;
            pixels[index..index + 4].copy_from_slice(&[235, 235, 235, 255]);
        }
    }
    RgbaImage {
        width: 320,
        height: 180,
        pixels,
    }
}

/// A service over the fixture's 320 × 180, 30 fps, 60-frame media clip
/// carrying `effects`, whose analysis double answers `frames`.
fn transform_track_service(
    effects: Vec<Effect>,
    frames: BTreeMap<TimeCode, RgbaImage>,
) -> (KinewrightMcp, Core) {
    let (seed, playback, _) = fixture();
    let Event::QueryResult(QueryResult::Document(seed_document)) =
        seed.request(Command::Query(Query::Document)).unwrap()
    else {
        panic!("expected fixture document");
    };
    let mut document = (*seed_document).clone();
    document.tracks[0].clips[0].effects = effects;
    let media = Arc::new(NoopMedia {
        thumbnail_frames: frames,
        ..NoopMedia::default()
    });
    let core = Core::spawn(document).unwrap();
    let service = KinewrightMcp::new(core.clone(), playback, media, ConfirmationBroker::default());
    (service, core)
}

/// CC5 §5.2's worked transform: `scale_percent 50`, `x_percent 20`,
/// `y_percent 20`.
///
/// The compositor accumulates `scale = 50 / 100 = 0.5` and
/// `offset = 20 / 50 = 0.4` on both axes, so the shader's placement is
/// `u_composite = 0.5·(u_layer − 0.5) + 0.4/2 + 0.5 = 0.5·u_layer + 0.45`
/// and its inverse is `u_layer = 2·u_composite − 0.9`.
fn half_scale_transform() -> Effect {
    Effect {
        id: EffectId(9),
        name: "transform".to_owned(),
        parameters: BTreeMap::from([
            ("scale_percent".to_owned(), ParamValue::Integer(50)),
            ("x_percent".to_owned(), ParamValue::Integer(20)),
            ("y_percent".to_owned(), ParamValue::Integer(20)),
        ]),
        keyframes: BTreeMap::new(),
    }
}

/// A layer scale that ramps 100 → 50 percent, linearly, over frames 0..=40.
fn keyframed_scale_transform() -> Effect {
    Effect {
        id: EffectId(9),
        name: "transform".to_owned(),
        parameters: BTreeMap::from([("scale_percent".to_owned(), ParamValue::Integer(100))]),
        keyframes: BTreeMap::from([(
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
        )]),
    }
}

/// A bounded mask at `center` percent with a `size` percent region.
fn tracking_mask_effect(center: [i64; 2], size: [i64; 2]) -> Effect {
    Effect {
        id: EffectId(1),
        name: "mask".to_owned(),
        parameters: BTreeMap::from([
            ("shape_token".to_owned(), ParamValue::Integer(1)),
            (
                "center_x_percent".to_owned(),
                ParamValue::Integer(center[0]),
            ),
            (
                "center_y_percent".to_owned(),
                ParamValue::Integer(center[1]),
            ),
            ("width_percent".to_owned(), ParamValue::Integer(size[0])),
            ("height_percent".to_owned(), ParamValue::Integer(size[1])),
        ]),
        keyframes: BTreeMap::new(),
    }
}

/// A 1:1 reframe whose focus starts at `focus` percent. The source is
/// 320 × 180, so a 10000 bp target crops *horizontally* to a 5625 bp
/// window and leaves the vertical axis whole.
fn tracking_reframe_effect(focus: [i64; 2]) -> Effect {
    Effect {
        id: EffectId(1),
        name: "reframe".to_owned(),
        parameters: BTreeMap::from([
            (
                "target_aspect_basis_points".to_owned(),
                ParamValue::Integer(10_000),
            ),
            ("focus_x_percent".to_owned(), ParamValue::Integer(focus[0])),
            ("focus_y_percent".to_owned(), ParamValue::Integer(focus[1])),
        ]),
        keyframes: BTreeMap::new(),
    }
}

/// The keyframe values one prepared curve carries, in order.
fn curve_values(structured: &serde_json::Value, name: &str) -> Vec<i64> {
    structured["curves"][name]["keyframes"]
        .as_array()
        .unwrap_or_else(|| panic!("curve {name} must be prepared"))
        .iter()
        .map(|keyframe| keyframe["value"].as_i64().unwrap())
        .collect()
}

/// One field of every observation, in order.
fn observation_values(structured: &serde_json::Value, key: &str, field: &str) -> Vec<i64> {
    structured[key]
        .as_array()
        .unwrap_or_else(|| panic!("{key} must be published"))
        .iter()
        .map(|sample| sample[field].as_i64().unwrap())
        .collect()
}

/// The five sample frames every test below uses: 0, 10, 20, 30, 40.
const TRANSFORM_TRACK_SAMPLES: [i64; 5] = [0, 10, 20, 30, 40];

fn mask_tracking_args() -> TrackMaskArgs {
    TrackMaskArgs {
        clip_id: ClipId(1),
        effect_id: EffectId(1),
        start_local_frame: Some(TimeCode(0)),
        end_local_frame: Some(TimeCode(41)),
        step_frames: Some(10),
        // A 5 percent radius is a 16 pixel horizontal search, whose coarse
        // grid lands exactly on a subject moving 8 pixels a sample, so the
        // template match is pixel-exact rather than plateaued.
        search_radius_percent: Some(5),
        max_width: Some(320),
    }
}

/// CC5 §5.2 (a): at the identity transform the written mask centres are the
/// analytic box centre, read as a *fraction of the extent*.
///
/// The subject centre is composite pixel `x = 140 + 0.8·frame`, so the
/// samples land on 140, 148, 156, 164 and 172 of 320 and
/// `round((pixel + 0.5) · 100 / 320)` is 44, 46, 49, 51, 54 — the analytic
/// box centre read as a fraction of the extent. The vertical axis is static
/// at pixel 90 of 180: `round(90.5 · 100 / 180) = 50`.
#[test]
fn track_mask_region_writes_layer_space_centres_at_the_identity() {
    let frames = (0..60)
        .map(|frame| {
            (
                TimeCode(frame),
                transform_box_frame([140 + frame * 4 / 5, 90], 5),
            )
        })
        .collect::<BTreeMap<_, _>>();
    // Seed on the subject: 44 percent of 320 is pixel 140 exactly. The
    // region is deliberately small — a 6 × 11 percent region is a 21 × 21
    // pixel template, and `track_region` subsamples a template that size
    // every pixel, so the match is exact rather than plateaued.
    let (service, core) =
        transform_track_service(vec![tracking_mask_effect([44, 50], [6, 11])], frames);

    let result = service.track_mask_region(&mask_tracking_args()).unwrap();
    let structured = result.structured_content.clone().unwrap();
    assert_ne!(
        result.is_error,
        Some(true),
        "tracking refused: {structured}"
    );

    assert_eq!(
        curve_values(&structured, "center_x_percent"),
        vec![44, 46, 49, 51, 54]
    );
    assert_eq!(
        curve_values(&structured, "center_y_percent"),
        vec![50, 50, 50, 50, 50]
    );
    // The observations carry the same layer values under both names.
    assert_eq!(
        observation_values(&structured, "observations", "center_x_percent"),
        observation_values(&structured, "observations", "layer_center_x_percent"),
    );
    // At the identity the layer and the composite readings agree to the
    // percent, so nothing about this shot could hide a missing conversion —
    // which is exactly why the transformed cases below exist. They agree
    // *exactly*, on every observation and both axes, only because the
    // composite provenance is read with the same fraction-of-the-extent
    // convention `coordinate_space.pixel_to_unit` publishes: on the
    // `extent − 1` lattice pixel 172 of 320 would read 54 against the same
    // 54 here but pixel 32 of 64 would read 51 against 51 only by luck, and
    // the identity would stop being an identity in general.
    for axis in ["x", "y"] {
        assert_eq!(
            observation_values(
                &structured,
                "observations",
                &format!("composite_center_{axis}_percent"),
            ),
            observation_values(
                &structured,
                "observations",
                &format!("layer_center_{axis}_percent"),
            ),
            "at the identity the composite provenance must equal the written layer value on {axis}",
        );
    }
    assert_eq!(structured["coordinate_space"]["samples"][0]["scale"], 1.0);
    assert_eq!(
        structured["coordinate_space"]["box_percent"],
        json!([6, 11])
    );
    assert_eq!(
        structured["coordinate_space"]["unit_to_percent"],
        "center_percent = round(u_layer * 100), clamped to 0..=100"
    );

    // Nothing is committed.
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
        "track_mask_region commits nothing"
    );
}

/// CC5 §5.2 (b): under a static `scale 50 / x 20 / y 20` layer transform the
/// written mask centres are the *layer*-space centres, not the composite
/// ones the tracker measured.
///
/// The subject sits at layer `u = 0.103125, 0.153125, 0.203125, 0.253125,
/// 0.303125`, which the forward map `u_c = 0.5·u_l + 0.45` puts at composite
/// `u = 0.5015625, 0.5265625, 0.5515625, 0.5765625, 0.6015625`, i.e. pixel
/// centres 160, 168, 176, 184 and 192 of 320 — where the fixture draws it.
/// Converting back with `u_l = 2·u_c − 0.9` gives 10, 15, 20, 25, 30
/// percent. The unconverted composite reading is 50, 53, 55, 58, 60
/// percent, so this test fails by tens of percent if the conversion is
/// removed.
#[test]
fn track_mask_region_converts_the_composite_centre_into_layer_space() {
    let frames = (0..60)
        .map(|frame| {
            (
                TimeCode(frame),
                transform_box_frame([160 + frame * 4 / 5, 125], 5),
            )
        })
        .collect::<BTreeMap<_, _>>();
    // Seed on the subject through the forward map: layer 10 percent is
    // composite 0.5·0.10 + 0.45 = 0.50, which is pixel 160 of 320. Layer 49
    // percent is composite 0.695, which is pixel 125 of 180.
    let (service, core) = transform_track_service(
        vec![
            half_scale_transform(),
            tracking_mask_effect([10, 49], [12, 22]),
        ],
        frames,
    );

    let result = service.track_mask_region(&mask_tracking_args()).unwrap();
    let structured = result.structured_content.clone().unwrap();
    assert_ne!(
        result.is_error,
        Some(true),
        "tracking refused: {structured}"
    );

    // The written, layer-space curve.
    let written = curve_values(&structured, "center_x_percent");
    for (index, expected) in [10_i64, 15, 20, 25, 30].iter().enumerate() {
        assert!(
            (written[index] - expected).abs() <= 2,
            "sample {index}: wrote {} against the analytic layer {expected}",
            written[index]
        );
    }
    // 49.4444 percent of the layer, from composite pixel 125 of 180.
    for value in curve_values(&structured, "center_y_percent") {
        assert!(
            (value - 49).abs() <= 2,
            "vertical layer centre {value} against the analytic 49"
        );
    }
    // The raw composite reading is preserved as provenance, and is nowhere
    // near the written value: this is the whole point of the conversion.
    let composite = observation_values(&structured, "observations", "composite_center_x_percent");
    assert_eq!(composite, vec![50, 53, 55, 58, 60]);
    assert_eq!(
        observation_values(&structured, "observations", "layer_center_x_percent"),
        written
    );
    // The template is the stored region rescaled by the layer scale:
    // 12 × 0.5 = 6 and 22 × 0.5 = 11 percent of the composite.
    assert_eq!(
        structured["coordinate_space"]["box_percent"],
        json!([6, 11])
    );
    // Seeded through the forward map: layer 10/49 percent is composite
    // 50/70 percent.
    assert_eq!(
        structured["coordinate_space"]["seed_center_percent"],
        json!([50, 70])
    );
    let samples = structured["coordinate_space"]["samples"]
        .as_array()
        .unwrap();
    assert_eq!(samples.len(), TRANSFORM_TRACK_SAMPLES.len());
    for sample in samples {
        assert_eq!(sample["scale"], 0.5);
        assert_eq!(sample["offset_x"], 0.4);
        assert_eq!(sample["offset_y"], 0.4);
    }

    // The plan is still exactly two non-destructive keyframe operations,
    // and nothing is committed.
    let preview = &structured["prepared_edit_plan"]["preview"];
    assert_eq!(preview["operation_count"], 2);
    assert_eq!(preview["destructive_operations"], json!([]));
    assert_eq!(preview["before_clips"], preview["after_clips"]);
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
        "track_mask_region commits nothing"
    );
}

/// CC5 §5.2 (c): a *keyframed* layer scale is converted sample by sample
/// rather than refused.
///
/// The scale ramps 100 → 50 percent over frames 0..=40, so at the samples it
/// is 1.0, 0.875, 0.75, 0.625, 0.5 and a subject pinned at layer `u = 0.25`
/// walks across the composite: `u_c = s·(0.25 − 0.5) + 0.5` is 0.25,
/// 0.28125, 0.3125, 0.34375, 0.375, i.e. pixel centres 80, 90, 100, 110 and
/// 120 of 320. Per-frame conversion recovers 25 percent at every sample; a
/// single static transform, or none at all, would report 25, 28, 31, 34, 38.
#[test]
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn track_mask_region_converts_a_keyframed_layer_transform_per_frame() {
    let frames = (0..60)
        .map(|frame| {
            let scale = 1.0 - 0.5 * (frame as f64) / 40.0;
            // The layer shrinks, so the subject drawn on the composite
            // shrinks with it: half of 40 px times the scale.
            let half = (5.0 * scale).round() as i64;
            (
                TimeCode(frame),
                transform_box_frame([80 + frame, 90], half.max(2)),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let (service, _core) = transform_track_service(
        vec![
            keyframed_scale_transform(),
            tracking_mask_effect([25, 50], [6, 11]),
        ],
        frames,
    );

    let result = service.track_mask_region(&mask_tracking_args()).unwrap();
    let structured = result.structured_content.clone().unwrap();
    assert_ne!(
        result.is_error,
        Some(true),
        "tracking refused: {structured}"
    );

    for (index, value) in curve_values(&structured, "center_x_percent")
        .into_iter()
        .enumerate()
    {
        assert!(
            (value - 25).abs() <= 2,
            "sample {index}: wrote {value} against the analytic layer 25"
        );
    }
    // The composite reading walks away from it — from about 25 percent to
    // about 37 — which is exactly what the per-frame conversion undoes.
    let composite = observation_values(&structured, "observations", "composite_center_x_percent");
    assert!(
        (composite[0] - 25).abs() <= 1,
        "the first composite reading is the seed: {composite:?}"
    );
    assert!(
        composite[4] - composite[0] >= 11,
        "the composite reading must drift as the layer shrinks: {composite:?}"
    );
    // One resolved transform per sample, and it moves.
    let samples = structured["coordinate_space"]["samples"]
        .as_array()
        .unwrap();
    assert_eq!(samples.len(), TRANSFORM_TRACK_SAMPLES.len());
    assert_eq!(samples[0]["local_frame"], 0);
    assert_eq!(samples[0]["scale"], 1.0);
    assert_eq!(samples[4]["local_frame"], 40);
    assert_eq!(samples[4]["scale"], 0.5);
    assert_eq!(structured["coordinate_space"]["per_frame_transform"], true);
}

/// CC5 §5.2 (d): the tracking template is the stored region *rescaled by the
/// layer scale*, so a region that is legal in layer space can still be an
/// illegal template on the composite — and the refusal says so.
#[test]
fn track_mask_region_refuses_a_template_the_layer_scale_pushes_out_of_range() {
    let frames = (0..60)
        .map(|frame| (TimeCode(frame), transform_box_frame([160, 90], 20)))
        .collect::<BTreeMap<_, _>>();
    let doubled = Effect {
        id: EffectId(9),
        name: "transform".to_owned(),
        parameters: BTreeMap::from([("scale_percent".to_owned(), ParamValue::Integer(200))]),
        keyframes: BTreeMap::new(),
    };
    // 50 percent of the layer is a legal mask, and 50 × 2 = 100 percent of
    // the composite is not a legal template.
    let (service, _core) = transform_track_service(
        vec![doubled, tracking_mask_effect([50, 50], [50, 50])],
        frames,
    );

    let result = service.track_mask_region(&mask_tracking_args()).unwrap();
    assert_eq!(result.is_error, Some(true));
    let message = result
        .content
        .iter()
        .find_map(|content| content.as_text().map(|text| text.text.clone()))
        .unwrap_or_default();
    assert!(
        message.contains("layer scale 2"),
        "the refusal must name the layer scale: {message}"
    );
    assert!(
        message.contains("100x100 percent template"),
        "the refusal must name the composite template: {message}"
    );
}

fn reframe_tracking_args(subject: [u8; 2], initial: [u8; 2]) -> TrackReframeArgs {
    TrackReframeArgs {
        clip_id: ClipId(1),
        effect_id: EffectId(1),
        subject_width_percent: subject[0],
        subject_height_percent: subject[1],
        initial_subject_x_percent: Some(initial[0]),
        initial_subject_y_percent: Some(initial[1]),
        minimum_focus_x_percent: None,
        maximum_focus_x_percent: None,
        minimum_focus_y_percent: None,
        maximum_focus_y_percent: None,
        focus_dead_zone_percent: Some(0),
        maximum_focus_step_percent: Some(25),
        start_local_frame: Some(TimeCode(0)),
        end_local_frame: Some(TimeCode(41)),
        step_frames: Some(10),
        search_radius_percent: Some(5),
        max_width: Some(320),
    }
}

/// CC5 §5.2 (a), reframe half: at the identity the planned focus is the
/// analytic subject centre, read as a fraction of the extent.
///
/// Composite pixel centres 140, 148, 156, 164 and 172 of 320 are
/// `round((pixel + 0.5) · 10000 / 320)` = 4391, 4641, 4891, 5141, 5391 bp.
#[test]
fn track_reframe_subject_writes_layer_space_focus_at_the_identity() {
    let frames = (0..60)
        .map(|frame| {
            (
                TimeCode(frame),
                transform_box_frame([140 + frame * 4 / 5, 90], 5),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let (service, core) = transform_track_service(vec![tracking_reframe_effect([44, 50])], frames);

    let result = service
        .track_reframe_subject(&reframe_tracking_args([6, 11], [44, 50]))
        .unwrap();
    let structured = result.structured_content.clone().unwrap();
    assert_ne!(
        result.is_error,
        Some(true),
        "tracking refused: {structured}"
    );

    let layer = observation_values(&structured, "subject_samples", "layer_x_basis_points");
    for (index, expected) in [4_391_i64, 4_641, 4_891, 5_141, 5_391].iter().enumerate() {
        assert!(
            (layer[index] - expected).abs() <= 200,
            "sample {index}: converted {} against the analytic {expected}",
            layer[index]
        );
    }
    // At the identity the composite and the layer reading are the same
    // number, which is why the transformed case below is the real gate.
    assert_eq!(
        observation_values(&structured, "subject_samples", "composite_x_basis_points"),
        layer
    );
    assert_eq!(structured["coordinate_space"]["samples"][0]["scale"], 1.0);

    // The planned focus follows the subject: the three-sample median lags a
    // ramp by one inter-sample step, which is 312 bp here.
    let focus = observation_values(&structured, "focus_keyframes", "x_basis_points");
    for (index, expected) in layer.iter().enumerate() {
        assert!(
            (focus[index] - expected).abs() <= 700,
            "sample {index}: focus {} against the subject {expected}",
            focus[index]
        );
    }

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
        "track_reframe_subject commits nothing"
    );
    assert!(
        document.markers.is_empty(),
        "the provenance marker is prepared, not committed"
    );
}

/// CC5 §5.2 (b), reframe half: under `scale 50 / x 20 / y 20` the planner is
/// fed *layer*-space subject centres.
///
/// The fixture draws the subject at composite pixel centres 207, 215, 223,
/// 231 and 239 of 320, which are composite 6484, 6734, 6984, 7234 and 7484
/// bp; `u_l = 2·u_c − 0.9` makes them layer 3969, 4469, 4969, 5469 and 5969
/// bp. Without the conversion the planner would see the composite numbers,
/// which are 2500 bp away.
#[test]
fn track_reframe_subject_converts_the_composite_centre_into_layer_space() {
    let frames = (0..60)
        .map(|frame| {
            (
                TimeCode(frame),
                transform_box_frame([207 + frame * 4 / 5, 125], 5),
            )
        })
        .collect::<BTreeMap<_, _>>();
    // Layer 40 percent is composite 0.5·0.40 + 0.45 = 0.65, which seeds at
    // pixel 207 of 320; layer 49 percent is composite 0.695, pixel 125.
    let (service, core) = transform_track_service(
        vec![half_scale_transform(), tracking_reframe_effect([40, 49])],
        frames,
    );

    let result = service
        .track_reframe_subject(&reframe_tracking_args([12, 22], [40, 49]))
        .unwrap();
    let structured = result.structured_content.clone().unwrap();
    assert_ne!(
        result.is_error,
        Some(true),
        "tracking refused: {structured}"
    );

    let layer = observation_values(&structured, "subject_samples", "layer_x_basis_points");
    for (index, expected) in [3_969_i64, 4_469, 4_969, 5_469, 5_969].iter().enumerate() {
        assert!(
            (layer[index] - expected).abs() <= 200,
            "sample {index}: converted {} against the analytic layer {expected}",
            layer[index]
        );
    }
    // The composite provenance is preserved, and is thousands of basis
    // points away from what was planned.
    let composite = observation_values(&structured, "subject_samples", "composite_x_basis_points");
    for (index, expected) in [6_484_i64, 6_734, 6_984, 7_234, 7_484].iter().enumerate() {
        assert!(
            (composite[index] - expected).abs() <= 200,
            "sample {index}: composite {} against the analytic {expected}",
            composite[index]
        );
        // The gap runs 2515 bp at the first sample down to 1515 at the
        // last, because the layer moves twice as far as the composite at
        // scale 0.5. Either end is far outside any tracker error.
        assert!(
            (composite[index] - layer[index]).abs() > 1_400,
            "the two spaces must not coincide, or this test proves nothing"
        );
    }
    // 49.4444 percent of the layer, from composite pixel 125 of 180.
    for value in observation_values(&structured, "subject_samples", "layer_y_basis_points") {
        assert!(
            (value - 4_944).abs() <= 200,
            "vertical layer centre {value} against the analytic 4944"
        );
    }
    // The subject template is rescaled onto the composite: 12 × 0.5 = 6 and
    // 22 × 0.5 = 11 percent.
    assert_eq!(
        structured["coordinate_space"]["box_percent"],
        json!([6, 11])
    );
    assert_eq!(
        structured["coordinate_space"]["seed_center_percent"],
        json!([65, 70])
    );
    assert_eq!(structured["coordinate_space"]["samples"][0]["scale"], 0.5);
    assert_eq!(
        structured["coordinate_space"]["samples"][0]["offset_x"],
        0.4
    );

    // The focus is planned in the same space it is written in, so it stays
    // near the layer-space subject and far from the composite reading.
    let focus = observation_values(&structured, "focus_keyframes", "x_basis_points");
    for (index, expected) in layer.iter().enumerate() {
        assert!(
            (focus[index] - expected).abs() <= 900,
            "sample {index}: focus {} against the layer subject {expected}",
            focus[index]
        );
    }

    // Nothing is committed: no keyframes, no provenance marker.
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
        "track_reframe_subject commits nothing"
    );
    assert!(document.markers.is_empty());
}

/// CC5 §5.2 (d), reframe half: the subject template is rescaled by the layer
/// scale, and a subject that maps past 75 percent of the composite is
/// refused with both numbers named.
#[test]
fn track_reframe_subject_refuses_a_subject_the_layer_scale_pushes_out_of_range() {
    let frames = (0..60)
        .map(|frame| (TimeCode(frame), transform_box_frame([160, 90], 20)))
        .collect::<BTreeMap<_, _>>();
    let doubled = Effect {
        id: EffectId(9),
        name: "transform".to_owned(),
        parameters: BTreeMap::from([("scale_percent".to_owned(), ParamValue::Integer(200))]),
        keyframes: BTreeMap::new(),
    };
    let (service, _core) =
        transform_track_service(vec![doubled, tracking_reframe_effect([50, 50])], frames);

    let result = service
        .track_reframe_subject(&reframe_tracking_args([60, 60], [50, 50]))
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    let message = result
        .content
        .iter()
        .find_map(|content| content.as_text().map(|text| text.text.clone()))
        .unwrap_or_default();
    assert!(
        message.contains("layer scale 2"),
        "the refusal must name the layer scale: {message}"
    );
    assert!(
        message.contains("120x120 percent template"),
        "the refusal must name the composite template: {message}"
    );
}

/// A layer scale that ramps 100 → 200 percent, linearly, over frames 0..=40.
///
/// The twin of [`keyframed_scale_transform`]: it is *legal* at the seed and
/// illegal at the far end, which is exactly the case a seed-only template
/// gate lets through.
fn growing_scale_transform() -> Effect {
    Effect {
        id: EffectId(9),
        name: "transform".to_owned(),
        parameters: BTreeMap::from([("scale_percent".to_owned(), ParamValue::Integer(100))]),
        keyframes: BTreeMap::from([(
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
                        value: 200,
                        interpolation: KeyframeInterpolation::Linear,
                    },
                ],
            },
        )]),
    }
}

/// CC5 §5.2: the provenance box is the converted layer centre bracketed by
/// the *declared* layer subject size — never the composite template, whose
/// size is pinned to the seed frame's scale.
///
/// Worked at `scale 0.5 / x 20 / y 20`, whose inverse is
/// `u_layer = 2·u_composite − 0.9`: composite 0.65 is layer 0.40 (4000 bp)
/// and composite 0.695 is layer 0.49 (4900 bp). A 12 × 22 percent subject
/// has half extents of 600 and 1100 basis points, so the box is
/// 3400..4600 horizontally and 3800..6000 vertically — exactly 1200 and
/// 2200 wide, which is `percent · 100`.
#[test]
fn layer_subject_bounds_brackets_the_declared_subject_around_the_layer_centre() {
    let transform = LayerTransform {
        scale: 0.5,
        offset_x: 0.4,
        offset_y: 0.4,
    };
    let centre = transform.composite_to_layer_unit([0.65, 0.695]);
    let bounds = layer_subject_bounds(TimeCode(7), centre, [12, 22]);

    assert_eq!(bounds.at, TimeCode(7));
    assert_eq!(bounds.left_basis_points, 3_400);
    assert_eq!(bounds.right_basis_points, 4_600);
    // The vertical pair takes the *second* declared percent, not the first.
    assert_eq!(bounds.top_basis_points, 3_800);
    assert_eq!(bounds.bottom_basis_points, 6_000);
    assert_eq!(bounds.right_basis_points - bounds.left_basis_points, 1_200);
    assert_eq!(bounds.bottom_basis_points - bounds.top_basis_points, 2_200);

    // The composite template the tracker matched with is *not* the box: at
    // this scale a 12 percent layer subject is a 6 percent composite
    // template, and converting that back would halve the box.
    assert_eq!(tracked_box_percent(12, transform.scale), 6);
}

/// CC5 §5.2: a box whose edges do not land on the basis-point grid rounds
/// **outward**, and a box that leaves the layer is clamped to `0..=10000`.
#[test]
fn layer_subject_bounds_rounds_outward_and_clamps_at_the_layer_edges() {
    // Layer centre 3968.5 bp, half extent 600: 3368.5 floors to 3368 and
    // 4568.5 ceils to 4569, so the box is one basis point wider than the
    // declared 1200 and never narrower.
    let bounds = layer_subject_bounds(TimeCode(0), [0.396_85, 0.5], [12, 12]);
    assert_eq!(bounds.left_basis_points, 3_368);
    assert_eq!(bounds.right_basis_points, 4_569);
    assert_eq!(bounds.right_basis_points - bounds.left_basis_points, 1_201);

    // 200 − 600 clamps to 0 and 9800 + 600 clamps to 10000: the crop can
    // only sample layer uv 0..1, so a subject hanging off the layer is
    // recorded up to the edge and no further.
    let clamped = layer_subject_bounds(TimeCode(0), [0.02, 0.98], [12, 12]);
    assert_eq!(clamped.left_basis_points, 0);
    assert_eq!(clamped.right_basis_points, 800);
    assert_eq!(clamped.top_basis_points, 9_200);
    assert_eq!(clamped.bottom_basis_points, 10_000);
}

/// CC5 §5.2: under a keyframed scale the provenance box stays the declared
/// subject size in *layer* basis points at every sample, and the focus
/// curve tracks the analytic layer centre.
///
/// The regression this pins: bracketing each composite centre with the
/// *seed-frame* template and converting the corners through the
/// *per-observation* scale inflates the box by `seed_scale / scale`. Here
/// the scale ramps 1.0 → 0.5, so the last sample's composite box becomes
/// more than 6000 bp in layer space against a 5625 bp delivery crop and
/// `focus_interval_for_subject_axis` refuses with "wider than the delivery
/// crop", even though the declared subject is 3000 bp wide.
///
/// The subject is drawn at a constant composite size so the frame-to-frame
/// template match is a pure translation; the fixture's job is to exercise
/// the conversion, not the matcher. It sits at layer 32 percent, which is
/// as far off centre as a 30 percent subject can sit while the *pre-fix*
/// converted box still fits inside 0..=10000 — otherwise the old clamp
/// hides the inflation instead of refusing.
///
/// Composite pixel centres are 102, 109, 116, 123 and 130 of 320, and
/// `u_layer = (u_composite − 0.5)/scale + 0.5` at scales 1.0, 0.875, 0.75,
/// 0.625 and 0.5 makes them layer 3203, 3196, 3188, 3175 and 3156 bp.
#[test]
#[allow(clippy::too_many_lines)]
fn track_reframe_subject_bounds_the_declared_subject_under_a_keyframed_scale() {
    let frames = (0..60)
        .map(|frame| {
            (
                TimeCode(frame),
                transform_box_frame([102 + frame * 18 / 25, 90], 5),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let (service, _core) = transform_track_service(
        vec![
            keyframed_scale_transform(),
            tracking_reframe_effect([32, 50]),
        ],
        frames,
    );

    let result = service
        .track_reframe_subject(&reframe_tracking_args([30, 30], [32, 50]))
        .unwrap();
    let message = result
        .content
        .iter()
        .find_map(|content| content.as_text().map(|text| text.text.clone()))
        .unwrap_or_default();
    assert!(
        !message.contains("wider than the delivery crop"),
        "the seed-template containment bug is back: {message}"
    );
    let structured = result.structured_content.clone().unwrap();
    assert_ne!(
        result.is_error,
        Some(true),
        "tracking refused: {structured}"
    );

    let samples = structured["subject_samples"].as_array().unwrap();
    assert_eq!(samples.len(), TRANSFORM_TRACK_SAMPLES.len());
    for (index, sample) in samples.iter().enumerate() {
        let bounds = &sample["layer_bounds_basis_points"];
        let left = bounds["left"].as_i64().unwrap();
        let right = bounds["right"].as_i64().unwrap();
        let top = bounds["top"].as_i64().unwrap();
        let bottom = bounds["bottom"].as_i64().unwrap();
        // 30 percent of the layer is 3000 basis points, plus at most the
        // one basis point the outward rounding adds when the centre does
        // not land on the grid. Nothing is clamped here: the box sits
        // between 1656 and 4703, well inside 0..=10000.
        assert!(
            (3_000..=3_001).contains(&(right - left)),
            "sample {index}: horizontal box {left}..{right} is not the declared 3000 bp"
        );
        assert!(
            (3_000..=3_001).contains(&(bottom - top)),
            "sample {index}: vertical box {top}..{bottom} is not the declared 3000 bp"
        );
        // The box is centred on the converted layer centre, not on a
        // composite reading.
        let centre = sample["layer_x_basis_points"].as_i64().unwrap();
        assert!(
            (i64::midpoint(left, right) - centre).abs() <= 1,
            "sample {index}: box {left}..{right} is not centred on {centre}"
        );
    }

    // The composite template stays the seed-scale one throughout, and at
    // the last sample converting *its* bounds through that sample's own
    // scale is what used to blow past the 5625 bp delivery crop.
    let last = samples.last().unwrap();
    let composite = &last["composite_bounds_basis_points"];
    let composite_width =
        composite["right"].as_i64().unwrap() - composite["left"].as_i64().unwrap();
    let last_scale = last["layer_transform"]["scale"].as_f64().unwrap();
    assert!((last_scale - 0.5).abs() < 1e-9, "last scale {last_scale}");
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    let seed_template_layer_width = (composite_width as f64 / last_scale).round() as i64;
    assert!(
        seed_template_layer_width > 5_625,
        "the pre-fix construction must be out of range, or this test proves nothing: {seed_template_layer_width}"
    );

    // The converted layer centres follow the analytic values. The template
    // is a coarse 30 percent box, so the matcher is subsampled and lags by
    // a couple of composite pixels; 200 bp covers that at every scale here.
    let layer = observation_values(&structured, "subject_samples", "layer_x_basis_points");
    let composite_centres =
        observation_values(&structured, "subject_samples", "composite_x_basis_points");
    for (index, expected) in [3_203_i64, 3_196, 3_188, 3_175, 3_156].iter().enumerate() {
        assert!(
            (layer[index] - expected).abs() <= 200,
            "sample {index}: converted {} against the analytic layer {expected}",
            layer[index]
        );
    }
    // The composite reading walks away from the layer reading as the layer
    // shrinks: 3203 bp at the seed against 4078 bp at the last sample.
    assert!(
        (composite_centres[4] - layer[4]).abs() > 700,
        "the two spaces must not coincide, or this test proves nothing: {composite_centres:?} against {layer:?}"
    );

    // The focus is planned in the same space, so it stays near the layer
    // subject; the three-sample median lags a ramp by one inter-sample
    // step, which is about 120 bp here.
    let focus = observation_values(&structured, "focus_keyframes", "x_basis_points");
    for (index, expected) in layer.iter().enumerate() {
        assert!(
            (focus[index] - expected).abs() <= 700,
            "sample {index}: focus {} against the layer subject {expected}",
            focus[index]
        );
    }

    // The published contract says the template is seed-sized while the
    // conversion is per frame, and names the resolved range.
    let note = structured["coordinate_space"]["keyframed_transform"]
        .as_str()
        .unwrap();
    assert!(note.contains("seed frame's scale 1"), "{note}");
    assert!(note.contains("0.5 at clip-local frame 40"), "{note}");
    assert!(note.contains("1 at clip-local frame 0"), "{note}");
}

/// CC5 §5.2: the `1..=75` template gate is applied at the smallest and the
/// largest resolved scale, so a ramp that is legal at the seed and illegal
/// at the far end is refused — naming the offending frame and scale.
#[test]
fn track_reframe_subject_refuses_a_template_the_largest_sampled_scale_pushes_out_of_range() {
    let frames = (0..60)
        .map(|frame| (TimeCode(frame), transform_box_frame([160, 90], 20)))
        .collect::<BTreeMap<_, _>>();
    let (service, _core) = transform_track_service(
        vec![growing_scale_transform(), tracking_reframe_effect([50, 50])],
        frames,
    );

    // 50 × 1.0 = 50 percent is a legal template at the seed frame, so a
    // seed-only gate would accept this and then match a 100 percent
    // template at frame 40.
    let result = service
        .track_reframe_subject(&reframe_tracking_args([50, 50], [50, 50]))
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    let message = result
        .content
        .iter()
        .find_map(|content| content.as_text().map(|text| text.text.clone()))
        .unwrap_or_default();
    assert!(
        message.contains("100x100 percent template"),
        "the refusal must name the offending template: {message}"
    );
    assert!(
        message.contains("layer scale 2"),
        "the refusal must name the offending scale: {message}"
    );
    assert!(
        message.contains("clip-local frame 40"),
        "the refusal must name the offending frame: {message}"
    );
}

/// The mask half of the same gate: legal at the seed, illegal at frame 40.
#[test]
fn track_mask_region_refuses_a_template_the_largest_sampled_scale_pushes_out_of_range() {
    let frames = (0..60)
        .map(|frame| (TimeCode(frame), transform_box_frame([160, 90], 20)))
        .collect::<BTreeMap<_, _>>();
    let (service, _core) = transform_track_service(
        vec![
            growing_scale_transform(),
            tracking_mask_effect([50, 50], [50, 50]),
        ],
        frames,
    );

    let result = service.track_mask_region(&mask_tracking_args()).unwrap();
    assert_eq!(result.is_error, Some(true));
    let message = result
        .content
        .iter()
        .find_map(|content| content.as_text().map(|text| text.text.clone()))
        .unwrap_or_default();
    assert!(
        message.contains("100x100 percent template"),
        "the refusal must name the offending template: {message}"
    );
    assert!(
        message.contains("layer scale 2 at clip-local frame 40"),
        "the refusal must name the offending frame and scale: {message}"
    );
}

/// CC5 §5.2: a seed whose forward map leaves the composited frame names no
/// pixel, so the tracker refuses typed instead of clamping to the raster
/// edge and following whatever sits in the corner.
///
/// `scale_percent 100` with `x_percent 100` accumulates `offset_x = 2.0`,
/// so `u_composite = (0.5 − 0.5)·1 + 2.0/2 + 0.5 = 1.5`.
#[test]
fn track_reframe_subject_refuses_a_seed_the_layer_transform_pushes_off_the_composite() {
    let frames = (0..60)
        .map(|frame| (TimeCode(frame), transform_box_frame([160, 90], 5)))
        .collect::<BTreeMap<_, _>>();
    let pushed_off = Effect {
        id: EffectId(9),
        name: "transform".to_owned(),
        parameters: BTreeMap::from([
            ("scale_percent".to_owned(), ParamValue::Integer(100)),
            ("x_percent".to_owned(), ParamValue::Integer(100)),
        ]),
        keyframes: BTreeMap::new(),
    };
    let (service, _core) =
        transform_track_service(vec![pushed_off, tracking_reframe_effect([50, 50])], frames);

    let result = service
        .track_reframe_subject(&reframe_tracking_args([6, 11], [50, 50]))
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.clone().unwrap();
    assert_eq!(structured["code"], "tracking_seed_outside_composite");
    let details = &structured["details"];
    // Only the horizontal axis left the frame, so the refusal names exactly
    // the one repairable argument rather than both or a generic selector.
    assert_eq!(details["field"], json!("initial_subject_x_percent"));
    assert_eq!(details["observed"]["layer_center_unit"], json!([0.5, 0.5]));
    assert_eq!(
        details["observed"]["composite_center_unit"],
        json!([1.5, 0.5])
    );
    assert_eq!(details["observed"]["scale"], 1.0);
    assert_eq!(details["observed"]["offset_x"], 2.0);
    assert_eq!(details["observed"]["clip_local_frame"], 0);
    assert!(
        details["allowed"].as_str().unwrap().contains("0..=1"),
        "{details}"
    );
    assert!(
        details["recovery_action"]
            .as_str()
            .unwrap()
            .contains("names none"),
        "{details}"
    );
}

/// CC5 §5.2: `track_matte_window` refuses an off-composite seed on exactly
/// the same terms as `track_reframe_subject`, and names the window's own
/// stored centre — the repairable parameter — rather than `window_index`.
///
/// The fixture window is the neutral one, centred at 5000 bp on both axes.
/// `scale_percent 100` with `x_percent 100` accumulates `offset_x = 2.0`,
/// so `u_composite = (0.5 − 0.5)·1 + 2.0/2 + 0.5 = 1.5` horizontally while
/// the vertical axis stays at 0.5: only the horizontal parameter is named.
#[test]
fn track_matte_window_refuses_a_seed_the_layer_transform_pushes_off_the_composite() {
    let pushed_off = Effect {
        id: EffectId(9),
        name: "transform".to_owned(),
        parameters: BTreeMap::from([
            ("scale_percent".to_owned(), ParamValue::Integer(100)),
            ("x_percent".to_owned(), ParamValue::Integer(100)),
        ]),
        keyframes: BTreeMap::new(),
    };
    let frames = BTreeMap::from([
        (TimeCode(0), matte_box_frame([160, 90])),
        (TimeCode(10), matte_box_frame([160, 90])),
    ]);
    let (service, _core) = matte_track_service(frames, BTreeMap::new(), vec![pushed_off]);

    let result = service
        .track_matte_window(&TrackMatteWindowArgs {
            expected_revision: None,
            clip_id: ClipId(1),
            effect_id: EffectId(1),
            window_index: 0,
            start_local_frame: Some(TimeCode(0)),
            end_local_frame: Some(TimeCode(11)),
            step_frames: Some(10),
            search_radius_percent: None,
            max_width: None,
            minimum_confidence_basis_points: None,
        })
        .unwrap();

    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.clone().unwrap();
    assert_eq!(structured["code"], "tracking_seed_outside_composite");
    let details = &structured["details"];
    // The offending *parameter*, not the selector that reached it.
    assert_eq!(
        details["field"],
        json!("matte_window0_center_x_basis_points")
    );
    // The selector is still available, as context.
    assert_eq!(details["observed"]["window_index"], 0);
    assert_eq!(details["observed"]["layer_center_unit"], json!([0.5, 0.5]));
    assert_eq!(
        details["observed"]["composite_center_unit"],
        json!([1.5, 0.5])
    );
    assert_eq!(details["observed"]["scale"], 1.0);
    assert_eq!(details["observed"]["offset_x"], 2.0);
    assert_eq!(details["observed"]["offset_y"], 0.0);
    assert_eq!(details["observed"]["clip_local_frame"], 0);
    assert!(
        details["allowed"].as_str().unwrap().contains("0..=1"),
        "{details}"
    );
    assert!(
        details["recovery_action"]
            .as_str()
            .unwrap()
            .contains("names none"),
        "{details}"
    );
}

/// The reframe tool writes `focus_x/y_basis_points`, so re-tracking a clip
/// it already touched must seed from those, not from the coarse percent
/// twin the compositor itself overrides.
///
/// Mirrors `compositor.rs`'s `ReframeFocusXBasisPoints` arm: an explicitly
/// stored basis-point focus wins, a missing one falls back to the percent,
/// and neither leaves the seed centred.
#[test]
fn track_reframe_subject_seeds_from_the_stored_focus_basis_points() {
    let frames = (0..60)
        .map(|frame| (TimeCode(frame), transform_box_frame([80, 90], 5)))
        .collect::<BTreeMap<_, _>>();
    let mut reframe = tracking_reframe_effect([50, 50]);
    reframe.parameters.insert(
        "focus_x_basis_points".to_owned(),
        ParamValue::Integer(2_500),
    );
    let (service, _core) = transform_track_service(vec![reframe], frames);

    let mut args = reframe_tracking_args([6, 11], [50, 50]);
    args.initial_subject_x_percent = None;
    args.initial_subject_y_percent = None;
    let result = service.track_reframe_subject(&args).unwrap();
    let structured = result.structured_content.clone().unwrap();
    assert_ne!(
        result.is_error,
        Some(true),
        "tracking refused: {structured}"
    );

    let initial = &structured["subject_template"]["initial_center_percent"];
    assert_eq!(
        initial["x"], 25,
        "the stored 2500 bp focus must seed at 25 percent, not the 50 percent twin"
    );
    // No `focus_y_basis_points` is stored, so the vertical axis falls back
    // to `focus_y_percent`.
    assert_eq!(initial["y"], 50);
    assert_eq!(
        structured["coordinate_space"]["seed_center_percent"],
        json!([25, 50])
    );
}

/// A 320 × 180 frame of deterministic per-frame noise.
///
/// Every frame is unlike every other, so a SAD template match has nothing
/// to lock onto and the confidence gate fires.
pub(super) fn matte_noise_frame(frame: i64) -> RgbaImage {
    let (width, height) = (320_u32, 180_u32);
    let seed = u32::try_from(frame.rem_euclid(4_096)).unwrap_or(0);
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            // An avalanche mix, so the field has no translational symmetry
            // a shifted template could exploit.
            let mut hash = x.wrapping_mul(0x9E37_79B9)
                ^ y.wrapping_mul(0x85EB_CA6B)
                ^ seed.wrapping_mul(0xC2B2_AE35);
            hash ^= hash >> 15;
            hash = hash.wrapping_mul(0x2545_F491);
            hash ^= hash >> 13;
            let value = u8::try_from(hash & 0xFF).unwrap_or(0);
            pixels.extend_from_slice(&[value, value.wrapping_add(83), value, 255]);
        }
    }
    RgbaImage {
        width,
        height,
        pixels,
    }
}
