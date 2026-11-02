/// The effect pump's 30 s clock observation: one more accounting record.
fn c5_fix10_tick(f: &Fixture, run: &RunId, attempt: &ExecutionIdentity) {
    let host = f.host.owner.upgrade().unwrap();
    let state = f.projection(run);
    let mut owner = host.lock().unwrap();
    let producer = owner.run_producer(run).unwrap();
    let before = owner.current_accounting_reference(&state).unwrap();
    owner
        .change_spending(
            &producer,
            attempt,
            crate::writer_host::native_invocation::SpendingChange::Clock,
            crate::protocol::limits::usage::ClockObservation {
                unix_ms: crate::writer_host::spending::unix_ms().unwrap(),
                monotonic_delta_ms: 30_000,
                trusted: true,
            },
        )
        .unwrap();
    drop(owner);
    let state = f.projection(run);
    let after = host
        .lock()
        .unwrap()
        .current_accounting_reference(&state)
        .unwrap();
    assert_ne!(before, after, "C5_FIX10_TICK_WROTE_NO_RECORD");
}

struct C5Fix10Run {
    f: Fixture,
    run: RunId,
    parent: ExecutionIdentity,
    child: TaskId,
    attempt: ExecutionIdentity,
    director: C4Mcp,
    editor: C4Mcp,
    core: Core,
}
fn c5_fix10_run() -> C5Fix10Run {
    let mut f = Fixture::new();
    let mut scoped = Document::default();
    if let kinewright_core::Operation::AddMarker { marker } = history_marker(1) {
        scoped.markers.push(marker);
    }
    f.brief.scope =
        crate::orchestration::policy::director_scope(&scoped, f.brief.scope.base.clone()).unwrap();
    let (run, parent, director) = c4_director(&f);
    let mut brief = f.brief.clone();
    brief.role = Role::Editor;
    director.accepted(
        "assign_task",
        OrchestrationCommand::AssignTask(Box::new(AssignTask {
            run: run.clone(),
            parent: parent.task.clone(),
            brief,
        })),
    );
    let child = f.projection(&run).tasks[1].id.clone();
    let (attempt, client, endpoint, core) = c4_reserve_task(&f, &run, &child);
    history_turn(&f, &attempt, &client, &endpoint, 1, true);
    let editor = C4Mcp::new(&f, &run, &child);
    let revision = grant_test_revision(&core);
    let prepared = editor.raw(
        "prepare_edit_plan",
        serde_json::json!({"expected_revision":revision,"operations":[{"op":"add_marker","marker":{"id":1,"position":1,"label":"marker 1","color_token":0}}]}),
    );
    assert_eq!(
        prepared.is_error,
        Some(false),
        "C5_FIX10_PREPARE {prepared:?}"
    );
    let committed = editor.raw(
        "commit_edit_plan",
        serde_json::json!({"expected_revision":revision,"plan_id":prepared.structured_content.unwrap()["plan_id"]}),
    );
    assert_eq!(
        committed.is_error,
        Some(false),
        "C5_FIX10_COMMIT {committed:?}"
    );
    C5Fix10Run {
        f,
        run,
        parent,
        child,
        attempt,
        director,
        editor,
        core,
    }
}
fn c5_fix10_text(reply: &rmcp::model::CallToolResult) -> String {
    serde_json::to_string(&reply.content)
        .unwrap_or_default()
        .chars()
        .take(400)
        .collect()
}
fn c5_fix10_freeze(mcp: &C4Mcp, child: &TaskId) -> TaskResult {
    let frozen = mcp.invoke(
        "freeze_task",
        mcp.control(OrchestrationCommand::FreezeTask(child.clone())),
    );
    assert_eq!(
        frozen.is_error,
        Some(false),
        "C5_FIX10_FREEZE {}",
        c5_fix10_text(&frozen)
    );
    serde_json::from_value(frozen.structured_content.unwrap()["result"].clone()).unwrap()
}
fn c5_fix10_publish(mcp: &C4Mcp, child: &TaskId, result: TaskResult) -> serde_json::Value {
    mcp.control(OrchestrationCommand::PublishResult(Box::new(
        PublishResult {
            task: child.clone(),
            result,
        },
    )))
}
fn c5_fix10_counts(f: &Fixture, run: &RunId) -> (usize, usize, usize, usize) {
    let state = f.projection(run);
    let sends = f
        .editor
        .request(HostCommand::ObserveRun {
            run: run.clone(),
            after_sequence: 0,
        })
        .unwrap()
        .run_view
        .unwrap()
        .accounting
        .map_or(0, |accounting| {
            accounting.tasks.iter().map(|task| task.sends).sum()
        });
    (
        state.results.len(),
        state.deliveries.len(),
        state.effects.len(),
        sends,
    )
}
fn c5_fix10_stored_result(f: &Fixture, reference: &ObjectRef) -> TaskResult {
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
        .read_run_record(reference)
        .unwrap();
    let OrchestrationPayload::Result(result) = record.payload else {
        panic!("C5_FIX10_NOT_A_RESULT");
    };
    *result
}

