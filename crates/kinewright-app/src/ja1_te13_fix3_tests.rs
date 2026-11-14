// Real Studio controls over persisted task histories in both writer modes.
#[cfg(target_os = "linux")]
fn te13_fix3_stop_studio(app: &mut Ja1Attachment) {
    use kinewright_agent::{
        orchestration::orchestration_records::*,
        writer_host::{HostCommand, ScopedOrchestrationRequest},
    };
    if let Some(host) = &app.host {
        let run = host.status.retained_task_work[0].run.clone();
        let page = host
            .client
            .request(HostCommand::ObserveRun {
                run: run.clone(),
                after_sequence: 0,
            })
            .unwrap()
            .run_view
            .unwrap();
        let issued = host
            .client
            .request(HostCommand::IssueMetadataIssuer { issuer: None })
            .unwrap()
            .metadata_issuer
            .unwrap();
        let mut envelope = OrchestrationEnvelope {
            id: issued.envelope.id.clone(),
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
                command_id: issued.envelope.id,
                run_id: run.clone(),
                expected_control_revision: page.projection.control_revision,
                action: RunAction::Stop,
            }),
        };
        envelope.digest = envelope.computed_digest().unwrap();
        let reply = host
            .client
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(ScopedOrchestrationRequest {
                    run: Some(run),
                    envelope,
                    consume_batch: None,
                }),
            })
            .unwrap();
        assert!(reply.error.is_none(), "TE13_FIX3_STUDIO_STOP {reply:?}");
        app.apply(Action::Retry).unwrap();
    }
}
#[cfg(target_os = "linux")]
fn te13_fix3_retained_studio(hosted: bool, scenario: &str) {
    let (_temp, mut app, branch, expected) = te13_fix3_retained_app(hosted, false, scenario);
    let label = format!("Save current edit, then open {}", branch.as_str());
    let text = rendered(&app);
    assert!(
        text.contains("work is kept on working branch") && text.contains(&label),
        "TE13_FIX3_UNMERGED_STUDIO_OFFER {scenario}/{hosted}: {text}"
    );
    if let Some(host) = &app.host {
        let work = host
            .status
            .retained_task_work
            .iter()
            .find(|work| work.source.branch == branch)
            .unwrap();
        let reply = host
            .client
            .request(kinewright_agent::writer_host::HostCommand::ObserveRun {
                run: work.run.clone(),
                after_sequence: 0,
            })
            .unwrap();
        assert!(
            reply
                .run_view
                .unwrap()
                .runtime_status
                .is_some_and(|s| s.contains(work.task.0.as_str()) && s.contains(&label)),
            "TE13_FIX3_UNMERGED_RUN_MESSAGE"
        );
    }
    let action = click_control(
        &app,
        &format!("Remove this working branch {}", branch.as_str()),
    );
    app.apply(action).unwrap();
    assert!(
        rendered(&app).contains("This branch contains retained task"),
        "TE13_FIX3_UNMERGED_REMOVAL_WARNING {}",
        rendered(&app)
    );
    app.apply(Action::Cancel).unwrap();
    let action = click_control(&app, &label);
    app.apply(action).unwrap();
    let action = click_control(&app, "Save current work, then open recovery point");
    app.apply(action).unwrap();
    assert_eq!(
        app.owner.core().export_history().unwrap(),
        expected,
        "TE13_FIX3_UNMERGED_STUDIO_EXACT"
    );
    assert!(
        !rendered(&app).contains(&label),
        "TE13_FIX3_NO_DUPLICATE_OPEN"
    );
    assert!(app.close());
}
#[cfg(target_os = "linux")]
#[test]
fn te13_fix3_standalone_root() {
    te13_fix3_retained_studio(false, "root");
}
#[cfg(target_os = "linux")]
#[test]
fn te13_fix3_host_root() {
    te13_fix3_retained_studio(true, "root");
}
#[cfg(target_os = "linux")]
#[test]
fn te13_fix3_standalone_observed() {
    te13_fix3_retained_studio(false, "observed");
}
#[cfg(target_os = "linux")]
#[test]
fn te13_fix3_host_observed() {
    te13_fix3_retained_studio(true, "observed");
}
#[cfg(target_os = "linux")]
#[test]
fn te13_fix3_standalone_completed_parent() {
    te13_fix3_retained_studio(false, "completed-parent");
}
#[cfg(target_os = "linux")]
#[test]
fn te13_fix3_host_completed_parent() {
    te13_fix3_retained_studio(true, "completed-parent");
}

