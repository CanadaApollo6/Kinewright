//! Agent-facing CC1 colour observability and evidence-only planning.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use kinewright_core::{
    AssetId, COLOR_CURVE_COORDINATE_MAX, COLOR_CURVE_COORDINATE_MIN, COLOR_CURVE_MAX_POINTS,
    COLOR_CURVE_MIN_POINTS, COLOR_CURVE_WHITE_BASIS_POINTS, COLOR_NODE_BYPASS_PARAMETER,
    COLOR_NODE_LIMIT_PER_LAYER, Clip, ClipContent, ClipId, ColorCurveChannel, ColorDescription,
    ColorNodeKind, ColorSourceError, ColorSourceProfile, ColorSourceProfileAssumption,
    ColorWheelChannel, ColorWheelControl, ColorWheelsParams, ColorWhitePoint, CurvePoints,
    Document, Effect, EffectCompatibilityStage, EffectId, MANAGED_COLOR_NODE_NAMES,
    MediaAvailabilityStatus, MediaError, MediaKind, Operation, ParamValue, ResolvedCurves,
    TimeCode, TimelineRevision, TrackKind, apply_batch, classify_color_node,
    classify_source_with_assumption, effect_compatibility_stage, effect_descriptor,
    managed_color_node_count,
};
use serde_json::{Value, json};
use thiserror::Error;

const PRIMARY_CORRECTION_EFFECT_NAME: &str = "primary_correction";

/// Every `ColorSourceProfileAssumption` variant a caller may request. The
/// enum is deliberately bounded; adding a variant must add an entry here.
const AVAILABLE_PROFILE_ASSUMPTIONS: [&str; 1] = ["d65"];

/// Serialise an applied assumption through its own serde representation so the
/// status surface cannot drift from the enum.
fn assumption_value(assumption: ColorSourceProfileAssumption) -> Value {
    serde_json::to_value(assumption).unwrap_or(Value::Null)
}

/// Reject `raw_only` combined with an explicit `profile_assumption`.
///
/// `raw_only` means "classify with no assumption at all". Silently discarding
/// the caller's explicit assumption would return evidence that answers a
/// different question from the one that was asked.
#[must_use]
pub(crate) fn raw_only_conflict(args: &ColorContextArgs) -> Option<Value> {
    if !args.raw_only || args.profile_assumption.is_none() {
        return None;
    }
    Some(json!({
        "code": "raw_only_conflicts_with_profile_assumption",
        "message": "raw_only=true classifies the source with no assumption, so an explicit profile_assumption cannot also be honoured; send exactly one of them.",
        "details": {
            "raw_only": true,
            "profile_assumption": args.profile_assumption.map_or(Value::Null, assumption_value),
            "asset_ids": args.asset_ids.iter().map(|asset| asset.0).collect::<Vec<_>>(),
            "recovery": "Send raw_only=true with no profile_assumption for unassumed classifier evidence, or omit raw_only to apply the explicit assumption.",
        },
        "evidence_only": true,
        "applied": false,
    }))
}

/// Canonical CC1 stage names exposed to human and agent evidence surfaces.
pub(crate) const CC1_STAGE_NAMES: [&str; 8] = [
    "source_range_expansion",
    "source_matrix_decode",
    "source_transfer_decode",
    "primaries_white_point_to_working_bt709",
    "primary_correction_nodes",
    "linear_light_layer_compositing",
    "monitoring_or_delivery_transfer",
    "final_clamp_quantization",
];

/// Arguments for the evidence-only primary-correction planner.
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct PrimaryCorrectionPlanArgs {
    /// Exact timeline revision returned by the preceding inspection.
    pub expected_revision: TimelineRevision,
    /// Stable visual media clip id to receive one new primary node.
    pub clip_id: ClipId,
    /// Optional explicit D65 assumption for a complete BT.709 source whose
    /// raw white point is unknown.
    #[serde(default)]
    pub profile_assumption: Option<ColorSourceProfileAssumption>,
    /// Integer CC1 parameters. Omitted controls resolve to descriptor neutrals.
    pub parameters: BTreeMap<String, i64>,
}

/// Arguments for an isolated, read-only before/after CC1 proof.
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct RenderColorProofArgs {
    /// Exact timeline revision returned by the preceding inspection.
    pub expected_revision: TimelineRevision,
    /// Stable visual media clip id receiving the proposed isolated node.
    pub clip_id: ClipId,
    /// Exact project frame to render in both before and after documents.
    pub timecode: TimeCode,
    /// Optional explicit D65 assumption. Omission uses the executed normative
    /// application profile assumption for an otherwise-complete BT.709 source.
    #[serde(default)]
    pub profile_assumption: Option<ColorSourceProfileAssumption>,
    /// Integer CC1 parameters. Omitted controls resolve to descriptor neutrals.
    pub parameters: BTreeMap<String, i64>,
}

impl From<&RenderColorProofArgs> for PrimaryCorrectionPlanArgs {
    fn from(args: &RenderColorProofArgs) -> Self {
        Self {
            expected_revision: args.expected_revision,
            clip_id: args.clip_id,
            profile_assumption: args.profile_assumption,
            parameters: args.parameters.clone(),
        }
    }
}

/// Optional explicit source-profile assumptions used only for inspection.
#[derive(Debug, Clone, Default, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct ColorContextArgs {
    /// Apply this explicit assumption to the selected source assets. Omit to
    /// use the executed application profile assumption for complete BT.709
    /// sources whose white point is unknown.
    #[serde(default)]
    pub profile_assumption: Option<ColorSourceProfileAssumption>,
    /// Inspect the raw classifier result without the application's normative
    /// D65 assumption. The default status reflects the transform the managed
    /// renderer actually executes.
    #[serde(default)]
    pub raw_only: bool,
    /// Asset ids to which the assumption applies. An empty list applies it to
    /// every otherwise complete BT.709 source with unknown white point.
    #[serde(default)]
    pub asset_ids: Vec<AssetId>,
}

/// The validated, unapplied proposal returned by the primary planner.
#[derive(Debug, Clone)]
pub(crate) struct PrimaryCorrectionPlan {
    pub expected_revision: TimelineRevision,
    pub clip_id: ClipId,
    /// The node the proposal targets: either the clip's existing last
    /// `primary_correction` node or the freshly allocated id for a new one.
    pub effect_id: EffectId,
    /// Whether the plan allocates a new node. A clip that already carries a
    /// managed primary is corrected in place rather than stacked.
    pub created_new_node: bool,
    pub source_profile: ColorSourceProfile,
    pub profile_assumption: Option<ColorSourceProfileAssumption>,
    pub requested_parameters: BTreeMap<String, i64>,
    pub resolved_parameters: BTreeMap<String, i64>,
    pub operations: Vec<Operation>,
    pub existing_primary_node_count: usize,
    /// Non-fatal advisories, such as an ambiguous multi-primary clip.
    pub warnings: Vec<String>,
    /// True when every requested control already holds the requested value, in
    /// which case `operations` is empty rather than a neutral no-op node.
    pub no_change: bool,
}

impl PrimaryCorrectionPlan {
    /// The node this proposal actually publishes.
    ///
    /// A no-op proposal that would have created a node never allocates one, so
    /// reporting `effect_id` there would publish a phantom id that no operation
    /// creates and that a later plan is free to reuse.
    #[must_use]
    pub fn target_effect_id(&self) -> Option<EffectId> {
        if self.no_change && !self.created_new_node && self.existing_primary_node_count == 0 {
            return None;
        }
        Some(self.effect_id)
    }
}

/// The managed primary-correction node a new proposal for one clip would
/// target, with the static parameter values a delta composes against.
#[derive(Debug, Clone)]
pub(crate) struct ExistingPrimaryNode {
    pub effect_id: EffectId,
    /// Static values of every descriptor parameter, defaulting to the
    /// descriptor neutral when the node does not carry the control.
    pub parameters: BTreeMap<String, i64>,
    /// Descriptor parameters that carry keyframes on this node. Composing a
    /// delta against an animated control is ambiguous, so callers report it.
    pub keyframed: Vec<String>,
    /// How many `primary_correction` nodes the clip carries. More than one is
    /// ambiguous; `effect_id` is the last in compositor evaluation order.
    pub node_count: usize,
}

/// Resolve the existing managed primary node for `clip_id`, if any.
///
/// Returns `None` when the clip carries no `primary_correction` node, in which
/// case a proposal starts from the descriptor neutral.
#[must_use]
pub(crate) fn existing_primary_node(
    document: &Document,
    clip_id: ClipId,
) -> Option<ExistingPrimaryNode> {
    let descriptor = effect_descriptor(PRIMARY_CORRECTION_EFFECT_NAME)?;
    let clip = document
        .tracks
        .iter()
        .flat_map(|track| track.clips.iter())
        .find(|clip| clip.id == clip_id)?;
    let primaries = clip
        .effects
        .iter()
        .filter(|effect| effect.name == PRIMARY_CORRECTION_EFFECT_NAME)
        .collect::<Vec<_>>();
    // The compositor evaluates the chain in order, so the last node is the one
    // a correction has to move.
    let effect = primaries.last()?;
    let parameters = descriptor
        .parameters
        .iter()
        .map(|parameter| {
            let value = match effect.parameters.get(parameter.name) {
                Some(ParamValue::Integer(value)) => *value,
                _ => parameter.neutral,
            };
            (parameter.name.to_owned(), value)
        })
        .collect();
    let keyframed = descriptor
        .parameters
        .iter()
        .filter(|parameter| {
            effect
                .keyframes
                .get(parameter.name)
                .is_some_and(|curve| !curve.keyframes.is_empty())
        })
        .map(|parameter| parameter.name.to_owned())
        .collect();
    Some(ExistingPrimaryNode {
        effect_id: effect.id,
        parameters,
        keyframed,
        node_count: primaries.len(),
    })
}

#[derive(Debug, Error)]
pub(crate) enum PrimaryPlanError {
    #[error("timeline revision conflict: expected {expected}, actual {actual}")]
    RevisionConflict {
        expected: TimelineRevision,
        actual: TimelineRevision,
    },
    #[error("clip {0} does not exist")]
    MissingClip(ClipId),
    #[error("clip {clip} is not a visual media clip (track={track:?}, content={content})")]
    WrongClipType {
        clip: ClipId,
        track: TrackKind,
        content: &'static str,
    },
    #[error("clip {clip} references missing asset {asset}")]
    MissingAsset { clip: ClipId, asset: AssetId },
    #[error("asset {asset} on clip {clip} is not video-capable ({kind:?})")]
    WrongAssetKind {
        clip: ClipId,
        asset: AssetId,
        kind: MediaKind,
    },
    #[error("clip {clip} source is not managed CC1-compatible: {error}")]
    UnsupportedSource {
        clip: ClipId,
        error: ColorSourceError,
    },
    #[error("unknown primary-correction parameter {name}")]
    UnknownParameter { name: String },
    #[error(
        "primary-correction parameter {name}={value} is outside the inclusive range {min}..={max}"
    )]
    ParameterOutOfRange {
        name: String,
        value: i64,
        min: i64,
        max: i64,
    },
    #[error("the Core primary_correction descriptor is unavailable")]
    MissingDescriptor,
    #[error("could not allocate a fresh primary effect id")]
    EffectIdExhausted,
    #[error("Core rejected the evidence-only primary plan: {0}")]
    CoreRejected(String),
}

#[derive(Debug, Error)]
pub(crate) enum ColorProofError {
    #[error("the project colour pipeline is not CC1 managed-SDR compatible: {reason}")]
    PipelineIncompatible { reason: String },
    #[error("project frame {frame} is outside project range 0..{duration}")]
    ProjectFrameOutOfRange { frame: TimeCode, duration: TimeCode },
    #[error("project frame {frame} is outside clip {clip} visibility {start}..{end}")]
    ClipFrameOutOfRange {
        clip: ClipId,
        frame: TimeCode,
        start: TimeCode,
        end: TimeCode,
    },
    #[error("clip {clip} timing could not be resolved: {reason}")]
    ClipTimingInvalid { clip: ClipId, reason: String },
    #[error("asset {asset} for clip {clip} is not currently renderable: {status:?}")]
    MediaUnavailable {
        clip: ClipId,
        asset: AssetId,
        status: MediaAvailabilityStatus,
    },
    #[error("managed compositor failed while rendering {stage}: {message}")]
    RenderFailed {
        stage: &'static str,
        message: String,
    },
    #[error(
        "managed compositor cannot decode a supported source format while rendering {stage}: {reason}"
    )]
    UnsupportedDecoderFormat {
        stage: &'static str,
        path: PathBuf,
        format: String,
        declared_bit_depth: Option<u8>,
        decoder_bit_depth: Option<u8>,
        reason: String,
    },
    #[error("managed compositor returned an invalid {stage} image: {message}")]
    InvalidImage {
        stage: &'static str,
        message: String,
    },
    #[error(
        "active rendered layer clip {clip} source asset {asset} is not managed CC1-compatible: {error}"
    )]
    UnsupportedActiveLayerSource {
        clip: ClipId,
        asset: AssetId,
        error: ColorSourceError,
        /// Non-blocking post-primary compatibility warnings collected across
        /// every active layer of the refused composite. They are only reachable
        /// here: a blocking source refuses the proof, so the success payload's
        /// `unsupported_layer_warnings` is never produced for this composite.
        layer_warnings: Vec<Value>,
    },
    #[error(transparent)]
    Primary(#[from] PrimaryPlanError),
}

impl ColorProofError {
    #[must_use]
    pub(crate) const fn code(&self) -> &'static str {
        match self {
            Self::PipelineIncompatible { .. } => "unsupported_color_pipeline",
            Self::ProjectFrameOutOfRange { .. } => "project_frame_out_of_range",
            Self::ClipFrameOutOfRange { .. } => "clip_frame_out_of_range",
            Self::ClipTimingInvalid { .. } => "clip_timing_invalid",
            Self::MediaUnavailable { status, .. } => match status.kind {
                kinewright_core::MediaAvailabilityKind::OfflineMissing => "media_offline",
                kinewright_core::MediaAvailabilityKind::Changed => "media_changed",
                kinewright_core::MediaAvailabilityKind::Unreadable => "media_unreadable",
                kinewright_core::MediaAvailabilityKind::OnlineVerified
                | kinewright_core::MediaAvailabilityKind::OnlineUnverified => "media_unavailable",
            },
            Self::RenderFailed { .. } => "color_proof_render_failed",
            Self::UnsupportedDecoderFormat { .. } => "unsupported_decoder_format",
            Self::InvalidImage { .. } => "color_proof_invalid_image",
            Self::UnsupportedActiveLayerSource { .. } => "active_layer_needs_color_override",
            Self::Primary(error) => error.code(),
        }
    }

    #[must_use]
    pub(crate) fn details(&self) -> Value {
        match self {
            Self::PipelineIncompatible { reason } => json!({"reason": reason}),
            Self::ProjectFrameOutOfRange { frame, duration } => {
                json!({"frame": frame, "duration": duration})
            }
            Self::ClipFrameOutOfRange {
                clip,
                frame,
                start,
                end,
            } => json!({
                "clip_id": clip,
                "frame": frame,
                "visible_start": start,
                "visible_end": end,
            }),
            Self::ClipTimingInvalid { clip, reason } => {
                json!({"clip_id": clip, "reason": reason})
            }
            Self::MediaUnavailable {
                clip,
                asset,
                status,
            } => json!({"clip_id": clip, "asset_id": asset, "availability": status}),
            Self::RenderFailed { stage, message } | Self::InvalidImage { stage, message } => {
                json!({"stage": stage, "message": message})
            }
            Self::UnsupportedDecoderFormat {
                stage,
                path,
                format,
                declared_bit_depth,
                decoder_bit_depth,
                reason,
            } => json!({
                "stage": stage,
                "path": path,
                "format": format,
                "declared_bit_depth": declared_bit_depth,
                "decoder_bit_depth": decoder_bit_depth,
                "reason": reason,
            }),
            Self::UnsupportedActiveLayerSource {
                clip,
                asset,
                error,
                layer_warnings,
            } => json!({
                "clip_id": clip,
                "asset_id": asset,
                "code": error.code(),
                "field": error.field(),
                "observed": error.observed(),
                "allowed": error.allowed_values(),
                "recovery": error.recovery_action(),
                "message": error.actionable_message(),
                "unsupported_layer_warnings": layer_warnings,
            }),
            Self::Primary(error) => error.details(),
        }
    }

    /// Preserve structured decoder recovery information at the proof
    /// boundary; generic backend failures remain ordinary render failures.
    pub(crate) fn from_media_error(stage: &'static str, error: MediaError) -> Self {
        match error {
            MediaError::UnsupportedDecoderFormat {
                path,
                format,
                declared_bit_depth,
                decoder_bit_depth,
                reason,
            } => Self::UnsupportedDecoderFormat {
                stage,
                path,
                format,
                declared_bit_depth,
                decoder_bit_depth,
                reason,
            },
            error => Self::RenderFailed {
                stage,
                message: error.to_string(),
            },
        }
    }
}

