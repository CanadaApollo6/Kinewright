use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    AssetId, CaptionPreset, ClipContent, ClipId, ColorBitDepth, ColorMatrix, ColorPrimaries,
    ColorProvenance, ColorRange, ColorTransfer, Document, Effect, EffectCompatibilityStage,
    MediaKind, ParamValue, TimeCode, TitlePixelBounds, TrackId, effect_compatibility_stage,
    title_layout,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QaSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QaIssue {
    pub severity: QaSeverity,
    pub code: String,
    pub message: String,
    /// Source asset associated with an issue that is not specific to one
    /// timeline clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(default)]
    pub asset: Option<AssetId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<std::ops::Range<TimeCode>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QaReport {
    pub document_duration: TimeCode,
    pub issues: Vec<QaIssue>,
}

impl QaReport {
    #[must_use]
    pub fn count(&self, severity: QaSeverity) -> usize {
        self.issues
            .iter()
            .filter(|issue| issue.severity == severity)
            .count()
    }

    #[must_use]
    pub fn export_ready(&self) -> bool {
        self.count(QaSeverity::Error) == 0
    }
}

/// Run deterministic structural and delivery checks against a document snapshot.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn qa_document(document: &Document) -> QaReport {
    let mut issues = Vec::new();
    let referenced_media_assets = document
        .timeline_referenced_media_assets()
        .into_iter()
        .map(|asset| asset.id)
        .collect::<std::collections::HashSet<_>>();
    if document.duration <= TimeCode::ZERO {
        issues.push(issue(
            QaSeverity::Error,
            "empty_timeline",
            "The timeline has no renderable duration.",
            None,
            None,
            None,
        ));
    }
    for asset in &document.media_pool {
        let mut source_color_concerns = Vec::new();
        if matches!(asset.kind, MediaKind::Video | MediaKind::AudioVideo) {
            if matches!(asset.color_description.primaries, ColorPrimaries::Unknown) {
                source_color_concerns.push("primaries are unknown");
            }
            if matches!(asset.color_description.transfer, ColorTransfer::Unknown) {
                source_color_concerns.push("transfer is unknown");
            }
            if matches!(asset.color_description.matrix, ColorMatrix::Unknown) {
                source_color_concerns.push("matrix is unknown");
            }
            if matches!(asset.color_description.range, ColorRange::Unknown) {
                source_color_concerns.push("range is unknown");
            }
            if matches!(asset.color_description.bit_depth, ColorBitDepth::Unknown) {
                source_color_concerns.push("bit depth is unknown");
            }
            match asset.color_description.provenance {
                ColorProvenance::Unknown => source_color_concerns.push("provenance is unknown"),
                ColorProvenance::Inferred => source_color_concerns.push("provenance is inferred"),
                _ => {}
            }
        }
        if !source_color_concerns.is_empty() {
            issues.push(QaIssue {
                severity: QaSeverity::Warning,
                code: "source_color_metadata_uncertain".to_owned(),
                message: format!(
                    "Asset {} ({:?}) needs source colour review: {}.",
                    asset.id,
                    asset.name,
                    source_color_concerns.join(", ")
                ),
                asset: Some(asset.id),
                track: None,
                clip: None,
                range: None,
            });
        }
        if referenced_media_assets.contains(&asset.id) && !asset.path.exists() {
            issues.push(QaIssue {
                severity: QaSeverity::Error,
                code: "missing_media".to_owned(),
                message: format!("Media file is missing: {}", asset.path.display()),
                asset: Some(asset.id),
                track: None,
                clip: None,
                range: None,
            });
        }
    }

    let mut has_audible_media = false;
    for track in &document.tracks {
        let mut previous_end = TimeCode::ZERO;
        let mut previous_was_media = false;
        for clip in &track.clips {
            for effect in &clip.effects {
                if let Some(stage) = effect_compatibility_stage(&effect.name) {
                    let message = match stage {
                        EffectCompatibilityStage::LegacyDisplayCoded => format!(
                            "Clip {} uses the legacy display-coded {} effect. It remains loadable through the compatibility path, but is outside the managed SDR primary conformance claim.",
                            clip.id, effect.name
                        ),
                        EffectCompatibilityStage::PostPrimaryLut => format!(
                            "Clip {} uses the post-primary {} compatibility LUT stage. It remains supported, but is outside the managed SDR primary conformance claim.",
                            clip.id, effect.name
                        ),
                    };
                    issues.push(issue(
                        QaSeverity::Warning,
                        stage.issue_code(),
                        message,
                        Some(track.id),
                        Some(clip.id),
                        None,
                    ));
                }
            }
            has_audible_media |= matches!(clip.content, ClipContent::Media)
                && clip.speed_percent == 100
                && document.asset(clip.asset).is_some_and(|asset| {
                    matches!(asset.kind, MediaKind::Audio | MediaKind::AudioVideo)
                });
            let duration = document.clip_duration(clip).unwrap_or(TimeCode::ZERO);
            let end = TimeCode(clip.timeline_start.0.saturating_add(duration.0));
            if clip.timeline_start > previous_end {
                issues.push(issue(
                    QaSeverity::Warning,
                    "track_gap",
                    format!(
                        "Track {} has a gap from frame {} to {}.",
                        track.id, previous_end.0, clip.timeline_start.0
                    ),
                    Some(track.id),
                    Some(clip.id),
                    Some(previous_end..clip.timeline_start),
                ));
            } else if previous_was_media
                && matches!(clip.content, ClipContent::Media)
                && clip.transition_in.is_none()
            {
                issues.push(issue(
                    QaSeverity::Info,
                    "abrupt_cut",
                    format!("Clip {} starts with a hard cut.", clip.id),
                    Some(track.id),
                    Some(clip.id),
                    Some(clip.timeline_start..TimeCode(clip.timeline_start.0.saturating_add(1))),
                ));
            }
            if matches!(clip.content, ClipContent::Media) && clip.speed_percent != 100 {
                issues.push(issue(
                    QaSeverity::Warning,
                    "retimed_audio_muted",
                    format!(
                        "Clip {} is retimed to {}%; its audio is muted.",
                        clip.id, clip.speed_percent
                    ),
                    Some(track.id),
                    Some(clip.id),
                    Some(clip.timeline_start..end),
                ));
            }
            if let ClipContent::Title(title) = &clip.content {
                let layout = title_layout(title, document.resolution);
                if layout.is_none() {
                    issues.push(issue(
                        QaSeverity::Error,
                        "title_layout_unavailable",
                        format!(
                            "Title clip {} cannot fit the {}x{} delivery safe area.",
                            clip.id, document.resolution.0, document.resolution.1
                        ),
                        Some(track.id),
                        Some(clip.id),
                        Some(clip.timeline_start..end),
                    ));
                }
                let Some(preset) = title.caption_preset else {
                    previous_end = end.max(previous_end);
                    previous_was_media = false;
                    continue;
                };
                if let Some(layout) = layout {
                    let animated = transformed_title_bounds(
                        layout.visual_bounds,
                        &clip.effects,
                        document.resolution,
                    );
                    if !layout.safe_bounds.contains(animated) {
                        issues.push(issue(
                            QaSeverity::Error,
                            "caption_outside_safe_area",
                            format!(
                                "Caption clip {} reaches [{},{}..{},{}] outside delivery safe area [{},{}..{},{}].",
                                clip.id,
                                animated.left,
                                animated.top,
                                animated.right,
                                animated.bottom,
                                layout.safe_bounds.left,
                                layout.safe_bounds.top,
                                layout.safe_bounds.right,
                                layout.safe_bounds.bottom,
                            ),
                            Some(track.id),
                            Some(clip.id),
                            Some(clip.timeline_start..end),
                        ));
                    }
                }
                let maximum = match preset {
                    CaptionPreset::Social => 32,
                    CaptionPreset::Clean | CaptionPreset::Minimal => 42,
                };
                if title.text.chars().count() > maximum {
                    issues.push(issue(
                        QaSeverity::Warning,
                        "caption_line_too_long",
                        format!(
                            "Caption clip {} exceeds the {maximum}-character {:?} preset target.",
                            clip.id, preset
                        ),
                        Some(track.id),
                        Some(clip.id),
                        Some(clip.timeline_start..end),
                    ));
                }
                let nominal_half_second = i64::from(document.fps.numerator())
                    / i64::from(document.fps.denominator().max(1))
                    / 2;
                if duration.0 < nominal_half_second.max(1) {
                    issues.push(issue(
                        QaSeverity::Warning,
                        "caption_too_brief",
                        format!("Caption clip {} may be too brief to read.", clip.id),
                        Some(track.id),
                        Some(clip.id),
                        Some(clip.timeline_start..end),
                    ));
                }
            }
            previous_end = end.max(previous_end);
            previous_was_media = matches!(clip.content, ClipContent::Media);
        }
    }
    if document.duration > TimeCode::ZERO && !has_audible_media {
        issues.push(issue(
            QaSeverity::Info,
            "no_audible_media",
            "The timeline has no real-time media clip with an audio stream.",
            None,
            None,
            None,
        ));
    }
    QaReport {
        document_duration: document.duration,
        issues,
    }
}

