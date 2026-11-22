// JA-1 S6 C4: `ListVersions` reports each version's sealed outcome, which
// the Studio ready line reads. Linux only, like the C7 fixtures.

#[cfg(target_os = "linux")]
#[test]
fn ja1_s6_c4_list_versions_reports_the_sealed_outcome() {
    let (job, receipt) = s6_sealed_job("A sealed title");
    let reply = ok(
        job.f.editor.request(HostCommand::ListVersions).unwrap(),
        "S6_C4_LIST_VERSIONS",
    );
    assert!(
        reply.versions.contains(&receipt.key.snapshot),
        "S6_C4_LISTED: {:?}",
        reply.versions
    );
    assert_eq!(
        reply.sealed_versions,
        vec![kinewright_project::review_job::delivery::SealedVersion {
            key: receipt.key.snapshot.clone(),
            number: 1,
            outcome: receipt.outcome.clone(),
            key_sha256: receipt.key_sha256.clone(),
        }],
        "S6_C4_LIST_SEALED"
    );
}

/// A private bus with a fake notification daemon, and the host's notifier
/// pointed at it. Nothing reaches the person's session bus.
#[cfg(target_os = "linux")]
fn s6c4_notify_on_private_bus(
    f: &Fixture,
    actions: bool,
) -> (
    super::super::notify::test_bus::PrivateBus,
    std::sync::Arc<super::super::notify::test_bus::Daemon>,
    zbus::blocking::Connection,
) {
    let bus = super::super::notify::test_bus::PrivateBus::start();
    let (daemon, server) = bus.daemon(actions);
    s6c4_install(f, Some(bus.connect()));
    (bus, daemon, server)
}

#[cfg(target_os = "linux")]
fn s6c4_install(f: &Fixture, connect: Option<super::super::notify::Connect>) {
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .review_runtime
        .notifier = super::super::notify::Notifier::new(connect);
}

#[cfg(target_os = "linux")]
fn s6c4_calls(
    daemon: &super::super::notify::test_bus::Daemon,
) -> Vec<super::super::notify::test_bus::NotifyCall> {
    daemon.calls.lock().unwrap().clone()
}

/// Seal `job` through the C7 driver.
#[cfg(target_os = "linux")]
fn s6c4_seal(job: &C7Job) -> kinewright_project::review_job::ReviewReceipt {
    use kinewright_project::review_job::SealResult;
    let mut reviewer = C7Reviewer(|_: &ReviewToken| {
        c7_judge(&job.reviewer);
        Ok(())
    });
    let cancel = kinewright_core::ExportCancellation::default();
    let SealResult::Sealed(sealed) =
        c7_drive(job, &c7_real_check, &mut reviewer, &cancel).expect("S6C4_DRIVER_SEALS")
    else {
        panic!("S6C4_SEALED: {:?}", c7_job_phase(job));
    };
    assert!(!sealed.replayed, "S6C4_FRESH_SEAL");
    sealed.receipt
}

#[cfg(target_os = "linux")]
fn s6c4_status(f: &Fixture) -> HostStatus {
    ok(f.editor.request(HostCommand::Status).unwrap(), "S6C4_STATUS").status
}

#[cfg(target_os = "linux")]
#[test]
fn ja1_s6_c4_a_seal_notifies_once_and_a_lost_reply_replay_sends_nothing() {
    use super::super::notify::test_bus::{click, wait_for};
    let job = c7_job(c7_title_doc("Hello world"));
    let (_bus, daemon, server) = s6c4_notify_on_private_bus(&job.f, true);
    let receipt = s6c4_seal(&job);
    wait_for(|| s6c4_calls(&daemon).len() == 1, "S6C4_HOST_SENT");
    let calls = s6c4_calls(&daemon);
    assert_eq!(
        calls[0].body,
        kinewright_project::review_job::delivery::seal_notice_text("v1", &receipt.outcome),
        "S6C4_HOST_LABEL"
    );
    assert_eq!(calls[0].body, "v1 is delivered", "S6C4_HOST_DELIVERED");
    // The fixture's Editor is the person-facing GUI: a click can select.
    assert_eq!(calls[0].actions, ["default", "Show"], "S6C4_HOST_ACTION");
    // A lost reply: Seal again returns the receipt and notifies nothing.
    let replay = ok(
        job.f
            .editor
            .request(HostCommand::Review(ReviewControl::Seal {
                token: job.token.clone(),
                command: kinewright_project::bundle_records::SessionId::fresh().unwrap(),
            }))
            .unwrap(),
        "S6C4_SEAL_REPLAY",
    );
    assert!(replay.review_receipt.is_some(), "S6C4_REPLAYED");
    // The click goes through status to the GUI, which acknowledges it.
    click(&server, calls[0].id);
    wait_for(
        || s6c4_status(&job.f).notification_select.is_some(),
        "S6C4_HOST_CLICK_IN_STATUS",
    );
    let select = s6c4_status(&job.f).notification_select.unwrap();
    assert_eq!(select.version, receipt.key.snapshot, "S6C4_HOST_SELECT_VERSION");
    assert_eq!(select.key_sha256, receipt.key_sha256, "S6C4_HOST_SELECT_DIGEST");
    let acknowledged = job
        .f
        .editor
        .request(HostCommand::Review(ReviewControl::AcknowledgeNotification {
            nonce: select.nonce,
        }))
        .unwrap();
    assert!(acknowledged.error.is_none(), "S6C4_ACK: {:?}", acknowledged.error);
    assert_eq!(
        acknowledged.status.notification_select, None,
        "S6C4_ACK_CLEARS_THE_SELECT"
    );
    assert_eq!(s6c4_calls(&daemon).len(), 1, "S6C4_REPLAY_SENDS_NOTHING");
}

