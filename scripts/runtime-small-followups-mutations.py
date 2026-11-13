#!/usr/bin/env python3
"""Runtime follow-up ordering pairs. Owns the shared lock and restores exact source bytes."""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / 'target/runtime-small-followups/pairs'
APP = 'crates/kinewright-app/src/'
HOST = 'crates/kinewright-agent/src/writer_host/'
TEST = 'studio_feedback::ja_s3_feedback::c7::retry::'
CASES = {
    'recovery-unwind': (HOST+'studio_runtime.rs', TEST+'c7_followup_panicked_recovery_releases_waiter_and_recovers', 'C7_PANIC_RELEASES_WAITER'),
    'export-close-admission': (APP+'app.rs', 'app::process_exit_fix1_tests::runtime_followup_export_preflight_cannot_slip_past_close_or_quit', 'FOLLOWUP_EXPORT_CLOSE_ADMISSION'),
}


def mutate(name, source):
    if name == 'recovery-unwind':
        start = source.index('impl Drop for RecoveryExit {')
        end = source.index('\npub(super) struct StudioRuntime', start)
        return source[:start] + 'impl Drop for RecoveryExit { fn drop(&mut self) {} }\n' + source[end:]
    assert source.count('queue.suspend_admission();') == 2
    return source.replace('queue.suspend_admission();', '// mutant: leave admission open across the unfinished-work check')


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
