//! Kept pre-dispatch requests follow host recovery, never a timer or a new identity.
use super::*;
use kinewright_agent::writer_host::{
    HostCommand, HostRefusal, ProtocolFixture, ProtocolFixtureMode, RecoveryPhase,
    RecoveryReadiness,
};
use std::{sync::Arc, time::Duration};

fn in_app(project: &mut Project, attachment: Ja1Attachment) -> KinewrightApp {
    let (mut app, _) = in1_harness(Document::default());
    app.projects[0].recovery.stop_for_studio();
    app.projects[0].ja1 = Some(attachment);
    app.focused_mut().studio.feedback = std::mem::take(&mut project.feedback);
    app
}
fn poll(app: &mut KinewrightApp) {
    app.focused_mut().studio.feedback.observed_at = None;
    app.feedback_frame();
}
fn assert_once(app: KinewrightApp, project: &Project, request: &ReviseRequest) {
    assert!(
        app.focused().studio.feedback.revise.is_none(),
        "C7_RETRY_SETTLED"
    );
    drop(app);
    let (batches, runs) = linked(&project.path);
    assert_eq!(runs, 1, "C7_RETRY_ONE_RUN");
    assert_eq!(batches.len(), 1);
    assert!(batches[0].is_some(), "C7_RETRY_ONE_CONSUMPTION");
    let owner =
        kinewright_project::editing_session::Ja1EditingSession::open(&project.path).unwrap();
    assert_eq!(
        owner
            .read_submitted_batch(&request.batch_id)
            .unwrap()
            .consumed_by,
        batches[0]
    );
}
fn retry_revise(mode: ProtocolFixtureMode) {
    let mut project = project("c7-retry-startup");
    let request = c7_unconsumed_batch(&mut project);
    let fixture = ProtocolFixture::new(project.temp.root(), mode);
    let attachment = launch_with_fixture(&project, &fixture);
    assert!(fixture.wait_recovery(RecoveryPhase::Entered, Duration::from_secs(5)));
    let mut app = in_app(&mut project, attachment);
    if mode == ProtocolFixtureMode::RecoveryFailsOnce {
        std::thread::scope(|scope| {
            let release = scope.spawn(|| {
                let queued = fixture.wait_recovery(RecoveryPhase::Queued, Duration::from_secs(5));
                fixture.release_recovery();
                queued
            });
            app.with_feedback(|feedback, port| feedback.submit(port))
                .unwrap();
            assert!(release.join().unwrap());
        });
        assert!(fixture.wait_recovery(RecoveryPhase::Completed, Duration::from_secs(5)));
    } else {
        app.with_feedback(|feedback, port| feedback.submit(port))
            .unwrap();
    }
    assert!(
        app.focused().studio.feedback.revise_recovery.is_some(),
        "C7_RECOVERY_RETRY_ARMED"
    );
    assert_eq!(
        app.focused().studio.feedback.revise.as_ref(),
        Some(&request)
    );
    assert_eq!(
        app.with_feedback(|_, port| port.recovery_readiness().unwrap())
            .unwrap(),
        RecoveryReadiness::Blocked
    );
    let before = fixture.dispatch_counts();
    let message = app.focused().studio.feedback.message.clone();
    poll(&mut app);
    poll(&mut app);
    assert_eq!(
        fixture.dispatch_counts(),
        before,
        "C7_BLOCKED_POLLS_DO_NOT_SEND"
    );
    assert_eq!(
        app.focused().studio.feedback.message,
        message,
        "C7_BLOCKED_REASON_STAYS_VISIBLE"
    );
    fixture.clear_recovery_completed();
    fixture.release();
    assert!(fixture.wait_recovery(RecoveryPhase::Completed, Duration::from_secs(5)));
    poll(&mut app);
    assert!(
        app.focused().studio.feedback.revise.is_none(),
        "C7_HEALTHY_RESENDS_KEPT_REVISE"
    );
    poll(&mut app);
    assert_eq!(
        fixture.dispatch_counts().0,
        before.0 + 1,
        "C7_ONE_RECOVERY_RESEND"
    );
    assert_once(app, &project, &request);
}
#[test]
fn c7_retry_failed_recovery_then_healthy() {
    retry_revise(ProtocolFixtureMode::RecoveryFailsOnce);
}
#[test]
fn c7_retry_expired_startup_then_healthy() {
    retry_revise(ProtocolFixtureMode::RecoveryExpired);
}

