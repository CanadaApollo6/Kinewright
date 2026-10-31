fn c5e_depth(f: &Fixture, maximum: &mut u64) {
    let host = f.host.owner.upgrade().unwrap();
    let depth = host
        .lock()
        .unwrap()
        .owner()
        .unwrap()
        .store()
        .unwrap()
        .maximum_committed_depth()
        .unwrap();
    *maximum = (*maximum).max(depth);
    assert!(*maximum <= 16, "C5E_CONSTANT_GRAPH_DEPTH {maximum}");
}
fn c5e_history(f: &Fixture, run: &RunId, task: &TaskId) -> kinewright_core::HistorySnapshot {
    let host = f.host.owner.upgrade().unwrap();
    let owner = host.lock().unwrap();
    let state = owner
        .owner()
        .unwrap()
        .orchestration_projection(run)
        .unwrap()
        .unwrap();
    let saved = owner.saved_task_history(&state, task).unwrap().unwrap();
    let store = owner.owner().unwrap().store().unwrap();
    let binding = owner
        .owner()
        .unwrap()
        .candidate_binding(&saved.source.branch)
        .unwrap();
    let checkpoint = store
        .read_record::<kinewright_project::bundle_records::BranchCheckpoint>(&binding.checkpoint)
        .unwrap();
    (*store
        .read_record::<kinewright_core::HistorySnapshot>(&checkpoint.history_ref)
        .unwrap())
    .clone()
}
fn c5e_send(f: &Fixture, execution: &ExecutionIdentity, turn: u64) {
    use kinewright_project::orchestration_accounting::SendConsumption;
    let producer = f.producer(&execution.run);
    let host = f.host.owner.upgrade().unwrap();
    let mut owner = host.lock().unwrap();
    let mut state = owner.spending_scope(&producer, &execution.run).unwrap();
    let previous = owner.native_accounting(&state).unwrap().unwrap();
    let mut next = previous.successor().unwrap();
    if !next
        .task_mut(&execution.task.0)
        .unwrap()
        .providers
        .iter()
        .any(|p| p.attempt == execution.attempt.0)
    {
        use kinewright_project::orchestration_accounting::{ProviderMetadata, ProviderSelection};
        let selection = ProviderSelection {
            connector: "scripted".into(),
            model: "fixture".into(),
            options: vec![],
        };
        next.task_mut(&execution.task.0)
            .unwrap()
            .select_provider(ProviderMetadata {
                attempt: execution.attempt.0.clone(),
                requested: selection.clone(),
                resolved: selection,
                cli_version: "fixture-1".into(),
            })
            .unwrap();
    }
    let index = owner.resolve_attachment(Some(&producer.bearer())).unwrap();
    let issued = owner.issue_metadata(index, None).unwrap();
    let payload = state
        .tasks
        .iter()
        .find(|t| t.id == execution.task)
        .unwrap()
        .brief
        .assignment
        .clone();
    next.task_mut(&execution.task.0)
        .unwrap()
        .consume_send(SendConsumption {
            command: issued.envelope.id.clone(),
            attempt: execution.attempt.0.clone(),
            turn,
            payload: payload.clone(),
        })
        .unwrap();
    next.validate_successor(&previous).unwrap();
    let record = owner
        .owner_mut()
        .unwrap()
        .admit_scheduler_record(&OrchestrationRecord {
            version: 1,
            payload: OrchestrationPayload::Accounting(Box::new(next)),
        })
        .unwrap();
    owner
        .replace_accounting_reference(&mut state, record.reference)
        .unwrap();
    let digest = kinewright_media::sha256_bytes(
        &serde_json::to_vec(&("c5e-send", &issued.envelope.id)).unwrap(),
    );
    let lane = state
        .tasks
        .iter()
        .find(|t| t.id == execution.task)
        .unwrap()
        .brief
        .scope
        .base
        .branch
        .clone();
    state.effects.push(EffectRecord {
        id: serde_json::from_value(serde_json::json!(&digest[..32])).unwrap(),
        task: execution.task.clone(),
        lane,
        order: state.effects.iter().map(|e| e.order).max().unwrap() + 1,
        safety: EffectSafety::ProcessBound,
        state: EffectState::Settled,
        payload,
    });
    let result = owner.publish_native_issued(&producer, state, execution, vec![], issued);
    assert!(result.is_ok(), "C5E_SEND_PUBLICATION {turn}: {result:?}");
}

