//! Round-five public recovery paths, with real namespace failures.
#![cfg(target_os = "linux")]
#![allow(clippy::too_many_lines)]
use kinewright_core::*;
use kinewright_media::test_support::TempDirectory;
use kinewright_project::{
    bundle::{BundleError, BundleWriteAdmission, LoadedBundle, ProjectBundle},
    bundle_commit::bound_history_probe,
    bundle_copy::{CopyStatus, SessionStore},
    io_observer,
    namespace::ProjectNamespace,
    owned_inputs::OwnedInputManifest,
    storage_budget::StorageLimits,
};
use std::path::Path;
fn temp(label: &str) -> TempDirectory {
    TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), label)
}
fn limits() -> StorageLimits {
    StorageLimits {
        branch_history_bytes: 1 << 20,
        ..StorageLimits::default()
    }
}
fn core() -> Core {
    Core::spawn(Document {
        tracks: (1..=200)
            .map(|id| Track {
                id: TrackId(id),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            })
            .collect(),
        ..Document::default()
    })
    .unwrap()
}
fn apply(core: &Core, command: Command) {
    assert!(matches!(
        core.request(command).unwrap(),
        Event::DocumentChanged { .. }
    ));
}
fn edit(core: &Core, locked: bool) {
    apply(
        core,
        Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked,
        }),
    );
}
fn grow(core: &Core) {
    for i in 0..40 {
        edit(core, i % 2 == 0);
    }
}
fn every_entry(core: &Core, before: &HistorySnapshot) {
    for entry in before.cursor.undo.iter().rev() {
        apply(core, Command::Undo);
        assert_eq!(
            core.export_history().unwrap().document,
            entry.document,
            "original Undo target unavailable"
        );
    }
    for _ in &before.cursor.undo {
        apply(core, Command::Redo);
    }
    assert_eq!(core.export_history().unwrap().document, before.document);
    for entry in before.cursor.redo.iter().rev() {
        apply(core, Command::Redo);
        assert_eq!(
            core.export_history().unwrap().document,
            entry.document,
            "original Redo target unavailable"
        );
    }
}
fn inputs(session: &mut SessionStore) -> kinewright_project::bundle_records::ObjectRef {
    session
        .admit_inputs(&OwnedInputManifest {
            project_id: None,
            session_id: Some(session.session_id().clone()),
            owner_epoch: session.store().owner_epoch(),
            entries: vec![],
        })
        .unwrap()
        .reference
        .clone()
}
fn start(
    path: &Path,
    core: &Core,
) -> (ProjectBundle, kinewright_project::bundle_records::ObjectRef) {
    let mut owner = ProjectBundle::activate(path, BundleWriteAdmission::new_document()).unwrap();
    let input = owner.empty_inputs().unwrap().reference.clone();
    owner.save(core, &input).unwrap();
    owner.restrict_limits_probe(limits()).unwrap();
    (owner, input)
}
fn reopen(path: &Path) -> ProjectBundle {
    LoadedBundle::read(path)
        .unwrap()
        .into_writer_with_limits_probe(limits())
        .unwrap()
}
fn exact(owner: &ProjectBundle, core: &Core) {
    assert_eq!(
        core.export_history().unwrap(),
        *owner.branch_history(owner.manual_branch_id()).unwrap()
    );
}

#[test]
fn pre_b5_object_uncertainty_releases_and_same_session_retries() {
    let temp = temp("r5-pre-b5");
    let core = core();
    grow(&core);
    for _ in 0..3 {
        apply(&core, Command::Undo);
    }
    let before = core.export_history().unwrap();
    assert!(
        bound_history_probe(&before, &limits())
            .unwrap()
            .floor_offset
            > 0
    );
    let mut session = SessionStore::create(temp.root(), StorageLimits::default()).unwrap();
    let input = inputs(&mut session);
    let path = temp.path("first.kinewright");
    let objects = temp.path("first.kinewright-assets/studio/objects");
    let mut copy = io_observer::fail_path("directory_child_sync", &objects, || {
        session.first_save_with_limits(&path, &core, &input, limits())
    })
    .unwrap();
    assert!(matches!(copy.status(), CopyStatus::DurabilityUnknown));
    assert!(!path.exists());
    assert!(
        core.pending_history_save().unwrap().is_none(),
        "pre-B5 uncertainty retained Core's barrier"
    );
    assert_eq!(core.export_history().unwrap(), before);
    every_entry(&core, &before);
    assert!(copy.reconcile().unwrap().is_none());
    drop(copy);
    let copy = session
        .first_save_with_limits(&path, &core, &input, limits())
        .expect("same session retry");
    let owner = copy
        .into_destination()
        .unwrap_or_else(|_| panic!("retry unsettled"));
    exact(&owner, &core);
}

