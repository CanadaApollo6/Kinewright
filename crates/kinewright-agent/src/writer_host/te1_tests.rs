#[test]
fn te1_sizes_do_not_stale_controls_and_flush_with_accounting() {
    let f = Fixture::new();
    let (run, execution, director) = c4_director(&f);
    let before = f.projection(&run).sequence;
    let _status = director.status();
    assert_eq!(
        f.projection(&run).sequence,
        before,
        "TE1_OBSERVATION_CANNOT_PUBLISH_AFTER_STATUS"
    );
    let observed = {
        let host = f.host.owner.upgrade().unwrap();
        let owner = host.lock().unwrap();
        owner.hooks.runs[&run].measurements[&execution.task]
            .lock()
            .unwrap()
            .clone()
    };
    assert_eq!(observed.result_count, 1);
    assert!(observed.result_bytes > 0);
    spending_change(
        &f,
        &execution,
        super::super::native_invocation::SpendingChange::Clock,
        native_now(&f, &run),
        0,
        true,
    )
    .unwrap();
    let saved = spending_record(&f, &run);
    assert_eq!(saved.tasks[0].measurements.as_ref().unwrap(), &observed);
}

#[test]
fn te1_review_native_size_loss_is_visible_and_sticky_after_detach() {
    for missing_run in [false, true] {
        let f = Fixture::new();
        let (run, execution, director) = c4_director(&f);
        let _status = director.status();
        let mut held_task = None;
        let mut held_run = None;
        {
            let host = f.host.owner.upgrade().unwrap();
            let mut owner = host.lock().unwrap();
            if missing_run {
                held_run = owner.hooks.runs.remove(&run);
            } else {
                held_task = owner
                    .hooks
                    .runs
                    .get_mut(&run)
                    .unwrap()
                    .tasks
                    .remove(&execution.task);
            }
        }
        let _reply = director.runtime.block_on(
            director.client.call_tool(
                rmcp::model::CallToolRequestParams::new("invoke_capability".to_owned())
                    .with_arguments(
                        serde_json::json!({"name":"task_status","arguments":{}})
                            .as_object()
                            .unwrap()
                            .clone(),
                    ),
            ),
        );
        {
            let host = f.host.owner.upgrade().unwrap();
            let mut owner = host.lock().unwrap();
            if let Some(held) = held_run {
                owner.hooks.runs.insert(run.clone(), held);
            }
            if let Some(held) = held_task {
                owner
                    .hooks
                    .runs
                    .get_mut(&run)
                    .unwrap()
                    .tasks
                    .insert(execution.task.clone(), held);
            }
        }
        let sizes = director.server.result_sizes();
        assert!(sizes.incomplete, "TE1_NATIVE_SIZE_LOSS_MUST_BE_VISIBLE");
        assert_eq!(sizes.requests, 2);
        assert_eq!(sizes.count, 2);
        spending_change(
            &f,
            &execution,
            super::super::native_invocation::SpendingChange::Clock,
            native_now(&f, &run),
            0,
            true,
        )
        .unwrap();
        assert!(
            spending_record(&f, &run).tasks[0]
                .measurements
                .as_ref()
                .unwrap()
                .incomplete
        );
    }
}

#[test]
fn te1_review_native_size_observer_survives_missing_host_and_poison() {
    for missing_host in [false, true] {
        let f = Fixture::new();
        let (run, execution, director) = c4_director(&f);
        let _status = director.status();
        let host = f.host.owner.upgrade().unwrap();
        let control = {
            let owner = host.lock().unwrap();
            let retained = &owner.hooks.runs[&run];
            super::super::task_mcp::TaskMcp::new(
                if missing_host {
                    std::sync::Weak::new()
                } else {
                    Arc::downgrade(&host)
                },
                owner.run_producer(&run).unwrap(),
                retained.tasks[&execution.task].endpoint(),
                retained.measurements[&execution.task].clone(),
                None,
            )
        };
        if !missing_host {
            let poisoned = host.clone();
            assert!(
                std::thread::spawn(move || {
                    let _guard = poisoned.lock().unwrap();
                    panic!("intentional fixture mutex poison");
                })
                .join()
                .is_err()
            );
        }
        assert!(!control.record_size("task_status", Some(13)));
        host.clear_poison();
        let retained = host.lock().unwrap().hooks.runs[&run].measurements[&execution.task].clone();
        assert_eq!(retained.lock().unwrap().result_count, 2);
        assert!(retained.lock().unwrap().incomplete);
        spending_change(
            &f,
            &execution,
            super::super::native_invocation::SpendingChange::Clock,
            native_now(&f, &run),
            0,
            true,
        )
        .unwrap();
        let record = spending_record(&f, &run);
        let saved = record.tasks[0].measurements.as_ref().unwrap();
        assert_eq!(saved.result_count, 2);
        assert!(saved.incomplete);
    }
}

