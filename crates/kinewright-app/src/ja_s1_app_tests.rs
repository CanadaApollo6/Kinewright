// Included inside app::in2b_tests; native storage guards do not gate any AU6 app tests.
#[cfg(target_os = "linux")]
#[test]
fn app_session_undo_stops_at_saved_floor_and_next_save_keeps_edits() {
    use kinewright_project::{
        bundle::{BundleWriteAdmission, LoadedBundle, ProjectBundle},
        storage_budget::StorageLimits,
    };
    let temp = TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "r2-app-session-floor",
    );
    let document = Document {
        tracks: (1..=200)
            .map(|id| Track {
                id: TrackId(id),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            })
            .collect(),
        ..Document::default()
    };
    let (mut app, engine) = in2b_harness(document, None);
    let path = temp.path("saved.kinewright");
    let mut owner = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    let inputs = owner.empty_inputs().unwrap().reference.clone();
    owner.save(&app.focused().core, &inputs).unwrap();
    owner
        .restrict_limits_probe(StorageLimits {
            branch_history_bytes: 1 << 20,
            ..StorageLimits::default()
        })
        .unwrap();
    for i in 0..40 {
        app.send_operation(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: i % 2 == 0,
        });
        owner.save(&app.focused().core, &inputs).unwrap();
    }
    let saved = app.focused().core.export_history().unwrap();
    assert!(saved.floor_offset > 0);
    assert_eq!(
        saved,
        *owner.branch_history(owner.manual_branch_id()).unwrap()
    );
    for _ in 0..saved.cursor.undo.len() + 3 {
        app.undo();
    }
    let floor = app.focused().core.export_history().unwrap();
    assert!(
        floor.cursor.undo.is_empty(),
        "app offered undo below the saved floor"
    );
    assert_eq!(floor.cursor.redo.len(), saved.cursor.undo.len());
    owner
        .save(&app.focused().core, &inputs)
        .expect("undo through the app/session path made every later save fail");
    for _ in 0..floor.cursor.redo.len() {
        app.redo();
    }
    assert_eq!(
        app.focused().core.export_history().unwrap().document,
        saved.document,
        "app/session undo lost a saved edit"
    );
    owner.save(&app.focused().core, &inputs).unwrap();
    drop(owner);
    let loaded = LoadedBundle::read(&path).unwrap();
    let restored = kinewright_core::Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&restored).unwrap();
    assert_eq!(
        restored.export_history().unwrap(),
        app.focused().core.export_history().unwrap()
    );
    in2b_quiesce_engine(&engine);
    in2b_shutdown(&mut app);
}
