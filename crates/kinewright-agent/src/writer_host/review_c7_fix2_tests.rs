// JA-1 S5a commit 7, fix round 2: the reopen thread, Save As, Resume after
// an exiting driver, and a failed automatic seal.

/// A gate a hook waits on: released by the test or by the thread's cancellation.
#[cfg(target_os = "linux")]
#[derive(Clone, Default)]
struct C7f2Gate {
    entered: Arc<std::sync::atomic::AtomicBool>,
    release: Arc<std::sync::atomic::AtomicBool>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    exited: Arc<std::sync::atomic::AtomicBool>,
    // How long the hook keeps its thread after a cancellation: a join
    // waits it out, a thread left running does not.
    linger: Duration,
}

#[cfg(target_os = "linux")]
impl C7f2Gate {
    fn hook(&self) -> super::super::review_job::ReviewCancelHook {
        let gate = self.clone();
        Arc::new(move |cancel: &kinewright_core::ExportCancellation| {
            gate.entered
                .store(true, std::sync::atomic::Ordering::SeqCst);
            while !gate.release.load(std::sync::atomic::Ordering::SeqCst) {
                if cancel.is_cancelled() {
                    gate.cancelled
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    std::thread::sleep(gate.linger);
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            gate.exited.store(true, std::sync::atomic::Ordering::SeqCst);
        })
    }
    fn entered(&self) -> bool {
        self.entered.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn open(&self) {
        self.release
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
    fn exited(&self) -> bool {
        self.exited.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(target_os = "linux")]
fn c7f2_owner(f: &Fixture) -> Arc<Mutex<HostCommandOwner>> {
    f.host.owner.upgrade().unwrap()
}

#[cfg(target_os = "linux")]
fn c7f2_generation(f: &Fixture) -> (u64, bool) {
    c7f2_owner(f)
        .lock()
        .unwrap()
        .review_fixture_driver_generation()
}

/// Restart with the reopen verification held at `gate`.
#[cfg(target_os = "linux")]
fn c7f2_restart_held(
    job: C7Job,
    gate: &C7f2Gate,
) -> (
    Fixture,
    ReviewToken,
    ExecutionIdentity,
    C4Mcp,
    ExecutionIdentity,
) {
    let project = job.f.temp.path("project.kinewright");
    super::super::review_job::VERIFY_HOOK_ON_OPEN
        .lock()
        .unwrap()
        .push((project, gate.hook()));
    let restarted = c7f_restart(job, true);
    s5_review_until(|| gate.entered(), "C7F2_REOPEN_HASHING");
    restarted
}

/// A `ReadyToSeal` job, its seal held until the old host stops.
#[cfg(target_os = "linux")]
fn c7f2_ready_job() -> C7Job {
    let job = c7_job_driven(c7f_fixture(c7_title_doc("Hello world")), true);
    let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
    c7f2_owner(&job.f).lock().unwrap().review_runtime.seal_hook = Some(c7f2_held_seal(&entered));
    c7_judge(&job.reviewer);
    s5_review_until(
        || {
            matches!(c7f_phase(&job.f), Some(ReviewPhase::ReadyToSeal { .. }))
                && entered.load(std::sync::atomic::Ordering::SeqCst)
        },
        "C7F2_READY_TO_SEAL_HELD",
    );
    job
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_fix2_reopen_hash_never_holds_the_host_and_seal_waits() {
    let gate = C7f2Gate::default();
    let (f, token, _, _, attempt) = c7f2_restart_held(c7f2_ready_job(), &gate);
    let shared = c7f2_owner(&f);
    // The artifact is hashed with the host mutex free.
    assert!(shared.try_lock().is_ok(), "C7F2_HASH_OFF_LOCK");
    // A Seal during the hash waits for it, then seals.
    let editor = f.editor.clone();
    let seal = {
        let token = token.clone();
        std::thread::spawn(move || {
            editor
                .request(HostCommand::Review(ReviewControl::Seal {
                    token,
                    command: kinewright_project::bundle_records::SessionId::fresh().unwrap(),
                }))
                .unwrap()
        })
    };
    std::thread::sleep(Duration::from_millis(100));
    assert!(!seal.is_finished(), "C7F2_SEAL_WAITS_FOR_THE_HASH");
    assert!(shared.try_lock().is_ok(), "C7F2_STATUS_SERVED_DURING_HASH");
    gate.open();
    let reply = seal.join().unwrap();
    assert!(
        reply.error.is_none(),
        "C7F2_SEAL_DURING_VERIFY: {:?}",
        reply.error
    );
    let receipt = reply
        .review_receipt
        .expect("C7F2_SEAL_DURING_VERIFY_RECEIPT");
    assert_eq!(receipt.key.reviewer_attempt, attempt.attempt.0);
    s5_review_until(|| c7f_phase(&f).is_none(), "C7F2_JOB_RETIRED");
    assert_eq!(c7f_receipts(&f, &token).len(), 1, "C7F2_ONE_RECEIPT");
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_fix2_check_during_reopen_verification_waits_then_binds() {
    let gate = C7f2Gate::default();
    let (f, token, parent, director, _) = c7f2_restart_held(
        c7_job_driven(c7f_fixture(c7_title_doc("Hello world")), true),
        &gate,
    );
    let (attempt, _, reviewer) = s5_child(&f, &director, &parent, Role::Reviewer);
    let editor = f.editor.clone();
    let check = {
        let token = token.clone();
        let reviewer_attempt = attempt.attempt.0.clone();
        std::thread::spawn(move || {
            editor
                .request(HostCommand::Review(ReviewControl::Check {
                    token,
                    reviewer_attempt,
                    command: kinewright_project::bundle_records::SessionId::fresh().unwrap(),
                }))
                .unwrap()
        })
    };
    std::thread::sleep(Duration::from_millis(100));
    assert!(!check.is_finished(), "C7F2_CHECK_WAITS_FOR_THE_HASH");
    gate.open();
    let reply = check.join().unwrap();
    assert!(
        reply.error.is_none(),
        "C7F2_CHECK_DURING_VERIFY: {:?}",
        reply.error
    );
    c7_judge(&reviewer);
    let receipt = c7f_await_receipt(&f, &token, "C7F2_CHECK_DURING_VERIFY_SEALED");
    assert_eq!(receipt.key.reviewer_attempt, attempt.attempt.0);
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_fix2_reopen_starts_no_driver_while_frozen() {
    let gate = C7f2Gate::default();
    let (f, token, _, _, _) = c7f2_restart_held(
        c7_job_driven(c7f_fixture(c7_title_doc("Hello world")), true),
        &gate,
    );
    let shared = c7f2_owner(&f);
    shared.lock().unwrap().review_fixture_freeze(true);
    gate.open();
    s5_review_until(
        || !shared.lock().unwrap().review_fixture_reopen_running(),
        "C7F2_REOPEN_RETURNED",
    );
    assert_eq!(
        c7f2_generation(&f),
        (0, false),
        "C7F2_FROZEN_REOPEN_STARTS_NO_DRIVER"
    );
    shared.lock().unwrap().review_fixture_freeze(false);
    s5_review_call(&f.editor, ReviewControl::Status);
    assert_eq!(c7f2_generation(&f), (1, true), "C7F2_THAWED_STATUS_DRIVES");
    s5_review_call(&f.editor, ReviewControl::Stop(token));
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_fix2_shutdown_cancels_and_joins_the_reopen_thread() {
    let gate = C7f2Gate {
        linger: Duration::from_millis(500),
        ..C7f2Gate::default()
    };
    let (f, token, _, _, _) = c7f2_restart_held(
        c7_job_driven(c7f_fixture(c7_title_doc("Hello world")), true),
        &gate,
    );
    let phase = c7f_phase(&f);
    let machine = f.temp.path("fixture-machine");
    // The host stops with the reopen thread inside its hash.
    let f = c4_restart(f, &machine);
    assert!(gate.cancelled(), "C7F2_SHUTDOWN_CANCELS_THE_REOPEN");
    assert!(gate.exited(), "C7F2_SHUTDOWN_JOINS_THE_REOPEN");
    // The next host opened the project: the old thread held no lock past
    // shutdown and changed nothing after it.
    assert_eq!(
        serde_json::to_value(c7f_phase(&f)).unwrap(),
        serde_json::to_value(phase).unwrap(),
        "C7F2_NOTHING_AFTER_THE_FENCE"
    );
    assert!(c7f_receipts(&f, &token).is_empty());
}

#[cfg(target_os = "linux")]
fn c7f2_save_as(f: &Fixture) -> HostReply {
    let snapshot = received_snapshot_for_test(c7f2_owner(f).lock().unwrap().core()).unwrap();
    f.editor
        .request(HostCommand::SaveAs {
            destination: f.temp.path("copy.kinewright"),
            snapshot,
        })
        .unwrap()
}

#[cfg(target_os = "linux")]
fn c7f2_source_phase(f: &Fixture, token: &ReviewToken) -> serde_json::Value {
    let shared = c7f2_owner(f);
    let owner = shared.lock().unwrap();
    let phase = owner
        .owner()
        .unwrap()
        .review_source_bundle(&token.project)
        .unwrap()
        .review_job()
        .unwrap()
        .map(|job| job.phase.clone());
    serde_json::to_value(phase).unwrap()
}

#[cfg(target_os = "linux")]
fn c7f2_source_sealed(f: &Fixture, token: &ReviewToken) -> bool {
    let shared = c7f2_owner(f);
    let mut owner = shared.lock().unwrap();
    owner
        .owner_mut()
        .unwrap()
        .review_source_bundle_mut(&token.project)
        .unwrap()
        .sealed_review(token)
        .unwrap()
        .is_some()
}

/// Save As away from a driven job: the driver is joined, nothing of the
/// source is verified, and the copy's reviews are free.
#[cfg(target_os = "linux")]
fn c7f2_assert_save_as_leaves(job: &C7Job, marker: &str) -> serde_json::Value {
    let (started, live) = c7f2_generation(&job.f);
    assert!(live, "{marker}_DRIVER_LIVE_BEFORE");
    let before = c7f2_source_phase(&job.f, &job.token);
    let reply = c7f2_save_as(&job.f);
    assert!(reply.error.is_none(), "{marker}_SAVE_AS: {:?}", reply.error);
    {
        let shared = c7f2_owner(&job.f);
        let owner = shared.lock().unwrap();
        assert_eq!(
            owner.review_fixture_driver_threads(),
            0,
            "{marker}_DRIVER_JOINED"
        );
        assert_eq!(owner.review_fixture_verified(), None, "{marker}_UNVERIFIED");
        assert_ne!(
            owner.owner().unwrap().project_id().unwrap(),
            job.token.project,
            "{marker}_COPY_IS_ACTIVE"
        );
    }
    // The copy's review controls are served, and no driver of the source
    // answers for it.
    let status = job
        .f
        .editor
        .request(HostCommand::Review(ReviewControl::Status))
        .unwrap();
    assert!(
        status.error.is_none(),
        "{marker}_COPY_STATUS: {:?}",
        status.error
    );
    assert!(status.review.is_none(), "{marker}_COPY_HAS_NO_JOB");
    assert_eq!(
        c7f2_generation(&job.f),
        (started, false),
        "{marker}_NO_DRIVER_FOR_THE_SOURCE"
    );
    assert_eq!(
        c7f2_source_phase(&job.f, &job.token),
        before,
        "{marker}_SOURCE_UNCHANGED"
    );
    before
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_fix2_save_as_joins_an_unbound_waiting_driver() {
    let job = c7_job_driven(c7f_fixture(c7_title_doc("Hello world")), true);
    let before = c7f2_assert_save_as_leaves(&job, "C7F2_UNBOUND");
    // The Reviewer's stills read no longer claims the source's check.
    let _ = job
        .reviewer
        .wire("read_review_stills", serde_json::json!({ "page_size": 4 }));
    assert_eq!(
        c7f2_source_phase(&job.f, &job.token),
        before,
        "C7F2_UNBOUND_SOURCE_NOT_CLAIMED"
    );
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_fix2_save_as_joins_a_bound_driver() {
    let job = c7_job_driven(c7f_fixture(c7_title_doc("Hello world")), true);
    let (stills, _, _) = s5_c6_read_all_stills(&job.reviewer, 4);
    s5_review_until(
        || {
            // Bound, with the technical rows recorded: the driver waits for
            // the Reviewer's judgment.
            matches!(c7f_phase(&job.f), Some(ReviewPhase::Checking(checks))
                if checks.reviewer_attempt.is_some() && checks.rows.len() >= 4)
        },
        "C7F2_BOUND",
    );
    let before = c7f2_assert_save_as_leaves(&job, "C7F2_BOUND");
    let _ = job.reviewer.wire(
        "submit_visual_judgment",
        serde_json::json!({
            "verdict": "pass",
            "reviewed_still_ids": stills.iter().map(|still| still.still_id.clone()).collect::<Vec<_>>(),
            "findings": []
        }),
    );
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        c7f2_source_phase(&job.f, &job.token),
        before,
        "C7F2_BOUND_SOURCE_UNCHANGED"
    );
    assert!(
        !c7f2_source_sealed(&job.f, &job.token),
        "C7F2_BOUND_NOT_SEALED"
    );
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_fix2_save_as_joins_a_driver_ready_to_seal() {
    let job = c7f2_ready_job();
    c7f2_assert_save_as_leaves(&job, "C7F2_READY");
    assert!(
        matches!(
            serde_json::from_value(c7f2_source_phase(&job.f, &job.token)).unwrap(),
            Some(ReviewPhase::ReadyToSeal { .. })
        ),
        "C7F2_READY_SOURCE_STAYS_READY"
    );
    assert!(
        !c7f2_source_sealed(&job.f, &job.token),
        "C7F2_READY_NOT_SEALED"
    );
}

/// A seal hook that fails the first `failures` seals, then lets them run.
#[cfg(target_os = "linux")]
fn c7f2_failing_seal(failures: usize) -> C7f2SealHook {
    let calls = Arc::new(AtomicUsize::new(0));
    Arc::new(move |_: &kinewright_core::ExportCancellation| {
        if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < failures {
            Err(HostRefusal::new("review_refused", "C7F2_SEAL_FAILED").into())
        } else {
            Ok(())
        }
    })
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_fix2_a_failed_automatic_seal_pauses_with_its_reason() {
    let job = c7_job_driven(c7f_fixture(c7_title_doc("Hello world")), true);
    c7f2_owner(&job.f).lock().unwrap().review_runtime.seal_hook =
        Some(c7f2_failing_seal(usize::MAX));
    c7_judge(&job.reviewer);
    s5_review_until(
        || matches!(c7f_phase(&job.f), Some(ReviewPhase::Paused { .. })),
        "C7F2_SEAL_FAILURE_PAUSES",
    );
    let status = s5_review_call(&job.f.editor, ReviewControl::Status).unwrap();
    let ReviewPhase::Paused { reason, .. } = &status.phase else {
        panic!("C7F2_STATUS_PAUSED: {:?}", status.phase);
    };
    assert!(
        reason.contains("C7F2_SEAL_FAILED"),
        "C7F2_STATUS_SAYS_WHY: {reason}"
    );
    // No silent retry: the driver ends and no other starts.
    s5_review_until(|| !c7f2_generation(&job.f).1, "C7F2_DRIVER_ENDS");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(c7f2_generation(&job.f), (1, false), "C7F2_NO_SILENT_RETRY");
    assert!(c7f_receipts(&job.f, &job.token).is_empty());
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_fix2_resume_while_the_old_driver_exits_starts_a_new_one() {
    let f = c7f_fixture(c7_title_doc("Hello world"));
    let gate = C7f2Gate::default();
    {
        // Installed before the job's driver starts on entry to Checking.
        let shared = c7f2_owner(&f);
        let mut owner = shared.lock().unwrap();
        owner.review_runtime.seal_hook = Some(c7f2_failing_seal(1));
        owner.review_runtime.driver_exit_hook = Some(gate.hook());
    }
    let job = c7_job_driven(f, true);
    c7_judge(&job.reviewer);
    // The paused driver has decided to exit and is still on its thread.
    s5_review_until(
        || matches!(c7f_phase(&job.f), Some(ReviewPhase::Paused { .. })) && gate.entered(),
        "C7F2_OLD_DRIVER_EXITING",
    );
    assert!(!gate.exited(), "C7F2_OLD_DRIVER_STILL_ON_ITS_THREAD");
    assert_eq!(c7f2_generation(&job.f), (1, false));
    c7f2_owner(&job.f)
        .lock()
        .unwrap()
        .review_runtime
        .driver_exit_hook = None;
    let resumed = s5_review_call(&job.f.editor, ReviewControl::Resume(job.token.clone())).unwrap();
    assert!(
        matches!(resumed.phase, ReviewPhase::Checking(_)),
        "C7F2_RESUMED: {:?}",
        resumed.phase
    );
    assert_eq!(
        c7f2_generation(&job.f),
        (2, true),
        "C7F2_RESUME_STARTS_A_NEW_DRIVER"
    );
    // The Resume request joined the old thread.
    assert!(gate.exited(), "C7F2_OLD_DRIVER_JOINED");
    s5_review_call(
        &job.f.editor,
        ReviewControl::Check {
            token: job.token.clone(),
            reviewer_attempt: job.attempt.attempt.0.clone(),
            command: kinewright_project::bundle_records::SessionId::fresh().unwrap(),
        },
    );
    c7_judge(&job.reviewer);
    let receipt = c7f_await_receipt(&job.f, &job.token, "C7F2_RESUMED_SEALED");
    assert_eq!(receipt.key.reviewer_attempt, job.attempt.attempt.0);
}

/// A seal hook that holds the driver's seal and every other seal apart.
#[cfg(target_os = "linux")]
#[derive(Clone, Default)]
struct C7f2SealRace {
    calls: Arc<AtomicUsize>,
    driver: Arc<std::sync::atomic::AtomicBool>,
    other: Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(target_os = "linux")]
impl C7f2SealRace {
    fn hook(&self) -> C7f2SealHook {
        let race = self.clone();
        Arc::new(move |cancel: &kinewright_core::ExportCancellation| {
            race.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let release = if std::thread::current().name() == Some("output-review-checks") {
                &race.driver
            } else {
                &race.other
            };
            while !release.load(std::sync::atomic::Ordering::SeqCst) && !cancel.is_cancelled() {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(())
        })
    }
    fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_fix2_a_seal_that_loses_the_race_replays_the_receipt() {
    let job = c7_job_driven(c7f_fixture(c7_title_doc("Hello world")), true);
    let race = C7f2SealRace::default();
    c7f2_owner(&job.f).lock().unwrap().review_runtime.seal_hook = Some(race.hook());
    c7_judge(&job.reviewer);
    s5_review_until(
        || matches!(c7f_phase(&job.f), Some(ReviewPhase::ReadyToSeal { .. })) && race.calls() == 1,
        "C7F2_DRIVER_SEAL_HELD",
    );
    // The person's Seal passes the first replay check while the driver's
    // seal is held, then hashes behind it.
    let seal = {
        let editor = job.f.editor.clone();
        let token = job.token.clone();
        std::thread::spawn(move || {
            editor
                .request(HostCommand::Review(ReviewControl::Seal {
                    token,
                    command: kinewright_project::bundle_records::SessionId::fresh().unwrap(),
                }))
                .unwrap()
        })
    };
    s5_review_until(|| race.calls() == 2, "C7F2_BOTH_SEALS_HELD");
    race.driver.store(true, std::sync::atomic::Ordering::SeqCst);
    s5_review_until(
        || !c7f_receipts(&job.f, &job.token).is_empty(),
        "C7F2_DRIVER_SEALED_FIRST",
    );
    race.other.store(true, std::sync::atomic::Ordering::SeqCst);
    let reply = seal.join().unwrap();
    assert!(
        reply.error.is_none(),
        "C7F2_LATE_SEAL_REPLAYS: {:?}",
        reply.error
    );
    let mut receipts = c7f_receipts(&job.f, &job.token);
    assert_eq!(receipts.len(), 1, "C7F2_ONE_RECEIPT_AFTER_THE_RACE");
    assert_eq!(
        serde_json::to_value(reply.review_receipt.expect("C7F2_LATE_SEAL_RECEIPT")).unwrap(),
        serde_json::to_value(receipts.pop().unwrap()).unwrap(),
        "C7F2_LATE_SEAL_IS_THE_DRIVER_RECEIPT"
    );
}
