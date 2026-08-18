use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    ClipContent, Document, Effect, EffectId, ExportCancellation, ExportSettings, OpError,
    ParamValue, QaIssue, QaSeverity, qa_document,
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
            track: None,
            clip: None,
            range: None,
        });
    }
    Ok(DeliveryConformanceReport {
        profile,
        container: profile.container_extension().to_owned(),
        resolution: settings.resolution,
        video_codec: settings.video_codec,
        audio_codec: settings.audio_codec,
        video_bitrate: settings.video_bitrate,
        audio_bitrate: settings.audio_bitrate,
        issues,
    })
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
        AssetId, Clip, MediaAsset, MediaKind, Rational, TimeCode, Track, TrackId, TrackKind,
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
    fn youtube_profile_uses_recommended_high_frame_rate_bitrate() {
        let mut source = fixture();
        source.fps = Rational::new(60, 1).unwrap();
        let settings =
            DeliveryProfile::Youtube1080p.export_settings(&source, ExportCancellation::default());

        assert_eq!(settings.resolution, (1920, 1080));
        assert_eq!(settings.video_bitrate, 12_000_000);
        assert_eq!(settings.audio_bitrate, 384_000);
    }
}
