#!/usr/bin/env python3
"""B2-0 host-picked task bases and pre-charge host failure pairs."""
from pathlib import Path
import argparse
import hashlib
import importlib.util
import json
import os
import signal
import sys

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent

def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'scripts' / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

h = load('b20_harness', 'te13-merge-mutations.py')
h.OUT = Path(os.environ.get('KINEWRIGHT_TE15_B20_OUT', ROOT / 'target/te1-5/b2-0/pairs'))
A = 'crates/kinewright-agent/src/'
W = A + 'writer_host/'
O = W + 'orchestration.rs'
P = W + 'task_preparation.rs'
E = A + 'orchestration/effect_pump.rs'
H = 'writer_host::hooks_tests::tests::'
PAIRS = []

def add(name, file, before, after, test, marker):
    PAIRS.append((name, [(file, before, after)], 'kinewright-agent', H + 'te15_b20_' + test, marker))

for name, test in [('motion', 'editor_then_motion'), ('editor', 'editor_then_editor'), ('assigned-early', 'assigned_before_merge_prepared_after')]:
    add('late-base-' + name, O, 'task_record.brief.scope.base = base;', 'let _ = base;', test, 'C4_MCP_ATTEMPT_HISTORY')
add('record-before-recovery', O, 'if saved.is_none()\n            && !self', 'if true\n            && !self', 'recorded_base_and_assignment_replay_survive_reopen', 'C4_MCP_ATTEMPT_HISTORY')
add('foreign-base', O, '    child.base == parent.base', '    true', 'foreign_and_widened_assignments_refuse', 'B20_AUTHORITY_REFUSAL')
add('object-limits', O, '        && child.objects.is_subset(&parent.objects)', '', 'foreign_and_widened_assignments_refuse', 'B20_AUTHORITY_REFUSAL')
add('run-owned-descent', P, 'if bases.last() != Some(&current) {', 'if false && bases.last() != Some(&current) {', 'unowned_checkpoint_refuses_before_charge', 'B20_NO_PROVIDER_CONSTRUCTION')
add('host-not-provider', E,
    'e.downcast_ref::<crate::writer_host::task_preparation::PreparationRefusal>()',
    'e.downcast_ref::<crate::writer_host::task_preparation::PreparationRefusal>().filter(|_| false)',
    'host_failure_notifies_director_without_retrying_child', 'B20_HOST_REPLAN_WAIT')
add('no-child-retry', P, '            effect.state = EffectState::Disposed;', '            effect.state = EffectState::Pending;', 'host_failure_notifies_director_without_retrying_child', 'B20_NO_CHILD_RETRY')
add('director-notified', P, '                state: EffectState::Pending,', '                state: EffectState::Disposed,', 'host_failure_notifies_director_without_retrying_child', 'B20_HOST_REPLAN_WAIT')
add('history-words', P, 'The task could not start because its candidate changed. This start sent nothing. The Director must replan the task; Continue cannot repair it.', 'The provider check failed before anything was sent. Continue to try again.', 'history_refusal_has_host_words', 'B20_HISTORY_WORDS')
# An unsafe retarget needs to bypass both stale-base and immutable merge-envelope
# checks. Mutate the real guards together; neither production guard is changed.
D = 'crates/kinewright-project/src/editing_session/durable.rs'
PAIRS.append(('same-checkpoint-conflict', [
    (D, 'if self.candidate_binding(&binding.branch)? != *binding {', 'if false && self.candidate_binding(&binding.branch)? != *binding {'),
    (D, 'expected_head: Some(frozen.base.head.clone()),', 'expected_head: Some(bundle.checkpoint(&frozen.base.branch)?.head_id.clone()),'),
    (A+'orchestration/tasks.rs', '            || envelope.expected_head.as_ref() != Some(&frozen.base().head)', '')
], 'kinewright-agent', H+'te15_b20_two_prepared_children_keep_the_existing_conflict', 'B20_CONFLICT_PATH'))

