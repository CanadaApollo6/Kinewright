#[allow(
    clippy::too_many_lines,
    reason = "real uncertain writes, revoked authority, owner Stop and exact recovery share one history"
)]
fn te13_fix3_owner_stop(
    completion_step: CompletionStep,
    committed: bool,
    revoked: bool,
    preissued: bool,
) {
    let r = c5_fix10_run();
    let expected = r.core.export_history().unwrap();
    let (client, original) = te13_fix2_request(&r);
    let mut stop = preissued.then(|| r.f.control(&r.f.editor, &r.run, RunAction::Stop));
    let operation = if committed {
        "root_parent_sync"
    } else {
        "root_rename_io"
    };
    let nth = if completion_step == CompletionStep::Freeze {
        2
    } else {
        3
    };
    let first = kinewright_project::io_observer::fail_nth(operation, nth, || {
        te13_fix2_dispatch(&r, &client, &original)
    });
    assert_eq!(
        first.orchestration.unwrap().decision,
        CommandDecision::Applied
    );
    let host = r.f.host.owner.upgrade().unwrap();
    {
        let mut owner = host.lock().unwrap();
        assert!(matches!(
            owner.hooks.pending,
            Some(super::super::orchestration::PendingPublication::Completion(
                _
            ))
        ));
        owner.owner_mut().unwrap().reconcile_metadata().unwrap();
        let intent = owner
            .owner()
            .unwrap()
            .orchestration_projection(&r.run)
            .unwrap()
            .unwrap()
            .tasks[1]
            .progress
            .completion
            .clone()
            .unwrap();
        assert_eq!(
            owner
                .owner()
                .unwrap()
                .orchestration_receipt(&r.run, &intent.step_id(completion_step).unwrap())
                .unwrap()
                .is_some(),
            committed,
            "TE13_FIX3_REAL_FAULT_FATE"
        );
        let index = owner
            .resolve_attachment(Some(&client.bootstrap.auth.bearer()))
            .unwrap();
        if revoked {
            owner.attachments[index]
                .alive
                .store(false, std::sync::atomic::Ordering::Release);
        } else {
            owner.attachments[index]
                .producer_scope
                .as_mut()
                .unwrap()
                .expires_at = Some(Instant::now());
        }
    }
    let before = r.f.projection(&r.run);
    let refused = te13_fix2_dispatch(&r, &client, &original);
    assert!(refused.error.is_some(), "TE13_FIX3_STALE_AUTHORITY_REFUSES");
    if !preissued {
        let issued = call(
            &r.f,
            &r.f.editor,
            HostCommand::IssueMetadataIssuer { issuer: None },
            0,
        );
        assert!(
            issued.error.is_none(),
            "TE13_FIX3_OWNER_STOP_ENVELOPE {completion_step:?}/{committed}/{revoked}: {issued:?}"
        );
        assert!(
            host.lock().unwrap().hooks.pending.is_none(),
            "TE13_FIX5_ISSUANCE_RELEASES_BEFORE_STOP"
        );
    }
    if !preissued {
        stop = Some(r.f.control(&r.f.editor, &r.run, RunAction::Stop));
    }
    let mut stop = stop.unwrap();
    {
        let mut owner = host.lock().unwrap();
        if preissued {
            owner.recovering = committed;
            owner.switch_frozen = !committed;
        }
        stop.envelope.expected_generation = owner.owner().unwrap().metadata_generation().unwrap();
        let OrchestrationCommand::ControlRun(control) = &mut stop.envelope.command else {
            unreachable!()
        };
        control.expected_control_revision = owner
            .owner()
            .unwrap()
            .orchestration_projection(&r.run)
            .unwrap()
            .unwrap()
            .control_revision;
        stop.envelope.digest = stop.envelope.computed_digest().unwrap();
    }
    let stopped = te13_fix2_dispatch(&r, &r.f.editor, &stop);
    assert!(
        stopped.error.is_none()
            && stopped
                .orchestration
                .as_ref()
                .is_some_and(|r| r.decision == CommandDecision::Applied),
        "TE13_FIX3_OWNER_STOP {stopped:?}"
    );
    if preissued {
        {
            let mut owner = host.lock().unwrap();
            owner.recovering = false;
            owner.switch_frozen = false;
        }
    }
    // No native process or execution claim exists in these scripted attempts.
    // Record their retirement after Stop, never by abandoning at issuance.
    c5_fix10_retire(&r.f, &r.run, &r.attempt);
    c5_fix10_retire(&r.f, &r.run, &r.parent);
    let after = r.f.projection(&r.run);
    assert!(matches!(after.outcome, RunOutcome::Cancelled(_)));
    assert_eq!(after.cleanup, RunCleanup::Settled, "TE13_FIX3_STOP_CLEANUP");
    assert_eq!(
        after.results, before.results,
        "TE13_FIX3_NO_EXTRA_PUBLICATION"
    );
    assert_eq!(
        after.deliveries.len(),
        before.deliveries.len(),
        "TE13_FIX3_NO_EXTRA_DELIVERY"
    );
    let mut owner = host.lock().unwrap();
    assert!(owner.hooks.pending.is_none());
    let work = owner
        .owner()
        .unwrap()
        .retained_task_work()
        .unwrap()
        .into_iter()
        .find(|work| work.task == r.child)
        .expect("TE13_FIX3_STOP_RETAINED");
    assert!(work.available && work.stopped && work.cleanup_settled);
    assert!(
        owner
            .run_recovery_message(&after)
            .unwrap()
            .is_some_and(|s| s.contains("Save current edit, then open")),
        "TE13_FIX3_STOP_DISCLOSED"
    );
    let project = owner.owner_mut().unwrap();
    project
        .prepare_recovery(
            kinewright_project::bundle_recovery::RecoverySelection::WorkingBranch(
                work.source.branch,
            ),
        )
        .unwrap();
    assert!(
        matches!(
            project.select_local(),
            kinewright_project::bundle_switch::ProjectSelectionResult::Activated(_)
        ),
        "TE13_FIX3_STOP_OPEN"
    );
    assert_eq!(
        project.core().export_history().unwrap(),
        expected,
        "TE13_FIX3_STOP_EXACT"
    );
}

