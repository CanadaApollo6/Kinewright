#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c3_fix1_release_after_six_hour_deadline() {
    use kinewright_project::orchestration_accounting::RUN_MILLIS;
    let f = Fixture::new();
    let (run, execution, _) = reserve(&f);
    let admitted = native_admitted(&f, &execution);
    let (profile, checks, scratch) = native_profile(&f, &admitted);
    let now = super::super::spending::unix_ms().unwrap();
    let deadline = now + 2000;
    let producer = f.producer(&run);
    {
        let host = f.host.owner.upgrade().unwrap();
        let mut owner = host.lock().unwrap();
        let mut state = owner.spending_scope(&producer, &run).unwrap();
        let mut record = owner.native_accounting(&state).unwrap().unwrap();
        record.start_unix_ms = deadline - RUN_MILLIS;
        record.deadline_unix_ms = deadline;
        record.last_unix_ms = now;
        record.elapsed_ms = RUN_MILLIS - 2000;
        let reference = owner
            .owner_mut()
            .unwrap()
            .admit_scheduler_record(&OrchestrationRecord {
                version: 1,
                payload: OrchestrationPayload::Accounting(Box::new(record)),
            })
            .unwrap()
            .reference;
        owner
            .replace_accounting_reference(&mut state, reference)
            .unwrap();
        owner
            .publish_native(&producer, state, &execution, vec![])
            .unwrap();
    }
    let invocation = native_charge(&f, &admitted, profile);
    let charged_at = super::super::spending::unix_ms().unwrap();
    assert!(charged_at < deadline);
    let barrier: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        let now = super::super::spending::unix_ms().unwrap();
        std::thread::sleep(Duration::from_millis(deadline.saturating_sub(now) + 100));
    });
    let (result, claim) =
        native_start_barrier(&f, admitted, invocation, checks, false, Some(barrier));
    let released_at = super::super::spending::unix_ms().unwrap();
    let mut ran = false;
    if let Ok(mut session) = result {
        wait_native(&scratch.join("execution-ready"));
        ran = true;
        session.interrupt();
        drop(session);
    }
    finish_stopped_native(claim);
    std::fs::remove_dir_all(scratch).unwrap();
    eprintln!(
        "ASTRA_DEADLINE charged_at={charged_at} deadline={deadline} released_at={released_at} executed={ran}"
    );
    assert!(!ran, "ASTRA_NO_PROVIDER_RELEASE_AFTER_SIX_HOURS");
}

