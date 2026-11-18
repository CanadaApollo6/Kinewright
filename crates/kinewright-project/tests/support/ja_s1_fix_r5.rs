//! Round-five public recovery paths, with real namespace failures.
#![cfg(target_os = "linux")]
#![allow(clippy::too_many_lines)]
use kinewright_core::*;
use kinewright_media::test_support::TempDirectory;
use kinewright_project::operation_actor::Core;
use kinewright_project::{
    bundle::{BundleError, BundleWriteAdmission, LoadedBundle, ProjectBundle},
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
        ..StorageLimits::default()
    }
}
fn core() -> Core {
    Core::for_test(Document {
        tracks: (1..=200)
            .map(|id| Track {
                id: TrackId(id),
                kind: TrackKind::Video,
                name: None,
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
fn every_entry(core: &Core, before: &kinewright_project::operation_history::OwnerSnapshot) {
    // Independent operation replay, including the saved Redo tail.
    let mut document = before.history().baseline.as_ref().clone();
    let mut boundaries = vec![document.clone()];
    for step in before
        .history()
        .undo
        .iter()
        .chain(before.history().redo.iter().rev())
    {
        for command in step.commands() {
            for operation in &command.operations {
                operation.apply(&mut document).unwrap();
            }
        }
        boundaries.push(document.clone());
    }
    let depth = before.history().undo.len();
    for at in (0..depth).rev() {
        apply(core, Command::Undo);
        assert_eq!(
            core.export_history().unwrap().history().document.as_ref(),
            &boundaries[at],
            "ORIGINAL_UNDO_TARGET_SURVIVES_PUBLICATION_FAILURE"
        );
    }
    for _ in 0..depth {
        apply(core, Command::Redo);
    }
    assert_eq!(
        core.export_history().unwrap().history().document,
        before.history().document
    );
    for document in &boundaries[depth + 1..] {
        apply(core, Command::Redo);
        assert_eq!(
            core.export_history().unwrap().history().document.as_ref(),
            document,
            "ORIGINAL_REDO_TARGET_SURVIVES_PUBLICATION_FAILURE"
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
    let mut owner = ProjectBundle::activate(
        path,
        BundleWriteAdmission::new_document(),
        core.history_pool(),
    )
    .unwrap();
    let input = owner.empty_inputs().unwrap().reference.clone();
    owner.save(core, &input).unwrap();
    owner.restrict_limits_probe(limits()).unwrap();
    (owner, input)
}
fn reopen(path: &Path, pool: &kinewright_project::operation_history::HistoryPool) -> ProjectBundle {
    LoadedBundle::read(path, pool)
        .unwrap()
        .into_writer_with_limits_probe(limits())
        .unwrap()
}
fn exact(owner: &ProjectBundle, core: &Core) {
    let source_identity = core.snapshot_identity().unwrap();
    let selected = owner.branch_history(owner.manual_branch_id()).unwrap();
    if selected.identity().unwrap() == source_identity {
        return;
    }
    let selected = serde_json::to_value(selected.into_capture().unwrap()).unwrap();
    let source = core.export_history().unwrap();
    let mut expected = serde_json::to_value(&source).unwrap();
    assert_eq!(
        selected["history"]["epoch"].as_u64().unwrap(),
        source.history().epoch + 1,
        "FIRST_COPY_SELECTS_ONE_FRESH_INSTALL_EPOCH"
    );
    expected["history"]["epoch"] = selected["history"]["epoch"].clone();
    assert!(
        expected == selected,
        "FIRST_COPY_PRESERVES_THE_COMPLETE_OWNER_EXCEPT_ITS_SELECTED_EPOCH"
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
    assert_eq!(
        before.history().undo.len() + before.history().redo.len(),
        40
    );
    let mut session = SessionStore::create(temp.root(), StorageLimits::default()).unwrap();
    let input = inputs(&mut session);
    let path = temp.path("first.kinewright");
    let objects = temp.path("first.kinewright-assets/studio/objects");
    let mut copy = io_observer::fail_path("directory_child_sync", &objects, || {
        session.first_save_with_limits(&path, &core, &input, limits())
    })
    .unwrap();
    assert!(matches!(copy.status(), CopyStatus::CleanupPending { .. }));
    assert!(!path.exists());
    assert!(
        core.attachment_state().unwrap() == AttachmentState::Live,
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
    assert!(matches!(copy.status(), CopyStatus::CleanupPending { .. }));
    assert_eq!(core.attachment_state().unwrap(), AttachmentState::Live);
    assert_eq!(core.export_history().unwrap(), before);
    assert!(io_observer::fail_path("directory_child_sync", &objects, || copy.reconcile()).is_err());
    drop(copy);
    let mut owner = ProjectBundle::activate(
        &path,
        BundleWriteAdmission::new_document(),
        core.history_pool(),
    )
    .unwrap();
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
            .fork_branch(
                &owner.manual_branch_id().clone(),
                kinewright_project::operation_history::Role::Ordinary,
            )
            .unwrap()
    } else {
        owner.manual_branch_id().clone()
    };
    grow(&core);
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
    assert!(
        owner.pending_attempt().is_some(),
        "UNKNOWN_PUBLICATION_RETAINS_ITS_EXACT_OWNER_CUT"
    );
    drop(owner);
    edit(&core, false);
    apply(&core, Command::Undo);
    let expected = core.snapshot_identity().unwrap();
    let mut owner = reopen(&path, core.history_pool());
    if branch {
        owner.save_branch(&branch_id, &core).unwrap();
    } else {
        owner.save(&core, &input).unwrap();
    }
    assert_eq!(
        core.snapshot_identity().unwrap(),
        expected,
        "REOPENED_PUBLISHER_NEVER_REPLACES_LATER_ACCEPTED_WORK"
    );
    assert_eq!(
        core.export_history().unwrap(),
        owner
            .branch_history(&branch_id)
            .unwrap()
            .into_capture()
            .unwrap()
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
    let mut owner = ProjectBundle::activate(
        &path,
        BundleWriteAdmission::new_document(),
        core.history_pool(),
    )
    .unwrap();
    let input = owner.empty_inputs().unwrap().reference.clone();
    owner.restrict_limits_probe(limits()).unwrap();
    let before = core.export_history().unwrap();
    assert!(matches!(
        io_observer::fail_once("root_rename_io", || owner.save(&core, &input)),
        Err(BundleError::DurabilityUnknown)
    ));
    assert!(!path.exists());
    assert!(owner.reconcile().unwrap().is_none());
    assert_eq!(core.attachment_state().unwrap(), AttachmentState::Live);
    assert_eq!(core.export_history().unwrap(), before);
    owner.save(&core, &input).unwrap();
    exact(&owner, &core);
}

fn save_as(unreadable: bool, copy_owner: bool) {
    use kinewright_project::editing_session::Ja1EditingSession;
    let temp = temp("r5-save-as");
    std::fs::create_dir(temp.path("A")).unwrap();
    let a = temp.path("A/edit.kinewright");
    let b = temp.path("B/edit.kinewright");
    let core = core();
    let mut owner = Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
    if copy_owner {
        grow(&core);
        let status =
            io_observer::fail_once("root_parent_sync", || owner.first_save(&a, limits())).unwrap();
        assert!(matches!(status, CopyStatus::DurabilityUnknown));
    } else {
        owner.first_save(&a, limits()).unwrap();
        grow(&core);
        assert!(matches!(
            io_observer::fail_once("root_parent_sync", || owner.save_active()),
            Err(BundleError::DurabilityUnknown)
        ));
    }
    let before = core.snapshot_identity().unwrap();
    if unreadable {
        std::fs::rename(temp.path("A"), temp.path("hidden-A")).unwrap();
        assert!(
            owner.save_as(&b, limits()).is_err(),
            "UNREADABLE_PENDING_A_CANNOT_BE_ACKNOWLEDGED_BY_SAVE_AS_B"
        );
        assert!(!b.exists());
        assert_eq!(
            core.snapshot_identity().unwrap(),
            before,
            "REFUSED_SAVE_AS_KEEPS_ALL_UNSAVED_WORK"
        );
        std::fs::rename(temp.path("hidden-A"), temp.path("A")).unwrap();
    }
    owner.reconcile_active().unwrap();
    assert!(matches!(
        owner.save_as(&b, limits()).unwrap(),
        CopyStatus::Committed(_)
    ));
    let selected = core.snapshot_identity().unwrap();
    drop(owner);
    let reopened = Ja1EditingSession::open(
        &b,
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .unwrap();
    assert_eq!(
        reopened.core().snapshot_identity().unwrap(),
        selected,
        "SAVE_AS_AFTER_SETTLEMENT_COLD_OPENS_EXACTLY"
    );
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
        core.attachment_state().unwrap() == AttachmentState::Live,
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
    let selected = core.snapshot_identity().unwrap();
    let moving = core.clone();
    let witnessed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let witness = std::sync::Arc::clone(&witnessed);
    let result = io_observer::at_barrier(
        "bundle_B0",
        move || {
            witness.store(true, std::sync::atomic::Ordering::SeqCst);
            edit(&moving, false);
        },
        || owner.save(&core, &input),
    );
    result.unwrap();
    assert!(
        witnessed.load(std::sync::atomic::Ordering::SeqCst),
        "CONCURRENT_EDIT_REACHED_REAL_PUBLICATION_BARRIER"
    );
    assert_eq!(
        owner
            .branch_history(owner.manual_branch_id())
            .unwrap()
            .identity()
            .unwrap(),
        selected,
        "PUBLISHED_CUT_DOES_NOT_INCLUDE_LATER_EDIT"
    );
    assert_ne!(
        core.snapshot_identity().unwrap(),
        selected,
        "LATER_EDIT_STAYS_IN_LIVE_CORE"
    );
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
    let original = core.snapshot_identity().unwrap();
    owner.save(&core, &input).unwrap();
    assert_eq!(
        core.snapshot_identity().unwrap(),
        original,
        "SAVE_NEVER_SELECTS_A_DIFFERENT_EXECUTOR_FLOOR"
    );
    owner
        .restrict_limits_probe(StorageLimits::default())
        .unwrap();
    owner.save(&core, &input).unwrap();
    assert_eq!(
        core.snapshot_identity().unwrap(),
        original,
        "STORAGE_LIMIT_CHANGE_DOES_NOT_REWRITE_EXECUTOR_HISTORY"
    );
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
    let mut loaded = LoadedBundle::read(
        &path,
        &kinewright_project::operation_history::HistoryPool::new(
            kinewright_project::operation_history::PoolLimits::default(),
        ),
    )
    .unwrap();
    let core = loaded
        .open_manual()
        .expect("reopen must settle before opening the saved owner");
    assert_eq!(core.attachment_state().unwrap(), AttachmentState::Live);
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
    assert_eq!(core.attachment_state().unwrap(), AttachmentState::Live);
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
    assert_eq!(
        before.history().undo.len() + before.history().redo.len(),
        40
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
        core.attachment_state().unwrap() == AttachmentState::Live,
        "ordinary-save object uncertainty retained Core's barrier"
    );
    // The owner cannot finish its object barrier while the fault persists. That stays
    // the store's business and no longer gates Core.
    assert!(
        io_observer::fail_path("object_directory_sync", objects, || owner.reconcile()).is_err()
    );
    every_entry(&core, &before);
    let attachment = owner.attach_core(&core).unwrap();
    let cut = attachment.freeze().unwrap();
    attachment
        .install_snapshot(&cut, before.validate().unwrap(), None)
        .unwrap();
    let installed = attachment.freeze().unwrap();
    attachment.resume(&installed).unwrap();
    drop(owner);
    let mut owner = reopen(&path, core.history_pool());
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
    assert_eq!(core.attachment_state().unwrap(), AttachmentState::Live);
    let before = core.snapshot_identity().unwrap();
    // The published A moves away and a new, empty A takes its path.
    std::fs::rename(temp.path("A"), temp.path("moved-A")).unwrap();
    std::fs::create_dir(temp.path("A")).unwrap();
    let observed = core.clone();
    let copy = io_observer::at_barrier(
        "copy_destination_pinned",
        move || {
            assert_eq!(
                observed.attachment_state().unwrap(),
                AttachmentState::Live,
                "REPLACED_PARENT_CANNOT_RETAIN_AN_EXECUTOR_TICKET"
            );
            assert_eq!(
                observed.snapshot_identity().unwrap(),
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
    let before = core.snapshot_identity().unwrap();
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
    assert_eq!(core.attachment_state().unwrap(), AttachmentState::Live);
    assert_eq!(core.snapshot_identity().unwrap(), before);
}
