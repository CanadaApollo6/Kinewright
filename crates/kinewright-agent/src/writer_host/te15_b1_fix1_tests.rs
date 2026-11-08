use crate::orchestration::adapters::{
    CertificationRefusal, ConnectorRuntimeChecks, ConnectorSpecification,
};
#[derive(Default)]
struct Te15Readiness {
    failed: Mutex<Option<String>>,
    checked: Mutex<Vec<String>>,
    host: std::sync::Weak<Mutex<HostCommandOwner>>,
}
impl ConnectorRuntimeChecks for Te15Readiness {
    fn preflight(&self, spec: &ConnectorSpecification) -> Result<(), CertificationRefusal> {
        if let Some(host) = self.host.upgrade() {
            assert!(host.try_lock().is_ok(), "TE15_FIX_PROBE_OUTSIDE_OWNER");
        }
        self.checked.lock().unwrap().push(spec.provider().into());
        if self.failed.lock().unwrap().as_deref() == Some(spec.provider()) {
            Err(CertificationRefusal::UnknownAuthentication)
        } else {
            Ok(())
        }
    }
    fn probe(
        &self,
        _: &ConnectorSpecification,
        _: &crate::orchestration::adapters::AttemptBinding,
    ) -> Result<crate::orchestration::adapters::RuntimeQualification, CertificationRefusal> {
        panic!("TE15_FIX_INSTALLED_PROBE");
    }
}
struct Te15CountingLauncher(Arc<AtomicUsize>);
struct Te15NoProcess(crossbeam_channel::Receiver<kinewright_core::AgentEvent>);
impl kinewright_core::AgentSession for Te15NoProcess {
    fn send_user_message(&mut self, _: String) -> Result<(), kinewright_core::AgentError> {
        Ok(())
    }
    fn events(&self) -> crossbeam_channel::Receiver<kinewright_core::AgentEvent> {
        self.0.clone()
    }
    fn interrupt(&mut self) {}
}
impl crate::orchestration::adapters::GuardianLauncher for Te15CountingLauncher {
    fn launch(
        &self,
        _: crate::orchestration::AdmittedAttempt,
        _: crate::orchestration::adapters::AuthorizedInvocation,
    ) -> Result<
        crate::orchestration::adapters::ContainedSession,
        crate::orchestration::adapters::LaunchError,
    > {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(
            crate::orchestration::adapters::ContainedSession::from_guardian(Box::new(
                Te15NoProcess(crossbeam_channel::unbounded().1),
            )),
        )
    }
}
struct Te15Profiles {
    runtime: Arc<super::super::studio_runtime::StudioRuntime>,
    host: std::sync::Weak<Mutex<HostCommandOwner>>,
    servers: Mutex<Vec<crate::McpServer>>,
    scratch: Mutex<Vec<std::path::PathBuf>>,
}
struct Te15ProfileChecks;
impl ConnectorRuntimeChecks for Te15ProfileChecks {
    fn probe(
        &self,
        spec: &ConnectorSpecification,
        binding: &crate::orchestration::adapters::AttemptBinding,
    ) -> Result<crate::orchestration::adapters::RuntimeQualification, CertificationRefusal> {
        Ok(te15_profile_runtime(
            spec,
            binding,
            std::path::Path::new("/usr/bin/true"),
        ))
    }
}
fn te15_profile_runtime(
    spec: &ConnectorSpecification,
    binding: &crate::orchestration::adapters::AttemptBinding,
    image: &std::path::Path,
) -> crate::orchestration::adapters::RuntimeQualification {
    use crate::orchestration::adapters::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let auth = binding.working_directory().with_extension("fixture-auth");
    std::fs::create_dir(&auth).unwrap();
    std::fs::set_permissions(&auth, std::fs::Permissions::from_mode(0o700)).unwrap();
    let source = auth.join("auth.json");
    std::fs::write(&source, br#"{"auth_mode":"chatgpt"}"#).unwrap();
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600)).unwrap();
    let metadata = std::fs::metadata(&source).unwrap();
    let mut runtime = crate::orchestration::adapters::test_runtime();
    runtime.version = spec.minimum_version();
    runtime.sandbox = Some(SandboxPolicy::LinuxFilesystemAndIpc);
    let executable = std::fs::metadata(image).unwrap();
    runtime.executable = ExecutableIdentity {
        path: image.into(),
        file_identity: [executable.dev(), executable.ino()],
    };
    runtime.launch_context.executable = runtime.executable.clone();
    let scope = &mut runtime.launch_context.credential_store;
    if spec.provider() == "codex" {
        scope.root = crate::codex_credentials::prepare_home(binding.working_directory()).unwrap();
        scope.single_file = Some(CredentialFile {
            source,
            identity: [metadata.dev(), metadata.ino()],
        });
        scope.credential_files = vec!["auth.json".into()];
    } else {
        assert_eq!(spec.provider(), "claude");
        scope.root = binding.working_directory().join("claude-config");
        std::fs::create_dir(&scope.root).unwrap();
        std::fs::set_permissions(&scope.root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let private = scope.root.join(".credentials.json");
        // Synthetic subscription evidence, never an installed provider's sign-in.
        std::fs::write(&private, br#"{"claudeAiOauth":{"accessToken":"fixture-only-token","expiresAt":9000000000000,"scopes":["user:inference"],"subscriptionType":"max"}}"#).unwrap();
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o600)).unwrap();
        scope.single_file = None;
        scope.credential_files = vec![".credentials.json".into()];
    }
    scope.lock_files.clear();
    scope.temporary_file_prefixes.clear();
    scope.refresh = spec.auth_contract().refresh;
    runtime.authentication.contract = spec.auth_contract();
    runtime.authentication.context = runtime.launch_context.clone();
    runtime.authentication.observed_store = runtime.launch_context.credential_store.clone();
    let program = crate::native_launcher::recipe(
        spec,
        binding,
        &runtime,
        &spec.session_config(binding),
        "fake prompt",
    )
    .unwrap();
    runtime.launch_context.environment_digest = native_digest(&program.environment);
    let mut arguments = program.arguments;
    arguments.pop();
    runtime.launch_context.settings_digest = native_digest(&arguments);
    runtime.authentication.context = runtime.launch_context.clone();
    runtime
}