#[test]
fn object_uncertainty_owner_drop_reopen_saves_without_losing_edits() {
    let temp = temp("r5-object-drop");
    let path = temp.path("edit.kinewright");
    let core = core();
    grow(&core);
    let before = core.export_history().unwrap();
    let mut session = SessionStore::create(temp.root(), StorageLimits::default()).unwrap();
    let input = inputs(&mut session);
    let objects = temp.path("edit.kinewright-assets/studio/objects");
    let mut copy = io_observer::fail_path("directory_child_sync", &objects, || {
        session.first_save_with_limits(&path, &core, &input, limits())
    })
    .unwrap();
    assert!(matches!(copy.status(), CopyStatus::DurabilityUnknown));
    assert!(core.pending_history_save().unwrap().is_none());
    assert_eq!(core.export_history().unwrap(), before);
    assert!(io_observer::fail_path("directory_child_sync", &objects, || copy.reconcile()).is_err());
    drop(copy);
    let mut owner = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    owner.restrict_limits_probe(limits()).unwrap();
    let input = owner.empty_inputs().unwrap().reference.clone();
    owner.save(&core, &input).unwrap();
    exact(&owner, &core);
}

fn orphan(own: bool, branch: bool) {
    let temp = temp("r5-orphan");
    let path = temp.path("edit.kinewright");
    let core = core();
    let (mut owner, input) = start(&path, &core);
    let branch_id = if branch {
        owner
            .fork_branch(&owner.manual_branch_id().clone())
            .unwrap()
    } else {
        owner.manual_branch_id().clone()
    };
    grow(&core);
    let before = core.export_history().unwrap();
    let selected = bound_history_probe(&before, &limits()).unwrap();
    let point = if own {
        "root_parent_sync"
    } else {
        "root_rename_io"
    };
    assert!(matches!(
        io_observer::fail_once(point, || if branch {
            owner.save_branch(&branch_id, &core)
        } else {
            owner.save(&core, &input)
        }),
        Err(BundleError::DurabilityUnknown)
    ));
    let pending = core
        .pending_history_save()
        .unwrap()
        .expect("B5 retains ticket");
    assert!(pending.ready && pending.tag.is_some());
    drop(owner);
    edit(&core, false);
    apply(&core, Command::Undo);
    let current = core.export_history().unwrap();
    let expected = if own {
        HistoryTransition::floor(current, usize::try_from(selected.floor_offset).unwrap())
            .unwrap()
            .after()
            .clone()
    } else {
        current
    };
    let mut owner = reopen(&path);
    let moving = core.clone();
    io_observer::at_barrier(
        "history_selected",
        move || {
            assert_eq!(
                moving.export_history().unwrap(),
                expected,
                "fresh owner must settle against current history before selecting again"
            );
            assert!(moving.pending_history_save().unwrap().is_none());
        },
        || {
            if branch {
                owner.save_branch(&branch_id, &core)
            } else {
                owner.save(&core, &input)
            }
        },
    )
    .unwrap();
    assert_eq!(
        core.export_history().unwrap(),
        *owner.branch_history(&branch_id).unwrap()
    );
}
#[test]
fn orphan_equal_root_commits_with_new_owner() {
    orphan(true, false);
}
#[test]
fn orphan_older_root_releases_with_new_owner() {
    orphan(false, false);
}
#[test]
fn orphan_branch_save_cannot_stack_a_barrier() {
    orphan(true, true);
}

