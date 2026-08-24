//! CC2 colour scopes and shot-matching evidence.
//!
//! The media backend owns decoding and the managed monitor proof.  This module
//! deliberately owns only the bounded, read-only agent workflow: it validates
//! a request, obtains an immutable raster, computes deterministic scope data,
//! and returns evidence that can be inspected before any edit is committed.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::Arc,
};

use kinewright_core::{
    Analysis, AssetId, Clip, ClipContent, ClipId, Document, MediaAvailabilityKind, MediaError,
    MonitorProofMetadata, NormalizedRoi as CoreNormalizedRoi, RgbaImage, SCOPE_BASIS_POINTS,
    ScopeEvidence, ScopeFrame, ScopeRequest as CoreScopeRequest,
    ScopeResolution as CoreScopeResolution, ScopeStage, TimeCode, TimelineRevision, TrackKind,
    apply_batch, compare_scope_evidence, measure_scopes,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::color_status::{PrimaryCorrectionPlanArgs, plan_primary_correction};

/// Hard upper bounds are part of the agent contract.  Scope tools must fail
/// closed rather than allowing a caller to accidentally request a long-
/// running full-resolution scan.
pub(crate) const MAX_SCOPE_SAMPLES: usize = 64;
const MAX_SCOPE_WIDTH: u32 = 4_096;
const MIN_PROXY_WIDTH: u32 = 32;
const DEFAULT_PROXY_WIDTH: u32 = 512;
const DEFAULT_HISTOGRAM_BINS: usize = 64;
const MIN_HISTOGRAM_BINS: usize = 16;
const MAX_HISTOGRAM_BINS: usize = 256;
const DEFAULT_WAVEFORM_COLUMNS: usize = 64;
const MAX_WAVEFORM_COLUMNS: usize = 256;
const MAX_VECTOR_BINS: usize = 64;

/// Canonical request envelope for `get_video_scopes_v2`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct VideoScopesV2Args {
    /// Optional revision returned by `get_timeline_state`.  When supplied,
    /// the immutable evidence snapshot must still be at this revision.
    #[serde(default)]
    pub expected_revision: Option<TimelineRevision>,
    /// Named managed-pipeline stage.  The current backend can prove the
    /// post-compositor monitor boundary; aliases for that named boundary are
    /// accepted and retained verbatim in provenance.
    pub stage: String,
    /// One exact project frame.  Omit this when `range` or `frames` is used.
    #[serde(default, alias = "frame")]
    pub timecode: Option<TimeCode>,
    /// Explicit half-open project-frame range.  `step_frames` controls the
    /// bounded sample spacing.
    #[serde(default, alias = "temporal_range")]
    pub range: Option<ScopeRangeArgs>,
    /// Explicit project frames.  They are never re-spaced or silently
    /// truncated.
    #[serde(default, alias = "project_frames")]
    pub frames: Option<Vec<TimeCode>>,
    /// Positive spacing for a half-open range.  When omitted, the range is
    /// deterministically distributed across the bounded sample budget.
    #[serde(default, alias = "sampling_step_frames")]
    pub step_frames: Option<TimeCode>,
    /// Normalized geometric region.  Omission means the full raster.
    #[serde(default)]
    pub roi: Option<ScopeRoiArgs>,
    /// `full_resolution` (default) or `proxy`.  A proxy request is explicit
    /// and is surfaced as such in every provenance record.
    #[serde(default, alias = "sampling_resolution")]
    pub resolution: Option<String>,
    /// Explicit proxy opt-in accepted for clients that model this as a bool.
    #[serde(default, alias = "use_proxy", alias = "proxy")]
    pub proxy_sampling: bool,
    /// Maximum proxy width.  It is ignored for full-resolution proofs.
    #[serde(default)]
    pub max_width: Option<u32>,
    /// Histogram/vector bin count.  It is bounded and retained in the result.
    #[serde(default)]
    pub bins: Option<u16>,
    /// Waveform/parade column count.  It is bounded and retained in the
    /// result.
    #[serde(default)]
    pub columns: Option<u16>,
}

/// A half-open exact project-frame range.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct ScopeRangeArgs {
    pub start: TimeCode,
    pub end: TimeCode,
}

/// Normalized geometric ROI.  The compact x/y/width/height form is the
/// canonical contract; left/top/right/bottom is accepted for clients that
/// already use bounds in their visual tools.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct ScopeRoiArgs {
    #[serde(default)]
    pub x: Option<f64>,
    #[serde(default)]
    pub y: Option<f64>,
    #[serde(default)]
    pub width: Option<f64>,
    #[serde(default)]
    pub height: Option<f64>,
    #[serde(default)]
    pub left: Option<f64>,
    #[serde(default)]
    pub top: Option<f64>,
    #[serde(default)]
    pub right: Option<f64>,
    #[serde(default)]
    pub bottom: Option<f64>,
}

/// Evidence-only shot analysis request.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct AnalyzeColorShotArgs {
    pub expected_revision: TimelineRevision,
    /// Stable visual media clip id representing the shot.
    pub clip_id: ClipId,
    /// Named managed-pipeline stage.
    #[serde(default = "default_scope_stage")]
    pub stage: String,
    #[serde(default, alias = "frame")]
    pub timecode: Option<TimeCode>,
    #[serde(default, alias = "temporal_range")]
    pub range: Option<ScopeRangeArgs>,
    #[serde(default, alias = "project_frames")]
    pub frames: Option<Vec<TimeCode>>,
    #[serde(default, alias = "sampling_step_frames")]
    pub step_frames: Option<TimeCode>,
    #[serde(default)]
    pub roi: Option<ScopeRoiArgs>,
    #[serde(default, alias = "sampling_resolution")]
    pub resolution: Option<String>,
    #[serde(default, alias = "use_proxy", alias = "proxy")]
    pub proxy_sampling: bool,
    #[serde(default)]
    pub max_width: Option<u32>,
    #[serde(default)]
    pub bins: Option<u16>,
    #[serde(default)]
    pub columns: Option<u16>,
}

/// A clip/frame selector used by the explicit reference and candidate forms.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct ShotSelectorArgs {
    pub clip_id: ClipId,
    #[serde(default, alias = "frame")]
    pub timecode: Option<TimeCode>,
}

/// Evidence-only shot-match planner.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct PlanShotMatchArgs {
    pub expected_revision: TimelineRevision,
    /// Preferred compact form for the one explicit reference shot.
    #[serde(default)]
    pub reference_clip_id: Option<ClipId>,
    /// Structured reference form retained in the response.
    #[serde(default)]
    pub reference_shot: Option<ShotSelectorArgs>,
    /// Preferred compact candidate list.
    #[serde(default)]
    pub candidate_clip_ids: Vec<ClipId>,
    /// Structured candidate list for callers that need per-shot frames.
    #[serde(default)]
    pub candidate_shots: Vec<ShotSelectorArgs>,
    #[serde(default = "default_scope_stage")]
    pub stage: String,
    #[serde(default)]
    pub roi: Option<ScopeRoiArgs>,
    #[serde(default, alias = "sampling_resolution")]
    pub resolution: Option<String>,
    #[serde(default, alias = "use_proxy", alias = "proxy")]
    pub proxy_sampling: bool,
    #[serde(default)]
    pub max_width: Option<u32>,
    #[serde(default)]
    pub bins: Option<u16>,
    #[serde(default)]
    pub columns: Option<u16>,
}

fn default_scope_stage() -> String {
    "post_compositor".to_owned()
}

#[derive(Debug, Clone, Copy)]
struct NormalizedRoi {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

impl NormalizedRoi {
    const FULL: Self = Self {
        left: 0.0,
        top: 0.0,
        right: 1.0,
        bottom: 1.0,
    };

