//! Deterministic CC6 colour QC evidence for the managed working stage.
//!
//! This module measures one already-rendered [`WorkingProof`] — the composited
//! scene-linear surface named [`crate::ScopeStage::WorkingLinearPostComposite`] — and
//! reports integers. It never maps, never clamps differently, never proposes a
//! fix, never mutates a document, and never moves a finished encode: CC6
//! measures and reports.
//!
//! **Why this stage.** CC1 invariant 2.2.5 says no colour stage clamps RGB to
//! `0..1` and that the only RGB clamp is the final monitor or delivery encode.
//! That single clamp is the only place a managed grade silently loses
//! information, and the monitor raster the CC2 scope engine measures has
//! already been through it. A gamut or legal-range excursion is therefore not
//! observable at `monitoring_post_composite` at all; it is observable here.
//!
//! **Purity.** [`measure_color_qc`] and everything it calls perform no I/O,
//! hold no renderer, construct no [`crate::Operation`], read no clock, and use
//! no RNG. Two runs on the same raster produce byte-identical output. The one
//! exception is the [`nodes`] submodule (CC6 §3.7), which renders and applies
//! operations to a *cloned* document; its cost and its impurity are stated
//! there, and it is never on [`measure_color_qc`]'s path.
//!
//! **Units.** Every count is a `u64`, every rate is integer-floor basis points
//! (`floor(value · 10_000 / count)`, CC2's rule), every linear or encoded
//! scalar is signed millionths (`round(v · 1_000_000)`, half away from zero),
//! every angle is centidegrees, every `Y'CbCr` excursion is hundredths of a
//! delivery code, and PSNR is hundredths of a dB. No CC6 API returns a float.

pub mod nodes;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    ClipId, ColorSourceProfile, DeliveryEncodeDepth, DeliveryLane, DeliveryTagCheck, EffectId,
    MatteRegionDescription, MediaError, NormalizedRoi, PixelRoi, QaSeverity, RgbaImage,
    SCOPE_BASIS_POINTS, WorkingProof,
    cc8_hdr::{
        CC8_BT709_TO_REC2020, CC8_BT2020_CB_DENOMINATOR, CC8_BT2020_CR_DENOMINATOR, CC8_BT2020_KB,
        CC8_BT2020_KG, CC8_BT2020_KR, CC8_REFERENCE_WHITE_NITS, cc8_apply_matrix,
        cc8_hlg_encode_working_linear,
    },
};

// ---------------------------------------------------------------------------
// §3.0: the delivery transfer core owns.
// ---------------------------------------------------------------------------

/// The f32 delivery transfer, owned by core.
///
/// Bit-identical to `kinewright_media::color_pipeline::encode_bt709` for every
/// `f32` input: the same seam (`linear < 0.018`), the same rounded BT.709
/// constants (`4.5`, `1.099`, `0.099`, `0.45`), the same sign-preserving odd
/// extension, and the same `f32` arithmetic order. Core must not gain a
/// dependency on `kinewright-media`, so this is the only permitted second copy
/// and a media fixture gates `to_bits()` equality over a dense sweep.
///
/// **No clamp.** The delivery clamp lives in `quantize_delivery16`; this
/// function is what that clamp *receives*, which is why an over-range result
/// is meaningful evidence rather than a bug.
///
/// The seam is stated, not assumed: Rust takes the **power** branch at exactly
/// `0.018f32`, because the `f32` literal `0.018` is `0.0179999992251396179`
/// and `linear < 0.018` compares that value to itself. BT.709's rounded
/// constants make the function discontinuous there by `2.479e-4`, which is
/// `0.0543` eight-bit codes.
#[must_use]
pub fn encode_bt709_delivery(linear: f32) -> f32 {
    if linear < 0.0 {
        -encode_bt709_delivery(-linear)
    } else if linear < 0.018 {
        4.5 * linear
    } else {
        1.099 * linear.powf(0.45) - 0.099
    }
}

/// The delivery encode of one **lane**, in that lane's own primaries and
/// transfer — CC8 §6 item 1's report-side half.
///
/// §6 item 1 makes legality "measured against **the lane's own matrix**", and a
/// matrix consumes `R'G'B'`: feeding BT.709-coded samples to the BT.2020 NCL
/// matrix would be exactly the "wrong number, not an approximate one" that item
/// names. So the *whole* encode is the lane's, not just the matrix that follows
/// it, and this function is the single place that decides which one runs.
///
/// - [`DeliveryLane::SdrRec709`] is [`encode_bt709_delivery`] per channel,
///   unchanged and in the same channel order, so every CC1–CC7 measurement is
///   byte-identical (§9.1 fixture 6).
/// - [`DeliveryLane::HdrHlgRec2020`] is §3.3's delivery line: the working
///   BT.709 linear goes through
///   [`CC8_BT709_TO_REC2020`](crate::cc8_hdr::CC8_BT709_TO_REC2020) and then
///   [`cc8_hlg_encode_working_linear`](crate::cc8_hdr::cc8_hlg_encode_working_linear),
///   which is the arithmetic `kinewright_media::color_pipeline::encode_delivery_hlg_rec2020_rgba16`
///   performs before `quantize_delivery16` clamps it. That clamp is what the
///   range report predicts, so predicting it against the BT.709 curve on the
///   HDR lane would predict a clamp no HDR export applies.
///
/// **No clamp**, on either lane: this is what the delivery clamp *receives*.
#[must_use]
pub fn encode_delivery_for_lane(lane: DeliveryLane, working_linear: [f32; 3]) -> [f32; 3] {
    match lane {
        DeliveryLane::SdrRec709 => [
            encode_bt709_delivery(working_linear[0]),
            encode_bt709_delivery(working_linear[1]),
            encode_bt709_delivery(working_linear[2]),
        ],
        DeliveryLane::HdrHlgRec2020 => {
            cc8_hlg_encode_working_linear(cc8_apply_matrix(CC8_BT709_TO_REC2020, working_linear))
        }
    }
}

// ---------------------------------------------------------------------------
// §3.4: the forward BT.709 limited-range `Y'CbCr` reference.
// ---------------------------------------------------------------------------

/// BT.709 luma coefficient for red.
pub const BT709_KR: f64 = 0.2126;
/// BT.709 luma coefficient for blue.
pub const BT709_KB: f64 = 0.0722;
/// `2 · (1 − Kb)`: the Cb normalization denominator.
pub const BT709_CB_DENOMINATOR: f64 = 1.8556;
/// `2 · (1 − Kr)`: the Cr normalization denominator.
pub const BT709_CR_DENOMINATOR: f64 = 1.5748;
/// Limited-range luma offset at 8 bits.
pub const YCBCR_LUMA_OFFSET: i32 = 16;
/// Limited-range luma span at 8 bits.
pub const YCBCR_LUMA_SPAN: i32 = 219;
/// Limited-range chroma offset at 8 bits.
pub const YCBCR_CHROMA_OFFSET: i32 = 128;
/// Limited-range chroma span at 8 bits.
pub const YCBCR_CHROMA_SPAN: i32 = 224;
/// Inclusive legal luma ceiling at 8 bits.
pub const YCBCR_LUMA_LEGAL_HIGH: i32 = 235;
/// Inclusive legal chroma ceiling at 8 bits.
pub const YCBCR_CHROMA_LEGAL_HIGH: i32 = 240;

/// Encode display-referred `R'G'B'` as BT.709 limited-range `Y'CbCr` codes.
///
/// `bits` is 8 or 10 and `s = 2^(bits − 8)`; the result is
/// `[Y_code, Cb_code, Cr_code]`, **unclamped and unrounded**, so an
/// out-of-legal input produces an out-of-legal code rather than a silently
/// corrected one.
///
/// ```text
/// Y'      = Kr·R' + (1 − Kr − Kb)·G' + Kb·B'
/// Cb      = (B' − Y') / 1.8556
/// Cr      = (R' − Y') / 1.5748
/// Y_code  =  16·s + 219·s·Y'
/// Cb_code = 128·s + 224·s·Cb
/// Cr_code = 128·s + 224·s·Cr
/// ```
///
/// These are the exact inverses of the constants the media crate already
/// carries for the decode direction: `Kb·1.8556/Kg = 0.1873242729306488` and
/// `Kr·1.5748/Kg = 0.46812427293064884` are `BT709_GREEN_FROM_CB` and
/// `BT709_GREEN_FROM_CR`, and the two denominators are `BT709_BLUE_FROM_CB`
/// and `BT709_RED_FROM_CR`.
#[must_use]
pub fn bt709_limited_ycbcr(encoded_rgb: [f64; 3], bits: u8) -> [f64; 3] {
    // CC6 has exactly two delivery lanes. A third depth would silently produce
    // a code scale no CC6 budget, legal box, or fixture is written against, so
    // it is caught in development rather than reported as evidence.
    // [`DeliveryEncodeDepth::bits`] is the only supported source of `bits`.
    debug_assert!(
        matches!(bits, 8 | 10),
        "bt709_limited_ycbcr takes an 8-bit or 10-bit delivery depth, got {bits}"
    );
    let [red, green, blue] = encoded_rgb;
    let luma = BT709_KR * red + (1.0 - BT709_KR - BT709_KB) * green + BT709_KB * blue;
    let cb = (blue - luma) / BT709_CB_DENOMINATOR;
    let cr = (red - luma) / BT709_CR_DENOMINATOR;
    let scale = ycbcr_scale(bits);
    [
        f64::from(YCBCR_LUMA_OFFSET).mul_add(scale, f64::from(YCBCR_LUMA_SPAN) * scale * luma),
        f64::from(YCBCR_CHROMA_OFFSET).mul_add(scale, f64::from(YCBCR_CHROMA_SPAN) * scale * cb),
        f64::from(YCBCR_CHROMA_OFFSET).mul_add(scale, f64::from(YCBCR_CHROMA_SPAN) * scale * cr),
    ]
}

/// Encode display-referred `R'G'B'` as BT.2020 **non-constant-luminance**
/// limited-range `Y'CbCr` codes — CC8 §6 item 1's sibling of
/// [`bt709_limited_ycbcr`].
///
/// §6 item 1, verbatim: "`bt709_limited_ycbcr` (`color_qc.rs:120-132`) gets a
/// BT.2020 sibling with `KR = 0.2627`, `KB = 0.0593` and the corresponding
/// denominators, pinned in the authority module. Legality (EBU R 103-shaped, as
/// CC6 §6.4) is measured against **the lane's own matrix**. Reusing the BT.709
/// reference on a BT.2020 file would be a wrong number, not an approximate
/// one."
///
/// The three coefficients and the two denominators come from
/// [`crate::cc8_hdr`] — [`CC8_BT2020_KR`], [`CC8_BT2020_KG`],
/// [`CC8_BT2020_KB`], [`CC8_BT2020_CB_DENOMINATOR`] and
/// [`CC8_BT2020_CR_DENOMINATOR`] — never restated here, so a mis-transcribed
/// coefficient fails the authority module's own test rather than agreeing with
/// itself in two places.
///
/// Everything else is [`bt709_limited_ycbcr`]'s, unchanged: the same limited
/// offsets and spans, the same `s = 2^(bits − 8)` scale, and the same
/// **unclamped and unrounded** result, so an out-of-legal input produces an
/// out-of-legal code rather than a silently corrected one.
///
/// **Why this arrives with §10 step 6 rather than step 7.** §6's report-level
/// work — the lane-aware gamut report, the ungated `MaxCLL`/`MaxFALL` rows, and
/// the withheld skin reason — is §10 step 7's. This one function is not: §9.1
/// fixture 8, which step 6 owns, gates on "decoded native-plane BT.2020
/// legality" and on "difference budgets against the re-rendered delivery
/// reference", and both are measured through the lane's matrix. Landing the
/// fixture with the BT.709 reference would be gating the HDR lane on the wrong
/// number, which §6 item 1 names as the mistake it exists to prevent.
#[must_use]
pub fn bt2020_ncl_limited_ycbcr(encoded_rgb: [f64; 3], bits: u8) -> [f64; 3] {
    debug_assert!(
        matches!(bits, 8 | 10),
        "bt2020_ncl_limited_ycbcr takes an 8-bit or 10-bit delivery depth, got {bits}"
    );
    let [red, green, blue] = encoded_rgb;
    let luma = CC8_BT2020_KR * red + CC8_BT2020_KG * green + CC8_BT2020_KB * blue;
    let cb = (blue - luma) / CC8_BT2020_CB_DENOMINATOR;
    let cr = (red - luma) / CC8_BT2020_CR_DENOMINATOR;
    let scale = ycbcr_scale(bits);
    [
        f64::from(YCBCR_LUMA_OFFSET).mul_add(scale, f64::from(YCBCR_LUMA_SPAN) * scale * luma),
        f64::from(YCBCR_CHROMA_OFFSET).mul_add(scale, f64::from(YCBCR_CHROMA_SPAN) * scale * cb),
        f64::from(YCBCR_CHROMA_OFFSET).mul_add(scale, f64::from(YCBCR_CHROMA_SPAN) * scale * cr),
    ]
}

