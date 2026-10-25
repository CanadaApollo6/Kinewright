//! Production inventory, retention, full verification and ingress boundaries.
#![allow(
    clippy::field_reassign_with_default,
    reason = "each boundary probe lowers the one named limit"
)]
use kinewright_core::{
    Command, Core, Document, Event, JournalCommand, Operation, Query, QueryResult,
    TimelineRevision, Track, TrackId, TrackKind,
};
use kinewright_media::test_support::TempDirectory;
use kinewright_project::{
    bundle::{BundleError, BundleWriteAdmission, LoadedBundle, ProjectBundle},
    bundle_records::{ObjectRef, ProjectId, RecordKind, SpecialistBaseRecord},
    io_observer,
    namespace::ProjectNamespace,
    receipts::{CommandEnvelope, CommandId, HistoryRequest, ReceiptLookup},
    storage_budget::StorageLimits,
    store::{ProjectStore, RecoveryInventory, StoreManifest},
};
use std::{
    fs,
    io::Cursor,
    path::{Path, PathBuf},
};

fn apply(core: &Core, command: Command) {
    assert!(matches!(
        core.request(command).unwrap(),
        Event::DocumentChanged { .. }
    ));
}
fn setup(path: &Path) -> (Core, ProjectBundle, ObjectRef) {
    let mut document = Document::default();
    document.tracks.push(Track {
        id: TrackId(1),
        kind: TrackKind::Video,
        sync_lock: true,
        clips: vec![],
    });
    let core = Core::spawn(document).unwrap();
    let mut bundle = ProjectBundle::activate(path, BundleWriteAdmission::new_document()).unwrap();
    let sealed = bundle.empty_inputs().unwrap();
    let inputs = sealed.reference.clone();
    bundle.save(&core, &inputs).unwrap();
    drop(sealed);
    (core, bundle, inputs)
}
fn objects(path: &Path) -> PathBuf {
    path.with_extension("kinewright-assets")
        .join("studio/objects")
}
fn object_path(path: &Path, r: &ObjectRef) -> PathBuf {
    objects(path).join(&r.sha256)
}
fn manifest(path: &Path, bundle: &ProjectBundle) -> StoreManifest {
    serde_json::from_slice(
        &fs::read(object_path(
            path,
            &bundle.root().unwrap().store_manifest_ref,
        ))
        .unwrap(),
    )
    .unwrap()
}
fn inventory(path: &Path) -> RecoveryInventory {
    serde_json::from_slice(
        &fs::read(
            path.with_extension("kinewright-assets")
                .join("studio/recovery/inventory"),
        )
        .unwrap(),
    )
    .unwrap()
}
fn reopen(path: &Path) -> (Core, ProjectBundle, ObjectRef) {
    let loaded = LoadedBundle::read(path).unwrap();
    let core = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&core).unwrap();
    let inputs = loaded.root().unwrap().input_manifest_ref.clone();
    let bundle = loaded.into_writer().unwrap();
    (core, bundle, inputs)
}
fn revision(core: &Core) -> TimelineRevision {
    let Event::QueryResult(QueryResult::Snapshot { revision, .. }) =
        core.request(Command::Query(Query::Snapshot)).unwrap()
    else {
        panic!("snapshot")
    };
    revision
}
fn request(
    bundle: &mut ProjectBundle,
    core: &Core,
    inputs: &ObjectRef,
    command: JournalCommand,
) -> CommandEnvelope {
    let issuer = bundle.issue_issuer(core, inputs, 1).unwrap();
    CommandEnvelope {
        manual: None,
        version: 1,
        action: "edit".into(),
        id: CommandId {
            issuer,
            sequence: 1,
        },
        project_id: bundle.project_id().clone(),
        target_branch: bundle.manual_branch_id().clone(),
        owner_epoch: bundle.store().owner_epoch(),
        lease: None,
        expected_head: bundle.manual_head().cloned(),
        expected_revision: revision(core),
        command: Some(command),
        comment_text: None,
        comment_strokes: None,
        evidence: vec![],
        transition: None,
    }
}
fn committed(result: ReceiptLookup) {
    match result {
        ReceiptLookup::Committed(receipt) | ReceiptLookup::Retired(receipt) => drop(receipt),
        other => panic!("expected committed receipt, got {other:?}"),
    }
}
fn lock(value: bool) -> Operation {
    Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked: value,
    }
}
fn reference(bytes: &[u8]) -> ObjectRef {
    ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::OwnedData,
    }
}
#[allow(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "the test counts exact owner temporary names"
)]
fn physical_payload(path: &Path) -> u64 {
    fn walk(path: &Path) -> u64 {
        if path.is_file() {
            return fs::metadata(path).unwrap().len();
        }
        if !path.exists() {
            return 0;
        }
        fs::read_dir(path)
            .unwrap()
            .map(|e| walk(&e.unwrap().path()))
            .sum()
    }
    let prefix = format!(
        ".studio-{}-",
        kinewright_media::sha256_bytes(path.file_name().unwrap().to_str().unwrap().as_bytes())
    );
    let temporary: u64 = fs::read_dir(path.parent().unwrap())
        .unwrap()
        .filter_map(|e| {
            let e = e.unwrap();
            e.file_name()
                .to_str()
                .filter(|n| n.starts_with(&prefix) && n.ends_with(".tmp"))
                .map(|_| e.metadata().unwrap().len())
        })
        .sum();
    fs::metadata(path).unwrap().len() + walk(&path.with_extension("kinewright-assets")) + temporary
}

