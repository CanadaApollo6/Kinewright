//! MO1 Part B render gates: transform goldens (R26), still hold (R27), enable
//! identity (R28), and the §10 B-gates that render.
//!
//! Two lanes: compositor-level goldens over hand-built rasters (cheap,
//! pixel-exact geometry pins through the same WGSL and `params_for` the
//! production path uses) and render-level gates through `FrameRenderer` +
//! decode (proving the shared path carries motion/stills/enable). R26 names
//! no CPU-reference twin — MO2 builds it (R26 re-gated there).

use std::collections::BTreeMap;
use std::sync::Arc;

use kinewright_core::{
    AssetId, AudioMix, AutomationCurve, Clip, ClipContent, ClipId, ColorBitDepth, ColorContext,
    ColorDescription, ColorMatrix, ColorPrimaries, ColorProvenance, ColorRange, ColorTransfer,
    ColorWhitePoint, Document, Effect, EffectId, FrameTexture, FreezeFrame, Keyframe,
    KeyframeInterpolation, MediaAsset, MediaCatalog, MediaKind, Operation, ParamValue, Rational,
    TimeCode, Track, TrackId, TrackKind,
};

use crate::compositor::{Compositor, CompositorLayer};
use crate::decode::probe_path;
use crate::gpu_test_support::fixture_gpu_or_skip;
use crate::render::{DecodeStrategy, FrameRenderer, RenderScale};
use crate::test_support::{GeneratedMedia, single_clip_document};
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

/// MO1 R26: a transform-less render is byte-identical to pre-MO1. The hash
/// was recorded on the pre-Part-B tree (see the report) and is pinned here;
/// the rotation-0 path computes exactly the pre-MO1 expression.
#[test]
fn mo1_identity_transformless_render_matches_pre_mo1_bytes() {
    crate::initialize_ffmpeg().expect("FFmpeg should initialize");
    let media = mo1_source("mo1-identity");
    let document = mo1_document(&media, Vec::new());
    assert_eq!(document.resolution, (64, 36));
    let frame = render_frame(&document, TimeCode::ZERO);
    let hash = crate::sha256::sha256_bytes(&frame.rgba);
    assert_eq!(
        hash, "c0e80450da170963a5fb004d1f4ff0c4bf5ab76f1ad25117efb2a525a176e3e0",
        "transform-less bytes must equal the pre-MO1 recording"
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

    // The combination hash: any wrong order, sign, or shear flips bytes.
    let hash = crate::sha256::sha256_bytes(&frame.rgba);
    assert_eq!(
        hash, "b06517cc94ac7f150b8ac7b8ed39f7c1e8a542b969337fc476394c07c2d9054a",
        "the L order golden must stay byte-stable"
    );
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
    let hash = crate::sha256::sha256_bytes(&frame.rgba);
    assert_eq!(
        hash, "16c50a6714c7641ad1e93c4a314d588c87dafe826e1456f718776eee7497a7e0",
        "the render-path L golden must stay byte-stable"
    );
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
/// evictions) rather than churning it.
#[test]
fn still_hold_parity_across_five_seconds() {
    crate::initialize_ffmpeg().expect("FFmpeg should initialize");
    let media = mo1_quadrant_still("mo1-hold", 64, 36);
    let (document, _) = mo1_still_document(&media, 150, (64, 36), Vec::new());
    let gpu = fixture_gpu_or_skip().expect("the gate needs an adapter");
    let mut renderer = FrameRenderer::new(gpu);
    let render = |renderer: &mut FrameRenderer, at: i64| {
        renderer
            .render(
                &document,
                TimeCode(at),
                document.resolution,
                RenderScale::FullResolution,
                DecodeStrategy::Seek,
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
            "hold frame {at} must equal frame 0"
        );
    }
    assert_eq!(
        renderer.video_seek_count(),
        1,
        "the still must decode exactly once across its span"
    );
    assert_eq!(
        renderer.cache_eviction_count(),
        0,
        "the pinned still frame must never evict"
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
    document.media_pool[0].color_description = ColorDescription::unknown();

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

    // The assumed stamp (what `mo1_still_document` carries) renders.
    let (document, _) = mo1_still_document(&media, 5, (64, 36), Vec::new());
    let mut renderer = FrameRenderer::new(fixture_gpu_or_skip().expect("adapter"));
    renderer
        .render(
            &document,
            TimeCode::ZERO,
            document.resolution,
            RenderScale::FullResolution,
            DecodeStrategy::Seek,
        )
        .expect("the assumed still must render");
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
}
