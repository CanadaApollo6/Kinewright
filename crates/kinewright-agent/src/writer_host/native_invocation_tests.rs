fn native_digest(value: &impl serde::Serialize) -> [u8; 32] {
    let digest = kinewright_media::sha256_bytes(&serde_json::to_vec(value).unwrap());
    std::array::from_fn(|index| u8::from_str_radix(&digest[index * 2..index * 2 + 2], 16).unwrap())
}
struct NativeChecks(crate::orchestration::adapters::RuntimeQualification);
impl crate::orchestration::adapters::ConnectorRuntimeChecks for NativeChecks {
    fn probe(
        &self,
        _: &crate::orchestration::adapters::ConnectorSpecification,
        _: &crate::orchestration::adapters::AttemptBinding,
    ) -> Result<
        crate::orchestration::adapters::RuntimeQualification,
        crate::orchestration::adapters::CertificationRefusal,
    > {
        Ok(self.0.clone())
    }
}
fn native_child() -> std::path::PathBuf {
    static CHILD: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    CHILD
        .get_or_init(|| {
            let path = kinewright_media::test_support::workspace_target_dir()
                .join(format!("s4b-protocol-{}", std::process::id()));
            let status = std::process::Command::new("rustc")
                .args(["--edition", "2024"])
                .arg(
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("tests/fixtures/ja_s4_native.rs"),
                )
                .arg("-o")
                .arg(&path)
                .status()
                .unwrap();
            assert!(status.success());
            path
        })
        .clone()
}
fn native_admitted(
    f: &Fixture,
    identity: &ExecutionIdentity,
) -> crate::orchestration::AdmittedAttempt {
    let owner = f.host.owner.upgrade().unwrap();
    let host = owner.lock().unwrap();
    let owner = host.owner().unwrap();
    let cut = owner.orchestration_cut(&identity.run).unwrap().unwrap();
    let record = owner
        .store()
        .unwrap()
        .read_record::<OrchestrationRecord>(&cut.segment)
        .unwrap();
    let OrchestrationPayload::EventSegment(segment) = &record.payload else {
        panic!("native event segment");
    };
    let update = ValidatedRunUpdate::new(
        &cut.receipt.envelope.project,
        cut.receipt.envelope.expected_generation,
        &cut.receipt.envelope,
        owner
            .orchestration_projection(&identity.run)
            .unwrap()
            .unwrap(),
        segment.clone(),
        cut.receipt.decision.clone(),
    )
    .unwrap();
    crate::orchestration::acknowledge_attempt_reservation(&update, &cut.receipt, identity).unwrap()
}
fn native_profile(
    f: &Fixture,
    admitted: &crate::orchestration::AdmittedAttempt,
) -> (
    crate::orchestration::adapters::CertifiedConnectorProfile,
    NativeChecks,
    std::path::PathBuf,
) {
    use crate::orchestration::adapters::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let scratch = kinewright_media::test_support::workspace_target_dir().join(format!(
        "n2-{}",
        admitted.identity().nonce.as_str().get(..8).unwrap()
    ));
    std::fs::create_dir(&scratch).unwrap();
    std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o700)).unwrap();
    let credentials = scratch.join("credentials");
    std::fs::create_dir(&credentials).unwrap();
    std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o700)).unwrap();
    let binding = AttemptBinding::issue(
        admitted,
        "http://127.0.0.1:12345/mcp".into(),
        vec![f.temp.root().into()],
        scratch.clone(),
    )
    .unwrap();
    let fixture = CertifiedConnectorProfile::fixture(admitted);
    let mut runtime = fixture.runtime().clone();
    let executable = native_child();
    let metadata = std::fs::metadata(&executable).unwrap();
    runtime.executable = ExecutableIdentity {
        path: executable,
        file_identity: [metadata.dev(), metadata.ino()],
    };
    runtime.launch_context.executable = runtime.executable.clone();
    runtime.launch_context.credential_store.root = credentials;
    runtime.authentication.context = runtime.launch_context.clone();
    runtime.authentication.refreshed_store = runtime.launch_context.credential_store.clone();
    let mut checks = NativeChecks(runtime);
    let profile = certify_connector("scripted", binding.clone(), &checks).unwrap();
    let program = crate::native_launcher::program(&profile, "placeholder").unwrap();
    checks.0.launch_context.environment_digest = native_digest(&program.environment);
    let mut arguments = program.arguments;
    arguments.pop();
    checks.0.launch_context.settings_digest = native_digest(&arguments);
    checks.0.authentication.context = checks.0.launch_context.clone();
    let profile = certify_connector("scripted", binding, &checks).unwrap();
    (profile, checks, scratch)
}
fn native_charge(
    f: &Fixture,
    admitted: &crate::orchestration::AdmittedAttempt,
    profile: crate::orchestration::adapters::CertifiedConnectorProfile,
) -> crate::orchestration::adapters::AuthorizedInvocation {
    let state = f.projection(&admitted.identity().run);
    let effect = state.effects[0].id.clone();
    let producer = f.producer(&admitted.identity().run);
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .charge_native_invocation(
            &producer,
            admitted,
            profile,
            &effect,
            crate::orchestration::adapters::SendKind::OpeningAssignment,
            1_000,
        )
        .unwrap()
}

