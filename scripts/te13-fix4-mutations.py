#!/usr/bin/env python3
"""Retained fix-4 contracts; obsolete pending-preservation/refusal pairs are replaced in fix 5."""
from pathlib import Path
import importlib.util
import os
import sys

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('te13_fix4_harness', ROOT / 'scripts/te13-fix3-mutations.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
harness = module.harness
harness.OUT = Path(os.environ.get('KINEWRIGHT_TE13_FIX4_MUTATION_OUT', ROOT / 'target/te1-3/fix4/pairs'))
harness.PAIRS = []
add = module.add
C, O = module.C, module.O
M = 'crates/kinewright-agent/src/writer_host/task_mcp.rs'
P = 'crates/kinewright-project/src/studio_store.rs'

add('only-pending-run-stop-abandons', C,
    'incoming.run == pending.original.run', 'true',
    'te13_fix5_different_run_stop_cannot_abandon_uncertain_finish', 'TE13_FIX5_ONLY_MATCHING_STOP_ABANDONS')
add('current-block-issuance', C,
    'self.inspect_recovery_blocks(&state)\n                        .unwrap_or_default()',
    'self.hooks.runs.get(&state.run).map(|run|run.recovery_blocks.clone()).unwrap_or_default()',
    'te13_fix4_successful_continuation_closes_both_frozen_exceptions', 'TE13_FIX4_CURRENT_ISSUANCE_GUARD')
add('narrow-issuance', C,
    'pending_run.is_some() || self.hooks.runs.keys().any(blocked)', 'true',
    'te13_fix4_successful_continuation_closes_both_frozen_exceptions', 'TE13_FIX4_CURRENT_ISSUANCE_GUARD')
add('current-block-stop', C,
    'pending_run == Some(&intent.run_id) || blocked(&intent.run_id)', 'true',
    'te13_fix4_successful_continuation_closes_both_frozen_exceptions', 'TE13_FIX4_CURRENT_STOP_GUARD')
add('successful-continuation-has-no-block', C,
    '.retain(|block| *block != RecoveryBlock::Completion(finish.task.clone()));',
    '.retain(|_| true);',
    'te13_fix4_successful_continuation_closes_both_frozen_exceptions', 'TE13_FIX4_SUCCESS_NO_RAW_BLOCK')
add('refused-continuation-keeps-block', C,
    'if !retryable\n            && let', 'if false\n            && let',
    'te13_fix4_refused_continuation_keeps_current_block_and_receipt', 'TE13_FIX4_REFUSED_CONTINUATION_BLOCKS')
add('slot-free-receipt-read', M,
    'if host.hooks.pending.is_some() {\n            return Some(self.retry_pending_finish',
    'host.hooks.pending.as_ref()?;\n        if host.hooks.pending.is_some() {\n            return Some(self.retry_pending_finish',
    'te13_fix4_refused_continuation_keeps_current_block_and_receipt', 'TE13_FIX4_EXACT_FINISH_REPLAY')
add('live-retry-still-continues', M,
    'if host.resolve_task_mcp(&self.auth, &self.endpoint).is_ok() {', 'if false {',
    'te13_fix1_live_failure_blocks_and_same_finish_clears_stale_cache', 'TE13_FIX1_PERMANENT_BLOCK')
add('committed-metadata-replay', P,
    'if lookup != ReceiptLookup::Absent {\n            return Ok(lookup);\n        }\n        if self.unsettled',
    'if false {\n            return Ok(lookup);\n        }\n        if self.unsettled',
    'te13_fix4_committed_note_replay_reads_original_receipt_during_completion', 'TE13_FIX4_COMMITTED_NOTE_REPLAY')
add('truthful-fallback', C,
    'needs recovery. Stop this run and wait for cleanup, then check Studio recovery and rescue for available work.',
    'has retained work. Stop and wait for cleanup before reviewing its saved task branch.',
    'te13_fix4_missing_row_fallback_names_no_saved_branch', 'TE13_FIX4_TRUTHFUL_MISSING_ROW')
add('real-switch-stop', O,
    '&& !self.completion_owner_recovery(index, &command)', '&& true',
    'te13_fix4_stop_between_real_switch_prepare_and_confirm', 'TE13_FIX4_REAL_SWITCH_STOP_ENVELOPE')

if __name__ == '__main__':
    harness.main()
