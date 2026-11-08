fn b201_pause(mode: &str) {
    let (f, _) = te15_fix_setup("codex");
    let run = if mode == "director" {
        f.start().envelope.identity("run").unwrap()
    } else {
        let (run, parent, director) = c4_director(&f);
        if mode == "child" {
            b20_assign(&f, &run, &parent, &director, Role::Editor);
        }
        b20_retire(&f, &run, &parent);
        if mode == "prepared" {
            let text = f
                .host
                .owner
                .upgrade()
                .unwrap()
                .lock()
                .unwrap()
                .owner_mut()
                .unwrap()
                .admit_host_grant(b"queued work")
                .unwrap();
            ok(
                f.editor
                    .request(HostCommand::DispatchOrchestration {
                        request: Box::new(f.control(
                            &f.editor,
                            &run,
                            RunAction::SendMessage { text_ref: text },
                        )),
                    })
                    .unwrap(),
                "B201_STEERING",
            );
        }
        run
    };
    let target = f
        .projection(&run)
        .effects
        .iter()
        .find(|e| e.state == EffectState::Pending)
        .unwrap()
        .clone();
    let (mut pump, sent) = fix4_pump(&f, &run, false);
    let gate = Arc::new(std::sync::Barrier::new(2));
    let release = Arc::new(std::sync::Barrier::new(2));
    let (a, b) = (gate.clone(), release.clone());
    pump.before_preparation = Some(Arc::new(move |_| {
        a.wait();
        b.wait();
        Ok(())
    }));
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| pump.tick());
        gate.wait();
        ok(
            f.editor
                .request(HostCommand::DispatchOrchestration {
                    request: Box::new(f.control(&f.editor, &run, RunAction::Pause)),
                })
                .unwrap(),
            "B201_REAL_PAUSE",
        );
        release.wait();
        worker.join().unwrap().unwrap();
    });
    let paused = f.projection(&run);
    assert_eq!(paused.phase, RunPhase::Paused, "B201_PAUSE_PRESERVED");
    assert_eq!(paused.wait, RunWait::None, "B201_PAUSE_NO_FAILURE_WORDS");
    assert!(
        paused.tasks.iter().all(|t| t.state != TaskState::Cancelled),
        "B201_PAUSE_NO_CANCEL"
    );
    assert!(
        paused.effects.iter().any(|e| e == &target),
        "B201_PAUSE_SAME_EFFECT"
    );
    assert!(
        crate::orchestration::machine::eligible(&paused, &RunAction::Continue),
        "B201_PAUSE_CONTINUE"
    );
    assert!(sent.lock().unwrap().is_empty());
    pump.before_preparation = None;
    fix3_continue(&f, &run);
    pump.tick().unwrap();
    assert_eq!(sent.lock().unwrap().len(), 1, "B201_PAUSE_ONE_SEND");
    assert!(
        f.projection(&run)
            .effects
            .iter()
            .any(|e| e.id == target.id && matches!(e.state, EffectState::Claimed(_))),
        "B201_PAUSE_SAME_CLAIM"
    );
}
#[test]
fn te15_b201_pause_fresh_director() {
    b201_pause("director");
}
#[test]
fn te15_b201_pause_fresh_child() {
    b201_pause("child");
}
#[test]
fn te15_b201_pause_prepared_director() {
    b201_pause("prepared");
}
fn b201_page(f: &Fixture, run: &RunId) -> super::super::orchestration::RunViewPage {
    f.editor
        .request(HostCommand::ObserveRun {
            run: run.clone(),
            after_sequence: 0,
        })
        .unwrap()
        .run_view
        .unwrap()
}
fn b201_retry(code: &str) {
    let (f, _) = te15_fix_setup("codex");
    let run = f.start().envelope.identity("run").unwrap();
    let original = f.projection(&run).effects[0].clone();
    let (mut pump, sent) = fix4_pump(&f, &run, false);
    let blocked = code.to_owned();
    pump.before_preparation = Some(Arc::new(move |_| {
        if blocked == "capacity" {
            Err(kinewright_project::bundle::BundleError::Capacity("fixture capacity").into())
        } else {
            Err(HostError::Io(std::io::Error::other(
                "fixture store failure",
            )))
        }
    }));
    pump.tick().unwrap();
    let state = f.projection(&run);
    assert!(
        state.tasks.iter().all(|t| t.state != TaskState::Cancelled),
        "B201_TRANSIENT_NO_CANCEL"
    );
    assert!(
        state.effects.iter().any(|e| e == &original),
        "B201_TRANSIENT_PENDING"
    );
    assert_eq!(state.effects.len(), 1, "B201_NO_EXTRA_DIRECTOR");
    assert_eq!(
        b201_page(&f, &run).wait_reason.as_deref(),
        Some(
            "The writer host could not start the task yet. This start sent nothing. Continue to try again."
        ),
        "B201_HOST_RETRY_WORDS"
    );
    assert!(sent.lock().unwrap().is_empty());
    pump.before_preparation = None;
    fix3_continue(&f, &run);
    pump.tick().unwrap();
    assert_eq!(sent.lock().unwrap().len(), 1, "B201_RETRY_ONE_SEND");
}
#[test]
fn te15_b201_capacity_retries() {
    b201_retry("capacity");
}
#[test]
fn te15_b201_io_retries() {
    b201_retry("io");
}
#[test]
fn te15_b201_typed_classification() {
    use super::super::task_preparation::{PreparationKind, preparation_refusal};
    let mut errors = vec![HostError::Io(std::io::Error::other(
        "history_changed task_scope",
    ))];
    for code in [
        "orchestration_refused",
        "recovering",
        "switch_frozen",
        "hooks.pending",
        "capacity",
        "attachment_cap",
        "storage_backpressure",
        "contention",
        "unsettled",
        "task_history",
        "role_refused",
    ] {
        errors.push(HostRefusal::new(code, "unprepared task scope changed").into());
    }
    for error in errors {
        let e = preparation_refusal(error);
        assert_eq!(
            e.get_ref()
                .unwrap()
                .downcast_ref::<super::super::task_preparation::PreparationRefusal>()
                .unwrap()
                .kind,
            PreparationKind::Retry,
            "B201_TYPED_RETRY"
        );
    }
    for code in [
        "history_changed",
        "task_scope",
        "task_preparation_scope",
        "task_preparation_inputs",
    ] {
        let e = preparation_refusal(HostRefusal::new(code, "unrelated text").into());
        assert!(
            e.get_ref()
                .unwrap()
                .downcast_ref::<super::super::task_preparation::PreparationRefusal>()
                .unwrap()
                .kind
                .deterministic(),
            "B201_TYPED_DETERMINISTIC"
        );
    }
}
fn b201_failed(f: &Fixture, run: &RunId, attempt: &ExecutionIdentity) {
    let auth = f.producer(run);
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .preparation_failed(
            &auth,
            attempt,
            super::super::task_preparation::PreparationKind::Scope,
        )
        .unwrap();
}
#[test]
fn te15_b201_director_words() {
    let f = b20_fixture();
    let (run, attempt, _) = c4_director(&f);
    b20_retire(&f, &run, &attempt);
    b201_failed(&f, &run, &attempt);
    let page = b201_page(&f, &run);
    assert_eq!(
        page.wait_reason.as_deref(),
        Some(
            "The writer host could not prepare the Director. This start sent nothing. Press Stop, then Reopen to recover any saved work before starting another run. Continue cannot repair it."
        ),
        "B201_DIRECTOR_WORDS"
    );
    assert_eq!(
        page.continue_unavailable_reason.as_deref(),
        Some("Continue cannot repair this task. Stop this run and start a new one."),
        "B201_DIRECTOR_NO_CONTINUE"
    );
    assert!(!page.allowed_actions.contains(&RunActionKind::Continue));
}
#[test]
fn te15_b201_sent_child_keeps_work() {
    let (f, _) = te15_fix_setup("codex");
    let r = c5_fix10_run_from(f);
    fix4_prior_work(&r.f, &r.run);
    let mut state = r.f.projection(&r.run);
    state
        .effects
        .iter_mut()
        .find(|e| e.task == r.child)
        .unwrap()
        .state = EffectState::Pending;
    publish_projection(&r.f, &r.f.producer_client(&r.run), &r.run, state, None);
    let (mut pump, sent) = fix4_pump(&r.f, &r.run, false);
    pump.tick().unwrap();
    assert_eq!(sent.lock().unwrap().len(), 1);
    let before = r.core.export_history().unwrap();
    fix4_prior_work(&r.f, &r.run);
    drop(pump);
    let mut state = r.f.projection(&r.run);
    state
        .effects
        .iter_mut()
        .find(|e| e.task == r.child)
        .unwrap()
        .state = EffectState::Pending;
    publish_projection(&r.f, &r.f.producer_client(&r.run), &r.run, state, None);
    let state = r.f.projection(&r.run);
    let identity = state
        .attempts
        .iter()
        .find(|a| a.identity.task == r.child)
        .unwrap()
        .identity
        .clone();
    let effects = state.effects.clone();
    let results = state.results.clone();
    b201_failed(&r.f, &r.run, &identity);
    let after = r.f.projection(&r.run);
    assert_ne!(
        after.tasks.iter().find(|t| t.id == r.child).unwrap().state,
        TaskState::Cancelled,
        "B201_SENT_NOT_CANCELLED"
    );
    assert_eq!(after.effects, effects, "B201_SENT_EFFECTS_KEPT");
    assert_eq!(after.results, results);
    assert_eq!(
        r.core.export_history().unwrap(),
        before,
        "B201_PAID_HISTORY_KEPT"
    );
    assert!(
        !b201_page(&r.f, &r.run)
            .allowed_actions
            .contains(&RunActionKind::Continue)
    );
    assert_eq!(
        b201_page(&r.f, &r.run).wait_reason.as_deref(),
        Some(super::super::task_preparation::SENT_TASK_RECOVERY),
        "B201_SENT_RECOVERY_WORDS"
    );
    ok(
        r.f.editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(r.f.control(&r.f.editor, &r.run, RunAction::Stop)),
            })
            .unwrap(),
        "B201_STOP_RECOVERY",
    );
    let owner = r.f.host.owner.upgrade().unwrap();
    let owner = owner.lock().unwrap();
    assert!(
        owner
            .owner()
            .unwrap()
            .retained_task_work()
            .unwrap()
            .iter()
            .any(|w| w.task == r.child && w.stopped && w.available),
        "B201_STOP_RETAINS_PAID_WORK"
    );
}
fn b201_replan_state() -> (Fixture, RunId, ExecutionIdentity, TaskId, ExecutionIdentity) {
    let f = b20_fixture();
    let (run, parent, director) = c4_director(&f);
    let child = b20_assign(&f, &run, &parent, &director, Role::Editor);
    let (attempt, _, _) = b20_start(&f, &run, &child);
    b20_retire(&f, &run, &attempt);
    b20_retire(&f, &run, &parent);
    (f, run, parent, child, attempt)
}
fn b201_wait_order(kind: &str) {
    let (f, run, _, child, attempt) = b201_replan_state();
    let mut state = f.projection(&run);
    let reason = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner_mut()
        .unwrap()
        .admit_host_grant(b"existing wait")
        .unwrap();
    let detail = WaitDetail {
        reason,
        missing: vec![],
        attempt: None,
    };
    state.wait = match kind {
        "files" => RunWait::Files(detail),
        "provider" => RunWait::Provider(detail),
        _ => RunWait::ProviderSettings(detail),
    };
    state
        .tasks
        .iter_mut()
        .find(|t| t.id == child)
        .unwrap()
        .progress
        .capacity = DependencyReservation::Reserved;
    let wait = state.wait.clone();
    publish_projection(&f, &f.producer_client(&run), &run, state, None);
    b201_failed(&f, &run, &attempt);
    assert_eq!(f.projection(&run).wait, wait, "B201_WAIT_PRIORITY");
    assert_eq!(
        f.projection(&run)
            .tasks
            .iter()
            .find(|t| t.id == child)
            .unwrap()
            .progress
            .capacity,
        DependencyReservation::Free,
        "B201_CANCEL_CAPACITY_FREE"
    );
}
#[test]
fn te15_b201_files_priority() {
    b201_wait_order("files");
}
#[test]
fn te15_b201_provider_priority() {
    b201_wait_order("provider");
}
#[test]
fn te15_b201_settings_priority() {
    b201_wait_order("settings");
}
#[test]
fn te15_b201_unknown_priority_and_accounting() {
    let (f, run, _, _, attempt) = b201_replan_state();
    b201_failed(&f, &run, &attempt);
    let owner = f.host.owner.upgrade().unwrap();
    // Create the ordinary accounting record, then read the retained host reason.
    let auth = f.producer(&run);
    owner
        .lock()
        .unwrap()
        .change_spending(
            &auth,
            &attempt,
            super::super::native_invocation::SpendingChange::Clock,
            crate::protocol::limits::usage::ClockObservation {
                unix_ms: super::super::spending::unix_ms().unwrap(),
                monotonic_delta_ms: 0,
                trusted: true,
            },
        )
        .unwrap();
    let state = f.projection(&run);
    let stop_reason = owner
        .lock()
        .unwrap()
        .durable_run_accounting(&state)
        .unwrap()
        .unwrap()
        .stop_reason;
    assert_eq!(
        stop_reason.as_deref(),
        b201_page(&f, &run).wait_reason.as_deref(),
        "B201_ACCOUNTING_REASON"
    );
    let mut state = state;
    state.effects[0].state = EffectState::OutcomeUnknown;
    publish_projection(&f, &f.producer_client(&run), &run, state, None);
    assert_eq!(
        b201_page(&f, &run).continue_unavailable_reason.as_deref(),
        Some("A task has an unknown send outcome. Continue cannot retry this send."),
        "B201_UNKNOWN_OUTRANKS_REPLAN"
    );
}
#[test]
fn te15_b201_notification_ends_without_assignment() {
    let (f, _) = te15_fix_setup("codex");
    let (run, parent, director) = c4_director(&f);
    let child = b20_assign(&f, &run, &parent, &director, Role::Editor);
    let (attempt, _, _) = b20_start(&f, &run, &child);
    b20_retire(&f, &run, &attempt);
    b20_retire(&f, &run, &parent);
    b201_failed(&f, &run, &attempt);
    drop(director);
    let (mut pump, sent, end) = b201_ending_pump(&f, &run);
    pump.tick().unwrap();
    assert_eq!(sent.lock().unwrap().len(), 1, "B201_ONE_NOTIFICATION");
    assert!(
        !matches!(f.projection(&run).wait, RunWait::Replan(_)),
        "B201_NOTIFICATION_CONSUMED"
    );
    // The scripted transport has no OS process. Record its turn exit and
    // retirement through the same H1 paths used by existing transport fixtures.
    let state = f.projection(&run);
    let identity = state
        .attempts
        .iter()
        .find(|a| a.identity.task == parent.task)
        .unwrap()
        .identity
        .clone();
    b20_retire(&f, &run, &identity);
    end.lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .send(kinewright_core::AgentEvent::Done)
        .unwrap();
    pump.tick().unwrap();
    let page = b201_page(&f, &run);
    assert_eq!(
        page.projection.phase,
        RunPhase::Interrupted,
        "B201_STALL_INTERRUPTED"
    );
    assert!(
        page.wait_reason.unwrap().starts_with("silent_stall:"),
        "B201_STALL_REASON"
    );
    assert_eq!(sent.lock().unwrap().len(), 1, "B201_NO_EXTRA_SPEND");
}
#[test]
fn te15_b201_other_run_candidate_refuses() {
    let f = b20_fixture();
    let run: RunId = f.start().envelope.identity("run").unwrap();
    ok(
        f.editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(f.control(&f.editor, &run, RunAction::Stop)),
            })
            .unwrap(),
        "B201_PRIOR_STOP",
    );
    let _other = f.start();
    let state = f.projection(&run);
    assert!(
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .preparation_base(&state)
            .is_err(),
        "B201_OTHER_RUN_OWNER"
    );
}

