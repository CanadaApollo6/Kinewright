fn unbound_identity(
    identity: &ExecutionIdentity,
) -> crate::job_guardian::protocol::AttemptIdentity {
    crate::job_guardian::protocol::AttemptIdentity {
        project: identity.project.as_str().into(),
        run: identity.run.0.as_str().into(),
        task: identity.task.0.as_str().into(),
        attempt: identity.attempt.0.as_str().into(),
        nonce: identity.nonce.as_str().into(),
        owner_instance: identity.owner_instance.as_str().into(),
    }
}
#[test]
fn s4b_c3_fix1_unbound_crash_child() {
    let Some(handoff) = std::env::var_os("KINEWRIGHT_C3_UNBOUND_HANDOFF") else {
        return;
    };
    let f = Fixture::new();
    let (run, identity, _) = reserve(&f);
    let admitted = native_admitted(&f, &identity);
    let (profile, _, scratch) = native_profile(&f, &admitted);
    let _charged = native_charge(&f, &admitted, profile);
    let machine = f.temp.path("unbound-machine");
    let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &machine,
        identity.project.as_str(),
        run.0.as_str(),
    )
    .unwrap();
    claim.reserve(&unbound_identity(&identity)).unwrap();
    assert!(
        machine
            .join(format!("attempt-{}.json", identity.nonce.as_str()))
            .exists()
    );
    assert!(
        !machine
            .join(format!("native-{}.json", identity.nonce.as_str()))
            .exists()
    );
    std::fs::write(handoff,serde_json::to_vec(&serde_json::json!({"root":f.temp.root(),"identity":identity,"machine":machine,"scratch":scratch})).unwrap()).unwrap();
    std::process::exit(86);
}
#[test]
fn s4b_c3_fix1_unbound_crash_recovers_one_charge() {
    let handoff = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "unbound-handoff",
    );
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "writer_host::hooks_tests::tests::s4b_c3_fix1_unbound_crash_child",
            "--nocapture",
        ])
        .env(
            "KINEWRIGHT_C3_UNBOUND_HANDOFF",
            handoff.path("handoff.json"),
        )
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(86),
        "C3_UNBOUND_CRASH_CUT {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let data: serde_json::Value =
        serde_json::from_slice(&std::fs::read(handoff.path("handoff.json")).unwrap()).unwrap();
    let root: std::path::PathBuf = serde_json::from_value(data["root"].clone()).unwrap();
    let machine: std::path::PathBuf = serde_json::from_value(data["machine"].clone()).unwrap();
    let identity: ExecutionIdentity = serde_json::from_value(data["identity"].clone()).unwrap();
    let scratch: std::path::PathBuf = serde_json::from_value(data["scratch"].clone()).unwrap();
    let writer = WriterHost::start(HostConfig {
        scheduler_settings: None,
        project: "hooks".into(),
        path: Some(root.join("project.kinewright")),
        roots: SessionRoots {
            read: vec![root.clone()],
            write: vec![root.clone()],
        },
        origin: HostOrigin::Headless,
    })
    .unwrap();
    let host = writer.owner.upgrade().unwrap();
    let claim = crate::execution_claim::MachineClaim::recover_fixture(&machine).unwrap();
    let recovered = crate::effect_pump::recover_run(&host, claim, &identity.run);
    assert!(
        recovered.is_ok(),
        "MUSE_UNBOUND_RESERVATION_RECOVERABLE: {:?}",
        recovered.as_ref().err()
    );
    let (claim, producer) = recovered.unwrap();
    assert_eq!(claim.retained_attempts(), 0);
    let (mut pump, claim, profiles) =
        effect_pump(&host, producer, identity.run.clone(), claim, &root);
    pump.tick().unwrap();
    assert!(
        profiles.observations.lock().unwrap().is_empty(),
        "C3_UNBOUND_NO_REPLACEMENT_PROCESS"
    );
    let state = host
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .orchestration_projection(&identity.run)
        .unwrap()
        .unwrap();
    assert!(matches!(state.attempts[0].state, AttemptState::Retired(_)));
    assert_eq!(state.effects[0].state, EffectState::OutcomeUnknown);
    assert_eq!(
        host.lock()
            .unwrap()
            .native_accounting(&state)
            .unwrap()
            .unwrap()
            .tasks[0]
            .sends
            .len(),
        1
    );
    assert!(!scratch.join("execution-ready").exists());
    drop(pump);
    finish_stopped_native(claim);
    drop(host);
    drop(writer);
    std::fs::remove_dir_all(scratch).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
