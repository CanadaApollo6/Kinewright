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
    use kinewright_project::orchestration_accounting::{
        ProviderMetadata, ProviderSelection, RUN_MILLIS, UsageMode, UsageSample, UsageValues,
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
    let (run, parent, director) = c4_director(&f);
    for role in [Role::Editor, Role::Reviewer] {
        let mut brief = f.brief.clone();
        brief.role = role;
        if role == Role::Reviewer {
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
    }
    drop(director);
    let tasks: Vec<_> = f
        .projection(&run)
        .tasks
        .iter()
        .map(|t| t.id.clone())
        .collect();
    let now = native_now(&f, &run);
    let mut current = vec![];
    let mut expected = vec![];
    for task in &tasks {
        let (execution, client, endpoint, core) = if *task == parent.task {
            let (client, endpoint) = task_actor(&f, &run, &parent, &f.producer_client(&run));
            let core = f.host.owner.upgrade().unwrap().lock().unwrap().hooks.runs[&run].tasks[task]
                .core()
                .clone();
            (parent.clone(), client, endpoint, core)
        } else {
            c4_reserve_task(&f, &run, task)
        };
        history_turn(&f, &execution, &client, &endpoint, 1, true);
        let history = if f
            .projection(&run)
            .tasks
            .iter()
            .find(|t| t.id == *task)
            .unwrap()
            .brief
            .role
            == Role::Editor
        {
            c4_mcp_edit_redo(&C4Mcp::new(&f, &run, task), &core)
        } else {
            core.export_history().unwrap()
        };
        let producer = f.producer(&run);
        f.host
            .owner
            .upgrade()
            .unwrap()
            .lock()
            .unwrap()
            .checkpoint_task_history(&producer, &run, task)
            .unwrap();
        history_turn(&f, &execution, &client, &endpoint, 1, false);
        spending_change(
            &f,
            &execution,
            SpendingChange::Ceiling(2_000_000),
            now,
            0,
            true,
        )
        .unwrap();
        expected.push(history);
        current.push(execution);
    }
    let sequence = f.projection(&run).sequence;
    let mut maximum = 0;
    let started = Instant::now();
    for tick in 1..720 {
        let result = spending_change(
            &f,
            &parent,
            SpendingChange::Clock,
            now + tick * 30_000,
            30_000,
            true,
        );
        assert!(
            result.is_ok(),
            "C5E_CLOCK_PUBLICATION tick={tick} {result:?}"
        );
        assert_eq!(
            f.projection(&run).sequence,
            sequence,
            "C5E_CLOCK_REPLACES_STATE"
        );
        c5e_depth(&f, &mut maximum);
        if tick % 60 == 0 {
            eprintln!(
                "C5E_CLOCK tick={tick} max_depth={maximum} elapsed_s={}",
                started.elapsed().as_secs()
            );
        }
    }
    eprintln!(
        "C5E_CLOCK_PHASE_DONE elapsed_s={}",
        started.elapsed().as_secs()
    );
    let claim_root = f.temp.path("c5e-machine");
    let mut claim = crate::execution_claim::MachineClaim::acquire_fixture(
        &claim_root,
        parent.project.as_str(),
        run.0.as_str(),
    )
    .unwrap();
    let mut all_attempts = vec![];
    for round in 0..3 {
        if round != 0 {
            current.clear();
            for (task, history) in tasks.iter().zip(&expected) {
                let (execution, _, _, core) = c4_reserve_task(&f, &run, task);
                assert_eq!(
                    core.export_history().unwrap(),
                    *history,
                    "C5E_ATTEMPT_HISTORY"
                );
                current.push(execution);
            }
        }
        for execution in &current {
            let producer = f.producer(&run);
            {
                let host = f.host.owner.upgrade().unwrap();
                let mut owner = host.lock().unwrap();
                let state = owner.spending_scope(&producer, &run).unwrap();
                let previous = owner.native_accounting(&state).unwrap().unwrap();
                let mut next = previous.successor().unwrap();
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
                owner
                    .publish_accounting(&producer, execution, &next)
                    .unwrap();
            }
            let count = if round == 2 { 26 } else { 27 };
            for turn in 1..=count {
                c5e_send(&f, execution, turn);
                if turn % 10 == 0 {
                    eprintln!(
                        "C5E_TURNS round={round} turn={turn} elapsed_s={}",
                        started.elapsed().as_secs()
                    );
                }
                c5e_depth(&f, &mut maximum);
                for (event, ordinal, input, output, complete) in [
                    ("partial", 1, 2, 1, false),
                    ("complete", 2, 5, 2, true),
                    ("complete", 2, 5, 2, true),
                ] {
                    let sample = crate::protocol::limits::usage::decode_usage(crate::protocol::limits::usage::UsageAdapter::Codex,
                        &serde_json::json!({"usage":{"input_tokens":input,"cached_input_tokens":1,"output_tokens":output,"reasoning_output_tokens":1}}),
                        UsageSample { attempt: execution.attempt.0.clone(), turn, event: event.into(), ordinal, mode: UsageMode::Cumulative, values: UsageValues::default(), complete, input_includes_cache: true, output_includes_reasoning: true }).unwrap();
                    spending_change(
                        &f,
                        execution,
                        SpendingChange::Usage(sample),
                        now + 719 * 30_000,
                        0,
                        true,
                    )
                    .unwrap();
                    c5e_depth(&f, &mut maximum);
                }
            }
            f.host
                .owner
                .upgrade()
                .unwrap()
                .lock()
                .unwrap()
                .checkpoint_task_history(&producer, &run, &execution.task)
                .unwrap();
        }
        if round != 2 {
            for execution in &current {
                crate::effect_pump::reconcile_attempt(
                    &f.host.owner.upgrade().unwrap(),
                    &mut claim,
                    &f.producer(&run),
                    execution,
                )
                .unwrap();
                c5e_depth(&f, &mut maximum);
            }
        }
        all_attempts.extend(current.clone());
        eprintln!("C5E_SENDS round={round} max_depth={maximum}");
    }
    let final_tick = spending_change(
        &f,
        &parent,
        SpendingChange::Clock,
        now + RUN_MILLIS,
        30_000,
        true,
    );
    assert!(final_tick.is_err(), "C5E_DEADLINE_POLICY");
    assert_eq!(
        spending_record(&f, &run).elapsed_ms,
        RUN_MILLIS,
        "C5E_FINAL_CLOCK_PUBLISHED"
    );
    c5e_depth(&f, &mut maximum);
    let stop = f.control(&f.editor, &run, RunAction::Stop);
    ok(
        f.editor
            .request(HostCommand::DispatchOrchestration {
                request: Box::new(stop),
            })
            .unwrap(),
        "C5E_STOP_AT_MAXIMAL_RUN",
    );
    for execution in &current {
        crate::effect_pump::reconcile_attempt(
            &f.host.owner.upgrade().unwrap(),
            &mut claim,
            &f.producer(&run),
            execution,
        )
        .unwrap();
        c5e_depth(&f, &mut maximum);
    }
    {
        let producer = f.producer(&run);
        let host = f.host.owner.upgrade().unwrap();
        let mut owner = host.lock().unwrap();
        let mut state = owner.spending_scope(&producer, &run).unwrap();
        owner.archive_retired_attempts(&mut state).unwrap();
        owner
            .publish_native(&producer, state, &parent, vec![])
            .unwrap();
    }
    claim.finish().unwrap();
    assert!(!claim_root.join("run.json").exists(), "C5E_CLAIM_RETIRED");
    let record = spending_record(&f, &run);
    assert_eq!(record.tasks.len(), 3);
    for task in &record.tasks {
        assert_eq!(task.sends.len(), 80);
        assert_eq!(task.providers.len(), 3);
        assert_eq!(task.usage.len(), 160);
        assert_eq!(task.totals.token_lower_bound, 560);
    }
    let state = f.projection(&run);
    assert_eq!(state.cleanup, RunCleanup::Settled);
    assert!(matches!(state.outcome, RunOutcome::Cancelled(_)));
    assert!(state.attempts.is_empty(), "C5E_RETIRED_ARCHIVED");
    for execution in &all_attempts {
        let host = f.host.owner.upgrade().unwrap();
        let owner = host.lock().unwrap();
        assert!(
            matches!(
                owner
                    .recovery_attempt(&state, &crate::native_launcher::native_identity(execution))
                    .unwrap()
                    .unwrap()
                    .state,
                AttemptState::Retired(_)
            ),
            "C5E_ARCHIVE_IDENTITIES"
        );
    }
    for (task, history) in tasks.iter().zip(&expected) {
        assert_eq!(
            c5e_history(&f, &run, task),
            *history,
            "C5E_HISTORY_AFTER_STOP"
        );
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
        manual
    );
    let f = c4_restart(f, &claim_root);
    assert_eq!(
        spending_record(&f, &run),
        record,
        "C5E_REOPEN_ACCOUNTING_EXACT"
    );
    assert_eq!(f.projection(&run), state, "C5E_REOPEN_PROJECTION_EXACT");
    for (task, history) in tasks.iter().zip(&expected) {
        assert_eq!(
            c5e_history(&f, &run, task),
            *history,
            "C5E_REOPEN_HISTORY_EXACT"
        );
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
        "C5E_REOPEN_MANUAL_REDO"
    );
    c5e_depth(&f, &mut maximum);
    eprintln!(
        "C5E_MAXIMAL clocks=720 interval_ms=30000 tasks=3 sends_per_task=80 attempts_per_task=3 samples_per_task=160 max_depth={maximum} stop=true cleanup=true reopen_exact=true"
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
