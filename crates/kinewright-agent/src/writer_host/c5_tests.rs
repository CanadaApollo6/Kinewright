#[test]
fn s4b_c5_unsaved_without_claim_has_no_cleanup_pending() {
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "c5-unsaved",
    );
    let machine = temp.path("machine");
    let host = WriterHost::start_inner(
        HostConfig {
            scheduler_settings: None,
            project: "unsaved".into(),
            path: None,
            roots: SessionRoots {
                read: vec![temp.root().into()],
                write: vec![temp.root().into()],
            },
            origin: HostOrigin::Headless,
        },
        HostAuth::generate().unwrap(),
        None,
        studio_runtime::RuntimePorts {
            fixture_claim: Some(machine.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    wait_runtime_recovery(&host);
    let status = host.client().request(HostCommand::Status).unwrap().status;
    assert_eq!(
        status.native_runtime_status, None,
        "C5_UNSAVED_NEEDS_NO_RECOVERY"
    );
    assert!(!machine.join("run.json").exists());
}

#[test]
fn s4b_c5_ci_protocol_child() {
    static CHILD: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    let binary = CHILD.get_or_init(|| {
        let path = kinewright_media::test_support::workspace_target_dir()
            .join(format!("c5-protocol-{}", std::process::id()));
        let status = std::process::Command::new("rustc")
            .args(["--edition", "2024"])
            .arg(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/ja_s5_protocol.rs"),
            )
            .arg("-o")
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        path
    });
    for mode in [
        "native-tool",
        "other-mcp",
        "other-tool",
        "malformed",
        "oversized",
        "duplicate-cache",
        "missing",
        "auth",
        "steering",
        "interrupt",
    ] {
        ci_protocol_case(binary, mode);
    }
}
fn ci_protocol_case(binary: &std::path::Path, mode: &str) {
    use crate::native_launcher::InvocationOwner;
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
    let mut child = std::process::Command::new(binary)
        .arg(mode)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let output = child.stdout.take().unwrap();
    let (sender, events) = crate::protocol::limits::event_channel();
    if mode == "steering" {
        port.steering(&execution, "later steering").unwrap();
        assert!(
            f.projection(&run)
                .effects
                .iter()
                .any(|e| e.state == EffectState::Pending)
        );
    }
    if mode == "interrupt" {
        sender.interrupt("fixture interruption");
    }
    let completed = crate::native_launcher::read_provider_output(output, &sender, |raw, n, eof| {
        port.raw(&execution, raw, n, eof)
    });
    let _ = child.kill();
    let _ = child.wait();
    let events: Vec<_> = events.try_iter().collect();
    assert!(
        !events.iter().any(|e| matches!(e, AgentEvent::Cost { .. })),
        "C5_NO_LEGACY_COST"
    );
    if matches!(
        mode,
        "native-tool" | "other-mcp" | "other-tool" | "malformed" | "oversized" | "interrupt"
    ) {
        assert!(sender.failed(), "C5_FRAME_REFUSAL {mode}");
    }
    if mode == "duplicate-cache" {
        assert!(completed);
    }
    let state = f.projection(&run);
    let mut owner = host.lock().unwrap();
    let accounting = owner.native_accounting(&state).unwrap().unwrap();
    assert_eq!(
        accounting.tasks[0].sends.len(),
        1,
        "C5_ONE_ACKNOWLEDGED_SEND {mode}"
    );
    if mode == "duplicate-cache" {
        assert_eq!(accounting.tasks[0].usage.len(), 1, "C5_USAGE_DEDUPLICATED");
        assert_eq!(accounting.tasks[0].totals.known.input, Some(20));
        assert_eq!(accounting.tasks[0].totals.known.cache_read, Some(10));
    }
    if mode == "auth" {
        assert!(!completed);
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentEvent::Error(_))),
            "C5_AUTH_REJECTION_REPORTED"
        );
        assert_eq!(
            accounting.tasks[0].repairs.len(),
            1,
            "C5_AUTH_FAILURE_RECORDED"
        );
    }
    if mode == "missing" {
        assert_eq!(accounting.tasks[0].totals.known.input, None);
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
    eprintln!("C5_CI_PROTOCOL mode={mode} charged=1 joined=true fixture_only=true");
    assert!(
        f.projection(&run)
            .attempts
            .iter()
            .all(|a| matches!(a.state, AttemptState::Retired(_))),
        "C5_CI_RETIRES_ONLY_FIXTURE_RESERVATION"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn s4b_c5_opening_prompt_contains_bound_brief_and_notes() {
    let f = Fixture::with_media(true);
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .qualification_director = true;
    let batch = manual_batch(&f);
    let request = submitted_request(&f, &batch);
    let result = revise_result(revise(&f, &f.editor, &request), "C5_PROMPT_REVISE");
    let kinewright_project::studio_records::ReviseResult::Accepted { run_id, .. } = result else {
        panic!("C5_PROMPT_ACCEPTED");
    };
    let state = f.projection(&run_of(&run_id));
    let owner = f.host.owner.upgrade().unwrap();
    let owner = owner.lock().unwrap();
    let assignment = owner
        .owner()
        .unwrap()
        .store()
        .unwrap()
        .read_evidence(&state.tasks[0].brief.assignment)
        .unwrap();
    let prompt = owner.codex_prompt(&state.tasks[0], &assignment).unwrap();
    let prompt: serde_json::Value = serde_json::from_slice(&prompt).unwrap();
    assert_eq!(
        prompt["brief"],
        serde_json::to_value(&state.tasks[0].brief).unwrap()
    );
    assert_eq!(
        prompt["notes"][0]["text"],
        "hook plan, script and assignment"
    );
    assert!(prompt["instructions"].as_str().unwrap().contains("Editor"));
    assert!(
        state.attempts.is_empty(),
        "C5_PROMPT_FIXTURE_NEVER_LAUNCHES"
    );
}
