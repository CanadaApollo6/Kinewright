//! CC7 §2: the scenario authority for the six named colour workflows.
//!
//! This module is the **single** place a CC7 raster rectangle, patch code,
//! camera transform, canonical operation, or budget threshold is written down.
//! Six scenarios times three execution paths (media fixture, scripted agent,
//! person builders) times two crates that also read them (agent eval, app) is
//! seven places a number could drift, so CC7 §2.1 forbids restating one of
//! these values as a literal anywhere else in the workspace.
//!
//! It is **data and arithmetic only** (CC7 §2.1): no `Document` mutation, no
//! rendering, no filesystem, no clock, no RNG. Two evaluations of any function
//! here produce identical values on both CI operating systems (CC7 §10.9).
//!
//! # What this module is not (CC7 §2.7)
//!
//! It does not re-implement `measure_color_qc`, `match_parameters`
//! (`kinewright-agent::color_scopes`), `bt709_limited_ycbcr`, or the
//! compositor. CC7 §11.0.1 forbids a fixture from obtaining an expected value
//! by calling any of them, and this module's job is to be the place the
//! analytic value is written down instead.
//!
//! # The three transfer transcriptions (R-M2)
//!
//! `crates/kinewright-core/Cargo.toml` has **no path dependency on
//! `kinewright-media`**, so [`crate::cc7_scenarios::cc7_encode_bt709`]'s
//! subjects — `encode_bt709`, `decode_display709` and `grade709_decode`,
//! owned by `crates/kinewright-media/src/color_pipeline.rs` — are unreachable
//! from here. This module therefore carries its own `f64` transcription of
//! each, with a comment naming the owning module, exactly as CC6's
//! `crates/kinewright-core/tests/cc6_core.rs:35-37` restated CC1's
//! `SPEC_F64_TOLERANCE` for the same reason. The transcriptions are held
//! honest from the media side by `cc7_core_transcriptions_agree_with_the_pipeline`
//! (CC7 §11.2.12b): a transcription nobody cross-checks is a second
//! definition; a transcription with a cross-check is a boundary.

use std::collections::BTreeMap;

use crate::{
    AutomationCurve, ClipId, Effect, EffectId, Keyframe, KeyframeInterpolation, LutAsset,
    LutAssetId, LutAssetKind, LutAssetSource, NormalizedRoi, Operation, ParamValue, TimeCode,
};

// ===========================================================================
// CC7 §2.7: the three transfer transcriptions, and the tolerance they hold to.
// ===========================================================================

/// CC1's `SPEC_F64_TOLERANCE`, restated here because `cc1_fixtures.rs` lives
/// in the media crate, is `pub(crate)`, and core cannot see it (R-M2).
pub const CC7_SPEC_F64_TOLERANCE: f64 = 1e-6;

/// The CC3 sign function: `sgn(0) = 0`.
///
/// Transcribed from `grade709_sign`, owned by
/// `crates/kinewright-media/src/color_pipeline.rs:938-946`. [`f64::signum`] is
/// deliberately not used; it returns `±1` at zero and would break `E(0) = 0`.
#[must_use]
fn cc7_sign(value: f64) -> f64 {
    if value > 0.0 {
        1.0
    } else if value < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// The CC1 BT.709 **monitor** display transfer, sign-preserving.
///
/// Independent `f64` transcription of `encode_bt709`, owned by
/// `crates/kinewright-media/src/color_pipeline.rs:354-364` (CC7 §2.7, R-M2).
/// Cross-checked against the owner by
/// `cc7_core_transcriptions_agree_with_the_pipeline` (CC7 §11.2.12b).
#[must_use]
pub fn cc7_encode_bt709(linear: f64) -> f64 {
    if linear < 0.0 {
        -cc7_encode_bt709(-linear)
    } else if linear < 0.018 {
        4.5 * linear
    } else {
        1.099 * linear.powf(0.45) - 0.099
    }
}

/// The exact sign-preserving inverse of [`cc7_encode_bt709`], with CC1's
/// rounded broadcast constants.
///
/// Independent `f64` transcription of `decode_display709`, owned by
/// `crates/kinewright-media/src/color_pipeline.rs:386-394` (CC7 §2.7, R-M2).
#[must_use]
pub fn cc7_decode_display709(encoded: f64) -> f64 {
    let sign = cc7_sign(encoded);
    let magnitude = encoded.abs();
    if magnitude < 0.081 {
        sign * magnitude / 4.5
    } else {
        sign * ((magnitude + 0.099) / 1.099).powf(1.0 / 0.45)
    }
}

/// CC3 §2.1's `grade709` decode: the working grading space back to
/// scene-linear light.
///
/// Independent `f64` transcription of `grade709_decode`, owned by
/// `crates/kinewright-media/src/color_pipeline.rs:975-985`, with CC3's precise
/// constants rather than the rounded broadcast ones (CC7 §2.7, R-M2).
#[must_use]
pub fn cc7_grade709_decode(encoded: f64) -> f64 {
    /// CC3 §2.1 `ALPHA`.
    const ALPHA: f64 = 1.099_296_8;
    /// CC3 §2.1 `BETA_E = 4.5 * BETA`.
    const BETA_ENCODED: f64 = 0.081_242_86;
    /// CC3 §2.1 `K = ALPHA - 1`.
    const K: f64 = 0.099_296_8;
    /// CC3 §2.1 `INV`, the `f32` nearest of `1 / 0.45`.
    const INVERSE_EXPONENT: f64 = 2.222_222_3;
    /// The BT.709 near-black slope.
    const SLOPE: f64 = 4.5;

    let sign = cc7_sign(encoded);
    let magnitude = encoded.abs();
    if magnitude < BETA_ENCODED {
        sign * magnitude / SLOPE
    } else {
        sign * ((magnitude + K) / ALPHA).powf(INVERSE_EXPONENT)
    }
}

/// One CC7 integer as `f64`. Every value this module converts — codes,
/// millionths, basis points, pixel counts — is far inside `2^53`, so the
/// conversion is exact and the pedantic precision-loss lint is answered here
/// once rather than at each call site.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub const fn cc7_as_f64(value: i64) -> f64 {
    value as f64
}

/// CC7 §10.1's rounding: half **away from zero**, to an integer.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn cc7_round_half_away_from_zero(value: f64) -> i64 {
    if value >= 0.0 {
        (value + 0.5).floor() as i64
    } else {
        -((-value + 0.5).floor() as i64)
    }
}

/// A `0.0 ..= 1.0` display value as an 8-bit monitoring code, clamped.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn cc7_display_code(display: f64) -> u8 {
    cc7_round_half_away_from_zero(display * 255.0).clamp(0, 255) as u8
}

/// A unit-interval value in CC7 §10.1's `_MILLIONTHS` integer unit.
#[must_use]
pub fn cc7_millionths(value: f64) -> i64 {
    cc7_round_half_away_from_zero(value * 1_000_000.0)
}

// ===========================================================================
// CC7 §2.3: raster geometry.
// ===========================================================================

/// CC7 §2.3.1's shared raster width, CC6's (`cc6_fixtures.rs:297-302`).
pub const CC7_SOURCE_WIDTH: u32 = 320;
/// CC7 §2.3.1's shared raster height.
pub const CC7_SOURCE_HEIGHT: u32 = 180;
/// CC7 §2.3.1's frame rate, so the encoder GOP is `2 · fps = 50`.
pub const CC7_SOURCE_FPS: u32 = 25;
/// Frames in scenarios (a)–(e) and (g), so CC6 §6.2's five delivery samples
/// are `0, 14, 29, 44, 59` and span two GOPs.
pub const CC7_SOURCE_FRAMES: u32 = 60;
/// Frames in scenario (f).
pub const CC7_TRACK_FRAMES: u32 = 100;
/// Every pixel of one CC7 frame.
pub const CC7_RASTER_PIXELS: u32 = CC7_SOURCE_WIDTH * CC7_SOURCE_HEIGHT;

/// CC5's `CHART_SURROUND` `[0.45, 0.45, 0.45]` grade709
/// (`cc5_fixtures.rs:4639`) at its display encoding,
/// `round(255 · 0.450_148) = 115`.
pub const CC7_SURROUND_CODE: u8 = 115;
/// The surround's grade709 value in millionths.
pub const CC7_SURROUND_GRADE709_MILLIONTHS: [i64; 3] = [450_000, 450_000, 450_000];

/// Width of one achromatic chart patch and of one primaries patch.
pub const CC7_CHART_PATCH_WIDTH: u32 = 8;
/// Width of one patch-row patch.
pub const CC7_ROW_PATCH_WIDTH: u32 = 12;
/// Height of every named patch band.
pub const CC7_PATCH_BAND_HEIGHT: u32 = 16;
/// Pixels in one chart or primaries patch, `8 · 16`.
pub const CC7_CHART_PATCH_PIXELS: u32 = CC7_CHART_PATCH_WIDTH * CC7_PATCH_BAND_HEIGHT;
/// Pixels in one patch-row patch, `12 · 16`.
pub const CC7_ROW_PATCH_PIXELS: u32 = CC7_ROW_PATCH_WIDTH * CC7_PATCH_BAND_HEIGHT;
/// The `product_red` patch's population, the exact (d) containment gate.
pub const CC7_PRODUCT_PATCH_PIXEL_COUNT: u32 = CC7_ROW_PATCH_PIXELS;

/// The twelve achromatic chart patches (A1).
pub const CC7_CHART_PATCH_COUNT: usize = 12;
/// The five saturated primaries; the pure red is deliberately absent (A1).
pub const CC7_PRIMARY_PATCH_COUNT: usize = 5;
/// The seven skin/product/shadow patches of the patch row.
pub const CC7_ROW_PATCH_COUNT: usize = 7;
/// Every named patch, in `chart, primaries, row` order (CC7 §10.5).
pub const CC7_PATCH_COUNT: usize =
    CC7_CHART_PATCH_COUNT + CC7_PRIMARY_PATCH_COUNT + CC7_ROW_PATCH_COUNT;

/// A half-open pixel rectangle in the `320 × 180` base grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cc7PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Cc7PixelRect {
    /// Construct a half-open pixel rectangle.
    #[must_use]
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The rectangle's pixel population.
    #[must_use]
    pub const fn pixels(self) -> u32 {
        self.width * self.height
    }
}

/// CC7 §2.3.2's normative basis-point conversion, start side.
///
/// `start_bp = ceil(p0 · 10_000 / E)`. **The `ceil` is load-bearing** (A19):
/// the naive floor for the patch row's `y 76` gives `4222`, which
/// [`NormalizedRoi::to_pixels`] resolves to `y 75, h 17` — 204 pixels rather
/// than 192, because the floored start lands one row early.
#[must_use]
pub const fn cc7_start_basis_points(pixel: u32, extent: u32) -> u32 {
    (pixel * 10_000).div_ceil(extent)
}

/// CC7 §2.3.2's normative basis-point conversion, exclusive-end side.
///
/// `end_bp = floor(p1 · 10_000 / E)`, matched to [`NormalizedRoi::to_pixels`]'s
/// `ceil` on the exclusive end.
#[must_use]
pub const fn cc7_end_basis_points(pixel: u32, extent: u32) -> u32 {
    pixel * 10_000 / extent
}

/// The [`NormalizedRoi`] that resolves to the half-open pixel rect
/// `[x0, x1) × [y0, y1)` on the CC7 raster, by CC7 §2.3.2's rule.
#[must_use]
pub const fn cc7_roi_for(x0: u32, x1: u32, y0: u32, y1: u32) -> NormalizedRoi {
    let x = cc7_start_basis_points(x0, CC7_SOURCE_WIDTH);
    let y = cc7_start_basis_points(y0, CC7_SOURCE_HEIGHT);
    NormalizedRoi::new(
        x,
        y,
        cc7_end_basis_points(x1, CC7_SOURCE_WIDTH) - x,
        cc7_end_basis_points(y1, CC7_SOURCE_HEIGHT) - y,
    )
}

/// The horizontal neutral ramp, `grey(x · 255 / 319)` by integer division.
pub const CC7_RAMP_RECT: Cc7PixelRect = Cc7PixelRect::new(0, 0, 320, 20);
/// [`CC7_RAMP_RECT`] in basis points.
pub const CC7_RAMP_ROI: NormalizedRoi = cc7_roi_for(0, 320, 0, 20);
/// The twelve-patch achromatic chart band (A1).
pub const CC7_CHART_BAND_RECT: Cc7PixelRect = Cc7PixelRect::new(0, 36, 96, 16);
/// [`CC7_CHART_BAND_RECT`] in basis points; the (a) `plan_shot_match` ROI.
pub const CC7_CHART_BAND_ROI: NormalizedRoi = cc7_roi_for(0, 96, 36, 52);
/// The five-patch primaries band (A1).
pub const CC7_PRIMARY_BAND_RECT: Cc7PixelRect = Cc7PixelRect::new(0, 56, 40, 16);
/// [`CC7_PRIMARY_BAND_RECT`] in basis points.
pub const CC7_PRIMARY_BAND_ROI: NormalizedRoi = cc7_roi_for(0, 40, 56, 72);
/// The seven-patch skin/product/shadow row.
pub const CC7_ROW_BAND_RECT: Cc7PixelRect = Cc7PixelRect::new(0, 76, 84, 16);
/// [`CC7_ROW_BAND_RECT`] in basis points.
pub const CC7_ROW_BAND_ROI: NormalizedRoi = cc7_roi_for(0, 84, 76, 92);
/// The four skin patches, the (a)(4) and (d)(3) skin ROI.
pub const CC7_SKIN_BAND_RECT: Cc7PixelRect = Cc7PixelRect::new(0, 76, 48, 16);
/// [`CC7_SKIN_BAND_RECT`] in basis points.
pub const CC7_SKIN_BAND_ROI: NormalizedRoi = cc7_roi_for(0, 48, 76, 92);
/// The `product_red` patch, the (d) qualifier sample ROI.
pub const CC7_PRODUCT_RED_RECT: Cc7PixelRect = Cc7PixelRect::new(48, 76, 12, 16);
/// [`CC7_PRODUCT_RED_RECT`] in basis points.
pub const CC7_PRODUCT_RED_ROI: NormalizedRoi = cc7_roi_for(48, 60, 76, 92);
/// The `deep_shadow` patch, the (e) exact gamut ROI (A3, A19).
pub const CC7_DEEP_SHADOW_RECT: Cc7PixelRect = Cc7PixelRect::new(72, 76, 12, 16);
/// [`CC7_DEEP_SHADOW_RECT`] in basis points; `y = 4223`, never the naive 4222.
pub const CC7_DEEP_SHADOW_ROI: NormalizedRoi = cc7_roi_for(72, 84, 76, 92);

