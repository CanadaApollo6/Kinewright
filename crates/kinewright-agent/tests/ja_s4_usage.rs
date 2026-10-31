//! Isolated C1 accounting/publication contracts. Durable H1 reopen is separate.
//!
//! Rust adaptation of <https://github.com/pingdotgg/t3code> at
//! 5d91ab1117ad51ef1288a94717d85c70e7af09fe, apps/server/src/orchestration-v2/
//! {ProviderEventIngestor,EventSink}.ts. Notice: LICENSES/t3code-MIT.txt.
//! Kinewright adds task-wide allowances and exact acknowledged-send admission.
#![allow(
    dead_code,
    reason = "isolated receiving ports include other C1 contracts"
)]

#[path = "../src/orchestration/limits.rs"]
mod limits;
use kinewright_agent::orchestration;

use kinewright_project::{
    bundle_records::{ObjectRef, RecordKind},
    orchestration_accounting::*,
    receipts::CommandId,
};
use limits::usage::{ClockObservation, PreparedSend, ingest_usage};
use orchestration::{orchestration_commit::*, orchestration_records::*};

fn id<T: serde::de::DeserializeOwned>(n: u64) -> T {
    serde_json::from_value(serde_json::json!(format!("{n:032x}"))).unwrap()
}
fn reference(bytes: &[u8]) -> ObjectRef {
    ObjectRef {
        sha256: kinewright_media::sha256_bytes(bytes),
        byte_length: bytes.len() as u64,
        record_kind: RecordKind::OwnedData,
    }
}
fn provider(attempt: u64) -> ProviderMetadata {
    ProviderMetadata {
        attempt: id(attempt),
        requested: ProviderSelection {
            connector: "subscription".into(),
            model: "requested".into(),
            options: vec![("effort".into(), "max".into())],
        },
        resolved: ProviderSelection {
            connector: "certified-subscription".into(),
            model: "resolved".into(),
            options: vec![("effort".into(), "max".into())],
        },
        cli_version: "fixture-1.2.3".into(),
    }
}
fn accounting(limit: Option<u64>) -> AccountingRecord {
    let mut record = AccountingRecord::new(id(1), id(2), 1000).unwrap();
    record.add_task(id(3), limit).unwrap();
    for n in [4, 5] {
        record
            .task_mut(&id(3))
            .unwrap()
            .select_provider(provider(n))
            .unwrap();
    }
    record
}
fn sample(
    attempt: u64,
    turn: u64,
    event: u64,
    mode: UsageMode,
    input: Option<u64>,
    output: Option<u64>,
) -> UsageSample {
    UsageSample {
        attempt: id(attempt),
        turn,
        event: format!("event-{event}"),
        ordinal: event,
        mode,
        values: UsageValues {
            input,
            output,
            ..UsageValues::default()
        },
        complete: true,
        input_includes_cache: true,
        output_includes_reasoning: true,
    }
}
fn send(n: u64, attempt: u64) -> SendConsumption {
    SendConsumption {
        command: CommandId {
            issuer: id(9),
            sequence: n,
        },
        attempt: id(attempt),
        turn: n,
        payload: reference(b"one admitted prompt"),
    }
}

#[test]
fn s4_first_unreported_turn_blocks_next_send() {
    let mut record = accounting(Some(100));
    let task = record.task_mut(&id(3)).unwrap();
    task.consume_send(send(1, 4)).unwrap();
    assert_eq!(
        task.consume_send(send(2, 4)),
        Err(AccountingRefusal::UsageUnavailable)
    );
    assert_eq!(task.sends, vec![send(1, 4)]);
}

#[test]
fn s4_unreported_turn_after_settlement_blocks_next_send() {
    let mut record = accounting(Some(100));
    let task = record.task_mut(&id(3)).unwrap();
    task.consume_send(send(1, 4)).unwrap();
    ingest_usage(
        task,
        sample(4, 1, 1, UsageMode::Cumulative, Some(2), Some(3)),
    )
    .unwrap();
    task.consume_send(send(2, 4)).unwrap();
    assert!(task.totals.tokens_complete);
    assert_eq!(
        task.consume_send(send(3, 5)),
        Err(AccountingRefusal::UsageUnavailable)
    );
}

