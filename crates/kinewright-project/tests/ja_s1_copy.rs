#![cfg(all(feature = "test-util", target_os = "linux"))]
use kinewright_core::*;
use kinewright_media::test_support::TempDirectory;
use kinewright_project::{
    bundle::{BundleError, BundleWriteAdmission, LoadedBundle, ProjectBundle},
    bundle_copy::{BundleCopy, CopyStatus, SaveAsOutcome, SessionStore},
    bundle_records::{ObjectRef, RecordKind, SpecialistBaseRecord},
    io_observer,
    owned_inputs::{InputRequest, OwnedInputEntry, OwnedInputManifest, OwnedInputRef},
    receipts::{CommandEnvelope, CommandId, ReceiptLookup},
    storage_budget::StorageLimits,
    store::{ProjectStore, SealedObject},
};
use std::{
    collections::BTreeMap,
    fs,
    io::{Cursor, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const LUT: &[u8] =
    b"TITLE \"Exact LUT\"\nLUT_3D_SIZE 2\n0 0 0\n0 0 1\n0 1 0\n0 1 1\n1 0 0\n1 0 1\n1 1 0\n1 1 1\n";
fn reference(bytes: &[u8], kind: RecordKind) -> ObjectRef {
    ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: kind,
    }
}
fn apply(core: &Core, command: Command) {
    assert!(matches!(
        core.request(command).unwrap(),
        Event::DocumentChanged { .. }
    ));
}
fn core(asset: MediaAsset) -> Core {
    let mut doc = Document::default();
    doc.media_pool.push(asset);
    doc.tracks.push(Track {
        id: TrackId(1),
        kind: TrackKind::Video,
        sync_lock: true,
        clips: vec![],
    });
    doc.lut_assets.push(LutAsset {
        id: LutAssetId(1),
        sha256: kinewright_media::sha256_bytes(LUT),
        title: "exact LUT".into(),
        kind: LutAssetKind::Cube3d,
        size: 2,
        byte_len: LUT.len() as u64,
        domain_min_millionths: [0; 3],
        domain_max_millionths: [1_000_000; 3],
        source: LutAssetSource::Imported {
            source_path: "/missing/external.cube".into(),
        },
    });
    let core = Core::spawn(doc).unwrap();
    apply(
        &core,
        Command::Do(Operation::AddTitle {
            track: TrackId(1),
            at: TimeCode(0),
            duration: TimeCode(30),
            title: Title::default(),
        }),
    );
    apply(
        &core,
        Command::Do(Operation::RemoveLutAsset {
            lut_asset: LutAssetId(1),
        }),
    );
    apply(
        &core,
        Command::Do(Operation::DeleteClip { clip: ClipId(1) }),
    );
    apply(&core, Command::Undo);
    assert_eq!(core.export_history().unwrap().cursor.redo.len(), 1);
    assert!(
        core.export_history()
            .unwrap()
            .document
            .lut_assets
            .is_empty()
    );
    core
}
fn known_wav_f32(tone: f32) -> Vec<u8> {
    let data_length = 4_800_u32 * 4;
    let mut bytes = Vec::new();
    bytes.extend(b"RIFF");
    bytes.extend((data_length + 36).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16_u32.to_le_bytes());
    bytes.extend(3_u16.to_le_bytes());
    bytes.extend(1_u16.to_le_bytes());
    bytes.extend(48_000_u32.to_le_bytes());
    bytes.extend(192_000_u32.to_le_bytes());
    bytes.extend(4_u16.to_le_bytes());
    bytes.extend(32_u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(data_length.to_le_bytes());
    for sample in 0..4_800_u16 {
        let value = 0.25 * (std::f32::consts::TAU * tone * f32::from(sample) / 48_000.0).sin();
        bytes.extend(value.to_le_bytes());
    }
    bytes
}
fn assets(
    store: &mut ProjectStore,
) -> (
    MediaAsset,
    Vec<OwnedInputEntry>,
    Vec<SealedObject>,
    BTreeMap<String, Vec<u8>>,
) {
    let mut entries = vec![];
    let mut exact = BTreeMap::new();
    for (id, kind, bytes) in [
        ("font:Inter", RecordKind::OwnedFont, title_font_bytes()),
        ("lut:1", RecordKind::OwnedLut, LUT),
        (
            "notice",
            RecordKind::OwnedNotice,
            b"exact notice  \r\n".as_slice(),
        ),
        (
            "script",
            RecordKind::OwnedScript,
            b"exact script\t\r\n".as_slice(),
        ),
        (
            "data",
            RecordKind::OwnedData,
            b"{\"exact\": [1, 2]}\r\n".as_slice(),
        ),
        (
            "style",
            RecordKind::OwnedStyle,
            b"  exact style  \r\n".as_slice(),
        ),
        (
            "reference",
            RecordKind::OwnedReference,
            b"exact reference bytes\0".as_slice(),
        ),
    ] {
        let object = reference(bytes, kind);
        let sealed = store
            .admit_blob(&mut Cursor::new(bytes), &object, &[])
            .unwrap();
        entries.push(OwnedInputEntry {
            entry_id: id.into(),
            object,
            dependencies: vec![],
            selected_stream: None,
            time_base: None,
            origin: None,
            profile: None,
        });
        exact.insert(id.into(), bytes.to_vec());
        drop(sealed);
    }
    let bytes = known_wav_f32(440.0);
    let (asset, entry, pins) = store
        .import_media(
            &mut bytes.as_slice(),
            bytes.len() as u64,
            AssetId(1),
            Path::new("/missing/source.wav"),
            None,
        )
        .unwrap();
    exact.insert(entry.entry_id.clone(), bytes);
    entries.insert(1, entry);
    (asset, entries, pins, exact)
}
fn named(path: &Path) -> (Core, ProjectBundle, ObjectRef, BTreeMap<String, Vec<u8>>) {
    let mut bundle = ProjectBundle::activate(path, BundleWriteAdmission::new_document()).unwrap();
    let (asset, entries, pins, exact) = assets(bundle.store_mut().unwrap());
    let core = core(asset);
    let manifest = OwnedInputManifest {
        project_id: Some(bundle.project_id().clone()),
        session_id: None,
        owner_epoch: bundle.store().owner_epoch(),
        entries,
    };
    let inputs = bundle.admit_inputs(&manifest).unwrap();
    let reference = inputs.reference.clone();
    bundle.save(&core, &reference).unwrap();
    drop(pins);
    (core, bundle, reference, exact)
}
fn copied(outcome: SaveAsOutcome) -> BundleCopy {
    match outcome {
        SaveAsOutcome::Copied(copy) => *copy,
        SaveAsOutcome::SameProject(_) => panic!("expected a distinct copy"),
    }
}
fn finish(copy: BundleCopy) -> ProjectBundle {
    copy.into_destination()
        .unwrap_or_else(|_| panic!("copy has not committed"))
}
fn byte_session(
    parent: &Path,
    bytes: &[u8],
) -> (
    SessionStore,
    kinewright_project::store::SealedObject,
    OwnedInputRef,
    Core,
) {
    let mut session = SessionStore::create(parent, StorageLimits::default()).unwrap();
    let request = InputRequest {
        project_id: None,
        session_id: Some(session.session_id().clone()),
        owner_epoch: session.store().owner_epoch(),
        job: "copy-source".into(),
        object: reference(bytes, RecordKind::OwnedEvidence),
        source_locator_hint: Some(parent.join("locator-is-not-data")),
        dependencies: vec![],
        selected_stream: None,
        time_base: None,
        origin: None,
        profile: None,
    };
    let (reference, sealed) = session
        .store_mut()
        .admit_input(&request, &mut Cursor::new(bytes))
        .unwrap();
    let history = Core::spawn(Document::default()).unwrap();
    (session, sealed, reference, history)
}
fn reopen(path: &Path) -> (Core, ProjectBundle) {
    let loaded = LoadedBundle::read(path).unwrap();
    let core = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&core).unwrap();
    (core, loaded.into_writer().unwrap())
}
fn read_assets(bundle: &ProjectBundle, expected: &BTreeMap<String, Vec<u8>>) {
    let root = bundle.root().unwrap();
    let manifest = bundle
        .store()
        .read_input_manifest(&root.input_manifest_ref)
        .unwrap();
    assert_eq!(manifest.project_id.as_ref(), Some(bundle.project_id()));
    for entry in &manifest.entries {
        for dependency in &entry.dependencies {
            bundle.store().verify(dependency).unwrap();
        }
        let mut reader = bundle
            .store()
            .lease_input(&OwnedInputRef {
                manifest_ref: root.input_manifest_ref.clone(),
                entry: entry.clone(),
            })
            .unwrap();
        assert_eq!(reader.owner_epoch(), bundle.store().owner_epoch());
        let mut bytes = vec![];
        reader.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, expected[&entry.entry_id]);
    }
}
fn graph(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        if !path.exists() {
            return;
        }
        if path.is_file() {
            out.insert(
                path.strip_prefix(root).unwrap().to_path_buf(),
                fs::read(path).unwrap(),
            );
            return;
        }
        for entry in fs::read_dir(path).unwrap() {
            walk(root, &entry.unwrap().path(), out);
        }
    }
    let base = path.parent().unwrap();
    let mut result = BTreeMap::new();
    walk(base, path, &mut result);
    walk(base, &path.with_extension("kinewright-assets"), &mut result);
    result
}

