// JA-1 S5a commit 7, fix round 1: the production driver. No witness here
// calls `drive_review`; the job reaches its seal from the host's own entry
// points and the Reviewer's own task tools.

#[cfg(target_os = "linux")]
fn c7f_fixture(doc: Document) -> Fixture {
    Fixture::with_inputs(false, doc, "project.kinewright", s5_c6_admit_embedded_font)
}

#[cfg(target_os = "linux")]
fn c7f_receipts(
    f: &Fixture,
    token: &ReviewToken,
) -> Vec<kinewright_project::review_job::ReviewReceipt> {
    let shared = f.host.owner.upgrade().unwrap();
    let owner = shared.lock().unwrap();
    owner
        .owner()
        .unwrap()
        .review_receipts(&token.snapshot)
        .unwrap()
}

#[cfg(target_os = "linux")]
fn c7f_phase(f: &Fixture) -> Option<ReviewPhase> {
    let shared = f.host.owner.upgrade().unwrap();
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

/// Restart the host; every attempt of the old process is retired. Returns the
/// fixture with a fresh Director attempt on the same run.
#[cfg(target_os = "linux")]
fn c7f_restart(
    job: C7Job,
    drive_on_open: bool,
) -> (
    Fixture,
    ReviewToken,
    ExecutionIdentity,
    C4Mcp,
    ExecutionIdentity,
) {
    let C7Job {
        f,
        token,
        attempt,
        reviewer,
        run,
        parent,
        director,
    } = job;
    drop(reviewer);
    drop(director);
    let machine = f.temp.path("fixture-machine");
    let project = f.temp.path("project.kinewright");
    if drive_on_open {
        super::super::review_job::AUTO_DRIVE_ON_OPEN
            .lock()
            .unwrap()
            .insert(project.clone());
    }
    let mut f = c4_restart(f, &machine);
    super::super::review_job::AUTO_DRIVE_ON_OPEN
        .lock()
        .unwrap()
        .remove(&project);
    holder(&mut f);
    let _ = c4_reserve_task(&f, &run, &parent.task);
    let director = C4Mcp::new(&f, &run, &parent.task);
    (f, token, parent, director, attempt)
}

/// Wait for the driver to seal, then return the one receipt.
#[cfg(target_os = "linux")]
fn c7f_await_receipt(
    f: &Fixture,
    token: &ReviewToken,
    marker: &str,
) -> kinewright_project::review_job::ReviewReceipt {
    s5_review_until(
        || {
            !c7f_receipts(f, token).is_empty()
                || matches!(c7f_phase(f), Some(ReviewPhase::Paused { .. }))
        },
        marker,
    );
    let mut receipts = c7f_receipts(f, token);
    assert_eq!(receipts.len(), 1, "{marker}: {:?}", c7f_phase(f));
    assert!(c7f_phase(f).is_none(), "{marker}_JOB_RETIRED");
    receipts.pop().unwrap()
}

/// Queue and encode, waiting on the bundle and the encode worker only.
#[cfg(target_os = "linux")]
fn c7f_encode_without_status(
    f: &Fixture,
    key: &kinewright_project::studio_records::SnapshotKey,
) -> ReviewToken {
    let token = s5_review_start(&f.editor, key);
    let shared = f.host.owner.upgrade().unwrap();
    s5_review_until(
        || {
            matches!(c7f_phase(f), Some(ReviewPhase::Checking(_)))
                && shared
                    .lock()
                    .unwrap()
                    .review_fixture_worker()
                    .is_none_or(|(_, done)| done)
        },
        "C7F_ENCODE_CHECKING",
    );
    token
}

#[cfg(target_os = "linux")]
fn c7f_driver_done(f: &Fixture) -> bool {
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .review_fixture_driver_done()
}

/// The receipt the Studio reads for the version's label.
#[cfg(target_os = "linux")]
fn c7f_studio_receipt(
    f: &Fixture,
    token: &ReviewToken,
) -> Option<kinewright_project::review_job::ReviewReceipt> {
    ok(
        f.editor
            .request(HostCommand::ReadEditorial {
                version: Some(token.snapshot.clone()),
            })
            .unwrap(),
        "C7F_READ_EDITORIAL",
    )
    .review_receipt
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_fix1_production_driver_seals_from_queue_to_label() {
    use kinewright_core::output_review::SealedReviewOutcome;
    // Queue → encode: the host starts the driver on entry to Checking.
    // The encode wait reads the bundle, never Status: Status would start the
    // driver itself and hide a missing start on entry to Checking.
    let job = c7_job_with(
        c7f_fixture(c7_title_doc("Hello world")),
        true,
        c7f_encode_without_status,
    );
    assert!(
        matches!(c7f_phase(&job.f), Some(ReviewPhase::Checking(_))),
        "C7F_CHECKING"
    );
    assert!(
        !c7f_driver_done(&job.f),
        "C7F_DRIVER_STARTS_ON_ENTRY_TO_CHECKING"
    );
    // No Check, no drive_review: the Reviewer's first stills read claims the
    // check, and its own judgment completes it.
    c7_judge(&job.reviewer);
    let receipt = c7f_await_receipt(&job.f, &job.token, "C7F_PRODUCTION_SEALED");
    assert_eq!(
        receipt.outcome,
        SealedReviewOutcome::Delivered,
        "C7F_DELIVERED"
    );
    assert_eq!(receipt.key.reviewer_attempt, job.attempt.attempt.0);
    let label = c7f_studio_receipt(&job.f, &job.token).expect("C7F_LABEL_RECEIPT");
    assert_eq!(
        serde_json::to_value(&label).unwrap(),
        serde_json::to_value(&receipt).unwrap(),
        "C7F_LABEL_IS_THE_SEALED_RECEIPT"
    );
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_fix1_reopen_during_checking_reaches_the_seal() {
    // The host stops while its driver waits for a Reviewer.
    // The reopened host verifies the artifact and resumes the driver itself.
    let (f, token, parent, director, _) = c7f_restart(
        c7_job_driven(c7f_fixture(c7_title_doc("Hello world")), true),
        true,
    );
    // No Status: Status would start a driver itself and hide a reopen that
    // never resumed one.
    s5_review_until(|| c7f2_generation(&f) == (1, true), "C7F_REOPEN_DRIVES");
    let phase = c7f_phase(&f);
    let Some(ReviewPhase::Checking(checks)) = &phase else {
        panic!("C7F_REOPENED_CHECKING: {phase:?}");
    };
    assert!(
        checks.reviewer_attempt.is_none(),
        "C7F_DEAD_ATTEMPT_UNBOUND"
    );
    // A Reviewer of this process claims the check.
    let (attempt, _, reviewer) = s5_child(&f, &director, &parent, Role::Reviewer);
    c7_judge(&reviewer);
    let receipt = c7f_await_receipt(&f, &token, "C7F_REOPEN_CHECKING_SEALED");
    assert_eq!(receipt.key.reviewer_attempt, attempt.attempt.0);
}

/// A job whose rows are all recorded and whose seal was held, after a reopen.
#[cfg(target_os = "linux")]
/// A seal hook that holds the seal until the driver is cancelled, then fails.
type C7f2SealHook = super::super::review_job::ReviewSealHook;
fn c7f2_held_seal(entered: &Arc<std::sync::atomic::AtomicBool>) -> C7f2SealHook {
    let entered = Arc::clone(entered);
    Arc::new(move |cancel: &kinewright_core::ExportCancellation| {
        entered.store(true, std::sync::atomic::Ordering::SeqCst);
        while !cancel.is_cancelled() {
            std::thread::sleep(Duration::from_millis(10));
        }
        Err(HostRefusal::new("review_refused", "C7F_SEAL_HELD").into())
    })
}

fn c7f_ready_then_reopen(
    drive_on_open: bool,
) -> (
    Fixture,
    ReviewToken,
    kinewright_project::bundle_records::SessionId,
) {
    let job = c7_job_driven(c7f_fixture(c7_title_doc("Hello world")), true);
    let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
    job.f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .review_runtime
        .seal_hook = Some(c7f2_held_seal(&entered));
    c7_judge(&job.reviewer);
    s5_review_until(
        || {
            matches!(c7f_phase(&job.f), Some(ReviewPhase::ReadyToSeal { .. }))
                && entered.load(std::sync::atomic::Ordering::SeqCst)
        },
        "C7F_READY_TO_SEAL_HELD",
    );
    let (f, token, _, _, attempt) = c7f_restart(job, drive_on_open);
    if !drive_on_open {
        assert!(
            matches!(c7f_phase(&f), Some(ReviewPhase::ReadyToSeal { .. })),
            "C7F_REOPENED_READY_TO_SEAL"
        );
    }
    (f, token, attempt.attempt.0)
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_fix1_reopen_during_ready_to_seal_reaches_the_seal() {
    // No command after the reopen: the host seals on its own.
    let (f, token, attempt) = c7f_ready_then_reopen(true);
    let receipt = c7f_await_receipt(&f, &token, "C7F_REOPEN_READY_SEALED");
    assert_eq!(receipt.key.reviewer_attempt, attempt);
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_fix1_seal_straight_after_reopen() {
    // No Status, no Resume and no driver: Seal verifies the retained artifact.
    let (f, token, _) = c7f_ready_then_reopen(false);
    let reply = f
        .editor
        .request(HostCommand::Review(ReviewControl::Seal {
            token: token.clone(),
            command: kinewright_project::bundle_records::SessionId::fresh().unwrap(),
        }))
        .unwrap();
    assert!(
        reply.error.is_none(),
        "C7F_SEAL_AFTER_REOPEN: {:?}",
        reply.error
    );
    assert!(
        reply.review_receipt.is_some(),
        "C7F_SEAL_AFTER_REOPEN_RECEIPT"
    );
    assert_eq!(c7f_receipts(&f, &token).len(), 1);
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_fix1_check_straight_after_reopen_binds_and_seals() {
    let (f, token, parent, director, _) = c7f_restart(
        c7_job_driven(c7f_fixture(c7_title_doc("Hello world")), true),
        false,
    );
    let (attempt, _, reviewer) = s5_child(&f, &director, &parent, Role::Reviewer);
    // No Status first: Check verifies the artifact, binds the attempt and drives.
    s5_review_call(
        &f.editor,
        ReviewControl::Check {
            token: token.clone(),
            reviewer_attempt: attempt.attempt.0.clone(),
            command: kinewright_project::bundle_records::SessionId::fresh().unwrap(),
        },
    );
    c7_judge(&reviewer);
    let receipt = c7f_await_receipt(&f, &token, "C7F_CHECK_AFTER_REOPEN_SEALED");
    assert_eq!(receipt.key.reviewer_attempt, attempt.attempt.0);
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_fix1_paused_resume_check_seal() {
    use kinewright_project::review_job::SealResult;
    let job = c7_job(c7_title_doc("Hello world"));
    let mut reviewer = C7Reviewer(|_: &ReviewToken| {
        c7_judge(&job.reviewer);
        Ok(())
    });
    let failing = |_: &std::fs::File, _: &kinewright_media::review_scan::ReviewScanInputs<'_>| {
        Err(kinewright_core::MediaError::Backend(
            "C7F_DECODER_LOST".into(),
        ))
    };
    let cancel = kinewright_core::ExportCancellation::default();
    let SealResult::Paused(_) = c7_drive(&job, &failing, &mut reviewer, &cancel).unwrap() else {
        panic!("C7F_PAUSED");
    };
    c7_auto_drive(&job.f, true);
    let resumed = s5_review_call(&job.f.editor, ReviewControl::Resume(job.token.clone())).unwrap();
    let ReviewPhase::Checking(checks) = &resumed.phase else {
        panic!("C7F_RESUMED_CHECKING: {:?}", resumed.phase);
    };
    assert!(
        checks.reviewer_attempt.is_none() && checks.rows.is_empty(),
        "C7F_RETRY_BINDS_AFRESH"
    );
    // Resume itself starts the driver; Check then binds the waiting one.
    assert!(!c7f_driver_done(&job.f), "C7F_RESUME_STARTS_THE_DRIVER");
    s5_review_call(
        &job.f.editor,
        ReviewControl::Check {
            token: job.token.clone(),
            reviewer_attempt: job.attempt.attempt.0.clone(),
            command: kinewright_project::bundle_records::SessionId::fresh().unwrap(),
        },
    );
    c7_judge(&job.reviewer);
    let receipt = c7f_await_receipt(&job.f, &job.token, "C7F_RESUMED_SEALED");
    assert_eq!(receipt.key.reviewer_attempt, job.attempt.attempt.0);
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_fix1_authoring_roles_and_the_director_cannot_judge() {
    let job = c7_job_driven(c7f_fixture(c7_title_doc("Hello world")), true);
    let (parent, director) = (&job.parent, &job.director);
    let (_, _, editor) = s5_child(&job.f, director, parent, Role::Editor);
    let judgment = serde_json::json!({"verdict": "pass", "reviewed_still_ids": [], "findings": []});
    // The cut's Editor and the Director never see the tool.
    for (name, mcp) in [("Editor", &editor), ("Director", director)] {
        let refused = s5_refusal(
            mcp,
            "submit_visual_judgment",
            judgment.clone(),
            "task_scope: tool hidden for durable task role",
        );
        assert!(refused.is_none(), "C7F_ROLE_REFUSED {name}: {refused:?}");
    }
    s5_review_call(&job.f.editor, ReviewControl::Stop(job.token.clone()));
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_fix1_another_reviewer_attempt_cannot_judge() {
    let job = c7_job_driven(c7f_fixture(c7_title_doc("Hello world")), true);
    let (_, _, second) = s5_child(&job.f, &job.director, &job.parent, Role::Reviewer);
    // The bound Reviewer claims the check; another Reviewer attempt is refused.
    let (stills, _, _) = s5_c6_read_all_stills(&job.reviewer, 4);
    let refused = s5_refusal(
        &second,
        "submit_visual_judgment",
        serde_json::json!({
            "verdict": "pass",
            "reviewed_still_ids": stills.iter().map(|still| still.still_id.clone()).collect::<Vec<_>>(),
            "findings": []
        }),
        "This reviewer attempt is not bound to the active output review check.",
    );
    assert!(refused.is_none(), "C7F_OTHER_ATTEMPT_REFUSED: {refused:?}");
    s5_review_call(&job.f.editor, ReviewControl::Stop(job.token.clone()));
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_fix1_an_authoring_attempt_cannot_be_the_visual_reviewer() {
    // Independence: the cut's Editor is never bound as its visual reviewer.
    let job = c7_job(c7_title_doc("Hello world"));
    let (editor, _, _) = s5_child(&job.f, &job.director, &job.parent, Role::Editor);
    let reply = job
        .f
        .editor
        .request(HostCommand::Review(ReviewControl::Check {
            token: job.token.clone(),
            reviewer_attempt: editor.attempt.0.clone(),
            command: kinewright_project::bundle_records::SessionId::fresh().unwrap(),
        }))
        .unwrap();
    assert!(
        reply.error.as_ref().is_some_and(|error| error
            .message
            .contains("The visual reviewer must be a Reviewer task attempt on the reviewed cut.")),
        "C7F_EDITOR_NOT_A_REVIEWER: {reply:?}"
    );
    assert!(
        matches!(c7f_phase(&job.f), Some(ReviewPhase::Checking(checks)) if checks.reviewer_attempt.is_none()),
        "C7F_NOTHING_BOUND"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn ja1_s5a_c7_fix1_queue_never_reads_the_person_data() {
    let f = Fixture::new();
    let key = s5_c6_publish_version(&f, None, vec![]);
    let mut f = f;
    holder(&mut f);
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .config
        .data_dir = None;
    let reply = f
        .editor
        .request(HostCommand::Review(ReviewControl::QueueDefault(key)))
        .unwrap();
    assert!(
        reply.error.as_ref().is_some_and(|error| error
            .message
            .contains(super::super::review_job::NO_DATA_ROOT)),
        "C7F_NO_DEFAULT_DATA_DIR: {reply:?}"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn ja1_s5a_c7_fix2_every_review_engine_needs_its_data_root() {
    // The encode and the overlay and stills engines share one root rule.
    let refused = super::super::review_job::review_media_engine_for_data_dir(None)
        .err()
        .expect("C7F2_ENGINE_REFUSES_NO_ROOT");
    assert!(
        refused
            .to_string()
            .contains(super::super::review_job::NO_DATA_ROOT),
        "C7F2_ENGINE_NO_ROOT: {refused}"
    );
}
