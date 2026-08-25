//! Agent-facing CC1 colour observability and evidence-only planning.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
};

use kinewright_core::{
    AssetId, COLOR_CURVE_COORDINATE_MAX, COLOR_CURVE_COORDINATE_MIN, COLOR_CURVE_MAX_POINTS,
    COLOR_CURVE_MIN_POINTS, COLOR_CURVE_WHITE_BASIS_POINTS, COLOR_NODE_BYPASS_PARAMETER,
    COLOR_NODE_LIMIT_PER_LAYER, Clip, ClipContent, ClipId, ColorCurveChannel, ColorDescription,
    ColorNodeInactiveReason, ColorNodeKind, ColorSourceError, ColorSourceProfile,
    ColorSourceProfileAssumption, ColorStage, ColorWheelChannel, ColorWheelControl,
    ColorWheelsParams, ColorWhitePoint, CurvePoints, Document, Effect, EffectCompatibilityStage,
    EffectId, LUT_ASSET_ID_PARAMETER, LUT_INPUT_ENCODING_PARAMETER, LUT_MIX_BASIS_POINTS_MAX,
    LUT_MIX_PARAMETER, LUT_NODE_LIMIT_PER_LAYER, LutAsset, LutAssetId, LutAssetSource,
    LutAvailabilityKind, LutAvailabilityStatus, LutNodeParams, MANAGED_COLOR_NODE_NAMES,
    MATTE_MIX_BASIS_POINTS_MAX, MATTE_WINDOW_LIMIT, MatteParams, MatteWindowParams,
    MediaAvailabilityStatus, MediaError, MediaKind, Operation, ParamValue, ResolvedCurves,
    TimeCode, TimelineRevision, TrackKind, apply_batch, classify_color_node,
    classify_source_with_assumption, effect_compatibility_stage, effect_descriptor, lut_node_count,
    lut_node_may_be_active, managed_color_node_count,
};
use serde_json::{Value, json};
use thiserror::Error;

const PRIMARY_CORRECTION_EFFECT_NAME: &str = "primary_correction";

/// The four managed colour node kinds that may carry a matte (CC5 §2.1).
///
/// `technical_lut` is deliberately absent: a technical input transform
/// normalizes the *whole* source, so a partially applied one is not a
/// meaningful state.
pub(crate) const MATTE_CAPABLE_NODE_NAMES: [&str; 4] = [
    "primary_correction",
    "color_wheels",
    "color_curves",
    "creative_look",
];

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

/// Which variant of a stored colour node the AFTER cell renders (CC4 §8).
///
/// The BEFORE cell is always the same composite with the node removed, so
/// `bypass` is provably the lossless twin of `before` rather than a preview
/// shortcut (CC4 §3.6).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LookComparison {
    /// The node removed, which is the BEFORE baseline itself.
    Before,
    /// The node exactly as stored, including its mix. The default.
    After,
    /// The node present with `bypass = 1` on a scratch copy of the document.
    Bypass,
}

impl LookComparison {
    /// The stable manifest token.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Before => "before",
            Self::After => "after",
            Self::Bypass => "bypass",
        }
    }
}

/// Which matte-scoped variant `render_color_proof` renders (CC5 §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MatteComparison {
    /// The CC5 §4.1 coverage image itself, `R = G = B = round(255·m)`.
    Coverage,
    /// The document exactly as stored: the correction applies inside the
    /// matte and nowhere else.
    InsideOnly,
    /// A scratch copy with `matte_invert` toggled, so the correction applies
    /// outside the matte and nowhere else.
    OutsideOnly,
}

