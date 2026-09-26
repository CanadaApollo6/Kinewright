//! MO1 Part B render gates: transform goldens (R26), still hold (R27), enable
//! identity (R28), and the §10 B-gates that render.
//!
//! Two lanes: compositor-level goldens over hand-built rasters (cheap,
//! pixel-exact geometry pins through the same WGSL and `params_for` the
//! production path uses) and render-level gates through `FrameRenderer` +
//! decode (proving the shared path carries motion/stills/enable). R26 names
//! no CPU-reference twin — MO2 builds it (R26 re-gated there).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use kinewright_core::{
    AssetId, AudioMix, AutomationCurve, Clip, ClipContent, ClipId, ColorBitDepth, ColorContext,
    ColorDescription, ColorMatrix, ColorPrimaries, ColorProvenance, ColorRange, ColorTransfer,
    ColorWhitePoint, Document, Effect, EffectId, ExportCancellation, ExportSettings, FrameTexture,
    FreezeFrame, Keyframe, KeyframeInterpolation, MediaAsset, MediaCatalog, MediaKind, Operation,
    ParamValue, Rational, TimeCode, Track, TrackId, TrackKind,
};

use crate::cc1_fixtures::{
    DELIVERY_CODEC_MAX, DELIVERY_CODEC_MEAN, DELIVERY_CODEC_P99, abs_code_diff_rgb,
    delivery_frame_to_rgba8,
};
use crate::compositor::{Compositor, CompositorLayer, LayerMode};
use crate::decode::probe_path;
use crate::export::mix_audio;
use crate::gpu_test_support::fixture_gpu_or_skip;
use crate::render::{DecodeStrategy, FrameRenderer, RenderScale};
use crate::test_support::{GeneratedMedia, TempDirectory, single_clip_document};
use crate::timeline::TransitionRenderParams;

/// Prefer the deterministic software fallback adapter; fail loudly when no
/// adapter exists unless skipping was explicitly permitted.
fn fallback() -> Option<Compositor> {
    Some(Compositor::new(fixture_gpu_or_skip()?))
}

fn pixel(frame: &FrameTexture, x: u32, y: u32) -> &[u8] {
    let index = usize::try_from((y * frame.width + x) * 4).unwrap();
    &frame.rgba[index..index + 4]
}

fn assert_pixel_close(actual: &[u8], expected: [u8; 4], tolerance: u8) {
    for (channel, expected) in actual.iter().zip(expected) {
        assert!(
            channel.abs_diff(expected) <= tolerance,
            "pixel {actual:?} differs from expected {expected:?}"
        );
    }
}

fn transform_effect(id: u64, parameters: &[(&str, i64)]) -> Effect {
    Effect {
        enabled: true,
        enabled_curve: None,
        id: EffectId(id),
        name: "transform".to_owned(),
        parameters: parameters
            .iter()
            .map(|(name, value)| ((*name).to_owned(), ParamValue::Integer(*value)))
            .collect(),
        keyframes: BTreeMap::new(),
    }
}

/// A quadrant raster: red TL, green TR, blue BL, white BR. Every rotation,
/// mirror, and axis swap permutes the quadrants observably.
fn quadrants(width: u32, height: u32) -> FrameTexture {
    let mut rgba = Vec::with_capacity(usize::try_from(width * height * 4).unwrap());
    for y in 0..height {
        for x in 0..width {
            let quad = match (x < width / 2, y < height / 2) {
                (true, true) => [255, 0, 0, 255],
                (false, true) => [0, 255, 0, 255],
                (true, false) => [0, 0, 255, 255],
                (false, false) => [255, 255, 255, 255],
            };
            rgba.extend_from_slice(&quad);
        }
    }
    FrameTexture {
        width,
        height,
        rgba: Arc::new(rgba),
    }
}

/// An L on black: a top bar plus a left bar. Asymmetric under every rotation
/// and reflection, so the L golden distinguishes each wrong order, sign,
/// and shear (R3).
fn l_raster(width: u32, height: u32) -> FrameTexture {
    let bar = height / 6;
    let stem = width / 8;
    let mut rgba = Vec::with_capacity(usize::try_from(width * height * 4).unwrap());
    for y in 0..height {
        for x in 0..width {
            let lit = y < bar || x < stem;
            rgba.extend_from_slice(if lit {
                &[255, 255, 255, 255]
            } else {
                &[0, 0, 0, 255]
            });
        }
    }
    FrameTexture {
        width,
        height,
        rgba: Arc::new(rgba),
    }
}

/// A vertical edge: black left half, white right half — for sub-pixel edge
/// tracking (gate 11, the 5 bp pin).
fn vertical_edge(width: u32, height: u32) -> FrameTexture {
    let mut rgba = Vec::with_capacity(usize::try_from(width * height * 4).unwrap());
    for _ in 0..height {
        for x in 0..width {
            rgba.extend_from_slice(if x < width / 2 {
                &[0, 0, 0, 255]
            } else {
                &[255, 255, 255, 255]
            });
        }
    }
    FrameTexture {
        width,
        height,
        rgba: Arc::new(rgba),
    }
}

/// A centred white square on black — the S1 order golden's silhouette
/// source (a square's silhouette is rotation-invariant, so only the
/// scale/rotate order moves the box).
fn centred_square(size: u32, square: u32) -> FrameTexture {
    let margin = (size - square) / 2;
    let mut rgba = Vec::with_capacity(usize::try_from(size * size * 4).unwrap());
    for y in 0..size {
        for x in 0..size {
            let lit = x >= margin && x < margin + square && y >= margin && y < margin + square;
            rgba.extend_from_slice(if lit {
                &[255, 255, 255, 255]
            } else {
                &[0, 0, 0, 255]
            });
        }
    }
    FrameTexture {
        width: size,
        height: size,
        rgba: Arc::new(rgba),
    }
}

fn render_layer(
    compositor: &Compositor,
    resolution: (u32, u32),
    frame: &FrameTexture,
    effects: &[Effect],
) -> FrameTexture {
    compositor
        .render(
            resolution,
            &[CompositorLayer {
                frame,
                effects,
                transition: TransitionRenderParams::default(),
                mode: LayerMode::NORMAL,
            }],
        )
        .expect("the golden layer should render")
}

/// A tagged 64 × 36 grey source (CC5 §9.1 raster size).
fn mo1_source(label: &str) -> GeneratedMedia {
    GeneratedMedia::ffmpeg(
        label,
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=gray:size=64x36:rate=25:duration=1",
            "-frames:v",
            "25",
            "-c:v",
            "ffv1",
            "-pix_fmt",
            "yuv444p",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-colorspace",
            "bt709",
            "-color_range",
            "tv",
        ],
        "mkv",
    )
}

/// Stage a single-clip document on a decodable source with the managed
/// description stamped (untagged fixtures cannot enter managed decode).
fn mo1_document(media: &GeneratedMedia, effects: Vec<Effect>) -> Document {
    let mut asset = probe_path(media.path(), AssetId(1)).expect("the MO1 source should probe");
    asset.color_description = ColorDescription {
        primaries: ColorPrimaries::Bt709,
        transfer: ColorTransfer::Bt709,
        matrix: ColorMatrix::Bt709,
        range: ColorRange::Limited,
        white_point: ColorWhitePoint::D65,
        bit_depth: ColorBitDepth::Eight,
        confidence_basis_points: 10_000,
        provenance: ColorProvenance::UserOverride,
    };
    let mut document = single_clip_document(asset);
    document.tracks[0].clips[0].effects = effects;
    document
}

fn render_frame(document: &Document, at: TimeCode) -> FrameTexture {
    let gpu = fixture_gpu_or_skip().expect("the render gate needs an adapter");
    let mut renderer = FrameRenderer::new(gpu);
    renderer
        .render(
            document,
            at,
            document.resolution,
            RenderScale::FullResolution,
            DecodeStrategy::Seek,
        )
        .expect("the render gate frame should render")
}

/// Stage a still document: an `Image` asset under a `Freeze` clip spanning
/// `span_frames` project frames (R8/MR21: the clip's `source_range` spans
/// the hold; `source_frame` 0 samples the still's one frame).
fn mo1_still_document(
    media: &GeneratedMedia,
    span_frames: i64,
    resolution: (u32, u32),
    effects: Vec<Effect>,
) -> (Document, MediaAsset) {
    let mut asset = probe_path(media.path(), AssetId(1)).expect("the still should probe");
    assert_eq!(asset.kind, MediaKind::Image);
    // The assumed-sRGB stamp an untagged still carries after the agent's
    // source-colour incident resolves (full-range RGB, not video's
    // limited-range BT.709).
    asset.color_description = ColorDescription {
        primaries: ColorPrimaries::Bt709,
        transfer: ColorTransfer::Srgb,
        matrix: ColorMatrix::Rgb,
        range: ColorRange::Full,
        white_point: ColorWhitePoint::D65,
        bit_depth: ColorBitDepth::Eight,
        confidence_basis_points: 10_000,
        provenance: ColorProvenance::UserOverride,
    };
    let document = Document {
        investigator: None,
        catalog: MediaCatalog::default(),
        audio_mix: AudioMix::default(),
        color_context: ColorContext::default(),
        lut_assets: Vec::new(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![Clip {
                enabled: true,
                enabled_curve: None,
                id: ClipId(1),
                asset: asset.id,
                source_range: TimeCode::ZERO..TimeCode(span_frames),
                content: ClipContent::Freeze(FreezeFrame {
                    source_frame: TimeCode::ZERO,
                }),
                timeline_start: TimeCode::ZERO,
                effects,
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
                audio_gain_curve: None,
                blend_mode: kinewright_core::BlendMode::Normal,
            }],
        }],
        media_pool: vec![asset.clone()],
        markers: Vec::new(),
        fps: Rational::default(),
        resolution,
        duration: TimeCode(span_frames),
    };
    document
        .validate()
        .expect("the still document should validate");
    (document, asset)
}

