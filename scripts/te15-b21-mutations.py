#!/usr/bin/env python3
"""B2-1 real consent/broker witnesses, named REDs and byte-identical restores."""
from pathlib import Path
import argparse
import hashlib
import importlib.util
import json
import os
import re
import shlex
import signal
import subprocess
import sys

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
OUT = Path(os.environ.get('KINEWRIGHT_TE15_B21_OUT', ROOT/'target/te1-5/b2-1/pairs'))
A='crates/kinewright-agent/src/'
S=A+'server.rs'
C=A+'writer_host/task_consent.rs'
B=A+'server/task_confirmation.rs'
H='writer_host::hooks_tests::tests::tests::te15_b21_'
U='server::task_confirmation::tests::'
PAIRS=[]
def add(name, edits, witness, marker, package='kinewright-agent'):
    PAIRS.append((name, edits, package, witness, marker))

add('grant-published-with-start',[(A+'writer_host/spending.rs','        self.seed_edit_consent(&mut state)?;','')],H+'pause_cut_is_approved_by_recorded_consent','B21_PREPARE')
add('grant-answers-without-legacy-wait',[(S,'if self.task_control.is_none()\n            && let Some(description) = confirmation.as_ref()','if true\n            && let Some(description) = confirmation.as_ref()')],H+'pause_cut_is_approved_by_recorded_consent','B21_NO_CONFIRMATION_WAIT')
prepare_guard='''                .map_err(|e| e.to_string())?;
        }
        self.ensure_verified_patched_sources(document, &operations)?;'''
for name,witness in [('missing-grant','missing_grant_refuses_at_prepare_and_commit'),('argument-not-grant','prompt_or_argument_cannot_supply_a_grant')]:
    add(name,[(S,prepare_guard,prepare_guard.replace('.map_err(|e| e.to_string())?;', '.unwrap_or(());'))],H+witness,'B21_REFUSED')
for field,guard in [('project','consent.project != binding.project'),('run','consent.run != state.run'),('candidate','consent.candidate != state.tasks[0].brief.scope.base.branch\n            || consent.candidate != task.brief.scope.base.branch'),('tracks','!task.brief.scope.tracks.is_subset(&consent.tracks)'),('classes','!operations\n                .iter()\n                .all(|op| class(op).is_some_and(|c| consent.classes.contains(c)))')]:
    add('grant-'+field,[(C,guard,'false')],H+'grant_cannot_cover_foreign_run_tracks_or_missing_classes','B21_REFUSED')
for field in ['run','task','attempt','turn','batch_digest']:
    add('exact-'+field,[(C,'if request.digest != digest(&current)? {','if false {')],H+'wrong_'+field+'_refuses','B21_REFUSED')
add('person-branch',[
    (S,'if !endpoint.matches_core(&self.core) {','if false {'),
    (C,'''            || !self.endpoint.matches_core(core)
            || runtime.core().actor_identity() != core.actor_identity()
            || host.owner()?.core().actor_identity() == core.actor_identity()''',''),
    # Also remove the independent runtime actor/cut guard.
    (C,'if actual != binding.revision || document.as_ref() != before {','if false {'),
],H+'same_batch_on_the_persons_actor_refuses','B21_PERSON_BRANCH_REFUSED')
add('asset-grant-does-not-widen-input-authority',[
    (S,'if self.host_scoped && !host_scoped_tool_admitted(&request) {','if self.host_scoped && false {'),
    (C,'        _ => None,','        _ => Some("timeline_edit"),'),
],H+'asset_deletion_and_file_effects_refuse_even_with_grant','B21_REFUSED')
for mode in ['pause','stop','attempt_retirement','endpoint_retirement']:
    add('pending-'+mode,[(B,'        let _ = self.broker.claim_task(self).map(drop);','')],H+'pending_'+mode+'_refuses','B21_PENDING_REQUEST_REFUSED')
