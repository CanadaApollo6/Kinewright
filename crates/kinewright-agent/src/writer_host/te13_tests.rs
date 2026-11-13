#[test]
fn te13_finish_fuses_freeze_and_publication() {
    let r = c5_fix10_run();
    let mut control = r
        .editor
        .control(OrchestrationCommand::FreezeTask(r.child.clone()));
    control["command"] = serde_json::json!({"kind":"finish_task","value":{
        "task":r.child,"expected_revision":grant_test_revision(&r.core)
    }});
    let reply = r.editor.wire("finish_task", control);
    assert_eq!(reply.is_error, Some(false), "TE13_FINISH_FUSED {reply:?}");
    let state = r.f.projection(&r.run);
    assert!(
        matches!(state.tasks[1].state, TaskState::Terminal(_)),
        "TE13_DURABLE_PUBLICATION"
    );
    assert_eq!(state.results.len(), 1, "TE13_ONE_RESULT");
    assert_eq!(state.deliveries.len(), 1, "TE13_ONE_DELIVERY");
    let host = r.f.host.owner.upgrade().unwrap();
    let host = host.lock().unwrap();
    assert!(
        host.saved_task_history(&state, &r.child)
            .unwrap()
            .unwrap()
            .frozen,
        "TE13_FROZEN_HISTORY"
    );
}

fn te13_control(mcp: &C4Mcp, task: &TaskId, core: &Core) -> serde_json::Value {
    mcp.control(OrchestrationCommand::FinishTask(FinishTask {
        task: task.clone(),
        expected_revision: grant_test_revision(core),
    }))
}

// Production Revise runs target a registered candidate, not the manual Core.
fn te13_candidate_fixture(mut f: Fixture) -> Fixture {
    {
        let host = f.host.owner.upgrade().unwrap();
        let mut host = host.lock().unwrap();
        let owner = host.owner_mut().unwrap();
        owner.save_active().unwrap();
        let mut candidate = owner
            .fork_durable_branch(
                "te13-candidate",
                kinewright_project::editing_session::DurableBranchRole::RunCandidate,
            )
            .unwrap();
        owner.persist_durable_branch(&mut candidate).unwrap();
        f.brief.scope.base = owner.candidate_binding(candidate.branch_id()).unwrap();
    }
    f
}
fn te13_fixture() -> Fixture {
    te13_candidate_fixture(Fixture::new())
}
fn te13_director(f: &Fixture) -> (RunId, ExecutionIdentity, C4Mcp) {
    let (run, execution, producer) = reserve(f);
    let (client, endpoint) = task_actor(f, &run, &execution, &producer);
    history_turn(f, &execution, &client, &endpoint, 1, true);
    let mcp = C4Mcp::new(f, &run, &execution.task);
    (run, execution, mcp)
}
fn te13_core(f: &Fixture, run: &RunId, task: &TaskId) -> Core {
    f.host.owner.upgrade().unwrap().lock().unwrap().hooks.runs[run].tasks[task]
        .core()
        .clone()
}
fn te13_cut(f: &Fixture, step: CompletionStep) {
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .hooks
        .completion_cut = Some(step);
}
fn te13_query(
    execution: &ExecutionIdentity,
    control: &serde_json::Value,
) -> super::super::receipt_reconciliation::ReceiptQuery {
    super::super::receipt_reconciliation::ReceiptQuery {
        project: execution.project.clone(),
        run: execution.run.clone(),
        task: execution.task.clone(),
        attempt: execution.attempt.clone(),
        id: serde_json::from_value(control["id"].clone()).unwrap(),
        digest: None,
        command: Some(serde_json::from_value(control["command"].clone()).unwrap()),
    }
}
fn te13_read(
    client: &HostClient,
    query: super::super::receipt_reconciliation::ReceiptQuery,
) -> (serde_json::Value, super::super::CompletionStatus) {
    let reply = client
        .request(HostCommand::ReconcileTaskReceipt { query })
        .unwrap();
    assert!(reply.error.is_none(), "TE13_READ {reply:?}");
    let Some(super::super::receipt_reconciliation::ReceiptReply::Stored {
        outcome,
        completion: Some(status),
        ..
    }) = reply.task_receipt
    else {
        panic!("TE13_RETAINED_COMPLETION {reply:?}");
    };
    (outcome, *status)
}
fn te13_status(f: &Fixture, run: &RunId, task: &TaskId) -> super::super::CompletionStatus {
    let host = f.host.owner.upgrade().unwrap();
    let host = host.lock().unwrap();
    let state = host
        .owner()
        .unwrap()
        .orchestration_projection(run)
        .unwrap()
        .unwrap();
    let intent = state
        .tasks
        .iter()
        .find(|t| t.id == *task)
        .unwrap()
        .progress
        .completion
        .as_deref()
        .unwrap();
    host.completion_status(&state, intent).unwrap()
}
fn te13_owner_command(f: &Fixture, run: &RunId, command: OrchestrationCommand) {
    let producer = f.producer(run);
    let request = f.request(&f.producer_client(run), Some(run.clone()), command);
    let host = f.host.owner.upgrade().unwrap();
    let mut host = host.lock().unwrap();
    let owner = host.owner().unwrap();
    let state = owner.orchestration_projection(run).unwrap().unwrap();
    let segment = owner.orchestration_cut(run).unwrap().unwrap().segment;
    let update = crate::orchestration::machine::prepare(
        Some(&state),
        &request.envelope.project,
        owner.metadata_generation().unwrap(),
        &request.envelope,
        Some(segment),
    )
    .unwrap();
    host.publish_orchestration_transition(&producer, &update, None)
        .unwrap();
}

