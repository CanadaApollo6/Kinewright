#!/usr/bin/env python3
"""Native-history C4/C5/C6 mutations: sealed controls and exact runtime outcomes.

Run after `source scripts/setup-ffmpeg.sh`, with nice -n 19 python3 -I.
A missing prerequisite, failed build or ignored witness is never a runtime red.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parent.parent
CATALOGUE = ROOT / 'scripts/undo-retargeted-mutations.json'
LOCK = '/tmp/kinewright-cargo-heavy.lock'
PREFIX = ['flock', '-o', LOCK, 'env', 'CARGO_BUILD_JOBS=4', 'nice', '-n', '10']


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def catalogue(name):
    return json.loads(CATALOGUE.read_text())['catalogues'][name]


def cases(name='c6'):
    return {row['name']: row for row in catalogue(name)['cases'] if row['status'] != 'retired'}


def apply_edits(row):
    originals = {}
    for edit in row['edits']:
        path = ROOT / edit['file']
        if not path.resolve().is_relative_to(ROOT) or not path.is_file():
            raise SystemExit(f"{row['name']}: missing/nonlocal source {path}")
        originals.setdefault(edit['file'], path.read_bytes())
    changed = {file: data.decode() for file, data in originals.items()}
    for edit in row['edits']:
        file, before, after = edit['file'], edit['old'], edit['new']
        if changed[file].count(before) != 1 or before == after:
            raise SystemExit(f"{row['name']}: nonunique/no-op edit in {file}")
        changed[file] = changed[file].replace(before, after)
    return originals, {file: data.encode() for file, data in changed.items()}


def check(name):
    section = catalogue(name)
    rows = section['cases']
    names = sorted(row['name'] for row in rows)
    if len(rows) != section['original_count'] or len(set(names)) != len(names):
        raise SystemExit(f'{name}: original case accounting changed')
    if hashlib.sha256(json.dumps(names).encode()).hexdigest() != section['original_names_sha256']:
        raise SystemExit(f'{name}: an original case is unaccounted for')
    counts = {status: sum(row['status'] == status for row in rows)
              for status in ['retained', 'retargeted', 'retired']}
    if sum(counts.values()) != len(rows):
        raise SystemExit(f'{name}: unknown status')
    for row in rows:
        if row['status'] == 'retired':
            if not row.get('reason') or not row.get('replacement_witnesses'):
                raise SystemExit(f"{row['name']}: retirement needs a reason and replacement proof")
        else:
            apply_edits(row)
    print(json.dumps({'catalogue': name, 'original_count': len(rows), **counts}), flush=True)


def prerequisite():
    available = next(int(line.split()[1]) for line in Path('/proc/meminfo').read_text().splitlines()
                     if line.startswith('MemAvailable:'))
    if available < 6 * 1024 * 1024:
        raise SystemExit('NOT RUN: 6 GiB memory prerequisite unavailable; no quiet wait')


def run_logged(command, path, environment=None):
    """Keep the foreground child attached, report progress and reap on cancellation."""
    path = Path(path)
    path.with_suffix('.command.json').write_text(json.dumps(command) + '\n')
    started = time.time_ns()
    with path.open('w') as output:
        child = subprocess.Popen(command, cwd=ROOT, env=environment, stdout=output,
                                 stderr=subprocess.STDOUT, start_new_session=True)
        try:
            while True:
                try:
                    code = child.wait(timeout=50)
                    break
                except subprocess.TimeoutExpired:
                    print(f'{path.name}: foreground command still running (or queued on lock)', flush=True)
        except BaseException:
            proc = Path('/proc') / str(child.pid)
            if child.poll() is None and proc.exists():
                if (proc / 'cwd').resolve() != ROOT:
                    raise RuntimeError('refusing to signal a child outside this worktree')
                cmdline = (proc / 'cmdline').read_bytes().split(b'\0')
                if not cmdline or b'flock' not in cmdline[0]:
                    raise RuntimeError('refusing to signal an unexpected child')
                # GNU timeout owns the test's separate group. Cancel it before
                # its flock parent so interruption cannot orphan a running test.
                children = (proc / 'task' / str(child.pid) / 'children').read_text().split()
                for raw_pid in children:
                    pid = int(raw_pid)
                    descendant = Path('/proc') / str(pid)
                    if not descendant.exists():
                        continue
                    argv = (descendant / 'cmdline').read_bytes().split(b'\0')
                    cwd = (descendant / 'cwd').resolve()
                    if argv and Path(os.fsdecode(argv[0])).name == 'timeout' and cwd == ROOT \
                            and os.getpgid(pid) == pid:
                        os.killpg(pid, signal.SIGTERM)
                os.killpg(child.pid, signal.SIGTERM)
            child.wait()
            raise
    result = {'exit': code, 'started_ns': started, 'finished_ns': time.time_ns()}
    path.with_suffix('.process.json').write_text(json.dumps(result, indent=2) + '\n')
    return code, path.read_text(errors='replace')


def cargo_args(row):
    args = ['test', '-p', row['package']]
    features = []
    if row['package'] == 'kinewright-agent':
        features.append('eval-harness')
    elif row['package'] == 'kinewright-project':
        features.append('test-util')
    if row['name'].startswith('slow-'):
        features.append('slow-tests')
    if features:
        args += ['--features', ','.join(features)]
    target = row['target']
    return args + (['--lib'] if target == '--lib' else ['--bin', 'kinewright-app']
                   if target == '--bin' else ['--test', target])


def build_binary(args, directory, label):
    prerequisite()
    command = [*PREFIX, 'cargo', *args, '--no-run', '--message-format=json']
    # Compilation and lock acquisition have no test deadline. Execution below
    # starts its unchanged 600-second deadline only after acquiring the lock.
    code, output = run_logged(command, directory / (label + '.log'))
    if code:
        raise SystemExit(f'{label}: build prerequisite failed; NOT a runtime red')
    package = args[args.index('-p') + 1]
    artifacts = [json.loads(line) for line in output.splitlines() if line.startswith('{')]
    matches = [a for a in artifacts if a.get('reason') == 'compiler-artifact'
               and a.get('executable') and a.get('profile', {}).get('test')
               and a.get('manifest_path') == str(ROOT / 'crates' / package / 'Cargo.toml')]
    if len(matches) != 1:
        raise SystemExit(f'{label}: missing/ambiguous selected test binary')
    return matches[0]


def source_identity(path):
    subprocess.run(['nice', '-n', '19', sys.executable, '-I', '-B',
                    str(ROOT / 'scripts/ja1-s2-c1-evidence.py'), 'identity', str(path)],
                   cwd=ROOT, check=True)


def interrupted(signum, _frame):
    raise KeyboardInterrupt(f'cancelled by signal {signum}')


def main(name='c6'):
    check(name)
    if sys.argv[1:] == ['--check']:
        return
    if len(sys.argv) < 2:
        raise SystemExit('usage: <script> --check | EVIDENCE_DIRECTORY [CASE ...]')
    signal.signal(signal.SIGTERM, interrupted)
    evidence = Path(sys.argv[1]).resolve()
    evidence.mkdir(parents=True, exist_ok=True)
    registry = cases(name)
    selected = sys.argv[2:] or list(registry)
    for selected_name in selected:
        if selected_name not in registry:
            raise SystemExit(f'{selected_name}: unknown or explicitly retired; see catalogue')
    source_identity(evidence / 'source-before.json')
    source = (evidence / 'source-before.json').read_bytes()
    rustc = subprocess.check_output(['rustc', '-Vv'], cwd=ROOT).decode()
    (evidence / 'rustc.txt').write_text(rustc)
    app_row = dict(name='ordinary', package='kinewright-app', target='--bin')
    configurations = [cargo_args(registry[key]) for key in selected]
    if any(registry[key].get('child_app') for key in selected):
        configurations.append(cargo_args(app_row))
    baselines = {}
    for args in configurations:
        key = hashlib.sha256(json.dumps(args).encode()).hexdigest()[:16]
        if key in baselines:
            continue
        directory = evidence / 'baselines' / key
        directory.mkdir(parents=True, exist_ok=True)
        artifact = build_binary(args, directory, 'original-build')
        original = Path(artifact['executable'])
        binary_sha = digest(original)
        sealed = directory / (original.name + '-' + binary_sha)
        shutil.copy2(original, sealed)
        code, listing = run_logged([*PREFIX, 'timeout', '600', str(sealed), '--list'],
                                   directory / 'list.log')
        if code:
            raise SystemExit('sealed original cannot enumerate; NOT RUN')
        baselines[key] = dict(binary=str(sealed), sha256=binary_sha,
                              compiled=re.findall(r'^(.+): test$', listing, re.M))
        (directory / 'identity.json').write_text(json.dumps({
            'source_sha256': hashlib.sha256(source).hexdigest(), 'rustc': rustc,
            'args': args, 'profile': artifact['profile'], **baselines[key]}, indent=2) + '\n')
    source_identity(evidence / 'source-after-builds.json')
    if (evidence / 'source-after-builds.json').read_bytes() != source:
        raise SystemExit('source changed while building controls')
    app_key = hashlib.sha256(json.dumps(cargo_args(app_row)).encode()).hexdigest()[:16]
    for key in selected:
        row = registry[key]
        args = cargo_args(row)
        baseline = baselines[hashlib.sha256(json.dumps(args).encode()).hexdigest()[:16]]
        if row['witness'] not in baseline['compiled']:
            raise SystemExit(f'{key}: exact witness absent from sealed original')
        directory = evidence / key
        directory.mkdir(exist_ok=True)
        originals, mutated = apply_edits(row)
        backup = directory / 'originals'
        for file, data in originals.items():
            destination = backup / file
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(data)
        (directory / 'identity.json').write_text(json.dumps({
            'case': row, 'original_sha256': {f: hashlib.sha256(b).hexdigest() for f, b in originals.items()},
            'mutated_sha256': {f: hashlib.sha256(b).hexdigest() for f, b in mutated.items()},
        }, indent=2) + '\n')
        try:
            for phase in ['red', 'green']:
                for file, data in (mutated if phase == 'red' else originals).items():
                    (ROOT / file).write_bytes(data)
                environment = os.environ.copy()
                environment.update(row.get('environment', {}))
                if row.get('child_app'):
                    app = build_binary(cargo_args(app_row), directory, 'red-app-build')['executable'] \
                        if phase == 'red' else baselines[app_key]['binary']
                    environment['JA_HARNESS_APP_TEST_BINARY'] = app
                    (directory / (phase + '.app.json')).write_text(json.dumps({'binary': app, 'sha256': digest(app)}) + '\n')
                if phase == 'red':
                    binary = build_binary(args, directory, 'red-build')['executable']
                else:
                    binary = baseline['binary']
                    if digest(binary) != baseline['sha256']:
                        raise SystemExit('sealed original binary changed')
                test_args = [row['witness'], '--exact', '--nocapture', '--test-threads=4']
                if row.get('include_ignored'):
                    test_args.append('--include-ignored')
                code, output = run_logged([*PREFIX, 'timeout', '600', binary, *test_args],
                                          directory / (phase + '.log'), environment)
                summaries = re.findall(r'^test result:.*$', output, re.M)
                expected = r'^test result: FAILED\. 0 passed; 1 failed; 0 ignored;' if phase == 'red' \
                    else r'^test result: ok\. 1 passed; 0 failed; 0 ignored;'
                if code != (101 if phase == 'red' else 0) or len(summaries) != 1 or not re.search(expected, output, re.M):
                    raise SystemExit(f'{key}/{phase}: expected one runtime outcome, exit {code}; inspect log')
                if phase == 'red' and row.get('diagnostic') and row['diagnostic'] not in output:
                    raise SystemExit(f'{key}: failed before required oracle {row["diagnostic"]!r}')
                (directory / (phase + '.result.json')).write_text(json.dumps({
                    'exit': code, 'raw_summary': summaries, 'binary': binary,
                    'binary_sha256': digest(binary), 'source_sha256': hashlib.sha256(source).hexdigest(),
                }, indent=2) + '\n')
                print(f'{key}/{phase}: {summaries[0]}', flush=True)
        finally:
            for file, data in originals.items():
                (ROOT / file).write_bytes(data)
    source_identity(evidence / 'source-after.json')
    if (evidence / 'source-after.json').read_bytes() != source:
        raise SystemExit('source identity changed across mutation run')


if __name__ == '__main__':
    main()