struct RefusePrepare;
impl crate::guardian::DurableAttemptOwner for RefusePrepare {
    fn reserve(
        &mut self,
        _: &crate::job_guardian::protocol::AttemptIdentity,
    ) -> std::io::Result<()> {
        Err(std::io::Error::other(
            "injected reserve-before-bind refusal",
        ))
    }
    fn authorize_commit(
        &mut self,
        _: &crate::job_guardian::protocol::RecoveryIdentity,
    ) -> std::io::Result<()> {
        panic!("no COMMIT for failed preparation")
    }
}
#[test]
fn s4b_c3_fix1_prepare_failure_retires_unbound_only_under_proof() {
    let f = Fixture::new();
    let (run, identity, _) = reserve(&f);
    let machine = f.temp.path("failed-prepare-machine");
    let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &machine,
        identity.project.as_str(),
        run.0.as_str(),
    )
    .unwrap();
    for _ in 0..4 {
        let mut id = unbound_identity(&identity);
        id.nonce = kinewright_project::bundle_records::SessionId::fresh()
            .unwrap()
            .as_str()
            .into();
        id.attempt = id.nonce.clone();
        let program = crate::job_guardian::protocol::Program {
            executable: "/usr/bin/true".into(),
            arguments: vec![],
            environment: std::collections::BTreeMap::default(),
            working_directory: f.temp.root().into(),
            sandbox: crate::job_guardian::protocol::ProviderSandbox::ContainmentOnly,
            pipe_directory: None,
        };
        let result = crate::guardian::ContainedSession::prepare(
            &mut claim,
            &mut RefusePrepare,
            id.clone(),
            program,
            &kinewright_media::test_support::workspace_target_dir()
                .join("debug/kinewright-job-guardian"),
            &crate::guardian::CertifiedProfile::linux_allowlist(),
        );
        assert!(result.is_err());
        assert_eq!(
            claim.retained_attempts(),
            0,
            "C3_FAILED_PREPARE_UNBOUND_RETIREMENT"
        );
        assert!(!machine.join(format!("attempt-{}.json", id.nonce)).exists());
    }
    claim.finish().unwrap();
}
#[test]
fn s4b_c3_fix1_missing_accounting_refuses_charge() {
    let f = Fixture::new();
    let (run, identity, _) = reserve(&f);
    let admitted = native_admitted(&f, &identity);
    let (profile, _, scratch) = native_profile(&f, &admitted);
    let producer = f.producer(&run);
    let host = f.host.owner.upgrade().unwrap();
    let mut owner = host.lock().unwrap();
    let mut state = owner.spending_scope(&producer, &run).unwrap();
    state.usage.clear();
    owner
        .publish_native(&producer, state.clone(), &identity, vec![])
        .unwrap();
    let refused = owner.charge_native_invocation(
        &producer,
        &admitted,
        profile,
        &state.effects[0].id,
        crate::orchestration::adapters::SendKind::OpeningAssignment,
        super::super::spending::unix_ms().unwrap(),
    );
    assert!(
        refused
            .err()
            .unwrap()
            .to_string()
            .contains("run has no durable clock"),
        "C3_MISSING_CLOCK_CANNOT_RESET_BUDGETS"
    );
    let current = owner.spending_scope(&producer, &run).unwrap();
    assert!(owner.native_accounting(&current).unwrap().is_none());
    assert_eq!(current.effects[0].state, EffectState::Pending);
    std::fs::remove_dir_all(scratch).unwrap();
}
struct FailedProfiles;
impl crate::effect_pump::ProfileFactory for FailedProfiles {
    fn bind(
        &self,
        _: &crate::orchestration::AdmittedAttempt,
    ) -> std::io::Result<crate::orchestration::adapters::CertifiedConnectorProfile> {
        Err(std::io::Error::other("injected profile refusal"))
    }
}
#[test]
fn s4b_c3_fix1_failed_tick_retires_reservation() {
    let f = Fixture::new();
    let start = f.start();
    let run: RunId = start.envelope.identity("run").unwrap();
    let host = f.host.owner.upgrade().unwrap();
    let producer = f.producer(&run);
    let (old, _, _, core) = history_task_endpoint(&f, &run);
    for id in [1, 2] {
        core.request(kinewright_core::Command::Do(history_marker(id)))
            .unwrap();
    }
    core.request(kinewright_core::Command::Undo).unwrap();
    let expected = core.export_history().unwrap();
    host.lock()
        .unwrap()
        .checkpoint_task_history(&producer, &run, &old.task)
        .unwrap();
    let claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &f.temp.path("failed-tick-machine"),
        start.envelope.project.as_str(),
        run.0.as_str(),
    )
    .unwrap();
    let (claim, producer) = crate::effect_pump::recover_run(&host, claim, &run).unwrap();
    let claim = Arc::new(Mutex::new(claim));
    let profiles = PumpProfiles {
        root: f.temp.root().into(),
        observations: Arc::default(),
        checks: Arc::default(),
    };
    let launcher = crate::native_launcher::NativeGuardianLauncher {
        claim: claim.clone(),
        owner: Arc::new(super::super::native_invocation::HostInvocationOwner::new(
            &host,
            producer.clone(),
        )),
        guardian: kinewright_media::test_support::workspace_target_dir()
            .join("debug/kinewright-job-guardian"),
        checks: Arc::new(profiles.clone()),
        lose_commit_ack: false,
        after_release: None,
        startup_failure: None,
    };
    let mut pump = crate::effect_pump::EffectPump::new(
        &host,
        producer,
        run.clone(),
        launcher,
        Arc::new(FailedProfiles),
        None,
    );
    for _ in 0..4 {
        assert!(
            pump.tick()
                .unwrap_err()
                .to_string()
                .contains("injected profile refusal")
        );
        let state = f.projection(&run);
        assert!(
            state
                .attempts
                .iter()
                .all(|a| matches!(a.state, AttemptState::Retired(_))),
            "C3_FAILED_TICK_RETIRES_RESERVATION"
        );
        assert!(
            host.lock()
                .unwrap()
                .native_accounting(&state)
                .unwrap()
                .unwrap()
                .tasks[0]
                .sends
                .is_empty()
        );
        assert_eq!(claim.lock().unwrap().retained_attempts(), 0);
        assert_eq!(
            core.export_history().unwrap(),
            expected,
            "C3_FAILED_TICK_PRESERVES_RETIRED_ACTOR"
        );
    }
    assert!(profiles.observations.lock().unwrap().is_empty());
    drop(pump);
    finish_stopped_native(claim);
}
#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c3_fix1_rate_wait_clears_after_bounded_clock_time() {
    use super::super::native_invocation::SpendingChange;
    use crate::protocol::limits::usage::ClockObservation;
    let f = Fixture::new();
    let (run, identity, _) = reserve(&f);
    let host = f.host.owner.upgrade().unwrap();
    let producer = f.producer(&run);
    let now = super::super::spending::unix_ms().unwrap();
    host.lock()
        .unwrap()
        .change_spending(
            &producer,
            &identity,
            SpendingChange::Wait(Some("provider rate limit".into())),
            ClockObservation {
                unix_ms: now,
                monotonic_delta_ms: 0,
                trusted: true,
            },
        )
        .unwrap();
    let claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &f.temp.path("rate-machine"),
        identity.project.as_str(),
        run.0.as_str(),
    )
    .unwrap();
    let (claim, producer) = crate::effect_pump::recover_run(&host, claim, &run).unwrap();
    {
        let mut owner = host.lock().unwrap();
        let mut state = owner.spending_scope(&producer, &run).unwrap();
        owner.archive_retired_attempts(&mut state).unwrap();
        assert!(state.attempts.is_empty());
        owner
            .publish_native(&producer, state, &identity, vec![])
            .unwrap();
    }
    let reset = super::super::spending::unix_ms().unwrap();
    host.lock()
        .unwrap()
        .change_spending(
            &producer,
            &identity,
            SpendingChange::Wait(Some("provider rate limit".into())),
            ClockObservation {
                unix_ms: reset,
                monotonic_delta_ms: 0,
                trusted: true,
            },
        )
        .unwrap();
    let (mut pump, claim, profiles) =
        effect_pump(&host, producer, run.clone(), claim, f.temp.root());
    pump.tick().unwrap();
    assert!(profiles.observations.lock().unwrap().is_empty());
    let waiting = f.projection(&run);
    let until = host
        .lock()
        .unwrap()
        .native_accounting(&waiting)
        .unwrap()
        .unwrap()
        .tasks[0]
        .wait_until_unix_ms
        .unwrap();
    std::thread::sleep(Duration::from_millis(
        until.saturating_sub(super::super::spending::unix_ms().unwrap()) + 100,
    ));
    pump.tick().unwrap();
    assert_eq!(
        profiles.observations.lock().unwrap().len(),
        1,
        "C3_BOUNDED_RATE_WAIT_RESUMES"
    );
    let state = f.projection(&run);
    let record = host
        .lock()
        .unwrap()
        .native_accounting(&state)
        .unwrap()
        .unwrap();
    assert!(
        record.tasks[0].wait_reason.is_none()
            && record.elapsed_ms >= super::super::native_invocation::RATE_LIMIT_BACKOFF_MS,
        "C3_RATE_WAIT_SPENDS_CLOCK"
    );
    kinewright_core::AgentSession::interrupt(&mut pump);
    drop(pump);
    finish_stopped_native(claim);
    for scratch in profiles.observations.lock().unwrap().iter() {
        std::fs::remove_dir_all(scratch).unwrap();
    }
}