#[test]
fn te13_unready_finish_does_not_reserve_the_parent_settlement_ids() {
    let f = te13_fixture();
    let (run, parent, mcp) = te13_director(&f);
    let mut brief = f.brief.clone();
    brief.role = Role::Editor;
    mcp.accepted(
        "assign_task",
        OrchestrationCommand::AssignTask(Box::new(AssignTask {
            run: run.clone(),
            parent: parent.task.clone(),
            brief,
        })),
    );
    let child = f.projection(&run).tasks[1].id.clone();
    let control = te13_control(&mcp, &parent.task, &te13_core(&f, &run, &parent.task));
    let reply = mcp.wire("finish_task", control);
    assert_eq!(
        reply.is_error,
        Some(true),
        "TE13_UNREADY_FINISH_REFUSED {reply:?}"
    );
    assert!(
        f.projection(&run).tasks[0].progress.completion.is_none(),
        "TE13_UNREADY_NO_ID_RESERVATION"
    );
    let (execution, client, endpoint, core) = c4_reserve_task(&f, &run, &child);
    history_turn(&f, &execution, &client, &endpoint, 1, true);
    let editor = C4Mcp::new(&f, &run, &child);
    let result = editor.wire("finish_task", te13_control(&editor, &child, &core));
    assert_eq!(
        result.is_error,
        Some(false),
        "TE13_CHILD_CAN_FINISH {result:?}"
    );
    te13_owner_command(
        &f,
        &run,
        OrchestrationCommand::OfferDeliveries(parent.task.clone(), parent.attempt.clone()),
    );
    let delivery = f.projection(&run).deliveries[0].id.clone();
    let observed = mcp.wire(
        "observe_delivery",
        mcp.control(OrchestrationCommand::ObserveDelivery(delivery)),
    );
    assert_eq!(
        observed.is_error,
        Some(false),
        "TE13_PARENT_CAN_SETTLE_AFTER_REFUSAL {observed:?}"
    );
    assert!(
        matches!(
            f.projection(&run).deliveries[0].state,
            DeliveryState::Observed(_)
        ),
        "TE13_PARENT_ACTUALLY_SETTLED"
    );
}

#[test]
fn te13_lost_freeze_reply_replays_original_substeps_after_clock_tick() {
    let f = te13_fixture();
    let (run, execution, mcp) = te13_director(&f);
    let core = te13_core(&f, &run, &execution.task);
    let control = te13_control(&mcp, &execution.task, &core);
    te13_cut(&f, CompletionStep::Freeze);
    let first = mcp.wire("finish_task", control.clone());
    assert_eq!(first.is_error, Some(false), "TE13_INTENT_ACK");
    let first = first.structured_content.unwrap();
    let pending = te13_status(&f, &run, &execution.task);
    assert_ne!(
        pending.state,
        super::super::CompletionState::Completed,
        "TE13_FREEZE_IS_NOT_COMPLETION"
    );
    assert!(
        pending.freeze_receipt.is_some() && pending.publication_receipt.is_none(),
        "TE13_DURABLE_FREEZE_ONLY"
    );
    let intent = f.projection(&run).tasks[0]
        .progress
        .completion
        .clone()
        .unwrap();
    assert_eq!(
        intent.id,
        serde_json::from_value(control["id"].clone()).unwrap(),
        "TE13_INTENT_DURABLE_BEFORE_FREEZE"
    );
    assert_eq!(
        pending.freeze_receipt.as_ref().unwrap().envelope.id,
        intent.step_id(CompletionStep::Freeze).unwrap(),
        "TE13_FIXED_FREEZE_ID"
    );
    c5_fix10_tick(&f, &run, &execution);
    let replay = mcp.wire("finish_task", control.clone());
    assert_eq!(
        replay.is_error,
        Some(false),
        "TE13_RETRY_AFTER_FREEZE {replay:?}"
    );
    let replay = replay.structured_content.unwrap();
    assert_eq!(
        first["outcome"], replay["outcome"],
        "TE13_IMMUTABLE_INTENT_OUTCOME"
    );
    let (outcome, complete) = te13_read(&f.editor, te13_query(&execution, &control));
    assert_eq!(outcome, first["outcome"], "TE13_RECONCILED_OUTCOME");
    assert_eq!(
        complete.state,
        super::super::CompletionState::Completed,
        "TE13_RETRY_COMPLETES"
    );
    assert_eq!(
        serde_json::to_value(&complete.freeze_receipt).unwrap(),
        serde_json::to_value(&pending.freeze_receipt).unwrap(),
        "TE13_ORIGINAL_FREEZE_RECEIPT"
    );
    assert_eq!(
        f.projection(&run).results.len(),
        1,
        "TE13_NO_DUPLICATE_PUBLICATION"
    );
}

