#!/usr/bin/env python3
"""Run the TE1 accounting and review mutation pairs with byte-exact restoration."""
from pathlib import Path
import hashlib
import json
import os
import re
import shlex
import signal
import subprocess

ROOT = Path(__file__).resolve().parent.parent
OUT = Path(os.environ.get("KINEWRIGHT_TE1_MUTATION_OUT", ROOT / "target/te1/mutations"))
OUT.mkdir(parents=True, exist_ok=True)
for key in ("TMPDIR", "LOCALAPPDATA"):
    path = ROOT / "target" / ("t" if key == "TMPDIR" else "l")
    path.mkdir(exist_ok=True)
    os.environ[key] = str(path)
os.environ["RUST_TEST_THREADS"] = "4"
os.environ["CARGO_TARGET_DIR"] = str(ROOT / "target")


def interrupted(number, _frame):
    raise SystemExit(128 + number)


signal.signal(signal.SIGTERM, interrupted)


def run(name, target, witness, expect_failure):
    args = ["test", "-p", "kinewright-agent", "--features", "eval-harness,slow-tests",
            *target, witness, "--", "--exact", "--nocapture"]
    command = (
        "source scripts/setup-ffmpeg.sh && "
        "flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 "
        "nice -n 10 cargo " + shlex.join(args)
    )
    with (OUT / (name + ".log")).open("w") as output:
        result = subprocess.run(["bash", "-c", command], cwd=ROOT, stdout=output, stderr=subprocess.STDOUT, check=False)
    log = (OUT / (name + ".log")).read_text()
    summary = re.search(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed;.*$", log, re.M)
    assert summary, name + " did not reach a test result"
    expected = ("FAILED", "0", "1") if expect_failure else ("ok", "1", "0")
    assert summary.groups() == expected, name + " must execute exactly its witness"
    assert (result.returncode != 0) == expect_failure, name + " exit status mismatch"
    print(name, summary[0], flush=True)
    return {"exit": result.returncode, "summary": summary[0]}


def raw_overflow_mutation(adapter):
    before = '''            match self.usage.observe(
                kinewright_project::usage_stream::Adapter::ADAPTER,
                &value,
            ) {
                Ok(Some(event)) => events.push(event),
                Ok(None) => {}
                Err(_) => {
                    usage_refused = true;
                    events.push(AgentEvent::Error(
                        "protocol error: raw_usage_refused".into(),
                    ));
                }
            }'''.replace('ADAPTER', adapter)
    after = before.replace('''events.push(AgentEvent::Error(
                        "protocol error: raw_usage_refused".into(),
                    ));''', 'return Err(AgentError::Protocol("raw_usage_refused".into()));')
    return before, after


ACCOUNTING = "crates/kinewright-project/src/orchestration_accounting.rs"
mutations = [
    ("replay", ACCOUNTING, "turn.values = watermark.values()?.values();",
     "turn.values = std::array::from_fn(|index| watermark.values().unwrap().values()[index].map(|value| turn.values[index].unwrap_or(0).saturating_add(value)));",
     ["--test", "te1_usage"], "te1_codex_cumulative_turns_replay_and_missing_optional_categories"),
    ("reset", ACCOUNTING, "binding.resumed != prior.is_some()", "false",
     ["--test", "te1_usage"], "te1_resume_fresh_reset_missing_final_and_changed_identity"),
    ("claude-uuid", "crates/kinewright-agent/src/usage_observer.rs",
     'Adapter::Claude21316V1 => "uuid",', 'Adapter::Claude21316V1 => "id",',
     ["--lib"], "usage_observer::tests::te1_claude_uuid_replay_and_changed_payload_are_checked_at_ingress"),
    ("model-watermarks", ACCOUNTING, "turn.values = watermark.values()?.values();",
     "turn.values = snapshot.values()?.values();", ["--test", "te1_usage"],
     "te1_partial_multi_model_snapshot_retains_each_known_watermark"),
    ("scoreboard-lower-bound", "crates/kinewright-agent/src/eval.rs",
     "results\n            .iter()\n            .map(EvalResult::total_tokens)\n            .fold(0_u64, u64::saturating_add),",
     "totals.input_tokens.saturating_add(totals.output_tokens),",
     ["--lib"], "eval::tests::te1_partial_multi_model_lower_bound_reaches_evaluation_totals"),
    ("pending-sends", "crates/kinewright-agent/src/eval.rs",
     "self.uncovered_sends = self.uncovered_sends.saturating_add(1);", "self.uncovered_sends = 0;",
     ["--lib"], "eval::tests::te1_two_send_missing_final_keeps_coverage_incomplete_for_both_providers"),
    ("payload-collision", "crates/kinewright-agent/src/orchestration/usage.rs",
     "old.snapshot == sample.snapshot", "true", ["--test", "te1_usage"],
     "te1_all_required_categories_and_payload_collisions"),
    ("observer-payload", "crates/kinewright-agent/src/usage_observer.rs",
     "if old != &sample {", "if old.values != sample.values {", ["--lib"],
     "usage_observer::tests::te1_claude_uuid_replay_and_changed_payload_are_checked_at_ingress"),
    ("model-regression", "crates/kinewright-project/src/usage_stream.rs",
     "if a.zip(b).is_some_and(|(a, b)| b < a) {", "if false && a.zip(b).is_some_and(|(a, b)| b < a) {",
     ["--test", "te1_usage"], "te1_partial_multi_model_watermarks_recover_after_restart_and_resume"),
    ("partial-coverage", ACCOUNTING, ".all(|m| m.values.values()[index].is_some())",
     ".all(|m| m.values.values()[index].is_some() || index < 5)",
     ["--test", "te1_usage"], "te1_partial_multi_model_watermarks_recover_after_restart_and_resume"),
    ("wake-events", "crates/kinewright-agent/src/writer_host/provider_usage.rs",
     'format!("{kind}:{}", snapshot.payload_sha256)', 'format!("{kind}:{:016x}", sample.ordinal)',
     ["--lib"], "writer_host::hooks_tests::tests::te1_review_wake_accounting_expansion_is_frozen_at_b4e9abc8"),
    ("wake-snapshots", "crates/kinewright-agent/src/writer_host/provider_usage.rs",
     "sample.snapshot = None;", "let _ = &sample.snapshot;", ["--lib"],
     "writer_host::hooks_tests::tests::te1_review_wake_accounting_expansion_is_frozen_at_b4e9abc8"),
    ("cost-retention", "crates/kinewright-agent/src/orchestration/usage.rs",
     "identity.values.cost_micros = Some(", "identity.values.cost_micros = None; let _: Option<u64> = Some(",
     ["--test", "te1_usage"], "te1_review_stream_cost_is_preserved_once_and_missing_cost_stays_unknown"),
    ("cost-budget", "crates/kinewright-agent/src/eval.rs", "cost > maximum", "false && cost > maximum",
     ["--lib"], "eval::tests::te1_review_raw_cost_is_cumulative_and_enforces_the_eval_ceiling"),
    ("direct-input-coverage", "crates/kinewright-project/src/usage_stream.rs",
     "direct_input: complete.then_some(direct).flatten(),", "direct_input: direct,",
     ["--test", "te1_usage"], "te1_review_stream_report_does_not_present_incomplete_direct_input_as_known"),
    ("native-observer-loss", "crates/kinewright-agent/src/writer_host/task_mcp.rs",
     "measurements.incomplete |= incomplete;", "measurements.incomplete |= incomplete && false;",
     ["--lib"], "writer_host::hooks_tests::tests::te1_review_native_size_observer_survives_missing_host_and_poison"),
    ("native-size-loss", "crates/kinewright-agent/src/server.rs",
     "metrics.incomplete |= !native_recorded;", "metrics.incomplete |= !native_recorded && false;",
     ["--lib"], "writer_host::hooks_tests::tests::te1_review_native_size_loss_is_visible_and_sticky_after_detach"),
    ("serialized-reserve", "crates/kinewright-agent/src/writer_host/native_spending.rs",
     "size > kinewright_project::run_admission::ACCOUNTING_BYTES - 4096",
     "false && size > kinewright_project::run_admission::ACCOUNTING_BYTES - 4096",
     ["--lib"], "writer_host::hooks_tests::tests::te1_perf_serialized_reserve_remains_durable"),
    ("persisted-bindings", ACCOUNTING,
     "self.validate_binding(binding, &self.streams[..index])?;", "let _ = (index, binding);",
     ["--test", "te1_usage"], "te1_persisted_stream_bindings_keep_scope_checks"),
    ("turn-start-noop", "crates/kinewright-agent/src/writer_host/native_spending.rs",
     'if kind == "turn.started"', 'if false && kind == "turn.started"',
     ["--lib"], "writer_host::hooks_tests::tests::te1_native_new_turn_requires_a_new_final_even_when_totals_replay"),
    ("turn-start-gap", "crates/kinewright-agent/src/writer_host/native_spending.rs",
     "task.usage.is_empty()", "true",
     ["--lib"], "writer_host::hooks_tests::tests::te1_native_new_turn_requires_a_new_final_even_when_totals_replay"),

    ("full-wake-reference", "crates/kinewright-agent/src/writer_host/native_wake.rs",
     'provider_result["usage_ref"] = serde_json::to_value(usage.reference)?;', 'let _ = usage.reference;',
     ["--lib"], "writer_host::hooks_tests::tests::te12_full_director_wake_moves_only_reply_metadata"),
    ("full-wake-revision", "crates/kinewright-agent/src/writer_host/provider_usage.rs",
     ".checked_sub(record.observer_publications)", ".checked_sub(0)",
     ["--lib"], "writer_host::hooks_tests::tests::te12_full_director_wake_moves_only_reply_metadata"),
    ("full-wake-totals", "crates/kinewright-agent/src/writer_host/provider_usage.rs",
     "task.totals = normalize(&task.usage).map_err(refused)?;", "let _ = normalize(&task.usage).map_err(refused)?;",
     ["--lib"], "writer_host::hooks_tests::tests::te12_full_director_wake_moves_only_reply_metadata"),
    ("observer-revision", ACCOUNTING,
     "|| self.observer_publications < previous.observer_publications", "|| false",
     ["--test", "te1_usage"], "te1_observer_revision_offset_is_bounded_and_persists"),
    ("full-wake-kind", "crates/kinewright-agent/src/writer_host/native_spending.rs",
     'kind.chars().take(64).collect::<String>(),',
     'match kind { "turn.completed" | "turn.usage" | "turn.started" | "native.eof" => kind.to_owned(), _ => "provider_usage".to_owned() },',
     ["--lib"], "writer_host::hooks_tests::tests::te12_full_director_wake_moves_only_reply_metadata"),

    ('raw-overflow-claude', "crates/kinewright-agent/src/protocol.rs",
     *raw_overflow_mutation('Claude21316V1'),
     ["--lib"], "eval::tests::te1_raw_observer_overflow_keeps_lower_bound_and_session_alive"),
    ('raw-overflow-codex', "crates/kinewright-agent/src/protocol.rs",
     *raw_overflow_mutation('Codex01660V1'),
     ["--lib"], "eval::tests::te1_raw_observer_overflow_keeps_lower_bound_and_session_alive"),
    ("cross-platform-instructions", "crates/kinewright-agent/src/writer_host/codex_instructions.rs",
     "You are a Kinewright task agent.", "You are a Kinewright fixture agent.",
     ["--lib"], "server::te1_surface_tests::te12_current_role_manifests_match_declared_tools_and_schemas"),
    ('zero-final-codex', "crates/kinewright-agent/src/orchestration/usage.rs",
     'if identity.complete\n        && matches!(adapter, UsageAdapter::Claude | UsageAdapter::Codex)', 'if identity.complete\n        && matches!(adapter, UsageAdapter::Claude)',
     ["--test", "te1_usage"], "te1_zero_final_after_accepted_send_is_unobserved_and_refuses_next_send"),
    ('zero-final-claude', "crates/kinewright-agent/src/orchestration/usage.rs",
     'if identity.complete\n        && matches!(adapter, UsageAdapter::Claude | UsageAdapter::Codex)', 'if identity.complete\n        && matches!(adapter, UsageAdapter::Codex)',
     ["--test", "te1_usage"], "te1_zero_final_after_accepted_send_is_unobserved_and_refuses_next_send"),
    ("zero-final-eof", "crates/kinewright-agent/src/writer_host/native_spending.rs",
     '.is_some_and(|s| s.complete)', '.is_some_and(|s| s.complete || s.event.starts_with("turn.completed:"))',
     ["--lib"], "writer_host::hooks_tests::tests::te1_native_zero_final_keeps_eof_gap_open_under_a_ceiling"),
    ("budget-lower-bound", "crates/kinewright-agent/src/eval.rs",
     "let tokens = outcome.session.total_tokens();",
     "let tokens = outcome.session.input_tokens.saturating_add(outcome.session.output_tokens);",
     ["--lib"], "eval::tests::te1_budget_scores_the_retained_118_token_lower_bound"),
    ("observer-regression-rollback", "crates/kinewright-agent/src/usage_observer.rs",
     "self.samples.pop();", "let _ = self.samples.last();",
     ["--lib"], "usage_observer::tests::te1_refused_regression_does_not_poison_later_valid_observations"),
    ("corpus-manual-fork", "crates/kinewright-project/src/editing_session/durable.rs",
     '        let core = Core::spawn(std::sync::Arc::clone(&live.document))\n            .map_err(|e| BundleError::Corrupt(e.to_string()))?;\n        let bundle = self.active_bundle_mut()?;', '        let core = self.core.clone();\n        let bundle = self.active_bundle_mut()?;',
     ["--test", "te1_corpus"], "te1_case1_task_fork_preserves_its_source_manual_history_and_redo"),
]
proof = []
for name, relative, before, after, target, witness in mutations:
    source = ROOT / relative
    original = source.read_bytes()
    before, after = before.encode(), after.encode()
    assert original.count(before) == 1, name + " mutation site drifted"
    (OUT / (name + ".original")).write_bytes(original)
    mutant = original.replace(before, after)
    row = {"name": name, "source": relative, "witness": witness,
           "original_sha256": hashlib.sha256(original).hexdigest(),
           "mutant_sha256": hashlib.sha256(mutant).hexdigest()}
    try:
        source.write_bytes(mutant)
        row["red"] = run(name + "-red", target, witness, True)
    finally:
        source.write_bytes(original)
        restored = source.read_bytes()
        assert restored == original, name + " source restoration changed bytes"
        row["restored_sha256"] = hashlib.sha256(restored).hexdigest()
        (OUT / (name + ".json")).write_text(json.dumps(row, indent=2) + "\n")
    row["green"] = run(name + "-green", target, witness, False)
    assert source.read_bytes() == original, name + " source changed during restored witness"
    (OUT / (name + ".json")).write_text(json.dumps(row, indent=2) + "\n")
    proof.append(row)
    (OUT / "pairs.json").write_text(json.dumps(proof, indent=2) + "\n")
