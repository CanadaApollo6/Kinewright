//! JA-1 S5a commit 7: one binding, one driver, then the seal.
//!
//! The driver forms a single [`ReviewCheckBinding`] whose digest covers the
//! commit-5 configuration, the overlay and stills configurations and the
//! reviewer attempt. It binds once under the writer lock and then runs the
//! technical checks, the overlay verification and the Reviewer's stills
//! session with the lock released. Every row reaches the job only through
//! `record_review_check`; a check that errors is recorded Unknown with its
//! reason and the other rows still run. The visual row is the bound Reviewer
//! attempt's own `submit_visual_judgment`, never a host caller's.
use std::{
    sync::{Arc, Mutex, MutexGuard, Weak},
    thread,
    time::Duration,
};

use kinewright_core::{
    ExportCancellation, MediaError,
    output_review::{RequiredReviewRow, ReviewRowResult},
};
use kinewright_media::{
    OverlayVerificationConfig, ReviewStillsConfig,
    review_scan::{RetainedSpeechConfig, TechnicalScanConfig},
};
use kinewright_project::{
    bundle_records::SessionId,
    orchestration_records::{AttemptState, Role},
    review_job::{ReviewCheckBinding, ReviewPhase, ReviewToken, SealResult, UpdateReviewJob},
};

use super::{
    super::review_overlays::{C6CheckConfiguration, run_prepared_overlay_verification},
    HostCommandOwner, HostError, refused,
    review_checks::{
        CheckReview, check_reason, review_check_configuration, run_technical_review_checks,
    },
};

const DRIVER_CONFIGURATION_TAG: &[u8] = b"kinewright.review-driver.s5a-c7.v1\0";
const REVIEWER_POLL: Duration = Duration::from_millis(50);
const CANCELLED: &str = "The review checks were cancelled.";

/// Every checker configuration one binding covers.
pub(in crate::writer_host) struct DriverConfiguration {
    pub scan: TechnicalScanConfig,
    pub speech: RetainedSpeechConfig,
    pub c6: C6CheckConfiguration,
}

impl DriverConfiguration {
    /// The fixed configuration production checks run with.
    pub(in crate::writer_host) fn production(reviewer_attempt: &SessionId) -> Self {
        Self::new(
            TechnicalScanConfig::default(),
            RetainedSpeechConfig::default(),
            OverlayVerificationConfig::default(),
            ReviewStillsConfig::default(),
            reviewer_attempt,
        )
    }

    pub(in crate::writer_host) fn new(
        scan: TechnicalScanConfig,
        speech: RetainedSpeechConfig,
        overlay: OverlayVerificationConfig,
        stills: ReviewStillsConfig,
        reviewer_attempt: &SessionId,
    ) -> Self {
        let mut digest = kinewright_hash::Sha256::new();
        digest.update(DRIVER_CONFIGURATION_TAG);
        digest.update(&review_check_configuration(&scan, &speech));
        digest.update(reviewer_attempt.as_str().as_bytes());
        let c6 = C6CheckConfiguration {
            overlay,
            stills,
            other_checks_sha256: Some(digest.finalize()),
        };
        Self { scan, speech, c6 }
    }

    pub(in crate::writer_host) fn binding(
        &self,
        reviewer_attempt: &SessionId,
    ) -> ReviewCheckBinding {
        ReviewCheckBinding {
            configuration_sha256: self.c6.configuration_sha256(),
            reviewer_attempt: reviewer_attempt.clone(),
        }
    }
}

/// The bound Reviewer's session. It reads stills and submits its judgment
/// through its own task tools while the driver waits with the lock released.
pub(in crate::writer_host) trait ReviewerSession {
    /// Return once the reviewer has judged or finished. `Err` is the reason
    /// the visual row stays Unknown.
    fn await_judgment(
        &mut self,
        host: &Weak<Mutex<HostCommandOwner>>,
        token: &ReviewToken,
        binding: &ReviewCheckBinding,
        cancel: &ExportCancellation,
    ) -> Result<(), String>;
}

/// Production: wait for the bound attempt's durable visual row, or for the
/// attempt to retire without one.
pub(in crate::writer_host) struct AwaitReviewer;

