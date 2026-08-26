//! Objective CC6 evidence fixtures for `docs/CC6-QC-AND-MANAGED-DELIVERY.md` §11.
//!
//! These fixtures live inside the media crate for the same reason the CC1,
//! CC3, CC4, and CC5 fixtures do: the `Rgba16Float` working surface, the
//! 16-bit delivery intermediate, the production export filter graph, and the
//! bindings decoder are internal seams, and the evidence has to exercise the
//! real path rather than a public re-implementation of it.
//!
//! What this file owns is the §11.2 fixtures that no earlier step already
//! owns:
//!
//! * §11.1's two generators — [`cc6_qc_raster`], hoisted here so the working
//!   proof fixtures in `compositor.rs` and the manifest read the same
//!   definition, and [`cc6_delivery_source`], the 320 × 180 / 25 fps /
//!   60-frame tagged FFV1 source the encoded round trip runs on;
//! * §11.2.10 and §11.2.11, **the exit gate**: the production export at both
//!   delivery depths, re-probed, decoded, and compared against a per-frame
//!   full-resolution reference with the measured margin recorded;
//! * §11.2.13, the starved-bitrate failing direction of that same gate;
//! * the media half of §11.2.14 — core's document order is asserted equal to
//!   `visual_layers_at`'s production z-order, which core cannot do because it
//!   cannot depend on this crate;
//! * §11.2.22, the two delivery transfers asserted bit-identical;
//! * §11.2.24, the P9 performance evidence on both lanes;
//! * §11.2.23, the manifest and the declared-test inventory.
//!
//! Every other §11.2 fixture is owned by the file that owns the code it
//! measures — `crates/kinewright-core/tests/cc6_core.rs`, `compositor.rs`,
//! `verify.rs`, `export.rs`, the agent, and the app — and is *declared* here
//! rather than re-implemented, which is what [`CC6_MEDIA_TESTS`] and its
//! sibling inventories exist for.
//!
//! Per rule 11.0.1 no expected value in this file is obtained by calling
//! `measure_color_qc`, `bt709_limited_ycbcr`, `encode_bt709`,
//! `encode_bt709_delivery`, the compositor, or swscale. The one place a
//! production-adjacent transform appears is [`bt709_limited_source_codes`],
//! which *generates source content* with an independently transcribed §3.4
//! matrix — explicitly permitted by rule 11.0.1's transcription clause and
//! required by §11.1.
//!
//! Per rule 11.0.5 every budget in the encoded gate is asserted with its
//! **measured margin**, and the same source is exported at a starved bitrate
//! so the gate is known to be able to fail.

#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::excessive_precision)]
#![allow(clippy::float_cmp)]
#![allow(clippy::items_after_statements)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::uninlined_format_args)]

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use half::f16;
use kinewright_core::{
    Analysis, AssetId, ClipId, ColorBitDepth, ColorMatrix, ColorPrimaries, ColorProvenance,
    ColorQcCheck, ColorQcRequest, ColorRange, ColorTransfer, ColorWhitePoint,
    DECODED_RANGE_EXCEPTION_BASIS_POINTS, DELIVERY_RGB_EXTREMES_NOTE,
    DELIVERY_VERIFICATION_FRAME_COUNT, DELIVERY_VERIFICATION_MAX_FRAMES, DeliveryColorError,
    DeliveryEncodeDepth, DeliveryProfile, DeliveryVerification, DeliveryVerificationRequest,
    Document, Effect, EffectId, ExportCancellation, ExportSettings, NormalizedRoi, ParamValue,
    QaSeverity, TimeCode, YCbCrLegalSource, measure_color_qc,
};
use serde_json::{Value, json};

use crate::{
    cc1_fixtures::{
        FixtureGpu, MONITOR_CPU_GPU_MAX, MONITOR_CPU_GPU_MEAN, MONITOR_CPU_GPU_P99,
        backend_metadata, fallback_gpu, file_hash, git_revision, hardware_gpu, simple_document,
        write_evidence_artefact,
    },
    color_pipeline::{DELIVERY_INTERMEDIATE_WHITE, grade709_decode},
    compositor::GpuContext,
    decode::probe_path,
    frame::WorkingFrame,
    render::{DecodeStrategy, FrameRenderer, RenderScale},
    test_support::{TempDirectory, run_ffmpeg},
    verify::EBU_R103_TOLERANCE_CODES_8BIT,
};

/// The contract token recorded on every CC6 evidence payload and asserted
/// against the manifest.
pub(crate) const CC6_CONTRACT: &str = "cc6_qc_and_managed_delivery";

// ===========================================================================
// §11.1: the QC raster.
// ===========================================================================

/// The four CC5 skin triples in `grade709` encoding, **independently
/// transcribed** from CC5 §9.2.17 (rule 11.0.1's transcription clause: a
/// fixture that imports the value it checks proves only that one copy exists).
pub(crate) const CC6_SKIN_PATCHES: [[f32; 3]; 4] = [
    [0.85, 0.68, 0.60],
    [0.72, 0.53, 0.44],
    [0.55, 0.38, 0.30],
    [0.32, 0.20, 0.15],
];
/// CC5's `product_red` and `product_cyan`, which must fall outside the §3.5
/// skin band.
pub(crate) const CC6_PRODUCT_PATCHES: [[f32; 3]; 2] = [[0.80, 0.10, 0.12], [0.10, 0.65, 0.75]];
/// The neutral surround the patches sit in: `C = 0` exactly.
pub(crate) const CC6_CHART_SURROUND: [f32; 3] = [0.45, 0.45, 0.45];

/// The §11.1 QC raster dimensions: one pixel is exactly 125 basis points
/// horizontally and 250 vertically.
pub(crate) const CC6_QC_RASTER: (u32, u32) = (80, 40);

/// The §11.1 population table, in the order the raster builder counts them:
/// ramp, over block, under block, skin patches, product patches, below-black
/// pixel, isolated over pixel, surround.
pub(crate) const CC6_QC_RASTER_POPULATIONS: [u32; 8] = [1152, 288, 288, 384, 192, 1, 1, 894];

/// The §11.1 whole-raster basis-point table, hand-computed on the 3 200
/// denominator and asserted by [`cc6_qc_raster_populations_are_the_contract_table`].
///
/// `(over, under_red, under_green_blue, gamut, clamped)`.
pub(crate) const CC6_QC_RASTER_BASIS_POINTS: [u32; 5] = [903, 903, 3, 903, 1806];

/// The §11.1 sub-threshold ROI: `left = 0, top = 0, right = 6125, bottom =
/// 6000`, i.e. 49 × 24 = 1 176 pixels, in which every counter is 8 bp and
/// therefore below the 10 bp threshold.
pub(crate) const CC6_SUB_THRESHOLD_ROI: NormalizedRoi = NormalizedRoi::new(0, 0, 6_125, 6_000);

/// The §11.1 QC raster: 80 × 40 = 3 200 pixels with basis-point-exact
/// rectangles.
///
/// The populations are the contract's: ramp 1 152, over block 288, under block
/// 288, four skin patches 384, two product patches 192, one below-black pixel,
/// one isolated over pixel, and 894 surround —
/// `1152 + 288 + 288 + 384 + 192 + 1 + 1 + 894 = 3200`.
///
/// The layout is pinned by the contract's sub-threshold ROI
/// (`0, 0, 6125, 6000` = 49 × 24): the ramp is `x in 0..48, y in 0..24`, the
/// isolated over pixel is at `(48, 0)`, the below-black pixel at `(48, 1)`,
/// and the remaining 22 pixels of that column are surround.
///
/// It lives here rather than in `compositor.rs`'s test module because three
/// different files measure it — the two working-proof parity fixtures, the
/// full-resolution refusal, and the manifest — and a second copy of a raster
/// is a second definition of every population in §11.1.
pub(crate) fn cc6_qc_raster() -> WorkingFrame {
    let (width, height) = CC6_QC_RASTER;
    let skin = CC6_SKIN_PATCHES.map(|patch| patch.map(grade709_decode));
    let product = CC6_PRODUCT_PATCHES.map(|patch| patch.map(grade709_decode));
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    let mut populations = [0_u32; 8];
    for y in 0..height {
        for x in 0..width {
            let (rgb, population) = if x < 48 && y < 24 {
                // In-range ramp: linear 0..1 on both axes, every channel
                // inside [0, 1], so this is the population that must trip
                // nothing.
                let fx = x as f32 / 47.0;
                let fy = y as f32 / 23.0;
                ([fx, fy, f32::midpoint(fx, fy)], 0)
            } else if x == 48 && y == 0 {
                ([1.2, 1.2, 1.2], 6)
            } else if x == 48 && y == 1 {
                ([-0.02, -0.005, -0.005], 5)
            } else if x < 36 && (24..32).contains(&y) {
                ([1.05, 1.05, 1.05], 1)
            } else if x < 36 && y >= 32 {
                ([-0.01, 0.5, 0.5], 2)
            } else if (36..60).contains(&x) && y >= 24 {
                let patch = usize::from(x >= 48) + 2 * usize::from(y >= 32);
                (skin[patch], 3)
            } else if (60..72).contains(&x) && y >= 24 {
                (product[usize::from(y >= 32)], 4)
            } else {
                (CC6_CHART_SURROUND, 7)
            };
            populations[population] += 1;
            pixels.extend(rgb.map(f16::from_f32));
            pixels.push(f16::from_f32(1.0));
        }
    }
    // The populations are asserted here rather than in prose: a layout edit
    // that silently moved a region would otherwise change what every
    // measurement below is measuring.
    assert_eq!(
        populations, CC6_QC_RASTER_POPULATIONS,
        "the CC6 QC raster populations are the §11.1 table"
    );
    assert_eq!(populations.iter().sum::<u32>(), width * height);
    WorkingFrame {
        width,
        height,
        pixels: Arc::new(pixels),
    }
}

/// §11.1. The raster's populations and its basis-point table are consistent on
/// the 3 200 denominator, and every threshold-relevant figure is derived from
/// the counts rather than restated.
///
/// This is the arithmetic §11.1 states in prose — `288/3200 = 900 bp` trips the
/// 10 bp threshold and `1/3200 = 3 bp` cannot — written out so a change to the
/// layout cannot leave the manifest's table describing a raster that no longer
/// exists.
#[test]
fn cc6_qc_raster_populations_are_the_contract_table() {
    let raster = cc6_qc_raster();
    let pixels = u64::from(CC6_QC_RASTER.0 * CC6_QC_RASTER.1);
    assert_eq!(pixels, 3_200);
    assert_eq!(raster.pixels.len(), (pixels * 4) as usize);

    // One pixel is exactly 125 basis points horizontally and 250 vertically.
    assert_eq!(10_000 / u64::from(CC6_QC_RASTER.0), 125);
    assert_eq!(10_000 / u64::from(CC6_QC_RASTER.1), 250);

    let basis_points = |count: u64| u32::try_from(count * 10_000 / pixels).expect("basis points");
    let [
        ramp,
        over_block,
        under_block,
        skin,
        product,
        below_black,
        isolated_over,
        surround,
    ] = CC6_QC_RASTER_POPULATIONS.map(u64::from);
    assert_eq!(
        ramp + over_block + under_block + skin + product + below_black + isolated_over + surround,
        pixels
    );
    assert_eq!(skin, 4 * 96);
    assert_eq!(product, 2 * 96);

    // Over: the 288-pixel block plus the isolated 1.2 pixel.
    assert_eq!(over_block + isolated_over, 289);
    assert_eq!(basis_points(over_block + isolated_over), 903);
    // Under, red: the 288-pixel block plus the below-black pixel.
    assert_eq!(basis_points(under_block + below_black), 903);
    // Under, green and blue: the below-black pixel alone, in the same
    // measurement, below the threshold.
    assert_eq!(basis_points(below_black), 3);
    // Gamut is the under-range pixel set exactly (§3.3).
    assert_eq!(basis_points(under_block + below_black), 903);
    // Clamped is over ∪ under: 289 + 289.
    assert_eq!(
        basis_points(over_block + isolated_over + under_block + below_black),
        1_806
    );
    assert_eq!(
        [
            basis_points(over_block + isolated_over),
            basis_points(under_block + below_black),
            basis_points(below_black),
            basis_points(under_block + below_black),
            basis_points(over_block + isolated_over + under_block + below_black),
        ],
        CC6_QC_RASTER_BASIS_POINTS
    );
    // A single pixel can never trip the 10 bp threshold at whole-raster scope.
    assert_eq!(basis_points(1), 3);
    assert!(basis_points(1) < 10);
    // ... while a 288-pixel block always does.
    assert_eq!(basis_points(288), 900);
    assert!(basis_points(288) >= 10);

    // The sub-threshold ROI is 49 × 24 = 1 176 pixels: the whole ramp, the
    // isolated over pixel, the below-black pixel, and 22 surround pixels.
    assert_eq!(CC6_SUB_THRESHOLD_ROI.width_basis_points, 6_125);
    assert_eq!(CC6_SUB_THRESHOLD_ROI.height_basis_points, 6_000);
    let roi_pixels = 49_u64 * 24;
    assert_eq!(roi_pixels, 1_176);
    assert_eq!(
        u64::from(CC6_QC_RASTER.0) * u64::from(CC6_SUB_THRESHOLD_ROI.width_basis_points) / 10_000,
        49
    );
    assert_eq!(
        u64::from(CC6_QC_RASTER.1) * u64::from(CC6_SUB_THRESHOLD_ROI.height_basis_points) / 10_000,
        24
    );
    // In it every counter is one pixel out of 1 176 = 8 bp, below 10.
    assert_eq!(10_000 / roi_pixels, 8);
    assert!(10_000 / roi_pixels < 10);
}

// ===========================================================================
// §11.1: the synthetic delivery source.
// ===========================================================================

/// The §11.1 delivery source raster. 320 × 180 keeps the encoded round trip
/// inside a default-lane CI budget on both operating systems while still
/// carrying every population §11.1 names.
pub(crate) const CC6_DELIVERY_SOURCE_SIZE: (u32, u32) = (320, 180);
/// 25 fps: the encoder's GOP is `2 · fps = 50` (`export.rs`), so 60 frames
/// span **two** GOPs and §6.2's sample set puts the last sample in the second.
pub(crate) const CC6_DELIVERY_SOURCE_FPS: u32 = 25;
/// 60 frames, so `T = 60` and §6.2's five samples are `0, 14, 29, 44, 59`.
pub(crate) const CC6_DELIVERY_SOURCE_FRAMES: u32 = 60;
/// The frames §6.2 samples on this source at the default `n = 5`, transcribed
/// from `f_i = floor(i · (T − 1) / (n − 1))` rather than obtained from the
/// production sampler.
pub(crate) const CC6_DELIVERY_SOURCE_SAMPLES: [u64; 5] = [0, 14, 29, 44, 59];
/// The encoder GOP `2 · fps` this source is sized against.
const CC6_DELIVERY_SOURCE_GOP: u32 = 2 * CC6_DELIVERY_SOURCE_FPS;

/// The pinned moving element: a 16 × 16 white square whose top-left corner is
/// at `(4 · frame, 20)` in the 320 × 180 base grid.
const CC6_MOVING_SQUARE_SIZE: u32 = 16;
const CC6_MOVING_SQUARE_STEP: u32 = 4;
const CC6_MOVING_SQUARE_TOP: u32 = 20;

/// CC1's own chart patch width (`chart_frame`), so the twelve-patch chart has
/// CC1's proportions on this raster.
const CC6_CHART_PATCH_WIDTH: u32 = 8;
/// The CC5 skin and product patches are wider, because §11.2.5's circular
/// statistics are measured on a region inside one of them.
const CC6_SKIN_PATCH_WIDTH: u32 = 12;

/// The §3.1 display codes of the twelve CC1 reference patches under a neutral
/// correction, **independently transcribed** from CC1 §3.1's analytic table
/// (`round(255 · (1.099·L^0.45 − 0.099))` above the linear seam).
const CC6_NEUTRAL_CHART_CODES: [[u8; 3]; 12] = [
    [0, 0, 0],
    [11, 11, 11],
    [104, 104, 104],
    [180, 180, 180],
    [242, 242, 242],
    [255, 255, 255],
    [255, 0, 0],
    [0, 255, 0],
    [0, 0, 255],
    [0, 255, 255],
    [255, 0, 255],
    [255, 255, 0],
];

/// The §3.4 forward BT.709 limited-range matrix, **independently transcribed**
/// in `f64` from the contract's equations.
///
/// Rule 11.0.1 forbids obtaining an *expected value* from
/// `bt709_limited_ycbcr`; it explicitly permits generating *source content*
/// with an independent transcription, which is what this is. Nothing in this
/// file compares a measurement against the output of this function.
fn bt709_limited_source_codes(rgb: [u8; 3]) -> [u8; 3] {
    const KR: f64 = 0.2126;
    const KB: f64 = 0.0722;
    const KG: f64 = 1.0 - KR - KB;
    const CB_DENOMINATOR: f64 = 1.8556;
    const CR_DENOMINATOR: f64 = 1.5748;
    let [r, g, b] = rgb.map(|code| f64::from(code) / 255.0);
    let luma = KR * r + KG * g + KB * b;
    let cb = (b - luma) / CB_DENOMINATOR;
    let cr = (r - luma) / CR_DENOMINATOR;
    let code = |value: f64| {
        let rounded = value.round();
        assert!(
            (0.0..=255.0).contains(&rounded),
            "a source code must land inside the 8-bit container: {value}"
        );
        rounded as u8
    };
    [
        code(16.0 + 219.0 * luma),
        code(128.0 + 224.0 * cb),
        code(128.0 + 224.0 * cr),
    ]
}