#[test]
fn te13_progress_read_failure_keeps_the_acknowledged_intent_outcome() {
    let f = te13_fixture();
    let (run, execution, mcp) = te13_director(&f);
    let control = te13_control(&mcp, &execution.task, &te13_core(&f, &run, &execution.task));
    te13_cut(&f, CompletionStep::Freeze);
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .hooks
        .completion_status_read_failure = true;
    let first = mcp.wire("finish_task", control.clone());
    assert_eq!(
        first.is_error,
        Some(false),
        "TE13_PROGRESS_FAILURE_ACK {first:?}"
    );
    let first = first.structured_content.unwrap();
    assert!(
        first["receipt"].is_object(),
        "TE13_PROGRESS_FAILURE_RETAINS_RECEIPT"
    );
    assert_eq!(
        first["completion_status"]["state"], "blocked",
        "TE13_PROGRESS_UNAVAILABLE_BLOCKED"
    );
    assert_eq!(
        first["completion_status"]["reason"], "completion_state_unavailable",
        "TE13_PROGRESS_REASON"
    );
    let (outcome, status) = te13_read(&f.editor, te13_query(&execution, &control));
    assert_eq!(
        outcome, first["outcome"],
        "TE13_PROGRESS_FAILURE_ORIGINAL_OUTCOME"
    );
    assert!(
        status.freeze_receipt.is_some() && status.publication_receipt.is_none(),
        "TE13_PROGRESS_FAILURE_RETAINED_FACTS"
    );
}

#[test]
fn te13_lost_publication_reply_returns_original_receipts_and_result() {
    let f = te13_fixture();
    let (run, execution, mcp) = te13_director(&f);
    let control = te13_control(&mcp, &execution.task, &te13_core(&f, &run, &execution.task));
    te13_cut(&f, CompletionStep::Publication);
    let first = mcp
        .wire("finish_task", control.clone())
        .structured_content
        .unwrap();
    let query = te13_query(&execution, &control);
    let (original, status) = te13_read(&f.editor, query.clone());
    assert_eq!(original, first["outcome"], "TE13_PUBLISH_ORIGINAL_OUTCOME");
    assert_eq!(
        status.state,
        super::super::CompletionState::Completed,
        "TE13_PUBLISH_RETAINED"
    );
    assert!(
        status.publication_receipt.is_some() && status.result.is_some(),
        "TE13_PUBLISH_ORIGINAL_RESULT"
    );
    c5_fix10_tick(&f, &run, &execution);
    let (again, replay) = te13_read(&f.editor, query);
    assert_eq!(again, original, "TE13_PUBLISH_REPLAY_OUTCOME");
    assert_eq!(replay, status, "TE13_PUBLISH_REPLAY_EXACT");
    assert_eq!(
        mcp.wire("finish_task", control).is_error,
        Some(true),
        "TE13_TERMINAL_PORT_STAYS_REVOKED"
    );
    assert_eq!(f.projection(&run).results.len(), 1, "TE13_PUBLISH_ONCE");
}

#[test]
fn te13_delivery_must_be_observed_and_lost_reply_is_reconciled() {
    let r = c5_fix10_run();
    let control = te13_control(&r.editor, &r.child, &r.core);
    r.editor.wire("finish_task", control.clone());
    let query = te13_query(&r.attempt, &control);
    let (_, waiting) = te13_read(&r.f.editor, query.clone());
    assert_eq!(
        waiting.state,
        super::super::CompletionState::Pending,
        "TE13_DELIVERY_REQUIRED"
    );
    assert_eq!(
        waiting.phase,
        super::super::CompletionPhase::Delivery,
        "TE13_WAITING_FOR_PARENT"
    );
    let owner = r.f.host.owner.upgrade().unwrap();
    let endpoint = owner.lock().unwrap().hooks.runs[&r.run].tasks[&r.parent.task].endpoint();
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
    let delivery = r.f.projection(&r.run).deliveries[0].id.clone();
    let observe = r
        .director
        .control(OrchestrationCommand::ObserveDelivery(delivery));
    let observed = r.director.wire("observe_delivery", observe.clone());
    assert_eq!(
        observed.is_error,
        Some(false),
        "TE13_AUTHENTICATED_DELIVERY {observed:?}"
    );
    let (_, completed) = te13_read(&r.f.editor, query.clone());
    assert_eq!(
        completed.state,
        super::super::CompletionState::Completed,
        "TE13_OBSERVED_COMPLETES"
    );
    let read =
        r.f.editor
            .request(HostCommand::ReconcileTaskReceipt {
                query: te13_query(&r.parent, &observe),
            })
            .unwrap();
    let Some(super::super::receipt_reconciliation::ReceiptReply::Stored { outcome, .. }) =
        read.task_receipt
    else {
        panic!("TE13_DELIVERY_RECEIPT_RETAINED");
    };
    assert_eq!(
        outcome,
        observed.structured_content.unwrap()["outcome"],
        "TE13_DELIVERY_LOST_REPLY"
    );
    assert_eq!(
        te13_read(&r.f.editor, query).1,
        completed,
        "TE13_DELIVERY_COMPLETES_ONCE"
    );
    assert_eq!(
        r.f.projection(&r.run).deliveries.len(),
        1,
        "TE13_DELIVERY_NOT_DUPLICATED"
    );
}