/// A gradient raster sensitive to any transform math (adopted from the
/// reviewer's identity scenarios): horizontal red, vertical green, and a
/// high-frequency blue channel.
fn mo1_gradient(width: u32, height: u32) -> FrameTexture {
    let mut rgba = Vec::new();
    for y in 0..height {
        for x in 0..width {
            rgba.extend_from_slice(&[
                u8::try_from(x * 255 / (width - 1)).unwrap(),
                u8::try_from(y * 255 / (height - 1)).unwrap(),
                u8::try_from(((x ^ y) * 7) % 256).unwrap(),
                255,
            ]);
        }
    }
    FrameTexture {
        width,
        height,
        rgba: Arc::new(rgba),
    }
}

/// MO1 R26/G5: the default transform is the identity — no-effect,
/// bare-transform, and legacy-neutral renders are byte-equal in the same
/// run on gradient content at three sizes. Same-run differential, so no
/// adapter bytes are recorded (a recorded SHA pin broke on WARP/Mesa).
#[test]
fn mo1_default_transform_identity_probes() {
    let Some(compositor) = fallback() else {
        return;
    };
    let neutral = [transform_effect(
        1,
        &[("scale_percent", 100), ("x_percent", 0), ("y_percent", 0)],
    )];
    let bare = [transform_effect(1, &[])];
    for (resolution, source) in [
        ((64, 36), mo1_gradient(64, 36)),
        ((1920, 1080), mo1_gradient(97, 53)),
        ((1080, 1920), mo1_gradient(640, 360)),
    ] {
        let plain = render_layer(&compositor, resolution, &source, &[]);
        let with_bare = render_layer(&compositor, resolution, &source, &bare);
        assert_eq!(
            plain.rgba.as_ref(),
            with_bare.rgba.as_ref(),
            "a bare transform must render as no transform at {resolution:?}"
        );
        let with_neutral = render_layer(&compositor, resolution, &source, &neutral);
        assert_eq!(
            plain.rgba.as_ref(),
            with_neutral.rgba.as_ref(),
            "legacy-neutral controls must render as no transform at {resolution:?}"
        );
    }
}

/// MO1 R26/G5: the render-path half of the identity pin — a document
/// carrying a default transform renders the strip byte-equal to the
/// transform-less document (same run, no recorded bytes).
#[test]
fn mo1_render_path_default_transform_is_identity() {
    crate::initialize_ffmpeg().expect("FFmpeg should initialize");
    let media = mo1_quadrant_video("mo1-identity-strip", 10);
    let plain = mo1_document(&media, Vec::new());
    let with_bare = mo1_document(&media, vec![transform_effect(1, &[])]);
    assert_eq!(
        render_strip(&plain, 0..10),
        render_strip(&with_bare, 0..10),
        "a default transform must add nothing through the render path"
    );
}

/// MO1 R26: a 50% master scale shrinks the quadrants to the centre 32 × 18
/// (x 16..48, y 9..27) over the black clear.
#[test]
fn mo1_scale_master_golden() {
    let Some(compositor) = fallback() else {
        return;
    };
    let source = quadrants(64, 36);
    let effects = [transform_effect(1, &[("scale_percent", 50)])];
    let frame = render_layer(&compositor, (64, 36), &source, &effects);

    assert_pixel_close(pixel(&frame, 24, 13), [255, 0, 0, 255], 8);
    assert_pixel_close(pixel(&frame, 40, 13), [0, 255, 0, 255], 8);
    assert_pixel_close(pixel(&frame, 24, 22), [0, 0, 255, 255], 8);
    assert_pixel_close(pixel(&frame, 40, 22), [255, 255, 255, 255], 8);
    assert_pixel_close(pixel(&frame, 0, 0), [0, 0, 0, 255], 2);
    assert_pixel_close(pixel(&frame, 63, 35), [0, 0, 0, 255], 2);
    assert_pixel_close(pixel(&frame, 8, 18), [0, 0, 0, 255], 2);
}

/// MO1 R26: per-axis scale squeezes X alone — full height, centre 32 wide.
#[test]
fn mo1_per_axis_scale_golden() {
    let Some(compositor) = fallback() else {
        return;
    };
    let source = quadrants(64, 36);
    let effects = [transform_effect(1, &[("scale_x_percent", 50)])];
    let frame = render_layer(&compositor, (64, 36), &source, &effects);

    assert_pixel_close(pixel(&frame, 24, 13), [255, 0, 0, 255], 8);
    assert_pixel_close(pixel(&frame, 40, 22), [255, 255, 255, 255], 8);
    // Full height survives: top and bottom rows still carry the image.
    assert_pixel_close(pixel(&frame, 24, 0), [255, 0, 0, 255], 8);
    assert_pixel_close(pixel(&frame, 24, 35), [0, 0, 255, 255], 8);
    // …while the squeezed sides clear.
    assert_pixel_close(pixel(&frame, 8, 18), [0, 0, 0, 255], 2);
    assert_pixel_close(pixel(&frame, 56, 18), [0, 0, 0, 255], 2);
}

/// MO1 R26: positive rotation reads clockwise (Premiere): +90° moves the
/// red TL quadrant to TR and the blue BL quadrant to TL.
#[test]
fn mo1_rotation_sign_golden() {
    let Some(compositor) = fallback() else {
        return;
    };
    let source = quadrants(64, 36);
    let effects = [transform_effect(1, &[("rotation_centidegrees", 9_000)])];
    let frame = render_layer(&compositor, (64, 36), &source, &effects);

    assert_pixel_close(pixel(&frame, 48, 9), [255, 0, 0, 255], 8);
    assert_pixel_close(pixel(&frame, 16, 9), [0, 0, 255, 255], 8);
    assert_pixel_close(pixel(&frame, 48, 27), [0, 255, 0, 255], 8);
    assert_pixel_close(pixel(&frame, 16, 27), [255, 255, 255, 255], 8);
}

/// MO1 R26: the R3 aspect correction — a 90° rotation preserves pixel
/// dimensions instead of shearing in NDC. At 50% the whole rotated L stays
/// on frame: the 64 × 6 top bar becomes a 3-wide 32-tall right bar (x
/// 38..41, y 2..34 — a naive NDC rotation would shear it ~5.3 wide and land
/// it off-centre), and the 8 × 36 stem a centred 18 × 4 top bar (x 23..41,
/// y 2..6).
#[test]
fn mo1_rotation_aspect_correction_golden() {
    let Some(compositor) = fallback() else {
        return;
    };
    let source = l_raster(64, 36);
    let effects = [transform_effect(
        1,
        &[("scale_percent", 50), ("rotation_centidegrees", 9_000)],
    )];
    let frame = render_layer(&compositor, (64, 36), &source, &effects);
    let lit = |x: u32, y: u32| {
        let p = pixel(&frame, x, y);
        assert!(
            p[0] > 128 && p[1] > 128 && p[2] > 128,
            "({x}, {y}) must be lit, got {p:?}"
        );
    };
    let dark = |x: u32, y: u32| {
        let p = pixel(&frame, x, y);
        assert!(
            p[0] < 128 && p[1] < 128 && p[2] < 128,
            "({x}, {y}) must be dark, got {p:?}"
        );
    };

    // Right bar: x 38..41, y 2..34.
    lit(40, 18);
    lit(40, 3);
    lit(40, 33);
    dark(37, 18);
    dark(43, 18);
    dark(40, 1);
    dark(40, 34);
    // Stem bar: x 23..41, y 2..6.
    lit(32, 4);
    lit(24, 4);
    dark(22, 4);
    dark(42, 4);
    dark(32, 1);
    dark(32, 7);
    // Between and around the bars: dark.
    dark(32, 25);
    dark(10, 10);
    dark(55, 10);
    dark(10, 30);
    dark(55, 30);
}

/// MO1 N4 G8 (R2 S1): scale runs before rotation in the vertex stage —
/// `scale_x` 50 squeezes the centred 40-px square to 20 wide, then +90°
/// lays it down as a 40 × 20 bar (x 12..52, y 22..42). Rotating first
/// would stand it up as 20 × 40 instead, so the order swap reds this.
#[test]
fn mo1_scale_before_rotate_order_golden() {
    let Some(compositor) = fallback() else {
        return;
    };
    let source = centred_square(64, 40);
    let effects = [transform_effect(
        1,
        &[("scale_x_percent", 50), ("rotation_centidegrees", 9_000)],
    )];
    let frame = render_layer(&compositor, (64, 64), &source, &effects);
    let lit = |x: u32, y: u32| {
        let p = pixel(&frame, x, y);
        assert!(
            p[0] > 128 && p[1] > 128 && p[2] > 128,
            "({x}, {y}) must be lit, got {p:?}"
        );
    };
    let dark = |x: u32, y: u32| {
        let p = pixel(&frame, x, y);
        assert!(
            p[0] < 128 && p[1] < 128 && p[2] < 128,
            "({x}, {y}) must be dark, got {p:?}"
        );
    };

    // The 40 x 20 bar: x 12..52, y 22..42.
    lit(32, 32);
    lit(13, 32);
    lit(51, 32);
    lit(32, 23);
    lit(32, 41);
    dark(11, 32);
    dark(53, 32);
    dark(32, 21);
    dark(32, 43);
    // The rotate-first 20 x 40 bar would light these instead.
    dark(32, 13);
    dark(13, 13);
}

/// MO1 R26: scaling about the top-left anchor keeps the image in the TL
/// quadrant — pins the anchor's top-left origin and the scale-about-anchor
/// order (a centre anchor would centre the shrunk image instead).
#[test]
fn mo1_anchor_offset_golden() {
    let Some(compositor) = fallback() else {
        return;
    };
    let source = quadrants(64, 36);
    let effects = [transform_effect(
        1,
        &[
            ("scale_percent", 50),
            ("anchor_x_basis_points", 0),
            ("anchor_y_basis_points", 0),
        ],
    )];
    let frame = render_layer(&compositor, (64, 36), &source, &effects);

    assert_pixel_close(pixel(&frame, 8, 4), [255, 0, 0, 255], 8);
    assert_pixel_close(pixel(&frame, 24, 4), [0, 255, 0, 255], 8);
    assert_pixel_close(pixel(&frame, 8, 13), [0, 0, 255, 255], 8);
    assert_pixel_close(pixel(&frame, 24, 13), [255, 255, 255, 255], 8);
    assert_pixel_close(pixel(&frame, 40, 4), [0, 0, 0, 255], 2);
    assert_pixel_close(pixel(&frame, 8, 22), [0, 0, 0, 255], 2);
}

