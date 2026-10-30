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
    native_profile_at(f.temp.root(), admitted)
}
fn native_profile_at(
    root: &std::path::Path,
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
        vec![root.into()],
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
fn native_now(f: &Fixture, run: &RunId) -> u64 {
    let producer = f.producer(run);
    let host = f.host.owner.upgrade().unwrap();
    let mut host = host.lock().unwrap();
    let state = host.spending_scope(&producer, run).unwrap();
    host.native_accounting(&state)
        .unwrap()
        .unwrap()
        .last_unix_ms
}
fn native_charge(
    f: &Fixture,
    admitted: &crate::orchestration::AdmittedAttempt,
    profile: crate::orchestration::adapters::CertifiedConnectorProfile,
) -> crate::orchestration::adapters::AuthorizedInvocation {
    let state = f.projection(&admitted.identity().run);
    let effect = state.effects[0].id.clone();
    let producer = f.producer(&admitted.identity().run);
    let now = native_now(f, &admitted.identity().run);
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
            now,
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
    native_start_barrier(f, admitted, invocation, checks, lose_commit_ack, None)
}
fn native_start_barrier(
    f: &Fixture,
    admitted: crate::orchestration::AdmittedAttempt,
    invocation: crate::orchestration::adapters::AuthorizedInvocation,
    checks: NativeChecks,
    lose_commit_ack: bool,
    after_release: Option<Arc<dyn Fn() + Send + Sync>>,
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
        after_release,
        startup_failure: None,
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
                let text = format!("result {ordinal}");
                let result = {
                    let host = f.host.owner.upgrade().unwrap();
                    let mut host = host.lock().unwrap();
                    let explanation = host
                        .owner_mut()
                        .unwrap()
                        .admit_host_grant(text.as_bytes())
                        .unwrap();
                    host.owner_mut()
                        .unwrap()
                        .admit_scheduler_record(&OrchestrationRecord {
                            version: 1,
                            payload: OrchestrationPayload::Result(Box::new(TaskResult {
                                outcome: AttemptOutcome::Completed,
                                base: Some(f.brief.scope.base.clone()),
                                head: Some(f.brief.scope.base.clone()),
                                delta_ref: None,
                                artifacts: vec![],
                                evidence: vec![],
                                usage_ref: state.usage[0].clone(),
                                explanation,
                            })),
                        })
                        .unwrap()
                        .reference
                };
                expected_results.push(text);
                child.state = TaskState::Terminal(result.clone());
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
        let now = native_now(&f, &run);
        let invocation = f
            .host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .charge_native_invocation(&producer, &admitted, profile, &effect, kind, now)
            .unwrap();
        assert_eq!(
            invocation.invocation().kind(),
            kind,
            "S4B_C2_ALL_SEND_ROUTES"
        );
        if kind == SendKind::AutomaticResultWake {
            let values: Vec<serde_json::Value> =
                serde_json::from_slice(invocation.invocation().bytes()).unwrap();
            let actual: Vec<_> = values
                .iter()
                .map(|value| value["explanation"].as_str().unwrap().to_owned())
                .collect();
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

fn finish_stopped_native(claim: Arc<Mutex<crate::execution_claim::MachineClaim>>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while claim.lock().unwrap().retained_attempts() != 0 || Arc::strong_count(&claim) != 1 {
        assert!(Instant::now() < deadline, "S4B_C2_FIX1_STOP_CLEANUP");
        std::thread::sleep(Duration::from_millis(1));
    }
    Arc::try_unwrap(claim)
        .ok()
        .unwrap()
        .into_inner()
        .unwrap()
        .finish()
        .unwrap();
}
fn stop_native_run(f: &Fixture, run: &RunId) {
    // Raw usage now publishes concurrent H1 cuts. An explicitly stale control
    // is a refused cut, so refresh its original view before asserting Stop won.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let stop = f.control(&f.editor, run, RunAction::Stop);
        let reply = ok(
            call(
                f,
                &f.editor,
                HostCommand::DispatchOrchestration {
                    request: Box::new(stop),
                },
                0,
            ),
            "S4B_C2_FIX1_CURRENT_STOP_ACK",
        );
        if reply.orchestration.unwrap().decision == CommandDecision::Applied {
            break;
        }
        assert!(Instant::now() < deadline, "S4B_C2_FIX1_CURRENT_STOP_ACK");
    }
    assert!(f.projection(run).attempts.iter().all(|attempt| matches!(
        attempt.state,
        AttemptState::Stopping | AttemptState::Retired(_)
    )));
}

#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c2_fix1_stop_between_release_and_commit_vetoes_execution() {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Stage {
        Waiting,
        Authorized,
        Resumed,
    }
    let f = Fixture::new();
    let (run, identity, _) = reserve(&f);
    let _unit_cleanup = NativeUnitCleanup::new(&identity);
    let admitted = native_admitted(&f, &identity);
    let (profile, checks, scratch) = native_profile(&f, &admitted);
    let invocation = native_charge(&f, &admitted, profile);
    let handoff = Arc::new((Mutex::new(Stage::Waiting), std::sync::Condvar::new()));
    let release_gate = handoff.clone();
    let barrier = Arc::new(move || {
        let (lock, changed) = &*release_gate;
        let mut stage = lock.lock().unwrap();
        *stage = Stage::Authorized;
        changed.notify_all();
        let (stage, _) = changed
            .wait_timeout_while(stage, Duration::from_secs(10), |stage| {
                *stage != Stage::Resumed
            })
            .unwrap();
        assert!(*stage == Stage::Resumed, "startup resume barrier deadline");
    });
    let (result, claim) = std::thread::scope(|scope| {
        let f = &f;
        let launch = scope.spawn(move || {
            native_start_barrier(f, admitted, invocation, checks, false, Some(barrier))
        });
        let (lock, changed) = &*handoff;
        let (stage, _) = changed
            .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(10), |stage| {
                *stage != Stage::Authorized
            })
            .unwrap();
        assert!(
            *stage == Stage::Authorized,
            "durable release barrier deadline"
        );
        drop(stage);
        assert_eq!(
            f.projection(&run).attempts[0].state,
            AttemptState::Contained
        );
        assert!(!scratch.join("executions").exists());
        stop_native_run(f, &run);
        *lock.lock().unwrap() = Stage::Resumed;
        changed.notify_all();
        launch.join().unwrap()
    });
    let refused = result.is_err();
    if let Ok(mut session) = result {
        wait_native(&scratch.join("execution-ready"));
        session.interrupt();
        drop(session);
    }
    finish_stopped_native(claim);
    assert!(
        refused && !scratch.join("executions").exists(),
        "S4B_C2_FIX1_STOP_WINS_ZERO_EXECUTION"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c2_fix1_commit_wins_stop_reaches_registered_session() {
    let f = Fixture::new();
    let (run, identity, _) = reserve(&f);
    let _unit_cleanup = NativeUnitCleanup::new(&identity);
    let admitted = native_admitted(&f, &identity);
    let (profile, checks, scratch) = native_profile(&f, &admitted);
    let invocation = native_charge(&f, &admitted, profile);
    let (session, claim) = native_start(&f, admitted, invocation, checks, false);
    let session = session.unwrap();
    wait_native(&scratch.join("execution-ready"));
    stop_native_run(&f, &run);
    // Keep the returned session alive: only the serialized Stop may close it.
    let deadline = Instant::now() + Duration::from_secs(10);
    while claim.lock().unwrap().retained_attempts() != 0 {
        assert!(
            Instant::now() < deadline,
            "S4B_C2_FIX1_RELEASE_WINS_STOP_CLEANUP"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        std::fs::read_to_string(scratch.join("executions")).unwrap(),
        "execution\n"
    );
    drop(session);
    finish_stopped_native(claim);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
#[allow(
    clippy::too_many_lines,
    reason = "four actual attempts share one bounded machine claim"
)]
fn s4b_c2_fix1_three_failed_launches_leave_machine_usable() {
    use crate::{
        native_launcher::{NativeGuardianLauncher, StartupFailure},
        orchestration::adapters::*,
    };
    use kinewright_core::AgentDriver;
    let f = Fixture::new();
    let (run, mut execution, actor) = reserve(&f);
    let machine = f.temp.path("failed-native-machine");
    let claim = Arc::new(Mutex::new(
        crate::execution_claim::MachineClaim::acquire_fixture(
            &machine,
            execution.project.as_str(),
            run.0.as_str(),
        )
        .unwrap(),
    ));
    let mut cleanups = Vec::new();
    let mut scratches = Vec::new();
    let mut usable = false;
    for (ordinal, failure) in [
        Some(StartupFailure::Pipes),
        Some(StartupFailure::Credentials),
        Some(StartupFailure::Reader),
        None,
    ]
    .into_iter()
    .enumerate()
    {
        if ordinal > 0 {
            // The fixture supplies C3's compacted next reservation after the
            // preceding cleanup; production recovery policy is not exercised.
            let mut projection = f.projection(&run);
            let previous = projection.attempts.last_mut().unwrap();
            previous.state = AttemptState::Retired(AttemptOutcome::Failed);
            let mut reservation = previous.clone();
            execution.attempt = identity();
            execution.nonce = identity();
            reservation.identity = execution.clone();
            reservation.state = AttemptState::Reserved;
            reservation.recovery_ref = f.brief.assignment.clone();
            projection.attempts = vec![reservation];
            projection.tasks[0].state = TaskState::Preparing;
            projection.tasks[0].progress.capacity =
                DependencyReservation::Backed(execution.attempt.clone());
            let mut effect = projection.effects.last_mut().unwrap().clone();
            projection.effects.last_mut().unwrap().state = EffectState::Disposed;
            effect.id = identity();
            effect.order += 1;
            effect.state = EffectState::Pending;
            projection.effects.push(effect);
            crate::orchestration::tasks::refresh_cleanup(&mut projection);
            publish_projection(&f, &actor, &run, projection, Some(&execution));
        }
        cleanups.push(NativeUnitCleanup::new(&execution));
        let admitted = native_admitted(&f, &execution);
        let (profile, checks, scratch) = native_profile(&f, &admitted);
        scratches.push(scratch.clone());
        let host = f.host.owner.upgrade().unwrap();
        let producer = f.producer(&run);
        let projection = f.projection(&run);
        let effect = projection.effects.last().unwrap().id.clone();
        let now = native_now(&f, &run);
        let invocation = host
            .lock()
            .unwrap()
            .charge_native_invocation(
                &producer,
                &admitted,
                profile,
                &effect,
                if ordinal == 0 {
                    SendKind::OpeningAssignment
                } else {
                    SendKind::QueuedSteering
                },
                now + ordinal as u64,
            )
            .unwrap();
        let config = invocation.profile().session_config().clone();
        let launcher = NativeGuardianLauncher {
            claim: claim.clone(),
            owner: Arc::new(
                crate::writer_host::native_invocation::HostInvocationOwner::new(&host, producer),
            ),
            guardian: kinewright_media::test_support::workspace_target_dir()
                .join("debug/kinewright-job-guardian"),
            checks: Arc::new(checks),
            lose_commit_ack: false,
            after_release: None,
            startup_failure: failure,
        };
        let driver = ContainedDriver::new(
            launcher,
            admitted,
            invocation,
            crate::ScriptedDriver::new(vec![]).detect().unwrap(),
        );
        let result = driver.start_session(config);
        if ordinal < 3 {
            assert!(result.is_err(), "fixture startup error required");
        } else if let Ok(mut session) = result {
            wait_native(&scratch.join("execution-ready"));
            usable = true;
            session.interrupt();
            drop(session);
        }
    }
    assert!(usable, "S4B_C2_FIX1_FAILED_LAUNCH_CAPACITY_RECLAIMED");
    finish_stopped_native(claim);
    for scratch in scratches {
        std::fs::remove_dir_all(scratch).unwrap();
    }
}
