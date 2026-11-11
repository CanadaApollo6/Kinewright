"""Measure default-limit first saves, without builds."""
from pathlib import Path
import gzip
import argparse
import hashlib
import json
import os
import re
import statistics
import shutil
import subprocess
import sys
import threading
import time

assert os.getpriority(os.PRIO_PROCESS, 0) == 19
parser=argparse.ArgumentParser()
parser.add_argument('out',type=Path)
parser.add_argument('--test-binaries',type=Path,required=True)
args=parser.parse_args()
repo = Path.cwd().resolve()
out = args.out.resolve()
out.mkdir(parents=True, exist_ok=True)
sealed = repo/'target/ja-s1-r3-measure'
artifact=Path(json.loads(args.test_binaries.read_text())['ja_s1_storage'])
storage = sealed/'storage'
shutil.copy2(artifact,storage)
hashfile = lambda p: hashlib.sha256(p.read_bytes()).hexdigest()
identity = {'head':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
 'source_sha256':{str(p):hashfile(p) for p in Path('crates').rglob('*.rs') if 'target' not in p.parts},
 'binary_sha256':hashfile(storage),
 'binary_bytes':storage.stat().st_size,
 'storage_artifact':str(artifact), 'test_binaries_sha256':hashfile(args.test_binaries),
 'profile':'debug/test; system allocator; no compilation during measurements', 'compiler':subprocess.check_output(['rustc','-Vv'],text=True), 'os':sys.platform, 'TMPDIR':os.environ.get('TMPDIR')}
with gzip.open(out/'identity.json.gz','wt') as stream: json.dump(identity,stream,indent=2)

def annotation():
    def read(args):
        try:
            return subprocess.check_output(args, text=True, stderr=subprocess.STDOUT).strip()
        except (subprocess.CalledProcessError, FileNotFoundError) as error:
            return str(error)
    return {'uptime':read(['uptime']), 'cpu':read(['top','-b','-n','2','-d','1','-w','160']), 'gpu':read(['nvidia-smi','--query-gpu=pstate,clocks.current.graphics,clocks.current.memory,utilization.gpu','--format=csv,noheader'])}

def sample_processes(stop, samples):
    # /proc deltas give a five-second CPU window rather than lifetime ps averages.
    def snapshot():
        found={}
        for path in Path('/proc').iterdir():
            if not path.name.isdigit():continue
            try:
                text=(path/'stat').read_text(); end=text.rfind(')'); fields=text[end+2:].split()
                found[int(path.name)]=(int(fields[11])+int(fields[12]), text[text.index('(')+1:end])
            except (OSError, ValueError, IndexError):pass
        return found
    before=snapshot(); start=time.monotonic(); ticks=os.sysconf('SC_CLK_TCK')
    while not stop.wait(5):
        now=snapshot(); elapsed=time.monotonic()-start
        busy=[]
        for pid,(cpu,name) in now.items():
            if pid in before:
                percent=(cpu-before[pid][0])/ticks/elapsed*100
                if percent > 20: busy.append({'pid':pid,'name':name,'cpu_percent':round(percent,1)})
        samples.append({'time':time.time(),'above_20_percent':busy})
        before=now; start=time.monotonic()


def run(stem, command, expected, cwd=repo, env=None):
    before=annotation(); samples=[]; stop=threading.Event()
    sampler=threading.Thread(target=sample_processes,args=(stop,samples)); sampler.start()
    log=out/(stem+'.log')
    started=time.monotonic()
    with log.open('w') as stream:
        process=subprocess.Popen(['timeout','600',*command],cwd=cwd,env=env,stdout=stream,stderr=subprocess.STDOUT)
        _,status,usage=os.wait4(process.pid,0)
        process.returncode=os.waitstatus_to_exitcode(status)
    elapsed=time.monotonic()-started
    stop.set();sampler.join(); after=annotation()
    text=log.read_text()
    with gzip.open(str(log)+'.gz','wt') as stream: stream.write(text)
    log.unlink()
    row={'stem':stem,'elapsed_s':elapsed,'maxrss_kib':usage.ru_maxrss,'exit':process.returncode,'before':before,'after':after,'load_samples':samples}
    with gzip.open(out/(stem+'-telemetry.json.gz'),'wt') as stream: json.dump(row,stream)
    assert process.returncode==expected,(stem,process.returncode,text[-3000:])
    return row,text

history=[]
for pair in range(1,6):
    for limit in ('encoded','batch'):
        stem=f'f4-pair-{pair}-{limit}'
        row,text=run(stem,['nice','-n','10',str(storage),'--exact',f'round_three::first_save_floors_history_above_default_{limit}_limit','--nocapture'],0)
        assert 'test result: ok. 1 passed; 0 failed;' in text
        match=re.search(r'R3_FIRST_SAVE batch_limit=(\w+) original_batches=(\d+) retained_batches=(\d+) floor=(\d+) elapsed_ms=([\d.]+) bound_ms=([\d.]+)',text)
        assert match,text
        values=match.groups()
        history.append({'pair':pair,'limit':limit,'original_batches':int(values[1]),'retained_batches':int(values[2]),'floor':int(values[3]),'first_save_ms':float(values[4]),'bound_ms':float(values[5]),'maxrss_kib':row['maxrss_kib']})
        print(stem,history[-1],flush=True)

def distribution(values):return {'median':statistics.median(values),'min':min(values),'max':max(values)} if values else None
summary={'identity':{k:v for k,v in identity.items() if k!='source_sha256'},'unexpected_errors':0,'history':history,
 'history_cost':{limit:{metric:distribution([r[metric] for r in history if r['limit']==limit]) for metric in ('first_save_ms','bound_ms','maxrss_kib')} for limit in ('encoded','batch')}}
with gzip.open(out/'summary.json.gz','wt') as stream:json.dump(summary,stream,indent=2)
print(json.dumps({k:summary[k] for k in ('history_cost','unexpected_errors')},indent=2),flush=True)
