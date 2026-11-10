// Unpaid, isolated native profile checks and real durable host transitions.
fn b2b_unknown(stream: bool) {
    use std::os::unix::fs::PermissionsExt;
    let (f, _) = te15_fix_setup("codex");
    let (run, director, _) = c4_director(&f);
    fix4_assignment(&f, &run, true);
    let (mut pump, sent) = fix4_pump(&f, &run, false);
    pump.tick().unwrap();
    let before_sends = sent.lock().unwrap().len();
    let owner = f.host.owner.upgrade().unwrap();
    let producer = f.producer(&run);
    owner
        .lock()
        .unwrap()
        .wait_for_provider(&run, super::super::provider_readiness::CHECK_RETRY)
        .unwrap();
    let mut state = f.projection(&run);
    let child = state.tasks[1].id.clone();
    let identity = state
        .attempts
        .iter()
        .find(|a| a.identity.task == child)
        .unwrap()
        .identity
        .clone();
    state
        .effects
        .iter_mut()
        .find(|e| e.task == child)
        .unwrap()
        .state = EffectState::OutcomeUnknown;
    owner
        .lock()
        .unwrap()
        .publish_native(&producer, state, &identity, vec![])
        .unwrap();
    let objects = owner
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .store()
        .unwrap()
        .resolved_root()
        .unwrap()
        .with_extension("kinewright-assets")
        .join("studio/objects");
    let permissions = std::fs::metadata(&objects).unwrap().permissions();
    std::fs::set_permissions(&objects, std::fs::Permissions::from_mode(0o500)).unwrap();
    let failed = super::super::provider_readiness::wait_on_launch_error(
        &owner,
        &run,
        &std::io::Error::other("uncertain child"),
        false,
    );
    std::fs::set_permissions(&objects, permissions).unwrap();
    assert!(failed.is_err(), "B2B_WARNING_WRITE_FAILED");
    if stream {
        owner
            .lock()
            .unwrap()
            .record_stream_refusal(&producer, &director, "stream refusal")
            .unwrap();
    } else {
        owner
            .lock()
            .unwrap()
            .record_runtime_refusal(&producer, &run, "final refusal")
            .unwrap();
    }
    let page = b201_page(&f, &run);
    assert!(
        matches!(page.projection.wait, RunWait::Provider(_)),
        "B2B_UNKNOWN_PRIORITY"
    );
    assert!(
        page.wait_reason
            .as_deref()
            .is_some_and(|s| s.starts_with("The send outcome is unknown.")
                && s.contains("Press Stop, then Reopen")),
        "B2B_UNKNOWN_WORDS"
    );
    assert!(!page.allowed_actions.contains(&RunActionKind::Continue));
    let sequence = page.projection.sequence;
    owner
        .lock()
        .unwrap()
        .record_stream_refusal(&producer, &director, "later refusal")
        .unwrap();
    owner
        .lock()
        .unwrap()
        .record_runtime_refusal(&producer, &run, "later final")
        .unwrap();
    assert_eq!(
        f.projection(&run).sequence,
        sequence,
        "B2B_UNKNOWN_IDEMPOTENT"
    );
    pump.tick().unwrap();
    assert_eq!(
        sent.lock().unwrap().len(),
        before_sends,
        "B2B_NO_EXTRA_SEND"
    );
}
#[test]
fn te15_b2b_unknown_final() {
    if te15_fix_counting_child("te15_b2b_unknown_final") {
        b2b_unknown(false);
    }
}
#[test]
fn te15_b2b_unknown_stream() {
    if te15_fix_counting_child("te15_b2b_unknown_stream") {
        b2b_unknown(true);
    }
}
#[test]
fn te15_b2b_permanent_wait_precedence() {
    if !te15_fix_counting_child("te15_b2b_permanent_wait_precedence") {
        return;
    }
    for kind in ["files", "provider", "settings", "replan", "paused"] {
        let (f, _) = te15_fix_setup("codex");
        let run = f.start().envelope.identity("run").unwrap();
        let (mut pump, sent) = fix4_pump(&f, &run, false);
        pump.before_preparation = Some(Arc::new(|_| {
            Err(std::io::Error::other("transient fixture").into())
        }));
        pump.tick().unwrap();
        let mut state = f.projection(&run);
        let identity = state.attempts[0].identity.clone();
        let owner = f.host.owner.upgrade().unwrap();
        let producer = f.producer(&run);
        let reason = owner
            .lock()
            .unwrap()
            .owner_mut()
            .unwrap()
            .admit_host_grant(b"original relevant wait")
            .unwrap();
        let detail = WaitDetail {
            reason,
            missing: vec![],
            attempt: None,
        };
        state.wait = match kind {
            "files" => RunWait::Files(detail),
            "provider" => RunWait::Provider(detail),
            "settings" => RunWait::ProviderSettings(detail),
            "replan" => RunWait::Replan(detail),
            _ => RunWait::None,
        };
        if kind == "paused" {
            state.phase = RunPhase::Paused;
        }
        publish_projection(&f, &f.producer_client(&run), &run, state.clone(), None);
        owner
            .lock()
            .unwrap()
            .preparation_failed(
                &producer,
                &identity,
                super::super::task_preparation::PreparationKind::FrozenCheckpoint,
            )
            .unwrap();
        let page = b201_page(&f, &run);
        assert_eq!(
            page.projection.wait, state.wait,
            "B2B_PERMANENT_WAIT {kind}"
        );
        assert_eq!(page.projection.phase, state.phase, "B2B_PAUSE_WINS {kind}");
        assert!(
            !page.allowed_actions.contains(&RunActionKind::Continue),
            "B2B_PERMANENT_CONTINUE"
        );
        assert!(
            !page.allowed_actions.contains(&RunActionKind::SendMessage),
            "B2B_PERMANENT_MESSAGE"
        );
        assert!(sent.lock().unwrap().is_empty());
    }
}
#[test]
fn te15_b2b_replan_reason() {
    if !te15_fix_counting_child("te15_b2b_replan_reason") {
        return;
    }
    let (f, _) = te15_fix_setup("codex");
    let run = f.start().envelope.identity("run").unwrap();
    let owner = f.host.owner.upgrade().unwrap();
    let producer = f.producer(&run);
    let reference = owner
        .lock()
        .unwrap()
        .owner_mut()
        .unwrap()
        .admit_host_grant(b"specific replan")
        .unwrap();
    let mut state = f.projection(&run);
    state.wait = RunWait::Replan(WaitDetail {
        reason: reference,
        missing: vec![],
        attempt: None,
    });
    publish_projection(&f, &f.producer_client(&run), &run, state, None);
    let state = f.projection(&run);
    assert!(matches!(state.wait, RunWait::Replan(_)), "B2B_REPLAN_SETUP");
    owner
        .lock()
        .unwrap()
        .record_runtime_refusal(&producer, &run, "generic provider failure")
        .unwrap();
    assert_eq!(f.projection(&run).wait, state.wait, "B2B_REPLAN_REASON");
}
fn b2b_permanent_pump(kind: &str) {
    let (f, _) = te15_fix_setup("codex");
    let r = c5_fix10_run_from(f);
    if kind == "frozen" {
        c5_fix10_freeze(&r.editor, &r.child);
    }
    fix4_prior_work(&r.f, &r.run);
    b20_retire(&r.f, &r.run, &r.attempt);
    b20_retire(&r.f, &r.run, &r.parent);
    let owner = r.f.host.owner.upgrade().unwrap();
    let mut state = r.f.projection(&r.run);
    let mut history = owner
        .lock()
        .unwrap()
        .saved_task_history(&state, &r.child)
        .unwrap()
        .unwrap();
    let mut host = owner.lock().unwrap();
    state
        .tasks
        .iter_mut()
        .find(|t| t.id == r.child)
        .unwrap()
        .progress
        .decisions
        .retain(|reference| {
            reference.record_kind != kinewright_project::bundle_records::RecordKind::RunIndex
        });
    if kind != "missing" {
        if kind == "frozen" {
            history.source.head = kinewright_project::bundle_records::HeadId::fresh().unwrap();
            if let Some(result) = history.frozen_result.as_mut() {
                result.head = Some(history.source.clone());
            }
        } else {
            history.base_ref = history.candidate_base_ref.clone();
        }
        let record = host
            .owner_mut()
            .unwrap()
            .admit_scheduler_record(&OrchestrationRecord {
                version: 1,
                payload: OrchestrationPayload::TaskHistory(Box::new(history)),
            })
            .unwrap();
        state
            .tasks
            .iter_mut()
            .find(|t| t.id == r.child)
            .unwrap()
            .progress
            .decisions
            .push(record.reference.clone());
    }
    host.hooks
        .runs
        .get_mut(&r.run)
        .unwrap()
        .tasks
        .remove(&r.child);
    drop(host);
    state
        .tasks
        .iter_mut()
        .find(|t| t.id == r.child)
        .unwrap()
        .state = TaskState::Queued;
    state
        .effects
        .iter_mut()
        .find(|e| e.task == r.child)
        .unwrap()
        .state = EffectState::Pending;
    state.phase = RunPhase::Running;
    state.wait = RunWait::None;
    publish_projection(&r.f, &r.f.producer_client(&r.run), &r.run, state, None);
    let (mut pump, sent) = fix4_pump(&r.f, &r.run, false);
    pump.tick().unwrap();
    let page = b201_page(&r.f, &r.run);
    let expected = match kind {
        "frozen" => super::super::task_preparation::PreparationKind::FrozenCheckpoint,
        "missing" => super::super::task_preparation::PreparationKind::MissingHistory,
        _ => super::super::task_preparation::PreparationKind::HistoryBase,
    };
    assert_eq!(
        owner
            .lock()
            .unwrap()
            .preparation_block(&page.projection)
            .unwrap(),
        Some(expected),
        "B2B_REAL_PREPARATION_KIND {kind}"
    );
    assert!(sent.lock().unwrap().is_empty(), "B2B_PERMANENT_NO_SEND");
    assert!(
        !page.allowed_actions.contains(&RunActionKind::Continue)
            && !page.allowed_actions.contains(&RunActionKind::SendMessage)
    );
    assert_eq!(
        page.wait_reason.as_deref(),
        Some(expected.words()),
        "B2B_REAL_PERMANENT_WORDS"
    );
    ok(
        r.f.editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(r.f.control(&r.f.editor, &r.run, RunAction::Stop)),
            })
            .unwrap(),
        "B2B_STOP_PERMANENT_RUN",
    );
    let stopped = b201_page(&r.f, &r.run);
    assert!(matches!(
        stopped.projection.outcome,
        RunOutcome::Cancelled(_)
    ));
    let status =
        r.f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .retained_task_work()
            .unwrap();
    if kind == "frozen" {
        assert!(
            status.iter().any(|w| w.run == r.run && w.task == r.child),
            "B2B_STOP_RETAINS_WORK"
        );
    }
    assert!(
        expected
            .words()
            .contains("Save current edit, then open <branch>")
            && expected
                .words()
                .contains("Save current work, then open recovery point"),
        "B2B_STOP_RECOVERY_WORDS"
    );
}
#[test]
fn te15_b2b_real_frozen() {
    if te15_fix_counting_child("te15_b2b_real_frozen") {
        b2b_permanent_pump("frozen");
    }
}
#[test]
fn te15_b2b_real_missing() {
    if te15_fix_counting_child("te15_b2b_real_missing") {
        b2b_permanent_pump("missing");
    }
}
#[test]
fn te15_b2b_real_base() {
    if te15_fix_counting_child("te15_b2b_real_base") {
        b2b_permanent_pump("base");
    }
}
fn b2b_archive(path: &std::path::Path) -> serde_json::Value {
    let mut objects = serde_json::Map::new();
    for entry in std::fs::read_dir(
        path.with_extension("kinewright-assets")
            .join("studio/objects"),
    )
    .unwrap()
    {
        let entry = entry.unwrap();
        let name = entry.file_name().into_string().unwrap();
        if entry.file_type().unwrap().is_file() && name.len() == 64 {
            objects.insert(
                name,
                serde_json::Value::String(std::fs::read_to_string(entry.path()).unwrap()),
            );
        }
    }
    serde_json::json!({"root":std::fs::read_to_string(path).unwrap(),"objects":objects})
}
fn b2b_unpack(path: &std::path::Path, archive: &serde_json::Value) {
    let assets = path.with_extension("kinewright-assets");
    if assets.exists() {
        std::fs::remove_dir_all(&assets).unwrap();
    }
    let objects = assets.join("studio/objects");
    std::fs::create_dir_all(&objects).unwrap();
    for (name, bytes) in archive["objects"].as_object().unwrap() {
        assert_eq!(name.len(), 64);
        assert!(name.bytes().all(|b| b.is_ascii_hexdigit()));
        std::fs::write(objects.join(name), bytes.as_str().unwrap()).unwrap();
    }
    std::fs::write(path, archive["root"].as_str().unwrap()).unwrap();
}
/// Prerequisites are prepared runtime binaries/fixtures, persistent target scratch,
/// and the synthetic catalog installed by `te15_fix_counting_child`.
/// Complete CI targets pass after setup. Fresh solo retry readiness remains
/// unverified; see the bounded attempt in the B2b follow-ups report.
#[test]
fn te15_b2b_fork_crash() {
    const KEY: &str = "KINEWRIGHT_B2B_FORK_CRASH";
    if !te15_fix_counting_child("te15_b2b_fork_crash") {
        return;
    }
    if let Some(output) = std::env::var_os(KEY) {
        let (f, _) = te15_fix_setup("codex");
        let (run, parent, director) = c4_director(&f);
        let child = b20_assign(&f, &run, &parent, &director, Role::Editor);
        b20_retire(&f, &run, &parent);
        let owner = f.host.owner.upgrade().unwrap();
        owner.lock().unwrap().preparation_fault =
            Some(super::super::task_preparation::PreparationCheckpoint::History);
        let (mut pump, sent) = fix4_pump(&f, &run, false);
        pump.tick().unwrap();
        assert!(sent.lock().unwrap().is_empty());
        let state = f.projection(&run);
        let fork = owner
            .lock()
            .unwrap()
            .retained_preparation_fork(&state, &child)
            .unwrap()
            .expect("B2B_CRASH_INTENT_DURABLE");
        assert!(
            owner
                .lock()
                .unwrap()
                .saved_task_history(&state, &child)
                .unwrap()
                .is_none()
        );
        let payload = serde_json::json!({"source_root":f.temp.root(),"archive":b2b_archive(&f.temp.path("project.kinewright")),"brief":f.brief,"run":run,"child":child,"fork":fork});
        let output = std::path::PathBuf::from(output);
        let pending = output.with_extension("partial");
        std::fs::write(&pending, serde_json::to_vec(&payload).unwrap()).unwrap();
        std::fs::rename(pending, output).unwrap();
        // Abrupt exit: no host, actor, bundle, or TempDirectory destructors.
        std::process::exit(0);
    }
    let capture = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "b2b-fork-crash",
    );
    let output = capture.path("captured.json");
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "writer_host::hooks_tests::tests::te15_b2b_fork_crash",
        ])
        .env(KEY, &output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "crash child failed: {}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    let source = std::path::Path::new(value["source_root"].as_str().unwrap());
    assert!(source.starts_with(kinewright_media::test_support::workspace_target_dir()));
    std::fs::remove_dir_all(source).unwrap();
    let run: RunId = serde_json::from_value(value["run"].clone()).unwrap();
    let child: TaskId = serde_json::from_value(value["child"].clone()).unwrap();
    let fork: kinewright_project::bundle_records::BranchId =
        serde_json::from_value(value["fork"].clone()).unwrap();
    let (mut f, _) = te15_fix_setup("codex");
    f.brief = serde_json::from_value(value["brief"].clone()).unwrap();
    let claim = f.temp.path("crash-reopen");
    let f = c4_restart_with(f, &claim, |path| b2b_unpack(path, &value["archive"]));
    f.host.runtime.stop();
    let owner = f.host.owner.upgrade().unwrap();
    owner.lock().unwrap().native_readiness_checks = Some(Arc::new(Te15Readiness::default()));
    let state = f.projection(&run);
    assert_eq!(
        owner
            .lock()
            .unwrap()
            .retained_preparation_fork(&state, &child)
            .unwrap(),
        Some(fork.clone()),
        "B2B_FORK_SURVIVES_CRASH"
    );
    if b201_page(&f, &run)
        .allowed_actions
        .contains(&RunActionKind::Continue)
    {
        fix3_continue(&f, &run);
    }
    let (mut pump, sent) = fix4_pump(&f, &run, false);
    for _ in 0..4 {
        pump.tick()
            .expect("B2B_CRASH_RETRY_PREPARED: retry must prepare");
        if !sent.lock().unwrap().is_empty() {
            break;
        }
    }
    let state = f.projection(&run);
    let history = owner
        .lock()
        .unwrap()
        .saved_task_history(&state, &child)
        .unwrap()
        .expect("B2B_CRASH_RETRY_PREPARED");
    assert_eq!(history.source.branch, fork, "B2B_CRASH_FORK_REUSED");
    assert_eq!(sent.lock().unwrap().len(), 1, "B2B_CRASH_ONE_SEND");
}
#[cfg(feature = "te1-eval")]
#[test]
fn te15_b2b_native_publication_injection() {
    if !te15_fix_counting_child("te15_b2b_native_publication_injection") {
        return;
    }
    for finish in [false, true] {
        let (f, _) = te15_fix_setup("codex");
        let r = c5_fix10_run_from(f);
        let owner = r.f.host.owner.upgrade().unwrap();
        let report=br#"{"kind":"task_result_report_v1","partial_coverage":true,"unmet_note_ids":["correct-homophone"],"proof_status":"failed"}"#;
        let reference = owner
            .lock()
            .unwrap()
            .owner_mut()
            .unwrap()
            .admit_host_grant(report)
            .unwrap();
        let observer = Arc::new(crate::te1_eval::observe::Observations::new(
            5,
            Arc::default(),
        ));
        observer.set_native_report(reference.clone()).unwrap();
        let state = r.f.projection(&r.run);
        for attempt in &state.attempts {
            let index = state
                .tasks
                .iter()
                .position(|t| t.id == attempt.identity.task)
                .unwrap();
            observer
                .bind(crate::te1_eval::SendContext {
                    endpoint: String::new(),
                    identity: serde_json::to_value(&attempt.identity).unwrap(),
                    brief: serde_json::to_value(&state.tasks[index].brief).unwrap(),
                    prompt: String::new(),
                    fixtures: serde_json::Value::Null,
                    child_index: index,
                })
                .unwrap();
        }
        owner.lock().unwrap().evaluation_observer = Some(observer);
        if finish {
            let reply = r
                .editor
                .wire("finish_task", te13_control(&r.editor, &r.child, &r.core));
            assert_eq!(
                reply.is_error,
                Some(false),
                "B2B_FINISH_PUBLICATION {reply:?}"
            );
        } else {
            b20_publish(&r.f, &r.run, &r.child, &r.editor);
        }
        let state = r.f.projection(&r.run);
        let history = owner
            .lock()
            .unwrap()
            .saved_task_history(&state, &r.child)
            .unwrap()
            .unwrap();
        let frozen = history.frozen_result.unwrap();
        assert_eq!(
            frozen.explanation, reference,
            "B2B_NATIVE_REPORT_OWN_PUBLISH_PATH"
        );
        assert_eq!(state.results.len(), 1);
        let read = r.director.raw(
            "read_child_result",
            serde_json::json!({"run":r.run,"child":r.child,"reference":state.results[0],"source":frozen.head}),
        );
        assert_eq!(
            read.is_error,
            Some(false),
            "B2B_DIRECTOR_READS_NATIVE_REPORT {read:?}"
        );
        assert!(
            serde_json::to_string(&read)
                .unwrap()
                .contains("correct-homophone")
        );
    }
}

