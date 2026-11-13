#!/usr/bin/env python3
"""Kept-request recovery pairs. Owns the shared lock and restores exact source bytes."""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / 'target/c7-kept-retry/pairs'
APP = 'crates/kinewright-app/src/'
HOST = 'crates/kinewright-agent/src/writer_host/'
TEST = 'studio_feedback::ja_s3_feedback::c7::retry::'
OLD = 'studio_feedback::ja_s3_feedback::te15_b1_fix4::te15_b1_fix4_'
CASES = {
    'failed-recovery': (APP+'studio_feedback.rs', TEST+'c7_retry_failed_recovery_then_healthy', 'C7_RECOVERY_RETRY_ARMED'),
    'expired-wait': (HOST+'protocol.rs', TEST+'c7_retry_expired_startup_then_healthy', 'C7_RECOVERY_RETRY_ARMED'),
    'busy-waiter': (HOST+'protocol.rs', TEST+'c7_retry_busy_startup_then_healthy', 'C7_BUSY_REQUEST_KEPT'),
    'restored-stop': (APP+'studio_stop.rs', TEST+'c7_retry_restored_stop_then_healthy', 'C7_HEALTHY_RESENDS_STOP'),
    'manual-interleave': (APP+'studio_feedback.rs', TEST+'c7_retry_manual_click_queued_at_healthy_transition_dispatches_once', 'C7_MANUAL_AND_POLL_ONE_DISPATCH'),
    'stopping-excluded': (HOST+'protocol.rs', TEST+'c7_retry_stopping_and_nonstartup_unknowns_stay_manual', 'C7_NO_UNSAFE_AUTOMATIC_RETRY'),
    'switch-cancels': (APP+'app.rs', TEST+'c7_retry_project_switch_cancels_automatic_send_but_keeps_request', 'C7_SWITCH_CANCELS_AUTOMATIC_ARM'),
    'near-waiting-slot': (APP+'studio_feedback.rs', OLD+'waiting_revise_keeps_valid_request', 'FIX4_WAITING_NOT_UNCONFIRMED'),
    'near-exact-continue': (APP+'studio_stop.rs', OLD+'continue_poll_confirms_same_request', 'FIX4_UI_POLL_REPLAY'),
}


def once(source, before, after):
    assert source.count(before) == 1, 'mutation target drift: ' + before
    return source.replace(before, after, 1)


def mutate(name, source):
    if name == 'failed-recovery':
        return once(source, 'self.revise_recovery = port.host_client().cloned();', 'self.revise_recovery = None;')
    if name in ('expired-wait', 'busy-waiter'):
        code = 'startup_wait_expired' if name == 'expired-wait' else 'startup_wait_busy'
        return once(source, '\n                | "'+code+'"', '')
    if name == 'stopping-excluded':
        return once(source, '            "recovery_frozen"', '            "host_stopping" | "recovery_frozen"')
    if name == 'restored-stop':
        return once(source, 'pending.action() == Some(RunActionKind::Continue) || pending.recovery.is_some()',
                    'pending.action() == Some(RunActionKind::Continue)')
    if name == 'manual-interleave':
        start = source.index('    fn start_run(')
        end = source.index('    /// Whether a linked run', start)
        body = once(source[start:end], '            self.revise = None;',
                    '            // mutant: keep the settled request eligible for a queued click')
        return source[:start] + body + source[end:]
    if name == 'switch-cancels':
        return once(source, '        self.focused_mut().studio.feedback.cancel_recovery_retry();\n', '')
    if name == 'near-waiting-slot':
        return once(source, 'if !matches!(start, RunStart::Unknown(_) | RunStart::RecoveryUnknown(_)) {',
                    'if !matches!(start, RunStart::Unknown(_) | RunStart::RecoveryUnknown(_) | RunStart::Known(ReviseResult::Waiting { .. })) {')
    start = source.index('    pub(crate) fn retry_ready_controls(')
    end = source.index('    pub(crate) fn retry_after_recovery(', start)
    return source[:start] + '    pub(crate) fn retry_ready_controls(&mut self, _port: &mut dyn FeedbackPort) {}\n' + source[end:]


def main():
    selected = sys.argv[1:] or list(CASES)
    assert all(name in CASES for name in selected)
    OUT.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, CARGO_BUILD_JOBS='4', CARGO_INCREMENTAL='0', RUST_TEST_THREADS='4',
               TMPDIR=str(ROOT/'target/t'), LOCALAPPDATA=str(ROOT/'target/l'), PYTHONDONTWRITEBYTECODE='1')
    rows = []
    for name in selected:
        print('WAIT', name, flush=True)
        with open('/tmp/kinewright-cargo-heavy.lock', 'a') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            file, witness, marker = CASES[name]
            path = ROOT/file
            original = path.read_bytes()
            digest = hashlib.sha256(original).hexdigest()
            try:
                for side in ('red', 'green'):
                    path.write_text(mutate(name, original.decode())) if side=='red' else path.write_bytes(original)
                    if side=='green': assert path.read_bytes()==original, 'restore differs'
                    log = OUT/f'{name}-{side}.log'
                    with log.open('w') as stream:
                        run = subprocess.run(['bash','-c',
                            'source scripts/setup-ffmpeg.sh && exec nice -n 10 cargo test -j4 -p kinewright-app --features slow-tests,kinewright-agent/te1-eval --bin kinewright-app "$1" -- --exact --nocapture',
                            'pair', witness], cwd=ROOT, env=env, stdout=stream, stderr=subprocess.STDOUT)
                    text = log.read_text()
                    summary = 'test result: FAILED. 0 passed; 1 failed;' if side=='red' else 'test result: ok. 1 passed; 0 failed;'
                    assert run.returncode==(101 if side=='red' else 0) and summary in text, (name, side, run.returncode)
                    assert side!='red' or marker in text, (name, 'wrong red')
                    print(name, side, run.returncode, flush=True)
            finally:
                path.write_bytes(original)
                assert path.read_bytes()==original, 'final restore differs'
            rows.append(dict(pair=name, red=101, green=0, restore_sha256=digest, source=file))
            (OUT/'results.json').write_text(json.dumps(rows, indent=2)+'\n')
            print('PASS', name, 'byte-identical restore', digest, flush=True)


if __name__=='__main__':
    main()
