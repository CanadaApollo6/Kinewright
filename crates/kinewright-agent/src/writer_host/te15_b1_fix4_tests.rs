#[derive(Clone)]
struct Fix4Sent {
    task: TaskId,
    kind: crate::orchestration::adapters::SendKind,
    bytes: Vec<u8>,
}
struct Fix4Launcher {
    refuse: bool,
    sent: Arc<Mutex<Vec<Fix4Sent>>>,
}
impl crate::orchestration::adapters::GuardianLauncher for Fix4Launcher {
    fn launch(
        &self,
        attempt: crate::orchestration::AdmittedAttempt,
        invocation: crate::orchestration::adapters::AuthorizedInvocation,
    ) -> Result<
        crate::orchestration::adapters::ContainedSession,
        crate::orchestration::adapters::LaunchError,
    > {
        if self.refuse {
            return Err(crate::orchestration::adapters::LaunchError(
                super::super::provider_readiness::certification_error(
                    "codex",
                    CertificationRefusal::VersionProbePending,
                ),
            ));
        }
        self.sent.lock().unwrap().push(Fix4Sent {
            task: attempt.identity().task.clone(),
            kind: invocation.invocation().kind(),
            bytes: invocation.invocation().bytes().to_vec(),
        });
        Ok(
            crate::orchestration::adapters::ContainedSession::from_guardian(Box::new(
                Te15NoProcess(crossbeam_channel::unbounded().1),
            )),
        )
    }
}
fn fix4_pump(
    f: &Fixture,
    run: &RunId,
    refuse: bool,
) -> (
    crate::effect_pump::EffectPump<Fix4Launcher>,
    Arc<Mutex<Vec<Fix4Sent>>>,
) {
    let owner = f.host.owner.upgrade().unwrap();
    let project = owner.lock().unwrap().spending_project().unwrap();
    let claim = Arc::new(Mutex::new(
        crate::execution_claim::MachineClaim::acquire_fixture(
            &f.temp.path(if refuse {
                "fix4-refused-claim"
            } else {
                "fix4-resumed-claim"
            }),
            project.as_str(),
            run.0.as_str(),
        )
        .unwrap(),
    ));
    let sent = Arc::new(Mutex::new(vec![]));
    (
        crate::effect_pump::EffectPump::with_launcher(
            &owner,
            f.producer(run),
            run.clone(),
            claim,
            Fix4Launcher {
                refuse,
                sent: sent.clone(),
            },
            Arc::new(Te15Profiles {
                runtime: f.host.runtime.clone(),
                host: Arc::downgrade(&owner),
                servers: Mutex::default(),
                scratch: Mutex::default(),
            }),
            std::num::NonZeroU64::new(8000),
        ),
        sent,
    )
}
// A previously delivered fixture turn has no live process and no charge.
fn fix4_prior_work(f: &Fixture, run: &RunId) {
    let mut state = f.projection(run);
    for attempt in &mut state.attempts {
        attempt.state = AttemptState::Retired(AttemptOutcome::Interrupted);
    }
    for effect in &mut state.effects {
        if !state
            .deliveries
            .iter()
            .any(|delivery| delivery.wake == effect.id)
        {
            effect.state = EffectState::Settled;
        }
    }
    for task in &mut state.tasks {
        task.progress.capacity = DependencyReservation::Free;
        task.progress.turn = None;
        task.progress.turn_ended = true;
        if !matches!(task.state, TaskState::Terminal(_)) {
            task.state = TaskState::Queued;
        }
    }
    crate::orchestration::tasks::refresh_cleanup(&mut state);
    publish_projection(f, &f.producer_client(run), run, state, None);
}
fn fix4_reopen(f: Fixture, run: &RunId) -> Fixture {
    let Fixture {
        host, brief, temp, ..
    } = f;
    drop(host);
    let roots = SessionRoots {
        read: vec![temp.root().into()],
        write: vec![temp.root().into()],
    };
    let issuer_auth = HostAuth::generate().unwrap();
    let host = WriterHost::start_inner(
        HostConfig {
                data_dir: Some((temp.path("scheduler")).with_extension("voice-data")),
            scheduler_settings: Some(temp.path("scheduler")),
            project: "fix4 reopened".into(),
            path: Some(temp.path("project.kinewright")),
            roots: roots.clone(),
            origin: HostOrigin::Headless,
        },
        HostAuth::generate().unwrap(),
        Some(issuer_auth.clone()),
        super::super::studio_runtime::RuntimePorts {
            checks: Arc::new(Te15Readiness::default()),
            fixture_claim: Some(temp.path("fixture-machine")),
            ceiling: None,
            ..Default::default()
        },
    )
    .unwrap();
    wait_runtime_recovery(&host);
    host.runtime.stop();
    host.owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .restore_native_run(run)
        .unwrap();
    let editor = host.client();
    let issuer = HostIssuer::new(host.endpoint.clone(), issuer_auth.clone(), roots).unwrap();
    Fixture {
        host,
        editor,
        issuer,
        issuer_auth,
        brief,
        temp,
    }
}
fn fix4_assignment(f: &Fixture, run: &RunId, own: bool) {
    let mut brief = f.brief.clone();
    brief.role = Role::Editor;
    let owner = f.host.owner.upgrade().unwrap();
    if own {
        let mut host = owner.lock().unwrap();
        let session = host.owner_mut().unwrap();
        session.save_active().unwrap();
        let mut branch = session
            .fork_durable_branch(
                "fix4 child",
                kinewright_project::editing_session::DurableBranchRole::RunCandidate,
            )
            .unwrap();
        session.persist_durable_branch(&mut branch).unwrap();
        brief.scope.base = session.candidate_binding(branch.branch_id()).unwrap();
    }
    let state = f.projection(run);
    let producer = f.producer_client(run);
    let request = f.request(
        &producer,
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
        .orchestration_cut(run)
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
    owner
        .lock()
        .unwrap()
        .publish_orchestration_transition(&producer.bootstrap.auth, &update, None)
        .unwrap();
    if own {
        let child = f.projection(run).tasks.last().unwrap().id.clone();
        b20_prepare_legacy_lane(f, run, &child);
    }
}
fn fix4_retry_work(kind: crate::orchestration::adapters::SendKind, own: bool) {
    use crate::orchestration::adapters::SendKind;
    let (f, _) = te15_fix_setup("codex");
    let (mut f, run) = if kind == SendKind::AutomaticResultWake {
        let r = c5_fix10_run_from(f);
        let reply = r
            .editor
            .wire("finish_task", te13_control(&r.editor, &r.child, &r.core));
        assert_eq!(reply.is_error, Some(false), "FIX4_REAL_RESULT {reply:?}");
        let run = r.run.clone();
        fix4_prior_work(&r.f, &run);
        (r.f, run)
    } else {
        let (run, _, _) = c4_director(&f);
        if kind == SendKind::OpeningAssignment {
            // c4_director creates an uncharged endpoint; retire its fixture reservation.
            let mut state = f.projection(&run);
            for a in &mut state.attempts {
                a.state = AttemptState::Retired(AttemptOutcome::Interrupted);
            }
            for t in &mut state.tasks {
                t.state = TaskState::Queued;
                t.progress.capacity = DependencyReservation::Free;
                t.progress.turn = None;
                t.progress.turn_ended = true;
            }
            crate::orchestration::tasks::refresh_cleanup(&mut state);
            publish_projection(&f, &f.producer_client(&run), &run, state, None);
        } else {
            fix4_prior_work(&f, &run);
        }
        if kind == SendKind::SpecialistStart {
            fix4_assignment(&f, &run, own);
        }
        if kind == SendKind::QueuedSteering {
            let text = f
                .host
                .owner
                .upgrade()
                .unwrap()
                .lock()
                .unwrap()
                .owner_mut()
                .unwrap()
                .admit_host_grant(b"FIX4 ORIGINAL STEERING: keep the shorter opening")
                .unwrap();
            let request = f.control(&f.editor, &run, RunAction::SendMessage { text_ref: text });
            ok(
                f.editor
                    .request(HostCommand::DispatchOrchestration {
                        request: Box::new(request),
                    })
                    .unwrap(),
                "FIX4_STEERING_QUEUED",
            );
        }
        (f, run)
    };
    let before = f.projection(&run);
    let original = before
        .effects
        .iter()
        .find(|e| e.state == EffectState::Pending)
        .unwrap()
        .clone();
    let (mut pump, sent) = fix4_pump(&f, &run, true);
    pump.tick().expect("FIX4_REFUSAL_RETAINED");
    assert!(sent.lock().unwrap().is_empty());
    let refused = f.projection(&run);
    let mut expected = original.clone();
    expected.state = EffectState::Pending;
    assert_eq!(
        refused.effects.iter().find(|e| e.id == original.id),
        Some(&expected),
        "FIX4_SAME_PENDING_EFFECT"
    );
    assert_eq!(refused.deliveries, before.deliveries, "FIX4_DELIVERY_LINKS");
    let failed = refused.attempts.last().unwrap().identity.clone();
    let charge = {
        let host = f.host.owner.upgrade().unwrap();
        let h = host.lock().unwrap();
        let a = h.native_accounting(&refused).unwrap().unwrap();
        a.tasks
            .iter()
            .flat_map(|t| &t.sends)
            .find(|send| send.attempt == failed.attempt.0)
            .unwrap()
            .clone()
    };
    drop(pump);
    f = fix4_reopen(f, &run);
    let reopened = f.projection(&run);
    assert_eq!(
        reopened.effects.iter().find(|e| e.id == original.id),
        Some(&expected),
        "FIX4_RESTART_PAYLOAD"
    );
    assert_eq!(
        reopened.deliveries, before.deliveries,
        "FIX4_RESTART_DELIVERY_LINKS"
    );
    let request = fix3_continue(&f, &run);
    ok(
        f.editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(request),
            })
            .unwrap(),
        "FIX4_CONTINUE_REPLAY",
    );
    assert_eq!(
        f.projection(&run).effects.len(),
        before.effects.len(),
        "FIX4_NO_GENERIC_REPLACEMENT"
    );
    let (mut pump, sent) = fix4_pump(&f, &run, false);
    pump.tick().unwrap();
    pump.tick().unwrap();
    pump.tick().unwrap();
    let sent = sent.lock().unwrap();
    assert_eq!(sent.len(), 1, "FIX4_ONE_ORIGINAL_SEND");
    assert_eq!(sent[0].task, original.task, "FIX4_ORIGINAL_TASK");
    assert_eq!(sent[0].kind, kind, "FIX4_ORIGINAL_KIND");
    let text = String::from_utf8_lossy(&sent[0].bytes);
    if kind == SendKind::AutomaticResultWake {
        assert!(
            text.contains(&before.deliveries[0].result.sha256),
            "FIX4_ORIGINAL_WAKE {text}"
        );
    } else {
        let payload = f
            .host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .store()
            .unwrap()
            .read_evidence(&original.payload)
            .unwrap();
        assert!(
            text.contains(&original.payload.sha256)
                || text.contains(std::str::from_utf8(&payload).unwrap()),
            "FIX4_ORIGINAL_PROMPT {text}"
        );
    }
    let state = f.projection(&run);
    assert!(
        state
            .effects
            .iter()
            .any(|e| e.id == original.id && matches!(e.state, EffectState::Claimed(_))),
        "FIX4_CLAIMED_ORIGINAL"
    );
    let host = f.host.owner.upgrade().unwrap();
    let h = host.lock().unwrap();
    let a = h.native_accounting(&state).unwrap().unwrap();
    assert_eq!(
        a.tasks
            .iter()
            .flat_map(|t| &t.sends)
            .filter(|send| send.attempt == failed.attempt.0)
            .collect::<Vec<_>>(),
        vec![&charge],
        "FIX4_ORIGINAL_CHARGE"
    );
    assert_eq!(
        a.tasks.iter().map(|t| t.sends.len()).sum::<usize>(),
        2,
        "FIX4_TWO_SLOTS_ONE_DELIVERY"
    );
}

