//! Source and pipeline colour metadata contracts.
//!
//! The values in this module describe metadata, not a colour transform.  In
//! particular, `Unknown` is an honest state: callers must not treat an asset
//! with unknown metadata as Rec.709 without an explicit decision.  The
//! `Other(String)` fallback keeps project files readable when a decoder learns
//! about a value newer than this version of the core model.

use std::hash::{Hash, Hasher};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as DeError};
use thiserror::Error;

use crate::cc8_hdr::{
    CC8_HDR_DEPTH_ALLOWED, CC8_HDR_MATRIX_ALLOWED, CC8_HDR_RANGE_ALLOWED, CC8_HDR_RECOVERY_ACTION,
    CC8_HDR_WHITE_POINT_ALLOWED, CC8_SOURCE_PROFILES, Cc8SourceProfile, cc8_source_profile_by_id,
    cc8_source_profile_for_primaries_and_transfer,
};

/// The maximum representable colour-confidence value in basis points.
pub const COLOR_CONFIDENCE_MAX_BASIS_POINTS: u16 = 10_000;

/// The complete closed source-profile set, as one phrase, for the
/// `UnsupportedCombination` allowed-value string.
///
/// CC1's two profiles then CC8 §2.1's two, in [`ColorSourceProfile::id`] order.
/// `supported_profile_ids_phrase_names_every_profile` asserts every variant's
/// id appears here, so a profile added without updating this phrase fails a
/// test rather than shipping a status that quietly under-reports the set.
const SUPPORTED_PROFILE_IDS: &str = "rec709_video, srgb_full, pq_rec2020, or hlg_rec2020";

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

        impl $name {
            /// The serialized wire spelling of this tag.
            ///
            /// The same string [`Serialize`] writes and [`Deserialize`] reads,
            /// returned by reference so a caller can match a value against a
            /// table of wire spellings without allocating or restating the
            /// literal. CC8 §2.1's profile table
            /// (`crate::cc8_hdr::CC8_SOURCE_PROFILES`) is that caller, and
            /// `cc8_source_profile_wire_spellings_are_the_color_tag_serde_forms`
            /// asserts the two agree, so a renamed tag cannot silently stop
            /// matching a profile.
            #[must_use]
            pub fn wire(&self) -> &str {
                match self {
                    $(Self::$variant => $wire,)+
                    Self::Other(value) => value.as_str(),
                }
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

/// The colour-pipeline semantics used to render a project.
///
/// `Legacy` is retained for projects whose pre-CC1 context cannot be proven to
/// be the exact CC0 application default. `Other` keeps a newer pipeline state
/// readable without pretending that this version can execute its semantics.
///
/// Per the CC1 contract (§4), an absent pipeline state means `legacy`, so
/// `Legacy` is also the type-level default. Only a context whose working,
/// monitoring, and delivery descriptions all match the managed SDR targets is
/// stamped [`ColorPipelineState::ManagedSdrV1`]; see
/// [`ColorContext::sdr_rec709`] for the target the first CC1 save writes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, JsonSchema, Default)]
#[schemars(with = "String")]
pub enum ColorPipelineState {
    #[default]
    Legacy,
    ManagedSdrV1,
    Other(String),
}

/// A managed source profile accepted by the colour contract.
///
/// [`Self::Rec709Video`] and [`Self::SrgbFull`] are CC1's managed SDR profiles.
/// [`Self::PqRec2020`] and [`Self::HlgRec2020`] are CC8 §2.1's two HDR
/// profiles, and the set stays **closed**: §1 requires an HDR source that is
/// not one of them to "produce a visible typed status with an explicit override
/// path", never a silent Rec.709 reading and never a silent tone map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ColorSourceProfile {
    Rec709Video,
    SrgbFull,
    /// CC8 §2.1 `pq_rec2020`: BT.2020 primaries, ST 2084, BT.2020 NCL or RGB,
    /// limited or full range, D65, 10..=16-bit integer samples.
    PqRec2020,
    /// CC8 §2.1 `hlg_rec2020`: the same table row with ARIB STD-B67.
    HlgRec2020,
}

impl ColorSourceProfile {
    /// Stable identifier used by status, proof, and agent surfaces.
    ///
    /// The two HDR ids are read from CC8 §2.1's table rather than restated, so
    /// this accessor and the authority module cannot drift apart;
    /// `hdr_profile_ids_come_from_the_cc8_authority_table` asserts the
    /// `serde` form agrees with both.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Rec709Video => "rec709_video",
            Self::SrgbFull => "srgb_full",
            Self::PqRec2020 => CC8_SOURCE_PROFILES[0].id,
            Self::HlgRec2020 => CC8_SOURCE_PROFILES[1].id,
        }
    }

    /// Whether this is one of CC8 §2.1's HDR profiles.
    ///
    /// The single question every surface that behaves differently on HDR asks:
    /// the §3.3 interpretation stages, §7 item 2's HDR-source/SDR-delivery
    /// block, §6 item 4's withheld skin report, and §8's status rows.
    #[must_use]
    pub const fn is_hdr(self) -> bool {
        matches!(self, Self::PqRec2020 | Self::HlgRec2020)
    }

    /// CC8 §2.1's table row for this profile, or `None` for a CC1 SDR profile.
    #[must_use]
    pub fn cc8_row(self) -> Option<&'static Cc8SourceProfile> {
        self.is_hdr().then(|| cc8_source_profile_by_id(self.id()))?
    }
}

/// An explicit assumption allowed while classifying a source description.
/// The raw description remains unchanged; callers must record the assumption
/// in their proof/status surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ColorSourceProfileAssumption {
    D65,
}

/// Exact failure reasons for the bounded CC1 source-profile classifier.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ColorSourceError {
    #[error("source colour primaries are unknown")]
    UnknownPrimaries,
    #[error("unsupported source colour primaries: {0:?}")]
    UnsupportedPrimaries(ColorPrimaries),
    #[error("source colour transfer is unknown")]
    UnknownTransfer,
    #[error("unsupported source colour transfer: {0:?}")]
    UnsupportedTransfer(ColorTransfer),
    #[error("source colour matrix is unknown")]
    UnknownMatrix,
    #[error("unsupported source colour matrix: {0:?}")]
    UnsupportedMatrix(ColorMatrix),
    #[error("source colour range is unknown")]
    UnknownRange,
    #[error("unsupported source colour range: {0:?}")]
    UnsupportedRange(ColorRange),
    #[error("source colour white point is unknown")]
    UnknownWhitePoint,
    #[error("unsupported source colour white point: {0:?}")]
    UnsupportedWhitePoint(ColorWhitePoint),
    #[error("source colour bit depth is unknown")]
    UnknownBitDepth,
    #[error("unsupported source colour bit depth: {0:?}")]
    UnsupportedBitDepth(ColorBitDepth),
    /// CC8 §2.1's 10-bit floor, as its own typed reason.
    ///
    /// §2.1: "The 10-bit floor is deliberate and normative: 8-bit PQ or HLG is
    /// banding by construction, and accepting it would be a claim CC8 cannot
    /// defend. **An 8- or 9-bit HDR tuple is a typed rejection naming the
    /// depth, not a warning.**"
    ///
    /// It is a separate variant from [`Self::UnsupportedBitDepth`] rather than
    /// a widened one because the two have different allowed sets — CC1 admits
    /// `8..=16`, CC8 admits `10..=16` — and because CC1's allowed-value strings
    /// are asserted verbatim by
    /// `cc1_fixtures::cc1_source_profile_classification_is_typed_and_actionable`.
    /// One variant carrying two answers would have to pick one of them. The
    /// same reasoning gives the three siblings below their own variants.
    #[error("unsupported CC8 HDR source colour bit depth: {0:?}")]
    UnsupportedHdrBitDepth(ColorBitDepth),
    /// A matrix outside CC8 §2.1's `bt2020_ncl`/`rgb` column on an otherwise
    /// HDR tuple — `bt2020_cl`, `ictcp`, and `chroma_derived_*` among them.
    #[error("unsupported CC8 HDR source colour matrix: {0:?}")]
    UnsupportedHdrMatrix(ColorMatrix),
    /// A range outside CC8 §2.1's `limited`/`full` column on an HDR tuple.
    #[error("unsupported CC8 HDR source colour range: {0:?}")]
    UnsupportedHdrRange(ColorRange),
    /// A **known** white point other than D65 on an HDR tuple.
    ///
    /// An *unknown* white point on an HDR tuple stays
    /// [`Self::UnknownWhitePoint`] deliberately. §2.1: "The CC1 D65 rule
    /// carries over unchanged" — the recovery is the same explicit
    /// `profile_assumption`, and every surface that keys on that variant to
    /// report `profile_assumption.required` must keep working for an HDR source
    /// without forking. The CC8-specific allowed phrase for that case is
    /// `cc8_hdr::CC8_HDR_WHITE_POINT_ALLOWED`, reported by §8's HDR status
    /// block; this variant is only the known-and-wrong case, which has no
    /// assumption recovery at all.
    #[error("unsupported CC8 HDR source colour white point: {0:?}")]
    UnsupportedHdrWhitePoint(ColorWhitePoint),
    #[error(
        "unsupported CC1 source combination: primaries={primaries:?}, transfer={transfer:?}, matrix={matrix:?}, range={range:?}"
    )]
    UnsupportedCombination {
        primaries: ColorPrimaries,
        transfer: ColorTransfer,
        matrix: ColorMatrix,
        range: ColorRange,
    },
}

