"""Paired F4 measurements with sealed test bytes and no compilation in the timing window."""
from pathlib import Path
import gzip, hashlib, json, os, re, shutil, statistics, subprocess, sys, threading, time
assert os.getpriority(os.PRIO_PROCESS, 0) == 19
repo=Path.cwd().resolve()
out=Path(sys.argv[1]).resolve(); out.mkdir(parents=True,exist_ok=True)
manifest=Path(sys.argv[2])
artifact=Path(json.loads(manifest.read_text())['ja_s1_storage'])
binary=out/'storage-test'
shutil.copy2(artifact,binary)
identity={'head':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
 'binary_sha256':hashlib.file_digest(binary.open('rb'),'sha256').hexdigest(),
 'compiler':subprocess.check_output(['rustc','-Vv'],text=True),
 'profile':'debug/test; system allocator; no compilation during measurements',
 'source_sha256':{str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in Path('crates').rglob('*.rs') if 'target' not in p.parts}}
with gzip.open(out/'identity.json.gz','wt') as stream:json.dump(identity,stream,indent=2)
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

rows=[]
for pair in range(1,6):
    for workload, witness in [
        ('encoded','round_three::first_save_floors_history_above_default_encoded_limit'),
        ('batch','round_three::first_save_floors_history_above_default_batch_limit'),
        ('excursion','round_four::prefilter_large_excursion_keeps_twenty_undo_steps')]:
        assert hashlib.file_digest(binary.open('rb'),'sha256').hexdigest()==identity['binary_sha256']
        row,text=run(f'f4-{pair}-{workload}',['nice','-n','10',str(binary),'--exact',witness,'--nocapture'],0)
        pattern=r'R4_EXCURSION bound_ms=([\d.]+)' if workload=='excursion' else r'R4_COLD batch_limit=\w+ bound_ms=([\d.]+)'
        cold=float(re.search(pattern,text).group(1))
        match=re.search(r'R3_FIRST_SAVE.*elapsed_ms=([\d.]+) bound_ms=([\d.]+)',text)
        result={'pair':pair,'workload':workload,'cold_bound_ms':cold,'maxrss_kib':row['maxrss_kib']}
        if match:result.update(first_save_retry_ms=float(match[1]),retry_bound_ms=float(match[2]))
        rows.append(result);print(json.dumps(result),flush=True)
summary={w:{'median_ms':statistics.median(r['cold_bound_ms'] for r in rows if r['workload']==w),
 'range_ms':[min(r['cold_bound_ms'] for r in rows if r['workload']==w),max(r['cold_bound_ms'] for r in rows if r['workload']==w)]} for w in ['encoded','batch','excursion']}
with gzip.open(out/'summary.json.gz','wt') as stream:json.dump({'runs':rows,'cold_selection':summary},stream,indent=2)
binary.unlink()
print(json.dumps(summary,indent=2),flush=True)