#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c3_fix1_commit_rechecks_monotonic_and_usage() {
    use super::super::native_invocation::SpendingChange;
    use crate::protocol::limits::usage::ClockObservation;
    use kinewright_project::orchestration_accounting::{UsageMode, UsageSample, UsageValues};
    for mode in ["monotonic", "tokens", "unknown", "clock"] {
        let f = Fixture::new();
        let (run, execution, _) = reserve(&f);
        let admitted = native_admitted(&f, &execution);
        let (profile, checks, scratch) = native_profile(&f, &admitted);
        let producer = f.producer(&run);
        if matches!(mode, "tokens" | "unknown") {
            f.host
                .owner
                .upgrade()
                .unwrap()
                .lock()
                .unwrap()
                .change_spending(
                    &producer,
                    &execution,
                    SpendingChange::Ceiling(100),
                    ClockObservation {
                        unix_ms: super::super::spending::unix_ms().unwrap(),
                        monotonic_delta_ms: 0,
                        trusted: true,
                    },
                )
                .unwrap();
        }
        let mut invocation = native_charge(&f, &admitted, profile);
        if mode == "monotonic" {
            invocation.expire_for_test();
        }
        let host = f.host.owner.upgrade().unwrap();
        let identity = execution.clone();
        let barrier = Arc::new(move || {
            if mode == "monotonic" {
                return;
            }
            let mut owner = host.lock().unwrap();
            let clock = ClockObservation {
                unix_ms: super::super::spending::unix_ms().unwrap(),
                monotonic_delta_ms: 0,
                trusted: mode != "clock",
            };
            if mode == "clock" {
                let mut state = owner.spending_scope(&producer, &identity.run).unwrap();
                let previous = owner.native_accounting(&state).unwrap().unwrap();
                owner
                    .owner_mut()
                    .unwrap()
                    .admit_scheduler_record(&OrchestrationRecord {
                        version: 1,
                        payload: OrchestrationPayload::AccountingArchive(Box::new(
                            previous.archive().unwrap(),
                        )),
                    })
                    .unwrap();
                let mut next = previous.successor().unwrap();
                next.clock_uncertain = true;
                let reference = owner
                    .owner_mut()
                    .unwrap()
                    .admit_scheduler_record(&OrchestrationRecord {
                        version: 1,
                        payload: OrchestrationPayload::Accounting(Box::new(next)),
                    })
                    .unwrap()
                    .reference;
                owner
                    .replace_accounting_reference(&mut state, reference)
                    .unwrap();
                owner
                    .publish_native(&producer, state, &identity, vec![])
                    .unwrap();
                return;
            }
            let change = {
                SpendingChange::Usage(UsageSample {
                    attempt: identity.attempt.0.clone(),
                    turn: 1,
                    event: "late-usage-before-commit".into(),
                    ordinal: 1,
                    mode: UsageMode::Cumulative,
                    complete: mode == "tokens",
                    input_includes_cache: true,
                    output_includes_reasoning: true,
                    values: if mode == "tokens" {
                        UsageValues {
                            input: Some(100),
                            output: Some(0),
                            cache_read: Some(0),
                            cache_write: Some(0),
                            reasoning: Some(0),
                            cost_micros: None,
                        }
                    } else {
                        UsageValues::default()
                    },
                })
            };
            owner
                .change_spending(&producer, &identity, change, clock)
                .unwrap();
        });
        let (result, claim) =
            native_start_barrier(&f, admitted, invocation, checks, false, Some(barrier));
        let refused = result.is_err();
        if let Ok(mut session) = result {
            session.interrupt();
            drop(session);
        }
        finish_stopped_native(claim);
        assert!(
            refused && !scratch.join("execution-ready").exists(),
            "C3_COMMIT_RELEASE_BUDGET {mode}"
        );
        let state = f.projection(&run);
        let host = f.host.owner.upgrade().unwrap();
        let owner = host.lock().unwrap();
        assert_eq!(
            owner.native_accounting(&state).unwrap().unwrap().tasks[0]
                .sends
                .len(),
            1
        );
        assert!(
            state
                .attempts
                .iter()
                .all(|a| matches!(a.state, AttemptState::Contained | AttemptState::Retired(_))),
            "C3_COMMIT_REFUSAL_RETAINS_H1_CLEANUP_OBLIGATION"
        );
        std::fs::remove_dir_all(scratch).unwrap();
    }
}

struct HistoryCrashOwner {
    inner: super::super::native_invocation::HostInvocationOwner,
    point: &'static str,
}
impl crate::native_launcher::InvocationOwner for HistoryCrashOwner {
    fn reserve(
        &self,
        v: &crate::orchestration::adapters::AuthorizedInvocation,
    ) -> std::io::Result<()> {
        self.inner.reserve(v)
    }
    fn release(
        &self,
        v: &crate::orchestration::adapters::AuthorizedInvocation,
        r: &crate::job_guardian::protocol::RecoveryIdentity,
        c: &crate::guardian::StartupControl,
    ) -> std::io::Result<()> {
        self.inner.release(v, r, c)
    }
    fn commit(
        &self,
        v: &crate::orchestration::adapters::AuthorizedInvocation,
        r: &crate::job_guardian::protocol::RecoveryIdentity,
        s: &mut dyn FnMut() -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        self.inner.commit(v, r, s)
    }
    fn started(&self, i: &ExecutionIdentity) -> std::io::Result<()> {
        self.inner.started(i)
    }
    fn raw(
        &self,
        i: &ExecutionIdentity,
        v: &serde_json::Value,
        n: u64,
        e: bool,
    ) -> std::io::Result<()> {
        self.inner.raw(i, v, n, e)
    }
    fn completed(
        &self,
        i: &ExecutionIdentity,
        c: bool,
        p: &crate::guardian::CleanupProof,
    ) -> std::io::Result<()> {
        kinewright_project::io_observer::at_barrier(
            self.point,
            || std::process::exit(86),
            || self.inner.completed(i, c, p),
        )
    }
    fn steering(&self, i: &ExecutionIdentity, t: &str) -> std::io::Result<()> {
        self.inner.steering(i, t)
    }
}