#[allow(clippy::similar_names)]
fn transformed_title_bounds(
    bounds: TitlePixelBounds,
    effects: &[Effect],
    resolution: (u32, u32),
) -> TitlePixelBounds {
    let mut scale_percent = 100_i64;
    let mut minimum_x_percent = 0_i64;
    let mut maximum_x_percent = 0_i64;
    let mut minimum_y_percent = 0_i64;
    let mut maximum_y_percent = 0_i64;
    for effect in effects.iter().filter(|effect| effect.name == "transform") {
        let (_, maximum_scale) = parameter_range(effect, "scale_percent", 100);
        scale_percent = ceil_div(scale_percent.saturating_mul(maximum_scale.max(1)), 100);
        let (minimum_x, maximum_x) = parameter_range(effect, "x_percent", 0);
        minimum_x_percent = minimum_x_percent.saturating_add(minimum_x);
        maximum_x_percent = maximum_x_percent.saturating_add(maximum_x);
        let (minimum_y, maximum_y) = parameter_range(effect, "y_percent", 0);
        minimum_y_percent = minimum_y_percent.saturating_add(minimum_y);
        maximum_y_percent = maximum_y_percent.saturating_add(maximum_y);
    }
    let center_x = i64::from(resolution.0) / 2;
    let center_y = i64::from(resolution.1) / 2;
    TitlePixelBounds {
        left: saturating_i32(transform_floor(
            i64::from(bounds.left),
            center_x,
            scale_percent,
            minimum_x_percent,
            i64::from(resolution.0),
        )),
        top: saturating_i32(transform_floor(
            i64::from(bounds.top),
            center_y,
            scale_percent,
            maximum_y_percent.saturating_neg(),
            i64::from(resolution.1),
        )),
        right: saturating_i32(transform_ceil(
            i64::from(bounds.right),
            center_x,
            scale_percent,
            maximum_x_percent,
            i64::from(resolution.0),
        )),
        bottom: saturating_i32(transform_ceil(
            i64::from(bounds.bottom),
            center_y,
            scale_percent,
            minimum_y_percent.saturating_neg(),
            i64::from(resolution.1),
        )),
    }
}

