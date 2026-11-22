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

/// A solid background under one plain title.
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
    run: RunId,
    parent: ExecutionIdentity,
    director: C4Mcp,
}

#[cfg(target_os = "linux")]
fn c7_job(doc: Document) -> C7Job {
    c7_job_on(Fixture::with_inputs(
        false,
        doc,
        "project.kinewright",
        s5_c6_admit_embedded_font,
    ))
}

#[cfg(target_os = "linux")]
fn c7_job_on(f: Fixture) -> C7Job {
    c7_job_driven(f, false)
}

/// `auto`: the production driver starts on entry to Checking.
#[cfg(target_os = "linux")]
fn c7_job_driven(f: Fixture, auto: bool) -> C7Job {
    c7_job_with(f, auto, s5_c6_start_and_finish_encode)
}

/// `encode` queues the version and returns once it is checking.
#[cfg(target_os = "linux")]
fn c7_job_with(
    mut f: Fixture,
    auto: bool,
    encode: fn(&Fixture, &kinewright_project::studio_records::SnapshotKey) -> ReviewToken,
) -> C7Job {
    let (run, parent, director) = c4_director(&f);
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
    if auto {
        c7_auto_drive(&f, true);
    }
    let token = encode(&f, &key);
    C7Job {
        f,
        token,
        attempt,
        reviewer,
        run,
        parent,
        director,
    }
}

