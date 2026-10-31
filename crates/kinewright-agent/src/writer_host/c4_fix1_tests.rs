#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one MCP sequence covers exact task/manual history across turn, attempt, restart, merge and Stop"
)]
fn astra_c4_merge_retry_after_reopen() {
    use kinewright_core::Command;
    let mut f = Fixture::new();
    let mut scoped = Document::default();
    for id in [1, 2] {
        if let kinewright_core::Operation::AddMarker { marker } = history_marker(id) {
            scoped.markers.push(marker);
        }
    }
    f.brief.scope =
        crate::orchestration::policy::director_scope(&scoped, f.brief.scope.base.clone()).unwrap();
    let settings = configure_director(&f);
    let _choice = std::fs::read(settings.join("director.json")).unwrap();
    let _manual = {
        let host = f.host.owner.upgrade().unwrap();
        let mut owner = host.lock().unwrap();
        for id in [50, 51] {
            owner
                .core()
                .request(Command::Do(history_marker(id)))
                .unwrap();
        }
        owner.core().request(Command::Undo).unwrap();
        owner.owner_mut().unwrap().save_active().unwrap();
        owner.core().export_history().unwrap()
    };
    let (run, parent, director) = c4_director(&f);
    let claim_root = f.temp.path("c4-machine");
    let claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &claim_root,
        parent.project.as_str(),
        run.0.as_str(),
    )
    .unwrap();
    let mut child = f.brief.clone();
    child.role = Role::Editor;
    director.accepted(
        "assign_task",
        OrchestrationCommand::AssignTask(Box::new(AssignTask {
            run: run.clone(),
            parent: parent.task.clone(),
            brief: child,
        })),
    );
    let child = f.projection(&run).tasks[1].id.clone();
    let (first, client, endpoint, core) = c4_reserve_task(&f, &run, &child);
    history_turn(&f, &first, &client, &endpoint, 1, true);
    let specialist = C4Mcp::new(&f, &run, &child);
    let expected = c4_mcp_edit_redo(&specialist, &core);
    {
        let host = f.host.owner.upgrade().unwrap();
        let mut owner = host.lock().unwrap();
        let producer = owner.run_producer(&run).unwrap();
        owner
            .checkpoint_task_history(&producer, &run, &child)
            .unwrap();
    }
    history_turn(&f, &first, &client, &endpoint, 1, false);
    drop(specialist);
    drop(director);
    drop(core);
    let (claim, _) =
        crate::effect_pump::recover_run(&f.host.owner.upgrade().unwrap(), claim, &run).unwrap();
    let (_, _, _, _second_parent) = c4_reserve_task(&f, &run, &parent.task);
    let director = C4Mcp::new(&f, &run, &parent.task);
    let (second_attempt, second_client, second_endpoint, second) =
        c4_reserve_task(&f, &run, &child);
    assert_eq!(
        second.export_history().unwrap(),
        expected,
        "C4_MCP_ATTEMPT_HISTORY"
    );
    history_turn(
        &f,
        &second_attempt,
        &second_client,
        &second_endpoint,
        1,
        true,
    );
    // Explicit Freeze asks H3 to end this turn and seal its history atomically.
    let frozen = director.accepted(
        "freeze_task",
        OrchestrationCommand::FreezeTask(child.clone()),
    );
    let freeze_command = serde_json::from_value::<OrchestrationCommand>(
        frozen["receipt"]["envelope"]["command"].clone(),
    )
    .unwrap();
    let mut freeze_retry = director.control(freeze_command);
    for field in ["id", "expected_generation"] {
        freeze_retry[field] = frozen["receipt"]["envelope"][field].clone();
    }
    let frozen_again = director.invoke("freeze_task", freeze_retry);
    assert_eq!(
        frozen_again.is_error,
        Some(false),
        "C4_FREEZE_ONCE {frozen_again:?}"
    );
    assert_eq!(
        frozen_again.structured_content.unwrap()["history"],
        frozen["history"],
        "C4_FREEZE_EXACT"
    );
    let result: TaskResult = serde_json::from_value(frozen["result"].clone()).unwrap();
    director.accepted(
        "publish_result",
        OrchestrationCommand::PublishResult(Box::new(PublishResult {
            task: child.clone(),
            result,
        })),
    );
    drop(director);
    drop(second);
    drop(claim);
    let f = c4_restart(f, &claim_root);
    assert!(
        f.projection(&run)
            .attempts
            .iter()
            .all(|a| matches!(a.state, AttemptState::Retired(_))),
        "C4_RECOVERY_ON_REOPEN"
    );
    let (_, _, _, parent_core) = c4_reserve_task(&f, &run, &parent.task);
    let director = C4Mcp::new(&f, &run, &parent.task);
    let restored = director.accepted(
        "freeze_task",
        OrchestrationCommand::FreezeTask(child.clone()),
    );
    let usage_ref: ObjectRef =
        serde_json::from_value(restored["result"]["usage_ref"].clone()).unwrap();
    let record = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .store()
        .unwrap()
        .read_record::<OrchestrationRecord>(&usage_ref)
        .unwrap();
    assert!(
        matches!(record.payload, OrchestrationPayload::Accounting(_)),
        "C4_RESULT_ACCOUNTING_AFTER_ARCHIVE"
    );
    assert_eq!(
        restored["history"], frozen["history"],
        "C4_MCP_RESTART_HISTORY"
    );
    let third_core = f.host.owner.upgrade().unwrap().lock().unwrap().hooks.runs[&run].tasks[&child]
        .core()
        .clone();
    assert_eq!(
        third_core.export_history().unwrap(),
        expected,
        "C4_MCP_RESTART_REDO"
    );
    let state = f.projection(&run);
    let result = state.results.last().unwrap().clone();
    let merge = director.control(OrchestrationCommand::MergeTask(MergeTask {
        task: child.clone(),
        target: f.brief.scope.base.clone(),
        result,
    }));
    if let Some(handoff) = std::env::var_os("KINEWRIGHT_C4_MERGE_HANDOFF") {
        std::fs::write(handoff, serde_json::to_vec(&serde_json::json!({
            "root": f.temp.root(), "run": run, "parent": parent.task, "child": child, "merge": merge
        })).unwrap()).unwrap();
    }
    let merged = director.invoke("merge_task", merge.clone());
    assert_eq!(merged.is_error, Some(false), "C4_MCP_MERGE {merged:?}");

    drop(director);
    drop(third_core);
    drop(parent_core);
    eprintln!("ASTRA_MERGE_ACKNOWLEDGED_BEFORE_REOPEN");
    let f = c4_restart(f, &claim_root);
    eprintln!("ASTRA_MERGE_REOPEN_RECOVERED_BEFORE_NEW_DIRECTOR");
    let (_, _, _, _parent_core) = c4_reserve_task(&f, &run, &parent.task);
    let director = C4Mcp::new(&f, &run, &parent.task);
    let mut retry = merge;
    let fresh = director.status()["control"].clone();
    retry["owner_epoch"] = fresh["owner_epoch"].clone();
    retry["expected_head"] = fresh["expected_head"].clone();
    let repeated = director.invoke("merge_task", retry);
    eprintln!("ASTRA_MERGE_RESTART_REPLY {repeated:?}");
    assert_eq!(
        repeated.is_error,
        Some(false),
        "ASTRA_MERGE_RECEIPT_AFTER_RESTART"
    );
    assert_eq!(merged.structured_content, repeated.structured_content);
}