/// Invocation 9's Editor sequence through the product's own task MCP
/// endpoint, with the pump's clock observation between freeze and publish.
/// At c129d5f5 and f91a6110 the publish was refused.
#[test]
fn s4b_c5_fix10_clock_tick_between_freeze_and_publish_is_accepted() {
    let r = c5_fix10_run();
    let result = c5_fix10_freeze(&r.editor, &r.child);
    c5_fix10_tick(&r.f, &r.run, &r.attempt);
    let published = r.editor.invoke(
        "publish_result",
        c5_fix10_publish(&r.editor, &r.child, result.clone()),
    );
    assert_eq!(
        published.is_error,
        Some(false),
        "C5_FIX10_TICK_REFUSED_PUBLISH {}",
        c5_fix10_text(&published)
    );
    let state = r.f.projection(&r.run);
    let stored = c5_fix10_stored_result(&r.f, state.results.last().unwrap());
    let current =
        r.f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .current_accounting_reference(&state)
            .unwrap();
    assert_eq!(stored.usage_ref, current, "C5_FIX10_USAGE_BOUND_AT_PUBLISH");
    // The Director's result wake expands `usage_ref`: the record bound at
    // publish is a successor of the freeze-time one, so it reports no less.
    let accounting = |reference: &ObjectRef| {
        let record =
            r.f.host
                .owner
                .upgrade()
                .unwrap()
                .lock()
                .unwrap()
                .owner()
                .unwrap()
                .store()
                .unwrap()
                .read_run_record(reference)
                .unwrap();
        let OrchestrationPayload::Accounting(accounting) = record.payload else {
            panic!("C5_FIX10_NOT_ACCOUNTING");
        };
        *accounting
    };
    let (frozen, bound) = (accounting(&result.usage_ref), accounting(&stored.usage_ref));
    assert!(bound.revision > frozen.revision, "C5_FIX10_USAGE_NOT_NEWER");
    for task in &frozen.tasks {
        let later = bound
            .tasks
            .iter()
            .find(|t| t.task == task.task)
            .expect("C5_FIX10_USAGE_TASK_LOST");
        assert!(
            later.sends.len() >= task.sends.len()
                && later.usage.len() >= task.usage.len()
                && later.totals.token_lower_bound >= task.totals.token_lower_bound,
            "C5_FIX10_USAGE_LESS_COMPLETE"
        );
    }
}

/// Publish, tick, then re-send the identical control and a fresh one: the
/// same publication, with no second result, delivery, wake or charge.
#[test]
fn s4b_c5_fix10_replay_after_ticks_returns_the_same_publication() {
    let r = c5_fix10_run();
    let result = c5_fix10_freeze(&r.director, &r.child);
    let control = c5_fix10_publish(&r.director, &r.child, result.clone());
    let first = r.director.invoke("publish_result", control.clone());
    assert_eq!(
        first.is_error,
        Some(false),
        "C5_FIX10_FIRST {}",
        c5_fix10_text(&first)
    );
    let published = r.f.projection(&r.run).results.last().unwrap().clone();
    let counts = c5_fix10_counts(&r.f, &r.run);
    for round in 0..2 {
        c5_fix10_tick(&r.f, &r.run, &r.attempt);
        let again = r.director.invoke("publish_result", control.clone());
        assert_eq!(
            again.is_error,
            Some(false),
            "C5_FIX10_SAME_CONTROL_REFUSED round={round} {}",
            c5_fix10_text(&again)
        );
        let again = again.structured_content.unwrap();
        assert_eq!(
            again["already_published"], true,
            "C5_FIX10_SAME_CONTROL_NEW"
        );
        assert_eq!(
            again["result"],
            serde_json::to_value(&published).unwrap(),
            "C5_FIX10_SAME_CONTROL_OTHER_RESULT"
        );
        assert!(
            !again["receipt"].is_null(),
            "C5_FIX10_SAME_CONTROL_NO_RECEIPT"
        );
        let fresh = r.director.invoke(
            "publish_result",
            c5_fix10_publish(&r.director, &r.child, result.clone()),
        );
        assert_eq!(
            fresh.is_error,
            Some(false),
            "C5_FIX10_FRESH_CONTROL_REFUSED round={round} {}",
            c5_fix10_text(&fresh)
        );
        assert_eq!(
            fresh.structured_content.unwrap()["result"],
            serde_json::to_value(&published).unwrap(),
            "C5_FIX10_FRESH_CONTROL_OTHER_RESULT"
        );
        assert_eq!(
            c5_fix10_counts(&r.f, &r.run),
            counts,
            "C5_FIX10_REPLAY_WROTE"
        );
    }
    assert_eq!(r.f.projection(&r.run).results.last(), Some(&published));
}

/// A result whose delta or head differs from the frozen task is still
/// refused, whatever `usage_ref` it carries.
#[test]
fn s4b_c5_fix10_changed_delta_after_freeze_is_refused() {
    let r = c5_fix10_run();
    let result = c5_fix10_freeze(&r.director, &r.child);
    let other =
        r.f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .owner_mut()
            .unwrap()
            .admit_host_grant(b"[]")
            .unwrap();
    let mut changed = result.clone();
    changed.delta_ref = Some(other);
    let mut moved = result.clone();
    moved.head.clone_from(&moved.base);
    for (label, candidate) in [("delta", changed), ("head", moved)] {
        let refused = r.director.invoke(
            "publish_result",
            c5_fix10_publish(&r.director, &r.child, candidate),
        );
        assert_eq!(
            refused.is_error,
            Some(true),
            "C5_FIX10_CHANGED_{label}_ACCEPTED"
        );
        assert!(
            c5_fix10_text(&refused).contains("result differs from the frozen task"),
            "C5_FIX10_CHANGED_{label}_WRONG_REFUSAL {}",
            c5_fix10_text(&refused)
        );
    }
    assert!(
        r.f.projection(&r.run).results.is_empty(),
        "C5_FIX10_CHANGED_PUBLISHED"
    );
    let published = r.director.invoke(
        "publish_result",
        c5_fix10_publish(&r.director, &r.child, result),
    );
    assert_eq!(
        published.is_error,
        Some(false),
        "C5_FIX10_EXACT_REFUSED {}",
        c5_fix10_text(&published)
    );
}