/// The display-encoded `R'G'B'` of one source pixel, in the 320 × 180 base
/// grid, at `frame`.
///
/// Every region is a rectangle of the base grid, so a larger raster is an
/// integer replication of it and the P9 1080p measurement runs on the same
/// content as the 320 × 180 gate.
fn cc6_source_base_rgb(x: u32, y: u32, frame: u32) -> [u8; 3] {
    let square_left = CC6_MOVING_SQUARE_STEP * frame;
    let grey = |code: u8| [code, code, code];
    // CC5's achromatic surround, at its display-encoded code. Everything that
    // is not a named region sits in it, which is what keeps the saturated
    // content a *feature* of the raster rather than most of it.
    let surround = grey((f64::from(CC6_CHART_SURROUND[0]) * 255.0).round() as u8);
    if y < 20 {
        // Horizontal neutral ramp.
        grey(u8::try_from(x * 255 / 319).expect("a ramp code"))
    } else if y < 36 {
        // The band the pinned moving element travels through.
        if (square_left..square_left + CC6_MOVING_SQUARE_SIZE).contains(&x)
            && (CC6_MOVING_SQUARE_TOP..CC6_MOVING_SQUARE_TOP + CC6_MOVING_SQUARE_SIZE).contains(&y)
        {
            grey(255)
        } else {
            grey(128)
        }
    } else if (36..52).contains(&y) && x < 12 * CC6_CHART_PATCH_WIDTH {
        // The twelve-patch CC1 neutral chart, at CC1's own eight-pixel patch
        // width (`chart_frame`, `cc1_fixtures.rs:1142-1155`) rather than
        // stretched across the raster: the chart's proportions are CC1's, and
        // a 320-wide chart would put eleven hard chroma edges in a fifth of
        // every row.
        CC6_NEUTRAL_CHART_CODES[(x / CC6_CHART_PATCH_WIDTH) as usize]
    } else if (76..92).contains(&y) && x < 6 * CC6_SKIN_PATCH_WIDTH {
        // The four CC5 skin patches and the two product patches, at their
        // display-encoded codes.
        let patch = (x / CC6_SKIN_PATCH_WIDTH) as usize;
        let encoded = if patch < 4 {
            CC6_SKIN_PATCHES[patch]
        } else {
            CC6_PRODUCT_PATCHES[patch - 4]
        };
        encoded.map(|value| (f64::from(value) * 255.0).round() as u8)
    } else if (116..134).contains(&y) && (40..120).contains(&x) {
        // **One** hard saturated edge: a pure-blue block abutting a pure-green
        // block, so §6.3(c)'s RGB-max term is exercised and reported. It is
        // one edge, deliberately: §6.3 measures what 4:2:0 decimation costs at
        // such an edge, and a raster made of them would measure the raster.
        if x < 80 { [0, 0, 255] } else { [0, 255, 0] }
    } else if y >= 146 {
        // Vertical neutral ramp.
        grey(u8::try_from((y - 146) * 255 / 33).expect("a ramp code"))
    } else {
        surround
    }
}

/// One frame of the source as `yuv444p` planes, at an integer multiple of the
/// 320 × 180 base grid.
fn cc6_source_frame_planes(size: (u32, u32), frame: u32) -> Vec<u8> {
    let (width, height) = size;
    assert_eq!(width % CC6_DELIVERY_SOURCE_SIZE.0, 0);
    assert_eq!(height % CC6_DELIVERY_SOURCE_SIZE.1, 0);
    let scale_x = width / CC6_DELIVERY_SOURCE_SIZE.0;
    let scale_y = height / CC6_DELIVERY_SOURCE_SIZE.1;
    let count = (width * height) as usize;
    let mut luma = Vec::with_capacity(count);
    let mut cb = Vec::with_capacity(count);
    let mut cr = Vec::with_capacity(count);
    for y in 0..height {
        for x in 0..width {
            let rgb = cc6_source_base_rgb(x / scale_x, y / scale_y, frame);
            let codes = bt709_limited_source_codes(rgb);
            luma.push(codes[0]);
            cb.push(codes[1]);
            cr.push(codes[2]);
        }
    }
    luma.append(&mut cb);
    luma.append(&mut cr);
    luma
}

/// §11.1's pinned moving element, at every frame §6.2 samples.
///
/// The delivery source's *only* temporal content is the 16 x 16 white square
/// that steps [`CC6_MOVING_SQUARE_STEP`] pixels a frame through the band at
/// `y ∈ [20, 36)`. If it ever stopped moving — a stale `frame` argument, a
/// band that drifted, a step that changed — every sampled frame would carry
/// identical content, the encode would become a still, and both exit gates
/// would go on passing while measuring nothing about a moving picture. So the
/// element is asserted where §11.1 claims it is, in the bytes actually
/// written, at each of §6.2's five sampled frames.
///
/// Both codes are transcribed independently (rule 11.0.1) rather than obtained
/// from [`bt709_limited_source_codes`]: display white `255` is limited luma
/// `16 + 219 = 235`, and the band's surround grey `128` is
/// `round(16 + 219 · 128 / 255) = 126`.
#[test]
fn cc6_delivery_source_moves_the_pinned_element_across_the_sampled_frames() {
    /// The middle row of the band the element travels through.
    const BAND_ROW: u32 = 28;
    /// Limited luma of display white, hand-derived.
    const NEAR_WHITE_LUMA: u8 = 235;
    /// Limited luma of the band's surround grey, hand-derived.
    const BAND_GREY_LUMA: u8 = 126;
    /// How far clear of the element the surround is sampled.
    const CLEARANCE: u32 = 8;

    let (width, _height) = CC6_DELIVERY_SOURCE_SIZE;
    assert!(
        (CC6_MOVING_SQUARE_TOP..CC6_MOVING_SQUARE_TOP + CC6_MOVING_SQUARE_SIZE).contains(&BAND_ROW),
        "the sampled row must be inside the band the element travels through"
    );
    let mut positions = Vec::new();
    for frame in CC6_DELIVERY_SOURCE_SAMPLES.map(|frame| u32::try_from(frame).expect("a frame")) {
        assert!(frame < CC6_DELIVERY_SOURCE_FRAMES);
        let planes = cc6_source_frame_planes(CC6_DELIVERY_SOURCE_SIZE, frame);
        let luma = |x: u32| planes[(BAND_ROW * width + x) as usize];
        let square_left = CC6_MOVING_SQUARE_STEP * frame;
        positions.push(square_left);

        // Eight pixels into the element: its middle column, near-white.
        let inside = square_left + CLEARANCE;
        assert_eq!(
            cc6_source_base_rgb(inside, BAND_ROW, frame),
            [255, 255, 255],
            "frame {frame}: the element must cover ({inside}, {BAND_ROW})"
        );
        assert_eq!(
            luma(inside),
            NEAR_WHITE_LUMA,
            "frame {frame}: ({inside}, {BAND_ROW}) must be written as near-white limited luma"
        );

        // Eight pixels clear of it, in the same band: the surround grey the
        // element travels through. At frame 0 the element sits against the
        // left edge and there is no pixel eight to its left, so the pixel
        // eight clear of its *right* edge — the same population, the same row
        // — stands in.
        let outside = if square_left >= CLEARANCE {
            square_left - CLEARANCE
        } else {
            square_left + CC6_MOVING_SQUARE_SIZE + CLEARANCE
        };
        assert!(outside < width);
        assert_eq!(
            cc6_source_base_rgb(outside, BAND_ROW, frame),
            [128, 128, 128],
            "frame {frame}: the element must not cover ({outside}, {BAND_ROW})"
        );
        assert_eq!(
            luma(outside),
            BAND_GREY_LUMA,
            "frame {frame}: ({outside}, {BAND_ROW}) must be written as the band's surround grey"
        );
    }
    // Non-vacuity: five distinct positions, strictly increasing, none of them
    // overlapping the previous sample's element.
    assert_eq!(positions.len(), CC6_DELIVERY_SOURCE_SAMPLES.len());
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(
        positions
            .windows(2)
            .all(|pair| pair[1] - pair[0] >= CC6_MOVING_SQUARE_SIZE),
        "two consecutive sampled frames must not show the element in overlapping columns, or the \
         sample set does not distinguish them: {positions:?}"
    );
}

/// **`cc6_delivery_source()`** — §11.1's synthetic delivery source, written as
/// a tagged limited-range BT.709 FFV1 file.
///
/// The fixture writes the raw `yuv444p` frames to a temp file and invokes the
/// pinned CLI through [`run_ffmpeg`], because `run_ffmpeg` cannot pipe stdin.
/// The tag recipe is CC1's (`generate_delivery_source` / `generate_ramp_media`):
/// `setparams` **and** the explicit `-color_*` flags, which CC1 requires
/// because the managed import rejects an untagged source.
///
/// `frames` is [`CC6_DELIVERY_SOURCE_FRAMES`] for every §11.2 gate. The P9
/// cost measurement (§11.2.24) passes a smaller count at a larger raster,
/// because what it measures is the per-frame cost of the 1080p path, not the
/// sampling rule.
pub(crate) fn cc6_delivery_source(
    directory: &TempDirectory,
    size: (u32, u32),
    frames: u32,
) -> PathBuf {
    let mut raw = Vec::new();
    for frame in 0..frames {
        raw.extend(cc6_source_frame_planes(size, frame));
    }
    assert_eq!(raw.len(), (size.0 * size.1 * 3 * frames) as usize);
    let raw_path = directory.path("cc6-delivery-source.yuv");
    std::fs::write(&raw_path, &raw).expect("the raw CC6 delivery source should write");
    let path = directory.path("cc6-delivery-source.mkv");
    let arguments = [
        "-f".to_owned(),
        "rawvideo".to_owned(),
        "-pix_fmt".to_owned(),
        "yuv444p".to_owned(),
        "-s".to_owned(),
        format!("{}x{}", size.0, size.1),
        "-r".to_owned(),
        CC6_DELIVERY_SOURCE_FPS.to_string(),
        "-i".to_owned(),
        raw_path.to_string_lossy().into_owned(),
        "-vf".to_owned(),
        "setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709".to_owned(),
        "-c:v".to_owned(),
        "ffv1".to_owned(),
        "-level".to_owned(),
        "3".to_owned(),
        "-g".to_owned(),
        "1".to_owned(),
        "-pix_fmt".to_owned(),
        "yuv444p".to_owned(),
        "-color_primaries".to_owned(),
        "bt709".to_owned(),
        "-color_trc".to_owned(),
        "bt709".to_owned(),
        "-colorspace".to_owned(),
        "bt709".to_owned(),
        "-color_range".to_owned(),
        "tv".to_owned(),
    ];
    run_ffmpeg(&arguments, &path);
    path
}

/// One `color_wheels` or `primary_correction` node.
fn effect_with(id: u64, name: &str, parameters: &[(&str, i64)]) -> Effect {
    Effect {
        id: EffectId(id),
        name: name.to_owned(),
        parameters: parameters
            .iter()
            .map(|(name, value)| ((*name).to_owned(), ParamValue::Integer(*value)))
            .collect::<BTreeMap<_, _>>(),
        keyframes: BTreeMap::new(),
    }
}

/// The §11.1 grade: a deliberately over-range and out-of-gamut managed stack,
/// so the excursion the encoded gate measures is a *product of the pipeline*
/// rather than a hand-written buffer.
///
/// A `color_wheels` node applies a strong gain inside one elliptical matte
/// window and a second applies a strong negative lift inside another, so the
/// raster carries both directions at once and the two regions are disjoint.
fn cc6_delivery_grade() -> Vec<Effect> {
    let mut gain = effect_with(1, "color_wheels", &[("gain_master_thousandths", 1_800)]);
    for (name, value) in [
        ("matte_enabled", 1),
        ("matte_window_count", 1),
        ("matte_window0_shape_token", 1),
        ("matte_window0_center_x_basis_points", 2_500),
        ("matte_window0_center_y_basis_points", 3_000),
        ("matte_window0_half_width_basis_points", 2_000),
        ("matte_window0_half_height_basis_points", 2_500),
        ("matte_window0_feather_basis_points", 1_500),
    ] {
        gain.parameters
            .insert(name.to_owned(), ParamValue::Integer(value));
    }
    let mut lift = effect_with(2, "color_wheels", &[("lift_master_basis_points", -1_600)]);
    for (name, value) in [
        ("matte_enabled", 1),
        ("matte_window_count", 1),
        ("matte_window0_shape_token", 1),
        ("matte_window0_center_x_basis_points", 7_500),
        ("matte_window0_center_y_basis_points", 7_000),
        ("matte_window0_half_width_basis_points", 2_000),
        ("matte_window0_half_height_basis_points", 2_500),
        ("matte_window0_feather_basis_points", 1_500),
    ] {
        lift.parameters
            .insert(name.to_owned(), ParamValue::Integer(value));
    }
    vec![gain, lift]
}

/// The managed import of [`cc6_delivery_source`] with the §11.1 grade applied.
pub(crate) fn cc6_delivery_document(source: &Path, size: (u32, u32), frames: u32) -> Document {
    let asset = probe_path(source, AssetId(2)).expect("the CC6 delivery source should probe");
    // The managed import path is CC1's: an untagged source is refused, so the
    // tags are asserted here rather than assumed.
    assert_eq!(asset.color_description.primaries, ColorPrimaries::Bt709);
    assert_eq!(asset.color_description.transfer, ColorTransfer::Bt709);
    assert_eq!(asset.color_description.matrix, ColorMatrix::Bt709);
    assert_eq!(asset.color_description.range, ColorRange::Limited);
    assert_eq!(asset.color_description.bit_depth, ColorBitDepth::Eight);
    assert_eq!(asset.duration, TimeCode(i64::from(frames)));
    assert_eq!(asset.resolution, Some(size));
    let mut document = simple_document(asset, size);
    document.tracks[0].clips[0].effects = cc6_delivery_grade();
    document
        .validate()
        .expect("the CC6 delivery document should validate");
    document
}

/// The production export settings for one lane, from the production profile.
pub(crate) fn cc6_delivery_settings(
    document: &Document,
    depth: DeliveryEncodeDepth,
) -> ExportSettings {
    let settings = DeliveryProfile::SourceMaster.export_settings(
        document,
        depth,
        ExportCancellation::default(),
    );
    // §4.1: the depth argument is the single authority. The document keeps
    // declaring the project's 8-bit delivery contract either way.
    assert_eq!(
        document.color_context.delivery.bit_depth,
        ColorBitDepth::Eight
    );
    assert_eq!(settings.resolution, document.resolution);
    assert_eq!(settings.fps, document.fps);
    settings
}

// ===========================================================================
// The encoded round trip: shared measurement, §11.2.10 / §11.2.11 / §11.2.13.
// ===========================================================================

/// Everything one lane's export and verification measured, so the two exit
/// gates, the starved-bitrate failing direction, and the evidence payload all
/// read the same numbers.
struct LaneMeasurement {
    depth: DeliveryEncodeDepth,
    output: PathBuf,
    verification: DeliveryVerification,
    export_seconds: f64,
    verify_seconds: f64,
}

/// Export one lane through the production path and verify it through the
/// production engine.
fn export_and_verify(
    gpu: &FixtureGpu,
    directory: &TempDirectory,
    name: &str,
    document: &Arc<Document>,
    settings: &ExportSettings,
    depth: DeliveryEncodeDepth,
) -> LaneMeasurement {
    let output = directory.path(name);
    let (progress_tx, _progress_rx) = crossbeam_channel::unbounded();
    let started = Instant::now();
    crate::export::export_document(
        document.as_ref(),
        &output,
        settings,
        &progress_tx,
        gpu.context(),
    )
    .expect("the production export path must write the delivery lane");
    let export_seconds = started.elapsed().as_secs_f64();

    let engine = crate::engine::FfmpegMediaEngine::new_with_gpu(gpu.context())
        .expect("the production media engine should start");
    let request = DeliveryVerificationRequest::new(depth, settings.delivery_color.clone());
    assert_eq!(request.frame_count, DELIVERY_VERIFICATION_FRAME_COUNT);
    let started = Instant::now();
    let verification = engine
        .verify_delivery_output(Arc::clone(document), &output, settings, request)
        .expect("the written export must verify");
    let verify_seconds = started.elapsed().as_secs_f64();

    LaneMeasurement {
        depth,
        output,
        verification,
        export_seconds,
        verify_seconds,
    }
}

/// The measured margin of one budget: `allowed / observed`, with an exactly
/// zero measurement reported as infinite rather than as a division.
fn margin(observed: i64, allowed: i64) -> f64 {
    if observed == 0 {
        f64::INFINITY
    } else {
        allowed as f64 / observed as f64
    }
}

/// The four gated ratios and the PSNR headroom of one lane.
#[derive(Debug, Clone, Copy)]
struct LaneMargins {
    luma_max: f64,
    luma_p99: f64,
    luma_mean: f64,
    rgb_mean: f64,
    psnr_headroom_hundredths: i32,
}

