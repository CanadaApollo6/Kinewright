//! Source and pipeline colour metadata contracts.
//!
//! The values in this module describe metadata, not a colour transform.  In
//! particular, `Unknown` is an honest state: callers must not treat an asset
//! with unknown metadata as Rec.709 without an explicit decision.  The
//! `Other(String)` fallback keeps project files readable when a decoder learns
//! about a value newer than this version of the core model.

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as DeError};

/// The maximum representable colour-confidence value in basis points.
pub const COLOR_CONFIDENCE_MAX_BASIS_POINTS: u16 = 10_000;

macro_rules! color_tag {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $($variant:ident => $wire:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, JsonSchema)]
        #[schemars(with = "String")]
        pub enum $name {
            $($variant,)+
            /// A value introduced by a newer decoder or project version.
            Other(String),
        }

        impl Default for $name {
            fn default() -> Self {
                Self::Unknown
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                match self {
                    $(Self::$variant => serializer.serialize_str($wire),)+
                    Self::Other(value) => serializer.serialize_str(value),
                }
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Ok(match value.as_str() {
                    $($wire => Self::$variant,)+
                    _ => Self::Other(value),
                })
            }
        }
    };
}

color_tag! {
    /// The RGB colour primaries declared by the source stream.
    pub enum ColorPrimaries {
        Unknown => "unknown",
        Srgb => "srgb",
        Bt709 => "bt709",
        Bt2020 => "bt2020",
        DisplayP3 => "display_p3",
        DciP3 => "dci_p3",
        Smpte170M => "smpte170_m",
        Smpte240M => "smpte240_m",
        Bt470M => "bt470_m",
        Bt470Bg => "bt470_bg",
        Film => "film",
    }
}

color_tag! {
    /// The transfer characteristic declared by the source stream.
    pub enum ColorTransfer {
        Unknown => "unknown",
        Srgb => "srgb",
        Bt709 => "bt709",
        Bt1886 => "bt1886",
        Linear => "linear",
        Gamma22 => "gamma22",
        Gamma28 => "gamma28",
        Smpte170M => "smpte170_m",
        Smpte2084 => "smpte2084",
        AribStdB67 => "arib_std_b67",
        Log => "log",
        LogC => "log_c",
        Log3G10 => "log3g10",
    }
}

color_tag! {
    /// The matrix coefficients declared by the source stream.
    pub enum ColorMatrix {
        Unknown => "unknown",
        Identity => "identity",
        Rgb => "rgb",
        Bt709 => "bt709",
        Bt2020Ncl => "bt2020_ncl",
        Bt2020Cl => "bt2020_cl",
        Smpte170M => "smpte170_m",
        Smpte240M => "smpte240_m",
        Ycgco => "ycgco",
        ChromaDerivedNcl => "chroma_derived_ncl",
        ChromaDerivedCl => "chroma_derived_cl",
        Ictcp => "ictcp",
    }
}

color_tag! {
    /// The encoded sample range declared by the source stream.
    pub enum ColorRange {
        Unknown => "unknown",
        Full => "full",
        Limited => "limited",
    }
}

color_tag! {
    /// The nominal reference white point of the source colour description.
    pub enum ColorWhitePoint {
        Unknown => "unknown",
        D50 => "d50",
        D55 => "d55",
        D60 => "d60",
        D65 => "d65",
        Dci => "dci",
    }
}

color_tag! {
    /// Where a source colour description came from.
    pub enum ColorProvenance {
        Unknown => "unknown",
        ContainerMetadata => "container_metadata",
        StreamMetadata => "stream_metadata",
        SidecarMetadata => "sidecar_metadata",
        UserOverride => "user_override",
        Inferred => "inferred",
        ApplicationDefault => "application_default",
    }
}

/// The source sample representation. Integer depths are named for the common
/// video cases, while `Integer` and `Other` keep less common/future values
/// representable without changing the project schema.
#[derive(Debug, Clone, PartialEq, Eq, Hash, JsonSchema, Default)]
#[schemars(schema_with = "color_bit_depth_schema")]
pub enum ColorBitDepth {
    #[default]
    Unknown,
    Eight,
    Ten,
    Twelve,
    Sixteen,
    Float16,
    Float32,
    Integer(u16),
    Other(String),
}

fn color_bit_depth_schema(generator: &mut SchemaGenerator) -> Schema {
    let _ = generator;
    json_schema!({
        "oneOf": [
            {
                "type": "integer",
                "minimum": 0,
                "maximum": u16::MAX
            },
            { "type": "string" }
        ]
    })
}

impl Serialize for ColorBitDepth {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Unknown => serializer.serialize_str("unknown"),
            Self::Eight => serializer.serialize_u16(8),
            Self::Ten => serializer.serialize_u16(10),
            Self::Twelve => serializer.serialize_u16(12),
            Self::Sixteen => serializer.serialize_u16(16),
            Self::Float16 => serializer.serialize_str("float16"),
            Self::Float32 => serializer.serialize_str("float32"),
            Self::Integer(bits) => serializer.serialize_u16(*bits),
            Self::Other(value) => serializer.serialize_str(value),
        }
    }
}

impl<'de> Deserialize<'de> for ColorBitDepth {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum WireValue {
            Integer(u16),
            Text(String),
        }

