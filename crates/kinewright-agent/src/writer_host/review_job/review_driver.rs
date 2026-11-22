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
    sync::{
        Arc, Mutex, MutexGuard, Weak,
        atomic::{AtomicBool, Ordering},
    },
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
    orchestration_records::AttemptState,
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
const NO_REVIEWER: &str =
    "No Reviewer is left to check this cut. Open the project and resume the review.";

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

/// What the driver does next for its job.
enum DriverStep {
    /// Checking with no Reviewer bound yet, or the encode worker still
    /// returning: wait.
    Wait,
    Drive(SessionId),
    Seal,
    /// JA-1 S6 C3: the GUI detached and no retained run can still start a
    /// Reviewer, so the job pauses instead of waiting forever.
    NoReviewer,
    Done,
}

/// The next step, decided under the lock. A step that ends the driver marks
/// it exiting before the lock is released, so a Resume that follows starts a
/// new driver instead of waiting on this one.
fn driver_step(
    host: &Weak<Mutex<HostCommandOwner>>,
    token: &ReviewToken,
    cancel: &ExportCancellation,
    stalled: bool,
    exiting: &AtomicBool,
) -> Result<DriverStep, HostError> {
    let host = upgrade(host)?;
    let mut owner = lock(&host)?;
    let step = owner.next_driver_step(token, cancel, stalled);
    if !matches!(
        step,
        Ok(DriverStep::Wait | DriverStep::Drive(_) | DriverStep::Seal | DriverStep::NoReviewer)
    ) {
        exiting.store(true, Ordering::Release);
    }
    step
}

impl HostCommandOwner {
    fn next_driver_step(
        &mut self,
        token: &ReviewToken,
        cancel: &ExportCancellation,
        stalled: bool,
    ) -> Result<DriverStep, HostError> {
        // A Stop, a change of project or the seal ended this host's authority
        // over the job; a failure that could not be recorded waits for Status.
        if stalled || cancel.is_cancelled() || self.review_runtime.verified.as_ref() != Some(token)
        {
            return Ok(DriverStep::Done);
        }
        // A freeze waits; a write already under way refuses at its own gate.
        self.review_owned(token)?;
        if self.review_frozen() {
            return Ok(DriverStep::Wait);
        }
        // The encode worker holds the staging until it is retired; the seal
        // refuses while it is held, so retire a finished worker here and wait
        // for one that is still returning.
        self.retire_review_worker()?;
        if self
            .review_runtime
            .worker
            .as_ref()
            .is_some_and(|worker| &worker.token == token)
        {
            return Ok(DriverStep::Wait);
        }
        let bundle = self.owner()?.review_source_bundle(&token.project)?;
        Ok(
            match bundle
                .review_job()?
                .filter(|job| &job.token == token)
                .map(|job| job.phase.clone())
            {
                Some(ReviewPhase::ReadyToSeal { .. }) => DriverStep::Seal,
                Some(ReviewPhase::Checking(checks)) => match checks.reviewer_attempt {
                    Some(attempt) => DriverStep::Drive(attempt),
                    None if self.no_reviewer_can_start() => DriverStep::NoReviewer,
                    None => DriverStep::Wait,
                },
                _ => DriverStep::Done,
            },
        )
    }

    /// A Reviewer attempt is a task of a retained run (lead question 2): with
    /// the GUI detached and no run still working or hook pending, none can
    /// bind until the person reopens the project.
    fn no_reviewer_can_start(&self) -> bool {
        self.detached && self.hooks.pending.is_none() && !self.retained_run_working()
    }
}

