#!/usr/bin/env python3
"""Five alternating lanes; run under the Cargo-heavy lock after release builds finish.

Only the standalone executor / synchronous raw Core request is timed. This is
not a GUI, writer, store publication, media, or full application measurement.
"""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time

binary=Path(sys.argv[1]).resolve()
output=Path(sys.argv[2]);output.mkdir(parents=True,exist_ok=True)
checkpoints=sys.argv[3] if len(sys.argv)>3 else '4'

def command(args):
    try:
        result=subprocess.run(args,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True,timeout=10,check=False)
        return {'status':result.returncode,'output':result.stdout.strip()}
    except (OSError,subprocess.TimeoutExpired) as error:
        return {'error':str(error)}

def state():
    before=ticks()
    result={'utc':time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime()),'uptime':command(['uptime']),
            'cores':os.cpu_count(),'nproc':command(['nproc']),'top5_ps_cpu':command(['ps','-eo','pid,pcpu,comm','--sort=-pcpu'])['output'].splitlines()[:6],
            'gpu':command(['nvidia-smi','--query-gpu=name,pstate,clocks.current.graphics,clocks.current.memory','--format=csv,noheader'])}
    after=ticks()
    result['instantaneous_top5']=usage(before,after)[:5]
    result['cpu_sample_interval_s']=after[0]-before[0]
    return result

def ticks():
    result={}
    for entry in Path('/proc').iterdir():
        if not entry.name.isdigit(): continue
        try:
            value=(entry/'stat').read_text()
            name=value[value.index('(')+1:value.rindex(')')]
            fields=value[value.rindex(')')+2:].split()
            result[int(entry.name)]=(int(fields[11])+int(fields[12]),name)
        except (OSError,ValueError,IndexError): pass
    return time.monotonic(),result

def usage(previous,current):
    interval=current[0]-previous[0]
    active=[]
    for pid,(value,name) in current[1].items():
        if pid in previous[1]:
            cpu=100*(value-previous[1][pid][0])/os.sysconf('SC_CLK_TCK')/interval
            active.append({'pid':pid,'cpu':round(cpu,2),'name':name})
    active.sort(key=lambda p:p['cpu'],reverse=True)
    return active

revision=command(['git','rev-parse','HEAD'])
metadata={'claim':'Report raw Core versus standalone operation history; test history-length dependence, replay latency and bounded memory. No end-to-end product speedup is claimed.',
          'binary':str(binary),'sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'revision':revision,'lanes':[]}
for pair in range(5):
    # Keep the two implementations interleaved throughout the run.
    for side in ['old','new']:
        lane={'pair':pair+1,'side':side,'start':state(),'contamination':[],'instantaneous_top5':[]}
        previous=ticks()
        start=time.monotonic()
        with (output/f'{pair+1}-{side}.stderr').open('w') as errors:
            child=subprocess.Popen(['timeout','600',str(binary),side,checkpoints],stdout=subprocess.PIPE,stderr=errors,text=True)
            while True:
                try:
                    stdout,_=child.communicate(timeout=5)
                    break
                except subprocess.TimeoutExpired:
                    current=ticks()
                    active=usage(previous,current)
                    lane['instantaneous_top5'].append({'elapsed':round(time.monotonic()-start,3),'processes':active[:5]})
                    # The benchmark is one thread plus the old actor. Identify the
                    # actual executable by /proc/exe; never signal any other process.
                    for item in active:
                        if item['cpu']<=20 or item['pid']==os.getpid(): continue
                        try: own=Path(f"/proc/{item['pid']}/exe").resolve()==binary
                        except OSError: own=False
                        if not own: lane['contamination'].append(item)
                    previous=current
        lane['elapsed_s']=time.monotonic()-start
        lane['exit']=child.returncode
        lane['end']=state()
        (output/f'{pair+1}-{side}.stdout').write_text(stdout)
        if child.returncode==0:
            lane['result']=json.loads(stdout)
            assert lane['result']['errors']==0
        metadata['lanes'].append(lane)
        (output/'runs.json').write_text(json.dumps(metadata,indent=2)+'\n')
        print(f"pair {pair+1} {side}: exit={child.returncode} wall={lane['elapsed_s']:.2f}s",flush=True)
        if child.returncode!=0: sys.exit(child.returncode)
