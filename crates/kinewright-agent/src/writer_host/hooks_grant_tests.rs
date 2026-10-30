fn grant_test_revision(core: &Core) -> kinewright_core::TimelineRevision {
    match core
        .request(kinewright_core::Command::Query(
            kinewright_core::Query::Snapshot,
        ))
        .unwrap()
    {
        kinewright_core::Event::QueryResult(kinewright_core::QueryResult::Snapshot {
            revision,
            ..
        }) => revision,
        _ => panic!("HF1_CORE_SNAPSHOT"),
    }
}
#[derive(Clone, Copy)]
enum ConsentCut {
    Stop,
    Deadline,
    Turn,
}
fn stopped_consent(kind: kinewright_core::RuntimeRequestKind, marker: &str, cut: ConsentCut) {
    let mut f = Fixture::new();
    let (run, execution, producer) = reserve(&f);
    let gate = Arc::new(std::array::from_fn(|_| AtomicBool::new(false)));
    let pump = Pump::start_config(
        &f,
        &run,
        &execution,
        &producer,
        kind,
        false,
        Some(gate.clone()),
    );
    let mut answer = response(&f, &run);
    if kind == kinewright_core::RuntimeRequestKind::Secret {
        answer.answer = AttentionAnswer::SecretReference("1".repeat(32));
    }
    let expires = Instant::now() + Duration::from_secs(2);
    if matches!(cut, ConsentCut::Deadline) {
        let owner = f.host.owner.upgrade().unwrap();
        let host = owner.lock().unwrap();
        host.hooks
            .attention
            .lock()
            .unwrap()
            .set_deadline_for_test(&run, expires);
    }
    holder(&mut f);
    ok(
        call(
            &f,
            &f.editor,
            HostCommand::ResolveAttention { response: answer },
            1,
        ),
        "HF1_CONSENT_QUEUED",
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while !gate[0].load(Ordering::Acquire) {
        assert!(Instant::now() < deadline, "HF1_CONSENT_BARRIER");
        std::thread::park_timeout(Duration::from_millis(2));
    }
    if matches!(cut, ConsentCut::Deadline) {
        while Instant::now() < expires {
            std::thread::park_timeout(Duration::from_millis(2));
        }
    } else if matches!(cut, ConsentCut::Turn) {
        let mut state = f.projection(&run);
        state.tasks[0].progress.turn.as_mut().unwrap().session = identity();
        publish_projection(&f, &producer, &run, state, None);
    } else {
        let stop = f.control(&f.editor, &run, RunAction::Stop);
        ok(
            call(
                &f,
                &f.editor,
                HostCommand::DispatchOrchestration {
                    request: Box::new(stop),
                },
                0,
            ),
            "HF1_STOP_ACK",
        );
        let durable = f.projection(&run);
        assert!(
            matches!(durable.outcome, RunOutcome::Cancelled(_)),
            "HF1_STOP_DURABLE"
        );
        assert_eq!(
            durable.attempts[0].state,
            AttemptState::Stopping,
            "HF1_STOP_ATTEMPT"
        );
        assert!(
            f.editor
                .request(HostCommand::Status)
                .unwrap()
                .error
                .is_none(),
            "HF1_HOLDER_STILL_ATTACHED"
        );
    }
    gate[1].store(true, Ordering::Release);
    while !gate[2].load(Ordering::Acquire) {
        assert!(Instant::now() < deadline, "HF1_CONSENT_PROCESSED");
        std::thread::park_timeout(Duration::from_millis(2));
    }
    assert_eq!(pump.delivered.load(Ordering::SeqCst), 0, "{marker}");
}

#[test]
fn h3_stopped_run_revokes_queued_approval() {
    stopped_consent(
        kinewright_core::RuntimeRequestKind::Approval,
        "HF1_STOP_APPROVAL_DELIVERY",
        ConsentCut::Stop,
    );
}
#[test]
fn h3_stopped_run_revokes_queued_secret() {
    stopped_consent(
        kinewright_core::RuntimeRequestKind::Secret,
        "HF1_STOP_SECRET_DELIVERY",
        ConsentCut::Stop,
    );
}
#[test]
fn h3_queued_consent_deadline_is_checked_at_delivery() {
    stopped_consent(
        kinewright_core::RuntimeRequestKind::Approval,
        "HF1_CONSENT_DEADLINE",
        ConsentCut::Deadline,
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one live MCP server before and after the acknowledged retirement cut"
)]
fn h3_retired_attempt_refuses_existing_mcp_prepared_edit_and_replay() {
    use kinewright_core::{Track, TrackId, TrackKind};
    use rmcp::{
        ServiceExt as _,
        model::CallToolRequestParams,
        transport::{
            StreamableHttpClientTransport,
            streamable_http_client::StreamableHttpClientTransportConfig,
        },
    };
    use serde_json::json;
    let mut f = Fixture::with_document(
        false,
        Document {
            tracks: vec![Track {
                id: TrackId(1),
                kind: TrackKind::Video,
                sync_lock: true,
                clips: vec![],
            }],
            ..Document::default()
        },
    );
    f.brief
        .scope
        .operations
        .insert("set_track_sync_lock".into());
    f.brief.scope.objects.insert("track:1".into());
    f.brief.scope.fields.insert("track.sync_lock".into());
    f.brief.scope.tracks.insert("track:1".into());
    let (run, execution, producer) = reserve(&f);
    let (_actor, _endpoint) = task_actor(&f, &run, &execution, &producer);
    let (server, core) = {
        let owner = f.host.owner.upgrade().unwrap();
        let host = owner.lock().unwrap();
        let core = host.hooks.runs[&run].tasks[&execution.task].core().clone();
        (
            host.start_task_server(&producer.bootstrap.auth, &run, &execution.task)
                .unwrap(),
            core,
        )
    };
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
    let call_mcp = |name: &str, args: serde_json::Value| {
        runtime
            .block_on(
                client.call_tool(
                    CallToolRequestParams::new(name.to_owned())
                        .with_arguments(args.as_object().unwrap().clone()),
                ),
            )
            .unwrap()
    };
    let prepare = |revision, locked| json!({"expected_revision":revision,"operations":[{"op":"set_track_sync_lock","track":1,"locked":locked}]});
    let revision = grant_test_revision(&core);
    let first = call_mcp("prepare_edit_plan", prepare(revision, false));
    assert_eq!(
        first.is_error,
        Some(false),
        "HF1_MCP_POSITIVE_PREPARE: {first:?}"
    );
    let first = json!({"plan_id":first.structured_content.unwrap()["plan_id"],"expected_revision":revision});
    let applied = call_mcp("commit_edit_plan", first.clone());
    assert_eq!(
        applied.is_error,
        Some(false),
        "HF1_MCP_POSITIVE_COMMIT: {applied:?}"
    );
    let revision = grant_test_revision(&core);
    let pending = call_mcp("prepare_edit_plan", prepare(revision, true));
    assert_eq!(pending.is_error, Some(false), "HF1_MCP_PENDING_PREPARE");
    let pending = json!({"plan_id":pending.structured_content.unwrap()["plan_id"],"expected_revision":revision});
    let mut state = f.projection(&run);
    state.attempts[0].state = AttemptState::Retired(AttemptOutcome::Failed);
    state.cleanup = RunCleanup::Settled;
    assert_eq!(state.tasks[0].state, TaskState::Preparing);
    assert_eq!(state.phase, RunPhase::Running);
    assert_eq!(state.outcome, RunOutcome::Pending);
    let ack = publish_projection(&f, &producer, &run, state.clone(), None);
    assert_eq!(
        f.projection(&run).sequence,
        ack.sequence,
        "HF1_RETIREMENT_ACK"
    );
    assert_eq!(
        f.projection(&run).attempts,
        state.attempts,
        "HF1_RETIREMENT_DURABLE"
    );
    let before = core.export_history().unwrap();
    for (name, args, marker) in [
        (
            "commit_edit_plan",
            pending.clone(),
            "HF1_RETIRED_MCP_PREPARED",
        ),
        (
            "commit_edit_plan",
            pending,
            "HF1_RETIRED_MCP_PREPARED_REPLAY",
        ),
        (
            "commit_edit_plan",
            first,
            "HF1_RETIRED_MCP_COMMITTED_REPLAY",
        ),
        (
            "prepare_edit_plan",
            prepare(revision, true),
            "HF1_RETIRED_MCP_NEW_PREPARE",
        ),
    ] {
        let refused = call_mcp(name, args);
        assert_eq!(refused.is_error, Some(true), "{marker}: {refused:?}");
        assert!(
            refused.content.iter().any(|content| content
                .as_text()
                .is_some_and(|text| text.text.contains("task_authority_expired"))),
            "{marker}: {refused:?}"
        );
        assert_eq!(
            core.export_history().unwrap(),
            before,
            "{marker}: exact history"
        );
        assert_eq!(grant_test_revision(&core), revision, "{marker}: revision");
    }
    runtime.block_on(client.cancel()).unwrap();
}

#[test]
fn h3_queued_consent_durable_turn_is_checked_at_delivery() {
    stopped_consent(
        kinewright_core::RuntimeRequestKind::Approval,
        "HF1_CONSENT_DURABLE_TURN",
        ConsentCut::Turn,
    );
}
#[test]
fn h1_pending_hook_blocks_generic_retry_after_replacement() {
    let f = Fixture::new();
    let request = f.start_request(&f.editor);
    let uncertain = kinewright_project::io_observer::fail_once("root_parent_sync", || {
        call(
            &f,
            &f.editor,
            HostCommand::DispatchOrchestration {
                request: Box::new(request),
            },
            0,
        )
    });
    assert!(uncertain.error.is_some(), "HF1_RETRY_UNCERTAIN_SETUP");
    let before = state(&f);
    for ingress in 0..2 {
        let reply = call(&f, &f.editor, HostCommand::Retry, ingress);
        assert_eq!(
            reply.error.as_ref().map(|error| error.code.as_str()),
            Some("orchestration_refused"),
            "HF1_RETRY_ORIGINAL"
        );
        assert_eq!(state(&f), before, "HF1_RETRY_ORIGINAL_UNCHANGED");
    }
    ok(
        f.editor.request(HostCommand::Detach).unwrap(),
        "HF1_RETRY_DETACH",
    );
    let replacement = f.issuer.reattach_editor().unwrap();
    let before = state(&f);
    for ingress in 0..2 {
        let reply = call(&f, &replacement, HostCommand::Retry, ingress);
        assert_eq!(
            reply.error.as_ref().map(|error| error.code.as_str()),
            Some("orchestration_refused"),
            "HF1_RETRY_REPLACEMENT"
        );
        assert_eq!(state(&f), before, "HF1_RETRY_REPLACEMENT_UNCHANGED");
    }
}
