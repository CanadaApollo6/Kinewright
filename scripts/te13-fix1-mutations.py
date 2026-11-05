#!/usr/bin/env python3
"""TE1.3 fix 1: named data-loss witnesses and exact-byte restores."""
from pathlib import Path
import importlib.util
import os
import sys

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('te13_pair_harness', ROOT / 'scripts/te13-merge-mutations.py')
harness = importlib.util.module_from_spec(spec)
spec.loader.exec_module(harness)
harness.OUT = Path(os.environ.get('KINEWRIGHT_TE13_FIX1_MUTATION_OUT', ROOT / 'target/te1-3/fix1/pairs'))
harness.PAIRS = []
A = 'kinewright-agent'
P = 'kinewright-project'
APP = 'kinewright-app'
H = 'writer_host::hooks_tests::tests::'
T = 'crates/kinewright-agent/src/orchestration/tasks.rs'
C = 'crates/kinewright-agent/src/writer_host/task_completion.rs'
O = 'crates/kinewright-agent/src/writer_host/orchestration.rs'
R = 'crates/kinewright-project/src/editing_session/recovery.rs'
W = 'crates/kinewright-project/src/editing_session/retained_work.rs'
RECOVERY = 'te13_fix1_crash_stop_recovery_preserves_both_histories_and_original_merge'
FENCE = 'te13_fix1_narrow_reopen_never_unseals_accepted_or_frozen_ingress'


def add(name, source, before, after, witness, marker, package=A):
    prefix = H if package == A else 'ja1_attachment::tests::' if package == APP else ''
    harness.PAIRS.append((name, source, before, after, package, prefix + witness, marker))


add('drain-before-revision', T,
    'let _gate = self.0.gate.try_lock().map_err(|_| "task_ingress_busy")?;',
    'let _gate = ();', 'te13_fix1_admitted_edit_cannot_race_an_accepted_finish', 'TE13_FIX1_RACE_DRAIN_BEFORE_REVISION')
add('refused-finish-reopens', T,
    'INGRESS_FINISH_DRAINING,\n            INGRESS_OPEN,',
    'INGRESS_FINISH_DRAINING,\n            INGRESS_SEALED,',
    'te13_revision_change_refuses_without_stranding_the_task', 'TE13_FIX1_REFUSED_FENCE_OPEN')
add('accepted-never-reopens', T,
    '''let _ = self.0.phase.compare_exchange(
            INGRESS_FINISH_UNCERTAIN,
            INGRESS_OPEN,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );''',
    'self.0.phase.store(INGRESS_OPEN, Ordering::SeqCst);', FENCE, 'TE13_FIX1_ACCEPTED_NEVER_REOPENS')
add('unknown-keeps-fence', T,
    'INGRESS_FINISH_DRAINING,\n            INGRESS_FINISH_UNCERTAIN,',
    'INGRESS_FINISH_DRAINING,\n            INGRESS_OPEN,', FENCE, 'TE13_FIX1_UNKNOWN_STAYS_FENCED')
add('sealed-unknown-never-reopens', T,
    '''let _ = self.endpoint.0.phase.compare_exchange(
            INGRESS_FINISH_DRAINING,
            INGRESS_FINISH_UNCERTAIN,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );''',
    'self.endpoint.0.phase.store(INGRESS_FINISH_UNCERTAIN, Ordering::SeqCst);', FENCE, 'TE13_FIX1_FROZEN_NEVER_REOPENS')
add('accepted-intent-keeps-latest-history', O,
    'let transition = self.stage_completion_history(transition)?;',
    'let transition = transition;', RECOVERY, 'TE13_FIX1_CRASH_LATEST_HISTORY')
add('stop-disclosure', C,
    'self.inspect_recovery_blocks(state).unwrap_or_default()',
    'return Ok(None)', RECOVERY, 'TE13_FIX1_STOP_DISCLOSURE')
add('published-task-kept-without-intent', W,
    'RetainedWorkKind::Task\n        };',
    'return Ok(None);\n        };', RECOVERY, 'TE13_FIX1_RETAINED_WORK')
add('recovery-activates-task', R,
    'self.core = prepared.restore.core;', 'let _ = &prepared.restore.core;',
    RECOVERY, 'TE13_FIX1_RECOVER_EXACT_TASK')
add('recovery-forks-protected-task', R,
    'prepared.fork_retained_target()?;', 'let _ = &prepared;', RECOVERY, 'TE13_FIX1_RECOVERY_FORKS_SOURCE')
add('recovery-once', R,
    '&& previous == &target', '&& previous != &target', RECOVERY, 'TE13_FIX1_RECOVERY_ONCE')
add('person-history-retained', 'crates/kinewright-project/src/bundle_commit.rs',
    'let mut branches = bundle.branches.clone();',
    'let mut branches = bundle.branches.iter().filter(|(id, _)| selection.is_none() || *id != &bundle.manual_branch_id).map(|(id, checkpoint)| (id.clone(), checkpoint.clone())).collect::<BTreeMap<_, _>>();',
    RECOVERY, 'TE13_FIX1_PERSON_BRANCH_RETAINED')