fn parameter_range(effect: &Effect, name: &str, default: i64) -> (i64, i64) {
    let base = effect
        .parameters
        .get(name)
        .and_then(|value| match value {
            ParamValue::Integer(value) => Some(*value),
            _ => None,
        })
        .unwrap_or(default);
    effect.keyframes.get(name).map_or((base, base), |curve| {
        curve
            .keyframes
            .iter()
            .map(|keyframe| keyframe.value)
            .fold((base, base), |(minimum, maximum), value| {
                (minimum.min(value), maximum.max(value))
            })
    })
}

fn transform_floor(
    value: i64,
    center: i64,
    scale_percent: i64,
    offset_percent: i64,
    extent: i64,
) -> i64 {
    center
        .saturating_add(
            value
                .saturating_sub(center)
                .saturating_mul(scale_percent)
                .div_euclid(100),
        )
        .saturating_add(extent.saturating_mul(offset_percent).div_euclid(100))
}

fn transform_ceil(
    value: i64,
    center: i64,
    scale_percent: i64,
    offset_percent: i64,
    extent: i64,
) -> i64 {
    center
        .saturating_add(ceil_div(
            value.saturating_sub(center).saturating_mul(scale_percent),
            100,
        ))
        .saturating_add(ceil_div(extent.saturating_mul(offset_percent), 100))
}

