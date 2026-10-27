use super::Ja1EditingSession;
use kinewright_core::{
    AttachmentState, Command, Core, Document, Event, Operation, Track, TrackId, TrackKind,
};
use kinewright_media::test_support::TempDirectory;
use kinewright_project::{
    bundle::LoadedBundle, bundle_copy::CopyStatus, io_observer, storage_budget::StorageLimits,
};
use std::path::Path;
fn temp() -> TempDirectory {
    TempDirectory::new_in(super::durable_test_root(), "s2-c5-rescue")
}
fn document() -> Document {
    Document {
        tracks: (1..=200)
            .map(|id| Track {
                id: TrackId(id),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            })
            .collect(),
        ..Document::default()
    }
}
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
fn grow(core: &Core) {
    for n in 0..40 {
        edit(core, n % 2 == 0);
    }
}
fn floor_limits() -> StorageLimits {
    StorageLimits {
        branch_history_bytes: 1 << 20,
        ..StorageLimits::default()
    }
}
fn rescue(
    session: &mut Ja1EditingSession,
    path: &Path,
    fault: Option<&'static str>,
) -> kinewright_project::bundle_rescue::RescueOutcome {
    rescue_profile(session, path, fault, StorageLimits::default())
}
fn rescue_profile(
    session: &mut Ja1EditingSession,
    path: &Path,
    fault: Option<&'static str>,
    profile: StorageLimits,
) -> kinewright_project::bundle_rescue::RescueOutcome {
    let offer = session.probe_active().unwrap().unwrap();
    let capture = session.prepare_rescue(&offer).unwrap().unwrap();
    let selection = session.preview_rescue(capture, profile).unwrap();
    let mut consent = selection.consent();
    if selection.removed_undo() > 0 {
        selection.consent_trim(&mut consent).unwrap();
    }
    if !selection.omissions().is_empty() {
        selection.consent_omission(&mut consent).unwrap();
    }
    let destination = session.inspect_rescue_destination(path).unwrap();
    if let Some(fault) = fault {
        io_observer::fail_once(fault, || session.rescue(selection, &consent, &destination)).unwrap()
    } else {
        session.rescue(selection, &consent, &destination).unwrap()
    }
}
#[test]
pub(super) fn w01_first_save_rescue_is_independent() {
    use kinewright_core::{LutAsset, LutAssetId, LutAssetKind, LutAssetSource};
    use kinewright_project::{
        bundle_records::{ObjectRef, RecordKind},
        owned_inputs::{OwnedInputEntry, OwnedInputManifest},
    };
    let temp = temp();
    let a = temp.path("drive/a.kinewright");
    let b = temp.path("b.kinewright");
    let core = Core::spawn(document()).unwrap();
    edit(&core, false);
    let mut session = Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
    let lut = b"TITLE \"Owned rescue LUT\"\nLUT_3D_SIZE 2\n0 0 0\n0 0 1\n0 1 0\n0 1 1\n1 0 0\n1 0 1\n1 1 0\n1 1 1\n";
    let object = ObjectRef {
        sha256: kinewright_media::sha256_bytes(lut),
        byte_length: lut.len() as u64,
        record_kind: RecordKind::OwnedLut,
    };
    let asset = LutAsset {
        id: LutAssetId(1),
        sha256: object.sha256.clone(),
        title: "owned LUT".into(),
        kind: LutAssetKind::Cube3d,
        size: 2,
        byte_len: object.byte_length,
        domain_min_millionths: [0; 3],
        domain_max_millionths: [1_000_000; 3],
        source: LutAssetSource::Imported {
            source_path: "/unavailable-original.cube".into(),
        },
    };
    assert!(matches!(
        core.request(Command::Do(Operation::AddLutAsset { asset }))
            .unwrap(),
        Event::DocumentChanged { .. }
    ));
    let source = session.source_mut().unwrap();
    let sealed = source
        .store_mut()
        .admit_blob(&mut &lut[..], &object, &[])
        .unwrap();
    let entries = vec![OwnedInputEntry {
        entry_id: format!("lut:1:{}:{}", object.sha256, object.byte_length),
        object: object.clone(),
        dependencies: vec![],
        selected_stream: None,
        time_base: None,
        origin: None,
        profile: Some("original LUT profile".into()),
    }];
    let manifest = source
        .admit_inputs(&OwnedInputManifest {
            project_id: None,
            session_id: Some(source.session_id().clone()),
            owner_epoch: source.store().owner_epoch(),
            entries: entries.clone(),
        })
        .unwrap()
        .reference
        .clone();
    drop(sealed);
    session.set_unsaved_inputs(manifest).unwrap();
    assert!(matches!(
        io_observer::fail_once("root_parent_sync", || session
            .first_save(&a, StorageLimits::default()))
        .unwrap(),
        CopyStatus::DurabilityUnknown
    ));
    let original_project = LoadedBundle::read(&a)
        .unwrap()
        .root()
        .unwrap()
        .project_id
        .clone();
    std::fs::rename(temp.path("drive"), temp.path("hidden-drive")).unwrap();
    let expected = core.export_history().unwrap();
    let outcome = rescue(&mut session, &b, None);
    assert_ne!(
        LoadedBundle::read(&b).unwrap().root().unwrap().project_id,
        original_project
    );
    assert!(outcome.activated, "{outcome:?}");
    assert_eq!(core.attachment_state().unwrap(), AttachmentState::Sealed);
    assert_ne!(core.actor_identity(), session.core().actor_identity());
    assert_eq!(session.core().export_history().unwrap(), expected);
    let root = LoadedBundle::read(&b).unwrap().root().unwrap().clone();
    let copied_inputs = session
        .store()
        .unwrap()
        .read_record::<OwnedInputManifest>(&root.input_manifest_ref)
        .unwrap();
    assert_eq!(copied_inputs.entries, entries);
    assert_eq!(
        std::fs::read(temp.path(&format!(
            "b.kinewright-assets/studio/objects/{}",
            object.sha256
        )))
        .unwrap(),
        lut
    );
    drop(copied_inputs);
    assert_eq!(session.rescue_slots(), 1);
    for n in 0..3 {
        edit(session.core(), n % 2 == 0);
        session.save_active().unwrap();
    }
    let later = session.core().export_history().unwrap();
    assert_eq!(session.rescue_slots(), 1);
    drop(session);
    drop(core);
    std::fs::remove_dir_all(temp.path("hidden-drive")).unwrap();
    let loaded = LoadedBundle::read(&b).unwrap();
    let fresh = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&fresh).unwrap();
    assert_eq!(fresh.export_history().unwrap(), later);
}
#[test]
pub(super) fn w16_first_save_a_writer_and_later_floor_continue() {
    for landed in [false, true] {
        let temp = temp();
        let a = temp.path("drive/a.kinewright");
        let b = temp.path("b.kinewright");
        let core = Core::spawn(document()).unwrap();
        grow(&core);
        let mut session =
            Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
        assert!(matches!(
            io_observer::fail_once(
                if landed {
                    "root_parent_sync"
                } else {
                    "root_rename_io"
                },
                || session.first_save(&a, floor_limits())
            )
            .unwrap(),
            CopyStatus::DurabilityUnknown
        ));
        let chosen = kinewright_project::bundle_commit::bound_history_probe(
            &core.export_history().unwrap(),
            &floor_limits(),
        )
        .unwrap();
        assert!(chosen.floor_offset > 0);
        let ticket = core.pending_history_save().unwrap().unwrap();
        std::fs::rename(temp.path("drive"), temp.path("hidden-drive")).unwrap();
        let outcome = rescue(&mut session, &b, Some("root_parent_sync"));
        assert!(!outcome.activated);
        assert!(matches!(outcome.status, CopyStatus::DurabilityUnknown));
        assert_eq!(core.attachment_state().unwrap(), AttachmentState::Live);
        edit(&core, true);
        let history = core.export_history().unwrap();
        session.reconcile_retained_destination(&b).unwrap();
        assert_eq!(core.export_history().unwrap(), history);
        std::fs::rename(temp.path("hidden-drive"), temp.path("drive")).unwrap();
        session.reconcile_active().unwrap();
        let oracle = if landed {
            kinewright_core::HistoryTransition::floor(
                history.clone(),
                usize::try_from(chosen.floor_offset).unwrap(),
            )
            .unwrap()
            .after()
            .clone()
        } else {
            history.clone()
        };
        assert_eq!(core.export_history().unwrap(), oracle);
        assert!(core.pending_history_save().unwrap().is_none());
        assert_eq!(ticket.actor, core.actor_identity());
        session.save_active().unwrap();
        assert!(!session.dirty().unwrap());
        assert_eq!(session.core().actor_identity(), core.actor_identity());
        let saved = core.export_history().unwrap();
        let reopened = LoadedBundle::read(&a).unwrap();
        let fresh = Core::spawn(Document::default()).unwrap();
        reopened.restore_manual(&fresh).unwrap();
        assert_eq!(fresh.export_history().unwrap(), saved);
    }
}
#[test]
pub(super) fn w16_repeated_active_b_attempts_settle_only_their_tickets() {
    let temp = temp();
    let a = temp.path("drive/a.kinewright");
    let b = temp.path("bdrive/b.kinewright");
    let core = Core::spawn(document()).unwrap();
    grow(&core);
    let mut session = Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
    io_observer::fail_once("root_parent_sync", || {
        session.first_save(&a, floor_limits())
    })
    .unwrap();
    std::fs::rename(temp.path("drive"), temp.path("hidden-a")).unwrap();
    assert!(rescue_profile(&mut session, &b, None, floor_limits()).activated);
    for (index, fault) in ["root_before_publish", "root_parent_sync"]
        .into_iter()
        .enumerate()
    {
        grow(session.core());
        let initial = session.core().export_history().unwrap();
        let chosen =
            kinewright_project::bundle_commit::bound_history_probe(&initial, &floor_limits())
                .unwrap();
        assert!(chosen.floor_offset > initial.floor_offset);
        assert!(io_observer::fail_once("root_parent_sync", || session.save_active()).is_err());
        let ticket = session.core().pending_history_save().unwrap().unwrap();
        std::fs::rename(temp.path("bdrive"), temp.path("hidden-b")).unwrap();
        let offer = session.probe_active().unwrap().unwrap();
        let capture = session.prepare_rescue(&offer).unwrap().unwrap();
        let selection = session
            .preview_rescue(capture, StorageLimits::default())
            .unwrap();
        let consent = selection.consent();
        let destination = session
            .inspect_rescue_destination(&temp.path(&format!("c{index}.kinewright")))
            .unwrap();
        let outcome =
            io_observer::fail_once(fault, || session.rescue(selection, &consent, &destination));
        if index == 0 {
            assert!(
                outcome.is_err()
                    || outcome.as_ref().is_ok_and(
                        |o| !o.activated && !matches!(o.status, CopyStatus::DurabilityUnknown)
                    )
            );
        } else {
            assert!(!outcome.unwrap().activated);
        }
        edit(session.core(), index % 2 == 0);
        let before = session.core().export_history().unwrap();
        let oracle = kinewright_core::HistoryTransition::floor(
            before.clone(),
            usize::try_from(chosen.floor_offset - initial.floor_offset).unwrap(),
        )
        .unwrap()
        .after()
        .clone();
        std::fs::rename(temp.path("hidden-b"), temp.path("bdrive")).unwrap();
        session.reconcile_active().unwrap();
        assert_eq!(session.core().export_history().unwrap(), oracle);
        assert!(session.core().pending_history_save().unwrap().is_none());
        assert_eq!(ticket.actor, session.core().actor_identity());
        session.save_active().unwrap();
        if index == 1 {
            let history = session.core().export_history().unwrap();
            session
                .reconcile_retained_destination(&temp.path("c1.kinewright"))
                .unwrap();
            assert_eq!(session.core().export_history().unwrap(), history);
        }
    }
    assert_eq!(session.rescue_slots(), 4);
    edit(session.core(), false);
    session.save_active().unwrap();
}