#[test]
fn astra_c4_reopen_without_private_claim() {
    let f = Fixture::new();
    let (run, execution, director) = c4_director(&f);
    let claim_root = f.temp.path("missing-machine-claim");
    drop(director);
    let f = c4_restart(f, &claim_root);
    let state = f.projection(&run);
    eprintln!(
        "ASTRA_NO_CLAIM attempts={:?} runtime_drive={:?}",
        state.attempts,
        f.host.runtime.drive()
    );
    assert!(
        state
            .attempts
            .iter()
            .all(|a| matches!(a.state, AttemptState::Retired(_))),
        "ASTRA_NO_CLAIM_H1_RECONCILED"
    );
    let _ = execution;
}

#[test]
fn astra_c4_other_project_release_recovers_waiting_host() {
    let f = Fixture::new();
    let start = f.start();
    let run: RunId = start.envelope.identity("run").unwrap();
    let machine = f.temp.path("shared-machine");
    let claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &machine,
        start.envelope.project.as_str(),
        run.0.as_str(),
    )
    .unwrap();
    let other = c4_restart(Fixture::new(), &machine);
    eprintln!(
        "ASTRA_OTHER_OPEN status={:?}",
        other.host.owner.upgrade().unwrap().lock().unwrap().message
    );
    claim.finish().unwrap();
    assert!(!machine.join("run.json").exists());
    c4_wait(
        || {
            other.host.runtime.update();
            other.host.runtime.drive().is_ok()
        },
        "C4_CONTENTION_RETRY",
    );
    let later = other.host.runtime.drive();
    eprintln!("ASTRA_OTHER_AFTER_RELEASE {later:?}");
    assert!(
        later.is_ok(),
        "ASTRA_OTHER_HOST_MUST_RECOVER_AFTER_CAPACITY_RELEASE"
    );
}

