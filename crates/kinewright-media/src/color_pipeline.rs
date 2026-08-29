//! Deterministic colour math for the CC1 managed SDR path.
//!
//! This module deliberately has no `FFmpeg` or GPU dependencies.  It is the
//! reference implementation for the source-code-value boundary, the linear
//! primary correction, and the final monitor encoding boundary.  The decoder
//! integration will supply RGB code values after its explicitly configured
//! matrix conversion; no display transfer or primary correction is delegated
//! to a backend here.

use std::fmt;
use std::sync::Arc;

use kinewright_core::{
    CC8_BT709_TO_REC2020, CC8_REC2020_TO_BT709, COLOR_CURVE_MIN_POINTS, COLOR_NODE_LIMIT_PER_LAYER,
    ColorBitDepth, ColorCurveChannel, ColorDescription, ColorMatrix, ColorNodeKind, ColorPrimaries,
    ColorRange, ColorTransfer, ColorWheelsParams, ColorWhitePoint, CurvePoints, DeliveryLane,
    Effect, LutAssetId, LutNodeParams, MatteParams, MatteQualifierParams, MatteWindowParams,
    MonitorPreview, ParamValue, ResolvedCurves, active_color_nodes, cc8_apply_matrix,
    cc8_hlg_decode_working_linear, cc8_hlg_encode_working_linear, cc8_pq_decode_working_linear,
    cc8_preview_tone_map_rgb, effect_descriptor, is_matte_parameter, managed_color_node_count,
};

use crate::lut::CubeLut;
use crate::lut_store::LutLibrary;

pub use kinewright_core::{
    ColorSourceError, ColorSourceProfile, ColorSourceProfileAssumption, classify_source,
    classify_source_with_assumption,
};

/// Compatibility alias for older media callers. Core remains the sole owner
/// of the profile policy and classifier implementation.
pub type SupportedSourceProfile = ColorSourceProfile;
/// Compatibility alias for older media callers. Core remains the sole owner
/// of the assumption policy.
pub type SourceProfileAssumption = ColorSourceProfileAssumption;

const BT709_LUMA_RED: f32 = 0.2126;
const BT709_LUMA_GREEN: f32 = 0.7152;
const BT709_LUMA_BLUE: f32 = 0.0722;
pub(crate) const BT709_RED_FROM_CR: f32 = 1.5748;
pub(crate) const BT709_GREEN_FROM_CB: f32 = -0.187_324;
pub(crate) const BT709_GREEN_FROM_CR: f32 = -0.468_124;
pub(crate) const BT709_BLUE_FROM_CB: f32 = 1.8556;

/// A metadata or control error that prevents the managed reference path from
/// making an implicit colour decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColorPipelineError {
    /// A Core-authoritative source profile rejected the decode boundary.
    SourceProfile(ColorSourceError),
    /// The source omitted one of the fields required for profile matching.
    UnknownPrimaries,
    /// The source primaries are not in either CC1 profile.
    UnsupportedPrimaries(ColorPrimaries),
    /// The source omitted its transfer characteristic.
    UnknownTransfer,
    /// The source transfer is not implemented by CC1.
    UnsupportedTransfer(ColorTransfer),
    /// The source omitted its matrix coefficients.
    UnknownMatrix,
    /// The source matrix is not implemented by CC1.
    UnsupportedMatrix(ColorMatrix),
    /// The source omitted its encoded range.
    UnknownRange,
    /// The source range cannot be used by a CC1 profile.
    UnsupportedRange(ColorRange),
    /// The source omitted its white point and no explicit assumption was
    /// supplied.
    UnknownWhitePoint,
    /// The source white point is not D65.
    UnsupportedWhitePoint(ColorWhitePoint),
    /// The source omitted its sample depth.
    UnknownBitDepth,
    /// The source depth is not an integer depth from 8 through 16 bits.
    UnsupportedBitDepth(ColorBitDepth),
    /// Every field is individually known, but the tuple does not match one of
    /// the two complete CC1 profiles.
    UnsupportedSourceCombination {
        /// The observed source primaries.
        primaries: ColorPrimaries,
        /// The observed source transfer.
        transfer: ColorTransfer,
        /// The observed source matrix.
        matrix: ColorMatrix,
        /// The observed source range.
        range: ColorRange,
    },
    /// A primary-control value is outside its inclusive descriptor range.
    InvalidPrimaryParameter {
        /// The offending control.
        parameter: PrimaryParameter,
        /// The observed value.
        value: i64,
        /// The descriptor minimum.
        min: i64,
        /// The descriptor maximum.
        max: i64,
    },
    /// An effect passed to the primary conversion seam is not the CC1 node.
    UnsupportedEffectName(String),
    /// An effect contains a parameter that is not in the Core descriptor.
    UnknownPrimaryParameter(String),
    /// An effect parameter is not an integer fixed-point value.
    NonIntegerPrimaryParameter {
        /// The serialized parameter name.
        name: String,
        /// The observed value.
        value: ParamValue,
    },
    /// The Core descriptor contains a parameter that this media reference
    /// layer does not know how to execute.
    UnsupportedPrimaryDescriptorParameter(String),
    /// The Core primary descriptor is unavailable or disagrees with the
    /// media-side canonical control table.
    PrimaryDescriptorMismatch(String),
    /// A CC8 §3.3 HDR interpretation stage was asked for a profile that is not
    /// one of CC8 §2.1's two HDR rows.
    ///
    /// A typed refusal rather than a silent identity: the HDR stages change
    /// what a pixel *means*, so running or skipping them on the wrong profile
    /// is a wrong picture, not a no-op.
    NotAnHdrSourceProfile(ColorSourceProfile),
    /// The target transfer is not the CC1 BT.709 monitor transfer.
    UnsupportedMonitorTransfer(ColorTransfer),
    /// The target transfer is not the CC1 BT.709 delivery transfer.
    UnsupportedDeliveryTransfer(ColorTransfer),
    /// A layer carries more managed colour nodes than CC3 §3.1 allows.  The
    /// limit is a typed error, never a silent truncation.
    TooManyColorNodes {
        /// The observed managed node count, active or bypassed.
        count: usize,
        /// The inclusive per-layer limit.
        limit: usize,
    },
    /// An active LUT node references an asset the verified library does not
    /// hold: `missing`, `changed`, `unreadable`, or unbound (CC4 §2.3, §4.4).
    /// The reference never silently renders a look-free frame.
    MissingLutAsset(LutAssetId),
    /// A LUT node's `input_encoding_token` is outside CC4 §3.4's three
    /// encodings.  Unreachable through the edit path, which clamps the stored
    /// integer to the descriptor bounds.
    UnsupportedInputEncoding(i64),
}

impl fmt::Display for ColorPipelineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SourceProfile(value) => write!(formatter, "source profile rejected: {value}"),
            Self::UnknownPrimaries => formatter.write_str("source colour primaries are unknown"),
            Self::UnsupportedPrimaries(value) => {
                write!(formatter, "unsupported source colour primaries: {value:?}")
            }
            Self::UnknownTransfer => formatter.write_str("source colour transfer is unknown"),
            Self::UnsupportedTransfer(value) => {
                write!(formatter, "unsupported source colour transfer: {value:?}")
            }
            Self::UnknownMatrix => formatter.write_str("source colour matrix is unknown"),
            Self::UnsupportedMatrix(value) => {
                write!(formatter, "unsupported source colour matrix: {value:?}")
            }
            Self::UnknownRange => formatter.write_str("source colour range is unknown"),
            Self::UnsupportedRange(value) => {
                write!(formatter, "unsupported source colour range: {value:?}")
            }
            Self::UnknownWhitePoint => formatter.write_str("source colour white point is unknown"),
            Self::UnsupportedWhitePoint(value) => {
                write!(
                    formatter,
                    "unsupported source colour white point: {value:?}"
                )
            }
            Self::UnknownBitDepth => formatter.write_str("source colour bit depth is unknown"),
            Self::UnsupportedBitDepth(value) => {
                write!(formatter, "unsupported source colour bit depth: {value:?}")
            }
            Self::UnsupportedSourceCombination {
                primaries,
                transfer,
                matrix,
                range,
            } => write!(
                formatter,
                "unsupported CC1 source combination: primaries={primaries:?}, transfer={transfer:?}, matrix={matrix:?}, range={range:?}"
            ),
            Self::InvalidPrimaryParameter {
                parameter,
                value,
                min,
                max,
            } => write!(
                formatter,
                "primary parameter {parameter}={value} is outside the inclusive range {min}..={max}"
            ),
            Self::UnsupportedEffectName(value) => write!(
                formatter,
                "unsupported primary effect name {value:?}; expected \"primary_correction\""
            ),
            Self::UnknownPrimaryParameter(value) => {
                write!(formatter, "unknown primary-correction parameter: {value}")
            }
            Self::NonIntegerPrimaryParameter { name, value } => write!(
                formatter,
                "primary-correction parameter {name} must be an integer, got {value:?}"
            ),
            Self::UnsupportedPrimaryDescriptorParameter(value) => write!(
                formatter,
                "Core primary descriptor parameter is not implemented by media: {value}"
            ),
            Self::PrimaryDescriptorMismatch(value) => {
                write!(formatter, "Core/media primary descriptor mismatch: {value}")
            }
            Self::NotAnHdrSourceProfile(value) => write!(
                formatter,
                "not_an_hdr_source_profile: observed {}, allowed {} or {} (CC8 §2.1)",
                value.id(),
                ColorSourceProfile::PqRec2020.id(),
                ColorSourceProfile::HlgRec2020.id(),
            ),
            Self::UnsupportedMonitorTransfer(value) => {
                write!(formatter, "unsupported monitor transfer: {value:?}")
            }
            Self::UnsupportedDeliveryTransfer(value) => {
                write!(formatter, "unsupported delivery transfer: {value:?}")
            }
            Self::TooManyColorNodes { count, limit } => write!(
                formatter,
                "too_many_color_nodes: {count} managed colour nodes exceed the per-layer limit of {limit}"
            ),
            Self::MissingLutAsset(id) => write!(
                formatter,
                "missing_lut_asset: no verified LUT asset {} is available for an active LUT node",
                id.0
            ),
            Self::UnsupportedInputEncoding(token) => write!(
                formatter,
                "unsupported_input_encoding: observed {token}, allowed 0 display709, 1 linear, 2 grade709"
            ),
        }
    }
}

impl std::error::Error for ColorPipelineError {}

/// Return the declared integer depth in bits after validating the CC1 range.
///
/// # Errors
///
/// Returns an error when the description omits or does not support an integer
/// depth in the inclusive 8--16 bit range.
pub fn integer_bit_depth(description: &ColorDescription) -> Result<u8, ColorPipelineError> {
    match &description.bit_depth {
        ColorBitDepth::Unknown => Err(ColorPipelineError::UnknownBitDepth),
        ColorBitDepth::Eight => Ok(8),
        ColorBitDepth::Ten => Ok(10),
        ColorBitDepth::Twelve => Ok(12),
        ColorBitDepth::Sixteen => Ok(16),
        ColorBitDepth::Integer(bits) if (8..=16).contains(bits) => u8::try_from(*bits)
            .map_err(|_| ColorPipelineError::UnsupportedBitDepth(description.bit_depth.clone())),
        value => Err(ColorPipelineError::UnsupportedBitDepth(value.clone())),
    }
}

/// Return the largest code that an integer source can carry after `FFmpeg`'s
/// promotion to the configured `RGBA64` boundary.
///
/// The managed swscale path promotes an `N`-bit integer code by shifting it
/// into the high bits of the 16-bit destination (`C_rgba64 = C_native <<
/// (16-N)`).  Consequently the promoted maximum is not generally `65535`:
/// it is `(2^N - 1) << (16-N)`.  Keeping this calculation next to the typed
/// source-depth validation prevents a caller from accidentally normalizing an
/// 8- or 10-bit source as though it had a native 16-bit white code.
///
/// # Errors
///
/// Returns an error when the description omits or does not support an integer
/// depth in the inclusive 8--16 bit range.
pub fn rgba64_promoted_max(description: &ColorDescription) -> Result<u32, ColorPipelineError> {
    let bits = integer_bit_depth(description)?;
    Ok(((1_u32 << bits) - 1) << (16 - bits))
}

/// Return the coded denominator used by the managed swscale-to-`RGBA64`
/// boundary for one source description.
///
/// Most integer paths use the high-bit promotion represented by
/// [`rgba64_promoted_max`].  There are two explicit swscale details that are
/// part of this boundary contract:
///
/// * the direct limited-range YUV-to-RGB path uses its 8-bit fixed-point
///   RGB scale even when the input YUV planes are 10 bits (or deeper), so its
///   nominal legal-white denominator is the 8-bit promoted maximum.  CC1 §3.1
///   states this for BT.709; CC8 §10 step 4 measured the **same** denominator on
///   the BT.2020 NCL limited lane, because it is one swscale fixed-point
///   YUV-to-RGB path whose matrix coefficients are selected by
///   `in_color_matrix` and whose destination scale is not.  Measured on the
///   pinned Linux build (`n8.0-23-gd1f31a829d`), a 10-bit `bt2020nc` limited
///   legal-white patch (`Y' = 940`, neutral chroma) lands on RGBA64 `65283`,
///   the identical figure CC1 §3.1 records for the BT.709 lane's legal white,
///   while legal black lands on `0`;
///   `cc8_bt2020_limited_boundary_uses_the_p8_denominator` re-measures it
///   through the production filter graph rather than restating the number.
///   Reusing the 10-bit promoted maximum `P_10 = 65472` there would make
///   nominal white read `0.9971` instead of `1.0000` — a wrong number, not an
///   approximate one; and
/// * the direct planar RGB path uses a true 16-bit destination scale for source
///   depths above 8 bits, reaching `65535` rather than a left-shifted source
///   maximum (its limited-range expansion is a separate working-frame step).
///
/// The source depth is still validated independently and is never inferred from
/// this effective denominator.  This function describes only the known,
/// explicitly configured `FFmpeg` boundary; it does not apply range expansion,
/// matrix conversion, transfer decoding, or clipping.
///
/// # Errors
///
/// Returns an error when the description omits or does not support an integer
/// depth in the inclusive 8--16 bit range.
pub fn rgba64_normalization_max(description: &ColorDescription) -> Result<u32, ColorPipelineError> {
    let bits = integer_bit_depth(description)?;
    // The BT.2020 NCL arm is CC8 §10 step 4's addition and is unreachable from
    // an SDR source: `classify_source_with_assumption` admits only `bt709`,
    // `rgb`, and `identity` matrices for `rec709_video` / `srgb_full`, so this
    // branch is entered exactly when the matrix column already says BT.2020,
    // which only a CC8 §2.1 HDR profile can carry. The BT.709 answer is
    // byte-unchanged.
    if matches!(
        description.matrix,
        ColorMatrix::Bt709 | ColorMatrix::Bt2020Ncl
    ) && matches!(description.range, ColorRange::Limited)
    {
        return Ok((u32::from(u8::MAX)) << 8);
    }
    if matches!(description.matrix, ColorMatrix::Rgb | ColorMatrix::Identity) && bits > 8 {
        return Ok(u32::from(u16::MAX));
    }
    rgba64_promoted_max(description)
}

/// Decode a BT.709 transfer-coded value to linear light.
///
/// The sign-preserving low branch allows range overshoot to survive until the
/// final monitor boundary.  For in-range non-negative values this is exactly
/// the BT.709 inverse OETF from the CC1 contract.
#[must_use]
pub fn decode_bt709(encoded: f32) -> f32 {
    if encoded < 0.081 {
        encoded / 4.5
    } else {
        ((encoded + 0.099) / 1.099).powf(1.0 / 0.45)
    }
}

/// Decode an sRGB transfer-coded value to linear light.
#[must_use]
pub fn decode_srgb(encoded: f32) -> f32 {
    if encoded <= 0.04045 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

/// Decode the CC1 zero-black-level BT.1886 transfer.
#[must_use]
pub fn decode_bt1886(encoded: f32) -> f32 {
    encoded.max(0.0).powf(2.4)
}

/// Decode one of the transfers accepted by a CC1 source profile.
///
/// **This is the SDR, per-channel decode and it stays that way.** CC8 §2.1's
/// two HDR profiles are decoded by [`decode_hdr_source_working_linear`]
/// instead, because HLG's stage chain is not separable per channel — §3.3's
/// OOTF stage takes a BT.2020 luma over the whole triple — and because
/// splitting an HDR decode into a per-channel call would let a caller take the
/// first stage and drop the rest, which is the mistake
/// `cc8_pq_decode_working_linear`'s fused shape exists to prevent. An HDR
/// transfer reaching here is still the typed refusal it is today.
///
/// # Errors
///
/// Returns an error when the transfer characteristic is unknown or outside
/// the CC1 source-profile contract.
pub fn decode_transfer(transfer: &ColorTransfer, encoded: f32) -> Result<f32, ColorPipelineError> {
    match transfer {
        ColorTransfer::Bt709 => Ok(decode_bt709(encoded)),
        ColorTransfer::Srgb => Ok(decode_srgb(encoded)),
        ColorTransfer::Bt1886 => Ok(decode_bt1886(encoded)),
        ColorTransfer::Unknown => Err(ColorPipelineError::UnknownTransfer),
        value => Err(ColorPipelineError::UnsupportedTransfer(value.clone())),
    }
}

/// CC8 §3.3's HDR source interpretation, from coded RGB to working linear.
///
/// This is the managed input path's HDR half: the named stages §3.3 marks `*`
/// on the source side, in §3.3's order, selected by the classified profile.
///
/// ```text
/// pq_rec2020:   source transfer decode (ST 2084 EOTF, absolute cd/m²)
///               -> reference-white normalization        (§2.2's anchor)
/// hlg_rec2020:  source transfer decode (ARIB STD-B67 inverse OETF)
///               -> HLG OOTF at §2.2's nominal peak and gamma (absolute cd/m²)
///               -> reference-white normalization        (§2.2's anchor)
/// ```
///
/// Every constant and every stage comes from `kinewright_core::cc8_hdr`; this
/// function contains no transfer arithmetic of its own, which is CC8 §2.2's
/// rule that the standards' constants "must not be delegated" restated from the
/// other side — they are also not *duplicated*.
///
/// **The result is Rec.2020-primaried linear light**, not working BT.709.
/// §3.3's next stage is "primaries conversion to working BT.709 D65 (\* now
/// non-identity)", and that stage is [`source_primaries_to_working_linear`].
/// [`decode_source_rgb`] composes the two in §3.3's order; a caller that takes
/// this function alone has the transfer half only, which is why the composition
/// has its own name.
///
/// **No clamp, at any stage.** CC1 §2.2 invariant 5, restated by CC8 §2.3.
/// Undershoot below black survives as negative working values and over-range
/// highlights survive at their magnitude: a PQ 10 000-nit code lands at
/// `≈ 49.3`, far inside `f16`'s range (§3.1).
///
/// # Errors
///
/// Returns [`ColorPipelineError::SourceProfile`] when the profile is not one of
/// CC8 §2.1's two, so a caller cannot reach the HDR stages with an SDR or
/// unclassified description.
pub fn decode_hdr_source_working_linear(
    profile: ColorSourceProfile,
    coded_rgb: [f32; 3],
) -> Result<[f32; 3], ColorPipelineError> {
    match profile {
        ColorSourceProfile::PqRec2020 => Ok(coded_rgb.map(cc8_pq_decode_working_linear)),
        ColorSourceProfile::HlgRec2020 => Ok(cc8_hlg_decode_working_linear(coded_rgb)),
        // A CC1 SDR profile has no HDR stages; asking for them is a caller
        // error, not a silent identity.
        profile => Err(ColorPipelineError::NotAnHdrSourceProfile(profile)),
    }
}

/// CC8 §2.3 / §3.3's **primaries conversion to working BT.709 D65** — the named
/// stage CC1 §3 reserved, non-identity for the first time.
///
/// ```text
/// pq_rec2020 | hlg_rec2020:  working = CC8_REC2020_TO_BT709 · linear
/// rec709_video | srgb_full:  working = linear          (the identity CC1 has)
/// ```
///
/// **The matrix is selected by source profile and by nothing else.** CC1 §3
/// wrote the stage as "an identity matrix... still a named stage so that CC2+
/// can add real conversion without changing the order", and CC8 §3.3 keeps that
/// promise from the other side: "the two `*` transfer stages and the primaries
/// conversion are selected by source profile, and on a Rec.709 source the
/// primaries stage stays the identity matrix it is now." So the SDR arm here
/// returns its argument — it does not multiply by an identity matrix, because
/// three multiplies and two adds per channel against pinned `f32` ones and
/// zeros is not guaranteed to be the identity in the last bit, and CC8's SDR
/// obligation (§7 item 1, §9.1 fixture 6) is byte-equality rather than
/// closeness.
///
/// **No clamp.** §2.3: "It is **not** a gamut map. Colours outside the Rec.709
/// triangle become negative BT.709 components and **must not be clamped** — CC1
/// §2.2 invariant 5 already forbids it, and CC8 restates it because negatives
/// here look like a bug and will tempt a future contributor to 'fix' them."
/// [`kinewright_core::cc8_apply_matrix`] is the unclamped application and
/// [`kinewright_core::CC8_BT709_TO_REC2020`] restores the excursion at delivery
/// (§10 step 6). §9.1 fixture 3 gates the round trip and asserts the negatives
/// are present, so the fixture cannot pass vacuously on in-gamut content.
///
/// The coefficients come from `kinewright_core::CC8_REC2020_TO_BT709`, which
/// §2.3 requires to be "derived from the two primary sets and D65, transcribed
/// to f32, not taken from a backend". The `cc8_derive_*` functions in that
/// module are the derivation the pinned rows are a transcription of, asserted
/// against them by `cc8_pinned_matrices_are_the_derivation_transcribed`; they
/// are cross-checks, not the production path, so nothing here derives a matrix
/// at run time.
#[must_use]
pub fn source_primaries_to_working_linear(
    profile: ColorSourceProfile,
    linear_rgb: [f32; 3],
) -> [f32; 3] {
    match profile {
        ColorSourceProfile::PqRec2020 | ColorSourceProfile::HlgRec2020 => {
            cc8_apply_matrix(CC8_REC2020_TO_BT709, linear_rgb)
        }
        ColorSourceProfile::Rec709Video | ColorSourceProfile::SrgbFull => linear_rgb,
    }
}

/// CC8 §3.3's HDR source chain end to end: coded RGB to **working BT.709 D65**
/// linear light.
///
/// [`decode_hdr_source_working_linear`] then [`source_primaries_to_working_linear`],
/// in §3.3's order, with no intermediate clamp between them and none inside
/// either. This exists as one name for the same reason
/// `cc8_pq_decode_working_linear` fuses ST 2084's two stages: a caller that took
/// the transfer decode alone would be holding Rec.2020-primaried values and
/// would composite them as BT.709, which CC8 §1 forbids.
///
/// # Errors
///
/// Returns [`ColorPipelineError::SourceProfile`] when the profile is not one of
/// CC8 §2.1's two, forwarded from [`decode_hdr_source_working_linear`].
pub fn hdr_source_to_working_bt709_linear(
    profile: ColorSourceProfile,
    coded_rgb: [f32; 3],
) -> Result<[f32; 3], ColorPipelineError> {
    Ok(source_primaries_to_working_linear(
        profile,
        decode_hdr_source_working_linear(profile, coded_rgb)?,
    ))
}

/// Encode a linear value with the CC1 BT.709 display transfer.
///
/// The extension for negative values is sign-preserving so that this function
/// itself does not discard over-range correction results.  [`encode_monitor_rgb8`]
/// performs the only display-range clamp.
#[must_use]
pub fn encode_bt709(linear: f32) -> f32 {
    if linear < 0.0 {
        -encode_bt709(-linear)
    } else if linear < 0.018 {
        4.5 * linear
    } else {
        1.099 * linear.powf(0.45) - 0.099
    }
}

/// Decode a `display709` grading value back to scene-linear light (CC4 §3.4).
///
/// The **exact sign-preserving inverse of [`encode_bt709`]**, with CC1's
/// rounded broadcast constants:
///
/// ```text
/// decode_display709(e) = sgn(e) * |e| / 4.5                       if |e| <  0.081
///                      = sgn(e) * ((|e| + 0.099) / 1.099)^(1/0.45) otherwise
/// ```
///
/// `sgn(0) = 0`, and [`f32::signum`] is deliberately not used because it
/// returns `±1` at zero and would break `decode_display709(0) = 0`, the
/// identity the CC4 §10.3.2 bit-exact gate needs.
///
/// This is a **node-internal grading parameterization**.  It must not replace
/// [`decode_bt709`], which stays the CC1 *source* decode: `decode_bt709` takes
/// its linear branch unconditionally for a negative argument, so it is not the
/// inverse of `encode_bt709` below `-0.018`.  Neither this function nor
/// `encode_bt709` may be used here as a monitoring or delivery transform; the
/// monitor transform still happens once, after compositing.
#[must_use]
pub fn decode_display709(e: f32) -> f32 {
    let sign = grade709_sign(e);
    let magnitude = e.abs();
    if magnitude < 0.081 {
        sign * magnitude / 4.5
    } else {
        sign * ((magnitude + 0.099) / 1.099).powf(1.0 / 0.45)
    }
}

/// Expand native normalized RGB code values using the declared integer depth
/// and range.  This is a reference helper for a native-code/`Y'CbCr` boundary;
/// it is not used by [`decode_source_rgb`], because the production swscale
/// boundary already returns full-range coded RGBA64 RGB values; the managed
/// working-frame decoder owns the effective boundary normalization.
///
/// # Errors
///
/// Returns an error when the depth or range is unknown or outside the CC1
/// contract.
#[allow(clippy::cast_precision_loss)]
pub fn expand_native_range(
    encoded_rgb: [f32; 3],
    bit_depth: &ColorBitDepth,
    range: &ColorRange,
) -> Result<[f32; 3], ColorPipelineError> {
    let bits = match bit_depth {
        ColorBitDepth::Unknown => return Err(ColorPipelineError::UnknownBitDepth),
        ColorBitDepth::Eight => 8,
        ColorBitDepth::Ten => 10,
        ColorBitDepth::Twelve => 12,
        ColorBitDepth::Sixteen => 16,
        ColorBitDepth::Integer(value) if (8..=16).contains(value) => u8::try_from(*value)
            .map_err(|_| ColorPipelineError::UnsupportedBitDepth(bit_depth.clone()))?,
        value => return Err(ColorPipelineError::UnsupportedBitDepth(value.clone())),
    };

    let expanded = match range {
        ColorRange::Unknown => return Err(ColorPipelineError::UnknownRange),
        ColorRange::Full => encoded_rgb,
        ColorRange::Limited => {
            let scale = (1_u32 << (bits - 8)) as f32;
            let max_code = ((1_u32 << bits) - 1) as f32;
            let luma_offset = 16.0 * scale;
            let luma_span = 219.0 * scale;
            encoded_rgb.map(|value| (value * max_code - luma_offset) / luma_span)
        }
        value @ ColorRange::Other(_) => {
            return Err(ColorPipelineError::UnsupportedRange(value.clone()));
        }
    };

    Ok(expanded)
}

/// Convert normalized BT.709 `Y'CbCr` code values to coded RGB.  This is the
/// explicit matrix reference used before transfer decoding; it is separate
/// from [`decode_source_rgb`], whose input is RGB after the swscale matrix
/// boundary.
///
/// # Errors
///
/// Returns an error when the depth or range is unknown or outside the CC1
/// contract.
#[allow(clippy::cast_precision_loss)]
pub fn decode_bt709_ycbcr(
    encoded_ycbcr: [f32; 3],
    bit_depth: &ColorBitDepth,
    range: &ColorRange,
) -> Result<[f32; 3], ColorPipelineError> {
    let bits = match bit_depth {
        ColorBitDepth::Unknown => return Err(ColorPipelineError::UnknownBitDepth),
        ColorBitDepth::Eight => 8,
        ColorBitDepth::Ten => 10,
        ColorBitDepth::Twelve => 12,
        ColorBitDepth::Sixteen => 16,
        ColorBitDepth::Integer(value) if (8..=16).contains(value) => u8::try_from(*value)
            .map_err(|_| ColorPipelineError::UnsupportedBitDepth(bit_depth.clone()))?,
        value => return Err(ColorPipelineError::UnsupportedBitDepth(value.clone())),
    };

    let (luma, cb, cr) = match range {
        ColorRange::Unknown => return Err(ColorPipelineError::UnknownRange),
        ColorRange::Full => (
            encoded_ycbcr[0],
            encoded_ycbcr[1] - 0.5,
            encoded_ycbcr[2] - 0.5,
        ),
        ColorRange::Limited => {
            let scale = (1_u32 << (bits - 8)) as f32;
            let max_code = ((1_u32 << bits) - 1) as f32;
            let y = (encoded_ycbcr[0] * max_code - 16.0 * scale) / (219.0 * scale);
            let cb = (encoded_ycbcr[1] * max_code - 128.0 * scale) / (224.0 * scale);
            let cr = (encoded_ycbcr[2] * max_code - 128.0 * scale) / (224.0 * scale);
            (y, cb, cr)
        }
        value @ ColorRange::Other(_) => {
            return Err(ColorPipelineError::UnsupportedRange(value.clone()));
        }
    };

    Ok([
        luma + BT709_RED_FROM_CR * cr,
        luma + BT709_GREEN_FROM_CB * cb + BT709_GREEN_FROM_CR * cr,
        luma + BT709_BLUE_FROM_CB * cb,
    ])
}

/// Decode normalized, coded RGB samples at the explicitly matched source
/// profile.  Matrix conversion is intentionally not performed here: the
/// bounded `FFmpeg`/swscale boundary must already have converted `Y'CbCr` to RGB.
///
/// The profile selects the interpretation, which is the whole point of CC1
/// §2.1's and CC8 §2.1's closed sets: a CC1 SDR profile takes the per-channel
/// [`decode_transfer`], and one of CC8 §2.1's two HDR profiles takes §3.3's
/// named HDR stages through [`hdr_source_to_working_bt709_linear`].
///
/// **Both arms return working BT.709 D65 linear light.** §3.3's primaries
/// conversion is the last source-side stage in both, and on a CC1 SDR profile
/// it is the identity CC1 §3 already has, so the SDR arm below is byte-for-byte
/// what it was before CC8 §10 step 4.
///
/// # Errors
///
/// Returns an error when the source metadata is not a complete supported
/// profile or its transfer cannot be decoded.
pub fn decode_source_rgb(
    description: &ColorDescription,
    encoded_rgb: [f32; 3],
    assumption: Option<SourceProfileAssumption>,
) -> Result<[f32; 3], ColorPipelineError> {
    let profile = classify_source_with_assumption(description, assumption)
        .map_err(ColorPipelineError::SourceProfile)?;
    if profile.is_hdr() {
        return hdr_source_to_working_bt709_linear(profile, encoded_rgb);
    }
    let mut decoded = [0.0; 3];
    for (index, value) in encoded_rgb.into_iter().enumerate() {
        decoded[index] = decode_transfer(&description.transfer, value)?;
    }
    Ok(decoded)
}

/// The canonical integer primary-control names and units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrimaryParameter {
    /// Exposure in 1/1000 stop.
    ExposureMilliStops,
    /// Warm/cool control in integer percentage points.
    TemperaturePercent,
    /// Green/magenta control in integer percentage points.
    TintPercent,
    /// Contrast scale around the pivot in integer percentage points.
    ContrastPercent,
    /// Contrast pivot in 1/10000 of display white.
    ContrastPivotBasisPoints,
    /// Low-end endpoint adjustment in integer percentage points.
    BlacksPercent,
    /// Lower-midtone adjustment in integer percentage points.
    ShadowsPercent,
    /// Upper-midtone adjustment in integer percentage points.
    HighlightsPercent,
    /// High-end endpoint adjustment in integer percentage points.
    WhitesPercent,
    /// Saturation scale around BT.709 luma in integer percentage points.
    SaturationPercent,
}