/// §11.2.10 and §11.2.11's shared body: everything both exit gates assert
/// about a lane that is inside its budgets.
fn assert_encoded_delivery_lane(lane: &LaneMeasurement, gpu: &FixtureGpu) -> LaneMargins {
    let depth = lane.depth;
    let verification = &lane.verification;
    let comparison = &verification.comparison;

    // --- the re-probe (§11.2.10's tag clause) ---------------------------
    assert_eq!(verification.delivery_bit_depth, depth);
    assert_eq!(verification.output_path, lane.output);
    assert_eq!(verification.decoded_pixel_format, depth.pixel_format());
    assert_eq!(verification.probed.primaries, ColorPrimaries::Bt709);
    assert_eq!(verification.probed.transfer, ColorTransfer::Bt709);
    assert_eq!(verification.probed.matrix, ColorMatrix::Bt709);
    assert_eq!(verification.probed.range, ColorRange::Limited);
    assert_eq!(
        verification.probed.bit_depth,
        match depth {
            DeliveryEncodeDepth::Eight => ColorBitDepth::Eight,
            DeliveryEncodeDepth::Ten => ColorBitDepth::Ten,
        },
        "a lane that silently delivered the other depth would land here"
    );
    assert_eq!(
        verification.probed.provenance,
        ColorProvenance::StreamMetadata
    );
    assert_eq!(verification.probed.confidence_basis_points, 10_000);
    assert_eq!(verification.tags.tag_source, "probed_output_file");
    assert!(
        verification.tags.conforming,
        "a probed managed export must carry conforming tags: {:?}",
        verification.tags.mismatches
    );
    assert!(verification.tags.mismatches.is_empty());
    // H.264 has no syntax for a white point, so exactly one field is not
    // representable and it is that one.
    assert_eq!(verification.tags.not_representable.len(), 1);
    assert_eq!(verification.tags.not_representable[0].field, "white_point");
    assert_eq!(verification.probed.white_point, ColorWhitePoint::Unknown);

    // --- §6.2 sampling ---------------------------------------------------
    assert_eq!(
        comparison.frames,
        CC6_DELIVERY_SOURCE_SAMPLES
            .map(|frame| frame as i64)
            .to_vec(),
        "§6.2 on T = 60 with n = 5 samples 0, 14, 29, 44, 59"
    );
    assert_eq!(
        u64::from(CC6_DELIVERY_SOURCE_FRAMES) / u64::from(CC6_DELIVERY_SOURCE_GOP),
        1,
        "60 frames span two GOPs of 50"
    );
    assert!(
        u64::from(CC6_DELIVERY_SOURCE_FRAMES) > u64::from(CC6_DELIVERY_SOURCE_GOP),
        "the last sample must fall in the second GOP"
    );
    assert!(
        *CC6_DELIVERY_SOURCE_SAMPLES.last().expect("a last sample")
            >= u64::from(CC6_DELIVERY_SOURCE_GOP)
    );

    // --- the reported, never gated, RGB extremes --------------------------
    assert_eq!(comparison.rgb_extremes_note, DELIVERY_RGB_EXTREMES_NOTE);
    assert!(comparison.rgb_extremes_note.contains("4:2:0"));
    // §6.3(c) is asserted, not just documented: the whole-raster RGB maximum
    // is larger than *every* gated bound on this source — the saturated edge
    // §11.1 mandates sees to that — and yet no exception names it and no
    // budget field carries it. A version that quietly gated it would fail
    // here.
    assert!(
        u64::from(comparison.combined.maximum_code_diff)
            > u64::from(comparison.budgets.luma_max_code),
        "the RGB maximum must exceed the gated luma maximum on this source, or 'reported, not \
         gated' is an untested claim"
    );
    for ungated in [
        "combined.maximum_code_diff",
        "combined.p99_code_diff_millionths",
    ] {
        assert!(
            !verification
                .exceptions
                .iter()
                .any(|exception| exception.field.as_deref() == Some(ungated)),
            "{ungated} is evidence, not a gate, and must never raise an exception"
        );
    }

    // --- §6.4 decoded native-plane legality -------------------------------
    assert_eq!(
        comparison.decoded_ycbcr.source,
        YCbCrLegalSource::DecodedNativePlanes
    );
    assert_eq!(
        comparison.decoded_ycbcr.bit_depth,
        match depth {
            DeliveryEncodeDepth::Eight => 8,
            DeliveryEncodeDepth::Ten => 10,
        }
    );
    let scale = i64::from(1_u8 << (comparison.decoded_ycbcr.bit_depth - 8));
    let tolerance = EBU_R103_TOLERANCE_CODES_8BIT * scale;
    let mut over_threshold_planes = Vec::new();
    let mut under_threshold_planes = Vec::new();
    // §6.4 (a)'s rate is taken over the plane's **own** sampled population, so
    // the fixture predicts that population from §11.1's raster and §6.2's
    // sample count rather than reading it back from the report. 4:2:0 makes
    // each chroma plane a quarter of the luma plane.
    let sampled_frames = u64::try_from(comparison.frames.len()).expect("a sample count");
    let luma_samples = u64::from(CC6_DELIVERY_SOURCE_SIZE.0)
        * u64::from(CC6_DELIVERY_SOURCE_SIZE.1)
        * sampled_frames;
    let chroma_samples = luma_samples / 4;
    for (name, plane, high, samples) in [
        (
            "luma",
            &comparison.decoded_ycbcr.luma,
            235 * scale,
            luma_samples,
        ),
        (
            "cb",
            &comparison.decoded_ycbcr.cb,
            240 * scale,
            chroma_samples,
        ),
        (
            "cr",
            &comparison.decoded_ycbcr.cr,
            240 * scale,
            chroma_samples,
        ),
    ] {
        // The EBU R 103 box is the hard half of the §6.4 rule, and this
        // source stays inside it on both lanes: after `ad6f6a8` the encoder
        // input never exceeds legal, so every decoded excursion is codec
        // ringing of a code or two.
        assert!(
            plane.minimum_code_hundredths >= (16 * scale - tolerance) * 100,
            "{name} minimum {} is outside the EBU R 103 box",
            plane.minimum_code_hundredths
        );
        assert!(
            plane.maximum_code_hundredths <= (high + tolerance) * 100,
            "{name} maximum {} is outside the EBU R 103 box",
            plane.maximum_code_hundredths
        );
        // The strict-box rate is the soft half of §6.4, and it is core's own
        // accessor over the **combined** count — `below + above`, not
        // `max(below, above)`, which under-reports exactly the plane that
        // leaves the box in both directions. `verify.rs`'s gate calls the same
        // accessor, so the prediction and the gate agree only if both are
        // right.
        //
        // The predicted population is checked against the report's two
        // separately reported rates first, so a wrong raster or a wrong sample
        // count fails here rather than quietly yielding a plausible combined
        // rate.
        assert_eq!(
            plane.below_basis_points,
            u32::try_from(plane.below_count.saturating_mul(10_000) / samples).expect("a rate"),
            "{name}: the predicted sampled population of {samples} disagrees with the reported \
             below-box rate"
        );
        assert_eq!(
            plane.above_basis_points,
            u32::try_from(plane.above_count.saturating_mul(10_000) / samples).expect("a rate"),
            "{name}: the predicted sampled population of {samples} disagrees with the reported \
             above-box rate"
        );
        // This source exercises the **under-threshold** direction only: after
        // `ad6f6a8` the encoder input never exceeds legal, so what is left is
        // codec ringing of a code or two and no plane is expected to reach
        // `DECODED_RANGE_EXCEPTION_BASIS_POINTS`. The *raising* direction is
        // owned by
        // `cc6_decoded_native_planes_report_ycbcr_excursions_in_delivery_code_units`
        // in `verify.rs`, which hand-builds a file with deliberately illegal
        // codes; the branch below stays here so a source that ever did cross
        // the threshold would still have to be reported rather than ignored.
        let rate = plane.excursion_basis_points(samples);
        let reported = verification.exceptions.iter().find(|exception| {
            exception.code == "decoded_range_excursion"
                && exception.field.as_deref() == Some(&format!("decoded_ycbcr.{name}"))
        });
        if rate > DECODED_RANGE_EXCEPTION_BASIS_POINTS {
            let reported = reported.unwrap_or_else(|| {
                panic!("{name} exceeds the strict-box rate at {rate} bp and must be reported")
            });
            // Rule 11.0.4: code, field, observed, allowed.
            assert_eq!(
                reported.severity,
                QaSeverity::Warning,
                "a decoded excursion is never an Error: it cannot be gated at zero (§6.4)"
            );
            assert_eq!(
                reported.field.as_deref(),
                Some(&*format!("decoded_ycbcr.{name}"))
            );
            assert!(reported.observed.is_some(), "{reported:?}");
            assert!(reported.allowed.is_some(), "{reported:?}");
            over_threshold_planes.push(name);
        } else {
            assert!(
                reported.is_none(),
                "{name} is under the strict-box rate at {rate} bp; on this source every plane \
                 also sits inside the R 103 box, so it must raise nothing: {reported:?}"
            );
            under_threshold_planes.push(name);
        }
    }
    println!(
        "CC6_LANE_R103 lane={} depth={:?} over_threshold={over_threshold_planes:?} under_threshold={under_threshold_planes:?}",
        gpu.lane.id(),
        depth
    );
    assert!(
        !under_threshold_planes.is_empty(),
        "at least one decoded plane must stay below the strict-box threshold, or the threshold \
         has no passing direction on this source"
    );

    // --- non-vacuity ------------------------------------------------------
    assert!(
        comparison.combined.mean_code_diff_millionths > 0,
        "the source does not exercise the codec"
    );
    assert!(comparison.luma.maximum_code_diff > 0);

    // --- §6.3(a) + (b), with the measured margin (rule 11.0.5) ------------
    let budgets = comparison.budgets;
    assert_eq!(budgets, kinewright_core::DeliveryBudgets::for_depth(depth));
    let margins = LaneMargins {
        luma_max: margin(
            i64::from(comparison.luma.maximum_code_diff),
            i64::from(budgets.luma_max_code),
        ),
        luma_p99: margin(
            comparison.luma.p99_code_diff_millionths,
            budgets.luma_p99_code_millionths,
        ),
        luma_mean: margin(
            comparison.luma.mean_code_diff_millionths,
            budgets.luma_mean_code_millionths,
        ),
        rgb_mean: margin(
            comparison.combined.mean_code_diff_millionths,
            budgets.rgb_mean_code_millionths,
        ),
        psnr_headroom_hundredths: comparison
            .psnr_db_hundredths
            .expect("a lossy encode has a finite MSE")
            - budgets.psnr_floor_db_hundredths,
    };
    println!(
        "CC6_LANE_MEASURED lane={} depth={:?} luma_max={} luma_p99_millionths={} luma_mean_millionths={} rgb_mean_millionths={} psnr_hundredths={:?} rgb_max={} rgb_p99_millionths={} r_mean_millionths={} g_mean_millionths={} b_mean_millionths={}",
        gpu.lane.id(),
        depth,
        comparison.luma.maximum_code_diff,
        comparison.luma.p99_code_diff_millionths,
        comparison.luma.mean_code_diff_millionths,
        comparison.combined.mean_code_diff_millionths,
        comparison.psnr_db_hundredths,
        comparison.combined.maximum_code_diff,
        comparison.combined.p99_code_diff_millionths,
        comparison.red.mean_code_diff_millionths,
        comparison.green.mean_code_diff_millionths,
        comparison.blue.mean_code_diff_millionths,
    );
    println!(
        "CC6_LANE_MARGIN lane={} depth={:?} luma_max={:.3}x luma_p99={:.3}x luma_mean={:.3}x rgb_mean={:.3}x psnr_headroom_hundredths={}",
        gpu.lane.id(),
        depth,
        margins.luma_max,
        margins.luma_p99,
        margins.luma_mean,
        margins.rgb_mean,
        margins.psnr_headroom_hundredths,
    );
    println!(
        "CC6_LANE_LEGAL lane={} depth={:?} luma={}..={} cb={}..={} cr={}..={} luma_bp={}/{} cb_bp={}/{} cr_bp={}/{}",
        gpu.lane.id(),
        depth,
        comparison.decoded_ycbcr.luma.minimum_code_hundredths,
        comparison.decoded_ycbcr.luma.maximum_code_hundredths,
        comparison.decoded_ycbcr.cb.minimum_code_hundredths,
        comparison.decoded_ycbcr.cb.maximum_code_hundredths,
        comparison.decoded_ycbcr.cr.minimum_code_hundredths,
        comparison.decoded_ycbcr.cr.maximum_code_hundredths,
        comparison.decoded_ycbcr.luma.below_basis_points,
        comparison.decoded_ycbcr.luma.above_basis_points,
        comparison.decoded_ycbcr.cb.below_basis_points,
        comparison.decoded_ycbcr.cb.above_basis_points,
        comparison.decoded_ycbcr.cr.below_basis_points,
        comparison.decoded_ycbcr.cr.above_basis_points,
    );

    // Rule 11.0.5: a budget no measurement approaches proves nothing, so every
    // gated term records its measured margin and asserts it.
    //
    // The two luma terms and the luma mean are the *codec-only* error: no
    // chroma decimation enters them, which is why §6.3 makes them the gate.
    for (name, measured) in [
        ("luma_max", margins.luma_max),
        ("luma_p99", margins.luma_p99),
        ("luma_mean", margins.luma_mean),
    ] {
        assert!(
            measured >= 2.0,
            "the {name} budget must keep at least a 2x margin on cc6_delivery_source(); measured \
             {measured}x"
        );
    }
    // The RGB mean is the whole-raster **sanity floor**, in the
    // 8-bit-equivalent units §6.3 words it in — `verify.rs` reports it in
    // those units, so nothing is converted here. It is dominated by the 4:2:0
    // chroma decimation §6.3(c) says must never be gated, which is why it was
    // re-baselined against this source rather than left at the value the
    // 1920x1080 probe chart produced, where the same saturated edges are a
    // ~36x smaller fraction of the raster.
    assert!(
        margins.rgb_mean >= 2.0,
        "the {depth:?} lane's RGB-mean budget must keep at least a 2x margin on \
         cc6_delivery_source(); measured {} 8-bit-equivalent code millionths against a budget of \
         {}, margin {}x",
        comparison.combined.mean_code_diff_millionths,
        budgets.rgb_mean_code_millionths,
        margins.rgb_mean,
    );
    assert!(
        margins.psnr_headroom_hundredths > 0,
        "the {depth:?} lane measures {} hundredths of a dB PSNR on cc6_delivery_source(), below \
         the floor of {}. PSNR is a whole-raster sanity floor on the same 8-bit-equivalent RGB \
         population as the mean above, so 4:2:0 chroma decimation dominates it too; the luma \
         plane — the codec-only term — is {}x inside its own maximum budget on this source.",
        comparison
            .psnr_db_hundredths
            .expect("a lossy encode has a finite MSE"),
        budgets.psnr_floor_db_hundredths,
        margins.luma_max,
    );
    assert!(comparison.within_budgets, "{comparison:?}");
    assert!(u64::from(comparison.luma.maximum_code_diff) <= u64::from(budgets.luma_max_code));
    assert!(comparison.luma.p99_code_diff_millionths <= budgets.luma_p99_code_millionths);
    assert!(comparison.luma.mean_code_diff_millionths <= budgets.luma_mean_code_millionths);
    assert!(comparison.combined.mean_code_diff_millionths <= budgets.rgb_mean_code_millionths);
    assert!(verification.technical_pass, "{:?}", verification.exceptions);
    assert!(
        !verification
            .exceptions
            .iter()
            .any(|exception| exception.severity == QaSeverity::Error)
    );

    // --- the budgets are not the compositor gate --------------------------
    assert_delivery_budgets_are_distinct(depth);
    margins
}

/// CC1 `cc1_fixtures.rs:3930-3940`'s rule, applied to CC6's lane budgets: a
/// codec tolerance and a compositor tolerance must be numerically distinct so
/// neither can be silently substituted for the other.
fn assert_delivery_budgets_are_distinct(depth: DeliveryEncodeDepth) {
    let budgets = kinewright_core::DeliveryBudgets::for_depth(depth);
    let monitor = [
        f64::from(MONITOR_CPU_GPU_MAX),
        MONITOR_CPU_GPU_P99,
        MONITOR_CPU_GPU_MEAN,
    ];
    let delivery = [
        f64::from(budgets.luma_max_code),
        budgets.luma_p99_code_millionths as f64 / 1_000_000.0,
        budgets.luma_mean_code_millionths as f64 / 1_000_000.0,
    ];
    for (index, (monitor, delivery)) in monitor.into_iter().zip(delivery).enumerate() {
        assert_ne!(
            monitor, delivery,
            "MONITOR_CPU_GPU and the {depth:?} delivery budget agree on term {index}; a codec \
             tolerance and a compositor tolerance must never be interchangeable"
        );
    }
}

/// Assert, for every sampled frame, that a full-resolution delivery reference
/// exists and claims `full_resolution` through the *production* derivation.
///
/// The claim is derived, never asserted as a flag: the scale that was
/// requested **and** the raster that came back are both fed to
/// `monitor_proof_metadata_for`, exactly as `verify.rs`'s
/// `delivery_reference` does. The failing leg — a proxy scale — is
/// §11.2.15's, and it lives with the code it refuses.
fn assert_per_frame_full_resolution_reference(
    gpu: &GpuContext,
    document: &Document,
    settings: &ExportSettings,
    frames: &[i64],
) {
    let mut renderer = FrameRenderer::new(gpu.clone());
    for frame in frames {
        let reference = renderer
            .render_delivery(
                document,
                TimeCode(*frame),
                settings.resolution,
                RenderScale::FullResolution,
                DecodeStrategy::Seek,
            )
            .expect("a full-resolution delivery reference renders");
        assert_eq!((reference.width, reference.height), settings.resolution);
        let claim = gpu.monitor_proof_metadata_for(
            RenderScale::FullResolution,
            (reference.width, reference.height),
            settings.resolution,
        );
        assert!(
            claim.full_resolution,
            "frame {frame}'s delivery reference must claim full resolution"
        );
        assert_eq!(
            reference.rgba64le.len(),
            (settings.resolution.0 * settings.resolution.1 * 8) as usize,
            "the delivery reference is the 16-bit RGBA intermediate"
        );
    }
}

