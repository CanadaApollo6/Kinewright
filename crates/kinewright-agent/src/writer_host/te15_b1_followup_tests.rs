#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one racing assignment retains the unknown wait through the Director's turn end"
)]
fn te15_b1_followup_unknown_wait_survives_assign_and_stream() {
    if !te15_fix_counting_child("te15_b1_followup_unknown_wait_survives_assign_and_stream") {
        return;
    }
    let (f, _) = te15_fix_setup("codex");
    let (run, director, mcp) = c4_director(&f);
    fix4_assignment(&f, &run, true);
    let (mut pump, sent) = fix4_pump(&f, &run, false);
    pump.tick().unwrap();
    let owner = f.host.owner.upgrade().unwrap();
    let producer = f.producer(&run);
    let mut state = f.projection(&run);
    let child = state.tasks[1].id.clone();
    assert_eq!(sent.lock().unwrap().len(), 1, "FOLLOWUP_UNKNOWN_HAS_A_SEND");
    assert!(
        state
            .effects
            .iter()
            .any(|e| e.task == child && matches!(e.state, EffectState::Claimed(_))),
        "FOLLOWUP_UNKNOWN_HAS_A_CLAIM"
    );
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
    let mut brief = f.brief.clone();
    brief.role = Role::Editor;
    let command = mcp.control(OrchestrationCommand::AssignTask(Box::new(AssignTask {
        run: run.clone(),
        parent: director.task.clone(),
        brief,
    })));
    let gate = Arc::new((std::sync::Barrier::new(2), std::sync::Barrier::new(2)));
    let checks = Arc::new(FollowupGatedChecks {
        gate: gate.clone(),
        calls: AtomicUsize::new(0),
    });
    owner.lock().unwrap().native_readiness_checks = Some(checks.clone());
    let original = std::thread::scope(|scope| {
        let assignment = scope.spawn(|| mcp.wire("assign_task", command));
        gate.0.wait();
        owner
            .lock()
            .unwrap()
            .publish_native(&producer, state, &identity, vec![])
            .unwrap();
        super::super::provider_readiness::wait_on_launch_error(
            &owner,
            &run,
            &std::io::Error::other("uncertain child send"),
            false,
        )
        .unwrap();
        let original = f.projection(&run).wait;
        gate.1.wait();
        assert_eq!(assignment.join().unwrap().is_error, Some(true));
        original
    });
    assert_eq!(
        checks.calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "FOLLOWUP_ASSIGN_CHECK_RAN"
    );
    assert_eq!(
        f.projection(&run).wait,
        original,
        "FOLLOWUP_UNKNOWN_READINESS_WAIT"
    );
    owner
        .lock()
        .unwrap()
        .record_stream_refusal(&producer, &director, "later stream refusal")
        .unwrap();
    assert_eq!(
        f.projection(&run).wait,
        original,
        "FOLLOWUP_UNKNOWN_STREAM_WAIT"
    );
    let mut ended = f.projection(&run);
    for effect in ended.effects.iter_mut().filter(|e| e.task == director.task) {
        effect.state = EffectState::Settled;
    }
    for attempt in ended
        .attempts
        .iter_mut()
        .filter(|a| a.identity.task == director.task)
    {
        attempt.state = AttemptState::Retired(AttemptOutcome::Completed);
    }
    let parent = ended
        .tasks
        .iter_mut()
        .find(|t| t.id == director.task)
        .unwrap();
    parent.progress.capacity = DependencyReservation::Free;
    parent.progress.turn = None;
    parent.progress.turn_ended = true;
    parent.state = TaskState::Queued;
    crate::orchestration::tasks::refresh_cleanup(&mut ended);
    owner
        .lock()
        .unwrap()
        .publish_native(&producer, ended, &director, vec![])
        .unwrap();
    let page = f
        .editor
        .request(HostCommand::ObserveRun {
            run: run.clone(),
            after_sequence: 0,
        })
        .unwrap()
        .run_view
        .unwrap();
    assert!(
        !page.allowed_actions.contains(&RunActionKind::Continue),
        "FOLLOWUP_UNKNOWN_DISABLED"
    );
    assert_eq!(
        page.continue_unavailable_reason.as_deref(),
        Some("A task has an unknown send outcome. Continue cannot retry this send."),
        "FOLLOWUP_HOST_DISABLED_REASON"
    );
    let before = sent.lock().unwrap().len();
    let reply = f
        .editor
        .request(HostCommand::DispatchOrchestration {
            request: Box::new(f.control(&f.editor, &run, RunAction::Continue)),
        })
        .unwrap();
    assert!(
        reply.error.is_some()
            || reply
                .orchestration
                .is_some_and(|r| matches!(r.decision, CommandDecision::Refused(_)))
    );
    pump.tick().unwrap();
    assert_eq!(
        sent.lock().unwrap().len(),
        before,
        "FOLLOWUP_UNKNOWN_NO_GENERIC"
    );
    let mut unstored = f.projection(&run);
    unstored.wait = RunWait::None;
    unstored.phase = RunPhase::Running;
    owner
        .lock()
        .unwrap()
        .publish_native(&producer, unstored, &director, vec![])
        .unwrap();
    owner
        .lock()
        .unwrap()
        .record_runtime_refusal(&producer, &run, "unknown wait publication failed")
        .unwrap();
    let interrupted = f.projection(&run);
    assert!(
        interrupted.phase == RunPhase::Interrupted
            && matches!(interrupted.wait, RunWait::Provider(_)),
        "FOLLOWUP_UNPUBLISHED_UNKNOWN_INTERRUPTS"
    );
}

