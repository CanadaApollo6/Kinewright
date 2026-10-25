//! Paired incremental-save measurement; no timing threshold is a correctness assertion.
#![cfg(all(feature = "test-util", target_os = "linux"))]
use kinewright_core::{Command, Core, Document, Event, Operation, Track, TrackId, TrackKind};
use kinewright_media::test_support::TempDirectory;
use kinewright_project::{
    bundle::{BundleWriteAdmission, LoadedBundle, ProjectBundle},
    bundle_records::{ObjectRef, RecordKind},
    io_observer,
    receipts::{CommandEnvelope, CommandId, ReceiptLookup},
};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn edit(core: &Core, locked: bool) {
    assert!(matches!(
        core.request(Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked
        }))
        .unwrap(),
        Event::DocumentChanged { .. }
    ));
}

struct SaveCostFixture {
    _temp: TempDirectory,
    path: PathBuf,
    core: Core,
    owner: ProjectBundle,
    inputs: ObjectRef,
    histories: Vec<ObjectRef>,
}
fn fixture() -> SaveCostFixture {
    let temp = TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "s2-save-cost");
    let path = temp.path("project.kinewright");
    let document = Document {
        duration: kinewright_core::TimeCode(900),
        tracks: (1..=12)
            .map(|track| Track {
                id: TrackId(track),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: (0..10)
                    .map(|clip| {
                        serde_json::from_value(serde_json::json!({
                            "id": track * 100 + clip + 1, "asset": 0,
                            "source_range": {"start": 0, "end": 90},
                            "content": {"solid": {"r": 20, "g": 40, "b": 60}},
                            "timeline_start": clip * 90, "effects": [], "transition_in": null
                        }))
                        .unwrap()
                    })
                    .collect(),
            })
            .collect(),
        ..Document::default()
    };
    let core = Core::spawn(document).unwrap();
    for n in 0..24 {
        edit(&core, n % 2 == 0);
    }
    let mut owner = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    let inputs = owner.empty_inputs().unwrap().reference.clone();
    owner.save(&core, &inputs).unwrap();
    let manual = owner.manual_branch_id().clone();
    let mut histories = vec![owner.checkpoint(&manual).unwrap().history_ref.clone()];
    for branch in 0..5 {
        let id = owner.fork_branch(&manual).unwrap();
        let other = Core::spawn((*owner.branch_history(&id).unwrap().document).clone()).unwrap();
        for n in 0..24 + branch {
            edit(&other, n % 2 == 0);
        }
        owner.save_branch(&id, &other).unwrap();
        histories.push(owner.checkpoint(&id).unwrap().history_ref.clone());
    }
    SaveCostFixture {
        _temp: temp,
        path,
        core,
        owner,
        inputs,
        histories,
    }
}
fn seconds(nanoseconds: u128) -> f64 {
    Duration::from_nanos(u64::try_from(nanoseconds).expect("bounded benchmark duration"))
        .as_secs_f64()
}
#[test]
fn repeated_small_saves_with_six_populated_branches() {
    let SaveCostFixture {
        _temp,
        path,
        core,
        mut owner,
        inputs,
        histories,
    } = fixture();
    let manual = owner.manual_branch_id().clone();
    let issuer = owner.issue_issuer(&core, &inputs, 1).unwrap();
    let text = b"small comment";
    let strokes = b"[]";
    let reference = |bytes: &[u8]| ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::OwnedEvidence,
    };
    let kinewright_core::Event::QueryResult(kinewright_core::QueryResult::Snapshot {
        revision,
        ..
    }) = core
        .request(Command::Query(kinewright_core::Query::Snapshot))
        .unwrap()
    else {
        panic!("snapshot");
    };
    let mut samples = Vec::new();
    let mut verified_bytes = Vec::new();
    let mut decodes = Vec::new();
    let mut decoded_peaks = Vec::new();
    let mut fit_seconds = Vec::new();
    let mut sync_seconds = Vec::new();
    let mut sync_calls = Vec::new();
    for sequence in 1..=12 {
        let envelope = CommandEnvelope {
            switch: None,
            manual: None,
            version: 1,
            action: "comment".into(),
            id: CommandId {
                issuer: issuer.clone(),
                sequence,
            },
            project_id: owner.project_id().clone(),
            target_branch: manual.clone(),
            owner_epoch: owner.store().owner_epoch(),
            lease: None,
            expected_head: owner.manual_head().cloned(),
            expected_revision: revision,
            command: None,
            comment_text: Some(reference(text)),
            comment_strokes: Some(reference(strokes)),
            evidence: vec![],
            transition: None,
        };
        owner.store().reset_decode_peaks_probe().unwrap();
        let usage = owner.store().usage().unwrap();
        let before_decodes = io_observer::history_decode_count();
        let fit_before = io_observer::publication_fit_profile();
        let started = Instant::now();
        let (result, sync) = io_observer::profile_sync(|| {
            owner.command(
                &envelope,
                &core,
                &inputs,
                &[
                    (envelope.comment_text.as_ref().unwrap(), text),
                    (envelope.comment_strokes.as_ref().unwrap(), strokes),
                ],
            )
        });
        assert!(matches!(result.unwrap(), ReceiptLookup::Committed(_)));
        fit_seconds.push(seconds(
            io_observer::publication_fit_profile().nanoseconds - fit_before.nanoseconds,
        ));
        sync_seconds.push(seconds(sync.nanoseconds));
        sync_calls.push(sync.calls);
        samples.push(started.elapsed().as_secs_f64());
        let after = owner.store().usage().unwrap();
        verified_bytes.push(after.verified_bytes - usage.verified_bytes);
        decodes.push(io_observer::history_decode_count() - before_decodes);
        decoded_peaks.push(after.peak_decoded_bytes);
    }
    let loaded = LoadedBundle::read(&path).unwrap();
    assert!(loaded.usage_probe().unwrap().live_history_bytes > 0);
    let report = serde_json::json!({
        "branches": 6, "tracks_per_branch": 12, "clips_per_branch": 120,
        "history_batches": [24,24,25,26,27,28],
        "history_encoded_bytes": histories.iter().map(|r| r.byte_length).collect::<Vec<_>>(),
        "repeated_comment_saves": 12, "seconds": samples, "publication_fit_seconds": fit_seconds, "sync_seconds": sync_seconds, "sync_calls": sync_calls, "history_decodes_per_save": decodes,
        "writer_peak_decoded_bytes_per_save": decoded_peaks, "verified_bytes_per_save": verified_bytes,
        "independent_reader_peak_decoded_bytes": loaded.usage_probe().unwrap().peak_decoded_bytes,
        "note": "decoded bytes are production accounting charges, not allocator RSS; writer high-water reset before each save"
    });
    if let Some(output) = std::env::var_os("JA_SAVE_BENCH_OUTPUT") {
        std::fs::write(output, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    eprintln!("SAVE_COST={report}");
}
