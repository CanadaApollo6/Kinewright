//! Host seam running L14 compositor-backed decoded-region overlay verification on frozen review
//! inputs and recording `RequiredReviewRow::OverlayVerification` exclusively through
//! `HostCommandOwner::record_review_check`.

#![cfg(target_os = "linux")]

use std::sync::{Arc, Mutex};

use kinewright_core::{ExportCancellation, MediaError, output_review::RequiredReviewRow};
use kinewright_media::{OverlayVerificationConfig, OverlayVerificationReport, ReviewOutputTiming};
use kinewright_project::{
    bundle_records::ObjectRef,
    review_job::{
        PreparedReviewArtifact, ReviewCheckBinding, ReviewPhase, ReviewRequest, ReviewStatus,
        ReviewToken, StoredReviewExpectations, UpdateReviewJob,
    },
    storage_budget::StorageReservation,
    studio_store::FrozenContextRequest,
};

pub use super::review_stills::{
    C6CheckConfiguration, combined_review_check_configuration,
    overlay_and_stills_check_configuration,
};
use super::{HostCommandOwner, HostError, HostRefusal};

fn refused(message: &str) -> HostError {
    HostRefusal::new("review_overlays", message).into()
}

/// Frozen overlay verification inputs prepared under the host lock without hashing any media
/// bytes, then verified and rendered purely off-lock. Never reads the working edit.
pub struct PreparedOverlayVerification {
    pub token: ReviewToken,
    pub artifact_ref: ObjectRef,
    pub artifact_media_ref: ObjectRef,
    pub artifact_timing: ReviewOutputTiming,
    artifact: PreparedReviewArtifact,
    context: FrozenContextRequest,
    expectations: StorageReservation<StoredReviewExpectations>,
    overlay_config: OverlayVerificationConfig,
    data_dir: Option<std::path::PathBuf>,
    cancellation: ExportCancellation,
    #[cfg(test)]
    verify_hook: Option<super::review_job::ReviewCancelHook>,
}

impl HostCommandOwner {
    /// Validate configuration binding and pin only frozen review records (`prepare_review_artifact`,
    /// `prepare_frozen_context_descriptor`, and `ReviewExpectations`) under the host lock.
    /// Performs zero media-byte hashing or decoding while the lock is held.
    ///
    /// # Errors
    /// Refuses when the configuration is invalid, disagrees with `binding.configuration_sha256` or
    /// the active `ReviewJob` binding, checking authority is missing, the Stop fence is armed, or
    /// frozen records cannot be pinned.
    pub fn prepare_overlay_verification(
        &mut self,
        token: &ReviewToken,
        binding: &ReviewCheckBinding,
        config: &C6CheckConfiguration,
        external_cancellation: Option<&ExportCancellation>,
    ) -> Result<PreparedOverlayVerification, HostError> {
        config.validate()?;
        if binding.configuration_sha256 != config.configuration_sha256() {
            return Err(refused(
                "Overlay verification configuration does not match the bound configuration digest.",
            ));
        }
        self.review_can_record(token)?;
        let (artifact_ref, artifact, context, expectations) = {
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
                    "Overlay verification binding does not match the active review check binding.",
                ));
            }
            let artifact_ref = job
                .artifact_ref
                .clone()
                .ok_or_else(|| refused("The output review artifact has not been published."))?;
            let request = bundle
                .store()
                .read_record::<ReviewRequest>(&job.request_ref)?;
            let expectations = bundle
                .store()
                .read_record::<StoredReviewExpectations>(&request.expectations_ref)?;
            let snapshot = bundle.read_snapshot(&token.snapshot)?;
            let context = bundle.prepare_frozen_context_descriptor(&snapshot.descriptor)?;
            let artifact = bundle.prepare_review_artifact(token)?;
            (artifact_ref, artifact, context, expectations)
        };
        let cancellation = self
            .review_runtime
            .register_review_cancellation(token, external_cancellation);
        Ok(PreparedOverlayVerification {
            token: token.clone(),
            artifact_ref,
            artifact_media_ref: artifact.artifact.media_ref.clone(),
            artifact_timing: artifact.artifact.output.clone(),
            artifact,
            context,
            expectations,
            overlay_config: config.overlay.clone(),
            data_dir: self.config.data_dir.clone(),
            cancellation,
            #[cfg(test)]
            verify_hook: self.review_runtime.overlay_verify_hook.clone(),
        })
    }

    /// Record the overlay verification row result through the single `record_review_check` seam
    /// after validating the configuration and matching it against `binding.configuration_sha256`.
    ///
    /// # Errors
    /// Refuses when configuration validation, digest binding, checking fence, Stop fence, or durable
    /// transition fails.
    pub fn record_overlay_verification(
        &mut self,
        token: &ReviewToken,
        binding: ReviewCheckBinding,
        config: &C6CheckConfiguration,
        report: &OverlayVerificationReport,
    ) -> Result<ReviewStatus, HostError> {
        config.validate()?;
        if binding.configuration_sha256 != config.configuration_sha256() {
            return Err(refused(
                "Overlay verification configuration does not match the bound configuration digest.",
            ));
        }
        self.record_review_check(
            token,
            UpdateReviewJob::RecordCheck {
                binding,
                row: RequiredReviewRow::OverlayVerification,
                result: report.row_result,
            },
        )
    }

    #[cfg(test)]
    pub(super) fn set_overlay_verify_hook(
        &mut self,
        hook: Option<super::review_job::ReviewCancelHook>,
    ) {
        self.review_runtime.overlay_verify_hook = hook;
    }
}