#[test]
pub(super) fn w10_pending_command_exclusion_and_original_id_lookup() {
    use kinewright_project::{
        bundle::{BundleWriteAdmission, ProjectBundle},
        receipts::{CommandEnvelope, CommandId, ReceiptLookup},
    };
    for (landed, immediate) in [(false, false), (true, false), (false, true), (true, true)] {
        let temp = temp();
        let a = temp.path("a.kinewright");
        let core = Core::spawn(document()).unwrap();
        let mut bundle = ProjectBundle::activate(&a, BundleWriteAdmission::new_document()).unwrap();
        let input = bundle.empty_inputs().unwrap().reference.clone();
        bundle.save(&core, &input).unwrap();
        let issuer = bundle.issue_issuer(&core, &input, 1).unwrap();
        let Event::QueryResult(kinewright_core::QueryResult::Snapshot { revision, .. }) = core
            .request(Command::Query(kinewright_core::Query::Snapshot))
            .unwrap()
        else {
            panic!("snapshot");
        };
        let envelope = CommandEnvelope {
            switch: None,
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
            expected_revision: revision,
            command: Some(kinewright_core::JournalCommand::Do(
                Operation::SetTrackSyncLock {
                    track: TrackId(1),
                    locked: false,
                },
            )),
            comment_text: None,
            comment_strokes: None,
            evidence: vec![],
            transition: None,
        };
        assert!(
            io_observer::fail_once(
                if landed {
                    "root_parent_sync"
                } else {
                    "root_rename_io"
                },
                || bundle.command(&envelope, &core, &input, &[])
            )
            .is_err()
        );
        assert!(core.pending_history_save().unwrap().is_none());
        let original = core.export_history().unwrap();
        let mut session = Ja1EditingSession::from_bundle(bundle, &core, input).unwrap();
        let parent = temp.root().to_path_buf();
        let hidden = parent.with_extension("pending-hidden");
        std::fs::rename(&parent, &hidden).unwrap();
        let bdir = parent.parent().unwrap().join(format!(
            "c5-pending-{}-{landed}-{immediate}",
            std::process::id()
        ));
        std::fs::create_dir_all(&bdir).unwrap();
        let b = bdir.join("b.kinewright");
        let outcome = rescue(
            &mut session,
            &b,
            if immediate {
                None
            } else {
                Some("root_parent_sync")
            },
        );
        assert_eq!(outcome.activated, immediate);
        let active = session.core().clone();
        assert_eq!(active.export_history().unwrap(), original);
        assert!(matches!(
            active
                .request(Command::Do(Operation::SetTrackSyncLock {
                    track: TrackId(2),
                    locked: false
                }))
                .unwrap(),
            Event::DocumentChanged { .. }
        ));
        let newer = active.export_history().unwrap();
        let dirty = session.dirty().unwrap();
        std::fs::rename(&hidden, &parent).unwrap();
        if immediate {
            session.reconcile_retained_destination(&a).unwrap();
        } else {
            session.reconcile_active().unwrap();
        }
        assert_eq!(active.export_history().unwrap(), newer);
        assert_eq!(session.dirty().unwrap(), dirty);
        assert_eq!(
            core.attachment_state().unwrap(),
            if immediate {
                AttachmentState::Sealed
            } else {
                AttachmentState::Live
            }
        );
        if !immediate {
            session.reconcile_retained_destination(&b).unwrap();
            assert_eq!(active.export_history().unwrap(), newer);
        }
        drop(session);
        drop(active);
        drop(core);
        let loaded = LoadedBundle::read(&b).unwrap();
        let fresh = Core::spawn(Document::default()).unwrap();
        loaded.restore_manual(&fresh).unwrap();
        assert_eq!(fresh.export_history().unwrap(), original);
        let owner = loaded.into_writer().unwrap();
        assert_eq!(
            owner.receipt_lookup(&envelope).unwrap(),
            ReceiptLookup::UnresolvedOrigin
        );
        let mut changed = envelope.clone();
        changed.expected_head = None;
        assert_eq!(
            owner.receipt_lookup(&changed).unwrap(),
            ReceiptLookup::IdPayloadConflict
        );
        drop(owner);
        std::fs::remove_dir_all(&bdir).unwrap();
    }
}

