//! C11 switches use the ordinary pinned Save/select transaction.
#![cfg(feature = "test-util")]

#[cfg(windows)]
#[test]
fn native_windows_switch_admission_refuses_before_mutation() {
    use kinewright_project::{
        bundle::{BundleError, BundleWriteAdmission, ProjectBundle},
        io_observer,
    };
    let root = kinewright_media::test_support::TempDirectory::new("c11-refusal");
    let path = root.path("a.kinewright");
    let (result, trace) = io_observer::observe(|| {
        ProjectBundle::activate(&path, BundleWriteAdmission::new_document())
    });
    assert!(matches!(result, Err(BundleError::DurabilityUnsupported)));
    assert!(trace.events.is_empty());
    assert!(!path.exists());
}

#[cfg(target_os = "linux")]
mod linux {
    use kinewright_core::{Command, Core, Document, Event, Operation, Track, TrackId, TrackKind};
    use kinewright_media::test_support::TempDirectory;
    use kinewright_project::{
        bundle::{BundleError, LoadedBundle},
        bundle_copy::CopyStatus,
        bundle_recovery::RecoverySelection,
        bundle_switch::SwitchTarget,
        editing_session::Ja1EditingSession,
        io_observer,
        receipts::{CommandEnvelope, ReceiptLookup, ReceiptResult},
        storage_budget::StorageLimits,
    };
    use std::path::{Path, PathBuf};

