use super::{apply, document, edit};
use kinewright_core::{Command, Core, HistorySaveTag, Operation, TrackId};
use kinewright_media::test_support::TempDirectory;
use kinewright_project::{
    bundle::{BundleError, BundleWriteAdmission, LoadedBundle, ProjectBundle},
    bundle_commit::bound_history_probe,
    bundle_copy::{CopyStatus, SessionStore},
    bundle_records::{ObjectRef, RecordKind},
    bundle_rescue::{MissingOwnedUse, ReconcilePhase, RescueCause, RescueEligibility},
    editing_session::{Ja1EditingSession, RescueSlots},
    io_observer,
    owned_inputs::{OwnedInputEntry, OwnedInputManifest},
    receipts::{CommandEnvelope, CommandId, ReceiptLookup},
    storage_budget::StorageLimits,
    store::ProjectStore,
};
use std::{
    fs,
    io::Read,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

fn temp(label: &str) -> TempDirectory {
    TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), label)
}
fn limits() -> StorageLimits {
    StorageLimits {
        branch_history_bytes: 1 << 20,
        ..StorageLimits::default()
    }
}
fn grow(core: &Core) {
    for n in 0..40 {
        edit(core, n % 2 == 0);
    }
}
fn object_path(store: &ProjectStore, object: &ObjectRef) -> PathBuf {
    let root = store.resolved_root().unwrap();
    root.with_file_name(format!(
        "{}.kinewright-assets",
        root.file_stem().unwrap().to_str().unwrap()
    ))
    .join("studio/objects")
    .join(&object.sha256)
}
fn read_bytes(store: &ProjectStore, object: &ObjectRef) -> Vec<u8> {
    fs::read(object_path(store, object)).unwrap()
}
fn blob(store: &mut ProjectStore, bytes: &[u8]) -> ObjectRef {
    let reference = ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::OwnedEvidence,
    };
    store
        .admit_blob(&mut &*bytes, &reference, &[])
        .unwrap()
        .reference
        .clone()
}
fn unknown(root: &Path, path: &Path, landed: bool) -> Ja1EditingSession {
    let core = Core::spawn(document()).unwrap();
    grow(&core);
    let mut session = Ja1EditingSession::new(root, &core, StorageLimits::default()).unwrap();
    let status = io_observer::fail_once(
        if landed {
            "root_parent_sync"
        } else {
            "root_rename_io"
        },
        || session.first_save(path, limits()),
    )
    .unwrap();
    assert!(matches!(status, CopyStatus::DurabilityUnknown));
    session
}
fn hide_parent(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap();
    let hidden = parent.with_extension("held");
    fs::rename(parent, &hidden).unwrap();
    hidden
}
fn restore_parent(path: &Path, hidden: &Path) {
    fs::rename(hidden, path.parent().unwrap()).unwrap();
}
fn adopt_current(session: &mut Ja1EditingSession, path: &Path, target: &Path) {
    let hidden = hide_parent(path);
    let offer = session.probe_active().unwrap().unwrap();
    let capture = session.prepare_rescue(&offer).unwrap().unwrap();
    let consent = capture.consent();
    let inspection = session.inspect_rescue_destination(target).unwrap();
    session
        .adopt_attempt_probe(capture, &consent, &inspection, None)
        .unwrap();
    restore_parent(path, &hidden);
    session.resume_probe().unwrap();
}

