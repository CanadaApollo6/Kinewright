use kinewright_core::{Command, Core, Document, Event, Operation, Track, TrackId, TrackKind};
use kinewright_project::{
    bundle_records::{BatchId, BranchId},
    history_record::HistoryRole,
    operation_history::{CreationOrigin, HistoryPool, OwnerHistory, PoolLimits, Role},
    storage_budget::StorageLimits,
};
fn document() -> Document {
    Document {
        tracks: vec![Track {
            id: TrackId(81),
            kind: TrackKind::Video,
            sync_lock: false,
            clips: vec![],
        }],
        ..Document::default()
    }
}
fn origin(n: u64) -> CreationOrigin {
    CreationOrigin {
        branch: serde_json::from_str::<BranchId>(&format!("\"{:032x}\"", 1)).unwrap(),
        batch: serde_json::from_str::<BatchId>(&format!("\"{n:032x}\"")).unwrap(),
    }
}
fn main() {
    let pool = HistoryPool::new(PoolLimits::default());
    let mut task = OwnerHistory::new(&pool, Role::Task, Role::Task.policy(), &origin(1), || {
        Ok(document())
    })
    .unwrap();
    for n in 0..2000 {
        task.execute(
            Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(81),
                locked: n % 2 != 0,
            }),
            origin(n + 2),
            vec![],
            None,
        )
        .unwrap();
    }
    let delta: Vec<_> = task.history().applied_delta().cloned().collect();
    let wire = serde_json::to_vec(&delta).unwrap();
    let independent_encoded: usize = delta
        .iter()
        .map(|operation| serde_json::to_vec(operation).unwrap().len())
        .sum();
    let old = Core::spawn(document()).unwrap();
    assert!(
        matches!(
            old.request(Command::DoBatch(serde_json::from_slice(&wire).unwrap()))
                .unwrap(),
            Event::DocumentChanged { .. }
        ),
        "OLD_CANDIDATE_ACCEPTS_THE_MERGE_BATCH"
    );
    let old_history = old.export_history().unwrap();
    let old_live =
        kinewright_core::history_size::restored_history_live_bytes(&old_history).unwrap();
    let old_cap = HistoryRole::Candidate.live_limit(&StorageLimits::default());
    let mut candidate = OwnerHistory::new(
        &pool,
        Role::Candidate,
        Role::Candidate.policy(),
        &origin(1),
        || Ok(document()),
    )
    .unwrap();
    let result = candidate.execute(Command::DoBatch(delta), origin(2002), vec![], None);
    let outcome = result.as_ref().map(|effects| effects.accepted);
    eprintln!(
        "HX_TASK_CANDIDATE_PROBE commands=2000 independent_encoded={independent_encoded} task_encoded={} task_decoded={} candidate_encoded_cap={} candidate_decoded_cap={} old_live={old_live} old_cap={old_cap} result={outcome:?}",
        task.history().charge().encoded,
        task.history().charge().decoded,
        Role::Candidate.bounds().operation_bytes,
        Role::Candidate.bounds().decoded_history_bytes
    );
    assert!(
        result.is_ok(),
        "ADMITTED_TASK_DELTA_MUST_FIT_CANDIDATE_MERGE_PATH"
    );
}
