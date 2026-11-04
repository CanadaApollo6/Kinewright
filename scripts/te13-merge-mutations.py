#!/usr/bin/env python3
"""C6 merge regressions: named RED assertions and byte-exact restored GREENs."""
from pathlib import Path
import argparse
import hashlib
import json
import os
import re
import shlex
import signal
import subprocess

ROOT = Path(__file__).resolve().parent.parent
OUT = Path(os.environ.get('KINEWRIGHT_TE13_MERGE_MUTATION_OUT', ROOT / 'target/te1-3/merge/c6-pairs'))
AGENT = 'kinewright-agent'
HOOK = 'writer_host::hooks_tests::tests::'
PAIRS = [
    ('probe-ignores-boot', 'crates/kinewright-agent/src/orchestration/version_probe.rs',
     '                Some(boot) => retire_prior_boot(claim, &boot)?,',
     '                Some(_) => settle(claim)?,', AGENT,
     'version_probe::tests::s4b_c6_version_probe_prior_boot_leftovers_admit', 'C6_VERSION_PROBE_PRIOR_BOOT_ADMITTED'),
    ('claim-drop-closes-only', 'crates/kinewright-agent/src/orchestration/execution_claim.rs',
     '    fn drop(&mut self) {\n        let _ = self.lock.unlock();', '    fn drop(&mut self) {', AGENT,
     'execution_claim::tests::s4b_c6_dropped_claim_releases_past_a_descriptor_copy', 'C6_CLAIM_HELD_BY_COPY'),
    ('feedback-lock-closes-only', 'crates/kinewright-project/src/recovery.rs',
     '        let _ = fs2::FileExt::unlock(&self.file);', '        let _ = &self.file;', 'kinewright-project',
     'recovery::tests::s4b_c6_released_feedback_lock_passes_a_descriptor_copy', 'C6_FEEDBACK_LOCK_HELD_BY_COPY'),
    ('walk-refuses-removed-cgroup', 'crates/kinewright-job-sys/src/linux.rs',
     '        let entries = match fs::read_dir(&path) {\n            Ok(entries) => entries,\n            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,\n            Err(e) => return Err(e),\n        };',
     '        let entries = fs::read_dir(&path)?;', 'kinewright-job-sys',
     'linux::tests::s4b_c6_unit_walk_passes_a_cgroup_removed_mid_walk', 'C6_WALK_REFUSED_A_REMOVED_CGROUP'),
    ('journal-closes-only', 'crates/kinewright-app/src/recovery.rs',
     '        #[cfg(unix)]\n        let _ = self.0.unlock();', '', 'kinewright-app',
     'recovery::tests::s4b_c6_ended_journal_unlocks_past_a_descriptor_copy', 'C6_JOURNAL_HELD_BY_COPY'),
    ('claude-adapter', 'crates/kinewright-agent/src/writer_host/native_invocation.rs',
     '== native_launcher::StreamDialect::Claude', '== native_launcher::StreamDialect::Codex', AGENT,
     HOOK + 's4b_c6_merged_claude_snapshot_replay_is_counted_once', 'C6_READER_REFUSAL snapshot-replay'),
    ('claude-uuid', 'crates/kinewright-agent/src/writer_host/native_spending.rs',
     'if kind == "result" { "uuid" } else { "id" }', 'if kind == "result" { "id" } else { "id" }', AGENT,
     HOOK + 's4b_c6_merged_claude_snapshot_replay_is_counted_once', 'C6_MODEL_USAGE_EXACT_REPLAY snapshot-replay'),
    ('claude-role-surface', 'crates/kinewright-agent/src/server.rs',
     'Ok(role) => task_role_tools(role)?,',
     'Ok(_) => task_role_tools(crate::orchestration::orchestration_records::Role::Director)?,', AGENT,
     HOOK + 's4b_c6_merged_claude_mcp_surface_is_the_durable_role', 'C6_DURABLE_ROLE_SURFACE'),
]


def run(name, package, witness, marker, red):
    target = ['--bin', 'kinewright-app'] if package == 'kinewright-app' else ['--lib']
    features = {AGENT: 'eval-harness,slow-tests', 'kinewright-project': 'test-util', 'kinewright-app': 'slow-tests'}.get(package)
    args = ['test', '-p', package, *target]
    if features:
        args += ['--features', features]
    args += [witness, '--', '--exact', '--nocapture']
    command = 'source scripts/setup-ffmpeg.sh && flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo ' + shlex.join(args)
    with (OUT / f'{name}.log').open('w') as log:
        result = subprocess.run(['bash', '-c', command], cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, check=False)
    text = (OUT / f'{name}.log').read_text(errors='replace')
    summary = re.search(r'^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed;.*$', text, re.M)
    assert summary, name + ': no test summary'
    assert summary.groups() == (('FAILED', '0', '1') if red else ('ok', '1', '0')), name
    assert (result.returncode != 0) == red, name
    if red:
        assert marker in text, name + ': named assertion missing'
    print(name, summary[0], flush=True)
    return {'exit': result.returncode, 'summary': summary[0], 'marker': marker if red else None}


def interrupted(number, _frame):
    raise RuntimeError(f'interrupted by signal {number}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--only', action='append', default=[])
    args = parser.parse_args()
    selected = [pair for pair in PAIRS if not args.only or pair[0] in args.only]
    assert selected and set(args.only) <= {pair[0] for pair in PAIRS}
    OUT.mkdir(parents=True, exist_ok=True)
    for key, directory in [('TMPDIR', 't'), ('LOCALAPPDATA', 'l')]:
        path = ROOT / 'target' / directory
        path.mkdir(exist_ok=True)
        os.environ[key] = str(path)
    os.environ['CARGO_TARGET_DIR'] = str(ROOT / 'target')
    os.environ['RUST_TEST_THREADS'] = '4'
    for number in (signal.SIGINT, signal.SIGTERM):
        signal.signal(number, interrupted)
    proofs = []
    for name, relative, before, after, package, witness, marker in selected:
        path = ROOT / relative
        original = path.read_bytes()
        before, after = before.encode(), after.encode()
        assert original.count(before) == 1, name + ': mutation anchor drifted'
        (OUT / f'{name}.original').write_bytes(original)
        mutant = original.replace(before, after)
        row = {'name': name, 'source': relative, 'witness': witness,
               'original_sha256': hashlib.sha256(original).hexdigest(), 'mutant_sha256': hashlib.sha256(mutant).hexdigest()}
        try:
            path.write_bytes(mutant)
            row['red'] = run(name + '-red', package, witness, marker, True)
        finally:
            path.write_bytes(original)
            assert path.read_bytes() == original, name
            row['restored_sha256'] = hashlib.sha256(original).hexdigest()
        row['green'] = run(name + '-green', package, witness, marker, False)
        assert path.read_bytes() == original, name
        (OUT / f'{name}.json').write_text(json.dumps(row, indent=2) + '\n')
        proofs.append(row)
        (OUT / 'pairs.json').write_text(json.dumps(proofs, indent=2) + '\n')
    print(f'{len(proofs)} pairs completed with exact byte restores', flush=True)


if __name__ == '__main__':
    main()