impl ReviewerSession for AwaitReviewer {
    fn await_judgment(
        &mut self,
        host: &Weak<Mutex<HostCommandOwner>>,
        token: &ReviewToken,
        binding: &ReviewCheckBinding,
        cancel: &ExportCancellation,
    ) -> Result<(), String> {
        loop {
            if cancel.is_cancelled() {
                return Err(CANCELLED.into());
            }
            {
                let host = upgrade(host).map_err(|error| error.to_string())?;
                let owner = lock(&host).map_err(|error| error.to_string())?;
                if owner
                    .review_row(token, RequiredReviewRow::IndependentVisualReview)
                    .map_err(|error| error.to_string())?
                    .is_some_and(|attempt| attempt == binding.reviewer_attempt)
                {
                    return Ok(());
                }
                let retired = owner
                    .reviewer_attempt(token, &binding.reviewer_attempt)
                    .map_err(|error| error.to_string())?
                    .is_none_or(|state| matches!(state, AttemptState::Retired(_)));
                if retired {
                    return Err("The reviewer finished without a visual judgment.".into());
                }
            }
            thread::sleep(REVIEWER_POLL);
        }
    }
}

fn upgrade(
    host: &Weak<Mutex<HostCommandOwner>>,
) -> Result<Arc<Mutex<HostCommandOwner>>, HostError> {
    host.upgrade()
        .ok_or_else(|| refused("Output review owner is unavailable."))
}

fn lock(host: &Mutex<HostCommandOwner>) -> Result<MutexGuard<'_, HostCommandOwner>, HostError> {
    host.lock()
        .map_err(|_| refused("Output review owner is unavailable."))
}

fn cancelled(cancel: &ExportCancellation) -> Result<(), HostError> {
    if cancel.is_cancelled() {
        return Err(refused(CANCELLED));
    }
    Ok(())
}

/// Bind once, run every check with the lock released, record each row, then
/// seal.
///
/// # Errors
/// The reviewer attempt is not a Reviewer of this cut, a fence refused a
/// binding or a row (Stop, a changed generation), or the run was cancelled.
/// Rows recorded before the refusal stay recorded.
#[allow(clippy::too_many_arguments)]
pub(in crate::writer_host) fn drive_review(
    host: &Weak<Mutex<HostCommandOwner>>,
    token: &ReviewToken,
    reviewer_attempt: &SessionId,
    command: &SessionId,
    config: &DriverConfiguration,
    check: &CheckReview<'_>,
    reviewer: &mut dyn ReviewerSession,
    cancel: &ExportCancellation,
) -> Result<SealResult, HostError> {
    let binding = config.binding(reviewer_attempt);
    {
        let host = upgrade(host)?;
        let mut owner = lock(&host)?;
        owner.require_reviewer_of_cut(token, reviewer_attempt)?;
        owner.record_review_check(token, UpdateReviewJob::BindChecks(binding.clone()))?;
        owner.register_bound_c6_configuration(token, &binding, &config.c6)?;
    }
    cancelled(cancel)?;
    // Technical and retained-speech rows: an errored check is recorded Unknown.
    run_technical_review_checks(&*upgrade(host)?, token, &binding, check)?;
    cancelled(cancel)?;
    // Overlay verification, off-lock.
    let prepared = lock(&*upgrade(host)?)?.prepare_overlay_verification(
        token,
        &binding,
        &config.c6,
        Some(cancel),
    )?;
    let overlay = run_prepared_overlay_verification(prepared);
    {
        let host = upgrade(host)?;
        let mut owner = lock(&host)?;
        match overlay {
            Ok(report) => {
                owner.record_overlay_verification(token, binding.clone(), &config.c6, &report)?;
            }
            Err(MediaError::Cancelled) => return Err(refused(CANCELLED)),
            Err(error) => {
                owner.record_review_check(
                    token,
                    UpdateReviewJob::RecordUnknown {
                        binding: binding.clone(),
                        row: RequiredReviewRow::OverlayVerification,
                        reason: check_reason(&error.to_string()),
                    },
                )?;
            }
        }
    }
    // The Reviewer's own judgment, with the lock released.
    let judged = reviewer.await_judgment(host, token, &binding, cancel);
    cancelled(cancel)?;
    {
        let host = upgrade(host)?;
        let mut owner = lock(&host)?;
        let recorded = owner.review_row(token, RequiredReviewRow::IndependentVisualReview)?;
        if recorded.as_ref() != Some(&binding.reviewer_attempt) {
            let reason = judged
                .err()
                .unwrap_or_else(|| "The reviewer submitted no visual judgment.".into());
            owner.record_review_check(
                token,
                UpdateReviewJob::RecordUnknown {
                    binding: binding.clone(),
                    row: RequiredReviewRow::IndependentVisualReview,
                    reason: check_reason(&reason),
                },
            )?;
        }
        let audit = owner
            .owner()?
            .review_source_bundle(&token.project)?
            .review_audit_complete(token)?;
        let update = match audit {
            Ok(()) => UpdateReviewJob::RecordCheck {
                binding: binding.clone(),
                row: RequiredReviewRow::EditorialAuditPresent,
                result: ReviewRowResult::Pass,
            },
            Err(reason) => UpdateReviewJob::RecordUnknown {
                binding: binding.clone(),
                row: RequiredReviewRow::EditorialAuditPresent,
                reason: reason.into(),
            },
        };
        owner.record_review_check(token, update)?;
    }
    seal_checked_review(
        &*upgrade(host)?,
        token,
        command,
        binding.configuration_sha256,
        cancel,
    )
}