#[test]
fn s4_usage_settlement_requires_acknowledged_matching_turn() {
    let mut previous = accounting(Some(100));
    previous.tasks[0].consume_send(send(1, 4)).unwrap();
    let mut next = previous.successor().unwrap();
    ingest_usage(
        &mut next.tasks[0],
        sample(4, 1, 1, UsageMode::Cumulative, Some(2), Some(3)),
    )
    .unwrap();
    next.tasks[0].consume_send(send(2, 4)).unwrap();
    assert_eq!(
        next.validate_successor(&previous),
        Err(AccountingRefusal::UsageUnavailable),
        "usage and the next send cannot share an unacknowledged cut"
    );
    next.tasks[0].sends.pop();
    next.validate_successor(&previous).unwrap();
    let mut send_after_ack = next.successor().unwrap();
    send_after_ack.tasks[0].consume_send(send(2, 4)).unwrap();
    send_after_ack.validate_successor(&next).unwrap();
    let mut unrelated = previous.successor().unwrap();
    ingest_usage(
        &mut unrelated.tasks[0],
        sample(5, 1, 1, UsageMode::Cumulative, Some(2), Some(3)),
    )
    .unwrap();
    unrelated.validate_successor(&previous).unwrap();
    assert_eq!(
        unrelated.tasks[0].consume_send(send(2, 5)),
        Err(AccountingRefusal::UsageUnavailable)
    );
    let mut relabelled = send_after_ack.clone();
    relabelled.tasks[0].sends[0].turn = 9;
    assert_eq!(
        relabelled.validate_successor(&next),
        Err(AccountingRefusal::Scope)
    );
}

#[test]
fn s4_reply_before_usage_publication_crash_child() {
    use std::io::{BufRead, Write};
    struct ReplySession(std::process::Child);
    impl Drop for ReplySession {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    impl kinewright_core::AgentSession for ReplySession {
        fn send_user_message(&mut self, text: String) -> Result<(), kinewright_core::AgentError> {
            writeln!(self.0.stdin.as_mut().unwrap(), "{text}").unwrap();
            let mut reply = String::new();
            std::io::BufReader::new(self.0.stdout.as_mut().unwrap())
                .read_line(&mut reply)
                .unwrap();
            assert_eq!(reply, "provider replied\n");
            assert!(self.0.wait().unwrap().success());
            Ok(())
        }
        fn events(&self) -> crossbeam_channel::Receiver<kinewright_core::AgentEvent> {
            crossbeam_channel::bounded(1).1
        }
        fn interrupt(&mut self) {}
    }
    let Some(root) = std::env::var_os("C5_ACCOUNTING_CRASH_ROOT") else {
        return;
    };
    let root = std::path::Path::new(&root);
    let fixture = root.join(format!("reply-fixture{}", std::env::consts::EXE_SUFFIX));
    assert!(
        std::process::Command::new("rustc")
            .args(["--edition", "2024", "-C", "debuginfo=0"])
            .arg(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/ja_s4_c5_pipe.rs")
            )
            .arg("-o")
            .arg(&fixture)
            .status()
            .unwrap()
            .success()
    );
    let current = accounting(Some(100));
    let envelope = envelope();
    let prepared = PreparedSend::new(&current, &id(3), send(1, 4), &envelope, clock()).unwrap();
    let cut = update(&envelope, prepared.reference().clone());
    let mut committed = std::fs::File::create(root.join("acknowledged-accounting.json")).unwrap();
    committed
        .write_all(&encode(prepared.record(), RECORD_BYTES).unwrap())
        .unwrap();
    committed.sync_all().unwrap();
    let permit = prepared.acknowledge(&cut, cut.receipt()).unwrap();
    let mut session = ReplySession(
        std::process::Command::new(&fixture)
            .arg("reply")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    );
    permit
        .dispatch(&id(4), &mut session, "one admitted prompt".into())
        .unwrap();
    drop(session);
    std::fs::write(root.join("provider-replied"), b"").unwrap();
    std::process::exit(86);
}

#[test]
fn s4_reply_before_usage_publication_crash_blocks_new_attempt() {
    let parent = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    let root = kinewright_media::test_support::TempDirectory::new_in(parent, "c5-usage-crash");
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "s4_reply_before_usage_publication_crash_child",
            "--test-threads=1",
        ])
        .env("C5_ACCOUNTING_CRASH_ROOT", root.root())
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(86),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(root.path("provider-replied").exists());
    let mut acknowledged: AccountingRecord =
        serde_json::from_slice(&std::fs::read(root.path("acknowledged-accounting.json")).unwrap())
            .unwrap();
    acknowledged.validate().unwrap();
    assert!(acknowledged.tasks[0].usage.is_empty());
    assert_eq!(
        acknowledged
            .task_mut(&id(3))
            .unwrap()
            .consume_send(send(2, 5)),
        Err(AccountingRefusal::UsageUnavailable)
    );
}