#[test]
fn te15_b1_fix4_retry_opening() {
    if te15_fix_counting_child("te15_b1_fix4_retry_opening") {
        fix4_retry_work(
            crate::orchestration::adapters::SendKind::OpeningAssignment,
            false,
        );
    }
}

#[test]
fn te15_b1_fix4_retry_child_shared() {
    if te15_fix_counting_child("te15_b1_fix4_retry_child_shared") {
        fix4_retry_work(
            crate::orchestration::adapters::SendKind::SpecialistStart,
            false,
        );
    }
}

#[test]
fn te15_b1_fix4_retry_child_own() {
    if te15_fix_counting_child("te15_b1_fix4_retry_child_own") {
        fix4_retry_work(
            crate::orchestration::adapters::SendKind::SpecialistStart,
            true,
        );
    }
}

#[test]
fn te15_b1_fix4_retry_steering() {
    if te15_fix_counting_child("te15_b1_fix4_retry_steering") {
        fix4_retry_work(
            crate::orchestration::adapters::SendKind::QueuedSteering,
            false,
        );
    }
}

#[test]
fn te15_b1_fix4_retry_wake() {
    if te15_fix_counting_child("te15_b1_fix4_retry_wake") {
        fix4_retry_work(
            crate::orchestration::adapters::SendKind::AutomaticResultWake,
            false,
        );
    }
}

