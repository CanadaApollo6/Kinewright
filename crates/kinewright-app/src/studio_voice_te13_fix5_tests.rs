use kinewright_agent::{
    orchestration::orchestration_records::CompletionStep,
    writer_host::{
        CompletionState, ProtocolFixture, ProtocolFixtureMode, SessionRoots, WriterHost,
    },
};

fn te13_fix5_hosted_case(
    run: impl FnOnce(&mut KinewrightApp, &crate::studio::tests::Fixtures, &mut Option<WriterHost>),
) {
    let _serial = VOICE_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut retire = crate::studio::tests::Retire::default();
    let mut fixture = fixtures();
    let document = fixture.contexts[0].document.clone();
    let key = fixture.contexts[0].snapshot.descriptor.key.clone();
    fixture.contexts.clear();
    let (mut app, engine) = in1_harness(document);
    retire.arm(&engine);
    app.frames = engine.controls().frames();
    app.media_events = engine.controls().events();
    let mut attachment =
        crate::ja1_attachment::Ja1Attachment::new(fixture.owners.remove(0)).unwrap();
    let writer = te13_fix5_launch(&mut attachment, &fixture);
    let context = Arc::new(attachment.studio_context(&key).unwrap());
    app.focused_mut().ja1 = Some(attachment);
    app.focused_mut().install_ja1_view();
    app.install_studio_context(context).unwrap();
    app.focused_mut().studio.feedback.directory_override = Some(fixture.root.path("kept-feedback"));
    wait_frame(&mut app, &egui::Context::default(), &key, TimeCode::ZERO);
    app.feedback_frame();
    let mut writer = Some(writer);
    run(&mut app, &fixture, &mut writer);
    // This host lives in the test process, so dropping its owner joins it.
    // It has no child process for HostAttachment::close to reap.
    drop(writer);
    in1_shutdown(&mut app);
    drop(app);
    drop(engine);
    retire.wait();
}
fn te13_fix5_launch(
    attachment: &mut crate::ja1_attachment::Ja1Attachment,
    fixture: &crate::studio::tests::Fixtures,
) -> WriterHost {
    let scheduler = fixture.root.path("fix5-scheduler");
    kinewright_agent::orchestration::director_setting::write_fixture_director_setting(&scheduler)
        .unwrap();
    let protocol = ProtocolFixture::new(
        &fixture.root.path("fix5-protocol"),
        ProtocolFixtureMode::Dormant,
    );
    let writer = attachment
        .launch_protocol_fixture(
            SessionRoots {
                read: vec![fixture.root.root().into()],
                write: vec![fixture.root.root().into()],
            },
            scheduler,
            &protocol,
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let reply = writer.client().request(HostCommand::ListFeedback).unwrap();
        if reply.status.native_runtime_status.is_none() {
            break;
        }
        assert!(Instant::now() < deadline, "TE13_FIX5_FIXTURE_RECOVERY");
        std::thread::sleep(Duration::from_millis(10));
    }
    writer
}
fn te13_fix5_studio_stop(
    app: &mut KinewrightApp,
    run: &kinewright_agent::orchestration::orchestration_records::RunId,
) {
    let page = app
        .voice_client()
        .unwrap()
        .request(HostCommand::ObserveRun {
            run: run.clone(),
            after_sequence: 0,
        })
        .unwrap()
        .run_view
        .unwrap();
    let run = serde_json::from_value(serde_json::to_value(run).unwrap()).unwrap();
    let mut issuer = None;
    app.with_feedback(|_, port| {
        let request = port
            .prepare_stop(&run, &page.projection.control_revision, &mut issuer)
            .unwrap();
        assert_eq!(
            format!("{:?}", port.send_stop(&request)),
            "Accepted",
            "TE13_FIX5_STUDIO_STOP"
        );
    })
    .unwrap();
}
fn te13_fix5_text(app: &mut KinewrightApp, text: &str) {
    let comment = app.capture_feedback().unwrap();
    app.focused_mut()
        .studio
        .feedback
        .draft_mut(&comment)
        .unwrap()
        .text = text.into();
    let result = app
        .with_feedback(|feedback, port| feedback.save(port, &comment))
        .unwrap();
    assert!(
        matches!(result, crate::studio_feedback::Outcome::Saved(_)),
        "TE13_FIX5_STUDIO_TEXT_SAVED {result:?}"
    );
    app.with_feedback(|feedback, port| {
        feedback.refresh(port).unwrap();
        assert_eq!(
            feedback
                .saved
                .iter()
                .filter(|saved| saved.comment == comment)
                .count(),
            1,
            "TE13_FIX5_STUDIO_TEXT_ONCE"
        );
    })
    .unwrap();
}
fn te13_fix5_saved(app: &mut KinewrightApp) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        app.voice_frame();
        if matches!(app.focused().studio.voice.state, State::Saved { .. }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "TE13_FIX5_STUDIO_VOICE_SAVED {}",
            app.focused().studio.voice.message
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn te13_fix5_kept_voice(fixture: &crate::studio::tests::Fixtures, note: &NoteId) -> KeptVoice {
    let file = std::fs::read_dir(fixture.root.path("kept-feedback"))
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .unwrap()
        .path();
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    serde_json::from_value(json["voices"][note.as_str()].clone()).unwrap()
}
fn te13_fix5_restart(
    app: &mut KinewrightApp,
    fixture: &crate::studio::tests::Fixtures,
    writer: &mut Option<WriterHost>,
    note: &NoteId,
) {
    let kept = te13_fix5_kept_voice(fixture, note);
    let path = app.focused().ja1.as_ref().unwrap().project_path().unwrap();
    let key = app
        .focused()
        .studio
        .selection
        .as_ref()
        .unwrap()
        .snapshot
        .descriptor
        .key
        .clone();
    let prior = app.focused_mut().ja1.take().unwrap();
    drop(writer.take());
    drop(prior);
    let owner = kinewright_project::editing_session::Ja1EditingSession::open(&path).unwrap();
    let mut attachment = crate::ja1_attachment::Ja1Attachment::new(owner).unwrap();
    *writer = Some(te13_fix5_launch(&mut attachment, fixture));
    // Re-establish the same authenticated subject, not authority over another
    // subject's issuer. The host incarnation and owner epoch are new.
    attachment.replace_fixture_editor(writer.as_ref().unwrap());
    let context = Arc::new(attachment.studio_context(&key).unwrap());
    app.focused_mut().ja1 = Some(attachment);
    app.focused_mut().install_ja1_view();
    app.install_studio_context(context).unwrap();
    app.focused_mut().studio.voice = Voice::default();
    app.focused_mut()
        .studio
        .feedback
        .voices
        .insert(note.clone(), kept);
    app.focused_mut().studio.feedback.active_voice = Some(note.clone());
    wait_frame(app, &egui::Context::default(), &key, TimeCode::ZERO);
    app.voice_frame();
}
#[allow(
    clippy::too_many_lines,
    reason = "one capture keeps its identity across real completion, a conflicting commit, restart and another recording"
)]
fn te13_fix5_voice_collision(winner: &str, restart: bool) {
    te13_fix5_hosted_case(|app, fixture, writer| {
        let finish = writer
            .as_ref()
            .unwrap()
            .fixture_pending_completion(CompletionStep::Publication)
            .unwrap();
        app.focused_mut()
            .ja1
            .as_mut()
            .unwrap()
            .replace_fixture_editor(writer.as_ref().unwrap());
        let issued = app
            .voice_client()
            .unwrap()
            .request(HostCommand::IssueMetadataIssuer { issuer: None })
            .unwrap();
        assert!(
            issued.error.is_none(),
            "TE13_FIX5_FRESH_EDITOR_PROVES_FATE {issued:?}"
        );
        let key = app
            .focused()
            .studio
            .selection
            .as_ref()
            .unwrap()
            .snapshot
            .descriptor
            .key
            .clone();
        let context = Arc::new(
            app.focused()
                .ja1
                .as_ref()
                .unwrap()
                .studio_context(&key)
                .unwrap(),
        );
        app.install_studio_context(context).unwrap();
        wait_frame(app, &egui::Context::default(), &key, TimeCode::ZERO);
        let captured = recorded(app, fixture);
        let note = app.focused().studio.voice.note.clone().unwrap();
        let path = captured.path.clone();
        let bytes = std::fs::read(&path).unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        UPLOAD_GATE.with(|gate| *gate.borrow_mut() = Some((started_tx, release_rx)));
        app.focused_mut().studio.voice.state = app.prepare_voice_upload(captured);
        let started = wait_for_upload_signal(&started_rx, Duration::from_secs(10));
        let action = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match winner {
            "stop" => te13_fix5_studio_stop(app, &finish.run),
            "note" => te13_fix5_text(app, "This other note wins the unconsumed ID."),
            "finish" => assert_eq!(
                finish.retry().unwrap().state,
                CompletionState::Completed,
                "TE13_FIX5_FINISH_DURING_UPLOAD"
            ),
            _ => unreachable!(),
        }));
        let _ = release_tx.send(());
        assert!(started.is_ok(), "TE13_FIX5_UPLOAD_WORKER_HELD");
        if let Err(panic) = action {
            std::panic::resume_unwind(panic);
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            app.voice_frame();
            if matches!(
                app.focused().studio.voice.state,
                State::Upload { reply: None, .. }
            ) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "TE13_FIX5_FIRST_UPLOAD_REFUSED {}",
                app.focused().studio.voice.message
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes,
            "TE13_FIX5_REFUSED_CAPTURE_KEPT"
        );
        if restart {
            finish.retire().unwrap();
            te13_fix5_restart(app, fixture, writer, &note);
        } else {
            app.voice_retry();
        }
        te13_fix5_saved(app);
        assert!(!path.exists(), "TE13_FIX5_ACK_RELEASES_CAPTURE");
        let index = app
            .voice_client()
            .unwrap()
            .request(HostCommand::ListFeedback)
            .unwrap()
            .feedback_index
            .unwrap();
        assert_eq!(index.notes.len(), 1, "TE13_FIX5_ONE_DURABLE_VOICE");
        assert!(index.notes.contains_key(&note));
        if !restart {
            if winner == "note" {
                assert_eq!(finish.retry().unwrap().state, CompletionState::Completed);
            }
            finish.retire().unwrap();
        }
        te13_fix5_text(app, "Text remains available after completion or Stop.");
        // A real second recording proves the old upload no longer holds Record.
        let second = recorded(app, fixture);
        app.focused_mut().studio.voice.state = app.prepare_voice_upload(second);
        te13_fix5_saved(app);
        let index = app
            .voice_client()
            .unwrap()
            .request(HostCommand::ListFeedback)
            .unwrap()
            .feedback_index
            .unwrap();
        assert_eq!(index.notes.len(), 2, "TE13_FIX5_RECORD_AVAILABLE_AGAIN");
        assert!(index.notes.contains_key(&note));
    });
}
#[test]
fn te13_fix5_stop_wins_voice_id() {
    te13_fix5_voice_collision("stop", false);
}
#[test]
fn te13_fix5_note_wins_voice_id() {
    te13_fix5_voice_collision("note", false);
}
#[test]
fn te13_fix5_stop_collision_across_restart() {
    te13_fix5_voice_collision("stop", true);
}
#[test]
fn te13_fix5_finish_before_voice_upload() {
    te13_fix5_voice_collision("finish", false);
}

#[test]
fn te13_fix5_text_during_pending_finish() {
    te13_fix5_hosted_case(|app, _, writer| {
        let finish = writer
            .as_ref()
            .unwrap()
            .fixture_pending_completion(CompletionStep::Freeze)
            .unwrap();
        finish.reconcile_storage().unwrap();
        te13_fix5_text(
            app,
            "A text note while the original finish is still waiting.",
        );
        assert_eq!(finish.retry().unwrap().state, CompletionState::Completed);
        te13_fix5_text(app, "A text note after that finish completed.");
        finish.retire().unwrap();
    });
}