/// Seal a job whose rows are recorded, or return the receipt a lost reply
/// already published. The artifact is verified with the lock released.
///
/// # Errors
/// A fence refused, the artifact failed verification, or publication failed;
/// the job then stays as it was and the seal can be retried.
pub(in crate::writer_host) fn seal_checked_review(
    host: &Mutex<HostCommandOwner>,
    token: &ReviewToken,
    command: &SessionId,
    expected_configuration_sha256: [u8; 32],
    cancel: &ExportCancellation,
) -> Result<SealResult, HostError> {
    let prepared = {
        let mut owner = lock(host)?;
        if let Some(sealed) = owner
            .owner_mut()?
            .review_source_bundle_mut(&token.project)?
            .sealed_review(token)?
        {
            return Ok(SealResult::Sealed(Box::new(sealed)));
        }
        owner.review_can_seal(token)?;
        owner
            .owner()?
            .review_source_bundle(&token.project)?
            .prepare_review_artifact(token)?
    };
    let verified = prepared.verify_for_seal(cancel)?;
    cancelled(cancel)?;
    let mut owner = lock(host)?;
    owner.review_can_seal(token)?;
    let result = owner
        .owner_mut()?
        .review_source_bundle_mut(&token.project)?
        .seal_review(token, command, expected_configuration_sha256, &verified)?;
    if matches!(result, SealResult::Sealed(_)) {
        owner.release_sealed_review(token);
    }
    Ok(result)
}

impl HostCommandOwner {
    /// The attempt that recorded `row`, if any.
    pub(in crate::writer_host) fn review_row(
        &self,
        token: &ReviewToken,
        row: RequiredReviewRow,
    ) -> Result<Option<SessionId>, HostError> {
        let bundle = self.owner()?.review_source_bundle(&token.project)?;
        Ok(bundle
            .review_job()?
            .filter(|job| &job.token == token)
            .and_then(|job| match &job.phase {
                ReviewPhase::Checking(checks) | ReviewPhase::ReadyToSeal { checks, .. } => {
                    checks.rows.get(&row).map(|report| report.attempt.clone())
                }
                _ => None,
            }))
    }

    /// The state of `attempt` when it is a Reviewer task attempt on the
    /// reviewed version's cut.
    pub(in crate::writer_host) fn reviewer_attempt(
        &self,
        token: &ReviewToken,
        attempt: &SessionId,
    ) -> Result<Option<AttemptState>, HostError> {
        let bundle = self.owner()?.review_source_bundle(&token.project)?;
        let checkpoint = bundle
            .read_snapshot(&token.snapshot)?
            .descriptor
            .checkpoint_ref
            .clone();
        let runs: Vec<_> = bundle.run_index()?.orchestration.keys().cloned().collect();
        for run in runs {
            let Some(projection) = bundle.orchestration_projection(&run)? else {
                continue;
            };
            let Some(reservation) = projection
                .attempts
                .iter()
                .find(|reservation| &reservation.identity.attempt.0 == attempt)
            else {
                continue;
            };
            let reviewer = projection.tasks.iter().any(|task| {
                task.id == reservation.identity.task
                    && task.brief.role == Role::Reviewer
                    && task.brief.scope.base.checkpoint == checkpoint
            });
            return Ok(reviewer.then(|| reservation.state.clone()));
        }
        Ok(None)
    }

    fn require_reviewer_of_cut(
        &self,
        token: &ReviewToken,
        attempt: &SessionId,
    ) -> Result<(), HostError> {
        match self.reviewer_attempt(token, attempt)? {
            Some(AttemptState::Retired(_)) => {
                Err(refused("The reviewer attempt has already finished."))
            }
            Some(_) => Ok(()),
            None => Err(refused(
                "The visual reviewer must be a Reviewer task attempt on the reviewed cut.",
            )),
        }
    }
}
