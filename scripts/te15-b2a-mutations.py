#!/usr/bin/env python3
"""Unpaid B2a oracle, observation, scheduling and pin pairs; exact-byte restore."""
from pathlib import Path
import argparse
import hashlib
import json
import os
import re
import signal
import subprocess
import sys
import time
ROOT=Path(__file__).resolve().parent.parent
OUT=Path(os.environ.get('KINEWRIGHT_TE15_B2A_OUT',ROOT/'target/te1-5/b2a/current/pairs'))
TEST='crates/kinewright-agent/tests/te1_measurement.rs'
DRIVER='crates/kinewright-agent/src/te1_eval.rs'
OBS='crates/kinewright-agent/src/te1_eval/observe.rs'
PIN='crates/kinewright-agent/tests/fixtures/te15-measurement-pin.json'
PAIRS=[]
for case in (1,5,6):
    PAIRS.append((f'oracle-{case}',TEST,'\n            ScriptedOutcome::Correct,','\n            ScriptedOutcome::Wrong,',f'te15_b2a_case{case}_scripted_studio_end_to_end','TE15_B2A_ORACLE_PASS'))
PAIRS += [
    ('durable-child',DRIVER,'return mcp.publish(Some(&context.fixtures["child_report"]));','return mcp.publish(None);','te15_b2a_case5_scripted_studio_end_to_end','durable child report did not reach Director'),
    ('conflict-schedule',OBS,'if self.case == 6','if self.case == 0','te15_b2a_case6_scripted_studio_end_to_end','case 6 stale plan was accepted'),
    ('pin-limit',PIN,'"tokens_per_task": 2000000','"tokens_per_task": 2000001','te15_b2a_measurement_pin_matches_every_observed_field','/limits/tokens_per_task'),
    ('source-hash',OBS,'//! Observes the real server boundary.','//! Mutant source hash. Observes the real server boundary.','te15_b2a_measurement_pin_matches_every_observed_field','/driver_sources_sha256/te1_eval~1observe.rs'),
]
POLICY='crates/kinewright-agent/src/orchestration/policy.rs'
ANALYSIS='crates/kinewright-agent/src/te1_eval/analysis.rs'
PAIRS += [
 ('ordinary-scope',POLICY,[
  ('if self.scope.objects.contains(object) {','if object == "track:88" || object == "mix:88" || self.scope.objects.contains(object) {'),
  ('if !self.scope.tracks.contains(&key) {','if key != "track:88" && !self.scope.tracks.contains(&key) {'),
  ('if !self.scope.bus_sidechain_closure.contains(key) {','if key != "track:88" && !self.scope.bus_sidechain_closure.contains(key) {'),
 ],None,'te15_b2a_case6_scripted_studio_end_to_end','TE15_B2A_ORDINARY_SCOPE'),

]
PAIRS.append(('two-note-batch','crates/kinewright-agent/src/writer_host/te1_eval.rs','for note in 0..if case == 5 { 2 } else { 1 } {','for note in 0..1 {','te15_b2a_case5_scripted_studio_end_to_end','TE15_B2A_NOTE_COUNT'))
active=None

def interrupt(number,_):
    raise InterruptedError(f'signal {number}')

def stop(child):
    try: os.killpg(child.pid,signal.SIGTERM)
    except ProcessLookupError: pass
    try: child.wait(timeout=10)
    except subprocess.TimeoutExpired:
        try: os.killpg(child.pid,signal.SIGKILL)
        except ProcessLookupError: pass
        child.wait()

def wait_for_cargo(child, log, timeout=900):
    """Exclude shared-lock queue time; retain the active build/test deadline."""
    started=None
    while True:
        try:
            return child.wait(timeout=min(1, timeout))
        except subprocess.TimeoutExpired:
            if started is None and re.search(
                r'^\s*(?:Compiling|Finished|Checking|Blocking|Running)\b',
                Path(log).read_text(errors='replace'), re.M,
            ):
                started=time.monotonic()
            if started is not None and time.monotonic()-started >= timeout:
                raise subprocess.TimeoutExpired(child.args, timeout) from None