#[test]
fn s4_usage_two_attempts_and_restart() {
    let mut record = accounting(None);
    let task = record.task_mut(&id(3)).unwrap();
    let mut first = sample(4, 1, 1, UsageMode::Incremental, Some(100), Some(20));
    first.values.cache_read = Some(40);
    first.values.reasoning = Some(5);
    first.complete = false;
    assert!(ingest_usage(task, first).unwrap());
    let mut cumulative = sample(4, 1, 2, UsageMode::Cumulative, Some(140), Some(30));
    cumulative.values.cache_read = Some(50);
    cumulative.values.reasoning = Some(8);
    assert!(ingest_usage(task, cumulative.clone()).unwrap());
    assert!(!ingest_usage(task, cumulative).unwrap());
    assert!(ingest_usage(task, sample(4, 2, 1, UsageMode::Cumulative, Some(80), None)).unwrap());
    assert_eq!(task.totals.known.input, Some(220));
    assert_eq!(task.totals.known.output, Some(30));
    assert!(!task.totals.complete[1]);
    assert_eq!(task.totals.known.cost_micros, None);
    task.wait_reason = Some("provider disconnected; partial totals retained".into());
    let saved = encode(&record, RECORD_BYTES).unwrap();
    let mut reopened: AccountingRecord = serde_json::from_slice(&saved).unwrap();
    reopened.validate().unwrap();
    assert_eq!(reopened, record);
    let task = reopened.task_mut(&id(3)).unwrap();
    let mut rest = sample(4, 2, 2, UsageMode::Cumulative, Some(100), Some(10));
    rest.values.reasoning = Some(3);
    ingest_usage(task, rest).unwrap();
    let mut second = sample(5, 1, 1, UsageMode::Incremental, Some(12), Some(4));
    second.values.cache_write = Some(2);
    second.values.reasoning = Some(2);
    second.values.cost_micros = Some(5000);
    second.input_includes_cache = false;
    ingest_usage(task, second.clone()).unwrap();
    assert!(!ingest_usage(task, second).unwrap());
    assert_eq!(
        task.totals.known.values(),
        [Some(252), Some(44), Some(50), Some(2), Some(13), Some(5000)]
    );
    assert_eq!(task.totals.token_lower_bound, 298);
    assert!(!task.totals.complete[5]);
    assert_eq!(task.providers, vec![provider(4), provider(5)]);
    reopened.validate().unwrap();
    let payload = OrchestrationRecord {
        version: 1,
        payload: OrchestrationPayload::Accounting(Box::new(reopened)),
    };
    assert!(payload.dependencies().unwrap().is_empty());
}

#[test]
fn s4_raw_usage_preserves_unavailable_and_subset_categories() {
    use limits::usage::{UsageAdapter, decode_usage};
    let identity = sample(4, 1, 1, UsageMode::Cumulative, None, None);
    let absent = decode_usage(
        UsageAdapter::Claude,
        &serde_json::json!({"usage":{}}),
        identity.clone(),
    )
    .unwrap();
    assert_eq!(absent.values, UsageValues::default());
    let claude=decode_usage(UsageAdapter::Claude,&serde_json::json!({"usage":{"input_tokens":10,"output_tokens":4,"cache_read_input_tokens":8,"reasoning_output_tokens":3}}),identity.clone()).unwrap();
    assert_eq!(normalize(&[claude]).unwrap().token_lower_bound, 22);
    let codex=decode_usage(UsageAdapter::Codex,&serde_json::json!({"usage":{"input_tokens":10,"output_tokens":4,"input_tokens_details":{"cached_tokens":8},"output_tokens_details":{"reasoning_tokens":3}}}),identity.clone()).unwrap();
    assert_eq!(normalize(&[codex]).unwrap().token_lower_bound, 14);
    let muse=decode_usage(UsageAdapter::Muse,&serde_json::json!({"promptTokens":10,"usage":{"inputTokens":90,"outputTokens":4,"cacheReadTokens":8,"reasoningTokens":3}}),identity).unwrap();
    assert_eq!(normalize(&[muse]).unwrap().token_lower_bound, 14);
}