#[test]
fn c7_retry_busy_startup_then_healthy() {
    let mut project = project("c7-retry-busy");
    let request = c7_unconsumed_batch(&mut project);
    let fixture = ProtocolFixture::new(project.temp.root(), ProtocolFixtureMode::RecoveryHeld);
    let attachment = launch_with_fixture(&project, &fixture);
    let client = attachment.host_client().unwrap();
    let mut app = in_app(&mut project, attachment);
    std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            client.request(HostCommand::ReviseCut {
                request: request.clone(),
            })
        });
        let queued = fixture.wait_recovery(RecoveryPhase::Queued, Duration::from_secs(5));
        if queued {
            app.with_feedback(|feedback, port| feedback.submit(port))
                .unwrap();
        }
        let armed = app.focused().studio.feedback.revise_recovery.is_some();
        let dispatched = fixture.dispatch_counts().0;
        // Release the first request even when the mutant failed to arm its retry.
        fixture.release();
        let first_reply = first.join().unwrap();
        assert!(queued, "C7_BUSY_WAITER_HELD");
        assert!(armed, "C7_BUSY_REQUEST_KEPT");
        assert_eq!(dispatched, 0);
        assert!(first_reply.unwrap().error.is_none());
    });
    assert!(fixture.wait_recovery(RecoveryPhase::Completed, Duration::from_secs(5)));
    poll(&mut app);
    assert!(
        app.focused().studio.feedback.revise.is_none(),
        "C7_BUSY_RECOVERY_RESEND"
    );
    poll(&mut app);
    assert_eq!(fixture.dispatch_counts().0, 2, "C7_BUSY_EXACT_REPLAY_ONCE");
    assert_once(app, &project, &request);
}

fn restored_stop(busy: bool) {
    let mut project = project("c7-retry-stop");
    let first = ProtocolFixture::new(project.temp.root(), ProtocolFixtureMode::Dormant);
    let mut attachment = launch_with_fixture(&project, &first);
    assert!(first.wait_recovery(RecoveryPhase::Completed, Duration::from_secs(5)));
    project.feedback.issuer = None;
    with_host(&attachment, |port| project.feedback.submit(port));
    let run = project.feedback.rows[0].run.clone().unwrap();
    // The old host accepted Stop, but its reply was lost. A never-sent old-epoch
    // command must refuse; the restored exact receipt is the supported replay.
    let proxy = attachment.drop_host_reply("dispatch_orchestration", false);
    let reply = with_host(&attachment, |port| project.feedback.stop_run(port, &run)).unwrap();
    assert!(matches!(reply, stop::ControlReply::Unknown(_)));
    assert!(proxy.dropped.load(std::sync::atomic::Ordering::Acquire));
    let control = project.feedback.controls[0].request.clone();
    let stopped = with_host(&attachment, |port| port.run_view(&run).unwrap().projection);
    assert!(
        matches!(stopped.outcome, RunOutcome::Cancelled(_)),
        "C7_STOP_COMMITTED_BEFORE_RESTART"
    );
    project.feedback.persist().unwrap();
    drop(proxy);
    let directory = project.temp.path("state");
    project.feedback = Feedback::default();
    drop(attachment);
    let fixture = Arc::new(ProtocolFixture::new(
        project.temp.root(),
        ProtocolFixtureMode::RecoveryHeld,
    ));
    let attachment = launch_with_fixture(&project, &fixture);
    assert!(fixture.wait_recovery(RecoveryPhase::Entered, Duration::from_secs(5)));
    project
        .feedback
        .open(attachment.studio_project_id().map_err(text), Ok(directory))
        .unwrap();
    assert!(project.feedback.resume);
    let mut app = in_app(&mut project, attachment);
    app.feedback_frame();
    assert_eq!(app.focused().studio.feedback.controls[0].request, control);
    assert!(
        app.focused().studio.feedback.controls[0].recovery.is_some(),
        "C7_STOP_RECOVERY_ARMED"
    );
    assert_eq!(fixture.dispatch_counts().1, 1);
    poll(&mut app);
    assert_eq!(fixture.dispatch_counts().1, 1);
    fixture.release();
    assert!(fixture.wait_recovery(RecoveryPhase::Completed, Duration::from_secs(5)));
    if busy {
        let holder_fixture = Arc::clone(&fixture);
        let client = app.focused().ja1.as_ref().unwrap().host_client().unwrap();
        let lane = Arc::new(std::sync::Mutex::new(None));
        let lane_result = Arc::clone(&lane);
        app.focused_mut().studio.feedback.recovery_ready_witness = Some(Box::new(move || {
            holder_fixture.hold_next_status();
            let thread = std::thread::spawn(move || client.request(HostCommand::Status));
            assert!(holder_fixture.wait_status(Duration::from_secs(5)));
            *lane_result.lock().unwrap() = Some(thread);
        }));
        poll(&mut app);
        let armed = app.focused().studio.feedback.controls[0].recovery.is_some();
        let message = app.focused().studio.feedback.message.clone().unwrap();
        fixture.release_status();
        assert!(lane.lock().unwrap().take().unwrap().join().unwrap().is_ok());
        assert!(armed, "C7_BUSY_STOP_KEEPS_ARM");
        assert_eq!(app.focused().studio.feedback.controls[0].request, control);
        assert!(message.contains("kept and waiting") && !message.contains("unknown"));
        assert_eq!(fixture.dispatch_counts().1, 1);
    }
    poll(&mut app);
    assert!(
        app.focused().studio.feedback.controls.is_empty(),
        "C7_HEALTHY_RESENDS_STOP"
    );
    poll(&mut app);
    assert_eq!(fixture.dispatch_counts().1, 2, "C7_STOP_RESENT_ONCE");
    let final_state = app
        .with_feedback(|_, port| port.run_view(&run).unwrap().projection)
        .unwrap();
    assert!(matches!(final_state.outcome, RunOutcome::Cancelled(_)));
    assert_eq!(
        final_state.sequence, stopped.sequence,
        "C7_STOP_APPLIES_ONLY_ONCE"
    );
}

