#[test]
fn te13_fix4_host_voice_refusal_keeps_capture_text_and_retry() {
    hosted_case(|app, fixture, _| {
        let captured = recorded(app, fixture);
        let note = app.focused().studio.voice.note.clone().unwrap();
        let comment = app.focused().studio.voice.comment.clone().unwrap();
        let typed = "Spoken draft: keep my edited words.";
        app.focused_mut().studio.voice.text = typed.into();
        app.focused_mut().studio.voice.edited = true;
        app.keep_voice_editor().unwrap();
        let feedback = &mut app.focused_mut().studio.feedback;
        feedback.draft_mut(&comment).unwrap().text = typed.into();
        let client = app.voice_client().unwrap();
        let issued = client
            .request(HostCommand::IssueMetadataIssuer { issuer: None })
            .unwrap()
            .metadata_issuer
            .unwrap();
        let (descriptor, log) = captured
            .prepare_upload(issued.envelope, issued.root_generation)
            .unwrap();
        app.keep_voice_capture(&captured, Some(&descriptor))
            .unwrap();
        let path = captured.path.clone();
        let bytes = std::fs::read(&path).unwrap();
        let message =
            "Task completion is awaiting its original retry. This write has not been saved.";
        let reply = upload(
            client,
            &captured,
            descriptor.clone(),
            log.clone(),
            Some(HostReply {
                error: Some(kinewright_agent::writer_host::HostRefusal::new(
                    "completion_pending",
                    message,
                )),
                ..HostReply::default()
            }),
        )
        .unwrap();
        app.focused_mut().studio.voice.state = State::Upload {
            captured,
            upload: descriptor,
            log,
            reply: Some(reply),
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            app.voice_frame();
            match &app.focused().studio.voice.state {
                State::Upload { reply: None, .. } => break,
                State::Upload { .. } => (),
                _ => panic!("TE13_FIX4_VOICE_CAPTURE_KEPT"),
            }
            assert!(Instant::now() < deadline, "TE13_FIX4_VOICE_RESPONSE");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            app.focused().studio.voice.message,
            message,
            "TE13_FIX4_VOICE_TRUTHFUL_MESSAGE"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes,
            "TE13_FIX4_VOICE_CAPTURE_KEPT"
        );
        assert_eq!(
            app.focused()
                .studio
                .feedback
                .drafts
                .iter()
                .find(|d| d.comment == comment)
                .unwrap()
                .text,
            typed,
            "TE13_FIX4_VOICE_TEXT_KEPT"
        );
        assert_eq!(
            te13_fix4_kept_voice(fixture.root.path("kept-feedback"))["voices"][note.as_str()],
            serde_json::to_value(&app.focused().studio.feedback.voices[&note]).unwrap(),
            "TE13_FIX4_VOICE_BINDING_ON_DISK"
        );
        app.voice_retry();
        wait_saved(app);
        assert!(!path.exists(), "TE13_FIX4_AUDIO_RELEASED_ONLY_AFTER_ACK");
        assert_eq!(
            app.focused()
                .studio
                .feedback
                .drafts
                .iter()
                .find(|d| d.comment == comment)
                .unwrap()
                .text,
            typed
        );
    });
}

fn te13_fix4_kept_voice(directory: std::path::PathBuf) -> serde_json::Value {
    let file = std::fs::read_dir(directory)
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "json")
        })
        .unwrap()
        .path();
    serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap()
}