#[test]
fn astra_c4_save_as_reopen_original_recovery() {
    let f = Fixture::new();
    let (run, _, director) = c4_director(&f);
    let machine = f.temp.path("copy-machine");
    let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &machine,
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .project_id()
            .unwrap()
            .as_str(),
        run.0.as_str(),
    )
    .unwrap();
    claim
        .record_project_locator(&f.temp.path("project.kinewright"))
        .unwrap();
    let stop = f.control(&f.editor, &run, RunAction::Stop);
    let stop_receipt = ok(
        f.editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(stop.clone()),
            })
            .unwrap(),
        "C4_ORIGINAL_STOP",
    )
    .orchestration
    .unwrap();
    let reply = native_save_as(&f, "copy.kinewright");
    assert!(reply.error.is_none(), "C4_COPY_SAVE {reply:?}");
    drop(director);
    drop(claim);
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
    let copied = WriterHost::start_inner(
        HostConfig {
            scheduler_settings: None,
            project: "copy".into(),
            path: Some(temp.path("copy.kinewright")),
            roots,
            origin: HostOrigin::Headless,
        },
        HostAuth::generate().unwrap(),
        Some(issuer_auth),
        super::super::studio_runtime::RuntimePorts {
            fixture_claim: Some(machine.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    wait_runtime_recovery(&copied);
    let result = copied.runtime.drive();
    eprintln!(
        "ASTRA_COPY_REOPEN_RECOVERY status={:?} drive={result:?}",
        copied.owner.upgrade().unwrap().lock().unwrap().message
    );
    assert!(
        result.is_ok(),
        "ASTRA_COPY_REOPEN_MUST_RECONCILE_ORIGINAL_CLAIM"
    );
    assert!(!machine.join("run.json").exists());
    assert!(
        copied
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .orchestration_runs()
            .unwrap()
            .is_empty(),
        "C4_COPY_PLAIN_RUN_INDEX"
    );
    let original = Ja1EditingSession::open(&temp.path("project.kinewright")).unwrap();
    let recovered = original.orchestration_projection(&run).unwrap().unwrap();
    assert!(
        recovered
            .attempts
            .iter()
            .all(|a| matches!(a.state, AttemptState::Retired(_))),
        "C4_ORIGINAL_PROJECT_RETIRED"
    );
    assert_eq!(
        original.orchestration_replay(&stop.envelope, None).unwrap(),
        Some(stop_receipt),
        "C4_ORIGINAL_STOP_ENVELOPE_KEPT"
    );
    let _ = brief;
}

// Test inputs are host-created launch intents, never provider evidence.
fn fixture_launch_intent(
    f: &Fixture,
    identity: &ExecutionIdentity,
    dispatched: bool,
    past_boot: bool,
) -> ObjectRef {
    let mut intent = super::super::native_invocation::NativeLaunchIntent::new(identity).unwrap();
    intent.dispatched = dispatched;
    if past_boot {
        intent.boot_id = "00000000-0000-0000-0000-000000000000".into();
    }
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner_mut()
        .unwrap()
        .admit_host_grant(&serde_json::to_vec(&intent).unwrap())
        .unwrap()
}

fn replace_fixture_launch(
    f: &Fixture,
    execution: &ExecutionIdentity,
    dispatched: bool,
    past_boot: bool,
) {
    let producer = f.producer_client(&execution.run);
    let mut state = f.projection(&execution.run);
    state
        .attempts
        .iter_mut()
        .find(|a| a.identity == *execution)
        .unwrap()
        .recovery_ref = fixture_launch_intent(f, execution, dispatched, past_boot);
    crate::orchestration::tasks::refresh_cleanup(&mut state);
    publish_projection(f, &producer, &execution.run, state, None);
}
#[test]
fn s4b_c4_fix1_missing_claim_executing_and_same_boot() {
    let identity = crate::job_guardian::protocol::AttemptIdentity {
        project: "a".into(),
        run: "b".into(),
        task: "c".into(),
        attempt: "d".into(),
        nonce: "e".into(),
        owner_instance: "f".into(),
    };
    assert!(
        crate::guardian::RecoveryObserver::reboot_cleanup(
            identity,
            &crate::guardian::boot_id().unwrap().to_uppercase()
        )
        .is_err(),
        "C4_BOOT_ID_CASE_IS_NOT_A_REBOOT"
    );
    for past_boot in [true, false] {
        let f = Fixture::new();
        let (run, execution, director) = c4_director(&f);
        let admitted = native_admitted(&f, &execution);
        let (profile, _, scratch) = native_profile(&f, &admitted);
        let _charged = native_charge(&f, &admitted, profile);
        replace_fixture_launch(&f, &execution, true, past_boot);
        let producer = f.producer_client(&run);
        let mut state = f.projection(&run);
        state
            .attempts
            .iter_mut()
            .find(|a| a.identity == execution)
            .unwrap()
            .state = AttemptState::Executing;
        crate::orchestration::tasks::refresh_cleanup(&mut state);
        publish_projection(&f, &producer, &run, state, None);
        let before_state = f.projection(&run);
        let before = f
            .host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .native_accounting(&before_state)
            .unwrap()
            .unwrap();
        drop(director);
        let machine = f.temp.path("gone-private-state");
        let f = c4_restart(f, &machine);
        let state = f.projection(&run);
        let host = f.host.owner.upgrade().unwrap();
        let owner = host.lock().unwrap();
        let after = owner.native_accounting(&state).unwrap().unwrap();
        assert_eq!(
            before.tasks[0].sends, after.tasks[0].sends,
            "C4_RECOVERY_NEVER_REFUNDS_OR_REPLAYS"
        );
        if past_boot {
            assert!(
                state
                    .attempts
                    .iter()
                    .all(|a| matches!(a.state, AttemptState::Retired(_))),
                "C4_REBOOT_EXECUTING_RETIRED"
            );
            assert!(!owner.native_recovery_pending);
        } else {
            assert_eq!(state.attempts[0].state, AttemptState::Executing);
            assert!(
                owner.native_recovery_pending,
                "C4_SAME_BOOT_NO_PROOF_BLOCKED"
            );
            assert!(
                owner
                    .native_recovery_status
                    .as_deref()
                    .unwrap()
                    .contains("cleanup pending")
            );
            drop(owner);
            let page = f
                .editor
                .request(HostCommand::ObserveRun {
                    run: run.clone(),
                    after_sequence: 0,
                })
                .unwrap()
                .run_view
                .unwrap();
            assert!(
                page.runtime_status.unwrap().contains("cleanup pending"),
                "C4_NOTES_POLL_RECOVERY_STATUS"
            );
            assert!(page.allowed_actions.is_empty());
        }
        std::fs::remove_dir_all(scratch).unwrap();
    }
}
#[test]
fn s4b_c4_fix1_foreign_missing_and_live_original_retry() {
    for live in [false, true] {
        let f = Fixture::new();
        let (run, execution, director) = c4_director(&f);
        let machine = f.temp.path("foreign-machine");
        let original = f.temp.path("project.kinewright");
        let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
            &machine,
            execution.project.as_str(),
            run.0.as_str(),
        )
        .unwrap();
        claim.record_project_locator(&original).unwrap();
        assert!(native_save_as(&f, "copy.kinewright").error.is_none());
        drop(director);
        drop(claim);
        let Fixture {
            host,
            temp,
            issuer_auth,
            ..
        } = f;
        drop(host);
        let original_owner = if live {
            Some(Ja1EditingSession::open(&original).unwrap())
        } else {
            std::fs::rename(&original, temp.path("moved.kinewright")).unwrap();
            None
        };
        let active_path = if live {
            let path = temp.path("unrelated.kinewright");
            let core = Core::spawn(Document::default()).unwrap();
            let mut bundle =
                ProjectBundle::activate(&path, BundleWriteAdmission::new_document()).unwrap();
            let inputs = bundle.empty_inputs().unwrap().reference.clone();
            bundle.save(&core, &inputs).unwrap();
            path
        } else {
            temp.path("copy.kinewright")
        };
        let roots = SessionRoots {
            read: vec![temp.root().into()],
            write: vec![temp.root().into()],
        };
        let copy = WriterHost::start_inner(
            HostConfig {
                scheduler_settings: None,
                project: "copy".into(),
                path: Some(active_path),
                roots,
                origin: HostOrigin::Headless,
            },
            HostAuth::generate().unwrap(),
            Some(issuer_auth),
            super::super::studio_runtime::RuntimePorts {
                fixture_claim: Some(machine.clone()),
                ..Default::default()
            },
        )
        .unwrap();
        wait_runtime_recovery(&copy);
        let status = copy
            .client()
            .request(HostCommand::Status)
            .unwrap()
            .status
            .native_runtime_status
            .unwrap();
        assert!(
            status.contains("cleanup pending for project.kinewright: open it to finish"),
            "C4_FOREIGN_BLOCK_NAMES_ORIGINAL {status}"
        );
        assert!(machine.join("run.json").exists());
        {
            let host = copy.owner.upgrade().unwrap();
            let mut owner = host.lock().unwrap();
            owner
                .core()
                .request(kinewright_core::Command::Do(history_marker(91)))
                .unwrap();
            owner.owner_mut().unwrap().save_active().unwrap();
            assert!(
                owner
                    .owner()
                    .unwrap()
                    .orchestration_runs()
                    .unwrap()
                    .is_empty(),
                "C4_COPY_NEVER_IMPORTS_RUN"
            );
        }
        if !live {
            std::fs::rename(temp.path("moved.kinewright"), &original).unwrap();
        }
        drop(original_owner);
        c4_wait(
            || {
                copy.runtime.update();
                copy.client()
                    .request(HostCommand::Status)
                    .unwrap()
                    .status
                    .native_runtime_status
                    .is_none()
            },
            "C4_FOREIGN_RETRY_FINISH",
        );
        assert!(!machine.join("run.json").exists());
        let original = Ja1EditingSession::open(&original).unwrap();
        let recovered = original.orchestration_projection(&run).unwrap().unwrap();
        assert!(
            recovered
                .attempts
                .iter()
                .all(|a| matches!(a.state, AttemptState::Retired(_))),
            "C4_FOREIGN_H1_STAYS_IN_ORIGINAL"
        );
        assert_ne!(
            copy.owner
                .upgrade()
                .unwrap()
                .lock()
                .unwrap()
                .spending_project()
                .unwrap(),
            execution.project
        );
    }
}
#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c4_fix1_child_discovery_precedes_recovery_proof() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let (run, execution, director) = c4_director(&f);
    let machine = f.temp.path("child-machine");
    let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &machine,
        execution.project.as_str(),
        run.0.as_str(),
    )
    .unwrap();
    claim
        .record_project_locator(&f.temp.path("project.kinewright"))
        .unwrap();
    claim
        .reserve(&crate::native_launcher::native_identity(&execution))
        .unwrap();
    // A partial private-state loss still needs the retained attempt's proof.
    std::fs::remove_file(machine.join("run.json")).unwrap();
    let barrier = f.temp.path("proof-barrier");
    std::fs::create_dir(&barrier).unwrap();
    let wrapper = f.temp.path("child-host");
    let quote =
        |path: &std::path::Path| format!("'{}'", path.to_str().unwrap().replace('\'', "'\"'\"'"));
    std::fs::write(&wrapper, format!("#!/bin/sh\nexport KINEWRIGHT_C4_RECOVERY_FIXTURE_ROOT={}\nexport KINEWRIGHT_C4_RECOVERY_FIXTURE_BARRIER={}\nexec {} \"$@\"\n", quote(&machine), quote(&barrier), quote(&kinewright_media::test_support::workspace_target_dir().join("debug/kinewright")))).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    let blocked_request = f.start_request(&f.editor);
    drop(director);
    drop(claim);
    let Fixture { host, temp, .. } = f;
    drop(host);
    let release = barrier.clone();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(6500));
        std::fs::write(release.join("release"), b"").unwrap();
    });
    let started = Instant::now();
    let launched = HostClient::launch_with_issuer(
        &wrapper,
        HostConfig {
            scheduler_settings: None,
            project: "child".into(),
            path: Some(temp.path("project.kinewright")),
            roots: SessionRoots {
                read: vec![temp.root().into()],
                write: vec![temp.root().into()],
            },
            origin: HostOrigin::Headless,
        },
    );
    let discovered = started.elapsed();
    let launch_ok = launched.is_ok();
    if let Ok((client, child, _issuer)) = launched {
        let mut child = C4FixtureChild(child);
        let pending = client
            .request(HostCommand::ObserveRun {
                run: run.clone(),
                after_sequence: 0,
            })
            .unwrap();
        assert!(
            pending.run_view.unwrap().runtime_status.is_some(),
            "C4_DISCOVERY_BEFORE_PROOF"
        );
        let blocked = client
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(blocked_request),
            })
            .unwrap();
        assert_eq!(
            blocked.error.unwrap().code,
            "recovery_frozen",
            "C4_NO_ORCHESTRATION_BEFORE_RECOVERY"
        );
        c4_wait(
            || barrier.join("entered").exists(),
            "C4_CHILD_PROOF_ENTERED",
        );
        releaser.join().unwrap();
        c4_wait(
            || {
                client
                    .request(HostCommand::ObserveRun {
                        run: run.clone(),
                        after_sequence: 0,
                    })
                    .unwrap()
                    .run_view
                    .is_some_and(|page| {
                        page.projection
                            .attempts
                            .iter()
                            .all(|a| matches!(a.state, AttemptState::Retired(_)))
                    })
            },
            "C4_CHILD_PROOF_RETIREMENT",
        );
        c4_wait(
            || !machine.join("run.json").exists(),
            "C4_CHILD_CLAIM_SETTLED",
        );
        let _ = client.request(HostCommand::Shutdown);
        if child.0.try_wait().unwrap().is_none() {
            child.0.kill().unwrap();
        }
        let _ = child.0.wait().unwrap();
    } else {
        releaser.join().unwrap();
    }
    assert!(
        launch_ok && discovered < Duration::from_secs(5),
        "C4_CHILD_DISCOVERY_SURVIVES_SLOW_RECOVERY {discovered:?}"
    );
}