/// MO1 R26: coarse + fine position split folds exactly as the combined
/// coarse — byte-identical renders (the split lane sums in float, so the
/// values are chosen bit-exact: 0.5 + 0.5 == 1.0).
#[test]
fn mo1_coarse_plus_fine_position_golden() {
    let Some(compositor) = fallback() else {
        return;
    };
    let source = quadrants(64, 36);
    let split = [transform_effect(
        1,
        &[("x_percent", 25), ("x_basis_points", 2_500)],
    )];
    let combined = [transform_effect(1, &[("x_percent", 50)])];
    let a = render_layer(&compositor, (64, 36), &source, &split);
    let b = render_layer(&compositor, (64, 36), &source, &combined);
    assert_eq!(
        a.rgba.as_ref(),
        b.rgba.as_ref(),
        "split must equal combined"
    );

    // The shift itself landed: half a frame right, red where green was.
    assert_pixel_close(pixel(&a, 48, 9), [255, 0, 0, 255], 8);
    assert_pixel_close(pixel(&a, 8, 9), [0, 0, 0, 255], 2);
}

/// MO1 R26: the fine lane pins sub-pixel response — a 5-basis-point nudge
/// moves a vertical edge exactly 1 px at 1080p (1920 × 5/10000 = 0.96 px,
/// crossing one pixel boundary).
#[test]
fn mo1_fine_position_nudge_moves_one_pixel_at_1080p() {
    let Some(compositor) = fallback() else {
        return;
    };
    let source = vertical_edge(1920, 1080);
    let edge_column = |frame: &FrameTexture| {
        (0..1920)
            .find(|x| pixel(frame, *x, 540)[0] > 128)
            .expect("the edge must cross mid-row")
    };

    let plain = render_layer(&compositor, (1920, 1080), &source, &[]);
    assert_eq!(edge_column(&plain), 960);

    let nudged = [transform_effect(1, &[("x_basis_points", 5)])];
    let shifted = render_layer(&compositor, (1920, 1080), &source, &nudged);
    assert_eq!(
        edge_column(&shifted),
        961,
        "5 bp at 1080p must move the edge exactly 1 px"
    );
}

/// MO1 R3/R26: the L-shaped 16:9 order golden — scale about the anchor, then
/// aspect-corrected rotation about the anchor, then the offset. The pinned
/// hash distinguishes every wrong order, sign, and shear; the probes below
/// pin the two orderings a hash alone could not explain: the offset applies
/// AFTER rotation (an offset-before-rotation would swing the bar off its
/// probed columns), and rotation runs about the ANCHOR (about-centre would
/// land the shrunk L's red block far from its probed pixels).
#[test]
fn mo1_l_shaped_order_shear_golden_16x9() {
    let Some(compositor) = fallback() else {
        return;
    };
    // Offset after rotation: the aspect golden's right bar (x 38..41)
    // shifts left 32 px to x 6..9 — under offset-before-rotation the NDC
    // offset itself would rotate and the bar would leave these columns.
    let source = l_raster(64, 36);
    let effects = [transform_effect(
        1,
        &[
            ("scale_percent", 50),
            ("rotation_centidegrees", 9_000),
            ("x_percent", -50),
        ],
    )];
    let frame = render_layer(&compositor, (64, 36), &source, &effects);
    let lit = |x: u32, y: u32| {
        let p = pixel(&frame, x, y);
        assert!(
            p[0] > 128 && p[1] > 128 && p[2] > 128,
            "({x}, {y}) must be lit, got {p:?}"
        );
    };
    let dark = |x: u32, y: u32| {
        let p = pixel(&frame, x, y);
        assert!(
            p[0] < 128 && p[1] < 128 && p[2] < 128,
            "({x}, {y}) must be dark, got {p:?}"
        );
    };
    lit(8, 18);
    lit(8, 3);
    lit(8, 33);
    dark(5, 18);
    dark(11, 18);
    dark(40, 18);
    // The stem bar shifted with it: x 23..41 → off-frame left..9.
    lit(4, 4);
    dark(10, 4);
    dark(32, 4);
    // Interior pins: more of the shifted bar, away from every edge.
    lit(7, 12);
    lit(7, 26);
    lit(6, 4);
    dark(12, 4);

    // Rotation about the anchor: 50% about (2500, 2500) then 90° CW about
    // the same point — red lands x 11.5..20.5, y 1..17 (centre (16, 9)),
    // green x 11.5..20.5, y 17..33, blue x 2.5..11.5, y 1..17, white x
    // 2.5..11.5, y 17..33 (all inverse-mapped by hand). About-centre
    // rotation would throw red off-frame top-right instead.
    let source = quadrants(64, 36);
    let effects = [transform_effect(
        1,
        &[
            ("scale_percent", 50),
            ("rotation_centidegrees", 9_000),
            ("anchor_x_basis_points", 2_500),
            ("anchor_y_basis_points", 2_500),
        ],
    )];
    let frame = render_layer(&compositor, (64, 36), &source, &effects);
    assert_pixel_close(pixel(&frame, 16, 9), [255, 0, 0, 255], 8);
    assert_pixel_close(pixel(&frame, 14, 5), [255, 0, 0, 255], 8);
    assert_pixel_close(pixel(&frame, 7, 9), [0, 0, 255, 255], 8);
    assert_pixel_close(pixel(&frame, 16, 25), [0, 255, 0, 255], 8);
    assert_pixel_close(pixel(&frame, 7, 25), [255, 255, 255, 255], 8);
    assert_pixel_close(pixel(&frame, 32, 18), [0, 0, 0, 255], 4);
    assert_pixel_close(pixel(&frame, 0, 30), [0, 0, 0, 255], 4);
    assert_pixel_close(pixel(&frame, 16, 35), [0, 0, 0, 255], 4);
    assert_pixel_close(pixel(&frame, 50, 8), [0, 0, 0, 255], 4);

    // Interior pins: more of each colour block, away from every edge.
    assert_pixel_close(pixel(&frame, 18, 12), [255, 0, 0, 255], 8);
    assert_pixel_close(pixel(&frame, 5, 12), [0, 0, 255, 255], 8);
    assert_pixel_close(pixel(&frame, 18, 28), [0, 255, 0, 255], 8);
    assert_pixel_close(pixel(&frame, 5, 28), [255, 255, 255, 255], 8);
}

/// MO1 R26: the L order golden through the shared decode + timeline path —
/// the same combined transform the compositor golden pins, resolved from a
/// real document over decoded media. Proves the render path carries
/// transform, not just the compositor entry.
#[test]
fn mo1_l_order_golden_through_the_render_path() {
    crate::initialize_ffmpeg().expect("FFmpeg should initialize");
    let media = GeneratedMedia::ffmpeg(
        "mo1-l-render",
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=black:size=64x36:rate=25:duration=1,drawbox=x=0:y=0:w=64:h=6:c=white:t=fill,drawbox=x=0:y=0:w=8:h=36:c=white:t=fill",
            "-frames:v",
            "25",
            "-c:v",
            "ffv1",
            "-pix_fmt",
            "yuv444p",
        ],
        "mkv",
    );
    let effects = vec![transform_effect(
        1,
        &[
            ("scale_percent", 50),
            ("rotation_centidegrees", 9_000),
            ("x_percent", -50),
        ],
    )];
    let document = mo1_document(&media, effects);
    let frame = render_frame(&document, TimeCode::ZERO);
    let lit = |x: u32, y: u32| {
        let p = pixel(&frame, x, y);
        assert!(
            p[0] > 128 && p[1] > 128 && p[2] > 128,
            "({x}, {y}) must be lit, got {p:?}"
        );
    };
    let dark = |x: u32, y: u32| {
        let p = pixel(&frame, x, y);
        assert!(
            p[0] < 128 && p[1] < 128 && p[2] < 128,
            "({x}, {y}) must be dark, got {p:?}"
        );
    };
    // The aspect golden's right bar shifted left 32 px: x 6..9, y 2..34.
    lit(8, 18);
    lit(8, 3);
    lit(8, 33);
    dark(5, 18);
    dark(11, 18);
    dark(40, 18);
    lit(4, 4);
    dark(10, 4);
    dark(32, 4);
    // Interior pins: more of the shifted bar, away from every edge.
    lit(7, 10);
    lit(7, 28);
    dark(14, 18);
    dark(2, 18);
}

/// §10 gate 11, contract half, re-driven through the real `params_for` fold
/// (N3 — the core mirror is deleted): a canonical push-in (master constant,
/// fine ramped) steps ≤ 1 px frame-to-frame on the evaluated effective
/// scale, while one whole master percent stays ~19 px. The lavapipe half is
/// `push_in_step_sub_pixel_on_lavapipe` below.
#[test]
fn push_in_step_sub_pixel() {
    let effect = Effect {
        enabled: true,
        enabled_curve: None,
        id: EffectId(1),
        name: "transform".to_owned(),
        parameters: BTreeMap::from([("scale_percent".to_owned(), ParamValue::Integer(100))]),
        keyframes: BTreeMap::from([(
            "scale_fine_hundredths".to_owned(),
            AutomationCurve {
                keyframes: vec![
                    Keyframe {
                        at: TimeCode(0),
                        value: 10_000,
                        interpolation: KeyframeInterpolation::Linear,
                        tangent_in: 0,
                        tangent_out: 0,
                    },
                    Keyframe {
                        at: TimeCode(50),
                        value: 10_050,
                        interpolation: KeyframeInterpolation::Linear,
                        tangent_in: 0,
                        tangent_out: 0,
                    },
                ],
            },
        )]),
    };
    // Effective width at 1080p through the production fold (f32 in, f64
    // comparison — the conversion is exact).
    let width_px = |frame: i64| {
        let evaluated = effect.evaluated_at(TimeCode(frame));
        let folded = crate::compositor::params_for(&[evaluated], TransitionRenderParams::default());
        f64::from(folded.scale_x) * 1920.0
    };
    let mut worst = 0.0_f64;
    for frame in 0..50 {
        worst = worst.max((width_px(frame + 1) - width_px(frame)).abs());
    }
    assert!(
        worst <= 1.0,
        "a canonical push-in steps ≤ 1 px frame-to-frame, worst {worst}"
    );
    assert!(
        worst >= 0.1,
        "the ramp must actually move (a dead fine lane would step 0): worst {worst}"
    );

    // The fails-rationale through the same fold: one whole master percent
    // is ~19 px and must stay visibly coarse.
    let width_of = |master: i64| {
        let folded = crate::compositor::params_for(
            &[transform_effect(1, &[("scale_percent", master)])],
            TransitionRenderParams::default(),
        );
        f64::from(folded.scale_x) * 1920.0
    };
    let coarse_step = width_of(101) - width_of(100);
    assert!(
        (coarse_step - 19.2).abs() < 0.01,
        "a 1% master step must stay ~19 px, got {coarse_step}"
    );
}

