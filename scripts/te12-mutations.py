#!/usr/bin/env python3
"""TE1.2 security witnesses: named RED assertions and byte-exact restored GREENs."""
from pathlib import Path
import argparse
import hashlib
import json
import os
import re
import signal
import subprocess

ROOT = Path(__file__).resolve().parent.parent
OUT = Path(os.environ.get('KINEWRIGHT_TE12_MUTATION_OUT', ROOT / 'target/te1-2/mutations'))
AGENT = 'kinewright-agent'
PROJECT = 'kinewright-project'
HOOK = 'writer_host::hooks_tests::tests::'
MCP = 'crates/kinewright-agent/src/writer_host/task_mcp.rs'
REPLY = 'crates/kinewright-agent/src/writer_host/task_mcp/reply.rs'
SERVER = 'crates/kinewright-agent/src/server.rs'
MANIFEST = 'crates/kinewright-agent/src/writer_host/task_mcp/manifest.rs'
RECONCILE = 'crates/kinewright-agent/src/writer_host/receipt_reconciliation.rs'
ADMISSION = 'crates/kinewright-project/src/run_admission.rs'
ORCHESTRATION = 'crates/kinewright-agent/src/writer_host/orchestration.rs'
CONTROL = '        let control: Control = serde_json::from_value(arguments)?;'
MUTATIONS = []


def add(name, source, before, after, witness, marker, package=AGENT):
    if package == AGENT:
        witness = HOOK + witness
    MUTATIONS.append((name, source, before, after, witness, marker, package))


add('fresh-control', MCP, CONTROL, CONTROL + '\n        if control.expected_generation > 0 { return Err(refused("fresh refused")); }',
    'te12_fresh_controls_and_stale_epoch_generation_head', 'TE12_FRESH_CONTROL')
for field, expression, marker in [
    ('attempt', 'self.endpoint.authority().attempt.clone()', 'TE12_STALE_ATTEMPT'),
    ('owner_epoch', 'self.endpoint.authority().owner_epoch', 'TE12_STALE_EPOCH'),
    ('expected_generation', 'host.owner()?.metadata_generation()?', 'TE12_STALE_GENERATION'),
    ('expected_head', 'self.endpoint.head().clone()', 'TE12_STALE_HEAD'),
]:
    add('stale-' + field, MCP, CONTROL,
        CONTROL.replace('let control:', 'let mut control:') + f'\n        control.{field} = {expression};',
        'te12_fresh_controls_and_stale_epoch_generation_head', marker)
add('list-allowed', SERVER, 'Ok(role) => task_role_tools(role),',
    'Ok(_) => task_role_tools(crate::orchestration::orchestration_records::Role::Reviewer),',
    'te12_list_allowed', 'TE12_LIST_ALLOWED')
add('list-hidden', MANIFEST, '    "assign_task",', '    "prepare_edit_plan",\n    "assign_task",',
    'te12_list_hidden', 'TE12_LIST_HIDDEN')
add('list-forged-role', REPLY, '        Ok(state\n', '        return Ok(Role::Director);\n        Ok(state\n',
    'te12_list_forged_role', 'TE12_LIST_FORGED_ROLE')
ROLE_CHECK = '''            if request
                .arguments
                .as_ref()
                .is_some_and(|arguments| arguments.contains_key("role"))'''
add('dispatch-allowed', SERVER, ROLE_CHECK,
    '''            if request.name == "get_timeline_state" { return Ok(error_text("allowed refused")); }
''' + ROLE_CHECK, 'te12_dispatch_allowed', 'TE12_DISPATCH_ALLOWED')
add('dispatch-hidden', SERVER,
    'if !crate::writer_host::task_mcp::manifest::names(role).contains(&request.name.as_ref())',
    'if false && !crate::writer_host::task_mcp::manifest::names(role).contains(&request.name.as_ref())',
    'te12_dispatch_hidden', 'TE12_DISPATCH_HIDDEN')
add('dispatch-forged-role', SERVER, ROLE_CHECK,
    '''            if request.arguments.as_ref().is_some_and(|arguments| arguments.contains_key("role")) {
                return Ok(match control.call(&request.name,serde_json::json!({})) {
                    Ok(value) => success_structured("ignored role",value), Err(error) => error_text(error.to_string()),
                });
            }
''' + ROLE_CHECK, 'te12_dispatch_forged_role', 'TE12_DISPATCH_FORGED_ROLE')
add('cross-task-receipt', RECONCILE, 'if !state.attempts.iter().any(original_attempt)',
    'if false && !state.attempts.iter().any(original_attempt)',
    'te12_receipt_reconciliation_is_read_only_and_scope_bound', 'TE12_CROSS_TASK_RECEIPT')
add('cross-run-receipt', 'crates/kinewright-project/src/orchestration_store.rs',
    'if original_run != run {',
    'if false && original_run != run {',
    'te12_receipt_reconciliation_is_read_only_and_scope_bound', 'TE12_STORE_CROSS_RUN')