#[test]
fn recovery_inventory_retains_two_predecessors_and_one_staging_slot() {
    let temp = TempDirectory::new_in(
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
        "ja-s1-retention",
    );
    let path = temp.path("edit.kinewright");
    let (core, mut bundle, inputs) = setup(&path);
    let mut previous = vec![];
    for i in 0..12 {
        previous.push(fs::read(&path).unwrap());
        apply(&core, Command::Do(lock(i % 2 == 0)));
        bundle.save(&core, &inputs).unwrap();
        let m = manifest(&path, &bundle);
        assert_eq!(m.recovery.predecessors.len(), previous.len().min(2));
        assert!(m.recovery.staging.is_none());
        for (r, bytes) in m.recovery.predecessors.iter().zip(previous.iter().rev()) {
            assert_eq!(fs::read(object_path(&path, r)).unwrap(), *bytes);
        }
        assert!(
            m.objects
                .iter()
                .filter(|r| r.record_kind == RecordKind::RootCheckpoint)
                .count()
                <= 2,
            "automatic inventories recursively retain old roots"
        );
    }
    let before = fs::read(&path).unwrap();
    apply(&core, Command::Do(lock(false)));
    assert!(io_observer::fail_once("root_before_publish", || bundle.save(&core, &inputs)).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    let staged = inventory(&path);
    assert_eq!(staged.predecessors.len(), 2);
    let staging = staged.staging.unwrap();
    assert_eq!(fs::read(object_path(&path, &staging)).unwrap(), before);
    drop(bundle);
    let (restored, mut bundle, inputs) = reopen(&path);
    assert_eq!(
        bundle.store().usage().unwrap().physical_bytes,
        physical_payload(&path)
    );
    bundle.save(&restored, &inputs).unwrap();
    // Retirement inventory/GC now finish after acknowledgment rather than delaying it.
    bundle.collect_after_ack().unwrap();
    assert!(inventory(&path).staging.is_none());
    assert!(!bundle.store().usage().unwrap().damaged_accounting);
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "inventory cuts and a native process-death cut share the same exact recovery property"
)]
fn inventory_retirement_crash_preserves_required_predecessors() {
    if let Some(path) = std::env::var_os("JA_S1_RETENTION_CHILD") {
        let path = PathBuf::from(path);
        let (core, mut bundle, inputs) = reopen(&path);
        apply(&core, Command::Do(lock(false)));
        bundle.save(&core, &inputs).unwrap();
        panic!("crash cut not reached");
    }
    super::durable_bundle::model_inventory_retirement_cuts();
    for fault in [
        "inventory_file_sync",
        "inventory_before_rename",
        "inventory_after_rename",
        "gc_before_unlink",
        "gc_after_unlink",
    ] {
        let temp = TempDirectory::new_in(
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
            "ja-s1-inventory-crash",
        );
        let path = temp.path("edit.kinewright");
        let (core, mut bundle, inputs) = setup(&path);
        for i in 0..4 {
            apply(&core, Command::Do(lock(i % 2 == 0)));
            bundle.save(&core, &inputs).unwrap();
        }
        let acknowledged = core.export_history().unwrap();
        let root = fs::read(&path).unwrap();
        let predecessors = manifest(&path, &bundle).recovery.predecessors;
        apply(&core, Command::Do(lock(false)));
        let (result, trace) =
            io_observer::observe(|| io_observer::fail_once(fault, || bundle.save(&core, &inputs)));
        assert!(
            trace
                .events
                .iter()
                .any(|e| e.operation == "injected_failure"),
            "fault was not exercised: {fault}"
        );
        if matches!(fault, "inventory_before_rename" | "inventory_after_rename") {
            assert!(
                result.is_err(),
                "uncertain inventory publication admitted a new save: {fault}"
            );
        } else {
            assert!(
                result.is_ok(),
                "definite maintenance failure blocked a save: {fault}"
            );
            assert!(bundle.collection_error().is_some());
        }
        drop(bundle);
        let loaded = LoadedBundle::read(&path).unwrap();
        let restored = Core::spawn(Document::default()).unwrap();
        loaded.restore_manual(&restored).unwrap();
        if fs::read(&path).unwrap() == root {
            assert_eq!(restored.export_history().unwrap(), acknowledged);
            for r in predecessors {
                assert!(object_path(&path, &r).exists());
            }
        } else {
            assert_eq!(
                restored.export_history().unwrap(),
                core.export_history().unwrap()
            );
        }
        let mut bundle = loaded.into_writer().unwrap();
        let usage = bundle.store().usage().unwrap();
        assert_eq!(usage.physical_bytes, physical_payload(&path));
        assert!(!usage.damaged_accounting);
        let inputs = bundle.root().unwrap().input_manifest_ref.clone();
        bundle.save(&restored, &inputs).unwrap();
        assert_eq!(
            bundle.store().usage().unwrap().physical_bytes,
            physical_payload(&path)
        );
    }
    let temp = TempDirectory::new_in(
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
        "ja-s1-physical-stage-crash",
    );
    let path = temp.path("edit.kinewright");
    let signals = temp.path("signals");
    fs::create_dir(&signals).unwrap();
    let (core, bundle, _) = setup(&path);
    let acknowledged = core.export_history().unwrap();
    drop(bundle);
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "retention::inventory_retirement_crash_preserves_required_predecessors",
        ])
        .env("JA_S1_RETENTION_CHILD", &path)
        .env("REV2_HOOK", "root_file_sync")
        .env("REV2_SIGNALS", &signals)
        .env("REV2_EXIT", "1")
        .output()
        .unwrap();
    assert_eq!(child.status.code(), Some(77));
    assert_eq!(
        fs::read_to_string(signals.join("paused")).unwrap(),
        "root_file_sync"
    );
    let root_temporaries: Vec<_> = fs::read_dir(temp.root())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with(".studio-")
                && p.extension().is_some_and(|e| e == "tmp")
        })
        .collect();
    assert!(
        !root_temporaries.is_empty(),
        "native root staging control was absent"
    );
    let (restored, mut bundle, inputs) = reopen(&path);
    assert_eq!(restored.export_history().unwrap(), acknowledged);
    let before = bundle.store().usage().unwrap();
    assert!(before.unclassified_bytes > 0);
    assert_eq!(before.physical_bytes, physical_payload(&path));
    bundle.save(&restored, &inputs).unwrap();
    assert!(
        root_temporaries.iter().all(|p| !p.exists()),
        "the owner-specific root temporary did not retire"
    );
    assert_eq!(
        bundle.store().usage().unwrap().physical_bytes,
        physical_payload(&path)
    );
    assert!(
        bundle.store().usage().unwrap().unclassified_bytes < before.unclassified_bytes,
        "owner temporary did not retire"
    );
    let orphan = b"retained unclassified immutable bytes";
    let orphan_ref = reference(orphan);
    drop(bundle);
    fs::write(object_path(&path, &orphan_ref), orphan).unwrap();
    let (restored, mut bundle, inputs) = reopen(&path);
    let before = bundle.store().usage().unwrap();
    assert!(before.unclassified_bytes >= orphan.len() as u64);
    assert_eq!(before.physical_bytes, physical_payload(&path));
    let sealed = bundle
        .store_mut()
        .unwrap()
        .admit_blob(&mut Cursor::new(orphan), &orphan_ref, &[])
        .unwrap();
    assert_eq!(
        bundle.store().usage().unwrap().physical_bytes,
        before.physical_bytes,
        "adoption double charged existing physical bytes"
    );
    assert_eq!(
        bundle.store().usage().unwrap().unclassified_bytes,
        before.unclassified_bytes - orphan.len() as u64
    );
    bundle.store().verify(&orphan_ref).unwrap();
    bundle.save(&restored, &inputs).unwrap();
    drop(sealed);
    bundle.collect_after_ack().unwrap();
    assert!(!object_path(&path, &orphan_ref).exists());
}

