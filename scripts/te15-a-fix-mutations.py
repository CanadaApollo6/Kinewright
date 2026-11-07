#!/usr/bin/env python3
"""Cold/warm media timeouts: real provenance faults and nearby exact restores."""
from pathlib import Path
import importlib.util
import os
import sys
sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('te15_a_fix_base', ROOT/'scripts/te13-merge-mutations.py')
h = importlib.util.module_from_spec(spec)
spec.loader.exec_module(h)
h.OUT = Path(os.environ.get('KINEWRIGHT_TE15_A_FIX_OUT', ROOT/'target/te1-5/a-fix/pairs'))
h.PAIRS = []
P = 'crates/kinewright-project/src/owned_media.rs'
H = 'writer_host::hooks_tests::tests::'
W = H+'te15_a_fix_repeated_decode_ends_after_two_timeouts'
def add(name, before, after, witness, marker):
    h.PAIRS.append((name, P, before, after, 'kinewright-agent', witness, marker))
add('cold-provenance', '            cold_certification: true,', '            cold_certification: false,', W, 'TE15_FIX_COLD_RETRY')
add('warm-provenance', '            cold_certification: false,', '            cold_certification: true,', W, 'TE15_FIX_WARM_TERMINAL')
add('retry-uses-memo', 'let memo = self.media_admission_memo(entry)?;', 'let memo = None;', H+'te15_a_fix_cold_decode_succeeds_on_warm_retry', 'TE15_FIX_REAL_WARM_MEMO')
for script, attr, names in [
    ('te15-a-mutations.py', 'h', {'phase-tail','finished-deadline','begin-liveness'}),
    ('te14-fix2-mutations.py', 'harness', {'verification-phase','decode-phase','busy-retry','wait-deadline','stop-wait','pause-wait','worker-failure-words'}),
    ('te14-fix1-mutations.py', 'harness', {'decoder-cancel'}),
]:
    spec = importlib.util.spec_from_file_location('te15_fix_near_'+script.replace('-','_'), ROOT/'scripts'/script)
    registry = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(registry)
    selected = [row for row in getattr(registry, attr).PAIRS if row[0] in names]
    assert len(selected) == len(names), (script, names-{row[0] for row in selected})
    h.PAIRS.extend(('retained-'+row[0], *row[1:]) for row in selected)
if __name__ == '__main__':
    h.main()