/// The production driver: wait for a Reviewer of the cut to claim the
/// check, run every check, then seal. A `ReadyToSeal` job is sealed directly.
/// A failure pauses the job with its reason where Status shows it, so only
/// `Resume` continues; a Stop or shutdown cancels it instead. The driver
/// stays with its job until a step under the lock ends it.
pub(in crate::writer_host) fn run_review_driver(
    host: &Weak<Mutex<HostCommandOwner>>,
    token: &ReviewToken,
    command: &SessionId,
    cancel: &ExportCancellation,
    exiting: &AtomicBool,
) {
    let mut stalled = false;
    loop {
        let failed = match driver_step(host, token, cancel, stalled, exiting) {
            Ok(DriverStep::Wait) => {
                thread::sleep(REVIEWER_POLL);
                continue;
            }
            Ok(DriverStep::Done) | Err(_) => return,
            Ok(DriverStep::NoReviewer) => Some(refused(NO_REVIEWER)),
            Ok(DriverStep::Drive(attempt)) => {
                let config = DriverConfiguration::production(&attempt);
                let check =
                    super::review_checks::commit_5_check(&config.scan, &config.speech, cancel);
                drive_review(
                    host,
                    token,
                    &attempt,
                    command,
                    &config,
                    &check,
                    &mut AwaitReviewer,
                    cancel,
                )
                .err()
            }
            Ok(DriverStep::Seal) => (|| {
                let host = upgrade(host)?;
                let expected = lock(&host)?.production_review_digest(token)?;
                seal_checked_review(&host, token, command, expected, cancel)
            })()
            .err(),
        };
        if let Some(error) = failed {
            stalled = !pause_after_failure(host, token, cancel, &error);
        }
    }
}

/// Record why the driver's checks or seal failed. `false`: nothing was
/// recorded, and the driver leaves the job to the next Status. During a
/// freeze nothing is recorded and the driver stays.
fn pause_after_failure(
    host: &Weak<Mutex<HostCommandOwner>>,
    token: &ReviewToken,
    cancel: &ExportCancellation,
    error: &HostError,
) -> bool {
    // A Stop, a change of project or shutdown: the next step ends the driver.
    if cancel.is_cancelled() {
        return true;
    }
    let Some(host) = host.upgrade() else {
        return false;
    };
    let Ok(mut owner) = host.lock() else {
        return false;
    };
    // A freeze refused the step: nothing is recorded, and the driver waits
    // it out and runs the step again.
    if owner.review_writes_frozen() {
        return true;
    }
    let paused = owner.review_can_seal(token).is_ok()
        && owner.owner_mut().is_ok_and(|session| {
            session
                .review_source_bundle_mut(&token.project)
                .and_then(|bundle| {
                    bundle.update_review_job(token, UpdateReviewJob::Pause(error.to_string()))
                })
                .is_ok()
        });
    // JA-1 S6 C3: a Paused job is durable; a detached host may now exit.
    if paused {
        owner.exit_when_detached_work_ends();
    }
    paused
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
        // The gate comes first: a replay settles the root, and a frozen or
        // elsewhere-owned root is settled only by its owner.
        owner.review_authority(token)?;
        if let Some(sealed) = owner
            .owner_mut()?
            .review_source_bundle_mut(&token.project)?
            .sealed_review(token)?
        {
            return Ok(SealResult::Sealed(Box::new(sealed)));
        }
        owner.review_can_seal(token)?;
        #[cfg(test)]
        let hook = owner.review_runtime.seal_hook.clone();
        let prepared = owner
            .owner()?
            .review_source_bundle(&token.project)?
            .prepare_review_artifact(token)?;
        #[cfg(test)]
        let prepared = (prepared, hook);
        prepared
    };
    #[cfg(test)]
    let prepared = {
        let (prepared, hook) = prepared;
        if let Some(hook) = hook {
            hook(cancel)?;
        }
        prepared
    };
    let verified = prepared.verify_for_seal(cancel)?;
    cancelled(cancel)?;
    let mut owner = lock(host)?;
    // A seal that raced this one, a lost reply's or the driver's, already
    // published the receipt: replay it, behind the same gate.
    owner.review_authority(token)?;
    if let Some(sealed) = owner
        .owner_mut()?
        .review_source_bundle_mut(&token.project)?
        .sealed_review(token)?
    {
        return Ok(SealResult::Sealed(Box::new(sealed)));
    }
    owner.review_can_seal(token)?;
    let result = owner
        .owner_mut()?
        .review_source_bundle_mut(&token.project)?
        .seal_review(token, command, expected_configuration_sha256, &verified)?;
    if matches!(result, SealResult::Sealed(_)) {
        owner.release_sealed_review(token);
        // JA-1 S6 C3: the seal ended the review; a detached host may exit.
        owner.exit_when_detached_work_ends();
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
        Ok(bundle.reviewer_attempt_on_cut(&checkpoint, attempt)?)
    }

    pub(super) fn require_reviewer_of_cut(
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