#[test]
fn te13_stop_blocks_missing_work_but_retains_original_freeze() {
    let f = te13_fixture();
    let (run, execution, mcp) = te13_director(&f);
    let control = te13_control(&mcp, &execution.task, &te13_core(&f, &run, &execution.task));
    te13_cut(&f, CompletionStep::Freeze);
    mcp.wire("finish_task", control.clone());
    let query = te13_query(&execution, &control);
    let (outcome, frozen) = te13_read(&f.editor, query.clone());
    assert!(
        frozen.freeze_receipt.is_some() && frozen.result.is_some(),
        "TE13_STOP_RETAINS_A_REAL_FREEZE"
    );
    let stop = f.control(&f.editor, &run, RunAction::Stop);
    ok(
        f.editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(stop),
            })
            .unwrap(),
        "TE13_STOP_APPLIED",
    );
    let before = f.projection(&run);
    assert_eq!(
        mcp.wire("finish_task", control).is_error,
        Some(true),
        "TE13_STOP_NO_AUTHORITY"
    );
    let (again, blocked) = te13_read(&f.editor, query);
    assert_eq!(
        blocked.state,
        super::super::CompletionState::Blocked,
        "TE13_STOP_BLOCKS_COMPLETION"
    );
    assert_eq!(
        blocked.reason.as_deref(),
        Some("run_stopped"),
        "TE13_STOP_REASON"
    );
    assert_eq!(again, outcome, "TE13_STOP_ORIGINAL_OUTCOME");
    assert_eq!(
        blocked.freeze_receipt, frozen.freeze_receipt,
        "TE13_STOP_ORIGINAL_FREEZE"
    );
    assert_eq!(
        f.projection(&run).results,
        before.results,
        "TE13_STOP_NO_PUBLICATION"
    );
}

#[test]
fn te13_changed_completion_payload_and_reserved_id_are_refused() {
    let f = te13_fixture();
    let (run, execution, mcp) = te13_director(&f);
    let control = te13_control(&mcp, &execution.task, &te13_core(&f, &run, &execution.task));
    te13_cut(&f, CompletionStep::Freeze);
    mcp.wire("finish_task", control.clone());
    let before = f.projection(&run);
    let mut changed = control.clone();
    changed["command"]["value"]["expected_revision"] =
        serde_json::json!(grant_test_revision(&te13_core(&f, &run, &execution.task)).0 + 1);
    assert_eq!(
        mcp.wire("finish_task", changed.clone()).is_error,
        Some(true),
        "TE13_COMPLETION_COLLISION"
    );
    let read = f
        .editor
        .request(HostCommand::ReconcileTaskReceipt {
            query: te13_query(&execution, &changed),
        })
        .unwrap();
    assert!(read.error.is_some(), "TE13_COLLISION_READ_REFUSED");
    let mut steal = mcp.control(OrchestrationCommand::CancelTask(execution.task.clone()));
    let intent = before.tasks[0].progress.completion.as_deref().unwrap();
    steal["id"] =
        serde_json::to_value(intent.step_id(CompletionStep::Publication).unwrap()).unwrap();
    assert_eq!(
        mcp.wire("cancel_task", steal).is_error,
        Some(true),
        "TE13_RESERVED_STEP_REFUSED"
    );
    assert_eq!(
        f.projection(&run).results,
        before.results,
        "TE13_COLLISION_NO_EFFECT"
    );
}