        match WireValue::deserialize(deserializer)? {
            WireValue::Integer(bits) => Ok(match bits {
                8 => Self::Eight,
                10 => Self::Ten,
                12 => Self::Twelve,
                16 => Self::Sixteen,
                other => Self::Integer(other),
            }),
            WireValue::Text(value) => Ok(match value.as_str() {
                "unknown" => Self::Unknown,
                "float16" => Self::Float16,
                "float32" => Self::Float32,
                _ => Self::Other(value),
            }),
        }
    }
}

/// A source colour description obtained during media probing or supplied by
/// an explicit user override.
///
/// Confidence is represented in basis points (`0..=10_000`) to keep the
/// serialized contract deterministic and consistent with other core analysis
/// values.  The core model intentionally does not clamp or infer it: probe and
/// override paths own validation, while consumers can inspect the raw value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ColorDescription {
    #[serde(default)]
    #[schemars(default)]
    pub primaries: ColorPrimaries,
    #[serde(default)]
    #[schemars(default)]
    pub transfer: ColorTransfer,
    #[serde(default)]
    #[schemars(default)]
    pub matrix: ColorMatrix,
    #[serde(default)]
    #[schemars(default)]
    pub range: ColorRange,
    #[serde(default)]
    #[schemars(default)]
    pub white_point: ColorWhitePoint,
    #[serde(default)]
    #[schemars(default)]
    pub bit_depth: ColorBitDepth,
    /// Confidence in the interpreted metadata, from 0.00% to 100.00%.
    #[serde(default, deserialize_with = "deserialize_confidence_basis_points")]
    #[schemars(default, range(max = 10_000))]
    pub confidence_basis_points: u16,
    #[serde(default)]
    #[schemars(default)]
    pub provenance: ColorProvenance,
}

impl Default for ColorDescription {
    fn default() -> Self {
        Self::unknown()
    }
}

impl ColorDescription {
    /// Construct an explicit unknown source description for legacy or
    /// incomplete media metadata.
    #[must_use]
    pub fn unknown() -> Self {
        Self {
            primaries: ColorPrimaries::Unknown,
            transfer: ColorTransfer::Unknown,
            matrix: ColorMatrix::Unknown,
            range: ColorRange::Unknown,
            white_point: ColorWhitePoint::Unknown,
            bit_depth: ColorBitDepth::Unknown,
            confidence_basis_points: 0,
            provenance: ColorProvenance::Unknown,
        }
    }

    /// Whether no source colour metadata has been established.
    #[must_use]
    pub fn is_unknown(&self) -> bool {
        self == &Self::unknown()
    }

    /// Whether the confidence value is inside the documented basis-point
    /// range.
    #[must_use]
    pub const fn confidence_is_valid(&self) -> bool {
        self.confidence_basis_points <= COLOR_CONFIDENCE_MAX_BASIS_POINTS
    }
}

/// Decode a colour confidence value and reject values outside the contract.
///
/// This helper is intentionally separate from `ColorDescription`'s derived
/// serde implementation so old projects can still omit the field while new
/// probe/override code can opt into strict validation.
pub fn deserialize_confidence_basis_points<'de, D>(deserializer: D) -> Result<u16, D::Error>
where
    D: Deserializer<'de>,
{
    let value = u16::deserialize(deserializer)?;
    if value > COLOR_CONFIDENCE_MAX_BASIS_POINTS {
        return Err(D::Error::custom(format!(
            "colour confidence must be in 0..={COLOR_CONFIDENCE_MAX_BASIS_POINTS} basis points"
        )));
    }
    Ok(value)
}

impl ColorDescription {
    fn application_default(matrix: ColorMatrix, range: ColorRange) -> Self {
        Self {
            primaries: ColorPrimaries::Bt709,
            transfer: ColorTransfer::Bt709,
            matrix,
            range,
            white_point: ColorWhitePoint::D65,
            bit_depth: ColorBitDepth::Eight,
            confidence_basis_points: COLOR_CONFIDENCE_MAX_BASIS_POINTS,
            provenance: ColorProvenance::ApplicationDefault,
        }
    }
}

/// The colour descriptions used by the current application SDR pipeline.
///
/// The three stages remain separate even while their defaults are fixed. This
/// makes the project contract ready for later managed-pipeline work without
/// conflating a working-space description with a monitor or delivery target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ColorContext {
    #[serde(default)]
    #[schemars(default)]
    pub working: ColorDescription,
    #[serde(default)]
    #[schemars(default)]
    pub monitoring: ColorDescription,
    #[serde(default)]
    #[schemars(default)]
    pub delivery: ColorDescription,
}

impl Default for ColorContext {
    fn default() -> Self {
        Self::sdr_rec709()
    }
}

impl ColorContext {
    /// Construct the current application SDR Rec.709 colour context.
    ///
    /// Working and monitoring use RGB/full-range application values. Delivery
    /// uses BT.709 matrix coefficients and limited range for the current
    /// video export path. All three descriptions use D65, 8-bit samples, and
    /// application-default provenance.
    #[must_use]
    pub fn sdr_rec709() -> Self {
        Self {
            working: ColorDescription::application_default(ColorMatrix::Rgb, ColorRange::Full),
            monitoring: ColorDescription::application_default(ColorMatrix::Rgb, ColorRange::Full),
            delivery: ColorDescription::application_default(
                ColorMatrix::Bt709,
                ColorRange::Limited,
            ),
        }
    }

    /// Whether this context is exactly the current application default.
    #[must_use]
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}
