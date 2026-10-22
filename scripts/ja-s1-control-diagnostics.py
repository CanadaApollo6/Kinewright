"""Print bounded prerequisite diagnostics for failed sealed predecessor controls."""
import json
from pathlib import Path
import sys

for name in ('format-corpus', 'history-loss', 'reply-retry', 'legacy-io'):
    report = Path(sys.argv[1]) / name / 'summary.json'
    if not report.exists():
        continue
    try:
        with report.open('rb') as stream:
            raw = stream.read((1 << 20) + 1)
        if len(raw) > 1 << 20:
            raise ValueError('summary exceeds 1 MiB diagnostic bound')
        row = json.loads(raw)
        print(json.dumps({'control': name, 'disposition': row.get('disposition'),
                          'missing_requirements': str(row.get('missing_requirements'))[:2048]}))
    except (OSError, ValueError) as error:
        print(json.dumps({'control': name, 'diagnostic_error': str(error)[:512]}))