impl PrimaryPlanError {
    /// Stable machine-readable recovery code for structured agent responses.
    #[must_use]
    pub(crate) const fn code(&self) -> &'static str {
        match self {
            Self::RevisionConflict { .. } => "revision_conflict",
            Self::MissingClip(_) => "clip_not_found",
            Self::WrongClipType { .. } => "unsupported_clip_type",
            Self::MissingAsset { .. } => "asset_not_found",
            Self::WrongAssetKind { .. } => "unsupported_asset_kind",
            Self::UnsupportedSource { .. } => "needs_color_override",
            Self::UnknownParameter { .. } => "unknown_primary_parameter",
            Self::ParameterOutOfRange { .. } => "primary_parameter_out_of_range",
            Self::MissingDescriptor => "missing_primary_descriptor",
            Self::EffectIdExhausted => "effect_id_exhausted",
            Self::CoreRejected(_) => "primary_plan_core_rejected",
        }
    }

    /// Structured recovery evidence for the agent response.
    #[must_use]
    pub(crate) fn details(&self) -> Value {
        match self {
            Self::RevisionConflict { expected, actual } => {
                json!({"expected_revision": expected, "actual_revision": actual})
            }
            Self::MissingClip(clip) => json!({"clip_id": clip}),
            Self::WrongClipType {
                clip,
                track,
                content,
            } => json!({"clip_id": clip, "track_kind": track, "content": content}),
            Self::MissingAsset { clip, asset } => json!({"clip_id": clip, "asset_id": asset}),
            Self::WrongAssetKind { clip, asset, kind } => {
                json!({"clip_id": clip, "asset_id": asset, "media_kind": kind})
            }
            Self::UnsupportedSource { clip, error } => json!({
                "clip_id": clip,
                "code": error.code(),
                "field": error.field(),
                "observed": error.observed(),
                "allowed": error.allowed_values(),
                "recovery": error.recovery_action(),
            }),
            Self::UnknownParameter { name } => json!({
                "parameter": name,
                "allowed_parameters": primary_parameter_documentation(),
            }),
            Self::ParameterOutOfRange {
                name,
                value,
                min,
                max,
            } => json!({"parameter": name, "value": value, "min": min, "max": max}),
            Self::MissingDescriptor | Self::EffectIdExhausted | Self::CoreRejected(_) => {
                Value::Null
            }
        }
    }
}

/// The exact CC1 primary controls, derived from the Core descriptor so the
/// tool description and the `unknown_primary_parameter` recovery evidence can
/// never drift from the values Core actually validates.
#[must_use]
pub(crate) fn primary_parameter_documentation() -> Vec<Value> {
    effect_descriptor(PRIMARY_CORRECTION_EFFECT_NAME).map_or_else(Vec::new, |descriptor| {
        descriptor
            .parameters
            .iter()
            .map(|parameter| {
                json!({
                    "name": parameter.name,
                    "min": parameter.min,
                    "max": parameter.max,
                    "neutral": parameter.neutral,
                })
            })
            .collect()
    })
}

/// One-line `name=min..=max, neutral N` summary of every CC1 primary control.
#[must_use]
pub(crate) fn primary_parameter_summary() -> String {
    effect_descriptor(PRIMARY_CORRECTION_EFFECT_NAME).map_or_else(String::new, |descriptor| {
        descriptor
            .parameters
            .iter()
            .map(|parameter| {
                format!(
                    "{}={}..={}, neutral {}",
                    parameter.name, parameter.min, parameter.max, parameter.neutral
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    })
}

/// Render the complete CC1 metadata/status surface without probing, mutating,
/// or silently changing any source description.
#[must_use]
pub(crate) fn color_context_value(revision: TimelineRevision, document: &Document) -> Value {
    color_context_value_with_options(revision, document, None, &[], false)
}

/// Render colour status with an explicit, non-mutating profile assumption.
#[must_use]
pub(crate) fn color_context_value_with_assumptions(
    revision: TimelineRevision,
    document: &Document,
    profile_assumption: Option<ColorSourceProfileAssumption>,
    assumption_asset_ids: &[AssetId],
) -> Value {
    if profile_assumption.is_none() && assumption_asset_ids.is_empty() {
        color_context_value(revision, document)
    } else {
        color_context_value_with_options(
            revision,
            document,
            profile_assumption,
            assumption_asset_ids,
            false,
        )
    }
}

/// Render colour status with optional explicit assumptions and a raw-only mode.
#[allow(clippy::too_many_lines)]
pub(crate) fn color_context_value_with_options(
    revision: TimelineRevision,
    document: &Document,
    profile_assumption: Option<ColorSourceProfileAssumption>,
    assumption_asset_ids: &[AssetId],
    raw_only: bool,
) -> Value {
    let referenced_visual_assets = referenced_visual_assets(document);
    let assets = document
        .media_pool
        .iter()
        .map(|asset| {
            let referenced = referenced_visual_assets.contains(&asset.id);
            let explicit_assumption_applies = profile_assumption.is_some()
                && (assumption_asset_ids.is_empty() || assumption_asset_ids.contains(&asset.id));
            let (assumption, assumption_source) = if raw_only {
                (None, None)
            } else if explicit_assumption_applies {
                (profile_assumption, Some("explicit"))
            } else if normative_d65_assumption(&asset.color_description).is_some() {
                (
                    Some(ColorSourceProfileAssumption::D65),
                    Some("application_profile_assumption"),
                )
            } else {
                (None, None)
            };
            let source_status =
                source_status(&asset.color_description, assumption, assumption_source);
            let blocking = source_status["status"] == "needs_color_override" && referenced;
            json!({
                "id": asset.id.0,
                "name": asset.name,
                "referenced_visual": referenced,
                "managed_blocking": blocking,
                "source": {
                    "raw_description": asset.color_description,
                    "status": source_status,
                    "formats": {
                        "input": {
                            "bit_depth": asset.color_description.bit_depth,
                            "range": asset.color_description.range,
                            // §5 requires the source raster alongside the
                            // input format. `null` means the probe did not
                            // report a resolution; it is never invented.
                            "raster": asset.resolution,
                        },
                    },
                },
            })
        })
        .collect::<Vec<_>>();

    // CC1 §5 observability is a *visual* colour surface. Audio-track clips can
    // never occupy a colour stage, so they are excluded rather than listed
    // without an ordering. Video tracks composite in ascending document order,
    // which is exactly the order `kinewright_media::visual_layers_at` uses to
    // build the `render_color_proof` manifest.
    let clips = document
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Video)
        .enumerate()
        .flat_map(|(z_order, track)| {
            track
                .clips
                .iter()
                .map(move |clip| clip_status(track.id.0, z_order, clip))
        })
        .collect::<Vec<_>>();
    let legacy_stage_warnings = legacy_stage_warnings_for_document(document);

    let working = &document.color_context.working;
    let monitoring = &document.color_context.monitoring;
    let delivery = &document.color_context.delivery;
    let value = json!({
        "timeline_revision": revision.0,
        "color_context": {
            "pipeline_state": document.color_context.pipeline_state,
            "working": working,
            "monitoring": monitoring,
            "delivery": delivery,
            "formats": {
                "working": {
                    "bit_depth": working.bit_depth,
                    "range": working.range,
                },
                "monitoring": {
                    "bit_depth": monitoring.bit_depth,
                    "range": monitoring.range,
                },
                "delivery": {
                    "bit_depth": delivery.bit_depth,
                    "range": delivery.range,
                },
            },
        },
        "ordered_stage_names": CC1_STAGE_NAMES,
        "proof": {
            "status": "on_demand",
            "available": true,
            "capability": "render_color_proof",
            "render_kind": "managed_compositor",
            "reason": "Call render_color_proof for an isolated mapped BEFORE/AFTER frame; this status response itself is metadata-only.",
        },
        "profile_assumption_policy": {
            "white_point": "d65",
            "default_source": "application_profile_assumption",
            "source_metadata_preserved": true,
            "default_application_assumption": "complete_bt709_unknown_white_point",
            "raw_only_requires_explicit_assumption": true,
            "raw_only_available": true,
        },
        // `get_color_context` reads persisted metadata only. It never renders
        // or samples a frame, so there is no sampled region to report and the
        // marker is explicit rather than absent.
        "sampling_region": Value::Null,
        "sampling_region_note": "get_color_context is metadata-only; call render_color_proof for a rendered frame, or get_video_scopes_v2, whose top-level `roi` key reports the region actually sampled.",
        "layer_scope": "video_tracks_only",
        "z_order_convention": "ascending video-track document index; a higher z_order composites above a lower one",
        "assets": assets,
        "clips": clips,
        "legacy_stage_warnings": legacy_stage_warnings,
        "managed_blocking_asset_ids": assets
            .iter()
            .filter(|asset| asset["managed_blocking"] == true)
            .filter_map(|asset| asset["id"].as_u64())
            .collect::<Vec<_>>(),
    });
    value
}

fn referenced_visual_assets(document: &Document) -> BTreeSet<AssetId> {
    document
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Video)
        .flat_map(|track| track.clips.iter())
        .filter(|clip| matches!(clip.content, ClipContent::Media | ClipContent::Freeze(_)))
        .filter_map(|clip| document.asset(clip.asset))
        .filter(|asset| matches!(asset.kind, MediaKind::Video | MediaKind::AudioVideo))
        .map(|asset| asset.id)
        .collect()
}

/// Return every post-primary compatibility warning for a document in stable
/// clip/effect order so status and proof manifests agree.
///
/// The colour layer scope is video tracks only, matching the `clips` list this
/// sits beside: an effect on an audio clip is never part of the CC1 managed
/// image chain and must not raise a colour compatibility warning.
#[must_use]
pub(crate) fn legacy_stage_warnings_for_document(document: &Document) -> Vec<Value> {
    document
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Video)
        .flat_map(|track| track.clips.iter().flat_map(legacy_stage_warnings))
        .collect()
}

/// Return ordered post-primary compatibility warnings for one clip.
#[must_use]
pub(crate) fn legacy_stage_warnings(clip: &Clip) -> Vec<Value> {
    clip.effects
        .iter()
        .enumerate()
        .filter_map(|(effect_index, effect)| legacy_warning(effect_index, effect))
        .collect()
}

fn source_status(
    description: &ColorDescription,
    profile_assumption: Option<ColorSourceProfileAssumption>,
    assumption_source: Option<&str>,
) -> Value {
    match classify_source_with_assumption(description, profile_assumption) {
        Ok(profile) => json!({
            "status": "supported",
            "supported_profile": profile.id(),
            "profile_assumption": {
                // Serialise the assumption that was actually applied rather
                // than a hardcoded name, so a future assumption variant cannot
                // be silently reported as d65.  An assumption only changes the
                // classification when the raw white point is unknown.
                "selected": if description.white_point == ColorWhitePoint::Unknown {
                    profile_assumption.map_or(Value::Null, assumption_value)
                } else {
                    Value::Null
                },
                "source": assumption_source.unwrap_or("metadata"),
                "required": false,
                "available": AVAILABLE_PROFILE_ASSUMPTIONS,
            },
            "blocking_reason": Value::Null,
        }),
        Err(error) => {
            let assumption_required = matches!(error, ColorSourceError::UnknownWhitePoint);
            json!({
                "status": "needs_color_override",
                "supported_profile": Value::Null,
                "profile_assumption": {
                    "selected": Value::Null,
                    "required": assumption_required,
                    "available": AVAILABLE_PROFILE_ASSUMPTIONS,
                },
                "blocking_reason": blocking_reason(&error),
            })
        }
    }
}

/// Classify one source-backed active proof layer with exactly the normative
/// assumption the managed renderer executes, returning both the status
/// evidence and the blocking classifier error when there is one.
///
/// Non-selected active layers are composited into the same BEFORE/AFTER
/// raster as the selected clip, so their source profile is part of the proof's
/// claim and must be reported rather than silently assumed supported.
#[must_use]
pub(crate) fn active_layer_source_classification(
    description: &ColorDescription,
) -> (Value, Option<ColorSourceError>) {
    let assumption = normative_d65_assumption(description);
    let assumption_source = assumption.map(|_| "application_profile_assumption");
    let status = source_status(description, assumption, assumption_source);
    let error = classify_source_with_assumption(description, assumption).err();
    (status, error)
}

fn normative_d65_assumption(
    description: &ColorDescription,
) -> Option<ColorSourceProfileAssumption> {
    (description.white_point == ColorWhitePoint::Unknown
        && classify_source_with_assumption(description, Some(ColorSourceProfileAssumption::D65))
            .is_ok())
    .then_some(ColorSourceProfileAssumption::D65)
}

fn managed_source_profile(
    description: &ColorDescription,
    requested_assumption: Option<ColorSourceProfileAssumption>,
) -> Result<(ColorSourceProfile, Option<ColorSourceProfileAssumption>), ColorSourceError> {
    if let Some(assumption) = requested_assumption {
        return classify_source_with_assumption(description, Some(assumption))
            .map(|profile| (profile, Some(assumption)));
    }
    match classify_source_with_assumption(description, None) {
        Ok(profile) => Ok((profile, None)),
        Err(error) => {
            let Some(assumption) = normative_d65_assumption(description) else {
                return Err(error);
            };
            classify_source_with_assumption(description, Some(assumption))
                .map(|profile| (profile, Some(assumption)))
        }
    }
}

fn blocking_reason(error: &ColorSourceError) -> Value {
    json!({
        "code": error.code(),
        "field": error.field(),
        "observed": error.observed(),
        "allowed": error.allowed_values(),
        "recovery": error.recovery_action(),
        "message": error.actionable_message(),
    })
}

fn clip_status(track_id: u64, z_order: usize, clip: &kinewright_core::Clip) -> Value {
    json!({
        "track_id": track_id,
        "track_kind": TrackKind::Video,
        "z_order": z_order,
        "clip_id": clip.id.0,
        "asset_id": clip.asset.0,
        "content": clip_content_name(&clip.content),
        "timeline_start": clip.timeline_start.0,
        // The complete ordered chain, not just the primary nodes: a legacy
        // stage between two primaries changes the result and must be visible.
        "effects": effect_chain_manifest(&clip.effects),
        // CC3 §8: the ordered managed colour-node stack, in `clip.effects`
        // order, with per-node bypass, activity, and resolved values.
        "color_nodes": color_node_manifest(&clip.effects),
        "legacy_stage_warnings": legacy_stage_warnings(clip),
        // Occlusion is frame-dependent and this surface is not a sampled
        // render; the marker is explicit rather than silently omitted.
        "active_at_frame": Value::Null,
    })
}

/// Ordered evaluated effect-chain manifest shared by `get_color_context` and
/// the `render_color_proof` layer manifest so the two colour surfaces can
/// never describe a different chain for the same clip.
///
/// The vector order is the compositor evaluation order and is intentionally
/// retained: two `primary_correction` nodes with equal values are not
/// interchangeable because the managed pipeline applies them serially.
#[must_use]
pub(crate) fn effect_chain_manifest(effects: &[Effect]) -> Vec<Value> {
    effects
        .iter()
        .enumerate()
        .map(|(effect_index, effect)| {
            let primary_parameters = if effect.name == PRIMARY_CORRECTION_EFFECT_NAME {
                resolved_primary_parameters(effect)
                    .map_or(Value::Null, |parameters| json!(parameters))
            } else {
                Value::Null
            };
            json!({
                "effect_index": effect_index,
                "effect_id": effect.id.0,
                "name": effect.name,
                "parameters": effect.parameters,
                "primary_parameters": primary_parameters,
                // CC3 §8: flag the managed colour nodes of the chain so a
                // reader knows which entries have a fully resolved,
                // bypass-aware description in the sibling `color_nodes` list
                // without duplicating that payload here.
                "color_node_kind": classify_color_node(effect)
                    .map_or(Value::Null, |kind| json!(kind.effect_name())),
                "keyframes": effect.keyframes,
                "compatibility_stage": effect_compatibility_stage(&effect.name)
                    .map_or(Value::Null, |stage| json!(stage.issue_code())),
            })
        })
        .collect()
}

/// Every descriptor control resolved against the stored effect, falling back
/// to the descriptor neutral for controls the effect does not carry.
fn resolved_primary_parameters(effect: &Effect) -> Option<BTreeMap<&'static str, ParamValue>> {
    effect_descriptor(PRIMARY_CORRECTION_EFFECT_NAME).map(|descriptor| {
        descriptor
            .parameters
            .iter()
            .map(|parameter| {
                (
                    parameter.name,
                    effect
                        .parameters
                        .get(parameter.name)
                        .cloned()
                        .unwrap_or(ParamValue::Integer(parameter.neutral)),
                )
            })
            .collect()
    })
}

fn clip_content_name(content: &ClipContent) -> &'static str {
    match content {
        ClipContent::Media => "media",
        ClipContent::Title(_) => "title",
        ClipContent::Freeze(_) => "freeze",
    }
}

