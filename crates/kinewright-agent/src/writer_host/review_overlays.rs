//! Host seam running decoded-region overlay verification on frozen review inputs and recording
//! `RequiredReviewRow::OverlayVerification` exclusively through `HostCommandOwner::record_review_check`.

#![cfg(target_os = "linux")]

use std::{collections::BTreeMap, fs::File, sync::Mutex};

use kinewright_core::{Document, ExportCancellation, MediaError, output_review::RequiredReviewRow};
use kinewright_media::{
    OverlayVerificationConfig, OverlayVerificationReport, ReviewOutputTiming, ReviewReadSeek,
    ReviewStillsConfig, verify_review_overlays,
};
use kinewright_project::{
    bundle_records::ObjectRef,
    owned_inputs::InputLeaseRequest,
    review_job::{
        ReviewCheckBinding, ReviewRequest, ReviewStatus, ReviewToken, StoredReviewExpectations,
        UpdateReviewJob,
    },
    storage_budget::StorageReservation,
};

use super::{HostCommandOwner, HostError, HostRefusal, review_stills::collect_pinned_image_leases};

fn refused(message: &str) -> HostError {
    HostRefusal::new("review_overlays", message).into()
}

/// Stable SHA-256 digest of the overlay-verification and reviewer-stills configurations combined.
/// Callers combine this with C5's check configuration digest into a single `ReviewCheckBinding`
/// before calling `BindChecks`; neither C6 runner binds checks on its own.
#[must_use]
pub fn overlay_and_stills_check_configuration(
    overlay: &OverlayVerificationConfig,
    stills: &ReviewStillsConfig,
) -> [u8; 32] {
    let mut hasher = kinewright_hash::Sha256::new();
    hasher.update(b"kinewright-overlay-and-stills-check-config-v1\0");
    hasher.update(&overlay.sha256());
    hasher.update(&stills.sha256());
    hasher.finalize()
}

/// Frozen overlay verification inputs opened under the host lock and executed purely off-lock.
/// Never reads the working edit.
pub struct PreparedOverlayVerification {
    pub token: ReviewToken,
    pub artifact_ref: ObjectRef,
    pub artifact_media_ref: ObjectRef,
    pub artifact_timing: ReviewOutputTiming,
    artifact_file: File,
    expectations: StorageReservation<StoredReviewExpectations>,
    document: StorageReservation<Document>,
    pinned_images: BTreeMap<([u8; 32], u64), InputLeaseRequest>,
}

impl HostCommandOwner {
    /// Open only frozen review inputs (`open_review_artifact`, frozen `Document`, frozen
    /// `ReviewExpectations`, and lease requests for pinned card blobs) under the host lock.
    ///
    /// # Errors
    /// Refuses when the host lacks checking authority, the Stop fence is armed, or frozen records
    /// cannot be opened and verified.
    pub fn prepare_overlay_verification(
        &self,
        token: &ReviewToken,
    ) -> Result<PreparedOverlayVerification, HostError> {
        self.review_can_record(token)?;
        let bundle = self.owner()?.review_source_bundle(&token.project)?;
        let job = bundle
            .review_job()?
            .filter(|job| &job.token == token)
            .ok_or_else(|| refused("The output review generation or version changed."))?;
        let artifact_ref = job
            .artifact_ref
            .clone()
            .ok_or_else(|| refused("The output review artifact has not been published."))?;
        let request = bundle
            .store()
            .read_record::<ReviewRequest>(&job.request_ref)?;
        let document = bundle
            .store()
            .read_record::<Document>(&request.document_ref)?;
        let expectations = bundle
            .store()
            .read_record::<StoredReviewExpectations>(&request.expectations_ref)?;
        let manifest = bundle
            .store()
            .read_input_manifest(&request.input_manifest_ref)?;
        let (artifact, artifact_file) = bundle.open_review_artifact(token)?;
        let pinned_images = collect_pinned_image_leases(
            bundle.store(),
            &request.input_manifest_ref,
            &manifest,
            &expectations.0,
        )?;
        Ok(PreparedOverlayVerification {
            token: token.clone(),
            artifact_ref,
            artifact_media_ref: artifact.media_ref.clone(),
            artifact_timing: artifact.output.clone(),
            artifact_file,
            expectations,
            document,
            pinned_images,
        })
    }

    /// Record the overlay verification row result through the single `record_review_check` seam.
    ///
    /// # Errors
    /// Refuses when the checking fence, binding, Stop fence, or durable transition fails.
    pub fn record_overlay_verification(
        &mut self,
        token: &ReviewToken,
        binding: ReviewCheckBinding,
        report: &OverlayVerificationReport,
    ) -> Result<ReviewStatus, HostError> {
        self.record_review_check(
            token,
            UpdateReviewJob::RecordCheck {
                binding,
                row: RequiredReviewRow::OverlayVerification,
                result: report.row_result,
            },
        )
    }
}

/// Run the pure decoded-region overlay check off the host lock over a prepared frozen input bundle.
///
/// # Errors
/// Returns [`MediaError::Cancelled`] when `cancellation` fires; undecidable expectations or stream
/// failures are returned as `ReviewRowResult::Unknown` inside [`OverlayVerificationReport`].
pub fn run_prepared_overlay_verification(
    mut prepared: PreparedOverlayVerification,
    config: &OverlayVerificationConfig,
    cancellation: &ExportCancellation,
) -> Result<OverlayVerificationReport, MediaError> {
    let mut loader = |sha256: [u8; 32],
                      byte_length: u64|
     -> Result<Box<dyn ReviewReadSeek>, MediaError> {
        let lease_request = prepared
            .pinned_images
            .remove(&(sha256, byte_length))
            .ok_or_else(|| {
                MediaError::Backend("pinned overlay card not found in frozen input manifest".into())
            })?;
        let lease = lease_request
            .open()
            .map_err(|error| MediaError::Backend(error.to_string()))?;
        Ok(Box::new(lease))
    };
    verify_review_overlays(
        prepared.artifact_file,
        &prepared.artifact_timing,
        &prepared.expectations.0,
        &prepared.document,
        &mut loader,
        config,
        cancellation,
    )
}

/// Prepare frozen inputs under lock, run overlay verification off lock, and record
/// `RequiredReviewRow::OverlayVerification` through `record_review_check`.
///
/// # Errors
/// Refuses when preparation, cancellation, or recording fails.
pub fn run_overlay_verification(
    host: &Mutex<HostCommandOwner>,
    token: &ReviewToken,
    binding: ReviewCheckBinding,
    config: &OverlayVerificationConfig,
    cancellation: &ExportCancellation,
) -> Result<(OverlayVerificationReport, ReviewStatus), HostError> {
    let prepared = {
        let owner = host
            .lock()
            .map_err(|_| refused("Output review owner is unavailable."))?;
        owner.prepare_overlay_verification(token)?
    };
    let report = run_prepared_overlay_verification(prepared, config, cancellation)
        .map_err(|error| refused(&error.to_string()))?;
    let status = {
        let mut owner = host
            .lock()
            .map_err(|_| refused("Output review owner is unavailable."))?;
        owner.record_overlay_verification(token, binding, &report)?
    };
    Ok((report, status))
}
