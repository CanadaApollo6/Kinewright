#!/usr/bin/env python3
"""Typed preparation failures, retained work and normal post-notification waits.

Includes all 26 B2-0 pairs. Each named mutant changes a production guard and
must fail its behavioral witness before a byte-exact restore passes.
"""
from pathlib import Path
import importlib.util
import os
import sys

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('b201', ROOT / 'scripts/te15-b20-mutations.py')
r = importlib.util.module_from_spec(spec)
spec.loader.exec_module(r)
r.h.OUT = Path(os.environ.get('KINEWRIGHT_TE15_B201_OUT', ROOT / 'target/te1-5/b2-0-fix1/pairs'))
A = 'crates/kinewright-agent/src/'
W = A + 'writer_host/'
P = W + 'task_preparation.rs'
T = A + 'orchestration/tasks.rs'
O = W + 'orchestration.rs'
H = 'writer_host::hooks_tests::tests::'

def add(name, file, before, after, test, marker):
    r.PAIRS.append((name, [(file, before, after)], 'kinewright-agent', H + 'te15_b201_' + test, marker))

for test in ['fresh_director', 'fresh_child', 'prepared_director']:
    add('pause-' + test, P, 'matches!(self, Self::History | Self::Scope)', 'true', 'pause_' + test, 'B201_PAUSE_')
for kind in ['capacity', 'io']:
    add('retry-' + kind, P, 'matches!(self, Self::History | Self::Scope)', 'true', kind + '_retries', 'B201_TRANSIENT_NO_CANCEL')
add('typed-code', P, '_ => PreparationKind::Retry,\n        },', '_ => PreparationKind::Scope,\n        },', 'typed_classification', 'B201_TYPED_RETRY')
add('pause-no-failure-words', P, 'if state.phase != RunPhase::Paused {', 'if true {', 'pause_prepared_director', 'B201_PAUSE_NO_FAILURE_WORDS')
add('fresh-unsent-retry', O, 'if !safe {', 'if true {', 'pause_fresh_director', 'B201_PAUSE_ONE_SEND')
add('legacy-history-proof', P, 'intent.prepared == Some(false)', 'intent.prepared != Some(true)', 'legacy_unprepared_is_not_proof', 'B201_LEGACY_HISTORY_FAILS_CLOSED')
add('sent-child-not-cancelled', P, 'if parent.is_some() && sent {', 'if false && parent.is_some() && sent {', 'sent_child_keeps_work', 'B201_SENT_NOT_CANCELLED')
add('director-actionable', P, 'Press Stop, then Reopen to recover any saved work before starting another run. Continue cannot repair it.', 'Stop this run and start a new one after reviewing the task.', 'director_words', 'B201_DIRECTOR_WORDS')
add('notification-clears', T, '        if !delivered {', '        if true || !delivered {', 'notification_ends_without_assignment', 'B201_NOTIFICATION_CONSUMED')
for kind in ['question', 'children']:
    add('derive-' + kind, T, '        if !delivered {', '        if true || !delivered {', kind + '_after_notification', 'B201_NEXT_WAIT')
r.PAIRS.append(('assignment-consumes-notification', [(T, 'if effect.id.0.as_str() == key\n                            && effect.state', 'if false && effect.id.0.as_str() == key\n                            && effect.state')], 'kinewright-agent', H+'te15_b20_host_failure_notifies_director_without_retrying_child', 'B201_ASSIGN_CONSUMES_NOTIFICATION'))
for kind in ['files', 'provider', 'settings']:
    add('priority-' + kind, P, 'if !matches!(\n            state.wait,\n            RunWait::Files(_) | RunWait::Provider(_) | RunWait::ProviderSettings(_)\n        )', 'if true', kind + '_priority', 'B201_WAIT_PRIORITY')
add('unknown-priority', O, 'if machine::has_unknown_send(&projection) {', 'if false && machine::has_unknown_send(&projection) {', 'unknown_priority_and_accounting', 'B201_UNKNOWN_OUTRANKS_REPLAN')
add('accounting-stop-reason', W+'spending.rs', '                        | crate::orchestration::orchestration_records::RunWait::Replan(wait)\n', '', 'unknown_priority_and_accounting', 'B201_ACCOUNTING_REASON')
add('cancel-frees-capacity', P, 'child.progress.capacity =\n            crate::orchestration::orchestration_records::DependencyReservation::Free;', 'child.progress.capacity = crate::orchestration::orchestration_records::DependencyReservation::Reserved;', 'files_priority', 'B201_CANCEL_CAPACITY_FREE')
add('candidate-another-run', P, 'if run != state.run\n                && self', 'if false && run != state.run\n                && self', 'other_run_candidate_refuses', 'B201_OTHER_RUN')
for name, before in [
    ('receipt_branch', 'receipt.target_branch != origin.branch\n        || '),
    ('checkpoint_branch', '        || checkpoint.branch_id != origin.branch\n'),
    ('checkpoint_head', '        || checkpoint.head_id != receipt.head_id\n'),
    ('checkpoint_predecessor', '        || checkpoint.predecessor.as_ref() != Some(&base.head)\n'),
]:
    # Keep the referenced parameter in use so a RED is a behavior failure.
    after = 'false\n        || ' if name == 'receipt_branch' else ('        || (let _ = base; false)\n' if name == 'checkpoint_predecessor' else '')
    if name == 'checkpoint_predecessor':
        after = '        || { let _ = base; false }\n'
    add('owned-' + name, P, before, after, name, 'B201_RECEIPT_GUARD')
add('restart-late-base', O, 'task_record.brief.scope.base = base;', 'let _ = base;', 'fresh_child_after_merge_restart', 'C4_MCP_ATTEMPT_HISTORY')
r.PAIRS.append(('studio-host-reason', [('crates/kinewright-app/src/studio_feedback.rs', 'if matches!(self.wait, RunWait::Replan(_)) {', 'if false && matches!(self.wait, RunWait::Replan(_)) {')], 'kinewright-app', 'studio_feedback::ja_s3_feedback::te15_b1_fix4::te15_b201_replan_status_uses_host_reason', 'B201_UI_HOST_REASON'))
r.PAIRS.append(('studio-host-retry', [('crates/kinewright-app/src/studio_run_view.rs', 'reason\n        == "The writer host could not start the task yet. This start sent nothing. Continue to try again."', 'reason == "mutant disabled host retry words"')], 'kinewright-app', 'studio_feedback::ja_s3_feedback::te15_b1_fix4::te15_b201_transient_status_uses_host_reason', 'B201_UI_HOST_RETRY'))
r.h.FAST_WITNESSES = {row[3] for row in r.PAIRS}

if __name__ == '__main__':
    r.main()