/// The Editor's process ends its turn: the state `native_completed` leaves.
fn c5_fix10_retire(f: &Fixture, run: &RunId, attempt: &ExecutionIdentity) {
    let producer = f.producer_client(run);
    let mut state = f.projection(run);
    state
        .attempts
        .iter_mut()
        .find(|a| a.identity == *attempt)
        .unwrap()
        .state = AttemptState::Retired(
        crate::orchestration::orchestration_records::AttemptOutcome::Completed,
    );
    let task = state
        .tasks
        .iter_mut()
        .find(|t| t.id == attempt.task)
        .unwrap();
    task.progress.capacity = DependencyReservation::Free;
    task.progress.turn_ended = true;
    if matches!(task.state, TaskState::Preparing | TaskState::Running(_)) {
        task.state = TaskState::Queued;
    }
    crate::orchestration::tasks::refresh_cleanup(&mut state);
    publish_projection(f, &producer, run, state, None);
}

/// A host restart between freeze and publish: the publish admits against the
/// reloaded frozen task.
#[test]
fn s4b_c5_fix10_restart_between_freeze_and_publish_admits() {
    let r = c5_fix10_run();
    let claim_root = r.f.temp.path("c5-fix10-machine");
    let claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &claim_root,
        r.parent.project.as_str(),
        r.run.0.as_str(),
    )
    .unwrap();
    c5_fix10_retire(&r.f, &r.run, &r.attempt);
    let result = c5_fix10_freeze(&r.director, &r.child);
    let C5Fix10Run {
        f,
        run,
        parent,
        child,
        director,
        editor,
        core,
        ..
    } = r;
    drop((director, editor, core, claim));
    let f = c4_restart(f, &claim_root);
    let (_, _, _, _parent_core) = c4_reserve_task(&f, &run, &parent.task);
    let director = C4Mcp::new(&f, &run, &parent.task);
    let published = director.invoke(
        "publish_result",
        c5_fix10_publish(&director, &child, result),
    );
    assert_eq!(
        published.is_error,
        Some(false),
        "C5_FIX10_RESTART_REFUSED {}",
        c5_fix10_text(&published)
    );
    assert_eq!(
        f.projection(&run).results.len(),
        1,
        "C5_FIX10_RESTART_RESULTS"
    );
}

// ---- Parts 2 to 4: provider failures, silent stalls, refusal codes ----

/// A charged Director attempt, as the C3 raw-boundary witness sets it up.
fn c5_fix10_charged(
    f: &Fixture,
) -> (
    RunId,
    ExecutionIdentity,
    HostClient,
    HostAuth,
    std::path::PathBuf,
    crate::orchestration::adapters::AuthorizedInvocation,
) {
    let (run, execution, actor) = reserve(f);
    let admitted = native_admitted(f, &execution);
    let (profile, _, scratch) = native_profile(f, &admitted);
    let charge = native_charge(f, &admitted, profile);
    let producer = f.producer(&run);
    (run, execution, actor, producer, scratch, charge)
}
fn c5_fix10_raw(
    f: &Fixture,
    producer: &HostAuth,
    execution: &ExecutionIdentity,
    raw: &serde_json::Value,
    ordinal: u64,
) -> Result<(), HostError> {
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .ingest_native_raw(producer, execution, raw, ordinal, false)
}
/// Codex 0.166 `McpToolCallItem` for a refused product call.
fn c5_fix10_failed_item(id: &str, text: &str) -> serde_json::Value {
    serde_json::json!({"type":"item.completed","item":{
        "id":id,"type":"mcp_tool_call","server":"kinewright","tool":"invoke_capability",
        "arguments":{},"result":{"content":[{"type":"text","text":text}],"structured_content":null},
        "error":null,"status":"failed"}})
}
fn c5_fix10_wait_reason(f: &Fixture, run: &RunId) -> Option<String> {
    let RunWait::Provider(reason) = f.projection(run).wait else {
        return None;
    };
    let host = f.host.owner.upgrade().unwrap();
    let owner = host.lock().unwrap();
    Some(
        String::from_utf8_lossy(
            &owner
                .owner()
                .unwrap()
                .store()
                .unwrap()
                .read_evidence(&reason.reason)
                .unwrap(),
        )
        .into_owned(),
    )
}