#[test]
fn te15_b1_fix4_empty_lane_gets_fresh_continuation() {
    if !te15_fix_counting_child("te15_b1_fix4_empty_lane_gets_fresh_continuation") {
        return;
    }
    let (f, _) = te15_fix_setup("codex");
    let (run, _, _) = c4_director(&f);
    fix4_prior_work(&f, &run);
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .wait_for_provider(&run, "Fix Settings.")
        .unwrap();
    let before = f.projection(&run);
    fix3_continue(&f, &run);
    let after = f.projection(&run);
    assert_eq!(
        after.effects.len(),
        before.effects.len() + 1,
        "FIX4_GENUINELY_EMPTY"
    );
    let host = f.host.owner.upgrade().unwrap();
    let payload = host
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .store()
        .unwrap()
        .read_evidence(&after.effects.last().unwrap().payload)
        .unwrap();
    assert_eq!(
        payload.as_slice(),
        b"The provider check passed. Continue this run from its retained task state.",
        "FIX4_CONTINUATION_WORDS"
    );
    let (mut pump, sent) = fix4_pump(&f, &run, false);
    pump.tick().unwrap();
    pump.tick().unwrap();
    assert_eq!(sent.lock().unwrap().len(), 1, "FIX4_EMPTY_ONE_SEND");
}
#[test]
fn te15_b1_fix4_no_run_retry_words() {
    if !te15_fix_counting_child("te15_b1_fix4_no_run_retry_words") {
        return;
    }
    let (f, _) = te15_fix_setup("codex");
    configure_director(&f);
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .fixture_director = false;
    let batch = manual_batch(&f);
    let revise = submitted_request(&f, &batch);
    let host = f.host.owner.upgrade().unwrap();
    host.lock().unwrap().native_readiness_checks = Some(Arc::new(Fix3Checks(Mutex::new(Some(
        CertificationRefusal::VersionProbePending,
    )))));
    let result = revise_result(
        f.editor
            .request(HostCommand::ReviseCut {
                request: revise.clone(),
            })
            .unwrap(),
        "FIX4_REVISE_WAIT",
    );
    assert!(
        matches!(result,ReviseResult::Waiting {ref reason} if reason=="The provider check failed before anything was sent. Press Start run to try again."),
        "FIX4_REVISE_WORDS {result:?}"
    );
    assert_eq!(consumed(&f, &batch), None);
    assert_eq!(run_count(&f), 0);
    let start = f.start_request(&f.editor);
    host.lock().unwrap().native_readiness_checks = Some(Arc::new(Fix3Checks(Mutex::new(Some(
        CertificationRefusal::SandboxUnavailable,
    )))));
    let reply = f
        .editor
        .request(HostCommand::DispatchOrchestration {
            request: Box::new(start),
        })
        .unwrap();
    assert_eq!(
        reply.error.unwrap().message,
        "The provider check failed before anything was sent. Start the run again to try again.",
        "FIX4_START_WORDS"
    );
    assert_eq!(run_count(&f), 0);
    let result = revise_result(
        f.editor
            .request(HostCommand::ReviseCut {
                request: revise.clone(),
            })
            .unwrap(),
        "FIX4_REVISE_RETRY",
    );
    assert!(
        matches!(result, ReviseResult::Accepted { .. }),
        "FIX4_REVISE_RETRY {result:?}"
    );
    let again = revise_result(
        f.editor
            .request(HostCommand::ReviseCut { request: revise })
            .unwrap(),
        "FIX4_REVISE_REPLAY",
    );
    assert_eq!(again, result);
    assert_eq!(run_count(&f), 1, "FIX4_REVISE_ONCE");
}
struct Fix4FailedWait {
    host: std::sync::Weak<Mutex<HostCommandOwner>>,
    calls: Arc<AtomicUsize>,
}
impl ConnectorRuntimeChecks for Fix4FailedWait {
    fn preflight(&self, _: &ConnectorSpecification) -> Result<(), CertificationRefusal> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.host
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .store()
            .unwrap()
            .exhaust_run_budget_probe();
        Err(CertificationRefusal::VersionProbePending)
    }
    fn probe(
        &self,
        _: &ConnectorSpecification,
        _: &crate::orchestration::adapters::AttemptBinding,
    ) -> Result<crate::orchestration::adapters::RuntimeQualification, CertificationRefusal> {
        panic!("FIX4_NO_INSTALLED_PROBE")
    }
}
#[test]
fn te15_b1_fix4_failed_wait_exits_pump() {
    if !te15_fix_counting_child("te15_b1_fix4_failed_wait_exits_pump") {
        return;
    }
    let (f, _) = te15_fix_setup("codex");
    let run: RunId = f.start().envelope.identity("run").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .native_readiness_checks = Some(Arc::new(Fix4FailedWait {
        host: f.host.owner.clone(),
        calls: calls.clone(),
    }));
    let (mut pump, sent) = fix4_pump(&f, &run, false);
    assert!(pump.tick().is_err(), "FIX4_WAIT_FAILURE_EXITS");
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "FIX4_NO_REPROBE"
    );
    assert!(sent.lock().unwrap().is_empty());
    assert!(
        matches!(f.projection(&run).wait, RunWait::None),
        "FIX4_WAIT_NOT_RECORDED"
    );
}

