// S4b C6: Claude Code as a Director, unpaid. The fast cases feed Claude's
// `stream-json` shapes through the real reader and the real host accounting;
// the on-demand cases run the protocol child in Claude's dialect under the
// real guardian, sealed image and Linux allowlist sandbox. No installed
// provider and no model send.

const C6_TOOLS: &str = r#""mcp__kinewright__get_timeline_state","mcp__kinewright__search_capabilities","mcp__kinewright__get_capability","mcp__kinewright__invoke_capability","mcp__kinewright__prepare_edit_plan","mcp__kinewright__commit_edit_plan","mcp__kinewright__discard_edit_plan""#;
fn c6_init(change: impl FnOnce(&mut serde_json::Value)) -> String {
    let mut init: serde_json::Value = serde_json::from_str(&format!(
        r#"{{"type":"system","subtype":"init","apiKeySource":"none","tools":[{C6_TOOLS}],"mcp_servers":[{{"name":"kinewright","status":"connected"}}],"permissionMode":"dontAsk","plugins":[],"skills":[],"agents":[],"slash_commands":[]}}"#
    ))
    .unwrap();
    change(&mut init);
    init.to_string()
}
const C6_TEXT: &str = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"fixture response"}]},"parent_tool_use_id":null}"#;
const C6_RESULT: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"done","usage":{"input_tokens":7,"output_tokens":3},"modelUsage":{"m":{"inputTokens":7,"outputTokens":3}}}"#;