/// The limited-range `Y'CbCr` reference of one delivery lane (CC8 §6 item 1).
///
/// One call site, one lane, one matrix: this is what keeps the verification's
/// reference luma from being a BT.709 assumption on a BT.2020 file. The lane
/// comes from the delivery description, which CC8 §5.2 clause 1 makes the
/// single source of truth for every lane-derived term.
#[must_use]
pub fn delivery_limited_ycbcr(
    lane: crate::DeliveryLane,
    encoded_rgb: [f64; 3],
    bits: u8,
) -> [f64; 3] {
    match lane {
        crate::DeliveryLane::SdrRec709 => bt709_limited_ycbcr(encoded_rgb, bits),
        crate::DeliveryLane::HdrHlgRec2020 => bt2020_ncl_limited_ycbcr(encoded_rgb, bits),
    }
}

/// `s = 2^(bits − 8)`, the delivery code scale.
fn ycbcr_scale(bits: u8) -> f64 {
    f64::from(1u32 << bits.saturating_sub(8).min(24))
}

/// Which measurement produced a [`YCbCrLegalReport`].
///
/// The two are not interchangeable, and the difference is the point: a
/// prediction cannot see codec ringing, and a decoded plane cannot be produced
/// before an export exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum YCbCrLegalSource {
    /// Computed from the *unclamped* delivery-encoded `R'G'B'` of a working
    /// proof. Its excursion set is exactly the §3.2 range set; what it adds is
    /// the magnitude in delivery code units and the attribution to luma versus
    /// chroma, which the RGB test cannot see.
    Predicted,
    /// Measured from a decoded file's actual Y, Cb, and Cr planes. Codec
    /// ringing and rounding can push a plane outside the legal box even after
    /// a perfectly legal encode, and nothing in the prediction can see it.
    DecodedNativePlanes,
}

/// Legal-range excursions on one `Y'CbCr` plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlaneLegalExcursion {
    pub below_count: u64,
    pub above_count: u64,
    pub below_basis_points: u32,
    pub above_basis_points: u32,
    /// The **lowest sample code observed** on this plane over the region, in
    /// hundredths of a delivery code. It is the observed extreme, not the
    /// excursion amount: subtract the plane's legal floor (`16·s`) to get the
    /// amount by which the worst sample fell below it.
    ///
    /// When no sample reached this plane at all, this is
    /// [`PlaneLegalExcursion::UNSEEN_MINIMUM_CODE_HUNDREDTHS`]; see
    /// [`PlaneLegalExcursion::samples_seen`].
    pub minimum_code_hundredths: i64,
    /// The **highest sample code observed** on this plane over the region, in
    /// hundredths of a delivery code. Subtract the plane's legal ceiling
    /// (`235·s` for luma, `240·s` for chroma) to get the excursion amount.
    ///
    /// When no sample reached this plane at all, this is
    /// [`PlaneLegalExcursion::UNSEEN_MAXIMUM_CODE_HUNDREDTHS`]; see
    /// [`PlaneLegalExcursion::samples_seen`].
    pub maximum_code_hundredths: i64,
}

impl PlaneLegalExcursion {
    /// [`Self::minimum_code_hundredths`] for a plane that saw no sample.
    ///
    /// A plane that saw nothing has no extreme, and reporting `0` for one
    /// would be indistinguishable from a plane whose worst sample really did
    /// land on code `0` — a legible-looking number nothing measured. The
    /// unseen pair is instead the **empty interval** `minimum > maximum`,
    /// which no real sample set can produce, so `seen = false` is recoverable
    /// from the two numbers themselves without a flag field. The flag is not a
    /// field because [`PlaneLegalExcursion`] is constructed by name outside
    /// this crate (the media verifier's decoded planes, and agent and app
    /// fixtures), and a new field would silently invalidate those literals.
    pub const UNSEEN_MINIMUM_CODE_HUNDREDTHS: i64 = i64::MAX;
    /// [`Self::maximum_code_hundredths`] for a plane that saw no sample.
    pub const UNSEEN_MAXIMUM_CODE_HUNDREDTHS: i64 = i64::MIN;

    /// Whether any sample reached this plane, so the extremes mean something.
    ///
    /// `false` exactly when the pair is the empty interval described on
    /// [`Self::UNSEEN_MINIMUM_CODE_HUNDREDTHS`]. In a CC6 prediction that
    /// happens only when every visible pixel in the region was non-finite and
    /// was therefore excluded from every accumulator; the report says so in
    /// [`ColorQcReport::non_finite_pixel_count`] and refuses to pass.
    #[must_use]
    pub const fn samples_seen(&self) -> bool {
        self.minimum_code_hundredths <= self.maximum_code_hundredths
    }

    /// The plane's **combined** strict-legal-box excursion rate over
    /// `sample_count` samples, in integer-floor basis points (§10.1).
    ///
    /// `below_count + above_count`, not `max(below, above)`: a plane whose
    /// samples leave the box in both directions leaves it once per excursion,
    /// and taking the larger of the two rates under-reports exactly the plane
    /// that is worst. The two directions are also reported separately, in
    /// [`Self::below_basis_points`] and [`Self::above_basis_points`], but
    /// neither of those is the rate
    /// [`DECODED_RANGE_EXCEPTION_BASIS_POINTS`](crate::DECODED_RANGE_EXCEPTION_BASIS_POINTS)
    /// is compared against.
    ///
    /// `sample_count` is the population the two counts were taken over; it is
    /// an argument rather than a field because [`PlaneLegalExcursion`] is
    /// constructed by name outside this crate and a new field would silently
    /// invalidate those literals (see
    /// [`Self::UNSEEN_MINIMUM_CODE_HUNDREDTHS`]). An empty population is
    /// `0`, the same answer [`Self::below_basis_points`] carries for one.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub const fn excursion_basis_points(&self, sample_count: u64) -> u32 {
        if sample_count == 0 {
            return 0;
        }
        // Exact `u128` arithmetic, the same strategy as `basis_points`, so a
        // saturating multiply can never understate the rate.
        let excursions = self.below_count.saturating_add(self.above_count) as u128;
        let rate = excursions * 10_000 / sample_count as u128;
        if rate > u32::MAX as u128 {
            u32::MAX
        } else {
            rate as u32
        }
    }
}

/// `Y'CbCr` limited-range legality at one delivery bit depth.
///
/// Counts, per plane, samples outside `[16·s, 235·s]` for Y and
/// `[16·s, 240·s]` for Cb and Cr. Both comparisons are strict, so a sample
/// sitting exactly on a bound — 75 % blue is `Cb = 240.0` exactly — is not an
/// excursion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct YCbCrLegalReport {
    /// 8 or 10.
    pub bit_depth: u8,
    pub luma: PlaneLegalExcursion,
    pub cb: PlaneLegalExcursion,
    pub cr: PlaneLegalExcursion,
    pub source: YCbCrLegalSource,
}

// ---------------------------------------------------------------------------
// §3.2: range.
// ---------------------------------------------------------------------------

/// Basis-point rate at or above which a range excursion is reported as a
/// [`QaSeverity::Warning`] rather than merely counted.
///
/// A constant rather than a parameter so two reports are comparable, and it
/// exists so one ringing pixel does not raise a warning on every frame of a
/// shot: at whole-raster scope on any raster wider than 10 000 pixels, no
/// isolated pixel can reach it.
pub const QC_RANGE_EXCEPTION_BASIS_POINTS: u32 = 10;

/// Basis-point rate at or above which a gamut excursion is reported as a
/// [`QaSeverity::Warning`]. Same reasoning as
/// [`QC_RANGE_EXCEPTION_BASIS_POINTS`].
pub const QC_GAMUT_EXCEPTION_BASIS_POINTS: u32 = 10;

/// Delivery-clamp events on one channel, measured in the encoded domain.
///
/// Both comparisons are strict, so `e = 1.0` exactly — which is `linear = 1.0`
/// exactly, since `1.099·1^0.45 − 0.099 = 1.000000` in both `f64` and `f32` —
/// is **not** an excursion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct ChannelRangeExcursion {
    pub over_pixel_count: u64,
    pub under_pixel_count: u64,
    pub over_basis_points: u32,
    pub under_basis_points: u32,
    /// `max(e) − 1.0` over the region, millionths; `0` when nothing is over.
    pub maximum_over_excursion_millionths: i64,
    /// `min(e, 0.0)` over the region, millionths; `0` when nothing is under.
    pub minimum_under_excursion_millionths: i64,
}

/// Per-channel delivery-clamp measurement over one region.
///
/// This is the *only* correct test for "this pixel will lose information at
/// delivery": the delivery clamp lives in `quantize_delivery16`, which clamps
/// the **delivery-encoded** value, so the measurement is on
/// [`encode_bt709_delivery`]'s unclamped output rather than on linear light.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ColorRangeReport {
    pub red: ChannelRangeExcursion,
    pub green: ChannelRangeExcursion,
    pub blue: ChannelRangeExcursion,
    /// Pixels with at least one clamped channel, in either direction.
    pub clamped_pixel_count: u64,
    pub clamped_basis_points: u32,
    /// The §3.4 prediction, in delivery code units at
    /// [`ColorQcReport::delivery_bit_depth`].
    pub predicted_ycbcr: YCbCrLegalReport,
}

// ---------------------------------------------------------------------------
// §3.3: gamut.
// ---------------------------------------------------------------------------

/// The fixed prose [`ColorGamutReport::definition`] carries on the Rec.709
/// triangle.
///
/// **Unchanged by CC8**, character for character: §9.1 fixture 6 makes every
/// CC1–CC7 pinned constant unmoved a condition of accepting CC8, and CC6's own
/// fixtures assert this string verbatim. §6 item 2's second triangle gets its
/// own prose in [`GAMUT_DEFINITION_REC2020`] rather than editing this one.
pub const GAMUT_DEFINITION: &str = "Out of gamut is min(r, g, b) < 0 in linear light: exactly the \
set of pixels with at least one under-range channel in the range report. The two reports describe \
one pixel set from two sides and must not be summed. An over-range positive value is a range \
excursion and is not a gamut excursion: it is inside the Rec.709 chromaticity triangle and merely \
brighter than diffuse white.";

/// [`ColorGamutReport::triangle`] for the Rec.709 report — CC6's, and the one
/// every SDR measurement carries.
pub const GAMUT_TRIANGLE_REC709: &str = "rec709";

/// [`ColorGamutReport::triangle`] for CC8 §6 item 2's second report.
pub const GAMUT_TRIANGLE_REC2020: &str = "rec2020";

/// The fixed prose the Rec.2020 [`ColorGamutReport::definition`] carries
/// (CC8 §6 item 2).
///
/// §6 item 2: "CC8 makes the triangle a property of the report, names it in
/// `ColorGamutReport.definition`, and reports Rec.2020 representability for
/// HDR-profile content... CC6 §3.2/§3.3's normative range/gamut relation is
/// restated per triangle so the double-counting risk CC6 §14 names does not
/// multiply." So this is [`GAMUT_DEFINITION`]'s statement re-derived for the
/// wider triangle rather than reused: the test is on the **Rec.2020** linear
/// components the delivery matrix will consume, and the luma whose desaturation
/// metric this report prints is BT.2020's, not BT.709's.
pub const GAMUT_DEFINITION_REC2020: &str = "Out of gamut is min(r, g, b) < 0 after the working \
BT.709 linear triple is converted to Rec.2020 linear by CC8 §2.3's matrix: the set of pixels the \
HDR delivery lane itself cannot represent. Its luma is BT.2020's 0.2627 / 0.6780 / 0.0593, because \
the components it measures are Rec.2020's. This report and the Rec.709 one describe one pixel set \
from two sides and must not be summed, exactly as the range and gamut reports must not be.";

