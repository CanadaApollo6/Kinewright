use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
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

    #[must_use]
    pub const fn resolution(self, source: (u32, u32)) -> (u32, u32) {
        match self.aspect() {
            None => source,
            Some(aspect) => aspect.resolution(),
        }
    }

    #[must_use]
    pub fn export_settings(
        self,
        document: &Document,
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
            delivery_color: document.color_context.delivery.clone(),
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
    horizontal_focus_percent: u8,
    vertical_focus_percent: u8,
) -> Result<DeliveryConformanceReport, DeliveryVariantError> {
    let delivery = document_for_delivery_profile(
        document,
        profile,
        horizontal_focus_percent,
        vertical_focus_percent,
    )?;
    let settings = profile.export_settings(&delivery, ExportCancellation::default());
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
    if !delivery_color_supported(&settings.delivery_color) {
        issues.push(QaIssue {
            severity: QaSeverity::Error,
            code: "unsupported_delivery_color".to_owned(),
            message: "Current libx264/YUV420P export requires explicit 8-bit SDR Rec.709 delivery colour metadata (BT.709 primaries, transfer, and matrix; limited range; D65; nonzero confidence; application-default or user-override provenance).".to_owned(),
            asset: None,
            track: None,
            clip: None,
            range: None,
        });
    }
    let delivery_color = settings.delivery_color.clone();
    Ok(DeliveryConformanceReport {
        profile,
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

fn delivery_color_supported(color: &ColorDescription) -> bool {
    color.confidence_is_valid()
        && color.confidence_basis_points > 0
        && matches!(
            &color.provenance,
            ColorProvenance::ApplicationDefault | ColorProvenance::UserOverride
        )
        && matches!(&color.primaries, ColorPrimaries::Bt709)
        && matches!(&color.transfer, ColorTransfer::Bt709)
        && matches!(&color.matrix, ColorMatrix::Bt709)
        && matches!(&color.range, ColorRange::Limited)
        && matches!(&color.white_point, ColorWhitePoint::D65)
        && matches!(&color.bit_depth, ColorBitDepth::Eight)
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
            let settings = profile.export_settings(&document, ExportCancellation::default());
            let report = delivery_conformance(&source, profile, 40, 60).unwrap();

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
        let report =
            delivery_conformance(&fixture(), DeliveryProfile::SourceMaster, 50, 50).unwrap();

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

        let report =
            delivery_conformance(&document, DeliveryProfile::SourceMaster, 50, 50).unwrap();

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

        let report =
            delivery_conformance(&document, DeliveryProfile::SourceMaster, 50, 50).unwrap();
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
        let settings =
            DeliveryProfile::Youtube1080p.export_settings(&source, ExportCancellation::default());

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
            let settings = profile.export_settings(&document, ExportCancellation::default());

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

        let settings =
            DeliveryProfile::SourceMaster.export_settings(&document, ExportCancellation::default());
        assert_eq!(settings.delivery_color, custom_delivery);

        let report = delivery_conformance(&document, DeliveryProfile::SourceMaster, 50, 50)
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
        let user_override =
            delivery_conformance(&document, DeliveryProfile::SourceMaster, 50, 50).unwrap();
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
            let report =
                delivery_conformance(&document, DeliveryProfile::SourceMaster, 50, 50).unwrap();

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

        let report = delivery_conformance(&document, DeliveryProfile::SourceMaster, 50, 50)
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

        let report = delivery_conformance(&document, DeliveryProfile::SourceMaster, 50, 50)
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

        let report = delivery_conformance(&document, DeliveryProfile::SourceMaster, 50, 50)
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

        let report = delivery_conformance(&document, DeliveryProfile::SourceMaster, 50, 50)
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

        let report = delivery_conformance(&document, DeliveryProfile::SourceMaster, 50, 50)
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

        let report =
            delivery_conformance(&document, DeliveryProfile::SourceMaster, 50, 50).unwrap();
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

    #[test]
    fn delivery_conformance_report_serializes_typed_delivery_color() {
        let mut document = fixture();
        document.media_pool[0].path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let report = delivery_conformance(&document, DeliveryProfile::SourceMaster, 50, 50)
            .expect("fixture should produce a conformance report");
        let serialized = serde_json::to_value(&report).expect("report should serialize");

        assert_eq!(
            serialized["delivery_color"],
            serde_json::to_value(&report.delivery_color).expect("colour should serialize")
        );
    }
}
