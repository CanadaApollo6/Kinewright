// ---- C5 fix 11: the retry exemption matches only Codex's retry notices ----

/// Astra's fix 10 probe strings: terminal errors that mention the network
/// but are not Codex retry notices.
const C5_FIX11_TERMINAL: [&str; 4] = [
    "TLS certificate verification failed; aborting",
    "network access denied by policy; request failed",
    "Connection quota exhausted; request rejected",
    "reconnect is disabled; fatal provider error",
];

#[test]
fn s4b_c5_fix11_retry_notice_grammar() {
    use crate::writer_host::native_invocation::{codex_retry_notice, provider_failure};
    for notice in [
        "Reconnecting... 1/5",
        "Reconnecting... 5/5",
        "Reconnecting... 100/100",
        "Reconnecting... 2/5 (stream disconnected before completion: error sending request)",
        "Reconnecting... 3/5 (unexpected status 503 Service Unavailable (retry later))",
        "Reconnecting... waiting for network",
        "Reconnecting... waiting for network (error sending request for url)",
    ] {
        assert!(
            codex_retry_notice(notice),
            "C5_FIX11_NOTICE_REFUSED {notice}"
        );
        assert_eq!(
            provider_failure("error", &serde_json::json!({"message": notice})),
            None,
            "C5_FIX11_NOTICE_COUNTED {notice}"
        );
    }
    for text in C5_FIX11_TERMINAL.into_iter().chain([
        "Reconnecting... 6/5",
        "Reconnecting... 0/5",
        "Reconnecting... 01/5",
        "Reconnecting... 1/05",
        "Reconnecting... 1/101",
        "Reconnecting... 1000/1000",
        "Reconnecting... 1/",
        "Reconnecting... /5",
        "Reconnecting... 1/5 fatal",
        "Reconnecting... 1/5; aborting",
        "Reconnecting... 1/5 ()",
        "Reconnecting... 1/5 (unterminated",
        "Reconnecting... waiting for network; aborting",
        "Reconnecting... waiting",
        "reconnecting... 1/5",
        "Reconnecting...1/5",
        " Reconnecting... 1/5",
        "error: Reconnecting... 1/5",
        "stream disconnected before completion",
        "",
    ]) {
        assert!(!codex_retry_notice(text), "C5_FIX11_NOT_A_NOTICE {text:?}");
        assert!(
            provider_failure("error", &serde_json::json!({"message": text})).is_some(),
            "C5_FIX11_TERMINAL_EXEMPT {text:?}"
        );
    }
    // A failed turn counts whatever its text, a retry notice's included.
    for text in [
        "Reconnecting... 5/5",
        "network failure",
        C5_FIX11_TERMINAL[0],
    ] {
        assert!(
            provider_failure(
                "turn.failed",
                &serde_json::json!({"error":{"message": text}})
            )
            .is_some(),
            "C5_FIX11_FAILED_TURN_EXEMPT {text}"
        );
    }
}

/// Each of Astra's terminal errors, repeated, reaches Replan through the
/// production accounting path.
#[test]
fn s4b_c5_fix11_repeated_terminal_network_errors_reach_replan() {
    use kinewright_project::orchestration_accounting::AccountingRefusal;
    for text in C5_FIX11_TERMINAL {
        let f = Fixture::new();
        let (run, execution, _, producer, scratch, _charge) = c5_fix10_charged(&f);
        let raw = serde_json::json!({"type":"error","message":text});
        c5_fix10_raw(&f, &producer, &execution, &raw, 1).unwrap();
        assert!(
            c5_fix10_raw(&f, &producer, &execution, &raw, 2).is_err(),
            "C5_FIX11_TERMINAL_NOT_REFUSED {text}"
        );
        let record = spending_record(&f, &run);
        let task = &record.tasks[0];
        assert_eq!(
            task.pause,
            Some(AccountingRefusal::Replan),
            "C5_FIX11_TERMINAL_NOT_REPLAN {text}"
        );
        assert_eq!(task.repeated_failures, 2, "{text}");
        std::fs::remove_dir_all(scratch).unwrap();
    }
}