/// The normative relation between the two triangles' reports (CC8 §6 item 2).
///
/// §12's third risk requires this to be "printed as a line": "CC6 §14 already
/// names two reports over one pixel set as an invitation to double-count; CC8
/// makes it two triangles as well. *Mitigation:* §6 requires both to be
/// reported only where they differ, with the relation normative and printed as
/// a line."
///
/// The relation is a strict nesting, and it is a fact about the triangles
/// rather than about any raster: Rec.709's primaries lie inside Rec.2020's, so
/// every colour Rec.2020 cannot represent is one Rec.709 cannot represent
/// either. That makes exactly one subtraction meaningful and every addition
/// wrong, and the line says which.
pub const GAMUT_TRIANGLE_RELATION: &str = "The Rec.709 triangle is contained in the Rec.2020 \
triangle, so the out-of-Rec.2020 pixels are a subset of the out-of-Rec.709 pixels and the two \
counts must never be summed. The one meaningful arithmetic is the difference: \
rec709.out_of_gamut_pixel_count - rec2020.out_of_gamut_pixel_count is the count of pixels this \
HDR delivery can carry and an SDR Rec.709 delivery cannot, which is the SDR-compatibility signal. \
Both reports are published only when they differ; when they agree, one report is the whole fact.";

/// Representability of a region's colours in the Rec.709 triangle.
///
/// Gamut is representability, not brightness. A linear Rec.709 triple is
/// outside the chromaticity triangle exactly when a channel is negative. What
/// this report adds over [`ColorRangeReport`] is the *amount* of colour that is
/// unrepresentable:
///
/// ```text
/// Y = 0.2126·r + 0.7152·g + 0.0722·b
/// m = min(r, g, b)                       ( < 0 for an out-of-gamut pixel )
/// d = -m / (Y - m)                       desaturation toward this pixel's own luma
/// ```
///
/// `Y` is a convex combination with strictly positive weights, so `Y ≥ m`
/// always. Given `Y > 0` and `m < 0`, `d ∈ (0, 1)`, approaching `1` as
/// `Y → 0⁺`. `d` is **only** bounded when `Y > 0`: for `m < Y < 0` it exceeds
/// `1` and diverges as `Y → m⁺`, and no blend toward luma can reach `min = 0`
/// because the luma itself is negative. Those pixels are counted in
/// [`Self::below_black_pixel_count`], are still out of gamut, and are excluded
/// from [`Self::maximum_desaturation_millionths`] — only the *metric* is
/// undefined for them.
///
/// Reporting `d` is a measurement, not a mapping: nothing applies it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ColorGamutReport {
    pub out_of_gamut_pixel_count: u64,
    pub out_of_gamut_basis_points: u32,
    /// `min(min(r, g, b), 0.0)` over the region, millionths. Never positive.
    pub minimum_linear_millionths: i64,
    /// The maximum of `d` over out-of-gamut pixels with `Y > 0`, millionths.
    pub maximum_desaturation_millionths: i64,
    /// Out-of-gamut pixels with `Y < 0`, excluded from the maximum above.
    pub below_black_pixel_count: u64,
    /// [`GAMUT_TRIANGLE_REC709`] or [`GAMUT_TRIANGLE_REC2020`] — CC8 §6 item
    /// 2's "the triangle is a property of the report", as a field so a consumer
    /// does not have to parse [`Self::definition`] to learn which one it read.
    pub triangle: String,
    /// [`GAMUT_DEFINITION`] on the Rec.709 triangle, [`GAMUT_DEFINITION_REC2020`]
    /// on the Rec.2020 one.
    pub definition: String,
}

// ---------------------------------------------------------------------------
// §3.5: skin diagnostics.
// ---------------------------------------------------------------------------

/// The statement every skin diagnostic carries verbatim.
pub const SKIN_DIAGNOSTIC_BOUNDARY: &str = "This is a diagnostic of a region the user chose. It is \
not a skin detector, it does not find faces, and it makes no claim about whether a skin tone is \
good.";

/// Chroma floor below which a pixel has no usable hue, in millionths.
///
/// `atan2` on a near-zero vector is dominated by quantization noise, so a pixel
/// below the floor contributes to `excluded_achromatic_pixel_count` and to
/// nothing else. `0.02 · 224 = 4.48` eight-bit code units of excursion from
/// 128, so the floor is a few codes rather than a fraction of one; the least
/// saturated CC5 skin patch, `skin_deep`, measures `chroma = 0.073341`, which
/// is 3.67x the floor.
pub const SKIN_MIN_CHROMA_MILLIONTHS: i64 = 20_000;

/// The four CC5 skin patches' hues, in centidegrees, measured counter-clockwise
/// from the `+Cb` axis.
///
/// `skin_light` and `skin_tan` genuinely share an angle: their `grade709`
/// triples differ by the constant vector `(0.30, 0.30, 0.30)`, so their encoded
/// channel differences — the only inputs to `Cb` and `Cr` — agree to within
/// `1e-6`. It is not a transcription error.
pub const SKIN_PATCH_HUE_CENTIDEGREES: [i32; 4] = [12_385, 12_396, 12_385, 12_188];

/// The circular mean of [`SKIN_PATCH_HUE_CENTIDEGREES`] (`R = 0.999885`).
///
/// Derived from the CC5 patches, not borrowed. That it lands within `0.39°` of
/// the derived NTSC `+I` axis at exactly `123.0000°` is corroboration,
/// recorded as such.
pub const SKIN_BAND_CENTER_CENTIDEGREES: i32 = 12_339;

/// Half-width of the skin hue band, in centidegrees (`12.00°`).
///
/// The band is `[111.39°, 135.39°]`. The tightest CC5 patch, `skin_deep`, sits
/// `10.49°` inside the lower edge; the Rec.709 red primary at `102.91°` sits
/// `8.48°` outside it.
pub const SKIN_BAND_HALF_WIDTH_CENTIDEGREES: i32 = 1_200;

/// In-band rate below which a `skin_region_outside_band` [`QaSeverity::Info`]
/// exception is raised.
///
/// Info, not Warning: a chosen region that is not skin is a user choice, not a
/// fault.
pub const SKIN_BAND_EXCEPTION_BASIS_POINTS: u32 = 5_000;

/// The circular-spread ceiling, in centidegrees.
///
/// Normative: `−2·ln R` diverges as `R → 0`, and a diagnostic must not print an
/// unbounded number. `R == 0` and `considered_pixel_count == 0` both report
/// exactly this value.
pub const SKIN_MAX_SPREAD_CENTIDEGREES: i32 = 18_000;

/// Circular hue and chroma statistics for one chosen region.
///
/// See [`SKIN_DIAGNOSTIC_BOUNDARY`], which every response carries verbatim.
/// `θ` is measured counter-clockwise from the `+Cb` axis, which reproduces the
/// conventional vectorscope graticule; the real BT.709 matrix is used
/// deliberately, **not** CC2's integer vectorscope axes, which are a display
/// convenience with a different geometry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkinDiagnostics {
    pub region_pixel_count: u64,
    /// Pixels whose chroma is at or above [`SKIN_MIN_CHROMA_MILLIONTHS`].
    pub considered_pixel_count: u64,
    pub excluded_achromatic_pixel_count: u64,
    /// Circular mean hue, `None` when nothing was considered.
    pub mean_hue_centidegrees: Option<i32>,
    /// `R = |mean resultant vector|`, clamped to `[0, 1]`, in millionths.
    pub hue_concentration_millionths: i64,
    /// `degrees(sqrt(−2·ln R))`, capped at [`SKIN_MAX_SPREAD_CENTIDEGREES`].
    pub circular_spread_centidegrees: i32,
    /// The lower median chroma over considered pixels, millionths.
    pub median_chroma_millionths: i64,
    /// In-band rate as basis points **of considered pixels**: an achromatic
    /// pixel has no hue and cannot be in or out of a hue band. `0` when
    /// nothing was considered.
    pub in_band_basis_points: u32,
    /// Always [`SKIN_BAND_CENTER_CENTIDEGREES`].
    pub band_center_centidegrees: i32,
    /// Always [`SKIN_BAND_HALF_WIDTH_CENTIDEGREES`].
    pub band_half_width_centidegrees: i32,
    /// Always [`SKIN_DIAGNOSTIC_BOUNDARY`].
    pub boundary: String,
}

// ---------------------------------------------------------------------------
// CC8 §6 item 3: MaxCLL and MaxFALL, measured and reported, never gated.
// ---------------------------------------------------------------------------

/// The statement every [`ColorLightLevelReport`] carries verbatim.
///
/// CC8 §6 item 3: "Computed from the working proof over the sampled frames, in
/// the CC6 evidence style, with the sampled population and its bounds recorded
/// beside the number. They are reported because an editor should be able to see
/// them and because a PQ lane (§0.2 Q1) would need them as *inputs*; they are
/// not gated because CC8 does not write them into a file and a threshold on
/// them would be invented."
///
/// The bound this sentence records is the one the population forces: a report
/// measures **one** working proof, so its `MaxCLL` is a lower bound on a
/// programme's and its `MaxFALL` is this frame's average, not the maximum over
/// frames. Printing it as if it were the programme's figure would be the
/// invented number §6 refuses.
pub const LIGHT_LEVEL_BOUNDARY: &str = "MaxCLL and MaxFALL are measured, reported, and never \
gated: CC8 §0.2 Q1 chose HLG, which carries no static metadata, so nothing writes these numbers \
into a file and any threshold on them would be invented. Light level is max(r, g, b) of the \
unclamped working sample scaled by CC8 §2.2's reference-white anchor, so a frame whose channels \
are all negative reports a negative light level rather than a clamped zero. The population is one \
working proof: this MaxCLL is a lower bound on a whole programme's, and this MaxFALL is this \
frame's average light level, not the maximum over frames.";

/// Content light level over one measured region (CC8 §6 item 3).
///
/// Units are **hundredths of a cd/m²**, following CC6's rule that no QC API
/// returns a float and PSNR's own hundredths-of-a-dB precedent. The conversion
/// from working linear to cd/m² is `nits = working · CC8_REFERENCE_WHITE_NITS`,
/// the §2.2 anchor read from `kinewright_core::cc8_hdr` and reported in
/// [`Self::reference_white_nits`] so the number can be re-derived from the
/// report alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ColorLightLevelReport {
    /// `MaxCLL`: the largest per-pixel `max(r, g, b)` over the population, in
    /// hundredths of a cd/m². `0` when the population was empty.
    pub max_cll_nits_hundredths: i64,
    /// `MaxFALL`: the mean per-pixel `max(r, g, b)` over the population, in
    /// hundredths of a cd/m². `0` when the population was empty.
    pub max_fall_nits_hundredths: i64,
    /// Frames this measurement spans. Always `1`: [`measure_color_qc`] measures
    /// one working proof, and the number is stated rather than implied so a
    /// consumer cannot read a single frame as a programme.
    pub sampled_frame_count: u32,
    /// The pixels the two numbers were taken over — visible and finite, which
    /// is the same population every other accumulator in this report uses.
    pub sampled_pixel_count: u64,
    /// The project frame the population came from.
    pub project_frame: i64,
    /// Always [`crate::cc8_hdr::CC8_REFERENCE_WHITE_NITS`].
    pub reference_white_nits: i32,
    /// Always `false`. CC8 §6 item 3 and §11: deliberately produced, and
    /// deliberately unapplied.
    pub gated: bool,
    /// Always [`LIGHT_LEVEL_BOUNDARY`].
    pub boundary: String,
}

// ---------------------------------------------------------------------------
// CC8 §6 item 4: HDR skin diagnostics, withheld with a named reason.
// ---------------------------------------------------------------------------

/// The exception code a withheld HDR skin report raises.
pub const SKIN_WITHHELD_CODE: &str = "hdr_skin_diagnostics_withheld";

/// The named reason CC8 §6 item 4 requires a withheld skin report to carry.
///
/// §6 item 4, verbatim: "CC6's band constants were derived at
/// `θ(+I) = 123.0000°` in `grade709` against Rec.709 primaries (`P4`). Those
/// numbers do not transfer to Rec.2020 or to HDR luminance, and re-deriving
/// them is a measurement programme. On an HDR-profile source the skin report is
/// **withheld with a named reason**, not silently computed against the wrong
/// constants. This is the deferral CC8 is most likely to be asked to reverse,
/// and it should be reversed by measurement or not at all."
pub const SKIN_WITHHELD_REASON: &str = "The skin diagnostic is withheld on a CC8 HDR source. CC6's \
band constants were derived in grade709 against Rec.709 primaries at SDR luminance - the four CC5 \
patch angles, their circular mean at 123.39 degrees, and the 12.00 degree half-width - and none of \
them transfers to Rec.2020 primaries or to HDR magnitudes. Computing them here would publish a hue \
statistic against the wrong constants, which is a wrong number rather than an approximate one. CC8 \
section 6 item 4 defers HDR skin diagnostics: re-deriving the band is a measurement programme, and \
this deferral should be reversed by measurement or not at all.";

