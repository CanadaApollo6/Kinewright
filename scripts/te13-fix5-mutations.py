#!/usr/bin/env python3
"""Fix 5: current-generation completion and identity-bound Studio voice recovery."""
from pathlib import Path
import importlib.util
import os
import sys

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('te13_fix5_harness', ROOT / 'scripts/te13-fix4-mutations.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
harness = module.harness
harness.OUT = Path(os.environ.get('KINEWRIGHT_TE13_FIX5_MUTATION_OUT', ROOT / 'target/te1-3/fix5/pairs'))
harness.PAIRS = []
C, O = module.C, module.O
V = 'crates/kinewright-agent/src/writer_host/voice.rs'
MOD = 'crates/kinewright-agent/src/writer_host/mod.rs'
P = 'crates/kinewright-project/src/studio_store.rs'
F = 'crates/kinewright-app/src/studio_feedback.rs'
AV = 'crates/kinewright-app/src/studio_voice.rs'

def add(name, source, before, after, test, marker, package='kinewright-agent'):
    prefix = 'writer_host::hooks_tests::tests::' if package == 'kinewright-agent' and '::' not in test else ''
    harness.PAIRS.append((name, source, before, after, package, prefix + test, marker))

for step in ('freeze', 'publication'):
    add('repeated-' + step, C,
        'let retryable = Self::completion_failure_retryable(error);', 'let retryable = false;',
        f'te13_fix5_repeated_{step}_fault', 'TE13_FIX5_RETRYABLE_NO_BLOCK')
for case in ('freeze_committed','freeze_absent','publication_committed','publication_absent'):
    add('issuance-releases-' + case, C,
        '        self.proved_completion_fate(&bound)?;\n        self.hooks.pending = None;',
        '        self.proved_completion_fate(&bound)?;',
        'te13_fix5_issue_' + case,'TE13_FIX5_PROOF_RELEASES_ENVELOPE')
add('issuance-does-not-abandon', C,
    '        self.proved_completion_fate(&bound)?;\n        self.hooks.pending = None;',
    '        self.proved_completion_fate(&bound)?;\n        if let OrchestrationCommand::FinishTask(finish) = &pending.original.envelope.command { self.hooks.runs.get_mut(pending.original.run.as_ref().unwrap()).unwrap().recovery_blocks.push(RecoveryBlock::Completion(finish.task.clone())); }\n        self.hooks.pending = None;',
    'te13_fix5_issue_publication_absent','TE13_FIX5_ISSUANCE_NO_BLOCK')
# Reproduce the former retained-envelope path only after a real first fault.
# Mutating completion_step's initial generation would break fixture setup first.
completion = (ROOT / C).read_text()
stale_completion = completion.replace(
    '        self.proved_completion_fate(&bound)?;\n        self.hooks.pending = None;',
    '        self.proved_completion_fate(&bound)?;')
stale_completion = stale_completion.replace(
    'self.authorize_orchestration(index, incoming)?;\n        }\n        self.drive_completion',
    'self.authorize_orchestration(index, incoming)?;\n            self.dispatch_orchestration(index, &pending.step)?;\n        }\n        self.drive_completion')
for case in ('save_after_proof','save_without_issuance','maintenance_after_proof','maintenance_without_issuance'):
    add('current-generation-' + case,C,completion,stale_completion,
        'te13_fix5_' + case,'TE13_FIX4_EXACT_FINISH_REPLAY')

# The former issuance exception allowed its proved Completion slot to remain.
# Restore that real companion guard for the stale-envelope mutants, so issuance
# succeeds and the named MCP retry, rather than setup, witnesses the regression.
original_run = harness.run
def run_with_stale_issuance(name, package, witness, marker, red):
    if not red or not (name.startswith('current-generation-') or name == 'real-switch-finish-red'):
        return original_run(name, package, witness, marker, red)
    import hashlib
    path = ROOT / O
    original = path.read_bytes()
    before = b'        if self.hooks.pending.is_some() {\n            return Err(BundleError::Unsettled.into());\n        }'
    after = b'        if self.hooks.pending.is_some() && !matches!(self.hooks.pending, Some(PendingPublication::Completion(_))) {\n            return Err(BundleError::Unsettled.into());\n        }'
    assert original.count(before) == 1
    mutant = original.replace(before, after)
    (harness.OUT / (name + '.issuance.original')).write_bytes(original)
    try:
        path.write_bytes(mutant)
        result = original_run(name, package, witness, marker, red)
    finally:
        path.write_bytes(original)
        assert path.read_bytes() == original
    result['companion_source'] = O
    result['companion_original_sha256'] = hashlib.sha256(original).hexdigest()
    result['companion_mutant_sha256'] = hashlib.sha256(mutant).hexdigest()
    result['companion_restored_sha256'] = hashlib.sha256(path.read_bytes()).hexdigest()
    return result
harness.run = run_with_stale_issuance
for mode in ('recovering','switch'):
    add('durable-stop-' + mode,C,
        'self.inspect_recovery_blocks(&state)\n                        .unwrap_or_default()',
        'self.run_recovery_blocks(&state)\n                        .unwrap_or_default()',
        f'te13_fix5_{mode}_stop_without_slot_or_block','TE13_FIX5_DURABLE_FROZEN_ISSUANCE')
    add('durable-stop-dispatch-' + mode,C,
        'pending_run == Some(&intent.run_id) || blocked(&intent.run_id)',
        'pending_run == Some(&intent.run_id)',
        f'te13_fix5_{mode}_stop_without_slot_or_block','TE13_FIX5_DURABLE_FROZEN_STOP')
add('read-failure-is-closed',C,
    '.flatten()\n                .is_some_and(|state| {',
    '.flatten()\n                .is_none_or(|state| {',
    'te13_fix5_durable_stop_read_failure_fails_closed','TE13_FIX5_UNREADABLE_NO_ISSUANCE_BYPASS')
add('temporary-authorization-refusal',C,
    '| "recovery_frozen"', '| "disabled_recovery_frozen"',
    'te13_fix5_temporary_recovery_does_not_install_block','TE13_FIX5_TEMPORARY_AUTH_NO_BLOCK')
add('stored-refusal-is-permanent',C,
    'let retryable = Self::completion_failure_retryable(error);','let retryable = true;',
    'te13_fix5_stored_refusal_is_permanent','TE13_FIX5_STORED_REFUSAL_BLOCKS')
add('refused-stop-keeps-finish',C,
    'self.prove_completion_for_owner(index)?;\n            return Ok(None);',
    'if let OrchestrationCommand::FinishTask(finish) = &pending.original.envelope.command { self.hooks.runs.get_mut(pending.original.run.as_ref().unwrap()).unwrap().recovery_blocks.push(RecoveryBlock::Completion(finish.task.clone())); }\n'
    '            self.prove_completion_for_owner(index)?;\n            return Ok(None);',
    'te13_fix5_refused_stop_does_not_abandon_finish','TE13_FIX5_REFUSED_STOP_NO_FALSE_BLOCK')
add('waiting-alive',C,
    'return Ok(Some("Task completion is waiting for its original finish retry. No automatic retry is confirmed.".into()));',
    'return Ok(None);','te13_fix5_repeated_freeze_fault','TE13_FIX5_TRUTHFUL_WAITING')
add('waiting-gone',C,
    'return Ok(Some("Task completion is waiting for its original finish retry. No automatic retry is confirmed.".into()));',
    'return Ok(None);','te13_fix5_waiting_without_caller_has_truthful_words','TE13_FIX5_TRUTHFUL_WAITING')
for restarted in (False,True):
    add('generic-id-' + ('restart' if restarted else 'live'),C,
        'if stored.is_none()','if false',
        'te13_fix5_generic_receipt_conflict_' + ('survives_restart' if restarted else 'refuses'),
        'TE13_FIX5_REOPENED_GENERIC_ID_NOT_ABSENT' if restarted else 'TE13_FIX5_GENERIC_RECEIPT_IS_NOT_ABSENCE')
add('real-switch-finish',C,completion,stale_completion,
    'te13_fix5_real_switch_then_exact_finish','TE13_FIX4_EXACT_FINISH_REPLAY')

for case in ('stop_wins_voice_id','note_wins_voice_id','stop_collision_across_restart'):
    add('studio-' + case,V,
        'is_some_and(|error| error.code == "id_payload_conflict")',
        'is_some_and(|error| error.code == "disabled_conflict")',
        'studio_voice::tests::te13_fix5_' + case,'TE13_FIX5_STUDIO_VOICE_SAVED','kinewright-app')
add('studio-finish-before-upload',V,
    '|| grant.root_generation != descriptor.root_generation','|| false',
    'studio_voice::tests::te13_fix5_finish_before_voice_upload','TE13_FIX5_STUDIO_VOICE_SAVED','kinewright-app')
add('conflict-always-rebinds',V,
    'if conflict\n                    || grant.envelope.owner_epoch',
    'if false\n                    || grant.envelope.owner_epoch',
    'writer_host::voice_tests::tests::te13_fix5_voice_conflict_rebinds_even_at_same_clock','TE13_FIX5_CONFLICT_ALWAYS_FRESH_ENVELOPE')
for restart in (False,True):
    add(('host-' if restart else '') + 'already-saved-note-' + ('restart' if restart else 'live'),V,
        'self.owner()?\n                            .feedback_index()?\n                            .notes\n                            .contains_key(&upload.note)',
        'false',
        'writer_host::voice_tests::tests::te13_fix5_accepted_voice_' + ('conflict_after_restart' if restart else 'changed_digest_is_read_only'),
        'TE13_FIX5_RESTART_NOTE_ID_PREVENTS_SECOND_SAVE' if restart else 'TE13_FIX5_NO_DUPLICATE_VOICE_PUBLICATION')
text = (ROOT / V).read_text()
start = text.index('                    if status.voice.audio_ref != upload.audio')
end = text.index('\n                    {',start)
add('changed-voice-evidence',V,text[start:end],'                    if false',
    'writer_host::voice_tests::tests::te13_fix5_accepted_voice_changed_evidence_refuses','TE13_FIX5_CHANGED_CAPTURE_REFUSED')
add('unknown-issuer-is-not-absent',V,
    'is_some_and(|error| error.code == "id_payload_conflict")','is_some_and(|_| true)',
    'writer_host::voice_tests::tests::te13_fix5_unpublished_voice_issuer_stays_unknown','TE13_FIX5_UNRESOLVED_IS_NOT_ABSENT')
add('standalone-voice-unchanged',AV,
    '            self.voice_client()?;', '            let _ = self.voice_client();',
    'studio_feedback::ja_s3_feedback::te13_fix4_standalone_spoken_note_requires_its_writer_host',
    'TE13_FIX4_STANDALONE_VOICE_NO_HOST','kinewright-app')
for mode in ('standalone','host'):
    add('studio-' + mode + '-text-save',F,
        'draft.state = DraftState::Pending(Box::new(write));','draft.state = DraftState::Editing;',
        f'studio_feedback::ja_s3_feedback::te13_fix5_{mode}_note_saves_once','TE13_FIX5_NORMAL_NOTE_SAVED','kinewright-app')
add('studio-text-during-finish',MOD,
    'let close_session = matches!(&command, HostCommand::CloseSession { .. });',
    'let close_session = matches!(&command, HostCommand::CloseSession { .. });\n'
    '        if matches!(self.hooks.pending, Some(orchestration::PendingPublication::Completion(_)))\n'
    '            && matches!(command, HostCommand::AdmitFeedbackEvidence { .. }) {\n'
    '            return Err(HostRefusal::new("completion_pending", "Task completion is awaiting its original retry. This write has not been saved.").into());\n        }',
    'studio_voice::tests::te13_fix5_text_during_pending_finish','TE13_FIX5_STUDIO_TEXT_SAVED','kinewright-app')
add('metadata-during-finish',MOD,
    'let result = self\n                    .owner_mut()?\n                    .metadata_command(&envelope, scope, &update)?;',
    'if matches!(self.hooks.pending, Some(orchestration::PendingPublication::Completion(_))) { return Err(BundleError::Unsettled.into()); }\n'
    '                let result = self\n                    .owner_mut()?\n                    .metadata_command(&envelope, scope, &update)?;',
    'te13_fix5_metadata_publishes_with_pending_finish','TE13_FIX5_METADATA_WITH_PENDING_SUCCESS')

if __name__ == '__main__':
    harness.main()
