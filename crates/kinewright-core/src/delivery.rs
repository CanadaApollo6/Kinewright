use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    AudioDeliveryPreset, AudioDeliveryTarget,
    ClipContent, ColorBitDepth, ColorDescription, ColorMatrix, ColorPipelineState, ColorPrimaries,
    ColorProvenance, ColorRange, ColorSourceError, ColorSourceProfileAssumption, ColorTransfer,
    ColorWhitePoint, Document, Effect, EffectId, ExportCancellation, ExportSettings, MediaKind,
    OpError, ParamValue, QaIssue, QaSeverity, TrackKind, classify_source,
    classify_source_with_assumption, qa_document,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryAspect {
    Widescreen,
    Vertical,
    Square,
}

/// The delivery encode precision, orthogonal to [`DeliveryProfile`].
///
/// A profile names a *composition* — raster, aspect, bitrate, platform. A bit
/// depth names an *encoding precision*. Folding the depth into the profile
/// would take four wire strings to eight now and to sixteen the moment a
/// second codec arrives, so it is its own enum and every existing
/// `DeliveryProfile` wire string stays byte-identical (CC6 §4.1).
///
/// The depth lives on [`crate::ExportSettings`], not in the project document:
/// [`crate::ColorContext::sdr_rec709`] pins the project's delivery description
/// to 8-bit, and this enum selects the depth when settings are materialized.
/// A 10-bit master is therefore exportable without editing the document's
/// colour contract, and `get_color_context` keeps reporting that contract.
/// The consequence is normative: a 10-bit job's
/// `settings.delivery_color.bit_depth` legitimately differs from the
/// document's, and a delivery tag check compares against the **settings**.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryEncodeDepth {
    #[default]
    Eight,
    Ten,
}

impl DeliveryEncodeDepth {
    pub const ALL: [Self; 2] = [Self::Eight, Self::Ten];

    /// Stable wire identifier used by agent and application surfaces.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Eight => "eight",
            Self::Ten => "ten",
        }
    }

    /// The integer sample depth of the encoded delivery.
    #[must_use]
    pub const fn bits(self) -> u8 {
        match self {
            Self::Eight => 8,
            Self::Ten => 10,
        }
    }

    /// The declared delivery colour depth for this lane.
    #[must_use]
    pub const fn color_bit_depth(self) -> ColorBitDepth {
        match self {
            Self::Eight => ColorBitDepth::Eight,
            Self::Ten => ColorBitDepth::Ten,
        }
    }

    /// The encoder pixel format for this lane.
    ///
    /// The pixel format is what selects libx264's High 10 profile; no
    /// `profile` option is set on either lane, because a 10-bit encode is
    /// measured byte-identical with and without `-profile:v high10` on the
    /// pinned build (CC6 §4.3).
    #[must_use]
    pub const fn pixel_format(self) -> &'static str {
        match self {
            Self::Eight => "yuv420p",
            Self::Ten => "yuv420p10le",
        }
    }
}

/// Stable export compositions. Names describe the delivery target while the
/// exact codec, raster, and bitrate contract remains inspectable by agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryProfile {
    SourceMaster,
    Youtube1080p,
    VerticalShort,
    SquareSocial,
}