/// What a report says instead of a skin diagnostic on an HDR source.
///
/// A named limitation, never silence: §12's mitigation for "HDR content will
/// make the existing SDR-shaped nodes look broken" is that "both limitations
/// are surfaced in the UI at the node that has them, not documented in a file
/// nobody opens", and this is the QC half of that.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkinWithheldReport {
    /// Always [`SKIN_WITHHELD_CODE`].
    pub code: String,
    /// The [`ColorSourceProfile::id`] that caused the withholding.
    pub source_profile: String,
    /// Always [`SKIN_WITHHELD_REASON`].
    pub reason: String,
    /// Always [`SKIN_DIAGNOSTIC_BOUNDARY`], so the withheld row carries the
    /// same boundary statement the published diagnostic would have.
    pub boundary: String,
}

// ---------------------------------------------------------------------------
// §3.7 report types (the measurement itself lives in `nodes`).
// ---------------------------------------------------------------------------

/// The maximum number of colour nodes one per-node attribution may report.
///
/// The bound is a cost bound: each reported node costs one full-resolution
/// scratch render and one `Arc<Document>` deep clone, on top of the single
/// baseline render.
pub const MAX_QC_NODE_CONTRIBUTIONS: usize = 16;

/// The only attribution method CC6 uses, stated so a consumer cannot mistake it.
pub const NODE_ATTRIBUTION_REMOVED: &str = "node_removed";

/// One colour node's contribution to the region's clipping.
///
/// Deltas are **with-all minus with-this-node-removed**, so a positive value
/// means this node adds clipping. Clipping is not additive: the deltas do not
/// sum to the total and no consumer may assume they do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ColorNodeQcContribution {
    pub clip: ClipId,
    pub effect: EffectId,
    pub node_kind: String,
    pub active: bool,
    /// [`crate::ColorNodeInactiveReason::as_str`] when the node is inactive.
    pub inactive_reason: Option<String>,
    pub range_basis_points_delta: i32,
    pub gamut_basis_points_delta: i32,
}

/// Per-node clipping attribution for one frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ColorQcNodeContributions {
    pub baseline_range_basis_points: u32,
    pub baseline_gamut_basis_points: u32,
    /// Candidate nodes found in the stated order, **before** truncation.
    pub considered_node_count: u32,
    pub truncated: bool,
    /// Always [`NODE_ATTRIBUTION_REMOVED`].
    pub attribution: String,
    pub nodes: Vec<ColorNodeQcContribution>,
}

// ---------------------------------------------------------------------------
// §3.0: request and region.
// ---------------------------------------------------------------------------

/// One optional QC measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ColorQcCheck {
    Range,
    Gamut,
    Skin,
    Tags,
    PerNode,
}

/// A CC5 matte scope plus the coverage raster the matte proof produced.
///
/// [`MatteRegionDescription`] carries clip, effect, threshold, and a covered
/// pixel count — not pixels. Core needs the coverage image to scope a region,
/// so the request carries it; the agent obtains it from
/// `Analysis::matte_proof_for_document` and the app from its existing matte
/// proof source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatteRegionScope {
    pub description: MatteRegionDescription,
    /// `MatteProof.coverage`: `R = G = B = round(255 · m)` with `A = 255`.
    pub coverage: RgbaImage,
}

/// One colour QC measurement request.
#[derive(Debug, Clone, PartialEq)]
pub struct ColorQcRequest {
    /// CC2's half-open basis-point rect. `None` measures the whole raster.
    pub roi: Option<NormalizedRoi>,
    /// CC5's matte scope. Composable with `roi`: the region is the
    /// intersection.
    pub matte_region: Option<MatteRegionScope>,
    pub checks: Vec<ColorQcCheck>,
    pub delivery_bit_depth: DeliveryEncodeDepth,
    /// Pre-export tag mode: the materialised `ExportSettings.delivery_color`.
    pub expected_delivery: Option<crate::ColorDescription>,
    /// Post-export tag mode: the probed description of a written file.
    pub observed_delivery: Option<crate::ColorDescription>,
    /// `1..=`[`MAX_QC_NODE_CONTRIBUTIONS`]. Validated by every entry point, so
    /// an out-of-range budget is refused before any measurement runs.
    pub max_nodes: u8,
    /// The project frame identity this proof was rendered at, the same `i64`
    /// identity `ScopeMeasurementMetadata.project_frames` carries. CC6
    /// introduces no new frame-identity type.
    pub project_frame: i64,
    /// The classified source profile of the layer this measurement is about,
    /// when the caller resolved one (CC8 §6 items 2 and 4).
    ///
    /// It is a *source* question, deliberately separate from the delivery lane
    /// [`Self::delivery_lane`] derives: §6 item 4 withholds the skin diagnostic
    /// "on an HDR-profile source", which is true of an HDR project whose
    /// delivery is still SDR — exactly the project §7 item 2 blocks from
    /// exporting. Keying the withholding on the lane would publish CC6's band
    /// constants against Rec.2020 content on precisely the projects that most
    /// need to be told they cannot be.
    ///
    /// `None` is the CC6 answer — no profile resolved, so nothing is withheld
    /// and no second triangle is reported. Callers that hold a document resolve
    /// it with [`crate::document_hdr_source_profile`].
    pub source_profile: Option<ColorSourceProfile>,
}

impl ColorQcRequest {
    /// The delivery lane this measurement predicts (CC8 §5.2 clause 1).
    ///
    /// **Derived, never carried.** §5.2 clause 1 makes the delivery
    /// `ColorDescription` the single source of truth for every lane-derived
    /// term, and [`DeliveryLane::for_description`] "is the only place that
    /// reads it". A second `lane` field on this request could disagree with
    /// [`Self::expected_delivery`], which is the divergence class §5.2 clause 3
    /// makes a fixture out of on the export side; there is nothing to diverge
    /// if the lane is a function.
    ///
    /// A request with no expected delivery description is measured on the SDR
    /// lane, which is CC6's behaviour unchanged and is what keeps every
    /// CC1–CC7 measurement byte-identical (§9.1 fixture 6).
    #[must_use]
    pub fn delivery_lane(&self) -> DeliveryLane {
        self.expected_delivery
            .as_ref()
            .map_or(DeliveryLane::SdrRec709, DeliveryLane::for_description)
    }

    /// Whether this measurement is about HDR content at all — CC8 §6 item 2's
    /// "HDR-profile content".
    ///
    /// Either side makes it so: an HDR delivery lane means the second triangle
    /// is what the file will be written in, and an HDR source means the working
    /// raster carries colours the Rec.709 triangle cannot hold whatever the
    /// delivery is.
    #[must_use]
    pub fn is_hdr_context(&self) -> bool {
        self.delivery_lane().is_hdr() || self.source_profile.is_some_and(ColorSourceProfile::is_hdr)
    }
}

impl Default for ColorQcRequest {
    fn default() -> Self {
        Self {
            roi: None,
            matte_region: None,
            checks: vec![ColorQcCheck::Range, ColorQcCheck::Gamut, ColorQcCheck::Tags],
            delivery_bit_depth: DeliveryEncodeDepth::Eight,
            expected_delivery: None,
            observed_delivery: None,
            max_nodes: MAX_QC_NODE_CONTRIBUTIONS_U8,
            project_frame: 0,
            source_profile: None,
        }
    }
}

/// [`MAX_QC_NODE_CONTRIBUTIONS`] as the `u8` the request field carries.
const MAX_QC_NODE_CONTRIBUTIONS_U8: u8 = 16;

/// The resolved population one report was measured over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ColorQcRegion {
    pub normalized_roi: NormalizedRoi,
    pub pixel_roi: PixelRoi,
    pub matte_region: Option<MatteRegionDescription>,
    pub region_pixel_count: u64,
    pub visible_pixel_count: u64,
    /// Visible pixels holding a non-finite linear or encoded sample, which are
    /// counted here and fed to **no** accumulator. See
    /// [`ColorQcReport::non_finite_pixel_count`].
    pub non_finite_pixel_count: u64,
    /// Always `0` at this stage: the composite target is cleared to opaque
    /// black and the alpha blend is `One / OneMinusSrcAlpha`, so `a' = 1`
    /// everywhere. Retained for schema symmetry with
    /// `ScopeMeasurementMetadata`; it **must not** be used as a check.
    pub transparent_pixel_count: u64,
}

// ---------------------------------------------------------------------------
// §3.8: report, exceptions, provenance, refusals.
// ---------------------------------------------------------------------------

/// How a [`ColorQcReport`] was computed, so the choices are auditable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ColorQcProvenance {
    pub engine: String,
    /// `f32`: the QC engine computes `e` in the precision the delivery clamp
    /// it predicts uses.
    pub encoded_precision: String,
    /// `f64`: sums, circular sums, and MSE.
    pub accumulator_precision: String,
    pub pixel_iteration: String,
    pub channel_order: String,
    pub exception_order: String,
    pub rate_units: String,
}

/// The engine identity recorded in every [`ColorQcProvenance`].
pub const COLOR_QC_ENGINE: &str = "kinewright_color_qc_v1";

impl Default for ColorQcProvenance {
    fn default() -> Self {
        Self {
            engine: COLOR_QC_ENGINE.to_owned(),
            encoded_precision: "f32".to_owned(),
            accumulator_precision: "f64".to_owned(),
            pixel_iteration: "row_major_top_left".to_owned(),
            channel_order: "red_green_blue".to_owned(),
            exception_order: "severity_desc_code_asc_tiebreak_asc".to_owned(),
            rate_units: "integer_floor_basis_points".to_owned(),
        }
    }
}

/// One reportable QC finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ColorQcException {
    pub code: String,
    pub severity: QaSeverity,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub field: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub observed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub allowed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub clip: Option<ClipId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub effect: Option<EffectId>,
}

/// One colour QC measurement of one working proof.
///
/// Deliberately carries **no** `verification` field: a verification can only be
/// produced against a written file, which [`measure_color_qc`] has no access
/// to. It also deliberately does not reuse the name `export_ready`:
/// `QaReport::export_ready` gates an export, and a QC report must never gate
/// one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ColorQcReport {
    /// Always [`crate::WORKING_PROOF_STAGE`].
    pub stage: String,
    /// Always `true`: a proof that is not full-resolution is refused before
    /// any measurement runs.
    pub full_resolution: bool,
    pub raster: (u32, u32),
    pub project_frame: i64,
    pub region: ColorQcRegion,
    pub visible_pixel_count: u64,
    /// Visible pixels whose linear or delivery-encoded sample was not finite.
    ///
    /// A `NaN` compares `false` against every bound and an infinity saturates
    /// every extreme, so such a pixel cannot be classified in range, in gamut,
    /// or on a plane: it is counted here, excluded from the channel, gamut,
    /// `Y'CbCr` plane, and skin accumulators, and raised as the
    /// `color_qc_non_finite_sample` [`QaSeverity::Error`] exception, which
    /// clears [`Self::technical_pass`]. It is a subset of
    /// [`Self::visible_pixel_count`], which stays the rate denominator, so
    /// every basis-point rate remains a rate of the visible population and no
    /// count is silently rebased.
    pub non_finite_pixel_count: u64,
    /// Always `0` at this stage; see [`ColorQcRegion::transparent_pixel_count`].
    pub transparent_pixel_count: u64,
    /// 8 or 10; selects the §3.4 code scale.
    pub delivery_bit_depth: u8,
    /// [`DeliveryLane::as_str`] of the lane this measurement predicted — the
    /// lane whose transfer, primaries, and `Y'CbCr` matrix the range and
    /// legality reports were taken through (CC8 §6 item 1).
    ///
    /// Echoed from [`ColorQcRequest::delivery_lane`] so a reader never has to
    /// infer from the numbers which matrix produced them.
    pub delivery_lane: String,
    pub range: ColorRangeReport,
    /// The Rec.709-triangle gamut report, on every lane. CC6's, unchanged.
    pub gamut: ColorGamutReport,
    /// CC8 §6 item 2's Rec.2020-triangle report, published **only** on HDR
    /// content and **only** when it differs from [`Self::gamut`].
    ///
    /// §6 item 2: "**Both are reported when they differ**, because 'outside
    /// Rec.709 but inside Rec.2020' is exactly the fact an editor delivering
    /// HDR needs, and collapsing it loses the SDR-compatibility signal." §12's
    /// third risk supplies the other half — "§6 requires both to be reported
    /// only where they differ" — so two identical reports are never published
    /// as if they were two facts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub gamut_rec2020: Option<ColorGamutReport>,
    /// [`GAMUT_TRIANGLE_RELATION`], present exactly when
    /// [`Self::gamut_rec2020`] is: §12's "the relation normative and printed as
    /// a line".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub gamut_triangle_relation: Option<String>,
    /// CC8 §6 item 3's `MaxCLL`/`MaxFALL`, measured on every lane and gated on
    /// none.
    pub light_level: ColorLightLevelReport,
    pub skin: Option<SkinDiagnostics>,
    /// CC8 §6 item 4: why there is no skin diagnostic on an HDR source.
    ///
    /// `Some` exactly when [`ColorQcRequest::source_profile`] resolved to one
    /// of §2.1's HDR profiles, whether or not [`ColorQcCheck::Skin`] was asked
    /// for — the limitation is a property of the content, and a report that
    /// mentions it only when someone happens to ask is the silence §6 refuses.
    /// [`Self::skin`] is `None` whenever this is `Some`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub skin_withheld: Option<SkinWithheldReport>,
    pub tags: Option<DeliveryTagCheck>,
    pub nodes: Option<ColorQcNodeContributions>,
    pub exceptions: Vec<ColorQcException>,
    /// No `Error`-severity exception. Warnings do not clear it: a blown
    /// highlight is frequently a deliberate creative choice, while a mis-tagged
    /// file is never one.
    pub technical_pass: bool,
    /// Always `true`.
    pub evidence_only: bool,
    pub provenance: ColorQcProvenance,
}

