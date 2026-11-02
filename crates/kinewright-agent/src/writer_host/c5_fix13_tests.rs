/// C5 fix 13: Stop and settle a run, as the maximal fixture does after its
/// run-wide limit. Returns the settled projection.
fn c5_fix13_stop_settle(
    f: &Fixture,
    run: &RunId,
    execution: &ExecutionIdentity,
    claim_root: &std::path::Path,
) -> RunProjection {
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
    let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
        claim_root,
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
    settled
}

/// Archive the run's retired attempts and republish, as the maximal fixture does.
fn c5_fix13_archive(
    f: &Fixture,
    run: &RunId,
    execution: &ExecutionIdentity,
) -> Result<(), HostError> {
    let run_auth = f.producer(run);
    let host = f.host.owner.upgrade().unwrap();
    let mut owner = host.lock().unwrap();
    let mut state = owner.spending_scope(&run_auth, run).unwrap();
    owner.archive_retired_attempts(&mut state).unwrap();
    owner.publish_native(&run_auth, state, execution, vec![])
}

/// One accounting record for the run, so its spending can be compared.
fn c5_fix13_ceiling(f: &Fixture, run: &RunId, execution: &ExecutionIdentity) {
    use super::super::native_invocation::SpendingChange;
    let now = native_now(f, run);
    spending_change(f, execution, SpendingChange::Ceiling(100), now, 0, true).unwrap();
}

/// C5 fix 14: the run's sealed attempt archive, read the way the host reads it.
fn c5_fix14_archive(f: &Fixture, state: &RunProjection) -> Vec<AttemptReservation> {
    let host = f.host.owner.upgrade().unwrap();
    let owner = host.lock().unwrap();
    let store = owner.owner().unwrap().store().unwrap();
    for reference in state.usage.iter().rev() {
        if let OrchestrationPayload::AttemptArchive(archive) = &store
            .read_record::<OrchestrationRecord>(reference)
            .unwrap()
            .payload
        {
            assert_eq!(archive.run, state.run);
            return archive.attempts.clone();
        }
    }
    vec![]
}

/// C5 fix 14: a manual history with one undone edit, so redo state is kept too.
fn c5_fix14_seed_manual(f: &Fixture) -> kinewright_core::HistorySnapshot {
    let host = f.host.owner.upgrade().unwrap();
    let mut owner = host.lock().unwrap();
    for id in [60, 61] {
        owner
            .core()
            .request(kinewright_core::Command::Do(history_marker(id)))
            .unwrap();
    }
    owner
        .core()
        .request(kinewright_core::Command::Undo)
        .unwrap();
    owner.owner_mut().unwrap().save_active().unwrap();
    owner.core().export_history().unwrap()
}

fn c5_fix14_manual(f: &Fixture) -> kinewright_core::HistorySnapshot {
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .core()
        .export_history()
        .unwrap()
}

/// What the archive must keep: the settled run's attempts, its existing
/// archive, its complete accounting and the manual history.
struct C5Fix14Before {
    settled: RunProjection,
    prefix: Vec<AttemptReservation>,
    accounting: kinewright_project::orchestration_accounting::AccountingRecord,
    manual: kinewright_core::HistorySnapshot,
}

impl C5Fix14Before {
    fn capture(
        f: &Fixture,
        settled: RunProjection,
        manual: kinewright_core::HistorySnapshot,
    ) -> Self {
        Self {
            prefix: c5_fix14_archive(f, &settled),
            accounting: spending_record(f, &settled.run),
            settled,
            manual,
        }
    }
    /// The archive holds the previous archive followed by every retired
    /// attempt, exactly; nothing else changed.
    fn check(&self, f: &Fixture, marker: &str) {
        let run = &self.settled.run;
        let archived = f.projection(run);
        assert!(
            archived.attempts.is_empty(),
            "C5_FIX13_ATTEMPTS_ARCHIVED {marker}"
        );
        assert_eq!(archived.cleanup, RunCleanup::Settled);
        let mut expected = self.prefix.clone();
        expected.extend(self.settled.attempts.iter().cloned());
        assert_eq!(
            c5_fix14_archive(f, &archived),
            expected,
            "C5_FIX14_ARCHIVE_EXACT {marker}"
        );
        assert_eq!(
            spending_record(f, run),
            self.accounting,
            "C5_FIX13_NO_SPENDING_AFTER_STOP {marker}"
        );
        assert_eq!(
            c5_fix14_manual(f),
            self.manual,
            "C5_FIX14_MANUAL_EXACT {marker}"
        );
    }
}

/// After the archive: no attempt left, the archive exact, nothing charged, no
/// new send admitted, and the archived state reopens exactly.
fn c5_fix13_after_archive(f: Fixture, before: &C5Fix14Before, claim_root: &std::path::Path) {
    let run = &before.settled.run;
    before.check(&f, "archive");
    let archived = f.projection(run);
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
    let f = c4_restart(f, claim_root);
    assert_eq!(f.projection(run), archived, "C5_FIX13_REOPEN_EXACT");
    before.check(&f, "reopen");
}

#[test]
fn s4b_c5_fix13_run_at_its_limit_stops_settles_and_archives() {
    let f = c5_fix1_limited_fixture();
    c5_fix1_occupy(&f, 80);
    let manual = c5_fix14_seed_manual(&f);
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
    let claim_root = f.temp.path("c5-fix13-machine");
    let settled = c5_fix13_stop_settle(&f, &run, &execution, &claim_root);
    let before = C5Fix14Before::capture(&f, settled, manual);
    let result = c5_fix13_archive(&f, &run, &execution);
    assert!(
        result.is_ok(),
        "C5_FIX13_SETTLED_ARCHIVE_REFUSED {result:?}"
    );
    c5_fix13_after_archive(f, &before, &claim_root);
}

#[test]
fn s4b_c5_fix13_settled_run_below_its_limit_archives() {
    let f = Fixture::new();
    f.host.runtime.stop();
    let manual = c5_fix14_seed_manual(&f);
    let (run, execution, _) = reserve(&f);
    c5_fix13_ceiling(&f, &run, &execution);
    let claim_root = f.temp.path("c5-fix13-machine");
    let settled = c5_fix13_stop_settle(&f, &run, &execution, &claim_root);
    let before = C5Fix14Before::capture(&f, settled, manual);
    let result = c5_fix13_archive(&f, &run, &execution);
    assert!(
        result.is_ok(),
        "C5_FIX13_SETTLED_ARCHIVE_REFUSED {result:?}"
    );
    c5_fix13_after_archive(f, &before, &claim_root);
}
