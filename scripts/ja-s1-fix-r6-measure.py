"""Time one already-built history reader; retain source, binary and shared-load evidence."""
from pathlib import Path
import gzip
import hashlib
import json
import os
import subprocess
import sys
import threading
import time

assert os.getpriority(os.PRIO_PROCESS, 0) == 19
out = Path(sys.argv[1]).resolve()
out.mkdir(parents=True, exist_ok=True)
binary = Path('target/debug/examples/ja_s1_history_read').resolve()
scratch = out / 'scratch'
scratch.mkdir(exist_ok=True)
def read(args):
    try:
        return subprocess.check_output(args, text=True, stderr=subprocess.STDOUT).strip()
    except (OSError, subprocess.CalledProcessError) as error:
        return str(error)
def annotation():
    top = read(['top', '-b', '-n', '2', '-d', '0.2', '-w', '160']).splitlines()
    headers = [i for i, line in enumerate(top) if line.lstrip().startswith('PID ')]
    return {'uptime': read(['uptime']), 'top_five_cpu': top[headers[-1]:headers[-1]+6] if headers else top[-6:],
            'gpu': read(['nvidia-smi', '--query-gpu=pstate,clocks.current.graphics,clocks.current.memory,utilization.gpu', '--format=csv,noheader'])}
def snapshot():
    rows = {}
    for path in Path('/proc').iterdir():
        if not path.name.isdigit():
            continue
        try:
            text = (path/'stat').read_text()
            end = text.rfind(')')
            fields = text[end+2:].split()
            rows[int(path.name)] = (int(fields[11])+int(fields[12]), text[text.index('(')+1:end])
        except (OSError, ValueError, IndexError):
            pass
    return rows
stop = threading.Event()
samples = []
def sample():
    previous = snapshot()
    started = time.monotonic()
    while not stop.wait(5):
        current = snapshot()
        elapsed = time.monotonic()-started
        busy = []
        for pid, (cpu, name) in current.items():
            if pid in previous:
                percent = (cpu-previous[pid][0])/os.sysconf('SC_CLK_TCK')/elapsed*100
                if percent > 20:
                    busy.append({'pid': pid, 'name': name, 'cpu_percent': percent})
        samples.append({'at': time.time(), 'above_20_percent': busy})
        previous = current
        started = time.monotonic()
identity = {'head': read(['git', 'rev-parse', 'HEAD']), 'diff_sha256': hashlib.sha256(subprocess.check_output(['git','diff','--binary'])).hexdigest(),
            'binary_sha256': hashlib.file_digest(binary.open('rb'),'sha256').hexdigest(), 'compiler': read(['rustc','-Vv']),
            'sources': {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in Path('crates').rglob('*.rs') if 'target' not in p.parts},
            'profile': 'debug; system allocator; already built; no concurrent builds/tests'}
before = annotation()
thread = threading.Thread(target=sample)
thread.start()
started = time.monotonic()
try:
    process = subprocess.run(['timeout','600','nice','-n','10',str(binary),str(scratch)], capture_output=True, text=True)
finally:
    stop.set()
    thread.join()
after = annotation()
with gzip.open(out/'history-read.log.gz', 'wt') as stream:
    stream.write(json.dumps(identity)+'\n'+process.stdout+process.stderr)
with gzip.open(out/'telemetry.json.gz','wt') as stream:
    json.dump({'before':before,'after':after,'load_samples':samples,'exit':process.returncode,'elapsed_seconds':time.monotonic()-started},stream,indent=2)
print(process.stdout+process.stderr, flush=True)
raise SystemExit(process.returncode)