/// A typed refusal to publish a colour QC measurement.
///
/// Mirrors [`crate::MatteCoverageError`]: a `const fn code`, plus `field`,
/// `observed`, and `allowed_values` on every variant, so a refusal is data
/// rather than one opaque sentence.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ColorQcError {
    #[error(
        "color_qc_proxy_proof_refused: working proof claims full_resolution={observed}, allowed {allowed}"
    )]
    ProxyProofRefused {
        observed: String,
        allowed: &'static str,
    },
    #[error("color_qc_raster_length_mismatch: pixel buffer is {observed}, allowed {allowed}")]
    RasterLengthMismatch { observed: String, allowed: String },
    #[error("color_qc_region_empty: resolved region is {observed}, allowed {allowed}")]
    EmptyPopulation {
        observed: String,
        allowed: &'static str,
    },
    #[error("color_qc_node_budget_exceeded: max_nodes is {observed}, allowed {allowed}")]
    NodeBudgetExceeded {
        observed: String,
        allowed: &'static str,
    },
    #[error(
        "color_qc_matte_region_raster_mismatch: coverage raster is {observed}, allowed {allowed}"
    )]
    MatteRegionRasterMismatch { observed: String, allowed: String },
    /// The per-node attribution's scratch [`crate::Operation::RemoveEffect`]
    /// was refused by the document model (CC6 §3.7).
    ///
    /// A document-model failure rather than a measurement refusal, but it is a
    /// refusal to publish a QC measurement all the same, so it travels as one:
    /// flattening it into [`MediaError::Backend`] left an agent surface with no
    /// code to report but `working_proof_unavailable`, which is not what
    /// happened — the proof rendered fine and the removal was rejected.
    ///
    /// **Nothing was mutated.** The removal is attempted on a clone; the live
    /// document is untouched whether it succeeds or not.
    #[error(
        "color_qc_node_removal_rejected: removing effect {effect} from a scratch clone of clip {clip} was rejected: {reason}"
    )]
    NodeRemovalRejected {
        clip: ClipId,
        effect: EffectId,
        reason: String,
    },
}

impl ColorQcError {
    /// Stable machine-readable status code for agent and UI consumers.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ProxyProofRefused { .. } => "color_qc_proxy_proof_refused",
            Self::RasterLengthMismatch { .. } => "color_qc_raster_length_mismatch",
            Self::EmptyPopulation { .. } => "color_qc_region_empty",
            Self::NodeBudgetExceeded { .. } => "color_qc_node_budget_exceeded",
            Self::MatteRegionRasterMismatch { .. } => "color_qc_matte_region_raster_mismatch",
            Self::NodeRemovalRejected { .. } => "color_qc_node_removal_rejected",
        }
    }

    /// Stable request or proof field associated with the refusal.
    #[must_use]
    pub const fn field(&self) -> &'static str {
        match self {
            Self::ProxyProofRefused { .. } => "full_resolution",
            Self::RasterLengthMismatch { .. } => "pixels",
            Self::EmptyPopulation { .. } => "region",
            Self::NodeBudgetExceeded { .. } => "max_nodes",
            Self::MatteRegionRasterMismatch { .. } => "coverage",
            Self::NodeRemovalRejected { .. } => "effects",
        }
    }

    /// Observed value formatted for a structured status surface.
    #[must_use]
    pub fn observed(&self) -> String {
        match self {
            Self::ProxyProofRefused { observed, .. }
            | Self::RasterLengthMismatch { observed, .. }
            | Self::EmptyPopulation { observed, .. }
            | Self::NodeBudgetExceeded { observed, .. }
            | Self::MatteRegionRasterMismatch { observed, .. } => observed.clone(),
            Self::NodeRemovalRejected {
                clip,
                effect,
                reason,
            } => format!("clip {clip} effect {effect}: {reason}"),
        }
    }

    /// Allowed values for the failed field.
    #[must_use]
    pub fn allowed_values(&self) -> String {
        match self {
            Self::ProxyProofRefused { allowed, .. }
            | Self::EmptyPopulation { allowed, .. }
            | Self::NodeBudgetExceeded { allowed, .. } => (*allowed).to_owned(),
            Self::RasterLengthMismatch { allowed, .. }
            | Self::MatteRegionRasterMismatch { allowed, .. } => allowed.clone(),
            Self::NodeRemovalRejected { .. } => {
                "an effect the document model permits removing from the clip it is attached to"
                    .to_owned()
            }
        }
    }

    /// Recovery action suitable for a visible status or agent response.
    #[must_use]
    pub const fn recovery_action(&self) -> &'static str {
        match self {
            Self::ProxyProofRefused { .. } => {
                "Request a working proof, which always binds full resolution. There is no proxy working proof, so a proxy raster can never be measured."
            }
            Self::RasterLengthMismatch { .. } => {
                "Re-read the working proof: its buffer must hold width x height x 4 f32 samples."
            }
            Self::EmptyPopulation { .. } => {
                "Widen the region of interest or choose a matte whose coverage is non-empty."
            }
            Self::NodeBudgetExceeded { .. } => {
                "Request between 1 and 16 nodes; each reported node costs one full-resolution scratch render."
            }
            Self::MatteRegionRasterMismatch { .. } => {
                "Obtain the matte coverage from a full-resolution matte proof of the same document and frame."
            }
            Self::NodeRemovalRejected { .. } => {
                "Read the document model's own reason above: per-node attribution removes each candidate effect from a scratch clone, so a rejected removal describes the document, not the render. Nothing was mutated."
            }
        }
    }

    /// Render the complete actionable status while retaining the structured
    /// accessors above for machine consumers.
    #[must_use]
    pub fn actionable_message(&self) -> String {
        format!(
            "{} (field={}, observed={}, allowed={}). {}",
            self,
            self.field(),
            self.observed(),
            self.allowed_values(),
            self.recovery_action()
        )
    }
}

/// A QC refusal travels through [`MediaError`] structurally, never as a
/// flattened `Backend` string, so `?` in a renderer-backed path keeps the typed
/// code recoverable through [`MediaError::recovery_code`] (§9.7, errata E32).
impl From<ColorQcError> for MediaError {
    fn from(error: ColorQcError) -> Self {
        Self::ColorQc(error)
    }
}

// ---------------------------------------------------------------------------
// §3.0/§3.1: the measurement.
// ---------------------------------------------------------------------------

/// Measure one working proof.
///
/// Pure: no renderer, no I/O, no clock, no RNG. Iteration is row-major from the
/// top-left, matching the compositor's own readback order, so a partial-sum
/// reordering cannot change a floating-point accumulation.
///
/// [`ColorRangeReport`], [`ColorGamutReport`] and CC8 §6 item 3's
/// [`ColorLightLevelReport`] are always produced — they are one pass over the
/// same pixels and they are what the report is *for*. `checks` selects the
/// optional sections: [`ColorQcCheck::Skin`] produces
/// [`ColorQcReport::skin`], and [`ColorQcCheck::Tags`] produces
/// [`ColorQcReport::tags`] when the request carries an expected delivery
/// description. [`ColorQcCheck::PerNode`] is never honoured here: per-node
/// attribution needs a renderer and lives in [`nodes`], whose result the caller
/// attaches with [`attach_node_contributions`].
///
/// **Lane-aware, from CC8 §6.** [`ColorQcRequest::delivery_lane`] selects the
/// delivery encode and the `Y'CbCr` matrix every range and legality number is
/// taken through (item 1); [`ColorQcRequest::is_hdr_context`] adds §6 item 2's
/// Rec.2020 triangle when it differs from the Rec.709 one; and
/// [`ColorQcRequest::source_profile`] withholds the skin diagnostic with §6
/// item 4's named reason on an HDR source. On the SDR lane with no source
/// profile — CC6's request, unchanged — every one of those is the identity, so
/// a CC1–CC7 measurement is byte-identical (§9.1 fixture 6).
///
/// # Errors
///
/// Returns [`ColorQcError`] when the proof is not full-resolution, its buffer
/// length disagrees with its dimensions, the node budget is out of range, a
/// matte coverage raster does not match the proof raster, or the resolved
/// region holds no visible pixel.
#[allow(clippy::too_many_lines)]
pub fn measure_color_qc(
    proof: &WorkingProof,
    request: &ColorQcRequest,
) -> Result<ColorQcReport, ColorQcError> {
    // CC6 §2.3: refuse before measuring. A raster that cannot claim the full
    // document raster cannot be spoken about as if it were the delivery input.
    if !proof.metadata.render.full_resolution {
        return Err(ColorQcError::ProxyProofRefused {
            observed: "false".to_owned(),
            allowed: "true (a working proof always binds full resolution)",
        });
    }
    let image = &proof.image;
    let expected_len = u64::from(image.width)
        .saturating_mul(u64::from(image.height))
        .saturating_mul(4);
    if u64::try_from(image.pixels.len()).unwrap_or(u64::MAX) != expected_len {
        return Err(ColorQcError::RasterLengthMismatch {
            observed: format!("{} f32 samples", image.pixels.len()),
            allowed: format!(
                "{expected_len} f32 samples for {}x{}",
                image.width, image.height
            ),
        });
    }
    validate_node_budget(request.max_nodes)?;

    let normalized_roi = request.roi.unwrap_or_else(NormalizedRoi::full_frame);
    let pixel_roi = normalized_roi
        .to_pixels(image.width, image.height)
        .map_err(|error| ColorQcError::EmptyPopulation {
            observed: error.to_string(),
            allowed: "a region of interest covering at least one source pixel",
        })?;
    if let Some(scope) = &request.matte_region {
        // Both the dimensions and the buffer they claim: a coverage raster
        // whose dimensions agree but whose buffer is short would silently
        // scope the region to the pixels that happen to exist, because the
        // per-pixel read below falls back to coverage `0`. That is a smaller
        // region reported as the requested one, so it is refused here.
        if scope.coverage.width != image.width
            || scope.coverage.height != image.height
            || u64::try_from(scope.coverage.pixels.len()).unwrap_or(u64::MAX) != expected_len
        {
            return Err(ColorQcError::MatteRegionRasterMismatch {
                observed: format!(
                    "{}x{} with {} u8 samples",
                    scope.coverage.width,
                    scope.coverage.height,
                    scope.coverage.pixels.len()
                ),
                allowed: format!(
                    "{}x{} with {expected_len} u8 samples",
                    image.width, image.height
                ),
            });
        }
    }

    let bits = request.delivery_bit_depth.bits();
    let lane = request.delivery_lane();
    let hdr_context = request.is_hdr_context();
    // CC8 §6 item 4: an HDR source withholds the diagnostic rather than
    // measuring it against constants that do not transfer, so the skin pass is
    // not merely unpublished — it never runs, and the chroma buffer it would
    // have filled is never allocated.
    let withheld_skin = request
        .source_profile
        .filter(|profile| profile.is_hdr())
        .map(|profile| SkinWithheldReport {
            code: SKIN_WITHHELD_CODE.to_owned(),
            source_profile: profile.id().to_owned(),
            reason: SKIN_WITHHELD_REASON.to_owned(),
            boundary: SKIN_DIAGNOSTIC_BOUNDARY.to_owned(),
        });
    let measure_skin = request.checks.contains(&ColorQcCheck::Skin) && withheld_skin.is_none();
    // The region's own upper bound, so the median buffer is allocated once
    // rather than doubling its way to the raster size. A matte narrows it
    // further: the coverage count is the most pixels the scope can admit, and
    // reserving the whole rectangle for a small matte would be the same waste
    // in the other direction.
    let skin_capacity = if measure_skin {
        let area = u64::from(pixel_roi.width).saturating_mul(u64::from(pixel_roi.height));
        let bound = request.matte_region.as_ref().map_or(area, |scope| {
            area.min(scope.description.covered_pixel_count)
        });
        usize::try_from(bound).unwrap_or(0)
    } else {
        0
    };
    let mut accumulator = RegionAccumulator::new(bits, lane, hdr_context, skin_capacity);
    for y in pixel_roi.y..pixel_roi.bottom() {
        for x in pixel_roi.x..pixel_roi.right() {
            let index = (y as usize * image.width as usize + x as usize) * 4;
            if let Some(scope) = &request.matte_region {
                // `MATTE_SCOPE_THRESHOLD`: coverage greater than zero, the set
                // the correction touched at all. Coverage is grey, so the red
                // channel is the coverage code.
                if scope.coverage.pixels.get(index).copied().unwrap_or(0) == 0 {
                    continue;
                }
            }
            let Some(pixel) = image.pixels.get(index..index + 4) else {
                continue;
            };
            accumulator.add([pixel[0], pixel[1], pixel[2]], pixel[3], measure_skin);
        }
    }

    if accumulator.region_pixel_count == 0 {
        return Err(ColorQcError::EmptyPopulation {
            observed: "0 pixels".to_owned(),
            allowed: "at least one pixel inside the region of interest and the matte coverage",
        });
    }
    if accumulator.visible_pixel_count == 0 {
        return Err(ColorQcError::EmptyPopulation {
            observed: format!(
                "0 visible pixels of {} region pixels",
                accumulator.region_pixel_count
            ),
            allowed: "at least one pixel with alpha greater than zero",
        });
    }

    let region = ColorQcRegion {
        normalized_roi,
        pixel_roi,
        matte_region: request
            .matte_region
            .as_ref()
            .map(|scope| scope.description.clone()),
        region_pixel_count: accumulator.region_pixel_count,
        visible_pixel_count: accumulator.visible_pixel_count,
        non_finite_pixel_count: accumulator.non_finite_pixel_count,
        transparent_pixel_count: accumulator.transparent_pixel_count,
    };
    let visible = accumulator.visible_pixel_count;
    let range = accumulator.range_report(visible);
    let gamut = accumulator.gamut_report(visible);
    // CC8 §6 item 2 and §12's third risk: the second triangle is published only
    // where it differs. Two identical reports are one fact printed twice, which
    // is the double-counting invitation CC6 §14 already names.
    let gamut_rec2020 = accumulator
        .rec2020_gamut_report(visible)
        .filter(|rec2020| !gamut_reports_agree(&gamut, rec2020));
    let skin = measure_skin.then(|| accumulator.skin_diagnostics());
    let tags = request
        .checks
        .contains(&ColorQcCheck::Tags)
        .then(|| tag_check_for(request))
        .flatten();

    let mut report = ColorQcReport {
        stage: crate::WORKING_PROOF_STAGE.to_owned(),
        full_resolution: true,
        raster: (image.width, image.height),
        project_frame: request.project_frame,
        region,
        visible_pixel_count: visible,
        non_finite_pixel_count: accumulator.non_finite_pixel_count,
        transparent_pixel_count: accumulator.transparent_pixel_count,
        delivery_bit_depth: bits,
        delivery_lane: lane.as_str().to_owned(),
        range,
        gamut,
        gamut_triangle_relation: gamut_rec2020
            .is_some()
            .then(|| GAMUT_TRIANGLE_RELATION.to_owned()),
        gamut_rec2020,
        light_level: accumulator.light_level_report(request.project_frame),
        skin,
        skin_withheld: withheld_skin,
        tags,
        nodes: None,
        exceptions: Vec::new(),
        technical_pass: true,
        evidence_only: true,
        provenance: ColorQcProvenance::default(),
    };
    report.exceptions = report_exceptions(&report);
    finish(&mut report);
    Ok(report)
}

