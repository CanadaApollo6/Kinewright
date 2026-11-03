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
        owner.hooks.runs[&run].measurements[&execution.task].clone()
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
            .insert(execution.task.clone(), buffered);
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
    {
        let mut owner = host.lock().unwrap();
        owner
            .ingest_native_raw(&producer, &execution, &start, 1, false)
            .unwrap();
        owner
            .ingest_native_raw(&producer, &execution, &final_raw, 2, false)
            .unwrap();
        owner
            .ingest_native_raw(&producer, &execution, &start, 3, false)
            .unwrap();
    }
    assert!(!spending_record(&f, &run).tasks[0].totals.tokens_complete);
    {
        let mut owner = host.lock().unwrap();
        owner
            .ingest_native_raw(&producer, &execution, &final_raw, 4, false)
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
            .ingest_native_raw(&producer, &execution, &start, 5, false)
            .unwrap();
        owner
            .ingest_native_raw(
                &producer,
                &execution,
                &serde_json::json!({"type":"native.eof"}),
                6,
                true,
            )
            .unwrap();
    }
    let task = &spending_record(&f, &run).tasks[0];
    assert_eq!(task.totals.token_lower_bound, 79);
    assert!(!task.totals.tokens_complete);
    std::fs::remove_dir_all(scratch).unwrap();
}
