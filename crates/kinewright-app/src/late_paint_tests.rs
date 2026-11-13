//! A laid-out paint marker may finish after Quit removes the final project.
use super::*;
use kinewright_media::test_support::{TempDirectory, workspace_target_dir};

#[test]
fn last_project_quit_discards_delayed_scrub_paint_without_changing_saved_bytes() {
    let directory = TempDirectory::new_in(&workspace_target_dir(), "exit-late-paint");
    let (mut app, _) = in1_tests::in1_harness(Document::default());
    let path = directory.path("saved.kinewright");
    app.write_project(&path).unwrap();
    let saved = std::fs::read(&path).unwrap();
    let document = Arc::clone(&app.focused().document);
    app.resume_after_scrub = true;
    app.release_scrub(TimeCode::ZERO);
    let (stamp, target) = app
        .pending_resume
        .expect("released scrub awaits its target paint");
    app.presenter.advance(1);
    app.presenter.collect(kinewright_core::PreviewFrame {
        at: target,
        stamp,
        texture: kinewright_core::FrameTexture {
            width: 1,
            height: 1,
            rgba: Arc::new(vec![0; 4]),
        },
    });
    assert!(app.presenter.finalize(|| (stamp, None), |_| ()).is_some());
    assert!(app.presenter.take_ack(2).is_none());
    // Layout happens before Quit. Rendering acknowledges this same marker later.
    let marker = app.presenter.marker(2, app.paint_clock());
    let context = egui::Context::default();
    app.request_exit(&context);
    assert!(app.allow_close && app.projects.is_empty());
    assert!(app.pending_project_action.is_none());
    marker.record();
    app.presenter.advance(3);
    let acknowledgement = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        app.acknowledge_paint(3);
    }));
    assert!(
        acknowledgement.is_ok(),
        "EXIT_LATE_PAINT_EMPTY_PROJECT_IS_SAFE"
    );
    assert!(
        app.pending_resume.is_none(),
        "EXIT_LATE_PAINT_RESUME_CLEARED"
    );
    assert!(!app.resume_after_scrub && !app.playing);
    assert!(app.presenter.take_ack(4).is_none());
    app.ui_replies
        .0
        .send(Box::new(|_, _| panic!("empty-project UI reply dispatched")))
        .unwrap();
    app.poll_background(&context);
    assert!(app.ui_replies.1.try_recv().is_ok());
    eframe::App::on_exit(&mut app);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        saved,
        "EXIT_LATE_PAINT_SAVED_BYTES_EXACT"
    );
    assert_eq!(
        kinewright_project::load_document(&path).unwrap().0,
        *document
    );
}