/// Attach a per-node attribution to a report, with its truncation exception.
///
/// Kept in core so the `qc_per_node_truncated` exception, the exception
/// ordering, and `technical_pass` are all owned by one place rather than
/// reassembled by every consumer.
pub fn attach_node_contributions(
    report: &mut ColorQcReport,
    contributions: ColorQcNodeContributions,
) {
    if contributions.truncated {
        report.exceptions.push(ColorQcException {
            code: "qc_per_node_truncated".to_owned(),
            severity: QaSeverity::Info,
            message: format!(
                "{} candidate colour nodes were found; only the first {} are attributed, in track, clip, then effect-chain order.",
                contributions.considered_node_count,
                contributions.nodes.len()
            ),
            field: Some("nodes".to_owned()),
            observed: Some(contributions.considered_node_count.to_string()),
            allowed: Some(MAX_QC_NODE_CONTRIBUTIONS.to_string()),
            clip: None,
            effect: None,
        });
    }
    report.nodes = Some(contributions);
    finish(report);
}

/// Whether two triangles' gamut reports say the same thing about one raster.
///
/// Everything except the two strings that name the triangle: those differ by
/// construction and comparing them would make "they differ" true on every HDR
/// measurement, which is precisely the double-printing §12's third risk asks to
/// avoid.
fn gamut_reports_agree(left: &ColorGamutReport, right: &ColorGamutReport) -> bool {
    left.out_of_gamut_pixel_count == right.out_of_gamut_pixel_count
        && left.out_of_gamut_basis_points == right.out_of_gamut_basis_points
        && left.minimum_linear_millionths == right.minimum_linear_millionths
        && left.maximum_desaturation_millionths == right.maximum_desaturation_millionths
        && left.below_black_pixel_count == right.below_black_pixel_count
}

/// Sort the exceptions and derive `technical_pass` from them.
fn finish(report: &mut ColorQcReport) {
    report.exceptions.sort_by(|left, right| {
        severity_rank(left.severity)
            .cmp(&severity_rank(right.severity))
            .then_with(|| left.code.cmp(&right.code))
            .then_with(|| exception_tiebreak(left).cmp(&exception_tiebreak(right)))
    });
    report.technical_pass = !report
        .exceptions
        .iter()
        .any(|exception| exception.severity == QaSeverity::Error);
}

/// `Error` first, then `Warning`, then `Info`: severity descending.
const fn severity_rank(severity: QaSeverity) -> u8 {
    match severity {
        QaSeverity::Error => 0,
        QaSeverity::Warning => 1,
        QaSeverity::Info => 2,
    }
}

/// The code's own tiebreak: field, then observed, then clip and effect.
///
/// Borrowed, not cloned: the comparator runs `O(n log n)` times and the two
/// `String` clones it used to make were pure allocation. A missing field or
/// observed value still sorts as the empty string, which is the same order.
fn exception_tiebreak(exception: &ColorQcException) -> (&str, &str, Option<u64>, Option<u64>) {
    (
        exception.field.as_deref().unwrap_or_default(),
        exception.observed.as_deref().unwrap_or_default(),
        exception.clip.map(|clip| clip.0),
        exception.effect.map(|effect| effect.0),
    )
}

/// Reject a node budget outside `1..=`[`MAX_QC_NODE_CONTRIBUTIONS`].
///
/// # Errors
///
/// Returns [`ColorQcError::NodeBudgetExceeded`] for `0` and for anything above
/// the bound.
pub fn validate_node_budget(max_nodes: u8) -> Result<(), ColorQcError> {
    if max_nodes == 0 || usize::from(max_nodes) > MAX_QC_NODE_CONTRIBUTIONS {
        return Err(ColorQcError::NodeBudgetExceeded {
            observed: max_nodes.to_string(),
            allowed: "1..=16",
        });
    }
    Ok(())
}

/// Materialize the tag check the request describes, if it describes one.
fn tag_check_for(request: &ColorQcRequest) -> Option<DeliveryTagCheck> {
    let expected = request.expected_delivery.as_ref()?;
    Some(match &request.observed_delivery {
        Some(observed) => crate::delivery_tag_check(
            expected,
            observed,
            crate::DeliveryTagSource::ProbedOutputFile,
        ),
        None => crate::delivery_tag_check(
            expected,
            expected,
            crate::DeliveryTagSource::MaterialisedExportSettings,
        ),
    })
}

/// The `color_qc_non_finite_sample` refusal, when the pass saw one.
///
/// [`QaSeverity::Error`], not a warning: every other count in this report is a
/// count of pixels that *were* classified, and a non-finite sample is one that
/// could not be. A measurement with a hole in its population must not read as
/// a clean pass, and a blown highlight — the thing warnings are for — is a
/// creative choice, while a `NaN` in the composite never is.
fn non_finite_exception(report: &ColorQcReport) -> Option<ColorQcException> {
    if report.non_finite_pixel_count == 0 {
        return None;
    }
    Some(ColorQcException {
        code: "color_qc_non_finite_sample".to_owned(),
        severity: QaSeverity::Error,
        message: format!(
            "{} of {} visible pixels carry a non-finite linear or delivery-encoded sample. They are counted and excluded from every range, gamut, plane, and skin accumulator: a NaN compares false against every bound and an infinity saturates every extreme, so classifying one would be inventing evidence. The renderer that produced this raster is the fault, not the grade.",
            report.non_finite_pixel_count, report.visible_pixel_count
        ),
        field: Some("non_finite_pixel_count".to_owned()),
        observed: Some(report.non_finite_pixel_count.to_string()),
        allowed: Some("0".to_owned()),
        clip: None,
        effect: None,
    })
}

/// CC8 §6 item 2's second-triangle warning, when the report has one and it
/// reaches [`QC_GAMUT_EXCEPTION_BASIS_POINTS`].
///
/// Its **own** code, never `delivery_gamut_excursion`. Reusing that name would
/// put two rows with one name in front of a reader and invite exactly the
/// addition §12's third risk warns about, so the code differs and the message
/// carries [`GAMUT_TRIANGLE_RELATION`] — an exceptions-only surface still
/// learns it must not sum the two.
fn rec2020_gamut_exception(report: &ColorQcReport) -> Option<ColorQcException> {
    let rec2020 = report.gamut_rec2020.as_ref()?;
    if rec2020.out_of_gamut_basis_points < QC_GAMUT_EXCEPTION_BASIS_POINTS {
        return None;
    }
    Some(ColorQcException {
        code: "delivery_gamut_excursion_rec2020".to_owned(),
        severity: QaSeverity::Warning,
        message: format!(
            "{} visible pixels ({} basis points) are outside the Rec.2020 chromaticity triangle, which the HDR delivery lane itself cannot represent. {}",
            rec2020.out_of_gamut_pixel_count,
            rec2020.out_of_gamut_basis_points,
            GAMUT_TRIANGLE_RELATION,
        ),
        field: Some("gamut_rec2020.out_of_gamut_basis_points".to_owned()),
        observed: Some(rec2020.out_of_gamut_basis_points.to_string()),
        allowed: Some(format!("< {QC_GAMUT_EXCEPTION_BASIS_POINTS}")),
        clip: None,
        effect: None,
    })
}

/// CC8 §6 item 4's named limitation, when the source withheld the diagnostic.
///
/// [`QaSeverity::Info`], because a withheld diagnostic is a statement about
/// CC8's constants rather than a fault in this grade, and because §6 keeps the
/// QC engine evidence-only. It exists at all so that a surface reading only
/// `exceptions` still learns why the skin section is empty — §12's "a named
/// limitation is a different support burden from a mysterious one".
fn withheld_skin_exception(report: &ColorQcReport) -> Option<ColorQcException> {
    let withheld = report.skin_withheld.as_ref()?;
    Some(ColorQcException {
        code: withheld.code.clone(),
        severity: QaSeverity::Info,
        message: withheld.reason.clone(),
        field: Some("skin".to_owned()),
        observed: Some(format!(
            "withheld on source profile {}",
            withheld.source_profile
        )),
        allowed: Some(
            "a CC1 SDR source profile, whose grade709 Rec.709 band constants the diagnostic was derived against"
                .to_owned(),
        ),
        clip: None,
        effect: None,
    })
}

