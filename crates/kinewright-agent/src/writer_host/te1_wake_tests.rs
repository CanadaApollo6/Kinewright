#[test]
#[allow(
    clippy::too_many_lines,
    reason = "fixed complete wakes cover pending, recovered and unpinned child publications"
)]
fn te1_full_director_wake_matches_b4e9abc8() {
    use crate::protocol::limits::usage::{UsageAdapter, decode_usage, ingest_usage};
    use kinewright_project::{bundle_records::SessionId, orchestration_accounting::*};
    let observed = true;
    let id = |n: u64| {
        serde_json::from_value::<SessionId>(serde_json::json!(format!("{n:032x}"))).unwrap()
    };
    let f = Fixture::new();
    let (run, _, _) = reserve(&f);
    let mut task = f.projection(&run).tasks[0].clone();
    task.id = serde_json::from_value(serde_json::json!(format!("{:032x}", 3))).unwrap();
    task.brief.provider.connector = "codex".into();
    let evidence = task.brief.plan.clone();
    task.brief.inputs = evidence.clone();
    task.brief.scope.base.branch =
        serde_json::from_value(serde_json::json!(format!("{:032x}", 5))).unwrap();
    task.brief.scope.base.head =
        serde_json::from_value(serde_json::json!(format!("{:032x}", 6))).unwrap();
    task.brief.scope.base.checkpoint = evidence;
    task.brief.scope.base.document_hash = "7".repeat(64);
    let host = f.host.owner.upgrade().unwrap();
    let mut owner = host.lock().unwrap();
    let mut accounting = AccountingRecord::new(
        serde_json::from_value(serde_json::json!(format!("{:032x}", 1))).unwrap(),
        id(2),
        1000,
    )
    .unwrap();
    accounting.add_task(id(3), None).unwrap();
    let selection = ProviderSelection {
        connector: "codex".into(),
        model: "fixture".into(),
        options: vec![],
    };
    accounting.tasks[0]
        .select_provider(ProviderMetadata {
            attempt: id(4),
            requested: selection.clone(),
            resolved: selection,
            cli_version: "0.166.0".into(),
        })
        .unwrap();
    // TE1_HEAD_BEGIN
    accounting.tasks[0]
        .bind_stream(kinewright_project::usage_stream::StreamBinding {
            attempt: id(4),
            stream: id(4),
            adapter: kinewright_project::usage_stream::Adapter::Codex01660V1,
            resumed: false,
        })
        .unwrap();
    // TE1_HEAD_END
    let explanation = owner
        .owner_mut()
        .unwrap()
        .admit_host_grant(b"fixed child result")
        .unwrap();
    let lines = [
        serde_json::json!({"type":"turn.started"}),
        serde_json::json!({"type":"turn.completed","usage":{"input_tokens":71,"cached_input_tokens":50,"output_tokens":8}}),
        serde_json::json!({"type":"turn.started"}),
        serde_json::json!({"type":"turn.completed","usage":{"input_tokens":90,"cached_input_tokens":65,"output_tokens":15}}),
        serde_json::json!({"type":"future.codex.usage.kind.with.a.long.identifier.that.exceeds.sixty.four.characters", "usage":{"input_tokens":110,"cached_input_tokens":70,"output_tokens":20}}),
    ];
    for (index, raw) in lines.iter().enumerate() {
        let ordinal = index as u64 + 1;
        let kind = raw["type"].as_str().unwrap();
        if kind != "turn.started" || (observed && !accounting.tasks[0].usage.is_empty()) {
            let previous = accounting;
            let mut next = previous.successor().unwrap();
            next.observe_clock(1000, 0, true).unwrap();
            let mut sample = UsageSample {
                // TE1_HEAD_BEGIN
                snapshot: None,
                // TE1_HEAD_END
                attempt: id(4),
                turn: 1,
                event: HostCommandOwner::native_usage_event(kind, raw, ordinal).unwrap(),
                ordinal,
                mode: UsageMode::Cumulative,
                values: UsageValues::default(),
                complete: kind == "turn.completed",
                input_includes_cache: true,
                output_includes_reasoning: true,
            };
            // TE1_HEAD_BEGIN
            sample.snapshot = Some(kinewright_project::usage_stream::Snapshot {
                stream: id(4),
                adapter: kinewright_project::usage_stream::Adapter::Codex01660V1,
                payload_sha256: "0".repeat(64),
                models: vec![],
            });
            // TE1_HEAD_END
            let sample = decode_usage(UsageAdapter::Codex, raw, sample).unwrap();
            ingest_usage(&mut next.tasks[0], sample).unwrap();
            HostCommandOwner::finish_accounting_observation(&previous, &mut next).unwrap();
            next.validate_successor(&previous).unwrap();
            accounting = next;
        }
        // TE1_HEAD_BEGIN
        if ordinal == 2 {
            let previous = accounting;
            let mut next = previous.successor().unwrap();
            next.tasks[0]
                .measurements
                .get_or_insert_with(Default::default)
                .result(kinewright_project::usage_measurement::ToolResult {
                    attempt: id(4),
                    turn: Some(1),
                    provider_call: None,
                    host_call: 0,
                    tool: "task_status".into(),
                    serialized_bytes: 17,
                });
            HostCommandOwner::finish_accounting_observation(&previous, &mut next).unwrap();
            next.validate_successor(&previous).unwrap();
            accounting = next;
        }
        // TE1_HEAD_END
        if ordinal < 3 {
            continue;
        }
        let usage = owner
            .owner_mut()
            .unwrap()
            .admit_scheduler_record(&OrchestrationRecord {
                version: 1,
                payload: OrchestrationPayload::Accounting(Box::new(accounting.clone())),
            })
            .unwrap();
        let result = TaskResult {
            outcome: AttemptOutcome::Completed,
            base: Some(task.brief.scope.base.clone()),
            head: Some(task.brief.scope.base.clone()),
            delta_ref: None,
            artifacts: vec![],
            evidence: vec![],
            usage_ref: usage.reference.clone(),
            explanation: explanation.clone(),
        };
        let stored = owner
            .owner_mut()
            .unwrap()
            .admit_scheduler_record(&OrchestrationRecord {
                version: 1,
                payload: OrchestrationPayload::Result(Box::new(result.clone())),
            })
            .unwrap();
        let payload = owner.result_wake_payload(&[&stored.reference]).unwrap();
        let prompt = owner.codex_prompt(&task, &payload).unwrap();
        let name = match ordinal {
            3 => "pending",
            4 => "complete",
            _ => "unpinned",
        };
        println!(
            "TE1_FULL_WAKE {name} {} bytes {}",
            prompt.len(),
            kinewright_media::sha256_bytes(&prompt)
        );
        if let Some(path) = std::env::var_os("KINEWRIGHT_TE1_CAPTURE_BASE") {
            assert!(!observed, "only the pinned base may create the oracle");
            std::fs::write(
                std::path::Path::new(&path).join(format!("te1-director-wake-{name}.json")),
                &prompt,
            )
            .unwrap();
        } else {
            let expected: &[u8] = if ordinal == 3 {
                include_bytes!("../../tests/fixtures/te1-director-wake-pending.json")
            } else if ordinal == 4 {
                include_bytes!("../../tests/fixtures/te1-director-wake-complete.json")
            } else {
                include_bytes!("../../tests/fixtures/te1-director-wake-unpinned.json")
            };
            assert_eq!(
                std::str::from_utf8(&prompt).unwrap(),
                std::str::from_utf8(expected).unwrap(),
                "TE1_COMPLETE_PROVIDER_WAKE_{name}"
            );
        }
        let original = owner
            .owner()
            .unwrap()
            .store()
            .unwrap()
            .read_run_record(&stored.reference)
            .unwrap();
        assert_eq!(
            original.payload,
            OrchestrationPayload::Result(Box::new(result)),
            "provider projection must not replace host-owned result references"
        );
    }
}
