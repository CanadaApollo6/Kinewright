#!/usr/bin/env python3
"""Three forced-ordering C7 shutdown witnesses. No provider or paid entry point."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import subprocess

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'crates/kinewright-agent/src/writer_host/mod.rs'
RUNTIME = SOURCE.with_name('studio_runtime.rs')
CASES = {
    'shutdown-wake': ('s3_c7_fix1_shutdown_cancels_waiting_revise',
                      'C7_SHUTDOWN_REFUSES_WITH_RECOVERY_HELD'),
    'ingress-fence': ('s3_c7_fix1_shutdown_fences_ingress_ready_revise',
                      'C7_SHUTDOWN_BEFORE_DISPATCH'),
    'refused-shutdown': ('s3_c7_fix1_refused_shutdown_keeps_queue_open',
                         'C7_REFUSED_SHUTDOWN_KEEPS_QUEUE_OPEN'),
}


def replace_once(data, before, after):
    assert data.count(before) == 1, before
    return data.replace(before, after, 1)


def mutate(name, data):
    if name == 'shutdown-wake':
        return replace_once(data, b'        self.changed.notify_waiters();',
                            b'        // mutant: shutdown does not wake startup waiters')
    if name == 'ingress-fence':
        data = replace_once(data, b'''            tokio::select! {
                biased;
                () = state.lifecycle.stopped() => None,
                permit = Arc::clone(&state.ingress).acquire_owned() => {
                    permit.ok().filter(|_| !state.lifecycle.stopping())
                }
            }''', b'            Arc::clone(&state.ingress).acquire_owned().await.ok()')
        return replace_once(data, b'''    if state.lifecycle.stopping() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
''', b'')
    data = replace_once(data, b'''            HostCommand::Shutdown => {
                if self.hooks.runs''', b'''            HostCommand::Shutdown => {
                self.fence_shutdown();
                if self.hooks.runs''')
    return replace_once(data, b'''                self.fence_shutdown();
                for session in self.sessions.values_mut()''',
                        b'                for session in self.sessions.values_mut()')


def run(name, side, output):
    witness = 'studio_feedback::ja_s3_feedback::c7::' + CASES[name][0]
    cargo = ['cargo', 'test', '-j4', '-p', 'kinewright-app', '--features', 'slow-tests',
             '--bin', 'kinewright-app', witness, '--', '--exact', '--nocapture']
    command = ('source scripts/setup-ffmpeg.sh; exec flock -o '
               '/tmp/kinewright-cargo-heavy.lock nice -n 10 ' + shlex.join(cargo))
    env = dict(os.environ, CARGO_BUILD_JOBS='4', CARGO_INCREMENTAL='0',
               RUST_TEST_THREADS='4', TMPDIR=str(ROOT / 'target/t'),
               LOCALAPPDATA=str(ROOT / 'target/l'))
    for key in ('TMPDIR', 'LOCALAPPDATA'):
        Path(env[key]).mkdir(parents=True, exist_ok=True)
    log = output / f'{name}-{side}.log'
    with log.open('w') as stream:
        result = subprocess.run(['bash', '-c', command], cwd=ROOT, env=env,
                                stdout=stream, stderr=subprocess.STDOUT)
    (output / f'{name}-{side}.exit').write_text(str(result.returncode) + '\n')
    return result.returncode, log.read_text()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('pair', choices=tuple(CASES) + ('all',), default='all', nargs='?')
    parser.add_argument('--out', type=Path, default=ROOT / 'target/c7-shutdown-fix1/pairs')
    args = parser.parse_args()
    output = args.out.resolve()
    output.mkdir(parents=True, exist_ok=True)
    original = SOURCE.read_bytes()
    runtime = RUNTIME.read_bytes()
    digest = hashlib.sha256(original).hexdigest()
    runtime_digest = hashlib.sha256(runtime).hexdigest()
    (output / 'mod.rs.restore').write_bytes(original)
    (output / 'studio_runtime.rs.restore').write_bytes(runtime)
    results = []
    for name in CASES if args.pair == 'all' else (args.pair,):
        print(f'START {name}', flush=True)
        try:
            SOURCE.write_bytes(mutate(name, original))
            if name == 'shutdown-wake':
                # Both the host fence and runtime cancellation notify the waiter.
                RUNTIME.write_bytes(replace_once(
                    runtime, b'        self.lifecycle.changed.notify_waiters();',
                    b'        // mutant: cancellation does not wake startup waiters'))
            red, red_text = run(name, 'red', output)
        finally:
            SOURCE.write_bytes(original)
            RUNTIME.write_bytes(runtime)
            assert SOURCE.read_bytes() == original, 'restore differs'
            assert RUNTIME.read_bytes() == runtime, 'runtime restore differs'
        assert red == 101 and CASES[name][1] in red_text, (name, red, 'wrong red')
        green, green_text = run(name, 'green', output)
        assert green == 0 and 'test result: ok. 1 passed;' in green_text, (name, green)
        assert SOURCE.read_bytes() == original, 'green changed source'
        assert RUNTIME.read_bytes() == runtime, 'green changed runtime'
        results.append(dict(pair=name, red=red, green=green, restore_sha256=digest,
                            runtime_restore_sha256=runtime_digest))
        (output / 'results.json').write_text(json.dumps(results, indent=2) + '\n')
        print(f'PASS {name}: red={red}, green={green}, restore={digest}', flush=True)


if __name__ == '__main__':
    main()
