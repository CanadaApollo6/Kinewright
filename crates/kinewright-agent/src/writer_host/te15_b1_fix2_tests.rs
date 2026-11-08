struct Fix2Profiles {
    inner: Te15Profiles,
    barrier: Arc<(Mutex<u8>, std::sync::Condvar)>,
}
impl crate::effect_pump::ProfileFactory for Fix2Profiles {
    fn bind(
        &self,
        attempt: &crate::orchestration::AdmittedAttempt,
    ) -> std::io::Result<crate::orchestration::adapters::CertifiedConnectorProfile> {
        let profile = crate::effect_pump::ProfileFactory::bind(&self.inner, attempt)?;
        let (lock, wake) = &*self.barrier;
        let mut step = lock.lock().unwrap();
        *step = 1;
        wake.notify_all();
        step = wake
            .wait_timeout_while(step, Duration::from_secs(10), |step| *step != 2)
            .unwrap()
            .0;
        assert_eq!(*step, 2, "TE15_FIX2_BOUND_BARRIER_RELEASED");
        Ok(profile)
    }
}
fn fix2_bound_case(settings: bool) {
    let (f, checks) = te15_fix_setup("codex");
    if !settings {
        te15_fix_specialist(&f, "claude");
    }
    let run: RunId = f.start().envelope.identity("run").unwrap();
    let host = f.host.owner.upgrade().unwrap();
    let project = host.lock().unwrap().spending_project().unwrap();
    let claim = Arc::new(Mutex::new(
        crate::execution_claim::MachineClaim::acquire_fixture(
            &f.temp.path("bound-machine"),
            project.as_str(),
            run.0.as_str(),
        )
        .unwrap(),
    ));
    let count = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new((Mutex::new(0), std::sync::Condvar::new()));
    let profiles = Arc::new(Fix2Profiles {
        inner: Te15Profiles {
            runtime: f.host.runtime.clone(),
            host: Arc::downgrade(&host),
            servers: Mutex::default(),
            scratch: Mutex::default(),
        },
        barrier: barrier.clone(),
    });
    let mut pump = crate::effect_pump::EffectPump::with_launcher(
        &host,
        f.producer(&run),
        run.clone(),
        claim,
        Te15CountingLauncher(count.clone()),
        profiles,
        None,
    );
    let worker = std::thread::spawn(move || {
        let result = pump.tick();
        (pump, result)
    });
    let (lock, wake) = &*barrier;
    let mut step = lock.lock().unwrap();
    step = wake
        .wait_timeout_while(step, Duration::from_secs(10), |step| *step != 1)
        .unwrap()
        .0;
    assert_eq!(*step, 1, "TE15_FIX2_BOUND_BARRIER_ENTERED");
    if settings {
        std::fs::create_dir_all(f.temp.path("scheduler")).unwrap();
        std::fs::write(f.temp.path("scheduler/specialists.json"), b"broken").unwrap();
    } else {
        *checks.failed.lock().unwrap() = Some("claude".into());
    }
    *step = 2;
    wake.notify_all();
    drop(step);
    let (mut pump, _result) = worker.join().unwrap();
    let state = f.projection(&run);
    assert_eq!(
        count.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "TE15_FIX2_FINAL_ZERO_SEND"
    );
    assert!(
        matches!(state.wait, RunWait::ProviderSettings(_)),
        "TE15_FIX2_FINAL_WAIT"
    );
    let accounting = host
        .lock()
        .unwrap()
        .native_accounting(&state)
        .unwrap()
        .unwrap();
    assert_eq!(
        accounting
            .tasks
            .iter()
            .map(|t| t.sends.len())
            .sum::<usize>(),
        0,
        "TE15_FIX2_FINAL_ZERO_CHARGE"
    );
    let sequence = state.sequence;
    for _ in 0..3 {
        pump.tick().unwrap();
    }
    assert_eq!(
        f.projection(&run).sequence,
        sequence,
        "TE15_FIX2_FINAL_ONE_WAIT"
    );
}
#[test]
fn te15_b1_fix2_bound_signout_does_not_charge() {
    if !te15_fix_counting_child("te15_b1_fix2_bound_signout_does_not_charge") {
        return;
    }
    fix2_bound_case(false);
}
#[test]
fn te15_b1_fix2_bound_settings_does_not_charge() {
    if !te15_fix_counting_child("te15_b1_fix2_bound_settings_does_not_charge") {
        return;
    }
    fix2_bound_case(true);
}

