#!/usr/bin/env python3
"""Dialogue-tool deliberate breaks. Owns the cargo lock; do not wrap in flock."""
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
OUT = ROOT / 'target/dq-tools/pairs'
CUT = 'crates/kinewright-agent/src/dialogue_tools/cut.rs'
SERVER = 'crates/kinewright-agent/src/dialogue_tools/server.rs'
PREFIX = 'server::tests::dialogue_tool_tests::'
PAIRS = {
    'ignore-guard': ([(CUT, 'if guard[index] {', 'if false && guard[index] {')],
                     'dq_tools_unsafe_heard_edges_refuse_without_editing', 'DQ_TOOLS_ACOUSTIC_GUARD_REQUIRED'),
    # Both the proposal and the independent final keep-side check implement
    # this policy. Reverse it at both sites, retaining the minimum-cut check.
    'remove-side-rounding': ([(CUT,
        'AudibleSide::Out => kinewright_core::FrameRounding::Ceil,\n        AudibleSide::In => kinewright_core::FrameRounding::Floor,',
        'AudibleSide::Out => kinewright_core::FrameRounding::Floor,\n        AudibleSide::In => kinewright_core::FrameRounding::Ceil,'),
        (CUT, 'if edges[0].is_some_and(|(id, t)| id == asset && t < proposal[0])\n        || edges[1].is_some_and(|(id, t)| id == asset && t > proposal[1])', 'if false && (edges[0].is_some_and(|(id, t)| id == asset && t < proposal[0])\n        || edges[1].is_some_and(|(id, t)| id == asset && t > proposal[1]))')],
        'dq_tools_unequal_rates_keep_both_sides_and_ten_ntsc_cuts', 'DQ_TOOLS_ROUND_KEEP_IN'),
    'precut-readback': ([(SERVER,
        'dialogue::check_created(doc, plan.track, &plan.created, &plan.bindings)',
        'dialogue::check_created(&before, plan.track, &plan.created, &plan.bindings)')],
        'dq_tools_word_cut_atomic_undo_and_fresh_readback', 'DQ_TOOLS_FRESH_COMMITTED_READBACK'),
    'partial-refused-batch': ([(SERVER, 'if !plan.valid {', 'if false && !plan.valid {')],
        'dq_tools_pause_air_and_whole_batch_refusal', 'DQ_TOOLS_WHOLE_BATCH_REFUSED'),
    'stored-end-guard': ([(CUT,
        'let instant = if direction == AudibleSide::Out {\n                s.source.end',
        'let instant = if direction == AudibleSide::Out {\n                ExactMicros::frames(doc.clip(s.clip).unwrap().source_range.end,doc.asset(s.asset).unwrap().fps).unwrap()')],
        'dq_tools_mixed_clock_guards_heard_edges_and_moves_safely', 'DQ_TOOLS_GUARD_AT_AUDIBLE_END'),
}


def run(label, witness):
    command = ['flock', '-o', '/tmp/kinewright-cargo-heavy.lock', 'env',
               'CARGO_BUILD_JOBS=4', 'RUST_TEST_THREADS=4', 'nice', '-n', '10',
               'cargo', 'test', '-p', 'kinewright-agent', '--lib', PREFIX + witness,
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