/// CC7 §2.3.3's population table: the five regions and their pixel counts.
///
/// `6 400 + 1 536 + 640 + 1 344 + 47 680 = 57 600 = 320 · 180`.
pub const CC7_REGION_POPULATIONS: [(&str, u32); 5] = [
    ("neutral_ramp_band", 6_400),
    ("achromatic_chart_band", 1_536),
    ("primaries_band", 640),
    ("patch_row", 1_344),
    ("surround", 47_680),
];

/// The number of surround pixels, CC7 §2.3.3's remainder.
pub const CC7_SURROUND_PIXELS: u32 = 47_680;

/// The ramp's display code at column `x`, `grey(x · 255 / 319)` by integer
/// division (CC7 §2.3.3).
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub const fn cc7_ramp_code(x: u32) -> u8 {
    (x * 255 / (CC7_SOURCE_WIDTH - 1)) as u8
}

// ===========================================================================
// CC7 §2.3.3 and §2.4.1: the patch tables.
// ===========================================================================

/// One named patch of the CC7 base scene, with every form CC7 measures it in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cc7Patch {
    /// The manifest name, also the coverage-by-region key.
    pub name: &'static str,
    /// The half-open pixel rect in the `320 × 180` base grid.
    pub rect: Cc7PixelRect,
    /// The basis-point rect that resolves to [`Cc7Patch::rect`] exactly.
    pub roi: NormalizedRoi,
    /// The authored grade709 value in millionths; `None` for the chart and
    /// primaries patches, which are authored as display codes directly.
    pub grade709: Option<[i64; 3]>,
    /// The camera-A display code, `round(255 · encode_bt709(grade709_decode(g)))`.
    pub display_code_cam_a: [u8; 3],
    /// The scene-linear value behind that code, in millionths.
    pub linear_millionths_cam_a: [i64; 3],
}

const fn chart_patch(
    name: &'static str,
    index: u32,
    display_code_cam_a: [u8; 3],
    linear_millionths_cam_a: [i64; 3],
) -> Cc7Patch {
    let x0 = index * CC7_CHART_PATCH_WIDTH;
    Cc7Patch {
        name,
        rect: Cc7PixelRect::new(x0, 36, CC7_CHART_PATCH_WIDTH, CC7_PATCH_BAND_HEIGHT),
        roi: cc7_roi_for(x0, x0 + CC7_CHART_PATCH_WIDTH, 36, 52),
        grade709: None,
        display_code_cam_a,
        linear_millionths_cam_a,
    }
}

const fn primary_patch(
    name: &'static str,
    index: u32,
    display_code_cam_a: [u8; 3],
    linear_millionths_cam_a: [i64; 3],
) -> Cc7Patch {
    let x0 = index * CC7_CHART_PATCH_WIDTH;
    Cc7Patch {
        name,
        rect: Cc7PixelRect::new(x0, 56, CC7_CHART_PATCH_WIDTH, CC7_PATCH_BAND_HEIGHT),
        roi: cc7_roi_for(x0, x0 + CC7_CHART_PATCH_WIDTH, 56, 72),
        grade709: None,
        display_code_cam_a,
        linear_millionths_cam_a,
    }
}

const fn row_patch(
    name: &'static str,
    index: u32,
    grade709: [i64; 3],
    display_code_cam_a: [u8; 3],
    linear_millionths_cam_a: [i64; 3],
) -> Cc7Patch {
    let x0 = index * CC7_ROW_PATCH_WIDTH;
    Cc7Patch {
        name,
        rect: Cc7PixelRect::new(x0, 76, CC7_ROW_PATCH_WIDTH, CC7_PATCH_BAND_HEIGHT),
        roi: cc7_roi_for(x0, x0 + CC7_ROW_PATCH_WIDTH, 76, 92),
        grade709: Some(grade709),
        display_code_cam_a,
        linear_millionths_cam_a,
    }
}

/// CC7 §2.3.3's twelve **achromatic** chart patches: CC1's six reference steps
/// plus six intermediates (A1). Every one satisfies `R == G == B`, which is
/// what makes (a)'s spread statistic meaningful over the whole band and (d)'s
/// exact containment reachable.
pub const CC7_CHART_PATCHES: [Cc7Patch; CC7_CHART_PATCH_COUNT] = [
    chart_patch("chart00", 0, [0, 0, 0], [0, 0, 0]),
    chart_patch("chart01", 1, [11, 11, 11], [9_586, 9_586, 9_586]),
    chart_patch("chart02", 2, [24, 24, 24], [20_981, 20_981, 20_981]),
    chart_patch("chart03", 3, [48, 48, 48], [50_697, 50_697, 50_697]),
    chart_patch("chart04", 4, [72, 72, 72], [95_172, 95_172, 95_172]),
    chart_patch("chart05", 5, [104, 104, 104], [179_084, 179_084, 179_084]),
    chart_patch("chart06", 6, [128, 128, 128], [261_482, 261_482, 261_482]),
    chart_patch("chart07", 7, [152, 152, 152], [361_292, 361_292, 361_292]),
    chart_patch("chart08", 8, [180, 180, 180], [500_507, 500_507, 500_507]),
    chart_patch("chart09", 9, [208, 208, 208], [665_016, 665_016, 665_016]),
    chart_patch("chart10", 10, [242, 242, 242], [899_828, 899_828, 899_828]),
    chart_patch(
        "chart11",
        11,
        [255, 255, 255],
        [1_000_000, 1_000_000, 1_000_000],
    ),
];

/// CC7 §2.3.3's five saturated primaries.
///
/// **The pure red `[255, 0, 0]` is deliberately absent** (A1): probe P5
/// measured the derived `product_red` qualifier at hue `35 865 ± 1 500`
/// centidegrees with `1 000` cd softness, and the red primary's grade709 hue
/// of `0` cd sits 135 cd from that centre, so it is captured and (d)'s "exactly
/// 192" could not pass. Magenta (`30 000` cd) and yellow (`6 000` cd) are more
/// than 2 500 cd away and stay; the blue primary stays because it is part of
/// the population that clips in (b2).
pub const CC7_PRIMARY_PATCHES: [Cc7Patch; CC7_PRIMARY_PATCH_COUNT] = [
    primary_patch("primary_green", 0, [0, 255, 0], [0, 1_000_000, 0]),
    primary_patch("primary_blue", 1, [0, 0, 255], [0, 0, 1_000_000]),
    primary_patch("primary_cyan", 2, [0, 255, 255], [0, 1_000_000, 1_000_000]),
    primary_patch(
        "primary_magenta",
        3,
        [255, 0, 255],
        [1_000_000, 0, 1_000_000],
    ),
    primary_patch(
        "primary_yellow",
        4,
        [255, 255, 0],
        [1_000_000, 1_000_000, 0],
    ),
];

/// CC7 §2.3.3's patch row. `deep_shadow` is the seventh column of the same
/// row, immediately to the right of `product_cyan`.
pub const CC7_ROW_PATCHES: [Cc7Patch; CC7_ROW_PATCH_COUNT] = [
    row_patch(
        "skin_light",
        0,
        [850_000, 680_000, 600_000],
        [217, 173, 153],
        [721_798, 465_557, 365_963],
    ),
    row_patch(
        "skin_medium",
        1,
        [720_000, 530_000, 440_000],
        [184, 135, 112],
        [520_332, 289_498, 205_445],
    ),
    row_patch(
        "skin_tan",
        2,
        [550_000, 380_000, 300_000],
        [140, 97, 77],
        [310_342, 158_076, 105_347],
    ),
    row_patch(
        "skin_deep",
        3,
        [320_000, 200_000, 150_000],
        [82, 51, 38],
        [117_434, 55_516, 36_983],
    ),
    row_patch(
        "product_red",
        4,
        [800_000, 100_000, 120_000],
        [204, 26, 31],
        [640_023, 22_489, 27_814],
    ),
    row_patch(
        "product_cyan",
        5,
        [100_000, 650_000, 750_000],
        [26, 166, 191],
        [22_489, 426_664, 563_622],
    ),
    row_patch(
        "deep_shadow",
        6,
        [50_000, 50_000, 50_000],
        [13, 13, 13],
        [11_111, 11_111, 11_111],
    ),
];

/// Every named patch in CC7 §10.5's iteration order: chart `0..=11`, then the
/// five primaries, then the row `0..=6`.
#[must_use]
pub fn cc7_all_patches() -> Vec<Cc7Patch> {
    let mut patches = Vec::with_capacity(CC7_PATCH_COUNT);
    patches.extend_from_slice(&CC7_CHART_PATCHES);
    patches.extend_from_slice(&CC7_PRIMARY_PATCHES);
    patches.extend_from_slice(&CC7_ROW_PATCHES);
    patches
}

// ===========================================================================
// CC7 §2.4.3: the camera transforms, applied in linear light.
// ===========================================================================

/// The five source characters CC7 authors (CC7 §2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cc7Camera {
    /// The reference: identity.
    A,
    /// Warm, half a stop under, slightly desaturated.
    B,
    /// Cool and 1.5 stops under: recoverable.
    C1,
    /// Cool and 2.5 stops under: beyond the planner's authority.
    C2,
    /// The log-like carrier of scenario (c); not a linear-light transform.
    LogLike,
}

/// The four cameras whose patch codes CC7 tabulates, in
/// [`CC7_CAMERA_PATCH_CODES`] order.
pub const CC7_CAMERA_ORDER: [Cc7Camera; 4] =
    [Cc7Camera::A, Cc7Camera::B, Cc7Camera::C1, Cc7Camera::C2];

/// CC7 §2.4.3's per-camera linear-light transform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cc7CameraTransform {
    /// Per-channel linear gain, `1_000_000` = unity.
    pub gain_millionths: [i64; 3],
    /// Exposure in milli stops, applied as `2^(milli_stops / 1000)`.
    pub exposure_milli_stops: i64,
    /// Rec.709 luma-preserving saturation, `1_000_000` = unchanged.
    pub saturation_millionths: i64,
}

/// CC7 §2.4.3's table. [`Cc7Camera::LogLike`] has no linear-light transform
/// and resolves to the identity; its content comes from
/// [`cc7_log_encode_code`] instead.
#[must_use]
pub const fn cc7_camera_transform(camera: Cc7Camera) -> Cc7CameraTransform {
    match camera {
        Cc7Camera::A | Cc7Camera::LogLike => Cc7CameraTransform {
            gain_millionths: [1_000_000, 1_000_000, 1_000_000],
            exposure_milli_stops: 0,
            saturation_millionths: 1_000_000,
        },
        Cc7Camera::B => Cc7CameraTransform {
            gain_millionths: [1_060_000, 1_000_000, 940_000],
            exposure_milli_stops: -500,
            saturation_millionths: 850_000,
        },
        Cc7Camera::C1 => Cc7CameraTransform {
            gain_millionths: [920_000, 1_000_000, 1_080_000],
            exposure_milli_stops: -1_500,
            saturation_millionths: 1_000_000,
        },
        Cc7Camera::C2 => Cc7CameraTransform {
            gain_millionths: [800_000, 1_000_000, 1_250_000],
            exposure_milli_stops: -2_500,
            saturation_millionths: 1_000_000,
        },
    }
}

/// BT.709 luma weights, CC7 §2.4.3's saturation mix in linear light.
const CC7_LUMA_WEIGHTS: [f64; 3] = [0.2126, 0.7152, 0.0722];

/// CC7 §2.4.3, verbatim:
/// `code_out(c) = round(255 · encode_bt709(sat(expo(gain(decode_display709(c/255))))))`.
///
/// This is **source-content authoring**, which CC7 §11.0.1's transcription
/// clause permits; nothing compares a measurement against it.
#[must_use]
pub fn cc7_camera_code(camera: Cc7Camera, rgb: [u8; 3]) -> [u8; 3] {
    let transform = cc7_camera_transform(camera);
    let scale = 2.0_f64.powf(cc7_as_f64(transform.exposure_milli_stops) / 1_000.0);
    let saturation = cc7_as_f64(transform.saturation_millionths) / 1_000_000.0;
    let mut linear = [0.0_f64; 3];
    for channel in 0..3 {
        let gain = cc7_as_f64(transform.gain_millionths[channel]) / 1_000_000.0;
        linear[channel] = cc7_decode_display709(f64::from(rgb[channel]) / 255.0) * gain * scale;
    }
    let luma = CC7_LUMA_WEIGHTS[0] * linear[0]
        + CC7_LUMA_WEIGHTS[1] * linear[1]
        + CC7_LUMA_WEIGHTS[2] * linear[2];
    let mut out = [0_u8; 3];
    for channel in 0..3 {
        let mixed = luma + saturation * (linear[channel] - luma);
        out[channel] = cc7_display_code(cc7_encode_bt709(mixed));
    }
    out
}

