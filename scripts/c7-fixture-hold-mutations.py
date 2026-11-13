#!/usr/bin/env python3
"""Reintroduce the blocking five-second fixture hold; own the cargo lock."""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'crates/kinewright-agent/src/writer_host/studio_runtime/fixture.rs'
OUT = ROOT / 'target/c7-shutdown-holds/scheduler-pair'
TEST = 'writer_host::studio_runtime::fixture::tests::c7_fixture_hold_leaves_the_runtime_available'


def replace_once(source, before, after):
    assert source.count(before) == 1, before
    return source.replace(before, after, 1)


def mutant(source):
    source = replace_once(source, 'tokio::task::block_in_place(|| {', '(|| {')
    source = replace_once(source, '                    });\n                }\n            })),',
                          '                    })();\n                }\n            })),')
    return replace_once(source,
                        '.wait_timeout_while(state, Self::RELEASE_GUARD, |state| {',
                        '.wait_timeout_while(state, Duration::from_secs(5), |state| {')


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    with open('/tmp/kinewright-cargo-heavy.lock', 'a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        original = SOURCE.read_bytes()
        digest = hashlib.sha256(original).hexdigest()
        env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_BUILD_JOBS='4',
                   RUST_TEST_THREADS='4', TMPDIR=str(ROOT / 'target/t'),
                   LOCALAPPDATA=str(ROOT / 'target/l'))
        try:
            for side in ('red', 'green'):
                SOURCE.write_bytes(mutant(original.decode()).encode() if side == 'red' else original)
                if side == 'green':
                    assert SOURCE.read_bytes() == original
                with (OUT / f'{side}.log').open('w') as log:
                    run = subprocess.run(['bash', '-c',
                        'source scripts/setup-ffmpeg.sh && exec nice -n 10 cargo test -j4 '
                        '-p kinewright-agent --lib "$1" -- --exact --nocapture', 'pair', TEST],
                        cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT)
                text = (OUT / f'{side}.log').read_text()
                assert run.returncode == (101 if side == 'red' else 0), (side, run.returncode)
                if side == 'red':
                    assert 'C7_FIXTURE_WORKER_AVAILABLE' in text
                    assert 'test result: FAILED. 0 passed; 1 failed;' in text
                else:
                    assert 'test result: ok. 1 passed; 0 failed;' in text
                print(side, run.returncode, flush=True)
        finally:
            SOURCE.write_bytes(original)
            assert SOURCE.read_bytes() == original, 'restore differs'
        (OUT / 'results.json').write_text(json.dumps({
            'pair': 'fixture-worker', 'red': 101, 'green': 0,
            'restore_sha256': digest}, indent=2) + '\n')
        print('PASS fixture-worker; exact restore', digest, flush=True)


if __name__ == '__main__':
    main()
