// JA-1 S6 C2 fix 1: a save across Stop, Save As, a change of project and
// shutdown. Uses the fixtures of `review_s6_c2_tests.rs`. Linux only.

/// How many pins the project's store holds on `media` (by digest).
#[cfg(target_os = "linux")]
fn s6f1_pins(bundle: &kinewright_project::bundle::ProjectBundle, media: &str) -> u64 {
    bundle
        .store()
        .retention_probe()
        .unwrap()
        .1
        .get(media)
        .copied()
        .unwrap_or(0)
}

#[cfg(target_os = "linux")]
fn s6f1_active_pins(f: &Fixture, media: &str) -> u64 {
    let shared = f.host.owner.upgrade().unwrap();
    let owner = shared.lock().unwrap();
    s6f1_pins(owner.owner().unwrap().review_bundle().unwrap(), media)
}

/// A started save, stalled inside its copy, of a freshly sealed job.
#[cfg(target_os = "linux")]
struct S6f1Saving {
    job: C7Job,
    receipt: kinewright_project::review_job::ReviewReceipt,
    out: std::path::PathBuf,
    stall: S6Stall,
    pins_before: u64,
}

#[cfg(target_os = "linux")]
fn s6f1_saving(text: &str) -> S6f1Saving {
    let (job, receipt) = s6_sealed_job(text);
    let out = job.f.temp.path("out");
    std::fs::create_dir(&out).unwrap();
    let pins_before = s6f1_active_pins(&job.f, &receipt.key.media_ref.sha256);
    let stall = S6Stall::default();
    stall.install(&job.f);
    ok(
        job.f
            .editor
            .request(s6_save_command(&receipt, &out))
            .unwrap(),
        "S6F1_SAVE_STARTED",
    );
    stall.wait_entered(&job.f);
    assert!(
        s6f1_active_pins(&job.f, &receipt.key.media_ref.sha256) > pins_before,
        "S6F1_PINNED_WHILE_SAVING"
    );
    S6f1Saving {
        job,
        receipt,
        out,
        stall,
        pins_before,
    }
}

/// Whether the host still holds a save worker of any kind.
#[cfg(target_os = "linux")]
fn s6f1_workers(f: &Fixture) -> usize {
    let shared = f.host.owner.upgrade().unwrap();
    let owner = shared.lock().unwrap();
    usize::from(owner.review_runtime.save.is_some()) + owner.review_runtime.retired_saves.len()
}

