//! S2-R accepted-review regressions. Each oracle reads the independent durable destination.
use kinewright_core::{
    AttachmentState, Command, Core, Document, Event, Operation, Track, TrackId, TrackKind,
};
use kinewright_media::test_support::TempDirectory;
use kinewright_project::{
    bundle::{BundleError, BundleWriteAdmission, LoadedBundle, ProjectBundle},
    bundle_copy::CopyStatus,
    bundle_records::{ObjectRef, RecordKind},
    bundle_recovery::RecoverySelection,
    editing_session::Ja1EditingSession,
    io_observer,
    receipts::{CommandEnvelope, CommandId, ReceiptLookup},
    storage_budget::StorageLimits,
    store::StoreManifest,
};
use std::path::Path;
fn temp() -> TempDirectory {
    TempDirectory::new_in(super::durable_test_root(), "s2-review-fix2")
}
fn document() -> Document {
    Document {
        tracks: vec![Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        }],
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
fn reopen(path: &Path) -> kinewright_core::HistorySnapshot {
    let reader = Core::spawn(Document::default()).unwrap();
    LoadedBundle::read(path)
        .unwrap()
        .restore_manual(&reader)
        .unwrap();
    reader.export_history().unwrap()
}
fn named(temp: &TempDirectory) -> Ja1EditingSession {
    let core = Core::spawn(document()).unwrap();
    let mut owner = Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
    assert!(matches!(
        owner
            .first_save(&temp.path("drive/a.kinewright"), StorageLimits::default())
            .unwrap(),
        CopyStatus::Committed(_)
    ));
    owner
}
fn save_race(mode: &str) {
    let temp = temp();
    let core = Core::spawn(document()).unwrap();
    let mut owner = Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
    let a = temp.path("drive/a.kinewright");
    if mode != "first" {
        owner.first_save(&a, StorageLimits::default()).unwrap();
    }
    edit(&core, false);
    let cut = core.export_history().unwrap();
    let raced = core.clone();
    let path = if mode == "as" {
        temp.path("b.kinewright")
    } else {
        a
    };
    io_observer::at_barrier(
        if mode == "save" {
            "history_selected"
        } else {
            "copy_before_publish"
        },
        move || edit(&raced, true),
        || match mode {
            "first" => {
                owner.first_save(&path, StorageLimits::default()).unwrap();
            }
            "as" => {
                owner.save_as(&path, StorageLimits::default()).unwrap();
            }
            _ => {
                owner.save_active().unwrap();
            }
        },
    );
    assert_eq!(
        reopen(&path),
        cut,
        "independent reopen excludes the post-cut suffix"
    );
    assert_ne!(core.export_history().unwrap(), cut);
    assert!(
        owner.dirty().unwrap(),
        "an accepted post-cut suffix must remain dirty"
    );
    assert!(
        !owner.close_disclosure().unwrap().is_empty(),
        "Close must disclose the unpublished suffix"
    );
    owner.save_active().unwrap();
    assert!(!owner.dirty().unwrap());
    assert_eq!(reopen(&path), core.export_history().unwrap());
}
#[test]
fn r1_first_save_suffix_stays_dirty() {
    save_race("first");
}
#[test]
fn r1_ordinary_save_suffix_stays_dirty() {
    save_race("save");
}
#[test]
fn r1_save_as_suffix_stays_dirty() {
    save_race("as");
}

fn object(bytes: &[u8]) -> ObjectRef {
    ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::OwnedEvidence,
    }
}
fn comment_rescue(landed: bool) {
    let temp = temp();
    let a = temp.path("drive/a.kinewright");
    let b = temp.path("b.kinewright");
    let core = Core::spawn(document()).unwrap();
    std::fs::create_dir(temp.path("drive")).unwrap();
    let mut bundle = ProjectBundle::activate(&a, BundleWriteAdmission::new_document()).unwrap();
    let input = bundle.empty_inputs().unwrap().reference.clone();
    bundle.save(&core, &input).unwrap();
    // Independently committed content remains selected even when pending comment content is excluded.
    let committed = object(b"INDEPENDENT COMMITTED EVIDENCE");
    let sealed = bundle
        .store_mut()
        .unwrap()
        .admit_blob(&mut &b"INDEPENDENT COMMITTED EVIDENCE"[..], &committed, &[])
        .unwrap();
    bundle.retain_evidence(&committed).unwrap();
    drop(sealed);
    let issuer = bundle.issue_issuer(&core, &input, 1).unwrap();
    let text = object(b"PENDING COMMENT MUST STAY AT A");
    let strokes = object(b"[]");
    let envelope = CommandEnvelope {
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
        expected_revision: match core
            .request(Command::Query(kinewright_core::Query::Snapshot))
            .unwrap()
        {
            Event::QueryResult(kinewright_core::QueryResult::Snapshot { revision, .. }) => revision,
            _ => panic!("snapshot"),
        },
        command: None,
        comment_text: Some(text.clone()),
        comment_strokes: Some(strokes.clone()),
        evidence: vec![],
        transition: None,
    };
    let old = bundle.evidence_refs().to_vec();
    let result = io_observer::fail_once(
        if landed {
            "root_parent_sync"
        } else {
            "root_rename_io"
        },
        || {
            bundle.command(
                &envelope,
                &core,
                &input,
                &[
                    (&text, b"PENDING COMMENT MUST STAY AT A"),
                    (&strokes, b"[]"),
                ],
            )
        },
    );
    assert!(result.is_err());
    let tentative = bundle
        .evidence_refs()
        .iter()
        .find(|r| !old.contains(r))
        .unwrap()
        .clone();
    let mut owner = Ja1EditingSession::from_bundle(bundle, &core, input).unwrap();
    std::fs::rename(temp.path("drive"), temp.path("hidden-drive")).unwrap();
    assert!(owner.reconcile_active().is_err());
    let offer = owner.probe_active().unwrap().unwrap();
    let capture = owner.prepare_rescue(&offer).unwrap().unwrap();
    let selection = owner
        .preview_rescue(capture, StorageLimits::default())
        .unwrap();
    let consent = selection.consent();
    let destination = owner.inspect_rescue_destination(&b).unwrap();
    assert!(
        owner
            .rescue(selection, &consent, &destination)
            .unwrap()
            .activated
    );
    assert!(owner.pending_agent_command_at(&a));
    drop(owner);
    let loaded = LoadedBundle::read(&b).unwrap();
    let manifest_ref = loaded.root().unwrap().store_manifest_ref.clone();
    let b_owner = loaded.into_writer().unwrap();
    let manifest = b_owner
        .store()
        .read_record::<StoreManifest>(&manifest_ref)
        .unwrap();
    assert!(manifest.durable_pins.contains(&committed));
    assert_eq!(
        b_owner
            .store()
            .read_evidence(&committed)
            .unwrap()
            .as_slice(),
        b"INDEPENDENT COMMITTED EVIDENCE"
    );
    for pending in [&tentative, &text, &strokes] {
        assert!(!manifest.durable_pins.contains(pending));
        assert!(
            !manifest.objects.contains(pending),
            "pending-only content cannot enter B's manifest"
        );
        assert!(
            b_owner.store().read_evidence(pending).is_err(),
            "pending-only bytes stay at A"
        );
    }
    assert_eq!(
        b_owner.receipt_lookup(&envelope).unwrap(),
        ReceiptLookup::UnresolvedOrigin
    );
    drop(manifest);
    drop(b_owner);
    std::fs::rename(temp.path("hidden-drive"), temp.path("drive")).unwrap();
    let a_owner = LoadedBundle::read(&a).unwrap().into_writer().unwrap();
    assert_eq!(
        matches!(
            a_owner.receipt_lookup(&envelope).unwrap(),
            ReceiptLookup::Committed(_)
        ),
        landed
    );
    assert_eq!(a_owner.evidence_refs().contains(&tentative), landed);
}
#[test]
fn r4_w10_comment_landed_a_excludes_pending_b() {
    comment_rescue(true);
}
#[test]
fn r4_w10_comment_absent_a_excludes_pending_b() {
    comment_rescue(false);
}
fn rescued(temp: &TempDirectory) -> Ja1EditingSession {
    let mut owner = named(temp);
    edit(owner.core(), false);
    assert!(io_observer::fail_once("root_parent_sync", || owner.save_active()).is_err());
    std::fs::rename(temp.path("drive"), temp.path("hidden-drive")).unwrap();
    let offer = owner.probe_active().unwrap().unwrap();
    let capture = owner.prepare_rescue(&offer).unwrap().unwrap();
    let selection = owner
        .preview_rescue(capture, StorageLimits::default())
        .unwrap();
    let consent = selection.consent();
    let destination = owner
        .inspect_rescue_destination(&temp.path("b.kinewright"))
        .unwrap();
    assert!(
        owner
            .rescue(selection, &consent, &destination)
            .unwrap()
            .activated
    );
    owner
}
fn records(
    owner: &Ja1EditingSession,
    path: &Path,
) -> Vec<kinewright_project::rescue_record::RescueCopyV1> {
    let loaded = LoadedBundle::read(path).unwrap();
    let manifest = owner
        .store()
        .unwrap()
        .read_record::<StoreManifest>(&loaded.root().unwrap().store_manifest_ref)
        .unwrap();
    manifest
        .durable_pins
        .iter()
        .filter_map(|r| {
            owner
                .store()
                .unwrap()
                .read_record::<kinewright_project::rescue_record::RescueCopyV1>(r)
                .ok()
                .map(|record| (*record).clone())
        })
        .collect()
}
#[test]
fn r5_retirement_staging_and_publish_refusals_preserve_complete_record() {
    for fault in ["object_file_sync", "refused_rename"] {
        let temp = temp();
        let mut owner = rescued(&temp);
        let b = temp.path("b.kinewright");
        let expected = records(&owner, &b);
        let point = expected[0].acknowledged_root.clone().unwrap();
        let raw = std::fs::read(&b).unwrap();
        let sha = owner.root_sha256().unwrap();
        assert!(
            if fault == "object_file_sync" {
                io_observer::fail_once(fault, || {
                    owner.retire_rescue_recovery_point(&point, &sha, true)
                })
            } else {
                io_observer::fail_native_once("root_rename_io", 13, || {
                    owner.retire_rescue_recovery_point(&point, &sha, true)
                })
            }
            .is_err()
        );
        assert_eq!(std::fs::read(&b).unwrap(), raw);
        owner.save_active().unwrap();
        assert_eq!(
            records(&owner, &b),
            expected,
            "refused retirement must preserve every record field after later Save"
        );
        let sha = owner.root_sha256().unwrap();
        owner
            .retire_rescue_recovery_point(&point, &sha, true)
            .unwrap();
        let mut retired = expected;
        retired[0].acknowledged_root = None;
        retired[0].acknowledged_absence =
            Some("Forget the older recovery point from A: explicitly retired".into());
        assert_eq!(
            records(&owner, &b),
            retired,
            "successful control changes only the retired edge and its required reason"
        );
    }
}
#[test]
fn r5_refused_branch_retirement_survives_later_save() {
    let temp = temp();
    let mut owner = named(&temp);
    let guard = owner.fork_durable_branch("retirement control").unwrap();
    let branch = guard.branch_id().clone();
    owner.close_durable_branch(&guard).unwrap();
    let before = owner.list_working_branches().unwrap();
    let raw = std::fs::read(temp.path("drive/a.kinewright")).unwrap();
    let sha = owner.root_sha256().unwrap();
    assert!(
        io_observer::fail_once("root_before_publish", || owner
            .retire_working_branch(&branch, &sha, true))
        .is_err()
    );
    assert_eq!(std::fs::read(temp.path("drive/a.kinewright")).unwrap(), raw);
    assert_eq!(owner.list_working_branches().unwrap(), before);
    owner.save_active().unwrap();
    drop(owner);
    let reopened = Ja1EditingSession::open(&temp.path("drive/a.kinewright")).unwrap();
    assert_eq!(reopened.list_working_branches().unwrap(), before);
}
#[test]
fn r7_registered_and_manual_branches_refuse_second_writer() {
    let temp = temp();
    let mut owner = named(&temp);
    let mut guard = owner.fork_durable_branch("writer").unwrap();
    let base = owner.persist_durable_branch(&mut guard).unwrap();
    let manual = guard.base().target_branch.clone();
    assert!(matches!(
        owner.reopen_durable_branch(manual, &base, "manual duplicate"),
        Err(BundleError::Contention(_))
    ));
    let selection = RecoverySelection::WorkingBranch(guard.branch_id().clone());
    assert!(matches!(
        owner.prepare_recovery(selection.clone()),
        Err(BundleError::Contention(_))
    ));
    owner.close_durable_branch(&guard).unwrap();
    owner.prepare_recovery(selection).unwrap();
    let reopened = owner
        .reopen_durable_branch(guard.branch_id().clone(), &base, "new writer after prepare")
        .unwrap();
    let raw = std::fs::read(temp.path("drive/a.kinewright")).unwrap();
    assert!(matches!(
        owner.select_local(),
        kinewright_project::bundle_switch::ProjectSelectionResult::Refused(
            BundleError::Contention(_)
        )
    ));
    assert_eq!(std::fs::read(temp.path("drive/a.kinewright")).unwrap(), raw);
    edit(reopened.core(), false);
}
#[test]
fn r9_a_seal_failure_never_retains_live_b() {
    let temp = temp();
    let a = temp.path("drive/a.kinewright");
    let core = Core::spawn(document()).unwrap();
    std::fs::create_dir(temp.path("drive")).unwrap();
    let mut bundle = ProjectBundle::activate(&a, BundleWriteAdmission::new_document()).unwrap();
    let attachment = bundle.attach_core(&core).unwrap();
    let inputs = bundle.empty_inputs().unwrap().reference.clone();
    bundle.save(&core, &inputs).unwrap();
    let mut owner = Ja1EditingSession::from_bundle(bundle, &core, inputs).unwrap();
    assert!(io_observer::fail_once("root_parent_sync", || owner.save_active()).is_err());
    std::fs::rename(temp.path("drive"), temp.path("hidden-drive")).unwrap();
    let offer = owner.probe_active().unwrap().unwrap();
    let capture = owner.prepare_rescue(&offer).unwrap().unwrap();
    let selection = owner
        .preview_rescue(capture, StorageLimits::default())
        .unwrap();
    let cut = attachment.capture().unwrap();
    let consent = selection.consent();
    let destination = owner
        .inspect_rescue_destination(&temp.path("b.kinewright"))
        .unwrap();
    let mut b = None;
    let _outcome = owner.rescue_with_activation(selection, &consent, &destination, |candidate| {
        b = Some(candidate.clone());
        // The owner-authorized interference makes A's exact seal fail deterministically.
        attachment.resume(&cut).unwrap();
        Ok(())
    });
    assert_ne!(
        b.unwrap().attachment_state().unwrap(),
        AttachmentState::Live,
        "B cannot remain Live when A cannot seal"
    );
}

