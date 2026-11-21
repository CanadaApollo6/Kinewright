// JA-1 S5a commit 7: one binding, one driver, then the seal. The Reviewer is
// scripted: it reads stills and submits its judgment over its own task MCP.

#[cfg(target_os = "linux")]
struct C7Reviewer<F: FnMut(&ReviewToken) -> Result<(), String>>(F);

#[cfg(target_os = "linux")]
impl<F: FnMut(&ReviewToken) -> Result<(), String>>
    super::super::review_job::review_driver::ReviewerSession for C7Reviewer<F>
{
    fn await_judgment(
        &mut self,
        host: &std::sync::Weak<std::sync::Mutex<HostCommandOwner>>,
        token: &ReviewToken,
        _binding: &ReviewCheckBinding,
        _cancel: &kinewright_core::ExportCancellation,
    ) -> Result<(), String> {
        // The driver waits with the writer lock released.
        assert!(
            host.upgrade().unwrap().try_lock().is_ok(),
            "C7_REVIEWER_RUNS_UNLOCKED"
        );
        (self.0)(token)
    }
}

/// A solid background under one plain title (not a caption: captions need a
/// transcript to compare against, which the production queue does not pass).
#[cfg(target_os = "linux")]
fn c7_title_doc(text: &str) -> Document {
    use kinewright_core::{
        Rational, SolidColor, TimeCode, Title, TitlePosition, Track, TrackId, TrackKind,
    };
    let title = Title {
        text: text.into(),
        font_size_token: 1,
        color_token: 0,
        position: TitlePosition::Top,
        background_scrim: true,
        fade_in_frames: TimeCode::ZERO,
        fade_out_frames: TimeCode::ZERO,
        caption_preset: None,
    };
    let doc = Document {
        fps: Rational::new(24, 1).unwrap(),
        resolution: (480, 270),
        duration: TimeCode(8),
        tracks: vec![
            Track {
                id: TrackId(1),
                name: Some("Bg".into()),
                kind: TrackKind::Video,
                sync_lock: false,
                clips: vec![s5_c6_solid_clip(
                    1,
                    0,
                    8,
                    SolidColor {
                        r: 36,
                        g: 56,
                        b: 92,
                    },
                )],
            },
            Track {
                id: TrackId(2),
                name: Some("Titles".into()),
                kind: TrackKind::Video,
                sync_lock: false,
                clips: vec![s5_c6_title_clip(2, 0, 8, title, vec![])],
            },
        ],
        ..Document::default()
    };
    doc.validate().unwrap();
    doc
}

/// A Reviewer-role child on the cut, its editorial audit recorded, and an
/// encoded review job of `doc` waiting in Checking.
#[cfg(target_os = "linux")]
struct C7Job {
    f: Fixture,
    token: ReviewToken,
    attempt: ExecutionIdentity,
    reviewer: C4Mcp,
}

#[cfg(target_os = "linux")]
fn c7_job(doc: Document) -> C7Job {
    let mut f = Fixture::with_inputs(false, doc, "project.kinewright", s5_c6_admit_embedded_font);
    let (_, parent, director) = c4_director(&f);
    let (attempt, _, reviewer) = s5_child(&f, &director, &parent, Role::Reviewer);
    holder(&mut f);
    let key = s5_c6_publish_version(&f, None, vec![]);
    s5_accepted(
        &reviewer,
        "record_editorial_audit",
        serde_json::json!({
            "version": key.version,
            "snapshot": key,
            "rulebook_ref": null,
            "entries": [
                {"line_or_beat": "Opening", "output": {"start_micros": 0, "end_micros": 100_000},
                 "text": "It holds.", "kind": "retention", "finding": "resolved"},
                {"line_or_beat": "Hook", "output": {"start_micros": 0, "end_micros": 100_000},
                 "text": "The hook works.", "kind": "hook", "finding": "resolved"},
                {"line_or_beat": "Story", "output": {"start_micros": 0, "end_micros": 100_000},
                 "text": "The story works.", "kind": "story", "finding": "resolved"}
            ]
        }),
    );
    let token = s5_c6_start_and_finish_encode(&f, &key);
    C7Job {
        f,
        token,
        attempt,
        reviewer,
    }
}