#[test]
fn old_floor_collection_keeps_independent_branch_version_and_receipt_pins() {
    let temp = TempDirectory::new_in(
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
        "ja-s1-floor-retention",
    );
    let path = temp.path("edit.kinewright");
    let (core, mut bundle, inputs) = setup(&path);
    let first = request(&mut bundle, &core, &inputs, JournalCommand::Do(lock(false)));
    committed(bundle.command(&first, &core, &inputs, &[]).unwrap());
    let manual = bundle.manual_branch_id().clone();
    let old_checkpoint = bundle.checkpoint_reference(&manual).unwrap();
    let baseline = bundle.checkpoint(&manual).unwrap().clone();
    let branch = bundle.fork_branch(&manual).unwrap();
    let base = SpecialistBaseRecord {
        target_branch: manual.clone(),
        base_head: baseline.head_id,
        base_document_hash: baseline.document_hash,
        original_baseline_ref: baseline.baseline_ref,
        frozen_checkpoint_ref: old_checkpoint.clone(),
        full_applied_delta: vec![],
    };
    let base_ref = bundle.retain_specialist_base(&base).unwrap();
    let expected_branch = (*bundle.branch_history(&branch).unwrap()).clone();
    let mut old_floors = vec![];
    for i in 0..6 {
        let edit = request(
            &mut bundle,
            &core,
            &inputs,
            JournalCommand::Do(lock(i % 2 == 0)),
        );
        committed(bundle.command(&edit, &core, &inputs, &[]).unwrap());
        let mut floor = request(&mut bundle, &core, &inputs, JournalCommand::Undo);
        let prior = bundle.checkpoint(&manual).unwrap().clone();
        floor.action = "floor".into();
        floor.command = None;
        floor.transition = Some(HistoryRequest::Floor {
            discard_batches: core.export_history().unwrap().batches.len(),
            preceding_checkpoint_generation: prior.checkpoint_generation,
            preceding_floor_generation: prior.floor_generation,
            command_cut: prior.command_cut.unwrap(),
        });
        committed(bundle.command(&floor, &core, &inputs, &[]).unwrap());
        old_floors.extend(
            bundle
                .evidence_refs()
                .iter()
                .filter(|r| r.record_kind == RecordKind::HistoryFloor)
                .cloned(),
        );
    }
    for _ in 0..4 {
        bundle.save(&core, &inputs).unwrap();
    }
    assert_eq!(
        bundle
            .evidence_refs()
            .iter()
            .filter(|r| r.record_kind == RecordKind::HistoryFloor)
            .count(),
        1
    );
    assert!(
        old_floors[..old_floors.len() - 1]
            .iter()
            .all(|r| !object_path(&path, r).exists()),
        "all automatic floors were retained"
    );
    assert!(object_path(&path, &base_ref).exists());
    assert!(object_path(&path, &old_checkpoint).exists());
    committed(bundle.receipt_lookup(&first).unwrap());
    assert_eq!(*bundle.branch_history(&branch).unwrap(), expected_branch);
    drop(bundle);
    let (_, bundle, _) = reopen(&path);
    committed(bundle.receipt_lookup(&first).unwrap());
    assert_eq!(*bundle.branch_history(&branch).unwrap(), expected_branch);
    assert!(object_path(&path, &base_ref).exists());
}