#[test]
fn n2_sealed_a_failed_b_resume_returns_committed_capture() {
    for frozen in [false, true] {
        let temp = temp();
        let mut owner = named(&temp);
        let original = owner.core().clone();
        edit(&original, false);
        assert!(io_observer::fail_once("root_parent_sync", || owner.save_active()).is_err());
        std::fs::rename(temp.path("drive"), temp.path("hidden-drive")).unwrap();
        if frozen {
            owner.freeze_probe().unwrap();
        }
        let offer = owner.probe_active().unwrap().unwrap();
        let capture = owner.prepare_rescue(&offer).unwrap().unwrap();
        let expected = original.export_history().unwrap();
        let selection = owner
            .preview_rescue(capture, StorageLimits::default())
            .unwrap();
        let consent = selection.consent();
        let b = temp.path("b.kinewright");
        let destination = owner.inspect_rescue_destination(&b).unwrap();
        let binder = ProjectBundle::activate(
            &temp.path("cut-authority.kinewright"),
            BundleWriteAdmission::new_document(),
        )
        .unwrap();
        let mut candidate = None;
        let outcome = owner
            .rescue_with_activation(selection, &consent, &destination, |core| {
                let attachment = binder.attach_core(core)?;
                let cut = attachment.capture().unwrap();
                attachment.resume(&cut).unwrap();
                edit(core, true);
                attachment.freeze().unwrap();
                candidate = Some(core.clone());
                Ok(())
            })
            .expect(
                "a committed capture must be returned after A seals, rather than resuming sealed A",
            );
        assert!(!outcome.activated);
        assert!(matches!(outcome.status, CopyStatus::Committed(_)));
        assert!(outcome.disclosure.contains("A is sealed"));
        assert!(outcome.disclosure.contains(&b.display().to_string()));
        assert!(outcome.disclosure.contains("Open B"));
        assert_eq!(
            original.attachment_state().unwrap(),
            AttachmentState::Sealed
        );
        assert_eq!(
            candidate.unwrap().attachment_state().unwrap(),
            AttachmentState::Frozen
        );
        assert_eq!(original.export_history().unwrap(), expected);
        assert!(owner.is_capture_destination(&b));
        assert!(
            Ja1EditingSession::open(&b).is_err(),
            "B remains owned until explicit release or Close"
        );
        assert_eq!(reopen(&b), expected);
        drop(owner);
        assert_eq!(reopen(&b), expected);
    }
}

