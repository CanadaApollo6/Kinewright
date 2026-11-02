/// C5 fix 13: Stop and settle a run, then archive its retired attempts and
/// republish, as the maximal fixture does after its run-wide limit.
/// Returns the archive publication's result and the state before it.
fn c5_fix13_stop_settle_archive(
    f: &Fixture,
    run: &RunId,
    execution: &ExecutionIdentity,
) -> (Result<(), HostError>, RunProjection) {
    let run_auth = f.producer(run);
    let host = f.host.owner.upgrade().unwrap();
    let stop = f.control(&f.editor, run, RunAction::Stop);
    ok(
        f.editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(stop),
            })
            .unwrap(),
        "C5_FIX13_STOP",
    );
    let claim_root = f.temp.path("c5-fix13-machine");
    let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &claim_root,
        execution.project.as_str(),
        run.0.as_str(),
    )
    .unwrap();
    crate::effect_pump::reconcile_attempt(&host, &mut claim, &run_auth, execution).unwrap();
    claim.finish().unwrap();
    let settled = f.projection(run);
    assert_eq!(settled.cleanup, RunCleanup::Settled, "C5_FIX13_SETTLED");
    assert!(
        !settled.attempts.is_empty()
            && settled
                .attempts
                .iter()
                .all(|a| matches!(a.state, AttemptState::Retired(_))),
        "C5_FIX13_RETIRED_BEFORE_ARCHIVE"
    );
    let mut owner = host.lock().unwrap();
    let mut state = owner.spending_scope(&run_auth, run).unwrap();
    owner.archive_retired_attempts(&mut state).unwrap();
    let result = owner.publish_native(&run_auth, state, execution, vec![]);
    (result, settled)
}

/// One accounting record for the run, so its spending can be compared.
fn c5_fix13_ceiling(f: &Fixture, run: &RunId, execution: &ExecutionIdentity) {
    use super::super::native_invocation::SpendingChange;
    let now = native_now(f, run);
    spending_change(f, execution, SpendingChange::Ceiling(100), now, 0, true).unwrap();
}

/// After the archive: no attempt left, nothing charged, no new send admitted,
/// and the archived state reopens exactly.
fn c5_fix13_after_archive(
    f: Fixture,
    run: &RunId,
    accounting: &kinewright_project::orchestration_accounting::AccountingRecord,
) {
    let archived = f.projection(run);
    assert!(archived.attempts.is_empty(), "C5_FIX13_ATTEMPTS_ARCHIVED");
    assert_eq!(archived.cleanup, RunCleanup::Settled);
    assert_eq!(
        spending_record(&f, run).tasks,
        accounting.tasks,
        "C5_FIX13_NO_SPENDING_AFTER_STOP"
    );
    let run_auth = f.producer(run);
    let reserved = f
        .host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .reserve_native_effect(&run_auth, run);
    assert!(
        !matches!(reserved, Ok(Some(_))),
        "C5_FIX13_STOPPED_RUN_RESERVES_NO_SEND"
    );
    let claim_root = f.temp.path("c5-fix13-machine");
    let f = c4_restart(f, &claim_root);
    assert_eq!(f.projection(run), archived, "C5_FIX13_REOPEN_EXACT");
    assert_eq!(
        spending_record(&f, run).tasks,
        accounting.tasks,
        "C5_FIX13_REOPEN_ACCOUNTING"
    );
}

#[test]
fn s4b_c5_fix13_run_at_its_limit_stops_settles_and_archives() {
    let f = c5_fix1_limited_fixture();
    c5_fix1_occupy(&f, 80);
    let (run, execution, _) = reserve(&f);
    c5_fix13_ceiling(&f, &run, &execution);
    let run_auth = f.producer(&run);
    let host = f.host.owner.upgrade().unwrap();
    let mut refused = false;
    for _ in 0..128 {
        let mut owner = host.lock().unwrap();
        let state = owner.spending_scope(&run_auth, &run).unwrap();
        if let Err(error) = owner.publish_native(&run_auth, state, &execution, vec![]) {
            assert!(
                error.to_string().contains("Run limit"),
                "C5_FIX13_LIMIT_REASON {error}"
            );
            refused = true;
            break;
        }
    }
    drop(host);
    assert!(refused, "C5_FIX13_RUN_LIMIT_REACHED");
    let accounting = spending_record(&f, &run);
    let (result, _) = c5_fix13_stop_settle_archive(&f, &run, &execution);
    assert!(
        result.is_ok(),
        "C5_FIX13_SETTLED_ARCHIVE_REFUSED {result:?}"
    );
    c5_fix13_after_archive(f, &run, &accounting);
}

#[test]
fn s4b_c5_fix13_settled_run_below_its_limit_archives() {
    let f = Fixture::new();
    f.host.runtime.stop();
    let (run, execution, _) = reserve(&f);
    c5_fix13_ceiling(&f, &run, &execution);
    let accounting = spending_record(&f, &run);
    let (result, _) = c5_fix13_stop_settle_archive(&f, &run, &execution);
    assert!(
        result.is_ok(),
        "C5_FIX13_SETTLED_ARCHIVE_REFUSED {result:?}"
    );
    c5_fix13_after_archive(f, &run, &accounting);
}