/// The production commit-5 check with its default configurations.
#[cfg(target_os = "linux")]
fn c7_real_check(
    file: &std::fs::File,
    inputs: &kinewright_media::review_scan::ReviewScanInputs<'_>,
) -> Result<kinewright_media::review_scan::ReviewCheckReport, kinewright_core::MediaError> {
    use kinewright_media::review_scan::{RetainedSpeechConfig, TechnicalScanConfig};
    let cancel = kinewright_core::ExportCancellation::default();
    super::super::review_job::review_checks::commit_5_check(
        &TechnicalScanConfig::default(),
        &RetainedSpeechConfig::default(),
        &cancel,
    )(file, inputs)
}

/// Read every still, then submit a pass over them.
#[cfg(target_os = "linux")]
fn c7_judge(reviewer: &C4Mcp) -> Vec<super::super::review_stills::ServedReviewStill> {
    let (stills, _, _) = s5_c6_read_all_stills(reviewer, 4);
    assert!(!stills.is_empty(), "C7_STILLS_SERVED");
    let reply = reviewer.wire(
        "submit_visual_judgment",
        serde_json::json!({
            "verdict": "pass",
            "reviewed_still_ids": stills.iter().map(|still| still.still_id.clone()).collect::<Vec<_>>(),
            "findings": []
        }),
    );
    assert_eq!(
        reply.is_error,
        Some(false),
        "C7_JUDGMENT_ACCEPTED: {reply:?}"
    );
    stills
}

#[cfg(target_os = "linux")]
fn c7_drive(
    job: &C7Job,
    check: &super::super::review_job::review_checks::CheckReview<'_>,
    reviewer: &mut dyn super::super::review_job::review_driver::ReviewerSession,
    cancel: &kinewright_core::ExportCancellation,
) -> Result<kinewright_project::review_job::SealResult, HostError> {
    use super::super::review_job::review_driver::{DriverConfiguration, drive_review};
    drive_review(
        &job.f.host.owner,
        &job.token,
        &job.attempt.attempt.0,
        &kinewright_project::bundle_records::SessionId::fresh().unwrap(),
        &DriverConfiguration::production(&job.attempt.attempt.0),
        check,
        reviewer,
        cancel,
    )
}

#[cfg(target_os = "linux")]
fn c7_receipts(job: &C7Job) -> Vec<kinewright_project::review_job::ReviewReceipt> {
    let shared = job.f.host.owner.upgrade().unwrap();
    let owner = shared.lock().unwrap();
    owner
        .owner()
        .unwrap()
        .review_receipts(&job.token.snapshot)
        .unwrap()
}

