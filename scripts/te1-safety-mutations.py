#!/usr/bin/env python3
"""Retained TE1 safety witness: locked source stamp recheck, exact restore."""
from pathlib import Path
import importlib.util
import os
import sys

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('te1_safety_base', ROOT / 'scripts/te13-merge-mutations.py')
h = importlib.util.module_from_spec(spec)
spec.loader.exec_module(h)
h.OUT = Path(os.environ.get('KINEWRIGHT_TE1_SAFETY_OUT', ROOT / 'target/te1-safety/pairs'))
# The original safety job ran this pair directly (docs/reports/te1-safety.md).
# Keep the same guard removal and unchanged witness in the retained registry.
h.PAIRS = [(
    'locked-source-recheck',
    'crates/kinewright-agent/src/writer_host/continuation.rs',
    '        prepared.recheck(&descriptor, self.owner()?.store()?.owner_epoch())?;',
    '        let _ = &descriptor;',
    'kinewright-agent',
    'writer_host::hooks_tests::tests::te1_safety_source_changed_between_certification_and_binding_starts_nothing',
    'TE1_SAFETY_RECHECK: changed input started a run',
)]
if __name__ == '__main__':
    h.main()