def run(label,witness,marker,red):
    global active
    target='--lib' if witness.startswith(('te1_eval::','writer_host::','job_guardian::')) else '--test te1_measurement'
    cmd=['bash','-c','source scripts/setup-ffmpeg.sh && exec flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 nice -n 19 cargo test -p kinewright-agent --features te1-eval '+target+' "$1" -- --exact --include-ignored --nocapture','b2a',witness]
    with (OUT/f'{label}.log').open('w') as log:
        active=subprocess.Popen(cmd,cwd=ROOT,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
        (OUT/'active-pid.json').write_text(json.dumps({'pid':active.pid,'label':label})+'\n')
        try: code=wait_for_cargo(active, OUT/f'{label}.log')
        finally:
            if active.poll() is None: stop(active)
            active=None
            (OUT/'active-pid.json').unlink(missing_ok=True)
    text=(OUT/f'{label}.log').read_text(errors='replace')
    summaries=re.findall(r'^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed;[^\n]*',text,re.M)
    assert summaries,label+': no completed test summary'
    assert summaries[-1]==(('FAILED','0','1') if red else ('ok','1','0')),(label,summaries[-1])
    assert (code!=0)==red,label
    if red: assert marker in text,label+': missing behavioral assertion '+marker
    print(label,code,summaries[-1],flush=True)
    return {'exit':code,'summary':summaries[-1],'marker':marker if red else None}

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--only',action='append',default=[])
    args=parser.parse_args()
    pairs=[p for p in PAIRS if not args.only or p[0] in args.only]
    assert pairs and set(args.only)<={p[0] for p in PAIRS}
    OUT.mkdir(parents=True,exist_ok=True)
    tmp=ROOT/'target/t';tmp.mkdir(exist_ok=True)
    os.environ['TMPDIR']=str(tmp)
    os.environ['LOCALAPPDATA']=str(ROOT/'target/l')
    for number in (signal.SIGINT,signal.SIGTERM):signal.signal(number,interrupt)
    proofs=[]
    originals={relative:(ROOT/relative).read_bytes() for _,relative,*_ in pairs}
    for name,relative,before,after,witness,marker in pairs:
        path=ROOT/relative;original=originals[relative];mutant=original
        if callable(before):
            mutant=before(original)
        else:
            replacements=before if isinstance(before,list) else [(before,after)]
            for before,after in replacements:
                before=before.encode();after=after.encode()
                assert mutant.count(before)==1,name+': anchor drifted'
                mutant=mutant.replace(before,after)
        row={'name':name,'source':relative,'witness':witness,'original_sha256':hashlib.sha256(original).hexdigest(),'mutant_sha256':hashlib.sha256(mutant).hexdigest()}
        try:
            path.write_bytes(mutant)
            row['red']=run(name+'-red',witness,marker,True)
        finally:
            if active is not None:stop(active)
            path.write_bytes(original)
            assert path.read_bytes()==original
        row['restored_sha256']=hashlib.sha256(path.read_bytes()).hexdigest()
        proofs.append(row)
        (OUT/f'{name}-red.json').write_text(json.dumps(row,indent=2)+'\n')
    # Every mutant is restored immediately. Identical restored witnesses share
    # one final GREEN, after checking all source files are byte-identical.
    for relative,original in originals.items():assert (ROOT/relative).read_bytes()==original
    greens={}
    for row in proofs:
        witness=row['witness']
        if witness not in greens:greens[witness]=run(witness+'-green',witness,'',False)
        row['green']=greens[witness]
        for relative,original in originals.items():assert (ROOT/relative).read_bytes()==original
        (OUT/f"{row['name']}.json").write_text(json.dumps(row,indent=2)+'\n')
    (OUT/'pairs.json').write_text(json.dumps(proofs,indent=2)+'\n')
    print(f'{len(proofs)} pairs complete; byte-for-byte restores; {len(greens)} distinct restored witnesses',flush=True)
if __name__=='__main__':main()
