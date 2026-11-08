#!/usr/bin/env python3
"""Charge-boundary recovery and ordinary fast-tier witnesses, with exact restores."""
from pathlib import Path
import importlib.util
import os
import sys
sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent

def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT/'scripts'/filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

h=load('fix3_base','te13-merge-mutations.py')
h.__doc__=__doc__
h.OUT=Path(os.environ.get('KINEWRIGHT_TE15_B1_FIX3_OUT',ROOT/'target/te1-5/b1-fix3/pairs'))
h.PAIRS=[]
A='crates/kinewright-agent/src/'
W=A+'writer_host/'
R=W+'provider_readiness.rs'
E=A+'orchestration/effect_pump.rs'
M=A+'orchestration/machine.rs'
O=W+'orchestration.rs'
F=W+'effect_execution.rs'
H='writer_host::hooks_tests::tests::'
T=H+'te15_b1_fix3_'
def add(name,file,before,after,witness,marker):
    h.PAIRS.append((name,file,before,after,'kinewright-agent',witness,marker))
def section(file,start,end):
    text=(ROOT/file).read_text(); a=text.index(start); return text[a:text.index(end,a)]
for name,suffix in [('probe-resume','probe_pending_resumes'),('sandbox-resume','sandbox_resumes')]:
    add(name,R,'owner.wait_for_provider(run, CHECK_RETRY)', '{ let producer=owner.run_producer(run)?; owner.record_runtime_refusal(&producer,run,&failure.to_string()) }',T+suffix,'FIX3_PRECHARGE_RESUMABLE')
add('binding-retry',E,'crate::orchestration::orchestration_records::EffectState::Pending\n                            | crate::orchestration::orchestration_records::EffectState::Settled', 'crate::orchestration::orchestration_records::EffectState::Settled',T+'binding_failure_resumes_without_charge','FIX3_BIND_RETRY')
body=section(E,'\n        owner\n            .lock()', '        let profile = self.profiles.bind(&attempt)?;')
add('initial-history',E,body,'',T+'binding_failure_resumes_without_charge','FIX3_BIND_HISTORY')
for name,witness,marker in [('not-sent',T+'never_sent_gets_one_fresh_continuation','FIX3_NEVER_SENT_SETTLED'),('guardian-not-sent',H+'te15_b1_fix2_setup_recheck_wait_and_unknown_continue_refusal','TE15_FIX2_SETUP_RECHECK_CLASSIFIED')]:
    add(name,F,'                EffectState::Settled','                EffectState::OutcomeUnknown',witness,marker)
add('not-sent-budget',F,'            self.record_not_dispatched_usage(producer, identity)?;', '            let _ = identity;', T+'never_sent_gets_one_fresh_continuation','FIX3_HOST_ZERO_PROOF')
add('proof-conflict',W+'native_spending.rs','if task.usage.iter().any(|sample| {','if false && task.usage.iter().any(|sample| {',T+'not_sent_proof_refuses_observed_usage','FIX3_PROOF_CONFLICT')
add('claimed-words',O,'return Err(refused(if live {','return Err(refused(if false && live {',T+'claimed_and_unknown_have_distinct_words','FIX3_TRUE_CONTINUE_WORDS')
body=section(M,'    super::requests::director(state).is_ok_and(|task| {','\n}\n\npub(crate) fn needs_setup_continuation')
add('unknown-refusal',M,body,'    let _=state; true',T+'claimed_and_unknown_have_distinct_words','FIX3_TRUE_CONTINUE_WORDS')
body=section(M,'        && !state.effects.iter().any(', '        && super::requests::director(state)')
add('child-only',M,body,'',T+'pending_child_owns_the_only_next_send','FIX3_NO_EXTRA_DIRECTOR')
add('fresh',M,'if needs_setup_continuation(&next) {','if false && needs_setup_continuation(&next) {',T+'never_sent_gets_one_fresh_continuation','FIX3_FRESH_CONTINUATION')
body=section(M,'    matches!(state.wait, RunWait::ProviderSettings(_))', '\n}\n\n/// C5 fix 10: the fixed reason')
add('pending-reuse',M,body,'    matches!(state.wait, RunWait::ProviderSettings(_))',T+'sandbox_resumes','FIX3_PENDING_REUSE')
body=section(O,'            && runtime\n                .last_continue', '\n        {\n            runtime.last_continue = Some(receipt.sequence);')
add('monotonic',O,body,'',T+'old_continue_cannot_unlock_worker','FIX3_CONTINUE_MONOTONIC')
add('restored-progress',W+'continuation.rs','last_continue: Some(continue_floor),','last_continue: None,',T+'restored_progress_ignores_old_continue','FIX3_CONTINUE_MONOTONIC')
add('child-authority',O,'!matches!(projection.wait, crate::orchestration::orchestration_records::RunWait::ProviderSettings(_))','true',T+'child_finishes_during_wait','FIX3_CHILD_AUTHORITY')
add('setup-words',R,'if matches!(\n        error,','if false && matches!(\n        error,',T+'fast_model_refusal_ends_turn','FIX3_END_TURN')
add('postcharge',R,'if resumable {','if true || resumable {',T+'postcharge_ambiguity_is_not_resumable','FIX3_POSTCHARGE_PROVIDER')
add('end-turn',R,'{reason} The run is waiting for the person to fix Settings. End your turn now. Do not retry this call.','{reason}',T+'fast_model_refusal_ends_turn','FIX3_END_TURN')
body=section(R,'        let reason = if setup_failure(failure)', '\n        if matches!(command, HostCommand::ReviseCut')
add('person-words',R,body,'        let reason=failure.to_string();',T+'person_gets_neutral_retry_words','FIX3_RETRY_WORDS')
add('continuation-words',M,'The provider check passed. Continue this run from its retained task state.','The provider setup was fixed. Continue this run from its retained task state.',T+'never_sent_gets_one_fresh_continuation','FIX3_CONTINUATION_WORDS')
near=load('fix3_near','te15-b1-fix2-mutations.py')
names={'final-other-provider','final-settings','charge-lock-compare','stream-wait','queued-retry-bound','near-replay-preflight','near-retained-immutable-replay'}
rows=[row for row in near.h.PAIRS if row[0] in names]; assert len(rows)==len(names)
h.PAIRS.extend(('retained-'+row[0],*row[1:]) for row in rows)
h.EQUIPPED_WITNESSES={H+'te15_b1_fix2_setup_recheck_wait_and_unknown_continue_refusal'}
h.FAST_WITNESSES={row[5] for row in h.PAIRS}-h.EQUIPPED_WITNESSES
for name,file,before,*_ in h.PAIRS:
    assert (ROOT/file).read_text().count(before)==1,name+': anchor drifted'
if __name__=='__main__': h.main()
