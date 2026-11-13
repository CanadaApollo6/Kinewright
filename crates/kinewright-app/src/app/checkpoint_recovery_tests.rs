//! Person-facing checkpoint rescue and successful-prefix journal recovery.
#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::app::in1_tests::in1_shutdown;
    use crate::app::in2b_tests::{in2b_harness, in2b_quiesce_engine};
    use kinewright_core::{Core, Track, TrackKind};
    use kinewright_media::test_support::{TempDirectory, workspace_target_dir};
    use kinewright_project::{
        bundle::{BundleWriteAdmission, LoadedBundle},
        bundle_copy::CopyStatus,
        bundle_records::LineageRecord,
        editing_session::Ja1EditingSession,
        owned_inputs::OwnedInputRef,
        storage_budget::StorageLimits,
    };
    use std::sync::atomic::AtomicBool;

    fn wav() -> Vec<u8> {
        let length = 9600_u32;
        let mut bytes = Vec::new();
        bytes.extend(b"RIFF");
        bytes.extend((length + 36).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16_u32.to_le_bytes());
        bytes.extend(1_u16.to_le_bytes());
        bytes.extend(1_u16.to_le_bytes());
        bytes.extend(48000_u32.to_le_bytes());
        bytes.extend(96000_u32.to_le_bytes());
        bytes.extend(2_u16.to_le_bytes());
        bytes.extend(16_u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend(length.to_le_bytes());
        for _ in 0..length / 2 {
            bytes.extend(12_i16.to_le_bytes());
        }
        bytes
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one real app open, owned-media rescue, activation and source-retention witness"
    )]
    fn history_checkpoint_app_rescue_keeps_owned_bytes_and_occurrences() {
        let root = TempDirectory::new_in(&workspace_target_dir(), "history-app-rescue");
        let source = root.path("source.kinewright");
        let destination = root.path("recovered.kinewright");
        let locator = root.path("original.wav");
        std::fs::write(&locator, wav()).unwrap();
        let mut document = Document::default();
        document.tracks.push(Track {
            id: TrackId(1),
            kind: TrackKind::Video,
            sync_lock: true,
            clips: vec![
                serde_json::from_value(serde_json::json!({
                    "id":7,"asset":0,"source_range":{"start":0,"end":90},
                    "content":{"solid":{"r":0,"g":0,"b":0}},
                    "timeline_start":0,"effects":[],"transition_in":null
                }))
                .unwrap(),
            ],
        });
        document.duration = TimeCode(90);
        let core = Core::spawn(document).unwrap();
        let mut owner =
            Ja1EditingSession::new(root.root(), &core, StorageLimits::default()).unwrap();
        owner.import_owned_media(&locator).unwrap();
        assert!(matches!(
            owner.first_save(&source, StorageLimits::default()).unwrap(),
            CopyStatus::Committed(_)
        ));
        let checkpoint = LoadedBundle::read(&source)
            .unwrap()
            .manual_checkpoint()
            .unwrap()
            .clone();
        let expected = core.export_history().unwrap().document;
        let occurrences = owner
            .store()
            .unwrap()
            .read_record::<LineageRecord>(&checkpoint.lineage_ref)
            .unwrap()
            .current
            .clone();
        drop(owner);
        drop(core);
        std::fs::remove_file(&locator).unwrap();
        let history_path = root.path(&format!(
            "source.kinewright-assets/studio/objects/{}",
            checkpoint.history_ref.sha256
        ));
        let history_bytes = std::fs::read(&history_path).unwrap();
        let damaged = vec![b'x'; history_bytes.len()];
        std::fs::write(&history_path, &damaged).unwrap();
        let original_root = std::fs::read(&source).unwrap();

        crate::recovery::with_ja_directory(root.path("recovery"), || {
            let (mut app, engine) = in2b_harness(Document::default(), None);
            app.open_project(&source);
            app.enable_ja1();
            assert!(app.focused().ja1.is_none());
            assert!(
                app.focused().checkpoint_recovery.is_some(),
                "{}",
                app.status
            );
            assert!(app.focused().ja1_read_only());
            assert!(app.status.contains("Undo history could not be restored"));
            let before = app.focused().core.export_history().unwrap();
            assert_eq!(before.document, expected);
            assert!(before.batches.is_empty() && before.cursor.undo.is_empty());
            app.send_operation(Operation::SetTrackSyncLock {
                track: TrackId(1),
                locked: false,
            });
            assert_eq!(
                app.focused().core.export_history().unwrap(),
                before,
                "RECOVERY_VIEW_IS_READ_ONLY"
            );
            assert!(
                !app.save_checkpoint_recovery(&source),
                "RECOVERY_CANNOT_OVERWRITE_SOURCE"
            );
            assert!(app.save_checkpoint_recovery(&destination), "{}", app.status);
            assert_eq!(app.focused().document, expected);
            app.enable_ja1();
            assert!(app.focused().ja1.is_some(), "{}", app.status);
            assert!(!app.focused().ja1_read_only(), "RECOVERED_COPY_IS_EDITABLE");
            in2b_quiesce_engine(&engine);
            in1_shutdown(&mut app);
            drop(app);
        });

        let owner = Ja1EditingSession::open(&destination).unwrap();
        let loaded = LoadedBundle::read(&destination).unwrap();
        let checkpoint = loaded.manual_checkpoint().unwrap();
        assert_eq!(
            owner
                .store()
                .unwrap()
                .read_record::<LineageRecord>(&checkpoint.lineage_ref)
                .unwrap()
                .current,
            occurrences,
            "CURRENT_OCCURRENCES_SURVIVE_APP_RESCUE"
        );
        let manifest = owner
            .store()
            .unwrap()
            .read_input_manifest(&checkpoint.input_manifest_ref)
            .unwrap();
        let entry = manifest
            .entries
            .iter()
            .find(|entry| {
                entry.object.record_kind
                    == kinewright_project::bundle_records::RecordKind::OwnedMedia
            })
            .unwrap();
        let input = OwnedInputRef {
            manifest_ref: checkpoint.input_manifest_ref.clone(),
            entry: entry.clone(),
        };
        let media = owner.store().unwrap().certify_media_input(&input).unwrap();
        assert_eq!(
            media.probe(Arc::new(AtomicBool::new(false))).unwrap().kind,
            kinewright_core::MediaKind::Audio
        );
        assert!(
            media.open_audio(Arc::new(AtomicBool::new(false))).is_ok(),
            "RECOVERED_OWNED_AUDIO_OPENS_WITHOUT_ORIGINAL_LOCATOR"
        );
        assert_eq!(std::fs::read(&source).unwrap(), original_root);
        assert_eq!(std::fs::read(&history_path).unwrap(), damaged);
        assert!(
            LoadedBundle::read(&source).is_err(),
            "ORIGINAL_HISTORY_REMAINS_QUARANTINED"
        );
    }

    #[test]
    fn history_journal_app_keeps_failed_suffix_after_save_close_and_restart() {
        let root = TempDirectory::new_in(&workspace_target_dir(), "history-prefix-retention");
        let project = root.path("saved.kinewright");
        let saved = root.path("recovered.kinewright");
        let directory = root.path("recovery");
        std::fs::create_dir_all(&directory).unwrap();
        let original = Document::default();
        std::fs::write(
            &project,
            serde_json::to_vec(&kinewright_project::ProjectFile {
                format_version: PROJECT_FORMAT_VERSION,
                studio: None,
                document: original.clone(),
            })
            .unwrap(),
        )
        .unwrap();
        let journal = directory.join("pending.journal");
        let mut bytes = kinewright_project::JOURNAL_MAGIC.to_vec();
        bytes.extend(serde_json::to_vec(&serde_json::json!({"format_version":PROJECT_FORMAT_VERSION,"project_path":project,"initial_document":original})).unwrap());
        bytes.push(b'\n');
        let commands = [
            JournalCommand::Do(Operation::AddTrack {
                track: Track {
                    id: TrackId(1),
                    kind: TrackKind::Video,
                    sync_lock: true,
                    clips: vec![],
                },
            }),
            JournalCommand::Do(Operation::SetTrackSyncLock {
                track: TrackId(999),
                locked: false,
            }),
            JournalCommand::Do(Operation::AddTrack {
                track: Track {
                    id: TrackId(2),
                    kind: TrackKind::Audio,
                    sync_lock: true,
                    clips: vec![],
                },
            }),
        ];
        for command in commands {
            bytes.extend(serde_json::to_vec(&command).unwrap());
            bytes.push(b'\n');
        }
        std::fs::write(&journal, &bytes).unwrap();
        crate::recovery::with_ja_directory(directory, || {
            let (mut app, engine) = in2b_harness(Document::default(), Some(project.clone()));
            let request = app.projects[0].recovery.pending_restore_for_test(&journal);
            assert!(request.damage.is_some());
            let prefix = request.document.clone();
            assert_eq!(
                prefix.tracks.len(),
                1,
                "ONLY_SUCCESSFUL_PREFIX_IS_RECOVERED"
            );
            app.apply_restore_request(request);
            assert_eq!(*app.focused().document, prefix);
            assert!(
                app.status.contains("successful prefix")
                    && app.status.contains("unrecovered bytes")
                    && app.status.contains("retained"),
                "{}",
                app.status
            );
            assert_eq!(
                std::fs::read(&journal).unwrap(),
                bytes,
                "FAILED_SUFFIX_KEPT_AFTER_RESTORE"
            );
            app.write_project(&saved).unwrap();
            assert_eq!(
                std::fs::read(&journal).unwrap(),
                bytes,
                "FAILED_SUFFIX_KEPT_AFTER_SAVE"
            );
            in2b_quiesce_engine(&engine);
            in1_shutdown(&mut app);
            drop(app);
            assert_eq!(
                std::fs::read(&journal).unwrap(),
                bytes,
                "FAILED_SUFFIX_KEPT_AFTER_CLOSE"
            );
            let (mut restarted, engine) = in2b_harness(Document::default(), None);
            let request = restarted.projects[0]
                .recovery
                .pending_restore_for_test(&journal);
            assert!(
                request.damage.is_some(),
                "RESTART_DISCLOSES_UNRECOVERED_SUFFIX_AGAIN"
            );
            assert_eq!(request.document, prefix);
            in2b_quiesce_engine(&engine);
            in1_shutdown(&mut restarted);
            assert_eq!(std::fs::read(&journal).unwrap(), bytes);
        });
        assert!(BundleWriteAdmission::read(&saved).is_ok());
    }
}
