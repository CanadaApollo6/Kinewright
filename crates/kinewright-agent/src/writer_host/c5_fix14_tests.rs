/// C5 fix 14: publish an owner-built projection for a run, returning the
/// result instead of asserting it, as Astra's review probe does.
fn c5_fix14_publish(
    f: &Fixture,
    actor: &HostClient,
    run: &RunId,
    mut projection: RunProjection,
) -> Result<OrchestrationReceipt, HostError> {
    let request = f.request(
        actor,
        Some(run.clone()),
        OrchestrationCommand::RecordContext {
            task: projection.tasks[0].id.clone(),
            feedback: vec![],
            decisions: vec![],
        },
    );
    projection.sequence += 1;
    projection.control_revision = request.envelope.identity("control-revision").unwrap();
    let owner = f.host.owner.upgrade().unwrap();
    let mut host = owner.lock().unwrap();
    let segment = EventSegment {
        run: run.clone(),
        first_sequence: projection.sequence,
        previous: Some(
            host.owner()
                .unwrap()
                .orchestration_cut(run)
                .unwrap()
                .unwrap()
                .segment,
        ),
        events: vec![OrchestrationEvent {
            command: request.envelope.id.clone(),
            sequence: projection.sequence,
            projection: projection.clone(),
        }],
    };
    let update = ValidatedRunUpdate::new(
        &request.envelope.project,
        request.envelope.expected_generation,
        &request.envelope,
        projection,
        segment,
        CommandDecision::Applied,
    )
    .unwrap();
    host.publish_orchestration_transition(&actor.bootstrap.auth, &update, None)
        .map(|(receipt, _)| receipt)
}

fn c5_fix14_refused(result: &Result<OrchestrationReceipt, HostError>, marker: &str) {
    assert!(
        result
            .as_ref()
            .is_err_and(|error| error.to_string().contains("Run limit")),
        "{marker} {result:?}"
    );
}

/// Astra's review probe: a finished, settled run flips its phase to
/// Interrupted (a terminal-shaped change) and back. At 741102aa each round
/// spent a terminal cut, and after 31 rounds the real archive got `run_limit`.
#[test]
fn s4b_c5_fix14_finished_phase_cycle_is_refused_and_archive_still_runs() {
    let f = Fixture::new();
    f.host.runtime.stop();
    let manual = c5_fix14_seed_manual(&f);
    let (run, execution, actor) = reserve(&f);
    c5_fix13_ceiling(&f, &run, &execution);
    let claim_root = f.temp.path("c5-fix13-machine");
    let settled = c5_fix13_stop_settle(&f, &run, &execution, &claim_root);
    let before = C5Fix14Before::capture(&f, settled.clone(), manual);
    for round in 0..31 {
        let mut state = f.projection(&run);
        state.phase = RunPhase::Interrupted;
        let result = c5_fix14_publish(&f, &actor, &run, state);
        if result.is_ok() {
            eprintln!("C5_FIX14_PHASE_CYCLE_ACCEPTED round={round}");
            let mut state = f.projection(&run);
            state.phase = RunPhase::Finished;
            if c5_fix14_publish(&f, &actor, &run, state).is_err() {
                break;
            }
            continue;
        }
        c5_fix14_refused(&result, "C5_FIX14_PHASE_CYCLE_REASON");
        break;
    }
    assert_eq!(
        f.projection(&run),
        settled,
        "C5_FIX14_PHASE_CYCLE_REFUSED_AT_FIRST_STEP"
    );
    let result = c5_fix13_archive(&f, &run, &execution);
    assert!(
        result.is_ok(),
        "C5_FIX14_ARCHIVE_AFTER_CYCLE_REFUSED {result:?}"
    );
    drop(actor);
    c5_fix13_after_archive(f, &before, &claim_root);
}

