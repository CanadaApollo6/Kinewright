//! On-demand whole-save and fsync measurements for changed native owner records.
//! Run with `cargo test -p kinewright-project --release --features test-util
//! --test save_cost_measure -- --ignored --nocapture`.
//! `SAVE_COST_POINTS` selects edit counts (default `100,400,1600,3000`), and
//! `SAVE_COST_SAVES` selects timed changed saves per point (default 12).
//! The retired journal support-field walk and save-side floor are not measured.
#![cfg(all(feature = "test-util", target_os = "linux"))]

use std::{path::Path, time::Instant};

use kinewright_core::{Command, Document, Event, Operation, Track, TrackId, TrackKind};
use kinewright_media::test_support::TempDirectory;
use kinewright_project::operation_actor::Core;
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
                name: None,
                id: TrackId(id),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            })
            .collect(),
        ..Document::default()
    };
    let core = Core::for_test(document).unwrap();
    for i in 0..edits {
        edit(&core, i);
    }
    let mut owner = ProjectBundle::activate(
        &path,
        BundleWriteAdmission::new_document(),
        core.history_pool(),
    )
    .unwrap();
    let inputs = owner.empty_inputs().unwrap().reference.clone();
    owner.save(&core, &inputs).unwrap();
    let mut next = edits;
    for _ in 0..3 {
        edit(&core, next);
        next += 1;
        owner.save(&core, &inputs).unwrap();
    }
    let (mut totals, mut sync_calls, mut sync_ms) = (vec![], vec![], vec![]);
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
    }
    let batches = core.export_history().unwrap().history().undo.len();
    let low = totals.iter().copied().fold(f64::MAX, f64::min);
    let high = totals.iter().copied().fold(0.0, f64::max);
    println!(
        "SAVE_COST edits={edits} batches={batches} saves={saves} save_ms_median={:.1} \
         save_ms_min={low:.1} save_ms_max={high:.1} fsync_calls={} fsync_ms={:.1}",
        median(totals),
        median(sync_calls),
        median(sync_ms),
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
