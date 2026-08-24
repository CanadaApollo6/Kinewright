//! Deterministic colour math for the CC1 managed SDR path.
//!
//! This module deliberately has no `FFmpeg` or GPU dependencies.  It is the
//! reference implementation for the source-code-value boundary, the linear
//! primary correction, and the final monitor encoding boundary.  The decoder
//! integration will supply RGB code values after its explicitly configured
//! matrix conversion; no display transfer or primary correction is delegated
//! to a backend here.

use std::fmt;

use kinewright_core::{
    COLOR_CURVE_MIN_POINTS, COLOR_NODE_LIMIT_PER_LAYER, ColorBitDepth, ColorCurveChannel,
    ColorDescription, ColorMatrix, ColorNodeKind, ColorPrimaries, ColorRange, ColorTransfer,
    ColorWheelsParams, ColorWhitePoint, CurvePoints, Effect, ParamValue, ResolvedCurves,
    active_color_nodes, effect_descriptor, managed_color_node_count,
};

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
const BT709_RED_FROM_CR: f32 = 1.5748;
const BT709_GREEN_FROM_CB: f32 = -0.187_324;
const BT709_GREEN_FROM_CR: f32 = -0.468_124;
const BT709_BLUE_FROM_CB: f32 = 1.8556;

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
/// * the direct BT.709 limited-range YUV-to-RGB path uses its 8-bit fixed-point
///   RGB scale even when the input YUV planes are 10 bits (or deeper), so its
///   nominal legal-white denominator is the 8-bit promoted maximum; and
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
    if matches!(description.matrix, ColorMatrix::Bt709)
        && matches!(description.range, ColorRange::Limited)
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
/// # Errors
///
/// Returns an error when the source metadata is not a complete supported CC1
/// profile or its transfer cannot be decoded.
pub fn decode_source_rgb(
    description: &ColorDescription,
    encoded_rgb: [f32; 3],
    assumption: Option<SourceProfileAssumption>,
) -> Result<[f32; 3], ColorPipelineError> {
    let _profile = classify_source_with_assumption(description, assumption)
        .map_err(ColorPipelineError::SourceProfile)?;
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
/// nodes.  This is the CC1 compatibility spelling of [`apply_color_nodes`] for
/// a stack that contains only `primary_correction` nodes; it additionally
/// re-validates each node's integer controls.
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
        linear_rgb = apply_color_node(&ColorNode::Primary(*correction), linear_rgb);
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
// CC3 §3.1: the ordered node stack.
// ---------------------------------------------------------------------------

/// One resolved node of the CC1/CC3 ordered colour-correction stack (§3.1).
///
/// `primary_correction`, `color_wheels`, and `color_curves` form **one**
/// ordered stack executed in `clip.effects` vector order.  There is no fixed
/// inter-kind precedence, and the reference must not flatten, reorder, or
/// merge nodes.
///
/// The curve variant is much larger than the other two because it owns four
/// solved point lists.  Boxing it is deliberately not done: a layer holds at
/// most sixteen nodes, the stack is resolved once per frame rather than per
/// pixel, and an indirection would sit inside the per-pixel dispatch.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ColorNode {
    /// The CC1 managed SDR primary correction.
    Primary(PrimaryCorrection),
    /// The CC3 ASC CDL slope/offset/power wheels.
    Wheels(ColorWheels),
    /// The CC3 master/red/green/blue curves.
    Curves(ColorCurves),
}

impl ColorNode {
    /// The Core node-kind tag for this resolved node.
    #[must_use]
    pub const fn kind(&self) -> ColorNodeKind {
        match self {
            Self::Primary(_) => ColorNodeKind::Primary,
            Self::Wheels(_) => ColorNodeKind::Wheels,
            Self::Curves(_) => ColorNodeKind::Curves,
        }
    }
}

fn apply_color_node(node: &ColorNode, linear_rgb: [f32; 3]) -> [f32; 3] {
    match node {
        ColorNode::Primary(correction) => correction.apply(linear_rgb),
        ColorNode::Wheels(wheels) => wheels.apply(linear_rgb),
        ColorNode::Curves(curves) => curves.apply(linear_rgb),
    }
}

/// Resolve the ordered colour-node stack of one *keyframe-evaluated* effect
/// list (CC3 §3.1, §3.3).
///
/// Keyframes must already have been resolved by the caller
/// (`Effect::evaluated_at`).  Nodes that Core reports inactive — bypassed or
/// neutral — are dropped entirely rather than evaluated as an approximate
/// identity, which is what makes the CC3 §10.3 identity gate bit-identical.
/// `primary_correction` has no bypass control and no neutral short-circuit, so
/// it is always included, exactly as CC1 specifies.  Effects that are not
/// managed colour nodes are ignored.
///
/// # Errors
///
/// Returns [`ColorPipelineError::TooManyColorNodes`] when the layer carries
/// more than [`COLOR_NODE_LIMIT_PER_LAYER`] managed nodes — CC3 §3.1 requires a
/// typed error, never a silent truncation — and any
/// [`PrimaryCorrection::from_effect`] error for a malformed primary node.
pub fn resolve_color_nodes(effects: &[Effect]) -> Result<Vec<ColorNode>, ColorPipelineError> {
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
        nodes.push(match kind {
            ColorNodeKind::Primary => ColorNode::Primary(PrimaryCorrection::from_effect(effect)?),
            ColorNodeKind::Wheels => ColorNode::Wheels(ColorWheels::from_params(
                &ColorWheelsParams::from_effect(effect),
            )),
            ColorNodeKind::Curves => ColorNode::Curves(ColorCurves::from_resolved(
                &ResolvedCurves::from_effect(effect),
            )),
        });
    }
    Ok(nodes)
}

