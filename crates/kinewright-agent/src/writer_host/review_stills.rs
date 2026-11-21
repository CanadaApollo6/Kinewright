//! Reviewer-only paged decoded stills, frozen overlay reference stills, and visual-judgment
//! validation recording `RequiredReviewRow::IndependentVisualReview` through `record_review_check`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
#[cfg(all(test, target_os = "linux"))]
use std::sync::Arc;

use image::{
    ImageBuffer, ImageEncoder, Rgba,
    codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder},
    imageops::{FilterType as ResizeFilter, resize},
};
use kinewright_core::{
    Document, ExactMicros, ExportCancellation, MediaError, Rational, TimeCode, TrackKind,
    output_review::{OverlayPixels, ReviewExpectations, ReviewLane, ReviewRowResult, ReviewSpan},
};
use kinewright_media::{
    DecodedReviewStill, MAX_REVIEW_STILLS, OmittedReferenceOverlay, PlannedReviewStill,
    ReviewOutputTiming, ReviewReadSeek, ReviewStillKind, ReviewStillPlan, ReviewStillPlanInput,
    ReviewStillReason, ReviewStillsConfig, UncoveredStillRegion, decode_review_stills_page,
    plan_review_stills, sha256_bytes,
};
use kinewright_project::{
    bundle::{BundleError, ProjectBundle},
    bundle_records::{ObjectRef, SessionId},
    editorial::{AuditDisposition, AuditKind, EditorialAudit},
    owned_inputs::{InputLeaseRequest, OwnedInputManifest, OwnedInputRef},
    review_job::{
        PreparedReviewArtifact, ReviewJob, ReviewPhase, ReviewRequest, ReviewToken,
        StoredReviewExpectations,
    },
    storage_budget::StorageReservation,
    store::ProjectStore,
    studio_records::{FeedbackRevision, ResolveFeedback, TemporalStatus},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::orchestration::{
    orchestration_records::{Role, TaskRecord},
    tasks::{RuntimeTaskAuthority, TaskEndpoint},
};

use super::{
    HostCommandOwner, HostError, HostRefusal,
    task_mcp::{TaskImageBlock, TaskMcpReply},
};

/// Maximum findings accepted in one reviewer visual judgment.
pub const MAX_VISUAL_FINDINGS: usize = 32;
/// Maximum UTF-8 bytes in one visual finding summary.
pub const MAX_VISUAL_NOTE_BYTES: usize = 512;
/// Maximum JSON bytes parsed for one reviewer visual judgment.
pub const MAX_VISUAL_JUDGMENT_BYTES: usize = 64 * 1024;
/// Maximum retired still IDs retained for cross-attempt refusal diagnostics.
const MAX_RETIRED_STILL_IDS: usize = 256;
/// Maximum PNG downscale retries when a still exceeds the configured PNG byte cap.
const MAX_PNG_DOWNSCALE_ROUNDS: usize = 4;

fn refused(message: &str) -> HostError {
    HostRefusal::new("review_stills", message).into()
}

fn scope_refused(message: &str) -> HostError {
    HostRefusal::new("task_scope", message).into()
}

/// Cryptographic identity binding one served still to its plan digest, PTS, PNG bytes, artifact,
/// review token, and reviewer attempt.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReviewStillId(pub String);

impl ReviewStillId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[allow(clippy::too_many_arguments)]
    fn derive(
        review_token: &ReviewToken,
        artifact_ref: &ObjectRef,
        artifact_media_ref: &ObjectRef,
        reviewer_attempt: &SessionId,
        plan_sha256: &str,
        sample_index: u32,
        frame_index: u64,
        pts: i64,
        kind: ReviewStillKind,
        png_sha256: &str,
        png_byte_length: u64,
    ) -> Result<Self, HostError> {
        let bytes = serde_json::to_vec(&(
            "review_still_v2",
            review_token,
            artifact_ref,
            artifact_media_ref,
            reviewer_attempt,
            plan_sha256,
            sample_index,
            frame_index,
            pts,
            kind,
            png_sha256,
            png_byte_length,
        ))?;
        Ok(Self(sha256_bytes(&bytes)))
    }
}

/// Host record of one PNG served to the read-only Reviewer through `read_review_stills`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServedReviewStill {
    pub still_id: ReviewStillId,
    pub sample_index: u32,
    pub frame_index: u64,
    pub pts: i64,
    pub output_micros: u64,
    pub kind: ReviewStillKind,
    pub reasons: Vec<ReviewStillReason>,
    pub width: u32,
    pub height: u32,
    pub png_sha256: String,
    pub png_byte_length: u64,
    pub plan_sha256: String,
    pub artifact_ref: ObjectRef,
    pub artifact_media_ref: ObjectRef,
    pub review_token: ReviewToken,
    pub reviewer_attempt: SessionId,
    #[serde(default)]
    pub omitted_overlays: Vec<OmittedReferenceOverlay>,
}

/// Arguments accepted by the Reviewer-only `read_review_stills` MCP tool.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewStillPageRequest {
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub page_size: Option<u8>,
}

/// Structured page payload returned alongside the PNG `ContentBlock::image` blocks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewStillPage {
    pub review_token: ReviewToken,
    pub artifact_ref: ObjectRef,
    pub artifact_media_ref: ObjectRef,
    pub reviewer_attempt: SessionId,
    pub plan_sha256: String,
    pub total_samples: u32,
    pub page_start_sample: u32,
    pub page_end_sample: u32,
    pub next_cursor: Option<String>,
    pub dropped_count: usize,
    pub dropped_repaired_or_commented: usize,
    pub uncovered_regions_total: usize,
    pub stills: Vec<ServedReviewStill>,
}

/// Reviewer's overall decision over the planned stills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisualVerdict {
    Pass,
    Findings,
}

/// Severity of one reviewer finding tied to a served still ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisualFindingSeverity {
    Blocking,
    Advisory,
}

/// One reviewer finding grounded in a served [`ReviewStillId`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VisualFinding {
    pub still_id: ReviewStillId,
    pub severity: VisualFindingSeverity,
    pub summary: String,
}

/// Structured reviewer visual judgment submitted for validation against served stills and plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VisualJudgment {
    pub verdict: VisualVerdict,
    pub reviewed_still_ids: Vec<ReviewStillId>,
    #[serde(default)]
    pub findings: Vec<VisualFinding>,
}

/// Typed refusal reasons when validating a reviewer's [`VisualJudgment`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VisualJudgmentError {
    #[error("visual judgment exceeds byte limit ({actual} > {limit})")]
    SizeLimit { actual: usize, limit: usize },
    #[error("invalid visual judgment JSON: {0}")]
    Parse(String),
    #[error("visual judgment bounds exceeded: {0}")]
    Bounds(String),
    #[error("contradictory visual verdict: {0}")]
    ContradictoryVerdict(String),
    #[error("duplicate reviewed still id: {0}")]
    DuplicateReviewedStill(String),
    #[error(
        "still id {still_id} belongs to reviewer attempt {recorded_attempt}, not {expected_attempt}"
    )]
    StillFromAnotherAttempt {
        still_id: String,
        expected_attempt: String,
        recorded_attempt: String,
    },
    #[error("unknown or unserved review still id: {0}")]
    UnknownStillId(String),
    #[error("review still binding mismatch: {0}")]
    ArtifactOrTokenMismatch(String),
    #[error(
        "still id {still_id} does not match planned frame {expected_frame} / PTS {expected_pts}"
    )]
    PtsOrPlanMismatch {
        still_id: String,
        expected_frame: u64,
        expected_pts: i64,
    },
    #[error("missing planned review still: sample {sample_index} frame {frame_index} ({kind:?})")]
    MissingPlannedStill {
        sample_index: u32,
        frame_index: u64,
        kind: ReviewStillKind,
    },
}