fn crash_host(root: &std::path::Path) -> WriterHost {
    let host = WriterHost::start_inner(
        HostConfig {
            scheduler_settings: None,
            project: "crash".into(),
            path: Some(root.join("project.kinewright")),
            roots: SessionRoots {
                read: vec![root.into()],
                write: vec![root.into()],
            },
            origin: HostOrigin::Headless,
        },
        HostAuth::generate().unwrap(),
        Some(HostAuth::generate().unwrap()),
        super::super::studio_runtime::RuntimePorts {
            fixture_claim: Some(root.join("c4-machine")),
            ..Default::default()
        },
    )
    .unwrap();
    wait_runtime_recovery(&host);
    host
}
fn crash_director(host: &WriterHost, run: &RunId, task: &TaskId) -> C4Mcp {
    let host_owner = host.owner.upgrade().unwrap();
    let mut owner = host_owner.lock().unwrap();
    let (producer, mut state) = owner.restore_native_run(run).unwrap();
    owner.archive_retired_attempts(&mut state).unwrap();
    let execution = ExecutionIdentity {
        version: 1,
        project: owner.spending_project().unwrap(),
        run: run.clone(),
        task: task.clone(),
        attempt: identity(),
        nonce: identity(),
        owner_instance: identity(),
    };
    let intent = super::super::native_invocation::NativeLaunchIntent::new(&execution).unwrap();
    let recovery_ref = owner
        .owner_mut()
        .unwrap()
        .admit_host_grant(&serde_json::to_vec(&intent).unwrap())
        .unwrap();
    state.attempts.push(AttemptReservation {
        identity: execution.clone(),
        owner_epoch: owner.owner().unwrap().store().unwrap().owner_epoch(),
        state: AttemptState::Reserved,
        recovery_ref,
    });
    let task = state.tasks.iter_mut().find(|t| &t.id == task).unwrap();
    task.state = TaskState::Preparing;
    task.progress.capacity = DependencyReservation::Backed(execution.attempt.clone());
    crate::orchestration::tasks::refresh_cleanup(&mut state);
    owner
        .publish_native(&producer, state, &execution, vec![])
        .unwrap();
    owner
        .prepare_task_endpoint(
            &producer,
            run,
            &execution.task,
            &execution.attempt,
            Duration::from_secs(300),
        )
        .unwrap_or_else(|e| panic!("C4_CRASH_CONTINUE {e}"));
    drop(owner);
    C4Mcp::for_host(host, run, &execution.task)
}
#[test]
#[ignore = "on-demand: durable merge crash cuts"]
fn s4b_c4_fix1_merge_crash_cuts() {
    for cut in [
        "before_candidate_commit",
        "after_candidate_commit",
        "after_history_checkpoint",
    ] {
        let temp = kinewright_media::test_support::TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "c4-crash-parent",
        );
        let handoff = temp.path("handoff.json");
        let log = temp.path("child.log");
        let output = std::fs::File::create(&log).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "writer_host::hooks_tests::tests::s4b_c4_fix1_merge_crash_child",
                "--nocapture",
            ])
            .env("KINEWRIGHT_C4_MERGE_CUT", cut)
            .env("KINEWRIGHT_C4_MERGE_HANDOFF", &handoff)
            .stdout(output.try_clone().unwrap())
            .stderr(output)
            .status()
            .unwrap();
        assert_eq!(
            status.code(),
            Some(86),
            "C4_MERGE_CRASH_REACHED {cut}: {}",
            std::fs::read_to_string(&log).unwrap()
        );
        let data: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&handoff).unwrap()).unwrap();
        let root = std::path::PathBuf::from(data["root"].as_str().unwrap());
        let run: RunId = serde_json::from_value(data["run"].clone()).unwrap();
        let parent: TaskId = serde_json::from_value(data["parent"].clone()).unwrap();
        let child: TaskId = serde_json::from_value(data["child"].clone()).unwrap();
        let host = crash_host(&root);
        let director = crash_director(&host, &run, &parent);
        let mut retry = data["merge"].clone();
        let fresh = director.status()["control"].clone();
        for field in ["owner_epoch", "expected_head"] {
            retry[field] = fresh[field].clone();
        }
        let merged = director.invoke("merge_task", retry.clone());
        assert_eq!(
            merged.is_error,
            Some(false),
            "C4_MERGE_CRASH_RETRY {cut}: {merged:?}"
        );
        let history = {
            let owner = host.owner.upgrade().unwrap();
            let owner = owner.lock().unwrap();
            let state = owner
                .owner()
                .unwrap()
                .orchestration_projection(&run)
                .unwrap()
                .unwrap();
            let saved = owner.saved_task_history(&state, &child).unwrap().unwrap();
            let envelope = saved.merge_envelope.unwrap();
            let kinewright_project::receipts::ReceiptLookup::Committed(receipt) = owner
                .owner()
                .unwrap()
                .manual_transition_receipt(&envelope)
                .unwrap()
            else {
                panic!("C4_MERGE_COMMITTED_RECEIPT");
            };
            assert_eq!(
                serde_json::to_value(receipt).unwrap(),
                merged.structured_content.as_ref().unwrap()["receipt"],
                "C4_MERGE_ORIGINAL_ENVELOPE_RECEIPT"
            );
            let guard = owner
                .owner()
                .unwrap()
                .durable_sessions()
                .into_iter()
                .find(|(_, g)| g.branch_id() == &saved.base.branch)
                .unwrap()
                .1;
            guard.core().export_history().unwrap()
        };
        let again = director.invoke("merge_task", retry);
        assert_eq!(again.is_error, Some(false), "C4_MERGE_CRASH_REPLAY {cut}");
        assert_eq!(
            again.structured_content, merged.structured_content,
            "C4_MERGE_EXACTLY_ONCE {cut}"
        );
        drop(director);
        drop(host);
        let reopened = crash_host(&root);
        let director = crash_director(&reopened, &run, &parent);
        let owner = reopened.owner.upgrade().unwrap();
        let current = owner
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .durable_sessions()
            .into_iter()
            .find(|(_, g)| g.core().export_history().unwrap() == history);
        assert!(
            current.is_some(),
            "C4_MERGE_CANDIDATE_EXACT_AFTER_CRASH {cut}"
        );
        drop(owner);
        drop(director);
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
        eprintln!("C4_MERGE_CRASH_CUT {cut} reopened=true continued=true exact_receipt=true");
    }
}

