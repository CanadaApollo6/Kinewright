// Included inside app::in2b_tests; native storage guards do not gate any AU6 app tests.
#[cfg(target_os = "linux")]
#[test]
fn app_session_save_preserves_all_undo_redo_and_later_edits() {
    use kinewright_project::bundle::{BundleWriteAdmission, LoadedBundle, ProjectBundle};
    let temp = TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "r2-app-session-history",
    );
    let document = Document {
        tracks: (1..=200)
            .map(|id| Track {
                id: TrackId(id),
                kind: TrackKind::Video,
                name: None,
                sync_lock: true,
                clips: vec![],
            })
            .collect(),
        ..Document::default()
    };
    let (mut app, engine) = in2b_harness(document, None);
    let path = temp.path("saved.kinewright");
    let mut owner = ProjectBundle::activate(
        &path,
        BundleWriteAdmission::new_document(),
        app.focused().core.history_pool(),
    )
    .unwrap();
    let inputs = owner.empty_inputs().unwrap().reference.clone();
    owner.save(&app.focused().core, &inputs).unwrap();
    for i in 0..40 {
        app.send_operation(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: i % 2 == 0,
        });
        owner.save(&app.focused().core, &inputs).unwrap();
    }
    let saved = serde_json::to_value(app.focused().core.export_history().unwrap()).unwrap();
    assert_eq!(
        saved["history"]["undo"].as_array().unwrap().len(),
        40,
        "SAVING_DOES_NOT_FOLD_EXECUTOR_HISTORY"
    );
    assert_eq!(
        saved,
        serde_json::to_value(
            owner
                .branch_history(owner.manual_branch_id())
                .unwrap()
                .into_capture()
                .unwrap()
        )
        .unwrap()
    );
    for _ in 0..saved["history"]["undo"].as_array().unwrap().len() + 3 {
        app.undo();
    }
    let undone = app.focused().core.export_history().unwrap();
    assert!(
        undone.history().undo.is_empty(),
        "APP_UNDO_WALK_RETURNS_TO_BASELINE"
    );
    assert_eq!(
        undone.history().redo.len(),
        saved["history"]["undo"].as_array().unwrap().len()
    );
    drop(undone);
    owner
        .save(&app.focused().core, &inputs)
        .expect("undo through the app/session path made every later save fail");
    for _ in 0..saved["history"]["undo"].as_array().unwrap().len() {
        app.redo();
    }
    assert_eq!(
        serde_json::to_value(
            &*app
                .focused()
                .core
                .export_history()
                .unwrap()
                .history()
                .document
        )
        .unwrap(),
        saved["history"]["document"],
        "app/session undo lost a saved edit"
    );
    owner.save(&app.focused().core, &inputs).unwrap();
    drop(owner);
    let mut loaded = LoadedBundle::read(
        &path,
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .unwrap();
    let restored = loaded.open_manual().unwrap();
    assert_eq!(
        restored.export_history().unwrap(),
        app.focused().core.export_history().unwrap()
    );
    in2b_quiesce_engine(&engine);
    in2b_shutdown(&mut app);
}
