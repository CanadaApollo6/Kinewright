use kinewright_core::{Command, Core, Event, Operation, TrackId};

#[test]
fn c3_real_shaped_task_history_cost_and_undo_redo_growth() {
    let core = Core::spawn(crate::c0_fixtures::save_cost_document()).unwrap();
    let limits = crate::storage_budget::StorageLimits::default();
    let measure = || {
        let history = core.export_history().unwrap();
        let bytes = serde_json::to_vec(&history).unwrap();
        (
            history.batches.len(),
            bytes.len(),
            crate::store::preflight_record(&bytes, &limits).unwrap(),
        )
    };
    let mut rows = vec![(0, measure())];
    for edit in 1..=32 {
        assert!(matches!(
            core.request(Command::DoBatch(vec![Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: edit % 2 == 0,
            }]))
            .unwrap(),
            Event::DocumentChanged { .. }
        ));
        let row = measure();
        rows.push((edit, row));
        if edit >= 2 && row.2 > crate::run_admission::HISTORY_BYTES {
            break;
        }
    }
    let before = measure();
    assert!(matches!(
        core.request(Command::Undo).unwrap(),
        Event::DocumentChanged { .. }
    ));
    let undo = measure();
    assert!(matches!(
        core.request(Command::Redo).unwrap(),
        Event::DocumentChanged { .. }
    ));
    let redo = measure();
    assert_eq!(undo.0, before.0 + 1);
    assert_eq!(redo.0, undo.0 + 1);
    let report = serde_json::json!({
        "tracks":12, "clips":120, "ordinary_history_limit":crate::run_admission::HISTORY_BYTES,
        "rows_order":["edits",["batches","encoded_bytes","decoded_bytes"]],
        "rows":rows, "before_undo":before, "after_undo":undo, "after_redo":redo,
    });
    println!("C3_HISTORY_MEASUREMENT={report}");
}