impl DeliveryProfile {
    pub const ALL: [Self; 4] = [
        Self::SourceMaster,
        Self::Youtube1080p,
        Self::VerticalShort,
        Self::SquareSocial,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SourceMaster => "source_master",
            Self::Youtube1080p => "youtube_1080p",
            Self::VerticalShort => "vertical_short",
            Self::SquareSocial => "square_social",
        }
    }

    #[must_use]
    pub const fn aspect(self) -> Option<DeliveryAspect> {
        match self {
            Self::SourceMaster => None,
            Self::Youtube1080p => Some(DeliveryAspect::Widescreen),
            Self::VerticalShort => Some(DeliveryAspect::Vertical),
            Self::SquareSocial => Some(DeliveryAspect::Square),
        }
    }

    #[must_use]
    pub const fn container_extension(self) -> &'static str {
        "mp4"
    }

    /// AD0: the loudness contract a profile's audience expects.
    ///
    /// The platform profiles normalize to −14 LUFS on ingest, so a file that
    /// lands there is heard as mixed; `source_master` is a mezzanine and only
    /// measures. A job may override this with any [`AudioDeliveryPreset`].
    #[must_use]
    pub const fn default_audio_preset(self) -> AudioDeliveryPreset {
        match self {
            Self::SourceMaster => AudioDeliveryPreset::MeasureOnly,
            Self::Youtube1080p | Self::VerticalShort | Self::SquareSocial => {
                AudioDeliveryPreset::Streaming
            }
        }
    }

    #[must_use]
    pub const fn resolution(self, source: (u32, u32)) -> (u32, u32) {
        match self.aspect() {
            None => source,
            Some(aspect) => aspect.resolution(),
        }
    }

    /// Materialize the export settings for this profile at one delivery depth.
    ///
    /// `delivery_color` is the document's delivery description with
    /// `bit_depth` replaced by `depth`'s (CC6 §4.1): the document keeps
    /// declaring the project's 8-bit delivery contract while the job carries
    /// the lane it will actually encode.
    #[must_use]
    pub fn export_settings(
        self,
        document: &Document,
        depth: DeliveryEncodeDepth,
        cancellation: ExportCancellation,
    ) -> ExportSettings {
        let high_frame_rate = u64::from(document.fps.numerator())
            > u64::from(document.fps.denominator()).saturating_mul(30);
        let video_bitrate = match self {
            Self::SourceMaster => 20_000_000,
            Self::Youtube1080p => {
                if high_frame_rate {
                    12_000_000
                } else {
                    8_000_000
                }
            }
            Self::VerticalShort | Self::SquareSocial => 10_000_000,
        };
        let audio_bitrate = match self {
            Self::Youtube1080p => 384_000,
            Self::SourceMaster | Self::VerticalShort | Self::SquareSocial => 192_000,
        };
        ExportSettings {
            fps: document.fps,
            resolution: self.resolution(document.resolution),
            delivery_color: delivery_color_for_depth(document, depth),
            video_codec: "libx264".to_owned(),
            audio_codec: "aac".to_owned(),
            video_bitrate,
            audio_bitrate,
            cancellation,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeliveryConformanceReport {
    pub profile: DeliveryProfile,
    /// The delivery lane this report was produced for.
    ///
    /// Defaulted on read so a report recorded before CC6 deserializes as the
    /// 8-bit lane, which is what it meant (CC6 §9.3).
    #[serde(default)]
    #[schemars(default)]
    pub delivery_bit_depth: DeliveryEncodeDepth,
    pub container: String,
    pub resolution: (u32, u32),
    pub delivery_color: ColorDescription,
    pub video_codec: String,
    pub audio_codec: String,
    pub video_bitrate: u64,
    pub audio_bitrate: u64,
    pub issues: Vec<QaIssue>,
}

impl DeliveryConformanceReport {
    #[must_use]
    pub fn export_ready(&self) -> bool {
        !self
            .issues
            .iter()
            .any(|issue| issue.severity == QaSeverity::Error)
    }
}

impl DeliveryAspect {
    pub const ALL: [Self; 3] = [Self::Widescreen, Self::Vertical, Self::Square];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Widescreen => "16:9",
            Self::Vertical => "9:16",
            Self::Square => "1:1",
        }
    }

    #[must_use]
    pub const fn resolution(self) -> (u32, u32) {
        match self {
            Self::Widescreen => (1920, 1080),
            Self::Vertical => (1080, 1920),
            Self::Square => (1080, 1080),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeliveryVariant {
    pub aspect: DeliveryAspect,
    /// Reviewable manual/model-selected focal point. This is deliberately not
    /// presented as learned subject tracking.
    pub focus_x_percent: u8,
    pub focus_y_percent: u8,
}

impl DeliveryVariant {
    /// Create a deterministic cover-framed delivery variant.
    ///
    /// # Errors
    ///
    /// Returns an error when either focal coordinate is outside 0..=100.
    #[allow(clippy::similar_names)]
    pub const fn new(
        aspect: DeliveryAspect,
        focus_x_percent: u8,
        focus_y_percent: u8,
    ) -> Result<Self, DeliveryVariantError> {
        if focus_x_percent > 100 || focus_y_percent > 100 {
            return Err(DeliveryVariantError::InvalidFocus {
                x: focus_x_percent,
                y: focus_y_percent,
            });
        }
        Ok(Self {
            aspect,
            focus_x_percent,
            focus_y_percent,
        })
    }

    #[must_use]
    pub const fn centered(aspect: DeliveryAspect) -> Self {
        Self {
            aspect,
            focus_x_percent: 50,
            focus_y_percent: 50,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DeliveryVariantError {
    #[error("delivery focal point ({x}, {y}) must stay inside 0..=100 percent")]
    InvalidFocus { x: u8, y: u8 },
    #[error("effect id space is exhausted")]
    EffectIdExhausted,
    #[error(transparent)]
    InvalidDocument(#[from] OpError),
}

/// Materialize a stable profile and its matching export settings.
///
/// # Errors
///
/// Returns an error when focal coordinates or the source document are invalid.
pub fn document_for_delivery_profile(
    document: &Document,
    profile: DeliveryProfile,
    horizontal_focus_percent: u8,
    vertical_focus_percent: u8,
) -> Result<Document, DeliveryVariantError> {
    if let Some(aspect) = profile.aspect() {
        document_for_delivery_variant(
            document,
            DeliveryVariant::new(aspect, horizontal_focus_percent, vertical_focus_percent)?,
        )
    } else {
        document.validate()?;
        Ok(document.clone())
    }
}

/// Run structural QA against the exact document and settings that will render.
///
/// # Errors
///
/// Returns an error when the delivery document cannot be materialized.
pub fn delivery_conformance(
    document: &Document,
    profile: DeliveryProfile,
    depth: DeliveryEncodeDepth,
    horizontal_focus_percent: u8,
    vertical_focus_percent: u8,
) -> Result<DeliveryConformanceReport, DeliveryVariantError> {
    let delivery = document_for_delivery_profile(
        document,
        profile,
        horizontal_focus_percent,
        vertical_focus_percent,
    )?;
    let settings = profile.export_settings(&delivery, depth, ExportCancellation::default());
    let mut issues = qa_document(&delivery).issues;
    append_managed_pipeline_issues(&delivery, &mut issues);
    append_managed_source_issues(&delivery, &mut issues);
    if settings.resolution.0 == 0
        || settings.resolution.1 == 0
        || !settings.resolution.0.is_multiple_of(2)
        || !settings.resolution.1.is_multiple_of(2)
    {
        issues.push(QaIssue {
            severity: QaSeverity::Error,
            code: "invalid_delivery_raster".to_owned(),
            message: format!(
                "Delivery raster {}x{} must be positive and even for H.264.",
                settings.resolution.0, settings.resolution.1
            ),
            asset: None,
            track: None,
            clip: None,
            range: None,
        });
    }
    if let Some(mismatch) = delivery_color_mismatch(&settings.delivery_color) {
        issues.push(QaIssue {
            severity: QaSeverity::Error,
            code: "unsupported_delivery_color".to_owned(),
            message: format!(
                "Current libx264 export requires explicit 8-bit or 10-bit SDR Rec.709 delivery colour metadata: field={}, observed={}, allowed={}. Reset the delivery colour target explicitly.",
                mismatch.field, mismatch.observed, mismatch.allowed,
            ),
            asset: None,
            track: None,
            clip: None,
            range: None,
        });
    }
    let delivery_color = settings.delivery_color.clone();
    Ok(DeliveryConformanceReport {
        profile,
        delivery_bit_depth: depth,
        container: profile.container_extension().to_owned(),
        resolution: settings.resolution,
        delivery_color,
        video_codec: settings.video_codec,
        audio_codec: settings.audio_codec,
        video_bitrate: settings.video_bitrate,
        audio_bitrate: settings.audio_bitrate,
        issues,
    })
}

fn append_managed_pipeline_issues(document: &Document, issues: &mut Vec<QaIssue>) {
    if !matches!(
        document.color_context.pipeline_state,
        ColorPipelineState::ManagedSdrV1
    ) {
        issues.push(QaIssue {
            severity: QaSeverity::Error,
            code: "unsupported_color_pipeline_state".to_owned(),
            message: format!(
                "Managed SDR delivery requires pipeline_state=managed_sdr_v1, observed {:?}. Reset the project colour pipeline before proof or export.",
                document.color_context.pipeline_state
            ),
            asset: None,
            track: None,
            clip: None,
            range: None,
        });
    }

    if !document.color_context.working_matches_managed_sdr() {
        issues.push(QaIssue {
            severity: QaSeverity::Error,
            code: "unsupported_working_color_context".to_owned(),
            message: format!(
                "Managed SDR delivery requires the exact linear BT.709/D65 Float16 working description; observed {:?}. Reset the working colour target explicitly.",
                document.color_context.working
            ),
            asset: None,
            track: None,
            clip: None,
            range: None,
        });
    }
    if !document.color_context.monitoring_matches_managed_sdr() {
        issues.push(QaIssue {
            severity: QaSeverity::Error,
            code: "unsupported_monitoring_color_context".to_owned(),
            message: format!(
                "Managed SDR delivery requires the exact BT.709/D65 Float16 monitoring description; observed {:?}. Reset the monitoring colour target explicitly.",
                document.color_context.monitoring
            ),
            asset: None,
            track: None,
            clip: None,
            range: None,
        });
    }
}

fn append_managed_source_issues(document: &Document, issues: &mut Vec<QaIssue>) {
    let referenced_visual_assets = document
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Video)
        .flat_map(|track| &track.clips)
        .filter(|clip| matches!(clip.content, ClipContent::Media | ClipContent::Freeze(_)))
        .filter_map(|clip| document.asset(clip.asset))
        .filter(|asset| matches!(asset.kind, MediaKind::Video | MediaKind::AudioVideo));

    let mut seen = std::collections::HashSet::new();
    for asset in referenced_visual_assets {
        if !seen.insert(asset.id) {
            continue;
        }
        let error = match classify_source(&asset.color_description) {
            Ok(_) => continue,
            Err(ColorSourceError::UnknownWhitePoint) => {
                match classify_source_with_assumption(
                    &asset.color_description,
                    Some(ColorSourceProfileAssumption::D65),
                ) {
                    Ok(_) => {
                        issues.push(QaIssue {
                            severity: QaSeverity::Warning,
                            code: "source_color_profile_assumption".to_owned(),
                            message: format!(
                                "Asset {} ({}) has raw source white_point=unknown; managed SDR is using the explicit profile assumption D65. The raw metadata remains unchanged.",
                                asset.id, asset.name
                            ),
                            asset: Some(asset.id),
                            track: None,
                            clip: None,
                            range: None,
                        });
                        continue;
                    }
                    Err(error) => error,
                }
            }
            Err(error) => error,
        };
        issues.push(QaIssue {
            severity: QaSeverity::Error,
            code: "unsupported_source_color".to_owned(),
            message: format!(
                "Asset {} ({}) cannot enter the managed SDR path: code={}, field={}, observed={}, allowed={}. {}",
                asset.id,
                asset.name,
                error.code(),
                error.field(),
                error.observed(),
                error.allowed_values(),
                error.recovery_action(),
            ),
            asset: Some(asset.id),
            track: None,
            clip: None,
            range: None,
        });
    }
}

/// One delivery-colour field that the current export contract rejects.
///
/// This carries the same three facts as [`ColorSourceError`] — the field, the
/// observed value, and the allowed values — so a rejection is diagnosable
/// rather than one opaque sentence.
///
/// [`QaIssue`] still has no structured detail map, so
/// `unsupported_delivery_color` formats the three facts into `message` as
/// `field=..., observed=..., allowed=...`. A consumer that wants them as data
/// uses [`delivery_color_mismatch`] or [`delivery_color_mismatches`] directly
/// rather than parsing that text (CC6 §3.6).
///
/// **This type must never be applied to a *probed* description.** A decoded
/// H.264 stream necessarily carries [`ColorProvenance::StreamMetadata`] and an
/// unknown white point, both of which are correct for a decoded file and both
/// of which this check rejects. [`delivery_tag_check`] is the only function
/// that may compare a probed description.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeliveryColorMismatch {
    /// Stable wire field name, such as `primaries` or `bit_depth`.
    pub field: String,
    pub observed: String,
    pub allowed: String,
}

/// Every delivery-colour field the current export contract rejects, in the
/// fixed check order.
///
/// The order is `primaries → transfer → matrix → range → white_point →
/// bit_depth → provenance → confidence`. An empty vector means the
/// description conforms.
#[must_use]
pub fn delivery_color_mismatches(color: &ColorDescription) -> Vec<DeliveryColorMismatch> {
    let mut mismatches = Vec::new();
    let mut push = |field: &str, observed: String, allowed: &str| {
        mismatches.push(DeliveryColorMismatch {
            field: field.to_owned(),
            observed,
            allowed: allowed.to_owned(),
        });
    };

    if !matches!(&color.primaries, ColorPrimaries::Bt709) {
        push("primaries", format!("{:?}", color.primaries), "bt709");
    }
    if !matches!(&color.transfer, ColorTransfer::Bt709) {
        push("transfer", format!("{:?}", color.transfer), "bt709");
    }
    if !matches!(&color.matrix, ColorMatrix::Bt709) {
        push("matrix", format!("{:?}", color.matrix), "bt709");
    }
    if !matches!(&color.range, ColorRange::Limited) {
        push("range", format!("{:?}", color.range), "limited");
    }
    if !matches!(&color.white_point, ColorWhitePoint::D65) {
        push("white_point", format!("{:?}", color.white_point), "d65");
    }
    // CC1 §2.1 makes `Integer(8)` and `Eight` the same declared depth, and CC6
    // §4.1 widens the accepted set to the two managed lanes; every other depth
    // stays rejected with a typed reason.
    if !DeliveryEncodeDepth::ALL
        .iter()
        .any(|depth| color.bit_depth == depth.color_bit_depth())
    {
        push(
            "bit_depth",
            format!("{:?}", color.bit_depth),
            DELIVERY_BIT_DEPTH_ALLOWED,
        );
    }
    if !matches!(
        &color.provenance,
        ColorProvenance::ApplicationDefault | ColorProvenance::UserOverride
    ) {
        push(
            "provenance",
            format!("{:?}", color.provenance),
            "application_default or user_override",
        );
    }
    if !color.confidence_is_valid() || color.confidence_basis_points == 0 {
        push(
            "confidence_basis_points",
            color.confidence_basis_points.to_string(),
            "1..=10000",
        );
    }
    mismatches
}

/// The allowed delivery depths, as one stable phrase.
pub const DELIVERY_BIT_DEPTH_ALLOWED: &str = "8 or 10 (named eight/ten or integer 8/10)";

/// The first delivery-colour field that the current export contract rejects.
#[must_use]
pub fn delivery_color_mismatch(color: &ColorDescription) -> Option<DeliveryColorMismatch> {
    delivery_color_mismatches(color).into_iter().next()
}

/// The delivery colour description a document exports with at one depth.
///
/// The document's own delivery description is left untouched; only
/// `bit_depth` is replaced (CC6 §3.0/§4.1). This is the single entry point
/// for the delivery depth, so the codec context and the filter graph cannot
/// diverge.
#[must_use]
pub fn delivery_color_for_depth(
    document: &Document,
    depth: DeliveryEncodeDepth,
) -> ColorDescription {
    ColorDescription {
        bit_depth: depth.color_bit_depth(),
        ..document.color_context.delivery.clone()
    }
}

/// Materialize a non-destructive delivery document from a master cut.
///
/// Animated reframes authored for the requested delivery aspect are preserved.
/// Other reframes are replaced so repeated previews never stack crops.
///
/// # Errors
///
/// Returns an error for invalid source/output documents or exhausted effect ids.
pub fn document_for_delivery_variant(
    document: &Document,
    variant: DeliveryVariant,
) -> Result<Document, DeliveryVariantError> {
    document.validate()?;
    if variant.focus_x_percent > 100 || variant.focus_y_percent > 100 {
        return Err(DeliveryVariantError::InvalidFocus {
            x: variant.focus_x_percent,
            y: variant.focus_y_percent,
        });
    }
    let mut output = document.clone();
    output.resolution = variant.aspect.resolution();
    let mut next_effect_id = output
        .tracks
        .iter()
        .flat_map(|track| &track.clips)
        .flat_map(|clip| &clip.effects)
        .map(|effect| effect.id.0)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or(DeliveryVariantError::EffectIdExhausted)?;
    let (width, height) = output.resolution;
    let aspect_basis_points = u64::from(width)
        .saturating_mul(10_000)
        .saturating_add(u64::from(height) / 2)
        / u64::from(height);
    let aspect_basis_points = i64::try_from(aspect_basis_points).unwrap_or(i64::MAX);
    for clip in output.tracks.iter_mut().flat_map(|track| &mut track.clips) {
        if matches!(clip.content, ClipContent::Title(_)) {
            continue;
        }
        let authored_reframe = clip
            .effects
            .iter()
            .find(|effect| {
                effect.name == "reframe"
                    && !effect.keyframes.is_empty()
                    && effect.parameters.get("target_aspect_basis_points")
                        == Some(&ParamValue::Integer(aspect_basis_points))
            })
            .cloned();
        clip.effects.retain(|effect| effect.name != "reframe");
        if let Some(effect) = authored_reframe {
            clip.effects.push(effect);
            continue;
        }
        let mut parameters = BTreeMap::new();
        parameters.insert(
            "target_aspect_basis_points".to_owned(),
            ParamValue::Integer(aspect_basis_points),
        );
        parameters.insert(
            "focus_x_percent".to_owned(),
            ParamValue::Integer(i64::from(variant.focus_x_percent)),
        );
        parameters.insert(
            "focus_y_percent".to_owned(),
            ParamValue::Integer(i64::from(variant.focus_y_percent)),
        );
        clip.effects.push(Effect {
            id: EffectId(next_effect_id),
            name: "reframe".to_owned(),
            parameters,
            keyframes: BTreeMap::default(),
        });
        next_effect_id = next_effect_id
            .checked_add(1)
            .ok_or(DeliveryVariantError::EffectIdExhausted)?;
    }
    output.validate()?;
    Ok(output)
}

// ---------------------------------------------------------------------------
// CC6 §4.2: typed delivery rejection.
// ---------------------------------------------------------------------------

/// A managed delivery encode refused for a typed colour reason.
///
/// Modelled on [`ColorSourceError`]: every variant carries the observed value
/// and the allowed set, and exposes `code`, `field`, `observed`,
/// `allowed_values`, and `recovery_action` so a rejection is data rather than
/// one opaque sentence (CC6 §4.2).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DeliveryColorError {
    #[error(
        "unsupported_delivery_codec: video codec {observed} cannot carry managed delivery tags"
    )]
    UnsupportedCodec {
        observed: String,
        allowed: &'static str,
    },
    #[error(
        "unsupported_delivery_color: delivery colour field {} is {}, allowed {}",
        .0.field, .0.observed, .0.allowed
    )]
    UnsupportedField(DeliveryColorMismatch),
    #[error(
        "delivery_pixel_format_depth_mismatch: negotiated pixel format {observed} does not carry the declared depth, allowed {allowed}"
    )]
    PixelFormatDepthMismatch { observed: String, allowed: String },
    #[error(
        "delivery_encoder_pixel_format_unavailable: this build's encoder does not offer {allowed}; it advertises {observed}"
    )]
    EncoderPixelFormatUnavailable { observed: String, allowed: String },
}