struct FollowupGatedChecks {
    gate: Arc<(std::sync::Barrier, std::sync::Barrier)>,
    calls: AtomicUsize,
}
impl ConnectorRuntimeChecks for FollowupGatedChecks {
    fn preflight(&self, _: &ConnectorSpecification) -> Result<(), CertificationRefusal> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.gate.0.wait();
        self.gate.1.wait();
        Err(CertificationRefusal::VersionProbePending)
    }
    fn probe(
        &self,
        _: &ConnectorSpecification,
        _: &crate::orchestration::adapters::AttemptBinding,
    ) -> Result<crate::orchestration::adapters::RuntimeQualification, CertificationRefusal> {
        panic!("FOLLOWUP_NO_INSTALLED_PROBE")
    }
}
struct FollowupWaitIoFailure {
    objects: std::path::PathBuf,
    calls: Arc<AtomicUsize>,
}
impl ConnectorRuntimeChecks for FollowupWaitIoFailure {
    fn preflight(&self, _: &ConnectorSpecification) -> Result<(), CertificationRefusal> {
        use std::os::unix::fs::PermissionsExt;
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        std::fs::set_permissions(&self.objects, std::fs::Permissions::from_mode(0o500)).unwrap();
        Err(CertificationRefusal::VersionProbePending)
    }
    fn probe(
        &self,
        _: &ConnectorSpecification,
        _: &crate::orchestration::adapters::AttemptBinding,
    ) -> Result<crate::orchestration::adapters::RuntimeQualification, CertificationRefusal> {
        panic!("FOLLOWUP_NO_INSTALLED_PROBE")
    }
}
#[test]
fn te15_b1_followup_wait_write_recovers_original_effect() {
    followup_wait_write(false);
}
#[test]
fn te15_b1_followup_wait_write_recovers_after_final_handler() {
    followup_wait_write(true);
}
#[allow(
    clippy::too_many_lines,
    reason = "one outage sequence verifies failed publication, bounded recovery and original dispatch"
)]
fn followup_wait_write(late: bool) {
    let name = if late {
        "te15_b1_followup_wait_write_recovers_after_final_handler"
    } else {
        "te15_b1_followup_wait_write_recovers_original_effect"
    };
    if !te15_fix_counting_child(name) {
        return;
    }
    let (f, checks) = te15_fix_setup("codex");
    let run: RunId = f.start().envelope.identity("run").unwrap();
    let original = f.projection(&run).effects[0].clone();
    let owner = f.host.owner.upgrade().unwrap();
    let producer = f.producer(&run);
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
    let calls = Arc::new(AtomicUsize::new(0));
    owner.lock().unwrap().native_readiness_checks = Some(Arc::new(FollowupWaitIoFailure {
        objects: objects.clone(),
        calls: calls.clone(),
    }));
    let (mut pump, sent) = fix4_pump(&f, &run, false);
    let result = pump.tick();
    let final_failure = late.then(|| {
        owner.lock().unwrap().record_runtime_refusal(
            &producer,
            &run,
            "native runtime refusal: wait write failed",
        )
    });
    std::fs::set_permissions(&objects, permissions).unwrap();
    if let Some(result) = final_failure {
        assert!(result.is_err());
    }
    let error = result.expect_err("FOLLOWUP_WAIT_WRITE_FAILED");
    assert!(matches!(f.projection(&run).wait, RunWait::None));
    let runtime = super::super::studio_runtime::StudioRuntime::new(
        &owner,
        super::super::studio_runtime::RuntimePorts {
            checks: checks.clone(),
            ceiling: None,
            fixture_claim: Some(f.temp.path("followup-blocked")),
            ..Default::default()
        },
    );
    runtime.failed_worker_fixture(&run);
    if !late {
        owner
            .lock()
            .unwrap()
            .record_runtime_refusal(&producer, &run, &error.to_string())
            .unwrap();
        assert!(
            matches!(f.projection(&run).wait, RunWait::ProviderSettings(_)),
            "FOLLOWUP_RESUMABLE_CLASSIFICATION"
        );
    }
    for _ in 0..3 {
        assert!(runtime.drive().is_err());
    }
    let page = f
        .editor
        .request(HostCommand::ObserveRun {
            run: run.clone(),
            after_sequence: 0,
        })
        .unwrap()
        .run_view
        .unwrap();
    runtime.stop();
    assert!(
        matches!(page.projection.wait, RunWait::ProviderSettings(_)),
        "FOLLOWUP_RESUMABLE_CLASSIFICATION"
    );
    assert_eq!(
        page.wait_reason.as_deref(),
        Some(super::super::provider_readiness::CHECK_RETRY),
        "FOLLOWUP_ORIGINAL_WAIT_REASON"
    );
    assert!(
        page.allowed_actions.contains(&RunActionKind::Continue),
        "FOLLOWUP_RECOVERED_CONTINUE"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "FOLLOWUP_NO_REPROBE"
    );
    assert!(sent.lock().unwrap().is_empty());
    assert_eq!(f.projection(&run).effects[0], original);
    owner.lock().unwrap().native_readiness_checks = Some(checks);
    fix3_continue(&f, &run);
    pump.tick().unwrap();
    pump.tick().unwrap();
    pump.tick().unwrap();
    let sends = sent.lock().unwrap();
    assert_eq!(sends.len(), 1, "FOLLOWUP_ORIGINAL_EFFECT_ONCE");
    assert_eq!(sends[0].task, original.task);
    assert_eq!(
        sends[0].kind,
        crate::orchestration::adapters::SendKind::OpeningAssignment
    );
    let payload = owner
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .store()
        .unwrap()
        .read_evidence(&original.payload)
        .unwrap();
    let text = String::from_utf8_lossy(&sends[0].bytes);
    assert!(
        text.contains(&original.payload.sha256)
            || text.contains(std::str::from_utf8(&payload).unwrap()),
        "FOLLOWUP_ORIGINAL_PAYLOAD"
    );
    assert!(
        f.projection(&run)
            .effects
            .iter()
            .any(|e| e.id == original.id && matches!(e.state, EffectState::Claimed(_)))
    );
}
