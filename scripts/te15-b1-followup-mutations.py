#!/usr/bin/env python3
"""B1 follow-ups: failed wait recovery, unknown sends, Studio controls and assembly."""
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
h = load('followup_base', 'te13-merge-mutations.py')
h.OUT = Path(os.environ.get('KINEWRIGHT_TE15_B1_FOLLOWUP_OUT', ROOT/'target/te1-5/b1-followups/pairs'))
h.PAIRS = []
W = 'crates/kinewright-agent/src/writer_host/'
R = W+'provider_readiness.rs'
S = W+'studio_runtime.rs'
C = W+'task_context.rs'
U = 'crates/kinewright-app/src/studio_feedback.rs'
H = 'writer_host::hooks_tests::tests::'
T = H+'te15_b1_followup_'
UI = 'studio_feedback::ja_s3_feedback::te15_b1_fix4::te15_b1_followup_'

def add(name, file, before, after, witness, marker, package='kinewright-agent'):
    h.PAIRS.append((name,file,before,after,package,witness,marker))

def section(file, start, end):
    text = (ROOT/file).read_text()
    a = text.index(start)
    return text[a:text.index(end,a)]

add('unknown-readiness',R,'if crate::orchestration::machine::has_unknown_send(&state) {','if false && crate::orchestration::machine::has_unknown_send(&state) {',T+'unknown_wait_survives_assign_and_stream','FOLLOWUP_UNKNOWN_READINESS_WAIT')
before = '        let state = self.spending_scope(producer, &identity.run)?;\n        if self.surface_unknown_send(producer, &state)? {\n            return Ok(());\n        }'
after = '        let state = self.spending_scope(producer, &identity.run)?;'
add('unknown-stream',S,before,after,T+'unknown_wait_survives_assign_and_stream','FOLLOWUP_UNKNOWN_STREAM_WAIT')
before = '        let state = self.spending_scope(producer, run)?;\n        if self.surface_unknown_send(producer, &state)? {\n            return Ok(());\n        }'
add('unpublished-unknown-final',S,before,'        let state = self.spending_scope(producer, run)?;',T+'unknown_wait_survives_assign_and_stream','FOLLOWUP_UNPUBLISHED_UNKNOWN_INTERRUPTS')
add('wait-classification',R,'.pending_provider_wait = Some(reason.to_owned());','.pending_provider_wait = None;',T+'wait_write_recovers_original_effect','FOLLOWUP_RESUMABLE_CLASSIFICATION')
add('wait-final-handler',S,'        self.retry_pending_provider_wait(run)?;','',T+'wait_write_recovers_original_effect','FOLLOWUP_RESUMABLE_CLASSIFICATION')
before = section(S,'                owner\n                    .retry_pending_provider_wait(&run)','                return Err(io::Error::other(reason.clone()));')
add('wait-blocked-recovery',S,before,'',T+'wait_write_recovers_after_final_handler','FOLLOWUP_RESUMABLE_CLASSIFICATION')
# Retired assembly-warm/assembly-cold: pending assembly now retains one job,
# superseded by te15-a/phase-tail and finished-deadline.
add('continue-only-waiting',U,'        && matches!(state.phase, RunPhase::Paused | RunPhase::Interrupted)','        && true',UI+'continue_only_when_waiting','FOLLOWUP_CONTINUE_ONLY_WAITING','kinewright-app')
before = section(U,'            .on_disabled_hover_text(','            .clicked()')
add('continue-hover',U,before,'',UI+'disabled_continue_shows_host_reason','FOLLOWUP_DISABLED_HOST_REASON','kinewright-app')
before = section(U,'            Some(reason) => format!(', '            None => format!("{phase}{wait}{cleanup}"),')
add('plain-row',U,before,'            Some(reason) => format!("{phase} · {reason}{cleanup}"),\n',UI+'wait_row_is_plain_and_details_keep_raw','FOLLOWUP_PLAIN_WAIT','kinewright-app')
near = load('followup_near_b1','te15-b1-fix4-mutations.py')
names = {'resend-opening','model-stop-sentence','unknown-suffix','unknown-final-handler','failed-wait-exits','ui-continue-handler','ui-keep-before-continue','ui-poll-exact-continue','near-retained-queued-retry-bound'}
selected = [row for row in near.h.PAIRS if row[0] in names]
assert len(selected) == len(names)
h.PAIRS.extend(('near-'+row[0],*row[1:]) for row in selected)
for filename, attr, names in [('te15-a-mutations.py','h',{'phase-tail','finished-deadline'}),('te14-fix1-mutations.py','harness',{'decoder-cancel'})]:
    registry = load('followup_'+filename.replace('-','_'),filename)
    selected = [row for row in getattr(registry,attr).PAIRS if row[0] in names]
    assert len(selected) == len(names)
    h.PAIRS.extend(('near-'+row[0],*row[1:]) for row in selected)
h.FAST_WITNESSES = {row[5] for row in h.PAIRS}
for name, file, before, *_ in h.PAIRS:
    assert (ROOT/file).read_text().count(before) == 1, name+': anchor drifted'
assert len(h.PAIRS) == 21
if __name__ == '__main__':
    h.main()
