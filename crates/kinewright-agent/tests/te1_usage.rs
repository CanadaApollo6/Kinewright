//! Frozen hand-computed TE1 usage cases. No installed provider is launched.
#[path = "../src/orchestration/limits.rs"]
mod limits;
use kinewright_agent::orchestration;
use kinewright_project::{bundle_records::SessionId, orchestration_accounting::*, usage_stream::*};
use limits::usage::{UsageAdapter, decode_usage, ingest_usage};
use serde_json::{Value, json};
fn id(n: u64) -> SessionId {
    serde_json::from_value(json!(format!("{n:032x}"))).unwrap()
}
fn task(adapter: Adapter) -> TaskAccounting {
    task_with_limit(adapter, None)
}
fn task_with_limit(adapter: Adapter, token_limit: Option<u64>) -> TaskAccounting {
    let mut r = AccountingRecord::new(
        serde_json::from_value(json!(format!("{:032x}", 1))).unwrap(),
        id(2),
        1000,
    )
    .unwrap();
    r.add_task(id(3), token_limit).unwrap();
    let mut t = r.tasks.remove(0);
    bind(&mut t, adapter, 4, 4, false).unwrap();
    t
}
fn bind(
    t: &mut TaskAccounting,
    adapter: Adapter,
    attempt: u64,
    stream: u64,
    resumed: bool,
) -> Result<(), AccountingRefusal> {
    let selection = ProviderSelection {
        connector: "fixture".into(),
        model: "fixture".into(),
        options: vec![],
    };
    t.select_provider(ProviderMetadata {
        attempt: id(attempt),
        requested: selection.clone(),
        resolved: selection,
        cli_version: "fixture".into(),
    })?;
    t.bind_stream(StreamBinding {
        attempt: id(attempt),
        stream: id(stream),
        adapter,
        resumed,
    })
}
#[allow(
    clippy::needless_pass_by_value,
    reason = "each ingress fixture owns its mutation payload"
)]
fn raw(
    t: &mut TaskAccounting,
    attempt: u64,
    event: u64,
    payload: Value,
    complete: bool,
) -> Result<bool, AccountingRefusal> {
    let binding = t.streams.iter().find(|b| b.attempt == id(attempt)).unwrap();
    let old = t
        .usage
        .iter()
        .find(|s| s.attempt == id(attempt) && s.event == format!("{event:064x}"));
    let sample = decode_usage(
        match binding.adapter {
            Adapter::Codex01660V1 => UsageAdapter::Codex,
            Adapter::Claude21316V1 => UsageAdapter::Claude,
        },
        &payload,
        UsageSample {
            snapshot: Some(Snapshot {
                payload_sha256: "0".repeat(64),
                stream: binding.stream.clone(),
                adapter: binding.adapter,
                models: vec![],
            }),
            attempt: id(attempt),
            turn: event,
            event: format!("{event:064x}"),
            ordinal: old.map_or(t.usage.len() as u64 + 1, |s| s.ordinal),
            mode: UsageMode::Cumulative,
            values: UsageValues::default(),
            complete,
            input_includes_cache: true,
            output_includes_reasoning: true,
        },
    )?;
    ingest_usage(t, sample)
}
fn claude(d: u64, r: u64, w: u64, o: u64) -> Value {
    json!({"type":"result","modelUsage":{"main":{"inputTokens":d,"cacheReadInputTokens":r,"cacheCreationInputTokens":w,"outputTokens":o}}})
}
fn codex(i: u64, r: u64, o: u64) -> Value {
    json!({"type":"turn.completed","usage":{"input_tokens":i,"cached_input_tokens":r,"output_tokens":o}})
}
fn uncached(t: &TaskAccounting) -> Option<u64> {
    t.stream_reports().ok()?.last()?.uncached_input
}
#[test]
fn te1_claude_two_turns_replay_and_compaction() {
    let mut t = task(Adapter::Claude21316V1);
    raw(&mut t, 4, 1, claude(10, 20, 5, 3), true).unwrap();
    assert_eq!(t.totals.token_lower_bound, 38);
    assert_eq!(uncached(&t), Some(15));
    raw(&mut t, 4, 2, claude(14, 50, 7, 8), true).unwrap();
    assert_eq!(t.totals.token_lower_bound, 79);
    assert_eq!(79 - 38, 41);
    assert_eq!(uncached(&t), Some(21));
    assert!(!raw(&mut t, 4, 2, claude(14, 50, 7, 8), true).unwrap());
    raw(&mut t, 4, 3, claude(14, 50, 7, 8), true).unwrap();
    assert_eq!(t.totals.token_lower_bound, 79, "TE1_REPLAY_ADDS_ZERO");
    let mut p = claude(14, 50, 7, 8);
    p["usage"] = json!({"input_tokens":41});
    p["modelUsage"]["helper"] = json!({"inputTokens":2,"cacheReadInputTokens":4,"cacheCreationInputTokens":1,"outputTokens":1});
    raw(&mut t, 4, 4, p, true).unwrap();
    assert_eq!(t.totals.token_lower_bound, 87);
    assert!(t.totals.tokens_complete);
}
#[test]
fn te1_codex_cumulative_turns_replay_and_missing_optional_categories() {
    let mut t = task(Adapter::Codex01660V1);
    raw(&mut t, 4, 1, codex(100, 80, 10), true).unwrap();
    assert_eq!(t.totals.token_lower_bound, 110);
    raw(&mut t, 4, 2, codex(150, 120, 18), true).unwrap();
    assert_eq!(t.totals.token_lower_bound, 168);
    assert_eq!(168 - 110, 58);
    raw(&mut t, 4, 3, codex(150, 120, 18), true).unwrap();
    assert_eq!(t.totals.token_lower_bound, 168, "TE1_REPLAY_ADDS_ZERO");
    assert!(t.totals.tokens_complete);
    assert_eq!(uncached(&t), Some(30));
    assert_eq!(t.totals.known.reasoning, None);
    assert_eq!(t.totals.known.cache_write, None);
}
#[test]
fn te1_resume_fresh_reset_missing_final_and_changed_identity() {
    for (a, first, next, fresh) in [
        (
            Adapter::Claude21316V1,
            claude(14, 50, 7, 8),
            claude(20, 60, 10, 10),
            claude(3, 0, 2, 1),
        ),
        (
            Adapter::Codex01660V1,
            codex(71, 50, 8),
            codex(90, 60, 10),
            codex(5, 0, 1),
        ),
    ] {
        let mut t = task(a);
        raw(&mut t, 4, 1, first.clone(), true).unwrap();
        assert_eq!(t.totals.token_lower_bound, 79);
        let saved = serde_json::to_vec(&t).unwrap();
        t = serde_json::from_slice(&saved).unwrap();
        bind(&mut t, a, 5, 4, true).unwrap();
        t.consume_send(SendConsumption {
            attempt: id(5),
            turn: 1,
            command: serde_json::from_value(json!({"issuer":"8".repeat(32),"sequence":1})).unwrap(),
            payload: serde_json::from_value(
                json!({"sha256":"9".repeat(64),"byte_length":1,"record_kind":"owned_evidence"}),
            )
            .unwrap(),
        })
        .unwrap();
        let mut rebound = t.usage[0].clone();
        rebound.attempt = id(5);
        rebound.ordinal += 1;
        let mut forged_ledger = t.usage.clone();
        forged_ledger.push(rebound);
        assert!(
            normalize(&forged_ledger).is_err(),
            "TE1_SAVED_REPLAY_CANNOT_REBIND_ATTEMPT"
        );
        let replayed = raw(&mut t, 5, 1, first.clone(), true).unwrap();
        let report = &t.stream_reports().unwrap()[0];
        assert!(
            !report.reported_complete,
            "TE1_REPLAY_CANNOT_CLOSE_RESUMED_GAP"
        );
        assert_eq!(report.inclusive_input, None);
        assert_eq!(report.lower_bound, 79);
        assert!(!replayed);
        assert_eq!(t.usage.len(), 1);
        assert!(raw(&mut t, 5, 1, next.clone(), true).is_err());
        raw(&mut t, 5, 2, json!({}), true).unwrap();
        assert!(!t.totals.tokens_complete);
        assert_eq!(t.totals.token_lower_bound, 79);
        raw(&mut t, 5, 3, next.clone(), true).unwrap();
        assert_eq!(t.totals.token_lower_bound, 100);
        assert!(t.totals.tokens_complete);
        assert!(
            raw(&mut t, 5, 3, first.clone(), true).is_err(),
            "TE1_CHANGED_PAYLOAD_REFUSED"
        );
        assert!(
            bind(&mut t, a, 6, 4, false).is_err(),
            "TE1_UNPROVEN_RESET_REFUSED"
        );
        assert!(
            bind(&mut t, a, 7, 99, true).is_err(),
            "TE1_UNBOUND_RESUME_REFUSED"
        );
        bind(&mut t, a, 8, 8, false).unwrap();
        raw(&mut t, 8, 4, fresh, true).unwrap();
        assert_eq!(t.totals.token_lower_bound, 106);
        assert!(
            raw(&mut t, 5, 5, first, true).is_err(),
            "TE1_REGRESSION_REFUSED"
        );
    }
}
#[test]
fn te1_missing_claude_category_is_unknown_with_lower_bound() {
    let mut t = task(Adapter::Claude21316V1);
    let mut p = claude(10, 20, 0, 3);
    p["modelUsage"]["main"]
        .as_object_mut()
        .unwrap()
        .remove("cacheCreationInputTokens");
    p["usage"] = json!({"cache_creation_input_tokens":999});
    raw(&mut t, 4, 1, p, true).unwrap();
    assert_eq!(t.totals.token_lower_bound, 33);
    assert!(!t.totals.tokens_complete);
    assert_eq!(uncached(&t), None);
}
#[test]
fn te1_measurement_overflow_is_incomplete_and_retains_only_sizes() {
    use kinewright_project::usage_measurement::*;
    let mut m = Measurements::default();
    for _ in 0..=DETAILS {
        m.result(ToolResult {
            attempt: id(4),
            turn: None,
            provider_call: None,
            host_call: 0,
            tool: "task_status".into(),
            serialized_bytes: 19,
        });
    }
    assert!(m.incomplete);
    assert_eq!(m.results.len(), DETAILS);
    assert_eq!(m.result_count, 257);
    assert_eq!(m.result_bytes, 4883);
    m.validate().unwrap();
    assert_eq!(m.provider_steps, None);
}

