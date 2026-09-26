//! MO2 Part B1's render gates (§13 gates 1–7), rule tests and pins.
//!
//! In `src/` for the reason every `ccN_fixtures.rs` is: the gates drive
//! `FrameRenderer`, `compositor_layers` and the `pub(crate)` compositor
//! seams, and MO2 widens no public surface to reach them.
//!
//! Lanes (R27): every document gate renders the production GPU path and the
//! CPU twin (`compositor::twin`) through the same `visual_layers_at` +
//! decode + `compositor_layers` assembly and compares them within the MO2
//! tolerances; probes then pin the expected values independently.

#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::default_trait_access)]
#![allow(clippy::float_cmp)]
#![allow(clippy::items_after_statements)]
#![allow(clippy::too_many_lines)]

use std::sync::Arc;

use half::f16;
use kinewright_core::{
    AssetId, BlendMode, Clip, ClipContent, ClipId, Document, Effect, EffectId, FrameTexture,
    LinearRgbaImage, LinkId, MediaError, ParamValue, Rational, SolidColor, TRANSITION_DESCRIPTORS,
    TimeCode, Title, TitlePosition, Track, TrackId, TrackKind, Transition, title_color,
};

use crate::{
    audio::ClipAudioShaping,
    cc1_fixtures::abs_code_diff_rgb,
    color_pipeline::{apply_color_nodes_at, encode_monitor_rgba8_for_description},
    compositor::{Compositor, CompositorLayer, GpuContext, LayerMode, LayerRole, twin},
    frame::WorkingFrame,
    gpu_test_support::fixture_gpu_or_skip,
    mo2_identity_corpus,
    render::{DecodeStrategy, FrameRenderer, RenderScale},
    timeline::{TransitionRenderParams, timeline_audio_segments},
};

/// Even on both transition axes (midpoint shifts of 80 / 44 whole pixels),
/// and quarter/half boundaries fall between pixel centres, never on one.
const W: u32 = 160;
const H: u32 = 88;
const GREY: [u8; 3] = [0x80, 0x80, 0x80];
const ORANGE: [u8; 3] = [0xF0, 0x90, 0x30];
const BLUE: [u8; 3] = [0x30, 0x60, 0xF0];
/// Title colour token 2.
const ACCENT: [u8; 3] = [0x42, 0xC7, 0xC9];

// ---------------------------------------------------------------- documents

fn clip(id: u64, content: ClipContent, blend_mode: BlendMode, effects: Vec<Effect>) -> Clip {
    Clip {
        enabled: true,
        enabled_curve: None,
        id: ClipId(id),
        asset: AssetId::default(),
        source_range: TimeCode(0)..TimeCode(9),
        content,
        timeline_start: TimeCode::ZERO,
        effects,
        transition_in: None,
        link: None,
        audio_gain_tenth_db: 0,
        audio_fade_in_frames: TimeCode::ZERO,
        audio_fade_out_frames: TimeCode::ZERO,
        speed_percent: 100,
        audio_gain_curve: None,
        blend_mode,
    }
}

fn solid(id: u64, rgb: [u8; 3], blend_mode: BlendMode, effects: Vec<Effect>) -> Clip {
    let [r, g, b] = rgb;
    let content = ClipContent::Solid(SolidColor { r, g, b });
    clip(id, content, blend_mode, effects)
}

fn adjustment(id: u64, blend_mode: BlendMode, effects: Vec<Effect>) -> Clip {
    clip(id, ClipContent::Adjustment, blend_mode, effects)
}

fn title_clip(id: u64, position: TitlePosition, effects: Vec<Effect>) -> Clip {
    let title = Title {
        text: "MO2".to_owned(),
        position,
        background_scrim: false,
        ..Title::default()
    };
    clip(id, ClipContent::Title(title), BlendMode::Normal, effects)
}

fn effect(id: u64, name: &str, parameters: &[(&str, i64)]) -> Effect {
    Effect {
        id: EffectId(id),
        name: name.to_owned(),
        parameters: parameters
            .iter()
            .map(|(key, value)| ((*key).to_owned(), ParamValue::Integer(*value)))
            .collect(),
        keyframes: std::collections::BTreeMap::new(),
        enabled: true,
        enabled_curve: None,
    }
}

fn opacity(id: u64, percent: i64) -> Effect {
    effect(id, "opacity", &[("percent", percent)])
}

fn primary(id: u64, parameters: &[(&str, i64)]) -> Effect {
    effect(id, "primary_correction", parameters)
}

/// One video track per clip, bottom-to-top; validated like every render.
fn document_sized(resolution: (u32, u32), clips: Vec<Clip>) -> Document {
    let document = Document {
        resolution,
        duration: TimeCode(9),
        tracks: clips
            .into_iter()
            .enumerate()
            .map(|(index, clip)| Track {
                id: TrackId(index as u64 + 1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![clip],
            })
            .collect(),
        ..Document::default()
    };
    document.validate().expect("MO2 fixtures are valid");
    document
}

fn document(clips: Vec<Clip>) -> Document {
    document_sized((W, H), clips)
}

fn with_transition(mut clip: Clip, name: &str, duration: i64) -> Clip {
    clip.transition_in = Some(Transition {
        name: name.to_owned(),
        duration: TimeCode(duration),
    });
    clip
}

// ---------------------------------------------------------------- lanes

fn gpu(r: &mut FrameRenderer, document: &Document, at: i64) -> Result<LinearRgbaImage, MediaError> {
    let (scale, strategy) = (RenderScale::FullResolution, DecodeStrategy::Seek);
    r.render_working(document, TimeCode(at), document.resolution, scale, strategy)
}

/// Signed position on the f16 storage grid, for ULP distances.
fn f16_key(value: f32) -> i32 {
    let bits = i32::from(f16::from_f32(value).to_bits());
    if bits & 0x8000 == 0 {
        bits
    } else {
        -(bits & 0x7fff)
    }
}

/// R27 working tolerances: unit domain max abs ≤ 1e-3; over-range relative
/// ≤ 2^-10 and ≤ 4 f16 ULP.
fn assert_r27(actual: &[f32], expected: &[f32], label: &str) {
    assert_r27_within(actual, expected, &[], label);
}

/// R27 for one value, widened by the ME9 sub-texel `slack` (zero for every
/// value nothing filters, so unfiltered pixels keep the unit 1e-3).
fn r27_close(a: f32, e: f32, slack: f32) -> bool {
    if a.abs() <= 1.0 && e.abs() <= 1.0 {
        (a - e).abs() <= 1e-3 + slack
    } else {
        (a - e).abs() <= e.abs() * 2_f32.powi(-10) + slack
            && (slack > 0.0 || f16_key(a).abs_diff(f16_key(e)) <= 4)
    }
}

fn assert_r27_within(actual: &[f32], expected: &[f32], slack: &[f32], label: &str) {
    assert_eq!(actual.len(), expected.len(), "{label}: raster size");
    for (index, (&a, &e)) in actual.iter().zip(expected).enumerate() {
        let slack = slack.get(index).copied().unwrap_or(0.0);
        let (pixel, channel) = (index / 4, index % 4);
        assert!(
            r27_close(a, e, slack),
            "{label}: pixel {pixel} channel {channel}: {a} vs {e} (slack {slack})"
        );
    }
}

/// R27 monitor tolerances (the CC1 `abs_code_diff_rgb` method).
fn assert_monitor(gpu: &FrameTexture, twin: &LinearRgbaImage, document: &Document, label: &str) {
    let monitoring = &document.color_context.monitoring;
    let expected = twin
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|pixel| encode_monitor_rgba8_for_description(*pixel, monitoring).unwrap())
        .collect::<Vec<_>>();
    let diff = abs_code_diff_rgb(&gpu.rgba, &expected);
    assert!(
        diff.max <= 2 && diff.p99 <= 1.0 && diff.mean <= 0.25,
        "{label}: monitor max {} p99 {} mean {}",
        diff.max,
        diff.p99,
        diff.mean
    );
}