#[cfg(target_os = "linux")]
fn c7_auto_drive(f: &Fixture, auto: bool) {
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .review_fixture_auto_drive(auto);
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
    // The default review encodes 1920x1080 (JA-1 S6 R1); the bound config
    // bounds each still of it, not the host-wide 64.
    let width = kinewright_media::bounded_still_dimensions(
        kinewright_core::OUTPUT_REVIEW_DEFAULT_DELIVERY.resolution((480, 270)),
        ReviewStillsConfig::default().max_long_edge,
    )
    .0;
    let mut reviewer = C7Reviewer(|_: &ReviewToken| {
        let stills = c7_judge(&job.reviewer);
        assert!(
            stills
                .iter()
                .all(|still| still.stills_config_sha256 == bound && still.width == width),
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
        let (stills, _, _) = s5_c6_read_all_stills(&job.reviewer, 4);
        assert!(!stills.is_empty(), "C7_STILLS_SERVED_BEFORE_STOP");
        let stopped = s5_review_call(&job.f.editor, ReviewControl::Stop(token.clone())).unwrap();
        assert!(
            !matches!(stopped.phase, ReviewPhase::Checking(_)),
            "C7_STOPPED: {:?}",
            stopped.phase
        );
        // Real still IDs served under the binding: refused by the fence,
        // not by an empty list.
        let late = s5_refusal(
            &job.reviewer,
            "submit_visual_judgment",
            serde_json::json!({
                "verdict": "pass",
                "reviewed_still_ids": stills.iter().map(|still| still.still_id.clone()).collect::<Vec<_>>(),
                "findings": []
            }),
            "No active output review job is checking.",
        );
        assert!(
            late.is_none(),
            "C7_LATE_JUDGMENT_REFUSED_BY_THE_FENCE: {late:?}"
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

/// Which stored transcript a talking-head witness has.
#[cfg(target_os = "linux")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum C7Transcript {
    Kept,
    Missing,
    OtherStream,
}

/// A plain talking head: the generated source cut whole, captioned "So
/// today" over its first two words, with its source-clock transcript stored
/// (or not) in the host's media store before the review is queued.
#[cfg(target_os = "linux")]
fn c7_talking_head_job(transcript: C7Transcript) -> (C7Job, String) {
    let (f, name) = c7_talking_head_fixture(transcript);
    (
        c7_job_with(f, false, s6_c1_start_default_encode_at_source_raster),
        name,
    )
}

/// The talking-head fixture of [`c7_talking_head_job`], saved, with its
/// candidate branch bound, before any review job exists.
#[cfg(target_os = "linux")]
fn c7_talking_head_fixture(transcript: C7Transcript) -> (Fixture, String) {
    use kinewright_core::{CaptionPreset, Rational, TimeCode, Track, TrackId, TrackKind};
    use kinewright_media::test_support::talking_head;
    let doc = Document {
        fps: Rational::new(talking_head::FPS, 1).unwrap(),
        resolution: (
            u32::try_from(talking_head::WIDTH).unwrap(),
            u32::try_from(talking_head::HEIGHT).unwrap(),
        ),
        duration: TimeCode(45),
        tracks: vec![
            Track {
                id: TrackId(1),
                name: Some("Speaker".into()),
                kind: TrackKind::Video,
                sync_lock: false,
                clips: vec![],
            },
            Track {
                id: TrackId(2),
                name: Some("Captions".into()),
                kind: TrackKind::Video,
                sync_lock: false,
                clips: vec![s5_c6_title_clip(
                    1,
                    0,
                    45,
                    CaptionPreset::Clean.title("So today"),
                    vec![],
                )],
            },
        ],
        ..Document::default()
    };
    doc.validate().unwrap();
    let mut f = Fixture::with_inputs(false, doc, "project.kinewright", s5_c6_admit_embedded_font);
    let path = f.temp.path("talking_head.mp4");
    talking_head::write(&path);
    let name = {
        let shared = f.host.owner.upgrade().unwrap();
        let mut host = shared.lock().unwrap();
        let (asset, _) = host.owner_mut().unwrap().import_owned_media(&path).unwrap();
        let frames = i64::try_from(talking_head::FRAMES).unwrap();
        assert_eq!(asset.duration, TimeCode(frames), "C7_TALKING_HEAD_FRAMES");
        host.core()
            .request(kinewright_core::Command::Do(
                kinewright_core::Operation::AddClip {
                    track: TrackId(1),
                    asset: asset.id,
                    at: TimeCode::ZERO,
                    source: TimeCode::ZERO..TimeCode(frames),
                },
            ))
            .unwrap();
        let manifest = host
            .owner()
            .unwrap()
            .store()
            .unwrap()
            .read_input_manifest(host.owner().unwrap().current_inputs_ref())
            .unwrap();
        let selected = manifest
            .entries
            .iter()
            .find(|entry| entry.matches_media_asset(&asset))
            .and_then(|entry| entry.media_interpretation().ok())
            .and_then(|interpretation| interpretation.audio)
            .expect("C7_TALKING_HEAD_AUDIO")
            .index;
        let data_dir = host.config.data_dir.clone().expect("C7_PRIVATE_DATA_DIR");
        let words: Vec<_> = talking_head::WORDS
            .iter()
            .map(|&(text, start, end)| kinewright_core::dialogue::Word {
                text: text.into(),
                start_micros: start,
                end_micros: end,
            })
            .collect();
        let stream = match transcript {
            C7Transcript::Kept => Some(selected),
            C7Transcript::Missing => None,
            C7Transcript::OtherStream => Some(selected + 1),
        };
        if let Some(stream) = stream {
            kinewright_media::test_store_source_transcript(
                &data_dir,
                asset.source_fingerprint.content_sha256.as_deref().unwrap(),
                asset.fps,
                asset.duration,
                stream,
                &words,
            )
            .unwrap();
        }
        asset.name
    };
    ok(
        f.editor
            .request(HostCommand::Save {
                snapshot: host_snapshot_for_test(&f.host),
            })
            .unwrap(),
        "C7_SAVE_TALKING_HEAD",
    );
    {
        let shared = f.host.owner.upgrade().unwrap();
        let mut host = shared.lock().unwrap();
        let owner = host.owner_mut().unwrap();
        let mut candidate = owner
            .fork_durable_branch(
                "talking-head",
                kinewright_project::editing_session::DurableBranchRole::RunCandidate,
            )
            .unwrap();
        owner.persist_durable_branch(&mut candidate).unwrap();
        f.brief.inputs = owner.current_inputs_ref().clone();
        f.brief.scope.base = owner.candidate_binding(candidate.branch_id()).unwrap();
    }
    (f, name)
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_talking_head_with_its_transcript_seals_delivered() {
    use kinewright_core::output_review::SealedReviewOutcome;
    use kinewright_project::review_job::SealResult;
    let (job, _) = c7_talking_head_job(C7Transcript::Kept);
    let mut reviewer = C7Reviewer(|_: &ReviewToken| {
        c7_judge(&job.reviewer);
        Ok(())
    });
    let cancel = kinewright_core::ExportCancellation::default();
    let result = c7_drive(&job, &c7_real_check, &mut reviewer, &cancel).expect("C7_DRIVER_SEALS");
    let SealResult::Sealed(sealed) = result else {
        panic!("C7_TALKING_HEAD_SEALED: {result:?}");
    };
    assert_eq!(
        sealed.receipt.outcome,
        SealedReviewOutcome::Delivered,
        "C7_TALKING_HEAD_DELIVERED: {:?}",
        sealed.receipt.outcome
    );
}

#[cfg(target_os = "linux")]
fn c7_assert_speech_unchecked(transcript: C7Transcript, label: &str) {
    use kinewright_core::output_review::{RequiredReviewRow, ReviewRowResult};
    use kinewright_project::review_job::SealResult;
    let (job, name) = c7_talking_head_job(transcript);
    let mut reviewer = C7Reviewer(|_: &ReviewToken| {
        c7_judge(&job.reviewer);
        Ok(())
    });
    let cancel = kinewright_core::ExportCancellation::default();
    let result = c7_drive(&job, &c7_real_check, &mut reviewer, &cancel).expect("C7_DRIVER_RUNS");
    let SealResult::Paused(status) = result else {
        panic!("{label}_PAUSED: {result:?}");
    };
    let expected = format!("Transcribe \"{name}\" to check its speech.");
    let ReviewPhase::Paused {
        findings,
        checkpoint: kinewright_project::review_job::ReviewCheckpoint::Checking(checks),
        ..
    } = &status.phase
    else {
        panic!("{label}_PAUSED_PHASE: {:?}", status.phase);
    };
    assert!(
        findings.iter().any(
            |finding| finding.row == Some(RequiredReviewRow::RetainedSpeechChecks)
                && finding.note == expected
        ),
        "{label}_NAMES_THE_SOURCE: {findings:?}"
    );
    let row = checks
        .rows
        .get(&RequiredReviewRow::RetainedSpeechChecks)
        .expect("C7_SPEECH_ROW_RECORDED");
    assert_eq!(row.result, ReviewRowResult::Unknown, "{label}_ROW_UNKNOWN");
    assert_eq!(
        row.reason.as_deref(),
        Some(expected.as_str()),
        "{label}_ROW_REASON"
    );
    assert!(c7_receipts(&job).is_empty(), "{label}_NOT_SEALED");
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_talking_head_without_a_transcript_pauses_speech_unknown() {
    c7_assert_speech_unchecked(C7Transcript::Missing, "C7_MISSING_TRANSCRIPT");
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_a_transcript_of_another_stream_is_refused() {
    c7_assert_speech_unchecked(C7Transcript::OtherStream, "C7_OTHER_STREAM");
}

// JA-1 S6 commit 1: loudness is never skipped (R1).

/// Queue `key` with explicit source-master settings, which carry no loudness
/// target, and return once it is checking.
#[cfg(target_os = "linux")]
fn s6_c1_start_untargeted_encode(
    f: &Fixture,
    key: &kinewright_project::studio_records::SnapshotKey,
) -> ReviewToken {
    s6_c1_start_encode_with(f, key, |document| {
        let settings = kinewright_core::DeliveryProfile::SourceMaster.export_settings(
            document,
            kinewright_core::DeliveryEncodeDepth::Eight,
            kinewright_core::ExportCancellation::default(),
        );
        assert_eq!(settings.loudness_normalization, None);
        settings
    })
}

/// Queue `key` with the default review settings, loudness target included,
/// at the document's own raster: a debug 1080p encode of the talking head
/// outlasts the encode guard under CI load, and those tests are about
/// transcripts, not the delivery raster.
#[cfg(target_os = "linux")]
fn s6_c1_start_default_encode_at_source_raster(
    f: &Fixture,
    key: &kinewright_project::studio_records::SnapshotKey,
) -> ReviewToken {
    s6_c1_start_encode_with(f, key, |document| {
        let mut settings = kinewright_core::output_review_default_settings(
            document,
            kinewright_core::ExportCancellation::default(),
        );
        settings.resolution = document.resolution;
        settings
    })
}

#[cfg(target_os = "linux")]
fn s6_c1_start_encode_with(
    f: &Fixture,
    key: &kinewright_project::studio_records::SnapshotKey,
    settings: fn(&Document) -> kinewright_core::ExportSettings,
) -> ReviewToken {
    let settings = {
        let shared = f.host.owner.upgrade().unwrap();
        let owner = shared.lock().unwrap();
        let document = owner
            .owner()
            .unwrap()
            .read_snapshot(key)
            .unwrap()
            .open_review_inputs()
            .unwrap()
            .document;
        settings(&document)
    };
    let queued = s5_review_call(
        &f.editor,
        ReviewControl::Queue {
            snapshot: key.clone(),
            settings,
            byte_limit: 64 << 20,
        },
    )
    .unwrap();
    assert_eq!(queued.phase, ReviewPhase::Queued);
    s5_review_call(&f.editor, ReviewControl::Resume(queued.token)).unwrap();
    let status = s5_review_wait(&f.editor, |phase| {
        matches!(
            phase,
            ReviewPhase::Checking(_) | ReviewPhase::Paused { .. } | ReviewPhase::Stopped
        )
    });
    assert!(
        matches!(status.phase, ReviewPhase::Checking(_)),
        "S6_C1_QUEUED_CHECKING: {:?}",
        status.phase
    );
    let shared = f.host.owner.upgrade().unwrap();
    s5_review_until(
        || {
            shared
                .lock()
                .unwrap()
                .review_fixture_worker()
                .is_none_or(|(_, done)| done)
        },
        "S6_C1_QUEUED_WORKER_DONE",
    );
    // A finished worker still holds the staging until it is retired, and
    // the seal refuses while it is held. Production retires it in the
    // driver's step; `c7_drive` skips that step, so retire it here the same
    // way, under the lock.
    {
        let mut owner = shared.lock().unwrap();
        owner.retire_review_worker().unwrap();
        assert!(
            owner.review_fixture_worker().is_none(),
            "S6_C1_QUEUED_WORKER_RETIRED"
        );
        let job = owner
            .owner()
            .unwrap()
            .review_bundle()
            .unwrap()
            .review_job()
            .unwrap()
            .expect("S6_C1_QUEUED_JOB");
        assert!(job.stage.is_none(), "S6_C1_QUEUED_STAGE_RETIRED");
    }
    status.token
}

/// The CI pause of the talking-head witness: the encode worker finishes after
/// the helper's last Status, so nothing retired it and the seal found
/// "Review staging is still held by the encode." Holding the worker after
/// publication puts that ordering on every run; the helper must retire it.
#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s6_c2_fix2_a_late_worker_is_retired_before_the_seal() {
    use kinewright_project::review_job::SealResult;
    let f = Fixture::with_inputs(
        false,
        c7_title_doc("Hello world"),
        "project.kinewright",
        s5_c6_admit_embedded_font,
    );
    // Status sees Checking at once; the thread returns a second later.
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .review_fixture_probe(|at| {
            if matches!(at, ReviewPoint::AfterPublication) {
                std::thread::sleep(Duration::from_secs(1));
            }
        });
    let job = c7_job_with(f, false, s6_c1_start_default_encode_at_source_raster);
    let mut reviewer = C7Reviewer(|_: &ReviewToken| {
        c7_judge(&job.reviewer);
        Ok(())
    });
    let cancel = kinewright_core::ExportCancellation::default();
    let result =
        c7_drive(&job, &c7_real_check, &mut reviewer, &cancel).expect("S6C2F2_DRIVER_RUNS");
    assert!(
        matches!(result, SealResult::Sealed(_)),
        "S6C2F2_LATE_WORKER_SEALS: {result:?}"
    );
}

/// The default queue freezes the named −14 LUFS streaming target into the
/// settings the checks judge, and a plain cut still seals Delivered.
#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s6_c1_default_queue_judges_loudness_against_the_streaming_target() {
    use kinewright_core::output_review::SealedReviewOutcome;
    use kinewright_project::review_job::SealResult;
    let job = c7_job(c7_title_doc("Hello world"));
    let mut reviewer = C7Reviewer(|_: &ReviewToken| {
        c7_judge(&job.reviewer);
        Ok(())
    });
    let seen = std::cell::Cell::new(None);
    let check = |file: &std::fs::File,
                 inputs: &kinewright_media::review_scan::ReviewScanInputs<'_>| {
        seen.set(Some(inputs.settings.loudness_normalization));
        c7_real_check(file, inputs)
    };
    let cancel = kinewright_core::ExportCancellation::default();
    let SealResult::Sealed(sealed) =
        c7_drive(&job, &check, &mut reviewer, &cancel).expect("S6_C1_DRIVER_SEALS")
    else {
        panic!("S6_C1_DEFAULT_SEALED: {:?}", c7_job_phase(&job));
    };
    assert_eq!(
        seen.get(),
        Some(Some(kinewright_core::STREAMING_PLATFORM_TARGET)),
        "S6_C1_DEFAULT_TARGET_FROZEN"
    );
    assert_eq!(
        kinewright_core::OUTPUT_REVIEW_DEFAULT_DELIVERY,
        kinewright_core::DeliveryProfile::Youtube1080p
    );
    assert_eq!(sealed.receipt.outcome, SealedReviewOutcome::Delivered);
}

/// An explicit queue whose settings carry no loudness target pauses with the
/// loudness row Unknown, its reason naming the missing target, and no seal.
#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s6_c1_a_queue_without_a_loudness_target_pauses_loudness_unknown() {
    use kinewright_core::output_review::{RequiredReviewRow, ReviewRowResult};
    use kinewright_project::review_job::SealResult;
    let job = c7_job_with(
        Fixture::with_inputs(
            false,
            c7_title_doc("Hello world"),
            "project.kinewright",
            s5_c6_admit_embedded_font,
        ),
        false,
        s6_c1_start_untargeted_encode,
    );
    let mut reviewer = C7Reviewer(|_: &ReviewToken| {
        c7_judge(&job.reviewer);
        Ok(())
    });
    let cancel = kinewright_core::ExportCancellation::default();
    let result = c7_drive(&job, &c7_real_check, &mut reviewer, &cancel).expect("S6_C1_DRIVER_RUNS");
    let SealResult::Paused(status) = result else {
        panic!("S6_C1_UNTARGETED_PAUSED: {result:?}");
    };
    let ReviewPhase::Paused {
        findings,
        checkpoint: kinewright_project::review_job::ReviewCheckpoint::Checking(checks),
        ..
    } = &status.phase
    else {
        panic!("S6_C1_PAUSED_PHASE: {:?}", status.phase);
    };
    let expected = super::super::review_job::review_checks::NO_LOUDNESS_TARGET;
    assert!(expected.starts_with("No loudness target"));
    let row = checks
        .rows
        .get(&RequiredReviewRow::LoudnessAndPeak)
        .expect("S6_C1_LOUDNESS_ROW_RECORDED");
    assert_eq!(row.result, ReviewRowResult::Unknown, "S6_C1_ROW_UNKNOWN");
    assert_eq!(row.reason.as_deref(), Some(expected), "S6_C1_ROW_REASON");
    // The paused finding keeps the reason's first 64 characters.
    assert!(
        findings.iter().any(
            |finding| finding.row == Some(RequiredReviewRow::LoudnessAndPeak)
                && finding.note.starts_with("No loudness target")
                && expected.starts_with(finding.note.as_str())
        ),
        "S6_C1_NAMES_THE_TARGET: {findings:?}"
    );
    for row in [
        RequiredReviewRow::StructureAndAvIntegrity,
        RequiredReviewRow::TechnicalDefectScan,
        RequiredReviewRow::RetainedSpeechChecks,
    ] {
        assert_eq!(
            checks.rows[&row].result,
            ReviewRowResult::Pass,
            "S6_C1_OTHER_ROWS_PASS {row:?}"
        );
    }
    assert!(c7_receipts(&job).is_empty(), "S6_C1_NOT_SEALED");
}