#[cfg(target_os = "linux")]
fn te13_fix3_fork_removed_studio(hosted: bool) {
    let (_temp, mut app, branch, expected) = te13_fix1_retained_app(hosted, true);
    let path = app.project_path().unwrap();
    let original_manual = LoadedBundle::read(&path)
        .unwrap()
        .manual_checkpoint()
        .unwrap()
        .branch_id
        .clone();
    let label = format!("Save current edit, then open {}", branch.as_str());
    let action = click_control(&app, &label);
    app.apply(action).unwrap();
    let action = click_control(&app, "Save current work, then open recovery point");
    app.apply(action).unwrap();
    app.send_manual(Command::Do(Operation::SetTrackSyncLock {
        track: TrackId(1),
        locked: false,
    }))
    .unwrap();
    app.settle_manual_for_test();
    app.apply(Action::Save).unwrap();
    let fork = LoadedBundle::read(&path)
        .unwrap()
        .manual_checkpoint()
        .unwrap()
        .branch_id
        .clone();
    assert_ne!(fork, branch);
    app.apply(Action::PreviewRecovery(RecoverySelection::WorkingBranch(
        original_manual,
    )))
    .unwrap();
    let action = click_control(&app, "Save current work, then open recovery point");
    app.apply(action).unwrap();
    let action = click_control(
        &app,
        &format!("Remove this working branch {}", fork.as_str()),
    );
    app.apply(action).unwrap();
    let action = click_control(&app, "Confirm working branch loss");
    app.apply(action).unwrap();
    let text = rendered(&app);
    assert!(
        text.contains(&label) && !text.contains("work was opened for review"),
        "TE13_FIX3_REMOVED_FORK_REOFFERS {hosted}: {text}"
    );
    let root = LoadedBundle::read(&path).unwrap();
    assert!(
        !root
            .root()
            .unwrap()
            .retained_task_reviews
            .contains_key(&fork),
        "TE13_FIX3_FORK_LINK_PRUNED"
    );
    drop(root);
    let action = click_control(&app, &label);
    app.apply(action).unwrap();
    let action = click_control(&app, "Save current work, then open recovery point");
    app.apply(action).unwrap();
    assert_eq!(
        app.owner.core().export_history().unwrap(),
        expected,
        "TE13_FIX3_REMOVED_FORK_ORIGINAL_EXACT"
    );
    assert!(app.close());
}
#[cfg(target_os = "linux")]
#[test]
fn te13_fix3_standalone_fork_removed() {
    te13_fix3_fork_removed_studio(false);
}
#[cfg(target_os = "linux")]
#[test]
fn te13_fix3_host_fork_removed() {
    te13_fix3_fork_removed_studio(true);
}

#[cfg(target_os = "linux")]
fn te13_fix3_shared_bad_studio(hosted: bool, unbound: bool) {
    let (_temp, mut app, branch, _) = te13_fix3_retained_app(hosted, true, "shared-bad");
    if unbound {
        if let Some(host) = &mut app.host {
            host.status.retained_task_failures[0].branch = None;
        } else {
            app.retained_task_failures[0].branch = None;
        }
    }
    let text = rendered(&app);
    assert!(
        text.contains("could not be inspected") && text.contains("work is kept on working branch")
    );
    assert!(
        !text
            .lines()
            .any(|line| line == format!("Save current edit, then open {}", branch.as_str())),
        "TE13_FIX3_BAD_BRANCH_NO_BUTTON {hosted}/{unbound}: {text}"
    );
    assert!(
        app.apply(Action::PreviewRecovery(RecoverySelection::WorkingBranch(
            branch.clone()
        )))
        .is_err()
    );
    assert!(app.apply(Action::PreviewRetirement(branch)).is_err());
    assert!(app.close());
}
#[cfg(target_os = "linux")]
#[test]
fn te13_fix3_standalone_shared_bad() {
    te13_fix3_shared_bad_studio(false, false);
}
#[cfg(target_os = "linux")]
#[test]
fn te13_fix3_host_shared_bad() {
    te13_fix3_shared_bad_studio(true, false);
}
#[cfg(target_os = "linux")]
#[test]
fn te13_fix3_standalone_unbound_bad() {
    te13_fix3_shared_bad_studio(false, true);
}
#[cfg(target_os = "linux")]
#[test]
fn te13_fix3_host_unbound_bad() {
    te13_fix3_shared_bad_studio(true, true);
}