/// Every named patch's manifest name, in CC7 §10.5's iteration order.
pub const CC7_PATCH_NAMES: [&str; CC7_PATCH_COUNT] = [
    "chart00",
    "chart01",
    "chart02",
    "chart03",
    "chart04",
    "chart05",
    "chart06",
    "chart07",
    "chart08",
    "chart09",
    "chart10",
    "chart11",
    "primary_green",
    "primary_blue",
    "primary_cyan",
    "primary_magenta",
    "primary_yellow",
    "skin_light",
    "skin_medium",
    "skin_tan",
    "skin_deep",
    "product_red",
    "product_cyan",
    "deep_shadow",
];

/// CC7 §2.4.3's full `(12 chart + 5 primary + 7 row) × 3` code table per
/// camera, indexed by [`CC7_CAMERA_ORDER`] then by [`CC7_PATCH_NAMES`].
///
/// Transcribed independently (rule 11.0.1) and reproduced by
/// [`cc7_camera_code`], which `cc7_camera_transforms_are_applied_in_linear_light`
/// asserts. The camera-A row is the base scene's own display codes.
pub const CC7_CAMERA_PATCH_CODES: [[[u8; 3]; CC7_PATCH_COUNT]; 4] = [
    // camera A
    [
        [0, 0, 0],       // chart00
        [11, 11, 11],    // chart01
        [24, 24, 24],    // chart02
        [48, 48, 48],    // chart03
        [72, 72, 72],    // chart04
        [104, 104, 104], // chart05
        [128, 128, 128], // chart06
        [152, 152, 152], // chart07
        [180, 180, 180], // chart08
        [208, 208, 208], // chart09
        [242, 242, 242], // chart10
        [255, 255, 255], // chart11
        [0, 255, 0],     // primary_green
        [0, 0, 255],     // primary_blue
        [0, 255, 255],   // primary_cyan
        [255, 0, 255],   // primary_magenta
        [255, 255, 0],   // primary_yellow
        [217, 173, 153], // skin_light
        [184, 135, 112], // skin_medium
        [140, 97, 77],   // skin_tan
        [82, 51, 38],    // skin_deep
        [204, 26, 31],   // product_red
        [26, 166, 191],  // product_cyan
        [13, 13, 13],    // deep_shadow
    ],
    // camera B
    [
        [0, 0, 0],       // chart00
        [8, 8, 7],       // chart01
        [18, 17, 16],    // chart02
        [39, 37, 36],    // chart03
        [60, 58, 56],    // chart04
        [88, 85, 83],    // chart05
        [109, 106, 103], // chart06
        [130, 126, 123], // chart07
        [154, 150, 146], // chart08
        [179, 174, 170], // chart09
        [209, 204, 198], // chart10
        [220, 215, 209], // chart11
        [63, 210, 63],   // primary_green
        [8, 8, 193],     // primary_blue
        [66, 211, 205],  // primary_cyan
        [209, 34, 197],  // primary_magenta
        [219, 214, 74],  // primary_yellow
        [183, 146, 128], // skin_light
        [154, 113, 95],  // skin_medium
        [116, 81, 65],   // skin_tan
        [66, 41, 31],    // skin_deep
        [165, 33, 35],   // product_red
        [49, 136, 151],  // product_cyan
        [10, 9, 9],      // deep_shadow
    ],
    // camera C1
    [
        [0, 0, 0],       // chart00
        [4, 4, 4],       // chart01
        [8, 9, 9],       // chart02
        [19, 21, 22],    // chart03
        [33, 36, 38],    // chart04
        [53, 56, 59],    // chart05
        [67, 71, 74],    // chart06
        [82, 86, 90],    // chart07
        [99, 103, 108],  // chart08
        [115, 121, 126], // chart09
        [136, 142, 148], // chart10
        [144, 150, 156], // chart11
        [0, 150, 0],     // primary_green
        [0, 0, 156],     // primary_blue
        [0, 150, 156],   // primary_cyan
        [144, 0, 156],   // primary_magenta
        [144, 150, 0],   // primary_yellow
        [121, 99, 90],   // skin_light
        [101, 75, 64],   // skin_medium
        [74, 51, 41],    // skin_tan
        [39, 23, 16],    // skin_deep
        [113, 9, 12],    // product_red
        [9, 95, 115],    // product_cyan
        [4, 5, 5],       // deep_shadow
    ],
    // camera C2
    [
        [0, 0, 0],      // chart00
        [2, 2, 2],      // chart01
        [3, 4, 5],      // chart02
        [8, 10, 13],    // chart03
        [15, 19, 24],   // chart04
        [28, 34, 40],   // chart05
        [38, 45, 52],   // chart06
        [48, 56, 65],   // chart07
        [60, 69, 79],   // chart08
        [71, 82, 93],   // chart09
        [86, 97, 110],  // chart10
        [91, 103, 117], // chart11
        [0, 103, 0],    // primary_green
        [0, 0, 117],    // primary_blue
        [0, 103, 117],  // primary_cyan
        [91, 0, 117],   // primary_magenta
        [91, 103, 0],   // primary_yellow
        [75, 66, 65],   // skin_light
        [62, 48, 44],   // skin_medium
        [43, 31, 27],   // skin_tan
        [19, 11, 9],    // skin_deep
        [70, 5, 7],     // product_red
        [4, 62, 84],    // product_cyan
        [2, 2, 3],      // deep_shadow
    ],
];

/// The codes [`CC7_CAMERA_PATCH_CODES`] holds for one camera, or `None`
/// for [`Cc7Camera::LogLike`], which is not a linear-light transform.
#[must_use]
pub const fn cc7_camera_patch_codes(
    camera: Cc7Camera,
) -> Option<&'static [[u8; 3]; CC7_PATCH_COUNT]> {
    match camera {
        Cc7Camera::A => Some(&CC7_CAMERA_PATCH_CODES[0]),
        Cc7Camera::B => Some(&CC7_CAMERA_PATCH_CODES[1]),
        Cc7Camera::C1 => Some(&CC7_CAMERA_PATCH_CODES[2]),
        Cc7Camera::C2 => Some(&CC7_CAMERA_PATCH_CODES[3]),
        Cc7Camera::LogLike => None,
    }
}

// ===========================================================================
// CC7 §2.4.2: the log-like curve of scenario (c).
// ===========================================================================

/// `v(x) = clamp((log2(x) + 8) / 12, 0, 1)`: the offset, in stops.
pub const CC7_LOG_OFFSET_STOPS: i64 = 8;
/// `v(x) = clamp((log2(x) + 8) / 12, 0, 1)`: the span, in stops.
pub const CC7_LOG_SPAN_STOPS: i64 = 12;
/// The curve's floor, `2^-8 = 0.003 906 25`, in millionths. Every linear value
/// below it stores `v = 0`.
pub const CC7_LOG_FLOOR_LINEAR_MILLIONTHS: i64 = 3_906;

/// CC7 §2.4.2's curve, in `f64`.
///
/// **The curve is fed the analytic grade709 linear, not the decoded 8-bit
/// code**: feeding the decoded code gives `skin_light 160,146,139` and
/// `deep_shadow 33`, which is visibly different and wrong.
#[must_use]
pub fn cc7_log_value(linear: f64) -> f64 {
    if linear <= 0.0 {
        return 0.0;
    }
    let offset = cc7_as_f64(CC7_LOG_OFFSET_STOPS);
    let span = cc7_as_f64(CC7_LOG_SPAN_STOPS);
    ((linear.log2() + offset) / span).clamp(0.0, 1.0)
}

/// CC7 §2.2's `cc7_log_encode_code`: one linear value, in millionths, as the
/// stored log-carrier display code `round(255 · v)`.
#[must_use]
pub fn cc7_log_encode_code(linear_millionths: i64) -> u8 {
    cc7_display_code(cc7_log_value(cc7_as_f64(linear_millionths) / 1_000_000.0))
}

/// CC7 §2.2's `cc7_log_inverse_display`: the **exact** inverse of the curve,
/// `display709(2^(12v - 8))`, in millionths. No lattice is involved.
#[must_use]
pub fn cc7_log_inverse_display(v_millionths: i64) -> i64 {
    let offset = cc7_as_f64(CC7_LOG_OFFSET_STOPS);
    let span = cc7_as_f64(CC7_LOG_SPAN_STOPS);
    let value = cc7_as_f64(v_millionths) / 1_000_000.0;
    cc7_millionths(cc7_encode_bt709(2.0_f64.powf(span * value - offset)))
}

/// `v(1.0) = 2/3` in millionths, CC7 §2.4.2's unity anchor.
pub const CC7_LOG_UNITY_ANCHOR_MILLIONTHS: i64 = 666_667;
/// The stored code at unity.
pub const CC7_LOG_UNITY_ANCHOR_CODE: u8 = 170;
/// `v(0.18) = 0.460 506` in millionths, CC7 §2.4.2's 18 % grey anchor. The
/// brief's `0.4589` did not satisfy its own formula and is superseded.
pub const CC7_LOG_MID_GREY_ANCHOR_MILLIONTHS: i64 = 460_506;
/// The stored code at 18 % grey.
pub const CC7_LOG_MID_GREY_ANCHOR_CODE: u8 = 117;

/// The twelve achromatic chart patches' **linear** values, in millionths:
/// `decode_display709(code / 255)`, the input CC7 §2.4.2's curve is fed.
pub const CC7_CHART_LINEAR_MILLIONTHS: [i64; CC7_CHART_PATCH_COUNT] = [
    0, 9_586, 20_981, 50_697, 95_172, 179_084, 261_482, 361_292, 500_507, 665_016, 899_828,
    1_000_000,
];

/// CC7 §2.4.2's stored log codes for the twelve achromatic chart patches.
pub const CC7_LOG_CHART_CODES: [u8; CC7_CHART_PATCH_COUNT] =
    [0, 28, 52, 79, 98, 117, 129, 139, 149, 157, 167, 170];

/// The same twelve codes back through the **exact** inverse (no lattice).
pub const CC7_LOG_CHART_INVERSE_CODES: [u8; CC7_CHART_PATCH_COUNT] =
    [4, 11, 24, 48, 72, 103, 128, 153, 181, 206, 243, 255];

/// CC7 §2.4.2's error column, `inverse − source`.
///
/// **The black patch's `+4` is a property of the curve, not of a LUT**
/// (A2): `v = 0` inverts to `2^-8` linear, which monitors as code 4, and no
/// lattice size changes that because the forward curve is not invertible at
/// zero.
pub const CC7_LOG_CHART_INVERSE_ERROR_CODES: [i64; CC7_CHART_PATCH_COUNT] =
    [4, 0, 0, 0, 0, -1, 0, 1, 1, -2, 1, 0];

/// CC7 §2.4.2's seven row patches through the same curve.
pub const CC7_LOG_ROW_CODES: [[u8; 3]; CC7_ROW_PATCH_COUNT] = [
    [160, 147, 139], // skin_light
    [150, 132, 121], // skin_medium
    [134, 113, 101], // skin_tan
    [104, 81, 69],   // skin_deep
    [156, 54, 60],   // product_red
    [54, 144, 152],  // product_cyan
    [32, 32, 32],    // deep_shadow
];

/// The surround through the same curve.
pub const CC7_LOG_SURROUND_CODE: u8 = 123;

// ===========================================================================
// CC7 §2.3.6: scenario (f) raster, sampling, and analytic centres.
// ===========================================================================

/// The tracked square's side, in pixels.
pub const CC7_TRACK_SQUARE_SIZE: i64 = 24;
/// The square path's x centre, in pixels.
pub const CC7_TRACK_CENTRE_X_PIXELS: i64 = 148;
/// The square path's x amplitude, in pixels (the brief's, kept by A12).
pub const CC7_TRACK_AMPLITUDE_X_PIXELS: i64 = 100;
/// The square path's y centre, in pixels.
pub const CC7_TRACK_CENTRE_Y_PIXELS: i64 = 78;
/// The square path's y amplitude, in pixels.
pub const CC7_TRACK_AMPLITUDE_Y_PIXELS: i64 = 40;
/// The four static skin patches of the (f) raster sit at `y 4..20`.
pub const CC7_TRACK_STATIC_PATCH_TOP: i64 = 4;
/// …and end at `y 20`, which is why the generator asserts `y(f) >= 24`.
pub const CC7_TRACK_STATIC_PATCH_BOTTOM: i64 = 20;

/// First occluded frame: the square is not drawn on `43..=47`.
pub const CC7_TRACK_OCCLUSION_FIRST_FRAME: i64 = 43;
/// Last occluded frame.
pub const CC7_TRACK_OCCLUSION_LAST_FRAME: i64 = 47;