#[test]
fn w01_retained_source_and_a_writer_control() {
    let temp = temp("c3-writer");
    let path = temp.path("new/parents/a.kinewright");
    let core = Core::spawn(document()).unwrap();
    let mut session = Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
    let source = session.source_mut().unwrap();
    let owned = blob(source.store_mut(), b"actual source bytes");
    let inputs = source
        .admit_inputs(&OwnedInputManifest {
            project_id: None,
            session_id: Some(source.session_id().clone()),
            owner_epoch: source.store().owner_epoch(),
            entries: vec![OwnedInputEntry {
                entry_id: "source".into(),
                object: owned.clone(),
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
    session.set_unsaved_inputs(inputs).unwrap();
    let retained = session
        .source()
        .unwrap()
        .store()
        .admit_retained_reads(&[(owned.clone(), MissingOwnedUse::Current)])
        .unwrap();
    let root = session.source().unwrap().store().resolved_root().unwrap();
    let usage = session.source().unwrap().store().usage_handle();
    grow(&core);
    assert!(matches!(
        io_observer::fail_once("root_parent_sync", || session.first_save(&path, limits())).unwrap(),
        CopyStatus::DurabilityUnknown
    ));
    adopt_current(&mut session, &path, &temp.path("b.kinewright"));
    session.reconcile_active().unwrap();
    assert!(session.dirty().unwrap());
    assert!(kinewright_project::namespace::ProjectNamespace::acquire(&path).is_err());
    edit(&core, true);
    session.save_active().unwrap();
    assert!(!session.dirty().unwrap());
    let mut bytes = vec![];
    retained
        .open(&owned)
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"actual source bytes");
    assert!(usage().unwrap().physical_bytes >= owned.byte_length);
    assert!(root.parent().unwrap().exists());
    drop(retained);
    assert_eq!(
        read_bytes(session.source().unwrap().store(), &owned),
        b"actual source bytes"
    );
    drop(session);
    assert!(!root.parent().unwrap().exists());
    assert!(!usage().unwrap().damaged_accounting);
    let _writer = LoadedBundle::read(&path).unwrap().into_writer().unwrap();
}

#[test]
fn w01_ordinary_handoff_retires_only_unadopted_source() {
    let temp = temp("c3-ordinary-retire");
    let core = Core::spawn(document()).unwrap();
    let mut session = Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
    let source = session.source().unwrap().store().resolved_root().unwrap();
    let usage = session.source().unwrap().store().usage_handle();
    session
        .first_save(&temp.path("a.kinewright"), StorageLimits::default())
        .unwrap();
    assert!(session.source().is_none());
    assert!(!source.parent().unwrap().exists());
    assert_eq!(usage().unwrap().reserved_copy_bytes, 0);
    assert_eq!(session.rescue_slots(), 0);
}

#[test]
fn w02_retained_handle_permission_boundaries() {
    let status = fs::read_to_string("/proc/self/status").unwrap();
    let uid = status
        .lines()
        .find(|line| line.starts_with("Uid:"))
        .unwrap()
        .split_whitespace()
        .nth(2)
        .unwrap();
    assert_ne!(uid, "0", "nonprivileged permission witness is required");
    let temp = temp("c3-permission");
    let above = temp.path("above");
    let parent = above.join("pinned");
    fs::create_dir_all(&parent).unwrap();
    let mut source = SessionStore::create(&parent, StorageLimits::default()).unwrap();
    let owned = blob(source.store_mut(), b"permission identity");
    let foreign = blob(source.store_mut(), b"outside the admitted closure");
    let retained = source
        .store()
        .admit_retained_reads(&[(owned.clone(), MissingOwnedUse::Current)])
        .unwrap();
    assert!(retained.open(&foreign).is_err());
    fs::set_permissions(&above, fs::Permissions::from_mode(0o0)).unwrap();
    let mut bytes = vec![];
    let result = retained
        .open(&owned)
        .and_then(|mut f| f.read_to_end(&mut bytes).map_err(BundleError::from));
    fs::set_permissions(&above, fs::Permissions::from_mode(0o700)).unwrap();
    result.unwrap();
    assert_eq!(bytes, b"permission identity");
    let objects = object_path(source.store(), &owned)
        .parent()
        .unwrap()
        .to_path_buf();
    fs::set_permissions(&objects, fs::Permissions::from_mode(0o0)).unwrap();
    let result = retained.open(&owned);
    let diagnostic = source
        .store()
        .diagnose_retained_reads(&[(owned.clone(), MissingOwnedUse::Current)], 0, 32)
        .unwrap();
    fs::set_permissions(&objects, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        matches!(result, Err(BundleError::Io(ref e)) if e.kind() == std::io::ErrorKind::PermissionDenied)
    );
    assert_eq!(diagnostic.entries[0].0, owned);
    assert!(diagnostic.entries[0].2.contains("Permission denied"));
    assert!(!temp.path("b.kinewright").exists());
    let mut restored = vec![];
    retained
        .open(&owned)
        .unwrap()
        .read_to_end(&mut restored)
        .unwrap();
    assert_eq!(restored.len() as u64, owned.byte_length);
    let original = read_bytes(source.store(), &owned);
    fs::write(
        object_path(source.store(), &owned),
        vec![b'x'; original.len()],
    )
    .unwrap();
    assert!(
        retained.open(&owned).is_err(),
        "a matching length cannot substitute for hash verification"
    );
    fs::write(object_path(source.store(), &owned), original).unwrap();
}

#[test]
fn w02_required_missing_bytes_and_incomplete_inventory() {
    let temp = temp("c3-diagnostic");
    let mut source = SessionStore::create(temp.root(), StorageLimits::default()).unwrap();
    let roles = [
        MissingOwnedUse::Current,
        MissingOwnedUse::Undo {
            branch: "manual".into(),
            cut: 3,
        },
        MissingOwnedUse::Redo {
            branch: "manual".into(),
            cut: 4,
        },
        MissingOwnedUse::Branch { id: "other".into() },
        MissingOwnedUse::Recovery {
            root_sha256: "b".repeat(64),
        },
        MissingOwnedUse::Manifest,
        MissingOwnedUse::Evidence {
            name: "proof".into(),
        },
    ];
    let roots = roles
        .iter()
        .enumerate()
        .map(|(i, role)| {
            (
                blob(source.store_mut(), format!("exact owned {i}").as_bytes()),
                role.clone(),
            )
        })
        .collect::<Vec<_>>();
    let originals = roots
        .iter()
        .map(|(r, _)| read_bytes(source.store(), r))
        .collect::<Vec<_>>();
    for (r, _) in &roots {
        fs::remove_file(object_path(source.store(), r)).unwrap();
    }
    let page = source
        .store()
        .diagnose_retained_reads(&roots, 0, 3)
        .unwrap();
    assert_eq!(page.entries.len(), 3);
    assert_eq!(page.omitted, 4);
    assert_eq!(page.next, Some(3));
    let mut all = page.entries;
    let page = source
        .store()
        .diagnose_retained_reads(&roots, 3, 32)
        .unwrap();
    all.extend(page.entries);
    assert_eq!(all.len(), roles.len());
    for (r, role) in &roots {
        assert!(all.iter().any(|(found, uses, cause)| found == r
            && uses == std::slice::from_ref(role)
            && cause.contains("missing owned object")));
    }
    assert!(matches!(
        source.store().admit_retained_reads(&roots),
        Err(BundleError::RescueMissingOwnedBytes(_))
    ));
    // The unavailable manifest has children that cannot be established from this inventory.
    let unknown = ObjectRef {
        sha256: "a".repeat(64),
        byte_length: 10,
        record_kind: RecordKind::InputManifest,
    };
    let incomplete = source
        .store()
        .diagnose_retained_reads(&[(unknown, MissingOwnedUse::Manifest)], 0, 32)
        .unwrap();
    assert!(!incomplete.dependencies_complete);
    for ((r, _), bytes) in roots.iter().zip(originals) {
        fs::write(object_path(source.store(), r), bytes).unwrap();
    }
    let retained = source.store().admit_retained_reads(&roots).unwrap();
    assert_eq!(retained.objects().len(), roles.len());
    assert!(!temp.path("b.kinewright").exists());
}

#[test]
fn w02_first_save_capture_prefers_local_owned_bytes() {
    let temp = temp("c3-local-first");
    let path = temp.path("a.kinewright");
    let mut session = unknown(temp.root(), &path, true);
    let object = session.store().unwrap().manifest().unwrap().objects[0].clone();
    let objects = object_path(session.store().unwrap(), &object)
        .parent()
        .unwrap()
        .to_path_buf();
    fs::set_permissions(&objects, fs::Permissions::from_mode(0o0)).unwrap();
    let prepared = (|| {
        let offer = session
            .probe_active()?
            .ok_or(BundleError::HistoryBarrierInvariant)?;
        session
            .prepare_rescue(&offer)?
            .ok_or(BundleError::HistoryBarrierInvariant)
    })();
    fs::set_permissions(&objects, fs::Permissions::from_mode(0o700)).unwrap();
    let capture = prepared.unwrap();
    for reference in capture.reads().objects() {
        let mut bytes = vec![];
        capture
            .reads()
            .open(reference)
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(
            bytes,
            read_bytes(session.source().unwrap().store(), reference)
        );
    }
    session.cancel_rescue(capture).unwrap();
    assert_eq!(session.rescue_slots(), 0);
    session.reconcile_active().unwrap();
}

#[test]
fn w03_probe_consent_and_premutation_cancel() {
    let temp = temp("c3-cancel");
    fs::create_dir(temp.path("a")).unwrap();
    let path = temp.path("a/a.kinewright");
    let mut session = unknown(temp.root(), &path, true);
    let hidden = hide_parent(&path);
    let before = session.core().export_history().unwrap();
    let ordinary = session.source().unwrap().store().usage().unwrap();
    for _ in 0..3 {
        let (capture, trace) = io_observer::observe(|| {
            let offer = session.probe_active().unwrap().unwrap();
            assert_eq!(offer.eligibility, RescueEligibility::Eligible);
            assert_eq!(offer.cause, RescueCause::Unavailable);
            session.prepare_rescue(&offer).unwrap().unwrap()
        });
        assert!(trace.events.iter().all(|e| {
            ![
                "mkdir",
                "claim",
                "object_rename",
                "root_rename",
                "rescue_slot_begin",
            ]
            .contains(&e.operation)
        }));
        assert_eq!(session.rescue_slots(), 0);
        assert!(session.retained_attempts().is_empty());
        session.cancel_rescue(capture).unwrap();
        assert_eq!(session.core().export_history().unwrap(), before);
    }
    let usage = session.source().unwrap().store().usage().unwrap();
    assert_eq!(usage.reserved_copy_bytes, ordinary.reserved_copy_bytes);
    assert_eq!(usage.physical_bytes, ordinary.physical_bytes);
    session.freeze_probe().unwrap();
    let offer = session.probe_active().unwrap().unwrap();
    let capture = session.prepare_rescue(&offer).unwrap().unwrap();
    session.cancel_rescue(capture).unwrap();
    assert_eq!(
        session.core().attachment_state().unwrap(),
        kinewright_core::AttachmentState::Frozen
    );
    session.resume_probe().unwrap();
    // A second probe that settles must never adopt the source or leave the actor paused.
    let offer = session.probe_active().unwrap().unwrap();
    restore_parent(&path, &hidden);
    assert!(session.prepare_rescue(&offer).unwrap().is_none());
    assert_eq!(session.rescue_slots(), 0);
    assert!(session.retained_attempts().is_empty());
    assert!(session.dirty().unwrap());
    session.save_active().unwrap();
}

#[test]
fn w03_native_probe_causes_keep_phase_operation_and_errno() {
    for (errno, cause, eligibility) in [
        (
            libc::EINTR,
            RescueCause::Interrupted,
            RescueEligibility::Ineligible,
        ),
        (
            libc::EBADF,
            RescueCause::Invalid,
            RescueEligibility::Ineligible,
        ),
        (
            libc::EIO,
            RescueCause::Unavailable,
            RescueEligibility::Eligible,
        ),
        (
            libc::ENOSPC,
            RescueCause::Persistence,
            RescueEligibility::Eligible,
        ),
    ] {
        let temp = self::temp("c3-native-probe");
        let path = temp.path("a.kinewright");
        let mut session = unknown(temp.root(), &path, true);
        let ticket = session.core().pending_history_save().unwrap();
        let probe = io_observer::fail_native_once("directory_sync_native", errno, || {
            session.probe_active()
        })
        .unwrap()
        .unwrap();
        assert_eq!(probe.phase, ReconcilePhase::Barrier);
        assert_eq!(probe.operation, "sync root namespace");
        assert_eq!(probe.cause, cause);
        assert_eq!(probe.eligibility, eligibility);
        assert_eq!(
            std::error::Error::source(&probe.error)
                .unwrap()
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .raw_os_error(),
            Some(errno)
        );
        assert!(
            matches!(probe.error, BundleError::Io(ref error) if error.raw_os_error() == Some(errno))
        );
        assert_eq!(session.core().pending_history_save().unwrap(), ticket);
        assert_eq!(session.rescue_slots(), 0);
        session.reconcile_active().unwrap();
    }
    let temp = self::temp("c3-wrapped-probe");
    let path = temp.path("a.kinewright");
    let mut session = unknown(temp.root(), &path, true);
    session.defer_parent_barrier_probe().unwrap();
    let probe = io_observer::fail_native_once("directory_sync_native", libc::EINTR, || {
        session.probe_active()
    })
    .unwrap()
    .unwrap();
    assert_eq!(probe.operation, "sync admitted directory");
    assert_eq!(probe.cause, RescueCause::Interrupted);
    assert_eq!(probe.eligibility, RescueEligibility::Ineligible);
    assert!(
        matches!(probe.error, BundleError::Io(ref error) if error.raw_os_error() == Some(libc::EINTR))
    );
    session.reconcile_active().unwrap();

    let temp = self::temp("c3-missing-graph-probe");
    let path = temp.path("a.kinewright");
    let mut session = unknown(temp.root(), &path, true);
    let reference = session
        .store()
        .unwrap()
        .manifest()
        .unwrap()
        .objects
        .into_iter()
        .find(|r| r.record_kind == RecordKind::InputManifest)
        .unwrap();
    let object = object_path(session.store().unwrap(), &reference);
    let bytes = fs::read(&object).unwrap();
    fs::remove_file(&object).unwrap();
    let probe = session.probe_active().unwrap().unwrap();
    fs::write(object, bytes).unwrap();
    assert_eq!(probe.phase, ReconcilePhase::Graph);
    assert_eq!(probe.cause, RescueCause::RequiredBytes);
    assert_eq!(probe.eligibility, RescueEligibility::Eligible);
    session.reconcile_active().unwrap();

    let temp = self::temp("c3-cleanup-probe");
    let path = temp.path("a.kinewright");
    let mut session = unknown(temp.root(), &path, false);
    let probe = io_observer::fail_native_once("root_cleanup_unlink", libc::EACCES, || {
        session.probe_active()
    })
    .unwrap()
    .unwrap();
    assert_eq!(probe.phase, ReconcilePhase::Cleanup);
    assert_eq!(probe.operation, "remove root temporary");
    assert_eq!(probe.cause, RescueCause::CleanupOnly);
    assert_eq!(probe.eligibility, RescueEligibility::Ineligible);
    assert!(
        matches!(probe.error, BundleError::Io(ref error) if error.raw_os_error() == Some(libc::EACCES))
    );
    session.reconcile_active().unwrap();

    let temp = self::temp("c3-authority-probe");
    let path = temp.path("a.kinewright");
    let mut session = unknown(temp.root(), &path, true);
    let lock = kinewright_project::lockfile_path_for_project(Some(&path)).unwrap();
    let held = temp.path("original-lock");
    fs::rename(&lock, &held).unwrap();
    let probe = session.probe_active().unwrap().unwrap();
    fs::rename(held, lock).unwrap();
    assert_eq!(probe.phase, ReconcilePhase::Authority);
    assert_eq!(probe.cause, RescueCause::Invalid);
    assert_eq!(probe.eligibility, RescueEligibility::Ineligible);
    session.reconcile_active().unwrap();

    // The exact-ticket proof also preserves native IO errors after reopening its own root.
    let temp = self::temp("c3-ticket-native");
    let path = temp.path("a.kinewright");
    let core = Core::spawn(document()).unwrap();
    let mut owner = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    owner.restrict_limits_probe(limits()).unwrap();
    let inputs = owner.empty_inputs().unwrap().reference.clone();
    owner.save(&core, &inputs).unwrap();
    grow(&core);
    assert!(io_observer::fail_once("root_parent_sync", || owner.save(&core, &inputs)).is_err());
    let attempt = owner.pending_attempt().unwrap();
    let attachment = owner.attach_core(&core).unwrap();
    let result = io_observer::fail_native_once("directory_sync_native", libc::EINTR, || {
        attachment.settle_attempt(&attempt)
    });
    assert!(
        matches!(result, Err(BundleError::Io(ref error)) if error.raw_os_error() == Some(libc::EINTR))
    );
    assert_eq!(
        core.pending_history_save().unwrap(),
        Some(attempt.ticket().clone())
    );
    owner.reconcile_destination(Some(&core)).unwrap();
}

#[test]
fn w03_changed_capture_invalidates_consent() {
    let temp = temp("c3-consent");
    fs::create_dir(temp.path("a")).unwrap();
    let path = temp.path("a/a.kinewright");
    let mut session = unknown(temp.root(), &path, true);
    let hidden = hide_parent(&path);
    let offer = session.probe_active().unwrap().unwrap();
    let before_holds = session.source().unwrap().store().retention_probe().unwrap();
    let before_bytes = session
        .source()
        .unwrap()
        .store()
        .usage()
        .unwrap()
        .reserved_copy_bytes;
    let capture = session.prepare_rescue(&offer).unwrap().unwrap();
    let admitted_bytes = capture
        .reads()
        .objects()
        .iter()
        .map(|r| r.byte_length)
        .sum::<u64>();
    assert_eq!(
        session
            .source()
            .unwrap()
            .store()
            .usage()
            .unwrap()
            .reserved_copy_bytes
            - before_bytes,
        admitted_bytes
    );
    let admitted_pins = session.source().unwrap().store().retention_probe().unwrap();
    assert_eq!(admitted_pins.0, before_holds.0 + 1);
    for object in capture.reads().objects() {
        assert_eq!(
            admitted_pins.1[&object.sha256],
            before_holds.1.get(&object.sha256).copied().unwrap_or(0) + 1
        );
    }
    let stale = capture.consent();
    session.cancel_rescue(capture).unwrap();
    edit(session.core(), true);
    let capture = session.prepare_rescue(&offer).unwrap().unwrap();
    assert!(capture.check_consent(&stale).is_err());
    let destination = session
        .inspect_rescue_destination(&temp.path("b.kinewright"))
        .unwrap();
    assert!(
        session
            .adopt_attempt_probe(capture, &stale, &destination, None)
            .is_err()
    );
    assert_eq!(session.rescue_slots(), 0);
    assert!(session.retained_attempts().is_empty());
    session.resume_probe().unwrap();
    restore_parent(&path, &hidden);
    session.reconcile_active().unwrap();
    grow(session.core());
    assert!(matches!(
        io_observer::fail_once("root_parent_sync", || session.save_active()),
        Err(BundleError::DurabilityUnknown)
    ));
    let hidden = hide_parent(&path);
    assert!(matches!(
        session.prepare_rescue(&offer),
        Err(BundleError::HistoryChanged)
    ));
    assert_eq!(session.rescue_slots(), 0);
    restore_parent(&path, &hidden);
    session.reconcile_active().unwrap();
    // Without a floor ticket, the tag itself must still invalidate a stale offer.
    let path = temp.path("plain/a.kinewright");
    let core = Core::spawn(document()).unwrap();
    let mut plain = Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
    io_observer::fail_once("root_parent_sync", || {
        plain.first_save(&path, StorageLimits::default())
    })
    .unwrap();
    assert!(core.pending_history_save().unwrap().is_none());
    let hidden = hide_parent(&path);
    let offer = plain.probe_active().unwrap().unwrap();
    restore_parent(&path, &hidden);
    plain.reconcile_active().unwrap();
    edit(&core, true);
    assert!(matches!(
        io_observer::fail_once("root_parent_sync", || plain.save_active()),
        Err(BundleError::DurabilityUnknown)
    ));
    assert!(core.pending_history_save().unwrap().is_none());
    let hidden = hide_parent(&path);
    assert!(matches!(
        plain.prepare_rescue(&offer),
        Err(BundleError::HistoryChanged)
    ));
    restore_parent(&path, &hidden);
    plain.reconcile_active().unwrap();
}

#[test]
fn w06_native_path_identity_and_missing_mount_separation() {
    use std::os::unix::fs::symlink;
    let temp = temp("c3-separation");
    fs::create_dir(temp.path("mount")).unwrap();
    let path = temp.path("mount/a.kinewright");
    let session = unknown(temp.root(), &path, true);
    let source = session.source().unwrap().store().resolved_root().unwrap();
    for target in [
        path.clone(),
        path.join("child.kinewright"),
        path.parent().unwrap().to_path_buf(),
        temp.path("mount/a.kinewright-assets/child.kinewright"),
        source.parent().unwrap().join("child.kinewright"),
        source.parent().unwrap().parent().unwrap().to_path_buf(),
    ] {
        assert!(
            session.inspect_rescue_destination(&target).is_err(),
            "{}",
            target.display()
        );
    }
    let sibling = temp.path("sibling/new/b.kinewright");
    let (inspection, trace) =
        io_observer::observe(|| session.inspect_rescue_destination(&sibling).unwrap());
    assert!(!sibling.parent().unwrap().exists());
    assert!(
        trace
            .events
            .iter()
            .all(|e| e.operation == "rescue_separation_inspect")
    );
    symlink(path.parent().unwrap(), temp.path("alias")).unwrap();
    assert!(
        session
            .inspect_rescue_destination(&temp.path("alias/b.kinewright"))
            .is_err()
    );
    fs::remove_file(temp.path("alias")).unwrap();
    let hidden = hide_parent(&path);
    assert!(
        session
            .inspect_rescue_destination(&temp.path("mount/new/b.kinewright"))
            .is_err()
    );
    fs::create_dir(temp.path("mount")).unwrap();
    assert!(
        session
            .inspect_rescue_destination(&temp.path("mount/new/b.kinewright"))
            .is_err()
    );
    assert!(
        session
            .inspect_rescue_destination(&temp.path("other/b.kinewright"))
            .is_ok()
    );
    fs::remove_dir(temp.path("mount")).unwrap();
    restore_parent(&path, &hidden);
    fs::create_dir_all(sibling.parent().unwrap()).unwrap();
    assert!(
        ProjectStore::revalidate_rescue_destination(&inspection, &[session.store().unwrap()])
            .is_err()
    );
}

fn tag(path: &Path, n: u64) -> HistorySaveTag {
    HistorySaveTag {
        destination: path.to_path_buf(),
        parent_device: 1,
        parent_inode: 1,
        root_sha256: format!("{n:064x}"),
        previous_root_sha256: None,
    }
}
#[test]
fn w07_mutation_boundary_and_four_attempt_slots() {
    // Pure C3 transition control. These tags model outcomes, not rescue publications.
    let temp = temp("c3-slots");
    let a = tag(&temp.path("a.kinewright"), 1);
    let b1 = tag(&temp.path("b.kinewright"), 2);
    let b2 = tag(&temp.path("b.kinewright"), 3);
    let c2 = tag(&temp.path("c2.kinewright"), 4);
    let mut slots = RescueSlots::default();
    slots.check(&a).unwrap();
    assert_eq!(slots.consumed(), 0);
    let reservation = slots.begin(a).unwrap();
    assert_eq!(slots.consumed(), 1);
    slots.finish(reservation, Some(b1.clone())).unwrap();
    assert_eq!(slots.consumed(), 2);
    let reservation = slots.begin(b1).unwrap();
    slots.finish(reservation, None).unwrap();
    assert_eq!(
        slots.consumed(),
        2,
        "mutating definite C1 refusal returns destination only"
    );
    let reservation = slots.begin(b2).unwrap();
    slots.finish(reservation, Some(c2)).unwrap();
    assert_eq!(slots.consumed(), 4);
    assert!(slots.begin(tag(&temp.path("d.kinewright"), 5)).is_err());
    // The ordinary-save API has no dependency on this full accounting ledger.
    let core = Core::spawn(document()).unwrap();
    let mut session = Ja1EditingSession::activate(
        &temp.path("ordinary.kinewright"),
        BundleWriteAdmission::new_document(),
        &core,
        None,
    )
    .unwrap();
    session.set_slots_probe(slots);
    for _ in 0..3 {
        edit(&core, false);
        session.save_active().unwrap();
    }
    assert_eq!(session.rescue_slots(), 4);
    assert!(!session.dirty().unwrap());
}

#[test]
fn w07_no_ticket_attempt_keeps_actual_namespace_owner() {
    let temp = temp("c3-no-ticket-owner");
    let path = temp.path("parent/a.kinewright");
    let core = Core::spawn(document()).unwrap();
    let mut session = Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
    assert!(matches!(
        io_observer::fail_once("root_parent_sync", || session
            .first_save(&path, StorageLimits::default()))
        .unwrap(),
        CopyStatus::DurabilityUnknown
    ));
    assert!(core.pending_history_save().unwrap().is_none());
    let before = core.export_history().unwrap();
    adopt_current(&mut session, &path, &temp.path("b.kinewright"));
    adopt_current(&mut session, &path, &temp.path("b2.kinewright"));
    assert_eq!(session.retained_attempts().len(), 1);
    assert!(session.retained_attempts()[0].ticket().is_none());
    session.reconcile_active().unwrap();
    assert_eq!(core.export_history().unwrap(), before);
    assert_eq!(session.rescue_slots(), 1);
    assert!(kinewright_project::namespace::ProjectNamespace::acquire(&path).is_err());
    session.save_active().unwrap();
    assert!(session.source().is_some());
}
#[test]
fn w07_repeated_source_attempts_do_not_deduplicate_by_destination() {
    let temp = temp("c3-repeated-slots");
    let path = temp.path("same.kinewright");
    let mut slots = RescueSlots::default();
    for attempt in 1..=3 {
        let reservation = slots.begin(tag(&path, attempt)).unwrap();
        slots.finish(reservation, None).unwrap();
        assert_eq!(slots.consumed(), usize::try_from(attempt).unwrap());
    }
    assert!(
        slots.begin(tag(&path, 4)).is_err(),
        "source plus destination must fit together"
    );
    fs::create_dir(temp.path("parent")).unwrap();
    let path = temp.path("parent/a.kinewright");
    let mut session = unknown(temp.root(), &path, true);
    let mut tags = vec![];
    let mut charges = vec![];
    for attempt in 1..=3 {
        let hidden = hide_parent(&path);
        let offer = session.probe_active().unwrap().unwrap();
        assert!(offer.ticket.is_some());
        tags.push(offer.tag.clone());
        let capture = session.prepare_rescue(&offer).unwrap().unwrap();
        let consent = capture.consent();
        let destination = session
            .inspect_rescue_destination(&temp.path(&format!("fresh-{attempt}.kinewright")))
            .unwrap();
        session
            .adopt_attempt_probe(capture, &consent, &destination, None)
            .unwrap();
        assert_eq!(session.rescue_slots(), attempt);
        assert_eq!(session.retained_attempts().len(), attempt);
        charges.push(
            session
                .source()
                .unwrap()
                .store()
                .usage()
                .unwrap()
                .reserved_copy_bytes,
        );
        restore_parent(&path, &hidden);
        session.resume_probe().unwrap();
        session.reconcile_active().unwrap();
        assert_eq!(session.rescue_slots(), attempt);
        assert_eq!(
            session.retained_attempts()[attempt - 1].publication_state(),
            kinewright_project::bundle::PublicationState::Committed
        );
        if attempt < 3 {
            grow(session.core());
            assert!(matches!(
                io_observer::fail_once("root_parent_sync", || session.save_active()),
                Err(BundleError::DurabilityUnknown)
            ));
        }
    }
    assert!(tags.windows(2).all(|pair| pair[0] != pair[1]));
    assert!(charges.windows(2).all(|pair| pair[1] > pair[0]));
    for (attempt, expected) in session.retained_attempts().iter().zip(tags) {
        assert_eq!(attempt.tag(), &expected);
        for object in attempt.reads().objects() {
            assert!(attempt.reads().open(object).is_ok());
        }
    }
}

#[test]
fn w08_actual_owners_survive_drop_settlement_and_reader_retirement() {
    let temp = temp("c3-owners");
    let path = temp.path("missing/parent/a.kinewright");
    let mut session = unknown(temp.root(), &path, true);
    let claim = temp.path(&format!(
        ".ja-s1-copy-parent-{}.kinewright",
        kinewright_media::sha256_bytes(path.as_os_str().as_encoded_bytes())
    ));
    assert!(kinewright_project::namespace::ProjectNamespace::acquire(&claim).is_err());
    let source_root = session.source().unwrap().store().resolved_root().unwrap();
    let source_files = session
        .source()
        .unwrap()
        .store()
        .manifest()
        .unwrap()
        .objects;
    let source_bytes = source_files
        .iter()
        .map(|r| read_bytes(session.source().unwrap().store(), r))
        .collect::<Vec<_>>();
    let source_usage = session.source().unwrap().store().usage_handle();
    let hidden = hide_parent(&path);
    let offer = session.probe_active().unwrap().unwrap();
    let capture = session.prepare_rescue(&offer).unwrap().unwrap();
    let consent = capture.consent();
    let inspection = session
        .inspect_rescue_destination(&temp.path("b.kinewright"))
        .unwrap();
    session
        .adopt_attempt_probe(capture, &consent, &inspection, None)
        .unwrap();
    assert_eq!(session.rescue_slots(), 1);
    assert_eq!(session.retained_attempts().len(), 1);
    let admission = session.retained_attempts()[0].reads();
    let reference = admission.objects()[0].clone();
    drop(admission.open(&reference).unwrap());
    drop(inspection);
    drop(consent);
    assert!(source_usage().unwrap().reserved_copy_bytes > 0);
    restore_parent(&path, &hidden);
    session.resume_probe().unwrap();
    let saved = session
        .association()
        .unwrap()
        .saved_digest()
        .map(str::to_owned);
    session.probe_active().unwrap();
    assert_eq!(
        session.association().unwrap().saved_digest(),
        saved.as_deref()
    );
    session.save_active().unwrap();
    session
        .store()
        .unwrap()
        .clear_verified_memo_probe()
        .unwrap();
    session
        .source()
        .unwrap()
        .store()
        .clear_verified_memo_probe()
        .unwrap();
    assert!(kinewright_project::namespace::ProjectNamespace::acquire(&claim).is_err());

    assert_eq!(session.rescue_slots(), 1);
    assert!(kinewright_project::namespace::ProjectNamespace::acquire(&path).is_err());
    for (r, bytes) in source_files.iter().zip(source_bytes) {
        assert_eq!(read_bytes(session.source().unwrap().store(), r), bytes);
    }
    assert!(source_root.parent().unwrap().exists());
    let retained = session.retained_attempts()[0].reads();
    let mut reader = retained.open(&reference).unwrap();
    let expected_read = read_bytes(session.source().unwrap().store(), &reference);
    eprintln!(
        "C3_OWNER_COST source={:?} destination={:?}",
        source_usage().unwrap(),
        session.store().unwrap().usage().unwrap()
    );
    drop(session);
    assert!(
        source_root.parent().unwrap().exists(),
        "the issued reader still owns source retention"
    );
    assert!(source_usage().unwrap().reserved_copy_bytes > 0);
    let mut read = vec![];
    reader.read_to_end(&mut read).unwrap();
    assert_eq!(read, expected_read);
    drop(reader);
    assert!(!source_root.parent().unwrap().exists());
    assert_eq!(source_usage().unwrap().reserved_copy_bytes, 0);
    assert!(!source_usage().unwrap().damaged_accounting);
    let mut claim = kinewright_project::namespace::ProjectNamespace::acquire(&claim).unwrap();
    claim.release().unwrap();
    let _next_writer = LoadedBundle::read(&path).unwrap().into_writer().unwrap();
}

#[test]
fn w09_lifecycle_continuity_refuses_changed_parent_and_foreign_root() {
    let temp = temp("c3-continuity");
    fs::create_dir(temp.path("parent")).unwrap();
    let path = temp.path("parent/a.kinewright");
    let mut session = unknown(temp.root(), &path, true);
    let ticket = session.core().pending_history_save().unwrap();
    let history = session.core().export_history().unwrap();
    let hidden = hide_parent(&path);
    fs::create_dir(path.parent().unwrap()).unwrap();
    fs::copy(hidden.join("a.kinewright"), &path).unwrap();
    assert!(session.reconcile_active().is_err());
    assert_eq!(session.core().pending_history_save().unwrap(), ticket);
    assert_eq!(session.core().export_history().unwrap(), history);
    fs::remove_file(&path).unwrap();
    fs::remove_dir(path.parent().unwrap()).unwrap();
    restore_parent(&path, &hidden);
    let own = fs::read(&path).unwrap();
    fs::write(&path, b"foreign bytes").unwrap();
    assert!(session.reconcile_active().is_err());
    assert_eq!(session.core().pending_history_save().unwrap(), ticket);
    fs::write(&path, own).unwrap();
    session.reconcile_active().unwrap();
    assert!(session.core().pending_history_save().unwrap().is_none());
}

#[test]
fn w10_lifecycle_no_ticket_destination_settlement_keeps_live_history() {
    for landed in [false, true] {
        for sealed in [false, true] {
            let temp = temp("c3-no-ticket");
            let path = temp.path("a.kinewright");
            let core = Core::spawn(document()).unwrap();
            let mut bundle =
                ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
            let inputs = bundle.empty_inputs().unwrap().reference.clone();
            bundle.save(&core, &inputs).unwrap();
            let issuer = bundle.issue_issuer(&core, &inputs, 1).unwrap();
            let attachment = bundle.attach_core(&core).unwrap();
            let command = CommandEnvelope {
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
                expected_revision: attachment.capture().unwrap().revision(),
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
            assert!(matches!(
                io_observer::fail_once(
                    if landed {
                        "root_parent_sync"
                    } else {
                        "root_rename_io"
                    },
                    || bundle.command(&command, &core, &inputs, &[])
                ),
                Err(BundleError::DurabilityUnknown)
            ));
            assert!(core.pending_history_save().unwrap().is_none());
            edit(&core, true);
            edit(&core, false);
            apply(&core, Command::Undo);
            if sealed {
                let cut = attachment.freeze().unwrap();
                attachment.seal(&cut).unwrap();
            }
            let before = core.export_history().unwrap();
            let mut session = Ja1EditingSession::from_bundle(bundle, &core, inputs).unwrap();
            assert!(session.probe_active().unwrap().is_none());
            session.reconcile_active().unwrap();
            assert_eq!(core.export_history().unwrap(), before);
            assert_eq!(session.rescue_slots(), 0);
            assert!(session.retained_attempts().is_empty());
            assert!(session.dirty().unwrap());
            drop(session);
            let bundle = LoadedBundle::read(&path).unwrap().into_writer().unwrap();
            assert!(if landed {
                matches!(
                    bundle.receipt_lookup(&command).unwrap(),
                    ReceiptLookup::Committed(_)
                )
            } else {
                matches!(
                    bundle.receipt_lookup(&command).unwrap(),
                    ReceiptLookup::Absent
                )
            });
        }
    }
}

#[test]
fn w16_first_save_a_writer_and_later_floor_continue() {
    for landed in [false, true] {
        for gesture in [false, true] {
            let temp = temp("c3-floor");
            let path = temp.path("parents/a.kinewright");
            let mut session = unknown(temp.root(), &path, landed);
            adopt_current(&mut session, &path, &temp.path("b.kinewright"));
            let core = session.core().clone();
            let selected = bound_history_probe(&core.export_history().unwrap(), &limits()).unwrap();
            assert!(selected.floor_offset > 0);
            edit(&core, true);
            edit(&core, false);
            if gesture {
                apply(
                    &core,
                    Command::DoBatchCoalesced {
                        operations: vec![Operation::SetTrackSyncLock {
                            track: TrackId(1),
                            locked: true,
                        }],
                        coalesce_key: "gesture".into(),
                    },
                );
            } else {
                apply(&core, Command::Undo);
            }
            let before = core.export_history().unwrap();
            let saved = session
                .association()
                .unwrap()
                .saved_digest()
                .map(str::to_owned);
            assert!(session.probe_active().unwrap().is_none());
            let expected = if landed {
                kinewright_core::HistoryTransition::floor(
                    before.clone(),
                    usize::try_from(selected.floor_offset).unwrap(),
                )
                .unwrap()
                .after()
                .clone()
            } else {
                before
            };
            assert_eq!(core.export_history().unwrap(), expected);
            assert!(core.pending_history_save().unwrap().is_none());
            assert_eq!(
                session.association().unwrap().saved_digest(),
                saved.as_deref()
            );
            assert!(session.dirty().unwrap());
            grow(&core);
            session.save_active().unwrap();
            assert!(!session.dirty().unwrap());
            let after = core.export_history().unwrap();
            assert!(after.floor_offset > expected.floor_offset);
            assert_eq!(session.rescue_slots(), 1);
            drop(session);
            let reopened = Ja1EditingSession::open(&path).unwrap();
            assert_eq!(reopened.core().export_history().unwrap(), after);
        }
    }
}

#[test]
fn w16_first_save_older_root_writer_reuses_retained_namespace() {
    let temp = temp("c3-older-root-writer");
    fs::create_dir(temp.path("parent")).unwrap();
    let path = temp.path("parent/a.kinewright");
    let prior = Core::spawn(document()).unwrap();
    let mut old = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    let inputs = old.empty_inputs().unwrap().reference.clone();
    old.save(&prior, &inputs).unwrap();
    let old_bytes = fs::read(&path).unwrap();
    drop(old);
    let mut session = unknown(temp.root(), &path, false);
    let core = session.core().clone();
    edit(&core, true);
    apply(&core, Command::Undo);
    let before = core.export_history().unwrap();
    adopt_current(&mut session, &path, &temp.path("b.kinewright"));
    session.reconcile_active().unwrap();
    assert_eq!(fs::read(&path).unwrap(), old_bytes);
    assert_eq!(core.export_history().unwrap(), before);
    assert!(core.pending_history_save().unwrap().is_none());
    assert!(session.dirty().unwrap());
    grow(&core);
    session.save_active().unwrap();
    assert!(session.source().is_some());
    assert!(!session.dirty().unwrap());
    let after = core.export_history().unwrap();
    assert!(after.floor_offset > before.floor_offset);
    drop(session);
    let reopened = Ja1EditingSession::open(&path).unwrap();
    assert_eq!(reopened.core().export_history().unwrap(), after);
}
