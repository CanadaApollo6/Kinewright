//! JA-1 S5a commit 5: run the technical and retained-speech checks on a
//! published review artifact and record their rows.
//!
//! The checks read only the verified artifact, the records frozen with its
//! request and the pinned inputs of the reviewed version. The writer lock is
//! held only to bind and to record (L13): verifying the artifact, opening the
//! frozen inputs, decoding and comparing all run with it released. Every
//! row this commit decides is recorded, even when the check could not run;
//! an error makes the row Unknown. Results reach the job only through
//! `record_review_check`.
use std::{fs::File, sync::Arc, sync::Mutex};

use kinewright_core::{
    ExportCancellation, MediaError,
    output_review::{RequiredReviewRow, ReviewRowResult},
};
use kinewright_media::review_scan::{
    RetainedSpeechConfig, ReviewCheckReport, ReviewScanInputs, TechnicalScanConfig,
    check_review_artifact,
};
use kinewright_project::review_job::{
    ReviewCheckBinding, ReviewStatus, ReviewToken, UpdateReviewJob,
};

use super::{HostCommandOwner, HostError, refused};

/// The rows commit 5 decides.
pub(in crate::writer_host) const COMMIT_5_ROWS: [RequiredReviewRow; 4] = [
    RequiredReviewRow::StructureAndAvIntegrity,
    RequiredReviewRow::LoudnessAndPeak,
    RequiredReviewRow::TechnicalDefectScan,
    RequiredReviewRow::RetainedSpeechChecks,
];

/// Digest of commit 5's own checker configuration. The caller combines it
/// with the other commits' configurations into the one binding it passes in.
#[must_use]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "commit 7 builds the binding it passes in")
)]
pub(in crate::writer_host) fn review_check_configuration(
    scan: &TechnicalScanConfig,
    speech: &RetainedSpeechConfig,
) -> [u8; 32] {
    let mut digest = kinewright_hash::Sha256::new();
    digest.update(b"kinewright.review-checks.s5a-c5.v2\0");
    digest.update(&scan.sha256());
    digest.update(&speech.sha256());
    digest.finalize()
}

/// What the checks decided, beside the row results they recorded.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "commit 7 keeps the reports as evidence")
)]
pub(in crate::writer_host) struct ReviewCheckRun {
    /// The report, or why no report could be made (every row then Unknown).
    pub report: Result<ReviewCheckReport, String>,
    pub status: ReviewStatus,
}

/// The check itself, given the verified artifact and the frozen inputs.
/// Production passes [`check_review_artifact`] with fixed configurations.
pub(in crate::writer_host) type CheckReview<'a> =
    dyn Fn(&File, &ReviewScanInputs<'_>) -> Result<ReviewCheckReport, MediaError> + 'a;

/// The production check of commit 5.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "commit 7 starts the checks after publication")
)]
pub(in crate::writer_host) fn commit_5_check<'a>(
    scan: &'a TechnicalScanConfig,
    speech: &'a RetainedSpeechConfig,
    cancel: &'a ExportCancellation,
) -> impl Fn(&File, &ReviewScanInputs<'_>) -> Result<ReviewCheckReport, MediaError> + 'a {
    move |artifact, inputs| check_review_artifact(artifact, inputs, scan, speech, cancel)
}

/// Bind `binding` under the lock, verify and check with the lock released,
/// then record every commit-5 row the profile requires.
///
/// # Errors
/// Binding or recording is refused (another binding, a stopped job), or the
/// check was cancelled. Nothing is recorded after a cancellation.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "commit 7 starts the checks after publication")
)]
pub(in crate::writer_host) fn run_technical_review_checks(
    owner: &Mutex<HostCommandOwner>,
    token: &ReviewToken,
    binding: &ReviewCheckBinding,
    check: &CheckReview<'_>,
) -> Result<ReviewCheckRun, HostError> {
    let pending = {
        let mut host = owner
            .lock()
            .map_err(|_| refused("Writer host lock is poisoned."))?;
        host.record_review_check(token, UpdateReviewJob::BindChecks(binding.clone()))?;
        host.owner()?
            .review_source_bundle(&token.project)?
            .prepare_review_check(token)?
    };
    let required = pending.required_rows();
    let report = match pending.open() {
        Ok(inputs) => run_check(inputs, check),
        Err(error) => Err(error.to_string()),
    };
    if let Err(reason) = &report
        && reason == &MediaError::Cancelled.to_string()
    {
        return Err(refused("The review checks were cancelled."));
    }
    let rows = COMMIT_5_ROWS
        .into_iter()
        .filter(|row| required.contains(row))
        .map(|row| {
            let result = report.as_ref().map_or(ReviewRowResult::Unknown, |report| {
                report
                    .rows()
                    .into_iter()
                    .find(|candidate| candidate.row == row)
                    .map_or(ReviewRowResult::Unknown, |candidate| candidate.result)
            });
            (row, result)
        });
    let mut host = owner
        .lock()
        .map_err(|_| refused("Writer host lock is poisoned."))?;
    let mut status = None;
    for (row, result) in rows {
        status = Some(host.record_review_check(
            token,
            UpdateReviewJob::RecordCheck {
                binding: binding.clone(),
                row,
                result,
            },
        )?);
    }
    Ok(ReviewCheckRun {
        report,
        status: status.ok_or_else(|| refused("The profile requires no commit 5 row."))?,
    })
}

/// Run the check on opened inputs. Any failure becomes the reason every row
/// is Unknown; cancellation is passed through by its message.
fn run_check(
    inputs: kinewright_project::review_job::ReviewCheckInputs,
    check: &CheckReview<'_>,
) -> Result<ReviewCheckReport, String> {
    let kinewright_project::review_job::ReviewCheckInputs {
        artifact,
        media,
        request,
        expectations,
        context,
    } = inputs;
    let context = Arc::new(context);
    let sources = context
        .playback_inputs()
        .map_err(|error| error.to_string())?;
    check(
        &media,
        &ReviewScanInputs {
            timing: &artifact.output,
            expectations: &expectations.0,
            document: &context.document,
            settings: &request.settings,
            sources: sources.as_ref(),
        },
    )
    .map_err(|error| error.to_string())
}