type NativeStart = (
    Result<Box<dyn kinewright_core::AgentSession>, kinewright_core::AgentError>,
    Arc<Mutex<crate::execution_claim::MachineClaim>>,
);
fn native_start(
    f: &Fixture,
    admitted: crate::orchestration::AdmittedAttempt,
    invocation: crate::orchestration::adapters::AuthorizedInvocation,
    checks: NativeChecks,
    lose_commit_ack: bool,
) -> NativeStart {
    use kinewright_core::AgentDriver;
    let machine = f.temp.path("native-machine");
    let claim = Arc::new(Mutex::new(
        crate::execution_claim::MachineClaim::acquire_fixture(
            &machine,
            admitted.identity().project.as_str(),
            admitted.identity().run.0.as_str(),
        )
        .unwrap(),
    ));
    let host = f.host.owner.upgrade().unwrap();
    let port = crate::writer_host::native_invocation::HostInvocationOwner::new(
        &host,
        f.producer(&admitted.identity().run),
    );
    let launcher = crate::native_launcher::NativeGuardianLauncher {
        claim: claim.clone(),
        owner: Arc::new(port),
        guardian: kinewright_media::test_support::workspace_target_dir()
            .join("debug/kinewright-job-guardian"),
        checks: Arc::new(checks),
        lose_commit_ack,
    };
    let config = invocation.profile().session_config().clone();
    let driver = crate::orchestration::adapters::ContainedDriver::new(
        launcher,
        admitted,
        invocation,
        crate::ScriptedDriver::new(vec![]).detect().unwrap(),
    );
    (driver.start_session(config), claim)
}
fn wait_native(path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        assert!(Instant::now() < deadline, "native execution signal");
        std::thread::sleep(Duration::from_millis(1));
    }
}
#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c2_durable_charge_guardian_session_and_queued_steering() {
    let f = Fixture::new();
    let (run, identity, _) = reserve(&f);
    let _unit_cleanup = NativeUnitCleanup::new(&identity);
    let admitted = native_admitted(&f, &identity);
    let (profile, checks, scratch) = native_profile(&f, &admitted);
    assert!(
        !scratch.join("executions").exists(),
        "S4B_C2_NO_EXECUTION_BEFORE_CHARGE"
    );
    let invocation = native_charge(&f, &admitted, profile);
    assert!(
        !scratch.join("executions").exists(),
        "S4B_C2_NO_EXECUTION_BEFORE_COMMIT"
    );
    let (session, claim) = native_start(&f, admitted, invocation, checks, false);
    let mut session = session.unwrap();
    session
        .send_user_message("hook plan, script and assignment".into())
        .unwrap();
    wait_native(&scratch.join("execution-ready"));
    session.send_user_message("queued steering".into()).unwrap();
    assert_eq!(
        std::fs::read_to_string(scratch.join("executions")).unwrap(),
        "execution\n"
    );
    let projection = f.projection(&run);
    assert!(
        projection
            .effects
            .iter()
            .any(|effect| effect.state == EffectState::Pending
                && effect.safety == EffectSafety::ProcessBound),
        "S4B_C2_STEERING_QUEUED"
    );
    std::fs::write(scratch.join("exit"), b"").unwrap();
    let events = session.events();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match events.try_recv() {
            Ok(AgentEvent::Done) => break,
            Ok(_) => {}
            Err(_) => {
                assert!(Instant::now() < deadline, "native session completion");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
    assert_eq!(
        claim.lock().unwrap().retained_attempts(),
        0,
        "S4B_C2_CLEANUP_BEFORE_RETIREMENT"
    );
    drop(session);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Arc::strong_count(&claim) != 1 {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    Arc::try_unwrap(claim)
        .ok()
        .unwrap()
        .into_inner()
        .unwrap()
        .finish()
        .unwrap();
    std::fs::remove_dir_all(scratch).unwrap();
}
#[test]
fn s4b_c2_lost_charge_ack_reopens_one_charge_without_replacement() {
    let f = Fixture::new();
    let (run, identity, _) = reserve(&f);
    let admitted = native_admitted(&f, &identity);
    let (profile, _, scratch) = native_profile(&f, &admitted);
    let invocation = native_charge(&f, &admitted, profile.clone());
    let reference = invocation.acknowledgment().accounting().clone();
    drop(invocation);
    let projection = f.projection(&run);
    let producer = f.producer(&run);
    assert!(
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .charge_native_invocation(
                &producer,
                &admitted,
                profile,
                &projection.effects[0].id,
                crate::orchestration::adapters::SendKind::OpeningAssignment,
                1_001
            )
            .is_err(),
        "S4B_C2_NO_REPLACEMENT_AFTER_LOST_CHARGE_ACK"
    );
    let Fixture { temp, host, .. } = f;
    let path = temp.path("project.kinewright");
    drop(host);
    let reopened = Ja1EditingSession::open(&path).unwrap();
    let projection = reopened.orchestration_projection(&run).unwrap().unwrap();
    assert!(
        crate::orchestration::outbox::next_effect(&projection, &projection.effects[0].lane)
            .is_none(),
        "S4B_C2_REOPEN_NO_REPLACEMENT"
    );
    let record = reopened
        .store()
        .unwrap()
        .read_record::<OrchestrationRecord>(&reference)
        .unwrap();
    let OrchestrationPayload::Accounting(accounting) = &record.payload else {
        panic!("native accounting kind");
    };
    assert_eq!(
        accounting.tasks[0].sends.len(),
        1,
        "S4B_C2_ONE_DURABLE_CHARGE"
    );
    assert!(!scratch.join("executions").exists());
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn s4b_c2_changed_charged_payload_refuses_execution() {
    let f = Fixture::new();
    let (_, identity, _) = reserve(&f);
    let _unit_cleanup = NativeUnitCleanup::new(&identity);
    let admitted = native_admitted(&f, &identity);
    let (profile, checks, scratch) = native_profile(&f, &admitted);
    let mut invocation = native_charge(&f, &admitted, profile);
    invocation.replace_payload_for_test(b"changed prompt".to_vec());
    std::fs::write(scratch.join("exit"), b"").unwrap();
    let (result, claim) = native_start(&f, admitted, invocation, checks, false);
    assert!(result.is_err(), "S4B_C2_EXACT_PAYLOAD_AT_RELEASE");
    assert!(!scratch.join("executions").exists());
    assert_eq!(claim.lock().unwrap().retained_attempts(), 0);
    std::fs::remove_dir_all(scratch).unwrap();
}
#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c2_lost_commit_ack_reopens_one_charge_and_one_execution() {
    let f = Fixture::new();
    let (run, identity, _) = reserve(&f);
    let _unit_cleanup = NativeUnitCleanup::new(&identity);
    let admitted = native_admitted(&f, &identity);
    let duplicate = native_admitted(&f, &identity);
    let (profile, checks, scratch) = native_profile(&f, &admitted);
    let invocation = native_charge(&f, &admitted, profile.clone());
    let reference = invocation.acknowledgment().accounting().clone();
    let (result, claim) = native_start(&f, admitted, invocation, checks, true);
    assert!(result.is_err(), "S4B_C2_LOST_EXECUTING_ACK");
    assert_eq!(claim.lock().unwrap().retained_attempts(), 0);
    assert_eq!(
        std::fs::read_to_string(scratch.join("executions")).unwrap(),
        "execution\n"
    );
    let projection = f.projection(&run);
    assert_eq!(
        projection.attempts[0].state,
        AttemptState::Contained,
        "S4B_C2_RECOVERY_ACK_BEFORE_EXECUTION"
    );
    let producer = f.producer(&run);
    assert!(
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .charge_native_invocation(
                &producer,
                &duplicate,
                profile,
                &projection.effects[0].id,
                crate::orchestration::adapters::SendKind::OpeningAssignment,
                1_001
            )
            .is_err()
    );
    let Fixture { host, temp, .. } = f;
    drop(host);
    let reopened = Ja1EditingSession::open(&temp.path("project.kinewright")).unwrap();
    let projection = reopened.orchestration_projection(&run).unwrap().unwrap();
    assert!(
        crate::orchestration::outbox::next_effect(&projection, &projection.effects[0].lane)
            .is_none(),
        "S4B_C2_COMMIT_REOPEN_NO_REPLACEMENT"
    );
    let record = reopened
        .store()
        .unwrap()
        .read_record::<OrchestrationRecord>(&reference)
        .unwrap();
    let OrchestrationPayload::Accounting(accounting) = &record.payload else {
        panic!("native accounting kind");
    };
    assert_eq!(
        accounting.tasks[0].sends.len(),
        1,
        "S4B_C2_COMMIT_REOPEN_ONE_CHARGE"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}
#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c2_full_event_queue_cancels_and_proves_cleanup() {
    let mut f = Fixture::new();
    let host = f.host.owner.upgrade().unwrap();
    f.brief.assignment = host
        .lock()
        .unwrap()
        .owner_mut()
        .unwrap()
        .admit_host_grant(b"block")
        .unwrap();
    drop(host);
    let (_, identity, _) = reserve(&f);
    let _unit_cleanup = NativeUnitCleanup::new(&identity);
    let admitted = native_admitted(&f, &identity);
    let (profile, checks, scratch) = native_profile(&f, &admitted);
    let invocation = native_charge(&f, &admitted, profile);
    let (result, claim) = native_start(&f, admitted, invocation, checks, false);
    let mut session = result.unwrap();
    session.send_user_message("block".into()).unwrap();
    wait_native(&scratch.join("execution-ready"));
    let events = session.events();
    let deadline = Instant::now() + Duration::from_secs(5);
    while events.len() < 30 {
        assert!(Instant::now() < deadline, "S4B_C2_FULL_EVENT_QUEUE");
        std::thread::sleep(Duration::from_millis(1));
    }
    session.interrupt();
    while claim.lock().unwrap().retained_attempts() != 0 {
        assert!(
            Instant::now() < deadline,
            "S4B_C2_FULL_QUEUE_CLEANUP_PROVEN"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        std::fs::read_to_string(scratch.join("executions")).unwrap(),
        "execution\n"
    );
    drop(session);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c2_overflow_cleans_before_reusing_the_slot() {
    let mut f = Fixture::new();
    let host = f.host.owner.upgrade().unwrap();
    f.brief.assignment = host
        .lock()
        .unwrap()
        .owner_mut()
        .unwrap()
        .admit_host_grant(b"overflow")
        .unwrap();
    drop(host);
    let (_, identity, _) = reserve(&f);
    let _unit_cleanup = NativeUnitCleanup::new(&identity);
    let admitted = native_admitted(&f, &identity);
    let (profile, checks, scratch) = native_profile(&f, &admitted);
    let invocation = native_charge(&f, &admitted, profile);
    let (result, claim) = native_start(&f, admitted, invocation, checks, false);
    let mut session = result.unwrap();
    session.send_user_message("overflow".into()).unwrap();
    wait_native(&scratch.join("execution-ready"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while claim.lock().unwrap().retained_attempts() != 0 {
        assert!(Instant::now() < deadline, "S4B_C2_OVERFLOW_CLEANUP_PROVEN");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(session.events().len(), 32);
    drop(session);
    std::fs::remove_dir_all(scratch).unwrap();
}
#[test]
fn s4b_c2_specialist_steering_and_coalesced_wakes_are_charged() {
    use crate::orchestration::adapters::SendKind;
    for kind in [
        SendKind::SpecialistStart,
        SendKind::QueuedSteering,
        SendKind::AutomaticResultWake,
    ] {
        let f = Fixture::new();
        let (run, mut execution, actor) = reserve(&f);
        let mut state = f.projection(&run);
        state.effects[0].state = EffectState::Settled;
        let mut target = state.tasks[0].clone();
        if kind == SendKind::SpecialistStart {
            target.id = identity();
            target.parent = Some(state.tasks[0].id.clone());
            target.brief.role = Role::Editor;
            execution.task = target.id.clone();
            execution.attempt = identity();
            execution.nonce = identity();
            target.progress.capacity = DependencyReservation::Backed(execution.attempt.clone());
            state.tasks.push(target.clone());
            let mut reservation = state.attempts[0].clone();
            reservation.identity = execution.clone();
            state.attempts.push(reservation);
        }
        let payload = if kind == SendKind::QueuedSteering {
            f.host
                .owner
                .upgrade()
                .unwrap()
                .lock()
                .unwrap()
                .owner_mut()
                .unwrap()
                .admit_host_grant(b"queued prompt")
                .unwrap()
        } else {
            f.brief.assignment.clone()
        };
        let effect: EffectId = identity();
        state.effects.push(EffectRecord {
            id: effect.clone(),
            task: target.id.clone(),
            lane: target.brief.scope.base.branch.clone(),
            order: 2,
            safety: EffectSafety::ReplaySafe,
            state: EffectState::Pending,
            payload,
        });
        let mut expected_results = vec![];
        if kind == SendKind::AutomaticResultWake {
            for ordinal in 0..2 {
                let mut child = target.clone();
                child.id = identity();
                child.parent = Some(target.id.clone());
                child.state = TaskState::Terminal(f.brief.assignment.clone());
                child.progress = TaskProgress::default();
                let result = f
                    .host
                    .owner
                    .upgrade()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .owner_mut()
                    .unwrap()
                    .admit_host_grant(format!("result {ordinal}").as_bytes())
                    .unwrap();
                expected_results.push(result.clone());
                state.deliveries.push(DeliveryRecord {
                    id: identity(),
                    task: child.id.clone(),
                    result,
                    generation: 1,
                    wake: effect.clone(),
                    message: identity(),
                    state: DeliveryState::Pending,
                });
                state.tasks.push(child);
            }
        }
        crate::orchestration::tasks::refresh_cleanup(&mut state);
        publish_projection(&f, &actor, &run, state, Some(&execution));
        let admitted = native_admitted(&f, &execution);
        let (profile, _, scratch) = native_profile(&f, &admitted);
        let producer = f.producer(&run);
        let invocation = f
            .host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .charge_native_invocation(&producer, &admitted, profile, &effect, kind, 1_000)
            .unwrap();
        assert_eq!(
            invocation.invocation().kind(),
            kind,
            "S4B_C2_ALL_SEND_ROUTES"
        );
        if kind == SendKind::AutomaticResultWake {
            let actual: Vec<kinewright_project::bundle_records::ObjectRef> =
                serde_json::from_slice(invocation.invocation().bytes()).unwrap();
            assert_eq!(actual, expected_results, "S4B_C2_COALESCED_WAKE_PAYLOAD");
        }
        assert_eq!(
            invocation.acknowledgment().consumption().attempt,
            execution.attempt.0
        );
        let state = f.projection(&run);
        let effect = state
            .effects
            .iter()
            .find(|candidate| candidate.id == effect)
            .unwrap();
        assert_eq!(
            effect.safety,
            EffectSafety::ProcessBound,
            "S4B_C2_MESSAGE_EFFECT_IS_PROCESS_BOUND"
        );
        assert_eq!(
            effect.state,
            EffectState::Claimed(execution.attempt),
            "S4B_C2_EFFECT_CLAIM_DURABLE"
        );
        assert!(!scratch.join("executions").exists());
        std::fs::remove_dir_all(scratch).unwrap();
    }
}

#[test]
fn s4b_c2_credential_store_contains_only_qualified_files() {
    let f = Fixture::new();
    let (_, identity, _) = reserve(&f);
    let admitted = native_admitted(&f, &identity);
    let (profile, _, scratch) = native_profile(&f, &admitted);
    std::fs::write(
        scratch.join("credentials/unrelated-home-file"),
        b"unrelated",
    )
    .unwrap();
    assert!(
        crate::native_launcher::program(&profile, "prompt").is_err(),
        "S4B_C2_EXACT_CREDENTIAL_SCOPE"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}
#[test]
fn s4b_c2_project_alias_is_not_private_scratch() {
    use crate::orchestration::adapters::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    let f = Fixture::new();
    let (_, identity, _) = reserve(&f);
    let admitted = native_admitted(&f, &identity);
    let alias = kinewright_media::test_support::workspace_target_dir()
        .join(format!("alias-{}", &identity.nonce.as_str()[..8]));
    symlink(f.temp.root(), &alias).unwrap();
    let scratch = alias.join("private");
    std::fs::create_dir(&scratch).unwrap();
    std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o700)).unwrap();
    let credentials = scratch.join("credentials");
    std::fs::create_dir(&credentials).unwrap();
    std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o700)).unwrap();
    let binding = AttemptBinding::issue(
        &admitted,
        "http://127.0.0.1:12345/mcp".into(),
        vec![f.temp.root().into()],
        scratch,
    )
    .unwrap();
    let fixture = CertifiedConnectorProfile::fixture(&admitted);
    let mut checks = NativeChecks(fixture.runtime().clone());
    let executable = native_child();
    let metadata = std::fs::metadata(&executable).unwrap();
    checks.0.executable = ExecutableIdentity {
        path: executable,
        file_identity: [metadata.dev(), metadata.ino()],
    };
    checks.0.launch_context.executable = checks.0.executable.clone();
    checks.0.launch_context.credential_store.root = credentials;
    checks.0.authentication.context = checks.0.launch_context.clone();
    checks.0.authentication.refreshed_store = checks.0.launch_context.credential_store.clone();
    let profile = certify_connector("scripted", binding, &checks).unwrap();
    let error = crate::native_launcher::program(&profile, "prompt")
        .expect_err("S4B_C2_PROJECT_ALIAS_DENIED");
    assert_eq!(
        error.to_string(),
        "provider allowlist overlaps project authority",
        "S4B_C2_PROJECT_ALIAS_DENIED"
    );
    std::fs::remove_file(alias).unwrap();
}
struct NativeUnitCleanup(String);
impl NativeUnitCleanup {
    fn new(identity: &ExecutionIdentity) -> Self {
        Self(format!(
            "kinewright-job-{}.service",
            identity.nonce.as_str()
        ))
    }
}
impl Drop for NativeUnitCleanup {
    fn drop(&mut self) {
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "stop", &self.0])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

#[test]
fn s4b_c2_studio_has_no_raw_provider_start_fallback() {
    let f = Fixture::new();
    for harness in ["codex", "claude", "muse"] {
        denied(
            &f,
            &f.editor,
            HostCommand::StartSession {
                harness: harness.into(),
                config: HostSessionConfig::default(),
            },
            "provider_unqualified",
            "S4B_C2_STUDIO_GUARDIAN_REQUIRED",
        );
    }
    assert!(
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .sessions
            .is_empty()
    );
}

#[test]
fn s4b_c2_terminal_or_expired_task_cannot_release_native() {
    use crate::native_launcher::InvocationOwner;
    for terminal in [true, false] {
        let f = Fixture::new();
        let (run, identity, actor) = reserve(&f);
        let admitted = native_admitted(&f, &identity);
        let (profile, _, scratch) = native_profile(&f, &admitted);
        let invocation = native_charge(&f, &admitted, profile);
        let mut projection = f.projection(&run);
        if terminal {
            projection.tasks[0].state = TaskState::Terminal(f.brief.assignment.clone());
        } else {
            projection.tasks[0].progress.expired = true;
        }
        publish_projection(&f, &actor, &run, projection, None);
        let host = f.host.owner.upgrade().unwrap();
        let port = crate::writer_host::native_invocation::HostInvocationOwner::new(
            &host,
            f.producer(&run),
        );
        assert!(
            port.reserve(&invocation).is_err(),
            "S4B_C2_LIVE_TASK_AT_NATIVE_BOUNDARY"
        );
        assert!(!scratch.join("executions").exists());
        std::fs::remove_dir_all(scratch).unwrap();
    }
}

#[test]
fn s4b_c2_executing_and_waiting_steering_is_queued_without_charge() {
    use crate::native_launcher::InvocationOwner;
    for state in [AttemptState::Executing, AttemptState::Waiting] {
        let f = Fixture::new();
        let (run, identity, actor) = reserve(&f);
        let admitted = native_admitted(&f, &identity);
        let (profile, _, scratch) = native_profile(&f, &admitted);
        let invocation = native_charge(&f, &admitted, profile);
        let mut projection = f.projection(&run);
        projection.tasks[0].state = TaskState::Running(identity.attempt.clone());
        projection.attempts[0].state = state;
        let charged = projection.usage.clone();
        publish_projection(&f, &actor, &run, projection, None);
        let host = f.host.owner.upgrade().unwrap();
        let port = crate::writer_host::native_invocation::HostInvocationOwner::new(
            &host,
            f.producer(&run),
        );
        port.steering(&identity, "later steering")
            .expect("S4B_C2_ACTIVE_STEERING_QUEUED");
        let projection = f.projection(&run);
        assert_eq!(projection.usage, charged);
        assert_eq!(
            projection.effects[0].state,
            EffectState::Claimed(identity.attempt)
        );
        assert_eq!(projection.effects[1].state, EffectState::Pending);
        assert!(!scratch.join("executions").exists());
        drop(invocation);
        std::fs::remove_dir_all(scratch).unwrap();
    }
}
