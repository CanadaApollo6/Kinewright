//! Measure the production durable-history read at the exact default byte cap.
#[cfg(all(target_os = "linux", feature = "test-util"))]
fn main() {
    use kinewright_core::{Core, Document, HistorySnapshot, Marker, MarkerId, TimeCode};
    use kinewright_project::{bundle_copy::SessionStore, storage_budget::StorageLimits};
    use std::{path::PathBuf, sync::Arc, time::Instant};
    let parent = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .expect("controlled durable scratch parent"),
    );
    let mut doc = Document::default();
    doc.markers.push(Marker {
        id: MarkerId(1),
        position: TimeCode(0),
        label: String::new(),
        color_token: 0,
    });
    let core = Core::spawn(doc).unwrap();
    let mut history = core.export_history().unwrap();
    let cap = StorageLimits::default().branch_history_bytes;
    if !(cap - history.encoded_bytes().unwrap()).is_multiple_of(2) {
        history.floor_offset = 1;
    }
    let padding = usize::try_from((cap - history.encoded_bytes().unwrap()) / 2).unwrap();
    let mut doc = (*history.document).clone();
    doc.markers[0].label = "\u{1}".repeat(padding / 6) + &"m".repeat(padding % 6);
    history.document = Arc::new(doc);
    history.baseline = Arc::clone(&history.document);
    assert_eq!(history.encoded_bytes().unwrap(), cap);
    let mut session = SessionStore::create(&parent, StorageLimits::default()).unwrap();
    let sealed = match session.store_mut().admit_history(&history) {
        Ok(sealed) => sealed,
        Err(error) => {
            println!("R6_HISTORY_CAP_REFUSAL bytes={cap} error={error}");
            // The default 64 MiB decode cap charges 2x encoded parser scratch plus
            // decoded values, so an exact 32 MiB serialized record is not admissible.
            let admitted_cap = 15_470_000;
            let mut doc = (*history.document).clone();
            doc.markers[0].label.clear();
            history.document = Arc::new(doc);
            history.baseline = Arc::clone(&history.document);
            let padding =
                usize::try_from((admitted_cap - history.encoded_bytes().unwrap()) / 2).unwrap();
            let mut doc = (*history.document).clone();
            doc.markers[0].label = "\u{1}".repeat(padding / 6) + &"m".repeat(padding % 6);
            history.document = Arc::new(doc);
            history.baseline = Arc::clone(&history.document);
            assert_eq!(history.encoded_bytes().unwrap(), admitted_cap);
            session.store_mut().admit_history(&history).unwrap()
        }
    };
    println!(
        "R6_HISTORY_READ bytes={} sha256={} shape=escaped-marker-empty-cursor",
        sealed.reference.byte_length, sealed.reference.sha256
    );
    assert_eq!(
        sealed.reference.byte_length,
        history.encoded_bytes().unwrap()
    );
    for run in 1..=5 {
        let started = Instant::now();
        let decoded = session
            .store()
            .read_record::<HistorySnapshot>(&sealed.reference)
            .unwrap();
        let read = started.elapsed();
        assert_eq!(*decoded, history);
        let drop_started = Instant::now();
        drop(decoded);
        println!(
            "R6_HISTORY_READ run={run} read_ms={:.3} drop_ms={:.3}",
            read.as_secs_f64() * 1000.0,
            drop_started.elapsed().as_secs_f64() * 1000.0
        );
    }
}
#[cfg(not(all(target_os = "linux", feature = "test-util")))]
fn main() {
    eprintln!("NOT RUN: requires Linux and test-util");
}
