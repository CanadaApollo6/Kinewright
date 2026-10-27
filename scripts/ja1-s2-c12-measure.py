"""Annotate and verify C12 paired timing; the shell starts every timed binary at native nice 10."""
import hashlib
import json
from pathlib import Path
import re
import runpy
import subprocess
import sys
import time
ROOT = Path(__file__).resolve().parent.parent
COMMON = runpy.run_path(str(ROOT / 'scripts/ja1-s2-c1-measure.py'))
def write(path, value): path.write_text(json.dumps(value, indent=2, sort_keys=True)+'\n')
def annotation():
    value = COMMON['annotation']()
    lines = value['top'].get('stdout', '').splitlines()
    heads = [i for i,line in enumerate(lines) if re.match(r'\s*PID\s+', line)]
    value['instant_top_five'] = lines[heads[-1]+1:heads[-1]+6] if heads else []
    return value
if sys.argv[1] == 'annotation': write(Path(sys.argv[2]), annotation())
elif sys.argv[1] == 'sample':
    root_pid = int(sys.argv[2]); output = Path(sys.argv[3]); observations = []
    previous = COMMON['process_ticks'](); previous_at = time.monotonic()
    while Path(f'/proc/{root_pid}').exists():
        time.sleep(5)
        current = COMMON['process_ticks'](); now = time.monotonic()
        external = []
        for pid, value in current.items():
            if pid not in previous: continue
            percent = 100 * (value['ticks']-previous[pid]['ticks']) / __import__('os').sysconf('SC_CLK_TCK') / (now-previous_at)
            if percent > 20 and not COMMON['is_workload'](pid, current, root_pid): external.append(dict(pid=pid, name=value['name'], cpu_percent=percent))
        observations.append(dict(monotonic=now, other_cpu_above_20=external))
        previous, previous_at = current, now
    write(output, observations)
elif sys.argv[1] == 'verify':
    directory, code = Path(sys.argv[2]), int(sys.argv[3])
    output = (directory/'stdout.log').read_text(); summaries = re.findall(r'^test result:.*$', output, re.M)
    expected = int(sys.argv[4])
    accepted = code == 0 and len(summaries) == 1 and f'{expected} passed; 0 failed; 0 ignored' in summaries[0]
    write(directory/'result.json', dict(exit=code, summaries=summaries, accepted=accepted))
    if not accepted: raise RuntimeError('timed work did not execute successfully')
    if (directory/'save-cost.json').exists():
        report = json.loads((directory/'save-cost.json').read_text())
        assert report['branches'] == 6 and len(report['seconds']) == 12 and report['repeated_comment_saves'] == 12
elif sys.argv[1] == 'summarize':
    import statistics
    root=Path(sys.argv[2]); data={}
    for side in ['A','B']:
        rows=[json.loads(p.read_text()) for p in sorted(root.glob(f'pair-*-{side}/save-cost.json'))]
        samples=[v*1000 for r in rows for v in r['seconds']]
        means=[statistics.mean(r['seconds'])*1000 for r in rows]
        data[side]=dict(lanes=len(rows), saves=len(samples), mean_ms=statistics.mean(samples), lane_means_ms=means, median_lane_mean_ms=statistics.median(means), lane_mean_range_ms=[min(means),max(means)], sync_fraction=sum(sum(r['sync_seconds']) for r in rows)/sum(sum(r['seconds']) for r in rows), history_decodes=sum(sum(r['history_decodes_per_save']) for r in rows))
    data['ratio_B_A']=data['B']['mean_ms']/data['A']['mean_ms']
    write(root/'summary.json',data)
