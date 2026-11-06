#!/usr/bin/env python3
"""TE1.3 fix 3: retained work, owner settlement and Studio controls."""
from pathlib import Path
import importlib.util
import os
import sys

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('te13_fix3_harness', ROOT / 'scripts/te13-fix2-mutations.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
harness = module.harness
harness.OUT = Path(os.environ.get('KINEWRIGHT_TE13_FIX3_MUTATION_OUT', ROOT / 'target/te1-3/fix3/pairs'))
harness.PAIRS = []
add = module.add
A, P, APP = module.A, module.P, module.APP
W, O, C = module.W, module.O, 'crates/kinewright-agent/src/writer_host/task_completion.rs'
BC = 'crates/kinewright-project/src/bundle_commit.rs'

for step in ('freeze', 'publication'):
    for fate in ('committed', 'absent'):
        for authority in ('expired', 'revoked'):
            add(f'stop-{step}-{fate}-{authority}', C,
                '            self.settle_completion_for_owner(index)?;', '            let _ = index;',
                f'te13_fix3_stop_{step}_{fate}_{authority}', 'TE13_FIX3_OWNER_STOP')
for fate in ('committed', 'absent'):
    add(f'preissued-stop-{fate}', C,
        'if self.attachments[index].role == super::AttachmentRole::Editor\n            && matches!(&incoming.envelope.command,',
        'if false\n            && matches!(&incoming.envelope.command,',
        f'te13_fix3_preissued_stop_{fate}', 'TE13_FIX3_OWNER_STOP')

publication_settles = ('let settled = merged || (matches!(task.state, crate::orchestration_records::TaskState::Terminal(_)) '
                       '&& !incomplete && state.deliveries.iter().filter(|delivery| delivery.task == task.id)'
                       '.all(|delivery| matches!(delivery.state, crate::orchestration_records::DeliveryState::Observed(_))));')
add('root-publication-retained', W, 'let settled = merged;', publication_settles,
    'te13_fix3_root_publication_is_not_a_merge', 'TE13_FIX3_ROOT_EMPTY_DELIVERY_KEPT')
for mode, source in (('standalone', module.S), ('host', module.HOST)):
    for scenario in ('root', 'observed', 'completed_parent'):
        add(f'{mode}-{scenario}-unmerged-offer', W, 'let settled = merged;', publication_settles,
            f'te13_fix3_{mode}_{scenario}', 'TE13_FIX3_UNMERGED_STUDIO_OFFER', APP)
    add(f'{mode}-unmerged-removal-warning', 'crates/kinewright-project/src/editing_session/recovery.rs',
        '"{task_disclosure}Remove working branch', '"Remove working branch',
        f'te13_fix3_{mode}_observed', 'TE13_FIX3_UNMERGED_REMOVAL_WARNING', APP)
    add(f'{mode}-removed-fork-reoffers', BC,
        'retained_task_reviews.retain(|fork, _| branches.contains_key(fork));',
        'let _ = &branches;', f'te13_fix3_{mode}_fork_removed', 'TE13_FIX3_FORK_LINK_PRUNED', APP)
    guard = ('!self.status.retained_task_failures' if mode == 'host' else '!self.retained_task_failures')
    # Filled below from the formatted source; select the entire real predicate.
    text = (ROOT / source).read_text()
    start = text.index('&& !self', text.index('&& work.reviewed_on.is_none()'))
    end = text.index('\n                            && ui', start)
    before = text[start:end]
    for scope in ('shared', 'unbound'):
        add(f'{mode}-{scope}-bad-branch-no-button', source, before, '&& true',
            f'te13_fix3_{mode}_{scope}_bad', 'TE13_FIX3_BAD_BRANCH_NO_BUTTON', APP)

add('stale-review-link-is-not-a-fork', W,
    'bundle.branches.contains_key(*branch) && **original == source',
    '**original == source',
    'editing_session::retained_work::tests::te13_fix2_retained_disclosure_tracks_in_memory_issuer_retirement',
    'TE13_FIX3_STALE_FORK_IGNORED', P)
add('host-live-blocked-stop-instruction', C,
    'let instruction = if state.outcome == RunOutcome::Pending {',
    'let instruction = if false {',
    'te13_fix3_live_blocked_names_owner_stop', 'TE13_FIX3_BLOCKED_STOP_DISCLOSURE', APP)
add('live-healthy-does-not-instruct-stop', W,
    'Review becomes available when the run and its cleanup finish.',
    'Stop this run and wait for cleanup.',
    'te13_fix3_live_healthy_has_no_stop_instruction', 'TE13_FIX3_HEALTHY_NO_STOP', APP)
current = (ROOT / C).read_text()
start = current.index('        let matching = blocks.iter().find_map(|block| {')
end = current.index('        let Some(block) = blocks.first()', start)
before = current[start:end]
after = before.replace('blocks.iter().find_map', 'blocks.iter().take(1).find_map').replace(
    'if state.outcome != RunOutcome::Pending {', 'if state.outcome != RunOutcome::Pending && blocks.is_empty() {')
add('host-second-task-run-message', C, before, after,
    'te13_fix3_removed_first_block_keeps_other_task_run_message', 'ASTRA_OTHER_RETAINED_TASK_RUN_MESSAGE', APP)
add('applied-receipt-clears-before-authority', C,
    'if stored.is_some() {', 'if false {',
    'te13_fix3_applied_receipt_settles_before_expired_continuation', 'TE13_FIX3_APPLIED_CLEARS_BEFORE_AUTHORITY')
for scenario in ('observed_child_stop', 'parent_finishes_without_merging_child'):
    add('real-' + scenario, W, 'let settled = merged;', publication_settles,
        'te13_fix3_real_' + scenario, 'TE13_FIX3_REAL_OBSERVED_CHILD_KEPT')
for fate in ('committed', 'absent'):
    add('owner-stop-frozen-' + fate, O,
        '&& !self.completion_owner_recovery(index, &command)', '&& true',
        'te13_fix3_preissued_stop_' + fate, 'TE13_FIX3_OWNER_STOP')

if __name__ == '__main__':
    harness.main()
