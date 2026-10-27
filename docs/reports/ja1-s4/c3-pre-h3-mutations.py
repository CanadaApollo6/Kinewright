"""Run only security/data-loss witnesses against one removed guard at a time."""
from pathlib import Path
import gzip
import hashlib
import json
import subprocess
import sys

root = Path(__file__).resolve().parents[3]
report = root / 'docs/reports/ja1-s4'
server = 'crates/kinewright-agent/src/server.rs'
tasks = 'crates/kinewright-agent/src/orchestration/tasks.rs'
policy = 'crates/kinewright-agent/src/orchestration/policy.rs'
mailbox = 'crates/kinewright-agent/src/orchestration/mailbox.rs'
machine = 'crates/kinewright-agent/src/orchestration/machine.rs'
role = 'server::tests::s4_role_scope_closure'
drain = 'server::tests::s4_drain_and_durable_base'
cases = [
    ('prepare_scope', server, 'endpoint.validate_batch(document, &operations)?;', 'let _ = (endpoint, document, &operations);', role),
    ('commit_scope', server, '&& let Err(error) = endpoint.validate_batch(&before, operations)', '&& let Err(error) = Ok::<(), String>(())', role),
    ('capability_scope', server, '&& let Err(error) = endpoint.admit_capability(request.name.as_ref())', '&& let Err(error) = Ok::<(), String>(())', role),
    ('planner_scope', server, '&& let Err(error) = endpoint.validate_batch(&document, &plan.operations)', '&& let Err(error) = Ok::<(), String>(())', role),
    ('endpoint_identity', server, 'if !endpoint.matches_core(&self.core) {', 'if false {', role),
    ('live_authority', tasks, 'if self.0.revoked.load(Ordering::SeqCst) || Instant::now() >= self.0.authority.expires_at {', 'if false {', role),
    ('freeze_ingress', tasks, 'self.0.closed.store(true, Ordering::SeqCst);', 'self.0.closed.store(false, Ordering::SeqCst);', drain),
    ('merge_scope', tasks, '.validate_batch(&document, frozen.delta())', '.check_live()', drain),
    ('audio_role', policy, 'Role::Audio => matches!(domain, Domain::Audio),', 'Role::Audio => true,', role),
    ('owner_project', tasks, 'if authority.project != owner.active_manual_project()?', 'if false', drain),
    ('owner_epoch', tasks, '|| authority.owner_epoch != owner.store()?.owner_epoch()', '|| false', drain),
    ('owner_expiry', tasks, '|| Instant::now() >= authority.expires_at', '|| false', drain),
    ('frozen_result_identity', mailbox, 'if result.sha256 != kinewright_media::sha256_bytes(&bytes)', 'if false', 's4_task_assignment_and_frozen_result_delivery_share_a_cut'),
    ('result_publication_freeze', machine, 'if self.uncertain || self.pending.is_some() {', 'if false {', 's4_unknown_publication_freezes_original_envelope_contract'),
    ('solo_collateral', policy, '&& before.track_audible(track.id) != after.track_audible(track.id)', '&& false', 'server::tests::s4_solo_removal_and_director_routing'),
    ('director_routing', policy, 'self.routing_member(&key)?;', 'if self.role == Role::Audio || !buses.is_empty() { self.routing_member(&key)?; }', 'server::tests::s4_solo_removal_and_director_routing'),
    ('editor_direct_effect_authority', policy, 'Role::Editor => matches!(domain, Domain::Editorial),', 'Role::Editor => true,', 'orchestration::policy::tests::editor_structural_edits_preserve_effects_without_direct_effect_authority'),
    ('routing_closure', policy, 'if !self.scope.bus_sidechain_closure.contains(key) {', 'if false {', 'orchestration::policy::tests::shared_bus_and_sidechain_closure_is_transitive_and_checks_old_routing'),
]
selected = set(sys.argv[1:])
previous = json.loads((report / 'c3-mutations.json').read_text()) if selected else []
results = previous
for name, relative, old, new, witness in cases:
    if selected and name not in selected:
        continue
    path = root / relative
    original = path.read_bytes()
    assert original.decode().count(old) == 1, name
    target = ['--test', 'ja_s4_kernel'] if name in {'frozen_result_identity', 'result_publication_freeze'} else ['--lib']
    command = ['bash', 'docs/reports/ja1-s4/c1-cargo.sh', 'test', '-p', 'kinewright-agent', *target, witness, '--', '--exact', '--test-threads=1']
    try:
        path.write_text(original.decode().replace(old, new))
        red = subprocess.run(command, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        with gzip.open(report / f'c3-mutation-{name}-red.log.gz', 'wb') as output:
            output.write(red.stdout)
        assert red.returncode != 0 and b'test result: FAILED. 0 passed; 1 failed' in red.stdout, red.stdout[-5000:].decode()
    finally:
        path.write_bytes(original)
    green = subprocess.run(command, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    with gzip.open(report / f'c3-mutation-{name}-green.log.gz', 'wb') as output:
        output.write(green.stdout)
    assert green.returncode == 0 and b'test result: ok. 1 passed' in green.stdout, green.stdout[-5000:].decode()
    results = [row for row in results if row['name'] != name]
    results.append({'name':name, 'source':relative, 'witness':witness, 'restored_sha256':hashlib.sha256(path.read_bytes()).hexdigest(), 'red':red.returncode, 'green':green.returncode})
    (report / 'c3-mutations.json').write_text(json.dumps(results, indent=2) + '\n')
    print(name, 'runtime red / restored green', flush=True)
(report / 'c3-mutations.json').write_text(json.dumps(results, indent=2) + '\n')
print('C3 protected mutation pairs:', len(results))
