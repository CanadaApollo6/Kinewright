#!/usr/bin/env python3
"""Unpaid B2b grant, durable recovery and observation pairs; exact byte restores."""
import importlib.util
import os
import hashlib
import json
import re
import subprocess
from pathlib import Path
ROOT=Path(__file__).resolve().parent.parent
spec=importlib.util.spec_from_file_location('b2b_pairs',ROOT/'scripts/te15-b2a-mutations.py')
e=importlib.util.module_from_spec(spec);spec.loader.exec_module(e)
e.OUT=Path(os.environ.get('KINEWRIGHT_B2B_PAIRS',ROOT/'target/te1-5/b2b/pairs'))
A='crates/kinewright-agent/src/'
W=A+'writer_host/'
G=A+'te1_eval/grant.rs'
P='crates/kinewright-project/src/'
H='writer_host::hooks_tests::tests::'
C=H+'tests::consent_followups::tests::'
APP='studio_feedback::ja_s3_feedback::te15_b1_fix4::'
def add(name,file,before,after,witness,marker):
    e.PAIRS.append((name,file,before,after,witness,marker))
def host(name,file,before,after,test,marker):add(name,file,before,after,H+'te15_b2b_'+test,marker)
e.PAIRS=[]
# Trusting a caller's plan instead of reading lead approval is the missing-grant mutant.
add('grant-required',G,'    check_directory(folder, true)?;', '    return Ok(ConsumedGrant {grant:expected.clone(),authority:LaunchAuthority::Unpaid});\n    check_directory(folder, true)?;', 'te1_eval::grant::tests::te15_b2b_grant_boundary','B2B_MISSING_GRANT')
add('candidate-six-sample',A+'te1_eval/analysis.rs','rows.len() != 6','rows.len() != 12','te15_b2b_analysis_retains_six_candidate_rows','B2B_SIX_CANDIDATES_ONLY')
add('candidate-arm-only',G,'        || grant.arm != "candidate"\n','', 'te1_eval::grant::tests::te15_b2b_candidate_grant_only','B2B_ONLY_CANDIDATE_GRANT')
add('grant-exact-plan',G,'if &grant != expected {','if false {','te1_eval::grant::tests::te15_b2b_grant_boundary','B2B_CHANGED_GRANT')
reuse='OFlags::RDWR | OFlags::CREATE | OFlags::EXCL'
for name,test,marker in [('grant-one-use','boundary','B2B_REUSED_GRANT'),('grant-interruption','interrupted','B2B_INTERRUPTION_NO_RETRY')]:
 add(name,G,reuse,'OFlags::RDWR | OFlags::CREATE | OFlags::TRUNC','te1_eval::grant::tests::te15_b2b_grant_'+test,marker)
add('grant-durable-before-construction',G,[( 'durable(&receipt)?;','let _ = &receipt;'),('durable(&consumed)?;',''),('durable(folder)?;','')],None,'te1_eval::grant::tests::te15_b2b_grant_durability','B2B_SYNC_BEFORE_CONSTRUCTION')
text=(ROOT/G).read_text();start=text.index('    for name in [');end=text.index(' {',start)
add('grant-run-identity',G,text[start:end], '    for name in [format!("id-{id}.json")]', 'te1_eval::grant::tests::te15_b2b_grant_boundary','B2B_RUN_ID_REUSED')
start=text.index('        if value["model"]');end=text.index('        {',start)
add('grant-open-choices',G,text[start:end],'        if false\n','te1_eval::grant::tests::te15_b2b_grant_open_and_identity_fields','B2B_OPEN_CHOICES_REFUSED')
start=text.index('    if grant.dry_pass');end=text.index('    {',start)
add('grant-dry-required',G,text[start:end],'    if false\n','te1_eval::grant::tests::te15_b2b_grant_dry_pass','B2B_DRY_PASS_REQUIRED')
add('temporary-is-not-native',G,'Err(error(\n                "temporary grant receipts cannot authorise native execution",\n            ))','Ok(Path::new("/unpaid-mutant"))','te1_eval::grant::tests::te15_b2b_grant_boundary','B2B_TEMP_RECEIPT_IS_NOT_NATIVE_AUTHORITY')
add('fixed-home',G,'if provided.map(Path::new) != Some(home) {','if false {','te1_eval::grant::tests::te15_b2b_fixed_account_home','B2B_HOME_OVERRIDE_REFUSED')
for name in ['final','stream']:
 host('unknown-'+name,W+'studio_runtime.rs','if !crate::orchestration::machine::has_unknown_send(state) {','if true {','unknown_'+name,'B2B_UNKNOWN_PRIORITY')