impl DeliveryColorError {
    /// Stable machine-readable status code for agent and UI consumers.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedCodec { .. } => "unsupported_delivery_codec",
            Self::UnsupportedField(_) => "unsupported_delivery_color",
            Self::PixelFormatDepthMismatch { .. } => "delivery_pixel_format_depth_mismatch",
            Self::EncoderPixelFormatUnavailable { .. } => {
                "delivery_encoder_pixel_format_unavailable"
            }
        }
    }

    /// Stable settings field associated with the failure.
    #[must_use]
    pub fn field(&self) -> &str {
        match self {
            Self::UnsupportedCodec { .. } => "video_codec",
            Self::UnsupportedField(mismatch) => mismatch.field.as_str(),
            Self::PixelFormatDepthMismatch { .. } | Self::EncoderPixelFormatUnavailable { .. } => {
                "pixel_format"
            }
        }
    }

    /// Observed value formatted for a structured status surface.
    #[must_use]
    pub fn observed(&self) -> String {
        match self {
            Self::UnsupportedCodec { observed, .. }
            | Self::PixelFormatDepthMismatch { observed, .. }
            | Self::EncoderPixelFormatUnavailable { observed, .. } => observed.clone(),
            Self::UnsupportedField(mismatch) => mismatch.observed.clone(),
        }
    }

    /// Allowed values for the failed field.
    #[must_use]
    pub fn allowed_values(&self) -> String {
        match self {
            Self::UnsupportedCodec { allowed, .. } => (*allowed).to_owned(),
            Self::UnsupportedField(mismatch) => mismatch.allowed.clone(),
            Self::PixelFormatDepthMismatch { allowed, .. }
            | Self::EncoderPixelFormatUnavailable { allowed, .. } => allowed.clone(),
        }
    }

    /// Recovery action suitable for a visible status or agent response.
    #[must_use]
    pub const fn recovery_action(&self) -> &'static str {
        match self {
            Self::UnsupportedCodec { .. } => {
                "Select the managed libx264 delivery lane before exporting."
            }
            Self::UnsupportedField(_) => {
                "Reset the delivery colour target explicitly, or choose a supported delivery depth."
            }
            Self::PixelFormatDepthMismatch { .. } => {
                "Re-materialize the export settings so the declared delivery depth and the encoder pixel format come from one source."
            }
            Self::EncoderPixelFormatUnavailable { .. } => {
                "Export the 8-bit lane, or install an FFmpeg build whose libx264 offers the 10-bit pixel format. The export never silently falls back."
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

/// Post-export delivery verification could not produce an honest measurement.
///
/// Each variant is a refusal to publish a number, not a codec failure: a
/// verification that cannot compare like with like reports why instead of
/// reporting a difference it did not measure (CC6 §4.2/§6).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DeliveryVerificationError {
    #[error(
        "delivery_verification_not_full_resolution: sampled reference render is {observed}, allowed {allowed}"
    )]
    NotFullResolution {
        observed: String,
        allowed: &'static str,
    },
    #[error(
        "delivery_verification_plane_out_of_container: native sample {observed} exceeds the container, allowed {allowed}"
    )]
    PlaneOutOfContainer {
        observed: String,
        allowed: &'static str,
    },
    #[error(
        "delivery_verification_frame_count_mismatch: decoded {observed} frames, expected {allowed}"
    )]
    FrameCountMismatch { observed: String, allowed: String },
    #[error(
        "delivery_verification_frame_count_out_of_range: requested frame_count is {observed}, allowed {allowed}"
    )]
    FrameCountOutOfRange {
        observed: String,
        allowed: &'static str,
    },
    /// The request's budgets are not the ones
    /// [`DeliveryBudgets::for_depth`] names for the lane the export settings
    /// declare.
    ///
    /// A verification that compared a lane against another lane's budgets
    /// would still report `within_budgets`, and the number it published would
    /// be a pass against a gate nobody chose. This is a refusal rather than a
    /// silent substitution of the correct budgets, because a caller that
    /// assembled the wrong pair is asking a question about the wrong lane.
    #[error(
        "delivery_verification_budget_lane_mismatch: request carries {observed}, allowed {allowed}"
    )]
    BudgetLaneMismatch { observed: String, allowed: String },
}

impl DeliveryVerificationError {
    /// Stable machine-readable status code for agent and UI consumers.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotFullResolution { .. } => "delivery_verification_not_full_resolution",
            Self::PlaneOutOfContainer { .. } => "delivery_verification_plane_out_of_container",
            Self::FrameCountMismatch { .. } => "delivery_verification_frame_count_mismatch",
            Self::FrameCountOutOfRange { .. } => "delivery_verification_frame_count_out_of_range",
            Self::BudgetLaneMismatch { .. } => "delivery_verification_budget_lane_mismatch",
        }
    }

    /// Stable verification field associated with the failure.
    #[must_use]
    pub const fn field(&self) -> &'static str {
        match self {
            Self::NotFullResolution { .. } => "full_resolution",
            Self::PlaneOutOfContainer { .. } => "native_plane_sample",
            Self::FrameCountMismatch { .. } | Self::FrameCountOutOfRange { .. } => "frame_count",
            Self::BudgetLaneMismatch { .. } => "budgets",
        }
    }

    /// Observed value formatted for a structured status surface.
    #[must_use]
    pub fn observed(&self) -> String {
        match self {
            Self::NotFullResolution { observed, .. }
            | Self::PlaneOutOfContainer { observed, .. }
            | Self::FrameCountMismatch { observed, .. }
            | Self::FrameCountOutOfRange { observed, .. }
            | Self::BudgetLaneMismatch { observed, .. } => observed.clone(),
        }
    }

    /// Allowed values for the failed field.
    #[must_use]
    pub fn allowed_values(&self) -> String {
        match self {
            Self::NotFullResolution { allowed, .. }
            | Self::PlaneOutOfContainer { allowed, .. }
            | Self::FrameCountOutOfRange { allowed, .. } => (*allowed).to_owned(),
            Self::FrameCountMismatch { allowed, .. } | Self::BudgetLaneMismatch { allowed, .. } => {
                allowed.clone()
            }
        }
    }

    /// Recovery action suitable for a visible status or agent response.
    #[must_use]
    pub const fn recovery_action(&self) -> &'static str {
        match self {
            Self::NotFullResolution { .. } => {
                "Re-run verification against a full-resolution delivery render; a proxy raster may never be labelled a delivery reference."
            }
            Self::PlaneOutOfContainer { .. } => {
                "The decoded plane does not fit its declared container. Re-check the decoded pixel format before trusting any plane measurement."
            }
            Self::FrameCountMismatch { .. } => {
                "Verify the export wrote every frame the document implies; verification never silently samples a shorter file."
            }
            Self::FrameCountOutOfRange { .. } => {
                "Request between 1 and 16 sampled frames. A request outside the range is refused rather than quietly clamped, because a clamped sample is a different measurement reported under the number that was asked for."
            }
            Self::BudgetLaneMismatch { .. } => {
                "Build the request with DeliveryVerificationRequest::new for the depth the export settings declare, so the budgets are the ones DeliveryBudgets::for_depth names for that lane."
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

// ---------------------------------------------------------------------------
// CC6 §3.6: delivery tag checks.
// ---------------------------------------------------------------------------

/// Which side of an export a [`DeliveryTagCheck`] was taken from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryTagSource {
    /// Pre-export: the expected description was materialized from
    /// [`crate::ExportSettings`] and nothing has been written yet.
    MaterialisedExportSettings,
    /// Post-export: the observed description was probed from a written file.
    ProbedOutputFile,
}

impl DeliveryTagSource {
    /// Stable wire identifier used by agent and application surfaces.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MaterialisedExportSettings => "materialised_export_settings",
            Self::ProbedOutputFile => "probed_output_file",
        }
    }
}

