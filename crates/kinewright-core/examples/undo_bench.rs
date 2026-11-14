#![allow(
    clippy::cast_precision_loss,
    reason = "measurement JSON reports byte counts and durations as f64"
)]
//! Paired raw-Core / operation-history measurements. No media files or GUI are opened.
#[path = "../tests/support/undo_fixture.rs"]
mod fixture;
use kinewright_core::undo::{History, PhaseCounters, Record, RetentionPolicy, RoleBounds};
use kinewright_core::{Command, Core, Document, Event, Operation, Query, QueryResult, TrackId};
use std::{collections::BTreeMap, sync::Arc, time::Instant};

#[allow(
    clippy::large_enum_variant,
    reason = "the benchmark owns exactly one executor"
)]
enum Engine {
    Old(Core),
    New(History),
}
impl Engine {
    fn new(side: &str, document: Document, limits: RoleBounds) -> Self {
        match side {
            "old" => Self::Old(Core::spawn(document).unwrap()),
            "new" => Self::New(History::new(document, RetentionPolicy::Fold, limits).unwrap()),
            _ => panic!("old or new"),
        }
    }
    fn execute(&mut self, command: Command) {
        match self {
            Self::Old(core) => assert!(matches!(
                core.request(command).unwrap(),
                Event::DocumentChanged { .. }
            )),
            Self::New(history) => {
                std::hint::black_box(history.execute(command).unwrap());
            }
        }
    }
    fn document(&self) -> Arc<Document> {
        match self {
            Self::Old(core) => {
                let Event::QueryResult(QueryResult::Document(document)) =
                    core.request(Command::Query(Query::Document)).unwrap()
                else {
                    panic!("document")
                };
                document
            }
            Self::New(history) => Arc::clone(history.document()),
        }
    }
    fn depth(&self) -> usize {
        match self {
            Self::Old(core) => core.export_history().unwrap().cursor.undo.len(),
            Self::New(history) => history.undo().len(),
        }
    }
    fn top_operation_count(&self) -> usize {
        match self {
            Self::Old(core) => core
                .export_history()
                .unwrap()
                .cursor
                .undo
                .last()
                .unwrap()
                .operations
                .len(),
            Self::New(history) => history.undo().back().unwrap().operations().count(),
        }
    }
    fn memory(&self) -> u64 {
        match self {
            Self::Old(core) => {
                kinewright_core::history_size::history_live_bytes(&core.export_history().unwrap())
                    .unwrap()
            }
            Self::New(history) => history.retained_live_bound().unwrap(),
        }
    }
}
fn limits(checkpoints: usize) -> RoleBounds {
    RoleBounds {
        checkpoint_documents: checkpoints,
        ..RoleBounds::ORDINARY
    }
}