text=(ROOT/(W+'task_preparation.rs')).read_text();start=text.index('            if state.phase != RunPhase::Paused');end=text.index('            {',start)
host('permanent-wait',W+'task_preparation.rs',text[start:end],'            if true\n','permanent_wait_precedence','B2B_PERMANENT_WAIT')
host('permanent-message',W+'orchestration.rs','!matches!(action, RunAction::Continue | RunAction::SendMessage { .. })','!matches!(action, RunAction::Continue)','permanent_wait_precedence','B2B_PERMANENT_MESSAGE')
# Retain an existing Replan instead of replacing it with a generic provider refusal.
host('replan-priority',W+'studio_runtime.rs','RunWait::Files(_) | RunWait::Replan(_) | RunWait::Provider(_)','RunWait::Files(_) | RunWait::Provider(_)','replan_reason','B2B_REPLAN_REASON')
text=(ROOT/(W+'orchestration.rs')).read_text();start=text.index('        let retained_fork =');end=text.index('        if saved.is_none()',start)
for name,witness,marker in [('fork-publication-reuse',H+'te15_b202_after_history','B2B_FORK_REUSED_AFTER_PUBLICATION_FAILURE'),('fork-crash-reuse',H+'te15_b2b_fork_crash','B2B_CRASH_FORK_REUSED')]:
 add(name,W+'orchestration.rs',text[start:end],'        let retained_fork = None;\n',witness,marker)
# These remove actual checks on saved data, never inject a synthetic refusal.
start=text.index('            if history.base != task_record.brief.scope.base');end=text.index('            {',start)
host('real-base',W+'orchestration.rs',text[start:end],'            if false\n','real_base','B2B_REAL_PREPARATION_KIND')
host('real-frozen',W+'orchestration.rs','if history.frozen {\n                        "task_preparation_frozen"','if false {\n                        "task_preparation_frozen"','real_frozen','B2B_REAL_PREPARATION_KIND')
host('real-missing',W+'orchestration.rs','                if !safe {','                if false {','real_missing','B2B_REAL_PREPARATION_KIND')
add('net-zero',W+'task_consent.rs','let description = actual_description.as_deref();','let description = _description;',C+'te15_b2b_net_zero_removal','B2B_NO_ZERO_APPROVAL')
add('liveness-code',A+'server.rs','format!("consent_liveness: {error}")','format!("consent_policy: {error}")',C+'te15_b2b_liveness_prepare_and_commit','B2B_LIVENESS_CODE')
add('application-retry',W+'context_publication.rs','self.append_pending_edit_applications(&mut projection)','Ok::<(),HostError>(())',C+'te15_b2b_applied_marker_retries','B2B_MARKER_NEXT_PUBLICATION')
add('applications-append-only',P+'orchestration_commit.rs','!after\n            .edit_applications\n            .starts_with(&before.edit_applications)','false',C+'te15_b2b_applications_are_append_only','B2B_APPLICATIONS_APPEND_ONLY')
text=(ROOT/(P+'bundle_recovery.rs')).read_text();start=text.index('                if let Some(previous) = previous_snapshot.as_ref()');end=text.index('                previous_snapshot =',start)
for kind in ['fixed_consent','append_only_approvals','append_only_applications']:
 add('open-'+kind,P+'bundle_recovery.rs',text[start:end],'', 'project::te15_b2b_open_'+kind,'B2B_OPEN_REFUSES_CHANGED_CONSENT_OR_AUDIT')
text=(ROOT/(A+'te1_eval/checks.rs')).read_text();start=text.index('            merge_ids\n');end=text.index('\n        })',start)
add('effective-merge',A+'te1_eval/checks.rs',text[start:end],text[start:end].replace('.then_some(position)','; Some(position)'),H+'te15_b2b_merge_replay_same_final_document','B2B_REAL_MERGE_READ_REPLAY_REFUSED')
add('finish-publication',A+'te1_eval/checks.rs','bad.tool == "finish_task"','bad.tool == "mutant_finish_task"','te1_eval::checks::tests::te15_b2b_finish_publication_order','B2B_FINISH_PUBLISHED_OBSERVED')
text=(ROOT/(W+'task_history.rs')).read_text();start=text.index('                if let Some(reference) = self');end=text.index('                {',start)
host('native-child-report',W+'task_history.rs',text[start:end],'                if let Some(reference) = None::<kinewright_project::bundle_records::ObjectRef>\n','native_publication_injection','B2B_NATIVE_REPORT_OWN_PUBLISH_PATH')
add('capacity-label','crates/kinewright-app/src/studio_run_view.rs','reason == kinewright_agent::job_guardian::protocol::INPUT_TOO_LONG','false',APP+'te15_b2b_feedback_capacity_label','B2B_CAPACITY_LABEL')
add('upgrade-words',W+'task_preparation_words.rs','Press Stop, then Reopen to keep and review your notes, and start a new run.','Reopen later.',APP+'te15_b2b_feedback_capacity_label','B2B_UPGRADE_STOP_WORDS')