/// The (f) call's `start_local_frame`.
pub const CC7_TRACK_RANGE_START_LOCAL_FRAME: i64 = 0;
/// The (f) call's `end_local_frame`, **exclusive** (A12).
///
/// **The range ends at the occlusion on purpose**: `track_matte_window` has no
/// re-acquisition, so a range that continues past frame 47 returns frozen
/// positions at confidence `10 000` — measured up to
/// [`CC7_TRACK_NO_REACQUISITION_DRIFT_BASIS_POINTS`] from the subject by frame
/// 74. No CC7 gate may span an occlusion.
pub const CC7_TRACK_RANGE_END_LOCAL_FRAME: i64 = 48;
/// The (f) call's `step_frames`.
pub const CC7_TRACK_STEP_FRAMES: i64 = 5;
/// The (f2) total-loss recipe's `step_frames`, whose sample set is `{0, 47}`.
pub const CC7_TRACK_F2_STEP_FRAMES: i64 = 47;
/// The (f) call's `search_radius_percent`, pinned with its reason (A18): the
/// per-sample motion is ≤ 25 thumbnail px, inside the 10 % radius of 25.6 px
/// on a 256-wide thumbnail, and probe-2 measured every observation,
/// confidence and keyframe **bit-identical** at 10 % and 25 %.
pub const CC7_TRACK_SEARCH_RADIUS_PERCENT: i64 = 10;
/// The (f) call's `max_width`, the tracking thumbnail width.
pub const CC7_TRACK_MAX_WIDTH: i64 = 256;

/// The eleven samples the tool distributes over `0..48` at step 5.
pub const CC7_TRACK_SAMPLE_COUNT: usize = 11;
/// The ten samples that survive the confidence floor.
pub const CC7_TRACK_SURVIVING_SAMPLE_COUNT: usize = 10;

/// CC7 §2.3.6's independent transcription of `tracking_sample_frames`
/// (`crates/kinewright-agent/src/server.rs:11810-11845`).
///
/// The tool does **not** step by `step_frames`: it treats `step` as a
/// *maximum* spacing, distributes `ceil(span / step)` intervals evenly over
/// `start ..= end − 1` as `f_i = start + floor(span · i / interval_count)`, and
/// appends `last`. A naive `start + k · step` gives `0, 5, 10, …` and is a
/// different list, which is the recipe error A12 corrects.
#[must_use]
pub fn cc7_tracking_sample_frames_for(start: i64, end: i64, step: i64) -> Vec<i64> {
    let Some(last) = end.checked_sub(1) else {
        return Vec::new();
    };
    if last < start {
        return Vec::new();
    }
    if last == start {
        return vec![start];
    }
    let span = i128::from(last) - i128::from(start);
    let requested_step = i128::from(step.max(1));
    let interval_count = ((span + requested_step - 1) / requested_step).max(1);
    let mut frames = Vec::new();
    for index in 0..=interval_count {
        let frame = if index == interval_count {
            last
        } else {
            let offset = span * index / interval_count;
            i64::try_from(i128::from(start) + offset).unwrap_or(last)
        };
        frames.push(frame);
    }
    frames
}

/// CC7 §2.3.6's `CC7_TRACK_SAMPLE_FRAMES`, the eleven local frames the (f)
/// call samples, asserted equal to `observations[].local_frame`.
#[must_use]
pub const fn cc7_tracking_sample_frames() -> [i64; CC7_TRACK_SAMPLE_COUNT] {
    [0, 4, 9, 14, 18, 23, 28, 32, 37, 42, 47]
}

/// The (f2) recipe's two samples.
pub const CC7_TRACK_F2_SAMPLE_FRAMES: [i64; 2] = [0, 47];

/// The one sample the confidence floor drops: the frame inside the occlusion.
pub const CC7_TRACK_EXPECTED_LOW_CONFIDENCE_FRAMES: [i64; 1] = [47];

/// The ten surviving samples, in order.
pub const CC7_TRACK_SURVIVING_SAMPLE_FRAMES: [i64; CC7_TRACK_SURVIVING_SAMPLE_COUNT] =
    [0, 4, 9, 14, 18, 23, 28, 32, 37, 42];

/// The last surviving keyframe, excluded from the containment gate by name
/// because it carries the tool's published `known_systematic_lag` (A17).
pub const CC7_TRACK_LAGGING_FINAL_KEYFRAME: i64 = 42;

/// CC7 §2.3.6's square path: `x(f) = round(148 + 100 · sin(2π f / 100))`,
/// `y(f) = round(78 + 40 · sin(2π f / 100))`, half away from zero.
#[must_use]
pub fn cc7_analytic_square_top_left(frame: i64) -> (i64, i64) {
    let frames = f64::from(CC7_TRACK_FRAMES);
    let angle = 2.0 * std::f64::consts::PI * cc7_as_f64(frame) / frames;
    let sine = angle.sin();
    (
        cc7_round_half_away_from_zero(
            cc7_as_f64(CC7_TRACK_CENTRE_X_PIXELS) + cc7_as_f64(CC7_TRACK_AMPLITUDE_X_PIXELS) * sine,
        ),
        cc7_round_half_away_from_zero(
            cc7_as_f64(CC7_TRACK_CENTRE_Y_PIXELS) + cc7_as_f64(CC7_TRACK_AMPLITUDE_Y_PIXELS) * sine,
        ),
    )
}

/// The square's centre in basis points of the composite,
/// `round(cx · 10000/320)` and `round(cy · 10000/180)` with CC7 §10.1's half
/// away from zero rule — load-bearing at frames 18, 28 and 32, where the exact
/// value is `…12.5` bp.
#[must_use]
pub fn cc7_analytic_square_centre_basis_points(frame: i64) -> (i64, i64) {
    let (x, y) = cc7_analytic_square_top_left(frame);
    let half = CC7_TRACK_SQUARE_SIZE / 2;
    (
        cc7_round_half_away_from_zero(
            cc7_as_f64(x + half) * 10_000.0 / f64::from(CC7_SOURCE_WIDTH),
        ),
        cc7_round_half_away_from_zero(
            cc7_as_f64(y + half) * 10_000.0 / f64::from(CC7_SOURCE_HEIGHT),
        ),
    )
}

/// Whether the square is drawn at `frame`: it is not, on `43..=47`.
#[must_use]
pub const fn cc7_square_is_drawn(frame: i64) -> bool {
    frame < CC7_TRACK_OCCLUSION_FIRST_FRAME || frame > CC7_TRACK_OCCLUSION_LAST_FRAME
}

/// CC7 §2.3.6's analytic centre table, one pair per sampled frame.
pub const CC7_TRACK_ANALYTIC_CENTRES_BASIS_POINTS: [[i64; 2]; CC7_TRACK_SAMPLE_COUNT] = [
    [5000, 5000], // frame 0, top-left (148, 78)
    [5781, 5556], // frame 4, top-left (173, 88)
    [6688, 6167], // frame 9, top-left (202, 99)
    [7406, 6722], // frame 14, top-left (225, 109)
    [7813, 7000], // frame 18, top-left (238, 114)
    [8094, 7222], // frame 23, top-left (247, 118)
    [8063, 7167], // frame 28, top-left (246, 117)
    [7813, 7000], // frame 32, top-left (238, 114)
    [7281, 6611], // frame 37, top-left (221, 107)
    [6500, 6056], // frame 42, top-left (196, 97)
    [5594, 5389], // frame 47, top-left (167, 85) (occluded)
];

/// The window the (f) node seeds, centred on frame 0's square: `center_x` and
/// `center_y` resolve to their descriptor neutral `5_000`, and the half
/// extents are `round(12/320 · 10000)` and `round(12/180 · 10000)`.
pub const CC7_TRACK_SEEDED_WINDOW_HALF_WIDTH_BASIS_POINTS: i64 = 375;
/// See [`CC7_TRACK_SEEDED_WINDOW_HALF_WIDTH_BASIS_POINTS`].
pub const CC7_TRACK_SEEDED_WINDOW_HALF_HEIGHT_BASIS_POINTS: i64 = 667;
/// Frame 0's square is exactly `x 148..172, y 78..102`, whose continuous
/// centre is `(160.0, 90.0)` px = `(5 000, 5 000)` bp exactly.
pub const CC7_TRACK_SEEDED_WINDOW_CENTRE_BASIS_POINTS: [i64; 2] = [5_000, 5_000];

/// The **1.5×** containment window (A17): 18 px, `round(18·10000/320) = 563`.
///
/// The seeded 1.0× window does not contain the moving square once tracked —
/// probe-2 measured the worst required half-extent at 14.77 px in x, so the
/// 12 px window is 2.77 px short.
pub const CC7_TRACK_WINDOW_HALF_WIDTH_BASIS_POINTS: i64 = 563;
/// See [`CC7_TRACK_WINDOW_HALF_WIDTH_BASIS_POINTS`]; `18 · 10000/180` exactly.
pub const CC7_TRACK_WINDOW_HALF_HEIGHT_BASIS_POINTS: i64 = 1_000;

/// The raw observed centres probe-2 measured on the (f) recipe (P2), in layer
/// space, one pair per sampled frame.
///
/// **Regression pins, and the contract says so** (R-M8, CC7 §5.1(4)): these
/// are what `track_matte_window`'s normalized-SAD template match produces, not
/// an independent derivation. The *analytic* centres are
/// [`CC7_TRACK_ANALYTIC_CENTRES_BASIS_POINTS`]; the gate is that the two agree
/// within [`CC7_TRACK_TOLERANCE_BASIS_POINTS`] on every surviving sample.
pub const CC7_TRACK_OBSERVED_CENTRES_BASIS_POINTS: [[i64; 2]; CC7_TRACK_SAMPLE_COUNT] = [
    [5_020, 5_035], // frame 0
    [5_801, 5_590], // frame 4
    [6_699, 6_215], // frame 9
    [7_402, 6_771], // frame 14
    [7_793, 7_049], // frame 18
    [8_066, 7_257], // frame 23
    [8_027, 7_188], // frame 28
    [7_793, 7_049], // frame 32
    [7_246, 6_632], // frame 37
    [6_465, 6_076], // frame 42
    [6_465, 6_076], // frame 47, occluded: the frozen pre-occlusion position
];

/// The confidences probe-2 measured at each sample (P2).
pub const CC7_TRACK_OBSERVED_CONFIDENCE_BASIS_POINTS: [i64; CC7_TRACK_SAMPLE_COUNT] = [
    10_000, 10_000, 9_869, 9_859, 9_862, 9_870, 9_865, 9_740, 9_740, 10_000, 7_349,
];

/// CC7's independent transcription of `median_filtered_observations` +
/// `reactive_focus_values`, owned by
/// `crates/kinewright-core/src/multicam.rs:1171-1236` and reached by
/// `track_matte_window` at `crates/kinewright-agent/src/server.rs:4611-4620`.
///
/// The transcription exists so the (f) keyframes are derived rather than
/// copied out of a tool response; `cc7_the_keyframe_smoother_transcription_matches_core`
/// asserts it against the owner in both directions.
#[must_use]
pub fn cc7_stabilized_centres(observations: &[i64], maximum_step: i64) -> Vec<i64> {
    let mut filtered = observations.to_vec();
    for index in 1..observations.len().saturating_sub(1) {
        let mut window = [
            observations[index - 1],
            observations[index],
            observations[index + 1],
        ];
        window.sort_unstable();
        filtered[index] = window[1];
    }
    if observations.len() >= 3 {
        let last = observations.len() - 1;
        let mut window = [
            observations[last - 2],
            observations[last - 1],
            observations[last],
        ];
        window.sort_unstable();
        filtered[last] = window[1];
    }
    let Some(first) = filtered.first().copied() else {
        return Vec::new();
    };
    let minimum = crate::MATTE_WINDOW_CENTER_MIN_BASIS_POINTS;
    let maximum = crate::MATTE_WINDOW_CENTER_MAX_BASIS_POINTS;
    let mut focus = first.clamp(minimum, maximum);
    filtered
        .iter()
        .copied()
        .map(|subject| {
            // CC5 §5.2's dead zone is deliberately zero, so `desired` is the
            // clamped subject at every sample.
            let desired = subject.clamp(minimum, maximum);
            focus = focus
                .saturating_add((desired - focus).clamp(-maximum_step, maximum_step))
                .clamp(minimum, maximum);
            focus
        })
        .collect()
}

/// `MATTE_TRACK_MAX_STEP_BASIS_POINTS`, restated because it is `pub(crate)` in
/// `crates/kinewright-agent/src/server.rs:11196` and core cannot see it.
///
/// It is reached exactly once on this path — the `4 → 9` segment, raw Δx 898 bp
/// clamped to 800 — and the clamp self-corrects at the next sample at a net
/// cost of ≤ 98 bp to the smoothed curve (A12).
pub const CC7_TRACK_MAX_STEP_BASIS_POINTS_RESTATED: i64 = 800;

/// The smoothed keyframe values the (f) commit writes, per axis, over
/// [`CC7_TRACK_SURVIVING_SAMPLE_FRAMES`].
///
/// Derived from the pinned raw observations through
/// [`cc7_stabilized_centres`], never copied from a tool response. The final
/// keyframe is the tool's published `known_systematic_lag`: the three-sample
/// median filter replaces the last value with `median(o[n-3], o[n-2], o[n-1])`,
/// so frame 42 is written as `7 246` instead of `6 465` — 746 bp off, for a
/// documented reason that has nothing to do with tracking quality (A17).
#[must_use]
pub fn cc7_track_keyframe_centres(axis: usize) -> Vec<i64> {
    let raw = CC7_TRACK_OBSERVED_CENTRES_BASIS_POINTS
        .iter()
        .take(CC7_TRACK_SURVIVING_SAMPLE_COUNT)
        .map(|centre| centre[axis.min(1)])
        .collect::<Vec<_>>();
    cc7_stabilized_centres(&raw, CC7_TRACK_MAX_STEP_BASIS_POINTS_RESTATED)
}

