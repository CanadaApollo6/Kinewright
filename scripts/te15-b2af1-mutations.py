#!/usr/bin/env python3
"""B2a fix 1 accounting, oracle, accepted-shape and launch-counter pairs."""
import importlib.util
import subprocess
from pathlib import Path

ROOT=Path(__file__).resolve().parent.parent
spec=importlib.util.spec_from_file_location('pairs',ROOT/'scripts/te15-b2a-mutations.py')
engine=importlib.util.module_from_spec(spec)
spec.loader.exec_module(engine)
engine.OUT=ROOT/'target/te1-5/b2a-fix1/new-pairs'
ANALYSIS='crates/kinewright-agent/src/te1_eval/analysis.rs'
CHECKS='crates/kinewright-agent/src/te1_eval/checks.rs'
DRIVER='crates/kinewright-agent/src/te1_eval.rs'
text=(ROOT/ANALYSIS).read_text()
a=text.index('        let mut usage_complete =');b=text.index('        Ok(Self {',a)
coverage=text[a:b]
legacy='        let usage_complete = !accounting.tasks.is_empty() && accounting.tasks.iter().all(|t| t.totals.tokens_complete);\n'
def unit(name):return 'te1_eval::checks::tests::te15_b2af1_'+name
PAIRS=[
 (name,ANALYSIS,coverage,legacy,'coverage::te15_b2af1_'+witness,marker)
 for name,witness,marker in [
  ('coverage-turn','uncovered_turn','TE15_B2AF1_TURN'),
  ('coverage-attempt','uncovered_attempt','TE15_B2AF1_ATTEMPT'),
  ('coverage-overflow','observation_overflow','TE15_B2AF1_OVERFLOW')]
]
PAIRS += [
 ('coverage-recovery',ANALYSIS,coverage,'        let usage_complete = false;\n','coverage::te15_b2af1_later_snapshot_closes_gap','TE15_B2AF1_RECOVER'),
 ('oracle-wav-path',CHECKS,'if oracle {','if false {',unit('original_audio_path'),'TE15_B2AF1_WAV'),
 ('recovery-sibling',CHECKS,'&& call.task == affected.task','&& true',unit('sibling_evidence'),'TE15_B2AF1_SIBLING'),
 ('recovery-fixture',CHECKS,'!call.fixture\n        && call.task.is_string()','true\n        && call.task.is_string()',unit('fixture_evidence'),'TE15_B2AF1_FIXTURE'),
 ('recovery-order',CHECKS,'for call in calls {','let mut unordered = calls.to_vec(); unordered.sort_by_key(|c| match c.tool.as_str() {"get_timeline_state" | "task_status" => 0, "prepare_edit_plan"=>1, _=>2}); for call in &unordered {',unit('ordered_recovery'),'TE15_B2AF1_ORDER'),
 ('launch-count',DRIVER,'trace.launch_requests += 1;','trace.launch_requests += 0;','te15_b2a_case1_scripted_studio_end_to_end','TE15_B2AF1_LAUNCH_COUNT'),
 ('native-count',DRIVER,'trace.provider_launches += 1;','trace.provider_launches += 0;',unit('native_fallback_counter'),'TE15_B2AF1_NATIVE_COUNT'),
 ('accepted-case1',CHECKS,'!history.cursor.undo.is_empty()','history.applied_operations() == serde_json::from_value::<Vec<kinewright_core::Operation>>(_data["operations"].clone()).unwrap()','te15_b2af1_alternative_outcomes','TE15_B2AF1_ALTERNATIVE'),
 ('accepted-case5',CHECKS,'&& Some(&read.task) == director','&& Some(&read.task) == director && !calls[..published+offset+1].iter().any(|c| c.tool=="merge_task" && !c.refused)',unit('case5_read_before_merge'),'TE15_B2AF1_READ_ALTERNATIVE'),
]
text=(ROOT/CHECKS).read_text();a=text.index('                .filter(|preview|');b=text.index('                .and_then(|_| reply.get("plan_id"))',a)
PAIRS.append(('bound-preview',CHECKS,text[a:b],'                .filter(|_| true)\n',unit('bound_preview'),'TE15_B2AF1_PREVIEW'))
a=text.index('        if (matches!(call.tool.as_str()');b=text.index('        if call.tool == "prepare_edit_plan"',a)
PAIRS.append(('accepted-case6',CHECKS,text[a:b],'        if call.tool == "get_timeline_state" && !call.refused { refreshed = true; }\n',unit('accepted_refresh_paths'),'TE15_B2AF1_REFRESH'))

def altered_audio(original):
    output=engine.OUT/'altered-carrier.mov'
    subprocess.run([str(ROOT/'third_party/ffmpeg/bin/ffmpeg'),'-v','error','-y','-i',str(ROOT/'crates/kinewright-agent/tests/fixtures/te15/talking-head-audio-owned.mov'),'-map','0','-c:v','copy','-c:a','pcm_s16le','-af','volume=0.5,asetnsamples=n=320:p=0','-fflags','+bitexact','-flags:a','+bitexact',str(output)],check=True)
    return output.read_bytes()
PAIRS.append(('carrier-audio','crates/kinewright-agent/tests/fixtures/te15/talking-head-audio-owned.mov',altered_audio,None,'te15_b2a_case1_scripted_studio_end_to_end','audio_samples: false'))
def old_packetization(original):
    return subprocess.check_output(['git','show','e9ae76a0:crates/kinewright-agent/tests/fixtures/te15/talking-head-audio-owned.mkv'],cwd=ROOT)
PAIRS.append(('carrier-seek','crates/kinewright-agent/tests/fixtures/te15/talking-head-audio-owned.mov',old_packetization,None,unit('carrier_seek_matches_wav'),'TE15_B2AF1_SEEK'))
PAIRS += [
 ('affected-task',CHECKS,'&& c.task == failed.task','&& true',unit('recovery_matches_injected_task'),'TE15_B2AF1_AFFECTED'),
 ('bound-plan',CHECKS,'call.arguments["plan_id"] == *plan','true',unit('bound_preview'),'TE15_B2AF1_PREVIEW'),
 ('bound-revision',CHECKS,'call.arguments["expected_revision"] == *revision','true',unit('bound_preview'),'TE15_B2AF1_PREVIEW'),
 ('bound-attempt',CHECKS,'&& call.attempt == affected.attempt','&& true',unit('bound_preview'),'TE15_B2AF1_PREVIEW'),
]
engine.PAIRS=PAIRS
if __name__=='__main__':engine.main()
