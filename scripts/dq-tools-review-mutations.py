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
OUT = ROOT / 'target/dq-tools-review/pairs'
CUT = 'crates/kinewright-agent/src/dialogue_tools/cut.rs'
SERVER = 'crates/kinewright-agent/src/dialogue_tools/server.rs'
PREFIX = 'server::tests::dialogue_tool_tests::'
CHECK = 'crates/kinewright-agent/src/dialogue_tools/check.rs'
RETENTION = 'crates/kinewright-agent/src/dialogue_tools/retention.rs'
PAIRS = {
    'outside-extents': ([(RETENTION, 'if self.exact_splits {', 'if true || self.exact_splits {')],
        'dq_tools_review_unselected_sounding_tail_is_preserved', 'DQ_TOOLS_UNSELECTED_TAIL_PROTECTED'),
    'scoped-overrule': ([(CUT, '[outgoing.overrule, last.overrule]', '[first.overrule, last.overrule]')],
        'dq_tools_review_adjacent_overrule_does_not_cover_the_backed_off_edge', 'DQ_TOOLS_SCOPED_OVERRULE'),
    'one-frame': ([(CUT, 'if end.0 - start.0 < 2 {', 'if end.0 - start.0 < 1 {')],
        'dq_tools_review_one_frame_is_structurally_refused', 'DQ_TOOLS_TWO_PROJECT_FRAMES'),
    'sync-tracks': ([(CUT, 't.id != cut.track && t.sync_lock', 'false && t.id != cut.track && t.sync_lock')],
        'dq_tools_review_sync_tracks_and_atomic_undo', 'DQ_TOOLS_SYNC_EXACT_GEOMETRY'),
    'late-binding': ([('crates/kinewright-agent/src/server.rs',
        'if let Err(reason) = validate() {\n            return Ok(error_text(reason));\n        }\n        let apply =',
        'if false && let Err(reason) = validate() {\n            return Ok(error_text(reason));\n        }\n        let apply =')],
        'dq_tools_review_binding_changes_during_confirmation', 'DQ_TOOLS_BINDING_RECHECK_AFTER_CONSENT'),
    'tail-overrule': ([(CUT, 'if tail && overrule.is_none() {', 'if tail {')],
        'dq_tools_review_last_word_overrule_keeps_the_requested_end', 'DQ_TOOLS_TAIL_OVERRULE_INSTANT'),
    'half-speed': ([(CHECK, 'c.speed_percent != 100 {', 'c.speed_percent > 100 {')],
        'dq_tools_review_half_speed_is_not_checked', 'DQ_TOOLS_HALF_SPEED_NOT_CHECKED'),
    'removal-cap': ([(CUT, 'args.removals.len() > 32', 'args.removals.len() > 33')],
        'dq_tools_review_removal_count_boundary', 'DQ_TOOLS_33_REFUSED'),
}



def run(label, witness):
    command = ['flock', '-o', '/tmp/kinewright-cargo-heavy.lock', 'env',
               'CARGO_BUILD_JOBS=4', 'RUST_TEST_THREADS=4', 'nice', '-n', '10',
               'cargo', 'test', '-p', 'kinewright-agent', '--lib', (witness if '::' in witness else PREFIX + witness),
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