/// Both lanes for one frame: GPU ≡ twin within R27 (working and monitor),
/// and the R9b opaque accumulator (alpha ≡ 1). Returns the GPU working raster.
fn matched(r: &mut FrameRenderer, document: &Document, at: i64, label: &str) -> LinearRgbaImage {
    let gpu = gpu(r, document, at).unwrap_or_else(|error| panic!("{label}: GPU {error}"));
    let twin = r
        .twin_working(document, TimeCode(at), document.resolution)
        .unwrap_or_else(|error| panic!("{label}: twin {error}"));
    // ME9: the sub-texel envelope is only computed when a value misses.
    let pairs = || gpu.pixels.iter().zip(&twin.pixels);
    let slack = if pairs().all(|(a, e)| r27_close(*a, *e, 0.0)) {
        Vec::new()
    } else {
        r.twin_envelope(document, TimeCode(at), document.resolution)
            .unwrap()
    };
    assert_r27_within(&gpu.pixels, &twin.pixels, &slack, label);
    assert!(
        gpu.pixels.chunks(4).all(|pixel| pixel[3] == 1.0),
        "{label}: R9b the accumulator is opaque"
    );
    let (scale, strategy) = (RenderScale::FullResolution, DecodeStrategy::Seek);
    let monitor = r
        .render(document, TimeCode(at), document.resolution, scale, strategy)
        .expect("the monitor path renders");
    assert_monitor(&monitor, &twin, document, label);
    gpu
}

fn px(image: &LinearRgbaImage, x: u32, y: u32) -> [f32; 3] {
    let start = ((y * image.width + x) * 4) as usize;
    std::array::from_fn(|channel| image.pixels[start + channel])
}

/// The working value a display-coded opaque colour enters as (R3), via the
/// shared generated-content conversion.
fn working(rgb: [u8; 3]) -> [f32; 3] {
    let frame = WorkingFrame::from_display_frame(&FrameTexture {
        width: 1,
        height: 1,
        rgba: Arc::new(vec![rgb[0], rgb[1], rgb[2], u8::MAX]),
    })
    .unwrap();
    std::array::from_fn(|channel| frame.pixels[channel].to_f32())
}

fn store(value: f32) -> f32 {
    f16::from_f32(value).to_f32()
}

fn mode_word(mode: BlendMode) -> u32 {
    BlendMode::ALL.iter().position(|m| *m == mode).unwrap() as u32
}

/// `α·B(S, D) + (1 − α)·D`, f16-stored (§3, R9b).
fn blend3(mode: BlendMode, s: [f32; 3], d: [f32; 3], alpha: f32) -> [f32; 3] {
    let word = mode_word(mode);
    std::array::from_fn(|c| store(alpha * twin::blend(word, s[c], d[c]) + (1.0 - alpha) * d[c]))
}

// ---------------------------------------------------------------- gates 1–3

/// Gate 1: a feather-masked 60% `Screen` light leak over a grey plate.
fn screen_light_leak_matches_twin_on(context: GpuContext) {
    let mut r = FrameRenderer::new(context);
    let mask = effect(
        2,
        "mask",
        &[
            ("shape_token", 2),
            ("width_percent", 60),
            ("height_percent", 80),
            ("feather_percent", 20),
        ],
    );
    let leak = solid(2, ORANGE, BlendMode::Screen, vec![opacity(1, 60), mask]);
    let plate = solid(1, GREY, BlendMode::Normal, vec![]);
    let image = matched(&mut r, &document(vec![plate, leak]), 0, "screen light leak");
    let (s, d) = (working(ORANGE), working(GREY));
    assert_r27(
        &px(&image, 80, 45),
        &blend3(BlendMode::Screen, s, d, 0.6),
        "leak centre",
    );
    assert_r27(&px(&image, 1, 1), &d, "outside the mask");
}

/// Gate 2: a cropped `Multiply` callout box.
fn multiply_callout_matches_twin_on(context: GpuContext) {
    let mut r = FrameRenderer::new(context);
    let crop = effect(
        1,
        "crop",
        &[
            ("left_percent", 25),
            ("right_percent", 25),
            ("top_percent", 25),
            ("bottom_percent", 25),
        ],
    );
    let callout = solid(2, ORANGE, BlendMode::Multiply, vec![crop]);
    let plate = solid(1, GREY, BlendMode::Normal, vec![]);
    let image = matched(
        &mut r,
        &document(vec![plate, callout]),
        0,
        "multiply callout",
    );
    let (s, d) = (working(ORANGE), working(GREY));
    assert_r27(
        &px(&image, 80, 45),
        &blend3(BlendMode::Multiply, s, d, 1.0),
        "inside the box",
    );
    assert_r27(&px(&image, 10, 10), &d, "outside the box");
}