/// A field the container cannot carry at all, reported instead of a mismatch.
///
/// A field that a format has no syntax for is not evidence of a wrong tag, and
/// reporting it as a mismatch would be a fabricated failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeliveryTagNotRepresentable {
    pub field: String,
    pub expected: String,
    pub reason: String,
}

/// The reason H.264/AVC cannot carry a white point, stated once.
pub const H264_WHITE_POINT_NOT_REPRESENTABLE_REASON: &str = "H.264/AVC carries colour_primaries, transfer_characteristics, and matrix_coefficients \
but no white-point field; bt709 primaries imply D65";

/// An expected delivery description compared against an observed one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeliveryTagCheck {
    /// [`DeliveryTagSource::as_str`] for the mode this check ran in.
    pub tag_source: String,
    pub expected: ColorDescription,
    pub observed: ColorDescription,
    /// Every mismatching field, in the fixed check order.
    pub mismatches: Vec<DeliveryColorMismatch>,
    /// Fields the container has no syntax for. Post-export mode only.
    pub not_representable: Vec<DeliveryTagNotRepresentable>,
    /// `mismatches.is_empty()`.
    pub conforming: bool,
}

/// Compare an expected delivery description against an observed one.
///
/// **Pre-export mode** ([`DeliveryTagSource::MaterialisedExportSettings`]):
/// nothing has been written, so there is no independent observation. The check
/// answers "would this document's delivery description be accepted by the
/// gates at this depth?" — `mismatches` is
/// [`delivery_color_mismatches`]`(expected)` and `not_representable` is empty.
/// The caller passes the materialized description as both arguments.
///
/// **Post-export mode** ([`DeliveryTagSource::ProbedOutputFile`]): `observed`
/// is the probed description of a written file, and three fields are excluded
/// from the mismatch list because a decoded file is *correct* to disagree on
/// them (CC6 §3.6):
///
/// - `white_point` — H.264/AVC has no white-point field, so it is reported as
///   not representable;
/// - `provenance` — a probed description necessarily carries
///   [`ColorProvenance::StreamMetadata`];
/// - `confidence_basis_points` — a probe states its own confidence.
///
/// This is the **only** function that may be applied to a probed description:
/// [`delivery_color_mismatch`] would reject every re-probed export.
#[must_use]
pub fn delivery_tag_check(
    expected: &ColorDescription,
    observed: &ColorDescription,
    tag_source: DeliveryTagSource,
) -> DeliveryTagCheck {
    let (mismatches, not_representable) = match tag_source {
        DeliveryTagSource::MaterialisedExportSettings => {
            (delivery_color_mismatches(expected), Vec::new())
        }
        DeliveryTagSource::ProbedOutputFile => {
            let mut mismatches = Vec::new();
            let mut compare = |field: &str, expected: String, observed: String| {
                if expected != observed {
                    mismatches.push(DeliveryColorMismatch {
                        field: field.to_owned(),
                        observed,
                        allowed: expected,
                    });
                }
            };
            compare(
                "primaries",
                format!("{:?}", expected.primaries),
                format!("{:?}", observed.primaries),
            );
            compare(
                "transfer",
                format!("{:?}", expected.transfer),
                format!("{:?}", observed.transfer),
            );
            compare(
                "matrix",
                format!("{:?}", expected.matrix),
                format!("{:?}", observed.matrix),
            );
            compare(
                "range",
                format!("{:?}", expected.range),
                format!("{:?}", observed.range),
            );
            if expected.bit_depth != observed.bit_depth {
                mismatches.push(DeliveryColorMismatch {
                    field: "bit_depth".to_owned(),
                    observed: format!("{:?}", observed.bit_depth),
                    allowed: format!("{:?}", expected.bit_depth),
                });
            }
            let not_representable = vec![DeliveryTagNotRepresentable {
                field: "white_point".to_owned(),
                expected: format!("{:?}", expected.white_point).to_lowercase(),
                reason: H264_WHITE_POINT_NOT_REPRESENTABLE_REASON.to_owned(),
            }];
            (mismatches, not_representable)
        }
    };
    DeliveryTagCheck {
        tag_source: tag_source.as_str().to_owned(),
        expected: expected.clone(),
        observed: observed.clone(),
        conforming: mismatches.is_empty(),
        mismatches,
        not_representable,
    }
}

// ---------------------------------------------------------------------------
// CC6 §6.2/§6.3: verification request, budgets, and decoded comparison.
// ---------------------------------------------------------------------------

/// Default number of frames one verification samples.
pub const DELIVERY_VERIFICATION_FRAME_COUNT: u8 = 5;
/// Hard cap on the number of frames one verification may sample.
pub const DELIVERY_VERIFICATION_MAX_FRAMES: u8 = 16;

/// Gated luma-plane maximum absolute difference, 8-bit lane, luma code units.
///
/// **Re-baselined on the CC6 fixture's own Linux measurement** (§6.3: a budget
/// is re-baselined *before* the fixture lands, never widened afterwards to make
/// a red build green). `cc6_delivery_source()` measures **2**; the margin is
/// **4.0x**, so the starting value stands unchanged.
///
/// This term and the two below it are the **codec-only** gate: the decoded
/// native `Y'` plane against a reference `Y'` through §3.4's matrix, with no
/// chroma decimation term in it at all.
pub const DELIVERY_LUMA_MAX_CODE_8BIT: u32 = 8;
/// Gated luma-plane P99 absolute difference, 8-bit lane, code millionths (3.0).
///
/// **Re-baselined on the CC6 fixture's own Linux measurement**:
/// `cc6_delivery_source()` measures **1 000 000** (1.0 code). At the draft's
/// 2 000 000 the margin was exactly **2.0x** — *at* the bar and not above it,
/// with no room for R5's cross-platform rule: the Windows build (§11.3's
/// second measurement) is a different libx264 and a P99 that lands one whole
/// code higher there would turn a healthy encode red. Widened to 3 000 000 for
/// a **3.0x** margin, which is still one third of the 10-bit lane's own
/// 8-bit-equivalent P99 budget and still numerically distinct from
/// `MONITOR_CPU_GPU_P99` (1.0).
pub const DELIVERY_LUMA_P99_CODE_8BIT_MILLIONTHS: i64 = 3_000_000;
/// Gated luma-plane mean absolute difference, 8-bit lane, code millionths (0.4).
///
/// **Re-baselined on the CC6 fixture's own Linux measurement**:
/// `cc6_delivery_source()` measures **85 247** (0.085 codes) against the
/// draft's 1 000 000, an 11.7x margin — far looser than a measurement that
/// close warrants. Tightened to 400 000, which still keeps a **4.69x** margin.
///
/// **Not** 0.5 codes, which would be the obvious halving: `MONITOR_CPU_GPU_MEAN`
/// is exactly 0.5, and §6.3 requires CC6's lane budgets to be *numerically
/// distinct* from the three compositor tolerances so a codec tolerance and a
/// compositor tolerance can never be silently substituted for one another.
/// 0.4 is the nearest value that tightens the budget and keeps them distinct.
pub const DELIVERY_LUMA_MEAN_CODE_8BIT_MILLIONTHS: i64 = 400_000;
/// Gated whole-raster RGB mean absolute difference, 8-bit lane,
/// **8-bit-equivalent** code millionths (1.75).
///
/// **Re-baselined on the CC6 fixture's own Linux measurement**:
/// `cc6_delivery_source()` measures **743 535** (0.744 codes) against the
/// draft's 1 000 000, a 1.34x margin — below the 2x rule. Widened to
/// 1 750 000 for a **2.35x** margin. Not 1 500 000, which would clear the 2x
/// rule by 0.02x and leave the same R5 Windows divergence unbudgeted as the
/// P99 term above.
///
/// This is a whole-raster **sanity floor**, not a codec gate. It is dominated
/// by 4:2:0 chroma decimation on the CC6 source, whose hard saturated edges are
/// a far larger fraction of a 320x180 raster than of the 1920x1080 chart the
/// draft value was baselined on (§6.3(c): the RGB extremes are evidence, never
/// a gate). The **luma** plane above is the codec-only gate.
pub const DELIVERY_RGB_MEAN_CODE_8BIT_MILLIONTHS: i64 = 1_750_000;
/// Gated PSNR floor, 8-bit lane, hundredths of a dB (33.00 dB).
///
/// **Re-baselined on the CC6 fixture's own Linux measurement**:
/// `cc6_delivery_source()` measures **3 686** (36.86 dB) against the draft's
/// 4 000, a shortfall of 3.14 dB. Lowered to 3 300 for **3.86 dB** of headroom,
/// and the starved-bitrate direction still trips well below it.
///
/// Like the RGB mean, PSNR is a whole-raster sanity floor computed on every RGB
/// sample, so it too is dominated by 4:2:0 chroma decimation on this source;
/// the luma plane is the codec-only gate.
pub const DELIVERY_PSNR_FLOOR_DB_HUNDREDTHS_8BIT: i32 = 3_300;

