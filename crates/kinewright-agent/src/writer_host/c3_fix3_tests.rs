fn restart_plain_copy_fixture(f: Fixture, name: &str) -> Fixture {
    let Fixture {
        host,
        temp,
        brief,
        issuer_auth,
        ..
    } = f;
    drop(host);
    let roots = SessionRoots {
        read: vec![temp.root().into()],
        write: vec![temp.root().into()],
    };
    let host = WriterHost::start_with_issuer(
        HostConfig {
            scheduler_settings: None,
            project: "hooks".into(),
            path: Some(temp.path(name)),
            roots: roots.clone(),
            origin: HostOrigin::Headless,
        },
        HostAuth::generate().unwrap(),
        Some(issuer_auth.clone()),
    )
    .unwrap();
    let editor = host.client();
    let issuer = HostIssuer::new(host.endpoint.clone(), issuer_auth.clone(), roots).unwrap();
    Fixture {
        host,
        editor,
        issuer,
        issuer_auth,
        brief,
        temp,
    }
}
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one real copy and both destination/source restarts retain the complete witness"
)]
fn s4b_c3_fix3_copies_keep_plain_task_history_and_original_run() {
    use kinewright_core::Command;
    use kinewright_project::bundle::LoadedBundle;
    for rescue in [false, true] {
        let original = if rescue {
            "drive/project.kinewright"
        } else {
            "project.kinewright"
        };
        let f = Fixture::with_document_at(false, Document::default(), original);
        let start = f.start();
        let run: RunId = start.envelope.identity("run").unwrap();
        let machine = f.temp.path("history-machine");
        let claim = crate::execution_claim::MachineClaim::acquire_fixture(
            &machine,
            start.envelope.project.as_str(),
            run.0.as_str(),
        )
        .unwrap();
        let (execution, _, _, core) = history_task_endpoint(&f, &run);
        for id in [1, 2] {
            core.request(Command::Do(history_marker(id))).unwrap();
        }
        core.request(Command::Undo).unwrap();
        let expected = core.export_history().unwrap();
        assert!(!expected.cursor.redo.is_empty());
        let producer = f.producer(&run);
        let saved = f
            .host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .checkpoint_task_history(&producer, &run, &execution.task)
            .unwrap();
        let (source_root, source_state) = {
            let loaded = LoadedBundle::read(&f.temp.path(original)).unwrap();
            (loaded.root().unwrap().clone(), f.projection(&run))
        };
        let name = if rescue {
            "rescued.kinewright"
        } else {
            "copied.kinewright"
        };
        let reply = if rescue {
            let accepted = {
                let host = f.host.owner.upgrade().unwrap();
                let mut owner = host.lock().unwrap();
                owner
                    .core()
                    .request(Command::Do(history_marker(50)))
                    .unwrap();
                let accepted = owner.core().export_history().unwrap();
                assert!(
                    kinewright_project::io_observer::fail_once("root_parent_sync", || owner
                        .owner_mut()
                        .unwrap()
                        .save_active())
                    .is_err()
                );
                accepted
            };
            std::fs::rename(f.temp.path("drive"), f.temp.path("hidden-drive")).unwrap();
            let preview = f
                .editor
                .request(HostCommand::PreviewRescue {
                    snapshot: HostSnapshot::from_history(accepted).unwrap(),
                })
                .unwrap();
            assert!(
                preview.error.is_none(),
                "C3_RESCUE_PREVIEW {:?}",
                preview.error
            );
            let reply = f
                .editor
                .request(HostCommand::RescueTo {
                    destination: f.temp.path(name),
                })
                .unwrap();
            std::fs::rename(f.temp.path("hidden-drive"), f.temp.path("drive")).unwrap();
            reply
        } else {
            native_save_as(&f, name)
        };
        assert!(
            reply.error.is_none(),
            "C3_COPY_PLAIN_TASK_BOUNDARY rescue={rescue} copy must succeed: {:?}",
            reply.error
        );
        drop(core);
        drop(claim);
        let f = restart_plain_copy_fixture(f, name);
        {
            let host = f.host.owner.upgrade().unwrap();
            let mut owner = host.lock().unwrap();
            let session = owner.owner().unwrap();
            let copied = session.candidate_binding(&saved.source.branch).unwrap();
            eprintln!(
                "C3_COPY_REOPEN rescue={rescue} checkpoint_exact={} inherited_run={}",
                copied == saved.source,
                session.orchestration_projection(&run).unwrap().is_some()
            );
            assert_eq!(
                copied, saved.source,
                "C3_COPY_EXACT_CHECKPOINT_BYTES rescue={rescue}"
            );
            let checkpoint = session
                .store()
                .unwrap()
                .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(
                    &copied.checkpoint,
                )
                .unwrap();
            let history = session
                .store()
                .unwrap()
                .read_record::<kinewright_core::HistorySnapshot>(&checkpoint.history_ref)
                .unwrap();
            assert_eq!(*history, expected);
            let loaded = LoadedBundle::read(&f.temp.path(name)).unwrap();
            assert!(
                loaded.root().unwrap().run_index_ref.is_none(),
                "C3_COPY_PLAIN_TASK_BOUNDARY rescue={rescue}"
            );
            assert_eq!(
                loaded.root().unwrap().receipt_index_ref,
                source_root.receipt_index_ref,
                "C3_COPY_RECEIPT_BYTES"
            );
            assert!(session.orchestration_runs().unwrap().is_empty());
            assert!(session.orchestration_projection(&run).unwrap().is_none());
            let before = session.root_sha256().unwrap();
            assert!(owner.restore_native_run(&run).is_err());
            assert_eq!(
                owner.owner().unwrap().root_sha256().unwrap(),
                before,
                "C3_COPY_RECOVERY_NO_OP"
            );
        }
        if let Some(directory) = std::env::var_os("KINEWRIGHT_C3_COPY_EVIDENCE_DIR") {
            let directory = std::path::PathBuf::from(directory);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::copy(f.temp.path(name), directory.join(name)).unwrap();
        }
        {
            use kinewright_project::{
                bundle_recovery::RecoverySelection, bundle_switch::ProjectSelectionResult,
            };
            let host = f.host.owner.upgrade().unwrap();
            let mut owner = host.lock().unwrap();
            let session = owner.owner_mut().unwrap();
            let archived = session
                .list_recovery_points()
                .unwrap()
                .into_iter()
                .next()
                .unwrap();
            session.prepare_recovery(archived).unwrap();
            assert!(
                matches!(session.select_local(), ProjectSelectionResult::Activated(_)),
                "C3_COPY_ARCHIVE_ADOPTION"
            );
            assert!(
                session.orchestration_runs().unwrap().is_empty(),
                "C3_COPY_ARCHIVE_CANNOT_RESTORE_RUN"
            );
            session
                .prepare_recovery(RecoverySelection::WorkingBranch(
                    saved.source.branch.clone(),
                ))
                .unwrap();
            assert!(
                matches!(session.select_local(), ProjectSelectionResult::Activated(_)),
                "C3_PLAIN_BRANCH_SELECTION"
            );
            assert_eq!(session.core().export_history().unwrap(), expected);
            session.core().request(Command::Redo).unwrap();
            session.save_active().unwrap();
            assert!(
                session.orchestration_runs().unwrap().is_empty(),
                "C3_PLAIN_BRANCH_SAVE_CANNOT_RESTORE_RUN"
            );
        }
        let f = restart_plain_copy_fixture(f, original);
        {
            let host = f.host.owner.upgrade().unwrap();
            let owner = host.lock().unwrap();
            let session = owner.owner().unwrap();
            assert_eq!(
                session.orchestration_projection(&run).unwrap().unwrap(),
                source_state
            );
            assert_eq!(
                owner
                    .saved_task_history(&source_state, &execution.task)
                    .unwrap()
                    .unwrap(),
                saved
            );
            assert_eq!(
                session.candidate_binding(&saved.source.branch).unwrap(),
                saved.source
            );
        }
        let host = f.host.owner.upgrade().unwrap();
        let claim = crate::execution_claim::MachineClaim::recover_fixture(&machine).unwrap();
        let (claim, _) = crate::effect_pump::recover_run(&host, claim, &run).unwrap();
        let (_, _, _, restored) = history_task_endpoint(&f, &run);
        assert_eq!(restored.export_history().unwrap(), expected);
        restored.request(Command::Redo).unwrap();
        assert!(restored.export_history().unwrap().cursor.redo.is_empty());
        drop(restored);
        drop(host);
        claim.finish().unwrap();
        eprintln!(
            "C3_COPY_PLAIN rescue={rescue} checkpoint_exact=true history_exact=true receipts_exact=true inherited_run=false recovery_noop=true original_continues=true"
        );
    }
}
