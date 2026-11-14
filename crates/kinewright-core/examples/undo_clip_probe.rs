//! Fixed heavy-fixture copy/ripple/split lanes and allocation-phase counters.
use kinewright_core::{
    ClipId, Command, Core, Document, Event, Operation, Query, QueryResult, TimeCode,
    undo::{History, PhaseCounters, RetentionPolicy, RoleBounds},
};
#[path = "../tests/support/undo_fixture.rs"]
mod fixture;
use std::{collections::BTreeMap, sync::Arc, time::Instant};
#[allow(
    clippy::large_enum_variant,
    reason = "one fixed benchmark engine; no extra indirection in either timed lane"
)]
enum Engine {
    Old(Core),
    New(History),
}
impl Engine {
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
                    panic!("document query")
                };
                document
            }
            Self::New(history) => Arc::clone(history.document()),
        }
    }
}
fn main() {
    let side = std::env::args().nth(1).expect("old or new");
    let layout = serde_json::json!({"step":std::mem::size_of::<kinewright_core::undo::Step>(),"operation":std::mem::size_of::<Operation>(),"clip":std::mem::size_of::<kinewright_core::Clip>(),"identity":std::mem::size_of::<kinewright_core::OperationIdentityEffect>(),"command_effects":std::mem::size_of::<kinewright_core::undo::CommandEffects>()});
    if side == "layout" {
        println!("{layout}");
        return;
    }
    let document = fixture::heavy();
    let mut engine = match side.as_str() {
        "old" => Engine::Old(Core::spawn(document).unwrap()),
        "new" => Engine::New(
            History::new(document, RetentionPolicy::Fold, RoleBounds::ORDINARY).unwrap(),
        ),
        _ => panic!("old or new"),
    };
    for index in 0..2000 {
        engine.execute(Command::Do(Operation::SetTrackSyncLock {
            track: kinewright_core::TrackId(1),
            locked: index % 2 == 1,
        }));
    }
    let mut metrics = BTreeMap::new();
    let mut phases = BTreeMap::new();
    for (name, operation) in [
        (
            "copy_ms",
            Operation::CopyClipAttributes {
                from_clip: ClipId(1),
                to_clip: ClipId(400),
                names: None,
                include_keyframes: true,
            },
        ),
        ("ripple_ms", Operation::RippleDeleteClip { clip: ClipId(1) }),
        (
            "split_ms",
            Operation::SplitClip {
                clip: ClipId(1),
                at: TimeCode(20),
            },
        ),
    ] {
        let before = engine.document();
        let counters = PhaseCounters::read();
        let start = Instant::now();
        engine.execute(Command::Do(operation.clone()));
        let elapsed = start.elapsed().as_secs_f64() * 1000.;
        let delta = PhaseCounters::read().since(counters);
        metrics.insert(name, elapsed);
        phases.insert(name, delta);
        // The oracle is an independent document value and runs outside timing
        // and counter snapshots, as in the main paired benchmark.
        let mut expected = (*before).clone();
        operation.apply(&mut expected).unwrap();
        assert_eq!(
            *engine.document(),
            expected,
            "CLIP_PROBE_EXPECTED_DOCUMENT {name}"
        );
        engine.execute(Command::Undo);
        assert_eq!(engine.document(), before, "CLIP_PROBE_UNDO_DOCUMENT {name}");
    }
    println!(
        "{}",
        serde_json::json!({"side":side,"layout":layout,"metrics":metrics,"phases":phases,"errors":0})
    );
}
