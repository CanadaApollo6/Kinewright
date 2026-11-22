//! The output review seal: one immutable receipt per version and coverage profile.
//!
//! The seal rechecks, under writer authority and in this order: the frozen key
//! (request inputs, frozen annotations, artifact bytes, checker configuration and
//! reviewer attempt), every required row of the profile, eligibility, the
//! editorial audit, reviewer independence, cleanup and unresolved blocking
//! findings. Any failure pauses the job with findings; nothing is sealed.
use std::collections::BTreeMap;

use kinewright_core::{
    ExportSettings,
    output_review::{
        CoverageProfileId, DeliveryException, RequiredReviewRow, ReviewCutIdentity,
        ReviewRowResult, SealedReviewOutcome, delivery_eligibility,
    },
};
use serde::{Deserialize, Serialize};

use super::{
    ReviewArtifact, ReviewCheckpoint, ReviewChecks, ReviewEvidence, ReviewJob, ReviewPhase,
    ReviewRequest, ReviewStatus, ReviewToken, StoredReviewExpectations, TerminalBudget, encode,
    status,
};
use crate::{
    bundle::{BundleError, ProjectBundle},
    bundle_records::{ObjectRef, RecordKind, SessionId},
    editorial::{AuditDisposition, EditorialAudit},
    orchestration_records::{AttemptState, OrchestrationPayload, Role, TaskId},
    storage_budget::{StorageLimits, StorageReservation},
    store::{BundleRecord, StorePin, record_sealed},
    studio_records::{AudioRange, SnapshotDescriptor, SnapshotKey, VersionId, bounded, kind},
    studio_store::{PendingReviewUpdate, ValidatedMetadataUpdate, record},
};

/// One receipt per coverage profile; S5a defines one profile.
pub(crate) const MAX_VERSION_RECEIPTS: usize = 4;
pub const MAX_REVIEW_FINDINGS: usize = 16;
pub const MAX_REVIEW_FINDING_NOTE: usize = 256;
const MAX_RECEIPT_EXCEPTIONS: usize = 3 * kinewright_core::output_review::MAX_REVIEW_ELEMENTS + 3;

/// The two sealed outcomes. `Delivered` only when every required row passed and
/// the cut is eligible; never upgraded later.
pub type ReviewOutcome = SealedReviewOutcome;

/// Everything a seal is bound to. Its digest is the `ReadyToSeal` proposed key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenReviewKey {
    pub snapshot: SnapshotKey,
    pub identity: ReviewCutIdentity,
    pub request_ref: ObjectRef,
    pub document_ref: ObjectRef,
    pub input_manifest_ref: ObjectRef,
    pub expectations_ref: ObjectRef,
    /// The frozen request annotations, never the version's current reference.
    pub annotations_ref: ObjectRef,
    pub rulebook_ref: Option<ObjectRef>,
    pub audit_ref: Option<ObjectRef>,
    pub settings: ExportSettings,
    pub profile: CoverageProfileId,
    pub artifact_ref: ObjectRef,
    /// Hash and length of the reviewed bytes.
    pub media_ref: ObjectRef,
    /// Decoder version, selected streams and the presented PTS map.
    pub output: kinewright_media::ReviewOutputTiming,
    pub configuration_sha256: [u8; 32],
    pub reviewer_attempt: SessionId,
}

impl FrozenReviewKey {
    pub fn sha256(&self) -> Result<String, BundleError> {
        encode(&("kinewright/review-seal-key/v1", self))
            .map(|bytes| kinewright_media::sha256_bytes(&bytes))
    }
}

/// The immutable seal on the exact version, kept in its review evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewReceipt {
    pub token: ReviewToken,
    /// The seal command that published this receipt.
    pub command: SessionId,
    pub key: FrozenReviewKey,
    pub key_sha256: String,
    pub rows: BTreeMap<RequiredReviewRow, ReviewRowResult>,
    pub outcome: ReviewOutcome,
}

