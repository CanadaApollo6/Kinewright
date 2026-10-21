use kinewright_core::*;

fn apply(core: &Core, command: Command) {
    assert!(matches!(
        core.request(command).unwrap(),
        Event::DocumentChanged { .. }
    ));
}

fn lock(locked: bool) -> Operation {
    Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked,
    }
}

fn initial() -> Core {
    let core = Core::spawn(Document::default()).unwrap();
    apply(
        &core,
        Command::Do(Operation::AddTrack {
            track: Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            },
        }),
    );
    core
}

fn restore(core: &Core) -> Core {
    let bytes = serde_json::to_vec(&core.export_history().unwrap()).unwrap();
    let reopened = Core::spawn(Document::default()).unwrap();
    reopened
        .restore_history(serde_json::from_slice(&bytes).unwrap())
        .unwrap();
    reopened
}

#[test]
fn history_export_restore_preserves_cursor_redo_and_batches() {
    let core = initial();
    apply(&core, Command::DoBatch(vec![lock(false), lock(true)]));
    apply(&core, Command::Do(lock(false)));
    apply(&core, Command::Undo);
    let reopened = restore(&core);
    assert_eq!(
        core.export_history().unwrap(),
        reopened.export_history().unwrap()
    );
    assert_eq!(reopened.export_history().unwrap().cursor.redo.len(), 1);
    for command in [Command::Redo, Command::Undo, Command::Undo, Command::Redo] {
        apply(&core, command.clone());
        apply(&reopened, command);
        assert_eq!(
            core.export_history().unwrap(),
            reopened.export_history().unwrap()
        );
    }
}

#[test]
fn open_drag_restore_replays_suffix_before_close() {
    let core = initial();
    let drag = |locked| Command::DoBatchCoalesced {
        operations: vec![lock(locked)],
        coalesce_key: "K".into(),
    };
    apply(&core, drag(false));
    let reopened = restore(&core);
    for command in [drag(true), Command::Undo, Command::Redo] {
        apply(&core, command.clone());
        apply(&reopened, command);
        assert_eq!(
            core.export_history().unwrap(),
            reopened.export_history().unwrap()
        );
    }
    let history = reopened.export_history().unwrap();
    assert_eq!(history.cursor.undo.len(), 2);
    assert!(history.cursor.undo[1].document.tracks[0].sync_lock);
    assert_eq!(history.cursor.undo[1].operations.len(), 2);
}

#[test]
fn history_restore_rejects_corruption_atomically() {
    let core = initial();
    apply(&core, Command::Do(lock(false)));
    apply(&core, Command::Undo);
    let original = core.export_history().unwrap();
    let mut variants = vec![original.clone(); 4];
    variants[0].cursor.redo.clear();
    variants[1].batches[1].command = JournalCommand::Do(lock(true));
    variants[2].batches[0].order = 999;
    variants[3].cursor.undo[0].coalesce_key = Some("corrupt".into());
    for corrupt in variants {
        assert_eq!(core.restore_history(corrupt), Err(HistoryError::Corrupt));
        assert_eq!(core.export_history().unwrap(), original);
    }
}

#[test]
fn identity_effects_distinguish_reuse_and_split_undo() {
    let core = initial();
    let add = Operation::AddSolidClip {
        track: TrackId(1),
        timeline_start: TimeCode(0),
        duration: TimeCode(30),
        color: SolidColor::default(),
    };
    apply(
        &core,
        Command::DoBatch(vec![
            add.clone(),
            Operation::DeleteClip { clip: ClipId(1) },
            add,
        ]),
    );
    let history = core.export_history().unwrap();
    let effects = &history.batches[1].identity_effects;
    assert_eq!(effects[0].created, vec![ClipId(1)]);
    assert_eq!(effects[1].deleted, vec![ClipId(1)]);
    assert_eq!(effects[2].created, vec![ClipId(1)]);
    apply(
        &core,
        Command::Do(Operation::SplitClip {
            clip: ClipId(1),
            at: TimeCode(15),
        }),
    );
    let split = core.export_history().unwrap();
    assert_eq!(
        split.batches[2].identity_effects[0]
            .split_parent
            .as_ref()
            .unwrap()
            .id,
        ClipId(1)
    );
    assert_eq!(
        split.batches[2].identity_effects[0].created,
        vec![ClipId(2)]
    );
    let children = &split.batches[2].identity_effects[0].split_results;
    assert_eq!(children.len(), 2);
    assert_eq!(children[0].source_range, TimeCode(0)..TimeCode(15));
    assert_eq!(children[1].source_range, TimeCode(15)..TimeCode(30));
    let reopened = restore(&core);
    apply(&core, Command::Undo);
    apply(&reopened, Command::Undo);
    assert_eq!(
        core.export_history().unwrap(),
        reopened.export_history().unwrap()
    );
    assert_eq!(
        reopened.export_history().unwrap().document.tracks[0]
            .clips
            .len(),
        1
    );
}
