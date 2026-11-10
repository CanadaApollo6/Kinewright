//! On-demand measurement for the ownership fixes (W2): one save of a session
//! with thousands of history batches. Each timed save follows one more edit,
//! so every save publishes a changed history through the commit path.
//!
//! Run with `cargo test -p kinewright-project --release --features test-util
//! --test ownership_w2_measure -- --ignored --nocapture`. It prints one
//! `W2_SAVE` line.
#![cfg(all(feature = "test-util", target_os = "linux"))]

use std::path::Path;

use kinewright_core::{Command, Core, Document, Event, Operation, Track, TrackId, TrackKind};
use kinewright_media::test_support::TempDirectory;
use kinewright_project::bundle::{BundleWriteAdmission, ProjectBundle};

const BATCHES: u64 = 3_000;
const SAVES: u64 = 40;

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

#[test]
#[ignore = "on-demand measurement"]
fn w2_long_session_save_cost() {
    let temp = TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "w2-measure");
    let path = temp.path("w2.kinewright");
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
    for i in 0..BATCHES {
        edit(&core, i);
    }
    let mut owner = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    let inputs = owner.empty_inputs().unwrap().reference.clone();
    owner.save(&core, &inputs).unwrap();
    for i in 0..3 {
        edit(&core, BATCHES + i);
        owner.save(&core, &inputs).unwrap();
    }
    let mut elapsed = std::time::Duration::ZERO;
    for i in 0..SAVES {
        edit(&core, BATCHES + 3 + i);
        let started = std::time::Instant::now();
        owner.save(&core, &inputs).unwrap();
        elapsed += started.elapsed();
    }
    let batches = core.export_history().unwrap().batches.len();
    println!(
        "W2_SAVE batches={batches} saves={SAVES} per_save_ms={:.2}",
        elapsed.as_secs_f64() * 1e3 / f64::from(u32::try_from(SAVES).unwrap()),
    );
}