impl crate::effect_pump::ProfileFactory for Te15Profiles {
    fn bind(
        &self,
        attempt: &crate::orchestration::AdmittedAttempt,
    ) -> std::io::Result<crate::orchestration::adapters::CertifiedConnectorProfile> {
        let host = self.host.upgrade().unwrap();
        let mut owner = host.lock().unwrap();
        let id = attempt.identity();
        let producer = owner.run_producer(&id.run).unwrap();
        owner
            .prepare_task_endpoint(
                &producer,
                &id.run,
                &id.task,
                &id.attempt,
                Duration::from_secs(300),
            )
            .unwrap();
        let server = owner
            .start_task_server(&producer, &id.run, &id.task)
            .unwrap();
        drop(owner);
        let profile = self.runtime.bind_profile_for_test(
            attempt,
            server.endpoint(),
            Arc::new(Te15ProfileChecks),
        )?;
        self.scratch.lock().unwrap().extend([
            profile.binding().working_directory().to_path_buf(),
            profile
                .binding()
                .working_directory()
                .with_extension("fixture-auth"),
        ]);
        self.servers.lock().unwrap().push(server);
        Ok(profile)
    }
}
impl Drop for Te15Profiles {
    fn drop(&mut self) {
        for path in self.scratch.get_mut().unwrap().drain(..) {
            std::fs::remove_dir_all(path).unwrap();
        }
    }
}
fn te15_fix_setup(provider: &str) -> (Fixture, Arc<Te15Readiness>) {
    let mut f = Fixture::new();
    f.host.runtime.stop();
    f.brief.provider = crate::orchestration::adapters::available_connectors()
        .iter()
        .find(|spec| spec.provider() == provider)
        .unwrap()
        .selection();
    let checks = Arc::new(Te15Readiness {
        host: f.host.owner.clone(),
        ..Te15Readiness::default()
    });
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .native_readiness_checks = Some(checks.clone());
    (f, checks)
}
fn te15_fix_specialist(f: &Fixture, provider: &str) {
    let selection = super::super::specialist_choices()
        .into_iter()
        .find(|s| s.connector == provider)
        .unwrap();
    super::super::write_specialist_setting(
        Some(&f.temp.path("scheduler")),
        &super::super::SpecialistChoice::Specific(selection),
    )
    .unwrap();
}
fn te15_fix_pump(
    f: &Fixture,
    run: &RunId,
    count: Arc<AtomicUsize>,
) -> crate::effect_pump::EffectPump<Te15CountingLauncher> {
    let owner = f.host.owner.upgrade().unwrap();
    let project = owner.lock().unwrap().spending_project().unwrap();
    let claim = Arc::new(Mutex::new(
        crate::execution_claim::MachineClaim::acquire_fixture(
            &f.temp.path("counting-claim"),
            project.as_str(),
            run.0.as_str(),
        )
        .unwrap(),
    ));
    crate::effect_pump::EffectPump::with_launcher(
        &owner,
        f.producer(run),
        run.clone(),
        claim,
        Te15CountingLauncher(count),
        Arc::new(Te15Profiles {
            runtime: f.host.runtime.clone(),
            host: Arc::downgrade(&owner),
            servers: Mutex::default(),
            scratch: Mutex::default(),
        }),
        None,
    )
}
// The real recipe resolves provider configuration roots even with a counting
// launcher. Isolate that lookup in a child test, without mutating the parallel
// test process's environment or relying on the person's provider directories.
fn te15_fix_counting_child(name: &str) -> bool {
    const CHILD: &str = "KINEWRIGHT_TE15_COUNTING_CHILD";
    if std::env::var(CHILD).as_deref() == Ok(name) {
        return true;
    }
    let temp = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "te15-counting-home",
    );
    for directory in [".codex", ".claude"] {
        std::fs::create_dir(temp.path(directory)).unwrap();
    }
    let models: Vec<_> = crate::orchestration::adapters::available_connectors()
        .into_iter()
        .filter(|spec| spec.provider() == "codex")
        .map(|spec| serde_json::json!({"slug":spec.selection().model}))
        .collect();
    std::fs::write(
        temp.path(".codex/models_cache.json"),
        serde_json::to_vec(&serde_json::json!({"models":models})).unwrap(),
    )
    .unwrap();
    let output_path = temp.path("child.log");
    let output = std::fs::File::create(&output_path).unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            &format!("writer_host::hooks_tests::tests::{name}"),
            "--nocapture",
            "--include-ignored",
        ])
        .env(CHILD, name)
        .env("HOME", temp.root())
        .env_remove("CODEX_HOME")
        .env_remove("OPENAI_API_KEY")
        .env("CLAUDE_CONFIG_DIR", temp.path(".claude"))
        .stdout(output.try_clone().unwrap())
        .stderr(output);
    for name in crate::claude_credentials::API_OVERRIDES {
        command.env_remove(name);
    }
    let mut child = command.spawn().unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(45);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let output = std::fs::read_to_string(output_path).unwrap();
    assert!(
        status.is_some_and(|s| s.success())
            && output
                .lines()
                .any(|line| line.starts_with("test result: ok. 1 passed; 0 failed; 0 ignored;")),
        "TE15_FIX_COUNTING_CHILD {name}: {output}"
    );
    for line in output
        .lines()
        .filter(|line| line.starts_with("TE15_FIX2_PROBE_COUNTS"))
    {
        println!("{line}");
    }
    false
}
fn te15_fix_preflight_case(provider: &str, failure: &str) {
    let (f, checks) = te15_fix_setup(provider);
    let other = if provider == "codex" {
        "claude"
    } else {
        "codex"
    };
    te15_fix_specialist(&f, other);
    let start = f.start();
    let run: RunId = start.envelope.identity("run").unwrap();
    match failure {
        "provider" => *checks.failed.lock().unwrap() = Some(other.into()),
        "malformed" => std::fs::write(f.temp.path("scheduler/specialists.json"), b"secret-file-derived-garbage").unwrap(),
        "retired" => std::fs::write(f.temp.path("scheduler/specialists.json"), br#"{"version":1,"specialists":{"kind":"specific","selection":{"connector":"claude","model":"retired-secret-model","options":[]}}}"#).unwrap(),
        _ => {}
    }
    let count = Arc::new(AtomicUsize::new(0));
    let mut pump = te15_fix_pump(&f, &run, count.clone());
    let result = pump.tick();
    if failure == "healthy" {
        assert!(result.is_ok(), "TE15_FIX_HEALTHY_LAUNCH {result:?}");
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "TE15_FIX_HEALTHY_LAUNCH"
        );
        assert!(checks.checked.lock().unwrap().iter().any(|p| p == other));
    } else {
        assert!(result.is_ok(), "TE15_FIX_ZERO_SEND_{failure}");
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "TE15_FIX_ZERO_SEND_{failure}"
        );
        let state = f.projection(&run);
        assert!(
            state.attempts.is_empty(),
            "TE15_FIX_PREFLIGHT_BEFORE_RESERVATION"
        );
        let RunWait::ProviderSettings(detail) = &state.wait else {
            panic!("TE15_FIX_SETUP_WAIT");
        };
        let reason = String::from_utf8(
            f.host
                .owner
                .upgrade()
                .unwrap()
                .lock()
                .unwrap()
                .owner()
                .unwrap()
                .store()
                .unwrap()
                .read_evidence(&detail.reason)
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(
            reason.contains("Settings"),
            "TE15_FIX_ACTIONABLE_WAIT {reason}"
        );
        if failure == "provider" {
            assert!(reason.contains(if other == "claude" { "Claude" } else { "Codex" }));
        }
        assert!(
            !reason.contains("secret"),
            "TE15_FIX_NO_SETTING_TEXT {reason}"
        );
        let sequence = state.sequence;
        let _ = super::super::provider_readiness::check_run(&f.host.owner.upgrade().unwrap(), &run);
        assert_eq!(f.projection(&run).sequence, sequence, "TE15_FIX_ONE_WAIT");
    }
}
#[test]
fn te15_b1_fix_codex_waits_for_claude_before_send() {
    if !te15_fix_counting_child("te15_b1_fix_codex_waits_for_claude_before_send") {
        return;
    }
    te15_fix_preflight_case("codex", "provider");
}
#[test]
fn te15_b1_fix_claude_waits_for_codex_before_send() {
    if !te15_fix_counting_child("te15_b1_fix_claude_waits_for_codex_before_send") {
        return;
    }
    te15_fix_preflight_case("claude", "provider");
}
#[test]
fn te15_b1_fix_malformed_waits_before_send() {
    if !te15_fix_counting_child("te15_b1_fix_malformed_waits_before_send") {
        return;
    }
    te15_fix_preflight_case("codex", "malformed");
}
#[test]
fn te15_b1_fix_retired_waits_before_send() {
    if !te15_fix_counting_child("te15_b1_fix_retired_waits_before_send") {
        return;
    }
    te15_fix_preflight_case("codex", "retired");
}
#[test]
fn te15_b1_fix_healthy_pair_launches() {
    if !te15_fix_counting_child("te15_b1_fix_healthy_pair_launches") {
        return;
    }
    te15_fix_preflight_case("codex", "healthy");
    te15_fix_preflight_case("claude", "healthy");
}
#[test]
fn te15_b1_fix_midrun_assignment_waits_without_another_send() {
    if !te15_fix_counting_child("te15_b1_fix_midrun_assignment_waits_without_another_send") {
        return;
    }
    let (f, checks) = te15_fix_setup("codex");
    te15_fix_specialist(&f, "claude");
    let run: RunId = f.start().envelope.identity("run").unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let mut pump = te15_fix_pump(&f, &run, count.clone());
    pump.tick().unwrap();
    let state = f.projection(&run);
    let mcp = C4Mcp::new(&f, &run, &state.tasks[0].id);
    let mut brief = f.brief.clone();
    brief.role = Role::Editor;
    let command = mcp.control(OrchestrationCommand::AssignTask(Box::new(AssignTask {
        run: run.clone(),
        parent: state.tasks[0].id.clone(),
        brief,
    })));
    *checks.failed.lock().unwrap() = Some("claude".into());
    let result = mcp.wire("assign_task", command);
    assert_eq!(result.is_error, Some(true), "TE15_FIX_MIDRUN_WAIT");
    let state = f.projection(&run);
    assert_eq!(state.phase, RunPhase::Interrupted, "TE15_FIX_MIDRUN_WAIT");
    assert_eq!(state.tasks.len(), 1);
    let sequence = state.sequence;
    for _ in 0..3 {
        let _ = pump.tick();
    }
    assert_eq!(
        count.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "TE15_FIX_NO_NEXT_DIRECTOR_SEND"
    );
    assert_eq!(
        f.projection(&run).sequence,
        sequence,
        "TE15_FIX_ONE_MIDRUN_WAIT"
    );
}
#[test]
fn te15_b1_fix_refused_assignment_replays_after_settings_change() {
    let f = Fixture::new();
    let (run, identity, mcp) = c4_director(&f);
    te15_fix_specialist(&f, "claude");
    let mut brief = f.brief.clone();
    brief.role = Role::Editor;
    let request = mcp.control(OrchestrationCommand::AssignTask(Box::new(AssignTask {
        run: run.clone(),
        parent: identity.task.clone(),
        brief,
    })));
    {
        let host = f.host.owner.upgrade().unwrap();
        let mut owner = host.lock().unwrap();
        let index = owner
            .attachments
            .iter()
            .position(|attachment| {
                attachment
                    .producer_scope
                    .as_ref()
                    .is_some_and(|scope| scope.attempt.as_ref() == Some(&identity.attempt))
            })
            .unwrap();
        let mut envelope = OrchestrationEnvelope {
            id: serde_json::from_value(request["id"].clone()).unwrap(),
            digest: serde_json::from_value(serde_json::json!("0".repeat(64))).unwrap(),
            project: identity.project.clone(),
            expected_generation: request["expected_generation"].as_u64().unwrap(),
            authority: AuthorityBinding {
                grant_ref: owner.expected_grant(index).unwrap(),
                owner_epoch: request["owner_epoch"].as_u64().unwrap(),
                endpoint: owner.attachments[index]
                    .producer_scope
                    .as_ref()
                    .unwrap()
                    .endpoint
                    .clone(),
                attempt: Some(identity.attempt.clone()),
            },
            command: serde_json::from_value(request["command"].clone()).unwrap(),
        };
        envelope.digest = envelope.computed_digest().unwrap();
        let bound = owner
            .bind_publication(&ScopedOrchestrationRequest {
                run: Some(run.clone()),
                envelope,
                consume_batch: None,
            })
            .unwrap();
        let mut state = owner
            .owner()
            .unwrap()
            .orchestration_projection(&run)
            .unwrap()
            .unwrap();
        state.sequence += 1;
        let segment = EventSegment {
            run: run.clone(),
            first_sequence: state.sequence,
            previous: Some(
                owner
                    .owner()
                    .unwrap()
                    .orchestration_cut(&run)
                    .unwrap()
                    .unwrap()
                    .segment,
            ),
            events: vec![OrchestrationEvent {
                command: bound.envelope.id.clone(),
                sequence: state.sequence,
                projection: state.clone(),
            }],
        };
        let transition = ValidatedRunUpdate::new(
            &identity.project,
            bound.envelope.expected_generation,
            &bound.envelope,
            state,
            segment,
            CommandDecision::Refused(
                crate::orchestration::orchestration_commit::RecordedRefusal::Unsupported,
            ),
        )
        .unwrap();
        crate::orchestration::orchestration_commit::commit_orchestration(
            owner.owner_mut().unwrap(),
            transition,
            None,
        )
        .unwrap();
    }
    let first = mcp.wire("assign_task", request.clone());
    assert_eq!(
        first.structured_content.as_ref().unwrap()["outcome"]["receipt"]["decision"]["kind"],
        "refused"
    );
    te15_fix_specialist(&f, "codex");
    let second = mcp.wire("assign_task", request.clone());
    assert_eq!(
        serde_json::to_vec(&first.structured_content.as_ref().unwrap()["outcome"]).unwrap(),
        serde_json::to_vec(&second.structured_content.as_ref().unwrap()["outcome"]).unwrap(),
        "TE15_FIX_REFUSED_REPLAY"
    );
}
#[test]
fn te15_b1_fix_assignment_omits_provider_on_the_wire() {
    let f = Fixture::new();
    let (run, identity, mcp) = c4_director(&f);
    te15_fix_specialist(&f, "claude");
    let mut brief = f.brief.clone();
    brief.role = Role::Editor;
    let mut request = mcp.control(OrchestrationCommand::AssignTask(Box::new(AssignTask {
        run: run.clone(),
        parent: identity.task.clone(),
        brief,
    })));
    request
        .pointer_mut("/command/value/brief")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("provider");
    let result = mcp.wire("assign_task", request);
    assert_eq!(
        result.is_error,
        Some(false),
        "TE15_FIX_NO_PROVIDER_REQUIRED {result:?}"
    );
    assert_eq!(
        f.projection(&run).tasks[1].brief.provider.connector,
        "claude"
    );
    let schema = super::super::task_mcp::schema::capability("assign_task");
    assert!(
        schema.pointer("/$defs/brief/properties/provider").is_none(),
        "TE15_FIX_SCHEMA_HOST_PROVIDER"
    );
    assert!(
        super::super::codex_instructions::CODEX_INSTRUCTIONS
            .contains("usage_ref and assignment brief.provider"),
        "TE15_FIX_DIGEST_WORDS"
    );
}

