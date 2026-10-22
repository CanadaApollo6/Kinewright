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
    assert process.returncode in (expected if isinstance(expected, tuple) else (expected,)),(stem,process.returncode,text[-3000:])
    return row,text

rows=[]
for pair in ([] if '--setup-only' in sys.argv[3:] else range(1,6)):
    for workload, witness in [
        ('encoded','round_three::first_save_floors_history_above_default_encoded_limit'),
        ('batch','round_three::first_save_floors_history_above_default_batch_limit'),
        ('excursion','round_four::prefilter_large_excursion_keeps_twenty_undo_steps')]:
        assert hashlib.file_digest(binary.open('rb'),'sha256').hexdigest()==identity['binary_sha256']
        row,text=run(f'f4-{pair}-{workload}',['nice','-n','10',str(binary),'--exact',witness,'--nocapture'],0,env=dict(os.environ, JA_S1_COMMIT_TIMING='1'))
        pattern=r'R4_EXCURSION bound_ms=([\d.]+)' if workload=='excursion' else r'R4_COLD batch_limit=\w+ bound_ms=([\d.]+)'
        cold=float(re.search(pattern,text).group(1))
        match=re.search(r'R3_FIRST_SAVE.*elapsed_ms=([\d.]+) bound_ms=([\d.]+)',text)
        result={'pair':pair,'workload':workload,'cold_bound_ms':cold,'maxrss_kib':row['maxrss_kib'], 'actor_commit':[{'batches':int(b),'elapsed_us':int(t)} for b,t in re.findall(r'R5_ACTOR_COMMIT batches=(\d+) elapsed_us=(\d+)',text)]}
        if match:result.update(first_save_retry_ms=float(match[1]),retry_bound_ms=float(match[2]))
        rows.append(result);print(json.dumps(result),flush=True)
summary=({w:{'median_ms':statistics.median(r['cold_bound_ms'] for r in rows if r['workload']==w),
 'range_ms':[min(r['cold_bound_ms'] for r in rows if r['workload']==w),max(r['cold_bound_ms'] for r in rows if r['workload']==w)]} for w in ['encoded','batch','excursion']}
 if rows else {})
if rows:
    with gzip.open(out/'summary.json.gz','wt') as stream:json.dump({'runs':rows,'cold_selection':summary},stream,indent=2)
binary.unlink()
print(json.dumps(summary,indent=2),flush=True)

# Re-run both already-built executables in one annotated timing window.
old = Path('/home/riels/Projects/Personal/Kinewright/target/review/ja/s1-evidence/r5/before/sealed/kinewright-ja-baseline')
new = out/'current/kinewright-ja-baseline'; new.parent.mkdir(exist_ok=True)
shutil.copy2(repo/'target/debug/kinewright-ja-baseline', new)
setup = []
for label, executable in [('before',old),('after',new)]:
    binary_hash = hashlib.file_digest(executable.open('rb'),'sha256').hexdigest()
    fixture = repo/'crates/kinewright-agent/tests/fixtures/ja/participants-lock.json'
    report = out/f'setup-{label}-data'; profile = out/f'setup-{label}-profile'
    row, text = run(f'setup-{label}', ['bash','scripts/ja-s1-profile-control.sh',str(executable),str(fixture),str(report),str(profile)], 0)
    events = [json.loads(line) for line in (report/'events.jsonl').read_text().splitlines()]
    spawns = [e for e in events if e['kind'] == 'process_spawn' and e['participant'] in ['A','B']]
    assert len(spawns) == 2
    assert hashlib.file_digest(executable.open('rb'),'sha256').hexdigest() == binary_hash
    setup.append({'label':label,'binary_sha256':binary_hash,'binary_bytes':executable.stat().st_size,'second_spawn_ms':int(spawns[-1]['elapsed_ns'])/1e6,'run':row})
with gzip.open(out/'setup-pair.json.gz','wt') as stream: json.dump(setup,stream,indent=2)
print(json.dumps([{'label':r['label'],'binary_bytes':r['binary_bytes'],'second_spawn_ms':r['second_spawn_ms']} for r in setup]),flush=True)
new.unlink()

# Drive the unchanged integration target against both already-built harnesses.
# No compilation or deadline adjustment occurs in this timing window.
app_binary = json.loads(manifest.read_text())['ja_baseline']
root_binary = repo/'target/debug/kinewright-ja-baseline'
backup = out/'current-root-backup'; shutil.copy2(root_binary, backup)
root_sha = hashlib.file_digest(root_binary.open('rb'),'sha256').hexdigest()
def replace_copy(source, destination):
    # Replace the directory entry rather than overwriting a Cargo hard-link alias.
    temporary = destination.with_name(destination.name + '.r5-measure')
    assert not temporary.exists()
    shutil.copy2(source, temporary)
    os.replace(temporary, destination)
try:
    integrations = []
    for label, executable in [('before', old), ('after', backup)]:
        expected_hash = hashlib.file_digest(executable.open('rb'),'sha256').hexdigest()
        replace_copy(executable, root_binary)
        assert hashlib.file_digest(root_binary.open('rb'),'sha256').hexdigest() == expected_hash
        row, text = run(f'ja-baseline-{label}', ['nice','-n','10',app_binary,'--nocapture','--test-threads=4'], (0,101) if label == 'before' else 0)
        assert hashlib.file_digest(root_binary.open('rb'),'sha256').hexdigest() == expected_hash
        integrations.append({'label':label,'cli_sha256':expected_hash,'cli_bytes':root_binary.stat().st_size,'exit':row['exit'],'summaries':re.findall(r'^test result: .+$', text, re.M),'c4_setup':re.findall(r'R5_C4 .+',text),'controller_failure_present':'controller_failure_still_reaps_both_recorded_participants ... FAILED' in text,'run':row})
        print(json.dumps({k:v for k,v in integrations[-1].items() if k != 'run'}),flush=True)
finally:
    replace_copy(backup,root_binary); backup.unlink()
    assert hashlib.file_digest(root_binary.open('rb'),'sha256').hexdigest() == root_sha
with gzip.open(out/'baseline-pair.json.gz','wt') as stream: json.dump(integrations,stream,indent=2)
