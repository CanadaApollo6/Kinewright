struct Delegate;
impl crate::SessionObserver for Delegate {
    fn on_event(&mut self, _: &AgentEvent) -> crate::SessionFlow {
        crate::SessionFlow::Continue
    }
    fn on_tick(&mut self, _: &crate::SessionCounters) -> crate::SessionFlow {
        crate::SessionFlow::Continue
    }
    fn prepare_studio_stop(
        &mut self,
        _: &crate::SessionStop,
        _: Option<&kinewright_core::RuntimeRequestBinding>,
    ) -> Result<(), String> {
        Ok(())
    }
}
struct LiveSession {
    request: kinewright_core::RuntimeRequest,
    binding: Arc<Mutex<Option<kinewright_core::RuntimeRequestBinding>>>,
    delivered: Arc<AtomicUsize>,
    sender: crossbeam_channel::Sender<AgentEvent>,
    events: crossbeam_channel::Receiver<AgentEvent>,
    unknown: bool,
    polled: Arc<AtomicUsize>,
}
impl kinewright_core::AgentSession for LiveSession {
    fn send_user_message(&mut self, _: String) -> Result<(), kinewright_core::AgentError> {
        Ok(())
    }
    fn events(&self) -> crossbeam_channel::Receiver<AgentEvent> {
        self.events.clone()
    }
    fn interrupt(&mut self) {
        *self.binding.lock().unwrap() = None;
    }
    fn poll_runtime_request(
        &mut self,
    ) -> Result<Option<kinewright_core::RuntimeRequest>, kinewright_core::AgentError> {
        self.polled.fetch_add(1, Ordering::SeqCst);
        Ok((self.unknown || self.delivered.load(Ordering::SeqCst) == 0)
            .then(|| self.request.clone()))
    }
    fn runtime_request_binding(&self) -> Option<kinewright_core::RuntimeRequestBinding> {
        self.binding.lock().unwrap().clone()
    }
    fn respond_runtime_request(
        &mut self,
        request: &kinewright_core::RuntimeRequest,
        response: kinewright_core::RuntimeResponse,
    ) -> Result<(), kinewright_core::AgentError> {
        response.validate(request)?;
        assert_eq!(request, &self.request, "H3_EXACT_LIVE_CALLBACK");
        assert_eq!(
            self.binding.lock().unwrap().as_ref(),
            Some(request.binding()),
            "H3_FRESH_CALLBACK"
        );
        assert_eq!(
            self.delivered.fetch_add(1, Ordering::SeqCst),
            0,
            "H3_ONE_CALLBACK"
        );
        if self.unknown {
            return Err(kinewright_core::AgentError::Unavailable(
                "response acknowledgment lost".into(),
            ));
        }
        *self.binding.lock().unwrap() = None;
        self.sender.send(AgentEvent::Done).unwrap();
        Ok(())
    }
}
struct Pump {
    cancelled: Arc<AtomicBool>,
    delivered: Arc<AtomicUsize>,
    events: crossbeam_channel::Sender<AgentEvent>,
    thread: Option<JoinHandle<()>>,
    polled: Arc<AtomicUsize>,
    gate: Option<Arc<[AtomicBool; 3]>>,
}
struct GatedObserver {
    inner: HostRunObserver,
    gate: Option<Arc<[AtomicBool; 3]>>,
}
impl crate::SessionObserver for GatedObserver {
    fn on_event(&mut self, event: &AgentEvent) -> crate::SessionFlow {
        self.inner.on_event(event)
    }
    fn on_tick(&mut self, counters: &crate::SessionCounters) -> crate::SessionFlow {
        self.inner.on_tick(counters)
    }
    fn supports_live_attention(&self) -> bool {
        self.inner.supports_live_attention()
    }
    fn on_runtime_binding(&mut self, binding: Option<&kinewright_core::RuntimeRequestBinding>) {
        self.inner.on_runtime_binding(binding);
    }
    fn on_runtime_request(
        &mut self,
        request: &kinewright_core::RuntimeRequest,
    ) -> kinewright_core::RuntimeResponse {
        self.inner.on_runtime_request(request)
    }
    fn prepare_studio_stop(
        &mut self,
        stop: &crate::SessionStop,
        binding: Option<&kinewright_core::RuntimeRequestBinding>,
    ) -> Result<(), String> {
        self.inner.prepare_studio_stop(stop, binding)
    }
    fn on_runtime_response_result(
        &mut self,
        request: &kinewright_core::RuntimeRequest,
        outcome: crate::LiveResponseOutcome,
    ) {
        self.inner.on_runtime_response_result(request, outcome);
        if let Some(gate) = &self.gate {
            gate[2].store(true, Ordering::Release);
        }
    }
    fn poll_runtime_response(&mut self) -> Option<kinewright_core::RuntimeResponseControl> {
        let response = self.inner.poll_runtime_response()?;
        if let Some(gate) = &self.gate {
            gate[0].store(true, Ordering::Release);
            let deadline = Instant::now() + Duration::from_secs(20);
            while !gate[1].load(Ordering::Acquire) {
                assert!(Instant::now() < deadline, "H3_DELIVERY_BARRIER_TIMEOUT");
                std::thread::park_timeout(Duration::from_millis(2));
            }
        }
        Some(response)
    }
}
impl Pump {
    fn start(
        f: &Fixture,
        run: &RunId,
        execution: &ExecutionIdentity,
        producer: &HostClient,
        kind: kinewright_core::RuntimeRequestKind,
    ) -> Self {
        Self::start_mode(f, run, execution, producer, kind, false)
    }
    fn start_mode(
        f: &Fixture,
        run: &RunId,
        execution: &ExecutionIdentity,
        producer: &HostClient,
        kind: kinewright_core::RuntimeRequestKind,
        unknown: bool,
    ) -> Self {
        Self::start_config(f, run, execution, producer, kind, unknown, None)
    }
    fn start_config(
        f: &Fixture,
        run: &RunId,
        execution: &ExecutionIdentity,
        producer: &HostClient,
        kind: kinewright_core::RuntimeRequestKind,
        unknown: bool,
        gate: Option<Arc<[AtomicBool; 3]>>,
    ) -> Self {
        let mut projection = f.projection(run);
        let turn = TaskTurn {
            task: execution.task.clone(),
            attempt: execution.attempt.clone(),
            session: identity(),
            ordinal: 1,
            owner_epoch: projection.attempts[0].owner_epoch,
            expected_head: f.brief.scope.base.head.clone(),
            purpose: TurnPurpose::Work,
        };
        projection.tasks[0].state = TaskState::Running(execution.attempt.clone());
        projection.tasks[0].progress.turn = Some(turn.clone());
        projection.attempts[0].state = AttemptState::Executing;
        publish_projection(f, producer, run, projection, None);
        let binding = kinewright_core::RuntimeRequestBinding {
            task: turn.task.0.as_str().to_owned(),
            attempt: turn.attempt.0.as_str().to_owned(),
            session: turn.session.as_str().to_owned(),
            turn: turn.ordinal,
            owner_epoch: turn.owner_epoch,
        };
        let request = kinewright_core::RuntimeRequest::new(
            "live-approval".into(),
            binding.clone(),
            kind,
            "Allow the requested operation?".into(),
        )
        .unwrap();
        let observer = f
            .host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .run_observer(
                &producer.bootstrap.auth,
                run,
                &execution.task,
                &execution.attempt,
                Box::new(Delegate),
            )
            .unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let delivered = Arc::new(AtomicUsize::new(0));
        let polled = Arc::new(AtomicUsize::new(0));
        let (sender, events) = crossbeam_channel::bounded(8);
        let mut session = LiveSession {
            request,
            binding: Arc::new(Mutex::new(Some(binding))),
            delivered: delivered.clone(),
            sender: sender.clone(),
            events,
            unknown,
            polled: polled.clone(),
        };
        let worker_cancelled = cancelled.clone();
        let worker_gate = gate.clone();
        let thread = std::thread::spawn(move || {
            let mut observer = GatedObserver {
                inner: observer,
                gate: worker_gate,
            };
            let mut prompts = ["fixture".to_owned()].into_iter();
            let events = kinewright_core::AgentSession::events(&session);
            crate::pump_session(
                &mut session,
                &events,
                &mut prompts,
                None,
                crate::ConfirmationPolicy::Studio,
                &crate::SessionLimits {
                    max_turns: 2,
                    max_wall_time: Duration::from_secs(30),
                    max_tokens: None,
                },
                &Arc::new(crate::SharedCounters::default()),
                &worker_cancelled,
                &mut observer,
            );
        });
        let pump = Self {
            cancelled,
            delivered,
            events: sender,
            thread: Some(thread),
            polled,
            gate,
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while f
            .host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .attention_prompts(run)
            .is_empty()
        {
            assert!(Instant::now() < deadline, "H3_LIVE_REQUEST_SETUP");
            std::thread::park_timeout(Duration::from_millis(2));
        }
        pump
    }
    fn wait_delivered(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.delivered.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < deadline, "H3_LIVE_CALLBACK_DELIVERED");
            std::thread::park_timeout(Duration::from_millis(2));
        }
    }
}
impl Drop for Pump {
    fn drop(&mut self) {
        if let Some(gate) = &self.gate {
            gate[1].store(true, Ordering::Release);
        }
        self.cancelled.store(true, Ordering::SeqCst);
        let _ = self.events.try_send(AgentEvent::Done);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}
fn response(f: &Fixture, run: &RunId) -> LiveAttentionResponse {
    let prompt = ok(
        f.editor
            .request(HostCommand::ObserveRun {
                run: run.clone(),
                after_sequence: 0,
            })
            .unwrap(),
        "H3_OBSERVE_ATTENTION",
    )
    .attention
    .pop()
    .unwrap();
    LiveAttentionResponse {
        run: run.clone(),
        request_id: prompt.request_id,
        binding_digest: prompt.binding_digest,
        command: 1,
        answer: AttentionAnswer::Approval(true),
    }
}
fn holder(f: &mut Fixture) {
    ok(
        f.editor.request(HostCommand::Detach).unwrap(),
        "H3_HOLDER_HANDOFF",
    );
    let auth = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .issue_person_facing_editor("person-holder".into(), f.host.roots.clone())
        .unwrap();
    f.editor = HostClient::connect(f.host.endpoint.clone(), auth, f.host.roots.clone()).unwrap();
}
#[test]
fn h3_viewer_approval_refused() {
    let mut f = Fixture::new();
    let (run, execution, producer) = reserve(&f);
    let pump = Pump::start(
        &f,
        &run,
        &execution,
        &producer,
        kinewright_core::RuntimeRequestKind::Approval,
    );
    let viewer = f.actor(AttachmentRole::Viewer, "viewer");
    let response = response(&f, &run);
    denied(
        &f,
        &viewer,
        HostCommand::ResolveAttention {
            response: response.clone(),
        },
        "attention_holder",
        "H3_VIEWER_APPROVAL",
    );
    assert_eq!(pump.delivered.load(Ordering::SeqCst), 0);
    holder(&mut f);
    ok(
        call(&f, &f.editor, HostCommand::ResolveAttention { response }, 1),
        "H3_HOLDER_APPROVAL",
    );
    pump.wait_delivered();
}
#[test]
fn h3_editor_role_alone_is_not_consent() {
    let mut f = Fixture::new();
    let (run, execution, producer) = reserve(&f);
    let pump = Pump::start(
        &f,
        &run,
        &execution,
        &producer,
        kinewright_core::RuntimeRequestKind::Approval,
    );
    let response = response(&f, &run);
    denied(
        &f,
        &f.editor,
        HostCommand::ResolveAttention {
            response: response.clone(),
        },
        "attention_holder",
        "H3_EDITOR_NOT_HOLDER",
    );
    holder(&mut f);
    ok(
        call(
            &f,
            &f.editor,
            HostCommand::ResolveAttention {
                response: response.clone(),
            },
            2,
        ),
        "H3_HOLDER_APPROVAL",
    );
    pump.wait_delivered();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .attention_prompts(&run)
        .is_empty()
    {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    denied(
        &f,
        &f.editor,
        HostCommand::ResolveAttention { response },
        "attention_expired",
        "H3_APPROVAL_REPLAY_FRESH",
    );
}
#[test]
fn h3_live_attention_refused_after_turn_changes() {
    let mut f = Fixture::new();
    let (run, execution, producer) = reserve(&f);
    let pump = Pump::start(
        &f,
        &run,
        &execution,
        &producer,
        kinewright_core::RuntimeRequestKind::Approval,
    );
    let response = response(&f, &run);
    holder(&mut f);
    let mut state = f.projection(&run);
    state.tasks[0].progress.turn.as_mut().unwrap().session = identity();
    publish_projection(&f, &producer, &run, state, None);
    denied(
        &f,
        &f.editor,
        HostCommand::ResolveAttention { response },
        "attention_expired",
        "H3_FRESH_LIVE_REQUEST",
    );
    assert_eq!(pump.delivered.load(Ordering::SeqCst), 0);
}
#[test]
fn h3_producer_approval_refused() {
    let f = Fixture::new();
    let (run, execution, producer) = reserve(&f);
    let pump = Pump::start(
        &f,
        &run,
        &execution,
        &producer,
        kinewright_core::RuntimeRequestKind::Approval,
    );
    let actor = f.actor(AttachmentRole::Producer, "unscoped-producer");
    denied(
        &f,
        &actor,
        HostCommand::ResolveAttention {
            response: response(&f, &run),
        },
        "attention_holder",
        "H3_PRODUCER_APPROVAL",
    );
    assert_eq!(pump.delivered.load(Ordering::SeqCst), 0);
}
#[test]
fn h3_secret_response_requires_matching_live_kind() {
    let mut f = Fixture::new();
    let (run, execution, producer) = reserve(&f);
    let pump = Pump::start(
        &f,
        &run,
        &execution,
        &producer,
        kinewright_core::RuntimeRequestKind::Secret,
    );
    let mut answer = response(&f, &run);
    holder(&mut f);
    denied(
        &f,
        &f.editor,
        HostCommand::ResolveAttention {
            response: answer.clone(),
        },
        "attention_kind",
        "H3_SECRET_KIND",
    );
    answer.answer = AttentionAnswer::SecretReference("1".repeat(32));
    ok(
        call(
            &f,
            &f.editor,
            HostCommand::ResolveAttention { response: answer },
            2,
        ),
        "H3_SECRET_REFERENCE",
    );
    pump.wait_delivered();
}
#[test]
fn h3_unknown_callback_never_reopens_consumed_consent() {
    let mut f = Fixture::new();
    let (run, execution, producer) = reserve(&f);
    let pump = Pump::start_mode(
        &f,
        &run,
        &execution,
        &producer,
        kinewright_core::RuntimeRequestKind::Approval,
        true,
    );
    let answer = response(&f, &run);
    holder(&mut f);
    ok(
        call(
            &f,
            &f.editor,
            HostCommand::ResolveAttention {
                response: answer.clone(),
            },
            1,
        ),
        "H3_UNKNOWN_CALLBACK_SETUP",
    );
    pump.wait_delivered();
    let repeats = pump.polled.load(Ordering::SeqCst) + 3;
    let deadline = Instant::now() + Duration::from_secs(5);
    while pump.polled.load(Ordering::SeqCst) < repeats {
        assert!(Instant::now() < deadline, "H3_REPEATED_REQUEST_OBSERVED");
        std::thread::park_timeout(Duration::from_millis(2));
    }
    assert!(
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .attention_prompts(&run)
            .is_empty(),
        "H3_CONSENT_ONCE"
    );
    denied(
        &f,
        &f.editor,
        HostCommand::ResolveAttention { response: answer },
        "attention_expired",
        "H3_CONSENT_ONCE",
    );
    assert_eq!(pump.delivered.load(Ordering::SeqCst), 1, "H3_CONSENT_ONCE");
}
#[test]
fn h3_attention_record_requires_actual_host_request() {
    let f = Fixture::new();
    let (run, execution, producer) = reserve(&f);
    let (actor, endpoint) = task_actor(&f, &run, &execution, &producer);
    let pump = Pump::start(
        &f,
        &run,
        &execution,
        &producer,
        kinewright_core::RuntimeRequestKind::Approval,
    );
    let prompt = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .attention_prompts(&run)
        .pop()
        .unwrap();
    let mut recorded = AttentionReference {
        task: execution.task.clone(),
        attempt: execution.attempt.clone(),
        request_ref: f.brief.plan.clone(),
    };
    let forged = task_request(
        &f,
        &actor,
        &endpoint,
        OrchestrationCommand::Request(RuntimeRequest::Approval(recorded.clone())),
    );
    denied(
        &f,
        &actor,
        HostCommand::DispatchOrchestration {
            request: Box::new(forged),
        },
        "attention_expired",
        "H3_RECORDED_ATTENTION_LIVE",
    );
    recorded.request_ref = prompt.request_ref;
    let request = task_request(
        &f,
        &actor,
        &endpoint,
        OrchestrationCommand::Request(RuntimeRequest::Approval(recorded)),
    );
    ok(
        call(
            &f,
            &actor,
            HostCommand::DispatchOrchestration {
                request: Box::new(request),
            },
            1,
        ),
        "H3_RECORDED_ATTENTION_CONTROL",
    );
    assert_eq!(
        pump.delivered.load(Ordering::SeqCst),
        0,
        "H3_RECORD_IS_NOT_CONSENT"
    );
}
#[test]
fn h3_queued_consent_is_revoked_with_its_holder() {
    let mut f = Fixture::new();
    let (run, execution, producer) = reserve(&f);
    let gate = Arc::new(std::array::from_fn(|_| AtomicBool::new(false)));
    let pump = Pump::start_config(
        &f,
        &run,
        &execution,
        &producer,
        kinewright_core::RuntimeRequestKind::Approval,
        false,
        Some(gate.clone()),
    );
    let answer = response(&f, &run);
    holder(&mut f);
    ok(
        call(
            &f,
            &f.editor,
            HostCommand::ResolveAttention { response: answer },
            1,
        ),
        "H3_QUEUED_CONSENT_SETUP",
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while !gate[0].load(Ordering::Acquire) {
        assert!(Instant::now() < deadline, "H3_DELIVERY_BARRIER");
        std::thread::park_timeout(Duration::from_millis(2));
    }
    ok(
        f.editor.request(HostCommand::Detach).unwrap(),
        "H3_DETACHED_HOLDER",
    );
    gate[1].store(true, Ordering::Release);
    while !gate[2].load(Ordering::Acquire) {
        assert!(Instant::now() < deadline, "H3_REVOKED_RESPONSE_PROCESSED");
        std::thread::park_timeout(Duration::from_millis(2));
    }
    assert_eq!(
        pump.delivered.load(Ordering::SeqCst),
        0,
        "H3_REVOKED_HOLDER"
    );
}
#[test]
fn h3_delegate_cannot_forge_live_consent() {
    use kinewright_core::{
        RuntimeRequest as LiveRequest, RuntimeRequestKind as Kind, RuntimeResponse as Answer,
        RuntimeResponseControl,
    };
    struct ForeignDelegate(Option<RuntimeResponseControl>);
    impl crate::SessionObserver for ForeignDelegate {
        fn on_event(&mut self, _: &AgentEvent) -> crate::SessionFlow {
            crate::SessionFlow::Continue
        }
        fn on_tick(&mut self, _: &crate::SessionCounters) -> crate::SessionFlow {
            crate::SessionFlow::Continue
        }
        fn poll_runtime_response(&mut self) -> Option<RuntimeResponseControl> {
            self.0.take()
        }
    }
    let f = Fixture::new();
    let (run, execution, producer) = reserve(&f);
    let binding = kinewright_core::RuntimeRequestBinding {
        task: execution.task.0.as_str().into(),
        attempt: execution.attempt.0.as_str().into(),
        session: identity::<kinewright_project::bundle_records::SessionId>()
            .as_str()
            .into(),
        turn: 1,
        owner_epoch: f.projection(&run).attempts[0].owner_epoch,
    };
    for (kind, answer) in [
        (Kind::Approval, Answer::ApprovalDecision(true)),
        (Kind::Secret, Answer::SecretReference("1".repeat(32))),
        (
            Kind::Question,
            Answer::QuestionAnswer("ordinary answer".into()),
        ),
    ] {
        let request =
            LiveRequest::new("delegate".into(), binding.clone(), kind, "fixture".into()).unwrap();
        let control = RuntimeResponseControl::new(request, answer)
            .unwrap()
            .with_live_authority(
                Arc::new(AtomicBool::new(true)),
                Arc::new(AtomicBool::new(false)),
                Instant::now() + Duration::from_secs(300),
            );
        let mut observer = f
            .host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .run_observer(
                &producer.bootstrap.auth,
                &run,
                &execution.task,
                &execution.attempt,
                Box::new(ForeignDelegate(Some(control))),
            )
            .unwrap();
        assert_eq!(
            crate::SessionObserver::poll_runtime_response(&mut observer).is_some(),
            kind == Kind::Question,
            "H3_DELEGATE_CONSENT"
        );
    }
}
