//! C2/C3 production controls: no rescue publication or UI handoff is claimed.
#![cfg(feature = "test-util")]
#![allow(clippy::too_many_lines)]
use kinewright_core::{
    Command, Core, CoreMutationRefusal, Document, Event, Operation, Track, TrackId, TrackKind,
};
#[cfg(target_os = "linux")]
use kinewright_project::editing_session::Ja1EditingSession;
#[cfg(target_os = "linux")]
#[path = "support/ja_s2_c3.rs"]
mod c3;

#[cfg(not(target_os = "linux"))]
#[test]
fn c3_unsupported_new_context_refuses_before_source_mutation() {
    use kinewright_media::test_support::TempDirectory;
    let temp = TempDirectory::new("c3-unsupported");
    let core = Core::spawn(document()).unwrap();
    let (result, trace) = kinewright_project::io_observer::observe(|| {
        kinewright_project::editing_session::Ja1EditingSession::new(
            temp.root(),
            &core,
            kinewright_project::storage_budget::StorageLimits::default(),
        )
    });
    assert!(matches!(
        result,
        Err(kinewright_project::bundle::BundleError::DurabilityUnsupported)
    ));
    assert!(trace.events.is_empty());
    assert_eq!(std::fs::read_dir(temp.root()).unwrap().count(), 0);
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
    apply(
        core,
        Command::Do(Operation::SetTrackSyncLock {
            track: TrackId(1),
            locked,
        }),
    );
}
fn apply(core: &Core, command: Command) {
    assert!(matches!(
        core.request(command).unwrap(),
        Event::DocumentChanged { .. }
    ));
}
#[test]
fn w01_separate_actor_and_eager_binding_control() {
    struct ForeignProof;
    let a = Core::spawn(document()).unwrap();
    let b = Core::spawn(document()).unwrap();
    assert_ne!(a.actor_identity(), b.actor_identity());
    assert_eq!(a.actor_identity(), a.clone().actor_identity());
    #[cfg(target_os = "linux")]
    {
        use kinewright_media::test_support::TempDirectory;
        use kinewright_project::bundle::{BundleWriteAdmission, ProjectBundle};
        let temp = TempDirectory::new_in(
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")),
            "s2-actors",
        );
        let owner_a = ProjectBundle::activate(
            &temp.path("a.kinewright"),
            BundleWriteAdmission::new_document(),
        )
        .unwrap();
        let owner_b = ProjectBundle::activate(
            &temp.path("b.kinewright"),
            BundleWriteAdmission::new_document(),
        )
        .unwrap();
        assert_ne!(owner_a.project_id(), owner_b.project_id());
        let attachment_a = owner_a.attach_core(&a).unwrap();
        let attachment_b = owner_b.attach_core(&b).unwrap();
        assert!(a.clone().history_save_owner(&ForeignProof).is_err());
        assert!(b.clone().history_save_owner(&ForeignProof).is_err());
        edit(&a, false);
        let captured = a.export_history().unwrap();
        attachment_b.restore_history(captured.clone()).unwrap();
        let cut = attachment_a.freeze().unwrap();
        attachment_a.seal(&cut).unwrap();
        edit(&b, true);
        assert_eq!(a.export_history().unwrap(), captured);
        assert_ne!(a.export_history().unwrap(), b.export_history().unwrap());
        assert!(matches!(
            a.request(Command::Undo).unwrap(),
            Event::MutationRefused {
                reason: CoreMutationRefusal::Sealed,
                ..
            }
        ));
    }
    #[cfg(not(target_os = "linux"))]
    {
        let authority = a.history_save_owner(&ForeignProof).unwrap();
        edit(&a, false);
        let cut = authority.freeze(&ForeignProof).unwrap();
        authority.seal(&ForeignProof, &cut).unwrap();
        assert!(matches!(
            a.request(Command::Undo).unwrap(),
            Event::MutationRefused {
                reason: CoreMutationRefusal::Sealed,
                ..
            }
        ));
        assert_ne!(a.export_history().unwrap(), b.export_history().unwrap());
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use kinewright_media::test_support::TempDirectory;
    use kinewright_project::{
        bundle::{BundleError, BundleWriteAdmission, LoadedBundle, ProjectBundle},
        bundle_commit::bound_history_probe,
        bundle_copy::{BundleCopy, CopyStatus, SessionStore},
        bundle_records::ObjectRef,
        io_observer,
        owned_inputs::OwnedInputManifest,
        receipts::{CommandEnvelope, CommandId, ReceiptLookup},
        storage_budget::StorageLimits,
    };
    use std::{fs, path::Path};
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
    fn new_copy(
        temp: &TempDirectory,
        path: &Path,
        core: &Core,
        landed: bool,
    ) -> (SessionStore, BundleCopy) {
        let mut session = SessionStore::create(temp.root(), StorageLimits::default()).unwrap();
        let inputs = session
            .admit_inputs(&OwnedInputManifest {
                project_id: None,
                session_id: Some(session.session_id().clone()),
                owner_epoch: session.store().owner_epoch(),
                entries: vec![],
            })
            .unwrap()
            .reference
            .clone();
        let mut copy = io_observer::fail_once(
            if landed {
                "root_parent_sync"
            } else {
                "root_rename_io"
            },
            || session.first_save_with_limits(path, core, &inputs, limits()),
        )
        .unwrap();
        assert!(matches!(copy.status(), CopyStatus::DurabilityUnknown));
        assert!(core.pending_history_save().unwrap().unwrap().ready);
        // Control: unrelated destination reconciliation is receipt-only by default.
        assert_eq!(
            copy.pending_attempt().unwrap().actor(),
            core.actor_identity()
        );
        let before = core.export_history().unwrap();
        copy.reconcile_destination(None).unwrap();
        assert_eq!(core.export_history().unwrap(), before);
        assert!(core.pending_history_save().unwrap().is_some());
        (session, copy)
    }
    #[test]
    fn w16_exact_attempt_ticket_commit_release_control() {
        for landed in [false, true] {
            for gesture in [false, true] {
                let temp = temp("s2-exact-attempt");
                let path = temp.path("a.kinewright");
                let (core, mut owner, inputs) = setup(&path);
                owner.restrict_limits_probe(limits()).unwrap();
                grow(&core);
                let original = core.export_history().unwrap();
                let selected = bound_history_probe(&original, &limits()).unwrap();
                assert!(selected.floor_offset > 0);
                assert!(matches!(
                    io_observer::fail_once(
                        if landed {
                            "root_parent_sync"
                        } else {
                            "root_rename_io"
                        },
                        || owner.save(&core, &inputs)
                    ),
                    Err(BundleError::DurabilityUnknown)
                ));
                let attempt = owner.pending_attempt().unwrap();
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
                            coalesce_key: "live-gesture".into(),
                        },
                    );
                } else {
                    apply(&core, Command::Undo);
                }
                let before = core.export_history().unwrap();
                owner.reconcile_destination(Some(&core)).unwrap();
                assert!(core.pending_history_save().unwrap().is_none());
                let settled = core.export_history().unwrap();
                let expected = if landed {
                    kinewright_core::HistoryTransition::floor(
                        before.clone(),
                        usize::try_from(selected.floor_offset).unwrap(),
                    )
                    .unwrap()
                    .after()
                    .clone()
                } else {
                    before.clone()
                };
                assert_eq!(settled, expected);
                owner.reconcile_destination(Some(&core)).unwrap();
                assert_eq!(core.export_history().unwrap(), settled);
                let attachment = owner.attach_core(&core).unwrap();
                grow(&core);
                let result =
                    io_observer::fail_once("root_parent_sync", || owner.save(&core, &inputs));
                assert!(matches!(result, Err(BundleError::DurabilityUnknown)));
                let newer = core.pending_history_save().unwrap().unwrap();
                assert_ne!(newer.id, attempt.ticket().id);
                let history = core.export_history().unwrap();
                assert!(attachment.settle_attempt(&attempt).is_err());
                assert_eq!(core.pending_history_save().unwrap(), Some(newer));
                assert_eq!(core.export_history().unwrap(), history);
                owner.reconcile_destination(Some(&core)).unwrap();
                assert!(core.pending_history_save().unwrap().is_none());
                grow(&core);
                owner.save(&core, &inputs).unwrap();
                let reopened = Core::spawn(Document::default()).unwrap();
                LoadedBundle::read(&path)
                    .unwrap()
                    .restore_manual(&reopened)
                    .unwrap();
                assert_eq!(
                    reopened.export_history().unwrap(),
                    core.export_history().unwrap()
                );
            }
        }
    }
    #[test]
    fn w09_destination_proof_and_exact_own_ticket_settlement() {
        let temp = temp("s2-proof");
        let a = Core::spawn(document()).unwrap();
        let b = Core::spawn(document()).unwrap();
        grow(&a);
        grow(&b);
        let path_a = temp.path("a.kinewright");
        let path_b = temp.path("b.kinewright");
        let (_sa, mut copy_a) = new_copy(&temp, &path_a, &a, true);
        let (_sb, mut copy_b) = new_copy(&temp, &path_b, &b, true);
        let attempt_a = copy_a.pending_attempt().unwrap();
        let attempt_b = copy_b.pending_attempt().unwrap();
        assert_eq!(attempt_a.ticket().id, attempt_b.ticket().id);
        edit(&a, true);
        edit(&b, true);
        let ha = a.export_history().unwrap();
        let hb = b.export_history().unwrap();
        assert!(copy_a.reconcile_destination(Some(&b)).is_err());
        assert!(copy_b.reconcile_destination(Some(&a)).is_err());
        assert_eq!(a.export_history().unwrap(), ha);
        assert_eq!(b.export_history().unwrap(), hb);
        let original = fs::read(&path_a).unwrap();
        fs::write(&path_a, fs::read(&path_b).unwrap()).unwrap();
        assert!(copy_a.reconcile_destination(Some(&a)).is_err());
        assert_eq!(a.export_history().unwrap(), ha);
        fs::write(&path_a, b"damaged root").unwrap();
        assert!(copy_a.reconcile_destination(Some(&a)).is_err());
        assert_eq!(a.export_history().unwrap(), ha);
        assert!(a.pending_history_save().unwrap().is_some());
        fs::write(&path_a, &original).unwrap();
        let attachment = copy_a.attach_core(&a).unwrap();
        let cut = attachment.freeze().unwrap();
        attachment.seal(&cut).unwrap();
        copy_a.reconcile_destination(Some(&a)).unwrap();
        assert_eq!(a.export_history().unwrap(), ha);
        assert_eq!(
            a.pending_history_save().unwrap(),
            Some(attempt_a.ticket().clone())
        );
        copy_b.reconcile_destination(Some(&b)).unwrap();
        assert!(b.pending_history_save().unwrap().is_none());
        // A never-published first-save root may be proved absent under the same parent.
        let absent = Core::spawn(document()).unwrap();
        grow(&absent);
        let (_source, mut absent_copy) =
            new_copy(&temp, &temp.path("absent.kinewright"), &absent, false);
        let before = absent.export_history().unwrap();
        absent_copy.reconcile_destination(Some(&absent)).unwrap();
        assert!(absent.pending_history_save().unwrap().is_none());
        assert_eq!(absent.export_history().unwrap(), before);
        // Missing acknowledged bytes still refuse the owner. That is distinct from
        // losslessly releasing this exact ticket after proving its root absent.
        let path = temp.path("vanished.kinewright");
        let (vanished, mut owner, inputs) = setup(&path);
        owner.restrict_limits_probe(limits()).unwrap();
        grow(&vanished);
        assert!(matches!(
            io_observer::fail_once("root_rename_io", || owner.save(&vanished, &inputs)),
            Err(BundleError::DurabilityUnknown)
        ));
        let before = vanished.export_history().unwrap();
        fs::remove_file(&path).unwrap();
        assert!(owner.reconcile_destination(None).is_err());
        assert!(vanished.pending_history_save().unwrap().is_some());
        assert!(owner.reconcile_destination(Some(&vanished)).is_err());
        assert!(vanished.pending_history_save().unwrap().is_none());
        assert_eq!(vanished.export_history().unwrap(), before);
    }
    #[test]
    fn w09_changed_parent_does_not_settle_an_exact_root_at_a_new_identity() {
        fn copy_tree(from: &Path, to: &Path) {
            fs::create_dir_all(to).unwrap();
            for entry in fs::read_dir(from).unwrap() {
                let entry = entry.unwrap();
                let target = to.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    copy_tree(&entry.path(), &target);
                } else {
                    fs::copy(entry.path(), target).unwrap();
                }
            }
        }
        let temp = temp("s2-parent-identity");
        let parent = temp.path("parent");
        fs::create_dir(&parent).unwrap();
        let path = parent.join("a.kinewright");
        let (core, mut owner, inputs) = setup(&path);
        owner.restrict_limits_probe(limits()).unwrap();
        grow(&core);
        assert!(matches!(
            io_observer::fail_once("root_parent_sync", || owner.save(&core, &inputs)),
            Err(BundleError::DurabilityUnknown)
        ));
        let attempt = owner.pending_attempt().unwrap();
        let attachment = owner.attach_core(&core).unwrap();
        let original = temp.path("original-parent");
        fs::rename(&parent, &original).unwrap();
        copy_tree(&original, &parent);
        let before = core.export_history().unwrap();
        assert!(matches!(
            attachment.settle_attempt(&attempt),
            Err(BundleError::DurabilityUnknownAt(_))
        ));
        assert_eq!(core.export_history().unwrap(), before);
        assert_eq!(
            core.pending_history_save().unwrap(),
            Some(attempt.ticket().clone())
        );
        // Restore our own namespace so teardown never leaves a modeled detached root.
        fs::remove_dir_all(&parent).unwrap();
        fs::rename(original, parent).unwrap();
        attachment.settle_attempt(&attempt).unwrap();
        assert!(core.pending_history_save().unwrap().is_none());
    }
    fn setup(path: &Path) -> (Core, ProjectBundle, ObjectRef) {
        let core = Core::spawn(document()).unwrap();
        let mut owner =
            ProjectBundle::activate(path, BundleWriteAdmission::new_document()).unwrap();
        let inputs = owner.empty_inputs().unwrap().reference.clone();
        owner.save(&core, &inputs).unwrap();
        (core, owner, inputs)
    }
    #[test]
    fn w10_no_ticket_destination_reconcile_keeps_live_history() {
        for landed in [false, true] {
            for sealed in [false, true] {
                let temp = temp("s2-no-ticket");
                let path = temp.path("a.kinewright");
                let (core, mut owner, inputs) = setup(&path);
                let issuer = owner.issue_issuer(&core, &inputs, 1).unwrap();
                let revision = owner
                    .attach_core(&core)
                    .unwrap()
                    .capture()
                    .unwrap()
                    .revision();
                let command = CommandEnvelope {
                    switch: None,
                    manual: None,
                    version: 1,
                    action: "edit".into(),
                    id: CommandId {
                        issuer,
                        sequence: 1,
                    },
                    project_id: owner.project_id().clone(),
                    target_branch: owner.manual_branch_id().clone(),
                    owner_epoch: owner.store().owner_epoch(),
                    lease: None,
                    expected_head: owner.manual_head().cloned(),
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
                assert!(matches!(
                    io_observer::fail_once(
                        if landed {
                            "root_parent_sync"
                        } else {
                            "root_rename_io"
                        },
                        || owner.command(&command, &core, &inputs, &[])
                    ),
                    Err(BundleError::DurabilityUnknown)
                ));
                assert!(core.pending_history_save().unwrap().is_none());
                edit(&core, true);
                edit(&core, false);
                apply(&core, Command::Undo);
                let before = core.export_history().unwrap();
                if sealed {
                    let attachment = owner.attach_core(&core).unwrap();
                    let cut = attachment.freeze().unwrap();
                    attachment.seal(&cut).unwrap();
                }
                owner.reconcile_command(&core).unwrap();
                owner.reconcile_command(&core).unwrap();
                assert_eq!(core.export_history().unwrap(), before);
                assert!(core.pending_history_save().unwrap().is_none());
                let result = owner.receipt_lookup(&command).unwrap();
                assert!(if landed {
                    matches!(result, ReceiptLookup::Committed(_))
                } else {
                    matches!(result, ReceiptLookup::Absent)
                });
                assert!(!owner.has_pending_command_probe());
            }
        }
    }
    #[test]
    fn w16_reconcile_floor_error_preserves_proven_publication() {
        fn exhausted_core() -> Core {
            let core = Core::spawn_at(document(), kinewright_core::TimelineRevision(u64::MAX - 40))
                .unwrap();
            grow(&core);
            core
        }
        let temp = temp("s2-reconcile-phase");
        let path = temp.path("named.kinewright");
        let (_, mut owner, inputs) = setup(&path);
        owner.restrict_limits_probe(limits()).unwrap();
        let core = exhausted_core();
        let before = core.export_history().unwrap();
        assert!(matches!(
            io_observer::fail_once("root_parent_sync", || owner.save(&core, &inputs)),
            Err(BundleError::DurabilityUnknown)
        ));
        assert_eq!(
            owner.publication_state(),
            kinewright_project::bundle::PublicationState::Unknown
        );
        assert!(matches!(
            owner.reconcile_destination(Some(&core)),
            Err(BundleError::HistoryCommitInvariant)
        ));
        assert_eq!(
            owner.publication_state(),
            kinewright_project::bundle::PublicationState::Committed
        );
        assert_eq!(core.export_history().unwrap(), before);
        assert!(core.pending_history_save().unwrap().is_none());
        owner.reconcile_destination(None).unwrap();
        assert_eq!(
            LoadedBundle::read(&path)
                .unwrap()
                .admission()
                .inspection_document(),
            Some(&*before.document)
        );

        let path = temp.path("first.kinewright");
        let mut session = SessionStore::create(temp.root(), StorageLimits::default()).unwrap();
        let input = session
            .admit_inputs(&OwnedInputManifest {
                project_id: None,
                session_id: Some(session.session_id().clone()),
                owner_epoch: session.store().owner_epoch(),
                entries: vec![],
            })
            .unwrap()
            .reference
            .clone();
        let core = exhausted_core();
        let before = core.export_history().unwrap();
        let mut copy = io_observer::fail_once("root_parent_sync", || {
            session.first_save_with_limits(&path, &core, &input, limits())
        })
        .unwrap();
        assert!(matches!(copy.status(), CopyStatus::DurabilityUnknown));
        assert!(matches!(
            copy.reconcile_destination(Some(&core)),
            Err(BundleError::HistoryCommitInvariant)
        ));
        assert!(matches!(
            copy.status(),
            CopyStatus::CommittedWithError { .. }
        ));
        assert_eq!(core.export_history().unwrap(), before);
        assert!(core.pending_history_save().unwrap().is_none());
        copy.reconcile_destination(None).unwrap();
        assert!(matches!(copy.status(), CopyStatus::Committed(_)));
        assert_eq!(core.export_history().unwrap(), before);
    }
}

#[cfg(target_os = "linux")]
#[path = "support/ja_s2_c5_rescue.rs"]
mod c5;

#[cfg(target_os = "linux")]
fn durable_test_root() -> &'static std::path::Path {
    std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
}