impl MatteComparison {
    /// The stable manifest token.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Coverage => "coverage",
            Self::InsideOnly => "inside_only",
            Self::OutsideOnly => "outside_only",
        }
    }
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
    /// Integer CC1 parameters for an evidence-only proposed primary. Must be
    /// absent when `effect_id` proofs a stored node instead.
    #[serde(default)]
    pub parameters: BTreeMap<String, i64>,
    /// Proof the *stored* managed colour node at this id instead of a proposed
    /// primary correction (CC4 §8). `parameters` must then be absent.
    #[serde(default)]
    pub effect_id: Option<EffectId>,
    /// Which variant of the stored node the AFTER cell renders. Requires
    /// `effect_id`; defaults to `after`.
    #[serde(default)]
    pub look_comparison: Option<LookComparison>,
    /// CC5 §7: render the node's matte instead of, or partitioned by, its
    /// colour. Valid only alongside `effect_id` on a matte-carrying node, and
    /// mutually exclusive with `look_comparison`, which selects a different
    /// AFTER cell for the same question.
    #[serde(default)]
    pub matte_comparison: Option<MatteComparison>,
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
    /// CC4 §2.3, §8: an active LUT node's asset is not in the verified library
    /// the renderer was handed, so the frame could not be produced.
    ///
    /// The managed renderer already refuses this rather than rendering a
    /// look-free frame; this variant is what turns that refusal into the same
    /// typed `field`/`observed`/`allowed`/`recovery_action` shape every other
    /// CC4 rejection uses, instead of a `render_failed` message an agent has
    /// to read as prose.
    #[error(
        "managed compositor refused the {stage} render: LUT asset {lut_asset} is not in the verified LUT library"
    )]
    MissingLutAsset {
        stage: &'static str,
        /// The node the renderer named, when the failure identified one.
        effect: Option<EffectId>,
        lut_asset: LutAssetId,
        /// The recorded identity, when the project registers the asset at all.
        sha256: Option<String>,
        title: Option<String>,
        /// Where the store would hold the bytes, when a root is published and
        /// the asset is imported rather than built in.
        store_path: Option<PathBuf>,
        availability: Value,
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
    #[error(
        "render_color_proof cannot take both effect_id and parameters: effect_id proofs the stored node"
    )]
    LookProofParametersConflict { effect: EffectId },
    #[error("look_comparison requires effect_id, which names the stored node to compare")]
    LookComparisonRequiresEffectId,
    #[error("clip {clip} has no effect {effect}")]
    ProofEffectNotFound { clip: ClipId, effect: EffectId },
    #[error("effect {effect} is {name}, which is not a managed colour node")]
    ProofEffectNotAColorNode { effect: EffectId, name: String },
    #[error(
        "look_comparison=bypass needs a node with a bypass control, and {kind} node {effect} has none"
    )]
    LookBypassUnsupported {
        effect: EffectId,
        kind: &'static str,
    },
    /// CC4 §8: the manifest *asserts* that the bypass variant is the
    /// byte-identical twin of the node-removed variant, so a difference is a
    /// typed refusal rather than a `bypass_matches_absent: false` footnote on
    /// an otherwise successful proof.
    #[error(
        "look_comparison=bypass must render byte-identically to the node-removed variant, and node {effect} did not"
    )]
    BypassNotLossless {
        effect: EffectId,
        absent_rgba8_pixels_sha256: String,
        bypass_rgba8_pixels_sha256: String,
        absent_raster: (u32, u32),
        bypass_raster: (u32, u32),
    },
    // ---- CC5 §7 ----
    #[error("matte_comparison requires effect_id, which names the matte-carrying node")]
    MatteComparisonRequiresEffectId,
    #[error(
        "matte_comparison and look_comparison both select the AFTER cell; send exactly one of them"
    )]
    MatteComparisonConflictsWithLookComparison,
    #[error("matte_comparison needs a matte-capable node, and {kind} node {effect} is not one")]
    MatteComparisonUnsupportedKind {
        effect: EffectId,
        kind: &'static str,
    },
    #[error("matte_comparison needs a node that carries a matte, and node {effect} carries none")]
    MatteComparisonNoMatte { effect: EffectId },
    #[error("could not render the CC5 matte proof for node {effect}: {message}")]
    MatteProofUnavailable { effect: EffectId, message: String },
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
            Self::MissingLutAsset { .. } => "missing_lut_asset",
            Self::MatteComparisonRequiresEffectId => "matte_comparison_requires_effect_id",
            Self::MatteComparisonConflictsWithLookComparison => {
                "matte_comparison_conflicts_with_look_comparison"
            }
            Self::MatteComparisonUnsupportedKind { .. } => "matte_unsupported_node_kind",
            Self::MatteComparisonNoMatte { .. } => "matte_proof_no_matte",
            Self::MatteProofUnavailable { .. } => MATTE_PROOF_UNAVAILABLE,
            Self::UnsupportedActiveLayerSource { .. } => "active_layer_needs_color_override",
            Self::LookProofParametersConflict { .. } => "look_proof_parameters_conflict",
            Self::LookComparisonRequiresEffectId => "look_comparison_requires_effect_id",
            Self::ProofEffectNotFound { .. } => "effect_not_found",
            Self::ProofEffectNotAColorNode { .. } => "not_a_managed_color_node",
            Self::LookBypassUnsupported { .. } => "bypass_unsupported_for_node",
            Self::BypassNotLossless { .. } => "bypass_not_lossless",
            Self::Primary(error) => error.code(),
        }
    }

    #[must_use]
    #[allow(clippy::too_many_lines)]
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
            Self::MissingLutAsset {
                stage,
                effect,
                lut_asset,
                sha256,
                title,
                store_path,
                availability,
            } => json!({
                "field": "lut_asset_id",
                "observed": lut_asset.0,
                "allowed": "a lut_asset_id whose bytes are hash-verified in the project LUT store and published to the renderer",
                "recovery_action": "Call list_look_assets for this asset's availability, then restore its store bytes or import a replacement and retarget the node before proofing.",
                "stage": stage,
                "effect_id": effect.map(|effect| effect.0),
                "lut_asset_id": lut_asset.0,
                "lut_title": title,
                "lut_sha256": sha256,
                "store_path": store_path,
                "availability": availability,
            }),
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
            Self::LookProofParametersConflict { effect } => json!({
                "field": "parameters",
                "observed": "a non-empty parameters object alongside effect_id",
                "allowed": "exactly one of parameters (propose a primary) or effect_id (proof the stored node)",
                "recovery_action": "Drop parameters to proof the stored node, or drop effect_id to proof a proposed primary correction.",
                "effect_id": effect.0,
            }),
            Self::MatteComparisonRequiresEffectId => json!({
                "field": "matte_comparison",
                "observed": "matte_comparison without effect_id",
                "allowed": "matte_comparison only alongside effect_id",
                "recovery_action": "Send effect_id naming the matte-carrying node, or drop matte_comparison.",
            }),
            Self::MatteComparisonConflictsWithLookComparison => json!({
                "field": "matte_comparison",
                "observed": "matte_comparison and look_comparison together",
                "allowed": "exactly one of matte_comparison or look_comparison",
                "recovery_action": "Both select what the AFTER cell renders, so send one: look_comparison for before/after/bypass, matte_comparison for coverage/inside_only/outside_only.",
            }),
            Self::MatteComparisonUnsupportedKind { effect, kind } => json!({
                "field": "matte_comparison",
                "observed": {"effect_id": effect.0, "kind": kind},
                "allowed": MATTE_CAPABLE_NODE_NAMES,
                "recovery_action": "A technical input transform normalizes the whole source, so it carries no matte (CC5 §2.1). Proof a primary_correction, color_wheels, color_curves, or creative_look node.",
                "effect_id": effect.0,
            }),
            Self::MatteComparisonNoMatte { effect } => json!({
                "field": "matte_comparison",
                "observed": {"effect_id": effect.0, "has_matte": false},
                "allowed": "a node whose resolved matte is active (CC5 §2.6)",
                // CC5 §4.1: a matte proof never returns a blank frame.
                "recovery_action": "Add a matte with plan_secondary_correction first; a node with no matte has no coverage to partition, and this proof never returns a blank frame.",
                "effect_id": effect.0,
            }),
            Self::MatteProofUnavailable { effect, message } => json!({
                "field": "matte_comparison",
                "observed": {"effect_id": effect.0, "message": message},
                "allowed": "a matte-carrying node this build's renderer can proof",
                "recovery_action": "Retry once this build's renderer supports matte proofs; no coverage image is invented here.",
                "effect_id": effect.0,
            }),
            Self::LookComparisonRequiresEffectId => json!({
                "field": "look_comparison",
                "observed": "look_comparison without effect_id",
                "allowed": "look_comparison only alongside effect_id",
                "recovery_action": "Send effect_id naming the stored node to compare, or drop look_comparison.",
            }),
            Self::ProofEffectNotFound { clip, effect } => json!({
                "field": "effect_id",
                "observed": effect.0,
                "allowed": "an effect id on the requested clip",
                "recovery_action": "Call get_color_context for the clip's ordered colour node stack and its effect ids.",
                "clip_id": clip.0,
            }),
            Self::ProofEffectNotAColorNode { effect, name } => json!({
                "field": "effect_id",
                "observed": name,
                "allowed": MANAGED_COLOR_NODE_NAMES,
                "recovery_action": "Name a managed colour node; legacy compatibility stages are outside the CC1 managed proof.",
                "effect_id": effect.0,
            }),
            Self::LookBypassUnsupported { effect, kind } => json!({
                "field": "look_comparison",
                "observed": {"effect_id": effect.0, "kind": kind, "look_comparison": "bypass"},
                "allowed": ["before", "after"],
                "recovery_action": "CC1 primaries carry no bypass control (CC3 §5 applies to CC3/CC4 nodes); compare against the node-removed variant with look_comparison=before instead.",
                "effect_id": effect.0,
            }),
            Self::BypassNotLossless {
                effect,
                absent_rgba8_pixels_sha256,
                bypass_rgba8_pixels_sha256,
                absent_raster,
                bypass_raster,
            } => json!({
                "field": "look_comparison",
                "observed": {
                    "absent_rgba8_pixels_sha256": absent_rgba8_pixels_sha256,
                    "bypass_rgba8_pixels_sha256": bypass_rgba8_pixels_sha256,
                    "absent_raster": {"width": absent_raster.0, "height": absent_raster.1},
                    "bypass_raster": {"width": bypass_raster.0, "height": bypass_raster.1},
                },
                "allowed": "the bypass variant must be the byte-identical twin of the node-removed variant",
                "recovery_action": "Compare against the node-removed variant with look_comparison=before, and report the bypass path: a bypassed node must contribute nothing at all (CC4 §3.6, §8).",
                "effect_id": effect.0,
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

    /// The proof-boundary form of [`Self::from_media_error`], which also
    /// recovers the managed renderer's `missing_lut_asset` refusal (CC4 §2.3).
    ///
    /// `MediaError` carries no LUT variant, so the compositor encodes the code
    /// as a `"missing_lut_asset: "` prefix behind `MediaError::Backend`. The id
    /// is parsed back out here and enriched from the session's
    /// [`LookAssetContext`], so a proof reports the asset, its recorded hash,
    /// where the store would hold it, and its live availability rather than a
    /// prose render failure. `proofed_effect` is the fallback for the renderer
    /// shapes that name no node.
    pub(crate) fn from_proof_render_error(
        stage: &'static str,
        error: MediaError,
        looks: &LookAssetContext,
        proofed_effect: Option<EffectId>,
    ) -> Self {
        let MediaError::Backend(message) = &error else {
            return Self::from_media_error(stage, error);
        };
        let Some(refusal) = message.strip_prefix("missing_lut_asset:") else {
            return Self::from_media_error(stage, error);
        };
        // Every renderer shape names the asset after `LUT asset`, with the
        // punctuation varying (`LUT asset 3`, `LUT asset(s) 3 (<sha>)`). A
        // report that names several takes the first, because the variant
        // describes one asset and the first is the one the operator recovers.
        let Some(lut_asset) = parse_id_after(refusal, "LUT asset").map(LutAssetId) else {
            return Self::from_media_error(stage, error);
        };
        let asset = looks.asset(lut_asset);
        Self::MissingLutAsset {
            stage,
            effect: parse_id_after(refusal, " node")
                .map(EffectId)
                .or(proofed_effect),
            lut_asset,
            sha256: asset.map(|asset| asset.sha256.clone()),
            title: asset.map(|asset| asset.title.clone()),
            store_path: asset.and_then(|asset| looks.store_path(asset)),
            availability: match asset {
                Some(_) => looks.availability_value(lut_asset),
                // Not "unknown because no store": the project registers no
                // asset with this id at all, which no store could change.
                None => json!({
                    "kind": "unregistered",
                    "reason": "the project registers no LUT asset with this id",
                }),
            },
        }
    }
}

/// The first decimal id following `marker`, or `None`.
///
/// The renderer writes the same id behind slightly different punctuation
/// depending on which layer refused — `LUT asset 3`, `LUT asset(s) 3 (<sha>)` —
/// so only that punctuation is skipped, never a word.
fn parse_id_after(text: &str, marker: &str) -> Option<u64> {
    let digits: String = text
        .split_once(marker)?
        .1
        .trim_start_matches(['(', 's', ')', ' '])
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
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
                // CC5 §2.2: the 47 matte parameters are legal on this node but
                // are described by one legend, never enumerated.
                "matte_parameters": matte_parameter_legend(),
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

// ---------------------------------------------------------------------------
// CC5 §2.2 — the compact matte legend
// ---------------------------------------------------------------------------

/// The 47 CC5 matte parameters, summarised in one legend (CC5 §2.2, M36).
///
/// Normative: no agent surface may enumerate the 32 `matte_window{j}_*`
/// parameters per kind. Four matte-capable descriptors × 47 parameters is
/// several kilobytes on every `AddEffect`/`SetEffectParam` description and on
/// every planner rejection, for a control surface an agent should reach
/// through `plan_secondary_correction` anyway.
///
/// Every bound is read from the Core descriptors, so the legend cannot drift
/// from the values Core actually validates.
#[must_use]
pub(crate) fn matte_parameter_legend() -> String {
    let bound = |name: &str| {
        kinewright_core::matte_parameters()
            .iter()
            .find(|parameter| parameter.name == name)
            .map_or_else(
                || "?".to_owned(),
                |parameter| format!("{}..={}", parameter.min, parameter.max),
            )
    };
    let window = |suffix: &str| {
        kinewright_core::matte_window_parameters(0)
            .and_then(|table| {
                table
                    .iter()
                    .find(|parameter| parameter.name.ends_with(suffix))
                    .map(|parameter| format!("{}..={}", parameter.min, parameter.max))
            })
            .unwrap_or_else(|| "?".to_owned())
    };
    format!(
        "matte_* CC5 secondary, {count} parameters: \
matte_enabled/matte_qualifier_enabled/matte_invert/matte_combine_token={token}, neutral 0, combine 0 union 1 intersection; \
matte_window_count={count_range}, neutral 0; \
matte_mix_basis_points={mix}, neutral {mix_neutral}; \
matte_hue_center_centidegrees={hue_center}, neutral 0; \
matte_hue_width_centidegrees and matte_hue_softness_centidegrees={hue_width}, width neutral {hue_disable} disables the hue leg; \
matte_saturation_ and matte_luma_ each with low/high/softness_basis_points={band}, neutral 0/{band_high}/0; \
matte_window{{j}}_* for j={window_min}..={window_max}: shape_token={shape}, 1 rect 2 ellipse, neutral 1; \
center_x/center_y_basis_points={centre}, neutral 5000; \
half_width/half_height_basis_points={half}, neutral 2500; \
rotation_centidegrees={rotation}, neutral 0; \
feather_basis_points={feather}, neutral 0; \
invert={token}, neutral 0. \
Prefer plan_secondary_correction, which accepts windows[] and qualifier{{}} and expands them; \
inspect_grade_matte reports measured coverage",
        count = kinewright_core::MATTE_PARAMETER_COUNT,
        token = bound("matte_enabled"),
        count_range = bound("matte_window_count"),
        mix = bound("matte_mix_basis_points"),
        mix_neutral = kinewright_core::MATTE_MIX_BASIS_POINTS_MAX,
        hue_center = bound("matte_hue_center_centidegrees"),
        hue_width = bound("matte_hue_width_centidegrees"),
        hue_disable = kinewright_core::MATTE_HUE_WIDTH_DISABLE_CENTIDEGREES,
        band = bound("matte_saturation_low_basis_points"),
        band_high = kinewright_core::MATTE_MIX_BASIS_POINTS_MAX,
        window_min = 0,
        window_max = kinewright_core::MATTE_WINDOW_LIMIT.saturating_sub(1),
        shape = window("_shape_token"),
        centre = window("_center_x_basis_points"),
        half = window("_half_width_basis_points"),
        rotation = window("_rotation_centidegrees"),
        feather = window("_feather_basis_points"),
    )
}

/// A one-line pointer to the matte, for surfaces that are not the matte's own.
///
/// The CC1/CC3/CC4 planners each own one node's *colour* controls; the matte is
/// `plan_secondary_correction`'s subject. Repeating the full
/// [`matte_parameter_legend`] in all four descriptions would push each past
/// M36's kilobyte budget to describe a request none of them accepts, so they
/// name the capability and the tool that expands it instead.
///
/// Deliberately terse. `plan_color_wheels` enumerates thirteen descriptor
/// controls before this is appended and had 981 bytes of its M36 kilobyte
/// spent already, so anything longer than a capability name and a tool name
/// puts that tool over budget.
/// `cc5_matte_tools_are_registered_read_only_inspectors` measures it.
#[must_use]
pub(crate) fn matte_parameter_pointer() -> String {
    format!("CC5 matte: {MATTE_PLANNER_TOOL}")
}

/// The tool that owns the matte, named once so the pointers cannot drift.
const MATTE_PLANNER_TOOL: &str = "plan_secondary_correction";

/// Where the full legend is served, for `plan_secondary_correction` itself.
///
/// The matte is that tool's own subject, so repeating
/// [`matte_parameter_legend`] there costs 975 bytes to tell a caller about the
/// integers its ergonomic `windows[]`/`qualifier{}` request exists to hide —
/// and the legend ends by recommending the very tool being described. It names
/// the two surfaces that do enumerate the parameters instead.
#[must_use]
pub(crate) fn matte_legend_reference() -> String {
    format!(
        "windows[] and qualifier{{}} expand to the {} matte_* integers; add_effect and set_effect_param enumerate them in full, and a rejection here repeats that legend in details.matte_parameters.",
        kinewright_core::MATTE_PARAMETER_COUNT,
    )
}

/// Whether `effect` is a matte-capable kind whose listings need the legend.
#[must_use]
pub(crate) fn effect_carries_matte_parameters(effect: &str) -> bool {
    kinewright_core::is_matte_capable_color_node(effect)
}

/// One descriptor's controls with the 47 matte parameters removed (CC5 §2.2).
///
/// Every enumerating surface — tool descriptions, planner summaries, planner
/// rejection evidence, and a plan's `resolved_parameters` — reads its control
/// list through here, so the matte can only ever be described by the legend.
fn non_matte_parameters(
    descriptor: kinewright_core::EffectDescriptor,
) -> impl Iterator<Item = &'static kinewright_core::EffectParameterDescriptor> {
    descriptor
        .parameters
        .iter()
        .filter(|parameter| !kinewright_core::is_matte_parameter(parameter.name))
}

/// Drop the 47 matte parameters a caller did not explicitly name (CC5 §2.2).
///
/// A plan's `resolved_parameters` is an enumerating surface, so it obeys the
/// same rule as every other: the matte is described by
/// [`matte_parameter_legend`], not by 47 integers nobody asked for. A
/// `matte_*` control the caller *did* name is kept, because a proposal must
/// never hide a parameter it would write.
fn retain_requested_matte_parameters(
    resolved: &mut BTreeMap<String, i64>,
    requested: &BTreeMap<String, i64>,
) {
    resolved.retain(|name, _| {
        !kinewright_core::is_matte_parameter(name) || requested.contains_key(name)
    });
}

/// The exact CC1 primary controls, derived from the Core descriptor so the
/// tool description and the `unknown_primary_parameter` recovery evidence can
/// never drift from the values Core actually validates.
#[must_use]
pub(crate) fn primary_parameter_documentation() -> Vec<Value> {
    effect_descriptor(PRIMARY_CORRECTION_EFFECT_NAME).map_or_else(Vec::new, |descriptor| {
        // CC5 §2.2: the 47 matte parameters are described by the legend that
        // rides alongside this list, never enumerated.
        non_matte_parameters(descriptor)
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
        let controls = non_matte_parameters(descriptor)
            .map(|parameter| {
                format!(
                    "{}={}..={}, neutral {}",
                    parameter.name, parameter.min, parameter.max, parameter.neutral
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        format!("{controls}; {}", matte_parameter_pointer())
    })
}

/// Render the complete CC1 metadata/status surface without probing, mutating,
/// or silently changing any source description.
///
/// The store-free shorthand used by unit tests; the served surface always
/// passes a store-backed [`LookAssetContext`].
#[cfg(test)]
#[must_use]
pub(crate) fn color_context_value(revision: TimelineRevision, document: &Document) -> Value {
    color_context_value_with_options(
        revision,
        document,
        None,
        &[],
        false,
        &LookAssetContext::document_only(document),
    )
}

/// Render colour status with an explicit, non-mutating profile assumption.
#[must_use]
pub(crate) fn color_context_value_with_assumptions(
    revision: TimelineRevision,
    document: &Document,
    profile_assumption: Option<ColorSourceProfileAssumption>,
    assumption_asset_ids: &[AssetId],
    looks: &LookAssetContext,
) -> Value {
    color_context_value_with_options(
        revision,
        document,
        profile_assumption,
        assumption_asset_ids,
        false,
        looks,
    )
}

/// Render colour status with optional explicit assumptions and a raw-only mode.
#[allow(clippy::too_many_lines)]
pub(crate) fn color_context_value_with_options(
    revision: TimelineRevision,
    document: &Document,
    profile_assumption: Option<ColorSourceProfileAssumption>,
    assumption_asset_ids: &[AssetId],
    raw_only: bool,
    looks: &LookAssetContext,
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
                .map(move |clip| clip_status(track.id.0, z_order, clip, looks))
        })
        .collect::<Vec<_>>();
    let legacy_stage_warnings = legacy_stage_warnings_for_document(document);
    let legacy_look_conversions = legacy_look_conversions_value(document);

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
        // CC4 §9: the explicit [AddLutAsset, ConvertLegacyLook] batch each
        // legacy look would need. Conversion is never automatic and is not
        // bit-identical, so the batch is evidence, not an applied edit.
        "legacy_look_conversions": legacy_look_conversions,
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

fn clip_status(
    track_id: u64,
    z_order: usize,
    clip: &kinewright_core::Clip,
    looks: &LookAssetContext,
) -> Value {
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
        "color_nodes": color_node_manifest(&clip.effects, looks),
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
///
/// CC5 §2.2/§7: the 47 `matte_*` controls are filtered out for the same reason
/// [`resolved_descriptor_parameters`] filters them — this feeds
/// `effect_chain[].primary_parameters`, which is an enumerating surface, and
/// the matte is described by the `color_nodes[].matte` object instead.
fn resolved_primary_parameters(effect: &Effect) -> Option<BTreeMap<&'static str, ParamValue>> {
    effect_descriptor(PRIMARY_CORRECTION_EFFECT_NAME).map(|descriptor| {
        non_matte_parameters(descriptor)
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

    // CC5 §2.2: an omitted matte parameter resolves to its neutral and the
    // matte is inactive, so a fresh primary stores its ten CC1 controls and
    // none of the 47 matte neutrals. Writing them would bloat every project
    // JSON and would make a CC4-era node textually different for no effect.
    let neutral_parameters = non_matte_parameters(descriptor)
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
    retain_requested_matte_parameters(&mut resolved_parameters, &args.parameters);
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
    /// CC4 LUT planners only: the exact `clip.effects` index the emitted
    /// `InsertEffect` uses, which is the first index satisfying the CC4 §3.2
    /// stage rule. `None` for the CC3 planners, which append.
    pub insert_index: Option<usize>,
    /// CC4 LUT planners only: the bound asset's title, hash, provenance, and
    /// availability.
    pub lut_asset: Option<Value>,
    /// CC5 §7: the resolved `matte` object this proposal produces, in the same
    /// compact integer shape the `color_nodes` manifest reports. `None` for
    /// every planner that does not touch a matte, so a CC4 response is
    /// byte-unchanged.
    pub matte: Option<Value>,
    /// CC5 §7: the §4.2 coverage statistics measured on a scratch document
    /// carrying this proposal, or a typed `matte_proof_unavailable` reason
    /// when the analysis backend cannot render a matte proof.
    pub predicted_coverage: Option<Value>,
    /// CC5 §7: measured hue/saturation/luma statistics of an optional
    /// `sample_roi`, returned as evidence whether or not the caller asked for
    /// a derived qualifier.
    pub sample_evidence: Option<Value>,
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
    #[error("clip {clip} already carries {actual} LUT nodes; the limit is {limit}")]
    TooManyLutNodes {
        clip: ClipId,
        actual: usize,
        limit: usize,
    },
    #[error("clip {clip} cannot bind lut_asset_id {lut_asset}: the project does not register it")]
    UnknownLutAsset {
        clip: ClipId,
        lut_asset: LutAssetId,
        /// Every registered asset id, ascending, so the caller can retry
        /// without a second round trip.
        allowed: Vec<u64>,
    },
    // ---- CC5 §7 ----
    #[error("send exactly one of target_effect_id or node_kind, not both")]
    MatteTargetAmbiguous,
    #[error("send target_effect_id or node_kind to name the node the matte belongs to")]
    MatteTargetRequired,
    #[error("clip {clip} carries no effect {effect}")]
    MatteTargetNotFound { clip: ClipId, effect: EffectId },
    #[error("effect {effect} is {name}, which is not a managed colour node")]
    MatteTargetNotAColorNode { effect: EffectId, name: String },
    #[error("{observed} cannot carry a matte")]
    MatteUnsupportedKind { observed: &'static str },
    #[error("{observed} is not a managed colour node kind")]
    MatteUnknownKind { observed: String },
    #[error("a matte carries at most {max} windows, {observed} requested")]
    MatteWindowCount { observed: usize, max: usize },
    #[error("{field} token {observed} is not recognized")]
    MatteTokenNotRecognized {
        field: &'static str,
        observed: String,
        allowed: &'static [&'static str],
    },
    #[error("windows[{index}].{field} token {observed} is not recognized")]
    MatteWindowTokenNotRecognized {
        index: usize,
        field: &'static str,
        observed: String,
        allowed: &'static [&'static str],
    },
    #[error(
        "node {effect} keyframes the Hold-only matte parameter {name}, so a static write would never render"
    )]
    MatteHoldOnlyParameterKeyframed { effect: EffectId, name: String },
    #[error("sample_roi field {field} is invalid: {observed}")]
    MatteSampleRoiInvalid {
        field: &'static str,
        observed: String,
    },
    #[error("sample_roi at frame {at} contains no visible pixel")]
    MatteSampleRoiEmpty {
        at: TimeCode,
        pixel_rect: (u32, u32, u32, u32),
    },
    #[error("could not render the sample frame {at}: {message}")]
    MatteSampleRenderFailed { at: TimeCode, message: String },
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
            Self::TooManyLutNodes { .. } => "too_many_lut_nodes",
            Self::UnknownLutAsset { .. } => "missing_lut_asset",
            Self::MatteTargetAmbiguous => "matte_target_ambiguous",
            Self::MatteTargetRequired => "matte_target_required",
            Self::MatteTargetNotFound { .. } => "matte_target_not_found",
            Self::MatteTargetNotAColorNode { .. } => "matte_target_not_a_color_node",
            Self::MatteUnsupportedKind { .. } => "matte_unsupported_node_kind",
            Self::MatteUnknownKind { .. } => "matte_unknown_node_kind",
            Self::MatteWindowCount { .. } => "matte_window_count_out_of_range",
            Self::MatteTokenNotRecognized { .. } | Self::MatteWindowTokenNotRecognized { .. } => {
                "matte_token_not_recognized"
            }
            Self::MatteHoldOnlyParameterKeyframed { .. } => "matte_hold_only_parameter_keyframed",
            Self::MatteSampleRoiInvalid { .. } => "matte_sample_roi_invalid",
            Self::MatteSampleRoiEmpty { .. } => "matte_sample_roi_empty",
            Self::MatteSampleRenderFailed { .. } => "matte_sample_render_failed",
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
                "matte_parameters": if effect_carries_matte_parameters(effect) {
                    json!(matte_parameter_legend())
                } else {
                    Value::Null
                },
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
            Self::TooManyLutNodes {
                clip,
                actual,
                limit,
            } => json!({
                "field": "clip_id",
                "observed": actual,
                "allowed": {"max": limit},
                "recovery_action": "Retarget an existing technical_lut or creative_look (omit append) or remove one; each LUT node needs its own texture atlas slot (CC4 §3.1).",
                "clip_id": clip.0,
            }),
            Self::UnknownLutAsset {
                clip,
                lut_asset,
                allowed,
            } => json!({
                "field": "lut_asset_id",
                "observed": lut_asset.0,
                "allowed": allowed,
                "recovery_action": "Call list_look_assets for the registered ids, or import_lut_asset to register a new .cube file first.",
                "clip_id": clip.0,
                "lut_asset_id": lut_asset.0,
            }),
            Self::MatteTargetAmbiguous => json!({
                "field": "target_effect_id",
                "observed": "target_effect_id and node_kind",
                "allowed": "exactly one of target_effect_id or node_kind",
                "recovery_action": "Send target_effect_id to matte one exact stored node, or node_kind to matte the clip's node of that kind.",
            }),
            Self::MatteTargetRequired => json!({
                "field": "target_effect_id",
                "observed": Value::Null,
                "allowed": "exactly one of target_effect_id or node_kind",
                "recovery_action": "Call get_color_context for the clip's colour_nodes, then send target_effect_id, or send node_kind for a new node.",
            }),
            Self::MatteTargetNotFound { clip, effect } => json!({
                "field": "target_effect_id",
                "observed": effect.0,
                "allowed": "an effect id on the requested clip",
                "recovery_action": "Call get_color_context for the clip's current effect ids.",
                "clip_id": clip.0,
            }),
            Self::MatteTargetNotAColorNode { effect, name } => json!({
                "field": "target_effect_id",
                "observed": {"effect_id": effect.0, "name": name},
                "allowed": MATTE_CAPABLE_NODE_NAMES,
                "recovery_action": "A matte belongs to a managed correction node; target one of the matte-capable kinds.",
            }),
            // CC5 §2.1: a technical input transform normalizes the *whole*
            // source, so a partially applied one is not a meaningful state.
            Self::MatteUnsupportedKind { observed } => json!({
                "field": "node_kind",
                "observed": observed,
                "allowed": MATTE_CAPABLE_NODE_NAMES,
                "recovery_action": "A technical input transform normalizes the whole source, so a partially applied one is not a meaningful state (CC5 §2.1). Matte a primary_correction, color_wheels, color_curves, or creative_look node instead.",
            }),
            Self::MatteUnknownKind { observed } => json!({
                "field": "node_kind",
                "observed": observed,
                "allowed": MATTE_CAPABLE_NODE_NAMES,
                "recovery_action": "Send one of the four matte-capable managed colour node names.",
            }),
            Self::MatteWindowCount { observed, max } => json!({
                "field": "windows",
                "observed": observed,
                "allowed": {"max": max},
                "recovery_action": format!("Send at most {max} windows; combine them with union or intersection (CC5 §2.3)."),
            }),
            Self::MatteTokenNotRecognized {
                field,
                observed,
                allowed,
            } => json!({
                "field": field,
                "observed": observed,
                "allowed": allowed,
                "recovery_action": format!("Send {field} as one of {}.", allowed.join(" or ")),
            }),
            Self::MatteWindowTokenNotRecognized {
                index,
                field,
                observed,
                allowed,
            } => json!({
                "field": format!("windows[{index}].{field}"),
                "observed": observed,
                "allowed": allowed,
                "recovery_action": format!("Send windows[{index}].{field} as one of {}.", allowed.join(" or ")),
            }),
            // CC5 §5.1: a token accepts Hold keyframes only, and an existing
            // Hold curve wins over a static write at every frame from its first
            // keyframe onward — so this is a refusal, not a warning.
            Self::MatteHoldOnlyParameterKeyframed { effect, name } => json!({
                "field": name,
                "observed": format!("effect {effect} keyframes {name}"),
                "allowed": "a Hold-only matte token with no automation, so the static value renders",
                "recovery_action": format!("Clear the automation on {name} with ClearEffectKeyframes, then resend; or send SetEffectKeyframes with Hold keyframes to animate the token deliberately (CC5 §5.1)."),
                "effect_id": effect.0,
                "hold_only": true,
            }),
            Self::MatteSampleRoiInvalid { field, observed } => json!({
                "field": field,
                "observed": observed,
                "allowed": "finite normalized coordinates with x, y >= 0, width, height > 0, and x + width, y + height <= 1",
                "recovery_action": "Send sample_roi as a normalized 0..=1 rectangle inside the frame.",
            }),
            Self::MatteSampleRoiEmpty { at, pixel_rect } => json!({
                "field": "sample_roi",
                "observed": {
                    "project_frame": at.0,
                    "pixel_rect": {"x": pixel_rect.0, "y": pixel_rect.1, "width": pixel_rect.2, "height": pixel_rect.3},
                    "visible_pixel_count": 0,
                },
                "allowed": "a region containing at least one pixel whose alpha is non-zero",
                "recovery_action": "Sample a region the clip actually covers at this frame; a fully transparent region carries no colour to measure (CC2's visible-pixel rule).",
            }),
            Self::MatteSampleRenderFailed { at, message } => json!({
                "field": "sample_roi",
                "observed": {"project_frame": at.0, "message": message},
                "allowed": "a frame the managed monitor proof can render",
                "recovery_action": "Check get_media_status for the clip's asset, then retry at a frame the clip covers.",
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
        // CC5 §2.2: base controls only; the matte legend accompanies the list.
        non_matte_parameters(descriptor)
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
        let controls = non_matte_parameters(descriptor)
            .map(|parameter| {
                format!(
                    "{}={}..={}, neutral {}",
                    parameter.name, parameter.min, parameter.max, parameter.neutral
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        format!("{controls}; {}", matte_parameter_pointer())
    })
}

/// A compact one-line summary of one CC4 LUT node's controls (CC4 §8, M36).
///
/// `lut_asset_id` spans `0..=2^53-1`, so its range is deliberately replaced by
/// a pointer to the tool that lists the ids a caller can actually use; every
/// other bound is read from the Core descriptor and cannot drift.
#[must_use]
pub(crate) fn lut_node_parameter_summary(kind: ColorNodeKind) -> String {
    effect_descriptor(kind.effect_name()).map_or_else(String::new, |descriptor| {
        let controls = non_matte_parameters(descriptor)
            .map(|parameter| {
                if parameter.name == LUT_ASSET_ID_PARAMETER {
                    "lut_asset_id (project LUT asset id; see list_look_assets)".to_owned()
                } else {
                    format!(
                        "{}={}..={}, neutral {}",
                        parameter.name, parameter.min, parameter.max, parameter.neutral
                    )
                }
            })
            .collect::<Vec<_>>()
            .join("; ");
        let mut summary = format!("{controls}; {}", input_encoding_legend());
        // CC5 §2.1: `technical_lut` carries no matte, so only `creative_look`
        // gains the legend.
        if effect_carries_matte_parameters(kind.effect_name()) {
            summary.push_str("; ");
            summary.push_str(&matte_parameter_pointer());
        }
        summary
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
    retain_requested_matte_parameters(&mut resolved_parameters, &args.parameters);

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
        insert_index: None,
        lut_asset: None,
        matte: None,
        predicted_coverage: None,
        sample_evidence: None,
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
        insert_index: None,
        lut_asset: None,
        matte: None,
        predicted_coverage: None,
        sample_evidence: None,
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
pub(crate) fn color_node_value(
    effect_index: usize,
    effect: &Effect,
    looks: &LookAssetContext,
) -> Option<Value> {
    let kind = classify_color_node(effect)?;
    let mut warnings = Vec::new();
    let mut lut_fields = serde_json::Map::new();
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
        // CC4 §8: the two LUT kinds are mathematically identical and differ
        // only in stage, role, and mix bounds, so one arm describes both.
        ColorNodeKind::TechnicalLut | ColorNodeKind::CreativeLook => {
            let resolved = LutNodeParams::from_effect(effect);
            let parameters = BTreeMap::from([
                (
                    LUT_ASSET_ID_PARAMETER.to_owned(),
                    lut_asset_id_as_i64(resolved.lut_asset_id),
                ),
                (LUT_MIX_PARAMETER.to_owned(), resolved.mix_basis_points),
                (
                    LUT_INPUT_ENCODING_PARAMETER.to_owned(),
                    resolved.input_encoding_token,
                ),
                (
                    COLOR_NODE_BYPASS_PARAMETER.to_owned(),
                    resolved.bypass_token,
                ),
            ]);
            lut_fields.extend(looks.node_manifest_fields(
                effect,
                resolved,
                &mut warnings,
                effect_index,
            ));
            (
                resolved.bypass_token,
                resolved.inactive_reason(),
                parameters,
                Value::Null,
            )
        }
    };
    // CC5 §2.6 rule 2: a matte that resolves to `m = 0` everywhere makes the
    // node the exact identity, which Core reports as `matte_excluded`. Reading
    // the reason through Core keeps the manifest, the inspector, and the
    // renderer describing one node the same way.
    let inactive_reason = kinewright_core::color_node_inactive_reason(effect).or(inactive_reason);
    if let Some(warning) = matte_band_warning(effect_index, effect) {
        warnings.push(warning);
    }
    let mut value = json!({
        // Position in `clip.effects`, which is the compositor evaluation order.
        "stage_index": effect_index,
        "effect_index": effect_index,
        "effect_id": effect.id.0,
        "kind": kind.effect_name(),
        "name": effect.name,
        // CC4 §3.1: the stable role and stage tokens every colour surface
        // reports, so a reader can tell an input transform from a look.
        "role": kind.role(),
        "color_stage": kind.stage().as_str(),
        "color_stage_rank": kind.stage().rank(),
        "bypass": bypass,
        // CC1 primaries carry no bypass control (CC3 §5 applies to CC3 nodes).
        "supports_bypass": kind != ColorNodeKind::Primary,
        "active": inactive_reason.is_none(),
        "inactive_reason": inactive_reason.map_or(Value::Null, |reason| json!(reason.as_str())),
        "parameters": parameters,
        "curves": curves,
        "keyframes": effect.keyframes,
        "warnings": warnings,
    });
    // CC5 §7: absent entirely when the node carries no matte, so every CC4
    // manifest is byte-unchanged.
    if let Some(matte) = matte_manifest_value(effect)
        && let Some(object) = value.as_object_mut()
    {
        object.insert("matte".to_owned(), matte);
    }
    if !lut_fields.is_empty()
        && let Some(object) = value.as_object_mut()
    {
        object.append(&mut lut_fields);
    }
    Some(value)
}

// ---------------------------------------------------------------------------
// CC5 §7 — the compact `matte` manifest object
// ---------------------------------------------------------------------------

/// The stable `combine` token for one resolved matte (CC5 §2.3).
#[must_use]
pub(crate) const fn matte_combine_token_name(matte: &MatteParams) -> &'static str {
    if matte.intersects() {
        "intersection"
    } else {
        "union"
    }
}

/// One resolved window as the compact integer object the manifest reports.
fn matte_window_value(window: &MatteWindowParams) -> Value {
    json!({
        "shape_token": window.shape_token,
        "shape": if window.is_ellipse() { "ellipse" } else { "rect" },
        "center_x_basis_points": window.center_x_bp,
        "center_y_basis_points": window.center_y_bp,
        "half_width_basis_points": window.half_width_bp,
        "half_height_basis_points": window.half_height_bp,
        "rotation_centidegrees": window.rotation_cd,
        "feather_basis_points": window.feather_bp,
        "invert": window.invert,
    })
}

/// The CC5 §7 `matte` object for one colour node, or `None` when the node
/// carries no matte.
///
/// Absent — not `null`, not an all-neutral object — whenever
/// [`MatteParams::has_matte`] is false, so a CC4-era manifest is byte-unchanged
/// (CC5 §7, §8).  `windows` is truncated to `window_count`, because a stored
/// window past the count is preserved but never rendered (CC5 §2.2), and
/// publishing it would describe geometry that affects no pixel.
#[must_use]
pub(crate) fn matte_manifest_value(effect: &Effect) -> Option<Value> {
    if !kinewright_core::is_matte_capable_color_node(&effect.name) {
        return None;
    }
    let matte = MatteParams::from_effect(effect);
    if !matte.has_matte() {
        return None;
    }
    let excluded = matte.node_excluded_by_matte();
    let degenerate = matte.degenerate_bands();
    Some(json!({
        "enabled": matte.is_enabled(),
        // CC5 §2.6 rule 2: an inverted empty matte or a zero mix makes the
        // whole node the exact identity.
        "active": !excluded,
        "inactive_reason": if excluded {
            json!(ColorNodeInactiveReason::MatteExcluded.as_str())
        } else {
            Value::Null
        },
        "window_count": matte.window_count,
        "combine": matte_combine_token_name(&matte),
        "combine_token": matte.combine_token,
        "invert": matte.invert,
        "mix_basis_points": matte.mix_bp,
        "qualifier": {
            "enabled": matte.qualifier.is_enabled(),
            "hue_leg_disabled": matte.qualifier.hue_leg_disabled(),
            "hue_center_centidegrees": matte.qualifier.hue_center_cd,
            "hue_width_centidegrees": matte.qualifier.hue_width_cd,
            "hue_softness_centidegrees": matte.qualifier.hue_softness_cd,
            "saturation_low_basis_points": matte.qualifier.sat_low_bp,
            "saturation_high_basis_points": matte.qualifier.sat_high_bp,
            "saturation_softness_basis_points": matte.qualifier.sat_softness_bp,
            "luma_low_basis_points": matte.qualifier.luma_low_bp,
            "luma_high_basis_points": matte.qualifier.luma_high_bp,
            "luma_softness_basis_points": matte.qualifier.luma_softness_bp,
        },
        // CC5 §2.2: stored windows past the count render nothing.
        "windows": matte
            .active_windows()
            .map(matte_window_value)
            .collect::<Vec<_>>(),
        // CC5 §2.6: a band whose low edge resolved above its high edge selects
        // nothing.  Core QA reports the same fact as
        // `matte_band_inverted_by_automation`; the manifest must agree.
        "degenerate_bands": degenerate,
    }))
}

/// The `matte_band_inverted_by_automation` manifest warning (CC5 §2.6).
///
/// Emitted next to Core QA's issue of the same code so an agent reading the
/// colour manifest sees the inversion without a second `get_qa_report` call.
fn matte_band_warning(effect_index: usize, effect: &Effect) -> Option<Value> {
    let matte = MatteParams::from_effect(effect);
    if !kinewright_core::is_matte_capable_color_node(&effect.name)
        || !matte.has_matte()
        || !matte.qualifier.is_enabled()
    {
        return None;
    }
    let bands = matte.degenerate_bands();
    if bands.is_empty() {
        return None;
    }
    Some(json!({
        "code": "matte_band_inverted_by_automation",
        "effect_id": effect.id.0,
        "stage_index": effect_index,
        "bands": bands,
        "message": format!(
            "effect {} resolves a matte {} band whose low edge is above its high edge, so that band selects nothing and the matte is empty (CC5 §2.6)",
            effect.id,
            bands.join(", "),
        ),
    }))
}

/// The ordered managed colour-node stack of one effect chain (CC3 §8).
///
/// Shared by `get_color_context` and the `render_color_proof` layer manifest
/// so the two colour surfaces can never describe a different node stack.
#[must_use]
pub(crate) fn color_node_manifest(effects: &[Effect], looks: &LookAssetContext) -> Vec<Value> {
    effects
        .iter()
        .enumerate()
        .filter_map(|(effect_index, effect)| color_node_value(effect_index, effect, looks))
        .collect()
}

/// Every descriptor control resolved against the stored effect, falling back
/// to the descriptor neutral for controls the effect does not carry.
///
/// CC5 §2.2/§7: the 47 `matte_*` controls are filtered out. This is an
/// enumerating surface, so it obeys the module rule — the matte is described
/// by the sibling `matte` manifest object, which CC5 §7 makes absent entirely
/// when the node carries no matte, so every CC4 manifest stays byte-unchanged.
fn resolved_descriptor_parameters(effect: &Effect, name: &str) -> BTreeMap<String, i64> {
    effect_descriptor(name).map_or_else(BTreeMap::new, |descriptor| {
        non_matte_parameters(descriptor)
            .map(|parameter| {
                (
                    parameter.name.to_owned(),
                    stored_parameter(Some(effect), parameter.name, parameter.neutral),
                )
            })
            .collect()
    })
}

// ---------------------------------------------------------------------------
// CC4 §8 — LUT assets, LUT node manifests, and the two look planners
// ---------------------------------------------------------------------------

/// The `availability` token reported when the project has never been saved, so
/// no LUT store root exists to probe (CC4 §2.2, §8).
pub(crate) const LUT_AVAILABILITY_UNKNOWN_NO_STORE: &str = "unknown_no_store";

/// The `.cube` sub-directory of a project LUT store, mirrored here so a manifest
/// can report the expected path without opening the media crate's store type.
const LUT_STORE_LUTS_DIRECTORY: &str = "luts";

/// Availability of a built-in generated asset, computed entirely from this
/// binary (CC4 §2.3).
///
/// `LutStore::availability` already answers this, but it is only reachable
/// through a store root, and §2.3 makes a built-in's status a property of the
/// *bake*, not of the filesystem: "built-in assets are `verified` when the
/// embedded bake hashes to the recorded `sha256`". A project that has never
/// been saved must therefore still report `verified`/`changed` for its
/// built-ins. `None` means the asset has imported provenance, whose bytes only
/// a store can resolve.
///
/// The reason strings are spelled with the media crate's own public
/// [`kinewright_media::LutStoreError`] so an agent reads the identical text
/// whether or not a store happened to be available.
#[must_use]
fn builtin_availability(asset: &LutAsset) -> Option<LutAvailabilityStatus> {
    let LutAssetSource::Builtin { name } = &asset.source else {
        return None;
    };
    let Some(look) = kinewright_media::BuiltinLook::from_name(name) else {
        return Some(LutAvailabilityStatus {
            kind: LutAvailabilityKind::Missing,
            observed_sha256: None,
            reason: Some(
                kinewright_media::LutStoreError {
                    code: kinewright_media::LutStoreErrorCode::UnknownBuiltinLook,
                    detail: "this build has no bake for the recorded built-in look".to_owned(),
                    observed: Some(name.clone()),
                    allowed: None,
                }
                .to_string(),
            ),
            path: None,
        });
    };
    let observed = look.sha256();
    if observed == asset.sha256 {
        return Some(LutAvailabilityStatus {
            kind: LutAvailabilityKind::Verified,
            observed_sha256: Some(observed.to_owned()),
            reason: None,
            path: None,
        });
    }
    Some(LutAvailabilityStatus {
        kind: LutAvailabilityKind::Changed,
        observed_sha256: Some(observed.to_owned()),
        reason: Some(
            kinewright_media::LutStoreError {
                code: kinewright_media::LutStoreErrorCode::ChangedLutAsset,
                detail: format!("this build's {name} bake differs from the recorded content"),
                observed: Some(observed.to_owned()),
                allowed: Some(asset.sha256.clone()),
            }
            .to_string(),
        ),
        path: None,
    })
}

/// The typed recovery for one LUT asset whose bytes did not verify (CC4 §8).
///
/// `None` for `verified`: there is nothing to recover.
#[must_use]
pub(crate) const fn lut_availability_recovery_action(
    kind: LutAvailabilityKind,
) -> Option<&'static str> {
    match kind {
        LutAvailabilityKind::Verified => None,
        LutAvailabilityKind::Missing => Some(
            "The recorded bytes are not in the store. Restore them from the original .cube with the human restore action, or call import_lut_asset for a replacement file and retarget each node with SetEffectParam{name: \"lut_asset_id\"}; a built-in reporting missing means this build has no bake for its recorded name.",
        ),
        LutAvailabilityKind::Changed => Some(
            "A file exists but hashes to different bytes. Restore the recorded bytes, or import_lut_asset the new file as a separate asset and retarget the nodes; no operation ever rewrites an asset's sha256 in place (CC4 §2.3).",
        ),
        LutAvailabilityKind::Unreadable => Some(
            "The store path exists but cannot be read. Repair the project LUT store directory's permissions or contents, then call list_look_assets again.",
        ),
    }
}

/// The recovery for an asset whose availability could not be resolved at all.
const LUT_AVAILABILITY_NO_STORE_RECOVERY: &str = "Save the project so it owns a LUT store root, then call list_look_assets again; imported bytes can only be hash-verified against a store (CC4 §2.2).";

/// Runtime LUT-asset evidence shared by every CC4 agent surface.
///
/// Availability is runtime state that only the store-owning layer can resolve
/// (CC4 §2.3), so it is *injected*: the caller passes the store's resolver and
/// the snapshot is taken once, here, rather than re-probed per node. A context
/// built without a resolver reports `unknown_no_store` and never invents a
/// status.
#[derive(Debug, Clone, Default)]
pub(crate) struct LookAssetContext {
    assets: BTreeMap<LutAssetId, LutAsset>,
    availability: BTreeMap<LutAssetId, LutAvailabilityStatus>,
    store_root: Option<PathBuf>,
}

impl LookAssetContext {
    /// Snapshot every document asset and, when a store root is known, its
    /// current availability.
    #[must_use]
    pub(crate) fn new(
        document: &Document,
        store_root: Option<PathBuf>,
        availability_for: Option<&dyn Fn(&LutAsset) -> LutAvailabilityStatus>,
    ) -> Self {
        let mut assets = BTreeMap::new();
        let mut availability = BTreeMap::new();
        for asset in &document.lut_assets {
            // CC4 §2.3: a built-in is `verified` exactly when this binary's
            // bake hashes to the record, which needs no store and no
            // filesystem at all. Reporting `unknown_no_store` for a built-in
            // in an unsaved project would be a status the contract says is
            // always knowable, so the built-in probe runs either way.
            let status = match availability_for {
                Some(resolver) => Some(resolver(asset)),
                None => builtin_availability(asset),
            };
            if let Some(status) = status {
                availability.insert(asset.id, status);
            }
            assets.insert(asset.id, asset.clone());
        }
        Self {
            assets,
            availability,
            store_root,
        }
    }

    /// The structural-only context used by surfaces that cannot reach a store.
    #[must_use]
    pub(crate) fn document_only(document: &Document) -> Self {
        Self::new(document, None, None)
    }

    /// Whether a store root is known, which is what makes availability
    /// resolvable at all.
    #[must_use]
    pub(crate) const fn store_root(&self) -> Option<&PathBuf> {
        self.store_root.as_ref()
    }

    /// One registered asset record.
    #[must_use]
    pub(crate) fn asset(&self, id: LutAssetId) -> Option<&LutAsset> {
        self.assets.get(&id)
    }

    /// Every registered asset id, ascending, for an error's `allowed` field.
    #[must_use]
    pub(crate) fn asset_ids(&self) -> Vec<u64> {
        self.assets.keys().map(|id| id.0).collect()
    }

    /// The expected `<store_root>/luts/<sha256>.cube` path for an imported
    /// asset. Built-ins are generated in the binary and never written to the
    /// store (CC4 §2.6), so they report no store path.
    #[must_use]
    pub(crate) fn store_path(&self, asset: &LutAsset) -> Option<PathBuf> {
        if matches!(asset.source, LutAssetSource::Builtin { .. }) {
            return None;
        }
        self.store_root.as_ref().map(|root| {
            root.join(LUT_STORE_LUTS_DIRECTORY)
                .join(format!("{}.cube", asset.sha256))
        })
    }

    /// The typed availability of one asset, or the honest `unknown_no_store`
    /// marker when no store root has been published to this session.
    ///
    /// A built-in never reaches the `unknown_no_store` arm: its bytes are in
    /// this binary, so [`builtin_availability`] resolves it with or without a
    /// store (CC4 §2.3).
    #[must_use]
    pub(crate) fn availability_value(&self, id: LutAssetId) -> Value {
        match self.availability.get(&id) {
            Some(status) => serde_json::to_value(status).unwrap_or(Value::Null),
            None => json!({
                "kind": LUT_AVAILABILITY_UNKNOWN_NO_STORE,
                "reason": "no LUT store root is published to this session, so the bytes cannot be hash-verified",
            }),
        }
    }

    /// The typed recovery that belongs next to [`Self::availability_value`]
    /// (CC4 §8): `null` when the asset verified and nothing needs recovering.
    #[must_use]
    pub(crate) fn availability_recovery_action(&self, id: LutAssetId) -> Value {
        match self.availability_kind(id) {
            Some(kind) => {
                lut_availability_recovery_action(kind).map_or(Value::Null, |action| json!(action))
            }
            None => json!(LUT_AVAILABILITY_NO_STORE_RECOVERY),
        }
    }

    /// The availability kind, when it could be resolved.
    #[must_use]
    pub(crate) fn availability_kind(&self, id: LutAssetId) -> Option<LutAvailabilityKind> {
        self.availability.get(&id).map(|status| status.kind)
    }

    /// The CC4 §8 provenance object: built-in name or informational source path.
    #[must_use]
    pub(crate) fn provenance_value(asset: &LutAsset) -> Value {
        match &asset.source {
            LutAssetSource::Builtin { name } => json!({"kind": "builtin", "name": name}),
            // `source_path` is informational only: never opened by the
            // renderer and never resolved relative to anything (CC4 §2.1).
            LutAssetSource::Imported { source_path } => {
                json!({"kind": "imported", "source_path": source_path})
            }
        }
    }

    /// The compact `lut_asset` summary both planners and `list_look_assets`
    /// return for one referenced asset.
    #[must_use]
    pub(crate) fn asset_summary(&self, asset: &LutAsset) -> Value {
        json!({
            "lut_asset_id": asset.id.0,
            "title": asset.title,
            "sha256": asset.sha256,
            "kind": asset.kind.as_str(),
            "size": asset.size,
            "byte_len": asset.byte_len,
            "domain_min_millionths": asset.domain_min_millionths,
            "domain_max_millionths": asset.domain_max_millionths,
            "provenance": Self::provenance_value(asset),
            "availability": self.availability_value(asset.id),
            // CC4 §8: a plan referencing a `missing`/`changed`/`unreadable`
            // asset is returned with the status *and* the recovery action, not
            // silently.
            "recovery_action": self.availability_recovery_action(asset.id),
            "store_path": self.store_path(asset),
        })
    }

    /// The CC4 §8 LUT fields one `color_nodes` entry carries, appending any
    /// asset-identity warning to the node's warning list.
    fn node_manifest_fields(
        &self,
        effect: &Effect,
        resolved: LutNodeParams,
        warnings: &mut Vec<Value>,
        effect_index: usize,
    ) -> serde_json::Map<String, Value> {
        let id = resolved.lut_asset_id;
        let asset = self.asset(id);
        let may_be_active = lut_node_may_be_active(effect);
        if id.0 == 0 {
            warnings.push(lut_node_warning(
                "missing_lut_asset",
                effect,
                effect_index,
                id,
                may_be_active,
                "the node stores no lut_asset_id, so it is unbound and renders as the exact identity",
            ));
        } else if asset.is_none() {
            warnings.push(lut_node_warning(
                "missing_lut_asset",
                effect,
                effect_index,
                id,
                may_be_active,
                "the node references a lut_asset_id that the project does not register",
            ));
        } else if let Some(kind) = self.availability_kind(id)
            && kind != LutAvailabilityKind::Verified
        {
            let (code, detail) = match kind {
                LutAvailabilityKind::Missing => (
                    "missing_lut_asset",
                    "the store file is absent or is not a regular file",
                ),
                LutAvailabilityKind::Changed => (
                    "changed_lut_asset",
                    "a store file exists but its bytes hash to something else",
                ),
                LutAvailabilityKind::Unreadable => (
                    "unreadable_lut_asset",
                    "the store path exists but its bytes or metadata cannot be read",
                ),
                LutAvailabilityKind::Verified => unreachable!("verified is filtered above"),
            };
            warnings.push(lut_node_warning(
                code,
                effect,
                effect_index,
                id,
                may_be_active,
                detail,
            ));
        }
        let mut fields = serde_json::Map::new();
        fields.insert("lut_asset_id".to_owned(), json!(id.0));
        fields.insert(
            "lut_title".to_owned(),
            asset.map_or(Value::Null, |asset| json!(asset.title)),
        );
        fields.insert(
            "lut_sha256".to_owned(),
            asset.map_or(Value::Null, |asset| json!(asset.sha256)),
        );
        fields.insert(
            "lut_size".to_owned(),
            asset.map_or(Value::Null, |asset| json!(asset.size)),
        );
        fields.insert(
            "lut_kind".to_owned(),
            asset.map_or(Value::Null, |asset| json!(asset.kind.as_str())),
        );
        fields.insert(
            "lut_provenance".to_owned(),
            asset.map_or(Value::Null, Self::provenance_value),
        );
        fields.insert("lut_availability".to_owned(), self.availability_value(id));
        fields.insert(
            "lut_store_path".to_owned(),
            asset
                .and_then(|asset| self.store_path(asset))
                .map_or(Value::Null, |path| json!(path)),
        );
        fields.insert(
            "mix_basis_points".to_owned(),
            json!(resolved.mix_basis_points),
        );
        fields.insert(
            "input_encoding".to_owned(),
            json!(input_encoding_name(resolved.input_encoding_token)),
        );
        fields.insert(
            "input_encoding_token".to_owned(),
            json!(resolved.input_encoding_token),
        );
        // Availability is evaluated conservatively for reporting exactly as it
        // is for export preflight: a node counts as active unless no stored or
        // keyframed value could make it evaluate (CC4 §2.3, §3.6).
        fields.insert("may_be_active".to_owned(), json!(may_be_active));
        fields
    }
}

/// One `lut_asset_id` as the `i64` the descriptor and the manifest use.
fn lut_asset_id_as_i64(id: LutAssetId) -> i64 {
    i64::try_from(id.0).unwrap_or(i64::MAX)
}

/// The stable `input_encoding` token name (CC4 §3.4).
#[must_use]
pub(crate) const fn input_encoding_name(token: i64) -> &'static str {
    match token {
        1 => "linear",
        2 => "grade709",
        _ => "display709",
    }
}

/// The one-line encoding legend both planners and the schema reuse.
#[must_use]
pub(crate) const fn input_encoding_legend() -> &'static str {
    "input_encoding_token: 0 display709, 1 linear, 2 grade709"
}

/// One structured LUT asset-identity warning on a node manifest entry.
fn lut_node_warning(
    code: &str,
    effect: &Effect,
    effect_index: usize,
    lut_asset: LutAssetId,
    blocking: bool,
    detail: &str,
) -> Value {
    json!({
        "code": code,
        "effect_id": effect.id.0,
        "stage_index": effect_index,
        "lut_asset_id": lut_asset.0,
        // An asset referenced only by nodes that can never evaluate does not
        // block proof or export; the status is still reported (CC4 §2.3).
        "blocking": blocking,
        "message": format!("effect {} references lut_asset_id {}: {detail}", effect.id, lut_asset.0),
        "recovery_action": "Call list_look_assets for the registered assets, then import_lut_asset or restore the store file before rendering or exporting.",
    })
}

/// The read-only `list_look_assets` payload (CC4 §8).
///
/// Compact by contract: no samples, and no domain data beyond the two integer
/// triples the record already carries.
#[must_use]
pub(crate) fn look_assets_value(
    revision: TimelineRevision,
    document: &Document,
    looks: &LookAssetContext,
) -> Value {
    let builtin = kinewright_media::BuiltinLook::ALL
        .iter()
        .map(|look| {
            let (domain_min, domain_max) = (look.domain().0, look.domain().1);
            json!({
                "name": look.name(),
                "title": look.title(),
                "size": look.size(),
                "sha256": look.pinned_sha256(),
                "byte_len": look.byte_len(),
                "domain_min": domain_min,
                "domain_max": domain_max,
                "preset_token": kinewright_media::BuiltinLook::ALL
                    .iter()
                    .position(|candidate| candidate == look),
            })
        })
        .collect::<Vec<_>>();
    let assets = document
        .lut_assets
        .iter()
        .map(|asset| {
            let mut summary = looks.asset_summary(asset);
            if let Some(object) = summary.as_object_mut() {
                object.insert(
                    "referenced_by".to_owned(),
                    json!(
                        document
                            .lut_asset_references(asset.id)
                            .into_iter()
                            .map(|(clip, effect)| json!({"clip_id": clip.0, "effect_id": effect.0}))
                            .collect::<Vec<_>>()
                    ),
                );
            }
            summary
        })
        .collect::<Vec<_>>();
    json!({
        "timeline_revision": revision.0,
        "store_root": looks.store_root(),
        "store_root_known": looks.store_root().is_some(),
        "builtin": builtin,
        "assets": assets,
        "asset_count": document.lut_assets.len(),
        "lut_node_limit_per_layer": LUT_NODE_LIMIT_PER_LAYER,
        "input_encoding_legend": input_encoding_legend(),
        "evidence_only": true,
        "applied": false,
        "next": "Bind an asset with plan_technical_lut or plan_creative_look, then submit the returned operations through prepare_edit_plan.",
    })
}

/// Arguments for the evidence-only CC4 LUT-node planners (CC4 §8).
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct LutNodePlanArgs {
    /// Exact timeline revision returned by the preceding inspection.
    pub expected_revision: TimelineRevision,
    /// Stable visual media clip id to receive the LUT node.
    pub clip_id: ClipId,
    /// Project LUT asset id to bind. Call `list_look_assets` for the
    /// registered ids; `import_lut_asset` registers a new one.
    pub lut_asset_id: LutAssetId,
    /// Look strength in basis points. `creative_look` accepts 0..=10000;
    /// `technical_lut` pins it at 10000.
    #[serde(default)]
    pub mix_basis_points: Option<i64>,
    /// 0 display709, 1 linear, 2 grade709.
    #[serde(default)]
    pub input_encoding_token: Option<i64>,
    /// Explicit `bypass` token for the node, `0` or `1`.
    #[serde(default)]
    pub bypass: Option<i64>,
    /// Stack a second node of this kind instead of retargeting the clip's
    /// existing one in place.
    #[serde(default)]
    pub append: bool,
    /// Optional explicit D65 assumption for a complete BT.709 source whose
    /// raw white point is unknown.
    #[serde(default)]
    pub profile_assumption: Option<ColorSourceProfileAssumption>,
}

/// The first index of `effects` at which a node of `kind` satisfies the CC4
/// §3.2 stage rule.
///
/// Stated constructively so the answer is both legal and adjacent to the
/// colour stack rather than merely legal:
///
/// * a `technical_lut` must precede every correction and look node, so it goes
///   at the first managed node of a higher stage;
/// * a `creative_look` must follow every technical and correction node, so it
///   goes one past the last managed node of a lower stage;
/// * when no such node exists the index is `effects.len()`, which appends and
///   leaves every unrelated effect's relative order untouched.
#[must_use]
pub(crate) fn stage_insert_index(effects: &[Effect], kind: ColorNodeKind) -> usize {
    let rank = kind.stage().rank();
    match kind.stage() {
        ColorStage::Input | ColorStage::Correction => effects
            .iter()
            .position(|effect| {
                classify_color_node(effect).is_some_and(|node| node.stage().rank() > rank)
            })
            .unwrap_or(effects.len()),
        ColorStage::Look => effects
            .iter()
            .rposition(|effect| {
                classify_color_node(effect).is_some_and(|node| node.stage().rank() < rank)
            })
            .map_or(effects.len(), |index| index.saturating_add(1)),
    }
}

/// Validate and construct an unapplied `technical_lut` proposal (CC4 §8).
///
/// Nothing is sent to Core: the operations are proved valid against a clone of
/// the analyzed document and returned for the caller to submit through
/// `prepare_edit_plan`.
///
/// # Errors
///
/// Returns the first CC4 §8 rejection: a stale revision, an unusable clip, an
/// unregistered `lut_asset_id`, an out-of-range control, or the per-layer LUT
/// node limit.
pub(crate) fn plan_technical_lut(
    document: &Document,
    actual_revision: TimelineRevision,
    args: &LutNodePlanArgs,
    looks: &LookAssetContext,
) -> Result<ColorNodePlan, ColorNodePlanError> {
    plan_lut_node(
        document,
        actual_revision,
        ColorNodeKind::TechnicalLut,
        args,
        looks,
    )
}

/// Validate and construct an unapplied `creative_look` proposal (CC4 §8).
///
/// # Errors
///
/// See [`plan_technical_lut`].
pub(crate) fn plan_creative_look(
    document: &Document,
    actual_revision: TimelineRevision,
    args: &LutNodePlanArgs,
    looks: &LookAssetContext,
) -> Result<ColorNodePlan, ColorNodePlanError> {
    plan_lut_node(
        document,
        actual_revision,
        ColorNodeKind::CreativeLook,
        args,
        looks,
    )
}

/// The shared body of both CC4 planners. The two kinds are mathematically
/// identical and differ only in stage, role, and mix bounds (CC4 §3.1).
#[allow(clippy::too_many_lines)]
fn plan_lut_node(
    document: &Document,
    actual_revision: TimelineRevision,
    kind: ColorNodeKind,
    args: &LutNodePlanArgs,
    looks: &LookAssetContext,
) -> Result<ColorNodePlan, ColorNodePlanError> {
    if args.expected_revision != actual_revision {
        return Err(ColorNodePlanError::RevisionConflict {
            expected: args.expected_revision,
            actual: actual_revision,
        });
    }
    let effect_name = kind.effect_name();
    let (clip, source_profile, profile_assumption) =
        managed_color_clip(document, args.clip_id, args.profile_assumption)?;
    let Some(descriptor) = effect_descriptor(effect_name) else {
        return Err(ColorNodePlanError::MissingDescriptor(effect_name));
    };

    // The asset must already be registered: `validate_document` forbids a
    // dangling reference, so a plan that named an unknown id could never be
    // committed (CC4 §2.7).
    let Some(asset) = looks.asset(args.lut_asset_id) else {
        return Err(ColorNodePlanError::UnknownLutAsset {
            clip: args.clip_id,
            lut_asset: args.lut_asset_id,
            allowed: looks.asset_ids(),
        });
    };

    let mut requested_parameters = BTreeMap::new();
    requested_parameters.insert(
        LUT_ASSET_ID_PARAMETER.to_owned(),
        lut_asset_id_as_i64(args.lut_asset_id),
    );
    if let Some(mix) = args.mix_basis_points {
        requested_parameters.insert(LUT_MIX_PARAMETER.to_owned(), mix);
    }
    if let Some(token) = args.input_encoding_token {
        requested_parameters.insert(LUT_INPUT_ENCODING_PARAMETER.to_owned(), token);
    }
    if let Some(bypass) = args.bypass {
        requested_parameters.insert(COLOR_NODE_BYPASS_PARAMETER.to_owned(), bypass);
    }
    for (name, value) in &requested_parameters {
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
    let existing_lut_node_count = lut_node_count(&clip.effects);
    let existing = existing_color_node(document, args.clip_id, kind);
    let existing_nodes_of_kind = existing.as_ref().map_or(0, |node| node.node_count);
    let target = if args.append { None } else { existing.as_ref() };
    let target_effect = target.map(|node| node.effect);
    let insert_index = stage_insert_index(&clip.effects, kind);

    let mut warnings = Vec::new();
    let mut assumptions = vec![
        format!(
            "Omitted {effect_name} controls resolve to their descriptor neutrals; {}.",
            input_encoding_legend()
        ),
        "The lattice bytes are the rendering authority; the record's size and domain mirrors are informational (CC4 §2.1).".to_owned(),
    ];
    match looks.availability_kind(args.lut_asset_id) {
        Some(LutAvailabilityKind::Verified) => {}
        Some(kind) => warnings.push(format!(
            "lut_asset {} ({}) is {}; managed proof and export stay blocked until the store bytes hash to {}",
            asset.id,
            asset.title,
            match kind {
                LutAvailabilityKind::Missing => "missing",
                LutAvailabilityKind::Changed => "changed",
                LutAvailabilityKind::Unreadable => "unreadable",
                LutAvailabilityKind::Verified => "verified",
            },
            asset.sha256,
        )),
        None => assumptions.push(
            "No LUT store root is published to this session, so the asset's bytes were not hash-verified while planning."
                .to_owned(),
        ),
    }
    if let Some(node) = target
        && node.node_count > 1
    {
        warnings.push(format!(
            "clip {} already carries {} {effect_name} nodes; this proposal targets the last node ({}) in compositor evaluation order",
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
    .filter(|name| requested_parameters.contains_key(name))
    {
        warnings.push(format!(
            "clip {} node {} keyframes {name}; this proposal writes the static value, which automation overrides at render time",
            args.clip_id,
            target_effect.map_or(0, |effect| effect.id.0)
        ));
    }
    if args.append && existing_nodes_of_kind > 0 {
        assumptions.push(format!(
            "append=true stacks a new {effect_name} node after the clip's existing {existing_nodes_of_kind}; ordered nodes compose serially and are not merged (CC4 §3.2)."
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
    for (name, value) in &requested_parameters {
        resolved_parameters.insert(name.clone(), *value);
    }
    retain_requested_matte_parameters(&mut resolved_parameters, &requested_parameters);

    let changed = requested_parameters
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
        if existing_color_node_count >= COLOR_NODE_LIMIT_PER_LAYER {
            return Err(ColorNodePlanError::TooManyColorNodes {
                clip: args.clip_id,
                actual: existing_color_node_count + 1,
                limit: COLOR_NODE_LIMIT_PER_LAYER,
            });
        }
        if existing_lut_node_count >= LUT_NODE_LIMIT_PER_LAYER {
            return Err(ColorNodePlanError::TooManyLutNodes {
                clip: args.clip_id,
                actual: existing_lut_node_count + 1,
                limit: LUT_NODE_LIMIT_PER_LAYER,
            });
        }
        let effect_id = next_effect_id(document).ok_or(ColorNodePlanError::EffectIdExhausted)?;
        // CC3 §2.4: a new node stores only the controls the caller moved.
        // `lut_asset_id` is the exception, because its neutral `0` is the
        // unbound state `validate_document` rejects (CC4 §3.3).
        let parameters = requested_parameters
            .iter()
            .filter(|(name, value)| {
                name.as_str() == LUT_ASSET_ID_PARAMETER
                    || descriptor
                        .parameter(name)
                        .is_none_or(|parameter| parameter.neutral != **value)
            })
            .map(|(name, value)| (name.clone(), ParamValue::Integer(*value)))
            .collect::<BTreeMap<_, _>>();
        let operations = vec![Operation::InsertEffect {
            clip: args.clip_id,
            index: insert_index,
            effect: Effect {
                id: effect_id,
                name: effect_name.to_owned(),
                parameters,
                keyframes: BTreeMap::new(),
            },
        }];
        (effect_id, true, operations)
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
        requested_curves: BTreeMap::new(),
        resolved_curves: BTreeMap::new(),
        operations,
        existing_color_node_count,
        existing_nodes_of_kind,
        warnings,
        assumptions,
        no_change,
        insert_index: Some(insert_index),
        lut_asset: Some(looks.asset_summary(asset)),
        matte: None,
        predicted_coverage: None,
        sample_evidence: None,
    })
}

// ---------------------------------------------------------------------------
// CC5 §7 — `plan_secondary_correction`
// ---------------------------------------------------------------------------

/// One ergonomic geometric window in a `plan_secondary_correction` request.
///
/// Every field is optional and falls back to the descriptor neutral, so a
/// caller who only wants "an ellipse over the face" sends four numbers rather
/// than eight (CC5 §7).
#[derive(Debug, Clone, Default, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct MatteWindowRequest {
    /// `rect` or `ellipse`, or the raw `1`/`2` token.
    #[serde(default)]
    pub shape: Option<String>,
    /// Centre X in basis points of the frame width.
    #[serde(default, alias = "center_x_basis_points")]
    pub center_x: Option<i64>,
    /// Centre Y in basis points of the frame height.
    #[serde(default, alias = "center_y_basis_points")]
    pub center_y: Option<i64>,
    #[serde(default, alias = "half_width_basis_points")]
    pub half_width: Option<i64>,
    #[serde(default, alias = "half_height_basis_points")]
    pub half_height: Option<i64>,
    #[serde(default, alias = "rotation_centidegrees")]
    pub rotation: Option<i64>,
    #[serde(default, alias = "feather_basis_points")]
    pub feather: Option<i64>,
    /// Complement this window only, before the combine.
    #[serde(default)]
    pub invert: Option<bool>,
}

/// The ergonomic HSL qualifier of a `plan_secondary_correction` request.
#[derive(Debug, Clone, Default, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct MatteQualifierRequest {
    /// Omit to enable the qualifier whenever any band is named.
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default, alias = "hue_center_centidegrees")]
    pub hue_center: Option<i64>,
    #[serde(default, alias = "hue_width_centidegrees")]
    pub hue_width: Option<i64>,
    #[serde(default, alias = "hue_softness_centidegrees")]
    pub hue_softness: Option<i64>,
    #[serde(default, alias = "saturation_low_basis_points")]
    pub saturation_low: Option<i64>,
    #[serde(default, alias = "saturation_high_basis_points")]
    pub saturation_high: Option<i64>,
    #[serde(default, alias = "saturation_softness_basis_points")]
    pub saturation_softness: Option<i64>,
    #[serde(default, alias = "luma_low_basis_points")]
    pub luma_low: Option<i64>,
    #[serde(default, alias = "luma_high_basis_points")]
    pub luma_high: Option<i64>,
    #[serde(default, alias = "luma_softness_basis_points")]
    pub luma_softness: Option<i64>,
}

/// Arguments for the evidence-only CC5 secondary planner (CC5 §7).
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct SecondaryCorrectionPlanArgs {
    /// Exact timeline revision returned by the preceding inspection.
    pub expected_revision: TimelineRevision,
    /// Stable visual media clip id carrying the correction node.
    pub clip_id: ClipId,
    /// Attach the matte to this exact stored colour node. Mutually exclusive
    /// with `node_kind`.
    #[serde(default)]
    pub target_effect_id: Option<EffectId>,
    /// One of `primary_correction`, `color_wheels`, `color_curves`, or
    /// `creative_look`. `technical_lut` carries no matte (CC5 §2.1).
    #[serde(default)]
    pub node_kind: Option<String>,
    /// Stack a new node of `node_kind` instead of matting the clip's existing
    /// one in place.
    #[serde(default)]
    pub append: bool,
    /// Up to four geometric windows.
    #[serde(default)]
    pub windows: Option<Vec<MatteWindowRequest>>,
    /// The HSL qualifier leg.
    #[serde(default)]
    pub qualifier: Option<MatteQualifierRequest>,
    /// `union` (default) or `intersection`.
    #[serde(default)]
    pub combine: Option<String>,
    /// Complement the combined coverage before the mix.
    #[serde(default)]
    pub invert: Option<bool>,
    /// Scales the final coverage, `0..=10000`. `0` makes the node inactive.
    #[serde(default)]
    pub mix_basis_points: Option<i64>,
    /// Exact project frame at which `predicted_coverage` and `sample_roi` are
    /// measured. Defaults to the clip's midpoint.
    #[serde(default, alias = "frame")]
    pub timecode: Option<TimeCode>,
    /// Measure the hue/saturation/luma statistics of this normalized region as
    /// evidence.
    #[serde(default)]
    pub sample_roi: Option<MatteSampleRoi>,
    /// Also propose a qualifier from `sample_roi` by CC5 §7's pinned formula.
    /// Ignored without `sample_roi`.
    #[serde(default)]
    pub derive_qualifier_from_sample: bool,
    /// Optional explicit D65 assumption for a complete BT.709 source whose raw
    /// white point is unknown.
    #[serde(default)]
    pub profile_assumption: Option<ColorSourceProfileAssumption>,
}

/// A normalized `0..=1` sample region, in the CC2 ROI shape.
#[derive(Debug, Clone, Copy, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct MatteSampleRoi {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// CC5 §7's pinned qualifier-derivation constants.
///
/// Named rather than inlined so the fixture asserts the constant, not a
/// literal, and so the formula in the response cannot drift from the code.
pub(crate) const MATTE_SAMPLE_HUE_WIDTH_CENTIDEGREES: i64 = 1_500;
/// See [`MATTE_SAMPLE_HUE_WIDTH_CENTIDEGREES`].
pub(crate) const MATTE_SAMPLE_SOFTNESS: i64 = 1_000;
/// The band margin CC5 §7 adds outside the sampled p10/p90 percentiles.
pub(crate) const MATTE_SAMPLE_BAND_MARGIN_BASIS_POINTS: i64 = 1_000;

/// The stable reason token published when the analysis backend cannot render a
/// matte proof, so `predicted_coverage` is `null` rather than invented.
pub(crate) const MATTE_PROOF_UNAVAILABLE: &str = "matte_proof_unavailable";

/// The CC5 §5.2 tracking-boundary statement, and CC5 §7's evidence posture.
pub(crate) const MATTE_PLAN_BOUNDARY: &str = "plan_secondary_correction is request-driven and evidence-only: it plans exactly the windows and qualifier bands it was given, and performs no subject, face, skin, or object detection, segmentation, or ML matting (CC5 §11). derive_qualifier_from_sample is a colour picker on an explicitly supplied region, evaluated by the pinned CC5 §7 formula.";

/// Resolve the `combine` token, rejecting anything outside the vocabulary.
fn matte_combine_token(combine: Option<&str>) -> Result<Option<i64>, ColorNodePlanError> {
    match combine {
        None => Ok(None),
        Some("union") => Ok(Some(0)),
        Some("intersection" | "intersect") => Ok(Some(1)),
        Some(other) => Err(ColorNodePlanError::MatteTokenNotRecognized {
            field: "combine",
            observed: other.to_owned(),
            allowed: &["union", "intersection"],
        }),
    }
}

/// Resolve a window `shape` token, accepting the words and the raw integers.
fn matte_shape_token(shape: Option<&str>, index: usize) -> Result<Option<i64>, ColorNodePlanError> {
    match shape {
        None => Ok(None),
        Some("rect" | "rectangle" | "1") => Ok(Some(1)),
        Some("ellipse" | "2") => Ok(Some(2)),
        Some(other) => Err(ColorNodePlanError::MatteWindowTokenNotRecognized {
            index,
            field: "shape",
            observed: other.to_owned(),
            allowed: &["rect", "ellipse"],
        }),
    }
}

/// Expand a validated request into the exact `matte_*` integers it names.
///
/// Only parameters the caller actually named are produced: an omitted control
/// resolves to its descriptor neutral (CC5 §2.2), so writing the other 40
/// would store values nobody asked for and would make a matte-free node
/// textually different for no rendered effect.
fn matte_request_parameters(
    args: &SecondaryCorrectionPlanArgs,
    derived_qualifier: Option<&BTreeMap<String, i64>>,
) -> Result<BTreeMap<String, i64>, ColorNodePlanError> {
    let mut parameters = BTreeMap::new();
    // Any CC5 request is a request for a matte, so the master switch is always
    // part of the proposal. Without it every other value would be stored and
    // ignored, which is the one failure a 47-integer expansion must not have.
    parameters.insert("matte_enabled".to_owned(), 1);

    if let Some(windows) = &args.windows {
        if windows.len() > MATTE_WINDOW_LIMIT {
            return Err(ColorNodePlanError::MatteWindowCount {
                observed: windows.len(),
                max: MATTE_WINDOW_LIMIT,
            });
        }
        parameters.insert(
            "matte_window_count".to_owned(),
            i64::try_from(windows.len()).unwrap_or(0),
        );
        for (index, window) in windows.iter().enumerate() {
            let Some(names) = kinewright_core::matte_window_parameter_names(index) else {
                return Err(ColorNodePlanError::MatteWindowCount {
                    observed: windows.len(),
                    max: MATTE_WINDOW_LIMIT,
                });
            };
            let mut set = |suffix: &str, value: Option<i64>| {
                if let Some(value) = value
                    && let Some(name) = names.iter().find(|name| name.ends_with(suffix))
                {
                    parameters.insert((*name).to_owned(), value);
                }
            };
            set(
                "_shape_token",
                matte_shape_token(window.shape.as_deref(), index)?,
            );
            set("_center_x_basis_points", window.center_x);
            set("_center_y_basis_points", window.center_y);
            set("_half_width_basis_points", window.half_width);
            set("_half_height_basis_points", window.half_height);
            set("_rotation_centidegrees", window.rotation);
            set("_feather_basis_points", window.feather);
            set("_invert", window.invert.map(i64::from));
        }
    }

    if let Some(token) = matte_combine_token(args.combine.as_deref())? {
        parameters.insert("matte_combine_token".to_owned(), token);
    }
    if let Some(invert) = args.invert {
        parameters.insert("matte_invert".to_owned(), i64::from(invert));
    }
    if let Some(mix) = args.mix_basis_points {
        parameters.insert("matte_mix_basis_points".to_owned(), mix);
    }

    if let Some(qualifier) = &args.qualifier {
        let mut named = BTreeMap::new();
        let mut set = |name: &str, value: Option<i64>| {
            if let Some(value) = value {
                named.insert(name.to_owned(), value);
            }
        };
        set("matte_hue_center_centidegrees", qualifier.hue_center);
        set("matte_hue_width_centidegrees", qualifier.hue_width);
        set("matte_hue_softness_centidegrees", qualifier.hue_softness);
        set(
            "matte_saturation_low_basis_points",
            qualifier.saturation_low,
        );
        set(
            "matte_saturation_high_basis_points",
            qualifier.saturation_high,
        );
        set(
            "matte_saturation_softness_basis_points",
            qualifier.saturation_softness,
        );
        set("matte_luma_low_basis_points", qualifier.luma_low);
        set("matte_luma_high_basis_points", qualifier.luma_high);
        set("matte_luma_softness_basis_points", qualifier.luma_softness);
        // An explicit `enabled` always wins; otherwise naming any band — here
        // or by asking for one to be derived from a sample — is the request to
        // enable the leg, because a band nobody evaluates is not a state a
        // caller can have meant.
        let enabled = qualifier
            .enabled
            .unwrap_or(!named.is_empty() || derived_qualifier.is_some());
        parameters.insert("matte_qualifier_enabled".to_owned(), i64::from(enabled));
        parameters.extend(named);
    }

    if let Some(derived) = derived_qualifier {
        // Same rule as the bands below: an explicit `qualifier.enabled: false`
        // is a request, and a derived sample is only evidence, so the derived
        // enable never overrides it. Absent an explicit qualifier there is
        // nothing to beat and the derived leg turns itself on.
        parameters
            .entry("matte_qualifier_enabled".to_owned())
            .or_insert(1);
        for (name, value) in derived {
            // An explicit qualifier field always beats a derived one: the
            // caller's number is a request, the sample is only evidence.
            parameters.entry(name.clone()).or_insert(*value);
        }
    }

    Ok(parameters)
}

/// Validate and construct an unapplied CC5 secondary proposal (CC5 §7).
///
/// Nothing is sent to Core: every operation is proved valid against a clone of
/// the analyzed document and returned for the caller to submit through
/// `prepare_edit_plan`.  `predicted_coverage` is measured on a second scratch
/// clone, so neither the live document nor the analyzed snapshot is touched.
///
/// # Errors
///
/// Returns the first CC5 §7 rejection: a stale revision, an unusable clip, an
/// ambiguous or absent target, a `technical_lut` target, more than four
/// windows, an out-of-range control, or a Hold-only matte token the target
/// node already keyframes.
#[allow(clippy::too_many_lines)]
pub(crate) fn plan_secondary_correction(
    document: &Document,
    actual_revision: TimelineRevision,
    analysis: &dyn kinewright_core::Analysis,
    args: &SecondaryCorrectionPlanArgs,
) -> Result<ColorNodePlan, ColorNodePlanError> {
    if args.expected_revision != actual_revision {
        return Err(ColorNodePlanError::RevisionConflict {
            expected: args.expected_revision,
            actual: actual_revision,
        });
    }
    let (clip, source_profile, profile_assumption) =
        managed_color_clip(document, args.clip_id, args.profile_assumption)?;

    // ------------------------------------------------------------------
    // Target resolution, before any operation exists.
    // ------------------------------------------------------------------
    if args.target_effect_id.is_some() && args.node_kind.is_some() {
        return Err(ColorNodePlanError::MatteTargetAmbiguous);
    }
    let (kind, target_effect) = match (args.target_effect_id, args.node_kind.as_deref()) {
        (Some(effect_id), _) => {
            let Some(effect) = clip.effects.iter().find(|effect| effect.id == effect_id) else {
                return Err(ColorNodePlanError::MatteTargetNotFound {
                    clip: args.clip_id,
                    effect: effect_id,
                });
            };
            let Some(kind) = classify_color_node(effect) else {
                return Err(ColorNodePlanError::MatteTargetNotAColorNode {
                    effect: effect_id,
                    name: effect.name.clone(),
                });
            };
            if !kind.supports_matte() {
                return Err(ColorNodePlanError::MatteUnsupportedKind {
                    observed: kind.effect_name(),
                });
            }
            (kind, Some(effect))
        }
        (None, Some(name)) => {
            let Some(kind) = ColorNodeKind::from_effect_name(name) else {
                return Err(ColorNodePlanError::MatteUnknownKind {
                    observed: name.to_owned(),
                });
            };
            if !kind.supports_matte() {
                return Err(ColorNodePlanError::MatteUnsupportedKind {
                    observed: kind.effect_name(),
                });
            }
            let existing = if args.append {
                None
            } else {
                existing_color_node(document, args.clip_id, kind).map(|node| node.effect)
            };
            (kind, existing)
        }
        (None, None) => return Err(ColorNodePlanError::MatteTargetRequired),
    };
    let effect_name = kind.effect_name();
    let Some(descriptor) = effect_descriptor(effect_name) else {
        return Err(ColorNodePlanError::MissingDescriptor(effect_name));
    };

    // ------------------------------------------------------------------
    // Evidence: the sample ROI, and the qualifier it may derive.
    // ------------------------------------------------------------------
    let measured_at = match args.timecode {
        Some(frame) => frame,
        None => matte_plan_default_frame(document, clip)?,
    };
    let sample = match &args.sample_roi {
        None => None,
        Some(roi) => Some(measure_matte_sample_roi(
            document,
            analysis,
            measured_at,
            *roi,
        )?),
    };
    let derived_qualifier = match (&sample, args.derive_qualifier_from_sample) {
        (Some(sample), true) => Some(sample.derived_qualifier()),
        _ => None,
    };

    // ------------------------------------------------------------------
    // Expand and validate every requested integer against the descriptor.
    // ------------------------------------------------------------------
    let requested_parameters = matte_request_parameters(args, derived_qualifier.as_ref())?;
    for (name, value) in &requested_parameters {
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

    // ------------------------------------------------------------------
    // Operations. Nothing above this line constructed one.
    // ------------------------------------------------------------------
    let existing_color_node_count = managed_color_node_count(&clip.effects);
    let existing_nodes_of_kind = clip
        .effects
        .iter()
        .filter(|effect| classify_color_node(effect) == Some(kind))
        .count();

    let mut warnings = Vec::new();
    let mut assumptions = vec![
        "Omitted matte controls resolve to their descriptor neutrals; a node whose matte is not enabled is byte-identical to its CC4 self (CC5 §2.6, §8).".to_owned(),
        MATTE_PLAN_BOUNDARY.to_owned(),
    ];
    if args.append && existing_nodes_of_kind > 0 {
        assumptions.push(format!(
            "append=true stacks a new {effect_name} node after the clip's existing {existing_nodes_of_kind}; ordered nodes compose serially and are not merged (CC3 §3.1)."
        ));
    }
    if target_effect.is_some_and(|effect| MatteParams::from_effect(effect).has_matte()) {
        assumptions.push(
            "The targeted node already carries a matte; omitted controls keep their stored values rather than resetting to neutral (CC5 §2.2)."
                .to_owned(),
        );
    }
    for name in keyframed_parameters(
        target_effect,
        &requested_parameters
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    ) {
        warnings.push(format!(
            "clip {} node {} keyframes {name}; this proposal writes the static value, which automation overrides at render time",
            args.clip_id,
            target_effect.map_or(0, |effect| effect.id.0)
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
    for (name, value) in &requested_parameters {
        resolved_parameters.insert(name.clone(), *value);
    }
    retain_requested_matte_parameters(&mut resolved_parameters, &requested_parameters);

    let changed = requested_parameters
        .iter()
        .filter(|(name, value)| current.get(*name) != Some(*value))
        .map(|(name, value)| (name.clone(), *value))
        .collect::<Vec<_>>();

    // CC5 §5.1: a Hold-only token that already carries automation is written by
    // that automation at every frame from its first keyframe onward, so a
    // static write is guaranteed dead. Every other planner warns about a
    // keyframed control and writes the static value anyway; here that would be
    // a lie, so the plan refuses and names the recovery.
    //
    // Every requested token is checked, not only the ones in `changed`: the
    // plan's manifest and `predicted_coverage` both describe the requested
    // value, and a Hold curve that disagrees with it renders something else
    // even when the stored static already matches (so nothing is written).
    // Every CC5 request injects `matte_enabled: 1`, so a curve that already
    // holds exactly the requested value at every keyframe is accepted — it
    // renders exactly what the plan claims — which keeps a node with a
    // `matte_enabled` Hold curve plannable (window slides and the like).
    for (name, value) in &requested_parameters {
        if !kinewright_core::is_hold_only_matte_parameter(name) {
            continue;
        }
        let overridden = target_effect.is_some_and(|effect| {
            effect.keyframes.get(name).is_some_and(|curve| {
                curve
                    .keyframes
                    .iter()
                    .any(|keyframe| keyframe.value != *value)
            })
        });
        if overridden {
            return Err(ColorNodePlanError::MatteHoldOnlyParameterKeyframed {
                effect: target_effect.map_or(EffectId(0), |effect| effect.id),
                name: name.clone(),
            });
        }
    }

    let mut insert_index = None;
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
        // CC5 §2.2: a fresh node stores only the values the caller moved off
        // their neutrals, exactly as CC3 §2.4 stores only the curve points that
        // exist.
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
            // CC4 §3.2: a new node is inserted at the first stage-legal index,
            // never appended, so an ordering rejection is unreachable through
            // the ordinary path.
            let index = stage_insert_index(&clip.effects, kind);
            insert_index = Some(index);
            vec![Operation::InsertEffect {
                clip: args.clip_id,
                index,
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
    let mut candidate = document.clone();
    if !operations.is_empty() {
        apply_batch(&mut candidate, &operations)
            .map_err(|error| ColorNodePlanError::CoreRejected(error.to_string()))?;
    }

    // The proposal's own matte, read back off the scratch document rather than
    // recomputed, so `matte` and `predicted_coverage` describe the same node.
    let planned_effect = candidate
        .clip(args.clip_id)
        .and_then(|clip| clip.effects.iter().find(|effect| effect.id == effect_id))
        .cloned();
    let matte = planned_effect
        .as_ref()
        .and_then(matte_manifest_value)
        .or(Some(Value::Null));
    let predicted_coverage = Some(predicted_matte_coverage(
        &candidate,
        analysis,
        measured_at,
        args.clip_id,
        effect_id,
        no_change,
    ));

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
        requested_curves: BTreeMap::new(),
        resolved_curves: BTreeMap::new(),
        operations,
        existing_color_node_count,
        existing_nodes_of_kind,
        warnings,
        assumptions,
        no_change,
        insert_index,
        lut_asset: None,
        matte,
        predicted_coverage,
        sample_evidence: sample.map(|sample| sample.value(measured_at, args)),
    })
}

/// The frame a secondary proposal measures when the caller names none.
fn matte_plan_default_frame(
    document: &Document,
    clip: &Clip,
) -> Result<TimeCode, ColorNodePlanError> {
    let duration = document
        .clip_duration(clip)
        .map_err(|error| ColorNodePlanError::CoreRejected(error.to_string()))?;
    let midpoint = clip
        .timeline_start
        .0
        .saturating_add(duration.0.max(1) / 2)
        .max(0);
    Ok(TimeCode(midpoint))
}

/// The CC5 §4.2 statistics of a proposal, measured on a scratch document.
///
/// Never invents a number: when the analysis backend cannot render a matte
/// proof — which is the ordinary state until the media engine lands
/// `matte_proof_for_document` — the value is `null` with the stable reason
/// [`MATTE_PROOF_UNAVAILABLE`] and the backend's own message.
fn predicted_matte_coverage(
    candidate: &Document,
    analysis: &dyn kinewright_core::Analysis,
    at: TimeCode,
    clip: ClipId,
    effect: EffectId,
    no_change: bool,
) -> Value {
    let unavailable = |reason: &str, detail: String| {
        json!({
            "statistics": Value::Null,
            "reason": reason,
            "message": detail,
            "project_frame": at.0,
            "recovery_action": "Commit the plan and call inspect_grade_matte, or retry once the analysis backend can render a matte proof; no coverage number is invented here.",
        })
    };
    if no_change {
        return unavailable(
            "matte_plan_changes_nothing",
            "the proposal writes no operation, so there is no proposed coverage to measure"
                .to_owned(),
        );
    }
    match analysis.matte_proof_for_document(Arc::new(candidate.clone()), at, clip, effect) {
        Err(error) => unavailable(MATTE_PROOF_UNAVAILABLE, error.to_string()),
        Ok(proof) => match kinewright_core::matte_coverage_statistics(&proof.coverage) {
            Err(error) => unavailable(error.code(), error.to_string()),
            Ok(statistics) => json!({
                "statistics": statistics,
                "reason": Value::Null,
                "project_frame": at.0,
                "raster": {"width": proof.coverage.width, "height": proof.coverage.height},
                "matte_threshold": kinewright_core::MATTE_SCOPE_THRESHOLD,
                "covered_pixel_count": statistics.covered_pixel_count,
                "coverage_encoding": proof.metadata.coverage_encoding,
                "coverage_scale": proof.metadata.coverage_scale,
                "measured_on": "scratch document carrying the unapplied proposal; the analyzed document is untouched",
            }),
        },
    }
}

/// Measured hue/saturation/luma statistics of one explicit sample region.
///
/// CC5 §7 calls this a colour picker on an explicitly supplied region, not
/// detection: it looks exactly where it was told, and the arithmetic below is
/// the document's.
#[derive(Debug, Clone)]
struct MatteSampleStatistics {
    /// Pixels inside the region whose alpha is non-zero (CC2's visible rule).
    visible_pixel_count: u64,
    total_pixel_count: u64,
    /// Median hue in hundredths of a degree over chromatic visible pixels.
    hue_median_centidegrees: Option<i64>,
    /// Visible pixels whose chroma is exactly zero, so their hue is undefined.
    achromatic_pixel_count: u64,
    saturation_p10_basis_points: i64,
    saturation_median_basis_points: i64,
    saturation_p90_basis_points: i64,
    luma_p10_basis_points: i64,
    luma_median_basis_points: i64,
    luma_p90_basis_points: i64,
    /// The pixel rectangle actually measured, after the CC2 ROI floor/ceil.
    pixel_rect: (u32, u32, u32, u32),
}

impl MatteSampleStatistics {
    /// CC5 §7's pinned derivation, and no other.
    fn derived_qualifier(&self) -> BTreeMap<String, i64> {
        let mut qualifier = BTreeMap::from([
            (
                "matte_saturation_low_basis_points".to_owned(),
                (self.saturation_p10_basis_points - MATTE_SAMPLE_BAND_MARGIN_BASIS_POINTS).max(0),
            ),
            (
                "matte_saturation_high_basis_points".to_owned(),
                (self.saturation_p90_basis_points + MATTE_SAMPLE_BAND_MARGIN_BASIS_POINTS)
                    .min(MATTE_MIX_BASIS_POINTS_MAX),
            ),
            (
                "matte_saturation_softness_basis_points".to_owned(),
                MATTE_SAMPLE_SOFTNESS,
            ),
            (
                "matte_luma_low_basis_points".to_owned(),
                (self.luma_p10_basis_points - MATTE_SAMPLE_BAND_MARGIN_BASIS_POINTS).max(0),
            ),
            (
                "matte_luma_high_basis_points".to_owned(),
                (self.luma_p90_basis_points + MATTE_SAMPLE_BAND_MARGIN_BASIS_POINTS)
                    .min(MATTE_MIX_BASIS_POINTS_MAX),
            ),
            (
                "matte_luma_softness_basis_points".to_owned(),
                MATTE_SAMPLE_SOFTNESS,
            ),
        ]);
        // CC5 §2.4: with no chromatic pixel the hue is undefined, so the hue
        // leg stays at its 180° neutral, which disables it rather than
        // selecting an arbitrary sector.
        if let Some(hue) = self.hue_median_centidegrees {
            qualifier.insert("matte_hue_center_centidegrees".to_owned(), hue);
            qualifier.insert(
                "matte_hue_width_centidegrees".to_owned(),
                MATTE_SAMPLE_HUE_WIDTH_CENTIDEGREES,
            );
            qualifier.insert(
                "matte_hue_softness_centidegrees".to_owned(),
                MATTE_SAMPLE_SOFTNESS,
            );
        }
        qualifier
    }

    fn value(&self, at: TimeCode, args: &SecondaryCorrectionPlanArgs) -> Value {
        let (x, y, width, height) = self.pixel_rect;
        json!({
            "project_frame": at.0,
            "requested_roi": args.sample_roi.map(|roi| json!({
                "x": roi.x, "y": roi.y, "width": roi.width, "height": roi.height,
            })),
            "measured_pixel_rect": {"x": x, "y": y, "width": width, "height": height},
            "visible_pixel_count": self.visible_pixel_count,
            "total_pixel_count": self.total_pixel_count,
            "achromatic_pixel_count": self.achromatic_pixel_count,
            "hue_median_centidegrees": self.hue_median_centidegrees,
            "saturation_basis_points": {
                "p10": self.saturation_p10_basis_points,
                "median": self.saturation_median_basis_points,
                "p90": self.saturation_p90_basis_points,
            },
            "luma_basis_points": {
                "p10": self.luma_p10_basis_points,
                "median": self.luma_median_basis_points,
                "p90": self.luma_p90_basis_points,
            },
            "derive_qualifier_from_sample": args.derive_qualifier_from_sample,
            "derived_qualifier": if args.derive_qualifier_from_sample {
                json!(self.derived_qualifier())
            } else {
                Value::Null
            },
            "formula": format!(
                "CC5 §7, pinned: hue_center = median hue of chromatic visible ROI pixels, hue_width = {MATTE_SAMPLE_HUE_WIDTH_CENTIDEGREES}, hue_softness = {MATTE_SAMPLE_SOFTNESS}, sat_low = max(0, p10 - {MATTE_SAMPLE_BAND_MARGIN_BASIS_POINTS}), sat_high = min({MATTE_MIX_BASIS_POINTS_MAX}, p90 + {MATTE_SAMPLE_BAND_MARGIN_BASIS_POINTS}), sat_softness = {MATTE_SAMPLE_SOFTNESS}, luma likewise",
            ),
            "measurement_basis": "the managed monitor proof at this frame, decoded through display709 and re-encoded to the CC5 §2.4 grade709 selector space. The qualifier renders on the value entering the node, so a sample taken on the monitored composite is evidence, not a prediction.",
            "evidence_only": true,
        })
    }
}

/// Measure one explicit normalized region of the managed monitor proof.
#[allow(clippy::too_many_lines)]
fn measure_matte_sample_roi(
    document: &Document,
    analysis: &dyn kinewright_core::Analysis,
    at: TimeCode,
    roi: MatteSampleRoi,
) -> Result<MatteSampleStatistics, ColorNodePlanError> {
    for (field, value) in [
        ("sample_roi.x", roi.x),
        ("sample_roi.y", roi.y),
        ("sample_roi.width", roi.width),
        ("sample_roi.height", roi.height),
    ] {
        if !value.is_finite() {
            return Err(ColorNodePlanError::MatteSampleRoiInvalid {
                field,
                observed: format!("{value}"),
            });
        }
    }
    if roi.width <= 0.0
        || roi.height <= 0.0
        || roi.x < 0.0
        || roi.y < 0.0
        || roi.x + roi.width > 1.0
        || roi.y + roi.height > 1.0
    {
        return Err(ColorNodePlanError::MatteSampleRoiInvalid {
            field: "sample_roi",
            observed: format!(
                "x={} y={} width={} height={}",
                roi.x, roi.y, roi.width, roi.height
            ),
        });
    }
    let proof = analysis
        .monitor_proof_for_document(Arc::new(document.clone()), at)
        .map_err(|error| ColorNodePlanError::MatteSampleRenderFailed {
            at,
            message: error.to_string(),
        })?;
    let image = &proof.image;
    if image.width == 0 || image.height == 0 {
        return Err(ColorNodePlanError::MatteSampleRenderFailed {
            at,
            message: format!("monitor proof raster is {}x{}", image.width, image.height),
        });
    }
    // CC2's ROI rule: a start boundary floors and an exclusive end ceils, so
    // the measured rectangle covers every pixel the caller named.
    let scale = |value: f64, extent: u32| (value * f64::from(extent)).max(0.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let left = (scale(roi.x, image.width).floor() as u32).min(image.width.saturating_sub(1));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let top = (scale(roi.y, image.height).floor() as u32).min(image.height.saturating_sub(1));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let right = (scale(roi.x + roi.width, image.width).ceil() as u32)
        .clamp(left.saturating_add(1), image.width);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let bottom = (scale(roi.y + roi.height, image.height).ceil() as u32)
        .clamp(top.saturating_add(1), image.height);

    let mut hues = Vec::new();
    let mut saturations = Vec::new();
    let mut lumas = Vec::new();
    let mut total_pixel_count = 0_u64;
    let mut achromatic_pixel_count = 0_u64;
    for y in top..bottom {
        for x in left..right {
            total_pixel_count = total_pixel_count.saturating_add(1);
            let index = ((u64::from(y) * u64::from(image.width)) + u64::from(x)).saturating_mul(4);
            let Ok(index) = usize::try_from(index) else {
                continue;
            };
            let Some(pixel) = image.pixels.get(index..index.saturating_add(4)) else {
                continue;
            };
            // CC2's rule: a fully transparent pixel is not part of the
            // population, and partial alpha is never a weight.
            if pixel[3] == 0 {
                continue;
            }
            let encoded = [0, 1, 2].map(|channel| {
                let display = f32::from(pixel[channel]) / 255.0;
                kinewright_media::color_pipeline::grade709_encode(
                    kinewright_media::color_pipeline::decode_display709(display),
                )
                .clamp(0.0, 1.0)
            });
            let maximum = encoded[0].max(encoded[1]).max(encoded[2]);
            let minimum = encoded[0].min(encoded[1]).min(encoded[2]);
            let chroma = maximum - minimum;
            let saturation = if maximum <= 0.0 {
                0.0
            } else {
                chroma / maximum
            };
            let luma = 0.2126 * encoded[0] + 0.7152 * encoded[1] + 0.0722 * encoded[2];
            saturations.push(basis_points_from_unit(saturation));
            lumas.push(basis_points_from_unit(luma));
            if chroma <= 0.0 {
                achromatic_pixel_count = achromatic_pixel_count.saturating_add(1);
            } else {
                // CC5 §2.4's branch order, written out so the sample and the
                // renderer agree on a tie. The comparisons are exact on
                // purpose: `maximum` *is* one of the three encoded values, by
                // construction, and CC5 §2.4 pins "the first matching branch in
                // that written order" so both implementations resolve a tie the
                // same way. An epsilon here would change which branch a tie
                // takes and make the agent disagree with the renderer.
                #[allow(clippy::float_cmp)]
                let degrees = if maximum == encoded[0] {
                    60.0 * (((encoded[1] - encoded[2]) / chroma).rem_euclid(6.0))
                } else if maximum == encoded[1] {
                    60.0 * (((encoded[2] - encoded[0]) / chroma) + 2.0)
                } else {
                    60.0 * (((encoded[0] - encoded[1]) / chroma) + 4.0)
                };
                #[allow(clippy::cast_possible_truncation)]
                let centidegrees = (f64::from(degrees) * 100.0).round() as i64;
                hues.push(centidegrees.rem_euclid(36_000));
            }
        }
    }

    let visible_pixel_count = u64::try_from(saturations.len()).unwrap_or(0);
    if visible_pixel_count == 0 {
        return Err(ColorNodePlanError::MatteSampleRoiEmpty {
            at,
            pixel_rect: (left, top, right - left, bottom - top),
        });
    }
    saturations.sort_unstable();
    lumas.sort_unstable();
    Ok(MatteSampleStatistics {
        visible_pixel_count,
        total_pixel_count,
        hue_median_centidegrees: circular_median_centidegrees(&mut hues),
        achromatic_pixel_count,
        saturation_p10_basis_points: percentile(&saturations, 10),
        saturation_median_basis_points: percentile(&saturations, 50),
        saturation_p90_basis_points: percentile(&saturations, 90),
        luma_p10_basis_points: percentile(&lumas, 10),
        luma_median_basis_points: percentile(&lumas, 50),
        luma_p90_basis_points: percentile(&lumas, 90),
        pixel_rect: (left, top, right - left, bottom - top),
    })
}

/// `round(value · 10000)` clamped into the qualifier's basis-point range.
fn basis_points_from_unit(value: f32) -> i64 {
    #[allow(clippy::cast_possible_truncation)]
    let rounded = (f64::from(value.clamp(0.0, 1.0)) * 10_000.0).round() as i64;
    rounded.clamp(0, MATTE_MIX_BASIS_POINTS_MAX)
}

/// The nearest-rank percentile of an ascending slice, `0..=100`.
///
/// Nearest-rank rather than an interpolated percentile: every input is already
/// an integer basis point, and interpolation would invent a value that no
/// measured pixel carries.
fn percentile(sorted: &[i64], percent: u64) -> i64 {
    if sorted.is_empty() {
        return 0;
    }
    let last = sorted.len().saturating_sub(1);
    let rank = u64::try_from(last).unwrap_or(0).saturating_mul(percent) / 100;
    sorted[usize::try_from(rank).unwrap_or(last).min(last)]
}

/// The median hue on the circle, in hundredths of a degree.
///
/// A plain median of `0..=35999` is wrong at the red seam — `35900` and `100`
/// are 2° apart but their arithmetic median is 18000, the opposite hue. This
/// returns the sample minimising the summed circular distance to every other
/// sample, which is a true circular median and needs no seam special case.
///
/// The minimiser is always one of the samples, so only the samples are scored.
/// Scoring each candidate against every other is O(n²) and this runs over
/// every chromatic pixel of a full-resolution ROI, so the sum is evaluated in
/// closed form instead: sort once, concatenate the sorted samples with
/// themselves shifted by one full turn so a window that crosses the seam stays
/// contiguous, take prefix sums, and sweep the `±18000` half-turn boundary
/// with a monotone pointer. For a candidate `c` the samples inside
/// `[c, c + 18000]` contribute `sum(h) − c·k` and the rest — which are nearer
/// the other way round the circle — contribute `(c + 36000)·k' − sum(h)`, both
/// read off the prefix sums in O(1). Sorting dominates, so the whole function
/// is O(n log n) and allocates one `2n + 1` prefix vector.
///
/// Ties keep the smallest sample, matching the ascending scan this replaced.
fn circular_median_centidegrees(hues: &mut [i64]) -> Option<i64> {
    if hues.is_empty() {
        return None;
    }
    hues.sort_unstable();
    let count = hues.len();
    // Prefix sums over `hues ++ (hues + 36000)`. A hue is at most 35999 and a
    // chromatic ROI has at most a few hundred million pixels, so the running
    // total cannot approach `i64::MAX`.
    let mut prefix = Vec::with_capacity(2 * count + 1);
    let mut total = 0_i64;
    prefix.push(total);
    for value in hues
        .iter()
        .copied()
        .chain(hues.iter().map(|hue| hue + 36_000))
    {
        total += value;
        prefix.push(total);
    }
    let doubled = |index: usize| -> i64 {
        if index < count {
            hues[index]
        } else {
            hues[index - count] + 36_000
        }
    };
    let as_i64 = |value: usize| i64::try_from(value).unwrap_or(i64::MAX);

    // The first index past the half-turn boundary. Both the boundary and the
    // window start advance with the candidate, so this never moves backwards
    // and the sweep is linear.
    let mut boundary = 0_usize;
    let mut best = (i64::MAX, hues[0]);
    for (index, candidate) in hues.iter().copied().enumerate() {
        boundary = boundary.max(index);
        while boundary < index + count && doubled(boundary) <= candidate + 18_000 {
            boundary += 1;
        }
        let near = as_i64(boundary - index);
        let far = as_i64(index + count - boundary);
        let forward = (prefix[boundary] - prefix[index]) - candidate * near;
        let backward = (candidate + 36_000) * far - (prefix[index + count] - prefix[boundary]);
        let cost = forward + backward;
        if cost < best.0 {
            best = (cost, candidate);
        }
    }
    Some(best.1)
}

// ---------------------------------------------------------------------------
// CC4 §9 — the explicit legacy conversion batch
// ---------------------------------------------------------------------------

/// What a legacy look at one effect position converts into (CC4 §9).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum LegacyLookConversion {
    /// A `look_lut` whose `preset_token` resolves to a built-in generated
    /// asset. The batch is complete and submittable as-is.
    Builtin {
        /// `[AddLutAsset, ConvertLegacyLook]`, or just `[ConvertLegacyLook]`
        /// when the built-in is already registered.
        operations: Vec<Operation>,
        /// The resolved built-in name from the CC4 §2.6 token table.
        builtin_name: &'static str,
        lut_asset: LutAssetId,
        /// `intensity_percent * 100`.
        mix_basis_points: i64,
        /// Whether an already-registered record with the pinned hash was
        /// reused instead of allocating a second identical asset.
        reused_existing_asset: bool,
    },
    /// A `cube_lut` whose external file must be imported into the project
    /// store before the batch can be built. Only `import_lut_asset` can do
    /// that, because only it can write the store (CC4 §8).
    NeedsImport {
        /// The legacy node's informational external path.
        path: String,
        mix_basis_points: i64,
    },
}

/// Why a legacy conversion could not be described.
#[derive(Debug, Error)]
pub(crate) enum LegacyLookConversionError {
    #[error("clip {0} does not exist")]
    MissingClip(ClipId),
    #[error("clip {clip} has no effect {effect}")]
    MissingEffect { clip: ClipId, effect: EffectId },
    #[error("effect {effect} is {name}, which is not a legacy look_lut or cube_lut")]
    NotALegacyLook { effect: EffectId, name: String },
    #[error("look_lut preset_token {observed} is outside the inclusive range 0..=4")]
    InvalidPresetToken { observed: i64 },
    #[error("cube_lut effect {effect} stores no external path")]
    MissingExternalPath { effect: EffectId },
    #[error("the project LUT asset id space is exhausted")]
    LutAssetIdExhausted,
}

impl LegacyLookConversionError {
    /// Stable machine-readable recovery code.
    #[must_use]
    pub(crate) const fn code(&self) -> &'static str {
        match self {
            Self::MissingClip(_) => "clip_not_found",
            Self::MissingEffect { .. } => "effect_not_found",
            Self::NotALegacyLook { .. } => "not_a_legacy_look",
            Self::InvalidPresetToken { .. } => "invalid_preset_token",
            Self::MissingExternalPath { .. } => "missing_external_lut_path",
            Self::LutAssetIdExhausted => "lut_asset_id_exhausted",
        }
    }

    /// The CC1/CC2 `field` this rejection is about.
    #[must_use]
    pub(crate) const fn field(&self) -> &'static str {
        match self {
            Self::MissingClip(_) => "clip_id",
            Self::MissingEffect { .. } | Self::NotALegacyLook { .. } => "effect_id",
            Self::InvalidPresetToken { .. } => "preset_token",
            Self::MissingExternalPath { .. } => "path",
            Self::LutAssetIdExhausted => "lut_asset_id",
        }
    }

    /// What was observed at [`Self::field`].
    #[must_use]
    pub(crate) fn observed(&self) -> Value {
        match self {
            Self::MissingClip(clip) => json!(clip.0),
            Self::MissingEffect { effect, .. } => json!(effect.0),
            Self::NotALegacyLook { name, .. } => json!(name),
            Self::InvalidPresetToken { observed } => json!(observed),
            Self::MissingExternalPath { .. } => Value::Null,
            Self::LutAssetIdExhausted => json!("exhausted"),
        }
    }

    /// What would have been accepted there.
    #[must_use]
    pub(crate) fn allowed(&self) -> Value {
        match self {
            Self::MissingClip(_) => json!("a visual clip id in this document"),
            Self::MissingEffect { .. } => json!("an effect id on the named clip"),
            Self::NotALegacyLook { .. } => json!(["look_lut", "cube_lut"]),
            Self::InvalidPresetToken { .. } => json!("an integer in the inclusive range 0..=4"),
            Self::MissingExternalPath { .. } => {
                json!("a cube_lut node carrying its external `path` text parameter")
            }
            Self::LutAssetIdExhausted => {
                json!(format!("1..={}", kinewright_core::LUT_ASSET_ID_MAX))
            }
        }
    }

    /// How the caller gets from here to a convertible node.
    #[must_use]
    pub(crate) const fn recovery_action(&self) -> &'static str {
        match self {
            Self::MissingClip(_) | Self::MissingEffect { .. } => {
                "Call get_color_context for this project's clips, their effect ids, and the legacy nodes that can be converted."
            }
            Self::NotALegacyLook { .. } => {
                "Only a legacy look_lut or cube_lut converts. A managed node is already managed; use plan_technical_lut or plan_creative_look to retarget it."
            }
            Self::InvalidPresetToken { .. } => {
                "Repair the legacy node's preset_token with SetEffectParam (0 identity, 1 warm, 2 cool, 3 monochrome, 4 bleach_bypass) before converting."
            }
            Self::MissingExternalPath { .. } => {
                "Give the legacy cube_lut node its external `path` with SetEffectParam, or delete the node: convert_legacy_look has no bytes to import without it."
            }
            Self::LutAssetIdExhausted => {
                "Remove unreferenced LUT asset records with RemoveLutAsset before converting another look."
            }
        }
    }
}

/// How a `ready` legacy conversion is actually submitted (CC4 §8, §9).
pub(crate) const LEGACY_LOOK_CONVERSION_RECOVERY_ACTION: &str = "Call convert_legacy_look with this clip_id and effect_id at the current timeline_revision; it registers the built-in and converts the node as one journaled, undoable batch. The operations are published for review only: AddLutAsset is not submittable through prepare_edit_plan or apply_edit_plan by design.";

/// How a `needs_import` legacy conversion is submitted.
pub(crate) const LEGACY_LOOK_CONVERSION_IMPORT_RECOVERY_ACTION: &str = "Call convert_legacy_look with this clip_id and effect_id at the current timeline_revision; it asks the operator to confirm before reading this path, imports it into the project LUT store, and converts the node in the same batch. A refused confirmation writes nothing.";

/// The CC4 §9 conversion evidence for every legacy look in one document.
///
/// Read-only: it describes the batch a caller would submit, and never applies
/// one. A `cube_lut` reports `needs_import` naming the external path, because
/// its bytes must reach the project store before the node can be converted;
/// `convert_legacy_look` performs that import behind a confirmation.
#[must_use]
pub(crate) fn legacy_look_conversions_value(document: &Document) -> Vec<Value> {
    let mut conversions = Vec::new();
    for track in document
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Video)
    {
        for clip in &track.clips {
            for effect in &clip.effects {
                if !matches!(effect.name.as_str(), "look_lut" | "cube_lut") {
                    continue;
                }
                let entry = match legacy_look_conversion(document, clip.id, effect.id) {
                    Ok(LegacyLookConversion::Builtin {
                        operations,
                        builtin_name,
                        lut_asset,
                        mix_basis_points,
                        reused_existing_asset,
                    }) => json!({
                        "clip_id": clip.id.0,
                        "effect_id": effect.id.0,
                        "legacy_effect": effect.name,
                        "status": "ready",
                        "builtin_name": builtin_name,
                        "lut_asset_id": lut_asset.0,
                        "mix_basis_points": mix_basis_points,
                        "reused_existing_asset": reused_existing_asset,
                        "operations": operations,
                        // `ready` means "submittable exactly as it stands".
                        // When the batch still has to register the built-in,
                        // the only path that can is `convert_legacy_look`:
                        // `AddLutAsset` is refused everywhere else by design
                        // (CC4 §8).
                        "recovery_action": LEGACY_LOOK_CONVERSION_RECOVERY_ACTION,
                    }),
                    Ok(LegacyLookConversion::NeedsImport {
                        path,
                        mix_basis_points,
                    }) => json!({
                        "clip_id": clip.id.0,
                        "effect_id": effect.id.0,
                        "legacy_effect": effect.name,
                        "status": "needs_import",
                        "path": path,
                        "mix_basis_points": mix_basis_points,
                        "recovery_action": LEGACY_LOOK_CONVERSION_IMPORT_RECOVERY_ACTION,
                    }),
                    Err(error) => json!({
                        "clip_id": clip.id.0,
                        "effect_id": effect.id.0,
                        "legacy_effect": effect.name,
                        "status": "unconvertible",
                        "code": error.code(),
                        "message": error.to_string(),
                        "field": error.field(),
                        "observed": error.observed(),
                        "allowed": error.allowed(),
                        "recovery_action": error.recovery_action(),
                    }),
                };
                conversions.push(entry);
            }
        }
    }
    conversions
}

/// Describe the explicit `[AddLutAsset, ConvertLegacyLook]` batch that turns
/// one legacy look into a managed `creative_look` (CC4 §9).
///
/// The conversion is never automatic and never bit-identical: the legacy stage
/// clamped to `[0, 1]` in display space, mixed intensity in the encoded
/// domain, and decoded with the non-invertible `decode_bt709`. That difference
/// is exactly why CC1 §4's "no silent visual change" rule applies here.
///
/// # Errors
///
/// Returns a typed rejection for a missing clip or effect, an effect that is
/// not a legacy look, or an unusable `preset_token`.
pub(crate) fn legacy_look_conversion(
    document: &Document,
    clip_id: ClipId,
    effect_id: EffectId,
) -> Result<LegacyLookConversion, LegacyLookConversionError> {
    let clip = document
        .clip(clip_id)
        .ok_or(LegacyLookConversionError::MissingClip(clip_id))?;
    let effect = clip
        .effects
        .iter()
        .find(|effect| effect.id == effect_id)
        .ok_or(LegacyLookConversionError::MissingEffect {
            clip: clip_id,
            effect: effect_id,
        })?;
    // CC4 §9: `intensity_percent * 100`, clamped to the managed basis-point
    // range so a hand-edited legacy value cannot produce an invalid node.
    let mix_basis_points = stored_parameter(Some(effect), "intensity_percent", 100)
        .saturating_mul(100)
        .clamp(0, LUT_MIX_BASIS_POINTS_MAX);
    match effect.name.as_str() {
        "look_lut" => {
            let token = stored_parameter(Some(effect), "preset_token", 0);
            let builtin = kinewright_media::BuiltinLook::from_preset_token(token)
                .ok_or(LegacyLookConversionError::InvalidPresetToken { observed: token })?;
            // Built-ins are content-addressed like every other asset, so an
            // already-registered record with the pinned hash is reused rather
            // than duplicated.
            let existing = document
                .lut_assets
                .iter()
                .find(|asset| asset.sha256 == builtin.pinned_sha256());
            let (lut_asset, register) = if let Some(asset) = existing {
                (asset.id, None)
            } else {
                let id = document
                    .next_lut_asset_id()
                    .map_err(|_| LegacyLookConversionError::LutAssetIdExhausted)?;
                (id, Some(builtin.to_lut_asset(id)))
            };
            let mut operations = Vec::new();
            if let Some(asset) = register {
                operations.push(Operation::AddLutAsset { asset });
            }
            operations.push(Operation::ConvertLegacyLook {
                clip: clip_id,
                effect: effect_id,
                lut_asset,
                mix_basis_points,
            });
            Ok(LegacyLookConversion::Builtin {
                operations,
                builtin_name: builtin.name(),
                lut_asset,
                mix_basis_points,
                reused_existing_asset: existing.is_some(),
            })
        }
        "cube_lut" => {
            let Some(ParamValue::Text(path)) = effect.parameters.get("path") else {
                return Err(LegacyLookConversionError::MissingExternalPath { effect: effect_id });
            };
            Ok(LegacyLookConversion::NeedsImport {
                path: path.clone(),
                mix_basis_points,
            })
        }
        name => Err(LegacyLookConversionError::NotALegacyLook {
            effect: effect_id,
            name: name.to_owned(),
        }),
    }
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
            lut_assets: Vec::new(),
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
            &LookAssetContext::document_only(&document),
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
        let raw = color_context_value_with_options(
            TimelineRevision(0),
            &document,
            None,
            &[],
            true,
            &LookAssetContext::document_only(&document),
        );
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

    // -----------------------------------------------------------------------
    // CC4 §10.3.14 — plan-not-apply, stage ordering, manifests, and evidence
    // -----------------------------------------------------------------------

    fn cc4_lut_asset(id: u64, title: &str, digit: char) -> kinewright_core::LutAsset {
        kinewright_core::LutAsset {
            id: LutAssetId(id),
            sha256: std::iter::repeat_n(digit, 64).collect(),
            title: title.to_owned(),
            kind: kinewright_core::LutAssetKind::Cube3d,
            size: 33,
            byte_len: 1_174_896,
            domain_min_millionths: [0, 0, 0],
            domain_max_millionths: [1_000_000, 1_000_000, 1_000_000],
            source: LutAssetSource::Imported {
                source_path: "/looks/k2383.cube".to_owned(),
            },
        }
    }

    fn cc4_effect(id: u64, name: &str, parameters: &[(&str, i64)]) -> Effect {
        Effect {
            id: EffectId(id),
            name: name.to_owned(),
            parameters: parameters
                .iter()
                .map(|(name, value)| ((*name).to_owned(), ParamValue::Integer(*value)))
                .collect(),
            keyframes: BTreeMap::new(),
        }
    }

    fn cc4_document(effects: Vec<Effect>, lut_assets: Vec<kinewright_core::LutAsset>) -> Document {
        let mut document = document();
        document.tracks[0].clips[0].effects = effects;
        document.lut_assets = lut_assets;
        document
    }

    fn cc4_plan_args(lut_asset_id: u64) -> LutNodePlanArgs {
        LutNodePlanArgs {
            expected_revision: TimelineRevision(0),
            clip_id: ClipId(1),
            lut_asset_id: LutAssetId(lut_asset_id),
            mix_basis_points: None,
            input_encoding_token: None,
            bypass: None,
            append: false,
            profile_assumption: None,
        }
    }

    /// CC4 §8: both planners emit the exact `InsertEffect` a caller submits,
    /// at the first index that satisfies the §3.2 stage rule, and neither
    /// touches the analyzed document.
    #[test]
    fn cc4_planners_emit_exact_insert_effect_operations_without_applying() {
        let document = cc4_document(
            vec![
                cc4_effect(1, "primary_correction", &[("exposure_milli_stops", 250)]),
                cc4_effect(2, "color_wheels", &[]),
            ],
            vec![cc4_lut_asset(1, "Kodak 2383 D65", 'a')],
        );
        let before = document.clone();
        let looks = LookAssetContext::document_only(&document);

        let technical =
            plan_technical_lut(&document, TimelineRevision(0), &cc4_plan_args(1), &looks)
                .expect("a registered asset plans");
        assert_eq!(technical.insert_index, Some(0));
        assert_eq!(
            technical.operations,
            vec![Operation::InsertEffect {
                clip: ClipId(1),
                index: 0,
                effect: Effect {
                    id: EffectId(3),
                    name: "technical_lut".to_owned(),
                    parameters: BTreeMap::from([(
                        "lut_asset_id".to_owned(),
                        ParamValue::Integer(1)
                    )]),
                    keyframes: BTreeMap::new(),
                },
            }]
        );

        let creative =
            plan_creative_look(&document, TimelineRevision(0), &cc4_plan_args(1), &looks)
                .expect("a registered asset plans");
        assert_eq!(creative.insert_index, Some(2));
        assert_eq!(
            creative.operations,
            vec![Operation::InsertEffect {
                clip: ClipId(1),
                index: 2,
                effect: Effect {
                    id: EffectId(3),
                    name: "creative_look".to_owned(),
                    parameters: BTreeMap::from([(
                        "lut_asset_id".to_owned(),
                        ParamValue::Integer(1)
                    )]),
                    keyframes: BTreeMap::new(),
                },
            }]
        );
        // Evidence-only: the analyzed document is byte-identical afterwards.
        assert_eq!(document, before);
    }

    /// CC4 §8: an existing node of the requested kind is retargeted in place
    /// with `SetEffectParam`, exactly as the CC2/CC3 planners are.
    #[test]
    fn cc4_planners_retarget_an_existing_node_in_place() {
        let document = cc4_document(
            vec![cc4_effect(
                7,
                "creative_look",
                &[("lut_asset_id", 1), ("mix_basis_points", 10_000)],
            )],
            vec![
                cc4_lut_asset(1, "Kodak 2383 D65", 'a'),
                cc4_lut_asset(2, "Bleach", 'b'),
            ],
        );
        let looks = LookAssetContext::document_only(&document);
        let mut args = cc4_plan_args(2);
        args.mix_basis_points = Some(6_000);

        let plan = plan_creative_look(&document, TimelineRevision(0), &args, &looks)
            .expect("a registered asset plans");
        assert!(plan.targets_existing_node);
        assert!(!plan.created_new_node);
        assert_eq!(plan.effect_id, EffectId(7));
        assert_eq!(
            plan.operations,
            vec![
                Operation::SetEffectParam {
                    clip: ClipId(1),
                    effect: EffectId(7),
                    name: "lut_asset_id".to_owned(),
                    value: ParamValue::Integer(2),
                },
                Operation::SetEffectParam {
                    clip: ClipId(1),
                    effect: EffectId(7),
                    name: "mix_basis_points".to_owned(),
                    value: ParamValue::Integer(6_000),
                },
            ]
        );
        assert_eq!(plan.resolved_parameters["mix_basis_points"], 6_000);
        assert_eq!(plan.resolved_parameters["input_encoding_token"], 0);
    }

    /// CC4 §8: both planners bind to the analyzed revision and fail closed.
    #[test]
    fn cc4_planners_fail_closed_on_a_stale_revision() {
        let document = cc4_document(Vec::new(), vec![cc4_lut_asset(1, "Warm", 'c')]);
        let looks = LookAssetContext::document_only(&document);
        let args = cc4_plan_args(1);
        for error in [
            plan_technical_lut(&document, TimelineRevision(4), &args, &looks).unwrap_err(),
            plan_creative_look(&document, TimelineRevision(4), &args, &looks).unwrap_err(),
        ] {
            assert_eq!(error.code(), "revision_conflict");
            let details = error.details();
            assert_eq!(details["field"], "expected_revision");
            assert_eq!(details["observed"], 0);
            assert_eq!(details["allowed"], 4);
        }
    }

    /// CC4 §3.2: the computed index skips unconstrained non-colour effects and
    /// still lands adjacent to the managed stack.
    #[test]
    fn cc4_stage_insert_index_ignores_non_colour_effects() {
        let effects = vec![
            cc4_effect(1, "crop", &[]),
            cc4_effect(2, "primary_correction", &[]),
            cc4_effect(3, "mask", &[]),
            cc4_effect(4, "color_wheels", &[]),
            cc4_effect(5, "opacity", &[]),
        ];
        assert_eq!(
            stage_insert_index(&effects, ColorNodeKind::TechnicalLut),
            1,
            "a technical LUT goes immediately before the first correction node"
        );
        assert_eq!(
            stage_insert_index(&effects, ColorNodeKind::CreativeLook),
            4,
            "a creative look goes immediately after the last correction node"
        );
        // With no managed node at all, both kinds append and leave every
        // unrelated effect's relative order untouched.
        let plain = vec![cc4_effect(1, "crop", &[])];
        assert_eq!(stage_insert_index(&plain, ColorNodeKind::TechnicalLut), 1);
        assert_eq!(stage_insert_index(&plain, ColorNodeKind::CreativeLook), 1);
        assert_eq!(
            stage_insert_index(&[], ColorNodeKind::TechnicalLut),
            0,
            "an empty stack accepts either kind at index 0"
        );
    }

    /// CC4 §5.1: `technical_lut` pins `mix_basis_points` by its descriptor
    /// bounds, so a partial technical normalization is rejected atomically.
    #[test]
    fn cc4_technical_lut_rejects_a_partial_mix() {
        let document = cc4_document(Vec::new(), vec![cc4_lut_asset(1, "Rec709 in", 'd')]);
        let looks = LookAssetContext::document_only(&document);
        let mut args = cc4_plan_args(1);
        args.mix_basis_points = Some(5_000);

        let error = plan_technical_lut(&document, TimelineRevision(0), &args, &looks).unwrap_err();
        assert_eq!(error.code(), "color_node_parameter_out_of_range");
        let details = error.details();
        assert_eq!(details["field"], "mix_basis_points");
        assert_eq!(details["observed"], 5_000);
        assert_eq!(details["allowed"], json!({"min": 10_000, "max": 10_000}));

        // The same value is legal on a creative look, whose mix is the
        // audition control.
        let mut creative = cc4_plan_args(1);
        creative.mix_basis_points = Some(5_000);
        let plan = plan_creative_look(&document, TimelineRevision(0), &creative, &looks)
            .expect("a creative look accepts a partial mix");
        assert_eq!(plan.resolved_parameters["mix_basis_points"], 5_000);
    }

    /// CC4 §2.7: a dangling `lut_asset_id` can never be committed, so the
    /// planner refuses it up front and names every id that would work.
    #[test]
    fn cc4_planners_reject_an_unregistered_lut_asset_id() {
        let document = cc4_document(
            Vec::new(),
            vec![cc4_lut_asset(1, "Warm", 'c'), cc4_lut_asset(4, "Cool", 'e')],
        );
        let looks = LookAssetContext::document_only(&document);
        let error = plan_creative_look(&document, TimelineRevision(0), &cc4_plan_args(9), &looks)
            .unwrap_err();
        assert_eq!(error.code(), "missing_lut_asset");
        let details = error.details();
        assert_eq!(details["field"], "lut_asset_id");
        assert_eq!(details["observed"], 9);
        assert_eq!(details["allowed"], json!([1, 4]));
        assert!(
            details["recovery_action"]
                .as_str()
                .unwrap()
                .contains("list_look_assets")
        );
    }

    /// CC4 §3.1: the per-layer LUT limit is tighter than the managed node
    /// limit because each LUT node needs its own atlas slot.
    #[test]
    fn cc4_planner_enforces_the_per_layer_lut_node_limit() {
        let effects = (1..=4)
            .map(|index| {
                cc4_effect(
                    index,
                    if index == 1 {
                        "technical_lut"
                    } else {
                        "creative_look"
                    },
                    &[("lut_asset_id", 1)],
                )
            })
            .collect::<Vec<_>>();
        let document = cc4_document(effects, vec![cc4_lut_asset(1, "Warm", 'c')]);
        let looks = LookAssetContext::document_only(&document);
        let mut args = cc4_plan_args(1);
        args.append = true;

        let error = plan_creative_look(&document, TimelineRevision(0), &args, &looks).unwrap_err();
        assert_eq!(error.code(), "too_many_lut_nodes");
        let details = error.details();
        assert_eq!(details["observed"], 5);
        assert_eq!(details["allowed"], json!({"max": LUT_NODE_LIMIT_PER_LAYER}));
    }

    /// CC4 §8: the `color_nodes` manifest carries the full LUT identity for a
    /// `[technical, primary, creative(bypassed)]` stack.
    #[test]
    fn cc4_color_node_manifest_reports_lut_identity_role_and_stage() {
        let document = cc4_document(
            vec![
                cc4_effect(
                    1,
                    "technical_lut",
                    &[("lut_asset_id", 1), ("input_encoding_token", 2)],
                ),
                cc4_effect(2, "primary_correction", &[]),
                cc4_effect(
                    3,
                    "creative_look",
                    &[
                        ("lut_asset_id", 2),
                        ("mix_basis_points", 4_000),
                        ("bypass", 1),
                    ],
                ),
            ],
            vec![
                cc4_lut_asset(1, "Kodak 2383 D65", 'a'),
                cc4_lut_asset(2, "Bleach", 'b'),
            ],
        );
        let looks = LookAssetContext::document_only(&document);
        let manifest = color_node_manifest(&document.tracks[0].clips[0].effects, &looks);
        assert_eq!(manifest.len(), 3);

        let technical = &manifest[0];
        assert_eq!(technical["kind"], "technical_lut");
        assert_eq!(technical["role"], "technical");
        assert_eq!(technical["color_stage"], "input");
        assert_eq!(technical["stage_index"], 0);
        assert_eq!(technical["lut_asset_id"], 1);
        assert_eq!(technical["lut_title"], "Kodak 2383 D65");
        assert_eq!(technical["lut_sha256"], "a".repeat(64));
        assert_eq!(technical["lut_size"], 33);
        assert_eq!(technical["lut_provenance"]["kind"], "imported");
        assert_eq!(technical["mix_basis_points"], 10_000);
        assert_eq!(technical["input_encoding"], "grade709");
        assert_eq!(technical["bypass"], 0);
        assert_eq!(technical["active"], true);
        assert_eq!(technical["inactive_reason"], Value::Null);
        // No store root is published to a document-only context, so the
        // availability marker is honest rather than invented.
        assert_eq!(
            technical["lut_availability"]["kind"],
            LUT_AVAILABILITY_UNKNOWN_NO_STORE
        );
        assert_eq!(technical["lut_store_path"], Value::Null);

        assert_eq!(manifest[1]["role"], "correction");
        assert_eq!(manifest[1]["color_stage"], "correction");

        let creative = &manifest[2];
        assert_eq!(creative["role"], "creative");
        assert_eq!(creative["color_stage"], "look");
        assert_eq!(creative["stage_index"], 2);
        assert_eq!(creative["lut_asset_id"], 2);
        assert_eq!(creative["mix_basis_points"], 4_000);
        assert_eq!(creative["bypass"], 1);
        assert_eq!(creative["active"], false);
        assert_eq!(creative["inactive_reason"], "bypassed");
        assert_eq!(creative["may_be_active"], false);
    }

    /// CC4 §2.3: a node bound to an unregistered asset reports the structural
    /// `missing_lut_asset` the same way a legacy stage is reported.
    #[test]
    fn cc4_manifest_reports_missing_lut_asset_for_a_dangling_reference() {
        let document = cc4_document(
            vec![cc4_effect(1, "creative_look", &[("lut_asset_id", 9)])],
            Vec::new(),
        );
        let looks = LookAssetContext::document_only(&document);
        let manifest = color_node_manifest(&document.tracks[0].clips[0].effects, &looks);
        let warnings = manifest[0]["warnings"].as_array().unwrap();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0]["code"], "missing_lut_asset");
        assert_eq!(warnings[0]["lut_asset_id"], 9);
        assert_eq!(warnings[0]["blocking"], true);
        assert_eq!(manifest[0]["lut_title"], Value::Null);
    }

    /// CC4 §2.3, §8: the managed renderer's `missing_lut_asset:` refusal is
    /// recovered into a typed proof error naming the asset, not left as a
    /// prose `render_failed` message.
    ///
    /// Pure over the rendered text and the session's look context, so it says
    /// the same thing on every adapter.
    #[test]
    fn cc4_a_render_refusal_for_an_unpublished_look_is_typed_not_prose() {
        let document = cc4_document(
            vec![cc4_effect(7, "creative_look", &[("lut_asset_id", 3)])],
            vec![cc4_lut_asset(3, "Kodak 2383 D65", 'a')],
        );
        let looks = LookAssetContext::document_only(&document);

        // The compositor's shape, which names both the node and the asset.
        let error = ColorProofError::from_proof_render_error(
            "after",
            MediaError::Backend(
                "missing_lut_asset: creative_look node 7 references LUT asset 3, which is not in \
                 the verified LUT library; restore or relink the asset before rendering"
                    .to_owned(),
            ),
            &looks,
            None,
        );
        assert_eq!(error.code(), "missing_lut_asset");
        let details = error.details();
        assert_eq!(details["field"], "lut_asset_id");
        assert_eq!(details["observed"], 3);
        assert_eq!(details["effect_id"], 7);
        assert_eq!(details["lut_title"], "Kodak 2383 D65");
        assert_eq!(details["lut_sha256"], "a".repeat(64));
        assert_eq!(details["stage"], "after");
        assert!(details["availability"].is_object(), "{details}");
        assert!(
            details["recovery_action"]
                .as_str()
                .is_some_and(|action| action.contains("list_look_assets")),
            "{details}"
        );

        // The colour-pipeline shape names no node, so the proofed node is the
        // honest fallback.
        let error = ColorProofError::from_proof_render_error(
            "before",
            MediaError::Backend(
                "missing_lut_asset: no verified LUT asset 3 is available for an active LUT node"
                    .to_owned(),
            ),
            &looks,
            Some(EffectId(7)),
        );
        assert_eq!(error.code(), "missing_lut_asset");
        assert_eq!(error.details()["effect_id"], 7);

        // The engine's pre-render binding shape, which names the ids as
        // `LUT asset(s) <id> (<sha256>)` and can name several.
        let error = ColorProofError::from_proof_render_error(
            "after",
            MediaError::Backend(format!(
                "missing_lut_asset: no published lattice matches LUT asset(s) 3 ({}); restore or \
                 re-import the asset and let the project republish its library before rendering",
                "a".repeat(64)
            )),
            &looks,
            Some(EffectId(7)),
        );
        assert_eq!(error.code(), "missing_lut_asset");
        let details = error.details();
        assert_eq!(details["observed"], 3);
        assert_eq!(details["effect_id"], 7);
        assert_eq!(details["lut_title"], "Kodak 2383 D65");

        // An id the project does not register reports exactly that, rather
        // than borrowing the "no store root is published" marker.
        let error = ColorProofError::from_proof_render_error(
            "after",
            MediaError::Backend(
                "missing_lut_asset: creative_look node 7 references LUT asset 99, which is not in \
                 the verified LUT library"
                    .to_owned(),
            ),
            &looks,
            None,
        );
        let details = error.details();
        assert_eq!(details["observed"], 99);
        assert_eq!(details["lut_title"], Value::Null);
        assert_eq!(details["availability"]["kind"], "unregistered");

        // Every other backend failure stays an ordinary render failure.
        let other = ColorProofError::from_proof_render_error(
            "after",
            MediaError::Backend("decoder gave up".to_owned()),
            &looks,
            None,
        );
        assert_eq!(other.code(), "color_proof_render_failed");
    }

    /// CC4 §8: `list_look_assets` publishes the built-in catalogue and every
    /// project asset with its references, and stays compact.
    #[test]
    fn cc4_list_look_assets_reports_builtins_assets_and_references() {
        let document = cc4_document(
            vec![cc4_effect(5, "creative_look", &[("lut_asset_id", 1)])],
            vec![cc4_lut_asset(1, "Kodak 2383 D65", 'a')],
        );
        let looks = LookAssetContext::document_only(&document);
        let value = look_assets_value(TimelineRevision(11), &document, &looks);

        assert_eq!(value["timeline_revision"], 11);
        assert_eq!(value["store_root"], Value::Null);
        assert_eq!(value["store_root_known"], false);
        assert_eq!(value["asset_count"], 1);
        assert_eq!(value["lut_node_limit_per_layer"], LUT_NODE_LIMIT_PER_LAYER);

        let builtin = value["builtin"].as_array().unwrap();
        assert_eq!(builtin.len(), kinewright_media::BuiltinLook::ALL.len());
        assert_eq!(builtin[0]["name"], "identity");
        assert_eq!(builtin[1]["name"], "warm");
        assert_eq!(
            builtin[1]["sha256"],
            kinewright_media::BuiltinLook::Warm.pinned_sha256()
        );

        let assets = value["assets"].as_array().unwrap();
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0]["lut_asset_id"], 1);
        assert_eq!(assets[0]["title"], "Kodak 2383 D65");
        assert_eq!(assets[0]["sha256"], "a".repeat(64));
        assert_eq!(assets[0]["kind"], "cube_3d");
        assert_eq!(assets[0]["provenance"]["kind"], "imported");
        assert_eq!(
            assets[0]["availability"]["kind"],
            LUT_AVAILABILITY_UNKNOWN_NO_STORE
        );
        assert_eq!(
            assets[0]["referenced_by"],
            json!([{"clip_id": 1, "effect_id": 5}])
        );
        // Compactness: no samples and no per-node payload ride along.
        assert!(assets[0].get("samples").is_none());
    }

    /// CC4 §9: a legacy `look_lut` converts through the explicit two-operation
    /// batch, and a `cube_lut` reports that only `import_lut_asset` can supply
    /// its bytes.
    #[test]
    fn cc4_legacy_look_conversion_resolves_preset_tokens_and_defers_external_luts() {
        let document = cc4_document(
            vec![
                cc4_effect(
                    1,
                    "look_lut",
                    &[("preset_token", 1), ("intensity_percent", 60)],
                ),
                Effect {
                    id: EffectId(2),
                    name: "cube_lut".to_owned(),
                    parameters: BTreeMap::from([(
                        "path".to_owned(),
                        ParamValue::Text("/looks/external.cube".to_owned()),
                    )]),
                    keyframes: BTreeMap::new(),
                },
            ],
            Vec::new(),
        );

        let converted = legacy_look_conversion(&document, ClipId(1), EffectId(1)).unwrap();
        let LegacyLookConversion::Builtin {
            operations,
            builtin_name,
            lut_asset,
            mix_basis_points,
            reused_existing_asset,
        } = converted
        else {
            panic!("preset_token 1 resolves to a built-in look");
        };
        assert_eq!(builtin_name, "warm");
        assert_eq!(lut_asset, LutAssetId(1));
        assert_eq!(mix_basis_points, 6_000);
        assert!(!reused_existing_asset);
        assert_eq!(operations.len(), 2);
        let Operation::AddLutAsset { asset } = &operations[0] else {
            panic!("the batch registers the built-in first");
        };
        assert_eq!(
            asset.sha256,
            kinewright_media::BuiltinLook::Warm.pinned_sha256()
        );
        assert_eq!(
            operations[1],
            Operation::ConvertLegacyLook {
                clip: ClipId(1),
                effect: EffectId(1),
                lut_asset: LutAssetId(1),
                mix_basis_points: 6_000,
            }
        );

        let external = legacy_look_conversion(&document, ClipId(1), EffectId(2)).unwrap();
        assert_eq!(
            external,
            LegacyLookConversion::NeedsImport {
                path: "/looks/external.cube".to_owned(),
                mix_basis_points: 10_000,
            }
        );

        // A non-legacy effect is a typed rejection, never a silent conversion.
        let document = cc4_document(vec![cc4_effect(3, "color_wheels", &[])], Vec::new());
        let error = legacy_look_conversion(&document, ClipId(1), EffectId(3)).unwrap_err();
        assert_eq!(error.code(), "not_a_legacy_look");
    }

    /// CC4 §8: every `legacy_look_conversions` entry carries a
    /// `recovery_action`, and `unconvertible` carries the full
    /// `field`/`observed`/`allowed` shape.
    ///
    /// The two unconvertible codes are only reachable from a hand-edited
    /// project - Core rejects an out-of-range `preset_token` and a `cube_lut`
    /// with no `path` at `validate` - so they are exercised here, on the
    /// value builder, rather than through a spawned Core.
    #[test]
    fn cc4_legacy_look_conversion_entries_always_carry_a_recovery_action() {
        let document = cc4_document(
            vec![
                cc4_effect(
                    1,
                    "look_lut",
                    &[("preset_token", 1), ("intensity_percent", 60)],
                ),
                Effect {
                    id: EffectId(2),
                    name: "cube_lut".to_owned(),
                    parameters: BTreeMap::from([(
                        "path".to_owned(),
                        ParamValue::Text("/looks/external.cube".to_owned()),
                    )]),
                    keyframes: BTreeMap::new(),
                },
                cc4_effect(3, "look_lut", &[("preset_token", 9)]),
                Effect {
                    id: EffectId(4),
                    name: "cube_lut".to_owned(),
                    parameters: BTreeMap::new(),
                    keyframes: BTreeMap::new(),
                },
            ],
            Vec::new(),
        );

        let conversions = legacy_look_conversions_value(&document);
        assert_eq!(conversions.len(), 4);
        for entry in &conversions {
            assert!(
                entry["recovery_action"]
                    .as_str()
                    .expect("every status carries a recovery_action")
                    .contains("convert_legacy_look")
                    || entry["status"] == "unconvertible",
                "{entry}"
            );
        }
        assert_eq!(conversions[0]["status"], "ready");
        assert_eq!(conversions[1]["status"], "needs_import");
        assert!(
            conversions[1]["recovery_action"]
                .as_str()
                .unwrap()
                .contains("confirm"),
            "{}",
            conversions[1]
        );

        assert_eq!(conversions[2]["status"], "unconvertible");
        assert_eq!(conversions[2]["code"], "invalid_preset_token");
        assert_eq!(conversions[2]["field"], "preset_token");
        assert_eq!(conversions[2]["observed"], 9);
        assert_eq!(
            conversions[2]["allowed"],
            "an integer in the inclusive range 0..=4"
        );
        assert!(
            conversions[2]["recovery_action"]
                .as_str()
                .unwrap()
                .contains("SetEffectParam")
        );

        assert_eq!(conversions[3]["status"], "unconvertible");
        assert_eq!(conversions[3]["code"], "missing_external_lut_path");
        assert_eq!(conversions[3]["field"], "path");
        assert_eq!(conversions[3]["observed"], Value::Null);
        assert!(conversions[3]["allowed"].is_string());
        assert!(conversions[3]["recovery_action"].is_string());
    }

    /// CC4 §8: a plan that binds a `missing`, `changed`, or `unreadable` asset
    /// is returned with the availability status *and* the recovery action.
    #[test]
    fn cc4_lut_asset_summary_carries_the_typed_recovery_action() {
        let verified = kinewright_media::BuiltinLook::Warm.to_lut_asset(LutAssetId(1));
        let mut missing = verified.clone();
        missing.id = LutAssetId(2);
        missing.source = LutAssetSource::Imported {
            source_path: "vendor.cube".to_owned(),
        };
        let document = cc4_document(Vec::new(), vec![verified.clone(), missing.clone()]);
        let resolver = |asset: &LutAsset| {
            if asset.id == LutAssetId(2) {
                LutAvailabilityStatus {
                    kind: LutAvailabilityKind::Missing,
                    observed_sha256: None,
                    reason: Some("the store file is absent".to_owned()),
                    path: None,
                }
            } else {
                LutAvailabilityStatus {
                    kind: LutAvailabilityKind::Verified,
                    observed_sha256: Some(asset.sha256.clone()),
                    reason: None,
                    path: None,
                }
            }
        };
        let looks = LookAssetContext::new(
            &document,
            Some(PathBuf::from("/projects/edit.kinewright-assets")),
            Some(&resolver as &dyn Fn(&LutAsset) -> LutAvailabilityStatus),
        );

        assert_eq!(
            looks.asset_summary(&verified)["recovery_action"],
            Value::Null
        );
        let summary = looks.asset_summary(&missing);
        assert_eq!(summary["availability"]["kind"], "missing");
        assert!(
            summary["recovery_action"]
                .as_str()
                .unwrap()
                .contains("import_lut_asset"),
            "{summary}"
        );

        // The planner returns exactly that summary next to its operations.
        let plan = plan_creative_look(
            &document,
            TimelineRevision(0),
            &LutNodePlanArgs {
                expected_revision: TimelineRevision(0),
                clip_id: ClipId(1),
                lut_asset_id: LutAssetId(2),
                mix_basis_points: None,
                input_encoding_token: None,
                bypass: None,
                append: false,
                profile_assumption: None,
            },
            &looks,
        )
        .expect("a missing asset is planned with its status, not refused");
        let lut_asset = plan.lut_asset.expect("the plan names its asset");
        assert_eq!(lut_asset["availability"]["kind"], "missing");
        assert!(lut_asset["recovery_action"].is_string());
    }

    // -----------------------------------------------------------------------
    // CC5 §7 — plan_secondary_correction
    // -----------------------------------------------------------------------

    /// An `Analysis` double that answers only what CC5 §7 needs.
    ///
    /// `matte_proof_for_document` is defaulted to `NotImplemented` on the trait
    /// until the media engine lands it, so every CC5 agent path has to work
    /// against a backend that cannot proof. This double answers with a
    /// hand-built coverage when one is installed, and with the real
    /// `NotImplemented` when one is not, so both branches are exercised.
    #[derive(Debug, Default)]
    struct MatteAnalysisDouble {
        coverage: Option<kinewright_core::RgbaImage>,
        monitor: Option<kinewright_core::RgbaImage>,
    }

    /// A `width × height` coverage raster whose codes come from `code(x, y)`.
    ///
    /// Written as a plain RGBA buffer rather than through any CC5 code path, so
    /// a statistic can never be "proved" by the function that produced it.
    fn coverage_raster(
        width: u32,
        height: u32,
        code: impl Fn(u32, u32) -> u8,
    ) -> kinewright_core::RgbaImage {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let value = code(x, y);
                pixels.extend_from_slice(&[value, value, value, 255]);
            }
        }
        kinewright_core::RgbaImage {
            width,
            height,
            pixels,
        }
    }

    impl kinewright_core::Analysis for MatteAnalysisDouble {
        fn probe(
            &self,
            _path: &std::path::Path,
        ) -> Result<kinewright_core::MediaAsset, MediaError> {
            Err(MediaError::NotImplemented)
        }

        fn media_availability(
            &self,
            _asset: &kinewright_core::MediaAsset,
        ) -> MediaAvailabilityStatus {
            MediaAvailabilityStatus {
                kind: kinewright_core::MediaAvailabilityKind::OnlineVerified,
                observed_fingerprint: None,
                reason: None,
            }
        }

        fn thumbnail_at(
            &self,
            _at: TimeCode,
            _max_width: u32,
        ) -> Result<kinewright_core::RgbaImage, MediaError> {
            Err(MediaError::NotImplemented)
        }

        fn monitor_proof_for_document(
            &self,
            _document: Arc<Document>,
            _at: TimeCode,
        ) -> Result<kinewright_core::MonitorProof, MediaError> {
            self.monitor
                .clone()
                .map_or(Err(MediaError::NotImplemented), |image| {
                    Ok(kinewright_core::MonitorProof {
                        image,
                        metadata: kinewright_core::MonitorProofMetadata::test_double(),
                    })
                })
        }

        fn matte_proof_for_document(
            &self,
            _document: Arc<Document>,
            _at: TimeCode,
            clip: ClipId,
            effect: EffectId,
        ) -> Result<kinewright_core::MatteProof, MediaError> {
            let Some(coverage) = self.coverage.clone() else {
                return Err(MediaError::NotImplemented);
            };
            Ok(kinewright_core::MatteProof {
                metadata: kinewright_core::MatteProofMetadata {
                    render: kinewright_core::MonitorProofMetadata::test_double(),
                    clip,
                    effect,
                    node_kind: "color_wheels".to_owned(),
                    coverage_encoding: kinewright_core::MATTE_COVERAGE_ENCODING.to_owned(),
                    coverage_scale: kinewright_core::MATTE_COVERAGE_SCALE,
                    raster_aspect_millionths: 1_777_778,
                    matte_enabled: true,
                    window_count: 1,
                    qualifier_enabled: false,
                },
                coverage,
            })
        }

        fn request_transcription(&self, _asset: kinewright_core::MediaAsset) {}

        fn transcript_status(
            &self,
            _asset: &kinewright_core::MediaAsset,
        ) -> kinewright_core::TranscriptStatus {
            kinewright_core::TranscriptStatus::NotRequested
        }

        fn timeline_transcript(
            &self,
            _document: &Document,
            _range: Option<std::ops::Range<TimeCode>>,
        ) -> Result<Vec<kinewright_core::TimelineTranscriptWord>, MediaError> {
            Ok(Vec::new())
        }

        fn request_silence_detection(&self, _asset: kinewright_core::MediaAsset) {}

        fn silence_status(
            &self,
            _asset: &kinewright_core::MediaAsset,
        ) -> kinewright_core::SilenceStatus {
            kinewright_core::SilenceStatus::NotRequested
        }

        fn timeline_silences(
            &self,
            _document: &Document,
            _range: Option<std::ops::Range<TimeCode>>,
            _minimum_source_frames: TimeCode,
        ) -> Result<Vec<kinewright_core::TimelineSilenceSpan>, MediaError> {
            Ok(Vec::new())
        }

        fn request_scene_detection(&self, _asset: kinewright_core::MediaAsset) {}

        fn scene_status(
            &self,
            _asset: &kinewright_core::MediaAsset,
        ) -> kinewright_core::SceneStatus {
            kinewright_core::SceneStatus::NotRequested
        }

        fn timeline_scene_changes(
            &self,
            _document: &Document,
            _range: Option<std::ops::Range<TimeCode>>,
            _minimum_confidence_basis_points: u16,
        ) -> Result<Vec<kinewright_core::TimelineSceneChange>, MediaError> {
            Ok(Vec::new())
        }

        fn request_waveform(
            &self,
            _asset: kinewright_core::MediaAsset,
            _request_generation: u64,
        ) -> bool {
            false
        }

        fn request_thumbnail(
            &self,
            _asset: kinewright_core::MediaAsset,
            _source_at: TimeCode,
            _max_width: u32,
            _request_generation: u64,
        ) -> bool {
            false
        }

        fn visual_asset_results(
            &self,
        ) -> crossbeam_channel::Receiver<kinewright_core::VisualAssetResult> {
            crossbeam_channel::never()
        }
    }

    fn secondary_args(
        target_effect_id: Option<EffectId>,
        node_kind: Option<&str>,
    ) -> SecondaryCorrectionPlanArgs {
        SecondaryCorrectionPlanArgs {
            expected_revision: TimelineRevision(0),
            clip_id: ClipId(1),
            target_effect_id,
            node_kind: node_kind.map(str::to_owned),
            append: false,
            windows: None,
            qualifier: None,
            combine: None,
            invert: None,
            mix_basis_points: None,
            timecode: Some(TimeCode(10)),
            sample_roi: None,
            derive_qualifier_from_sample: false,
            profile_assumption: None,
        }
    }

    /// One centred elliptical window plus a saturation-and-luma qualifier,
    /// spelled out so the expansion is asserted against hand-written names and
    /// values rather than against the code that produced them.
    fn one_window_and_qualifier(args: &mut SecondaryCorrectionPlanArgs) {
        args.windows = Some(vec![MatteWindowRequest {
            shape: Some("ellipse".to_owned()),
            center_x: Some(6_000),
            center_y: Some(4_000),
            half_width: Some(1_500),
            half_height: Some(2_000),
            rotation: Some(-4_500),
            feather: Some(1_200),
            invert: Some(false),
        }]);
        args.qualifier = Some(MatteQualifierRequest {
            saturation_low: Some(3_000),
            saturation_high: Some(9_000),
            luma_low: Some(2_000),
            luma_high: Some(8_500),
            ..MatteQualifierRequest::default()
        });
        args.mix_basis_points = Some(7_500);
    }

    /// CC5 §7 / §9.2.15: the plan returns exact operations, binds to the
    /// analyzed revision, and leaves the source document byte-identical.
    #[test]
    fn secondary_plan_emits_exact_matte_operations_and_never_applies() {
        let mut document = document();
        document.tracks[0].clips[0]
            .effects
            .push(wheels_node(5, integers([("gain_red_thousandths", 1_200)])));
        let before = document.clone();

        let mut args = secondary_args(Some(EffectId(5)), None);
        one_window_and_qualifier(&mut args);
        let plan = plan_secondary_correction(
            &document,
            TimelineRevision(0),
            &MatteAnalysisDouble::default(),
            &args,
        )
        .expect("valid secondary plan");

        // Every operation, hand-written: the exact `matte_*` names CC5 §2.2
        // generates and the exact integers the request named.
        let expected = [
            ("matte_enabled", 1),
            ("matte_luma_high_basis_points", 8_500),
            ("matte_luma_low_basis_points", 2_000),
            ("matte_mix_basis_points", 7_500),
            ("matte_qualifier_enabled", 1),
            ("matte_saturation_high_basis_points", 9_000),
            ("matte_saturation_low_basis_points", 3_000),
            ("matte_window0_center_x_basis_points", 6_000),
            ("matte_window0_center_y_basis_points", 4_000),
            ("matte_window0_feather_basis_points", 1_200),
            ("matte_window0_half_height_basis_points", 2_000),
            ("matte_window0_half_width_basis_points", 1_500),
            ("matte_window0_rotation_centidegrees", -4_500),
            ("matte_window0_shape_token", 2),
            ("matte_window_count", 1),
        ];
        assert_eq!(
            plan.operations,
            expected
                .iter()
                .map(|(name, value)| Operation::SetEffectParam {
                    clip: ClipId(1),
                    effect: EffectId(5),
                    name: (*name).to_owned(),
                    value: ParamValue::Integer(*value),
                })
                .collect::<Vec<_>>()
        );
        // `matte_window0_invert` was requested as `false`, which is its
        // neutral, so no operation writes it — an unchanged control produces
        // none (CC5 §2.2).
        assert!(
            !plan
                .operations
                .iter()
                .any(|operation| format!("{operation:?}").contains("matte_window0_invert"))
        );
        assert!(plan.targets_existing_node);
        assert!(!plan.created_new_node);
        assert_eq!(plan.effect_id, EffectId(5));
        assert_eq!(plan.kind, ColorNodeKind::Wheels);
        assert!(!plan.no_change);
        // Evidence only: the analyzed document is byte-identical afterwards.
        assert_eq!(document, before);
    }

    /// CC5 §7: a new node is *inserted* at the stage-legal index, not appended,
    /// and stores only the values the caller moved off their neutrals.
    #[test]
    fn secondary_plan_inserts_a_new_node_at_the_stage_legal_index() {
        let mut document = document();
        // A creative_look sits at the Look stage, so a new color_wheels node
        // must land before it (CC4 §3.2).
        document.lut_assets.push(LutAsset {
            id: LutAssetId(1),
            sha256: "a".repeat(64),
            title: "look".to_owned(),
            kind: kinewright_core::LutAssetKind::Cube3d,
            size: 17,
            byte_len: 1_024,
            domain_min_millionths: [0; 3],
            domain_max_millionths: [1_000_000; 3],
            source: LutAssetSource::Builtin {
                name: "neutral".to_owned(),
            },
        });
        document.tracks[0].clips[0].effects.push(Effect {
            id: EffectId(9),
            name: "creative_look".to_owned(),
            parameters: integers([("lut_asset_id", 1)]),
            keyframes: BTreeMap::new(),
        });

        let mut args = secondary_args(None, Some("color_wheels"));
        args.windows = Some(vec![MatteWindowRequest {
            center_x: Some(2_500),
            ..MatteWindowRequest::default()
        }]);
        let plan = plan_secondary_correction(
            &document,
            TimelineRevision(0),
            &MatteAnalysisDouble::default(),
            &args,
        )
        .expect("valid secondary plan");

        assert_eq!(plan.insert_index, Some(0));
        assert!(plan.created_new_node);
        assert_eq!(
            plan.operations,
            vec![Operation::InsertEffect {
                clip: ClipId(1),
                index: 0,
                effect: Effect {
                    id: EffectId(10),
                    name: "color_wheels".to_owned(),
                    // Only the non-neutral values: `matte_window0_center_x` is
                    // 2500 against a 5000 neutral, and the count and the master
                    // switch are both off their neutrals.
                    parameters: integers([
                        ("matte_enabled", 1),
                        ("matte_window0_center_x_basis_points", 2_500),
                        ("matte_window_count", 1),
                    ]),
                    keyframes: BTreeMap::new(),
                },
            }]
        );
    }

    /// CC5 §7: revision-gated, and a stale snapshot fails closed.
    #[test]
    fn secondary_plan_fails_closed_on_a_stale_revision() {
        let document = document();
        let mut args = secondary_args(None, Some("color_wheels"));
        one_window_and_qualifier(&mut args);
        args.expected_revision = TimelineRevision(3);

        let error = plan_secondary_correction(
            &document,
            TimelineRevision(7),
            &MatteAnalysisDouble::default(),
            &args,
        )
        .unwrap_err();

        assert_eq!(error.code(), "revision_conflict");
        let details = error.details();
        assert_eq!(details["field"], "expected_revision");
        assert_eq!(details["observed"], 3);
        assert_eq!(details["allowed"], 7);
    }

    /// CC5 §2.1: a technical input transform normalizes the *whole* source, so
    /// it carries no matte and naming it is a typed refusal.
    #[test]
    fn secondary_plan_rejects_a_technical_lut_target() {
        let mut document = document();
        document.tracks[0].clips[0].effects.push(Effect {
            id: EffectId(4),
            name: "technical_lut".to_owned(),
            parameters: BTreeMap::new(),
            keyframes: BTreeMap::new(),
        });

        for args in [
            secondary_args(Some(EffectId(4)), None),
            secondary_args(None, Some("technical_lut")),
        ] {
            let error = plan_secondary_correction(
                &document,
                TimelineRevision(0),
                &MatteAnalysisDouble::default(),
                &args,
            )
            .unwrap_err();

            assert_eq!(error.code(), "matte_unsupported_node_kind");
            let details = error.details();
            assert_eq!(details["field"], "node_kind");
            assert_eq!(details["observed"], "technical_lut");
            assert_eq!(details["allowed"], json!(MATTE_CAPABLE_NODE_NAMES));
            assert!(
                details["recovery_action"]
                    .as_str()
                    .unwrap()
                    .contains("normalizes the whole source")
            );
        }
    }

    /// CC5 §2.2: every bound is validated against the Core descriptor before
    /// any operation is constructed, with `field`/`observed`/`allowed`.
    #[test]
    fn secondary_plan_rejects_out_of_range_controls_with_the_exact_bounds() {
        let mut document = document();
        document.tracks[0].clips[0]
            .effects
            .push(wheels_node(5, BTreeMap::new()));

        let mut args = secondary_args(Some(EffectId(5)), None);
        args.windows = Some(vec![MatteWindowRequest {
            // The centre range is deliberately wide so a tracked window may
            // leave and re-enter frame; 20001 is one past its maximum.
            center_x: Some(20_001),
            ..MatteWindowRequest::default()
        }]);
        let error = plan_secondary_correction(
            &document,
            TimelineRevision(0),
            &MatteAnalysisDouble::default(),
            &args,
        )
        .unwrap_err();
        assert_eq!(error.code(), "color_node_parameter_out_of_range");
        let details = error.details();
        assert_eq!(details["field"], "matte_window0_center_x_basis_points");
        assert_eq!(details["observed"], 20_001);
        assert_eq!(details["allowed"], json!({"min": -10_000, "max": 20_000}));

        // A half extent of zero is a degenerate window, so the minimum is 1.
        let mut args = secondary_args(Some(EffectId(5)), None);
        args.windows = Some(vec![MatteWindowRequest {
            half_width: Some(0),
            ..MatteWindowRequest::default()
        }]);
        let error = plan_secondary_correction(
            &document,
            TimelineRevision(0),
            &MatteAnalysisDouble::default(),
            &args,
        )
        .unwrap_err();
        assert_eq!(
            details_field(&error),
            "matte_window0_half_width_basis_points"
        );
        assert_eq!(error.details()["allowed"], json!({"min": 1, "max": 10_000}));

        // More than four windows is refused by the window rule itself, not by
        // a descriptor bound, because there is no fifth window to name.
        let mut args = secondary_args(Some(EffectId(5)), None);
        args.windows = Some(vec![MatteWindowRequest::default(); 5]);
        let error = plan_secondary_correction(
            &document,
            TimelineRevision(0),
            &MatteAnalysisDouble::default(),
            &args,
        )
        .unwrap_err();
        assert_eq!(error.code(), "matte_window_count_out_of_range");
        assert_eq!(error.details()["observed"], 5);
        assert_eq!(error.details()["allowed"], json!({"max": 4}));

        // An unrecognized shape token names the vocabulary it violated.
        let mut args = secondary_args(Some(EffectId(5)), None);
        args.windows = Some(vec![MatteWindowRequest {
            shape: Some("polygon".to_owned()),
            ..MatteWindowRequest::default()
        }]);
        let error = plan_secondary_correction(
            &document,
            TimelineRevision(0),
            &MatteAnalysisDouble::default(),
            &args,
        )
        .unwrap_err();
        assert_eq!(error.code(), "matte_token_not_recognized");
        assert_eq!(error.details()["field"], "windows[0].shape");
        assert_eq!(error.details()["observed"], "polygon");
        assert_eq!(error.details()["allowed"], json!(["rect", "ellipse"]));
    }

    fn details_field(error: &ColorNodePlanError) -> String {
        error.details()["field"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }

    /// CC5 §5.1: `matte_window{j}_shape_token` accepts `Hold` keyframes only,
    /// and an existing Hold curve wins over a static write at every frame from
    /// its first keyframe onward — so the plan refuses rather than warning.
    #[test]
    fn secondary_plan_refuses_to_write_a_keyframed_hold_only_token() {
        let mut with_token = document();
        let mut node = wheels_node(5, BTreeMap::new());
        node.keyframes.insert(
            "matte_window0_shape_token".to_owned(),
            kinewright_core::AutomationCurve {
                keyframes: vec![kinewright_core::Keyframe {
                    at: TimeCode(0),
                    value: 1,
                    interpolation: kinewright_core::KeyframeInterpolation::Hold,
                }],
            },
        );
        with_token.tracks[0].clips[0].effects.push(node);

        let mut args = secondary_args(Some(EffectId(5)), None);
        args.windows = Some(vec![MatteWindowRequest {
            shape: Some("ellipse".to_owned()),
            ..MatteWindowRequest::default()
        }]);
        let error = plan_secondary_correction(
            &with_token,
            TimelineRevision(0),
            &MatteAnalysisDouble::default(),
            &args,
        )
        .unwrap_err();

        assert_eq!(error.code(), "matte_hold_only_parameter_keyframed");
        let details = error.details();
        assert_eq!(details["field"], "matte_window0_shape_token");
        assert_eq!(details["hold_only"], true);
        assert!(
            details["observed"]
                .as_str()
                .unwrap()
                .contains("matte_window0_shape_token")
        );
        assert!(
            details["allowed"]
                .as_str()
                .unwrap()
                .contains("no automation")
        );
        assert!(
            details["recovery_action"]
                .as_str()
                .unwrap()
                .contains("ClearEffectKeyframes")
        );
        // Every fully keyframable control keeps the ordinary CC3 posture: a
        // warning, and the static value is still written.
        let mut node = wheels_node(6, BTreeMap::new());
        node.keyframes.insert(
            "matte_mix_basis_points".to_owned(),
            kinewright_core::AutomationCurve {
                keyframes: vec![kinewright_core::Keyframe {
                    at: TimeCode(0),
                    value: 5_000,
                    interpolation: kinewright_core::KeyframeInterpolation::Linear,
                }],
            },
        );
        let mut second = document();
        second.tracks[0].clips[0].effects.push(node);
        let document = second;
        let mut args = secondary_args(Some(EffectId(6)), None);
        args.mix_basis_points = Some(7_500);
        let plan = plan_secondary_correction(
            &document,
            TimelineRevision(0),
            &MatteAnalysisDouble::default(),
            &args,
        )
        .expect("a keyframed non-token control is a warning, not a refusal");
        assert!(
            plan.warnings
                .iter()
                .any(|warning| warning.contains("matte_mix_basis_points"))
        );
    }

    /// CC5 §7: `predicted_coverage` is `null` with the stable
    /// `matte_proof_unavailable` reason while the engine cannot proof, and
    /// carries the §4.2 statistics once it can. It is never invented.
    #[test]
    fn secondary_plan_predicted_coverage_is_measured_or_typed_unavailable() {
        let mut document = document();
        document.tracks[0].clips[0]
            .effects
            .push(wheels_node(5, BTreeMap::new()));
        let mut args = secondary_args(Some(EffectId(5)), None);
        one_window_and_qualifier(&mut args);

        let unavailable = plan_secondary_correction(
            &document,
            TimelineRevision(0),
            &MatteAnalysisDouble::default(),
            &args,
        )
        .expect("valid plan")
        .predicted_coverage
        .expect("the field is always published");
        assert_eq!(unavailable["statistics"], Value::Null);
        assert_eq!(unavailable["reason"], MATTE_PROOF_UNAVAILABLE);
        assert!(
            unavailable["recovery_action"]
                .as_str()
                .unwrap()
                .contains("inspect_grade_matte")
        );

        // A 4 × 2 coverage: the left half fully covered, the right half not.
        // Hand-derived: 4 covered of 8, so 5000 basis points.
        let analysis = MatteAnalysisDouble {
            coverage: Some(coverage_raster(4, 2, |x, _| if x < 2 { 255 } else { 0 })),
            monitor: None,
        };
        let measured = plan_secondary_correction(&document, TimelineRevision(0), &analysis, &args)
            .expect("valid plan")
            .predicted_coverage
            .expect("the field is always published");
        assert_eq!(measured["reason"], Value::Null);
        assert_eq!(measured["covered_pixel_count"], 4);
        assert_eq!(measured["statistics"]["total_pixel_count"], 8);
        assert_eq!(measured["statistics"]["full_pixel_count"], 4);
        assert_eq!(measured["statistics"]["partial_pixel_count"], 0);
        assert_eq!(measured["statistics"]["covered_basis_points"], 5_000);
        assert_eq!(
            measured["matte_threshold"],
            kinewright_core::MATTE_SCOPE_THRESHOLD
        );
        assert_eq!(measured["raster"], json!({"width": 4, "height": 2}));
        // The scratch document the coverage was measured on is not the
        // analyzed one: the source clip still carries no matte parameter.
        assert!(
            document.clip(ClipId(1)).unwrap().effects[0]
                .parameters
                .keys()
                .all(|name| !name.starts_with("matte_"))
        );
    }

    /// CC5 §7: `sample_roi` returns measured evidence, and only
    /// `derive_qualifier_from_sample` turns it into a proposal — by the pinned
    /// formula and no other.
    ///
    /// The monitor double returns a 4 × 2 raster whose left half is a
    /// saturated red and whose right half is mid grey. The ROI names the left
    /// half only, so every measured pixel is that one red.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn secondary_plan_sample_roi_is_evidence_until_derivation_is_requested() {
        let mut document = document();
        document.tracks[0].clips[0]
            .effects
            .push(wheels_node(5, BTreeMap::new()));
        // Display-coded (200, 40, 40) on the left, (128, 128, 128) on the
        // right. The plan measures only the left half.
        let mut pixels = Vec::new();
        for _ in 0..2 {
            pixels.extend_from_slice(&[200, 40, 40, 255]);
            pixels.extend_from_slice(&[200, 40, 40, 255]);
            pixels.extend_from_slice(&[128, 128, 128, 255]);
            pixels.extend_from_slice(&[128, 128, 128, 255]);
        }
        let analysis = MatteAnalysisDouble {
            coverage: None,
            monitor: Some(kinewright_core::RgbaImage {
                width: 4,
                height: 2,
                pixels,
            }),
        };

        let mut args = secondary_args(Some(EffectId(5)), None);
        args.windows = Some(vec![MatteWindowRequest {
            center_x: Some(2_500),
            ..MatteWindowRequest::default()
        }]);
        args.sample_roi = Some(MatteSampleRoi {
            x: 0.0,
            y: 0.0,
            width: 0.5,
            height: 1.0,
        });

        // Without the opt-in the sample is evidence only: no qualifier
        // parameter is proposed at all.
        let evidence_only =
            plan_secondary_correction(&document, TimelineRevision(0), &analysis, &args)
                .expect("valid plan");
        let sample = evidence_only
            .sample_evidence
            .clone()
            .expect("sample_roi always publishes its measurement");
        assert_eq!(sample["visible_pixel_count"], 4);
        assert_eq!(sample["total_pixel_count"], 4);
        assert_eq!(sample["achromatic_pixel_count"], 0);
        assert_eq!(
            sample["measured_pixel_rect"],
            json!({"x": 0, "y": 0, "width": 2, "height": 2})
        );
        assert_eq!(sample["derive_qualifier_from_sample"], false);
        assert_eq!(sample["derived_qualifier"], Value::Null);
        assert!(
            evidence_only
                .requested_parameters
                .keys()
                .all(|name| !name.starts_with("matte_hue")
                    && !name.starts_with("matte_saturation")
                    && !name.starts_with("matte_luma")),
            "evidence must not become a proposal without the explicit opt-in"
        );
        // The measured hue of a red-dominant pixel sits near 0 degrees: with
        // `max == r` the hue is `60 * ((g - b) / C mod 6)`, and `g == b` here,
        // so the hue is exactly 0.
        assert_eq!(sample["hue_median_centidegrees"], 0);
        let saturation = sample["saturation_basis_points"]["median"]
            .as_i64()
            .unwrap();
        assert!(
            (5_000..=10_000).contains(&saturation),
            "a saturated red must measure a high saturation, was {saturation}"
        );

        // With the opt-in, CC5 §7's pinned formula produces the qualifier.
        args.derive_qualifier_from_sample = true;
        let derived = plan_secondary_correction(&document, TimelineRevision(0), &analysis, &args)
            .expect("valid plan");
        let requested = &derived.requested_parameters;
        assert_eq!(requested["matte_qualifier_enabled"], 1);
        assert_eq!(requested["matte_hue_center_centidegrees"], 0);
        assert_eq!(
            requested["matte_hue_width_centidegrees"],
            MATTE_SAMPLE_HUE_WIDTH_CENTIDEGREES
        );
        assert_eq!(
            requested["matte_hue_softness_centidegrees"],
            MATTE_SAMPLE_SOFTNESS
        );
        assert_eq!(
            requested["matte_saturation_softness_basis_points"],
            MATTE_SAMPLE_SOFTNESS
        );
        assert_eq!(
            requested["matte_luma_softness_basis_points"],
            MATTE_SAMPLE_SOFTNESS
        );
        // The bands are the measured percentiles widened by the pinned margin
        // and clamped to the descriptor range.
        let p10 = sample["saturation_basis_points"]["p10"].as_i64().unwrap();
        let p90 = sample["saturation_basis_points"]["p90"].as_i64().unwrap();
        assert_eq!(
            requested["matte_saturation_low_basis_points"],
            (p10 - MATTE_SAMPLE_BAND_MARGIN_BASIS_POINTS).max(0)
        );
        assert_eq!(
            requested["matte_saturation_high_basis_points"],
            (p90 + MATTE_SAMPLE_BAND_MARGIN_BASIS_POINTS).min(10_000)
        );
        // The formula is published next to the numbers it produced.
        let formula = derived.sample_evidence.unwrap()["formula"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(formula.contains("hue_width = 1500"));
        assert!(formula.contains("p10 - 1000"));
        assert!(formula.contains("p90 + 1000"));

        // An explicit qualifier field always beats a derived one: the caller's
        // number is a request, the sample is only evidence.
        args.qualifier = Some(MatteQualifierRequest {
            hue_center: Some(12_000),
            ..MatteQualifierRequest::default()
        });
        let explicit = plan_secondary_correction(&document, TimelineRevision(0), &analysis, &args)
            .expect("valid plan");
        assert_eq!(
            explicit.requested_parameters["matte_hue_center_centidegrees"],
            12_000
        );
    }

    /// CC5 §7: an ROI outside the frame, or one containing no visible pixel, is
    /// a typed refusal with `field`/`observed`/`allowed`.
    #[test]
    fn secondary_plan_rejects_an_unusable_sample_roi() {
        let mut document = document();
        document.tracks[0].clips[0]
            .effects
            .push(wheels_node(5, BTreeMap::new()));
        let analysis = MatteAnalysisDouble {
            coverage: None,
            monitor: Some(kinewright_core::RgbaImage {
                width: 4,
                height: 2,
                // Fully transparent: CC2's rule says a transparent pixel is not
                // part of the population, and partial alpha is never a weight.
                pixels: vec![0; 32],
            }),
        };

        let mut args = secondary_args(Some(EffectId(5)), None);
        args.windows = Some(vec![MatteWindowRequest {
            center_x: Some(2_500),
            ..MatteWindowRequest::default()
        }]);
        args.sample_roi = Some(MatteSampleRoi {
            x: 0.8,
            y: 0.0,
            width: 0.5,
            height: 1.0,
        });
        let error = plan_secondary_correction(&document, TimelineRevision(0), &analysis, &args)
            .unwrap_err();
        assert_eq!(error.code(), "matte_sample_roi_invalid");
        assert_eq!(error.details()["field"], "sample_roi");
        assert!(
            error.details()["allowed"]
                .as_str()
                .unwrap()
                .contains("x + width")
        );

        args.sample_roi = Some(MatteSampleRoi {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        });
        let error = plan_secondary_correction(&document, TimelineRevision(0), &analysis, &args)
            .unwrap_err();
        assert_eq!(error.code(), "matte_sample_roi_empty");
        assert_eq!(error.details()["observed"]["visible_pixel_count"], 0);
        assert!(
            error.details()["recovery_action"]
                .as_str()
                .unwrap()
                .contains("visible-pixel rule")
        );
    }

    // -----------------------------------------------------------------------
    // CC5 §7 — the `matte` manifest object
    // -----------------------------------------------------------------------

    /// CC5 §7: absent entirely when the node carries no matte, so every CC4
    /// manifest is byte-unchanged.
    #[test]
    fn matte_manifest_object_is_absent_until_the_node_carries_a_matte() {
        let looks = LookAssetContext::default();
        let plain = wheels_node(5, integers([("gain_red_thousandths", 1_200)]));
        let value = color_node_value(0, &plain, &looks).expect("a colour node");
        assert!(
            value.get("matte").is_none(),
            "a CC4 manifest entry must be byte-unchanged"
        );
        assert!(matte_manifest_value(&plain).is_none());

        // CC5 §2.6: `matte_enabled = 0` is still no matte at all, whatever the
        // other 46 integers say.
        let disabled = wheels_node(
            5,
            integers([("matte_enabled", 0), ("matte_window_count", 2)]),
        );
        assert!(matte_manifest_value(&disabled).is_none());

        // Enabled but selecting everything at full strength is also inactive.
        let neutral = wheels_node(5, integers([("matte_enabled", 1)]));
        assert!(matte_manifest_value(&neutral).is_none());
    }

    /// CC5 §7: the compact integer object, with `windows` truncated to
    /// `window_count`.
    #[test]
    fn matte_manifest_object_reports_the_resolved_matte_and_truncates_windows() {
        let looks = LookAssetContext::default();
        let node = wheels_node(
            5,
            integers([
                ("matte_enabled", 1),
                ("matte_window_count", 1),
                ("matte_combine_token", 1),
                ("matte_mix_basis_points", 7_500),
                ("matte_qualifier_enabled", 1),
                ("matte_hue_center_centidegrees", 3_000),
                ("matte_hue_width_centidegrees", 1_500),
                ("matte_window0_shape_token", 2),
                ("matte_window0_center_x_basis_points", 6_000),
                // Window 1 is stored but past the count, so it renders nothing
                // and must not be published.
                ("matte_window1_center_x_basis_points", 1_234),
            ]),
        );
        let value = color_node_value(0, &node, &looks).expect("a colour node");
        let matte = &value["matte"];

        assert_eq!(matte["enabled"], true);
        assert_eq!(matte["active"], true);
        assert_eq!(matte["inactive_reason"], Value::Null);
        assert_eq!(matte["window_count"], 1);
        assert_eq!(matte["combine"], "intersection");
        assert_eq!(matte["combine_token"], 1);
        assert_eq!(matte["invert"], 0);
        assert_eq!(matte["mix_basis_points"], 7_500);
        assert_eq!(matte["qualifier"]["enabled"], true);
        assert_eq!(matte["qualifier"]["hue_center_centidegrees"], 3_000);
        assert_eq!(matte["qualifier"]["hue_width_centidegrees"], 1_500);
        assert_eq!(matte["qualifier"]["hue_leg_disabled"], false);
        // Omitted qualifier controls resolve to their descriptor neutrals.
        assert_eq!(matte["qualifier"]["saturation_low_basis_points"], 0);
        assert_eq!(matte["qualifier"]["saturation_high_basis_points"], 10_000);

        let windows = matte["windows"].as_array().expect("a window list");
        assert_eq!(windows.len(), 1, "windows truncate to window_count");
        assert_eq!(windows[0]["shape"], "ellipse");
        assert_eq!(windows[0]["shape_token"], 2);
        assert_eq!(windows[0]["center_x_basis_points"], 6_000);
        assert_eq!(windows[0]["center_y_basis_points"], 5_000);
        assert_eq!(windows[0]["half_width_basis_points"], 2_500);
    }

    /// CC5 §2.6 rule 2: a zero mix, or an inverted empty matte, makes the whole
    /// node the exact identity, reported as `matte_excluded`.
    #[test]
    fn matte_manifest_reports_the_node_excluded_by_its_matte() {
        let looks = LookAssetContext::default();
        // A non-neutral gain, so the node is not already the identity: Core
        // tests the matte rule *last*, and a bypassed, neutral, or unbound node
        // keeps the reason it already had.
        for mut parameters in [
            integers([("matte_enabled", 1), ("matte_mix_basis_points", 0)]),
            integers([("matte_enabled", 1), ("matte_invert", 1)]),
        ] {
            parameters.insert(
                "gain_red_thousandths".to_owned(),
                ParamValue::Integer(1_200),
            );
            let node = wheels_node(5, parameters);
            let value = color_node_value(0, &node, &looks).expect("a colour node");
            assert_eq!(value["active"], false);
            assert_eq!(value["inactive_reason"], "matte_excluded");
            assert_eq!(value["matte"]["active"], false);
            assert_eq!(value["matte"]["inactive_reason"], "matte_excluded");
        }
    }

    /// CC5 §2.6: a band whose low edge resolved above its high edge selects
    /// nothing, and the manifest says so with the same code Core QA uses.
    #[test]
    fn matte_manifest_warns_when_a_qualifier_band_is_inverted() {
        let looks = LookAssetContext::default();
        let node = wheels_node(
            5,
            integers([
                ("matte_enabled", 1),
                ("matte_qualifier_enabled", 1),
                ("matte_saturation_low_basis_points", 9_000),
                ("matte_saturation_high_basis_points", 1_000),
            ]),
        );
        let value = color_node_value(0, &node, &looks).expect("a colour node");
        let warning = value["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|warning| warning["code"] == "matte_band_inverted_by_automation")
            .expect("the inverted band must be reported");
        assert_eq!(warning["bands"], json!(["saturation"]));
        assert_eq!(warning["effect_id"], 5);
        assert_eq!(value["matte"]["degenerate_bands"], json!(["saturation"]));
    }

    /// CC5 §2.2, M36: no planner surface enumerates the 47 matte parameters,
    /// and the legend states every bound the descriptors carry.
    #[test]
    fn planner_listings_summarise_the_matte_and_never_enumerate_it() {
        // The base control lists are unchanged by CC5.
        assert_eq!(primary_parameter_documentation().len(), 10);
        assert_eq!(color_node_parameter_documentation("color_wheels").len(), 13);
        for entry in primary_parameter_documentation()
            .iter()
            .chain(color_node_parameter_documentation("color_wheels").iter())
        {
            assert!(
                !entry["name"].as_str().unwrap().starts_with("matte_"),
                "an allowed-parameter list must never enumerate the matte"
            );
        }

        // The one-line pointer rides on every planner summary, so a caller is
        // told the capability exists and which tool expands it. It is
        // deliberately terse: `plan_color_wheels` spends 981 of its M36
        // kilobyte on thirteen descriptor controls before the pointer is
        // appended, so the pointer names the capability and the one tool that
        // expands it and nothing else. `inspect_grade_matte` is reachable from
        // there and from the legend, and is not repeated here.
        assert!(
            matte_parameter_pointer().len() <= 42,
            "the pointer is {} bytes; plan_color_wheels has only 42 to spare",
            matte_parameter_pointer().len()
        );
        for summary in [
            primary_parameter_summary(),
            color_wheels_parameter_summary(),
            lut_node_parameter_summary(ColorNodeKind::CreativeLook),
        ] {
            assert!(summary.contains("plan_secondary_correction"));
            assert!(summary.contains("matte"));
            assert!(
                !summary.contains("matte_window0_"),
                "a summary must never enumerate a generated window parameter"
            );
            assert!(summary.len() < 1_024, "summary is {} bytes", summary.len());
        }
        // `plan_secondary_correction`'s own description points at the two
        // surfaces that enumerate the legend instead of repeating it, and does
        // not recommend itself.
        let reference = matte_legend_reference();
        assert!(reference.contains("add_effect"));
        assert!(reference.contains("details.matte_parameters"));
        assert!(!reference.contains("plan_secondary_correction"));
        assert!(!reference.contains("matte_window{j}_*"));
        // CC5 §2.1: `technical_lut` carries no matte, so it gets no pointer.
        let technical = lut_node_parameter_summary(ColorNodeKind::TechnicalLut);
        assert!(!technical.contains("plan_secondary_correction"));

        // The full legend appears where a caller writes raw parameters.
        let legend = matte_parameter_legend();
        assert!(legend.len() < 1_024, "legend is {} bytes", legend.len());
        assert!(legend.contains("matte_window{j}_*"));
        assert!(legend.contains("-10000..=20000"));
        assert!(!legend.contains("matte_window0_"));

        // A rejection carries the base list *and* the legend, so an agent that
        // reached for a matte name is pointed at the right surface.
        let error = plan_color_wheels(
            &document(),
            TimelineRevision(0),
            &wheels_args(BTreeMap::from([("matte_glow".to_owned(), 1)])),
        )
        .unwrap_err();
        let details = error.details();
        assert_eq!(details["allowed"].as_array().unwrap().len(), 13);
        assert!(
            details["matte_parameters"]
                .as_str()
                .unwrap()
                .contains("plan_secondary_correction")
        );
    }

    /// A brute-force circular median, written the obvious O(n²) way so the
    /// closed-form sweep is checked against a different algorithm rather than
    /// against itself.
    fn brute_force_circular_median(hues: &[i64]) -> Option<i64> {
        let mut sorted = hues.to_vec();
        sorted.sort_unstable();
        let mut best: Option<(i64, i64)> = None;
        for candidate in sorted.iter().copied() {
            let cost = sorted
                .iter()
                .map(|hue| {
                    let delta = (hue - candidate).abs();
                    delta.min(36_000 - delta)
                })
                .sum::<i64>();
            if best.is_none_or(|(best_cost, _)| cost < best_cost) {
                best = Some((cost, candidate));
            }
        }
        best.map(|(_, hue)| hue)
    }

    /// CC5 §7: the circular median is the sample minimising summed circular
    /// distance, and it is computed in O(n log n) because it runs over every
    /// chromatic pixel of a full-resolution ROI.
    #[test]
    fn circular_median_agrees_with_brute_force_and_survives_two_million_samples() {
        assert_eq!(circular_median_centidegrees(&mut []), None);

        // A deterministic, dependency-free generator: a 64-bit LCG. Random-ish
        // inputs, reproducible failures.
        let mut state = 0x2545_F491_4F6C_DD1D_u64;
        let mut next = |modulus: u64| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            i64::try_from((state >> 33) % modulus).unwrap()
        };

        let mut cases: Vec<Vec<i64>> = vec![
            vec![0],
            vec![0, 18_000],
            vec![35_900, 100],
            // The red seam, straddled: 359° and 1°. A plain median answers
            // 18000, the opposite hue; the circular median must answer one of
            // the reds.
            vec![35_900, 35_950, 35_990, 10, 60, 100],
            vec![35_990, 10],
            // Two antipodal clusters, where the tie-break matters.
            vec![0, 0, 18_000, 18_000],
            // Everything on one point.
            vec![12_345; 9],
        ];
        for size in [1_usize, 2, 3, 5, 17, 64, 200] {
            // Uniform over the whole circle.
            cases.push((0..size).map(|_| next(36_000)).collect());
            // A tight cluster straddling the seam.
            cases.push(
                (0..size)
                    .map(|_| (next(1_200) + 35_400).rem_euclid(36_000))
                    .collect(),
            );
            // Two clusters, one of them across the seam.
            cases.push(
                (0..size)
                    .map(|index| {
                        if index % 2 == 0 {
                            (next(600) + 35_700).rem_euclid(36_000)
                        } else {
                            next(600) + 12_000
                        }
                    })
                    .collect(),
            );
        }

        for case in &cases {
            let mut subject = case.clone();
            let measured = circular_median_centidegrees(&mut subject);
            assert_eq!(
                measured,
                brute_force_circular_median(case),
                "circular median disagreed with brute force on {case:?}"
            );
        }

        // The seam case, spelled out: every sample is within 1° of red, so the
        // answer must be a red and never the opposite hue.
        let mut seam = vec![35_900, 35_950, 35_990, 10, 60, 100];
        let median = circular_median_centidegrees(&mut seam).unwrap();
        assert!(
            median >= 35_000 || median <= 1_000,
            "the seam median must be a red, was {median}"
        );

        // Two million samples is the order of a 1080p full-frame ROI. The old
        // double loop took tens of minutes; this must be a fraction of a
        // second. The threshold is generous so a loaded CI box does not flake,
        // but it is four orders of magnitude below quadratic.
        let mut large = (0..2_000_000_i64)
            .map(|index| (index * 7_919) % 36_000)
            .collect::<Vec<_>>();
        let started = std::time::Instant::now();
        let measured = circular_median_centidegrees(&mut large).expect("a median exists");
        let elapsed = started.elapsed();
        assert!((0..36_000).contains(&measured));
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "two million samples took {elapsed:?}; the sweep is not O(n log n)"
        );
    }

    /// CC5 §2.2/§7: a primary node's manifests describe the ten CC1 controls
    /// and nothing else. The matte is described by the `matte` object, which
    /// CC5 §7 makes absent entirely when the node carries none — so a CC4
    /// manifest is byte-unchanged.
    #[test]
    fn primary_node_manifests_never_enumerate_the_matte() {
        let mut document = document();
        document.tracks[0].clips[0].effects.push(Effect {
            id: EffectId(7),
            name: PRIMARY_CORRECTION_EFFECT_NAME.to_owned(),
            parameters: integers([("exposure_milli_stops", 250)]),
            keyframes: BTreeMap::new(),
        });
        let effects = &document.tracks[0].clips[0].effects;
        let looks = LookAssetContext::new(&document, None, None);

        let nodes = color_node_manifest(effects, &looks);
        let parameters = nodes[0]["parameters"].as_object().unwrap();
        assert_eq!(
            parameters.len(),
            10,
            "a matte-free primary node reports its ten CC1 controls: {:?}",
            parameters.keys().collect::<Vec<_>>()
        );
        assert!(parameters.keys().all(|name| !name.starts_with("matte_")));
        assert_eq!(parameters["exposure_milli_stops"], 250);
        assert!(
            nodes[0].get("matte").is_none(),
            "a node with no matte carries no matte object"
        );

        let chain = effect_chain_manifest(effects);
        let primary_parameters = chain[0]["primary_parameters"].as_object().unwrap();
        assert_eq!(
            primary_parameters.len(),
            10,
            "effect_chain[].primary_parameters reports the same ten: {:?}",
            primary_parameters.keys().collect::<Vec<_>>()
        );
        assert!(
            primary_parameters
                .keys()
                .all(|name| !name.starts_with("matte_"))
        );

        // Setting a real matte adds the `matte` object and still adds no
        // parameter: one window, so it is not the inactive neutral matte.
        for (name, value) in [("matte_enabled", 1), ("matte_window_count", 1)] {
            document.tracks[0].clips[0].effects[0]
                .parameters
                .insert(name.to_owned(), ParamValue::Integer(value));
        }
        let effects = &document.tracks[0].clips[0].effects;
        let matted = color_node_manifest(effects, &looks);
        assert_eq!(matted[0]["parameters"].as_object().unwrap().len(), 10);
        assert_eq!(matted[0]["matte"]["enabled"], true);
        assert_eq!(matted[0]["matte"]["window_count"], 1);
        assert_eq!(
            effect_chain_manifest(effects)[0]["primary_parameters"]
                .as_object()
                .unwrap()
                .len(),
            10
        );
    }

    /// CC5 §7: "explicit beats derived" applies to `qualifier.enabled` too. A
    /// caller who asks for the bands to be derived *and* says the leg is off
    /// gets the bands and an off leg, not a silent override.
    #[test]
    fn secondary_plan_explicit_qualifier_disable_survives_a_derived_qualifier() {
        let mut document = document();
        document.tracks[0].clips[0]
            .effects
            .push(wheels_node(5, BTreeMap::new()));
        let analysis = MatteAnalysisDouble {
            coverage: None,
            monitor: Some(kinewright_core::RgbaImage {
                width: 2,
                height: 1,
                pixels: vec![200, 40, 40, 255, 200, 40, 40, 255],
            }),
        };
        let mut args = secondary_args(Some(EffectId(5)), None);
        args.sample_roi = Some(MatteSampleRoi {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        });
        args.derive_qualifier_from_sample = true;
        args.qualifier = Some(MatteQualifierRequest {
            enabled: Some(false),
            ..MatteQualifierRequest::default()
        });

        let plan = plan_secondary_correction(&document, TimelineRevision(0), &analysis, &args)
            .expect("valid plan");
        assert_eq!(
            plan.requested_parameters["matte_qualifier_enabled"], 0,
            "an explicit qualifier.enabled: false must beat the derived enable"
        );
        // The derived bands are still proposed: only the switch was refused.
        assert!(
            plan.requested_parameters
                .contains_key("matte_hue_center_centidegrees")
        );

        // Without the explicit `false`, deriving turns the leg on.
        args.qualifier = None;
        let derived = plan_secondary_correction(&document, TimelineRevision(0), &analysis, &args)
            .expect("valid plan");
        assert_eq!(derived.requested_parameters["matte_qualifier_enabled"], 1);

        // An empty `qualifier: {}` alongside a derivation is not an explicit
        // "off" either: the derived bands are the request to enable the leg.
        args.qualifier = Some(MatteQualifierRequest::default());
        let empty = plan_secondary_correction(&document, TimelineRevision(0), &analysis, &args)
            .expect("valid plan");
        assert_eq!(empty.requested_parameters["matte_qualifier_enabled"], 1);
    }

    /// CC5 §5.1: the Hold-only refusal is about a *dead write*, so it fires
    /// only for a token the plan would actually move. Every CC5 request injects
    /// `matte_enabled: 1`, and refusing on the whole request made a node with a
    /// `matte_enabled` Hold curve unable to receive any plan at all.
    #[test]
    fn secondary_plan_moves_a_window_on_a_node_whose_matte_enabled_is_keyframed() {
        let hold = |value: i64| kinewright_core::AutomationCurve {
            keyframes: vec![kinewright_core::Keyframe {
                at: TimeCode(0),
                value,
                interpolation: kinewright_core::KeyframeInterpolation::Hold,
            }],
        };
        let mut document = document();
        let mut node = wheels_node(
            5,
            integers([
                ("matte_enabled", 1),
                ("matte_window_count", 1),
                ("matte_invert", 0),
            ]),
        );
        node.keyframes.insert("matte_enabled".to_owned(), hold(1));
        node.keyframes.insert("matte_invert".to_owned(), hold(0));
        document.tracks[0].clips[0].effects.push(node);
        let analysis = MatteAnalysisDouble::default();

        // Only the window centre moves. `matte_enabled` and `matte_window_count`
        // are already at the requested values, so the plan writes neither, and
        // the curve on `matte_enabled` overrides nothing.
        let mut args = secondary_args(Some(EffectId(5)), None);
        args.windows = Some(vec![MatteWindowRequest {
            center_x: Some(7_000),
            ..MatteWindowRequest::default()
        }]);
        let plan = plan_secondary_correction(&document, TimelineRevision(0), &analysis, &args)
            .expect("a window move must not be refused by an unrelated Hold curve");
        assert_eq!(
            plan.operations,
            vec![Operation::SetEffectParam {
                clip: ClipId(1),
                effect: EffectId(5),
                name: "matte_window0_center_x_basis_points".to_owned(),
                value: ParamValue::Integer(7_000),
            }]
        );
        // The keyframed control is still called out, as every other planner
        // calls out automation it did not write.
        assert!(
            plan.warnings
                .iter()
                .any(|warning| warning.contains("matte_enabled")),
            "{:?}",
            plan.warnings
        );

        // Toggling `matte_invert` against its own Hold curve is still refused:
        // that write really would be dead.
        args.invert = Some(true);
        let error = plan_secondary_correction(&document, TimelineRevision(0), &analysis, &args)
            .unwrap_err();
        assert_eq!(error.code(), "matte_hold_only_parameter_keyframed");
        assert_eq!(error.details()["field"], "matte_invert");

        // And a Hold curve that already holds exactly the requested value is
        // not a dead write, because the render already agrees with the plan.
        args.invert = Some(false);
        plan_secondary_correction(&document, TimelineRevision(0), &analysis, &args)
            .expect("a curve that already holds the requested value is not overridden");
    }
}
