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
    ColorBitDepth, ColorDescription, ColorMatrix, ColorPrimaries, ColorRange, ColorTransfer,
    ColorWhitePoint, Effect, ParamValue, effect_descriptor,
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
/// nodes.  This is the reference for the later compositor integration seam.
///
/// # Errors
///
/// Returns an error when any primary node fails descriptor validation.
pub fn apply_primary_corrections(
    mut linear_rgb: [f32; 3],
    corrections: &[PrimaryCorrection],
) -> Result<[f32; 3], ColorPipelineError> {
    for correction in corrections {
        linear_rgb = correction.apply_checked(linear_rgb)?;
    }
    Ok(linear_rgb)
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
    }
}