#[test]
pub(super) fn w13_one_level_rescue_recopy_and_explicit_ack_retirement() {
    use kinewright_project::bundle::{BundleWriteAdmission, ProjectBundle};
    let temp = temp();
    let a = temp.path("drive/a.kinewright");
    let b = temp.path("bdrive/b.kinewright");
    let c = temp.path("c.kinewright");
    let core = Core::spawn(document()).unwrap();
    std::fs::create_dir_all(a.parent().unwrap()).unwrap();
    let mut bundle = ProjectBundle::activate(&a, BundleWriteAdmission::new_document()).unwrap();
    let inputs = bundle.empty_inputs().unwrap().reference.clone();
    let bytes = b"independent evidence survives rescue and explicit ack retirement";
    let evidence = kinewright_project::bundle_records::ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: kinewright_project::bundle_records::RecordKind::OwnedEvidence,
    };
    let admitted = bundle
        .store_mut()
        .unwrap()
        .admit_blob(&mut &bytes[..], &evidence, &[])
        .unwrap();
    bundle.retain_evidence(&evidence).unwrap();
    drop(admitted);
    let mut session = Ja1EditingSession::from_bundle(bundle, &core, inputs).unwrap();
    session.save_active().unwrap();
    for lock in [false, true] {
        edit(&core, lock);
        session.save_active().unwrap();
    }
    edit(&core, false);
    assert!(io_observer::fail_once("root_parent_sync", || session.save_active()).is_err());
    std::fs::rename(temp.path("drive"), temp.path("hidden-a")).unwrap();
    assert!(rescue(&mut session, &b, None).activated);
    let points = session.list_recovery_points().unwrap();
    assert_eq!(
        points.len(),
        3,
        "dedicated acknowledged root plus both automatic predecessors"
    );
    for point in &points {
        let kinewright_project::bundle_recovery::RecoverySelection::Checkpoint { root, .. } = point
        else {
            panic!("checkpoint")
        };
        assert!(session.store().unwrap().verify(root).is_ok());
    }
    edit(session.core(), true);
    assert!(io_observer::fail_once("root_parent_sync", || session.save_active()).is_err());
    std::fs::rename(temp.path("bdrive"), temp.path("hidden-b")).unwrap();
    assert!(rescue(&mut session, &c, None).activated);
    let records = LoadedBundle::read(&c).unwrap();
    let writer_manifest = records.root().unwrap().store_manifest_ref.clone();
    drop(records);
    let manifest = session
        .store()
        .unwrap()
        .read_record::<kinewright_project::store::StoreManifest>(&writer_manifest)
        .unwrap();
    let (top_reference, top) = manifest
        .durable_pins
        .iter()
        .find_map(|r| {
            session
                .store()
                .unwrap()
                .read_record::<kinewright_project::rescue_record::RescueCopyV1>(r)
                .ok()
                .map(|v| (r.clone(), (*v).clone()))
        })
        .unwrap();
    let ack = top.acknowledged_root.clone().unwrap();
    assert_eq!(
        manifest.dependencies.get(&top_reference.sha256),
        Some(&vec![ack.clone()]),
        "published rescue record needs its dedicated acknowledged-root edge"
    );
    assert!(top.recovery_origin.as_ref().unwrap().rebound);
    let independent = session
        .store()
        .unwrap()
        .pin(std::slice::from_ref(&ack))
        .unwrap();
    let root = session.root_sha256().unwrap();
    assert!(
        session
            .retire_rescue_recovery_point(&ack, &root, false)
            .is_err()
    );
    assert_eq!(session.root_sha256().unwrap(), root);
    session
        .retire_rescue_recovery_point(&ack, &root, true)
        .unwrap();
    for n in 0..4 {
        edit(session.core(), n % 2 == 0);
        session.save_active().unwrap();
    }
    assert!(session.store().unwrap().verify(&ack).is_ok());
    assert_eq!(
        &*session.store().unwrap().read_evidence(&evidence).unwrap(),
        bytes
    );
    drop(independent);
    let expected = session.core().export_history().unwrap();
    drop(manifest);
    drop(session);
    drop(core);
    let mut writer = LoadedBundle::read(&c).unwrap().into_writer().unwrap();
    writer.collect_after_ack().unwrap();
    assert_eq!(&*writer.store().read_evidence(&evidence).unwrap(), bytes);
    assert!(
        writer
            .rescue_records()
            .unwrap()
            .iter()
            .all(|r| r.acknowledged_root.is_none())
    );
    drop(writer);
    let loaded = LoadedBundle::read(&c).unwrap();
    let core = Core::spawn(Document::default()).unwrap();
    loaded.restore_manual(&core).unwrap();
    assert_eq!(core.export_history().unwrap(), expected);
}

