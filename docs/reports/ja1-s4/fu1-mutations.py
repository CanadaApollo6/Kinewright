"""Protected FU1 regressions. Each edit is restored before its green rerun."""
import gzip
import hashlib
import json
from pathlib import Path
import re
import signal
import subprocess

ROOT = Path(__file__).resolve().parents[3]
REPORT = ROOT / 'docs/reports/ja1-s4'
RUNNER = REPORT / 'c1-cargo.sh'
TASKS = 'crates/kinewright-agent/src/orchestration/tasks.rs'
COMMIT = 'crates/kinewright-project/src/bundle_commit.rs'
SESSION = 'crates/kinewright-project/src/editing_session.rs'
DURABLE = 'crates/kinewright-project/src/editing_session/durable.rs'
SERVER = 'crates/kinewright-agent/src/server.rs'
VALIDATE = 'crates/kinewright-project/src/orchestration_commit.rs'
RESULTS = []


def replace_once(text, before, after):
    assert text.count(before) == 1, (before, text.count(before))
    return text.replace(before, after, 1)


def old_source(path):
    return subprocess.check_output(['git', 'show', f'061573e:{path}'], cwd=ROOT).decode()


def run(name, color, args):
    result = subprocess.run(args, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    output = result.stdout.decode(errors='replace')
    log = REPORT / f'fu1-{name}-{color}.log.gz'
    with gzip.open(log, 'wb') as archive:
        archive.write(result.stdout)
    return result.returncode, output, log


def pair(name, edits, cargo=None):
    originals = {path: (ROOT / path).read_bytes() for path in edits}
    args = ['bash', str(RUNNER), *cargo] if cargo else ['nice', '-n', '19', 'python3', '-I', str(REPORT / 'fu1-audit.py')]
    try:
        for path, change in edits.items():
            (ROOT / path).write_text(change(originals[path].decode()))
        red_code, red, red_log = run(name, 'red', args)
    finally:
        for path, content in originals.items():
            (ROOT / path).write_bytes(content)
    if cargo:
        assert red_code != 0 and re.search(r'test result: FAILED\. .*? [1-9][0-9]* failed;', red) and 'panicked at' in red, red[-6000:]
    else:
        assert red_code != 0 and 'AssertionError' in red, red
    green_code, green, green_log = run(name, 'green', args)
    assert green_code == 0, green[-6000:]
    if cargo:
        assert re.search(r'test result: ok\. [1-9][0-9]* passed;', green), green[-6000:]
    else:
        assert 'PASS' in green, green
    RESULTS.append({
        'name': name, 'command': args,
        'restored_sources': {path: hashlib.sha256(content).hexdigest() for path, content in originals.items()},
        'red': [line for line in red.splitlines() if 'test result:' in line or 'AssertionError' in line],
        'green': [line for line in green.splitlines() if 'test result:' in line or 'PASS' in line],
        'logs': {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in [red_log, green_log]},
    })
    (REPORT / 'fu1-mutations.json').write_text(json.dumps(RESULTS, indent=2) + '\n')
    print(f'{name}: runtime red/restored green' if cargo else f'{name}: audit red/restored green', flush=True)


def interrupted(_signal, _frame):
    raise SystemExit('terminated')


signal.signal(signal.SIGTERM, interrupted)
project = ['test', '-p', 'kinewright-project', '--features', 'test-util']
agent = ['test', '-p', 'kinewright-agent']
serial = ['--', '--test-threads=1']
pair('M1-candidate-refusal', {DURABLE: lambda _: old_source(DURABLE), SESSION: lambda _: old_source(SESSION)}, project + ['--test', 'ja_s4_candidate', 'fu1_'] + serial)
pair('M2-production-admission', {SERVER: lambda text: replace_once(text, 'Ok(guard) => Some(guard),', 'Ok(guard) => { drop(guard); Some(()) },')}, agent + ['--lib', 's4_drain_and_durable_base'] + serial)
pair('A1-receipt-before-authority', {TASKS: lambda text: replace_once(text, '''        if let Some(envelope) = &self.merge_envelope {
            if envelope.id != id {
                return Err(BundleError::HistoryChanged);
            }
            let receipt = owner.manual_transition_receipt(envelope)?;
            if receipt != ReceiptLookup::Absent {
                return Ok(receipt);
            }
        }
''', '')}, agent + ['--lib', 's4_drain_and_durable_base'] + serial)

def unknown_branch(text):
    text = replace_once(text, '''        let checkpoint = bundle
            .branches
            .get(target)
            .ok_or_else(|| BundleError::Unsupported("unknown branch".into()))?;
''', '')
    text = replace_once(text, 'envelope.expected_head.as_ref() != Some(&checkpoint.head_id)', 'envelope.expected_head.as_ref() != bundle.branches.get(target).map(|c| &c.head_id)')
    return replace_once(text, '''        {
            let acknowledged = if let Some(cached) = bundle.cached_history(target) {''', '''        if let Some(checkpoint) = bundle.branches.get(target) {
            let acknowledged = if let Some(cached) = bundle.cached_history(target) {''')

pair('M4-unknown-branch', {COMMIT: unknown_branch}, project + ['--lib', 'fu1_unknown_branch'] + serial)
pair('M8-task-close', {TASKS: lambda text: replace_once(text, '        owner.close_durable_branch(&self.registration)?;\n', '')}, agent + ['--lib', 's4_drain_and_durable_base'] + serial)
pair('C4-R2-1-background-settlement', {TASKS: lambda text: replace_once(text, '            task.progress.background = 0;', '            let _ = task;')}, agent + ['--test', 'ja_s4_lifecycle', 'fu1_expired_background'] + serial)
pair('C4-R2-2-cancelled-ready', {VALIDATE: lambda text: replace_once(text, '''                .any(|task| !(matches!(task.state, TaskState::Terminal(_))
                    || task.parent.is_some() && task.state == TaskState::Cancelled))''', '''                .any(|task| !matches!(task.state, TaskState::Terminal(_)))''')}, agent + ['--test', 'ja_s4_lifecycle', 'fu1_cancelled_reviewer'] + serial)
pair('A2-C1-parent', {'docs/reports/ja1-s4/c1-identities.json': lambda text: replace_once(text, '"base": "4250d146eb767b64a058bcbafb5f9adbccbb5264"', '"base": "7fa471fcd2ee0455142f7ee41893ec4ccaf646df"')})
pair('M7-provenance', {f'crates/kinewright-agent/src/orchestration/{filename}': lambda text: '\n'.join(line for line in text.split('\n') if 'Original Kinewright' not in line) for filename in ['policy.rs', 'server_tests.rs']})
pair('T1-request-owner', {'docs/reports/ja1-s4/c4-pre-h3.md': lambda text: replace_once(text, "included at the lead's request", "included at Riel's explicit request")})
print(f'FU1 protected runtime pairs: {sum("command" in row and "c1-cargo.sh" in " ".join(row["command"]) for row in RESULTS)}; audit pairs: 3', flush=True)