#[test]
fn metadata_commit_verifies_new_bytes_but_recovery_verifies_full_graph() {
    let temp = TempDirectory::new_in(
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
        "ja-s1-verification-work",
    );
    let path = temp.path("edit.kinewright");
    let (core, mut bundle, inputs) = setup(&path);
    let bytes = vec![71; 4 << 20];
    let mut object = reference(&bytes);
    object.record_kind = RecordKind::OwnedEvidence;
    let sealed = bundle
        .store_mut()
        .unwrap()
        .admit_blob(&mut Cursor::new(&bytes), &object, &[])
        .unwrap();
    bundle.retain_evidence(&object).unwrap();
    bundle.save(&core, &inputs).unwrap();
    drop(sealed);
    let before = bundle.store().usage().unwrap();
    apply(&core, Command::Do(lock(false)));
    bundle.save(&core, &inputs).unwrap();
    let after = bundle.store().usage().unwrap();
    assert!(after.verified_calls > before.verified_calls);
    assert!(
        after.verified_bytes - before.verified_bytes < object.byte_length,
        "metadata save reread sealed large payload"
    );
    let expected = core.export_history().unwrap();
    drop(bundle);
    let loaded = LoadedBundle::read(&path).unwrap();
    let restored = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&restored).unwrap();
    assert_eq!(restored.export_history().unwrap(), expected);
    let bundle = loaded.into_writer().unwrap();
    assert!(bundle.store().usage().unwrap().verified_bytes >= object.byte_length);
    drop(bundle);
    let mut tampered = bytes;
    tampered[3] ^= 1;
    fs::write(object_path(&path, &object), tampered).unwrap();
    assert!(
        matches!(LoadedBundle::read(&path), Err(BundleError::Corrupt(_))),
        "recovery skipped full byte verification"
    );
}

