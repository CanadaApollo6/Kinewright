"""Change the real strip command; require its exact arguments in the run provenance."""
from pathlib import Path
import gzip
import hashlib
import json
import os
import subprocess
import sys
assert os.getpriority(os.PRIO_PROCESS, 0) == 19
out = Path(sys.argv[1]).resolve(); out.mkdir(parents=True, exist_ok=True)
path = Path('scripts/ja-s1-predecessor-controls.sh'); original = path.read_text()
command = 'strip_arguments=(--strip-all)'
metadata = "manifest['strip_arguments']=(sealed/'strip-arguments.txt').read_text().splitlines()"
assert original.count(command) == original.count(metadata) == 1
alternate = original.replace(command, 'strip_arguments=(--strip-debug)')
rows = []
try:
    for phase, source in [('red', alternate.replace(metadata, "manifest['strip_arguments']=['--strip-all']")), ('restored', alternate)]:
        path.write_text(source)
        result = subprocess.run(['bash', str(path), str(out/phase)], capture_output=True, text=True)
        identity_path = Path('target/ja-s1-predecessor/sealed/identity.json')
        identity = json.loads(identity_path.read_text())
        assert result.returncode == 0, result.stdout[-4000:] + result.stderr[-4000:]
        matches = identity['strip_arguments'] == ['--strip-debug']
        assert matches == (phase == 'restored'), (phase, identity['strip_arguments'])
        row = {'phase':phase, 'source_sha256':hashlib.sha256(path.read_bytes()).hexdigest(), 'identity_sha256':hashlib.sha256(identity_path.read_bytes()).hexdigest(), 'actual_command_arguments':['--strip-debug'], 'recorded_arguments':identity['strip_arguments'], 'controls_exit':result.returncode, 'provenance_check_exit':0 if matches else 1}
        with gzip.open(out/(phase+'.log.gz'),'wt') as log: log.write(json.dumps(row)+'\n'+result.stdout+result.stderr)
        rows.append(row)
        print(json.dumps(row), flush=True)
finally:
    path.write_text(original)
with gzip.open(out/'results.json.gz','wt') as log: json.dump(rows,log,indent=2)