add('person-pause', P,
    'if matches!(state.phase, RunPhase::Queued | RunPhase::Running) {',
    'if true {', 'host_refusal_preserves_person_pause', 'B20_PERSON_PAUSE_RETAINED')
add('resume-notification', A+'orchestration/machine.rs',
    'RunWait::None | RunWait::ProviderSettings(_) | RunWait::Replan(_)',
    'RunWait::None | RunWait::ProviderSettings(_)',
    'host_refusal_preserves_person_pause', 'B20_RESUME_DIRECTOR_NOTIFICATION')

for filename, attr, names in [
    ('te13-mutations.py', 'harness', {'production-merge-recovery', 'lost-publication', 'lost-delivery'}),
    ('te13-fix1-mutations.py', 'harness', {'merge-cache-clears'}),
    ('te13-fix4-mutations.py', 'harness', {'only-pending-run-stop-abandons'}),
    ('te15-b1-fix4-mutations.py', 'h', {'resend-opening', 'resend-child_shared', 'resend-child_own', 'resend-wake', 'unknown-suffix'}),
    ('te15-b1-followup-mutations.py', 'h', {'unknown-readiness'}),
    ('te15-b1-fix2-mutations.py', 'h', {'unknown-continue'}),
]:
    registry = load('b20_' + filename.replace('-', '_'), filename)
    rows = [row for row in getattr(registry, attr).PAIRS if row[0] in names]
    assert len(rows) == len(names)
    for name, file, before, after, package, witness, marker in rows:
        PAIRS.append(('near-' + name, [(file, before, after)], package, witness, marker))
h.FAST_WITNESSES = {row[3] for row in PAIRS}

def main():
    os.nice(19 - os.getpriority(os.PRIO_PROCESS, 0))
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--only', action='append', default=[])
    args = parser.parse_args()
    selected = [row for row in PAIRS if not args.only or row[0] in args.only]
    assert selected and set(args.only) <= {row[0] for row in PAIRS}
    h.OUT.mkdir(parents=True, exist_ok=True)
    for key, directory in [('TMPDIR', 't'), ('LOCALAPPDATA', 'l')]:
        path = ROOT / 'target' / directory
        path.mkdir(exist_ok=True)
        os.environ[key] = str(path)
    os.environ['CARGO_TARGET_DIR'] = str(ROOT / 'target')
    os.environ['RUST_TEST_THREADS'] = '4'
    for number in (signal.SIGINT, signal.SIGTERM):
        signal.signal(number, h.interrupted)
    proofs = []
    for name, edits, package, witness, marker in selected:
        originals = {file: (ROOT/file).read_bytes() for file, _, _ in edits}
        changed = dict(originals)
        for file, before, after in edits:
            assert changed[file].count(before.encode()) == 1, name + ': anchor drifted'
            changed[file] = changed[file].replace(before.encode(), after.encode())
        row = {'name': name, 'witness': witness, 'sources': []}
        for file, original in originals.items():
            (h.OUT/(name+'-'+Path(file).name+'.original')).write_bytes(original)
            row['sources'].append({'path': file, 'original_sha256': hashlib.sha256(original).hexdigest(), 'mutant_sha256': hashlib.sha256(changed[file]).hexdigest()})
        try:
            for file, mutant in changed.items():
                (ROOT/file).write_bytes(mutant)
            row['red'] = h.run(name+'-red', package, witness, marker, True)
        finally:
            for file, original in originals.items():
                (ROOT/file).write_bytes(original)
                assert (ROOT/file).read_bytes() == original
        row['green'] = h.run(name+'-green', package, witness, marker, False)
        for source in row['sources']:
            source['restored_sha256'] = hashlib.sha256((ROOT/source['path']).read_bytes()).hexdigest()
            assert source['restored_sha256'] == source['original_sha256']
        (h.OUT/(name+'.json')).write_text(json.dumps(row, indent=2)+'\n')
        proofs.append(row)
        (h.OUT/'pairs.json').write_text(json.dumps(proofs, indent=2)+'\n')
    print(f'{len(proofs)} pairs completed with exact byte restores', flush=True)

if __name__ == '__main__':
    main()