/// Classify one effect through Core's single source of truth so agent status,
/// QA, and delivery conformance cannot report different codes for the same
/// effect. `color_grade` deliberately has no arm here: Core canonicalises that
/// wire name to `primary_correction` on load, so it can never reach this
/// function.
fn legacy_warning(effect_index: usize, effect: &Effect) -> Option<Value> {
    let stage = effect_compatibility_stage(&effect.name)?;
    let (compatibility_stage, message) = match stage {
        EffectCompatibilityStage::LegacyDisplayCoded => (
            "legacy_display_coded",
            "legacy display-coded colour semantics are outside CC1 managed conformance",
        ),
        EffectCompatibilityStage::PostPrimaryLut => (
            "post_primary_lut",
            "legacy LUT stage is post-primary and outside CC1 managed conformance",
        ),
    };
    Some(json!({
        "code": stage.issue_code(),
        "effect_id": effect.id.0,
        "effect_index": effect_index,
        "name": effect.name,
        "message": message,
        "stage": "post_primary_legacy",
        "compatibility_stage": compatibility_stage,
        "inspector_warning": stage.inspector_warning(),
    }))
}

/// Validate and construct an unapplied `AddEffect` + `SetEffectParam` proposal.
#[allow(clippy::too_many_lines)]
pub(crate) fn plan_primary_correction(
    document: &Document,
    actual_revision: TimelineRevision,
    args: &PrimaryCorrectionPlanArgs,
) -> Result<PrimaryCorrectionPlan, PrimaryPlanError> {
    if args.expected_revision != actual_revision {
        return Err(PrimaryPlanError::RevisionConflict {
            expected: args.expected_revision,
            actual: actual_revision,
        });
    }
    let (clip, source_profile, profile_assumption) =
        managed_color_clip(document, args.clip_id, args.profile_assumption)?;

    let Some(descriptor) = effect_descriptor(PRIMARY_CORRECTION_EFFECT_NAME) else {
        return Err(PrimaryPlanError::MissingDescriptor);
    };
    for (name, value) in &args.parameters {
        let Some(parameter) = descriptor.parameter(name) else {
            return Err(PrimaryPlanError::UnknownParameter { name: name.clone() });
        };
        if !(parameter.min..=parameter.max).contains(value) {
            return Err(PrimaryPlanError::ParameterOutOfRange {
                name: name.clone(),
                value: *value,
                min: parameter.min,
                max: parameter.max,
            });
        }
    }

    let neutral_parameters = descriptor
        .parameters
        .iter()
        .map(|parameter| {
            (
                parameter.name.to_owned(),
                ParamValue::Integer(parameter.neutral),
            )
        })
        .collect::<BTreeMap<_, _>>();

    // A managed clip carries at most one primary correction. Stacking a second
    // node silently compounds two white-balance/exposure transforms, so an
    // existing node is corrected in place instead.
    let existing = existing_primary_node(document, args.clip_id);
    let existing_primary_node_count = existing.as_ref().map_or(0, |node| node.node_count);
    let mut warnings = Vec::new();
    let (effect_id, created_new_node, current_parameters) = match existing {
        Some(node) => {
            if node.node_count > 1 {
                warnings.push(format!(
                    "clip {} already carries {} primary_correction nodes; this proposal targets the last node ({}) in compositor evaluation order",
                    clip.id, node.node_count, node.effect_id
                ));
            }
            for name in node
                .keyframed
                .iter()
                .filter(|name| args.parameters.contains_key(*name))
            {
                warnings.push(format!(
                    "clip {} node {} keyframes {name}; this proposal writes the static value, which automation overrides at render time",
                    clip.id, node.effect_id
                ));
            }
            (node.effect_id, false, node.parameters)
        }
        None => (
            next_effect_id(document).ok_or(PrimaryPlanError::EffectIdExhausted)?,
            true,
            descriptor
                .parameters
                .iter()
                .map(|parameter| (parameter.name.to_owned(), parameter.neutral))
                .collect::<BTreeMap<_, _>>(),
        ),
    };

    let mut resolved_parameters = current_parameters.clone();
    for (name, value) in &args.parameters {
        resolved_parameters.insert(name.clone(), *value);
    }
    // Only controls whose value actually moves are written. An unchanged
    // control produces no operation, so an empty proposal stays empty.
    let changed = args
        .parameters
        .iter()
        .filter(|(name, value)| current_parameters.get(*name) != Some(*value))
        .map(|(name, value)| Operation::SetEffectParam {
            clip: args.clip_id,
            effect: effect_id,
            name: name.clone(),
            value: ParamValue::Integer(*value),
        })
        .collect::<Vec<_>>();

    let mut operations = Vec::new();
    if !changed.is_empty() {
        if created_new_node {
            operations.push(Operation::AddEffect {
                clip: args.clip_id,
                effect: Effect {
                    id: effect_id,
                    name: PRIMARY_CORRECTION_EFFECT_NAME.to_owned(),
                    parameters: neutral_parameters,
                    keyframes: BTreeMap::new(),
                },
            });
        }
        operations.extend(changed);
    }
    let no_change = operations.is_empty();

    // Core rejects an empty batch, and a no-op proposal is a legitimate answer
    // rather than a rejection: there is simply nothing to validate.
    if !operations.is_empty() {
        let mut candidate = document.clone();
        apply_batch(&mut candidate, &operations)
            .map_err(|error| PrimaryPlanError::CoreRejected(error.to_string()))?;
    }
    Ok(PrimaryCorrectionPlan {
        expected_revision: args.expected_revision,
        clip_id: args.clip_id,
        effect_id,
        created_new_node: created_new_node && !no_change,
        source_profile,
        profile_assumption,
        requested_parameters: args.parameters.clone(),
        resolved_parameters,
        operations,
        existing_primary_node_count,
        warnings,
        no_change,
    })
}

/// The next unused effect id in the whole document.
///
/// Ids are document-wide, so a proposal must not reuse one that another clip
/// already carries. `None` means the id space is exhausted.
fn next_effect_id(document: &Document) -> Option<EffectId> {
    document
        .tracks
        .iter()
        .flat_map(|track| track.clips.iter())
        .flat_map(|clip| clip.effects.iter())
        .map(|effect| effect.id.0)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .map(EffectId)
}

// ---------------------------------------------------------------------------
// CC3 §8 — managed colour-node planners (`plan_color_wheels`, `plan_color_curves`)
// ---------------------------------------------------------------------------

/// Why a clip cannot receive a managed colour node.
///
/// The CC1 primary planner and the CC3 wheels/curves planners share this
/// preamble verbatim so the three surfaces can never disagree about which
/// clips are eligible or which source profile a proposal assumes.
#[derive(Debug, Clone)]
pub(crate) enum ColorClipRejection {
    MissingClip(ClipId),
    WrongClipType {
        clip: ClipId,
        track: TrackKind,
        content: &'static str,
    },
    MissingAsset {
        clip: ClipId,
        asset: AssetId,
    },
    WrongAssetKind {
        clip: ClipId,
        asset: AssetId,
        kind: MediaKind,
    },
    UnsupportedSource {
        clip: ClipId,
        error: ColorSourceError,
    },
}

/// Resolve one managed-colour-eligible clip and the source profile a proposal
/// against it assumes.
fn managed_color_clip(
    document: &Document,
    clip_id: ClipId,
    profile_assumption: Option<ColorSourceProfileAssumption>,
) -> Result<
    (
        &Clip,
        ColorSourceProfile,
        Option<ColorSourceProfileAssumption>,
    ),
    ColorClipRejection,
> {
    let Some(track) = document
        .tracks
        .iter()
        .find(|track| track.clips.iter().any(|clip| clip.id == clip_id))
    else {
        return Err(ColorClipRejection::MissingClip(clip_id));
    };
    let Some(clip) = track.clips.iter().find(|clip| clip.id == clip_id) else {
        return Err(ColorClipRejection::MissingClip(clip_id));
    };
    if track.kind != TrackKind::Video
        || !matches!(clip.content, ClipContent::Media | ClipContent::Freeze(_))
    {
        return Err(ColorClipRejection::WrongClipType {
            clip: clip.id,
            track: track.kind,
            content: clip_content_name(&clip.content),
        });
    }
    let Some(asset) = document.asset(clip.asset) else {
        return Err(ColorClipRejection::MissingAsset {
            clip: clip.id,
            asset: clip.asset,
        });
    };
    if !matches!(asset.kind, MediaKind::Video | MediaKind::AudioVideo) {
        return Err(ColorClipRejection::WrongAssetKind {
            clip: clip.id,
            asset: asset.id,
            kind: asset.kind,
        });
    }
    let (source_profile, applied_assumption) =
        managed_source_profile(&asset.color_description, profile_assumption).map_err(|error| {
            ColorClipRejection::UnsupportedSource {
                clip: clip.id,
                error,
            }
        })?;
    Ok((clip, source_profile, applied_assumption))
}

impl From<ColorClipRejection> for PrimaryPlanError {
    fn from(rejection: ColorClipRejection) -> Self {
        match rejection {
            ColorClipRejection::MissingClip(clip) => Self::MissingClip(clip),
            ColorClipRejection::WrongClipType {
                clip,
                track,
                content,
            } => Self::WrongClipType {
                clip,
                track,
                content,
            },
            ColorClipRejection::MissingAsset { clip, asset } => Self::MissingAsset { clip, asset },
            ColorClipRejection::WrongAssetKind { clip, asset, kind } => {
                Self::WrongAssetKind { clip, asset, kind }
            }
            ColorClipRejection::UnsupportedSource { clip, error } => {
                Self::UnsupportedSource { clip, error }
            }
        }
    }
}

impl From<ColorClipRejection> for ColorNodePlanError {
    fn from(rejection: ColorClipRejection) -> Self {
        match rejection {
            ColorClipRejection::MissingClip(clip) => Self::MissingClip(clip),
            ColorClipRejection::WrongClipType {
                clip,
                track,
                content,
            } => Self::WrongClipType {
                clip,
                track,
                content,
            },
            ColorClipRejection::MissingAsset { clip, asset } => Self::MissingAsset { clip, asset },
            ColorClipRejection::WrongAssetKind { clip, asset, kind } => {
                Self::WrongAssetKind { clip, asset, kind }
            }
            ColorClipRejection::UnsupportedSource { clip, error } => {
                Self::UnsupportedSource { clip, error }
            }
        }
    }
}

/// Arguments for the evidence-only `color_wheels` planner (CC3 §8).
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ColorWheelsPlanArgs {
    /// Exact timeline revision returned by the preceding inspection.
    pub expected_revision: TimelineRevision,
    /// Stable visual media clip id to receive the wheels node.
    pub clip_id: ClipId,
    /// Optional explicit D65 assumption for a complete BT.709 source whose
    /// raw white point is unknown.
    #[serde(default)]
    pub profile_assumption: Option<ColorSourceProfileAssumption>,
    /// Integer CC3 §4.1 controls. Omitted controls resolve to descriptor
    /// neutrals; `bypass` is a 0/1 token.
    pub parameters: BTreeMap<String, i64>,
    /// Stack a second `color_wheels` node instead of correcting the clip's
    /// existing one in place. Ordered node stacks are legal, so this is an
    /// explicit opt-in rather than the default.
    #[serde(default)]
    pub append: bool,
}

/// One curve of a `plan_color_curves` request: `[[x, y], ...]` in basis points.
type CurvePointRequest = Vec<[i64; 2]>;

/// The ergonomic `curves` request object (CC3 §8).
///
/// Unknown keys are rejected rather than ignored: a typo like `reds` would
/// otherwise silently plan nothing for the curve the caller meant.
#[derive(Debug, Clone, Default, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ColorCurvesRequest {
    /// Master curve points, applied identically to all three channels.
    #[serde(default)]
    pub master: Option<CurvePointRequest>,
    #[serde(default)]
    pub red: Option<CurvePointRequest>,
    #[serde(default)]
    pub green: Option<CurvePointRequest>,
    #[serde(default)]
    pub blue: Option<CurvePointRequest>,
}

impl ColorCurvesRequest {
    /// Every requested curve in `ColorCurveChannel::ALL` order.
    fn requested(&self) -> Vec<(ColorCurveChannel, &CurvePointRequest)> {
        ColorCurveChannel::ALL
            .into_iter()
            .filter_map(|curve| self.points(curve).map(|points| (curve, points)))
            .collect()
    }

    fn points(&self, curve: ColorCurveChannel) -> Option<&CurvePointRequest> {
        match curve {
            ColorCurveChannel::Master => self.master.as_ref(),
            ColorCurveChannel::Red => self.red.as_ref(),
            ColorCurveChannel::Green => self.green.as_ref(),
            ColorCurveChannel::Blue => self.blue.as_ref(),
        }
    }
}

/// Arguments for the evidence-only `color_curves` planner (CC3 §8).
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ColorCurvesPlanArgs {
    /// Exact timeline revision returned by the preceding inspection.
    pub expected_revision: TimelineRevision,
    /// Stable visual media clip id to receive the curves node.
    pub clip_id: ClipId,
    /// Optional explicit D65 assumption for a complete BT.709 source whose
    /// raw white point is unknown.
    #[serde(default)]
    pub profile_assumption: Option<ColorSourceProfileAssumption>,
    /// Point lists per curve. 2..=16 points, coordinates in -2000..=12000
    /// basis points of the grade709 range, strictly increasing in x.
    pub curves: ColorCurvesRequest,
    /// Explicit `bypass` token for the node, `0` or `1`.
    #[serde(default)]
    pub bypass: Option<i64>,
    /// Stack a second `color_curves` node instead of editing the clip's
    /// existing one in place.
    #[serde(default)]
    pub append: bool,
}

/// The validated, unapplied CC3 node proposal returned by both planners.
#[derive(Debug, Clone)]
pub(crate) struct ColorNodePlan {
    pub kind: ColorNodeKind,
    pub expected_revision: TimelineRevision,
    pub clip_id: ClipId,
    /// The node the proposal targets: the clip's existing last node of this
    /// kind, or the freshly allocated id for a new one.
    pub effect_id: EffectId,
    /// Whether the plan actually allocates a new node.
    pub created_new_node: bool,
    /// Whether the plan edits a node that already exists.
    pub targets_existing_node: bool,
    pub source_profile: ColorSourceProfile,
    pub profile_assumption: Option<ColorSourceProfileAssumption>,
    /// The request expanded to the exact integer parameters it names.
    pub requested_parameters: BTreeMap<String, i64>,
    /// Every parameter the targeted node carries after the proposal, with
    /// descriptor neutrals merged in.
    pub resolved_parameters: BTreeMap<String, i64>,
    /// Curves only: the request echoed back per curve.
    pub requested_curves: BTreeMap<&'static str, Vec<[i64; 2]>>,
    /// Curves only: every curve's resolved point list after the proposal.
    pub resolved_curves: BTreeMap<&'static str, Vec<[i64; 2]>>,
    pub operations: Vec<Operation>,
    /// Every managed colour node the clip already carries, of any kind.
    pub existing_color_node_count: usize,
    /// Nodes of the requested kind the clip already carries.
    pub existing_nodes_of_kind: usize,
    /// Non-fatal advisories, such as a keyframed target control.
    pub warnings: Vec<String>,
    /// What the proposal takes as given, stated rather than implied.
    pub assumptions: Vec<String>,
    /// True when every requested control already holds the requested value.
    pub no_change: bool,
}

impl ColorNodePlan {
    /// The node this proposal actually publishes.
    ///
    /// A no-op proposal that would have created a node never allocates one, so
    /// reporting `effect_id` there would publish a phantom id that no
    /// operation creates and that a later plan is free to reuse.
    #[must_use]
    pub fn target_effect_id(&self) -> Option<EffectId> {
        if self.no_change && !self.created_new_node && !self.targets_existing_node {
            return None;
        }
        Some(self.effect_id)
    }
}