impl ColorSourceError {
    /// Stable machine-readable status code for agent and UI consumers.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::UnknownPrimaries => "unknown_source_primaries",
            Self::UnsupportedPrimaries(_) => "unsupported_source_primaries",
            Self::UnknownTransfer => "unknown_source_transfer",
            Self::UnsupportedTransfer(_) => "unsupported_source_transfer",
            Self::UnknownMatrix => "unknown_source_matrix",
            Self::UnsupportedMatrix(_) => "unsupported_source_matrix",
            Self::UnknownRange => "unknown_source_range",
            Self::UnsupportedRange(_) => "unsupported_source_range",
            Self::UnknownWhitePoint => "unknown_source_white_point",
            Self::UnsupportedWhitePoint(_) => "unsupported_source_white_point",
            Self::UnknownBitDepth => "unknown_source_bit_depth",
            Self::UnsupportedBitDepth(_) => "unsupported_source_bit_depth",
            Self::UnsupportedHdrBitDepth(_) => "unsupported_hdr_source_bit_depth",
            Self::UnsupportedHdrMatrix(_) => "unsupported_hdr_source_matrix",
            Self::UnsupportedHdrRange(_) => "unsupported_hdr_source_range",
            Self::UnsupportedHdrWhitePoint(_) => "unsupported_hdr_source_white_point",
            Self::UnsupportedCombination { .. } => "unsupported_source_combination",
        }
    }

    /// Stable source-description field associated with the failure.
    #[must_use]
    pub const fn field(&self) -> &'static str {
        match self {
            Self::UnknownPrimaries | Self::UnsupportedPrimaries(_) => "primaries",
            Self::UnknownTransfer | Self::UnsupportedTransfer(_) => "transfer",
            Self::UnknownMatrix | Self::UnsupportedMatrix(_) | Self::UnsupportedHdrMatrix(_) => {
                "matrix"
            }
            Self::UnknownRange | Self::UnsupportedRange(_) | Self::UnsupportedHdrRange(_) => {
                "range"
            }
            Self::UnknownWhitePoint
            | Self::UnsupportedWhitePoint(_)
            | Self::UnsupportedHdrWhitePoint(_) => "white_point",
            Self::UnknownBitDepth
            | Self::UnsupportedBitDepth(_)
            | Self::UnsupportedHdrBitDepth(_) => "bit_depth",
            Self::UnsupportedCombination { .. } => "profile",
        }
    }

    /// Observed value formatted for a structured status surface.
    #[must_use]
    pub fn observed(&self) -> String {
        match self {
            Self::UnknownPrimaries
            | Self::UnknownTransfer
            | Self::UnknownMatrix
            | Self::UnknownRange
            | Self::UnknownWhitePoint
            | Self::UnknownBitDepth => "unknown".to_owned(),
            Self::UnsupportedPrimaries(value) => format!("{value:?}"),
            Self::UnsupportedTransfer(value) => format!("{value:?}"),
            Self::UnsupportedMatrix(value) | Self::UnsupportedHdrMatrix(value) => {
                format!("{value:?}")
            }
            Self::UnsupportedRange(value) | Self::UnsupportedHdrRange(value) => {
                format!("{value:?}")
            }
            Self::UnsupportedWhitePoint(value) | Self::UnsupportedHdrWhitePoint(value) => {
                format!("{value:?}")
            }
            Self::UnsupportedBitDepth(value) | Self::UnsupportedHdrBitDepth(value) => {
                format!("{value:?}")
            }
            Self::UnsupportedCombination {
                primaries,
                transfer,
                matrix,
                range,
            } => format!(
                "primaries={primaries:?}, transfer={transfer:?}, matrix={matrix:?}, range={range:?}"
            ),
        }
    }

    /// Allowed values or tuple shapes for the failed field.
    ///
    /// **The CC1 phrases are CC1 statements and are unchanged**, because
    /// `cc1_fixtures::cc1_source_profile_classification_is_typed_and_actionable`
    /// pins each verbatim and CC8 §9.1 fixture 6 requires every CC1 fixture to
    /// pass unmoved. So `unsupported_source_primaries` on a Rec.2020 tuple
    /// still reads "bt709 or srgb in a supported CC1 profile" — true of the CC1
    /// profiles, and complemented rather than contradicted by
    /// [`cc8_hdr::CC8_HDR_PRIMARIES_ALLOWED`](crate::cc8_hdr::CC8_HDR_PRIMARIES_ALLOWED),
    /// which §8's HDR status block reports and which
    /// [`SUPPORTED_PROFILE_IDS`] names in the combination phrase. The CC8 §2.1
    /// reasons carry their own phrases from the authority module.
    #[must_use]
    pub const fn allowed_values(&self) -> &'static str {
        match self {
            Self::UnknownPrimaries | Self::UnsupportedPrimaries(_) => {
                "bt709 or srgb in a supported CC1 profile"
            }
            Self::UnknownTransfer | Self::UnsupportedTransfer(_) => {
                "bt709, bt1886, or srgb in a matching profile"
            }
            Self::UnknownMatrix | Self::UnsupportedMatrix(_) => {
                "bt709/rgb or rgb/identity in a matching profile"
            }
            Self::UnknownRange | Self::UnsupportedRange(_) => {
                "full or limited in a matching profile"
            }
            Self::UnknownWhitePoint | Self::UnsupportedWhitePoint(_) => {
                "d65, or an explicit D65 assumption for BT.709"
            }
            Self::UnknownBitDepth | Self::UnsupportedBitDepth(_) => "integer depth 8..=16",
            Self::UnsupportedHdrBitDepth(_) => CC8_HDR_DEPTH_ALLOWED,
            Self::UnsupportedHdrMatrix(_) => CC8_HDR_MATRIX_ALLOWED,
            Self::UnsupportedHdrRange(_) => CC8_HDR_RANGE_ALLOWED,
            Self::UnsupportedHdrWhitePoint(_) => CC8_HDR_WHITE_POINT_ALLOWED,
            Self::UnsupportedCombination { .. } => SUPPORTED_PROFILE_IDS,
        }
    }

    /// Recovery action suitable for a visible status or agent response.
    ///
    /// CC1's sentence is unchanged for every CC1 reason and is asserted
    /// verbatim by `cc1_fixtures::cc1_unsupported_metadata_classification`; the
    /// CC8 §2.1 reasons get their own, because "relink to compatible media" is
    /// not a useful instruction for a file that *is* HDR and is being refused
    /// for a named field rather than for being HDR.
    #[must_use]
    pub const fn recovery_action(&self) -> &'static str {
        match self {
            Self::UnsupportedHdrBitDepth(_)
            | Self::UnsupportedHdrMatrix(_)
            | Self::UnsupportedHdrRange(_)
            | Self::UnsupportedHdrWhitePoint(_) => CC8_HDR_RECOVERY_ACTION,
            _ => {
                "Apply an explicit supported source-colour override or relink to compatible media."
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

impl Serialize for ColorPipelineState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Legacy => serializer.serialize_str("legacy"),
            Self::ManagedSdrV1 => serializer.serialize_str("managed_sdr_v1"),
            Self::Other(value) => serializer.serialize_str(value),
        }
    }
}

