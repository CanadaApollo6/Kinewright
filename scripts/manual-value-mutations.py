#!/usr/bin/env python3
"""Final manual value controls. Owns the cargo lock; restores exact source bytes."""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / 'target/gesture-final-value/pairs'
APP = 'crates/kinewright-app/src/'
PREFIX = 'ja1_attachment::manual_value_tests::'
CASES = {
    'latest-value': (APP+'manual_edits.rs', PREFIX+'manual_value_final_drag_is_saved_and_one_undo', 'MANUAL_FINAL_VALUE_LANDS'),
    'final-refusal': (APP+'manual_edits.rs', PREFIX+'manual_value_final_capacity_refusal_is_truthful', 'MANUAL_FINAL_REFUSAL_DROPS_VALUE'),
    'pending-close': (APP+'ja1_attachment.rs', PREFIX+'manual_value_close_and_quit_ask_for_unapplied_value', 'MANUAL_CLOSE_KEEPS_PENDING_PROJECT'),
    'release-frame': (APP+'app.rs', PREFIX+'manual_value_release_frame_submits_before_finishing_gesture', 'MANUAL_RELEASE_BEFORE_WIDGETS'),
    'retry-correlation': (APP+'app.rs', 'app::in1_tests::in1b_the_routers_own_rejected_send_is_dropped_not_left_outstanding', 'MANUAL_RETRY_CORRELATION'),
    'retry-reservation': (APP+'app.rs', PREFIX+'manual_value_final_drag_is_saved_and_one_undo', 'MANUAL_RETRY_RESERVATION'),
}

def mutate(name, source):
    if name == 'latest-value':
        before = '            action.latest = Some(command);\n            self.notice = None;'
        after = ('            if action.waiting_at != Some(self.acknowledgement) {\n'
                 '                action.latest = Some(command);\n'
                 '            }\n            self.notice = None;')
    elif name == 'final-refusal':
        before = 'reason: CoreMutationRefusal::ManualBackpressure,'
        after = 'reason: _,'
    elif name == 'pending-close':
        start = source.index('    pub(crate) fn manual_edits_pending(')
        end = source.index('    pub(crate) fn manual_edit_message(', start)
        return source[:start] + '    pub(crate) fn manual_edits_pending(&self) -> bool { false }\n' + source[end:]
    elif name == 'retry-correlation':
        before = '*reason != kinewright_core::CoreMutationRefusal::ManualBackpressure'
        after = 'true'
    elif name == 'retry-reservation':
        before = 'if reason != kinewright_core::CoreMutationRefusal::ManualBackpressure {'
        after = 'if true {'
    else:
        before = '        self.retirement.prune();'
        after = '        self.finish_manual_frame(ctx);\n        self.retirement.prune();'
        ending = '        self.finish_manual_frame(ui.ctx());'
        assert source.count(ending) == 1
        source = source.replace(ending, '// mutant: finish before release widgets', 1)
    assert source.count(before) == 1, 'mutation target drift: ' + before
    return source.replace(before, after, 1)

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
                        run = subprocess.run(['bash', '-c',
                            'source scripts/setup-ffmpeg.sh && exec nice -n 10 cargo test -j4 -p kinewright-app --bin kinewright-app "$1" -- --exact --nocapture',
                            'pair', witness], cwd=ROOT, env=env, stdout=stream, stderr=subprocess.STDOUT, timeout=300)
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

if __name__ == '__main__':
    main()