#[test]
fn c7_retry_restored_stop_then_healthy() {
    restored_stop(false);
}
#[test]
fn c7_followup_busy_restored_stop_keeps_exact_arm() {
    restored_stop(true);
}

#[test]
fn c7_retry_stopping_and_nonstartup_unknowns_stay_manual() {
    for code in [
        "host_stopping",
        "durability_unknown",
        "unsettled",
        "attachment_refused",
        "history_changed",
    ] {
        assert!(
            !HostRefusal::new(code, "kept").retry_after_recovery(),
            "C7_NO_UNSAFE_AUTOMATIC_RETRY {code}"
        );
    }
    let mut project = project("c7-retry-shutdown");
    let request = c7_unconsumed_batch(&mut project);
    let fixture = ProtocolFixture::new(project.temp.root(), ProtocolFixtureMode::RecoveryExpired);
    let attachment = launch_with_fixture(&project, &fixture);
    let client = attachment.host_client().unwrap();
    let mut app = in_app(&mut project, attachment);
    app.with_feedback(|feedback, port| feedback.submit(port))
        .unwrap();
    assert!(
        client
            .request(HostCommand::Shutdown)
            .unwrap()
            .error
            .is_none()
    );
    poll(&mut app);
    fixture.release();
    poll(&mut app);
    assert_eq!(
        app.focused().studio.feedback.revise.as_ref(),
        Some(&request)
    );
    assert_eq!(fixture.dispatch_counts().0, 0, "C7_STOPPING_NEVER_RESENDS");
    drop(app);
    assert_eq!(linked(&project.path), (vec![None], 0));
}