/// Gated luma-plane maximum absolute difference, 10-bit lane, luma code units.
///
/// Baselined, never derived: scaling the 8-bit constants by four would reuse a
/// compositor tolerance as a codec tolerance, which the roadmap forbids.
///
/// **Re-baselined on the CC6 fixture's own Linux measurement**:
/// `cc6_delivery_source()` measures **1** code against the draft's 32, a 32x
/// margin. Tightened to 16, which still keeps a **16.0x** margin.
pub const DELIVERY_LUMA_MAX_CODE_10BIT: u32 = 16;
/// Gated luma-plane P99 absolute difference, 10-bit lane, code millionths (4.0).
///
/// Baselined, never derived. **Re-baselined on the CC6 fixture's own Linux
/// measurement**: `cc6_delivery_source()` measures **0** — the 10-bit lane's
/// P99 luma difference is exactly zero, so the margin is infinite and the
/// draft's 8 000 000 was pure headroom. Halved to 4 000 000, which is still
/// four times the 8-bit lane's own measured P99 in 8-bit-equivalent terms.
pub const DELIVERY_LUMA_P99_CODE_10BIT_MILLIONTHS: i64 = 4_000_000;
/// Gated luma-plane mean absolute difference, 10-bit lane, code millionths (1.0).
///
/// Baselined, never derived. **Re-baselined on the CC6 fixture's own Linux
/// measurement**: `cc6_delivery_source()` measures **5 545** (0.0055 codes)
/// against the draft's 4 000 000, a 721x margin. Tightened to 1 000 000, which
/// still keeps a **180.3x** margin.
pub const DELIVERY_LUMA_MEAN_CODE_10BIT_MILLIONTHS: i64 = 1_000_000;
/// Gated whole-raster RGB mean absolute difference, 10-bit lane,
/// **8-bit-equivalent** code millionths (1.0).
///
/// Baselined, never derived. **Re-baselined on the CC6 fixture's own Linux
/// measurement**: `cc6_delivery_source()` measures **414 572**
/// 8-bit-equivalent code millionths (0.415 codes) against the draft's 500 000,
/// a 1.21x margin — below the 2x rule. Widened to 1 000 000 for a **2.41x**
/// margin, which is still strictly tighter than the 8-bit lane's 1 500 000, as
/// the lane's justification requires.
///
/// A whole-raster **sanity floor**, not a codec gate: it is dominated by 4:2:0
/// chroma decimation, which costs the 10-bit lane the same as the 8-bit one on
/// the saturated edges §11.1 mandates. The luma plane above is the codec-only
/// gate.
pub const DELIVERY_RGB_MEAN_CODE_10BIT_MILLIONTHS: i64 = 1_000_000;
/// Gated PSNR floor, 10-bit lane, hundredths of a dB on the 8-bit-equivalent
/// MSE (33.00 dB).
///
/// Baselined, never derived. **Re-baselined on the CC6 fixture's own Linux
/// measurement**: `cc6_delivery_source()` measures **3 700** (37.00 dB) against
/// the draft's 4 000, a shortfall of 3.00 dB. Lowered to 3 300 for **4.00 dB**
/// of headroom.
///
/// A whole-raster sanity floor for the same reason as the 8-bit lane's: the
/// 10-bit lane buys ~9 dB on flat fields and nothing at all on a 4:2:0 edge.
pub const DELIVERY_PSNR_FLOOR_DB_HUNDREDTHS_10BIT: i32 = 3_300;

/// Strict-legal-box excursion rate at which a decoded plane raises
/// `decoded_range_excursion` (1 %, CC6 §6.4).
pub const DECODED_RANGE_EXCEPTION_BASIS_POINTS: u32 = 100;

/// Why the whole-raster RGB extremes are evidence rather than a gate.
pub const DELIVERY_RGB_EXTREMES_NOTE: &str = "4:2:0 chroma decimation at hard saturated edges dominates these two numbers in both lanes; \
they are evidence, not a gate.";

/// Every gated number of CC6 §6.3 for one delivery lane.
///
/// A caller may not invent a looser set: [`DeliveryBudgets::for_depth`] is the
/// only constructor that names the lane's constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeliveryBudgets {
    /// Luma-plane maximum absolute difference, delivery code units at the lane
    /// depth.
    pub luma_max_code: u32,
    pub luma_p99_code_millionths: i64,
    pub luma_mean_code_millionths: i64,
    /// Whole-raster RGB mean absolute difference, 8-bit-equivalent code units.
    pub rgb_mean_code_millionths: i64,
    pub psnr_floor_db_hundredths: i32,
}

impl DeliveryBudgets {
    /// The named constants for one delivery lane.
    #[must_use]
    pub const fn for_depth(depth: DeliveryEncodeDepth) -> Self {
        match depth {
            DeliveryEncodeDepth::Eight => Self {
                luma_max_code: DELIVERY_LUMA_MAX_CODE_8BIT,
                luma_p99_code_millionths: DELIVERY_LUMA_P99_CODE_8BIT_MILLIONTHS,
                luma_mean_code_millionths: DELIVERY_LUMA_MEAN_CODE_8BIT_MILLIONTHS,
                rgb_mean_code_millionths: DELIVERY_RGB_MEAN_CODE_8BIT_MILLIONTHS,
                psnr_floor_db_hundredths: DELIVERY_PSNR_FLOOR_DB_HUNDREDTHS_8BIT,
            },
            DeliveryEncodeDepth::Ten => Self {
                luma_max_code: DELIVERY_LUMA_MAX_CODE_10BIT,
                luma_p99_code_millionths: DELIVERY_LUMA_P99_CODE_10BIT_MILLIONTHS,
                luma_mean_code_millionths: DELIVERY_LUMA_MEAN_CODE_10BIT_MILLIONTHS,
                rgb_mean_code_millionths: DELIVERY_RGB_MEAN_CODE_10BIT_MILLIONTHS,
                psnr_floor_db_hundredths: DELIVERY_PSNR_FLOOR_DB_HUNDREDTHS_10BIT,
            },
        }
    }
}

/// One post-export verification request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeliveryVerificationRequest {
    /// `1..=`[`DELIVERY_VERIFICATION_MAX_FRAMES`]; defaults to
    /// [`DELIVERY_VERIFICATION_FRAME_COUNT`].
    pub frame_count: u8,
    pub budgets: DeliveryBudgets,
    /// The tags the file is expected to carry: `ExportSettings.delivery_color`.
    pub expected_delivery: ColorDescription,
    /// AD0: what the decoded audio is expected to measure. Defaults to
    /// [`AudioDeliveryPreset::MeasureOnly`], which reports and gates nothing.
    #[serde(default)]
    #[schemars(default)]
    pub audio_target: AudioDeliveryTarget,
}

impl DeliveryVerificationRequest {
    /// A default-sampled request for one lane and one expected description,
    /// measuring audio without gating it.
    #[must_use]
    pub fn new(depth: DeliveryEncodeDepth, expected_delivery: ColorDescription) -> Self {
        Self {
            frame_count: DELIVERY_VERIFICATION_FRAME_COUNT,
            budgets: DeliveryBudgets::for_depth(depth),
            expected_delivery,
            audio_target: AudioDeliveryTarget::default(),
        }
    }

    /// The same request with an audio target attached.
    #[must_use]
    pub const fn with_audio_target(mut self, audio_target: AudioDeliveryTarget) -> Self {
        self.audio_target = audio_target;
        self
    }

    /// Refuse a request whose `frame_count` is outside
    /// `1..=`[`DELIVERY_VERIFICATION_MAX_FRAMES`].
    ///
    /// Callers validate **before** sampling: [`Self::sample_frames`] clamps so
    /// that it can stay total and infallible, and a clamp is a different
    /// measurement reported under the number that was asked for. This is the
    /// one place that difference is turned into a typed refusal carrying
    /// `code`, `field`, `observed`, and `allowed_values`.
    ///
    /// # Errors
    ///
    /// Returns [`DeliveryVerificationError::FrameCountOutOfRange`] for `0` and
    /// for anything above [`DELIVERY_VERIFICATION_MAX_FRAMES`].
    pub fn validate(&self) -> Result<(), DeliveryVerificationError> {
        if self.frame_count == 0 || self.frame_count > DELIVERY_VERIFICATION_MAX_FRAMES {
            return Err(DeliveryVerificationError::FrameCountOutOfRange {
                observed: self.frame_count.to_string(),
                allowed: "1..=16",
            });
        }
        Ok(())
    }

    /// The frames one verification samples, given the document's implied frame
    /// count `total_frames` (CC6 §6.2).
    ///
    /// Closed-form integer arithmetic: no clock, no adaptive stride. For
    /// `n >= 2` the sample always includes frame `0` and frame `T - 1`; for
    /// `n == 1` it includes frame `0` only. Duplicates, possible only when `T`
    /// is small, are removed while preserving order.
    ///
    /// **Callers validate first.** A `frame_count` outside
    /// `1..=`[`DELIVERY_VERIFICATION_MAX_FRAMES`] is clamped here so this
    /// function stays total, which means the sample it returns is *not* the
    /// one the caller asked for. [`Self::validate`] is the refusal that keeps
    /// that from being published as if it were, and every entry point calls it
    /// before sampling.
    #[must_use]
    pub fn sample_frames(&self, total_frames: u64) -> Vec<u64> {
        let requested = u64::from(self.frame_count.clamp(1, DELIVERY_VERIFICATION_MAX_FRAMES));
        if total_frames == 0 {
            return Vec::new();
        }
        if requested == 1 {
            return vec![0];
        }
        if total_frames <= requested {
            return (0..total_frames).collect();
        }
        let mut frames = Vec::with_capacity(usize::try_from(requested).unwrap_or(0));
        for index in 0..requested {
            let frame = index * (total_frames - 1) / (requested - 1);
            if frames.last() != Some(&frame) {
                frames.push(frame);
            }
        }
        frames
    }
}

/// Absolute-difference statistics for one compared channel or plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeliveryChannelDifference {
    /// Lane code units (8-bit codes on the 8-bit lane, 10-bit codes on the
    /// 10-bit lane). For the RGB channels this is reported, never gated.
    pub maximum_code_diff: u32,
    /// Lane code units, millionths. For the RGB channels reported, never gated.
    pub p99_code_diff_millionths: i64,
    /// For the luma plane: lane code units, millionths. For the RGB channels
    /// and `combined`: **8-bit-equivalent** code units (`d / 2^(bits − 8)`),
    /// millionths, so the §6.3 RGB-mean budget and the §11.2.11 lane
    /// comparison read the same scale on both lanes.
    pub mean_code_diff_millionths: i64,
}