#[test]
fn te15_b1_fix_revise_and_direct_start_wait_before_spending() {
    let (f, checks) = te15_fix_setup("codex");
    te15_fix_specialist(&f, "claude");
    let folder = configure_director(&f);
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .fixture_director = false;
    let batch = manual_batch(&f);
    let request = submitted_request(&f, &batch);
    *checks.failed.lock().unwrap() = Some("claude".into());
    let result = revise_result(
        f.editor
            .request(HostCommand::ReviseCut { request })
            .unwrap(),
        "TE15_FIX_REVISE_WAIT",
    );
    assert!(
        matches!(result, ReviseResult::Waiting { ref reason } if reason.contains("Claude") && reason.contains("Settings")),
        "TE15_FIX_REVISE_WAIT {result:?}"
    );
    assert_eq!(consumed(&f, &batch), None);
    let start = f.start_request(&f.editor);
    let result = f
        .editor
        .request(HostCommand::DispatchOrchestration {
            request: Box::new(start),
        })
        .unwrap();
    assert!(
        result
            .error
            .as_ref()
            .is_some_and(|error| error.code == "provider_wait" && error.message.contains("Claude")),
        "TE15_FIX_START_WAIT {result:?}"
    );
    assert!(
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .hooks
            .runs
            .is_empty(),
        "TE15_FIX_ENTRY_ZERO_SENDS"
    );
    assert!(std::fs::metadata(folder.join("specialists.json")).is_ok());
}
#[test]
fn te15_b1_fix_continue_and_steering_recheck_choices() {
    let (f, checks) = te15_fix_setup("codex");
    let run: RunId = f.start().envelope.identity("run").unwrap();
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .wait_for_provider(&run, "Codex needs attention in Settings.")
        .unwrap();
    te15_fix_specialist(&f, "claude");
    *checks.failed.lock().unwrap() = Some("claude".into());
    let request = f.control(&f.editor, &run, RunAction::Continue);
    let refused = f
        .editor
        .request(HostCommand::DispatchOrchestration {
            request: Box::new(request),
        })
        .unwrap();
    assert!(refused.error.is_some(), "TE15_FIX_CONTINUE_WAIT");
    assert_eq!(
        f.projection(&run).phase,
        RunPhase::Interrupted,
        "TE15_FIX_CONTINUE_WAIT"
    );
    *checks.failed.lock().unwrap() = None;
    let request = f.control(&f.editor, &run, RunAction::Continue);
    ok(
        f.editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(request),
            })
            .unwrap(),
        "TE15_FIX_CONTINUE_REPAIRED",
    );
    assert_eq!(
        f.projection(&run).phase,
        RunPhase::Queued,
        "TE15_FIX_CONTINUE_REPAIRED"
    );
    std::fs::write(f.temp.path("scheduler/specialists.json"), b"invalid").unwrap();
    let request = f.control(
        &f.editor,
        &run,
        RunAction::SendMessage {
            text_ref: f.brief.assignment.clone(),
        },
    );
    let refused = f
        .editor
        .request(HostCommand::DispatchOrchestration {
            request: Box::new(request),
        })
        .unwrap();
    assert!(refused.error.is_some(), "TE15_FIX_STEERING_WAIT");
    assert_eq!(f.projection(&run).phase, RunPhase::Interrupted);
    assert!(
        f.projection(&run).attempts.is_empty(),
        "TE15_FIX_CONTINUE_ZERO_SENDS"
    );
}
#[test]
fn te15_b1_fix_assignment_without_a_run_refuses_at_binding() {
    let f = Fixture::new();
    let start = f.start_request(&f.editor);
    let mut request = start;
    request.envelope.command = OrchestrationCommand::AssignTask(Box::new(AssignTask {
        run: identity(),
        parent: identity(),
        brief: f.brief.clone(),
    }));
    request.envelope.digest = request.envelope.computed_digest().unwrap();
    let host = f.host.owner.upgrade().unwrap();
    let mut owner = host.lock().unwrap();
    assert!(
        owner.bind_publication(&request).is_err(),
        "TE15_FIX_UNBOUND_ASSIGNMENT"
    );
    request.run = Some(identity());
    assert!(
        owner.bind_publication(&request).is_err(),
        "TE15_FIX_UNKNOWN_RUN_ASSIGNMENT"
    );
}

