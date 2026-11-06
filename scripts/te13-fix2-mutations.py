#!/usr/bin/env python3
"""TE1.3 fix 2: public replay and Studio witnesses with exact restores."""
from pathlib import Path
import importlib.util
import os
import sys

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('te13_fix2_harness', ROOT / 'scripts/te13-merge-mutations.py')
harness = importlib.util.module_from_spec(spec)
spec.loader.exec_module(harness)
harness.OUT = Path(os.environ.get('KINEWRIGHT_TE13_FIX2_MUTATION_OUT', ROOT / 'target/te1-3/fix2/pairs'))
harness.PAIRS = []
A, P, APP = 'kinewright-agent', 'kinewright-project', 'kinewright-app'
H = 'writer_host::hooks_tests::tests::'
O = 'crates/kinewright-agent/src/writer_host/orchestration.rs'
W = 'crates/kinewright-project/src/editing_session/retained_work.rs'
S = 'crates/kinewright-app/src/ja1_attachment.rs'
HOST = 'crates/kinewright-app/src/ja1_host.rs'


def add(name, source, before, after, witness, marker, package=A):
    prefix = H if package == A else 'ja1_attachment::tests::' if package == APP else ''
    harness.PAIRS.append((name, source, before, after, package, prefix + witness, marker))


add('refused-replay-reopens', O,
    'self.reopen_unapplied_finish(request)?;', 'let _ = request;',
    'te13_fix2_reconciled_refusal_reopens_real_edit_undo_redo', 'TE13_FIX2_RECONCILED_REFUSAL_REDO')
for step in ('freeze', 'publication'):
    for fate in ('committed', 'absent'):
        add(f'uncertain-{step}-{fate}', O,
            'if let Some(receipt) = self.reconcile_completion_substep(index, incoming)? {',
            'if let Some(receipt) = None::<OrchestrationReceipt> {',
            f'te13_fix2_uncertain_{step}_{fate}', 'TE13_FIX2_SUBSTEP_RETRY')
add('mcp-exact-pending-finish', 'crates/kinewright-agent/src/server.rs',
    'if request.name == "finish_task"\n                && let Some(result) = control.retry_pending_completion(',
    'if false\n                && let Some(result) = control.retry_pending_completion(',
    'te13_fix2_real_mcp_can_retry_the_retained_finish', 'TE13_FIX2_MCP_PENDING_FINISH')

before = '''                    Err(_) => {
                        if let Some(branch) = &source {
                            discovery.task_branches.push(branch.clone());
                        }
                        discovery.failures.push(RetainedWorkFailure {
                            run: Some(run.clone()), task: Some(task.id.clone()), branch: source,
                            disclosure: format!("Task {} kept work could not be inspected. Keep its working branch and try again. Opening or removing that work is unavailable until this is resolved.", task.id.0.as_str()),
                        });
                    }'''
add('history-errors-are-per-task', W, before,
    '                    Err(error) => return Err(error),',
    'te13_fix2_standalone_studio_isolates_bad_history', 'TE13_FIX2_HEALTHY_TASK_VISIBLE', APP)
add('standalone-shows-inspection-error', S,
    'for failure in &self.retained_task_failures {',
    'for failure in self.retained_task_failures.iter().take(0) {',
    'te13_fix2_standalone_studio_isolates_bad_history', 'TE13_FIX2_STUDIO_INSPECTION_FAILURE', APP)
add('host-status-shows-inspection-error', 'crates/kinewright-agent/src/writer_host/mod.rs',
    'status.retained_task_failures = retained.failures;',
    'status.retained_task_failures = Vec::new();',
    'te13_fix2_host_studio_isolates_bad_history', 'TE13_FIX2_STUDIO_INSPECTION_FAILURE', APP)
before = "if discovery\n            .failures\n            .iter()\n            .any(|failure| failure.blocks_branch(branch))\n        {"
add('bad-source-action-refuses', W, before, 'if false {',
    'te13_fix2_standalone_studio_isolates_bad_history', 'TE13_FIX2_BAD_SELECTION_REFUSED', APP)
add('healthy-source-action-survives', W,
    '.is_none_or(|failed| failed == branch)', '.is_none_or(|_| true)',
    'te13_fix2_standalone_studio_isolates_bad_history', 'TE13_FIX2_HEALTHY_TASK_VISIBLE', APP)
add('merged-work-has-no-offer', W,
    'let settled = merged', 'let settled = false',
    'te13_fix2_completed_merge_has_no_recovery_offer', 'TE13_FIX2_MERGED_NO_OFFER')
# Publication and observation never settle unmerged work. The two obsolete
# no-offer witnesses were removed in fix 3, rather than preserving their bug.
add('stop-disposed-delivery-stays-kept', W,
    'let settled = merged;',
    'let settled = merged || matches!(task.state, crate::orchestration_records::TaskState::Terminal(_)) && !incomplete;',
    'te13_fix2_stop_disposed_delivery_keeps_unreviewed_task_work', 'TE13_FIX2_DISPOSED_DELIVERY_KEPT')
for mode, source in (('standalone', S), ('host', HOST)):
    add(f'{mode}-removed-work-has-no-offer', W,
        'if !bundle.branches.contains_key(&history.source.branch) {', 'if false {',
        f'te13_fix2_{mode}_studio_hides_removed_task_branch', 'TE13_FIX2_REMOVED_BRANCH_NO_DISCLOSURE', APP)
    add(f'{mode}-review-link-survives-save', 'crates/kinewright-project/src/bundle_commit.rs',
        '            retained_task_reviews,', '            retained_task_reviews: BTreeMap::new(),',
        f'te13_fix2_{mode}_studio_keeps_review_provenance', 'TE13_FIX2_REVIEWED_NO_REOFFER', APP)
    add(f'{mode}-reviewed-action-stays-hidden', source,
        '&& work.reviewed_on.is_none()', '&& true',
        f'te13_fix2_{mode}_studio_keeps_review_provenance', 'TE13_FIX2_REVIEWED_NO_REOFFER', APP)
add('unsaved-session-is-not-inspection-error', W,
    'if self.destination.is_none() {', 'if false {',
    'editing_session::retained_work::tests::te13_fix2_unsaved_session_has_no_history_inspection_failure',
    'TE13_FIX2_UNSAVED_NO_INSPECTION_FAILURE', P)
add('receipt-state-invalidates-disclosure', W,
    '                &bundle.issuers,', '                &(),',
    'editing_session::retained_work::tests::te13_fix2_retained_disclosure_tracks_in_memory_issuer_retirement',
    'TE13_FIX2_RECEIPT_CACHE_REFRESH', P)

# The hosted witness starts the real CLI binary. Hosted Project/HostStatus mutants must be
# rebuilt into that executable in both arms, rather than testing a stale host.
original_run = harness.run

def run_with_host(name, package, witness, marker, red):
    if name.startswith('host-'):
        import hashlib
        import subprocess
        command = 'source scripts/setup-ffmpeg.sh && flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo build -p kinewright-agent --bin kinewright'
        with (harness.OUT / f'{name}.cli-build.log').open('w') as log:
            subprocess.run(['bash', '-c', command], cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, check=True)
        binary = ROOT / 'target/debug/kinewright'
        result = original_run(name, package, witness, marker, red)
        result['host_binary_sha256'] = hashlib.sha256(binary.read_bytes()).hexdigest()
        return result
    return original_run(name, package, witness, marker, red)

harness.run = run_with_host

if __name__ == '__main__':
    harness.main()