// ===========================================================================
// CC7 §2.6: budget constants.
// ===========================================================================
//
// Every threshold CC7 gates on is a `SCREAMING_SNAKE` constant here with its
// unit in the name. **No CC7 gate uses a literal, and no CC7 constant is a
// float**: fractional terms are `_MILLIONTHS`, rates are `_BASIS_POINTS`,
// angles are `_CENTIDEGREES`, counts are plain integers with `_PIXELS` or
// `_CODE`.

/// (a)(2) / (b1): the post-match achromatic spread budget, in 8-bit
/// monitoring codes (A8, A15).
///
/// **The budget is 5, not 6**: probe-2 measured the *unmatched* cam B at
/// exactly 6 on the amended twelve-patch band, so a `≤ 6` gate would have
/// passed its own failing-direction fixture.
pub const CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE: i64 = 5;
/// (b1): the residual achromatic spread budget for the wrong-white-balance
/// recovery, in 8-bit monitoring codes (C-E7).
///
/// **Measured 3** on the canonical (b1) document — the corrected C1 clip built
/// from `CC7_B1_OPERATIONS` on the amended twelve-patch band — for a **2.0x**
/// margin. Both failing directions clear it wide: uncorrected C1 measures
/// **7** and corrected C2 **19**.
///
/// Deliberately **one code above** the (a) budget, because (b1) is a harder
/// recovery: (a) matches a candidate against a reference that was shot on the
/// same chart, while (b1) recovers a clip that arrives wrong-balanced *and*
/// underexposed from a planner proposal alone. It is a **separate** constant
/// rather than a widening of [`CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE`], which must
/// stay at 5: 6 is exactly what the *unmatched* cam B measures, so a shared
/// budget of 6 would admit (a)'s own failing direction (A15).
pub const CC7_B1_RESIDUAL_SPREAD_MAX_CODE: i64 = 6;
/// (a)(3): the chart-band luma mean delta budget, in code millionths (A8).
pub const CC7_MATCH_LUMA_MEAN_MAX_CODE_MILLIONTHS: i64 = 5_000_000;
/// (c)(1): the log carrier's luma first-percentile floor, in **16-bit** codes
/// (A9, A21).
///
/// `analyze_color_shot` serializes `scope_statistics.luma.first_percentile` as
/// `8-bit × 257` (`scopes.rs:576-586`, `:1330-1339`); `mean_code_values.luma`
/// is an 8-bit *mean* and is the wrong field. Left in 8 bits the p1 gate would
/// have passed on every source and the p99 gate failed on every source.
pub const CC7_LOG_FIRST_PERCENTILE_MIN_CODE16: i64 = 5_140;
/// (c)(1): the log carrier's luma 99th-percentile ceiling, in 16-bit codes.
pub const CC7_LOG_P99_MAX_CODE16: i64 = 51_400;
/// (c)(2): the set-wide worst monitoring-code error of the inverse `.cube`
/// over the twelve achromatic plus four skin patches (A2, A22).
pub const CC7_LOG_INVERSE_MAX_CODE: i64 = 12;
/// (c)(3): the pinned lattice size. Read as a *selection rule* the sweep would
/// choose 33 at a 1.7× margin, so the contract pins the size and requires the
/// sweep to be monotone non-increasing with size 17 genuinely failing (A22).
pub const CC7_LOG_CUBE_SIZE: u32 = 65;
/// (d2): the tolerance on the discrete pixel-centre feather model, absorbing
/// the basis-point quantization of `cx/cy/hw/hh` at a boundary (A7).
pub const CC7_FEATHER_PARTIAL_TOLERANCE_PIXELS: i64 = 4;
/// (e)(2): the exact out-of-gamut population of the `deep_shadow` ROI under
/// the built-in `warm` look, `12 × 16` (A3).
pub const CC7_LOOK_DEEP_SHADOW_OUT_OF_GAMUT_PIXELS: i64 = 192;
/// (f)(1): the confidence floor, pinned **between two populations** (A14).
///
/// It sits `+1 089 bp` above the measured occluded maximum `7 411` and
/// `−1 240 bp` below the measured clean minimum `9 740`, on a `2 329 bp`
/// separation. The tracker default `5 000` drops nothing, which is why the
/// default must not be reused.
pub const CC7_TRACK_MIN_CONFIDENCE_BASIS_POINTS: i64 = 8_500;
/// (f)(2): the observation-accuracy tolerance, CC5's `200`, reused unchanged
/// against a measured worst clean raw observation error of 49 bp (A14).
pub const CC7_TRACK_TOLERANCE_BASIS_POINTS: i64 = 200;

// --- Reported, never gated -------------------------------------------------

/// The corrected C2 residual spread — the compromise the human is asked
/// about, and (a)(2)'s second failing direction (A15; was 17 on the six-patch
/// grey ROI).
pub const CC7_UNRECOVERABLE_RESIDUAL_SPREAD_REPORTED_CODE: i64 = 19;
/// The corrected C2 clip's skin `in_band_basis_points`, still above
/// `SKIN_BAND_EXCEPTION_BASIS_POINTS = 5_000`, so no Info exception fires (A8).
///
/// Re-pinned from §2.6's 9 411 to the amended-scene measurement (Implementer C
/// erratum C-E9: `considered_pixel_count 768`, `excluded_achromatic_pixel_count
/// 0`, so every considered pixel is in band). The manifest carries this one
/// number and no second copy (R4-m6).
pub const CC7_C2_SKIN_IN_BAND_REPORTED_BASIS_POINTS: i64 = 10_000;
/// The corrected C2 over-range population, **on the blue channel only** (A16).
pub const CC7_C2_OVER_RANGE_PIXELS_REPORTED: i64 = 672;
/// The same, as a rate over the raster (A16; was 22 on the pre-A1 scene).
pub const CC7_C2_OVER_RANGE_BASIS_POINTS_REPORTED: i64 = 116;
/// The `warm` look's whole-raster out-of-gamut count (A19; 1 608 pre-A1 — the
/// 128-pixel difference is exactly the removed pure-red patch).
pub const CC7_WARM_WHOLE_RASTER_OUT_OF_GAMUT_PIXELS_REPORTED: i64 = 1_480;
/// The same, as a rate over the raster (A19; 279 pre-A1).
pub const CC7_WARM_WHOLE_RASTER_OUT_OF_GAMUT_BASIS_POINTS: i64 = 256;
/// The black patch's inverse error, at **every** lattice size (A22).
pub const CC7_LOG_BLACK_PATCH_REPORTED_CODE: i64 = 4;
/// The saturated primaries' inverse error at size 65 — a sub-percent `e` error
/// amplified through the exponential. A2 excludes them from the gate set (A22).
pub const CC7_LOG_PRIMARY_REPORTED_CODE: i64 = 5;
/// The set-wide worst under an identity 33³ cube, (c)(2)'s failing direction.
pub const CC7_LOG_IDENTITY_CUBE_REPORTED_CODE: i64 = 85;
/// The canonical 65³ `.cube`'s size in bytes, 44.2 % of `LUT_MAX_FILE_BYTES`.
pub const CC7_LOG_CUBE_BYTES_REPORTED: i64 = 7_414_990;
/// The highest confidence any occluded sample reached (A14).
pub const CC7_TRACK_OCCLUDED_CONFIDENCE_MAX_REPORTED: i64 = 7_411;
/// The lowest confidence any clean sample reached (A14).
pub const CC7_TRACK_CLEAN_CONFIDENCE_MIN_REPORTED: i64 = 9_740;
/// The occluded sample's confidence on the (f) recipe itself.
pub const CC7_TRACK_OCCLUDED_CONFIDENCE_ON_THE_RECIPE_REPORTED: i64 = 7_349;
/// The occluded sample's confidence on the (f2) recipe.
pub const CC7_TRACK_F2_OCCLUDED_CONFIDENCE_REPORTED: i64 = 7_309;
/// The worst clean raw observation error probe-2 measured (P2).
pub const CC7_TRACK_WORST_RAW_OBSERVATION_ERROR_BASIS_POINTS_REPORTED: i64 = 49;
/// The smoothed curve's final-keyframe lag, the tool's published
/// `known_systematic_lag` (A17). Every tracking gate reads `observations[]`.
pub const CC7_TRACK_FINAL_KEYFRAME_LAG_BASIS_POINTS_REPORTED: i64 = 746;
/// How far the frozen post-occlusion centre drifts from the subject by frame
/// 74 on a `0..100` range — the reason no CC7 gate spans the occlusion (A12).
pub const CC7_TRACK_NO_REACQUISITION_DRIFT_BASIS_POINTS: i64 = 5_176;
/// (f)(3)'s measured **required** half-extent in x, in pixel hundredths.
///
/// Re-pinned from §2.3.6's 1 477 to C-E6's measured 1 478 (14.784 px); the
/// containment gate keeps its two-hundredth window (R4-m11).
pub const CC7_TRACK_CONTAINMENT_REQUIRED_HALF_WIDTH_PIXELS_REPORTED: i64 = 1_478;
/// (f)(3)'s measured required half-extent in y, in pixel hundredths.
pub const CC7_TRACK_CONTAINMENT_REQUIRED_HALF_HEIGHT_PIXELS_REPORTED: i64 = 1_288;
/// The 1.5× window's worst x margin, in pixel hundredths.
pub const CC7_TRACK_CONTAINMENT_WORST_MARGIN_X_PIXELS_HUNDREDTHS: i64 = 323;
/// The 1.5× window's worst y margin, in pixel hundredths.
///
/// Re-pinned from §2.3.6's 512 to C-E6's measured 511 (5.118 px against the
/// window's 18.000 px); the containment gate keeps its two-hundredth window
/// (R4-m11).
pub const CC7_TRACK_CONTAINMENT_WORST_MARGIN_Y_PIXELS_HUNDREDTHS: i64 = 511;

// --- Exact constants, derived rather than measured -------------------------

/// (d2): the feather, in basis points of the window's own half-extents.
pub const CC7_FEATHER_BASIS_POINTS: i64 = 1_000;
/// (d), (d2), (f): the secondary's saturation move.
pub const CC7_SECONDARY_SATURATION_PERCENT: i64 = 40;
/// (e): the look's mix — the descriptor neutral, and therefore never stored.
pub const CC7_LOOK_MIX_BASIS_POINTS: i64 = 10_000;
/// The built-in `warm` look's blue zero crossing, as a **display709** value in
/// millionths — the name carries its encoding because the number is not a
/// linear one (CC7 §2.6's unit rule).
///
/// `Warm[2] = (e2 − 0.5)·1.08 + 0.46` (`builtin_looks.rs:167-176`), so the
/// output is negative for `e2 < 0.5 − 0.46/1.08 = 0.074_074_1`.
pub const CC7_LOOK_BLUE_ZERO_CROSSING_DISPLAY709_MILLIONTHS: i64 = 74_074;
/// The same crossing in scene-linear light, `74_074 / 4.5` millionths.
pub const CC7_LOOK_BLUE_ZERO_CROSSING_LINEAR_MILLIONTHS: i64 = 16_461;
/// (a)(4), (d)(3): the skin band rate over the **considered (chromatic)**
/// pixels, exact on both cam A and matched cam B.
pub const CC7_SKIN_IN_BAND_EXACT_BASIS_POINTS: i64 = 10_000;
/// (d)(2): no tolerance may excuse one changed pixel outside the matte.
pub const CC7_MATTE_OUTSIDE_CHANGED_PIXELS_MAX: i64 = 0;
/// (g)(1): the one exception code a conforming H.264 export always carries,
/// because the format has no white-point field (A6).
pub const CC7_DELIVERY_ALLOWED_INFO_CODES: [&str; 1] = ["delivery_tag_not_representable"];
/// (g): the CI delivery leg's Linux budget, in seconds (A10).
pub const CC7_DELIVERY_LEG_BUDGET_SECONDS_LINUX: i64 = 90;

// --- (d) qualifier and (d2) window, CC7 §2.5 -------------------------------

/// `MATTE_SAMPLE_HUE_WIDTH_CENTIDEGREES`, restated: it is `pub(crate)` in
/// `crates/kinewright-agent/src/color_status.rs:4390` and core cannot see it.
pub const CC7_MATTE_SAMPLE_HUE_WIDTH_CENTIDEGREES: i64 = 1_500;
/// `MATTE_SAMPLE_SOFTNESS`, restated (`color_status.rs:4392`).
pub const CC7_MATTE_SAMPLE_SOFTNESS: i64 = 1_000;
/// `MATTE_SAMPLE_BAND_MARGIN_BASIS_POINTS`, restated (`color_status.rs:4394`).
pub const CC7_MATTE_SAMPLE_BAND_MARGIN_BASIS_POINTS: i64 = 1_000;
/// The `product_red` sample's median hue, measured over 192 visible, 192
/// chromatic, 0 achromatic pixels (P5).
pub const CC7_PRODUCT_SAMPLE_HUE_MEDIAN_CENTIDEGREES: i64 = 35_865;
/// The sample's saturation p10 and p90, which coincide on a flat patch (P5).
pub const CC7_PRODUCT_SAMPLE_SATURATION_BASIS_POINTS: i64 = 8_728;
/// The sample's luma p10 and p90 (P5).
pub const CC7_PRODUCT_SAMPLE_LUMA_BASIS_POINTS: i64 = 2_513;