/// Outcome of a validated reviewer visual judgment ready for `record_review_check`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidatedVisualReview {
    pub verdict: VisualVerdict,
    pub row_result: ReviewRowResult,
    pub reviewed_count: usize,
    pub blocking_findings: usize,
    pub advisory_findings: usize,
    pub dropped_repaired_or_commented: usize,
    pub uncovered_regions_total: usize,
    pub omitted_reference_overlays: usize,
    pub findings: Vec<VisualFinding>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActiveStillBinding {
    review_token: ReviewToken,
    artifact_ref: ObjectRef,
    artifact_media_ref: ObjectRef,
    reviewer_attempt: SessionId,
    plan_sha256: String,
}

/// Bounded per-host ledger of served reviewer stills, per-attempt page budget, and recently
/// retired attempts (so stale still IDs from prior attempts are explicitly rejected).
#[derive(Default)]
pub(super) struct ServedReviewStills {
    config: ReviewStillsConfig,
    active: Option<ActiveStillBinding>,
    pages_admitted: usize,
    served: BTreeMap<ReviewStillId, ServedReviewStill>,
    retired: VecDeque<(ReviewStillId, SessionId)>,
}

impl ServedReviewStills {
    fn rotate_binding_if_needed(&mut self, binding: &ActiveStillBinding) {
        if self.active.as_ref() != Some(binding) {
            for (id, entry) in std::mem::take(&mut self.served) {
                if self.retired.len() >= MAX_RETIRED_STILL_IDS {
                    self.retired.pop_front();
                }
                self.retired.push_back((id, entry.reviewer_attempt));
            }
            self.active = Some(binding.clone());
            self.pages_admitted = 0;
        }
    }

    fn admit_page(
        &mut self,
        binding: &ActiveStillBinding,
        max_pages_per_attempt: usize,
    ) -> Result<(), HostError> {
        self.rotate_binding_if_needed(binding);
        if self.pages_admitted >= max_pages_per_attempt {
            return Err(refused("Review stills per-attempt page budget exhausted."));
        }
        self.pages_admitted += 1;
        Ok(())
    }

    fn record_served(&mut self, binding: ActiveStillBinding, stills: &[ServedReviewStill]) {
        self.rotate_binding_if_needed(&binding);
        for still in stills {
            self.served.insert(still.still_id.clone(), still.clone());
        }
    }
}

/// Parse a reviewer's JSON visual judgment with an explicit ingress byte cap.
///
/// # Errors
/// Returns [`VisualJudgmentError::SizeLimit`] or [`VisualJudgmentError::Parse`].
pub fn parse_visual_judgment(bytes: &[u8]) -> Result<VisualJudgment, VisualJudgmentError> {
    if bytes.len() > MAX_VISUAL_JUDGMENT_BYTES {
        return Err(VisualJudgmentError::SizeLimit {
            actual: bytes.len(),
            limit: MAX_VISUAL_JUDGMENT_BYTES,
        });
    }
    serde_json::from_slice(bytes).map_err(|error| VisualJudgmentError::Parse(error.to_string()))
}

/// Frozen review context used to validate a reviewer's [`VisualJudgment`].
pub struct VisualReviewContext<'a> {
    pub plan: &'a ReviewStillPlan,
    pub plan_sha256: &'a str,
    pub token: &'a ReviewToken,
    pub artifact_ref: &'a ObjectRef,
    pub artifact_media_ref: &'a ObjectRef,
    pub reviewer_attempt: &'a SessionId,
    pub served: &'a BTreeMap<ReviewStillId, ServedReviewStill>,
    pub retired: &'a [(ReviewStillId, SessionId)],
}

/// Validate a parsed [`VisualJudgment`] against the exact planned sample set, active review token,
/// artifact identity, plan digest, reviewer attempt, and served still ledger.
///
/// Under Ruling L12, any confirmed blocking finding records [`ReviewRowResult::Failed`]; when no
/// blocking finding is present, any dropped repaired/commented point, unmappable/unreadable region,
/// or omitted overlay reference downgrades [`ReviewRowResult::Pass`] to [`ReviewRowResult::Unknown`].
///
/// # Errors
/// Refuses missing planned sample points, unknown still IDs, still IDs from another reviewer
/// attempt, artifact/token/plan/PTS mismatches, contradictory verdicts, or exceeded bounds.
pub fn validate_visual_judgment(
    judgment: &VisualJudgment,
    context: &VisualReviewContext<'_>,
) -> Result<ValidatedVisualReview, VisualJudgmentError> {
    check_judgment_bounds(judgment)?;

    let mut seen_ids = BTreeSet::new();
    let mut covered: BTreeSet<(u32, ReviewStillKind)> = BTreeSet::new();
    let mut omitted_reference_overlays = 0_usize;

    for id in &judgment.reviewed_still_ids {
        if !seen_ids.insert(id.clone()) {
            return Err(VisualJudgmentError::DuplicateReviewedStill(
                id.as_str().to_owned(),
            ));
        }
        let still = lookup_and_verify_still(id, context)?;
        if still.kind == ReviewStillKind::OverlayReference {
            omitted_reference_overlays =
                omitted_reference_overlays.saturating_add(still.omitted_overlays.len());
        }
        covered.insert((still.sample_index, still.kind));
    }

    for finding in &judgment.findings {
        let _ = lookup_and_verify_still(&finding.still_id, context)?;
        if !seen_ids.contains(&finding.still_id) {
            return Err(VisualJudgmentError::UnknownStillId(format!(
                "{} was cited in findings without being in reviewed_still_ids",
                finding.still_id.as_str()
            )));
        }
    }

    for sample in &context.plan.samples {
        if !covered.contains(&(sample.sample_index, ReviewStillKind::DecodedOutput)) {
            return Err(VisualJudgmentError::MissingPlannedStill {
                sample_index: sample.sample_index,
                frame_index: sample.frame_index,
                kind: ReviewStillKind::DecodedOutput,
            });
        }
        if sample.expects_reference()
            && !covered.contains(&(sample.sample_index, ReviewStillKind::OverlayReference))
        {
            return Err(VisualJudgmentError::MissingPlannedStill {
                sample_index: sample.sample_index,
                frame_index: sample.frame_index,
                kind: ReviewStillKind::OverlayReference,
            });
        }
    }

    let blocking_findings = judgment
        .findings
        .iter()
        .filter(|finding| finding.severity == VisualFindingSeverity::Blocking)
        .count();
    let advisory_findings = judgment.findings.len().saturating_sub(blocking_findings);

    match judgment.verdict {
        VisualVerdict::Pass if blocking_findings > 0 => {
            return Err(VisualJudgmentError::ContradictoryVerdict(
                "pass verdict cannot contain blocking findings".into(),
            ));
        }
        VisualVerdict::Findings if judgment.findings.is_empty() => {
            return Err(VisualJudgmentError::ContradictoryVerdict(
                "findings verdict requires at least one finding with a valid still_id".into(),
            ));
        }
        VisualVerdict::Pass | VisualVerdict::Findings => {}
    }

    let row_result = if blocking_findings > 0 {
        ReviewRowResult::Failed
    } else if context.plan.dropped_repaired_or_commented > 0
        || context.plan.uncovered_regions_total > 0
        || omitted_reference_overlays > 0
    {
        ReviewRowResult::Unknown
    } else {
        ReviewRowResult::Pass
    };

    Ok(ValidatedVisualReview {
        verdict: judgment.verdict,
        row_result,
        reviewed_count: judgment.reviewed_still_ids.len(),
        blocking_findings,
        advisory_findings,
        dropped_repaired_or_commented: context.plan.dropped_repaired_or_commented,
        uncovered_regions_total: context.plan.uncovered_regions_total,
        omitted_reference_overlays,
        findings: judgment.findings.clone(),
    })
}