fn manual_retry_order(auto_first: bool) {
    let mut project = project("c7-retry-manual-gate");
    let request = c7_unconsumed_batch(&mut project);
    let fixture = ProtocolFixture::new(project.temp.root(), ProtocolFixtureMode::RecoveryExpired);
    let attachment = launch_with_fixture(&project, &fixture);
    let mut app = in_app(&mut project, attachment);
    app.with_feedback(|feedback, port| feedback.submit(port))
        .unwrap();
    fixture.release();
    assert!(fixture.wait_recovery(RecoveryPhase::Completed, Duration::from_secs(5)));
    let gate = std::sync::Arc::new((
        std::sync::Mutex::new((false, false)),
        std::sync::Condvar::new(),
    ));
    let ui_gate = std::sync::Arc::clone(&gate);
    app.focused_mut().studio.feedback.recovery_ready_witness = Some(Box::new(move || {
        let (lock, changed) = &*ui_gate;
        let mut state = lock.lock().unwrap();
        state.0 = true;
        changed.notify_all();
        let (state, _) = changed
            .wait_timeout_while(state, Duration::from_secs(5), |state| !state.1)
            .unwrap();
        assert!(state.1, "C7_MANUAL_GATE_RELEASED");
    }));
    let sender = app.ui_replies.0.clone(); // The pending click arrives while the frame owns Feedback.
    std::thread::scope(|scope| {
        let click = scope.spawn(|| {
            let (lock, changed) = &*gate;
            let state = lock.lock().unwrap();
            let (state, _) = changed
                .wait_timeout_while(state, Duration::from_secs(5), |state| !state.0)
                .unwrap();
            let entered = state.0;
            drop(state);
            if entered {
                sender
                    .send(Box::new(|app, _| {
                        app.with_feedback(|feedback, port| feedback.submit(port))
                            .unwrap();
                    }))
                    .unwrap();
            }
            lock.lock().unwrap().1 = true;
            changed.notify_all();
            entered
        });
        if !auto_first {
            assert_eq!(
                app.with_feedback(|_, port| port.recovery_readiness().unwrap())
                    .unwrap(),
                RecoveryReadiness::Ready
            );
            // A pending UI click is dispatched before this frame's recovery poll.
            app.focused_mut()
                .studio
                .feedback
                .recovery_ready_witness
                .take()
                .unwrap()();
            let ctx = egui::Context::default();
            while let Ok(reply) = app.ui_replies.1.try_recv() {
                reply(&mut app, &ctx);
            }
        }
        poll(&mut app);
        assert!(click.join().unwrap(), "C7_HEALTHY_TRANSITION_GATED");
    });
    let ctx = egui::Context::default();
    while let Ok(reply) = app.ui_replies.1.try_recv() {
        reply(&mut app, &ctx);
    }
    assert_eq!(
        fixture.dispatch_counts().0,
        1,
        "C7_MANUAL_AND_POLL_ONE_DISPATCH"
    );
    // Later polls do not dispatch a second request in either ordering.
    poll(&mut app);
    assert_eq!(fixture.dispatch_counts().0, 1);
    assert_once(app, &project, &request);
}

#[test]
fn c7_retry_manual_click_queued_at_healthy_transition_dispatches_once() {
    manual_retry_order(true);
}
#[test]
fn c7_retry_manual_click_before_healthy_poll_dispatches_once() {
    manual_retry_order(false);
}

#[test]
fn c7_retry_project_switch_cancels_automatic_send_but_keeps_request() {
    let mut project = project("c7-retry-switch");
    let request = c7_unconsumed_batch(&mut project);
    let fixture = ProtocolFixture::new(project.temp.root(), ProtocolFixtureMode::RecoveryExpired);
    let attachment = launch_with_fixture(&project, &fixture);
    let mut app = in_app(&mut project, attachment);
    app.with_feedback(|feedback, port| feedback.submit(port))
        .unwrap();
    assert!(app.focused().studio.feedback.revise_recovery.is_some());
    app.new_project();
    app.focus_project(0);
    let cancelled = app.focused().studio.feedback.revise_recovery.is_none();
    fixture.release();
    assert!(fixture.wait_recovery(RecoveryPhase::Completed, Duration::from_secs(5)));
    assert!(cancelled, "C7_SWITCH_CANCELS_AUTOMATIC_ARM");
    poll(&mut app);
    assert_eq!(
        app.focused().studio.feedback.revise.as_ref(),
        Some(&request)
    );
    assert_eq!(
        fixture.dispatch_counts().0,
        0,
        "C7_SWITCH_NEVER_SENDS_LATER"
    );
    drop(app);
    assert_eq!(linked(&project.path), (vec![None], 0));
}

