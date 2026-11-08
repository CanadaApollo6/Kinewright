struct Fix3Checks(Mutex<Option<CertificationRefusal>>);
impl ConnectorRuntimeChecks for Fix3Checks {
    fn preflight(&self, _: &ConnectorSpecification) -> Result<(), CertificationRefusal> {
        self.0.lock().unwrap().take().map_or(Ok(()), Err)
    }
    fn probe(
        &self,
        _: &ConnectorSpecification,
        _: &crate::orchestration::adapters::AttemptBinding,
    ) -> Result<crate::orchestration::adapters::RuntimeQualification, CertificationRefusal> {
        panic!("FIX3_NO_INSTALLED_PROBE")
    }
}
struct Fix3Launcher {
    failure: Mutex<Option<std::io::Error>>,
    sent: Arc<AtomicUsize>,
}
impl crate::orchestration::adapters::GuardianLauncher for Fix3Launcher {
    fn launch(
        &self,
        _: crate::orchestration::AdmittedAttempt,
        _: crate::orchestration::adapters::AuthorizedInvocation,
    ) -> Result<
        crate::orchestration::adapters::ContainedSession,
        crate::orchestration::adapters::LaunchError,
    > {
        if let Some(error) = self.failure.lock().unwrap().take() {
            return Err(crate::orchestration::adapters::LaunchError(error));
        }
        self.sent.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(
            crate::orchestration::adapters::ContainedSession::from_guardian(Box::new(
                Te15NoProcess(crossbeam_channel::unbounded().1),
            )),
        )
    }
}
struct Fix3Profiles {
    inner: Te15Profiles,
    failure: Mutex<Option<std::io::Error>>,
}
impl crate::effect_pump::ProfileFactory for Fix3Profiles {
    fn bind(
        &self,
        attempt: &crate::orchestration::AdmittedAttempt,
    ) -> std::io::Result<crate::orchestration::adapters::CertifiedConnectorProfile> {
        if let Some(error) = self.failure.lock().unwrap().take() {
            return Err(error);
        }
        crate::effect_pump::ProfileFactory::bind(&self.inner, attempt)
    }
}
fn fix3_pump(
    f: &Fixture,
    run: &RunId,
    failure: Option<std::io::Error>,
) -> (
    crate::effect_pump::EffectPump<Fix3Launcher>,
    Arc<AtomicUsize>,
) {
    fix3_pump_with_binding(f, run, failure, None)
}
fn fix3_pump_with_binding(
    f: &Fixture,
    run: &RunId,
    failure: Option<std::io::Error>,
    binding_failure: Option<std::io::Error>,
) -> (
    crate::effect_pump::EffectPump<Fix3Launcher>,
    Arc<AtomicUsize>,
) {
    let owner = f.host.owner.upgrade().unwrap();
    let project = owner.lock().unwrap().spending_project().unwrap();
    let claim = Arc::new(Mutex::new(
        crate::execution_claim::MachineClaim::acquire_fixture(
            &f.temp.path("fix3-claim"),
            project.as_str(),
            run.0.as_str(),
        )
        .unwrap(),
    ));
    let sent = Arc::new(AtomicUsize::new(0));
    (
        crate::effect_pump::EffectPump::with_launcher(
            &owner,
            f.producer(run),
            run.clone(),
            claim,
            Fix3Launcher {
                failure: Mutex::new(failure),
                sent: sent.clone(),
            },
            Arc::new(Fix3Profiles {
                failure: Mutex::new(binding_failure),
                inner: Te15Profiles {
                    runtime: f.host.runtime.clone(),
                    host: Arc::downgrade(&owner),
                    servers: Mutex::default(),
                    scratch: Mutex::default(),
                },
            }),
            std::num::NonZeroU64::new(8000),
        ),
        sent,
    )
}
fn fix3_continue(f: &Fixture, run: &RunId) -> ScopedOrchestrationRequest {
    let request = f.control(&f.editor, run, RunAction::Continue);
    ok(
        f.editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(request.clone()),
            })
            .unwrap(),
        "FIX3_CONTINUE",
    );
    request
}
fn fix3_precharge(refusal: CertificationRefusal) {
    let (f, _) = te15_fix_setup("codex");
    let run: RunId = f.start().envelope.identity("run").unwrap();
    let original = f.projection(&run).effects[0].id.clone();
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .native_readiness_checks = Some(Arc::new(Fix3Checks(Mutex::new(Some(refusal)))));
    let (mut pump, sent) = fix3_pump(&f, &run, None);
    assert!(pump.tick().is_ok(), "FIX3_PRECHARGE_RESUMABLE");
    let state = f.projection(&run);
    assert!(
        crate::orchestration::machine::eligible(&state, &RunAction::Continue),
        "FIX3_PRECHARGE_RESUMABLE"
    );
    assert!(state.attempts.is_empty(), "FIX3_NO_PRECHARGE_ATTEMPT");
    fix3_continue(&f, &run);
    assert_eq!(f.projection(&run).effects.len(), 1, "FIX3_PENDING_REUSE");
    assert_eq!(f.projection(&run).effects[0].id, original);
    pump.tick().unwrap();
    pump.tick().unwrap();
    assert_eq!(
        sent.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "FIX3_EXACTLY_ONE_START"
    );
}
#[test]
fn te15_b1_fix3_probe_pending_resumes() {
    if te15_fix_counting_child("te15_b1_fix3_probe_pending_resumes") {
        fix3_precharge(CertificationRefusal::VersionProbePending);
    }
}
#[test]
fn te15_b1_fix3_sandbox_resumes() {
    if te15_fix_counting_child("te15_b1_fix3_sandbox_resumes") {
        fix3_precharge(CertificationRefusal::SandboxUnavailable);
    }
}
#[test]
fn te15_b1_fix3_never_sent_reuses_original_effect() {
    if !te15_fix_counting_child("te15_b1_fix3_never_sent_reuses_original_effect") {
        return;
    }
    let (f, _) = te15_fix_setup("codex");
    let run: RunId = f.start().envelope.identity("run").unwrap();
    let (mut pump, sent) = fix3_pump(
        &f,
        &run,
        Some(super::super::provider_readiness::certification_error(
            "codex",
            CertificationRefusal::UnknownAuthentication,
        )),
    );
    pump.tick().expect("FIX3_NEVER_SENT_PENDING");
    let before = f.projection(&run);
    assert_eq!(
        before.effects[0].state,
        EffectState::Pending,
        "FIX3_NEVER_SENT_PENDING"
    );
    assert_eq!(sent.load(std::sync::atomic::Ordering::SeqCst), 0);
    let owner = f.host.owner.upgrade().unwrap();
    let accounting = owner
        .lock()
        .unwrap()
        .native_accounting(&before)
        .unwrap()
        .unwrap();
    assert!(
        accounting.tasks[0]
            .usage
            .iter()
            .any(|sample| sample.event == "host.not_dispatched"
                && sample.complete
                && sample.values
                    == kinewright_project::orchestration_accounting::UsageValues::from_values(
                        [Some(0); 6]
                    )),
        "FIX3_HOST_ZERO_PROOF"
    );
    assert_eq!(
        accounting.tasks[0].sends.len(),
        1,
        "FIX3_FAILED_SLOT_RETAINED"
    );
    let failed_charge = accounting.tasks[0].sends[0].clone();
    drop(accounting);
    let request = fix3_continue(&f, &run);
    let state = f.projection(&run);
    assert_eq!(state.effects.len(), 1, "FIX3_ORIGINAL_EFFECT_REUSED");
    let effect = &state.effects[0];
    let owner = f.host.owner.upgrade().unwrap();
    let bytes = owner
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .store()
        .unwrap()
        .read_evidence(&effect.payload)
        .unwrap();
    assert_eq!(
        bytes.as_slice(),
        b"hook plan, script and assignment",
        "FIX3_ORIGINAL_PAYLOAD"
    );
    ok(
        f.editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(request),
            })
            .unwrap(),
        "FIX3_REPLAY",
    );
    pump.tick().unwrap();
    pump.tick().unwrap();
    assert_eq!(
        sent.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "FIX3_EXACTLY_ONE_CONTINUATION"
    );
    let state = f.projection(&run);
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
            .map(|task| task.sends.len())
            .sum::<usize>(),
        2,
        "FIX3_ORIGINAL_CHARGE_KEPT"
    );
    assert_eq!(
        accounting.tasks[0].sends[0], failed_charge,
        "FIX3_FAILED_SLOT_RETAINED"
    );
}
#[test]
fn te15_b1_fix3_claimed_and_unknown_have_distinct_words() {
    if !te15_fix_counting_child("te15_b1_fix3_claimed_and_unknown_have_distinct_words") {
        return;
    }
    let (f, _) = te15_fix_setup("codex");
    let run: RunId = f.start().envelope.identity("run").unwrap();
    let (mut pump, _) = fix3_pump(&f, &run, None);
    pump.tick().unwrap();
    let producer = f.producer(&run);
    let owner = f.host.owner.upgrade().unwrap();
    owner
        .lock()
        .unwrap()
        .wait_for_provider(&run, "Fix Settings.")
        .unwrap();
    for unknown in [false, true] {
        if unknown {
            let mut state = f.projection(&run);
            let identity = state.attempts[0].identity.clone();
            state.effects[0].state = EffectState::OutcomeUnknown;
            owner
                .lock()
                .unwrap()
                .publish_native(&producer, state, &identity, vec![])
                .unwrap();
        }
        let reply = f
            .editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(f.control(&f.editor, &run, RunAction::Continue)),
            })
            .unwrap();
        let text = reply.error.expect("FIX3_TRUE_CONTINUE_WORDS").message;
        assert!(
            text.contains(if unknown {
                "unknown send outcome"
            } else {
                "A task is still working. Continue when its turn ends."
            }),
            "FIX3_TRUE_CONTINUE_WORDS {text}"
        );
    }
}
#[test]
fn te15_b1_fix3_child_finishes_during_wait() {
    let r = c5_fix10_run();
    let owner = r.f.host.owner.upgrade().unwrap();
    owner
        .lock()
        .unwrap()
        .wait_for_provider(&r.run, "Fix Settings.")
        .unwrap();
    let status = r.editor.wire("task_status", serde_json::json!({}));
    assert_ne!(
        status.is_error,
        Some(true),
        "FIX3_CHILD_AUTHORITY {status:?}"
    );
    let controls = status.structured_content.unwrap()["controls"].clone();
    let mut control = controls;
    control["command"] = serde_json::json!({"kind":"finish_task","value":{"task":r.child,"expected_revision":grant_test_revision(&r.core)}});
    let result = r.editor.wire("finish_task", control);
    assert_eq!(
        result.is_error,
        Some(false),
        "FIX3_CHILD_AUTHORITY {result:?}"
    );
    assert_eq!(
        r.f.projection(&r.run).deliveries.len(),
        1,
        "FIX3_CHILD_RESULT_QUEUED"
    );
}
fn fix3_historical_continue(restored: bool) {
    let (f, checks) = te15_fix_setup("codex");
    let run: RunId = f.start().envelope.identity("run").unwrap();
    let owner = f.host.owner.upgrade().unwrap();
    owner
        .lock()
        .unwrap()
        .wait_for_provider(&run, "Fix Settings.")
        .unwrap();
    let a = fix3_continue(&f, &run);
    owner
        .lock()
        .unwrap()
        .wait_for_provider(&run, "Fix Settings.")
        .unwrap();
    let b = fix3_continue(&f, &run);
    if restored {
        let state = f.projection(&run);
        let mut host = owner.lock().unwrap();
        host.hooks.runs.remove(&run);
        host.retain_run(&run, &state.tasks[0].brief).unwrap();
    }
    let runtime = super::super::studio_runtime::StudioRuntime::new(
        &owner,
        super::super::studio_runtime::RuntimePorts {
            checks,
            ceiling: None,
            fixture_claim: Some(f.temp.path("fixed-progress")),
            ..Default::default()
        },
    );
    runtime.failed_worker_fixture(&run);
    assert_eq!(
        runtime.drive().unwrap_err().to_string(),
        "fixture worker failed"
    );
    for request in [a.clone(), b, a] {
        ok(
            f.editor
                .request(HostCommand::DispatchOrchestration {
                    request: Box::new(request),
                })
                .unwrap(),
            "FIX3_HISTORICAL_REPLAY",
        );
        assert_eq!(
            runtime.drive().unwrap_err().to_string(),
            "fixture worker failed",
            "FIX3_CONTINUE_MONOTONIC"
        );
    }
    runtime.stop();
}
#[test]
fn te15_b1_fix3_old_continue_cannot_unlock_worker() {
    fix3_historical_continue(false);
}
#[test]
fn te15_b1_fix3_restored_progress_ignores_old_continue() {
    fix3_historical_continue(true);
}

