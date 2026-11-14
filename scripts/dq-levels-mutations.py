#!/usr/bin/env python3
"""Level-measurement deliberate breaks. Owns the cargo lock; do not wrap in flock."""
from pathlib import Path
import argparse
import hashlib
import json
import os
import re
import shlex
import subprocess
import time

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / 'target/dq-levels-guide/pairs'
SRC = 'crates/kinewright-core/src/dialogue/measure.rs'
PREFIX = 'dialogue::measure::tests::'
PAIRS = {
    'channel-mean': ([(SRC, '.fold(0.0, f64::max)', '.sum::<f64>() / self.energy.len() as f64')],
        'stereo_reports_the_loud_channel_not_the_channel_mean', 'MEASURE_CHANNEL_MAX'),
    'float-hop': ([(SRC,
        'u128::from(frame) * 1_000_000 / (u128::from(rate.get()) * u128::from(DEFAULT_HOP_MICROS))',
        '(frame as f64 * 1_000_000.0 / (f64::from(rate.get()) * f64::from(DEFAULT_HOP_MICROS))).floor() as u128')],
        'integer_hop_clock_is_exact_beyond_float_precision', 'MEASURE_INTEGER_HOP_CLOCK'),
    'floor-duration': ([(SRC,
        '(u128::from(self.frames) * 1_000_000).div_ceil(u128::from(self.rate.get()))',
        '(u128::from(self.frames) * 1_000_000) / u128::from(self.rate.get())')],
        'duration_covers_the_last_sample_frame_and_empty_tail_hops', 'MEASURE_DURATION_CEIL'),
    'closed-voice-end': ([(SRC,
        'interval.end_micros.div_ceil(hop_us)',
        '(interval.end_micros / hop_us + 1).min(u64::try_from(count).unwrap())')],
        'voice_overlap_is_half_open_and_unavailable_is_not_no_voice', 'MEASURE_VOICE_HALF_OPEN_END'),
}


def run(label, witness):
    command = ['flock', '-o', '/tmp/kinewright-cargo-heavy.lock', 'env',
               'CARGO_BUILD_JOBS=4', 'RUST_TEST_THREADS=4', 'nice', '-n', '10',
               'cargo', 'test', '-p', 'kinewright-core', '--lib', PREFIX + witness,
               '--', '--exact', '--nocapture']
    env = os.environ.copy()
    env.update(CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=str(ROOT / 'target'),
               TMPDIR=str(ROOT / 'target/t'), LOCALAPPDATA=str(ROOT / 'target/l'))
    path = OUT / (label + '.log')
    start = time.monotonic()
    with path.open('w') as log:
        result = subprocess.run(['bash', '-c', 'source ./scripts/setup-ffmpeg.sh && exec ' + shlex.join(command)],
                                cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT, check=False)
    text = path.read_text(errors='replace')
    summaries = re.findall(r'^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed;[^\n]*', text, re.M)
    return {'exit': result.returncode, 'seconds': time.monotonic() - start,
            'log': str(path.relative_to(ROOT)), 'summaries': summaries}, text


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--pair', choices=PAIRS)
    args = parser.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)
    for name in [args.pair] if args.pair else PAIRS:
        changes, witness, marker = PAIRS[name]
        originals = {source: (ROOT / source).read_bytes() for source, _, _ in changes}
        baseline = subprocess.check_output(['git', 'diff', '--binary'], cwd=ROOT)
        try:
            for source, before, after in changes:
                path = ROOT / source
                current = path.read_bytes()
                assert current.count(before.encode()) == 1, (name, source, 'mutation anchor')
                path.write_bytes(current.replace(before.encode(), after.encode()))
            red, text = run(name + '-red', witness)
        finally:
            for source, original in originals.items():
                (ROOT / source).write_bytes(original)
                assert (ROOT / source).read_bytes() == original, (name, source, 'restore')
            assert subprocess.check_output(['git', 'diff', '--binary'], cwd=ROOT) == baseline, (name, 'diff restore')
        green, _ = run(name + '-green', witness)
        proof = {'pair': name, 'marker': marker, 'marker_seen': marker in text,
                 'red': red, 'green': green,
                 'restored_sha256': {s: hashlib.sha256(b).hexdigest() for s, b in originals.items()}}
        (OUT / (name + '-result.json')).write_text(json.dumps(proof, indent=2) + '\n')
        assert red['exit'] != 0 and red['summaries'] == [('FAILED', '0', '1')] and marker in text, proof
        assert green['exit'] == 0 and green['summaries'] == [('ok', '1', '0')], proof
        print(name, 'RED 0/1 at', marker, 'GREEN 1/0; byte-identical restore', flush=True)


if __name__ == '__main__':
    main()