/// Build every exception a finished measurement raises.
fn report_exceptions(report: &ColorQcReport) -> Vec<ColorQcException> {
    let mut exceptions = Vec::new();
    exceptions.extend(non_finite_exception(report));
    for (name, channel) in [
        ("red", &report.range.red),
        ("green", &report.range.green),
        ("blue", &report.range.blue),
    ] {
        for (direction, rate, count) in [
            ("over", channel.over_basis_points, channel.over_pixel_count),
            (
                "under",
                channel.under_basis_points,
                channel.under_pixel_count,
            ),
        ] {
            if rate >= QC_RANGE_EXCEPTION_BASIS_POINTS {
                exceptions.push(ColorQcException {
                    code: "delivery_range_excursion".to_owned(),
                    severity: QaSeverity::Warning,
                    message: format!(
                        "{count} visible pixels ({rate} basis points) are {direction} the delivery range on the {name} channel. Counting them is not a judgement: a blown highlight is frequently a deliberate creative choice."
                    ),
                    field: Some(format!("{name}.{direction}_basis_points")),
                    observed: Some(rate.to_string()),
                    allowed: Some(format!("< {QC_RANGE_EXCEPTION_BASIS_POINTS}")),
                    clip: None,
                    effect: None,
                });
            }
        }
    }
    if report.gamut.out_of_gamut_basis_points >= QC_GAMUT_EXCEPTION_BASIS_POINTS {
        exceptions.push(ColorQcException {
            code: "delivery_gamut_excursion".to_owned(),
            severity: QaSeverity::Warning,
            message: format!(
                "{} visible pixels ({} basis points) are outside the Rec.709 chromaticity triangle. This is the same pixel set as the under-range channels and must not be added to it.",
                report.gamut.out_of_gamut_pixel_count, report.gamut.out_of_gamut_basis_points
            ),
            field: Some("out_of_gamut_basis_points".to_owned()),
            observed: Some(report.gamut.out_of_gamut_basis_points.to_string()),
            allowed: Some(format!("< {QC_GAMUT_EXCEPTION_BASIS_POINTS}")),
            clip: None,
            effect: None,
        });
    }
    exceptions.extend(rec2020_gamut_exception(report));
    exceptions.extend(withheld_skin_exception(report));
    if let Some(tags) = &report.tags {
        for mismatch in &tags.mismatches {
            exceptions.push(ColorQcException {
                code: "delivery_tag_mismatch".to_owned(),
                severity: QaSeverity::Error,
                message: format!(
                    "Delivery tag {} is {}, expected {}. A mis-tagged file is never a creative choice: it will be misinterpreted by every downstream tool.",
                    mismatch.field, mismatch.observed, mismatch.allowed
                ),
                field: Some(mismatch.field.clone()),
                observed: Some(mismatch.observed.clone()),
                allowed: Some(mismatch.allowed.clone()),
                clip: None,
                effect: None,
            });
        }
        for entry in &tags.not_representable {
            exceptions.push(ColorQcException {
                code: "delivery_tag_not_representable".to_owned(),
                severity: QaSeverity::Info,
                message: format!(
                    "Delivery tag {} cannot be carried by this container, so it is reported rather than compared: {}",
                    entry.field, entry.reason
                ),
                field: Some(entry.field.clone()),
                observed: Some("not_representable".to_owned()),
                allowed: Some(entry.expected.clone()),
                clip: None,
                effect: None,
            });
        }
    }
    if let Some(skin) = &report.skin
        && skin.considered_pixel_count > 0
        && skin.in_band_basis_points < SKIN_BAND_EXCEPTION_BASIS_POINTS
    {
        exceptions.push(ColorQcException {
            code: "skin_region_outside_band".to_owned(),
            severity: QaSeverity::Info,
            message: format!(
                "{} basis points of the considered pixels fall inside the skin hue band. {}",
                skin.in_band_basis_points, SKIN_DIAGNOSTIC_BOUNDARY
            ),
            field: Some("in_band_basis_points".to_owned()),
            observed: Some(skin.in_band_basis_points.to_string()),
            allowed: Some(format!(">= {SKIN_BAND_EXCEPTION_BASIS_POINTS}")),
            clip: None,
            effect: None,
        });
    }
    exceptions
}

// ---------------------------------------------------------------------------
// Scalar CPU accumulators. No GPU reduction, CC2's non-goal, unchanged.
// ---------------------------------------------------------------------------

/// Per-channel delivery-clamp accumulator.
#[derive(Debug, Clone, Copy, Default)]
struct ChannelAccumulator {
    over: u64,
    under: u64,
    maximum_encoded: f64,
    minimum_encoded: f64,
    saw_over: bool,
    saw_under: bool,
}

impl ChannelAccumulator {
    fn add(&mut self, encoded: f64) {
        if encoded > 1.0 {
            self.over = self.over.saturating_add(1);
            if !self.saw_over || encoded > self.maximum_encoded {
                self.maximum_encoded = encoded;
                self.saw_over = true;
            }
        }
        if encoded < 0.0 {
            self.under = self.under.saturating_add(1);
            if !self.saw_under || encoded < self.minimum_encoded {
                self.minimum_encoded = encoded;
                self.saw_under = true;
            }
        }
    }

    fn report(self, visible: u64) -> ChannelRangeExcursion {
        ChannelRangeExcursion {
            over_pixel_count: self.over,
            under_pixel_count: self.under,
            over_basis_points: basis_points(self.over, visible),
            under_basis_points: basis_points(self.under, visible),
            maximum_over_excursion_millionths: if self.saw_over {
                millionths(self.maximum_encoded - 1.0)
            } else {
                0
            },
            minimum_under_excursion_millionths: if self.saw_under {
                millionths(self.minimum_encoded.min(0.0))
            } else {
                0
            },
        }
    }
}

/// One `Y'CbCr` plane's legality accumulator.
#[derive(Debug, Clone, Copy)]
struct PlaneAccumulator {
    low: f64,
    high: f64,
    below: u64,
    above: u64,
    minimum: f64,
    maximum: f64,
    seen: bool,
}

impl PlaneAccumulator {
    const fn new(low: f64, high: f64) -> Self {
        Self {
            low,
            high,
            below: 0,
            above: 0,
            minimum: 0.0,
            maximum: 0.0,
            seen: false,
        }
    }

    fn add(&mut self, code: f64) {
        if self.seen {
            self.minimum = self.minimum.min(code);
            self.maximum = self.maximum.max(code);
        } else {
            self.minimum = code;
            self.maximum = code;
            self.seen = true;
        }
        if code < self.low {
            self.below = self.below.saturating_add(1);
        }
        if code > self.high {
            self.above = self.above.saturating_add(1);
        }
    }

    fn report(self, visible: u64) -> PlaneLegalExcursion {
        // The `seen` flag, not the initial `0.0`: a plane that saw no sample
        // has no extreme to report, and `0` would be a number nothing
        // measured.
        let (minimum, maximum) = if self.seen {
            (hundredths(self.minimum), hundredths(self.maximum))
        } else {
            (
                PlaneLegalExcursion::UNSEEN_MINIMUM_CODE_HUNDREDTHS,
                PlaneLegalExcursion::UNSEEN_MAXIMUM_CODE_HUNDREDTHS,
            )
        };
        PlaneLegalExcursion {
            below_count: self.below,
            above_count: self.above,
            below_basis_points: basis_points(self.below, visible),
            above_basis_points: basis_points(self.above, visible),
            minimum_code_hundredths: minimum,
            maximum_code_hundredths: maximum,
        }
    }
}

/// One chromaticity triangle's gamut accumulator.
///
/// CC8 §6 item 2 makes the triangle "a property of the report", so it is a
/// property of the accumulator too: the Rec.709 instance holds CC6's numbers
/// unchanged and the Rec.2020 instance holds the same arithmetic over the
/// converted components with BT.2020's luma. One type, two instances, so the
/// two reports cannot drift into two implementations.
#[derive(Debug, Clone, Copy)]
struct GamutAccumulator {
    /// The three luma weights this triangle's desaturation metric uses.
    luma: [f64; 3],
    out_of_gamut: u64,
    below_black: u64,
    minimum_linear: f64,
    maximum_desaturation: f64,
    saw_desaturation: bool,
}

impl GamutAccumulator {
    const fn new(luma: [f64; 3]) -> Self {
        Self {
            luma,
            out_of_gamut: 0,
            below_black: 0,
            minimum_linear: 0.0,
            maximum_desaturation: 0.0,
            saw_desaturation: false,
        }
    }

    /// CC6's Rec.709 triangle: `0.2126 / 0.7152 / 0.0722`.
    const fn rec709() -> Self {
        Self::new([BT709_KR, 1.0 - BT709_KR - BT709_KB, BT709_KB])
    }

    /// CC8 §6 item 2's Rec.2020 triangle, with BT.2020's own luma read from the
    /// authority module.
    const fn rec2020() -> Self {
        Self::new([CC8_BT2020_KR, CC8_BT2020_KG, CC8_BT2020_KB])
    }

    fn add(&mut self, linear: [f64; 3]) {
        let [red, green, blue] = linear;
        let minimum = red.min(green).min(blue);
        self.minimum_linear = self.minimum_linear.min(minimum).min(0.0);
        if minimum < 0.0 {
            self.out_of_gamut = self.out_of_gamut.saturating_add(1);
            let luma = self.luma[0].mul_add(red, self.luma[1].mul_add(green, self.luma[2] * blue));
            if luma < 0.0 {
                self.below_black = self.below_black.saturating_add(1);
            } else {
                // `Y >= m` always and `m < 0`, so the denominator is positive.
                let desaturation = -minimum / (luma - minimum);
                if !self.saw_desaturation || desaturation > self.maximum_desaturation {
                    self.maximum_desaturation = desaturation;
                    self.saw_desaturation = true;
                }
            }
        }
    }

    fn report(&self, visible: u64, triangle: &str, definition: &str) -> ColorGamutReport {
        ColorGamutReport {
            out_of_gamut_pixel_count: self.out_of_gamut,
            out_of_gamut_basis_points: basis_points(self.out_of_gamut, visible),
            minimum_linear_millionths: millionths(self.minimum_linear.min(0.0)),
            maximum_desaturation_millionths: if self.saw_desaturation {
                millionths(self.maximum_desaturation)
            } else {
                0
            },
            below_black_pixel_count: self.below_black,
            triangle: triangle.to_owned(),
            definition: definition.to_owned(),
        }
    }
}

/// CC8 §6 item 3's `MaxCLL`/`MaxFALL` accumulator.
///
/// Two numbers over the population every other accumulator here uses: the
/// running maximum of `max(r, g, b)` and its `f64` sum. Nothing is clamped and
/// nothing is gated — see [`LIGHT_LEVEL_BOUNDARY`].
#[derive(Debug, Clone, Copy, Default)]
struct LightLevelAccumulator {
    maximum: f64,
    sum: f64,
    count: u64,
    seen: bool,
}

impl LightLevelAccumulator {
    fn add(&mut self, linear: [f64; 3]) {
        let light = linear[0].max(linear[1]).max(linear[2]);
        if !self.seen || light > self.maximum {
            self.maximum = light;
            self.seen = true;
        }
        self.sum += light;
        self.count = self.count.saturating_add(1);
    }

    fn report(&self, project_frame: i64) -> ColorLightLevelReport {
        let anchor = f64::from(CC8_REFERENCE_WHITE_NITS);
        #[allow(clippy::cast_precision_loss)]
        let mean = if self.count == 0 {
            0.0
        } else {
            self.sum / self.count as f64
        };
        ColorLightLevelReport {
            max_cll_nits_hundredths: if self.seen {
                hundredths(self.maximum * anchor)
            } else {
                0
            },
            max_fall_nits_hundredths: hundredths(mean * anchor),
            sampled_frame_count: 1,
            sampled_pixel_count: self.count,
            project_frame,
            reference_white_nits: CC8_REFERENCE_WHITE_NITS,
            gated: false,
            boundary: LIGHT_LEVEL_BOUNDARY.to_owned(),
        }
    }
}

/// Every scalar accumulator one measurement pass fills.
#[derive(Debug, Clone)]
struct RegionAccumulator {
    bits: u8,
    /// The lane whose delivery encode and `Y'CbCr` matrix every code below is
    /// taken through (CC8 §6 item 1).
    lane: DeliveryLane,
    region_pixel_count: u64,
    visible_pixel_count: u64,
    transparent_pixel_count: u64,
    non_finite_pixel_count: u64,
    channels: [ChannelAccumulator; 3],
    clamped: u64,
    gamut: GamutAccumulator,
    /// CC8 §6 item 2's second triangle, filled only on HDR content.
    gamut_rec2020: Option<GamutAccumulator>,
    light_level: LightLevelAccumulator,
    luma: PlaneAccumulator,
    cb: PlaneAccumulator,
    cr: PlaneAccumulator,
    skin_cos: f64,
    skin_sin: f64,
    skin_considered: u64,
    skin_excluded: u64,
    skin_in_band: u64,
    /// `f32`, not `f64`: this is the one per-pixel buffer the measurement
    /// keeps, and the median it feeds is reported in millionths, which `f32`
    /// resolves for every chroma the delivery box can hold (`|C| <= 0.5`, so
    /// one `f32` ulp is under `6e-8` and the millionth is decided by the
    /// sixth digit). Halving it halves the peak footprint of a UHD skin
    /// measurement.
    skin_chroma: Vec<f32>,
}

