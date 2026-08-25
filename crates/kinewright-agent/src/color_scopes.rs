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

use crate::color_status::{
    ExistingPrimaryNode, PrimaryCorrectionPlanArgs, existing_primary_node, plan_primary_correction,
};

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

/// One `plan_shot_match` call renders one full-resolution monitor proof per
/// candidate.  The candidate list is therefore capped exactly like the
/// temporal sample budget, and the cap is enforced before any render.
pub(crate) const MAX_SHOT_MATCH_CANDIDATES: usize = 16;

/// The exact stage vocabulary this tool accepts.  The first entry is the
/// canonical CC2 name; the second is Core's own serde alias for the same
/// `ScopeStage::MonitoringPostComposite` value.  Nothing else is accepted:
/// a friendly alias for a stage the backend cannot prove is a silent
/// misattribution of evidence.
const SUPPORTED_SCOPE_STAGES: [&str; 2] =
    ["monitoring_post_composite", "monitoring/post-composite"];

/// BT.709 EOTF (CC2 §3.1) constants, decoding a normalized 8-bit monitoring
/// code back to scene-referred linear light.  These match
/// `kinewright_media::color_pipeline::decode_bt709` exactly.
const BT709_EOTF_BREAKPOINT: f64 = 0.081;
const BT709_EOTF_LINEAR_SLOPE: f64 = 4.5;
const BT709_EOTF_OFFSET: f64 = 0.099;
const BT709_EOTF_SCALE: f64 = 1.099;
const BT709_EOTF_EXPONENT: f64 = 0.45;

/// BT.709 linear-light luminance weights, used to fold linearised channel
/// means into one exposure observation.
const BT709_LUMA_RED: f64 = 0.212_6;
const BT709_LUMA_GREEN: f64 = 0.715_2;
const BT709_LUMA_BLUE: f64 = 0.072_2;

/// The CC1 managed white-balance model (`kinewright_media::color_pipeline`,
/// `PrimaryCorrection::apply`) is exactly:
///
/// ```text
/// red_gain   = 1 + 0.1 * (temperature_percent / 100)
/// blue_gain  = 1 - 0.1 * (temperature_percent / 100)
/// green_gain = 1 - 0.1 * (tint_percent / 100)
/// ```
///
/// so one *percent* of either control moves its channel by
/// `0.1 / 100 = 0.001` of linear gain.  Both derivations below use that
/// single constant rather than restating the model.
const CC1_WHITE_BALANCE_GAIN_PER_PERCENT: f64 = 0.001;

/// Full 8-bit code range used to normalize the measured channel means.
const CODE_VALUE_MAXIMUM: f64 = 255.0;

/// Decode one normalized BT.709 display code to linear light (CC2 §3.1).
///
/// Scope means are gamma-encoded 8-bit monitoring codes.  Treating them as a
/// linear-light quantity understates a needed exposure change by roughly a
/// factor of two, so every gain proposal linearises first.
fn bt709_eotf(encoded: f64) -> f64 {
    let encoded = encoded.clamp(0.0, 1.0);
    if encoded < BT709_EOTF_BREAKPOINT {
        encoded / BT709_EOTF_LINEAR_SLOPE
    } else {
        ((encoded + BT709_EOTF_OFFSET) / BT709_EOTF_SCALE).powf(1.0 / BT709_EOTF_EXPONENT)
    }
}

/// Canonical request envelope for `get_video_scopes_v2`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct VideoScopesV2Args {
    /// Optional revision returned by `get_timeline_state`.  When supplied,
    /// the immutable evidence snapshot must still be at this revision.
    #[serde(default)]
    pub expected_revision: Option<TimelineRevision>,
    /// Named managed-pipeline stage.  Exactly one stage is provable today:
    /// `monitoring_post_composite` (Core's `monitoring/post-composite` serde
    /// alias is also accepted).  Anything else fails closed.
    #[serde(default = "default_scope_stage")]
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
    /// Include the waveform/parade/vectorscope density grids.  They dominate
    /// the payload, so they default to `true` only for this dedicated scope
    /// tool.  Statistics, clipping, and histograms are always returned.
    #[serde(default)]
    pub include_grids: Option<bool>,
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
    /// Include the waveform/parade/vectorscope density grids.  Diagnosis needs
    /// statistics, clipping, and histograms, so the grids default to `false`
    /// here and the response records `grids_omitted`.
    #[serde(default)]
    pub include_grids: Option<bool>,
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
    /// Include the waveform/parade/vectorscope density grids.  A match plan
    /// renders one proof per candidate, so the grids default to `false` and
    /// the response records `grids_omitted`.
    #[serde(default)]
    pub include_grids: Option<bool>,
}