#[test]
fn te13_public_dispatch_also_drives_completion() {
    let f = te13_fixture();
    let (run, execution, producer) = reserve(&f);
    let (client, endpoint) = task_actor(&f, &run, &execution, &producer);
    history_turn(&f, &execution, &client, &endpoint, 1, true);
    let core = te13_core(&f, &run, &execution.task);
    let mut request = f.request(
        &client,
        Some(run.clone()),
        OrchestrationCommand::FinishTask(FinishTask {
            task: execution.task.clone(),
            expected_revision: grant_test_revision(&core),
        }),
    );
    request.envelope.authority.endpoint = Some(endpoint.authority().endpoint.clone());
    request.envelope.authority.attempt = Some(execution.attempt.clone());
    request.envelope.digest = request.envelope.computed_digest().unwrap();
    let reply = client
        .request(HostCommand::DispatchOrchestration {
            request: Box::new(request),
        })
        .unwrap();
    assert!(reply.error.is_none(), "TE13_RAW_FINISH {reply:?}");
    assert!(
        matches!(f.projection(&run).tasks[0].state, TaskState::Terminal(_)),
        "TE13_PUBLIC_DISPATCH_FUSED"
    );
}

#[test]
fn te13_commit_returns_its_revision_and_affected_scope() {
    let r = c5_fix10_run();
    let revision = grant_test_revision(&r.core);
    let prepared=r.editor.raw("prepare_edit_plan",serde_json::json!({"expected_revision":revision,"operations":[{"op":"move_marker","marker":1,"to":2}]}));
    assert_eq!(
        prepared.is_error,
        Some(false),
        "TE13_SCOPE_PREPARED {prepared:?}"
    );
    let plan = prepared.structured_content.unwrap()["plan_id"].clone();
    let committed = r.editor.wire(
        "commit_edit_plan",
        serde_json::json!({"expected_revision":revision,"plan_id":plan}),
    );
    assert_eq!(
        committed.is_error,
        Some(false),
        "TE13_SCOPE_COMMIT {committed:?}"
    );
    let outcome = &committed.structured_content.as_ref().unwrap()["outcome"];
    assert_eq!(
        outcome["revision"],
        serde_json::json!(grant_test_revision(&r.core)),
        "TE13_COMMIT_REVISION"
    );
    assert_eq!(
        outcome["affected_scope"],
        serde_json::json!({"operations":["move_marker"],"paths":["/markers/0/position"]}),
        "TE13_COMMIT_SCOPE"
    );
    let rejected = r.editor.wire(
        "commit_edit_plan",
        serde_json::json!({"expected_revision":revision,"plan_id":plan}),
    );
    assert_eq!(
        rejected.is_error,
        Some(true),
        "TE13_SCOPE_STALE_COMMIT_REFUSED"
    );
}

#[test]
fn te13_revision_change_refuses_without_stranding_the_task() {
    let r = c5_fix10_run();
    let stale = te13_control(&r.editor, &r.child, &r.core);
    let undo = r.editor.wire(
        "undo_task_edit",
        serde_json::json!({"expected_revision":grant_test_revision(&r.core)}),
    );
    assert_eq!(undo.is_error, Some(false));
    let reply = r.editor.wire("finish_task", stale.clone());
    let projection = r.f.projection(&r.run);
    assert!(
        projection.tasks[1].progress.completion.is_none(),
        "TE13_REVISION_BOUND_TO_INTENT {reply:?}"
    );
    assert!(
        reply.structured_content.unwrap()["outcome"]["receipt"]["decision"]
            .to_string()
            .contains("refused"),
        "TE13_FIX1_STALE_RECEIPT"
    );
    let endpoint = r.f.host.owner.upgrade().unwrap().lock().unwrap().hooks.runs[&r.run].tasks
        [&r.child]
        .endpoint();
    assert!(
        !endpoint.is_closed_for_fixture(),
        "TE13_FIX1_REFUSED_FENCE_OPEN"
    );
    let redo = r.editor.wire(
        "redo_task_edit",
        serde_json::json!({"expected_revision":grant_test_revision(&r.core)}),
    );
    assert_eq!(
        redo.is_error,
        Some(false),
        "TE13_FIX1_REFUSED_REDO {redo:?}"
    );
    let undo = r.editor.wire(
        "undo_task_edit",
        serde_json::json!({"expected_revision":grant_test_revision(&r.core)}),
    );
    assert_eq!(
        undo.is_error,
        Some(false),
        "TE13_FIX1_REFUSED_UNDO {undo:?}"
    );
    let redo = r.editor.wire(
        "redo_task_edit",
        serde_json::json!({"expected_revision":grant_test_revision(&r.core)}),
    );
    assert_eq!(redo.is_error, Some(false));
    let revision = grant_test_revision(&r.core);
    let prepared = r.editor.raw("prepare_edit_plan", serde_json::json!({"expected_revision":revision,"operations":[{"op":"move_marker","marker":1,"to":3}]}));
    assert_eq!(prepared.is_error, Some(false), "TE13_FIX1_REFUSED_PREPARE");
    let committed = r.editor.raw("commit_edit_plan",serde_json::json!({"expected_revision":revision,"plan_id":prepared.structured_content.unwrap()["plan_id"]}));
    assert_eq!(
        committed.is_error,
        Some(false),
        "TE13_FIX1_REFUSED_EDIT {committed:?}"
    );
    let fresh = te13_control(&r.editor, &r.child, &r.core);
    assert_ne!(fresh["id"], stale["id"], "TE13_FIX1_NEW_FINISH_ID");
    let reply = r.editor.wire("finish_task", fresh);
    assert_eq!(
        reply.is_error,
        Some(false),
        "TE13_FIX1_FRESH_FINISH {reply:?}"
    );
    assert!(
        r.f.projection(&r.run).tasks[1]
            .progress
            .completion
            .is_some(),
        "TE13_FIX1_NEW_INTENT"
    );
}

