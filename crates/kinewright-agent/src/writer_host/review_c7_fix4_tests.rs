// JA-1 S5a commit 7, fix round 4: while frozen, no seal settles the root, no
// Status or Stop reads an unsettled root, and a Stop queued behind the freeze
// persists at the first request after it.

/// Mark the active source root as needing a settle, as a publication with an
/// unknown outcome leaves it.
#[cfg(target_os = "linux")]
fn c7f4_mark_unsettled(f: &Fixture) {
    let shared = c7f2_owner(f);
    let mut owner = shared.lock().unwrap();
    let session = owner.owner_mut().unwrap();
    let project = session.project_id().unwrap();
    session
        .review_source_bundle_mut(&project)
        .unwrap()
        .mark_unsettled_for_test();
}

#[cfg(target_os = "linux")]
fn c7f4_unsettled(f: &Fixture) -> bool {
    c7f2_owner(f)
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .review_bundle()
        .unwrap()
        .unsettled_probe()
}

/// A reopened Checking job whose reopen thread has finished.
#[cfg(target_os = "linux")]
fn c7f4_reopened() -> (Fixture, ReviewToken, ReviewPhase) {
    let (f, token, _, _, _) = c7f_restart(
        c7_job_driven(c7f_fixture(c7_title_doc("Hello world")), true),
        false,
    );
    let shared = c7f2_owner(&f);
    s5_review_until(
        || !shared.lock().unwrap().review_fixture_reopen_running(),
        "C7F4_REOPEN_RECONCILED",
    );
    let stored = c7f_phase(&f).expect("C7F4_REOPENED_JOB");
    assert!(
        matches!(stored, ReviewPhase::Checking(_)),
        "C7F4_REOPENED_CHECKING {stored:?}"
    );
    (f, token, stored)
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_fix4_a_seal_under_way_never_settles_a_frozen_root() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let job = c7_job_driven(c7f_fixture(c7_title_doc("Hello world")), true);
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
    let accepted = job.reviewer.wire("submit_visual_judgment", judgment);
    assert_ne!(accepted.is_error, Some(true), "C7F4_JUDGMENT {accepted:?}");
    s5_review_until(
        || {
            matches!(c7f_phase(&job.f), Some(ReviewPhase::ReadyToSeal { .. }))
                && entered.load(Ordering::SeqCst)
        },
        "C7F4_SEAL_HELD",
    );
    // A switch freezes the project while its root needs a settle. The seal's
    // relock must not settle it, not even to look for a replayed receipt.
    {
        let shared = c7f2_owner(&job.f);
        shared.lock().unwrap().review_fixture_freeze(true);
    }
    c7f4_mark_unsettled(&job.f);
    release.store(true, Ordering::SeqCst);
    s5_review_until(|| returned.load(Ordering::SeqCst), "C7F4_HELD_SEAL_RETURNS");
    std::thread::sleep(Duration::from_millis(300));
    assert!(c7f4_unsettled(&job.f), "C7F4_FROZEN_SEAL_SETTLES_NOTHING");
    assert!(
        c7f_receipts(&job.f, &job.token).is_empty(),
        "C7F4_FROZEN_SEAL_PUBLISHES_NOTHING"
    );
    // After the freeze the waiting driver settles and seals.
    c7f3_freeze(&job.f, false);
    s5_review_until(
        || c7f_receipts(&job.f, &job.token).len() == 1,
        "C7F4_THAWED_SEAL",
    );
    assert!(!c7f4_unsettled(&job.f), "C7F4_THAWED_SEAL_SETTLED");
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_fix4_frozen_status_and_stop_never_read_an_unsettled_root() {
    let (f, token, stored) = c7f4_reopened();
    c7f3_freeze(&f, true);
    c7f4_mark_unsettled(&f);
    let status = f
        .editor
        .request(HostCommand::Review(ReviewControl::Status))
        .unwrap();
    assert!(
        status
            .error
            .as_ref()
            .is_some_and(|error| error.message.contains("requires reconciliation")),
        "C7F4_FROZEN_UNSETTLED_STATUS_SHOWS_NO_PHASE {status:?}"
    );
    c7f3_refused(
        &f,
        ReviewControl::Stop(token.clone()),
        "C7F4_FROZEN_UNSETTLED_STOP_REFUSED",
    );
    assert!(
        c7f2_owner(&f).lock().unwrap().review_fixture_stops_queued() == 0,
        "C7F4_FROZEN_UNSETTLED_STOP_ARMS_NOTHING"
    );
    assert!(c7f4_unsettled(&f), "C7F4_FROZEN_READS_SETTLE_NOTHING");
    // After the freeze Status settles and shows the job, which no Stop reached.
    c7f3_freeze(&f, false);
    let thawed = s5_review_call(&f.editor, ReviewControl::Status).expect("C7F4_THAWED");
    assert_eq!(
        (&thawed.token, &thawed.phase, thawed.stop_pending),
        (&token, &stored, false),
        "C7F4_THAWED_STATUS_IS_THE_UNSTOPPED_JOB"
    );
}

#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s5a_c7_fix4_a_queued_stop_persists_at_the_first_request_after_a_thaw() {
    let (f, token, stored) = c7f4_reopened();
    c7f3_freeze(&f, true);
    let stop = s5_review_call(&f.editor, ReviewControl::Stop(token.clone())).expect("C7F4_STOP");
    assert!(stop.stop_pending, "C7F4_FROZEN_STOP_QUEUED {stop:?}");
    assert_eq!(c7f_phase(&f).as_ref(), Some(&stored), "C7F4_QUEUED_ONLY");
    // The thaw itself writes nothing; the next /host request of any kind,
    // here a plain host Status, persists the queued Stop.
    c7f3_freeze(&f, false);
    let reply = f.editor.request(HostCommand::Status).unwrap();
    assert!(reply.error.is_none(), "C7F4_HOST_STATUS {reply:?}");
    assert!(
        matches!(
            c7f_phase(&f),
            Some(ReviewPhase::Stopping | ReviewPhase::Stopped) | None
        ),
        "C7F4_QUEUED_STOP_PERSISTED {:?}",
        c7f_phase(&f)
    );
    assert!(
        c7f2_owner(&f).lock().unwrap().review_fixture_stops_queued() == 0,
        "C7F4_NO_STOP_LEFT_QUEUED"
    );
}