#[cfg(feature = "te1-eval")]
#[test]
fn te15_b2b_merge_replay_same_final_document() {
    if !te15_fix_counting_child("te15_b2b_merge_replay_same_final_document") {
        return;
    }
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
    assert!(home.starts_with(kinewright_media::test_support::workspace_target_dir()));
    std::fs::write(home.join(".codex/models_cache.json"),br#"{"models":[{"slug":"gpt-6.1-sol","supported_reasoning_levels":[{"effort":"high"},{"effort":"medium"}],"service_tiers":[{"id":"priority","name":"Fast"}]}]}"#).unwrap();
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "b2b-merge-order",
    );
    let measurement = crate::te1_eval::run_scripted(
        &temp.path("case5"),
        5,
        "codex",
        crate::te1_eval::ScriptedOutcome::Correct,
    )
    .unwrap();
    assert!(measurement.checks.passed(), "B2B_MERGE_ORIGINAL_ORACLE");
    let document = measurement.history.clone();
    let mut trace = measurement.trace.clone();
    let read = trace
        .calls
        .iter()
        .find(|c| {
            matches!(c.tool.as_str(), "read_task_result" | "read_child_result")
                && serde_json::to_string(&c.reply)
                    .unwrap()
                    .contains("correct-homophone")
                && serde_json::to_string(&c.reply).unwrap().contains("failed")
        })
        .unwrap()
        .clone();
    let replay = trace
        .calls
        .iter()
        .find(|c| c.tool == "merge_task" && !c.refused)
        .unwrap()
        .clone();
    trace
        .calls
        .retain(|c| !matches!(c.tool.as_str(), "read_task_result" | "read_child_result"));
    let first = trace
        .calls
        .iter()
        .position(|c| c.tool == "merge_task" && !c.refused)
        .unwrap();
    trace.calls.insert(first, read.clone());
    trace.calls.push(replay.clone());
    assert!(
        crate::te1_eval::checks::observed(5, &trace),
        "B2B_REAL_READ_MERGE_REPLAY"
    );
    trace.calls.remove(first);
    trace.calls.pop();
    trace.calls.push(read);
    trace.calls.push(replay);
    assert!(
        !crate::te1_eval::checks::observed(5, &trace),
        "B2B_REAL_MERGE_READ_REPLAY_REFUSED"
    );
    assert_eq!(measurement.history, document, "B2B_SAME_FINAL_DOCUMENT");
}