/// (d2)'s window: the `product_red` patch's own rect in basis points, which
/// resolves to `cx = 53.984`, `cy = 83.988`, `hw = 5.984`, `hh = 7.992` px.
pub const CC7_D2_WINDOW_CENTRE_BASIS_POINTS: [i64; 2] = [1_687, 4_666];
/// See [`CC7_D2_WINDOW_CENTRE_BASIS_POINTS`].
pub const CC7_D2_WINDOW_HALF_EXTENTS_BASIS_POINTS: [i64; 2] = [187, 444];
/// (d2)'s analytic discrete pixel-centre counts, `full / covered / partial`.
///
/// **The continuous-area formula `4·hw·hh·((1+f)² − (1−f)²) = 76.8` is the
/// wrong model** — it is wrong by 35 pixels on this window (31 %) — and CC7
/// names it so no reader re-derives it (A7).
pub const CC7_D2_FEATHER_COUNTS_PIXELS: [i64; 3] = [140, 252, 112];
/// The wrong model's value, asserted **not** to match (A7).
pub const CC7_D2_CONTINUOUS_AREA_WRONG_MODEL_PIXELS_TENTHS: i64 = 768;

// ===========================================================================
// CC7 §2.5: the canonical operations per scenario.
// ===========================================================================

/// The reference clip of a two-clip (a)/(b) document, always camera A.
pub const CC7_REFERENCE_CLIP_ID: ClipId = ClipId(1);
/// The candidate clip of a two-clip (a)/(b) document.
pub const CC7_CANDIDATE_CLIP_ID: ClipId = ClipId(2);
/// The single clip of a (c)/(d)/(e)/(f) document.
pub const CC7_SINGLE_CLIP_ID: ClipId = ClipId(1);
/// The effect id `next_effect_id` allocates on a document with no effects.
pub const CC7_NODE_EFFECT_ID: EffectId = EffectId(1);
/// The index `stage_insert_index` returns for the first node of an empty
/// stack (CC4 §3.2: a new node is inserted at the first stage-legal index).
pub const CC7_NODE_INSERT_INDEX: usize = 0;
/// The LUT asset id `next_lut_asset_id` allocates on a fresh project.
pub const CC7_LUT_ASSET_ID: LutAssetId = LutAssetId(1);

/// The `primary_correction` effect name (`effect.rs:1492-1495`).
pub const CC7_PRIMARY_CORRECTION_EFFECT: &str = "primary_correction";
/// The `technical_lut` effect name (`effect.rs:1504-1506`).
pub const CC7_TECHNICAL_LUT_EFFECT: &str = "technical_lut";
/// The `creative_look` effect name (`effect.rs:1507-1509`).
pub const CC7_CREATIVE_LOOK_EFFECT: &str = "creative_look";

/// The proposal `match_parameters` produces for one candidate.
///
/// **These are regression pins** (R-M8): they are exactly what
/// `crates/kinewright-agent/src/color_scopes.rs:1860-1965` produces, measured
/// by an independent `f64` transcription of that function and never by calling
/// the tool and writing down its answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cc7MatchProposal {
    pub exposure_milli_stops: i64,
    pub temperature_percent: i64,
    pub tint_percent: i64,
    /// Whether `temperature_percent` was clamped to the descriptor bound.
    pub temperature_clamped: bool,
    /// The unrounded temperature delta before the clamp, when it clamped.
    pub temperature_unrounded_delta: Option<i64>,
}

/// Cam B on the amended twelve-patch achromatic ROI (P2).
pub const CC7_MATCH_PROPOSAL_B: Cc7MatchProposal = Cc7MatchProposal {
    exposure_milli_stops: 477,
    temperature_percent: -45,
    tint_percent: 6,
    temperature_clamped: false,
    temperature_unrounded_delta: None,
};
/// Cam C1 on the amended twelve-patch achromatic ROI, measured live at CC7
/// §12 step 5 (errata D-E5): `unrounded_delta 1464.54 / 80.82`, and the tint
/// delta rounds to `0`, so `match_parameters` **omits** `tint_percent` from the
/// proposal entirely (`color_scopes.rs:1897-1903`). A `0` here means "not
/// proposed", and `CC7_B1_OPERATIONS` accordingly carries two controls.
pub const CC7_MATCH_PROPOSAL_C1: Cc7MatchProposal = Cc7MatchProposal {
    exposure_milli_stops: 1_465,
    temperature_percent: 81,
    tint_percent: 0,
    temperature_clamped: false,
    temperature_unrounded_delta: None,
};
/// Cam C2 on the amended twelve-patch achromatic ROI (P2): the temperature
/// control clamps at `+100` from a raw `+248`.
pub const CC7_MATCH_PROPOSAL_C2: Cc7MatchProposal = Cc7MatchProposal {
    exposure_milli_stops: 2_410,
    temperature_percent: 100,
    tint_percent: -30,
    temperature_clamped: true,
    temperature_unrounded_delta: Some(248),
};

/// One canonical node: an effect name and the parameters the commit stores.
///
/// A parameter equal to its descriptor neutral is **not stored** — the
/// planners filter it (`color_status.rs:4789-4795`) and the app builders never
/// write it — so a neutral value never appears in this table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cc7Operation {
    pub effect_name: &'static str,
    pub parameters: &'static [(&'static str, i64)],
}

/// Scenario (a): one `primary_correction` on clip 2, no `saturation_percent`,
/// no operation on clip 1.
pub const CC7_A_OPERATIONS: [Cc7Operation; 1] = [Cc7Operation {
    effect_name: CC7_PRIMARY_CORRECTION_EFFECT,
    parameters: &[
        ("exposure_milli_stops", 477),
        ("temperature_percent", -45),
        ("tint_percent", 6),
    ],
}];

/// Scenario (b1), the recoverable candidate.
pub const CC7_B1_OPERATIONS: [Cc7Operation; 1] = [Cc7Operation {
    effect_name: CC7_PRIMARY_CORRECTION_EFFECT,
    parameters: &[("exposure_milli_stops", 1_465), ("temperature_percent", 81)],
}];

/// Scenario (b2), the candidate beyond the planner's authority: the
/// `temperature_percent` value **is** the clamp.
pub const CC7_B2_OPERATIONS: [Cc7Operation; 1] = [Cc7Operation {
    effect_name: CC7_PRIMARY_CORRECTION_EFFECT,
    parameters: &[
        ("exposure_milli_stops", 2_410),
        ("temperature_percent", 100),
        ("tint_percent", -30),
    ],
}];

/// Scenario (c): one `technical_lut` at the input stage carrying **only**
/// `lut_asset_id`. `input_encoding_token = 0` is the descriptor neutral and is
/// not stored (`color_status.rs:4205-4216`), and `mix_basis_points` is pinned
/// at its neutral `10_000` by its own bounds.
pub const CC7_C_OPERATIONS: [Cc7Operation; 1] = [Cc7Operation {
    effect_name: CC7_TECHNICAL_LUT_EFFECT,
    parameters: &[("lut_asset_id", 1)],
}];

/// Scenario (d): the **qualifier-only** node (R-B4).
///
/// `matte_qualifier_enabled` is stored because its descriptor neutral is `0`
/// and `matte_request_parameters` injects it whenever a qualifier is derived
/// (`color_status.rs:4533-4544`); the nine bands below are exactly what
/// `MatteSampleStatistics::derived_qualifier` produces from the `product_red`
/// sample (`color_status.rs:4970-5014`).
pub const CC7_D_OPERATIONS: [Cc7Operation; 1] = [Cc7Operation {
    effect_name: CC7_PRIMARY_CORRECTION_EFFECT,
    parameters: &[
        ("matte_enabled", 1),
        ("matte_hue_center_centidegrees", 35_865),
        ("matte_hue_softness_centidegrees", 1_000),
        ("matte_hue_width_centidegrees", 1_500),
        ("matte_luma_high_basis_points", 3_513),
        ("matte_luma_low_basis_points", 1_513),
        ("matte_luma_softness_basis_points", 1_000),
        ("matte_qualifier_enabled", 1),
        ("matte_saturation_high_basis_points", 9_728),
        ("matte_saturation_low_basis_points", 7_728),
        ("matte_saturation_softness_basis_points", 1_000),
        ("saturation_percent", 40),
    ],
}];

/// Scenario (d2): the **window-only** node (R-B4), a separate document.
///
/// `Matte::coverage` multiplies the window and qualifier legs
/// (`color_pipeline.rs:2109-2112`), so a node carrying both would measure
/// `192 / 140 / 52` rather than the feather band §4(d)(4) measures.
/// `matte_qualifier_enabled` is left at its neutral, so it is not stored, and
/// neither is `matte_window0_shape_token = 1`, which **is** the rect shape's
/// descriptor neutral (`effect.rs:744`).
pub const CC7_D2_OPERATIONS: [Cc7Operation; 1] = [Cc7Operation {
    effect_name: CC7_PRIMARY_CORRECTION_EFFECT,
    parameters: &[
        ("matte_enabled", 1),
        ("matte_window0_center_x_basis_points", 1_687),
        ("matte_window0_center_y_basis_points", 4_666),
        ("matte_window0_feather_basis_points", 1_000),
        ("matte_window0_half_height_basis_points", 444),
        ("matte_window0_half_width_basis_points", 187),
        ("matte_window_count", 1),
        ("saturation_percent", 40),
    ],
}];

/// Scenario (e): one `creative_look` at the look stage carrying **only**
/// `lut_asset_id`; `mix_basis_points = 10_000` is the neutral and is not
/// stored (`effect.rs:214-228`).
pub const CC7_E_OPERATIONS: [Cc7Operation; 1] = [Cc7Operation {
    effect_name: CC7_CREATIVE_LOOK_EFFECT,
    parameters: &[("lut_asset_id", 1)],
}];

/// Scenario (f): the tracked window node.
///
/// `matte_window0_center_x_basis_points` and `_center_y_basis_points` are
/// **not** stored as statics: `5_000` is their descriptor neutral
/// (`effect.rs:749-761`), which is exactly frame 0's square centre. They arrive
/// as the two `SetEffectKeyframes` curves instead.
pub const CC7_F_OPERATIONS: [Cc7Operation; 1] = [Cc7Operation {
    effect_name: CC7_PRIMARY_CORRECTION_EFFECT,
    parameters: &[
        ("matte_enabled", 1),
        ("matte_window0_half_height_basis_points", 667),
        ("matte_window0_half_width_basis_points", 375),
        ("matte_window_count", 1),
        ("saturation_percent", 40),
    ],
}];

/// The two parameter names the (f) track writes curves for.
pub const CC7_F_KEYFRAMED_PARAMETERS: [&str; 2] = [
    "matte_window0_center_x_basis_points",
    "matte_window0_center_y_basis_points",
];

// ===========================================================================
// CC7 §2.2: the scenario specs.
// ===========================================================================

/// The six named colour workflows CC7 proves end to end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cc7Scenario {
    /// (a) mixed-camera interview.
    MixedCamera,
    /// (b) wrong white balance and underexposure; the committed document is
    /// (b2)'s, and (b1)'s is [`cc7_b1_canonical_operations`].
    WhiteBalance,
    /// (c) log-like input.
    LogLike,
    /// (d) product and skin; the window-only (d2) document is
    /// [`cc7_d2_canonical_operations`].
    ProductAndSkin,
    /// (e) creative look.
    CreativeLook,
    /// (f) tracked secondary.
    TrackedSecondary,
}

/// CC7 §10.5's scenario iteration order.
pub const CC7_SCENARIOS: [Cc7Scenario; 6] = [
    Cc7Scenario::MixedCamera,
    Cc7Scenario::WhiteBalance,
    Cc7Scenario::LogLike,
    Cc7Scenario::ProductAndSkin,
    Cc7Scenario::CreativeLook,
    Cc7Scenario::TrackedSecondary,
];

/// Which raster a clip is cut from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cc7Source {
    /// CC7 §2.3.3's 60-frame base scene.
    BaseScene,
    /// CC7 §2.3.6's 100-frame tracked scene.
    TrackedSquare,
}

/// One clip of a CC7 scenario document, in timeline order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cc7Clip {
    pub clip_id: u64,
    pub camera: Cc7Camera,
    pub source: Cc7Source,
}

/// Whether the person path can express a scenario's canonical document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cc7PersonPath {
    /// The inspector's operation builders can author it by hand.
    Expressible,
    /// It cannot be authored by hand, with the reason the code itself gives.
    NotApplicable { reason: &'static str },
}

/// The `Track window…` tooltip, verbatim (`inspector_ui.rs:2868-2872`).
pub const CC7_TRACK_PERSON_PATH_REASON: &str = "Tracking is agent-driven in CC5: ask the agent to run track_matte_window. The app has no agent-tool call path, so this button would pretend to work.";

/// Everything CC7 pins about one scenario.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cc7ScenarioSpec {
    pub scenario: Cc7Scenario,
    /// `"a".."f"`, the eval task suffix.
    pub id: &'static str,
    pub title: &'static str,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Frames of the **document**: 120 for a two-clip (a)/(b) document.
    pub frames: u32,
    /// Camera per clip, in timeline order.
    pub clips: &'static [Cc7Clip],
    pub canonical_operations: &'static [Cc7Operation],
    /// The matrix question the blind reviewer is asked; `None` for (c), which
    /// is objective-only and contributes no entry to the blind package.
    pub human_question: Option<&'static str>,
    pub person_path: Cc7PersonPath,
}