/// §10 gate 11, lavapipe half: a canonical fine ramp (10000 → 11000 over 60
/// frames, left-anchored so the edge travels) steps the rendered edge 0/1
/// px per frame for ~16 px total.
#[test]
fn push_in_step_sub_pixel_on_lavapipe() {
    crate::initialize_ffmpeg().expect("FFmpeg should initialize");
    let media = GeneratedMedia::ffmpeg(
        "mo1-push-in-edge",
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=black:size=320x180:rate=25:duration=3,drawbox=x=160:y=0:w=160:h=180:c=white:t=fill",
            "-frames:v",
            "75",
            "-c:v",
            "ffv1",
            "-pix_fmt",
            "yuv444p",
        ],
        "mkv",
    );
    let mut ramp = transform_effect(
        1,
        &[
            ("anchor_x_basis_points", 0),
            ("anchor_y_basis_points", 5_000),
        ],
    );
    ramp.keyframes.insert(
        "scale_fine_hundredths".to_owned(),
        AutomationCurve {
            keyframes: vec![
                Keyframe {
                    at: TimeCode(0),
                    value: 10_000,
                    interpolation: KeyframeInterpolation::Linear,
                    tangent_in: 0,
                    tangent_out: 0,
                },
                Keyframe {
                    at: TimeCode(60),
                    value: 11_000,
                    interpolation: KeyframeInterpolation::Linear,
                    tangent_in: 0,
                    tangent_out: 0,
                },
            ],
        },
    );
    let document = mo1_document(&media, vec![ramp]);
    assert_eq!(document.resolution, (320, 180));
    assert_push_in_sampler_path(&document);
    let gpu = fixture_gpu_or_skip().expect("the gate needs an adapter");
    let mut renderer = FrameRenderer::new(gpu);
    let edge = |renderer: &mut FrameRenderer, at: i64| {
        let frame = renderer
            .render(
                &document,
                TimeCode(at),
                document.resolution,
                RenderScale::FullResolution,
                DecodeStrategy::Seek,
            )
            .expect("push-in frames should render");
        (0..320)
            .find(|x| pixel(&frame, *x, 90)[0] > 128)
            .expect("the edge must cross mid-row")
    };
    let mut worst_step = 0_u32;
    let mut previous = edge(&mut renderer, 0);
    assert_eq!(previous, 160, "the unscaled edge starts at the centre");
    for at in 1..=60 {
        let column = edge(&mut renderer, at);
        assert!(
            column >= previous,
            "a push-in never moves the edge backwards: {previous} → {column} at {at}"
        );
        worst_step = worst_step.max(column - previous);
        previous = column;
    }
    assert!(
        worst_step <= 1,
        "rendered steps must stay ≤ 1 px, worst {worst_step}"
    );
    assert!(
        previous >= 172,
        "the ramp must travel ~16 px total, ended at {previous}"
    );
}

/// G1: the folded params of the push-in ramp take the filtering sampler
/// off-identity — a fine-only scale is still a scale, and the point
/// sampler would shimmer it. Frame 0 (exact identity) keeps the point
/// fast path. The dummy frame only carries dimensions; the blit gate reads
/// nothing else from the layer.
fn assert_push_in_sampler_path(document: &Document) {
    use crate::compositor::{Compositor as BlitCompositor, params_for};
    let effect = &document.tracks[0].clips[0].effects[0];
    let dummy = FrameTexture {
        width: 320,
        height: 180,
        rgba: Arc::new(vec![0; 320 * 180 * 4]),
    };
    for (at, expect_blit) in [(0, true), (30, false), (60, false)] {
        let evaluated = effect.evaluated_at(TimeCode(at));
        let params = params_for(&[evaluated], TransitionRenderParams::default());
        let layer = CompositorLayer {
            frame: &dummy,
            effects: &[],
            transition: TransitionRenderParams::default(),
            mode: LayerMode::NORMAL,
        };
        assert_eq!(
            BlitCompositor::is_pixel_exact_blit(&layer, &params, 320, 180),
            expect_blit,
            "frame {at} must take the {} sampler",
            if expect_blit { "point" } else { "filtering" }
        );
    }
}

/// §10 gate 1, lavapipe frame pins: an eased scale push-in trimmed +20 at
/// the head renders byte-identically on the surviving frames (keep-outside
/// animation stays fixed in timeline space while the same source frames
/// show through), and trimming back out restores every frame byte-exactly.
#[test]
fn push_in_survives_trim_in_then_out_on_lavapipe() {
    crate::initialize_ffmpeg().expect("FFmpeg should initialize");
    let media = GeneratedMedia::ffmpeg(
        "mo1-trim-quadrants",
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=red:size=64x36:rate=25:duration=3,drawbox=x=32:y=0:w=32:h=36:c=green:t=fill,drawbox=x=0:y=18:w=32:h=18:c=blue:t=fill,drawbox=x=32:y=18:w=32:h=18:c=white:t=fill",
            "-frames:v",
            "75",
            "-c:v",
            "ffv1",
            "-pix_fmt",
            "yuv444p",
        ],
        "mkv",
    );
    let mut push_in = transform_effect(1, &[]);
    push_in.keyframes.insert(
        "scale_percent".to_owned(),
        AutomationCurve {
            keyframes: vec![
                Keyframe {
                    at: TimeCode(10),
                    value: 100,
                    interpolation: KeyframeInterpolation::EaseInOut,
                    tangent_in: 0,
                    tangent_out: 0,
                },
                Keyframe {
                    at: TimeCode(50),
                    value: 120,
                    interpolation: KeyframeInterpolation::Linear,
                    tangent_in: 0,
                    tangent_out: 0,
                },
            ],
        },
    );
    let mut document = mo1_document(&media, vec![push_in]);
    document
        .validate()
        .expect("the gate-1 document should validate");
    let gpu = fixture_gpu_or_skip().expect("the gate needs an adapter");
    let mut renderer = FrameRenderer::new(gpu);
    let render = |renderer: &mut FrameRenderer, document: &Document, at: i64| {
        renderer
            .render(
                document,
                TimeCode(at),
                document.resolution,
                RenderScale::FullResolution,
                DecodeStrategy::Seek,
            )
            .expect("gate-1 frames should render")
            .rgba
            .as_ref()
            .clone()
    };

    let before: Vec<Vec<u8>> = (0..75)
        .map(|at| render(&mut renderer, &document, at))
        .collect();

    Operation::TrimClip {
        clip: ClipId(1),
        new_source: TimeCode(20)..TimeCode(75),
    }
    .apply(&mut document)
    .expect("head trim should apply");
    document
        .validate()
        .expect("the trimmed document should validate");
    for at in 20..75 {
        assert_eq!(
            render(&mut renderer, &document, at),
            before[usize::try_from(at).unwrap()],
            "trimmed-in frame {at} must match its pre-trim bytes"
        );
    }

    Operation::TrimClip {
        clip: ClipId(1),
        new_source: TimeCode(0)..TimeCode(75),
    }
    .apply(&mut document)
    .expect("trim-out should apply");
    document
        .validate()
        .expect("the restored document should validate");
    for at in 0..75 {
        assert_eq!(
            render(&mut renderer, &document, at),
            before[usize::try_from(at).unwrap()],
            "restored frame {at} must match its pre-trim bytes"
        );
    }
}

/// A quadrant still for hold/Ken Burns goldens (patterned: a solid would
/// hide every transform).
fn mo1_quadrant_still(label: &str, width: u32, height: u32) -> GeneratedMedia {
    GeneratedMedia::ffmpeg(
        label,
        &[
            "-f",
            "lavfi",
            "-i",
            &format!(
                "color=c=red:size={width}x{height}:rate=1:duration=1,drawbox=x={w}:y=0:w={w}:h={height}:c=green:t=fill,drawbox=x=0:y={h}:w={w}:h={h}:c=blue:t=fill,drawbox=x={w}:y={h}:w={w}:h={h}:c=white:t=fill",
                w = width / 2,
                h = height / 2,
            ),
            "-frames:v",
            "1",
            "-c:v",
            "png",
        ],
        "png",
    )
}

/// MO1 R27: a still renders identically across its whole five-second span,
/// decoding once — the frame cache pins the one still frame (one seek, no
/// evictions) rather than churning it. MO1 N4 G8: the same parity holds on
/// the export (`Sequential`) decode path, byte-equal to the `Seek` path.
#[test]
fn still_hold_parity_across_five_seconds() {
    crate::initialize_ffmpeg().expect("FFmpeg should initialize");
    let media = mo1_quadrant_still("mo1-hold", 64, 36);
    let (document, _) = mo1_still_document(&media, 150, (64, 36), Vec::new());
    let gpu = fixture_gpu_or_skip().expect("the gate needs an adapter");
    let mut first_by_strategy = Vec::new();
    for strategy in [DecodeStrategy::Seek, DecodeStrategy::Sequential] {
        let mut renderer = FrameRenderer::new(gpu.clone());
        let render = |renderer: &mut FrameRenderer, at: i64| {
            renderer
                .render(
                    &document,
                    TimeCode(at),
                    document.resolution,
                    RenderScale::FullResolution,
                    strategy,
                )
                .expect("hold frames should render")
                .rgba
                .as_ref()
                .clone()
        };

        let first = render(&mut renderer, 0);
        for at in [1, 2, 3, 37, 74, 75, 112, 148, 149] {
            assert_eq!(
                render(&mut renderer, at),
                first,
                "hold frame {at} must equal frame 0 ({strategy:?})"
            );
        }
        assert_eq!(
            renderer.video_seek_count(),
            1,
            "the still must decode exactly once across its span ({strategy:?})"
        );
        assert_eq!(
            renderer.cache_eviction_count(),
            0,
            "the pinned still frame must never evict ({strategy:?})"
        );
        first_by_strategy.push(first);
    }
    assert_eq!(
        first_by_strategy[0], first_by_strategy[1],
        "the export decode path must render hold bytes equal to preview"
    );
}