/// Run artifact SHA-256 verification and L14 compositor-backed overlay verification completely
/// off the host lock over a prepared frozen input bundle.
///
/// # Errors
/// Returns [`MediaError::Cancelled`] when Stop or caller cancellation fires; undecidable
/// expectations or stream failures are returned as `ReviewRowResult::Unknown` inside
/// [`OverlayVerificationReport`].
pub fn run_prepared_overlay_verification(
    prepared: PreparedOverlayVerification,
) -> Result<OverlayVerificationReport, MediaError> {
    #[cfg(test)]
    if let Some(hook) = &prepared.verify_hook {
        hook(&prepared.cancellation);
    }
    if prepared.cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    let artifact_file = prepared
        .artifact
        .open_verified_cancellable(&prepared.cancellation)
        .map_err(|error| {
            if prepared.cancellation.is_cancelled() {
                MediaError::Cancelled
            } else {
                MediaError::Backend(error.to_string())
            }
        })?;
    if prepared.cancellation.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    let inputs = Arc::new(
        prepared
            .context
            .open()
            .map_err(|error| MediaError::Backend(error.to_string()))?,
    )
    .playback_inputs()
    .map_err(|error| MediaError::Backend(error.to_string()))?;
    let engine = super::review_job::review_media_engine_for_data_dir(prepared.data_dir)
        .map_err(|error| MediaError::Backend(error.to_string()))?;
    engine.verify_review_overlays(
        artifact_file,
        &prepared.artifact_timing,
        &prepared.expectations.0,
        &inputs,
        &prepared.overlay_config,
        &prepared.cancellation,
    )
}

/// Prepare frozen inputs under lock (zero media hashing), verify bytes and run L14 overlay
/// verification off lock, and record `RequiredReviewRow::OverlayVerification` through
/// `record_review_check`.
///
/// # Errors
/// Refuses when configuration validation/binding, preparation, cancellation, or recording fails.
pub fn run_overlay_verification(
    host: &Mutex<HostCommandOwner>,
    token: &ReviewToken,
    binding: ReviewCheckBinding,
    config: &C6CheckConfiguration,
    cancellation: &ExportCancellation,
) -> Result<(OverlayVerificationReport, ReviewStatus), HostError> {
    let prepared = {
        let mut owner = host
            .lock()
            .map_err(|_| refused("Output review owner is unavailable."))?;
        owner.prepare_overlay_verification(token, &binding, config, Some(cancellation))?
    };
    let report =
        run_prepared_overlay_verification(prepared).map_err(|error| refused(&error.to_string()))?;
    let status = {
        let mut owner = host
            .lock()
            .map_err(|_| refused("Output review owner is unavailable."))?;
        owner.record_overlay_verification(token, binding, config, &report)?
    };
    Ok((report, status))
}