    fn validate(args: Option<&ScopeRoiArgs>) -> Result<Self, ScopeError> {
        let Some(args) = args else {
            return Ok(Self::FULL);
        };
        let rectangular = [args.x, args.y, args.width, args.height];
        let bounds = [args.left, args.top, args.right, args.bottom];
        let has_rectangular = rectangular.iter().any(Option::is_some);
        let has_bounds = bounds.iter().any(Option::is_some);
        if has_rectangular && has_bounds {
            return Err(ScopeError::invalid_roi(
                "ROI must use either x/y/width/height or left/top/right/bottom, not both",
            ));
        }
        let (left, top, right, bottom) = if has_bounds {
            (
                args.left
                    .ok_or_else(|| ScopeError::invalid_roi("ROI left is required"))?,
                args.top
                    .ok_or_else(|| ScopeError::invalid_roi("ROI top is required"))?,
                args.right
                    .ok_or_else(|| ScopeError::invalid_roi("ROI right is required"))?,
                args.bottom
                    .ok_or_else(|| ScopeError::invalid_roi("ROI bottom is required"))?,
            )
        } else if has_rectangular {
            let x = args
                .x
                .ok_or_else(|| ScopeError::invalid_roi("ROI x is required"))?;
            let y = args
                .y
                .ok_or_else(|| ScopeError::invalid_roi("ROI y is required"))?;
            let width = args
                .width
                .ok_or_else(|| ScopeError::invalid_roi("ROI width is required"))?;
            let height = args
                .height
                .ok_or_else(|| ScopeError::invalid_roi("ROI height is required"))?;
            (x, y, x + width, y + height)
        } else {
            return Err(ScopeError::invalid_roi(
                "ROI must provide all four normalized coordinates",
            ));
        };
        if ![left, top, right, bottom]
            .iter()
            .all(|value| value.is_finite())
        {
            return Err(ScopeError::invalid_roi("ROI coordinates must be finite"));
        }
        if left < 0.0 || top < 0.0 || right > 1.0 || bottom > 1.0 {
            return Err(ScopeError::invalid_roi(
                "ROI coordinates must be normalized to 0..=1",
            ));
        }
        if right <= left || bottom <= top {
            return Err(ScopeError::invalid_roi(
                "ROI must have positive width and height",
            ));
        }
        Ok(Self {
            left,
            top,
            right,
            bottom,
        })
    }

    fn value(self) -> Value {
        json!({
            "left": self.left,
            "top": self.top,
            "right": self.right,
            "bottom": self.bottom,
            "coordinate_system": "normalized_half_open",
        })
    }

    fn core(self) -> Result<CoreNormalizedRoi, ScopeError> {
        let basis = normalized_basis_points;
        let roi = CoreNormalizedRoi::new(
            basis(self.left),
            basis(self.top),
            basis(self.right - self.left),
            basis(self.bottom - self.top),
        );
        roi.validate().map_err(|error| {
            ScopeError::invalid_roi(format!("core normalized ROI validation failed: {error}"))
        })?;
        Ok(roi)
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn normalized_basis_points(value: f64) -> u32 {
    (value * f64::from(SCOPE_BASIS_POINTS))
        .round()
        .clamp(0.0, f64::from(SCOPE_BASIS_POINTS)) as u32
}

#[derive(Debug, Clone, Copy)]
struct SamplingResolution {
    proxy: bool,
    max_width: u32,
}

impl SamplingResolution {
    fn parse(
        resolution: Option<&str>,
        proxy_sampling: bool,
        max_width: Option<u32>,
    ) -> Result<Self, ScopeError> {
        let normalized = resolution
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase);
        let resolution_proxy = match normalized.as_deref() {
            None | Some("full_resolution" | "full" | "monitor") => false,
            Some("proxy" | "bounded_proxy" | "thumbnail") => true,
            Some(value) => {
                return Err(ScopeError::unsupported_resolution(value));
            }
        };
        if proxy_sampling
            && matches!(
                normalized.as_deref(),
                Some("full_resolution" | "full" | "monitor")
            )
        {
            return Err(ScopeError::invalid_request(
                "proxy_sampling=true conflicts with an explicit full-resolution mode",
            ));
        }
        let proxy = proxy_sampling || resolution_proxy;
        let width = max_width.unwrap_or(DEFAULT_PROXY_WIDTH);
        if proxy && !(MIN_PROXY_WIDTH..=MAX_SCOPE_WIDTH).contains(&width) {
            return Err(ScopeError::invalid_request(format!(
                "proxy max_width must be {MIN_PROXY_WIDTH}..={MAX_SCOPE_WIDTH}, got {width}"
            )));
        }
        Ok(Self {
            proxy,
            max_width: width,
        })
    }

    fn value(self) -> Value {
        if self.proxy {
            json!({
                "mode": "proxy",
                "renderer": "analysis.thumbnail_for_document",
                "full_resolution": false,
                "max_width": self.max_width,
            })
        } else {
            json!({
                "mode": "full_resolution",
                "renderer": "analysis.monitor_proof_for_document",
                "full_resolution": true,
                "max_width": Value::Null,
            })
        }
    }
}

#[derive(Debug, Clone)]
struct ScopeRequest {
    stage: String,
    frames: Vec<TimeCode>,
    roi: NormalizedRoi,
    resolution: SamplingResolution,
    histogram_bins: usize,
    waveform_columns: usize,
    vector_bins: usize,
}

#[derive(Debug, Clone)]
struct RenderedSample {
    frame: TimeCode,
    image: RgbaImage,
    provenance: Value,
}

#[derive(Debug, Clone)]
pub(crate) struct ScopeError {
    code: String,
    message: String,
    details: Value,
}

impl ScopeError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: Value::Null,
        }
    }

    fn with_details(mut self, details: Value) -> Self {
        self.details = details;
        self
    }

    fn invalid_request(message: impl Into<String>) -> Self {
        Self::new("invalid_request", message)
    }

    fn invalid_roi(message: impl Into<String>) -> Self {
        Self::new("invalid_roi", message)
    }

    fn invalid_range(message: impl Into<String>) -> Self {
        Self::new("invalid_temporal_range", message)
    }

    fn excessive(message: impl Into<String>) -> Self {
        Self::new("excessive_sample_request", message)
    }

    fn unsupported_stage(stage: impl Into<String>) -> Self {
        let stage = stage.into();
        Self::new(
            "unsupported_stage",
            format!("named colour stage {stage:?} is not available from the managed monitor proof"),
        )
        .with_details(json!({
            "stage": stage,
            "supported_stages": ["monitoring_post_composite"],
        }))
    }

    fn unsupported_resolution(resolution: &str) -> Self {
        Self::new(
            "unsupported_sampling_resolution",
            format!(
                "sampling resolution {resolution:?} is not supported; use full_resolution or proxy"
            ),
        )
    }

    fn unavailable(asset: AssetId, status: &kinewright_core::MediaAvailabilityStatus) -> Self {
        Self::new(
            "media_unavailable",
            format!(
                "asset {asset} is not currently renderable: {:?} ({})",
                status.kind,
                status
                    .reason
                    .as_deref()
                    .unwrap_or("no backend reason supplied")
            ),
        )
        .with_details(json!({"asset_id": asset.0, "availability": status}))
    }

    fn render(error: &MediaError, frame: TimeCode) -> Self {
        Self::new(
            "scope_render_failed",
            format!("could not render project frame {frame}: {error}"),
        )
    }

    fn stale(expected: TimelineRevision, actual: TimelineRevision) -> Self {
        Self::new(
            "stale_revision",
            format!("timeline revision conflict: expected {expected}, actual {actual}"),
        )
        .with_details(json!({
            "expected_revision": expected.0,
            "actual_revision": actual.0,
        }))
    }

    pub(crate) fn code(&self) -> &str {
        &self.code
    }

    pub(crate) fn details(&self) -> Value {
        self.details.clone()
    }
}