#[test]
fn orphan_absent_root_releases_and_retries() {
    let temp = temp("r5-absent");
    let path = temp.path("first.kinewright");
    let core = core();
    grow(&core);
    let mut owner = ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
    let input = owner.empty_inputs().unwrap().reference.clone();
    owner.restrict_limits_probe(limits()).unwrap();
    let before = core.export_history().unwrap();
    assert!(matches!(
        io_observer::fail_once("root_rename_io", || owner.save(&core, &input)),
        Err(BundleError::DurabilityUnknown)
    ));
    assert!(!path.exists());
    assert!(owner.reconcile().unwrap().is_none());
    assert!(core.pending_history_save().unwrap().is_none());
    assert_eq!(core.export_history().unwrap(), before);
    owner.save(&core, &input).unwrap();
    exact(&owner, &core);
}

fn save_as(unreadable: bool, copy_owner: bool) {
    let temp = temp("r5-save-as");
    std::fs::create_dir(temp.path("A")).unwrap();
    let a = temp.path("A/edit.kinewright");
    let b = temp.path("B/edit.kinewright");
    let core = core();
    let mut session = SessionStore::create(temp.root(), StorageLimits::default()).unwrap();
    let input = inputs(&mut session);
    if copy_owner {
        grow(&core);
        let copy = io_observer::fail_once("root_parent_sync", || {
            session.first_save_with_limits(&a, &core, &input, limits())
        })
        .unwrap();
        assert!(matches!(copy.status(), CopyStatus::DurabilityUnknown));
        drop(copy);
    } else {
        let (mut owner, input) = start(&a, &core);
        grow(&core);
        assert!(matches!(
            io_observer::fail_once("root_parent_sync", || owner.save(&core, &input)),
            Err(BundleError::DurabilityUnknown)
        ));
        drop(owner);
    }
    let tag = core.pending_history_save().unwrap().unwrap();
    let selected = bound_history_probe(&core.export_history().unwrap(), &limits()).unwrap();
    if unreadable {
        std::fs::rename(temp.path("A"), temp.path("hidden-A")).unwrap();
        let before = core.export_history().unwrap();
        let (result, trace) =
            io_observer::observe(|| session.first_save_with_limits(&b, &core, &input, limits()));
        assert!(matches!(result, Err(BundleError::DurabilityUnknownAt(ref path)) if path == &a));
        assert!(
            trace.events.is_empty(),
            "unreadable A must refuse before any mutation: {:?}",
            trace.events
        );
        assert!(!temp.path("B").exists());
        assert_eq!(core.pending_history_save().unwrap(), Some(tag.clone()));
        assert_eq!(core.export_history().unwrap(), before);
        edit(&core, false);
        apply(&core, Command::Undo);
        std::fs::rename(temp.path("hidden-A"), temp.path("A")).unwrap();
    }
    let observed = core.clone();
    let floor = selected.floor_offset;
    let old_ticket = tag.id;
    let copy = io_observer::at_barrier(
        "copy_destination_pinned",
        move || {
            assert!(
                observed
                    .pending_history_save()
                    .unwrap()
                    .is_none_or(|ticket| ticket.id != old_ticket && ticket.tag.is_none()),
                "A must settle before B preparation"
            );
            assert_eq!(observed.export_history().unwrap().floor_offset, floor);
        },
        || session.first_save_with_limits(&b, &core, &input, limits()),
    )
    .unwrap();
    let owner = copy
        .into_destination()
        .unwrap_or_else(|_| panic!("Save As unsettled"));
    exact(&owner, &core);
}
#[test]
fn readable_orphan_a_reconciles_before_save_as_b() {
    save_as(false, false);
}
#[test]
fn unreadable_orphan_a_refuses_save_as_b_without_mutations_then_retries() {
    save_as(true, false);
}
#[test]
fn dropped_first_copy_can_save_as_in_the_same_session() {
    save_as(true, true);
}