    fn setup() -> (TempDirectory, PathBuf, Ja1EditingSession) {
        let root = TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "c11-project");
        let path = root.path("a.kinewright");
        let core = Core::spawn(Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            }],
            ..Document::default()
        })
        .unwrap();
        let mut session =
            Ja1EditingSession::new(root.root(), &core, StorageLimits::default()).unwrap();
        core.request(Command::Undo).unwrap();
        assert!(matches!(
            session.first_save(&path, StorageLimits::default()).unwrap(),
            CopyStatus::Committed(_)
        ));
        edit(session.core(), false);
        session.save_active().unwrap();
        (root, path, session)
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
    fn request(session: &mut Ja1EditingSession, target: Option<SwitchTarget>) -> CommandEnvelope {
        let target =
            target.unwrap_or_else(|| match session.list_recovery_points().unwrap().remove(0) {
                RecoverySelection::Checkpoint { root, branch } => SwitchTarget::Snapshot {
                    root,
                    branch,
                    version: "1".repeat(32),
                },
                RecoverySelection::WorkingBranch(_) => panic!("checkpoint control"),
            });
        let lease = session
            .issue_manual_lease("a".repeat(64), "private-switch-token".into())
            .unwrap();
        session.switch_envelope(&lease, target).unwrap()
    }
    fn committed(lookup: ReceiptLookup) -> kinewright_project::bundle_switch::SwitchResult {
        let ReceiptLookup::Committed(receipt) = lookup else {
            panic!("committed switch receipt")
        };
        let ReceiptResult::Switch { result } = receipt.result else {
            panic!("switch result")
        };
        *result
    }
    #[test]
    fn g05a_atomic_retain_create_select_and_switch_generation() {
        let (root, path, mut session) = setup();
        // Keep a genuine Redo stack in L. Adoption must not be a whole-document Undo.
        edit(session.core(), true);
        session.core().request(Command::Undo).unwrap();
        session.save_active().unwrap();
        let old = session.current_switch_attachment().unwrap();
        let old_core = session.core().clone();
        let envelope = request(&mut session, None);
        session.prepare_host_switch(&envelope).unwrap();
        let target = session.switch_preview().unwrap();
        let result = committed(session.publish_host_switch(&envelope).unwrap().0);
        assert_eq!(result.old.branch, old.current.branch);
        assert_eq!(result.old.history, old.current.history);
        assert_ne!(result.new.branch, old.current.branch);
        assert_eq!(
            result.target_version.as_deref(),
            Some("1".repeat(32).as_str())
        );
        assert_eq!(session.list_working_branches().unwrap().len(), 2);
        assert_eq!(result.switch_generation, 1);
        assert!(matches!(
            old_core.request(Command::Undo).unwrap(),
            Event::MutationRefused { .. }
        ));
        let adopted = session.core().export_history().unwrap();
        assert_eq!(adopted, target.history);
        assert_eq!(
            session.current_switch_attachment().unwrap().inputs,
            target.inputs
        );
        assert_eq!(
            session.current_switch_attachment().unwrap().lineage,
            target.lineage
        );
        edit(session.core(), !adopted.document.tracks[0].sync_lock);
        session.core().request(Command::Undo).unwrap();
        assert_eq!(
            session.core().export_history().unwrap().document,
            adopted.document
        );
        session.save_active().unwrap();
        assert_eq!(session.manual_switch_generation().unwrap(), 1);
        let return_request = request(
            &mut session,
            Some(SwitchTarget::WorkingBranch {
                branch: old.current.branch.clone(),
            }),
        );
        let returned = committed(session.publish_host_switch(&return_request).unwrap().0);
        assert_eq!(returned.new.branch, old.current.branch);
        assert_eq!(session.core().export_history().unwrap(), old.history);
        assert_eq!(returned.switch_generation, 2);
        session.save_active().unwrap();
        assert_eq!(session.manual_switch_generation().unwrap(), 2);
        let copy = root.path("b.kinewright");
        assert!(matches!(
            session.save_as(&copy, StorageLimits::default()).unwrap(),
            CopyStatus::Committed(_)
        ));
        assert_eq!(
            LoadedBundle::read(&copy)
                .unwrap()
                .root()
                .unwrap()
                .manual_switch_generation,
            2
        );
        assert_eq!(
            LoadedBundle::read(&path)
                .unwrap()
                .root()
                .unwrap()
                .manual_switch_generation,
            2
        );
    }
    #[test]
    fn g05a_original_envelope_reply_loss_and_superseding_result() {
        let (_root, _path, mut session) = setup();
        let envelope = request(&mut session, None);
        session.prepare_host_switch(&envelope).unwrap();
        let old = session.core().clone();
        let outcome = io_observer::fail_once("root_parent_sync", || {
            session.publish_host_switch(&envelope)
        });
        assert!(outcome.is_err());
        assert_eq!(
            old.attachment_state().unwrap(),
            kinewright_core::AttachmentState::Frozen
        );
        let original = committed(session.publish_host_switch(&envelope).unwrap().0);
        assert_eq!(
            committed(session.publish_host_switch(&envelope).unwrap().0),
            original
        );
        assert_eq!(session.list_working_branches().unwrap().len(), 2);
        let mut conflict = envelope.clone();
        conflict.switch.as_mut().unwrap().switch_generation += 1;
        assert_eq!(
            session.publish_host_switch(&conflict).unwrap().0,
            ReceiptLookup::IdPayloadConflict
        );
        let back = request(
            &mut session,
            Some(SwitchTarget::WorkingBranch {
                branch: original.old.branch.clone(),
            }),
        );
        committed(session.publish_host_switch(&back).unwrap().0);
        let current = session.current_switch_attachment().unwrap();
        assert_eq!(
            committed(session.publish_host_switch(&envelope).unwrap().0),
            original
        );
        assert_eq!(session.current_switch_attachment().unwrap(), current);
    }
    #[test]
    fn g05a_prepublication_refusal_preserves_saved_l_and_target_pin() {
        let (_root, _path, mut session) = setup();
        let envelope = request(&mut session, None);
        session.prepare_host_switch(&envelope).unwrap();
        let before = session.current_switch_attachment().unwrap();
        let outcome = io_observer::fail_once("root_before_publish", || {
            session.publish_host_switch(&envelope)
        });
        assert!(outcome.is_err());
        assert_eq!(session.current_switch_attachment().unwrap(), before);
        assert_eq!(
            session.core().attachment_state().unwrap(),
            kinewright_core::AttachmentState::Live
        );
        assert!(session.has_prepared_selection());
        assert_eq!(
            committed(session.publish_host_switch(&envelope).unwrap().0).switch_generation,
            1
        );
    }
    #[test]
    fn g05a_each_source_identity_is_required_before_selection() {
        for field in 0..10 {
            let (_root, _path, mut session) = setup();
            let mut envelope = request(&mut session, None);
            let before = session.current_switch_attachment().unwrap();
            match field {
                0 => envelope.expected_head = None,
                1 => envelope.owner_epoch ^= 1,
                2 => envelope.switch.as_mut().unwrap().checkpoint_generation += 1,
                3 => envelope.switch.as_mut().unwrap().floor_generation += 1,
                4 => envelope.switch.as_mut().unwrap().switch_generation += 1,
                5 => envelope.switch.as_mut().unwrap().source_checkpoint.sha256 = "0".repeat(64),
                6 => {
                    envelope.project_id =
                        kinewright_project::bundle_records::ProjectId::fresh().unwrap();
                }
                7 => {
                    envelope.target_branch =
                        kinewright_project::bundle_records::BranchId::fresh().unwrap();
                }
                8 => envelope.expected_revision.0 += 1,
                _ => envelope.id.sequence += 1,
            }
            assert!(matches!(
                session.prepare_host_switch(&envelope),
                Err(BundleError::HistoryChanged)
            ));
            assert_eq!(session.current_switch_attachment().unwrap(), before);
            assert!(!session.has_prepared_selection());
        }
    }
    #[test]
    fn g05a_receipt_support_is_required_and_legacy_envelopes_remain_identical() {
        let (_root, path, mut session) = setup();
        let envelope = request(&mut session, None);
        let mut legacy = serde_json::to_value(&envelope).unwrap();
        legacy.as_object_mut().unwrap().remove("switch");
        legacy["action"] = serde_json::json!("edit");
        legacy["lease"] = serde_json::Value::Null;
        legacy["command"] = serde_json::to_value(kinewright_core::JournalCommand::Undo).unwrap();
        let encoded = serde_json::to_vec(&legacy).unwrap();
        let parsed: CommandEnvelope = serde_json::from_slice(&encoded).unwrap();
        let original_value = serde_json::to_value(&parsed).unwrap();
        assert_eq!(original_value, legacy);
        assert!(!original_value.as_object().unwrap().contains_key("switch"));
        committed(session.publish_host_switch(&envelope).unwrap().0);
        assert!(matches!(
            kinewright_project::bundle_recovery::without_api_probe("host-switch", || {
                LoadedBundle::read(&path)
            }),
            Err(BundleError::Unsupported(_))
        ));
        assert!(LoadedBundle::read(&path).is_ok());
    }
    #[test]
    fn g05a_exact_receipt_bearing_publication_requires_checkpoint_agreement() {
        let (_root, path, mut session) = setup();
        let envelope = request(&mut session, None);
        committed(session.publish_host_switch(&envelope).unwrap().0);
        let root = std::fs::read(&path).unwrap();
        let history = session.core().export_history().unwrap();
        session.change_switch_receipt_probe().unwrap();
        assert!(
            matches!(session.save_active(), Err(BundleError::Corrupt(ref reason)) if reason == "switch receipt checkpoint agreement")
        );
        assert_eq!(std::fs::read(&path).unwrap(), root);
        assert_eq!(session.core().export_history().unwrap(), history);
        assert!(LoadedBundle::read(&path).is_ok());
    }
    #[test]
    fn g05a_independent_open_checks_raw_switch_receipt_and_api_declarations() {
        use kinewright_project::{
            bundle_records::{ObjectRef, RecordKind},
            receipts::ReceiptLedger,
            store::StoreManifest,
        };
        let (_root, path, mut session) = setup();
        let envelope = request(&mut session, None);
        committed(session.publish_host_switch(&envelope).unwrap().0);
        drop(session);
        let original = std::fs::read(&path).unwrap();
        let root: serde_json::Value = serde_json::from_slice(&original).unwrap();
        let objects = path
            .with_extension("kinewright-assets")
            .join("studio/objects");
        let receipt_ref: ObjectRef =
            serde_json::from_value(root["studio"]["receipt_index_ref"].clone()).unwrap();
        let manifest_ref: ObjectRef =
            serde_json::from_value(root["studio"]["store_manifest_ref"].clone()).unwrap();
        let ledger: ReceiptLedger =
            serde_json::from_slice(&std::fs::read(objects.join(&receipt_ref.sha256)).unwrap())
                .unwrap();
        let manifest: StoreManifest =
            serde_json::from_slice(&std::fs::read(objects.join(&manifest_ref.sha256)).unwrap())
                .unwrap();
        assert!(LoadedBundle::read(&path).is_ok());
        for case in 0..3 {
            let mut next = manifest.clone();
            let mut changed = root.clone();
            let mut created = vec![];
            if case == 0 {
                let mut ledger = ledger.clone();
                for receipt in &mut ledger.detailed {
                    if let ReceiptResult::Switch { result } = &mut receipt.result {
                        result.new.history = result.old.history.clone();
                    }
                }
                let bytes = serde_json::to_vec(&ledger).unwrap();
                let reference = ObjectRef {
                    sha256: kinewright_media::sha256_bytes(&bytes),
                    byte_length: bytes.len() as u64,
                    record_kind: RecordKind::ReceiptIndex,
                };
                std::fs::write(objects.join(&reference.sha256), bytes).unwrap();
                created.push(reference.sha256.clone());
                for object in &mut next.objects {
                    if *object == receipt_ref {
                        *object = reference.clone();
                    }
                }
                let edges = next.dependencies.remove(&receipt_ref.sha256).unwrap();
                next.dependencies.insert(reference.sha256.clone(), edges);
                changed["studio"]["receipt_index_ref"] = serde_json::to_value(reference).unwrap();
            } else if case == 1 {
                next.required_apis.remove("host-switch");
            } else {
                next.required_apis.insert("host-switch".into(), 2);
            }
            let bytes = serde_json::to_vec(&next).unwrap();
            let reference = ObjectRef {
                sha256: kinewright_media::sha256_bytes(&bytes),
                byte_length: bytes.len() as u64,
                record_kind: RecordKind::StoreManifest,
            };
            std::fs::write(objects.join(&reference.sha256), bytes).unwrap();
            created.push(reference.sha256.clone());
            changed["studio"]["store_manifest_ref"] = serde_json::to_value(reference).unwrap();
            std::fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
            let result = LoadedBundle::read(&path);
            if case == 0 {
                assert!(
                    matches!(result, Err(BundleError::Corrupt(ref reason)) if reason == "switch receipt checkpoint agreement")
                );
            } else {
                assert!(matches!(result, Err(BundleError::Unsupported(_))));
            }
            std::fs::write(&path, &original).unwrap();
            for hash in created {
                std::fs::remove_file(objects.join(hash)).unwrap();
            }
            assert!(LoadedBundle::read(&path).is_ok());
        }
    }
}