#[cfg(target_os = "linux")]
#[test]
fn ja1_s6_c4_a_missing_bus_is_in_status_and_the_seal_still_succeeds() {
    let job = c7_job(c7_title_doc("Hello world"));
    s6c4_install(
        &job.f,
        Some(super::super::notify::test_bus::PrivateBus::refused()),
    );
    let receipt = s6c4_seal(&job);
    super::super::notify::test_bus::wait_for(
        || s6c4_status(&job.f).notify_error.is_some(),
        "S6C4_NOTIFY_ERROR_IN_STATUS",
    );
    assert!(
        s6c4_status(&job.f)
            .notify_error
            .unwrap()
            .contains("no session bus in this test"),
        "S6C4_NOTIFY_ERROR_TEXT"
    );
    let receipts = c7_receipts(&job);
    assert_eq!(receipts.len(), 1, "S6C4_SEALED_ANYWAY");
    assert_eq!(receipts[0].key_sha256, receipt.key_sha256, "S6C4_SEALED_ANYWAY");
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s6_c4_a_facts_not_checked_seal_is_never_called_delivered() {
    let job = c7_job(c7_title_doc("Step 1"));
    let (_bus, daemon, _server) = s6c4_notify_on_private_bus(&job.f, true);
    s6c4_seal(&job);
    super::super::notify::test_bus::wait_for(
        || s6c4_calls(&daemon).len() == 1,
        "S6C4_HOST_SENT",
    );
    assert_eq!(
        s6c4_calls(&daemon)[0].body,
        "v1 is reviewed, facts not checked",
        "S6C4_HOST_FACTS_NOT_CHECKED"
    );
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s6_c4_a_failed_seal_sends_nothing() {
    let job = c7_job_driven(c7f_fixture(c7_title_doc("Hello world")), true);
    let (_bus, daemon, _server) = s6c4_notify_on_private_bus(&job.f, true);
    c7f2_owner(&job.f).lock().unwrap().review_runtime.seal_hook =
        Some(c7f2_failing_seal(usize::MAX));
    c7_judge(&job.reviewer);
    s5_review_until(
        || matches!(c7f_phase(&job.f), Some(ReviewPhase::Paused { .. })),
        "S6C4_FAILED_SEAL_PAUSES",
    );
    s5_review_until(|| !c7f2_generation(&job.f).1, "S6C4_DRIVER_ENDS");
    assert!(c7f_receipts(&job.f, &job.token).is_empty(), "S6C4_NOT_SEALED");
    std::thread::sleep(Duration::from_millis(300));
    assert!(s6c4_calls(&daemon).is_empty(), "S6C4_FAILED_SEAL_SENDS_NOTHING");
}

#[cfg(target_os = "linux")]
#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn ja1_s6_c4_a_detached_host_notifies_without_an_action_then_exits_joined() {
    let job = s6c3_job(
        c7f_fixture(c7_title_doc("Hello world")),
        c7f_encode_without_status,
    );
    let (_bus, daemon, _server) = s6c4_notify_on_private_bus(&job.f, true);
    let probe = job
        .f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .review_runtime
        .notifier
        .probe();
    let (stills, _, _) = s5_c6_read_all_stills(&job.reviewer, 4);
    s6c3_beside_wait(&job.f, |waiting| {
        let detached = s6c3_detach(&job.f, "S6C4_DETACH");
        assert!(detached.retained_review, "S6C4_DETACH_RETAINS_THE_REVIEW");
        let reply = job.reviewer.wire(
            "submit_visual_judgment",
            serde_json::json!({
                "verdict": "pass",
                "reviewed_still_ids": stills.iter().map(|still| still.still_id.clone()).collect::<Vec<_>>(),
                "findings": []
            }),
        );
        assert_eq!(reply.is_error, Some(false), "S6C4_JUDGED: {reply:?}");
        s5_review_until(
            || !c7f_receipts(&job.f, &job.token).is_empty(),
            "S6C4_SEALED_DETACHED",
        );
        s6c3_end_run(&job, "S6C4");
        s6c3_await_exit(waiting, "S6C4_DETACHED_EXIT");
    });
    // The exit sent the queued notification before joining its thread.
    let calls = s6c4_calls(&daemon);
    assert_eq!(calls.len(), 1, "S6C4_DETACHED_SENT_ONCE");
    assert!(calls[0].actions.is_empty(), "S6C4_DETACHED_HAS_NO_ACTION");
    // The host has exited and dropped its owner; the notifier went with it.
    assert!(job.f.host.owner.upgrade().is_none(), "S6C4_HOST_GONE");
    assert!(probe.joined(), "S6C4_NOTIFIER_JOINED");
}
