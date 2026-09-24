use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    AssetId, CaptionPreset, ClipContent, ClipId, ColorBitDepth, ColorMatrix, ColorPrimaries,
    ColorProvenance, ColorRange, ColorTransfer, Document, Effect, EffectCompatibilityStage,
    LutAssetId, MediaKind, ParamValue, TimeCode, TitlePixelBounds, TrackId,
    effect_compatibility_stage, title_layout,
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
    // AU3 §2.6: a clip that would have qualified had its track been audible.
    let mut has_audio_bearing_clip = false;
    for track in &document.tracks {
        let track_audible = document.track_audible(track.id);
        let mut gaps = document
            .track_gaps(track.id)
            .unwrap_or_default()
            .into_iter()
            .peekable();
        let mut previous_was_media = false;
        for clip in &track.clips {
            for effect in &clip.effects {
                // MO1 G6: a never-enabled effect lights no stage in the
                // compositor, so QA warns nothing — aligned with
                // `legacy_stage_active`.
                if effect_ever_enabled(effect)
                    && let Some(stage) = effect_compatibility_stage(&effect.name)
                {
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
                for lut_asset in dangling_lut_asset_references(document, effect) {
                    issues.push(issue(
                        QaSeverity::Error,
                        "missing_lut_asset",
                        format!(
                            "Clip {} effect {} references LUT asset {}, which is not registered in this project. Re-import the look or retarget the node before exporting.",
                            clip.id, effect.id, lut_asset
                        ),
                        Some(track.id),
                        Some(clip.id),
                        None,
                    ));
                }
                if let Some(inversion) = matte_band_inversion(effect) {
                    let at = TimeCode(clip.timeline_start.0.saturating_add(inversion.at.0));
                    issues.push(issue(
                        QaSeverity::Warning,
                        "matte_band_inverted_by_automation",
                        format!(
                            "Clip {} effect {} resolves a matte {} band at frame {} whose low edge is above its high edge, so that band selects nothing and the matte is empty (CC5 §2.6).",
                            clip.id,
                            effect.id,
                            inversion.bands.join(" and "),
                            at.0
                        ),
                        Some(track.id),
                        Some(clip.id),
                        Some(at..TimeCode(at.0.saturating_add(1))),
                    ));
                }
                if let Some(truncation) = curve_truncation(effect) {
                    let at = TimeCode(clip.timeline_start.0.saturating_add(truncation.at.0));
                    issues.push(issue(
                        QaSeverity::Warning,
                        "curve_truncated_by_automation",
                        format!(
                            "Clip {} effect {} resolves {} at frame {}: automation left the point list without strictly increasing x, so the curve is truncated to its longest valid prefix (CC3 §3.4).",
                            clip.id,
                            effect.id,
                            truncation.curves.join(", "),
                            at.0
                        ),
                        Some(track.id),
                        Some(clip.id),
                        Some(at..TimeCode(at.0.saturating_add(1))),
                    ));
                }
            }
            let audio_bearing = matches!(clip.content, ClipContent::Media)
                && clip.speed_percent == 100
                && document.asset(clip.asset).is_some_and(|asset| {
                    matches!(asset.kind, MediaKind::Audio | MediaKind::AudioVideo)
                });
            has_audio_bearing_clip |= audio_bearing;
            has_audible_media |= audio_bearing && track_audible;
            let duration = document.clip_duration(clip).unwrap_or(TimeCode::ZERO);
            let end = TimeCode(clip.timeline_start.0.saturating_add(duration.0));
            if let Some(gap) = gaps.next_if(|gap| gap.end == clip.timeline_start) {
                issues.push(issue(
                    QaSeverity::Warning,
                    "track_gap",
                    format!(
                        "Track {} has a gap from frame {} to {}.",
                        track.id, gap.start.0, gap.end.0
                    ),
                    Some(track.id),
                    Some(clip.id),
                    Some(gap),
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
            previous_was_media = matches!(clip.content, ClipContent::Media);
        }
    }
    issues.extend(noise_profile_issues(document));
    if document.duration > TimeCode::ZERO && !has_audible_media {
        let message = if has_audio_bearing_clip {
            "Every audio-bearing track is muted or silenced by another track's solo."
        } else {
            "The timeline has no real-time media clip with an audio stream."
        };
        issues.push(issue(
            QaSeverity::Info,
            "no_audible_media",
            message,
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

/// AU5 §2.3 rule 18: an `audio_denoise` node that reduces but has learned
/// nothing.
///
/// Raised at `Warning` for every denoise node — bus or master — whose
/// `reduction_tenth_db` resolves above 0 and whose 31 profile bands are all at
/// [`PROFILE_BAND_NEUTRAL_TENTH_DB`] or absent, which is the same thing on the
/// wire: omit-defaults never writes an unlearned node's rows.
///
/// It deliberately does **not** block export — [`QaReport::export_ready`]
/// counts `Error` only — consistent with `track_gap`, and correct, because
/// R4's neutral has already made the configuration harmless rather than
/// destructive. It is advice about a wasted node, not a gate.
///
/// "Resolves above 0" reads the whole automation curve through
/// [`parameter_range`], so a node parked at 0 that rides up under a keyframe
/// still earns the warning.
fn noise_profile_issues(document: &Document) -> Vec<QaIssue> {
    let mut issues = Vec::new();
    let owners = document
        .audio_mix
        .buses
        .iter()
        .map(|bus| (format!("bus {}", bus.id.0), &bus.effects))
        .chain(std::iter::once((
            "the master".to_owned(),
            &document.audio_mix.master.effects,
        )));
    for (owner, effects) in owners {
        for effect in effects
            .iter()
            .filter(|effect| effect.name == "audio_denoise")
        {
            let (_, maximum_reduction) = parameter_range(effect, "reduction_tenth_db", 0);
            if maximum_reduction <= 0 {
                continue;
            }
            let learned = crate::NOISE_PROFILE_PARAMETER_NAMES.iter().any(|name| {
                parameter_range(effect, name, crate::PROFILE_BAND_NEUTRAL_TENTH_DB).1
                    > crate::PROFILE_BAND_NEUTRAL_TENTH_DB
            });
            if learned {
                continue;
            }
            issues.push(issue(
                QaSeverity::Warning,
                "noise_profile_missing",
                format!(
                    "denoise node {effect_id} on {owner} has a reduction of {maximum_reduction} tenth dB but no learned profile; learn one or the node does nothing",
                    effect_id = effect.id.0,
                ),
                None,
                None,
                None,
            ));
        }
    }
    issues
}

#[allow(clippy::similar_names)]
/// Whether an effect lights any frame: no curve means the static flag;
/// a curve enables exactly the frames evaluating >= 1, and key values are
/// exact at their frames while interpolation stays within the key range —
/// so any key >= 1 enables somewhere, and all keys < 1 enables nowhere.
fn effect_ever_enabled(effect: &Effect) -> bool {
    match &effect.enabled_curve {
        None => effect.enabled,
        Some(curve) => curve.keyframes.iter().any(|key| key.value >= 1),
    }
}

/// The conservative axis-aligned bound of title bounds under every
/// ever-enabled transform, chained effect by effect (each step contains
/// the true set, so chaining stays conservative). Rotation-free effects
/// take an exact integer path; anything with rotation takes an f64 path
/// with a 0.05 px outward margin covering sampler and float error.
fn transformed_title_bounds(
    bounds: TitlePixelBounds,
    effects: &[Effect],
    resolution: (u32, u32),
) -> TitlePixelBounds {
    let mut bounds = bounds;
    for effect in effects.iter().filter(|effect| effect.name == "transform") {
        // MO1 G6: a never-enabled transform moves no pixels.
        if !effect_ever_enabled(effect) {
            continue;
        }
        bounds = transform_bounds_once(bounds, effect, resolution);
    }
    bounds
}

/// Per-axis scale extremes as numerators over 100^3: master/100 ×
/// axis/100 × fine/10000. Products are multilinear, so the extremes sit
/// on the range corners — including negative (mirroring) scales.
fn axis_scale_extremes(master: (i64, i64), axis: (i64, i64), fine: (i64, i64)) -> (i64, i64) {
    let mut extremes = (i64::MAX, i64::MIN);
    for value in [master.0, master.1] {
        for axis in [axis.0, axis.1] {
            for fine in [fine.0, fine.1] {
                let product = value.saturating_mul(axis).saturating_mul(fine);
                extremes = (extremes.0.min(product), extremes.1.max(product));
            }
        }
    }
    extremes
}

/// Offset extremes in basis points of the extent: percent × 100 + basis.
fn offset_basis_extremes(percent: (i64, i64), basis: (i64, i64)) -> (i64, i64) {
    let mut extremes = (i64::MAX, i64::MIN);
    for percent in [percent.0, percent.1] {
        for basis in [basis.0, basis.1] {
            let total = percent.saturating_mul(100).saturating_add(basis);
            extremes = (extremes.0.min(total), extremes.1.max(total));
        }
    }
    extremes
}

fn transform_bounds_once(
    bounds: TitlePixelBounds,
    effect: &Effect,
    resolution: (u32, u32),
) -> TitlePixelBounds {
    let rotation = parameter_range(effect, "rotation_centidegrees", 0);
    if rotation == (0, 0) {
        return transform_bounds_integer(bounds, effect, resolution);
    }
    transform_bounds_rotated(bounds, effect, resolution, rotation)
}

/// Exact integer path: no rotation, so every lane is multilinear and the
/// extremes sit on the corners. Matches the pre-MO1 fold on its lanes
/// (master scale, whole-percent offsets, centred anchor).
#[allow(clippy::similar_names)]
fn transform_bounds_integer(
    bounds: TitlePixelBounds,
    effect: &Effect,
    resolution: (u32, u32),
) -> TitlePixelBounds {
    let width = i64::from(resolution.0);
    let height = i64::from(resolution.1);
    let master = parameter_range(effect, "scale_percent", 100);
    let fine = parameter_range(effect, "scale_fine_hundredths", 10_000);
    let (sx_min, sx_max) = axis_scale_extremes(
        master,
        parameter_range(effect, "scale_x_percent", 100),
        fine,
    );
    let (sy_min, sy_max) = axis_scale_extremes(
        master,
        parameter_range(effect, "scale_y_percent", 100),
        fine,
    );
    let (ox_min_bp, ox_max_bp) = offset_basis_extremes(
        parameter_range(effect, "x_percent", 0),
        parameter_range(effect, "x_basis_points", 0),
    );
    let (oy_min_bp, oy_max_bp) = offset_basis_extremes(
        parameter_range(effect, "y_percent", 0),
        parameter_range(effect, "y_basis_points", 0),
    );
    // Pixels move down while offsets point up: negate into plain ranges.
    let ox_min = width.saturating_mul(ox_min_bp).div_euclid(10_000);
    let ox_max = ceil_div(width.saturating_mul(ox_max_bp), 10_000);
    let oy_min = ceil_div(height.saturating_mul(oy_max_bp), 10_000).saturating_neg();
    let oy_max = height
        .saturating_mul(oy_min_bp)
        .div_euclid(10_000)
        .saturating_neg();
    let (ax_min_bp, ax_max_bp) = parameter_range(effect, "anchor_x_basis_points", 5000);
    let (ay_min_bp, ay_max_bp) = parameter_range(effect, "anchor_y_basis_points", 5000);
    // Anchor pixels round both ways: each variant feeds the corner loop.
    let ax = [
        width.saturating_mul(ax_min_bp).div_euclid(10_000),
        ceil_div(width.saturating_mul(ax_max_bp), 10_000),
    ];
    let ay = [
        height.saturating_mul(ay_min_bp).div_euclid(10_000),
        ceil_div(height.saturating_mul(ay_max_bp), 10_000),
    ];
    let corners = [
        (i64::from(bounds.left), i64::from(bounds.top)),
        (i64::from(bounds.right), i64::from(bounds.top)),
        (i64::from(bounds.left), i64::from(bounds.bottom)),
        (i64::from(bounds.right), i64::from(bounds.bottom)),
    ];
    let mut extremes = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
    for (x, y) in corners {
        for sx in [sx_min, sx_max] {
            for sy in [sy_min, sy_max] {
                for anchor_x in ax {
                    for anchor_y in ay {
                        let down_x = scaled_div(x.saturating_sub(anchor_x), sx, false);
                        let down_y = scaled_div(y.saturating_sub(anchor_y), sy, false);
                        extremes.0 = extremes
                            .0
                            .min(anchor_x.saturating_add(down_x).saturating_add(ox_min));
                        extremes.1 = extremes
                            .1
                            .min(anchor_y.saturating_add(down_y).saturating_add(oy_min));
                        let up_x = scaled_div(x.saturating_sub(anchor_x), sx, true);
                        let up_y = scaled_div(y.saturating_sub(anchor_y), sy, true);
                        extremes.2 = extremes
                            .2
                            .max(anchor_x.saturating_add(up_x).saturating_add(ox_max));
                        extremes.3 = extremes
                            .3
                            .max(anchor_y.saturating_add(up_y).saturating_add(oy_max));
                    }
                }
            }
        }
    }
    TitlePixelBounds {
        left: saturating_i32(extremes.0),
        top: saturating_i32(extremes.1),
        right: saturating_i32(extremes.2),
        bottom: saturating_i32(extremes.3),
    }
}

/// `(delta * numerator) / 100^3`, floored (minimum side) or ceiled
/// (maximum side). The `+den-1` ceil is only valid for non-negative
/// products, so negatives ceil via negation.
fn scaled_div(delta: i64, numerator: i64, ceil: bool) -> i64 {
    const DENOMINATOR: i64 = 100_000_000;
    let product = delta.saturating_mul(numerator);
    if !ceil {
        return product.div_euclid(DENOMINATOR);
    }
    if product >= 0 {
        product
            .saturating_add(DENOMINATOR - 1)
            .div_euclid(DENOMINATOR)
    } else {
        product
            .saturating_neg()
            .div_euclid(DENOMINATOR)
            .saturating_neg()
    }
}

/// Outward conservatism margin for the rotated path, in pixels: covers
/// the 1-degree sampler (<= 0.04 px at 1920 wide) and float error.
const ROTATED_BOUND_MARGIN_PX: f64 = 0.05;

/// Rotation path: the angle range is sampled (endpoints plus every whole
/// degree; a full turn or more falls back to the circumscribed circle),
/// and every corner is transformed over the scale/offset/anchor corners
/// at each sample. Positive angles rotate clockwise on screen, matching
/// the compositor.
#[allow(clippy::similar_names, clippy::too_many_lines)]
#[allow(clippy::cast_precision_loss)] // lanes are basis points / pixels: far below 2^53.
fn transform_bounds_rotated(
    bounds: TitlePixelBounds,
    effect: &Effect,
    resolution: (u32, u32),
    rotation: (i64, i64),
) -> TitlePixelBounds {
    let width = f64::from(resolution.0);
    let height = f64::from(resolution.1);
    let master = parameter_range(effect, "scale_percent", 100);
    let fine = parameter_range(effect, "scale_fine_hundredths", 10_000);
    let (sx_min, sx_max) = axis_scale_extremes(
        master,
        parameter_range(effect, "scale_x_percent", 100),
        fine,
    );
    let (sy_min, sy_max) = axis_scale_extremes(
        master,
        parameter_range(effect, "scale_y_percent", 100),
        fine,
    );
    let (ox_min_bp, ox_max_bp) = offset_basis_extremes(
        parameter_range(effect, "x_percent", 0),
        parameter_range(effect, "x_basis_points", 0),
    );
    let (oy_min_bp, oy_max_bp) = offset_basis_extremes(
        parameter_range(effect, "y_percent", 0),
        parameter_range(effect, "y_basis_points", 0),
    );
    let scales_x = [sx_min as f64 / 100_000_000.0, sx_max as f64 / 100_000_000.0];
    let scales_y = [sy_min as f64 / 100_000_000.0, sy_max as f64 / 100_000_000.0];
    let offsets_x = [
        ox_min_bp as f64 * width / 10_000.0,
        ox_max_bp as f64 * width / 10_000.0,
    ];
    let offsets_y = [
        -(oy_max_bp as f64) * height / 10_000.0,
        -(oy_min_bp as f64) * height / 10_000.0,
    ];
    let (ax_min_bp, ax_max_bp) = parameter_range(effect, "anchor_x_basis_points", 5000);
    let (ay_min_bp, ay_max_bp) = parameter_range(effect, "anchor_y_basis_points", 5000);
    let anchors_x = [
        ax_min_bp as f64 * width / 10_000.0,
        ax_max_bp as f64 * width / 10_000.0,
    ];
    let anchors_y = [
        ay_min_bp as f64 * height / 10_000.0,
        ay_max_bp as f64 * height / 10_000.0,
    ];
    let corners = [
        (f64::from(bounds.left), f64::from(bounds.top)),
        (f64::from(bounds.right), f64::from(bounds.top)),
        (f64::from(bounds.left), f64::from(bounds.bottom)),
        (f64::from(bounds.right), f64::from(bounds.bottom)),
    ];
    // A range spanning a full turn covers every angle: bind the circle.
    if rotation.1.saturating_sub(rotation.0) >= 36_000 {
        let mut radius = 0.0_f64;
        for (x, y) in corners {
            for sx in scales_x {
                for sy in scales_y {
                    for anchor_x in anchors_x {
                        for anchor_y in anchors_y {
                            let dx = (x - anchor_x) * sx;
                            let dy = (y - anchor_y) * sy;
                            radius = radius.max(dx.hypot(dy));
                        }
                    }
                }
            }
        }
        let margin = ROTATED_BOUND_MARGIN_PX;
        return TitlePixelBounds {
            left: clamp_i32((anchors_x[0] - radius - margin).floor()),
            top: clamp_i32((anchors_y[0] - radius - margin).floor()),
            right: clamp_i32((anchors_x[1] + radius + margin).ceil()),
            bottom: clamp_i32((anchors_y[1] + radius + margin).ceil()),
        };
    }
    let mut angles = vec![rotation.0, rotation.1];
    let mut stepped = rotation.0.div_euclid(100) * 100;
    while stepped <= rotation.1 {
        angles.push(stepped);
        stepped += 100;
    }
    let margin = ROTATED_BOUND_MARGIN_PX;
    let mut extremes = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for centidegrees in angles {
        let radians = centidegrees as f64 * std::f64::consts::PI / 18_000.0;
        let (sin, cos) = radians.sin_cos();
        for (x, y) in corners {
            for sx in scales_x {
                for sy in scales_y {
                    for anchor_x in anchors_x {
                        for anchor_y in anchors_y {
                            for offset_x in offsets_x {
                                for offset_y in offsets_y {
                                    let dx = (x - anchor_x) * sx;
                                    let dy = (y - anchor_y) * sy;
                                    let rotated_x = anchor_x + dx * cos - dy * sin + offset_x;
                                    let rotated_y = anchor_y + dx * sin + dy * cos + offset_y;
                                    extremes.0 = extremes.0.min(rotated_x);
                                    extremes.1 = extremes.1.min(rotated_y);
                                    extremes.2 = extremes.2.max(rotated_x);
                                    extremes.3 = extremes.3.max(rotated_y);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    TitlePixelBounds {
        left: clamp_i32((extremes.0 - margin).floor()),
        top: clamp_i32((extremes.1 - margin).floor()),
        right: clamp_i32((extremes.2 + margin).ceil()),
        bottom: clamp_i32((extremes.3 + margin).ceil()),
    }
}

#[allow(clippy::cast_possible_truncation)] // pre-clamped to i32 range; callers pass floor/ceil.
fn clamp_i32(value: f64) -> i32 {
    if !value.is_finite() {
        return if value.is_sign_negative() {
            i32::MIN
        } else {
            i32::MAX
        };
    }
    value.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
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

/// The frames a bounded `color_curves` truncation scan examines.
///
/// The scan evaluates each `color_curves` node at frame zero plus every
/// keyframe frame of that node's curve parameters. That covers the static
/// document, every whole-curve step (CC3 §6 policy 1 keyframes are `Hold`, so
/// the resolved list only changes at a keyframe), and every crossing that is
/// present at a keyframe under point-wise interpolation. A crossing that
/// exists strictly between two keyframes - possible when two coordinates use
/// different keyframe frames or easings - is still handled correctly at render
/// time by the §3.4 truncation rule; QA simply does not sample every frame of
/// the clip. The scan is bounded to this many distinct frames so a document
/// with pathological automation cannot make a QA pass unbounded.
const CURVE_TRUNCATION_SCAN_FRAME_LIMIT: usize = 256;

struct CurveTruncation {
    at: TimeCode,
    curves: Vec<String>,
}

/// The frames a bounded matte band scan examines.
///
/// The scan evaluates each matte-capable node at frame zero plus every
/// keyframe frame of that node's band parameters, which is where an inverted
/// band can appear: the four edges are the only controls whose ordering the
/// rule tests, and `bypass` and `matte_enabled` decide whether an inversion is
/// reportable at all. A crossing that exists strictly between two keyframes -
/// possible when the two edges use different keyframe frames or easings - is
/// still evaluated correctly at render time by the §2.6 rule; QA simply does
/// not sample every frame of the clip. Bounded, like the CC3 truncation scan,
/// so pathological automation cannot make a QA pass unbounded.
const MATTE_BAND_SCAN_FRAME_LIMIT: usize = 256;

struct MatteBandInversion {
    at: TimeCode,
    bands: Vec<String>,
}

/// Every `lut_asset_id` one node references that the document does not own.
///
/// Both the stored static value and every `Hold` keyframe value count, exactly
/// as they do for `RemoveLutAsset`. The unbound sentinel `0` counts too: a
/// valid document never stores it, so seeing it means the node would render
/// nothing while claiming to carry a look. The result is deduplicated and
/// keeps document order so one hand-edited node reports once per distinct id.
fn dangling_lut_asset_references(document: &Document, effect: &Effect) -> Vec<LutAssetId> {
    if !crate::is_lut_color_node(&effect.name) {
        return Vec::new();
    }
    let mut referenced = vec![crate::LutNodeParams::from_effect(effect).lut_asset_id];
    if let Some(curve) = effect.keyframes.get(crate::LUT_ASSET_ID_PARAMETER) {
        for keyframe in &curve.keyframes {
            referenced.push(LutAssetId(
                u64::try_from(keyframe.value).unwrap_or_default(),
            ));
        }
    }
    let mut dangling = Vec::new();
    for lut_asset in referenced {
        if (lut_asset.0 == 0 || document.lut_asset(lut_asset).is_none())
            && !dangling.contains(&lut_asset)
        {
            dangling.push(lut_asset);
        }
    }
    dangling
}

/// Report the first frame at which a `color_curves` node's evaluated curves
/// are truncated by the CC3 §3.4 rule.
///
/// A bypassed node is the exact identity, so its truncation is not reported.
fn curve_truncation(effect: &Effect) -> Option<CurveTruncation> {
    if crate::classify_color_node(effect) != Some(crate::ColorNodeKind::Curves) {
        return None;
    }
    let mut frames = vec![TimeCode::ZERO];
    for (name, curve) in &effect.keyframes {
        if crate::ColorCurveChannel::owning(name).is_none()
            && name != crate::COLOR_NODE_BYPASS_PARAMETER
        {
            continue;
        }
        for keyframe in &curve.keyframes {
            frames.push(keyframe.at);
        }
    }
    frames.sort_unstable();
    frames.dedup();
    frames.truncate(CURVE_TRUNCATION_SCAN_FRAME_LIMIT);
    for at in frames {
        let resolved = crate::ResolvedCurves::from_effect(&effect.evaluated_at(at));
        if resolved.bypass() || !resolved.truncated() {
            continue;
        }
        return Some(CurveTruncation {
            at,
            curves: resolved
                .truncated_curves()
                .into_iter()
                .map(|curve| format!("the {} curve", curve.name()))
                .collect(),
        });
    }
    None
}

/// Report the first frame at which a matte-capable node resolves a qualifier
/// band whose low edge is above its high edge (CC5 §2.6).
///
/// Non-blocking: the band evaluates to `0`, which is a legal resolved state,
/// not an error. A bypassed node is the exact identity and an inactive matte
/// is never evaluated, so neither is reported; a disabled qualifier is not
/// reported either, because its bands do not participate in the coverage.
fn matte_band_inversion(effect: &Effect) -> Option<MatteBandInversion> {
    if !crate::is_matte_capable_color_node(&effect.name) {
        return None;
    }
    let mut frames = vec![TimeCode::ZERO];
    for (name, curve) in &effect.keyframes {
        if !matches!(
            name.as_str(),
            "matte_saturation_low_basis_points"
                | "matte_saturation_high_basis_points"
                | "matte_luma_low_basis_points"
                | "matte_luma_high_basis_points"
                | "matte_qualifier_enabled"
                | "matte_enabled"
                | crate::COLOR_NODE_BYPASS_PARAMETER
        ) {
            continue;
        }
        for keyframe in &curve.keyframes {
            frames.push(keyframe.at);
        }
    }
    frames.sort_unstable();
    frames.dedup();
    frames.truncate(MATTE_BAND_SCAN_FRAME_LIMIT);
    for at in frames {
        let evaluated = effect.evaluated_at(at);
        if crate::color_node_inactive_reason(&evaluated).is_some() {
            continue;
        }
        let matte = crate::MatteParams::from_effect(&evaluated);
        if !matte.has_matte() || !matte.qualifier.is_enabled() {
            continue;
        }
        let bands = matte.degenerate_bands();
        if bands.is_empty() {
            continue;
        }
        return Some(MatteBandInversion {
            at,
            bands: bands.into_iter().map(ToOwned::to_owned).collect(),
        });
    }
    None
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
            assumed_from: None,
        };
        let document = Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![
                    Clip {
                        enabled: true,
                        enabled_curve: None,
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
                        audio_gain_curve: None,
                    },
                    Clip {
                        enabled: true,
                        enabled_curve: None,
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
                        audio_gain_curve: None,
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
            assumed_from: None,
        };
        let media_clip = |id| Clip {
            enabled: true,
            enabled_curve: None,
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
            audio_gain_curve: None,
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
            assumed_from: None,
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
            assumed_from: None,
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
            assumed_from: None,
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
                    enabled: true,
                    enabled_curve: None,
                    id: ClipId(34),
                    asset: crate::AssetId::default(),
                    source_range: TimeCode::ZERO..TimeCode(30),
                    content: ClipContent::Title(crate::Title::default()),
                    timeline_start: TimeCode::ZERO,
                    effects: vec![
                        Effect {
                            enabled: true,
                            enabled_curve: None,
                            id: crate::EffectId(1),
                            name: "look_lut".to_owned(),
                            parameters: std::collections::BTreeMap::new(),
                            keyframes: std::collections::BTreeMap::new(),
                        },
                        Effect {
                            enabled: true,
                            enabled_curve: None,
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
                    audio_gain_curve: None,
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

    fn mo1_transform(
        id: u64,
        enabled: bool,
        params: &[(&str, i64)],
        keyed: &[(&str, &[(i64, i64)])],
        enabled_curve: Option<crate::AutomationCurve>,
    ) -> Effect {
        Effect {
            enabled,
            enabled_curve,
            id: crate::EffectId(id),
            name: "transform".to_owned(),
            parameters: params
                .iter()
                .map(|(name, value)| ((*name).to_owned(), ParamValue::Integer(*value)))
                .collect(),
            keyframes: keyed
                .iter()
                .map(|(name, keys)| {
                    (
                        (*name).to_owned(),
                        crate::AutomationCurve {
                            keyframes: keys
                                .iter()
                                .map(|(at, value)| crate::Keyframe {
                                    at: TimeCode(*at),
                                    value: *value,
                                    interpolation: crate::KeyframeInterpolation::Linear,
                                    tangent_in: 0,
                                    tangent_out: 0,
                                })
                                .collect(),
                        },
                    )
                })
                .collect(),
        }
    }

    fn mo1_hold_curve(keys: &[(i64, i64)]) -> crate::AutomationCurve {
        crate::AutomationCurve {
            keyframes: keys
                .iter()
                .map(|(at, value)| crate::Keyframe {
                    at: TimeCode(*at),
                    value: *value,
                    interpolation: crate::KeyframeInterpolation::Hold,
                    tangent_in: 0,
                    tangent_out: 0,
                })
                .collect(),
        }
    }

    /// MO1 G6: the title bound folds the full transform — per-axis scale,
    /// fine scale, and basis-point offsets — about the anchor. Box
    /// 100,40..140,60 in 320x180: x halves about 160 then shifts +32,
    /// y doubles about 90 then shifts +9.
    #[test]
    fn transformed_title_bounds_fold_per_axis_fine_and_basis_lanes() {
        let bounds = TitlePixelBounds {
            left: 100,
            top: 40,
            right: 140,
            bottom: 60,
        };
        let effects = [mo1_transform(
            1,
            true,
            &[
                ("scale_percent", 100),
                ("scale_x_percent", 50),
                ("scale_y_percent", 200),
                ("scale_fine_hundredths", 10_000),
                ("x_basis_points", 1000),
                ("y_basis_points", -500),
            ],
            &[],
            None,
        )];
        let moved = transformed_title_bounds(bounds, &effects, (320, 180));
        assert_eq!(
            (moved.left, moved.top, moved.right, moved.bottom),
            (162, -1, 182, 39)
        );
    }

    /// MO1 G6: rotation expands the bound about the anchor — 90 degrees
    /// clockwise about the frame centre swaps the box footprint. The
    /// f64 path keeps a 0.05 px conservatism margin, hence the
    /// one-outward expectations.
    #[test]
    fn transformed_title_bounds_rotate_about_the_anchor() {
        let bounds = TitlePixelBounds {
            left: 100,
            top: 40,
            right: 140,
            bottom: 60,
        };
        let effects = [mo1_transform(
            1,
            true,
            &[("rotation_centidegrees", 9000)],
            &[],
            None,
        )];
        let moved = transformed_title_bounds(bounds, &effects, (320, 180));
        assert_eq!(
            (moved.left, moved.top, moved.right, moved.bottom),
            (189, 29, 211, 71)
        );

        // 180 degrees about the top-left corner negates both axes.
        let effects = [mo1_transform(
            1,
            true,
            &[
                ("rotation_centidegrees", 18_000),
                ("anchor_x_basis_points", 0),
                ("anchor_y_basis_points", 0),
            ],
            &[],
            None,
        )];
        let moved = transformed_title_bounds(bounds, &effects, (320, 180));
        assert_eq!(
            (moved.left, moved.top, moved.right, moved.bottom),
            (-141, -61, -99, -39)
        );
    }

    /// MO1 G6: a 45-degree static rotation about the box centre grows
    /// each half-extent by (w + h) / sqrt(2) / 2 — pinned via the
    /// closed form, not the implementation's sampler.
    #[test]
    #[allow(clippy::cast_possible_truncation)] // closed-form pin: values are small by construction.
    fn transformed_title_bounds_match_the_45_degree_closed_form() {
        let bounds = TitlePixelBounds {
            left: 100,
            top: 40,
            right: 140,
            bottom: 60,
        };
        let effects = [mo1_transform(
            1,
            true,
            &[
                ("rotation_centidegrees", 4500),
                ("anchor_x_basis_points", 3750),
                ("anchor_y_basis_points", 2500),
            ],
            &[],
            None,
        )];
        let moved = transformed_title_bounds(bounds, &effects, (320, 200));
        let half = 30.0 / 2.0_f64.sqrt();
        let expected = (
            (120.0 - half - 0.05).floor() as i32,
            (50.0 - half - 0.05).floor() as i32,
            (120.0 + half + 0.05).ceil() as i32,
            (50.0 + half + 0.05).ceil() as i32,
        );
        assert_eq!(expected, (98, 28, 142, 72));
        assert_eq!((moved.left, moved.top, moved.right, moved.bottom), expected);
    }

    /// MO1 G6: keyframed scale ranges take their extremes per corner.
    /// Corners left of / above the anchor have negative deltas, so their
    /// maxima sit at the SMALLEST scale (50): right = 160 - 20/2 = 150,
    /// bottom = 90 - 30/2 = 75. The old maximum-only fold pinned (120, 30)
    /// here and so under-approximated the range — red against the rewrite.
    #[test]
    fn transformed_title_bounds_take_keyframe_extremes() {
        let bounds = TitlePixelBounds {
            left: 100,
            top: 40,
            right: 140,
            bottom: 60,
        };
        let effects = [mo1_transform(
            1,
            true,
            &[("scale_percent", 100)],
            &[("scale_percent", &[(0, 50), (10, 200)])],
            None,
        )];
        let moved = transformed_title_bounds(bounds, &effects, (320, 180));
        assert_eq!(
            (moved.left, moved.top, moved.right, moved.bottom),
            (40, -10, 150, 75)
        );
    }

    /// MO1 G6: disabled transforms are ignored — statically, or via an
    /// all-zero enable curve — while an enable curve that reaches 1
    /// anywhere still applies the transform.
    #[test]
    fn transformed_title_bounds_ignore_disabled_transforms() {
        let bounds = TitlePixelBounds {
            left: 100,
            top: 40,
            right: 140,
            bottom: 60,
        };
        let params: &[(&str, i64)] = &[
            ("scale_percent", 100),
            ("scale_x_percent", 50),
            ("scale_y_percent", 200),
            ("x_basis_points", 1000),
            ("y_basis_points", -500),
        ];
        let unchanged = (bounds.left, bounds.top, bounds.right, bounds.bottom);
        let moved_case = (162, -1, 182, 39);

        let off = [mo1_transform(1, false, params, &[], None)];
        let moved = transformed_title_bounds(bounds, &off, (320, 180));
        assert_eq!(
            (moved.left, moved.top, moved.right, moved.bottom),
            unchanged,
            "a statically disabled transform must not move the bound"
        );

        // Master lanes the old fold honors: the skip rule itself must win.
        let master = [mo1_transform(
            1,
            false,
            &[("scale_percent", 200), ("x_percent", 50)],
            &[],
            None,
        )];
        let moved = transformed_title_bounds(bounds, &master, (320, 180));
        assert_eq!(
            (moved.left, moved.top, moved.right, moved.bottom),
            unchanged,
            "a disabled 200% + half-frame shift must not move the bound"
        );

        let zero_curve = [mo1_transform(
            1,
            true,
            params,
            &[],
            Some(mo1_hold_curve(&[(0, 0)])),
        )];
        let moved = transformed_title_bounds(bounds, &zero_curve, (320, 180));
        assert_eq!(
            (moved.left, moved.top, moved.right, moved.bottom),
            unchanged,
            "an all-zero enable curve must not move the bound"
        );

        let cut_in = [mo1_transform(
            1,
            false,
            params,
            &[],
            Some(mo1_hold_curve(&[(0, 0), (5, 1)])),
        )];
        let moved = transformed_title_bounds(bounds, &cut_in, (320, 180));
        assert_eq!(
            (moved.left, moved.top, moved.right, moved.bottom),
            moved_case,
            "an enable curve reaching 1 must still apply the transform"
        );
    }

    /// MO1 G6: the legacy-stage warning follows the compositor — a
    /// statically disabled legacy effect, or one whose enable curve
    /// never reaches 1, warns nothing; a curve reaching 1 still warns.
    #[test]
    fn legacy_stage_warning_skips_disabled_effects() {
        let document = |effects: Vec<Effect>| Document {
            tracks: vec![crate::Track {
                id: crate::TrackId(1),
                kind: crate::TrackKind::Video,
                sync_lock: true,
                clips: vec![crate::Clip {
                    enabled: true,
                    enabled_curve: None,
                    id: crate::ClipId(1),
                    asset: crate::AssetId::default(),
                    source_range: TimeCode::ZERO..TimeCode(30),
                    content: crate::ClipContent::Media,
                    timeline_start: TimeCode::ZERO,
                    effects,
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: TimeCode::ZERO,
                    audio_fade_out_frames: TimeCode::ZERO,
                    speed_percent: 100,
                    audio_gain_curve: None,
                }],
            }],
            duration: TimeCode(30),
            ..Document::default()
        };
        let lut = |enabled: bool, curve: Option<crate::AutomationCurve>| Effect {
            enabled,
            enabled_curve: curve,
            id: crate::EffectId(1),
            name: "look_lut".to_owned(),
            parameters: std::collections::BTreeMap::new(),
            keyframes: std::collections::BTreeMap::new(),
        };
        let legacy_warnings = |document: &Document| {
            qa_document(document)
                .issues
                .iter()
                .filter(|issue| issue.code == "legacy_lut_stage")
                .count()
        };

        assert_eq!(
            legacy_warnings(&document(vec![lut(false, None)])),
            0,
            "a disabled legacy effect must not warn"
        );
        assert_eq!(
            legacy_warnings(&document(vec![lut(true, Some(mo1_hold_curve(&[(0, 0)])))])),
            0,
            "an all-zero enable curve must not warn"
        );
        assert_eq!(
            legacy_warnings(&document(vec![lut(
                false,
                Some(mo1_hold_curve(&[(0, 0), (5, 1)]))
            )])),
            1,
            "an enable curve reaching 1 must still warn"
        );
    }
}
