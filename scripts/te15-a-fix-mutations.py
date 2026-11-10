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
# Retired cold-provenance: Pending reads no longer branch on cold/warm provenance; the retained job continues after the HTTP wait. See te15-a/phase-tail.
# Retired warm-provenance: Pending reads no longer branch on cold/warm provenance; the retained job continues after the HTTP wait. See te15-a/phase-tail.
# Retired retry-uses-memo: Media reads retain their certified source even after memo eviction and do not reopen an admission lease. See te14-fix1/cold-cert-owner.
for script, attr, names in [
    ('te15-a-mutations.py', 'h', {'phase-tail','finished-deadline','begin-liveness'}),
    ('te14-fix2-mutations.py', 'harness', {'verification-phase','busy-retry','wait-deadline','stop-wait','pause-wait','worker-failure-words'}),
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
