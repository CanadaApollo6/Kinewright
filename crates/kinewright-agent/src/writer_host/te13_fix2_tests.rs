fn te13_fix2_request(r: &C5Fix10Run) -> (HostClient, ScopedOrchestrationRequest) {
    let producer = r.f.producer_client(&r.run);
    let (client, endpoint) = task_actor(&r.f, &r.run, &r.attempt, &producer);
    let request = task_request(
        &r.f,
        &client,
        &endpoint,
        OrchestrationCommand::FinishTask(FinishTask {
            task: r.child.clone(),
            expected_revision: grant_test_revision(&r.core),
        }),
    );
    (client, request)
}
fn te13_fix2_dispatch(
    r: &C5Fix10Run,
    client: &HostClient,
    request: &ScopedOrchestrationRequest,
) -> HostReply {
    call(
        &r.f,
        client,
        HostCommand::DispatchOrchestration {
            request: Box::new(request.clone()),
        },
        0,
    )
}
#[test]
fn te13_fix2_reconciled_refusal_reopens_real_edit_undo_redo() {
    let r = c5_fix10_run();
    let (client, stale) = te13_fix2_request(&r);
    let undone = r.editor.raw(
        "undo_task_edit",
        serde_json::json!({"expected_revision":grant_test_revision(&r.core)}),
    );
    assert_eq!(undone.is_error, Some(false));
    let first = kinewright_project::io_observer::fail_once("root_parent_sync", || {
        te13_fix2_dispatch(&r, &client, &stale)
    });
    assert_eq!(first.error.unwrap().code, "durability_unknown");
    let replay = te13_fix2_dispatch(&r, &client, &stale);
    assert!(replay.error.is_none(), "{replay:?}");
    assert!(matches!(
        replay.orchestration.unwrap().decision,
        CommandDecision::Refused(_)
    ));
    assert!(
        r.f.projection(&r.run).tasks[1]
            .progress
            .completion
            .is_none()
    );
    let redone = r.editor.raw(
        "redo_task_edit",
        serde_json::json!({"expected_revision":grant_test_revision(&r.core)}),
    );
    assert_eq!(
        redone.is_error,
        Some(false),
        "TE13_FIX2_RECONCILED_REFUSAL_REDO {redone:?}"
    );
    let undone = r.editor.raw(
        "undo_task_edit",
        serde_json::json!({"expected_revision":grant_test_revision(&r.core)}),
    );
    assert_eq!(
        undone.is_error,
        Some(false),
        "TE13_FIX2_RECONCILED_REFUSAL_UNDO {undone:?}"
    );
    let redone = r.editor.raw(
        "redo_task_edit",
        serde_json::json!({"expected_revision":grant_test_revision(&r.core)}),
    );
    assert_eq!(redone.is_error, Some(false));
    let revision = grant_test_revision(&r.core);
    let prepared = r.editor.raw("prepare_edit_plan", serde_json::json!({"expected_revision":revision,"operations":[{"op":"move_marker","marker":1,"to":3}]}));
    assert_eq!(prepared.is_error, Some(false));
    let edited = r.editor.raw("commit_edit_plan", serde_json::json!({"expected_revision":revision,"plan_id":prepared.structured_content.unwrap()["plan_id"]}));
    assert_eq!(
        edited.is_error,
        Some(false),
        "TE13_FIX2_RECONCILED_REFUSAL_EDIT {edited:?}"
    );
    let fresh = te13_control(&r.editor, &r.child, &r.core);
    let finished = r.editor.wire("finish_task", fresh);
    assert_eq!(
        finished.is_error,
        Some(false),
        "TE13_FIX2_RECONCILED_REFUSAL_FINISH {finished:?}"
    );
    assert_eq!(r.f.projection(&r.run).results.len(), 1);
}
#[allow(
    clippy::too_many_lines,
    reason = "real uncertainty, counterfactual identities and exactly-once completion share one retained command"
)]
fn te13_fix2_uncertain_substep(step: CompletionStep, committed: bool) {
    let r = c5_fix10_run();
    let (client, original) = te13_fix2_request(&r);
    let operation = if committed {
        "root_parent_sync"
    } else {
        "root_rename_io"
    };
    let nth = if step == CompletionStep::Freeze { 2 } else { 3 };
    let first = kinewright_project::io_observer::fail_nth(operation, nth, || {
        te13_fix2_dispatch(&r, &client, &original)
    });
    assert!(first.error.is_none(), "TE13_FIX2_OUTER_INTENT {first:?}");
    let receipt = first.orchestration.unwrap();
    assert_eq!(receipt.decision, CommandDecision::Applied);
    let host = r.f.host.owner.upgrade().unwrap();
    {
        let owner = host.lock().unwrap();
        assert!(
            matches!(
                owner.hooks.pending,
                Some(super::super::orchestration::PendingPublication::Completion(
                    _
                ))
            ),
            "TE13_FIX2_REAL_SUBSTEP_UNKNOWN {:?}",
            owner.hooks.completion_failure
        );
    }
    {
        let mut owner = host.lock().unwrap();
        owner.owner_mut().unwrap().reconcile_metadata().unwrap();
        let step_id = CommandId {
            issuer: original.envelope.id.issuer.clone(),
            sequence: original.envelope.id.sequence
                + if step == CompletionStep::Freeze { 1 } else { 2 },
        };
        assert_eq!(
            owner
                .owner()
                .unwrap()
                .orchestration_receipt(&r.run, &step_id)
                .unwrap()
                .is_some(),
            committed,
            "TE13_FIX2_FAULT_OUTCOME {step:?}"
        );
    }
    let root = std::fs::read(r.f.temp.path("project.kinewright")).unwrap();
    let mut wrong = original.clone();
    wrong.envelope.id.sequence += 9;
    wrong.envelope.digest = wrong.envelope.computed_digest().unwrap();
    let different = te13_fix2_dispatch(&r, &client, &wrong);
    assert!(different.error.is_some(), "TE13_FIX2_SUBSTEP_COMMAND_BOUND");
    let mut wrong = original.clone();
    let OrchestrationCommand::FinishTask(finish) = &mut wrong.envelope.command else {
        unreachable!()
    };
    finish.task = r.parent.task.clone();
    wrong.envelope.digest = wrong.envelope.computed_digest().unwrap();
    assert!(
        te13_fix2_dispatch(&r, &client, &wrong).error.is_some(),
        "TE13_FIX2_SUBSTEP_TASK_BOUND"
    );
    let mut wrong = original.clone();
    wrong.envelope.authority.attempt = Some(identity());
    wrong.envelope.digest = wrong.envelope.computed_digest().unwrap();
    assert!(
        te13_fix2_dispatch(&r, &client, &wrong).error.is_some(),
        "TE13_FIX2_SUBSTEP_ATTEMPT_BOUND"
    );
    let mut wrong = original.clone();
    wrong.run = Some(RunId(identity()));
    assert!(
        te13_fix2_dispatch(&r, &client, &wrong).error.is_some(),
        "TE13_FIX2_SUBSTEP_RUN_BOUND"
    );
    let other = te13_fix2_dispatch(&r, &r.f.editor, &original);
    assert!(other.error.is_some(), "TE13_FIX2_SUBSTEP_ATTACHMENT_BOUND");
    assert_eq!(
        std::fs::read(r.f.temp.path("project.kinewright")).unwrap(),
        root,
        "TE13_FIX2_REFUSALS_NO_WRITE"
    );
    // The original finish is the only public request that can resume the private step.
    let replay = te13_fix2_dispatch(&r, &client, &original);
    assert!(
        replay.error.is_none(),
        "TE13_FIX2_SUBSTEP_RETRY {step:?}/{committed}: {replay:?}"
    );
    assert_eq!(
        replay.orchestration.unwrap(),
        receipt,
        "TE13_FIX2_ORIGINAL_RECEIPT"
    );
    let state = r.f.projection(&r.run);
    assert_eq!(state.results.len(), 1, "TE13_FIX2_ONE_PUBLICATION");
    assert_eq!(state.deliveries.len(), 1, "TE13_FIX2_ONE_DELIVERY");
    assert!(host.lock().unwrap().hooks.pending.is_none());
    let intent = state.tasks[1].progress.completion.as_ref().unwrap();
    let owner = host.lock().unwrap();
    let freeze = owner
        .owner()
        .unwrap()
        .orchestration_receipt(&r.run, &intent.step_id(CompletionStep::Freeze).unwrap())
        .unwrap()
        .unwrap();
    let publish = owner
        .owner()
        .unwrap()
        .orchestration_receipt(
            &r.run,
            &intent.step_id(CompletionStep::Publication).unwrap(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(freeze.decision, CommandDecision::Applied);
    assert_eq!(publish.decision, CommandDecision::Applied);
    drop(owner);
    // The now-terminal Producer gains no new authority. The owner can still
    // read its original immutable receipt through the normal reconciliation API.
    let repeated = call(
        &r.f,
        &r.f.editor,
        HostCommand::ReconcileTaskReceipt {
            query: super::super::receipt_reconciliation::ReceiptQuery {
                project: original.envelope.project.clone(),
                run: r.run.clone(),
                task: r.child.clone(),
                attempt: r.attempt.attempt.clone(),
                id: original.envelope.id.clone(),
                digest: Some(task_command_digest(&original.envelope.command).unwrap()),
                command: None,
            },
        },
        0,
    );
    assert!(repeated.error.is_none(), "{repeated:?}");
    let Some(super::super::receipt_reconciliation::ReceiptReply::Stored {
        receipt: repeated, ..
    }) = repeated.task_receipt
    else {
        panic!("TE13_FIX2_STORED_OUTCOME")
    };
    assert_eq!(repeated, serde_json::to_value(&receipt).unwrap());
    let state = r.f.projection(&r.run);
    assert_eq!(state.results.len(), 1);
    assert_eq!(state.deliveries.len(), 1);
    let owner = host.lock().unwrap();
    assert_eq!(
        owner
            .owner()
            .unwrap()
            .orchestration_receipt(&r.run, &freeze.envelope.id)
            .unwrap(),
        Some(freeze)
    );
    assert_eq!(
        owner
            .owner()
            .unwrap()
            .orchestration_receipt(&r.run, &publish.envelope.id)
            .unwrap(),
        Some(publish)
    );
}
#[test]
fn te13_fix2_uncertain_freeze_committed() {
    te13_fix2_uncertain_substep(CompletionStep::Freeze, true);
}
#[test]
fn te13_fix2_uncertain_freeze_absent() {
    te13_fix2_uncertain_substep(CompletionStep::Freeze, false);
}
#[test]
fn te13_fix2_uncertain_publication_committed() {
    te13_fix2_uncertain_substep(CompletionStep::Publication, true);
}
#[test]
fn te13_fix2_uncertain_publication_absent() {
    te13_fix2_uncertain_substep(CompletionStep::Publication, false);
}

#[test]
fn te13_fix2_real_mcp_can_retry_the_retained_finish() {
    for (step, committed) in [
        (CompletionStep::Freeze, true),
        (CompletionStep::Freeze, false),
        (CompletionStep::Publication, true),
        (CompletionStep::Publication, false),
    ] {
        let r = c5_fix10_run();
        let (client, original) = te13_fix2_request(&r);
        let operation = if committed {
            "root_parent_sync"
        } else {
            "root_rename_io"
        };
        let nth = if step == CompletionStep::Freeze { 2 } else { 3 };
        let first = kinewright_project::io_observer::fail_nth(operation, nth, || {
            te13_fix2_dispatch(&r, &client, &original)
        });
        let receipt = first.orchestration.unwrap();
        let control = serde_json::json!({
            "id":original.envelope.id, "attempt":r.attempt.attempt,
            "expected_generation":original.envelope.expected_generation,
            "owner_epoch":original.envelope.authority.owner_epoch,
            "expected_head":r.f.brief.scope.base.head, "command":original.envelope.command,
        });
        let replay = r.editor.wire("finish_task", control);
        assert_eq!(
            replay.is_error,
            Some(false),
            "TE13_FIX2_MCP_PENDING_FINISH {step:?}/{committed}: {replay:?}"
        );
        assert_eq!(
            replay.structured_content.unwrap()["outcome"]["receipt"],
            serde_json::to_value(receipt).unwrap()
        );
        let state = r.f.projection(&r.run);
        assert_eq!(state.results.len(), 1);
        assert_eq!(state.deliveries.len(), 1);
    }
}

#[test]
fn te13_fix2_completed_merge_has_no_recovery_offer() {
    let r = c5_fix10_run();
    let finish = te13_control(&r.editor, &r.child, &r.core);
    let first = r.editor.wire("finish_task", finish);
    assert_eq!(first.is_error, Some(false));
    assert!(
        r.f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .retained_task_work()
            .unwrap()
            .iter()
            .any(|work| work.task == r.child),
        "TE13_FIX2_UNMERGED_STILL_KEPT"
    );
    let merged = r.director.wire(
        "merge_task",
        r.director
            .control(OrchestrationCommand::MergeTask(MergeTask {
                task: r.child.clone(),
                target: r.f.brief.scope.base.clone(),
                result: r.f.projection(&r.run).results.last().unwrap().clone(),
            })),
    );
    assert_eq!(merged.is_error, Some(false), "{merged:?}");
    let state = r.f.projection(&r.run);
    let host = r.f.host.owner.upgrade().unwrap();
    {
        let owner = host.lock().unwrap();
        assert!(
            !owner
                .owner()
                .unwrap()
                .retained_task_work()
                .unwrap()
                .iter()
                .any(|work| work.task == r.child),
            "TE13_FIX2_MERGED_NO_OFFER"
        );
        assert!(
            owner.run_recovery_message(&state).unwrap().is_none(),
            "TE13_FIX2_MERGED_NO_RUN_MESSAGE"
        );
    }
    te13_fix1_stop_host(&r.f.host, &r.run);
    let stopped = r.f.projection(&r.run);
    let owner = host.lock().unwrap();
    assert!(
        !owner
            .owner()
            .unwrap()
            .retained_task_work()
            .unwrap()
            .iter()
            .any(|work| work.task == r.child),
        "TE13_FIX2_STOPPED_MERGED_NO_OFFER"
    );
    let message = owner
        .run_recovery_message(&stopped)
        .unwrap()
        .unwrap_or_default();
    assert!(
        !message.contains(r.child.0.as_str()),
        "TE13_FIX2_STOPPED_MERGED_NO_RUN_MESSAGE {message}"
    );
}

#[test]
fn te13_fix2_standalone_freeze_storage_refusal_can_retry() {
    let r = c5_fix10_run();
    let producer = r.f.producer_client(&r.run);
    let (client, endpoint) = task_actor(&r.f, &r.run, &r.attempt, &producer);
    let request = task_request(
        &r.f,
        &client,
        &endpoint,
        OrchestrationCommand::FreezeTask(r.child.clone()),
    );
    let first = kinewright_project::io_observer::fail_once("object_file_sync", || {
        te13_fix2_dispatch(&r, &client, &request)
    });
    assert!(
        first.error.is_some(),
        "TE13_FIX2_STANDALONE_FREEZE_REAL_STORAGE_FAILURE"
    );
    assert!(
        endpoint.admit().is_err(),
        "TE13_FIX2_STANDALONE_FREEZE_SEALED"
    );
    let state = r.f.projection(&r.run);
    assert!(state.tasks[1].progress.completion.is_none());
    assert!(
        r.f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .orchestration_receipt(&r.run, &request.envelope.id)
            .unwrap()
            .is_none()
    );
    let retry = te13_fix2_dispatch(&r, &client, &request);
    assert!(
        retry.error.is_none(),
        "TE13_FIX2_STANDALONE_FREEZE_RETRY {retry:?}"
    );
    assert_eq!(
        retry.orchestration.unwrap().decision,
        CommandDecision::Applied
    );
}

#[test]
fn te13_fix2_stop_disposed_delivery_keeps_unreviewed_task_work() {
    let r = c5_fix10_run();
    let expected = r.core.export_history().unwrap();
    let finish = te13_control(&r.editor, &r.child, &r.core);
    let reply = r.editor.wire("finish_task", finish);
    assert_eq!(reply.is_error, Some(false));
    let state = r.f.projection(&r.run);
    assert!(matches!(state.deliveries[0].state, DeliveryState::Pending));
    let host = r.f.host.owner.upgrade().unwrap();
    let saved = host
        .lock()
        .unwrap()
        .saved_task_history(&state, &r.child)
        .unwrap()
        .unwrap();
    // These scripted attempts have no native process. Record their actual
    // finished cleanup before Stop, so the person can use the recovery action.
    c5_fix10_retire(&r.f, &r.run, &r.attempt);
    c5_fix10_retire(&r.f, &r.run, &r.parent);
    te13_fix1_stop_host(&r.f.host, &r.run);
    let stopped = r.f.projection(&r.run);
    assert!(matches!(
        stopped.deliveries[0].state,
        DeliveryState::Disposed
    ));
    let mut owner = host.lock().unwrap();
    let work = owner.owner().unwrap().retained_task_work().unwrap();
    let kept = work.iter().find(|work| work.task == r.child);
    assert!(
        kept.is_some_and(|work| work.available && work.stopped && work.cleanup_settled),
        "TE13_FIX2_DISPOSED_DELIVERY_KEPT {work:?} cleanup={:?}",
        stopped.cleanup
    );
    assert_eq!(kept.unwrap().source, saved.source);
    assert!(
        owner
            .run_recovery_message(&stopped)
            .unwrap()
            .is_some_and(|text| text.contains("Save current edit, then open")),
        "TE13_FIX2_DISPOSED_DELIVERY_DISCLOSED"
    );
    let project = owner.owner_mut().unwrap();
    project
        .prepare_recovery(
            kinewright_project::bundle_recovery::RecoverySelection::WorkingBranch(
                saved.source.branch.clone(),
            ),
        )
        .unwrap();
    assert!(
        matches!(
            project.select_local(),
            kinewright_project::bundle_switch::ProjectSelectionResult::Activated(_)
        ),
        "TE13_FIX2_DISPOSED_DELIVERY_RECOVERS"
    );
    assert_eq!(project.core().export_history().unwrap(), expected);
}