const CC7_A_CLIPS: [Cc7Clip; 2] = [
    Cc7Clip {
        clip_id: 1,
        camera: Cc7Camera::A,
        source: Cc7Source::BaseScene,
    },
    Cc7Clip {
        clip_id: 2,
        camera: Cc7Camera::B,
        source: Cc7Source::BaseScene,
    },
];
const CC7_B_CLIPS: [Cc7Clip; 2] = [
    Cc7Clip {
        clip_id: 1,
        camera: Cc7Camera::A,
        source: Cc7Source::BaseScene,
    },
    Cc7Clip {
        clip_id: 2,
        camera: Cc7Camera::C2,
        source: Cc7Source::BaseScene,
    },
];
const CC7_C_CLIPS: [Cc7Clip; 1] = [Cc7Clip {
    clip_id: 1,
    camera: Cc7Camera::LogLike,
    source: Cc7Source::BaseScene,
}];
const CC7_A_CAMERA_CLIPS: [Cc7Clip; 1] = [Cc7Clip {
    clip_id: 1,
    camera: Cc7Camera::A,
    source: Cc7Source::BaseScene,
}];
const CC7_F_CLIPS: [Cc7Clip; 1] = [Cc7Clip {
    clip_id: 1,
    camera: Cc7Camera::A,
    source: Cc7Source::TrackedSquare,
}];

/// The six specs, in [`CC7_SCENARIOS`] order.
pub const CC7_SCENARIO_SPECS: [Cc7ScenarioSpec; 6] = [
    Cc7ScenarioSpec {
        scenario: Cc7Scenario::MixedCamera,
        id: "a",
        title: "Mixed-camera interview",
        width: CC7_SOURCE_WIDTH,
        height: CC7_SOURCE_HEIGHT,
        fps: CC7_SOURCE_FPS,
        frames: 2 * CC7_SOURCE_FRAMES,
        clips: &CC7_A_CLIPS,
        canonical_operations: &CC7_A_OPERATIONS,
        human_question: Some("Does the match preserve natural and intentional differences?"),
        person_path: Cc7PersonPath::Expressible,
    },
    Cc7ScenarioSpec {
        scenario: Cc7Scenario::WhiteBalance,
        id: "b",
        title: "Wrong white balance and underexposure",
        width: CC7_SOURCE_WIDTH,
        height: CC7_SOURCE_HEIGHT,
        fps: CC7_SOURCE_FPS,
        frames: 2 * CC7_SOURCE_FRAMES,
        clips: &CC7_B_CLIPS,
        canonical_operations: &CC7_B2_OPERATIONS,
        human_question: Some("Is the proposed compromise acceptable?"),
        person_path: Cc7PersonPath::Expressible,
    },
    Cc7ScenarioSpec {
        scenario: Cc7Scenario::LogLike,
        id: "c",
        title: "Log-like input",
        width: CC7_SOURCE_WIDTH,
        height: CC7_SOURCE_HEIGHT,
        fps: CC7_SOURCE_FPS,
        frames: CC7_SOURCE_FRAMES,
        clips: &CC7_C_CLIPS,
        canonical_operations: &CC7_C_OPERATIONS,
        human_question: None,
        person_path: Cc7PersonPath::Expressible,
    },
    Cc7ScenarioSpec {
        scenario: Cc7Scenario::ProductAndSkin,
        id: "d",
        title: "Product and skin",
        width: CC7_SOURCE_WIDTH,
        height: CC7_SOURCE_HEIGHT,
        fps: CC7_SOURCE_FPS,
        frames: CC7_SOURCE_FRAMES,
        clips: &CC7_A_CAMERA_CLIPS,
        canonical_operations: &CC7_D_OPERATIONS,
        human_question: Some("Does attention remain on the intended subject?"),
        person_path: Cc7PersonPath::Expressible,
    },
    Cc7ScenarioSpec {
        scenario: Cc7Scenario::CreativeLook,
        id: "e",
        title: "Creative look",
        width: CC7_SOURCE_WIDTH,
        height: CC7_SOURCE_HEIGHT,
        fps: CC7_SOURCE_FPS,
        frames: CC7_SOURCE_FRAMES,
        clips: &CC7_A_CAMERA_CLIPS,
        canonical_operations: &CC7_E_OPERATIONS,
        human_question: Some("Does the look support the story?"),
        person_path: Cc7PersonPath::Expressible,
    },
    Cc7ScenarioSpec {
        scenario: Cc7Scenario::TrackedSecondary,
        id: "f",
        title: "Tracked secondary",
        width: CC7_SOURCE_WIDTH,
        height: CC7_SOURCE_HEIGHT,
        fps: CC7_SOURCE_FPS,
        frames: CC7_TRACK_FRAMES,
        clips: &CC7_F_CLIPS,
        canonical_operations: &CC7_F_OPERATIONS,
        human_question: Some("Are any visible corrections distracting?"),
        person_path: Cc7PersonPath::NotApplicable {
            reason: CC7_TRACK_PERSON_PATH_REASON,
        },
    },
];

/// The spec for one scenario.
#[must_use]
pub const fn cc7_spec(scenario: Cc7Scenario) -> &'static Cc7ScenarioSpec {
    match scenario {
        Cc7Scenario::MixedCamera => &CC7_SCENARIO_SPECS[0],
        Cc7Scenario::WhiteBalance => &CC7_SCENARIO_SPECS[1],
        Cc7Scenario::LogLike => &CC7_SCENARIO_SPECS[2],
        Cc7Scenario::ProductAndSkin => &CC7_SCENARIO_SPECS[3],
        Cc7Scenario::CreativeLook => &CC7_SCENARIO_SPECS[4],
        Cc7Scenario::TrackedSecondary => &CC7_SCENARIO_SPECS[5],
    }
}

/// The clip a scenario's canonical node lands on: clip 2 for the two-clip
/// (a)/(b) documents, clip 1 otherwise.
#[must_use]
pub const fn cc7_target_clip(scenario: Cc7Scenario) -> ClipId {
    match scenario {
        Cc7Scenario::MixedCamera | Cc7Scenario::WhiteBalance => CC7_CANDIDATE_CLIP_ID,
        _ => CC7_SINGLE_CLIP_ID,
    }
}

// ===========================================================================
// CC7 §2.2: `cc7_canonical_operations` — the exact core batch.
// ===========================================================================

fn effect_for(id: EffectId, node: &Cc7Operation) -> Effect {
    Effect {
        id,
        name: node.effect_name.to_owned(),
        parameters: node
            .parameters
            .iter()
            .map(|(name, value)| ((*name).to_owned(), ParamValue::Integer(*value)))
            .collect::<BTreeMap<_, _>>(),
        keyframes: BTreeMap::new(),
    }
}

fn insert_batch(clip: ClipId, nodes: &[Cc7Operation]) -> Vec<Operation> {
    nodes
        .iter()
        .enumerate()
        .map(|(index, node)| Operation::InsertEffect {
            clip,
            index: CC7_NODE_INSERT_INDEX + index,
            effect: effect_for(EffectId(CC7_NODE_EFFECT_ID.0 + index as u64), node),
        })
        .collect()
}

/// The two `SetEffectKeyframes` operations the (f) track commit lands.
#[must_use]
pub fn cc7_track_keyframe_operations() -> Vec<Operation> {
    CC7_F_KEYFRAMED_PARAMETERS
        .iter()
        .enumerate()
        .map(|(axis, name)| Operation::SetEffectKeyframes {
            clip: CC7_SINGLE_CLIP_ID,
            effect: CC7_NODE_EFFECT_ID,
            name: (*name).to_owned(),
            curve: AutomationCurve {
                keyframes: CC7_TRACK_SURVIVING_SAMPLE_FRAMES
                    .iter()
                    .zip(cc7_track_keyframe_centres(axis))
                    .map(|(frame, value)| Keyframe {
                        at: TimeCode(*frame),
                        value,
                        // CC5 §5.2: sustained movement gets continuous
                        // velocity; M40 rejected eased per-segment curves.
                        interpolation: KeyframeInterpolation::Linear,
                    })
                    .collect(),
            },
        })
        .collect()
}

/// CC7 §2.2's `cc7_canonical_operations`: the operations in the order a commit
/// must apply them, and the **single** definition of "the canonical document".
///
/// Scenarios (c) and (e) bind a LUT asset, whose record is file- or
/// bake-derived and therefore cannot be pinned in a module that reads no file;
/// this function returns their node operation alone and
/// [`cc7_lut_backed_canonical_operations`] prepends the `AddLutAsset` the real
/// batch carries (`[AddLutAsset?, InsertEffect]`, `inspector_ui.rs:569-586`).
#[must_use]
pub fn cc7_canonical_operations(scenario: Cc7Scenario) -> Vec<Operation> {
    let spec = cc7_spec(scenario);
    let mut operations = insert_batch(cc7_target_clip(scenario), spec.canonical_operations);
    if scenario == Cc7Scenario::TrackedSecondary {
        operations.extend(cc7_track_keyframe_operations());
    }
    operations
}

/// Scenario (b1)'s canonical batch — the recoverable candidate, whose document
/// is a second (b) document rather than a seventh scenario (CC7 §2.5).
#[must_use]
pub fn cc7_b1_canonical_operations() -> Vec<Operation> {
    insert_batch(CC7_CANDIDATE_CLIP_ID, &CC7_B1_OPERATIONS)
}

/// Scenario (d2)'s canonical batch: the **window-only** node, a second
/// document rather than a window added to (d)'s node (R-B4).
#[must_use]
pub fn cc7_d2_canonical_operations() -> Vec<Operation> {
    insert_batch(CC7_SINGLE_CLIP_ID, &CC7_D2_OPERATIONS)
}

/// The `[AddLutAsset, InsertEffect]` batch scenarios (c) and (e) commit.
///
/// # Panics
///
/// Panics when `scenario` is not [`Cc7Scenario::LogLike`] or
/// [`Cc7Scenario::CreativeLook`], neither of which binds a LUT asset.
#[must_use]
pub fn cc7_lut_backed_canonical_operations(
    scenario: Cc7Scenario,
    asset: LutAsset,
) -> Vec<Operation> {
    assert!(
        matches!(scenario, Cc7Scenario::LogLike | Cc7Scenario::CreativeLook),
        "only scenarios (c) and (e) bind a LUT asset"
    );
    let mut operations = vec![Operation::AddLutAsset { asset }];
    operations.extend(cc7_canonical_operations(scenario));
    operations
}

/// The asset record scenario (c)'s `import_lut_asset` registers.
///
/// `sha256` and `byte_len` are properties of the generated `.cube` file, which
/// this module never reads, so the caller supplies them.
#[must_use]
pub fn cc7_log_lut_asset(sha256: &str, byte_len: u64, source_path: &str) -> LutAsset {
    LutAsset {
        id: CC7_LUT_ASSET_ID,
        sha256: sha256.to_owned(),
        title: CC7_LOG_CUBE_TITLE.to_owned(),
        kind: LutAssetKind::Cube3d,
        size: CC7_LOG_CUBE_SIZE,
        byte_len,
        domain_min_millionths: [0, 0, 0],
        domain_max_millionths: [1_000_000, 1_000_000, 1_000_000],
        source: LutAssetSource::Imported {
            source_path: source_path.to_owned(),
        },
    }
}

/// The `TITLE` the CC7 log-like inverse `.cube` carries.
///
/// Its length is load-bearing for [`CC7_LOG_CUBE_BYTES_REPORTED`]: CC4's
/// canonical `.cube` header (`lut.rs:219-240`) is `100 + title.len()` bytes,
/// and each of the `S³` sample lines is exactly 27.
pub const CC7_LOG_CUBE_TITLE: &str = "CC7 log inverse";

// ===========================================================================
// CC7 §4.1: the measured column, and the budget table it is checked against.
// ===========================================================================

/// `scopes.rs:576-586`: `ChannelStatistics::{first_percentile,
/// ninety_ninth_percentile}` are **16-bit** codes, produced at `:1330-1339`
/// where `percentile_code` returns `value * 257`.
pub const CC7_SCOPE_SIXTEEN_BIT_SCALE: i64 = 257;
/// A9's bare `20`, surviving only as an 8-bit prose equivalent (A21).
pub const CC7_LOG_FIRST_PERCENTILE_MIN_CODE8_PROSE: i64 = 20;
/// A9's bare `200`, surviving only as an 8-bit prose equivalent (A21).
pub const CC7_LOG_P99_MAX_CODE8_PROSE: i64 = 200;
/// The log carrier's `{first, median, ninety_ninth}` luma percentiles in
/// 16-bit codes (P3, probe-3, amended scene).
pub const CC7_LOG_CARRIER_LUMA_PERCENTILES_CODE16: [i64; 3] = [7_196, 31_611, 42_919];
/// The same in 8-bit prose.
pub const CC7_LOG_CARRIER_LUMA_PERCENTILES_CODE8: [i64; 3] = [28, 123, 167];
/// Cam A's luma percentiles in 16-bit codes: (c)(1)'s failing direction, which
/// fails **both** bounds.
pub const CC7_CAM_A_LUMA_PERCENTILES_CODE16: [i64; 3] = [2_570, 29_555, 62_194];
/// The same in 8-bit prose.
pub const CC7_CAM_A_LUMA_PERCENTILES_CODE8: [i64; 3] = [10, 115, 242];

/// (c)(3)'s lattice sweep: `size → set-wide worst monitoring-code error`,
/// measured over the twelve achromatic plus four skin patches (P3, probe-3).
///
/// Asserted **monotone non-increasing** with `17 > CC7_LOG_INVERSE_MAX_CODE ≥
/// 33 > 65`, so size 17 genuinely fails and the sweep is not vacuous.
pub const CC7_LOG_CUBE_SIZE_LADDER: [(u32, i64); 3] = [(17, 13), (33, 7), (65, 4)];