impl<'de> Deserialize<'de> for ColorPipelineState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(match value.as_str() {
            "legacy" => Self::Legacy,
            "managed_sdr_v1" => Self::ManagedSdrV1,
            _ => Self::Other(value),
        })
    }
}

/// The source sample representation. Integer depths are named for the common
/// video cases, while `Integer` and `Other` keep less common/future values
/// representable without changing the project schema.
///
/// CC1 §2.1 requires the named integer variants to be equivalent to their
/// numeric form, so `Integer(8) == Eight`, `Integer(10) == Ten`, and so on.
/// Equality and hashing therefore run through the canonical integer form, and
/// [`ColorBitDepth::integer`] plus the deserializer normalise a numeric depth
/// to its named variant when one exists.
#[derive(Debug, Clone, JsonSchema, Default)]
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

/// Canonical comparison form for [`ColorBitDepth`].
///
/// The named integer variants and their numeric spellings collapse onto the
/// same value so equality and hashing stay consistent with CC1 §2.1.
#[derive(Debug, PartialEq, Eq, Hash)]
enum CanonicalBitDepth<'a> {
    Unknown,
    Integer(u16),
    Float16,
    Float32,
    Other(&'a str),
}

impl ColorBitDepth {
    /// Construct an integer depth, normalising the common video depths to
    /// their named variants.
    #[must_use]
    pub const fn integer(bits: u16) -> Self {
        match bits {
            8 => Self::Eight,
            10 => Self::Ten,
            12 => Self::Twelve,
            16 => Self::Sixteen,
            other => Self::Integer(other),
        }
    }

    /// The integer sample depth, if this value describes integer samples.
    ///
    /// Float and unknown/opaque depths return `None`, as do integer depths
    /// that cannot be a real sample depth (`> 255`).
    #[must_use]
    pub fn integer_bits(&self) -> Option<u8> {
        match self {
            Self::Eight => Some(8),
            Self::Ten => Some(10),
            Self::Twelve => Some(12),
            Self::Sixteen => Some(16),
            Self::Integer(bits) => u8::try_from(*bits).ok(),
            _ => None,
        }
    }

    fn canonical(&self) -> CanonicalBitDepth<'_> {
        match self {
            Self::Unknown => CanonicalBitDepth::Unknown,
            Self::Eight => CanonicalBitDepth::Integer(8),
            Self::Ten => CanonicalBitDepth::Integer(10),
            Self::Twelve => CanonicalBitDepth::Integer(12),
            Self::Sixteen => CanonicalBitDepth::Integer(16),
            Self::Float16 => CanonicalBitDepth::Float16,
            Self::Float32 => CanonicalBitDepth::Float32,
            Self::Integer(bits) => CanonicalBitDepth::Integer(*bits),
            Self::Other(value) => CanonicalBitDepth::Other(value.as_str()),
        }
    }
}

impl PartialEq for ColorBitDepth {
    fn eq(&self, other: &Self) -> bool {
        self.canonical() == other.canonical()
    }
}

impl Eq for ColorBitDepth {}

impl Hash for ColorBitDepth {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.canonical().hash(state);
    }
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
            WireValue::Integer(bits) => Ok(Self::integer(bits)),
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
        Self::application_default_with(matrix, range, ColorTransfer::Bt709, ColorBitDepth::Eight)
    }

    fn application_default_with(
        matrix: ColorMatrix,
        range: ColorRange,
        transfer: ColorTransfer,
        bit_depth: ColorBitDepth,
    ) -> Self {
        Self {
            primaries: ColorPrimaries::Bt709,
            transfer,
            matrix,
            range,
            white_point: ColorWhitePoint::D65,
            bit_depth,
            confidence_basis_points: COLOR_CONFIDENCE_MAX_BASIS_POINTS,
            provenance: ColorProvenance::ApplicationDefault,
        }
    }
}

/// The CC8 §2.1 profile row a description's primaries/transfer pair selects.
///
/// This is the one place the HDR arm is entered from, and it is keyed on the
/// **pair** because that is what §2.1's table makes distinctive: a description
/// whose pair is not one of the two rows is not an HDR tuple at all and is
/// diagnosed entirely by CC1's field rules, which is what keeps a BT.709/PQ or
/// Rec.2020/BT.709 mismatch reported against the field CC1 already names.
fn cc8_hdr_row(description: &ColorDescription) -> Option<&'static Cc8SourceProfile> {
    cc8_source_profile_for_primaries_and_transfer(
        description.primaries.wire(),
        description.transfer.wire(),
    )
}

/// Whether a colour description is one of CC8 §2.1's HDR profile *shapes*.
///
/// The pair only — this answers "is this description HDR?" for surfaces that
/// need the question before a full classification, such as §7 item 2's
/// HDR-source/SDR-delivery check, which must recognise an HDR delivery target
/// whether or not every other field would classify.
#[must_use]
pub fn color_description_is_cc8_hdr(description: &ColorDescription) -> bool {
    cc8_hdr_row(description).is_some()
}

/// Classify a source description against the bounded managed input profiles:
/// CC1's two SDR profiles and CC8 §2.1's two HDR profiles. Unknown white point
/// is intentionally rejected; callers that have an explicit, inspectable D65
/// assumption may use [`classify_source_with_assumption`].
///
/// # Errors
///
/// Returns the typed source-field or source-combination reason when the
/// description is unknown, unsupported, or incomplete for a CC1 profile.
pub fn classify_source(
    description: &ColorDescription,
) -> Result<ColorSourceProfile, ColorSourceError> {
    classify_source_with_assumption(description, None)
}