#[test]
fn te1_gap_cannot_erase_a_model_watermark_or_complete_unsampled_work() {
    for (adapter, first, mut gap, lower, restored, category) in [
        (
            Adapter::Claude21316V1,
            claude(14, 50, 7, 8),
            claude(14, 50, 7, 8),
            claude(13, 50, 7, 8),
            claude(20, 60, 10, 10),
            "/modelUsage/main/inputTokens",
        ),
        (
            Adapter::Codex01660V1,
            codex(71, 50, 8),
            codex(71, 50, 8),
            codex(70, 50, 8),
            codex(90, 60, 10),
            "/usage/input_tokens",
        ),
    ] {
        let mut t = task(adapter);
        raw(&mut t, 4, 1, first, true).unwrap();
        *gap.pointer_mut(category).unwrap() = Value::Null;
        raw(&mut t, 4, 2, gap, false).unwrap();
        assert_eq!(t.totals.token_lower_bound, 79);
        assert!(!t.totals.tokens_complete);
        assert!(
            raw(&mut t, 4, 3, lower, true).is_err(),
            "TE1_GAP_RETAINS_WATERMARK"
        );
        raw(&mut t, 4, 4, restored, true).unwrap();
        assert_eq!(t.totals.token_lower_bound, 100);
        assert!(t.stream_reports().unwrap()[0].reported_complete);
        t.consume_send(SendConsumption {
            attempt: id(4),
            turn: 5,
            command: serde_json::from_value(json!({"issuer":"8".repeat(32),"sequence":1})).unwrap(),
            payload: serde_json::from_value(
                json!({"sha256":"9".repeat(64),"byte_length":1,"record_kind":"owned_evidence"}),
            )
            .unwrap(),
        })
        .unwrap();
        let report = &t.stream_reports().unwrap()[0];
        assert!(!report.reported_complete);
        assert_eq!(report.inclusive_input, None);
        assert_eq!(report.lower_bound, 100);
    }
}

