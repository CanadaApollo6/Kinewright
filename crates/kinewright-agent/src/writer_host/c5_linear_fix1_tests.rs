fn c5_fix1_context_to(f: &Fixture, run: &RunId, task: &TaskId, count: usize) {
    let producer = f.producer(run);
    let state = f.projection(run);
    let current = state
        .tasks
        .iter()
        .find(|record| &record.id == task)
        .unwrap()
        .progress
        .decisions
        .len();
    let execution = ExecutionIdentity {
        version: 1,
        project: f
            .host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .project_id()
            .unwrap(),
        run: run.clone(),
        task: task.clone(),
        attempt: identity(),
        nonce: identity(),
        owner_instance: identity(),
    };
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .publish_native(
            &producer,
            state,
            &execution,
            vec![f.brief.plan.clone(); count - current],
        )
        .unwrap();
    assert_eq!(
        f.projection(run)
            .tasks
            .iter()
            .find(|record| &record.id == task)
            .unwrap()
            .progress
            .decisions
            .len(),
        count
    );
}

#[test]
fn c5_linear_fix1_launch_refuses_before_any_seal_at_the_ordinary_bound() {
    let f = Fixture::new();
    f.host.runtime.stop();
    let run: RunId = f.start().envelope.identity("run").unwrap();
    let task = f.projection(&run).tasks[0].id.clone();
    c5_fix1_context_to(&f, &run, &task, TASK_DECISION_LIMIT);
    let producer = f.producer(&run);
    let before = state(&f);
    let budget = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .run_budget_probe(&run)
        .unwrap();
    assert!(!budget.exhausted);
    let projection = serde_json::to_vec(&f.projection(&run)).unwrap();
    let (result, trace) = kinewright_project::io_observer::observe(|| {
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .reserve_native_effect(&producer, &run)
    });
    let Err(HostError::Refused(error)) = result else {
        panic!("typed launch refusal");
    };
    assert_eq!(error.code, "task_decision_capacity");
    assert_eq!(error.message, TASK_DECISION_CAPACITY_WORDS);
    assert_eq!(serde_json::to_vec(&f.projection(&run)).unwrap(), projection);
    assert_eq!(
        state(&f),
        before,
        "no sealed intent, accounting, projection or latch"
    );
    assert_eq!(
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .run_budget_probe(&run)
            .unwrap(),
        budget,
        "no run-budget latch or counter change"
    );
    assert!(
        !trace
            .events
            .iter()
            .any(|event| event.operation.contains("seal"))
    );
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
#[allow(
    clippy::too_many_lines,
    reason = "one whole admitted lifecycle, dirty Stop and reopen witness"
)]
fn c5_linear_fix1_admitted_launch_keeps_its_slots_through_dirty_stop() {
    use crate::guardian::{AttemptRetirementProof, RecoveryObserver, StartupControl};
    use crate::native_launcher::InvocationOwner;
    let mut f = Fixture::new();
    f.host.runtime.stop();
    let run: RunId = f.start().envelope.identity("run").unwrap();
    let task = f.projection(&run).tasks[0].id.clone();
    c5_fix1_context_to(
        &f,
        &run,
        &task,
        TASK_DECISION_LIMIT - NATIVE_ATTEMPT_DECISIONS,
    );
    let producer = f.producer(&run);
    let host = f.host.owner.upgrade().unwrap();
    let admitted = host
        .lock()
        .unwrap()
        .reserve_native_effect(&producer, &run)
        .unwrap()
        .unwrap()
        .0;
    let execution = admitted.identity().clone();
    let before = state(&f);
    let original = f.projection(&run);
    let original_bytes = serde_json::to_vec(&original).unwrap();
    let result = host.lock().unwrap().publish_native(
        &producer,
        original,
        &execution,
        vec![f.brief.plan.clone()],
    );
    assert!(
        matches!(result, Err(HostError::Refused(HostRefusal { ref code, ref message, .. }))
        if code == "task_decision_capacity" && message == TASK_DECISION_CAPACITY_WORDS)
    );
    assert_eq!(
        state(&f),
        before,
        "other writers cannot spend the admitted launch's slots"
    );
    assert_eq!(
        serde_json::to_vec(&f.projection(&run)).unwrap(),
        original_bytes
    );
    let (actor, endpoint) = task_actor(&f, &run, &execution, &f.producer_client(&run));
    let core = host.lock().unwrap().hooks.runs[&run].tasks[&task]
        .core()
        .clone();
    let kinewright_core::Event::QueryResult(kinewright_core::QueryResult::Snapshot {
        revision,
        ..
    }) = core
        .request(kinewright_core::Command::Query(
            kinewright_core::Query::Snapshot,
        ))
        .unwrap()
    else {
        panic!("task revision");
    };
    let before = state(&f);
    let read = {
        let mut owner = host.lock().unwrap();
        let (index, projection, id) = owner
            .resolve_task_mcp(&actor.bootstrap.auth, &endpoint)
            .unwrap();
        let task = projection.tasks.iter().find(|task| task.id == id).unwrap();
        owner.read_context(
            index,
            &projection,
            task,
            &EvidenceRead {
                revision,
                evidence: EvidenceKind::Timeline { clips: vec![] },
                cursor: None,
            },
        )
    };
    assert!(
        matches!(read, Err(HostError::Refused(HostRefusal { ref code, ref message, .. }))
        if code == "task_decision_capacity" && message == TASK_DECISION_CAPACITY_WORDS)
    );
    assert_eq!(
        state(&f),
        before,
        "evidence expansion refuses before sealing or metadata issuance"
    );
    let (profile, _, scratch) = native_profile(&f, &admitted);
    let invocation = native_charge(&f, &admitted, profile);
    let port = super::super::native_invocation::HostInvocationOwner::new(&host, producer.clone());
    port.reserve(&invocation).unwrap();
    let count = f.projection(&run).tasks[0].progress.decisions.len();
    port.reserve(&invocation).unwrap();
    assert_eq!(
        f.projection(&run).tasks[0].progress.decisions.len(),
        count,
        "dispatch replay uses no slot"
    );
    let recovery = crate::job_guardian::protocol::RecoveryIdentity {
        version: crate::job_guardian::protocol::VERSION,
        attempt: unbound_identity(&execution),
        guardian_pid: std::process::id(),
        guardian_start: 1,
        containment: "fixture".into(),
        device: 1,
        inode: 1,
        service_device: 1,
        service_inode: 1,
    };
    port.release(&invocation, &recovery, &StartupControl::closed_fixture())
        .unwrap();
    assert_eq!(
        f.projection(&run).tasks[0].progress.decisions.len(),
        TASK_DECISION_LIMIT
    );
    history_turn(&f, &execution, &actor, &endpoint, 1, true);
    core.request(kinewright_core::Command::Do(history_marker(1)))
        .unwrap();
    let expected = core.export_history().unwrap();
    assert_ne!(cleanup_history(&f, &run, &task), expected);
    let machine = f.temp.path("c5-linear-lifecycle-machine");
    let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &machine,
        execution.project.as_str(),
        run.0.as_str(),
    )
    .unwrap();
    let native = unbound_identity(&execution);
    claim.reserve(&native).unwrap();
    let proof = RecoveryObserver::unbound_cleanup(&mut claim, &native).unwrap();
    let stop = f.control(&f.editor, &run, RunAction::Stop);
    ok(
        f.editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(stop),
            })
            .unwrap(),
        "C5_WHOLE_LAUNCH_STOP",
    );
    host.lock()
        .unwrap()
        .reconcile_native_effect(
            &producer,
            &execution,
            &AttemptRetirementProof::Unbound(&proof),
        )
        .unwrap();
    claim.retire_unbound(&proof).unwrap();
    claim.finish().unwrap();
    assert_eq!(f.projection(&run).cleanup, RunCleanup::Settled);
    assert_eq!(cleanup_history(&f, &run, &task), expected);
    drop(port);
    drop(invocation);
    drop(actor);
    drop(endpoint);
    drop(core);
    drop(host);
    f = c4_restart(f, &machine);
    f.host.runtime.stop();
    assert_eq!(
        cleanup_history(&f, &run, &task),
        expected,
        "retained dirty work reopens exactly"
    );
    eprintln!(
        "C5_WHOLE_LAUNCH decisions={TASK_DECISION_LIMIT} reserved={NATIVE_ATTEMPT_DECISIONS} dirty_stop=reopened_exact"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
#[allow(
    clippy::too_many_lines,
    reason = "24 distinct attempt failures share one parent at the ordinary bound"
)]
fn c5_linear_fix1_terminal_notifications_leave_the_history_slot() {
    use super::super::task_preparation::PreparationKind;
    let f = Fixture::new();
    f.host.runtime.stop();
    let run: RunId = f.start().envelope.identity("run").unwrap();
    let root = f.projection(&run).tasks[0].id.clone();
    c5_fix1_context_to(&f, &run, &root, TASK_DECISION_LIMIT);
    let producer = f.producer_client(&run);
    let host = f.host.owner.upgrade().unwrap();
    let mut projection = f.projection(&run);
    for _ in 0..7 {
        let mut brief = f.brief.clone();
        brief.role = Role::Editor;
        projection.tasks.push(TaskRecord {
            id: identity(),
            parent: Some(root.clone()),
            brief,
            state: TaskState::Queued,
            progress: TaskProgress::default(),
        });
    }
    publish_projection(&f, &producer, &run, projection, None);
    let mut expected_replans = BTreeSet::new();
    for n in 0..24 {
        let mut projection = f.projection(&run);
        host.lock()
            .unwrap()
            .archive_retired_attempts(&mut projection)
            .unwrap();
        let task = if n < 21 {
            projection.tasks[1 + n % 7].id.clone()
        } else {
            root.clone()
        };
        let execution = ExecutionIdentity {
            version: 1,
            project: host.lock().unwrap().owner().unwrap().project_id().unwrap(),
            run: run.clone(),
            task,
            attempt: identity(),
            nonce: identity(),
            owner_instance: identity(),
        };
        if n < 21 {
            expected_replans.insert(crate::orchestration::tasks::replan_effect_key(
                &execution.attempt,
            ));
        }
        let epoch = host
            .lock()
            .unwrap()
            .owner()
            .unwrap()
            .store()
            .unwrap()
            .owner_epoch();
        let mut intent =
            super::super::native_invocation::NativeLaunchIntent::new(&execution).unwrap();
        intent.prepared = Some(false);
        let recovery_ref = host
            .lock()
            .unwrap()
            .owner_mut()
            .unwrap()
            .admit_host_grant(&serde_json::to_vec(&intent).unwrap())
            .unwrap();
        projection.attempts.push(AttemptReservation {
            identity: execution.clone(),
            owner_epoch: epoch,
            // The pump reports a preparation failure after retiring its never-sent reservation.
            state: AttemptState::Retired(AttemptOutcome::Interrupted),
            recovery_ref,
        });
        publish_projection(&f, &producer, &run, projection, None);
        host.lock()
            .unwrap()
            .preparation_failed(
                &producer.bootstrap.auth,
                &execution,
                if n % 2 == 0 {
                    PreparationKind::History
                } else {
                    PreparationKind::Scope
                },
            )
            .unwrap();
        if n % 4 == 3 {
            eprintln!("C5_TERMINAL failure={}", n + 1);
        }
    }
    let projection = f.projection(&run);
    let parent = &projection.tasks[0].progress;
    assert_eq!(
        parent.decisions.len(),
        TASK_DECISION_LIMIT + 3,
        "21 child failures (two reasons) and three own failures (one reason)"
    );
    parent
        .check_recovery_decisions(1)
        .expect("a first history checkpoint still has its slot");
    assert!(parent.decisions.len() < TASK_RETAINED_DECISION_LIMIT);
    eprintln!(
        "C5_TERMINAL attempts=24 child_wakes=21 reasons=3 ordinary={TASK_DECISION_LIMIT} observed={} with_first_history={} retained_limit={TASK_RETAINED_DECISION_LIMIT}",
        parent.decisions.len(),
        parent.decisions.len() + 1
    );
    let replans: BTreeSet<_> = projection
        .effects
        .iter()
        .filter(|effect| expected_replans.contains(effect.id.0.as_str()))
        .map(|effect| effect.id.0.as_str())
        .collect();
    assert_eq!(
        replans,
        expected_replans
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        "every child failure keeps its own durable wake identity"
    );
    assert_eq!(
        host.lock()
            .unwrap()
            .attempt_archive(&projection)
            .unwrap()
            .1
            .attempts
            .len()
            + projection.attempts.len(),
        24
    );
}
