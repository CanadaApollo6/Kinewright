#!/usr/bin/env python3
"""DQ3 audit deliberate breaks. Run with nice python3 -I, without an outer flock."""
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
OUT = ROOT / 'target/dq3/pairs'
PAIRS = {'room-tone-both-sides': ('crates/kinewright-core/src/dialogue/audit/joins.rs',
                          'i32::from(inn.rms_cdb) - i32::from(out.rms_cdb)',
                          'i32::from(out.rms_cdb) - i32::from(out.rms_cdb)',
                          'kinewright-core',
                          'dialogue::audit::tests::dialogue_audit_quiet_twelve_db_jump_warns_with_both_guards_passing',
                          'DQ3_ROOM_TONE_USES_BOTH_SIDES'),
 'repeats-across-join': ('crates/kinewright-core/src/dialogue/audit/leftovers.rs',
                         'let across_join = saying.iter().any(|w| w.piece != '
                         'saying[0].piece);',
                         'let across_join = saying.iter().any(|w| w.piece != '
                         'saying[0].piece);\n'
                         '        if across_join { i += 2 * found; continue; }',
                         'kinewright-core',
                         'dialogue::audit::tests::dialogue_audit_repeats_cross_a_join_and_preserve_source_indices',
                         'DQ3_REPEAT_ACROSS_JOIN_IS_LISTED')}


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
        (OUT / (name + '-original')).write_bytes(original)
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