impl fmt::Display for ScopeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ScopeError {}

/// Build read-only v2 scopes from a single immutable document snapshot.
pub(crate) fn video_scopes_v2(
    document: &Arc<Document>,
    revision: TimelineRevision,
    analysis: &dyn Analysis,
    args: &VideoScopesV2Args,
) -> Result<Value, ScopeError> {
    if let Some(expected) = args.expected_revision
        && expected != revision
    {
        return Err(ScopeError::stale(expected, revision));
    }
    validate_stage(&args.stage)?;
    ensure_document_media_available(document, analysis)?;
    let request = build_scope_request(
        document,
        &args.stage,
        args.timecode,
        args.range.as_ref(),
        args.frames.as_deref(),
        args.step_frames,
        args.roi.as_ref(),
        args.resolution.as_deref(),
        args.proxy_sampling,
        args.max_width,
        args.bins,
        args.columns,
    )?;
    let rendered = render_samples(document, analysis, &request)?;
    scope_response(revision, document, &request, &rendered)
}

/// Analyze one explicit color shot without generating or applying operations.
pub(crate) fn analyze_color_shot(
    document: &Arc<Document>,
    revision: TimelineRevision,
    analysis: &dyn Analysis,
    args: &AnalyzeColorShotArgs,
) -> Result<Value, ScopeError> {
    if args.expected_revision != revision {
        return Err(ScopeError::stale(args.expected_revision, revision));
    }
    validate_stage(&args.stage)?;
    let clip = visual_clip(document, args.clip_id)?;
    ensure_clip_available(document, analysis, clip)?;
    let (default_frame, duration) = clip_midpoint(document, clip)?;
    let timecode = if args.timecode.is_none() && args.range.is_none() && args.frames.is_none() {
        Some(default_frame)
    } else {
        args.timecode
    };
    let request = build_scope_request(
        document,
        &args.stage,
        timecode,
        args.range.as_ref(),
        args.frames.as_deref(),
        args.step_frames,
        args.roi.as_ref(),
        args.resolution.as_deref(),
        args.proxy_sampling,
        args.max_width,
        args.bins,
        args.columns,
    )?;
    ensure_frames_in_clip(&request.frames, clip, duration)?;
    let rendered = render_samples(document, analysis, &request)?;
    let evidence = measure_core_scopes(&request, &rendered)?;
    let value = scope_response(revision, document, &request, &rendered)?;
    Ok(json!({
        "timeline_revision": revision.0,
        "clip_id": args.clip_id.0,
        "asset_id": clip.asset.0,
        "stage": ScopeStage::MonitoringPostComposite.as_str(),
        "requested_stage": args.stage,
        "evidence_only": true,
        "applied": false,
        "shot": shot_evidence(&rendered, &evidence, request.roi),
        "scopes": value,
        "assumptions": assumptions(document, clip.asset, &request),
        "confidence": confidence(&evidence),
    }))
}

/// Compare one explicit reference shot to one or more explicit candidates and
/// return separate, revision-gated primary-correction proposals.
#[allow(clippy::too_many_lines)]
pub(crate) fn plan_shot_match(
    document: &Arc<Document>,
    revision: TimelineRevision,
    analysis: &dyn Analysis,
    args: &PlanShotMatchArgs,
) -> Result<Value, ScopeError> {
    if args.expected_revision != revision {
        return Err(ScopeError::stale(args.expected_revision, revision));
    }
    validate_stage(&args.stage)?;
    let reference = resolve_reference(args)?;
    let candidates = resolve_candidates(args)?;
    if candidates.is_empty() {
        return Err(ScopeError::invalid_request(
            "plan_shot_match requires at least one candidate shot",
        ));
    }
    let reference_clip = visual_clip(document, reference.clip_id)?;
    ensure_clip_available(document, analysis, reference_clip)?;
    let reference_frame = reference
        .timecode
        .unwrap_or(clip_midpoint(document, reference_clip)?.0);
    ensure_frames_in_clip(
        &[reference_frame],
        reference_clip,
        clip_midpoint(document, reference_clip)?.1,
    )?;
    let request = build_scope_request(
        document,
        &args.stage,
        Some(reference_frame),
        None,
        None,
        None,
        args.roi.as_ref(),
        args.resolution.as_deref(),
        args.proxy_sampling,
        args.max_width,
        args.bins,
        args.columns,
    )?;
    let reference_rendered = render_samples(document, analysis, &request)?;
    let reference_scope_evidence = measure_core_scopes(&request, &reference_rendered)?;
    let reference_stats = shot_stats(&reference_scope_evidence);
    let reference_evidence = json!({
        "summary": shot_evidence(&reference_rendered, &reference_scope_evidence, request.roi),
        "scope_evidence": serde_json::to_value(&reference_scope_evidence)
            .map_err(|error| ScopeError::new("scope_serialization_failed", error.to_string()))?,
    });

    let mut candidate_evidence = Vec::with_capacity(candidates.len());
    let mut candidate_operations = Vec::with_capacity(candidates.len());
    let mut operation_document = (**document).clone();
    for candidate in candidates {
        if candidate.clip_id == reference.clip_id {
            return Err(ScopeError::invalid_request(
                "reference shot must not also be a candidate",
            ));
        }
        let clip = visual_clip(document, candidate.clip_id)?;
        ensure_clip_available(document, analysis, clip)?;
        let frame = candidate
            .timecode
            .unwrap_or(clip_midpoint(document, clip)?.0);
        ensure_frames_in_clip(&[frame], clip, clip_midpoint(document, clip)?.1)?;
        let candidate_request = build_scope_request(
            document,
            &args.stage,
            Some(frame),
            None,
            None,
            None,
            args.roi.as_ref(),
            args.resolution.as_deref(),
            args.proxy_sampling,
            args.max_width,
            args.bins,
            args.columns,
        )?;
        let rendered = render_samples(document, analysis, &candidate_request)?;
        let candidate_scope_evidence = measure_core_scopes(&candidate_request, &rendered)?;
        let stats = shot_stats(&candidate_scope_evidence);
        let deltas = signed_deltas(reference_stats, stats);
        let scope_comparison =
            compare_scope_evidence(&reference_scope_evidence, &candidate_scope_evidence)
                .map_err(|error| ScopeError::new("scope_comparison_failed", error.to_string()))?;
        let proposed_parameters = match_parameters(reference_stats, stats);
        let plan_args = PrimaryCorrectionPlanArgs {
            expected_revision: revision,
            clip_id: candidate.clip_id,
            profile_assumption: None,
            parameters: proposed_parameters.clone(),
        };
        let plan = plan_primary_correction(&operation_document, revision, &plan_args)
            .map_err(|error| ScopeError::new("shot_match_plan_rejected", error.to_string()))?;
        let operations = serde_json::to_value(&plan.operations).map_err(|error| {
            ScopeError::new(
                "shot_match_serialization_failed",
                format!("could not serialize candidate operations: {error}"),
            )
        })?;
        apply_batch(&mut operation_document, &plan.operations).map_err(|error| {
            ScopeError::new(
                "shot_match_plan_rejected",
                format!("Core rejected candidate operations: {error}"),
            )
        })?;
        candidate_operations.push(json!({
            "clip_id": candidate.clip_id.0,
            "expected_revision": revision.0,
            "parameters": proposed_parameters,
            "operations": operations,
            "operation_visibility": "exact_unapplied_primary_correction_operations",
            "evidence_only": true,
            "applied": false,
        }));
        candidate_evidence.push(json!({
            "clip_id": candidate.clip_id.0,
            "asset_id": clip.asset.0,
            "project_frame": frame.0,
            "shot": shot_evidence(&rendered, &candidate_scope_evidence, candidate_request.roi),
            "signed_deltas": deltas,
            "scope_comparison": serde_json::to_value(scope_comparison).map_err(|error| ScopeError::new("scope_comparison_failed", error.to_string()))?,
            "proposed_parameters": proposed_parameters,
            "confidence": confidence(&candidate_scope_evidence),
            "assumptions": assumptions(document, clip.asset, &candidate_request),
        }));
    }
    Ok(json!({
        "timeline_revision": revision.0,
        "reference_shot": {
            "clip_id": reference.clip_id.0,
            "asset_id": reference_clip.asset.0,
            "project_frame": reference_frame.0,
            "evidence": reference_evidence,
        },
        "candidates": candidate_evidence,
        "editable_operations": candidate_operations,
        "evidence_only": true,
        "applied": false,
        "reference_retained": true,
        "match_scope": request.roi.value(),
        "stage": ScopeStage::MonitoringPostComposite.as_str(),
        "requested_stage": args.stage,
        "assumptions": assumptions(document, reference_clip.asset, &request),
        "next": "Review each candidate's exact operations, then submit the desired operations through prepare_edit_plan at this revision; this call never mutates the timeline.",
    }))
}