#[test]
fn s4_registered_accounting_bytes_and_replacement_contract() {
    let current = accounting(None);
    let wrapped = OrchestrationRecord {
        version: 1,
        payload: OrchestrationPayload::Accounting(Box::new(current.clone())),
    };
    let bytes = serde_json::to_vec(&wrapped).unwrap();
    assert_eq!(current.record_bytes().unwrap(), bytes);
    let reference = current.record_reference().unwrap();
    assert_eq!(reference.sha256, kinewright_media::sha256_bytes(&bytes));
    assert_eq!(reference.byte_length, bytes.len() as u64);
    let next = current.successor().unwrap();
    assert_eq!(
        next.dependencies().unwrap(),
        current.dependencies().unwrap()
    );
    next.validate_successor(&current).unwrap();
}

#[test]
fn s4_missing_incremental_usage_cannot_restore_token_allowance() {
    let mut previous = accounting(Some(21));
    let task = previous.task_mut(&id(3)).unwrap();
    let mut first = sample(4, 1, 1, UsageMode::Incremental, Some(10), Some(2));
    first.complete = false;
    ingest_usage(task, first).unwrap();
    ingest_usage(task, sample(4, 1, 2, UsageMode::Incremental, None, Some(3))).unwrap();
    assert!(!task.totals.tokens_complete);
    assert_eq!(task.pause, Some(AccountingRefusal::UsageUnavailable));
    assert_eq!(
        task.consume_send(send(1, 4)),
        Err(AccountingRefusal::UsageUnavailable)
    );
    let mut next = previous.successor().unwrap();
    let task = next.task_mut(&id(3)).unwrap();
    ingest_usage(
        task,
        sample(4, 1, 3, UsageMode::Cumulative, Some(30), Some(5)),
    )
    .unwrap();
    assert!(task.totals.tokens_complete);
    assert_eq!(task.totals.token_lower_bound, 35);
    assert_eq!(task.pause, Some(AccountingRefusal::UsageUnavailable));
    next.validate_successor(&previous).unwrap();
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one witness checks allowances across attempts and restart"
)]
fn s4_limits_do_not_reset() {
    let mut record = accounting(None);
    for n in 1..=40 {
        record
            .task_mut(&id(3))
            .unwrap()
            .consume_send(send(n, 4))
            .unwrap();
    }
    let mut record: AccountingRecord =
        serde_json::from_slice(&encode(&record, RECORD_BYTES).unwrap()).unwrap();
    for n in 41..=80 {
        record
            .task_mut(&id(3))
            .unwrap()
            .consume_send(send(n, 5))
            .unwrap();
    }
    let task = record.task_mut(&id(3)).unwrap();
    let accepted = task.sends.clone();
    assert_eq!(
        task.consume_send(send(81, 5)),
        Err(AccountingRefusal::Sends)
    );
    assert_eq!(task.sends, accepted);
    task.wait_reason = Some("rate_limit; quota is provider-owned and unavailable".into());
    assert_eq!(
        record.observe_clock(1000 + RUN_MILLIS, 0, true),
        Err(AccountingRefusal::Time)
    );
    let expired: AccountingRecord =
        serde_json::from_slice(&encode(&record, RECORD_BYTES).unwrap()).unwrap();
    assert_eq!(expired.check_clock(), Err(AccountingRefusal::Time));
    assert_eq!(expired.tasks[0].sends, accepted);
    let mut rollback = accounting(None);
    assert_eq!(
        rollback.observe_clock(999, 1, true),
        Err(AccountingRefusal::ClockUncertain)
    );
    assert_eq!(
        rollback.observe_clock(1001, 0, true),
        Err(AccountingRefusal::ClockUncertain)
    );
    let mut downtime = accounting(None);
    downtime
        .observe_clock(1000 + RUN_MILLIS - 1, 0, true)
        .unwrap();
    assert_eq!(
        downtime.observe_clock(1000 + RUN_MILLIS - 1, 1, true),
        Err(AccountingRefusal::Time)
    );
    let mut tokens = accounting(Some(21));
    let task = tokens.task_mut(&id(3)).unwrap();
    task.consume_send(send(1, 4)).unwrap();
    ingest_usage(
        task,
        sample(4, 1, 1, UsageMode::Cumulative, Some(20), Some(12)),
    )
    .unwrap();
    assert_eq!(
        task.consume_send(send(2, 4)),
        Err(AccountingRefusal::Tokens)
    );
    assert_eq!(
        task.totals.token_lower_bound, 32,
        "one completed-turn overshoot is retained"
    );
    let providers = task.providers.clone();
    assert_eq!(task.providers, providers);
    let mut repairs = accounting(None);
    let task = repairs.task_mut(&id(3)).unwrap();
    task.repair(&id(40), &id(4), "a".repeat(64)).unwrap();
    assert_eq!(
        task.repair(&id(40), &id(4), "a".repeat(64)),
        Err(AccountingRefusal::Replan)
    );
    assert_eq!(task.repairs[0].rounds, 2);
    assert_eq!(
        task.repair(&id(40), &id(4), "b".repeat(64)),
        Err(AccountingRefusal::Replan)
    );
    assert_eq!(task.repairs[0].rounds, 2);
    let mut across = accounting(None);
    let task = across.task_mut(&id(3)).unwrap();
    task.repair(&id(40), &id(4), "c".repeat(64)).unwrap();
    assert_eq!(
        task.repair(&id(40), &id(5), "c".repeat(64)),
        Err(AccountingRefusal::Replan)
    );
    let mut different = accounting(None);
    let task = different.task_mut(&id(3)).unwrap();
    task.repair(&id(40), &id(4), "c".repeat(64)).unwrap();
    task.repair(&id(40), &id(4), "d".repeat(64)).unwrap();
    assert_eq!(
        task.repair(&id(40), &id(4), "e".repeat(64)),
        Err(AccountingRefusal::Replan)
    );
    let mut missing = accounting(Some(50));
    let task = missing.task_mut(&id(3)).unwrap();
    ingest_usage(task, sample(4, 1, 1, UsageMode::Cumulative, Some(20), None)).unwrap();
    assert_eq!(
        task.consume_send(send(1, 4)),
        Err(AccountingRefusal::UsageUnavailable)
    );
    assert_eq!(task.totals.known.output, None);
}

