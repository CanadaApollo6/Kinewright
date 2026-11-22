// JA-1 S6 C3: closing the window keeps an active review. After an explicit
// detach the host exits on its own once no retained work remains; an attached
// GUI is never idle. The host's own `wait` loop runs on a scoped thread, as
// the shipping child runs it.

/// Wait for the host's `wait` loop to return: the detached exit finished.
fn s6c3_await_exit(handle: &std::thread::ScopedJoinHandle<'_, ()>, marker: &str) {
    let started = std::time::Instant::now();
    while !handle.is_finished() {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(120),
            "{marker}: the detached host never exited"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Run `body` beside the host's own `wait` loop. A failing body still ends
/// the loop, so a broken witness fails instead of hanging.
fn s6c3_beside_wait<R>(
    f: &Fixture,
    body: impl FnOnce(&std::thread::ScopedJoinHandle<'_, ()>) -> R,
) -> R {
    std::thread::scope(|scope| {
        let waiting = scope.spawn(|| f.host.wait_until_stopped());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(&waiting)));
        if result.is_err()
            && let Some(owner) = f.host.owner.upgrade()
        {
            let mut owner = owner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            owner.begin_shutdown();
            owner.lifecycle.request_detached_exit();
        }
        result.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

/// Retire every live attempt of `run` as the native runtime does once its
/// process is proven gone. No Editor is attached: this is not a Stop.
fn s6c3_retire_attempts(f: &Fixture, run: &RunId, marker: &str) {
    use crate::guardian::{AttemptRetirementProof, RecoveryObserver};
    let projection = f.projection(run);
    let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &f.temp.path("s6c3-machine"),
        projection.attempts[0].identity.project.as_str(),
        run.0.as_str(),
    )
    .unwrap();
    let producer = f.producer(run);
    for attempt in projection
        .attempts
        .iter()
        .filter(|attempt| !matches!(attempt.state, AttemptState::Retired(_)))
    {
        let native = crate::native_launcher::native_identity(&attempt.identity);
        claim.reserve(&native).unwrap();
        let proof = RecoveryObserver::unbound_cleanup(&mut claim, &native).unwrap();
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .reconcile_native_effect(
                &producer,
                &attempt.identity,
                &AttemptRetirementProof::Unbound(&proof),
            )
            .unwrap_or_else(|error| panic!("{marker}: {error}"));
        claim.retire_unbound(&proof).unwrap();
    }
    claim.finish().unwrap();
}

fn s6c3_detach(f: &Fixture, marker: &str) -> DetachOutcome {
    ok(
        f.editor
            .request(HostCommand::DetachForContinuation)
            .unwrap(),
        marker,
    )
    .detached
    .unwrap()
}

#[test]
fn ja1_s6_c3_a_continuation_run_that_ends_after_detach_exits_the_host() {
    let f = te13_fixture();
    let (run, execution, director) = te13_director(&f);
    let core = te13_core(&f, &run, &execution.task);
    s6c3_beside_wait(&f, |waiting| {
        let detached = s6c3_detach(&f, "S6C3_RUN_DETACH");
        assert_eq!(detached.retained_runs, vec![run.clone()]);
        let finished = director.wire(
            "finish_task",
            te13_control(&director, &execution.task, &core),
        );
        assert_eq!(finished.is_error, Some(false), "S6C3_FINISH {finished:?}");
        drop(core);
        s6c3_retire_attempts(&f, &run, "S6C3_RUN_RETIRED");
        s6c3_await_exit(waiting, "S6C3_RUN_EXIT");
    });
}

/// A Director run with a Reviewer child on the cut, its editorial audit
/// recorded, and a review of the published version queued through `encode`.
/// The production driver starts on entry to Checking.
struct S6c3Job {
    f: Fixture,
    token: ReviewToken,
    run: RunId,
    // Held so the Director's task client stays connected.
    director: C4Mcp,
    reviewer: C4Mcp,
}

fn s6c3_job(
    mut f: Fixture,
    encode: fn(&Fixture, &kinewright_project::studio_records::SnapshotKey) -> ReviewToken,
) -> S6c3Job {
    let (run, parent, director) = te13_director(&f);
    let (_, _, reviewer) = s5_child(&f, &director, &parent, Role::Reviewer);
    holder(&mut f);
    let key = s5_c6_publish_version(&f, None, vec![]);
    s5_accepted(&reviewer, "record_editorial_audit", s6c3_audit(&key));
    c7_auto_drive(&f, true);
    let token = encode(&f, &key);
    S6c3Job {
        f,
        token,
        run,
        director,
        reviewer,
    }
}

fn s6c3_audit(key: &kinewright_project::studio_records::SnapshotKey) -> serde_json::Value {
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
    })
}

/// Queue only; the caller decides when the encode may finish.
fn s6c3_queue_only(
    f: &Fixture,
    key: &kinewright_project::studio_records::SnapshotKey,
) -> ReviewToken {
    s5_review_start(&f.editor, key)
}

/// The Director's and the Reviewer's processes are gone and their attempts
/// retire: the run is durable and Pending with nothing in flight, and no
/// Editor is attached.
fn s6c3_end_run(job: &S6c3Job, marker: &str) {
    s6c3_retire_attempts(&job.f, &job.run, marker);
}

fn s6c3_status(f: &Fixture) -> HostStatus {
    ok(
        f.editor.request(HostCommand::Status).unwrap(),
        "S6C3_STATUS",
    )
    .status
}

/// A seal hook that holds every seal until released.
fn s6c3_held_seal(
    entered: &Arc<std::sync::atomic::AtomicBool>,
    release: &Arc<std::sync::atomic::AtomicBool>,
) -> super::super::review_job::ReviewSealHook {
    let entered = Arc::clone(entered);
    let release = Arc::clone(release);
    Arc::new(move |cancel: &kinewright_core::ExportCancellation| {
        use std::sync::atomic::Ordering;
        entered.store(true, Ordering::SeqCst);
        let started = std::time::Instant::now();
        while !release.load(Ordering::SeqCst) && !cancel.is_cancelled() {
            assert!(
                started.elapsed() < S5_REVIEW_HANG,
                "S6C3_SEAL_HOLD_NEVER_RELEASED"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    })
}

/// The manual history a reopen must keep: every command and its fate.
fn s6c3_history(f: &Fixture) -> serde_json::Value {
    let owner = f.host.owner.upgrade().unwrap();
    let owner = owner.lock().unwrap();
    let mut history = serde_json::to_value(owner.core().export_history().unwrap()).unwrap();
    let mut commands = history["ledger"]["commands"].take();
    for command in commands.as_array_mut().unwrap() {
        command.as_object_mut().unwrap().remove("install_epoch");
    }
    commands
}

/// One manual edit on the working cut, so the history has an entry to keep.
fn s6c3_manual_edit(f: &Fixture) {
    let owner = f.host.owner.upgrade().unwrap();
    let owner = owner.lock().unwrap();
    owner
        .core()
        .request(kinewright_core::Command::DoBatch(vec![
            kinewright_core::Operation::SetTrackSyncLock {
                track: kinewright_core::TrackId(1),
                locked: true,
            },
        ]))
        .unwrap();
}

/// Reopen the project in a new host process on the same bundle.
fn s6c3_reopen(f: Fixture) -> Fixture {
    let machine = f.temp.path("fixture-machine");
    c4_restart(f, &machine)
}

/// What a person's reopen meets while the detached host still holds the
/// project: the bundle's writer lock refuses it.
fn s6c3_reopen_refused(f: &Fixture) -> String {
    let refused = Ja1EditingSession::open(
        &f.temp.path("project.kinewright"),
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .err()
    .expect("S6C3_REOPEN_WHILE_DETACHED_IS_REFUSED");
    refused.to_string()
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s6_c3_close_during_encoding_seals_exits_and_reopens_sealed() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let f = c7f_fixture(c7_title_doc("Hello world"));
    let (arrived, release_encode, _) = s5_review_hold(&f.host, ReviewPoint::BeforeEncode);
    let job = s6c3_job(f, s6c3_queue_only);
    s5_review_until(|| arrived.load(Ordering::Acquire), "S6C3_ENCODING_HELD");
    assert!(s6c3_status(&job.f).review_active, "S6C3_ENCODING_IS_ACTIVE");
    s6c3_manual_edit(&job.f);
    let history = s6c3_history(&job.f);
    let entered = Arc::new(AtomicBool::new(false));
    let release_seal = Arc::new(AtomicBool::new(false));
    job.f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .review_runtime
        .seal_hook = Some(s6c3_held_seal(&entered, &release_seal));
    let refusal = s6c3_beside_wait(&job.f, |waiting| {
        let detached = s6c3_detach(&job.f, "S6C3_ENCODING_DETACH");
        assert_eq!(detached.retained_runs, vec![job.run.clone()]);
        assert!(detached.retained_review, "S6C3_ENCODING_RETAINS_THE_REVIEW");
        release_encode.store(true, Ordering::Release);
        // The encode publishes and the driver starts with no GUI Status; the
        // Reviewer's own stills read binds it and its judgment completes it.
        s5_review_until(
            || matches!(c7f_phase(&job.f), Some(ReviewPhase::Checking(_))),
            "S6C3_ENCODED_DETACHED",
        );
        c7_judge(&job.reviewer);
        s5_review_until(|| entered.load(Ordering::SeqCst), "S6C3_SEAL_REACHED");
        // The run ends while the seal is held: only the review keeps the host.
        s6c3_end_run(&job, "S6C3_ENCODING");
        std::thread::sleep(Duration::from_millis(500));
        assert!(!waiting.is_finished(), "S6C3_THE_REVIEW_KEEPS_THE_HOST");
        let refusal = s6c3_reopen_refused(&job.f);
        release_seal.store(true, Ordering::SeqCst);
        s6c3_await_exit(waiting, "S6C3_ENCODING_EXIT");
        refusal
    });
    eprintln!("S6C3_REOPEN_WHILE_DETACHED: {refusal}");
    // The agents' task clients go before the reopen, as their processes would.
    let S6c3Job {
        f,
        token,
        reviewer,
        director,
        ..
    } = job;
    drop((reviewer, director));
    let f = s6c3_reopen(f);
    let receipts = c7f_receipts(&f, &token);
    assert_eq!(
        receipts.len(),
        1,
        "S6C3_REOPEN_SHOWS_THE_RECEIPT: {:?}",
        c7f_phase(&f)
    );
    assert!(c7f_phase(&f).is_none(), "S6C3_REOPEN_CLEAN");
    assert_eq!(s6c3_history(&f), history, "S6C3_HISTORY_KEPT");
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s6_c3_close_during_checking_after_the_reviewer_bound_seals() {
    let job = s6c3_job(
        c7f_fixture(c7_title_doc("Hello world")),
        c7f_encode_without_status,
    );
    // The Reviewer's first stills read binds the check before the close.
    let (stills, _, _) = s5_c6_read_all_stills(&job.reviewer, 4);
    assert!(
        matches!(c7f_phase(&job.f), Some(ReviewPhase::Checking(checks)) if checks.reviewer_attempt.is_some()),
        "S6C3_REVIEWER_BOUND"
    );
    s6c3_beside_wait(&job.f, |waiting| {
        let detached = s6c3_detach(&job.f, "S6C3_CHECKING_DETACH");
        assert!(detached.retained_review, "S6C3_CHECKING_RETAINS_THE_REVIEW");
        let reply = job.reviewer.wire(
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
            "S6C3_JUDGED_DETACHED: {reply:?}"
        );
        s5_review_until(
            || !c7f_receipts(&job.f, &job.token).is_empty(),
            "S6C3_CHECKING_SEALED_DETACHED",
        );
        assert!(!waiting.is_finished(), "S6C3_THE_RUN_STILL_KEEPS_THE_HOST");
        s6c3_end_run(&job, "S6C3_CHECKING");
        s6c3_await_exit(waiting, "S6C3_CHECKING_EXIT");
    });
    // The agents' task clients go before the reopen, as their processes would.
    let S6c3Job {
        f,
        token,
        reviewer,
        director,
        ..
    } = job;
    drop((reviewer, director));
    let f = s6c3_reopen(f);
    assert_eq!(c7f_receipts(&f, &token).len(), 1, "S6C3_CHECKING_RECEIPT");
    assert!(c7f_phase(&f).is_none(), "S6C3_CHECKING_CLEAN");
}

#[test]
fn ja1_s6_c3_close_with_no_active_work_shuts_down_as_today() {
    let f = Fixture::new();
    s6c3_beside_wait(&f, |waiting| {
        let reply = f
            .editor
            .request(HostCommand::DetachForContinuation)
            .unwrap();
        let refusal = reply.error.expect("S6C3_NOTHING_TO_RETAIN");
        assert_eq!(refusal.code, "continuation_refused");
        assert_eq!(refusal.message, "no independently retained run or review");
        assert!(!reply.status.review_active, "S6C3_NO_REVIEW");
        assert_eq!(reply.status.continuation_runs, 0);
        ok(
            f.editor.request(HostCommand::Shutdown).unwrap(),
            "S6C3_SHUTDOWN",
        );
        s6c3_await_exit(waiting, "S6C3_SHUTDOWN_EXIT");
    });
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s6_c3_close_during_a_save_finishes_the_save_then_exits() {
    use kinewright_project::review_job::delivery::{SealedSaveStatus, sealed_cut_name};
    let job = s6c3_job(
        c7f_fixture(c7_title_doc("Hello world")),
        c7f_encode_without_status,
    );
    c7_judge(&job.reviewer);
    s5_review_until(
        || !c7f_receipts(&job.f, &job.token).is_empty(),
        "S6C3_SAVE_SEALED",
    );
    let receipt = c7f_receipts(&job.f, &job.token).pop().unwrap();
    s6c3_end_run(&job, "S6C3_SAVE");
    assert!(!s6c3_status(&job.f).review_active, "S6C3_THE_REVIEW_ENDED");
    let out = job.f.temp.path("out");
    std::fs::create_dir(&out).unwrap();
    let stall = S6Stall::default();
    stall.install(&job.f);
    ok(
        job.f
            .editor
            .request(s6_save_command(&receipt, &out))
            .unwrap(),
        "S6C3_SAVE_STARTED",
    );
    stall.wait_entered(&job.f);
    s6c3_beside_wait(&job.f, |waiting| {
        let detached = s6c3_detach(&job.f, "S6C3_SAVE_DETACH");
        assert!(detached.retained_review, "S6C3_THE_SAVE_IS_RETAINED");
        std::thread::sleep(Duration::from_millis(500));
        assert!(!waiting.is_finished(), "S6C3_THE_SAVE_KEEPS_THE_HOST");
        stall.release();
        s6c3_await_exit(waiting, "S6C3_SAVE_EXIT");
    });
    let name = sealed_cut_name("project", &receipt.key.snapshot.version, &receipt.outcome);
    assert_eq!(s6_entries(&out), vec![name], "S6C3_THE_SAVE_COMPLETED");
    let saved = job
        .f
        .host
        .owner
        .upgrade()
        .map(|owner| owner.lock().unwrap().review_runtime.sealed_save_status());
    assert!(
        saved.is_none_or(|status| matches!(status, Some(SealedSaveStatus::Saved { .. }))),
        "S6C3_SAVED"
    );
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s6_c3_a_review_with_no_reviewer_pauses_after_detach_and_the_host_exits() {
    let mut f = c7f_fixture(c7_title_doc("Hello world"));
    holder(&mut f);
    let key = s5_c6_publish_version(&f, None, vec![]);
    c7_auto_drive(&f, true);
    // Queued from the Studio: no run, so no Reviewer can ever bind.
    let token = c7f_encode_without_status(&f, &key);
    s6c3_beside_wait(&f, |waiting| {
        let detached = s6c3_detach(&f, "S6C3_PAUSE_DETACH");
        assert!(
            detached.retained_runs.is_empty() && detached.retained_review,
            "S6C3_ONLY_THE_REVIEW_IS_RETAINED"
        );
        s6c3_await_exit(waiting, "S6C3_PAUSE_EXIT");
    });
    let f = s6c3_reopen(f);
    let phase = c7f_phase(&f);
    let Some(ReviewPhase::Paused { reason, .. }) = &phase else {
        panic!("S6C3_PAUSED_ON_REOPEN: {phase:?}");
    };
    assert!(
        reason.contains("No Reviewer is left"),
        "S6C3_PAUSE_REASON: {reason}"
    );
    assert!(c7f_receipts(&f, &token).is_empty(), "S6C3_NOTHING_SEALED");
}

#[test]
fn ja1_s6_c3_an_attached_gui_with_no_traffic_never_exits_the_host() {
    let f = te13_fixture();
    let (run, execution, director) = te13_director(&f);
    let core = te13_core(&f, &run, &execution.task);
    s6c3_beside_wait(&f, |waiting| {
        // No request for longer than the detached poll, the reviewer poll and
        // the GUI heartbeat interval.
        std::thread::sleep(Duration::from_secs(3));
        assert!(!waiting.is_finished(), "S6C3_ATTACHED_QUIET_STAYS");
        // The run ends with the GUI attached: still not idle.
        let finished = director.wire(
            "finish_task",
            te13_control(&director, &execution.task, &core),
        );
        assert_eq!(finished.is_error, Some(false), "S6C3_ATTACHED_FINISH");
        drop(core);
        s6c3_retire_attempts(&f, &run, "S6C3_ATTACHED_RETIRED");
        std::thread::sleep(Duration::from_secs(1));
        assert!(!waiting.is_finished(), "S6C3_ATTACHED_WORK_ENDED_STAYS");
        // A seal, pause or save end runs the exit check directly, without
        // the detached poll's gate: with the GUI attached it still stays.
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .exit_when_detached_work_ends();
        std::thread::sleep(Duration::from_millis(300));
        assert!(!waiting.is_finished(), "S6C3_ATTACHED_WORK_END_CHECK_STAYS");
        ok(
            f.editor.request(HostCommand::Status).unwrap(),
            "S6C3_ATTACHED_STILL_ANSWERS",
        );
        // Only an explicit detach makes the host idle; with nothing left in
        // flight it then exits.
        s6c3_detach(&f, "S6C3_ATTACHED_DETACH");
        s6c3_await_exit(waiting, "S6C3_ATTACHED_EXIT");
    });
}
