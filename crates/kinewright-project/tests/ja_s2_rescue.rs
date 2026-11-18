//! C2/C3 production controls: no rescue publication or UI handoff is claimed.
#![cfg(feature = "test-util")]
#![allow(clippy::too_many_lines)]
use kinewright_core::{
    Command, CoreMutationRefusal, Document, Event, Operation, Track, TrackId, TrackKind,
};
#[cfg(target_os = "linux")]
use kinewright_project::editing_session::Ja1EditingSession;
use kinewright_project::operation_actor::Core;
#[cfg(target_os = "linux")]
#[path = "support/ja_s2_c3.rs"]
mod c3;

#[cfg(not(target_os = "linux"))]
#[test]
fn c3_unsupported_new_context_refuses_before_source_mutation() {
    use kinewright_media::test_support::TempDirectory;
    let temp = TempDirectory::new("c3-unsupported");
    let core = Core::for_test(document()).unwrap();
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
                name: None,
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
    let a = Core::for_test(document()).unwrap();
    let b = Core::for_test(document()).unwrap();
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
            a.history_pool(),
        )
        .unwrap();
        let owner_b = ProjectBundle::activate(
            &temp.path("b.kinewright"),
            BundleWriteAdmission::new_document(),
            b.history_pool(),
        )
        .unwrap();
        assert_ne!(owner_a.project_id(), owner_b.project_id());
        let attachment_a = owner_a.attach_core(&a).unwrap();
        let attachment_b = owner_b.attach_core(&b).unwrap();
        assert!(a.clone().attach(&ForeignProof).is_err());
        assert!(b.clone().attach(&ForeignProof).is_err());
        edit(&a, false);
        let captured = a.snapshot_identity().unwrap();
        let selected = a
            .portable_history()
            .unwrap()
            .validate(b.history_pool())
            .unwrap();
        let cut_b = attachment_b.freeze().unwrap();
        attachment_b
            .install_snapshot(&cut_b, selected, None)
            .unwrap();
        let selected_b = attachment_b.freeze().unwrap();
        attachment_b.resume(&selected_b).unwrap();
        let cut = attachment_a.freeze().unwrap();
        attachment_a.seal(&cut).unwrap();
        edit(&b, true);
        assert_eq!(a.snapshot_identity().unwrap(), captured);
        assert_ne!(a.export_history().unwrap(), b.export_history().unwrap());
        assert!(matches!(
            a.request(Command::Undo).unwrap_err().error,
            kinewright_project::operation_actor::Error::MutationRefused(
                CoreMutationRefusal::Sealed
            )
        ));
    }
    #[cfg(not(target_os = "linux"))]
    {
        let authority = a.attach(&ForeignProof).unwrap();
        edit(&a, false);
        let cut = authority.freeze(&ForeignProof).unwrap();
        authority.seal(&ForeignProof, cut).unwrap();
        assert!(matches!(
            a.request(Command::Undo).unwrap_err().error,
            kinewright_project::operation_actor::Error::MutationRefused(
                CoreMutationRefusal::Sealed
            )
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
        let copy = io_observer::fail_once(
            if landed {
                "root_parent_sync"
            } else {
                "root_rename_io"
            },
            || session.first_save_with_limits(path, core, &inputs, limits()),
        )
        .unwrap();
        assert!(matches!(copy.status(), CopyStatus::DurabilityUnknown));
        assert_eq!(
            core.attachment_state().unwrap(),
            kinewright_core::AttachmentState::Live,
            "COPY_UNCERTAINTY_DOES_NOT_INSTALL_OR_FLOOR_ITS_SOURCE"
        );
        assert!(
            copy.pending_attempt().is_none(),
            "FIRST_SAVE_OWNS_A_COPY_PUBLICATION_NOT_A_NATIVE_COMMAND_TICKET"
        );
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
                assert_eq!(core.export_history().unwrap().history().undo.len(), 40);
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
                let before = core.snapshot_identity().unwrap();
                owner.reconcile_destination(Some(&core)).unwrap();
                assert!(owner.pending_attempt().is_none());
                assert_eq!(
                    core.snapshot_identity().unwrap(),
                    before,
                    "ORDINARY_SETTLEMENT_PRESERVES_LATER_ACCEPTED_OWNER"
                );
                owner.reconcile_destination(Some(&core)).unwrap();
                assert_eq!(core.snapshot_identity().unwrap(), before);
                let attachment = owner.attach_core(&core).unwrap();
                grow(&core);
                assert!(matches!(
                    io_observer::fail_once("root_parent_sync", || owner.save(&core, &inputs)),
                    Err(BundleError::DurabilityUnknown)
                ));
                let newer = owner.pending_attempt().unwrap();
                assert_ne!(
                    newer.tag(),
                    attempt.tag(),
                    "NEW_PUBLICATION_HAS_ITS_OWN_IMMUTABLE_ROOT_TAG"
                );
                let history = core.snapshot_identity().unwrap();
                let _ = attachment.settle_attempt(&attempt);
                assert_eq!(
                    owner.pending_attempt(),
                    Some(newer),
                    "OLD_DESTINATION_PROOF_CANNOT_CONSUME_NEWER_ATTEMPT"
                );
                assert_eq!(core.snapshot_identity().unwrap(), history);
                owner.reconcile_destination(Some(&core)).unwrap();
                assert!(owner.pending_attempt().is_none());
                grow(&core);
                owner.save(&core, &inputs).unwrap();
                let mut loaded = LoadedBundle::read(
                    &path,
                    &kinewright_project::operation_history::HistoryPool::new(
                        kinewright_project::operation_history::PoolLimits::default(),
                    ),
                )
                .unwrap();
                let reopened = loaded.open_manual().unwrap();
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
        let path_a = temp.path("a.kinewright");
        let path_b = temp.path("b.kinewright");
        let (a, mut copy_a, inputs_a) = setup(&path_a);
        let (b, mut copy_b, inputs_b) = setup(&path_b);
        grow(&a);
        grow(&b);
        assert!(matches!(
            io_observer::fail_once("root_parent_sync", || copy_a.save(&a, &inputs_a)),
            Err(BundleError::DurabilityUnknown)
        ));
        assert!(matches!(
            io_observer::fail_once("root_parent_sync", || copy_b.save(&b, &inputs_b)),
            Err(BundleError::DurabilityUnknown)
        ));
        let attempt_a = copy_a.pending_attempt().unwrap();
        let attempt_b = copy_b.pending_attempt().unwrap();
        assert_eq!(attempt_a.change().position, attempt_b.change().position);
        assert_ne!(
            attempt_a.actor(),
            attempt_b.actor(),
            "EQUAL_COUNTERS_NEVER_ALIAS_DIFFERENT_ACTOR_ATTEMPTS"
        );
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
        assert!(copy_a.pending_attempt().is_some());
        fs::write(&path_a, &original).unwrap();
        let attachment = copy_a.attach_core(&a).unwrap();
        let cut = attachment.freeze().unwrap();
        attachment.seal(&cut).unwrap();
        copy_a.reconcile_destination(Some(&a)).unwrap();
        assert_eq!(a.export_history().unwrap(), ha);
        assert_eq!(a.snapshot_identity().unwrap(), ha.identity().unwrap());
        copy_b.reconcile_destination(Some(&b)).unwrap();
        assert!(copy_b.pending_attempt().is_none());
        // A never-published first-save root may be proved absent under the same parent.
        let absent = Core::for_test(document()).unwrap();
        grow(&absent);
        let (_source, mut absent_copy) =
            new_copy(&temp, &temp.path("absent.kinewright"), &absent, false);
        let before = absent.export_history().unwrap();
        absent_copy.reconcile_destination(Some(&absent)).unwrap();
        assert!(absent_copy.pending_attempt().is_none());
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
        assert!(
            owner.pending_attempt().is_none(),
            "ABSENT_EXACT_TICKET_SETTLES_BEFORE_OWNER_REPAIR"
        );
        assert!(owner.reconcile_destination(Some(&vanished)).is_err());
        assert!(
            owner.pending_attempt().is_none(),
            "EXACT_ABSENT_TICKET_RELEASED_WITHOUT_REPAIRING_MISSING_ACKNOWLEDGED_ROOT"
        );
        assert!(owner.save(&vanished, &inputs).is_err());
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
        assert_eq!(owner.pending_attempt(), Some(attempt.clone()));
        // Restore our own namespace so teardown never leaves a modeled detached root.
        fs::remove_dir_all(&parent).unwrap();
        fs::rename(original, parent).unwrap();
        attachment.settle_attempt(&attempt).unwrap();
        owner.reconcile_destination(Some(&core)).unwrap();
        assert!(owner.pending_attempt().is_none());
    }
    fn setup(path: &Path) -> (Core, ProjectBundle, ObjectRef) {
        let core = Core::for_test(document()).unwrap();
        let mut owner = ProjectBundle::activate(
            path,
            BundleWriteAdmission::new_document(),
            core.history_pool(),
        )
        .unwrap();
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
                    .change()
                    .revision;
                let command = CommandEnvelope {
                    switch: None,
                    context: None,
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
                assert_eq!(
                    core.attachment_state().unwrap(),
                    kinewright_core::AttachmentState::Frozen
                );
                let pending = core.snapshot_identity().unwrap();
                assert!(
                    core.request(Command::Undo).is_err(),
                    "UNSETTLED_NATIVE_UPDATE_FENCES_ITS_SHADOW"
                );
                assert_eq!(core.snapshot_identity().unwrap(), pending);
                owner.reconcile_command(&core).unwrap();
                assert_eq!(
                    core.export_history().unwrap().history().document.tracks[0].sync_lock,
                    !landed,
                    "NATIVE_SETTLEMENT_APPLIES_ONLY_THE_PUBLISHED_UPDATE"
                );
                edit(&core, true);
                edit(&core, false);
                apply(&core, Command::Undo);
                let before = core.snapshot_identity().unwrap();
                if sealed {
                    let attachment = owner.attach_core(&core).unwrap();
                    let cut = attachment.freeze().unwrap();
                    attachment.seal(&cut).unwrap();
                }
                owner.reconcile_command(&core).unwrap();
                assert_eq!(
                    core.snapshot_identity().unwrap(),
                    before,
                    "REPEATED_RECEIPT_RECONCILIATION_NEVER_REPLAYS_THE_EDIT"
                );
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
            let core = Core::for_test_at_revision(
                document(),
                kinewright_core::TimelineRevision(u64::MAX - 40),
            )
            .unwrap();
            grow(&core);
            core
        }
        let temp = temp("s2-reconcile-phase");
        let path = temp.path("named.kinewright");
        let core = exhausted_core();
        let mut owner = ProjectBundle::activate(
            &path,
            BundleWriteAdmission::new_document(),
            core.history_pool(),
        )
        .unwrap();
        let inputs = owner.empty_inputs().unwrap().reference.clone();
        owner.restrict_limits_probe(limits()).unwrap();
        let before = core.export_history().unwrap();
        assert!(matches!(
            io_observer::fail_once("root_parent_sync", || owner.save(&core, &inputs)),
            Err(BundleError::DurabilityUnknown)
        ));
        assert_eq!(
            owner.publication_state(),
            kinewright_project::bundle::PublicationState::Unknown
        );
        owner.reconcile_destination(Some(&core)).unwrap();
        assert_eq!(
            owner.publication_state(),
            kinewright_project::bundle::PublicationState::Committed
        );
        assert_eq!(core.export_history().unwrap(), before);
        assert_eq!(
            core.attachment_state().unwrap(),
            kinewright_core::AttachmentState::Live
        );
        owner.reconcile_destination(None).unwrap();
        assert_eq!(
            LoadedBundle::read(
                &path,
                &kinewright_project::operation_history::HistoryPool::new(
                    kinewright_project::operation_history::PoolLimits::default()
                )
            )
            .unwrap()
            .admission()
            .inspection_document(),
            Some(&*before.history().document)
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
        let result = session.first_save_with_limits(&path, &core, &input, limits());
        assert!(
            matches!(result, Err(BundleError::Capacity("install revision"))),
            "EXHAUSTED_INSTALL_REVISION_REFUSES_BEFORE_PUBLICATION"
        );
        assert!(
            !path.exists(),
            "REVISION_REFUSAL_PUBLISHES_NO_UNINSTALLABLE_ROOT"
        );
        assert_eq!(core.export_history().unwrap(), before);
        assert_eq!(
            core.attachment_state().unwrap(),
            kinewright_core::AttachmentState::Live
        );
    }
}

#[cfg(target_os = "linux")]
#[path = "support/ja_s2_c5_rescue.rs"]
mod c5;

#[cfg(target_os = "linux")]
fn durable_test_root() -> &'static std::path::Path {
    std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
}