fn b201_receipt_check(mode: &str) {
    use kinewright_project::{bundle_records::BranchCheckpoint, receipts::ReceiptLookup};
    let f = b20_fixture();
    let (run, parent, director) = c4_director(&f);
    let child = b20_assign(&f, &run, &parent, &director, Role::Editor);
    let (attempt, core, mcp) = b20_start(&f, &run, &child);
    b20_edit(
        &mcp,
        &core,
        &serde_json::json!([{"op":"move_marker","marker":1,"to":2}]),
    );
    let (reference, result) = b20_publish(&f, &run, &child, &mcp);
    b20_retire(&f, &run, &attempt);
    assert_eq!(
        b20_merge(&director, &child, &reference, &result).is_error,
        Some(false)
    );
    let owner = f.host.owner.upgrade().unwrap();
    let state = f.projection(&run);
    let owner = owner.lock().unwrap();
    let history = owner.saved_task_history(&state, &child).unwrap().unwrap();
    let envelope = history.merge_envelope.unwrap();
    let ReceiptLookup::Committed(mut receipt) = owner
        .owner()
        .unwrap()
        .manual_transition_receipt(&envelope)
        .unwrap()
    else {
        panic!("committed")
    };
    let kinewright_project::receipts::ReceiptResult::Edit { checkpoint_ref, .. } = &receipt.result
    else {
        panic!("edit")
    };
    let checkpoint = owner
        .owner()
        .unwrap()
        .store()
        .unwrap()
        .read_record::<BranchCheckpoint>(checkpoint_ref)
        .unwrap();
    let mut checkpoint = (*checkpoint).clone();
    match mode {
        "receipt" => receipt.target_branch = identity(),
        "branch" => checkpoint.branch_id = identity(),
        "head" => checkpoint.head_id = identity(),
        _ => checkpoint.predecessor = None,
    }
    assert!(
        super::super::task_preparation::validate_merge_checkpoint(
            &f.brief.scope.base,
            &history.base,
            &receipt,
            &checkpoint
        )
        .is_err(),
        "B201_RECEIPT_GUARD {mode}"
    );
}
#[test]
fn te15_b201_receipt_branch() {
    b201_receipt_check("receipt");
}
#[test]
fn te15_b201_checkpoint_branch() {
    b201_receipt_check("branch");
}
#[test]
fn te15_b201_checkpoint_head() {
    b201_receipt_check("head");
}
#[test]
fn te15_b201_checkpoint_predecessor() {
    b201_receipt_check("predecessor");
}
#[test]
fn te15_b201_fresh_child_after_merge_restart() {
    let f = b20_fixture();
    let (run, parent, director) = c4_director(&f);
    let child = b20_assign(&f, &run, &parent, &director, Role::Editor);
    let (attempt, core, mcp) = b20_start(&f, &run, &child);
    b20_edit(
        &mcp,
        &core,
        &serde_json::json!([{"op":"move_marker","marker":1,"to":2}]),
    );
    let (reference, result) = b20_publish(&f, &run, &child, &mcp);
    b20_retire(&f, &run, &attempt);
    assert_eq!(
        b20_merge(&director, &child, &reference, &result).is_error,
        Some(false)
    );
    drop(mcp);
    drop(director);
    let claim = f.temp.path("b201-restart");
    let f = c4_restart(f, &claim);
    let (_, _, director) = b20_start(&f, &run, &parent.task);
    let child = b20_assign(&f, &run, &parent, &director, Role::Editor);
    let (attempt, core, mcp) = b20_start(&f, &run, &child);
    assert_ne!(
        f.projection(&run).tasks.last().unwrap().brief.scope.base,
        f.brief.scope.base,
        "B201_FRESH_RESTART_BASE"
    );
    b20_edit(
        &mcp,
        &core,
        &serde_json::json!([{"op":"move_marker","marker":1,"to":3}]),
    );
    let (reference, result) = b20_publish(&f, &run, &child, &mcp);
    b20_retire(&f, &run, &attempt);
    assert_eq!(
        b20_merge(&director, &child, &reference, &result).is_error,
        Some(false),
        "B201_FRESH_RESTART_MERGE"
    );
}

