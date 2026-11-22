// JA-1 S5a commit 7, fix round 3: while recovery, a branch switch or native
// recovery holds the project, Status reads, Stop queues and no review write
// lands; the open itself never reconciles.

#[cfg(target_os = "linux")]
const C7F3_FROZEN: &str = "The project is recovering";

#[cfg(target_os = "linux")]
fn c7f3_freeze(f: &Fixture, frozen: bool) {
    c7f2_owner(f).lock().unwrap().review_fixture_freeze(frozen);
}

#[cfg(target_os = "linux")]
fn c7f3_refused(f: &Fixture, control: ReviewControl, marker: &str) {
    let reply = f.editor.request(HostCommand::Review(control)).unwrap();
    assert!(
        reply
            .error
            .as_ref()
            .is_some_and(|error| error.message.contains(C7F3_FROZEN)),
        "{marker} {reply:?}"
    );
}

#[cfg(target_os = "linux")]
fn c7f3_bound(
    phase: Option<&ReviewPhase>,
) -> Option<kinewright_project::bundle_records::SessionId> {
    match phase {
        Some(ReviewPhase::Checking(checks)) => checks.reviewer_attempt.clone(),
        _ => None,
    }
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_fix3_frozen_status_reads_stop_queues_and_writes_refuse() {
    let (f, token, _, _, _) = c7f_restart(
        c7_job_driven(c7f_fixture(c7_title_doc("Hello world")), true),
        false,
    );
    let shared = c7f2_owner(&f);
    s5_review_until(
        || !shared.lock().unwrap().review_fixture_reopen_running(),
        "C7F3_REOPEN_RECONCILED",
    );
    // Any hash of the retained artifact enters this hook.
    let gate = C7f2Gate::default();
    gate.open();
    {
        let mut owner = shared.lock().unwrap();
        owner.review_runtime.verify_hook = Some(gate.hook());
        owner.review_fixture_freeze(true);
    }
    let stored = c7f_phase(&f).expect("C7F3_REOPENED_JOB");
    assert!(
        matches!(stored, ReviewPhase::Checking(_)),
        "C7F3_REOPENED_CHECKING {stored:?}"
    );
    let status = s5_review_call(&f.editor, ReviewControl::Status).expect("C7F3_FROZEN_STATUS");
    assert_eq!(
        (&status.token, &status.phase, status.stop_pending),
        (&token, &stored, false),
        "C7F3_FROZEN_STATUS_IS_THE_STORED_JOB"
    );
    assert!(!gate.entered(), "C7F3_FROZEN_STATUS_HASHES_NOTHING");
    assert_eq!(
        shared.lock().unwrap().review_fixture_verified(),
        None,
        "C7F3_FROZEN_STATUS_VERIFIES_NOTHING"
    );
    // Every /host review mutation refuses with the freeze's own message.
    c7f3_refused(
        &f,
        ReviewControl::Resume(token.clone()),
        "C7F3_FROZEN_RESUME_REFUSED",
    );
    c7f3_refused(
        &f,
        ReviewControl::Annotate {
            snapshot: token.snapshot.clone(),
            fields: kinewright_project::review_job::ReviewAnnotationFields::default(),
        },
        "C7F3_FROZEN_ANNOTATE_REFUSED",
    );
    c7f3_refused(
        &f,
        ReviewControl::QueueDefault(token.snapshot.clone()),
        "C7F3_FROZEN_QUEUE_REFUSED",
    );
    // Stop is queued behind its fence; nothing is written yet.
    let stop = s5_review_call(&f.editor, ReviewControl::Stop(token.clone())).expect("C7F3_STOP");
    assert!(stop.stop_pending, "C7F3_FROZEN_STOP_QUEUED {stop:?}");
    assert_eq!(
        c7f_phase(&f).as_ref(),
        Some(&stored),
        "C7F3_FROZEN_STOP_WRITES_NOTHING"
    );
    let pending = s5_review_call(&f.editor, ReviewControl::Status).expect("C7F3_PENDING");
    assert!(pending.stop_pending, "C7F3_STATUS_SHOWS_THE_QUEUED_STOP");
    assert!(!gate.entered(), "C7F3_FROZEN_STOP_HASHES_NOTHING");
    // The first Status after the freeze persists the queued Stop.
    c7f3_freeze(&f, false);
    let thawed = s5_review_call(&f.editor, ReviewControl::Status).expect("C7F3_THAWED");
    assert!(
        thawed.token == token
            && !thawed.stop_pending
            && matches!(thawed.phase, ReviewPhase::Stopping | ReviewPhase::Stopped),
        "C7F3_QUEUED_STOP_PERSISTS_AFTER_THE_FREEZE {thawed:?}"
    );
    assert!(
        !matches!(c7f_phase(&f), Some(ReviewPhase::Checking(_))),
        "C7F3_JOB_STOPPED"
    );
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_fix3_reviewer_and_driver_writes_wait_out_a_freeze() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let job = c7_job_driven(c7f_fixture(c7_title_doc("Hello world")), true);
    // The first seal is held off-lock until released; later ones pass.
    let entered = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let returned = Arc::new(AtomicBool::new(false));
    let first = Arc::new(AtomicBool::new(true));
    {
        let (entered, release, returned, first) = (
            entered.clone(),
            release.clone(),
            returned.clone(),
            first.clone(),
        );
        let hook: C7f2SealHook = Arc::new(move |cancel: &kinewright_core::ExportCancellation| {
            if first.swap(false, Ordering::SeqCst) {
                entered.store(true, Ordering::SeqCst);
                while !release.load(Ordering::SeqCst) && !cancel.is_cancelled() {
                    std::thread::sleep(Duration::from_millis(5));
                }
                returned.store(true, Ordering::SeqCst);
            }
            Ok(())
        });
        c7f2_owner(&job.f).lock().unwrap().review_runtime.seal_hook = Some(hook);
    }
    let (stills, _, _) = s5_c6_read_all_stills(&job.reviewer, 4);
    let judgment = serde_json::json!({
        "verdict": "pass",
        "reviewed_still_ids": stills.iter().map(|still| still.still_id.clone()).collect::<Vec<_>>(),
        "findings": []
    });
    let before = c7f_phase(&job.f);
    // Task tools already refuse during each freeze, before the review gate
    // (`resolve_task_mcp` and the attachment check in `task_mcp.rs`).
    let owner = c7f2_owner(&job.f);
    let freezes: [fn(&mut HostCommandOwner, bool); 3] = [
        HostCommandOwner::review_fixture_native_pending,
        HostCommandOwner::review_fixture_freeze,
        HostCommandOwner::review_fixture_recovering,
    ];
    for freeze in freezes {
        freeze(&mut owner.lock().unwrap(), true);
        let refused = job
            .reviewer
            .wire("submit_visual_judgment", judgment.clone());
        freeze(&mut owner.lock().unwrap(), false);
        assert_eq!(
            refused.is_error,
            Some(true),
            "C7F3_FROZEN_JUDGMENT_REFUSED {refused:?}"
        );
        assert_eq!(
            c7f_phase(&job.f),
            before,
            "C7F3_FROZEN_JUDGMENT_WRITES_NOTHING"
        );
    }
    let accepted = job.reviewer.wire("submit_visual_judgment", judgment);
    assert_ne!(
        accepted.is_error,
        Some(true),
        "C7F3_THAWED_JUDGMENT {accepted:?}"
    );
    s5_review_until(
        || {
            matches!(c7f_phase(&job.f), Some(ReviewPhase::ReadyToSeal { .. }))
                && entered.load(Ordering::SeqCst)
        },
        "C7F3_SEAL_HELD",
    );
    // The seal under way finds the project frozen when it relocks: it
    // publishes nothing, records no pause, and its driver waits.
    c7f3_freeze(&job.f, true);
    release.store(true, Ordering::SeqCst);
    s5_review_until(|| returned.load(Ordering::SeqCst), "C7F3_HELD_SEAL_RETURNS");
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        c7f_receipts(&job.f, &job.token).is_empty(),
        "C7F3_FROZEN_SEAL_PUBLISHES_NOTHING"
    );
    assert!(
        matches!(c7f_phase(&job.f), Some(ReviewPhase::ReadyToSeal { .. })),
        "C7F3_FROZEN_SEAL_RECORDS_NO_PAUSE {:?}",
        c7f_phase(&job.f)
    );
    assert_eq!(
        c7f2_generation(&job.f),
        (1, true),
        "C7F3_FROZEN_DRIVER_WAITS"
    );
    // After the freeze the same driver seals, with no command.
    c7f3_freeze(&job.f, false);
    s5_review_until(
        || c7f_receipts(&job.f, &job.token).len() == 1,
        "C7F3_THAWED_SEAL",
    );
    assert_eq!(c7f2_generation(&job.f).0, 1, "C7F3_ONE_DRIVER");
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_fix3_an_encode_finishing_while_frozen_publishes_nothing() {
    use std::sync::atomic::Ordering;
    let temp = kinewright_media::test_support::TempDirectory::new("review-frozen-encode");
    s5_review_seed(temp.root());
    let (host, client) = s5_review_host(temp.root());
    let key = s5_review_version(&host);
    let held = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let shared = host.owner.upgrade().unwrap();
    {
        let (held, release) = (held.clone(), release.clone());
        shared.lock().unwrap().review_fixture_probe(move |at| {
            if !matches!(at, ReviewPoint::BeforePublication) {
                return;
            }
            held.store(true, Ordering::Release);
            let until = std::time::Instant::now() + S5_REVIEW_HANG;
            while !release.load(Ordering::Acquire) {
                assert!(std::time::Instant::now() < until, "C7F3_HOLD_RELEASED");
                std::thread::sleep(Duration::from_millis(5));
            }
        });
    }
    let token = s5_review_start(&client, &key);
    s5_review_until(
        || held.load(Ordering::Acquire),
        "C7F3_ENCODE_BEFORE_PUBLICATION",
    );
    shared.lock().unwrap().review_fixture_freeze(true);
    release.store(true, Ordering::Release);
    s5_review_until(
        || {
            shared
                .lock()
                .unwrap()
                .review_fixture_worker()
                .is_none_or(|(_, done)| done)
        },
        "C7F3_FROZEN_ENCODE_ENDS",
    );
    {
        let owner = shared.lock().unwrap();
        let bundle = owner.owner().unwrap().review_bundle().unwrap();
        let job = bundle.review_job().unwrap().expect("C7F3_JOB_KEPT");
        assert_eq!(job.token, token, "C7F3_SAME_JOB");
        assert_eq!(
            job.phase,
            ReviewPhase::Encoding,
            "C7F3_FROZEN_ENCODE_PUBLISHES_NOTHING"
        );
        assert!(job.artifact_ref.is_none(), "C7F3_FROZEN_ENCODE_NO_ARTIFACT");
    }
    // After the freeze the job reconciles as any interrupted encode does.
    shared.lock().unwrap().review_fixture_freeze(false);
    let status = s5_review_call(&client, ReviewControl::Status).expect("C7F3_THAWED_STATUS");
    assert_ne!(
        status.phase,
        ReviewPhase::Encoding,
        "C7F3_THAWED_RECONCILES"
    );
}

/// `c7f_restart` that runs `between` while no host holds the project.
#[cfg(target_os = "linux")]
fn c7f3_restart_between(job: C7Job, between: impl FnOnce(&std::path::Path)) -> Fixture {
    let C7Job {
        f,
        reviewer,
        run,
        parent,
        director,
        ..
    } = job;
    drop(reviewer);
    drop(director);
    let machine = f.temp.path("fixture-machine");
    let mut f = c4_restart_with(f, &machine, between);
    holder(&mut f);
    let _ = c4_reserve_task(&f, &run, &parent.task);
    f
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_fix3_an_open_never_reconciles_the_review_job() {
    let job = c7_job_driven(c7f_fixture(c7_title_doc("Hello world")), true);
    s5_c6_read_all_stills(&job.reviewer, 4);
    s5_review_until(
        || c7f3_bound(c7f_phase(&job.f).as_ref()).is_some(),
        "C7F3_REVIEWER_BOUND",
    );
    let bound = c7f3_bound(c7f_phase(&job.f).as_ref());
    let roots = job.f.temp.root().to_path_buf();
    let f = c7f3_restart_between(job, |project| {
        // The open a recovery-only owner makes (`studio_runtime.rs`): it must
        // leave the job exactly as the gone process left it.
        let owner = HostCommandOwner::open(HostConfig {
            data_dir: None,
            desktop_notifications: false,
            scheduler_settings: None,
            project: "hooks".into(),
            path: Some(project.to_path_buf()),
            roots: SessionRoots {
                read: vec![roots.clone()],
                write: vec![roots.clone()],
            },
            origin: HostOrigin::Headless,
        })
        .expect("C7F3_RECOVERY_OPEN");
        let phase = owner
            .owner()
            .unwrap()
            .review_bundle()
            .unwrap()
            .review_job()
            .unwrap()
            .map(|job| job.phase.clone());
        assert_eq!(
            c7f3_bound(phase.as_ref()),
            bound,
            "C7F3_OPEN_LEAVES_THE_BINDING"
        );
    });
    // The served host's reopen thread reconciles once the project thaws.
    s5_review_until(
        || c7f3_bound(c7f_phase(&f).as_ref()).is_none(),
        "C7F3_THAWED_REOPEN_UNBINDS",
    );
}
