#![allow(
    clippy::too_many_lines,
    reason = "each witness keeps its complete scenario and assertions together"
)]
use kinewright_core::*;
use kinewright_media::test_support::TempDirectory;
use kinewright_project::{
    bundle::{BundleError, BundleWriteAdmission, LoadedBundle, ProjectBundle},
    bundle_commit::{BundleCommit, CommitOutcome},
    bundle_copy::SessionStore,
    bundle_records::{ObjectRef, RecordKind},
    io_observer,
    namespace::{PortablePath, ProjectNamespace},
    owned_inputs::{OwnedInputEntry, OwnedInputManifest},
    storage_budget::StorageLimits,
};
use std::{fs, path::Path};
fn temp(label: &str) -> TempDirectory {
    TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), label)
}
fn apply(core: &Core, op: Operation) {
    assert!(matches!(
        core.request(Command::Do(op)).unwrap(),
        Event::DocumentChanged { .. }
    ));
}
fn reference(bytes: &[u8], record_kind: RecordKind) -> ObjectRef {
    ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind,
    }
}
fn setup(path: &Path) -> (Core, ProjectBundle, ObjectRef) {
    let mut doc = Document::default();
    doc.tracks.push(Track {
        id: TrackId(1),
        kind: TrackKind::Video,
        sync_lock: true,
        clips: vec![],
    });
    let core = Core::spawn(doc).unwrap();
    let mut bundle = ProjectBundle::activate(path, BundleWriteAdmission::new_document()).unwrap();
    let inputs = bundle.empty_inputs().unwrap();
    let input_ref = inputs.reference.clone();
    bundle.save(&core, &input_ref).unwrap();
    (core, bundle, input_ref)
}
#[test]
fn directory_sync_failure_retains_barriers_and_retry_seals_existing_ancestors() {
    for failure in ["directory_child_sync", "directory_parent_sync"] {
        let temp = temp("r1-directory-retry");
        let path = temp.path("edit.kinewright");
        fs::write(&path, b"old").unwrap();
        let mut owner = ProjectNamespace::acquire(&path).unwrap();
        let ((), bootstrap) = io_observer::observe(|| {
            owner
                .publish_immutable_probe(
                    &PortablePath::new(Path::new("bootstrap")).unwrap(),
                    b"bootstrap",
                )
                .unwrap();
        });
        let name = PortablePath::new(Path::new("studio/objects/exact")).unwrap();
        let (result, failed) = io_observer::observe(|| {
            io_observer::fail_once(failure, || {
                owner.publish_immutable_probe(&name, b"exact object")
            })
        });
        assert!(matches!(result, Err(BundleError::DurabilityUnknown)));
        assert!(
            failed
                .events
                .iter()
                .any(|e| e.operation == "injected_failure")
        );
        assert!(
            owner.unsettled_probe(),
            "failed ancestor barriers were forgotten"
        );
        drop(owner);
        let mut owner = ProjectNamespace::acquire(&path).unwrap();
        let ((), retry) = io_observer::observe(|| {
            owner
                .publish_immutable_probe(&name, b"exact object")
                .unwrap();
            owner.replace_root_probe(b"complete root").unwrap();
        });
        let publish = retry
            .events
            .iter()
            .position(|e| e.operation == "root_rename")
            .unwrap();
        let pending_parent = if failure == "directory_parent_sync" {
            temp.path("edit.kinewright-assets")
        } else {
            temp.path("edit.kinewright-assets/studio")
        };
        assert!(
            retry.events[..publish]
                .iter()
                .any(|e| e.operation == "directory_sync" && e.path == pending_parent),
            "retry forgot the failed directory barrier"
        );
        // Replay the successful retry with the failed prefix through the existing dropped-write model.
        let mut model = super::durable_bundle::CrashFs::new(&path, b"old");
        for event in bootstrap
            .events
            .iter()
            .chain(&failed.events)
            .chain(&retry.events)
        {
            model.apply(event);
        }
        let bytes = std::collections::BTreeMap::from([
            (
                kinewright_media::sha256_bytes(b"complete root"),
                b"complete root".to_vec(),
            ),
            (
                kinewright_media::sha256_bytes(b"exact object"),
                b"exact object".to_vec(),
            ),
        ]);
        assert_eq!(
            model
                .read(
                    &temp.path("edit.kinewright-assets/studio/objects/exact"),
                    &bytes
                )
                .unwrap(),
            b"exact object"
        );
        owner.release().unwrap();
    }
}
#[test]
fn tmpfs_refuses_named_and_session_storage_with_zero_mutations() {
    let root = [Path::new("/dev/shm"), Path::new("/tmp")]
        .into_iter()
        .find(|path| rustix::fs::statfs(*path).is_ok_and(|s| s.f_type == 0x0102_1994));
    let Some(root) = root else {
        panic!("NOT RUN: no accessible tmpfs witness root");
    };
    let temp = TempDirectory::new_in(root, "r1-tmpfs-refusal");
    let ((), trace) = io_observer::observe(|| {
        assert!(matches!(
            ProjectNamespace::acquire(&temp.path("new.kinewright")),
            Err(BundleError::DurabilityUnsupported)
        ));
        assert!(matches!(
            SessionStore::create(temp.root(), StorageLimits::default()),
            Err(BundleError::DurabilityUnsupported)
        ));
    });
    assert!(trace.events.is_empty());
    assert_eq!(fs::read_dir(temp.root()).unwrap().count(), 0);
}
#[test]
fn reused_lut_id_retains_both_content_identities_through_first_save_and_recovery() {
    let temp = temp("r1-lut-reuse");
    let core = Core::spawn(Document::default()).unwrap();
    let a = b"exact LUT A";
    let b = b"different LUT B";
    let lut = |bytes: &[u8]| LutAsset {
        id: LutAssetId(1),
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_len: bytes.len() as u64,
        title: "owned LUT".into(),
        kind: LutAssetKind::Cube3d,
        size: 2,
        domain_min_millionths: [0; 3],
        domain_max_millionths: [1_000_000; 3],
        source: LutAssetSource::Imported {
            source_path: "lost.cube".into(),
        },
    };
    apply(&core, Operation::AddLutAsset { asset: lut(a) });
    apply(
        &core,
        Operation::RemoveLutAsset {
            lut_asset: LutAssetId(1),
        },
    );
    apply(&core, Operation::AddLutAsset { asset: lut(b) });
    let history = core.export_history().unwrap();
    let mut session = SessionStore::create(temp.root(), StorageLimits::default()).unwrap();
    let mut seals = vec![];
    let mut entries = vec![];
    for bytes in [a.as_slice(), b.as_slice()] {
        let object = reference(bytes, RecordKind::OwnedLut);
        seals.push(
            session
                .store_mut()
                .admit_blob(&mut &*bytes, &object, &[])
                .unwrap(),
        );
        entries.push(OwnedInputEntry {
            entry_id: format!("lut:1:{}", object.sha256),
            object,
            dependencies: vec![],
            selected_stream: None,
            time_base: None,
            origin: None,
            profile: None,
        });
    }
    let session_id = session.session_id().clone();
    let epoch = session.store().owner_epoch();
    let manifest = |entries| OwnedInputManifest {
        project_id: None,
        session_id: Some(session_id.clone()),
        owner_epoch: epoch,
        entries,
    };
    let mut wrong = entries[1].clone();
    wrong.entry_id = "lut:1".into();
    let bad = session.admit_inputs(&manifest(vec![wrong])).unwrap();
    let path = temp.path("bad.kinewright");
    assert!(
        session.first_save(&path, &core, &bad.reference).is_err(),
        "missing LUT A must refuse before publication"
    );
    assert!(!path.exists());
    let good_manifest = manifest(entries.clone());
    let good = session.admit_inputs(&good_manifest).unwrap();
    let destination = temp.path("saved.kinewright");
    let copy = session
        .first_save(&destination, &core, &good.reference)
        .unwrap();
    let owner = copy
        .into_destination()
        .unwrap_or_else(|_| panic!("first save pending"));
    drop(owner);
    drop(good);
    drop(bad);
    drop(seals);
    drop(session);
    let loaded = LoadedBundle::read(&destination).unwrap();
    let restored = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&restored).unwrap();
    assert_eq!(restored.export_history().unwrap(), history);
    let mut owner = loaded.into_writer().unwrap();
    let mut wrong_current = entries[0].clone();
    wrong_current.entry_id = "lut:1".into();
    let incomplete = owner
        .admit_inputs(&OwnedInputManifest {
            project_id: Some(owner.project_id().clone()),
            session_id: None,
            owner_epoch: owner.store().owner_epoch(),
            entries: vec![wrong_current],
        })
        .unwrap();
    let root_before = fs::read(&destination).unwrap();
    assert!(
        owner.save(&restored, &incomplete.reference).is_err(),
        "current LUT B borrowed its identity from a historical context instead of its active manifest"
    );
    assert_eq!(fs::read(&destination).unwrap(), root_before);
    for entry in entries {
        let bytes = fs::read(
            destination
                .with_extension("kinewright-assets")
                .join("studio/objects")
                .join(entry.object.sha256),
        )
        .unwrap();
        assert!(bytes == a || bytes == b);
    }
}
#[test]
fn ambiguous_rename_error_requires_reconciliation() {
    let temp = temp("r1-rename-ambiguous");
    let path = temp.path("edit.kinewright");
    let (core, mut bundle, inputs) = setup(&path);
    let before = fs::read(&path).unwrap();
    let outcome = io_observer::fail_once("root_rename_io", || {
        BundleCommit::run(&mut bundle, &core, &inputs)
    });
    assert!(
        matches!(outcome, CommitOutcome::DurabilityUnknown),
        "{outcome:?}"
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    bundle.reconcile().unwrap();
    bundle.save(&core, &inputs).unwrap();
}
#[test]
fn post_ack_gc_failure_never_changes_committed_outcome_and_retries() {
    let temp = temp("r1-post-ack-gc");
    let path = temp.path("edit.kinewright");
    let (core, mut bundle, inputs) = setup(&path);
    for i in 0..4 {
        apply(
            &core,
            Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: i % 2 == 0,
            },
        );
        bundle.save(&core, &inputs).unwrap();
    }
    bundle.collect_after_ack().unwrap();
    let (outcome, trace) = io_observer::observe(|| {
        io_observer::fail_once("gc_before_unlink", || {
            BundleCommit::run(&mut bundle, &core, &inputs)
        })
    });
    let CommitOutcome::Committed(report) = outcome else {
        panic!("a GC failure cannot erase the acknowledged publication: {outcome:?}");
    };
    assert!(
        !trace
            .events
            .iter()
            .any(|e| e.operation == "injected_failure"),
        "GC still blocks the acknowledged path"
    );
    let (result, trace) = io_observer::observe(|| {
        io_observer::fail_once("gc_before_unlink", || bundle.collect_after_ack())
    });
    assert!(result.is_err());
    assert!(
        trace
            .events
            .iter()
            .any(|e| e.operation == "injected_failure")
    );
    assert_eq!(
        LoadedBundle::read(&path)
            .unwrap()
            .root()
            .unwrap()
            .generation,
        report.generation
    );
    bundle.collect_after_ack().unwrap();
    bundle.save(&core, &inputs).unwrap();
}
#[test]
fn realistic_history_saves_every_edit_and_advances_durable_floor() {
    let temp = temp("r1-history-capacity");
    let path = temp.path("edit.kinewright");
    let mut doc = Document::default();
    for id in 1..=2_000 {
        doc.tracks.push(Track {
            id: TrackId(id),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        });
    }
    assert!(serde_json::to_vec(&doc).unwrap().len() >= 50_000);
    let core = Core::spawn(doc).unwrap();
    let mut bundle = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    let inputs = bundle.empty_inputs().unwrap();
    let input_ref = inputs.reference.clone();
    bundle.save(&core, &input_ref).unwrap();
    drop(inputs);
    // Lower the real ingress bound so this witness crosses it repeatedly without filling 32 MiB.
    let limits = StorageLimits {
        branch_history_bytes: 8 << 20,
        ..StorageLimits::default()
    };
    bundle.restrict_limits_probe(limits).unwrap();
    let issuer = bundle.issue_issuer(&core, &input_ref, 1).unwrap();
    for i in 0..110 {
        let Event::QueryResult(QueryResult::Snapshot { revision, .. }) =
            core.request(Command::Query(Query::Snapshot)).unwrap()
        else {
            panic!("snapshot");
        };
        let envelope = kinewright_project::receipts::CommandEnvelope {
            version: 1,
            action: "edit".into(),
            id: kinewright_project::receipts::CommandId {
                issuer: issuer.clone(),
                sequence: i + 1,
            },
            project_id: bundle.project_id().clone(),
            target_branch: bundle.manual_branch_id().clone(),
            owner_epoch: bundle.store().owner_epoch(),
            lease: None,
            expected_head: bundle.manual_head().cloned(),
            expected_revision: revision,
            command: Some(JournalCommand::Do(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: i % 2 == 0,
            })),
            comment_text: None,
            comment_strokes: None,
            evidence: vec![],
            transition: None,
        };
        assert!(
            matches!(
                bundle.command(&envelope, &core, &input_ref, &[]).unwrap(),
                kinewright_project::receipts::ReceiptLookup::Committed(_)
            ),
            "accepted edit was not saved"
        );
        assert_eq!(
            BundleWriteAdmission::read(&path)
                .unwrap()
                .inspection_document()
                .unwrap(),
            &*core.export_history().unwrap().document
        );
    }
    // Full recovery visits every exact receipt checkpoint once after all saves.
    let loaded = LoadedBundle::read(&path).unwrap();
    assert_eq!(
        loaded.admission().inspection_document().unwrap(),
        &*core.export_history().unwrap().document
    );
    let checkpoint = bundle.checkpoint(bundle.manual_branch_id()).unwrap();
    assert!(
        checkpoint.floor_generation > 0,
        "history grew without advancing its durable floor"
    );
    let history = bundle.branch_history(bundle.manual_branch_id()).unwrap();
    assert!(history.floor_offset > 0);
    assert!(!history.cursor.undo.is_empty());
    assert_eq!(history.document, core.export_history().unwrap().document);
}
#[test]
fn legacy_large_document_keeps_its_previous_open_size_contract() {
    let temp = temp("r1-legacy-large");
    let path = temp.path("legacy.kinewright");
    let mut doc = Document::default();
    for id in 1..=200_000 {
        doc.tracks.push(Track {
            id: TrackId(id),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        });
    }
    let bytes = serde_json::to_vec(&doc).unwrap();
    assert!(bytes.len() >= 5_000_000);
    fs::write(&path, &bytes).unwrap();
    assert_eq!(kinewright_project::load_document(&path).unwrap().0, doc);
    assert_eq!(fs::read(&path).unwrap(), bytes);
}
#[test]
fn baseline_change_is_not_a_continuation_of_a_zero_batch_checkpoint() {
    let temp = temp("r1-baseline-continuation");
    let (core, mut bundle, inputs) = setup(&temp.path("edit.kinewright"));
    let mut history = core.export_history().unwrap();
    let mut doc = (*history.baseline).clone();
    doc.resolution = (1280, 720);
    history.baseline = std::sync::Arc::new(doc);
    history.document = history.baseline.clone();
    core.restore_history(history.clone()).unwrap();
    assert!(
        bundle.save(&core, &inputs).is_err(),
        "different baseline accepted as a continuation"
    );
    history.floor_offset = 1;
    core.restore_history(history).unwrap();
    assert!(
        bundle.save(&core, &inputs).is_err(),
        "unproved floor offset bypassed baseline continuity"
    );
}
#[test]
fn save_as_new_destination_syncs_each_child_and_parent_before_root() {
    let temp = temp("r1-destination-ancestors");
    let (core, mut source, _) = setup(&temp.path("source.kinewright"));
    let destination = temp.path("new/parents/copy.kinewright");
    let ((), trace) = io_observer::observe(|| {
        source.save_as(&destination).unwrap();
    });
    let root = trace
        .events
        .iter()
        .position(|e| e.operation == "root_rename")
        .unwrap();
    let mut components = 0;
    for (created, event) in trace
        .events
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.operation, "mkdir" | "directory_create"))
    {
        components += 1;
        for path in [event.path.as_path(), event.path.parent().unwrap()] {
            assert!(
                trace.events[created + 1..root]
                    .iter()
                    .any(|e| e.operation == "directory_sync" && e.path == path),
                "missing child/parent barrier after {}: {}",
                event.path.display(),
                path.display()
            );
        }
    }
    assert!(
        components >= 5,
        "destination ancestor creation was not exercised"
    );
    let loaded = LoadedBundle::read(&destination).unwrap();
    assert_eq!(
        loaded.admission().inspection_document().unwrap(),
        &*core.export_history().unwrap().document
    );
    drop(loaded);
    let retry = temp.path("retry/parents/copy.kinewright");
    assert!(io_observer::fail_once("destination_parent_sync", || source.save_as(&retry)).is_err());
    assert!(!retry.exists());
    source.save_as(&retry).unwrap();
    assert!(LoadedBundle::read(&retry).is_ok());
}
#[test]
fn migration_retries_b3_lost_b5_and_corrupt_backup_without_duplicate_identity() {
    for failure in ["root_before_publish", "root_parent_sync"] {
        let temp = temp("r1-migration-retry");
        let path = temp.path("legacy.kinewright");
        let bytes = kinewright_project::serialize_project_document(&Document::default())
            .unwrap()
            .into_bytes();
        fs::write(&path, &bytes).unwrap();
        let admission = BundleWriteAdmission::read(&path).unwrap();
        let mut bundle = ProjectBundle::activate(&path, admission.clone()).unwrap();
        let id = bundle.project_id().clone();
        let branch = bundle.manual_branch_id().clone();
        let core = Core::spawn(admission.legacy_document().unwrap()).unwrap();
        let inputs = bundle.empty_inputs().unwrap();
        assert!(io_observer::fail_once(failure, || bundle.save(&core, &inputs.reference)).is_err());
        if failure == "root_before_publish" {
            assert_eq!(fs::read(&path).unwrap(), bytes);
            assert!(
                admission.authorize_legacy_write(&path).is_err(),
                "an abandoned activation allowed its original bytes to change"
            );
        }
        drop(inputs);
        drop(bundle);
        let mut retry =
            ProjectBundle::activate(&path, BundleWriteAdmission::read(&path).unwrap()).unwrap();
        assert_eq!(retry.project_id(), &id);
        assert_eq!(retry.manual_branch_id(), &branch);
        let inputs = retry.empty_inputs().unwrap();
        retry.save(&core, &inputs.reference).unwrap();
        retry.collect_after_ack().unwrap();
        let inventory: serde_json::Value = serde_json::from_slice(
            &fs::read(
                path.with_extension("kinewright-assets")
                    .join("studio/recovery/inventory"),
            )
            .unwrap(),
        )
        .unwrap();
        let hash = inventory["pre_migration"]["sha256"].as_str().unwrap();
        assert_eq!(hash, kinewright_media::sha256_bytes(&bytes));
        assert_eq!(
            fs::read(
                path.with_extension("kinewright-assets")
                    .join("studio/objects")
                    .join(hash)
            )
            .unwrap(),
            bytes
        );
        assert!(LoadedBundle::read(&path).is_ok());
    }
    {
        let temp = temp("r1-migration-backup-failure");
        let path = temp.path("legacy.kinewright");
        let bytes = kinewright_project::serialize_project_document(&Document::default())
            .unwrap()
            .into_bytes();
        fs::write(&path, &bytes).unwrap();
        assert!(
            io_observer::fail_once("object_file_sync", || ProjectBundle::activate(
                &path,
                BundleWriteAdmission::read(&path).unwrap()
            ))
            .is_err()
        );
        assert_eq!(
            fs::read(&path).unwrap(),
            bytes,
            "failed backup changed the only original"
        );
        assert!(ProjectBundle::activate(&path, BundleWriteAdmission::read(&path).unwrap()).is_ok());
    }
    let temp = temp("r1-migration-damage");
    let path = temp.path("legacy.kinewright");
    let bytes = kinewright_project::serialize_project_document(&Document::default())
        .unwrap()
        .into_bytes();
    fs::write(&path, &bytes).unwrap();
    let bundle =
        ProjectBundle::activate(&path, BundleWriteAdmission::read(&path).unwrap()).unwrap();
    drop(bundle);
    let backup = path
        .with_extension("kinewright-assets")
        .join("studio/objects")
        .join(kinewright_media::sha256_bytes(&bytes));
    fs::write(&backup, b"corrupt backup").unwrap();
    assert!(ProjectBundle::activate(&path, BundleWriteAdmission::read(&path).unwrap()).is_err());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(fs::read(&backup).unwrap(), b"corrupt backup");
}
#[test]
fn verified_reopen_retires_unreachable_generated_orphans_and_their_charges() {
    let temp = temp("r1-orphan-retirement");
    let path = temp.path("edit.kinewright");
    let (_, bundle, _) = setup(&path);
    drop(bundle);
    let bytes = b"orphan from an interrupted copy";
    let orphan = path
        .with_extension("kinewright-assets")
        .join("studio/objects")
        .join(kinewright_media::sha256_bytes(bytes));
    fs::write(&orphan, bytes).unwrap();
    let mut reopened = LoadedBundle::read(&path).unwrap().into_writer().unwrap();
    let before = reopened.store().usage().unwrap();
    assert_eq!(before.unclassified_bytes, bytes.len() as u64);
    reopened.collect_after_ack().unwrap();
    assert!(
        !orphan.exists(),
        "unreachable generated object remained charged forever"
    );
    assert_eq!(reopened.store().usage().unwrap().unclassified_bytes, 0);
    assert!(!reopened.store().usage().unwrap().damaged_accounting);
}

#[test]
fn verified_reopen_retires_over_quota_orphan_before_admission() {
    let temp = temp("r1-orphan-quota");
    let path = temp.path("edit.kinewright");
    let (_, bundle, _) = setup(&path);
    drop(bundle);
    let before = fs::read(&path).unwrap();
    let orphan = path
        .with_extension("kinewright-assets")
        .join("studio/objects")
        .join(kinewright_media::sha256_bytes(
            b"interrupted oversized copy",
        ));
    let file = fs::File::create(&orphan).unwrap();
    // Logical-length ingress only. This sparse file does not fill the 20 GiB allowance.
    file.set_len(StorageLimits::default().owned_bytes + 1)
        .unwrap();
    drop(file);
    let mut writer = LoadedBundle::read(&path).unwrap().into_writer().unwrap();
    assert!(
        !orphan.exists(),
        "orphan quota prevented verified recovery cleanup"
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    writer.collect_after_ack().unwrap();
    assert_eq!(writer.store().usage().unwrap().unclassified_bytes, 0);
}