fn check_judgment_bounds(judgment: &VisualJudgment) -> Result<(), VisualJudgmentError> {
    let max_stills = MAX_REVIEW_STILLS.saturating_mul(2);
    if judgment.reviewed_still_ids.len() > max_stills {
        return Err(VisualJudgmentError::Bounds(format!(
            "reviewed_still_ids count {} exceeds {max_stills}",
            judgment.reviewed_still_ids.len()
        )));
    }
    if judgment.findings.len() > MAX_VISUAL_FINDINGS {
        return Err(VisualJudgmentError::Bounds(format!(
            "findings count {} exceeds {MAX_VISUAL_FINDINGS}",
            judgment.findings.len()
        )));
    }
    for finding in &judgment.findings {
        if finding.summary.trim().is_empty() || finding.summary.len() > MAX_VISUAL_NOTE_BYTES {
            return Err(VisualJudgmentError::Bounds(format!(
                "finding summary must be 1..={MAX_VISUAL_NOTE_BYTES} non-empty bytes"
            )));
        }
    }
    Ok(())
}

fn lookup_and_verify_still<'a>(
    id: &ReviewStillId,
    context: &VisualReviewContext<'a>,
) -> Result<&'a ServedReviewStill, VisualJudgmentError> {
    let Some(still) = context.served.get(id) else {
        if let Some((_, recorded_attempt)) = context
            .retired
            .iter()
            .rev()
            .find(|(stored, _)| stored == id)
        {
            return Err(VisualJudgmentError::StillFromAnotherAttempt {
                still_id: id.as_str().to_owned(),
                expected_attempt: context.reviewer_attempt.as_str().to_owned(),
                recorded_attempt: recorded_attempt.as_str().to_owned(),
            });
        }
        return Err(VisualJudgmentError::UnknownStillId(id.as_str().to_owned()));
    };
    if &still.reviewer_attempt != context.reviewer_attempt {
        return Err(VisualJudgmentError::StillFromAnotherAttempt {
            still_id: id.as_str().to_owned(),
            expected_attempt: context.reviewer_attempt.as_str().to_owned(),
            recorded_attempt: still.reviewer_attempt.as_str().to_owned(),
        });
    }
    if &still.review_token != context.token
        || &still.artifact_ref != context.artifact_ref
        || &still.artifact_media_ref != context.artifact_media_ref
        || still.plan_sha256 != context.plan_sha256
    {
        return Err(VisualJudgmentError::ArtifactOrTokenMismatch(format!(
            "still {} is bound to a different review token, artifact, or still plan",
            id.as_str()
        )));
    }
    let Some(sample) = context
        .plan
        .samples
        .iter()
        .find(|sample| sample.sample_index == still.sample_index)
    else {
        return Err(VisualJudgmentError::ArtifactOrTokenMismatch(format!(
            "still {} references unknown sample index {}",
            id.as_str(),
            still.sample_index
        )));
    };
    if still.frame_index != sample.frame_index || still.pts != sample.expected_pts {
        return Err(VisualJudgmentError::PtsOrPlanMismatch {
            still_id: id.as_str().to_owned(),
            expected_frame: sample.frame_index,
            expected_pts: sample.expected_pts,
        });
    }
    Ok(still)
}

/// Collect lease requests for every `OverlayPixels::PinnedImage` referenced by frozen expectations.
/// Runs under the host lock without reading or hashing any media bytes.
fn collect_pinned_image_leases(
    store: &ProjectStore,
    manifest_ref: &ObjectRef,
    manifest: &OwnedInputManifest,
    expectations: &ReviewExpectations,
) -> Result<BTreeMap<([u8; 32], u64), InputLeaseRequest>, HostError> {
    let mut leases = BTreeMap::new();
    for overlay in expectations
        .overlays
        .captions
        .known()
        .chain(expectations.overlays.titles.known())
        .chain(expectations.overlays.cards.known())
    {
        let OverlayPixels::PinnedImage {
            sha256,
            byte_length,
        } = overlay.pixels
        else {
            continue;
        };
        if leases.contains_key(&(sha256, byte_length)) {
            continue;
        }
        let hex = hex_digest(&sha256);
        let Some(entry) = manifest
            .entries
            .iter()
            .find(|entry| entry.object.sha256 == hex && entry.object.byte_length == byte_length)
        else {
            continue;
        };
        let lease = store.input_lease_request(&OwnedInputRef {
            manifest_ref: manifest_ref.clone(),
            entry: entry.clone(),
        })?;
        leases.insert((sha256, byte_length), lease);
    }
    Ok(leases)
}

