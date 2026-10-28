"""Summarize raw local gates, keeping early-return controls separate from test counts."""
import gzip
import hashlib
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[5]
OUT = Path(__file__).resolve().parent


def digest(data):
    return hashlib.sha256(data).hexdigest()


def lane(label):
    archive = OUT / f'{label}.log.gz'
    body = gzip.open(archive, 'rt', errors='replace').read()
    raw = [line for line in body.splitlines() if line.startswith('test result:')]
    totals = [tuple(map(int, row)) for row in re.findall(
        r'test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;', body)]
    result = {
        'command': (OUT / f'{label}.command').read_text().strip(),
        'exit': int((OUT / f'{label}.exit').read_text()),
        'log': archive.name, 'log_sha256': digest(archive.read_bytes()),
        'raw_summaries': raw,
        'aggregate': dict(zip(['passed', 'failed', 'ignored'], map(sum, zip(*totals)))) if totals else {},
        'failed_tests': re.findall(r'^test (.+) \.\.\. FAILED$', body, re.M),
        'ignored_tests': re.findall(r'^test (.+) \.\.\. ignored[^\n]*$', body, re.M),
        'not_run_messages': [line for line in body.splitlines() if 'NOT RUN' in line],
    }
    if label == 'workspace-fast':
        result['skip_verification_lines'] = [line for line in body.splitlines()
                                             if 'verified' in line or 'unexpected' in line or 'skipped' in line]
    if label == 'windows':
        result['dependency_diagnostics'] = [line for line in body.splitlines()
                                             if 'pthread_rwlock_t' in line or 'aws-lc-sys' in line][:12]
    return result


def main():
    source = {}
    for line in (OUT / 'source.sha256').read_text().splitlines():
        wanted, relative = line.split('  ', 1)
        assert digest((ROOT / relative).read_bytes()) == wanted, relative
        source[relative] = wanted
    gates = {name: lane(name) for name in ['app', 'project', 'project-test-util', 'media',
                                          'build', 'clippy', 'fmt', 'windows', 'workspace-fast']}
    mutations = json.loads((OUT / 'mutations/summary.json').read_text())
    assert len(mutations) == 3
    assert all(row['runtime_red'] and row['runtime_green'] and row['restored_exactly'] for row in mutations)
    record = {
        'parent_commit': subprocess.check_output(['git', 'rev-parse', '06ad2b18'], cwd=ROOT, text=True).strip(),
        'branch': subprocess.check_output(['git', 'branch', '--show-current'], cwd=ROOT, text=True).strip(),
        'source_sha256': source, 'gates': gates,
        'runtime_red_restored_green_pairs': len(mutations),
        'mutations': mutations,
        'not_run': (OUT / 'NOT-RUN.md').read_text(),
    }
    (OUT / 'gate-summary.json').write_text(json.dumps(record, indent=2) + '\n')
    for name, gate in gates.items():
        print(name, 'exit', gate['exit'], gate['aggregate'])
    print('runtime red/restored-green pairs:', len(mutations))


if __name__ == '__main__':
    main()