#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn te15_b1_fix_bad_setting_keeps_the_live_director() {
    let f = Fixture::new();
    let live = LiveNative::start(&f);
    let mut brief = f.brief.clone();
    brief.role = Role::Editor;
    let request = f.request(
        &f.editor,
        Some(live.execution.run.clone()),
        OrchestrationCommand::AssignTask(Box::new(AssignTask {
            run: live.execution.run.clone(),
            parent: live.execution.task.clone(),
            brief,
        })),
    );
    std::fs::create_dir_all(f.temp.path("scheduler")).unwrap();
    std::fs::write(f.temp.path("scheduler/specialists.json"), b"broken").unwrap();
    let result = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .bind_publication(&request);
    let closed = live.control.closed();
    live.finish();
    assert!(result.is_err());
    assert!(!closed, "TE15_FIX2_LIVE_TURN_PRESERVED");
}
#[test]
fn te15_b1_fix_reopen_continue_checks_the_new_setting() {
    let (f, checks) = te15_fix_setup("codex");
    let run: RunId = f.start().envelope.identity("run").unwrap();
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
            scheduler_settings: Some(temp.path("scheduler")),
            project: "hooks".into(),
            path: Some(temp.path("project.kinewright")),
            roots: roots.clone(),
            origin: HostOrigin::Headless,
        },
        HostAuth::generate().unwrap(),
        Some(issuer_auth.clone()),
        super::super::studio_runtime::RuntimePorts {
            checks: checks.clone(),
            fixture_claim: Some(temp.path("reopen-machine")),
            ceiling: None,
            ..Default::default()
        },
    )
    .unwrap();
    wait_runtime_recovery(&host);
    host.runtime.stop();
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
    te15_fix_specialist(&f, "claude");
    *checks.failed.lock().unwrap() = Some("claude".into());
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .wait_for_provider(&run, "Review provider Settings.")
        .unwrap();
    let request = f.control(&f.editor, &run, RunAction::Continue);
    let reply = f
        .editor
        .request(HostCommand::DispatchOrchestration {
            request: Box::new(request),
        })
        .unwrap();
    assert!(
        reply
            .error
            .as_ref()
            .is_some_and(|error| error.message.contains("Claude")),
        "TE15_FIX_REOPEN_PREFLIGHT {reply:?}"
    );
    assert_eq!(f.projection(&run).phase, RunPhase::Interrupted);
    assert!(f.projection(&run).attempts.is_empty());
}