#[test]
fn te13_finish_preserves_exact_task_and_person_redo_histories() {
    let C5Fix2History {
        f,
        run,
        child,
        expected,
        manual,
        ..
    } = c5_fix2_editor(Fixture::new());
    let mcp = C4Mcp::new(&f, &run, &child);
    let core = te13_core(&f, &run, &child);
    let control = te13_control(&mcp, &child, &core);
    let reply = mcp.wire("finish_task", control);
    assert_eq!(reply.is_error, Some(false), "TE13_HISTORY_FINISH {reply:?}");
    assert_eq!(
        core.export_history().unwrap(),
        expected,
        "TE13_EXACT_TASK_HISTORY"
    );
    let host = f.host.owner.upgrade().unwrap();
    let host = host.lock().unwrap();
    let state = host
        .owner()
        .unwrap()
        .orchestration_projection(&run)
        .unwrap()
        .unwrap();
    let saved = host.saved_task_history(&state, &child).unwrap().unwrap();
    assert!(
        saved.frozen && saved.frozen_result.is_some(),
        "TE13_EXACT_DURABLE_TASK_HISTORY"
    );
    let checkpoint = host
        .owner()
        .unwrap()
        .store()
        .unwrap()
        .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(
            &saved.source.checkpoint,
        )
        .unwrap();
    let persisted = host
        .owner()
        .unwrap()
        .store()
        .unwrap()
        .resolve_history(&checkpoint.history_ref)
        .unwrap();
    assert_eq!(*persisted, expected, "TE13_EXACT_DURABLE_TASK_HISTORY");
    assert_eq!(
        host.core().export_history().unwrap(),
        manual,
        "TE13_EXACT_MANUAL_REDO_HISTORY"
    );
    host.core().request(kinewright_core::Command::Redo).unwrap();
    assert!(
        host.core()
            .export_history()
            .unwrap()
            .document
            .markers
            .iter()
            .any(|marker| marker.id == kinewright_core::MarkerId(51)),
        "TE13_PERSON_REDO_STILL_WORKS"
    );
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
fn te13_finish_cannot_spend_the_terminal_reserve() {
    let f = te13_candidate_fixture(c5_fix1_fixture_limits(
        kinewright_project::storage_budget::StorageLimits {
            objects: 512,
            ..Default::default()
        },
    ));
    let (run, execution, mcp) = te13_director(&f);
    let core = te13_core(&f, &run, &execution.task);
    let host = f.host.owner.upgrade().unwrap();
    let before = core.export_history().unwrap();
    let producer = f.producer(&run);
    let mut closed = false;
    for _ in 0..128 {
        let mut owner = host.lock().unwrap();
        let state = owner.spending_scope(&producer, &run).unwrap();
        match owner.publish_native(&producer, state, &execution, vec![]) {
            Ok(()) => {}
            Err(error) => {
                assert!(
                    error.to_string().contains("Run limit"),
                    "TE13_RESERVE_PRESSURE {error}"
                );
                closed = true;
                break;
            }
        }
    }
    assert!(closed, "TE13_RESERVE_PRESSURE_PRECONDITION");
    let control = te13_control(&mcp, &execution.task, &core);
    assert!(
        control["id"].is_object(),
        "TE13_FRESH_CONTROL_AT_RESERVE {control}"
    );
    let reply = mcp.wire("finish_task", control);
    assert_eq!(
        reply.is_error,
        Some(true),
        "TE13_ORDINARY_INTENT_CANNOT_USE_RESERVE {reply:?}"
    );
    assert!(
        f.projection(&run).tasks[0].progress.completion.is_none(),
        "TE13_NO_UNADMITTED_INTENT"
    );
    assert_eq!(
        core.export_history().unwrap(),
        before,
        "TE13_RESERVE_NO_FREEZE_EFFECT"
    );
    let stop = f.control(&f.editor, &run, RunAction::Stop);
    ok(
        f.editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(stop),
            })
            .unwrap(),
        "TE13_RESERVED_STOP_STILL_WORKS",
    );
}