type C5Fix14Change = (&'static str, fn(&mut RunProjection));

/// A finished, settled run may not change any other field, whether the
/// change looks like terminal progress or not.
#[test]
fn s4b_c5_fix14_finished_run_refuses_every_other_change() {
    let f = Fixture::new();
    f.host.runtime.stop();
    let manual = c5_fix14_seed_manual(&f);
    let (run, execution, actor) = reserve(&f);
    c5_fix13_ceiling(&f, &run, &execution);
    let claim_root = f.temp.path("c5-fix13-machine");
    let settled = c5_fix13_stop_settle(&f, &run, &execution, &claim_root);
    let before = C5Fix14Before::capture(&f, settled.clone(), manual);
    let changes: [C5Fix14Change; 5] = [
        // Removes the retired attempts without sealing them into an archive.
        ("unsealed_removal", |s| s.attempts.clear()),
        // Removes the retired attempts and also drops a result.
        ("removal_with_result", |s| {
            s.attempts.clear();
            s.results.clear();
        }),
        ("results", |s| s.results.clear()),
        ("usage", |s| s.usage.clear()),
        ("phase", |s| s.phase = RunPhase::Running),
    ];
    for (name, change) in changes {
        let mut state = f.projection(&run);
        change(&mut state);
        if state == settled {
            continue;
        }
        c5_fix14_refused(
            &c5_fix14_publish(&f, &actor, &run, state),
            &format!("C5_FIX14_OTHER_CHANGE_REFUSED {name}"),
        );
        assert_eq!(
            f.projection(&run),
            settled,
            "C5_FIX14_UNCHANGED_AFTER {name}"
        );
    }
    let result = c5_fix13_archive(&f, &run, &execution);
    assert!(result.is_ok(), "C5_FIX14_ARCHIVE_AFTER_REFUSALS {result:?}");
    drop(actor);
    c5_fix13_after_archive(f, &before, &claim_root);
}

/// A run whose archive already holds an attempt from before Stop: the final
/// archive keeps that prefix and appends the settled run's retired attempts.
#[test]
fn s4b_c5_fix14_archive_keeps_its_existing_prefix() {
    let f = Fixture::new();
    let manual = c5_fix14_seed_manual(&f);
    let start = f.start();
    let run: RunId = start.envelope.identity("run").unwrap();
    let producer = f.producer(&run);
    let claim_root = f.temp.path("c5-fix14-prefix");
    let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &claim_root,
        start.envelope.project.as_str(),
        run.0.as_str(),
    )
    .unwrap();
    let mut last = None;
    for _ in 0..2 {
        let next = f
            .host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .reserve_native_effect(&producer, &run);
        let (admitted, _, _) = next.unwrap().expect("C5_FIX14_PREFIX_RESERVATION");
        last = Some(admitted.identity().clone());
        (claim, _) =
            crate::effect_pump::recover_run(&f.host.owner.upgrade().unwrap(), claim, &run).unwrap();
    }
    claim.finish().unwrap();
    let execution = last.unwrap();
    let prefix = c5_fix14_archive(&f, &f.projection(&run));
    assert_eq!(prefix.len(), 1, "C5_FIX14_PREFIX_SEEDED");
    let settled = c5_fix13_stop_settle(&f, &run, &execution, &claim_root);
    let before = C5Fix14Before::capture(&f, settled, manual);
    assert_eq!(before.prefix, prefix);
    let result = c5_fix13_archive(&f, &run, &execution);
    assert!(result.is_ok(), "C5_FIX14_PREFIX_ARCHIVE_REFUSED {result:?}");
    c5_fix13_after_archive(f, &before, &claim_root);
}

/// Restart after Stop, before cleanup settles. The reopened host settles the
/// run during recovery and keeps its Producer, then archives it. (A run that
/// settled before the reopen is not retained by the new host, so it has no
/// Producer to archive with; this is the latest restart that can archive.)
#[test]
fn s4b_c5_fix14_restart_after_stop_then_settle_and_archive() {
    let f = Fixture::new();
    f.host.runtime.stop();
    let manual = c5_fix14_seed_manual(&f);
    let (run, execution, actor) = reserve(&f);
    c5_fix13_ceiling(&f, &run, &execution);
    let stop = f.control(&f.editor, &run, RunAction::Stop);
    ok(
        f.editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(stop),
            })
            .unwrap(),
        "C5_FIX14_RESTART_STOP",
    );
    drop(actor);
    let stopped = f.projection(&run);
    let claim_root = f.temp.path("c5-fix13-machine");
    let f = c4_restart(f, &claim_root);
    let reopened = f.projection(&run);
    eprintln!(
        "C5_FIX14_RESTART_STATE cleanup={:?} attempts={:?} retained={}",
        reopened.cleanup,
        reopened
            .attempts
            .iter()
            .map(|a| &a.state)
            .collect::<Vec<_>>(),
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .run_producer(&run)
            .is_ok()
    );
    assert_eq!(reopened.outcome, stopped.outcome);
    let host = f.host.owner.upgrade().unwrap();
    let run_auth = f.producer(&run);
    let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &claim_root,
        execution.project.as_str(),
        run.0.as_str(),
    )
    .unwrap();
    crate::effect_pump::reconcile_attempt(&host, &mut claim, &run_auth, &execution).unwrap();
    claim.finish().unwrap();
    drop(host);
    let settled = f.projection(&run);
    assert_eq!(
        settled.cleanup,
        RunCleanup::Settled,
        "C5_FIX14_RESTART_SETTLED"
    );
    let before = C5Fix14Before::capture(&f, settled, manual);
    let result = c5_fix13_archive(&f, &run, &execution);
    assert!(
        result.is_ok(),
        "C5_FIX14_RESTART_ARCHIVE_REFUSED {result:?}"
    );
    c5_fix13_after_archive(f, &before, &claim_root);
}