#[test]
fn c7_retry_replaced_host_cancels_automatic_send_but_keeps_request() {
    let mut project = project("c7-retry-host-change");
    let request = c7_unconsumed_batch(&mut project);
    let first = ProtocolFixture::new(project.temp.root(), ProtocolFixtureMode::RecoveryExpired);
    let attachment = launch_with_fixture(&project, &first);
    with_host(&attachment, |port| project.feedback.submit(port));
    assert!(project.feedback.revise_recovery.is_some());
    first.release();
    drop(attachment);
    let next = ProtocolFixture::new(project.temp.root(), ProtocolFixtureMode::Dormant);
    let attachment = launch_with_fixture(&project, &next);
    assert!(next.wait_recovery(RecoveryPhase::Completed, Duration::from_secs(5)));
    with_host(&attachment, |port| {
        project.feedback.retry_after_recovery(port);
    });
    assert!(
        project.feedback.revise_recovery.is_none(),
        "C7_REPLACED_HOST_CANCELS_ARM"
    );
    assert_eq!(project.feedback.revise.as_ref(), Some(&request));
    assert_eq!(next.dispatch_counts().0, 0);
    drop(attachment);
    assert_eq!(linked(&project.path), (vec![None], 0));
}

#[test]
fn c7_followup_panicked_recovery_releases_waiter_and_recovers() {
    let mut project = project("c7-followup-panic");
    let request = c7_unconsumed_batch(&mut project);
    let fixture =
        ProtocolFixture::new(project.temp.root(), ProtocolFixtureMode::RecoveryPanicsOnce);
    let attachment = launch_with_fixture(&project, &fixture);
    assert!(fixture.wait_recovery(RecoveryPhase::Entered, Duration::from_secs(5)));
    let client = attachment.host_client().unwrap();
    let completion = (std::sync::Mutex::new(false), std::sync::Condvar::new());
    let (reply, prompt, waiting) = std::thread::scope(|scope| {
        let control = scope.spawn(|| {
            let waiting = fixture.wait_recovery(RecoveryPhase::Waiting, Duration::from_secs(5));
            fixture.release_recovery();
            let (done, _) = completion
                .1
                .wait_timeout_while(
                    completion.0.lock().unwrap(),
                    Duration::from_secs(2),
                    |done| !*done,
                )
                .unwrap();
            let prompt = *done;
            drop(done);
            // A broken guard is released through the real shutdown fence, not a 55 s wait.
            if !prompt {
                let _ = client.request(HostCommand::Shutdown);
            }
            (prompt, waiting)
        });
        let reply = client.request(HostCommand::ReviseCut {
            request: request.clone(),
        });
        *completion.0.lock().unwrap() = true;
        completion.1.notify_all();
        let (prompt, waiting) = control.join().unwrap();
        (reply, prompt, waiting)
    });
    if !prompt {
        fixture.release();
    }
    assert!(waiting);
    assert!(prompt, "C7_PANIC_RELEASES_WAITER");
    let kinewright_agent::writer_host::HostError::Refused(refusal) = reply.unwrap_err() else {
        panic!("panic must be a typed pre-dispatch refusal")
    };
    assert_eq!(refusal.code, "startup_wait_failed");
    assert!(refusal.retry_after_recovery());
    assert_eq!(
        with_host(&attachment, |port| port.recovery_readiness().unwrap()),
        RecoveryReadiness::Blocked
    );
    assert_eq!(fixture.dispatch_counts().0, 0);
    fixture.release();
    assert!(fixture.wait_recovery(RecoveryPhase::Completed, Duration::from_secs(5)));
    assert_eq!(
        with_host(&attachment, |port| port.recovery_readiness().unwrap()),
        RecoveryReadiness::Ready
    );
    drop(attachment);
    assert_eq!(linked(&project.path), (vec![None], 0));
    assert!(fixture.observations().is_empty());
}

