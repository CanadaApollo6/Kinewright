struct C4Mcp {
    runtime: tokio::runtime::Runtime,
    client: rmcp::service::RunningService<rmcp::RoleClient, ()>,
    _server: crate::McpServer,
}
impl C4Mcp {
    fn new(f: &Fixture, run: &RunId, task: &TaskId) -> Self {
        use rmcp::{
            ServiceExt,
            transport::{
                StreamableHttpClientTransport,
                streamable_http_client::StreamableHttpClientTransportConfig,
            },
        };
        let host = f.host.owner.upgrade().unwrap();
        let owner = host.lock().unwrap();
        let server = owner
            .start_task_server(&owner.run_producer(run).unwrap(), run, task)
            .unwrap();
        drop(owner);
        let bootstrap = mcp_credentials(server.endpoint()).unwrap();
        let config = StreamableHttpClientTransportConfig::with_uri(server.endpoint().to_owned())
            .custom_headers(
                mcp_headers(&bootstrap)
                    .unwrap()
                    .into_iter()
                    .filter_map(|(name, value)| name.map(|name| (name, value)))
                    .collect(),
            );
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let client = runtime
            .block_on(async {
                ().serve(StreamableHttpClientTransport::with_client(
                    reqwest::Client::new(),
                    config,
                ))
                .await
            })
            .unwrap();
        Self {
            runtime,
            client,
            _server: server,
        }
    }
    #[allow(
        clippy::needless_pass_by_value,
        reason = "test requests take owned JSON values"
    )]
    fn raw(&self, name: &str, arguments: serde_json::Value) -> rmcp::model::CallToolResult {
        self.runtime
            .block_on(
                self.client.call_tool(
                    rmcp::model::CallToolRequestParams::new(name.to_owned())
                        .with_arguments(arguments.as_object().unwrap().clone()),
                ),
            )
            .unwrap()
    }
    #[allow(
        clippy::needless_pass_by_value,
        reason = "test requests take owned JSON values"
    )]
    fn invoke(&self, name: &str, arguments: serde_json::Value) -> rmcp::model::CallToolResult {
        self.raw(
            "invoke_capability",
            serde_json::json!({"name":name,"arguments":arguments}),
        )
    }
    fn status(&self) -> serde_json::Value {
        let reply = self.invoke("task_status", serde_json::json!({}));
        assert_eq!(reply.is_error, Some(false), "C4_TASK_STATUS {reply:?}");
        reply.structured_content.unwrap()
    }
    fn control(&self, command: OrchestrationCommand) -> serde_json::Value {
        let mut control = self.status()["control"].clone();
        let mut command = command;
        if let OrchestrationCommand::ControlRun(intent) = &mut command {
            intent.command_id = serde_json::from_value(control["id"].clone()).unwrap();
        }
        control["command"] = serde_json::to_value(command).unwrap();
        control
    }
    fn accepted(&self, name: &str, command: OrchestrationCommand) -> serde_json::Value {
        let result = self.invoke(name, self.control(command));
        assert_eq!(
            result.is_error,
            Some(false),
            "C4_MCP_CONTROL {name}: {result:?}"
        );
        result.structured_content.unwrap()
    }
}
fn c4_director(f: &Fixture) -> (RunId, ExecutionIdentity, C4Mcp) {
    let (run, execution, producer) = reserve(f);
    task_actor(f, &run, &execution, &producer);
    let mcp = C4Mcp::new(f, &run, &execution.task);
    (run, execution, mcp)
}
#[test]
fn s4b_c4_mcp_narrows_assignment_and_keeps_seven_tools() {
    let mut f = Fixture::new();
    f.brief.scope.operations.insert("add_marker".into());
    let (run, execution, director) = c4_director(&f);
    let listed = director
        .runtime
        .block_on(director.client.list_all_tools())
        .unwrap();
    assert_eq!(
        listed.iter().map(|t| t.name.as_ref()).collect::<Vec<_>>(),
        crate::runtime::COMPACT_TOOL_NAMES
    );
    let schemas = director.raw(
        "get_capability",
        serde_json::json!({"names":["assign_task","freeze_task","merge_task","task_status"]}),
    );
    assert_eq!(
        schemas.is_error,
        Some(false),
        "C4_CONTROL_DISCOVERY {schemas:?}"
    );
    let mut child = f.brief.clone();
    child.role = Role::Editor;
    let mut wider = child.clone();
    wider.scope.operations.insert("add_track".into());
    let denied = director.invoke(
        "assign_task",
        director.control(OrchestrationCommand::AssignTask(Box::new(AssignTask {
            run: run.clone(),
            parent: execution.task.clone(),
            brief: wider,
        }))),
    );
    assert_eq!(
        denied.is_error,
        Some(true),
        "C4_AUTHORITY_NARROWED {denied:?}"
    );
    let mut bad_head = director.control(OrchestrationCommand::AssignTask(Box::new(AssignTask {
        run: run.clone(),
        parent: execution.task.clone(),
        brief: child.clone(),
    })));
    bad_head["expected_head"] = serde_json::json!("0".repeat(32));
    assert_eq!(
        director.invoke("assign_task", bad_head).is_error,
        Some(true),
        "C4_HEAD_NARROWED"
    );
    let command = OrchestrationCommand::AssignTask(Box::new(AssignTask {
        run: run.clone(),
        parent: execution.task.clone(),
        brief: child.clone(),
    }));
    let mut stale_epoch = director.control(command);
    stale_epoch["owner_epoch"] = serde_json::json!(0);
    assert_eq!(
        director.invoke("assign_task", stale_epoch).is_error,
        Some(true),
        "C4_STALE_EPOCH"
    );
    let mut wider_root = child.clone();
    wider_root
        .scope
        .asset_roots
        .push(f.brief.assignment.clone());
    assert_eq!(
        director
            .invoke(
                "assign_task",
                director.control(OrchestrationCommand::AssignTask(Box::new(AssignTask {
                    run: run.clone(),
                    parent: execution.task.clone(),
                    brief: wider_root,
                })))
            )
            .is_error,
        Some(true),
        "C4_ROOT_NARROWED"
    );
    let assigned = director.accepted(
        "assign_task",
        OrchestrationCommand::AssignTask(Box::new(AssignTask {
            run: run.clone(),
            parent: execution.task.clone(),
            brief: child,
        })),
    );
    assert_eq!(
        assigned["receipt"]["decision"],
        serde_json::json!({"kind":"applied"})
    );
    let state = f.projection(&run);
    assert_eq!(state.tasks.len(), 2);
    assert_eq!(state.tasks[1].brief.role, Role::Editor);
    assert_eq!(
        state.tasks[1].progress.capacity,
        DependencyReservation::Reserved
    );
    let child_id = state.tasks[1].id.clone();
    let (_, _, _, _) = c4_reserve_task(&f, &run, &child_id);
    let specialist = C4Mcp::new(&f, &run, &child_id);
    let mut brief = f.brief.clone();
    brief.role = Role::Reviewer;
    assert_eq!(
        specialist
            .invoke(
                "assign_task",
                specialist.control(OrchestrationCommand::AssignTask(Box::new(AssignTask {
                    run: run.clone(),
                    parent: child_id,
                    brief,
                })))
            )
            .is_error,
        Some(true),
        "C4_ROLE_NARROWED"
    );
    let viewer = f.actor(AttachmentRole::Viewer, "c4-viewer");
    let reply = viewer
        .request(HostCommand::DispatchOrchestration {
            request: Box::new(f.control(&viewer, &run, RunAction::Stop)),
        })
        .unwrap();
    assert!(reply.error.is_some(), "C4_VIEWER_CONTROL_REFUSED");
}
#[test]
fn s4b_c4_mcp_stale_control_never_queues_a_send() {
    let f = Fixture::new();
    let (run, execution, director) = c4_director(&f);
    let stale_revision = f.projection(&run).control_revision;
    let stale_command = OrchestrationCommand::ControlRun(RunIntent {
        command_id: CommandId {
            issuer: identity(),
            sequence: 1,
        },
        run_id: run.clone(),
        expected_control_revision: stale_revision,
        action: RunAction::SendMessage {
            text_ref: f.brief.assignment.clone(),
        },
    });
    // An acknowledged independent transition invalidates the control revision.
    let mut child = f.brief.clone();
    child.role = Role::Reviewer;
    director.accepted(
        "assign_task",
        OrchestrationCommand::AssignTask(Box::new(AssignTask {
            run: run.clone(),
            parent: execution.task.clone(),
            brief: child,
        })),
    );
    let before = f.projection(&run).effects;
    let stale = director.control(stale_command);
    let result = director.invoke("send_message", stale);
    assert_eq!(
        result.is_error,
        Some(false),
        "C4_STALE_CONTROL_RECORDED {result:?}"
    );
    let result = result.structured_content.unwrap();
    assert_eq!(
        result["receipt"]["decision"],
        serde_json::json!({"kind":"refused","reason":"stale_control"}),
        "C4_STALE_CONTROL {result}"
    );
    assert_eq!(
        f.projection(&run).effects,
        before,
        "C4_STALE_SEND_UNCHANGED"
    );
}
fn c4_result(f: &Fixture, run: &RunId, state: &RunProjection) -> RunProjection {
    let mut next = state.clone();
    let mut child = next.tasks[0].clone();
    child.id = identity();
    child.parent = Some(next.tasks[0].id.clone());
    child.brief.role = Role::Reviewer;
    child.state = TaskState::Freezing;
    child.progress = TaskProgress::default();
    child.progress.turn_ended = true;
    child.progress.turn = Some(TaskTurn {
        task: child.id.clone(),
        attempt: identity(),
        session: identity(),
        ordinal: 1,
        owner_epoch: next.attempts[0].owner_epoch,
        expected_head: child.brief.scope.base.head.clone(),
        purpose: TurnPurpose::Work,
    });
    let result = TaskResult {
        outcome: AttemptOutcome::Completed,
        base: Some(child.brief.scope.base.clone()),
        head: Some(child.brief.scope.base.clone()),
        delta_ref: None,
        artifacts: vec![],
        evidence: vec![],
        usage_ref: next.usage[0].clone(),
        explanation: f.brief.assignment.clone(),
    };
    let request = f.request(
        &f.editor,
        Some(run.clone()),
        OrchestrationCommand::PublishResult(Box::new(PublishResult {
            task: child.id.clone(),
            result: result.clone(),
        })),
    );
    let reference = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner_mut()
        .unwrap()
        .admit_scheduler_record(&OrchestrationRecord {
            version: 1,
            payload: OrchestrationPayload::Result(Box::new(result)),
        })
        .unwrap()
        .reference;
    next.tasks.push(child);
    crate::orchestration::mailbox::result_and_delivery(&next, &request.envelope, reference).unwrap()
}
#[test]
fn s4b_c4_late_result_gets_a_new_charged_wake_and_offer() {
    use crate::orchestration::adapters::SendKind;
    let f = Fixture::new();
    let (run, execution, actor) = reserve(&f);
    let mut state = f.projection(&run);
    state.effects[0].state = EffectState::Settled;
    let state = c4_result(&f, &run, &state);
    let first = state.deliveries[0].clone();
    publish_projection(&f, &actor, &run, state, Some(&execution));
    let admitted = native_admitted(&f, &execution);
    let (profile, _, scratch) = native_profile(&f, &admitted);
    let host = f.host.owner.upgrade().unwrap();
    let producer = f.producer(&run);
    let now = native_now(&f, &run);
    let charged = host
        .lock()
        .unwrap()
        .charge_native_invocation(
            &producer,
            &admitted,
            profile,
            &first.wake,
            SendKind::AutomaticResultWake,
            now,
        )
        .unwrap();
    let mut state = c4_result(&f, &run, &f.projection(&run));
    let second = state.deliveries[1].clone();
    assert_ne!(second.wake, first.wake, "C4_LATE_RESULT_NEW_WAKE");
    assert_eq!(
        second.generation,
        first.generation + 1,
        "C4_LATE_RESULT_NEW_GENERATION"
    );
    assert_eq!(state.deliveries[0], first, "C4_ORIGINAL_RESULT_IDENTITY");
    state.attempts[0].state = AttemptState::Contained;
    publish_projection(&f, &actor, &run, state, None);
    host.lock()
        .unwrap()
        .native_started(&producer, &execution)
        .unwrap();
    let state = f.projection(&run);
    assert_eq!(
        state.deliveries[0].state,
        DeliveryState::Offered(execution.attempt, first.message),
        "C4_CHARGED_WAKE_OFFERED"
    );
    assert_eq!(state.deliveries[1], second, "C4_UNCHARGED_WAKE_NOT_OFFERED");
    assert_eq!(state.deliveries[1].state, DeliveryState::Pending);
    drop(charged);
    std::fs::remove_dir_all(scratch).unwrap();
}
fn c4_reserve_task(
    f: &Fixture,
    run: &RunId,
    task: &TaskId,
) -> (
    ExecutionIdentity,
    HostClient,
    crate::orchestration::tasks::TaskEndpoint,
    Core,
) {
    let producer = f.producer_client(run);
    let host = f.host.owner.upgrade().unwrap();
    let mut state = f.projection(run);
    let epoch = {
        let mut owner = host.lock().unwrap();
        owner.archive_retired_attempts(&mut state).unwrap();
        owner.owner().unwrap().store().unwrap().owner_epoch()
    };
    let execution = ExecutionIdentity {
        version: 1,
        project: host.lock().unwrap().owner().unwrap().project_id().unwrap(),
        run: run.clone(),
        task: task.clone(),
        attempt: identity(),
        nonce: identity(),
        owner_instance: identity(),
    };
    let mut execution = execution;
    execution.project = host.lock().unwrap().owner().unwrap().project_id().unwrap();
    state.attempts.push(AttemptReservation {
        identity: execution.clone(),
        owner_epoch: epoch,
        state: AttemptState::Reserved,
        recovery_ref: f.brief.assignment.clone(),
    });
    let record = state.tasks.iter_mut().find(|t| &t.id == task).unwrap();
    record.state = TaskState::Preparing;
    record.progress.capacity = DependencyReservation::Backed(execution.attempt.clone());
    crate::orchestration::tasks::refresh_cleanup(&mut state);
    publish_projection(f, &producer, run, state, Some(&execution));
    let (auth, endpoint) = host
        .lock()
        .unwrap()
        .prepare_task_endpoint(
            &producer.bootstrap.auth,
            run,
            task,
            &execution.attempt,
            Duration::from_secs(300),
        )
        .unwrap_or_else(|error| panic!("C4_MCP_ATTEMPT_HISTORY: {error}"));
    let client = HostClient::connect(f.host.endpoint.clone(), auth, f.host.roots.clone()).unwrap();
    let core = host.lock().unwrap().hooks.runs[run].tasks[task]
        .core()
        .clone();
    (execution, client, endpoint, core)
}
fn c4_restart(f: Fixture, claim_root: &std::path::Path) -> Fixture {
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
    let ports = super::super::studio_runtime::RuntimePorts {
        fixture_claim: Some(claim_root.into()),
        ..Default::default()
    };
    let host = WriterHost::start_inner(
        HostConfig {
            scheduler_settings: Some(temp.path("scheduler")),
            project: "hooks".into(),
            path: Some(temp.path("project.kinewright")),
            roots: roots.clone(),
            origin: HostOrigin::Headless,
        },
        HostAuth::generate().unwrap(),
        Some(issuer_auth.clone()),
        ports,
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
    reason = "one MCP sequence covers exact task/manual history across turn, attempt, restart, merge and Stop"
)]
fn s4b_c4_mcp_history_survives_attempt_restart_merge_and_stop() {
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
    let choice = std::fs::read(settings.join("director.json")).unwrap();
    let manual = {
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
    let (_, _, _, _parent_core) = c4_reserve_task(&f, &run, &parent.task);
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
    let merged = director.invoke("merge_task", merge.clone());
    assert_eq!(merged.is_error, Some(false), "C4_MCP_MERGE {merged:?}");
    let repeated = director.invoke("merge_task", merge);
    assert_eq!(repeated.is_error, Some(false), "C4_MERGE_ONCE {repeated:?}");
    assert_eq!(
        merged.structured_content, repeated.structured_content,
        "C4_MERGE_ORIGINAL_RECEIPT"
    );
    assert_eq!(
        third_core.export_history().unwrap(),
        expected,
        "C4_MCP_HISTORY_EXACT"
    );
    let saved_state = f.projection(&run);
    let saved = {
        let host = f.host.owner.upgrade().unwrap();
        let owner = host.lock().unwrap();
        assert_eq!(
            owner.core().export_history().unwrap(),
            manual,
            "C4_MANUAL_HISTORY_EXACT"
        );
        owner
            .saved_task_history(&saved_state, &child)
            .unwrap()
            .unwrap()
    };
    let stop = f.control(&f.editor, &run, RunAction::Stop);
    ok(
        f.editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(stop),
            })
            .unwrap(),
        "C4_STOP",
    );
    drop(director);
    drop(third_core);
    let f = c4_restart(f, &claim_root);
    let state = f.projection(&run);
    assert_eq!(state.cleanup, RunCleanup::Settled, "C4_STOP_REOPEN_CLEANUP");
    let host = f.host.owner.upgrade().unwrap();
    let owner = host.lock().unwrap();
    let recovered = owner.saved_task_history(&state, &child).unwrap().unwrap();
    assert_eq!(recovered, saved, "C4_MCP_MERGE_RECEIPT_REOPEN");
    let checkpoint = owner
        .owner()
        .unwrap()
        .store()
        .unwrap()
        .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(
            &saved.source.checkpoint,
        )
        .unwrap();
    let history = owner
        .owner()
        .unwrap()
        .store()
        .unwrap()
        .read_record::<kinewright_core::HistorySnapshot>(&checkpoint.history_ref)
        .unwrap();
    assert_eq!(*history, expected, "C4_MCP_HISTORY_AFTER_STOP_REOPEN");
    assert_eq!(
        owner.core().export_history().unwrap(),
        manual,
        "C4_MANUAL_REDO_AFTER_STOP"
    );
    assert_eq!(
        std::fs::read(settings.join("director.json")).unwrap(),
        choice,
        "C4_SAVED_DIRECTOR_CHOICE"
    );
    eprintln!(
        "C4_MCP_HISTORY task_exact=true task_redo=true manual_exact=true manual_redo=true merge_once=true choice_exact=true"
    );
}
fn c4_mcp_edit_redo(specialist: &C4Mcp, core: &Core) -> kinewright_core::HistorySnapshot {
    for id in [1, 2] {
        let revision = grant_test_revision(core);
        let prepared=specialist.raw("prepare_edit_plan",serde_json::json!({"expected_revision":revision,"operations":[{"op":"add_marker","marker":{"id":id,"position":id,"label":format!("marker {id}"),"color_token":0}}]}));
        assert_eq!(
            prepared.is_error,
            Some(false),
            "C4_SPECIALIST_MCP_PREPARE {prepared:?}"
        );
        let committed=specialist.raw("commit_edit_plan",serde_json::json!({"expected_revision":revision,"plan_id":prepared.structured_content.unwrap()["plan_id"]}));
        assert_eq!(
            committed.is_error,
            Some(false),
            "C4_SPECIALIST_MCP_EDIT {committed:?}"
        );
    }
    let undo = specialist.invoke(
        "undo_task_edit",
        serde_json::json!({"expected_revision":grant_test_revision(core)}),
    );
    assert_eq!(
        undo.is_error,
        Some(false),
        "C4_SPECIALIST_MCP_UNDO {undo:?}"
    );
    let expected = core.export_history().unwrap();
    assert!(!expected.cursor.redo.is_empty(), "C4_MCP_REDO_TAIL");
    eprintln!("C4_MCP_EDIT_ACK redo=true");
    expected
}
fn c4_wait(mut ready: impl FnMut() -> bool, marker: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready() {
        assert!(Instant::now() < deadline, "{marker}");
        std::thread::park_timeout(Duration::from_millis(2));
    }
}
#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c4_host_session_drives_production_effect_pump() {
    use super::super::studio_runtime::fixture::{ProtocolFixture, ProtocolFixtureMode};
    let mut f = Fixture::new();
    let mut scoped = Document::default();
    for id in [1, 2] {
        if let kinewright_core::Operation::AddMarker { marker } = history_marker(id) {
            scoped.markers.push(marker);
        }
    }
    f.brief.scope =
        crate::orchestration::policy::director_scope(&scoped, f.brief.scope.base.clone()).unwrap();
    let started = f.start();
    let run: RunId = started.envelope.identity("run").unwrap();
    let fixture = ProtocolFixture::new(f.temp.root(), ProtocolFixtureMode::Normal);
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
    let host = WriterHost::start_inner(
        HostConfig {
            scheduler_settings: None,
            project: "hooks".into(),
            path: Some(temp.path("project.kinewright")),
            roots: roots.clone(),
            origin: HostOrigin::Headless,
        },
        HostAuth::generate().unwrap(),
        Some(issuer_auth.clone()),
        fixture.ports(),
    )
    .unwrap();
    let editor = host.client();
    let issuer = HostIssuer::new(host.endpoint.clone(), issuer_auth.clone(), roots).unwrap();
    let f = Fixture {
        host,
        editor,
        issuer,
        issuer_auth,
        brief,
        temp,
    };
    // Status uses the host's production lifecycle, not an explicit EffectPump tick.
    ok(
        f.editor.request(HostCommand::Status).unwrap(),
        "C4_PRODUCTION_DRIVE",
    );
    c4_wait(
        || {
            let projection = f.projection(&run);
            if projection.phase == RunPhase::Interrupted
                && let RunWait::Provider(reason) = projection.wait
            {
                let host = f.host.owner.upgrade().unwrap();
                let owner = host.lock().unwrap();
                panic!(
                    "C4_PRODUCTION_REFUSAL {}",
                    String::from_utf8_lossy(
                        &owner
                            .owner()
                            .unwrap()
                            .store()
                            .unwrap()
                            .read_evidence(&reason.reason)
                            .unwrap()
                    )
                );
            }
            fixture
                .observations()
                .first()
                .is_some_and(|o| o.scratch.join("execution-ready").exists())
        },
        "C4_PRODUCTION_EXECUTED",
    );
    let observation = fixture.observations()[0].clone();
    let _unit = NativeUnitCleanup::new(&observation.identity);
    assert!(
        mcp_credentials(&observation.endpoint).is_some(),
        "C4_REAL_TASK_MCP_BINDING"
    );
    assert!(
        f.projection(&run)
            .attempts
            .iter()
            .any(|a| a.identity == observation.identity && a.state == AttemptState::Executing)
    );
    let task_mcp = C4Mcp::new(&f, &run, &observation.identity.task);
    let task_core = f.host.owner.upgrade().unwrap().lock().unwrap().hooks.runs[&run].tasks
        [&observation.identity.task]
        .core()
        .clone();
    let expected = c4_mcp_edit_redo(&task_mcp, &task_core);
    let frozen = task_mcp.accepted(
        "freeze_task",
        OrchestrationCommand::FreezeTask(observation.identity.task.clone()),
    );
    assert!(
        f.projection(&run).tasks[0].progress.turn_ended,
        "C4_HOST_ENDS_TURN_WITH_FROZEN_HISTORY"
    );
    let result: TaskResult = serde_json::from_value(frozen["result"].clone()).unwrap();
    task_mcp.accepted(
        "publish_result",
        OrchestrationCommand::PublishResult(Box::new(PublishResult {
            task: observation.identity.task.clone(),
            result,
        })),
    );
    assert!(
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .hooks
            .native
            .values()
            .all(|attempt| !attempt.control.closed()),
        "C4_RESULT_DRAINS_USAGE_BEFORE_CLEANUP"
    );
    drop(task_mcp);
    std::fs::write(observation.scratch.join("exit"), b"").unwrap();
    c4_wait(
        || {
            f.projection(&run)
                .attempts
                .iter()
                .all(|a| matches!(a.state, AttemptState::Retired(_)))
        },
        "C4_PRODUCTION_CLEANUP",
    );
    assert_eq!(
        f.projection(&run).outcome,
        RunOutcome::Ready,
        "C4_RESULT_READY_AFTER_PROOF"
    );
    assert_eq!(
        task_core.export_history().unwrap(),
        expected,
        "C4_ATOMIC_FREEZE_REDO"
    );
    let record = spending_record(&f, &run);
    assert_eq!(record.tasks[0].sends.len(), 1, "C4_ONE_DURABLE_SEND");
    assert_eq!(record.tasks[0].totals.known.input, Some(1));
    assert_eq!(
        record.tasks[0].totals.known.cost_micros, None,
        "C4_UNAVAILABLE_COST"
    );
    let view = ok(
        f.editor
            .request(HostCommand::ObserveRun {
                run: run.clone(),
                after_sequence: 0,
            })
            .unwrap(),
        "C4_DURABLE_RUN_VIEW",
    )
    .run_view
    .unwrap();
    let accounting = view.accounting.unwrap();
    assert_eq!(
        accounting.tasks[0].totals, record.tasks[0].totals,
        "C4_RUN_VIEW_DURABLE_ACCOUNTING"
    );
    assert_eq!(
        accounting.tasks[0]
            .provider
            .as_ref()
            .unwrap()
            .requested
            .model,
        record.tasks[0].providers[0].requested.model
    );
    let stop = f.control(&f.editor, &run, RunAction::Stop);
    ok(
        f.editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(stop),
            })
            .unwrap(),
        "C4_NATIVE_STOP",
    );
    c4_wait(
        || {
            let _ = f.editor.request(HostCommand::Status);
            !fixture.claim_root().join("run.json").exists()
        },
        "C4_CLEAN_CLAIM_FINISHED",
    );
    fixture.cleanup().unwrap();
    eprintln!(
        "C4_HOST_PUMP exact_task_endpoint=true durable_send=1 raw_usage=true cost_unavailable=true cleanup=true"
    );
}
#[test]
fn s4b_c4_production_has_no_contained_driver_path() {
    for source in [
        include_str!("studio_runtime.rs"),
        include_str!("mod.rs"),
        include_str!("../orchestration/effect_pump.rs"),
    ] {
        assert!(
            !source.contains("ContainedDriver"),
            "C4_NO_CONTAINED_DRIVER_REACHABILITY"
        );
    }
    let runtime = include_str!("studio_runtime.rs");
    assert!(
        runtime.contains("EffectPump::new") && runtime.contains("crate::pump_session("),
        "C4_SOLE_EFFECT_SESSION"
    );
}
#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c4_stop_during_startup_and_full_native_queue() {
    use super::super::studio_runtime::fixture::{ProtocolFixture, ProtocolFixtureMode};
    for mode in [ProtocolFixtureMode::Startup, ProtocolFixtureMode::FullQueue] {
        let f = Fixture::new();
        let start = f.start();
        let run: RunId = start.envelope.identity("run").unwrap();
        let fixture = ProtocolFixture::new(f.temp.root(), mode);
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
        let host = WriterHost::start_inner(
            HostConfig {
                project: "hooks".into(),
                path: Some(temp.path("project.kinewright")),
                roots: roots.clone(),
                origin: HostOrigin::Headless,
                scheduler_settings: None,
            },
            HostAuth::generate().unwrap(),
            Some(issuer_auth.clone()),
            fixture.ports(),
        )
        .unwrap();
        let editor = host.client();
        let issuer = HostIssuer::new(host.endpoint.clone(), issuer_auth.clone(), roots).unwrap();
        let f = Fixture {
            host,
            editor,
            issuer,
            issuer_auth,
            brief,
            temp,
        };
        ok(
            f.editor.request(HostCommand::Status).unwrap(),
            "C4_DRIVE_STOP_FIXTURE",
        );
        c4_wait(|| fixture.startup_entered(), "C4_STARTUP_REACHED");
        let units = if mode == ProtocolFixtureMode::FullQueue {
            c4_wait(
                || fixture.event_entered() && fixture.native_queue_full(),
                "C4_REAL_FULL_NATIVE_QUEUE",
            );
            let units = fixture
                .observations()
                .iter()
                .map(|observation| NativeUnitCleanup::new(&observation.identity))
                .collect::<Vec<_>>();
            assert!(
                f.projection(&run)
                    .attempts
                    .iter()
                    .any(|attempt| attempt.state == AttemptState::Executing),
                "C4_FULL_QUEUE_CHILD_STILL_EXECUTING"
            );
            units
        } else {
            vec![]
        };
        let stop = f.control(&f.editor, &run, RunAction::Stop);
        let reply = ok(
            f.editor
                .request(HostCommand::DispatchOrchestration {
                    request: Box::new(stop),
                })
                .unwrap(),
            "C4_STOP_ADMITTED_WITH_BLOCKED_PUMP",
        );
        assert_eq!(
            reply.orchestration.unwrap().decision,
            CommandDecision::Applied,
            "C4_STOP_APPLIED"
        );
        if mode == ProtocolFixtureMode::Startup {
            assert_ne!(
                f.projection(&run).cleanup,
                RunCleanup::Settled,
                "C4_STOP_WAITING_FOR_CLEANUP_PROOF"
            );
        }
        fixture.release();
        c4_wait(
            || {
                let _ = f.editor.request(HostCommand::Status);
                !fixture.claim_root().join("run.json").exists()
            },
            "C4_STOP_FINAL_CLEANUP",
        );
        assert_eq!(
            f.projection(&run).cleanup,
            RunCleanup::Settled,
            "C4_STOP_PROOF_RECEIVED"
        );
        if mode == ProtocolFixtureMode::Startup {
            assert!(
                fixture
                    .observations()
                    .iter()
                    .all(|observation| !observation.scratch.join("execution-ready").exists()),
                "C4_STARTUP_STOP_NO_EXECUTION"
            );
            assert!(
                spending_record(&f, &run)
                    .tasks
                    .iter()
                    .all(|task| task.sends.is_empty()),
                "C4_STARTUP_STOP_NO_CHARGE"
            );
        }
        fixture.cleanup().unwrap();
        drop(units);
    }
}