fn resolve_reference(args: &PlanShotMatchArgs) -> Result<ShotSelectorArgs, ScopeError> {
    match (args.reference_clip_id, args.reference_shot.clone()) {
        (Some(id), None) => Ok(ShotSelectorArgs {
            clip_id: id,
            timecode: None,
        }),
        (None, Some(reference)) => Ok(reference),
        (Some(id), Some(reference)) if id == reference.clip_id => Ok(reference),
        (Some(_), Some(_)) => Err(ScopeError::invalid_request(
            "reference_clip_id and reference_shot identify different clips",
        )),
        (None, None) => Err(ScopeError::invalid_request(
            "plan_shot_match requires exactly one reference_clip_id or reference_shot",
        )),
    }
}

fn resolve_candidates(args: &PlanShotMatchArgs) -> Result<Vec<ShotSelectorArgs>, ScopeError> {
    let mut candidates = args
        .candidate_clip_ids
        .iter()
        .copied()
        .map(|clip_id| ShotSelectorArgs {
            clip_id,
            timecode: None,
        })
        .collect::<Vec<_>>();
    candidates.extend(args.candidate_shots.clone());
    let mut seen = BTreeSet::new();
    for candidate in &candidates {
        if !seen.insert(candidate.clip_id) {
            return Err(ScopeError::invalid_request(format!(
                "candidate clip {} appears more than once",
                candidate.clip_id
            )));
        }
    }
    Ok(candidates)
}

fn validate_stage(stage: &str) -> Result<(), ScopeError> {
    let normalized = stage.trim().to_ascii_lowercase();
    if matches!(
        normalized.as_str(),
        "monitoring_post_composite" | "monitoring/post-composite" | "post_compositor" | "monitor"
    ) {
        Ok(())
    } else {
        Err(ScopeError::unsupported_stage(stage.to_owned()))
    }
}

#[allow(clippy::too_many_arguments)]
fn build_scope_request(
    document: &Document,
    stage: &str,
    timecode: Option<TimeCode>,
    range: Option<&ScopeRangeArgs>,
    explicit_frames: Option<&[TimeCode]>,
    step_frames: Option<TimeCode>,
    roi: Option<&ScopeRoiArgs>,
    resolution: Option<&str>,
    proxy_sampling: bool,
    max_width: Option<u32>,
    bins: Option<u16>,
    columns: Option<u16>,
) -> Result<ScopeRequest, ScopeError> {
    let frames = select_frames(
        document.duration,
        timecode,
        range,
        explicit_frames,
        step_frames,
    )?;
    let roi = NormalizedRoi::validate(roi)?;
    let resolution = SamplingResolution::parse(resolution, proxy_sampling, max_width)?;
    let histogram_bins = match bins {
        Some(value)
            if usize::from(value) < MIN_HISTOGRAM_BINS
                || usize::from(value) > MAX_HISTOGRAM_BINS =>
        {
            return Err(ScopeError::invalid_request(format!(
                "bins must be {MIN_HISTOGRAM_BINS}..={MAX_HISTOGRAM_BINS}, got {value}"
            )));
        }
        Some(value) => usize::from(value),
        None => DEFAULT_HISTOGRAM_BINS,
    };
    let waveform_columns = match columns {
        Some(value) if value == 0 || usize::from(value) > MAX_WAVEFORM_COLUMNS => {
            return Err(ScopeError::invalid_request(format!(
                "columns must be 1..={MAX_WAVEFORM_COLUMNS}, got {value}"
            )));
        }
        Some(value) => usize::from(value),
        None => DEFAULT_WAVEFORM_COLUMNS,
    };
    let vector_bins = histogram_bins.min(MAX_VECTOR_BINS);
    Ok(ScopeRequest {
        stage: stage.to_owned(),
        frames,
        roi,
        resolution,
        histogram_bins,
        waveform_columns,
        vector_bins,
    })
}

fn select_frames(
    duration: TimeCode,
    timecode: Option<TimeCode>,
    range: Option<&ScopeRangeArgs>,
    explicit_frames: Option<&[TimeCode]>,
    step_frames: Option<TimeCode>,
) -> Result<Vec<TimeCode>, ScopeError> {
    let selector_count = usize::from(timecode.is_some())
        .saturating_add(usize::from(range.is_some()))
        .saturating_add(usize::from(explicit_frames.is_some()));
    if selector_count > 1 {
        return Err(ScopeError::invalid_range(
            "provide exactly one of timecode, range, or explicit project frames",
        ));
    }
    if step_frames.is_some() && range.is_none() {
        return Err(ScopeError::invalid_range(
            "step_frames is valid only with a half-open temporal range",
        ));
    }
    let contains = |frame: TimeCode| frame >= TimeCode::ZERO && frame < duration;
    if let Some(frame) = timecode {
        if !contains(frame) {
            return Err(ScopeError::invalid_range(format!(
                "project frame {} is outside 0..{}",
                frame.0, duration.0
            )));
        }
        return Ok(vec![frame]);
    }
    if let Some(frames) = explicit_frames {
        if frames.is_empty() {
            return Err(ScopeError::invalid_range(
                "explicit project frames must not be empty",
            ));
        }
        if frames.len() > MAX_SCOPE_SAMPLES {
            return Err(ScopeError::excessive(format!(
                "explicit project frame count {} exceeds the {} sample limit",
                frames.len(),
                MAX_SCOPE_SAMPLES
            )));
        }
        let mut seen = BTreeSet::new();
        for frame in frames {
            if !contains(*frame) {
                return Err(ScopeError::invalid_range(format!(
                    "project frame {} is outside 0..{}",
                    frame.0, duration.0
                )));
            }
            if !seen.insert(*frame) {
                return Err(ScopeError::invalid_range(format!(
                    "explicit project frame {} appears more than once",
                    frame.0
                )));
            }
        }
        return Ok(frames.to_vec());
    }
    let Some(range) = range else {
        if duration <= TimeCode::ZERO {
            return Err(ScopeError::invalid_range("project duration is empty"));
        }
        return Ok(vec![TimeCode::ZERO]);
    };
    if range.start < TimeCode::ZERO || range.end <= range.start || range.end > duration {
        return Err(ScopeError::invalid_range(format!(
            "range {}..{} must be half-open, non-empty, and inside 0..{}",
            range.start.0, range.end.0, duration.0
        )));
    }
    let span = range.end.0.saturating_sub(range.start.0);
    let step = if let Some(step) = step_frames {
        if step.0 <= 0 {
            return Err(ScopeError::invalid_range("step_frames must be positive"));
        }
        step.0
    } else {
        // A range without an explicit step still has a deterministic bounded
        // default.  Explicit steps are never clamped or reinterpreted.
        span.saturating_add(i64::try_from(MAX_SCOPE_SAMPLES).unwrap_or(i64::MAX) - 1)
            / i64::try_from(MAX_SCOPE_SAMPLES).unwrap_or(1)
    }
    .max(1);
    let count =
        usize::try_from((span.saturating_add(step).saturating_sub(1)) / step).unwrap_or(usize::MAX);
    if count == 0 {
        return Err(ScopeError::invalid_range("range produced no samples"));
    }
    if count > MAX_SCOPE_SAMPLES {
        return Err(ScopeError::excessive(format!(
            "range and step request {count} samples, above the {MAX_SCOPE_SAMPLES} sample limit"
        )));
    }
    let mut frames = Vec::with_capacity(count);
    let mut frame = range.start.0;
    while frame < range.end.0 {
        frames.push(TimeCode(frame));
        frame = frame
            .checked_add(step)
            .ok_or_else(|| ScopeError::invalid_range("temporal sample frame overflowed"))?;
    }
    Ok(frames)
}

