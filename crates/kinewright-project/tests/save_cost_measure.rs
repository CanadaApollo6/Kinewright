//! On-demand measurement: what one save of a long session costs, and how much of it is
//! the history support-field walk. Each timed save follows one more edit, so every save
//! publishes a changed history through the commit path (the `ownership_w2_measure` shape).
//!
//! Run with `cargo test -p kinewright-project --release --features test-util
//! --test save_cost_measure -- --ignored --nocapture`. For each point it prints one
//! `SAVE_COST` line: the median, minimum and maximum whole-save time, the fsync calls and
//! time inside a save (from the `io_observer` sync profile), and the median time of one
//! `history_support_fields` walk over that save's history (timed outside the save; a save
//! runs it twice). `LOADAVG` lines bracket the run.
//!
//! `SAVE_COST_POINTS` sets the edit counts (default `100,400,1600,3000`; 3,000 edits holds
//! the history at the 32 MiB cap, so every save there floors one batch) and `SAVE_COST_SAVES`
//! the timed saves per point (default 12).
#![cfg(all(feature = "test-util", target_os = "linux"))]

use std::{path::Path, time::Instant};

use kinewright_core::{Command, Core, Document, Event, Operation, Track, TrackId, TrackKind};
use kinewright_media::test_support::TempDirectory;
use kinewright_project::{
    bundle::{BundleWriteAdmission, ProjectBundle},
    io_observer,
};

fn edit(core: &Core, i: u64) {
    assert!(matches!(
        core.request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: i.is_multiple_of(2),
        }))
        .unwrap(),
        Event::DocumentChanged { .. }
    ));
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    if n % 2 == 1 {
        values[n / 2]
    } else {
        f64::midpoint(values[n / 2 - 1], values[n / 2])
    }
}

fn millis(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1e3
}

fn load_average() -> String {
    std::fs::read_to_string("/proc/loadavg").map_or_else(|_| "unknown".into(), |l| l.trim().into())
}

fn point(edits: u64, saves: u64) {
    let temp = TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "save-cost");
    let path = temp.path("p.kinewright");
    let document = Document {
        tracks: (1..=8)
            .map(|id| Track {
                id: TrackId(id),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            })
            .collect(),
        ..Document::default()
    };
    let core = Core::spawn(document).unwrap();
    for i in 0..edits {
        edit(&core, i);
    }
    let mut owner = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    let inputs = owner.empty_inputs().unwrap().reference.clone();
    owner.save(&core, &inputs).unwrap();
    let mut next = edits;
    for _ in 0..3 {
        edit(&core, next);
        next += 1;
        owner.save(&core, &inputs).unwrap();
    }
    let (mut totals, mut sync_calls, mut sync_ms, mut walks) = (vec![], vec![], vec![], vec![]);
    for _ in 0..saves {
        edit(&core, next);
        next += 1;
        let started = Instant::now();
        let (result, sync) = io_observer::profile_sync(|| owner.save(&core, &inputs));
        totals.push(millis(started));
        result.unwrap();
        #[allow(clippy::cast_precision_loss, reason = "a measurement report")]
        {
            sync_calls.push(sync.calls as f64);
            sync_ms.push(sync.nanoseconds as f64 / 1e6);
        }
        // The walk alone, over the history as the save serialized it.
        let history = serde_json::to_value(core.export_history().unwrap()).unwrap();
        let started = Instant::now();
        let fields = kinewright_core::history_support_fields(&history).unwrap();
        walks.push(millis(started));
        assert!(!fields.is_empty());
    }
    let batches = core.export_history().unwrap().batches.len();
    let low = totals.iter().copied().fold(f64::MAX, f64::min);
    let high = totals.iter().copied().fold(0.0, f64::max);
    println!(
        "SAVE_COST edits={edits} batches={batches} saves={saves} save_ms_median={:.1} \
         save_ms_min={low:.1} save_ms_max={high:.1} fsync_calls={} fsync_ms={:.1} \
         walk_ms_median={:.1}",
        median(totals),
        median(sync_calls),
        median(sync_ms),
        median(walks),
    );
}

#[test]
#[ignore = "on-demand measurement"]
fn long_session_save_cost() {
    let points: Vec<u64> = std::env::var("SAVE_COST_POINTS")
        .unwrap_or_else(|_| "100,400,1600,3000".into())
        .split(',')
        .map(|point| point.trim().parse().unwrap())
        .collect();
    let saves = std::env::var("SAVE_COST_SAVES")
        .ok()
        .and_then(|saves| saves.parse().ok())
        .unwrap_or(12);
    println!("LOADAVG start {}", load_average());
    for edits in points {
        point(edits, saves);
    }
    println!("LOADAVG end {}", load_average());
}
