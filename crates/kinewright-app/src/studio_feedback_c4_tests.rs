//! C4 uses the real Notes port and native host runtime, with protocol-only profiles.
#![allow(
    clippy::too_many_lines,
    reason = "one complete Stop lifecycle retains its authority, persistence, cleanup and history oracles"
)]
use super::*;
use crate::ja1_attachment::Ja1Attachment;
use kinewright_agent::writer_host::{
    HostCommand, ProtocolFixture, ProtocolFixtureMode, SessionRoots,
};
use std::{
    os::unix::fs::PermissionsExt,
    time::{Duration, Instant},
};

fn wait(mut ready: impl FnMut() -> bool, marker: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready() {
        assert!(Instant::now() < deadline, "{marker}");
        std::thread::park_timeout(Duration::from_millis(2));
    }
}
fn queue_message(
    port: &mut dyn FeedbackPort,
    run: &RunId,
) -> kinewright_project::orchestration_records::EffectId {
    let page = port.run_view(run).unwrap();
    let mut issuer = None;
    let mut request = port
        .prepare_stop(run, &page.projection.control_revision, &mut issuer)
        .unwrap();
    let kinewright_project::orchestration_records::OrchestrationCommand::ControlRun(intent) =
        &mut request.envelope.command
    else {
        unreachable!()
    };
    intent.action = kinewright_project::orchestration_records::RunAction::SendMessage {
        text_ref: page.projection.tasks[0].brief.assignment.clone(),
    };
    request.envelope.digest = request.envelope.computed_digest().unwrap();
    assert_eq!(
        port.send_stop(&request),
        stop::StopReply::Accepted,
        "C4_QUEUED_MESSAGE_ACK"
    );
    let page = port.run_view(run).unwrap();
    let effect = page.projection.effects.last().unwrap();
    assert_eq!(
        effect.state,
        kinewright_project::orchestration_records::EffectState::Pending,
        "C4_WORK_IS_QUEUED"
    );
    effect.id.clone()
}
fn with_port<R>(attachment: &Ja1Attachment, act: impl FnOnce(&mut HostFeedback<'_>) -> R) -> R {
    let client = attachment.host_client().unwrap();
    act(&mut HostFeedback {
        client: &client,
        reader: attachment,
    })
}
#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c4_notes_stop_startup_full_queue_and_execution() {
    for mode in [
        ProtocolFixtureMode::Startup,
        ProtocolFixtureMode::FullQueue,
        ProtocolFixtureMode::Normal,
    ] {
        let mut project = c7::project("c4-notes-native");
        let setting = std::fs::read(project.temp.path("scheduler/director.json")).unwrap();
        let fixture = ProtocolFixture::new(project.temp.root(), mode);
        let owner = Ja1EditingSession::open(&project.path).unwrap();
        let mut attachment = Ja1Attachment::new(owner).unwrap();
        let writer = attachment
            .launch_protocol_fixture(
                SessionRoots {
                    read: vec![project.temp.root().into()],
                    write: vec![project.temp.root().into()],
                },
                project.temp.path("scheduler"),
                &fixture,
            )
            .unwrap();
        project.feedback.issuer = None;
        let Outcome::Submitted(_) = with_port(&attachment, |port| project.feedback.submit(port))
        else {
            panic!("C4_NOTES_SUBMITTED {:?}", project.feedback.message);
        };
        let run = project.feedback.rows[0].run.clone().unwrap();
        let saved_notes = project
            .feedback
            .saved
            .iter()
            .map(|note| note.reference.clone())
            .collect::<Vec<_>>();
        let manual = attachment.owner.core().export_history().unwrap();
        wait(|| fixture.startup_entered(), "C4_APP_STARTUP_BARRIER");
        if mode != ProtocolFixtureMode::Startup {
            wait(
                || {
                    fixture
                        .observations()
                        .first()
                        .is_some_and(|o| o.scratch.join("execution-ready").exists())
                },
                "C4_APP_NATIVE_EXECUTING",
            );
        }
        if mode == ProtocolFixtureMode::FullQueue {
            wait(
                || fixture.event_entered() && fixture.native_queue_full(),
                "C4_APP_REAL_FULL_QUEUE",
            );
        }
        if mode == ProtocolFixtureMode::Normal {
            wait(
                || {
                    with_port(&attachment, |port| port.run_view(&run).unwrap())
                        .accounting
                        .as_ref()
                        .is_some_and(|record| {
                            record
                                .tasks
                                .iter()
                                .any(|task| task.totals.known.input == Some(1))
                        })
                },
                "C4_APP_RAW_USAGE_DURABLE",
            );
        }
        let viewer = writer
            .attach(
                "c4-notes-viewer".into(),
                kinewright_agent::writer_host::AttachmentRole::Viewer,
                SessionRoots {
                    read: vec![project.temp.root().into()],
                    write: vec![project.temp.root().into()],
                },
            )
            .unwrap();
        let denied = project.feedback.stop_run(
            &mut HostFeedback {
                client: &viewer,
                reader: &attachment,
            },
            &run,
        );
        assert!(denied.is_err(), "C4_VIEWER_STOP_REFUSED");
        assert!(
            project.feedback.stops.is_empty(),
            "C4_VIEWER_HAS_NO_KEPT_INTENT"
        );
        let queued = with_port(&attachment, |port| queue_message(port, &run));
        with_port(&attachment, |port| project.feedback.observe_runs(port));
        let page = with_port(&attachment, |port| port.run_view(&run).unwrap());
        if mode != ProtocolFixtureMode::Startup {
            assert!(
                page.projection.attempts.iter().any(|attempt| attempt.state
                    == kinewright_project::orchestration_records::AttemptState::Executing),
                "C4_APP_STOP_EXECUTING_CHILD"
            );
        }
        assert!(
            project.feedback.rows[0]
                .state
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .can_stop
        );
        project.feedback.toggle(&saved_notes[0]);
        project.feedback.persist().unwrap();
        let selection = project.feedback.selection.clone();
        let path = match &project.feedback.keeping {
            Keeping::Kept { path, .. } => path.clone(),
            _ => panic!("C4_NOTES_KEPT"),
        };
        if mode == ProtocolFixtureMode::Startup {
            let bytes = std::fs::read(&path).unwrap();
            let directory = path.parent().unwrap();
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o500)).unwrap();
            let refused = with_port(&attachment, |port| project.feedback.stop_run(port, &run));
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700)).unwrap();
            assert!(refused.is_err(), "C4_FAILED_INTENT_NEVER_DISPATCHED");
            assert!(project.feedback.stops.is_empty());
            assert_eq!(
                std::fs::read(&path).unwrap(),
                bytes,
                "C4_FAILED_INTENT_KEEPS_STATE"
            );
            assert_eq!(
                with_port(&attachment, |port| port.run_view(&run).unwrap())
                    .projection
                    .outcome,
                RunOutcome::Pending,
                "C4_FAILED_INTENT_RUN_UNCHANGED"
            );
        }
        let reply = with_port(&attachment, |port| {
            let mut lossy = Lossy::new(port);
            lossy.crash_at_dispatch = Some(path.clone());
            lossy.after = 1;
            let reply = project.feedback.stop_run(&mut lossy, &run).unwrap();
            assert_eq!(lossy.calls, 1, "C4_ONE_STOP_DISPATCH");
            assert_eq!(lossy.seen.len(), 1, "C4_STOP_DURABLE_BEFORE_SEND");
            reply
        });
        assert!(matches!(reply, stop::StopReply::Unknown(_)));
        let original = project.feedback.stops[0].request.clone();
        let kept: Kept = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(kept.stops[0].request, original, "C4_EXACT_STOP_RETRY_KEPT");
        if mode == ProtocolFixtureMode::Startup {
            assert!(
                project.feedback.rows[0].label().contains("cleanup pending"),
                "C4_NOTES_CLEANUP_PENDING"
            );
            assert!(
                !project.feedback.rows[0].settled(),
                "C4_NOT_STOPPED_BEFORE_PROOF"
            );
        }
        let project_id = project.feedback.keeping.project().unwrap().clone();
        fixture.release();
        wait(
            || {
                let _ = attachment
                    .host_client()
                    .unwrap()
                    .request(HostCommand::Status);
                with_port(&attachment, |port| project.feedback.observe_runs(port));
                project.feedback.rows[0].settled()
                    && !fixture.claim_root().join("run.json").exists()
            },
            "C4_NOTES_CLEANUP_PROOF",
        );
        // Restart both owners after the accepted reply was lost. The original
        // request retains its old epoch/revision and resolves to its exact receipt.
        drop(attachment);
        drop(writer);
        let mut attachment =
            Ja1Attachment::new(Ja1EditingSession::open(&project.path).unwrap()).unwrap();
        let writer = attachment
            .launch_protocol_fixture(
                SessionRoots {
                    read: vec![project.temp.root().into()],
                    write: vec![project.temp.root().into()],
                },
                project.temp.path("scheduler"),
                &fixture,
            )
            .unwrap();
        let fresh = attachment
            .host_client()
            .unwrap()
            .request(HostCommand::IssueMetadataIssuer { issuer: None })
            .unwrap()
            .metadata_issuer
            .unwrap();
        assert_ne!(
            fresh.envelope.owner_epoch, original.envelope.authority.owner_epoch,
            "C4_STOP_REOPEN_CHANGED_EPOCH"
        );
        project.feedback = Feedback::default();
        project
            .feedback
            .open(Ok(project_id), Ok(project.temp.path("state")))
            .unwrap();
        assert_eq!(
            project.feedback.stops[0].request, original,
            "C4_STOP_REOPEN_SAME_ENVELOPE"
        );
        with_port(&attachment, |port| project.feedback.resume(port));
        assert!(
            project.feedback.stops.is_empty(),
            "C4_STOP_ORIGINAL_RECEIPT_RECONCILED"
        );
        let page = with_port(&attachment, |port| port.run_view(&run).unwrap());
        assert_eq!(page.projection.cleanup, RunCleanup::Settled);
        assert_eq!(
            page.projection
                .effects
                .iter()
                .find(|effect| effect.id == queued)
                .unwrap()
                .state,
            kinewright_project::orchestration_records::EffectState::Disposed,
            "C4_STOP_DISPOSES_QUEUED_WORK"
        );
        assert!(
            page.projection.effects.iter().all(|effect| !matches!(
                effect.state,
                kinewright_project::orchestration_records::EffectState::Pending
                    | kinewright_project::orchestration_records::EffectState::Claimed(_)
            )),
            "C4_STOP_NO_LIVE_EFFECTS"
        );
        let accounting = page.accounting.as_ref().unwrap();
        let view = &project.feedback.rows[0]
            .state
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .view;
        if let Some(provider) = accounting
            .tasks
            .first()
            .and_then(|task| task.provider.as_ref())
        {
            assert_eq!(
                view.tasks[0].provider, provider.resolved.connector,
                "C4_RUN_VIEW_DURABLE_PROVIDER"
            );
            assert_eq!(
                view.tasks[0].model, provider.resolved.model,
                "C4_RUN_VIEW_DURABLE_MODEL"
            );
        } else {
            assert_eq!(view.tasks[0].provider, "unavailable");
            assert_eq!(view.tasks[0].model, "unavailable");
        }
        let mut recorded = page.clone();
        recorded.accounting.as_mut().unwrap().elapsed_ms = 12_345;
        recorded.accounting.as_mut().unwrap().stop_reason = Some("durable reason".into());
        let rendered = crate::studio_run_view::RunView::of(&recorded);
        assert_eq!(
            rendered.elapsed, "0:00:12 / 6 hours",
            "C4_RUN_VIEW_DURABLE_CLOCK"
        );
        assert_eq!(rendered.stop_reason.as_deref(), Some("durable reason"));
        assert_eq!(view.stop_reason, accounting.stop_reason);
        assert!(view.stop_reason.is_some(), "C4_RUN_VIEW_STOP_REASON");
        assert!(
            view.tasks
                .iter()
                .all(|task| task.usage.contains("Cost (USD): unavailable")),
            "C4_RUN_VIEW_UNKNOWN_COST"
        );
        assert_eq!(
            project.feedback.selection, selection,
            "C4_STOP_KEEPS_SELECTION"
        );
        assert_eq!(
            project
                .feedback
                .saved
                .iter()
                .map(|note| note.reference.clone())
                .collect::<Vec<_>>(),
            saved_notes,
            "C4_STOP_KEEPS_NOTES"
        );
        assert_eq!(
            attachment.owner.core().export_history().unwrap(),
            manual,
            "C4_STOP_KEEPS_MANUAL_HISTORY"
        );
        assert_eq!(
            std::fs::read(project.temp.path("scheduler/director.json")).unwrap(),
            setting,
            "C4_STOP_KEEPS_DIRECTOR_CHOICE"
        );
        let sends: usize = accounting.tasks.iter().map(|task| task.sends).sum();
        assert_eq!(
            sends,
            usize::from(mode != ProtocolFixtureMode::Startup),
            "C4_STOP_NO_DUPLICATE_SEND"
        );
        println!(
            "C4_NOTES_STOP startup={} full_queue={} durable_before_send=true exact_retry=true cleanup=true sends={sends}",
            mode == ProtocolFixtureMode::Startup,
            mode == ProtocolFixtureMode::FullQueue
        );
        fixture.cleanup().unwrap();
        drop(attachment);
        drop(writer);
    }
}