fn render_samples(
    document: &Arc<Document>,
    analysis: &dyn Analysis,
    request: &ScopeRequest,
) -> Result<Vec<RenderedSample>, ScopeError> {
    request
        .frames
        .iter()
        .map(|frame| {
            if request.resolution.proxy {
                let image = analysis
                    .thumbnail_for_document(Arc::clone(document), *frame, request.resolution.max_width)
                    .map_err(|error| ScopeError::render(&error, *frame))?;
                validate_image(&image, *frame)?;
                Ok(RenderedSample {
                    frame: *frame,
                    image,
                    provenance: request.resolution.value(),
                })
            } else {
                let proof = analysis
                    .monitor_proof_for_document(Arc::clone(document), *frame)
                    .map_err(|error| ScopeError::render(&error, *frame))?;
                if !proof.metadata.full_resolution {
                    return Err(ScopeError::new(
                        "invalid_render_provenance",
                        "full-resolution scope sampling returned a proof not marked full_resolution",
                    ));
                }
                validate_image(&proof.image, *frame)?;
                if (proof.image.width, proof.image.height) != document.resolution {
                    return Err(ScopeError::new(
                        "invalid_render_provenance",
                        format!(
                            "full-resolution scope frame {frame} is {}x{}, expected project raster {}x{}",
                            proof.image.width,
                            proof.image.height,
                            document.resolution.0,
                            document.resolution.1,
                        ),
                    ));
                }
                Ok(RenderedSample {
                    frame: *frame,
                    image: proof.image,
                    provenance: full_provenance(&proof.metadata),
                })
            }
        })
        .collect()
}