#[test]
pub(super) fn w13_omission_only_precedes_trim() {
    use kinewright_project::{
        bundle::{BundleWriteAdmission, ProjectBundle},
        bundle_recovery::RecoverySelection,
    };
    let temp = temp();
    let a = temp.path("drive/a.kinewright");
    std::fs::create_dir_all(a.parent().unwrap()).unwrap();
    let b = temp.path("b.kinewright");
    let core = Core::spawn(document()).unwrap();
    let mut bundle = ProjectBundle::activate(&a, BundleWriteAdmission::new_document()).unwrap();
    let input = bundle.empty_inputs().unwrap().reference.clone();
    bundle.save(&core, &input).unwrap();
    for n in 0..3 {
        edit(&core, n % 2 == 0);
        bundle.save(&core, &input).unwrap();
    }
    let mut session = Ja1EditingSession::from_bundle(bundle, &core, input).unwrap();
    session.save_active().unwrap();
    let RecoverySelection::Checkpoint { root: missing, .. } = session
        .list_recovery_points()
        .unwrap()
        .last()
        .unwrap()
        .clone()
    else {
        panic!("checkpoint");
    };
    edit(&core, false);
    assert!(io_observer::fail_once("root_rename_io", || session.save_active()).is_err());
    std::fs::remove_file(temp.path(&format!(
        "drive/a.kinewright-assets/studio/objects/{}",
        missing.sha256
    )))
    .unwrap();
    std::fs::rename(temp.path("drive"), temp.path("hidden-a")).unwrap();
    let prepare = |session: &mut Ja1EditingSession| {
        let offer = session.probe_active().unwrap().unwrap();
        let capture = session.prepare_rescue(&offer).unwrap().unwrap();
        session
            .preview_rescue(capture, StorageLimits::default())
            .unwrap()
    };
    let selection = prepare(&mut session);
    assert_eq!(selection.removed_undo(), 0);
    assert_eq!(selection.omissions().len(), 1);
    assert_eq!(selection.omissions()[0].root, missing);
    let consent = selection.consent();
    let inspection = session.inspect_rescue_destination(&b).unwrap();
    let (result, trace) = io_observer::observe(|| session.rescue(selection, &consent, &inspection));
    assert!(result.is_err());
    assert_eq!(session.rescue_slots(), 0);
    assert!(!b.exists());
    assert!(
        !trace
            .events
            .iter()
            .any(|e| e.operation != "rescue_separation_inspect" && e.path.starts_with(&b))
    );
    let selection = prepare(&mut session);
    let mut consent = selection.consent();
    selection.consent_omission(&mut consent).unwrap();
    let expected = core.export_history().unwrap();
    assert!(
        session
            .rescue(selection, &consent, &inspection)
            .unwrap()
            .activated
    );
    assert_eq!(session.core().export_history().unwrap(), expected);
    drop(session);
    drop(core);
    let loaded = LoadedBundle::read(&b).unwrap();
    let owner = loaded.into_writer().unwrap();
    let records = owner.rescue_records().unwrap();
    assert!(records[0].acknowledged_root.is_some());
    assert_eq!(records[0].omissions[0].root, missing);
    assert!(
        !temp
            .path(&format!(
                "b.kinewright-assets/studio/objects/{}",
                missing.sha256
            ))
            .exists()
    );
}