#[test]
fn te1_all_required_categories_and_payload_collisions() {
    for (category, lower_bound) in [
        ("inputTokens", 28),
        ("cacheReadInputTokens", 18),
        ("cacheCreationInputTokens", 33),
        ("outputTokens", 35),
    ] {
        let mut t = task(Adapter::Claude21316V1);
        let mut p = claude(10, 20, 5, 3);
        p["modelUsage"]["main"]
            .as_object_mut()
            .unwrap()
            .remove(category);
        raw(&mut t, 4, 1, p, true).unwrap();
        assert!(!t.totals.tokens_complete);
        assert_eq!(t.totals.token_lower_bound, lower_bound);
        assert_eq!(uncached(&t), None);
    }
    let mut t = task(Adapter::Codex01660V1);
    let mut p = codex(150, 120, 18);
    p["usage"]
        .as_object_mut()
        .unwrap()
        .remove("cached_input_tokens");
    raw(&mut t, 4, 1, p.clone(), true).unwrap();
    assert!(t.totals.tokens_complete);
    assert_eq!(t.totals.token_lower_bound, 168);
    assert_eq!(uncached(&t), None);
    p["ignored_provider_text"] = json!("changed payload");
    assert!(
        raw(&mut t, 4, 1, p, true).is_err(),
        "TE1_WHOLE_PAYLOAD_COLLISION_REFUSED"
    );
}

fn two_model_claude(a: Option<u64>, b: u64) -> Value {
    json!({"type":"result","modelUsage":{
        "A":{"inputTokens":a,"cacheReadInputTokens":0,"cacheCreationInputTokens":0,"outputTokens":0},
        "B":{"inputTokens":b,"cacheReadInputTokens":0,"cacheCreationInputTokens":0,"outputTokens":0}
    }})
}

