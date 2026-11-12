#[cfg(test)]
mod kept_audit {
    use super::*;
    fn remove_clip(b: &B21, clip: u64) {
        let kinewright_core::Event::QueryResult(kinewright_core::QueryResult::Snapshot {
            revision,
            document,
        }) = b
            .core
            .request(kinewright_core::Command::Query(
                kinewright_core::Query::Snapshot,
            ))
            .unwrap()
        else {
            panic!("snapshot")
        };
        assert!(matches!(
            control(b)
                .apply_with_edit_consent(
                    &crate::ConfirmationBroker::default(),
                    &b.core,
                    revision,
                    &document,
                    &[kinewright_core::Operation::DeleteClip {
                        clip: kinewright_core::ClipId(clip)
                    }],
                    Some("Remove clip")
                )
                .unwrap()
                .unwrap(),
            kinewright_core::Event::DocumentChanged { .. }
        ));
    }
    #[test]
    fn c4b_run_limit_audit_crash_child() {
        let Some(handoff) = std::env::var_os("KINEWRIGHT_C4B_AUDIT_HANDOFF") else {
            return;
        };
        let b = b21_start();
        remove_clip(&b, 1);
        remove_clip(&b, 2);
        let state = b.f.projection(&b.run);
        assert_eq!(
            (state.edit_approvals.len(), state.edit_applications.len()),
            (2, 2)
        );
        let live = b201_page(&b.f, &b.run);
        assert!(live.edit_approvals.iter().all(|row| row.applied));
        assert!(
            live.missing_approved_edits.is_empty(),
            "C4B_LIVE_ACTOR_STILL_HAS_THE_EDITS"
        );
        if std::env::var_os("KINEWRIGHT_C4B_KEEP_CHECKPOINT").is_some() {
            let owner = b.f.host.owner.upgrade().unwrap();
            let mut host = owner.lock().unwrap();
            let producer = host.run_producer(&b.run).unwrap();
            host.checkpoint_task_history(&producer, &b.run, &b.child)
                .unwrap();
        }
        let kept = c5e_history(&b.f, &b.run, &b.child);
        std::fs::write(handoff, serde_json::to_vec(&serde_json::json!({"root": b.f.temp.root(), "run": b.run, "task": b.child, "kept": kept})).unwrap()).unwrap();
        // No Drop, Stop, freeze, or final task checkpoint after the durable audits.
        std::process::exit(86);
    }
    fn stop_with_absence_proof(host: &WriterHost, run: &RunId, root: &std::path::Path) {
        let shared = host.owner.upgrade().unwrap();
        let state = shared
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .orchestration_projection(run)
            .unwrap()
            .unwrap();
        let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
            &root.join("audit-stop-machine"),
            state.attempts[0].identity.project.as_str(),
            run.0.as_str(),
        )
        .unwrap();
        let mut proofs = Vec::new();
        for attempt in state
            .attempts
            .iter()
            .filter(|attempt| !matches!(attempt.state, AttemptState::Retired(_)))
        {
            let native = crate::native_launcher::native_identity(&attempt.identity);
            claim.reserve(&native).unwrap();
            proofs.push((
                attempt.identity.clone(),
                crate::guardian::RecoveryObserver::unbound_cleanup(&mut claim, &native).unwrap(),
            ));
        }
        let client = host.client();
        let issued = ok(
            client
                .request(HostCommand::IssueMetadataIssuer { issuer: None })
                .unwrap(),
            "C4B_STOP_ISSUER",
        )
        .metadata_issuer
        .unwrap();
        let state = shared
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .orchestration_projection(run)
            .unwrap()
            .unwrap();
        let mut envelope = OrchestrationEnvelope {
            id: issued.envelope.id,
            digest: serde_json::from_value(serde_json::json!("0".repeat(64))).unwrap(),
            project: issued.envelope.project_id,
            expected_generation: issued.root_generation,
            authority: AuthorityBinding {
                grant_ref: issued.grant_ref,
                owner_epoch: issued.envelope.owner_epoch,
                endpoint: None,
                attempt: None,
            },
            command: OrchestrationCommand::ControlRun(RunIntent {
                command_id: CommandId {
                    issuer: serde_json::from_value(serde_json::json!("0".repeat(32))).unwrap(),
                    sequence: 1,
                },
                run_id: run.clone(),
                expected_control_revision: state.control_revision,
                action: RunAction::Stop,
            }),
        };
        let OrchestrationCommand::ControlRun(intent) = &mut envelope.command else {
            unreachable!()
        };
        intent.command_id = envelope.id.clone();
        envelope.digest = envelope.computed_digest().unwrap();
        ok(
            client
                .request(HostCommand::DispatchOrchestration {
                    request: Box::new(ScopedOrchestrationRequest {
                        run: Some(run.clone()),
                        envelope,
                        consume_batch: None,
                    }),
                })
                .unwrap(),
            "C4B_RECOVERY_STOP",
        );
        let producer = shared.lock().unwrap().run_producer(run).unwrap();
        for (identity, proof) in &proofs {
            shared
                .lock()
                .unwrap()
                .reconcile_native_effect(
                    &producer,
                    identity,
                    &crate::guardian::AttemptRetirementProof::Unbound(proof),
                )
                .unwrap();
            claim.retire_unbound(proof).unwrap();
        }
        claim.finish().unwrap();
        assert_eq!(
            shared
                .lock()
                .unwrap()
                .owner()
                .unwrap()
                .orchestration_projection(run)
                .unwrap()
                .unwrap()
                .cleanup,
            RunCleanup::Settled
        );
    }
    fn assert_rows(
        page: &crate::writer_host::orchestration::RunViewPage,
        task: &TaskId,
        gap: bool,
    ) {
        let rows = page
            .edit_approvals
            .iter()
            .filter(|row| &row.task == task)
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter().all(|row| row.applied != gap),
            "C4B_KEPT_TRUTH {rows:?}"
        );
        assert_eq!(page.missing_approved_edits.len(), if gap { 2 } else { 0 });
        if gap {
            let words = kinewright_project::edit_audit::missing_edits_disclosure(
                &page.missing_approved_edits,
            );
            assert!(words.contains("2 approved edits are not in the kept version"));
            assert!(
                words.contains("clip 1") && words.contains("clip 2") && words.contains("track 81")
            );
            assert!(words.contains("Re-run") && words.contains("continue the task"));
        }
    }
    #[test]
    fn c4b_run_limit_reopen_discloses_two_missing_edits_and_checkpoint_control() {
        if !te15_fix_counting_child(
            "tests::storage_admission::tests::kept_audit::c4b_run_limit_reopen_discloses_two_missing_edits_and_checkpoint_control",
        ) {
            return;
        }
        for checkpoint in [false, true] {
            let temp = kinewright_media::test_support::TempDirectory::new_in(
                &kinewright_media::test_support::workspace_target_dir(),
                "c4b-audit-handoff",
            );
            let log = std::fs::File::create(temp.path("crash.log")).unwrap();
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command.args(["--exact", "writer_host::hooks_tests::tests::tests::storage_admission::tests::kept_audit::c4b_run_limit_audit_crash_child", "--nocapture"])
                .env("KINEWRIGHT_C4B_AUDIT_HANDOFF", temp.path("handoff.json")).stdout(log.try_clone().unwrap()).stderr(log);
            if checkpoint {
                command.env("KINEWRIGHT_C4B_KEEP_CHECKPOINT", "1");
            }
            let mut child = command.spawn().unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(35);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                if std::time::Instant::now() >= deadline {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    panic!("C4B_CRASH_CHILD_TIMEOUT");
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            assert_eq!(
                status.code(),
                Some(86),
                "C4B_CRASH_CUT {}",
                std::fs::read_to_string(temp.path("crash.log")).unwrap()
            );
            let mut handoff: serde_json::Value =
                serde_json::from_slice(&std::fs::read(temp.path("handoff.json")).unwrap()).unwrap();
            let root: std::path::PathBuf = serde_json::from_value(handoff["root"].take()).unwrap();
            let run: RunId = serde_json::from_value(handoff["run"].take()).unwrap();
            let task: TaskId = serde_json::from_value(handoff["task"].take()).unwrap();
            let kept: kinewright_core::HistorySnapshot =
                serde_json::from_value(handoff["kept"].take()).unwrap();
            let roots = SessionRoots {
                read: vec![root.clone()],
                write: vec![root.clone()],
            };
            let host = WriterHost::start_inner(
                HostConfig {
                    scheduler_settings: Some(root.join("scheduler")),
                    project: "C4b reopened".into(),
                    path: Some(root.join("project.kinewright")),
                    roots: roots.clone(),
                    origin: HostOrigin::Headless,
                },
                HostAuth::generate().unwrap(),
                None,
                crate::writer_host::studio_runtime::RuntimePorts {
                    checks: Arc::new(Te15Observe(std::sync::Mutex::new(None))),
                    fixture_claim: Some(root.join("fixture-machine")),
                    ceiling: None,
                    ..Default::default()
                },
            )
            .unwrap();
            wait_runtime_recovery(&host);
            host.runtime.stop();
            let shared = host.owner.upgrade().unwrap();
            let producer = shared.lock().unwrap().restore_native_run(&run).unwrap().0;
            let director = HostClient::connect(host.endpoint.clone(), producer, roots).unwrap();
            for client in [&host.client(), &director] {
                let page = ok(
                    client
                        .request(HostCommand::ObserveRun {
                            run: run.clone(),
                            after_sequence: 0,
                        })
                        .unwrap(),
                    "C4B_RECOVERY_TYPED_VIEW",
                )
                .run_view
                .unwrap();
                assert_rows(&page, &task, !checkpoint);
            }
            stop_with_absence_proof(&host, &run, &root);
            let (source, manual) = {
                let owner = shared.lock().unwrap();
                let work = owner
                    .owner()
                    .unwrap()
                    .retained_task_work()
                    .unwrap()
                    .into_iter()
                    .find(|work| work.task == task)
                    .unwrap();
                assert_eq!(
                    work.missing_approved_edits.len(),
                    if checkpoint { 0 } else { 2 }
                );
                if !checkpoint {
                    assert!(
                        work.disclosure.contains("clip 1") && work.disclosure.contains("clip 2")
                    );
                    assert!(
                        work.disclosure.contains("not in the kept version")
                            && work.disclosure.contains("continue the task")
                    );
                }
                assert!(work.available && work.cleanup_settled);
                (
                    work.source.branch,
                    HostSnapshot::capture(owner.core()).unwrap(),
                )
            };
            let editor = host.client();
            let client_id = "d".repeat(64);
            ok(
                editor
                    .request(HostCommand::AcquireManual {
                        client: client_id.clone(),
                    })
                    .unwrap(),
                "C4B_REVIEW_LEASE",
            );
            let review = editor.with_manual_editor(client_id.clone());
            let prepared = ok(
                review
                    .request(HostCommand::PreviewRecovery {
                        target:
                            kinewright_project::bundle_recovery::RecoverySelection::WorkingBranch(
                                source,
                            )
                            .into(),
                        snapshot: manual,
                    })
                    .unwrap(),
                "C4B_REVIEW_SELECTION",
            );
            let selected = ok(
                review
                    .request(HostCommand::Switch {
                        envelope: Box::new(prepared.switch_envelope.unwrap()),
                    })
                    .unwrap(),
                "C4B_OPEN_KEPT_VERSION",
            );
            ok(
                review
                    .request(HostCommand::ConfirmSwitch {
                        client: client_id,
                        installed: Box::new(
                            selected.switch_attachment.unwrap().identity().unwrap(),
                        ),
                    })
                    .unwrap(),
                "C4B_REVIEW_INSTALLED",
            );
            assert_eq!(
                shared.lock().unwrap().core().export_history().unwrap(),
                kept,
                "C4B_EXACT_LAST_CHECKPOINT"
            );
            let replay = Core::spawn(Arc::clone(&kept.baseline)).unwrap();
            replay.restore_history(kept).unwrap();
            assert_eq!(
                replay.export_history().unwrap().document.tracks[0]
                    .clips
                    .len(),
                if checkpoint { 1 } else { 3 }
            );
            drop(replay);
            drop(director);
            drop(shared);
            drop(host);
            assert!(root.starts_with(kinewright_media::test_support::workspace_target_dir()));
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