#[test]
pub(super) fn w13_trim_then_separate_omission_recomputes_closure() {
    use kinewright_core::{LutAsset, LutAssetId, LutAssetKind, LutAssetSource};
    use kinewright_project::{
        bundle::{BundleWriteAdmission, ProjectBundle},
        bundle_records::{ObjectRef, RecordKind},
        owned_inputs::{OwnedInputEntry, OwnedInputManifest},
    };
    let lut=b"TITLE \"Exact LUT\"\nLUT_3D_SIZE 2\n0 0 0\n0 0 1\n0 1 0\n0 1 1\n1 0 0\n1 0 1\n1 1 0\n1 1 1\n";
    for independent in [false, true] {
        let temp = temp();
        let a = temp.path("drive/a.kinewright");
        std::fs::create_dir_all(a.parent().unwrap()).unwrap();
        let b = temp.path("b.kinewright");
        let core = Core::spawn(document()).unwrap();
        let mut bundle = ProjectBundle::activate(&a, BundleWriteAdmission::new_document()).unwrap();
        let empty = bundle.empty_inputs().unwrap().reference.clone();
        bundle.save(&core, &empty).unwrap();
        let object = ObjectRef {
            sha256: kinewright_media::sha256_bytes(lut),
            byte_length: lut.len() as u64,
            record_kind: RecordKind::OwnedLut,
        };
        let sealed = bundle
            .store_mut()
            .unwrap()
            .admit_blob(&mut &lut[..], &object, &[])
            .unwrap();
        let inputs = bundle
            .admit_inputs(&OwnedInputManifest {
                project_id: Some(bundle.project_id().clone()),
                session_id: None,
                owner_epoch: bundle.store().owner_epoch(),
                entries: vec![OwnedInputEntry {
                    entry_id: format!("lut:1:{}:{}", object.sha256, object.byte_length),
                    object: object.clone(),
                    dependencies: vec![],
                    selected_stream: None,
                    time_base: None,
                    origin: None,
                    profile: None,
                }],
            })
            .unwrap()
            .reference
            .clone();
        let asset = LutAsset {
            id: LutAssetId(1),
            sha256: object.sha256.clone(),
            title: "old Undo LUT".into(),
            kind: LutAssetKind::Cube3d,
            size: 2,
            byte_len: object.byte_length,
            domain_min_millionths: [0; 3],
            domain_max_millionths: [1_000_000; 3],
            source: LutAssetSource::Imported {
                source_path: "/unavailable.cube".into(),
            },
        };
        assert!(matches!(
            core.request(Command::Do(Operation::AddLutAsset { asset }))
                .unwrap(),
            Event::DocumentChanged { .. }
        ));
        bundle.save(&core, &inputs).unwrap();
        edit(&core, false);
        bundle.save(&core, &inputs).unwrap();
        assert!(matches!(
            core.request(Command::Do(Operation::RemoveLutAsset {
                lut_asset: LutAssetId(1)
            }))
            .unwrap(),
            Event::DocumentChanged { .. }
        ));
        bundle.save(&core, &empty).unwrap();
        if independent {
            let evidence_bytes = b"independent LUT evidence";
            let evidence = ObjectRef {
                sha256: kinewright_media::sha256_bytes(evidence_bytes),
                byte_length: evidence_bytes.len() as u64,
                record_kind: RecordKind::OwnedEvidence,
            };
            let held = bundle
                .store_mut()
                .unwrap()
                .admit_blob(
                    &mut &evidence_bytes[..],
                    &evidence,
                    std::slice::from_ref(&object),
                )
                .unwrap();
            bundle.retain_evidence(&evidence).unwrap();
            drop(held);
            bundle.save(&core, &empty).unwrap();
        }
        let mut session = Ja1EditingSession::from_bundle(bundle, &core, empty).unwrap();
        session.save_active().unwrap();
        edit(&core, true);
        assert!(io_observer::fail_once("root_parent_sync", || session.save_active()).is_err());
        drop(sealed);
        std::fs::remove_file(temp.path(&format!(
            "drive/a.kinewright-assets/studio/objects/{}",
            object.sha256
        )))
        .unwrap();
        std::fs::rename(temp.path("drive"), temp.path("hidden-a")).unwrap();
        let offer = session.probe_active().unwrap().unwrap();
        let capture = session.prepare_rescue(&offer);
        if independent {
            assert!(capture.is_err());
            assert!(!b.exists());
            continue;
        }
        let selection = session
            .preview_rescue(capture.unwrap().unwrap(), StorageLimits::default())
            .unwrap();
        assert!(selection.removed_undo() > 0);
        assert!(
            selection
                .omissions()
                .iter()
                .any(|o| o.role == "acknowledged_root")
        );
        let before = core.export_history().unwrap();
        let mut consent = selection.consent();
        assert!(selection.consent_omission(&mut consent).is_err());
        selection.consent_trim(&mut consent).unwrap();
        let inspection = session.inspect_rescue_destination(&b).unwrap();
        assert!(session.rescue(selection, &consent, &inspection).is_err());
        assert!(!b.exists());
        let offer = session.probe_active().unwrap().unwrap();
        let capture = session.prepare_rescue(&offer).unwrap().unwrap();
        let selection = session
            .preview_rescue(capture, StorageLimits::default())
            .unwrap();
        let mut consent = selection.consent();
        selection.consent_trim(&mut consent).unwrap();
        selection.consent_omission(&mut consent).unwrap();
        let inspection = session.inspect_rescue_destination(&b).unwrap();
        assert!(
            session
                .rescue(selection, &consent, &inspection)
                .unwrap()
                .activated
        );
        assert_eq!(core.export_history().unwrap(), before);
        let selected = session.core().export_history().unwrap();
        assert!(selected.baseline.lut_assets.is_empty());
        assert_eq!(selected.document, before.document);
        assert_eq!(selected.cursor.redo, before.cursor.redo);
        assert!(selected.cursor.undo.len() < before.cursor.undo.len());
        drop(session);
        drop(core);
        let loaded = LoadedBundle::read(&b).unwrap();
        let fresh = Core::spawn(Document::default()).unwrap();
        loaded.restore_manual(&fresh).unwrap();
        assert_eq!(fresh.export_history().unwrap(), selected);
        assert!(
            !temp
                .path(&format!(
                    "b.kinewright-assets/studio/objects/{}",
                    object.sha256
                ))
                .exists()
        );
    }
}