/// One `N×1` working raster from explicit linear values.
fn row(values: &[[f32; 4]]) -> WorkingFrame {
    WorkingFrame {
        width: values.len() as u32,
        height: 1,
        pixels: Arc::new(values.iter().flatten().map(|v| f16::from_f32(*v)).collect()),
    }
}

fn pixels_layer(frame: &WorkingFrame, blend: BlendMode) -> CompositorLayer<'_, WorkingFrame> {
    CompositorLayer {
        frame,
        effects: &[],
        transition: TransitionRenderParams::default(),
        mode: LayerMode {
            blend,
            role: LayerRole::Pixels,
        },
    }
}

type Lanes = (
    Result<LinearRgbaImage, MediaError>,
    Result<LinearRgbaImage, MediaError>,
);

/// `above` (`mode`) over `below` (`Normal`) through the compositor and twin.
fn pair_lanes(compositor: &Compositor, mode: BlendMode, below: [f32; 4], above: [f32; 4]) -> Lanes {
    let (d, s) = (row(&[below]), row(&[above]));
    let layers = [pixels_layer(&d, BlendMode::Normal), pixels_layer(&s, mode)];
    (
        compositor.render_working((1, 1), &layers),
        twin::render_working((1, 1), &layers, None),
    )
}

fn grey4(v: f32) -> [f32; 4] {
    [v, v, v, 1.0]
}

/// Gate 3: the other modes over the plate, every §3 extended-domain vector,
/// the CC8 peaks, and an over-1.0 `Add` surviving to `render_working`.
fn remaining_modes_match_twin_on(gpu_context: GpuContext) {
    let mut r = FrameRenderer::new(gpu_context.clone());
    for mode in [
        BlendMode::Overlay,
        BlendMode::Darken,
        BlendMode::Lighten,
        BlendMode::Add,
    ] {
        let top = solid(2, ORANGE, mode, vec![opacity(1, 70)]);
        let plate = solid(1, GREY, BlendMode::Normal, vec![]);
        let label = format!("{mode:?}");
        let image = matched(&mut r, &document(vec![plate, top]), 0, &label);
        let expected = blend3(mode, working(ORANGE), working(GREY), 0.7);
        assert_r27(&px(&image, 30, 30), &expected, &label);
    }
    let white = |id, mode| solid(id, [255; 3], mode, vec![]);
    let doubled = document(vec![white(1, BlendMode::Normal), white(2, BlendMode::Add)]);
    let image = matched(&mut r, &doubled, 0, "over-range Add");
    assert_eq!(
        px(&image, 5, 5),
        [2.0; 3],
        "Add(1, 1) = 2 survives the readback"
    );

    use BlendMode::{Add, Darken, Lighten, Multiply, Overlay, Screen};
    #[rustfmt::skip]
    let vectors = [
        (Screen, 0.5, 0.5, 0.75), (Screen, 2.0, 2.0, 3.0), (Screen, 3.0, 3.0, 5.0),
        (Screen, 2.0, 0.5, 2.0), (Screen, -1.0, 0.5, -0.5), (Screen, -1.0, -1.0, -2.0),
        (Screen, 0.999, 1.001, 1.001), (Screen, 0.18, 4.0, 4.0),
        (Overlay, 0.75, 0.4, 0.6), (Overlay, 0.75, 0.5, 0.75), (Overlay, 0.75, 0.6, 0.8),
        (Overlay, 0.75, 2.0, 2.0), (Overlay, 2.0, 0.4, 1.8), (Overlay, 0.75, -1.0, -1.0),
        (Overlay, 0.18, 4.0, 4.0), (Multiply, -1.0, -1.0, 1.0), (Add, 2.0, 2.0, 4.0),
        (Darken, 2.0, -1.0, -1.0), (Lighten, 2.0, -1.0, 2.0),
        // R11 raw-working CC8 peaks: Screen(P,P) = Overlay(P,P) = 2P − 1.
        (Screen, 3.776_475, 3.776_475, 6.552_95), (Overlay, 3.776_475, 3.776_475, 6.552_95),
        (Screen, 46.4159, 46.4159, 91.8318), (Overlay, 46.4159, 46.4159, 91.8318),
    ];
    let compositor = Compositor::new(gpu_context);
    for (mode, s, d, expected) in vectors {
        let label = format!("{mode:?}({s}, {d})");
        let exact = twin::blend(mode_word(mode), s, d);
        assert!(
            (exact - expected).abs() <= 1e-4 * expected.abs().max(1.0),
            "{label} = {exact}"
        );
        let (gpu, cpu) = pair_lanes(&compositor, mode, grey4(d), grey4(s));
        let (gpu, cpu) = (gpu.unwrap(), cpu.unwrap());
        let stored = store(twin::blend(mode_word(mode), store(s), store(d)));
        assert_r27(&gpu.pixels, &[stored, stored, stored, 1.0], &label);
        assert_r27(&gpu.pixels, &cpu.pixels, &format!("{label} twin"));
    }
}

/// R9b: `Screen` S=0.75 over D=0.5 at αs=0.5 gives 0.6875 at alpha 1.
fn r9b_opaque_accumulator_vector_on(gpu_context: GpuContext) {
    let compositor = Compositor::new(gpu_context);
    let (gpu, cpu) = pair_lanes(
        &compositor,
        BlendMode::Screen,
        grey4(0.5),
        [0.75, 0.75, 0.75, 0.5],
    );
    assert_eq!(gpu.unwrap().pixels, [0.6875, 0.6875, 0.6875, 1.0]);
    assert_eq!(cpu.unwrap().pixels, [0.6875, 0.6875, 0.6875, 1.0]);
}

/// R9b: a colour fade with a mask under a non-`Normal` blend.
fn r9b_colour_fade_masked_blend_matches_twin_on(context: GpuContext) {
    let mut r = FrameRenderer::new(context);
    let mask = effect(
        2,
        "mask",
        &[
            ("shape_token", 1),
            ("width_percent", 50),
            ("feather_percent", 30),
        ],
    );
    let top = solid(2, ORANGE, BlendMode::Overlay, vec![opacity(1, 50), mask]);
    let fading = with_transition(top, "fade_from_white", 5);
    let document = document(vec![solid(1, GREY, BlendMode::Normal, vec![]), fading]);
    for at in [0, 2, 4] {
        matched(
            &mut r,
            &document,
            at,
            &format!("fade + mask + Overlay at {at}"),
        );
    }
}

// ---------------------------------------------------------------- R10