#[test]
fn te15_b1_fix4_live_child_uses_neutral_words() {
    if !te15_fix_counting_child("te15_b1_fix4_live_child_uses_neutral_words") {
        return;
    }
    let (f, _) = te15_fix_setup("codex");
    let (run, _, _) = c4_director(&f);
    fix4_prior_work(&f, &run);
    fix4_assignment(&f, &run, false);
    let (mut pump, _) = fix4_pump(&f, &run, false);
    pump.tick().unwrap();
    let state = f.projection(&run);
    let child = &state.tasks[1].id;
    assert!(
        state
            .effects
            .iter()
            .any(|effect| effect.task == *child && matches!(effect.state, EffectState::Claimed(_)))
    );
    assert!(
        !state
            .effects
            .iter()
            .any(|effect| effect.task == state.tasks[0].id
                && matches!(effect.state, EffectState::Claimed(_)))
    );
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .wait_for_provider(&run, "Fix Settings.")
        .unwrap();
    let owner = f.host.owner.upgrade().unwrap();
    let producer = f.producer(&run);
    for unknown in [false, true] {
        if unknown {
            let mut state = f.projection(&run);
            let identity = state.attempts.last().unwrap().identity.clone();
            for effect in &mut state.effects {
                if effect.task == *child && matches!(effect.state, EffectState::Claimed(_)) {
                    effect.state = EffectState::OutcomeUnknown;
                }
            }
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
        assert_eq!(
            reply.error.unwrap().message,
            if unknown {
                "recovery_requires_person: a task has an unknown send outcome; Continue cannot replay it."
            } else {
                "A task is still working. Continue when its turn ends."
            },
            "FIX4_RIGHT_TASK_WORDS"
        );
    }
}