#[test]
#[ignore = "on-demand: maximal six-hour durable run"]
#[allow(
    clippy::too_many_lines,
    reason = "one maximal run retains its exact histories through Stop and reopen"
)]
fn s4b_c5e_maximal_run_keeps_depth_stop_and_recovery_bounded() {
    use super::super::native_invocation::SpendingChange;
    use kinewright_project::{
        orchestration_accounting::{RUN_MILLIS, UsageMode, UsageSample, UsageValues},
        run_admission,
    };
    let mut f = Fixture::new();
    f.host.runtime.stop();
    let mut scoped = Document::default();
    for id in [1, 2] {
        if let kinewright_core::Operation::AddMarker { marker } = history_marker(id) {
            scoped.markers.push(marker);
        }
    }
    f.brief.scope =
        crate::orchestration::policy::director_scope(&scoped, f.brief.scope.base.clone()).unwrap();
    let manual = {
        let host = f.host.owner.upgrade().unwrap();
        let mut owner = host.lock().unwrap();
        for id in [50, 51] {
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
    };
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .owner_mut()
        .unwrap()
        .admit_host_grant(b"hook plan, script and assignment")
        .unwrap();
    let (run, parent, _) = reserve(&f);
    let run_auth = f.producer(&run);
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .prepare_task_endpoint(
            &run_auth,
            &run,
            &parent.task,
            &parent.attempt,
            Duration::from_secs(3600),
        )
        .unwrap();
    let director = C4Mcp::new(&f, &run, &parent.task);
    let now = native_now(&f, &run);
    let mut maximum = 0;
    let sequence = f.projection(&run).sequence;
    for tick in 1..720 {
        assert!(
            spending_change(
                &f,
                &parent,
                SpendingChange::Clock,
                now + tick * 30_000,
                30_000,
                true
            )
            .is_ok(),
            "C5_FIX1_CLOCK {tick}"
        );
        assert_eq!(
            f.projection(&run).sequence,
            sequence,
            "C5_FIX1_CLOCK_REPLACES"
        );
        if tick % 120 == 0 {
            eprintln!("C5_FIX1_CLOCK {tick}");
            c5e_depth(&f, &mut maximum);
        }
    }
    let claim_root = f.temp.path("c5-fix1-machine");
    let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &claim_root,
        parent.project.as_str(),
        run.0.as_str(),
    )
    .unwrap();
    let mut all_attempts = vec![];
    let mut histories = vec![];
    let mut last = parent.clone();
    // Seven serial specialists fit the three simultaneous dependency slots.
    // One task reaches its 80-send ceiling; the run reaches its shared 128-send ceiling.
    for task_index in 0..8 {
        let task = if task_index == 7 {
            parent.task.clone()
        } else {
            let mut brief = f.brief.clone();
            brief.role = if task_index == 0 {
                Role::Editor
            } else {
                Role::Reviewer
            };
            if brief.role == Role::Reviewer {
                brief.scope.operations.clear();
            }
            director.accepted(
                "assign_task",
                OrchestrationCommand::AssignTask(Box::new(AssignTask {
                    run: run.clone(),
                    parent: parent.task.clone(),
                    brief,
                })),
            );
            f.projection(&run).tasks.last().unwrap().id.clone()
        };
        let sends: usize = if task_index == 7 {
            80
        } else if task_index == 6 {
            6
        } else {
            7
        };
        let mut history = None;
        for round in 0..3 {
            let (execution, client, endpoint, core) = if task_index == 7 && round == 0 {
                let (client, endpoint) = task_actor(&f, &run, &parent, &f.producer_client(&run));
                let core = f.host.owner.upgrade().unwrap().lock().unwrap().hooks.runs[&run].tasks
                    [&task]
                    .core()
                    .clone();
                (parent.clone(), client, endpoint, core)
            } else {
                c4_reserve_task(&f, &run, &task)
            };
            if round == 0 {
                history_turn(&f, &execution, &client, &endpoint, 1, true);
                history = Some(if task_index == 0 {
                    c4_mcp_edit_redo(&C4Mcp::new(&f, &run, &task), &core)
                } else {
                    core.export_history().unwrap()
                });
                f.host
                    .owner
                    .upgrade()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .checkpoint_task_history(&run_auth, &run, &task)
                    .unwrap();
                history_turn(&f, &execution, &client, &endpoint, 1, false);
                spending_change(
                    &f,
                    &execution,
                    SpendingChange::Ceiling(2_000_000),
                    now + 719 * 30_000,
                    0,
                    true,
                )
                .unwrap();
            } else {
                assert_eq!(
                    core.export_history().unwrap(),
                    *history.as_ref().unwrap(),
                    "C5_FIX1_ATTEMPT_HISTORY"
                );
            }
            let count = sends / 3 + usize::from(round < sends % 3);
            for turn in 1..=count as u64 {
                c5e_send(&f, &execution, turn);
                for (event, ordinal, input, output, complete) in [
                    ("partial", 1, 2, 1, false),
                    ("complete", 2, 5, 2, true),
                    ("complete", 2, 5, 2, true),
                ] {
                    let sample=crate::protocol::limits::usage::decode_usage(crate::protocol::limits::usage::UsageAdapter::Codex,
                        &serde_json::json!({"usage":{"input_tokens":input,"cached_input_tokens":1,"output_tokens":output,"reasoning_output_tokens":1}}),
                        UsageSample {attempt:execution.attempt.0.clone(),turn,event:event.into(),ordinal,mode:UsageMode::Cumulative,values:UsageValues::default(),complete,input_includes_cache:true,output_includes_reasoning:true}).unwrap();
                    spending_change(
                        &f,
                        &execution,
                        SpendingChange::Usage(sample),
                        now + 719 * 30_000,
                        0,
                        true,
                    )
                    .unwrap();
                }
            }
            all_attempts.push(execution.clone());
            last = execution.clone();
            if task_index != 7 || round != 2 {
                crate::effect_pump::reconcile_attempt(
                    &f.host.owner.upgrade().unwrap(),
                    &mut claim,
                    &run_auth,
                    &execution,
                )
                .unwrap();
            }
            c5e_depth(&f, &mut maximum);
        }
        histories.push((task, history.unwrap(), sends));
        eprintln!("C5_FIX1_MAX_TASK {task_index} sends={sends}");
    }
    assert_eq!(histories.len(), run_admission::TASKS);
    assert_eq!(all_attempts.len(), run_admission::ATTEMPTS);
    assert_eq!(
        spending_record(&f, &run)
            .tasks
            .iter()
            .map(|t| t.sends.len())
            .sum::<usize>(),
        run_admission::SENDS
    );
    drop(director);
    assert!(
        spending_change(
            &f,
            &last,
            SpendingChange::Clock,
            now + RUN_MILLIS,
            30_000,
            true
        )
        .is_err()
    );
    assert_eq!(spending_record(&f, &run).elapsed_ms, RUN_MILLIS);
    let full = spending_record(&f, &run);
    assert!(
        matches!(
            f.host
                .owner
                .upgrade()
                .unwrap()
                .lock()
                .unwrap()
                .owner()
                .unwrap()
                .check_run_send_budget(&full),
            Err(kinewright_project::bundle::BundleError::RunLimit)
        ),
        "C5_FIX1_MAX_SEND_ADMISSION_CLOSED"
    );
    let stop = f.control(&f.editor, &run, RunAction::Stop);
    ok(
        f.editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(stop),
            })
            .unwrap(),
        "C5_FIX1_MAX_STOP",
    );
    crate::effect_pump::reconcile_attempt(
        &f.host.owner.upgrade().unwrap(),
        &mut claim,
        &run_auth,
        &last,
    )
    .unwrap();
    {
        let host = f.host.owner.upgrade().unwrap();
        let mut owner = host.lock().unwrap();
        let mut state = owner.spending_scope(&run_auth, &run).unwrap();
        owner.archive_retired_attempts(&mut state).unwrap();
        owner
            .publish_native(&run_auth, state, &last, vec![])
            .unwrap();
        let store = owner.owner().unwrap().store().unwrap();
        eprintln!("C5_FIX1_MAX_USAGE {:?}", store.usage().unwrap());
    }
    claim.finish().unwrap();
    let record = spending_record(&f, &run);
    let state = f.projection(&run);
    assert_eq!(state.cleanup, RunCleanup::Settled);
    assert!(state.attempts.is_empty());
    for (task, history, sends) in &histories {
        let t = record.tasks.iter().find(|t| t.task == task.0).unwrap();
        assert_eq!(t.sends.len(), *sends);
        assert_eq!(t.providers.len(), 3);
        assert_eq!(t.usage.len(), sends * 2);
        assert_eq!(t.totals.token_lower_bound, *sends as u64 * 7);
        assert_eq!(c5e_history(&f, &run, task), *history);
    }
    for execution in &all_attempts {
        assert!(matches!(
            f.host
                .owner
                .upgrade()
                .unwrap()
                .lock()
                .unwrap()
                .recovery_attempt(&state, &crate::native_launcher::native_identity(execution))
                .unwrap()
                .unwrap()
                .state,
            AttemptState::Retired(_)
        ));
    }
    let f = c4_restart(f, &claim_root);
    assert_eq!(
        spending_record(&f, &run),
        record,
        "C5_FIX1_MAX_REOPEN_ACCOUNTING"
    );
    assert_eq!(f.projection(&run), state, "C5_FIX1_MAX_REOPEN_PROJECTION");
    for (task, history, _) in &histories {
        assert_eq!(c5e_history(&f, &run, task), *history);
    }
    assert_eq!(
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .core()
            .export_history()
            .unwrap(),
        manual,
        "C5_FIX1_MAX_REOPEN_MANUAL"
    );
    c5e_depth(&f, &mut maximum);
    eprintln!(
        "C5_FIX1_MAXIMAL clocks=720 tasks=8 sends=128 attempts=24 max_depth={maximum} stop=true cleanup=true reopen_exact=true"
    );
}