/// §10 gate 3: a fitted still with a 100→120 master ramp over its span —
/// first frame == the untransformed fit, last frame == the 120% static
/// render, middle frame different from both.
#[test]
fn ken_burns_still_renders() {
    crate::initialize_ffmpeg().expect("FFmpeg should initialize");
    // Portrait still in a landscape frame, so the R10 bake is non-trivial:
    // 32×36 in 64×36 fits 2:1 → (50, 100, 10000).
    let media = mo1_quadrant_still("mo1-ken-burns", 32, 36);
    let (baked_x, baked_y, baked_fine) = kinewright_core::scale_to_frame_fit((32, 36), (64, 36));
    assert_eq!((baked_x, baked_y, baked_fine), (50, 100, 10_000));

    let ramp = {
        let mut effect = transform_effect(
            1,
            &[
                ("scale_x_percent", baked_x),
                ("scale_y_percent", baked_y),
                ("scale_fine_hundredths", baked_fine),
            ],
        );
        effect.keyframes.insert(
            "scale_percent".to_owned(),
            AutomationCurve {
                keyframes: vec![
                    Keyframe {
                        at: TimeCode(0),
                        value: 100,
                        interpolation: KeyframeInterpolation::Linear,
                        tangent_in: 0,
                        tangent_out: 0,
                    },
                    Keyframe {
                        at: TimeCode(149),
                        value: 120,
                        interpolation: KeyframeInterpolation::Linear,
                        tangent_in: 0,
                        tangent_out: 0,
                    },
                ],
            },
        );
        effect
    };
    let (document, _) = mo1_still_document(&media, 150, (64, 36), vec![ramp]);
    let (fit, _) = mo1_still_document(
        &media,
        150,
        (64, 36),
        vec![transform_effect(
            1,
            &[
                ("scale_x_percent", baked_x),
                ("scale_y_percent", baked_y),
                ("scale_fine_hundredths", baked_fine),
            ],
        )],
    );
    let (pushed, _) = mo1_still_document(
        &media,
        150,
        (64, 36),
        vec![transform_effect(
            1,
            &[
                ("scale_percent", 120),
                ("scale_x_percent", baked_x),
                ("scale_y_percent", baked_y),
                ("scale_fine_hundredths", baked_fine),
            ],
        )],
    );

    let first = render_frame(&document, TimeCode::ZERO)
        .rgba
        .as_slice()
        .to_vec();
    let last = render_frame(&document, TimeCode(149))
        .rgba
        .as_slice()
        .to_vec();
    let middle = render_frame(&document, TimeCode(75))
        .rgba
        .as_slice()
        .to_vec();
    assert_eq!(
        first.as_slice(),
        render_frame(&fit, TimeCode::ZERO).rgba.as_slice(),
        "the ramp's first frame must equal the untransformed fit"
    );
    assert_eq!(
        last.as_slice(),
        render_frame(&pushed, TimeCode::ZERO).rgba.as_slice(),
        "the ramp's last frame must equal the static 120% render"
    );
    assert_ne!(middle, first, "the middle frame must move off the fit");
    assert_ne!(middle, last, "the middle frame must not reach the push");
}

/// MO1 R7: an untagged still renders nothing until the source-colour
/// incident resolves — the managed decode refuses with the existing
/// `SourceColor` incident, and the assumed stamp renders.
#[test]
fn untagged_still_reaches_the_source_colour_incident() {
    crate::initialize_ffmpeg().expect("FFmpeg should initialize");
    let media = mo1_quadrant_still("mo1-untagged", 64, 36);
    let (mut document, _) = mo1_still_document(&media, 5, (64, 36), Vec::new());
    // G3: no hand stamp — the incident path runs on the real probed
    // description (decoder-known range/matrix kept).
    let probed = probe_path(media.path(), AssetId(1))
        .expect("the untagged still should probe")
        .color_description;
    assert_eq!(probed.primaries, ColorPrimaries::Unknown);
    assert_eq!(probed.transfer, ColorTransfer::Unknown);
    assert_eq!(probed.matrix, ColorMatrix::Rgb);
    assert_eq!(probed.range, ColorRange::Full);
    document.media_pool[0].color_description = probed.clone();
    document
        .validate()
        .expect("the incident document should validate");

    let gpu = fixture_gpu_or_skip().expect("the gate needs an adapter");
    let mut renderer = FrameRenderer::new(gpu);
    let error = renderer
        .render(
            &document,
            TimeCode::ZERO,
            document.resolution,
            RenderScale::FullResolution,
            DecodeStrategy::Seek,
        )
        .expect_err("an untagged still must refuse managed decode");
    let kinewright_core::MediaError::SourceColorForAsset(refusal) = error else {
        panic!("the refusal must be the existing source-colour incident, got {error:?}");
    };
    assert_eq!(
        refusal.asset,
        AssetId(1),
        "the incident must name the still"
    );
    assert_eq!(
        refusal.error,
        kinewright_core::ColorSourceError::UnknownPrimaries,
        "the incident must stay the unknown-source field report, got {:?}",
        refusal.error
    );
    assert!(
        matches!(refusal.description.primaries, ColorPrimaries::Unknown),
        "untagged PNG primaries must stay Unknown, got {:?}",
        refusal.description.primaries
    );

    // The policy refuses the Rec.709 recovery honestly (it would
    // overwrite the known Rgb matrix), while the real recovery
    // description — full range kept — still decodes.
    assert!(
        !kinewright_core::rec709_compatible(&probed),
        "the recovery must refuse honestly, not overwrite known fields"
    );
    let recovered = kinewright_core::recovery_description(&probed);
    assert_eq!(recovered.range, ColorRange::Full);
    document.media_pool[0].color_description = recovered;
    let frame = renderer
        .render(
            &document,
            TimeCode::ZERO,
            document.resolution,
            RenderScale::FullResolution,
            DecodeStrategy::Seek,
        )
        .expect("the recovered still must render");
    let red = pixel(&frame, 16, 9);
    assert!(
        red[0] > 150,
        "the recovered still must really render, got {red:?}"
    );
}

/// MO1 G3: the real incident recovery on an untagged JPEG keeps full
/// range — shadow steps survive instead of crushing to black (a
/// limited-range recovery would clip bars 8/16 to 0 and bar 250 to 255).
#[test]
fn untagged_jpeg_recovery_preserves_shadow_steps() {
    crate::initialize_ffmpeg().expect("FFmpeg should initialize");
    let media = GeneratedMedia::ffmpeg(
        "mo1-g3-jpegbars",
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=black:size=64x36:rate=1:duration=1,drawbox=x=13:y=0:w=13:h=36:c=0x080808:t=fill,drawbox=x=26:y=0:w=13:h=36:c=0x101010:t=fill,drawbox=x=39:y=0:w=13:h=36:c=0xFAFAFA:t=fill,drawbox=x=52:y=0:w=12:h=36:c=white:t=fill",
            "-frames:v",
            "1",
            "-c:v",
            "mjpeg",
            "-q:v",
            "1",
        ],
        "jpg",
    );
    let probed = probe_path(media.path(), AssetId(1))
        .expect("the untagged JPEG should probe")
        .color_description;
    assert_eq!(probed.primaries, ColorPrimaries::Unknown);
    assert_eq!(probed.transfer, ColorTransfer::Unknown);
    assert_eq!(probed.matrix, ColorMatrix::Other("bt470bg".to_owned()));
    assert_eq!(probed.range, ColorRange::Full);
    assert!(
        !kinewright_core::rec709_compatible(&probed),
        "the recovery must refuse honestly, not overwrite known fields"
    );
    let recovered = kinewright_core::recovery_description(&probed);
    assert_eq!(recovered.range, ColorRange::Full);

    let (mut document, _) = mo1_still_document(&media, 5, (64, 36), Vec::new());
    document.media_pool[0].color_description = recovered;
    document
        .validate()
        .expect("the recovered document should validate");
    let gpu = fixture_gpu_or_skip().expect("the gate needs an adapter");
    let mut renderer = FrameRenderer::new(gpu);
    let frame = renderer
        .render(
            &document,
            TimeCode::ZERO,
            document.resolution,
            RenderScale::FullResolution,
            DecodeStrategy::Seek,
        )
        .expect("the recovered JPEG must render");
    // Region means over bar centres (JPEG noise averages out).
    let bar = |x0: u32| {
        let mut sum = 0u32;
        let mut count = 0u32;
        for y in 12..24 {
            for x in x0 + 4..x0 + 9 {
                sum += u32::from(pixel(&frame, x, y)[0]);
                count += 1;
            }
        }
        sum / count
    };
    let levels = [bar(0), bar(13), bar(26), bar(39), bar(52)];
    assert!(levels[0] <= 8, "black must stay black, got {levels:?}");
    assert!(
        (1..=12).contains(&levels[1]),
        "code 8 must survive, not crush to 0, got {levels:?}",
    );
    assert!(
        (3..=20).contains(&levels[2]),
        "code 16 must survive, not crush to 0, got {levels:?}",
    );
    assert!(
        (240..=253).contains(&levels[3]),
        "code 250 must survive, not clip to 255, got {levels:?}",
    );
    assert!(levels[4] >= 250, "white must stay white, got {levels:?}");
    assert!(
        levels.windows(2).all(|pair| pair[0] <= pair[1]),
        "bar levels must not invert, got {levels:?}"
    );
}