#[derive(Debug, Error)]
pub(crate) enum ColorNodePlanError {
    #[error("timeline revision conflict: expected {expected}, actual {actual}")]
    RevisionConflict {
        expected: TimelineRevision,
        actual: TimelineRevision,
    },
    #[error("clip {0} does not exist")]
    MissingClip(ClipId),
    #[error("clip {clip} is not a visual media clip (track={track:?}, content={content})")]
    WrongClipType {
        clip: ClipId,
        track: TrackKind,
        content: &'static str,
    },
    #[error("clip {clip} references missing asset {asset}")]
    MissingAsset { clip: ClipId, asset: AssetId },
    #[error("asset {asset} on clip {clip} is not video-capable ({kind:?})")]
    WrongAssetKind {
        clip: ClipId,
        asset: AssetId,
        kind: MediaKind,
    },
    #[error("clip {clip} source is not managed CC1-compatible: {error}")]
    UnsupportedSource {
        clip: ClipId,
        error: ColorSourceError,
    },
    #[error("unknown {effect} parameter {name}")]
    UnknownParameter { effect: &'static str, name: String },
    #[error("{effect} parameter {name}={value} is outside the inclusive range {min}..={max}")]
    ParameterOutOfRange {
        effect: &'static str,
        name: String,
        value: i64,
        min: i64,
        max: i64,
    },
    #[error(
        "color_curves {curve} curve declares {observed} points, outside the inclusive range {min}..={max}"
    )]
    CurvePointCount {
        curve: &'static str,
        observed: usize,
        min: usize,
        max: usize,
    },
    #[error(
        "color_curves {curve} point {index} {axis}={value} is outside the inclusive range {min}..={max}"
    )]
    CurveCoordinateOutOfRange {
        curve: &'static str,
        index: usize,
        axis: &'static str,
        value: i64,
        min: i64,
        max: i64,
    },
    #[error(
        "color_curves {curve} point {index} x={x} does not strictly increase past point {previous_index} x={previous_x}"
    )]
    CurveOrder {
        curve: &'static str,
        index: usize,
        previous_index: usize,
        previous_x: i64,
        x: i64,
    },
    #[error("clip {clip} already carries {actual} managed colour nodes; the limit is {limit}")]
    TooManyColorNodes {
        clip: ClipId,
        actual: usize,
        limit: usize,
    },
    #[error("the Core {0} descriptor is unavailable")]
    MissingDescriptor(&'static str),
    #[error("could not allocate a fresh effect id")]
    EffectIdExhausted,
    #[error("Core rejected the evidence-only colour node plan: {0}")]
    CoreRejected(String),
}

impl ColorNodePlanError {
    /// Stable machine-readable recovery code for structured agent responses.
    #[must_use]
    pub(crate) const fn code(&self) -> &'static str {
        match self {
            Self::RevisionConflict { .. } => "revision_conflict",
            Self::MissingClip(_) => "clip_not_found",
            Self::WrongClipType { .. } => "unsupported_clip_type",
            Self::MissingAsset { .. } => "asset_not_found",
            Self::WrongAssetKind { .. } => "unsupported_asset_kind",
            Self::UnsupportedSource { .. } => "needs_color_override",
            Self::UnknownParameter { .. } => "unknown_color_node_parameter",
            Self::ParameterOutOfRange { .. } => "color_node_parameter_out_of_range",
            Self::CurvePointCount { .. } => "invalid_curve_point_count",
            Self::CurveCoordinateOutOfRange { .. } => "curve_coordinate_out_of_range",
            Self::CurveOrder { .. } => "invalid_curve_points",
            Self::TooManyColorNodes { .. } => "too_many_color_nodes",
            Self::MissingDescriptor(_) => "missing_color_node_descriptor",
            Self::EffectIdExhausted => "effect_id_exhausted",
            Self::CoreRejected(_) => "color_node_plan_core_rejected",
        }
    }

    /// Structured `field`/`observed`/`allowed`/`recovery_action` evidence, in
    /// the CC1/CC2 shape.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub(crate) fn details(&self) -> Value {
        match self {
            Self::RevisionConflict { expected, actual } => json!({
                "field": "expected_revision",
                "observed": expected.0,
                "allowed": actual.0,
                "recovery_action": "Re-inspect with get_color_context and resend the plan at the current timeline_revision.",
                "expected_revision": expected.0,
                "actual_revision": actual.0,
            }),
            Self::MissingClip(clip) => json!({
                "field": "clip_id",
                "observed": clip.0,
                "allowed": "an existing visual media or freeze clip id",
                "recovery_action": "Call get_timeline_state or get_color_context for the current clip ids.",
                "clip_id": clip.0,
            }),
            Self::WrongClipType {
                clip,
                track,
                content,
            } => json!({
                "field": "clip_id",
                "observed": {"clip_id": clip.0, "track_kind": track, "content": content},
                "allowed": "a video-track media or freeze clip",
                "recovery_action": "Target a video-track media or freeze clip; titles and audio clips have no managed colour stage.",
                "clip_id": clip.0,
                "track_kind": track,
                "content": content,
            }),
            Self::MissingAsset { clip, asset } => json!({
                "field": "clip_id",
                "observed": {"clip_id": clip.0, "asset_id": asset.0},
                "allowed": "a clip whose asset is present in the media pool",
                "recovery_action": "Call get_media_status; relink or re-import the missing asset first.",
                "clip_id": clip.0,
                "asset_id": asset.0,
            }),
            Self::WrongAssetKind { clip, asset, kind } => json!({
                "field": "clip_id",
                "observed": {"clip_id": clip.0, "asset_id": asset.0, "media_kind": kind},
                "allowed": ["Video", "AudioVideo"],
                "recovery_action": "Target a clip backed by a video-capable asset.",
                "clip_id": clip.0,
                "asset_id": asset.0,
                "media_kind": kind,
            }),
            Self::UnsupportedSource { clip, error } => json!({
                "field": error.field(),
                "observed": error.observed(),
                "allowed": error.allowed_values(),
                "recovery_action": error.recovery_action(),
                "clip_id": clip.0,
                "code": error.code(),
                "message": error.actionable_message(),
            }),
            Self::UnknownParameter { effect, name } => json!({
                "field": "parameters",
                "observed": name,
                "allowed": color_node_parameter_documentation(effect),
                "recovery_action": format!("Send one of the documented {effect} control names."),
                "parameter": name,
            }),
            Self::ParameterOutOfRange {
                effect,
                name,
                value,
                min,
                max,
            } => json!({
                "field": name,
                "observed": value,
                "allowed": {"min": min, "max": max},
                "recovery_action": format!("Send {name} as an integer in {min}..={max}; see the {effect} descriptor."),
                "parameter": name,
                "min": min,
                "max": max,
            }),
            Self::CurvePointCount {
                curve,
                observed,
                min,
                max,
            } => json!({
                "field": format!("curves.{curve}"),
                "observed": observed,
                "allowed": {"min": min, "max": max},
                "recovery_action": format!("Send {min}..={max} points for the {curve} curve."),
                "curve": curve,
                "point_count": observed,
            }),
            Self::CurveCoordinateOutOfRange {
                curve,
                index,
                axis,
                value,
                min,
                max,
            } => json!({
                "field": format!("curves.{curve}[{index}].{axis}"),
                "observed": value,
                "allowed": {"min": min, "max": max},
                "recovery_action": format!("Send curve coordinates as integers in {min}..={max} basis points of the grade709 range."),
                "curve": curve,
                "index": index,
                "axis": axis,
            }),
            Self::CurveOrder {
                curve,
                index,
                previous_index,
                previous_x,
                x,
            } => json!({
                "field": format!("curves.{curve}[{index}].x"),
                "observed": x,
                "allowed": format!("strictly greater than curves.{curve}[{previous_index}].x = {previous_x}"),
                "recovery_action": "Sort the curve points by x and keep every x strictly increasing; equal x is rejected.",
                "curve": curve,
                "index": index,
                "previous_index": previous_index,
                "previous_x": previous_x,
                "x": x,
            }),
            Self::TooManyColorNodes {
                clip,
                actual,
                limit,
            } => json!({
                "field": "clip_id",
                "observed": actual,
                "allowed": {"max": limit},
                "recovery_action": "Edit an existing colour node in place (omit append) or remove one before stacking another.",
                "clip_id": clip.0,
            }),
            Self::MissingDescriptor(effect) => json!({
                "field": "effect",
                "observed": effect,
                "allowed": MANAGED_COLOR_NODE_NAMES,
                "recovery_action": "This build does not register the requested colour node; upgrade Kinewright.",
            }),
            Self::EffectIdExhausted => json!({
                "field": "effect_id",
                "observed": "exhausted",
                "allowed": "an unused effect id",
                "recovery_action": "The project has exhausted effect ids; remove unused effects.",
            }),
            Self::CoreRejected(message) => json!({
                "field": "operations",
                "observed": message,
                "allowed": "a batch Core accepts against a clone of the analyzed document",
                "recovery_action": "Re-inspect with get_color_context and resend a request Core validates.",
            }),
        }
    }
}

/// Every control of one managed colour node, derived from the Core descriptor
/// so the tool description and the recovery evidence can never drift.
#[must_use]
pub(crate) fn color_node_parameter_documentation(effect: &str) -> Vec<Value> {
    effect_descriptor(effect).map_or_else(Vec::new, |descriptor| {
        descriptor
            .parameters
            .iter()
            .map(|parameter| {
                json!({
                    "name": parameter.name,
                    "min": parameter.min,
                    "max": parameter.max,
                    "neutral": parameter.neutral,
                })
            })
            .collect()
    })
}

