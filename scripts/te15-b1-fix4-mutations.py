#!/usr/bin/env python3
"""B1 fix 4 original-work recovery, final words and Studio controls, with exact restores."""
from pathlib import Path
import importlib.util
import os
import sys
sys.dont_write_bytecode = True
ROOT=Path(__file__).resolve().parent.parent

def load(name,filename):
    spec=importlib.util.spec_from_file_location(name, ROOT/'scripts'/filename)
    module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module);return module
h=load('fix4_base','te13-merge-mutations.py')
h.__doc__=__doc__
h.OUT=Path(os.environ.get('KINEWRIGHT_TE15_B1_FIX4_OUT',ROOT/'target/te1-5/b1-fix4/pairs'))
h.PAIRS=[]
A='crates/kinewright-agent/src/'
W=A+'writer_host/'
R=W+'provider_readiness.rs'
E=A+'orchestration/effect_pump.rs'
M=A+'orchestration/machine.rs'
O=W+'orchestration.rs'
S=W+'studio_runtime.rs'
F=W+'effect_execution.rs'
U='crates/kinewright-app/src/studio_feedback.rs'
C='crates/kinewright-app/src/studio_stop.rs'
H='writer_host::hooks_tests::tests::'
T=H+'te15_b1_fix4_'
UI='studio_feedback::ja_s3_feedback::te15_b1_fix4::te15_b1_fix4_'
def add(name,file,before,after,witness,marker,package='kinewright-agent'):
    h.PAIRS.append((name,file,before,after,package,witness,marker))
def section(file,start,end):
    text=(ROOT/file).read_text();a=text.index(start);return text[a:text.index(end,a)]
for suffix in ['opening','child_shared','child_own','steering','wake']:
    add('resend-'+suffix,F,'                EffectState::Pending','                EffectState::Settled',T+'retry_'+suffix,'FIX4_REFUSAL_RETAINED')
add('empty-continuation',M,'if needs_setup_continuation(&next) {','if false && needs_setup_continuation(&next) {',T+'empty_lane_gets_fresh_continuation','FIX4_GENUINELY_EMPTY')
model='The run is waiting for the person to retry the provider check. End your turn now. Do not retry this call.'
add('model-stop-sentence',R,model,'The run is waiting for the person to retry the provider check.',H+'te15_b1_fix2_nonsetup_model_refusal_keeps_provider_words','TE15_FIX2_NONSETUP_MODEL_WORDS')
add('model-person-sentence',R,model,'The provider check failed before anything was sent. Continue to try again. End your turn now. Do not retry this call.',H+'te15_b1_fix2_nonsetup_model_refusal_keeps_provider_words','TE15_FIX2_NONSETUP_MODEL_WORDS')
add('setup-model-stop',R,'The run is waiting for the person to fix Settings. End your turn now. Do not retry this call.','The run is waiting for the person to fix Settings.',H+'te15_b1_fix3_fast_model_refusal_ends_turn','FIX3_END_TURN')
for name,words,marker in [('revise-words','The provider check failed before anything was sent. Press Start run to try again.','FIX4_REVISE_WORDS'),('start-words','The provider check failed before anything was sent. Start the run again to try again.','FIX4_START_WORDS')]:
    add(name,R,words,'The provider check failed before anything was sent. Continue to try again.',T+'no_run_retry_words',marker)
