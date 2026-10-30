#[test]
fn s4b_c3_generic_claim_and_retry_cannot_claim_process_bound() {
    let f = Fixture::new();
    let (run, execution, _) = reserve(&f);
    let mut state = f.projection(&run);
    let effect = state.effects[0].id.clone();
    assert!(
        crate::orchestration::outbox::claim(&state, &effect, &execution.attempt).is_err(),
        "C3_GENERIC_PROCESS_CLAIM_REFUSED"
    );
    state.effects[0].state = EffectState::OutcomeUnknown;
    assert!(
        crate::orchestration::outbox::retry_reconciled_safe(&state, &effect).is_err(),
        "C3_GENERIC_PROCESS_RETRY_REFUSED"
    );
    state.effects[0].safety = EffectSafety::ReplaySafe;
    state = crate::orchestration::outbox::retry_reconciled_safe(&state, &effect).unwrap();
    assert!(crate::orchestration::outbox::claim(&state, &effect, &execution.attempt).is_ok());
}
fn spending_change(
    f: &Fixture,
    execution: &ExecutionIdentity,
    change: super::super::native_invocation::SpendingChange,
    now: u64,
    delta: u64,
    trusted: bool,
) -> Result<(), HostError> {
    let producer = f.producer(&execution.run);
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .change_spending(
            &producer,
            execution,
            change,
            crate::protocol::limits::usage::ClockObservation {
                unix_ms: now,
                monotonic_delta_ms: delta,
                trusted,
            },
        )
}
fn spending_record(
    f: &Fixture,
    run: &RunId,
) -> kinewright_project::orchestration_accounting::AccountingRecord {
    let state = f.projection(run);
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .native_accounting(&state)
        .unwrap()
        .unwrap()
}
#[test]
fn s4b_c3_raw_usage_deduplicates_cumulative_and_keeps_missing_unavailable() {
    use super::super::native_invocation::SpendingChange;
    let f = Fixture::new();
    let (run, execution, _) = reserve(&f);
    let now = native_now(&f, &run);
    spending_change(&f, &execution, SpendingChange::Ceiling(100), now, 0, true).unwrap();
    let admitted = native_admitted(&f, &execution);
    let (profile, _, scratch) = native_profile(&f, &admitted);
    let _charge = native_charge(&f, &admitted, profile);
    let producer = f.producer(&run);
    let raw = serde_json::json!({"type":"turn.usage", "id":"a", "usage":{"input_tokens":20, "cached_input_tokens":10,"output_tokens":5,"reasoning_output_tokens":3}});
    let final_raw = serde_json::json!({"type":"turn.completed", "id":"b", "usage":{"input_tokens":90, "cached_input_tokens":30,"output_tokens":20,"reasoning_output_tokens":15}});
    {
        let host = f.host.owner.upgrade().unwrap();
        let mut host = host.lock().unwrap();
        host.ingest_native_raw(&producer, &execution, &raw, 1, false)
            .unwrap();
        host.ingest_native_raw(&producer, &execution, &raw, 2, false)
            .unwrap();
        host.ingest_native_raw(&producer, &execution, &final_raw, 3, false)
            .unwrap();
        host.ingest_native_raw(
            &producer,
            &execution,
            &serde_json::json!({"type":"native.eof"}),
            4,
            true,
        )
        .unwrap();
    }
    let record = spending_record(&f, &run);
    let task = &record.tasks[0];
    assert_eq!(task.usage.len(), 2);
    assert_eq!(
        task.totals.token_lower_bound, 110,
        "C3_CACHE_REASONING_COUNTED_ONCE"
    );
    assert_eq!(task.totals.known.input, Some(90));
    assert_eq!(task.totals.known.output, Some(20));
    assert_eq!(task.totals.known.cost_micros, None, "C3_COST_UNAVAILABLE");
    assert_eq!(
        task.pause,
        Some(kinewright_project::orchestration_accounting::AccountingRefusal::Tokens)
    );
    let mut next = task.clone();
    let mut send = task.sends[0].clone();
    send.attempt = kinewright_project::bundle_records::SessionId::fresh().unwrap();
    send.command.sequence += 1;
    assert!(
        next.consume_send(send).is_err(),
        "C3_POSITIVE_CEILING_STOPS_NEXT_SEND"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}
#[test]
fn s4b_c3_unresolved_usage_blocks_capped_send() {
    use super::super::native_invocation::SpendingChange;
    let f = Fixture::new();
    let (run, execution, _) = reserve(&f);
    spending_change(
        &f,
        &execution,
        SpendingChange::Ceiling(1000),
        native_now(&f, &run),
        0,
        true,
    )
    .unwrap();
    let admitted = native_admitted(&f, &execution);
    let (profile, _, scratch) = native_profile(&f, &admitted);
    let _charge = native_charge(&f, &admitted, profile);
    let mut record = spending_record(&f, &run);
    let mut send = record.tasks[0].sends[0].clone();
    send.command.sequence += 1;
    send.attempt = kinewright_project::bundle_records::SessionId::fresh().unwrap();
    let mut provider = record.tasks[0].providers[0].clone();
    provider.attempt = send.attempt.clone();
    record.tasks[0].select_provider(provider).unwrap();
    assert_eq!(
        record.tasks[0].consume_send(send.clone()),
        Err(kinewright_project::orchestration_accounting::AccountingRefusal::UsageUnavailable),
        "C3_OUTSTANDING_USAGE_BLOCKS"
    );
    let producer = f.producer(&run);
    f.host
        .owner
        .upgrade()
        .unwrap()
        .lock()
        .unwrap()
        .ingest_native_raw(
            &producer,
            &execution,
            &serde_json::json!({"type":"turn.completed"}),
            1,
            false,
        )
        .unwrap();
    let mut record = spending_record(&f, &run);
    assert_eq!(record.tasks[0].totals.known.input, None);
    assert_eq!(record.tasks[0].totals.known.output, None);
    assert!(
        record.tasks[0].consume_send(send).is_err(),
        "C3_MISSING_USAGE_IS_NOT_ZERO"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}
#[test]
fn s4b_c3_all_budgets_survive_restart_and_downtime() {
    use super::super::native_invocation::SpendingChange;
    use kinewright_project::orchestration_accounting::*;
    let f = Fixture::new();
    let (run, execution, _) = reserve(&f);
    let mut record = spending_record(&f, &run);
    let start = record.start_unix_ms;
    for ordinal in 1..=80 {
        let attempt = kinewright_project::bundle_records::SessionId::fresh().unwrap();
        let task = record.task_mut(&execution.task.0).unwrap();
        let provider = ProviderSelection {
            connector: "scripted".into(),
            model: "fixture".into(),
            options: vec![],
        };
        task.select_provider(ProviderMetadata {
            attempt: attempt.clone(),
            requested: provider.clone(),
            resolved: provider,
            cli_version: "1.2.3".into(),
        })
        .unwrap();
        task.consume_send(SendConsumption {
            command: CommandId {
                issuer: Fixture::issue(&f.editor).envelope.id.issuer,
                sequence: ordinal,
            },
            attempt,
            turn: 1,
            payload: f.brief.assignment.clone(),
        })
        .unwrap();
    }
    let mut extra = record.tasks[0].sends[0].clone();
    extra.command.sequence = 81;
    extra.attempt = kinewright_project::bundle_records::SessionId::fresh().unwrap();
    assert_eq!(
        record.tasks[0].consume_send(extra.clone()),
        Err(AccountingRefusal::Sends)
    );
    {
        let producer = f.producer(&run);
        let owner = f.host.owner.upgrade().unwrap();
        let mut host = owner.lock().unwrap();
        let mut state = host.spending_scope(&producer, &run).unwrap();
        let reference = host
            .owner_mut()
            .unwrap()
            .admit_scheduler_record(&OrchestrationRecord {
                version: 1,
                payload: OrchestrationPayload::Accounting(Box::new(record)),
            })
            .unwrap()
            .reference;
        host.replace_accounting_reference(&mut state, reference)
            .unwrap();
        host.publish_native(&producer, state, &execution, vec![])
            .unwrap();
    }
    spending_change(
        &f,
        &execution,
        SpendingChange::Wait(Some("rate limited".into())),
        start + 10_000,
        10_000,
        true,
    )
    .unwrap();
    let delivery = kinewright_project::bundle_records::SessionId::fresh().unwrap();
    spending_change(
        &f,
        &execution,
        SpendingChange::Repair {
            delivery: delivery.clone(),
            failure: "a".repeat(64),
        },
        start + 20_000,
        10_000,
        true,
    )
    .unwrap();
    assert!(
        spending_change(
            &f,
            &execution,
            SpendingChange::Repair {
                delivery,
                failure: "a".repeat(64)
            },
            start + 30_000,
            10_000,
            true
        )
        .is_err()
    );
    let expected = spending_record(&f, &run);
    let Fixture { host, temp, .. } = f;
    drop(host);
    let owner = Ja1EditingSession::open(&temp.path("project.kinewright")).unwrap();
    let state = owner.orchestration_projection(&run).unwrap().unwrap();
    let reference = state.usage.last().unwrap();
    let record = owner
        .store()
        .unwrap()
        .read_record::<OrchestrationRecord>(reference)
        .unwrap();
    let OrchestrationPayload::Accounting(mut record) = record.payload.clone() else {
        panic!("accounting");
    };
    assert_eq!(*record, expected, "C3_RESTART_EVERY_BUDGET");
    assert_eq!(record.tasks[0].sends.len(), 80);
    assert_eq!(record.tasks[0].repairs[0].rounds, 2);
    assert_eq!(record.tasks[0].pause, Some(AccountingRefusal::Replan));
    assert_eq!(record.tasks[0].wait_reason.as_deref(), Some("rate limited"));
    assert!(
        record.tasks[0].consume_send(extra).is_err(),
        "C3_81ST_ACROSS_ATTEMPTS"
    );
    assert_eq!(
        record.observe_clock(start + RUN_MILLIS, 0, true),
        Err(AccountingRefusal::Time),
        "C3_DOWNTIME_SPENDS_SIX_HOURS"
    );
}
#[test]
fn s4b_c3_clock_uncertainty_pauses_at_the_host() {
    use super::super::native_invocation::SpendingChange;
    let f = Fixture::new();
    let (run, execution, _) = reserve(&f);
    let now = native_now(&f, &run);
    assert!(spending_change(&f, &execution, SpendingChange::Clock, now - 1, 1, true).is_err());
    assert_eq!(f.projection(&run).phase, RunPhase::Paused);
    let record = spending_record(&f, &run);
    assert!(record.clock_uncertain);
    assert!(record.check_clock().is_err());
}

#[test]
fn s4b_c3_raw_waits_and_failures_reach_the_durable_budget() {
    let f = Fixture::new();
    let (run, execution, _) = reserve(&f);
    let admitted = native_admitted(&f, &execution);
    let (profile, _, scratch) = native_profile(&f, &admitted);
    let _charge = native_charge(&f, &admitted, profile);
    let producer = f.producer(&run);
    let host = f.host.owner.upgrade().unwrap();
    {
        let mut host = host.lock().unwrap();
        host.ingest_native_raw(
            &producer,
            &execution,
            &serde_json::json!({"type":"rate_limit"}),
            1,
            false,
        )
        .unwrap();
        host.ingest_native_raw(&producer,&execution,&serde_json::json!({"type":"item.completed","item":{"status":"failed","id":"first","error":{"message":"same scoped failure"}}}),2,false).unwrap();
        assert!(host.ingest_native_raw(&producer,&execution,&serde_json::json!({"type":"item.completed","item":{"status":"failed","id":"second","error":{"message":"same scoped failure"}}}),3,false).is_err());
    }
    let record = spending_record(&f, &run);
    assert_eq!(
        record.tasks[0].repairs[0].rounds, 2,
        "C3_REPAIR_BUDGET_AT_RAW_BOUNDARY"
    );
    assert_eq!(record.tasks[0].repeated_failures, 2);
    assert_eq!(
        record.tasks[0].pause,
        Some(kinewright_project::orchestration_accounting::AccountingRefusal::Replan)
    );
    assert_eq!(
        record.tasks[0].wait_reason.as_deref(),
        Some("provider rate limit")
    );
    std::fs::remove_dir_all(scratch).unwrap();
}
