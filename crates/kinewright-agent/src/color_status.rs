//! Agent-facing CC1 colour observability and evidence-only planning.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use kinewright_core::{
    AssetId, Clip, ClipContent, ClipId, ColorDescription, ColorSourceError, ColorSourceProfile,
    ColorSourceProfileAssumption, ColorWhitePoint, Document, Effect, EffectCompatibilityStage,
    EffectId, MediaAvailabilityStatus, MediaError, MediaKind, Operation, ParamValue, TimeCode,
    TimelineRevision, TrackKind, apply_batch, classify_source_with_assumption,
    effect_compatibility_stage, effect_descriptor,
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
        "primary_nodes": primary_node_manifest(&clip.effects),
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
                "keyframes": effect.keyframes,
                "compatibility_stage": effect_compatibility_stage(&effect.name)
                    .map_or(Value::Null, |stage| json!(stage.issue_code())),
            })
        })
        .collect()
}

/// Keep the primary-only view derived from the same ordered chain above.
#[must_use]
pub(crate) fn primary_node_manifest(effects: &[Effect]) -> Vec<Value> {
    effect_chain_manifest(effects)
        .into_iter()
        .filter(|effect| !effect["primary_parameters"].is_null())
        .map(|effect| {
            json!({
                "effect_id": effect["effect_id"],
                "effect_index": effect["effect_index"],
                "name": effect["name"],
                "parameters": effect["primary_parameters"],
                "keyframes": effect["keyframes"],
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
    let Some(track) = document
        .tracks
        .iter()
        .find(|track| track.clips.iter().any(|clip| clip.id == args.clip_id))
    else {
        return Err(PrimaryPlanError::MissingClip(args.clip_id));
    };
    let Some(clip) = track.clips.iter().find(|clip| clip.id == args.clip_id) else {
        return Err(PrimaryPlanError::MissingClip(args.clip_id));
    };
    if track.kind != TrackKind::Video
        || !matches!(clip.content, ClipContent::Media | ClipContent::Freeze(_))
    {
        return Err(PrimaryPlanError::WrongClipType {
            clip: clip.id,
            track: track.kind,
            content: clip_content_name(&clip.content),
        });
    }
    let Some(asset) = document.asset(clip.asset) else {
        return Err(PrimaryPlanError::MissingAsset {
            clip: clip.id,
            asset: clip.asset,
        });
    };
    if !matches!(asset.kind, MediaKind::Video | MediaKind::AudioVideo) {
        return Err(PrimaryPlanError::WrongAssetKind {
            clip: clip.id,
            asset: asset.id,
            kind: asset.kind,
        });
    }

    let (source_profile, profile_assumption) =
        managed_source_profile(&asset.color_description, args.profile_assumption).map_err(
            |error| PrimaryPlanError::UnsupportedSource {
                clip: clip.id,
                error,
            },
        )?;

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
            next_effect_id(document)?,
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

fn next_effect_id(document: &Document) -> Result<EffectId, PrimaryPlanError> {
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
        .ok_or(PrimaryPlanError::EffectIdExhausted)
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
            value["clips"][0]["primary_nodes"].as_array().unwrap().len(),
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
        assert_eq!(effects[1]["name"], "look_lut");
        assert_eq!(effects[1]["compatibility_stage"], "legacy_lut_stage");
        assert_eq!(clips[0]["primary_nodes"].as_array().unwrap().len(), 1);
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
}