/// MO1 R7: PNG alpha survives the hold path into alpha-over — a half-red
/// still over grey video composites the honest blend.
#[test]
fn still_alpha_composites_over_video() {
    crate::initialize_ffmpeg().expect("FFmpeg should initialize");
    let top = GeneratedMedia::ffmpeg(
        "mo1-alpha-top",
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=red@0.5:size=64x36:rate=1:duration=1,format=rgba",
            "-frames:v",
            "1",
            "-c:v",
            "png",
        ],
        "png",
    );
    let bottom = mo1_source("mo1-alpha-bottom");
    let (mut document, _) = mo1_still_document(&top, 10, (64, 36), Vec::new());
    let mut under = probe_path(bottom.path(), AssetId(2)).expect("video should probe");
    under.color_description = document.media_pool[0].color_description.clone();
    // Video stamp: the still's full-range RGB stamp is wrong for YUV —
    // restamp limited-range BT.709 like `mo1_document`.
    under.color_description.matrix = ColorMatrix::Bt709;
    under.color_description.range = ColorRange::Limited;
    under.color_description.transfer = ColorTransfer::Bt709;
    document.fps = under.fps;
    document.media_pool.push(under);
    document.tracks.insert(
        0,
        Track {
            id: TrackId(2),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![Clip {
                enabled: true,
                enabled_curve: None,
                id: ClipId(2),
                asset: AssetId(2),
                source_range: TimeCode::ZERO..TimeCode(10),
                content: ClipContent::Media,
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
                audio_gain_curve: None,
                blend_mode: kinewright_core::BlendMode::Normal,
            }],
        },
    );
    // Track order note: `visual_layers_at` composites ascending document
    // index, so the still (index 1) alpha-overs the video (index 0).
    document
        .validate()
        .expect("the alpha document should validate");

    let frame = render_frame(&document, TimeCode(5));
    let grey = render_frame(
        &{
            let mut solo = document.clone();
            solo.tracks.remove(1);
            solo
        },
        TimeCode(5),
    );
    let (r, g, b) = {
        let p = pixel(&frame, 32, 18);
        (p[0], p[1], p[2])
    };
    let (gr, gg, gb) = {
        let p = pixel(&grey, 32, 18);
        (p[0], p[1], p[2])
    };
    // Half red over grey: red rises toward 255, green/blue fall toward the
    // grey they halve. The blend must sit strictly between the layers —
    // opaque red (255, 0, 0) fails the red ceiling and the green floor.
    assert!(r > gr, "red must rise over grey ({r} vs {gr})");
    assert!(g < gg, "green must fall toward the halve ({g} vs {gg})");
    assert!(b < gb, "blue must fall toward the halve ({b} vs {gb})");
    assert!(
        (150..240).contains(&r) && (40..120).contains(&g) && (40..120).contains(&b),
        "the blend must be a real mix, got [{r}, {g}, {b}] over [{gr}, {gg}, {gb}]"
    );
    // MO1 N4 G8: pin the LINEAR-light value — the coded bounds above admit
    // double premultiplication (red 161 lands inside 150..240). The top
    // green is 0, so the green channel solves for the coverage alpha;
    // red and blue must then match the honest straight-alpha blend in
    // linear light (readback is BT.709-coded; the blend runs linear).
    let linear = |coded: u8| crate::color_pipeline::decode_bt709(f32::from(coded) / 255.0);
    let (red, green, blue) = (linear(r), linear(g), linear(b));
    let (grey_red, grey_green, grey_blue) = (linear(gr), linear(gg), linear(gb));
    let alpha = 1.0 - green / grey_green;
    assert!(
        (0.47..=0.53).contains(&alpha),
        "the fixture alpha must read ~0.5, got {alpha}"
    );
    let honest = |top: f32, under: f32| alpha * top + (1.0 - alpha) * under;
    assert!(
        (red - honest(1.0, grey_red)).abs() < 0.03,
        "linear red {red} must match the honest blend {} (alpha {alpha})",
        honest(1.0, grey_red)
    );
    assert!(
        (blue - honest(0.0, grey_blue)).abs() < 0.03,
        "linear blue {blue} must match the honest blend {} (alpha {alpha})",
        honest(0.0, grey_blue)
    );
}

/// A quadrant-motion video source: `frames` frames at 25 fps. Flat grey
/// would hide transform motion, so the trim/push-in/export gates render
/// quadrants (any scale/position change permutes them observably).
fn mo1_quadrant_video(label: &str, frames: i64) -> GeneratedMedia {
    let seconds = frames / 25 + 1;
    GeneratedMedia::ffmpeg(
        label,
        &[
            "-f",
            "lavfi",
            "-i",
            &format!(
                "color=c=red:size=64x36:rate=25:duration={seconds},drawbox=x=32:y=0:w=32:h=36:c=green:t=fill,drawbox=x=0:y=18:w=32:h=18:c=blue:t=fill,drawbox=x=32:y=18:w=32:h=18:c=white:t=fill"
            ),
            "-frames:v",
            &format!("{frames}"),
            "-c:v",
            "ffv1",
            "-pix_fmt",
            "yuv444p",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-colorspace",
            "bt709",
            "-color_range",
            "tv",
        ],
        "mkv",
    )
}

/// Five luma bars over flat chroma (CC1's proven bar codes): the R29
/// content. The established codec gate was calibrated on near-flat rasters
/// — sharp chroma edges ring past it through 4:2:0 — so the round trip
/// moves bars; a push-in still shifts every bar boundary observably.
fn mo1_bar_bytes(width: u32, height: u32) -> Vec<u8> {
    const BARS: [u8; 5] = [16, 64, 128, 192, 235];
    let pixels = usize::try_from(width * height).expect("bar raster");
    let mut bytes = Vec::with_capacity(pixels * 3);
    for _ in 0..height {
        for x in 0..width {
            bytes.push(BARS[usize::try_from((x * 5 / width).min(4)).expect("bar")]);
        }
    }
    bytes.extend(std::iter::repeat_n(128_u8, pixels));
    bytes.extend(std::iter::repeat_n(128_u8, pixels));
    bytes
}

/// A bar-motion video source: `frames` frames at 25 fps.
fn mo1_bar_video(label: &str, frames: i64) -> GeneratedMedia {
    // The raw demuxer takes no loop flag, so the reel holds every frame.
    let reel = mo1_bar_bytes(64, 36).repeat(usize::try_from(frames).expect("frames"));
    let raw = GeneratedMedia::from_bytes(&format!("{label}-bars"), "yuv", &reel);
    GeneratedMedia::ffmpeg(
        label,
        &[
            "-f",
            "rawvideo",
            "-pix_fmt",
            "yuv444p",
            "-s",
            "64x36",
            "-r",
            "25",
            "-i",
            raw.path().to_str().expect("fixture path"),
            "-frames:v",
            &format!("{frames}"),
            "-c:v",
            "ffv1",
            "-pix_fmt",
            "yuv444p",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-colorspace",
            "bt709",
            "-color_range",
            "tv",
        ],
        "mkv",
    )
}

/// A bar still (the Ken Burns half of the R29 timeline).
fn mo1_bar_still(label: &str, width: u32, height: u32) -> GeneratedMedia {
    let raw = GeneratedMedia::from_bytes(
        &format!("{label}-bars"),
        "yuv",
        &mo1_bar_bytes(width, height),
    );
    GeneratedMedia::ffmpeg(
        label,
        &[
            "-f",
            "rawvideo",
            "-pix_fmt",
            "yuv444p",
            "-s",
            &format!("{width}x{height}"),
            "-r",
            "1",
            "-i",
            raw.path().to_str().expect("fixture path"),
            "-frames:v",
            "1",
            "-c:v",
            "png",
        ],
        "png",
    )
}

/// Grey video + 440 Hz sine: the mix-null source (10 frames at 25 fps).
fn mo1_av_source(label: &str) -> GeneratedMedia {
    GeneratedMedia::ffmpeg(
        label,
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=gray:size=64x36:rate=25:duration=1",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=1",
            "-frames:v",
            "10",
            "-c:v",
            "ffv1",
            "-pix_fmt",
            "yuv444p",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-colorspace",
            "bt709",
            "-color_range",
            "tv",
            "-c:a",
            "pcm_s16le",
            "-shortest",
        ],
        "mkv",
    )
}

/// The limited-range BT.709 stamp a tagged video source carries into
/// managed decode (what `mo1_document` stamps, factored for hand-built
/// multi-asset documents).
fn limited_bt709_stamp() -> ColorDescription {
    ColorDescription {
        primaries: ColorPrimaries::Bt709,
        transfer: ColorTransfer::Bt709,
        matrix: ColorMatrix::Bt709,
        range: ColorRange::Limited,
        white_point: ColorWhitePoint::D65,
        bit_depth: ColorBitDepth::Eight,
        confidence_basis_points: 10_000,
        provenance: ColorProvenance::UserOverride,
    }
}

/// The assumed-sRGB stamp a resolved still carries (what
/// `mo1_still_document` stamps, factored for hand-built documents).
fn assumed_still_stamp() -> ColorDescription {
    ColorDescription {
        primaries: ColorPrimaries::Bt709,
        transfer: ColorTransfer::Srgb,
        matrix: ColorMatrix::Rgb,
        range: ColorRange::Full,
        white_point: ColorWhitePoint::D65,
        bit_depth: ColorBitDepth::Eight,
        confidence_basis_points: 10_000,
        provenance: ColorProvenance::UserOverride,
    }
}

/// Render every project frame in `frames` through one renderer — strips
/// share the decoder and cache instead of re-initing the GPU per frame.
fn render_strip(document: &Document, frames: std::ops::Range<i64>) -> Vec<Vec<u8>> {
    let gpu = fixture_gpu_or_skip().expect("the render gate needs an adapter");
    let mut renderer = FrameRenderer::new(gpu);
    frames
        .map(|at| {
            renderer
                .render(
                    document,
                    TimeCode(at),
                    document.resolution,
                    RenderScale::FullResolution,
                    DecodeStrategy::Seek,
                )
                .expect("strip frames should render")
                .rgba
                .as_ref()
                .clone()
        })
        .collect()
}

/// The delivery proof strip: `render_delivery` frames as RGBA8, the R29
/// reference the export decode is compared against.
fn render_delivery_strip(document: &Document, frames: std::ops::Range<i64>) -> Vec<Vec<u8>> {
    let gpu = fixture_gpu_or_skip().expect("the render gate needs an adapter");
    let mut renderer = FrameRenderer::new(gpu);
    frames
        .map(|at| {
            let delivery = renderer
                .render_delivery(
                    document,
                    TimeCode(at),
                    document.resolution,
                    RenderScale::FullResolution,
                    DecodeStrategy::Seek,
                )
                .expect("delivery proof frames should render");
            delivery_frame_to_rgba8(&delivery)
        })
        .collect()
}