#[test]
fn s4b_c4_fix1_merge_crash_child() {
    let Some(handoff) = std::env::var_os("KINEWRIGHT_C4_MERGE_HANDOFF") else {
        return;
    };
    let mut f = Fixture::new();
    let mut scoped = Document::default();
    for id in [1, 2] {
        if let kinewright_core::Operation::AddMarker { marker } = history_marker(id) {
            scoped.markers.push(marker);
        }
    }
    f.brief.scope =
        crate::orchestration::policy::director_scope(&scoped, f.brief.scope.base.clone()).unwrap();
    let (run, parent, director) = c4_director(&f);
    {
        let host = f.host.owner.upgrade().unwrap();
        let mut owner = host.lock().unwrap();
        let producer = owner.run_producer(&run).unwrap();
        owner
            .checkpoint_task_history(&producer, &run, &parent.task)
            .unwrap();
    }
    let mut child = f.brief.clone();
    child.role = Role::Editor;
    director.accepted(
        "assign_task",
        OrchestrationCommand::AssignTask(Box::new(AssignTask {
            run: run.clone(),
            parent: parent.task.clone(),
            brief: child,
        })),
    );
    let child = f.projection(&run).tasks[1].id.clone();
    let (execution, client, endpoint, core) = c4_reserve_task(&f, &run, &child);
    history_turn(&f, &execution, &client, &endpoint, 1, true);
    let specialist = C4Mcp::new(&f, &run, &child);
    c4_mcp_edit_redo(&specialist, &core);
    let frozen = director.accepted(
        "freeze_task",
        OrchestrationCommand::FreezeTask(child.clone()),
    );
    let result: TaskResult = serde_json::from_value(frozen["result"].clone()).unwrap();
    director.accepted(
        "publish_result",
        OrchestrationCommand::PublishResult(Box::new(PublishResult {
            task: child.clone(),
            result,
        })),
    );
    let merge = director.control(OrchestrationCommand::MergeTask(MergeTask {
        task: child.clone(),
        target: f.brief.scope.base.clone(),
        result: f.projection(&run).results.last().unwrap().clone(),
    }));
    std::fs::write(handoff,serde_json::to_vec(&serde_json::json!({"root":f.temp.root(),"run":run,"parent":parent.task,"child":child,"merge":merge})).unwrap()).unwrap();
    let merged = director.invoke("merge_task", merge);
    panic!("C4_MERGE_CRASH_NOT_REACHED {merged:?}");
}