impl PrimaryParameter {
    /// All controls in stable descriptor order.
    pub const ALL: [Self; 10] = [
        Self::ExposureMilliStops,
        Self::TemperaturePercent,
        Self::TintPercent,
        Self::ContrastPercent,
        Self::ContrastPivotBasisPoints,
        Self::BlacksPercent,
        Self::ShadowsPercent,
        Self::HighlightsPercent,
        Self::WhitesPercent,
        Self::SaturationPercent,
    ];

    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "exposure_milli_stops" => Self::ExposureMilliStops,
            "temperature_percent" => Self::TemperaturePercent,
            "tint_percent" => Self::TintPercent,
            "contrast_percent" => Self::ContrastPercent,
            "contrast_pivot_basis_points" => Self::ContrastPivotBasisPoints,
            "blacks_percent" => Self::BlacksPercent,
            "shadows_percent" => Self::ShadowsPercent,
            "highlights_percent" => Self::HighlightsPercent,
            "whites_percent" => Self::WhitesPercent,
            "saturation_percent" => Self::SaturationPercent,
            _ => return None,
        })
    }

    /// The serialized parameter name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::ExposureMilliStops => "exposure_milli_stops",
            Self::TemperaturePercent => "temperature_percent",
            Self::TintPercent => "tint_percent",
            Self::ContrastPercent => "contrast_percent",
            Self::ContrastPivotBasisPoints => "contrast_pivot_basis_points",
            Self::BlacksPercent => "blacks_percent",
            Self::ShadowsPercent => "shadows_percent",
            Self::HighlightsPercent => "highlights_percent",
            Self::WhitesPercent => "whites_percent",
            Self::SaturationPercent => "saturation_percent",
        }
    }

    /// Inclusive descriptor minimum, maximum, and neutral value.
    #[must_use]
    pub const fn bounds(self) -> (i64, i64, i64) {
        match self {
            Self::ExposureMilliStops => (-5_000, 5_000, 0),
            Self::TemperaturePercent
            | Self::TintPercent
            | Self::ContrastPercent
            | Self::BlacksPercent
            | Self::ShadowsPercent
            | Self::HighlightsPercent
            | Self::WhitesPercent
            | Self::SaturationPercent => (-100, 100, 0),
            Self::ContrastPivotBasisPoints => (0, 10_000, 5_000),
        }
    }
}

impl fmt::Display for PrimaryParameter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// The CC1 serializable primary correction values.  Fields use the same
/// integer units as the effect descriptor contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrimaryCorrection {
    /// Exposure in 1/1000 stop.
    pub exposure_milli_stops: i32,
    /// Warm/cool control in integer percentage points.
    pub temperature_percent: i32,
    /// Green/magenta control in integer percentage points.
    pub tint_percent: i32,
    /// Contrast scale in integer percentage points.
    pub contrast_percent: i32,
    /// Contrast pivot in 1/10000 of display white.
    pub contrast_pivot_basis_points: i32,
    /// Low-end endpoint adjustment in integer percentage points.
    pub blacks_percent: i32,
    /// Lower-midtone adjustment in integer percentage points.
    pub shadows_percent: i32,
    /// Upper-midtone adjustment in integer percentage points.
    pub highlights_percent: i32,
    /// High-end endpoint adjustment in integer percentage points.
    pub whites_percent: i32,
    /// Saturation scale in integer percentage points.
    pub saturation_percent: i32,
}

impl Default for PrimaryCorrection {
    fn default() -> Self {
        Self {
            exposure_milli_stops: 0,
            temperature_percent: 0,
            tint_percent: 0,
            contrast_percent: 0,
            contrast_pivot_basis_points: 5_000,
            blacks_percent: 0,
            shadows_percent: 0,
            highlights_percent: 0,
            whites_percent: 0,
            saturation_percent: 0,
        }
    }
}

impl PrimaryCorrection {
    /// Read a Core `primary_correction` effect into the media reference
    /// controls.  Missing parameters resolve to the descriptor neutral.  The
    /// Core descriptor remains authoritative for names, integer types, and
    /// inclusive bounds; legacy `color_grade` is intentionally rejected.
    ///
    /// # Errors
    ///
    /// Returns an error when the effect name, descriptor, parameter types, or
    /// parameter values do not satisfy the Core primary-correction contract.
    pub fn from_effect(effect: &Effect) -> Result<Self, ColorPipelineError> {
        if effect.name != "primary_correction" {
            return Err(ColorPipelineError::UnsupportedEffectName(
                effect.name.clone(),
            ));
        }

        let Some(descriptor) = effect_descriptor("primary_correction") else {
            return Err(ColorPipelineError::PrimaryDescriptorMismatch(
                "primary_correction is not registered in Core".to_owned(),
            ));
        };

        let mut correction = Self::default();
        for parameter in PrimaryParameter::ALL {
            let Some(core_parameter) = descriptor.parameter(parameter.name()) else {
                return Err(ColorPipelineError::PrimaryDescriptorMismatch(format!(
                    "missing Core parameter {}",
                    parameter.name()
                )));
            };
            let (local_min, local_max, local_neutral) = parameter.bounds();
            if (
                core_parameter.min,
                core_parameter.max,
                core_parameter.neutral,
            ) != (local_min, local_max, local_neutral)
            {
                return Err(ColorPipelineError::PrimaryDescriptorMismatch(format!(
                    "{} is Core {}..={} neutral {}, media {}..={} neutral {}",
                    parameter.name(),
                    core_parameter.min,
                    core_parameter.max,
                    core_parameter.neutral,
                    local_min,
                    local_max,
                    local_neutral
                )));
            }
            correction.set_parameter(parameter, core_parameter.neutral)?;
        }

        for (name, value) in &effect.parameters {
            // CC5 §2.1: `primary_correction` is matte-capable, so its Core
            // descriptor carries the 47 `matte_*` parameters. They are
            // resolved by `MatteParams::from_effect` and are not CC1 controls,
            // so they must not be read as an unimplemented Core parameter.
            if is_matte_parameter(name) {
                continue;
            }
            let Some(core_parameter) = descriptor.parameter(name) else {
                return Err(ColorPipelineError::UnknownPrimaryParameter(name.clone()));
            };
            let Some(parameter) = PrimaryParameter::from_name(name) else {
                return Err(ColorPipelineError::UnsupportedPrimaryDescriptorParameter(
                    name.clone(),
                ));
            };
            let ParamValue::Integer(value) = value else {
                return Err(ColorPipelineError::NonIntegerPrimaryParameter {
                    name: name.clone(),
                    value: value.clone(),
                });
            };
            if *value < core_parameter.min || *value > core_parameter.max {
                return Err(ColorPipelineError::InvalidPrimaryParameter {
                    parameter,
                    value: *value,
                    min: core_parameter.min,
                    max: core_parameter.max,
                });
            }
            correction.set_parameter(parameter, *value)?;
        }

        Ok(correction)
    }

    /// Validate all integer controls against the CC1 inclusive descriptors.
    ///
    /// # Errors
    ///
    /// Returns an error when any control is outside its descriptor bounds.
    pub fn validate(&self) -> Result<(), ColorPipelineError> {
        for parameter in PrimaryParameter::ALL {
            let value = self.parameter(parameter);
            let (min, max, _) = parameter.bounds();
            if value < min || value > max {
                return Err(ColorPipelineError::InvalidPrimaryParameter {
                    parameter,
                    value,
                    min,
                    max,
                });
            }
        }
        Ok(())
    }

    /// Read one control using its serialized parameter identity.
    #[must_use]
    pub const fn parameter(&self, parameter: PrimaryParameter) -> i64 {
        match parameter {
            PrimaryParameter::ExposureMilliStops => self.exposure_milli_stops as i64,
            PrimaryParameter::TemperaturePercent => self.temperature_percent as i64,
            PrimaryParameter::TintPercent => self.tint_percent as i64,
            PrimaryParameter::ContrastPercent => self.contrast_percent as i64,
            PrimaryParameter::ContrastPivotBasisPoints => self.contrast_pivot_basis_points as i64,
            PrimaryParameter::BlacksPercent => self.blacks_percent as i64,
            PrimaryParameter::ShadowsPercent => self.shadows_percent as i64,
            PrimaryParameter::HighlightsPercent => self.highlights_percent as i64,
            PrimaryParameter::WhitesPercent => self.whites_percent as i64,
            PrimaryParameter::SaturationPercent => self.saturation_percent as i64,
        }
    }

    /// Set one control after checking its integer descriptor range.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` is outside the inclusive descriptor
    /// range for `parameter`.
    pub fn set_parameter(
        &mut self,
        parameter: PrimaryParameter,
        value: i64,
    ) -> Result<(), ColorPipelineError> {
        let (min, max, _) = parameter.bounds();
        if value < min || value > max {
            return Err(ColorPipelineError::InvalidPrimaryParameter {
                parameter,
                value,
                min,
                max,
            });
        }
        let value =
            i32::try_from(value).map_err(|_| ColorPipelineError::InvalidPrimaryParameter {
                parameter,
                value,
                min,
                max,
            })?;
        match parameter {
            PrimaryParameter::ExposureMilliStops => self.exposure_milli_stops = value,
            PrimaryParameter::TemperaturePercent => self.temperature_percent = value,
            PrimaryParameter::TintPercent => self.tint_percent = value,
            PrimaryParameter::ContrastPercent => self.contrast_percent = value,
            PrimaryParameter::ContrastPivotBasisPoints => {
                self.contrast_pivot_basis_points = value;
            }
            PrimaryParameter::BlacksPercent => self.blacks_percent = value,
            PrimaryParameter::ShadowsPercent => self.shadows_percent = value,
            PrimaryParameter::HighlightsPercent => self.highlights_percent = value,
            PrimaryParameter::WhitesPercent => self.whites_percent = value,
            PrimaryParameter::SaturationPercent => self.saturation_percent = value,
        }
        Ok(())
    }

    /// Apply the canonical white-balance, exposure, tonal-balance,
    /// contrast/pivot, and saturation sequence without an RGB clamp.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn apply(&self, linear_rgb: [f32; 3]) -> [f32; 3] {
        let temperature = self.temperature_percent as f32 / 100.0;
        let tint = self.tint_percent as f32 / 100.0;
        let red_gain = 1.0 + 0.1 * temperature;
        let green_gain = 1.0 - 0.1 * tint;
        let blue_gain = 1.0 - 0.1 * temperature;

        let exposure_gain = 2.0_f32.powf(self.exposure_milli_stops as f32 / 1_000.0);
        let mut corrected = [
            linear_rgb[0] * red_gain * exposure_gain,
            linear_rgb[1] * green_gain * exposure_gain,
            linear_rgb[2] * blue_gain * exposure_gain,
        ];

        for value in &mut corrected {
            let u = value.clamp(0.0, 1.0);
            let black_weight = 1.0 - smoothstep(0.0, 0.25, u);
            let shadow_weight = 1.0 - smoothstep(0.15, 0.50, u);
            let highlight_weight = smoothstep(0.50, 0.85, u);
            let white_weight = smoothstep(0.75, 1.0, u);

            *value += 0.25 * self.blacks_percent as f32 / 100.0 * black_weight;
            *value += 0.20 * self.shadows_percent as f32 / 100.0 * shadow_weight;
            *value += 0.20 * self.highlights_percent as f32 / 100.0 * highlight_weight;
            *value += 0.25 * self.whites_percent as f32 / 100.0 * white_weight;
        }

        let pivot = self.contrast_pivot_basis_points as f32 / 10_000.0;
        let contrast_scale = 1.0 + self.contrast_percent as f32 / 100.0;
        for value in &mut corrected {
            *value = pivot + (*value - pivot) * contrast_scale;
        }

        let luma = BT709_LUMA_RED * corrected[0]
            + BT709_LUMA_GREEN * corrected[1]
            + BT709_LUMA_BLUE * corrected[2];
        let saturation_scale = 1.0 + self.saturation_percent as f32 / 100.0;
        corrected.map(|value| luma + (value - luma) * saturation_scale)
    }

    /// Validate controls, then apply the canonical sequence.
    ///
    /// # Errors
    ///
    /// Returns an error when any serialized control is outside its descriptor
    /// bounds.
    pub fn apply_checked(&self, linear_rgb: [f32; 3]) -> Result<[f32; 3], ColorPipelineError> {
        self.validate()?;
        Ok(self.apply(linear_rgb))
    }
}

/// Apply serialized primary nodes in vector order without an RGB clamp between
/// nodes.  This is the CC1 compatibility spelling of [`apply_color_nodes_at`]
/// for a stack that contains only `primary_correction` nodes; it additionally
/// re-validates each node's integer controls.
///
/// **This is the matte-free primary path** (CC5 §3.4).  It keeps its CC1
/// signature because it takes no position: a [`PrimaryCorrection`] value is
/// the resolved CC1 control set, which carries no matte, so every node here is
/// applied at full strength everywhere.  A matte-scoped primary must go
/// through [`resolve_color_nodes_with`] and [`apply_color_nodes_at`].
///
/// # Errors
///
/// Returns an error when any primary node fails descriptor validation.
pub fn apply_primary_corrections(
    mut linear_rgb: [f32; 3],
    corrections: &[PrimaryCorrection],
) -> Result<[f32; 3], ColorPipelineError> {
    for correction in corrections {
        correction.validate()?;
        linear_rgb = apply_color_node(&ColorNode::Primary(*correction, None), linear_rgb);
    }
    Ok(linear_rgb)
}

// ---------------------------------------------------------------------------
// CC3 §2: the `grade709` working encoding.
// ---------------------------------------------------------------------------

/// CC3 §2.1 `ALPHA`: the precise BT.709 alpha, not the rounded 1.099.
const GRADE709_ALPHA: f32 = 1.099_296_8;
/// CC3 §2.1 `BETA`: the precise BT.709 linear-segment breakpoint.
///
/// Written with the contract's own digits so the constant can be checked
/// against CC3 §2.1 by eye; `0.018_053_969` and the shorter `0.018_053_97`
/// are the same `f32`.
#[allow(clippy::excessive_precision)]
const GRADE709_BETA: f32 = 0.018_053_969;
/// CC3 §2.1 `BETA_E` = `4.5 * BETA`, the encoded-side breakpoint.
const GRADE709_BETA_ENCODED: f32 = 0.081_242_86;
/// CC3 §2.1 `K` = `ALPHA - 1`.
const GRADE709_K: f32 = 0.099_296_8;
/// CC3 §2.1 `INV`: the f32 nearest of `1 / 0.45`.
const GRADE709_INVERSE_EXPONENT: f32 = 2.222_222_3;
/// The BT.709 OETF exponent.
const GRADE709_EXPONENT: f32 = 0.45;
/// The BT.709 near-black slope.
const GRADE709_SLOPE: f32 = 4.5;

/// Basis points per unit of the `grade709` range (CC3 §2.3).
const CURVE_BASIS_POINTS_PER_UNIT: f32 = 10_000.0;
/// Thousandths per unit of a `color_wheels` gain or gamma control (CC3 §4.1).
const WHEEL_THOUSANDTHS_PER_UNIT: f32 = 1_000.0;
/// Basis points per unit of a `color_wheels` lift control (CC3 §4.1).
const WHEEL_BASIS_POINTS_PER_UNIT: f32 = 10_000.0;

/// The CC3 sign function: `sgn(0) = 0`, and `sgn(NaN) = 0`.
///
/// [`f32::signum`] is deliberately not used; it returns `±1` at zero and would
/// break `E(0) = 0`, the identity that makes the CC3 §10.3 identity gate
/// bit-identical rather than tolerance-bounded.  WGSL `sign` already matches
/// this definition.
fn grade709_sign(value: f32) -> f32 {
    if value > 0.0 {
        1.0
    } else if value < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// Encode a scene-linear value into the CC3 `grade709` grading space (§2.1).
///
/// This is a grading parameterization, **not** a monitor or delivery
/// transform: [`encode_bt709`] keeps the rounded broadcast constants and
/// remains the normative monitor encoding.  `grade709_encode` is odd, strictly
/// increasing, and an exact analytic inverse of [`grade709_decode`], so the
/// pair round-trips over negatives and over-range highlights without a clamp.
#[must_use]
pub fn grade709_encode(x: f32) -> f32 {
    let sign = grade709_sign(x);
    let magnitude = x.abs();
    if magnitude < GRADE709_BETA {
        sign * GRADE709_SLOPE * magnitude
    } else {
        sign * (GRADE709_ALPHA * magnitude.powf(GRADE709_EXPONENT) - GRADE709_K)
    }
}

/// Decode a CC3 `grade709` value back to scene-linear light (§2.1).
///
/// The exact analytic inverse of [`grade709_encode`]; see that function for why
/// this pair is distinct from [`decode_bt709`].
#[must_use]
pub fn grade709_decode(e: f32) -> f32 {
    let sign = grade709_sign(e);
    let magnitude = e.abs();
    if magnitude < GRADE709_BETA_ENCODED {
        sign * magnitude / GRADE709_SLOPE
    } else {
        sign * ((magnitude + GRADE709_K) / GRADE709_ALPHA).powf(GRADE709_INVERSE_EXPONENT)
    }
}

// ---------------------------------------------------------------------------
// CC3 §2.2: `color_wheels`.
// ---------------------------------------------------------------------------

/// The CC3 `color_wheels` node resolved to ASC CDL slope/offset/power triples.
///
/// Master combines multiplicatively for gain and power and additively for
/// lift, exactly as CC3 §2.2 states.  The values are per channel in red,
/// green, blue order and are evaluated in `grade709`, never in scene-linear
/// light.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorWheels {
    slope: [f32; 3],
    offset: [f32; 3],
    power: [f32; 3],
}

impl ColorWheels {
    /// Resolve the twelve stored integers into SOP coefficients (CC3 §2.2).
    ///
    /// The caller passes parameters resolved from a *keyframe-evaluated*
    /// effect; this type performs no automation evaluation and no bypass or
    /// neutrality test.  [`resolve_color_nodes`] owns the CC3 §3.3 skip.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn from_params(params: &ColorWheelsParams) -> Self {
        let gain = params.gain_thousandths;
        let gamma = params.gamma_thousandths;
        let lift = params.lift_basis_points;
        let gain_master = gain.master as f32 / WHEEL_THOUSANDTHS_PER_UNIT;
        let gamma_master = gamma.master as f32 / WHEEL_THOUSANDTHS_PER_UNIT;
        let slope = |channel: i64| channel as f32 / WHEEL_THOUSANDTHS_PER_UNIT * gain_master;
        let power = |channel: i64| channel as f32 / WHEEL_THOUSANDTHS_PER_UNIT * gamma_master;
        let offset = |channel: i64| (channel + lift.master) as f32 / WHEEL_BASIS_POINTS_PER_UNIT;
        Self {
            slope: [slope(gain.red), slope(gain.green), slope(gain.blue)],
            offset: [offset(lift.red), offset(lift.green), offset(lift.blue)],
            power: [power(gamma.red), power(gamma.green), power(gamma.blue)],
        }
    }

    /// The resolved per-channel slope, in red, green, blue order.
    #[must_use]
    pub const fn slope(&self) -> [f32; 3] {
        self.slope
    }

    /// The resolved per-channel offset, in red, green, blue order.
    #[must_use]
    pub const fn offset(&self) -> [f32; 3] {
        self.offset
    }

    /// The resolved per-channel power, in red, green, blue order.
    #[must_use]
    pub const fn power(&self) -> [f32; 3] {
        self.power
    }

    /// Evaluate slope/offset/power per channel in `grade709` (CC3 §2.2).
    ///
    /// No stage clamps.  The CC3 deviation from ASC CDL v1.2 is deliberate:
    /// `y` is not clamped to `[0, 1]` before the power step, and the odd
    /// extension `sgn(y)·|y|^p` keeps recoverable undershoot and over-range
    /// highlights alive.  `power` is always strictly positive, so `|0|^p = 0`
    /// and no NaN is produced; `slope` may be exactly `0`, which makes the
    /// channel a legal constant.
    #[must_use]
    pub fn apply(&self, linear_rgb: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|channel| {
            let encoded = grade709_encode(linear_rgb[channel]);
            let shifted = encoded * self.slope[channel] + self.offset[channel];
            let powered = grade709_sign(shifted) * shifted.abs().powf(self.power[channel]);
            grade709_decode(powered)
        })
    }
}

// ---------------------------------------------------------------------------
// CC3 §2.3: `color_curves`.
// ---------------------------------------------------------------------------

/// One CC3 curve with its Fritsch--Carlson tangents already solved (§2.3).
///
/// This is the CPU *reference* implementation.  It is written from the CC3
/// §2.3 equations alone and deliberately shares no code with the compositor's
/// production host-side solve, so the parity fixtures compare two independent
/// implementations of the written contract (CC3 §3.2).
#[derive(Debug, Clone, PartialEq)]
pub struct ColorCurve {
    xs: Vec<f32>,
    ys: Vec<f32>,
    tangents: Vec<f32>,
}

impl ColorCurve {
    /// Convert resolved basis-point coordinates to `grade709` units and solve
    /// the monotone tangents (CC3 §2.3 steps 1--3).
    ///
    /// The caller passes a [`CurvePoints`] already resolved by Core, so the
    /// CC3 §3.4 truncation of a non-increasing automated prefix has happened
    /// before this point and the `x` sequence is strictly increasing.  A list
    /// shorter than two points is treated as the identity curve.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn from_points(points: &CurvePoints) -> Self {
        let count = points.points.len();
        if count < COLOR_CURVE_MIN_POINTS {
            return Self::from_points(&CurvePoints::identity());
        }
        let xs: Vec<f32> = points
            .points
            .iter()
            .map(|(x, _)| *x as f32 / CURVE_BASIS_POINTS_PER_UNIT)
            .collect();
        let ys: Vec<f32> = points
            .points
            .iter()
            .map(|(_, y)| *y as f32 / CURVE_BASIS_POINTS_PER_UNIT)
            .collect();

        // Step 1: secant slopes.  A non-positive span cannot occur for a
        // Core-resolved curve; guarding it keeps a hand-built point list from
        // producing an infinity instead of a flat segment.
        let deltas: Vec<f32> = xs
            .windows(2)
            .zip(ys.windows(2))
            .map(|(x, y)| {
                let span = x[1] - x[0];
                if span > 0.0 {
                    (y[1] - y[0]) / span
                } else {
                    0.0
                }
            })
            .collect();

        // Step 2: initial tangents.
        let mut tangents = Vec::with_capacity(count);
        tangents.push(deltas[0]);
        // The contract writes the interior tangent as a literal average.
        // `f32::midpoint` takes a different branch for huge magnitudes, and a
        // reference implementation must not carry a second rounding rule.
        #[allow(clippy::manual_midpoint)]
        tangents.extend(deltas.windows(2).map(|pair| (pair[0] + pair[1]) / 2.0));
        tangents.push(deltas[count - 2]);

        // Step 3: the Fritsch--Carlson limiter, forward and in place.  The
        // visitation order is normative: index `i + 1` is read after index `i`
        // has already been rewritten.
        for (index, delta) in deltas.iter().copied().enumerate() {
            if delta == 0.0 {
                tangents[index] = 0.0;
                tangents[index + 1] = 0.0;
                continue;
            }
            let a = tangents[index] / delta;
            let b = tangents[index + 1] / delta;
            if a < 0.0 {
                tangents[index] = 0.0;
            }
            if b < 0.0 {
                tangents[index + 1] = 0.0;
            }
            if a >= 0.0 && b >= 0.0 && a * a + b * b > 9.0 {
                let tau = 3.0 / (a * a + b * b).sqrt();
                tangents[index] = tau * a * delta;
                tangents[index + 1] = tau * b * delta;
            }
        }

        Self { xs, ys, tangents }
    }

    /// The point abscissae in `grade709` units.
    #[must_use]
    pub fn xs(&self) -> &[f32] {
        &self.xs
    }

    /// The point ordinates in `grade709` units.
    #[must_use]
    pub fn ys(&self) -> &[f32] {
        &self.ys
    }

    /// The solved, limited tangents.  Tangents are dimensionless (`dy/dx`).
    #[must_use]
    pub fn tangents(&self) -> &[f32] {
        &self.tangents
    }

    /// Evaluate the curve at one `grade709` coordinate (CC3 §2.3).
    ///
    /// Inside the point domain this is the cubic Hermite basis written out in
    /// the contract; outside it the curve extrapolates linearly with the
    /// limited end tangents, so over-range values stay alive instead of being
    /// silently clamped.
    #[must_use]
    pub fn evaluate(&self, x: f32) -> f32 {
        let last = self.xs.len() - 1;
        if x < self.xs[0] {
            return self.ys[0] + self.tangents[0] * (x - self.xs[0]);
        }
        if x >= self.xs[last] {
            return self.ys[last] + self.tangents[last] * (x - self.xs[last]);
        }
        let mut segment = 0;
        for index in 0..last {
            if x >= self.xs[index] && x < self.xs[index + 1] {
                segment = index;
            }
        }
        let x0 = self.xs[segment];
        let y0 = self.ys[segment];
        let m0 = self.tangents[segment];
        let x1 = self.xs[segment + 1];
        let y1 = self.ys[segment + 1];
        let m1 = self.tangents[segment + 1];
        let h = x1 - x0;
        let t = (x - x0) / h;
        let t2 = t * t;
        let t3 = t2 * t;
        (2.0 * t3 - 3.0 * t2 + 1.0) * y0
            + (t3 - 2.0 * t2 + t) * h * m0
            + (-2.0 * t3 + 3.0 * t2) * y1
            + (t3 - t2) * h * m1
    }
}

/// The four curves of one CC3 `color_curves` node (§2.3).
#[derive(Debug, Clone, PartialEq)]
pub struct ColorCurves {
    master: ColorCurve,
    red: ColorCurve,
    green: ColorCurve,
    blue: ColorCurve,
}

impl ColorCurves {
    /// Solve all four curves of a Core-resolved `color_curves` node.
    ///
    /// The caller passes curves resolved from a *keyframe-evaluated* effect;
    /// this type performs no automation evaluation and no bypass or neutrality
    /// test.  [`resolve_color_nodes`] owns the CC3 §3.3 skip.
    #[must_use]
    pub fn from_resolved(resolved: &ResolvedCurves) -> Self {
        Self {
            master: ColorCurve::from_points(&resolved.master),
            red: ColorCurve::from_points(&resolved.red),
            green: ColorCurve::from_points(&resolved.green),
            blue: ColorCurve::from_points(&resolved.blue),
        }
    }

    /// One solved curve.
    #[must_use]
    pub const fn curve(&self, curve: ColorCurveChannel) -> &ColorCurve {
        match curve {
            ColorCurveChannel::Master => &self.master,
            ColorCurveChannel::Red => &self.red,
            ColorCurveChannel::Green => &self.green,
            ColorCurveChannel::Blue => &self.blue,
        }
    }