/// Classify a source with an optional explicit profile assumption. The source
/// description is never rewritten by this helper.
///
/// # Errors
///
/// Returns the typed source-field or source-combination reason when the
/// description is not compatible with the selected CC1 profile assumption.
pub fn classify_source_with_assumption(
    description: &ColorDescription,
    assumption: Option<ColorSourceProfileAssumption>,
) -> Result<ColorSourceProfile, ColorSourceError> {
    if let Err(error) = validate_source_fields(description, assumption) {
        // An unknown white point must not conceal an independently invalid
        // primaries/transfer/matrix/range tuple. Diagnose the combination
        // with the profile's normative D65 value, but retain the raw unknown
        // error whenever the remaining tuple is otherwise supported. This is
        // diagnostic only and never rewrites source metadata or broadens the
        // explicit BT.709 D65-assumption policy.
        if error == ColorSourceError::UnknownWhitePoint {
            let mut diagnostic = description.clone();
            diagnostic.white_point = ColorWhitePoint::D65;
            if let Err(ColorSourceError::UnsupportedCombination {
                primaries,
                transfer,
                matrix,
                range,
            }) = classify_source_with_assumption(&diagnostic, None)
            {
                return Err(ColorSourceError::UnsupportedCombination {
                    primaries,
                    transfer,
                    matrix,
                    range,
                });
            }
        }
        return Err(error);
    }

    // CC8 §2.1's closed HDR set. `validate_source_fields` has already checked
    // every other column of the selected row, and the row is selected by the
    // primaries/transfer pair, so a row that survives validation is a complete
    // match on all listed fields — §2.1's "a partial match is not enough".
    if let Some(row) = cc8_hdr_row(description) {
        return Ok(match row.transfer {
            transfer if transfer == CC8_SOURCE_PROFILES[0].transfer => {
                ColorSourceProfile::PqRec2020
            }
            _ => ColorSourceProfile::HlgRec2020,
        });
    }

    let rec709_video = matches!(description.primaries, ColorPrimaries::Bt709)
        && matches!(
            description.transfer,
            ColorTransfer::Bt709 | ColorTransfer::Bt1886
        )
        && matches!(description.matrix, ColorMatrix::Bt709 | ColorMatrix::Rgb)
        && matches!(description.range, ColorRange::Limited | ColorRange::Full);
    if rec709_video {
        return Ok(ColorSourceProfile::Rec709Video);
    }

    let srgb_full = matches!(
        description.primaries,
        ColorPrimaries::Srgb | ColorPrimaries::Bt709
    ) && matches!(description.transfer, ColorTransfer::Srgb)
        && matches!(description.matrix, ColorMatrix::Rgb | ColorMatrix::Identity)
        && matches!(description.range, ColorRange::Full);
    if srgb_full {
        return Ok(ColorSourceProfile::SrgbFull);
    }

    Err(ColorSourceError::UnsupportedCombination {
        primaries: description.primaries.clone(),
        transfer: description.transfer.clone(),
        matrix: description.matrix.clone(),
        range: description.range.clone(),
    })
}

fn validate_source_fields(
    description: &ColorDescription,
    assumption: Option<ColorSourceProfileAssumption>,
) -> Result<(), ColorSourceError> {
    if let Some(row) = cc8_hdr_row(description) {
        return validate_cc8_hdr_source_fields(description, row, assumption);
    }
    match &description.primaries {
        ColorPrimaries::Unknown => return Err(ColorSourceError::UnknownPrimaries),
        ColorPrimaries::Srgb | ColorPrimaries::Bt709 => {}
        value => return Err(ColorSourceError::UnsupportedPrimaries(value.clone())),
    }

    match &description.transfer {
        ColorTransfer::Unknown => return Err(ColorSourceError::UnknownTransfer),
        ColorTransfer::Srgb | ColorTransfer::Bt709 | ColorTransfer::Bt1886 => {}
        value => return Err(ColorSourceError::UnsupportedTransfer(value.clone())),
    }

    match &description.matrix {
        ColorMatrix::Unknown => return Err(ColorSourceError::UnknownMatrix),
        ColorMatrix::Identity | ColorMatrix::Rgb | ColorMatrix::Bt709 => {}
        value => return Err(ColorSourceError::UnsupportedMatrix(value.clone())),
    }

    match &description.range {
        ColorRange::Unknown => return Err(ColorSourceError::UnknownRange),
        ColorRange::Full | ColorRange::Limited => {}
        value @ ColorRange::Other(_) => {
            return Err(ColorSourceError::UnsupportedRange(value.clone()));
        }
    }

    match &description.white_point {
        ColorWhitePoint::D65 => {}
        ColorWhitePoint::Unknown
            if matches!(assumption, Some(ColorSourceProfileAssumption::D65))
                && matches!(description.primaries, ColorPrimaries::Bt709) => {}
        ColorWhitePoint::Unknown => return Err(ColorSourceError::UnknownWhitePoint),
        value => return Err(ColorSourceError::UnsupportedWhitePoint(value.clone())),
    }

    match &description.bit_depth {
        ColorBitDepth::Unknown => Err(ColorSourceError::UnknownBitDepth),
        // CC1 §2.1: named and numeric integer depths are equivalent, and only
        // 8..=16 integer samples enter the managed path.
        value => match value.integer_bits() {
            Some(bits) if (8..=16).contains(&bits) => Ok(()),
            _ => Err(ColorSourceError::UnsupportedBitDepth(value.clone())),
        },
    }
}

/// Validate the remaining columns of one CC8 §2.1 profile row.
///
/// The primaries and transfer are already known to be the row's, because the
/// row was selected by that pair. Everything else is checked against the row
/// itself rather than against a literal, so widening §2.1's table is a change
/// to `cc8_hdr::CC8_SOURCE_PROFILES` and nothing else.
///
/// Each failure is one of the CC8-specific typed reasons, never a CC1 one:
/// CC1's allowed-value strings are pinned verbatim by
/// `cc1_fixtures::cc1_source_profile_classification_is_typed_and_actionable`
/// and describe a different allowed set.
fn validate_cc8_hdr_source_fields(
    description: &ColorDescription,
    row: &'static Cc8SourceProfile,
    assumption: Option<ColorSourceProfileAssumption>,
) -> Result<(), ColorSourceError> {
    match &description.matrix {
        ColorMatrix::Unknown => return Err(ColorSourceError::UnknownMatrix),
        value if row.accepts_matrix(value.wire()) => {}
        // §2.1: `bt2020_cl`, `ictcp`, and `chroma_derived_*` are "explicit CC8
        // failures, not guesses".
        value => return Err(ColorSourceError::UnsupportedHdrMatrix(value.clone())),
    }

    match &description.range {
        ColorRange::Unknown => return Err(ColorSourceError::UnknownRange),
        value if row.accepts_range(value.wire()) => {}
        value => return Err(ColorSourceError::UnsupportedHdrRange(value.clone())),
    }

    match &description.white_point {
        value if row.accepts_white_point(value.wire()) => {}
        // §2.1: "The CC1 D65 rule carries over unchanged: an unknown white
        // point on an otherwise supported HDR tuple may use the normative D65
        // value only through an explicit `profile_assumption` recorded in the
        // colour status and proof. The raw source metadata stays `Unknown`. No
        // code may rewrite it." Nothing here writes to `description`.
        ColorWhitePoint::Unknown
            if matches!(assumption, Some(ColorSourceProfileAssumption::D65)) => {}
        ColorWhitePoint::Unknown => return Err(ColorSourceError::UnknownWhitePoint),
        value => return Err(ColorSourceError::UnsupportedHdrWhitePoint(value.clone())),
    }

    match &description.bit_depth {
        ColorBitDepth::Unknown => Err(ColorSourceError::UnknownBitDepth),
        // §2.1's 10-bit floor, and CC1 §2.1's rule that a named depth and its
        // numeric spelling are the same declared depth.
        value => match value.integer_bits() {
            Some(bits) if row.accepts_integer_depth(bits) => Ok(()),
            _ => Err(ColorSourceError::UnsupportedHdrBitDepth(value.clone())),
        },
    }
}

/// The colour descriptions used by the current application SDR pipeline.
///
/// The three stages remain separate even while their defaults are fixed. The
/// pipeline state is explicit on new project JSON. When reading a pre-CC1
/// context whose state is absent, the custom deserializer migrates the old CC0
/// working placeholder (CC1 §4.2) but only stamps `managed_sdr_v1` when the
/// migrated working, monitoring, and delivery descriptions all match the
/// managed SDR targets; a custom or incompatible context remains `Legacy` so
/// later conformance can block it rather than guessing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ColorContext {
    /// The versioned semantics of the working/monitoring/delivery descriptions.
    pub pipeline_state: ColorPipelineState,
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

