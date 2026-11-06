#!/usr/bin/env python3
"""TE1.3 fix 4: issuance preserves exact completion; only matching Stop abandons."""
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
MOD = 'crates/kinewright-agent/src/writer_host/mod.rs'
APP = 'kinewright-app'
for case in ('freeze_committed', 'freeze_absent', 'publication_committed', 'publication_absent', 'different_run'):
    add('issuance-' + case, O,
        'let completion_pending = self.prove_completion_for_owner(index)?;',
        'self.settle_completion_for_owner(index)?;\n        let completion_pending = false;',
        'te13_fix4_issue_' + case, 'TE13_FIX4_UNRELATED_ISSUANCE_PRESERVES_PENDING')
add('only-pending-run-stop-abandons', C,
    'incoming.run == pending.original.run', 'true',
    'te13_fix4_issue_different_run', 'TE13_FIX4_OTHER_RUN_STOP_CANNOT_ABANDON')
add('current-block-issuance', C,
    'self.run_recovery_blocks(&state)\n                        .unwrap_or_default()',
    'self.hooks.runs.get(&state.run).map(|run|run.recovery_blocks.clone()).unwrap_or_default()',
    'te13_fix4_successful_continuation_closes_both_frozen_exceptions', 'TE13_FIX4_CURRENT_ISSUANCE_GUARD')
add('narrow-issuance', C,
    'pending_run.is_some() || self.hooks.runs.keys().any(blocked)', 'true',
    'te13_fix4_successful_continuation_closes_both_frozen_exceptions', 'TE13_FIX4_CURRENT_ISSUANCE_GUARD')
add('current-block-stop', C,
    'pending_run == Some(&intent.run_id) || blocked(&intent.run_id)', 'true',
    'te13_fix4_successful_continuation_closes_both_frozen_exceptions', 'TE13_FIX4_CURRENT_STOP_GUARD')
add('successful-continuation-has-no-block', C,
    'if result.is_err() {\n            self.retain_completion_block(&pending);',
    'if true {\n            self.retain_completion_block(&pending);',
    'te13_fix4_successful_continuation_closes_both_frozen_exceptions', 'TE13_FIX4_SUCCESS_NO_RAW_BLOCK')
add('refused-continuation-keeps-block', C,
    'if result.is_err() {\n            self.retain_completion_block(&pending);',
    'if false {\n            self.retain_completion_block(&pending);',
    'te13_fix4_refused_continuation_keeps_current_block_and_receipt', 'TE13_FIX4_REFUSED_CONTINUATION_BLOCKS')
add('slot-free-receipt-read', M,
    'if host.hooks.pending.is_some() {\n            return Some(self.retry_pending_finish',
    'host.hooks.pending.as_ref()?;\n        if host.hooks.pending.is_some() {\n            return Some(self.retry_pending_finish',
    'te13_fix4_refused_continuation_keeps_current_block_and_receipt', 'TE13_FIX4_EXACT_FINISH_REPLAY')
add('live-retry-still-continues', M,
    'if host.resolve_task_mcp(&self.auth, &self.endpoint).is_ok() {', 'if false {',
    'te13_fix1_live_failure_blocks_and_same_finish_clears_stale_cache', 'TE13_FIX1_PERMANENT_BLOCK')
add('note-write-refusal', C,
    'if matches!(\n            self.hooks.pending,\n            Some(super::orchestration::PendingPublication::Completion(_))\n        ) {',
    'if false {',
    'te13_fix4_notes_and_voice_refuse_without_abandoning_completion', 'TE13_FIX4_NOTE_PENDING_REFUSAL')
add('metadata-write-refusal', MOD,
    'ReceiptLookup::Absent => {\n                            self.completion_write_available()?;',
    'ReceiptLookup::Absent => {',
    'te13_fix4_notes_and_voice_refuse_without_abandoning_completion', 'TE13_FIX4_METADATA_PENDING_REFUSAL')
add('committed-metadata-replay', MOD,
    'let receipt = self\n                        .owner()?\n                        .manual_transition_receipt',
    'self.completion_write_available()?;\n                    let receipt = self\n                        .owner()?\n                        .manual_transition_receipt',
    'te13_fix4_committed_note_replay_reads_original_receipt_during_completion', 'TE13_FIX4_COMMITTED_NOTE_REPLAY')
add('truthful-fallback', C,
    'needs recovery. Stop this run and wait for cleanup, then check Studio recovery and rescue for available work.',
    'has retained work. Stop and wait for cleanup before reviewing its saved task branch.',
    'te13_fix4_missing_row_fallback_names_no_saved_branch', 'TE13_FIX4_TRUTHFUL_MISSING_ROW')
add('real-switch-stop', O,
    '&& !self.completion_owner_recovery(index, &command)', '&& true',
    'te13_fix4_stop_between_real_switch_prepare_and_confirm', 'TE13_FIX4_REAL_SWITCH_STOP_ENVELOPE')
F = 'crates/kinewright-app/src/studio_feedback.rs'
before = '''            Reply::Refused(why) => {
                if let Some(draft) = self.draft_mut(comment) {
                    draft.state = DraftState::Refused(why.clone());
                }
                Outcome::Refused(why)
            }'''
for mode in ('standalone','host'):
    harness.PAIRS.append((mode+'-note-wording',F,before,
        before.replace('DraftState::Refused(why.clone())','DraftState::Editing'),APP,
        f'studio_feedback::ja_s3_feedback::te13_fix4_{mode}_note_refusal_keeps_exact_draft',
        'TE13_FIX4_STUDIO_TRUTHFUL_NOTE'))
V = 'crates/kinewright-app/src/studio_voice.rs'
text = (ROOT / V).read_text()
start = text.index('                    Some(Ok(response)) => {')
end = text.index('                    Some(Err(TryRecvError::Disconnected))',start)
before = text[start:end]
state_start = before.index('                        State::Upload {')
state_end = before.index('\n                    }',state_start)
after = before[:state_start] + '                        State::Idle' + before[state_end:]
harness.PAIRS.append(('voice-capture-kept',V,before,after,APP,
    'studio_voice::tests::te13_fix4_host_voice_refusal_keeps_capture_text_and_retry','TE13_FIX4_VOICE_CAPTURE_KEPT'))

if __name__ == '__main__':
    harness.main()