/// Invocation 9: two different refused product calls arrive as Codex's real
/// `status: failed, error: null` items. They are not provider failures, so
/// nothing is hashed and the task is not paused. At c129d5f5 both hashed
/// `null`, matched, and the second paused the task as Replan.
#[test]
fn s4b_c5_fix10_failed_tool_items_are_not_repairs() {
    let f = Fixture::new();
    let (run, execution, _, producer, scratch, _charge) = c5_fix10_charged(&f);
    for (ordinal, (id, text)) in [
        (
            "item_1",
            "orchestration_refused: result differs from the frozen task",
        ),
        ("item_2", "orchestration_refused: task is not frozen"),
    ]
    .into_iter()
    .enumerate()
    {
        let ingested = c5_fix10_raw(
            &f,
            &producer,
            &execution,
            &c5_fix10_failed_item(id, text),
            ordinal as u64 + 1,
        );
        assert!(
            ingested.is_ok(),
            "C5_FIX10_FAILED_ITEM_REFUSED {id} {ingested:?}"
        );
    }
    let record = spending_record(&f, &run);
    let task = &record.tasks[0];
    assert!(task.repairs.is_empty(), "C5_FIX10_ITEM_COUNTED_AS_REPAIR");
    assert_eq!(task.repeated_failures, 0, "C5_FIX10_ITEM_REPEATED");
    assert_eq!(task.last_failure, None, "C5_FIX10_ITEM_HASHED");
    assert_eq!(task.pause, None, "C5_FIX10_ITEM_PAUSED");
    std::fs::remove_dir_all(scratch).unwrap();
}

