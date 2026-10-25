//! C8 project half: real retained source bytes and charges, no host ownership.
#![cfg(feature = "test-util")]

#[cfg(not(target_os = "linux"))]
#[test]
fn source_owner_unsupported_profile_has_no_mutation() {
    let temp = kinewright_media::test_support::TempDirectory::new("c8-source-unsupported");
    let (result, trace) = kinewright_project::io_observer::observe(|| {
        kinewright_project::bundle_copy::SessionStore::create(
            temp.root(),
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

#[cfg(target_os = "linux")]
mod linux {
    use kinewright_core::{
        AttachmentState, Command, Core, Document, Event, HistorySnapshot, Operation, Track,
        TrackId, TrackKind,
    };
    use kinewright_media::test_support::TempDirectory;
    use kinewright_project::{
        bundle::{BundleError, LoadedBundle},
        bundle_copy::{BundleCopy, CopyStatus, SessionStore},
        bundle_records::{ObjectRef, RecordKind},
        bundle_recovery::RecoverySelection,
        bundle_switch::ProjectSelectionResult,
        editing_session::{HostActivationKind, Ja1EditingSession},
        namespace::NamespaceIdentity,
        owned_inputs::{OwnedInputEntry, OwnedInputManifest},
        source_boundary::{
            BorrowedSnapshotBinding, BorrowedSourceDescriptor, BorrowedSourceReader,
            BorrowedSourceService, SOURCE_READ_CHUNK_BYTES,
        },
        storage_budget::StorageLimits,
    };
    use std::{
        io::Read,
        panic::{AssertUnwindSafe, catch_unwind},
        path::Path,
    };

    fn temp(name: &str) -> TempDirectory {
        TempDirectory::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")), name)
    }
    fn bytes() -> Vec<u8> {
        (0_u8..=255)
            .cycle()
            .take(SOURCE_READ_CHUNK_BYTES * 2 + 17)
            .collect()
    }
    fn source(temp: &TempDirectory) -> (SessionStore, ObjectRef, ObjectRef) {
        source_with_limits(temp, StorageLimits::default())
    }
    fn source_with_limits(
        temp: &TempDirectory,
        limits: StorageLimits,
    ) -> (SessionStore, ObjectRef, ObjectRef) {
        let mut owner = SessionStore::create(temp.root(), limits).unwrap();
        let data = bytes();
        let object = ObjectRef {
            sha256: kinewright_media::sha256_bytes(&data),
            byte_length: data.len() as u64,
            record_kind: RecordKind::OwnedData,
        };
        let blob = owner
            .store_mut()
            .admit_blob(&mut data.as_slice(), &object, &[])
            .unwrap();
        let manifest = OwnedInputManifest {
            project_id: None,
            session_id: Some(owner.session_id().clone()),
            owner_epoch: owner.store().owner_epoch(),
            entries: vec![OwnedInputEntry {
                entry_id: "actual-source".into(),
                object: object.clone(),
                dependencies: vec![],
                selected_stream: None,
                time_base: None,
                origin: None,
                profile: None,
            }],
        };
        let inputs = owner.admit_inputs(&manifest).unwrap().reference.clone();
        drop(blob);
        (owner, inputs, object)
    }
    fn history() -> HistorySnapshot {
        Core::spawn(Document::default())
            .unwrap()
            .export_history()
            .unwrap()
    }
    fn digest(history: &HistorySnapshot) -> String {
        kinewright_media::sha256_bytes(&serde_json::to_vec(history).unwrap())
    }
    fn add_track(core: &Core, id: u64) {
        assert!(matches!(
            core.request(Command::Do(Operation::AddTrack {
                track: Track {
                    id: TrackId(id),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![]
                },
            }))
            .unwrap(),
            Event::DocumentChanged { .. }
        ));
    }

    #[test]
    fn borrowed_copy_drop_and_unwind_keep_actual_gui_source_pins_and_charges() {
        let temp = temp("c8-source-drop");
        let (mut owner, inputs, object) = source(&temp);
        let history = history();
        let descriptor = owner
            .retain_borrowed_source(
                std::slice::from_ref(&inputs),
                history.encoded_bytes().unwrap(),
            )
            .unwrap();
        let service = owner.borrowed_source_service(&descriptor).unwrap();
        service.ensure_snapshot(&descriptor, &history).unwrap();
        let root = owner.store().resolved_root().unwrap();
        let before = owner.store().retention_probe().unwrap();
        let charge = owner.store().usage().unwrap();
        assert_eq!(before.0, 1);
        assert_eq!(before.1.get(&object.sha256), Some(&1));
        assert!(charge.reserved_copy_bytes > object.byte_length);
        for unwind in [false, true] {
            let target = temp.path(if unwind {
                "unwind.kinewright"
            } else {
                "orderly.kinewright"
            });
            let mut reader = service.clone();
            let copy = BundleCopy::first_save_borrowed(
                &target,
                descriptor.clone(),
                &mut reader,
                &history,
                &inputs,
                StorageLimits::default(),
            )
            .unwrap();
            assert!(matches!(copy.status(), CopyStatus::Committed(_)));
            assert_eq!(copy.borrowed_source(), Some(&descriptor));
            let outcome = catch_unwind(AssertUnwindSafe(move || {
                let _actual_host_copy = copy;
                let _actual_host_callback = reader;
                assert!(
                    !unwind,
                    "caught host unwind executes the real copy Drop path"
                );
            }));
            assert_eq!(outcome.is_err(), unwind);
            assert!(root.parent().unwrap().exists());
            assert_eq!(owner.store().retention_probe().unwrap(), before);
            let after = owner.store().usage().unwrap();
            assert_eq!(after.reserved_copy_bytes, charge.reserved_copy_bytes);
            assert_eq!(after.physical_bytes, charge.physical_bytes);
            let mut actual = vec![];
            owner
                .open_borrowed_source(&descriptor, &object)
                .unwrap()
                .read_to_end(&mut actual)
                .unwrap();
            assert_eq!(actual, bytes());
            let loaded = LoadedBundle::read(&target).unwrap();
            assert_eq!(
                loaded.manual_checkpoint().unwrap().history_ref.sha256,
                digest(&history)
            );
            let rebound = loaded.root().unwrap().input_manifest_ref.clone();
            let bundle = loaded.into_writer().unwrap();
            let manifest = bundle.store().read_input_manifest(&rebound).unwrap();
            assert_eq!(manifest.entries[0].object, object);
            assert_eq!(manifest.project_id.as_ref(), Some(bundle.project_id()));
        }
        drop(service);
        assert_eq!(owner.store().retention_probe().unwrap(), before);
    }

    #[test]
    fn originating_close_fences_service_clones_and_retires_once_after_last_reader() {
        let temp = temp("c8-source-close");
        let (mut owner, inputs, object) = source(&temp);
        let descriptor = owner.retain_borrowed_source(&[inputs], 1234).unwrap();
        let service = owner.borrowed_source_service(&descriptor).unwrap();
        let surviving_service = service.clone();
        let mut reader = service.open_borrowed_source(&descriptor, &object).unwrap();
        let usage = owner.store().usage_handle();
        let root = owner.store().resolved_root().unwrap();
        let held = owner.store().retention_probe().unwrap();
        let charged = usage().unwrap().reserved_copy_bytes;
        owner.close_borrowed_sources();
        owner.close_borrowed_sources();
        assert_eq!(owner.store().retention_probe().unwrap(), held);
        assert_eq!(usage().unwrap().reserved_copy_bytes, charged);
        assert!(
            surviving_service
                .read_borrowed_source(&descriptor, &object, 0, &mut [0; 1])
                .is_err()
        );
        assert!(!owner.close_originating_source().unwrap());
        assert!(owner.retain_borrowed_source(&[], 0).is_err());
        drop(owner);
        assert!(root.parent().unwrap().exists());
        let mut actual = vec![];
        reader.read_to_end(&mut actual).unwrap();
        assert_eq!(actual, bytes());
        drop(reader);
        assert!(!root.parent().unwrap().exists());
        assert_eq!(usage().unwrap().reserved_copy_bytes, 0);
        assert_eq!(usage().unwrap().physical_bytes, 0);
        assert!(!usage().unwrap().damaged_accounting);
        drop(surviving_service);
        assert_eq!(usage().unwrap().reserved_copy_bytes, 0);
    }

    #[test]
    fn explicit_source_close_keeps_failed_cleanup_charged_and_retryable() {
        let temp = temp("c8-close-retry");
        let (mut owner, inputs, _) = source(&temp);
        let descriptor = owner.retain_borrowed_source(&[inputs], 4321).unwrap();
        let service = owner.borrowed_source_service(&descriptor).unwrap();
        let root = owner.store().resolved_root().unwrap();
        let usage = owner.store().usage_handle();
        let failed = kinewright_project::io_observer::fail_once("gc_before_unlink", || {
            owner.close_originating_source()
        });
        assert!(failed.is_err());
        assert!(root.parent().unwrap().exists());
        assert!(usage().unwrap().physical_bytes > 0);
        assert_eq!(
            usage().unwrap().physical_bytes,
            owner.store().collection_status().unwrap().pending_bytes
        );
        assert!(
            service
                .open_borrowed_source(&descriptor, descriptor.roots().first().unwrap())
                .is_err()
        );
        assert!(owner.close_originating_source().unwrap());
        assert!(owner.close_originating_source().unwrap());
        assert!(!root.parent().unwrap().exists());
        assert_eq!(usage().unwrap().physical_bytes, 0);
        assert_eq!(usage().unwrap().reserved_copy_bytes, 0);
        assert!(!usage().unwrap().damaged_accounting);
        drop(owner);
        assert!(!usage().unwrap().unsettled);
    }

    #[test]
    fn close_reply_retry_never_unlinks_successor_temporary_name() {
        let temp = temp("c8-close-reply-retry");
        let (mut owner, inputs, _) = source(&temp);
        let _descriptor = owner.retain_borrowed_source(&[inputs], 1).unwrap();
        let root = owner.store().resolved_root().unwrap();
        let directory = root.parent().unwrap();
        let usage = owner.store().usage_handle();
        assert!(
            kinewright_project::io_observer::fail_once("session_cleanup_base_sync", || owner
                .close_originating_source())
            .is_err()
        );
        assert!(!directory.exists());
        std::fs::create_dir(directory).unwrap();
        std::fs::write(directory.join("successor"), b"different owner").unwrap();
        assert!(owner.close_originating_source().unwrap());
        assert!(owner.close_originating_source().unwrap());
        drop(owner);
        assert_eq!(
            std::fs::read(directory.join("successor")).unwrap(),
            b"different owner"
        );
        assert_eq!(usage().unwrap().physical_bytes, 0);
        assert!(!usage().unwrap().damaged_accounting);
    }

    #[test]
    fn borrowed_destination_inside_actual_source_refuses_before_any_mutation() {
        let temp = temp("c8-source-separation");
        let (mut owner, inputs, _) = source(&temp);
        let history = history();
        let descriptor = owner
            .retain_borrowed_source(
                std::slice::from_ref(&inputs),
                history.encoded_bytes().unwrap(),
            )
            .unwrap();
        let mut service = owner.borrowed_source_service(&descriptor).unwrap();
        service.ensure_snapshot(&descriptor, &history).unwrap();
        let base = owner
            .store()
            .resolved_root()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let destination = base.join("forbidden").join("copy.kinewright");
        let before = owner.store().usage().unwrap();
        let (result, trace) = kinewright_project::io_observer::observe(|| {
            BundleCopy::first_save_borrowed(
                &destination,
                descriptor.clone(),
                &mut service,
                &history,
                &inputs,
                StorageLimits::default(),
            )
        });
        assert!(matches!(result, Err(BundleError::Identity(_))));
        assert!(!base.join("forbidden").exists());
        assert!(!trace.overflowed);
        assert!(
            trace
                .events
                .iter()
                .all(|e| e.operation == "rescue_separation_inspect")
        );
        assert_eq!(owner.store().usage().unwrap(), before);
        // Same service permits a fresh, nested, genuinely separate destination.
        let legal = temp.path("independent/new/copy.kinewright");
        let copy = BundleCopy::first_save_borrowed(
            &legal,
            descriptor,
            &mut service,
            &history,
            &inputs,
            StorageLimits::default(),
        )
        .unwrap();
        assert!(matches!(copy.status(), CopyStatus::Committed(_)));
    }

    struct CorruptReader(BorrowedSourceService);
    impl BorrowedSourceReader for CorruptReader {
        fn prove_snapshot(
            &mut self,
            source: &BorrowedSourceDescriptor,
            snapshot: &BorrowedSnapshotBinding,
        ) -> Result<(), BundleError> {
            self.0.prove_snapshot(source, snapshot)
        }
        fn inspect_destination(
            &mut self,
            source: &BorrowedSourceDescriptor,
            path: &Path,
        ) -> Result<NamespaceIdentity, BundleError> {
            self.0.inspect_destination(source, path)
        }
        fn read_source(
            &mut self,
            source: &BorrowedSourceDescriptor,
            object: &ObjectRef,
            offset: u64,
            buffer: &mut [u8],
        ) -> Result<usize, BundleError> {
            let count = self
                .0
                .read_borrowed_source(source, object, offset, buffer)?;
            if count != 0 {
                buffer[0] ^= 1;
            }
            Ok(count)
        }
    }
    #[test]
    fn borrowed_source_identity_range_and_stream_digest_are_verified() {
        let temp = temp("c8-source-verification");
        let (mut owner, inputs, object) = source(&temp);
        let history = history();
        let descriptor = owner
            .retain_borrowed_source(
                std::slice::from_ref(&inputs),
                history.encoded_bytes().unwrap(),
            )
            .unwrap();
        let service = owner.borrowed_source_service(&descriptor).unwrap();
        service.ensure_snapshot(&descriptor, &history).unwrap();
        assert!(
            service
                .read_borrowed_source(&descriptor, &object, object.byte_length + 1, &mut [0; 1])
                .is_err()
        );
        assert!(
            service
                .read_borrowed_source(
                    &descriptor,
                    &object,
                    0,
                    &mut vec![0; SOURCE_READ_CHUNK_BYTES + 1]
                )
                .is_err()
        );
        let mut wire = serde_json::to_value(&descriptor).unwrap();
        wire["owner_epoch"] = serde_json::Value::from(descriptor.owner_epoch().wrapping_add(1));
        let tampered: BorrowedSourceDescriptor = serde_json::from_value(wire).unwrap();
        assert!(service.open_borrowed_source(&tampered, &object).is_err());
        let target = temp.path("corrupt.kinewright");
        let before = owner.store().retention_probe().unwrap();
        assert!(
            BundleCopy::first_save_borrowed(
                &target,
                descriptor,
                &mut CorruptReader(service),
                &history,
                &inputs,
                StorageLimits::default()
            )
            .is_err()
        );
        assert!(!target.exists());
        assert_eq!(owner.store().retention_probe().unwrap(), before);
    }

    #[test]
    fn gui_split_keeps_actor_and_source_and_host_loss_refuses_fresh_save() {
        let temp = temp("c8-gui-split");
        let core = Core::spawn(Document::default()).unwrap();
        add_track(&core, 1);
        let mut owner =
            Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
        let root = owner.source().unwrap().store().resolved_root().unwrap();
        let prepared = owner.prepare_host_attachment().unwrap();
        assert_eq!(owner.core().actor_identity(), core.actor_identity());
        assert_eq!(core.attachment_state().unwrap(), AttachmentState::Frozen);
        let cut = owner.freeze_host_cut().unwrap();
        owner.complete_host_cut(&cut, None, None).unwrap();
        assert_eq!(core.attachment_state().unwrap(), AttachmentState::Live);
        owner.host_lost().unwrap();
        assert_eq!(core.export_history().unwrap(), prepared.history);
        assert_eq!(core.attachment_state().unwrap(), AttachmentState::Frozen);
        assert!(owner.freeze_host_cut().is_err());
        assert!(
            owner
                .first_save(
                    &temp.path("must-not-save.kinewright"),
                    StorageLimits::default()
                )
                .is_err()
        );
        drop(prepared);
        assert!(root.parent().unwrap().exists());
        assert!(
            owner
                .source()
                .unwrap()
                .store()
                .usage()
                .unwrap()
                .reserved_copy_bytes
                > 0
        );
        drop(owner);
        assert!(!root.parent().unwrap().exists());
    }

    #[test]
    fn remote_activation_prepares_fresh_actor_before_exact_old_seal() {
        let temp = temp("c8-gui-activate");
        let original = Core::spawn(Document::default()).unwrap();
        add_track(&original, 1);
        let mut owner =
            Ja1EditingSession::new(temp.root(), &original, StorageLimits::default()).unwrap();
        let prepared = owner.prepare_host_attachment().unwrap();
        let cut = owner.freeze_host_cut().unwrap();
        let candidate = Core::spawn(Document::default()).unwrap();
        add_track(&candidate, 2);
        let history = candidate.export_history().unwrap();
        let mut failed_view_actor = None;
        assert!(
            owner
                .activate_host_cut(
                    &cut,
                    history.clone(),
                    &digest(&history),
                    HostActivationKind::Rescue,
                    |core| {
                        failed_view_actor = Some(core.clone());
                        Err(BundleError::Capacity("fallible prepared view"))
                    }
                )
                .is_err()
        );
        assert_eq!(
            failed_view_actor.unwrap().attachment_state().unwrap(),
            AttachmentState::Sealed
        );
        assert_eq!(
            original.attachment_state().unwrap(),
            AttachmentState::Frozen
        );
        assert_eq!(original.export_history().unwrap(), prepared.history);
        let fresh = owner
            .activate_host_cut(
                &cut,
                history.clone(),
                &digest(&history),
                HostActivationKind::Rescue,
                |core| {
                    assert_ne!(core.actor_identity(), original.actor_identity());
                    assert_eq!(core.attachment_state().unwrap(), AttachmentState::Frozen);
                    assert_eq!(
                        original.attachment_state().unwrap(),
                        AttachmentState::Frozen
                    );
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(
            original.attachment_state().unwrap(),
            AttachmentState::Sealed
        );
        assert_eq!(fresh.export_history().unwrap(), history);
        assert_eq!(owner.retained_host_actor_count(), 1);
        let next_cut = owner.freeze_host_cut().unwrap();
        let restored = owner
            .activate_host_cut(
                &next_cut,
                prepared.history.clone(),
                &digest(&prepared.history),
                HostActivationKind::Recovery,
                |_| Ok(()),
            )
            .unwrap();
        assert_ne!(restored.actor_identity(), fresh.actor_identity());
        assert_eq!(fresh.attachment_state().unwrap(), AttachmentState::Sealed);
        assert_eq!(
            owner.retained_host_actor_count(),
            1,
            "local recovery creates no hidden retained old actor"
        );
        assert!(
            owner
                .source()
                .unwrap()
                .store()
                .usage()
                .unwrap()
                .reserved_copy_bytes
                > 0
        );
    }

    #[test]
    fn failed_old_cut_seal_retires_escaped_prepared_actor_and_keeps_gui_source() {
        let temp = temp("c8-activation-old-cut-race");
        let original = Core::spawn(Document::default()).unwrap();
        add_track(&original, 1);
        let mut owner =
            Ja1EditingSession::new(temp.root(), &original, StorageLimits::default()).unwrap();
        let old_authority = owner.source().unwrap().attach_core(&original).unwrap();
        let root = owner.source().unwrap().store().resolved_root().unwrap();
        let prepared = owner.prepare_host_attachment().unwrap();
        let cut = owner.freeze_host_cut().unwrap();
        let replacement = history();
        let mut escaped = None;
        let failed = owner.activate_host_cut(
            &cut,
            replacement.clone(),
            &digest(&replacement),
            HostActivationKind::Rescue,
            |fresh| {
                escaped = Some(fresh.clone());
                old_authority.resume(&cut).unwrap();
                add_track(&original, 3);
                old_authority.freeze().unwrap();
                Ok(())
            },
        );
        assert!(failed.is_err());
        let escaped = escaped.unwrap();
        assert_eq!(escaped.attachment_state().unwrap(), AttachmentState::Sealed);
        assert!(matches!(
            escaped.request(Command::Undo).unwrap(),
            Event::MutationRefused { .. }
        ));
        assert_eq!(owner.core().actor_identity(), original.actor_identity());
        assert_eq!(
            original.attachment_state().unwrap(),
            AttachmentState::Frozen
        );
        assert_ne!(original.export_history().unwrap(), prepared.history);
        assert!(owner.freeze_host_cut().is_err());
        assert_eq!(owner.retained_host_actor_count(), 0);
        assert!(root.parent().unwrap().exists());
        assert!(
            owner
                .source()
                .unwrap()
                .store()
                .usage()
                .unwrap()
                .reserved_copy_bytes
                > 0
        );
    }

    #[test]
    fn publisher_history_matches_exact_report_and_ignores_later_actor_edits() {
        let temp = temp("c8-published-history");
        let core = Core::spawn(Document::default()).unwrap();
        add_track(&core, 1);
        let mut owner =
            Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
        let history = core.export_history().unwrap();
        let status = owner
            .first_save(&temp.path("published.kinewright"), StorageLimits::default())
            .unwrap();
        let CopyStatus::Committed(report) = status else {
            panic!("actual publisher must commit");
        };
        add_track(&core, 2);
        assert_ne!(core.export_history().unwrap(), history);
        assert_eq!(owner.published_history(&report).unwrap(), history);
        let mut wrong = report.clone();
        wrong.history_sha256 = digest(&core.export_history().unwrap());
        assert!(owner.published_history(&wrong).is_err());
        wrong = report.clone();
        wrong.head_id = kinewright_project::bundle_records::HeadId::fresh().unwrap();
        assert!(owner.published_history(&wrong).is_err());
        wrong = report.clone();
        wrong.root_sha256 = "00".repeat(32);
        assert!(owner.published_history(&wrong).is_err());
        wrong = report.clone();
        wrong.byte_length += 1;
        assert!(owner.published_history(&wrong).is_err());
        let next = owner.save_active().unwrap();
        let published = core.export_history().unwrap();
        add_track(&core, 3);
        assert_eq!(owner.published_history(&next).unwrap(), published);
        assert!(owner.published_history(&report).is_err());
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one source ledger witnesses exact quota, refusals and verified copy continuation"
    )]
    fn foreground_snapshot_growth_is_exact_at_copy_cap_and_refusal_preserves_charge() {
        let temp = temp("c8-snapshot-growth-cap");
        let limits = StorageLimits {
            metadata_bytes: 4 << 20,
            decoded_record_bytes: 2 << 20,
            copy_overlap_bytes: 0,
            ..StorageLimits::default()
        };
        let copy_cap = limits.copy_overlap_bytes + limits.metadata_bytes;
        let record_cap = limits.decoded_record_bytes;
        let (mut owner, inputs, _) = source_with_limits(&temp, limits);
        let core = Core::spawn(Document::default()).unwrap();
        let initial = core.export_history().unwrap();
        let descriptor = owner
            .retain_borrowed_source(
                std::slice::from_ref(&inputs),
                initial.encoded_bytes().unwrap(),
            )
            .unwrap();
        let mut service = owner.borrowed_source_service(&descriptor).unwrap();
        let initial_binding = service.ensure_snapshot(&descriptor, &initial).unwrap();
        for id in 1..=12 {
            add_track(&core, id);
        }
        let larger = core.export_history().unwrap();
        let growth = larger.encoded_bytes().unwrap() - initial_binding.encoded_bytes;
        assert!(growth > 0);
        let held_before = owner.store().retention_probe().unwrap();
        let before = owner.store().usage().unwrap();
        // Keep an independently admitted overlap reservation live so the larger
        // snapshot reaches the actual ledger cap exactly, with no limit mutation.
        let independent = owner
            .store()
            .reserve_copy(copy_cap - before.reserved_copy_bytes - growth)
            .unwrap();
        let binding = service.ensure_snapshot(&descriptor, &larger).unwrap();
        let at_cap = owner.store().usage().unwrap();
        assert_eq!(at_cap.reserved_copy_bytes, copy_cap);
        assert_eq!(at_cap.physical_bytes, before.physical_bytes);
        assert_eq!(owner.store().retention_probe().unwrap(), held_before);
        assert_eq!(held_before.0, 1);
        assert_eq!(
            service.ensure_snapshot(&descriptor, &larger).unwrap(),
            binding
        );
        assert_eq!(owner.store().usage().unwrap(), at_cap);
        assert!(
            service
                .prove_snapshot(&descriptor, &initial_binding)
                .is_err()
        );
        service.prove_snapshot(&descriptor, &binding).unwrap();
        add_track(&core, 13);
        let over = core.export_history().unwrap();
        let refused_before = owner.store().usage().unwrap();
        assert!(matches!(
            service.ensure_snapshot(&descriptor, &over),
            Err(BundleError::Capacity(_))
        ));
        assert_eq!(owner.store().usage().unwrap(), refused_before);
        assert_eq!(owner.store().retention_probe().unwrap(), held_before);
        service.prove_snapshot(&descriptor, &binding).unwrap();
        let stale_target = temp.path("stale.kinewright");
        let (stale, trace) = kinewright_project::io_observer::observe(|| {
            BundleCopy::first_save_borrowed(
                &stale_target,
                descriptor.clone(),
                &mut service,
                &initial,
                &inputs,
                StorageLimits::default(),
            )
        });
        assert!(stale.is_err());
        assert!(trace.events.is_empty());
        assert!(!stale_target.exists());
        let target = temp.path("larger.kinewright");
        let copy = BundleCopy::first_save_borrowed(
            &target,
            descriptor.clone(),
            &mut service,
            &larger,
            &inputs,
            StorageLimits::default(),
        )
        .unwrap();
        let CopyStatus::Committed(report) = copy.status() else {
            panic!("larger admitted copy must commit");
        };
        assert_eq!(report.history_sha256, binding.history_sha256);
        assert_eq!(owner.store().usage().unwrap().reserved_copy_bytes, copy_cap);
        assert_eq!(owner.store().retention_probe().unwrap(), held_before);
        drop(copy);
        drop(independent);
        assert_eq!(
            owner.store().usage().unwrap().reserved_copy_bytes,
            before.reserved_copy_bytes + growth
        );
        assert_eq!(owner.store().retention_probe().unwrap(), held_before);
        // A later metadata refusal occurs after reserving the new copy delta.
        // Only that uncommitted extension may unwind; the admitted B charge stays.
        let metadata_before = owner.store().usage().unwrap();
        let leave = over.encoded_bytes().unwrap() - 1;
        let padding =
            copy_cap - metadata_before.metadata_bytes - metadata_before.decoded_bytes - leave;
        let scratch = (
            owner
                .store()
                .reserve_probe(padding.min(record_cap), 0)
                .unwrap(),
            owner
                .store()
                .reserve_probe(padding.saturating_sub(record_cap), 0)
                .unwrap(),
        );
        let refused_before = owner.store().usage().unwrap();
        assert_eq!(
            refused_before.metadata_bytes + refused_before.decoded_bytes + leave,
            copy_cap
        );
        assert!(matches!(
            service.ensure_snapshot(&descriptor, &over),
            Err(BundleError::Capacity(_))
        ));
        let refused_after = owner.store().usage().unwrap();
        assert_eq!(
            refused_after.reserved_copy_bytes,
            refused_before.reserved_copy_bytes
        );
        assert_eq!(refused_after.reserved_bytes, refused_before.reserved_bytes);
        assert_eq!(refused_after.decoded_bytes, refused_before.decoded_bytes);
        assert_eq!(refused_after.physical_bytes, refused_before.physical_bytes);
        assert!(!refused_after.damaged_accounting);
        assert_eq!(owner.store().retention_probe().unwrap(), held_before);
        service.prove_snapshot(&descriptor, &binding).unwrap();
        drop(scratch);
    }

    #[test]
    fn gui_foreground_cut_grows_its_existing_source_admission_before_first_save() {
        let temp = temp("c8-gui-snapshot-growth");
        let core = Core::spawn(Document::default()).unwrap();
        let mut owner =
            Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
        let prepared = owner.prepare_host_attachment().unwrap();
        let first_cut = owner.freeze_host_cut().unwrap();
        let initial_bytes = first_cut.history().encoded_bytes().unwrap();
        let before = owner.source().unwrap().store().usage().unwrap();
        let holds = owner.source().unwrap().store().retention_probe().unwrap();
        owner.complete_host_cut(&first_cut, None, None).unwrap();
        for id in 1..=12 {
            add_track(&core, id);
        }
        let cut = owner.freeze_host_cut().unwrap();
        let binding = BorrowedSnapshotBinding::from_history(cut.history()).unwrap();
        prepared
            .source_service
            .as_ref()
            .unwrap()
            .prove_snapshot(prepared.source.as_ref().unwrap(), &binding)
            .expect("freeze must admit the larger snapshot before any explicit ensure call");
        assert_eq!(binding.history_sha256, digest(cut.history()));
        let after = owner.source().unwrap().store().usage().unwrap();
        assert_eq!(
            after.reserved_copy_bytes - before.reserved_copy_bytes,
            binding.encoded_bytes - initial_bytes
        );
        assert_eq!(after.physical_bytes, before.physical_bytes);
        assert_eq!(
            owner.source().unwrap().store().retention_probe().unwrap(),
            holds
        );
        assert_eq!(owner.ensure_host_snapshot(&cut).unwrap().unwrap(), binding);
        assert_eq!(owner.freeze_host_cut().unwrap(), cut);
        assert_eq!(owner.source().unwrap().store().usage().unwrap(), after);
        let mut service = prepared.source_service.unwrap();
        let source = prepared.source.unwrap();
        service.prove_snapshot(&source, &binding).unwrap();
        let copy = BundleCopy::first_save_borrowed(
            &temp.path("foreground.kinewright"),
            source,
            &mut service,
            cut.history(),
            &prepared.inputs,
            StorageLimits::default(),
        )
        .unwrap();
        assert!(matches!(copy.status(), CopyStatus::Committed(_)));
    }

    #[test]
    fn pinned_save_as_preserves_dirty_source_floor_and_external_nested_copy() {
        let temp = temp("c8-admitted-save-as");
        let path = temp.path("original/a.kinewright");
        let target = temp.path("external/nested/b.kinewright");
        let core = Core::spawn(Document::default()).unwrap();
        add_track(&core, 1);
        let mut owner =
            Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
        owner.first_save(&path, StorageLimits::default()).unwrap();
        for id in 2..=20 {
            add_track(&core, id);
        }
        let candidate = core.export_history().unwrap();
        let floor = kinewright_core::HistoryTransition::floor(candidate.clone(), 10).unwrap();
        let limits = StorageLimits {
            branch_history_bytes: kinewright_project::bundle_commit::history_cost_probe(
                floor.after(),
                &StorageLimits::default(),
            )
            .unwrap(),
            ..StorageLimits::default()
        };
        let expected =
            kinewright_project::bundle_commit::bound_history_probe(&candidate, &limits).unwrap();
        assert!(expected.floor_offset > candidate.floor_offset);
        owner.restrict_limits_probe(limits.clone()).unwrap();
        let inspection = owner.inspect_rescue_destination(&target).unwrap();
        let status = owner.save_as_admitted(&inspection, limits).unwrap();
        let CopyStatus::Committed(report) = status else {
            panic!("admitted external Save As must commit");
        };
        assert_eq!(report.history_sha256, digest(&expected));
        assert_eq!(core.export_history().unwrap(), expected);
        assert!(!owner.dirty().unwrap());
        assert_eq!(owner.published_history(&report).unwrap(), expected);
        assert_eq!(
            LoadedBundle::read(&path)
                .unwrap()
                .manual_checkpoint()
                .unwrap()
                .history_ref
                .sha256,
            report.history_sha256
        );
        assert_eq!(
            LoadedBundle::read(&target)
                .unwrap()
                .manual_checkpoint()
                .unwrap()
                .history_ref
                .sha256,
            report.history_sha256
        );
        let alias = temp.path("external/nested/alias.kinewright");
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        add_track(&core, 99);
        let same_project =
            kinewright_project::store::ProjectStore::inspect_borrowed_destination(&alias, &[])
                .unwrap();
        let same = owner
            .save_as_admitted(&same_project, StorageLimits::default())
            .unwrap();
        let CopyStatus::Committed(same) = same else {
            panic!("same-project alias must ordinary Save");
        };
        assert_eq!(same.project_id, report.project_id);
        assert_eq!(same.history_sha256, digest(&core.export_history().unwrap()));
        assert_eq!(owner.core().actor_identity(), core.actor_identity());
    }

    #[test]
    fn pinned_save_as_rejects_replaced_inspected_parent_before_target_mutation() {
        let temp = temp("c8-admitted-save-as-parent");
        let core = Core::spawn(Document::default()).unwrap();
        add_track(&core, 1);
        let mut owner =
            Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
        owner
            .first_save(&temp.path("a.kinewright"), StorageLimits::default())
            .unwrap();
        let parent = temp.path("admitted");
        std::fs::create_dir(&parent).unwrap();
        let target = parent.join("b.kinewright");
        let inspection = owner.inspect_rescue_destination(&target).unwrap();
        let original = temp.path("original-admitted");
        std::fs::rename(&parent, &original).unwrap();
        std::fs::create_dir(&parent).unwrap();
        let before = core.export_history().unwrap();
        assert!(matches!(
            owner.save_as_admitted(&inspection, StorageLimits::default()),
            Err(BundleError::Identity(_))
        ));
        assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 0);
        assert_eq!(std::fs::read_dir(&original).unwrap().count(), 0);
        assert_eq!(core.export_history().unwrap(), before);
        assert!(!owner.dirty().unwrap());
    }

    fn dirty_selection(
        temp: &TempDirectory,
    ) -> (Ja1EditingSession, HistorySnapshot, RecoverySelection) {
        let core = Core::spawn(Document::default()).unwrap();
        add_track(&core, 1);
        let mut owner =
            Ja1EditingSession::new(temp.root(), &core, StorageLimits::default()).unwrap();
        owner
            .first_save(&temp.path("a.kinewright"), StorageLimits::default())
            .unwrap();
        add_track(&core, 2);
        owner.save_active().unwrap();
        let target = owner.list_recovery_points().unwrap().remove(0);
        for id in 3..=20 {
            add_track(&core, id);
        }
        let candidate = core.export_history().unwrap();
        let floor = kinewright_core::HistoryTransition::floor(candidate.clone(), 10).unwrap();
        let limits = StorageLimits {
            branch_history_bytes: kinewright_project::bundle_commit::history_cost_probe(
                floor.after(),
                &StorageLimits::default(),
            )
            .unwrap(),
            ..StorageLimits::default()
        };
        let expected =
            kinewright_project::bundle_commit::bound_history_probe(&candidate, &limits).unwrap();
        assert!(expected.floor_offset > candidate.floor_offset);
        owner.restrict_limits_probe(limits).unwrap();
        owner.prepare_recovery(target.clone()).unwrap();
        (owner, expected, target)
    }

    #[test]
    fn preliminary_save_report_survives_selection_refusal_unknown_and_activation() {
        for fault in [Some("root_before_publish"), Some("root_parent_sync"), None] {
            let temp = temp("c8-preliminary-save-report");
            let (mut owner, expected, _) = dirty_selection(&temp);
            let result = if let Some(fault) = fault {
                kinewright_project::io_observer::fail_nth(fault, 2, || owner.select_local())
            } else {
                owner.select_local()
            };
            match (&result, fault) {
                (ProjectSelectionResult::Refused(_), Some("root_before_publish"))
                | (ProjectSelectionResult::Unknown, Some("root_parent_sync"))
                | (ProjectSelectionResult::Activated(_), None) => {}
                _ => panic!("unexpected selection result: {result:?}"),
            }
            let report = owner
                .take_preliminary_save_report()
                .expect("the actual preceding L Save must be delivered");
            assert!(owner.take_preliminary_save_report().is_none());
            assert_eq!(report.history_sha256, digest(&expected));
            assert_eq!(owner.published_history(&report).unwrap(), expected);
            let mut wrong = report.clone();
            wrong.root_sha256 = "00".repeat(32);
            assert!(owner.published_history(&wrong).is_err());
            if let ProjectSelectionResult::Activated(selected) = result {
                assert!(report.generation < selected.generation);
                assert_ne!(report.head_id, selected.head_id);
                assert_ne!(owner.core().export_history().unwrap(), expected);
                assert_eq!(
                    owner.published_history(&selected).unwrap(),
                    owner.core().export_history().unwrap()
                );
                assert_eq!(owner.published_history(&report).unwrap(), expected);
            }
        }
    }

    #[test]
    fn preliminary_reconciliation_report_is_publisher_derived_and_delivered_once() {
        let temp = temp("c8-preliminary-reconcile-report");
        let (mut owner, expected, _) = dirty_selection(&temp);
        assert!(matches!(
            kinewright_project::io_observer::fail_once("root_parent_sync", || owner.select_local()),
            ProjectSelectionResult::Unknown
        ));
        assert!(owner.take_preliminary_save_report().is_none());
        let selected = owner.select_local();
        assert!(
            matches!(selected, ProjectSelectionResult::Activated(_)),
            "{selected:?}"
        );
        let report = owner
            .take_preliminary_save_report()
            .expect("actual committed reconciliation must be delivered");
        assert!(owner.take_preliminary_save_report().is_none());
        assert_eq!(report.history_sha256, digest(&expected));
        assert_eq!(owner.published_history(&report).unwrap(), expected);
        assert_ne!(owner.core().export_history().unwrap(), expected);
    }
}