#[test]
fn s4b_c3_fix1_history_crash_child() {
    use kinewright_core::{AgentDriver, Command};
    let Some(handoff) = std::env::var_os("KINEWRIGHT_C3_HISTORY_HANDOFF") else {
        return;
    };
    let path = std::env::var("KINEWRIGHT_C3_HISTORY_PATH").unwrap();
    let point = match std::env::var("KINEWRIGHT_C3_HISTORY_CUT").unwrap().as_str() {
        "staged" => "task_checkpoint_staged",
        "before" => "task_checkpoint_before_publish",
        "after" => "task_checkpoint_after_publish",
        _ => panic!("unknown history cut"),
    };
    let f = Fixture::new();
    let manual = {
        let host = f.host.owner.upgrade().unwrap();
        let mut owner = host.lock().unwrap();
        owner
            .core()
            .request(Command::Do(history_marker(50)))
            .unwrap();
        owner
            .core()
            .request(Command::Do(history_marker(51)))
            .unwrap();
        owner.core().request(Command::Undo).unwrap();
        owner.owner_mut().unwrap().save_active().unwrap();
        owner.core().export_history().unwrap()
    };
    let start = f.start();
    let run: RunId = start.envelope.identity("run").unwrap();
    let (identity, _, _, core) = history_task_endpoint(&f, &run);
    for id in [1, 2] {
        core.request(Command::Do(history_marker(id))).unwrap();
    }
    core.request(Command::Undo).unwrap();
    let before = core.export_history().unwrap();
    let host = f.host.owner.upgrade().unwrap();
    let producer = f.producer(&run);
    host.lock()
        .unwrap()
        .checkpoint_task_history(&producer, &run, &identity.task)
        .unwrap();
    core.request(Command::Do(history_marker(3))).unwrap();
    core.request(Command::Undo).unwrap();
    let after = core.export_history().unwrap();
    assert!(
        !before.cursor.redo.is_empty()
            && !after.cursor.redo.is_empty()
            && !manual.cursor.redo.is_empty()
    );
    let machine = f.temp.path("history-machine");
    let claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &machine,
        identity.project.as_str(),
        run.0.as_str(),
    )
    .unwrap();
    let mut scratch = None;
    if path == "completion" {
        let admitted = native_admitted(&f, &identity);
        let (profile, checks, dir) = native_profile(&f, &admitted);
        scratch = Some(dir.clone());
        let invocation = native_charge(&f, &admitted, profile);
        let claim = Arc::new(Mutex::new(claim));
        let launcher = crate::native_launcher::NativeGuardianLauncher {
            claim,
            owner: Arc::new(HistoryCrashOwner {
                inner: super::super::native_invocation::HostInvocationOwner::new(&host, producer),
                point,
            }),
            guardian: kinewright_media::test_support::workspace_target_dir()
                .join("debug/kinewright-job-guardian"),
            checks: Arc::new(checks),
            lose_commit_ack: false,
            after_release: None,
            startup_failure: None,
        };
        std::fs::write(&handoff,serde_json::to_vec(&serde_json::json!({"root":f.temp.root(),"machine":machine,"identity":identity,"before":before,"after":after,"manual":manual,"scratch":scratch})).unwrap()).unwrap();
        let config = invocation.profile().session_config().clone();
        let driver = crate::orchestration::adapters::ContainedDriver::new(
            launcher,
            admitted,
            invocation,
            crate::ScriptedDriver::new(vec![]).detect().unwrap(),
        );
        let _session = driver.start_session(config).unwrap();
        wait_native(&dir.join("execution-ready"));
        std::fs::write(dir.join("exit"), b"").unwrap();
        std::thread::sleep(Duration::from_secs(15));
        panic!("history completion crash cut not reached");
    }
    std::fs::write(&handoff,serde_json::to_vec(&serde_json::json!({"root":f.temp.root(),"machine":machine,"identity":identity,"before":before,"after":after,"manual":manual,"scratch":scratch})).unwrap()).unwrap();
    kinewright_project::io_observer::at_barrier(
        point,
        || std::process::exit(86),
        || match path.as_str() {
            "checkpoint" => {
                host.lock()
                    .unwrap()
                    .checkpoint_task_history(&producer, &run, &identity.task)
                    .unwrap();
            }
            "freeze" => {
                host.lock()
                    .unwrap()
                    .freeze_task_history(&producer, &run, &identity.task)
                    .unwrap();
            }
            "recovery" => {
                crate::effect_pump::recover_run(&host, claim, &run).unwrap();
            }
            _ => panic!("unknown history path"),
        },
    );
    panic!("history crash cut not reached");
}