#[cfg(target_os = "linux")]
#[test]
fn te13_fix3_removed_first_block_keeps_other_task_run_message() {
    let (_temp, mut app, branch, _) = te13_fix3_retained_app(true, true, "two");
    let work = app.host.as_ref().unwrap().status.retained_task_work.clone();
    assert_eq!(work.len(), 2);
    assert!(
        app.host
            .as_ref()
            .unwrap()
            .status
            .retained_task_failures
            .is_empty()
    );
    let remaining = work
        .iter()
        .find(|work| work.source.branch != branch)
        .unwrap();
    let action = click_control(
        &app,
        &format!("Remove this working branch {}", branch.as_str()),
    );
    app.apply(action).unwrap();
    let action = click_control(&app, "Confirm working branch loss");
    app.apply(action).unwrap();
    assert_eq!(
        app.host.as_ref().unwrap().status.retained_task_work.len(),
        1
    );
    let observed = app
        .host
        .as_ref()
        .unwrap()
        .client
        .request(kinewright_agent::writer_host::HostCommand::ObserveRun {
            run: remaining.run.clone(),
            after_sequence: 0,
        })
        .unwrap();
    let message = observed.run_view.unwrap().runtime_status;
    assert!(
        message
            .as_ref()
            .is_some_and(|message| message.contains(remaining.task.0.as_str())),
        "ASTRA_OTHER_RETAINED_TASK_RUN_MESSAGE {message:?}"
    );
    let label = format!(
        "Save current edit, then open {}",
        remaining.source.branch.as_str()
    );
    let action = click_control(&app, &label);
    app.apply(action).unwrap();
    let action = click_control(&app, "Save current work, then open recovery point");
    app.apply(action).unwrap();
    assert!(app.close());
}

#[cfg(target_os = "linux")]
#[test]
fn te13_fix3_live_healthy_has_no_stop_instruction() {
    for hosted in [false, true] {
        let (_temp, mut app, _, _) = te13_fix3_retained_app(hosted, false, "live");
        let text = rendered(&app);
        assert!(text.contains("work is kept on working branch"));
        assert!(
            !text.contains("Stop this run") && !text.contains("Stop and wait"),
            "TE13_FIX3_HEALTHY_NO_STOP {hosted}: {text}"
        );
        if let Some(host) = &app.host {
            let work = &host.status.retained_task_work[0];
            let observed = host
                .client
                .request(kinewright_agent::writer_host::HostCommand::ObserveRun {
                    run: work.run.clone(),
                    after_sequence: 0,
                })
                .unwrap();
            assert!(
                observed.run_view.unwrap().runtime_status.is_none(),
                "TE13_FIX3_HEALTHY_NO_BLOCK"
            );
        }
        te13_fix3_stop_studio(&mut app);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !app.close() {
            app.poll_host();
            assert!(
                std::time::Instant::now() < deadline,
                "TE13_FIX3_CLOSE {}",
                app.message
            );
            std::thread::park_timeout(std::time::Duration::from_millis(2));
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn te13_fix3_live_blocked_names_owner_stop() {
    let (_temp, mut app, _, _) = te13_fix3_retained_app(true, true, "live");
    let host = app.host.as_ref().unwrap();
    let work = &host.status.retained_task_work[0];
    let reply = host
        .client
        .request(kinewright_agent::writer_host::HostCommand::ObserveRun {
            run: work.run.clone(),
            after_sequence: 0,
        })
        .unwrap();
    let page = reply.run_view.unwrap();
    assert!(
        page.runtime_status
            .as_ref()
            .is_some_and(|s| s.contains("Stop this run and wait for cleanup")),
        "TE13_FIX3_BLOCKED_STOP_DISCLOSURE {page:?}"
    );
    assert!(
        page.allowed_actions
            .contains(&kinewright_agent::orchestration::orchestration_records::RunActionKind::Stop)
    );
    te13_fix3_stop_studio(&mut app);
    assert!(app.close());
}
#[cfg(target_os = "linux")]
fn te15_b202_recover_words(reason: &str) {
    for hosted in [false, true] {
        let (_temp, mut app, branch, expected) =
            te13_fix3_retained_app(hosted, false, if hosted { "live" } else { "root" });
        te13_fix3_stop_studio(&mut app);
        assert_eq!(
            crate::studio_run_view::plain_wait_reason(reason, false),
            reason,
            "B202_RECOVERY_STATUS_WORDS"
        );
        let instructions: Vec<_> = reason.split('"').collect();
        let open = instructions[1].replace("<branch>", branch.as_str());
        assert!(rendered(&app).contains(&open), "B202_REAL_RECOVERY_CONTROL");
        let action = click_control(&app, &open);
        app.apply(action).unwrap();
        let action = click_control(&app, instructions[3]);
        app.apply(action).unwrap();
        assert_eq!(
            app.owner.core().export_history().unwrap(),
            expected,
            "B202_INSTRUCTIONS_RECOVER_EXACT_HISTORY"
        );
        assert!(app.close());
    }
}
#[cfg(target_os = "linux")]
#[test]
fn te15_b202_sent_recovery_controls() {
    te15_b202_recover_words(
        kinewright_agent::writer_host::task_preparation_words::SENT_TASK_RECOVERY,
    );
}
#[cfg(target_os = "linux")]
#[test]
fn te15_b202_director_recovery_controls() {
    te15_b202_recover_words(
        kinewright_agent::writer_host::task_preparation_words::DIRECTOR_RECOVERY,
    );
}