    /// Evaluate the per-channel curves and then the master curve, in
    /// `grade709` (CC3 §2.3).
    ///
    /// The fourth curve is `master`, not `luma`: it is applied identically to
    /// each channel after that channel's own curve, which keeps the
    /// per-channel monotonicity guarantee that a `y'/y` chroma rescale would
    /// destroy.
    #[must_use]
    pub fn apply(&self, linear_rgb: [f32; 3]) -> [f32; 3] {
        let encoded = [
            self.red.evaluate(grade709_encode(linear_rgb[0])),
            self.green.evaluate(grade709_encode(linear_rgb[1])),
            self.blue.evaluate(grade709_encode(linear_rgb[2])),
        ];
        encoded.map(|value| grade709_decode(self.master.evaluate(value)))
    }
}

// ---------------------------------------------------------------------------
// CC4 §3.4--§3.5: LUT nodes.
//
// This evaluator is written from the CC4 §3.5 contract text alone.  It shares
// no code with `compositor.rs` or `compositor.wgsl`, because the CC3/CC4
// parity fixtures must compare two independent implementations of the written
// contract rather than one implementation with itself (CC4 §4.4).
// ---------------------------------------------------------------------------

/// The encoding a LUT node's lattice is authored in (CC4 §3.4).
///
/// The token is the stored `input_encoding_token` value; `display709` is the
/// default for both node kinds because published `.cube` looks are authored
/// against display-coded Rec.709 values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LutInputEncoding {
    /// Token `0`: [`encode_bt709`] / [`decode_display709`].
    Display709 = 0,
    /// Token `1`: the identity in both directions.
    Linear = 1,
    /// Token `2`: [`grade709_encode`] / [`grade709_decode`].
    Grade709 = 2,
}

impl LutInputEncoding {
    /// Every encoding, in token order.
    pub const ALL: [Self; 3] = [Self::Display709, Self::Linear, Self::Grade709];

    /// Classify a stored `input_encoding_token`.
    ///
    /// Returns `None` outside `0..=2`; CC4 §3.4 restricts LUT nodes to exactly
    /// these three encodings, and a log source cannot reach the node stack at
    /// all because CC1 rejects a log transfer at the decode boundary.
    #[must_use]
    pub const fn from_token(token: i64) -> Option<Self> {
        match token {
            0 => Some(Self::Display709),
            1 => Some(Self::Linear),
            2 => Some(Self::Grade709),
            _ => None,
        }
    }

    /// The stored token for this encoding.
    #[must_use]
    pub const fn token(self) -> i64 {
        match self {
            Self::Display709 => 0,
            Self::Linear => 1,
            Self::Grade709 => 2,
        }
    }

    /// The stable manifest/inspector token for this encoding.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Display709 => "display709",
            Self::Linear => "linear",
            Self::Grade709 => "grade709",
        }
    }

    /// `ENC`: scene-linear working value to the lattice's authored encoding.
    #[must_use]
    pub fn encode(self, x: f32) -> f32 {
        match self {
            Self::Display709 => encode_bt709(x),
            Self::Linear => x,
            Self::Grade709 => grade709_encode(x),
        }
    }

    /// `DEC`: the exact inverse of [`LutInputEncoding::encode`].
    #[must_use]
    pub fn decode(self, e: f32) -> f32 {
        match self {
            Self::Display709 => decode_display709(e),
            Self::Linear => e,
            Self::Grade709 => grade709_decode(e),
        }
    }
}

/// A verified 3D lattice evaluated with CC4 §3.5 tetrahedral interpolation.
///
/// The lattice edge and the domain always come from the **verified bytes**,
/// never from the document record (CC4 §2.1), so this type is built from a
/// parsed [`CubeLut`] and carries no independent metadata of its own.
#[derive(Debug, Clone, PartialEq)]
pub struct Lut3d {
    size: u32,
    domain_min: [f32; 3],
    domain_max: [f32; 3],
    samples: Arc<CubeLut>,
}

impl Lut3d {
    /// Wrap an already-shared parsed lattice without copying its samples.
    #[must_use]
    pub fn from_shared(samples: Arc<CubeLut>) -> Self {
        Self {
            size: samples.size,
            domain_min: samples.domain_min,
            domain_max: samples.domain_max,
            samples,
        }
    }

    /// Wrap a parsed lattice, cloning its samples into a fresh allocation.
    #[must_use]
    pub fn from_cube(cube: &CubeLut) -> Self {
        Self::from_shared(Arc::new(cube.clone()))
    }

    /// The lattice edge length `S`, from the verified bytes.
    #[must_use]
    pub const fn size(&self) -> u32 {
        self.size
    }

    /// `DOMAIN_MIN` per channel, from the verified bytes.
    #[must_use]
    pub const fn domain_min(&self) -> [f32; 3] {
        self.domain_min
    }

    /// `DOMAIN_MAX` per channel, from the verified bytes.
    #[must_use]
    pub const fn domain_max(&self) -> [f32; 3] {
        self.domain_max
    }

    /// The parsed lattice this evaluator reads.
    #[must_use]
    pub fn samples(&self) -> &Arc<CubeLut> {
        &self.samples
    }

    /// `V(i_r, i_g, i_b)`: one lattice point, red-fastest IRIDAS order
    /// (CC4 §3.5).
    ///
    /// Indices past the lattice edge are clamped rather than wrapped or
    /// panicking; §3.5's `i = min(floor(s), S - 2)` already keeps every
    /// interpolation index in range, so the clamp is defensive only.
    #[must_use]
    pub fn lattice(&self, i_r: u32, i_g: u32, i_b: u32) -> [f32; 3] {
        let last = self.size.saturating_sub(1);
        let (i_r, i_g, i_b) = (i_r.min(last), i_g.min(last), i_b.min(last));
        let size = self.size as usize;
        let index = ((i_b as usize * size) + i_g as usize) * size + i_r as usize;
        self.samples.sample(index).unwrap_or([0.0; 3])
    }

    /// Evaluate the lattice at an encoded triple (CC4 §3.5).
    ///
    /// The coordinate is clamped to the lattice domain, interpolated
    /// tetrahedrally with the contract's exact branch structure, and the
    /// out-of-domain excursion is then **added back** in the encoded domain:
    /// `z = y + (e - u)`.  A pure clamp would collapse every over-range
    /// highlight onto one lattice value, destroying the information CC1 §2.2
    /// invariant 5 exists to preserve.
    #[must_use]
    #[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
    #[allow(clippy::cast_possible_truncation, clippy::many_single_char_names)]
    pub fn lookup(&self, e: [f32; 3]) -> [f32; 3] {
        let last_index = self.size.saturating_sub(2);
        let span = f32::from(u16::try_from(self.size.saturating_sub(1)).unwrap_or(u16::MAX));

        let mut u = [0.0_f32; 3];
        let mut i = [0_u32; 3];
        let mut f = [0.0_f32; 3];
        for channel in 0..3 {
            let minimum = self.domain_min[channel];
            let maximum = self.domain_max[channel];
            let clamped = e[channel].clamp(minimum, maximum);
            let t = (clamped - minimum) / (maximum - minimum);
            let s = t * span;
            let index = (s.floor() as u32).min(last_index);
            u[channel] = clamped;
            i[channel] = index;
            f[channel] = s - f32::from(u16::try_from(index).unwrap_or(u16::MAX));
        }

        let (f_r, f_g, f_b) = (f[0], f[1], f[2]);
        let corner =
            |d_r: u32, d_g: u32, d_b: u32| self.lattice(i[0] + d_r, i[1] + d_g, i[2] + d_b);
        let c000 = corner(0, 0, 0);

        // CC4 §3.5, transcribed verbatim so tie handling is identical on the
        // CPU reference and the shader.  The conditions are strict `>`.
        let y = if f_r > f_g {
            if f_g > f_b {
                let (c100, c110, c111) = (corner(1, 0, 0), corner(1, 1, 0), corner(1, 1, 1));
                tetra(c000, f_r, c100, c000, f_g, c110, c100, f_b, c111, c110)
            } else if f_r > f_b {
                let (c100, c101, c111) = (corner(1, 0, 0), corner(1, 0, 1), corner(1, 1, 1));
                tetra(c000, f_r, c100, c000, f_g, c111, c101, f_b, c101, c100)
            } else {
                let (c001, c101, c111) = (corner(0, 0, 1), corner(1, 0, 1), corner(1, 1, 1));
                tetra(c000, f_r, c101, c001, f_g, c111, c101, f_b, c001, c000)
            }
        } else if f_b > f_g {
            let (c001, c011, c111) = (corner(0, 0, 1), corner(0, 1, 1), corner(1, 1, 1));
            tetra(c000, f_r, c111, c011, f_g, c011, c001, f_b, c001, c000)
        } else if f_b > f_r {
            let (c010, c011, c111) = (corner(0, 1, 0), corner(0, 1, 1), corner(1, 1, 1));
            tetra(c000, f_r, c111, c011, f_g, c010, c000, f_b, c011, c010)
        } else {
            let (c010, c110, c111) = (corner(0, 1, 0), corner(1, 1, 0), corner(1, 1, 1));
            tetra(c000, f_r, c110, c010, f_g, c010, c000, f_b, c111, c110)
        };

        [
            y[0] + (e[0] - u[0]),
            y[1] + (e[1] - u[1]),
            y[2] + (e[2] - u[2]),
        ]
    }
}

/// `base + f_r*(a1 - a0) + f_g*(b1 - b0) + f_b*(d1 - d0)`, the shared shape of
/// all six CC4 §3.5 tetrahedral formulas.
///
/// Keeping one term order for every branch is what makes a tie well defined to
/// the last bit: the six formulas agree analytically on the shared faces, and
/// the fixed association removes the f32 rounding difference that would
/// otherwise let two implementations disagree by a ULP at a tie.
#[allow(clippy::too_many_arguments)]
fn tetra(
    base: [f32; 3],
    f_r: f32,
    a1: [f32; 3],
    a0: [f32; 3],
    f_g: f32,
    b1: [f32; 3],
    b0: [f32; 3],
    f_b: f32,
    d1: [f32; 3],
    d0: [f32; 3],
) -> [f32; 3] {
    let mut out = [0.0_f32; 3];
    for channel in 0..3 {
        out[channel] = base[channel]
            + f_r * (a1[channel] - a0[channel])
            + f_g * (b1[channel] - b0[channel])
            + f_b * (d1[channel] - d0[channel]);
    }
    out
}

/// One resolved `technical_lut` or `creative_look` node (CC4 §3.5).
///
/// The two kinds are mathematically identical; they differ only in stage,
/// role, mix bounds, and UI placement, so one type serves both.
#[derive(Debug, Clone, PartialEq)]
pub struct LutNode {
    lut: Arc<Lut3d>,
    encoding: LutInputEncoding,
    mix: f32,
}

impl LutNode {
    /// Resolve a keyframe-evaluated LUT node against the verified library.
    ///
    /// The caller must have resolved keyframes already
    /// (`Effect::evaluated_at`), and must have skipped inactive nodes
    /// (CC4 §3.6): this constructor deliberately builds a node for whatever
    /// parameters it is handed so that an unbound reference is a typed error
    /// rather than a silently look-free frame.
    ///
    /// # Errors
    ///
    /// Returns [`ColorPipelineError::MissingLutAsset`] when the library holds
    /// no verified lattice for the referenced id — a `missing`, `changed`,
    /// `unreadable`, or unbound asset — and
    /// [`ColorPipelineError::UnsupportedInputEncoding`] for a token outside
    /// CC4 §3.4's three encodings.
    pub fn from_params(
        params: &LutNodeParams,
        library: &LutLibrary,
    ) -> Result<Self, ColorPipelineError> {
        let encoding = LutInputEncoding::from_token(params.input_encoding_token).ok_or(
            ColorPipelineError::UnsupportedInputEncoding(params.input_encoding_token),
        )?;
        let samples = library
            .get(params.lut_asset_id)
            .ok_or(ColorPipelineError::MissingLutAsset(params.lut_asset_id))?;
        Ok(Self {
            lut: Arc::new(Lut3d::from_shared(Arc::clone(samples))),
            encoding,
            mix: params.mix(),
        })
    }

    /// Build a node directly from a lattice, for the reference and fixtures.
    #[must_use]
    pub fn new(lut: Arc<Lut3d>, encoding: LutInputEncoding, mix: f32) -> Self {
        Self { lut, encoding, mix }
    }

    /// The lattice this node evaluates.
    #[must_use]
    pub fn lut(&self) -> &Arc<Lut3d> {
        &self.lut
    }

    /// The encoding the lattice is authored in.
    #[must_use]
    pub const fn encoding(&self) -> LutInputEncoding {
        self.encoding
    }

    /// The linear-light blend factor in `0.0..=1.0`.
    #[must_use]
    pub const fn mix(&self) -> f32 {
        self.mix
    }

    /// Apply the node to one scene-linear working triple (CC4 §3.5).
    ///
    /// `e = ENC(x)`, `z = lookup(e)`, `x' = DEC(z)`, and the mix happens in
    /// **linear light**: `out = x + (x' - x) * mix`.  Mixing in the encoded
    /// domain would need a fourth normative transfer decision and is undefined
    /// when the encoding is `linear`.
    #[must_use]
    pub fn apply(&self, x: [f32; 3]) -> [f32; 3] {
        let e = [
            self.encoding.encode(x[0]),
            self.encoding.encode(x[1]),
            self.encoding.encode(x[2]),
        ];
        let z = self.lut.lookup(e);
        let looked = [
            self.encoding.decode(z[0]),
            self.encoding.decode(z[1]),
            self.encoding.decode(z[2]),
        ];
        [
            x[0] + (looked[0] - x[0]) * self.mix,
            x[1] + (looked[1] - x[1]) * self.mix,
            x[2] + (looked[2] - x[2]) * self.mix,
        ]
    }
}

/// The CC4 §10.3.3 counter-implementation: eight-vertex trilinear
/// interpolation of the same lattice.
///
/// This is **test-only on purpose**.  CC4 §3.5 makes tetrahedral the normative
/// interpolation; shipping a trilinear entry point would invite a caller to
/// pick the wrong one.  The fixture uses it to prove the production evaluator
/// is not secretly trilinear.
#[cfg(test)]
impl Lut3d {
    #[must_use]
    #[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
    #[allow(clippy::cast_possible_truncation, clippy::many_single_char_names)]
    pub fn trilinear_lookup(&self, e: [f32; 3]) -> [f32; 3] {
        let last_index = self.size.saturating_sub(2);
        let span = f32::from(u16::try_from(self.size.saturating_sub(1)).unwrap_or(u16::MAX));
        let mut u = [0.0_f32; 3];
        let mut i = [0_u32; 3];
        let mut f = [0.0_f32; 3];
        for channel in 0..3 {
            let minimum = self.domain_min[channel];
            let maximum = self.domain_max[channel];
            let clamped = e[channel].clamp(minimum, maximum);
            let s = (clamped - minimum) / (maximum - minimum) * span;
            let index = (s.floor() as u32).min(last_index);
            u[channel] = clamped;
            i[channel] = index;
            f[channel] = s - f32::from(u16::try_from(index).unwrap_or(u16::MAX));
        }
        let mut y = [0.0_f32; 3];
        for d_b in 0..2_u32 {
            for d_g in 0..2_u32 {
                for d_r in 0..2_u32 {
                    let weight = [(d_r, f[0]), (d_g, f[1]), (d_b, f[2])]
                        .into_iter()
                        .map(
                            |(delta, fraction)| {
                                if delta == 1 { fraction } else { 1.0 - fraction }
                            },
                        )
                        .product::<f32>();
                    let corner = self.lattice(i[0] + d_r, i[1] + d_g, i[2] + d_b);
                    for channel in 0..3 {
                        y[channel] += weight * corner[channel];
                    }
                }
            }
        }
        [
            y[0] + (e[0] - u[0]),
            y[1] + (e[1] - u[1]),
            y[2] + (e[2] - u[2]),
        ]
    }
}

// ---------------------------------------------------------------------------
// CC5 §2.3--§2.5: the per-node matte.
//
// This is the **independent CPU reference**.  It is written from the CC5
// contract text alone and deliberately shares no code with the compositor's
// matte-block builder or with `compositor.wgsl` (CC5 §3.4, CC3's rule), so the
// parity fixtures compare two independent implementations of the same written
// equations.
// ---------------------------------------------------------------------------

/// Basis points per unit of a matte control (CC5 §2.2).
const MATTE_BASIS_POINTS_PER_UNIT: f64 = 10_000.0;
/// Hundredths of a degree per degree of a matte angle control (CC5 §2.2).
const MATTE_CENTIDEGREES_PER_DEGREE: f64 = 100.0;
/// The `matte_window{j}_shape_token` value that selects the ellipse (CC5 §2.2).
const MATTE_ELLIPSE_SHAPE_TOKEN: i64 = 2;
/// The hue half-width, in degrees, at or above which the hue leg is disabled
/// entirely — achromatic pixels included (CC5 §2.4).
const MATTE_HUE_LEG_DISABLED_DEGREES: f32 = 180.0;
/// One full turn in degrees, for the hue wrap in CC5 §2.4.
const MATTE_HUE_FULL_TURN_DEGREES: f32 = 360.0;
/// Degrees per hexagonal hue sector (CC5 §2.4).
const MATTE_HUE_SECTOR_DEGREES: f32 = 60.0;
/// Sectors in the hexagonal hue formula (CC5 §2.4).
const MATTE_HUE_SECTORS: f32 = 6.0;

/// The geometric shape of one matte window (CC5 §2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatteWindowShape {
    /// `shape_token == 1`: `D = max(|n.x|, |n.y|)`.
    Rect,
    /// `shape_token == 2`: `D = sqrt(n.x² + n.y²)`.
    Ellipse,
}

/// One geometric matte window resolved to render-ready floats (CC5 §2.3).
///
/// `cos_t` and `sin_t` are solved once here in `f64` and rounded once to
/// `f32`, exactly as the GPU matte block stores them, so both implementations
/// consume identical constants rather than each evaluating a transcendental.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MatteWindow {
    shape: MatteWindowShape,
    cx: f32,
    cy: f32,
    hw: f32,
    hh: f32,
    cos_t: f32,
    sin_t: f32,
    feather: f32,
    invert: bool,
}

impl MatteWindow {
    /// Resolve one window's stored integers (CC5 §2.2, §2.3).
    ///
    /// Basis points become uv fractions and centidegrees become a `(cos, sin)`
    /// pair; no clamping happens here, because Core already clamped the stored
    /// integers to their descriptor ranges.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    pub fn from_params(params: &MatteWindowParams) -> Self {
        let unit = |basis_points: i64| (basis_points as f64 / MATTE_BASIS_POINTS_PER_UNIT) as f32;
        let radians = (params.rotation_cd as f64 / MATTE_CENTIDEGREES_PER_DEGREE).to_radians();
        Self {
            shape: if params.shape_token == MATTE_ELLIPSE_SHAPE_TOKEN {
                MatteWindowShape::Ellipse
            } else {
                MatteWindowShape::Rect
            },
            cx: unit(params.center_x_bp),
            cy: unit(params.center_y_bp),
            hw: unit(params.half_width_bp),
            hh: unit(params.half_height_bp),
            cos_t: radians.cos() as f32,
            sin_t: radians.sin() as f32,
            feather: unit(params.feather_bp),
            invert: params.invert >= 1,
        }
    }

    /// The window's shape.
    #[must_use]
    pub const fn shape(&self) -> MatteWindowShape {
        self.shape
    }

    /// The window centre `(cx, cy)` in layer uv.
    #[must_use]
    pub const fn center(&self) -> [f32; 2] {
        [self.cx, self.cy]
    }

    /// The half-extents `(hw, hh)`: `hw` in fractions of the frame width, `hh`
    /// in fractions of the frame height (CC5 §2.3).
    #[must_use]
    pub const fn half_extents(&self) -> [f32; 2] {
        [self.hw, self.hh]
    }

    /// The host-solved `(cosT, sinT)` pair the shader also consumes.
    #[must_use]
    pub const fn rotation_cos_sin(&self) -> [f32; 2] {
        [self.cos_t, self.sin_t]
    }

    /// The symmetric feather band half-width, in units of the normalized
    /// field.
    #[must_use]
    pub const fn feather(&self) -> f32 {
        self.feather
    }

    /// Whether this window's own weight is complemented before the combine.
    #[must_use]
    pub const fn is_inverted(&self) -> bool {
        self.invert
    }

    /// The normalized distance field `D` at `uv` (CC5 §2.3).
    ///
    /// A degenerate half-extent yields [`f32::INFINITY`], which drives both
    /// feather branches to `w = 0`. This is the contract's defensive rule
    /// (CC5 §2.3) stated as a distance rather than as a separate early
    /// return; the shader guards the same case explicitly, and both agree on
    /// `w = 0` for every degenerate window.
    fn distance(&self, uv: [f32; 2], aspect: f32) -> f32 {
        if self.hw <= 0.0 || self.hh <= 0.0 {
            return f32::INFINITY;
        }
        let dx = (uv[0] - self.cx) * aspect;
        let dy = uv[1] - self.cy;
        let qx = dx * self.cos_t + dy * self.sin_t;
        let qy = -dx * self.sin_t + dy * self.cos_t;
        let nx = qx / (self.hw * aspect);
        let ny = qy / self.hh;
        match self.shape {
            MatteWindowShape::Rect => nx.abs().max(ny.abs()),
            // Written as the contract writes it.  `mul_add` is deliberately
            // not used: a fused multiply-add rounds once where the shader
            // rounds twice, which is exactly the kind of silent divergence the
            // independent-reference rule exists to catch.
            MatteWindowShape::Ellipse => (nx * nx + ny * ny).sqrt(),
        }
    }

    /// This window's weight at `uv`, including its own invert (CC5 §2.3).
    ///
    /// `aspect` is the **output raster aspect** `W / H`, supplied by the host;
    /// it is never sniffed from a texture size.  The `feather <= 0` hard
    /// branch is mandatory: `smoothstep` with equal edges is undefined.
    #[must_use]
    pub fn weight(&self, uv: [f32; 2], aspect: f32) -> f32 {
        let distance = self.distance(uv, aspect);
        let weight = if self.feather <= 0.0 {
            if distance <= 1.0 { 1.0 } else { 0.0 }
        } else {
            1.0 - smoothstep(1.0 - self.feather, 1.0 + self.feather, distance)
        };
        if self.invert { 1.0 - weight } else { weight }
    }
}

/// The three selectors the CC5 §2.4 qualifier judges, in the `grade709`
/// encoding, on a local clamped copy.
///
/// `hue` is `None` exactly when `C == 0`, which is the contract's "hue
/// undefined" state — modelled as an absent value rather than a sentinel angle
/// so the achromatic rule cannot be reached by accident.
#[derive(Debug, Clone, Copy, PartialEq)]
struct MatteSelectors {
    chroma: f32,
    saturation: f32,
    luma: f32,
    hue: Option<f32>,
}

/// The CC5 §2.4 HSL qualifier resolved to render-ready floats.
///
/// Angles are degrees and the band edges are unit fractions, matching the GPU
/// matte block word for word.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MatteQualifier {
    hue_center: f32,
    hue_width: f32,
    hue_softness: f32,
    sat_low: f32,
    sat_high: f32,
    sat_softness: f32,
    luma_low: f32,
    luma_high: f32,
    luma_softness: f32,
}

impl MatteQualifier {
    /// Resolve the qualifier's stored integers (CC5 §2.2, §2.4).
    ///
    /// This conversion does **not** consult `enabled`; whether the leg
    /// participates at all is [`Matte::from_params`]'s question.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    pub fn from_params(params: &MatteQualifierParams) -> Self {
        let degrees =
            |centidegrees: i64| (centidegrees as f64 / MATTE_CENTIDEGREES_PER_DEGREE) as f32;
        let unit = |basis_points: i64| (basis_points as f64 / MATTE_BASIS_POINTS_PER_UNIT) as f32;
        Self {
            hue_center: degrees(params.hue_center_cd),
            hue_width: degrees(params.hue_width_cd),
            hue_softness: degrees(params.hue_softness_cd),
            sat_low: unit(params.sat_low_bp),
            sat_high: unit(params.sat_high_bp),
            sat_softness: unit(params.sat_softness_bp),
            luma_low: unit(params.luma_low_bp),
            luma_high: unit(params.luma_high_bp),
            luma_softness: unit(params.luma_softness_bp),
        }
    }

    /// The hue band centre, half-width, and shoulder, in degrees.
    #[must_use]
    pub const fn hue_degrees(&self) -> [f32; 3] {
        [self.hue_center, self.hue_width, self.hue_softness]
    }

    /// The saturation band low edge, high edge, and shoulder, in `0..=1`.
    #[must_use]
    pub const fn saturation_band(&self) -> [f32; 3] {
        [self.sat_low, self.sat_high, self.sat_softness]
    }

    /// The luma band low edge, high edge, and shoulder, in `0..=1`.
    #[must_use]
    pub const fn luma_band(&self) -> [f32; 3] {
        [self.luma_low, self.luma_high, self.luma_softness]
    }

    /// The CC5 §2.4 selectors of the value **entering** the node.
    ///
    /// The `grade709` encoding and the `[0, 1]` clamp are local to this
    /// function: CC1 §2.2.5 is preserved because the clamped copy never
    /// escapes and the pipeline value is untouched.
    ///
    /// The exact `==` tests against `maximum` are normative, not sloppy: the
    /// hexagonal formula selects a branch by *which channel is the maximum*,
    /// and the written branch order is what resolves a tie.  A tolerance here
    /// would let the reference and the shader disagree at the sector seams.
    #[allow(clippy::float_cmp)]
    fn selectors(linear_rgb_in: [f32; 3]) -> MatteSelectors {
        let clamped = linear_rgb_in.map(|value| grade709_encode(value).clamp(0.0, 1.0));
        let [red, green, blue] = clamped;
        let maximum = red.max(green).max(blue);
        let minimum = red.min(green).min(blue);
        let chroma = maximum - minimum;
        let saturation = if maximum <= 0.0 {
            0.0
        } else {
            chroma / maximum
        };
        let luma = BT709_LUMA_RED * red + BT709_LUMA_GREEN * green + BT709_LUMA_BLUE * blue;
        // The written branch order is normative: a tie is resolved by the
        // first matching channel, red before green before blue, so this
        // reference and the shader agree on the seam.
        let hue = if chroma == 0.0 {
            None
        } else if maximum == red {
            Some(MATTE_HUE_SECTOR_DEGREES * ((green - blue) / chroma).rem_euclid(MATTE_HUE_SECTORS))
        } else if maximum == green {
            Some(MATTE_HUE_SECTOR_DEGREES * ((blue - red) / chroma + 2.0))
        } else {
            Some(MATTE_HUE_SECTOR_DEGREES * ((red - green) / chroma + 4.0))
        };
        MatteSelectors {
            chroma,
            saturation,
            luma,
            hue,
        }
    }

    /// One CC5 §2.4 band: `min` of the two shoulders, never a product, so the
    /// interior is exactly `1` and the shoulders cannot interact.
    ///
    /// A degenerate band (`lo > hi`) evaluates to `0` — no clamp, no reorder,
    /// no error (CC5 §2.6).
    fn band(value: f32, low: f32, high: f32, softness: f32) -> f32 {
        if low > high {
            return 0.0;
        }
        if softness <= 0.0 {
            return if low <= value && value <= high {
                1.0
            } else {
                0.0
            };
        }
        smoothstep(low - softness, low, value).min(1.0 - smoothstep(high, high + softness, value))
    }

    /// The hue leg (CC5 §2.4), including the normative achromatic rule.
    fn hue_weight(&self, hue: Option<f32>) -> f32 {
        if self.hue_width >= MATTE_HUE_LEG_DISABLED_DEGREES {
            return 1.0;
        }
        let Some(hue) = hue else {
            return 0.0;
        };
        let separation = (hue - self.hue_center).abs();
        let separation = separation.min(MATTE_HUE_FULL_TURN_DEGREES - separation);
        if self.hue_softness <= 0.0 {
            if separation <= self.hue_width {
                1.0
            } else {
                0.0
            }
        } else {
            1.0 - smoothstep(
                self.hue_width,
                self.hue_width + self.hue_softness,
                separation,
            )
        }
    }

    /// The qualifier weight of the value entering the node (CC5 §2.4).
    #[must_use]
    pub fn weight(&self, linear_rgb_in: [f32; 3]) -> f32 {
        let selectors = Self::selectors(linear_rgb_in);
        self.hue_weight(selectors.hue)
            * Self::band(
                selectors.saturation,
                self.sat_low,
                self.sat_high,
                self.sat_softness,
            )
            * Self::band(
                selectors.luma,
                self.luma_low,
                self.luma_high,
                self.luma_softness,
            )
    }
}

/// One node's resolved matte: the windows, the qualifier, the invert, and the
/// mix (CC5 §2.5).
///
/// A [`Matte`] exists only when Core's integer test says the matte is active,
/// so a node that carries one **always** takes the blend path and a node that
/// does not **never** does — which is what makes "a pre-CC5 project renders
/// bit-identically" provable rather than approximate (CC5 §2.5, rule 4).
#[derive(Debug, Clone, PartialEq)]
pub struct Matte {
    windows: Vec<MatteWindow>,
    combine_union: bool,
    qualifier: Option<MatteQualifier>,
    invert: bool,
    mix: f32,
}

