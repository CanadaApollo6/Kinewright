//! Paired shipping actor/owner measurements. No GUI or provider is launched.
#![allow(
    clippy::cast_precision_loss,
    reason = "measurement JSON reports integer counts as f64"
)]
#[path = "../../kinewright-core/tests/support/undo_fixture.rs"]
mod fixture;
use kinewright_core::undo::{PhaseCounters, RoleBounds};
use kinewright_core::{
    Command, Core as OldCore, Document, Event, Operation, Query, QueryResult, TrackId,
};
use kinewright_project::{
    bundle::BundleSaveReport,
    bundle_records::{BatchId, ObjectRef, RecordKind},
    editing_session::Ja1EditingSession,
    operation_actor::{CommandContext, Core},
    operation_history::{
        Approval, CreationOrigin, HistoryPool, OwnerCounters, PoolLimits, RemovedTarget,
    },
    storage_budget::StorageLimits,
};
use std::{collections::BTreeMap, sync::Arc, time::Instant};
const EVIDENCE: &[u8] = b"owner benchmark approval";
fn evidence() -> ObjectRef {
    ObjectRef {
        sha256: kinewright_media::sha256_bytes(EVIDENCE),
        byte_length: EVIDENCE.len() as u64,
        record_kind: RecordKind::OwnedEvidence,
    }
}
enum Engine {
    Old(OldCore),
    Product(Core),
}
impl Engine {
    fn new(side: &str, document: Document, folding: bool) -> Self {
        match side {
            "old" => Self::Old(OldCore::spawn(document).unwrap()),
            "product" => Self::Product(
                Core::for_test_with_bounds(
                    document,
                    RoleBounds {
                        operation_bytes: if folding {
                            500
                        } else {
                            RoleBounds::ORDINARY.operation_bytes
                        },
                        ..RoleBounds::ORDINARY
                    },
                )
                .unwrap(),
            ),
            _ => panic!("old or product"),
        }
    }
    fn execute(&self, command: Command) {
        let event = match self {
            Self::Old(core) => core.request(command).unwrap(),
            Self::Product(core) => core.request(command).unwrap(),
        };
        assert!(
            matches!(event, Event::DocumentChanged { .. }),
            "BENCH_ACCEPTED_COMMAND"
        );
    }
    fn approve(&self) {
        let command = Command::DoBatch(vec![Operation::DeleteClip {
            clip: kinewright_core::ClipId(400),
        }]);
        match self {
            Self::Old(_) => self.execute(command),
            Self::Product(core) => {
                let event = core
                    .request_with_context(
                        command,
                        CommandContext {
                            creation: CreationOrigin {
                                branch: core.creation_branch().unwrap(),
                                batch: BatchId::fresh().unwrap(),
                            },
                            inputs: vec![],
                            approval: Some(Approval {
                                reference: evidence(),
                                targets: vec![RemovedTarget::Clip(kinewright_core::ClipId(400))],
                            }),
                            transition: None,
                        },
                    )
                    .unwrap();
                assert!(matches!(event, Event::DocumentChanged { .. }));
            }
        }
        self.execute(Command::Undo);
    }
    fn document(&self) -> Arc<Document> {
        let event = match self {
            Self::Old(core) => core.request(Command::Query(Query::Document)).unwrap(),
            Self::Product(core) => core.request(Command::Query(Query::Document)).unwrap(),
        };
        let Event::QueryResult(QueryResult::Document(document)) = event else {
            panic!("document")
        };
        document
    }
    fn depth(&self) -> usize {
        match self {
            Self::Old(core) => core.export_history().unwrap().cursor.undo.len(),
            Self::Product(core) => core.export_history().unwrap().history().undo.len(),
        }
    }
    fn counters(&self) -> Option<(PhaseCounters, OwnerCounters)> {
        match self {
            Self::Old(_) => None,
            Self::Product(core) => Some(core.counters().unwrap()),
        }
    }
    fn timed(
        &self,
        command: Command,
        lane: &str,
        phases: &mut BTreeMap<String, serde_json::Value>,
    ) -> f64 {
        let before = self.counters();
        let start = Instant::now();
        self.execute(command);
        let elapsed = start.elapsed().as_secs_f64() * 1000.;
        if let Some((history, owner)) = before {
            let after = self.counters().unwrap();
            let row = phases
                .entry(lane.into())
                .or_insert_with(|| serde_json::json!({"commands":0}));
            row["commands"] = (row["commands"].as_u64().unwrap() + 1).into();
            for (prefix, values) in [
                ("", serde_json::to_value(after.0.since(history)).unwrap()),
                (
                    "owner_",
                    serde_json::to_value(after.1.since(owner)).unwrap(),
                ),
            ] {
                for (key, value) in values.as_object().unwrap() {
                    let name = format!("{prefix}{key}");
                    let sum = row
                        .get(&name)
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0)
                        + value.as_u64().unwrap();
                    row[&name] = sum.into();
                }
            }
        }
        elapsed
    }
}
fn edit(index: u64) -> Command {
    Command::Do(Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked: index % 2 == 1,
    })
}
fn quantile(samples: &mut [f64], percent: usize) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[(samples.len() * percent).div_ceil(100).saturating_sub(1)]
}
fn cold_product(core: &Core) -> (f64, f64, serde_json::Value) {
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "undo-product-bench",
    );
    let mut session = Ja1EditingSession::new(temp.root(), core, StorageLimits::default()).unwrap();
    let mut proof_bytes = EVIDENCE;
    let proof = session
        .source_mut()
        .unwrap()
        .store_mut()
        .admit_blob(&mut proof_bytes, &evidence(), &[])
        .unwrap();
    let status = session
        .first_save(&temp.path("project.kinewright"), StorageLimits::default())
        .unwrap();
    let kinewright_project::bundle_copy::CopyStatus::Committed(BundleSaveReport {
        history_sha256,
        ..
    }) = status
    else {
        panic!("bench save")
    };
    drop(proof);
    // Release the publishing namespace and retire its actor before a real cold
    // open. The next timed lane constructs its own unchanged fixture.
    drop(session);
    let pool = HistoryPool::new(PoolLimits::default());
    let counters = PhaseCounters::read();
    let owner = OwnerCounters::read();
    let start = Instant::now();
    let mut reopened = Ja1EditingSession::open(&temp.path("project.kinewright"), &pool).unwrap();
    let opened = reopened.core();
    let open_ms = start.elapsed().as_secs_f64() * 1000.;
    assert_eq!(
        opened.snapshot_identity().unwrap().to_string(),
        history_sha256,
        "PRODUCT_OPEN_COMBINED_IDENTITY"
    );
    let mut phases = serde_json::json!({"history":PhaseCounters::read().since(counters), "owner":OwnerCounters::read().since(owner)});
    // The GUI's live producer borrows one replay anchor. Bind it outside the
    // timer so cold Undo uses the actual product cache window, not the larger
    // unbound-reader window. The writer publication/acknowledgement is excluded.
    let lease = reopened
        .issue_manual_lease("benchmark".into(), "benchmark".into())
        .unwrap();
    let inputs = reopened.manual_input_context().unwrap();
    let permit = reopened
        .bind_manual_producer(
            lease,
            inputs,
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
        )
        .unwrap();
    let (undo_counters, undo_owner) = reopened.core().counters().unwrap();
    let start = Instant::now();
    permit.request(Command::Undo).unwrap();
    let undo_ms = start.elapsed().as_secs_f64() * 1000.;
    let (after, owner) = reopened.core().counters().unwrap();
    phases["cold_undo"] =
        serde_json::json!({"history":after.since(undo_counters), "owner":owner.since(undo_owner)});
    let opened = reopened.core();
    assert_eq!(
        opened.export_history().unwrap().history().undo.len(),
        1999,
        "PRODUCT_COLD_UNDO_DEPTH"
    );
    assert!(
        !opened.export_history().unwrap().history().document.tracks[0].sync_lock,
        "PRODUCT_COLD_UNDO_DOCUMENT"
    );
    (open_ms, undo_ms, phases)
}
#[allow(
    clippy::too_many_lines,
    reason = "one fixed paired protocol keeps the historical fixture and sample counts visible"
)]
fn main() {
    let side = std::env::args().nth(1).expect("old or product");
    let mut metrics = BTreeMap::new();
    let mut samples = BTreeMap::new();
    let mut phases = BTreeMap::new();
    let engine = Engine::new(&side, fixture::heavy(), false);
    engine.approve();
    for index in 0..2000 {
        engine.execute(edit(index));
    }
    assert_eq!(engine.depth(), 2000);
    if let Engine::Product(core) = &engine {
        let (open, undo, phase) = cold_product(core);
        metrics.insert("open_ms".to_owned(), open);
        metrics.insert("cold_undo_ms".to_owned(), undo);
        phases.insert("open".to_owned(), phase);
    }
    drop(engine);
    let engine = Engine::new(&side, fixture::heavy(), false);
    engine.approve();
    for index in 0..2000 {
        engine.execute(edit(index));
    }
    let start = Instant::now();
    let before = engine.counters();
    for index in 0..120 {
        engine.execute(Command::DoBatchCoalesced {
            operations: vec![Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: index % 2 == 0,
            }],
            coalesce_key: "120-frame-drag".into(),
        });
    }
    metrics.insert(
        "gesture_apply_ms".into(),
        start.elapsed().as_secs_f64() * 1000.,
    );
    if let Some((history, owner)) = before {
        let after = engine.counters().unwrap();
        phases.insert("gesture_apply".into(), serde_json::json!({"commands":120, "history":after.0.since(history), "owner":after.1.since(owner)}));
    }
    assert_eq!(engine.depth(), 2001);
    assert!(!engine.document().tracks[0].sync_lock);
    engine.execute(Command::Undo);
    metrics.insert(
        "gesture_redo_ms".into(),
        engine.timed(Command::Redo, "gesture_redo", &mut phases),
    );
    engine.execute(Command::Undo);
    let mut walk = Vec::new();
    for index in 0..2000 {
        walk.push(engine.timed(Command::Undo, "walk", &mut phases));
        assert_eq!(
            engine.document().tracks[0].sync_lock,
            index % 2 == 1,
            "PRODUCT_WALK_EVERY_BOUNDARY"
        );
    }
    assert_eq!(engine.depth(), 0);
    assert_eq!(*engine.document(), fixture::heavy());
    metrics.insert("walk_p95_ms".into(), quantile(&mut walk, 95));
    metrics.insert("walk_worst_ms".into(), quantile(&mut walk, 100));
    samples.insert("walk_ms".into(), walk);
    drop(engine);
    let engine = Engine::new(&side, fixture::heavy(), false);
    engine.approve();
    let mut count = 0;
    for depth in [1000, 10_000] {
        while count < depth {
            engine.execute(edit(count));
            count += 1;
        }
        let lane = format!("edit_{depth}");
        let mut times = Vec::new();
        for _ in 0..32 {
            times.push(engine.timed(edit(count), &lane, &mut phases));
            count += 1;
        }
        assert_eq!(engine.depth(), usize::try_from(count).unwrap());
        metrics.insert(format!("{lane}_ms"), quantile(&mut times, 50));
        samples.insert(format!("{lane}_ms"), times);
    }
    drop(engine);
    let folded = Engine::new(&side, fixture::heavy(), true);
    folded.approve();
    for index in 0..32 {
        folded.execute(edit(index));
    }
    let mut times = Vec::new();
    for index in 32..64 {
        times.push(folded.timed(edit(index), "fold", &mut phases));
    }
    if let Engine::Product(core) = &folded {
        let snapshot = core.export_history().unwrap();
        assert_eq!(
            snapshot.history().undo.len(),
            10,
            "PRODUCT_FOLD_STEADY_DEPTH"
        );
        assert_eq!(
            snapshot.history().undo.front().unwrap().commands()[0].position,
            56,
            "PRODUCT_FOLD_ADVANCES_FRONTIER"
        );
        for name in ["edit_1000", "edit_10000"] {
            let row = &phases[name];
            assert_eq!(
                row["document_copies"], row["commands"],
                "PRODUCT_WARM_ONE_DOCUMENT_COPY"
            );
            assert_eq!(
                row["exact_measurements"], 0,
                "PRODUCT_WARM_NO_DOCUMENT_MEASUREMENT"
            );
            assert_eq!(row["owner_document_scans"], 0, "PRODUCT_WARM_NO_OWNER_SCAN");
            assert_eq!(
                row["owner_owner_identity_hashes"], 0,
                "PRODUCT_WARM_NO_OWNER_HASH"
            );
        }
    }
    metrics.insert("fold_ms".into(), quantile(&mut times, 50));
    samples.insert("fold_ms".into(), times);
    let process_status = std::fs::read_to_string("/proc/self/status").unwrap();
    let rss_kib: u64 = process_status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    println!(
        "{}",
        serde_json::json!({"side":side,"metrics":metrics,"samples":samples,"phases":phases,"rss_kib":rss_kib,"errors":0})
    );
}
