"""Sample only an owned fixture process tree; never change or signal any process.

The executable read intervals identify debug SHA work. Events identify header and
participant readiness. Shell wrappers separately time header's external probes.
"""
from pathlib import Path
import argparse
import gzip
import json
import os
import time

parser = argparse.ArgumentParser()
parser.add_argument('pid', type=int)
parser.add_argument('binary', type=Path)
parser.add_argument('output', type=Path)
parser.add_argument('report', type=Path)
args = parser.parse_args()
assert os.getpriority(os.PRIO_PROCESS, 0) == 19
binary = args.binary.resolve()
start = time.monotonic()
rows = {}
seen = {args.pid}
while time.monotonic() - start < 180:
    live = set()
    queue = [args.pid]
    while queue:
        pid = queue.pop()
        if pid in live:
            continue
        proc = Path('/proc') / str(pid)
        try:
            stat = (proc/'stat').read_text().split(') ', 1)[1].split()
            if stat[0] == 'Z':
                continue
            live.add(pid)
            for task in (proc/'task').iterdir():
                queue.extend(map(int, (task/'children').read_text().split()))
            now = time.monotonic() - start
            command = (proc/'cmdline').read_bytes().split(b'\0')[:12]
            row = rows.setdefault(str(pid), {'command': [x.decode(errors='replace')[:256] for x in command], 'start_s':now, 'hash_read_start_s':None, 'hash_read_end_s':None, 'max_hash_position':0})
            row['end_s'] = now
            row['cpu_s'] = (int(stat[11]) + int(stat[12])) / os.sysconf('SC_CLK_TCK')
            for fd in (proc/'fd').iterdir():
                try:
                    if fd.resolve() == binary:
                        info = (proc/'fdinfo'/fd.name).read_text()
                        position = int(next(x.split()[1] for x in info.splitlines() if x.startswith('pos:')))
                        if row['hash_read_start_s'] is None:
                            row['hash_read_start_s'] = now
                        row['hash_read_end_s'] = now
                        row['max_hash_position'] = max(row['max_hash_position'], position)
                except (OSError, StopIteration):
                    pass
        except (OSError, ValueError):
            continue
    seen.update(live)
    if not live:
        break
    time.sleep(.02)
events = []
event_file = args.report/'events.jsonl'
if event_file.exists():
    with event_file.open() as stream:
        lines = stream.read(1048576).splitlines()
    for line in lines:
        try:
            event = json.loads(line)
        except ValueError:
            continue
        if event.get('event') in ('captured_input', 'process_spawn') or event.get('kind') in ('captured_input', 'process_spawn'):
            events.append(event)
result = {'sample_interval_ms':20, 'elapsed_s':time.monotonic()-start, 'binary_bytes':binary.stat().st_size, 'processes':rows, 'setup_events':events[:32]}
probe_file = args.output.parent/'probes.jsonl'
probes = []
if probe_file.exists():
    with probe_file.open() as stream:
        for line in stream.read(65536).splitlines():
            probe = json.loads(line)
            probes.append({'probe':probe['probe'], 'elapsed_ms':(probe['end_ns']-probe['start_ns'])/1e6, 'exit':probe['exit']})
result['probes'] = probes[:32]
with gzip.open(args.output, 'wt') as output:
    json.dump(result, output, indent=2)
def phase(row):
    command=row['command']
    if '--participant' in command:
        return 'worker '+command[command.index('--participant')+1]
    return 'header' if '--role' in command and command[command.index('--role')+1]=='fixture' else Path(command[0]).name

hashes = [{'pid':pid,'phase':phase(row),'start_ms':1000*row['hash_read_start_s'],'read_ms':1000*(row['hash_read_end_s']-row['hash_read_start_s']), 'bytes':row['max_hash_position']} for pid,row in rows.items() if row['hash_read_start_s'] is not None]
slow = [{'pid':pid,'process':Path(row['command'][0]).name,'observed_ms':1000*(row['end_s']-row['start_s'])} for pid,row in rows.items() if row['hash_read_start_s'] is None and row['end_s']-row['start_s']>.1][:16]
print(json.dumps({'setup_profile':str(args.output), 'elapsed_s':result['elapsed_s'], 'executable_reads':hashes,'probes':result['probes'],'other_slow_processes':slow}))