#[test]
fn r1_ordinary_floor_committed_identity_excludes_suffix() {
    let temp = temp();
    let mut doc = document();
    doc.tracks = (1..=200)
        .map(|id| Track {
            id: TrackId(id),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![],
        })
        .collect();
    let core = Core::spawn(doc).unwrap();
    let mut owner = Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
    let path = temp.path("a.kinewright");
    owner.first_save(&path, StorageLimits::default()).unwrap();
    for n in 0..40 {
        edit(&core, n % 2 == 0);
    }
    let limits = StorageLimits {
        branch_history_bytes: 1 << 20,
        ..StorageLimits::default()
    };
    owner.restrict_limits_probe(limits.clone()).unwrap();
    let before = core.export_history().unwrap();
    let selected =
        kinewright_project::bundle_commit::bound_history_probe(&before, &limits).unwrap();
    assert!(selected.floor_offset > before.floor_offset);
    let suffix = core.clone();
    io_observer::at_barrier(
        "before_floor_commit",
        move || edit(&suffix, true),
        || owner.save_active(),
    )
    .unwrap();
    assert_eq!(reopen(&path), selected);
    assert!(core.export_history().unwrap().floor_offset > before.floor_offset);
    assert!(
        owner.dirty().unwrap(),
        "committing an ordinary floor cannot mark the later suffix Saved"
    );
    let ((), trace) = io_observer::observe(|| {
        for _ in 0..100 {
            assert!(owner.dirty().unwrap());
        }
    });
    assert!(
        !trace
            .events
            .iter()
            .any(|e| e.operation == "studio_dirty_history_hash")
    );
}