fn refused(result: Result<LinearRgbaImage, MediaError>, layer: usize, label: &str) {
    match result {
        Err(MediaError::NonFiniteRender { layer: got, .. }) => assert_eq!(got, layer, "{label}"),
        other => panic!("{label}: expected a non-finite refusal, got {other:?}"),
    }
}

/// R10: overflow at the f16 store and forced NaN/inf refuse typed on both
/// lanes, stickily (an opaque `Normal` cover does not hide it), while a
/// representable over-range result is preserved.
fn r10_non_finite_blends_refuse_typed_and_sticky_on(gpu_context: GpuContext) {
    let compositor = Compositor::new(gpu_context);
    for (mode, s, d, label) in [
        (BlendMode::Add, 40_000.0, 40_000.0, "Add(40000, 40000)"),
        (BlendMode::Multiply, 256.0, 256.0, "Multiply(256, 256)"),
        (BlendMode::Add, f32::NAN, 0.5, "forced NaN"),
        (BlendMode::Screen, f32::INFINITY, 0.5, "forced inf"),
    ] {
        let (gpu, cpu) = pair_lanes(&compositor, mode, grey4(d), grey4(s));
        refused(gpu, 1, label);
        refused(cpu, 1, label);
    }
    let (d, s, cover) = (
        row(&[grey4(256.0)]),
        row(&[grey4(256.0)]),
        row(&[grey4(0.5)]),
    );
    let layers = [
        pixels_layer(&d, BlendMode::Normal),
        pixels_layer(&s, BlendMode::Multiply),
        pixels_layer(&cover, BlendMode::Normal),
    ];
    refused(
        compositor.render_working((1, 1), &layers),
        1,
        "sticky under a cover",
    );
    refused(
        twin::render_working((1, 1), &layers, None),
        1,
        "twin sticky",
    );
    let (gpu, cpu) = pair_lanes(&compositor, BlendMode::Add, grey4(2.0), grey4(2.0));
    assert_eq!(gpu.unwrap().pixels, [4.0, 4.0, 4.0, 1.0]);
    assert_eq!(cpu.unwrap().pixels, [4.0, 4.0, 4.0, 1.0]);
}

/// R10 / N6 R-C: through every document render entry the refusal names the
/// offending clip and project frame; a real `Normal` pixel-layer overflow
/// keeps the CC3 contract (unchecked fast path, no refusal).
fn r10_refusal_names_clip_and_frame_on_every_path_on(context: GpuContext) {
    let mut r = FrameRenderer::new(context);
    let boost = |id| primary(id, &[("exposure_milli_stops", 5_000)]);
    let hot = |id, blend| solid(id, [255; 3], blend, vec![boost(1), boost(2)]);
    let document = document(vec![hot(1, BlendMode::Normal), hot(2, BlendMode::Multiply)]);
    let expected = MediaError::NonFiniteRender {
        layer: 1,
        clip: Some(ClipId(2)),
        at: Some(TimeCode(3)),
    };
    let (at, full, seek) = (
        TimeCode(3),
        RenderScale::FullResolution,
        DecodeStrategy::Seek,
    );
    let size = document.resolution;
    assert_eq!(gpu(&mut r, &document, 3).err(), Some(expected.clone()));
    assert_eq!(
        r.render(&document, at, size, full, seek).err(),
        Some(expected.clone())
    );
    let delivery = r.render_delivery(&document, at, size, full, seek);
    assert_eq!(delivery.err(), Some(expected.clone()));
    assert_eq!(r.twin_working(&document, at, size).err(), Some(expected));
    let boosts = (1..=4).map(boost).collect();
    let normal = self::document(vec![solid(1, [255; 3], BlendMode::Normal, boosts)]);
    let saturated = gpu(&mut r, &normal, 3).expect("a Normal pixel overflow is not refused");
    assert!(
        saturated.pixels[0] >= 65_504.0 || !saturated.pixels[0].is_finite(),
        "2^20 really leaves the f16 range: {}",
        saturated.pixels[0]
    );
}

// ---------------------------------------------------------------- gate 4

/// Gate 4: an adjustment over two tracks equals grade → `blend_over` at
/// zero/half/full strength; stacked adjustments compose in track order;
/// upper tracks are invariant; plus an empty below-stack, a disabled
/// adjustment and a non-`Normal` adjustment blend.
fn adjustment_look_over_section_on(context: GpuContext) {
    let mut r = FrameRenderer::new(context);
    let plate = || solid(1, GREY, BlendMode::Normal, vec![]);
    let half = || solid(2, ORANGE, BlendMode::Normal, vec![opacity(1, 50)]);
    let look = || {
        primary(
            1,
            &[("saturation_percent", -80), ("exposure_milli_stops", 500)],
        )
    };
    let below = matched(&mut r, &document(vec![plate(), half()]), 0, "below-stack");
    let library = Default::default();
    let nodes = crate::color_pipeline::resolve_color_nodes_with(&[look()], &library).unwrap();
    let d = px(&below, 50, 50);
    let uv = [50.5 / W as f32, 50.5 / H as f32];
    let graded = apply_color_nodes_at(&nodes, d, uv, W as f32 / H as f32);
    for strength in [0, 50, 100] {
        let adjust = adjustment(3, BlendMode::Normal, vec![look(), opacity(2, strength)]);
        let label = format!("strength {strength}");
        let image = matched(&mut r, &document(vec![plate(), half(), adjust]), 0, &label);
        let alpha = strength as f32 / 100.0;
        let expected =
            std::array::from_fn::<_, 3, _>(|c| store(alpha * graded[c] + (1.0 - alpha) * d[c]));
        assert_r27(&px(&image, 50, 50), &expected, &label);
        if strength == 0 {
            assert_eq!(
                image.pixels, below.pixels,
                "zero strength is the below-stack"
            );
        }
    }
    let exposure = || {
        adjustment(
            3,
            BlendMode::Normal,
            vec![primary(1, &[("exposure_milli_stops", -700)])],
        )
    };
    let contrast = || {
        adjustment(
            4,
            BlendMode::Normal,
            vec![primary(1, &[("contrast_percent", 60)])],
        )
    };
    let e_c = matched(
        &mut r,
        &document(vec![plate(), half(), exposure(), contrast()]),
        0,
        "E, C",
    );
    let c_e = matched(
        &mut r,
        &document(vec![plate(), half(), contrast(), exposure()]),
        0,
        "C, E",
    );
    assert_ne!(
        px(&e_c, 50, 50),
        px(&c_e, 50, 50),
        "stacked adjustments compose in order"
    );
    let cover = solid(4, BLUE, BlendMode::Normal, vec![]);
    let adjust = adjustment(3, BlendMode::Normal, vec![look()]);
    let covered = matched(
        &mut r,
        &document(vec![plate(), half(), adjust, cover]),
        0,
        "upper",
    );
    assert_r27(
        &px(&covered, 50, 50),
        &working(BLUE),
        "the look never reaches the layer above",
    );
    let alone = document(vec![adjustment(1, BlendMode::Normal, vec![look()])]);
    let alone = matched(&mut r, &alone, 0, "empty below-stack");
    assert_eq!(px(&alone, 3, 3), [0.0; 3], "the black clear stays black");
    let mut off = adjustment(3, BlendMode::Normal, vec![look()]);
    off.enabled = false;
    let disabled = gpu(&mut r, &document(vec![plate(), half(), off]), 0).unwrap();
    assert_eq!(
        disabled.pixels, below.pixels,
        "a disabled adjustment is removed"
    );
    let screened = adjustment(3, BlendMode::Screen, vec![look()]);
    let image = matched(
        &mut r,
        &document(vec![plate(), half(), screened]),
        0,
        "Screen look",
    );
    let expected = blend3(BlendMode::Screen, graded.map(store), d, 1.0);
    assert_r27(&px(&image, 50, 50), &expected, "Screen(G(D), D)");
}