#[allow(
    clippy::items_after_statements,
    reason = "RAII child guard belongs to this controlled process test"
)]
fn controlled_copy_case(
    source: &Path,
    destination: &Path,
    signals: &Path,
    kind: &str,
    hook: &str,
    while_paused: impl FnOnce(),
) {
    fs::create_dir(signals).unwrap();
    let log = fs::File::create(signals.join("child.log")).unwrap();
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = Child(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "save_as_alias_traversal_case_collision_and_foreign_store_refuse",
                "--nocapture",
            ])
            .env("JA_S1_COPY_SPECIAL_CASE", kind)
            .env("JA_S1_COPY_SOURCE", source)
            .env("JA_S1_COPY_DESTINATION", destination)
            .env("REV2_HOOK", hook)
            .env("REV2_SIGNALS", signals)
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    while !signals.join("paused").exists() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "copy setup failed before barrier: {}",
            fs::read_to_string(signals.join("child.log")).unwrap()
        );
        assert!(Instant::now() < deadline, "copy barrier setup deadline");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(fs::read_to_string(signals.join("paused")).unwrap(), hook);
    while_paused();
    fs::write(signals.join("resume"), b"continue the same copy attempt").unwrap();
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(
                status.success(),
                "copy property failed: {}",
                fs::read_to_string(signals.join("child.log")).unwrap()
            );
            break;
        }
        assert!(Instant::now() < deadline, "copy completion deadline");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one complete retained graph with independent receipt and specialist pins"
)]
fn save_as_copies_every_retained_asset_including_luts_before_root() {
    let temp = TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "ja-s1-copy-all");
    let source = temp.path("source.kinewright");
    let destination = temp.path("new/parents/copied.kinewright");
    let (core, mut bundle, inputs, exact) = named(&source);
    let issuer = bundle.issue_issuer(&core, &inputs, 1).unwrap();
    let Event::QueryResult(QueryResult::Snapshot { revision, .. }) =
        core.request(Command::Query(Query::Snapshot)).unwrap()
    else {
        panic!("snapshot")
    };
    let text = " exact comment 🧭  \r\n".as_bytes();
    let strokes = b" [ [3,5], [13,21] ]\r\n";
    let text_ref = reference(text, RecordKind::OwnedEvidence);
    let strokes_ref = reference(strokes, RecordKind::OwnedEvidence);
    let request = CommandEnvelope {
        switch: None,
        manual: None,
        version: 1,
        action: "comment".into(),
        id: CommandId {
            issuer,
            sequence: 1,
        },
        project_id: bundle.project_id().clone(),
        target_branch: bundle.manual_branch_id().clone(),
        owner_epoch: bundle.store().owner_epoch(),
        lease: None,
        expected_head: bundle.manual_head().cloned(),
        expected_revision: revision,
        command: None,
        comment_text: Some(text_ref.clone()),
        comment_strokes: Some(strokes_ref.clone()),
        evidence: vec![],
        transition: None,
    };
    let ReceiptLookup::Committed(comment) = bundle
        .command(
            &request,
            &core,
            &inputs,
            &[(&text_ref, text), (&strokes_ref, strokes)],
        )
        .unwrap()
    else {
        panic!("comment did not commit")
    };
    let manual = bundle.manual_branch_id().clone();
    let base_checkpoint = bundle.checkpoint(&manual).unwrap().clone();
    let frozen_checkpoint_ref = bundle.checkpoint_reference(&manual).unwrap();
    let candidate = bundle.fork_branch(&manual).unwrap();
    let candidate_core = Core::spawn(Document::default()).unwrap();
    candidate_core
        .restore_history((*bundle.branch_history(&candidate).unwrap()).clone())
        .unwrap();
    apply(
        &candidate_core,
        Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked: false,
        }),
    );
    let candidate_history = candidate_core.export_history().unwrap();
    bundle.save_branch(&candidate, &candidate_core).unwrap();
    let base = SpecialistBaseRecord {
        target_branch: manual,
        base_head: base_checkpoint.head_id,
        base_document_hash: base_checkpoint.document_hash,
        original_baseline_ref: base_checkpoint.document_ref,
        frozen_checkpoint_ref,
        full_applied_delta: candidate_history.applied_operations(),
    };
    let base_ref = bundle.retain_specialist_base(&base).unwrap();
    let before = graph(&source);
    let history = core.export_history().unwrap();
    let project = bundle.project_id().clone();
    let branch = bundle.manual_branch_id().clone();
    let (copy, trace) = io_observer::observe(|| copied(bundle.save_as(&destination).unwrap()));
    assert!(!trace.overflowed);
    let root = trace
        .events
        .iter()
        .position(|e| e.operation == "root_rename")
        .unwrap();
    for bytes in exact.values() {
        let hash = kinewright_media::sha256_bytes(bytes);
        assert!(
            trace.events[..root].iter().any(
                |e| e.operation == "object_stage" && e.attempted_sha256.as_ref() == Some(&hash)
            ),
            "asset was not copied before root: {hash}"
        );
    }
    for reference in [&text_ref, &strokes_ref] {
        assert!(
            trace.events[..root]
                .iter()
                .any(|e| e.operation == "object_stage"
                    && e.attempted_sha256.as_ref() == Some(&reference.sha256))
        );
    }
    assert!(
        trace
            .events
            .iter()
            .any(|e| e.operation == "mkdir" && e.path.ends_with("parents"))
    );
    let destination_owner = finish(copy);
    assert_ne!(destination_owner.project_id(), &project);
    assert_eq!(destination_owner.manual_branch_id(), &branch);
    assert!(
        destination_owner
            .root()
            .unwrap()
            .origin_project_ids
            .contains(&project)
    );
    read_assets(&destination_owner, &exact);
    assert!(
        destination_owner
            .root()
            .unwrap()
            .candidate_branch_ids
            .contains(&candidate)
    );
    assert_eq!(
        *destination_owner.branch_history(&candidate).unwrap(),
        candidate_history
    );
    assert_eq!(
        *destination_owner
            .store()
            .read_record::<SpecialistBaseRecord>(&base_ref)
            .unwrap(),
        base
    );
    let candidate_checkpoint = destination_owner.checkpoint(&candidate).unwrap();
    for reference in &candidate_checkpoint.history_input_manifest_refs {
        assert_eq!(
            destination_owner
                .store()
                .read_input_manifest(reference)
                .unwrap()
                .project_id
                .as_ref(),
            Some(destination_owner.project_id())
        );
    }
    assert_eq!(
        &*destination_owner.store().read_evidence(&text_ref).unwrap(),
        text
    );
    assert_eq!(
        &*destination_owner
            .store()
            .read_evidence(&strokes_ref)
            .unwrap(),
        strokes
    );
    let destination_inputs = destination_owner.root().unwrap().input_manifest_ref.clone();
    let mut destination_owner = destination_owner;
    let (retried, trace) = io_observer::observe(|| {
        destination_owner.command(&request, &core, &destination_inputs, &[])
    });
    assert_eq!(retried.unwrap(), ReceiptLookup::Committed(comment));
    assert!(trace.events.is_empty());
    let mut fresh = request.clone();
    fresh.id.sequence += 1;
    fresh.project_id = destination_owner.project_id().clone();
    fresh.owner_epoch = destination_owner.store().owner_epoch();
    fresh.expected_head = destination_owner.manual_head().cloned();
    assert_eq!(
        destination_owner
            .command(&fresh, &core, &destination_inputs, &[])
            .unwrap(),
        ReceiptLookup::RetiredUnresolved,
        "copied issuers are archival, never live grants"
    );
    assert_eq!(graph(&source), before);
    drop(destination_owner);
    let (restored, destination_owner) = reopen(&destination);
    read_assets(&destination_owner, &exact);
    assert_eq!(restored.export_history().unwrap(), history);
    for command in [Command::Redo, Command::Undo, Command::Undo] {
        apply(&core, command.clone());
        apply(&restored, command);
        assert_eq!(
            core.export_history().unwrap(),
            restored.export_history().unwrap()
        );
    }
}