/// The decoded-versus-reference comparison of one verified export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeliveryComparison {
    /// Project frame identities, the `i64` identity
    /// `ScopeMeasurementMetadata.project_frames` carries.
    pub frames: Vec<i64>,
    /// GATED: the luma plane at full resolution, delivery code units at the
    /// lane depth. This is codec-only error; no chroma decimation term enters
    /// it.
    pub luma: DeliveryChannelDifference,
    /// REPORTED, NOT GATED — see [`DeliveryComparison::rgb_extremes_note`].
    pub red: DeliveryChannelDifference,
    /// REPORTED, NOT GATED.
    pub green: DeliveryChannelDifference,
    /// REPORTED, NOT GATED.
    pub blue: DeliveryChannelDifference,
    /// Only `mean_code_diff_millionths` is gated.
    pub combined: DeliveryChannelDifference,
    /// GATED. `None` means the 8-bit-equivalent MSE was exactly zero.
    pub psnr_db_hundredths: Option<i32>,
    /// Measured from the decoded file's native planes, never from a raster
    /// that has already been through the scaler.
    pub decoded_ycbcr: crate::YCbCrLegalReport,
    /// Always [`DELIVERY_RGB_EXTREMES_NOTE`].
    pub rgb_extremes_note: String,
    pub budgets: DeliveryBudgets,
    pub within_budgets: bool,
}

/// One decoded, re-probed, and compared delivery encode.
///
/// Produced only by `Analysis::verify_delivery_output`. A verification is a
/// measurement: it never moves, renames, or deletes the file it read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeliveryVerification {
    pub output_path: std::path::PathBuf,
    pub delivery_bit_depth: DeliveryEncodeDepth,
    pub probed: ColorDescription,
    /// `tag_source` is always
    /// [`DeliveryTagSource::ProbedOutputFile`].
    pub tags: DeliveryTagCheck,
    pub decoded_pixel_format: String,
    pub comparison: DeliveryComparison,
    pub exceptions: Vec<crate::ColorQcException>,
    /// No `Error`-severity entry in `exceptions`. Colour only: the audio leg
    /// carries its own `technical_pass` inside [`AudioVerification::Measured`]
    /// so a loudness miss cannot masquerade as a decoded-picture budget overrun
    /// (CC6 §3.8 pins this field to exactly two colour codes).
    pub technical_pass: bool,
    /// AD0: the decoded audio, measured against `request.audio_target`.
    /// Defaulted on read so a verification recorded before AD0 deserializes as
    /// "not measured", which is what it meant.
    #[serde(default)]
    #[schemars(default)]
    pub audio: AudioVerification,
}

/// The audio leg of one delivery verification.
#[derive(
    Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema,
)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AudioVerification {
    /// Recorded before AD0, or by a backend that does not measure audio.
    #[default]
    NotMeasured,
    /// The probed file carries no audio stream. Not a failure: a picture-only
    /// deliverable has nothing to measure.
    NoAudioStream,
    /// The file has audio but it could not be decoded or measured. The reason
    /// is recorded; nothing is invented.
    Unavailable { reason: String },
    Measured(AudioDeliveryVerification),
}

impl AudioVerification {
    /// `true` unless a measured leg failed. Not-measured, no-stream, and
    /// unavailable are all "no evidence against", never a pass or a fail.
    #[must_use]
    pub fn technical_pass(&self) -> bool {
        match self {
            Self::Measured(measured) => measured.report.technical_pass,
            Self::NotMeasured | Self::NoAudioStream | Self::Unavailable { .. } => true,
        }
    }

    #[must_use]
    pub const fn measured(&self) -> Option<&AudioDeliveryVerification> {
        match self {
            Self::Measured(measured) => Some(measured),
            _ => None,
        }
    }
}

/// One decoded audio stream, measured and compared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AudioDeliveryVerification {
    /// The codec the probe reported for the stream that was decoded.
    pub probed_audio_codec: String,
    /// The channel count and rate the *file* carries, before the analysis
    /// path's fixed 48 kHz stereo conversion.
    pub probed_sample_rate: u32,
    pub probed_channels: u16,
    pub report: crate::AudioQcReport,
}

#[cfg(test)]
mod tests {
    use crate::{
        AssetId, Clip, ColorContext, ColorPrimaries, ColorTransfer, ColorWhitePoint, MediaAsset,
        MediaKind, Rational, TimeCode, Track, TrackId, TrackKind,
    };

    use super::*;