/// Neon F5: `StopSave` over `/host` stops the running copy, leaves nothing,
/// releases the pin, and is refused when nothing is saving.
#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s6_c2_fix1_host_stop_save_stops_the_copy() {
    use kinewright_project::review_job::delivery::{SAVE_STOPPED, SealedSaveStatus};
    let saving = s6f1_saving("Hello world");
    let f = &saving.job.f;
    ok(
        f.editor
            .request(HostCommand::Review(ReviewControl::StopSave))
            .unwrap(),
        "S6F1_STOP_SAVE",
    );
    let started = std::time::Instant::now();
    let status = loop {
        match s6_save_status(f) {
            Some(SealedSaveStatus::Saving { .. }) => {}
            other => break other,
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(30),
            "S6F1_STOP_NEVER_LANDED"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert_eq!(
        status,
        Some(SealedSaveStatus::Failed {
            reason: SAVE_STOPPED.into()
        }),
        "S6F1_STOPPED_STATUS"
    );
    assert!(s6_entries(&saving.out).is_empty(), "S6F1_STOP_LEAVES_NOTHING");
    assert_eq!(
        s6f1_active_pins(f, &saving.receipt.key.media_ref.sha256),
        saving.pins_before,
        "S6F1_STOP_RELEASES_THE_PIN"
    );
    let idle = f
        .editor
        .request(HostCommand::Review(ReviewControl::StopSave))
        .unwrap();
    assert_eq!(
        s6_refusal(&idle),
        super::super::review_job::sealed_save::NO_SAVE,
        "S6F1_NOTHING_TO_STOP"
    );
}

/// Neon F3: Save As during a save stops and joins it before the reply, so
/// the source's store is let go, and no status of it reaches the copy.
#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s6_c2_fix1_save_as_mid_save_releases_the_source() {
    let saving = s6f1_saving("Hello world");
    let f = &saving.job.f;
    let source = saving.receipt.key.snapshot.project.clone();
    let reply = c7f2_save_as(f);
    assert!(reply.error.is_none(), "S6F1_SAVE_AS: {:?}", reply.error);
    // The worker is joined before the reply, so its pinned cut, the last
    // reference it held to the source's store, is gone. (The retained
    // source keeps pins of its own after a Save As, so the count is not
    // compared here; the switch witness compares it exactly.)
    assert_eq!(s6f1_workers(f), 0, "S6F1_SAVE_JOINED_BY_SAVE_AS");
    {
        let shared = f.host.owner.upgrade().unwrap();
        let owner = shared.lock().unwrap();
        let session = owner.owner().unwrap();
        assert_ne!(session.project_id().unwrap(), source, "S6F1_COPY_ACTIVE");
    }
    assert_eq!(s6_save_status(f), None, "S6F1_NO_STATUS_ON_THE_COPY");
    assert!(s6_entries(&saving.out).is_empty(), "S6F1_SAVE_AS_LEAVES_NOTHING");
    // The copy can save again: nothing of the source's save is running.
    let idle = f
        .editor
        .request(HostCommand::Review(ReviewControl::StopSave))
        .unwrap();
    assert_eq!(
        s6_refusal(&idle),
        super::super::review_job::sealed_save::NO_SAVE,
        "S6F1_NO_SOURCE_SAVE_LEFT"
    );
    saving.stall.release();
}

/// Neon F3: any change of project goes through `leave_project`; a save of
/// the project left is stopped, joined with the lock released, and its late
/// report never lands.
#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s6_c2_fix1_a_project_switch_mid_save_stops_and_forgets_it() {
    let saving = s6f1_saving("Hello world");
    let f = &saving.job.f;
    let shared = f.host.owner.upgrade().unwrap();
    shared
        .lock()
        .unwrap()
        .review_runtime
        .leave_project(&saving.receipt.key.snapshot.project);
    // The worker may report only after the switch; release it now so a
    // late report would land if it were not fenced.
    saving.stall.release();
    HostCommandOwner::join_retired_review_drivers(&shared);
    assert_eq!(s6f1_workers(f), 0, "S6F1_SWITCH_JOINS_THE_SAVE");
    assert!(
        shared.lock().unwrap().review_runtime.save_status.is_none(),
        "S6F1_SWITCH_LEAVES_NO_STATUS"
    );
    assert_eq!(
        s6f1_active_pins(f, &saving.receipt.key.media_ref.sha256),
        saving.pins_before,
        "S6F1_SWITCH_RELEASES_THE_PIN"
    );
    let entries = s6_entries(&saving.out);
    assert!(
        entries.is_empty(),
        "S6F1_SWITCH_PUBLISHES_NOTHING: {entries:?}"
    );
}

/// Neon F5: a shutdown mid-save lets the project go: the next host opens it,
/// and nothing was saved.
#[test]
#[cfg(target_os = "linux")]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s6_c2_fix1_shutdown_mid_save_lets_the_project_reopen() {
    let saving = s6f1_saving("Hello world");
    let S6f1Saving {
        job,
        out,
        receipt,
        pins_before,
        ..
    } = saving;
    // The restart drops the host, which cancels and joins the save.
    let (f, ..) = c7f_restart(job, false);
    assert!(s6_entries(&out).is_empty(), "S6F1_SHUTDOWN_LEAVES_NOTHING");
    assert_eq!(
        s6f1_active_pins(&f, &receipt.key.media_ref.sha256),
        pins_before,
        "S6F1_REOPENED_WITH_ONLY_ITS_OWN_PINS"
    );
    assert_eq!(s6_save_status(&f), None, "S6F1_REOPENED_WITH_NO_SAVE");
}