/// One-line `name=min..=max, neutral N` summary of every `color_wheels`
/// control. Thirteen entries, so enumeration stays cheap (CC3 §2.4).
#[must_use]
pub(crate) fn color_wheels_parameter_summary() -> String {
    effect_descriptor(ColorNodeKind::Wheels.effect_name()).map_or_else(String::new, |descriptor| {
        descriptor
            .parameters
            .iter()
            .map(|parameter| {
                format!(
                    "{}={}..={}, neutral {}",
                    parameter.name, parameter.min, parameter.max, parameter.neutral
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    })
}

/// The compact `color_curves` request contract. The 133 stored parameters are
/// deliberately never enumerated (CC3 §2.4, M36).
#[must_use]
pub(crate) fn color_curves_request_summary() -> String {
    format!(
        "curves.{{master|red|green|blue}} = [[x, y], ...] with {COLOR_CURVE_MIN_POINTS}..={COLOR_CURVE_MAX_POINTS} points; x and y are integers in {COLOR_CURVE_COORDINATE_MIN}..={COLOR_CURVE_COORDINATE_MAX} basis points of the grade709 range where 0 is black and 10000 is display white; x must strictly increase. The planner expands each list to {{curve}}_point_count and {{curve}}_x{{j}}/{{curve}}_y{{j}} integer parameters. bypass is a 0..=1 token."
    )
}

/// One managed colour node a proposal for a clip would target.
struct ExistingColorNode<'a> {
    effect: &'a Effect,
    /// How many nodes of this kind the clip carries. `effect` is the last one
    /// in compositor evaluation order.
    node_count: usize,
}

/// Resolve the clip's last managed colour node of `kind`, if any.
fn existing_color_node(
    document: &Document,
    clip_id: ClipId,
    kind: ColorNodeKind,
) -> Option<ExistingColorNode<'_>> {
    let clip = document
        .tracks
        .iter()
        .flat_map(|track| track.clips.iter())
        .find(|clip| clip.id == clip_id)?;
    let name = kind.effect_name();
    let matching = clip
        .effects
        .iter()
        .filter(|effect| effect.name == name)
        .collect::<Vec<_>>();
    let effect = *matching.last()?;
    Some(ExistingColorNode {
        effect,
        node_count: matching.len(),
    })
}

/// The stored static integer of one descriptor parameter, or its neutral.
fn stored_parameter(effect: Option<&Effect>, name: &str, neutral: i64) -> i64 {
    match effect.and_then(|effect| effect.parameters.get(name)) {
        Some(ParamValue::Integer(value)) => *value,
        _ => neutral,
    }
}

/// Descriptor parameters of `effect` that carry automation.
fn keyframed_parameters(effect: Option<&Effect>, names: &[&str]) -> Vec<String> {
    let Some(effect) = effect else {
        return Vec::new();
    };
    names
        .iter()
        .filter(|name| {
            effect
                .keyframes
                .get(**name)
                .is_some_and(|curve| !curve.keyframes.is_empty())
        })
        .map(|name| (*name).to_owned())
        .collect()
}

/// Validate and construct an unapplied `color_wheels` proposal (CC3 §8).
///
/// Nothing is sent to Core: the operations are proved valid against a clone of
/// the analyzed document and returned for the caller to submit through
/// `prepare_edit_plan`.
#[allow(clippy::too_many_lines)]
pub(crate) fn plan_color_wheels(
    document: &Document,
    actual_revision: TimelineRevision,
    args: &ColorWheelsPlanArgs,
) -> Result<ColorNodePlan, ColorNodePlanError> {
    if args.expected_revision != actual_revision {
        return Err(ColorNodePlanError::RevisionConflict {
            expected: args.expected_revision,
            actual: actual_revision,
        });
    }
    let kind = ColorNodeKind::Wheels;
    let effect_name = kind.effect_name();
    let (clip, source_profile, profile_assumption) =
        managed_color_clip(document, args.clip_id, args.profile_assumption)?;
    let Some(descriptor) = effect_descriptor(effect_name) else {
        return Err(ColorNodePlanError::MissingDescriptor(effect_name));
    };
    for (name, value) in &args.parameters {
        let Some(parameter) = descriptor.parameter(name) else {
            return Err(ColorNodePlanError::UnknownParameter {
                effect: effect_name,
                name: name.clone(),
            });
        };
        if !(parameter.min..=parameter.max).contains(value) {
            return Err(ColorNodePlanError::ParameterOutOfRange {
                effect: effect_name,
                name: name.clone(),
                value: *value,
                min: parameter.min,
                max: parameter.max,
            });
        }
    }

    let existing_color_node_count = managed_color_node_count(&clip.effects);
    let existing = existing_color_node(document, args.clip_id, kind);
    let existing_nodes_of_kind = existing.as_ref().map_or(0, |node| node.node_count);
    let target = if args.append { None } else { existing.as_ref() };
    let target_effect = target.map(|node| node.effect);

    let mut warnings = Vec::new();
    let mut assumptions = vec![
        "Omitted color_wheels controls resolve to their descriptor neutrals; the node is the exact identity while every control is neutral (CC3 §3.3).".to_owned(),
    ];
    if let Some(node) = target
        && node.node_count > 1
    {
        warnings.push(format!(
            "clip {} already carries {} color_wheels nodes; this proposal targets the last node ({}) in compositor evaluation order",
            args.clip_id, node.node_count, node.effect.id
        ));
    }
    for name in keyframed_parameters(
        target_effect,
        &descriptor
            .parameters
            .iter()
            .map(|parameter| parameter.name)
            .collect::<Vec<_>>(),
    )
    .into_iter()
    .filter(|name| args.parameters.contains_key(name))
    {
        warnings.push(format!(
            "clip {} node {} keyframes {name}; this proposal writes the static value, which automation overrides at render time",
            args.clip_id,
            target_effect.map_or(0, |effect| effect.id.0)
        ));
    }
    if args.append && existing_nodes_of_kind > 0 {
        assumptions.push(format!(
            "append=true stacks a new color_wheels node after the clip's existing {existing_nodes_of_kind}; ordered nodes compose serially and are not merged (CC3 §3.1)."
        ));
    }

    let current = descriptor
        .parameters
        .iter()
        .map(|parameter| {
            (
                parameter.name.to_owned(),
                stored_parameter(target_effect, parameter.name, parameter.neutral),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut resolved_parameters = current.clone();
    for (name, value) in &args.parameters {
        resolved_parameters.insert(name.clone(), *value);
    }

    let changed = args
        .parameters
        .iter()
        .filter(|(name, value)| current.get(*name) != Some(*value))
        .map(|(name, value)| (name.clone(), *value))
        .collect::<Vec<_>>();

    let (effect_id, created_new_node, operations) = if let Some(effect) = target_effect {
        let operations = changed
            .iter()
            .map(|(name, value)| Operation::SetEffectParam {
                clip: args.clip_id,
                effect: effect.id,
                name: name.clone(),
                value: ParamValue::Integer(*value),
            })
            .collect::<Vec<_>>();
        (effect.id, false, operations)
    } else {
        let effect_id = next_effect_id(document).ok_or(ColorNodePlanError::EffectIdExhausted)?;
        // CC3 §2.4: an omitted parameter resolves to its neutral, so a new
        // node stores only the controls the caller actually moved.
        let parameters = changed
            .iter()
            .filter(|(name, value)| {
                descriptor
                    .parameter(name)
                    .is_none_or(|parameter| parameter.neutral != *value)
            })
            .map(|(name, value)| (name.clone(), ParamValue::Integer(*value)))
            .collect::<BTreeMap<_, _>>();
        let operations = if parameters.is_empty() {
            Vec::new()
        } else {
            if existing_color_node_count >= COLOR_NODE_LIMIT_PER_LAYER {
                return Err(ColorNodePlanError::TooManyColorNodes {
                    clip: args.clip_id,
                    actual: existing_color_node_count + 1,
                    limit: COLOR_NODE_LIMIT_PER_LAYER,
                });
            }
            vec![Operation::AddEffect {
                clip: args.clip_id,
                effect: Effect {
                    id: effect_id,
                    name: effect_name.to_owned(),
                    parameters,
                    keyframes: BTreeMap::new(),
                },
            }]
        };
        let created = !operations.is_empty();
        (effect_id, created, operations)
    };

    let no_change = operations.is_empty();
    if !operations.is_empty() {
        let mut candidate = document.clone();
        apply_batch(&mut candidate, &operations)
            .map_err(|error| ColorNodePlanError::CoreRejected(error.to_string()))?;
    }
    Ok(ColorNodePlan {
        kind,
        expected_revision: args.expected_revision,
        clip_id: args.clip_id,
        effect_id,
        created_new_node,
        targets_existing_node: target_effect.is_some(),
        source_profile,
        profile_assumption,
        requested_parameters: args.parameters.clone(),
        resolved_parameters,
        requested_curves: BTreeMap::new(),
        resolved_curves: BTreeMap::new(),
        operations,
        existing_color_node_count,
        existing_nodes_of_kind,
        warnings,
        assumptions,
        no_change,
    })
}

/// Validate one requested curve against CC3 §2.3 count, bounds, and ordering.
fn validate_requested_curve(
    curve: ColorCurveChannel,
    points: &[[i64; 2]],
) -> Result<(), ColorNodePlanError> {
    if points.len() < COLOR_CURVE_MIN_POINTS || points.len() > COLOR_CURVE_MAX_POINTS {
        return Err(ColorNodePlanError::CurvePointCount {
            curve: curve.name(),
            observed: points.len(),
            min: COLOR_CURVE_MIN_POINTS,
            max: COLOR_CURVE_MAX_POINTS,
        });
    }
    for (index, [x, y]) in points.iter().enumerate() {
        for (axis, value) in [("x", *x), ("y", *y)] {
            if !(COLOR_CURVE_COORDINATE_MIN..=COLOR_CURVE_COORDINATE_MAX).contains(&value) {
                return Err(ColorNodePlanError::CurveCoordinateOutOfRange {
                    curve: curve.name(),
                    index,
                    axis,
                    value,
                    min: COLOR_CURVE_COORDINATE_MIN,
                    max: COLOR_CURVE_COORDINATE_MAX,
                });
            }
        }
        if index > 0 && *x <= points[index - 1][0] {
            return Err(ColorNodePlanError::CurveOrder {
                curve: curve.name(),
                index,
                previous_index: index - 1,
                previous_x: points[index - 1][0],
                x: *x,
            });
        }
    }
    Ok(())
}

/// The `{curve}_point_count` and active coordinate parameters one point list
/// expands to (CC3 §2.4). Points at index `>= point_count` are omitted.
fn curve_parameters(curve: ColorCurveChannel, points: &[[i64; 2]]) -> BTreeMap<String, i64> {
    let minimum = i64::try_from(COLOR_CURVE_MIN_POINTS).unwrap_or(2);
    let mut parameters = BTreeMap::from([(
        curve.point_count_parameter().to_owned(),
        i64::try_from(points.len()).unwrap_or(minimum),
    )]);
    for (index, [x, y]) in points.iter().enumerate() {
        if let Some(name) = curve.x_parameter(index) {
            parameters.insert(name.to_owned(), *x);
        }
        if let Some(name) = curve.y_parameter(index) {
            parameters.insert(name.to_owned(), *y);
        }
    }
    parameters
}

/// The point list one `color_curves` effect currently stores for `curve`.
fn stored_curve_points(effect: Option<&Effect>, curve: ColorCurveChannel) -> Vec<[i64; 2]> {
    let Some(effect) = effect else {
        let white = i64::from(COLOR_CURVE_WHITE_BASIS_POINTS);
        return vec![[0, 0], [white, white]];
    };
    CurvePoints::from_effect(effect, curve)
        .points
        .into_iter()
        .map(|(x, y)| [i64::from(x), i64::from(y)])
        .collect()
}

/// Ordered `SetEffectParam` operations that move one curve of an existing node
/// from its stored points to `target`.
///
/// Core validates the strictly-increasing `x` rule on every individual
/// `SetEffectParam` against the map the change would produce, so the write
/// order is part of the contract:
///
/// 1. Collapse `{curve}_point_count` to two. A prefix of a valid point list is
///    still strictly increasing, so this step can never be rejected.
/// 2. Move points 0 and 1. With stored `a0 < a1` and requested `b0 < b1`, at
///    least one of the two write orders is legal: if neither `b0 < a1` nor
///    `a0 < b1` held, then `b0 >= a1 > a0 >= b1` would give `b0 > b1`, which
///    contradicts the validated request.
/// 3. Write points 2.. while they are still inactive, so they are not examined.
/// 4. Restore `{curve}_point_count` to the requested count.
fn curve_operations(
    clip: ClipId,
    effect_id: EffectId,
    effect: &Effect,
    curve: ColorCurveChannel,
    target: &[[i64; 2]],
) -> Vec<Operation> {
    if stored_curve_points(Some(effect), curve) == target {
        return Vec::new();
    }
    let descriptors = curve.parameters();
    let stored = |name: &str| {
        let neutral = descriptors
            .iter()
            .find(|parameter| parameter.name == name)
            .map_or(0, |parameter| parameter.neutral);
        stored_parameter(Some(effect), name, neutral)
    };
    let operation = |name: &str, value: i64| Operation::SetEffectParam {
        clip,
        effect: effect_id,
        name: name.to_owned(),
        value: ParamValue::Integer(value),
    };
    let minimum = i64::try_from(COLOR_CURVE_MIN_POINTS).unwrap_or(2);
    let count = i64::try_from(target.len()).unwrap_or(minimum);
    let point_count = curve.point_count_parameter();
    let declared = stored(point_count);

    let mut operations = Vec::new();
    let mut active_count = declared;
    if declared > minimum {
        operations.push(operation(point_count, minimum));
        active_count = minimum;
    }
    // `x_parameter(1)` always exists; the saturating fallback simply keeps
    // the natural index order if a future descriptor ever shrinks.
    let stored_x1 = curve.x_parameter(1).map_or(i64::MAX, &stored);
    let leading_order = if target[0][0] < stored_x1 {
        [0_usize, 1]
    } else {
        [1, 0]
    };
    for index in leading_order.into_iter().chain(2..target.len()) {
        for (name, value) in [
            (curve.x_parameter(index), target[index][0]),
            (curve.y_parameter(index), target[index][1]),
        ] {
            let Some(name) = name else { continue };
            if stored(name) != value {
                operations.push(operation(name, value));
            }
        }
    }
    if active_count != count {
        operations.push(operation(point_count, count));
    }
    operations
}

/// Validate and construct an unapplied `color_curves` proposal (CC3 §8).
#[allow(clippy::too_many_lines)]
pub(crate) fn plan_color_curves(
    document: &Document,
    actual_revision: TimelineRevision,
    args: &ColorCurvesPlanArgs,
) -> Result<ColorNodePlan, ColorNodePlanError> {
    if args.expected_revision != actual_revision {
        return Err(ColorNodePlanError::RevisionConflict {
            expected: args.expected_revision,
            actual: actual_revision,
        });
    }
    let kind = ColorNodeKind::Curves;
    let effect_name = kind.effect_name();
    let (clip, source_profile, profile_assumption) =
        managed_color_clip(document, args.clip_id, args.profile_assumption)?;
    let Some(descriptor) = effect_descriptor(effect_name) else {
        return Err(ColorNodePlanError::MissingDescriptor(effect_name));
    };
    let requested = args.curves.requested();
    for (curve, points) in &requested {
        validate_requested_curve(*curve, points)?;
    }
    if let Some(bypass) = args.bypass {
        let Some(parameter) = descriptor.parameter(COLOR_NODE_BYPASS_PARAMETER) else {
            return Err(ColorNodePlanError::MissingDescriptor(effect_name));
        };
        if !(parameter.min..=parameter.max).contains(&bypass) {
            return Err(ColorNodePlanError::ParameterOutOfRange {
                effect: effect_name,
                name: COLOR_NODE_BYPASS_PARAMETER.to_owned(),
                value: bypass,
                min: parameter.min,
                max: parameter.max,
            });
        }
    }

    let existing_color_node_count = managed_color_node_count(&clip.effects);
    let existing = existing_color_node(document, args.clip_id, kind);
    let existing_nodes_of_kind = existing.as_ref().map_or(0, |node| node.node_count);
    let target = if args.append { None } else { existing.as_ref() };
    let target_effect = target.map(|node| node.effect);

    let mut warnings = Vec::new();
    let mut assumptions = vec![
        "Points at index >= {curve}_point_count are omitted from the stored parameter map; they resolve to their neutrals and are ignored by rendering (CC3 §2.4).".to_owned(),
    ];
    if let Some(node) = target
        && node.node_count > 1
    {
        warnings.push(format!(
            "clip {} already carries {} color_curves nodes; this proposal targets the last node ({}) in compositor evaluation order",
            args.clip_id, node.node_count, node.effect.id
        ));
    }
    let unspecified = ColorCurveChannel::ALL
        .into_iter()
        .filter(|curve| args.curves.points(*curve).is_none())
        .map(ColorCurveChannel::name)
        .collect::<Vec<_>>();
    if !unspecified.is_empty() {
        assumptions.push(if target_effect.is_some() {
            format!(
                "the {} curve(s) are not named in this request, so node {} keeps its current points for them; send them explicitly to change them",
                unspecified.join(", "),
                target_effect.map_or(0, |effect| effect.id.0)
            )
        } else {
            format!(
                "the {} curve(s) are not named in this request, so the new node leaves them at the structural identity (0,0)-(10000,10000)",
                unspecified.join(", ")
            )
        });
    }
    if args.append && existing_nodes_of_kind > 0 {
        assumptions.push(format!(
            "append=true stacks a new color_curves node after the clip's existing {existing_nodes_of_kind}; ordered nodes compose serially and are not merged (CC3 §3.1)."
        ));
    }

    let mut requested_parameters = BTreeMap::new();
    for (curve, points) in &requested {
        requested_parameters.extend(curve_parameters(*curve, points));
    }
    if let Some(bypass) = args.bypass {
        requested_parameters.insert(COLOR_NODE_BYPASS_PARAMETER.to_owned(), bypass);
    }
    for name in keyframed_parameters(
        target_effect,
        &descriptor
            .parameters
            .iter()
            .map(|parameter| parameter.name)
            .collect::<Vec<_>>(),
    )
    .into_iter()
    .filter(|name| requested_parameters.contains_key(name))
    {
        warnings.push(format!(
            "clip {} node {} keyframes {name}; this proposal writes the static value, which automation overrides at render time",
            args.clip_id,
            target_effect.map_or(0, |effect| effect.id.0)
        ));
    }

    let mut requested_curves = BTreeMap::new();
    for (curve, points) in &requested {
        requested_curves.insert(curve.name(), (*points).clone());
    }
    let mut resolved_curves = BTreeMap::new();
    for curve in ColorCurveChannel::ALL {
        let points = args
            .curves
            .points(curve)
            .cloned()
            .unwrap_or_else(|| stored_curve_points(target_effect, curve));
        resolved_curves.insert(curve.name(), points);
    }
    let mut resolved_parameters = BTreeMap::new();
    for curve in ColorCurveChannel::ALL {
        resolved_parameters.extend(curve_parameters(curve, &resolved_curves[curve.name()]));
    }
    resolved_parameters.insert(
        COLOR_NODE_BYPASS_PARAMETER.to_owned(),
        args.bypass
            .unwrap_or_else(|| stored_parameter(target_effect, COLOR_NODE_BYPASS_PARAMETER, 0)),
    );

    let (effect_id, created_new_node, operations) = if let Some(effect) = target_effect {
        let mut operations = Vec::new();
        for (curve, points) in &requested {
            operations.extend(curve_operations(
                args.clip_id,
                effect.id,
                effect,
                *curve,
                points,
            ));
        }
        if let Some(bypass) = args.bypass
            && stored_parameter(Some(effect), COLOR_NODE_BYPASS_PARAMETER, 0) != bypass
        {
            operations.push(Operation::SetEffectParam {
                clip: args.clip_id,
                effect: effect.id,
                name: COLOR_NODE_BYPASS_PARAMETER.to_owned(),
                value: ParamValue::Integer(bypass),
            });
        }
        (effect.id, false, operations)
    } else {
        let effect_id = next_effect_id(document).ok_or(ColorNodePlanError::EffectIdExhausted)?;
        // CC3 §2.4: store only the parameters that move off their neutrals.
        let parameters = requested_parameters
            .iter()
            .filter(|(name, value)| {
                descriptor
                    .parameter(name)
                    .is_none_or(|parameter| parameter.neutral != **value)
            })
            .map(|(name, value)| (name.clone(), ParamValue::Integer(*value)))
            .collect::<BTreeMap<_, _>>();
        let operations = if parameters.is_empty() {
            Vec::new()
        } else {
            if existing_color_node_count >= COLOR_NODE_LIMIT_PER_LAYER {
                return Err(ColorNodePlanError::TooManyColorNodes {
                    clip: args.clip_id,
                    actual: existing_color_node_count + 1,
                    limit: COLOR_NODE_LIMIT_PER_LAYER,
                });
            }
            vec![Operation::AddEffect {
                clip: args.clip_id,
                effect: Effect {
                    id: effect_id,
                    name: effect_name.to_owned(),
                    parameters,
                    keyframes: BTreeMap::new(),
                },
            }]
        };
        let created = !operations.is_empty();
        (effect_id, created, operations)
    };

    let no_change = operations.is_empty();
    if !operations.is_empty() {
        let mut candidate = document.clone();
        apply_batch(&mut candidate, &operations)
            .map_err(|error| ColorNodePlanError::CoreRejected(error.to_string()))?;
    }
    Ok(ColorNodePlan {
        kind,
        expected_revision: args.expected_revision,
        clip_id: args.clip_id,
        effect_id,
        created_new_node,
        targets_existing_node: target_effect.is_some(),
        source_profile,
        profile_assumption,
        requested_parameters,
        resolved_parameters,
        requested_curves,
        resolved_curves,
        operations,
        existing_color_node_count,
        existing_nodes_of_kind,
        warnings,
        assumptions,
        no_change,
    })
}

// ---------------------------------------------------------------------------
// CC3 §8 — the ordered `color_nodes` manifest
// ---------------------------------------------------------------------------

/// One managed colour node of the ordered CC1/CC3 stack, fully resolved.
///
/// `effects` must already be keyframe-evaluated when the caller renders a
/// frame (`render_color_proof` passes `visual_layers_at` output); the stored
/// static values are the honest answer for the metadata-only
/// `get_color_context` surface.
#[must_use]
#[allow(clippy::too_many_lines)]
pub(crate) fn color_node_value(effect_index: usize, effect: &Effect) -> Option<Value> {
    let kind = classify_color_node(effect)?;
    let mut warnings = Vec::new();
    let (bypass, inactive_reason, parameters, curves) = match kind {
        // CC1 primaries have neither a bypass control nor a neutral
        // short-circuit, so they are always evaluated (CC3 §3.3).
        ColorNodeKind::Primary => (
            0,
            None,
            resolved_descriptor_parameters(effect, kind.effect_name()),
            Value::Null,
        ),
        ColorNodeKind::Wheels => {
            let resolved = ColorWheelsParams::from_effect(effect);
            let mut parameters = BTreeMap::new();
            for control in ColorWheelControl::ALL {
                for channel in ColorWheelChannel::ALL {
                    parameters.insert(
                        control.parameter_name(channel).to_owned(),
                        resolved.control(control, channel),
                    );
                }
            }
            parameters.insert(
                COLOR_NODE_BYPASS_PARAMETER.to_owned(),
                resolved.bypass_token,
            );
            (
                resolved.bypass_token,
                resolved.inactive_reason(),
                parameters,
                Value::Null,
            )
        }
        ColorNodeKind::Curves => {
            let resolved = ResolvedCurves::from_effect(effect);
            let mut parameters = BTreeMap::new();
            let mut curves = serde_json::Map::new();
            for curve in ColorCurveChannel::ALL {
                let points = resolved.curve(curve);
                parameters.insert(
                    curve.point_count_parameter().to_owned(),
                    i64::try_from(points.points.len()).unwrap_or_default(),
                );
                for (index, (x, y)) in points.points.iter().enumerate() {
                    if let Some(name) = curve.x_parameter(index) {
                        parameters.insert(name.to_owned(), i64::from(*x));
                    }
                    if let Some(name) = curve.y_parameter(index) {
                        parameters.insert(name.to_owned(), i64::from(*y));
                    }
                }
                curves.insert(
                    curve.name().to_owned(),
                    json!({
                        "points": points.points.iter().map(|(x, y)| json!([x, y])).collect::<Vec<_>>(),
                        "point_count": points.points.len(),
                        "declared_point_count": points.declared_point_count,
                        "truncated": points.truncated,
                        "structural_identity": points.is_structural_identity(),
                    }),
                );
            }
            parameters.insert(
                COLOR_NODE_BYPASS_PARAMETER.to_owned(),
                resolved.bypass_token,
            );
            // A bypassed node renders as the exact identity (CC3 §5), so its
            // truncation changes no pixel. Core QA and the inspector both
            // suppress the warning there; the agent surface must agree or the
            // three descriptions of one node disagree.
            let truncated = if resolved.bypass() {
                Vec::new()
            } else {
                resolved.truncated_curves()
            };
            if !truncated.is_empty() {
                let names = truncated
                    .iter()
                    .map(|curve| curve.name())
                    .collect::<Vec<_>>();
                warnings.push(json!({
                    "code": "curve_truncated_by_automation",
                    "effect_id": effect.id.0,
                    "stage_index": effect_index,
                    "curves": names,
                    "message": format!(
                        "effect {} resolves {} without strictly increasing x, so each curve is truncated to its longest valid prefix (CC3 §3.4)",
                        effect.id,
                        names.join(", "),
                    ),
                }));
            }
            (
                resolved.bypass_token,
                resolved.inactive_reason(),
                parameters,
                Value::Object(curves),
            )
        }
    };
    Some(json!({
        // Position in `clip.effects`, which is the compositor evaluation order.
        "stage_index": effect_index,
        "effect_index": effect_index,
        "effect_id": effect.id.0,
        "kind": kind.effect_name(),
        "name": effect.name,
        "bypass": bypass,
        // CC1 primaries carry no bypass control (CC3 §5 applies to CC3 nodes).
        "supports_bypass": kind != ColorNodeKind::Primary,
        "active": inactive_reason.is_none(),
        "inactive_reason": inactive_reason.map_or(Value::Null, |reason| json!(reason.as_str())),
        "parameters": parameters,
        "curves": curves,
        "keyframes": effect.keyframes,
        "warnings": warnings,
    }))
}

/// The ordered managed colour-node stack of one effect chain (CC3 §8).
///
/// Shared by `get_color_context` and the `render_color_proof` layer manifest
/// so the two colour surfaces can never describe a different node stack.
#[must_use]
pub(crate) fn color_node_manifest(effects: &[Effect]) -> Vec<Value> {
    effects
        .iter()
        .enumerate()
        .filter_map(|(effect_index, effect)| color_node_value(effect_index, effect))
        .collect()
}

/// Every descriptor control resolved against the stored effect, falling back
/// to the descriptor neutral for controls the effect does not carry.
fn resolved_descriptor_parameters(effect: &Effect, name: &str) -> BTreeMap<String, i64> {
    effect_descriptor(name).map_or_else(BTreeMap::new, |descriptor| {
        descriptor
            .parameters
            .iter()
            .map(|parameter| {
                (
                    parameter.name.to_owned(),
                    stored_parameter(Some(effect), parameter.name, parameter.neutral),
                )
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinewright_core::{
        AssetId, AudioMix, Clip, ColorBitDepth, ColorContext, ColorMatrix, ColorPrimaries,
        ColorProvenance, ColorRange, ColorTransfer, MediaCatalog, MediaSourceFingerprint, Rational,
        Title, Track, TrackId,
    };

    fn description() -> ColorDescription {
        ColorDescription {
            primaries: ColorPrimaries::Bt709,
            transfer: ColorTransfer::Bt709,
            matrix: ColorMatrix::Bt709,
            range: ColorRange::Limited,
            white_point: ColorWhitePoint::D65,
            bit_depth: ColorBitDepth::Eight,
            confidence_basis_points: 10_000,
            provenance: ColorProvenance::StreamMetadata,
        }
    }

    fn document() -> Document {
        let asset = kinewright_core::MediaAsset {
            id: AssetId(1),
            path: "source.mp4".into(),
            name: "source".to_owned(),
            duration: kinewright_core::TimeCode(100),
            fps: Rational::new(30, 1).expect("valid fps"),
            kind: MediaKind::Video,
            resolution: Some((1920, 1080)),
            source_fingerprint: MediaSourceFingerprint::default(),
            color_description: description(),
        };
        Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![Clip {
                    id: ClipId(1),
                    asset: AssetId(1),
                    source_range: kinewright_core::TimeCode(0)..kinewright_core::TimeCode(100),
                    content: ClipContent::Media,
                    timeline_start: kinewright_core::TimeCode(0),
                    effects: Vec::new(),
                    transition_in: None,
                    link: None,
                    audio_gain_tenth_db: 0,
                    audio_fade_in_frames: kinewright_core::TimeCode(0),
                    audio_fade_out_frames: kinewright_core::TimeCode(0),
                    speed_percent: 100,
                }],
            }],
            media_pool: vec![asset],
            markers: Vec::new(),
            catalog: MediaCatalog::default(),
            audio_mix: AudioMix::default(),
            color_context: ColorContext::default(),
            fps: Rational::new(30, 1).expect("valid fps"),
            resolution: (1920, 1080),
            duration: kinewright_core::TimeCode(100),
        }
    }

    #[test]
    fn status_reports_supported_profile_and_referenced_scope() {
        let value = color_context_value(TimelineRevision(3), &document());
        assert_eq!(value["timeline_revision"], 3);
        assert_eq!(
            value["assets"][0]["source"]["status"]["status"],
            "supported"
        );
        assert_eq!(
            value["assets"][0]["source"]["status"]["supported_profile"],
            ColorSourceProfile::Rec709Video.id()
        );
        assert_eq!(value["assets"][0]["managed_blocking"], false);
        assert_eq!(
            value["clips"][0]["color_nodes"].as_array().unwrap().len(),
            0
        );
        assert_eq!(value["ordered_stage_names"].as_array().unwrap().len(), 8);
    }

    #[test]
    fn status_blocks_only_referenced_unsupported_sources() {
        let mut document = document();
        document.media_pool.push(kinewright_core::MediaAsset {
            id: AssetId(2),
            path: "unused.mp4".into(),
            name: "unused".to_owned(),
            duration: kinewright_core::TimeCode(100),
            fps: Rational::new(30, 1).expect("valid fps"),
            kind: MediaKind::Video,
            resolution: Some((1920, 1080)),
            source_fingerprint: MediaSourceFingerprint::default(),
            color_description: ColorDescription::unknown(),
        });
        document.media_pool[0].color_description = ColorDescription::unknown();
        let value = color_context_value(TimelineRevision(0), &document);
        assert_eq!(value["assets"][0]["managed_blocking"], true);
        assert_eq!(value["assets"][1]["managed_blocking"], false);
        assert_eq!(value["managed_blocking_asset_ids"], json!([1]));
    }

    #[test]
    fn status_records_explicit_d65_assumption_without_rewriting_raw_metadata() {
        let mut document = document();
        document.media_pool[0].color_description.white_point = ColorWhitePoint::Unknown;
        let value = color_context_value_with_assumptions(
            TimelineRevision(0),
            &document,
            Some(ColorSourceProfileAssumption::D65),
            &[AssetId(1)],
        );
        assert_eq!(value["assets"][0]["managed_blocking"], false);
        assert_eq!(
            value["assets"][0]["source"]["status"]["supported_profile"],
            "rec709_video"
        );
        assert_eq!(
            value["assets"][0]["source"]["status"]["profile_assumption"]["selected"],
            "d65"
        );
        assert_eq!(
            value["assets"][0]["source"]["raw_description"]["white_point"],
            "unknown"
        );
    }

    #[test]
    fn default_status_matches_renderer_d65_assumption_and_raw_only_is_available() {
        let mut document = document();
        document.media_pool[0].color_description.white_point = ColorWhitePoint::Unknown;
        let value = color_context_value(TimelineRevision(0), &document);
        assert_eq!(value["assets"][0]["managed_blocking"], false);
        assert_eq!(
            value["assets"][0]["source"]["status"]["supported_profile"],
            "rec709_video"
        );
        assert_eq!(
            value["assets"][0]["source"]["status"]["profile_assumption"]["selected"],
            "d65"
        );
        assert_eq!(
            value["assets"][0]["source"]["status"]["profile_assumption"]["source"],
            "application_profile_assumption"
        );
        assert_eq!(
            value["assets"][0]["source"]["raw_description"]["white_point"],
            "unknown"
        );
        let raw = color_context_value_with_options(TimelineRevision(0), &document, None, &[], true);
        assert_eq!(raw["assets"][0]["managed_blocking"], true);
        assert_eq!(
            raw["assets"][0]["source"]["status"]["blocking_reason"]["code"],
            "unknown_source_white_point"
        );
    }

    #[test]
    fn primary_plan_is_revision_bound_and_never_applies() {
        let document = document();
        let args = PrimaryCorrectionPlanArgs {
            expected_revision: TimelineRevision(4),
            clip_id: ClipId(1),
            profile_assumption: None,
            parameters: BTreeMap::from([
                ("exposure_milli_stops".to_owned(), 1_000),
                ("saturation_percent".to_owned(), -100),
            ]),
        };
        let plan = plan_primary_correction(&document, TimelineRevision(4), &args)
            .expect("valid primary plan");
        assert_eq!(plan.operations.len(), 3);
        assert!(matches!(plan.operations[0], Operation::AddEffect { .. }));
        assert!(
            plan.operations
                .iter()
                .skip(1)
                .all(|operation| matches!(operation, Operation::SetEffectParam { .. }))
        );
        assert_eq!(plan.resolved_parameters.len(), 10);
        assert_eq!(plan.resolved_parameters["exposure_milli_stops"], 1_000);
        assert_eq!(
            plan.resolved_parameters["contrast_pivot_basis_points"],
            5_000
        );
        assert_eq!(document.clip(ClipId(1)).unwrap().effects.len(), 0);
    }

    #[test]
    fn primary_plan_uses_normative_d65_assumption_when_omitted() {
        let mut document = document();
        document.media_pool[0].color_description.white_point = ColorWhitePoint::Unknown;
        let args = PrimaryCorrectionPlanArgs {
            expected_revision: TimelineRevision(0),
            clip_id: ClipId(1),
            profile_assumption: None,
            parameters: BTreeMap::new(),
        };
        let plan = plan_primary_correction(&document, TimelineRevision(0), &args)
            .expect("managed renderer assumption should make the source eligible");
        assert_eq!(plan.source_profile.id(), "rec709_video");
        assert_eq!(
            plan.profile_assumption,
            Some(ColorSourceProfileAssumption::D65)
        );
    }

    #[test]
    fn primary_plan_rejects_stale_wrong_type_unknown_and_out_of_range() {
        let document = document();
        let base = |parameters| PrimaryCorrectionPlanArgs {
            expected_revision: TimelineRevision(0),
            clip_id: ClipId(1),
            profile_assumption: None,
            parameters,
        };
        assert!(matches!(
            plan_primary_correction(&document, TimelineRevision(1), &base(BTreeMap::new())),
            Err(PrimaryPlanError::RevisionConflict { .. })
        ));
        assert!(matches!(
            plan_primary_correction(
                &document,
                TimelineRevision(0),
                &PrimaryCorrectionPlanArgs {
                    clip_id: ClipId(99),
                    ..base(BTreeMap::new())
                }
            ),
            Err(PrimaryPlanError::MissingClip(ClipId(99)))
        ));
        let mut title_document = document.clone();
        title_document.tracks[0].clips[0].content = ClipContent::Title(Title::default());
        assert!(matches!(
            plan_primary_correction(&title_document, TimelineRevision(0), &base(BTreeMap::new())),
            Err(PrimaryPlanError::WrongClipType { .. })
        ));
        assert!(matches!(
            plan_primary_correction(
                &document,
                TimelineRevision(0),
                &base(BTreeMap::from([("unknown".to_owned(), 0)]))
            ),
            Err(PrimaryPlanError::UnknownParameter { .. })
        ));
        assert!(matches!(
            plan_primary_correction(
                &document,
                TimelineRevision(0),
                &base(BTreeMap::from([("saturation_percent".to_owned(), 101)]))
            ),
            Err(PrimaryPlanError::ParameterOutOfRange { .. })
        ));
        let mut unsupported_document = document;
        unsupported_document.media_pool[0].color_description = ColorDescription::unknown();
        assert!(matches!(
            plan_primary_correction(
                &unsupported_document,
                TimelineRevision(0),
                &base(BTreeMap::new())
            ),
            Err(PrimaryPlanError::UnsupportedSource { .. })
        ));
    }
    #[test]
    fn raw_only_with_an_explicit_assumption_is_a_typed_conflict() {
        assert!(raw_only_conflict(&ColorContextArgs::default()).is_none());
        assert!(
            raw_only_conflict(&ColorContextArgs {
                profile_assumption: Some(ColorSourceProfileAssumption::D65),
                raw_only: false,
                asset_ids: Vec::new(),
            })
            .is_none()
        );
        let conflict = raw_only_conflict(&ColorContextArgs {
            profile_assumption: Some(ColorSourceProfileAssumption::D65),
            raw_only: true,
            asset_ids: vec![AssetId(1)],
        })
        .expect("raw_only plus an explicit assumption must be rejected");
        assert_eq!(
            conflict["code"],
            "raw_only_conflicts_with_profile_assumption"
        );
        assert_eq!(conflict["details"]["profile_assumption"], "d65");
        assert_eq!(conflict["details"]["asset_ids"], json!([1]));
    }

    #[test]
    fn legacy_warnings_use_the_core_compatibility_stage_codes() {
        let warning = |name: &str| {
            legacy_warning(
                0,
                &Effect {
                    id: EffectId(1),
                    name: name.to_owned(),
                    parameters: BTreeMap::new(),
                    keyframes: BTreeMap::new(),
                },
            )
        };
        for name in ["brightness", "contrast", "saturation"] {
            let value = warning(name).unwrap();
            assert_eq!(value["code"], "legacy_colour_semantics");
            assert_eq!(value["compatibility_stage"], "legacy_display_coded");
        }
        for name in ["look_lut", "cube_lut"] {
            assert_eq!(warning(name).unwrap()["code"], "legacy_lut_stage");
        }
        // Core canonicalises color_grade to primary_correction on load, so the
        // dead arm is gone and neither name is a compatibility stage.
        assert!(warning("color_grade").is_none());
        assert!(warning("primary_correction").is_none());
        assert!(warning("opacity").is_none());
    }

    #[test]
    fn status_reports_video_only_layers_with_z_order_and_the_full_chain() {
        let mut document = document();
        document.tracks[0].clips[0].effects.push(Effect {
            id: EffectId(1),
            name: "primary_correction".to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::new(),
        });
        document.tracks[0].clips[0].effects.push(Effect {
            id: EffectId(2),
            name: "look_lut".to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::new(),
        });
        // An audio clip must not appear in a colour layer list at all, and its
        // effects are not part of the CC1 image chain: a legacy-named effect
        // there must not produce a colour compatibility warning either.
        document.tracks.push(Track {
            id: TrackId(2),
            kind: TrackKind::Audio,
            sync_lock: true,
            clips: vec![Clip {
                id: ClipId(2),
                asset: AssetId(1),
                source_range: kinewright_core::TimeCode(0)..kinewright_core::TimeCode(100),
                content: ClipContent::Media,
                timeline_start: kinewright_core::TimeCode(0),
                effects: vec![Effect {
                    id: EffectId(3),
                    name: "look_lut".to_owned(),
                    parameters: BTreeMap::new(),
                    keyframes: BTreeMap::new(),
                }],
                transition_in: None,
                link: None,
                audio_gain_tenth_db: 0,
                audio_fade_in_frames: kinewright_core::TimeCode(0),
                audio_fade_out_frames: kinewright_core::TimeCode(0),
                speed_percent: 100,
            }],
        });

        let value = color_context_value(TimelineRevision(0), &document);
        let clips = value["clips"].as_array().unwrap();
        assert_eq!(clips.len(), 1, "audio-track clips are not colour layers");
        assert_eq!(clips[0]["z_order"], 0);
        assert_eq!(clips[0]["track_kind"], "Video");
        assert_eq!(clips[0]["active_at_frame"], Value::Null);
        let effects = clips[0]["effects"].as_array().unwrap();
        assert_eq!(effects.len(), 2, "the full ordered chain must be visible");
        assert_eq!(effects[0]["effect_index"], 0);
        assert_eq!(effects[0]["name"], "primary_correction");
        assert_eq!(effects[0]["color_node_kind"], "primary_correction");
        assert_eq!(effects[1]["name"], "look_lut");
        assert_eq!(effects[1]["color_node_kind"], Value::Null);
        assert_eq!(effects[1]["compatibility_stage"], "legacy_lut_stage");
        let color_nodes = clips[0]["color_nodes"].as_array().unwrap();
        assert_eq!(
            color_nodes.len(),
            1,
            "look_lut is not a managed colour node"
        );
        assert_eq!(color_nodes[0]["stage_index"], 0);
        assert_eq!(color_nodes[0]["kind"], "primary_correction");
        assert_eq!(color_nodes[0]["active"], true);
        assert_eq!(color_nodes[0]["inactive_reason"], Value::Null);
        assert_eq!(
            value["assets"][0]["source"]["formats"]["input"]["raster"],
            json!([1920, 1080])
        );
        assert_eq!(value["sampling_region"], Value::Null);
        assert_eq!(value["layer_scope"], "video_tracks_only");

        let warnings = value["legacy_stage_warnings"].as_array().unwrap();
        assert_eq!(
            warnings.len(),
            1,
            "only the video clip's legacy stage is a colour warning: {warnings:?}"
        );
        assert_eq!(warnings[0]["effect_id"], 2);
        assert!(
            warnings.iter().all(|warning| warning["effect_id"] != 3),
            "an effect on an audio clip is outside the colour layer scope"
        );
    }

    #[test]
    fn an_existing_primary_node_is_corrected_in_place() {
        let mut document = document();
        document.tracks[0].clips[0].effects.push(Effect {
            id: EffectId(5),
            name: "primary_correction".to_owned(),
            parameters: BTreeMap::from([(
                "exposure_milli_stops".to_owned(),
                ParamValue::Integer(250),
            )]),
            keyframes: BTreeMap::new(),
        });
        let plan = plan_primary_correction(
            &document,
            TimelineRevision(0),
            &PrimaryCorrectionPlanArgs {
                expected_revision: TimelineRevision(0),
                clip_id: ClipId(1),
                profile_assumption: None,
                parameters: BTreeMap::from([("exposure_milli_stops".to_owned(), 900)]),
            },
        )
        .unwrap();
        assert_eq!(plan.existing_primary_node_count, 1);
        assert_eq!(plan.effect_id, EffectId(5));
        assert!(!plan.created_new_node);
        assert!(!plan.no_change);
        assert!(plan.warnings.is_empty());
        assert_eq!(plan.operations.len(), 1);
        assert!(matches!(
            plan.operations[0],
            Operation::SetEffectParam {
                effect: EffectId(5),
                ..
            }
        ));
        assert_eq!(plan.resolved_parameters["exposure_milli_stops"], 900);

        // Requesting the value the node already holds proposes nothing.
        let unchanged = plan_primary_correction(
            &document,
            TimelineRevision(0),
            &PrimaryCorrectionPlanArgs {
                expected_revision: TimelineRevision(0),
                clip_id: ClipId(1),
                profile_assumption: None,
                parameters: BTreeMap::from([("exposure_milli_stops".to_owned(), 250)]),
            },
        )
        .unwrap();
        assert!(unchanged.no_change);
        assert!(unchanged.operations.is_empty());
        // The targeted node still exists, so it is still the honest answer.
        assert_eq!(unchanged.target_effect_id(), Some(EffectId(5)));

        // Two primaries: target the last one and warn.
        document.tracks[0].clips[0].effects.push(Effect {
            id: EffectId(6),
            name: "primary_correction".to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::new(),
        });
        let ambiguous = plan_primary_correction(
            &document,
            TimelineRevision(0),
            &PrimaryCorrectionPlanArgs {
                expected_revision: TimelineRevision(0),
                clip_id: ClipId(1),
                profile_assumption: None,
                parameters: BTreeMap::from([("tint_percent".to_owned(), 4)]),
            },
        )
        .unwrap();
        assert_eq!(ambiguous.existing_primary_node_count, 2);
        assert_eq!(ambiguous.effect_id, EffectId(6));
        assert_eq!(ambiguous.warnings.len(), 1);
        assert!(ambiguous.warnings[0].contains("2 primary_correction nodes"));
    }

    /// A proposal that changes nothing and has no node to target never
    /// allocates an effect id, so it must not publish one.
    #[test]
    fn a_no_op_plan_on_an_ungraded_clip_publishes_no_target_effect_id() {
        let document = document();
        let plan = plan_primary_correction(
            &document,
            TimelineRevision(0),
            &PrimaryCorrectionPlanArgs {
                expected_revision: TimelineRevision(0),
                clip_id: ClipId(1),
                // The descriptor neutral: nothing moves.
                parameters: BTreeMap::from([("exposure_milli_stops".to_owned(), 0)]),
                profile_assumption: None,
            },
        )
        .unwrap();

        assert!(plan.no_change);
        assert!(!plan.created_new_node);
        assert_eq!(plan.existing_primary_node_count, 0);
        assert!(plan.operations.is_empty());
        assert_eq!(
            plan.target_effect_id(),
            None,
            "no operation allocates {:?}, so it must not be published",
            plan.effect_id
        );
    }

    /// Composing a proposal against an animated control is ambiguous. The plan
    /// still writes the static value, and says so.
    #[test]
    fn a_keyframed_target_parameter_is_reported_as_a_warning() {
        let mut document = document();
        document.tracks[0].clips[0].effects.push(Effect {
            id: EffectId(5),
            name: "primary_correction".to_owned(),
            parameters: BTreeMap::from([(
                "exposure_milli_stops".to_owned(),
                ParamValue::Integer(250),
            )]),
            keyframes: BTreeMap::from([(
                "exposure_milli_stops".to_owned(),
                kinewright_core::AutomationCurve {
                    keyframes: vec![kinewright_core::Keyframe {
                        at: kinewright_core::TimeCode::ZERO,
                        value: 250,
                        interpolation: kinewright_core::KeyframeInterpolation::default(),
                    }],
                },
            )]),
        });

        let plan = plan_primary_correction(
            &document,
            TimelineRevision(0),
            &PrimaryCorrectionPlanArgs {
                expected_revision: TimelineRevision(0),
                clip_id: ClipId(1),
                parameters: BTreeMap::from([("exposure_milli_stops".to_owned(), 900)]),
                profile_assumption: None,
            },
        )
        .unwrap();

        assert_eq!(plan.effect_id, EffectId(5));
        assert_eq!(plan.warnings.len(), 1);
        assert!(
            plan.warnings[0].contains("keyframes exposure_milli_stops")
                && plan.warnings[0].contains("static value"),
            "{:?}",
            plan.warnings
        );

        // A control the proposal does not touch raises nothing.
        let untouched = plan_primary_correction(
            &document,
            TimelineRevision(0),
            &PrimaryCorrectionPlanArgs {
                expected_revision: TimelineRevision(0),
                clip_id: ClipId(1),
                parameters: BTreeMap::from([("tint_percent".to_owned(), 4)]),
                profile_assumption: None,
            },
        )
        .unwrap();
        assert!(untouched.warnings.is_empty());
    }

    #[test]
    fn unknown_parameter_errors_list_every_allowed_control() {
        let error = plan_primary_correction(
            &document(),
            TimelineRevision(0),
            &PrimaryCorrectionPlanArgs {
                expected_revision: TimelineRevision(0),
                clip_id: ClipId(1),
                profile_assumption: None,
                parameters: BTreeMap::from([("gamma".to_owned(), 1)]),
            },
        )
        .unwrap_err();
        assert_eq!(error.code(), "unknown_primary_parameter");
        let allowed = error.details()["allowed_parameters"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(allowed.len(), 10);
        assert!(
            allowed
                .iter()
                .any(|entry| entry["name"] == "exposure_milli_stops"
                    && entry["min"] == -5_000
                    && entry["max"] == 5_000
                    && entry["neutral"] == 0)
        );
        let summary = primary_parameter_summary();
        assert!(summary.contains("exposure_milli_stops=-5000..=5000, neutral 0"));
        assert!(summary.contains("contrast_pivot_basis_points=0..=10000, neutral 5000"));
    }

    // -----------------------------------------------------------------------
    // CC3 §8 — plan_color_wheels
    // -----------------------------------------------------------------------

    fn wheels_args(parameters: BTreeMap<String, i64>) -> ColorWheelsPlanArgs {
        ColorWheelsPlanArgs {
            expected_revision: TimelineRevision(0),
            clip_id: ClipId(1),
            profile_assumption: None,
            parameters,
            append: false,
        }
    }

    fn curves_args(curves: ColorCurvesRequest) -> ColorCurvesPlanArgs {
        ColorCurvesPlanArgs {
            expected_revision: TimelineRevision(0),
            clip_id: ClipId(1),
            profile_assumption: None,
            curves,
            bypass: None,
            append: false,
        }
    }

    fn wheels_node(id: u64, parameters: BTreeMap<String, ParamValue>) -> Effect {
        Effect {
            id: EffectId(id),
            name: "color_wheels".to_owned(),
            parameters,
            keyframes: BTreeMap::new(),
        }
    }

    fn curves_node(id: u64, parameters: BTreeMap<String, ParamValue>) -> Effect {
        Effect {
            id: EffectId(id),
            name: "color_curves".to_owned(),
            parameters,
            keyframes: BTreeMap::new(),
        }
    }

    fn integers<const N: usize>(entries: [(&str, i64); N]) -> BTreeMap<String, ParamValue> {
        entries
            .into_iter()
            .map(|(name, value)| (name.to_owned(), ParamValue::Integer(value)))
            .collect()
    }

    /// CC3 §10.3 fixture 11: the planner is bound to the analyzed revision,
    /// fails closed on a stale one, and never touches the document.
    #[test]
    fn color_wheels_plan_is_revision_bound_and_never_applies() {
        let document = document();
        let args = ColorWheelsPlanArgs {
            expected_revision: TimelineRevision(7),
            ..wheels_args(BTreeMap::from([("gain_red_thousandths".to_owned(), 1_200)]))
        };
        let plan =
            plan_color_wheels(&document, TimelineRevision(7), &args).expect("valid wheels plan");
        assert_eq!(plan.expected_revision, TimelineRevision(7));
        assert!(!plan.no_change);
        assert!(document.clip(ClipId(1)).unwrap().effects.is_empty());

        let stale = plan_color_wheels(&document, TimelineRevision(8), &args).unwrap_err();
        assert_eq!(stale.code(), "revision_conflict");
        assert_eq!(stale.details()["observed"], 7);
        assert_eq!(stale.details()["allowed"], 8);
        assert!(document.clip(ClipId(1)).unwrap().effects.is_empty());
    }

    /// A new node stores only the controls the caller moved: CC3 §4.1 resolves
    /// every omitted parameter to its neutral, so writing the other twelve
    /// would be noise in project JSON and in the reviewed plan.
    #[test]
    fn color_wheels_plan_emits_one_exact_add_effect_for_a_new_node() {
        let document = document();
        let plan = plan_color_wheels(
            &document,
            TimelineRevision(0),
            &wheels_args(BTreeMap::from([
                ("gain_red_thousandths".to_owned(), 1_200),
                ("lift_master_basis_points".to_owned(), -500),
                // A control requested at its neutral moves nothing.
                ("gamma_blue_thousandths".to_owned(), 1_000),
            ])),
        )
        .expect("valid wheels plan");

        assert_eq!(
            plan.operations,
            vec![Operation::AddEffect {
                clip: ClipId(1),
                effect: Effect {
                    id: EffectId(1),
                    name: "color_wheels".to_owned(),
                    parameters: integers([
                        ("gain_red_thousandths", 1_200),
                        ("lift_master_basis_points", -500),
                    ]),
                    keyframes: BTreeMap::new(),
                },
            }]
        );
        assert!(plan.created_new_node);
        assert!(!plan.targets_existing_node);
        assert_eq!(plan.target_effect_id(), Some(EffectId(1)));
        assert_eq!(plan.existing_color_node_count, 0);
        assert_eq!(plan.existing_nodes_of_kind, 0);
        // Thirteen resolved controls: twelve wheels plus bypass.
        assert_eq!(plan.resolved_parameters.len(), 13);
        assert_eq!(plan.resolved_parameters["gain_red_thousandths"], 1_200);
        assert_eq!(plan.resolved_parameters["gain_blue_thousandths"], 1_000);
        assert_eq!(plan.resolved_parameters["lift_master_basis_points"], -500);
        assert_eq!(plan.resolved_parameters["lift_green_basis_points"], 0);
        assert_eq!(plan.resolved_parameters["bypass"], 0);
    }

    /// CC2's review rule: an existing node of the requested kind is corrected
    /// in place, and `append` is the explicit opt-out.
    #[test]
    fn color_wheels_plan_targets_an_existing_node_unless_append() {
        let mut document = document();
        document.tracks[0].clips[0].effects.push(wheels_node(
            5,
            integers([("gain_red_thousandths", 1_200), ("bypass", 1)]),
        ));

        let plan = plan_color_wheels(
            &document,
            TimelineRevision(0),
            &wheels_args(BTreeMap::from([
                ("gain_red_thousandths".to_owned(), 1_300),
                ("gamma_master_thousandths".to_owned(), 900),
                ("bypass".to_owned(), 0),
            ])),
        )
        .expect("valid wheels plan");

        assert_eq!(
            plan.operations,
            vec![
                Operation::SetEffectParam {
                    clip: ClipId(1),
                    effect: EffectId(5),
                    name: "bypass".to_owned(),
                    value: ParamValue::Integer(0),
                },
                Operation::SetEffectParam {
                    clip: ClipId(1),
                    effect: EffectId(5),
                    name: "gain_red_thousandths".to_owned(),
                    value: ParamValue::Integer(1_300),
                },
                Operation::SetEffectParam {
                    clip: ClipId(1),
                    effect: EffectId(5),
                    name: "gamma_master_thousandths".to_owned(),
                    value: ParamValue::Integer(900),
                },
            ]
        );
        assert!(!plan.created_new_node);
        assert!(plan.targets_existing_node);
        assert_eq!(plan.effect_id, EffectId(5));
        assert_eq!(plan.existing_nodes_of_kind, 1);
        assert_eq!(plan.existing_color_node_count, 1);

        let appended = plan_color_wheels(
            &document,
            TimelineRevision(0),
            &ColorWheelsPlanArgs {
                append: true,
                ..wheels_args(BTreeMap::from([("gain_red_thousandths".to_owned(), 1_300)]))
            },
        )
        .expect("valid appended wheels plan");
        assert!(appended.created_new_node);
        assert!(!appended.targets_existing_node);
        assert_eq!(appended.effect_id, EffectId(6));
        assert_eq!(
            appended.operations,
            vec![Operation::AddEffect {
                clip: ClipId(1),
                effect: wheels_node(6, integers([("gain_red_thousandths", 1_300)])),
            }]
        );
        assert_eq!(appended.existing_nodes_of_kind, 1);
        assert!(
            appended
                .assumptions
                .iter()
                .any(|entry| entry.contains("append=true")),
            "{:?}",
            appended.assumptions
        );

        // Requesting the value the node already holds proposes nothing, and
        // the targeted node is still the honest answer.
        let unchanged = plan_color_wheels(
            &document,
            TimelineRevision(0),
            &wheels_args(BTreeMap::from([("gain_red_thousandths".to_owned(), 1_200)])),
        )
        .unwrap();
        assert!(unchanged.no_change);
        assert!(unchanged.operations.is_empty());
        assert_eq!(unchanged.target_effect_id(), Some(EffectId(5)));
    }

    /// A no-op proposal on an ungraded clip never allocates an effect id, so
    /// it must not publish one.
    #[test]
    fn a_no_op_color_wheels_plan_publishes_no_target_effect_id() {
        let plan = plan_color_wheels(
            &document(),
            TimelineRevision(0),
            &wheels_args(BTreeMap::from([(
                "gamma_master_thousandths".to_owned(),
                1_000,
            )])),
        )
        .unwrap();
        assert!(plan.no_change);
        assert!(plan.operations.is_empty());
        assert_eq!(plan.target_effect_id(), None);
    }

    #[test]
    fn color_wheels_plan_rejects_unknown_and_out_of_range_controls() {
        let document = document();
        let unknown = plan_color_wheels(
            &document,
            TimelineRevision(0),
            &wheels_args(BTreeMap::from([("gain_red_percent".to_owned(), 120)])),
        )
        .unwrap_err();
        assert_eq!(unknown.code(), "unknown_color_node_parameter");
        let details = unknown.details();
        assert_eq!(details["field"], "parameters");
        assert_eq!(details["observed"], "gain_red_percent");
        let allowed = details["allowed"].as_array().unwrap();
        assert_eq!(allowed.len(), 13);
        assert!(
            allowed
                .iter()
                .any(|entry| entry["name"] == "gain_red_thousandths"
                    && entry["min"] == 0
                    && entry["max"] == 4_000
                    && entry["neutral"] == 1_000)
        );
        assert!(
            details["recovery_action"]
                .as_str()
                .unwrap()
                .contains("color_wheels")
        );

        let out_of_range = plan_color_wheels(
            &document,
            TimelineRevision(0),
            &wheels_args(BTreeMap::from([(
                "lift_master_basis_points".to_owned(),
                -2_001,
            )])),
        )
        .unwrap_err();
        assert_eq!(out_of_range.code(), "color_node_parameter_out_of_range");
        let details = out_of_range.details();
        assert_eq!(details["field"], "lift_master_basis_points");
        assert_eq!(details["observed"], -2_001);
        assert_eq!(details["allowed"], json!({"min": -2_000, "max": 2_000}));
    }

    /// Composing a proposal against an animated control is ambiguous. The plan
    /// still writes the static value, and says so.
    #[test]
    fn a_keyframed_color_wheels_target_is_reported_as_a_warning() {
        let mut document = document();
        let mut node = wheels_node(5, integers([("gain_red_thousandths", 1_200)]));
        node.keyframes.insert(
            "gain_red_thousandths".to_owned(),
            kinewright_core::AutomationCurve {
                keyframes: vec![kinewright_core::Keyframe {
                    at: kinewright_core::TimeCode::ZERO,
                    value: 1_200,
                    interpolation: kinewright_core::KeyframeInterpolation::Hold,
                }],
            },
        );
        document.tracks[0].clips[0].effects.push(node);

        let plan = plan_color_wheels(
            &document,
            TimelineRevision(0),
            &wheels_args(BTreeMap::from([("gain_red_thousandths".to_owned(), 1_400)])),
        )
        .unwrap();
        assert_eq!(plan.warnings.len(), 1);
        assert!(
            plan.warnings[0].contains("keyframes gain_red_thousandths")
                && plan.warnings[0].contains("static value"),
            "{:?}",
            plan.warnings
        );

        // A control the proposal does not touch raises nothing.
        let untouched = plan_color_wheels(
            &document,
            TimelineRevision(0),
            &wheels_args(BTreeMap::from([(
                "gain_blue_thousandths".to_owned(),
                1_100,
            )])),
        )
        .unwrap();
        assert!(untouched.warnings.is_empty());
    }

    // -----------------------------------------------------------------------
    // CC3 §8 — plan_color_curves
    // -----------------------------------------------------------------------

    #[test]
    fn color_curves_plan_expands_a_three_point_master_curve_and_never_applies() {
        let document = document();
        let plan = plan_color_curves(
            &document,
            TimelineRevision(0),
            &curves_args(ColorCurvesRequest {
                master: Some(vec![[0, 0], [5_000, 6_000], [10_000, 10_000]]),
                ..ColorCurvesRequest::default()
            }),
        )
        .expect("valid curves plan");

        assert_eq!(
            plan.requested_parameters,
            BTreeMap::from([
                ("master_point_count".to_owned(), 3),
                ("master_x0".to_owned(), 0),
                ("master_y0".to_owned(), 0),
                ("master_x1".to_owned(), 5_000),
                ("master_y1".to_owned(), 6_000),
                ("master_x2".to_owned(), 10_000),
                ("master_y2".to_owned(), 10_000),
            ])
        );
        // CC3 §2.4: only the parameters that move off their neutrals are
        // stored. `master_x0`/`master_y0` are neutral at 0 and point 2 lands
        // exactly on the neutral (10000, 10000).
        assert_eq!(
            plan.operations,
            vec![Operation::AddEffect {
                clip: ClipId(1),
                effect: curves_node(
                    1,
                    integers([
                        ("master_point_count", 3),
                        ("master_x1", 5_000),
                        ("master_y1", 6_000),
                    ]),
                ),
            }]
        );
        assert_eq!(
            plan.resolved_curves["master"],
            vec![[0, 0], [5_000, 6_000], [10_000, 10_000]]
        );
        for curve in ["red", "green", "blue"] {
            assert_eq!(
                plan.resolved_curves[curve],
                vec![[0, 0], [10_000, 10_000]],
                "{curve} stays at the structural identity"
            );
        }
        assert_eq!(plan.resolved_parameters["red_point_count"], 2);
        assert_eq!(plan.resolved_parameters["bypass"], 0);
        assert!(
            plan.assumptions
                .iter()
                .any(|entry| entry.contains("structural identity")),
            "{:?}",
            plan.assumptions
        );
        assert!(document.clip(ClipId(1)).unwrap().effects.is_empty());

        let stale = plan_color_curves(
            &document,
            TimelineRevision(3),
            &curves_args(ColorCurvesRequest {
                master: Some(vec![[0, 0], [5_000, 6_000], [10_000, 10_000]]),
                ..ColorCurvesRequest::default()
            }),
        )
        .unwrap_err();
        assert_eq!(stale.code(), "revision_conflict");
    }

    #[test]
    fn color_curves_plan_reports_count_bounds_and_ordering_violations() {
        let document = document();
        let plan = |curves| plan_color_curves(&document, TimelineRevision(0), &curves_args(curves));

        let seventeen = (0..17)
            .map(|index| [i64::from(index) * 500, 0])
            .collect::<Vec<_>>();
        let error = plan(ColorCurvesRequest {
            master: Some(seventeen),
            ..ColorCurvesRequest::default()
        })
        .unwrap_err();
        assert_eq!(error.code(), "invalid_curve_point_count");
        let details = error.details();
        assert_eq!(details["field"], "curves.master");
        assert_eq!(details["observed"], 17);
        assert_eq!(details["allowed"], json!({"min": 2, "max": 16}));

        let error = plan(ColorCurvesRequest {
            red: Some(vec![[0, 0], [5_000, 5_000], [5_000, 9_000]]),
            ..ColorCurvesRequest::default()
        })
        .unwrap_err();
        assert_eq!(error.code(), "invalid_curve_points");
        let details = error.details();
        assert_eq!(details["field"], "curves.red[2].x");
        assert_eq!(details["observed"], 5_000);
        assert_eq!(details["previous_x"], 5_000);
        assert_eq!(details["index"], 2);
        assert_eq!(details["curve"], "red");
        assert_eq!(
            details["allowed"],
            "strictly greater than curves.red[1].x = 5000"
        );

        let error = plan(ColorCurvesRequest {
            blue: Some(vec![[0, 0], [10_000, 12_001]]),
            ..ColorCurvesRequest::default()
        })
        .unwrap_err();
        assert_eq!(error.code(), "curve_coordinate_out_of_range");
        let details = error.details();
        assert_eq!(details["field"], "curves.blue[1].y");
        assert_eq!(details["observed"], 12_001);
        assert_eq!(details["allowed"], json!({"min": -2_000, "max": 12_000}));
        assert_eq!(details["axis"], "y");
    }

    /// Editing an existing node in place must never produce an intermediate
    /// parameter map Core rejects, and curves the caller did not name keep
    /// their points.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn color_curves_plan_edits_an_existing_node_in_a_core_valid_order() {
        let mut document = document();
        document.tracks[0].clips[0].effects.push(curves_node(
            9,
            integers([
                ("master_point_count", 4),
                ("master_x1", 3_000),
                ("master_y1", 3_000),
                ("master_x2", 6_000),
                ("master_y2", 6_000),
                ("master_x3", 10_000),
                ("master_y3", 10_000),
                ("red_point_count", 3),
                ("red_x1", 4_000),
                ("red_y1", 4_000),
            ]),
        ));

        // Shrinking from four points to two: the active prefix is collapsed
        // first so no single SetEffectParam ever sees a crossing x.
        let plan = plan_color_curves(
            &document,
            TimelineRevision(0),
            &curves_args(ColorCurvesRequest {
                master: Some(vec![[2_000, 1_000], [10_000, 10_000]]),
                ..ColorCurvesRequest::default()
            }),
        )
        .expect("valid curves plan");
        let set = |name: &str, value: i64| Operation::SetEffectParam {
            clip: ClipId(1),
            effect: EffectId(9),
            name: name.to_owned(),
            value: ParamValue::Integer(value),
        };
        assert_eq!(
            plan.operations,
            vec![
                set("master_point_count", 2),
                set("master_x0", 2_000),
                set("master_y0", 1_000),
                set("master_x1", 10_000),
                set("master_y1", 10_000),
            ]
        );
        assert!(!plan.created_new_node);
        assert!(plan.targets_existing_node);
        assert_eq!(plan.effect_id, EffectId(9));
        // An unnamed curve keeps whatever the node already stores.
        assert_eq!(
            plan.resolved_curves["red"],
            vec![[0, 0], [4_000, 4_000], [10_000, 10_000]]
        );
        assert!(
            plan.assumptions
                .iter()
                .any(|entry| entry.contains("keeps its current points")),
            "{:?}",
            plan.assumptions
        );

        // Moving point 0 past the stored point 1 must write point 1 first.
        document.tracks[0].clips[0].effects = vec![curves_node(
            9,
            integers([("master_x1", 4_000), ("master_y1", 4_000)]),
        )];
        let reordered = plan_color_curves(
            &document,
            TimelineRevision(0),
            &curves_args(ColorCurvesRequest {
                master: Some(vec![[5_000, 2_000], [9_000, 9_000]]),
                ..ColorCurvesRequest::default()
            }),
        )
        .expect("valid curves plan");
        assert_eq!(
            reordered.operations,
            vec![
                set("master_x1", 9_000),
                set("master_y1", 9_000),
                set("master_x0", 5_000),
                set("master_y0", 2_000),
            ],
            "writing master_x0=5000 while master_x1 is still 4000 would be rejected"
        );

        // append stacks a second node instead.
        let appended = plan_color_curves(
            &document,
            TimelineRevision(0),
            &ColorCurvesPlanArgs {
                append: true,
                ..curves_args(ColorCurvesRequest {
                    master: Some(vec![[5_000, 2_000], [9_000, 9_000]]),
                    ..ColorCurvesRequest::default()
                })
            },
        )
        .expect("valid appended curves plan");
        assert!(appended.created_new_node);
        assert_eq!(appended.effect_id, EffectId(10));
        assert_eq!(
            appended.operations,
            vec![Operation::AddEffect {
                clip: ClipId(1),
                effect: curves_node(
                    10,
                    integers([
                        ("master_x0", 5_000),
                        ("master_y0", 2_000),
                        ("master_x1", 9_000),
                        ("master_y1", 9_000),
                    ]),
                ),
            }]
        );
    }

    #[test]
    fn a_keyframed_color_curves_target_is_reported_as_a_warning() {
        let mut document = document();
        let mut node = curves_node(9, integers([("master_x1", 4_000), ("master_y1", 4_000)]));
        node.keyframes.insert(
            "master_y1".to_owned(),
            kinewright_core::AutomationCurve {
                keyframes: vec![kinewright_core::Keyframe {
                    at: kinewright_core::TimeCode::ZERO,
                    value: 4_000,
                    interpolation: kinewright_core::KeyframeInterpolation::Linear,
                }],
            },
        );
        document.tracks[0].clips[0].effects.push(node);

        let plan = plan_color_curves(
            &document,
            TimelineRevision(0),
            &curves_args(ColorCurvesRequest {
                master: Some(vec![[0, 0], [4_000, 7_000], [10_000, 10_000]]),
                ..ColorCurvesRequest::default()
            }),
        )
        .unwrap();
        assert_eq!(plan.warnings.len(), 1, "{:?}", plan.warnings);
        assert!(
            plan.warnings[0].contains("keyframes master_y1")
                && plan.warnings[0].contains("static value"),
            "{:?}",
            plan.warnings
        );
    }

    // -----------------------------------------------------------------------
    // CC3 §8 — the ordered `color_nodes` manifest
    // -----------------------------------------------------------------------

    #[test]
    fn color_nodes_report_ordered_stages_with_bypass_and_inactive_reasons() {
        let mut document = document();
        document.tracks[0].clips[0].effects = vec![
            Effect {
                id: EffectId(1),
                name: "primary_correction".to_owned(),
                parameters: integers([("exposure_milli_stops", 250)]),
                keyframes: BTreeMap::new(),
            },
            wheels_node(
                2,
                integers([("lift_master_basis_points", -300), ("bypass", 1)]),
            ),
            curves_node(3, integers([("master_x1", 5_000), ("master_y1", 6_000)])),
            // A neutral node is inactive for a different, reported reason.
            wheels_node(4, BTreeMap::new()),
            Effect {
                id: EffectId(5),
                name: "look_lut".to_owned(),
                parameters: BTreeMap::new(),
                keyframes: BTreeMap::new(),
            },
        ];

        let value = color_context_value(TimelineRevision(0), &document);
        let nodes = value["clips"][0]["color_nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 4, "look_lut is not a managed colour node");

        assert_eq!(nodes[0]["stage_index"], 0);
        assert_eq!(nodes[0]["kind"], "primary_correction");
        assert_eq!(nodes[0]["active"], true);
        assert_eq!(nodes[0]["inactive_reason"], Value::Null);
        assert_eq!(nodes[0]["supports_bypass"], false);
        assert_eq!(nodes[0]["parameters"]["exposure_milli_stops"], 250);
        assert_eq!(nodes[0]["parameters"]["contrast_pivot_basis_points"], 5_000);

        assert_eq!(nodes[1]["stage_index"], 1);
        assert_eq!(nodes[1]["kind"], "color_wheels");
        assert_eq!(nodes[1]["bypass"], 1);
        assert_eq!(nodes[1]["active"], false);
        assert_eq!(nodes[1]["inactive_reason"], "bypassed");
        assert_eq!(nodes[1]["parameters"].as_object().unwrap().len(), 13);
        assert_eq!(nodes[1]["parameters"]["lift_master_basis_points"], -300);
        assert_eq!(nodes[1]["parameters"]["gain_blue_thousandths"], 1_000);

        assert_eq!(nodes[2]["stage_index"], 2);
        assert_eq!(nodes[2]["kind"], "color_curves");
        assert_eq!(nodes[2]["bypass"], 0);
        assert_eq!(nodes[2]["active"], true);
        assert_eq!(nodes[2]["inactive_reason"], Value::Null);
        assert_eq!(
            nodes[2]["curves"]["master"]["points"],
            json!([[0, 0], [5_000, 6_000]])
        );
        assert_eq!(nodes[2]["curves"]["master"]["declared_point_count"], 2);
        assert_eq!(nodes[2]["curves"]["master"]["truncated"], false);
        assert_eq!(nodes[2]["curves"]["red"]["structural_identity"], true);
        assert_eq!(nodes[2]["parameters"]["master_x1"], 5_000);
        assert_eq!(nodes[2]["warnings"], json!([]));

        assert_eq!(nodes[3]["stage_index"], 3);
        assert_eq!(nodes[3]["bypass"], 0);
        assert_eq!(nodes[3]["active"], false);
        assert_eq!(nodes[3]["inactive_reason"], "neutral");
    }

    /// CC3 §3.4: automation that leaves a point list without strictly
    /// increasing x truncates the curve and is reported, never rendered as an
    /// error.
    #[test]
    fn a_curve_truncated_by_automation_is_reported_on_its_node() {
        let mut document = document();
        document.tracks[0].clips[0].effects = vec![curves_node(
            3,
            integers([
                ("master_point_count", 3),
                ("master_x1", 5_000),
                ("master_y1", 6_000),
                ("master_x2", 4_000),
                ("master_y2", 9_000),
            ]),
        )];

        let value = color_context_value(TimelineRevision(0), &document);
        let node = &value["clips"][0]["color_nodes"][0];
        assert_eq!(node["curves"]["master"]["truncated"], true);
        assert_eq!(node["curves"]["master"]["declared_point_count"], 3);
        assert_eq!(node["curves"]["master"]["point_count"], 2);
        assert_eq!(
            node["curves"]["master"]["points"],
            json!([[0, 0], [5_000, 6_000]])
        );
        let warnings = node["warnings"].as_array().unwrap();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0]["code"], "curve_truncated_by_automation");
        assert_eq!(warnings[0]["curves"], json!(["master"]));
        assert_eq!(warnings[0]["effect_id"], 3);
        assert_eq!(warnings[0]["stage_index"], 0);
    }

    /// A bypassed node renders as the exact identity (CC3 §5), so its
    /// truncation changes no pixel. Core QA and the inspector both suppress the
    /// warning there, and the agent surface has to agree: three descriptions of
    /// the same node that disagree are worse than none.
    ///
    /// `color_node_value` is handed frame-evaluated effects by
    /// `render_color_proof`, which is how a declared sixteen-point curve with
    /// omitted coordinates reaches it while the stored document stays legal.
    #[test]
    fn a_bypassed_node_reports_no_truncation_warning() {
        let mut document = document();
        document.tracks[0].clips[0].effects = vec![curves_node(
            3,
            integers([("master_point_count", 16), (COLOR_NODE_BYPASS_PARAMETER, 1)]),
        )];

        let value = color_context_value(TimelineRevision(0), &document);
        let node = &value["clips"][0]["color_nodes"][0];
        assert_eq!(node["bypass"], 1);
        assert_eq!(node["inactive_reason"], "bypassed");
        assert_eq!(
            node["curves"]["master"]["truncated"], true,
            "the resolved shape is still described faithfully",
        );
        assert_eq!(
            node["warnings"].as_array().map(Vec::len),
            Some(0),
            "a bypassed node is the exact identity and warns about nothing",
        );

        // Releasing the bypass restores the warning.
        document.tracks[0].clips[0].effects[0].parameters.insert(
            COLOR_NODE_BYPASS_PARAMETER.to_owned(),
            ParamValue::Integer(0),
        );
        let value = color_context_value(TimelineRevision(0), &document);
        let warnings = value["clips"][0]["color_nodes"][0]["warnings"]
            .as_array()
            .expect("warnings are an array")
            .clone();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0]["code"], "curve_truncated_by_automation");
    }

    /// The planner's "no node yet" baseline is the structural identity core
    /// defines, not a second copy of its coordinates.
    #[test]
    fn the_absent_node_baseline_is_the_core_structural_identity() {
        let identity: Vec<[i64; 2]> = CurvePoints::identity()
            .points
            .into_iter()
            .map(|(x, y)| [i64::from(x), i64::from(y)])
            .collect();
        for curve in ColorCurveChannel::ALL {
            assert_eq!(stored_curve_points(None, curve), identity);
        }
    }
}
