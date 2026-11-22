//! Durable output review records and their Studio root transaction.
#![allow(clippy::missing_errors_doc)]

use std::{cell::Cell, collections::BTreeMap};
#[cfg(target_os = "linux")]
mod staging;
#[cfg(target_os = "linux")]
pub(crate) use staging::LiveStageSlot;
#[cfg(target_os = "linux")]
pub use staging::{ReviewStageCharge, ReviewStaging};
mod check_inputs;
pub use check_inputs::{PendingReviewCheck, ReviewCheckInputs};
mod seal;
pub(crate) use seal::validate_version_receipt;
pub use seal::{
    FrozenReviewKey, MAX_REVIEW_FINDING_NOTE, MAX_REVIEW_FINDINGS, ReviewFinding, ReviewOutcome,
    ReviewReceipt, SealResult, SealedReview, VerifiedReviewArtifact,
};

use kinewright_core::{
    ExportSettings,
    output_review::{
        CoverageProfileId, RequiredReviewRow, ReviewAnnotations, ReviewCutIdentity,
        ReviewExpectations, ReviewRowId, ReviewRowResult, ReviewSnapshotId, check_review_coverage,
    },
};
use serde::{Deserialize, Serialize};

use crate::{
    bundle::{BundleError, ProjectBundle},
    bundle_commit::BundleCommit,
    bundle_records::{ObjectRef, ProjectId, RecordKind, SessionId},
    editing_session::Ja1EditingSession,
    storage_budget::{StorageLimits, StorageReservation},
    store::{BundleRecord, ProjectStore, SealedObject, record_sealed},
    studio_records::{SnapshotDescriptor, SnapshotKey, bounded, kind},
    studio_store::{PendingReviewUpdate, ValidatedMetadataUpdate, record},
};

// Keep space for both revocation and the subsequent retirement root. Ordinary
// writes, including edits and imports, share this ceiling while a job is live.
pub(crate) const OBJECT_HEADROOM: u64 = 24;
pub(crate) const METADATA_HEADROOM: u64 = 96 << 20;
pub(crate) const DISK_HEADROOM: u64 = 32 << 20;
thread_local! { static TERMINAL_STORE: Cell<Option<usize>> = const { Cell::new(None) }; }

pub(crate) fn using_terminal_budget(store: &ProjectStore) -> bool {
    TERMINAL_STORE.get() == Some(store.admission_key())
}

struct TerminalBudget(Option<usize>);

impl TerminalBudget {
    fn enter(store: &ProjectStore) -> Self {
        Self(TERMINAL_STORE.replace(Some(store.admission_key())))
    }
}