/// Apply a resolved node stack in vector order, with no RGB clamp between
/// nodes (CC3 §3.1).
///
/// Each node consumes scene-linear working RGB and produces scene-linear
/// working RGB.  An empty stack is the exact identity.
#[must_use]
pub fn apply_color_nodes(nodes: &[ColorNode], rgb: [f32; 3]) -> [f32; 3] {
    let mut linear_rgb = rgb;
    for node in nodes {
        linear_rgb = apply_color_node(node, linear_rgb);
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

/// Quantize one already-encoded delivery value to a 16-bit full-range code.
///
/// This is the single quantization allowed before codec packing: the value is
/// clamped once and rounded once, in f32, at the delivery boundary.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn quantize_delivery16(value: f32) -> u16 {
    let clamped = if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    };
    (clamped * 65_535.0).round() as u16
}

/// Apply BT.709 delivery encoding, then clamp and quantize RGB to 16-bit
/// full range. Alpha is a real 16-bit destination channel and is quantized
/// without a transfer.
#[must_use]
pub fn encode_delivery_rgba16(linear_rgba: [f32; 4]) -> [u16; 4] {
    [
        quantize_delivery16(encode_bt709(linear_rgba[0])),
        quantize_delivery16(encode_bt709(linear_rgba[1])),
        quantize_delivery16(encode_bt709(linear_rgba[2])),
        quantize_delivery16(linear_rgba[3]),
    ]
}

/// Encode a linear RGBA value using the requested delivery description.
///
/// CC1 delivers Rec.709 only. The description is checked so the delivery
/// target can never be a `libavfilter` or codec default, and the result is
/// full-range 16-bit so the only 8-bit quantization in the export path is the
/// YUV420P conversion itself.
///
/// # Errors
///
/// Returns an error when the delivery transfer is unknown or not the CC1
/// BT.709 transfer.
pub fn encode_delivery_for_description(
    linear_rgba: [f32; 4],
    delivery: &ColorDescription,
) -> Result<[u16; 4], ColorPipelineError> {
    match &delivery.transfer {
        ColorTransfer::Bt709 => Ok(encode_delivery_rgba16(linear_rgba)),
        ColorTransfer::Unknown => Err(ColorPipelineError::UnknownTransfer),
        value => Err(ColorPipelineError::UnsupportedDeliveryTransfer(
            value.clone(),
        )),
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
        // 16-bit full-range code, not on an 8-bit code re-promoted to 16 bits.
        let encoded = encode_bt709(0.5);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let expected = (encoded * 65_535.0).round() as u16;
        assert_eq!(expected, 46_236);
        assert_eq!(
            encode_delivery_rgba16([0.5, 0.5, 0.5, 1.0]),
            [expected, expected, expected, 65_535]
        );

        // The 8-bit monitor code for the same value is 180; a delivery path
        // that quantized to 8 bits first would produce 180 * 257 = 46260.
        assert_eq!(encode_monitor_rgb8([0.5; 3]), [180; 3]);
        assert_ne!(expected, 46_260);

        // Only the final step clamps.
        assert_eq!(
            encode_delivery_rgba16([-1.0, 0.0, 2.0, 1.5]),
            [0, 0, 65_535, 65_535]
        );
        assert_eq!(
            encode_delivery_rgba16([f32::NAN, 1.0, f32::INFINITY, f32::NAN]),
            [0, 65_535, 65_535, 0]
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
        assert_close(apply_color_nodes(&wheels_first, sample)[0], 0.355_061, 2e-5);
        // curves then wheels, red: the master curve gives 0.262_430_5, then
        // D(E(0.262_430_5) * 1.2) = 0.369_891_6.
        assert_close(apply_color_nodes(&curves_first, sample)[0], 0.369_892, 2e-5);
        assert!(
            (apply_color_nodes(&wheels_first, sample)[0]
                - apply_color_nodes(&curves_first, sample)[0])
                .abs()
                > 1e-2
        );
    }

    #[test]
    fn inactive_nodes_are_skipped_bit_identically() {
        let sample = [0.18_f32, -0.05, 2.5];
        let baseline = apply_color_nodes(&[], sample);
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
            assert_eq!(bits(apply_color_nodes(&nodes, sample)), bits(sample));
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
        let nodes = [ColorNode::Primary(first), ColorNode::Primary(second)];
        assert_eq!(
            bits(apply_primary_corrections(sample, &[first, second]).expect("valid controls")),
            bits(apply_color_nodes(&nodes, sample))
        );
    }
}