DISPATCH = '''        if let Some(control) = &self.task_control {
            let role = match control.role()'''
add('old-dispatcher', SERVER, DISPATCH,
    '''        if request.name == "invoke_capability" && let Some(control) = &self.task_control {
            let args: InvokeCapabilityArgs = decode_args("invoke_capability",request.arguments.clone().unwrap_or_default())?;
            return Ok(match control.call(&args.name,args.arguments) {
                Ok(value) => success_structured("legacy control",value), Err(error) => error_text(error.to_string()),
            });
        }
''' + DISPATCH, 'te12_old_dispatcher_is_refused', 'TE12_OLD_DISPATCHER')
add('committed-refresh-failure', REPLY, '        reply\n    }',
    '        if reply.get("controls").is_none() { return json!({"outcome":{"error":"refresh failed"}}); }\n        reply\n    }',
    'te12_terminal_reply_keeps_receipt_and_state_without_controls', 'TE12_TERMINAL_RECEIPT')
add('terminal-controls', REPLY, '        reply\n    }',
    '        if reply.get("controls").is_none() { reply["controls"] = json!({}); }\n        reply\n    }',
    'te12_terminal_reply_keeps_receipt_and_state_without_controls', 'TE12_TERMINAL_NO_CONTROLS')
add('freeze-outcome-rewrite', RECONCILE,
    '''                return Ok(
                    serde_json::json!({"receipt":receipt,"history":history,"result":result}),
                );''',
    '''                let mut result = result.clone();
                let current = self.owner()?.orchestration_projection(&state.run)?.unwrap();
                result.usage_ref = self.current_accounting_reference(&current)?;
                return Ok(serde_json::json!({"receipt":receipt,"history":history,"result":result}));''',
    'te12_freeze_reconciliation_is_exact_and_does_not_restore_actor', 'TE12_FREEZE_IMMUTABLE_OUTCOME')
add('fix2-public-guard', ADMISSION, '    pub(crate) fn admit_run_write(&self)',
    '    pub fn terminal_admission(&mut self) { self.set_run_budget(false); }\n    pub(crate) fn admit_run_write(&self)',
    's4b_c5_fix2_public_writes_cannot_spend_terminal_reserve', 'C5_FIX2_PUBLIC_GUARD_ACCEPTED_WRITE')
add('fix2-read-authority', ADMISSION,
    '''        self.publication
            .as_ref()
            .is_some_and(|token| token.matches_store(self))''',
    '''        READ_HEADROOM.with_borrow(|entries| entries.contains_key(&self.admission_key())) || self.publication
            .as_ref()
            .is_some_and(|token| token.matches_store(self))''',
    'run_admission::tests::s4b_c5_fix1_record_limits_keep_terminal_headroom',
    'C5_FIX2_READING_IS_NOT_WRITE_AUTHORITY', PROJECT)
add('fix10-host-usage', ORCHESTRATION,
    '            result.usage_ref = self.current_accounting_reference(&state)?;',
    '            let _ = self.current_accounting_reference(&state)?;',
    's4b_c5_fix10_clock_tick_between_freeze_and_publish_is_accepted', 'C5_FIX10_')
add('fix13-settled-archive', ADMISSION, 'if budget.active || (finished && archival()?)', 'if budget.active',
    's4b_c5_fix13_settled_run_below_its_limit_archives', 'C5_FIX13_SETTLED_ARCHIVE_REFUSED')
add('fix14-archival-only', ADMISSION, 'if budget.active || (finished && archival()?)', 'if budget.active || finished',
    's4b_c5_fix14_finished_phase_cycle_is_refused_and_archive_still_runs', 'C5_FIX14_PHASE_CYCLE_')

add('evidence-not-receipt', SERVER,
    'let has_receipt = crate::writer_host::task_mcp::is_capability(&request.name);',
    'let has_receipt = true;',
    'te12_evidence_cannot_supply_a_receipt', 'TE12_EVIDENCE_NOT_RECEIPT')
add('grant-publication', ORCHESTRATION,
    'let published = owner.issue_metadata_authority(&subject, issuer, &grant);',
    'let published = match issuer { Some(issuer) => Ok(issuer.clone()), None => owner.reuse_metadata_issuer_for(&subject) }.and_then(|issuer| Ok((issuer, owner.admit_host_grant(&grant)?)));',
    's4b_c4_fix1_foreign_missing_and_live_original_retry', 'C4_FOREIGN_COPY')

add('terminal-state-retained', MCP, '        self.remember_terminal_state(&host);',
    '        let _ = &host;',
    'te12_terminal_reply_keeps_receipt_and_state_without_controls', 'TE12_TERMINAL_STATE')

