#!/usr/bin/env python3
"""Run an exact test with empty provider folders, without changing cargo's home."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parent.parent / 'target' / 'te1-5' / 'b1-fix2'
root.mkdir(parents=True, exist_ok=True)
with tempfile.TemporaryDirectory(prefix='empty-catalog-', dir=root) as folder:
    folder = Path(folder)
    home = folder / 'home'
    codex = folder / 'codex'
    home.mkdir()
    codex.mkdir()
    environment = os.environ.copy()
    environment.update(HOME=str(home), CODEX_HOME=str(codex))
    result = subprocess.run(sys.argv[1:], env=environment, check=False)
sys.exit(result.returncode)
