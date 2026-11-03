#[test]
fn te1_full_prompt_bytes_are_frozen_at_b4e9abc8() {
    let f = Fixture::new();
    let (run, _, _) = reserve(&f);
    let mut task = f.projection(&run).tasks[0].clone();
    task.id = serde_json::from_value(serde_json::json!("1".repeat(32))).unwrap();
    let evidence = task.brief.plan.clone();
    task.brief.inputs = evidence.clone();
    task.brief.scope.base.branch =
        serde_json::from_value(serde_json::json!("2".repeat(32))).unwrap();
    task.brief.scope.base.head = serde_json::from_value(serde_json::json!("3".repeat(32))).unwrap();
    task.brief.scope.base.checkpoint = evidence;
    task.brief.scope.base.document_hash = "4".repeat(64);
    let host = f.host.owner.upgrade().unwrap();
    let owner = host.lock().unwrap();
    let bytes = owner
        .codex_prompt(&task, b"frozen application payload")
        .unwrap();
    let hash = kinewright_media::sha256_bytes(&bytes);
    println!("TE1_FULL_PROMPT {hash}");
    let expected: String =
        serde_json::from_str(include_str!("../../tests/fixtures/te1-prompt.json")).unwrap();
    assert_eq!(hash, expected);
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "build the legacy and observed accounting fixtures through the same real store"
)]
fn te1_review_wake_accounting_expansion_is_frozen_at_b4e9abc8() {
    use crate::protocol::limits::usage::{UsageAdapter, decode_usage};
    use kinewright_project::{
        bundle_records::SessionId, orchestration_accounting::*, usage_stream::*,
    };
    let id = |n: u64| {
        serde_json::from_value::<SessionId>(serde_json::json!(format!("{n:032x}"))).unwrap()
    };
    let f = Fixture::new();
    let host = f.host.owner.upgrade().unwrap();
    let mut owner = host.lock().unwrap();
    let project = owner.owner().unwrap().project_id().unwrap();
    let raw = serde_json::json!({"type":"turn.completed","usage":{"input_tokens":71,"cached_input_tokens":50,"output_tokens":8}});
    let start = serde_json::json!({"type":"turn.started"});
    let sample = |raw: &serde_json::Value, ordinal: u64, observed: bool| {
        let kind = raw["type"].as_str().unwrap();
        let event = if observed {
            format!("{kind}:{ordinal:016x}")
        } else {
            format!(
                "{kind}:{}",
                kinewright_media::sha256_bytes(&serde_json::to_vec(raw).unwrap())
            )
        };
        decode_usage(
            UsageAdapter::Codex,
            raw,
            UsageSample {
                snapshot: observed.then(|| Snapshot {
                    payload_sha256: "0".repeat(64),
                    stream: id(4),
                    adapter: Adapter::Codex01660V1,
                    models: vec![],
                }),
                attempt: id(4),
                turn: 1,
                event,
                ordinal,
                mode: UsageMode::Cumulative,
                values: UsageValues::default(),
                complete: kind == "turn.completed",
                input_includes_cache: true,
                output_includes_reasoning: true,
            },
        )
        .unwrap()
    };
    let mut legacy = AccountingRecord::new(project, id(2), 1000).unwrap();
    legacy.add_task(id(3), None).unwrap();
    let selection = ProviderSelection {
        connector: "fixture".into(),
        model: "fixture".into(),
        options: vec![],
    };
    legacy.tasks[0]
        .select_provider(ProviderMetadata {
            attempt: id(4),
            requested: selection.clone(),
            resolved: selection,
            cli_version: "fixture-1".into(),
        })
        .unwrap();
    legacy.tasks[0].usage.push(sample(&raw, 2, false));
    legacy.tasks[0].totals = normalize(&legacy.tasks[0].usage).unwrap();
    let mut observed = legacy.clone();
    let task = &mut observed.tasks[0];
    task.bind_stream(StreamBinding {
        attempt: id(4),
        stream: id(4),
        adapter: Adapter::Codex01660V1,
        resumed: false,
    })
    .unwrap();
    task.measurements
        .get_or_insert_with(Default::default)
        .result(kinewright_project::usage_measurement::ToolResult {
            attempt: id(4),
            turn: None,
            provider_call: None,
            host_call: 0,
            tool: "task_status".into(),
            serialized_bytes: 19,
        });
    task.usage = vec![sample(&start, 1, true), sample(&raw, 2, true)];
    task.totals = normalize(&task.usage).unwrap();
    let admitted = owner
        .owner_mut()
        .unwrap()
        .admit_scheduler_record(&OrchestrationRecord {
            version: 1,
            payload: OrchestrationPayload::Accounting(Box::new(observed)),
        })
        .unwrap();
    // Exercise the production wake's actual record expansion, including its store read.
    let mut actual = owner.expanded_result_contents(&admitted.reference).unwrap();
    actual["payload"]["record"]["project"] = serde_json::json!("1".repeat(32));
    legacy.project = serde_json::from_value(serde_json::json!("1".repeat(32))).unwrap();
    let expected = serde_json::to_value(OrchestrationRecord {
        version: 1,
        payload: OrchestrationPayload::Accounting(Box::new(legacy)),
    })
    .unwrap();
    let expected_bytes = serde_json::to_vec(&expected).unwrap();
    let hash = kinewright_media::sha256_bytes(&expected_bytes);
    println!("TE1_BASE_WAKE_USAGE {hash}");
    assert_eq!(
        serde_json::to_vec(&actual).unwrap(),
        expected_bytes,
        "TE1_WAKE_ACCOUNTING_SURFACE"
    );
    let frozen: String =
        serde_json::from_str(include_str!("../../tests/fixtures/te1-wake.json")).unwrap();
    assert_eq!(hash, frozen);
}
