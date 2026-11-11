#![cfg(all(target_os = "linux", feature = "test-util"))]
use kinewright_core::{
    ClipId, Command, Core, Document, Event, HistorySnapshot, Operation, TrackId,
};
use kinewright_media::test_support::TempDirectory;
use kinewright_project::{
    editing_session::{DurableBranchRole, Ja1EditingSession},
    storage_budget::StorageLimits,
    task_history_admission::{LIMIT_REASON, TaskHistoryAdmission},
};
fn document(clips: usize) -> Document {
    let mut document = kinewright_project::c0_fixtures::save_cost_document();
    let template = document.tracks.remove(0);
    document.tracks.clear();
    for index in 1..=clips / 10 {
        let mut track = template.clone();
        track.id = TrackId(index as u64);
        for (offset, clip) in track.clips.iter_mut().enumerate() {
            clip.id = ClipId(index as u64 * 100 + offset as u64 + 1);
        }
        document.tracks.push(track);
    }
    document.validate().unwrap();
    document
}
fn history(seed: &HistorySnapshot, edits: usize, ripple: bool) -> HistorySnapshot {
    let core = Core::spawn(std::sync::Arc::clone(&seed.baseline)).unwrap();
    // Independent probes preserve the native seed's document sharing and cursor.
    core.restore_history(seed.clone()).unwrap();
    apply_edits(&core, edits, ripple);
    core.export_history().unwrap()
}
fn apply_edits(core: &Core, edits: usize, ripple: bool) {
    for edit in 0..edits {
        let operations = if ripple && edit == 100 {
            (1..=20)
                .map(|track| Operation::RippleDeleteClip {
                    clip: ClipId(track * 100 + 10),
                })
                .collect()
        } else {
            vec![Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: edit % 2 == 0,
            }]
        };
        let event = core.request(Command::DoBatch(operations)).unwrap();
        assert!(matches!(event, Event::DocumentChanged { .. }), "{event:?}");
    }
}
fn largest_history(
    admission: &TaskHistoryAdmission,
    seed: &HistorySnapshot,
    ripple: bool,
) -> (usize, HistorySnapshot) {
    let mut low = 0;
    let mut high = 1;
    while admission.check(&history(seed, high, ripple)).is_ok() {
        low = high;
        high *= 2;
        assert!(high <= kinewright_core::HISTORY_COMMANDS);
    }
    while high - low > 1 {
        let mid = usize::midpoint(low, high);
        if admission.check(&history(seed, mid, ripple)).is_ok() {
            low = mid;
        } else {
            high = mid;
        }
    }
    let refused = history(seed, high, ripple);
    let error = admission.check(&refused).unwrap_err();
    assert!(error.to_string().contains(LIMIT_REASON), "{error}");
    let accepted = history(seed, low, ripple);
    admission.check(&accepted).unwrap();
    (low, accepted)
}
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one matrix compares admission, staged persistence and fresh decoding"
)]
fn native_task_history_capacity_table() {
    for clips in [40, 120, 400, 1200] {
        let temporary = TempDirectory::new_in(
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
            "history-capacity-matrix",
        );
        let core = Core::spawn(document(clips)).unwrap();
        let mut owner =
            Ja1EditingSession::new(temporary.root(), &core, StorageLimits::default()).unwrap();
        owner
            .first_save(
                &temporary.path("project.kinewright"),
                StorageLimits::default(),
            )
            .unwrap();
        let mut candidate = owner
            .fork_durable_branch("candidate", DurableBranchRole::RunCandidate)
            .unwrap();
        owner.persist_durable_branch(&mut candidate).unwrap();
        let binding = owner.candidate_binding(candidate.branch_id()).unwrap();
        let mut task = owner
            .fork_registered_from_candidate(&binding, "task")
            .unwrap();
        let admission = owner.task_history_admission(&task).unwrap();
        let seed = task.core().export_history().unwrap();
        let (edits, history) = largest_history(&admission, &seed, clips == 400);
        apply_edits(task.core(), edits, clips == 400);
        admission
            .check(&task.core().export_history().unwrap())
            .unwrap();
        assert_eq!(task.core().export_history().unwrap(), history);
        let frozen = owner.freeze_registered_task_checkpoint(&mut task).unwrap();
        let checkpoint = owner
            .store()
            .unwrap()
            .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(
                &frozen.source().checkpoint,
            )
            .unwrap();
        let restored = owner
            .store()
            .unwrap()
            .resolve_history(&checkpoint.history_ref)
            .unwrap();
        assert_eq!(
            *restored, history,
            "ADMITTED_NATIVE_TASK_FREEZES_AND_RESOLVES"
        );
        let metrics = |history: &HistorySnapshot| {
            let lineage =
                kinewright_project::task_history_admission::lineage_for_fixture(history).unwrap();
            let (history_charge, lineage_charge) =
                kinewright_project::task_history_admission::live_history_bytes_for_fixture(
                    history, &lineage,
                )
                .unwrap();
            let record = kinewright_project::history_record::HistoryRecord::capture(
                history,
                kinewright_project::history_record::HistoryRole::Task,
            )
            .unwrap();
            serde_json::json!({
                "history_charge":history_charge,
                "lineage_charge":lineage_charge,
                "live_pair_charge":history_charge + lineage_charge,
                "portable_bytes":serde_json::to_vec(history).unwrap().len(),
                "record_encoded":serde_json::to_vec(&record).unwrap().len(),
                "record_decoded":kinewright_project::task_history_admission::decoded_history_bytes_for_fixture(history).unwrap(),
                "lineage_encoded":serde_json::to_vec(&lineage).unwrap().len(),
                "replay_bound":kinewright_core::history_size::replay_live_bound(&record.journal, &history.baseline, &history.document).unwrap(),
            })
        };
        println!(
            "NATIVE_HISTORY_CAPACITY={}",
            serde_json::json!({"clips":clips,"edits":edits,"ripple_delete_20": clips==400,
                "accepted":metrics(&history), "first_refused":metrics(&self::history(&seed, edits + 1, clips == 400))})
        );
        if clips == 400 {
            assert!(edits >= 200, "REQUIRED_400_200");
        }
        let decoded_lineage = owner
            .store()
            .unwrap()
            .read_record::<kinewright_project::bundle_records::LineageRecord>(
                &checkpoint.lineage_ref,
            )
            .unwrap();
        let (decoded_history, decoded_lineage) =
            kinewright_project::task_history_admission::live_history_bytes_for_fixture(
                &restored,
                &decoded_lineage,
            )
            .unwrap();
        assert!(
            decoded_history + decoded_lineage <= kinewright_project::history_record::TASK_LIVE,
            "ADMITTED_TASK_DECODE_FITS_ROLE clips={clips} history={decoded_history} lineage={decoded_lineage}"
        );
    }
}