#[test]
fn destination_contention_drops_barrier_and_preserves_every_entry() {
    let temp = temp("r5-contention");
    let path = temp.path("edit.kinewright");
    let core = core();
    grow(&core);
    for _ in 0..3 {
        apply(&core, Command::Undo);
    }
    let before = core.export_history().unwrap();
    let mut held = ProjectNamespace::acquire(&path).unwrap();
    let mut session = SessionStore::create(temp.root(), StorageLimits::default()).unwrap();
    let input = inputs(&mut session);
    assert!(matches!(
        session.first_save_with_limits(&path, &core, &input, limits()),
        Err(BundleError::Contention(_))
    ));
    assert!(
        core.pending_history_save().unwrap().is_none(),
        "drop must release unretained barrier"
    );
    every_entry(&core, &before);
    held.release().unwrap();
    let copy = session
        .first_save_with_limits(&path, &core, &input, limits())
        .unwrap();
    let owner = copy
        .into_destination()
        .unwrap_or_else(|_| panic!("contention retry unsettled"));
    exact(&owner, &core);
}
#[test]
fn command_during_selection_is_a_retryable_conflict() {
    let temp = temp("r5-capture");
    let core = core();
    let path = temp.path("edit.kinewright");
    let (mut owner, input) = start(&path, &core);
    grow(&core);
    let moving = core.clone();
    let (result, trace) = io_observer::observe(|| {
        io_observer::at_barrier(
            "history_selected",
            move || edit(&moving, false),
            || owner.save(&core, &input),
        )
    });
    assert!(
        matches!(result, Err(BundleError::HistoryChanged)),
        "{result:?}"
    );
    assert!(trace.events.is_empty());
    owner.save(&core, &input).unwrap();
    exact(&owner, &core);
}
#[test]
fn selected_floor_never_precedes_the_durable_floor() {
    let temp = temp("r5-alignment");
    let core = core();
    let path = temp.path("edit.kinewright");
    let (mut owner, input) = start(&path, &core);
    grow(&core);
    let original = core.export_history().unwrap();
    owner.save(&core, &input).unwrap();
    let durable = core.export_history().unwrap();
    assert!(durable.floor_offset > 0);
    core.restore_history(original).unwrap();
    // A larger cap would otherwise select the untrimmed cut below the durable floor.
    owner
        .restrict_limits_probe(StorageLimits::default())
        .unwrap();
    owner.save(&core, &input).unwrap();
    assert_eq!(core.export_history().unwrap(), durable);
    exact(&owner, &core);
}

#[test]
fn reopen_settles_the_orphan_before_restore() {
    let temp = temp("r5-reopen");
    let path = temp.path("edit.kinewright");
    let core = core();
    let (mut owner, input) = start(&path, &core);
    grow(&core);
    assert!(matches!(
        io_observer::fail_once("root_parent_sync", || owner.save(&core, &input)),
        Err(BundleError::DurabilityUnknown)
    ));
    drop(owner);
    let loaded = LoadedBundle::read(&path).unwrap();
    loaded
        .restore_manual(&core)
        .expect("reopen must settle before restore");
    assert!(core.pending_history_save().unwrap().is_none());
    let mut owner = loaded.into_writer().unwrap();
    owner.save(&core, &input).unwrap();
    exact(&owner, &core);
}

#[test]
fn frozen_save_as_settles_its_source_core_before_copying() {
    let temp = temp("r5-frozen-copy");
    let path = temp.path("edit.kinewright");
    let core = core();
    let (mut owner, input) = start(&path, &core);
    grow(&core);
    assert!(matches!(
        io_observer::fail_once("root_parent_sync", || owner.save(&core, &input)),
        Err(BundleError::DurabilityUnknown)
    ));
    let outcome = owner
        .save_as(&temp.path("copied.kinewright"))
        .expect("frozen copy must reconcile its source");
    let kinewright_project::bundle_copy::SaveAsOutcome::Copied(copy) = outcome else {
        panic!("new destination");
    };
    assert!(core.pending_history_save().unwrap().is_none());
    let destination = copy
        .into_destination()
        .unwrap_or_else(|_| panic!("copy unsettled"));
    exact(&destination, &core);
}