#[test]
fn te13_fix3_stop_freeze_committed_expired() {
    te13_fix3_owner_stop(CompletionStep::Freeze, true, false, false);
}
#[test]
fn te13_fix3_stop_freeze_absent_expired() {
    te13_fix3_owner_stop(CompletionStep::Freeze, false, false, false);
}
#[test]
fn te13_fix3_stop_freeze_committed_revoked() {
    te13_fix3_owner_stop(CompletionStep::Freeze, true, true, false);
}
#[test]
fn te13_fix3_stop_freeze_absent_revoked() {
    te13_fix3_owner_stop(CompletionStep::Freeze, false, true, false);
}
#[test]
fn te13_fix3_stop_publication_committed_expired() {
    te13_fix3_owner_stop(CompletionStep::Publication, true, false, false);
}
#[test]
fn te13_fix3_stop_publication_absent_expired() {
    te13_fix3_owner_stop(CompletionStep::Publication, false, false, false);
}
#[test]
fn te13_fix3_stop_publication_committed_revoked() {
    te13_fix3_owner_stop(CompletionStep::Publication, true, true, false);
}
#[test]
fn te13_fix3_stop_publication_absent_revoked() {
    te13_fix3_owner_stop(CompletionStep::Publication, false, true, false);
}
#[test]
fn te13_fix3_preissued_stop_committed() {
    te13_fix3_owner_stop(CompletionStep::Freeze, true, true, true);
}
#[test]
fn te13_fix3_preissued_stop_absent() {
    te13_fix3_owner_stop(CompletionStep::Publication, false, true, true);
}

#[test]
fn te13_fix3_root_publication_is_not_a_merge() {
    let f = te13_fixture();
    let (run, execution, director) = te13_director(&f);
    let core = te13_core(&f, &run, &execution.task);
    core.request(kinewright_core::Command::Do(history_marker(1)))
        .unwrap();
    core.request(kinewright_core::Command::Do(history_marker(2)))
        .unwrap();
    core.request(kinewright_core::Command::Undo).unwrap();
    let expected = core.export_history().unwrap();
    let reply = director.wire(
        "finish_task",
        te13_control(&director, &execution.task, &core),
    );
    assert_eq!(
        reply.is_error,
        Some(false),
        "TE13_FIX3_ROOT_FINISH {reply:?}"
    );
    assert!(f.projection(&run).deliveries.is_empty());
    c5_fix10_retire(&f, &run, &execution);
    te13_fix1_stop_host(&f.host, &run);
    let host = f.host.owner.upgrade().unwrap();
    let mut owner = host.lock().unwrap();
    let state = owner
        .owner()
        .unwrap()
        .orchestration_projection(&run)
        .unwrap()
        .unwrap();
    let work = owner
        .owner()
        .unwrap()
        .retained_task_work()
        .unwrap()
        .into_iter()
        .find(|work| work.task == execution.task)
        .expect("TE13_FIX3_ROOT_EMPTY_DELIVERY_KEPT");
    assert!(
        owner
            .run_recovery_message(&state)
            .unwrap()
            .is_some_and(|s| s.contains("Save current edit, then open")),
        "TE13_FIX3_ROOT_MESSAGE"
    );
    // Task publication does not publish a Studio Snapshot/Version.
    assert!(owner.owner().unwrap().studio_versions().unwrap().is_empty());
    let project = owner.owner_mut().unwrap();
    project
        .prepare_recovery(
            kinewright_project::bundle_recovery::RecoverySelection::WorkingBranch(
                work.source.branch,
            ),
        )
        .unwrap();
    assert!(matches!(
        project.select_local(),
        kinewright_project::bundle_switch::ProjectSelectionResult::Activated(_)
    ));
    assert_eq!(
        project.core().export_history().unwrap(),
        expected,
        "TE13_FIX3_ROOT_EXACT"
    );
}