impl Matte {
    /// Resolve a keyframe-evaluated node's matte parameters (CC5 §2.6).
    ///
    /// Returns `None` for an inactive matte — the master switch off, or an
    /// enabled matte that selects everything at full strength — because such a
    /// node must be byte-identical to its CC4 self.
    #[must_use]
    pub fn from_params(params: &MatteParams) -> Option<Self> {
        if params.is_inactive_matte() {
            return None;
        }
        Some(Self {
            windows: params
                .active_windows()
                .map(MatteWindow::from_params)
                .collect(),
            combine_union: !params.intersects(),
            qualifier: params
                .qualifier
                .is_enabled()
                .then(|| MatteQualifier::from_params(&params.qualifier)),
            invert: params.is_inverted(),
            mix: params.mix(),
        })
    }

    /// The active windows, in index order.
    #[must_use]
    pub fn windows(&self) -> &[MatteWindow] {
        &self.windows
    }

    /// Whether the windows combine by union (`max`) rather than intersection
    /// (`min`).
    #[must_use]
    pub const fn is_union(&self) -> bool {
        self.combine_union
    }

    /// The HSL qualifier leg, when enabled.
    #[must_use]
    pub const fn qualifier(&self) -> Option<&MatteQualifier> {
        self.qualifier.as_ref()
    }

    /// Whether the combined coverage is complemented before the mix.
    #[must_use]
    pub const fn is_inverted(&self) -> bool {
        self.invert
    }

    /// The final coverage scale, `0.0..=1.0`.
    #[must_use]
    pub const fn mix(&self) -> f32 {
        self.mix
    }

    /// The combined geometric weight (CC5 §2.3): `1` with no window, `max`
    /// under union, `min` under intersection.
    fn window_weight(&self, uv: [f32; 2], aspect: f32) -> f32 {
        let mut combined: Option<f32> = None;
        for window in &self.windows {
            let weight = window.weight(uv, aspect);
            combined = Some(match combined {
                None => weight,
                Some(previous) if self.combine_union => previous.max(weight),
                Some(previous) => previous.min(weight),
            });
        }
        combined.unwrap_or(1.0)
    }

    /// The coverage `m ∈ [0, 1]` at one pixel (CC5 §2.5).
    ///
    /// `rgb_in` is the value **entering** this node, not the original frame:
    /// each managed node stays a pure function of its own input.
    #[must_use]
    pub fn coverage(&self, uv: [f32; 2], aspect: f32, rgb_in: [f32; 3]) -> f32 {
        let raw = self.window_weight(uv, aspect)
            * self
                .qualifier
                .map_or(1.0, |qualifier| qualifier.weight(rgb_in));
        let inverted = if self.invert { 1.0 - raw } else { raw };
        inverted * self.mix
    }
}

// ---------------------------------------------------------------------------
// CC3 §3.1: the ordered node stack.
// ---------------------------------------------------------------------------

/// One resolved node of the CC1/CC3 ordered colour-correction stack (§3.1).
///
/// `technical_lut`, `primary_correction`, `color_wheels`, `color_curves`, and
/// `creative_look` form **one** ordered stack executed in `clip.effects`
/// vector order.  Within a stage there is no fixed inter-kind precedence, and
/// the reference must not flatten, reorder, or merge nodes.
///
/// The curve variant is much larger than the others because it owns four
/// solved point lists.  Boxing it is deliberately not done: a layer holds at
/// most sixteen nodes, the stack is resolved once per frame rather than per
/// pixel, and an indirection would sit inside the per-pixel dispatch.
///
/// CC4 adds the two LUT kinds at either end of the stage order.  The variant
/// order here is [`kinewright_core::MANAGED_COLOR_NODE_NAMES`] stage order,
/// which is a readability choice only: the executed order is always the
/// `clip.effects` vector order.
/// CC5 §3.4 attaches the optional matte to the four matte-capable variants as
/// a second payload field rather than wrapping the enum in a struct: the enum
/// stays the node, `technical_lut` keeps a shape in which a matte is
/// *unrepresentable*, and no existing payload type has to give up `Copy`.
/// `None` is the CC4 shape and takes the CC4 path, byte for byte.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ColorNode {
    /// The CC4 technical input transform (stage 0).  It carries no matte:
    /// a partially applied source normalization is not a meaningful state
    /// (CC5 §2.1).
    TechnicalLut(LutNode),
    /// The CC1 managed SDR primary correction, with its optional CC5 matte.
    Primary(PrimaryCorrection, Option<Matte>),
    /// The CC3 ASC CDL slope/offset/power wheels, with its optional CC5 matte.
    Wheels(ColorWheels, Option<Matte>),
    /// The CC3 master/red/green/blue curves, with its optional CC5 matte.
    Curves(ColorCurves, Option<Matte>),
    /// The CC4 creative look (stage 2), with its optional CC5 matte.
    CreativeLook(LutNode, Option<Matte>),
}

impl ColorNode {
    /// The Core node-kind tag for this resolved node.
    #[must_use]
    pub const fn kind(&self) -> ColorNodeKind {
        match self {
            Self::TechnicalLut(_) => ColorNodeKind::TechnicalLut,
            Self::Primary(..) => ColorNodeKind::Primary,
            Self::Wheels(..) => ColorNodeKind::Wheels,
            Self::Curves(..) => ColorNodeKind::Curves,
            Self::CreativeLook(..) => ColorNodeKind::CreativeLook,
        }
    }

    /// The resolved LUT node, for the two LUT kinds.
    #[must_use]
    pub const fn lut_node(&self) -> Option<&LutNode> {
        match self {
            Self::TechnicalLut(node) | Self::CreativeLook(node, _) => Some(node),
            Self::Primary(..) | Self::Wheels(..) | Self::Curves(..) => None,
        }
    }

    /// This node's active matte, or `None` when it carries none (CC5 §2.6).
    ///
    /// `None` is normative, not an optimization: it is what makes
    /// [`apply_color_nodes_at`] skip the blend entirely, and
    /// `x + (y − x)·1.0` is not bit-identical to `y` in `f32` (CC5 §2.5,
    /// rule 4).
    #[must_use]
    pub const fn matte(&self) -> Option<&Matte> {
        match self {
            Self::TechnicalLut(_) => None,
            Self::Primary(_, matte)
            | Self::Wheels(_, matte)
            | Self::Curves(_, matte)
            | Self::CreativeLook(_, matte) => matte.as_ref(),
        }
    }
}

fn apply_color_node(node: &ColorNode, linear_rgb: [f32; 3]) -> [f32; 3] {
    match node {
        ColorNode::TechnicalLut(lut) | ColorNode::CreativeLook(lut, _) => lut.apply(linear_rgb),
        ColorNode::Primary(correction, _) => correction.apply(linear_rgb),
        ColorNode::Wheels(wheels, _) => wheels.apply(linear_rgb),
        ColorNode::Curves(curves, _) => curves.apply(linear_rgb),
    }
}

/// The pre-CC4 spelling of [`resolve_color_nodes_with`], kept so no caller has
/// to change at once — but resolved against an **empty** library.
///
/// CC4 §4.4 requires exactly this: the old symbol survives, and a LUT node it
/// cannot resolve fails with `missing_lut_asset` rather than silently
/// rendering a look-free frame.  Any caller that can carry a LUT node must
/// move to [`resolve_color_nodes_with`].
///
/// # Errors
///
/// Returns [`ColorPipelineError::TooManyColorNodes`] when the layer carries
/// more than [`COLOR_NODE_LIMIT_PER_LAYER`] managed nodes — CC3 §3.1 requires a
/// typed error, never a silent truncation — any
/// [`PrimaryCorrection::from_effect`] error for a malformed primary node, and
/// [`ColorPipelineError::MissingLutAsset`] for **every** active LUT node,
/// because this wrapper resolves against an empty library.
pub fn resolve_color_nodes(effects: &[Effect]) -> Result<Vec<ColorNode>, ColorPipelineError> {
    resolve_color_nodes_with(effects, &LutLibrary::default())
}

/// Resolve the ordered colour-node stack of one *keyframe-evaluated* effect
/// list against a verified LUT library (CC3 §3.1, §3.3; CC4 §3.6, §4.4).
///
/// Keyframes must already have been resolved by the caller
/// (`Effect::evaluated_at`).  Nodes that Core reports inactive — bypassed,
/// neutral, or (for a LUT node) unbound — are dropped entirely rather than
/// evaluated as an approximate identity, which is what makes the CC3 §10.3 and
/// CC4 §10.3.2 identity gates bit-identical.  `primary_correction` has no
/// bypass control and no neutral short-circuit, so it is always included,
/// exactly as CC1 specifies.  Effects that are not managed colour nodes are
/// ignored.
///
/// The vector order is the execution order: this function never sorts by
/// stage, because the inspector, the proof manifest, and `clip.effects` must
/// agree about what runs when (CC4 §3.2).  A stage-order violation is rejected
/// by Core at the edit boundary, not repaired here.
///
/// # Errors
///
/// Returns [`ColorPipelineError::TooManyColorNodes`] when the layer carries
/// more than [`COLOR_NODE_LIMIT_PER_LAYER`] managed nodes, any
/// [`PrimaryCorrection::from_effect`] error for a malformed primary node,
/// [`ColorPipelineError::MissingLutAsset`] when an active LUT node references
/// an asset the library did not verify, and
/// [`ColorPipelineError::UnsupportedInputEncoding`] for an out-of-range
/// encoding token.
pub fn resolve_color_nodes_with(
    effects: &[Effect],
    library: &LutLibrary,
) -> Result<Vec<ColorNode>, ColorPipelineError> {
    let count = managed_color_node_count(effects);
    if count > COLOR_NODE_LIMIT_PER_LAYER {
        return Err(ColorPipelineError::TooManyColorNodes {
            count,
            limit: COLOR_NODE_LIMIT_PER_LAYER,
        });
    }
    let mut nodes = Vec::with_capacity(count);
    for (index, kind) in active_color_nodes(effects) {
        let effect = &effects[index];
        // CC5 §2.6: the matte question is answered on Core's stored integers.
        // `active_color_nodes` has already dropped every node Core reports
        // inactive, `MatteExcluded` included, so a node that reaches this loop
        // either carries a matte the renderer must evaluate or none at all.
        let matte = Matte::from_params(&MatteParams::from_effect(effect));
        nodes.push(match kind {
            ColorNodeKind::TechnicalLut => ColorNode::TechnicalLut(LutNode::from_params(
                &LutNodeParams::from_effect(effect),
                library,
            )?),
            ColorNodeKind::Primary => {
                ColorNode::Primary(PrimaryCorrection::from_effect(effect)?, matte)
            }
            ColorNodeKind::Wheels => ColorNode::Wheels(
                ColorWheels::from_params(&ColorWheelsParams::from_effect(effect)),
                matte,
            ),
            ColorNodeKind::Curves => ColorNode::Curves(
                ColorCurves::from_resolved(&ResolvedCurves::from_effect(effect)),
                matte,
            ),
            ColorNodeKind::CreativeLook => ColorNode::CreativeLook(
                LutNode::from_params(&LutNodeParams::from_effect(effect), library)?,
                matte,
            ),
        });
    }
    Ok(nodes)
}

/// Apply a resolved node stack in vector order **at one position**, with no
/// RGB clamp between nodes (CC3 §3.1, CC5 §2.5, §3.4).
///
/// `uv` is the layer output quad uv with `u.y = 0` at the top, evaluated at
/// the pixel centre `((x + 0.5) / W, (y + 0.5) / H)`; `aspect` is the output
/// raster aspect `W / H`, supplied by the caller rather than sniffed.  Each
/// node consumes scene-linear working RGB and produces scene-linear working
/// RGB.  An empty stack is the exact identity.
///
/// This replaces CC3's two-argument `apply_color_nodes`.  The positionless
/// spelling is deliberately **not** kept as a wrapper: it would have to invent
/// a position, and applying a matte-scoped correction to every pixel is
/// precisely the failure CC5 exists to prevent (CC5 §3.4).
///
/// Two normative branches, neither an optimization (CC5 §2.5, rules 4 and 5):
///
/// * a node with no matte is applied directly, because `x + (y − x)·1.0` is
///   not bit-identical to `y` in `f32`;
/// * a coverage of exactly `0` returns `x` untouched, because
///   `x + (node(x) − x)·0.0` maps `−0.0` to `+0.0` and maps any `x` to `NaN`
///   whenever `node(x)` is non-finite, which CC1 §2.2.5's no-clamp rule makes
///   reachable on over-range input.
#[must_use]
pub fn apply_color_nodes_at(
    nodes: &[ColorNode],
    rgb: [f32; 3],
    uv: [f32; 2],
    aspect: f32,
) -> [f32; 3] {
    let mut linear_rgb = rgb;
    for node in nodes {
        let Some(matte) = node.matte() else {
            linear_rgb = apply_color_node(node, linear_rgb);
            continue;
        };
        let coverage = matte.coverage(uv, aspect, linear_rgb);
        if coverage == 0.0 {
            continue;
        }
        let corrected = apply_color_node(node, linear_rgb);
        linear_rgb = std::array::from_fn(|channel| {
            linear_rgb[channel] + (corrected[channel] - linear_rgb[channel]) * coverage
        });
    }
    linear_rgb
}

fn smoothstep(start: f32, end: f32, value: f32) -> f32 {
    let t = ((value - start) / (end - start)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn quantize_monitor(value: f32) -> u8 {
    let clamped = if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    };
    (clamped * 255.0).round() as u8
}

/// Apply BT.709 monitor encoding, then clamp and quantize RGB to 8-bit.
#[must_use]
pub fn encode_monitor_rgb8(linear_rgb: [f32; 3]) -> [u8; 3] {
    linear_rgb.map(|value| quantize_monitor(encode_bt709(value)))
}

/// Apply BT.709 monitor encoding to RGB and final clamp/quantization to alpha.
#[must_use]
pub fn encode_monitor_rgba8(linear_rgba: [f32; 4]) -> [u8; 4] {
    [
        quantize_monitor(encode_bt709(linear_rgba[0])),
        quantize_monitor(encode_bt709(linear_rgba[1])),
        quantize_monitor(encode_bt709(linear_rgba[2])),
        quantize_monitor(linear_rgba[3]),
    ]
}

/// Encode a linear RGB value using the requested monitoring description.
///
/// CC1 currently has one monitor transfer.  The description is checked so a
/// future target cannot silently select a backend default.
///
/// # Errors
///
/// Returns an error when the monitoring transfer is unknown or not the CC1
/// BT.709 transfer.
pub fn encode_monitor_for_description(
    linear_rgb: [f32; 3],
    monitoring: &ColorDescription,
) -> Result<[u8; 3], ColorPipelineError> {
    match &monitoring.transfer {
        ColorTransfer::Bt709 => Ok(encode_monitor_rgb8(linear_rgb)),
        ColorTransfer::Unknown => Err(ColorPipelineError::UnknownTransfer),
        value => Err(ColorPipelineError::UnsupportedMonitorTransfer(
            value.clone(),
        )),
    }
}

/// Encode a linear RGBA value using the requested monitoring description.
///
/// This is the RGBA form of [`encode_monitor_for_description`] used by the
/// production compositor readback, so §2.2.6 monitor selection is made from
/// project state instead of a hardcoded transform.
///
/// # Errors
///
/// Returns an error when the monitoring transfer is unknown or not the CC1
/// BT.709 transfer.
pub fn encode_monitor_rgba8_for_description(
    linear_rgba: [f32; 4],
    monitoring: &ColorDescription,
) -> Result<[u8; 4], ColorPipelineError> {
    match &monitoring.transfer {
        ColorTransfer::Bt709 => Ok(encode_monitor_rgba8(linear_rgba)),
        ColorTransfer::Unknown => Err(ColorPipelineError::UnknownTransfer),
        value => Err(ColorPipelineError::UnsupportedMonitorTransfer(
            value.clone(),
        )),
    }
}

/// CC8 §4's tone-mapping stage on the monitoring branch, applied to RGB and
/// leaving alpha alone.
///
/// §3.3 orders it "monitoring: tone-mapped preview (§4) on an SDR display" ->
/// "final clamp, quantization, and display/codec packing", so this runs in
/// **linear light**, ahead of the BT.709 display encode, and clamps nothing:
/// [`encode_monitor_rgb8`] still performs the single display clamp CC1 §2.2
/// invariant 5 allows. The curve itself is
/// [`kinewright_core::cc8_preview_tone_map`] in the authority module, where §4
/// item 2 requires its parameter to be pinned; this function is only the
/// composition, so the media crate carries no second copy of the arithmetic.
///
/// Alpha is untouched for the reason [`encode_delivery_hlg_rec2020_rgba16`]
/// gives on the other branch: a preview transform changes the colour stages,
/// not the alpha convention.
#[must_use]
pub fn tone_map_preview_rgba(linear_rgba: [f32; 4]) -> [f32; 4] {
    let mapped = cc8_preview_tone_map_rgb([linear_rgba[0], linear_rgba[1], linear_rgba[2]]);
    [mapped[0], mapped[1], mapped[2], linear_rgba[3]]
}

/// Encode a linear RGB value for one monitoring description **and one CC8 §4
/// preview arm**.
///
/// [`MonitorPreview::Direct`] is [`encode_monitor_for_description`], called
/// unchanged: the SDR monitor transform does not move, and the statement
/// executed for an SDR project is the one that was executed before CC8 §10
/// step 8 (§9.1 fixture 6's obligation, taken at the monitoring boundary).
/// [`MonitorPreview::Cc8ToneMappedHdr`] inserts §4's stage ahead of it and
/// changes nothing else.
///
/// # Errors
///
/// Returns an error when the monitoring transfer is unknown or not the CC1
/// BT.709 transfer, forwarded unchanged from
/// [`encode_monitor_for_description`].
pub fn encode_monitor_for_preview(
    linear_rgb: [f32; 3],
    monitoring: &ColorDescription,
    preview: MonitorPreview,
) -> Result<[u8; 3], ColorPipelineError> {
    match preview {
        MonitorPreview::Direct => encode_monitor_for_description(linear_rgb, monitoring),
        MonitorPreview::Cc8ToneMappedHdr => {
            encode_monitor_for_description(cc8_preview_tone_map_rgb(linear_rgb), monitoring)
        }
    }
}

/// The RGBA form of [`encode_monitor_for_preview`], used by the production
/// compositor readback.
///
/// # Errors
///
/// Returns an error when the monitoring transfer is unknown or not the CC1
/// BT.709 transfer, forwarded unchanged from
/// [`encode_monitor_rgba8_for_description`].
pub fn encode_monitor_rgba8_for_preview(
    linear_rgba: [f32; 4],
    monitoring: &ColorDescription,
    preview: MonitorPreview,
) -> Result<[u8; 4], ColorPipelineError> {
    match preview {
        MonitorPreview::Direct => encode_monitor_rgba8_for_description(linear_rgba, monitoring),
        MonitorPreview::Cc8ToneMappedHdr => {
            encode_monitor_rgba8_for_description(tone_map_preview_rgba(linear_rgba), monitoring)
        }
    }
}

/// Nominal white in the 16-bit RGB delivery intermediate: `255 << 8`.
///
/// The intermediate exists only to carry the compositor's single 16-bit
/// quantization into the export filter graph (`rgba64le` → `scale` →
/// `yuv420p`), and `libswscale`'s convention for 16-bit RGB input is the
/// left-shifted 8-bit code, not a rescale to `u16::MAX`.  This is exactly the
/// `P_8 = 65280` promotion §3.1 already documents for the decode direction
/// (`C_rgba64 = C_native << (16 - N)`), applied to the encode direction so the
/// round trip is symmetric.
///
/// Scaling to `65_535` instead makes swscale read nominal white as *above*
/// nominal white, so limited-range luma lands on 236 (10-bit 943) rather than
/// legal white 235 (10-bit 940), and every value below it is biased high.
pub const DELIVERY_INTERMEDIATE_WHITE: u16 = 65_280;

/// Quantize one already-encoded delivery value to a 16-bit intermediate code.
///
/// This is the single quantization allowed before codec packing: the value is
/// clamped once and rounded once, in f32, at the delivery boundary.  The scale
/// is [`DELIVERY_INTERMEDIATE_WHITE`], so `1.0` becomes swscale's nominal
/// 16-bit RGB white rather than `u16::MAX`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn quantize_delivery16(value: f32) -> u16 {
    let clamped = if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    };
    (clamped * f32::from(DELIVERY_INTERMEDIATE_WHITE)).round() as u16
}

/// Apply BT.709 delivery encoding, then clamp and quantize RGB to the 16-bit
/// intermediate. Alpha is a real 16-bit destination channel and is quantized
/// without a transfer, on the same [`DELIVERY_INTERMEDIATE_WHITE`] scale as
/// RGB so the intermediate has exactly one scale to invert.
#[must_use]
pub fn encode_delivery_rgba16(linear_rgba: [f32; 4]) -> [u16; 4] {
    [
        quantize_delivery16(encode_bt709(linear_rgba[0])),
        quantize_delivery16(encode_bt709(linear_rgba[1])),
        quantize_delivery16(encode_bt709(linear_rgba[2])),
        quantize_delivery16(linear_rgba[3]),
    ]
}

/// CC8 §3.3's **delivery** direction for §5.1's HDR lane: working BT.709 D65
/// scene-linear to the 16-bit HLG-coded Rec.2020 delivery intermediate.
///
/// §3.3's pipeline listing gives the encode side in two stages, in this order:
///
/// ```text
/// delivery:   primaries conversion to Rec.2020 -> HLG OOTF+OETF -> §5
///   -> final clamp, quantization, and display/codec packing
/// ```
///
/// so:
///
/// 1. **Primaries conversion to Rec.2020.**
///    [`kinewright_core::CC8_BT709_TO_REC2020`] through
///    [`kinewright_core::cc8_apply_matrix`] — the exact inverse of the decode
///    side's [`source_primaries_to_working_linear`], and §2.3's own words: "The
///    inverse matrix at delivery restores them exactly." Unclamped, so an
///    out-of-Rec.709 colour that survived the working space as **negative**
///    BT.709 components comes back as the positive Rec.2020 value it started
///    as (CC1 §2.2 invariant 5, restated by §2.3).
/// 2. **HLG OOTF+OETF**, which is [`kinewright_core::cc8_hlg_encode_working_linear`]:
///    the anchor scale to cd/m², then the BT.2100 **inverse** OOTF, then the
///    ARIB STD-B67 OETF. §3.3 names the stage by its OETF/OOTF pair rather than
///    by direction — `cc8_hlg_decode_working_linear`'s doc comment records that
///    reading and its mirror image on the decode line — and the function whose
///    signature matches the direction it is used in is the one that realizes
///    it. This composition is the exact inverse of the decode chain, which is
///    what makes §9.1 fixture 8's round trip a measurement rather than a hope.
/// 3. **Final clamp and quantization**, [`quantize_delivery16`]: the single
///    clamp and the single rounding CC1 §2.2 invariant 5 allows, on the
///    [`DELIVERY_INTERMEDIATE_WHITE`] scale.
///
/// **What white means on this intermediate.** §5.1 settles it rather than
/// leaving it to be guessed: the HDR lane "adds a *colour description*, not a
/// codec path, so `DELIVERY_SCALER_FLAGS = "bicubic"`, the
/// `DELIVERY_INTERMEDIATE_WHITE = 65_280` convention, and the single-pass
/// filter graph are unchanged and are not re-measured." So `65_280` is
/// `libswscale`'s nominal 16-bit RGB white here exactly as on the SDR lane, and
/// what lands on it is the **HLG signal** `E' = 1.0` — the top of the HLG
/// signal range — which swscale's full-to-limited conversion then writes as
/// 10-bit legal white 940. It is *not* the working diffuse white: BT.2408 puts
/// HLG diffuse white at [`kinewright_core::CC8_HLG_REFERENCE_WHITE_SIGNAL_PERCENT`]
/// of the signal range, so working `1.0` encodes to `E' = 0.75`, code
/// `48_960`, and 10-bit luma 721. That is the same relation the decode side
/// already carries, seen from the other end, and §9.1 fixture 8 measures it.
///
/// Alpha is a real 16-bit destination channel and is quantized without a
/// transfer, on the same scale as RGB, exactly as [`encode_delivery_rgba16`]
/// does — the lane changes the colour stages, not the alpha convention.
#[must_use]
pub fn encode_delivery_hlg_rec2020_rgba16(linear_rgba: [f32; 4]) -> [u16; 4] {
    let working = [linear_rgba[0], linear_rgba[1], linear_rgba[2]];
    let rec2020_linear = cc8_apply_matrix(CC8_BT709_TO_REC2020, working);
    let hlg_signal = cc8_hlg_encode_working_linear(rec2020_linear);
    [
        quantize_delivery16(hlg_signal[0]),
        quantize_delivery16(hlg_signal[1]),
        quantize_delivery16(hlg_signal[2]),
        quantize_delivery16(linear_rgba[3]),
    ]
}