#[test]
fn te1_partial_multi_model_snapshot_retains_each_known_watermark() {
    let mut t = task_with_limit(Adapter::Claude21316V1, Some(225));
    raw(&mut t, 4, 1, two_model_claude(Some(100), 100), true).unwrap();
    assert_eq!(t.totals.token_lower_bound, 200);
    raw(&mut t, 4, 2, two_model_claude(None, 150), false).unwrap();
    assert_eq!(t.totals.known.input, Some(250), "TE1_MODEL_WATERMARK_SUM");
    assert_eq!(t.totals.token_lower_bound, 250);
    assert!(
        t.totals.token_lower_bound >= 225,
        "known ceiling crossing survives a partial snapshot"
    );
    assert_eq!(t.pause, Some(AccountingRefusal::Tokens));
    assert!(!t.totals.tokens_complete);
    assert!(!t.totals.complete[0]);
    assert_eq!(t.stream_reports().unwrap()[0].lower_bound, 250);
}

#[test]
fn te1_partial_multi_model_watermarks_recover_after_restart_and_resume() {
    let mut t = task(Adapter::Claude21316V1);
    raw(&mut t, 4, 1, two_model_claude(Some(100), 100), true).unwrap();
    raw(&mut t, 4, 2, two_model_claude(None, 150), true).unwrap();
    assert_eq!(
        t.totals.token_lower_bound, 250,
        "TE1_MODEL_WATERMARK_RECOVERY"
    );
    assert!(
        !t.totals.tokens_complete,
        "TE1_CURRENT_SNAPSHOT_DEFINES_COMPLETENESS"
    );
    let saved = serde_json::to_vec(&t).unwrap();
    t = serde_json::from_slice(&saved).unwrap();
    bind(&mut t, Adapter::Claude21316V1, 5, 4, true).unwrap();
    raw(&mut t, 5, 3, json!({}), false).unwrap();
    assert_eq!(t.totals.token_lower_bound, 250);
    assert!(
        raw(&mut t, 5, 4, two_model_claude(Some(99), 175), true).is_err(),
        "TE1_MODEL_REGRESSION_REFUSED_AFTER_RECOVERY"
    );
    raw(&mut t, 5, 5, two_model_claude(Some(125), 175), true).unwrap();
    assert_eq!(t.totals.known.input, Some(300));
    assert_eq!(t.totals.token_lower_bound, 300);
    assert!(t.totals.tokens_complete);
    assert_eq!(t.stream_reports().unwrap()[0].inclusive_input, Some(300));
    assert!(t.stream_reports().unwrap()[0].reported_complete);
}

#[test]
fn te1_review_stream_report_does_not_present_incomplete_direct_input_as_known() {
    let mut t = task(Adapter::Claude21316V1);
    raw(&mut t, 4, 1, two_model_claude(Some(100), 100), true).unwrap();
    raw(&mut t, 4, 2, two_model_claude(None, 150), true).unwrap();
    let report = &t.stream_reports().unwrap()[0];
    assert_eq!(report.lower_bound, 250);
    assert_eq!(
        report.direct_input, None,
        "TE1_PARTIAL_DIRECT_INPUT_IS_UNKNOWN"
    );
    assert!(!report.reported_complete);
}

#[test]
fn te1_review_stream_cost_is_preserved_once_and_missing_cost_stays_unknown() {
    for (adapter, mut first, mut next) in [
        (
            Adapter::Claude21316V1,
            two_model_claude(Some(100), 100),
            two_model_claude(Some(125), 175),
        ),
        (
            Adapter::Codex01660V1,
            codex(150, 120, 18),
            codex(200, 160, 20),
        ),
    ] {
        let mut t = task(adapter);
        first["total_cost_usd"] = json!(0.125);
        raw(&mut t, 4, 1, first, true).unwrap();
        assert_eq!(
            t.usage[0].values.cost_micros,
            Some(125_000),
            "TE1_STREAM_COST_IS_RETAINED"
        );
        assert_eq!(t.totals.known.cost_micros, Some(125_000));
        next["total_cost_usd"] = json!(0.25);
        raw(&mut t, 4, 2, next.clone(), true).unwrap();
        assert_eq!(t.totals.known.cost_micros, Some(250_000));
        assert!(t.totals.complete[5]);
        next.as_object_mut().unwrap().remove("total_cost_usd");
        raw(&mut t, 4, 3, next.clone(), true).unwrap();
        assert_eq!(t.totals.known.cost_micros, Some(250_000));
        assert!(!t.totals.complete[5]);
        next["total_cost_usd"] = json!(0.2);
        assert!(raw(&mut t, 4, 4, next.clone(), true).is_err());
        next["total_cost_usd"] = json!(0.3);
        raw(&mut t, 4, 5, next, true).unwrap();
        assert_eq!(t.totals.known.cost_micros, Some(300_000));
        assert!(t.totals.complete[5]);
    }
}
