#!/usr/bin/env python3
"""DQ2/DQ4 deliberate breaks. Run with nice python3 -I, without an outer flock."""
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
OUT = ROOT / 'target/dq2/pairs'
PAIRS = {
    'frame-nearest': (
        'crates/kinewright-core/src/dialogue/frame.rs',
        'let selected = nearest.clamp(first, last);', 'let selected = nearest;',
        'kinewright-core',
        'dialogue::tests::dialogue_frame_rounds_into_measured_support_not_nearest_outside',
        'DQ2_FRAME_ROUNDS_INSIDE_RUN',
    ),
    'two-dips': (
        'crates/kinewright-core/src/dialogue/resolve.rs',
        'if start.is_dip() && end.is_dip() {',
        'if false && start.is_dip() && end.is_dip() {',
        'kinewright-core',
        'dialogue::tests::dialogue_two_dip_edges_refuse_a_word_from_running_speech',
        'DQ2_TWO_DIPS_REFUSED',
    ),
    'precision-recall': (
        'crates/kinewright-agent/src/cut_score/mod.rs',
        'precision: Ratio::new(tp, tp + fp),\n            recall: Ratio::new(tp, tp + missed),',
        'precision: Ratio::new(tp, tp + missed),\n            recall: Ratio::new(tp, tp + fp),',
        'kinewright-agent',
        'cut_score::tests::cut_score_precision_recall_are_not_swapped',
        'DQ4_PRECISION_RECALL_NOT_SWAPPED',
    ),
}


def run(label, package, witness):
    command = ['flock', '-o', '/tmp/kinewright-cargo-heavy.lock', 'env',
               'CARGO_BUILD_JOBS=4', 'RUST_TEST_THREADS=4', 'nice', '-n', '10',
               'cargo', 'test', '-p', package, '--lib', witness, '--', '--exact', '--nocapture']
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
    proofs = []
    for name in [args.pair] if args.pair else PAIRS:
        source, before, after, package, witness, marker = PAIRS[name]
        path = ROOT / source
        original = path.read_bytes()
        assert original.count(before.encode()) == 1, (name, 'mutation anchor')
        baseline = subprocess.check_output(['git', 'diff', '--binary'], cwd=ROOT)
        try:
            path.write_bytes(original.replace(before.encode(), after.encode()))
            red, red_text = run(name + '-red', package, witness)
        finally:
            path.write_bytes(original)
            assert path.read_bytes() == original, (name, 'byte restore')
            assert subprocess.check_output(['git', 'diff', '--binary'], cwd=ROOT) == baseline, (name, 'diff restore')
        green, _ = run(name + '-green', package, witness)
        proof = {'pair': name, 'source': source, 'original_sha256': hashlib.sha256(original).hexdigest(),
                 'restored_sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
                 'marker': marker, 'marker_seen': marker in red_text, 'red': red, 'green': green}
        proofs.append(proof)
        (OUT / (name + '-result.json')).write_text(json.dumps(proof, indent=2) + '\n')
        assert red['exit'] != 0 and red['summaries'] == [('FAILED', '0', '1')] and marker in red_text, proof
        assert green['exit'] == 0 and green['summaries'] == [('ok', '1', '0')], proof
        print(name, 'RED 0/1 at', marker, 'GREEN 1/0; exact restore', flush=True)
    (OUT / 'last-results.json').write_text(json.dumps(proofs, indent=2) + '\n')


if __name__ == '__main__':
    main()