add('person-selection-requires-stop', W,
    '.any(|work| !work.stopped || !work.cleanup_settled)',
    '.any(|_| false)', RECOVERY, 'TE13_FIX1_LIVE_PERSON_SELECTION_REFUSED')
add('retained-removal-warning', R,
    '"{task_disclosure}Remove working branch', '"Remove working branch',
    RECOVERY, 'TE13_FIX1_REMOVE_WARNING')
add('permanent-live-block', C,
    'runtime.recovery_blocks.push(block);', 'let _ = block;',
    'te13_fix1_live_failure_blocks_and_same_finish_clears_stale_cache', 'TE13_FIX1_PERMANENT_BLOCK')
add('permanent-status-retained', C,
    'retained_block.then(|| "completion_recovery_requires_person".into())', 'None',
    'te13_fix1_live_failure_blocks_and_same_finish_clears_stale_cache',
    'TE13_FIX1_PERMANENT_STATUS_SURVIVES_LAST_FAILURE')
add('permanent-refuses-ready-send', 'crates/kinewright-agent/src/writer_host/effect_execution.rs',
    'if !self.run_recovery_blocks(&state)?.is_empty() {\n            return Ok(None);\n        }',
    'let _ = self.run_recovery_blocks(&state)?;',
    'te13_fix1_live_failure_blocks_and_same_finish_clears_stale_cache', 'TE13_FIX1_PERMANENT_NO_CHARGE')
add('permanent-refuses-reserved-charge', 'crates/kinewright-agent/src/writer_host/native_invocation.rs',
    'if !self.run_recovery_blocks(&state)?.is_empty() {\n            return Err(refused(\n                "recovery_requires_person: unfinished task completion or merge",\n            ));\n        }',
    'let _ = self.run_recovery_blocks(&state)?;',
    'te13_fix1_live_failure_blocks_and_same_finish_clears_stale_cache', 'TE13_FIX1_PRECOMMIT_BLOCKS_RESERVED_SEND')
add('retryable-stays-retryable', C,
    '''if refusal.message.contains("fixture lost reply") {
            return true;
        }''',
    '''if refusal.message.contains("fixture lost reply") {
            return false;
        }''',
    'te13_fix1_live_failure_blocks_and_same_finish_clears_stale_cache', 'TE13_FIX1_RETRYABLE_NOT_BLOCKED')
add('completion-cache-clears', C,
    '.filter(|block| current.contains(block))', '.filter(|_| true)',
    'te13_fix1_live_failure_blocks_and_same_finish_clears_stale_cache', 'TE13_FIX1_CLEARED_CACHE')
add('merge-cache-clears', C,
    '.filter(|block| current.contains(block))', '.filter(|_| true)',
    'te13_fix1_merge_receipt_clears_live_recovery_cache', 'TE13_FIX1_MERGE_CACHE_CLEARED')
add('standalone-studio-disclosure', 'crates/kinewright-app/src/ja1_attachment.rs',
    'self.retained_task_work = retained.work;',
    'self.retained_task_work = Vec::new();',
    'te13_fix1_standalone_studio_opens_retained_task_work', 'TE13_FIX1_STUDIO_DISCLOSURE', APP)
add('host-studio-disclosure', 'crates/kinewright-app/src/ja1_host.rs',
    'for work in &self.status.retained_task_work {',
    'for work in self.status.retained_task_work.iter().take(0) {',
    'te13_fix1_host_studio_opens_retained_task_work', 'TE13_FIX1_STUDIO_DISCLOSURE', APP)
add('redo-validates-pre-redo-document', T,
    '''if redo {
                &history.document
            } else {
                &boundary.document
            },''', '&boundary.document,',
    'te13_revision_change_refuses_without_stranding_the_task', 'TE13_FIX1_REFUSED_REDO')
add('freeze-phase-needs-applied', C,
    '''let phase = if freeze_receipt
                .as_ref()
                .is_some_and(|receipt| receipt.decision == CommandDecision::Applied)''',
    'let phase = if freeze_receipt.is_some()',
    'te13_fix1_refused_freeze_receipt_does_not_advance_phase', 'TE13_FIX1_REFUSED_FREEZE_PHASE')
protocol = 'crates/kinewright-agent/src/protocol.rs'
before = '''        if !self.usage.has_known_tokens() {
            events.retain(|event| !matches!(event, AgentEvent::Cost { .. }));
        }
        for event in &mut events {
            self.usage.project(event);
        }'''
after = '''        for event in &mut events {
            self.usage.project(event);
        }
        if !self.usage.has_known_tokens() {
            events.retain(|event| !matches!(event, AgentEvent::Cost { .. }));
        }'''
harness.PAIRS.append(('unknown-display-cost-keeps-watermark',protocol,before,after,A,
    'protocol::tests::te13_fix1_unknown_cost_does_not_advance_display_watermarks','TE13_FIX1_DISPLAY_WATERMARK_79'))

if __name__ == '__main__':
    harness.main()