impl Drop for TerminalBudget {
    fn drop(&mut self) {
        TERMINAL_STORE.set(self.0);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewToken {
    pub project: ProjectId,
    pub job: SessionId,
    pub generation: u64,
    pub snapshot: SnapshotKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewStage {
    pub id: SessionId,
    pub boot: String,
    pub pid: u32,
    pub process_start: u64,
    pub byte_limit: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewRequest {
    pub snapshot: SnapshotKey,
    pub identity: ReviewCutIdentity,
    pub document_ref: ObjectRef,
    pub input_manifest_ref: ObjectRef,
    pub expectations_ref: ObjectRef,
    pub annotations_ref: ObjectRef,
    pub rulebook_ref: Option<ObjectRef>,
    pub settings: ExportSettings,
    pub profile: CoverageProfileId,
    pub byte_limit: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewArtifact {
    pub snapshot: SnapshotKey,
    pub job: SessionId,
    pub request_ref: ObjectRef,
    pub generation: u64,
    pub media_ref: ObjectRef,
    pub output: kinewright_media::ReviewOutputTiming,
}

const MAX_VERSION_ARTIFACTS: usize = 16;

/// Version-owned review evidence: every published artifact and, at most once
/// per coverage profile, the immutable seal receipt.
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewEvidence {
    artifacts: Vec<ObjectRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    receipts: Vec<ObjectRef>,
}

impl ReviewEvidence {
    pub fn artifacts(&self) -> impl DoubleEndedIterator<Item = &ObjectRef> {
        self.artifacts.iter()
    }

    pub fn receipts(&self) -> impl DoubleEndedIterator<Item = &ObjectRef> {
        self.receipts.iter()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewCheckBinding {
    pub configuration_sha256: [u8; 32],
    pub reviewer_attempt: SessionId,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewChecks {
    pub configuration_sha256: Option<[u8; 32]>,
    pub reviewer_attempt: Option<SessionId>,
    pub rows: BTreeMap<RequiredReviewRow, ReportedReviewCheck>,
}

impl ReviewChecks {
    fn bind(&mut self, binding: ReviewCheckBinding) -> Result<(), BundleError> {
        if self
            .configuration_sha256
            .is_some_and(|previous| previous != binding.configuration_sha256)
            || self
                .reviewer_attempt
                .as_ref()
                .is_some_and(|previous| previous != &binding.reviewer_attempt)
        {
            return Err(BundleError::Identity(
                "review check binding already established".into(),
            ));
        }
        self.configuration_sha256 = Some(binding.configuration_sha256);
        self.reviewer_attempt = Some(binding.reviewer_attempt);
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportedReviewCheck {
    pub attempt: SessionId,
    pub result: ReviewRowResult,
    /// Why a row is not Pass, when its checker could say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewCheckpoint {
    Queued,
    Checking(ReviewChecks),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewPhase {
    Queued,
    Encoding,
    Checking(ReviewChecks),
    ReadyToSeal {
        checks: ReviewChecks,
        proposed_key: String,
    },
    Paused {
        checkpoint: ReviewCheckpoint,
        reason: String,
        /// Why a seal was refused, linked to the version and output time.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        findings: Vec<ReviewFinding>,
    },
    Stopping,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewJob {
    pub token: ReviewToken,
    pub request_ref: ObjectRef,
    pub stage: Option<ReviewStage>,
    pub artifact_ref: Option<ObjectRef>,
    pub phase: ReviewPhase,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewStatus {
    pub token: ReviewToken,
    pub phase: ReviewPhase,
    pub stop_pending: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReviewControl {
    Status,
    Annotate {
        snapshot: SnapshotKey,
        fields: ReviewAnnotationFields,
    },
    QueueDefault(SnapshotKey),
    Queue {
        snapshot: SnapshotKey,
        settings: ExportSettings,
        byte_limit: u64,
    },
    Resume(ReviewToken),
    Stop(ReviewToken),
    /// Run every check of the published artifact, with `reviewer_attempt`
    /// as the visual reviewer, then seal under `command`.
    Check {
        token: ReviewToken,
        reviewer_attempt: SessionId,
        command: SessionId,
    },
    /// Seal checked rows again, or return the receipt a lost reply published.
    Seal {
        token: ReviewToken,
        command: SessionId,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewAnnotationFields {
    pub sourced_elements: Vec<kinewright_core::ClipId>,
    pub text_cards: Vec<kinewright_core::ClipId>,
    pub intended_pauses: Vec<std::ops::Range<kinewright_core::TimeCode>>,
}

#[derive(Serialize, Deserialize)]
#[serde(transparent)]
pub struct StoredReviewAnnotations(pub ReviewAnnotations);

#[derive(Serialize, Deserialize)]
#[serde(transparent)]
pub struct StoredReviewExpectations(pub ReviewExpectations);

record!(
    StoredReviewAnnotations,
    ReviewAnnotations,
    |_: &StoredReviewAnnotations| vec![],
    |v: &StoredReviewAnnotations, _: &StorageLimits| {
        use kinewright_core::output_review::{MAX_REVIEW_ELEMENTS, MAX_REVIEW_PAUSES};
        if v.0.sourced_elements.len() > MAX_REVIEW_ELEMENTS
            || v.0.text_cards.len() > MAX_REVIEW_ELEMENTS
            || v.0.intended_pauses.len() > MAX_REVIEW_PAUSES
            || v.0.intended_pauses.iter().any(std::ops::Range::is_empty)
        {
            return Err(BundleError::Corrupt("review annotation bounds".into()));
        }
        Ok(())
    }
);
record!(
    StoredReviewExpectations,
    ReviewExpectations,
    |_: &StoredReviewExpectations| vec![],
    |_: &StoredReviewExpectations, _: &StorageLimits| Ok::<(), BundleError>(())
);
record!(
    ReviewRequest,
    ReviewRequest,
    |v: &ReviewRequest| [
        Some(&v.document_ref),
        Some(&v.input_manifest_ref),
        Some(&v.expectations_ref),
        Some(&v.annotations_ref),
        v.rulebook_ref.as_ref(),
    ]
    .into_iter()
    .flatten()
    .cloned()
    .collect(),
    |v: &ReviewRequest, l: &StorageLimits| {
        kind(&v.document_ref, RecordKind::Document)?;
        kind(&v.input_manifest_ref, RecordKind::InputManifest)?;
        kind(&v.expectations_ref, RecordKind::ReviewExpectations)?;
        kind(&v.annotations_ref, RecordKind::ReviewAnnotations)?;
        if let Some(rules) = &v.rulebook_ref {
            kind(rules, RecordKind::RulebookRevision)?;
        }
        if v.byte_limit == 0 || v.byte_limit > l.owned_bytes {
            return Err(BundleError::Capacity("review staging byte limit"));
        }
        Ok(())
    }
);
record!(
    ReviewArtifact,
    ReviewArtifact,
    |v: &ReviewArtifact| vec![v.request_ref.clone(), v.media_ref.clone()],
    |v: &ReviewArtifact, _: &StorageLimits| {
        kind(&v.request_ref, RecordKind::ReviewRequest)?;
        kind(&v.media_ref, RecordKind::OwnedMedia)?;
        if v.generation == 0 || v.media_ref.byte_length == 0 || v.output.video_frames == 0 {
            return Err(BundleError::Corrupt("review artifact identity".into()));
        }
        Ok(())
    }
);
record!(
    ReviewEvidence,
    ReviewEvidence,
    |v: &ReviewEvidence| v.artifacts.iter().chain(&v.receipts).cloned().collect(),
    |v: &ReviewEvidence, _: &StorageLimits| {
        if v.artifacts.is_empty() || v.artifacts.len() > MAX_VERSION_ARTIFACTS {
            return Err(BundleError::Capacity("published artifacts per version"));
        }
        if v.receipts.len() > seal::MAX_VERSION_RECEIPTS {
            return Err(BundleError::Capacity("review receipts per version"));
        }
        for reference in &v.artifacts {
            kind(reference, RecordKind::ReviewArtifact)?;
        }
        for reference in &v.receipts {
            kind(reference, RecordKind::ReviewReceipt)?;
        }
        Ok(())
    }
);
record!(
    ReviewJob,
    ReviewJob,
    |v: &ReviewJob| std::iter::once(v.request_ref.clone())
        .chain(v.artifact_ref.clone())
        .collect(),
    |v: &ReviewJob, limits: &StorageLimits| {
        kind(&v.request_ref, RecordKind::ReviewRequest)?;
        if let Some(reference) = &v.artifact_ref {
            kind(reference, RecordKind::ReviewArtifact)?;
        }
        if let Some(stage) = &v.stage
            && (stage.boot.len() != 36
                || stage.pid == 0
                || stage.process_start == 0
                || stage.byte_limit == 0
                || stage.byte_limit > limits.owned_bytes
                || v.token.generation == 0)
        {
            return Err(BundleError::Corrupt("review stage ownership".into()));
        }
        let has_artifact = v.artifact_ref.is_some();
        let valid = match &v.phase {
            ReviewPhase::Queued => v.stage.is_none() && !has_artifact,
            ReviewPhase::Encoding => v.stage.is_some() && !has_artifact,
            ReviewPhase::Checking(_) | ReviewPhase::ReadyToSeal { .. } => has_artifact,
            ReviewPhase::Paused {
                checkpoint,
                reason,
                findings,
            } => {
                !reason.is_empty()
                    && reason.len() <= 1024
                    && ReviewFinding::valid(findings)
                    && findings
                        .iter()
                        .all(|finding| finding.version == v.token.snapshot.version)
                    && has_artifact == matches!(checkpoint, ReviewCheckpoint::Checking(_))
            }
            ReviewPhase::Stopping => true,
            ReviewPhase::Stopped => false,
        };
        if !valid {
            return Err(BundleError::Corrupt("review phase dependencies".into()));
        }
        let checks = match &v.phase {
            ReviewPhase::Checking(checks)
            | ReviewPhase::ReadyToSeal { checks, .. }
            | ReviewPhase::Paused {
                checkpoint: ReviewCheckpoint::Checking(checks),
                ..
            } => Some(checks),
            _ => None,
        };
        if checks.is_some_and(|checks| {
            checks.rows.values().any(|row| {
                row.reason.as_deref().is_some_and(|reason| {
                    row.result == ReviewRowResult::Pass
                        || !crate::editorial::valid_text(reason, seal::MAX_REVIEW_FINDING_NOTE)
                })
            })
        }) {
            return Err(BundleError::Corrupt("review check reason".into()));
        }
        Ok(())
    }
);

impl ReviewRequest {
    pub(crate) fn validate_snapshot(
        &self,
        snapshot: &SnapshotDescriptor,
    ) -> Result<(), BundleError> {
        if self.snapshot != snapshot.key
            || self.document_ref != snapshot.document_ref
            || self.input_manifest_ref != snapshot.input_manifest_ref
            || self.identity.snapshot != ReviewSnapshotId(snapshot.identity.digest())
            || hex_digest(&self.identity.input_manifest_sha256) != self.input_manifest_ref.sha256
            || encode(&self.settings).map(|bytes| kinewright_media::sha256_bytes(&bytes))?
                != hex_digest(&self.identity.export_settings_sha256)
        {
            return Err(BundleError::Identity(
                "frozen review request binding".into(),
            ));
        }
        Ok(())
    }
}

fn encode(value: &impl Serialize) -> Result<Vec<u8>, BundleError> {
    serde_json::to_vec(value).map_err(|error| BundleError::Corrupt(error.to_string()))
}

fn hex_digest(digest: &[u8; 32]) -> String {
    use std::fmt::Write as _;
    let mut hex = String::with_capacity(64);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[derive(Debug)]
pub enum UpdateReviewJob {
    BeginEncoding(ReviewStage),
    RecoverEncoding,
    RetireStage,
    Pause(String),
    Retry,
    Stop,
    RetireStopped,
    BindChecks(ReviewCheckBinding),
    UnbindChecks,
    RecordCheck {
        binding: ReviewCheckBinding,
        row: RequiredReviewRow,
        result: ReviewRowResult,
    },
    /// A check that errored: the row is Unknown and keeps the reason.
    RecordUnknown {
        binding: ReviewCheckBinding,
        row: RequiredReviewRow,
        reason: String,
    },
}

impl ProjectBundle {
    pub fn review_job(&self) -> Result<Option<StorageReservation<ReviewJob>>, BundleError> {
        self.root
            .as_ref()
            .and_then(|root| root.active_review_ref.as_ref())
            .map(|reference| self.store.read_record(reference))
            .transpose()
    }

    /// Read the status with the revocation headroom, settling first.
    pub fn review_status_for_stop(&mut self) -> Result<Option<ReviewStatus>, BundleError> {
        let _budget = TerminalBudget::enter(&self.store);
        self.settle_review_root()?;
        self.review_status()
    }

    /// A settle would write first: the root shows a job its owner may still
    /// roll back. A failed object query counts as unsettled.
    #[must_use]
    pub fn review_root_unsettled(&self) -> bool {
        self.unsettled
            || self.pending_metadata.is_some()
            || self.store.has_unsettled_objects().unwrap_or(true)
    }

    pub fn review_status(&self) -> Result<Option<ReviewStatus>, BundleError> {
        if let Some(job) = self.review_job()? {
            return Ok(Some(ReviewStatus {
                token: job.token.clone(),
                phase: job.phase.clone(),
                stop_pending: false,
            }));
        }
        Ok(self.root.as_ref().and_then(|root| {
            root.stopped_review.clone().map(|token| ReviewStatus {
                token,
                phase: ReviewPhase::Stopped,
                stop_pending: false,
            })
        }))
    }

    pub fn review_annotations(
        &self,
        snapshot: &SnapshotDescriptor,
    ) -> Result<Option<StorageReservation<StoredReviewAnnotations>>, BundleError> {
        snapshot
            .review_annotations_ref
            .as_ref()
            .map(|reference| self.store.read_record(reference))
            .transpose()
    }

    pub fn set_review_annotations(
        &mut self,
        snapshot: &SnapshotKey,
        annotations: ReviewAnnotations,
    ) -> Result<(), BundleError> {
        self.settle_review_root()?;
        if self
            .review_job()?
            .is_some_and(|job| &job.token.snapshot == snapshot)
        {
            return Err(BundleError::Identity(
                "The output review of this version must finish or stop before its annotations change."
                    .into(),
            ));
        }
        self.write_review_annotations(snapshot, annotations)
    }

    /// Test seam: change a frozen version's annotations behind an active job.
    #[cfg(any(test, feature = "test-util"))]
    pub fn set_review_annotations_unfenced_for_test(
        &mut self,
        snapshot: &SnapshotKey,
        annotations: ReviewAnnotations,
    ) -> Result<(), BundleError> {
        self.settle_review_root()?;
        self.write_review_annotations(snapshot, annotations)
    }

    fn write_review_annotations(
        &mut self,
        snapshot: &SnapshotKey,
        annotations: ReviewAnnotations,
    ) -> Result<(), BundleError> {
        let mut versions = self.version_index()?.take();
        let descriptor = versions
            .snapshots
            .get_mut(&snapshot.version)
            .filter(|d| &d.key == snapshot)
            .ok_or_else(|| BundleError::Identity("review annotation version".into()))?;
        if annotations.snapshot != ReviewSnapshotId(descriptor.identity.digest()) {
            return Err(BundleError::Identity("review annotation cut".into()));
        }
        let annotations = self
            .store
            .admit_record(&StoredReviewAnnotations(annotations))?;
        descriptor.review_annotations_ref = Some(annotations.reference.clone());
        let versions = self.store.admit_record(&versions)?;
        self.publish_review_metadata(ValidatedMetadataUpdate {
            versions: Some(versions.reference.clone()),
            seals: vec![annotations, versions],
            ..Default::default()
        })
    }

    pub fn queue_review(
        &mut self,
        frozen: &SnapshotDescriptor,
        rulebook_ref: Option<ObjectRef>,
        settings: ExportSettings,
        byte_limit: u64,
        expectations: ReviewExpectations,
    ) -> Result<ReviewToken, BundleError> {
        self.settle_review_root()?;
        if self.review_job()?.is_some() {
            return Err(BundleError::Identity(
                "an output review is already active".into(),
            ));
        }
        self.store.set_review_budget(true);
        let result =
            self.queue_review_reserved(frozen, rulebook_ref, settings, byte_limit, expectations);
        if !self.unsettled
            && self
                .root
                .as_ref()
                .is_none_or(|root| root.active_review_ref.is_none())
        {
            self.store.set_review_budget(false);
        }
        result
    }

    fn queue_review_reserved(
        &mut self,
        frozen: &SnapshotDescriptor,
        rulebook_ref: Option<ObjectRef>,
        settings: ExportSettings,
        byte_limit: u64,
        expectations: ReviewExpectations,
    ) -> Result<ReviewToken, BundleError> {
        let _space = self.store.reserve(0, 0, false)?;
        let current = self.read_snapshot(&frozen.key)?;
        if self.sealed_profile(&current.descriptor, CoverageProfileId::OutputReviewV1)? {
            return Err(BundleError::Identity(
                "This version already has a sealed output review.".into(),
            ));
        }
        if current.descriptor.identity != frozen.identity
            || current.descriptor.review_annotations_ref != frozen.review_annotations_ref
            || self.approved_rulebook() != rulebook_ref.as_ref()
        {
            return Err(BundleError::Identity(
                "review inputs changed during preparation".into(),
            ));
        }
        let (annotations_ref, mut seals) = if let Some(reference) = &frozen.review_annotations_ref {
            (reference.clone(), Vec::new())
        } else {
            let empty = self
                .store
                .admit_record(&StoredReviewAnnotations(empty_annotations(frozen)))?;
            (empty.reference.clone(), vec![empty])
        };
        let identity = expectations.identity;
        let expectations = self
            .store
            .admit_record(&StoredReviewExpectations(expectations))?;
        let request = ReviewRequest {
            snapshot: frozen.key.clone(),
            identity,
            document_ref: frozen.document_ref.clone(),
            input_manifest_ref: frozen.input_manifest_ref.clone(),
            expectations_ref: expectations.reference.clone(),
            annotations_ref,
            rulebook_ref,
            settings,
            profile: CoverageProfileId::OutputReviewV1,
            byte_limit,
        };
        request.validate_snapshot(&current.descriptor)?;
        let request = self.store.admit_record(&request)?;
        let job = ReviewJob {
            token: ReviewToken {
                project: self.project_id().clone(),
                job: SessionId::fresh()?,
                generation: 0,
                snapshot: frozen.key.clone(),
            },
            request_ref: request.reference.clone(),
            stage: None,
            artifact_ref: None,
            phase: ReviewPhase::Queued,
        };
        seals.extend([expectations, request]);
        self.publish_review_job(&job, seals, None)?;
        Ok(job.token)
    }

    pub fn update_review_job(
        &mut self,
        token: &ReviewToken,
        update: UpdateReviewJob,
    ) -> Result<ReviewStatus, BundleError> {
        use ReviewPhase::{Checking, Encoding, Paused, Queued, Stopping};
        let terminal = matches!(
            update,
            UpdateReviewJob::Stop
                | UpdateReviewJob::RetireStopped
                | UpdateReviewJob::RetireStage
                | UpdateReviewJob::RecoverEncoding
                | UpdateReviewJob::Pause(_)
        );
        let _budget = terminal.then(|| TerminalBudget::enter(&self.store));
        self.settle_review_root()?;
        if matches!(update, UpdateReviewJob::Stop)
            && let Some(status) = self.review_status()?
            && &status.token == token
            && matches!(status.phase, ReviewPhase::Stopped | ReviewPhase::Stopping)
        {
            return Ok(status);
        }
        let mut job = self.require_review_job(token)?;
        #[cfg(any(test, feature = "test-util"))]
        if matches!(update, UpdateReviewJob::Stop) {
            crate::io_observer::check_failure(
                "review_stop_before_admission",
                &self.store.resolved_root()?,
            )?;
        }
        let invalid = || BundleError::Identity("review transition or check binding".into());
        match update {
            UpdateReviewJob::BeginEncoding(stage) if job.phase == Queued => {
                self.begin_review_encoding(&mut job, stage)?;
            }
            UpdateReviewJob::RecoverEncoding if job.phase == Encoding => {
                job.stage = None;
                job.phase = Queued;
            }
            UpdateReviewJob::RetireStage if job.stage.is_some() && job.phase != Encoding => {
                job.stage = None;
            }
            UpdateReviewJob::Pause(reason) => {
                // A failed seal pauses like a refused one: Retry binds afresh.
                let checkpoint = match job.phase {
                    Encoding => ReviewCheckpoint::Queued,
                    Checking(checks) | ReviewPhase::ReadyToSeal { checks, .. } => {
                        ReviewCheckpoint::Checking(checks)
                    }
                    _ => return Err(invalid()),
                };
                let reason = reason.chars().take(256).collect::<String>();
                job.phase = Paused {
                    checkpoint,
                    reason,
                    findings: Vec::new(),
                };
            }
            UpdateReviewJob::Retry if job.stage.is_none() => {
                let Paused { checkpoint, .. } = job.phase else {
                    return Err(invalid());
                };
                job.phase = match checkpoint {
                    ReviewCheckpoint::Queued => Queued,
                    // A retry binds afresh: the digest covers the attempt,
                    // and no row of an earlier binding counts.
                    ReviewCheckpoint::Checking(_) => Checking(ReviewChecks::default()),
                };
            }
            UpdateReviewJob::Stop => job.phase = Stopping,
            UpdateReviewJob::RetireStopped if job.phase == Stopping => {
                self.publish_review_metadata(ValidatedMetadataUpdate {
                    review: Some(PendingReviewUpdate::Retired(job.token.clone())),
                    ..Default::default()
                })?;
                job.phase = ReviewPhase::Stopped;
                return Ok(status(job));
            }
            UpdateReviewJob::UnbindChecks => {
                let Checking(checks) = &mut job.phase else {
                    return Err(invalid());
                };
                // The digest covers the attempt, so the next attempt binds
                // afresh, and no row of the earlier binding counts.
                *checks = ReviewChecks::default();
            }
            UpdateReviewJob::BindChecks(binding) => {
                let Checking(checks) = &mut job.phase else {
                    return Err(invalid());
                };
                checks.bind(binding)?;
            }
            UpdateReviewJob::RecordCheck { .. } | UpdateReviewJob::RecordUnknown { .. } => {
                self.record_review_check(&mut job, update)?;
            }
            _ => return Err(invalid()),
        }
        self.publish_review_job(&job, Vec::new(), None)?;
        Ok(status(job))
    }

    fn begin_review_encoding(
        &self,
        job: &mut ReviewJob,
        stage: ReviewStage,
    ) -> Result<(), BundleError> {
        let request = self.store.read_record::<ReviewRequest>(&job.request_ref)?;
        if stage.byte_limit != request.byte_limit {
            return Err(BundleError::Identity(
                "review staging byte limit changed".into(),
            ));
        }
        job.token.generation = job
            .token
            .generation
            .checked_add(1)
            .ok_or(BundleError::Capacity("review generation"))?;
        job.stage = Some(stage);
        job.phase = ReviewPhase::Encoding;
        Ok(())
    }

    fn record_review_check(
        &self,
        job: &mut ReviewJob,
        update: UpdateReviewJob,
    ) -> Result<(), BundleError> {
        let invalid = || BundleError::Identity("review transition or check binding".into());
        let (binding, row, result, reason) = match update {
            UpdateReviewJob::RecordCheck {
                binding,
                row,
                result,
            } => (binding, row, result, None),
            UpdateReviewJob::RecordUnknown {
                binding,
                row,
                reason,
            } => (binding, row, ReviewRowResult::Unknown, Some(reason)),
            _ => return Err(invalid()),
        };
        let ReviewPhase::Checking(checks) = &mut job.phase else {
            return Err(invalid());
        };
        if reason.as_deref().is_some_and(|reason| {
            !crate::editorial::valid_text(reason, seal::MAX_REVIEW_FINDING_NOTE)
        }) {
            return Err(invalid());
        }
        let request = self.store.read_record::<ReviewRequest>(&job.request_ref)?;
        if checks.configuration_sha256 != Some(binding.configuration_sha256)
            || checks.reviewer_attempt.as_ref() != Some(&binding.reviewer_attempt)
            || !request.profile.required_rows().contains(&row)
            || checks.rows.get(&row).is_some_and(|previous| {
                previous.attempt == binding.reviewer_attempt && previous.result != result
            })
        {
            return Err(invalid());
        }
        checks.rows.insert(
            row,
            ReportedReviewCheck {
                attempt: binding.reviewer_attempt.clone(),
                result,
                reason,
            },
        );
        let coverage = checks
            .rows
            .iter()
            .map(|(&id, report)| (ReviewRowId::Required(id), report.result))
            .collect();
        if check_review_coverage(request.profile, &coverage).all_required_passed() {
            let key = self.frozen_review_key(
                &job.token,
                &job.request_ref,
                job.artifact_ref.as_ref(),
                checks,
            )?;
            job.phase = ReviewPhase::ReadyToSeal {
                checks: std::mem::take(checks),
                proposed_key: key.sha256()?,
            };
        }
        Ok(())
    }

    pub fn publish_review_artifact(
        &mut self,
        token: &ReviewToken,
        staged: &mut std::fs::File,
        media_ref: &ObjectRef,
        output: kinewright_media::ReviewOutputTiming,
    ) -> Result<ReviewStatus, BundleError> {
        self.settle_review_root()?;
        let mut job = self.require_review_job(token)?;
        let length = staged.metadata()?.len();
        if media_ref.byte_length != length
            || media_ref.record_kind != RecordKind::OwnedMedia
            || job.phase != ReviewPhase::Encoding
            || job
                .stage
                .as_ref()
                .is_none_or(|stage| length == 0 || length > stage.byte_limit)
        {
            return Err(BundleError::Identity(
                "review artifact has no encoding authority".into(),
            ));
        }
        let mut versions = self.version_index()?.take();
        let descriptor = versions
            .snapshots
            .get_mut(&token.snapshot.version)
            .filter(|d| d.key == token.snapshot)
            .ok_or_else(|| BundleError::Identity("review artifact version missing".into()))?;
        let mut evidence = descriptor
            .review_evidence_ref
            .as_ref()
            .map(|reference| self.store.read_record::<ReviewEvidence>(reference))
            .transpose()?
            .map_or_else(ReviewEvidence::default, StorageReservation::take);
        if evidence.artifacts.len() >= MAX_VERSION_ARTIFACTS {
            return Err(BundleError::Capacity("published artifacts per version"));
        }
        let media = self.store.admit_blob(staged, media_ref, &[])?;
        let artifact = self.store.admit_record(&ReviewArtifact {
            snapshot: token.snapshot.clone(),
            job: token.job.clone(),
            request_ref: job.request_ref.clone(),
            generation: token.generation,
            media_ref: media.reference.clone(),
            output,
        })?;
        evidence.artifacts.push(artifact.reference.clone());
        let evidence = self.store.admit_record(&evidence)?;
        descriptor.review_evidence_ref = Some(evidence.reference.clone());
        let versions = self.store.admit_record(&versions)?;
        job.artifact_ref = Some(artifact.reference.clone());
        job.phase = ReviewPhase::Checking(ReviewChecks::default());
        self.publish_review_job(&job, vec![media, artifact, evidence], Some(versions))?;
        Ok(status(job))
    }

    fn require_review_job(&self, token: &ReviewToken) -> Result<ReviewJob, BundleError> {
        let job = self
            .review_job()?
            .filter(|job| &job.token == token)
            .ok_or_else(|| BundleError::Identity("review generation or version changed".into()))?;
        Ok(job.take())
    }

    fn publish_review_job(
        &mut self,
        job: &ReviewJob,
        mut seals: Vec<SealedObject>,
        versions: Option<SealedObject>,
    ) -> Result<(), BundleError> {
        let versions_ref = versions.as_ref().map(|index| index.reference.clone());
        seals.extend(versions);
        let sealed = self.store.admit_record(job)?;
        let reference = sealed.reference.clone();
        seals.push(sealed);
        self.publish_review_metadata(ValidatedMetadataUpdate {
            review: Some(PendingReviewUpdate::Set(reference)),
            versions: versions_ref,
            seals,
            ..Default::default()
        })
    }

    fn publish_review_metadata(
        &mut self,
        pending: ValidatedMetadataUpdate,
    ) -> Result<(), BundleError> {
        self.pending_metadata = Some(pending);
        let result = BundleCommit::publish_maintenance(self);
        // An uncertain outcome is settled from the disk root. The abandoned
        // update must never ride on a later publication.
        self.pending_metadata = None;
        result.map(|_| ())
    }
}

fn status(job: ReviewJob) -> ReviewStatus {
    ReviewStatus {
        token: job.token,
        phase: job.phase,
        stop_pending: false,
    }
}

#[must_use]
pub fn empty_annotations(snapshot: &SnapshotDescriptor) -> ReviewAnnotations {
    ReviewAnnotations {
        snapshot: ReviewSnapshotId(snapshot.identity.digest()),
        sourced_elements: Vec::new(),
        text_cards: Vec::new(),
        intended_pauses: Vec::new(),
    }
}

impl Ja1EditingSession {
    pub fn review_bundle(&self) -> Result<&ProjectBundle, BundleError> {
        self.active_bundle()
    }

    pub fn queue_review(
        &mut self,
        frozen: &SnapshotDescriptor,
        rulebook_ref: Option<ObjectRef>,
        settings: ExportSettings,
        byte_limit: u64,
        expectations: ReviewExpectations,
    ) -> Result<ReviewToken, BundleError> {
        self.active_bundle_mut()?.queue_review(
            frozen,
            rulebook_ref,
            settings,
            byte_limit,
            expectations,
        )
    }

    pub fn set_review_annotations(
        &mut self,
        snapshot: &SnapshotKey,
        annotations: ReviewAnnotations,
    ) -> Result<(), BundleError> {
        self.active_bundle_mut()?
            .set_review_annotations(snapshot, annotations)
    }

    pub fn update_review_job(
        &mut self,
        token: &ReviewToken,
        update: UpdateReviewJob,
    ) -> Result<ReviewStatus, BundleError> {
        self.active_bundle_mut()?.update_review_job(token, update)
    }

    pub fn seal_review(
        &mut self,
        token: &ReviewToken,
        command: &SessionId,
        expected_configuration_sha256: [u8; 32],
        verified: &VerifiedReviewArtifact,
    ) -> Result<SealResult, BundleError> {
        self.active_bundle_mut()?.seal_review(
            token,
            command,
            expected_configuration_sha256,
            verified,
        )
    }

    /// The seal receipts kept on this exact version.
    pub fn review_receipts(&self, key: &SnapshotKey) -> Result<Vec<ReviewReceipt>, BundleError> {
        let bundle = self.active_bundle()?;
        let snapshot = bundle.read_snapshot(key)?;
        Ok(bundle
            .review_receipts(&snapshot.descriptor)?
            .into_iter()
            .map(|(_, receipt)| receipt.take())
            .collect())
    }

    pub fn review_annotations(
        &self,
        key: &SnapshotKey,
    ) -> Result<Option<ReviewAnnotationFields>, BundleError> {
        let bundle = self.active_bundle()?;
        let snapshot = bundle.read_snapshot(key)?;
        Ok(bundle
            .review_annotations(&snapshot.descriptor)?
            .map(|saved| {
                let StoredReviewAnnotations(annotations) = saved.take();
                ReviewAnnotationFields {
                    sourced_elements: annotations.sourced_elements,
                    text_cards: annotations.text_cards,
                    intended_pauses: annotations.intended_pauses,
                }
            }))
    }
}

impl crate::studio_store::PinnedSnapshot {
    pub fn open_review_inputs(&self) -> Result<ReviewPreparationInputs, BundleError> {
        let store = self.pin.read_store();
        let document = store.read_record(&self.descriptor.document_ref)?;
        let manifest = store.read_input_manifest(&self.descriptor.input_manifest_ref)?;
        let annotations = self
            .descriptor
            .review_annotations_ref
            .as_ref()
            .map(|reference| store.read_record(reference))
            .transpose()?;
        Ok(ReviewPreparationInputs {
            document,
            manifest,
            annotations,
        })
    }
}

pub struct ReviewPreparationInputs {
    pub document: StorageReservation<kinewright_core::Document>,
    pub manifest: StorageReservation<crate::owned_inputs::OwnedInputManifest>,
    pub annotations: Option<StorageReservation<StoredReviewAnnotations>>,
}

/// Pinned review artifact metadata prepared under the writer lock so full-file SHA-256
/// verification of the artifact media bytes runs strictly off-lock (L13).
pub struct PreparedReviewArtifact {
    pub artifact: StorageReservation<ReviewArtifact>,
    reference: ObjectRef,
    pin: crate::store::StorePin,
}

impl PreparedReviewArtifact {
    /// Verify the pinned artifact media bytes off the writer lock and return the rewound file.
    pub fn open_verified(&self) -> Result<std::fs::File, BundleError> {
        self.pin
            .read_store()
            .open_verified(&self.artifact.media_ref)
    }

    /// Cancellation-aware verification of the pinned artifact media bytes off the writer lock.
    pub fn open_verified_cancellable(
        &self,
        cancellation: &kinewright_core::ExportCancellation,
    ) -> Result<std::fs::File, BundleError> {
        self.pin
            .read_store()
            .open_verified_cancellable(&self.artifact.media_ref, cancellation)
    }
}

impl ProjectBundle {
    pub fn settle_review_root(&mut self) -> Result<(), BundleError> {
        if self.unsettled
            || self.pending_metadata.is_some()
            || self.store.has_unsettled_objects()?
        {
            // A save, command or copy whose outcome is unknown is settled only
            // by its own owner, which still holds what it attempted.
            if self.pending_publication.is_some()
                || self.pending_command.is_some()
                || self.pending_result.is_some()
                || self.pending_terminal_publication.is_some()
                || self.copy_publication.is_some()
            {
                return Err(BundleError::Unsettled);
            }
            let _budget = TerminalBudget::enter(&self.store);
            self.reconcile_metadata()?;
        }
        Ok(())
    }

    /// Read the small review-artifact record and pin its media bytes under the writer lock
    /// without hashing the media file on the lock (L13).
    pub fn prepare_review_artifact(
        &self,
        token: &ReviewToken,
    ) -> Result<PreparedReviewArtifact, BundleError> {
        let job = self
            .review_job()?
            .filter(|job| &job.token == token && job.phase != ReviewPhase::Stopping)
            .ok_or_else(|| BundleError::Identity("review artifact authority changed".into()))?;
        let reference = job.artifact_ref.as_ref().ok_or_else(|| {
            BundleError::Identity("review artifact has not been published".into())
        })?;
        let artifact = self.store.read_record::<ReviewArtifact>(reference)?;
        let pin = self
            .store
            .pin(&[reference.clone(), artifact.media_ref.clone()])?;
        Ok(PreparedReviewArtifact {
            artifact,
            reference: reference.clone(),
            pin,
        })
    }

    pub fn open_review_artifact(
        &self,
        token: &ReviewToken,
    ) -> Result<(StorageReservation<ReviewArtifact>, std::fs::File), BundleError> {
        let prepared = self.prepare_review_artifact(token)?;
        let verified = prepared.open_verified()?;
        Ok((prepared.artifact, verified))
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
