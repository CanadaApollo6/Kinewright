"""Targeted runtime failures and restored passes for H2's data-loss boundaries."""
from pathlib import Path
import gzip
import hashlib
import json
import subprocess

root = Path(__file__).resolve().parents[3]
report = root / 'docs/reports/ja1-s4'
durable = 'crates/kinewright-project/src/editing_session/durable.rs'
publisher = 'crates/kinewright-project/src/bundle_commit.rs'
cases = [
    ('candidate_base', durable, 'if self.candidate_binding(&binding.branch)? != *binding {', 'if false {'),
    ('frozen_payload', durable, '|| *envelope != self.candidate_merge_envelope(frozen, envelope.id.clone())?', '|| false'),
    ('merge_replay', durable, 'if previous != crate::receipts::ReceiptLookup::Absent {', 'if false {'),
    ('candidate_install', durable, '.install_frozen_checkpoint(&cut, (*after).clone())', '.install_frozen_checkpoint(&cut, cut.history().clone())'),
    ('continuous_freeze', durable, 'self.persist_durable_branch_inner(guard, false)?', 'self.persist_durable_branch_inner(guard, true)?'),
    ('candidate_batch', publisher, '.or_else(|| branches[branch_id].batch_ids.last().cloned()),', '.or_else(|| branches[&manual_branch_id].batch_ids.last().cloned()),'),
]
results = []
command = ['bash', 'docs/reports/ja1-s4/c1-cargo.sh', 'test', '-p', 'kinewright-project', '--features', 'test-util', '--test', 'ja_s4_candidate', 'h2_candidate_commit_preserves_pending_manual_and_replays_after_reopen', '--', '--exact', '--test-threads=1']
for name, relative, old, new in cases:
    path = root / relative
    original = path.read_bytes()
    assert original.decode().count(old) == 1, name
    try:
        path.write_text(original.decode().replace(old, new))
        red = subprocess.run(command, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        with gzip.open(report / f'h2-mutation-{name}-red.log.gz', 'wb') as stream:
            stream.write(red.stdout)
        assert red.returncode != 0 and b'test result: FAILED. 0 passed; 1 failed' in red.stdout, red.stdout[-4000:].decode()
    finally:
        path.write_bytes(original)
    green = subprocess.run(command, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    with gzip.open(report / f'h2-mutation-{name}-green.log.gz', 'wb') as stream:
        stream.write(green.stdout)
    assert green.returncode == 0 and b'test result: ok. 1 passed' in green.stdout, green.stdout[-4000:].decode()
    results.append({'name': name, 'source': relative, 'restored_sha256': hashlib.sha256(path.read_bytes()).hexdigest(), 'red': red.returncode, 'green': green.returncode})
    print(name, 'runtime red / restored green', flush=True)
(report / 'h2-mutations.json').write_text(json.dumps(results, indent=2) + '\n')
print('H2 protected mutation pairs:', len(results))
