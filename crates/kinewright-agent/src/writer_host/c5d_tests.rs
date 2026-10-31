#[test]
fn s4b_c5d_profile_budget_can_be_lowered_and_raised_without_rewriting_usage() {
    use super::super::native_invocation::SpendingChange;
    let default = crate::orchestration::adapters::codex_candidate()
        .default_token_ceiling()
        .get();
    assert_eq!(default, 2_000_000, "C5D_PROFILE_TOTAL_TOKEN_DEFAULT");
    assert_eq!(
        super::super::studio_runtime::RuntimePorts::default()
            .ceiling
            .unwrap()
            .get(),
        default
    );
    for limit in [default, 900_000, 3_000_000] {
        let f = Fixture::new();
        let (run, execution, _) = reserve(&f);
        let now = native_now(&f, &run);
        spending_change(&f, &execution, SpendingChange::Ceiling(limit), now, 0, true).unwrap();
        let record = spending_record(&f, &run);
        assert_eq!(record.tasks[0].token_limit, Some(limit));
        assert!(
            record.tasks[0].usage.is_empty(),
            "C5D_LIMIT_NEVER_INVENTS_USAGE"
        );
    }
}

#[test]
fn s4b_c5d_acknowledged_merge_holds_outside_owner_and_stop_revokes_it() {
    let mut f = Fixture::new();
    let mut scoped = Document::default();
    for id in [1, 2] {
        if let kinewright_core::Operation::AddMarker { marker } = history_marker(id) {
            scoped.markers.push(marker);
        }
    }
    f.brief.scope =
        crate::orchestration::policy::director_scope(&scoped, f.brief.scope.base.clone()).unwrap();
    let gate = Arc::new(super::super::qualification_gate::MergeStopGate::default());
    let _release = gate.release_on_drop();
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .qualification_stop = Some(gate.clone());
    let (run, parent, director) = c4_director(&f);
    let mut brief = f.brief.clone();
    brief.role = Role::Editor;
    director.accepted(
        "assign_task",
        OrchestrationCommand::AssignTask(Box::new(AssignTask {
            run: run.clone(),
            parent: parent.task,
            brief,
        })),
    );
    let child = f.projection(&run).tasks[1].id.clone();
    let (attempt, client, endpoint, core) = c4_reserve_task(&f, &run, &child);
    history_turn(&f, &attempt, &client, &endpoint, 1, true);
    let specialist = C4Mcp::new(&f, &run, &child);
    let expected = c4_mcp_edit_redo(&specialist, &core);
    let frozen = director.accepted(
        "freeze_task",
        OrchestrationCommand::FreezeTask(child.clone()),
    );
    let result = serde_json::from_value(frozen["result"].clone()).unwrap();
    director.accepted(
        "publish_result",
        OrchestrationCommand::PublishResult(Box::new(PublishResult {
            task: child.clone(),
            result,
        })),
    );
    let state = f.projection(&run);
    director.accepted(
        "merge_task",
        OrchestrationCommand::MergeTask(MergeTask {
            task: child,
            target: f.brief.scope.base.clone(),
            result: state.results.last().unwrap().clone(),
        }),
    );
    assert!(!gate.holding(), "C5D_MERGE_REPLY_RETURNED_BEFORE_HOLD");
    let worker = std::thread::spawn(move || director.invoke("task_status", serde_json::json!({})));
    let deadline = Instant::now() + Duration::from_secs(2);
    while !gate.holding() && Instant::now() < deadline {
        std::thread::yield_now();
    }
    let held = gate.holding();
    // A blocked MCP request must not hold H3. This also keeps a broken test bounded.
    let host = f.host.owner.upgrade().unwrap();
    let owner_available = host.try_lock().is_ok();
    if held && owner_available {
        let stop = f.control(&f.editor, &run, RunAction::Stop);
        ok(
            f.editor
                .request(HostCommand::DispatchOrchestration {
                    request: Box::new(stop),
                })
                .unwrap(),
            "C5D_ACTIVE_STOP",
        );
    }
    gate.release();
    let result = worker.join().unwrap();
    assert!(held, "C5D_MERGE_ACK_BEFORE_STOP");
    assert!(owner_available, "C5D_BARRIER_OUTSIDE_OWNER");
    assert_eq!(result.is_error, Some(true), "C5D_STOP_REVOKES_HELD_CONTROL");
    assert_eq!(
        core.export_history().unwrap(),
        expected,
        "C5D_STOP_PRESERVES_CHILD"
    );
}
