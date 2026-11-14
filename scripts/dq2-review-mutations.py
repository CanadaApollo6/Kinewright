#!/usr/bin/env python3
"""DQ2/DQ4 review deliberate breaks. Run with nice python3 -I, without an outer flock."""
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
OUT = ROOT / 'target/dq3/review-pairs'
PAIRS = {'dip-word-order': ('crates/kinewright-core/src/dialogue/resolve.rs',
                    'if let Some((index, _, depth)) = self.deepest_intrusion(n, d) {',
                    'if dip { return Ok(()); }\n'
                    '        if let Some((index, _, depth)) = self.deepest_intrusion(n, d) {',
                    'kinewright-core',
                    'dialogue::tests::dialogue_dip_inside_word_beyond_reach_is_refused',
                    'DQ2_DIP_INTRUSION_BOUND'),
 'fractional-guard': ('crates/kinewright-core/src/dialogue/frame.rs',
                      'self.guard_ratio(instant_numerator, n, edge.side)?;',
                      'self.guard_ratio(instant_numerator / n, 1, edge.side)?;',
                      'kinewright-core',
                      'dialogue::tests::dialogue_frame_guard_checks_the_current_profile_and_exact_fraction',
                      'DQ2_FRACTIONAL_GUARD_NOT_TRUNCATED'),
 'exclusive-dip-end': ('crates/kinewright-core/src/dialogue/frame.rs',
                       'if support.inclusive_end {',
                       'if true {',
                       'kinewright-core',
                       'dialogue::tests::dialogue_exact_dip_support_end_is_excluded',
                       'DQ2_DIP_SUPPORT_END_IS_EXCLUSIVE'),
 'inclusive-tolerance': ('crates/kinewright-agent/src/cut_score/matching.rs',
                         'a.abs_diff(b) <= window',
                         'a.abs_diff(b) < window',
                         'kinewright-agent',
                         'cut_score::tests::cut_score_tolerance_boundaries_include_20_100_and_1000_ms',
                         'DQ4_TOLERANCES_ARE_INCLUSIVE'),
 'maximum-cardinality': ('crates/kinewright-agent/src/cut_score/matching.rs',
                         'fn match_count(candidate: &[Edge], reference: &[Edge], window: u64) -> usize {\n'
                         '    let mut count = 0;\n'
                         '    for side in [EdgeSide::In, EdgeSide::Out] {\n'
                         '        let c = ordered(candidate, side);\n'
                         '        let r = ordered(reference, side);\n'
                         '        let (mut i, mut j) = (0, 0);\n'
                         '        while i < c.len() && j < r.len() {\n'
                         '            if within(c[i].0, r[j].0, window) {\n'
                         '                count += 1;\n'
                         '                i += 1;\n'
                         '                j += 1;\n'
                         '            } else if c[i].0 < r[j].0 {\n'
                         '                i += 1;\n'
                         '            } else {\n'
                         '                j += 1;\n'
                         '            }\n'
                         '        }\n'
                         '    }\n'
                         '    count\n'
                         '}',
                         'fn match_count(candidate: &[Edge], reference: &[Edge], window: u64) -> usize {\n'
                         '    let mut possible=Vec::new();\n'
                         '    for (i,c) in candidate.iter().enumerate(){for (j,r) in '
                         'reference.iter().enumerate(){\n'
                         '        if c.side==r.side && '
                         'within(c.at_micros,r.at_micros,window){possible.push((c.at_micros.abs_diff(r.at_micros),i,j));}\n'
                         '    }}\n'
                         '    possible.sort_unstable();let mut used_c=vec![false;candidate.len()];let mut '
                         'used_r=vec![false;reference.len()];let mut count=0;\n'
                         '    for (_,i,j) in possible{if '
                         '!used_c[i]&&!used_r[j]{used_c[i]=true;used_r[j]=true;count+=1;}}\n'
                         '    count\n'
                         '}',
                         'kinewright-agent',
                         'cut_score::tests::cut_score_competing_neighbors_maximize_matches_before_error',
                         'DQ4_EDGE_ASSIGNMENT_MAX_CARDINALITY')}


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
