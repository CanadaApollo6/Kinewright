//! Bounded native storage workload. Run release timing and heap profiling separately.
#![allow(clippy::too_many_lines)]
use kinewright_core::{
    Command, Core, Document, Event, JournalCommand, Operation, Query, QueryResult, Track, TrackId,
    TrackKind,
};
use kinewright_project::{
    bundle::{BundleWriteAdmission, LoadedBundle, ProjectBundle},
    bundle_copy::{SaveAsOutcome, SessionStore},
    bundle_records::{ObjectRef, RecordKind},
    io_observer,
    owned_inputs::{OwnedInputEntry, OwnedInputManifest, OwnedInputRef},
    receipts::{CommandEnvelope, CommandId, HistoryRequest, ReceiptLookup},
    storage_budget::{StorageLimits, StorageUsage},
};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Cursor, Read},
    path::Path,
    time::Instant,
};
#[cfg(feature = "storage-heap-profile")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;
fn apply(core: &Core, command: Command) {
    assert!(matches!(
        core.request(command).unwrap(),
        Event::DocumentChanged { .. }
    ));
}
fn reference(bytes: &[u8], kind: RecordKind) -> ObjectRef {
    ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: kind,
    }
}
fn usage(s: &StorageUsage) -> Value {
    json!({"physical_bytes":s.physical_bytes,"owned_bytes":s.owned_bytes,"metadata_reservation_bytes":s.metadata_bytes,"objects":s.objects,"edges":s.edges,"verified_calls":s.verified_calls,"verified_bytes":s.verified_bytes,"peak_physical_bytes":s.peak_physical_bytes,"peak_reserved_bytes":s.peak_reserved_bytes,"peak_decoded_reservation_bytes":s.peak_decoded_bytes,"live_history_reservation_bytes":s.live_history_bytes,"unclassified_bytes":s.unclassified_bytes})
}
fn rss() -> Value {
    let text = fs::read_to_string("/proc/self/status").unwrap();
    let number = |name: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(name))
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            * 1024
    };
    json!({"rss_bytes":number("VmRSS:"),"hwm_bytes":number("VmHWM:")})
}
fn heap() -> Value {
    #[cfg(feature = "storage-heap-profile")]
    {
        let s = dhat::HeapStats::get();
        json!({"current_bytes":s.curr_bytes,"peak_bytes":s.max_bytes,"total_allocated_bytes":s.total_bytes,"allocations":s.total_blocks})
    }
    #[cfg(not(feature = "storage-heap-profile"))]
    {
        Value::Null
    }
}
fn physical(path: &Path) -> Value {
    fn walk(path: &Path) -> (u64, u64, u64) {
        let metadata = fs::metadata(path).unwrap();
        if metadata.is_dir() {
            return fs::read_dir(path)
                .unwrap()
                .map(|e| walk(&e.unwrap().path()))
                .fold((0, 0, 0), |a, b| (a.0 + b.0, a.1 + b.1, a.2 + b.2));
        }
        #[cfg(unix)]
        let allocated = {
            use std::os::unix::fs::MetadataExt;
            metadata.blocks() * 512
        };
        #[cfg(not(unix))]
        let allocated = 0;
        (1, metadata.len(), allocated)
    }
    let root = walk(path);
    let assets = walk(&path.with_extension("kinewright-assets"));
    json!({"files":root.0+assets.0,"payload_bytes":root.1+assets.1,"allocated_file_bytes":root.2+assets.2,"lock_payload_bytes":fs::metadata(path.with_extension("kinewright.lock")).unwrap().len()})
}
fn timed<T>(times: &mut Vec<f64>, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let result = f();
    times.push(start.elapsed().as_secs_f64() * 1000.);
    result
}
fn latencies(times: &[f64]) -> Value {
    let mut sorted = times.to_vec();
    sorted.sort_by(f64::total_cmp);
    let p = |n: usize| sorted[(sorted.len() * n).div_ceil(100).saturating_sub(1)];
    json!({"count":sorted.len(),"p50_ms":p(50),"p95_ms":p(95),"min_ms":sorted[0],"max_ms":sorted[sorted.len()-1]})
}
fn request(
    bundle: &mut ProjectBundle,
    core: &Core,
    issuer: &kinewright_project::receipts::IssuerId,
    sequence: u64,
    command: JournalCommand,
) -> CommandEnvelope {
    let Event::QueryResult(QueryResult::Snapshot { revision, .. }) =
        core.request(Command::Query(Query::Snapshot)).unwrap()
    else {
        panic!("snapshot")
    };
    CommandEnvelope {
        version: 1,
        action: "edit".into(),
        id: CommandId {
            issuer: issuer.clone(),
            sequence,
        },
        project_id: bundle.project_id().clone(),
        target_branch: bundle.manual_branch_id().clone(),
        owner_epoch: bundle.store().owner_epoch(),
        lease: None,
        expected_head: bundle.manual_head().cloned(),
        expected_revision: revision,
        command: Some(command),
        comment_text: None,
        comment_strokes: None,
        evidence: vec![],
        transition: None,
    }
}
fn main() {
    #[cfg(feature = "storage-heap-profile")]
    let _profiler = dhat::Profiler::builder()
        .file_name(std::env::var_os("JA_S1_HEAP_FILE").expect("JA_S1_HEAP_FILE"))
        .trim_backtraces(Some(4))
        .build();
    let ((), sync) = io_observer::profile_sync(run);
    eprintln!("sync_profile: {}", serde_json::to_string(&sync).unwrap());
}
fn run() {
    let args: Vec<_> = std::env::args_os().collect();
    assert_eq!(args.len(), 2, "one new workload directory required");
    let root = Path::new(&args[1]);
    fs::create_dir(root).unwrap();
    let baseline_rss = rss();
    let baseline_heap = heap();
    let started = Instant::now();
    let mut document = Document::default();
    for id in 1..=32 {
        document.tracks.push(Track {
            id: TrackId(id),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        });
    }
    let core = Core::spawn(document).unwrap();
    for i in 0..96 {
        apply(
            &core,
            Command::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: i % 2 == 0,
            }),
        );
    }
    for _ in 0..24 {
        apply(&core, Command::Undo);
    }
    let history = core.export_history().unwrap();
    let encoded = history.encoded_bytes().unwrap();
    let history_heap = heap();
    let mut restore_times = vec![];
    for _ in 0..5 {
        let other = Core::spawn(Document::default()).unwrap();
        timed(&mut restore_times, || {
            other.restore_history(history.clone()).unwrap();
        });
        assert_eq!(other.export_history().unwrap(), history);
    }
    let restore_heap = heap();
    let path = root.join("source.kinewright");
    let mut bundle = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    let bytes = vec![113_u8; 8 << 20];
    let object = reference(&bytes, RecordKind::OwnedEvidence);
    let before_duplicate = bundle.store().usage().unwrap();
    let sealed = bundle
        .store_mut()
        .unwrap()
        .admit_blob(&mut Cursor::new(&bytes), &object, &[])
        .unwrap();
    let first = bundle.store().usage().unwrap();
    let duplicate = bundle
        .store_mut()
        .unwrap()
        .admit_blob(&mut Cursor::new(&bytes), &object, &[])
        .unwrap();
    let second = bundle.store().usage().unwrap();
    assert_eq!(first.physical_bytes, second.physical_bytes);
    assert_eq!(first.objects, second.objects);
    bundle.retain_evidence(&object).unwrap();
    let inputs = bundle.empty_inputs().unwrap();
    let input_ref = inputs.reference.clone();
    bundle.save(&core, &input_ref).unwrap();
    let decode_before_heap = heap();
    let decode_before_history = bundle.store().usage().unwrap().live_history_bytes;
    let decoded_history = bundle
        .store()
        .read_record::<kinewright_core::HistorySnapshot>(
            &bundle
                .checkpoint(bundle.manual_branch_id())
                .unwrap()
                .history_ref,
        )
        .unwrap();
    let history_decoded_reservation_bytes =
        bundle.store().usage().unwrap().live_history_bytes - decode_before_history;
    let decode_after_heap = heap();
    #[cfg(feature = "storage-heap-profile")]
    assert!(
        decode_after_heap["current_bytes"].as_u64().unwrap()
            - decode_before_heap["current_bytes"].as_u64().unwrap()
            <= history_decoded_reservation_bytes,
        "measured live decode exceeds its format-defined reservation"
    );
    drop(decoded_history);
    drop(inputs);
    drop((sealed, duplicate));
    let mut saves = vec![];
    let before_metadata = bundle.store().usage().unwrap();
    for _ in 0..8 {
        timed(&mut saves, || bundle.save(&core, &input_ref).unwrap());
    }
    let after_metadata = bundle.store().usage().unwrap();
    assert!(after_metadata.verified_bytes - before_metadata.verified_bytes < object.byte_length);
    let metadata_heap = heap();
    let issuer = bundle.issue_issuer(&core, &input_ref, 1).unwrap();
    let mut comment_times = vec![];
    let mut comment_syncs = vec![];
    for sequence in 1..=8 {
        let mut envelope = request(&mut bundle, &core, &issuer, sequence, JournalCommand::Undo);
        envelope.action = "comment".into();
        envelope.command = None;
        let text = b"exact bounded storage measurement comment\r\n";
        let strokes = b"[[1,2],[3,5]]";
        let a = reference(text, RecordKind::OwnedEvidence);
        let b = reference(strokes, RecordKind::OwnedEvidence);
        envelope.comment_text = Some(a.clone());
        envelope.comment_strokes = Some(b.clone());
        let sync_before = io_observer::sync_profile();
        assert!(matches!(
            timed(&mut comment_times, || bundle
                .command(&envelope, &core, &input_ref, &[(&a, text), (&b, strokes)])
                .unwrap()),
            ReceiptLookup::Committed(_)
        ));
        let sync_after = io_observer::sync_profile();
        comment_syncs.push(json!({"calls":sync_after.calls-sync_before.calls,"nanoseconds":sync_after.nanoseconds-sync_before.nanoseconds}));
    }
    // Floors use a separate small branch history so every ordered cut is fully exercised.
    let floor_path = root.join("floor.kinewright");
    let mut floor_doc = Document::default();
    floor_doc.tracks.push(Track {
        id: TrackId(1),
        kind: TrackKind::Video,
        sync_lock: true,
        clips: vec![],
    });
    let floor_core = Core::spawn(floor_doc).unwrap();
    let mut floors =
        ProjectBundle::activate(&floor_path, BundleWriteAdmission::new_document()).unwrap();
    let inputs = floors.empty_inputs().unwrap();
    let floor_inputs = inputs.reference.clone();
    floors.save(&floor_core, &floor_inputs).unwrap();
    drop(inputs);
    let floor_issuer = floors.issue_issuer(&floor_core, &floor_inputs, 1).unwrap();
    let mut floor_times = vec![];
    for i in 0..6 {
        let edit = request(
            &mut floors,
            &floor_core,
            &floor_issuer,
            i * 2 + 1,
            JournalCommand::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: i % 2 == 0,
            }),
        );
        assert!(matches!(
            floors
                .command(&edit, &floor_core, &floor_inputs, &[])
                .unwrap(),
            ReceiptLookup::Committed(_)
        ));
        let mut envelope = request(
            &mut floors,
            &floor_core,
            &floor_issuer,
            i * 2 + 2,
            JournalCommand::Undo,
        );
        let checkpoint = floors
            .checkpoint(floors.manual_branch_id())
            .unwrap()
            .clone();
        envelope.action = "floor".into();
        envelope.command = None;
        envelope.transition = Some(HistoryRequest::Floor {
            discard_batches: floor_core.export_history().unwrap().batches.len(),
            preceding_checkpoint_generation: checkpoint.checkpoint_generation,
            preceding_floor_generation: checkpoint.floor_generation,
            command_cut: checkpoint.command_cut.unwrap(),
        });
        assert!(matches!(
            timed(&mut floor_times, || floors
                .command(&envelope, &floor_core, &floor_inputs, &[])
                .unwrap()),
            ReceiptLookup::Committed(_)
        ));
    }
    for _ in 0..3 {
        floors.save(&floor_core, &floor_inputs).unwrap();
    }
    assert_eq!(
        floors
            .evidence_refs()
            .iter()
            .filter(|r| r.record_kind == RecordKind::HistoryFloor)
            .count(),
        1
    );
    let floor_usage = usage(&floors.store().usage().unwrap());
    drop(floors);
    let source_root = fs::read(&path).unwrap();
    let mut failed = 0;
    for n in 0..3 {
        let copy = root.join(format!("failed-{n}.kinewright"));
        assert!(io_observer::fail_once("copy_verify", || bundle.save_as(&copy)).is_err());
        assert!(!copy.exists());
        assert!(!copy.with_extension("kinewright-assets").exists());
        assert_eq!(fs::read(&path).unwrap(), source_root);
        failed += 1;
    }
    assert_eq!(bundle.store().usage().unwrap().reserved_copy_bytes, 0);
    let copy_path = root.join("copy.kinewright");
    let copy = match bundle.save_as(&copy_path).unwrap() {
        SaveAsOutcome::Copied(copy) => *copy,
        SaveAsOutcome::SameProject(_) => panic!("distinct copy"),
    };
    assert!(bundle.store().usage().unwrap().reserved_copy_bytes >= object.byte_length);
    let copied = copy
        .into_destination()
        .unwrap_or_else(|_| panic!("copy pending"));
    let copied_usage = usage(&copied.store().usage().unwrap());
    assert_eq!(bundle.store().usage().unwrap().reserved_copy_bytes, 0);
    drop(copied);
    let source_usage = usage(&bundle.store().usage().unwrap());
    let source_physical = physical(&path);
    assert_eq!(
        source_usage["physical_bytes"],
        source_physical["payload_bytes"]
    );
    drop(bundle);
    let mut recover_times = vec![];
    let mut recovery_usage = Value::Null;
    for _ in 0..5 {
        let loaded = timed(&mut recover_times, || LoadedBundle::read(&path).unwrap());
        let restored = Core::spawn(Document::default()).unwrap();
        loaded.restore_manual(&restored).unwrap();
        assert_eq!(restored.export_history().unwrap(), history);
        let writer = loaded.into_writer().unwrap();
        assert!(writer.store().usage().unwrap().verified_bytes >= object.byte_length);
        recovery_usage = usage(&writer.store().usage().unwrap());
    }
    let mut session = SessionStore::create(root, StorageLimits::default()).unwrap();
    let session_bytes = vec![89_u8; 2 << 20];
    let r = reference(&session_bytes, RecordKind::OwnedData);
    let sealed = session
        .store_mut()
        .admit_blob(&mut Cursor::new(&session_bytes), &r, &[])
        .unwrap();
    let entry = OwnedInputEntry {
        entry_id: "data".into(),
        object: r,
        dependencies: vec![],
        selected_stream: None,
        time_base: None,
        origin: None,
        profile: None,
    };
    let manifest = OwnedInputManifest {
        project_id: None,
        session_id: Some(session.session_id().clone()),
        owner_epoch: session.store().owner_epoch(),
        entries: vec![entry.clone()],
    };
    let inputs = session.admit_inputs(&manifest).unwrap();
    let original = OwnedInputRef {
        manifest_ref: inputs.reference.clone(),
        entry,
    };
    let mut reader = session.store().lease_input(&original).unwrap();
    let monitor = session.store().usage_handle();
    let empty = Core::spawn(Document::default()).unwrap();
    let copy = session
        .first_save(
            &root.join("first.kinewright"),
            &empty,
            &original.manifest_ref,
        )
        .unwrap();
    let first_overlap = usage(&monitor().unwrap());
    let destination = copy
        .into_destination()
        .unwrap_or_else(|_| panic!("first save pending"));
    let destination_first = usage(&destination.store().usage().unwrap());
    drop(inputs);
    drop(sealed);
    drop(session);
    assert!(monitor().unwrap().physical_bytes > 0);
    let mut read = vec![];
    reader.read_to_end(&mut read).unwrap();
    assert_eq!(read, session_bytes);
    drop(reader);
    assert_eq!(monitor().unwrap().physical_bytes, 0);
    assert!(!monitor().unwrap().damaged_accounting);
    drop(destination);
    println!(
        "{}",
        json!({"schema":1,"profiled":cfg!(feature="storage-heap-profile"),"errors":0,"expected_failed_copies":failed,"baseline_rss":baseline_rss,"final_rss":rss(),"baseline_heap":baseline_heap,"history_heap":history_heap,"restore_heap":restore_heap,"metadata_heap":metadata_heap,"final_heap":heap(),"elapsed_ms":started.elapsed().as_secs_f64()*1000.,"history":{"encoded_bytes":encoded,"decoded_reservation_bytes":history_decoded_reservation_bytes,"decode_before_heap":decode_before_heap,"decode_after_heap":decode_after_heap,"tracks":32,"accepted_commands":120,"undo_entries":history.cursor.undo.len(),"redo_entries":history.cursor.redo.len(),"restore":latencies(&restore_times)},"metadata":{"saves":latencies(&saves),"comments":latencies(&comment_times),"comment_syncs":comment_syncs,"verified_bytes_delta":after_metadata.verified_bytes-before_metadata.verified_bytes},"duplicate":{"before":usage(&before_duplicate),"first":usage(&first),"second":usage(&second)},"floor":{"latencies":latencies(&floor_times),"usage":floor_usage},"source":source_usage,"source_physical":source_physical,"copied":copied_usage,"recovery":{"latencies":latencies(&recover_times),"usage":recovery_usage},"first_save":{"source_overlap":first_overlap,"destination":destination_first,"source_after_last_reader":usage(&monitor().unwrap())}})
    );
}