fn validate_image(image: &RgbaImage, frame: TimeCode) -> Result<(), ScopeError> {
    let expected = usize::try_from(image.width)
        .ok()
        .and_then(|width| {
            usize::try_from(image.height)
                .ok()
                .map(|height| width.saturating_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4));
    if image.width == 0 || image.height == 0 || expected != Some(image.pixels.len()) {
        return Err(ScopeError::new(
            "invalid_rendered_image",
            format!("rendered scope frame {frame} has invalid RGBA dimensions"),
        ));
    }
    Ok(())
}

fn full_provenance(metadata: &MonitorProofMetadata) -> Value {
    json!({
        "mode": "full_resolution",
        "renderer": "analysis.monitor_proof_for_document",
        "full_resolution": metadata.full_resolution,
        "render_kind": metadata.render_kind,
        "backend": metadata.backend,
        "adapter": metadata.adapter,
        "software_fallback": metadata.software_fallback,
        "gpu_claim": metadata.gpu_claim,
    })
}

fn scope_response(
    revision: TimelineRevision,
    document: &Document,
    request: &ScopeRequest,
    rendered: &[RenderedSample],
) -> Result<Value, ScopeError> {
    let core_evidence = measure_core_scopes(request, rendered)?;
    let mut core_value = serde_json::to_value(&core_evidence).map_err(|error| {
        ScopeError::new(
            "scope_serialization_failed",
            format!("could not serialize core scope evidence: {error}"),
        )
    })?;
    // The core engine measures the raster it receives and therefore marks that
    // raster as full-resolution.  The agent additionally records whether the
    // raster came from the full-resolution monitor proof or an explicit proxy
    // request so a proxy result can never be mistaken for delivery evidence.
    if request.resolution.proxy
        && let Some(metadata) = core_value.get_mut("metadata")
    {
        metadata["full_resolution"] = Value::Bool(false);
    }
    let mut samples = Vec::with_capacity(rendered.len());
    for sample in rendered {
        let evidence = measure_core_scopes(request, std::slice::from_ref(sample))?;
        samples.push(sample_value(sample, &evidence, request.resolution.proxy));
    }
    let mut value = json!({
        "timeline_revision": revision.0,
        "stage": ScopeStage::MonitoringPostComposite.as_str(),
        "requested_stage": request.stage,
        "render_stage": ScopeStage::MonitoringPostComposite.as_str(),
        "resolution": request.resolution.value(),
        "roi": request.roi.value(),
        "temporal": {
            "frames": request.frames.iter().map(|frame| frame.0).collect::<Vec<_>>(),
            "range": {
                "start": request.frames.first().map_or(0, |frame| frame.0),
                "end": request.frames.last().map_or(0, |frame| frame.0.saturating_add(1)),
                "half_open": true,
            },
            "sample_count": request.frames.len(),
            "maximum_samples": MAX_SCOPE_SAMPLES,
        },
        "provenance": {
            "timeline_revision": revision.0,
            "document_resolution": {"width": document.resolution.0, "height": document.resolution.1},
            "stage_requested": request.stage,
            "stage_measured": ScopeStage::MonitoringPostComposite.as_str(),
            "samples": rendered.iter().map(|sample| json!({
                "project_frame": sample.frame.0,
                "resolution": sample.provenance,
            })).collect::<Vec<_>>(),
        },
        "waveform": core_value["waveform"].clone(),
        "rgb_parade": core_value["parade"].clone(),
        "vectorscope": core_value["vectorscope"].clone(),
        "histogram": core_value["histograms"].clone(),
        "clipping": core_value["clipping"].clone(),
        "gamut": {"out_of_range_pixels": 0, "out_of_range_basis_points": 0, "definition": "RGBA8 monitor proof is display-clamped; source/working gamut requires a named high-precision stage"},
        "core_evidence": core_value,
        "sample_results": samples,
    });
    // Retain an explicit source of truth for callers that use the typed core
    // evidence fields while keeping the compact agent aliases above.
    value["provenance"]["scope_engine"] = json!("kinewright_core::measure_scopes");
    Ok(value)
}

fn measure_core_scopes(
    request: &ScopeRequest,
    rendered: &[RenderedSample],
) -> Result<ScopeEvidence, ScopeError> {
    let resolution = CoreScopeResolution::new(
        u16::try_from(request.histogram_bins).unwrap_or(u16::MAX),
        u16::try_from(request.waveform_columns).unwrap_or(u16::MAX),
        256,
        u16::try_from(request.waveform_columns).unwrap_or(u16::MAX),
        256,
        u16::try_from(request.vector_bins).unwrap_or(u16::MAX),
    )
    .map_err(|error| ScopeError::new("invalid_scope_resolution", error.to_string()))?;
    let roi = request.roi.core()?;
    let core_request = CoreScopeRequest {
        stage: ScopeStage::MonitoringPostComposite,
        roi,
        resolution,
    };
    let frames = rendered
        .iter()
        .map(|sample| ScopeFrame::new(sample.frame.0, &sample.image))
        .collect::<Vec<_>>();
    measure_scopes(&frames, &core_request)
        .map_err(|error| ScopeError::new("scope_measurement_failed", error.to_string()))
}

fn sample_value(sample: &RenderedSample, evidence: &ScopeEvidence, proxy: bool) -> Value {
    let mut metadata = evidence.metadata.clone();
    if proxy {
        metadata.full_resolution = false;
    }
    json!({
        "project_frame": sample.frame.0,
        "image": {"width": sample.image.width, "height": sample.image.height},
        "metadata": metadata,
        "statistics": evidence.statistics,
        "clipping": evidence.clipping,
        "provenance": sample.provenance,
    })
}

fn shot_stats(evidence: &ScopeEvidence) -> ShotStats {
    let mean =
        |channel: kinewright_core::ChannelStatistics| f64::from(channel.mean) * 255.0 / 1_000_000.0;
    let means = [
        mean(evidence.statistics.red),
        mean(evidence.statistics.green),
        mean(evidence.statistics.blue),
        mean(evidence.statistics.luma),
    ];
    let count = evidence.metadata.visible_pixel_count;
    let chroma =
        (means[0].max(means[1]).max(means[2]) - means[0].min(means[1]).min(means[2])).max(0.0);
    ShotStats {
        count,
        means,
        chroma,
    }
}

#[derive(Debug, Clone, Copy)]
struct ShotStats {
    count: u64,
    means: [f64; 4],
    chroma: f64,
}

fn shot_evidence(
    rendered: &[RenderedSample],
    evidence: &ScopeEvidence,
    roi: NormalizedRoi,
) -> Value {
    let stats = shot_stats(evidence);
    json!({
        "sample_count": rendered.len(),
        "frames": rendered.iter().map(|sample| sample.frame.0).collect::<Vec<_>>(),
        "mean_code_values": {"red": stats.means[0], "green": stats.means[1], "blue": stats.means[2], "luma": stats.means[3]},
        "mean_normalized": {"red": stats.means[0] / 255.0, "green": stats.means[1] / 255.0, "blue": stats.means[2] / 255.0, "luma": stats.means[3] / 255.0},
        "chroma_mean_code_values": stats.chroma,
        "pixel_count": stats.count,
        "roi": roi.value(),
        "clipping": evidence.clipping,
        "scope_statistics": evidence.statistics,
    })
}

fn confidence(evidence: &ScopeEvidence) -> Value {
    let samples = evidence
        .metadata
        .project_frames
        .len()
        .min(MAX_SCOPE_SAMPLES);
    let sample_score = (samples.saturating_mul(10_000) / 4).min(10_000);
    let pixel_score = u64::from(evidence.metadata.visible_pixel_count > 0) * 10_000;
    let basis_points = sample_score
        .midpoint(usize::try_from(pixel_score).unwrap_or(10_000))
        .min(10_000);
    json!({
        "basis_points": basis_points,
        "label": if basis_points >= 7_500 { "high" } else if basis_points >= 4_000 { "medium" } else { "low" },
        "basis": "deterministic sample coverage and non-transparent pixel coverage; not a learned semantic confidence",
    })
}

fn signed_deltas(reference: ShotStats, candidate: ShotStats) -> Value {
    json!({
        "red_code_values": signed_round(candidate.means[0] - reference.means[0]),
        "green_code_values": signed_round(candidate.means[1] - reference.means[1]),
        "blue_code_values": signed_round(candidate.means[2] - reference.means[2]),
        "luma_code_values": signed_round(candidate.means[3] - reference.means[3]),
        "red_basis_points": signed_round((candidate.means[0] - reference.means[0]) * 10_000.0 / 255.0),
        "green_basis_points": signed_round((candidate.means[1] - reference.means[1]) * 10_000.0 / 255.0),
        "blue_basis_points": signed_round((candidate.means[2] - reference.means[2]) * 10_000.0 / 255.0),
        "luma_basis_points": signed_round((candidate.means[3] - reference.means[3]) * 10_000.0 / 255.0),
        "sign_convention": "candidate minus reference; proposed correction moves candidate toward reference",
    })
}

fn match_parameters(reference: ShotStats, candidate: ShotStats) -> BTreeMap<String, i64> {
    let exposure = if candidate.means[3] <= 0.5 || reference.means[3] <= 0.5 {
        0
    } else {
        signed_round((reference.means[3] / candidate.means[3]).log2() * 1_000.0)
    };
    let saturation = signed_round((reference.chroma - candidate.chroma) * 100.0 / 128.0);
    let temperature = signed_round(
        ((reference.means[0] - reference.means[2]) - (candidate.means[0] - candidate.means[2]))
            * 100.0
            / 255.0,
    );
    let tint = signed_round(
        ((reference.means[1] - f64::midpoint(reference.means[0], reference.means[2]))
            - (candidate.means[1] - f64::midpoint(candidate.means[0], candidate.means[2])))
            * 100.0
            / 255.0,
    );
    let mut parameters = BTreeMap::new();
    if exposure != 0 {
        parameters.insert(
            "exposure_milli_stops".to_owned(),
            exposure.clamp(-5_000, 5_000),
        );
    }
    if temperature != 0 {
        parameters.insert(
            "temperature_percent".to_owned(),
            temperature.clamp(-100, 100),
        );
    }
    if tint != 0 {
        parameters.insert("tint_percent".to_owned(), tint.clamp(-100, 100));
    }
    if saturation != 0 {
        parameters.insert("saturation_percent".to_owned(), saturation.clamp(-100, 100));
    }
    parameters
}

fn assumptions(document: &Document, asset: AssetId, request: &ScopeRequest) -> Value {
    let color_description = document.asset(asset).map(|asset| &asset.color_description);
    json!([
        "RGBA8 scope values are measured after the named managed compositor boundary",
        "candidate-minus-reference deltas are signed display-code evidence, not a flattened LUT",
        {"asset_id": asset.0, "source_color_description": color_description},
        {"roi": request.roi.value(), "sampling": request.resolution.value()},
    ])
}

fn visual_clip(document: &Document, clip_id: ClipId) -> Result<&Clip, ScopeError> {
    for track in &document.tracks {
        if let Some(clip) = track.clips.iter().find(|clip| clip.id == clip_id) {
            if track.kind != TrackKind::Video
                || !matches!(clip.content, ClipContent::Media | ClipContent::Freeze(_))
            {
                return Err(ScopeError::invalid_request(format!(
                    "clip {clip_id} is not a visual media shot"
                )));
            }
            return Ok(clip);
        }
    }
    Err(ScopeError::invalid_request(format!(
        "clip {clip_id} does not exist"
    )))
}

fn clip_midpoint(document: &Document, clip: &Clip) -> Result<(TimeCode, TimeCode), ScopeError> {
    let duration = document.clip_duration(clip).map_err(|error| {
        ScopeError::invalid_request(format!("clip {} timing is invalid: {error}", clip.id))
    })?;
    if duration <= TimeCode::ZERO {
        return Err(ScopeError::invalid_request(format!(
            "clip {} has no positive duration",
            clip.id
        )));
    }
    let frame = clip
        .timeline_start
        .0
        .checked_add(duration.0 / 2)
        .ok_or_else(|| ScopeError::invalid_range("clip midpoint overflowed"))?;
    Ok((TimeCode(frame), duration))
}

fn ensure_frames_in_clip(
    frames: &[TimeCode],
    clip: &Clip,
    duration: TimeCode,
) -> Result<(), ScopeError> {
    let end = clip
        .timeline_start
        .0
        .checked_add(duration.0)
        .ok_or_else(|| ScopeError::invalid_range("clip end overflowed"))?;
    for frame in frames {
        if frame.0 < clip.timeline_start.0 || frame.0 >= end {
            return Err(ScopeError::invalid_range(format!(
                "project frame {} is outside clip {} visibility {}..{}",
                frame.0, clip.id, clip.timeline_start.0, end
            )));
        }
    }
    Ok(())
}

fn ensure_clip_available(
    document: &Document,
    analysis: &dyn Analysis,
    clip: &Clip,
) -> Result<(), ScopeError> {
    let asset = document.asset(clip.asset).ok_or_else(|| {
        ScopeError::invalid_request(format!(
            "clip {} references missing asset {}",
            clip.id, clip.asset
        ))
    })?;
    let status = analysis.media_availability(asset);
    if !matches!(status.kind, MediaAvailabilityKind::OnlineVerified) {
        return Err(ScopeError::unavailable(asset.id, &status));
    }
    Ok(())
}

fn ensure_document_media_available(
    document: &Document,
    analysis: &dyn Analysis,
) -> Result<(), ScopeError> {
    let mut seen = BTreeSet::new();
    for track in &document.tracks {
        for clip in &track.clips {
            if track.kind != TrackKind::Video
                || matches!(clip.content, ClipContent::Title(_))
                || !seen.insert(clip.asset)
            {
                continue;
            }
            ensure_clip_available(document, analysis, clip)?;
        }
    }
    Ok(())
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn signed_round(value: f64) -> i64 {
    if !value.is_finite() {
        0
    } else if value >= i64::MAX as f64 {
        i64::MAX
    } else if value <= i64::MIN as f64 {
        i64::MIN
    } else {
        value.round() as i64
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        path::{Path, PathBuf},
    };

    use crossbeam_channel::Receiver;
    use kinewright_core::{
        ColorBitDepth, ColorContext, ColorMatrix, ColorPrimaries, ColorProvenance, ColorRange,
        ColorTransfer, ColorWhitePoint, MediaAsset, MediaAvailabilityStatus, MediaError, MediaKind,
        MonitorProof, SceneStatus, SilenceStatus, TimelineSceneChange, TimelineSilenceSpan,
        TimelineTranscriptWord, TranscriptStatus, VisualAssetResult,
    };

    use super::*;

    #[derive(Debug)]
    struct StubAnalysis {
        frames: BTreeMap<TimeCode, RgbaImage>,
    }

    impl StubAnalysis {
        fn frame(&self, at: TimeCode) -> RgbaImage {
            self.frames.range(..=at).next_back().map_or_else(
                || self.frames.values().next().cloned().unwrap(),
                |(_, image)| image.clone(),
            )
        }
    }

    impl Analysis for StubAnalysis {
        fn probe(&self, _path: &Path) -> Result<MediaAsset, MediaError> {
            Err(MediaError::NotImplemented)
        }

        fn media_availability(&self, _asset: &MediaAsset) -> MediaAvailabilityStatus {
            MediaAvailabilityStatus {
                kind: MediaAvailabilityKind::OnlineVerified,
                observed_fingerprint: None,
                reason: Some("stub verified source".to_owned()),
            }
        }

        fn thumbnail_at(&self, at: TimeCode, _max_width: u32) -> Result<RgbaImage, MediaError> {
            Ok(self.frame(at))
        }

        fn monitor_proof_for_document(
            &self,
            _document: Arc<Document>,
            at: TimeCode,
        ) -> Result<MonitorProof, MediaError> {
            Ok(MonitorProof {
                image: self.frame(at),
                metadata: MonitorProofMetadata::test_double(),
            })
        }

        fn request_transcription(&self, _asset: MediaAsset) {}

        fn transcript_status(&self, _asset: &MediaAsset) -> TranscriptStatus {
            TranscriptStatus::NotRequested
        }

        fn timeline_transcript(
            &self,
            _document: &Document,
            _range: Option<std::ops::Range<TimeCode>>,
        ) -> Result<Vec<TimelineTranscriptWord>, MediaError> {
            Ok(Vec::new())
        }

        fn request_silence_detection(&self, _asset: MediaAsset) {}

        fn silence_status(&self, _asset: &MediaAsset) -> SilenceStatus {
            SilenceStatus::NotRequested
        }

        fn timeline_silences(
            &self,
            _document: &Document,
            _range: Option<std::ops::Range<TimeCode>>,
            _minimum_source_frames: TimeCode,
        ) -> Result<Vec<TimelineSilenceSpan>, MediaError> {
            Ok(Vec::new())
        }

        fn request_scene_detection(&self, _asset: MediaAsset) {}

        fn scene_status(&self, _asset: &MediaAsset) -> SceneStatus {
            SceneStatus::NotRequested
        }

        fn timeline_scene_changes(
            &self,
            _document: &Document,
            _range: Option<std::ops::Range<TimeCode>>,
            _minimum_confidence_basis_points: u16,
        ) -> Result<Vec<TimelineSceneChange>, MediaError> {
            Ok(Vec::new())
        }

        fn request_waveform(&self, _asset: MediaAsset, _request_generation: u64) -> bool {
            false
        }

        fn request_thumbnail(
            &self,
            _asset: MediaAsset,
            _source_at: TimeCode,
            _max_width: u32,
            _request_generation: u64,
        ) -> bool {
            false
        }

        fn visual_asset_results(&self) -> Receiver<VisualAssetResult> {
            crossbeam_channel::never()
        }
    }

    fn managed_color() -> kinewright_core::ColorDescription {
        kinewright_core::ColorDescription {
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

    fn two_shot_document() -> Document {
        let asset = |id: u64| MediaAsset {
            id: AssetId(id),
            path: PathBuf::from(format!("stub-{id}.mp4")),
            name: format!("stub-{id}"),
            duration: TimeCode(30),
            fps: kinewright_core::Rational::new(30, 1).unwrap(),
            kind: MediaKind::Video,
            resolution: Some((2, 1)),
            source_fingerprint: kinewright_core::MediaSourceFingerprint::unknown(),
            color_description: managed_color(),
        };
        let first = asset(1);
        let second = asset(2);
        Document {
            catalog: kinewright_core::MediaCatalog::default(),
            audio_mix: kinewright_core::AudioMix::default(),
            tracks: vec![kinewright_core::Track {
                id: kinewright_core::TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![
                    Clip {
                        id: ClipId(1),
                        asset: first.id,
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
                    },
                    Clip {
                        id: ClipId(2),
                        asset: second.id,
                        source_range: TimeCode::ZERO..TimeCode(30),
                        content: ClipContent::Media,
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
            media_pool: vec![first, second],
            markers: Vec::new(),
            fps: kinewright_core::Rational::new(30, 1).unwrap(),
            resolution: (2, 1),
            duration: TimeCode(60),
            color_context: ColorContext::default(),
        }
    }

    fn image() -> RgbaImage {
        RgbaImage {
            width: 2,
            height: 1,
            pixels: vec![10, 20, 30, 255, 240, 220, 200, 255],
        }
    }

    #[test]
    fn roi_is_normalized_and_geometric() {
        let roi = NormalizedRoi::validate(Some(&ScopeRoiArgs {
            x: Some(0.25),
            y: Some(0.0),
            width: Some(0.5),
            height: Some(1.0),
            left: None,
            top: None,
            right: None,
            bottom: None,
        }))
        .unwrap();
        let pixels = roi.core().unwrap().to_pixels(2, 1).unwrap();
        assert_eq!(pixels.width, 2);
        assert_eq!(pixels.height, 1);
    }

    #[test]
    fn roi_rejects_out_of_bounds_and_mixed_forms() {
        let bad = ScopeRoiArgs {
            x: Some(-0.1),
            y: Some(0.0),
            width: Some(0.5),
            height: Some(1.0),
            left: None,
            top: None,
            right: None,
            bottom: None,
        };
        assert_eq!(
            NormalizedRoi::validate(Some(&bad)).unwrap_err().code(),
            "invalid_roi"
        );
        let mixed = ScopeRoiArgs {
            x: Some(0.0),
            y: Some(0.0),
            width: Some(1.0),
            height: Some(1.0),
            left: Some(0.0),
            top: None,
            right: None,
            bottom: None,
        };
        assert_eq!(
            NormalizedRoi::validate(Some(&mixed)).unwrap_err().code(),
            "invalid_roi"
        );
    }

    #[test]
    fn temporal_ranges_are_half_open_and_bounded() {
        assert_eq!(
            select_frames(
                TimeCode(10),
                None,
                Some(&ScopeRangeArgs {
                    start: TimeCode(2),
                    end: TimeCode(8)
                }),
                None,
                Some(TimeCode(2))
            )
            .unwrap(),
            vec![TimeCode(2), TimeCode(4), TimeCode(6)]
        );
        assert_eq!(
            select_frames(
                TimeCode(10),
                None,
                None,
                Some(&[TimeCode(0), TimeCode(9)]),
                None
            )
            .unwrap(),
            vec![TimeCode(0), TimeCode(9)]
        );
        assert_eq!(
            select_frames(
                TimeCode(10),
                None,
                Some(&ScopeRangeArgs {
                    start: TimeCode(2),
                    end: TimeCode(8)
                }),
                None,
                Some(TimeCode(0))
            )
            .unwrap_err()
            .code(),
            "invalid_temporal_range"
        );
        assert_eq!(
            select_frames(
                TimeCode(10),
                Some(TimeCode(2)),
                None,
                None,
                Some(TimeCode(2))
            )
            .unwrap_err()
            .code(),
            "invalid_temporal_range"
        );
    }

    #[test]
    fn scope_math_reports_channels_and_signed_deltas() {
        let roi = NormalizedRoi::FULL.core().unwrap();
        let request = CoreScopeRequest {
            stage: ScopeStage::MonitoringPostComposite,
            roi,
            resolution: CoreScopeResolution::new(16, 2, 8, 2, 8, 8).unwrap(),
        };
        let evidence = measure_scopes(&[ScopeFrame::new(0, &image())], &request).unwrap();
        let stats = shot_stats(&evidence);
        assert!(stats.means[0] > 0.0 && stats.means[2] > 0.0);
        let delta = signed_deltas(
            stats,
            ShotStats {
                means: [2.0, 2.0, 2.0, 2.0],
                count: 1,
                chroma: 0.0,
            },
        );
        assert!(delta["red_code_values"].is_number());
        assert!(serde_json::to_value(evidence).unwrap()["parade"]["red"].is_object());
    }

    #[test]
    fn shot_match_is_read_only_full_resolution_and_revision_gated() {
        let document = Arc::new(two_shot_document());
        let original = (*document).clone();
        let analysis = StubAnalysis {
            frames: BTreeMap::from([
                (
                    TimeCode(0),
                    RgbaImage {
                        width: 2,
                        height: 1,
                        pixels: vec![40, 50, 60, 255, 40, 50, 60, 255],
                    },
                ),
                (
                    TimeCode(30),
                    RgbaImage {
                        width: 2,
                        height: 1,
                        pixels: vec![100, 120, 140, 255, 100, 120, 140, 255],
                    },
                ),
            ]),
        };
        let args = PlanShotMatchArgs {
            expected_revision: TimelineRevision(7),
            reference_clip_id: Some(ClipId(1)),
            reference_shot: None,
            candidate_clip_ids: vec![ClipId(2)],
            candidate_shots: Vec::new(),
            stage: "monitoring_post_composite".to_owned(),
            roi: None,
            resolution: None,
            proxy_sampling: false,
            max_width: None,
            bins: None,
            columns: None,
        };
        let result = plan_shot_match(&document, TimelineRevision(7), &analysis, &args).unwrap();
        assert_eq!(result["reference_shot"]["clip_id"], 1);
        assert_eq!(result["reference_retained"], true);
        assert_eq!(result["applied"], false);
        assert_eq!(
            result["reference_shot"]["evidence"]["scope_evidence"]["metadata"]["full_resolution"],
            true
        );
        assert!(
            result["candidates"][0]["signed_deltas"]["luma_code_values"]
                .as_i64()
                .is_some_and(|delta| delta > 0)
        );
        let operations = result["editable_operations"][0]["operations"]
            .as_array()
            .unwrap();
        assert!(!operations.is_empty());
        assert!(operations[0].get("AddEffect").is_some());
        assert_eq!(result["editable_operations"][0]["expected_revision"], 7);
        assert_eq!(*document, original);

        let stale = plan_shot_match(&document, TimelineRevision(8), &analysis, &args).unwrap_err();
        assert_eq!(stale.code(), "stale_revision");

        let unsupported = plan_shot_match(
            &document,
            TimelineRevision(7),
            &analysis,
            &PlanShotMatchArgs {
                stage: "delivery".to_owned(),
                ..args
            },
        )
        .unwrap_err();
        assert_eq!(unsupported.code(), "unsupported_stage");
    }

    #[test]
    fn temporal_shot_analysis_and_proxy_provenance_are_explicit() {
        let document = Arc::new(two_shot_document());
        let analysis = StubAnalysis {
            frames: BTreeMap::from([
                (
                    TimeCode::ZERO,
                    RgbaImage {
                        width: 2,
                        height: 1,
                        pixels: vec![40, 50, 60, 255, 40, 50, 60, 255],
                    },
                ),
                (
                    TimeCode(10),
                    RgbaImage {
                        width: 2,
                        height: 1,
                        pixels: vec![60, 70, 80, 255, 60, 70, 80, 255],
                    },
                ),
            ]),
        };
        let analysis_value = analyze_color_shot(
            &document,
            TimelineRevision(3),
            &analysis,
            &AnalyzeColorShotArgs {
                expected_revision: TimelineRevision(3),
                clip_id: ClipId(1),
                stage: "monitoring_post_composite".to_owned(),
                timecode: None,
                range: Some(ScopeRangeArgs {
                    start: TimeCode::ZERO,
                    end: TimeCode(20),
                }),
                frames: None,
                step_frames: Some(TimeCode(10)),
                roi: None,
                resolution: None,
                proxy_sampling: false,
                max_width: None,
                bins: None,
                columns: None,
            },
        )
        .unwrap();
        assert_eq!(analysis_value["scopes"]["temporal"]["sample_count"], 2);

        let proxy_value = video_scopes_v2(
            &document,
            TimelineRevision(3),
            &analysis,
            &VideoScopesV2Args {
                expected_revision: Some(TimelineRevision(3)),
                stage: "monitoring_post_composite".to_owned(),
                timecode: Some(TimeCode::ZERO),
                range: None,
                frames: None,
                step_frames: None,
                roi: None,
                resolution: Some("proxy".to_owned()),
                proxy_sampling: false,
                max_width: Some(512),
                bins: Some(16),
                columns: Some(8),
            },
        )
        .unwrap();
        assert_eq!(proxy_value["resolution"]["full_resolution"], false);
        assert_eq!(
            proxy_value["core_evidence"]["metadata"]["full_resolution"],
            false
        );
        assert_eq!(
            proxy_value["sample_results"][0]["metadata"]["full_resolution"],
            false
        );
        assert_eq!(proxy_value["vectorscope"]["size"], 16);

        assert_eq!(
            SamplingResolution::parse(Some("full_resolution"), true, None)
                .unwrap_err()
                .code(),
            "invalid_request"
        );
    }
}
