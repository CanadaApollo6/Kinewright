#!/usr/bin/env python3
"""Forced partial-Quit, acknowledgement and media-activation pairs; owns the cargo lock."""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'crates/kinewright-app/src/app.rs'
OUT = ROOT / 'target/process-exit-fix2/pairs'
PREFIX = 'app::process_exit_fix1_tests::fix2::'
CASES = {
    'pending-legacy': ('exit_fix2_incomplete_quit_removes_healthy_legacy_project', 'EXIT_FIX2_NO_DEAD_HEALTHY_TAB'),
    'pending-studio': ('exit_fix2_incomplete_quit_removes_healthy_studio_project', 'EXIT_FIX2_NO_DEAD_HEALTHY_TAB'),
    'sidecar-before-other-host': ('exit_fix2_sidecar_ack_follows_other_projects_saves_and_host_stop', 'EXIT_FIX2_ALL_SAVES_AND_HOST_STOPS_PRECEDE_ACK'),
    'recorder-before-other-host': ('exit_fix2_recorder_io_follows_other_projects_saves_and_host_stop', 'EXIT_FIX2_ALL_SAVES_AND_HOST_STOPS_PRECEDE_ACK'),
    'activation-revokes-export': ('exit_fix2_studio_activation_preserves_running_and_queued_session_media_exports', 'EXIT_FIX2_ACTIVATION_REFUSES_BEFORE_REVOCATION'),
}


def once(data, before, after):
    assert data.count(before) == 1, 'mutation target drift'
    return data.replace(before, after, 1)


def mutate(name, source):
    if name == 'activation-revokes-export':
        return once(source, '''        if self.project_has_exports(self.focused_project) {
            self.status =
                "Wait for the export to finish, or cancel it, before turning on Studio".into();
            return;
        }
''', '')
    start = source.index('    pub(crate) fn request_exit(')
    end = source.index('\n    fn handle_close_request', start)
    body = source[start:end]
    if name.startswith('pending-'):
        body = once(body, '                self.remove_closed_project(index);',
                    '                // mutant: leave a closed owner in the project list')
    else:
        body = once(body, '.map(|project| project.request_close("Kinewright is closing"))',
                    '.map(|project| { let prepared = project.request_close("Kinewright is closing"); let _ = project.finish_close(prepared); prepared })')
    return source[:start] + body + source[end:]


def main():
    selected = sys.argv[1:] or list(CASES)
    assert all(name in CASES for name in selected)
    OUT.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, CARGO_BUILD_JOBS='4', CARGO_INCREMENTAL='0',
               RUST_TEST_THREADS='4', TMPDIR=str(ROOT / 'target/t'),
               LOCALAPPDATA=str(ROOT / 'target/l'))
    rows = []
    for name in selected:
        print('WAIT', name, flush=True)
        with open('/tmp/kinewright-cargo-heavy.lock', 'a') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            original = SOURCE.read_bytes()
            digest = hashlib.sha256(original).hexdigest()
            try:
                for color in ('red', 'green'):
                    if color == 'red':
                        SOURCE.write_text(mutate(name, original.decode()))
                    else:
                        SOURCE.write_bytes(original)
                        assert SOURCE.read_bytes() == original, 'restore differs'
                    log = OUT / f'{name}-{color}.log'
                    with log.open('w') as stream:
                        result = subprocess.run(['bash', '-c',
                            'source scripts/setup-ffmpeg.sh && exec nice -n 10 cargo test -j4 -p kinewright-app --features slow-tests --bin kinewright-app "$1" -- --exact --nocapture',
                            'pair', PREFIX + CASES[name][0]], cwd=ROOT, env=env,
                            stdout=stream, stderr=subprocess.STDOUT)
                    text = log.read_text()
                    expected = ('test result: FAILED. 0 passed; 1 failed;' if color == 'red'
                                else 'test result: ok. 1 passed; 0 failed;')
                    assert result.returncode == (101 if color == 'red' else 0) and expected in text, (name, color, result.returncode)
                    assert color != 'red' or CASES[name][1] in text, (name, 'wrong red')
                    print(name, color, result.returncode, flush=True)
            finally:
                SOURCE.write_bytes(original)
                assert SOURCE.read_bytes() == original, 'final restore differs'
            rows.append(dict(pair=name, red=101, green=0, restored_sha256=digest))
            (OUT / 'results.json').write_text(json.dumps(rows, indent=2) + '\n')
            print('PASS', name, 'byte-identical restore', digest, flush=True)


if __name__ == '__main__':
    main()