/// Decode every frame of an export to RGBA (R29 compares the whole strip;
/// the CC1 helper pins one frame).
fn export_decode_rgba_all(path: &Path, width: u32, height: u32, frames: usize) -> Vec<Vec<u8>> {
    use std::process::Command as ProcessCommand;

    let output = ProcessCommand::new(crate::test_support::ffmpeg_executable())
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(path)
        .args(["-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"])
        .output()
        .expect("the export decode should start");
    assert!(
        output.status.success(),
        "the export decode failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stride = usize::try_from(width * height * 4).expect("frame bytes");
    assert_eq!(
        output.stdout.len(),
        stride * frames,
        "the export must decode to {frames} frames"
    );
    output.stdout.chunks(stride).map(<[u8]>::to_vec).collect()
}

/// A Hold 1→0 step at frame 5: the R28 enable cut shape.
fn enable_cut_curve() -> AutomationCurve {
    hold_step_curve(&[(0, 1), (5, 0)])
}

/// §10 gate 5, lavapipe half: a disabled 50% transform renders
/// byte-identically to the effect removed (R28 removal identity).
#[test]
fn disabled_effect_matches_removal_on_lavapipe() {
    crate::initialize_ffmpeg().expect("FFmpeg should initialize");
    let media = mo1_quadrant_video("mo1-r28-effect", 10);
    let document = mo1_document(&media, vec![transform_effect(1, &[("scale_percent", 50)])]);

    let mut disabled = document.clone();
    disabled.tracks[0].clips[0].effects[0].enabled = false;
    disabled
        .validate()
        .expect("the disabled document should validate");
    let mut removed = document.clone();
    removed.tracks[0].clips[0].effects.clear();
    removed
        .validate()
        .expect("the removed document should validate");

    assert_eq!(
        render_strip(&disabled, 0..10),
        render_strip(&removed, 0..10),
        "a disabled effect must render as the effect removed"
    );
    assert_ne!(
        render_strip(&document, 0..1),
        render_strip(&disabled, 0..1),
        "50% must be visible or the identity is vacuous"
    );
}

/// R28 lavapipe: a Hold-enabled 50% transform cuts at the key's first
/// frame — frames 0..5 equal the static scaled render, 5..10 the plain.
#[test]
fn keyframed_enable_cuts_at_key_frame_on_lavapipe() {
    crate::initialize_ffmpeg().expect("FFmpeg should initialize");
    let media = mo1_quadrant_video("mo1-r28-keycut", 10);
    let mut effect = transform_effect(1, &[("scale_percent", 50)]);
    effect.enabled_curve = Some(enable_cut_curve());
    let document = mo1_document(&media, vec![effect]);
    let scaled = mo1_document(&media, vec![transform_effect(1, &[("scale_percent", 50)])]);
    let plain = mo1_document(&media, Vec::new());

    let cut = render_strip(&document, 0..10);
    let on = render_strip(&scaled, 0..10);
    let off = render_strip(&plain, 0..10);
    assert_ne!(on[0], off[0], "50% must be visible or the cut is vacuous");
    for at in 0..5 {
        assert_eq!(cut[at], on[at], "frame {at} must carry the effect");
    }
    for at in 5..10 {
        assert_eq!(cut[at], off[at], "frame {at} must drop the effect");
    }
}

/// §10 gate 6, lavapipe half: disabling the covering still renders
/// byte-identically to removing it (R28).
#[test]
fn disabled_clip_matches_removal_on_lavapipe() {
    crate::initialize_ffmpeg().expect("FFmpeg should initialize");
    let video = mo1_quadrant_video("mo1-r28-under", 10);
    let still = mo1_quadrant_still("mo1-r28-over", 64, 36);
    let (baked_x, baked_y, baked_fine) = kinewright_core::scale_to_frame_fit((64, 36), (64, 36));
    assert_eq!((baked_x, baked_y, baked_fine), (100, 100, 10_000));
    let (mut document, _) = mo1_still_document(
        &still,
        10,
        (64, 36),
        vec![transform_effect(
            1,
            &[
                ("scale_x_percent", baked_x),
                ("scale_y_percent", baked_y),
                ("scale_fine_hundredths", baked_fine),
            ],
        )],
    );
    let mut under = probe_path(video.path(), AssetId(2)).expect("video should probe");
    under.color_description = limited_bt709_stamp();
    document.fps = under.fps;
    document.media_pool.push(under);
    document.tracks.insert(
        0,
        Track {
            id: TrackId(2),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![Clip {
                enabled: true,
                enabled_curve: None,
                id: ClipId(2),
                asset: AssetId(2),
                source_range: TimeCode::ZERO..TimeCode(10),
                content: ClipContent::Media,
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
                audio_gain_curve: None,
                blend_mode: kinewright_core::BlendMode::Normal,
            }],
        },
    );
    document
        .validate()
        .expect("the layered document should validate");

    let mut disabled = document.clone();
    disabled.tracks[1].clips[0].enabled = false;
    disabled
        .validate()
        .expect("the disabled document should validate");
    let mut removed = document.clone();
    removed.tracks.remove(1);
    removed
        .validate()
        .expect("the removed document should validate");

    assert_eq!(
        render_strip(&disabled, 0..10),
        render_strip(&removed, 0..10),
        "a disabled clip must render as the clip removed"
    );
    assert_ne!(
        render_strip(&document, 0..1),
        render_strip(&disabled, 0..1),
        "the covering still must be visible or the identity is vacuous"
    );
}

/// §10 gate 6, mix-null half: a disabled clip is silent in the mix —
/// sample-identical to the clip removed (R28).
#[test]
fn disabled_clip_is_silent_in_the_mix() {
    crate::initialize_ffmpeg().expect("FFmpeg should initialize");
    let media = mo1_av_source("mo1-r28-mix");
    let mut asset = probe_path(media.path(), AssetId(1)).expect("the AV source should probe");
    assert_eq!(asset.kind, MediaKind::AudioVideo);
    assert_eq!(asset.duration, TimeCode(10));
    asset.color_description = limited_bt709_stamp();
    let clip = |id: u64, start: i64| Clip {
        enabled: true,
        enabled_curve: None,
        id: ClipId(id),
        asset: asset.id,
        source_range: TimeCode(start)..TimeCode(start + 5),
        content: ClipContent::Media,
        timeline_start: TimeCode(start),
        effects: Vec::new(),
        transition_in: None,
        link: None,
        audio_gain_tenth_db: 0,
        audio_fade_in_frames: TimeCode::ZERO,
        audio_fade_out_frames: TimeCode::ZERO,
        speed_percent: 100,
        audio_gain_curve: None,
        blend_mode: kinewright_core::BlendMode::Normal,
    };
    let document = Document {
        investigator: None,
        catalog: MediaCatalog::default(),
        audio_mix: AudioMix::default(),
        color_context: ColorContext::default(),
        lut_assets: Vec::new(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![clip(1, 0), clip(2, 5)],
        }],
        media_pool: vec![asset],
        markers: Vec::new(),
        fps: Rational::new(25, 1).unwrap(),
        resolution: (64, 36),
        duration: TimeCode(10),
    };
    document
        .validate()
        .expect("the mix document should validate");
    let settings = ExportSettings {
        fps: document.fps,
        resolution: document.resolution,
        delivery_color: ColorContext::sdr_rec709().delivery,
        video_codec: "libx264".to_owned(),
        audio_codec: "aac".to_owned(),
        video_bitrate: 1,
        audio_bitrate: 1,
        loudness_normalization: None,
        cancellation: ExportCancellation::default(),
    };

    let mut disabled = document.clone();
    disabled.tracks[0].clips[0].enabled = false;
    disabled
        .validate()
        .expect("the disabled document should validate");
    let mut removed = document.clone();
    removed.tracks[0].clips.remove(0);
    removed
        .validate()
        .expect("the removed document should validate");

    let off = mix_audio(&disabled, &settings).expect("the disabled mix should run");
    let gone = mix_audio(&removed, &settings).expect("the removed mix should run");
    assert_eq!(off, gone, "a disabled clip must mix as the clip removed");
    let on = mix_audio(&document, &settings).expect("the enabled mix should run");
    assert_ne!(
        on, off,
        "the sine must be audible or the silence is vacuous"
    );
}

/// A linear 100→120 master ramp over local frames 0..=4.
fn scale_ramp_curve() -> AutomationCurve {
    AutomationCurve {
        keyframes: vec![
            Keyframe {
                at: TimeCode(0),
                value: 100,
                interpolation: KeyframeInterpolation::Linear,
                tangent_in: 0,
                tangent_out: 0,
            },
            Keyframe {
                at: TimeCode(4),
                value: 120,
                interpolation: KeyframeInterpolation::Linear,
                tangent_in: 0,
                tangent_out: 0,
            },
        ],
    }
}

/// A Hold step through `(at, value)` keys.
fn hold_step_curve(keys: &[(i64, i64)]) -> AutomationCurve {
    AutomationCurve {
        keyframes: keys
            .iter()
            .map(|(at, value)| Keyframe {
                at: TimeCode(*at),
                value: *value,
                interpolation: KeyframeInterpolation::Hold,
                tangent_in: 0,
                tangent_out: 0,
            })
            .collect(),
    }
}