#[test]
pub(super) fn w04_oldest_first_trim_preserves_redo_gesture_and_inputs() {
    for gesture in [false, true] {
        let temp = temp();
        let a = temp.path("drive/a.kinewright");
        let b = temp.path("b.kinewright");
        let core = Core::spawn(document()).unwrap();
        grow(&core);
        if gesture {
            assert!(matches!(
                core.request(Command::DoBatchCoalesced {
                    operations: vec![Operation::SetTrackSyncLock {
                        track: TrackId(1),
                        locked: false
                    }],
                    coalesce_key: "retained-drag".into()
                })
                .unwrap(),
                Event::DocumentChanged { .. }
            ));
        } else {
            assert!(matches!(
                core.request(Command::Undo).unwrap(),
                Event::DocumentChanged { .. }
            ));
        }
        let before = core.export_history().unwrap();
        let mut session =
            Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
        io_observer::fail_once("root_parent_sync", || {
            session.first_save(&a, StorageLimits::default())
        })
        .unwrap();
        std::fs::rename(temp.path("drive"), temp.path("hidden-a")).unwrap();
        let offer = session.probe_active().unwrap().unwrap();
        let capture = session.prepare_rescue(&offer).unwrap().unwrap();
        let profile = floor_limits();
        let selection = session.preview_rescue(capture, profile.clone()).unwrap();
        assert!(selection.removed_undo() > 0);
        assert!(selection.omissions().is_empty());
        let ordinary = (0..=before.batches.len())
            .find_map(|cut| {
                let candidate = if cut == 0 {
                    before.clone()
                } else {
                    kinewright_core::HistoryTransition::floor(before.clone(), cut)
                        .ok()?
                        .after()
                        .clone()
                };
                let cost =
                    kinewright_project::bundle_commit::history_cost_probe(&candidate, &profile)
                        .ok()?;
                (candidate.encoded_bytes().ok()? <= profile.branch_history_bytes
                    && cost <= profile.branch_history_bytes)
                    .then_some(candidate)
            })
            .expect("independent earliest legal fitting floor");
        let missing = selection.consent();
        let inspection = session.inspect_rescue_destination(&b).unwrap();
        assert!(session.rescue(selection, &missing, &inspection).is_err());
        assert!(!b.exists());
        let offer = session.probe_active().unwrap().unwrap();
        let capture = session.prepare_rescue(&offer).unwrap().unwrap();
        let selection = session.preview_rescue(capture, profile.clone()).unwrap();
        let mut consent = selection.consent();
        selection.consent_trim(&mut consent).unwrap();
        let inspection = session.inspect_rescue_destination(&b).unwrap();
        assert!(
            session
                .rescue(selection, &consent, &inspection)
                .unwrap()
                .activated
        );
        assert_eq!(core.export_history().unwrap(), before);
        assert_eq!(session.core().export_history().unwrap(), ordinary);
        assert_eq!(
            session.core().export_history().unwrap().cursor.redo,
            before.cursor.redo
        );
        let selected = session.core().export_history().unwrap();
        drop(session);
        drop(core);
        let loaded = LoadedBundle::read_with_limits_probe(&b, profile).unwrap();
        let fresh = Core::spawn(Document::default()).unwrap();
        loaded.restore_manual(&fresh).unwrap();
        assert_eq!(fresh.export_history().unwrap(), selected);
    }
}