struct B201EndingLauncher {
    sent: Arc<Mutex<Vec<Fix4Sent>>>,
    end: Arc<Mutex<Option<crossbeam_channel::Sender<kinewright_core::AgentEvent>>>>,
}
impl crate::orchestration::adapters::GuardianLauncher for B201EndingLauncher {
    fn launch(
        &self,
        attempt: crate::orchestration::AdmittedAttempt,
        invocation: crate::orchestration::adapters::AuthorizedInvocation,
    ) -> Result<
        crate::orchestration::adapters::ContainedSession,
        crate::orchestration::adapters::LaunchError,
    > {
        self.sent.lock().unwrap().push(Fix4Sent {
            task: attempt.identity().task.clone(),
            kind: invocation.invocation().kind(),
            bytes: invocation.invocation().bytes().to_vec(),
        });
        let (tx, rx) = crossbeam_channel::unbounded();
        *self.end.lock().unwrap() = Some(tx);
        Ok(
            crate::orchestration::adapters::ContainedSession::from_guardian(Box::new(
                Te15NoProcess(rx),
            )),
        )
    }
}
type B201End = Arc<Mutex<Option<crossbeam_channel::Sender<kinewright_core::AgentEvent>>>>;
fn b201_ending_pump(
    f: &Fixture,
    run: &RunId,
) -> (
    crate::effect_pump::EffectPump<B201EndingLauncher>,
    Arc<Mutex<Vec<Fix4Sent>>>,
    B201End,
) {
    let owner = f.host.owner.upgrade().unwrap();
    let project = owner.lock().unwrap().spending_project().unwrap();
    let claim = Arc::new(Mutex::new(
        crate::execution_claim::MachineClaim::acquire_fixture(
            &f.temp.path("b201-ending"),
            project.as_str(),
            run.0.as_str(),
        )
        .unwrap(),
    ));
    let sent = Arc::new(Mutex::new(vec![]));
    let end = Arc::new(Mutex::new(None));
    let pump = crate::effect_pump::EffectPump::with_launcher(
        &owner,
        f.producer(run),
        run.clone(),
        claim,
        B201EndingLauncher {
            sent: sent.clone(),
            end: end.clone(),
        },
        Arc::new(Te15Profiles {
            runtime: f.host.runtime.clone(),
            host: Arc::downgrade(&owner),
            servers: Mutex::default(),
            scratch: Mutex::default(),
        }),
        None,
    );
    (pump, sent, end)
}