// ---------------------------------------------------------------- gates 5–6

/// R4: `(sign, vertical)` from a geometric descriptor's entry edge.
fn direction(name: &str) -> (i32, bool) {
    match name.rsplit('_').next().unwrap() {
        "left" => (-1, false),
        "right" => (1, false),
        "up" => (-1, true),
        _ => (1, true),
    }
}

/// Grey with hard-edged orange bars over the left 24% (x < 38) and the top
/// 24% (y < 21), so probes discriminate shifts on both axes. Rectangle
/// masks (crop stops at 45%); both edges fall between pixel centres.
fn quartered() -> Vec<Clip> {
    let bar = |id, centre: &str, extent: &str, other: &str| {
        let mask = effect(
            1,
            "mask",
            &[("shape_token", 1), (centre, 12), (extent, 24), (other, 200)],
        );
        solid(id, ORANGE, BlendMode::Normal, vec![mask])
    };
    let plate = solid(1, GREY, BlendMode::Normal, vec![]);
    let left = bar(2, "center_x_percent", "width_percent", "height_percent");
    let top = bar(3, "center_y_percent", "height_percent", "width_percent");
    vec![plate, left, top]
}

fn quartered_d0(x: i32, y: i32) -> [f32; 3] {
    working(if x < 38 || y < 21 { ORANGE } else { GREY })
}

/// Push geometry at `p = 0.5` (R13/R21): whether the entering layer covers
/// `(x, y)`, and the below-stack value there — `D0(x − q)` inside the
/// raster, unshifted `D0(x)` outside it.
fn push_at(name: &str, x: i32, y: i32) -> (bool, [f32; 3]) {
    let (sign, vertical) = direction(name);
    let (along, half) = if vertical { (y, 44) } else { (x, 80) };
    let covered = if sign < 0 {
        along < half
    } else {
        along >= half
    };
    let source = along + sign * half;
    let inside = (0..2 * half).contains(&source);
    let backdrop = match (inside, vertical) {
        (false, _) => quartered_d0(x, y),
        (true, false) => quartered_d0(source, y),
        (true, true) => quartered_d0(x, source),
    };
    (covered, backdrop)
}

const PROBES: [(i32, i32); 6] = [(10, 7), (50, 7), (110, 7), (150, 50), (50, 75), (110, 75)];

/// Gate 5: all four Push directions at the exact midpoint of an odd
/// duration; OOB `D0(x)` through a translucent title; completion equals
/// ordinary composition; plus the R23 blend/adjustment Push compositions.
fn push_midpoint_splits_frame_on(context: GpuContext) {
    let mut r = FrameRenderer::new(context);
    for name in ["push_left", "push_right", "push_up", "push_down"] {
        let mut clips = quartered();
        clips.push(with_transition(
            solid(4, BLUE, BlendMode::Normal, vec![]),
            name,
            5,
        ));
        let image = matched(&mut r, &document(clips), 2, name);
        for (x, y) in PROBES {
            let (covered, backdrop) = push_at(name, x, y);
            let expected = if covered { working(BLUE) } else { backdrop };
            assert_r27(
                &px(&image, x as u32, y as u32),
                &expected,
                &format!("{name} ({x}, {y})"),
            );
        }
        // Transparent title pixels reveal the pushed below-stack, including
        // the unshifted `D0(x)` where the source leaves the raster.
        let (position, y) = if name == "push_up" {
            (TitlePosition::LowerThird, 7)
        } else {
            (TitlePosition::Top, 75)
        };
        let mut clips = quartered();
        clips.push(with_transition(title_clip(4, position, vec![]), name, 5));
        let image = matched(&mut r, &document(clips), 2, &format!("{name} title"));
        for x in [10, 50, 110, 150] {
            let (_, backdrop) = push_at(name, x, y);
            assert_r27(
                &px(&image, x as u32, y as u32),
                &backdrop,
                &format!("{name} title ({x}, {y})"),
            );
        }
        // Completion: from `offset = d − 1` the layer composites ordinarily.
        let translucent = || solid(4, BLUE, BlendMode::Normal, vec![opacity(1, 60)]);
        let mut plain = quartered();
        plain.push(translucent());
        let mut pushed = quartered();
        pushed.push(with_transition(translucent(), name, 5));
        for at in [4, 5] {
            let ordinary = gpu(&mut r, &document(plain.clone()), at).unwrap();
            let done = gpu(&mut r, &document(pushed.clone()), at).unwrap();
            assert_eq!(done.pixels, ordinary.pixels, "{name} completes at {at}");
        }
    }
    // R23: a `Screen` layer under Push blends against the shifted backdrop.
    let mut clips = quartered();
    clips.push(with_transition(
        solid(4, BLUE, BlendMode::Screen, vec![]),
        "push_left",
        5,
    ));
    let image = matched(&mut r, &document(clips), 2, "Screen under push_left");
    for (x, y) in PROBES {
        let (covered, backdrop) = push_at("push_left", x, y);
        let expected = if covered {
            blend3(BlendMode::Screen, working(BLUE), backdrop, 1.0)
        } else {
            backdrop
        };
        assert_r27(
            &px(&image, x as u32, y as u32),
            &expected,
            &format!("Screen push ({x}, {y})"),
        );
    }
    // R13/R23: an adjustment Push grades the unshifted `D0`, Normal and not.
    // ME10: moved left, its quad also rasterizes pixels coverage rejects,
    // which must keep the shifted backdrop, not the `D0` it samples.
    let look = vec![primary(1, &[("saturation_percent", -100)])];
    let moved = effect(2, "transform", &[("x_percent", -30)]);
    for blend in [BlendMode::Normal, BlendMode::Multiply] {
        for effects in [look.clone(), vec![look[0].clone(), moved.clone()]] {
            let mut clips = quartered();
            let label = format!("{blend:?} adjustment push, {} effects", effects.len());
            clips.push(with_transition(
                adjustment(4, blend, effects),
                "push_right",
                5,
            ));
            matched(&mut r, &document(clips), 2, &label);
        }
    }
}

