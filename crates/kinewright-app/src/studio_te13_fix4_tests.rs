// Shared Studio setup for both writer modes and the unchanged standalone voice guard.
fn te13_fix4_studio_case(
    hosted: bool,
    run: impl FnOnce(&mut KinewrightApp, &crate::studio::tests::Fixtures),
) {
    let mut retire = Retire::default();
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
    if hosted {
        attachment
            .launch_host(
                "te13-fix4-notes".into(),
                kinewright_agent::writer_host::SessionRoots {
                    read: vec![fixture.root.root().into()],
                    write: vec![fixture.root.root().into()],
                },
            )
            .unwrap();
    }
    let context = Arc::new(attachment.studio_context(&key).unwrap());
    app.focused_mut().ja1 = Some(attachment);
    app.focused_mut().install_ja1_view();
    app.install_studio_context(context).unwrap();
    app.focused_mut().studio.feedback.directory_override = Some(fixture.root.path("kept-feedback"));
    wait_frame(&mut app, &egui::Context::default(), &key, TimeCode::ZERO);
    app.feedback_frame();
    run(&mut app, &fixture);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !app.focused_mut().ja1.as_mut().unwrap().close() {
        assert!(
            std::time::Instant::now() < deadline,
            "TE13_FIX4_STUDIO_CLOSE"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    in1_shutdown(&mut app);
    drop(app);
    drop(engine);
    retire.wait();
}
#[test]
fn te13_fix4_standalone_spoken_note_requires_its_writer_host() {
    te13_fix4_studio_case(false, |app, _| {
        let comment = app.capture_feedback().unwrap();
        app.focused_mut()
            .studio
            .feedback
            .draft_mut(&comment)
            .unwrap()
            .text = "Keep my standalone draft.".into();
        let before = app
            .focused()
            .studio
            .feedback
            .drafts
            .iter()
            .map(|draft| (draft.comment.clone(), draft.text.clone()))
            .collect::<Vec<_>>();
        app.voice_press();
        let context = egui::Context::default();
        let output = context.run_ui(egui::RawInput::default(), |ui| app.voice_controls(ui));
        let painted = crate::ja1_attachment::tests::painted_text(&output);
        assert!(
            painted.contains(
                "Save and open this project with its writer host before recording a note."
            ),
            "TE13_FIX4_STANDALONE_VOICE_NO_HOST {painted}"
        );
        assert_eq!(
            app.focused()
                .studio
                .feedback
                .drafts
                .iter()
                .map(|draft| (draft.comment.clone(), draft.text.clone()))
                .collect::<Vec<_>>(),
            before,
            "TE13_FIX5_STANDALONE_VOICE_PRESERVES_DRAFT"
        );
    });
}