#[test]
fn te15_b1_fix3_postcharge_ambiguity_is_not_resumable() {
    if !te15_fix_counting_child("te15_b1_fix3_postcharge_ambiguity_is_not_resumable") {
        return;
    }
    let (f, _) = te15_fix_setup("codex");
    let run: RunId = f.start().envelope.identity("run").unwrap();
    let (mut pump, _) = fix3_pump(&f, &run, None);
    pump.tick().unwrap();
    let mut unknown = f.projection(&run);
    let identity = unknown.attempts[0].identity.clone();
    unknown.effects[0].state = EffectState::OutcomeUnknown;
    let producer = f.producer(&run);
    let owner = f.host.owner.upgrade().unwrap();
    owner
        .lock()
        .unwrap()
        .publish_native(&producer, unknown, &identity, vec![])
        .unwrap();
    super::super::provider_readiness::wait_on_launch_error(
        &owner,
        &run,
        &super::super::provider_readiness::certification_error(
            "codex",
            CertificationRefusal::UnknownAuthentication,
        ),
        false,
    )
    .unwrap();
    owner
        .lock()
        .unwrap()
        .record_runtime_refusal(&producer, &run, "raw final runtime failure")
        .unwrap();
    let state = f.projection(&run);
    let RunWait::Provider(detail) = &state.wait else {
        panic!("FIX3_POSTCHARGE_PROVIDER")
    };
    let reason = owner
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .store()
        .unwrap()
        .read_evidence(&detail.reason)
        .unwrap();
    assert!(std::str::from_utf8(&reason).unwrap().ends_with(" The send outcome is unknown. Press Stop, then Reopen to review your notes before starting another run. Continue cannot retry this send."), "FIX4_FINAL_UNKNOWN_WORDS {}", String::from_utf8_lossy(&reason));
    let page = f
        .editor
        .request(HostCommand::ObserveRun {
            run: run.clone(),
            after_sequence: 0,
        })
        .unwrap()
        .run_view
        .unwrap();
    assert_eq!(
        page.wait_reason.as_deref(),
        std::str::from_utf8(&reason).ok(),
        "FIX4_HOST_WAIT_REASON"
    );
    assert!(
        !page.allowed_actions.contains(&RunActionKind::Continue),
        "FIX4_HOST_UNKNOWN_DISABLED"
    );
    assert!(
        !crate::orchestration::machine::eligible(&f.projection(&run), &RunAction::Continue),
        "FIX3_POSTCHARGE_PROVIDER"
    );
}
#[test]
fn te15_b1_fix3_fast_model_refusal_ends_turn() {
    if !te15_fix_counting_child("te15_b1_fix3_fast_model_refusal_ends_turn") {
        return;
    }
    let (f, _) = te15_fix_setup("codex");
    let run: RunId = f.start().envelope.identity("run").unwrap();
    let (mut pump, sent) = fix3_pump(&f, &run, None);
    pump.tick().unwrap();
    let state = f.projection(&run);
    let mcp = C4Mcp::new(&f, &run, &state.tasks[0].id);
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .native_readiness_checks = Some(Arc::new(Fix3Checks(Mutex::new(Some(
        CertificationRefusal::UnknownAuthentication,
    )))));
    let mut brief = f.brief.clone();
    brief.role = Role::Editor;
    let reply = mcp.wire(
        "assign_task",
        mcp.control(OrchestrationCommand::AssignTask(Box::new(AssignTask {
            run: run.clone(),
            parent: state.tasks[0].id.clone(),
            brief,
        }))),
    );
    let text = serde_json::to_string(&reply).unwrap();
    assert_eq!(reply.is_error, Some(true));
    assert!(text.contains("The run is waiting for the person to fix Settings. End your turn now. Do not retry this call."), "FIX3_END_TURN {text}");
    assert_eq!(sent.load(std::sync::atomic::Ordering::SeqCst), 1);
}
#[test]
fn te15_b1_fix3_pending_child_owns_the_only_next_send() {
    if !te15_fix_counting_child("te15_b1_fix3_pending_child_owns_the_only_next_send") {
        return;
    }
    let (f, _) = te15_fix_setup("codex");
    let run: RunId = f.start().envelope.identity("run").unwrap();
    let (mut pump, sent) = fix3_pump(&f, &run, None);
    pump.tick().unwrap();
    let owner = f.host.owner.upgrade().unwrap();
    let base = {
        let mut host = owner.lock().unwrap();
        let owner = host.owner_mut().unwrap();
        owner.save_active().unwrap();
        let mut branch = owner.fork_durable_branch("fix3-child").unwrap();
        owner.persist_durable_branch(&mut branch).unwrap();
        owner.candidate_binding(branch.branch_id()).unwrap()
    };
    let state = f.projection(&run);
    let mut brief = f.brief.clone();
    brief.role = Role::Editor;
    brief.scope.base = base;
    let producer_client = f.producer_client(&run);
    let request = f.request(
        &producer_client,
        Some(run.clone()),
        OrchestrationCommand::AssignTask(Box::new(AssignTask {
            run: run.clone(),
            parent: state.tasks[0].id.clone(),
            brief,
        })),
    );
    let segment = owner
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .orchestration_cut(&run)
        .unwrap()
        .unwrap()
        .segment;
    let update = crate::orchestration::machine::prepare(
        Some(&state),
        &request.envelope.project,
        request.envelope.expected_generation,
        &request.envelope,
        Some(segment),
    )
    .unwrap();
    // Public model assignment remains narrowed. H1 also retains branch-scoped tasks.
    owner
        .lock()
        .unwrap()
        .publish_orchestration_transition(&producer_client.bootstrap.auth, &update, None)
        .unwrap();
    let state = f.projection(&run);
    let child = state.tasks[1].id.clone();
    b20_prepare_legacy_lane(&f, &run, &child);
    let state = f.projection(&run);
    let original = state.effects.last().unwrap().id.clone();
    let producer = f.producer(&run);
    let identity = state.attempts[0].identity.clone();
    owner
        .lock()
        .unwrap()
        .reconcile_native_effect(
            &producer,
            &identity,
            &crate::guardian::AttemptRetirementProof::NotDispatched(
                crate::native_launcher::native_identity(&identity),
            ),
        )
        .unwrap();
    let mut delivered = f.projection(&run);
    delivered.effects[0].state = EffectState::Settled;
    owner
        .lock()
        .unwrap()
        .publish_native(&producer, delivered, &identity, vec![])
        .unwrap();
    owner
        .lock()
        .unwrap()
        .wait_for_provider(&run, "Fix Settings.")
        .unwrap();
    let before = f.projection(&run);
    assert_ne!(
        before.tasks[0].brief.scope.base.branch,
        before.tasks[1].brief.scope.base.branch
    );
    fix3_continue(&f, &run);
    let after = f.projection(&run);
    assert_eq!(
        after.effects.len(),
        before.effects.len(),
        "FIX3_NO_EXTRA_DIRECTOR"
    );
    assert!(
        after
            .effects
            .iter()
            .any(|e| e.id == original && e.state == EffectState::Pending)
    );
    pump.tick().unwrap();
    pump.tick().unwrap();
    assert_eq!(
        sent.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "FIX3_ONE_CHILD_SEND"
    );
    assert!(
        f.projection(&run)
            .effects
            .iter()
            .any(|e| e.task == child && matches!(e.state, EffectState::Claimed(_))),
        "FIX3_CHILD_CLAIMED"
    );
}
#[test]
fn te15_b1_fix3_binding_failure_resumes_without_charge() {
    if !te15_fix_counting_child("te15_b1_fix3_binding_failure_resumes_without_charge") {
        return;
    }
    let (f, _) = te15_fix_setup("codex");
    let run: RunId = f.start().envelope.identity("run").unwrap();
    let (mut pump, sent) = fix3_pump_with_binding(
        &f,
        &run,
        None,
        Some(std::io::Error::other("saved connector policy changed")),
    );
    assert!(pump.tick().is_ok(), "FIX3_BIND_RETRY");
    let before = f.projection(&run);
    assert!(
        crate::orchestration::machine::eligible(&before, &RunAction::Continue),
        "FIX3_BIND_RETRY"
    );
    let owner = f.host.owner.upgrade().unwrap();
    assert!(
        owner
            .lock()
            .unwrap()
            .saved_task_history(&before, &before.tasks[0].id)
            .unwrap()
            .is_some(),
        "FIX3_BIND_HISTORY"
    );
    assert_eq!(
        owner
            .lock()
            .unwrap()
            .native_accounting(&before)
            .unwrap()
            .unwrap()
            .tasks
            .iter()
            .map(|t| t.sends.len())
            .sum::<usize>(),
        0,
        "FIX3_BIND_NO_CHARGE"
    );
    fix3_continue(&f, &run);
    pump.tick().unwrap();
    pump.tick().unwrap();
    assert_eq!(
        sent.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "FIX3_BIND_RETRY_ONE_SEND"
    );
}
#[test]
fn te15_b1_fix3_person_gets_neutral_retry_words() {
    let (f, _) = te15_fix_setup("codex");
    let run: RunId = f.start().envelope.identity("run").unwrap();
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .native_readiness_checks = Some(Arc::new(Fix3Checks(Mutex::new(Some(
        CertificationRefusal::VersionProbePending,
    )))));
    let request = f.control(
        &f.editor,
        &run,
        RunAction::SendMessage {
            text_ref: f.brief.assignment.clone(),
        },
    );
    let reply = f
        .editor
        .request(HostCommand::DispatchOrchestration {
            request: Box::new(request),
        })
        .unwrap();
    assert_eq!(
        reply.error.unwrap().message,
        "The provider check failed before anything was sent. Continue to try again.",
        "FIX3_RETRY_WORDS"
    );
    assert!(
        crate::orchestration::machine::eligible(&f.projection(&run), &RunAction::Continue),
        "FIX3_PERSON_RETRY"
    );
}
#[test]
fn te15_b1_fix3_not_sent_proof_refuses_observed_usage() {
    use kinewright_project::orchestration_accounting::{UsageMode, UsageSample, UsageValues};
    if !te15_fix_counting_child("te15_b1_fix3_not_sent_proof_refuses_observed_usage") {
        return;
    }
    let (f, _) = te15_fix_setup("codex");
    let run: RunId = f.start().envelope.identity("run").unwrap();
    let (mut pump, _) = fix3_pump(&f, &run, None);
    pump.tick().unwrap();
    let state = f.projection(&run);
    let identity = state.attempts[0].identity.clone();
    let producer = f.producer(&run);
    let owner = f.host.owner.upgrade().unwrap();
    let mut host = owner.lock().unwrap();
    let accounting = host.native_accounting(&state).unwrap().unwrap();
    let now = accounting.last_unix_ms;
    let binding = accounting.tasks[0].streams[0].clone();
    drop(accounting);
    host.change_spending(
        &producer,
        &identity,
        super::super::native_invocation::SpendingChange::Usage(UsageSample {
            snapshot: Some(kinewright_project::usage_stream::Snapshot {
                payload_sha256: "0".repeat(64),
                stream: binding.stream,
                adapter: binding.adapter,
                models: vec![],
            }),
            attempt: identity.attempt.0.clone(),
            turn: 1,
            event: "native.eof:0".into(),
            ordinal: 1,
            mode: UsageMode::Cumulative,
            values: UsageValues::default(),
            complete: false,
            input_includes_cache: true,
            output_includes_reasoning: true,
        }),
        crate::protocol::limits::usage::ClockObservation {
            unix_ms: now,
            monotonic_delta_ms: 0,
            trusted: true,
        },
    )
    .unwrap();
    let before = host.spending_scope(&producer, &run).unwrap();
    let result = host.reconcile_native_effect(
        &producer,
        &identity,
        &crate::guardian::AttemptRetirementProof::NotDispatched(
            crate::native_launcher::native_identity(&identity),
        ),
    );
    let after = host.spending_scope(&producer, &run).unwrap();
    drop(host);
    assert!(result.is_err(), "FIX3_PROOF_CONFLICT");
    assert_eq!(after, before, "FIX3_PROOF_CONFLICT_PRESERVED");
}