/// Gate 6: the eight Slide/Wipe directions at the midpoint over a
/// stationary backdrop; a scaled-up entering clip is still invisible at
/// `p = 0`; all 15 endpoints; transformed/translucent sources through the
/// last transition frame and the following frame.
fn slide_and_wipe_midpoints_on(context: GpuContext) {
    let mut r = FrameRenderer::new(context);
    let below = gpu(&mut r, &document(quartered()), 0).unwrap();
    for kind in ["slide", "wipe"] {
        for edge in ["left", "right", "up", "down"] {
            let name = format!("{kind}_{edge}");
            let (sign, vertical) = direction(&name);
            let mut clips = quartered();
            clips.push(with_transition(
                solid(4, BLUE, BlendMode::Normal, vec![]),
                &name,
                5,
            ));
            let image = matched(&mut r, &document(clips), 2, &name);
            for (x, y) in PROBES {
                let (along, half) = if vertical { (y, 44) } else { (x, 80) };
                let covered = if sign < 0 {
                    along < half
                } else {
                    along >= half
                };
                let expected = if covered {
                    working(BLUE)
                } else {
                    quartered_d0(x, y)
                };
                assert_r27(
                    &px(&image, x as u32, y as u32),
                    &expected,
                    &format!("{name} ({x}, {y})"),
                );
            }
            let scaled = effect(1, "transform", &[("scale_percent", 400)]);
            let mut clips = quartered();
            clips.push(with_transition(
                solid(4, BLUE, BlendMode::Normal, vec![scaled]),
                &name,
                5,
            ));
            let start = gpu(&mut r, &document(clips), 0).unwrap();
            assert_eq!(
                start.pixels, below.pixels,
                "{name}: scaled-up entering invisible at p = 0"
            );
        }
    }
    // All 15 endpoints: the entering layer is absent at `p = 0` except the
    // colour fades (opaque colour, M20); every row is ordinary at `d − 1`.
    let mut plain = quartered();
    plain.push(solid(4, BLUE, BlendMode::Normal, vec![]));
    let ordinary = gpu(&mut r, &document(plain), 4).unwrap();
    for descriptor in TRANSITION_DESCRIPTORS {
        let mut clips = quartered();
        clips.push(with_transition(
            solid(4, BLUE, BlendMode::Normal, vec![]),
            descriptor.name,
            5,
        ));
        let start = gpu(&mut r, &document(clips), 0).unwrap();
        let expected = match descriptor.name {
            "fade_from_black" => [0.0, 0.0, 0.0, 1.0],
            "fade_from_white" => [1.0; 4],
            name => {
                assert_eq!(start.pixels, below.pixels, "{name} starts invisible");
                continue;
            }
        };
        assert!(
            start.pixels.chunks(4).all(|p| p == expected),
            "{}",
            descriptor.name
        );
    }
    for descriptor in TRANSITION_DESCRIPTORS {
        let mut clips = quartered();
        clips.push(with_transition(
            solid(4, BLUE, BlendMode::Normal, vec![]),
            descriptor.name,
            5,
        ));
        let end = gpu(&mut r, &document(clips), 4).unwrap();
        assert_eq!(
            end.pixels, ordinary.pixels,
            "{} ends ordinary",
            descriptor.name
        );
    }
    // Transformed and translucent sources, through the last frame and after.
    let moved = || {
        effect(
            1,
            "transform",
            &[
                ("scale_percent", 60),
                ("x_percent", 20),
                ("rotation_centidegrees", 1_500),
            ],
        )
    };
    let sources = [
        solid(4, BLUE, BlendMode::Normal, vec![moved(), opacity(2, 70)]),
        title_clip(4, TitlePosition::Center, vec![moved()]),
    ];
    for source in sources {
        for name in ["slide_right", "wipe_up", "push_down", "slide_down"] {
            let mut clips = quartered();
            clips.push(with_transition(source.clone(), name, 5));
            let document = document(clips);
            for at in [1, 3, 4, 5] {
                matched(
                    &mut r,
                    &document,
                    at,
                    &format!("{name} transformed at {at}"),
                );
            }
        }
    }
}

/// ME9 (G4): the value Windows WARP produced for `slide_right` over the
/// transformed title at frame 3 (run 36222189672) misses the unit 1e-3
/// against the exact twin but lies in the 8-bit sub-texel envelope.
fn warp_midpoint_departure_lies_within_the_envelope_on(context: GpuContext) {
    let mut r = FrameRenderer::new(context);
    let moved = [
        ("scale_percent", 60),
        ("x_percent", 20),
        ("rotation_centidegrees", 1_500),
    ];
    let title = title_clip(
        4,
        TitlePosition::Center,
        vec![effect(1, "transform", &moved)],
    );
    let mut clips = quartered();
    clips.push(with_transition(title, "slide_right", 5));
    let document = document(clips);
    let (at, size, value) = (TimeCode(3), document.resolution, 6708 * 4);
    let twin = r.twin_working(&document, at, size).unwrap();
    let slack = r.twin_envelope(&document, at, size).unwrap();
    let (warp, exact) = (0.330_810_55_f32, twin.pixels[value]);
    println!(
        "pixel 6708: twin {exact} slack {} WARP {warp}",
        slack[value]
    );
    assert_eq!(exact, 0.332_031_25, "the CI twin value");
    assert!(!r27_close(warp, exact, 0.0) && r27_close(warp, exact, slack[value]));
}