/// Encode a linear RGBA value using the requested delivery description.
///
/// The **delivery lane** selects the encode, and the lane is a function of the
/// delivery description alone (CC8 §5.2 clause 1). The description is still
/// checked so the delivery target can never be a `libavfilter` or codec
/// default, and the result is still the 16-bit [`DELIVERY_INTERMEDIATE_WHITE`]
/// intermediate so the only depth quantization in the export path is the
/// YUV420P/YUV420P10LE conversion itself.
///
/// The HDR arm accepts §5.1's transfer only. A `bt2020` + `smpte2084`
/// description selects the HDR lane by its pair but is not §5.1's lane, so it
/// is refused here as well as at the export gate — §11 defers PQ delivery, and
/// a silent HLG encode under a PQ tag would be exactly the mislabelling §5.2
/// exists to prevent.
///
/// # Errors
///
/// Returns an error when the delivery transfer is unknown, or is not the lane's
/// transfer: BT.709 on the SDR lane, ARIB STD-B67 on CC8 §5.1's HDR lane.
pub fn encode_delivery_for_description(
    linear_rgba: [f32; 4],
    delivery: &ColorDescription,
) -> Result<[u16; 4], ColorPipelineError> {
    match DeliveryLane::for_description(delivery) {
        DeliveryLane::SdrRec709 => match &delivery.transfer {
            ColorTransfer::Bt709 => Ok(encode_delivery_rgba16(linear_rgba)),
            ColorTransfer::Unknown => Err(ColorPipelineError::UnknownTransfer),
            value => Err(ColorPipelineError::UnsupportedDeliveryTransfer(
                value.clone(),
            )),
        },
        DeliveryLane::HdrHlgRec2020 => match &delivery.transfer {
            ColorTransfer::AribStdB67 => Ok(encode_delivery_hlg_rec2020_rgba16(linear_rgba)),
            value => Err(ColorPipelineError::UnsupportedDeliveryTransfer(
                value.clone(),
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn rec709(range: ColorRange, transfer: ColorTransfer) -> ColorDescription {
        ColorDescription {
            primaries: ColorPrimaries::Bt709,
            transfer,
            matrix: ColorMatrix::Bt709,
            range,
            white_point: ColorWhitePoint::D65,
            bit_depth: ColorBitDepth::Eight,
            confidence_basis_points: 10_000,
            provenance: kinewright_core::ColorProvenance::StreamMetadata,
        }
    }

    fn srgb() -> ColorDescription {
        ColorDescription {
            primaries: ColorPrimaries::Srgb,
            transfer: ColorTransfer::Srgb,
            matrix: ColorMatrix::Identity,
            range: ColorRange::Full,
            white_point: ColorWhitePoint::D65,
            bit_depth: ColorBitDepth::Eight,
            confidence_basis_points: 10_000,
            provenance: kinewright_core::ColorProvenance::StreamMetadata,
        }
    }

    fn assert_close(actual: f32, expected: f32, tolerance: f32) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "actual {actual} expected {expected} (tol {tolerance})"
        );
    }

    fn primary_effect(parameters: BTreeMap<String, ParamValue>) -> Effect {
        Effect {
            id: kinewright_core::EffectId(1),
            name: "primary_correction".to_owned(),
            parameters,
            keyframes: BTreeMap::new(),
        }
    }

    #[test]
    fn source_profiles_match_only_complete_tuples() {
        assert_eq!(
            classify_source(&rec709(ColorRange::Limited, ColorTransfer::Bt709)),
            Ok(SupportedSourceProfile::Rec709Video)
        );
        assert_eq!(
            classify_source(&rec709(ColorRange::Full, ColorTransfer::Bt1886)),
            Ok(SupportedSourceProfile::Rec709Video)
        );
        assert_eq!(
            classify_source(&srgb()),
            Ok(SupportedSourceProfile::SrgbFull)
        );

        let mut bt709_srgb = rec709(ColorRange::Full, ColorTransfer::Srgb);
        bt709_srgb.matrix = ColorMatrix::Rgb;
        assert_eq!(
            classify_source(&bt709_srgb),
            Ok(SupportedSourceProfile::SrgbFull)
        );
    }

    #[test]
    fn explicit_d65_assumption_does_not_rewrite_metadata() {
        let mut description = rec709(ColorRange::Limited, ColorTransfer::Bt709);
        description.white_point = ColorWhitePoint::Unknown;
        assert_eq!(
            classify_source(&description),
            Err(ColorSourceError::UnknownWhitePoint)
        );
        assert_eq!(
            classify_source_with_assumption(&description, Some(SourceProfileAssumption::D65)),
            Ok(SupportedSourceProfile::Rec709Video)
        );
        assert_eq!(description.white_point, ColorWhitePoint::Unknown);
    }

    fn cc8_hdr_description(transfer: ColorTransfer) -> ColorDescription {
        ColorDescription {
            primaries: ColorPrimaries::Bt2020,
            transfer,
            matrix: ColorMatrix::Bt2020Ncl,
            range: ColorRange::Limited,
            white_point: ColorWhitePoint::D65,
            bit_depth: ColorBitDepth::Ten,
            confidence_basis_points: 10_000,
            provenance: kinewright_core::ColorProvenance::StreamMetadata,
        }
    }

    /// CC8 §2.2/§3.3: the managed input conversion routes a classified HDR
    /// source through the HDR stages, and keeps the SDR path per-channel.
    #[test]
    fn decode_source_rgb_selects_the_cc8_hdr_stages_by_profile() {
        // The PQ leg lands §2.2's anchor: a signal encoding 203 cd/m² is
        // working 1.0. Neither 203 nor 1.0/203 is written here.
        #[allow(clippy::cast_precision_loss)]
        let anchor = kinewright_core::CC8_REFERENCE_WHITE_NITS as f32;
        let pq_signal = kinewright_core::cc8_pq_inverse_eotf(anchor);
        let pq = decode_source_rgb(
            &cc8_hdr_description(ColorTransfer::Smpte2084),
            [pq_signal; 3],
            None,
        )
        .expect("a pq_rec2020 source decodes");
        for channel in pq {
            assert!((channel - 1.0).abs() < 1.0e-3, "PQ decoded to {pq:?}");
        }

        // The HLG leg lands the same 1.0 from BT.2408's 75 % signal — §10 step
        // 3's determination, exercised through the production entry point.
        #[allow(clippy::cast_precision_loss)]
        let hlg_signal = kinewright_core::CC8_HLG_REFERENCE_WHITE_SIGNAL_PERCENT as f32 / 100.0;
        let hlg = decode_source_rgb(
            &cc8_hdr_description(ColorTransfer::AribStdB67),
            [hlg_signal; 3],
            None,
        )
        .expect("an hlg_rec2020 source decodes");
        for channel in hlg {
            assert!(
                (channel - 1.0).abs() < 0.5 / anchor,
                "HLG decoded to {hlg:?}"
            );
        }

        // The failing direction that proves the routing is real: the SDR
        // per-channel decode refuses both HDR transfers, so an HDR source that
        // reached `decode_transfer` would error rather than be misread.
        for transfer in [ColorTransfer::Smpte2084, ColorTransfer::AribStdB67] {
            assert!(matches!(
                decode_transfer(&transfer, 0.5),
                Err(ColorPipelineError::UnsupportedTransfer(_))
            ));
        }
        // And the HDR stages refuse an SDR profile rather than acting as an
        // identity on it.
        for profile in [
            ColorSourceProfile::Rec709Video,
            ColorSourceProfile::SrgbFull,
        ] {
            let error = decode_hdr_source_working_linear(profile, [0.5; 3])
                .expect_err("a CC1 profile has no CC8 HDR stages");
            assert!(matches!(
                error,
                ColorPipelineError::NotAnHdrSourceProfile(_)
            ));
            assert!(error.to_string().contains("not_an_hdr_source_profile"));
        }

        // §2.3/CC1 §2.2 invariant 5: no clamp at any stage. A negative signal
        // survives as a negative working value and an over-range PQ code
        // survives at HDR magnitude.
        let undershoot = decode_source_rgb(
            &cc8_hdr_description(ColorTransfer::Smpte2084),
            [-0.05; 3],
            None,
        )
        .expect("undershoot decodes");
        assert!(undershoot.iter().all(|value| *value < 0.0));
        let peak = decode_source_rgb(
            &cc8_hdr_description(ColorTransfer::Smpte2084),
            [1.0; 3],
            None,
        )
        .expect("the PQ peak decodes");
        assert!(peak.iter().all(|value| *value > 49.0));
    }

    /// CC8 §2.3/§3.3's primaries stage, at the unit-test scale that sits beside
    /// the function; §9.1 fixture 3 is the raster-scale gate.
    #[test]
    #[allow(clippy::float_cmp)]
    fn source_primaries_stage_is_selected_by_profile_and_is_never_clamped() {
        // §3.3: "on a Rec.709 source the primaries stage stays the identity
        // matrix it is now" — bit-for-bit, so the CC1 lanes cannot move.
        for profile in [
            ColorSourceProfile::Rec709Video,
            ColorSourceProfile::SrgbFull,
        ] {
            for value in [[0.0_f32; 3], [0.25, 0.5, 0.75], [1.5, -0.05, 49.26]] {
                assert_eq!(source_primaries_to_working_linear(profile, value), value);
            }
        }
        // Both §2.1 HDR rows select the same non-identity matrix, because both
        // carry the same `Primaries` column.
        let saturated = [0.0_f32, 1.0, 0.0];
        let working = source_primaries_to_working_linear(ColorSourceProfile::PqRec2020, saturated);
        assert_eq!(
            working,
            source_primaries_to_working_linear(ColorSourceProfile::HlgRec2020, saturated),
        );
        // §2.3: a saturated Rec.2020 primary leaves **negative** BT.709
        // components, and nothing clamps them.
        assert!(
            working[0] < -0.1 && working[2] < 0.0,
            "a saturated Rec.2020 green must leave negatives: {working:?}",
        );
        // And the composition is the one `decode_source_rgb` runs, so a caller
        // cannot get the transfer half alone from the production entry point.
        let coded = kinewright_core::cc8_pq_inverse_eotf(kinewright_core::cc8_hdr::cc8_as_f32(
            kinewright_core::CC8_REFERENCE_WHITE_NITS,
        ));
        let composed =
            hdr_source_to_working_bt709_linear(ColorSourceProfile::PqRec2020, [coded, coded, 0.0])
                .expect("the PQ profile decodes");
        let staged = source_primaries_to_working_linear(
            ColorSourceProfile::PqRec2020,
            decode_hdr_source_working_linear(ColorSourceProfile::PqRec2020, [coded, coded, 0.0])
                .expect("the PQ transfer stages run"),
        );
        assert_eq!(composed, staged);
    }

    #[test]
    fn unknown_and_unsupported_metadata_report_the_exact_field() {
        let mut description = rec709(ColorRange::Limited, ColorTransfer::Bt709);

        description.primaries = ColorPrimaries::Unknown;
        assert_eq!(
            classify_source(&description),
            Err(ColorSourceError::UnknownPrimaries)
        );
        description.primaries = ColorPrimaries::Bt2020;
        assert_eq!(
            classify_source(&description),
            Err(ColorSourceError::UnsupportedPrimaries(
                ColorPrimaries::Bt2020
            ))
        );

        description.primaries = ColorPrimaries::Bt709;
        description.transfer = ColorTransfer::Unknown;
        assert_eq!(
            classify_source(&description),
            Err(ColorSourceError::UnknownTransfer)
        );
        description.transfer = ColorTransfer::Smpte2084;
        assert_eq!(
            classify_source(&description),
            Err(ColorSourceError::UnsupportedTransfer(
                ColorTransfer::Smpte2084
            ))
        );

        description.transfer = ColorTransfer::Bt709;
        description.matrix = ColorMatrix::Unknown;
        assert_eq!(
            classify_source(&description),
            Err(ColorSourceError::UnknownMatrix)
        );
        description.matrix = ColorMatrix::Smpte170M;
        assert_eq!(
            classify_source(&description),
            Err(ColorSourceError::UnsupportedMatrix(ColorMatrix::Smpte170M))
        );

        description.matrix = ColorMatrix::Bt709;
        description.range = ColorRange::Unknown;
        assert_eq!(
            classify_source(&description),
            Err(ColorSourceError::UnknownRange)
        );
        description.range = ColorRange::Other("extended".to_owned());
        assert_eq!(
            classify_source(&description),
            Err(ColorSourceError::UnsupportedRange(ColorRange::Other(
                "extended".to_owned()
            )))
        );

        description.range = ColorRange::Limited;
        description.white_point = ColorWhitePoint::Unknown;
        assert_eq!(
            classify_source(&description),
            Err(ColorSourceError::UnknownWhitePoint)
        );
        description.white_point = ColorWhitePoint::D50;
        assert_eq!(
            classify_source(&description),
            Err(ColorSourceError::UnsupportedWhitePoint(
                ColorWhitePoint::D50
            ))
        );

        description.white_point = ColorWhitePoint::D65;
        description.bit_depth = ColorBitDepth::Unknown;
        assert_eq!(
            classify_source(&description),
            Err(ColorSourceError::UnknownBitDepth)
        );
        description.bit_depth = ColorBitDepth::Float16;
        assert_eq!(
            classify_source(&description),
            Err(ColorSourceError::UnsupportedBitDepth(
                ColorBitDepth::Float16
            ))
        );
        description.bit_depth = ColorBitDepth::Integer(7);
        assert_eq!(
            classify_source(&description),
            Err(ColorSourceError::UnsupportedBitDepth(
                ColorBitDepth::Integer(7)
            ))
        );
    }

    #[test]
    fn integer_depths_8_through_16_are_supported() {
        for bits in 8_u8..=16 {
            let mut description = rec709(ColorRange::Limited, ColorTransfer::Bt709);
            description.bit_depth = ColorBitDepth::Integer(u16::from(bits));
            assert_eq!(
                classify_source(&description),
                Ok(SupportedSourceProfile::Rec709Video)
            );
            assert_eq!(integer_bit_depth(&description), Ok(bits));
        }
    }

    #[test]
    fn rgba64_normalization_matches_the_configured_swscale_boundary() {
        let mut full_8 = rec709(ColorRange::Full, ColorTransfer::Bt709);
        assert_eq!(rgba64_promoted_max(&full_8), Ok(65_280));
        assert_eq!(rgba64_normalization_max(&full_8), Ok(65_280));

        full_8.bit_depth = ColorBitDepth::Ten;
        assert_eq!(rgba64_promoted_max(&full_8), Ok(65_472));
        assert_eq!(rgba64_normalization_max(&full_8), Ok(65_472));

        let mut limited_10 = rec709(ColorRange::Limited, ColorTransfer::Bt709);
        limited_10.bit_depth = ColorBitDepth::Ten;
        // The direct limited YUV -> RGBA64 swscale path uses the 8-bit
        // fixed-point RGB scale even when its input planes are 10-bit.
        assert_eq!(rgba64_promoted_max(&limited_10), Ok(65_472));
        assert_eq!(rgba64_normalization_max(&limited_10), Ok(65_280));

        let mut rgb_8 = srgb();
        assert_eq!(rgba64_promoted_max(&rgb_8), Ok(65_280));
        assert_eq!(rgba64_normalization_max(&rgb_8), Ok(65_280));
        rgb_8.bit_depth = ColorBitDepth::Ten;
        assert_eq!(rgba64_promoted_max(&rgb_8), Ok(65_472));
        assert_eq!(rgba64_normalization_max(&rgb_8), Ok(65_535));

        let mut unsupported = srgb();
        unsupported.bit_depth = ColorBitDepth::Float16;
        assert_eq!(
            rgba64_normalization_max(&unsupported),
            Err(ColorPipelineError::UnsupportedBitDepth(
                ColorBitDepth::Float16
            ))
        );
    }

    #[test]
    fn rec709_rgb_limited_remains_an_explicitly_supported_source_tuple() {
        let mut description = rec709(ColorRange::Limited, ColorTransfer::Bt709);
        description.matrix = ColorMatrix::Rgb;
        description.bit_depth = ColorBitDepth::Eight;
        assert_eq!(
            classify_source(&description),
            Ok(SupportedSourceProfile::Rec709Video)
        );
        assert_eq!(rgba64_normalization_max(&description), Ok(65_280));

        description.bit_depth = ColorBitDepth::Ten;
        assert_eq!(rgba64_normalization_max(&description), Ok(65_535));
    }

    #[test]
    fn transfer_thresholds_match_the_contract() {
        // The CC1 inverse is specified with a strict low-branch comparison:
        // exactly .081 therefore takes the nonlinear branch. The rounded
        // constants intentionally leave a tiny seam at the branch boundary.
        assert_close(
            decode_bt709(0.081),
            ((0.081_f32 + 0.099) / 1.099).powf(1.0 / 0.45),
            1.0e-6,
        );
        assert_close(decode_bt709(0.081 - 1.0e-6), (0.081 - 1.0e-6) / 4.5, 1.0e-7);
        assert_close(
            decode_bt709(0.081 + 1.0e-6),
            ((0.081_f32 + 1.0e-6 + 0.099) / 1.099).powf(1.0 / 0.45),
            1.0e-6,
        );
        assert_close(decode_srgb(0.04045), 0.04045 / 12.92, 1.0e-6);
        assert_close(
            decode_srgb(0.04045 + 1.0e-6),
            ((0.04045_f32 + 1.0e-6 + 0.055) / 1.055).powf(2.4),
            1.0e-6,
        );
        assert_close(decode_bt1886(-0.25), 0.0, 0.0);
        assert_close(decode_bt1886(0.5), 0.5_f32.powf(2.4), 1.0e-6);
        // The forward contract also uses a strict low-branch comparison, so
        // exactly .018 is evaluated by the nonlinear branch.
        assert_close(
            encode_bt709(0.018),
            1.099 * 0.018_f32.powf(0.45) - 0.099,
            1.0e-6,
        );
        assert_close(
            encode_bt709(0.018 + 1.0e-6),
            1.099 * (0.018_f32 + 1.0e-6).powf(0.45) - 0.099,
            1.0e-6,
        );
    }

    #[test]
    fn transfer_ramps_are_monotonic_and_finite() {
        for decode in [
            decode_srgb as fn(f32) -> f32,
            decode_bt1886 as fn(f32) -> f32,
        ] {
            let mut previous = f32::NEG_INFINITY;
            for index in 0_u16..=10_000 {
                let value = decode(f32::from(index) / 10_000.0);
                assert!(value.is_finite());
                assert!(value >= previous);
                previous = value;
            }
        }

        // Rounded BT.709 constants produce a small, specified discontinuity
        // at the strict .081 branch boundary. Each branch remains monotone;
        // retain an explicit seam assertion so a larger regression fails.
        let mut previous = f32::NEG_INFINITY;
        for index in 0_u16..=809 {
            let value = decode_bt709(f32::from(index) / 10_000.0);
            assert!(value.is_finite());
            assert!(value >= previous);
            previous = value;
        }
        let boundary_low = previous;
        let boundary_high = decode_bt709(0.081);
        assert!(boundary_low - boundary_high < 1.0e-3);
        previous = boundary_high;
        for index in 810_u16..=10_000 {
            let value = decode_bt709(f32::from(index) / 10_000.0);
            assert!(value.is_finite());
            assert!(value >= previous);
            previous = value;
        }

        let mut previous = f32::NEG_INFINITY;
        for index in 0_u16..=10_000 {
            let value = encode_bt709(f32::from(index) / 10_000.0);
            assert!(value >= previous);
            previous = value;
        }
    }

    #[test]
    fn limited_range_expansion_uses_declared_depth_without_clamping() {
        let black = expand_native_range(
            [64.0 / 1023.0; 3],
            &ColorBitDepth::Ten,
            &ColorRange::Limited,
        )
        .expect("10-bit limited range");
        for value in black {
            assert_close(value, 0.0, 1.0e-6);
        }

        let white = expand_native_range(
            [940.0 / 1023.0; 3],
            &ColorBitDepth::Ten,
            &ColorRange::Limited,
        )
        .expect("10-bit limited range");
        for value in white {
            assert_close(value, 1.0, 1.0e-6);
        }

        let overshoot = expand_native_range(
            [0.0, 1023.0 / 1023.0, 64.0 / 1023.0],
            &ColorBitDepth::Ten,
            &ColorRange::Limited,
        )
        .expect("10-bit limited range");
        assert!(overshoot[0] < 0.0);
        assert!(overshoot[1] > 1.0);
        assert_close(overshoot[2], 0.0, 1.0e-6);
    }

    #[test]
    fn bt709_matrix_matches_the_explicit_coefficients() {
        let rgb = decode_bt709_ycbcr(
            [512.0 / 1023.0, 512.0 / 1023.0, 512.0 / 1023.0],
            &ColorBitDepth::Ten,
            &ColorRange::Limited,
        )
        .expect("10-bit BT.709 matrix");
        let y = (512.0 - 64.0) / 876.0;
        let c = (512.0 - 512.0) / 896.0;
        assert_close(rgb[0], y + 1.5748 * c, 1.0e-6);
        assert_close(rgb[1], y - 0.187_324 * c - 0.468_124 * c, 1.0e-6);
        assert_close(rgb[2], y + 1.8556 * c, 1.0e-6);
    }

    #[test]
    fn source_decode_transfer_decodes_post_swscale_rgb_without_range_expansion() {
        let description = rec709(ColorRange::Limited, ColorTransfer::Bt709);
        let decoded = decode_source_rgb(&description, [0.5; 3], None).expect("source decode");
        assert_close(decoded[0], decode_bt709(0.5), 1.0e-6);
        assert_close(decoded[1], decoded[0], 1.0e-6);
        assert_close(decoded[2], decoded[0], 1.0e-6);
    }

    #[test]
    fn primary_identity_and_exposure_units_are_exact_enough() {
        let identity = PrimaryCorrection::default();
        let input = [0.1, 0.5, 1.25];
        let output = identity.apply_checked(input).expect("neutral controls");
        for (actual, expected) in output.into_iter().zip(input) {
            assert_close(actual, expected, 1.0e-7);
        }

        let mut one_stop = identity;
        one_stop.exposure_milli_stops = 1_000;
        let output = one_stop.apply_checked([0.25, 0.5, 1.0]).expect("one stop");
        assert_close(output[0], 0.5, 1.0e-6);
        assert_close(output[1], 1.0, 1.0e-6);
        assert_close(output[2], 2.0, 1.0e-6);
    }

    #[test]
    fn primary_control_order_and_weights_match_contract() {
        let correction = PrimaryCorrection {
            temperature_percent: 100,
            tint_percent: 100,
            exposure_milli_stops: 0,
            contrast_percent: 0,
            contrast_pivot_basis_points: 5_000,
            blacks_percent: 100,
            shadows_percent: 100,
            highlights_percent: 100,
            whites_percent: 100,
            saturation_percent: 0,
        };
        let output = correction
            .apply_checked([0.0, 0.5, 1.0])
            .expect("valid controls");

        // White balance first gives [0.0, 0.45, 0.9]. Tonal weights use the
        // clamped post-white-balance values and never clamp the resulting x.
        let expected_red = 0.25 + 0.20;
        let expected_green = 0.45 + 0.20 * (1.0 - smoothstep(0.15, 0.50, 0.45));
        let expected_blue =
            0.9 + 0.20 * smoothstep(0.50, 0.85, 0.9) + 0.25 * smoothstep(0.75, 1.0, 0.9);
        assert_close(output[0], expected_red, 1.0e-6);
        assert_close(output[1], expected_green, 1.0e-6);
        assert_close(output[2], expected_blue, 1.0e-6);
    }

    #[test]
    fn primary_bounds_are_inclusive_and_invalid_values_are_rejected() {
        let mut correction = PrimaryCorrection::default();
        for parameter in PrimaryParameter::ALL {
            let (min, max, neutral) = parameter.bounds();
            assert_eq!(correction.parameter(parameter), neutral);
            correction
                .set_parameter(parameter, min)
                .expect("minimum is inclusive");
            correction
                .set_parameter(parameter, max)
                .expect("maximum is inclusive");
        }
        assert!(matches!(
            correction.set_parameter(PrimaryParameter::SaturationPercent, 101),
            Err(ColorPipelineError::InvalidPrimaryParameter {
                parameter: PrimaryParameter::SaturationPercent,
                value: 101,
                min: -100,
                max: 100,
            })
        ));
        correction.contrast_pivot_basis_points = -1;
        assert!(matches!(
            correction.validate(),
            Err(ColorPipelineError::InvalidPrimaryParameter {
                parameter: PrimaryParameter::ContrastPivotBasisPoints,
                value: -1,
                min: 0,
                max: 10_000,
            })
        ));
    }

    #[test]
    fn primary_effect_conversion_uses_core_descriptor_and_neutrals() {
        let effect = primary_effect(BTreeMap::from([
            (
                "exposure_milli_stops".to_owned(),
                ParamValue::Integer(1_000),
            ),
            (
                "contrast_pivot_basis_points".to_owned(),
                ParamValue::Integer(4_200),
            ),
            ("saturation_percent".to_owned(), ParamValue::Integer(-100)),
        ]));
        let correction = PrimaryCorrection::from_effect(&effect).expect("primary effect");
        assert_eq!(correction.exposure_milli_stops, 1_000);
        assert_eq!(correction.contrast_pivot_basis_points, 4_200);
        assert_eq!(correction.saturation_percent, -100);
        assert_eq!(correction.temperature_percent, 0);
        assert_eq!(correction.whites_percent, 0);
    }

    #[test]
    fn primary_effect_conversion_rejects_legacy_names_types_unknowns_and_bounds() {
        let mut legacy = primary_effect(BTreeMap::new());
        legacy.name = "color_grade".to_owned();
        assert_eq!(
            PrimaryCorrection::from_effect(&legacy),
            Err(ColorPipelineError::UnsupportedEffectName(
                "color_grade".to_owned()
            ))
        );

        let unknown = primary_effect(BTreeMap::from([(
            "unknown_control".to_owned(),
            ParamValue::Integer(0),
        )]));
        assert_eq!(
            PrimaryCorrection::from_effect(&unknown),
            Err(ColorPipelineError::UnknownPrimaryParameter(
                "unknown_control".to_owned()
            ))
        );

        let wrong_type = primary_effect(BTreeMap::from([(
            "exposure_milli_stops".to_owned(),
            ParamValue::Boolean(true),
        )]));
        assert_eq!(
            PrimaryCorrection::from_effect(&wrong_type),
            Err(ColorPipelineError::NonIntegerPrimaryParameter {
                name: "exposure_milli_stops".to_owned(),
                value: ParamValue::Boolean(true),
            })
        );

        let out_of_bounds = primary_effect(BTreeMap::from([(
            "saturation_percent".to_owned(),
            ParamValue::Integer(101),
        )]));
        assert_eq!(
            PrimaryCorrection::from_effect(&out_of_bounds),
            Err(ColorPipelineError::InvalidPrimaryParameter {
                parameter: PrimaryParameter::SaturationPercent,
                value: 101,
                min: -100,
                max: 100,
            })
        );
    }

    #[test]
    fn correction_does_not_clamp_before_monitor_boundary() {
        let positive = PrimaryCorrection {
            exposure_milli_stops: 1_000,
            ..PrimaryCorrection::default()
        };
        let negative = PrimaryCorrection {
            exposure_milli_stops: -1_000,
            ..PrimaryCorrection::default()
        };
        let over_range = positive
            .apply_checked([0.75, 0.75, 0.75])
            .expect("valid controls");
        assert!(over_range.iter().all(|value| *value > 1.0));

        // A later negative exposure recovers the original sample because the
        // serial node boundary does not clamp the over-range intermediate.
        let recovered = apply_primary_corrections([0.75, 0.75, 0.75], &[positive, negative])
            .expect("valid serial controls");
        for value in recovered {
            assert_close(value, 0.75, 1.0e-6);
        }
        assert_eq!(encode_monitor_rgb8(over_range), [255, 255, 255]);

        // Clamping after the first node would irreversibly lose the exposure
        // result and cannot recover the original sample.
        let incorrectly_clipped = negative
            .apply_checked(over_range.map(|value| value.clamp(0.0, 1.0)))
            .expect("valid clipped controls");
        assert!(incorrectly_clipped.iter().all(|value| *value < 0.75));
    }

    #[test]
    fn monitor_encoding_clamps_only_at_final_quantization() {
        assert_eq!(encode_monitor_rgb8([-1.0, 0.0, 2.0]), [0, 0, 255]);
        assert_eq!(
            encode_monitor_rgba8([-1.0, 0.0, 2.0, 1.5]),
            [0, 0, 255, 255]
        );
        assert_eq!(encode_monitor_rgb8([0.0, 0.5, 1.0]), [0, 180, 255]);
        assert_eq!(
            encode_monitor_rgb8([f32::NAN, 0.5, f32::INFINITY]),
            [0, 180, 255]
        );
    }

    #[test]
    fn monitor_description_cannot_silently_select_another_transfer() {
        let mut monitoring = rec709(ColorRange::Full, ColorTransfer::Bt709);
        monitoring.bit_depth = ColorBitDepth::Float32;
        assert_eq!(
            encode_monitor_for_description([0.5; 3], &monitoring),
            Ok([180; 3])
        );
        monitoring.transfer = ColorTransfer::Srgb;
        assert_eq!(
            encode_monitor_for_description([0.5; 3], &monitoring),
            Err(ColorPipelineError::UnsupportedMonitorTransfer(
                ColorTransfer::Srgb
            ))
        );
        assert_eq!(
            encode_monitor_rgba8_for_description([0.5, 0.5, 0.5, 1.0], &monitoring),
            Err(ColorPipelineError::UnsupportedMonitorTransfer(
                ColorTransfer::Srgb
            ))
        );
    }

    #[test]
    fn delivery_encoding_quantizes_mid_gray_once_at_sixteen_bits() {
        // One BT.709 OETF in f32, one clamp, one rounding.  Mid-gray linear
        // 0.5 encodes to 1.099 * 0.5^0.45 - 0.099 and must land on the exact
        // 16-bit intermediate code, not on an 8-bit code re-promoted to 16
        // bits.  The scale is swscale's nominal 16-bit RGB white, not
        // `u16::MAX`.
        assert_eq!(DELIVERY_INTERMEDIATE_WHITE, 65_280);
        assert_eq!(u32::from(DELIVERY_INTERMEDIATE_WHITE), 255_u32 << 8);
        let encoded = encode_bt709(0.5);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let expected = (encoded * f32::from(DELIVERY_INTERMEDIATE_WHITE)).round() as u16;
        assert_eq!(expected, 46_056);
        assert_eq!(
            encode_delivery_rgba16([0.5, 0.5, 0.5, 1.0]),
            [expected, expected, expected, DELIVERY_INTERMEDIATE_WHITE]
        );

        // The 8-bit monitor code for the same value is 180; a delivery path
        // that quantized to 8 bits first would produce 180 << 8 = 46080.
        assert_eq!(encode_monitor_rgb8([0.5; 3]), [180; 3]);
        assert_ne!(expected, 46_080);

        // Only the final step clamps.
        assert_eq!(
            encode_delivery_rgba16([-1.0, 0.0, 2.0, 1.5]),
            [
                0,
                0,
                DELIVERY_INTERMEDIATE_WHITE,
                DELIVERY_INTERMEDIATE_WHITE
            ]
        );
        assert_eq!(
            encode_delivery_rgba16([f32::NAN, 1.0, f32::INFINITY, f32::NAN]),
            [
                0,
                DELIVERY_INTERMEDIATE_WHITE,
                DELIVERY_INTERMEDIATE_WHITE,
                0
            ]
        );
    }

    #[test]
    fn delivery_description_cannot_silently_select_another_transfer() {
        let mut delivery = rec709(ColorRange::Limited, ColorTransfer::Bt709);
        assert_eq!(
            encode_delivery_for_description([0.5, 0.5, 0.5, 1.0], &delivery),
            Ok(encode_delivery_rgba16([0.5, 0.5, 0.5, 1.0]))
        );
        delivery.transfer = ColorTransfer::Srgb;
        assert_eq!(
            encode_delivery_for_description([0.5, 0.5, 0.5, 1.0], &delivery),
            Err(ColorPipelineError::UnsupportedDeliveryTransfer(
                ColorTransfer::Srgb
            ))
        );
        delivery.transfer = ColorTransfer::Unknown;
        assert_eq!(
            encode_delivery_for_description([0.5, 0.5, 0.5, 1.0], &delivery),
            Err(ColorPipelineError::UnknownTransfer)
        );
    }

    // -----------------------------------------------------------------------
    // CC3 curves and wheels.
    // -----------------------------------------------------------------------

    /// The CC3 §10.2 parity raster levels: negatives, the 0..1 range, the
    /// `grade709` breakpoint itself, and six levels above display white.
    const CC3_RASTER_LEVELS: [f32; 24] = [
        -0.50,
        -0.25,
        -0.10,
        -0.02,
        -0.005,
        0.0,
        0.002,
        0.005,
        GRADE709_BETA,
        0.03,
        0.06,
        0.10,
        0.18,
        0.25,
        0.35,
        0.50,
        0.65,
        0.80,
        0.90,
        1.00,
        1.20,
        1.50,
        2.50,
        4.00,
    ];

    fn color_node_effect(id: u64, name: &str, parameters: Vec<(String, i64)>) -> Effect {
        Effect {
            id: kinewright_core::EffectId(id),
            name: name.to_owned(),
            parameters: parameters
                .into_iter()
                .map(|(name, value)| (name, ParamValue::Integer(value)))
                .collect(),
            keyframes: BTreeMap::new(),
        }
    }

    fn wheels_effect_with_id(id: u64, parameters: &[(&str, i64)]) -> Effect {
        color_node_effect(
            id,
            "color_wheels",
            parameters
                .iter()
                .map(|(name, value)| ((*name).to_owned(), *value))
                .collect(),
        )
    }

    fn wheels_effect(parameters: &[(&str, i64)]) -> Effect {
        wheels_effect_with_id(2, parameters)
    }

    fn wheels(parameters: &[(&str, i64)]) -> ColorWheels {
        ColorWheels::from_params(&ColorWheelsParams::from_effect(&wheels_effect(parameters)))
    }

    /// A `color_curves` effect whose `curve` carries `points` and whose other
    /// three curves stay at the structural identity.
    fn curve_effect(curve: ColorCurveChannel, points: &[(i32, i32)], bypass: i64) -> Effect {
        let mut parameters = vec![(
            curve.point_count_parameter().to_owned(),
            i64::try_from(points.len()).expect("point count"),
        )];
        for (index, (x, y)) in points.iter().enumerate() {
            parameters.push((
                curve.x_parameter(index).expect("x name").to_owned(),
                i64::from(*x),
            ));
            parameters.push((
                curve.y_parameter(index).expect("y name").to_owned(),
                i64::from(*y),
            ));
        }
        parameters.push((
            kinewright_core::COLOR_NODE_BYPASS_PARAMETER.to_owned(),
            bypass,
        ));
        color_node_effect(3, "color_curves", parameters)
    }

    fn curve_points(points: &[(i32, i32)]) -> CurvePoints {
        CurvePoints {
            points: points.to_vec(),
            declared_point_count: points.len(),
            truncated: false,
        }
    }

    fn solved_curve(points: &[(i32, i32)]) -> ColorCurve {
        ColorCurve::from_points(&curve_points(points))
    }

    /// Only `curve` is non-identity; the other three stay structural identity.
    fn one_curve(curve: ColorCurveChannel, points: &[(i32, i32)]) -> ColorCurves {
        let mut resolved = ResolvedCurves {
            master: CurvePoints::identity(),
            red: CurvePoints::identity(),
            green: CurvePoints::identity(),
            blue: CurvePoints::identity(),
            bypass_token: 0,
        };
        let slot = match curve {
            ColorCurveChannel::Master => &mut resolved.master,
            ColorCurveChannel::Red => &mut resolved.red,
            ColorCurveChannel::Green => &mut resolved.green,
            ColorCurveChannel::Blue => &mut resolved.blue,
        };
        *slot = curve_points(points);
        ColorCurves::from_resolved(&resolved)
    }

    fn bits(values: [f32; 3]) -> [u32; 3] {
        values.map(f32::to_bits)
    }

    /// The CC5 §3.4 reference at the centre of a square raster.
    ///
    /// Every CC1/CC3/CC4 case below carries no matte, so the position and the
    /// aspect are immaterial and the result is the CC4 path byte for byte;
    /// `matte_free_nodes_take_the_cc4_path_bit_identically` is the assertion
    /// that says so.
    fn apply_stack(nodes: &[ColorNode], rgb: [f32; 3]) -> [f32; 3] {
        apply_color_nodes_at(nodes, rgb, [0.5, 0.5], 1.0)
    }

    #[test]
    fn grade709_matches_the_contract_anchors() {
        // Every value is CC3 §2.1's worked-anchor table, normative to 2e-5.
        const TOLERANCE: f32 = 2e-5;

        assert_close(grade709_encode(0.18), 0.408_848, TOLERANCE);

        // gain_red = 1200 -> slope_red = 1.2, every other control neutral.
        assert_close(
            wheels(&[("gain_red_thousandths", 1_200)]).apply([0.18; 3])[0],
            0.250_771,
            TOLERANCE,
        );

        // lift_master = -500 -> offset -0.05; gamma_master = 1200 -> power 1.2.
        assert_close(
            wheels(&[
                ("lift_master_basis_points", -500),
                ("gamma_master_thousandths", 1_200),
            ])
            .apply([0.18; 3])[0],
            0.100_923,
            TOLERANCE,
        );

        // master curve (0,0) (5000,6000) (10000,10000).
        assert_close(
            one_curve(
                ColorCurveChannel::Master,
                &[(0, 0), (5_000, 6_000), (10_000, 10_000)],
            )
            .apply([0.18; 3])[0],
            0.262_441,
            TOLERANCE,
        );
    }

    #[test]
    fn grade709_is_a_strictly_increasing_bijection_over_the_raster() {
        let mut previous = f32::NEG_INFINITY;
        for level in CC3_RASTER_LEVELS {
            let encoded = grade709_encode(level);
            assert!(
                encoded > previous,
                "E is not strictly increasing at {level}: {encoded} <= {previous}"
            );
            previous = encoded;

            let decoded = grade709_decode(encoded);
            let tolerance = 1e-6 * level.abs().max(1.0);
            assert!(
                (decoded - level).abs() <= tolerance,
                "D(E({level})) = {decoded} (tol {tolerance})"
            );
        }
    }

    #[test]
    fn grade709_and_wheels_preserve_zero_exactly() {
        // sgn(0) = 0, so E(0) = 0 and D(0) = 0 with no rounding at all.
        assert_eq!(grade709_encode(0.0).to_bits(), 0.0_f32.to_bits());
        assert_eq!(grade709_encode(-0.0).to_bits(), 0.0_f32.to_bits());
        assert_eq!(grade709_decode(0.0).to_bits(), 0.0_f32.to_bits());
        assert_eq!(grade709_decode(-0.0).to_bits(), 0.0_f32.to_bits());

        // A wheels node with zero offset maps 0 to 0: y = 0*slope + 0 = 0,
        // z = sgn(0)*|0|^p = 0, D(0) = 0, for any slope and any p > 0.
        let node = wheels(&[
            ("gain_red_thousandths", 1_200),
            ("gamma_master_thousandths", 100),
            ("gamma_blue_thousandths", 4_000),
        ]);
        assert_eq!(bits(node.apply([0.0; 3])), bits([0.0; 3]));
    }

    #[test]
    fn fritsch_carlson_limits_tangents_above_the_radius_three_circle() {
        // Points (0,0) (1250,1250) (2500,11250) are x = 0, 0.125, 0.25 and
        // y = 0, 0.125, 1.125 -- all exact in f32.
        //   delta   = [1.0, 8.0]
        //   step 2  -> m = [1.0, (1.0 + 8.0)/2 = 4.5, 8.0]
        //   i = 0: delta = 1, a = 1.0, b = 4.5, a^2 + b^2 = 21.25 > 9
        //          tau  = 3 / sqrt(21.25) = 0.650_791_373
        //          m[0] = tau * 1.0 * 1.0 = 0.650_791_373
        //          m[1] = tau * 4.5 * 1.0 = 2.928_561_181
        //   i = 1: delta = 8, a = m[1]/8 = 0.366_070_148, b = 1.0,
        //          a^2 + b^2 = 1.134 <= 9, so nothing more is limited.
        let curve = solved_curve(&[(0, 0), (1_250, 1_250), (2_500, 11_250)]);
        assert_close(curve.tangents()[0], 0.650_791_4, 1e-6);
        assert_close(curve.tangents()[1], 2.928_561_2, 1e-6);
        assert_close(curve.tangents()[2], 8.0, 1e-6);
    }

    #[test]
    fn fritsch_carlson_leaves_the_radius_three_boundary_unlimited() {
        // Points (0,0) (2500,0) (5000,625) (7500,3750) are x = 0, 0.25, 0.5,
        // 0.75 and y = 0, 0, 0.0625, 0.375 -- all exact in f32.
        //   delta   = [0.0, 0.25, 1.25]
        //   step 2  -> m = [0.0, 0.125, 0.75, 1.25]
        //   i = 0: delta == 0 -> m[0] = 0, m[1] = 0
        //   i = 1: delta = 0.25, a = 0/0.25 = 0, b = 0.75/0.25 = 3 exactly,
        //          a^2 + b^2 = 9 exactly, and 9 > 9 is false: no limiting.
        //   i = 2: delta = 1.25, a = 0.75/1.25 = 0.6, b = 1.0, sum = 1.36.
        let curve = solved_curve(&[(0, 0), (2_500, 0), (5_000, 625), (7_500, 3_750)]);
        let expected = [0.0_f32, 0.0, 0.75, 1.25];
        assert_eq!(
            curve
                .tangents()
                .iter()
                .map(|v| v.to_bits())
                .collect::<Vec<_>>(),
            expected.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn zero_slope_plateau_zeroes_both_tangents_and_stays_monotone() {
        // Points (0,0) (2500,5000) (5000,5000) (10000,10000) are x = 0, 0.25,
        // 0.5, 1.0 and y = 0, 0.5, 0.5, 1.0.
        //   delta   = [2.0, 0.0, 1.0]
        //   step 2  -> m = [2.0, 1.0, 0.5, 1.0]
        //   i = 0: delta = 2, a = 1.0, b = 0.5, sum = 1.25 <= 9
        //   i = 1: delta == 0 -> m[1] = 0, m[2] = 0
        //   i = 2: delta = 1, a = 0.0, b = 1.0, sum = 1.0 <= 9
        let curve = solved_curve(&[(0, 0), (2_500, 5_000), (5_000, 5_000), (10_000, 10_000)]);
        let expected = [2.0_f32, 0.0, 0.0, 1.0];
        assert_eq!(
            curve
                .tangents()
                .iter()
                .map(|v| v.to_bits())
                .collect::<Vec<_>>(),
            expected.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
        );

        // Both endpoints of the plateau segment are 0.5 with zero tangents, so
        // the Hermite basis is the constant 0.5 across it.
        for sample in [0.25_f32, 0.3, 0.375, 0.45, 0.499] {
            assert_close(curve.evaluate(sample), 0.5, 1e-6);
        }

        let mut previous = f32::NEG_INFINITY;
        for step in 0_i16..=200 {
            let sample = -0.2 + f32::from(step) * 0.01;
            let value = curve.evaluate(sample);
            assert!(
                value >= previous,
                "curve descends at {sample}: {value} < {previous}"
            );
            previous = value;
        }
    }

    #[test]
    fn sixteen_point_collinear_curve_is_identity_without_a_short_circuit() {
        const COORDINATES: [i32; 16] = [
            -2_000, -1_000, 0, 1_000, 2_000, 3_000, 4_000, 5_000, 6_000, 7_000, 8_000, 9_000,
            10_000, 10_500, 11_000, 12_000,
        ];
        let points: Vec<(i32, i32)> = COORDINATES.iter().map(|x| (*x, *x)).collect();
        let curve = ColorCurve::from_points(&curve_points(&points));

        // dy and dx are the same f32, so every secant slope is exactly 1.0,
        // the averages are exactly 1.0, and a = b = 1 never trips the limiter.
        for tangent in curve.tangents() {
            assert_eq!(tangent.to_bits(), 1.0_f32.to_bits());
        }
        for sample in [-0.25_f32, -0.2, -0.05, 0.0, 0.18, 0.5, 0.999, 1.0, 1.2, 1.5] {
            assert_close(curve.evaluate(sample), sample, 1e-6);
        }

        // The curve is mathematically identity but not *structurally* identity,
        // so CC3 §3.3 must still evaluate the node.
        let effect = curve_effect(ColorCurveChannel::Master, &points, 0);
        assert_eq!(
            resolve_color_nodes(std::slice::from_ref(&effect))
                .expect("collinear curve resolves")
                .len(),
            1
        );
    }

    #[test]
    fn curves_extrapolate_with_the_limited_end_tangents() {
        // (0,0) (5000,6000) (10000,10000): delta = [1.2, 0.8], m = [1.2, 1.0,
        // 0.8], and neither segment trips the limiter.
        let curve = solved_curve(&[(0, 0), (5_000, 6_000), (10_000, 10_000)]);
        assert_close(curve.tangents()[0], 1.2, 1e-6);
        assert_close(curve.tangents()[1], 1.0, 1e-6);
        assert_close(curve.tangents()[2], 0.8, 1e-6);

        // y = y[0] + m[0] * (x - x[0]) = 0.0 + 1.2 * (-0.2 - 0.0) = -0.24
        assert_close(curve.evaluate(-0.2), -0.24, 1e-6);
        // y = y[n-1] + m[n-1] * (x - x[n-1]) = 1.0 + 0.8 * (1.5 - 1.0) = 1.4
        assert_close(curve.evaluate(1.5), 1.4, 1e-6);
    }

    #[test]
    fn color_nodes_are_per_channel_independent() {
        let sample = [0.18_f32, 0.35, 0.90];

        let neutral_curves = one_curve(ColorCurveChannel::Master, &[(0, 0), (10_000, 10_000)]);
        let red_curves = one_curve(
            ColorCurveChannel::Red,
            &[(0, 0), (5_000, 6_000), (10_000, 10_000)],
        );
        let baseline = neutral_curves.apply(sample);
        let changed = red_curves.apply(sample);
        assert_eq!(changed[1].to_bits(), baseline[1].to_bits());
        assert_eq!(changed[2].to_bits(), baseline[2].to_bits());
        assert!((changed[0] - baseline[0]).abs() > 1e-3);

        let baseline = wheels(&[]).apply(sample);
        let changed = wheels(&[("gain_red_thousandths", 1_200)]).apply(sample);
        assert_eq!(changed[1].to_bits(), baseline[1].to_bits());
        assert_eq!(changed[2].to_bits(), baseline[2].to_bits());
        assert!((changed[0] - baseline[0]).abs() > 1e-3);
    }

    #[test]
    fn color_wheels_stay_finite_and_documented_at_the_control_bounds() {
        // gain_master = 0 -> slope 0 on every channel.  With offset 0 the node
        // is the constant 0: y = 0, z = sgn(0)*|0|^1 = 0, D(0) = 0.
        let zero_gain = wheels(&[("gain_master_thousandths", 0)]);
        assert_eq!(bits(zero_gain.slope()), bits([0.0; 3]));
        for level in [-0.5_f32, 0.18, 4.0] {
            assert_eq!(bits(zero_gain.apply([level; 3])), bits([0.0; 3]));
        }

        // gain_master = 0 with lift_master = 500 -> y = 0.05 for every input,
        // power 1, and |0.05| < BETA_E, so the output is 0.05 / 4.5.
        let lifted = wheels(&[
            ("gain_master_thousandths", 0),
            ("lift_master_basis_points", 500),
        ]);
        for level in [-0.5_f32, 0.18, 4.0] {
            assert_close(lifted.apply([level; 3])[0], 0.011_111_111, 1e-7);
        }

        // Both gamma controls at their minimum give the documented minimum
        // power of 0.1 * 0.1 = 0.01.
        let flat = wheels(&[
            ("gamma_master_thousandths", 100),
            ("gamma_red_thousandths", 100),
            ("gamma_green_thousandths", 100),
            ("gamma_blue_thousandths", 100),
        ]);
        assert_close(flat.power()[0], 0.01, 1e-7);
        // y = E(0.18) = 0.408_848_126; z = y^0.01 = 0.991_095_764;
        // D(z) = ((z + K) / ALPHA)^INV = 0.982_089_168
        assert_close(flat.apply([0.18; 3])[0], 0.982_089, 2e-5);
        // y = E(2.5) = 1.526_281; z = y^0.01 = 1.004_463_252; D(z) = 1.009_044_8
        assert_close(flat.apply([2.5; 3])[0], 1.009_045, 2e-5);

        // Both gain controls at their maximum give the documented maximum
        // slope of 4 * 4 = 16.
        let gain_max = wheels(&[
            ("gain_master_thousandths", 4_000),
            ("gain_red_thousandths", 4_000),
        ]);
        assert_close(gain_max.slope()[0], 16.0, 1e-6);
        // y = E(-0.5) * 16 = -11.272_690; power 1; D(y) = -180.363_045
        assert_close(gain_max.apply([-0.5; 3])[0], -180.363_05, 3e-3);
        // y = E(0.18) * 16 = 6.541_570; D(y) = 54.425_152
        assert_close(gain_max.apply([0.18; 3])[0], 54.425_15, 1e-3);
        // y = E(4.0) * 16 = 31.236_505; D(y) = 1710.256_596
        assert_close(gain_max.apply([4.0; 3])[0], 1_710.256_6, 3e-2);

        // Both gamma controls at their maximum give the documented maximum
        // power of 4 * 4 = 16.
        let gamma_max = wheels(&[
            ("gamma_master_thousandths", 4_000),
            ("gamma_red_thousandths", 4_000),
        ]);
        assert_close(gamma_max.power()[0], 16.0, 1e-6);
        // y = E(0.18) = 0.408_848; z = y^16 = 1.229_899e-6; D(z) = 1.354_502e-7
        assert_close(gamma_max.apply([0.18; 3])[0], 1.354_502e-7, 1e-11);
        // The odd extension keeps undershoot signed: z = -|E(-0.5)|^16.
        assert_close(gamma_max.apply([-0.5; 3])[0], -8.358_053e-4, 1e-8);
        // Maximum slope and maximum power together on the largest raster
        // level would exceed the f32 range; on an in-gamut sample they do not.
        // y = E(0.18) * 16 = 6.541_570; z = y^16 = 3.263_60e26 (approximately)
        let loud = wheels(&[
            ("gain_master_thousandths", 4_000),
            ("gain_red_thousandths", 4_000),
            ("gamma_master_thousandths", 4_000),
            ("gamma_red_thousandths", 4_000),
        ]);
        assert_close(loud.apply([0.18; 3])[0], 8.140_69e28, 1e25);
        for level in CC3_RASTER_LEVELS {
            assert!(
                gain_max
                    .apply([level; 3])
                    .iter()
                    .all(|value| value.is_finite()),
                "maximum gain is not finite at {level}"
            );
            assert!(
                gamma_max
                    .apply([level; 3])
                    .iter()
                    .all(|value| value.is_finite()),
                "maximum gamma is not finite at {level}"
            );
        }
    }

    #[test]
    fn node_order_is_the_clip_effects_order_and_changes_the_result() {
        let wheels_node = wheels_effect(&[("gain_red_thousandths", 1_200)]);
        let curves_node = curve_effect(
            ColorCurveChannel::Master,
            &[(0, 0), (5_000, 6_000), (10_000, 10_000)],
            0,
        );

        let wheels_first = resolve_color_nodes(&[wheels_node.clone(), curves_node.clone()])
            .expect("wheels then curves");
        let curves_first =
            resolve_color_nodes(&[curves_node, wheels_node]).expect("curves then wheels");
        assert_eq!(wheels_first.len(), 2);
        assert_eq!(curves_first.len(), 2);
        assert_eq!(wheels_first[0].kind(), ColorNodeKind::Wheels);
        assert_eq!(curves_first[0].kind(), ColorNodeKind::Curves);

        // wheels then curves, red: D(E(0.18)*1.2) = 0.250_770_15, then the
        // master curve on E of that gives D(...) = 0.355_061_1.
        let sample = [0.18_f32; 3];
        assert_close(apply_stack(&wheels_first, sample)[0], 0.355_061, 2e-5);
        // curves then wheels, red: the master curve gives 0.262_430_5, then
        // D(E(0.262_430_5) * 1.2) = 0.369_891_6.
        assert_close(apply_stack(&curves_first, sample)[0], 0.369_892, 2e-5);
        assert!(
            (apply_stack(&wheels_first, sample)[0] - apply_stack(&curves_first, sample)[0]).abs()
                > 1e-2
        );
    }

    #[test]
    fn inactive_nodes_are_skipped_bit_identically() {
        let sample = [0.18_f32, -0.05, 2.5];
        let baseline = apply_stack(&[], sample);
        assert_eq!(bits(baseline), bits(sample));

        let identity = [(0, 0), (10_000, 10_000)];
        let shaped = [(0, 0), (5_000, 6_000), (10_000, 10_000)];
        let inactive = [
            wheels_effect(&[]),
            wheels_effect(&[("gain_red_thousandths", 1_200), ("bypass", 1)]),
            curve_effect(ColorCurveChannel::Master, &identity, 0),
            curve_effect(ColorCurveChannel::Master, &shaped, 1),
        ];
        for effect in &inactive {
            let nodes = resolve_color_nodes(std::slice::from_ref(effect)).expect("inactive node");
            assert!(nodes.is_empty(), "{} was not skipped", effect.name);
            assert_eq!(bits(apply_stack(&nodes, sample)), bits(sample));
        }
    }

    #[test]
    fn resolve_color_nodes_rejects_more_than_the_per_layer_limit() {
        let effects: Vec<Effect> = (0..17)
            .map(|id| wheels_effect_with_id(id, &[("gain_red_thousandths", 1_200)]))
            .collect();
        assert_eq!(
            resolve_color_nodes(&effects),
            Err(ColorPipelineError::TooManyColorNodes {
                count: 17,
                limit: COLOR_NODE_LIMIT_PER_LAYER,
            })
        );
        assert_eq!(
            resolve_color_nodes(&effects[..COLOR_NODE_LIMIT_PER_LAYER])
                .expect("sixteen nodes are legal")
                .len(),
            COLOR_NODE_LIMIT_PER_LAYER
        );
    }

    #[test]
    fn apply_primary_corrections_agrees_with_the_node_stack() {
        let first = PrimaryCorrection {
            exposure_milli_stops: 500,
            saturation_percent: 20,
            ..PrimaryCorrection::default()
        };
        let second = PrimaryCorrection {
            contrast_percent: 15,
            ..PrimaryCorrection::default()
        };
        let sample = [0.18_f32, 0.35, 0.90];
        let nodes = [
            ColorNode::Primary(first, None),
            ColorNode::Primary(second, None),
        ];
        assert_eq!(
            bits(apply_primary_corrections(sample, &[first, second]).expect("valid controls")),
            bits(apply_stack(&nodes, sample))
        );
    }

    // -----------------------------------------------------------------------
    // CC4 §10.3.2--§10.3.6: LUT nodes.
    //
    // Every expected value below is transcribed from the CC4 contract text or
    // derived by hand from the §3.5 equations. No expectation is obtained by
    // calling `Lut3d::lookup`, `LutNode::apply`, or `apply_color_nodes_at`
    // (CC4 §10.1.1).
    // -----------------------------------------------------------------------

    /// The eight CC3 §10.2 channel patterns, verbatim.
    fn cc4_pattern_sample(pattern: usize, level: f32) -> [f32; 3] {
        match pattern {
            0 => [level, level, level],
            1 => [level, 0.0, 0.0],
            2 => [0.0, level, 0.0],
            3 => [0.0, 0.0, level],
            4 => [0.0, level, level],
            5 => [level, 0.0, level],
            6 => [level, level, 0.0],
            7 => [level, level / 2.0, level / 4.0],
            other => panic!("CC3 raster has eight patterns; asked for {other}"),
        }
    }

    /// The CC3 §10.2 raster CC4 §10.2 reuses verbatim: 24 levels x 8 patterns.
    fn cc4_raster() -> Vec<[f32; 3]> {
        let mut samples = Vec::with_capacity(192);
        for level in CC3_RASTER_LEVELS {
            for pattern in 0..8 {
                samples.push(cc4_pattern_sample(pattern, level));
            }
        }
        assert_eq!(samples.len(), 192);
        samples
    }

    /// A lattice built red-fastest from a closure over the integer indices.
    fn cube_from(
        size: u32,
        domain_min: [f32; 3],
        domain_max: [f32; 3],
        value: impl Fn(u32, u32, u32) -> [f32; 3],
    ) -> CubeLut {
        let mut rgba = Vec::with_capacity((size as usize).pow(3) * 4);
        for blue in 0..size {
            for green in 0..size {
                for red in 0..size {
                    let sample = value(red, green, blue);
                    rgba.extend_from_slice(&[sample[0], sample[1], sample[2], 1.0]);
                }
            }
        }
        CubeLut {
            size,
            domain_min,
            domain_max,
            rgba,
            title: None,
        }
    }

    /// CC4 §10.3.3 LUT B: `S = 2`, domain `[0, 1]`, the seven half-unit
    /// corners plus a white corner at `(1, 1, 1)`.
    fn lut_b() -> Lut3d {
        Lut3d::from_cube(&cube_from(2, [0.0; 3], [1.0; 3], |red, green, blue| {
            if (red, green, blue) == (1, 1, 1) {
                [1.0, 1.0, 1.0]
            } else {
                [red, green, blue].map(|index| 0.5 * f32::from(u8::try_from(index).unwrap()))
            }
        }))
    }

    /// CC4 §10.3.3 LUT C/D lattice: separable, `f = (0, 0.25, 1.0)`.
    fn lut_cd(domain_min: [f32; 3], domain_max: [f32; 3]) -> Lut3d {
        const F: [f32; 3] = [0.0, 0.25, 1.0];
        Lut3d::from_cube(&cube_from(3, domain_min, domain_max, |red, green, blue| {
            [F[red as usize], F[green as usize], F[blue as usize]]
        }))
    }

    /// An identity lattice at edge `size` over `[0, 1]`.
    fn identity_lut(size: u32) -> Lut3d {
        let last = f32::from(u16::try_from(size - 1).expect("edge fits"));
        Lut3d::from_cube(&cube_from(size, [0.0; 3], [1.0; 3], |red, green, blue| {
            [red, green, blue]
                .map(|index| f32::from(u16::try_from(index).expect("index fits")) / last)
        }))
    }

    fn lut_node(lut: Lut3d, encoding: LutInputEncoding, mix: f32) -> LutNode {
        LutNode::new(Arc::new(lut), encoding, mix)
    }

    fn lut_effect(id: u64, name: &str, asset: u64, extra: &[(&str, i64)]) -> Effect {
        let mut parameters = vec![("lut_asset_id".to_owned(), i64::try_from(asset).unwrap())];
        for (name, value) in extra {
            parameters.push(((*name).to_owned(), *value));
        }
        color_node_effect(id, name, parameters)
    }

    /// A library holding the two built-in bakes CC4 §2.6 pins, admitted
    /// through the production `LutLibrary::build` path with no store.
    fn builtin_library() -> LutLibrary {
        let assets = [
            crate::builtin_looks::BuiltinLook::Warm.to_lut_asset(LutAssetId(1)),
            crate::builtin_looks::BuiltinLook::Cool.to_lut_asset(LutAssetId(2)),
        ];
        let (library, statuses) = LutLibrary::build(&assets, None);
        for (id, status) in statuses {
            assert_eq!(
                status.kind,
                kinewright_core::LutAvailabilityKind::Verified,
                "built-in asset {id:?} must verify from the pinned bake"
            );
        }
        assert_eq!(library.len(), 2);
        library
    }

    // --- §3.4 -------------------------------------------------------------

    #[test]
    fn decode_display709_is_the_exact_inverse_of_encode_bt709() {
        // CC4 §3.4: `decode_display709(encode_bt709(x)) = x` for every finite
        // `x` in exact arithmetic, including the negatives and the over-range
        // levels CC3 §10.2 carries and where `decode_bt709` cannot.
        let mut worst = 0.0_f64;
        for level in CC3_RASTER_LEVELS {
            for x in [level, -level] {
                let round_trip = decode_display709(encode_bt709(x));
                let tolerance = 1e-6_f32 * 1.0_f32.max(x.abs());
                assert!(
                    (round_trip - x).abs() <= tolerance,
                    "decode_display709(encode_bt709({x})) = {round_trip}"
                );
                worst = worst.max(f64::from((round_trip - x).abs()));
            }
        }
        // Recorded, not gated: the measured f32 `powf` round-trip floor.
        assert!(worst < 1e-6, "observed worst round-trip deviation {worst}");

        // `sgn(0) = 0`, so zero is exact rather than merely close, and the
        // sign is preserved rather than taken from `f32::signum`.
        assert_eq!(decode_display709(0.0).to_bits(), 0.0_f32.to_bits());
        assert_eq!(
            decode_display709(encode_bt709(0.0)).to_bits(),
            0.0_f32.to_bits()
        );
        assert_eq!(
            decode_display709(-0.45).to_bits(),
            (-decode_display709(0.45)).to_bits()
        );

        // This is exactly why CC4 needs its own decode: CC1's source decode
        // takes the linear branch unconditionally for a negative argument.
        assert!((decode_bt709(encode_bt709(-0.25)) + 0.25).abs() > 0.1);
        assert!((decode_display709(-0.5) - decode_bt709(-0.5)).abs() > 0.1);
        // ... and it is untouched: `decode_bt709(-0.5) = -0.5 / 4.5`.
        assert_close(decode_bt709(-0.5), -0.5 / 4.5, 1e-7);
    }

    #[test]
    fn decode_display709_crosses_the_0_018_seam_consistently() {
        // CC1's encode branches at linear 0.018; CC4's decode branches at the
        // encoded 0.081. `0.018` itself takes the *power* branch of the
        // encode (the test is `linear < 0.018`), so its encoded value must
        // land at or above 0.081 and decode through the power branch.
        let encoded = encode_bt709(0.018);
        assert!(
            encoded >= 0.081,
            "encode_bt709(0.018) = {encoded} must take the power branch"
        );
        assert_close(decode_display709(encoded), 0.018, 1e-6);

        // The f32 immediately below 0.018 takes the linear branch and must
        // encode strictly below 0.081, so no encoded value can fall into the
        // gap and be decoded through the wrong branch.
        let below = f32::from_bits(0.018_f32.to_bits() - 1);
        let encoded_below = encode_bt709(below);
        assert!(
            encoded_below < 0.081,
            "encode_bt709({below}) = {encoded_below} must take the linear branch"
        );
        assert_close(decode_display709(encoded_below), below, 1e-9);
        assert!(encoded_below < encoded, "the encode stays increasing");
    }

    #[test]
    fn lut_input_encodings_map_tokens_and_round_trip() {
        assert_eq!(
            LutInputEncoding::from_token(0),
            Some(LutInputEncoding::Display709)
        );
        assert_eq!(
            LutInputEncoding::from_token(1),
            Some(LutInputEncoding::Linear)
        );
        assert_eq!(
            LutInputEncoding::from_token(2),
            Some(LutInputEncoding::Grade709)
        );
        assert_eq!(LutInputEncoding::from_token(3), None);
        assert_eq!(LutInputEncoding::from_token(-1), None);
        for encoding in LutInputEncoding::ALL {
            assert_eq!(
                LutInputEncoding::from_token(encoding.token()),
                Some(encoding)
            );
        }
        assert_eq!(LutInputEncoding::Display709.as_str(), "display709");
        assert_eq!(LutInputEncoding::Linear.as_str(), "linear");
        assert_eq!(LutInputEncoding::Grade709.as_str(), "grade709");

        // `linear` is the identity in both directions, bit for bit.
        for value in [-2.5_f32, 0.0, 0.18, 4.0] {
            assert_eq!(
                LutInputEncoding::Linear.encode(value).to_bits(),
                value.to_bits()
            );
            assert_eq!(
                LutInputEncoding::Linear.decode(value).to_bits(),
                value.to_bits()
            );
        }
        assert_eq!(
            LutInputEncoding::Display709.encode(0.18).to_bits(),
            encode_bt709(0.18).to_bits()
        );
        assert_eq!(
            LutInputEncoding::Display709.decode(0.5).to_bits(),
            decode_display709(0.5).to_bits()
        );
        assert_eq!(
            LutInputEncoding::Grade709.encode(0.18).to_bits(),
            grade709_encode(0.18).to_bits()
        );
        assert_eq!(
            LutInputEncoding::Grade709.decode(0.5).to_bits(),
            grade709_decode(0.5).to_bits()
        );
    }

    // --- §10.3.3 interpolation anchors ------------------------------------

    #[test]
    fn lut_b_reproduces_the_three_tetrahedral_anchors() {
        let lut = lut_b();
        // Row 1, branch `f_r > f_g > f_b`:
        //   c000 + 0.75*(c100-c000) + 0.50*(c110-c100) + 0.25*(c111-c110)
        // = 0.75*(.5,0,0) + 0.5*(0,.5,0) + 0.25*(.5,.5,1)
        // = (0.500000, 0.375000, 0.250000)
        assert_eq!(
            bits(lut.lookup([0.75, 0.50, 0.25])),
            bits([0.500_000, 0.375_000, 0.250_000])
        );
        // Row 2, `f_r <= f_g` then `f_b > f_g`:
        //   c000 + 0.25*(c111-c011) + 0.50*(c011-c001) + 0.75*(c001-c000)
        // = 0.25*(1,.5,.5) + 0.5*(0,.5,0) + 0.75*(0,0,.5)
        // = (0.250000, 0.375000, 0.500000)
        assert_eq!(
            bits(lut.lookup([0.25, 0.50, 0.75])),
            bits([0.250_000, 0.375_000, 0.500_000])
        );
        // Row 3, the three-way tie falling to the final `else`:
        //   c000 + 0.5*(c110-c010) + 0.5*(c010-c000) + 0.5*(c111-c110)
        // = 0.5*(.5,0,0) + 0.5*(0,.5,0) + 0.5*(.5,.5,1)
        // = (0.500000, 0.500000, 0.500000)
        assert_eq!(
            bits(lut.lookup([0.50, 0.50, 0.50])),
            bits([0.500_000, 0.500_000, 0.500_000])
        );
    }

    #[test]
    fn the_first_anchor_differs_from_trilinear_interpolation() {
        // CC4 §10.3.3: trilinear interpolation of the same lattice at the same
        // point is (0.421875, 0.296875, 0.171875) -- proving the production
        // evaluator is actually tetrahedral rather than a renamed trilinear.
        let lut = lut_b();
        assert_eq!(
            bits(lut.trilinear_lookup([0.75, 0.50, 0.25])),
            bits([0.421_875, 0.296_875, 0.171_875])
        );
        assert_ne!(
            bits(lut.lookup([0.75, 0.50, 0.25])),
            bits(lut.trilinear_lookup([0.75, 0.50, 0.25]))
        );
        // The two agree on a lattice point, as both must.
        assert_eq!(
            bits(lut.lookup([1.0, 1.0, 1.0])),
            bits(lut.trilinear_lookup([1.0, 1.0, 1.0]))
        );
    }

    #[test]
    fn the_tie_case_agrees_through_all_six_branch_formulas() {
        // CC4 §3.5 states the six formulas agree analytically on the shared
        // faces, so a tie is well defined. At `f = (0.5, 0.5, 0.5)` every
        // branch condition is false, so this transcribes all six directly
        // from the contract and asserts they agree with each other and with
        // the production evaluator.
        let lut = lut_b();
        let corner = |r: u32, g: u32, b: u32| lut.lattice(r, g, b);
        let (c000, c001, c010, c011) = (
            corner(0, 0, 0),
            corner(0, 0, 1),
            corner(0, 1, 0),
            corner(0, 1, 1),
        );
        let (c100, c101, c110, c111) = (
            corner(1, 0, 0),
            corner(1, 0, 1),
            corner(1, 1, 0),
            corner(1, 1, 1),
        );
        let (f_r, f_g, f_b) = (0.5_f32, 0.5_f32, 0.5_f32);
        let combine =
            |a1: [f32; 3], a0: [f32; 3], b1: [f32; 3], b0: [f32; 3], d1: [f32; 3], d0: [f32; 3]| {
                let mut out = [0.0_f32; 3];
                for channel in 0..3 {
                    out[channel] = c000[channel]
                        + f_r * (a1[channel] - a0[channel])
                        + f_g * (b1[channel] - b0[channel])
                        + f_b * (d1[channel] - d0[channel]);
                }
                out
            };
        let branches = [
            combine(c100, c000, c110, c100, c111, c110),
            combine(c100, c000, c111, c101, c101, c100),
            combine(c101, c001, c111, c101, c001, c000),
            combine(c111, c011, c011, c001, c001, c000),
            combine(c111, c011, c010, c000, c011, c010),
            combine(c110, c010, c010, c000, c111, c110),
        ];
        for (index, branch) in branches.iter().enumerate() {
            assert_eq!(
                bits(*branch),
                bits([0.5, 0.5, 0.5]),
                "branch {index} must agree at the tie"
            );
        }
        assert_eq!(bits(lut.lookup([0.5, 0.5, 0.5])), bits(branches[5]));
    }

    #[test]
    fn lut_c_reproduces_the_separable_anchor() {
        // CC4 §10.3.3 LUT C, `S = 3`, domain [0, 1], f = (0, 0.25, 1.0).
        // s = (1.5, 0.5, 1.0); i = (1, 0, 1); f = (0.5, 0.5, 0.0) -> the tie
        // falls to the final `else`, giving (0.625, 0.125, 0.25).
        let lut = lut_cd([0.0; 3], [1.0; 3]);
        assert_eq!(
            bits(lut.lookup([0.75, 0.25, 0.50])),
            bits([0.625_000, 0.125_000, 0.250_000])
        );
    }

    #[test]
    fn lut_d_reproduces_the_domain_mapping_anchor() {
        // CC4 §10.3.3 LUT D: the LUT C lattice over DOMAIN [-0.5, 1.5].
        // t = (u + 0.5) / 2 = (0.5, 0.25, 0.75); s = (1.0, 0.5, 1.5);
        // i = (1, 0, 1); f = (0.0, 0.5, 0.5) -> the `f_b > f_r` branch.
        let lut = lut_cd([-0.5; 3], [1.5; 3]);
        assert_eq!(
            bits(lut.lookup([0.50, 0.00, 1.00])),
            bits([0.250_000, 0.125_000, 0.625_000])
        );
    }

    // --- §10.3.4 out of domain --------------------------------------------

    #[test]
    fn out_of_domain_excursions_are_restored_additively_not_clamped() {
        let lut = lut_cd([-0.5; 3], [1.5; 3]);

        // e = (2, 2, 2) clamps to (1.5, 1.5, 1.5), whose lookup is (1, 1, 1);
        // the 0.5 excursion above dmax is added back on top of the boundary
        // value, so the node output is (1.5, 1.5, 1.5).
        assert_eq!(bits(lut.lookup([2.0; 3])), bits([1.5, 1.5, 1.5]));
        // e = (-1, -1, -1) clamps to (-0.5, -0.5, -0.5), lookup (0, 0, 0),
        // output (-0.5, -0.5, -0.5).
        assert_eq!(bits(lut.lookup([-1.0; 3])), bits([-0.5, -0.5, -0.5]));

        // A pure-clamp implementation would return the boundary lookups
        // (1, 1, 1) and (0, 0, 0). Asserting the difference is what stops the
        // additive rule regressing silently.
        let clamp_only_high = lut.lookup([1.5; 3]);
        let clamp_only_low = lut.lookup([-0.5; 3]);
        assert_eq!(bits(clamp_only_high), bits([1.0, 1.0, 1.0]));
        assert_eq!(bits(clamp_only_low), bits([0.0, 0.0, 0.0]));
        assert_ne!(bits(lut.lookup([2.0; 3])), bits(clamp_only_high));
        assert_ne!(bits(lut.lookup([-1.0; 3])), bits(clamp_only_low));

        // Ordering above the domain boundary survives, which is the whole
        // point: a pure clamp would collapse both to (1, 1, 1).
        assert!(lut.lookup([2.5; 3])[0] > lut.lookup([2.0; 3])[0]);
    }

    // --- §10.3.5 mix -------------------------------------------------------

    #[test]
    fn mix_blends_in_linear_light_at_both_endpoints_and_the_midpoint() {
        // CC4 §10.3.5 with LUT B, `input_encoding = linear`,
        // x = (0.75, 0.50, 0.25), look(x) = (0.5, 0.375, 0.25).
        let sample = [0.75_f32, 0.50, 0.25];
        let neutral = lut_node(lut_b(), LutInputEncoding::Linear, 0.0);
        assert_eq!(bits(neutral.apply(sample)), bits(sample));

        let half = lut_node(lut_b(), LutInputEncoding::Linear, 0.5);
        assert_eq!(
            bits(half.apply(sample)),
            bits([0.625_000, 0.437_500, 0.250_000])
        );

        let full = lut_node(lut_b(), LutInputEncoding::Linear, 1.0);
        assert_eq!(
            bits(full.apply(sample)),
            bits([0.500_000, 0.375_000, 0.250_000])
        );

        // The basis-point ladder Core resolves must reach exactly those
        // factors: 0, 5000, and 10000.
        for (basis_points, expected) in [(0_i64, 0.0_f32), (5_000, 0.5), (10_000, 1.0)] {
            let params = LutNodeParams {
                lut_asset_id: LutAssetId(1),
                mix_basis_points: basis_points,
                input_encoding_token: 0,
                bypass_token: 0,
            };
            assert_eq!(params.mix().to_bits(), expected.to_bits());
        }
    }

    // --- §10.3.2 identity --------------------------------------------------

    #[test]
    fn identity_lattices_are_bit_exact_in_linear_at_dyadic_sizes() {
        // CC4 §3.5: with `input_encoding = linear`, domain [0, 1], and
        // `S - 1` a power of two, every lattice coordinate, fraction, and
        // interpolation weight is an exact binary fraction.
        for size in [2_u32, 17, 33, 65] {
            let node = lut_node(identity_lut(size), LutInputEncoding::Linear, 1.0);
            for sample in cc4_raster() {
                if sample.iter().any(|value| *value < 0.0 || *value > 1.0) {
                    continue;
                }
                assert_eq!(
                    bits(node.apply(sample)),
                    bits(sample),
                    "S = {size} must be bit-exact at {sample:?}"
                );
            }
        }
    }

    #[test]
    fn a_non_dyadic_identity_lattice_is_not_bit_exact() {
        // The counter-check for the claim above: `S = 64` gives `S - 1 = 63`,
        // so `1/63` is not a binary fraction and the reproduction is only
        // close, not exact. This is why CC4 §3.5 names {2, 17, 33, 65}
        // explicitly rather than "any size".
        let node = lut_node(identity_lut(64), LutInputEncoding::Linear, 1.0);
        let mut differing = 0_usize;
        let mut worst = 0.0_f32;
        for sample in cc4_raster() {
            if sample.iter().any(|value| *value < 0.0 || *value > 1.0) {
                continue;
            }
            let observed = node.apply(sample);
            if bits(observed) != bits(sample) {
                differing += 1;
            }
            for channel in 0..3 {
                worst = worst.max((observed[channel] - sample[channel]).abs());
            }
        }
        assert!(
            differing > 0,
            "S = 64 must not be bit-exact; the dyadic claim would be vacuous"
        );
        assert!(
            worst <= 1.5e-3,
            "S = 64 stays inside the linear gate: {worst}"
        );
    }

    #[test]
    fn identity_lattices_are_tolerance_exact_in_display709_and_grade709() {
        // CC4 §3.5: for `display709` / `grade709` the f32 `pow` round trip is
        // not bit-exact even though the pair is an exact analytic bijection,
        // so the gate is the CC1 §6.2 linear maximum, never bit equality.
        const LINEAR_MAX: f32 = 1.5e-3;
        for encoding in [LutInputEncoding::Display709, LutInputEncoding::Grade709] {
            let mut worst = 0.0_f32;
            let mut bit_exact = true;
            for size in [2_u32, 17, 33, 65] {
                let node = lut_node(identity_lut(size), encoding, 1.0);
                for sample in cc4_raster() {
                    if sample.iter().any(|value| *value < 0.0 || *value > 1.0) {
                        continue;
                    }
                    let observed = node.apply(sample);
                    if bits(observed) != bits(sample) {
                        bit_exact = false;
                    }
                    for channel in 0..3 {
                        worst = worst.max((observed[channel] - sample[channel]).abs());
                    }
                }
            }
            assert!(
                worst <= LINEAR_MAX,
                "{} identity deviation {worst} exceeds {LINEAR_MAX}",
                encoding.as_str()
            );
            assert!(
                !bit_exact,
                "{} is not claimed bit-exact; if it were, the separate gate would be dead code",
                encoding.as_str()
            );
        }
    }

    // --- §10.3.10 built-in bakes -------------------------------------------

    #[test]
    fn builtin_bakes_reproduce_their_closed_form_on_the_raster() {
        // CC4 §2.6/§10.3.10: all five formulas are affine in `e`, and
        // tetrahedral interpolation reproduces an affine function exactly on
        // every simplex, so the node output must match the closed form to
        // within 2e-6 in display code. The expectation is computed in f64
        // from `BuiltinLook::formula`, which is the contract's closed form,
        // not the code under test.
        const DISPLAY_CODE_MAX: f64 = 2e-6;
        for look in crate::builtin_looks::BuiltinLook::ALL {
            let node = LutNode::new(
                Arc::new(Lut3d::from_shared(look.cached_bake())),
                LutInputEncoding::Display709,
                1.0,
            );
            let mut worst = 0.0_f64;
            let mut outside_unit = 0_usize;
            for sample in cc4_raster() {
                let encoded = sample.map(encode_bt709);
                if encoded.iter().any(|value| *value < 0.0 || *value > 1.0) {
                    outside_unit += 1;
                }
                let expected = look.formula(encoded.map(f64::from));
                let observed = node.apply(sample).map(encode_bt709);
                for channel in 0..3 {
                    let difference = (f64::from(observed[channel]) - expected[channel]).abs();
                    assert!(
                        difference <= DISPLAY_CODE_MAX,
                        "{} channel {channel} at {sample:?}: display code {} vs {}",
                        look.name(),
                        observed[channel],
                        expected[channel]
                    );
                    worst = worst.max(difference);
                }
            }
            assert!(worst.is_finite(), "{}", look.name());
            // CC4 §10.2: the raster is non-vacuous for the out-of-domain rule.
            assert!(
                outside_unit >= 40,
                "only {outside_unit} raster samples encode outside [0, 1]"
            );
        }
    }

    // --- §3.6 inactive nodes and §4.4 resolution ---------------------------

    #[test]
    fn lut_node_resolution_reports_a_missing_asset_and_a_bad_encoding() {
        let library = builtin_library();
        let bound = LutNodeParams {
            lut_asset_id: LutAssetId(1),
            mix_basis_points: 10_000,
            input_encoding_token: 0,
            bypass_token: 0,
        };
        let node = LutNode::from_params(&bound, &library).expect("asset 1 is verified");
        assert_eq!(node.encoding(), LutInputEncoding::Display709);
        assert_eq!(node.mix().to_bits(), 1.0_f32.to_bits());
        assert_eq!(node.lut().size(), 17);
        assert_eq!(bits(node.lut().domain_min()), bits([-1.0; 3]));
        assert_eq!(bits(node.lut().domain_max()), bits([2.0; 3]));

        let dangling = LutNodeParams {
            lut_asset_id: LutAssetId(9),
            ..bound
        };
        let error = LutNode::from_params(&dangling, &library).expect_err("asset 9 is absent");
        assert_eq!(error, ColorPipelineError::MissingLutAsset(LutAssetId(9)));
        assert!(error.to_string().starts_with("missing_lut_asset: "));

        let unbound = LutNodeParams {
            lut_asset_id: LutAssetId(0),
            ..bound
        };
        assert_eq!(
            LutNode::from_params(&unbound, &library).expect_err("id 0 is unbound"),
            ColorPipelineError::MissingLutAsset(LutAssetId(0))
        );

        let bad_encoding = LutNodeParams {
            input_encoding_token: 3,
            ..bound
        };
        let error =
            LutNode::from_params(&bad_encoding, &library).expect_err("token 3 is not an encoding");
        assert_eq!(error, ColorPipelineError::UnsupportedInputEncoding(3));
        assert!(error.to_string().contains("observed 3"));
        assert!(error.to_string().contains("allowed 0 display709"));
    }

    #[test]
    fn inactive_lut_nodes_are_skipped_bit_identically() {
        // CC4 §3.6: bypass, `mix = 0`, and an unbound reference are each the
        // exact identity, tested on the stored integers, so each is losslessly
        // identical to removing the node.
        let library = builtin_library();
        let sample = [0.18_f32, -0.05, 2.5];
        let wheels = wheels_effect_with_id(9, &[("gain_master_thousandths", 1_200)]);
        let baseline = resolve_color_nodes_with(std::slice::from_ref(&wheels), &library)
            .expect("wheels alone");
        let expected = apply_stack(&baseline, sample);
        assert_eq!(baseline.len(), 1);

        // The same asset with an active node really does change the frame, so
        // the identity claims below are not vacuous.
        let active = lut_effect(1, "creative_look", 1, &[]);
        let with_look =
            resolve_color_nodes_with(&[wheels.clone(), active], &library).expect("look resolves");
        assert_eq!(with_look.len(), 2);
        assert_eq!(with_look[1].kind(), ColorNodeKind::CreativeLook);
        assert_ne!(bits(apply_stack(&with_look, sample)), bits(expected));

        for (label, effect) in [
            (
                "bypassed",
                lut_effect(1, "creative_look", 1, &[("bypass", 1)]),
            ),
            (
                "neutral",
                lut_effect(1, "creative_look", 1, &[("mix_basis_points", 0)]),
            ),
            ("unbound", lut_effect(1, "creative_look", 0, &[])),
        ] {
            let params = LutNodeParams::from_effect(&effect);
            assert!(!params.is_active(), "{label} must resolve inactive");
            let nodes = resolve_color_nodes_with(&[wheels.clone(), effect], &library)
                .expect("an inactive LUT node never needs its asset");
            assert_eq!(nodes.len(), 1, "{label} must be dropped entirely");
            assert_eq!(
                bits(apply_stack(&nodes, sample)),
                bits(expected),
                "{label} must be bit-identical to the node removed"
            );
        }

        // The unbound reason is the CC4 §3.6 token, constructed below the
        // operation layer because §3.3 makes it unreachable through Core.
        let unbound = LutNodeParams {
            lut_asset_id: LutAssetId(0),
            mix_basis_points: 10_000,
            input_encoding_token: 0,
            bypass_token: 0,
        };
        assert_eq!(
            unbound.inactive_reason(),
            Some(kinewright_core::ColorNodeInactiveReason::Unbound)
        );
    }

    #[test]
    fn the_legacy_resolver_fails_closed_on_an_active_lut_node() {
        // CC4 §4.4: the old symbol is kept as a wrapper over an *empty*
        // library, so no caller silently renders a look-free frame.
        let technical = lut_effect(1, "technical_lut", 1, &[]);
        assert_eq!(
            resolve_color_nodes(std::slice::from_ref(&technical)),
            Err(ColorPipelineError::MissingLutAsset(LutAssetId(1)))
        );
        let creative = lut_effect(2, "creative_look", 4, &[]);
        assert_eq!(
            resolve_color_nodes(std::slice::from_ref(&creative)),
            Err(ColorPipelineError::MissingLutAsset(LutAssetId(4)))
        );

        // An inactive LUT node needs no asset, so the wrapper still resolves.
        let bypassed = lut_effect(3, "creative_look", 1, &[("bypass", 1)]);
        assert_eq!(
            resolve_color_nodes(&[bypassed])
                .expect("a bypassed node needs no asset")
                .len(),
            0
        );
        // And the CC3 stack keeps working through the wrapper unchanged.
        let wheels = wheels_effect_with_id(4, &[("gain_master_thousandths", 1_200)]);
        assert_eq!(
            resolve_color_nodes(&[wheels]).expect("wheels alone").len(),
            1
        );
    }

    #[test]
    fn the_stage_order_of_the_five_kind_stack_is_the_execution_order() {
        // CC4 §10.3.6: the CPU reference evaluates the vector order, so the
        // stage-ordered stack and the reversed one differ. The reversed stack
        // cannot be stored -- Core rejects it with ColorStageOrderViolation --
        // so it is asserted directly against the reference here.
        let library = builtin_library();
        let ordered = vec![
            lut_effect(1, "technical_lut", 1, &[]),
            primary_effect(BTreeMap::from([(
                "exposure_milli_stops".to_owned(),
                ParamValue::Integer(500),
            )])),
            wheels_effect_with_id(3, &[("gain_master_thousandths", 1_200)]),
            curve_effect(
                ColorCurveChannel::Master,
                &[(0, 0), (5_000, 6_000), (10_000, 10_000)],
                0,
            ),
            lut_effect(5, "creative_look", 2, &[]),
        ];
        let stack = resolve_color_nodes_with(&ordered, &library).expect("stage-ordered stack");
        assert_eq!(
            stack.iter().map(ColorNode::kind).collect::<Vec<_>>(),
            vec![
                ColorNodeKind::TechnicalLut,
                ColorNodeKind::Primary,
                ColorNodeKind::Wheels,
                ColorNodeKind::Curves,
                ColorNodeKind::CreativeLook,
            ]
        );

        let sample = [0.18_f32, 0.35, 0.62];
        let forward = apply_stack(&stack, sample);

        let mut reversed = ordered.clone();
        reversed.reverse();
        let reversed_stack =
            resolve_color_nodes_with(&reversed, &library).expect("reversed stack resolves");
        assert_eq!(reversed_stack[0].kind(), ColorNodeKind::CreativeLook);
        assert_eq!(reversed_stack[4].kind(), ColorNodeKind::TechnicalLut);
        let backward = apply_stack(&reversed_stack, sample);

        assert_ne!(bits(forward), bits(backward));
        let separation = (0..3)
            .map(|channel| (forward[channel] - backward[channel]).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            separation > 1e-3,
            "vector order must matter visibly: forward {forward:?} backward {backward:?}"
        );

        // The two-node §10.3.6 case, recorded with its values.
        let two = vec![
            lut_effect(1, "technical_lut", 1, &[]),
            lut_effect(2, "creative_look", 2, &[]),
        ];
        let mut two_reversed = two.clone();
        two_reversed.reverse();
        let technical_first =
            apply_stack(&resolve_color_nodes_with(&two, &library).unwrap(), sample);
        let look_first = apply_stack(
            &resolve_color_nodes_with(&two_reversed, &library).unwrap(),
            sample,
        );
        assert_ne!(bits(technical_first), bits(look_first));
        println!(
            "CC4_STAGE_ORDER sample={sample:?} technical_first={technical_first:?} look_first={look_first:?}"
        );
    }

    #[test]
    fn color_node_kinds_cover_every_managed_name() {
        // The resolved-node dispatch must stay total as Core grows the stack.
        assert_eq!(
            kinewright_core::MANAGED_COLOR_NODE_NAMES,
            [
                "technical_lut",
                "primary_correction",
                "color_wheels",
                "color_curves",
                "creative_look"
            ]
        );
        let library = builtin_library();
        let node = LutNode::from_params(
            &LutNodeParams::from_effect(&lut_effect(1, "technical_lut", 1, &[])),
            &library,
        )
        .expect("technical node");
        assert_eq!(
            ColorNode::TechnicalLut(node.clone()).kind(),
            ColorNodeKind::TechnicalLut
        );
        assert_eq!(
            ColorNode::CreativeLook(node.clone(), None).kind(),
            ColorNodeKind::CreativeLook
        );
        assert!(ColorNode::TechnicalLut(node.clone()).lut_node().is_some());
        assert!(ColorNode::CreativeLook(node, None).lut_node().is_some());
        assert!(
            ColorNode::Primary(PrimaryCorrection::default(), None)
                .lut_node()
                .is_none()
        );
    }

    // -----------------------------------------------------------------------
    // CC5 §2.3--§2.5: the matte.
    //
    // Every expected value below is derived by hand from the CC5 contract
    // equations, or transcribed independently in f64 here.  No expectation is
    // obtained by calling `Matte::coverage`, `MatteWindow::weight`,
    // `MatteQualifier::weight`, `apply_color_nodes_at`, or the compositor
    // (CC5 §9.0.1).
    // -----------------------------------------------------------------------

    /// The CC5 §9.1 raster: 64 x 36, so `a = 16/9` and there are 2304 pixels.
    const MATTE_RASTER_WIDTH: usize = 64;
    /// The CC5 §9.1 raster height.
    const MATTE_RASTER_HEIGHT: usize = 36;
    /// The CC5 §9.1 raster aspect `a = W / H`.
    const MATTE_RASTER_ASPECT: f32 = 64.0 / 36.0;
    /// A neutral mid-grey sample: every window-only case is independent of it.
    const MATTE_NEUTRAL_SAMPLE: [f32; 3] = [0.18; 3];

    /// The CC5 §3.4 pixel centre of raster cell `(x, y)`.
    #[allow(clippy::cast_precision_loss)]
    fn matte_uv(x: usize, y: usize) -> [f32; 2] {
        [
            (x as f32 + 0.5) / MATTE_RASTER_WIDTH as f32,
            (y as f32 + 0.5) / MATTE_RASTER_HEIGHT as f32,
        ]
    }

    /// A `color_wheels` effect carrying `matte_enabled = 1` plus `parameters`.
    fn matte_effect(parameters: &[(&str, i64)]) -> Effect {
        let mut stored = vec![("matte_enabled".to_owned(), 1_i64)];
        stored.extend(
            parameters
                .iter()
                .map(|(name, value)| ((*name).to_owned(), *value)),
        );
        color_node_effect(9, "color_wheels", stored)
    }

    /// The resolved matte of an enabled `color_wheels` matte block.
    fn matte(parameters: &[(&str, i64)]) -> Matte {
        Matte::from_params(&MatteParams::from_effect(&matte_effect(parameters)))
            .expect("an enabled, non-neutral matte must resolve")
    }

    /// One rect or ellipse window, in CC5 §2.2 stored integers.
    fn window_parameters(
        shape_token: i64,
        centre: (i64, i64),
        half: (i64, i64),
        rotation_cd: i64,
        feather_bp: i64,
    ) -> Vec<(&'static str, i64)> {
        vec![
            ("matte_window_count", 1),
            ("matte_window0_shape_token", shape_token),
            ("matte_window0_center_x_basis_points", centre.0),
            ("matte_window0_center_y_basis_points", centre.1),
            ("matte_window0_half_width_basis_points", half.0),
            ("matte_window0_half_height_basis_points", half.1),
            ("matte_window0_rotation_centidegrees", rotation_cd),
            ("matte_window0_feather_basis_points", feather_bp),
        ]
    }

    /// Every raster cell whose coverage is non-zero, in raster order.
    fn covered_cells(matte: &Matte) -> Vec<(usize, usize)> {
        let mut covered = Vec::new();
        for y in 0..MATTE_RASTER_HEIGHT {
            for x in 0..MATTE_RASTER_WIDTH {
                if matte.coverage(matte_uv(x, y), MATTE_RASTER_ASPECT, MATTE_NEUTRAL_SAMPLE) > 0.0 {
                    covered.push((x, y));
                }
            }
        }
        covered
    }

    #[test]
    #[allow(clippy::cast_possible_wrap)]
    fn matte_window_geometry_matches_the_hand_derived_pixel_counts() {
        // §9.2.2, anchor 1.  A centred rect with `hw = hh = 0.25` selects
        // `|u.x - 0.5| <= 0.25` and `|u.y - 0.5| <= 0.25` — the aspect factor
        // cancels exactly at theta = 0 (§2.3).  With pixel centres at
        // `(x + 0.5)/64` and `(y + 0.5)/36` that is `16 <= x <= 47` and
        // `9 <= y <= 26`: 32 x 18 = 576 of 2304 pixels, exactly 2500 basis
        // points.  No pixel centre is on the boundary: the tightest margins
        // are |n.x| = 0.96875 / 1.03125 at x = 16 / 15 and |n.y| = 0.94444 /
        // 1.05555 at y = 9 / 8.
        let centred = matte(&window_parameters(1, (5_000, 5_000), (2_500, 2_500), 0, 0));
        let mut inside = 0_usize;
        for y in 0..MATTE_RASTER_HEIGHT {
            for x in 0..MATTE_RASTER_WIDTH {
                let coverage =
                    centred.coverage(matte_uv(x, y), MATTE_RASTER_ASPECT, MATTE_NEUTRAL_SAMPLE);
                let expected = (16..=47).contains(&x) && (9..=26).contains(&y);
                // `feather = 0` takes the hard branch, so the weight is
                // exactly 1 or exactly 0 — never an epsilon (§2.3).
                assert_eq!(
                    coverage.to_bits(),
                    if expected { 1.0_f32 } else { 0.0_f32 }.to_bits(),
                    "centred rect at ({x}, {y}) resolved to {coverage}"
                );
                inside += usize::from(expected);
            }
        }
        assert_eq!(inside, 576);
        assert_eq!(
            MATTE_RASTER_WIDTH * MATTE_RASTER_HEIGHT - inside,
            1_728,
            "the outside set is the containment gate's population"
        );

        // §9.2.2, anchor 2.  `hw = 0.1125`, `hh = 0.2`: `hw * a = hh = 0.2`,
        // so the field is 7.2 pixels in every direction.  Pixel offsets from
        // the raster centre are `dx = x - 31.5`, `dy = y - 17.5`, both
        // half-integers, so `|d| <= 7.2` is `|d| <= 6.5`: 14 columns
        // (25..=38) x 14 rows (11..=24) = 196.
        let square = matte(&window_parameters(1, (5_000, 5_000), (1_125, 2_000), 0, 0));
        let unrotated = covered_cells(&square);
        assert_eq!(unrotated.len(), 196);
        assert!(
            unrotated
                .iter()
                .all(|(x, y)| (25..=38).contains(x) && (11..=24).contains(y)),
            "the unrotated pixel square is columns 25..=38 by rows 11..=24"
        );

        // §9.2.2, anchor 3 — **the aspect gate**.  Rotated 45 degrees the
        // condition is `|dx + dy| <= 7.2 * sqrt(2) = 10.18234` and
        // `|dy - dx| <= 10.18234`; both are integers (`x + y - 49` and
        // `y - x + 14`), so it is `<= 10`, and their sum `2 * dy` is odd, so
        // exactly one of them is odd.  Counting gives 220.
        let rotated = matte(&window_parameters(
            1,
            (5_000, 5_000),
            (1_125, 2_000),
            4_500,
            0,
        ));
        let rotated_cells = covered_cells(&rotated);
        assert_eq!(rotated_cells.len(), 220);
        for &(x, y) in &rotated_cells {
            let sum = x as i64 + y as i64 - 49;
            let difference = y as i64 - x as i64 + 14;
            assert!(
                sum.abs() <= 10 && difference.abs() <= 10 && (sum + difference) % 2 != 0,
                "({x}, {y}) is outside the hand-derived 45 degree set"
            );
        }
        // The covered set is symmetric under `(dx, dy) -> (dy, dx)`, which is
        // true only when the aspect correction is applied: without it a 45
        // degree rotation on a 16:9 raster shears the window (§2.3).
        let offsets: std::collections::BTreeSet<(i64, i64)> = rotated_cells
            .iter()
            .map(|&(x, y)| (2 * x as i64 - 63, 2 * y as i64 - 35))
            .collect();
        let swapped: std::collections::BTreeSet<(i64, i64)> =
            offsets.iter().map(|&(dx, dy)| (dy, dx)).collect();
        assert_eq!(offsets, swapped, "the 45 degree set must be swap-symmetric");

        // §9.2.2, anchor 4.  The same half-extents as an ellipse:
        // `dx^2 + dy^2 <= 51.84`.  Per quadrant the row counts are
        // 7, 7, 7, 6, 6, 5, 3 = 41, so 4 * 41 = 164, and the bounding box is
        // 14 x 14.  `(2i+1)^2 + (2j+1)^2 = 207.36` has no integer solution, so
        // no pixel centre is on the boundary; the tightest interior margin is
        // 1.34 px^2 at (+-5.5, +-4.5) and the tightest exterior margin is
        // 2.66 px^2 at (+-3.5, +-6.5).
        let ellipse = matte(&window_parameters(2, (5_000, 5_000), (1_125, 2_000), 0, 0));
        let ellipse_cells = covered_cells(&ellipse);
        assert_eq!(ellipse_cells.len(), 164);
        let columns: std::collections::BTreeSet<usize> =
            ellipse_cells.iter().map(|&(x, _)| x).collect();
        let rows: std::collections::BTreeSet<usize> =
            ellipse_cells.iter().map(|&(_, y)| y).collect();
        assert_eq!((columns.len(), rows.len()), (14, 14));
        assert_eq!(
            (
                *columns.first().expect("a column"),
                *columns.last().expect("a column"),
                *rows.first().expect("a row"),
                *rows.last().expect("a row"),
            ),
            (25, 38, 11, 24)
        );

        // The ellipse is circular in pixels exactly when `hw * a == hh`, and
        // for this half-extent pair that holds as exact f32 equality on the
        // constants the shader also consumes.
        let resolved = ellipse.windows()[0].half_extents();
        for aspect in [64.0 / 36.0, 16.0 / 9.0, 1920.0 / 1080.0_f32] {
            assert_eq!(
                (resolved[0] * aspect).to_bits(),
                resolved[1].to_bits(),
                "hw * a must equal hh bit for bit at a = {aspect}"
            );
        }

        // §2.3's defensive rule: a degenerate half-extent makes that window's
        // weight 0, with no error and no clamp.
        let degenerate = MatteWindow::from_params(&MatteWindowParams {
            half_width_bp: 0,
            ..MatteWindowParams::NEUTRAL
        });
        assert_eq!(
            degenerate.weight([0.5, 0.5], MATTE_RASTER_ASPECT).to_bits(),
            0.0_f32.to_bits()
        );
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn matte_feather_matches_the_hand_derived_smoothstep() {
        // §9.2.3.  A rect centred at `(0.5, 0.0)` with `hw = hh = 0.5` makes
        // `D = u.y / 0.5` along `u.x = 0.5`, which is exact in f32 for every
        // dyadic `u.y`, so these anchors test the smoothstep and not the
        // rounding of the distance field.
        let feathered = matte(&window_parameters(1, (5_000, 0), (5_000, 5_000), 0, 4_000));
        let weight_at = |distance_half: f32| {
            feathered.coverage(
                [0.5, distance_half],
                MATTE_RASTER_ASPECT,
                MATTE_NEUTRAL_SAMPLE,
            )
        };

        // `f = 0.4`: `w = 1 - smoothstep(0.6, 1.4, D)`.
        // D = 0.8 -> t = 0.25 -> s = 0.15625 -> w = 0.84375.
        assert_eq!(weight_at(0.4).to_bits(), 0.843_75_f32.to_bits());
        // D = 1.0 -> t = 0.5 -> s = 0.5 -> w = 0.5, exact at the edge.
        assert_eq!(weight_at(0.5).to_bits(), 0.5_f32.to_bits());
        // D = 1.2 -> t = 0.75 -> s = 0.84375 -> w = 0.15625.  `4000 / 10000`
        // is not a dyadic rational, so `1 - f` and `1 + f` each round and `t`
        // lands one ulp above 0.75; the deviation is 1.2e-7, which is why this
        // one anchor is asserted to within two ulps rather than bit for bit.
        assert_close(weight_at(0.6), 0.156_25, 1.5e-7);
        // The band straddles the edge: `w = 1` at `D <= 1 - f` and `w = 0` at
        // `D >= 1 + f`, both exactly.
        assert_eq!(weight_at(0.3).to_bits(), 1.0_f32.to_bits());
        assert_eq!(weight_at(0.7).to_bits(), 0.0_f32.to_bits());
        // Symmetry: `w(1 - d) + w(1 + d) = 1`, so `1 - w` is the exact
        // complement of the same shape with the same band.
        // `u.y = (1 -+ delta) / 2` for delta = 0.1, 0.2, 0.4, written as the
        // dyadic literals so the halving is exact.
        for (below, above) in [(0.45_f32, 0.55_f32), (0.4, 0.6), (0.3, 0.7)] {
            assert_close(weight_at(below) + weight_at(above), 1.0, 1.5e-7);
        }

        // `f = 0.25` is dyadic, so the same three smoothstep anchors — now at
        // D = 0.875, 1.0, 1.125 — and the symmetry are exact in f32.
        let dyadic = matte(&window_parameters(1, (5_000, 0), (5_000, 5_000), 0, 2_500));
        let dyadic_at = |distance_half: f32| {
            dyadic.coverage(
                [0.5, distance_half],
                MATTE_RASTER_ASPECT,
                MATTE_NEUTRAL_SAMPLE,
            )
        };
        assert_eq!(dyadic_at(0.437_5).to_bits(), 0.843_75_f32.to_bits());
        assert_eq!(dyadic_at(0.5).to_bits(), 0.5_f32.to_bits());
        assert_eq!(dyadic_at(0.562_5).to_bits(), 0.156_25_f32.to_bits());
        for (below, above) in [(0.45_f32, 0.55_f32), (0.437_5, 0.562_5), (0.375, 0.625)] {
            assert_eq!(
                (dyadic_at(below) + dyadic_at(above)).to_bits(),
                1.0_f32.to_bits(),
                "D = {} and D = {} must be exact complements",
                below * 2.0,
                above * 2.0
            );
        }

        // The affected set of a feathered centred rect is exactly `{D < 1.4}`,
        // measured against an independent f64 transcription of §2.3.  The
        // tightest margin on this raster is |D - 1.4| = 0.00625, four orders
        // of magnitude above f32 noise.
        let banded = matte(&window_parameters(
            1,
            (5_000, 5_000),
            (2_500, 2_500),
            0,
            4_000,
        ));
        let mut affected = 0_usize;
        for y in 0..MATTE_RASTER_HEIGHT {
            for x in 0..MATTE_RASTER_WIDTH {
                let spec_x = ((x as f64 + 0.5) / 64.0 - 0.5) / 0.25;
                let spec_y = ((y as f64 + 0.5) / 36.0 - 0.5) / 0.25;
                let spec_distance = spec_x.abs().max(spec_y.abs());
                let coverage =
                    banded.coverage(matte_uv(x, y), MATTE_RASTER_ASPECT, MATTE_NEUTRAL_SAMPLE);
                assert_eq!(
                    coverage > 0.0,
                    spec_distance < 1.4,
                    "({x}, {y}) D = {spec_distance} resolved to {coverage}"
                );
                if spec_distance >= 1.4 {
                    assert_eq!(coverage.to_bits(), 0.0_f32.to_bits());
                } else {
                    affected += 1;
                }
            }
        }
        assert!(
            affected > 576,
            "a 0.4 feather must reach further than the hard-edged 576"
        );
    }

    #[test]
    fn matte_windows_combine_by_union_and_intersection() {
        // §9.2.4.  Window A is the centred 2500/2500 rect (columns 16..=47);
        // window B is the same rect at `center_x = 7500` (columns 32..=63).
        // Both cover 576 pixels over rows 9..=26 and overlap on columns
        // 32..=47, which is 16 x 18 = 288.  Inclusion-exclusion gives
        // 576 + 576 - 288 = 864 for the union.
        let two_windows = |combine_token: i64, invert_a: i64| {
            matte(&[
                ("matte_window_count", 2),
                ("matte_combine_token", combine_token),
                ("matte_window0_center_x_basis_points", 5_000),
                ("matte_window0_center_y_basis_points", 5_000),
                ("matte_window0_half_width_basis_points", 2_500),
                ("matte_window0_half_height_basis_points", 2_500),
                ("matte_window0_invert", invert_a),
                ("matte_window1_center_x_basis_points", 7_500),
                ("matte_window1_center_y_basis_points", 5_000),
                ("matte_window1_half_width_basis_points", 2_500),
                ("matte_window1_half_height_basis_points", 2_500),
            ])
        };
        let union = covered_cells(&two_windows(0, 0));
        assert_eq!(union.len(), 864);
        assert!(
            union
                .iter()
                .all(|&(x, y)| (16..=63).contains(&x) && (9..=26).contains(&y))
        );
        let intersection = covered_cells(&two_windows(1, 0));
        assert_eq!(intersection.len(), 288);
        assert!(
            intersection
                .iter()
                .all(|&(x, y)| (32..=47).contains(&x) && (9..=26).contains(&y))
        );

        // Per-window invert inside a union: `!A | B` is the complement of
        // `A & !B`, which is 576 - 288 = 288 pixels, so 2304 - 288 = 2016.
        assert_eq!(covered_cells(&two_windows(0, 1)).len(), 2_016);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn matte_qualifier_matches_the_hand_derived_anchors() {
        // §9.2.5.  The anchors are stated on `grade709`-encoded triples, so
        // they are fed to the qualifier as `grade709_decode(e)` — the value
        // entering the node — and the qualifier re-encodes them itself.
        let encoded = |e: [f32; 3]| e.map(grade709_decode);

        // e = (0.8, 0.2, 0.2): M = 0.8 = r, mn = 0.2, C = 0.6, S = C/M = 0.75,
        // Y = 0.2126*0.8 + 0.7152*0.2 + 0.0722*0.2 = 0.32756, H = 0 because
        // g == b makes (g - b)/C exactly zero.
        let warm = MatteQualifier::selectors(encoded([0.8, 0.2, 0.2]));
        assert_close(warm.chroma, 0.6, 2e-6);
        assert_close(warm.saturation, 0.75, 2e-6);
        assert_close(warm.luma, 0.327_56, 2e-6);
        assert_eq!(
            warm.hue.expect("a chromatic pixel").to_bits(),
            0.0_f32.to_bits()
        );

        // Wraparound: e = (0.8, 0.2, 0.35) keeps M = r and C = 0.6, and
        // (g - b)/C = -0.25, so H = 60 * (5.75) = 345.
        let wrapped = MatteQualifier::selectors(encoded([0.8, 0.2, 0.35]));
        assert_close(wrapped.hue.expect("a chromatic pixel"), 345.0, 1e-3);

        // Achromatic: e = (0.5, 0.5, 0.5) has C = 0, so the hue is undefined.
        let grey = MatteQualifier::selectors(encoded([0.5, 0.5, 0.5]));
        assert_eq!(grey.chroma.to_bits(), 0.0_f32.to_bits());
        assert_eq!(grey.saturation.to_bits(), 0.0_f32.to_bits());
        assert_close(grey.luma, 0.5, 2e-6);
        assert!(grey.hue.is_none());

        let qualifier = |parameters: &[(&str, i64)]| {
            let mut stored = vec![("matte_qualifier_enabled", 1_i64)];
            stored.extend_from_slice(parameters);
            matte(&stored)
        };
        let weight = |parameters: &[(&str, i64)], e: [f32; 3]| {
            qualifier(parameters).coverage([0.5, 0.5], MATTE_RASTER_ASPECT, encoded(e))
        };

        // H = 0 inside a +-30 degree band with no softness: q = 1 exactly.
        assert_eq!(
            weight(
                &[
                    ("matte_hue_center_centidegrees", 0),
                    ("matte_hue_width_centidegrees", 3_000),
                ],
                [0.8, 0.2, 0.2],
            )
            .to_bits(),
            1.0_f32.to_bits()
        );
        // H = 345 against a band centred on 350 with a 10 degree half-width:
        // dh = 5 <= 10, so h = 1.  The seam is crossed, not the long way.
        assert_eq!(
            weight(
                &[
                    ("matte_hue_center_centidegrees", 35_000),
                    ("matte_hue_width_centidegrees", 1_000),
                ],
                [0.8, 0.2, 0.35],
            )
            .to_bits(),
            1.0_f32.to_bits()
        );
        // H = 345 against a band centred on 2 degrees: dh = min(343, 17) = 17,
        // t = (17 - 10)/10 = 0.7, smoothstep = 0.49 * (3 - 1.4) = 0.784, so
        // h = 0.216.
        assert_close(
            weight(
                &[
                    ("matte_hue_center_centidegrees", 200),
                    ("matte_hue_width_centidegrees", 1_000),
                    ("matte_hue_softness_centidegrees", 1_000),
                ],
                [0.8, 0.2, 0.35],
            ),
            0.216,
            1e-4,
        );
        // Achromatic, both normative branches: a named hue excludes grey, and
        // the 180 degree half-width disables the hue leg and includes it.
        assert_eq!(
            weight(&[("matte_hue_width_centidegrees", 3_000)], [0.5, 0.5, 0.5],).to_bits(),
            0.0_f32.to_bits()
        );
        assert_eq!(
            weight(&[("matte_hue_width_centidegrees", 18_000)], [0.5, 0.5, 0.5],).to_bits(),
            1.0_f32.to_bits()
        );
        // Saturation softness: S = 0.75 against the band 0.8..1.0 with a 0.1
        // shoulder is min(smoothstep(0.7, 0.8, 0.75), 1) = min(0.5, 1) = 0.5.
        assert_close(
            weight(
                &[
                    ("matte_saturation_low_basis_points", 8_000),
                    ("matte_saturation_high_basis_points", 10_000),
                    ("matte_saturation_softness_basis_points", 1_000),
                ],
                [0.8, 0.2, 0.2],
            ),
            0.5,
            1e-5,
        );
        // The luma leg brackets Y = 0.32756 to within a basis point.
        for (low, high, expected) in [
            (3_275_i64, 3_276_i64, 1.0_f32),
            (0, 3_275, 0.0),
            (3_276, 10_000, 0.0),
        ] {
            assert_eq!(
                weight(
                    &[
                        ("matte_luma_low_basis_points", low),
                        ("matte_luma_high_basis_points", high),
                    ],
                    [0.8, 0.2, 0.2],
                )
                .to_bits(),
                expected.to_bits(),
                "luma band {low}..{high}"
            );
        }
        // A degenerate band evaluates to 0 — no clamp, no reorder, no error.
        assert_eq!(
            weight(
                &[
                    ("matte_saturation_low_basis_points", 8_000),
                    ("matte_saturation_high_basis_points", 2_000),
                ],
                [0.8, 0.2, 0.2],
            )
            .to_bits(),
            0.0_f32.to_bits()
        );
        assert_eq!(
            MatteQualifier::band(0.5, 0.8, 0.2, 0.1).to_bits(),
            0.0_f32.to_bits()
        );
    }

    /// CC3 §2.1 `grade709_encode` transcribed independently in f64.
    fn spec_grade709_encode_f64(x: f64) -> f64 {
        const ALPHA: f64 = 1.099_296_826_809_442;
        const BETA: f64 = 0.018_053_968_510_807;
        let sign = if x > 0.0 {
            1.0
        } else if x < 0.0 {
            -1.0
        } else {
            0.0
        };
        let magnitude = x.abs();
        if magnitude < BETA {
            sign * 4.5 * magnitude
        } else {
            sign * (ALPHA * magnitude.powf(0.45) - (ALPHA - 1.0))
        }
    }

    /// CC3 §2.1 `grade709_decode` transcribed independently in f64.
    fn spec_grade709_decode_f64(e: f64) -> f64 {
        const ALPHA: f64 = 1.099_296_826_809_442;
        const BETA_ENCODED: f64 = 4.5 * 0.018_053_968_510_807;
        let sign = if e > 0.0 {
            1.0
        } else if e < 0.0 {
            -1.0
        } else {
            0.0
        };
        let magnitude = e.abs();
        if magnitude < BETA_ENCODED {
            sign * magnitude / 4.5
        } else {
            sign * ((magnitude + (ALPHA - 1.0)) / ALPHA).powf(1.0 / 0.45)
        }
    }

    /// `color_wheels` with `gain_master_thousandths = 1500` and every other
    /// control neutral: slope 1.5, offset 0, power 1 (CC3 §2.2), in f64.
    fn spec_gain_master_1500_f64(x: f64) -> f64 {
        spec_grade709_decode_f64(1.5 * spec_grade709_encode_f64(x))
    }

    #[test]
    #[allow(clippy::cast_possible_truncation)]
    fn matte_mix_and_invert_scale_the_coverage() {
        // §9.2.6.  A 0.4 feather evaluated exactly on the edge gives
        // `m_raw = 0.5`; `matte_mix = 6000` scales it to 0.3.
        let mixed = matte(&[
            ("matte_window_count", 1),
            ("matte_window0_center_x_basis_points", 5_000),
            ("matte_window0_center_y_basis_points", 0),
            ("matte_window0_half_width_basis_points", 5_000),
            ("matte_window0_half_height_basis_points", 5_000),
            ("matte_window0_feather_basis_points", 4_000),
            ("matte_mix_basis_points", 6_000),
        ]);
        let edge_uv = [0.5_f32, 0.5];
        let coverage = mixed.coverage(edge_uv, MATTE_RASTER_ASPECT, MATTE_NEUTRAL_SAMPLE);
        assert_eq!(coverage.to_bits(), 0.3_f32.to_bits());

        // `out = x + (node(x) - x) * 0.3`, against an independent f64
        // transcription of the wheels node, on three samples: mid grey, a
        // negative undershoot, and an over-range highlight.
        let node = wheels_effect(&[("gain_master_thousandths", 1_500)]);
        let mut effect = node;
        for (name, value) in [
            ("matte_enabled", 1_i64),
            ("matte_window_count", 1),
            ("matte_window0_center_x_basis_points", 5_000),
            ("matte_window0_center_y_basis_points", 0),
            ("matte_window0_half_width_basis_points", 5_000),
            ("matte_window0_half_height_basis_points", 5_000),
            ("matte_window0_feather_basis_points", 4_000),
            ("matte_mix_basis_points", 6_000),
        ] {
            effect
                .parameters
                .insert(name.to_owned(), ParamValue::Integer(value));
        }
        let nodes =
            resolve_color_nodes(std::slice::from_ref(&effect)).expect("a matte-carrying node");
        assert_eq!(nodes.len(), 1);
        assert!(nodes[0].matte().is_some());
        for sample in [[0.18_f32; 3], [-0.05, 0.5, 1.5], [0.9, 0.02, 0.4]] {
            let actual = apply_color_nodes_at(&nodes, sample, edge_uv, MATTE_RASTER_ASPECT);
            for channel in 0..3 {
                let x = f64::from(sample[channel]);
                let expected = x + (spec_gain_master_1500_f64(x) - x) * 0.3;
                assert_close(actual[channel], expected as f32, 2e-5);
            }
        }

        // `matte_invert` is a true complement of the feather band:
        // `m_raw = 0.15625` becomes 0.84375, exactly.
        let inverted = matte(&[
            ("matte_window_count", 1),
            ("matte_window0_center_x_basis_points", 5_000),
            ("matte_window0_center_y_basis_points", 0),
            ("matte_window0_half_width_basis_points", 5_000),
            ("matte_window0_half_height_basis_points", 5_000),
            ("matte_window0_feather_basis_points", 2_500),
            ("matte_invert", 1),
        ]);
        assert_eq!(
            inverted
                .coverage([0.5, 0.562_5], MATTE_RASTER_ASPECT, MATTE_NEUTRAL_SAMPLE)
                .to_bits(),
            0.843_75_f32.to_bits()
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn zero_coverage_is_an_exact_per_pixel_identity() {
        // §2.5.5.  `x + (node(x) - x) * 0.0` is not the identity in f32: it
        // maps -0.0 to +0.0 and maps any x to NaN when node(x) is non-finite.
        // The `m == 0` branch is therefore normative, not an optimization.
        let mut effect = wheels_effect(&[
            ("gain_master_thousandths", 4_000),
            ("gain_red_thousandths", 4_000),
            ("gamma_master_thousandths", 4_000),
            ("gamma_red_thousandths", 4_000),
        ]);
        for (name, value) in [
            ("matte_enabled", 1_i64),
            ("matte_window_count", 1),
            ("matte_window0_center_x_basis_points", 5_000),
            ("matte_window0_center_y_basis_points", 5_000),
            ("matte_window0_half_width_basis_points", 2_500),
            ("matte_window0_half_height_basis_points", 2_500),
        ] {
            effect
                .parameters
                .insert(name.to_owned(), ParamValue::Integer(value));
        }
        let nodes = resolve_color_nodes(std::slice::from_ref(&effect)).expect("a matte node");
        let outside = matte_uv(0, 0);
        let inside = matte_uv(32, 18);
        let matte = nodes[0].matte().expect("an active matte");
        assert_eq!(
            matte
                .coverage(outside, MATTE_RASTER_ASPECT, [4.0; 3])
                .to_bits(),
            0.0_f32.to_bits()
        );

        // Slope 16 and power 16 on linear 4.0 overflows f32 to +inf in red.
        let overflowed = apply_color_node(&nodes[0], [4.0; 3]);
        assert_eq!(overflowed[0], f32::INFINITY);
        let held = apply_color_nodes_at(&nodes, [4.0; 3], outside, MATTE_RASTER_ASPECT);
        assert_eq!(
            bits(held),
            bits([4.0; 3]),
            "a non-finite node output must not leak through m = 0"
        );
        assert!(held.iter().all(|value| !value.is_nan()));

        // Signed zero survives: -0.0 and +0.0 have different bits.
        let signed_zero = [-0.0_f32, -0.0, -0.0];
        assert_eq!(
            bits(apply_color_nodes_at(
                &nodes,
                signed_zero,
                outside,
                MATTE_RASTER_ASPECT
            )),
            bits(signed_zero)
        );

        // Inside the window the same stack does move the sample.
        let moved = apply_color_nodes_at(&nodes, [0.18; 3], inside, MATTE_RASTER_ASPECT);
        assert_ne!(bits(moved), bits([0.18_f32; 3]));
    }

    #[test]
    fn matte_free_nodes_take_the_cc4_path_bit_identically() {
        // §2.5.4.  A node without a matte must not take the blend path at all,
        // because `x + (y - x) * 1.0` is not bit-identical to `y` in f32.
        let effects = [
            wheels_effect(&[("gain_red_thousandths", 1_200)]),
            curve_effect(
                ColorCurveChannel::Master,
                &[(0, 0), (5_000, 6_000), (10_000, 10_000)],
                0,
            ),
        ];
        let nodes = resolve_color_nodes(&effects).expect("a matte-free stack");
        assert_eq!(nodes.len(), 2);
        assert!(nodes.iter().all(|node| node.matte().is_none()));
        for sample in [
            [0.18_f32; 3],
            [-0.05, 0.5, 1.5],
            [0.0, 1.0, 4.0],
            [-0.0, -0.0, -0.0],
        ] {
            let mut expected = sample;
            for node in &nodes {
                expected = apply_color_node(node, expected);
            }
            assert_eq!(
                bits(apply_color_nodes_at(
                    &nodes,
                    sample,
                    [0.123, 0.789],
                    MATTE_RASTER_ASPECT
                )),
                bits(expected),
                "matte-free node stack moved {sample:?}"
            );
            // The result is independent of the position, which is what makes
            // a pre-CC5 project render bit-identically.
            assert_eq!(
                bits(apply_color_nodes_at(&nodes, sample, [0.9, 0.1], 1.0)),
                bits(expected)
            );
            // An empty stack is the exact identity at every position.
            assert_eq!(
                bits(apply_color_nodes_at(
                    &[],
                    sample,
                    [0.25, 0.75],
                    MATTE_RASTER_ASPECT
                )),
                bits(sample)
            );
        }
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn resolve_color_nodes_attaches_active_mattes_and_skips_excluded_nodes() {
        let with_matte = |parameters: &[(&str, i64)]| {
            let mut effect = wheels_effect(&[("gain_red_thousandths", 1_200)]);
            for (name, value) in parameters {
                effect
                    .parameters
                    .insert((*name).to_owned(), ParamValue::Integer(*value));
            }
            resolve_color_nodes(std::slice::from_ref(&effect)).expect("a resolvable node")
        };

        // §2.6 rule 1: an inactive matte attaches nothing.
        for inactive in [
            vec![("matte_enabled", 0_i64), ("matte_window_count", 2)],
            vec![("matte_enabled", 1), ("matte_mix_basis_points", 10_000)],
        ] {
            let nodes = with_matte(&inactive);
            assert_eq!(nodes.len(), 1);
            assert!(
                nodes[0].matte().is_none(),
                "{inactive:?} must resolve to no matte"
            );
        }

        // §2.6 rule 2: `MatteExcluded` drops the node entirely, so bypass and
        // a zero-mix matte are losslessly identical to removing the node.
        for excluded in [
            vec![("matte_enabled", 1_i64), ("matte_mix_basis_points", 0)],
            vec![("matte_enabled", 1), ("matte_invert", 1)],
        ] {
            assert!(
                with_matte(&excluded).is_empty(),
                "{excluded:?} must be skipped as MatteExcluded"
            );
        }

        // An active matte is attached to the node and carries its windows.
        let nodes = with_matte(&[
            ("matte_enabled", 1),
            ("matte_window_count", 2),
            ("matte_combine_token", 1),
            ("matte_qualifier_enabled", 1),
            ("matte_hue_width_centidegrees", 3_000),
            ("matte_mix_basis_points", 5_000),
        ]);
        let matte = nodes[0].matte().expect("an active matte");
        assert_eq!(matte.windows().len(), 2);
        assert!(!matte.is_union());
        assert!(!matte.is_inverted());
        assert!(matte.qualifier().is_some());
        assert_eq!(matte.mix().to_bits(), 0.5_f32.to_bits());
        assert_eq!(
            matte.qualifier().expect("a qualifier").hue_degrees()[1],
            30.0
        );

        // `technical_lut` cannot carry a matte at all: Core resolves the
        // neutral block, and the variant has no place to put one.
        let technical = lut_effect(1, "technical_lut", 1, &[("matte_enabled", 1)]);
        let library = builtin_library();
        let resolved = resolve_color_nodes_with(std::slice::from_ref(&technical), &library)
            .expect("a LUT node");
        assert_eq!(resolved.len(), 1);
        assert!(resolved[0].matte().is_none());
    }
}