struct C4RefusingProfiles;
impl crate::effect_pump::ProfileFactory for C4RefusingProfiles {
    fn bind(
        &self,
        _: &crate::orchestration::AdmittedAttempt,
    ) -> std::io::Result<crate::orchestration::adapters::CertifiedConnectorProfile> {
        Err(std::io::Error::other("C4 permanent bind refusal"))
    }
    fn bind_for_task(
        &self,
        attempt: &crate::orchestration::AdmittedAttempt,
        _: &str,
    ) -> std::io::Result<crate::orchestration::adapters::CertifiedConnectorProfile> {
        self.bind(attempt)
    }
}
#[test]
fn s4b_c4_fix1_worker_refusal_does_not_churn_on_poll() {
    let f = Fixture::new();
    let start = f.start();
    let run: RunId = start.envelope.identity("run").unwrap();
    let machine = f.temp.path("refusing-runtime");
    let Fixture {
        host,
        temp,
        issuer_auth,
        brief,
        ..
    } = f;
    drop(host);
    let host = WriterHost::start_inner(
        HostConfig {
            scheduler_settings: None,
            project: "refusal".into(),
            path: Some(temp.path("project.kinewright")),
            roots: SessionRoots {
                read: vec![temp.root().into()],
                write: vec![temp.root().into()],
            },
            origin: HostOrigin::Headless,
        },
        HostAuth::generate().unwrap(),
        Some(issuer_auth.clone()),
        super::super::studio_runtime::RuntimePorts {
            profiles: Some(Arc::new(C4RefusingProfiles)),
            fixture_claim: Some(machine.clone()),
            fixture_autostart: true,
            ..Default::default()
        },
    )
    .unwrap();
    let roots = host.roots.clone();
    let f = Fixture {
        editor: host.client(),
        issuer: HostIssuer::new(host.endpoint.clone(), issuer_auth.clone(), roots).unwrap(),
        host,
        issuer_auth,
        brief,
        temp,
    };
    let host = &f.host;
    let client = f.editor.clone();
    c4_wait(
        || {
            client
                .request(HostCommand::ObserveRun {
                    run: run.clone(),
                    after_sequence: 0,
                })
                .unwrap()
                .run_view
                .is_some_and(|page| {
                    page.runtime_status
                        .is_some_and(|status| status.contains("native runtime refusal"))
                })
        },
        "C4_WORKER_REFUSAL_RECORDED",
    );
    let before = client
        .request(HostCommand::ObserveRun {
            run: run.clone(),
            after_sequence: 0,
        })
        .unwrap()
        .run_view
        .unwrap()
        .projection;
    assert_eq!(before.attempts.len(), 1);
    assert!(
        before
            .attempts
            .iter()
            .all(|a| matches!(a.state, AttemptState::Retired(_)))
    );
    for _ in 0..16 {
        let page = client
            .request(HostCommand::ObserveRun {
                run: run.clone(),
                after_sequence: 0,
            })
            .unwrap()
            .run_view
            .unwrap();
        assert_eq!(
            page.projection.attempts, before.attempts,
            "C4_WORKER_REFUSAL_LATCHED"
        );
        assert_eq!(
            page.projection.sequence, before.sequence,
            "C4_REFUSAL_NO_POLL_WRITES"
        );
        assert!(
            page.allowed_actions.contains(&RunActionKind::Stop),
            "C4_REFUSAL_STILL_ALLOWS_STOP"
        );
    }
    let host_owner = host.owner.upgrade().unwrap();
    let mut owner = host_owner.lock().unwrap();
    let accounting = owner.native_accounting(&before).unwrap().unwrap();
    assert!(accounting.tasks[0].sends.is_empty());
    owner.revoke_native_owner(None);
    drop(owner);
    let stop = f.control(&f.editor, &run, RunAction::Stop);
    ok(
        f.editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(stop),
            })
            .unwrap(),
        "C4_LATCHED_RUN_STOPS",
    );
    c4_wait(
        || {
            let _ = f.editor.request(HostCommand::Status);
            !machine.join("run.json").exists()
        },
        "C4_LATCHED_RUN_RELEASES_CLAIM",
    );
    let next: RunId = f.start().envelope.identity("run").unwrap();
    assert_ne!(next, run);
    c4_wait(
        || {
            f.projection(&next)
                .attempts
                .iter()
                .any(|attempt| matches!(attempt.state, AttemptState::Retired(_)))
        },
        "C4_NEW_RUN_IS_NOT_OLD_REFUSAL",
    );
    assert_eq!(f.projection(&next).attempts.len(), 1);
}