/// R12/R13/B8 copy counts per frame: Normal-only stacks (Slide/Wipe too) 0;
/// an ordinary special layer 1; a Normal Push 1; a non-`Normal` Push 2;
/// any adjustment Push 2 (errata ME2, ME10).
fn accumulator_copy_counts_on(context: GpuContext) {
    let mut r = FrameRenderer::new(context);
    let top = |blend, transition: Option<&str>, adjust: bool| {
        let clip = if adjust {
            adjustment(2, blend, vec![primary(1, &[("saturation_percent", -50)])])
        } else {
            solid(2, ORANGE, blend, vec![opacity(1, 50)])
        };
        match transition {
            Some(name) => with_transition(clip, name, 5),
            None => clip,
        }
    };
    // (clip, copies mid-transition, copies after it)
    let cases = [
        (top(BlendMode::Normal, None, false), 0, 0),
        (top(BlendMode::Normal, Some("slide_left"), false), 0, 0),
        (top(BlendMode::Normal, Some("wipe_down"), false), 0, 0),
        (top(BlendMode::Screen, None, false), 1, 1),
        (top(BlendMode::Normal, None, true), 1, 1),
        (top(BlendMode::Normal, Some("push_left"), false), 1, 0),
        (top(BlendMode::Normal, Some("push_left"), true), 2, 1),
        (top(BlendMode::Add, Some("push_up"), false), 2, 1),
        (top(BlendMode::Overlay, Some("push_up"), true), 2, 1),
    ];
    for (clip, during, after) in cases {
        let label = format!(
            "{:?} {:?} {:?}",
            clip.content, clip.blend_mode, clip.transition_in
        );
        let document = document(vec![solid(1, GREY, BlendMode::Normal, vec![]), clip]);
        for (at, copies) in [(2, during), (6, after)] {
            let before = r.accumulator_copies();
            gpu(&mut r, &document, at).unwrap();
            assert_eq!(r.accumulator_copies() - before, copies, "{label} at {at}");
        }
    }
}

// ---------------------------------------------------------------- gate 7

/// Gate 7: a solid card with opacity and transform; the solid/title
/// same-colour identity through the shared generated-content input (R3/B6).
fn solid_title_card_renders_on(context: GpuContext) {
    let mut r = FrameRenderer::new(context);
    let transform = effect(2, "transform", &[("scale_percent", 50), ("x_percent", 10)]);
    let card = solid(
        2,
        ACCENT,
        BlendMode::Normal,
        vec![opacity(1, 80), transform],
    );
    let plate = solid(1, GREY, BlendMode::Normal, vec![]);
    let image = matched(&mut r, &document(vec![plate, card]), 0, "solid card");
    let (s, d) = (working(ACCENT), working(GREY));
    let over = std::array::from_fn::<_, 3, _>(|c| store(0.8 * s[c] + 0.2 * d[c]));
    assert_r27(&px(&image, 96, 45), &over, "card centre");
    assert_r27(&px(&image, 2, 2), &d, "outside the card");

    assert_eq!(
        title_color(2).unwrap().rgba,
        [ACCENT[0], ACCENT[1], ACCENT[2], u8::MAX]
    );
    let resolution = (1280, 720);
    let title = Title {
        text: "MO2".to_owned(),
        font_size_token: 2,
        color_token: 2,
        position: TitlePosition::Center,
        background_scrim: false,
        ..Title::default()
    };
    let raster = crate::title::TitleRasterizer::new()
        .rasterize(&title, resolution)
        .unwrap();
    let text = clip(1, ClipContent::Title(title), BlendMode::Normal, vec![]);
    let titled = gpu(&mut r, &document_sized(resolution, vec![text]), 0).unwrap();
    let flat = document_sized(
        resolution,
        vec![solid(1, ACCENT, BlendMode::Normal, vec![])],
    );
    let flat = gpu(&mut r, &flat, 0).unwrap();
    let opaque = raster
        .rgba
        .chunks(4)
        .enumerate()
        .filter(|(_, pixel)| pixel[3] == u8::MAX);
    let mut compared = 0;
    for (index, _) in opaque {
        let texel = index * 4..index * 4 + 4;
        assert_eq!(
            titled.pixels[texel.clone()],
            flat.pixels[texel],
            "texel {index}"
        );
        compared += 1;
    }
    assert!(
        compared > 100,
        "the identity compares {compared} opaque title texels"
    );
}

// ---------------------------------------------------------------- R32, R22

/// R32 seam: generated pixels with varying alpha under non-`Normal` blends
/// flow through the real `compositor_layers` + branch + twin assembly.
fn r32_generated_alpha_under_blend_matches_twin_on(context: GpuContext) {
    let mut r = FrameRenderer::new(context);
    for blend in [BlendMode::Screen, BlendMode::Multiply, BlendMode::Add] {
        let mut text = title_clip(4, TitlePosition::Center, vec![opacity(1, 80)]);
        text.blend_mode = blend;
        let mut clips = quartered();
        clips.push(text);
        matched(&mut r, &document(clips), 0, &format!("{blend:?} title"));
    }
}

/// R22: the M20 linear gain ramp applies verbatim to all 15 descriptors
/// (type-independent), to linked audio, and a disabled clip has no audio.
#[test]
fn r22_audio_ramp_is_type_independent() {
    let fps = Rational::new(30, 1).unwrap();
    let gains = |clip: &Clip| {
        let shaping = ClipAudioShaping::new(clip, TimeCode(9), 3_000, fps);
        (0..1_000)
            .step_by(7)
            .map(|sample| shaping.gain_at(sample))
            .collect::<Vec<_>>()
    };
    let media = || clip(1, ClipContent::Media, BlendMode::Normal, vec![]);
    let reference = gains(&with_transition(media(), "crossfade", 7));
    assert!(reference.iter().any(|gain| *gain < 1.0), "the ramp is live");
    for descriptor in TRANSITION_DESCRIPTORS {
        let clip = with_transition(media(), descriptor.name, 7);
        assert_eq!(gains(&clip), reference, "{}", descriptor.name);
    }
    // Linked audio: the audio-track partner of a pushed clip ramps alike.
    let mut video = with_transition(media(), "push_left", 7);
    video.link = Some(LinkId(1));
    let mut audio = video.clone();
    audio.id = ClipId(2);
    assert_eq!(gains(&audio), reference, "linked audio");
    let mut linked = Document {
        duration: TimeCode(9),
        media_pool: vec![kinewright_core::MediaAsset {
            id: AssetId::default(),
            path: "linked.mp4".into(),
            name: "linked".to_owned(),
            duration: TimeCode(60),
            fps,
            kind: kinewright_core::MediaKind::AudioVideo,
            resolution: Some((W, H)),
            source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
            color_description: kinewright_core::ColorDescription::default(),
            assumed_from: None,
        }],
        ..Document::default()
    };
    for (id, kind, clip) in [(1, TrackKind::Video, video), (2, TrackKind::Audio, audio)] {
        linked.tracks.push(Track {
            id: TrackId(id),
            kind,
            sync_lock: true,
            clips: vec![clip],
        });
    }
    let range = TimeCode(0)..TimeCode(9);
    let segments = timeline_audio_segments(&linked, range.clone()).unwrap();
    assert!(
        segments.iter().any(|segment| segment.clip == ClipId(2)),
        "linked audio plays"
    );
    // A disabled clip contributes no audio, transition or not.
    for track in &mut linked.tracks {
        track.clips[0].enabled = false;
    }
    assert!(timeline_audio_segments(&linked, range).unwrap().is_empty());
}