struct Fix2ImageChecks(std::path::PathBuf);
impl ConnectorRuntimeChecks for Fix2ImageChecks {
    fn probe(
        &self,
        spec: &ConnectorSpecification,
        binding: &crate::orchestration::adapters::AttemptBinding,
    ) -> Result<crate::orchestration::adapters::RuntimeQualification, CertificationRefusal> {
        Ok(te15_profile_runtime(spec, binding, &self.0))
    }
}
struct Fix2NativeProfiles {
    image: Option<kinewright_media::test_support::TempDirectory>,
    runtime: Arc<super::super::studio_runtime::StudioRuntime>,
    servers: Mutex<Vec<crate::McpServer>>,
    root: std::path::PathBuf,
    host: std::sync::Weak<Mutex<HostCommandOwner>>,
    producer: HostAuth,
    runtimes: Mutex<
        std::collections::BTreeMap<AttemptId, crate::orchestration::adapters::RuntimeQualification>,
    >,
    locations: Mutex<Vec<(ExecutionIdentity, std::path::PathBuf)>>,
    units: Mutex<Vec<NativeUnitCleanup>>,
    refusal: Option<CertificationRefusal>,
}
impl crate::effect_pump::ProfileFactory for Fix2NativeProfiles {
    fn bind(
        &self,
        attempt: &crate::orchestration::AdmittedAttempt,
    ) -> std::io::Result<crate::orchestration::adapters::CertifiedConnectorProfile> {
        let id = attempt.identity();
        self.host
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .prepare_task_endpoint(
                &self.producer,
                &id.run,
                &id.task,
                &id.attempt,
                Duration::from_secs(300),
            )
            .unwrap();
        let (profile, runtime, scratch) = if let Some(image) = &self.image {
            let server = self
                .host
                .upgrade()
                .unwrap()
                .lock()
                .unwrap()
                .start_task_server(&self.producer, &id.run, &id.task)
                .unwrap();
            let profile = self.runtime.bind_profile_for_test(
                attempt,
                server.endpoint(),
                Arc::new(Fix2ImageChecks(image.path("node_modules/fixture/bin/tool"))),
            )?;
            let runtime = profile.runtime().clone();
            let scratch = profile.binding().working_directory().to_path_buf();
            self.servers.lock().unwrap().push(server);
            (profile, runtime, scratch)
        } else {
            let (profile, checks, scratch) = native_profile_at(&self.root, attempt);
            (profile, checks.0, scratch)
        };
        std::fs::write(scratch.join("event-mode"), b"hold-turn").unwrap();
        self.runtimes
            .lock()
            .unwrap()
            .insert(id.attempt.clone(), runtime);
        self.units.lock().unwrap().push(NativeUnitCleanup::new(id));
        self.locations.lock().unwrap().push((id.clone(), scratch));
        Ok(profile)
    }
}
impl ConnectorRuntimeChecks for Fix2NativeProfiles {
    fn probe(
        &self,
        _: &ConnectorSpecification,
        binding: &crate::orchestration::adapters::AttemptBinding,
    ) -> Result<crate::orchestration::adapters::RuntimeQualification, CertificationRefusal> {
        if let Some(refusal) = &self.refusal {
            return Err(refusal.clone());
        }
        Ok(self.runtimes.lock().unwrap()[&binding.identity().attempt].clone())
    }
}
struct Fix2NativeLauncher {
    inner: crate::native_launcher::NativeGuardianLauncher,
    sends: Arc<Mutex<Vec<crate::orchestration::adapters::SendKind>>>,
}
impl crate::orchestration::adapters::GuardianLauncher for Fix2NativeLauncher {
    fn launch(
        &self,
        attempt: crate::orchestration::AdmittedAttempt,
        invocation: crate::orchestration::adapters::AuthorizedInvocation,
    ) -> Result<
        crate::orchestration::adapters::ContainedSession,
        crate::orchestration::adapters::LaunchError,
    > {
        let kind = invocation.invocation().kind();
        let result = crate::orchestration::adapters::GuardianLauncher::launch(
            &self.inner,
            attempt,
            invocation,
        )?;
        self.sends.lock().unwrap().push(kind);
        Ok(result)
    }
}
struct Fix2NativeRun {
    pump: Option<crate::effect_pump::EffectPump<Fix2NativeLauncher>>,
    profiles: Arc<Fix2NativeProfiles>,
    claim: Option<Arc<Mutex<crate::execution_claim::MachineClaim>>>,
    sends: Arc<Mutex<Vec<crate::orchestration::adapters::SendKind>>>,
    original: std::path::PathBuf,
    machine: std::path::PathBuf,
}
impl Fix2NativeRun {
    fn new(f: &Fixture, run: &RunId) -> Self {
        Self::with_failure(f, run, None, None)
    }
    fn with_failure(
        f: &Fixture,
        run: &RunId,
        startup_failure: Option<crate::native_launcher::StartupFailure>,
        refusal: Option<CertificationRefusal>,
    ) -> Self {
        let host = f.host.owner.upgrade().unwrap();
        let producer = f.producer(run);
        let project = host.lock().unwrap().spending_project().unwrap();
        let machine = f.temp.path("fix2-machine");
        let claim = Arc::new(Mutex::new(
            crate::execution_claim::MachineClaim::acquire_fixture(
                &machine,
                project.as_str(),
                run.0.as_str(),
            )
            .unwrap(),
        ));
        let image = if f.brief.provider.connector == "codex" {
            use std::os::unix::fs::PermissionsExt;
            let image = kinewright_media::test_support::TempDirectory::new_in(
                &kinewright_media::test_support::workspace_target_dir(),
                "te15-native-install",
            );
            std::fs::create_dir_all(image.path("node_modules/fixture/bin")).unwrap();
            std::fs::write(image.path("node_modules/fixture/package.json"), b"{}").unwrap();
            std::fs::copy(native_child(), image.path("node_modules/fixture/bin/tool")).unwrap();
            std::fs::set_permissions(
                image.path("node_modules/fixture/bin/tool"),
                std::fs::Permissions::from_mode(0o700),
            )
            .unwrap();
            Some(image)
        } else {
            None
        };
        let profiles = Arc::new(Fix2NativeProfiles {
            image,
            runtime: f.host.runtime.clone(),
            servers: Mutex::default(),
            root: f.temp.root().into(),
            host: Arc::downgrade(&host),
            producer: producer.clone(),
            runtimes: Mutex::default(),
            locations: Mutex::default(),
            units: Mutex::default(),
            refusal,
        });
        let sends = Arc::new(Mutex::new(vec![]));
        let launcher = Fix2NativeLauncher {
            sends: sends.clone(),
            inner: crate::native_launcher::NativeGuardianLauncher {
                images: Arc::default(),
                claim: claim.clone(),
                owner: Arc::new(super::super::native_invocation::HostInvocationOwner::new(
                    &host,
                    producer.clone(),
                )),
                guardian: kinewright_media::test_support::workspace_target_dir()
                    .join("debug/kinewright-job-guardian"),
                checks: profiles.clone(),
                first_progress: crate::native_launcher::FIRST_PROGRESS,
                lose_commit_ack: false,
                after_release: None,
                startup_failure,
            },
        };
        Self {
            pump: Some(crate::effect_pump::EffectPump::with_launcher(
                &host,
                producer,
                run.clone(),
                claim.clone(),
                launcher,
                profiles.clone(),
                None,
            )),
            profiles,
            claim: Some(claim),
            sends,
            original: f.temp.path("project.kinewright"),
            machine,
        }
    }
    fn tick(&mut self) {
        self.pump.as_mut().unwrap().tick().unwrap();
    }
    fn location(&self, index: usize) -> (ExecutionIdentity, std::path::PathBuf) {
        let result = self.profiles.locations.lock().unwrap().get(index).cloned();
        let result = result.expect("TE15_FIX2_EXPECTED_NATIVE_SEND");
        wait_native(&result.1.join("execution-ready"));
        result
    }
    fn finish_turn(&mut self, f: &Fixture, index: usize) {
        let (identity, path) = self.location(index);
        std::fs::write(path.join("final-release"), b"").unwrap();
        std::fs::write(path.join("exit"), b"").unwrap();
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            self.tick();
            let retired = f
                .projection(&identity.run)
                .attempts
                .iter()
                .find(|a| a.identity == identity)
                .is_none_or(|a| matches!(a.state, AttemptState::Retired(_)));
            let private = self
                .claim
                .as_ref()
                .unwrap()
                .lock()
                .unwrap()
                .recovery_identities()
                .iter()
                .any(|native| {
                    super::super::native_invocation::same_native_attempt(&identity, native)
                });
            if retired && !private {
                break;
            }
            assert!(Instant::now() < end, "TE15_FIX2_NATIVE_FINISH");
            std::thread::sleep(Duration::from_millis(5));
        }
        self.tick();
    }
}
impl Drop for Fix2NativeRun {
    fn drop(&mut self) {
        drop(self.pump.take());
        finish_native_after_owner_loss(self.claim.take().unwrap(), &self.original, &self.machine);
        for (_, path) in self
            .profiles
            .locations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
        {
            let _ = std::fs::remove_dir_all(path.with_extension("fixture-auth"));
            let _ = std::fs::remove_dir_all(path);
        }
    }
}
fn fix2_live_continue(mixed: bool, steering: bool) {
    let f = if mixed {
        let (f, _) = te15_fix_setup("codex");
        te15_fix_specialist(&f, "claude");
        te13_candidate_fixture(f)
    } else {
        te13_fixture()
    };
    f.host.runtime.stop();
    let run: RunId = f.start().envelope.identity("run").unwrap();
    let mut native = Fix2NativeRun::new(&f, &run);
    native.tick();
    let (director, _) = native.location(0);
    let mcp = C4Mcp::new(&f, &run, &director.task);
    let queued = if steering {
        let text_ref = f
            .host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .owner_mut()
            .unwrap()
            .admit_host_grant(b"Keep this steering message.")
            .unwrap();
        ok(
            f.editor
                .request(HostCommand::DispatchOrchestration {
                    request: Box::new(f.control(
                        &f.editor,
                        &run,
                        RunAction::SendMessage { text_ref },
                    )),
                })
                .unwrap(),
            "TE15_FIX2_QUEUED_STEERING",
        );
        Some(f.projection(&run).effects.last().unwrap().id.clone())
    } else {
        None
    };
    let child = if mixed {
        let mut brief = f.brief.clone();
        brief.role = Role::Editor;
        mcp.accepted(
            "assign_task",
            OrchestrationCommand::AssignTask(Box::new(AssignTask {
                run: run.clone(),
                parent: director.task.clone(),
                brief,
            })),
        );
        native.finish_turn(&f, 0);
        let (identity, _) = native.location(1);
        let editor = C4Mcp::new(&f, &run, &identity.task);
        let core = te13_core(&f, &run, &identity.task);
        Some((identity, editor, core))
    } else {
        None
    };
    if mixed {
        let state = f.projection(&run);
        assert_eq!(
            state.tasks[0].brief.provider.connector, "codex",
            "TE15_FIX2_MIXED_DIRECTOR"
        );
        assert_eq!(
            state.tasks[1].brief.provider.connector, "claude",
            "TE15_FIX2_MIXED_CHILD"
        );
    }
    std::fs::create_dir_all(f.temp.path("scheduler")).unwrap();
    std::fs::write(f.temp.path("scheduler/specialists.json"), b"broken").unwrap();
    if mixed {
        let reply = f
            .editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(f.control(
                    &f.editor,
                    &run,
                    RunAction::SendMessage {
                        text_ref: f.brief.assignment.clone(),
                    },
                )),
            })
            .unwrap();
        assert!(reply.error.is_some(), "TE15_FIX2_PERSON_SETUP_WAIT");
    } else {
        let mut brief = f.brief.clone();
        brief.role = Role::Editor;
        let request = mcp.control(OrchestrationCommand::AssignTask(Box::new(AssignTask {
            run: run.clone(),
            parent: director.task.clone(),
            brief,
        })));
        let refusal = mcp.wire("assign_task", request);
        assert_eq!(refusal.is_error, Some(true), "TE15_FIX2_MODEL_WAIT");
        assert!(
            format!("{refusal:?}").contains("End your turn now. Do not retry this call."),
            "TE15_FIX2_END_TURN_WORDS"
        );
    }
    native.tick();
    let owner = f.host.owner.upgrade().unwrap();
    let state = f.projection(&run);
    let preserved = owner
        .lock()
        .unwrap()
        .hooks
        .native
        .values()
        .filter(|n| {
            n.identity.run == run
                && state.attempts.iter().any(|a| {
                    a.identity == n.identity && !matches!(a.state, AttemptState::Retired(_))
                })
        })
        .all(|n| !n.control.closed());
    assert!(preserved, "TE15_FIX2_ALL_LIVE_PRESERVED");
    if !mixed {
        native.finish_turn(&f, 0);
    }
    let pending = if let Some((child, editor, core)) = child {
        let status = editor.wire("task_status", serde_json::json!({}));
        assert_eq!(
            status.is_error,
            Some(false),
            "TE15_FIX2_CHILD_CAN_FINISH {status:?}"
        );
        let mut finish = status.structured_content.unwrap()["controls"].clone();
        finish["command"] = serde_json::to_value(OrchestrationCommand::FinishTask(FinishTask {
            task: child.task,
            expected_revision: grant_test_revision(&core),
        }))
        .unwrap();
        let result = editor.wire("finish_task", finish);
        assert_eq!(
            result.is_error,
            Some(false),
            "TE15_FIX2_CHILD_CAN_FINISH {result:?}"
        );
        native.finish_turn(&f, 1);
        let state = f.projection(&run);
        assert_eq!(state.deliveries.len(), 1, "TE15_FIX2_CHILD_WAKE_QUEUED");
        Some(state.deliveries[0].wake.clone())
    } else {
        queued
    };
    let before = f.projection(&run);
    let count = native.sends.lock().unwrap().len();
    assert_eq!(count, if mixed { 2 } else { 1 }, "TE15_FIX2_NO_WAIT_SEND");
    assert!(
        before
            .effects
            .iter()
            .all(|e| e.state != EffectState::OutcomeUnknown),
        "TE15_FIX2_NO_UNKNOWN_HEAD"
    );
    let receipts = owner
        .lock()
        .unwrap()
        .native_accounting(&before)
        .unwrap()
        .unwrap()
        .tasks
        .into_iter()
        .flat_map(|t| t.sends)
        .collect::<Vec<_>>();
    if mixed {
        te15_fix_specialist(&f, "claude");
    } else {
        std::fs::remove_file(f.temp.path("scheduler/specialists.json")).unwrap();
    }
    ok(
        f.editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(f.control(&f.editor, &run, RunAction::Continue)),
            })
            .unwrap(),
        "TE15_FIX2_CONTINUE_ACCEPTED",
    );
    let state = f.projection(&run);
    assert!(
        !crate::orchestration::machine::silently_stalled(&state),
        "TE15_FIX2_REAL_CONTINUATION"
    );
    if let Some(id) = &pending {
        assert_eq!(
            state.effects.len(),
            before.effects.len(),
            "TE15_FIX2_PENDING_REUSED"
        );
        assert!(
            state
                .effects
                .iter()
                .any(|e| &e.id == id && e.state == EffectState::Pending),
            "TE15_FIX2_PENDING_ID_RETAINED"
        );
    } else {
        let effect = crate::orchestration::outbox::next_effect(
            &state,
            &state.tasks[0].brief.scope.base.branch,
        )
        .expect("TE15_FIX2_REAL_CONTINUATION");
        assert_eq!(
            owner
                .lock()
                .unwrap()
                .owner()
                .unwrap()
                .store()
                .unwrap()
                .read_evidence(&effect.payload)
                .unwrap()
                .as_slice(),
            crate::orchestration::machine::SETUP_CONTINUED,
            "TE15_FIX2_RESUME_WORDS"
        );
    }
    native.tick();
    native.location(count);
    assert_eq!(
        native.sends.lock().unwrap().len(),
        count + 1,
        "TE15_FIX2_ONE_NEW_SEND"
    );
    if pending.is_some() {
        assert_eq!(
            native.sends.lock().unwrap().last(),
            Some(&if mixed {
                crate::orchestration::adapters::SendKind::AutomaticResultWake
            } else {
                crate::orchestration::adapters::SendKind::QueuedSteering
            }),
            "TE15_FIX2_PENDING_SEND_KIND"
        );
    }
    let after = f.projection(&run);
    let accounting = owner
        .lock()
        .unwrap()
        .native_accounting(&after)
        .unwrap()
        .unwrap();
    let sends = accounting
        .tasks
        .iter()
        .flat_map(|t| t.sends.iter())
        .collect::<Vec<_>>();
    assert_eq!(
        sends.len(),
        receipts.len() + 1,
        "TE15_FIX2_NO_DOUBLE_CHARGE"
    );
    for receipt in &receipts {
        assert!(
            sends.contains(&receipt),
            "TE15_FIX2_ORIGINAL_CHARGE_RETAINED"
        );
    }
    native.finish_turn(&f, count);
}
#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn te15_b1_fix2_live_continue_sends_once() {
    fix2_live_continue(false, false);
}
#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn te15_b1_fix2_live_child_wakes_after_repair() {
    if !te15_fix_counting_child("te15_b1_fix2_live_child_wakes_after_repair") {
        return;
    }
    fix2_live_continue(true, false);
}
#[test]
fn te15_b1_fix2_stream_refusal_preserves_setup_wait() {
    let f = Fixture::new();
    let (run, identity, _) = reserve(&f);
    let owner = f.host.owner.upgrade().unwrap();
    let producer = f.producer(&run);
    let mut owner = owner.lock().unwrap();
    owner.wait_for_provider(&run, "Fix Settings.").unwrap();
    let before = owner.spending_scope(&producer, &run).unwrap();
    owner
        .record_stream_refusal(
            &producer,
            &identity,
            crate::native_launcher::STREAM_USAGE_UNRESOLVED,
        )
        .unwrap();
    let after = owner.spending_scope(&producer, &run).unwrap();
    assert_eq!(before, after, "TE15_FIX2_STREAM_WAIT_PRESERVED");
}
#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn te15_b1_fix2_setup_recheck_wait_and_unknown_continue_refusal() {
    let f = te13_fixture();
    f.host.runtime.stop();
    let run: RunId = f.start().envelope.identity("run").unwrap();
    let mut native = Fix2NativeRun::with_failure(
        &f,
        &run,
        None,
        Some(CertificationRefusal::UnknownAuthentication),
    );
    let result = native.pump.as_mut().unwrap().tick();
    if let Err(error) = &result {
        let producer = f.producer(&run);
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .record_runtime_refusal(&producer, &run, &error.to_string())
            .unwrap();
    }
    let state = f.projection(&run);
    assert!(
        matches!(state.wait, RunWait::ProviderSettings(_)),
        "TE15_FIX2_SETUP_RECHECK_CLASSIFIED"
    );
    assert!(result.is_ok(), "TE15_FIX2_SETUP_WAIT_PUMP_ALIVE");
    assert_eq!(
        state.effects[0].state,
        EffectState::OutcomeUnknown,
        "TE15_FIX2_UNCERTAIN_SEND_RETAINED"
    );
    let reply = f
        .editor
        .request(HostCommand::DispatchOrchestration {
            request: Box::new(f.control(&f.editor, &run, RunAction::Continue)),
        })
        .unwrap();
    assert!(
        reply
            .error
            .as_ref()
            .is_some_and(|e| e.message.contains("unknown send outcome")),
        "TE15_FIX2_UNKNOWN_CONTINUE_REFUSED {reply:?}"
    );
}
#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn te15_b1_fix2_postcommit_error_keeps_provider_recovery() {
    let f = te13_fixture();
    f.host.runtime.stop();
    let run: RunId = f.start().envelope.identity("run").unwrap();
    let mut native = Fix2NativeRun::with_failure(
        &f,
        &run,
        Some(crate::native_launcher::StartupFailure::Reader),
        None,
    );
    let result = native.pump.as_mut().unwrap().tick();
    let owner = f.host.owner.upgrade().unwrap();
    let producer = f.producer(&run);
    if let Err(error) = &result {
        owner
            .lock()
            .unwrap()
            .record_runtime_refusal(&producer, &run, &error.to_string())
            .unwrap();
    }
    let state = f.projection(&run);
    assert!(
        matches!(state.wait, RunWait::Provider(_)),
        "TE15_FIX2_POSTCOMMIT_PROVIDER_WAIT"
    );
    let error = result.expect_err("TE15_FIX2_POSTCOMMIT_PROVIDER_WAIT");
    assert!(
        !crate::orchestration::machine::eligible(&state, &RunAction::Continue),
        "TE15_FIX2_POSTCOMMIT_NOT_CONTINUABLE"
    );
    assert_eq!(
        state.effects[0].state,
        EffectState::OutcomeUnknown,
        "TE15_FIX2_POSTCOMMIT_UNKNOWN"
    );
    let accounting = owner
        .lock()
        .unwrap()
        .native_accounting(&state)
        .unwrap()
        .unwrap();
    assert_eq!(
        accounting
            .tasks
            .iter()
            .map(|t| t.sends.len())
            .sum::<usize>(),
        1,
        "TE15_FIX2_POSTCOMMIT_ORIGINAL_CHARGE"
    );
    assert!(
        !error.to_string().contains("Settings"),
        "TE15_FIX2_POSTCOMMIT_TRUE_WORDS"
    );
}
struct Fix2ProbeProfiles {
    inner: Te15Profiles,
    binds: Arc<AtomicUsize>,
}
impl crate::effect_pump::ProfileFactory for Fix2ProbeProfiles {
    fn bind(
        &self,
        attempt: &crate::orchestration::AdmittedAttempt,
    ) -> std::io::Result<crate::orchestration::adapters::CertifiedConnectorProfile> {
        let result = crate::effect_pump::ProfileFactory::bind(&self.inner, attempt)?;
        self.binds.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(result)
    }
}
struct Fix2ProbeLauncher {
    rechecks: Arc<AtomicUsize>,
}
impl crate::orchestration::adapters::GuardianLauncher for Fix2ProbeLauncher {
    fn launch(
        &self,
        _attempt: crate::orchestration::AdmittedAttempt,
        invocation: crate::orchestration::adapters::AuthorizedInvocation,
    ) -> Result<
        crate::orchestration::adapters::ContainedSession,
        crate::orchestration::adapters::LaunchError,
    > {
        self.rechecks
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        invocation
            .profile()
            .recheck_runtime(&NativeChecks(invocation.profile().runtime().clone()))
            .unwrap();
        Ok(
            crate::orchestration::adapters::ContainedSession::from_guardian(Box::new(
                Te15NoProcess(crossbeam_channel::unbounded().1),
            )),
        )
    }
}
#[test]
fn te15_b1_fix2_probe_counts_stay_bounded() {
    if !te15_fix_counting_child("te15_b1_fix2_probe_counts_stay_bounded") {
        return;
    }
    for mixed in [false, true] {
        let (f, checks) = te15_fix_setup("codex");
        if mixed {
            te15_fix_specialist(&f, "claude");
        }
        let run: RunId = f.start().envelope.identity("run").unwrap();
        checks.checked.lock().unwrap().clear();
        let host = f.host.owner.upgrade().unwrap();
        let project = host.lock().unwrap().spending_project().unwrap();
        let claim = Arc::new(Mutex::new(
            crate::execution_claim::MachineClaim::acquire_fixture(
                &f.temp.path("probe-machine"),
                project.as_str(),
                run.0.as_str(),
            )
            .unwrap(),
        ));
        let binds = Arc::new(AtomicUsize::new(0));
        let rechecks = Arc::new(AtomicUsize::new(0));
        let profiles = Arc::new(Fix2ProbeProfiles {
            binds: binds.clone(),
            inner: Te15Profiles {
                runtime: f.host.runtime.clone(),
                host: Arc::downgrade(&host),
                servers: Mutex::default(),
                scratch: Mutex::default(),
            },
        });
        let mut pump = crate::effect_pump::EffectPump::with_launcher(
            &host,
            f.producer(&run),
            run,
            claim,
            Fix2ProbeLauncher {
                rechecks: rechecks.clone(),
            },
            profiles,
            None,
        );
        pump.tick().unwrap();
        let bind = binds.load(std::sync::atomic::Ordering::SeqCst);
        let recheck = rechecks.load(std::sync::atomic::Ordering::SeqCst);
        let preflight = checks.checked.lock().unwrap().len();
        println!(
            "TE15_FIX2_PROBE_COUNTS mixed={mixed} preflight={preflight} bind={bind} recheck={recheck} total={}",
            preflight + bind + recheck
        );
        assert_eq!((bind, recheck), (1, 1), "TE15_FIX2_BOUND_PROBE_REUSE");
        assert_eq!(
            preflight + bind + recheck,
            if mixed { 5 } else { 3 },
            "TE15_FIX2_PROBE_BUDGET"
        );
    }
}
#[test]
fn te15_b1_fix2_queued_failed_worker_needs_a_new_continue() {
    let (f, checks) = te15_fix_setup("codex");
    let run: RunId = f.start().envelope.identity("run").unwrap();
    let owner = f.host.owner.upgrade().unwrap();
    let runtime = super::super::studio_runtime::StudioRuntime::new(
        &owner,
        super::super::studio_runtime::RuntimePorts {
            checks: checks.clone(),
            ceiling: None,
            fixture_claim: Some(f.temp.path("blocked-machine")),
            ..Default::default()
        },
    );
    runtime.failed_worker_fixture(&run);
    checks.checked.lock().unwrap().clear();
    for _ in 0..3 {
        assert_eq!(
            runtime.drive().unwrap_err().to_string(),
            "fixture worker failed",
            "TE15_FIX2_BLOCKED_QUEUED"
        );
    }
    assert!(
        checks.checked.lock().unwrap().is_empty(),
        "TE15_FIX2_NO_IDLE_REPROBE"
    );
    owner
        .lock()
        .unwrap()
        .wait_for_provider(&run, "Fix Settings.")
        .unwrap();
    let request = f.control(&f.editor, &run, RunAction::Continue);
    ok(
        f.editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(request.clone()),
            })
            .unwrap(),
        "TE15_FIX2_NEW_CONTINUE",
    );
    assert!(
        runtime
            .drive()
            .unwrap_err()
            .to_string()
            .contains("positive token ceiling"),
        "TE15_FIX2_CONTINUE_UNBLOCKED"
    );
    runtime.failed_worker_fixture(&run);
    assert_eq!(
        runtime.drive().unwrap_err().to_string(),
        "fixture worker failed"
    );
    ok(
        f.editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(request),
            })
            .unwrap(),
        "TE15_FIX2_CONTINUE_REPLAY",
    );
    assert_eq!(
        runtime.drive().unwrap_err().to_string(),
        "fixture worker failed",
        "TE15_FIX2_REPLAY_NOT_A_NEW_RESUME"
    );
    runtime.stop();
}
struct Fix2FinalChecks {
    inner: Arc<Te15Readiness>,
    calls: AtomicUsize,
    barrier: Arc<(Mutex<u8>, std::sync::Condvar)>,
}
impl ConnectorRuntimeChecks for Fix2FinalChecks {
    fn preflight(&self, spec: &ConnectorSpecification) -> Result<(), CertificationRefusal> {
        if spec.provider() == "claude"
            && self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1
        {
            let (lock, wake) = &*self.barrier;
            let mut step = lock.lock().unwrap();
            *step = 1;
            wake.notify_all();
            step = wake
                .wait_timeout_while(step, Duration::from_secs(10), |step| *step != 2)
                .unwrap()
                .0;
            assert_eq!(*step, 2, "TE15_FIX2_CHARGE_BARRIER_RELEASED");
        }
        self.inner.preflight(spec)
    }
    fn probe(
        &self,
        _: &ConnectorSpecification,
        _: &crate::orchestration::adapters::AttemptBinding,
    ) -> Result<crate::orchestration::adapters::RuntimeQualification, CertificationRefusal> {
        unreachable!()
    }
}
#[test]
fn te15_b1_fix2_settings_compare_is_under_the_charge_lock() {
    if !te15_fix_counting_child("te15_b1_fix2_settings_compare_is_under_the_charge_lock") {
        return;
    }
    let (f, checks) = te15_fix_setup("codex");
    te15_fix_specialist(&f, "claude");
    let run: RunId = f.start().envelope.identity("run").unwrap();
    let host = f.host.owner.upgrade().unwrap();
    let project = host.lock().unwrap().spending_project().unwrap();
    let barrier = Arc::new((Mutex::new(0), std::sync::Condvar::new()));
    host.lock().unwrap().native_readiness_checks = Some(Arc::new(Fix2FinalChecks {
        inner: checks,
        calls: AtomicUsize::new(0),
        barrier: barrier.clone(),
    }));
    let claim = Arc::new(Mutex::new(
        crate::execution_claim::MachineClaim::acquire_fixture(
            &f.temp.path("last-machine"),
            project.as_str(),
            run.0.as_str(),
        )
        .unwrap(),
    ));
    let count = Arc::new(AtomicUsize::new(0));
    let profiles = Arc::new(Te15Profiles {
        runtime: f.host.runtime.clone(),
        host: Arc::downgrade(&host),
        servers: Mutex::default(),
        scratch: Mutex::default(),
    });
    let mut pump = crate::effect_pump::EffectPump::with_launcher(
        &host,
        f.producer(&run),
        run.clone(),
        claim,
        Te15CountingLauncher(count.clone()),
        profiles,
        None,
    );
    let worker = std::thread::spawn(move || {
        let result = pump.tick();
        (pump, result)
    });
    let (lock, wake) = &*barrier;
    let step = lock.lock().unwrap();
    let mut step = wake
        .wait_timeout_while(step, Duration::from_secs(10), |step| *step != 1)
        .unwrap()
        .0;
    assert_eq!(*step, 1, "TE15_FIX2_CHARGE_BARRIER_ENTERED");
    std::fs::write(f.temp.path("scheduler/specialists.json"), b"broken").unwrap();
    *step = 2;
    wake.notify_all();
    drop(step);
    let (_pump, _result) = worker.join().unwrap();
    assert_eq!(
        count.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "TE15_FIX2_CHARGE_COMPARE"
    );
    let state = f.projection(&run);
    assert!(
        matches!(state.wait, RunWait::ProviderSettings(_)),
        "TE15_FIX2_CHARGE_WAIT"
    );
    assert_eq!(
        host.lock()
            .unwrap()
            .native_accounting(&state)
            .unwrap()
            .unwrap()
            .tasks
            .iter()
            .map(|t| t.sends.len())
            .sum::<usize>(),
        0,
        "TE15_FIX2_CHARGE_COMPARE"
    );
}