fn envelope() -> OrchestrationEnvelope {
    let mut envelope = OrchestrationEnvelope {
        id: CommandId {
            issuer: id(9),
            sequence: 1,
        },
        digest: serde_json::from_value(serde_json::json!("0".repeat(64))).unwrap(),
        project: id(1),
        expected_generation: 7,
        authority: AuthorityBinding {
            grant_ref: reference(b"trusted grant"),
            owner_epoch: 2,
            endpoint: Some(id(20)),
            attempt: Some(id(4)),
        },
        command: OrchestrationCommand::ControlRun(RunIntent {
            command_id: CommandId {
                issuer: id(9),
                sequence: 1,
            },
            run_id: id(2),
            expected_control_revision: id(6),
            action: RunAction::SendMessage {
                text_ref: reference(b"one admitted prompt"),
            },
        }),
    };
    envelope.digest = envelope.computed_digest().unwrap();
    envelope
}
fn typed_reference(bytes: &[u8], kind: RecordKind) -> ObjectRef {
    let mut reference = reference(bytes);
    reference.record_kind = kind;
    reference
}
fn clock() -> ClockObservation {
    ClockObservation {
        unix_ms: 1000,
        monotonic_delta_ms: 0,
        trusted: true,
    }
}
fn update(envelope: &OrchestrationEnvelope, accounting_ref: ObjectRef) -> ValidatedRunUpdate {
    let projection = RunProjection {
        run: id(2),
        sequence: 1,
        control_revision: id(6),
        phase: RunPhase::Running,
        outcome: RunOutcome::Pending,
        wait: RunWait::None,
        cleanup: RunCleanup::Pending(vec![CleanupIdentity {
            attempt: id(4),
            nonce: id(11),
            owner_instance: id(12),
            recovery_ref: reference(b"recovery"),
        }]),
        tasks: vec![TaskRecord {
            id: id(3),
            parent: None,
            progress: TaskProgress::default(),
            state: TaskState::Running(id(4)),
            brief: TaskBrief {
                plan: typed_reference(b"plan", RecordKind::OwnedScript),
                script: typed_reference(b"script", RecordKind::OwnedScript),
                inputs: typed_reference(b"inputs", RecordKind::InputManifest),
                assignment: reference(b"assignment"),
                role: Role::Director,
                provider: ProviderSelectionRecord {
                    connector: "fixture".into(),
                    model: "fixture".into(),
                    options: vec![],
                },
                scope: TaskScope {
                    operations: std::collections::BTreeSet::new(),
                    objects: std::collections::BTreeSet::new(),
                    fields: std::collections::BTreeSet::new(),
                    tracks: std::collections::BTreeSet::new(),
                    bus_sidechain_closure: std::collections::BTreeSet::new(),
                    asset_roots: vec![],
                    consent_refs: vec![],
                    base: BranchBinding {
                        branch: id(30),
                        head: id(31),
                        checkpoint: typed_reference(b"base", RecordKind::BranchCheckpoint),
                        document_hash: "a".repeat(64),
                    },
                },
            },
        }],
        effects: vec![],
        attempts: vec![AttemptReservation {
            identity: ExecutionIdentity {
                version: 1,
                project: id(1),
                run: id(2),
                task: id(3),
                attempt: id(4),
                nonce: id(11),
                owner_instance: id(12),
            },
            owner_epoch: 2,
            state: AttemptState::Executing,
            recovery_ref: reference(b"recovery"),
        }],
        deliveries: vec![],
        requests: vec![],
        results: vec![],
        usage: vec![accounting_ref],
    };
    let segment = EventSegment {
        run: id(2),
        first_sequence: 1,
        previous: None,
        events: vec![OrchestrationEvent {
            command: envelope.id.clone(),
            sequence: 1,
            projection: projection.clone(),
        }],
    };
    ValidatedRunUpdate::new(
        &id(1),
        7,
        envelope,
        projection,
        segment,
        CommandDecision::Applied,
    )
    .unwrap()
}
struct CountingSession {
    sends: Vec<String>,
}
impl kinewright_core::AgentSession for CountingSession {
    fn send_user_message(&mut self, text: String) -> Result<(), kinewright_core::AgentError> {
        self.sends.push(text);
        Ok(())
    }
    fn events(&self) -> crossbeam_channel::Receiver<kinewright_core::AgentEvent> {
        crossbeam_channel::bounded(1).1
    }
    fn interrupt(&mut self) {}
}