/// The R29 timeline: a push-in over bars (with an enable cut at frame
/// 3) followed by a Ken Burns bar still.
fn mo1_export_document(video: &GeneratedMedia, still: &GeneratedMedia) -> Document {
    let mut video_asset =
        probe_path(video.path(), AssetId(1)).expect("the video source should probe");
    video_asset.color_description = limited_bt709_stamp();
    let mut still_asset =
        probe_path(still.path(), AssetId(2)).expect("the still source should probe");
    assert_eq!(still_asset.kind, MediaKind::Image);
    still_asset.color_description = assumed_still_stamp();
    let fps = video_asset.fps;

    let mut push_in = transform_effect(1, &[("scale_percent", 100)]);
    push_in
        .keyframes
        .insert("scale_percent".to_owned(), scale_ramp_curve());
    push_in.enabled_curve = Some(hold_step_curve(&[(0, 1), (3, 0)]));
    let (baked_x, baked_y, baked_fine) = kinewright_core::scale_to_frame_fit((32, 36), (64, 36));
    let mut ken_burns = transform_effect(
        2,
        &[
            ("scale_x_percent", baked_x),
            ("scale_y_percent", baked_y),
            ("scale_fine_hundredths", baked_fine),
        ],
    );
    ken_burns
        .keyframes
        .insert("scale_percent".to_owned(), scale_ramp_curve());
    Document {
        investigator: None,
        catalog: MediaCatalog::default(),
        audio_mix: AudioMix::default(),
        color_context: ColorContext::default(),
        lut_assets: Vec::new(),
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![
                Clip {
                    enabled: true,
                    enabled_curve: None,
                    id: ClipId(1),
                    asset: video_asset.id,
                    source_range: TimeCode::ZERO..TimeCode(5),
                    content: ClipContent::Media,
                    timeline_start: TimeCode::ZERO,
                    effects: vec![push_in],
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                    audio_gain_curve: None,
                    blend_mode: kinewright_core::BlendMode::Normal,
                },
                Clip {
                    enabled: true,
                    enabled_curve: None,
                    id: ClipId(2),
                    asset: still_asset.id,
                    source_range: TimeCode::ZERO..TimeCode(5),
                    content: ClipContent::Freeze(FreezeFrame {
                        source_frame: TimeCode::ZERO,
                    }),
                    timeline_start: TimeCode(5),
                    effects: vec![ken_burns],
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                    audio_gain_curve: None,
                    blend_mode: kinewright_core::BlendMode::Normal,
                },
            ],
        }],
        media_pool: vec![video_asset, still_asset],
        markers: Vec::new(),
        fps,
        resolution: (64, 36),
        duration: TimeCode(10),
    }
}

/// R29: one motion-heavy timeline (push-in + Ken Burns + an enable cut)
/// exports through delivery and decodes back within the established H.264
/// codec gate, frame by frame — proving the shared path carries motion.
#[test]
fn export_round_trip_carries_motion() {
    crate::initialize_ffmpeg().expect("FFmpeg should initialize");
    let video = mo1_bar_video("mo1-r29-video", 10);
    let still = mo1_bar_still("mo1-r29-still", 32, 36);
    let document = mo1_export_document(&video, &still);
    document
        .validate()
        .expect("the export document should validate");

    // The proof strip carries the motion: the ramp moves, the cut lands,
    // the Ken Burns moves.
    let reference = render_delivery_strip(&document, 0..10);
    assert_ne!(reference[0], reference[2], "the push-in must move");
    assert_eq!(reference[3], reference[4], "the cut must land on plain");
    assert_ne!(reference[5], reference[9], "the Ken Burns must move");

    let directory = TempDirectory::new("mo1-r29-export");
    let output = directory.path("mo1-round-trip.mp4");
    let settings = ExportSettings {
        fps: document.fps,
        resolution: document.resolution,
        delivery_color: ColorContext::sdr_rec709().delivery,
        video_codec: "libx264".to_owned(),
        audio_codec: "aac".to_owned(),
        video_bitrate: 20_000_000,
        audio_bitrate: 192_000,
        loudness_normalization: None,
        cancellation: ExportCancellation::default(),
    };
    let (progress_tx, _progress_rx) = crossbeam_channel::unbounded();
    let gpu = fixture_gpu_or_skip().expect("the export gate needs an adapter");
    crate::export::export_document(&document, &output, &settings, &progress_tx, gpu)
        .expect("the motion export should write");

    let decoded = export_decode_rgba_all(&output, 64, 36, 10);
    assert_ne!(decoded[0], decoded[2], "the codec must carry the push-in");
    assert_ne!(decoded[5], decoded[9], "the codec must carry the Ken Burns");
    for (at, (actual, expected)) in decoded.iter().zip(reference.iter()).enumerate() {
        let metric = abs_code_diff_rgb(actual, expected);
        assert!(
            metric.max <= DELIVERY_CODEC_MAX,
            "frame {at} delivery max metric: {metric:?}"
        );
        assert!(
            metric.p99 <= DELIVERY_CODEC_P99,
            "frame {at} delivery P99 metric: {metric:?}"
        );
        assert!(
            metric.mean <= DELIVERY_CODEC_MEAN,
            "frame {at} delivery mean metric: {metric:?}"
        );
    }
}

/// MO2 R16 (discharges MO1 §11): every MO1 R26 golden's input — raster,
/// output size and transform — re-gated against the MO2 CPU twin within
/// the R27 working tolerances, so the goldens' probes are no longer the
/// only witness of the vertex/sampling geometry.
#[test]
#[allow(clippy::too_many_lines)]
fn mo1_transform_r26_matches_twin() {
    let Some(compositor) = fallback() else {
        return;
    };
    let t = |parameters: &[(&str, i64)]| vec![transform_effect(1, parameters)];
    let quarter = [
        ("anchor_x_basis_points", 2_500),
        ("anchor_y_basis_points", 2_500),
    ];
    let cases = [
        ("default", quadrants(64, 36), (64, 36), t(&[])),
        (
            "gradient",
            mo1_gradient(97, 53),
            (192, 108),
            t(&[("scale_percent", 100)]),
        ),
        (
            "scale",
            quadrants(64, 36),
            (64, 36),
            t(&[("scale_percent", 50)]),
        ),
        (
            "per-axis",
            quadrants(64, 36),
            (64, 36),
            t(&[("scale_x_percent", 50)]),
        ),
        (
            "rotation",
            quadrants(64, 36),
            (64, 36),
            t(&[("rotation_centidegrees", 9_000)]),
        ),
        (
            "aspect",
            l_raster(64, 36),
            (64, 36),
            t(&[("scale_percent", 50), ("rotation_centidegrees", 9_000)]),
        ),
        (
            "scale-then-rotate",
            centred_square(64, 40),
            (64, 64),
            t(&[("scale_x_percent", 50), ("rotation_centidegrees", 9_000)]),
        ),
        (
            "anchor",
            quadrants(64, 36),
            (64, 36),
            t(&[
                ("scale_percent", 50),
                ("anchor_x_basis_points", 0),
                ("anchor_y_basis_points", 0),
            ]),
        ),
        (
            "coarse+fine",
            quadrants(64, 36),
            (64, 36),
            t(&[("x_percent", 25), ("x_basis_points", 2_500)]),
        ),
        (
            "fine nudge",
            vertical_edge(1920, 1080),
            (1920, 1080),
            t(&[("x_basis_points", 5)]),
        ),
        (
            "L order",
            l_raster(64, 36),
            (64, 36),
            t(&[
                ("scale_percent", 50),
                ("rotation_centidegrees", 9_000),
                ("x_percent", -50),
            ]),
        ),
        (
            "L about anchor",
            quadrants(64, 36),
            (64, 36),
            t(&[
                ("scale_percent", 50),
                ("rotation_centidegrees", 9_000),
                quarter[0],
                quarter[1],
            ]),
        ),
        (
            "off-axis",
            mo1_gradient(97, 53),
            (64, 36),
            t(&[
                ("scale_percent", 73),
                ("rotation_centidegrees", 1_234),
                ("y_percent", 7),
            ]),
        ),
    ];
    for (label, source, resolution, effects) in cases {
        // The production input type: display-coded rasters enter as linear
        // f16 working frames (renders never composite 8-bit textures).
        let source = crate::frame::WorkingFrame::from_display_frame(&source).unwrap();
        let layers = [CompositorLayer {
            frame: &source,
            effects: &effects,
            transition: TransitionRenderParams::default(),
            mode: LayerMode::NORMAL,
        }];
        let gpu = compositor.render_working(resolution, &layers).unwrap();
        let twin = crate::compositor::twin::render_working(resolution, &layers, None).unwrap();
        // MO2 ME9: resampled values also get the 8-bit sub-texel envelope
        // (zero where nothing is filtered), computed only on a miss.
        let pairs = || gpu.pixels.iter().zip(&twin.pixels);
        let slack = if pairs().all(|(a, e)| (a - e).abs() <= 1e-3) {
            vec![0.0; twin.pixels.len()]
        } else {
            crate::compositor::twin::subtexel_envelope(resolution, &layers, None).unwrap()
        };
        for (index, (a, e)) in pairs().enumerate() {
            assert!(
                (a - e).abs() <= 1e-3 + slack[index],
                "{label}: value {index} (pixel {}) GPU {a} vs twin {e}, slack {}",
                index / 4,
                slack[index]
            );
        }
    }
}

/// MO2 ME9 (G4): the value Windows WARP produced on the R26 gradient
/// (run 36222189672) lies outside the unit 1e-3 but inside the derived
/// 8-bit sub-texel envelope, so the widening is what that adapter needs.
#[test]
fn mo2_warp_r26_departure_lies_within_the_subtexel_envelope() {
    let source = crate::frame::WorkingFrame::from_display_frame(&mo1_gradient(97, 53)).unwrap();
    let effects = vec![transform_effect(1, &[("scale_percent", 100)])];
    let layers = [CompositorLayer {
        frame: &source,
        effects: &effects,
        transition: TransitionRenderParams::default(),
        mode: LayerMode::NORMAL,
    }];
    let twin = crate::compositor::twin::render_working((192, 108), &layers, None).unwrap();
    let slack = crate::compositor::twin::subtexel_envelope((192, 108), &layers, None).unwrap();
    let (warp, exact) = (0.848_144_53_f32, twin.pixels[290]);
    println!("value 290: twin {exact} slack {} WARP {warp}", slack[290]);
    assert!(
        (warp - exact).abs() > 1e-3,
        "WARP misses the unit tolerance"
    );
    assert!(
        (warp - exact).abs() <= 1e-3 + slack[290],
        "and lies in the envelope"
    );
    let unfiltered = slack.iter().filter(|v| **v == 0.0).count();
    println!("{unfiltered} of {} values carry no slack", slack.len());
    // A pixel-exact blit filters nothing: no value gets any slack.
    let source = crate::frame::WorkingFrame::from_display_frame(&quadrants(64, 36)).unwrap();
    let layers = [CompositorLayer {
        frame: &source,
        ..layers[0]
    }];
    let blit = crate::compositor::twin::subtexel_envelope((64, 36), &layers, None).unwrap();
    assert!(
        blit.iter().all(|v| *v == 0.0),
        "unfiltered pixels keep 1e-3"
    );
}