add('kept-revise',U,'        self.revise = Some(request);','        let _ = request;',UI+'waiting_revise_keeps_valid_request','FIX4_REVISE_REPAIRED','kinewright-app')
add('waiting-revise-slot',U,'        if !matches!(start, RunStart::Unknown(_)) {','        if !matches!(start, RunStart::Unknown(_) | RunStart::Known(ReviseResult::Waiting { .. })) {',UI+'waiting_revise_keeps_valid_request','FIX4_WAITING_NOT_UNCONFIRMED','kinewright-app')
unknown='format!("{error} The send outcome is unknown. Press Stop, then Reopen to review your notes before starting another run. Continue cannot retry this send.")'
add('unknown-suffix',R,unknown,'error.to_string()',H+'te15_b1_fix3_postcharge_ambiguity_is_not_resumable','FIX4_FINAL_UNKNOWN_WORDS')
body=section(S,'            ) || (matches!(','\n        })')
add('unknown-final-handler',S,body,'            )',H+'te15_b1_fix3_postcharge_ambiguity_is_not_resumable','FIX4_FINAL_UNKNOWN_WORDS')
add('claimed-child-words',O,'"A task is still working. Continue when its turn ends."\n                        } else','"The Director is still working. Continue when its turn ends."\n                        } else',T+'live_child_uses_neutral_words','FIX4_RIGHT_TASK_WORDS')
add('unknown-child-words',O,'recovery_requires_person: a task has an unknown send outcome; Continue cannot replay it.','recovery_requires_person: the Director has an unknown send outcome; Continue cannot replay it.',T+'live_child_uses_neutral_words','FIX4_RIGHT_TASK_WORDS')
add('ui-pending-control-words',U,'A Continue request is not confirmed.','A Stop request is not confirmed.',UI+'notes_continue_and_exact_reopen_replay','FIX4_UI_PENDING_CONTROL_WORDS','kinewright-app')
body=section(U,'            Some(NoteAction::Continue(run)) => {','            Some(NoteAction::FirstSave)')
add('ui-continue-handler',U,body,'            Some(NoteAction::Continue(_)) => {}\n',UI+'full_notes_continue_reaches_host','FIX4_UI_REAL_HANDLER','kinewright-app')
add('ui-unknown-disabled',U,'.add_enabled(state.can_continue, egui::Button::new("Continue").small())','.add_enabled(true, egui::Button::new("Continue").small())',UI+'notes_unknown_disables_continue','FIX4_UI_UNKNOWN_DISABLED','kinewright-app')
for name,witness,marker in [('ui-settings-reason','notes_continue_and_exact_reopen_replay','FIX4_UI_WAIT_REASON'),('ui-unknown-reason','notes_unknown_disables_continue','FIX4_UI_UNKNOWN_REASON')]:
    body=section(U,'            Some(reason) => format!(', '            None => format!("{phase}{wait}{cleanup}"),')
    add(name,U,body,'            Some(_reason) => format!("{phase}{wait}{cleanup}"),\n',UI+witness,marker,'kinewright-app')
body=section(C,'                if let Err(why) = self.retain_for_dispatch() {','\n                request')
add('ui-keep-before-continue',C,body,'',UI+'notes_continue_and_exact_reopen_replay','C4_STOP_INTENT_KEPT_BEFORE_DISPATCH','kinewright-app')
body=section(C,'    pub(crate) fn retry_continues(','    pub(crate) fn retry_controls(')
add('ui-poll-exact-continue',C,body,'    pub(crate) fn retry_continues(&mut self, _port: &mut dyn FeedbackPort) {}\n',UI+'continue_poll_confirms_same_request','FIX4_UI_POLL_REPLAY','kinewright-app')
add('failed-wait-exits',E,'                return Err(error);','                return Ok(());',T+'failed_wait_exits_pump','FIX4_WAIT_FAILURE_EXITS')
near=load('fix4_near','te15-b1-fix3-mutations.py')
names={'proof-conflict','unknown-refusal','retained-final-other-provider','retained-final-settings','retained-charge-lock-compare','retained-stream-wait','retained-queued-retry-bound','retained-near-replay-preflight','retained-near-retained-immutable-replay'}
rows=[row for row in near.h.PAIRS if row[0] in names];assert len(rows)==len(names)
h.PAIRS.extend(('near-'+row[0],*row[1:]) for row in rows)
h.FAST_WITNESSES={row[5] for row in h.PAIRS}
for name,file,before,*_ in h.PAIRS:
    assert (ROOT/file).read_text().count(before)==1,name+': anchor drifted'
if __name__=='__main__':h.main()