#[test]
fn save_as_contention_preserves_source_and_destination_graphs() {
    let temp = TempDirectory::new_in(
        Path::new(env!("CARGO_TARGET_TMPDIR")),
        "ja-s1-copy-contention",
    );
    let a = temp.path("a.kinewright");
    let b = temp.path("b.kinewright");
    let (_, mut first, _, exact) = named(&a);
    let (second_core, mut second, second_inputs, _) = named(&b);
    // A failed copy must also preserve the prior conservative recovery inventory.
    for _ in 0..4 {
        second.save(&second_core, &second_inputs).unwrap();
    }
    let before_a = graph(&a);
    let before_b = graph(&b);
    let start = Instant::now();
    assert!(matches!(first.save_as(&b), Err(BundleError::Contention(_))));
    assert!(start.elapsed() < Duration::from_secs(5));
    assert_eq!(graph(&a), before_a);
    assert_eq!(graph(&b), before_b);
    drop(second);
    for fault in [
        "copy_object",
        "copy_verify",
        "copy_backup",
        "root_before_publish",
    ] {
        let (result, trace) =
            io_observer::observe(|| io_observer::fail_once(fault, || first.save_as(&b)));
        assert!(matches!(result, Err(BundleError::Io(_))), "{fault}");
        assert!(
            trace
                .events
                .iter()
                .any(|e| e.operation == "injected_failure")
        );
        assert_eq!(graph(&a), before_a, "source after {fault}");
        assert_eq!(graph(&b), before_b, "complete destination after {fault}");
        assert!(LoadedBundle::read(&b).is_ok());
        assert_eq!(first.store().usage().unwrap().reserved_copy_bytes, 0);
    }
    let quota = temp.path("quota.kinewright");
    let limits = StorageLimits {
        owned_bytes: 1,
        ..StorageLimits::default()
    };
    assert!(matches!(
        first.save_as_with_limits(&quota, limits),
        Err(BundleError::Capacity(_))
    ));
    assert!(!quota.exists());
    assert!(!quota.with_extension("kinewright-assets").exists());
    assert_eq!(graph(&a), before_a);
    let owner = finish(copied(first.save_as(&b).unwrap()));
    read_assets(&owner, &exact);
    assert_eq!(graph(&a), before_a);
    drop(owner);
    assert!(LoadedBundle::read(&b).is_ok());
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one namespace witness including controlled retarget and corruption"
)]
fn save_as_alias_traversal_case_collision_and_foreign_store_refuse() {
    use std::os::unix::fs::symlink;
    if let Ok(kind) = std::env::var("JA_S1_COPY_SPECIAL_CASE") {
        let source = PathBuf::from(std::env::var_os("JA_S1_COPY_SOURCE").unwrap());
        let destination = PathBuf::from(std::env::var_os("JA_S1_COPY_DESTINATION").unwrap());
        let (_, mut owner) = reopen(&source);
        let before = graph(&source);
        let result = owner.save_as(&destination);
        if kind == "alias" {
            assert!(
                matches!(result, Err(BundleError::Identity(_))),
                "retargeted destination was mutated or acknowledged"
            );
        } else {
            assert!(
                matches!(result, Err(BundleError::Corrupt(_))),
                "tampered destination dependency was acknowledged"
            );
        }
        assert_eq!(graph(&source), before);
        return;
    }
    let temp = TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "ja-s1-copy-paths");
    let path = temp.path("source.kinewright");
    let (_, mut source, _, _) = named(&path);
    let alias = temp.path("alias.kinewright");
    symlink(&path, &alias).unwrap();
    let before = graph(&path);
    let project = source.project_id().clone();
    let (same, trace) = io_observer::observe(|| source.save_as(&alias));
    let SaveAsOutcome::SameProject(report) = same.unwrap() else {
        panic!("same project alias acquired another owner")
    };
    assert_eq!(report.project_id, project);
    assert!(trace.events.is_empty());
    assert_eq!(graph(&path), before);
    let foreign = temp.path("foreign.kinewright");
    fs::create_dir(foreign.with_extension("kinewright-assets")).unwrap();
    fs::write(
        foreign.with_extension("kinewright-assets").join("keep"),
        b"foreign exact bytes",
    )
    .unwrap();
    for destination in [
        temp.path("other/../escape.kinewright"),
        temp.path("SOURCE.kinewright"),
        temp.path("CON.kinewright"),
        foreign.clone(),
    ] {
        let before_foreign = graph(&foreign);
        let (result, trace) = io_observer::observe(|| source.save_as(&destination));
        assert!(result.is_err());
        assert!(trace.events.is_empty());
        assert_eq!(graph(&foreign), before_foreign);
        assert_eq!(graph(&path), before);
    }
    let admitted = fs::read(&path).unwrap();
    fs::write(&path, b"{\"format_version\":4}").unwrap();
    let rejected = temp.path("changed-source-copy.kinewright");
    let (result, trace) = io_observer::observe(|| source.save_as(&rejected));
    assert!(result.is_err());
    assert!(trace.events.is_empty());
    assert!(!rejected.exists());
    fs::write(&path, admitted).unwrap();
    drop(source);
    let pinned = temp.path("pinned-parent");
    let other = temp.path("other-parent");
    fs::create_dir(&pinned).unwrap();
    fs::create_dir(&other).unwrap();
    fs::write(other.join("keep"), b"foreign exact bytes").unwrap();
    let alias_destination = pinned.join("copy.kinewright");
    controlled_copy_case(
        &path,
        &alias_destination,
        &temp.path("alias-signals"),
        "alias",
        "copy_destination_pinned",
        || {
            fs::rename(&pinned, temp.path("original-parent")).unwrap();
            symlink(&other, &pinned).unwrap();
        },
    );
    assert_eq!(
        fs::read(other.join("keep")).unwrap(),
        b"foreign exact bytes"
    );
    assert_eq!(fs::read_dir(&other).unwrap().count(), 1);
    assert_eq!(graph(&path), before);
    let verified = temp.path("verified.kinewright");
    let font = verified
        .with_extension("kinewright-assets")
        .join("studio/objects")
        .join(kinewright_media::sha256_bytes(title_font_bytes()));
    controlled_copy_case(
        &path,
        &verified,
        &temp.path("verify-signals"),
        "verify",
        "copy_before_publish",
        || {
            let mut bytes = fs::read(&font).unwrap();
            bytes[0] ^= 1;
            fs::write(&font, bytes).unwrap();
        },
    );
    assert!(!verified.exists());
    assert!(!verified.with_extension("kinewright-assets").exists());
    assert_eq!(graph(&path), before);
}