fn hex_digest(bytes: &[u8; 32]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(64);
    for byte in bytes {
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn output_duration_micros(expectations: &ReviewExpectations) -> Option<ExactMicros> {
    ExactMicros::frames(expectations.output_frames, expectations.output_fps).ok()
}

fn project_frame_to_output_micros(
    frame: i64,
    fps: Rational,
    expectations: &ReviewExpectations,
    document: &Document,
) -> Option<ExactMicros> {
    if frame < 0
        || frame >= document.duration.0
        || !fps.is_valid()
        || fps != expectations.project_fps
    {
        return None;
    }
    let at = ExactMicros::frames(TimeCode(frame), fps).ok()?;
    let limit = output_duration_micros(expectations)?;
    (at <= limit).then_some(at)
}

fn map_parent_output_span_to_candidate(
    parent_doc: &Document,
    parent_manifest: &OwnedInputManifest,
    candidate_doc: &Document,
    candidate_manifest: &OwnedInputManifest,
    expectations: &ReviewExpectations,
    start_us: u64,
    end_us: u64,
) -> Vec<ReviewSpan> {
    let mut mapped = Vec::new();
    for lane in [ReviewLane::Video, ReviewLane::Audio] {
        let target_kind = match lane {
            ReviewLane::Video => TrackKind::Video,
            ReviewLane::Audio => TrackKind::Audio,
        };
        for track in &parent_doc.tracks {
            if track.kind != target_kind {
                continue;
            }
            for clip in &track.clips {
                if !clip.enabled {
                    continue;
                }
                let Some(parent_asset) = parent_doc.asset(clip.asset) else {
                    continue;
                };
                if parent_asset.source_fingerprint.content_sha256.is_none()
                    || parent_asset.source_fingerprint.byte_len.is_none()
                {
                    continue;
                }
                let Some(parent_entry) = parent_manifest
                    .entries
                    .iter()
                    .find(|entry| entry.matches_media_asset(parent_asset))
                else {
                    continue;
                };
                let Ok(duration) = parent_doc.clip_duration(clip) else {
                    continue;
                };
                let Some(clip_end_tc) = clip.timeline_start.checked_add(duration) else {
                    continue;
                };
                let (Ok(clip_start_exact), Ok(clip_end_exact)) = (
                    ExactMicros::frames(clip.timeline_start, parent_doc.fps),
                    ExactMicros::frames(clip_end_tc, parent_doc.fps),
                ) else {
                    continue;
                };
                let clip_start_u = clip_start_exact.floor();
                let clip_end_u = clip_end_exact.floor();
                let ov_start = start_us.max(clip_start_u);
                let ov_end = end_us.min(clip_end_u);
                if ov_start >= ov_end {
                    continue;
                }
                let Ok(src_in_exact) =
                    ExactMicros::frames(clip.source_range.start, parent_asset.fps)
                else {
                    continue;
                };
                let src_in_u = src_in_exact.floor();
                let speed = u128::from(clip.speed_percent.max(1));
                let rel_start = u128::from(ov_start.saturating_sub(clip_start_u));
                let rel_end = u128::from(ov_end.saturating_sub(clip_start_u));
                let src_start_u = u64::try_from(u128::from(src_in_u) + (rel_start * speed) / 100)
                    .unwrap_or(u64::MAX);
                let src_end_u = u64::try_from(u128::from(src_in_u) + (rel_end * speed) / 100)
                    .unwrap_or(u64::MAX);
                if src_start_u >= src_end_u {
                    continue;
                }

                for seg in &expectations.segments {
                    if seg.id.lane != lane {
                        continue;
                    }
                    let Some(cand_asset) = candidate_doc.asset(seg.asset) else {
                        continue;
                    };
                    if cand_asset.source_fingerprint != parent_asset.source_fingerprint {
                        continue;
                    }
                    if !candidate_manifest.entries.iter().any(|entry| {
                        entry.object == parent_entry.object && entry.matches_media_asset(cand_asset)
                    }) {
                        continue;
                    }
                    let seg_src_start = seg.source.start.floor();
                    let seg_src_end = seg.source.end.floor();
                    let seg_ov_start = src_start_u.max(seg_src_start);
                    let seg_ov_end = src_end_u.min(seg_src_end);
                    if seg_ov_start >= seg_ov_end || seg_src_end <= seg_src_start {
                        continue;
                    }
                    let seg_src_len = u128::from(seg_src_end - seg_src_start);
                    let seg_out_start = seg.output.start.floor();
                    let seg_out_end = seg.output.end.floor();
                    let seg_out_len = u128::from(seg_out_end.saturating_sub(seg_out_start));
                    let mapped_start = u64::try_from(
                        u128::from(seg_out_start)
                            + (u128::from(seg_ov_start - seg_src_start) * seg_out_len)
                                / seg_src_len,
                    )
                    .unwrap_or(seg_out_start);
                    let mapped_end = u64::try_from(
                        u128::from(seg_out_start)
                            + (u128::from(seg_ov_end - seg_src_start) * seg_out_len) / seg_src_len,
                    )
                    .unwrap_or(seg_out_end)
                    .max(mapped_start);
                    mapped
                        .push(ExactMicros::integer(mapped_start)..ExactMicros::integer(mapped_end));
                }
            }
        }
        if !mapped.is_empty() {
            break;
        }
    }
    mapped
}

/// Extract commented and repaired output regions solely from frozen snapshot and editorial records,
/// mapping cross-version feedback through `ProjectBundle::resolve_feedback` and parent blocking
/// audits through source-time lineage. Unreadable or unmappable regions are recorded explicitly in
/// `uncovered_regions` (never silently dropped).
pub(super) fn extract_plan_input_from_snapshot(
    bundle: &ProjectBundle,
    token: &ReviewToken,
    expectations: &ReviewExpectations,
    document: &Document,
) -> Result<ReviewStillPlanInput, BundleError> {
    let snapshot = bundle.read_snapshot(&token.snapshot)?;
    let mut input = ReviewStillPlanInput::default();
    let out_dur = output_duration_micros(expectations).unwrap_or(ExactMicros::ZERO);

    // 1. Feedback revisions attached to this snapshot.
    for rev_ref in &snapshot.descriptor.revision_refs {
        let revision = match bundle.store().read_record::<FeedbackRevision>(rev_ref) {
            Ok(revision) => revision,
            Err(error) => {
                input.uncovered_regions.push(UncoveredStillRegion {
                    reason: ReviewStillReason::CommentedRegion,
                    detail: format!(
                        "failed to read feedback revision {}: {error}",
                        rev_ref.sha256
                    ),
                });
                continue;
            }
        };
        if revision.anchor.snapshot == token.snapshot {
            let mapped = Rational::new(
                revision.anchor.fps.numerator,
                revision.anchor.fps.denominator,
            )
            .ok()
            .and_then(|fps| {
                project_frame_to_output_micros(revision.anchor.frame, fps, expectations, document)
            });
            if let Some(at) = mapped {
                input.commented_regions.push(at..at);
            } else {
                input.uncovered_regions.push(UncoveredStillRegion {
                    reason: ReviewStillReason::CommentedRegion,
                    detail: format!(
                        "feedback revision {} frame {} cannot be mapped onto snapshot clock",
                        rev_ref.sha256, revision.anchor.frame
                    ),
                });
            }
        } else {
            match bundle.resolve_feedback(
                &ResolveFeedback {
                    comment_revision_ref: rev_ref.clone(),
                    candidate_checkpoint_ref: snapshot.descriptor.checkpoint_ref.clone(),
                    beat_map_ref: None,
                },
                None,
            ) {
                Err(error) => {
                    input.uncovered_regions.push(UncoveredStillRegion {
                        reason: ReviewStillReason::CommentedRegion,
                        detail: format!(
                            "failed to resolve cross-version feedback {}: {error}",
                            rev_ref.sha256
                        ),
                    });
                }
                Ok(mapped) => match mapped.resolution.temporal {
                    TemporalStatus::Exact | TemporalStatus::Mapped => {
                        let mut added = 0_usize;
                        for cand in &mapped.candidates {
                            if let Some(at) = project_frame_to_output_micros(
                                cand.frame,
                                expectations.project_fps,
                                expectations,
                                document,
                            ) {
                                input.commented_regions.push(at..at);
                                added += 1;
                            }
                        }
                        if added == 0 {
                            input.uncovered_regions.push(UncoveredStillRegion {
                                reason: ReviewStillReason::CommentedRegion,
                                detail: format!(
                                    "feedback revision {} resolved outside candidate output clock",
                                    rev_ref.sha256
                                ),
                            });
                        }
                    }
                    TemporalStatus::Removed => {}
                    TemporalStatus::Ambiguous => {
                        for cand in mapped.candidates.iter().take(8) {
                            if let Some(at) = project_frame_to_output_micros(
                                cand.frame,
                                expectations.project_fps,
                                expectations,
                                document,
                            ) {
                                input.commented_regions.push(at..at);
                            }
                        }
                        input.uncovered_regions.push(UncoveredStillRegion {
                            reason: ReviewStillReason::CommentedRegion,
                            detail: format!(
                                "feedback revision {} mapped ambiguously across {} candidate frames",
                                rev_ref.sha256,
                                mapped.candidates.len()
                            ),
                        });
                    }
                    TemporalStatus::Unresolved => {
                        input.uncovered_regions.push(UncoveredStillRegion {
                            reason: ReviewStillReason::CommentedRegion,
                            detail: format!(
                                "feedback revision {} could not be temporally mapped onto candidate cut",
                                rev_ref.sha256
                            ),
                        });
                    }
                },
            }
        }
    }

    // 2. Current snapshot's resolved audit entries (already in this candidate's output clock).
    let mut resolved_keys: Vec<(String, AuditKind)> = Vec::new();
    if let Some(audit_ref) = &snapshot.descriptor.audit_ref {
        match bundle.store().read_record::<EditorialAudit>(audit_ref) {
            Err(error) => {
                input.uncovered_regions.push(UncoveredStillRegion {
                    reason: ReviewStillReason::RepairedRegion,
                    detail: format!("failed to read current audit {}: {error}", audit_ref.sha256),
                });
            }
            Ok(audit) => {
                for entry in audit.entries() {
                    if entry.disposition == AuditDisposition::Resolved {
                        resolved_keys.push((
                            entry.observation.line_or_beat.clone(),
                            entry.observation.kind,
                        ));
                        let start = ExactMicros::integer(entry.observation.output.start_micros);
                        let end = ExactMicros::integer(entry.observation.output.end_micros);
                        if start <= end && start <= out_dur {
                            input.repaired_regions.push(start..end.min(out_dur));
                        } else {
                            input.uncovered_regions.push(UncoveredStillRegion {
                                reason: ReviewStillReason::RepairedRegion,
                                detail: format!(
                                    "resolved audit entry '{}' ({}..{} us) lies outside output duration",
                                    entry.observation.line_or_beat,
                                    entry.observation.output.start_micros,
                                    entry.observation.output.end_micros,
                                ),
                            });
                        }
                    }
                }
            }
        }
    }

    // 3. Parent snapshot's blocking audit entries not already superseded by current Resolved entries.
    if let Some(parent_version) = &snapshot.descriptor.parent_version {
        let versions = match bundle.version_index() {
            Ok(versions) => Some(versions),
            Err(error) => {
                input.uncovered_regions.push(UncoveredStillRegion {
                    reason: ReviewStillReason::RepairedRegion,
                    detail: format!(
                        "failed to read version index for parent {}: {error}",
                        parent_version.as_str()
                    ),
                });
                None
            }
        };
        if let Some(versions) = versions {
            let Some(parent_desc) = versions.snapshots.get(parent_version) else {
                input.uncovered_regions.push(UncoveredStillRegion {
                    reason: ReviewStillReason::RepairedRegion,
                    detail: format!(
                        "parent version {} missing from version index",
                        parent_version.as_str()
                    ),
                });
                return Ok(input);
            };
            if let Some(parent_audit_ref) = &parent_desc.audit_ref {
                match bundle
                    .store()
                    .read_record::<EditorialAudit>(parent_audit_ref)
                {
                    Err(error) => {
                        input.uncovered_regions.push(UncoveredStillRegion {
                            reason: ReviewStillReason::RepairedRegion,
                            detail: format!(
                                "failed to read parent audit {}: {error}",
                                parent_audit_ref.sha256
                            ),
                        });
                    }
                    Ok(parent_audit) => {
                        let pending_blocking: Vec<_> = parent_audit
                            .entries()
                            .filter(|entry| {
                                entry.disposition == AuditDisposition::Blocking
                                    && !resolved_keys.contains(&(
                                        entry.observation.line_or_beat.clone(),
                                        entry.observation.kind,
                                    ))
                            })
                            .collect();
                        if !pending_blocking.is_empty() {
                            let same_cut = parent_desc.document_ref
                                == snapshot.descriptor.document_ref
                                && parent_desc.input_manifest_ref
                                    == snapshot.descriptor.input_manifest_ref;
                            if same_cut {
                                for entry in pending_blocking {
                                    let start =
                                        ExactMicros::integer(entry.observation.output.start_micros);
                                    let end =
                                        ExactMicros::integer(entry.observation.output.end_micros);
                                    if start <= end && start <= out_dur {
                                        input.repaired_regions.push(start..end.min(out_dur));
                                    } else {
                                        input.uncovered_regions.push(UncoveredStillRegion {
                                            reason: ReviewStillReason::RepairedRegion,
                                            detail: format!(
                                                "parent blocking entry '{}' ({}..{} us) lies outside output duration",
                                                entry.observation.line_or_beat,
                                                entry.observation.output.start_micros,
                                                entry.observation.output.end_micros,
                                            ),
                                        });
                                    }
                                }
                            } else {
                                let parent_records = (
                                    bundle
                                        .store()
                                        .read_record::<Document>(&parent_desc.document_ref),
                                    bundle
                                        .store()
                                        .read_input_manifest(&parent_desc.input_manifest_ref),
                                    bundle.store().read_input_manifest(
                                        &snapshot.descriptor.input_manifest_ref,
                                    ),
                                );
                                match parent_records {
                                    (
                                        Ok(parent_doc),
                                        Ok(parent_manifest),
                                        Ok(candidate_manifest),
                                    ) => {
                                        for entry in pending_blocking {
                                            let mapped = map_parent_output_span_to_candidate(
                                                &parent_doc,
                                                &parent_manifest,
                                                document,
                                                &candidate_manifest,
                                                expectations,
                                                entry.observation.output.start_micros,
                                                entry.observation.output.end_micros,
                                            );
                                            if mapped.is_empty() {
                                                input.uncovered_regions.push(UncoveredStillRegion {
                                                    reason: ReviewStillReason::RepairedRegion,
                                                    detail: format!(
                                                        "parent blocking entry '{}' ({}..{} us) could not be mapped onto candidate output clock",
                                                        entry.observation.line_or_beat,
                                                        entry.observation.output.start_micros,
                                                        entry.observation.output.end_micros,
                                                    ),
                                                });
                                            } else {
                                                input.repaired_regions.extend(mapped);
                                            }
                                        }
                                    }
                                    _ => {
                                        input.uncovered_regions.push(UncoveredStillRegion {
                                            reason: ReviewStillReason::RepairedRegion,
                                            detail: format!(
                                                "parent version {} records unreadable for {} blocking audit entries",
                                                parent_version.as_str(),
                                                pending_blocking.len(),
                                            ),
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(input)
}

fn plan_digest(plan: &ReviewStillPlan) -> Result<String, HostError> {
    Ok(sha256_bytes(&serde_json::to_vec(plan)?))
}

fn cursor_binding_tag(binding: &ActiveStillBinding) -> Result<String, HostError> {
    let digest = sha256_bytes(&serde_json::to_vec(&(
        "review_still_cursor_v1",
        &binding.review_token,
        &binding.artifact_ref,
        &binding.artifact_media_ref,
        &binding.reviewer_attempt,
        &binding.plan_sha256,
    ))?);
    Ok(digest[..32].to_owned())
}

fn format_cursor(offset: usize, binding_tag: &str) -> String {
    format!("v1:{offset}:{binding_tag}")
}

fn parse_cursor(
    cursor: &str,
    expected_tag: &str,
    total_samples: usize,
) -> Result<usize, HostError> {
    let mut parts = cursor.split(':');
    let version = parts.next();
    let offset_str = parts.next();
    let tag = parts.next();
    if version != Some("v1") || parts.next().is_some() {
        return Err(refused("Malformed review stills cursor."));
    }
    let Some(tag) = tag else {
        return Err(refused("Malformed review stills cursor."));
    };
    if tag != expected_tag {
        return Err(refused(
            "Review stills cursor does not match the active review artifact, plan, or reviewer attempt.",
        ));
    }
    let offset = offset_str
        .and_then(|s| s.parse::<usize>().ok())
        .ok_or_else(|| refused("Malformed review stills cursor offset."))?;
    if offset >= total_samples {
        return Err(refused("Review stills cursor is out of range."));
    }
    Ok(offset)
}

/// Prepared off-lock state for one `read_review_stills` page decode.
/// Holds only pinned object descriptors; zero media bytes are hashed under the host lock.
pub(super) struct PreparedReviewStillsRead {
    binding: ActiveStillBinding,
    binding_tag: String,
    total_samples: u32,
    page_start_sample: u32,
    page_end_sample: u32,
    next_offset: Option<usize>,
    dropped_count: usize,
    dropped_repaired_or_commented: usize,
    uncovered_regions_total: usize,
    page_samples: Vec<PlannedReviewStill>,
    artifact_timing: ReviewOutputTiming,
    artifact: PreparedReviewArtifact,
    expectations: StorageReservation<StoredReviewExpectations>,
    document: StorageReservation<Document>,
    pinned_images: BTreeMap<([u8; 32], u64), InputLeaseRequest>,
    config: ReviewStillsConfig,
    cancellation: ExportCancellation,
    #[cfg(all(test, target_os = "linux"))]
    render_hook: Option<Arc<dyn Fn(&ExportCancellation) + Send + Sync>>,
}

/// Rendered off-lock PNGs and metadata awaiting re-verified host recording.
pub(super) struct RenderedReviewStillsPage {
    binding: ActiveStillBinding,
    page: ReviewStillPage,
    images: Vec<TaskImageBlock>,
}

impl PreparedReviewStillsRead {
    /// Verify artifact bytes, decode the requested page of stills (with seeking), and encode
    /// bounded PNGs completely off the host lock while honouring Stop and task cancellation.
    pub(super) fn render(
        mut self,
        endpoint: &TaskEndpoint,
    ) -> Result<RenderedReviewStillsPage, HostError> {
        #[cfg(all(test, target_os = "linux"))]
        if let Some(hook) = &self.render_hook {
            hook(&self.cancellation);
        }
        let check_live = |cancel: &ExportCancellation| -> Result<(), HostError> {
            if cancel.is_cancelled() || !endpoint.media_read_live() {
                cancel.cancel();
                return Err(refused("Output review stills read was cancelled."));
            }
            Ok(())
        };
        check_live(&self.cancellation)?;

        let artifact_file = self
            .artifact
            .open_verified_cancellable(&self.cancellation)
            .map_err(|error| {
                if self.cancellation.is_cancelled() || !endpoint.media_read_live() {
                    refused("Output review stills read was cancelled.")
                } else {
                    refused(&error.to_string())
                }
            })?;
        check_live(&self.cancellation)?;

        let mut loader =
            |sha256: [u8; 32], byte_length: u64| -> Result<Box<dyn ReviewReadSeek>, MediaError> {
                let lease_request = self
                    .pinned_images
                    .remove(&(sha256, byte_length))
                    .ok_or_else(|| {
                        MediaError::Backend(
                            "pinned overlay card not found in frozen input manifest".into(),
                        )
                    })?;
                let lease = lease_request
                    .open()
                    .map_err(|error| MediaError::Backend(error.to_string()))?;
                Ok(Box::new(lease))
            };
        let decoded = decode_review_stills_page(
            artifact_file,
            &self.artifact_timing,
            &self.expectations.0,
            &self.document,
            &mut loader,
            &self.page_samples,
            &self.config,
            &self.cancellation,
        )
        .map_err(|error| refused(&error.to_string()))?;

        let mut stills = Vec::with_capacity(decoded.stills.len());
        let mut images = Vec::with_capacity(decoded.stills.len());
        for entry in decoded.stills {
            check_live(&self.cancellation)?;
            let (still, image) =
                encode_served_still(&self.binding, &entry, self.config.max_png_bytes)?;
            stills.push(still);
            images.push(image);
        }

        let next_cursor = self
            .next_offset
            .map(|offset| format_cursor(offset, &self.binding_tag));
        let page = ReviewStillPage {
            review_token: self.binding.review_token.clone(),
            artifact_ref: self.binding.artifact_ref.clone(),
            artifact_media_ref: self.binding.artifact_media_ref.clone(),
            reviewer_attempt: self.binding.reviewer_attempt.clone(),
            plan_sha256: self.binding.plan_sha256.clone(),
            total_samples: self.total_samples,
            page_start_sample: self.page_start_sample,
            page_end_sample: self.page_end_sample,
            next_cursor,
            dropped_count: self.dropped_count,
            dropped_repaired_or_commented: self.dropped_repaired_or_commented,
            uncovered_regions_total: self.uncovered_regions_total,
            stills,
        };
        Ok(RenderedReviewStillsPage {
            binding: self.binding,
            page,
            images,
        })
    }
}

fn encode_served_still(
    binding: &ActiveStillBinding,
    still: &DecodedReviewStill,
    max_png_bytes: usize,
) -> Result<(ServedReviewStill, TaskImageBlock), HostError> {
    let (width, height, png) =
        encode_bounded_png(&still.rgba8, still.width, still.height, max_png_bytes)?;
    let png_sha256 = sha256_bytes(&png);
    let png_byte_length = u64::try_from(png.len()).unwrap_or(u64::MAX);
    let still_id = ReviewStillId::derive(
        &binding.review_token,
        &binding.artifact_ref,
        &binding.artifact_media_ref,
        &binding.reviewer_attempt,
        &binding.plan_sha256,
        still.sample_index,
        still.frame_index,
        still.pts,
        still.kind,
        &png_sha256,
        png_byte_length,
    )?;
    let served = ServedReviewStill {
        still_id,
        sample_index: still.sample_index,
        frame_index: still.frame_index,
        pts: still.pts,
        output_micros: still.output_micros,
        kind: still.kind,
        reasons: still.reasons.clone(),
        width,
        height,
        png_sha256,
        png_byte_length,
        plan_sha256: binding.plan_sha256.clone(),
        artifact_ref: binding.artifact_ref.clone(),
        artifact_media_ref: binding.artifact_media_ref.clone(),
        review_token: binding.review_token.clone(),
        reviewer_attempt: binding.reviewer_attempt.clone(),
        omitted_overlays: still.omitted_overlays.clone(),
    };
    Ok((
        served,
        TaskImageBlock {
            mime_type: "image/png",
            bytes: png,
        },
    ))
}

fn encode_bounded_png(
    rgba8: &[u8],
    width: u32,
    height: u32,
    max_png_bytes: usize,
) -> Result<(u32, u32, Vec<u8>), HostError> {
    let mut current_w = width;
    let mut current_h = height;
    let mut current_rgba: Option<Vec<u8>> = None;

    for round in 0..=MAX_PNG_DOWNSCALE_ROUNDS {
        let pixels = current_rgba.as_deref().unwrap_or(rgba8);
        let mut png = Vec::new();
        PngEncoder::new_with_quality(&mut png, CompressionType::Default, PngFilter::Adaptive)
            .write_image(
                pixels,
                current_w,
                current_h,
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|error| refused(&format!("PNG encoding failed: {error}")))?;
        if png.len() <= max_png_bytes {
            return Ok((current_w, current_h, png));
        }
        if round == MAX_PNG_DOWNSCALE_ROUNDS {
            break;
        }
        let next_w = ((u64::from(current_w) * 3) / 4).max(64);
        let next_h = ((u64::from(current_h) * 3) / 4).max(64);
        let next_w = u32::try_from(next_w).unwrap_or(64);
        let next_h = u32::try_from(next_h).unwrap_or(64);
        let Some(buffer) =
            ImageBuffer::<Rgba<u8>, _>::from_raw(current_w, current_h, pixels.to_vec())
        else {
            return Err(refused(
                "Decoded review still dimensions disagree with RGBA buffer.",
            ));
        };
        let resized = resize(&buffer, next_w, next_h, ResizeFilter::Triangle);
        current_w = next_w;
        current_h = next_h;
        current_rgba = Some(resized.into_raw());
    }
    Err(refused("Encoded review still exceeds PNG byte cap."))
}

impl HostCommandOwner {
    /// Configure the validated reviewer-stills limits enforced by `read_review_stills`.
    ///
    /// # Errors
    /// Refuses invalid or out-of-range configuration limits.
    pub fn set_review_stills_config(
        &mut self,
        config: ReviewStillsConfig,
    ) -> Result<(), HostError> {
        config
            .validate()
            .map_err(|error| refused(&error.to_string()))?;
        self.review_stills.config = config;
        Ok(())
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(super) fn set_review_stills_render_hook(
        &mut self,
        hook: Option<Arc<dyn Fn(&ExportCancellation) + Send + Sync>>,
    ) {
        self.review_runtime.stills_render_hook = hook;
    }

    /// Prepare a Reviewer-only `read_review_stills` page under the host lock without hashing any
    /// media bytes. Enforces per-attempt page budgets before releasing the lock.
    pub(super) fn prepare_review_stills_read(
        &mut self,
        task: &TaskRecord,
        authority: &RuntimeTaskAuthority,
        request: &ReviewStillPageRequest,
    ) -> Result<PreparedReviewStillsRead, HostError> {
        if task.brief.role != Role::Reviewer {
            return Err(scope_refused(
                "read_review_stills is restricted to Role::Reviewer",
            ));
        }
        let reviewer_attempt = authority.attempt.0.clone();
        let config = self.review_stills.config.clone();
        config
            .validate()
            .map_err(|error| refused(&error.to_string()))?;

        let (
            job_token,
            binding,
            binding_tag,
            total_samples,
            start,
            end,
            next_offset,
            dropped_count,
            dropped_repaired_or_commented,
            uncovered_regions_total,
            page_samples,
            artifact_timing,
            artifact,
            expectations,
            document,
            pinned_images,
        ) = {
            let (job, bundle) = self.require_active_reviewer_job(&reviewer_attempt)?;
            let snapshot = bundle.read_snapshot(&job.token.snapshot)?;
            if snapshot.descriptor.checkpoint_ref != task.brief.scope.base.checkpoint {
                return Err(scope_refused(
                    "Active review snapshot is outside the Reviewer task cut.",
                ));
            }
            let artifact_ref = job
                .artifact_ref
                .clone()
                .ok_or_else(|| refused("The output review artifact has not been published."))?;
            let review_request = bundle
                .store()
                .read_record::<ReviewRequest>(&job.request_ref)?;
            let document = bundle
                .store()
                .read_record::<Document>(&review_request.document_ref)?;
            let expectations = bundle
                .store()
                .read_record::<StoredReviewExpectations>(&review_request.expectations_ref)?;
            let manifest = bundle
                .store()
                .read_input_manifest(&review_request.input_manifest_ref)?;
            let artifact = bundle.prepare_review_artifact(&job.token)?;
            let plan_input =
                extract_plan_input_from_snapshot(bundle, &job.token, &expectations.0, &document)?;
            let plan = plan_review_stills(
                &expectations.0,
                &artifact.artifact.output,
                &plan_input,
                &config,
            )
            .map_err(|error| refused(&error.to_string()))?;
            let plan_sha256 = plan_digest(&plan)?;

            let binding = ActiveStillBinding {
                review_token: job.token.clone(),
                artifact_ref,
                artifact_media_ref: artifact.artifact.media_ref.clone(),
                reviewer_attempt,
                plan_sha256,
            };
            let binding_tag = cursor_binding_tag(&binding)?;
            let start = match request.cursor.as_deref() {
                Some(cursor) => parse_cursor(cursor, &binding_tag, plan.samples.len())?,
                None => 0,
            };
            let page_size = request
                .page_size
                .map_or(config.max_stills_per_page, usize::from)
                .clamp(1, config.max_stills_per_page);
            let end = start.saturating_add(page_size).min(plan.samples.len());
            let next_offset = (end < plan.samples.len()).then_some(end);
            let page_samples = plan.samples[start..end].to_vec();
            let pinned_images = collect_pinned_image_leases(
                bundle.store(),
                &review_request.input_manifest_ref,
                &manifest,
                &expectations.0,
            )?;
            let artifact_timing = artifact.artifact.output.clone();
            (
                job.token.clone(),
                binding,
                binding_tag,
                u32::try_from(plan.samples.len()).unwrap_or(u32::MAX),
                start,
                end,
                next_offset,
                plan.dropped_count,
                plan.dropped_repaired_or_commented,
                plan.uncovered_regions_total,
                page_samples,
                artifact_timing,
                artifact,
                expectations,
                document,
                pinned_images,
            )
        };

        self.review_stills
            .admit_page(&binding, config.max_pages_per_attempt)?;

        #[cfg(target_os = "linux")]
        let cancellation = self
            .review_runtime
            .register_review_cancellation(&job_token, None);
        #[cfg(not(target_os = "linux"))]
        let cancellation = {
            let _ = &job_token;
            ExportCancellation::default()
        };

        Ok(PreparedReviewStillsRead {
            binding,
            binding_tag,
            total_samples,
            page_start_sample: u32::try_from(start).unwrap_or(u32::MAX),
            page_end_sample: u32::try_from(end).unwrap_or(u32::MAX),
            next_offset,
            dropped_count,
            dropped_repaired_or_commented,
            uncovered_regions_total,
            page_samples,
            artifact_timing,
            artifact,
            expectations,
            document,
            pinned_images,
            config,
            cancellation,
            #[cfg(all(test, target_os = "linux"))]
            render_hook: self.review_runtime.stills_render_hook.clone(),
        })
    }

    /// Re-verify live Reviewer authority and Stop fence after off-lock decoding, record served
    /// stills in the bounded host ledger, and return the typed image-bearing MCP reply.
    pub(super) fn finish_review_stills_read(
        &mut self,
        task: &TaskRecord,
        authority: &RuntimeTaskAuthority,
        rendered: RenderedReviewStillsPage,
    ) -> Result<TaskMcpReply, HostError> {
        if task.brief.role != Role::Reviewer {
            return Err(scope_refused(
                "read_review_stills is restricted to Role::Reviewer",
            ));
        }
        let reviewer_attempt = authority.attempt.0.clone();
        let (job, _) = self.require_active_reviewer_job(&reviewer_attempt)?;
        if job.token != rendered.binding.review_token
            || job.artifact_ref.as_ref() != Some(&rendered.binding.artifact_ref)
        {
            return Err(refused(
                "The active output review artifact or generation changed while decoding stills.",
            ));
        }
        self.review_stills
            .record_served(rendered.binding, &rendered.page.stills);
        let summary = format!(
            "Review stills samples {}..{} of {} ({} image blocks; next_cursor={})",
            rendered.page.page_start_sample,
            rendered.page.page_end_sample,
            rendered.page.total_samples,
            rendered.images.len(),
            rendered.page.next_cursor.as_deref().unwrap_or("none"),
        );
        Ok(TaskMcpReply::WithImages {
            summary,
            structured: serde_json::to_value(&rendered.page)?,
            images: rendered.images,
        })
    }

    fn require_active_reviewer_job(
        &self,
        reviewer_attempt: &SessionId,
    ) -> Result<(StorageReservation<ReviewJob>, &ProjectBundle), HostError> {
        let bundle = self.owner()?.review_bundle()?;
        let job = bundle
            .review_job()?
            .ok_or_else(|| refused("No active output review job is checking."))?;
        #[cfg(target_os = "linux")]
        self.review_can_record(&job.token)?;
        #[cfg(not(target_os = "linux"))]
        if self.lifecycle.stopping()
            || !self.retention_proven
            || self.hooks.pending.is_some()
            || self
                .owner()
                .is_ok_and(kinewright_project::editing_session::Ja1EditingSession::switch_publication_unresolved)
        {
            return Err(refused("Output review authority was revoked."));
        }
        let ReviewPhase::Checking(checks) = &job.phase else {
            return Err(refused(
                "The active output review is not in the checking phase.",
            ));
        };
        if checks.reviewer_attempt.as_ref() != Some(reviewer_attempt) {
            return Err(refused(
                "This reviewer attempt is not bound to the active output review check.",
            ));
        }
        Ok((job, bundle))
    }

    /// Validate a reviewer's visual judgment against the bound configuration, frozen plan, and
    /// served still ledger, then record `RequiredReviewRow::IndependentVisualReview` exclusively
    /// through `record_review_check`.
    ///
    /// # Errors
    /// Refuses with a descriptive error (without recording any check row) when the configuration
    /// fails validation or disagrees with `binding.configuration_sha256`, the judgment omits any
    /// planned sample point, cites a still ID from another attempt or plan, fails binding checks,
    /// or the Stop/checking fence rejects recording.
    #[cfg(target_os = "linux")]
    pub fn record_visual_review_judgment(
        &mut self,
        token: &ReviewToken,
        binding: kinewright_project::review_job::ReviewCheckBinding,
        config: &super::review_overlays::C6CheckConfiguration,
        judgment: &VisualJudgment,
    ) -> Result<
        (
            ValidatedVisualReview,
            kinewright_project::review_job::ReviewStatus,
        ),
        HostError,
    > {
        use kinewright_core::output_review::RequiredReviewRow;
        use kinewright_project::review_job::UpdateReviewJob;

        config.validate()?;
        if binding.configuration_sha256 != config.configuration_sha256() {
            return Err(refused(
                "Visual review configuration does not match the bound configuration digest.",
            ));
        }
        self.review_can_record(token)?;
        let (plan, plan_sha256, artifact_ref, artifact_media_ref) = {
            let bundle = self.owner()?.review_source_bundle(&token.project)?;
            let job = bundle
                .review_job()?
                .filter(|job| &job.token == token)
                .ok_or_else(|| refused("The output review generation or version changed."))?;
            let ReviewPhase::Checking(checks) = &job.phase else {
                return Err(refused("The output review is not in the checking phase."));
            };
            if checks.configuration_sha256 != Some(binding.configuration_sha256)
                || checks.reviewer_attempt.as_ref() != Some(&binding.reviewer_attempt)
            {
                return Err(refused(
                    "Visual review judgment does not match the bound reviewer attempt or configuration.",
                ));
            }
            let artifact_ref = job
                .artifact_ref
                .clone()
                .ok_or_else(|| refused("The output review artifact has not been published."))?;
            let review_request = bundle
                .store()
                .read_record::<ReviewRequest>(&job.request_ref)?;
            let document = bundle
                .store()
                .read_record::<Document>(&review_request.document_ref)?;
            let expectations = bundle
                .store()
                .read_record::<StoredReviewExpectations>(&review_request.expectations_ref)?;
            let artifact = bundle
                .store()
                .read_record::<kinewright_project::review_job::ReviewArtifact>(&artifact_ref)?;
            let plan_input =
                extract_plan_input_from_snapshot(bundle, token, &expectations.0, &document)?;
            let plan = plan_review_stills(
                &expectations.0,
                &artifact.output,
                &plan_input,
                &config.stills,
            )
            .map_err(|error| refused(&error.to_string()))?;
            let plan_sha256 = plan_digest(&plan)?;
            (plan, plan_sha256, artifact_ref, artifact.media_ref.clone())
        };

        let retired = self.review_stills.retired.make_contiguous();
        let context = VisualReviewContext {
            plan: &plan,
            plan_sha256: &plan_sha256,
            token,
            artifact_ref: &artifact_ref,
            artifact_media_ref: &artifact_media_ref,
            reviewer_attempt: &binding.reviewer_attempt,
            served: &self.review_stills.served,
            retired,
        };
        let validated = validate_visual_judgment(judgment, &context)
            .map_err(|error| HostRefusal::new("review_visual_judgment", &error.to_string()))?;

        let status = self.record_review_check(
            token,
            UpdateReviewJob::RecordCheck {
                binding,
                row: RequiredReviewRow::IndependentVisualReview,
                result: validated.row_result,
            },
        )?;
        Ok((validated, status))
    }

    /// Expose the frozen still plan for an active review job (read-only).
    #[cfg(target_os = "linux")]
    pub fn review_still_plan(
        &self,
        token: &ReviewToken,
        config: &ReviewStillsConfig,
    ) -> Result<ReviewStillPlan, HostError> {
        let bundle = self.owner()?.review_source_bundle(&token.project)?;
        let job = bundle
            .review_job()?
            .filter(|job| &job.token == token)
            .ok_or_else(|| refused("The output review generation or version changed."))?;
        let artifact_ref = job
            .artifact_ref
            .as_ref()
            .ok_or_else(|| refused("The output review artifact has not been published."))?;
        let review_request = bundle
            .store()
            .read_record::<ReviewRequest>(&job.request_ref)?;
        let document = bundle
            .store()
            .read_record::<Document>(&review_request.document_ref)?;
        let expectations = bundle
            .store()
            .read_record::<StoredReviewExpectations>(&review_request.expectations_ref)?;
        let artifact = bundle
            .store()
            .read_record::<kinewright_project::review_job::ReviewArtifact>(artifact_ref)?;
        let plan_input =
            extract_plan_input_from_snapshot(bundle, token, &expectations.0, &document)?;
        plan_review_stills(&expectations.0, &artifact.output, &plan_input, config)
            .map_err(|error| refused(&error.to_string()))
    }
}