#[test]
fn te13_finish_crash_child() {
    let Ok(handoff) = std::env::var("KINEWRIGHT_TE13_HANDOFF") else {
        return;
    };
    let phase = std::env::var("KINEWRIGHT_TE13_PHASE").unwrap();
    if phase == "published-child" {
        let r = c5_fix10_run();
        let revision = grant_test_revision(&r.core);
        let prepared = r.editor.raw("prepare_edit_plan", serde_json::json!({"expected_revision":revision,"operations":[{"op":"move_marker","marker":1,"to":2}]}));
        assert_eq!(prepared.is_error, Some(false));
        let committed = r.editor.raw("commit_edit_plan",serde_json::json!({"expected_revision":revision,"plan_id":prepared.structured_content.unwrap()["plan_id"]}));
        assert_eq!(committed.is_error, Some(false));
        let undone = r.editor.wire(
            "undo_task_edit",
            serde_json::json!({"expected_revision":grant_test_revision(&r.core)}),
        );
        assert_eq!(undone.is_error, Some(false));
        let result = c5_fix10_freeze(&r.editor, &r.child);
        r.editor.accepted(
            "publish_result",
            OrchestrationCommand::PublishResult(Box::new(PublishResult {
                task: r.child.clone(),
                result,
            })),
        );
        std::fs::write(handoff,serde_json::to_vec(&serde_json::json!({"root":r.f.temp.root(),"run":r.run,"history":r.core.export_history().unwrap()})).unwrap()).unwrap();
        std::process::exit(86);
    }
    let mut f = te13_fixture();
    let mut scoped = Document::default();
    for id in [1, 2] {
        if let kinewright_core::Operation::AddMarker { marker } = history_marker(id) {
            scoped.markers.push(marker);
        }
    }
    f.brief.scope =
        crate::orchestration::policy::director_scope(&scoped, f.brief.scope.base.clone()).unwrap();
    let (run, execution, mcp) = te13_director(&f);
    let core = te13_core(&f, &run, &execution.task);
    for id in [1, 2] {
        core.request(kinewright_core::Command::Do(history_marker(id)))
            .unwrap();
    }
    core.request(kinewright_core::Command::Undo).unwrap();
    let control = te13_control(&mcp, &execution.task, &core);
    if phase == "before-freeze" {
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .hooks
            .completion_before_freeze = true;
    } else {
        te13_cut(
            &f,
            if phase == "freeze" {
                CompletionStep::Freeze
            } else {
                CompletionStep::Publication
            },
        );
    }
    let reply = mcp.wire("finish_task", control.clone());
    assert_eq!(reply.is_error, Some(false), "TE13_CRASH_ACK");
    std::fs::write(handoff,serde_json::to_vec(&serde_json::json!({"root":f.temp.root(),"run":run,"execution":execution,"control":control,"reply":reply.structured_content.unwrap(),"history":core.export_history().unwrap()})).unwrap()).unwrap();
    std::process::exit(86);
}