#[test]
fn c7_followup_busy_resend_keeps_exact_arm_and_one_status_per_tick() {
    let mut project = project("c7-followup-busy");
    let request = c7_unconsumed_batch(&mut project);
    let fixture = Arc::new(ProtocolFixture::new(
        project.temp.root(),
        ProtocolFixtureMode::RecoveryExpired,
    ));
    let attachment = launch_with_fixture(&project, &fixture);
    let client = attachment.host_client().unwrap();
    let mut app = in_app(&mut project, attachment);
    app.with_feedback(|feedback, port| feedback.submit(port))
        .unwrap();
    fixture.release();
    assert!(fixture.wait_recovery(RecoveryPhase::Completed, Duration::from_secs(5)));
    // The healthy reply is already read when the other real lane takes ingress.
    fixture.hold_next_status();
    let lane = std::thread::spawn(move || client.request(HostCommand::Status));
    assert!(fixture.wait_status(Duration::from_secs(5)));
    // A busy poll itself never clears a recovery arm or fabricates readiness.
    poll(&mut app);
    let busy_poll_armed = app.focused().studio.feedback.revise_recovery.is_some();
    fixture.release_status();
    assert!(lane.join().unwrap().is_ok());
    assert!(busy_poll_armed);

    // Hold the other lane specifically between the healthy Status and its resend.
    let fixture_for_lane = Arc::clone(&fixture);
    let client = app.focused().ja1.as_ref().unwrap().host_client().unwrap();
    let lane = Arc::new(std::sync::Mutex::new(None));
    let lane_result = Arc::clone(&lane);
    app.focused_mut().studio.feedback.recovery_ready_witness = Some(Box::new(move || {
        fixture_for_lane.hold_next_status();
        let thread = std::thread::spawn(move || client.request(HostCommand::Status));
        assert!(fixture_for_lane.wait_status(Duration::from_secs(5)));
        *lane_result.lock().unwrap() = Some(thread);
    }));
    poll(&mut app);
    let armed = app.focused().studio.feedback.revise_recovery.is_some();
    let kept = app.focused().studio.feedback.revise.as_ref() == Some(&request);
    let message = app.focused().studio.feedback.message.clone().unwrap();
    fixture.release_status();
    assert!(lane.lock().unwrap().take().unwrap().join().unwrap().is_ok());
    assert!(armed && kept, "C7_BUSY_RESEND_KEEPS_ARM");
    assert!(
        message.contains("kept and waiting") && !message.contains("unknown"),
        "{message}"
    );
    assert_eq!(fixture.dispatch_counts().0, 0);
    let before = fixture.status_count();
    poll(&mut app);
    assert_eq!(fixture.status_count() - before, 1, "C7_ONE_STATUS_PER_TICK");
    assert_eq!(fixture.dispatch_counts().0, 1);
    let before = fixture.status_count();
    poll(&mut app);
    assert_eq!(fixture.status_count() - before, 1);
    assert_once(app, &project, &request);
}

#[test]
fn c7_followup_manual_busy_stays_manual_with_truthful_words() {
    let mut project = project("c7-followup-manual-busy");
    let request = c7_unconsumed_batch(&mut project);
    let fixture = ProtocolFixture::new(project.temp.root(), ProtocolFixtureMode::Dormant);
    let attachment = launch_with_fixture(&project, &fixture);
    assert!(fixture.wait_recovery(RecoveryPhase::Completed, Duration::from_secs(5)));
    let client = attachment.host_client().unwrap();
    fixture.hold_next_status();
    let lane = std::thread::spawn(move || client.request(HostCommand::Status));
    assert!(fixture.wait_status(Duration::from_secs(5)));
    with_host(&attachment, |port| project.feedback.submit(port));
    let message = project.feedback.message.clone().unwrap();
    fixture.release_status();
    assert!(lane.join().unwrap().is_ok());
    assert!(project.feedback.revise_recovery.is_none());
    assert_eq!(project.feedback.revise.as_ref(), Some(&request));
    assert!(
        message.contains("kept and waiting") && !message.contains("unknown"),
        "{message}"
    );
    with_host(&attachment, |port| {
        project.feedback.retry_after_recovery(port);
    });
    assert_eq!(fixture.dispatch_counts().0, 0);
    with_host(&attachment, |port| project.feedback.submit(port));
    assert_eq!(fixture.dispatch_counts().0, 1);
    drop(attachment);
    assert_eq!(linked(&project.path).1, 1);
}