#[test]
fn te15_b1_fix_host_mcp_revise_waits_before_spending() {
    let (f, checks) = te15_fix_setup("codex");
    te15_fix_specialist(&f, "claude");
    configure_director(&f);
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .fixture_director = false;
    let batch = manual_batch(&f);
    let request = submitted_request(&f, &batch);
    *checks.failed.lock().unwrap() = Some("claude".into());
    let value = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"host_command","arguments":{"command":HostCommand::ReviseCut { request }}}});
    let response: serde_json::Value = serde_json::from_slice(
        &f.editor
            .post("mcp", &serde_json::to_vec(&value).unwrap())
            .unwrap(),
    )
    .unwrap();
    let reply: HostReply =
        serde_json::from_value(response["result"]["structuredContent"].clone()).unwrap();
    assert!(
        matches!(reply.revise_result, Some(ReviseResult::Waiting { ref reason }) if reason.contains("Claude") && reason.contains("Settings")),
        "TE15_FIX_HOST_MCP_WAIT {reply:?}"
    );
    assert_eq!(consumed(&f, &batch), None);
    assert_eq!(run_count(&f), 0);
}

#[test]
fn te15_b1_fix_replay_keeps_outcome_but_waits_on_broken_choice() {
    let f = Fixture::new();
    let (run, identity, mcp) = c4_director(&f);
    te15_fix_specialist(&f, "claude");
    let mut brief = f.brief.clone();
    brief.role = Role::Editor;
    let request = mcp.control(OrchestrationCommand::AssignTask(Box::new(AssignTask {
        run: run.clone(),
        parent: identity.task.clone(),
        brief,
    })));
    let first = mcp.wire("assign_task", request.clone());
    assert_eq!(first.is_error, Some(false));
    std::fs::write(f.temp.path("scheduler/specialists.json"), b"broken").unwrap();
    let replay = mcp.wire("assign_task", request);
    assert_eq!(
        serde_json::to_vec(&first.structured_content.as_ref().unwrap()["outcome"]).unwrap(),
        serde_json::to_vec(&replay.structured_content.as_ref().unwrap()["outcome"]).unwrap(),
        "TE15_FIX_REPLAY_ORIGINAL_OUTCOME"
    );
    let state = f.projection(&run);
    assert!(
        matches!(state.wait, RunWait::ProviderSettings(_)),
        "TE15_FIX_REPLAY_SETUP_WAIT"
    );
    assert_eq!(state.phase, RunPhase::Interrupted);
    assert_eq!(
        replay.structured_content.as_ref().unwrap()["controls"]["attempt"],
        serde_json::to_value(&identity.attempt).unwrap(),
        "TE15_FIX2_REPLAY_TURN_CONTROLS"
    );
}