    fn fixture() -> Document {
        let asset = MediaAsset {
            id: AssetId(1),
            path: "variant.mp4".into(),
            name: "variant".to_owned(),
            duration: TimeCode(30),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::Video,
            resolution: Some((1920, 1080)),
            source_fingerprint: crate::MediaSourceFingerprint::default(),
            color_description: ColorContext::sdr_rec709().delivery,
        };
        Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: crate::ClipId(1),
                    asset: asset.id,
                    source_range: TimeCode(0)..TimeCode(30),
                    content: ClipContent::Media,
                    timeline_start: TimeCode::ZERO,
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                }],
            }],
            media_pool: vec![asset],
            duration: TimeCode(30),
            ..Document::default()
        }
    }

    #[test]
    fn every_delivery_aspect_materializes_one_valid_non_stacking_reframe() {
        for aspect in DeliveryAspect::ALL {
            let variant = DeliveryVariant::centered(aspect);
            let first = document_for_delivery_variant(&fixture(), variant).unwrap();
            let second = document_for_delivery_variant(&first, variant).unwrap();
            assert_eq!(second.resolution, aspect.resolution());
            let effects = &second.tracks[0].clips[0].effects;
            assert_eq!(
                effects
                    .iter()
                    .filter(|effect| effect.name == "reframe")
                    .count(),
                1
            );
            assert_eq!(
                effects[0].parameters["focus_x_percent"],
                ParamValue::Integer(50)
            );
        }
    }

    #[test]
    fn matching_animated_reframe_survives_delivery_materialization() {
        let mut source = fixture();
        let tracked = Effect {
            id: EffectId(41),
            name: "reframe".to_owned(),
            parameters: BTreeMap::from([
                (
                    "target_aspect_basis_points".to_owned(),
                    ParamValue::Integer(5_625),
                ),
                ("focus_x_percent".to_owned(), ParamValue::Integer(50)),
                ("focus_y_percent".to_owned(), ParamValue::Integer(42)),
            ]),
            keyframes: BTreeMap::from([(
                "focus_x_percent".to_owned(),
                crate::AutomationCurve {
                    keyframes: vec![
                        crate::Keyframe {
                            at: TimeCode::ZERO,
                            value: 50,
                            interpolation: crate::KeyframeInterpolation::EaseInOut,
                        },
                        crate::Keyframe {
                            at: TimeCode(29),
                            value: 35,
                            interpolation: crate::KeyframeInterpolation::EaseInOut,
                        },
                    ],
                },
            )]),
        };
        source.tracks[0].clips[0].effects.push(tracked.clone());

        let delivered = document_for_delivery_variant(
            &source,
            DeliveryVariant::centered(DeliveryAspect::Vertical),
        )
        .unwrap();

        assert_eq!(delivered.tracks[0].clips[0].effects, vec![tracked]);
    }

    #[test]
    fn precise_reframe_automation_survives_delivery_materialization() {
        let mut source = fixture();
        source.tracks[0].clips[0].effects.push(Effect {
            id: EffectId(41),
            name: "reframe".to_owned(),
            parameters: BTreeMap::from([
                (
                    "target_aspect_basis_points".to_owned(),
                    ParamValue::Integer(5_625),
                ),
                ("focus_x_percent".to_owned(), ParamValue::Integer(50)),
                ("focus_y_percent".to_owned(), ParamValue::Integer(50)),
            ]),
            keyframes: BTreeMap::new(),
        });
        let plan = crate::plan_subject_reframe_basis_points(
            &source,
            crate::SubjectReframeSettings {
                clip: crate::ClipId(1),
                effect: EffectId(41),
                bounds: crate::ReframeFocusBounds::default(),
                minimum_confidence_basis_points: 0,
                focus_dead_zone_percent: 0,
                maximum_focus_step_percent: 25,
            },
            &[
                crate::SubjectCenterBasisPointSample {
                    at: TimeCode::ZERO,
                    x_basis_points: 5_001,
                    y_basis_points: 4_999,
                    confidence_basis_points: 10_000,
                },
                crate::SubjectCenterBasisPointSample {
                    at: TimeCode(29),
                    x_basis_points: 5_002,
                    y_basis_points: 4_998,
                    confidence_basis_points: 10_000,
                },
            ],
        )
        .unwrap();
        crate::apply_batch(&mut source, &plan.operations).unwrap();

        let delivered = document_for_delivery_variant(
            &source,
            DeliveryVariant::centered(DeliveryAspect::Vertical),
        )
        .unwrap();
        let effect = &delivered.tracks[0].clips[0].effects[0];

        assert_eq!(
            effect.keyframes["focus_x_basis_points"].keyframes[0].value,
            5_001
        );
        assert_eq!(
            effect.keyframes["focus_x_basis_points"].keyframes[1].value,
            5_002
        );
        assert_eq!(
            effect.keyframes["focus_y_basis_points"].keyframes[1].value,
            4_998
        );
    }

    #[test]
    fn mismatched_animated_reframe_is_replaced_for_delivery_aspect() {
        let mut source = fixture();
        source.tracks[0].clips[0].effects.push(Effect {
            id: EffectId(41),
            name: "reframe".to_owned(),
            parameters: BTreeMap::from([(
                "target_aspect_basis_points".to_owned(),
                ParamValue::Integer(10_000),
            )]),
            keyframes: BTreeMap::from([(
                "focus_x_percent".to_owned(),
                crate::AutomationCurve {
                    keyframes: vec![crate::Keyframe {
                        at: TimeCode::ZERO,
                        value: 10,
                        interpolation: crate::KeyframeInterpolation::Linear,
                    }],
                },
            )]),
        });

        let delivered = document_for_delivery_variant(
            &source,
            DeliveryVariant::centered(DeliveryAspect::Vertical),
        )
        .unwrap();
        let effects = &delivered.tracks[0].clips[0].effects;

        assert_eq!(effects.len(), 1);
        assert_eq!(
            effects[0].parameters["target_aspect_basis_points"],
            ParamValue::Integer(5_625)
        );
        assert!(effects[0].keyframes.is_empty());
    }

    #[test]
    fn focal_points_are_bounded() {
        assert_eq!(
            DeliveryVariant::new(DeliveryAspect::Vertical, 101, 50),
            Err(DeliveryVariantError::InvalidFocus { x: 101, y: 50 })
        );
    }

    #[test]
    fn delivery_profiles_materialize_the_document_the_settings_describe() {
        let mut source = fixture();
        source.media_pool[0].path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        for profile in DeliveryProfile::ALL {
            let document = document_for_delivery_profile(&source, profile, 40, 60).unwrap();
            let settings = profile.export_settings(
                &document,
                DeliveryEncodeDepth::Eight,
                ExportCancellation::default(),
            );
            let report =
                delivery_conformance(&source, profile, DeliveryEncodeDepth::Eight, 40, 60).unwrap();

            assert_eq!(document.resolution, settings.resolution);
            assert_eq!(report.profile, profile);
            assert_eq!(report.container, "mp4");
            assert_eq!(report.resolution, settings.resolution);
            assert_eq!(report.delivery_color, settings.delivery_color);
            assert_eq!(report.video_codec, "libx264");
            assert_eq!(report.audio_codec, "aac");
            assert!(report.export_ready());
            assert!(report.video_bitrate > 0);
            assert!(report.audio_bitrate > 0);
        }
    }

    #[test]
    fn missing_source_media_blocks_delivery() {
        let report = delivery_conformance(
            &fixture(),
            DeliveryProfile::SourceMaster,
            DeliveryEncodeDepth::Eight,
            50,
            50,
        )
        .unwrap();

        assert!(!report.export_ready());
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.code == "missing_media")
        );
    }

    #[test]
    fn unused_offline_media_pool_asset_does_not_block_delivery() {
        let mut document = fixture();
        document.media_pool[0].path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        document.media_pool.push(MediaAsset {
            id: AssetId(2),
            path: "definitely-missing-delivery-unused-fixture.mov".into(),
            name: "unused-offline".to_owned(),
            duration: TimeCode(30),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::AudioVideo,
            resolution: Some((1920, 1080)),
            source_fingerprint: crate::MediaSourceFingerprint::default(),
            color_description: ColorContext::sdr_rec709().delivery,
        });

        let report = delivery_conformance(
            &document,
            DeliveryProfile::SourceMaster,
            DeliveryEncodeDepth::Eight,
            50,
            50,
        )
        .unwrap();

        assert!(report.export_ready());
        assert!(
            !report
                .issues
                .iter()
                .any(|issue| issue.code == "missing_media")
        );
    }

    #[test]
    fn delivery_reports_post_primary_luts_without_blocking() {
        let mut document = fixture();
        document.media_pool[0].path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        document.tracks[0].clips[0].effects.extend([
            Effect {
                id: EffectId(90),
                name: "look_lut".to_owned(),
                parameters: BTreeMap::new(),
                keyframes: BTreeMap::new(),
            },
            Effect {
                id: EffectId(91),
                name: "cube_lut".to_owned(),
                parameters: BTreeMap::from([(
                    "path".to_owned(),
                    ParamValue::Text("compatibility.cube".to_owned()),
                )]),
                keyframes: BTreeMap::new(),
            },
        ]);

        let report = delivery_conformance(
            &document,
            DeliveryProfile::SourceMaster,
            DeliveryEncodeDepth::Eight,
            50,
            50,
        )
        .unwrap();
        let warnings = report
            .issues
            .iter()
            .filter(|issue| issue.code == "legacy_lut_stage")
            .collect::<Vec<_>>();

        assert_eq!(warnings.len(), 2);
        assert!(
            warnings
                .iter()
                .all(|issue| issue.severity == QaSeverity::Warning)
        );
        assert!(report.export_ready());
    }

    #[test]
    fn youtube_profile_uses_recommended_high_frame_rate_bitrate() {
        let mut source = fixture();
        source.fps = Rational::new(60, 1).unwrap();
        let settings = DeliveryProfile::Youtube1080p.export_settings(
            &source,
            DeliveryEncodeDepth::Eight,
            ExportCancellation::default(),
        );

        assert_eq!(settings.resolution, (1920, 1080));
        assert_eq!(settings.video_bitrate, 12_000_000);
        assert_eq!(settings.audio_bitrate, 384_000);
    }

    #[test]
    fn every_delivery_profile_uses_the_document_delivery_color() {
        let document = fixture();
        assert_eq!(
            document.color_context.delivery,
            ColorContext::sdr_rec709().delivery
        );
        for profile in DeliveryProfile::ALL {
            let settings = profile.export_settings(
                &document,
                DeliveryEncodeDepth::Eight,
                ExportCancellation::default(),
            );

            assert_eq!(settings.delivery_color, document.color_context.delivery);
        }
    }

    #[test]
    fn custom_delivery_context_propagates_and_is_rejected_when_unsupported() {
        let mut document = fixture();
        document.media_pool[0].path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let mut custom_delivery = ColorContext::sdr_rec709().delivery;
        custom_delivery.transfer = ColorTransfer::Smpte2084;
        document.color_context.delivery = custom_delivery.clone();

        let settings = DeliveryProfile::SourceMaster.export_settings(
            &document,
            DeliveryEncodeDepth::Eight,
            ExportCancellation::default(),
        );
        assert_eq!(settings.delivery_color, custom_delivery);

        let report = delivery_conformance(
            &document,
            DeliveryProfile::SourceMaster,
            DeliveryEncodeDepth::Eight,
            50,
            50,
        )
        .expect("custom delivery context should still produce a report");
        assert_eq!(report.delivery_color, custom_delivery);
        assert!(!report.export_ready());
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.code == "unsupported_delivery_color")
        );
    }

    #[test]
    fn delivery_color_allows_only_application_default_or_user_override_provenance() {
        let mut document = fixture();
        document.media_pool[0].path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        document.color_context.delivery.provenance = ColorProvenance::UserOverride;
        let user_override = delivery_conformance(
            &document,
            DeliveryProfile::SourceMaster,
            DeliveryEncodeDepth::Eight,
            50,
            50,
        )
        .unwrap();
        assert!(user_override.export_ready());

        for provenance in [
            ColorProvenance::Unknown,
            ColorProvenance::ContainerMetadata,
            ColorProvenance::StreamMetadata,
            ColorProvenance::SidecarMetadata,
            ColorProvenance::Inferred,
            ColorProvenance::Other("future_project_source".to_owned()),
        ] {
            document.color_context.delivery.provenance = provenance.clone();
            let report = delivery_conformance(
                &document,
                DeliveryProfile::SourceMaster,
                DeliveryEncodeDepth::Eight,
                50,
                50,
            )
            .unwrap();

            assert!(
                !report.export_ready(),
                "unexpected supported delivery provenance: {provenance:?}"
            );
            assert!(
                report
                    .issues
                    .iter()
                    .any(|issue| issue.code == "unsupported_delivery_color")
            );
        }
    }

    #[test]
    fn managed_context_accepts_semantically_exact_user_overrides() {
        let mut document = fixture();
        document.media_pool[0].path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        document.color_context.working.provenance = ColorProvenance::UserOverride;
        document.color_context.working.confidence_basis_points = 9_500;
        document.color_context.monitoring.provenance = ColorProvenance::UserOverride;
        document.color_context.monitoring.confidence_basis_points = 9_500;

        let report = delivery_conformance(
            &document,
            DeliveryProfile::SourceMaster,
            DeliveryEncodeDepth::Eight,
            50,
            50,
        )
        .expect("user-authorized managed context should produce a report");

        assert!(report.export_ready());
        assert!(!report.issues.iter().any(|issue| {
            matches!(
                issue.code.as_str(),
                "unsupported_working_color_context" | "unsupported_monitoring_color_context"
            )
        }));
    }

    #[test]
    fn incompatible_pipeline_state_and_targets_block_with_actionable_codes() {
        let mut document = fixture();
        document.media_pool[0].path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        document.color_context.pipeline_state = ColorPipelineState::Legacy;
        document.color_context.working.bit_depth = crate::ColorBitDepth::Eight;
        document.color_context.monitoring.transfer = ColorTransfer::Bt1886;

        let report = delivery_conformance(
            &document,
            DeliveryProfile::SourceMaster,
            DeliveryEncodeDepth::Eight,
            50,
            50,
        )
        .expect("incompatible colour context should still produce a report");

        assert!(!report.export_ready());
        for code in [
            "unsupported_color_pipeline_state",
            "unsupported_working_color_context",
            "unsupported_monitoring_color_context",
        ] {
            let issue = report
                .issues
                .iter()
                .find(|issue| issue.code == code)
                .unwrap_or_else(|| panic!("expected issue code {code}"));
            assert!(issue.message.contains("Reset"));
        }
    }

    #[test]
    fn referenced_unknown_source_blocks_but_unused_bin_media_remains_inspectable() {
        let mut document = fixture();
        document.media_pool[0].path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        document.media_pool.push(MediaAsset {
            id: AssetId(2),
            path: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
            name: "unused-unknown".to_owned(),
            duration: TimeCode(30),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::Video,
            resolution: Some((1920, 1080)),
            source_fingerprint: crate::MediaSourceFingerprint::default(),
            color_description: crate::ColorDescription::unknown(),
        });
        document.media_pool[0].color_description = crate::ColorDescription::unknown();

        let report = delivery_conformance(
            &document,
            DeliveryProfile::SourceMaster,
            DeliveryEncodeDepth::Eight,
            50,
            50,
        )
        .expect("source metadata failures should be reported, not returned as errors");

        assert!(!report.export_ready());
        let blocking_assets: Vec<_> = report
            .issues
            .iter()
            .filter(|issue| issue.code == "unsupported_source_color")
            .filter_map(|issue| issue.asset)
            .collect();
        assert_eq!(blocking_assets, vec![AssetId(1)]);
    }

    #[test]
    fn audio_track_reference_to_av_media_does_not_enter_visual_source_gate() {
        let mut document = fixture();
        document.media_pool[0].path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        document.media_pool.push(MediaAsset {
            id: AssetId(2),
            path: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
            name: "audio-track-av".to_owned(),
            duration: TimeCode(30),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::AudioVideo,
            resolution: Some((1920, 1080)),
            source_fingerprint: crate::MediaSourceFingerprint::default(),
            color_description: crate::ColorDescription::unknown(),
        });
        document.tracks.push(Track {
            id: TrackId(2),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: vec![Clip {
                id: crate::ClipId(2),
                asset: AssetId(2),
                source_range: TimeCode(0)..TimeCode(30),
                content: ClipContent::Media,
                timeline_start: TimeCode::ZERO,
                effects: Vec::new(),
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: TimeCode::ZERO,
                audio_fade_out_frames: TimeCode::ZERO,
                speed_percent: 100,
            }],
        });

        let report = delivery_conformance(
            &document,
            DeliveryProfile::SourceMaster,
            DeliveryEncodeDepth::Eight,
            50,
            50,
        )
        .expect("audio-only A/V use should remain deliverable");

        assert!(report.export_ready());
        assert!(!report.issues.iter().any(|issue| {
            issue.code == "unsupported_source_color" && issue.asset == Some(AssetId(2))
        }));
    }

    #[test]
    fn unsupported_known_source_profile_blocks_managed_delivery() {
        let mut document = fixture();
        document.media_pool[0].path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        document.media_pool[0].color_description = crate::ColorDescription {
            primaries: crate::ColorPrimaries::Bt2020,
            transfer: crate::ColorTransfer::Smpte2084,
            matrix: crate::ColorMatrix::Bt2020Ncl,
            range: crate::ColorRange::Limited,
            white_point: crate::ColorWhitePoint::D65,
            bit_depth: crate::ColorBitDepth::Ten,
            confidence_basis_points: crate::COLOR_CONFIDENCE_MAX_BASIS_POINTS,
            provenance: crate::ColorProvenance::StreamMetadata,
        };

        let report = delivery_conformance(
            &document,
            DeliveryProfile::SourceMaster,
            DeliveryEncodeDepth::Eight,
            50,
            50,
        )
        .expect("unsupported source profile should produce a report");
        let issue = report
            .issues
            .iter()
            .find(|issue| issue.code == "unsupported_source_color")
            .expect("known unsupported source must be a blocking issue");
        assert_eq!(issue.asset, Some(AssetId(1)));
        assert!(issue.message.contains("unsupported_source_primaries"));
        assert!(issue.message.contains("field=primaries"));
        assert!(!report.export_ready());
    }

    #[test]
    fn uncertain_source_color_is_a_non_blocking_conformance_warning_with_asset_id() {
        let mut document = fixture();
        document.media_pool[0].path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        document.media_pool[0].color_description = crate::ColorDescription {
            primaries: ColorPrimaries::Bt709,
            transfer: ColorTransfer::Bt709,
            matrix: crate::ColorMatrix::Bt709,
            range: crate::ColorRange::Limited,
            white_point: ColorWhitePoint::Unknown,
            bit_depth: crate::ColorBitDepth::Eight,
            confidence_basis_points: crate::COLOR_CONFIDENCE_MAX_BASIS_POINTS,
            provenance: crate::ColorProvenance::Inferred,
        };

        let report = delivery_conformance(
            &document,
            DeliveryProfile::SourceMaster,
            DeliveryEncodeDepth::Eight,
            50,
            50,
        )
        .unwrap();
        let warning = report
            .issues
            .iter()
            .find(|issue| issue.code == "source_color_metadata_uncertain")
            .expect("source colour warning should flow into conformance");

        assert_eq!(warning.severity, QaSeverity::Warning);
        assert_eq!(warning.asset, Some(AssetId(1)));
        assert!(report.export_ready());
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.code == "source_color_profile_assumption")
        );
    }

    fn report_for_delivery_color(
        mutate: impl FnOnce(&mut ColorDescription),
    ) -> DeliveryConformanceReport {
        let mut document = fixture();
        document.media_pool[0].path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        mutate(&mut document.color_context.delivery);
        delivery_conformance(
            &document,
            DeliveryProfile::SourceMaster,
            DeliveryEncodeDepth::Eight,
            50,
            50,
        )
        .expect("unsupported delivery colour must be reported, not returned as an error")
    }

    /// `(field, observed, allowed, delivery description)` for every field the
    /// current export contract can reject.
    fn unsupported_delivery_color_cases()
    -> Vec<(&'static str, &'static str, &'static str, ColorDescription)> {
        let supported = ColorContext::sdr_rec709().delivery;
        vec![
            (
                "primaries",
                "Bt2020",
                "bt709",
                ColorDescription {
                    primaries: ColorPrimaries::Bt2020,
                    ..supported.clone()
                },
            ),
            (
                "transfer",
                "Smpte2084",
                "bt709",
                ColorDescription {
                    transfer: ColorTransfer::Smpte2084,
                    ..supported.clone()
                },
            ),
            (
                "matrix",
                "Rgb",
                "bt709",
                ColorDescription {
                    matrix: ColorMatrix::Rgb,
                    ..supported.clone()
                },
            ),
            (
                "range",
                "Full",
                "limited",
                ColorDescription {
                    range: ColorRange::Full,
                    ..supported.clone()
                },
            ),
            (
                "white_point",
                "D50",
                "d65",
                ColorDescription {
                    white_point: ColorWhitePoint::D50,
                    ..supported.clone()
                },
            ),
            (
                "provenance",
                "Inferred",
                "application_default or user_override",
                ColorDescription {
                    provenance: ColorProvenance::Inferred,
                    ..supported.clone()
                },
            ),
            (
                "confidence_basis_points",
                "0",
                "1..=10000",
                ColorDescription {
                    confidence_basis_points: 0,
                    ..supported
                },
            ),
        ]
    }

    #[test]
    fn unsupported_delivery_color_names_the_mismatching_field_observed_and_allowed() {
        for (field, observed, allowed, delivery) in unsupported_delivery_color_cases() {
            let report = report_for_delivery_color(|color| *color = delivery);
            let issue = report
                .issues
                .iter()
                .find(|issue| issue.code == "unsupported_delivery_color")
                .unwrap_or_else(|| panic!("delivery colour field {field} must block export"));

            assert_eq!(issue.severity, QaSeverity::Error);
            assert!(!report.export_ready());
            for expected in [
                format!("field={field}"),
                format!("observed={observed}"),
                format!("allowed={allowed}"),
            ] {
                assert!(
                    issue.message.contains(&expected),
                    "expected {expected} in {}",
                    issue.message
                );
            }
            assert!(issue.message.contains("Reset"));
        }
    }

    #[test]
    fn unsupported_delivery_color_reports_only_the_first_mismatching_field() {
        let report = report_for_delivery_color(|color| {
            color.primaries = ColorPrimaries::Bt2020;
            color.transfer = ColorTransfer::Smpte2084;
        });
        let issue = report
            .issues
            .iter()
            .find(|issue| issue.code == "unsupported_delivery_color")
            .expect("multiple mismatches must still block");

        assert!(issue.message.contains("field=primaries"));
        assert!(!issue.message.contains("field=transfer"));
    }

    #[test]
    fn delivery_bit_depth_leg_accepts_the_two_managed_lanes_and_rejects_every_other() {
        let supported = ColorContext::sdr_rec709().delivery;
        // Passing direction: both managed lanes, in both spellings. CC1 §2.1
        // makes `Integer(n)` and the named variant the same declared depth.
        for accepted in [
            ColorBitDepth::Eight,
            ColorBitDepth::Ten,
            ColorBitDepth::Integer(8),
            ColorBitDepth::Integer(10),
        ] {
            let color = ColorDescription {
                bit_depth: accepted.clone(),
                ..supported.clone()
            };
            assert_eq!(
                delivery_color_mismatch(&color),
                None,
                "{accepted:?} is a managed delivery lane"
            );
        }
        // Failing direction: every other depth, one step away.
        for rejected in [
            ColorBitDepth::Twelve,
            ColorBitDepth::Sixteen,
            ColorBitDepth::Integer(9),
            ColorBitDepth::Float16,
            ColorBitDepth::Unknown,
        ] {
            let color = ColorDescription {
                bit_depth: rejected.clone(),
                ..supported.clone()
            };
            assert_eq!(
                delivery_color_mismatch(&color),
                Some(DeliveryColorMismatch {
                    field: "bit_depth".to_owned(),
                    observed: format!("{rejected:?}"),
                    allowed: DELIVERY_BIT_DEPTH_ALLOWED.to_owned(),
                }),
                "{rejected:?} is not a managed delivery lane"
            );
        }
    }

    #[test]
    fn both_managed_delivery_depths_are_accepted_and_carry_their_own_lane() {
        for depth in DeliveryEncodeDepth::ALL {
            let mut document = fixture();
            document.media_pool[0].path =
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
            let report =
                delivery_conformance(&document, DeliveryProfile::SourceMaster, depth, 50, 50)
                    .expect("both managed lanes should produce a report");

            assert_eq!(report.delivery_bit_depth, depth);
            assert_eq!(report.delivery_color.bit_depth, depth.color_bit_depth());
            // The document keeps declaring the project's 8-bit contract.
            assert_eq!(
                document.color_context.delivery.bit_depth,
                ColorBitDepth::Eight
            );
            assert!(
                !report
                    .issues
                    .iter()
                    .any(|issue| issue.code == "unsupported_delivery_color"),
                "{depth:?} must be an accepted delivery lane"
            );
            assert!(report.export_ready());
        }
    }

    #[test]
    fn numeric_integer_eight_delivery_depth_is_accepted_like_the_named_variant() {
        let mut document = fixture();
        document.media_pool[0].path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        document.color_context.delivery.bit_depth = ColorBitDepth::Integer(8);

        let report = delivery_conformance(
            &document,
            DeliveryProfile::SourceMaster,
            DeliveryEncodeDepth::Eight,
            50,
            50,
        )
        .expect("numeric integer depth should produce a report");

        assert!(
            !report
                .issues
                .iter()
                .any(|issue| issue.code == "unsupported_delivery_color"),
            "CC1 §2.1 makes Integer(8) equivalent to Eight"
        );
        assert!(report.export_ready());
    }

    #[test]
    fn delivery_conformance_report_serializes_typed_delivery_color() {
        let mut document = fixture();
        document.media_pool[0].path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let report = delivery_conformance(
            &document,
            DeliveryProfile::SourceMaster,
            DeliveryEncodeDepth::Eight,
            50,
            50,
        )
        .expect("fixture should produce a conformance report");
        let serialized = serde_json::to_value(&report).expect("report should serialize");

        assert_eq!(
            serialized["delivery_color"],
            serde_json::to_value(&report.delivery_color).expect("colour should serialize")
        );
    }
}