#[test]
fn te13_fix3_applied_receipt_settles_before_expired_continuation() {
    let r = c5_fix10_run();
    let (client, original) = te13_fix2_request(&r);
    let first = kinewright_project::io_observer::fail_nth("root_parent_sync", 2, || {
        te13_fix2_dispatch(&r, &client, &original)
    });
    assert_eq!(
        first.orchestration.unwrap().decision,
        CommandDecision::Applied
    );
    let host = r.f.host.owner.upgrade().unwrap();
    let mut owner = host.lock().unwrap();
    let index = owner
        .resolve_attachment(Some(&client.bootstrap.auth.bearer()))
        .unwrap();
    owner.attachments[index]
        .producer_scope
        .as_mut()
        .unwrap()
        .expires_at = Some(Instant::now());
    assert!(
        owner
            .reconcile_completion_substep(index, &original)
            .is_err()
    );
    assert!(
        owner.hooks.pending.is_none(),
        "TE13_FIX3_APPLIED_CLEARS_BEFORE_AUTHORITY"
    );
    let state = owner
        .owner()
        .unwrap()
        .orchestration_projection(&r.run)
        .unwrap()
        .unwrap();
    assert!(
        state.results.is_empty() && state.deliveries.is_empty(),
        "TE13_FIX3_EXPIRED_NO_CONTINUATION"
    );
}

fn te13_fix3_observed_child_kept(parent_finishes: bool) {
    let r = c5_fix10_run();
    let expected = r.core.export_history().unwrap();
    let reply = r
        .editor
        .wire("finish_task", te13_control(&r.editor, &r.child, &r.core));
    assert_eq!(reply.is_error, Some(false));
    let host = r.f.host.owner.upgrade().unwrap();
    let endpoint = host.lock().unwrap().hooks.runs[&r.run].tasks[&r.parent.task].endpoint();
    te13_owner_command(
        &r.f,
        &r.run,
        OrchestrationCommand::BeginTurn(TaskTurn {
            task: r.parent.task.clone(),
            attempt: r.parent.attempt.clone(),
            session: identity(),
            ordinal: 1,
            owner_epoch: endpoint.authority().owner_epoch,
            expected_head: r.f.brief.scope.base.head.clone(),
            purpose: TurnPurpose::Work,
        }),
    );
    te13_owner_command(
        &r.f,
        &r.run,
        OrchestrationCommand::OfferDeliveries(r.parent.task.clone(), r.parent.attempt.clone()),
    );
    let offered = r.f.projection(&r.run);
    assert!(matches!(
        offered.deliveries[0].state,
        DeliveryState::Offered(_, _)
    ));
    let reply = r.director.wire(
        "observe_delivery",
        r.director.control(OrchestrationCommand::ObserveDelivery(
            offered.deliveries[0].id.clone(),
        )),
    );
    assert_eq!(reply.is_error, Some(false));
    if parent_finishes {
        let core = te13_core(&r.f, &r.run, &r.parent.task);
        let reply = r.director.wire(
            "finish_task",
            te13_control(&r.director, &r.parent.task, &core),
        );
        assert_eq!(
            reply.is_error,
            Some(false),
            "TE13_FIX3_PARENT_FINISH_WITHOUT_MERGE {reply:?}"
        );
    }
    c5_fix10_retire(&r.f, &r.run, &r.attempt);
    c5_fix10_retire(&r.f, &r.run, &r.parent);
    te13_fix1_stop_host(&r.f.host, &r.run);
    assert!(matches!(
        r.f.projection(&r.run).deliveries[0].state,
        DeliveryState::Observed(_)
    ));
    let mut owner = host.lock().unwrap();
    let work = owner
        .owner()
        .unwrap()
        .retained_task_work()
        .unwrap()
        .into_iter()
        .find(|work| work.task == r.child)
        .expect("TE13_FIX3_REAL_OBSERVED_CHILD_KEPT");
    let project = owner.owner_mut().unwrap();
    project
        .prepare_recovery(
            kinewright_project::bundle_recovery::RecoverySelection::WorkingBranch(
                work.source.branch,
            ),
        )
        .unwrap();
    assert!(matches!(
        project.select_local(),
        kinewright_project::bundle_switch::ProjectSelectionResult::Activated(_)
    ));
    assert_eq!(
        project.core().export_history().unwrap(),
        expected,
        "TE13_FIX3_REAL_OBSERVED_CHILD_EXACT"
    );
}
#[test]
fn te13_fix3_real_observed_child_stop() {
    te13_fix3_observed_child_kept(false);
}
#[test]
fn te13_fix3_real_parent_finishes_without_merging_child() {
    te13_fix3_observed_child_kept(true);
}