add('pending-restart',[(B,'        let _ = self.broker.claim_task(self).map(drop);','')],H+'pending_host_restart_refuses_and_preserves_the_grant','B21_PENDING_REQUEST_REFUSED')
add('broker-digest',[(B,'if bound.get(&request.id) != Some(&request.digest) {','if false {')],U+'te15_b21_broker_binds_digest_and_refuses_late_replayed_or_legacy_answers','B21_DIGEST_BOUND')
add('broker-instance',[(B,'if !Arc::ptr_eq(&self.pending, &request.broker.pending) {','if false {')],U+'te15_b21_broker_binds_digest_and_refuses_late_replayed_or_legacy_answers','B21_BROKER_INSTANCE_BOUND')
# The replay mutant keeps a sender. Disable request-Drop cleanup as well,
# so assertion unwinding cannot send a third refusal into the full channel.
add('broker-once',[(B,'.remove(&request.id)\n            .ok_or("confirmation cancelled")?', '.get(&request.id).cloned()\n            .ok_or("confirmation cancelled")?'),(B,'        bound.remove(&request.id);',''),(B,'        let _ = self.broker.claim_task(self).map(drop);','')],U+'te15_b21_broker_binds_digest_and_refuses_late_replayed_or_legacy_answers','B21_ANSWER_ONCE')
add('no-legacy-answer',[(S,'if bound.contains_key(&id) {','if false {')],U+'te15_b21_broker_binds_digest_and_refuses_late_replayed_or_legacy_answers','B21_NO_LEGACY_ANSWER')
add('time-is-refusal',[(B,'if request.created.elapsed() >= self.timeout {','if false {')],U+'te15_b21_broker_never_treats_elapsed_time_or_no_answer_as_consent','B21_TIME_IS_REFUSAL')
add('late-held-answer',[(B,'if self.created.elapsed() >= self.timeout {','if false {')],U+'te15_b21_broker_never_treats_elapsed_time_or_no_answer_as_consent','B21_ANSWER_DEADLINE')
add('no-unanswered-approval',[(B,'        let _ = self.broker.claim_task(self).map(drop);','')],U+'te15_b21_broker_never_treats_elapsed_time_or_no_answer_as_consent','B21_UNANSWERED_REFUSED')
add('pending-capacity',[(B,'if pending.len() >= kinewright_project::orchestration_records::ITEM_LIMIT {','if false {')],U+'te15_b21_pending_broker_capacity_is_bounded_and_reusable','B21_PENDING_CAPACITY')
add('audit-capacity',[(C,'if state.edit_approvals.len() >= ITEM_LIMIT {','if false {'),('crates/kinewright-project/src/orchestration_commit.rs','            state.edit_approvals.len(),','')],H+'approval_audit_is_bounded_before_another_edit','B21_AUDIT_CAPACITY')
add('trim-and-track-classes',[(C,'"trim_clip" => Some("trim"),','"trim_clip" => None,')],H+'grant_covers_reversible_trim_and_track_removal','B21_CLASSIFIED_TRIM_TRACK')
add('legacy-investigator',[(A+'session.rs','broker.reject(request.id, INVESTIGATOR_CONFIRMATION_REFUSAL);','broker.approve(request.id);')],'session::tests::in2_a_confirmation_inside_a_session_ends_it_with_a_policy_violation',"the request is rejected with the contract's literal reason")
add('legacy-studio',[(A+'session.rs','broker.reject(request.id, INVESTIGATOR_CONFIRMATION_REFUSAL);','broker.approve(request.id);')],'session::tests::te15_b21_legacy_studio_confirmation_still_rejects_and_stops','B21_LEGACY_STUDIO_REFUSES')
add('legacy-eval',[(A+'session.rs','if !broker.approve(request.id) {','if !broker.reject(request.id, "refused") {')],'session::tests::in2_the_pump_under_approve_all_approves_and_continues','B21_LEGACY_EVAL_APPROVES')
add('visible-approval',[(A+'writer_host/orchestration.rs','edit_approvals: self.edit_approval_details(&projection)?,','edit_approvals: vec![],')],H+'pause_cut_is_approved_by_recorded_consent','B21_VISIBLE_APPROVAL')
add('ui-approval',[( 'crates/kinewright-app/src/studio_run_view.rs','edit_approvals: page\n                .edit_approvals','edit_approvals: Vec::<kinewright_agent::writer_host::EditApproval>::new()')], 'studio_feedback::ja_s3_feedback::te15_b1_fix4::te15_b21_run_details_lists_the_approved_private_batch','B21_PLAIN_APPROVAL_DETAILS','kinewright-app')
# Retained guards adjacent to the real task-server and commit path.
def load(name, filename):
    spec=importlib.util.spec_from_file_location(name,ROOT/'scripts'/filename)
    module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module);return module
for filename, attr, names in [
    ('te15-b20-mutations.py','PAIRS',{'late-base-motion','late-base-editor'}),
    ('te12-mutations.py','MUTATIONS',{'dispatch-hidden','dispatch-forged-role'}),
]:
    module=load('b21_'+filename.replace('-','_'),filename)
    rows=getattr(module,attr)
    found=[]
    for row in rows:
        if row[0] not in names:continue
        found.append(row[0])
        if len(row)==5:
            name,edits,package,witness,marker=row
        else:
            name,file,before,after,witness,marker,package=row;edits=[(file,before,after)]
        if name=='dispatch-hidden':
            # Root tasks are now independently barred by recorded edit consent.
            # Remove both prepare guards to reach the unauthorized plan.
            edits=[*edits,(S,prepare_guard,prepare_guard.replace('.map_err(|e| e.to_string())?;', '.unwrap_or(());'))]
        add('near-'+name,edits,witness,marker,package)
    assert set(found)==names,(filename,found)