impl RegionAccumulator {
    /// `skin_capacity` is the region's pixel count when skin is requested and
    /// `0` otherwise, so the one growable buffer is allocated once instead of
    /// doubling its way to the raster size.
    fn new(bits: u8, lane: DeliveryLane, hdr_context: bool, skin_capacity: usize) -> Self {
        let scale = ycbcr_scale(bits);
        Self {
            bits,
            lane,
            region_pixel_count: 0,
            visible_pixel_count: 0,
            transparent_pixel_count: 0,
            non_finite_pixel_count: 0,
            channels: [ChannelAccumulator::default(); 3],
            clamped: 0,
            gamut: GamutAccumulator::rec709(),
            gamut_rec2020: hdr_context.then(GamutAccumulator::rec2020),
            light_level: LightLevelAccumulator::default(),
            luma: PlaneAccumulator::new(
                f64::from(YCBCR_LUMA_OFFSET) * scale,
                f64::from(YCBCR_LUMA_LEGAL_HIGH) * scale,
            ),
            cb: PlaneAccumulator::new(
                f64::from(YCBCR_LUMA_OFFSET) * scale,
                f64::from(YCBCR_CHROMA_LEGAL_HIGH) * scale,
            ),
            cr: PlaneAccumulator::new(
                f64::from(YCBCR_LUMA_OFFSET) * scale,
                f64::from(YCBCR_CHROMA_LEGAL_HIGH) * scale,
            ),
            skin_cos: 0.0,
            skin_sin: 0.0,
            skin_considered: 0,
            skin_excluded: 0,
            skin_in_band: 0,
            skin_chroma: Vec::with_capacity(skin_capacity),
        }
    }

    /// Accumulate one region pixel.
    ///
    /// A pixel is *visible* when `alpha > 0.0`, CC2's rule restated for `f32`.
    /// Alpha is never a weight.
    ///
    /// **Non-finite samples are counted, never classified.** A `NaN` fails
    /// every ordered comparison, so it would be silently reported as in range,
    /// in gamut, and inside the legal box; an infinity would saturate an
    /// extreme to `i64::MAX`. Either would be a fabricated measurement, so the
    /// guard below is the single place a non-finite pixel is recognised, and
    /// it feeds no channel, gamut, plane, or skin accumulator.
    fn add(&mut self, linear: [f32; 3], alpha: f32, measure_skin: bool) {
        self.region_pixel_count = self.region_pixel_count.saturating_add(1);
        // Written as the negation of the visibility test rather than
        // `alpha <= 0.0`, because a `NaN` alpha is not visible and `NaN <= 0.0`
        // is false: the two spellings differ exactly on `NaN`.
        #[allow(clippy::neg_cmp_op_on_partial_ord)]
        if !(alpha > 0.0) {
            self.transparent_pixel_count = self.transparent_pixel_count.saturating_add(1);
            return;
        }
        self.visible_pixel_count = self.visible_pixel_count.saturating_add(1);

        // `e` is computed in f32, matching the delivery clamp it predicts, and
        // through **this lane's** encode (CC8 §6 item 1): BT.709 on the SDR
        // lane, and Rec.2020 primaries with the HLG OETF on §5.1's HDR lane,
        // which is the arithmetic `quantize_delivery16` receives there.
        let encoded_f32 = encode_delivery_for_lane(self.lane, linear);
        if !linear
            .iter()
            .chain(&encoded_f32)
            .all(|value| value.is_finite())
        {
            self.non_finite_pixel_count = self.non_finite_pixel_count.saturating_add(1);
            return;
        }
        let encoded = [
            f64::from(encoded_f32[0]),
            f64::from(encoded_f32[1]),
            f64::from(encoded_f32[2]),
        ];
        let mut clamped = false;
        for (channel, value) in self.channels.iter_mut().zip(encoded) {
            channel.add(value);
            if value > 1.0 || value < 0.0 {
                clamped = true;
            }
        }
        if clamped {
            self.clamped = self.clamped.saturating_add(1);
        }

        let working = [
            f64::from(linear[0]),
            f64::from(linear[1]),
            f64::from(linear[2]),
        ];
        self.gamut.add(working);
        // CC8 §6 item 2: the second triangle measures the **Rec.2020** linear
        // components §2.3's matrix produces, because those are the components
        // the HDR delivery encode consumes. The matrix runs in `f32`, which is
        // the precision the delivery path applies it in, and widens once here.
        if let Some(rec2020) = self.gamut_rec2020.as_mut() {
            let converted = cc8_apply_matrix(CC8_BT709_TO_REC2020, linear);
            rec2020.add([
                f64::from(converted[0]),
                f64::from(converted[1]),
                f64::from(converted[2]),
            ]);
        }
        // CC8 §6 item 3: light level is a property of the working sample, not
        // of a lane, so it is measured on the same working triple on every
        // lane and scaled by §2.2's anchor when the report is built.
        self.light_level.add(working);

        // CC8 §6 item 1: the lane's own matrix, never BT.709's on a BT.2020
        // file. `encoded` is already this lane's `R'G'B'`.
        let codes = delivery_limited_ycbcr(self.lane, encoded, self.bits);
        self.luma.add(codes[0]);
        self.cb.add(codes[1]);
        self.cr.add(codes[2]);

        if measure_skin {
            self.add_skin(encoded);
        }
    }

    fn add_skin(&mut self, encoded: [f64; 3]) {
        let luma = BT709_KR.mul_add(
            encoded[0],
            (1.0 - BT709_KR - BT709_KB).mul_add(encoded[1], BT709_KB * encoded[2]),
        );
        let cb = (encoded[2] - luma) / BT709_CB_DENOMINATOR;
        let cr = (encoded[0] - luma) / BT709_CR_DENOMINATOR;
        let chroma = cb.hypot(cr);
        // §3.5's test is on the unrounded product, so a pixel one part in a
        // million below the floor is excluded rather than rounded into the
        // population.
        #[allow(clippy::cast_precision_loss)]
        let floor = SKIN_MIN_CHROMA_MILLIONTHS as f64;
        if chroma * 1_000_000.0 < floor {
            self.skin_excluded = self.skin_excluded.saturating_add(1);
            return;
        }
        let theta = cr.atan2(cb);
        self.skin_cos += theta.cos();
        self.skin_sin += theta.sin();
        self.skin_considered = self.skin_considered.saturating_add(1);
        // The circular sums stay `f64`; only the median's buffer is narrowed.
        #[allow(clippy::cast_possible_truncation)]
        self.skin_chroma.push(chroma as f32);
        let degrees = wrap_degrees(theta.to_degrees());
        let centidegrees = centidegrees(degrees);
        let distance = angular_distance_centidegrees(centidegrees, SKIN_BAND_CENTER_CENTIDEGREES);
        if distance <= SKIN_BAND_HALF_WIDTH_CENTIDEGREES {
            self.skin_in_band = self.skin_in_band.saturating_add(1);
        }
    }

    fn range_report(&self, visible: u64) -> ColorRangeReport {
        ColorRangeReport {
            red: self.channels[0].report(visible),
            green: self.channels[1].report(visible),
            blue: self.channels[2].report(visible),
            clamped_pixel_count: self.clamped,
            clamped_basis_points: basis_points(self.clamped, visible),
            predicted_ycbcr: YCbCrLegalReport {
                bit_depth: self.bits,
                luma: self.luma.report(visible),
                cb: self.cb.report(visible),
                cr: self.cr.report(visible),
                source: YCbCrLegalSource::Predicted,
            },
        }
    }

    fn gamut_report(&self, visible: u64) -> ColorGamutReport {
        self.gamut
            .report(visible, GAMUT_TRIANGLE_REC709, GAMUT_DEFINITION)
    }

    /// CC8 §6 item 2's second report, or `None` on SDR content.
    fn rec2020_gamut_report(&self, visible: u64) -> Option<ColorGamutReport> {
        self.gamut_rec2020.as_ref().map(|rec2020| {
            rec2020.report(visible, GAMUT_TRIANGLE_REC2020, GAMUT_DEFINITION_REC2020)
        })
    }

    fn light_level_report(&self, project_frame: i64) -> ColorLightLevelReport {
        self.light_level.report(project_frame)
    }

    fn skin_diagnostics(&self) -> SkinDiagnostics {
        let considered = self.skin_considered;
        let (mean_hue, concentration, spread) = if considered == 0 {
            (None, 0, SKIN_MAX_SPREAD_CENTIDEGREES)
        } else {
            // Computed from f64 sums of cos and sin, not from a running
            // angular average, so there is no order dependence and no wrap
            // discontinuity.
            let mean = wrap_degrees(self.skin_sin.atan2(self.skin_cos).to_degrees());
            #[allow(clippy::cast_precision_loss)]
            let count = considered as f64;
            // Clamped before the logarithm: the unclamped quotient can exceed
            // 1.0 in f64 for a uniform patch, which would make the spread NaN.
            let resultant = (self.skin_cos.hypot(self.skin_sin) / count).clamp(0.0, 1.0);
            let spread = if resultant <= 0.0 {
                SKIN_MAX_SPREAD_CENTIDEGREES
            } else {
                let radians = (-2.0 * resultant.ln()).max(0.0).sqrt();
                let value = scaled_round(radians.to_degrees(), 100.0);
                i32::try_from(value)
                    .unwrap_or(SKIN_MAX_SPREAD_CENTIDEGREES)
                    .min(SKIN_MAX_SPREAD_CENTIDEGREES)
            };
            (Some(centidegrees(mean)), millionths(resultant), spread)
        };
        let mut chroma = self.skin_chroma.clone();
        chroma.sort_by(f32::total_cmp);
        // Lower median: element `floor((n - 1) / 2)`, so it is deterministic
        // for even `n`. CC2's percentile convention, reused.
        let median = f64::from(
            chroma
                .get(chroma.len().saturating_sub(1) / 2)
                .copied()
                .unwrap_or(0.0),
        );
        SkinDiagnostics {
            region_pixel_count: self.region_pixel_count,
            considered_pixel_count: considered,
            excluded_achromatic_pixel_count: self.skin_excluded,
            mean_hue_centidegrees: mean_hue,
            hue_concentration_millionths: concentration,
            circular_spread_centidegrees: spread,
            median_chroma_millionths: millionths(median),
            in_band_basis_points: basis_points(self.skin_in_band, considered),
            band_center_centidegrees: SKIN_BAND_CENTER_CENTIDEGREES,
            band_half_width_centidegrees: SKIN_BAND_HALF_WIDTH_CENTIDEGREES,
            boundary: SKIN_DIAGNOSTIC_BOUNDARY.to_owned(),
        }
    }
}

/// `floor(value · 10_000 / count)`, `0` for an empty population and never a
/// division by zero. CC2's rule, reused rather than reimplemented.
#[must_use]
pub fn basis_points(value: u64, count: u64) -> u32 {
    if count == 0 {
        return 0;
    }
    u32::try_from(u128::from(value) * u128::from(SCOPE_BASIS_POINTS) / u128::from(count))
        .unwrap_or(u32::MAX)
}

/// `round(v · 1_000_000)`, half away from zero.
#[must_use]
pub fn millionths(value: f64) -> i64 {
    scaled_round(value, 1_000_000.0)
}

/// `round(v · 100)`, half away from zero.
#[must_use]
pub fn hundredths(value: f64) -> i64 {
    scaled_round(value, 100.0)
}

/// Scale and round half away from zero, saturating rather than wrapping.
///
/// `f64::round` rounds half away from zero, which is the rule CC6 §3.1 states.
#[allow(clippy::cast_possible_truncation)]
fn scaled_round(value: f64, scale: f64) -> i64 {
    let scaled = (value * scale).round();
    if scaled.is_nan() {
        return 0;
    }
    #[allow(clippy::cast_precision_loss)]
    if scaled >= i64::MAX as f64 {
        return i64::MAX;
    }
    #[allow(clippy::cast_precision_loss)]
    if scaled <= i64::MIN as f64 {
        return i64::MIN;
    }
    // The two guards above bracket `scaled` inside `i64`, and it is already an
    // integral `f64`, so this conversion is exact.
    scaled as i64
}

/// Wrap degrees into `[0, 360)`.
fn wrap_degrees(degrees: f64) -> f64 {
    let wrapped = degrees % 360.0;
    if wrapped < 0.0 {
        wrapped + 360.0
    } else {
        wrapped
    }
}

/// Round wrapped degrees to centidegrees in `0..=35_999`.
fn centidegrees(degrees: f64) -> i32 {
    let value = scaled_round(degrees, 100.0).rem_euclid(36_000);
    i32::try_from(value).unwrap_or(0)
}

/// The shorter arc between two centidegree angles, in centidegrees.
fn angular_distance_centidegrees(left: i32, right: i32) -> i32 {
    let difference = (left - right).rem_euclid(36_000);
    difference.min(36_000 - difference)
}
