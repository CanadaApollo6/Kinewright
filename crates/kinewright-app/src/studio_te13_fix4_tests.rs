// The Agent fault witnesses exercise the real pending-completion refusal. These
// Studio boundary witnesses deliver that response to the production draft/voice
// state machines over each actual feedback-port mode; they do not synthesize a
// successful save or install a second writer for the same project.
const TE13_FIX4_BUSY: &str =
    "Task completion is awaiting its original retry. This write has not been saved.";
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
fn te13_fix4_studio_note(hosted: bool) {
    te13_fix4_studio_case(hosted, |app, _fixture| {
        let comment = app.capture_feedback().unwrap();
        let text = "Keep this exact note. Café\nSecond line.";
        app.focused_mut()
            .studio
            .feedback
            .draft_mut(&comment)
            .unwrap()
            .text = text.into();
        let reply = app
            .with_feedback(|feedback, port| {
                let mut busy = Lossy::new(port);
                busy.refused = Some(TE13_FIX4_BUSY.into());
                feedback.save(&mut busy, &comment)
            })
            .unwrap();
        assert!(
            matches!(reply, Outcome::Refused(ref why) if why == TE13_FIX4_BUSY),
            "TE13_FIX4_STUDIO_NOTE_REFUSAL"
        );
        draft_kept(
            &app.focused().studio.feedback,
            &comment,
            text,
            "TE13_FIX4_STUDIO_NOTE_KEPT",
        );
        let Keeping::Kept { path, .. } = &app.focused().studio.feedback.keeping else {
            panic!("kept");
        };
        let kept: Kept = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert!(
            kept.drafts
                .iter()
                .any(|d| d.comment == comment && d.text == text),
            "TE13_FIX4_STUDIO_NOTE_ON_DISK"
        );
        let context = egui::Context::default();
        let output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 2000.0))),
                ..Default::default()
            },
            |ui| app.studio_notes(ui),
        );
        let painted = crate::ja1_attachment::tests::painted_text(&output);
        assert!(
            painted.contains(&format!("Not saved · {TE13_FIX4_BUSY}")),
            "TE13_FIX4_STUDIO_TRUTHFUL_NOTE {painted}"
        );
        // Once the refusal has cleared, the same real port saves this exact note.
        let saved = app
            .with_feedback(|feedback, port| feedback.save(port, &comment))
            .unwrap();
        assert!(
            matches!(saved, Outcome::Saved(_)),
            "TE13_FIX4_STUDIO_NOTE_RETRY {saved:?}"
        );
    });
}
#[test]
fn te13_fix4_standalone_note_refusal_keeps_exact_draft() {
    te13_fix4_studio_note(false);
}
#[test]
fn te13_fix4_host_note_refusal_keeps_exact_draft() {
    te13_fix4_studio_note(true);
}
#[test]
fn te13_fix4_standalone_spoken_note_requires_its_writer_host() {
    te13_fix4_studio_case(false, |app, _| {
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
    });
}