struct C4FixtureChild(std::process::Child);
impl Drop for C4FixtureChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn s4b_c4_fix1_reboot_cleanup_keeps_archived_completion() {
    let f = Fixture::new();
    let (run, execution, actor) = reserve(&f);
    replace_fixture_launch(&f, &execution, true, true);
    let mut state = f.projection(&run);
    state.attempts[0].state = AttemptState::Retired(AttemptOutcome::Completed);
    state.tasks[0].state = TaskState::Queued;
    state.tasks[0].progress.capacity = DependencyReservation::Free;
    crate::orchestration::tasks::refresh_cleanup(&mut state);
    let owner = f.host.owner.upgrade().unwrap();
    owner
        .lock()
        .unwrap()
        .archive_retired_attempts(&mut state)
        .unwrap();
    publish_projection(&f, &actor, &run, state, None);
    let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &f.temp.path("retired-machine"),
        execution.project.as_str(),
        run.0.as_str(),
    )
    .unwrap();
    claim
        .reserve(&crate::native_launcher::native_identity(&execution))
        .unwrap();
    let (claim, _) = crate::effect_pump::recover_run(&owner, claim, &run).unwrap();
    claim.finish().unwrap();
    let state = f.projection(&run);
    assert!(state.attempts.is_empty());
    let recovered = owner
        .lock()
        .unwrap()
        .recovery_attempt(&state, &crate::native_launcher::native_identity(&execution))
        .unwrap()
        .unwrap();
    assert_eq!(
        recovered.state,
        AttemptState::Retired(AttemptOutcome::Completed),
        "C4_REBOOT_NEVER_REWRITES_RETIRED_COMPLETION"
    );
}