// Wait derivation resumes after the notification is consumed. These are the
// ordinary question/children inputs to the same kernel used by the live pump.
fn b201_next_wait(question: bool) {
    let (f, run, parent, _, attempt) = b201_replan_state();
    b201_failed(&f, &run, &attempt);
    let mut state = f.projection(&run);
    let notification = state
        .effects
        .iter_mut()
        .find(|e| e.state == EffectState::Pending)
        .unwrap();
    notification.state = EffectState::Claimed(parent.attempt.clone());
    let root = state
        .tasks
        .iter()
        .find(|t| t.id == parent.task)
        .unwrap()
        .clone();
    if question {
        let id =
            serde_json::from_value(serde_json::json!("abababababababababababababababab")).unwrap();
        state
            .requests
            .push(RuntimeRequest::Question(QuestionRecord {
                id,
                task: root.id,
                prompt: root.brief.assignment,
                origin: TaskTurn {
                    task: parent.task,
                    attempt: parent.attempt,
                    session: serde_json::from_value(serde_json::json!(
                        "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"
                    ))
                    .unwrap(),
                    ordinal: 1,
                    owner_epoch: 1,
                    expected_head: root.brief.scope.base.head,
                    purpose: TurnPurpose::Work,
                },
                status: QuestionStatus::Pending,
                wake: QuestionWake::Pending,
            }));
    } else {
        let mut sibling = root.clone();
        sibling.id =
            serde_json::from_value(serde_json::json!("abababababababababababababababab")).unwrap();
        sibling.parent = Some(root.id.clone());
        sibling.state = TaskState::Queued;
        state
            .tasks
            .iter_mut()
            .find(|t| t.id == root.id)
            .unwrap()
            .state = TaskState::WaitingForChildren(vec![sibling.id.clone()]);
        state.tasks.push(sibling);
    }
    crate::orchestration::tasks::refresh_wait(&mut state);
    assert!(
        if question {
            matches!(state.wait, RunWait::Question(_))
        } else {
            matches!(state.wait, RunWait::Children(_))
        },
        "B201_NEXT_WAIT"
    );
}
#[test]
fn te15_b201_question_after_notification() {
    b201_next_wait(true);
}
#[test]
fn te15_b201_children_after_notification() {
    b201_next_wait(false);
}