/// Non-vacuity for §11.1's grade: the deliberately over-range and
/// out-of-gamut stack must actually clip, measured through the production QC
/// engine on the production working proof.
///
/// Without this the encoded gate could pass on a source whose grade had
/// silently become a no-op — a matte window that resolved to zero coverage,
/// say — and would then be measuring the codec on an ungraded raster while
/// claiming to measure the managed pipeline.
fn assert_the_delivery_grade_clips(gpu: &FixtureGpu, document: &Arc<Document>) {
    let engine = crate::engine::FfmpegMediaEngine::new_with_gpu(gpu.context())
        .expect("the production media engine should start");
    let proof = engine
        .working_proof_for_document(Arc::clone(document), TimeCode::ZERO)
        .expect("the delivery document's working proof renders");
    assert!(proof.metadata.render.full_resolution);
    let report = measure_color_qc(&proof, &ColorQcRequest::default())
        .expect("the delivery document's working proof measures");
    println!(
        "CC6_SOURCE_GRADE clamped_bp={} over_bp={:?} gamut_bp={} below_black={}",
        report.range.clamped_basis_points,
        [
            report.range.red.over_basis_points,
            report.range.green.over_basis_points,
            report.range.blue.over_basis_points,
        ],
        report.gamut.out_of_gamut_basis_points,
        report.gamut.below_black_pixel_count,
    );
    assert!(
        report.range.clamped_basis_points > 0,
        "§11.1's grade must clip: the encoded gate would otherwise measure an ungraded raster"
    );
    assert!(
        report.gamut.out_of_gamut_basis_points > 0,
        "§11.1's grade must drive part of the raster out of gamut"
    );
    for (name, channel) in [
        ("red", &report.range.red),
        ("green", &report.range.green),
        ("blue", &report.range.blue),
    ] {
        assert!(
            channel.over_basis_points > 0,
            "the gain window must push {name} over range: {channel:?}"
        );
        assert!(
            channel.maximum_over_excursion_millionths > 0,
            "an over-range channel must record how far over it went: {channel:?}"
        );
    }
    // The negative-lift window drives whole regions below black on every
    // channel, so `Y < 0` there and §3.3's desaturation fraction is undefined
    // and excluded from the maximum by design. What the grade must produce —
    // and what is asserted — is the below-black population itself.
    assert!(
        report.gamut.below_black_pixel_count > 0,
        "the negative-lift window must drive part of the raster below black"
    );
    assert_eq!(
        report.gamut.definition,
        kinewright_core::GAMUT_DEFINITION,
        "the gamut report states the range/gamut relation §3.3 makes normative"
    );
}

/// §11.2.10 — **the exit gate**, 8-bit lane.
#[test]
fn cc6_eight_bit_encoded_delivery_passes_tag_luma_and_difference_budgets() {
    crate::initialize_ffmpeg().expect("FFmpeg must initialize for the CC6 exit gate");
    let gpu = fallback_gpu();
    let directory = TempDirectory::new("cc6-eight-bit-exit-gate");
    let size = CC6_DELIVERY_SOURCE_SIZE;
    let source = cc6_delivery_source(&directory, size, CC6_DELIVERY_SOURCE_FRAMES);
    let document = Arc::new(cc6_delivery_document(
        &source,
        size,
        CC6_DELIVERY_SOURCE_FRAMES,
    ));
    let settings = cc6_delivery_settings(&document, DeliveryEncodeDepth::Eight);
    assert_eq!(settings.delivery_color.bit_depth, ColorBitDepth::Eight);

    assert_the_delivery_grade_clips(&gpu, &document);

    let lane = export_and_verify(
        &gpu,
        &directory,
        "cc6-eight-bit-delivery.mp4",
        &document,
        &settings,
        DeliveryEncodeDepth::Eight,
    );
    let margins = assert_encoded_delivery_lane(&lane, &gpu);
    assert_per_frame_full_resolution_reference(
        &gpu.context(),
        document.as_ref(),
        &settings,
        &lane.verification.comparison.frames,
    );
    emit_cc6_lane_evidence(
        "cc6_eight_bit_encoded_delivery_passes_tag_luma_and_difference_budgets",
        &gpu,
        &lane,
        margins,
        json!({}),
    );
}

/// §11.2.11 — **the exit gate**, 10-bit lane, plus the justification for the
/// lane existing at all.
#[test]
fn cc6_ten_bit_encoded_delivery_passes_tag_luma_and_difference_budgets() {
    crate::initialize_ffmpeg().expect("FFmpeg must initialize for the CC6 exit gate");
    // R5's cross-platform rule: the build's encoder is interrogated at
    // runtime, and a build without the lane's pixel format fails **typed**.
    // It never skips, so the first run on a new platform answers the question
    // in a red or green build rather than in silence.
    assert_libx264_advertises_the_ten_bit_lane();

    let gpu = fallback_gpu();
    let directory = TempDirectory::new("cc6-ten-bit-exit-gate");
    let size = CC6_DELIVERY_SOURCE_SIZE;
    let source = cc6_delivery_source(&directory, size, CC6_DELIVERY_SOURCE_FRAMES);
    let document = Arc::new(cc6_delivery_document(
        &source,
        size,
        CC6_DELIVERY_SOURCE_FRAMES,
    ));

    let ten_settings = cc6_delivery_settings(&document, DeliveryEncodeDepth::Ten);
    assert_eq!(ten_settings.delivery_color.bit_depth, ColorBitDepth::Ten);
    let ten = export_and_verify(
        &gpu,
        &directory,
        "cc6-ten-bit-delivery.mp4",
        &document,
        &ten_settings,
        DeliveryEncodeDepth::Ten,
    );
    let ten_margins = assert_encoded_delivery_lane(&ten, &gpu);
    assert_per_frame_full_resolution_reference(
        &gpu.context(),
        document.as_ref(),
        &ten_settings,
        &ten.verification.comparison.frames,
    );

    // The same source and the same frames through the 8-bit lane: the only
    // claim that makes the 10-bit lane worth having is that it is measurably
    // better, and it is measured rather than assumed.
    let eight_settings = cc6_delivery_settings(&document, DeliveryEncodeDepth::Eight);
    let eight = export_and_verify(
        &gpu,
        &directory,
        "cc6-ten-bit-gate-eight-bit-control.mp4",
        &document,
        &eight_settings,
        DeliveryEncodeDepth::Eight,
    );
    assert_eq!(
        eight.verification.comparison.frames,
        ten.verification.comparison.frames
    );

    // §6.3 words this comparison in **8-bit-equivalent** units, and that is
    // the unit `DeliveryComparison.combined.mean_code_diff_millionths` already
    // carries — `verify.rs` divides by `s = 2^(bits − 8)` where the histogram
    // is read, exactly as §6.3 words it, and PSNR has always been on the
    // 8-bit-equivalent MSE for the same reason. The two lanes are therefore
    // compared field to field, with no conversion in the fixture: a fixture
    // that had to convert would be evidence that the report itself was in the
    // wrong unit.
    let eight_rgb_mean = eight
        .verification
        .comparison
        .combined
        .mean_code_diff_millionths;
    let ten_rgb_mean = ten
        .verification
        .comparison
        .combined
        .mean_code_diff_millionths;
    let eight_psnr = eight
        .verification
        .comparison
        .psnr_db_hundredths
        .expect("a lossy 8-bit encode has a finite MSE");
    let ten_psnr = ten
        .verification
        .comparison
        .psnr_db_hundredths
        .expect("a lossy 10-bit encode has a finite MSE");
    println!(
        "CC6_TEN_BIT_JUSTIFICATION eight_rgb_mean_millionths_8bit_equiv={eight_rgb_mean} ten_rgb_mean_millionths_8bit_equiv={ten_rgb_mean} eight_psnr_hundredths={eight_psnr} ten_psnr_hundredths={ten_psnr}"
    );

    // Non-vacuity clause: an 8-bit lane measuring exactly zero would mean the
    // source never exercised the codec, and "10-bit is not worse than
    // nothing" is not a justification.
    assert_ne!(
        eight_rgb_mean, 0,
        "the source does not exercise the codec: the 8-bit lane's RGB mean is exactly 0, so the \
         10-bit comparison would be vacuous"
    );
    assert!(
        ten_rgb_mean < eight_rgb_mean,
        "the 10-bit lane must measure a strictly smaller 8-bit-equivalent RGB mean than the 8-bit \
         lane on the same source and frames: {ten_rgb_mean} vs {eight_rgb_mean}"
    );
    assert!(
        ten_psnr > eight_psnr,
        "the 10-bit lane must measure a strictly larger PSNR than the 8-bit lane on the same \
         source and frames: {ten_psnr} vs {eight_psnr}"
    );

    emit_cc6_lane_evidence(
        "cc6_ten_bit_encoded_delivery_passes_tag_luma_and_difference_budgets",
        &gpu,
        &ten,
        ten_margins,
        json!({
            "eight_bit_rgb_mean_code_millionths_8bit_equivalent": eight_rgb_mean,
            "ten_bit_rgb_mean_code_millionths_8bit_equivalent": ten_rgb_mean,
            "eight_bit_psnr_db_hundredths": eight_psnr,
            "ten_bit_psnr_db_hundredths": ten_psnr,
            "eight_bit_luma_max_code_diff": eight.verification.comparison.luma.maximum_code_diff,
            "ten_bit_luma_max_code_diff": ten.verification.comparison.luma.maximum_code_diff,
        }),
    );
}