#[test]
fn te1_observation_capacity_is_durable_and_sticky() {
    use super::super::native_invocation::SpendingChange;
    use kinewright_project::orchestration_accounting::{
        AccountingRefusal, UsageMode, UsageSample, UsageValues, normalize,
    };
    let f = Fixture::new();
    let (run, execution, _) = reserve(&f);
    let admitted = native_admitted(&f, &execution);
    let (profile, _, scratch) = native_profile(&f, &admitted);
    let _charge = native_charge(&f, &admitted, profile);
    let previous = spending_record(&f, &run);
    let mut seed = previous.successor().unwrap();
    let task = &mut seed.tasks[0];
    for ordinal in 1..=kinewright_project::run_admission::USAGE_SAMPLES as u64 {
        task.usage.push(UsageSample {
            snapshot: None,
            attempt: execution.attempt.0.clone(),
            turn: 1,
            event: format!("{ordinal:064x}"),
            ordinal,
            mode: UsageMode::Cumulative,
            values: UsageValues {
                input: Some(1),
                output: Some(0),
                ..UsageValues::default()
            },
            complete: true,
            input_includes_cache: true,
            output_includes_reasoning: true,
        });
    }
    task.totals = normalize(&task.usage).unwrap();
    seed.validate_successor(&previous).unwrap();
    let producer = f.producer(&run);
    {
        let host = f.host.owner.upgrade().unwrap();
        let mut owner = host.lock().unwrap();
        owner
            .publish_accounting(&producer, &execution, &seed)
            .unwrap();
    }
    let mut excess = seed.tasks[0].usage.last().unwrap().clone();
    excess.ordinal += 1;
    excess.event = format!("{:064x}", excess.ordinal);
    assert!(
        spending_change(
            &f,
            &execution,
            SpendingChange::Usage(excess),
            native_now(&f, &run),
            0,
            true
        )
        .is_err()
    );
    let bounded = spending_record(&f, &run);
    assert_eq!(bounded.tasks[0].usage, seed.tasks[0].usage);
    assert!(bounded.tasks[0].measurements.as_ref().unwrap().incomplete);
    assert_eq!(bounded.tasks[0].pause, Some(AccountingRefusal::Capacity));
    {
        let host = f.host.owner.upgrade().unwrap();
        let mut owner = host.lock().unwrap();
        let mut buffered = bounded.tasks[0].measurements.clone().unwrap();
        buffered.incomplete = false;
        owner
            .hooks
            .runs
            .get_mut(&run)
            .unwrap()
            .measurements
            .insert(execution.task.clone(), Arc::new(Mutex::new(buffered)));
    }
    spending_change(
        &f,
        &execution,
        SpendingChange::Clock,
        native_now(&f, &run),
        0,
        true,
    )
    .unwrap();
    assert!(
        spending_record(&f, &run).tasks[0]
            .measurements
            .as_ref()
            .unwrap()
            .incomplete
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "exercise the serialized reserve through a real accounting publication"
)]
fn te1_perf_serialized_reserve_remains_durable() {
    use kinewright_project::bundle_records::SessionId;
    use kinewright_project::orchestration_accounting::{
        AccountingRefusal, ProviderMetadata, ProviderSelection, encode,
    };
    let f = Fixture::new();
    let (run, execution, _) = reserve(&f);
    let admitted = native_admitted(&f, &execution);
    let (profile, _, scratch) = native_profile(&f, &admitted);
    let _charge = native_charge(&f, &admitted, profile);
    let previous = spending_record(&f, &run);
    let mut seed = previous.successor().unwrap();
    for _ in 0..8 {
        let selection = ProviderSelection {
            connector: "fixture".into(),
            model: "fixture".into(),
            options: (0..64)
                .map(|i| (format!("key-{i}"), "x".repeat(1024)))
                .collect(),
        };
        seed.tasks[0]
            .select_provider(ProviderMetadata {
                attempt: SessionId::fresh().unwrap(),
                requested: selection.clone(),
                resolved: selection,
                cli_version: "fixture".into(),
            })
            .unwrap();
    }
    let limit = kinewright_project::run_admission::ACCOUNTING_BYTES;
    let target = limit - 4096 - 64;
    let mut excess = encode(&seed, limit * 2)
        .unwrap()
        .len()
        .checked_sub(target)
        .unwrap();
    for provider in seed.tasks[0].providers.iter_mut().skip(1) {
        for selection in [&mut provider.requested, &mut provider.resolved] {
            for (_, value) in &mut selection.options {
                let removed = excess.min(value.len());
                value.truncate(value.len() - removed);
                excess -= removed;
            }
        }
    }
    assert_eq!(excess, 0);
    assert_eq!(encode(&seed, limit).unwrap().len(), target);
    assert_eq!(seed.validate_successor_size(&previous).unwrap(), target);
    let producer = f.producer(&run);
    let host = f.host.owner.upgrade().unwrap();
    {
        let mut owner = host.lock().unwrap();
        owner
            .publish_accounting(&producer, &execution, &seed)
            .unwrap();
    }
    let raw = serde_json::json!({"type":"turn.completed","usage":{"input_tokens":71,"cached_input_tokens":50,"output_tokens":8}});
    assert!(
        host.lock()
            .unwrap()
            .ingest_native_raw(&producer, &execution, &raw, 1, false)
            .is_err(),
        "TE1_SERIALIZED_RESERVE_MUST_REFUSE"
    );
    let after = spending_record(&f, &run);
    assert_eq!(after.tasks[0].usage, seed.tasks[0].usage);
    assert_eq!(after.tasks[0].pause, Some(AccountingRefusal::Capacity));
    assert!(after.tasks[0].measurements.as_ref().unwrap().incomplete);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn te1_native_new_turn_requires_a_new_final_even_when_totals_replay() {
    let f = Fixture::new();
    let (run, execution, _) = reserve(&f);
    let admitted = native_admitted(&f, &execution);
    let (profile, _, scratch) = native_profile(&f, &admitted);
    let _charge = native_charge(&f, &admitted, profile);
    let producer = f.producer(&run);
    let host = f.host.owner.upgrade().unwrap();
    let start = serde_json::json!({"type":"turn.started"});
    let final_raw = serde_json::json!({"type":"turn.completed","usage":{"input_tokens":71,"cached_input_tokens":50,"output_tokens":8}});
    let before = spending_record(&f, &run);
    let sequence = f.projection(&run).sequence;
    host.lock()
        .unwrap()
        .ingest_native_raw(&producer, &execution, &start, 1, false)
        .unwrap();
    assert_eq!(
        f.projection(&run).sequence,
        sequence,
        "TE1_INITIAL_START_NEEDS_NO_PUBLICATION"
    );
    assert_eq!(spending_record(&f, &run), before);
    {
        let mut owner = host.lock().unwrap();
        owner
            .ingest_native_raw(&producer, &execution, &final_raw, 2, false)
            .unwrap();
        owner
            .ingest_native_raw(&producer, &execution, &start, 3, false)
            .unwrap();
    }
    assert!(!spending_record(&f, &run).tasks[0].totals.tokens_complete);
    let pending = spending_record(&f, &run);
    assert_eq!(
        pending.observer_publications,
        before.observer_publications + 1
    );
    let legacy_complete =
        super::super::native_invocation::provider_accounting_view(&pending).unwrap();
    assert_eq!(
        legacy_complete.revision,
        before.revision - before.observer_publications + 1
    );
    assert!(legacy_complete.tasks[0].totals.tokens_complete);
    let reopened: kinewright_project::orchestration_accounting::AccountingRecord =
        serde_json::from_slice(&serde_json::to_vec(&pending).unwrap()).unwrap();
    assert_eq!(reopened, pending);
    let sequence = f.projection(&run).sequence;
    host.lock()
        .unwrap()
        .ingest_native_raw(&producer, &execution, &start, 4, false)
        .unwrap();
    assert_eq!(
        f.projection(&run).sequence,
        sequence,
        "TE1_PENDING_START_NEEDS_NO_PUBLICATION"
    );
    {
        let mut owner = host.lock().unwrap();
        owner
            .ingest_native_raw(&producer, &execution, &final_raw, 5, false)
            .unwrap();
    }
    assert_eq!(
        spending_record(&f, &run).tasks[0].totals.token_lower_bound,
        79
    );
    assert!(spending_record(&f, &run).tasks[0].totals.tokens_complete);
    {
        let mut owner = host.lock().unwrap();
        owner
            .ingest_native_raw(&producer, &execution, &start, 6, false)
            .unwrap();
        owner
            .ingest_native_raw(
                &producer,
                &execution,
                &serde_json::json!({"type":"native.eof"}),
                7,
                true,
            )
            .unwrap();
    }
    let task = &spending_record(&f, &run).tasks[0];
    assert_eq!(task.totals.token_lower_bound, 79);
    assert!(!task.totals.tokens_complete);
    let legacy =
        super::super::native_invocation::provider_accounting_view(&spending_record(&f, &run))
            .unwrap();
    assert_eq!(
        legacy, legacy_complete,
        "observer starts, exact final replay and EOF cannot change the legacy wake"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}
