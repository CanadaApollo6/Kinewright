#[test]
#[cfg_attr(
    all(target_os = "linux", not(feature = "slow-tests")),
    ignore = "slow tier: cargo test --features slow-tests"
)]
#[allow(
    clippy::too_many_lines,
    reason = "one real pump pass, both readers, unchanged full task and completed Stop"
)]
fn c5_launch_capacity_wait_does_not_stall_other_tasks() {
    use crate::guardian::{AttemptRetirementProof, RecoveryObserver};

    if !te15_fix_counting_child("c5_launch_capacity_wait_does_not_stall_other_tasks") {
        return;
    }
    let (f, _) = te15_fix_setup("codex");
    let (run, _, director) = c4_director(&f);
    // The existing checked-assignment fixture retains two independent task lanes.
    // Neither task has sent; the Director endpoint remains live for observation.
    fix4_assignment(&f, &run, true);
    let full = f.projection(&run).tasks.last().unwrap().id.clone();
    fix4_assignment(&f, &run, true);
    let healthy = f.projection(&run).tasks.last().unwrap().id.clone();
    c5_fix1_context_to(&f, &run, &full, TASK_DECISION_LIMIT);
    let before = f.projection(&run);
    for task in [&full, &healthy] {
        assert!(before.effects.iter().any(|effect| {
            &effect.task == task
                && crate::orchestration::outbox::next_effect(&before, &effect.lane)
                    .is_some_and(|next| next.id == effect.id)
        }));
    }
    assert!(
        before.effects.iter().position(|effect| effect.task == full)
            < before
                .effects
                .iter()
                .position(|effect| effect.task == healthy),
        "C5_CAPACITY_FIRST_EFFECT_IS_FULL"
    );
    let full_bytes =
        serde_json::to_vec(before.tasks.iter().find(|t| t.id == full).unwrap()).unwrap();
    let (mut pump, sent) = fix4_pump(&f, &run, false);
    pump.tick()
        .expect("C5_CAPACITY_ONE_PASS_LAUNCHES_HEALTHY_TASK");
    {
        let sends = sent.lock().unwrap();
        assert_eq!(sends.len(), 1, "C5_CAPACITY_ONE_PASS_LAUNCHES_HEALTHY_TASK");
        assert_eq!(sends[0].task, healthy);
    }
    pump.tick().expect("C5_CAPACITY_LATER_TICK_DOES_NOT_ERROR");
    assert_eq!(sent.lock().unwrap().len(), 1);
    let after = f.projection(&run);
    assert_eq!(
        serde_json::to_vec(after.tasks.iter().find(|t| t.id == full).unwrap()).unwrap(),
        full_bytes,
        "C5_CAPACITY_FULL_TASK_UNCHANGED"
    );
    assert_eq!(after.wait, before.wait, "C5_CAPACITY_NO_RUN_LATCH");
    let page = b201_page(&f, &run);
    let wait = |page: &super::super::orchestration::RunViewPage| {
        page.accounting
            .as_ref()
            .and_then(|accounting| accounting.tasks.iter().find(|task| task.task == full))
            .and_then(|task| task.wait_reason.as_deref())
            .map(str::to_owned)
    };
    assert_eq!(wait(&page).as_deref(), Some(TASK_DECISION_CAPACITY_WORDS));
    let reply = director.raw("task_status", serde_json::json!({}));
    assert_eq!(
        reply.is_error,
        Some(false),
        "C5_CAPACITY_DIRECTOR_READ {reply:?}"
    );
    let observed: super::super::orchestration::RunViewPage =
        serde_json::from_value(reply.structured_content.unwrap()["run"].take()).unwrap();
    assert_eq!(
        wait(&observed).as_deref(),
        Some(TASK_DECISION_CAPACITY_WORDS)
    );
    let host = f.host.owner.upgrade().unwrap();
    // The projection is passed by value to the read-only derivation: room must
    // clear the wait without a stored wait-clear command or decision append.
    let mut room = after;
    room.tasks
        .iter_mut()
        .find(|t| t.id == full)
        .unwrap()
        .progress
        .decisions
        .truncate(TASK_DECISION_LIMIT - NATIVE_ATTEMPT_DECISIONS);
    assert!(!HostCommandOwner::native_launch_context_full(&room, &full));
    assert!(
        host.lock()
            .unwrap()
            .durable_run_accounting(&room)
            .unwrap()
            .unwrap()
            .tasks
            .iter()
            .filter(|t| t.task == full)
            .all(|t| t.wait_reason.is_none())
    );
    assert_eq!(
        serde_json::to_vec(
            f.projection(&run)
                .tasks
                .iter()
                .find(|t| t.id == full)
                .unwrap()
        )
        .unwrap(),
        full_bytes,
        "C5_CAPACITY_OBSERVATION_DOES_NOT_PUBLISH"
    );
    let state = f.projection(&run);
    let project = host.lock().unwrap().spending_project().unwrap();
    let producer = f.producer(&run);
    let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &f.temp.path("capacity-stop-machine"),
        project.as_str(),
        run.0.as_str(),
    )
    .unwrap();
    let mut proofs = vec![];
    for attempt in state
        .attempts
        .iter()
        .filter(|a| !matches!(a.state, AttemptState::Retired(_)))
    {
        let native = crate::native_launcher::native_identity(&attempt.identity);
        claim.reserve(&native).unwrap();
        let proof = RecoveryObserver::unbound_cleanup(&mut claim, &native).unwrap();
        proofs.push((attempt.identity.clone(), proof));
    }
    let stop = f.control(&f.editor, &run, RunAction::Stop);
    ok(
        f.editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(stop),
            })
            .unwrap(),
        "C5_CAPACITY_STOP",
    );
    for (identity, proof) in &proofs {
        host.lock()
            .unwrap()
            .reconcile_native_effect(&producer, identity, &AttemptRetirementProof::Unbound(proof))
            .unwrap();
        claim.retire_unbound(proof).unwrap();
    }
    claim.finish().unwrap();
    assert_eq!(
        f.projection(&run).cleanup,
        RunCleanup::Settled,
        "C5_CAPACITY_STOP_COMPLETES"
    );
}
