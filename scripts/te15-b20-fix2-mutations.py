#!/usr/bin/env python3
"""Preparation publication rollback and person-facing recovery pairs."""
from pathlib import Path
import importlib.util
import os
import sys
sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('b202', ROOT/'scripts/te15-b20-fix1-mutations.py')
f = importlib.util.module_from_spec(spec)
spec.loader.exec_module(f)
r = f.r
r.h.OUT = Path(os.environ.get('KINEWRIGHT_TE15_B202_OUT', ROOT/'target/te1-5/b2-0-fix2/pairs'))
A='crates/kinewright-agent/src/'
W=A+'writer_host/'
P=W+'task_preparation.rs'
O=W+'orchestration.rs'
H='writer_host::hooks_tests::tests::'
def add(name,file,before,after,test,marker):
    r.PAIRS.append((name,[(file,before,after)],'kinewright-agent',H+'te15_b202_'+test,marker))
for test in ['after_history','setup_publish']:
    add('rollback-'+test,O,'if publication.is_err() && saved.is_none() {','if false && publication.is_err() && saved.is_none() {',test,'B202_FIRST_RETRY')
add('history-base-consistency',W+'task_history.rs','if base.base_head != brief.scope.base.head {','if false && base.base_head != brief.scope.base.head {','history_base_guard','B202_MIXED_BASE_REFUSED')
for name in ['frozen','history','legacy','base']:
    add('permanent-'+name,P,'if kind.permanent() {','if false && kind.permanent() {','permanent_'+name,'B202_PERMANENT_NO_RETRY')
for name in ['files','provider','settings']:
    before='''            if matches!(
                state.wait,
                RunWait::Files(_) | RunWait::Provider(_) | RunWait::ProviderSettings(_)
            ) {'''
    add('sent-priority-'+name,P,before,'            if false {','sent_'+name,'B202_SENT_WAIT_PRIORITY')
for role in ['sent','director']:
    r.PAIRS.append(('recovery-status-'+role,[('crates/kinewright-app/src/studio_run_view.rs','kinewright_agent::writer_host::task_preparation_words::is_recovery_reason(reason)','false && kinewright_agent::writer_host::task_preparation_words::is_recovery_reason(reason)')],'kinewright-app','ja1_attachment::tests::te15_b202_'+role+'_recovery_controls','B202_RECOVERY_STATUS_WORDS'))
r.PAIRS.append(('real-recovery-controls',[(W+'task_preparation_words.rs','Save current edit, then open <branch>','Reopen')],'kinewright-app','ja1_attachment::tests::te15_b202_sent_recovery_controls','B202_REAL_RECOVERY_CONTROL'))
add('permanent-marker',P,'intent.permanent_preparation_failure = Some(kind);','intent.permanent_preparation_failure = None;','permanent_frozen','B202_CONTINUE_REFUSED')
add('permanent-control',O,'&& let Some(kind) = self.preparation_block(&state)?','&& false && let Some(kind) = self.preparation_block(&state)?','permanent_frozen','B202_CONTINUE_REFUSED')
r.PAIRS.append(('permanent-replay', [(O, 'self\n                        .owner()?\n                        .orchestration_receipt(&intent.run_id, &request.envelope.id)?\n                        .is_none()\n                        && let Some(kind) = self.preparation_block(&state)?', 'true && let Some(kind) = self.preparation_block(&state)?')], 'kinewright-agent', H+'te15_b202_permanent_replay', 'B202_ACCEPTED_REPLAY'))
r.PAIRS=r.PAIRS[55:]+r.PAIRS[:55]
r.h.FAST_WITNESSES={row[3] for row in r.PAIRS}
if __name__=='__main__':
    r.main()