fn default_scope_stage() -> String {
    SUPPORTED_SCOPE_STAGES[0].to_owned()
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
        // Core quantizes the ROI to basis points, so a positive-but-subquantum
        // extent silently collapses to an empty region.  Reject it here, at the
        // agent boundary, naming the offending field and the quantum.
        for (field, extent) in [("width", right - left), ("height", bottom - top)] {
            if normalized_basis_points(extent) == 0 {
                return Err(ScopeError::invalid_roi(format!(
                    "ROI {field} {extent} rounds to 0 of {SCOPE_BASIS_POINTS} basis points; the scope ROI quantum is 1/{SCOPE_BASIS_POINTS} of the raster, so {field} must round to at least 1 basis point"
                )));
            }
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
                // The proxy path returns a bare raster with no backend
                // metadata, so there is nothing to attribute.  Reporting the
                // request back as if it were backend provenance would fabricate
                // evidence, so both fields are explicit instead.
                "backend": Value::Null,
                "adapter": Value::Null,
                "provenance": "proxy_unverified_by_backend",
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
    /// The half-open range the caller asked for, retained verbatim so the
    /// response can distinguish it from the frames actually sampled.
    requested_range: Option<(TimeCode, TimeCode)>,
    /// The spacing actually used for a range, whether explicit or derived.
    step_frames: Option<TimeCode>,
    /// Whether `step_frames` came from the caller or from the bounded default.
    step_source: &'static str,
    roi: NormalizedRoi,
    resolution: SamplingResolution,
    histogram_bins: usize,
    waveform_columns: usize,
    vector_bins: usize,
    include_grids: bool,
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
            "supported_stages": SUPPORTED_SCOPE_STAGES,
            "default_stage": SUPPORTED_SCOPE_STAGES[0],
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
        // The dedicated scope tool is the one surface whose whole purpose is
        // the density grids, so they are included unless the caller opts out.
        args.include_grids.unwrap_or(true),
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
        // Diagnosis needs statistics, clipping, and histograms; the density
        // grids are opt-in so a routine analysis stays a small payload.
        args.include_grids.unwrap_or(false),
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
        // Marked at every level: a proxy sample must never be readable as a
        // full-resolution proof from any nesting depth of this response.
        "resolution": request.resolution.value(),
        "full_resolution": !request.resolution.proxy,
        "grids_omitted": !request.include_grids,
        "shot": shot_evidence(&rendered, &evidence, &request),
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
        args.include_grids.unwrap_or(false),
    )?;
    let reference_rendered = render_samples(document, analysis, &request)?;
    let reference_scope_evidence = measure_core_scopes(&request, &reference_rendered)?;
    let reference_stats = shot_stats(&reference_scope_evidence);
    let reference_evidence = json!({
        "summary": shot_evidence(&reference_rendered, &reference_scope_evidence, &request),
        // Routed through the same override `scope_response` applies, so the
        // reference cannot report `full_resolution: true` for a proxy sample.
        "scope_evidence": core_evidence_value(&request, &reference_scope_evidence)?,
        "resolution": request.resolution.value(),
        "full_resolution": !request.resolution.proxy,
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
            args.include_grids.unwrap_or(false),
        )?;
        let rendered = render_samples(document, analysis, &candidate_request)?;
        let candidate_scope_evidence = measure_core_scopes(&candidate_request, &rendered)?;
        let stats = shot_stats(&candidate_scope_evidence);
        let deltas = signed_deltas(reference_stats, stats);
        let scope_comparison =
            compare_scope_evidence(&reference_scope_evidence, &candidate_scope_evidence)
                .map_err(|error| ScopeError::new("scope_comparison_failed", error.to_string()))?;
        // The scopes measured this candidate through whatever grade it already
        // carries, so the match term is a delta on top of that node. Read the
        // node from the staged document the plan will be validated against and
        // compose, or the proposal would overwrite the existing grade with the
        // delta alone.
        let existing = existing_primary_node(&operation_document, candidate.clip_id);
        let proposal = match_parameters(reference_stats, stats, existing.as_ref());
        let proposed_parameters = proposal.parameters.clone();
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
        // An already-matching candidate proposes nothing; Core rejects an empty
        // batch, so there is simply nothing to stage for the next candidate.
        if !plan.operations.is_empty() {
            apply_batch(&mut operation_document, &plan.operations).map_err(|error| {
                ScopeError::new(
                    "shot_match_plan_rejected",
                    format!("Core rejected candidate operations: {error}"),
                )
            })?;
        }
        candidate_operations.push(json!({
            "clip_id": candidate.clip_id.0,
            "expected_revision": revision.0,
            "parameters": proposed_parameters,
            "proposal_details": proposal.details,
            "operations": operations,
            "operation_visibility": "exact_unapplied_primary_correction_operations",
            "existing_primary_node_count": plan.existing_primary_node_count,
            // Null when the proposal changes nothing and no node exists: the
            // planner's fresh id is never allocated by any operation here.
            "target_effect_id": plan.target_effect_id().map(|effect| effect.0),
            "created_new_node": plan.created_new_node,
            "no_change": plan.no_change,
            // The written values are absolute. `composed` says whether they
            // already include the node's prior grade.
            "composed": proposal.composed,
            "current_parameters": proposal.details["current_parameters"].clone(),
            "delta_parameters": proposal.details["delta_parameters"].clone(),
            "warnings": plan.warnings,
            "evidence_only": true,
            "applied": false,
        }));
        candidate_evidence.push(json!({
            "clip_id": candidate.clip_id.0,
            "asset_id": clip.asset.0,
            "project_frame": frame.0,
            "shot": shot_evidence(&rendered, &candidate_scope_evidence, &candidate_request),
            "signed_deltas": deltas,
            "scope_comparison": comparison_value(&candidate_request, &scope_comparison)?,
            "scope_evidence": core_evidence_value(&candidate_request, &candidate_scope_evidence)?,
            "proposed_parameters": proposed_parameters,
            "proposal_details": proposal.details,
            "resolution": candidate_request.resolution.value(),
            "full_resolution": !candidate_request.resolution.proxy,
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
        // Marked at every level: top-level, reference evidence, each candidate,
        // each sample, and inside the typed core evidence.
        "resolution": request.resolution.value(),
        "full_resolution": !request.resolution.proxy,
        "grids_omitted": !request.include_grids,
        "candidate_limit": MAX_SHOT_MATCH_CANDIDATES,
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
    // Each candidate costs one full-resolution monitor proof, so the list is
    // capped before any render rather than after the first few have already
    // been paid for.
    if candidates.len() > MAX_SHOT_MATCH_CANDIDATES {
        return Err(ScopeError::excessive(format!(
            "plan_shot_match requested {} candidate shots, above the {MAX_SHOT_MATCH_CANDIDATES} candidate limit; each candidate renders one managed monitor proof",
            candidates.len()
        ))
        .with_details(json!({
            "max": MAX_SHOT_MATCH_CANDIDATES,
            "requested": candidates.len(),
        })));
    }
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

/// Accept only the exact CC2 stage vocabulary.
///
/// Friendly aliases such as `post_compositor` or `monitor` used to be accepted
/// and then silently reported as `monitoring_post_composite`, which lets a
/// caller believe a stage was measured that the backend cannot prove.
fn validate_stage(stage: &str) -> Result<(), ScopeError> {
    if SUPPORTED_SCOPE_STAGES.contains(&stage) {
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
    include_grids: bool,
) -> Result<ScopeRequest, ScopeError> {
    let selection = select_frames(
        document.duration,
        timecode,
        range,
        explicit_frames,
        step_frames,
    )?;
    let frames = selection.frames;
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
        requested_range: range.map(|range| (range.start, range.end)),
        step_frames: selection.step_frames,
        step_source: selection.step_source,
        roi,
        resolution,
        histogram_bins,
        waveform_columns,
        vector_bins,
        include_grids,
    })
}

/// The frames a request resolves to plus the spacing that produced them.
#[derive(Debug)]
struct FrameSelection {
    frames: Vec<TimeCode>,
    step_frames: Option<TimeCode>,
    step_source: &'static str,
}

impl FrameSelection {
    fn explicit(frames: Vec<TimeCode>) -> Self {
        Self {
            frames,
            step_frames: None,
            step_source: "not_applicable",
        }
    }
}

#[allow(clippy::too_many_lines)]
fn select_frames(
    duration: TimeCode,
    timecode: Option<TimeCode>,
    range: Option<&ScopeRangeArgs>,
    explicit_frames: Option<&[TimeCode]>,
    step_frames: Option<TimeCode>,
) -> Result<FrameSelection, ScopeError> {
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
        return Ok(FrameSelection::explicit(vec![frame]));
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
        return Ok(FrameSelection::explicit(frames.to_vec()));
    }
    let Some(range) = range else {
        if duration <= TimeCode::ZERO {
            return Err(ScopeError::invalid_range("project duration is empty"));
        }
        return Ok(FrameSelection::explicit(vec![TimeCode::ZERO]));
    };
    if range.start < TimeCode::ZERO || range.end <= range.start || range.end > duration {
        return Err(ScopeError::invalid_range(format!(
            "range {}..{} must be half-open, non-empty, and inside 0..{}",
            range.start.0, range.end.0, duration.0
        )));
    }
    let span = range.end.0.saturating_sub(range.start.0);
    let (step, step_source) = if let Some(step) = step_frames {
        if step.0 <= 0 {
            return Err(ScopeError::invalid_range("step_frames must be positive"));
        }
        (step.0, "explicit")
    } else {
        // A range without an explicit step still has a deterministic bounded
        // default.  Explicit steps are never clamped or reinterpreted.
        (
            span.saturating_add(i64::try_from(MAX_SCOPE_SAMPLES).unwrap_or(i64::MAX) - 1)
                / i64::try_from(MAX_SCOPE_SAMPLES).unwrap_or(1),
            "bounded_default",
        )
    };
    let step = step.max(1);
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
    Ok(FrameSelection {
        frames,
        step_frames: Some(TimeCode(step)),
        step_source,
    })
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

/// Grid scopes whose density arrays dominate the serialized payload.
const DENSITY_GRID_KEYS: [&str; 3] = ["waveform", "parade", "vectorscope"];

/// Serialize Core scope evidence with the two agent-owned overrides applied:
/// the honest proxy/full-resolution marker, and the optional omission of the
/// density grids.
///
/// Core measures whatever raster it is handed and therefore always marks it
/// `full_resolution`.  Only the agent knows whether that raster came from the
/// managed monitor proof or from an explicit proxy request, so the override is
/// applied at *every* level a caller could read.
fn core_evidence_value(
    request: &ScopeRequest,
    evidence: &ScopeEvidence,
) -> Result<Value, ScopeError> {
    let mut value = serde_json::to_value(evidence).map_err(|error| {
        ScopeError::new(
            "scope_serialization_failed",
            format!("could not serialize core scope evidence: {error}"),
        )
    })?;
    if request.resolution.proxy
        && let Some(metadata) = value.get_mut("metadata")
    {
        metadata["full_resolution"] = Value::Bool(false);
    }
    if !request.include_grids
        && let Some(object) = value.as_object_mut()
    {
        for key in DENSITY_GRID_KEYS {
            object.remove(key);
        }
    }
    value["grids_omitted"] = Value::Bool(!request.include_grids);
    value["omitted_grids"] = if request.include_grids {
        json!([])
    } else {
        json!(DENSITY_GRID_KEYS)
    };
    value["full_resolution"] = Value::Bool(!request.resolution.proxy);
    Ok(value)
}

/// Drop the density-grid deltas from a scope comparison when the request did
/// not ask for grids, so a comparison cannot reintroduce the payload the
/// evidence itself omitted.
fn comparison_value(
    request: &ScopeRequest,
    comparison: &kinewright_core::ScopeComparison,
) -> Result<Value, ScopeError> {
    let mut value = serde_json::to_value(comparison)
        .map_err(|error| ScopeError::new("scope_comparison_failed", error.to_string()))?;
    if !request.include_grids
        && let Some(object) = value.as_object_mut()
    {
        for key in DENSITY_GRID_KEYS {
            object.remove(key);
        }
    }
    value["grids_omitted"] = Value::Bool(!request.include_grids);
    value["full_resolution"] = Value::Bool(!request.resolution.proxy);
    Ok(value)
}

fn scope_response(
    revision: TimelineRevision,
    document: &Document,
    request: &ScopeRequest,
    rendered: &[RenderedSample],
) -> Result<Value, ScopeError> {
    let core_evidence = measure_core_scopes(request, rendered)?;
    let core_value = core_evidence_value(request, &core_evidence)?;
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
        "full_resolution": !request.resolution.proxy,
        "roi": request.roi.value(),
        "temporal": {
            "frames": request.frames.iter().map(|frame| frame.0).collect::<Vec<_>>(),
            // The requested span and the span actually covered by samples are
            // different facts.  A step larger than one frame leaves the tail of
            // the request unsampled, so both are reported separately and the
            // ambiguous single `range` key is gone.
            "requested_range": request.requested_range.map_or(Value::Null, |(start, end)| json!({
                "start": start.0,
                "end": end.0,
                "half_open": true,
            })),
            "sampled_frames": {
                "first": request.frames.first().map_or(Value::Null, |frame| json!(frame.0)),
                "last": request.frames.last().map_or(Value::Null, |frame| json!(frame.0)),
                "count": request.frames.len(),
            },
            "step_frames": request.step_frames.map_or(Value::Null, |step| json!(step.0)),
            "step_source": request.step_source,
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
        "gamut": {"out_of_range_pixels": 0, "out_of_range_basis_points": 0, "definition": "RGBA8 monitor proof is display-clamped; source/working gamut requires a named high-precision stage"},
        // `core_evidence` is the single typed source of truth.  The former
        // top-level waveform/rgb_parade/vectorscope/histogram/clipping aliases
        // repeated the same arrays verbatim and roughly doubled the payload.
        "core_evidence": core_value,
        "grids_omitted": !request.include_grids,
        "sample_results": samples,
    });
    // `core_evidence` above is the only copy of the scope arrays, so name the
    // engine that produced them explicitly rather than leaving callers to infer
    // it from the typed field shapes.
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
    let mean = |channel: kinewright_core::ChannelStatistics| {
        f64::from(channel.mean) * CODE_VALUE_MAXIMUM / 1_000_000.0
    };
    let means = [
        mean(evidence.statistics.red),
        mean(evidence.statistics.green),
        mean(evidence.statistics.blue),
        mean(evidence.statistics.luma),
    ];
    let count = evidence.metadata.visible_pixel_count;
    let chroma =
        (means[0].max(means[1]).max(means[2]) - means[0].min(means[1]).min(means[2])).max(0.0);
    // Every gain proposal is computed in linear light, so the display-coded
    // channel means are decoded once here and reused.
    let linear = [
        bt709_eotf(means[0] / CODE_VALUE_MAXIMUM),
        bt709_eotf(means[1] / CODE_VALUE_MAXIMUM),
        bt709_eotf(means[2] / CODE_VALUE_MAXIMUM),
    ];
    let linear_luma =
        BT709_LUMA_RED * linear[0] + BT709_LUMA_GREEN * linear[1] + BT709_LUMA_BLUE * linear[2];
    ShotStats {
        count,
        means,
        chroma,
        linear,
        linear_luma,
    }
}

#[derive(Debug, Clone, Copy)]
struct ShotStats {
    count: u64,
    /// Red, green, blue, and luma means as 8-bit display codes.
    means: [f64; 4],
    /// Spread of the three channel means; a colour-cast indicator, not a
    /// saturation measurement.
    chroma: f64,
    /// Red, green, and blue means decoded to linear light through the BT.709
    /// EOTF.
    linear: [f64; 3],
    /// BT.709-weighted linear luminance of `linear`.
    linear_luma: f64,
}

fn shot_evidence(
    rendered: &[RenderedSample],
    evidence: &ScopeEvidence,
    request: &ScopeRequest,
) -> Value {
    let stats = shot_stats(evidence);
    json!({
        "sample_count": rendered.len(),
        "frames": rendered.iter().map(|sample| sample.frame.0).collect::<Vec<_>>(),
        // The two `luma` fields are different quantities and must not be
        // compared or converted into each other: the display-coded means carry
        // the integer luma statistic the scope engine measured, while the
        // linear-light means apply BT.709 weights to the *linearised* RGB
        // means. Linearising is non-linear, so weighting-then-linearising and
        // linearising-then-weighting do not agree.
        "mean_code_values": {"red": stats.means[0], "green": stats.means[1], "blue": stats.means[2], "luma": stats.means[3], "luma_basis": "integer_luma_code"},
        "mean_normalized": {"red": stats.means[0] / CODE_VALUE_MAXIMUM, "green": stats.means[1] / CODE_VALUE_MAXIMUM, "blue": stats.means[2] / CODE_VALUE_MAXIMUM, "luma": stats.means[3] / CODE_VALUE_MAXIMUM, "luma_basis": "integer_luma_code"},
        "mean_linear_light": {"red": stats.linear[0], "green": stats.linear[1], "blue": stats.linear[2], "luma": stats.linear_luma, "luma_basis": "bt709_weights_on_linearised_means"},
        "chroma_mean_code_values": stats.chroma,
        "pixel_count": stats.count,
        "roi": request.roi.value(),
        "resolution": request.resolution.value(),
        "full_resolution": !request.resolution.proxy,
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
        "luma_basis_points": signed_round((candidate.means[3] - reference.means[3]) * 10_000.0 / CODE_VALUE_MAXIMUM),
        // Chroma is the spread of the three channel means, so it moves with any
        // colour cast.  It is reported as evidence and is deliberately never
        // turned into a saturation proposal.
        "chroma_code_values": signed_round(candidate.chroma - reference.chroma),
        "linear_luma_ratio_basis_points": signed_round(
            linear_ratio(reference.linear_luma, candidate.linear_luma).map_or(0.0, |ratio| (ratio - 1.0) * 10_000.0)
        ),
        "sign_convention": "candidate minus reference; proposed correction moves candidate toward reference",
    })
}

/// The bounded, unapplied starting proposal for one candidate shot.
struct MatchProposal {
    /// Exactly the controls that are proposed, as **absolute** values already
    /// composed with any existing grade and clamped to the Core descriptor
    /// range. `SetEffectParam` writes absolute values, so these are what the
    /// operation carries.
    parameters: BTreeMap<String, i64>,
    /// Per-control `current`/`delta`/`requested`/`value`/`clamped`/`min`/`max`
    /// evidence, plus the `composed`, `composition_model`, `current_parameters`
    /// and `delta_parameters` summary, so neither the clamp nor the
    /// composition is ever silent.
    details: Value,
    /// True when the proposal was composed against an existing
    /// `primary_correction` node rather than starting from neutral.
    composed: bool,
}

/// `reference / candidate`, or `None` when either side is non-positive.
///
/// A zero or negative linear mean means the ROI carried no measurable signal
/// on that axis; proposing a gain from it would be inventing evidence.
fn linear_ratio(reference: f64, candidate: f64) -> Option<f64> {
    (reference > 0.0 && candidate > 0.0 && reference.is_finite() && candidate.is_finite())
        .then(|| reference / candidate)
}

/// Inclusive Core descriptor bounds for one primary control.
///
/// Read from `EFFECT_DESCRIPTORS` rather than hardcoded so the clamp reported
/// to the caller is exactly the range Core will validate against.
fn primary_parameter_bounds(name: &str) -> (i64, i64) {
    kinewright_core::effect_descriptor("primary_correction")
        .and_then(|descriptor| {
            descriptor
                .parameter(name)
                .map(|parameter| (parameter.min, parameter.max))
        })
        .unwrap_or((i64::MIN, i64::MAX))
}

/// Derive a bounded, first-order starting proposal that moves one candidate
/// shot toward the reference shot.
///
/// This is deliberately *not* a solve.  It is a single first-order step in the
/// exact CC1 managed model, computed from linearised channel means, and every
/// term is reported with its raw value so a human or agent can see what was
/// clamped.
///
/// # Exposure
///
/// The managed model multiplies linear light by
/// `2^(exposure_milli_stops / 1000)`.  Matching the candidate's linear luma to
/// the reference's therefore needs
/// `exposure_milli_stops = 1000 * log2(reference_linear_luma / candidate_linear_luma)`.
/// A candidate darker than the reference yields a positive proposal.
///
/// # Temperature
///
/// The model scales red by `1 + 0.001 * temperature_percent` and blue by
/// `1 - 0.001 * temperature_percent` (see
/// [`CC1_WHITE_BALANCE_GAIN_PER_PERCENT`]).  Writing `p` for the proposal and
/// `k` for that per-percent gain, the candidate's red/blue ratio after
/// correction is `(candR / candB) * (1 + k*p) / (1 - k*p)`.  Setting that equal
/// to `refR / refB` and expanding to first order in `p`:
///
/// ```text
/// (1 + k*p) / (1 - k*p) ~= 1 + 2*k*p
/// 1 + 2*k*p             =  (refR/refB) / (candR/candB)
/// p                     =  ((refR/refB) / (candR/candB) - 1) / (2*k)
/// ```
///
/// A blue-cast candidate has a red/blue ratio below the reference's, so the
/// right-hand side is positive: the proposal warms the shot.
///
/// # Tint
///
/// The model scales green by `1 - 0.001 * tint_percent`.  Red and blue move in
/// opposite directions under the temperature term, so their mid-point is
/// unchanged **exactly** only for a neutral red/blue balance
/// (`candR == candB`); for any other balance a first-order residual of order
/// `k * temperature_percent * (candR - candB) / (candR + candB)` remains in the
/// mid-point, and the tint proposal absorbs it.  Writing
/// `g_cand = candG / mid(candR, candB)` and likewise for the reference, the
/// correction must satisfy `g_cand * (1 - k*p) = g_ref`, so
///
/// ```text
/// p = (1 - g_ref / g_cand) / k
/// ```
///
/// A too-green candidate has `g_cand > g_ref`, so the proposal is **positive** —
/// which is what reduces green in this model.  The previous implementation used
/// `reference - candidate` here and therefore made a green cast greener.
///
/// # Saturation
///
/// No saturation is proposed.  The available chroma statistic is the spread of
/// the three channel means, which already moves with any colour cast, so a
/// saturation term derived from it double-counts the white-balance correction.
///
/// # Composition with an existing grade
///
/// Every term above is a **delta**: it describes how much further the shot has
/// to move from where the scopes just measured it.  The scopes measure the
/// monitoring output, so a clip that already carries a `primary_correction`
/// node was measured *through* that grade.  Emitting the delta as the node's
/// absolute value would therefore discard the existing grade entirely.
///
/// When `existing` is supplied, each control is composed as
/// `existing + delta` **before** clamping, which is the documented first-order
/// additive model for these three controls, and the composed value is what the
/// proposal writes.  Composing before the clamp matters: clamping the delta
/// first and adding afterwards can land outside the descriptor range.
fn match_parameters(
    reference: ShotStats,
    candidate: ShotStats,
    existing: Option<&ExistingPrimaryNode>,
) -> MatchProposal {
    let gain_per_percent = CC1_WHITE_BALANCE_GAIN_PER_PERCENT;

    let exposure = linear_ratio(reference.linear_luma, candidate.linear_luma)
        .map(|ratio| ratio.log2() * 1_000.0);

    let reference_red_blue = linear_ratio(reference.linear[0], reference.linear[2]);
    let candidate_red_blue = linear_ratio(candidate.linear[0], candidate.linear[2]);
    let temperature = match (reference_red_blue, candidate_red_blue) {
        (Some(reference_ratio), Some(candidate_ratio)) if candidate_ratio > 0.0 => {
            Some((reference_ratio / candidate_ratio - 1.0) / (2.0 * gain_per_percent))
        }
        _ => None,
    };

    let green_over_mid = |stats: &ShotStats| {
        let mid = f64::midpoint(stats.linear[0], stats.linear[2]);
        linear_ratio(stats.linear[1], mid)
    };
    let tint = match (green_over_mid(&reference), green_over_mid(&candidate)) {
        (Some(reference_ratio), Some(candidate_ratio)) if candidate_ratio > 0.0 => {
            Some((1.0 - reference_ratio / candidate_ratio) / gain_per_percent)
        }
        _ => None,
    };

    let composed = existing.is_some();
    let mut parameters = BTreeMap::new();
    let mut current_parameters = serde_json::Map::new();
    let mut delta_parameters = serde_json::Map::new();
    let mut controls = serde_json::Map::new();
    for (name, raw) in [
        ("exposure_milli_stops", exposure),
        ("temperature_percent", temperature),
        ("tint_percent", tint),
    ] {
        let Some(raw_delta) = raw.filter(|value| value.is_finite()) else {
            continue;
        };
        let delta = signed_round(raw_delta);
        if delta == 0 {
            // Nothing to move: the composed value would equal the value the
            // node already holds, so the proposal stays empty for this control.
            continue;
        }
        let current = existing.map_or(0, |node| {
            node.parameters.get(name).copied().unwrap_or_default()
        });
        // First-order additive composition, before the clamp: clamping the
        // delta on its own and adding afterwards can leave the descriptor range.
        let requested = current.saturating_add(delta);
        let (min, max) = primary_parameter_bounds(name);
        let value = requested.clamp(min, max);
        let keyframed = existing.is_some_and(|node| node.keyframed.iter().any(|key| key == name));
        parameters.insert(name.to_owned(), value);
        current_parameters.insert(name.to_owned(), json!(current));
        delta_parameters.insert(name.to_owned(), json!(delta));
        controls.insert(
            name.to_owned(),
            json!({
                "current": current,
                "delta": delta,
                "requested": requested,
                "value": value,
                "clamped": value != requested,
                "min": min,
                "max": max,
                // The raw first-order term before rounding. It is a delta, not
                // the value written: `requested` is `current + delta`.
                "unrounded_delta": raw_delta,
                "composed": composed,
                "keyframed": keyframed,
            }),
        );
    }
    let mut details = controls;
    details.insert("composed".to_owned(), json!(composed));
    details.insert(
        "composition_model".to_owned(),
        json!(if composed {
            "existing_plus_delta_first_order_additive"
        } else {
            "absolute_delta_from_neutral"
        }),
    );
    details.insert(
        "current_parameters".to_owned(),
        Value::Object(current_parameters),
    );
    details.insert(
        "delta_parameters".to_owned(),
        Value::Object(delta_parameters),
    );
    MatchProposal {
        parameters,
        details: Value::Object(details),
        composed,
    }
}

fn assumptions(document: &Document, asset: AssetId, request: &ScopeRequest) -> Value {
    let color_description = document.asset(asset).map(|asset| &asset.color_description);
    let mut entries = vec![
        json!("RGBA8 scope values are measured after the named managed compositor boundary"),
        json!(
            "candidate-minus-reference deltas are signed display-code evidence, not a flattened LUT"
        ),
        json!({
            "proposal_basis": "linearised channel means of 8-bit monitoring codes; first-order bounded starting proposal, not a solve",
        }),
        json!({
            "saturation_proposal": "omitted",
            "reason": "the available chroma statistic is the spread of the three channel means, which already moves with any colour cast; a saturation term derived from it would double-count the white-balance proposal",
            "chroma_delta_role": "evidence_only",
        }),
        json!({"asset_id": asset.0, "source_color_description": color_description}),
        json!({"roi": request.roi.value(), "sampling": request.resolution.value()}),
    ];
    if request.resolution.proxy {
        entries.push(json!({
            "proxy_sampling": "shares_live_playback_renderer",
            "detail": "Proxy samples come from the same thumbnail renderer the live playback path uses, so they are not isolated from playback state and carry no backend/adapter attribution. Isolating the proxy renderer is engine work outside this tool.",
            "backend": Value::Null,
            "provenance": "proxy_unverified_by_backend",
        }));
    }
    if !request.include_grids {
        entries.push(json!({
            "grids_omitted": true,
            "omitted_grids": DENSITY_GRID_KEYS,
            "recovery": "Send include_grids=true to receive the waveform, parade, and vectorscope density grids.",
        }));
    }
    Value::Array(entries)
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
            lut_assets: Vec::new(),
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
        let selection = select_frames(
            TimeCode(10),
            None,
            Some(&ScopeRangeArgs {
                start: TimeCode(2),
                end: TimeCode(8),
            }),
            None,
            Some(TimeCode(2)),
        )
        .unwrap();
        assert_eq!(
            selection.frames,
            vec![TimeCode(2), TimeCode(4), TimeCode(6)]
        );
        assert_eq!(selection.step_frames, Some(TimeCode(2)));
        assert_eq!(selection.step_source, "explicit");
        let explicit = select_frames(
            TimeCode(10),
            None,
            None,
            Some(&[TimeCode(0), TimeCode(9)]),
            None,
        )
        .unwrap();
        assert_eq!(explicit.frames, vec![TimeCode(0), TimeCode(9)]);
        assert_eq!(explicit.step_frames, None);
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
    fn roi_rejects_a_subquantum_extent_by_field_name() {
        let error = NormalizedRoi::validate(Some(&ScopeRoiArgs {
            x: Some(0.5),
            y: Some(0.0),
            width: Some(0.000_01),
            height: Some(1.0),
            left: None,
            top: None,
            right: None,
            bottom: None,
        }))
        .unwrap_err();
        assert_eq!(error.code(), "invalid_roi");
        assert!(error.to_string().contains("width"), "{error}");
        assert!(error.to_string().contains("basis point"), "{error}");
    }

    #[test]
    fn only_the_exact_stage_vocabulary_is_accepted() {
        assert!(validate_stage("monitoring_post_composite").is_ok());
        assert!(validate_stage("monitoring/post-composite").is_ok());
        for rejected in [
            "post_compositor",
            "monitor",
            " monitoring_post_composite ",
            "MONITORING_POST_COMPOSITE",
            "delivery",
        ] {
            let error = validate_stage(rejected).unwrap_err();
            assert_eq!(
                error.code(),
                "unsupported_stage",
                "{rejected} must fail closed"
            );
            assert_eq!(
                error.details()["supported_stages"],
                json!(["monitoring_post_composite", "monitoring/post-composite"])
            );
        }
        assert_eq!(default_scope_stage(), "monitoring_post_composite");
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
        // The fixture is one dark pixel (10,20,30) and one bright pixel
        // (240,220,200), so every channel mean sits near the midpoint.
        assert!((stats.means[0] - 125.0).abs() < 1.0, "{:?}", stats.means);
        assert!((stats.means[1] - 120.0).abs() < 1.0, "{:?}", stats.means);
        assert!((stats.means[2] - 115.0).abs() < 1.0, "{:?}", stats.means);
        // Linearising a mid-grey code must land well below the coded value.
        assert!(
            stats.linear[0] < 0.30 && stats.linear[0] > 0.15,
            "{:?}",
            stats.linear
        );
        assert!(stats.linear_luma > 0.0 && stats.linear_luma < 0.30);

        let darker = ShotStats {
            means: [2.0, 2.0, 2.0, 2.0],
            count: 1,
            chroma: 0.0,
            linear: [0.001, 0.001, 0.001],
            linear_luma: 0.001,
        };
        let delta = signed_deltas(stats, darker);
        // Candidate minus reference: the darker candidate is negative on every
        // channel, and the sign is asserted rather than merely "is a number".
        assert!(delta["red_code_values"].as_i64().unwrap() < -100);
        assert!(delta["green_code_values"].as_i64().unwrap() < -100);
        assert!(delta["blue_code_values"].as_i64().unwrap() < -100);
        assert!(delta["luma_code_values"].as_i64().unwrap() < -100);
        assert!(delta["red_basis_points"].as_i64().unwrap() < -4_000);
        // The reference is chromatic and the candidate is neutral, so the
        // chroma delta is negative evidence.
        assert!(delta["chroma_code_values"].as_i64().unwrap() < 0);
        assert!(serde_json::to_value(evidence).unwrap()["parade"]["red"].is_object());
    }

    fn flat_stats(red: f64, green: f64, blue: f64) -> ShotStats {
        let means = [
            red,
            green,
            blue,
            0.2126 * red + 0.7152 * green + 0.0722 * blue,
        ];
        let chroma =
            (means[0].max(means[1]).max(means[2]) - means[0].min(means[1]).min(means[2])).max(0.0);
        let linear = [
            bt709_eotf(red / CODE_VALUE_MAXIMUM),
            bt709_eotf(green / CODE_VALUE_MAXIMUM),
            bt709_eotf(blue / CODE_VALUE_MAXIMUM),
        ];
        ShotStats {
            count: 2,
            means,
            chroma,
            linear_luma: BT709_LUMA_RED * linear[0]
                + BT709_LUMA_GREEN * linear[1]
                + BT709_LUMA_BLUE * linear[2],
            linear,
        }
    }

    #[test]
    fn match_parameters_move_a_candidate_toward_the_reference() {
        let reference = flat_stats(128.0, 128.0, 128.0);

        // A green cast must produce a POSITIVE tint, because the CC1 model uses
        // green_gain = 1 - 0.1 * tint.
        let green_cast = match_parameters(reference, flat_stats(128.0, 150.0, 128.0), None);
        let tint = green_cast.parameters["tint_percent"];
        assert!(
            tint > 0,
            "green cast must propose a positive tint, got {tint}"
        );
        assert!(!green_cast.parameters.contains_key("saturation_percent"));

        // A blue cast must warm the shot with a POSITIVE temperature.
        let blue_cast = match_parameters(reference, flat_stats(110.0, 128.0, 150.0), None);
        let temperature = blue_cast.parameters["temperature_percent"];
        assert!(
            temperature > 0,
            "blue cast must propose a positive temperature, got {temperature}"
        );

        // A warm candidate must cool down.
        let warm_cast = match_parameters(reference, flat_stats(150.0, 128.0, 110.0), None);
        assert!(warm_cast.parameters["temperature_percent"] < 0);

        // A dark candidate must be brightened.
        let dark = match_parameters(reference, flat_stats(64.0, 64.0, 64.0), None);
        let exposure = dark.parameters["exposure_milli_stops"];
        assert!(
            exposure > 0,
            "dark candidate must propose a positive exposure, got {exposure}"
        );
        // 128 -> 64 in code is roughly a 2.4 stop linear change, far more than
        // the ~1 stop a naive gamma-coded log2 ratio would have proposed.
        assert!(exposure > 1_500, "linearised exposure was {exposure}");

        // No difference proposes nothing at all.
        assert!(
            match_parameters(reference, reference, None)
                .parameters
                .is_empty()
        );
    }

    #[test]
    fn clamped_proposals_report_the_raw_request() {
        // An extreme cast saturates the +-100 percent temperature range.
        let proposal = match_parameters(
            flat_stats(250.0, 128.0, 20.0),
            flat_stats(20.0, 128.0, 250.0),
            None,
        );
        let details = &proposal.details["temperature_percent"];
        assert_eq!(details["max"], 100);
        assert_eq!(details["min"], -100);
        assert_eq!(details["clamped"], true);
        assert_eq!(details["value"], 100);
        assert!(details["requested"].as_i64().unwrap() > 100);
        assert_eq!(proposal.parameters["temperature_percent"], 100);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
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
            include_grids: None,
        };
        let result = plan_shot_match(&document, TimelineRevision(7), &analysis, &args).unwrap();
        assert_eq!(result["reference_shot"]["clip_id"], 1);
        assert_eq!(result["reference_retained"], true);
        assert_eq!(result["applied"], false);
        assert_eq!(result["full_resolution"], true);
        assert_eq!(result["resolution"]["full_resolution"], true);
        assert_eq!(
            result["reference_shot"]["evidence"]["scope_evidence"]["metadata"]["full_resolution"],
            true
        );
        assert_eq!(
            result["reference_shot"]["evidence"]["full_resolution"],
            true
        );
        assert_eq!(result["candidates"][0]["full_resolution"], true);
        assert!(
            result["candidates"][0]["signed_deltas"]["luma_code_values"]
                .as_i64()
                .is_some_and(|delta| delta > 0)
        );

        // The candidate (100,120,140) is darker, bluer, and greener than the
        // reference (40,50,60) is... in fact it is brighter, so the proposal
        // must darken it while correcting the blue cast.
        let parameters = &result["candidates"][0]["proposed_parameters"];
        assert!(
            parameters["exposure_milli_stops"].as_i64().unwrap() < 0,
            "brighter candidate must be darkened: {parameters}"
        );
        assert!(parameters.get("saturation_percent").is_none());
        assert!(
            result["candidates"][0]["proposal_details"]["exposure_milli_stops"]["clamped"]
                .as_bool()
                .is_some()
        );

        let operations = result["editable_operations"][0]["operations"]
            .as_array()
            .unwrap();
        assert!(!operations.is_empty());
        assert!(operations[0].get("AddEffect").is_some());
        assert_eq!(result["editable_operations"][0]["expected_revision"], 7);
        assert_eq!(
            result["editable_operations"][0]["existing_primary_node_count"],
            0
        );
        assert_eq!(result["editable_operations"][0]["created_new_node"], true);
        assert_eq!(result["editable_operations"][0]["no_change"], false);
        assert!(
            result["editable_operations"][0]["target_effect_id"]
                .as_u64()
                .is_some()
        );
        assert_eq!(*document, original);

        let stale = plan_shot_match(&document, TimelineRevision(8), &analysis, &args).unwrap_err();
        assert_eq!(stale.code(), "stale_revision");

        let unsupported = plan_shot_match(
            &document,
            TimelineRevision(7),
            &analysis,
            &PlanShotMatchArgs {
                stage: "delivery".to_owned(),
                ..args.clone()
            },
        )
        .unwrap_err();
        assert_eq!(unsupported.code(), "unsupported_stage");

        let alias_rejected = plan_shot_match(
            &document,
            TimelineRevision(7),
            &analysis,
            &PlanShotMatchArgs {
                stage: "post_compositor".to_owned(),
                ..args
            },
        )
        .unwrap_err();
        assert_eq!(alias_rejected.code(), "unsupported_stage");
    }

    #[test]
    fn shot_match_signs_are_asserted_for_synthetic_casts() {
        // Reference is neutral mid-grey; the candidate carries an explicit cast
        // in each variant below.
        let document = Arc::new(two_shot_document());
        let base = |red: u8, green: u8, blue: u8| RgbaImage {
            width: 2,
            height: 1,
            pixels: vec![red, green, blue, 255, red, green, blue, 255],
        };
        let plan = |candidate: RgbaImage| {
            let analysis = StubAnalysis {
                frames: BTreeMap::from([
                    (TimeCode(0), base(128, 128, 128)),
                    (TimeCode(30), candidate),
                ]),
            };
            plan_shot_match(
                &document,
                TimelineRevision(7),
                &analysis,
                &PlanShotMatchArgs {
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
                    include_grids: None,
                },
            )
            .unwrap()
        };
        let operations_for = |value: &Value| {
            value["editable_operations"][0]["operations"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|operation| operation.get("SetEffectParam").cloned())
                .map(|operation| {
                    (
                        operation["name"].as_str().unwrap().to_owned(),
                        operation["value"].as_i64().unwrap(),
                    )
                })
                .collect::<BTreeMap<_, _>>()
        };

        let green = plan(base(128, 150, 128));
        let green_ops = operations_for(&green);
        assert!(
            green_ops["tint_percent"] > 0,
            "green candidate must get a positive tint: {green_ops:?}"
        );
        assert!(!green_ops.contains_key("saturation_percent"));
        assert_eq!(
            green["candidates"][0]["proposed_parameters"]["tint_percent"],
            green_ops["tint_percent"]
        );

        let blue = plan(base(110, 128, 150));
        let blue_ops = operations_for(&blue);
        assert!(
            blue_ops["temperature_percent"] > 0,
            "blue candidate must get a positive temperature: {blue_ops:?}"
        );

        let dark = plan(base(64, 64, 64));
        let dark_ops = operations_for(&dark);
        assert!(
            dark_ops["exposure_milli_stops"] > 0,
            "dark candidate must get a positive exposure: {dark_ops:?}"
        );

        // Confidence and assumptions are part of the contract.
        let confidence = &dark["candidates"][0]["confidence"];
        assert!(confidence["basis_points"].as_u64().unwrap() <= 10_000);
        assert!(["low", "medium", "high"].contains(&confidence["label"].as_str().unwrap()));
        let assumptions = dark["candidates"][0]["assumptions"].as_array().unwrap();
        assert!(assumptions.iter().any(|entry| {
            entry["proposal_basis"]
                == "linearised channel means of 8-bit monitoring codes; first-order bounded starting proposal, not a solve"
        }));
        assert!(
            assumptions
                .iter()
                .any(|entry| entry["saturation_proposal"] == "omitted")
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn shot_match_targets_an_existing_primary_node_without_stacking() {
        let mut document = two_shot_document();
        // A non-neutral grade the scopes already measured through. The match
        // term is a delta on top of these values, so the proposal has to
        // compose rather than overwrite them.
        document.tracks[0].clips[1]
            .effects
            .push(kinewright_core::Effect {
                id: kinewright_core::EffectId(9),
                name: "primary_correction".to_owned(),
                parameters: BTreeMap::from([
                    (
                        "exposure_milli_stops".to_owned(),
                        kinewright_core::ParamValue::Integer(500),
                    ),
                    (
                        "temperature_percent".to_owned(),
                        kinewright_core::ParamValue::Integer(40),
                    ),
                ]),
                keyframes: BTreeMap::from([(
                    "exposure_milli_stops".to_owned(),
                    kinewright_core::AutomationCurve {
                        keyframes: vec![kinewright_core::Keyframe {
                            at: TimeCode::ZERO,
                            value: 500,
                            interpolation: kinewright_core::KeyframeInterpolation::default(),
                        }],
                    },
                )]),
            });
        let document = Arc::new(document);
        let analysis = StubAnalysis {
            frames: BTreeMap::from([
                (
                    TimeCode(0),
                    RgbaImage {
                        width: 2,
                        height: 1,
                        pixels: vec![128, 128, 128, 255, 128, 128, 128, 255],
                    },
                ),
                (
                    TimeCode(30),
                    RgbaImage {
                        width: 2,
                        height: 1,
                        pixels: vec![64, 64, 64, 255, 64, 64, 64, 255],
                    },
                ),
            ]),
        };
        let result = plan_shot_match(
            &document,
            TimelineRevision(7),
            &analysis,
            &PlanShotMatchArgs {
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
                include_grids: None,
            },
        )
        .unwrap();
        let operations = result["editable_operations"][0]["operations"]
            .as_array()
            .unwrap();
        assert!(
            operations
                .iter()
                .all(|operation| operation.get("AddEffect").is_none()),
            "an existing primary node must be corrected in place: {operations:?}"
        );
        assert_eq!(
            result["editable_operations"][0]["existing_primary_node_count"],
            1
        );
        assert_eq!(result["editable_operations"][0]["created_new_node"], false);
        assert_eq!(result["editable_operations"][0]["target_effect_id"], 9);
        assert!(
            operations
                .iter()
                .all(|operation| operation["SetEffectParam"]["effect"] == 9)
        );

        // The candidate is half the reference's code value, so the match term
        // is a large positive exposure delta and nothing else moves.
        let entry = &result["editable_operations"][0];
        assert_eq!(entry["composed"], true);
        assert_eq!(entry["current_parameters"]["exposure_milli_stops"], 500);
        let delta = entry["delta_parameters"]["exposure_milli_stops"]
            .as_i64()
            .expect("the exposure delta is reported");
        assert!(delta > 1_500, "exposure delta was {delta}");
        let composed = entry["parameters"]["exposure_milli_stops"]
            .as_i64()
            .expect("the composed exposure is proposed");
        assert_eq!(
            composed,
            500 + delta,
            "an existing grade composes with the delta instead of being discarded"
        );

        let details = &entry["proposal_details"]["exposure_milli_stops"];
        assert_eq!(details["current"], 500);
        assert_eq!(details["delta"], delta);
        assert_eq!(details["requested"], composed);
        assert_eq!(details["value"], composed);
        assert_eq!(details["composed"], true);
        assert_eq!(details["keyframed"], true);
        assert_eq!(
            entry["proposal_details"]["composition_model"],
            "existing_plus_delta_first_order_additive"
        );

        // The written operation carries the composed absolute value, not the
        // bare delta.
        let exposure_operation = operations
            .iter()
            .find(|operation| operation["SetEffectParam"]["name"] == "exposure_milli_stops")
            .expect("the exposure control is written");
        assert_eq!(exposure_operation["SetEffectParam"]["value"], composed);

        // A control whose delta rounds to zero is left exactly as the operator
        // graded it: no operation, no entry in the proposal.
        assert!(entry["parameters"].get("temperature_percent").is_none());
        assert!(
            operations
                .iter()
                .all(|operation| operation["SetEffectParam"]["name"] != "temperature_percent")
        );

        // Composing against an animated control is ambiguous, so the plan says
        // it targets the static value instead of failing silently.
        let warnings = entry["warnings"].as_array().unwrap();
        assert!(
            warnings.iter().any(|warning| {
                let warning = warning.as_str().unwrap_or_default();
                warning.contains("keyframes exposure_milli_stops")
                    && warning.contains("static value")
            }),
            "a keyframed target parameter must be reported: {warnings:?}"
        );
    }

    /// A proposal that changes nothing and would have had to create the node
    /// never allocates an effect id, so it must not publish one.
    #[test]
    fn a_no_op_shot_match_publishes_no_target_effect_id() {
        let document = Arc::new(two_shot_document());
        // Identical shots: every match term rounds to zero.
        let frame = RgbaImage {
            width: 2,
            height: 1,
            pixels: vec![128, 128, 128, 255, 128, 128, 128, 255],
        };
        let analysis = StubAnalysis {
            frames: BTreeMap::from([(TimeCode(0), frame.clone()), (TimeCode(30), frame)]),
        };
        let result = plan_shot_match(
            &document,
            TimelineRevision(7),
            &analysis,
            &PlanShotMatchArgs {
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
                include_grids: None,
            },
        )
        .unwrap();

        let entry = &result["editable_operations"][0];
        assert_eq!(entry["no_change"], true);
        assert_eq!(entry["created_new_node"], false);
        assert_eq!(entry["existing_primary_node_count"], 0);
        assert_eq!(
            entry["target_effect_id"],
            Value::Null,
            "no operation allocates this id, so it must not be published"
        );
        assert_eq!(entry["composed"], false);
        assert!(entry["operations"].as_array().unwrap().is_empty());
    }

    #[test]
    fn shot_match_rejects_more_than_the_candidate_cap_before_rendering() {
        let document = Arc::new(two_shot_document());
        let analysis = StubAnalysis {
            frames: BTreeMap::from([(TimeCode(0), image())]),
        };
        let error = plan_shot_match(
            &document,
            TimelineRevision(7),
            &analysis,
            &PlanShotMatchArgs {
                expected_revision: TimelineRevision(7),
                reference_clip_id: Some(ClipId(1)),
                reference_shot: None,
                candidate_clip_ids: (2..=18).map(ClipId).collect(),
                candidate_shots: Vec::new(),
                stage: "monitoring_post_composite".to_owned(),
                roi: None,
                resolution: None,
                proxy_sampling: false,
                max_width: None,
                bins: None,
                columns: None,
                include_grids: None,
            },
        )
        .unwrap_err();
        assert_eq!(error.code(), "excessive_sample_request");
        assert_eq!(error.details()["max"], 16);
        assert_eq!(error.details()["requested"], 17);
    }

    #[test]
    fn shot_match_marks_a_proxy_sample_at_every_level() {
        let document = Arc::new(two_shot_document());
        let analysis = StubAnalysis {
            frames: BTreeMap::from([
                (
                    TimeCode(0),
                    RgbaImage {
                        width: 2,
                        height: 1,
                        pixels: vec![128, 128, 128, 255, 128, 128, 128, 255],
                    },
                ),
                (
                    TimeCode(30),
                    RgbaImage {
                        width: 2,
                        height: 1,
                        pixels: vec![64, 64, 64, 255, 64, 64, 64, 255],
                    },
                ),
            ]),
        };
        let result = plan_shot_match(
            &document,
            TimelineRevision(7),
            &analysis,
            &PlanShotMatchArgs {
                expected_revision: TimelineRevision(7),
                reference_clip_id: Some(ClipId(1)),
                reference_shot: None,
                candidate_clip_ids: vec![ClipId(2)],
                candidate_shots: Vec::new(),
                stage: "monitoring_post_composite".to_owned(),
                roi: None,
                resolution: Some("proxy".to_owned()),
                proxy_sampling: false,
                max_width: Some(64),
                bins: None,
                columns: None,
                include_grids: None,
            },
        )
        .unwrap();
        assert_eq!(result["full_resolution"], false);
        assert_eq!(result["resolution"]["full_resolution"], false);
        assert_eq!(result["resolution"]["backend"], Value::Null);
        assert_eq!(
            result["resolution"]["provenance"],
            "proxy_unverified_by_backend"
        );
        assert_eq!(
            result["reference_shot"]["evidence"]["full_resolution"],
            false
        );
        assert_eq!(
            result["reference_shot"]["evidence"]["scope_evidence"]["metadata"]["full_resolution"],
            false
        );
        assert_eq!(
            result["reference_shot"]["evidence"]["scope_evidence"]["full_resolution"],
            false
        );
        assert_eq!(
            result["reference_shot"]["evidence"]["summary"]["full_resolution"],
            false
        );
        assert_eq!(result["candidates"][0]["full_resolution"], false);
        assert_eq!(result["candidates"][0]["shot"]["full_resolution"], false);
        assert_eq!(
            result["candidates"][0]["scope_evidence"]["metadata"]["full_resolution"],
            false
        );
        assert_eq!(
            result["candidates"][0]["scope_comparison"]["full_resolution"],
            false
        );
        assert!(
            result["candidates"][0]["assumptions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["provenance"] == "proxy_unverified_by_backend")
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
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
                include_grids: None,
            },
        )
        .unwrap();
        let temporal = &analysis_value["scopes"]["temporal"];
        assert_eq!(temporal["sample_count"], 2);
        // The requested span and the sampled span are separate facts.
        assert_eq!(temporal["requested_range"]["start"], 0);
        assert_eq!(temporal["requested_range"]["end"], 20);
        assert_eq!(temporal["step_frames"], 10);
        assert_eq!(temporal["step_source"], "explicit");
        assert_eq!(temporal["sampled_frames"]["first"], 0);
        assert_eq!(temporal["sampled_frames"]["last"], 10);
        assert_eq!(temporal["sampled_frames"]["count"], 2);
        assert!(temporal.get("range").is_none());
        assert_eq!(analysis_value["full_resolution"], true);
        assert_eq!(analysis_value["grids_omitted"], true);

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
                include_grids: None,
            },
        )
        .unwrap();
        assert_eq!(proxy_value["resolution"]["full_resolution"], false);
        assert_eq!(proxy_value["full_resolution"], false);
        assert_eq!(
            proxy_value["core_evidence"]["metadata"]["full_resolution"],
            false
        );
        assert_eq!(
            proxy_value["sample_results"][0]["metadata"]["full_resolution"],
            false
        );
        assert_eq!(proxy_value["core_evidence"]["vectorscope"]["size"], 16);
        // The typed core evidence is the single source of truth; the old
        // top-level aliases duplicated it verbatim.
        assert!(proxy_value.get("waveform").is_none());
        assert!(proxy_value.get("rgb_parade").is_none());
        assert!(proxy_value.get("vectorscope").is_none());
        assert!(proxy_value.get("histogram").is_none());
        assert_eq!(proxy_value["grids_omitted"], false);

        assert_eq!(
            SamplingResolution::parse(Some("full_resolution"), true, None)
                .unwrap_err()
                .code(),
            "invalid_request"
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn evidence_payloads_stay_small_by_default() {
        let document = Arc::new(two_shot_document());
        let analysis = StubAnalysis {
            frames: BTreeMap::from([
                (
                    TimeCode(0),
                    RgbaImage {
                        width: 2,
                        height: 1,
                        pixels: vec![128, 128, 128, 255, 128, 128, 128, 255],
                    },
                ),
                (
                    TimeCode(30),
                    RgbaImage {
                        width: 2,
                        height: 1,
                        pixels: vec![64, 64, 64, 255, 64, 64, 64, 255],
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
                range: None,
                frames: None,
                step_frames: None,
                roi: None,
                resolution: None,
                proxy_sampling: false,
                max_width: None,
                bins: None,
                columns: None,
                include_grids: None,
            },
        )
        .unwrap();
        let analysis_bytes = serde_json::to_vec(&analysis_value).unwrap().len();
        assert!(
            analysis_bytes < 20_000,
            "analyze_color_shot default payload was {analysis_bytes} bytes"
        );
        // Statistics, clipping, and histograms survive the default omission.
        assert!(analysis_value["scopes"]["core_evidence"]["histograms"].is_object());
        assert!(analysis_value["scopes"]["core_evidence"]["clipping"].is_object());
        assert!(analysis_value["scopes"]["core_evidence"]["statistics"].is_object());
        assert!(analysis_value["scopes"]["core_evidence"]["waveform"].is_null());

        let plan = plan_shot_match(
            &document,
            TimelineRevision(7),
            &analysis,
            &PlanShotMatchArgs {
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
                include_grids: None,
            },
        )
        .unwrap();
        let plan_bytes = serde_json::to_vec(&plan).unwrap().len();
        assert!(
            plan_bytes < 20_000,
            "plan_shot_match default payload was {plan_bytes} bytes"
        );

        // Opting in restores the grids and is expected to be much larger.
        let with_grids = analyze_color_shot(
            &document,
            TimelineRevision(3),
            &analysis,
            &AnalyzeColorShotArgs {
                expected_revision: TimelineRevision(3),
                clip_id: ClipId(1),
                stage: "monitoring_post_composite".to_owned(),
                timecode: None,
                range: None,
                frames: None,
                step_frames: None,
                roi: None,
                resolution: None,
                proxy_sampling: false,
                max_width: None,
                bins: None,
                columns: None,
                include_grids: Some(true),
            },
        )
        .unwrap();
        assert!(with_grids["scopes"]["core_evidence"]["waveform"].is_object());
        assert!(serde_json::to_vec(&with_grids).unwrap().len() > analysis_bytes);
    }
}