#[test]
fn first_save_failure_undo_live_reader() {
    let temp = TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "ja-s1-first-save");
    let path = temp.path("saved.kinewright");
    let mut session = SessionStore::create(temp.root(), StorageLimits::default()).unwrap();
    let (asset, entries, pins, exact) = assets(session.store_mut());
    let core = core(asset);
    let history = core.export_history().unwrap();
    let inputs = session
        .admit_inputs(&OwnedInputManifest {
            project_id: None,
            session_id: Some(session.session_id().clone()),
            owner_epoch: session.store().owner_epoch(),
            entries,
        })
        .unwrap();
    drop(pins);
    let input_ref = inputs.reference.clone();
    let manifest = session.store().read_input_manifest(&input_ref).unwrap();
    let source_ref = OwnedInputRef {
        manifest_ref: input_ref.clone(),
        entry: manifest.entries[0].clone(),
    };
    drop(manifest);
    let mut reader = session.store().lease_input(&source_ref).unwrap();
    let source_root = session
        .store()
        .resolved_root()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let usage = session.store().usage_handle();
    let physical = usage().unwrap().physical_bytes;
    let failed = io_observer::fail_once("copy_object", || {
        session.first_save(&path, &core, &input_ref)
    });
    assert!(matches!(failed, Err(BundleError::Io(_))));
    assert!(!path.exists());
    assert_eq!(usage().unwrap().physical_bytes, physical);
    assert_eq!(usage().unwrap().reserved_copy_bytes, 0);
    apply(&core, Command::Redo);
    apply(&core, Command::Undo);
    let continued = core.export_history().unwrap();
    assert_eq!(continued.document, history.document);
    assert_eq!(continued.cursor, history.cursor);
    let history = continued;
    reader.seek(SeekFrom::Start(0)).unwrap();
    let copy = session.first_save(&path, &core, &input_ref).unwrap();
    assert!(usage().unwrap().reserved_copy_bytes > 0);
    let rebound = copy.rebound_input(&source_ref).unwrap();
    assert_ne!(rebound.manifest_ref, source_ref.manifest_ref);
    let destination = finish(copy);
    read_assets(&destination, &exact);
    assert_ne!(destination.store().owner_epoch(), reader.owner_epoch());
    assert!(destination.store().lease_input(&source_ref).is_err());
    assert!(destination.store().lease_input(&rebound).is_ok());
    assert_eq!(usage().unwrap().reserved_copy_bytes, 0);
    drop(inputs);
    drop(session);
    assert!(source_root.exists());
    assert_eq!(usage().unwrap().physical_bytes, physical);
    let mut bytes = vec![];
    reader.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, exact["font:Inter"]);
    drop(reader);
    assert!(!source_root.exists());
    assert_eq!(usage().unwrap().physical_bytes, 0);
    assert!(!usage().unwrap().damaged_accounting);
    let reopened = LoadedBundle::read(&path).unwrap();
    let restored = Core::spawn(Document::default()).unwrap();
    reopened.restore_manual(&restored).unwrap();
    assert_eq!(restored.export_history().unwrap(), history);
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "complete copy reconciliation and live-reader witness"
)]
fn copy_b5_unknown_reconciles_before_handoff_or_retry() {
    let temp = TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "ja-s1-copy-unknown");
    let source = temp.path("source.kinewright");
    let destination = temp.path("copy.kinewright");
    let (core, mut bundle, _, exact) = named(&source);
    let before = graph(&source);
    let usage = bundle.store().usage_handle();
    let copy = io_observer::fail_once("root_parent_sync", || {
        copied(bundle.save_as(&destination).unwrap())
    });
    assert!(matches!(copy.status(), CopyStatus::DurabilityUnknown));
    assert!(usage().unwrap().unsettled);
    assert!(usage().unwrap().reserved_copy_bytes > 0);
    let mut pending = copy
        .into_destination()
        .err()
        .expect("uncertain destination writer escaped");
    assert!(bundle.save_as(&destination).is_err());
    assert_eq!(graph(&source), before);
    let report = pending.reconcile().unwrap().unwrap();
    assert_eq!(
        pending.reconcile().unwrap().unwrap().root_sha256,
        report.root_sha256
    );
    let destination_owner = finish(*pending);
    assert_eq!(usage().unwrap().reserved_copy_bytes, 0);
    assert!(!usage().unwrap().unsettled);
    read_assets(&destination_owner, &exact);
    assert_eq!(graph(&source), before);
    drop(destination_owner);
    let (restored, _) = reopen(&destination);
    assert_eq!(
        restored.export_history().unwrap(),
        core.export_history().unwrap()
    );
    let (mut session, sealed, input, history) =
        byte_session(temp.root(), b"frozen source without another reader");
    let source_directory = session
        .store()
        .resolved_root()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let monitor = session.store().usage_handle();
    let copy = session
        .first_save(
            &temp.path("without-reader.kinewright"),
            &history,
            &input.manifest_ref,
        )
        .unwrap();
    drop(sealed);
    assert_eq!(
        session.store_mut().collect_unpublished().unwrap(),
        0,
        "the copy owns the whole source closure until handoff"
    );
    let mut original = session.store().lease_input(&input).unwrap();
    let mut bytes = vec![];
    original.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"frozen source without another reader");
    drop(original);
    drop(session);
    assert!(source_directory.exists());
    let _saved = finish(copy);
    assert!(!source_directory.exists());
    assert_eq!(monitor().unwrap().physical_bytes, 0);
    let (mut session, sealed, input, history) =
        byte_session(temp.root(), b"uncertain first-save source");
    let source_directory = session
        .store()
        .resolved_root()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let monitor = session.store().usage_handle();
    let mut copy = io_observer::fail_once("root_parent_sync", || {
        session
            .first_save(
                &temp.path("uncertain-session.kinewright"),
                &history,
                &input.manifest_ref,
            )
            .unwrap()
    });
    assert!(matches!(copy.status(), CopyStatus::DurabilityUnknown));
    drop(sealed);
    drop(session);
    assert!(source_directory.exists());
    assert!(monitor().unwrap().physical_bytes > 0);
    copy.reconcile().unwrap().unwrap();
    assert!(source_directory.exists());
    let _saved = finish(copy);
    assert!(!source_directory.exists());
    assert_eq!(monitor().unwrap().physical_bytes, 0);
    assert_eq!(monitor().unwrap().reserved_copy_bytes, 0);
    assert!(!monitor().unwrap().damaged_accounting);
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "two native owners, barrier, bounded refusal and retry are one witness"
)]
#[allow(
    clippy::items_after_statements,
    reason = "native child role returns before the controller-only child guard"
)]
fn opposing_save_as_try_locks_do_not_deadlock() {
    const CHILD: &str = "JA_S1_COPY_PARTICIPANT";
    if let Ok(role) = std::env::var(CHILD) {
        let directory = PathBuf::from(std::env::var_os("JA_S1_COPY_DIRECTORY").unwrap());
        let (source, destination) = if role == "A" {
            ("a.kinewright", "b.kinewright")
        } else {
            ("b.kinewright", "a.kinewright")
        };
        let source = directory.join(source);
        let destination = directory.join(destination);
        let (_, mut owner) = reopen(&source);
        fs::write(
            directory.join(format!("{role}.ready")),
            std::process::id().to_string(),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !directory.join("go").exists() {
            assert!(Instant::now() < deadline, "controller setup deadline");
            std::thread::sleep(Duration::from_millis(5));
        }
        let before_source = graph(&source);
        let before_destination = graph(&destination);
        let start = Instant::now();
        let result = owner.save_as(&destination);
        let elapsed = start.elapsed();
        assert!(
            matches!(result, Err(BundleError::Contention(_))),
            "opposing destination did not refuse contention"
        );
        assert!(elapsed < Duration::from_secs(5));
        assert_eq!(graph(&source), before_source);
        assert_eq!(graph(&destination), before_destination);
        fs::write(directory.join(format!("{role}.result")),serde_json::to_vec(&serde_json::json!({"pid":std::process::id(),"contention":true,"source_preserved":true,"destination_preserved":true,"elapsed_ms":elapsed.as_millis()})).unwrap()).unwrap();
        while !directory.join("release").exists() {
            assert!(Instant::now() < deadline, "controller release deadline");
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(owner);
        return;
    }
    struct Participant(std::process::Child);
    impl Drop for Participant {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let temp = TempDirectory::new_in(
        Path::new(env!("CARGO_TARGET_TMPDIR")),
        "ja-s1-opposing-copies",
    );
    let a = temp.path("a.kinewright");
    let b = temp.path("b.kinewright");
    let (_, first, _, exact) = named(&a);
    let (_, second, _, _) = named(&b);
    drop(first);
    drop(second);
    let before_a = graph(&a);
    let before_b = graph(&b);
    let executable = std::env::current_exe().unwrap();
    let mut children = vec![];
    for role in ["A", "B"] {
        let log = fs::File::create(temp.path(&format!("{role}.log"))).unwrap();
        children.push(Participant(
            std::process::Command::new(&executable)
                .args([
                    "--exact",
                    "opposing_save_as_try_locks_do_not_deadlock",
                    "--nocapture",
                ])
                .env(CHILD, role)
                .env("JA_S1_COPY_DIRECTORY", temp.root())
                .stdout(log.try_clone().unwrap())
                .stderr(log)
                .spawn()
                .unwrap(),
        ));
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    for suffix in ["ready", "result"] {
        for role in ["A", "B"] {
            let path = temp.path(&format!("{role}.{suffix}"));
            while !path.exists() {
                for child in &mut children {
                    assert!(
                        child.0.try_wait().unwrap().is_none(),
                        "native child setup failed: {} {}",
                        fs::read_to_string(temp.path("A.log")).unwrap(),
                        fs::read_to_string(temp.path("B.log")).unwrap()
                    );
                }
                assert!(Instant::now() < deadline, "native participant deadline");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        if suffix == "ready" {
            fs::write(temp.path("go"), b"both actual source locks held").unwrap();
        }
    }
    let first: serde_json::Value =
        serde_json::from_slice(&fs::read(temp.path("A.result")).unwrap()).unwrap();
    let second: serde_json::Value =
        serde_json::from_slice(&fs::read(temp.path("B.result")).unwrap()).unwrap();
    assert_ne!(first["pid"], second["pid"]);
    assert_eq!(first["contention"], true);
    assert_eq!(second["contention"], true);
    fs::write(temp.path("release"), b"release both owners before retry").unwrap();
    for child in &mut children {
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    assert_eq!(graph(&a), before_a);
    assert_eq!(graph(&b), before_b);
    let (_, mut source) = reopen(&a);
    let copy = copied(source.save_as(&b).unwrap());
    let destination = finish(copy);
    read_assets(&destination, &exact);
    assert_eq!(graph(&a), before_a);
    for (path, bytes) in before_b
        .iter()
        .filter(|(path, _)| path.components().any(|c| c.as_os_str() == "objects"))
    {
        assert_eq!(
            fs::read(temp.root().join(path)).unwrap(),
            *bytes,
            "prior destination dependency was lost: {}",
            path.display()
        );
    }
}