fn ceil_div(numerator: i64, denominator: i64) -> i64 {
    numerator
        .saturating_neg()
        .div_euclid(denominator)
        .saturating_neg()
}

fn saturating_i32(value: i64) -> i32 {
    i32::try_from(value).unwrap_or(if value.is_negative() {
        i32::MIN
    } else {
        i32::MAX
    })
}

fn issue(
    severity: QaSeverity,
    code: &str,
    message: impl Into<String>,
    track: Option<TrackId>,
    clip: Option<ClipId>,
    range: Option<std::ops::Range<TimeCode>>,
) -> QaIssue {
    QaIssue {
        severity,
        code: code.to_owned(),
        message: message.into(),
        asset: None,
        track,
        clip,
        range,
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        CaptionCue, CaptionMotion, Clip, MediaAsset, MediaKind, Rational, Title, Track, TrackKind,
        animated_caption_operations, apply_batch,
    };

    use super::*;

    #[test]
    fn qa_reports_missing_media_gaps_retiming_and_caption_readability() {
        let asset = MediaAsset {
            id: crate::AssetId(1),
            path: "definitely-missing-m31-fixture.mp4".into(),
            name: "fixture".to_owned(),
            duration: TimeCode(60),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::AudioVideo,
            resolution: Some((1920, 1080)),
            source_fingerprint: crate::MediaSourceFingerprint::default(),
            color_description: crate::ColorDescription::default(),
        };
        let document = Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![
                    Clip {
                        id: ClipId(1),
                        asset: asset.id,
                        source_range: TimeCode(0)..TimeCode(30),
                        content: ClipContent::Media,
                        timeline_start: TimeCode(10),
                        effects: Vec::new(),
                        transition_in: None,
                        link: None,
                        audio_gain_tenth_db: 0,
                        audio_fade_in_frames: TimeCode::ZERO,
                        audio_fade_out_frames: TimeCode::ZERO,
                        speed_percent: 200,
                    },
                    Clip {
                        id: ClipId(2),
                        asset: crate::AssetId::default(),
                        source_range: TimeCode(0)..TimeCode(4),
                        content: ClipContent::Title(Title {
                            caption_preset: Some(CaptionPreset::Social),
                            text: "A caption line that is intentionally far too long".to_owned(),
                            ..Title::default()
                        }),
                        timeline_start: TimeCode(30),
                        effects: Vec::new(),
                        transition_in: None,
                        link: None,
                        audio_gain_tenth_db: 0,
                        audio_fade_in_frames: TimeCode::ZERO,
                        audio_fade_out_frames: TimeCode::ZERO,
                        speed_percent: 100,
                    },
                ],
            }],
            media_pool: vec![asset],
            duration: TimeCode(34),
            ..Document::default()
        };
        let report = qa_document(&document);
        for code in [
            "missing_media",
            "track_gap",
            "retimed_audio_muted",
            "caption_line_too_long",
            "caption_too_brief",
        ] {
            assert!(report.issues.iter().any(|issue| issue.code == code));
        }
        assert!(!report.export_ready());
    }

    #[test]
    fn qa_missing_media_scopes_to_timeline_references_including_audio() {
        let offline = |id, kind| MediaAsset {
            id: crate::AssetId(id),
            path: format!("definitely-missing-qa-scope-{id}.mov").into(),
            name: format!("fixture-{id}"),
            duration: TimeCode(30),
            fps: Rational::new(30, 1).unwrap(),
            kind,
            resolution: Some((1920, 1080)),
            source_fingerprint: crate::MediaSourceFingerprint::default(),
            color_description: crate::ColorDescription::default(),
        };
        let media_clip = |id| Clip {
            id: ClipId(id),
            asset: crate::AssetId(id),
            source_range: TimeCode::ZERO..TimeCode(30),
            content: ClipContent::Media,
            timeline_start: TimeCode::ZERO,
            effects: Vec::new(),
            transition_in: None,
            link: None,
            audio_gain_tenth_db: 0,
            audio_fade_in_frames: TimeCode::ZERO,
            audio_fade_out_frames: TimeCode::ZERO,
            speed_percent: 100,
        };
        let document = Document {
            tracks: vec![
                Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![media_clip(1)],
                },
                Track {
                    id: TrackId(2),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: vec![media_clip(2)],
                },
            ],
            media_pool: vec![
                offline(1, MediaKind::Video),
                offline(2, MediaKind::Audio),
                offline(3, MediaKind::AudioVideo),
            ],
            duration: TimeCode(30),
            ..Document::default()
        };

        let missing_assets = qa_document(&document)
            .issues
            .into_iter()
            .filter(|issue| issue.code == "missing_media")
            .filter_map(|issue| issue.asset)
            .collect::<Vec<_>>();
        assert_eq!(missing_assets, vec![crate::AssetId(1), crate::AssetId(2)]);
    }

    #[test]
    fn source_color_warning_is_typed_and_non_blocking() {
        let asset = MediaAsset {
            id: crate::AssetId(7),
            path: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
            name: "unknown-source".to_owned(),
            duration: TimeCode(1),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::Video,
            resolution: Some((1920, 1080)),
            source_fingerprint: crate::MediaSourceFingerprint::default(),
            color_description: crate::ColorDescription::unknown(),
        };
        let document = Document {
            media_pool: vec![asset],
            duration: TimeCode(1),
            ..Document::default()
        };

        let report = qa_document(&document);
        let warning = report
            .issues
            .iter()
            .find(|issue| issue.code == "source_color_metadata_uncertain")
            .expect("unknown video source colour should be visible to readiness checks");

        assert_eq!(warning.severity, QaSeverity::Warning);
        assert_eq!(warning.asset, Some(crate::AssetId(7)));
        assert!(warning.message.contains("primaries are unknown"));
        assert!(warning.message.contains("provenance is unknown"));
        assert!(report.export_ready());
    }

    #[test]
    fn unknown_white_point_alone_does_not_warn_about_source_color() {
        let asset = MediaAsset {
            id: crate::AssetId(8),
            path: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
            name: "probe-complete".to_owned(),
            duration: TimeCode(1),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::AudioVideo,
            resolution: Some((1920, 1080)),
            source_fingerprint: crate::MediaSourceFingerprint::default(),
            color_description: crate::ColorDescription {
                primaries: ColorPrimaries::Bt709,
                transfer: ColorTransfer::Bt709,
                matrix: ColorMatrix::Bt709,
                range: ColorRange::Limited,
                white_point: crate::ColorWhitePoint::Unknown,
                bit_depth: ColorBitDepth::Eight,
                confidence_basis_points: 10_000,
                provenance: ColorProvenance::StreamMetadata,
            },
        };
        let document = Document {
            media_pool: vec![asset],
            duration: TimeCode(1),
            ..Document::default()
        };

        let report = qa_document(&document);

        assert!(
            !report
                .issues
                .iter()
                .any(|issue| issue.code == "source_color_metadata_uncertain")
        );
        assert!(report.export_ready());
    }

    #[test]
    fn inferred_source_color_provenance_warns_even_when_fields_are_known() {
        let mut color_description = crate::ColorContext::sdr_rec709().delivery;
        color_description.provenance = ColorProvenance::Inferred;
        let asset = MediaAsset {
            id: crate::AssetId(9),
            path: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
            name: "inferred-source".to_owned(),
            duration: TimeCode(1),
            fps: Rational::new(30, 1).unwrap(),
            kind: MediaKind::Video,
            resolution: Some((1920, 1080)),
            source_fingerprint: crate::MediaSourceFingerprint::default(),
            color_description,
        };
        let document = Document {
            media_pool: vec![asset],
            duration: TimeCode(1),
            ..Document::default()
        };

        let report = qa_document(&document);
        let warning = report
            .issues
            .iter()
            .find(|issue| issue.code == "source_color_metadata_uncertain")
            .expect("inferred provenance should be visible to readiness checks");

        assert_eq!(warning.asset, Some(crate::AssetId(9)));
        assert!(warning.message.contains("provenance is inferred"));
        assert!(report.export_ready());
    }

    #[test]
    fn post_primary_lut_stages_are_typed_non_blocking_warnings() {
        let document = Document {
            tracks: vec![Track {
                id: TrackId(12),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(34),
                    asset: crate::AssetId::default(),
                    source_range: TimeCode::ZERO..TimeCode(30),
                    content: ClipContent::Title(crate::Title::default()),
                    timeline_start: TimeCode::ZERO,
                    effects: vec![
                        Effect {
                            id: crate::EffectId(1),
                            name: "look_lut".to_owned(),
                            parameters: std::collections::BTreeMap::new(),
                            keyframes: std::collections::BTreeMap::new(),
                        },
                        Effect {
                            id: crate::EffectId(2),
                            name: "cube_lut".to_owned(),
                            parameters: std::collections::BTreeMap::new(),
                            keyframes: std::collections::BTreeMap::new(),
                        },
                    ],
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                }],
            }],
            duration: TimeCode(30),
            ..Document::default()
        };

        let report = qa_document(&document);
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
        assert!(
            warnings
                .iter()
                .any(|issue| issue.message.contains("look_lut"))
        );
        assert!(
            warnings
                .iter()
                .any(|issue| issue.message.contains("cube_lut"))
        );
        assert!(report.export_ready());
    }

    #[test]
    fn every_caption_preset_and_builtin_motion_stays_inside_vertical_safe_area() {
        for preset in CaptionPreset::ALL {
            for motion in CaptionMotion::ALL {
                let mut document = Document {
                    resolution: (1_080, 1_920),
                    ..Document::default()
                };
                let operations = animated_caption_operations(
                    &document,
                    &[CaptionCue {
                        start: TimeCode::ZERO,
                        end: TimeCode(30),
                        text: "A readable delivery-aware caption with motion".to_owned(),
                    }],
                    preset,
                    motion,
                )
                .unwrap();
                apply_batch(&mut document, &operations).unwrap();
                let report = qa_document(&document);

                assert!(
                    !report
                        .issues
                        .iter()
                        .any(|issue| issue.code == "caption_outside_safe_area"),
                    "preset={preset:?}, motion={motion:?}, issues={:?}",
                    report.issues
                );
                assert!(report.export_ready());
            }
        }
    }

    #[test]
    fn qa_blocks_a_caption_transform_that_leaves_the_safe_area() {
        let mut document = Document {
            resolution: (1_080, 1_920),
            ..Document::default()
        };
        let operations = animated_caption_operations(
            &document,
            &[CaptionCue {
                start: TimeCode::ZERO,
                end: TimeCode(30),
                text: "Moved off screen".to_owned(),
            }],
            CaptionPreset::Social,
            CaptionMotion::Pop,
        )
        .unwrap();
        apply_batch(&mut document, &operations).unwrap();
        let transform = document.tracks[0].clips[0]
            .effects
            .iter_mut()
            .find(|effect| effect.name == "transform")
            .unwrap();
        transform
            .parameters
            .insert("x_percent".to_owned(), ParamValue::Integer(100));

        let report = qa_document(&document);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.code == "caption_outside_safe_area")
        );
        assert!(!report.export_ready());
    }
}