#[test]
#[ignore = "on-demand: equipped Linux guardian qualification"]
fn s4b_c3_fix1_history_atomic_crash_cuts() {
    for path in ["checkpoint", "freeze", "completion", "recovery"] {
        for cut in ["staged", "before", "after"] {
            let handoff = kinewright_media::test_support::TempDirectory::new_in(
                &kinewright_media::test_support::workspace_target_dir(),
                "c3-history-handoff",
            );
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "writer_host::hooks_tests::tests::s4b_c3_fix1_history_crash_child",
                    "--nocapture",
                ])
                .env(
                    "KINEWRIGHT_C3_HISTORY_HANDOFF",
                    handoff.path("handoff.json"),
                )
                .env("KINEWRIGHT_C3_HISTORY_PATH", path)
                .env("KINEWRIGHT_C3_HISTORY_CUT", cut)
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(86),
                "C3_HISTORY_CRASH_CUT {path}/{cut}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let data: serde_json::Value =
                serde_json::from_slice(&std::fs::read(handoff.path("handoff.json")).unwrap())
                    .unwrap();
            let root: std::path::PathBuf = serde_json::from_value(data["root"].clone()).unwrap();
            let machine: std::path::PathBuf =
                serde_json::from_value(data["machine"].clone()).unwrap();
            let identity: ExecutionIdentity =
                serde_json::from_value(data["identity"].clone()).unwrap();
            let expected: kinewright_core::HistorySnapshot = serde_json::from_value(
                data[if cut == "after" { "after" } else { "before" }].clone(),
            )
            .unwrap();
            let manual: kinewright_core::HistorySnapshot =
                serde_json::from_value(data["manual"].clone()).unwrap();
            let writer = WriterHost::start(HostConfig {
                scheduler_settings: None,
                project: "hooks".into(),
                path: Some(root.join("project.kinewright")),
                roots: SessionRoots {
                    read: vec![root.clone()],
                    write: vec![root.clone()],
                },
                origin: HostOrigin::Headless,
            })
            .unwrap();
            let host = writer.owner.upgrade().unwrap();
            let claim = crate::execution_claim::MachineClaim::recover_fixture(&machine).unwrap();
            if path == "completion" && cut == "after" {
                let mut owner = host.lock().unwrap();
                let (producer, mut state) = owner.restore_native_run(&identity.run).unwrap();
                owner.archive_retired_attempts(&mut state).unwrap();
                assert!(state.attempts.is_empty());
                owner
                    .publish_native(&producer, state, &identity, vec![])
                    .unwrap();
            }
            let (claim, producer) =
                crate::effect_pump::recover_run(&host, claim, &identity.run).unwrap();
            {
                let mut owner = host.lock().unwrap();
                let state = owner
                    .owner()
                    .unwrap()
                    .orchestration_projection(&identity.run)
                    .unwrap()
                    .unwrap();
                let saved = owner
                    .saved_task_history(&state, &identity.task)
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    owner
                        .owner()
                        .unwrap()
                        .candidate_binding(&saved.source.branch)
                        .unwrap(),
                    saved.source,
                    "C3_ATOMIC_HISTORY_BINDING {path}/{cut}"
                );
                owner.restore_task_candidate(&saved).unwrap();
                let guard = owner
                    .owner_mut()
                    .unwrap()
                    .reopen_registered_candidate(saved.source.branch.clone(), &saved.base_ref)
                    .unwrap();
                assert_eq!(
                    guard.core().export_history().unwrap(),
                    expected,
                    "C3_ATOMIC_HISTORY_EXACT_REDO {path}/{cut}"
                );
                assert_eq!(
                    owner.core().export_history().unwrap(),
                    manual,
                    "C3_ATOMIC_MANUAL_HISTORY {path}/{cut}"
                );
                assert_eq!(saved.frozen, path == "freeze" && cut == "after");
                if path == "completion" {
                    assert_eq!(
                        owner.native_accounting(&state).unwrap().unwrap().tasks[0]
                            .sends
                            .len(),
                        1
                    );
                } else {
                    let (admitted, _, _) = owner
                        .reserve_native_effect(&producer, &identity.run)
                        .unwrap()
                        .unwrap();
                    let next = admitted.identity();
                    owner
                        .prepare_task_endpoint(
                            &producer,
                            &identity.run,
                            &next.task,
                            &next.attempt,
                            Duration::from_secs(300),
                        )
                        .unwrap_or_else(|e| {
                            panic!("C3_ATOMIC_HISTORY_CONTINUATION {path}/{cut}: {e}")
                        });
                    assert_eq!(
                        owner.hooks.runs[&identity.run].tasks[&next.task]
                            .core()
                            .export_history()
                            .unwrap(),
                        expected
                    );
                }
            }
            claim.finish().unwrap();
            drop(host);
            drop(writer);
            if let Some(scratch) = data["scratch"].as_str() {
                std::fs::remove_dir_all(scratch).unwrap();
            }
            std::fs::remove_dir_all(root).unwrap();
            eprintln!("C3_ATOMIC_HISTORY_RECOVERED {path}/{cut}");
        }
    }
}

