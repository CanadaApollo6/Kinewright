#!/usr/bin/env python3
"""TE1.3 completion witnesses, real fault mutations and exact byte restores."""
from pathlib import Path
import importlib.util
import os
import sys

sys.dont_write_bytecode = True

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('te13_merge_pairs', ROOT / 'scripts/te13-merge-mutations.py')
harness = importlib.util.module_from_spec(spec)
spec.loader.exec_module(harness)
harness.OUT = Path(os.environ.get('KINEWRIGHT_TE13_MUTATION_OUT', ROOT / 'target/te1-3/feature/te13-pairs'))
A = 'kinewright-agent'
H = 'writer_host::hooks_tests::tests::'
C = 'crates/kinewright-agent/src/writer_host/task_completion.rs'
T = 'crates/kinewright-agent/src/orchestration/tasks.rs'
R = 'crates/kinewright-agent/src/writer_host/receipt_reconciliation.rs'
E = 'crates/kinewright-agent/src/writer_host/effect_execution.rs'
S = 'crates/kinewright-agent/src/server.rs'
harness.PAIRS = []

def add(name, source, before, after, witness, marker):
    harness.PAIRS.append((name, source, before, after, A, H + witness, marker))

add('intent-first', T, 'task.progress.completion = Some(Box::new(intent));',
    'let _ = intent;', 'te13_finish_fuses_freeze_and_publication', 'TE13_DURABLE_PUBLICATION')
add('freeze-is-not-complete', C, 'if published.is_some() && settled {',
    'if freeze_receipt.is_some() && settled {',
    'te13_lost_freeze_reply_replays_original_substeps_after_clock_tick', 'TE13_FREEZE_IS_NOT_COMPLETION')
add('lost-freeze', C, 'if let Some(receipt) = self.owner()?.orchestration_receipt(run, &id)? {',
    'if let Some(receipt) = self.owner()?.orchestration_receipt(run, &id)?.filter(|_| false) {',
    'te13_lost_freeze_reply_replays_original_substeps_after_clock_tick', 'TE13_RETRY_COMPLETES')
add('lost-publication', C, 'Some(result.result.clone())', 'None',
    'te13_lost_publication_reply_returns_original_receipts_and_result', 'TE13_PUBLISH_ORIGINAL_RESULT')
add('delivery-settlement', C, 'delivery.is_some_and(|d| matches!(d.state, DeliveryState::Observed(_)))',
    'delivery.is_some()', 'te13_delivery_must_be_observed_and_lost_reply_is_reconciled', 'TE13_DELIVERY_REQUIRED')
add('lost-delivery', R, 'let outcome = self.stored_task_outcome(&receipt)?;',
    'let outcome = serde_json::json!({});',
    'te13_delivery_must_be_observed_and_lost_reply_is_reconciled', 'TE13_DELIVERY_LOST_REPLY')
add('retry-stop', C, 'if published.is_some() && settled {',
    'if (published.is_some() || state.outcome != RunOutcome::Pending) && settled {',
    'te13_stop_blocks_missing_work_but_retains_original_freeze', 'TE13_STOP_BLOCKS_COMPLETION')
add('retry-restart', C, 'Ok(endpoint.is_some_and(|endpoint| {', 'Ok(endpoint.is_none_or(|endpoint| {',
    'te13_restart_retains_completion_and_never_revives_authority', 'TE13_RESTART_BLOCKED_NO_AUTHORITY')
add('payload-collision', 'crates/kinewright-project/src/orchestration_store.rs',
    'if &cut.receipt.envelope != envelope || cut.consumed_batch.as_ref() != consume_batch',
    'if cut.consumed_batch.as_ref() != consume_batch',
    'te13_changed_completion_payload_and_reserved_id_are_refused', 'TE13_COMPLETION_COLLISION')
add('reserved-step', C, 'if !correct {', 'if false && !correct {',
    'te13_changed_completion_payload_and_reserved_id_are_refused', 'TE13_RESERVED_STEP_REFUSED')
# These faults execute a real extra Core edit at the completion boundary. They
# test exact histories, rather than a synthetic refusal or changed test assertion.
freeze = '        let _drain = self\n            .endpoint\n            .close_and_drain()\n            .map_err(BundleError::Unsupported)?;\n        if let Some(expected) = expected {'
add('task-history', T, freeze,
    '        self.core().request(kinewright_core::Command::Undo).unwrap();\n' + freeze,
    'te13_finish_preserves_exact_task_and_person_redo_histories', 'TE13_EXACT_TASK_HISTORY')
publication = '        self.completion_step(\n            index,\n            run,\n            &intent,\n            CompletionStep::Publication,'
add('manual-history', C, publication,
    '        self.owner()?.core().request(kinewright_core::Command::Undo).unwrap();\n' + publication,
    'te13_finish_preserves_exact_task_and_person_redo_histories', 'TE13_EXACT_MANUAL_REDO_HISTORY')
add('revision-bound', T, 'if revision != expected {\n                return Err(BundleError::Unsupported("timeline revision changed".into()));', 'if false && revision != expected {\n                return Err(BundleError::Unsupported("timeline revision changed".into()));',
    'te13_revision_change_refuses_to_rebase_the_accepted_intent', 'TE13_REVISION_BOUND_TO_INTENT')
stop = 'if matches!(&update.envelope().command, OrchestrationCommand::ControlRun(intent) if intent.action==RunAction::Stop)'
add('terminal-reserve', 'crates/kinewright-project/src/run_admission.rs', stop,
    'if matches!(&update.envelope().command, OrchestrationCommand::FinishTask(_)) || ' + stop[3:],
    'te13_finish_cannot_spend_the_terminal_reserve', 'TE13_ORDINARY_INTENT_CANNOT_USE_RESERVE')
add('production-merge-recovery', E,
    '        let blocks = self.inspect_recovery_blocks(&state)?;', '        let blocks = Vec::new();',
    'te13_before_commit_merge_reopen_has_a_production_person_block', 'TE13_RECOVERED_MERGE_VISIBLE')
add('commit-revision', S, 'serde_json::json!({"revision":revision,"affected_scope":committed_scope(&before,&doc,operations)})',
    'serde_json::json!({"affected_scope":committed_scope(&before,&doc,operations)})',
    'te13_commit_returns_its_revision_and_affected_scope', 'TE13_COMMIT_REVISION')
add('commit-scope', S, 'serde_json::json!({"revision":revision,"affected_scope":committed_scope(&before,&doc,operations)})',
    'serde_json::json!({"revision":revision})',
    'te13_commit_returns_its_revision_and_affected_scope', 'TE13_COMMIT_SCOPE')

add('progress-read-ack', 'crates/kinewright-agent/src/writer_host/task_mcp.rs',
    'return Ok(json!({"receipt":receipt,"completion_status":status}));',
    'return Ok(json!({"completion_status":status}));',
    'te13_progress_read_failure_keeps_the_acknowledged_intent_outcome', 'TE13_PROGRESS_FAILURE_RETAINS_RECEIPT')

add('readiness-before-intent', T, 'if !ready_for_completion(state, &request.task, true) {',
    'if false && !ready_for_completion(state, &request.task, true) {',
    'te13_unready_finish_does_not_reserve_the_parent_settlement_ids', 'TE13_UNREADY_FINISH_REFUSED')

if __name__ == '__main__':
    harness.main()