#[derive(Debug, Deserialize)]
struct ColorContextWire {
    #[serde(default)]
    working: ColorDescription,
    #[serde(default)]
    monitoring: ColorDescription,
    #[serde(default)]
    delivery: ColorDescription,
    #[serde(default)]
    pipeline_state: Option<ColorPipelineState>,
}

impl<'de> Deserialize<'de> for ColorContext {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ColorContextWire::deserialize(deserializer)?;
        let legacy = Self {
            pipeline_state: ColorPipelineState::Legacy,
            working: wire.working,
            monitoring: wire.monitoring,
            delivery: wire.delivery,
        };

        Ok(match wire.pipeline_state {
            Some(pipeline_state) => Self {
                pipeline_state,
                ..legacy
            },
            // CC0's working description was a storage placeholder rather than
            // an executed transform.  Migrate that stage independently so a
            // project-custom monitoring or delivery target is not allowed to
            // strand the project on the old 8-bit working contract.  The
            // other stages are upgraded only when they are still the exact
            // application defaults; genuinely custom values remain intact.
            None if legacy.working_matches_cc0_placeholder() => {
                let current = Self::sdr_rec709();
                let monitoring_matches_default = legacy.monitoring_matches_cc0_default();
                let delivery_matches_default = legacy.delivery_matches_cc0_default();
                let migrated = Self {
                    pipeline_state: ColorPipelineState::Legacy,
                    working: current.working,
                    monitoring: if monitoring_matches_default {
                        current.monitoring
                    } else {
                        legacy.monitoring.clone()
                    },
                    delivery: if delivery_matches_default {
                        current.delivery
                    } else {
                        legacy.delivery.clone()
                    },
                };
                // CC1 §4: `managed_sdr_v1` is a claim about the whole
                // pipeline, so it is only stamped when working, monitoring,
                // and delivery all match the managed SDR targets after the
                // migration. A project-custom monitoring or delivery target
                // keeps the migrated working description but stays `legacy`
                // so conformance blocks it with the exact incompatible field
                // instead of silently claiming the managed contract.
                let managed = migrated.working_matches_managed_sdr()
                    && migrated.monitoring_matches_managed_sdr()
                    && migrated.delivery_matches_managed_sdr();
                Self {
                    pipeline_state: if managed {
                        ColorPipelineState::ManagedSdrV1
                    } else {
                        ColorPipelineState::Legacy
                    },
                    ..migrated
                }
            }
            None => legacy,
        })
    }
}

impl Default for ColorContext {
    fn default() -> Self {
        Self::sdr_rec709()
    }
}