/// R5's runtime encoder-format check: this build's libx264 must advertise the
/// 10-bit lane's pixel format, and a build that does not fails **typed**.
fn assert_libx264_advertises_the_ten_bit_lane() {
    use ffmpeg_next as ffmpeg;

    let codec = ffmpeg::encoder::find_by_name("libx264")
        .expect("the managed delivery encoder libx264 must be present in this build");
    let advertised = codec
        .video()
        .expect("libx264 is a video encoder")
        .formats()
        .map(std::iter::Iterator::collect::<Vec<_>>)
        .unwrap_or_default();
    if !advertised.contains(&ffmpeg::format::Pixel::YUV420P10LE) {
        let error = DeliveryColorError::EncoderPixelFormatUnavailable {
            observed: if advertised.is_empty() {
                "no advertised pixel formats".to_owned()
            } else {
                advertised
                    .iter()
                    .map(|format| format!("{format:?}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            },
            allowed: DeliveryEncodeDepth::Ten.pixel_format().to_owned(),
        };
        panic!(
            "{}: field {}, observed {}, allowed {} — this build cannot deliver the 10-bit lane. \
             CC6 §11.2.11 fails typed rather than skipping, so this is a red build, not silence.",
            error.code(),
            error.field(),
            error.observed(),
            error.allowed_values()
        );
    }
}

/// ~100 kb/s: the encode still succeeds and the file is still a valid
/// deliverable, which is the whole point of §6.5's outcome policy.
const CC6_STARVED_VIDEO_BITRATE: u64 = 100_000;

/// Every gated field name §6.3 can raise `decoded_difference_over_budget` on.
///
/// Declared once so a starved fixture asserts the **exact set** of terms that
/// tripped rather than "at least one of the five": a re-baseline that moved the
/// failure from the codec-only luma plane to the whole-raster sanity floor
/// would otherwise pass unnoticed, and the two mean very different things.
const CC6_GATED_BUDGET_FIELDS: [&str; 5] = [
    "luma.maximum_code_diff",
    "luma.p99_code_diff_millionths",
    "luma.mean_code_diff_millionths",
    "combined.mean_code_diff_millionths",
    "psnr_db_hundredths",
];

/// §11.2.13's shared body, on either lane: the same source and the same
/// production path at a bitrate that cannot carry it.
///
/// Returns the gated field names that actually tripped, in the order §10.2
/// sorts the exceptions into (severity, then field), so each lane's fixture can
/// pin its own set.
fn assert_starved_bitrate_direction(
    gpu: &FixtureGpu,
    depth: DeliveryEncodeDepth,
    directory_name: &str,
    output_name: &str,
) -> (LaneMeasurement, Vec<String>) {
    let directory = TempDirectory::new(directory_name);
    let size = CC6_DELIVERY_SOURCE_SIZE;
    let source = cc6_delivery_source(&directory, size, CC6_DELIVERY_SOURCE_FRAMES);
    let document = Arc::new(cc6_delivery_document(
        &source,
        size,
        CC6_DELIVERY_SOURCE_FRAMES,
    ));
    let mut settings = cc6_delivery_settings(&document, depth);
    settings.video_bitrate = CC6_STARVED_VIDEO_BITRATE;

    let lane = export_and_verify(gpu, &directory, output_name, &document, &settings, depth);
    let verification = &lane.verification;
    let comparison = &verification.comparison;
    println!(
        "CC6_STARVED_MEASURED lane={} depth={depth:?} bitrate={CC6_STARVED_VIDEO_BITRATE} luma_max={} luma_p99_millionths={} luma_mean_millionths={} rgb_mean_millionths_8bit_equiv={} psnr_hundredths={:?} rgb_max={} rgb_p99_millionths={}",
        gpu.lane.id(),
        comparison.luma.maximum_code_diff,
        comparison.luma.p99_code_diff_millionths,
        comparison.luma.mean_code_diff_millionths,
        comparison.combined.mean_code_diff_millionths,
        comparison.psnr_db_hundredths,
        comparison.combined.maximum_code_diff,
        comparison.combined.p99_code_diff_millionths,
    );

    assert_eq!(verification.delivery_bit_depth, depth);
    assert_eq!(verification.decoded_pixel_format, depth.pixel_format());
    assert!(
        !comparison.within_budgets,
        "a {CC6_STARVED_VIDEO_BITRATE} b/s encode of this source must not fit the {depth:?} lane's \
         budgets: {comparison:?}"
    );

    // Rule 11.0.4 on every raised exception, not just the first: code, field,
    // observed, and allowed.
    let mut tripped = Vec::new();
    for exception in &verification.exceptions {
        if exception.code != "decoded_difference_over_budget" {
            continue;
        }
        assert_eq!(exception.severity, QaSeverity::Error);
        let field = exception
            .field
            .clone()
            .unwrap_or_else(|| panic!("a budget exception names its field: {exception:?}"));
        assert!(
            CC6_GATED_BUDGET_FIELDS.contains(&field.as_str()),
            "unexpected budget field {field}"
        );
        assert!(exception.observed.is_some(), "{exception:?}");
        assert!(exception.allowed.is_some(), "{exception:?}");
        assert!(
            exception.message.contains("never moves the file")
                || exception.message.contains("PSNR"),
            "a budget message states what it measured: {exception:?}"
        );
        tripped.push(field);
    }
    assert!(
        !tripped.is_empty(),
        "a starved encode reports the budget it broke: {:?}",
        verification.exceptions
    );
    assert!(!verification.technical_pass);

    // The measurement never moves the file it measured: the encode is still
    // where the export wrote it, and it is still a readable delivery.
    assert_eq!(verification.output_path, lane.output);
    assert!(
        lane.output.is_file(),
        "a budget overrun must leave the finished encode at its original path"
    );
    assert_eq!(
        probe_path(&lane.output, AssetId(7))
            .expect("the starved export still probes")
            .color_description
            .bit_depth,
        match depth {
            DeliveryEncodeDepth::Eight => ColorBitDepth::Eight,
            DeliveryEncodeDepth::Ten => ColorBitDepth::Ten,
        }
    );

    // The tags are still right: this is a *difference* failure, not a tag
    // failure, and the two are reported separately.
    assert!(verification.tags.conforming, "{:?}", verification.tags);

    println!(
        "CC6_STARVED_TRIPPED lane={} depth={depth:?} fields={tripped:?}",
        gpu.lane.id()
    );
    (lane, tripped)
}

/// One starved lane's evidence payload.
fn emit_cc6_starved_evidence(
    fixture: &str,
    gpu: &FixtureGpu,
    lane: &LaneMeasurement,
    tripped: &[String],
) {
    let comparison = &lane.verification.comparison;
    emit_cc6_evidence(
        fixture,
        gpu,
        json!({
            "video_bitrate": CC6_STARVED_VIDEO_BITRATE,
            "delivery_bit_depth": lane.depth.bits(),
        }),
        json!({
            "within_budgets": comparison.within_budgets,
            "technical_pass": lane.verification.technical_pass,
            "luma_max_code_diff": comparison.luma.maximum_code_diff,
            "luma_p99_code_millionths": comparison.luma.p99_code_diff_millionths,
            "luma_mean_code_millionths": comparison.luma.mean_code_diff_millionths,
            "rgb_mean_code_millionths_8bit_equivalent":
                comparison.combined.mean_code_diff_millionths,
            "psnr_db_hundredths": comparison.psnr_db_hundredths,
            "budgets": comparison.budgets,
            "over_budget_fields": tripped,
            "export_seconds": lane.export_seconds,
            "verify_seconds": lane.verify_seconds,
        }),
    );
}

/// §11.2.13 — the failing direction of the exit gate, 8-bit lane: the same
/// source, the same production path, at a bitrate that cannot carry it.
#[test]
fn cc6_starved_bitrate_export_trips_the_decoded_difference_budget() {
    crate::initialize_ffmpeg().expect("FFmpeg must initialize for the starved-bitrate fixture");
    let gpu = fallback_gpu();
    let (lane, tripped) = assert_starved_bitrate_direction(
        &gpu,
        DeliveryEncodeDepth::Eight,
        "cc6-starved-bitrate",
        "cc6-starved-bitrate.mp4",
    );

    // The **codec-only** terms are what a starved codec breaks, and after the
    // §6.3 re-baseline they are the only ones that do on this lane: at
    // 100 kb/s the luma plane measures 35 codes against a budget of 8, a P99
    // of 6.0 against 2.0, and a mean of 0.621 against 0.4, while the
    // whole-raster RGB mean (1.330 of 1.5 8-bit-equivalent codes) and PSNR
    // (35.88 dB against a 33.00 dB floor) stay inside their sanity floors —
    // those two are dominated by 4:2:0 chroma decimation, which starving the
    // bitrate barely moves. A fixture that accepted "any one of the five"
    // would not have noticed which half of §6.3 actually caught the defect.
    assert_eq!(
        tripped,
        vec![
            "luma.maximum_code_diff".to_owned(),
            "luma.mean_code_diff_millionths".to_owned(),
            "luma.p99_code_diff_millionths".to_owned(),
        ],
        "the 8-bit starved direction must trip on the codec-only luma terms"
    );

    emit_cc6_starved_evidence(
        "cc6_starved_bitrate_export_trips_the_decoded_difference_budget",
        &gpu,
        &lane,
        &tripped,
    );
}

/// §11.2.13 on the **Ten** lane: the failing direction of §11.2.11's exit gate.
///
/// The 10-bit lane has its own separately baselined budgets, so "the 8-bit
/// starved case fails" says nothing about whether the 10-bit budgets can fail
/// at all. This is that lane's failing direction (rule 11.0.5), measured the
/// same way on the same source.
#[test]
fn cc6_starved_bitrate_ten_bit_export_trips_the_decoded_difference_budget() {
    crate::initialize_ffmpeg().expect("FFmpeg must initialize for the starved-bitrate fixture");
    assert_libx264_advertises_the_ten_bit_lane();
    let gpu = fallback_gpu();
    let (lane, tripped) = assert_starved_bitrate_direction(
        &gpu,
        DeliveryEncodeDepth::Ten,
        "cc6-starved-bitrate-ten",
        "cc6-starved-bitrate-ten.mp4",
    );

    // All three luma terms, and — unlike the 8-bit lane — the RGB mean too.
    // The 10-bit lane's sanity floor is 1.0 8-bit-equivalent codes rather than
    // 1.5, because its *healthy* measurement is 0.415 rather than 0.744; a
    // starved 10-bit encode measures 1.181 and crosses it. PSNR (35.98 dB
    // against the same 33.00 dB floor) still does not trip, so the two halves
    // of §6.3's sanity floor are not redundant with each other either.
    assert_eq!(
        tripped,
        vec![
            "combined.mean_code_diff_millionths".to_owned(),
            "luma.maximum_code_diff".to_owned(),
            "luma.mean_code_diff_millionths".to_owned(),
            "luma.p99_code_diff_millionths".to_owned(),
        ],
        "the 10-bit starved direction must trip on the codec-only luma terms"
    );

    emit_cc6_starved_evidence(
        "cc6_starved_bitrate_ten_bit_export_trips_the_decoded_difference_budget",
        &gpu,
        &lane,
        &tripped,
    );
}

// ===========================================================================
// Evidence.
// ===========================================================================

/// Every fixture in this file that emits a `CC6_EVIDENCE` payload.
///
/// Declared rather than free-form so a payload cannot appear under a name the
/// manifest does not list, which is how CC5 keeps its evidence auditable.
const CC6_EVIDENCE_FIXTURES: [&str; 7] = [
    "cc6_eight_bit_encoded_delivery_passes_tag_luma_and_difference_budgets",
    "cc6_ten_bit_encoded_delivery_passes_tag_luma_and_difference_budgets",
    "cc6_starved_bitrate_export_trips_the_decoded_difference_budget",
    "cc6_starved_bitrate_ten_bit_export_trips_the_decoded_difference_budget",
    "cc6_per_node_contribution_order_matches_production_z_order",
    "cc6_performance_evidence_is_recorded_on_software_fallback",
    "cc6_performance_evidence_is_recorded_on_hardware",
];

fn emit_cc6_evidence(fixture: &str, gpu: &FixtureGpu, controls: Value, metrics: Value) {
    assert!(
        CC6_EVIDENCE_FIXTURES.contains(&fixture),
        "every CC6 evidence payload must be declared in CC6_EVIDENCE_FIXTURES and in the \
         manifest; {fixture} is not"
    );
    let provenance = backend_metadata(gpu.backend());
    let field = |key: &str| provenance.get(key).cloned().unwrap_or(Value::Null);
    let payload = json!({
        "contract": CC6_CONTRACT,
        "fixture": fixture,
        "lane": gpu.lane.id(),
        "git_revision": git_revision(),
        "backend": gpu.backend(),
        "backend_name": field("backend"),
        "adapter": field("adapter"),
        "software_fallback": field("software_fallback"),
        "gpu_claim": field("gpu_claim"),
        "backend_lane": field("lane"),
        "backend_metadata": provenance,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "controls": controls,
        "metrics": metrics,
    });
    println!("CC6_EVIDENCE {payload}");
    write_evidence_artefact(fixture, &payload);
}

/// The evidence payload of one encoded lane: every measured number §11.2.10
/// and §11.2.11 record, plus the margins and the output hash.
fn emit_cc6_lane_evidence(
    fixture: &str,
    gpu: &FixtureGpu,
    lane: &LaneMeasurement,
    margins: LaneMargins,
    extra: Value,
) {
    let verification = &lane.verification;
    let comparison = &verification.comparison;
    let ratio = |value: f64| {
        if value.is_finite() {
            json!(value)
        } else {
            Value::String("infinite (measured exactly zero)".to_owned())
        }
    };
    let channel = |difference: &kinewright_core::DeliveryChannelDifference| {
        json!({
            "maximum_code_diff": difference.maximum_code_diff,
            "p99_code_diff_millionths": difference.p99_code_diff_millionths,
            "mean_code_diff_millionths": difference.mean_code_diff_millionths,
        })
    };
    let plane = |excursion: &kinewright_core::PlaneLegalExcursion| {
        json!({
            "below_count": excursion.below_count,
            "above_count": excursion.above_count,
            "below_basis_points": excursion.below_basis_points,
            "above_basis_points": excursion.above_basis_points,
            "minimum_code_hundredths": excursion.minimum_code_hundredths,
            "maximum_code_hundredths": excursion.maximum_code_hundredths,
        })
    };
    let mut metrics = json!({
        "delivery_bit_depth": match lane.depth {
            DeliveryEncodeDepth::Eight => 8,
            DeliveryEncodeDepth::Ten => 10,
        },
        "decoded_pixel_format": verification.decoded_pixel_format,
        "sampled_frames": comparison.frames,
        "luma": channel(&comparison.luma),
        "red": channel(&comparison.red),
        "green": channel(&comparison.green),
        "blue": channel(&comparison.blue),
        // `maximum_code_diff` and `p99_code_diff_millionths` are in lane code
        // units; `mean_code_diff_millionths` is 8-bit-equivalent (§6.3).
        "combined": channel(&comparison.combined),
        "psnr_db_hundredths": comparison.psnr_db_hundredths,
        "within_budgets": comparison.within_budgets,
        "technical_pass": verification.technical_pass,
        "budgets": comparison.budgets,
        "margin_ratio": {
            "luma_max": ratio(margins.luma_max),
            "luma_p99": ratio(margins.luma_p99),
            "luma_mean": ratio(margins.luma_mean),
            "rgb_mean": ratio(margins.rgb_mean),
        },
        "psnr_headroom_db_hundredths": margins.psnr_headroom_hundredths,
        "decoded_ycbcr": {
            "bit_depth": comparison.decoded_ycbcr.bit_depth,
            "luma": plane(&comparison.decoded_ycbcr.luma),
            "cb": plane(&comparison.decoded_ycbcr.cb),
            "cr": plane(&comparison.decoded_ycbcr.cr),
        },
        "export_seconds": lane.export_seconds,
        "verify_seconds": lane.verify_seconds,
        "output_hash_sha256": file_hash(&lane.output),
        "source_raster": {
            "width": CC6_DELIVERY_SOURCE_SIZE.0,
            "height": CC6_DELIVERY_SOURCE_SIZE.1,
            "frames": CC6_DELIVERY_SOURCE_FRAMES,
            "fps": CC6_DELIVERY_SOURCE_FPS,
        },
    });
    if let (Some(metrics), Some(extra)) = (metrics.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            metrics.insert(key.clone(), value.clone());
        }
    }
    emit_cc6_evidence(
        fixture,
        gpu,
        json!({
            "grade": "color_wheels gain 1.8 in one matte window, color_wheels lift -0.16 in another",
            "video_bitrate": 20_000_000,
            "delivery_profile": "source_master",
        }),
        metrics,
    );
}

// ===========================================================================
// §11.2.14 (media half): document order against production z-order.
// ===========================================================================

/// §11.2.14. Core's candidate ordering — document track order, then clip order
/// within a track, then effect-chain order within a clip — is asserted equal
/// to `visual_layers_at`'s production z-order on a three-track document.
///
/// Core cannot make this assertion itself: `kinewright-core` must not depend
/// on `kinewright-media`, so `color_qc/nodes.rs` states the ordering and
/// leaves the agreement to be proven here. Without it the two orders could
/// drift and every per-node attribution would name the wrong node while every
/// core test still passed.
///
/// The numeric half of §11.2.14 — the hand-computed deltas, the inactive node,
/// the seventeen-candidate truncation, and the byte-identical document — is
/// `cc6_core.rs`'s, because that arithmetic is core's and needs no GPU.
#[test]
fn cc6_per_node_contribution_order_matches_production_z_order() {
    use kinewright_core::{Clip, ClipContent, Track, TrackId, TrackKind};

    let gpu = fallback_gpu();
    let directory = TempDirectory::new("cc6-node-order");
    let size = CC6_DELIVERY_SOURCE_SIZE;
    let source = cc6_delivery_source(&directory, size, CC6_DELIVERY_SOURCE_FRAMES);
    let base = cc6_delivery_document(&source, size, CC6_DELIVERY_SOURCE_FRAMES);
    let asset = base.media_pool[0].clone();

    // Three video tracks, each carrying one clip of the same asset, with two
    // colour nodes on the bottom clip, one on the middle, and two on the top.
    let mut document = base.clone();
    document.tracks = (0..3_u64)
        .map(|track| Track {
            id: TrackId(track + 1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![Clip {
                id: ClipId(track + 1),
                asset: asset.id,
                source_range: TimeCode::ZERO..asset.duration,
                content: ClipContent::Media,
                timeline_start: TimeCode::ZERO,
                effects: match track {
                    0 => vec![
                        effect_with(1, "primary_correction", &[("exposure_milli_stops", 100)]),
                        effect_with(2, "color_wheels", &[("gain_master_thousandths", 1_100)]),
                    ],
                    1 => vec![effect_with(
                        3,
                        "primary_correction",
                        &[("contrast_percent", 5)],
                    )],
                    _ => vec![
                        effect_with(4, "color_wheels", &[("lift_master_basis_points", -100)]),
                        effect_with(5, "primary_correction", &[("saturation_percent", -10)]),
                    ],
                },
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
            }],
        })
        .collect();
    document
        .validate()
        .expect("the three-track ordering document should validate");

    // Production z-order, bottom to top, with each layer's effect chain.
    let layers = crate::timeline::visual_layers_at(&document, TimeCode::ZERO)
        .expect("the production z-order resolves");
    assert_eq!(layers.len(), 3, "one visible clip on each of three tracks");
    let production: Vec<(u64, u64)> = layers
        .iter()
        .flat_map(|layer| {
            let (clip, effects) = match layer {
                crate::timeline::TimelineVisualLayer::Video(video) => {
                    (video.source.clip, &video.effects)
                }
                crate::timeline::TimelineVisualLayer::Title(title) => (title.clip, &title.effects),
            };
            effects
                .iter()
                .map(move |effect| (clip.0, effect.id.0))
                .collect::<Vec<_>>()
        })
        .collect();

    // Core's own order, through the production entry point.
    let engine = crate::engine::FfmpegMediaEngine::new_with_gpu(gpu.context())
        .expect("the production media engine should start");
    let contributions = kinewright_core::nodes::measure_node_contributions(
        &engine,
        Arc::new(document.clone()),
        TimeCode::ZERO,
        &ColorQcRequest::default(),
    )
    .expect("the per-node attribution renders");
    let core_order: Vec<(u64, u64)> = contributions
        .nodes
        .iter()
        .map(|node| (node.clip.0, node.effect.0))
        .collect();

    assert_eq!(
        core_order, production,
        "core's document order and `visual_layers_at`'s production z-order must agree, or every \
         per-node attribution names the wrong node"
    );
    assert_eq!(
        core_order,
        vec![(1, 1), (1, 2), (2, 3), (3, 4), (3, 5)],
        "hand-written: track 1 then 2 then 3, effect-chain order within each clip"
    );
    assert_eq!(contributions.considered_node_count, 5);
    assert!(!contributions.truncated);

    // Failing direction: a document whose tracks are reversed produces the
    // reversed order on *both* sides, so the assertion above is comparing two
    // live orders rather than one order against itself.
    let mut reversed = document.clone();
    reversed.tracks.reverse();
    reversed
        .validate()
        .expect("the reversed document should validate");
    let reversed_production: Vec<u64> =
        crate::timeline::visual_layers_at(&reversed, TimeCode::ZERO)
            .expect("the production z-order resolves")
            .iter()
            .map(|layer| match layer {
                crate::timeline::TimelineVisualLayer::Video(video) => video.source.clip.0,
                crate::timeline::TimelineVisualLayer::Title(title) => title.clip.0,
            })
            .collect();
    assert_eq!(reversed_production, vec![3, 2, 1]);
    let reversed_core = kinewright_core::nodes::measure_node_contributions(
        &engine,
        Arc::new(reversed),
        TimeCode::ZERO,
        &ColorQcRequest::default(),
    )
    .expect("the per-node attribution renders");
    let reversed_core_order: Vec<(u64, u64)> = reversed_core
        .nodes
        .iter()
        .map(|node| (node.clip.0, node.effect.0))
        .collect();
    assert_eq!(
        reversed_core_order,
        vec![(3, 4), (3, 5), (2, 3), (1, 1), (1, 2)]
    );
    assert_ne!(reversed_core_order, core_order);

    emit_cc6_evidence(
        "cc6_per_node_contribution_order_matches_production_z_order",
        &gpu,
        json!({"tracks": 3, "nodes": 5}),
        json!({
            "core_order": core_order,
            "production_order": production,
            "reversed_core_order": reversed_core_order,
            "considered_node_count": contributions.considered_node_count,
            "attribution": contributions.attribution,
        }),
    );
}

// ===========================================================================
// §11.2.22: one delivery transfer, two crates.
// ===========================================================================

/// §11.2.22. `kinewright_core::color_qc::encode_bt709_delivery` and
/// `kinewright_media::color_pipeline::encode_bt709` agree on `to_bits()` for
/// the §3.2 anchors and for a dense sweep of `−2.0 ..= 2.0`.
///
/// Two crates carrying the same transfer is a duplication CC6 accepted (core
/// cannot depend on media); this fixture is the price of that decision. The
/// failing direction proves the sweep can see a one-branch error.
#[test]
fn cc6_core_delivery_transfer_is_bit_identical_to_the_media_transfer() {
    use crate::color_pipeline::encode_bt709;
    use kinewright_core::encode_bt709_delivery;

    // The ten §3.2 anchors, transcribed from the contract's table.
    const ANCHORS: [f32; 10] = [
        -1.0,
        -0.02,
        0.0,
        0.001,
        0.017_999_999,
        0.018,
        0.18,
        0.5,
        1.0,
        1.05,
    ];
    for anchor in ANCHORS {
        assert_eq!(
            encode_bt709_delivery(anchor).to_bits(),
            encode_bt709(anchor).to_bits(),
            "the two delivery transfers disagree at {anchor}"
        );
    }
    // `e(1.0) == 1.0` exactly, and it is the value the strict `>` test does
    // not count.
    assert_eq!(encode_bt709_delivery(1.0), 1.0);
    assert_eq!(encode_bt709(1.0), 1.0);

    // A dense sweep in steps of 1/4096, including both sides of the 0.018
    // seam and both signs.
    let mut compared = 0_u32;
    let mut step = -2.0_f32 * 4_096.0;
    while step <= 2.0 * 4_096.0 {
        let value = step / 4_096.0;
        assert_eq!(
            encode_bt709_delivery(value).to_bits(),
            encode_bt709(value).to_bits(),
            "the two delivery transfers disagree at {value}"
        );
        compared += 1;
        step += 1.0;
    }
    assert_eq!(compared, 16_385, "the sweep covers -2.0 ..= 2.0 at 1/4096");

    // Failing direction: a deliberately mis-seamed transcription (`<=` rather
    // than `<` at the branch) differs at exactly 0.018, so the sweep is known
    // to be able to see a one-branch error.
    fn mis_seamed(linear: f32) -> f32 {
        if linear < 0.0 {
            -mis_seamed(-linear)
        } else if linear <= 0.018 {
            4.5 * linear
        } else {
            1.099 * linear.powf(0.45) - 0.099
        }
    }
    assert_ne!(
        mis_seamed(0.018).to_bits(),
        encode_bt709_delivery(0.018).to_bits(),
        "the mis-seamed transcription must differ at the seam, or the sweep proves nothing"
    );
    // ... and agrees everywhere else on the anchors, so the difference is the
    // branch and not a second bug.
    for anchor in ANCHORS {
        if anchor == 0.018 {
            continue;
        }
        assert_eq!(
            mis_seamed(anchor).to_bits(),
            encode_bt709_delivery(anchor).to_bits(),
            "the mis-seamed control differs away from the seam at {anchor}"
        );
    }
}

// ===========================================================================
// §11.2.24 (P9): performance evidence.
// ===========================================================================

/// The P9 raster: one 1920 × 1080 frame is what §11.2.24 costs, so the cost is
/// measured at that raster and not extrapolated from 320 × 180.
const CC6_PERFORMANCE_RASTER: (u32, u32) = (1_920, 1_080);
/// Enough frames for the default five-frame sample and no more: what P9
/// measures is the per-verification cost, not the sampling rule.
const CC6_PERFORMANCE_FRAMES: u32 = 8;
/// §11.2.24's soft budget: one 24 fps frame, in milliseconds. Recorded
/// evidence, **not** a hard gate — but a regression must be visible.
const CC6_QC_SOFT_BUDGET_MILLISECONDS: f64 = 41.666_666_666_666_664;

/// §11.2.24 / P9. The wall time of one 1920 × 1080 working proof plus a full
/// `ColorQcReport`, and of a five-frame `verify_delivery_output`, recorded on
/// one lane.
///
/// This measurement is taken **before** `verify`'s default is final (§12 step
/// 5), which is why it is a fixture and not a comment.
fn assert_cc6_performance_evidence(gpu: &FixtureGpu, fixture: &str) {
    crate::initialize_ffmpeg().expect("FFmpeg must initialize for the P9 measurement");
    let directory = TempDirectory::new("cc6-performance");
    let size = CC6_PERFORMANCE_RASTER;
    let source = cc6_delivery_source(&directory, size, CC6_PERFORMANCE_FRAMES);
    let document = Arc::new(cc6_delivery_document(&source, size, CC6_PERFORMANCE_FRAMES));
    let settings = cc6_delivery_settings(&document, DeliveryEncodeDepth::Eight);
    let engine = crate::engine::FfmpegMediaEngine::new_with_gpu(gpu.context())
        .expect("the production media engine should start");

    // --- the working proof ------------------------------------------------
    let started = Instant::now();
    let proof = engine
        .working_proof_for_document(Arc::clone(&document), TimeCode::ZERO)
        .expect("the production working proof renders at 1080p");
    let proof_milliseconds = started.elapsed().as_secs_f64() * 1_000.0;
    assert_eq!((proof.image.width, proof.image.height), size);
    assert!(proof.metadata.render.full_resolution);

    // --- a full report: range + gamut + skin + tags -----------------------
    let request = ColorQcRequest {
        roi: Some(NormalizedRoi::new(200, 4_500, 1_000, 1_000)),
        checks: vec![
            ColorQcCheck::Range,
            ColorQcCheck::Gamut,
            ColorQcCheck::Skin,
            ColorQcCheck::Tags,
        ],
        expected_delivery: Some(settings.delivery_color.clone()),
        ..ColorQcRequest::default()
    };
    let started = Instant::now();
    let report = measure_color_qc(&proof, &request).expect("the 1080p proof measures");
    let qc_milliseconds = started.elapsed().as_secs_f64() * 1_000.0;
    assert!(report.full_resolution);
    assert_eq!(report.raster, size);
    assert!(report.skin.is_some(), "the skin section was requested");
    assert!(report.tags.is_some(), "the tag section was requested");

    // --- a five-frame verification ---------------------------------------
    let output = directory.path("cc6-performance-delivery.mp4");
    let (progress_tx, _progress_rx) = crossbeam_channel::unbounded();
    let started = Instant::now();
    crate::export::export_document(
        document.as_ref(),
        &output,
        &settings,
        &progress_tx,
        gpu.context(),
    )
    .expect("the production export writes the P9 delivery");
    let export_milliseconds = started.elapsed().as_secs_f64() * 1_000.0;
    let request = DeliveryVerificationRequest::new(
        DeliveryEncodeDepth::Eight,
        settings.delivery_color.clone(),
    );
    let sampled = request.sample_frames(u64::from(CC6_PERFORMANCE_FRAMES));
    assert_eq!(
        sampled.len(),
        usize::from(DELIVERY_VERIFICATION_FRAME_COUNT)
    );
    let started = Instant::now();
    let verification = engine
        .verify_delivery_output(Arc::clone(&document), &output, &settings, request)
        .expect("the P9 delivery verifies");
    let verify_milliseconds = started.elapsed().as_secs_f64() * 1_000.0;
    assert_eq!(verification.comparison.frames.len(), sampled.len());

    println!(
        "CC6_PERFORMANCE lane={} raster={}x{} working_proof_ms={proof_milliseconds:.1} color_qc_ms={qc_milliseconds:.1} proof_plus_qc_ms={:.1} qc_soft_budget_ms={CC6_QC_SOFT_BUDGET_MILLISECONDS:.1} export_ms={export_milliseconds:.1} verify_five_frames_ms={verify_milliseconds:.1} verify_per_frame_ms={:.1}",
        gpu.lane.id(),
        size.0,
        size.1,
        proof_milliseconds + qc_milliseconds,
        verify_milliseconds / f64::from(DELIVERY_VERIFICATION_FRAME_COUNT),
    );
    if qc_milliseconds > CC6_QC_SOFT_BUDGET_MILLISECONDS {
        println!(
            "CC6_PERFORMANCE_SOFT_BUDGET lane={} color_qc_ms={qc_milliseconds:.1} exceeds the one-frame soft budget of {CC6_QC_SOFT_BUDGET_MILLISECONDS:.1} ms; recorded, not gated",
            gpu.lane.id()
        );
    }
    // Vacuity: a measurement of exactly zero would mean the clock, not the
    // cost, was measured.
    assert!(proof_milliseconds > 0.0);
    assert!(qc_milliseconds > 0.0);
    assert!(verify_milliseconds > 0.0);

    emit_cc6_evidence(
        fixture,
        gpu,
        json!({
            "raster": {"width": size.0, "height": size.1},
            "frames": CC6_PERFORMANCE_FRAMES,
            "checks": ["range", "gamut", "skin", "tags"],
            "verification_frame_count": DELIVERY_VERIFICATION_FRAME_COUNT,
        }),
        json!({
            "working_proof_milliseconds": proof_milliseconds,
            "color_qc_milliseconds": qc_milliseconds,
            "working_proof_plus_color_qc_milliseconds": proof_milliseconds + qc_milliseconds,
            "color_qc_soft_budget_milliseconds": CC6_QC_SOFT_BUDGET_MILLISECONDS,
            "color_qc_within_soft_budget": qc_milliseconds <= CC6_QC_SOFT_BUDGET_MILLISECONDS,
            "export_milliseconds": export_milliseconds,
            "verify_five_frames_milliseconds": verify_milliseconds,
            "verify_per_sampled_frame_milliseconds":
                verify_milliseconds / f64::from(DELIVERY_VERIFICATION_FRAME_COUNT),
            "sampled_frames": verification.comparison.frames,
            "verification_frame_count_default": DELIVERY_VERIFICATION_FRAME_COUNT,
            "verification_frame_count_maximum": DELIVERY_VERIFICATION_MAX_FRAMES,
        }),
    );
}

#[test]
fn cc6_performance_evidence_is_recorded_on_software_fallback() {
    assert_cc6_performance_evidence(
        &fallback_gpu(),
        "cc6_performance_evidence_is_recorded_on_software_fallback",
    );
}

#[test]
#[ignore = "requires a physical supported GPU; run explicitly with --ignored --nocapture"]
fn cc6_performance_evidence_is_recorded_on_hardware() {
    assert_cc6_performance_evidence(
        &hardware_gpu(),
        "cc6_performance_evidence_is_recorded_on_hardware",
    );
}

// ===========================================================================
// §11.2.23: the manifest and the declared-test inventory.
// ===========================================================================

/// Every `cc6_*` test the **media** crate declares, across the three files
/// that own CC6 evidence.
///
/// They all carry the `cc6_` prefix so `cargo test -p kinewright-media -- cc6`
/// really runs the whole slice.
const CC6_MEDIA_TESTS: [&str; 25] = [
    // cc6_fixtures.rs
    "cc6_qc_raster_populations_are_the_contract_table",
    "cc6_delivery_source_moves_the_pinned_element_across_the_sampled_frames",
    "cc6_eight_bit_encoded_delivery_passes_tag_luma_and_difference_budgets",
    "cc6_ten_bit_encoded_delivery_passes_tag_luma_and_difference_budgets",
    "cc6_starved_bitrate_export_trips_the_decoded_difference_budget",
    "cc6_starved_bitrate_ten_bit_export_trips_the_decoded_difference_budget",
    "cc6_per_node_contribution_order_matches_production_z_order",
    "cc6_core_delivery_transfer_is_bit_identical_to_the_media_transfer",
    "cc6_performance_evidence_is_recorded_on_software_fallback",
    "cc6_performance_evidence_is_recorded_on_hardware",
    "cc6_manifest_declares_every_required_fixture_and_constant",
    "cc6_declared_test_names_exist_in_their_source_files",
    // compositor.rs, next to the working surface it measures.
    "cc6_working_proof_matches_the_cpu_reference_on_the_software_lane",
    "cc6_working_proof_matches_the_cpu_reference_on_hardware",
    "cc6_working_proof_refuses_a_claim_that_is_not_full_resolution",
    // verify.rs, next to the decode path it measures.
    "cc6_decoded_native_planes_report_ycbcr_excursions_in_delivery_code_units",
    "cc6_delivery_verification_plane_out_of_container_is_typed",
    "cc6_eight_bit_export_verifies_end_to_end_through_the_production_surface",
    "cc6_verification_refuses_an_export_whose_edit_list_drops_the_last_frame",
    "cc6_delivery_reference_denominator_is_the_delivery_intermediate_white",
    "cc6_delivery_budgets_are_distinct_from_the_compositor_gate",
    "cc6_an_unseen_plane_reports_the_empty_interval_and_an_empty_sample_set_is_refused",
    "cc6_verification_refuses_budgets_from_the_other_delivery_lane",
    "cc6_verification_refuses_a_frame_count_the_sampler_would_have_clamped",
    "cc6_a_decoded_frame_that_is_not_the_requested_frame_is_refused",
];

/// The media files that own a `cc6_*` test.
const CC6_MEDIA_TEST_SOURCES: [&str; 3] = [
    "crates/kinewright-media/src/cc6_fixtures.rs",
    "crates/kinewright-media/src/compositor.rs",
    "crates/kinewright-media/src/verify.rs",
];

/// The CC6 tests `export.rs` owns. They are **not** `cc6_`-prefixed because
/// they are the delivery gate's own unit tests, written where the gate lives;
/// the inventory names them explicitly, as CC5 did for its compositor tests.
const CC6_EXPORT_TESTS: [&str; 10] = [
    "accepts_the_ten_bit_sdr_rec709_delivery_contract",
    "rejects_a_delivery_depth_outside_the_two_managed_lanes",
    "delivery_lane_pixel_format_matches_the_core_lane_names",
    "libx264_advertises_both_delivery_lane_pixel_formats",
    "rejects_a_pixel_format_that_does_not_carry_the_declared_delivery_depth",
    "rejects_a_delivery_pixel_format_this_build_does_not_advertise",
    "delivery_filter_converts_sixteen_bit_full_range_rgb_to_limited_yuv420p10le",
    "delivery_nominal_white_encodes_to_legal_white_through_the_export_filter",
    "ten_bit_export_probes_as_rec709_limited_ten_bit_yuv420p10le",
    "every_exported_frame_is_presented_after_the_mp4_edit_list",
];

/// Every `cc6_*` test `crates/kinewright-core/tests/cc6_core.rs` declares.
const CC6_CORE_TESTS: [&str; 20] = [
    "cc6_range_anchors_match_the_hand_derived_delivery_encode",
    "cc6_negative_range_anchors_take_the_power_branch",
    "cc6_gamut_and_range_under_describe_the_same_pixel_set",
    "cc6_bt709_forward_ycbcr_matches_the_spec_at_eight_and_ten_bits",
    "cc6_bt709_limited_ycbcr_refuses_a_depth_that_is_not_a_delivery_lane",
    "cc6_ycbcr_legal_bounds_are_strict_through_the_measurement",
    "cc6_skin_band_constants_are_derived_from_the_cc5_patches",
    "cc6_skin_diagnostics_report_circular_statistics_on_a_chosen_region",
    "cc6_working_proof_refuses_a_claim_that_is_not_full_resolution",
    "cc6_delivery_tag_check_covers_both_modes_and_marks_white_point_not_representable",
    "cc6_per_node_contribution_attributes_clipping_to_the_node_that_causes_it",
    "cc6_per_node_candidates_find_the_on_screen_clip_whatever_the_clip_order",
    "cc6_typed_qc_refusals_carry_code_field_observed_and_allowed",
    "cc6_non_finite_samples_are_counted_and_never_classified",
    "cc6_exceptions_sort_by_severity_then_code_then_field",
    "cc6_delivery_verification_sampling_is_the_closed_form_integer_rule",
    "cc6_delivery_verification_refuses_a_frame_count_outside_the_sampled_range",
    "cc6_export_settings_and_job_records_serialize_deterministically",
    "cc6_qc_refusals_keep_their_code_through_media_error",
    "cc6_plane_excursion_basis_points_count_both_directions",
];

/// Every `cc6_*` test the agent crate declares.
const CC6_AGENT_TESTS: [&str; 10] = [
    "cc6_a_cancel_before_verification_records_why_the_file_is_unmeasured",
    "cc6_get_color_qc_is_evidence_only_and_revision_gated",
    "cc6_video_scopes_v2_points_at_get_color_qc_instead_of_a_fabricated_zero",
    "cc6_a_verified_export_publishes_its_decoded_comparison_on_the_record",
    "cc6_a_failing_verification_completes_the_job_and_leaves_the_output_alone",
    "cc6_a_panicking_verification_is_contained_and_leaves_the_output_alone",
    "cc6_an_unavailable_verification_records_its_reason_instead_of_a_pass",
    "cc6_cancelling_during_a_verification_leaves_the_record_cancelled_and_unverified",
    "cc6_verify_false_skips_the_measurement_and_serializes_byte_identically",
    "cc6_a_pre_cc6_job_record_deserializes_with_the_eight_bit_lane_and_no_verification",
];

/// Every `cc6_*` test the app crate declares.
const CC6_APP_TESTS: [&str; 9] = [
    "cc6_a_panicking_verification_is_contained_and_the_encode_is_still_reported",
    "cc6_qc_mask_marks_only_the_flagged_pixels",
    "cc6_scopes_panel_renders_absolute_per_channel_clipping",
    "cc6_export_dialog_reports_the_verification_result",
    "cc6_export_dialog_and_queue_agree_on_delivery_color",
    "cc6_verification_block_reports_every_probed_tag_field",
    "cc6_the_dialog_names_the_verifying_stage_instead_of_freezing_the_bar",
    "cc6_cancelling_before_verification_reports_not_verified_with_the_reason",
    "cc6_conformance_cache_does_not_cross_delivery_lanes",
];

/// The two §11.2.23 inventory tests, which are fixture-quality rules rather
/// than numbered §11.2 items and are therefore claimed by `manifest_self_test`
/// rather than by a numbered fixture.
const CC6_INVENTORY_TESTS: [&str; 2] = [
    "cc6_manifest_declares_every_required_fixture_and_constant",
    "cc6_declared_test_names_exist_in_their_source_files",
];

/// The §11.2 items whose evidence lives outside this crate.
const CC6_EXTERNAL_OWNERS: [(u64, &str); 14] = [
    (1, "kinewright-core"),
    (2, "kinewright-core"),
    (3, "kinewright-core"),
    (4, "kinewright-core"),
    (5, "kinewright-core"),
    (9, "kinewright-core"),
    // §6.4's excursion **rate** is core's accessor, and its own proof lives
    // beside it; the decoded planes it is measured on are this crate's.
    (12, "kinewright-core"),
    (15, "kinewright-core"),
    (16, "kinewright-agent"),
    (17, "kinewright-core"),
    (18, "kinewright-agent"),
    (19, "kinewright-app"),
    (20, "kinewright-app"),
    (21, "kinewright-app"),
];

/// The sources every declared CC6 test name is verified against, keyed by the
/// workspace-relative path the manifest names.
///
/// `include_str!` rather than a runtime read on purpose: the check becomes a
/// **compile-time** dependency, so renaming a test in the core, agent, or app
/// crate rebuilds this fixture and fails it, instead of leaving a manifest
/// entry that names a function nobody has written for three commits.
const CC6_TEST_SOURCES: [(&str, &str); 10] = [
    (
        "crates/kinewright-media/src/cc6_fixtures.rs",
        include_str!("cc6_fixtures.rs"),
    ),
    (
        "crates/kinewright-media/src/compositor.rs",
        include_str!("compositor.rs"),
    ),
    (
        "crates/kinewright-media/src/verify.rs",
        include_str!("verify.rs"),
    ),
    (
        "crates/kinewright-media/src/export.rs",
        include_str!("export.rs"),
    ),
    (
        "crates/kinewright-core/tests/cc6_core.rs",
        include_str!("../../kinewright-core/tests/cc6_core.rs"),
    ),
    (
        "crates/kinewright-agent/tests/mcp_server.rs",
        include_str!("../../kinewright-agent/tests/mcp_server.rs"),
    ),
    (
        "crates/kinewright-agent/src/export_queue.rs",
        include_str!("../../kinewright-agent/src/export_queue.rs"),
    ),
    (
        "crates/kinewright-app/src/preview_ui.rs",
        include_str!("../../kinewright-app/src/preview_ui.rs"),
    ),
    (
        "crates/kinewright-app/src/color_scopes_ui.rs",
        include_str!("../../kinewright-app/src/color_scopes_ui.rs"),
    ),
    (
        "crates/kinewright-app/src/export_ui.rs",
        include_str!("../../kinewright-app/src/export_ui.rs"),
    ),
];

/// One source's text, or a panic naming the path the manifest invented.
fn cc6_test_source(path: &str) -> &'static str {
    CC6_TEST_SOURCES
        .iter()
        .find_map(|(candidate, source)| (*candidate == path).then_some(*source))
        .unwrap_or_else(|| {
            panic!(
                "the manifest names source {path}, which cc6_fixtures.rs does not include; add it \
                 to CC6_TEST_SOURCES"
            )
        })
}

fn is_test_attribute(line: &str) -> bool {
    line == "#[test]" || line.starts_with("#[tokio::test")
}

/// Whether `source` declares `name` as a `#[test]` (or `#[tokio::test]`)
/// function.
///
/// The attribute is required, so a name mentioned in a doc comment, a string
/// literal, or a helper function is not mistaken for a fixture.
fn declares_test(source: &str, name: &str) -> bool {
    let needle = format!("fn {name}(");
    let lines = source.lines().collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        if !line.contains(&needle) {
            continue;
        }
        for previous in lines[..index].iter().rev() {
            let previous = previous.trim();
            if is_test_attribute(previous) {
                return true;
            }
            if previous.is_empty() || previous.starts_with("//") || previous.starts_with("#[") {
                continue;
            }
            break;
        }
    }
    false
}

/// Every `#[test]` function in `source` whose name starts with `prefix`, in
/// declaration order.
fn declared_test_names(source: &str, prefix: &str) -> Vec<String> {
    let lines = source.lines().collect::<Vec<_>>();
    let mut names = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !is_test_attribute(line.trim()) {
            continue;
        }
        for candidate in &lines[index + 1..] {
            let candidate = candidate.trim();
            if candidate.is_empty() || candidate.starts_with("//") || candidate.starts_with("#[") {
                continue;
            }
            let Some(rest) = candidate.split_once("fn ").map(|(_, rest)| rest) else {
                break;
            };
            let Some((name, _)) = rest.split_once('(') else {
                break;
            };
            if name.starts_with(prefix) {
                names.push(name.to_owned());
            }
            break;
        }
    }
    names
}

