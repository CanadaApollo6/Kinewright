#!/usr/bin/env python3
"""Fix 6: waiting words match completion status, paused is not expired, durable Stop in a real switch."""
from pathlib import Path
import importlib.util
import os
import sys

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('te13_fix6_harness', ROOT / 'scripts/te13-fix4-mutations.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
harness = module.harness
harness.OUT = Path(os.environ.get('KINEWRIGHT_TE13_FIX6_MUTATION_OUT', ROOT / 'target/te1-3/fix6/pairs'))
harness.PAIRS = []
C = module.C
M = 'crates/kinewright-agent/src/writer_host/task_mcp.rs'


def add(name, source, before, after, test, marker):
    harness.PAIRS.append((name, source, before, after, 'kinewright-agent', 'writer_host::hooks_tests::tests::' + test, marker))


words = 'Ok(false) => blocks.push(RecoveryBlock::Completion(task.id.clone())),'
add('no-retry-words-dead', C, words, 'Ok(false) => waiting = true,', 'te13_fix6_dead_caller_is_blocked_not_waiting', 'TE13_FIX6_NO_RETRY_WORDS')
add('no-retry-words-revoked', C, words, 'Ok(false) => waiting = true,', 'te13_fix6_revoked_authority_is_blocked_not_waiting', 'TE13_FIX6_NO_RETRY_WORDS')
add('dead-attachment-has-no-authority', C,
    'attachment.alive.load(std::sync::atomic::Ordering::Acquire)\n                        && subject.as_deref()',
    'subject.as_deref()',
    'te13_fix6_dead_caller_is_blocked_not_waiting', 'TE13_FIX6_NO_RETRY_WORDS')
add('paused-is-not-expired', M,
    'HostRefusal::new(&reason, "the run is paused; retry after it resumes")',
    'HostRefusal::new("task_authority_expired", "task authority expired")',
    'te13_fix6_paused_run_still_waits_and_tells_the_caller', 'TE13_FIX6_PAUSED_IS_NOT_EXPIRED')
add('paused-still-waits', C,
    '.map_or_else(|reason| reason == "task_authority_paused", |()| true)',
    '.is_ok()',
    'te13_fix6_paused_run_still_waits_and_tells_the_caller', 'TE13_FIX5_TRUTHFUL_WAITING')
add('real-switch-durable-stop', C,
    'pending_run == Some(&intent.run_id) || blocked(&intent.run_id)',
    'pending_run == Some(&intent.run_id)',
    'te13_fix6_durable_stop_inside_real_switch', 'TE13_FIX6_REAL_SWITCH_STOP')

if __name__ == '__main__':
    harness.main()