#[test]
fn s4b_c4_notes_close_readiness_and_stale_stop_preserve_work() {
    let mut project = c7::project("c4-notes-readiness");
    let gpu = kinewright_media::FfmpegMediaEngine::test_gpu_context().unwrap();
    let engine = Arc::new(
        kinewright_media::FfmpegMediaEngine::new_with_paced_test_audio(
            gpu,
            project.temp.root().into(),
        )
        .unwrap(),
    );
    let (mut app, _engine) =
        crate::app::in1_tests::in1_harness_with_engine(Document::default(), engine);
    let attachment = c7::launch(&project);
    app.projects[0].recovery.stop_for_studio();
    app.projects[0].ja1 = Some(attachment);
    project.feedback.issuer = None;
    project.feedback.directory_override = Some(project.temp.path("state"));
    app.focused_mut().studio.feedback = project.feedback;
    app.feedback_frame();
    assert!(matches!(
        app.with_feedback(|feedback, port| feedback.submit(port))
            .unwrap(),
        Outcome::Submitted(_)
    ));
    let run = app.focused().studio.feedback.rows[0].run.clone().unwrap();
    let notes = app
        .focused()
        .studio
        .feedback
        .saved
        .iter()
        .map(|note| note.reference.clone())
        .collect::<Vec<_>>();
    let original = with_port(app.focused().ja1.as_ref().unwrap(), |port| {
        port.run_view(&run).unwrap()
    });
    let path = project.temp.path("scheduler/director.json");
    let unavailable = kinewright_agent::writer_host::DirectorSetting {
        version: 1,
        director: kinewright_project::orchestration_records::ProviderSelectionRecord {
            connector: "codex".into(),
            model: "saved-unavailable-model".into(),
            options: vec![],
        },
    };
    let setting = serde_json::to_vec(&unavailable).unwrap();
    std::fs::write(&path, &setting).unwrap();
    app.refresh_director_setting();
    app.check_director_readiness();
    assert!(
        matches!(&app.director_setting.as_ref().unwrap().status, kinewright_agent::writer_host::DirectorStatus::NoLongerOffered(selection) if selection == &unavailable.director),
        "C4_UNAVAILABLE_SELECTION_PRESERVED"
    );
    app.focused_mut().studio.tab = crate::studio::Tab::Commentary;
    app.feedback_frame();
    let after = with_port(app.focused().ja1.as_ref().unwrap(), |port| {
        port.run_view(&run).unwrap()
    });
    assert_eq!(
        after.projection, original.projection,
        "C4_CLOSING_NOTES_IS_NOT_STOP"
    );
    assert!(app.focused().studio.feedback.stops.is_empty());
    app.focused_mut().studio.tab = crate::studio::Tab::Notes;
    let context = egui::Context::default();
    crate::theme::install(&context);
    let _ = context.run_ui(egui::RawInput::default(), |ui| {
        app.studio_notes(ui);
        app.show_director_settings(ui);
    });
    assert_eq!(
        std::fs::read(&path).unwrap(),
        setting,
        "C4_CHECK_AGAIN_KEEPS_CHOICE_BYTES"
    );
    assert_eq!(
        app.focused()
            .studio
            .feedback
            .saved
            .iter()
            .map(|note| note.reference.clone())
            .collect::<Vec<_>>(),
        notes,
        "C4_CHECK_AGAIN_KEEPS_NOTES"
    );
    with_port(app.focused().ja1.as_ref().unwrap(), |port| {
        queue_message(port, &run);
    });
    let result = app
        .with_feedback(|feedback, port| feedback.stop_run(port, &run))
        .unwrap();
    assert!(result.is_err(), "C4_APP_STALE_STOP_REFUSED");
    assert!(
        app.focused().studio.feedback.stops.is_empty(),
        "C4_STALE_INTENT_NOT_KEPT"
    );
    assert_eq!(
        with_port(app.focused().ja1.as_ref().unwrap(), |port| port
            .run_view(&run)
            .unwrap())
        .projection
        .outcome,
        RunOutcome::Pending,
        "C4_STALE_STOP_NEVER_ACTIVATED"
    );
    app.with_feedback(|feedback, port| {
        feedback.observe_runs(port);
        feedback.stop_run(port, &run).unwrap();
    })
    .unwrap();
    in1_shutdown(&mut app);
}