fn edit(index: u64) -> Command {
    Command::Do(Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked: index % 2 == 1,
    })
}
thread_local! {static PHASES:std::cell::RefCell<BTreeMap<String,serde_json::Value>>=const{std::cell::RefCell::new(BTreeMap::new())};}
fn phase(name: &str, before: PhaseCounters, commands: u64) {
    let count = PhaseCounters::read().since(before);
    PHASES.with_borrow_mut(|phases|{
        let row=phases.entry(name.to_string()).or_insert_with(||serde_json::json!({"commands":0,"document_copies":0,"exact_measurements":0,"document_bytes_serialised":0,"bytes_serialised":0,"kernel_validation_passes":0,"kernel_duration_passes":0,"kernel_identity_passes":0}));
        row["commands"]=serde_json::json!(row["commands"].as_u64().unwrap()+commands);
        for (key,value) in serde_json::to_value(count).unwrap().as_object().unwrap(){row[key]=serde_json::json!(row[key].as_u64().unwrap()+value.as_u64().unwrap());}
    });
}
fn timed(engine: &mut Engine, command: Command, name: &str) -> f64 {
    let before = PhaseCounters::read();
    let start = Instant::now();
    engine.execute(command);
    let elapsed = start.elapsed().as_secs_f64() * 1000.;
    phase(name, before, 1);
    elapsed
}
fn quantile(samples: &[f64], percent: usize) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[(sorted.len() * percent).div_ceil(100).saturating_sub(1)]
}
fn rss() -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find(|line| line.starts_with("VmRSS:"))?
        .split_whitespace()
        .nth(1)?
        .parse::<u64>()
        .ok()
        .map(|n| n * 1024)
}
#[expect(
    clippy::too_many_lines,
    reason = "one benchmark lane keeps the fixture, executor and ordered measurement stream together"
)]
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let side = args.get(1).expect("old or new");
    let checkpoints = args.get(2).map_or(4, |s| s.parse().unwrap());
    let document = fixture::heavy();
    let document_live = kinewright_core::history_size::document_live_bytes(&document).unwrap();
    let fixture_json = serde_json::to_vec(&document).unwrap().len();
    if side == "fixture" {
        println!(
            "{}",
            serde_json::json!({"document_live_bytes":document_live,"fixture_json_bytes":fixture_json,"step_size":std::mem::size_of::<kinewright_core::undo::Step>(),"command_size":std::mem::size_of::<kinewright_core::undo::StepCommand>(),"operation_size":std::mem::size_of::<Operation>()})
        );
        return;
    }
    let rss_start = rss();
    let mut rows = BTreeMap::new();
    let mut samples = BTreeMap::new();
    // This scenario owns a fresh fixture; the original remains the expected walk boundary.
    let mut engine = Engine::new(side, document.clone(), limits(checkpoints));
    for index in 0..2000 {
        engine.execute(edit(index));
    }
    assert_eq!(engine.depth(), 2000);
    rows.insert("retained_charge_bytes".into(), engine.memory() as f64);
    rows.insert("retained_rss_total_bytes".into(), rss().unwrap_or(0) as f64);
    rows.insert("startup_rss_bytes".into(), rss_start.unwrap_or(0) as f64);
    rows.insert(
        "retained_rss_delta_bytes".into(),
        rss().unwrap_or(0).saturating_sub(rss_start.unwrap_or(0)) as f64,
    );
    rows.insert("document_live_bytes".into(), document_live as f64);
    rows.insert("fixture_json_bytes".into(), fixture_json as f64);
    let mut single = vec![];
    for _ in 0..32 {
        single.push(timed(&mut engine, Command::Undo, "single_undo"));
        engine.execute(Command::Redo);
    }
    rows.insert("single_undo_ms".into(), quantile(&single, 50));
    samples.insert("single_undo_ms".into(), single);
    let mut open_refused = false;
    match &engine {
        Engine::Old(core) => {
            let captured = core.export_history().unwrap();
            let transport = kinewright_core::HistoryTransport::capture(&captured).unwrap();
            let counters = PhaseCounters::read();
            let start = Instant::now();
            let opened = transport.resolve_verified();
            rows.insert("open_ms".into(), start.elapsed().as_secs_f64() * 1000.0);
            phase("open", counters, 1);
            assert!(
                matches!(opened, Err(kinewright_core::HistoryError::Capacity)),
                "old bounded transport at 2000 heavy steps"
            );
            open_refused = true;
        }
        Engine::New(history) => {
            let bytes = history.record().encode().unwrap();
            let counters = PhaseCounters::read();
            let start = Instant::now();
            let snapshot = Record::decode(&bytes, limits(checkpoints)).unwrap();
            let mut opened = History::new(
                Document::default(),
                RetentionPolicy::Fold,
                limits(checkpoints),
            )
            .unwrap();
            opened.install_snapshot(snapshot).unwrap();
            rows.insert("open_ms".into(), start.elapsed().as_secs_f64() * 1000.0);
            phase("open", counters, 1);
            assert_eq!(opened.document(), history.document());
            let counters = PhaseCounters::read();
            let start = Instant::now();
            std::hint::black_box(opened.execute(Command::Undo).unwrap());
            rows.insert(
                "cold_undo_ms".into(),
                start.elapsed().as_secs_f64() * 1000.0,
            );
            phase("cold_undo", counters, 1);
            assert_eq!(opened.undo().len(), 1999, "COLD_UNDO_DEPTH");
            assert!(!opened.document().tracks[0].sync_lock, "COLD_UNDO_DOCUMENT");
        }
    }
    let gesture = |index: u64| Command::DoBatchCoalesced {
        operations: vec![Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: index.is_multiple_of(2),
        }],
        coalesce_key: "120-frame-drag".into(),
    };
    let counters = PhaseCounters::read();
    let start = Instant::now();
    for index in 0..120 {
        engine.execute(gesture(index));
    }
    rows.insert(
        "gesture_apply_ms".into(),
        start.elapsed().as_secs_f64() * 1000.0,
    );
    phase("gesture_apply", counters, 120);
    assert_eq!(engine.depth(), 2001);
    assert_eq!(engine.top_operation_count(), 120);
    assert!(!engine.document().tracks[0].sync_lock);
    rows.insert(
        "gesture_undo_ms".into(),
        timed(&mut engine, Command::Undo, "gesture_undo"),
    );
    assert!(engine.document().tracks[0].sync_lock);
    rows.insert(
        "gesture_redo_ms".into(),
        timed(&mut engine, Command::Redo, "gesture_redo"),
    );
    assert!(!engine.document().tracks[0].sync_lock);
    engine.execute(Command::Undo);
    let copy = Command::Do(Operation::CopyClipAttributes {
        from_clip: kinewright_core::ClipId(1),
        to_clip: kinewright_core::ClipId(400),
        names: None,
        include_keyframes: true,
    });
    rows.insert("copy_ms".into(), timed(&mut engine, copy, "copy"));
    let copied = engine.document();
    assert_eq!(copied.tracks[0].clips[399].effects.len(), 16);
    assert_eq!(
        copied.tracks[0].clips[399]
            .effects
            .iter()
            .flat_map(|effect| effect.keyframes.values())
            .map(|curve| curve.keyframes.len())
            .sum::<usize>(),
        128
    );
    drop(copied);
    engine.execute(Command::Undo);
    let ripple = Command::Do(Operation::RippleDeleteClip {
        clip: kinewright_core::ClipId(1),
    });
    rows.insert("ripple_ms".into(), timed(&mut engine, ripple, "ripple"));
    let rippled = engine.document();
    assert_eq!(rippled.tracks[0].clips.len(), 399);
    assert_eq!(rippled.tracks[0].clips[0].id, kinewright_core::ClipId(2));
    assert_eq!(
        rippled.tracks[0].clips[0].timeline_start,
        kinewright_core::TimeCode(0)
    );
    assert_eq!(
        rippled.tracks[0].clips[398].timeline_start,
        kinewright_core::TimeCode(23_880)
    );
    drop(rippled);
    engine.execute(Command::Undo);
    assert_eq!(engine.depth(), 2000);
    assert_eq!(engine.document().tracks[0].clips.len(), 400);
    let mut walk = vec![];
    for index in 0..2000 {
        walk.push(timed(&mut engine, Command::Undo, "walk"));
        assert_eq!(
            engine.document().tracks[0].sync_lock,
            index % 2 == 1,
            "EVERY_UNDO_REACHES_ITS_BOUNDARY"
        );
    }
    assert_eq!(engine.depth(), 0);
    assert_eq!(engine.document(), Arc::new(document.clone()));
    rows.insert("walk_median_ms".into(), quantile(&walk, 50));
    rows.insert("walk_p95_ms".into(), quantile(&walk, 95));
    rows.insert("walk_worst_ms".into(), quantile(&walk, 100));
    samples.insert("walk_ms".into(), walk);
    let mut large = Engine::new(side, document.clone(), limits(checkpoints));
    let mut count = 0;
    for target in [1000, 10_000] {
        while count < target {
            large.execute(edit(count));
            count += 1;
        }
        let mut times = vec![];
        for _ in 0..32 {
            times.push(timed(&mut large, edit(count), &format!("edit_{target}")));
            count += 1;
        }
        assert_eq!(large.document().tracks[0].sync_lock, (count - 1) % 2 == 1);
        assert_eq!(large.depth(), usize::try_from(count).unwrap());
        rows.insert(format!("edit_{target}_ms"), quantile(&times, 50));
        samples.insert(format!("edit_{target}_ms"), times);
    }
    let mut fold_limits = limits(checkpoints);
    fold_limits.operation_bytes = 500;
    let mut folded = Engine::new(side, document, fold_limits);
    for index in 0..32 {
        folded.execute(edit(index));
    }
    if let Engine::New(history) = &folded {
        assert_eq!(history.undo().len(), 10, "FOLDING_HAS_STARTED");
        assert_eq!(history.undo().front().unwrap().commands()[0].position, 22);
    }
    let mut fold = vec![];
    for index in 32..64 {
        fold.push(timed(&mut folded, edit(index), "fold"));
        if let Engine::New(history) = &folded {
            assert_eq!(
                history.baseline().tracks[0].sync_lock,
                index % 2 == 1,
                "EVERY_FOLD_ADVANCES_BASELINE"
            );
        }
    }
    if let Engine::New(history) = &folded {
        assert_eq!(history.undo().len(), 10, "STEADY_FOLD_DEPTH");
        assert_eq!(history.charge().encoded, 465, "STEADY_FOLD_LITERAL_CHARGE");
        assert_eq!(history.undo().front().unwrap().commands()[0].position, 54);
    }
    rows.insert("fold_ms".into(), quantile(&fold, 50));
    samples.insert("fold_ms".into(), fold);
    let phases = PHASES.with_borrow(Clone::clone);
    if side == "new" {
        for name in ["edit_1000", "edit_10000", "gesture_apply"] {
            let p = &phases[name];
            assert_eq!(p["document_copies"], p["commands"], "BENCH_ONE_COPY {name}");
            assert_eq!(
                p["exact_measurements"], 0,
                "BENCH_NO_EXACT_MEASUREMENT {name}"
            );
            assert_eq!(
                p["document_bytes_serialised"], 0,
                "BENCH_NO_DOCUMENT_SERIALIZATION {name}"
            );
        }
    }
    println!(
        "{}",
        serde_json::json!({"history_counters_available":side=="new","phases":phases,"side":side,"checkpoints":checkpoints,"metrics":rows,"samples":samples,"expected_old_open_refusal":open_refused,"errors":0})
    );
}