/// Whether `source` *uses* `needle` as code rather than merely naming it in a
/// comment or a message.
///
/// The distinction matters because this file has to be able to say
/// "`fixture_gpu_or_skip` is forbidden here" in the very assertion that
/// forbids it, and `include_str!` cannot tell prose from code on its own.
fn uses_outside_prose(source: &str, needle: &str) -> bool {
    // A call is the identifier followed by `(`, on a line that is not a
    // comment, with any trailing `//` comment stripped first. Exempting every
    // line that contains a string literal would let
    // `fixture_gpu_or_skip("cc6-verify")` — the natural spelling — evade the
    // guard, so string literals are not exempt; only the identifier-plus-paren
    // shape counts, and prose mentions inside quotes never carry the paren
    // directly after the name.
    // The quoted form is the `std::env::var("NAME")` shape — the needle
    // directly inside a call's parentheses — so this file's own needle list
    // (`["…", "…"]`) cannot match itself.
    let call = format!("{needle}(");
    let env = format!("(\"{needle}\")");
    source.lines().any(|line| {
        let code = line.split("//").next().unwrap_or_default();
        code.contains(&call) || code.contains(&env)
    })
}

fn sorted(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut names = names.into_iter().collect::<Vec<_>>();
    names.sort_unstable();
    names
}

