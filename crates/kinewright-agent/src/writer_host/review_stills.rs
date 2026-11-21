//! Reviewer-only paged decoded stills, frozen overlay reference stills, and visual-judgment
//! validation recording `RequiredReviewRow::IndependentVisualReview` through `record_review_check`.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs::File,
};

use image::{
    ImageBuffer, ImageEncoder, Rgba,
    codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder},
    imageops::{FilterType as ResizeFilter, resize},
};
use kinewright_core::{
    Document, ExactMicros, ExportCancellation, MediaError, Rational, TimeCode,
    output_review::{OverlayPixels, ReviewExpectations, ReviewRowResult},
};
use kinewright_media::{
    DecodedReviewStill, MAX_REVIEW_STILLS, MAX_STILL_LONG_EDGE, MAX_STILL_PNG_BYTES,
    MAX_STILLS_PER_PAGE, PlannedReviewStill, ReviewOutputTiming, ReviewReadSeek, ReviewStillKind,
    ReviewStillPlan, ReviewStillPlanInput, ReviewStillReason, decode_review_stills_page,
    plan_review_stills, sha256_bytes,
};
use kinewright_project::{
    bundle::{BundleError, ProjectBundle},
    bundle_records::{ObjectRef, SessionId},
    editorial::{AuditDisposition, EditorialAudit},
    owned_inputs::{InputLeaseRequest, OwnedInputManifest, OwnedInputRef},
    review_job::{ReviewJob, ReviewPhase, ReviewRequest, ReviewToken, StoredReviewExpectations},
    storage_budget::StorageReservation,
    store::ProjectStore,
    studio_records::FeedbackRevision,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::orchestration::{
    orchestration_records::{Role, TaskRecord},
    tasks::RuntimeTaskAuthority,
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
/// Maximum PNG downscale retries when a still exceeds [`MAX_STILL_PNG_BYTES`].
const MAX_PNG_DOWNSCALE_ROUNDS: usize = 4;

fn refused(message: &str) -> HostError {
    HostRefusal::new("review_stills", message).into()
}

fn scope_refused(message: &str) -> HostError {
    HostRefusal::new("task_scope", message).into()
}

/// Cryptographic identity binding one served still to its PTS, PNG bytes, artifact, review token,
/// and reviewer attempt.
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
        sample_index: u32,
        frame_index: u64,
        pts: i64,
        kind: ReviewStillKind,
        png_sha256: &str,
        png_byte_length: u64,
    ) -> Result<Self, HostError> {
        let bytes = serde_json::to_vec(&(
            "review_still_v1",
            review_token,
            artifact_ref,
            artifact_media_ref,
            reviewer_attempt,
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
    pub artifact_ref: ObjectRef,
    pub artifact_media_ref: ObjectRef,
    pub review_token: ReviewToken,
    pub reviewer_attempt: SessionId,
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

/// Bounded per-host ledger of served reviewer stills for the active checking attempt and recently
/// retired attempts (so stale still IDs from prior attempts are explicitly rejected).
#[derive(Default)]
pub(super) struct ServedReviewStills {
    active: Option<ActiveStillBinding>,
    served: BTreeMap<ReviewStillId, ServedReviewStill>,
    retired: VecDeque<(ReviewStillId, SessionId)>,
}

impl ServedReviewStills {
    fn record_served(&mut self, binding: ActiveStillBinding, stills: &[ServedReviewStill]) {
        if self.active.as_ref() != Some(&binding) {
            for (id, entry) in std::mem::take(&mut self.served) {
                if self.retired.len() >= MAX_RETIRED_STILL_IDS {
                    self.retired.pop_front();
                }
                self.retired.push_back((id, entry.reviewer_attempt));
            }
            self.active = Some(binding);
        }
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
    pub token: &'a ReviewToken,
    pub artifact_ref: &'a ObjectRef,
    pub artifact_media_ref: &'a ObjectRef,
    pub reviewer_attempt: &'a SessionId,
    pub served: &'a BTreeMap<ReviewStillId, ServedReviewStill>,
    pub retired: &'a [(ReviewStillId, SessionId)],
}

/// Validate a parsed [`VisualJudgment`] against the exact planned sample set, active review token,
/// artifact identity, reviewer attempt, and served still ledger.
///
/// # Errors
/// Refuses missing planned sample points, unknown still IDs, still IDs from another reviewer
/// attempt, artifact/token/PTS mismatches, contradictory verdicts, or exceeded bounds.
pub fn validate_visual_judgment(
    judgment: &VisualJudgment,
    context: &VisualReviewContext<'_>,
) -> Result<ValidatedVisualReview, VisualJudgmentError> {
    check_judgment_bounds(judgment)?;

    let mut seen_ids = BTreeSet::new();
    let mut covered: BTreeSet<(u32, ReviewStillKind)> = BTreeSet::new();

    for id in &judgment.reviewed_still_ids {
        if !seen_ids.insert(id.clone()) {
            return Err(VisualJudgmentError::DuplicateReviewedStill(
                id.as_str().to_owned(),
            ));
        }
        let still = lookup_and_verify_still(id, context)?;
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
    } else {
        ReviewRowResult::Pass
    };

    Ok(ValidatedVisualReview {
        verdict: judgment.verdict,
        row_result,
        reviewed_count: judgment.reviewed_still_ids.len(),
        blocking_findings,
        advisory_findings,
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
    {
        return Err(VisualJudgmentError::ArtifactOrTokenMismatch(format!(
            "still {} is bound to a different review token or artifact",
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
pub(super) fn collect_pinned_image_leases(
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

/// Extract commented and repaired output regions solely from frozen snapshot and editorial records.
pub(super) fn extract_plan_input_from_snapshot(
    bundle: &ProjectBundle,
    token: &ReviewToken,
) -> Result<ReviewStillPlanInput, BundleError> {
    let snapshot = bundle.read_snapshot(&token.snapshot)?;
    let mut input = ReviewStillPlanInput::default();

    for rev_ref in &snapshot.descriptor.revision_refs {
        if let Ok(revision) = bundle.store().read_record::<FeedbackRevision>(rev_ref)
            && let Ok(fps) = Rational::new(
                revision.anchor.fps.numerator,
                revision.anchor.fps.denominator,
            )
            && fps.is_valid()
            && revision.anchor.frame >= 0
            && let Ok(at) = ExactMicros::frames(TimeCode(revision.anchor.frame), fps)
        {
            input.commented_regions.push(at..at);
        }
    }

    if let Some(parent_version) = &snapshot.descriptor.parent_version
        && let Ok(versions) = bundle.version_index()
        && let Some(parent_desc) = versions.snapshots.get(parent_version)
        && let Some(audit_ref) = &parent_desc.audit_ref
        && let Ok(audit) = bundle.store().read_record::<EditorialAudit>(audit_ref)
    {
        for entry in audit.entries() {
            if entry.disposition == AuditDisposition::Blocking {
                let start = ExactMicros::integer(entry.observation.output.start_micros);
                let end = ExactMicros::integer(entry.observation.output.end_micros);
                if start <= end {
                    input.repaired_regions.push(start..end);
                }
            }
        }
    }

    if let Some(audit_ref) = &snapshot.descriptor.audit_ref
        && let Ok(audit) = bundle.store().read_record::<EditorialAudit>(audit_ref)
    {
        for entry in audit.entries() {
            if entry.disposition == AuditDisposition::Resolved {
                let start = ExactMicros::integer(entry.observation.output.start_micros);
                let end = ExactMicros::integer(entry.observation.output.end_micros);
                if start <= end {
                    input.repaired_regions.push(start..end);
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
pub(super) struct PreparedReviewStillsRead {
    binding: ActiveStillBinding,
    binding_tag: String,
    total_samples: u32,
    page_start_sample: u32,
    page_end_sample: u32,
    next_offset: Option<usize>,
    page_samples: Vec<PlannedReviewStill>,
    artifact_timing: ReviewOutputTiming,
    artifact_file: File,
    expectations: StorageReservation<StoredReviewExpectations>,
    document: StorageReservation<Document>,
    pinned_images: BTreeMap<([u8; 32], u64), InputLeaseRequest>,
}

/// Rendered off-lock PNGs and metadata awaiting re-verified host recording.
pub(super) struct RenderedReviewStillsPage {
    binding: ActiveStillBinding,
    page: ReviewStillPage,
    images: Vec<TaskImageBlock>,
}

impl PreparedReviewStillsRead {
    /// Decode the requested page of stills and encode bounded PNGs completely off the host lock.
    pub(super) fn render(
        mut self,
        cancellation: &ExportCancellation,
    ) -> Result<RenderedReviewStillsPage, HostError> {
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
            self.artifact_file,
            &self.artifact_timing,
            &self.expectations.0,
            &self.document,
            &mut loader,
            &self.page_samples,
            MAX_STILL_LONG_EDGE,
            cancellation,
        )
        .map_err(|error| refused(&error.to_string()))?;

        let mut stills = Vec::with_capacity(decoded.len());
        let mut images = Vec::with_capacity(decoded.len());
        for entry in decoded {
            let (still, image) = encode_served_still(&self.binding, &entry)?;
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
) -> Result<(ServedReviewStill, TaskImageBlock), HostError> {
    let (width, height, png) = encode_bounded_png(&still.rgba8, still.width, still.height)?;
    let png_sha256 = sha256_bytes(&png);
    let png_byte_length = u64::try_from(png.len()).unwrap_or(u64::MAX);
    let still_id = ReviewStillId::derive(
        &binding.review_token,
        &binding.artifact_ref,
        &binding.artifact_media_ref,
        &binding.reviewer_attempt,
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
        artifact_ref: binding.artifact_ref.clone(),
        artifact_media_ref: binding.artifact_media_ref.clone(),
        review_token: binding.review_token.clone(),
        reviewer_attempt: binding.reviewer_attempt.clone(),
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
        if png.len() <= MAX_STILL_PNG_BYTES {
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
    /// Prepare a Reviewer-only `read_review_stills` page under the host lock.
    pub(super) fn prepare_review_stills_read(
        &self,
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
        let (artifact, artifact_file) = bundle.open_review_artifact(&job.token)?;
        let plan_input = extract_plan_input_from_snapshot(bundle, &job.token)?;
        let plan = plan_review_stills(&expectations.0, &artifact.output, &plan_input)
            .map_err(|error| refused(&error.to_string()))?;
        let plan_sha256 = plan_digest(&plan)?;

        let binding = ActiveStillBinding {
            review_token: job.token.clone(),
            artifact_ref,
            artifact_media_ref: artifact.media_ref.clone(),
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
            .map_or(MAX_STILLS_PER_PAGE, usize::from)
            .clamp(1, MAX_STILLS_PER_PAGE);
        let end = start.saturating_add(page_size).min(plan.samples.len());
        let next_offset = (end < plan.samples.len()).then_some(end);
        let page_samples = plan.samples[start..end].to_vec();
        let pinned_images = collect_pinned_image_leases(
            bundle.store(),
            &review_request.input_manifest_ref,
            &manifest,
            &expectations.0,
        )?;

        Ok(PreparedReviewStillsRead {
            binding,
            binding_tag,
            total_samples: u32::try_from(plan.samples.len()).unwrap_or(u32::MAX),
            page_start_sample: u32::try_from(start).unwrap_or(u32::MAX),
            page_end_sample: u32::try_from(end).unwrap_or(u32::MAX),
            next_offset,
            page_samples,
            artifact_timing: artifact.output.clone(),
            artifact_file,
            expectations,
            document,
            pinned_images,
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

    /// Validate a reviewer's visual judgment against the frozen plan and served still ledger, then
    /// record `RequiredReviewRow::IndependentVisualReview` exclusively through `record_review_check`.
    ///
    /// # Errors
    /// Refuses with a descriptive error (without recording any check row) when the judgment omits
    /// any planned sample point, cites a still ID from another attempt, fails binding checks, or
    /// the Stop/checking fence rejects recording.
    #[cfg(target_os = "linux")]
    pub fn record_visual_review_judgment(
        &mut self,
        token: &ReviewToken,
        binding: kinewright_project::review_job::ReviewCheckBinding,
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

        self.review_can_record(token)?;
        let (plan, artifact_ref, artifact_media_ref) = {
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
            let expectations = bundle
                .store()
                .read_record::<StoredReviewExpectations>(&review_request.expectations_ref)?;
            let artifact = bundle
                .store()
                .read_record::<kinewright_project::review_job::ReviewArtifact>(&artifact_ref)?;
            let plan_input = extract_plan_input_from_snapshot(bundle, token)?;
            let plan = plan_review_stills(&expectations.0, &artifact.output, &plan_input)
                .map_err(|error| refused(&error.to_string()))?;
            (plan, artifact_ref, artifact.media_ref.clone())
        };

        let retired = self.review_stills.retired.make_contiguous();
        let context = VisualReviewContext {
            plan: &plan,
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
    pub fn review_still_plan(&self, token: &ReviewToken) -> Result<ReviewStillPlan, HostError> {
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
        let expectations = bundle
            .store()
            .read_record::<StoredReviewExpectations>(&review_request.expectations_ref)?;
        let artifact = bundle
            .store()
            .read_record::<kinewright_project::review_job::ReviewArtifact>(artifact_ref)?;
        let plan_input = extract_plan_input_from_snapshot(bundle, token)?;
        plan_review_stills(&expectations.0, &artifact.output, &plan_input)
            .map_err(|error| refused(&error.to_string()))
    }
}