#[test]
fn te13_restart_retains_completion_and_never_revives_authority() {
    for phase in ["freeze", "publication"] {
        let temp = kinewright_media::test_support::TempDirectory::new_in(
            &kinewright_media::test_support::workspace_target_dir(),
            "te13-crash",
        );
        let handoff = temp.path("handoff.json");
        let log = temp.path("child.log");
        let output = std::fs::File::create(&log).unwrap();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "writer_host::hooks_tests::tests::te13_finish_crash_child",
                "--nocapture",
            ])
            .env("KINEWRIGHT_TE13_HANDOFF", &handoff)
            .env("KINEWRIGHT_TE13_PHASE", phase)
            .stdout(output.try_clone().unwrap())
            .stderr(output)
            .status()
            .unwrap();
        assert_eq!(
            child.code(),
            Some(86),
            "TE13_REAL_CRASH {phase}: {}",
            std::fs::read_to_string(&log).unwrap()
        );
        let data: serde_json::Value =
            serde_json::from_slice(&std::fs::read(handoff).unwrap()).unwrap();
        let root = std::path::PathBuf::from(data["root"].as_str().unwrap());
        assert!(
            root.starts_with(kinewright_media::test_support::workspace_target_dir()),
            "TE13_OWN_ROOT"
        );
        let execution: ExecutionIdentity =
            serde_json::from_value(data["execution"].clone()).unwrap();
        let control = data["control"].clone();
        let host = crash_host(&root);
        let owner = host.owner.upgrade().unwrap();
        let producer = owner
            .lock()
            .unwrap()
            .restore_native_run(&execution.run)
            .unwrap()
            .0;
        let generation = owner
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .metadata_generation()
            .unwrap();
        let (outcome, status) = te13_read(&host.client(), te13_query(&execution, &control));
        assert_eq!(
            outcome, data["reply"]["outcome"],
            "TE13_RESTART_ORIGINAL_OUTCOME {phase}"
        );
        assert_eq!(
            serde_json::to_value(&status.freeze_receipt).unwrap(),
            data["reply"]["completion_status"]["freeze_receipt"],
            "TE13_RESTART_ORIGINAL_FREEZE"
        );
        if phase == "freeze" {
            assert_eq!(
                status.state,
                super::super::CompletionState::Blocked,
                "TE13_RESTART_BLOCKED_NO_AUTHORITY"
            );
            assert_eq!(
                status.reason.as_deref(),
                Some("task_authority_unavailable"),
                "TE13_RESTART_MISSING_AUTHORITY_REASON"
            );
            assert!(
                status.publication_receipt.is_none(),
                "TE13_RESTART_NO_INVENTED_PUBLICATION"
            );
            assert!(
                owner
                    .lock()
                    .unwrap()
                    .reserve_native_effect(&producer, &execution.run)
                    .unwrap()
                    .is_none(),
                "TE13_RESTART_NO_SEND"
            );
            let page = host
                .client()
                .request(HostCommand::ObserveRun {
                    run: execution.run.clone(),
                    after_sequence: 0,
                })
                .unwrap()
                .run_view
                .unwrap();
            assert!(
                page.runtime_status
                    .as_deref()
                    .is_some_and(|reason| reason.contains("completion_recovery_requires_person")),
                "TE13_RESTART_VISIBLE_BLOCK"
            );
        } else {
            assert_eq!(
                status.state,
                super::super::CompletionState::Completed,
                "TE13_RESTART_PUBLISHED_IS_COMPLETE"
            );
            assert_eq!(
                serde_json::to_value(&status.result).unwrap(),
                data["reply"]["completion_status"]["result"],
                "TE13_RESTART_ORIGINAL_RESULT"
            );
        }
        assert_eq!(
            owner
                .lock()
                .unwrap()
                .owner()
                .unwrap()
                .metadata_generation()
                .unwrap(),
            generation,
            "TE13_RESTART_READ_ONLY"
        );
        drop(owner);
        drop(host);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn te13_before_commit_merge_reopen_has_a_production_person_block() {
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "te13-merge-crash",
    );
    let handoff = temp.path("handoff.json");
    let log = temp.path("child.log");
    let output = std::fs::File::create(&log).unwrap();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "writer_host::hooks_tests::tests::s4b_c4_fix1_merge_crash_child",
            "--nocapture",
        ])
        .env("KINEWRIGHT_C4_MERGE_CUT", "before_candidate_commit")
        .env("KINEWRIGHT_C4_MERGE_HANDOFF", &handoff)
        .stdout(output.try_clone().unwrap())
        .stderr(output)
        .status()
        .unwrap();
    assert_eq!(
        child.code(),
        Some(86),
        "TE13_MERGE_CRASH {}",
        std::fs::read_to_string(&log).unwrap()
    );
    let data: serde_json::Value = serde_json::from_slice(&std::fs::read(handoff).unwrap()).unwrap();
    let root = std::path::PathBuf::from(data["root"].as_str().unwrap());
    assert!(
        root.starts_with(kinewright_media::test_support::workspace_target_dir()),
        "TE13_OWN_MERGE_ROOT"
    );
    let run: RunId = serde_json::from_value(data["run"].clone()).unwrap();
    let child: TaskId = serde_json::from_value(data["child"].clone()).unwrap();
    let host = crash_host(&root);
    let owner = host.owner.upgrade().unwrap();
    let producer = owner.lock().unwrap().restore_native_run(&run).unwrap().0;
    let before = owner
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .metadata_generation()
        .unwrap();
    let page = host
        .client()
        .request(HostCommand::ObserveRun {
            run: run.clone(),
            after_sequence: 0,
        })
        .unwrap()
        .run_view
        .unwrap();
    assert!(
        page.runtime_status
            .as_deref()
            .is_some_and(|reason| reason.contains("merge_recovery_requires_person")),
        "TE13_RECOVERED_MERGE_VISIBLE"
    );
    assert!(
        !page.allowed_actions.contains(&RunActionKind::Continue),
        "TE13_RECOVERED_MERGE_NOT_CONTINUABLE"
    );
    assert!(
        owner
            .lock()
            .unwrap()
            .reserve_native_effect(&producer, &run)
            .unwrap()
            .is_none(),
        "TE13_RECOVERED_MERGE_NO_SEND"
    );
    let locked = owner.lock().unwrap();
    let history = locked
        .saved_task_history(&page.projection, &child)
        .unwrap()
        .unwrap();
    assert!(
        history.frozen && history.frozen_result.is_some(),
        "TE13_RECOVERED_MERGE_WORK_RETAINED"
    );
    assert_eq!(
        locked
            .owner()
            .unwrap()
            .manual_transition_receipt(history.merge_envelope.as_ref().unwrap())
            .unwrap(),
        kinewright_project::receipts::ReceiptLookup::Absent,
        "TE13_RECOVERED_MERGE_NEVER_INVENTED"
    );
    assert_eq!(
        locked.owner().unwrap().metadata_generation().unwrap(),
        before,
        "TE13_RECOVERED_MERGE_READ_ONLY"
    );
    drop(locked);
    drop(owner);
    drop(host);
    std::fs::remove_dir_all(root).unwrap();
}