#[cfg(target_os = "linux")]
fn c7_job_phase(job: &C7Job) -> Option<ReviewPhase> {
    let shared = job.f.host.owner.upgrade().unwrap();
    let owner = shared.lock().unwrap();
    owner
        .owner()
        .unwrap()
        .review_bundle()
        .unwrap()
        .review_job()
        .unwrap()
        .map(|job| job.phase.clone())
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_driver_delivers_a_plain_cut_with_the_bound_stills_config() {
    use kinewright_core::output_review::SealedReviewOutcome;
    use kinewright_media::ReviewStillsConfig;
    use kinewright_project::review_job::SealResult;
    let job = c7_job(c7_title_doc("Hello world"));
    // The host-wide stills configuration differs from the bound one.
    job.f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .set_review_stills_config(ReviewStillsConfig {
            max_long_edge: 64,
            max_pages_per_attempt: 1,
            ..ReviewStillsConfig::default()
        })
        .unwrap();
    let bound = ReviewStillsConfig::default().sha256();
    let bound = bound.iter().fold(String::new(), |mut hex, byte| {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
        hex
    });
    let mut reviewer = C7Reviewer(|_: &ReviewToken| {
        let stills = c7_judge(&job.reviewer);
        assert!(
            stills
                .iter()
                .all(|still| still.stills_config_sha256 == bound && still.width == 480),
            "C7_STILLS_USE_BOUND_CONFIG: {:?}",
            stills
                .iter()
                .map(|still| (still.width, still.height))
                .collect::<Vec<_>>()
        );
        Ok(())
    });
    let cancel = kinewright_core::ExportCancellation::default();
    let SealResult::Sealed(sealed) =
        c7_drive(&job, &c7_real_check, &mut reviewer, &cancel).expect("C7_DRIVER_SEALS")
    else {
        panic!("C7_PLAIN_CUT_SEALED: {:?}", c7_job_phase(&job));
    };
    assert_eq!(
        sealed.receipt.outcome,
        SealedReviewOutcome::Delivered,
        "C7_PLAIN_CUT_DELIVERED: {:?}",
        sealed.receipt.outcome
    );
    assert_eq!(sealed.receipt.key.reviewer_attempt, job.attempt.attempt.0);
    assert!(c7_job_phase(&job).is_none(), "C7_JOB_RETIRED");
    assert_eq!(c7_receipts(&job).len(), 1);

    // A lost reply: Seal again returns the same receipt.
    let replay = ok(
        job.f
            .editor
            .request(HostCommand::Review(ReviewControl::Seal {
                token: job.token.clone(),
                command: kinewright_project::bundle_records::SessionId::fresh().unwrap(),
            }))
            .unwrap(),
        "C7_SEAL_REPLAY",
    );
    let replayed = replay.review_receipt.expect("C7_REPLAY_RECEIPT");
    assert_eq!(
        serde_json::to_value(&replayed).unwrap(),
        serde_json::to_value(&sealed.receipt).unwrap(),
        "C7_REPLAY_SAME_RECEIPT"
    );
    assert_eq!(c7_receipts(&job).len(), 1, "C7_SEALED_ONCE");
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_driver_labels_a_digit_title_facts_not_checked() {
    use kinewright_core::output_review::{
        DeliveryException, EligibilityReason, SealedReviewOutcome,
    };
    use kinewright_project::review_job::SealResult;
    let job = c7_job(c7_title_doc("Step 1"));
    let mut reviewer = C7Reviewer(|_: &ReviewToken| {
        c7_judge(&job.reviewer);
        Ok(())
    });
    let cancel = kinewright_core::ExportCancellation::default();
    let SealResult::Sealed(sealed) =
        c7_drive(&job, &c7_real_check, &mut reviewer, &cancel).expect("C7_DRIVER_SEALS")
    else {
        panic!("C7_DIGIT_CUT_SEALED: {:?}", c7_job_phase(&job));
    };
    let SealedReviewOutcome::ReviewedFactsNotChecked { exceptions } = &sealed.receipt.outcome
    else {
        panic!("C7_DIGIT_NOT_DELIVERED: {:?}", sealed.receipt.outcome);
    };
    assert!(
        exceptions.iter().any(|exception| matches!(
            exception,
            DeliveryException::Element {
                reason: EligibilityReason::NumericCharacter,
                ..
            }
        )),
        "C7_DIGIT_ELEMENT_LISTED: {exceptions:?}"
    );
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_driver_records_the_other_rows_when_one_check_errors() {
    use kinewright_core::output_review::{RequiredReviewRow, ReviewRowResult};
    use kinewright_project::review_job::SealResult;
    let job = c7_job(c7_title_doc("Hello world"));
    let mut reviewer = C7Reviewer(|_: &ReviewToken| {
        c7_judge(&job.reviewer);
        Ok(())
    });
    let cancel = kinewright_core::ExportCancellation::default();
    let failing = |_: &std::fs::File, _: &kinewright_media::review_scan::ReviewScanInputs<'_>| {
        Err(kinewright_core::MediaError::Backend(
            "C7_DECODER_LOST".into(),
        ))
    };
    let SealResult::Paused(status) =
        c7_drive(&job, &failing, &mut reviewer, &cancel).expect("C7_DRIVER_PAUSES")
    else {
        panic!("C7_ERRORED_ROW_NOT_SEALED");
    };
    let ReviewPhase::Paused {
        checkpoint: kinewright_project::review_job::ReviewCheckpoint::Checking(checks),
        findings,
        ..
    } = &status.phase
    else {
        panic!("C7_PAUSED_CHECKING: {:?}", status.phase);
    };
    for row in [
        RequiredReviewRow::StructureAndAvIntegrity,
        RequiredReviewRow::LoudnessAndPeak,
        RequiredReviewRow::TechnicalDefectScan,
        RequiredReviewRow::RetainedSpeechChecks,
    ] {
        assert_eq!(
            checks.rows[&row].result,
            ReviewRowResult::Unknown,
            "{row:?}"
        );
        assert!(
            findings.iter().any(|finding| finding.row == Some(row)
                && finding.note.contains("C7_DECODER_LOST")),
            "C7_REASON_KEPT {row:?}: {findings:?}"
        );
    }
    for row in [
        RequiredReviewRow::OverlayVerification,
        RequiredReviewRow::IndependentVisualReview,
        RequiredReviewRow::EditorialAuditPresent,
    ] {
        assert_eq!(
            checks.rows[&row].result,
            ReviewRowResult::Pass,
            "C7_OTHER_ROWS_RECORDED {row:?}"
        );
    }
    assert!(c7_receipts(&job).is_empty(), "C7_NO_RECEIPT");
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_stop_fences_late_rows_and_annotate_is_refused() {
    let job = c7_job(c7_title_doc("Hello world"));
    // Annotate on the active job's version is refused.
    let annotate = job
        .f
        .editor
        .request(HostCommand::Review(ReviewControl::Annotate {
            snapshot: job.token.snapshot.clone(),
            fields: kinewright_project::review_job::ReviewAnnotationFields::default(),
        }))
        .unwrap();
    assert!(
        annotate.error.as_ref().is_some_and(|error| error
            .message
            .contains("must finish or stop before its annotations change")),
        "C7_ANNOTATE_REFUSED {annotate:?}"
    );
    // The reviewer is still judging when the person stops the review.
    let mut reviewer = C7Reviewer(|token: &ReviewToken| {
        let stopped = s5_review_call(&job.f.editor, ReviewControl::Stop(token.clone())).unwrap();
        assert!(
            !matches!(stopped.phase, ReviewPhase::Checking(_)),
            "C7_STOPPED: {:?}",
            stopped.phase
        );
        let late = job.reviewer.wire(
            "submit_visual_judgment",
            serde_json::json!({"verdict": "pass", "reviewed_still_ids": [], "findings": []}),
        );
        assert_eq!(
            late.is_error,
            Some(true),
            "C7_LATE_JUDGMENT_REFUSED {late:?}"
        );
        Ok(())
    });
    let cancel = kinewright_core::ExportCancellation::default();
    let refused = c7_drive(&job, &c7_real_check, &mut reviewer, &cancel)
        .expect_err("C7_STOP_FENCES_THE_DRIVER");
    // Stop cancels the driver's registered work; nothing after it records.
    assert!(
        refused
            .to_string()
            .contains("The review checks were cancelled."),
        "C7_STOP_REFUSAL: {refused}"
    );
    assert!(
        !matches!(
            c7_job_phase(&job),
            Some(ReviewPhase::Checking(_) | ReviewPhase::ReadyToSeal { .. })
        ),
        "C7_NO_ROWS_AFTER_STOP"
    );
    assert!(c7_receipts(&job).is_empty(), "C7_NO_RECEIPT_AFTER_STOP");
}