fn cc6_manifest() -> Value {
    serde_json::from_str(include_str!("../tests/fixtures/cc6_manifest.json"))
        .expect("CC6 fixture manifest must be valid JSON")
}

/// CC6 §11.2.23. The declared test inventories are tied to the source they
/// claim to describe, in **both** directions: every name this file lists
/// exists as a `#[test]` function in the file that owns it, and every `cc6_*`
/// test the media sources declare is listed.
#[test]
fn cc6_declared_test_names_exist_in_their_source_files() {
    // --- both directions, for the media sources --------------------------
    let declared_in_media = sorted(
        CC6_MEDIA_TEST_SOURCES
            .into_iter()
            .flat_map(|path| declared_test_names(cc6_test_source(path), "cc6_")),
    );
    assert_eq!(
        declared_in_media,
        sorted(CC6_MEDIA_TESTS.map(str::to_owned)),
        "CC6_MEDIA_TESTS and the `cc6_*` tests the media sources actually declare disagree"
    );
    for name in CC6_MEDIA_TESTS {
        assert!(
            name.starts_with("cc6_"),
            "{name} does not match the `cargo test -- cc6` filter"
        );
    }
    // The media CC6 fixtures take the panicking `fallback_gpu()` convention,
    // not `fixture_gpu_or_skip()`, which passes when the skip opt-in is set.
    // `export.rs` is held to the same rule: its three GPU-backed CC6 export
    // tests are §11.2 evidence for items 11 and 15, and evidence that reports
    // `ok` without running is not evidence.
    let fixtures = cc6_test_source("crates/kinewright-media/src/cc6_fixtures.rs");
    for path in [
        "crates/kinewright-media/src/cc6_fixtures.rs",
        "crates/kinewright-media/src/export.rs",
        "crates/kinewright-media/src/verify.rs",
    ] {
        for needle in ["fixture_gpu_or_skip", "KINEWRIGHT_GPU_TESTS_MAY_SKIP"] {
            assert!(
                !uses_outside_prose(cc6_test_source(path), needle),
                "rule 11.0.6: {path} must never reach for {needle}; the CC6 lanes take the \
                 panicking fallback_gpu() / hardware_gpu() convention"
            );
        }
    }

    // --- both directions, for the other three crates ----------------------
    for (path, expected) in [
        (
            "crates/kinewright-core/tests/cc6_core.rs",
            CC6_CORE_TESTS.to_vec(),
        ),
        (
            "crates/kinewright-media/src/export.rs",
            CC6_EXPORT_TESTS.to_vec(),
        ),
    ] {
        for name in &expected {
            assert!(
                declares_test(cc6_test_source(path), name),
                "{path} does not declare a #[test] named {name}"
            );
        }
    }
    let declared_in_core = declared_test_names(
        cc6_test_source("crates/kinewright-core/tests/cc6_core.rs"),
        "cc6_",
    );
    assert_eq!(
        sorted(declared_in_core),
        sorted(CC6_CORE_TESTS.map(str::to_owned)),
        "CC6_CORE_TESTS and the `cc6_*` tests cc6_core.rs actually declares disagree"
    );

    let agent_sources = [
        "crates/kinewright-agent/tests/mcp_server.rs",
        "crates/kinewright-agent/src/export_queue.rs",
    ];
    let app_sources = [
        "crates/kinewright-app/src/preview_ui.rs",
        "crates/kinewright-app/src/color_scopes_ui.rs",
        "crates/kinewright-app/src/export_ui.rs",
    ];
    for (label, sources, expected) in [
        ("agent", agent_sources.to_vec(), CC6_AGENT_TESTS.to_vec()),
        ("app", app_sources.to_vec(), CC6_APP_TESTS.to_vec()),
    ] {
        let declared = sorted(
            sources
                .iter()
                .flat_map(|path| declared_test_names(cc6_test_source(path), "cc6_")),
        );
        let missing = expected
            .iter()
            .filter(|name| !declared.iter().any(|declared| declared == *name))
            .collect::<Vec<_>>();
        assert!(
            missing.is_empty(),
            "the {label} crate does not yet declare {missing:?}; the inventory names what §11.2 \
             requires and is not trimmed to match an unfinished crate"
        );
        assert_eq!(
            declared,
            sorted(expected.iter().map(|name| (*name).to_owned())),
            "CC6_{}_TESTS and the `cc6_*` tests the {label} sources actually declare disagree",
            label.to_uppercase()
        );
    }

    // --- every name the manifest claims exists in the source it names -----
    let manifest = cc6_manifest();
    let mut verified = 0_usize;
    for entry in manifest["required_fixtures"]
        .as_array()
        .expect("the manifest must list the §11.2 items")
    {
        let number = entry["item"].as_u64().expect("a numbered item");
        for owner in entry["owners"].as_array().expect("owners") {
            let crate_name = owner["owner"].as_str().expect("an owner crate name");
            let sources = owner["sources"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("§11.2 item {number} owner {crate_name} must name its source files")
                })
                .iter()
                .map(|path| {
                    let path = path.as_str().expect("a source path");
                    assert!(
                        path.starts_with(&format!("crates/{crate_name}/")),
                        "§11.2 item {number} owner {crate_name} names source {path}, which is not \
                         in that crate"
                    );
                    cc6_test_source(path)
                })
                .collect::<Vec<_>>();
            for test in owner["tests"].as_array().expect("tests") {
                let name = test.as_str().expect("a test name");
                assert!(
                    sources.iter().any(|source| declares_test(source, name)),
                    "§11.2 item {number} owner {crate_name} claims a test named {name}, which \
                     none of its declared sources declares as a #[test] function"
                );
                verified += 1;
            }
        }
    }
    for name in CC6_INVENTORY_TESTS {
        assert!(
            declares_test(fixtures, name),
            "the §11.2.23 inventory test {name} is not declared in cc6_fixtures.rs"
        );
        verified += 1;
    }
    assert_eq!(
        manifest["manifest_self_test"]["test"], CC6_INVENTORY_TESTS[0],
        "the manifest must name the test that asserts it against the code"
    );
    assert_eq!(
        manifest["manifest_self_test"]["inventory_test"], CC6_INVENTORY_TESTS[1],
        "the manifest must name the test that ties its declared test names to their sources"
    );
    // A count, so a manifest that quietly emptied its `tests` arrays cannot
    // pass this test vacuously.
    assert!(
        verified >= 45,
        "only {verified} declared test names were verified; the manifest has lost entries"
    );

    // Every §11.2 item is declared exactly once, and the items whose evidence
    // lives outside this crate are the ones CC6_EXTERNAL_OWNERS names.
    let items = manifest["required_fixtures"]
        .as_array()
        .expect("items")
        .iter()
        .map(|entry| entry["item"].as_u64().expect("a numbered item"))
        .collect::<Vec<_>>();
    assert_eq!(items, (1..=24).collect::<Vec<_>>());
    for (item, owner) in CC6_EXTERNAL_OWNERS {
        let entry = manifest["required_fixtures"]
            .as_array()
            .expect("items")
            .iter()
            .find(|entry| entry["item"] == item)
            .unwrap_or_else(|| panic!("§11.2 item {item} is not in the manifest"));
        assert!(
            entry["owners"]
                .as_array()
                .expect("owners")
                .iter()
                .any(|declared| declared["owner"] == owner),
            "§11.2 item {item} must be owned by {owner}"
        );
    }
}

/// Assert one declared manifest integer equals the code constant the fixtures
/// actually gate with.
///
/// The `i64` sibling of CC1's [`assert_manifest_f64`]: every CC6 threshold that
/// is an integer constant — millionths, hundredths, basis points, code bounds —
/// is asserted exactly, with no float round trip that could hide a one-unit
/// drift.
fn assert_manifest_i64(parent: &Value, key: &str, expected: i64) {
    let declared = parent
        .get(key)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("manifest must declare an integer {key}"));
    assert_eq!(
        declared, expected,
        "manifest {key} does not match the code constant"
    );
}