/// One Claude stream case: the lines, the refusal the reader must give
/// (None: accepted), and whether the turn completed.
fn c6_cases() -> Vec<(&'static str, Vec<String>, Option<&'static str>, bool)> {
    use crate::native_launcher::{
        STREAM_AMBIENT_REFUSED as AMBIENT, STREAM_AUTH_REFUSED as AUTH,
        STREAM_MALFORMED as MALFORMED, STREAM_NATIVE_TOOL_REFUSED as NATIVE,
        STREAM_UNKNOWN_EVENT as UNKNOWN,
    };
    let normal = c6_init(|_| {});
    let line = |text: &str| text.to_owned();
    vec![
        (
            "normal",
            vec![normal.clone(), line(C6_TEXT), line(C6_RESULT)],
            None,
            true,
        ),
        (
            "mcp-tool-call",
            vec![
                normal.clone(),
                line(
                    r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":""},{"type":"tool_use","id":"t1","name":"mcp__kinewright__get_timeline_state","input":{}}]},"parent_tool_use_id":null}"#,
                ),
                line(
                    r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]},"parent_tool_use_id":null}"#,
                ),
                line(r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}"#),
                line(C6_RESULT),
            ],
            None,
            true,
        ),
        (
            "ambient-tool",
            vec![c6_init(|init| {
                init["tools"].as_array_mut().unwrap().push("Bash".into());
            })],
            Some(AMBIENT),
            false,
        ),
        (
            "ambient-server",
            vec![c6_init(|init| {
                init["mcp_servers"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({"name":"person","status":"connected"}));
            })],
            Some(AMBIENT),
            false,
        ),
        (
            "ambient-plugin",
            vec![c6_init(|init| {
                init["plugins"] = serde_json::json!([{"name":"p","path":"/p"}]);
            })],
            Some(AMBIENT),
            false,
        ),
        (
            "ambient-skill",
            vec![c6_init(|init| init["skills"] = serde_json::json!(["s"]))],
            Some(AMBIENT),
            false,
        ),
        (
            "ambient-permission",
            vec![c6_init(|init| {
                init["permissionMode"] = "bypassPermissions".into();
            })],
            Some(AMBIENT),
            false,
        ),
        (
            "hook",
            vec![line(
                r#"{"type":"system","subtype":"hook_started","hook_name":"SessionStart:startup"}"#,
            )],
            Some(AMBIENT),
            false,
        ),
        (
            "subagent",
            vec![
                normal.clone(),
                line(
                    r#"{"type":"assistant","message":{"content":[{"type":"text","text":"x"}]},"parent_tool_use_id":"toolu_1"}"#,
                ),
            ],
            Some(AMBIENT),
            false,
        ),
        (
            "background-task",
            vec![
                normal.clone(),
                line(r#"{"type":"system","subtype":"task_started","task_id":"t"}"#),
            ],
            Some(AMBIENT),
            false,
        ),
        (
            "apikey",
            vec![c6_init(|init| {
                init["apiKeySource"] = "ANTHROPIC_API_KEY".into();
            })],
            Some(AUTH),
            false,
        ),
        (
            "helper",
            vec![c6_init(|init| init["apiKeySource"] = "apiKeyHelper".into())],
            Some(AUTH),
            false,
        ),
        (
            "missing-key-source",
            vec![c6_init(|init| {
                init.as_object_mut().unwrap().remove("apiKeySource");
            })],
            Some(AUTH),
            false,
        ),
        (
            "native-tool",
            vec![
                normal.clone(),
                line(
                    r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t","name":"Bash","input":{}}]},"parent_tool_use_id":null}"#,
                ),
            ],
            Some(NATIVE),
            false,
        ),
        (
            "server-tool",
            vec![
                normal.clone(),
                line(
                    r#"{"type":"assistant","message":{"content":[{"type":"server_tool_use","id":"t","name":"web_search","input":{}}]},"parent_tool_use_id":null}"#,
                ),
            ],
            Some(NATIVE),
            false,
        ),
        (
            "other-mcp-tool",
            vec![
                normal.clone(),
                line(
                    r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t","name":"mcp__kinewright__delete_project","input":{}}]},"parent_tool_use_id":null}"#,
                ),
            ],
            Some(NATIVE),
            false,
        ),
        (
            "malformed",
            vec![normal.clone(), line("{\"type\":\"assistant\",")],
            Some(MALFORMED),
            false,
        ),
        (
            "overflow",
            vec![
                normal.clone(),
                format!(
                    "{{\"type\":\"assistant\",\"pad\":\"{}\"}}",
                    "x".repeat(96 * 1024)
                ),
            ],
            Some(MALFORMED),
            false,
        ),
        ("before-init", vec![line(C6_TEXT)], Some(MALFORMED), false),
        (
            "second-init",
            vec![normal.clone(), normal.clone()],
            Some(MALFORMED),
            false,
        ),
        (
            "auth-status",
            vec![
                normal.clone(),
                line(r#"{"type":"auth_status","isAuthenticating":true,"output":[]}"#),
            ],
            Some(AUTH),
            false,
        ),
        (
            "control-request",
            vec![
                normal.clone(),
                line(
                    r#"{"type":"control_request","request_id":"r","request":{"subtype":"can_use_tool"}}"#,
                ),
            ],
            Some(AMBIENT),
            false,
        ),
        (
            "unknown-type",
            vec![
                normal.clone(),
                line(r#"{"type":"session_title","title":"x"}"#),
            ],
            Some(UNKNOWN),
            false,
        ),
        (
            "partial-stream-event",
            vec![
                normal.clone(),
                line(r#"{"type":"stream_event","event":{}}"#),
            ],
            Some(UNKNOWN),
            false,
        ),
        (
            "unknown-system-subtype",
            vec![
                normal.clone(),
                line(r#"{"type":"system","subtype":"memory_saved"}"#),
            ],
            Some(UNKNOWN),
            false,
        ),
        (
            "unknown-block",
            vec![
                normal.clone(),
                line(
                    r#"{"type":"assistant","message":{"content":[{"type":"citation_card"}]},"parent_tool_use_id":null}"#,
                ),
            ],
            Some(UNKNOWN),
            false,
        ),
        (
            "ui-invalidate-mod",
            vec![line(
                r#"{"type":"system","subtype":"ui_invalidate","event":"ui.render","prompts_screened":true}"#,
            )],
            Some(AMBIENT),
            false,
        ),
        (
            "ui-invalidate-plugin-state",
            vec![line(
                r#"{"type":"system","subtype":"ui_invalidate","event":"ui.render","instances":[]}"#,
            )],
            Some(AMBIENT),
            false,
        ),
        (
            "ui-invalidate-unknown-field",
            vec![line(
                r#"{"type":"system","subtype":"ui_invalidate","event":"ui.render","note":"x"}"#,
            )],
            Some(UNKNOWN),
            false,
        ),
        (
            "ui-invalidate-unknown-event",
            vec![line(
                r#"{"type":"system","subtype":"ui_invalidate","event":"ui.toast"}"#,
            )],
            Some(UNKNOWN),
            false,
        ),
        (
            "ui-invalidate-before-init",
            vec![
                line(
                    r#"{"type":"system","subtype":"ui_invalidate","event":"ui.render","session_id":"s","uuid":"u"}"#,
                ),
                normal.clone(),
                line(C6_TEXT),
                line(C6_RESULT),
            ],
            None,
            true,
        ),
        (
            "known-informational",
            vec![
                normal.clone(),
                line(r#"{"type":"system","subtype":"api_retry","attempt":1}"#),
                line(r#"{"type":"keep_alive"}"#),
                line(C6_TEXT),
                line(C6_RESULT),
            ],
            None,
            true,
        ),
        ("eof", vec![normal.clone(), line(C6_TEXT)], None, false),
        (
            "error-result",
            vec![
                normal,
                line(
                    r#"{"type":"result","subtype":"success","is_error":true,"result":"API Error: Connection error."}"#,
                ),
            ],
            None,
            false,
        ),
    ]
}

/// The installed Claude recipe's fixed flags and complete environment: no
/// built-in tool schema, only the host MCP server, tool search off, no
/// person settings, and no API-key, helper, base URL or provider variable.
#[cfg(target_os = "linux")]
#[test]
fn s4b_c6_claude_argv_and_environment_are_isolated() {
    use crate::native_launcher::{claude_argv, claude_environment};
    let mut config = kinewright_core::SessionConfig {
        model: Some("claude-opus-5-5".into()),
        effort: Some("high".into()),
        ..Default::default()
    };
    config.tool_names = Some(vec![
        "get_timeline_state".into(),
        "invoke_capability".into(),
    ]);
    let mcp = serde_json::json!({"mcpServers":{"kinewright":{"type":"http","url":"http://127.0.0.1:1/mcp"}}});
    let argv = claude_argv(&mcp, &config, "the task").unwrap();
    let after = |flag: &str| {
        argv.iter()
            .position(|arg| arg == flag)
            .and_then(|at| argv.get(at + 1))
            .unwrap_or_else(|| panic!("C6_FLAG_MISSING {flag}"))
            .as_str()
    };
    assert_eq!(after("--tools"), "", "NO_BUILT_IN_TOOL_SCHEMA");
    assert!(
        argv.iter().any(|arg| arg == "--strict-mcp-config"),
        "ONLY_HOST_MCP"
    );
    assert!(
        argv.iter().any(|arg| arg == "--restricted"),
        "NO_PERSON_SETTINGS"
    );
    assert_eq!(after("--mcp-config"), mcp.to_string());
    assert_eq!(
        after("--allowedTools"),
        "mcp__kinewright__get_timeline_state,mcp__kinewright__invoke_capability"
    );
    let settings: serde_json::Value = serde_json::from_str(after("--settings")).unwrap();
    assert_eq!(settings["disableAllHooks"], true, "NO_HOOKS");
    assert_eq!(
        settings["forceLoginMethod"], "claudeai",
        "SUBSCRIPTION_SIGN_IN_ONLY"
    );
    assert_eq!(
        settings["enabledPlugins"],
        serde_json::json!({
            "cc-plugin-agents-md@builtin": false,
            "cc-plugin-privacy@builtin": false,
            "cc-plugin-plugin-authoring@builtin": false,
        }),
        "BUILTIN_PLUGINS_OFF"
    );
    assert_eq!(
        argv[argv.len() - 2..],
        ["--".to_owned(), "the task".to_owned()]
    );
    config.tool_names = None;
    assert!(
        claude_argv(&mcp, &config, "x").is_err(),
        "NO_TOOL_LIST_NO_LAUNCH"
    );
    let scratch = std::path::Path::new("/s");
    let store = std::path::Path::new("/s/claude");
    let environment = claude_environment(scratch, store);
    assert_eq!(
        environment["ENABLE_TOOL_SEARCH"], "false",
        "TOOL_SEARCH_OFF"
    );
    assert_eq!(environment["CLAUDE_CONFIG_DIR"], "/s/claude");
    assert_eq!(environment["HOME"], "/s");
    assert!(
        environment
            .keys()
            .all(|name| !name.starts_with("ANTHROPIC_")
                && !name.starts_with("AWS_")
                && !name.starts_with("GOOGLE_")
                && !matches!(
                    name.as_str(),
                    "CLAUDE_CODE_USE_BEDROCK"
                        | "CLAUDE_CODE_USE_VERTEX"
                        | "CLAUDE_CODE_OAUTH_TOKEN"
                )),
        "NO_API_OR_THIRD_PARTY_AUTH {environment:?}"
    );
}

/// A display queue nobody drains overflows: the reader stops reading the
/// provider at once, so its caller goes straight to guardian cleanup, and the
/// terminal `Error` and `Done` still fit in the slots reserved for them.
#[test]
fn s4b_c6_claude_reader_overflow_stops_reading_and_keeps_the_terminal() {
    use crate::native_launcher::{StreamDialect, read_provider_stream_as};
    use crate::protocol::limits::EVENTS;
    let mut lines = vec![c6_init(|_| {})];
    lines.extend(std::iter::repeat_n(C6_TEXT.to_owned(), EVENTS + 8));
    lines.push(C6_RESULT.to_owned());
    let mut bytes = lines.join("\n").into_bytes();
    bytes.push(b'\n');
    let (sender, events) = crate::protocol::limits::event_channel();
    let mut read_lines = 0_usize;
    let read = read_provider_stream_as(
        StreamDialect::Claude,
        std::io::Cursor::new(bytes),
        &sender,
        |_, _, eof| {
            read_lines += usize::from(!eof);
            Ok(())
        },
    );
    assert!(!read.completed, "C6_OVERFLOW_NEVER_COMPLETES");
    assert!(
        read_lines <= EVENTS,
        "C6_OVERFLOW_STOPS_READING read {read_lines} of {} lines",
        lines.len()
    );
    let events: Vec<_> = events.try_iter().collect();
    assert!(
        matches!(
            events.as_slice(),
            [.., AgentEvent::Error(reason), AgentEvent::Done] if reason.contains("event_capacity")
        ),
        "C6_OVERFLOW_KEEPS_THE_TERMINAL"
    );
}

#[test]
fn s4b_c6_claude_reader_and_accounting() {
    for (mode, lines, refusal, completed) in c6_cases() {
        c6_reader_case(mode, &lines, refusal, completed);
    }
}

fn c6_reader_case(mode: &str, lines: &[String], refusal: Option<&'static str>, completed: bool) {
    use crate::native_launcher::{InvocationOwner, StreamDialect};
    let f = Fixture::new();
    let (run, execution, actor) = reserve(&f);
    let admitted = native_admitted(&f, &execution);
    let (profile, _, scratch) = native_profile(&f, &admitted);
    let _invocation = native_charge(&f, &admitted, profile);
    let mut state = f.projection(&run);
    state.attempts[0].state = AttemptState::Contained;
    publish_projection(&f, &actor, &run, state, None);
    let host = f.host.owner.upgrade().unwrap();
    let producer = f.producer(&run);
    let port = super::super::native_invocation::HostInvocationOwner::new(&host, producer.clone());
    port.started(&execution).unwrap();
    let mut bytes = lines.join("\n").into_bytes();
    bytes.push(b'\n');
    let (sender, events) = crate::protocol::limits::event_channel();
    let read = crate::native_launcher::read_provider_stream_as(
        StreamDialect::Claude,
        std::io::Cursor::new(bytes),
        &sender,
        |raw, n, eof| port.raw_as(StreamDialect::Claude, &execution, raw, n, eof),
    );
    assert_eq!(read.refusal, refusal, "C6_READER_REFUSAL {mode}");
    assert_eq!(read.completed, completed, "C6_READER_COMPLETED {mode}");
    let events: Vec<_> = events.try_iter().collect();
    assert!(
        !events.iter().any(|e| matches!(e, AgentEvent::Cost { .. })),
        "C6_NO_DISPLAY_COST_REACHES_THE_RUN {mode}"
    );
    let state = f.projection(&run);
    let mut owner = host.lock().unwrap();
    let accounting = owner.native_accounting(&state).unwrap().unwrap();
    let task = &accounting.tasks[0];
    assert_eq!(task.sends.len(), 1, "C6_ONE_ACKNOWLEDGED_SEND {mode}");
    // Until TE1.1, Claude's usage is never guessed: every sample is unknown.
    assert!(
        task.usage.iter().all(|sample| sample.values
            == kinewright_project::orchestration_accounting::UsageValues::default()),
        "C6_CLAUDE_USAGE_UNKNOWN {mode}"
    );
    assert_eq!(task.totals.known.input, None, "C6_NO_GUESSED_INPUT {mode}");
    assert_eq!(
        task.totals.known.output, None,
        "C6_NO_GUESSED_OUTPUT {mode}"
    );
    if refusal.is_none() {
        assert!(
            task.usage.iter().any(|sample| sample.complete),
            "C6_TERMINAL_SAMPLE_COMPLETE {mode}"
        );
    }
    if mode == "error-result" {
        assert_eq!(task.repairs.len(), 1, "C6_FAILED_RESULT_IS_A_REPAIR");
        assert!(read.failed_turn, "C6_FAILED_RESULT_FAILS_THE_TURN");
    } else {
        assert!(task.repairs.is_empty(), "C6_NO_REPAIR {mode}");
    }
    owner
        .reconcile_native_effect(
            &producer,
            &execution,
            &crate::guardian::AttemptRetirementProof::NotDispatched(
                crate::native_launcher::native_identity(&execution),
            ),
        )
        .unwrap();
    drop(owner);
    std::fs::remove_dir_all(scratch).unwrap();
    eprintln!("C6_CLAUDE_READER mode={mode} refusal={refusal:?} completed={completed}");
}

/// An owner with no Claude accounting refuses Claude events, so a Claude
/// stream can never be counted as Codex usage.
#[test]
fn s4b_c6_owner_without_claude_accounting_refuses_the_stream() {
    struct CodexOnly;
    impl crate::native_launcher::InvocationOwner for CodexOnly {
        fn reserve(
            &self,
            _: &crate::orchestration::adapters::AuthorizedInvocation,
        ) -> std::io::Result<()> {
            Ok(())
        }
        fn release(
            &self,
            _: &crate::orchestration::adapters::AuthorizedInvocation,
            _: &crate::job_guardian::protocol::RecoveryIdentity,
            _: &crate::guardian::StartupControl,
        ) -> std::io::Result<()> {
            Ok(())
        }
        fn commit(
            &self,
            _: &crate::orchestration::adapters::AuthorizedInvocation,
            _: &crate::job_guardian::protocol::RecoveryIdentity,
            _: &mut dyn FnMut() -> std::io::Result<()>,
        ) -> std::io::Result<()> {
            Ok(())
        }
        fn started(&self, _: &ExecutionIdentity) -> std::io::Result<()> {
            Ok(())
        }
        fn raw(
            &self,
            _: &ExecutionIdentity,
            _: &serde_json::Value,
            _: u64,
            _: bool,
        ) -> std::io::Result<()> {
            Ok(())
        }
        fn completed(
            &self,
            _: &ExecutionIdentity,
            _: bool,
            _: &crate::guardian::CleanupProof,
        ) -> std::io::Result<()> {
            Ok(())
        }
        fn unbound_completed(
            &self,
            _: &ExecutionIdentity,
            _: &crate::guardian::UnboundCleanupProof,
        ) -> std::io::Result<()> {
            Ok(())
        }
        fn steering(&self, _: &ExecutionIdentity, _: &str) -> std::io::Result<()> {
            Ok(())
        }
    }
    use crate::native_launcher::{InvocationOwner, StreamDialect};
    let f = Fixture::new();
    let (_, identity, _) = reserve(&f);
    let (sender, _events) = crate::protocol::limits::event_channel();
    let read = crate::native_launcher::read_provider_stream_as(
        StreamDialect::Claude,
        std::io::Cursor::new(format!("{}\n", c6_init(|_| {}))),
        &sender,
        |raw, n, eof| CodexOnly.raw_as(StreamDialect::Claude, &identity, raw, n, eof),
    );
    assert_eq!(
        read.refusal,
        Some(crate::native_launcher::STREAM_ACCOUNTING_REFUSED),
        "C6_CLAUDE_NEVER_COUNTED_AS_CODEX"
    );
}

fn c6_host(
    mode: super::super::studio_runtime::fixture::ProtocolFixtureMode,
    event: Option<&'static str>,
) -> (
    Fixture,
    RunId,
    super::super::studio_runtime::fixture::ProtocolFixture,
) {
    use super::super::studio_runtime::fixture::{ProtocolFixture, claude_fixture_selection};
    let mut f = Fixture::new();
    f.brief.provider = claude_fixture_selection();
    let started = f.start();
    let run: RunId = started.envelope.identity("run").unwrap();
    let fixture = ProtocolFixture::claude(f.temp.root(), mode, event);
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
    wait_runtime_recovery(&host);
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
        "C6_PRODUCTION_DRIVE",
    );
    (f, run, fixture)
}
fn c6_units(
    fixture: &super::super::studio_runtime::fixture::ProtocolFixture,
) -> Vec<NativeUnitCleanup> {
    fixture
        .observations()
        .iter()
        .map(|observation| NativeUnitCleanup::new(&observation.identity))
        .collect()
}

/// The Claude-dialect child completes a turn under the real guardian with
/// exactly the recipe's environment, and its usage stays unknown.
#[test]
#[ignore = "on-demand: equipped Linux guardian test"]
fn s4b_c6_claude_fixture_turn_under_guardian() {
    use super::super::studio_runtime::fixture::ProtocolFixtureMode;
    let (f, run, fixture) = c6_host(ProtocolFixtureMode::Normal, None);
    c4_wait(
        || {
            if let Some(refusal) = c5_fix3_refusal(&f, &run) {
                panic!("C6_FIXTURE_REFUSED {refusal}");
            }
            spending_record(&f, &run)
                .tasks
                .iter()
                .any(|task| task.usage.iter().any(|sample| sample.complete))
        },
        "C6_CLAUDE_TURN_COMPLETED",
    );
    let _units = c6_units(&fixture);
    let observation = &fixture.observations()[0];
    let report: serde_json::Value = serde_json::from_slice(
        &std::fs::read(observation.scratch.join("env-report.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(report["tool_search"], "false", "C6_TOOL_SEARCH_OFF");
    assert_eq!(
        report["config_dir"],
        observation
            .scratch
            .join("credentials")
            .to_string_lossy()
            .as_ref(),
        "C6_PRIVATE_CONFIG_DIR"
    );
    assert_eq!(
        report["api_overrides"], 0,
        "C6_NO_API_OVERRIDE_REACHES_CLAUDE"
    );
    assert_eq!(report["token"], true, "C6_MCP_BEARER_FROM_GUARDIAN");
    let record = spending_record(&f, &run);
    let task = &record.tasks[0];
    assert_eq!(
        task.totals.known.input, None,
        "C6_USAGE_UNKNOWN_UNDER_GUARDIAN"
    );
    assert!(task.repairs.is_empty());
    c5_fix3_stop(&f, &run, &fixture);
    eprintln!("C6_CLAUDE_FIXTURE_TURN env={report}");
}

/// Every refused Claude stream goes to guardian cleanup with one fixed
/// reason: the child, which marks `survived` 3 s later, is killed first.
#[test]
#[ignore = "on-demand: equipped Linux guardian test"]
fn s4b_c6_claude_stream_refusals_under_guardian() {
    use super::super::studio_runtime::fixture::ProtocolFixtureMode;
    use crate::native_launcher::{
        STREAM_AMBIENT_REFUSED, STREAM_AUTH_REFUSED, STREAM_MALFORMED, STREAM_NATIVE_TOOL_REFUSED,
    };
    for (event, expected) in [
        ("ambient-tool", STREAM_AMBIENT_REFUSED),
        ("ambient-server", STREAM_AMBIENT_REFUSED),
        ("hook", STREAM_AMBIENT_REFUSED),
        ("subagent", STREAM_AMBIENT_REFUSED),
        ("apikey", STREAM_AUTH_REFUSED),
        ("nativetool", STREAM_NATIVE_TOOL_REFUSED),
        ("malformed", STREAM_MALFORMED),
        ("overflow", STREAM_MALFORMED),
    ] {
        let (f, run, fixture) = c6_host(ProtocolFixtureMode::Normal, Some(event));
        let (state, reason) = c5_fix10_visible_end(&f, &run);
        let _units = c6_units(&fixture);
        assert_eq!(
            reason, expected,
            "C6_FIXED_REFUSAL {event} {:?}",
            state.phase
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
            "C6_REFUSED_ATTEMPT_RETIRED",
        );
        std::thread::sleep(Duration::from_secs(4));
        let observations = fixture.observations();
        assert_eq!(observations.len(), 1, "C6_NO_RELAUNCH {event}");
        assert!(
            !observations[0].scratch.join("survived").exists(),
            "C6_REFUSED_PROVIDER_NOT_KILLED {event}"
        );
        c5_fix3_stop(&f, &run, &fixture);
        eprintln!("C6_CLAUDE_REFUSAL event={event} reason={reason:?} killed=true");
    }
}

/// Stop reaches the whole provider unit through the guardian, not only the
/// host's direct child: a grandchild heartbeat stops with it.
#[test]
#[ignore = "on-demand: equipped Linux guardian test"]
fn s4b_c6_claude_stop_cancels_the_whole_unit() {
    use super::super::studio_runtime::fixture::ProtocolFixtureMode;
    let (f, run, fixture) = c6_host(ProtocolFixtureMode::Normal, Some("descendant"));
    c4_wait(
        || {
            fixture.observations().first().is_some_and(|o| {
                if let Ok(error) = std::fs::read_to_string(o.scratch.join("heartbeat-error")) {
                    panic!("C6_DESCENDANT_SPAWN_REFUSED {error}");
                }
                o.scratch.join("heartbeat").exists()
            })
        },
        "C6_DESCENDANT_RUNNING",
    );
    let _units = c6_units(&fixture);
    let scratch = fixture.observations()[0].scratch.clone();
    let exec_errno = std::fs::read_to_string(scratch.join("exec-errno")).unwrap_or_default();
    assert_eq!(exec_errno, "13", "C6_PROVIDER_CANNOT_EXEC");
    let beat = scratch.join("heartbeat");
    let stop = f.control(&f.editor, &run, RunAction::Stop);
    ok(
        f.editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(stop),
            })
            .unwrap(),
        "C6_STOP_APPLIED",
    );
    let stopped = Instant::now();
    let modified = || std::fs::metadata(&beat).and_then(|m| m.modified()).ok();
    let mut last = modified();
    let mut quiet_since = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(200));
        let now = modified();
        if now != last {
            last = now;
            quiet_since = Instant::now();
        }
        if quiet_since.elapsed() >= Duration::from_secs(1) {
            break;
        }
        assert!(
            stopped.elapsed() < Duration::from_secs(15),
            "C6_DESCENDANT_SURVIVED_STOP"
        );
    }
    // The guardian's own cancellation empties the unit within its cleanup
    // deadline. A later backstop (the guardian giving up, then the unit
    // being removed) also silences the descendant, but only after it.
    let beating_after_stop = quiet_since.saturating_duration_since(stopped);
    assert!(
        beating_after_stop < crate::job_guardian::protocol::CLEANUP,
        "C6_DESCENDANT_OUTLIVED_GUARDIAN_CLEANUP {beating_after_stop:?}"
    );
    c4_wait(
        || {
            let _ = f.editor.request(HostCommand::Status);
            !fixture.claim_root().join("run.json").exists()
        },
        "C6_STOP_CLAIM_FINISHED",
    );
    assert_eq!(
        f.projection(&run).cleanup,
        RunCleanup::Settled,
        "C6_STOP_SETTLED"
    );
    fixture.cleanup().unwrap();
    eprintln!(
        "C6_CLAUDE_STOP_WHOLE_UNIT quiet_after_ms={} provider_exec_errno={exec_errno}",
        stopped.elapsed().as_millis()
    );
}

/// A full native queue does not lose Stop for a Claude-dialect provider.
#[test]
#[ignore = "on-demand: equipped Linux guardian test"]
fn s4b_c6_claude_full_queue_keeps_stop() {
    use super::super::studio_runtime::fixture::ProtocolFixtureMode;
    let (f, run, fixture) = c6_host(ProtocolFixtureMode::FullQueue, None);
    c4_wait(
        || fixture.event_entered() && fixture.native_queue_full(),
        "C6_REAL_FULL_NATIVE_QUEUE",
    );
    let _units = c6_units(&fixture);
    let stop = f.control(&f.editor, &run, RunAction::Stop);
    let reply = ok(
        f.editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(stop),
            })
            .unwrap(),
        "C6_STOP_ADMITTED_WITH_FULL_QUEUE",
    );
    assert_eq!(
        reply.orchestration.unwrap().decision,
        CommandDecision::Applied
    );
    fixture.release();
    c4_wait(
        || {
            let _ = f.editor.request(HostCommand::Status);
            !fixture.claim_root().join("run.json").exists()
        },
        "C6_FULL_QUEUE_CLEANUP",
    );
    assert_eq!(f.projection(&run).cleanup, RunCleanup::Settled);
    fixture.cleanup().unwrap();
    eprintln!("C6_CLAUDE_FULL_QUEUE_STOP settled=true");
}

/// Claude `-p` reads a piped stdin first; the host's stdin ends at launch.
/// A Claude provider that never speaks meets the first-progress deadline.
#[test]
#[ignore = "on-demand: equipped Linux guardian test"]
fn s4b_c6_claude_stdin_eof_and_first_progress() {
    use super::super::studio_runtime::fixture::ProtocolFixtureMode;
    let (f, run, fixture) = c6_host(ProtocolFixtureMode::ReadStdinFirst, None);
    c4_wait(
        || {
            if let Some(refusal) = c5_fix3_refusal(&f, &run) {
                panic!("C6_STDIN_HELD {refusal}");
            }
            fixture
                .observations()
                .first()
                .is_some_and(|o| o.scratch.join("stdin-eof").exists())
        },
        "C6_STDIN_EOF",
    );
    let units = c6_units(&fixture);
    c5_fix3_stop(&f, &run, &fixture);
    drop(units);
    let (f, run, fixture) = c6_host(ProtocolFixtureMode::SilentAlive, None);
    let started = Instant::now();
    let refusal = loop {
        if let Some(refusal) = c5_fix3_refusal(&f, &run) {
            break refusal;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "C6_SILENT_PROVIDER_WAITS"
        );
        std::thread::park_timeout(Duration::from_millis(5));
    };
    let _units = c6_units(&fixture);
    assert_eq!(
        refusal,
        crate::native_launcher::first_progress_failure(ProtocolFixtureMode::SHORT_FIRST_PROGRESS),
        "C6_FIRST_PROGRESS_REASON"
    );
    c5_fix3_stop(&f, &run, &fixture);
    eprintln!("C6_CLAUDE_STDIN_AND_FIRST_PROGRESS reason={refusal:?}");
}