/// Two identical classified `turn.failed` events still pause the task as
/// Replan, and the reader's refusal now shows it: the run is Paused with the
/// fixed Replan reason, and nothing more is charged.
#[test]
fn s4b_c5_fix10_identical_turn_failures_pause_visibly() {
    let f = Fixture::new();
    let (run, execution, _, producer, scratch, _charge) = c5_fix10_charged(&f);
    let failed = serde_json::json!({"type":"turn.failed","error":{"message":"same failure"}});
    c5_fix10_raw(&f, &producer, &execution, &failed, 1).unwrap();
    assert!(
        c5_fix10_raw(&f, &producer, &execution, &failed, 2).is_err(),
        "C5_FIX10_REPEAT_NOT_REPLAN"
    );
    let record = spending_record(&f, &run);
    assert_eq!(
        record.tasks[0].pause,
        Some(kinewright_project::orchestration_accounting::AccountingRefusal::Replan)
    );
    let sends = record.tasks[0].sends.len();
    // The reader refused that event and killed the provider; its one
    // publication of why:
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .record_stream_refusal(
            &producer,
            &execution,
            crate::native_launcher::STREAM_ACCOUNTING_REFUSED,
        )
        .unwrap();
    let state = f.projection(&run);
    assert_eq!(state.phase, RunPhase::Paused, "C5_FIX10_REPLAN_NOT_PAUSED");
    assert_eq!(
        c5_fix10_wait_reason(&f, &run).as_deref(),
        Some(super::super::studio_runtime::REPLAN_PAUSE),
        "C5_FIX10_REPLAN_REASON"
    );
    assert!(!crate::orchestration::machine::silently_stalled(&state));
    assert_eq!(
        spending_record(&f, &run).tasks[0].sends.len(),
        sends,
        "C5_FIX10_REPLAN_CHARGED"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

/// A counted failure without a code or message is never hashed and never a
/// repeat, so two of them do not pause the task; they still spend repair
/// rounds, and the third is refused by the round bound.
#[test]
fn s4b_c5_fix10_unclassified_failures_never_repeat() {
    use kinewright_project::orchestration_accounting::{AccountingRefusal, UNCLASSIFIED_FAILURE};
    let f = Fixture::new();
    let (run, execution, _, producer, scratch, _charge) = c5_fix10_charged(&f);
    for (ordinal, raw) in [
        serde_json::json!({"type":"turn.failed","error":null}),
        serde_json::json!({"type":"turn.failed","error":{"message":"  "}}),
    ]
    .into_iter()
    .enumerate()
    {
        c5_fix10_raw(&f, &producer, &execution, &raw, ordinal as u64 + 1).unwrap();
    }
    let record = spending_record(&f, &run);
    let task = &record.tasks[0];
    assert_eq!(task.last_failure.as_deref(), Some(UNCLASSIFIED_FAILURE));
    assert_eq!(task.repeated_failures, 1, "C5_FIX10_UNCLASSIFIED_REPEATED");
    assert_eq!(task.repairs[0].rounds, 2);
    assert_eq!(task.pause, None, "C5_FIX10_UNCLASSIFIED_PAUSED");
    assert!(
        c5_fix10_raw(
            &f,
            &producer,
            &execution,
            &serde_json::json!({"type":"error"}),
            3
        )
        .is_err()
    );
    assert_eq!(
        spending_record(&f, &run).tasks[0].pause,
        Some(AccountingRefusal::Replan),
        "C5_FIX10_ROUND_BOUND"
    );
    // A top-level `error` with a message is classified by kind and message.
    assert_eq!(
        crate::writer_host::native_invocation::provider_failure(
            "error",
            &serde_json::json!({"type":"error","message":"m"})
        ),
        Some(kinewright_media::sha256_bytes(b"error\0message\0m"))
    );
    assert_ne!(
        crate::writer_host::native_invocation::provider_failure(
            "error",
            &serde_json::json!({"message":"m"})
        ),
        crate::writer_host::native_invocation::provider_failure(
            "turn.failed",
            &serde_json::json!({"error":{"message":"m"}})
        )
    );
    assert_eq!(
        crate::writer_host::native_invocation::provider_failure(
            "item.completed",
            &c5_fix10_failed_item("i", "t")
        ),
        None
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

/// The reader-kill end state of invocation 9: the attempt retired as
/// interrupted, the task queued, its charged effect's outcome unknown. The
/// invariant witness reports it, and the pump's check shows it once as an
/// interrupted run with the fixed stall reason.
#[test]
fn s4b_c5_fix10_silent_stall_is_reported_and_surfaced() {
    use crate::orchestration::machine::{SILENT_STALL, silently_stalled};
    let f = Fixture::new();
    let (run, _, actor, producer, scratch, _charge) = c5_fix10_charged(&f);
    assert!(
        !silently_stalled(&f.projection(&run)),
        "C5_FIX10_LIVE_ATTEMPT_STALLED"
    );
    let mut state = f.projection(&run);
    for attempt in &mut state.attempts {
        attempt.state = AttemptState::Retired(
            crate::orchestration::orchestration_records::AttemptOutcome::Interrupted,
        );
    }
    state.tasks[0].state = TaskState::Queued;
    state.tasks[0].progress.capacity = DependencyReservation::Free;
    for effect in &mut state.effects {
        if matches!(effect.state, EffectState::Claimed(_)) {
            effect.state = EffectState::OutcomeUnknown;
        }
    }
    crate::orchestration::tasks::refresh_cleanup(&mut state);
    publish_projection(&f, &actor, &run, state, None);
    assert!(
        silently_stalled(&f.projection(&run)),
        "C5_FIX10_FIXTURE_NOT_STALLED"
    );
    let observe = || {
        f.editor
            .request(HostCommand::ObserveRun {
                run: run.clone(),
                after_sequence: 0,
            })
            .unwrap()
            .run_view
            .unwrap()
    };
    assert_eq!(
        observe().runtime_status.as_deref(),
        Some(SILENT_STALL),
        "C5_FIX10_STALL_UNREPORTED"
    );
    let host = f.host.owner.upgrade().unwrap();
    assert!(
        host.lock()
            .unwrap()
            .surface_silent_stall(&producer, &run)
            .unwrap()
    );
    let sequence = f.projection(&run).sequence;
    assert!(
        !host
            .lock()
            .unwrap()
            .surface_silent_stall(&producer, &run)
            .unwrap(),
        "C5_FIX10_STALL_SURFACED_TWICE"
    );
    let state = f.projection(&run);
    assert_eq!(state.sequence, sequence);
    assert_eq!(state.phase, RunPhase::Interrupted);
    assert_eq!(
        c5_fix10_wait_reason(&f, &run).as_deref(),
        Some(SILENT_STALL)
    );
    let view = observe();
    assert_eq!(view.runtime_status, None);
    assert!(view.allowed_actions.contains(&RunActionKind::Stop));
    std::fs::remove_dir_all(scratch).unwrap();
}

/// Every refused product call keeps its product tool name and a fixed code
/// in the caller's durable accounting and the run view; no refusal text.
#[test]
fn s4b_c5_fix10_refused_calls_keep_fixed_codes() {
    use kinewright_project::orchestration_accounting::{ToolRefusal, ToolRefusalCode};
    let r = c5_fix10_run();
    let stale_prepare = || {
        let stale = r.editor.raw(
            "prepare_edit_plan",
            serde_json::json!({"expected_revision":grant_test_revision(&r.core).0 + 7,"operations":[]}),
        );
        assert_eq!(
            stale.is_error,
            Some(true),
            "C5_FIX10_STALE_PREPARED {stale:?}"
        );
    };
    stale_prepare();
    let result = c5_fix10_freeze(&r.director, &r.child);
    let mut changed = result.clone();
    changed.head.clone_from(&changed.base);
    for _ in 0..2 {
        let refused = r.director.invoke(
            "publish_result",
            c5_fix10_publish(&r.director, &r.child, changed.clone()),
        );
        assert_eq!(refused.is_error, Some(true));
    }
    // A frozen task's endpoint refuses further edits.
    stale_prepare();
    let record = spending_record(&r.f, &r.run);
    let refusals = |task: &TaskId| {
        record
            .tasks
            .iter()
            .find(|t| t.task == task.0)
            .unwrap()
            .refusals
            .clone()
    };
    let refusal = |tool: &str, code, count| ToolRefusal {
        tool: tool.into(),
        code,
        count,
    };
    let director = refusals(&r.parent.task);
    assert!(
        director.contains(&refusal("publish_result", ToolRefusalCode::StaleFreeze, 2)),
        "C5_FIX10_STALE_FREEZE_CODE {director:?}"
    );
    assert_eq!(director.len(), 1, "C5_FIX10_DIRECTOR_CODES {director:?}");
    let editor = refusals(&r.child);
    assert_eq!(
        editor,
        [
            refusal("prepare_edit_plan", ToolRefusalCode::StaleRevision, 1),
            refusal("prepare_edit_plan", ToolRefusalCode::TerminalReserved, 1),
        ],
        "C5_FIX10_EDITOR_CODES"
    );
    let bytes = String::from_utf8(record.record_bytes().unwrap()).unwrap();
    for text in [
        "differs",
        "frozen task",
        "conflict",
        "task_freezing",
        "orchestration_refused",
    ] {
        assert!(!bytes.contains(text), "C5_FIX10_REFUSAL_TEXT_KEPT {text}");
    }
    let view =
        r.f.editor
            .request(HostCommand::ObserveRun {
                run: r.run.clone(),
                after_sequence: 0,
            })
            .unwrap()
            .run_view
            .unwrap();
    let shown = view
        .accounting
        .unwrap()
        .tasks
        .iter()
        .map(|task| task.refusals.len())
        .sum::<usize>();
    assert_eq!(shown, 3, "C5_FIX10_REFUSALS_NOT_IN_VIEW");
    eprintln!("C5_FIX10_REFUSAL_CODES director={director:?} editor={editor:?}");
}

/// The fixed refusal codes of the product's own refusal texts, and the
/// product tool names a refusal is kept under.
#[test]
fn s4b_c5_fix10_refusal_code_table() {
    use super::super::task_mcp::{product_tool_name, refusal_code};
    use kinewright_project::orchestration_accounting::ToolRefusalCode as Code;
    for (text, code) in [
        (
            "orchestration_refused: result differs from the frozen task",
            Code::StaleFreeze,
        ),
        ("orchestration_refused: task is not frozen", Code::NotFrozen),
        (
            "orchestration_refused: published result changed",
            Code::PublishedChanged,
        ),
        ("orchestration_refused: StaleGeneration", Code::StaleControl),
        (
            "timeline revision conflict: expected 3, actual 4",
            Code::StaleRevision,
        ),
        ("storage_backpressure: wait", Code::Storage),
        ("orchestration_refused: Sends", Code::Budget),
        ("orchestration_refused: Replan", Code::Budget),
        (
            "orchestration_refused: RetiredUnresolved",
            Code::TerminalReserved,
        ),
        ("task_freezing", Code::TerminalReserved),
        ("task_authority_expired", Code::NotAdmitted),
        ("orchestration_refused: InvalidState", Code::InvalidState),
        ("task_scope: no task control attachment", Code::NotAdmitted),
        ("orchestration_refused: ScopeConflict", Code::NotAdmitted),
        (
            "invalid_request: invalid bounded protocol value",
            Code::InvalidArguments,
        ),
        ("something new", Code::Other),
    ] {
        assert_eq!(refusal_code(text), code, "C5_FIX10_CODE {text}");
    }
    assert_eq!(
        product_tool_name("invoke_capability", Some("publish_result")),
        "publish_result"
    );
    assert_eq!(
        product_tool_name("invoke_capability", Some("add_marker")),
        "invoke_capability"
    );
    assert_eq!(
        product_tool_name("prepare_edit_plan", None),
        "prepare_edit_plan"
    );
    assert_eq!(product_tool_name("bash", None), "unknown_tool");
}

/// The stall predicate as of C5 fix 10, kept local so the reader-kill trace
/// also compiles and runs at c129d5f5 and reports the stall it left there.
fn c5_fix10_stalled(state: &RunProjection) -> bool {
    state.outcome == RunOutcome::Pending
        && !matches!(
            state.phase,
            RunPhase::Paused | RunPhase::Interrupted | RunPhase::Reviewing | RunPhase::Finished
        )
        && matches!(state.wait, RunWait::None | RunWait::Children(_))
        && state
            .attempts
            .iter()
            .all(|attempt| matches!(attempt.state, AttemptState::Retired(_)))
        && !state.effects.iter().any(|effect| {
            crate::orchestration::outbox::next_effect(state, &effect.lane)
                .is_some_and(|next| next.id == effect.id)
        })
}
/// Wait for a visible end (Paused or Interrupted with a provider reason);
/// fail if the run sits silently stalled for 5 s or nothing ends in 30 s.
fn c5_fix10_visible_end(f: &Fixture, run: &RunId) -> (RunProjection, String) {
    let bound = Instant::now() + Duration::from_secs(30);
    let mut stalled_since: Option<Instant> = None;
    loop {
        let state = f.projection(run);
        if matches!(state.phase, RunPhase::Paused | RunPhase::Interrupted)
            && let Some(reason) = c5_fix10_wait_reason(f, run)
        {
            return (state, reason);
        }
        if c5_fix10_stalled(&state) {
            let since = *stalled_since.get_or_insert_with(Instant::now);
            assert!(
                since.elapsed() < Duration::from_secs(5),
                "C5_FIX10_SILENT_STALL phase={:?} tasks={:?} attempts={:?} effects={:?}",
                state.phase,
                state.tasks.iter().map(|t| &t.state).collect::<Vec<_>>(),
                state.attempts.iter().map(|a| &a.state).collect::<Vec<_>>(),
                state.effects.iter().map(|e| &e.state).collect::<Vec<_>>()
            );
        } else {
            stalled_since = None;
        }
        assert!(Instant::now() < bound, "C5_FIX10_NO_VISIBLE_END {state:?}");
        std::thread::park_timeout(Duration::from_millis(5));
    }
}

/// The reader-kill trace, with the real guardian, sealed image and sandbox:
/// two identical classified `turn.failed` events. The host pauses the task
/// for a new plan, the reader refuses the event and kills the provider, and
/// the run ends Paused with the fixed Replan reason: one refusal, one
/// execution, no further charge. At c129d5f5 the same trace ends silently
/// stalled (`C5_FIX10_SILENT_STALL`).
#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c5_fix10_reader_kill_trace() {
    use super::super::studio_runtime::fixture::ProtocolFixtureMode;
    let (f, run, fixture) = c5_fix3_host(ProtocolFixtureMode::RepeatedFailure);
    let (state, reason) = c5_fix10_visible_end(&f, &run);
    let _units = fixture
        .observations()
        .iter()
        .map(|observation| NativeUnitCleanup::new(&observation.identity))
        .collect::<Vec<_>>();
    assert_eq!(
        state.phase,
        RunPhase::Paused,
        "C5_FIX10_REPLAN_PHASE reason={reason:?}"
    );
    assert_eq!(
        reason,
        super::super::studio_runtime::REPLAN_PAUSE,
        "C5_FIX10_REPLAN_REASON"
    );
    c4_wait(
        || {
            f.projection(&run).attempts.iter().all(|attempt| {
                matches!(
                    attempt.state,
                    crate::orchestration::orchestration_records::AttemptState::Retired(_)
                )
            })
        },
        "C5_FIX10_KILLED_ATTEMPT_RETIRED",
    );
    let sequence = f.projection(&run).sequence;
    let observations = fixture.observations();
    assert_eq!(observations.len(), 1, "C5_FIX10_NO_RELAUNCH");
    // The fixture marks `survived` 3 s after its failures unless killed.
    std::thread::sleep(Duration::from_secs(4));
    let killed = !observations[0].scratch.join("survived").exists();
    assert!(killed, "C5_FIX10_PROVIDER_NOT_KILLED");
    let executions =
        std::fs::read_to_string(observations[0].scratch.join("executions")).unwrap_or_default();
    assert_eq!(executions.lines().count(), 1, "C5_FIX10_ONE_EXECUTION");
    let after = f.projection(&run);
    assert_eq!(
        after.sequence, sequence,
        "C5_FIX10_SECOND_REFUSAL_PUBLICATION"
    );
    assert_eq!(
        c5_fix10_wait_reason(&f, &run).as_deref(),
        Some(reason.as_str())
    );
    let record = spending_record(&f, &run);
    let task = &record.tasks[0];
    assert_eq!(task.sends.len(), 1, "C5_FIX10_KILL_CHARGED_AGAIN");
    assert_eq!(
        task.pause,
        Some(kinewright_project::orchestration_accounting::AccountingRefusal::Replan)
    );
    assert_eq!(task.repairs[0].rounds, 2);
    c5_fix3_stop(&f, &run, &fixture);
    eprintln!(
        "C5_FIX10_READER_KILL_TRACE killed={killed} phase={:?} reason={reason:?} executions=1 sends={} repairs={} sequence_stable=true",
        after.phase,
        task.sends.len(),
        task.repairs[0].rounds
    );
}

/// Codex 0.166's real refused-call items through the real reader: two
/// different refusals with `error: null` are not provider failures, so the
/// provider's turn completes and nothing pauses.
#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c5_fix10_failed_tool_items_complete_the_turn() {
    use super::super::studio_runtime::fixture::ProtocolFixtureMode;
    let (f, run, fixture) = c5_fix3_host(ProtocolFixtureMode::FailedToolItems);
    c4_wait(
        || {
            spending_record(&f, &run)
                .tasks
                .iter()
                .any(|task| task.usage.iter().any(|sample| sample.complete))
        },
        "C5_FIX10_ITEMS_TURN_COMPLETED",
    );
    let _units = fixture
        .observations()
        .iter()
        .map(|observation| NativeUnitCleanup::new(&observation.identity))
        .collect::<Vec<_>>();
    let record = spending_record(&f, &run);
    let task = &record.tasks[0];
    assert!(task.repairs.is_empty(), "C5_FIX10_ITEMS_REPAIRED");
    assert_eq!(task.last_failure, None);
    assert_eq!(task.pause, None, "C5_FIX10_ITEMS_PAUSED");
    assert_ne!(
        c5_fix10_wait_reason(&f, &run).as_deref(),
        Some(super::super::studio_runtime::REPLAN_PAUSE)
    );
    c5_fix3_stop(&f, &run, &fixture);
    eprintln!("C5_FIX10_FAILED_ITEMS_COMPLETED repairs=0 pause=None");
}

/// Lead ruling on C5 fix 10: Codex's retry notices are top-level `error`
/// events while the turn goes on. Three of them, then a completed turn: no
/// repair, no pause, and the turn's usage is recorded.
#[test]
fn s4b_c5_fix10_reconnect_notices_are_not_repairs() {
    let f = Fixture::new();
    let (run, execution, _, producer, scratch, _charge) = c5_fix10_charged(&f);
    let mut events = (1..=3)
        .map(|attempt| serde_json::json!({"type":"error","message":format!("Reconnecting... {attempt}/5")}))
        .collect::<Vec<_>>();
    events.push(serde_json::json!({"type":"turn.completed","id":"r","usage":{"input_tokens":20,"cached_input_tokens":10,"output_tokens":5}}));
    for (ordinal, raw) in events.iter().enumerate() {
        let ingested = c5_fix10_raw(&f, &producer, &execution, raw, ordinal as u64 + 1);
        assert!(
            ingested.is_ok(),
            "C5_FIX10_RECONNECT_REFUSED {raw} {ingested:?}"
        );
    }
    let record = spending_record(&f, &run);
    let task = &record.tasks[0];
    assert!(task.repairs.is_empty(), "C5_FIX10_RECONNECT_REPAIRED");
    assert_eq!(task.repeated_failures, 0);
    assert_eq!(task.last_failure, None, "C5_FIX10_RECONNECT_HASHED");
    assert_eq!(task.pause, None, "C5_FIX10_RECONNECT_PAUSED");
    assert!(
        task.usage.iter().any(|sample| sample.complete),
        "C5_FIX10_RECONNECT_TURN_USAGE"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

/// Codex's last retry notice, then `turn.failed`: exactly one repair round.
/// A non-retry top-level `error` still counts.
#[test]
fn s4b_c5_fix10_exhausted_reconnect_counts_once_and_other_errors_count() {
    use crate::writer_host::native_invocation::provider_failure;
    let f = Fixture::new();
    let (run, execution, _, producer, scratch, _charge) = c5_fix10_charged(&f);
    for (ordinal, raw) in [
        serde_json::json!({"type":"error","message":"Reconnecting... 5/5"}),
        serde_json::json!({"type":"turn.failed","error":{"message":"stream disconnected before completion"}}),
    ]
    .iter()
    .enumerate()
    {
        c5_fix10_raw(&f, &producer, &execution, raw, ordinal as u64 + 1).unwrap();
    }
    let record = spending_record(&f, &run);
    let task = &record.tasks[0];
    assert_eq!(task.repairs.len(), 1);
    assert_eq!(task.repairs[0].rounds, 1, "C5_FIX10_EXHAUSTED_ROUNDS");
    assert_eq!(task.repeated_failures, 1);
    assert_eq!(
        task.last_failure.as_deref(),
        Some(
            kinewright_media::sha256_bytes(
                b"turn.failed\0message\0stream disconnected before completion"
            )
            .as_str()
        )
    );
    assert_eq!(task.pause, None);
    std::fs::remove_dir_all(scratch).unwrap();

    let f = Fixture::new();
    let (run, execution, _, producer, scratch, _charge) = c5_fix10_charged(&f);
    let other = serde_json::json!({"type":"error","message":"the model refused the request"});
    c5_fix10_raw(&f, &producer, &execution, &other, 1).unwrap();
    let record = spending_record(&f, &run);
    let task = &record.tasks[0];
    assert_eq!(task.repairs.len(), 1, "C5_FIX10_OTHER_ERROR_NOT_REPAIR");
    assert_eq!(task.repairs[0].rounds, 1);
    assert_eq!(
        task.last_failure.as_deref(),
        Some(
            kinewright_media::sha256_bytes(b"error\0message\0the model refused the request")
                .as_str()
        )
    );
    std::fs::remove_dir_all(scratch).unwrap();

    for notice in ["Reconnecting... 1/5", "Reconnecting... waiting for network"] {
        assert_eq!(
            provider_failure(
                "error",
                &serde_json::json!({"type":"error","message":notice})
            ),
            None,
            "C5_FIX10_NOTICE_CLASSIFIED {notice}"
        );
    }
    // Only the retry notice is exempt: a failed turn always counts, and an
    // MCP message is never a transport notice.
    assert!(
        provider_failure(
            "turn.failed",
            &serde_json::json!({"error":{"message":"Reconnecting... 5/5"}})
        )
        .is_some()
    );
    assert!(
        provider_failure(
            "error",
            &serde_json::json!({"message":"mcp server connection closed"})
        )
        .is_some()
    );
}

/// The real reader and guardian: three retry notices, then a completed turn.
#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c5_fix10_reconnect_notices_complete_the_turn() {
    use super::super::studio_runtime::fixture::ProtocolFixtureMode;
    let (f, run, fixture) = c5_fix3_host(ProtocolFixtureMode::ReconnectNotices);
    c4_wait(
        || {
            spending_record(&f, &run)
                .tasks
                .iter()
                .any(|task| task.usage.iter().any(|sample| sample.complete))
        },
        "C5_FIX10_RECONNECT_TURN_COMPLETED",
    );
    let _units = fixture
        .observations()
        .iter()
        .map(|observation| NativeUnitCleanup::new(&observation.identity))
        .collect::<Vec<_>>();
    let record = spending_record(&f, &run);
    let task = &record.tasks[0];
    assert!(task.repairs.is_empty(), "C5_FIX10_RECONNECT_REPAIRED");
    assert_eq!(task.last_failure, None);
    assert_eq!(task.pause, None, "C5_FIX10_RECONNECT_PAUSED");
    c5_fix3_stop(&f, &run, &fixture);
    eprintln!("C5_FIX10_RECONNECT_COMPLETED notices=3 repairs=0 pause=None");
}

/// The real reader and guardian: Codex's last retry notice, then
/// `turn.failed` and exit. One repair round, no pause, and the run ends
/// visibly rather than silently stalled.
#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c5_fix10_exhausted_reconnect_counts_one_round() {
    use super::super::studio_runtime::fixture::ProtocolFixtureMode;
    let (f, run, fixture) = c5_fix3_host(ProtocolFixtureMode::ReconnectExhausted);
    let (state, reason) = c5_fix10_visible_end(&f, &run);
    let _units = fixture
        .observations()
        .iter()
        .map(|observation| NativeUnitCleanup::new(&observation.identity))
        .collect::<Vec<_>>();
    let record = spending_record(&f, &run);
    let task = &record.tasks[0];
    assert_eq!(task.repairs.len(), 1, "C5_FIX10_EXHAUSTED_REPAIRS");
    assert_eq!(task.repairs[0].rounds, 1, "C5_FIX10_EXHAUSTED_ROUNDS");
    assert_eq!(task.pause, None, "C5_FIX10_EXHAUSTED_PAUSED");
    assert_eq!(task.sends.len(), 1);
    c5_fix3_stop(&f, &run, &fixture);
    eprintln!(
        "C5_FIX10_EXHAUSTED_ONE_ROUND phase={:?} reason={reason:?} rounds=1 pause=None",
        state.phase
    );
}