#[test]
fn s4_allowance_before_send_contract() {
    let current = accounting(None);
    let envelope = envelope();
    assert!(
        PreparedSend::new(
            &current,
            &id(3),
            send(1, 4),
            &envelope,
            ClockObservation {
                unix_ms: 1000 + RUN_MILLIS,
                monotonic_delta_ms: 0,
                trusted: true
            }
        )
        .is_err()
    );
    let prepared = PreparedSend::new(&current, &id(3), send(1, 4), &envelope, clock()).unwrap();
    let committed = prepared.record().clone();
    let exact = update(&envelope, prepared.reference().clone());
    let mut wrong = exact.receipt().clone();
    wrong.cut_digest = "f".repeat(64);
    assert!(prepared.acknowledge(&exact, &wrong).is_err());
    let prepared = PreparedSend::new(&current, &id(3), send(1, 4), &envelope, clock()).unwrap();
    let permit = prepared.acknowledge(&exact, exact.receipt()).unwrap();
    let mut session = CountingSession { sends: vec![] };
    permit
        .dispatch(&id(4), &mut session, "one admitted prompt".into())
        .unwrap();
    assert_eq!(session.sends, vec!["one admitted prompt"]);
    assert_eq!(committed.tasks[0].sends, vec![send(1, 4)]);
    assert!(PreparedSend::new(&committed, &id(3), send(1, 4), &envelope, clock()).is_err());
    let prepared = PreparedSend::new(&current, &id(3), send(1, 4), &envelope, clock()).unwrap();
    let missing = update(&envelope, reference(b"wrong accounting record"));
    assert!(prepared.acknowledge(&missing, missing.receipt()).is_err());
    assert_eq!(session.sends.len(), 1);
}

#[test]
#[ignore = "S3 H1 durable orchestration publisher/reopen is not integrated"]
fn s4_usage_durable_reopen_h1() {
    panic!(
        "H1 must replace this explicit prerequisite gap with bundle publication and fresh-open assertions"
    );
}