#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn te15_b1_fix2_live_steering_reuses_its_effect() {
    fix2_live_continue(false, true);
}

struct Fix2SandboxUnavailable;
impl ConnectorRuntimeChecks for Fix2SandboxUnavailable {
    fn preflight(&self, _: &ConnectorSpecification) -> Result<(), CertificationRefusal> {
        Err(CertificationRefusal::SandboxUnavailable)
    }
    fn probe(
        &self,
        _: &ConnectorSpecification,
        _: &crate::orchestration::adapters::AttemptBinding,
    ) -> Result<crate::orchestration::adapters::RuntimeQualification, CertificationRefusal> {
        panic!("TE15_FIX2_NONSETUP_NO_BIND");
    }
}
#[test]
fn te15_b1_fix2_nonsetup_model_refusal_keeps_provider_words() {
    if !te15_fix_counting_child("te15_b1_fix2_nonsetup_model_refusal_keeps_provider_words") {
        return;
    }
    let (f, _) = te15_fix_setup("codex");
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
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .native_readiness_checks = Some(Arc::new(Fix2SandboxUnavailable));
    let result = mcp.wire("assign_task", command);
    assert_eq!(
        result.is_error,
        Some(true),
        "TE15_FIX2_NONSETUP_MODEL_REFUSED"
    );
    let text = serde_json::to_string(&result).unwrap();
    assert!(
        text.contains(&CertificationRefusal::SandboxUnavailable.to_string())
            && !text.contains("fix Settings")
            && !text.contains("subscription sign-in"),
        "TE15_FIX2_NONSETUP_MODEL_WORDS {text}"
    );
    assert!(
        matches!(f.projection(&run).wait, RunWait::Provider(_)),
        "TE15_FIX2_NONSETUP_MODEL_WAIT"
    );
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
}
