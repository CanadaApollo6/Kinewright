#[derive(Clone)]
struct PumpProfiles {
    root: std::path::PathBuf,
    observations: Arc<Mutex<Vec<std::path::PathBuf>>>,
    checks: Arc<
        Mutex<
            std::collections::BTreeMap<
                String,
                crate::orchestration::adapters::RuntimeQualification,
            >,
        >,
    >,
}
impl crate::effect_pump::ProfileFactory for PumpProfiles {
    fn bind(
        &self,
        admitted: &crate::orchestration::AdmittedAttempt,
    ) -> std::io::Result<crate::orchestration::adapters::CertifiedConnectorProfile> {
        let (profile, checks, scratch) = native_profile_at(&self.root, admitted);
        self.checks
            .lock()
            .unwrap()
            .insert(admitted.identity().nonce.as_str().into(), checks.0);
        // End the fixture's one turn, after its completed usage frame.
        std::fs::write(scratch.join("exit"), b"")?;
        self.observations.lock().unwrap().push(scratch);
        Ok(profile)
    }
}
impl crate::orchestration::adapters::ConnectorRuntimeChecks for PumpProfiles {
    fn probe(
        &self,
        _: &crate::orchestration::adapters::ConnectorSpecification,
        binding: &crate::orchestration::adapters::AttemptBinding,
    ) -> Result<
        crate::orchestration::adapters::RuntimeQualification,
        crate::orchestration::adapters::CertificationRefusal,
    > {
        Ok(self
            .checks
            .lock()
            .unwrap()
            .get(binding.identity().nonce.as_str())
            .expect("factory ran outside host")
            .clone())
    }
}
fn effect_pump(
    host: &Arc<Mutex<HostCommandOwner>>,
    producer: HostAuth,
    run: RunId,
    claim: crate::execution_claim::MachineClaim,
    root: &std::path::Path,
) -> (
    crate::effect_pump::EffectPump,
    Arc<Mutex<crate::execution_claim::MachineClaim>>,
    PumpProfiles,
) {
    let profiles = PumpProfiles {
        root: root.into(),
        observations: Arc::default(),
        checks: Arc::default(),
    };
    let claim = Arc::new(Mutex::new(claim));
    let launcher = crate::native_launcher::NativeGuardianLauncher {
        claim: claim.clone(),
        owner: Arc::new(super::super::native_invocation::HostInvocationOwner::new(
            host,
            producer.clone(),
        )),
        guardian: kinewright_media::test_support::workspace_target_dir()
            .join("debug/kinewright-job-guardian"),
        checks: Arc::new(profiles.clone()),
        lose_commit_ack: false,
        after_release: None,
        startup_failure: None,
    };
    (
        crate::effect_pump::EffectPump::new(
            host,
            producer,
            run,
            launcher,
            Arc::new(profiles.clone()),
            None,
        ),
        claim,
        profiles,
    )
}
struct CrashCommit {
    inner: super::super::native_invocation::HostInvocationOwner,
    scratch: std::path::PathBuf,
}
impl crate::native_launcher::InvocationOwner for CrashCommit {
    fn unbound_completed(
        &self,
        identity: &ExecutionIdentity,
        proof: &crate::guardian::UnboundCleanupProof,
    ) -> std::io::Result<()> {
        self.inner.unbound_completed(identity, proof)
    }
    fn reserve(
        &self,
        v: &crate::orchestration::adapters::AuthorizedInvocation,
    ) -> std::io::Result<()> {
        self.inner.reserve(v)
    }
    fn release(
        &self,
        v: &crate::orchestration::adapters::AuthorizedInvocation,
        r: &crate::job_guardian::protocol::RecoveryIdentity,
        c: &crate::guardian::StartupControl,
    ) -> std::io::Result<()> {
        self.inner.release(v, r, c)
    }
    fn commit(
        &self,
        v: &crate::orchestration::adapters::AuthorizedInvocation,
        r: &crate::job_guardian::protocol::RecoveryIdentity,
        send: &mut dyn FnMut() -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        self.inner.commit(v, r, send)?;
        wait_native(&self.scratch.join("execution-ready"));
        // No Drop/cleanup and no EXECUTING read. The new host must recover it.
        std::process::exit(86)
    }
    fn started(&self, i: &ExecutionIdentity) -> std::io::Result<()> {
        self.inner.started(i)
    }
    fn raw(
        &self,
        i: &ExecutionIdentity,
        v: &serde_json::Value,
        n: u64,
        e: bool,
    ) -> std::io::Result<()> {
        self.inner.raw(i, v, n, e)
    }
    fn completed(
        &self,
        i: &ExecutionIdentity,
        c: bool,
        p: &crate::guardian::CleanupProof,
    ) -> std::io::Result<()> {
        self.inner.completed(i, c, p)
    }
    fn steering(&self, i: &ExecutionIdentity, t: &str) -> std::io::Result<()> {
        self.inner.steering(i, t)
    }
}
#[test]
fn s4b_c3_crash_helper() {
    use crate::orchestration::adapters::GuardianLauncher;
    let Some(path) = std::env::var_os("KINEWRIGHT_C3_CRASH_HANDOFF") else {
        return;
    };
    let after = std::env::var_os("KINEWRIGHT_C3_CRASH_AFTER").is_some();
    let f = Fixture::new();
    let (run, identity, _) = reserve(&f);
    let admitted = native_admitted(&f, &identity);
    let (profile, checks, scratch) = native_profile(&f, &admitted);
    let machine = f.temp.path("native-machine");
    let claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &machine,
        identity.project.as_str(),
        run.0.as_str(),
    )
    .unwrap();
    std::fs::write(path, serde_json::to_vec(&serde_json::json!({"root":f.temp.root(),"identity":identity,"machine":machine,"scratch":scratch})).unwrap()).unwrap();
    if !after {
        let refused = kinewright_project::io_observer::fail_nth("root_parent_sync", 2, || {
            let producer = f.producer(&run);
            let now = native_now(&f, &run);
            let effect = f.projection(&run).effects[0].id.clone();
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
                    &effect,
                    crate::orchestration::adapters::SendKind::OpeningAssignment,
                    now,
                )
        });
        assert!(refused.is_err(), "C3_LOST_CHARGE_ACK_SETUP");
        std::process::exit(86)
    }
    let invocation = native_charge(&f, &admitted, profile);
    let host = f.host.owner.upgrade().unwrap();
    let port = CrashCommit {
        inner: super::super::native_invocation::HostInvocationOwner::new(&host, f.producer(&run)),
        scratch,
    };
    let launcher = crate::native_launcher::NativeGuardianLauncher {
        claim: Arc::new(Mutex::new(claim)),
        owner: Arc::new(port),
        guardian: kinewright_media::test_support::workspace_target_dir()
            .join("debug/kinewright-job-guardian"),
        checks: Arc::new(checks),
        lose_commit_ack: false,
        after_release: None,
        startup_failure: None,
    };
    let _ = launcher.launch(admitted, invocation);
    panic!("crash cut did not exit");
}
fn real_recovery(after: bool) {
    let handoff = kinewright_media::test_support::TempDirectory::new_in(
        &kinewright_media::test_support::workspace_target_dir(),
        "c3-recovery",
    );
    let path = handoff.path("handoff.json");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap());
    child
        .args([
            "--exact",
            "writer_host::hooks_tests::tests::s4b_c3_crash_helper",
            "--nocapture",
        ])
        .env("KINEWRIGHT_C3_CRASH_HANDOFF", &path)
        .env_remove("KINEWRIGHT_C3_CRASH_AFTER");
    if after {
        child.env("KINEWRIGHT_C3_CRASH_AFTER", "yes");
    }
    let output = child.output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(86),
        "C3_REAL_CRASH_CUT: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let root = std::path::PathBuf::from(v["root"].as_str().unwrap());
    let machine = std::path::PathBuf::from(v["machine"].as_str().unwrap());
    let scratch = std::path::PathBuf::from(v["scratch"].as_str().unwrap());
    let identity: ExecutionIdentity = serde_json::from_value(v["identity"].clone()).unwrap();
    let _unit = NativeUnitCleanup::new(&identity);
    let roots = SessionRoots {
        read: vec![root.clone()],
        write: vec![root.clone()],
    };
    let writer = WriterHost::start(HostConfig {
        scheduler_settings: None,
        project: "hooks".into(),
        path: Some(root.join("project.kinewright")),
        roots,
        origin: HostOrigin::Headless,
    })
    .unwrap();
    let host = writer.owner.upgrade().unwrap();
    let original = host
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .orchestration_projection(&identity.run)
        .unwrap()
        .unwrap();
    assert_eq!(
        host.lock()
            .unwrap()
            .native_accounting(&original)
            .unwrap()
            .unwrap()
            .tasks[0]
            .sends
            .len(),
        1,
        "C3_REAL_CHARGE_CUT"
    );
    let claim = crate::execution_claim::MachineClaim::recover_fixture(&machine).unwrap();
    assert_eq!(claim.retained_attempts(), usize::from(after));
    if after {
        let recovery = claim.recovery(&claim.recovery_identities()[0]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        // Observe manager cleanup AFTER host loss; retain no pre-crash handle
        // and perform no cleanup here. Exercise the already-gone recovery path.
        while std::path::Path::new(&recovery.containment)
            .parent()
            .unwrap()
            .try_exists()
            .unwrap()
            || !kinewright_job_sys::process_identity_reaped(
                recovery.guardian_pid,
                recovery.guardian_start,
            )
            .unwrap()
        {
            assert!(
                Instant::now() < deadline,
                "C3_LATE_RECOVERY_NATIVE_TREE_GONE"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    let (claim, producer) = crate::effect_pump::recover_run(&host, claim, &identity.run).unwrap();
    assert_eq!(
        claim.retained_attempts(),
        0,
        "C3_RECOVERY_REQUIRES_NATIVE_PROOF"
    );
    let (mut pump, claim, profiles) =
        effect_pump(&host, producer, identity.run.clone(), claim, &root);
    pump.tick().unwrap();
    // A broken recovery that retries a PB effect executes a SECOND real child.
    for path in profiles.observations.lock().unwrap().iter() {
        wait_native(&path.join("execution-ready"));
    }
    let state = host
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .orchestration_projection(&identity.run)
        .unwrap()
        .unwrap();
    let record = host
        .lock()
        .unwrap()
        .native_accounting(&state)
        .unwrap()
        .unwrap();
    let new_processes = profiles.observations.lock().unwrap().len();
    let initial = std::fs::read_to_string(scratch.join("executions"))
        .unwrap_or_default()
        .lines()
        .count();
    kinewright_core::AgentSession::interrupt(&mut pump);
    drop(pump);
    finish_stopped_native(claim);
    for path in profiles.observations.lock().unwrap().iter() {
        std::fs::remove_dir_all(path).unwrap();
    }
    drop(host);
    drop(writer);
    std::fs::remove_dir_all(scratch).unwrap();
    std::fs::remove_dir_all(root).unwrap();
    assert_eq!(initial, usize::from(after));
    assert_eq!(new_processes, 0, "C3_LOST_ACK_NO_SECOND_PROCESS");
    assert_eq!(record.tasks[0].sends.len(), 1, "C3_LOST_ACK_ONE_CHARGE");
    assert_eq!(state.effects[0].state, EffectState::OutcomeUnknown);
}
#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c3_real_recovery_before_commit_has_no_duplicate() {
    real_recovery(false);
}
#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c3_real_recovery_after_commit_has_no_duplicate() {
    real_recovery(true);
}
#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c3_one_effect_pump_charges_and_records_raw_usage() {
    use kinewright_core::AgentSession;
    let f = Fixture::new();
    let started = f.start();
    let run: RunId = started.envelope.identity("run").unwrap();
    let host = f.host.owner.upgrade().unwrap();
    let claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &f.temp.path("native-machine"),
        started.envelope.project.as_str(),
        run.0.as_str(),
    )
    .unwrap();
    let (mut pump, claim, profiles) =
        effect_pump(&host, f.producer(&run), run.clone(), claim, f.temp.root());
    pump.send_user_message(String::new()).unwrap();
    let execution = f.projection(&run).attempts[0].identity.clone();
    let _unit = NativeUnitCleanup::new(&execution);
    let events = pump.events();
    let mut observer = EffectObserver {
        host: host.clone(),
        run: run.clone(),
        settled: false,
    };
    crate::pump_session(
        &mut pump,
        &events,
        &mut std::iter::once(String::new()),
        None,
        crate::ConfirmationPolicy::Studio,
        &crate::SessionLimits {
            max_turns: 1,
            max_wall_time: Duration::from_secs(10),
            max_tokens: None,
        },
        &Arc::new(crate::SharedCounters::default()),
        &Arc::new(std::sync::atomic::AtomicBool::new(false)),
        &mut observer,
    );
    assert!(observer.settled, "C3_SINGLE_PUMP_COMPLETES");
    let record = spending_record(&f, &run);
    assert_eq!(record.tasks[0].sends.len(), 1);
    assert_eq!(
        record.tasks[0].totals.known.input,
        Some(1),
        "C3_NATIVE_RAW_BEFORE_LEGACY_COST"
    );
    assert_eq!(record.tasks[0].totals.known.cost_micros, None);
    assert_eq!(profiles.observations.lock().unwrap().len(), 1);
    assert!(
        !pump
            .events()
            .try_iter()
            .any(|e| matches!(e, kinewright_core::AgentEvent::Cost { .. })),
        "C3_NO_ZERO_FILLED_COST_IN_PUMP"
    );
    drop(pump);
    finish_stopped_native(claim);
    for path in profiles.observations.lock().unwrap().iter() {
        std::fs::remove_dir_all(path).unwrap();
    }
}

struct EffectObserver {
    host: Arc<Mutex<HostCommandOwner>>,
    run: RunId,
    settled: bool,
}
impl crate::SessionObserver for EffectObserver {
    fn on_event(&mut self, event: &kinewright_core::AgentEvent) -> crate::SessionFlow {
        assert!(
            !matches!(event, kinewright_core::AgentEvent::Cost { .. }),
            "C3_NO_ZERO_FILLED_COST_IN_PUMP"
        );
        crate::SessionFlow::Continue
    }
    fn on_tick(&mut self, _: &crate::SessionCounters) -> crate::SessionFlow {
        self.settled = self
            .host
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .orchestration_projection(&self.run)
            .unwrap()
            .unwrap()
            .effects[0]
            .state
            == EffectState::Settled;
        if self.settled {
            crate::SessionFlow::Stop("fixture settled".into())
        } else {
            crate::SessionFlow::Continue
        }
    }
    fn prepare_studio_stop(
        &mut self,
        _: &crate::SessionStop,
        _: Option<&kinewright_core::RuntimeRequestBinding>,
    ) -> Result<(), String> {
        Ok(())
    }
}
