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
use std::{collections::BTreeSet, fmt::Write as _, fs::File, sync::Arc, sync::Mutex};

use kinewright_core::{
    ExportCancellation, MediaError,
    output_review::{RequiredReviewRow, ReviewExpectations},
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
    expect(
        dead_code,
        reason = "the driver keeps only the recorded rows; tests read the report"
    )
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
    let (report, untranscribed) = match pending.open() {
        Ok(inputs) => run_check(inputs, check),
        Err(error) => (Err(error.to_string()), None),
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
            // Speech without a bound transcript is not checked (L2), whatever
            // the scan concluded from the words it had.
            if row == RequiredReviewRow::RetainedSpeechChecks
                && report.is_ok()
                && let Some(reason) = &untranscribed
            {
                return (row, Err(reason.as_str()));
            }
            let result = report.as_ref().map_err(String::as_str).and_then(|report| {
                report
                    .rows()
                    .into_iter()
                    .find(|candidate| candidate.row == row)
                    .map(|candidate| candidate.result)
                    .ok_or("The check reported no result for this row.")
            });
            (row, result)
        });
    let mut host = owner
        .lock()
        .map_err(|_| refused("Writer host lock is poisoned."))?;
    let mut status = None;
    for (row, result) in rows {
        let update = match result {
            Ok(result) => UpdateReviewJob::RecordCheck {
                binding: binding.clone(),
                row,
                result,
            },
            Err(reason) => UpdateReviewJob::RecordUnknown {
                binding: binding.clone(),
                row,
                reason: check_reason(reason),
            },
        };
        status = Some(host.record_review_check(token, update)?);
    }
    Ok(ReviewCheckRun {
        report,
        status: status.ok_or_else(|| refused("The profile requires no commit 5 row."))?,
    })
}

/// Run the check on opened inputs. Any failure becomes the reason every row
/// is Unknown; cancellation is passed through by its message. Beside the
/// report, name the sources whose kept speech no transcript covers.
fn run_check(
    inputs: kinewright_project::review_job::ReviewCheckInputs,
    check: &CheckReview<'_>,
) -> (Result<ReviewCheckReport, String>, Option<String>) {
    let kinewright_project::review_job::ReviewCheckInputs {
        artifact,
        media,
        request,
        expectations,
        context,
    } = inputs;
    let context = Arc::new(context);
    let untranscribed = untranscribed_sources(&expectations.0, &context.document);
    let report = context
        .playback_inputs()
        .map_err(|error| error.to_string())
        .and_then(|sources| {
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
        });
    (report, untranscribed)
}

/// Which sources need transcribing before their kept speech can be checked.
fn untranscribed_sources(
    expectations: &ReviewExpectations,
    document: &kinewright_core::Document,
) -> Option<String> {
    let assets: BTreeSet<_> = expectations
        .uncovered_speech_segments
        .iter()
        .filter_map(|id| {
            expectations
                .segments
                .iter()
                .find(|segment| segment.id == *id)
                .map(|segment| segment.asset)
        })
        .collect();
    if assets.is_empty() && expectations.kept_words.is_determined() {
        return None;
    }
    let names = assets.iter().fold(String::new(), |mut names, asset| {
        if !names.is_empty() {
            names.push_str(", ");
        }
        match document.asset(*asset) {
            Some(source) => {
                let _ = write!(names, "\"{}\"", source.name);
            }
            None => {
                let _ = write!(names, "asset {asset}");
            }
        }
        names
    });
    // The seal keeps the first 64 characters of a note: lead with the source.
    Some(match assets.len() {
        0 => "Speech not checked: a source needs transcribing.".into(),
        1 => format!("Transcribe {names} to check its speech."),
        _ => format!("Transcribe {names} to check their speech."),
    })
}

/// A checker's error, bounded to what a check row keeps.
pub(in crate::writer_host) fn check_reason(reason: &str) -> String {
    let limit = kinewright_project::review_job::MAX_REVIEW_FINDING_NOTE;
    let mut bounded = String::new();
    for character in reason.trim().chars() {
        if bounded.len() + character.len_utf8() > limit {
            break;
        }
        bounded.push(character);
    }
    if bounded.is_empty() {
        "The check failed without a reason.".into()
    } else {
        bounded
    }
}