fn store(temp: &TempDirectory, name: &str, limits: StorageLimits) -> ProjectStore {
    ProjectStore::new(
        ProjectNamespace::acquire(&temp.path(name)).unwrap(),
        Some(ProjectId::fresh().unwrap()),
        None,
        limits,
    )
    .unwrap()
}
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one boundary/one-beyond matrix at production storage ingress"
)]
fn every_storage_cap_backpressures_without_data_loss() {
    let temp = TempDirectory::new_in(
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
        "ja-s1-all-caps",
    );
    combined_metadata_cap(&temp);
    // Boundary and one beyond at actual byte, count, graph and transient ingress.
    let mut limits = StorageLimits::default();
    limits.owned_bytes = 4;
    limits.objects = 2;
    let mut s = store(&temp, "owned.kinewright", limits);
    let bytes = b"four";
    let r = reference(bytes);
    let pin = s.admit_blob(&mut Cursor::new(bytes), &r, &[]).unwrap();
    assert_eq!(s.usage().unwrap().owned_bytes, 4);
    assert_eq!(
        s.collect_unpublished().unwrap(),
        0,
        "a live pin was evicted"
    );
    assert!(matches!(
        s.admit_blob(&mut Cursor::new(b"x"), &reference(b"x"), &[]),
        Err(BundleError::Capacity(_))
    ));
    assert_eq!(s.usage().unwrap().owned_bytes, 4);
    s.verify(&r).unwrap();
    drop(pin);
    assert_eq!(s.collect_unpublished().unwrap(), 4);
    assert_eq!(s.usage().unwrap().physical_bytes, 0);
    assert_eq!(s.collect_unpublished().unwrap(), 0);
    let mut limits = StorageLimits::default();
    limits.objects = 1;
    let mut s = store(&temp, "objects.kinewright", limits);
    let pin = s.admit_blob(&mut Cursor::new(bytes), &r, &[]).unwrap();
    assert!(
        s.admit_blob(&mut Cursor::new(b"x"), &reference(b"x"), &[])
            .is_err()
    );
    assert_eq!(s.usage().unwrap().objects, 1);
    drop(pin);
    let mut limits = StorageLimits::default();
    limits.graph_depth = 1;
    let mut s = store(&temp, "depth.kinewright", limits);
    let parent_pin = s.admit_blob(&mut Cursor::new(bytes), &r, &[]).unwrap();
    let child_ref = reference(b"b");
    let b_pin = s
        .admit_blob(&mut Cursor::new(b"b"), &child_ref, std::slice::from_ref(&r))
        .unwrap();
    let grandchild_ref = reference(b"c");
    assert!(
        s.admit_blob(
            &mut Cursor::new(b"c"),
            &grandchild_ref,
            std::slice::from_ref(&child_ref)
        )
        .is_err()
    );
    assert_eq!(s.usage().unwrap().objects, 2);
    drop((parent_pin, b_pin));
    for field in ["decoded", "live_history", "metadata"] {
        let mut limits = StorageLimits::default();
        match field {
            "decoded" => limits.decoded_record_bytes = 4,
            "live_history" => limits.live_history_bytes = 4,
            _ => limits.metadata_bytes = 4,
        }
        let s = store(&temp, &format!("{field}.kinewright"), limits);
        let reservation = if field == "live_history" {
            s.reserve_probe(0, 4)
        } else {
            s.reserve_probe(4, 0)
        }
        .unwrap();
        assert!(
            if field == "live_history" {
                s.reserve_probe(0, 1)
            } else {
                s.reserve_probe(if field == "decoded" { 5 } else { 1 }, 0)
            }
            .is_err()
        );
        drop(reservation);
        assert_eq!(s.usage().unwrap().decoded_bytes, 0);
        assert_eq!(s.usage().unwrap().live_history_bytes, 0);
    }
    let mut limits = StorageLimits::default();
    limits.copy_overlap_bytes = 4;
    limits.metadata_bytes = 0;
    let s = store(&temp, "overlap.kinewright", limits);
    let hold = s.reserve_copy(4).unwrap();
    assert!(s.reserve_copy(1).is_err());
    drop(hold);
    assert_eq!(s.usage().unwrap().reserved_copy_bytes, 0);
    let core = Core::spawn(Document::default()).unwrap();
    let history = core.export_history().unwrap();
    let encoded = history.encoded_bytes().unwrap();
    let mut control = store(
        &temp,
        "history-control.kinewright",
        StorageLimits::default(),
    );
    let control_pin = control.admit_history(&history).unwrap();
    let decoded = control
        .read_record::<kinewright_core::HistorySnapshot>(&control_pin.reference)
        .unwrap();
    let live = control.usage().unwrap().live_history_bytes;
    drop(decoded);
    let mut limits = StorageLimits::default();
    limits.branch_history_bytes = encoded.max(live);
    let mut s = store(&temp, "history.kinewright", limits);
    let pin = s.admit_history(&history).unwrap();
    apply(
        &core,
        Command::Do(Operation::AddTrack {
            track: Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            },
        }),
    );
    assert!(s.admit_history(&core.export_history().unwrap()).is_err());
    drop(pin);
    command_caps(&temp);
    version_and_control_caps(&temp);
}

