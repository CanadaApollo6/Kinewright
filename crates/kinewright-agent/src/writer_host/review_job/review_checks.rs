//! JA-1 S5a commit 5: run the technical scan and retained-speech checks on a
//! published review artifact and record their rows.
//!
//! The checks read only the verified artifact and the records frozen with its
//! request. The writer lock is held only to bind, to open those inputs and to
//! record results; decoding and recognition run with it released. Results
//! reach the job only through `record_review_check`.
use std::{fs::File, io::Seek, sync::Mutex};

use kinewright_core::{ExportCancellation, MediaError, output_review::ReviewRowResult};
use kinewright_media::{
    ReviewOutputTiming,
    review_scan::{
        RetainedSpeechConfig, RetainedSpeechReport, ReviewRecognition, ReviewScanInputs,
        TechnicalReviewReport, TechnicalScanConfig, check_retained_speech, scan_review_artifact,
    },
};
use kinewright_project::{
    bundle_records::SessionId,
    review_job::{
        ReviewCheckBinding, ReviewCheckInputs, ReviewStatus, ReviewToken, UpdateReviewJob,
    },
};

use super::{HostCommandOwner, HostError, refused};

/// The checker configuration commit 5 binds: the scan and speech digests.
#[must_use]
pub(in crate::writer_host) fn review_check_configuration(
    scan: &TechnicalScanConfig,
    speech: &RetainedSpeechConfig,
) -> [u8; 32] {
    let mut digest = kinewright_hash::Sha256::new();
    digest.update(b"kinewright.review-checks.s5a-c5\0");
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
    pub technical: TechnicalReviewReport,
    pub speech: RetainedSpeechReport,
    pub status: ReviewStatus,
}

/// Recognise the review audio. Production passes the verified English model
/// through `recognize_review_audio`; tests pass a deterministic recogniser.
pub(in crate::writer_host) type RecognizeReview<'a> = dyn FnOnce(
        File,
        &ReviewOutputTiming,
        &RetainedSpeechConfig,
        &ExportCancellation,
    ) -> Result<ReviewRecognition, MediaError>
    + 'a;

/// Bind `attempt`, run both checks with the lock released, then record every
/// decided row the profile requires.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "commit 7 starts the checks after publication")
)]
pub(in crate::writer_host) fn run_technical_review_checks(
    owner: &Mutex<HostCommandOwner>,
    token: &ReviewToken,
    attempt: SessionId,
    configs: (&TechnicalScanConfig, &RetainedSpeechConfig),
    recognize: Box<RecognizeReview<'_>>,
    cancel: &ExportCancellation,
) -> Result<ReviewCheckRun, HostError> {
    let (scan, speech) = configs;
    let binding = ReviewCheckBinding {
        configuration_sha256: review_check_configuration(scan, speech),
        reviewer_attempt: attempt,
    };
    let inputs = {
        let mut host = owner
            .lock()
            .map_err(|_| refused("Writer host lock is poisoned."))?;
        host.record_review_check(token, UpdateReviewJob::BindChecks(binding.clone()))?;
        host.owner()?
            .review_source_bundle(&token.project)?
            .open_review_check_inputs(token)?
    };
    let (technical, speech_report) = run_checks(&inputs, scan, speech, recognize, cancel)
        .map_err(|error| refused(&error.to_string()))?;
    let required = inputs.request.profile.required_rows();
    let decided = technical
        .rows()
        .map(|row| (row.row, row.result))
        .into_iter()
        .chain([(speech_report.row.row, speech_report.row.result)])
        .filter(|&(row, result)| result != ReviewRowResult::Unrun && required.contains(&row));
    let mut host = owner
        .lock()
        .map_err(|_| refused("Writer host lock is poisoned."))?;
    let mut status = None;
    for (row, result) in decided {
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
        technical,
        speech: speech_report,
        status: status.ok_or_else(|| refused("No required check row was decided."))?,
    })
}

fn run_checks(
    inputs: &ReviewCheckInputs,
    scan: &TechnicalScanConfig,
    speech: &RetainedSpeechConfig,
    recognize: Box<RecognizeReview<'_>>,
    cancel: &ExportCancellation,
) -> Result<(TechnicalReviewReport, RetainedSpeechReport), MediaError> {
    let timing = &inputs.artifact.output;
    let expectations = &inputs.expectations.0;
    // Each pass reads its own descriptor of the verified file from offset 0.
    let reopen = || -> Result<File, MediaError> {
        let mut file = inputs.media.try_clone().map_err(io_error)?;
        file.rewind().map_err(io_error)?;
        Ok(file)
    };
    let technical = scan_review_artifact(
        reopen()?,
        &ReviewScanInputs {
            timing,
            expectations,
            document: &inputs.document,
            settings: &inputs.request.settings,
        },
        scan,
        cancel,
    )?;
    let recognized = recognize(reopen()?, timing, speech, cancel)?;
    let speech_report =
        check_retained_speech(reopen()?, timing, expectations, &recognized, speech, cancel)?;
    Ok((technical, speech_report))
}

#[allow(clippy::needless_pass_by_value, reason = "used as a map_err adapter")]
fn io_error(error: std::io::Error) -> MediaError {
    MediaError::Backend(format!("review artifact read failed: {error}"))
}
