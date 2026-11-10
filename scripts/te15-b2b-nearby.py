#!/usr/bin/env python3
"""Only adjacent B2b retained guards; no import of the full historical registry."""
import importlib.util
import os
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('nearby_engine', ROOT/'scripts/te15-b2a-mutations.py')
e = importlib.util.module_from_spec(spec)
spec.loader.exec_module(e)
e.OUT = Path(os.environ.get('KINEWRIGHT_B2B_NEARBY', ROOT/'target/te1-5/b2b/nearby'))
A = 'crates/kinewright-agent/src/'
W = A+'writer_host/'
P = 'crates/kinewright-project/src/orchestration_commit.rs'
H = 'writer_host::hooks_tests::tests::'
C = H+'tests::consent_followups::tests::te15_b21f_'
LIVE = H+'te15_b2b0_guardian_long_input_and_stop'
e.PAIRS = []
def add(name, file, before, after, witness, marker):
    e.PAIRS.append((name, file, before, after, witness, marker))
for test in ['after_history', 'setup_publish']:
    add('rollback-'+test, W+'orchestration.rs',
        'if publication.is_err() && saved.is_none() {',
        'if false && publication.is_err() && saved.is_none() {',
        H+'te15_b202_'+test, 'B202_UNPUBLISHED_ROLLED_BACK')
add('history-base-consistency', W+'task_history.rs',
    'if base.base_head != brief.scope.base.head {',
    'if false && base.base_head != brief.scope.base.head {',
    H+'te15_b202_history_base_guard', 'B202_MIXED_BASE_REFUSED')
add('real-recovery-controls', W+'task_preparation_words.rs',
    'Save current edit, then open <branch>', 'Reopen',
    'app::ja1_attachment::tests::te15_b202_sent_recovery_controls', 'B202_REAL_RECOVERY_CONTROL')
add('grant-fixed-at-store', P, 'if after.edit_consent != before.edit_consent {',
    'if false {', C+'grant_is_fixed', 'FOL_FIXED_STORE')
add('approvals-append-at-store', P, '!after.edit_approvals.starts_with(&before.edit_approvals)',
    'false', C+'approvals_are_append_only', 'FOL_FIXED_STORE')
add('applied-only-after-success', W+'task_consent.rs', 'if outcome.applied() {',
    'if true {', C+'failed_core_is_not_applied', 'FOL_NOT_APPLIED')
add('guardian-input', A+'job_guardian/linux.rs',
    '    command.stdin(stdin.unwrap_or_else(Stdio::null));', '    drop(stdin);',
    LIVE, 'B2B0_CHILD_EXACT_BYTES')
text = (ROOT/(A+'te1_eval/analysis.rs')).read_text()
start = text.index('        let mut usage_complete =')
end = text.index('        Ok(Self {', start)
add('coverage-turn', A+'te1_eval/analysis.rs', text[start:end],
    '        let usage_complete = !accounting.tasks.is_empty() && accounting.tasks.iter().all(|t| t.totals.tokens_complete);\n',
    'coverage::te15_b2af1_uncovered_turn', 'TE15_B2AF1_TURN')
for kind in ['frozen', 'history', 'legacy', 'base']:
    add('permanent-'+kind, W+'task_preparation.rs', 'if kind.permanent() {',
        'if false && kind.permanent() {', H+'te15_b202_permanent_'+kind, 'B202_PERMANENT_NO_RETRY')
text = (ROOT/(W+'studio_runtime.rs')).read_text()
for name, function, end_at, witness, marker in [
    ('canonical-final', '    pub(super) fn record_runtime_refusal(', '    fn surface_unknown_send(', H+'te15_b1_fix3_postcharge_ambiguity_is_not_resumable', 'FIX4_FINAL_UNKNOWN_WORDS'),
    ('canonical-stream', '    pub(super) fn record_stream_refusal(', '    /// C5 fix 10: a run that can make no progress', H+'te15_b1_followup_unknown_wait_survives_assign_and_stream', 'FOLLOWUP_UNKNOWN_STREAM_WAIT'),
]:
    start = text.index(function)
    body = text[start:text.index(end_at, start)]
    add(name, W+'studio_runtime.rs', body,
        body.replace('if self.surface_unknown_send(producer, &state)? {', 'if false {'),
        witness, marker)
original_run = e.run
def run(label, witness, marker, red):
    if witness == LIVE:
        with (e.OUT/(label+'-build.log')).open('w') as log:
            subprocess.run(['bash', '-c', 'source scripts/setup-ffmpeg.sh && exec flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 nice -n19 cargo build -j4 -p kinewright-agent --bin kinewright-job-guardian'], cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, check=True)
    if not witness.startswith('app::'):
        return original_run(label, witness, marker, red)
    command = ['bash', '-c', 'source scripts/setup-ffmpeg.sh && exec flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 nice -n19 cargo test -j4 -p kinewright-app --features kinewright-agent/te1-eval --bin kinewright-app "$1" -- --exact --nocapture', 'nearby', witness.removeprefix('app::')]
    with (e.OUT/(label+'.log')).open('w') as log:
        e.active = subprocess.Popen(command, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            code = e.wait_for_cargo(e.active, e.OUT/f'{label}.log')
        finally:
            if e.active.poll() is None:
                e.stop(e.active)
            e.active = None
    output = (e.OUT/(label+'.log')).read_text()
    summaries = re.findall(r'^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed;', output, re.M)
    assert summaries[-1] == (('FAILED', '0', '1') if red else ('ok', '1', '0')), (label, summaries)
    assert (code != 0) == red, label
    if red:
        assert marker in output, label+': missing named failure'
    print(label, code, summaries[-1], flush=True)
    return {'exit': code, 'summary': summaries[-1], 'marker': marker if red else None}
e.run = run
if __name__ == '__main__':
    e.main()