fn combined_metadata_cap(temp: &TempDirectory) {
    // Index bytes become live while the streaming scratch and caller decode are still held.
    let stream_scratch = (1 << 20) + 65_536;
    for beyond in [0, 1] {
        let limits = StorageLimits {
            metadata_bytes: stream_scratch + 20 + 256 - beyond,
            ..StorageLimits::default()
        };
        let mut owner = store(
            temp,
            &format!("combined-metadata-{beyond}.kinewright"),
            limits,
        );
        let decode = owner.reserve_probe(20, 0).unwrap();
        let bytes = b"four";
        let reference = reference(bytes);
        let result = owner.admit_blob(&mut Cursor::new(bytes), &reference, &[]);
        assert_eq!(
            result.is_ok(),
            beyond == 0,
            "combined stored metadata and live decode exceeded the admitted cap"
        );
        if beyond == 1 {
            assert_eq!(owner.usage().unwrap().physical_bytes, 0);
            assert_eq!(owner.usage().unwrap().objects, 0);
        }
        drop(result);
        drop(decode);
    }
}

fn version_and_control_caps(temp: &TempDirectory) {
    for delta in [0, 1] {
        let path = temp.path(&format!("control-{delta}.kinewright"));
        let (core, mut bundle, inputs) = setup(&path);
        let before = fs::read(&path).unwrap();
        let mut limits = StorageLimits::default();
        limits.control_bytes = before.len() as u64 - delta;
        bundle.restrict_limits_probe(limits).unwrap();
        let result = bundle.save(&core, &inputs);
        assert_eq!(result.is_ok(), delta == 0, "{result:?}");
        if delta == 1 {
            assert_eq!(fs::read(&path).unwrap(), before);
        }
    }
    let path = temp.path("versions.kinewright");
    let (core, mut bundle, _) = setup(&path);
    let branch = bundle.manual_branch_id().clone();
    let checkpoint = bundle.checkpoint(&branch).unwrap().clone();
    let mut base = SpecialistBaseRecord {
        target_branch: branch.clone(),
        base_head: checkpoint.head_id,
        base_document_hash: checkpoint.document_hash,
        original_baseline_ref: checkpoint.baseline_ref,
        frozen_checkpoint_ref: bundle.checkpoint_reference(&branch).unwrap(),
        full_applied_delta: vec![],
    };
    let mut limits = StorageLimits::default();
    limits.versions = 1;
    bundle.restrict_limits_probe(limits).unwrap();
    let reference = bundle.retain_specialist_base(&base).unwrap();
    let root = fs::read(&path).unwrap();
    let before = core.export_history().unwrap();
    base.full_applied_delta.push(lock(false));
    assert!(matches!(
        bundle.retain_specialist_base(&base),
        Err(BundleError::Capacity(_))
    ));
    assert_eq!(core.export_history().unwrap(), before);
    assert_eq!(fs::read(&path).unwrap(), root);
    assert!(object_path(&path, &reference).exists());
}