#[test]
fn s4b_c5e_cumulative_reopen_prefix_and_lost_ack() {
    use super::super::native_invocation::SpendingChange;
    let f = Fixture::new();
    f.host.runtime.stop();
    let (run, execution, _) = reserve(&f);
    let admitted = native_admitted(&f, &execution);
    let (profile, _, scratch) = native_profile(&f, &admitted);
    let _charge = native_charge(&f, &admitted, profile);
    let producer = f.producer(&run);
    let host = f.host.owner.upgrade().unwrap();
    let raw = serde_json::json!({"type":"turn.completed","id":"completed","usage":{"input_tokens":8,"output_tokens":2,"cached_input_tokens":3,"reasoning_output_tokens":1}});
    {
        let mut owner = host.lock().unwrap();
        owner
            .ingest_native_raw(&producer, &execution, &raw, 1, false)
            .unwrap();
        owner
            .ingest_native_raw(&producer, &execution, &raw, 2, false)
            .unwrap();
    }
    let before = spending_record(&f, &run);
    spending_change(
        &f,
        &execution,
        SpendingChange::Clock,
        before.last_unix_ms + 1000,
        1000,
        true,
    )
    .unwrap();
    let record = spending_record(&f, &run);
    assert_eq!(record.tasks[0].usage.len(), 1);
    assert_eq!(record.tasks[0].totals.token_lower_bound, 10);
    let mut owner = host.lock().unwrap();
    let root = owner.owner().unwrap().root_sha256().unwrap();
    let mut forged = record.successor().unwrap();
    forged.tasks[0].usage.clear();
    forged.tasks[0].totals = kinewright_project::orchestration_accounting::UsageTotals::default();
    assert!(
        owner
            .publish_accounting(&producer, &execution, &forged)
            .is_err(),
        "C5E_CUMULATIVE_PREFIX_REQUIRED"
    );
    assert_eq!(owner.owner().unwrap().root_sha256().unwrap(), root);
    let mut next = record.successor().unwrap();
    next.observe_clock(record.last_unix_ms + 1000, 1000, true)
        .unwrap();
    let result = kinewright_project::io_observer::fail_nth("root_parent_sync", 1, || {
        owner.publish_accounting(&producer, &execution, &next)
    });
    assert!(result.is_err(), "C5E_LOST_ACK_CUT");
    let pending = owner.hooks.pending.clone();
    assert!(pending.is_some(), "C5E_RETAIN_UNCERTAIN_CUMULATIVE");
    assert!(
        owner
            .publish_accounting(&producer, &execution, &next.successor().unwrap())
            .is_err(),
        "C5E_NO_SUBSTITUTED_CUMULATIVE_RETRY"
    );
    assert!(
        owner.hooks.pending == pending,
        "C5E_PENDING_IDENTITY_RETAINED"
    );
    owner.reconcile_native_pending(&producer).unwrap();
    assert!(owner.hooks.pending.is_none());
    let state = owner
        .owner()
        .unwrap()
        .orchestration_projection(&run)
        .unwrap()
        .unwrap();
    assert_eq!(
        owner.native_accounting(&state).unwrap().unwrap(),
        next,
        "C5E_ORIGINAL_RECONCILED"
    );
    drop(owner);
    drop(host);
    std::fs::remove_dir_all(scratch).unwrap();
    let path = f.temp.path("project.kinewright");
    let Fixture { host, temp, .. } = f;
    drop(host);
    let reopened = Ja1EditingSession::open(&path).unwrap();
    let projection = reopened.orchestration_projection(&run).unwrap().unwrap();
    let saved = projection
        .usage
        .iter()
        .find_map(|reference| {
            let record = reopened
                .store()
                .unwrap()
                .read_record::<OrchestrationRecord>(reference)
                .unwrap();
            if let OrchestrationPayload::Accounting(record) = &record.payload {
                Some((**record).clone())
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(saved, next, "C5E_REOPEN_CURRENT_CUMULATIVE_EXACT");
    drop(reopened);
    drop(temp);
}