#[test]
fn s4b_c3_fix1_history_lost_ack_reconciles_original_cut() {
    use kinewright_core::Command;
    let f = Fixture::new();
    let start = f.start();
    let run: RunId = start.envelope.identity("run").unwrap();
    let (identity, _, _, core) = history_task_endpoint(&f, &run);
    for id in [1, 2] {
        core.request(Command::Do(history_marker(id))).unwrap();
    }
    core.request(Command::Undo).unwrap();
    let expected = core.export_history().unwrap();
    let producer = f.producer(&run);
    let host = f.host.owner.upgrade().unwrap();
    let mut owner = host.lock().unwrap();
    let refused = kinewright_project::io_observer::fail_nth("root_parent_sync", 2, || {
        owner.checkpoint_task_history(&producer, &run, &identity.task)
    });
    assert!(refused.is_err(), "C3_HISTORY_H1_ACK_CUT");
    let Some(super::super::orchestration::PendingPublication::Owner(original)) =
        owner.hooks.pending.clone()
    else {
        panic!("original H1 cut retained");
    };
    owner
        .publish_orchestration_transition(&producer, &original, None)
        .unwrap();
    let saved = owner
        .checkpoint_task_history(&producer, &run, &identity.task)
        .unwrap();
    assert_eq!(
        owner
            .owner()
            .unwrap()
            .candidate_binding(&saved.source.branch)
            .unwrap(),
        saved.source,
        "C3_HISTORY_ORIGINAL_ACK_BINDING"
    );
    assert_eq!(core.export_history().unwrap(), expected);
    let again = owner
        .checkpoint_task_history(&producer, &run, &identity.task)
        .unwrap();
    assert_eq!(
        saved, again,
        "C3_HISTORY_RECONCILIATION_PRESERVES_CHECKPOINT"
    );
}