/// A finding that stopped a seal, linked to the version and, where it has one,
/// an output time range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewFinding {
    pub version: VersionId,
    pub row: Option<RequiredReviewRow>,
    pub output: Option<AudioRange>,
    pub note: String,
}

impl ReviewFinding {
    pub(crate) fn valid(findings: &[Self]) -> bool {
        findings.len() <= MAX_REVIEW_FINDINGS
            && findings.iter().all(|finding| {
                !finding.note.is_empty()
                    && finding.note.len() <= MAX_REVIEW_FINDING_NOTE
                    && finding
                        .output
                        .as_ref()
                        .is_none_or(|range| range.start_micros < range.end_micros)
            })
    }
}

record!(
    ReviewReceipt,
    ReviewReceipt,
    |v: &ReviewReceipt| [
        Some(&v.key.request_ref),
        Some(&v.key.artifact_ref),
        v.key.audit_ref.as_ref(),
    ]
    .into_iter()
    .flatten()
    .cloned()
    .collect(),
    |v: &ReviewReceipt, _: &StorageLimits| {
        let key = &v.key;
        kind(&key.request_ref, RecordKind::ReviewRequest)?;
        kind(&key.document_ref, RecordKind::Document)?;
        kind(&key.input_manifest_ref, RecordKind::InputManifest)?;
        kind(&key.expectations_ref, RecordKind::ReviewExpectations)?;
        kind(&key.annotations_ref, RecordKind::ReviewAnnotations)?;
        kind(&key.artifact_ref, RecordKind::ReviewArtifact)?;
        kind(&key.media_ref, RecordKind::OwnedMedia)?;
        if let Some(rulebook) = &key.rulebook_ref {
            kind(rulebook, RecordKind::RulebookRevision)?;
        }
        let audit = key
            .audit_ref
            .as_ref()
            .ok_or_else(|| BundleError::Corrupt("review receipt audit".into()))?;
        kind(audit, RecordKind::EditorialAudit)?;
        let required = key.profile.required_rows();
        if v.token.snapshot != key.snapshot
            || v.key_sha256 != key.sha256()?
            || v.rows.len() != required.len()
            || required
                .iter()
                .any(|row| v.rows.get(row) != Some(&ReviewRowResult::Pass))
        {
            return Err(BundleError::Corrupt("review receipt binding".into()));
        }
        if let SealedReviewOutcome::ReviewedFactsNotChecked { exceptions } = &v.outcome
            && (exceptions.is_empty() || exceptions.len() > MAX_RECEIPT_EXCEPTIONS)
        {
            return Err(BundleError::Corrupt("review receipt exceptions".into()));
        }
        Ok(())
    }
);

/// The reviewed bytes, hashed off the writer lock and kept pinned until the seal.
pub struct VerifiedReviewArtifact {
    pub(super) artifact_ref: ObjectRef,
    pub(super) media_ref: ObjectRef,
    _pin: StorePin,
}

