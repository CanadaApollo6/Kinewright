#![cfg(all(target_os = "linux", feature = "test-util"))]
//! Isolated allocator witness: the compact wire form must cover real restored heap.
use kinewright_core::{ClipId, Command, Core, Event, HistorySnapshot, Operation, TrackId};
use serde_json::json;

#[derive(serde::Serialize)]
struct BoundaryOracle<'a> {
    floor_offset: u64,
    baseline: &'a kinewright_core::Document,
    document: &'a kinewright_core::Document,
    batches: &'a [kinewright_core::HistoryBatch],
    cursor: &'a kinewright_core::HistoryCursor,
}

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

fn document(clips: usize) -> kinewright_core::Document {
    let mut document = kinewright_project::c0_fixtures::save_cost_document();
    let template = document.tracks.remove(0);
    document.tracks.clear();
    for index in 1..=clips / 10 {
        let mut track = template.clone();
        track.id = TrackId(index as u64);
        for (clip_index, clip) in track.clips.iter_mut().enumerate() {
            clip.id = ClipId(index as u64 * 100 + clip_index as u64 + 1);
        }
        document.tracks.push(track);
    }
    document.validate().unwrap();
    document
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one isolated allocator profile and its role-cap table"
)]
fn history_capacity_restored_heap_is_covered_by_typed_charge() {
    let mut results = vec![];
    for (clips, edits, ripple) in [(400, 200, false), (400, 200, true), (1200, 200, false)] {
        let core = Core::spawn(document(clips)).unwrap();
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
        let history = core.export_history().unwrap();
        let lineage =
            kinewright_project::task_history_admission::lineage_for_fixture(&history).unwrap();
        let history_bytes = serde_json::to_vec(&history).unwrap();
        let lineage_bytes = serde_json::to_vec(&lineage).unwrap();
        let oracle = BoundaryOracle {
            floor_offset: history.floor_offset,
            baseline: &history.baseline,
            document: &history.document,
            batches: &history.batches,
            cursor: &history.cursor,
        };
        let boundary_documents_encoded = serde_json::to_vec(&oracle).unwrap().len();
        let profiler = dhat::Profiler::builder().testing().build();
        let before = dhat::HeapStats::get();
        let started = std::time::Instant::now();
        let restored_history: HistorySnapshot = serde_json::from_slice(&history_bytes).unwrap();
        let after_history = dhat::HeapStats::get();
        let restored_lineage: kinewright_project::bundle_records::LineageRecord =
            serde_json::from_slice(&lineage_bytes).unwrap();
        let elapsed = started.elapsed();
        let after = dhat::HeapStats::get();
        let history_heap = after_history.curr_bytes - before.curr_bytes;
        let lineage_heap = after.curr_bytes - after_history.curr_bytes;
        // Restoring a verified capture temporarily rebuilds ordinary actor state;
        // afterwards the actor shares the supplied boundary documents.
        let shadow = Core::spawn(std::sync::Arc::clone(&restored_history.baseline)).unwrap();
        shadow.restore_history(restored_history.clone()).unwrap();
        shadow
            .request(Command::Query(kinewright_core::Query::Snapshot))
            .unwrap();
        let with_shadow = dhat::HeapStats::get();
        let actor_shadow_heap = with_shadow.curr_bytes - after.curr_bytes;
        let peak = with_shadow.max_bytes - before.curr_bytes;
        drop(profiler);
        assert_eq!(history, restored_history);
        assert_eq!(lineage, restored_lineage);
        let (history_charge, lineage_charge) =
            kinewright_project::task_history_admission::live_history_bytes_for_fixture(
                &restored_history,
                &restored_lineage,
            )
            .unwrap();
        assert!(
            history_charge >= (history_heap + actor_shadow_heap) as u64,
            "HISTORY_AND_ACTOR_LIVE_UNDERCHARGED {history_charge} < {history_heap} + {actor_shadow_heap}"
        );
        assert!(
            lineage_charge >= lineage_heap as u64,
            "LINEAGE_LIVE_UNDERCHARGED {lineage_charge} < {lineage_heap}"
        );
        if clips == 400 && ripple {
            assert!(
                history_charge + lineage_charge <= kinewright_project::history_record::TASK_LIVE,
                "REQUIRED_400_200_FITS history={history_charge} lineage={lineage_charge}"
            );
        }
        let result = json!({"clips": clips, "edits": edits, "ripple_delete_20": ripple,
            "history_encoded": history_bytes.len(), "lineage_encoded": lineage_bytes.len(),
            "boundary_documents_encoded": boundary_documents_encoded, "history_charge":history_charge,"lineage_charge":lineage_charge,
            "history_heap": history_heap, "lineage_heap": lineage_heap, "combined_heap": history_heap + lineage_heap,
            "actor_shadow_heap": actor_shadow_heap,
            "combined_peak": peak, "restore_ms": elapsed.as_secs_f64() * 1000.0,
            "raw_metadata_decoded_limit": 512_u64 << 20});
        println!("STAGE1_CAPACITY_HEAP={result}");
        results.push(result);
    }
    for (clips, operations_per_merge) in
        [(40, 200), (120, 200), (400, 200), (1200, 200), (1200, 80)]
    {
        let core = Core::spawn(document(clips)).unwrap();
        let mut rows = vec![];
        for merges in 0..=16 {
            if merges > 0 {
                let delta = (0..operations_per_merge)
                    .map(|index| Operation::SetTrackSyncLock {
                        track: TrackId(1),
                        locked: index % 2 == 0,
                    })
                    .collect();
                assert!(matches!(
                    core.request(Command::DoBatch(delta)).unwrap(),
                    Event::DocumentChanged { .. }
                ));
            }
            let history = core.export_history().unwrap();
            let lineage =
                kinewright_project::task_history_admission::lineage_for_fixture(&history).unwrap();
            let (history_charge, lineage_charge) =
                kinewright_project::task_history_admission::live_history_bytes_for_fixture(
                    &history, &lineage,
                )
                .unwrap();
            rows.push(json!({"merges":merges,"history_charge":history_charge,"lineage_charge":lineage_charge,
                "fits_candidate_4_mib":history_charge+lineage_charge <= 4 << 20,
                "fits_candidate_2_mib":history_charge+lineage_charge <= 2 << 20}));
            if history_charge + lineage_charge > 4 << 20 {
                break;
            }
        }
        results.push(json!({"candidate_clips":clips,"operations_per_merge":operations_per_merge,"rows":rows}));
    }
    let report = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/history-capacity/stage1-combined-heap.json");
    std::fs::create_dir_all(report.parent().unwrap()).unwrap();
    std::fs::write(report, serde_json::to_vec_pretty(&results).unwrap()).unwrap();
}
