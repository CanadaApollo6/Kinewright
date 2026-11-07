#[test]
#[allow(
    clippy::too_many_lines,
    reason = "fixed complete wakes cover pending, recovered and unpinned child publications"
)]
fn te12_full_director_wake_moves_only_reply_metadata() {
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
    let fixture_state = f.projection(&run);
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
        let legacy_payload = owner.result_wake_payload(&[&stored.reference]).unwrap();
        let mut wake_state = fixture_state.clone();
        wake_state.run = crate::orchestration::orchestration_records::RunId(id(2));
        let mut child = task.clone();
        child.id = crate::orchestration::orchestration_records::TaskId(id(8));
        child.parent = Some(task.id.clone());
        child.state = TaskState::Terminal(stored.reference.clone());
        wake_state.tasks = vec![task.clone(), child];
        let payload = owner
            .context_result_wake(&wake_state, &task.id, &[&stored.reference])
            .unwrap();
        let prompt = owner
            .codex_prompt(&task, &payload, &te12_fixed_context())
            .unwrap();
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
        let baseline: &[u8] = match ordinal {
            3 => include_bytes!("../../tests/fixtures/te12-director-wake-pending.json"),
            4 => include_bytes!("../../tests/fixtures/te12-director-wake-complete.json"),
            _ => include_bytes!("../../tests/fixtures/te12-director-wake-unpinned.json"),
        };
        let mut legacy: serde_json::Value = serde_json::from_slice(baseline).unwrap();
        legacy["live"] = te12_fixed_context();
        let rendered: serde_json::Value = serde_json::from_slice(&prompt).unwrap();
        let wake: serde_json::Value =
            serde_json::from_str(rendered["data"]["payload"].as_str().unwrap()).unwrap();
        assert_eq!(wake["mode"], "full", "TE14_GOLDEN_FULL_WAKE");
        assert_eq!(
            serde_json::to_vec(&wake["full_records"]).unwrap(),
            legacy_payload,
            "TE12_ONLY_REPLY_METADATA_MOVED_{name}"
        );
        assert_eq!(
            legacy_payload,
            legacy["payload"].as_str().unwrap().as_bytes(),
            "TE14_LEGACY_FULL_RECORD_BYTES_{name}"
        );
        println!(
            "TE14_FULL_WAKE_SIZE {name} payload_before={} payload_after={} prompt_before={} prompt_after={}",
            legacy_payload.len(),
            payload.len(),
            baseline.len(),
            prompt.len()
        );
        if let Some(path) = std::env::var_os("KINEWRIGHT_TE14_CAPTURE") {
            std::fs::write(
                std::path::Path::new(&path).join(format!("te14-director-wake-{name}.json")),
                &prompt,
            )
            .unwrap();
        } else {
            let expected: &[u8] = match ordinal {
                3 => include_bytes!("../../tests/fixtures/te14-director-wake-pending.json"),
                4 => include_bytes!("../../tests/fixtures/te14-director-wake-complete.json"),
                _ => include_bytes!("../../tests/fixtures/te14-director-wake-unpinned.json"),
            };
            assert_eq!(prompt, expected, "TE1_COMPLETE_PROVIDER_WAKE_{name}");
        }
        if ordinal == 5 {
            let mut large = result.clone();
            large.explanation = owner
                .owner_mut()
                .unwrap()
                .admit_host_grant(&vec![b'x'; 24_000])
                .unwrap();
            let summary_ref = owner
                .owner_mut()
                .unwrap()
                .admit_scheduler_record(&OrchestrationRecord {
                    version: 1,
                    payload: OrchestrationPayload::Result(Box::new(large)),
                })
                .unwrap()
                .reference;
            wake_state.tasks[1].state = TaskState::Terminal(summary_ref.clone());
            let before = owner.result_wake_payload(&[&summary_ref]).unwrap();
            let after = owner
                .context_result_wake(&wake_state, &task.id, &[&summary_ref])
                .unwrap();
            let parsed: serde_json::Value = serde_json::from_slice(&after).unwrap();
            assert_eq!(parsed["mode"], "summary", "TE14_GOLDEN_SUMMARY_WAKE");
            assert_eq!(
                parsed["omitted_details"], true,
                "TE14_GOLDEN_SUMMARY_OMISSION"
            );
            let mut old_prompt = legacy.clone();
            old_prompt["payload"] = serde_json::json!(std::str::from_utf8(&before).unwrap());
            let old_prompt = serde_json::to_vec(&old_prompt).unwrap();
            let new_prompt = owner
                .codex_prompt(&task, &after, &te12_fixed_context())
                .unwrap();
            let sizes = serde_json::json!({"full_payload_before":legacy_payload.len(),"full_payload_after":payload.len(),"full_prompt_before":baseline.len(),"full_prompt_after":prompt.len(),"summary_payload_before":before.len(),"summary_payload_after":after.len(),"summary_prompt_before":old_prompt.len(),"summary_prompt_after":new_prompt.len()});
            println!("TE14_WAKE_SIZES {sizes}");
            if let Some(path) = std::env::var_os("KINEWRIGHT_TE14_CAPTURE") {
                let path = std::path::Path::new(&path);
                std::fs::write(path.join("te14-director-wake-summary.json"), &new_prompt).unwrap();
                std::fs::write(
                    path.join("te14-wake-sizes.json"),
                    serde_json::to_vec(&sizes).unwrap(),
                )
                .unwrap();
            } else {
                assert_eq!(
                    new_prompt,
                    include_bytes!("../../tests/fixtures/te14-director-wake-summary.json"),
                    "TE14_FULL_PRODUCTION_SUMMARY_GOLDEN"
                );
                assert_eq!(
                    sizes,
                    serde_json::from_slice::<serde_json::Value>(include_bytes!(
                        "../../tests/fixtures/te14-wake-sizes.json"
                    ))
                    .unwrap(),
                    "TE14_WAKE_SIZE_GOLDEN"
                );
            }
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