impl super::PreparedReviewArtifact {
    /// Hash the pinned artifact bytes off the writer lock for the seal.
    pub fn verify_for_seal(
        self,
        cancellation: &kinewright_core::ExportCancellation,
    ) -> Result<VerifiedReviewArtifact, BundleError> {
        drop(self.open_verified_cancellable(cancellation)?);
        Ok(VerifiedReviewArtifact {
            artifact_ref: self.reference,
            media_ref: self.artifact.media_ref.clone(),
            _pin: self.pin,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedReview {
    pub receipt_ref: ObjectRef,
    pub receipt: ReviewReceipt,
    /// The receipt already existed: a lost reply or a second seal.
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SealResult {
    Sealed(Box<SealedReview>),
    Paused(Box<ReviewStatus>),
}

fn finding(version: &VersionId, row: Option<RequiredReviewRow>, note: &str) -> ReviewFinding {
    ReviewFinding {
        version: version.clone(),
        row,
        output: None,
        note: note.chars().take(MAX_REVIEW_FINDING_NOTE / 4).collect(),
    }
}

const NO_AUDIT: &str = "The version has no complete editorial audit.";
const NOT_A_REVIEWER: &str = "The visual reviewer is not a Reviewer task attempt on this cut.";
const OTHER_RULEBOOK: &str = "The editorial audit was made under another rulebook.";

/// Why `audit` does not bind to the version and the job's frozen rulebook.
fn audit_binding(
    descriptor: &SnapshotDescriptor,
    audit: &EditorialAudit,
    rulebook: Option<&ObjectRef>,
) -> Option<&'static str> {
    if crate::editorial_store::validate_editorial_binding(descriptor, audit).is_err() {
        return Some("The editorial audit is not bound to this version.");
    }
    (audit.rulebook_ref() != rulebook).then_some(OTHER_RULEBOOK)
}

/// 2. Every required row of the profile, from the bound attempt.
fn row_findings(
    profile: CoverageProfileId,
    checks: &ReviewChecks,
    version: &VersionId,
    findings: &mut Vec<ReviewFinding>,
) {
    for &row in profile.required_rows() {
        let note = match checks.rows.get(&row) {
            None => "This check has no recorded result.",
            Some(report) if checks.reviewer_attempt.as_ref() != Some(&report.attempt) => {
                "This check was recorded by an earlier reviewer attempt."
            }
            Some(report) => match (report.result, report.reason.as_deref()) {
                (ReviewRowResult::Pass, _) => continue,
                (_, Some(reason)) => reason,
                (ReviewRowResult::Unknown, None) => "This check could not determine a result.",
                (ReviewRowResult::Failed, None) => "This check failed.",
                (ReviewRowResult::Unrun, None) => "This check did not run.",
            },
        };
        findings.push(finding(version, Some(row), note));
    }
}

impl ProjectBundle {
    pub(super) fn review_descriptor(
        &self,
        snapshot: &SnapshotKey,
    ) -> Result<SnapshotDescriptor, BundleError> {
        self.version_index()?
            .snapshots
            .get(&snapshot.version)
            .filter(|descriptor| &descriptor.key == snapshot)
            .cloned()
            .ok_or_else(|| BundleError::Identity("review version missing".into()))
    }

    /// Every seal receipt kept on this version.
    pub fn review_receipts(
        &self,
        descriptor: &SnapshotDescriptor,
    ) -> Result<Vec<(ObjectRef, StorageReservation<ReviewReceipt>)>, BundleError> {
        let Some(reference) = &descriptor.review_evidence_ref else {
            return Ok(Vec::new());
        };
        let evidence = self.store.read_record::<ReviewEvidence>(reference)?;
        evidence
            .receipts()
            .map(|reference| {
                Ok((
                    reference.clone(),
                    self.store.read_record::<ReviewReceipt>(reference)?,
                ))
            })
            .collect()
    }

    /// A lost seal reply: the receipt this job generation already published.
    ///
    /// # Errors
    /// The root cannot be settled, or the version or its receipts cannot be read.
    pub fn sealed_review(
        &mut self,
        token: &ReviewToken,
    ) -> Result<Option<SealedReview>, BundleError> {
        self.settle_review_root()?;
        let descriptor = self.review_descriptor(&token.snapshot)?;
        Ok(self
            .review_receipts(&descriptor)?
            .into_iter()
            .find(|(_, receipt)| &receipt.token == token)
            .map(|(receipt_ref, receipt)| SealedReview {
                receipt_ref,
                receipt: receipt.take(),
                replayed: true,
            }))
    }

    /// Whether the reviewed version carries a complete editorial audit bound
    /// to it and to the rulebook the job froze; `Err` gives the reason it
    /// does not.
    ///
    /// # Errors
    /// The job, the version or its audit cannot be read.
    pub fn review_audit_complete(
        &self,
        token: &ReviewToken,
    ) -> Result<Result<(), &'static str>, BundleError> {
        let descriptor = self.review_descriptor(&token.snapshot)?;
        let job = self.require_review_job(token)?;
        let request = self.store.read_record::<ReviewRequest>(&job.request_ref)?;
        let Some(reference) = &descriptor.audit_ref else {
            return Ok(Err(NO_AUDIT));
        };
        let audit = self.store.read_record::<EditorialAudit>(reference)?;
        Ok(audit_binding(&descriptor, &audit, request.rulebook_ref.as_ref()).map_or(Ok(()), Err))
    }

    /// The Reviewer task attempt `attempt` on the cut `checkpoint`, with its
    /// state; `None` when it is not one.
    ///
    /// # Errors
    /// The run index or a run projection cannot be read.
    pub fn reviewer_attempt_on_cut(
        &self,
        checkpoint: &ObjectRef,
        attempt: &SessionId,
    ) -> Result<Option<AttemptState>, BundleError> {
        if let Some((task, state)) = self.reserved_task(attempt)? {
            return Ok(self.reviewer_task(&task, checkpoint)?.then_some(state));
        }
        #[cfg(all(test, target_os = "linux"))]
        if test_reviewers::contains(checkpoint, attempt) {
            return Ok(Some(AttemptState::Executing));
        }
        Ok(None)
    }

    /// The seal's check, from the orchestration records alone: the task of
    /// the live reservation or, once a reopen retired and pruned it, of the
    /// run's cumulative attempt archive must be a Reviewer task on the cut.
    fn reviewer_on_cut(
        &self,
        checkpoint: &ObjectRef,
        attempt: &SessionId,
    ) -> Result<bool, BundleError> {
        let task = match self.reserved_task(attempt)? {
            Some((task, _)) => Some(task),
            None => self.archived_task(attempt)?,
        };
        if let Some(task) = task {
            return self.reviewer_task(&task, checkpoint);
        }
        #[cfg(all(test, target_os = "linux"))]
        if test_reviewers::contains(checkpoint, attempt) {
            return Ok(true);
        }
        Ok(false)
    }

    fn reserved_task(
        &self,
        attempt: &SessionId,
    ) -> Result<Option<(TaskId, AttemptState)>, BundleError> {
        let runs: Vec<_> = self.run_index()?.orchestration.keys().cloned().collect();
        for run in runs {
            let Some(projection) = self.orchestration_projection(&run)? else {
                continue;
            };
            if let Some(reservation) = projection
                .attempts
                .iter()
                .find(|reservation| &reservation.identity.attempt.0 == attempt)
            {
                return Ok(Some((
                    reservation.identity.task.clone(),
                    reservation.state.clone(),
                )));
            }
        }
        Ok(None)
    }

    /// The task a retired, pruned attempt was reserved for. Each run keeps one
    /// cumulative archive, the last one in its usage records.
    fn archived_task(&self, attempt: &SessionId) -> Result<Option<TaskId>, BundleError> {
        let runs: Vec<_> = self.run_index()?.orchestration.keys().cloned().collect();
        for run in runs {
            let Some(projection) = self.orchestration_projection(&run)? else {
                continue;
            };
            for reference in projection.usage.iter().rev() {
                if let OrchestrationPayload::AttemptArchive(archive) =
                    self.store.read_run_record(reference)?.payload
                {
                    if let Some(reservation) = archive
                        .attempts
                        .iter()
                        .find(|reservation| &reservation.identity.attempt.0 == attempt)
                    {
                        return Ok(Some(reservation.identity.task.clone()));
                    }
                    break;
                }
            }
        }
        Ok(None)
    }

    fn reviewer_task(&self, task: &TaskId, checkpoint: &ObjectRef) -> Result<bool, BundleError> {
        let runs: Vec<_> = self.run_index()?.orchestration.keys().cloned().collect();
        for run in runs {
            let Some(projection) = self.orchestration_projection(&run)? else {
                continue;
            };
            if let Some(record) = projection.tasks.iter().find(|record| &record.id == task) {
                return Ok(record.brief.role == Role::Reviewer
                    && &record.brief.scope.base.checkpoint == checkpoint);
            }
        }
        Ok(false)
    }

    /// 1. The key: inputs, frozen annotations, bytes, configuration, attempt.
    #[allow(clippy::too_many_arguments)]
    fn key_findings(
        &self,
        token: &ReviewToken,
        job: &ReviewJob,
        checks: &ReviewChecks,
        proposed_key: Option<String>,
        expected_configuration_sha256: [u8; 32],
        verified: &VerifiedReviewArtifact,
        findings: &mut Vec<ReviewFinding>,
    ) -> Option<FrozenReviewKey> {
        let version = &token.snapshot.version;
        let key = match self.frozen_review_key(
            token,
            &job.request_ref,
            job.artifact_ref.as_ref(),
            checks,
        ) {
            Ok(key) => key,
            Err(error) => {
                findings.push(finding(version, None, &error.to_string()));
                return None;
            }
        };
        if proposed_key.is_some_and(|proposed| key.sha256().ok() != Some(proposed)) {
            findings.push(finding(
                version,
                None,
                "The reviewed inputs changed after every check passed.",
            ));
        }
        if key.configuration_sha256 != expected_configuration_sha256 {
            findings.push(finding(
                version,
                None,
                "The checker configuration differs from the bound configuration.",
            ));
        }
        if verified.artifact_ref != key.artifact_ref || verified.media_ref != key.media_ref {
            findings.push(finding(
                version,
                None,
                "The verified bytes are not the reviewed artifact.",
            ));
        }
        Some(key)
    }

    /// 4. The editorial audit, 5. reviewer independence, 7. blocking findings.
    fn audit_findings(
        &self,
        descriptor: &SnapshotDescriptor,
        request: &ReviewRequest,
        checks: &ReviewChecks,
        version: &VersionId,
        findings: &mut Vec<ReviewFinding>,
    ) -> Result<(), BundleError> {
        let audit_row = Some(RequiredReviewRow::EditorialAuditPresent);
        let visual_row = Some(RequiredReviewRow::IndependentVisualReview);
        // The visual row must come from a Reviewer task attempt on this cut,
        // whether or not the editor filed a rationale.
        let reviewer = match &checks.reviewer_attempt {
            Some(attempt) => self.reviewer_on_cut(&descriptor.checkpoint_ref, attempt)?,
            None => false,
        };
        if !reviewer {
            findings.push(finding(version, visual_row, NOT_A_REVIEWER));
        }
        match &descriptor.audit_ref {
            None => findings.push(finding(version, audit_row, NO_AUDIT)),
            Some(reference) => {
                let audit = self.store.read_record::<EditorialAudit>(reference)?;
                if let Some(note) = audit_binding(descriptor, &audit, request.rulebook_ref.as_ref())
                {
                    findings.push(finding(version, audit_row, note));
                }
                if audit.rationale.as_ref().is_some_and(|rationale| {
                    checks.reviewer_attempt.as_ref() == Some(&rationale.attempt.attempt.0)
                }) {
                    findings.push(finding(
                        version,
                        visual_row,
                        "The visual reviewer is the editor who made this cut.",
                    ));
                }
                for entry in audit.entries() {
                    if entry.disposition == AuditDisposition::Blocking {
                        findings.push(ReviewFinding {
                            version: version.clone(),
                            row: audit_row,
                            output: Some(entry.observation.output.clone()),
                            note: entry
                                .observation
                                .text
                                .chars()
                                .take(MAX_REVIEW_FINDING_NOTE / 4)
                                .collect(),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// A second seal of an already sealed version and profile retires the job
    /// and returns the existing receipt; a receipt is never replaced.
    fn retire_sealed_profile(
        &mut self,
        token: &ReviewToken,
        descriptor: &SnapshotDescriptor,
        profile: CoverageProfileId,
    ) -> Result<Option<SealedReview>, BundleError> {
        let Some((receipt_ref, receipt)) = self
            .review_receipts(descriptor)?
            .into_iter()
            .find(|(_, receipt)| receipt.key.profile == profile)
        else {
            return Ok(None);
        };
        self.publish_review_metadata(ValidatedMetadataUpdate {
            review: Some(PendingReviewUpdate::Sealed(token.clone())),
            ..Default::default()
        })?;
        Ok(Some(SealedReview {
            receipt_ref,
            receipt: receipt.take(),
            replayed: true,
        }))
    }

    pub(super) fn sealed_profile(
        &self,
        descriptor: &SnapshotDescriptor,
        profile: CoverageProfileId,
    ) -> Result<bool, BundleError> {
        Ok(self
            .review_receipts(descriptor)?
            .iter()
            .any(|(_, receipt)| receipt.key.profile == profile))
    }

    /// The key a complete set of rows is proposed for; also rebuilt at seal.
    pub(super) fn frozen_review_key(
        &self,
        token: &ReviewToken,
        request_ref: &ObjectRef,
        artifact_ref: Option<&ObjectRef>,
        checks: &ReviewChecks,
    ) -> Result<FrozenReviewKey, BundleError> {
        let unbound = || BundleError::Identity("review check binding missing".into());
        let artifact_ref = artifact_ref.ok_or_else(unbound)?;
        let request = self.store.read_record::<ReviewRequest>(request_ref)?;
        let artifact = self.store.read_record::<ReviewArtifact>(artifact_ref)?;
        let descriptor = self.review_descriptor(&token.snapshot)?;
        request.validate_snapshot(&descriptor)?;
        if artifact.snapshot != token.snapshot
            || artifact.job != token.job
            || artifact.generation != token.generation
            || &artifact.request_ref != request_ref
        {
            return Err(BundleError::Identity("review artifact generation".into()));
        }
        let request = request.take();
        let artifact = artifact.take();
        Ok(FrozenReviewKey {
            snapshot: token.snapshot.clone(),
            identity: request.identity,
            request_ref: request_ref.clone(),
            document_ref: request.document_ref,
            input_manifest_ref: request.input_manifest_ref,
            expectations_ref: request.expectations_ref,
            annotations_ref: request.annotations_ref,
            rulebook_ref: request.rulebook_ref,
            audit_ref: descriptor.audit_ref,
            settings: request.settings,
            profile: request.profile,
            artifact_ref: artifact_ref.clone(),
            media_ref: artifact.media_ref,
            output: artifact.output,
            configuration_sha256: checks.configuration_sha256.ok_or_else(unbound)?,
            reviewer_attempt: checks.reviewer_attempt.clone().ok_or_else(unbound)?,
        })
    }

    /// Host-only. Seal the active job's frozen version, or resolve a retry.
    ///
    /// `expected_configuration_sha256` is the checker configuration the host
    /// would bind now; `verified` proves the reviewed bytes were hashed.
    pub fn seal_review(
        &mut self,
        token: &ReviewToken,
        command: &SessionId,
        expected_configuration_sha256: [u8; 32],
        verified: &VerifiedReviewArtifact,
    ) -> Result<SealResult, BundleError> {
        if let Some(sealed) = self.sealed_review(token)? {
            return Ok(SealResult::Sealed(Box::new(sealed)));
        }
        let descriptor = self.review_descriptor(&token.snapshot)?;
        let mut job = self.require_review_job(token)?;
        let request = self.store.read_record::<ReviewRequest>(&job.request_ref)?;
        request.validate_snapshot(&descriptor)?;
        if let Some(sealed) = self.retire_sealed_profile(token, &descriptor, request.profile)? {
            return Ok(SealResult::Sealed(Box::new(sealed)));
        }
        let (checks, proposed_key) = match &job.phase {
            ReviewPhase::Checking(checks) => (checks.clone(), None),
            ReviewPhase::ReadyToSeal {
                checks,
                proposed_key,
            } => (checks.clone(), Some(proposed_key.clone())),
            _ => {
                return Err(BundleError::Identity(
                    "the output review is not ready to seal".into(),
                ));
            }
        };
        let version = &token.snapshot.version;
        let mut findings = Vec::new();
        let key = self.key_findings(
            token,
            &job,
            &checks,
            proposed_key,
            expected_configuration_sha256,
            verified,
            &mut findings,
        );
        if descriptor
            .review_annotations_ref
            .as_ref()
            .is_some_and(|current| current != &request.annotations_ref)
        {
            findings.push(finding(
                version,
                None,
                "The version's annotations changed after the review froze them.",
            ));
        }
        row_findings(request.profile, &checks, version, &mut findings);
        // 3. Eligibility, from the frozen expectations.
        let expectations = self
            .store
            .read_record::<StoredReviewExpectations>(&request.expectations_ref)?;
        if expectations.0.identity != request.identity {
            findings.push(finding(
                version,
                None,
                "The frozen expectations are not this cut's.",
            ));
        }
        let exceptions: Vec<DeliveryException> = delivery_eligibility(&expectations.0);
        self.audit_findings(&descriptor, &request, &checks, version, &mut findings)?;
        // 6. Cleanup: staging is reclaimed before the version is sealed.
        if job.stage.is_some() {
            findings.push(finding(
                version,
                None,
                "Review staging is still held by the encode.",
            ));
        }
        let Some(key) = key.filter(|_| findings.is_empty()) else {
            findings.truncate(MAX_REVIEW_FINDINGS);
            let _budget = TerminalBudget::enter(&self.store);
            job.phase = ReviewPhase::Paused {
                checkpoint: ReviewCheckpoint::Checking(checks),
                reason: "The output review could not be sealed.".into(),
                findings,
            };
            self.publish_review_job(&job, Vec::new(), None)?;
            return Ok(SealResult::Paused(Box::new(status(job))));
        };
        let outcome = if exceptions.is_empty() {
            SealedReviewOutcome::Delivered
        } else {
            SealedReviewOutcome::ReviewedFactsNotChecked { exceptions }
        };
        let receipt = ReviewReceipt {
            token: token.clone(),
            command: command.clone(),
            key_sha256: key.sha256()?,
            rows: request
                .profile
                .required_rows()
                .iter()
                .map(|&row| (row, ReviewRowResult::Pass))
                .collect(),
            key,
            outcome,
        };
        self.publish_review_receipt(token, receipt)
    }

    fn publish_review_receipt(
        &mut self,
        token: &ReviewToken,
        receipt: ReviewReceipt,
    ) -> Result<SealResult, BundleError> {
        #[cfg(any(test, feature = "test-util"))]
        crate::io_observer::check_failure(
            "review_seal_before_admission",
            &self.store.resolved_root()?,
        )?;
        let mut versions = self.version_index()?.take();
        let descriptor = versions
            .snapshots
            .get_mut(&token.snapshot.version)
            .filter(|descriptor| descriptor.key == token.snapshot)
            .ok_or_else(|| BundleError::Identity("review version missing".into()))?;
        let mut evidence = descriptor
            .review_evidence_ref
            .as_ref()
            .map(|reference| self.store.read_record::<ReviewEvidence>(reference))
            .transpose()?
            .map(StorageReservation::take)
            .ok_or_else(|| BundleError::Identity("review evidence missing".into()))?;
        if evidence.receipts.len() >= MAX_VERSION_RECEIPTS {
            return Err(BundleError::Capacity("review receipts per version"));
        }
        let sealed = self.store.admit_record(&receipt)?;
        let receipt_ref = sealed.reference.clone();
        evidence.receipts.push(receipt_ref.clone());
        let evidence = self.store.admit_record(&evidence)?;
        descriptor.review_evidence_ref = Some(evidence.reference.clone());
        let versions = self.store.admit_record(&versions)?;
        // One root: the receipt, the version entry and the job's retirement.
        self.publish_review_metadata(ValidatedMetadataUpdate {
            review: Some(PendingReviewUpdate::Sealed(token.clone())),
            versions: Some(versions.reference.clone()),
            seals: vec![sealed, evidence, versions],
            ..Default::default()
        })?;
        Ok(SealResult::Sealed(Box::new(SealedReview {
            receipt_ref,
            receipt,
            replayed: false,
        })))
    }
}

/// Recovery: a kept receipt matches its version, artifact, request and the
/// outcome its frozen expectations allow, once per profile.
/// Settings as they persist. `ExportSettings` equality includes its live
/// cancellation handle, which no record keeps.
fn same_settings(left: &ExportSettings, right: &ExportSettings) -> bool {
    matches!(
        (serde_json::to_value(left), serde_json::to_value(right)),
        (Ok(left), Ok(right)) if left == right
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn validate_version_receipt(
    descriptor: &SnapshotDescriptor,
    evidence: &ReviewEvidence,
    receipt: &ReviewReceipt,
    artifact: &ReviewArtifact,
    request: &ReviewRequest,
    expectations: &kinewright_core::output_review::ReviewExpectations,
    audit: &EditorialAudit,
    profiles: &mut Vec<CoverageProfileId>,
) -> Result<(), BundleError> {
    let key = &receipt.key;
    // The sealed audit: this version's, under the frozen rulebook, with no
    // blocking entry, and not filed by the visual reviewer as its editor.
    if audit.snapshot() != &key.snapshot
        || audit.rulebook_ref() != key.rulebook_ref.as_ref()
        || audit
            .entries()
            .any(|entry| entry.disposition == AuditDisposition::Blocking)
        || audit
            .rationale
            .as_ref()
            .is_some_and(|rationale| rationale.attempt.attempt.0 == key.reviewer_attempt)
    {
        return Err(BundleError::Corrupt("review receipt audit binding".into()));
    }
    let exceptions = delivery_eligibility(expectations);
    let outcome = if exceptions.is_empty() {
        SealedReviewOutcome::Delivered
    } else {
        SealedReviewOutcome::ReviewedFactsNotChecked { exceptions }
    };
    if key.snapshot != descriptor.key
        || profiles.contains(&key.profile)
        || !evidence
            .artifacts()
            .any(|artifact| artifact == &key.artifact_ref)
        || artifact.media_ref != key.media_ref
        || artifact.output != key.output
        || artifact.request_ref != key.request_ref
        || artifact.snapshot != key.snapshot
        || artifact.job != receipt.token.job
        || artifact.generation != receipt.token.generation
        || request.snapshot != key.snapshot
        || request.identity != key.identity
        || expectations.identity != request.identity
        || request.document_ref != key.document_ref
        || request.input_manifest_ref != key.input_manifest_ref
        || request.expectations_ref != key.expectations_ref
        || request.annotations_ref != key.annotations_ref
        || request.rulebook_ref != key.rulebook_ref
        || !same_settings(&request.settings, &key.settings)
        || request.profile != key.profile
        || receipt.outcome != outcome
    {
        return Err(BundleError::Corrupt(
            "review receipt version binding".into(),
        ));
    }
    profiles.push(key.profile);
    Ok(())
}

/// Project unit tests stand in for the orchestration records that make an
/// attempt a Reviewer of a cut; the host's tests use the real records.
/// Gated like the review job tests that admit to it.
#[cfg(all(test, target_os = "linux"))]
pub(crate) mod test_reviewers {
    use std::cell::RefCell;

    use crate::bundle_records::{ObjectRef, SessionId};

    thread_local! {
        static REVIEWERS: RefCell<Vec<(ObjectRef, SessionId)>> = const { RefCell::new(Vec::new()) };
    }

    pub(crate) fn admit(checkpoint: &ObjectRef, attempt: &SessionId) {
        REVIEWERS.with(|reviewers| {
            reviewers
                .borrow_mut()
                .push((checkpoint.clone(), attempt.clone()));
        });
    }

    pub(super) fn contains(checkpoint: &ObjectRef, attempt: &SessionId) -> bool {
        REVIEWERS.with(|reviewers| {
            reviewers
                .borrow()
                .iter()
                .any(|(cut, reviewer)| cut == checkpoint && reviewer == attempt)
        })
    }
}