/// Codex's notices as its JSONL actually writes them (with the underlying
/// error in parentheses) stay exempt; the completed turn records usage.
#[test]
fn s4b_c5_fix11_detailed_retry_notices_stay_exempt() {
    let f = Fixture::new();
    let (run, execution, _, producer, scratch, _charge) = c5_fix10_charged(&f);
    let mut events = (1..=5)
        .map(|attempt| {
            serde_json::json!({"type":"error","message":format!(
                "Reconnecting... {attempt}/5 (stream disconnected before completion: error sending request)"
            )})
        })
        .collect::<Vec<_>>();
    events.push(serde_json::json!({"type":"error","message":"Reconnecting... waiting for network (error sending request for url)"}));
    events.push(serde_json::json!({"type":"turn.completed","id":"d","usage":{"input_tokens":20,"cached_input_tokens":10,"output_tokens":5}}));
    for (ordinal, raw) in events.iter().enumerate() {
        c5_fix10_raw(&f, &producer, &execution, raw, ordinal as u64 + 1).unwrap();
    }
    let record = spending_record(&f, &run);
    let task = &record.tasks[0];
    assert!(task.repairs.is_empty(), "C5_FIX11_NOTICE_REPAIRED");
    assert_eq!(task.last_failure, None);
    assert_eq!(task.pause, None);
    assert!(task.usage.iter().any(|sample| sample.complete));
    std::fs::remove_dir_all(scratch).unwrap();
}

/// Muse Minor 2: a refusal from an attempt the projection does not hold is
/// not counted, and no identity is invented for it.
#[test]
fn s4b_c5_fix11_refusal_needs_an_admitted_attempt() {
    use crate::orchestration::tasks::RuntimeTaskAuthority;
    use kinewright_project::{
        bundle_records::SessionId,
        orchestration_accounting::{ToolRefusal, ToolRefusalCode},
    };
    let f = Fixture::new();
    let (run, execution, _, _producer, scratch, _charge) = c5_fix10_charged(&f);
    let authority = |attempt: AttemptId, task: TaskId| RuntimeTaskAuthority {
        project: execution.project.clone(),
        run: run.clone(),
        task,
        attempt,
        endpoint: EndpointId(SessionId::fresh().unwrap()),
        owner_epoch: 0,
        expires_at: Instant::now() + Duration::from_secs(60),
    };
    let record = |f: &Fixture| spending_record(f, &run);
    let before = record(&f);
    let owner = f.host.owner.upgrade().unwrap();
    for stray in [
        authority(
            AttemptId(SessionId::fresh().unwrap()),
            execution.task.clone(),
        ),
        authority(
            execution.attempt.clone(),
            TaskId(SessionId::fresh().unwrap()),
        ),
    ] {
        owner
            .lock()
            .unwrap()
            .record_tool_refusal(&stray, "publish_result", ToolRefusalCode::StaleFreeze)
            .unwrap();
    }
    assert_eq!(record(&f), before, "C5_FIX11_STRAY_REFUSAL_COUNTED");
    owner
        .lock()
        .unwrap()
        .record_tool_refusal(
            &authority(execution.attempt.clone(), execution.task.clone()),
            "publish_result",
            ToolRefusalCode::StaleFreeze,
        )
        .unwrap();
    assert_eq!(
        record(&f).tasks[0].refusals,
        vec![ToolRefusal {
            tool: "publish_result".into(),
            code: ToolRefusalCode::StaleFreeze,
            count: 1
        }],
        "C5_FIX11_ADMITTED_REFUSAL_LOST"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

/// The real reader and guardian: Astra's four terminal errors. They are
/// repairs, the third reaches the round bound, and the run pauses visibly
/// with the provider killed.
#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c5_fix11_terminal_network_errors_pause_through_the_reader() {
    use super::super::studio_runtime::fixture::ProtocolFixtureMode;
    let (f, run, fixture) = c5_fix3_host(ProtocolFixtureMode::TerminalNetworkErrors);
    let (state, reason) = c5_fix10_visible_end(&f, &run);
    let _units = fixture
        .observations()
        .iter()
        .map(|observation| NativeUnitCleanup::new(&observation.identity))
        .collect::<Vec<_>>();
    assert_eq!(
        state.phase,
        RunPhase::Paused,
        "C5_FIX11_TERMINAL_PHASE {reason}"
    );
    assert_eq!(reason, super::super::studio_runtime::REPLAN_PAUSE);
    let observations = fixture.observations();
    std::thread::sleep(Duration::from_secs(4));
    let killed = !observations[0].scratch.join("survived").exists();
    assert!(killed, "C5_FIX11_TERMINAL_PROVIDER_NOT_KILLED");
    let record = spending_record(&f, &run);
    let task = &record.tasks[0];
    assert_eq!(
        task.pause,
        Some(kinewright_project::orchestration_accounting::AccountingRefusal::Replan)
    );
    assert_eq!(task.repairs[0].rounds, 2, "C5_FIX11_TERMINAL_ROUNDS");
    assert_eq!(task.sends.len(), 1);
    c5_fix3_stop(&f, &run, &fixture);
    eprintln!(
        "C5_FIX11_TERMINAL_REPLAN killed={killed} phase={:?} rounds=2 sends=1",
        state.phase
    );
}