fn command_caps(temp: &TempDirectory) {
    let path = temp.path("comment-boundary.kinewright");
    let (core, mut bundle, inputs) = setup(&path);
    let mut comment = request(&mut bundle, &core, &inputs, JournalCommand::Undo);
    comment.action = "comment".into();
    comment.command = None;
    let text = b"four";
    let points = b"[[1,2]]";
    let mut a = reference(text);
    a.record_kind = RecordKind::OwnedEvidence;
    let mut b = reference(points);
    b.record_kind = RecordKind::OwnedEvidence;
    comment.comment_text = Some(a.clone());
    comment.comment_strokes = Some(b.clone());
    let mut limits = StorageLimits::default();
    limits.comments = 1;
    limits.comment_text_bytes = 4;
    limits.comment_points = 1;
    bundle.restrict_limits_probe(limits).unwrap();
    committed(
        bundle
            .command(&comment, &core, &inputs, &[(&a, text), (&b, points)])
            .unwrap(),
    );
    let root = fs::read(&path).unwrap();
    let before = core.export_history().unwrap();
    comment.id.sequence = 2;
    comment.expected_head = bundle.manual_head().cloned();
    comment.expected_revision = revision(&core);
    assert!(matches!(
        bundle.command(&comment, &core, &inputs, &[(&a, text), (&b, points)]),
        Err(BundleError::Capacity(_))
    ));
    assert_eq!(fs::read(&path).unwrap(), root);
    assert_eq!(core.export_history().unwrap(), before);
    let path = temp.path("commands.kinewright");
    let (core, mut bundle, inputs) = setup(&path);
    let mut edit = request(&mut bundle, &core, &inputs, JournalCommand::Do(lock(false)));
    let mut limits = StorageLimits::default();
    limits.command_bytes = edit.encoded_bytes().unwrap();
    limits.pending_commands = 1;
    limits.receipts = 1;
    limits.issuers = 1;
    bundle.restrict_limits_probe(limits).unwrap();
    committed(bundle.command(&edit, &core, &inputs, &[]).unwrap());
    let before = core.export_history().unwrap();
    let root = fs::read(&path).unwrap();
    edit.id.sequence = 2;
    edit.expected_head = bundle.manual_head().cloned();
    edit.expected_revision = revision(&core);
    assert!(bundle.command(&edit, &core, &inputs, &[]).is_err());
    assert_eq!(core.export_history().unwrap(), before);
    assert_eq!(fs::read(&path).unwrap(), root);
    assert!(bundle.issue_issuer(&core, &inputs, 1).is_err());
    for field in ["command", "pending", "text", "points", "comments"] {
        let path = temp.path(&format!("{field}-commands.kinewright"));
        let (core, mut bundle, inputs) = setup(&path);
        let mut request = request(&mut bundle, &core, &inputs, JournalCommand::Do(lock(false)));
        let mut limits = StorageLimits::default();
        if field == "command" {
            limits.command_bytes = request.encoded_bytes().unwrap() - 1;
        } else if field == "pending" {
            limits.pending_commands = 0;
        } else {
            request.action = "comment".into();
            request.command = None;
            let text = b"four";
            let points = b"[[1,2]]";
            let mut a = reference(text);
            a.record_kind = RecordKind::OwnedEvidence;
            let mut b = reference(points);
            b.record_kind = RecordKind::OwnedEvidence;
            request.comment_text = Some(a);
            request.comment_strokes = Some(b);
            match field {
                "text" => limits.comment_text_bytes = 3,
                "points" => limits.comment_points = 0,
                _ => limits.comments = 0,
            }
        }
        bundle.restrict_limits_probe(limits).unwrap();
        let before = core.export_history().unwrap();
        let root = fs::read(&path).unwrap();
        let blobs: Vec<_> = request
            .comment_text
            .iter()
            .map(|r| (r, b"four".as_slice()))
            .chain(
                request
                    .comment_strokes
                    .iter()
                    .map(|r| (r, b"[[1,2]]".as_slice())),
            )
            .collect();
        assert!(
            bundle.command(&request, &core, &inputs, &blobs).is_err(),
            "{field} cap not enforced"
        );
        assert_eq!(core.export_history().unwrap(), before);
        assert_eq!(fs::read(&path).unwrap(), root);
    }
}