// ---------------------------------------------------------------- pins

/// The pre-MO2 records for this adapter, or `None` (logged) where no
/// environment baseline was recorded from the pre-MO2 tree.
fn pre_mo2(gpu: &GpuContext) -> Option<Vec<mo2_identity_corpus::Record>> {
    let path = mo2_identity_corpus::baseline_path(gpu);
    let Ok(bytes) = std::fs::read(&path) else {
        eprintln!(
            "SKIPPED: no pre-MO2 baseline {} for this adapter",
            path.display()
        );
        return None;
    };
    Some(mo2_identity_corpus::decode(&bytes))
}

fn post_mo2(gpu: GpuContext) -> (Vec<mo2_identity_corpus::Record>, u64) {
    crate::initialize_ffmpeg().expect("FFmpeg should initialize");
    let media = mo2_identity_corpus::source();
    let mut r = FrameRenderer::new(gpu);
    let records = mo2_identity_corpus::render(&mut r, media.path());
    (records, r.accumulator_copies())
}

fn same_bytes(post: &[u8], pre: &[u8], label: &str, buffer: &str) {
    assert_eq!(post.len(), pre.len(), "{label}: {buffer} size");
    if let Some(at) = post.iter().zip(pre).position(|(a, b)| a != b) {
        panic!(
            "{label}: {buffer} byte {at} differs pre/post MO2 ({} vs {})",
            post[at], pre[at]
        );
    }
}

/// R15 pin (B8): a `Normal`-only corpus renders byte-identical working and
/// monitor buffers before (97fc937, recorded per adapter) and after MO2,
/// and every frame takes the fast path — zero accumulator copies.
fn normal_pre_post_identity_on(gpu: GpuContext) {
    let pre = pre_mo2(&gpu);
    let (post, copies) = post_mo2(gpu);
    assert_eq!(
        copies, 0,
        "a Normal-only stack never copies the accumulator"
    );
    let Some(pre) = pre else {
        return;
    };
    assert_eq!(post.len(), pre.len());
    for (post, pre) in post.iter().zip(&pre) {
        assert_eq!(post.label, pre.label);
        same_bytes(&post.working, &pre.working, &post.label, "working");
        same_bytes(&post.monitor, &pre.monitor, &post.label, "monitor");
    }
}

/// CC8 G2 through the MO2 window: the SDR bytes — monitor RGBA8 and the
/// delivery RGBA64LE handed to the encoder — are identical pre/post MO2 on
/// the same adapter (per-environment baselines).
fn cc8_g2_sdr_identity_on(gpu: GpuContext) {
    let Some(pre) = pre_mo2(&gpu) else {
        return;
    };
    let (post, _) = post_mo2(gpu);
    assert_eq!(post.len(), pre.len());
    for (post, pre) in post.iter().zip(&pre) {
        same_bytes(&post.monitor, &pre.monitor, &post.label, "monitor");
        same_bytes(&post.delivery, &pre.delivery, &post.label, "delivery");
    }
}

/// Both GPU lanes (R27): each body runs on the default lane (lavapipe) as
/// its §13-named test, and all of them run on the physical adapter in one
/// `--ignored` test (the NVIDIA lane).
macro_rules! gpu_lanes {
    ($($name:ident => $body:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                if let Some(gpu) = fixture_gpu_or_skip() {
                    $body(gpu);
                }
            }
        )*

        #[test]
        #[ignore = "requires a physical GPU adapter; run with --ignored"]
        fn mo2_gates_on_hardware() {
            let gpu = crate::cc1_fixtures::hardware_gpu().context();
            $( $body(gpu.clone()); )*
        }
    };
}

gpu_lanes! {
    screen_light_leak_matches_twin => screen_light_leak_matches_twin_on,
    multiply_callout_matches_twin => multiply_callout_matches_twin_on,
    r9b_colour_fade_masked_blend_matches_twin => r9b_colour_fade_masked_blend_matches_twin_on,
    r10_refusal_names_clip_and_frame_on_every_path => r10_refusal_names_clip_and_frame_on_every_path_on,
    adjustment_look_over_section => adjustment_look_over_section_on,
    push_midpoint_splits_frame => push_midpoint_splits_frame_on,
    slide_and_wipe_midpoints => slide_and_wipe_midpoints_on,
    warp_midpoint_departure_lies_within_the_envelope => warp_midpoint_departure_lies_within_the_envelope_on,
    accumulator_copy_counts => accumulator_copy_counts_on,
    solid_title_card_renders => solid_title_card_renders_on,
    r32_generated_alpha_under_blend_matches_twin => r32_generated_alpha_under_blend_matches_twin_on,
    remaining_modes_match_twin => remaining_modes_match_twin_on,
    r9b_opaque_accumulator_vector => r9b_opaque_accumulator_vector_on,
    r10_non_finite_blends_refuse_typed_and_sticky => r10_non_finite_blends_refuse_typed_and_sticky_on,
    normal_pre_post_identity => normal_pre_post_identity_on,
    cc8_g2_sdr_identity => cc8_g2_sdr_identity_on,
}

/// The B1 reviewers' probes, retained (fix round 1).
#[path = "mo2_review_probes.rs"]
mod review_probes;
