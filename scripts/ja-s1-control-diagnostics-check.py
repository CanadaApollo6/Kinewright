"""Exercise the bounded control diagnostic and its missing-reason omission mutation."""
import gzip
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

out = Path(sys.argv[1]).resolve()
out.mkdir(parents=True, exist_ok=True)
script = Path(__file__).with_name('ja-s1-control-diagnostics.py')
source = script.read_text()
needle = "str(row.get('missing_requirements'))[:2048]"
assert source.count(needle) == 1
rows = []
with tempfile.TemporaryDirectory(prefix='ja-s1-r2-diagnostic-', dir='target') as temporary:
    root = Path(temporary)
    (root/'reply-retry').mkdir()
    reason = 'fixture cleanup deadline: ' + 'x' * 10000
    fixture = json.dumps({'disposition': 'not_run', 'missing_requirements': reason})
    (root/'reply-retry/summary.json').write_text(fixture)
    for phase, code in [('red', source.replace(needle, 'None')), ('restored', source)]:
        process = subprocess.run(['nice', '-n', '19', sys.executable, '-I', '-c', code, str(root)], capture_output=True, text=True)
        assert process.returncode == 0, process.stderr
        row = json.loads(process.stdout)
        assert row['control'] == 'reply-retry' and row['disposition'] == 'not_run'
        passed = row['missing_requirements'] == reason[:2048]
        assert passed == (phase == 'restored'), (phase, row)
        evidence = {'phase': phase, 'baseline_source_sha256': hashlib.sha256(source.encode()).hexdigest(),
                    'executed_source_sha256': hashlib.sha256(code.encode()).hexdigest(),
                    'input_sha256': hashlib.sha256(fixture.encode()).hexdigest(),
                    'process_exit': process.returncode, 'positive_control': 'not_run reply-retry diagnostic executed',
                    'assertion': 'missing_requirements equals the original reason clipped to 2048 characters',
                    'assertion_passed': passed, 'output': row}
        with gzip.open(out/f'{phase}.json.gz', 'wt') as log:
            json.dump(evidence, log, indent=2)
        rows.append(evidence)
        print(phase, 'diagnostic assertion:', 'passed' if passed else 'FAILED')
with gzip.open(out/'results.json.gz', 'wt') as log:
    json.dump(rows, log, indent=2)