/// CC6 §11.2.23 and §11.3. Every required fixture is declared with its owner,
/// and every declared threshold is asserted **equal to the code constant** the
/// fixtures gate with — never restated as a literal.
#[test]
fn cc6_manifest_declares_every_required_fixture_and_constant() {
    use kinewright_core::{
        BT709_CB_DENOMINATOR, BT709_CR_DENOMINATOR, BT709_KB, BT709_KR, COLOR_QC_ENGINE,
        DELIVERY_LUMA_MAX_CODE_8BIT, DELIVERY_LUMA_MAX_CODE_10BIT,
        DELIVERY_LUMA_MEAN_CODE_8BIT_MILLIONTHS, DELIVERY_LUMA_MEAN_CODE_10BIT_MILLIONTHS,
        DELIVERY_LUMA_P99_CODE_8BIT_MILLIONTHS, DELIVERY_LUMA_P99_CODE_10BIT_MILLIONTHS,
        DELIVERY_PSNR_FLOOR_DB_HUNDREDTHS_8BIT, DELIVERY_PSNR_FLOOR_DB_HUNDREDTHS_10BIT,
        DELIVERY_RGB_MEAN_CODE_8BIT_MILLIONTHS, DELIVERY_RGB_MEAN_CODE_10BIT_MILLIONTHS,
        MAX_QC_NODE_CONTRIBUTIONS, QC_GAMUT_EXCEPTION_BASIS_POINTS,
        QC_RANGE_EXCEPTION_BASIS_POINTS, SKIN_BAND_CENTER_CENTIDEGREES,
        SKIN_BAND_EXCEPTION_BASIS_POINTS, SKIN_BAND_HALF_WIDTH_CENTIDEGREES,
        SKIN_MAX_SPREAD_CENTIDEGREES, SKIN_MIN_CHROMA_MILLIONTHS, SKIN_PATCH_HUE_CENTIDEGREES,
        ScopeStage, WORKING_PROOF_ENCODING, WORKING_PROOF_STAGE, YCBCR_CHROMA_LEGAL_HIGH,
        YCBCR_CHROMA_OFFSET, YCBCR_CHROMA_SPAN, YCBCR_LUMA_LEGAL_HIGH, YCBCR_LUMA_OFFSET,
        YCBCR_LUMA_SPAN, delivery_color_for_depth,
    };

    let manifest = cc6_manifest();
    assert_eq!(manifest["manifest_version"], 1);
    assert_eq!(manifest["contract"], "CC6 QC and managed delivery");
    assert_eq!(manifest["contract_token"], CC6_CONTRACT);

    // --- §2.1 the two stages, and which one the scope engine may measure ---
    let stages = manifest["stages"].as_array().expect("two stages");
    assert_eq!(stages.len(), 2);
    for (declared, stage) in stages.iter().zip([
        ScopeStage::MonitoringPostComposite,
        ScopeStage::WorkingLinearPostComposite,
    ]) {
        assert_eq!(declared["stage"], stage.as_str());
        assert_eq!(
            declared["measurable_by_scope_engine"],
            stage.measurable_by_scope_engine()
        );
    }
    assert_eq!(manifest["working_proof"]["stage"], WORKING_PROOF_STAGE);
    assert_eq!(
        manifest["working_proof"]["encoding"],
        WORKING_PROOF_ENCODING
    );
    assert_eq!(manifest["working_proof"]["full_resolution_only"], true);

    // --- §4.1/§4.3 the two delivery lanes ---------------------------------
    let lanes = manifest["delivery_lanes"].as_array().expect("two lanes");
    assert_eq!(lanes.len(), 2);
    let export_source = cc6_test_source("crates/kinewright-media/src/export.rs");
    for (declared, depth) in lanes.iter().zip(DeliveryEncodeDepth::ALL) {
        assert_eq!(declared["name"], depth.as_str());
        assert_eq!(declared["bit_depth"], i64::from(depth.bits()));
        assert_eq!(declared["pixel_format"], depth.pixel_format());
        // §4.3, R7/A10: no `profile` option on either lane — the pixel format
        // selects High 10 and the output is byte-identical either way.
        assert_eq!(declared["profile_option"], Value::Null);
        // The codec, the x264 parameter string, and the scaler flags are
        // private constants in `export.rs`. They are tied to the manifest
        // through the *source* the inventory already includes, so a change to
        // either fails this fixture at compile time rather than drifting.
        for key in ["codec", "x264_params"] {
            let value = declared[key].as_str().expect("a declared string");
            assert!(
                export_source.contains(&format!("\"{value}\"")),
                "manifest delivery_lanes.{key} = {value} does not appear as a constant in \
                 export.rs"
            );
        }
        // The materialized delivery description of this lane, field by field.
        let document = Document::default();
        let color = delivery_color_for_depth(&document, depth);
        let serialized = serde_json::to_value(&color).expect("a delivery description serializes");
        for field in ["primaries", "transfer", "matrix", "range", "white_point"] {
            assert_eq!(
                declared.get(field),
                serialized.get(field),
                "manifest delivery_lanes.{field} does not match delivery_color_for_depth({depth:?})"
            );
        }
        assert_eq!(
            serialized.get("bit_depth"),
            Some(&json!(i64::from(depth.bits()))),
        );
    }
    assert!(
        export_source.contains("\"bicubic\""),
        "§5.3's DELIVERY_SCALER_FLAGS must still be the measured `bicubic`"
    );

    // --- §5.2 the delivery intermediate -----------------------------------
    assert_manifest_i64(
        &manifest["delivery_intermediate"],
        "white",
        i64::from(DELIVERY_INTERMEDIATE_WHITE),
    );
    assert_eq!(
        manifest["delivery_intermediate"]["source_commit"],
        "ad6f6a8"
    );

    // --- §3.5 the skin band -----------------------------------------------
    let skin = &manifest["skin"];
    assert_manifest_i64(
        skin,
        "band_center_centidegrees",
        i64::from(SKIN_BAND_CENTER_CENTIDEGREES),
    );
    assert_manifest_i64(
        skin,
        "band_half_width_centidegrees",
        i64::from(SKIN_BAND_HALF_WIDTH_CENTIDEGREES),
    );
    assert_manifest_i64(skin, "min_chroma_millionths", SKIN_MIN_CHROMA_MILLIONTHS);
    assert_manifest_i64(
        skin,
        "band_exception_basis_points",
        i64::from(SKIN_BAND_EXCEPTION_BASIS_POINTS),
    );
    assert_manifest_i64(
        skin,
        "max_spread_centidegrees",
        i64::from(SKIN_MAX_SPREAD_CENTIDEGREES),
    );
    assert_eq!(
        skin["patch_hue_centidegrees"],
        json!(SKIN_PATCH_HUE_CENTIDEGREES)
    );
    assert!(
        skin["derivation"]
            .as_str()
            .is_some_and(|note| note.contains("circular mean")),
        "the manifest must record how the band constants were derived"
    );

    // --- §3.4 the Y'CbCr reference ----------------------------------------
    let ycbcr = &manifest["ycbcr"];
    crate::cc1_fixtures::assert_manifest_f64(ycbcr, "bt709_kr", BT709_KR);
    crate::cc1_fixtures::assert_manifest_f64(ycbcr, "bt709_kb", BT709_KB);
    crate::cc1_fixtures::assert_manifest_f64(ycbcr, "bt709_cb_denominator", BT709_CB_DENOMINATOR);
    crate::cc1_fixtures::assert_manifest_f64(ycbcr, "bt709_cr_denominator", BT709_CR_DENOMINATOR);
    assert_manifest_i64(ycbcr, "luma_offset", i64::from(YCBCR_LUMA_OFFSET));
    assert_manifest_i64(ycbcr, "luma_span", i64::from(YCBCR_LUMA_SPAN));
    assert_manifest_i64(ycbcr, "chroma_offset", i64::from(YCBCR_CHROMA_OFFSET));
    assert_manifest_i64(ycbcr, "chroma_span", i64::from(YCBCR_CHROMA_SPAN));
    assert_manifest_i64(ycbcr, "luma_legal_high", i64::from(YCBCR_LUMA_LEGAL_HIGH));
    assert_manifest_i64(
        ycbcr,
        "chroma_legal_high",
        i64::from(YCBCR_CHROMA_LEGAL_HIGH),
    );
    // The eight anchors, at both depths, against an **independent** f64
    // transcription of §3.4's equations. Nothing here calls
    // `bt709_limited_ycbcr` (rule 11.0.1).
    let anchors = ycbcr["anchors"].as_array().expect("eight anchor rows");
    assert_eq!(anchors.len(), 8);
    for anchor in anchors {
        let rgb: Vec<f64> = anchor["encoded_rgb"]
            .as_array()
            .expect("an encoded triple")
            .iter()
            .map(|value| value.as_f64().expect("a number"))
            .collect();
        let (r, g, b) = (rgb[0], rgb[1], rgb[2]);
        let luma = 0.2126 * r + (1.0 - 0.2126 - 0.0722) * g + 0.0722 * b;
        let cb = (b - luma) / 1.8556;
        let cr = (r - luma) / 1.5748;
        for (bits, keys) in [
            (8_u8, ["y_8", "cb_8", "cr_8"]),
            (10, ["y_10", "cb_10", "cr_10"]),
        ] {
            let scale = f64::from(1_u32 << (bits - 8));
            for (key, expected) in keys.into_iter().zip([
                16.0 * scale + 219.0 * scale * luma,
                128.0 * scale + 224.0 * scale * cb,
                128.0 * scale + 224.0 * scale * cr,
            ]) {
                let declared = anchor[key].as_f64().expect("a declared anchor code");
                assert!(
                    (declared - expected).abs() <= 1e-6,
                    "manifest ycbcr anchor {key} for {rgb:?} is {declared}, hand-derived {expected}"
                );
            }
        }
    }

    // --- §11.1 the raster --------------------------------------------------
    let raster = &manifest["raster"];
    assert_manifest_i64(raster, "width", i64::from(CC6_QC_RASTER.0));
    assert_manifest_i64(raster, "height", i64::from(CC6_QC_RASTER.1));
    assert_manifest_i64(
        raster,
        "pixels",
        i64::from(CC6_QC_RASTER.0 * CC6_QC_RASTER.1),
    );
    let populations = &raster["populations"];
    for (key, expected) in [
        "in_range_ramp",
        "over_block",
        "under_block",
        "skin_patches",
        "product_patches",
        "below_black_pixel",
        "isolated_over_pixel",
        "surround",
    ]
    .into_iter()
    .zip(CC6_QC_RASTER_POPULATIONS)
    {
        assert_manifest_i64(populations, key, i64::from(expected));
    }
    for (key, expected) in [
        "over",
        "under_red",
        "under_green_blue",
        "out_of_gamut",
        "clamped",
    ]
    .into_iter()
    .zip(CC6_QC_RASTER_BASIS_POINTS)
    {
        assert_manifest_i64(
            &raster["whole_raster_basis_points"],
            key,
            i64::from(expected),
        );
    }
    let roi = &raster["sub_threshold_roi"];
    assert_manifest_i64(
        roi,
        "width_basis_points",
        i64::from(CC6_SUB_THRESHOLD_ROI.width_basis_points),
    );
    assert_manifest_i64(
        roi,
        "height_basis_points",
        i64::from(CC6_SUB_THRESHOLD_ROI.height_basis_points),
    );
    let source = &raster["delivery_source"];
    assert_manifest_i64(source, "width", i64::from(CC6_DELIVERY_SOURCE_SIZE.0));
    assert_manifest_i64(source, "height", i64::from(CC6_DELIVERY_SOURCE_SIZE.1));
    assert_manifest_i64(source, "fps", i64::from(CC6_DELIVERY_SOURCE_FPS));
    assert_manifest_i64(source, "frames", i64::from(CC6_DELIVERY_SOURCE_FRAMES));
    assert_manifest_i64(source, "gop", i64::from(2 * CC6_DELIVERY_SOURCE_FPS));
    assert_eq!(
        source["sampled_frames"],
        json!(CC6_DELIVERY_SOURCE_SAMPLES),
        "the manifest's sampled frames are §6.2's closed form on T = 60, n = 5"
    );

    // --- §11.3 thresholds: one key per pinned constant --------------------
    let thresholds = &manifest["thresholds"];
    let declared = thresholds
        .as_object()
        .expect("the manifest must declare a thresholds object");
    // Rule: no unresolved probe placeholder. Every threshold key holds a
    // number, so a key count alone cannot be satisfied by a placeholder.
    for (key, value) in declared {
        assert!(
            value.is_number(),
            "manifest threshold {key} is {value}, not a number; a placeholder cannot satisfy the \
             key count"
        );
    }
    crate::cc1_fixtures::assert_manifest_f64(thresholds, "bt709_kr", BT709_KR);
    crate::cc1_fixtures::assert_manifest_f64(thresholds, "bt709_kb", BT709_KB);
    crate::cc1_fixtures::assert_manifest_f64(
        thresholds,
        "bt709_cb_denominator",
        BT709_CB_DENOMINATOR,
    );
    crate::cc1_fixtures::assert_manifest_f64(
        thresholds,
        "bt709_cr_denominator",
        BT709_CR_DENOMINATOR,
    );
    for (key, expected) in [
        ("ycbcr_luma_offset", i64::from(YCBCR_LUMA_OFFSET)),
        ("ycbcr_luma_span", i64::from(YCBCR_LUMA_SPAN)),
        ("ycbcr_chroma_offset", i64::from(YCBCR_CHROMA_OFFSET)),
        ("ycbcr_chroma_span", i64::from(YCBCR_CHROMA_SPAN)),
        ("ycbcr_luma_legal_high", i64::from(YCBCR_LUMA_LEGAL_HIGH)),
        (
            "ycbcr_chroma_legal_high",
            i64::from(YCBCR_CHROMA_LEGAL_HIGH),
        ),
        (
            "qc_range_exception_basis_points",
            i64::from(QC_RANGE_EXCEPTION_BASIS_POINTS),
        ),
        (
            "qc_gamut_exception_basis_points",
            i64::from(QC_GAMUT_EXCEPTION_BASIS_POINTS),
        ),
        ("skin_min_chroma_millionths", SKIN_MIN_CHROMA_MILLIONTHS),
        (
            "skin_band_center_centidegrees",
            i64::from(SKIN_BAND_CENTER_CENTIDEGREES),
        ),
        (
            "skin_band_half_width_centidegrees",
            i64::from(SKIN_BAND_HALF_WIDTH_CENTIDEGREES),
        ),
        (
            "skin_band_exception_basis_points",
            i64::from(SKIN_BAND_EXCEPTION_BASIS_POINTS),
        ),
        (
            "skin_max_spread_centidegrees",
            i64::from(SKIN_MAX_SPREAD_CENTIDEGREES),
        ),
        (
            "max_qc_node_contributions",
            i64::try_from(MAX_QC_NODE_CONTRIBUTIONS).expect("the node bound"),
        ),
        (
            "delivery_verification_frame_count",
            i64::from(DELIVERY_VERIFICATION_FRAME_COUNT),
        ),
        (
            "delivery_verification_max_frames",
            i64::from(DELIVERY_VERIFICATION_MAX_FRAMES),
        ),
        (
            "delivery_luma_max_code_8bit",
            i64::from(DELIVERY_LUMA_MAX_CODE_8BIT),
        ),
        (
            "delivery_luma_p99_code_8bit_millionths",
            DELIVERY_LUMA_P99_CODE_8BIT_MILLIONTHS,
        ),
        (
            "delivery_luma_mean_code_8bit_millionths",
            DELIVERY_LUMA_MEAN_CODE_8BIT_MILLIONTHS,
        ),
        (
            "delivery_rgb_mean_code_8bit_millionths",
            DELIVERY_RGB_MEAN_CODE_8BIT_MILLIONTHS,
        ),
        (
            "delivery_psnr_floor_db_hundredths_8bit",
            i64::from(DELIVERY_PSNR_FLOOR_DB_HUNDREDTHS_8BIT),
        ),
        (
            "delivery_luma_max_code_10bit",
            i64::from(DELIVERY_LUMA_MAX_CODE_10BIT),
        ),
        (
            "delivery_luma_p99_code_10bit_millionths",
            DELIVERY_LUMA_P99_CODE_10BIT_MILLIONTHS,
        ),
        (
            "delivery_luma_mean_code_10bit_millionths",
            DELIVERY_LUMA_MEAN_CODE_10BIT_MILLIONTHS,
        ),
        (
            "delivery_rgb_mean_code_10bit_millionths",
            DELIVERY_RGB_MEAN_CODE_10BIT_MILLIONTHS,
        ),
        (
            "delivery_psnr_floor_db_hundredths_10bit",
            i64::from(DELIVERY_PSNR_FLOOR_DB_HUNDREDTHS_10BIT),
        ),
        (
            "decoded_range_exception_basis_points",
            i64::from(DECODED_RANGE_EXCEPTION_BASIS_POINTS),
        ),
        (
            "delivery_intermediate_white",
            i64::from(DELIVERY_INTERMEDIATE_WHITE),
        ),
        (
            "delivery_reference_denominator",
            i64::from(crate::verify::DELIVERY_REFERENCE_DENOMINATOR),
        ),
        (
            "ebu_r103_tolerance_codes_8bit",
            EBU_R103_TOLERANCE_CODES_8BIT,
        ),
    ] {
        assert_manifest_i64(thresholds, key, expected);
    }
    // The key count, so a constant cannot be added to the code without being
    // declared here.
    assert_eq!(
        declared.len(),
        34,
        "every pinned CC6 constant of §3-§6 must have exactly one threshold key"
    );

    // --- §6.3 budgets, measured, and their distinctness -------------------
    let budgets = &manifest["budgets"];
    for (lane, depth) in [
        ("eight_bit", DeliveryEncodeDepth::Eight),
        ("ten_bit", DeliveryEncodeDepth::Ten),
    ] {
        let declared = &budgets[lane];
        let code = kinewright_core::DeliveryBudgets::for_depth(depth);
        for (key, expected) in [
            ("luma_max_code", i64::from(code.luma_max_code)),
            ("luma_p99_code_millionths", code.luma_p99_code_millionths),
            ("luma_mean_code_millionths", code.luma_mean_code_millionths),
            ("rgb_mean_code_millionths", code.rgb_mean_code_millionths),
            (
                "psnr_floor_db_hundredths",
                i64::from(code.psnr_floor_db_hundredths),
            ),
        ] {
            assert_manifest_i64(&declared[key], "threshold", expected);
            assert!(
                declared[key]["measured"].is_number(),
                "§11.3: {lane}.{key} must record the measurement the fixture made"
            );
        }
        // Rule 11.0.5, recorded: `margin_ratio` is the arithmetic of the two
        // numbers beside it, so a stale margin cannot survive a re-baseline.
        for key in [
            "luma_max_code",
            "luma_p99_code_millionths",
            "luma_mean_code_millionths",
            "rgb_mean_code_millionths",
        ] {
            let entry = &declared[key];
            let threshold = entry["threshold"].as_f64().expect("a threshold");
            let measured = entry["measured"].as_f64().expect("a measurement");
            match entry["margin_ratio"].as_f64() {
                Some(ratio) => {
                    assert!(measured > 0.0, "a zero measurement has no finite margin");
                    assert!(
                        (ratio - threshold / measured).abs() <= 0.001 * ratio.max(1.0),
                        "{lane}.{key} declares a margin of {ratio}x, but {threshold}/{measured} \
                         is {}x",
                        threshold / measured
                    );
                }
                None => assert_eq!(
                    measured, 0.0,
                    "{lane}.{key} may only decline a numeric margin when the measurement is zero"
                ),
            }
        }
    }
    // CC1's rule: a codec tolerance and a compositor tolerance must be
    // numerically distinct, in the manifest as well as in the code.
    let monitor = &budgets["monitor_cpu_gpu"];
    crate::cc1_fixtures::assert_manifest_f64(monitor, "max_code", f64::from(MONITOR_CPU_GPU_MAX));
    crate::cc1_fixtures::assert_manifest_f64(monitor, "p99_code", MONITOR_CPU_GPU_P99);
    crate::cc1_fixtures::assert_manifest_f64(monitor, "mean_code", MONITOR_CPU_GPU_MEAN);
    for depth in DeliveryEncodeDepth::ALL {
        assert_delivery_budgets_are_distinct(depth);
    }
    // The 10-bit lane's justification, in the units §6.3 words it in.
    let justification = &budgets["ten_bit_justification"];
    assert!(
        justification["ten_bit_rgb_mean_code_millionths"]
            .as_i64()
            .expect("a recorded measurement")
            < justification["eight_bit_rgb_mean_code_millionths"]
                .as_i64()
                .expect("a recorded measurement")
    );
    assert!(
        justification["ten_bit_psnr_db_hundredths"]
            .as_i64()
            .expect("a recorded measurement")
            > justification["eight_bit_psnr_db_hundredths"]
                .as_i64()
                .expect("a recorded measurement")
    );
    assert_eq!(justification["strictly_better"], true);

    // §11.2.13's recorded failing direction, both lanes. The manifest names
    // which gated terms tripped and which stayed inside; the fixtures pin the
    // same sets, so a re-baseline that moved the failure from the codec-only
    // luma plane to the whole-raster sanity floor cannot be recorded here as
    // if nothing had changed.
    let starved = &budgets["starved_bitrate_failing_direction"];
    assert_eq!(starved["within_budgets"], false);
    assert_eq!(starved["technical_pass"], false);
    assert_manifest_i64(
        starved,
        "video_bitrate",
        i64::try_from(CC6_STARVED_VIDEO_BITRATE).expect("the starved bitrate"),
    );
    for lane in ["eight_bit", "ten_bit"] {
        let declared = &starved[lane];
        let names = |key: &str| -> Vec<String> {
            declared[key]
                .as_array()
                .unwrap_or_else(|| panic!("starved {lane} must declare {key}"))
                .iter()
                .map(|value| value.as_str().expect("a gated field name").to_owned())
                .collect()
        };
        let over = names("over_budget_fields");
        let inside = names("inside_budget_terms");
        assert!(!over.is_empty(), "a starved lane must break something");
        for field in over.iter().chain(&inside) {
            assert!(
                CC6_GATED_BUDGET_FIELDS.contains(&field.as_str()),
                "starved {lane} names {field}, which is not one of §6.3's gated terms"
            );
        }
        for field in &over {
            assert!(
                !inside.contains(field),
                "starved {lane} declares {field} both over and inside its budget"
            );
        }
        assert_eq!(
            sorted(over.into_iter().chain(inside)),
            sorted(CC6_GATED_BUDGET_FIELDS.map(str::to_owned)),
            "starved {lane} must account for every gated term exactly once"
        );
        assert!(
            declared["luma_max_code_diff"]
                .as_i64()
                .expect("a measurement")
                > i64::from(
                    kinewright_core::DeliveryBudgets::for_depth(match lane {
                        "eight_bit" => DeliveryEncodeDepth::Eight,
                        _ => DeliveryEncodeDepth::Ten,
                    })
                    .luma_max_code
                ),
            "the recorded starved {lane} luma maximum must actually exceed the lane's budget"
        );
    }

    // --- §5 measured behaviour --------------------------------------------
    let behaviour = &manifest["measured_behaviour"];
    assert!(
        behaviour["dither"]
            .as_str()
            .is_some_and(|note| note.contains("8x8 ordered dither")),
        "§5.4's dither finding must be recorded"
    );
    assert!(
        behaviour["inert_options"]
            .as_array()
            .expect("the inert-option list")
            .len()
            >= 6
    );
    assert_eq!(behaviour["scaler"]["chosen"], "bicubic");
    assert_eq!(
        behaviour["scaler"]["comparison"]
            .as_array()
            .expect("three scalers")
            .len(),
        3
    );
    assert!(
        behaviour["decode_flags_rule"]
            .as_str()
            .is_some_and(|note| note.contains("full_chroma_int")),
        "§5.5's decode-flag rule must be recorded"
    );

    // --- §11.2 the fixtures ------------------------------------------------
    assert_eq!(
        manifest["evidence_fixtures"],
        json!(CC6_EVIDENCE_FIXTURES),
        "every evidence payload this file emits must be declared in the manifest"
    );
    assert_eq!(
        manifest["required_fixtures"]
            .as_array()
            .expect("the §11.2 items")
            .len(),
        24
    );
    assert_eq!(manifest["working_proof"]["stage"], WORKING_PROOF_STAGE);
    assert_eq!(COLOR_QC_ENGINE, "kinewright_color_qc_v1");
}