impl ColorContext {
    /// Construct the current application SDR Rec.709 colour context.
    ///
    /// Working and monitoring use the managed SDR linear-RGB/float16 working
    /// representation and full range. Delivery uses BT.709 matrix coefficients
    /// and limited range for the current video export path. All three
    /// descriptions use D65 and application-default provenance.
    #[must_use]
    pub fn sdr_rec709() -> Self {
        Self {
            pipeline_state: ColorPipelineState::ManagedSdrV1,
            working: ColorDescription::application_default_with(
                ColorMatrix::Rgb,
                ColorRange::Full,
                ColorTransfer::Linear,
                ColorBitDepth::Float16,
            ),
            monitoring: ColorDescription::application_default_with(
                ColorMatrix::Rgb,
                ColorRange::Full,
                ColorTransfer::Bt709,
                ColorBitDepth::Float16,
            ),
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

    /// Whether this context selects the CC1 managed state and compatible
    /// working, monitoring, and delivery targets. Explicit user overrides are
    /// accepted when their semantic fields match the managed targets and
    /// their confidence is valid and nonzero.
    #[must_use]
    pub fn is_managed_sdr_compatible(&self) -> bool {
        matches!(self.pipeline_state, ColorPipelineState::ManagedSdrV1)
            && self.working_matches_managed_sdr()
            && self.monitoring_matches_managed_sdr()
            && self.delivery_matches_managed_sdr()
    }

    /// Whether the working description is semantically the CC1 target with an
    /// accepted explicit authority and nonzero valid confidence.
    #[must_use]
    pub fn working_matches_managed_sdr(&self) -> bool {
        color_description_matches_managed(&self.working, &Self::sdr_rec709().working)
    }

    /// Whether the monitoring description is semantically the CC1 target with
    /// an accepted explicit authority and nonzero valid confidence.
    #[must_use]
    pub fn monitoring_matches_managed_sdr(&self) -> bool {
        color_description_matches_managed(&self.monitoring, &Self::sdr_rec709().monitoring)
    }

    /// Whether the delivery description is semantically the CC1 target with
    /// an accepted explicit authority and nonzero valid confidence.
    #[must_use]
    pub fn delivery_matches_managed_sdr(&self) -> bool {
        color_description_matches_managed(&self.delivery, &Self::sdr_rec709().delivery)
    }

    fn working_matches_cc0_placeholder(&self) -> bool {
        self.working == ColorDescription::application_default(ColorMatrix::Rgb, ColorRange::Full)
    }

    fn monitoring_matches_cc0_default(&self) -> bool {
        self.monitoring == ColorDescription::application_default(ColorMatrix::Rgb, ColorRange::Full)
    }

    fn delivery_matches_cc0_default(&self) -> bool {
        self.delivery
            == ColorDescription::application_default(ColorMatrix::Bt709, ColorRange::Limited)
    }
}

fn color_description_matches_managed(
    actual: &ColorDescription,
    expected: &ColorDescription,
) -> bool {
    actual.confidence_is_valid()
        && actual.confidence_basis_points > 0
        && matches!(
            actual.provenance,
            ColorProvenance::ApplicationDefault | ColorProvenance::UserOverride
        )
        && actual.primaries == expected.primaries
        && actual.transfer == expected.transfer
        && actual.matrix == expected.matrix
        && actual.range == expected.range
        && actual.white_point == expected.white_point
        && actual.bit_depth == expected.bit_depth
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn old_cc0_context_json() -> Value {
        let old = json!({
            "working": ColorDescription::application_default(ColorMatrix::Rgb, ColorRange::Full),
            "monitoring": ColorDescription::application_default(ColorMatrix::Rgb, ColorRange::Full),
            "delivery": ColorDescription::application_default(
                ColorMatrix::Bt709,
                ColorRange::Limited,
            ),
        });
        old
    }

    #[test]
    fn default_is_managed_float16_and_serializes_pipeline_state() {
        let context = ColorContext::default();

        assert_eq!(context.pipeline_state, ColorPipelineState::ManagedSdrV1);
        assert_eq!(context.working.transfer, ColorTransfer::Linear);
        assert_eq!(context.working.matrix, ColorMatrix::Rgb);
        assert_eq!(context.working.range, ColorRange::Full);
        assert_eq!(context.working.bit_depth, ColorBitDepth::Float16);
        assert_eq!(context.monitoring.transfer, ColorTransfer::Bt709);
        assert_eq!(context.monitoring.bit_depth, ColorBitDepth::Float16);
        assert_eq!(context.delivery.bit_depth, ColorBitDepth::Eight);

        let serialized = serde_json::to_value(&context).expect("context should serialize");
        assert_eq!(serialized["pipeline_state"], "managed_sdr_v1");
        assert_eq!(serialized["working"]["transfer"], "linear");
        assert_eq!(serialized["working"]["bit_depth"], "float16");
    }

    #[test]
    fn exact_old_cc0_context_migrates_to_managed_sdr_v1() {
        let migrated: ColorContext =
            serde_json::from_value(old_cc0_context_json()).expect("old CC0 context should migrate");

        assert_eq!(migrated, ColorContext::sdr_rec709());
        assert_eq!(migrated.pipeline_state, ColorPipelineState::ManagedSdrV1);
    }

    #[test]
    fn old_working_placeholder_migrates_without_rewriting_custom_targets() {
        let mut old = old_cc0_context_json();
        old["monitoring"]["transfer"] = json!("bt1886");
        old["monitoring"]["provenance"] = json!("user_override");
        old["delivery"]["transfer"] = json!("bt1886");
        old["delivery"]["provenance"] = json!("user_override");

        let migrated: ColorContext =
            serde_json::from_value(old).expect("custom CC0 context should remain readable");

        assert_eq!(
            migrated.pipeline_state,
            ColorPipelineState::Legacy,
            "custom monitoring/delivery targets must not be stamped managed_sdr_v1"
        );
        assert!(!migrated.is_managed_sdr_compatible());
        assert_eq!(
            migrated.working,
            ColorContext::sdr_rec709().working,
            "the old working placeholder must become the managed Float16 working target"
        );
        assert_eq!(migrated.monitoring.transfer, ColorTransfer::Bt1886);
        assert_eq!(
            migrated.monitoring.provenance,
            ColorProvenance::UserOverride
        );
        assert_eq!(migrated.delivery.transfer, ColorTransfer::Bt1886);
        assert_eq!(migrated.delivery.provenance, ColorProvenance::UserOverride);
    }

    #[test]
    fn custom_monitoring_target_migrates_working_but_remains_legacy() {
        let mut old = old_cc0_context_json();
        old["monitoring"]["transfer"] = json!("bt1886");
        old["monitoring"]["provenance"] = json!("user_override");

        let migrated: ColorContext =
            serde_json::from_value(old).expect("custom monitoring should remain readable");

        assert_eq!(migrated.pipeline_state, ColorPipelineState::Legacy);
        assert_eq!(migrated.working, ColorContext::sdr_rec709().working);
        assert!(migrated.working_matches_managed_sdr());
        assert!(!migrated.monitoring_matches_managed_sdr());
        assert!(migrated.delivery_matches_managed_sdr());
        assert_eq!(migrated.monitoring.transfer, ColorTransfer::Bt1886);
    }

    #[test]
    fn custom_delivery_target_migrates_working_but_remains_legacy() {
        let mut old = old_cc0_context_json();
        old["delivery"]["range"] = json!("full");
        old["delivery"]["provenance"] = json!("user_override");

        let migrated: ColorContext =
            serde_json::from_value(old).expect("custom delivery should remain readable");

        assert_eq!(migrated.pipeline_state, ColorPipelineState::Legacy);
        assert_eq!(migrated.working, ColorContext::sdr_rec709().working);
        assert!(!migrated.delivery_matches_managed_sdr());
        assert_eq!(migrated.delivery.range, ColorRange::Full);
    }

    #[test]
    fn absent_pipeline_state_is_legacy_per_cc1_section_4() {
        #[derive(Debug, Deserialize)]
        struct Wire {
            #[serde(default)]
            pipeline_state: ColorPipelineState,
        }

        assert_eq!(ColorPipelineState::default(), ColorPipelineState::Legacy);
        let decoded: Wire =
            serde_json::from_value(json!({})).expect("absent pipeline state should decode");
        assert_eq!(decoded.pipeline_state, ColorPipelineState::Legacy);
    }

    #[test]
    fn custom_context_without_pipeline_state_remains_legacy() {
        let mut custom = old_cc0_context_json();
        custom["working"]["transfer"] = json!("gamma22");
        custom["working"]["provenance"] = json!("user_override");

        let decoded: ColorContext =
            serde_json::from_value(custom).expect("custom legacy context should remain readable");

        assert_eq!(decoded.pipeline_state, ColorPipelineState::Legacy);
        assert_eq!(decoded.working.transfer, ColorTransfer::Gamma22);
        assert_eq!(decoded.working.provenance, ColorProvenance::UserOverride);
    }

    #[test]
    fn explicit_legacy_state_is_preserved_even_for_old_defaults() {
        let mut old = old_cc0_context_json();
        old["pipeline_state"] = json!("legacy");

        let decoded: ColorContext =
            serde_json::from_value(old).expect("explicit legacy state should be preserved");

        assert_eq!(decoded.pipeline_state, ColorPipelineState::Legacy);
        assert_eq!(
            decoded.working,
            ColorDescription::application_default(ColorMatrix::Rgb, ColorRange::Full)
        );
    }

    #[test]
    fn future_pipeline_state_round_trips_as_other() {
        let mut value =
            serde_json::to_value(ColorContext::default()).expect("context should serialize");
        value["pipeline_state"] = json!("managed_sdr_v2");

        let decoded: ColorContext =
            serde_json::from_value(value).expect("future pipeline state should remain readable");

        assert_eq!(
            decoded.pipeline_state,
            ColorPipelineState::Other("managed_sdr_v2".to_owned())
        );
        let round_trip = serde_json::to_value(decoded).expect("future state should serialize");
        assert_eq!(round_trip["pipeline_state"], "managed_sdr_v2");
    }

    #[test]
    fn missing_context_fields_are_legacy_unknown_not_guessed_rec709() {
        let decoded: ColorContext =
            serde_json::from_value(json!({})).expect("empty legacy context should remain readable");

        assert_eq!(decoded.pipeline_state, ColorPipelineState::Legacy);
        assert!(decoded.working.is_unknown());
        assert!(decoded.monitoring.is_unknown());
        assert!(decoded.delivery.is_unknown());
    }

    #[test]
    fn source_classifier_accepts_cc1_profiles_and_reports_typed_failures() {
        let rec709 = ColorContext::sdr_rec709().delivery;
        assert_eq!(
            classify_source(&rec709).expect("delivery should be rec709 video"),
            ColorSourceProfile::Rec709Video
        );

        let srgb = ColorDescription {
            primaries: ColorPrimaries::Srgb,
            transfer: ColorTransfer::Srgb,
            matrix: ColorMatrix::Identity,
            range: ColorRange::Full,
            white_point: ColorWhitePoint::D65,
            bit_depth: ColorBitDepth::Ten,
            confidence_basis_points: COLOR_CONFIDENCE_MAX_BASIS_POINTS,
            provenance: ColorProvenance::StreamMetadata,
        };
        assert_eq!(
            classify_source(&srgb).expect("sRGB should be accepted"),
            ColorSourceProfile::SrgbFull
        );

        let unknown_white_point = ColorDescription {
            white_point: ColorWhitePoint::Unknown,
            ..rec709.clone()
        };
        assert_eq!(
            classify_source(&unknown_white_point),
            Err(ColorSourceError::UnknownWhitePoint)
        );
        assert_eq!(
            classify_source_with_assumption(
                &unknown_white_point,
                Some(ColorSourceProfileAssumption::D65)
            )
            .expect("explicit D65 assumption should be accepted"),
            ColorSourceProfile::Rec709Video
        );

        let unsupported = ColorDescription {
            primaries: ColorPrimaries::Bt2020,
            ..rec709
        };
        let error = classify_source(&unsupported).expect_err("BT.2020 must be rejected");
        assert_eq!(error.code(), "unsupported_source_primaries");
        assert_eq!(error.field(), "primaries");
        assert_eq!(error.observed(), "Bt2020");
        assert!(error.allowed_values().contains("bt709"));
        assert!(error.actionable_message().contains("relink"));
    }

    #[test]
    fn named_and_numeric_integer_bit_depths_are_equivalent() {
        use std::collections::hash_map::DefaultHasher;

        fn hash_of(value: &ColorBitDepth) -> u64 {
            let mut hasher = DefaultHasher::new();
            value.hash(&mut hasher);
            hasher.finish()
        }

        for (bits, named) in [
            (8_u16, ColorBitDepth::Eight),
            (10, ColorBitDepth::Ten),
            (12, ColorBitDepth::Twelve),
            (16, ColorBitDepth::Sixteen),
        ] {
            let numeric = ColorBitDepth::Integer(bits);
            assert_eq!(numeric, named, "CC1 §2.1 equivalence for {bits} bits");
            assert_eq!(named, numeric);
            assert_eq!(
                hash_of(&numeric),
                hash_of(&named),
                "Hash must stay consistent with Eq for {bits} bits"
            );
            assert_eq!(ColorBitDepth::integer(bits), named);
            assert_eq!(numeric.integer_bits(), u8::try_from(bits).ok());
            assert_eq!(named.integer_bits(), u8::try_from(bits).ok());

            let encoded = serde_json::to_value(&numeric).expect("depth should serialize");
            assert_eq!(encoded, json!(bits));
            let decoded: ColorBitDepth =
                serde_json::from_value(encoded).expect("depth should deserialize");
            assert_eq!(decoded, named);
            assert_eq!(decoded, numeric);
        }

        assert_eq!(ColorBitDepth::integer(17), ColorBitDepth::Integer(17));
        assert_eq!(ColorBitDepth::Integer(17).integer_bits(), Some(17));
        assert_eq!(ColorBitDepth::Integer(300).integer_bits(), None);
        assert_eq!(ColorBitDepth::Float16.integer_bits(), None);
        assert_eq!(ColorBitDepth::Unknown.integer_bits(), None);
        assert_ne!(ColorBitDepth::Eight, ColorBitDepth::Ten);
        assert_ne!(ColorBitDepth::Float16, ColorBitDepth::Sixteen);
        assert_ne!(
            ColorBitDepth::Other("eight".to_owned()),
            ColorBitDepth::Eight
        );
    }

    #[test]
    fn numeric_and_named_source_bit_depths_classify_identically() {
        let rec709 = ColorContext::sdr_rec709().delivery;
        for depth in [
            ColorBitDepth::Eight,
            ColorBitDepth::Integer(8),
            ColorBitDepth::Ten,
            ColorBitDepth::Integer(10),
            ColorBitDepth::Integer(9),
            ColorBitDepth::Sixteen,
            ColorBitDepth::Integer(16),
        ] {
            let description = ColorDescription {
                bit_depth: depth.clone(),
                ..rec709.clone()
            };
            assert_eq!(
                classify_source(&description),
                Ok(ColorSourceProfile::Rec709Video),
                "depth {depth:?} must be accepted"
            );
        }

        for depth in [
            ColorBitDepth::Integer(17),
            ColorBitDepth::Integer(7),
            ColorBitDepth::Integer(300),
            ColorBitDepth::Float16,
            ColorBitDepth::Float32,
        ] {
            let description = ColorDescription {
                bit_depth: depth.clone(),
                ..rec709.clone()
            };
            assert_eq!(
                classify_source(&description),
                Err(ColorSourceError::UnsupportedBitDepth(depth.clone())),
                "depth {depth:?} must be rejected"
            );
        }
    }

    fn hdr_description(transfer: ColorTransfer) -> ColorDescription {
        ColorDescription {
            primaries: ColorPrimaries::Bt2020,
            transfer,
            matrix: ColorMatrix::Bt2020Ncl,
            range: ColorRange::Limited,
            white_point: ColorWhitePoint::D65,
            bit_depth: ColorBitDepth::Ten,
            confidence_basis_points: COLOR_CONFIDENCE_MAX_BASIS_POINTS,
            provenance: ColorProvenance::StreamMetadata,
        }
    }

    /// CC8 §2.1's two rows, on every column the table lists.
    #[test]
    fn cc8_source_classifier_accepts_the_two_hdr_profiles_on_every_listed_column() {
        for (transfer, expected) in [
            (ColorTransfer::Smpte2084, ColorSourceProfile::PqRec2020),
            (ColorTransfer::AribStdB67, ColorSourceProfile::HlgRec2020),
        ] {
            let base = hdr_description(transfer.clone());
            assert_eq!(classify_source(&base), Ok(expected));
            assert!(expected.is_hdr());
            assert_eq!(
                expected.cc8_row().map(|row| row.transfer),
                Some(match transfer {
                    ColorTransfer::Smpte2084 => "smpte2084",
                    _ => "arib_std_b67",
                })
            );

            // §2.1's `Matrix` column: `bt2020_ncl` or `rgb`, both spellings.
            for matrix in [ColorMatrix::Bt2020Ncl, ColorMatrix::Rgb] {
                assert_eq!(
                    classify_source(&ColorDescription {
                        matrix,
                        ..base.clone()
                    }),
                    Ok(expected)
                );
            }
            // §2.1's `Range` column: `limited` or `full`.
            for range in [ColorRange::Limited, ColorRange::Full] {
                assert_eq!(
                    classify_source(&ColorDescription {
                        range,
                        ..base.clone()
                    }),
                    Ok(expected)
                );
            }
            // §2.1's `Integer depth` column, at both ends and in both
            // spellings — CC1 §2.1's named/numeric equivalence, on the HDR row.
            for depth in [
                ColorBitDepth::Ten,
                ColorBitDepth::Integer(10),
                ColorBitDepth::Twelve,
                ColorBitDepth::Sixteen,
                ColorBitDepth::Integer(16),
            ] {
                assert_eq!(
                    classify_source(&ColorDescription {
                        bit_depth: depth.clone(),
                        ..base.clone()
                    }),
                    Ok(expected),
                    "depth {depth:?} is inside §2.1's 10..=16",
                );
            }
        }
    }

    /// CC8 §2.1's closed set: every HDR-adjacent tuple outside it fails typed.
    #[test]
    fn cc8_source_classifier_rejects_every_hdr_adjacent_tuple_outside_the_closed_set() {
        let pq = hdr_description(ColorTransfer::Smpte2084);

        // §2.1's 10-bit floor. Both codes below it, in both spellings.
        for depth in [
            ColorBitDepth::Eight,
            ColorBitDepth::Integer(8),
            ColorBitDepth::Integer(9),
            ColorBitDepth::Integer(17),
            ColorBitDepth::Float16,
        ] {
            let error = classify_source(&ColorDescription {
                bit_depth: depth.clone(),
                ..pq.clone()
            })
            .expect_err("a depth outside §2.1's column must be rejected");
            assert_eq!(error, ColorSourceError::UnsupportedHdrBitDepth(depth));
            assert_eq!(error.code(), "unsupported_hdr_source_bit_depth");
            assert_eq!(error.field(), "bit_depth");
            assert_eq!(error.allowed_values(), CC8_HDR_DEPTH_ALLOWED);
            assert!(error.recovery_action().contains("CC8 HDR source profile"));
        }

        // §2.1's named matrix rejections.
        for matrix in [
            ColorMatrix::Bt2020Cl,
            ColorMatrix::Ictcp,
            ColorMatrix::ChromaDerivedNcl,
            ColorMatrix::ChromaDerivedCl,
            ColorMatrix::Bt709,
            ColorMatrix::Identity,
        ] {
            let error = classify_source(&ColorDescription {
                matrix: matrix.clone(),
                ..pq.clone()
            })
            .expect_err("a matrix outside §2.1's column must be rejected");
            assert_eq!(error, ColorSourceError::UnsupportedHdrMatrix(matrix));
            assert_eq!(error.code(), "unsupported_hdr_source_matrix");
            assert_eq!(error.allowed_values(), CC8_HDR_MATRIX_ALLOWED);
        }

        // P3 primaries in any combination, including with an HDR transfer.
        // The pair is not one of §2.1's rows, so this is diagnosed by CC1's
        // primaries rule — the tuple is not HDR, it is unsupported.
        for primaries in [ColorPrimaries::DisplayP3, ColorPrimaries::DciP3] {
            let error = classify_source(&ColorDescription {
                primaries: primaries.clone(),
                ..pq.clone()
            })
            .expect_err("P3 primaries are an explicit CC8 failure");
            assert_eq!(error, ColorSourceError::UnsupportedPrimaries(primaries));
        }

        // §2.1's mismatched pairs, both directions, reported against the field
        // CC1 already names them by so no CC1 fixture moves.
        for transfer in [ColorTransfer::Smpte2084, ColorTransfer::AribStdB67] {
            let error = classify_source(&ColorDescription {
                primaries: ColorPrimaries::Bt709,
                matrix: ColorMatrix::Bt709,
                transfer: transfer.clone(),
                ..pq.clone()
            })
            .expect_err("an HDR transfer on BT.709 primaries is an explicit failure");
            assert_eq!(error, ColorSourceError::UnsupportedTransfer(transfer));
        }
        let error = classify_source(&ColorDescription {
            transfer: ColorTransfer::Bt709,
            ..pq.clone()
        })
        .expect_err("Rec.2020 primaries with a BT.709 transfer is an explicit failure");
        assert_eq!(
            error,
            ColorSourceError::UnsupportedPrimaries(ColorPrimaries::Bt2020)
        );

        // A range outside `limited`/`full` is the HDR range reason.
        let other_range = ColorRange::Other("mpeg".to_owned());
        assert_eq!(
            classify_source(&ColorDescription {
                range: other_range.clone(),
                ..pq
            }),
            Err(ColorSourceError::UnsupportedHdrRange(other_range)),
        );
    }

    /// CC8 §2.1's white-point and unknown-field rules on an HDR tuple.
    #[test]
    fn cc8_source_classifier_keeps_the_d65_rule_and_unknown_fields_honest() {
        let pq = hdr_description(ColorTransfer::Smpte2084);

        // A known-but-wrong white point on an HDR tuple, and the D65 rule.
        let error = classify_source(&ColorDescription {
            white_point: ColorWhitePoint::Dci,
            ..pq.clone()
        })
        .expect_err("a non-D65 white point on an HDR tuple must be rejected");
        assert_eq!(
            error,
            ColorSourceError::UnsupportedHdrWhitePoint(ColorWhitePoint::Dci)
        );
        assert_eq!(error.allowed_values(), CC8_HDR_WHITE_POINT_ALLOWED);

        let unknown_white_point = ColorDescription {
            white_point: ColorWhitePoint::Unknown,
            ..pq.clone()
        };
        assert_eq!(
            classify_source(&unknown_white_point),
            Err(ColorSourceError::UnknownWhitePoint),
            "§2.1 carries CC1's D65 rule over: no silent inference",
        );
        assert_eq!(
            classify_source_with_assumption(
                &unknown_white_point,
                Some(ColorSourceProfileAssumption::D65)
            ),
            Ok(ColorSourceProfile::PqRec2020),
        );
        assert_eq!(
            unknown_white_point.white_point,
            ColorWhitePoint::Unknown,
            "§2.1: the raw source metadata stays Unknown. No code may rewrite it.",
        );

        // Unknown fields stay honest rather than being inferred from the rest
        // of an otherwise complete HDR tuple.
        for (description, expected) in [
            (
                ColorDescription {
                    matrix: ColorMatrix::Unknown,
                    ..pq.clone()
                },
                ColorSourceError::UnknownMatrix,
            ),
            (
                ColorDescription {
                    range: ColorRange::Unknown,
                    ..pq.clone()
                },
                ColorSourceError::UnknownRange,
            ),
            (
                ColorDescription {
                    bit_depth: ColorBitDepth::Unknown,
                    ..pq
                },
                ColorSourceError::UnknownBitDepth,
            ),
        ] {
            assert_eq!(classify_source(&description), Err(expected));
        }
    }

    /// CC8 §7: the classification's identity serialises and round-trips, and
    /// the wire spellings are the authority table's.
    #[test]
    fn hdr_profile_ids_come_from_the_cc8_authority_table() {
        for profile in [
            ColorSourceProfile::PqRec2020,
            ColorSourceProfile::HlgRec2020,
        ] {
            let encoded = serde_json::to_value(profile).expect("a profile serialises");
            assert_eq!(
                encoded,
                Value::String(profile.id().to_owned()),
                "the serde form and `id()` must be the same string",
            );
            let decoded: ColorSourceProfile =
                serde_json::from_value(encoded).expect("a profile round-trips");
            assert_eq!(decoded, profile);
            assert!(cc8_source_profile_by_id(profile.id()).is_some());
        }
        assert_eq!(ColorSourceProfile::PqRec2020.id(), "pq_rec2020");
        assert_eq!(ColorSourceProfile::HlgRec2020.id(), "hlg_rec2020");
        assert!(!ColorSourceProfile::Rec709Video.is_hdr());
        assert!(!ColorSourceProfile::SrgbFull.is_hdr());
        assert!(ColorSourceProfile::Rec709Video.cc8_row().is_none());
    }

    #[test]
    fn supported_profile_ids_phrase_names_every_profile() {
        for profile in [
            ColorSourceProfile::Rec709Video,
            ColorSourceProfile::SrgbFull,
            ColorSourceProfile::PqRec2020,
            ColorSourceProfile::HlgRec2020,
        ] {
            assert!(
                SUPPORTED_PROFILE_IDS.contains(profile.id()),
                "the combination allowed-value phrase omits {}",
                profile.id(),
            );
        }
    }

    #[test]
    fn color_tag_wire_spellings_are_the_serde_forms() {
        // `wire()` is what CC8 §2.1's table is matched against, so it must be
        // the same string `Serialize` writes for every variant, including the
        // `Other` passthrough that keeps a newer decoder's value readable.
        for value in [
            ColorPrimaries::Bt709,
            ColorPrimaries::Bt2020,
            ColorPrimaries::DisplayP3,
            ColorPrimaries::Other("acesap0".to_owned()),
        ] {
            let encoded = serde_json::to_value(&value).expect("a tag serialises");
            assert_eq!(encoded, Value::String(value.wire().to_owned()));
        }
        assert_eq!(ColorTransfer::AribStdB67.wire(), "arib_std_b67");
        assert_eq!(ColorMatrix::Bt2020Ncl.wire(), "bt2020_ncl");
        assert_eq!(ColorWhitePoint::D65.wire(), "d65");
        assert_eq!(ColorRange::Unknown.wire(), "unknown");
    }

    #[test]
    fn unknown_white_point_does_not_mask_an_unsupported_profile_tuple() {
        let description = ColorDescription {
            primaries: ColorPrimaries::Srgb,
            transfer: ColorTransfer::Bt709,
            matrix: ColorMatrix::Rgb,
            range: ColorRange::Full,
            white_point: ColorWhitePoint::Unknown,
            bit_depth: ColorBitDepth::Eight,
            confidence_basis_points: COLOR_CONFIDENCE_MAX_BASIS_POINTS,
            provenance: ColorProvenance::StreamMetadata,
        };

        assert!(matches!(
            classify_source_with_assumption(&description, Some(ColorSourceProfileAssumption::D65)),
            Err(ColorSourceError::UnsupportedCombination { .. })
        ));
        assert_eq!(description.white_point, ColorWhitePoint::Unknown);
    }
}