#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c4_fix1_failed_launch_publishes_retirement_before_private_delete() {
    use crate::orchestration::adapters::GuardianLauncher;
    for mode in ["unbound", "pipes", "reader", "ack_failure"] {
        let f = Fixture::new();
        let (run, execution, _) = reserve(&f);
        let admitted = native_admitted(&f, &execution);
        let (profile, checks, scratch) = native_profile(&f, &admitted);
        let invocation = native_charge(&f, &admitted, profile);
        let before = f.projection(&run);
        let machine = f.temp.path("abort-machine");
        let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
            &machine,
            execution.project.as_str(),
            run.0.as_str(),
        )
        .unwrap();
        claim
            .record_project_locator(&f.temp.path("project.kinewright"))
            .unwrap();
        let claim = Arc::new(Mutex::new(claim));
        let owner = f.host.owner.upgrade().unwrap();
        let launcher = crate::native_launcher::NativeGuardianLauncher {
            claim: claim.clone(),
            owner: Arc::new(super::super::native_invocation::HostInvocationOwner::new(
                &owner,
                f.producer(&run),
            )),
            guardian: if matches!(mode, "unbound" | "ack_failure") {
                f.temp.path("missing-guardian")
            } else {
                kinewright_media::test_support::workspace_target_dir()
                    .join("debug/kinewright-job-guardian")
            },
            checks: Arc::new(checks),
            lose_commit_ack: false,
            after_release: None,
            startup_failure: match mode {
                "pipes" => Some(crate::native_launcher::StartupFailure::Pipes),
                "reader" => Some(crate::native_launcher::StartupFailure::Reader),
                _ => None,
            },
        };
        let result = if mode == "ack_failure" {
            // The dispatch cut is first; fail the subsequent H1 retirement cut.
            kinewright_project::io_observer::fail_nth("root_before_publish", 2, || {
                launcher.launch(admitted, invocation)
            })
        } else {
            launcher.launch(admitted, invocation)
        };
        assert!(result.is_err(), "C4_ABORT_FIXTURE_REACHED {mode}");
        eprintln!("C4_ABORT_RESULT {mode}: {:?}", result.as_ref().err());
        drop(result);
        drop(launcher);
        drop(owner);
        if mode == "ack_failure" {
            assert_eq!(
                claim.lock().unwrap().retained_attempts(),
                1,
                "C4_UNACKNOWLEDGED_ABORT_KEEPS_PRIVATE_PROOF"
            );
            assert!(
                machine
                    .join(format!("attempt-{}.json", execution.nonce.as_str()))
                    .exists()
            );
            drop(claim);
            let f = c4_restart(f, &machine);
            assert!(
                f.projection(&run)
                    .attempts
                    .iter()
                    .all(|a| matches!(a.state, AttemptState::Retired(_))),
                "C4_ABORT_REOPEN_SAME_EXECUTOR"
            );
            assert!(!machine.join("run.json").exists());
        } else {
            assert_eq!(
                claim.lock().unwrap().retained_attempts(),
                0,
                "C4_ABORT_PRIVATE_SETTLED {mode}"
            );
            let state = f.projection(&run);
            assert!(
                state
                    .attempts
                    .iter()
                    .all(|a| matches!(a.state, AttemptState::Retired(_))),
                "C4_ABORT_H1_BEFORE_PRIVATE_DELETE {mode}"
            );
            let owner = f.host.owner.upgrade().unwrap();
            let owner = owner.lock().unwrap();
            assert_eq!(
                owner.native_accounting(&state).unwrap().unwrap().tasks[0].sends,
                owner.native_accounting(&before).unwrap().unwrap().tasks[0].sends,
                "C4_ABORT_NEVER_REFUNDS_OR_REPLAYS"
            );
            drop(owner);
            Arc::try_unwrap(claim)
                .ok()
                .unwrap()
                .into_inner()
                .unwrap()
                .finish()
                .unwrap();
        }
        std::fs::remove_dir_all(scratch).unwrap();
    }
}