#[test]
fn te15_b201_legacy_unprepared_is_not_proof() {
    let (f, _) = te15_fix_setup("codex");
    let run = f.start().envelope.identity("run").unwrap();
    let (mut pump, sent) = fix4_pump(&f, &run, false);
    pump.before_preparation = Some(Arc::new(|_| {
        Err(HostError::Io(std::io::Error::other(
            "fixture store failure",
        )))
    }));
    pump.tick().unwrap();
    let mut state = f.projection(&run);
    let identity = state.attempts[0].identity.clone();
    let owner = f.host.owner.upgrade().unwrap();
    let reference = {
        let mut host = owner.lock().unwrap();
        let mut intent = host
            .native_launch_intent(&state, &identity)
            .unwrap()
            .unwrap();
        assert_eq!(intent.prepared, Some(false));
        // Simulate a retained pre-fix intent: absence cannot prove that a task
        // branch was never created. Do not refork it after a missing history.
        intent.prepared = None;
        host.owner_mut()
            .unwrap()
            .admit_host_grant(&serde_json::to_vec(&intent).unwrap())
            .unwrap()
    };
    state.attempts[0].recovery_ref = reference;
    publish_projection(&f, &f.producer_client(&run), &run, state, None);
    pump.before_preparation = None;
    fix3_continue(&f, &run);
    pump.tick().unwrap();
    assert!(
        sent.lock().unwrap().is_empty(),
        "B201_LEGACY_HISTORY_FAILS_CLOSED"
    );
    assert!(
        f.projection(&run)
            .tasks
            .iter()
            .all(|t| t.state != TaskState::Cancelled)
    );
}