#[test]
pub(super) fn w13_recovery_on_active_rescue_copy_settles_the_new_attempt() {
    use kinewright_project::bundle_switch::ProjectSelectionResult;
    for occurrence in [1, 2] {
        let temp = temp();
        let a = temp.path("drive/a.kinewright");
        let b = temp.path("b.kinewright");
        let core = Core::spawn(document()).unwrap();
        let mut session =
            Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
        io_observer::fail_once("root_parent_sync", || {
            session.first_save(&a, StorageLimits::default())
        })
        .unwrap();
        std::fs::rename(temp.path("drive"), temp.path("hidden-a")).unwrap();
        assert!(rescue(&mut session, &b, None).activated);
        for n in 0..3 {
            edit(session.core(), n % 2 == 0);
            session.save_active().unwrap();
        }
        let target = session
            .list_recovery_points()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        edit(session.core(), false);
        session.prepare_recovery(target).unwrap();
        let old = session.core().clone();
        assert!(matches!(
            io_observer::fail_nth("root_parent_sync", occurrence, || session.select_local()),
            ProjectSelectionResult::Unknown
        ));
        assert!(matches!(
            session.select_local(),
            ProjectSelectionResult::Activated(_)
        ));
        assert_eq!(old.attachment_state().unwrap(), AttachmentState::Sealed);
        assert_eq!(session.rescue_slots(), 1);
    }
}