text=(ROOT/(A+'te1_eval.rs')).read_text();start=text.index('    let result = result.and_then(');end=text.index('    let row = match &result',start)
body=text[start:end];inside=body[body.index('{')+1:body.rfind('    });')]
add('failed-accounting-output',A+'te1_eval.rs',body,'    let result = match result { Ok(value) => {'+inside+'    }, Err(error) => Err(error) };\n','te1_eval::output_tests::te15_b2b_bad_accounting_retains_failure_record','B2B_INVALID_ACCOUNTING_FAILURE_RETAINED')
add('output-no-overwrite',A+'te1_eval.rs','.create_new(true)','.create(true).truncate(true)','te1_eval::output_tests::te15_b2b_result_never_overwrites_evidence','B2B_OUTPUT_IMMUTABLE')
add('native-publication-pin',W+'task_history.rs',
    '            let frozen_result = frozen_result.map(|mut result| {',
    '            // source-drift mutant\n            let frozen_result = frozen_result.map(|mut result| {',
    'te15_b2a_measurement_pin_matches_every_observed_field',
    '/arm_bridge_sha256/native_child_publication')
add('retained-fork-helper',P+'editing_session/durable.rs',
    'if let Some(branch) = branch.as_ref()',
    'if let Some(branch) = None::<&BranchId>',
    H+'te15_b202_after_history',
    'B2B_FORK_REUSED_AFTER_PUBLICATION_FAILURE')

original_run=e.run
original_hashes={rel:hashlib.sha256((ROOT/rel).read_bytes()).hexdigest() for _,rel,*_ in e.PAIRS}
def run(label,witness,marker,red):
    cached=e.OUT/f'{label}.json'
    if red and os.environ.get('KINEWRIGHT_B2B_RESUME')=='1' and cached.exists():
        proof=json.loads(cached.read_text());old=proof['red']
        if proof['witness']==witness and proof['original_sha256']==original_hashes[proof['source']] and proof['mutant_sha256']==hashlib.sha256((ROOT/proof['source']).read_bytes()).hexdigest() and old['marker']==marker:
            recorded=(e.OUT/f'{label}.log').read_text()
            summaries=re.findall(r'^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed;',recorded,re.M)
            assert summaries[-1]==('FAILED','0','1') and marker in recorded and old['exit']==101
            print(label,'reused completed RED; matching original/mutant hashes and named failure',flush=True)
            return {**old,'reused_completed_execution':True}
    if not witness.startswith(('project::',APP)):
        return original_run(label,witness,marker,red)
    project=witness.startswith('project::')
    actual=witness.removeprefix('project::')
    options=['-p','kinewright-project','--features','test-util','--test','te15_b2b_recovery'] if project else ['-p','kinewright-app','--features','kinewright-agent/te1-eval','--bin','kinewright-app']
    args=['bash','-c','source scripts/setup-ffmpeg.sh && exec flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 nice -n19 cargo test -j4 "$@"','b2b',*options,actual,'--','--exact','--nocapture']
    with (e.OUT/f'{label}.log').open('w') as log:
        e.active=subprocess.Popen(args,cwd=ROOT,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
        (e.OUT/'active-pid.json').write_text(json.dumps({'pid':e.active.pid,'label':label})+'\n')
        try:code=e.wait_for_cargo(e.active, e.OUT/f'{label}.log')
        finally:
            if e.active.poll() is None:e.stop(e.active)
            e.active=None
            (e.OUT/'active-pid.json').unlink(missing_ok=True)
    text=(e.OUT/f'{label}.log').read_text(errors='replace')
    summaries=re.findall(r'^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed;',text,re.M)
    assert summaries,label+': no completed test summary'
    assert summaries[-1]==(('FAILED','0','1') if red else ('ok','1','0')),(label,summaries[-1])
    assert (code!=0)==red,label
    if red:assert marker in text,label+': missing behavior '+marker
    print(label,code,summaries[-1],flush=True)
    return {'exit':code,'summary':summaries[-1],'marker':marker if red else None}
e.run=run
if __name__=='__main__':e.main()