#[test]
fn te15_b1_fix_pending_revise_reconciles_before_new_choices() {
    let syncs = {
        let f = Fixture::new();
        configure_director(&f);
        let batch = manual_batch(&f);
        let request = submitted_request(&f, &batch);
        let (reply, trace) = io_observer::observe(|| revise(&f, &f.editor, &request));
        revise_result(reply, "TE15_FIX_PENDING_PREPARE");
        trace
            .events
            .iter()
            .filter(|event| event.operation == "root_rename")
            .count()
    };
    let f = Fixture::new();
    configure_director(&f);
    let batch = manual_batch(&f);
    let request = submitted_request(&f, &batch);
    let unknown = io_observer::fail_nth("root_parent_sync", syncs, || {
        revise(&f, &f.editor, &request)
    });
    assert_eq!(
        unknown.error.as_ref().map(|error| error.code.as_str()),
        Some("durability_unknown")
    );
    std::fs::write(f.temp.path("scheduler/specialists.json"), b"broken").unwrap();
    let reply = f
        .editor
        .request(HostCommand::ReviseCut {
            request: request.clone(),
        })
        .unwrap();
    let result = revise_result(reply, "TE15_FIX_PENDING_REVISE_RECONCILES");
    let ReviseResult::Accepted { run_id, .. } = result else {
        panic!("TE15_FIX_PENDING_REVISE_RECONCILES {result:?}");
    };
    assert_eq!(consumed(&f, &batch).as_deref(), Some(run_id.as_str()));
    assert_eq!(run_count(&f), 1);
    assert!(f.projection(&run_of(&run_id)).attempts.is_empty());
    assert!(
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .hooks
            .pending
            .is_none()
    );
}