#[test]
fn ordinary_save_object_uncertainty_releases_then_new_owner_saves() {
    let temp = temp("r5-ordinary-object");
    let path = temp.path("edit.kinewright");
    let core = core();
    let (mut owner, input) = start(&path, &core);
    grow(&core);
    for _ in 0..3 {
        apply(&core, Command::Undo);
    }
    let before = core.export_history().unwrap();
    let published = std::fs::read(&path).unwrap();
    assert!(
        bound_history_probe(&before, &limits())
            .unwrap()
            .floor_offset
            > before.floor_offset
    );
    // Object directory barriers are keyed by their bundle-relative path.
    let objects = Path::new("studio/objects");
    assert!(matches!(
        io_observer::fail_path("object_directory_sync", objects, || owner
            .save(&core, &input)),
        Err(BundleError::DurabilityUnknown)
    ));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        published,
        "the uncertain object write preceded root publication"
    );
    assert!(
        core.pending_history_save().unwrap().is_none(),
        "ordinary-save object uncertainty retained Core's barrier"
    );
    // The owner cannot finish its object barrier while the fault persists. That stays
    // the store's business and no longer gates Core.
    assert!(
        io_observer::fail_path("object_directory_sync", objects, || owner.reconcile()).is_err()
    );
    every_entry(&core, &before);
    core.restore_history(before.clone()).unwrap();
    drop(owner);
    let mut owner = reopen(&path);
    owner.save(&core, &input).unwrap();
    exact(&owner, &core);
}

#[test]
fn recreated_parent_a_is_a_definite_release_before_save_as_b() {
    let temp = temp("r5-recreated");
    std::fs::create_dir(temp.path("A")).unwrap();
    let a = temp.path("A/edit.kinewright");
    let b = temp.path("B/edit.kinewright");
    let core = core();
    let mut session = SessionStore::create(temp.root(), StorageLimits::default()).unwrap();
    let input = inputs(&mut session);
    let (mut owner, owner_input) = start(&a, &core);
    grow(&core);
    assert!(matches!(
        io_observer::fail_once("root_parent_sync", || owner.save(&core, &owner_input)),
        Err(BundleError::DurabilityUnknown)
    ));
    drop(owner);
    assert!(core.pending_history_save().unwrap().unwrap().ready);
    let before = core.export_history().unwrap();
    // The published A moves away and a new, empty A takes its path.
    std::fs::rename(temp.path("A"), temp.path("moved-A")).unwrap();
    std::fs::create_dir(temp.path("A")).unwrap();
    let observed = core.clone();
    let copy = io_observer::at_barrier(
        "copy_destination_pinned",
        move || {
            assert!(
                observed
                    .pending_history_save()
                    .unwrap()
                    .is_none_or(|ticket| ticket.tag.is_none()),
                "A must settle before B preparation"
            );
            assert_eq!(
                observed.export_history().unwrap(),
                before,
                "a replaced parent must release every entry, never commit"
            );
        },
        || session.first_save_with_limits(&b, &core, &input, limits()),
    )
    .expect("a readable replaced parent is a definite reconciliation");
    let owner = copy
        .into_destination()
        .unwrap_or_else(|_| panic!("Save As unsettled"));
    exact(&owner, &core);
}

#[test]
fn published_owner_does_not_accept_a_vanished_root_as_unpublished() {
    let temp = temp("r5-vanished");
    let path = temp.path("edit.kinewright");
    let core = core();
    let (mut owner, input) = start(&path, &core);
    grow(&core);
    let before = core.export_history().unwrap();
    assert!(matches!(
        io_observer::fail_once("root_rename_io", || owner.save(&core, &input)),
        Err(BundleError::DurabilityUnknown)
    ));
    std::fs::remove_file(&path).unwrap();
    assert!(
        owner.reconcile().is_err(),
        "a previously published owner must not treat a vanished root as its original"
    );
    // Core settles independently of the owner: the readable parent holds no tagged root.
    assert!(core.pending_history_save().unwrap().is_none());
    assert_eq!(core.export_history().unwrap(), before);
}
