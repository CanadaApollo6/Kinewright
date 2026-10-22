"""Measure sealed before/after storage workloads serially, with load and sync evidence."""
from pathlib import Path
import gzip
import hashlib
import json
import os
import resource
import statistics
import subprocess
import sys
import threading
import time

out = Path(sys.argv[1]).resolve()
out.mkdir(parents=True, exist_ok=True)
sealed = Path('target/ja-s1-measurements/r1').resolve()
binaries = {name: sealed/name for name in ('before', 'after', 'profiled')}
hashfile = lambda p: hashlib.sha256(p.read_bytes()).hexdigest()
identity = {
    'before_source': '0f4f88e0a46544810be41db654f66472f7d4540c plus per-comment counter instrumentation only',
    'after_parent': subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
    'after_diff_sha256': hashlib.sha256(subprocess.check_output(['git','diff','--binary'])).hexdigest(),
    'binary_sha256': {name:hashfile(path) for name,path in binaries.items()},
    'rustc': subprocess.check_output(['rustc','-Vv'],text=True),
    'profile':'release; ordinary system allocator; separate DHAT 0.3.3 profiled runs',
    'after_source_files_sha256':{str(path):hashfile(path) for path in Path('crates').rglob('*.rs') if 'target' not in path.parts},
}
(out/'identity.json').write_text(json.dumps(identity,indent=2)+'\n')

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

runs=[]
for pair in range(1,6):
    for name in ('before','after'):
        assert hashfile(binaries[name]) == identity['binary_sha256'][name]
        stem=f'pair-{pair}-{name}'
        assert not (out/f'{stem}.json.gz').exists()
        before=annotation(); samples=[]; stop=threading.Event()
        sampler=threading.Thread(target=sample_processes,args=(stop,samples));sampler.start()
        cpu_before=resource.getrusage(resource.RUSAGE_CHILDREN)
        started=time.monotonic()
        result=subprocess.run(['timeout','600','nice','-n','10',str(binaries[name]),str(out/f'{stem}-workload')],capture_output=True,text=True)
        elapsed=time.monotonic()-started
        cpu_after=resource.getrusage(resource.RUSAGE_CHILDREN)
        stop.set();sampler.join()
        after=annotation()
        with gzip.open(out/f'{stem}.stderr.gz','wt') as file:file.write(result.stderr)
        assert result.returncode==0,(stem,result.returncode,result.stderr[-4000:])
        value=json.loads(result.stdout)
        assert value['errors']==0 and value['expected_failed_copies']==3
        assert value['source']['physical_bytes']==value['source_physical']['payload_bytes']
        assert value['history']['accepted_commands']==120
        assert value['metadata']['comments']['count']==8
        assert value['first_save']['source_after_last_reader']['physical_bytes']==0
        with gzip.open(out/f'{stem}.json.gz','wt') as file:json.dump(value,file)
        row={'pair':pair,'side':name,'elapsed_seconds':elapsed,'user_cpu_seconds':cpu_after.ru_utime-cpu_before.ru_utime,'system_cpu_seconds':cpu_after.ru_stime-cpu_before.ru_stime,'comment':value['metadata']['comments'],'save':value['metadata']['saves'],'comment_syncs':value['metadata']['comment_syncs'],'rss':value['final_rss'],'history':value['history'],'annotation_before':before,'annotation_after':after,'contamination_samples':samples}
        with gzip.open(out/f'{stem}-evidence.json.gz','wt') as file:json.dump(row,file)
        runs.append({k:v for k,v in row.items() if not k.startswith('annotation') and k!='contamination_samples'})
        print(stem, f"comment p50={row['comment']['p50_ms']:.3f} ms p95={row['comment']['p95_ms']:.3f} ms syncs={[s['calls'] for s in row['comment_syncs']]}",flush=True)

heap=[]
for pair in range(1,6):
    name='profiled';stem=f'pair-{pair}-{name}'
    assert hashfile(binaries[name])==identity['binary_sha256'][name]
    env=dict(os.environ,JA_S1_HEAP_FILE=str(out/f'{stem}-heap.json'))
    before=annotation(); samples=[]; stop=threading.Event()
    sampler=threading.Thread(target=sample_processes,args=(stop,samples));sampler.start()
    result=subprocess.run(['timeout','600','nice','-n','10',str(binaries[name]),str(out/f'{stem}-workload')],capture_output=True,text=True,env=env)
    stop.set();sampler.join()
    after=annotation()
    with gzip.open(out/f'{stem}.stderr.gz','wt') as file:file.write(result.stderr)
    assert result.returncode==0,(stem,result.stderr[-4000:])
    value=json.loads(result.stdout)
    assert value['errors']==0
    history=value['history'];allocated=history['decode_after_heap']['current_bytes']-history['decode_before_heap']['current_bytes']
    assert allocated <= history['decoded_reservation_bytes']
    heap.append({'pair':pair,'decode_live_allocation_bytes':allocated,'decode_reservation_bytes':history['decoded_reservation_bytes'],'encoded_history_bytes':history['encoded_bytes'],'whole_process_rust_allocator_peak_bytes':value['final_heap']['peak_bytes'],'rss':value['final_rss']})
    with gzip.open(out/f'{stem}.json.gz','wt') as file:json.dump({'measurements':value,'before':before,'after':after,'contamination_samples':samples},file)
    path=out/f'{stem}-heap.json'
    with gzip.open(str(path)+'.gz','wb') as file:file.write(path.read_bytes())
    path.unlink()
    print(stem,f'decode allocation={allocated} B bound={history["decoded_reservation_bytes"]} B',flush=True)

def distribution(values):return {'median':statistics.median(values),'min':min(values),'max':max(values)}
summary={'identity':{key:value for key,value in identity.items() if key != 'after_source_files_sha256'},'pairs':5,'unexpected_errors':0,'timing_scope':'five alternating unprofiled runs per side, eight durable comments per run; DHAT timings excluded','runs':runs,'heap_runs':heap,'comment_ms':{},'comment_sync_calls':{}}
for side in ('before','after'):
    selected=[r for r in runs if r['side']==side]
    summary['comment_ms'][side]={p:distribution([r['comment'][p] for r in selected]) for p in ('p50_ms','p95_ms')}
    summary['comment_sync_calls'][side]=distribution([s['calls'] for r in selected for s in r['comment_syncs']])
Path('docs/reports/ja1-s1/r1-storage-measurements.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps({key:summary[key] for key in ('comment_ms','comment_sync_calls','heap_runs')},indent=2),flush=True)
