//! Agent-facing CC1 colour observability and evidence-only planning.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use kinewright_core::{
    AssetId, Clip, ClipContent, ClipId, ColorDescription, ColorSourceError, ColorSourceProfile,
    ColorSourceProfileAssumption, ColorWhitePoint, Document, Effect, EffectId,
    MediaAvailabilityStatus, MediaError, MediaKind, Operation, ParamValue, TimeCode,
    TimelineRevision, TrackKind, apply_batch, classify_source_with_assumption, effect_descriptor,
};
use serde_json::{Value, json};
use thiserror::Error;

const PRIMARY_CORRECTION_EFFECT_NAME: &str = "primary_correction";

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
    pub effect_id: EffectId,
    pub source_profile: ColorSourceProfile,
    pub profile_assumption: Option<ColorSourceProfileAssumption>,
    pub requested_parameters: BTreeMap<String, i64>,
    pub resolved_parameters: BTreeMap<String, i64>,
    pub operations: Vec<Operation>,
    pub existing_primary_node_count: usize,
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
            Self::UnknownParameter { name } => json!({"parameter": name}),
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
                        },
                    },
                },
            })
        })
        .collect::<Vec<_>>();

    let clips = document
        .tracks
        .iter()
        .flat_map(|track| {
            track
                .clips
                .iter()
                .map(move |clip| clip_status(track.id.0, track.kind, clip))
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
#[must_use]
pub(crate) fn legacy_stage_warnings_for_document(document: &Document) -> Vec<Value> {
    document
        .tracks
        .iter()
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
                "selected": if profile_assumption.is_some()
                    && description.white_point == ColorWhitePoint::Unknown
                {
                    json!("d65")
                } else {
                    Value::Null
                },
                "source": assumption_source.unwrap_or("metadata"),
                "required": false,
                "available": ["d65"],
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
                    "available": ["d65"],
                },
                "blocking_reason": blocking_reason(&error),
            })
        }
    }
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

fn clip_status(track_id: u64, track_kind: TrackKind, clip: &kinewright_core::Clip) -> Value {
    let primary_nodes = clip
        .effects
        .iter()
        .enumerate()
        .filter(|(_, effect)| effect.name == PRIMARY_CORRECTION_EFFECT_NAME)
        .map(|(effect_index, effect)| primary_node(effect_index, effect))
        .collect::<Vec<_>>();
    let legacy_stage_warnings = legacy_stage_warnings(clip);
    json!({
        "track_id": track_id,
        "track_kind": track_kind,
        "clip_id": clip.id.0,
        "asset_id": clip.asset.0,
        "content": clip_content_name(&clip.content),
        "primary_nodes": primary_nodes,
        "legacy_stage_warnings": legacy_stage_warnings,
    })
}

fn clip_content_name(content: &ClipContent) -> &'static str {
    match content {
        ClipContent::Media => "media",
        ClipContent::Title(_) => "title",
        ClipContent::Freeze(_) => "freeze",
    }
}

fn primary_node(effect_index: usize, effect: &Effect) -> Value {
    let parameters = effect_descriptor(PRIMARY_CORRECTION_EFFECT_NAME)
        .map(|descriptor| {
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
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    json!({
        "effect_id": effect.id.0,
        "effect_index": effect_index,
        "name": effect.name,
        "parameters": parameters,
        "keyframes": effect.keyframes,
    })
}

fn legacy_warning(effect_index: usize, effect: &Effect) -> Option<Value> {
    let (code, message) = match effect.name.as_str() {
        "brightness" | "contrast" | "saturation" => (
            "legacy_display_effect",
            "legacy display-coded colour semantics are outside CC1 managed conformance",
        ),
        "color_grade" => (
            "legacy_color_grade",
            "legacy color_grade must be migrated or explicitly retained as a compatibility stage",
        ),
        "look_lut" | "cube_lut" => (
            "legacy_lut_stage",
            "legacy LUT stage is post-primary and outside CC1 managed conformance",
        ),
        _ => return None,
    };
    Some(json!({
        "code": code,
        "effect_id": effect.id.0,
        "effect_index": effect_index,
        "name": effect.name,
        "message": message,
        "stage": "post_primary_legacy",
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

    let effect_id = next_effect_id(document)?;
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
    let mut resolved_parameters = neutral_parameters
        .iter()
        .map(|(name, value)| {
            let ParamValue::Integer(value) = value else {
                unreachable!("primary descriptor neutrals are integers")
            };
            (name.clone(), *value)
        })
        .collect::<BTreeMap<_, _>>();
    for (name, value) in &args.parameters {
        resolved_parameters.insert(name.clone(), *value);
    }
    let effect = Effect {
        id: effect_id,
        name: PRIMARY_CORRECTION_EFFECT_NAME.to_owned(),
        parameters: neutral_parameters,
        keyframes: BTreeMap::new(),
    };
    let mut operations = vec![Operation::AddEffect {
        clip: args.clip_id,
        effect,
    }];
    operations.extend(
        args.parameters
            .iter()
            .map(|(name, value)| Operation::SetEffectParam {
                clip: args.clip_id,
                effect: effect_id,
                name: name.clone(),
                value: ParamValue::Integer(*value),
            }),
    );

    let mut candidate = document.clone();
    apply_batch(&mut candidate, &operations)
        .map_err(|error| PrimaryPlanError::CoreRejected(error.to_string()))?;
    Ok(PrimaryCorrectionPlan {
        expected_revision: args.expected_revision,
        clip_id: args.clip_id,
        effect_id,
        source_profile,
        profile_assumption,
        requested_parameters: args.parameters.clone(),
        resolved_parameters,
        operations,
        existing_primary_node_count: clip
            .effects
            .iter()
            .filter(|effect| effect.name == PRIMARY_CORRECTION_EFFECT_NAME)
            .count(),
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
}