#[test]
pub(super) fn w08_context_close_fences_handles_and_retires_temporary_sources_once() {
    let temp = temp();
    let a = temp.path("drive/a.kinewright");
    let b = temp.path("b.kinewright");
    let core = Core::spawn(document()).unwrap();
    let mut session = Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
    let source = session
        .source()
        .unwrap()
        .store()
        .resolved_root()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let usage = session.source().unwrap().store().usage_handle();
    io_observer::fail_once("root_parent_sync", || {
        session.first_save(&a, StorageLimits::default())
    })
    .unwrap();
    std::fs::rename(temp.path("drive"), temp.path("hidden-a")).unwrap();
    assert!(rescue(&mut session, &b, None).activated);
    let active = session.core().clone();
    assert!(source.exists());
    drop(session);
    assert!(
        !source.exists(),
        "context teardown can retire source bytes after every reader retires"
    );
    assert_eq!(core.attachment_state().unwrap(), AttachmentState::Sealed);
    assert_eq!(active.attachment_state().unwrap(), AttachmentState::Sealed);
    assert!(!usage().unwrap().damaged_accounting);
    assert_eq!(usage().unwrap().reserved_copy_bytes, 0);
}

#[test]
pub(super) fn w06_rescue_bounds_refuse_before_destination_mutation() {
    let temp = temp();
    let a = temp.path("drive/a.kinewright");
    let b = temp.path("b.kinewright");
    let core = Core::spawn(document()).unwrap();
    edit(&core, false);
    let mut session = Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
    io_observer::fail_once("root_parent_sync", || {
        session.first_save(&a, StorageLimits::default())
    })
    .unwrap();
    std::fs::rename(temp.path("drive"), temp.path("hidden-a")).unwrap();
    let before = core.export_history().unwrap();
    for profile in [
        StorageLimits {
            objects: 0,
            ..StorageLimits::default()
        },
        StorageLimits {
            metadata_bytes: 0,
            ..StorageLimits::default()
        },
        StorageLimits {
            decoded_record_bytes: 0,
            ..StorageLimits::default()
        },
        StorageLimits {
            owned_bytes: 0,
            ..StorageLimits::default()
        },
        StorageLimits {
            copy_overlap_bytes: 0,
            ..StorageLimits::default()
        },
    ] {
        let offer = session.probe_active().unwrap().unwrap();
        let capture = session.prepare_rescue(&offer).unwrap().unwrap();
        assert!(session.preview_rescue(capture, profile).is_err());
        assert_eq!(core.attachment_state().unwrap(), AttachmentState::Live);
        assert_eq!(core.export_history().unwrap(), before);
        assert_eq!(session.rescue_slots(), 0);
        assert!(!b.exists());
        assert!(!temp.path("b.kinewright-assets").exists());
    }
    assert!(rescue(&mut session, &b, None).activated);
}

#[test]
pub(super) fn w13_available_recovery_roots_cannot_be_omitted_for_quota() {
    use kinewright_project::bundle::BundleWriteAdmission;
    let temp = temp();
    let a = temp.path("drive/a.kinewright");
    let b = temp.path("b.kinewright");
    std::fs::create_dir_all(a.parent().unwrap()).unwrap();
    let core = Core::spawn(document()).unwrap();
    grow(&core);
    let before = core.export_history().unwrap();
    assert!(
        kinewright_project::bundle_commit::bound_history_probe(&before, &floor_limits())
            .unwrap()
            .floor_offset
            > before.floor_offset
    );
    let mut session =
        Ja1EditingSession::activate(&a, BundleWriteAdmission::new_document(), &core, None).unwrap();
    session.save_active().unwrap();
    edit(&core, false);
    assert!(io_observer::fail_once("root_parent_sync", || session.save_active()).is_err());
    std::fs::rename(temp.path("drive"), temp.path("hidden-a")).unwrap();
    let offer = session.probe_active().unwrap().unwrap();
    let capture = session.prepare_rescue(&offer).unwrap().unwrap();
    let before = core.export_history().unwrap();
    assert!(
        session.preview_rescue(capture, floor_limits()).is_err(),
        "available acknowledged history cannot be discarded to fit quota"
    );
    assert!(!b.exists());
    assert_eq!(session.rescue_slots(), 0);
    assert_eq!(core.export_history().unwrap(), before);
    assert!(rescue(&mut session, &b, None).activated);
}