add('lost-reply-client-digest', 'crates/kinewright-project/src/orchestration_records.rs',
    'if matches!(command, OrchestrationCommand::PublishResult(_)) {',
    'if false && matches!(command, OrchestrationCommand::PublishResult(_)) {',
    'te12_lost_reply_uses_only_the_original_sent_command', 'TE12_LOST_REPLY_FROM_SENT_COMMAND')
add('publication-payload-replay', ORCHESTRATION, '            if *stored != original {',
    '            if false && *stored != original {',
    'te12_changed_publication_metadata_is_not_an_exact_replay', 'TE12_CHANGED_PUBLICATION_METADATA')


add('recovery-controls', MCP, '        if self.native_recovery_pending || self.recovery_only {',
    '        if false && (self.native_recovery_pending || self.recovery_only) {',
    'te12_recovery_mode_does_not_issue_controls', 'TE12_RECOVERY_NO_CONTROLS')

add('legacy-frozen-preservation', 'crates/kinewright-agent/src/writer_host/task_history.rs',
    '.filter(|history| history.frozen)',
    '.filter(|history| history.frozen && history.frozen_result.is_some())',
    'te12_legacy_frozen_history_is_not_upgraded', 'TE12_LEGACY_FROZEN_NOT_UPGRADED')
add('legacy-merge-origin', RECONCILE, '            if history.merge_envelope.is_some() {',
    '            if false && history.merge_envelope.is_some() {',
    'te12_legacy_merge_cannot_gain_current_provenance', 'TE12_LEGACY_ORIGIN_NOT_MINTED')


def interrupted(number, _frame):
    raise RuntimeError(f'interrupted by signal {number}')


def run(name, package, witness, marker, red):
    features = 'eval-harness,slow-tests' if package == AGENT else 'test-util'
    args = ['bash', 'target/te1/cargo.sh', 'test', '-p', package, '--lib', '--features', features,
            witness, '--', '--exact', '--nocapture']
    with (OUT / f'{name}.log').open('w') as output:
        result = subprocess.run(args, cwd=ROOT, stdout=output, stderr=subprocess.STDOUT, check=False)
    log = (OUT / f'{name}.log').read_text()
    summary = re.search(r'^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed;.*$', log, re.M)
    assert summary, f'{name}: no test summary (compile/infrastructure failure)'
    expected = ('FAILED', '0', '1') if red else ('ok', '1', '0')
    assert summary.groups() == expected, f'{name}: must execute exactly its witness; {summary[0]}'
    assert (result.returncode != 0) == red, name + ': incorrect exit status'
    if red:
        assert marker in log, name + ': named assertion was not reached'
    print(name, summary[0], flush=True)
    return {'exit': result.returncode, 'summary': summary[0], 'named_assertion': marker if red else None}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--only', action='append', default=[])
    args = parser.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)
    for key, name in [('TMPDIR', 't'), ('LOCALAPPDATA', 'l')]:
        path = ROOT / 'target' / name
        path.mkdir(exist_ok=True)
        os.environ[key] = str(path)
    os.environ['CARGO_TARGET_DIR'] = str(ROOT / 'target')
    os.environ['RUST_TEST_THREADS'] = '4'
    for number in (signal.SIGINT, signal.SIGTERM):
        signal.signal(number, interrupted)
    selected = [m for m in MUTATIONS if not args.only or m[0] in args.only]
    assert selected and set(args.only) <= {m[0] for m in MUTATIONS}, 'unknown mutation'
    proofs = {}
    for name, relative, before, after, witness, marker, package in selected:
        source = ROOT / relative
        original = source.read_bytes()
        before, after = before.encode(), after.encode()
        assert original.count(before) == 1, name + ': mutation site drifted'
        (OUT / f'{name}.original').write_bytes(original)
        mutant = original.replace(before, after)
        row = {'name': name, 'source': relative, 'witness': witness,
               'original_sha256': hashlib.sha256(original).hexdigest(),
               'mutant_sha256': hashlib.sha256(mutant).hexdigest()}
        try:
            source.write_bytes(mutant)
            row['red'] = run(name + '-red', package, witness, marker, True)
        finally:
            source.write_bytes(original)
            assert source.read_bytes() == original, name + ': byte restore failed'
            row['restored_sha256'] = hashlib.sha256(source.read_bytes()).hexdigest()
            (OUT / f'{name}.json').write_text(json.dumps(row, indent=2) + '\n')
        row['green'] = run(name + '-green', package, witness, marker, False)
        assert source.read_bytes() == original, name + ': restored source changed during GREEN'
        (OUT / f'{name}.json').write_text(json.dumps(row, indent=2) + '\n')
        proofs[name] = row
        (OUT / 'pairs.json').write_text(json.dumps(list(proofs.values()), indent=2) + '\n')
    print(f'{len(proofs)} pairs completed with exact byte restores', flush=True)


if __name__ == '__main__':
    main()