active=None

def stop(child):
    try:os.killpg(child.pid,signal.SIGTERM)
    except ProcessLookupError:pass
    try:child.wait(timeout=10)
    except subprocess.TimeoutExpired:
        try:os.killpg(child.pid,signal.SIGKILL)
        except ProcessLookupError:pass
        child.wait()

def interrupted(number,_):raise InterruptedError(f'signal {number}')

def run(label,package,witness,marker,red):
    global active
    target=['--bin','kinewright-app'] if package=='kinewright-app' else ['--lib']
    cmd=['cargo','test','-p',package,*target,witness,'--','--exact','--nocapture']
    command='source scripts/setup-ffmpeg.sh >/dev/null\nexec flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 nice -n 19 '+shlex.join(cmd)
    with (OUT/f'{label}.log').open('w') as log:
        active=subprocess.Popen(['bash','-c',command],cwd=ROOT,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
        (OUT/'active-pid.json').write_text(json.dumps({'pid':active.pid,'label':label})+'\n')
        try:code=active.wait(timeout=1200)
        finally:
            if active.poll() is None:stop(active)
            active=None;(OUT/'active-pid.json').unlink(missing_ok=True)
    output=(OUT/f'{label}.log').read_text(errors='replace')
    summaries=re.findall(r'^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed;[^\n]*',output,re.M)
    assert summaries,label+': no completed summary'
    assert summaries[-1]==(('FAILED','0','1') if red else ('ok','1','0')),(label,summaries[-1])
    assert (code!=0)==red,label
    if red:assert marker in output,label+': missing named failure '+marker
    print(label,code,summaries[-1],flush=True)
    return {'exit':code,'summary':summaries[-1],'marker':marker if red else None}

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--only',action='append',default=[]);args=parser.parse_args()
    pairs=[row for row in PAIRS if not args.only or row[0] in args.only];assert pairs and set(args.only)<={row[0] for row in PAIRS}
    OUT.mkdir(parents=True,exist_ok=True)
    for var,folder in [('TMPDIR','t'),('LOCALAPPDATA','l')]:
        (ROOT/'target'/folder).mkdir(exist_ok=True);os.environ[var]=str(ROOT/'target'/folder)
    for number in (signal.SIGINT,signal.SIGTERM):signal.signal(number,interrupted)
    # Several witnesses remove the same guard. Compile that identical mutant
    # once, run each named RED, restore once, then run each named GREEN.
    groups={}
    for pair in pairs:
        groups.setdefault(tuple(pair[1]),[]).append(pair)
    proofs=[]
    for group in groups.values():
        edits=group[0][1]
        originals={file:(ROOT/file).read_bytes() for file,_,_ in edits}
        changed=dict(originals)
        for file,before,after in edits:
            assert changed[file].count(before.encode())==1,group[0][0]+': anchor drifted '+file
            changed[file]=changed[file].replace(before.encode(),after.encode())
        rows=[]
        for name,_,package,witness,marker in group:
            row={'name':name,'witness':witness,'sources':[]}
            for file,original in originals.items():
                (OUT/f'{name}-{Path(file).name}.original').write_bytes(original)
                row['sources'].append({'path':file,'original_sha256':hashlib.sha256(original).hexdigest(),'mutant_sha256':hashlib.sha256(changed[file]).hexdigest()})
            rows.append(row)
        try:
            for file,content in changed.items():(ROOT/file).write_bytes(content)
            for row,(_,_,package,witness,marker) in zip(rows,group,strict=True):
                row['red']=run(row['name']+'-red',package,witness,marker,True)
        finally:
            if active is not None:stop(active)
            for file,content in originals.items():(ROOT/file).write_bytes(content)
        for file,content in originals.items():assert (ROOT/file).read_bytes()==content
        for row,(_,_,package,witness,marker) in zip(rows,group,strict=True):
            row['green']=run(row['name']+'-green',package,witness,marker,False)
            for source in row['sources']:
                source['restored_sha256']=hashlib.sha256((ROOT/source['path']).read_bytes()).hexdigest()
                assert source['restored_sha256']==source['original_sha256']
            proofs.append(row)
            (OUT/f"{row['name']}.json").write_text(json.dumps(row,indent=2)+'\n')
            (OUT/'pairs.json').write_text(json.dumps(proofs,indent=2)+'\n')
    print(f'{len(proofs)} pairs complete; byte-for-byte restores',flush=True)
if __name__=='__main__':main()
