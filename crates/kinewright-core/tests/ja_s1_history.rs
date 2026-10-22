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

#[test]
fn ordered_floor_install_preserves_open_gesture_and_redo() {
    let core = initial();
    let drag = |locked| Command::DoBatchCoalesced {
        operations: vec![lock(locked)],
        coalesce_key: "floor-drag".into(),
    };
    apply(&core, drag(false));
    let frozen = core.export_history().unwrap();
    let reopened = restore(&core);
    let transition = HistoryTransition::floor(frozen.clone(), 1).unwrap();
    reopened.install_history_transition(transition).unwrap();
    let transition = HistoryTransition::floor(frozen, 1).unwrap();
    core.install_history_transition(transition).unwrap();
    assert_eq!(core.export_history().unwrap().cursor.undo.len(), 1);
    assert_eq!(
        core.export_history().unwrap().cursor.undo[0]
            .coalesce_key
            .as_deref(),
        Some("floor-drag")
    );
    for command in [
        drag(true),
        drag(false),
        Command::Undo,
        Command::Redo,
        Command::Undo,
    ] {
        apply(&core, command.clone());
        apply(&reopened, command);
        assert_eq!(
            core.export_history().unwrap(),
            reopened.export_history().unwrap()
        );
    }
    let with_redo = core.export_history().unwrap();
    let restored = restore(&core);
    assert_eq!(restored.export_history().unwrap(), with_redo);
    apply(&restored, Command::Redo);
    assert_eq!(restored.export_history().unwrap().cursor.undo.len(), 1);
    assert!(
        restored.export_history().unwrap().cursor.undo[0]
            .document
            .tracks[0]
            .sync_lock
    );
    // A transition validated at a previous cut cannot overwrite a newer suffix.
    let stale = HistoryTransition::recovery_close(with_redo.clone(), &with_redo).unwrap();
    assert_eq!(
        restored.install_history_transition(stale),
        Err(HistoryError::Corrupt)
    );
}

#[test]
fn recovery_close_follows_suffix_equivalence() {
    let uninterrupted = initial();
    let drag = |locked| Command::DoBatchCoalesced {
        operations: vec![lock(locked)],
        coalesce_key: "recovery-K".into(),
    };
    apply(&uninterrupted, drag(false));
    let recovered = restore(&uninterrupted);
    apply(&uninterrupted, drag(true));
    let stale = recovered.export_history().unwrap();
    apply(&recovered, drag(true));
    let cut = recovered.export_history().unwrap();
    assert!(HistoryTransition::recovery_close(cut.clone(), &stale).is_err());
    assert_eq!(cut, uninterrupted.export_history().unwrap());
    for core in [&uninterrupted, &recovered] {
        let transition =
            HistoryTransition::recovery_close(core.export_history().unwrap(), &cut).unwrap();
        core.install_history_transition(transition).unwrap();
        assert_eq!(core.export_history().unwrap().cursor.undo.len(), 2);
        assert_eq!(
            core.export_history().unwrap().cursor.undo[1].coalesce_key,
            None
        );
    }
    assert_eq!(
        uninterrupted.export_history().unwrap(),
        recovered.export_history().unwrap()
    );
    for command in [Command::Undo, Command::Redo, drag(false)] {
        apply(&uninterrupted, command.clone());
        apply(&recovered, command);
        assert_eq!(
            uninterrupted.export_history().unwrap(),
            recovered.export_history().unwrap()
        );
    }
    assert_eq!(recovered.export_history().unwrap().cursor.undo.len(), 3);
}

#[test]
fn checkpoint_install_refuses_a_newer_producer_suffix() {
    let core = initial();
    apply(&core, Command::Do(lock(false)));
    let before = core.export_history().unwrap();
    let floor = HistoryTransition::floor(before.clone(), 1).unwrap();
    apply(&core, Command::Do(lock(true)));
    let current = core.export_history().unwrap();
    assert!(
        core.install_checkpoint(before, floor.after().clone())
            .is_err(),
        "checkpoint overwrote a newer accepted suffix"
    );
    assert_eq!(core.export_history().unwrap(), current);
}
#[test]
fn pending_floor_blocks_undo_and_commits_current_suffix() {
    let core = initial();
    for value in [false, true, false, true] {
        core.request(Command::Do(lock(value))).unwrap();
    }
    let before = core.export_history().unwrap();
    let transition = HistoryTransition::floor(before.clone(), 2).unwrap();
    let retained = transition.after().cursor.undo.len();
    let mut barrier = core.begin_history_floor(transition).unwrap();
    assert_eq!(
        core.export_history().unwrap(),
        before,
        "barrier must retain entries"
    );
    for _ in 0..retained {
        core.request(Command::Undo).unwrap();
    }
    let boundary = core.export_history().unwrap();
    core.request(Command::Undo).unwrap();
    assert_eq!(
        core.export_history().unwrap(),
        boundary,
        "pending floor must block Undo"
    );
    core.request(Command::Redo).unwrap();
    core.request(Command::Do(lock(false))).unwrap();
    let current = core.export_history().unwrap();
    let expected = HistoryTransition::floor(current.clone(), 2)
        .unwrap()
        .after()
        .clone();
    let revision = || match core.request(Command::Query(Query::Snapshot)).unwrap() {
        Event::QueryResult(QueryResult::Snapshot { revision, .. }) => revision,
        _ => panic!("snapshot response"),
    };
    let prior = revision();
    barrier.commit().unwrap();
    assert_eq!(
        revision().0,
        prior.0 + 1,
        "private replay must not expose extra revisions"
    );
    assert_eq!(
        core.export_history().unwrap(),
        expected,
        "commit must use current suffix"
    );
}

#[test]
fn pending_floor_release_restores_entries_and_rejects_a_stale_capture() {
    let core = initial();
    for value in [false, true, false] {
        core.request(Command::Do(lock(value))).unwrap();
    }
    core.request(Command::Undo).unwrap();
    let before = core.export_history().unwrap();
    let transition = HistoryTransition::floor(before.clone(), 1).unwrap();
    let mut barrier = core.begin_history_floor(transition).unwrap();
    assert!(
        core.restore_history(before.clone()).is_err(),
        "restore cannot bypass a pending floor"
    );
    barrier.release().unwrap();
    let mut retry_barrier = core
        .begin_history_floor(HistoryTransition::floor(before.clone(), 1).unwrap())
        .unwrap();
    assert!(
        barrier.commit().is_err(),
        "released handle must not commit a retry's floor"
    );
    assert!(
        barrier.release().is_err(),
        "released handle must not release a retry's floor"
    );
    retry_barrier.release().unwrap();
    assert_eq!(core.export_history().unwrap(), before);
    for _ in &before.cursor.undo {
        core.request(Command::Undo).unwrap();
    }
    assert_eq!(core.export_history().unwrap().document, before.baseline);
    for _ in 0..before.cursor.undo.len() + before.cursor.redo.len() {
        core.request(Command::Redo).unwrap();
    }
    assert!(core.export_history().unwrap().cursor.redo.is_empty());
    let stale = HistoryTransition::floor(before, 1).unwrap();
    assert!(
        core.begin_history_floor(stale).is_err(),
        "capture/install race must refuse before IO"
    );
}