/// (a)(2): the matched cam B spread, worst patch `chart02` (P2).
pub const CC7_MEASURED_MATCH_NEUTRAL_SPREAD_CODE: i64 = 2;
/// (b1): the corrected C1 residual spread, measured on the canonical (b1)
/// document built from the re-measured `CC7_B1_OPERATIONS` (C-E3, C-E7).
///
/// Probe-1's 2 was taken on the pre-amendment six-patch grey band; the amended
/// twelve-patch band measures 3, which is why this row gates against
/// [`CC7_B1_RESIDUAL_SPREAD_MAX_CODE`] rather than against the (a) budget.
pub const CC7_MEASURED_B1_RESIDUAL_SPREAD_CODE: i64 = 3;
/// (b1)'s failing direction: the **uncorrected** C1 clip, which fails the same
/// gate the corrected one passes (C-E7).
pub const CC7_MEASURED_UNCORRECTED_C1_SPREAD_CODE: i64 = 7;
/// (a)(2)'s first failing direction: the **unmatched** cam B, the measurement
/// that forced the budget from 6 to 5 (A15).
pub const CC7_MEASURED_UNMATCHED_B_SPREAD_CODE: i64 = 6;
/// (a)(3): the matched chart-band luma mean delta (P2).
pub const CC7_MEASURED_MATCH_LUMA_MEAN_CODE_MILLIONTHS: i64 = -1_381_567;
/// (a)(3)'s failing direction: unmatched cam B, 3.98× over.
pub const CC7_MEASURED_UNMATCHED_B_LUMA_MEAN_CODE_MILLIONTHS: i64 = -19_904_917;
/// Corrected C2 **passes** the luma term, which is why the spread and the luma
/// mean are two gates and not one (R-M10).
pub const CC7_MEASURED_CORRECTED_C2_LUMA_MEAN_CODE_MILLIONTHS: i64 = -4_302_267;
/// (c)(2): the set-wide worst inverse error at size 65 (P3).
pub const CC7_MEASURED_LOG_INVERSE_CODE: i64 = 4;
/// (d2): the measured error against the discrete pixel-centre model (P5).
pub const CC7_MEASURED_FEATHER_MODEL_ERROR_PIXELS: i64 = 0;
/// (e)(2): the ROI out-of-gamut count on the amended scene (P2).
pub const CC7_MEASURED_DEEP_SHADOW_OUT_OF_GAMUT_PIXELS: i64 = 192;

/// CC6's delivery terms at Eight, in `DeliveryBudgets` field order:
/// luma max, luma P99 millionths, luma mean millionths, RGB mean millionths,
/// PSNR hundredths.
///
/// Each entry is the **worst of the six scenarios** measured on the amended
/// scene (Implementer C erratum C-E8), not probe-1's P7 figures, which were
/// taken on the pre-A1 scene and on the two-clip (a) document only. "Worst"
/// is the maximum for a ceiling term and the minimum for the PSNR floor. The
/// per-scenario triples live in the manifest's `budgets.delivery`, and
/// `assert_cc7_delivery_lane` asserts every lane against them, so these five
/// numbers cannot go stale again (R4-M2).
///
/// The luma mean is the worst row in the whole slice: scenario (e) measures
/// 377 538 against CC6's 400 000, a **1.06x** margin, which is why its
/// `CC7_BUDGETS` row is a [`Cc7BudgetKind::RecordedMargin`]. CC7 never
/// re-baselines a CC6 constant (§4.1 note 2, §4(g)(1)).
pub const CC7_MEASURED_DELIVERY_EIGHT: [i64; 5] = [2, 1_000_000, 377_538, 855_810, 4_059];
/// The same at Ten, worst per term over the six scenarios (C-E8). The luma
/// P99 measured **exactly zero** on every scenario.
pub const CC7_MEASURED_DELIVERY_TEN: [i64; 5] = [1, 0, 347, 385_514, 4_129];

/// How CC7 §4.1 checks one budget row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cc7BudgetKind {
    /// `budget / measured ≥ 2`, and `|measured|` strictly inside the budget.
    RatioAtLeastTwo,
    /// The measurement is the budget: an exact count, not a bound.
    Exact,
    /// Measured at or near zero: the margin is the failing-direction fixture,
    /// never a fabricated ratio (CC7 §4.1 note 3).
    MeasuredZero,
    /// A floor: `measured ≥ budget`, and the headroom is a code distance.
    Floor,
    /// A ceiling: `measured ≤ budget`, and the headroom is a code distance.
    Ceiling,
    /// The measurement is strictly inside a budget CC7 does **not** own and
    /// may not move, but it does **not** clear the 2x bar: the margin is
    /// recorded rather than asserted (§4.1 note 2, §4(g)(1); R4-M2).
    ///
    /// This is not a general escape hatch —
    /// `cc7_every_budget_carries_the_declared_margin` asserts that a
    /// `RecordedMargin` row genuinely fails the 2x bar, so a row that clears
    /// it must be a [`Cc7BudgetKind::RatioAtLeastTwo`] row.
    RecordedMargin,
}

/// One row of CC7 §4.1's `budget | measured | margin` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cc7Budget {
    pub term: &'static str,
    pub constant: &'static str,
    pub budget: i64,
    pub measured: i64,
    pub kind: Cc7BudgetKind,
}

/// CC7 §4.1's table, minus the two-sided track confidence floor and the
/// containment half-extents, which CC7 §4.1 notes 3 and 5 state are not ratio
/// rows and which `cc7_every_budget_carries_the_declared_margin` checks by
/// their own rule.
#[allow(clippy::cast_lossless)]
pub const CC7_BUDGETS: [Cc7Budget; 17] = [
    Cc7Budget {
        term: "neutral spread, matched cam B",
        constant: "CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE",
        budget: CC7_MATCH_NEUTRAL_SPREAD_MAX_CODE,
        measured: CC7_MEASURED_MATCH_NEUTRAL_SPREAD_CODE,
        kind: Cc7BudgetKind::RatioAtLeastTwo,
    },
    Cc7Budget {
        term: "(b1) residual spread",
        constant: "CC7_B1_RESIDUAL_SPREAD_MAX_CODE",
        budget: CC7_B1_RESIDUAL_SPREAD_MAX_CODE,
        measured: CC7_MEASURED_B1_RESIDUAL_SPREAD_CODE,
        kind: Cc7BudgetKind::RatioAtLeastTwo,
    },
    Cc7Budget {
        term: "chart luma mean delta",
        constant: "CC7_MATCH_LUMA_MEAN_MAX_CODE_MILLIONTHS",
        budget: CC7_MATCH_LUMA_MEAN_MAX_CODE_MILLIONTHS,
        measured: CC7_MEASURED_MATCH_LUMA_MEAN_CODE_MILLIONTHS,
        kind: Cc7BudgetKind::RatioAtLeastTwo,
    },
    Cc7Budget {
        term: "log first percentile (floor), 16-bit",
        constant: "CC7_LOG_FIRST_PERCENTILE_MIN_CODE16",
        budget: CC7_LOG_FIRST_PERCENTILE_MIN_CODE16,
        measured: CC7_LOG_CARRIER_LUMA_PERCENTILES_CODE16[0],
        kind: Cc7BudgetKind::Floor,
    },
    Cc7Budget {
        term: "log 99th percentile (ceiling), 16-bit",
        constant: "CC7_LOG_P99_MAX_CODE16",
        budget: CC7_LOG_P99_MAX_CODE16,
        measured: CC7_LOG_CARRIER_LUMA_PERCENTILES_CODE16[2],
        kind: Cc7BudgetKind::Ceiling,
    },
    Cc7Budget {
        term: "log inverse patch error (set-wide)",
        constant: "CC7_LOG_INVERSE_MAX_CODE",
        budget: CC7_LOG_INVERSE_MAX_CODE,
        measured: CC7_MEASURED_LOG_INVERSE_CODE,
        kind: Cc7BudgetKind::RatioAtLeastTwo,
    },
    Cc7Budget {
        term: "feather counts",
        constant: "CC7_FEATHER_PARTIAL_TOLERANCE_PIXELS",
        budget: CC7_FEATHER_PARTIAL_TOLERANCE_PIXELS,
        measured: CC7_MEASURED_FEATHER_MODEL_ERROR_PIXELS,
        kind: Cc7BudgetKind::MeasuredZero,
    },
    Cc7Budget {
        term: "deep-shadow gamut count",
        constant: "CC7_LOOK_DEEP_SHADOW_OUT_OF_GAMUT_PIXELS",
        budget: CC7_LOOK_DEEP_SHADOW_OUT_OF_GAMUT_PIXELS,
        measured: CC7_MEASURED_DEEP_SHADOW_OUT_OF_GAMUT_PIXELS,
        kind: Cc7BudgetKind::Exact,
    },
    Cc7Budget {
        term: "delivery, 8-bit luma max",
        constant: "DELIVERY_LUMA_MAX_CODE_8BIT",
        budget: crate::DELIVERY_LUMA_MAX_CODE_8BIT as i64,
        measured: CC7_MEASURED_DELIVERY_EIGHT[0],
        kind: Cc7BudgetKind::RatioAtLeastTwo,
    },
    Cc7Budget {
        term: "delivery, 8-bit luma P99",
        constant: "DELIVERY_LUMA_P99_CODE_8BIT_MILLIONTHS",
        budget: crate::DELIVERY_LUMA_P99_CODE_8BIT_MILLIONTHS,
        measured: CC7_MEASURED_DELIVERY_EIGHT[1],
        kind: Cc7BudgetKind::RatioAtLeastTwo,
    },
    Cc7Budget {
        term: "delivery, 8-bit luma mean",
        constant: "DELIVERY_LUMA_MEAN_CODE_8BIT_MILLIONTHS",
        budget: crate::DELIVERY_LUMA_MEAN_CODE_8BIT_MILLIONTHS,
        measured: CC7_MEASURED_DELIVERY_EIGHT[2],
        // The worst row in the slice: 377 538 against 400 000 on scenario
        // (e), a 1.06x margin against a CC6 constant CC7 must not move.
        kind: Cc7BudgetKind::RecordedMargin,
    },
    Cc7Budget {
        term: "delivery, 8-bit RGB mean",
        constant: "DELIVERY_RGB_MEAN_CODE_8BIT_MILLIONTHS",
        budget: crate::DELIVERY_RGB_MEAN_CODE_8BIT_MILLIONTHS,
        measured: CC7_MEASURED_DELIVERY_EIGHT[3],
        kind: Cc7BudgetKind::RatioAtLeastTwo,
    },
    Cc7Budget {
        term: "delivery, 8-bit PSNR",
        constant: "DELIVERY_PSNR_FLOOR_DB_HUNDREDTHS_8BIT",
        budget: crate::DELIVERY_PSNR_FLOOR_DB_HUNDREDTHS_8BIT as i64,
        measured: CC7_MEASURED_DELIVERY_EIGHT[4],
        kind: Cc7BudgetKind::Floor,
    },
    Cc7Budget {
        term: "delivery, 10-bit luma max",
        constant: "DELIVERY_LUMA_MAX_CODE_10BIT",
        budget: crate::DELIVERY_LUMA_MAX_CODE_10BIT as i64,
        measured: CC7_MEASURED_DELIVERY_TEN[0],
        kind: Cc7BudgetKind::RatioAtLeastTwo,
    },
    Cc7Budget {
        term: "delivery, 10-bit luma P99",
        constant: "DELIVERY_LUMA_P99_CODE_10BIT_MILLIONTHS",
        budget: crate::DELIVERY_LUMA_P99_CODE_10BIT_MILLIONTHS,
        measured: CC7_MEASURED_DELIVERY_TEN[1],
        kind: Cc7BudgetKind::MeasuredZero,
    },
    Cc7Budget {
        term: "delivery, 10-bit luma mean",
        constant: "DELIVERY_LUMA_MEAN_CODE_10BIT_MILLIONTHS",
        budget: crate::DELIVERY_LUMA_MEAN_CODE_10BIT_MILLIONTHS,
        measured: CC7_MEASURED_DELIVERY_TEN[2],
        kind: Cc7BudgetKind::RatioAtLeastTwo,
    },
    Cc7Budget {
        term: "delivery, 10-bit RGB mean",
        constant: "DELIVERY_RGB_MEAN_CODE_10BIT_MILLIONTHS",
        budget: crate::DELIVERY_RGB_MEAN_CODE_10BIT_MILLIONTHS,
        measured: CC7_MEASURED_DELIVERY_TEN[3],
        kind: Cc7BudgetKind::RatioAtLeastTwo,
    },
];

/// The 10-bit PSNR floor row, kept out of [`CC7_BUDGETS`] only because the
/// array is sized to the terms CC7 §4.1 lists once each; it is asserted beside
/// the table.
pub const CC7_DELIVERY_TEN_PSNR_MEASURED_HUNDREDTHS: i64 = CC7_MEASURED_DELIVERY_TEN[4];

/// The observation-accuracy row, checked beside the table because its budget
/// is CC5's rather than CC7's own.
pub const CC7_TRACK_OBSERVATION_BUDGET_ROW: Cc7Budget = Cc7Budget {
    term: "track observation error",
    constant: "CC7_TRACK_TOLERANCE_BASIS_POINTS",
    budget: CC7_TRACK_TOLERANCE_BASIS_POINTS,
    measured: CC7_TRACK_WORST_RAW_OBSERVATION_ERROR_BASIS_POINTS_REPORTED,
    kind: Cc7BudgetKind::RatioAtLeastTwo,
};

/// CC7 §4.1 note 5's bar: the confidence floor must keep more than this many
/// basis points on **both** sides of the measured separation.
pub const CC7_TRACK_CONFIDENCE_SEPARATION_MIN_BASIS_POINTS: i64 = 1_000;
