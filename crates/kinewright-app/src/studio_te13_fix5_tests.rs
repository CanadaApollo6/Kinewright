fn te13_fix5_studio_text(hosted: bool) {
    te13_fix4_studio_case(hosted, |app, _| {
        let comment = app.capture_feedback().unwrap();
        let text = "Keep this exact note. Café\nSecond line.";
        app.focused_mut()
            .studio
            .feedback
            .draft_mut(&comment)
            .unwrap()
            .text = text.into();
        let saved = app
            .with_feedback(|feedback, port| feedback.save(port, &comment))
            .unwrap();
        assert!(
            matches!(saved, Outcome::Saved(_)),
            "TE13_FIX5_NORMAL_NOTE_SAVED {saved:?}"
        );
        assert!(
            !app.focused()
                .studio
                .feedback
                .drafts
                .iter()
                .any(|draft| draft.comment == comment)
        );
        app.with_feedback(|feedback, port| {
            feedback.refresh(port).unwrap();
            let ours = feedback
                .saved
                .iter()
                .filter(|saved| saved.comment == comment)
                .collect::<Vec<_>>();
            assert_eq!(ours.len(), 1, "TE13_FIX5_NORMAL_NOTE_EXACTLY_ONCE");
            assert_eq!(ours[0].text, text, "TE13_FIX5_NORMAL_NOTE_EXACT_WORDS");
            // The draft has gone; a repeated UI Save cannot create a revision.
            assert!(matches!(feedback.save(port, &comment), Outcome::Refused(_)));
            feedback.refresh(port).unwrap();
            assert_eq!(
                feedback
                    .saved
                    .iter()
                    .filter(|saved| saved.comment == comment)
                    .count(),
                1,
                "TE13_FIX5_NORMAL_NOTE_EXACTLY_ONCE"
            );
        })
        .unwrap();
    });
}
#[test]
fn te13_fix5_standalone_note_saves_once() {
    te13_fix5_studio_text(false);
}
#[test]
fn te13_fix5_host_note_saves_once() {
    te13_fix5_studio_text(true);
}
