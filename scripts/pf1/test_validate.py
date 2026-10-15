"""Synthetic artifact witnesses; --mutation NAME changes only the named decision."""
import copy
import itertools
import fnmatch
import re
import os
import launch
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
import validate as v

MUTATIONS = {
    'schema': ("    if '$ref' in rule:", "    return\n    if '$ref' in rule:"),
    'duplicate': ("len(result) == len(items)", "True"),
    'nonfinite': ("require(False, 'nonfinite ' + x)", "0"),
    'identity': ("row['identity'] == dict(identity, pins=digest(pins_path), host=pins['host']['sha256'])", "True"),
    'fingerprint': ("row['fingerprint'] == fingerprint(row)", "True"),
    'logs': ("digest(path) == ref['sha256']", "True"),
    'adapter': ("gpu['selected'] == pins['gpu']['selected'] and gpu['stack'] == pins['gpu']['stack']", "True"),
    'policy': ("all(bounds[0] <= observation[key] <= bounds[1] for key, bounds in policy['requested'].items() if key != 'pstates')", "True"),
    'coverage': ("isinstance(telemetry, list) and telemetry[0]['timestamp_s'] <= row['started_s']", "True or isinstance(telemetry, list) and telemetry[0]['timestamp_s'] <= row['started_s']"),
    'spacing': ("all(5 <= b['timestamp_s'] - a['timestamp_s'] <= 6 for a, b in zip(samples, samples[1:]))", "True"),
    'busy': ("return all(s['utilization_pct'] >= 80 for s in samples) or any(not p['owned'] and (p['kind'] == 'C' or p['memory_bytes'] > 2147483648) for s in samples for p in s['processes'])", "return False"),
    'threshold': ("ratio <= limit)", "True)"),
    'paired': ("statistics.median(y / x for x, y in zip(a, b))", "statistics.median(b) / statistics.median(a)"),
    'incomplete': ("len(rows) == len(order)", "True"),
    'exit': ("any(r['exit'] != 0 for r in rows)", "False"),
    'environment': ("any(r['validity'] == 'environment_limited_at_load' for r in rows)", "False"),
    'contract': ("pins['lane'] in lanes.split() and protocol['name'] == name and set(inputs[pins['arms']['candidate']]) == workloads", "True"),
    'freshness': ("0 <= started_s - samples[-1]['timestamp_s'] <= 10", "True"),
    'strict_floor': ("pins['gate'] in ['G3', 'VM-D6'] or ", ""),
    'semantics': ("set(row['metrics']) == set(pins['metrics'])", "True"),
    'counter_registry': ("rules = COUNTERS", "rules = {k: (r['decision'], r['unit'], r['limit']) for k,r in pins['metrics'].items()}"),
    'rotation_seeds': ("len(seeds) == len(set(seeds)) == 3 and all(r['pair'] == i // 3 and r['seed'] == seeds[(i // 3) % 3] for i, r in enumerate(orders))", "True"),
    'rss_delta': ("all(y <= x+limit for x, y in zip(a, b))", "True"),
    'export_equal': ("'equal': lambda: a == b", "'equal': lambda: True"),
    'r65_small': ("abs(statistics.mean(b) - statistics.mean(a)) <= max(1, .25 * statistics.mean(a)) and max(min(a), min(b)) <= min(max(a), max(b))", "True"),
    'owned_cleanup': ('[signal.SIGTERM, signal.SIGKILL]', '[signal.SIGTERM]'),
    'r50': ("else 75 for x, y in zip(a, b)", "else 0 for x, y in zip(a, b)"),
}
if '--mutation' in sys.argv:
    index = sys.argv.index('--mutation'); name = sys.argv[index+1]; del sys.argv[index:index+2]
    old, new = MUTATIONS[name]
    target = launch if name == 'owned_cleanup' else v
    source = Path(target.__file__).read_text(); assert old in source
    exec(compile(source.replace(old, new), target.__file__, 'exec'), target.__dict__)

class Pins(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='kinewright-pf1-pins-'); self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name); self.p = self.root/'pins.json'
        repo = Path(__file__).resolve().parents[2]
        shutil.copytree(repo/'docs/reports/pf1-s4/example', self.root/'docs/reports/pf1-s4/example')
        self.pins = v.read(repo/'docs/PF1-PINS.json'); self.status = v.read(self.root/'docs/reports/pf1-s4/example/status.json')
    def run_record(self, expected, resign=True):
        self.p.write_text(json.dumps(self.pins), encoding='utf-8')
        for row in self.status['runs']:
            row['identity']['pins'] = v.digest(self.p)
            if resign: row['fingerprint'] = v.fingerprint(row)
        status = self.root/'status.json'; status.write_text(json.dumps(self.status), encoding='utf-8')
        try: code = v.evaluate(self.p, status, self.root)
        except (ValueError, OSError, KeyError, TypeError): code = 2
        self.assertEqual(code, expected)
    def test_schema(self):
        for change in [lambda p: p.pop('host'), lambda p: p.update(extra=1), lambda p: p['gpu']['selected'].update(vendor_id=True), lambda p: p['gpu']['selected'].update(driver={'status':'unavailable','reason':'no'}), lambda p: p['protocol'].update(poll_ms=-1), lambda p: p['gpu']['selected'].update(name=''), lambda p: p['protocol'].update(sample_ms=float('inf'))]:
            p = copy.deepcopy(self.pins); change(p)
            with self.subTest(p=p), self.assertRaises(ValueError): v.schema(p, v.SCHEMA['$defs']['pins'])
        with self.assertRaises(ValueError): v.schema({'s4':{}}, v.SCHEMA['$defs']['builds'])
    def test_duplicate(self):
        self.p.write_text('{"x":1,"x":2}')
        with self.assertRaises(ValueError): v.read(self.p)
    def test_nonfinite(self):
        self.p.write_text('{"x":NaN}')
        with self.assertRaises(ValueError): v.read(self.p)
    def test_identity(self): self.status['runs'][0]['identity']['binary'] = 'f'*64; self.run_record(2)
    def test_fingerprint(self): self.run_record(2, resign=False)
    def test_logs(self):
        for row in self.status['runs']: row['logs'] = [{'path':'changed.log','sha256':'f'*64}]
        (self.root/'changed.log').write_text('changed raw bytes'); self.run_record(2)
    def test_adapter(self): self.status['runs'][0]['gpu']['selected']['name'] = 'other'; self.run_record(2)
    def test_policy(self): self.status['runs'][0]['gpu']['policy']['observed']['graphics_mhz'] = 999; self.run_record(2)
    def test_coverage(self): self.status['runs'][0]['gpu']['telemetry'].pop(); self.run_record(2)
    def test_spacing(self): self.status['runs'][0]['preflight'][1]['timestamp_s'] += 1; self.run_record(2)
    def test_busy(self): self.status['runs'][0]['preflight'][0]['processes'][0]['kind'] = 'C'; self.run_record(75)
    def test_threshold(self): self.status['runs'][1]['metrics']['mean_ms'] = 200; self.status['runs'][3]['metrics']['mean_ms'] = 200; self.run_record(1)
    def test_paired(self):
        for row, value in zip(self.status['runs'], [1,2,10,20,100,10]): row['metrics']['mean_ms'] = value
        self.run_record(1)
    def test_incomplete(self): self.status['runs'].pop(); self.run_record(2)
    def test_exit(self): self.status['runs'][0]['exit'] = 1; self.run_record(101)
    def test_environment(self): self.status['runs'][0]['validity'] = 'environment_limited_at_load'; self.run_record(75)
    def test_green_and_cli(self):
        self.run_record(0)
        command = [sys.executable, str(Path(v.__file__)), '--pins', str(self.p), '--run', str(self.root/'status.json'), '--root', str(self.root)]
        self.assertEqual(subprocess.run(command, capture_output=True).returncode, 0)

    def configure(self, gate):
        lanes, protocol, workloads, parameters = v.CONTRACTS[gate]
        self.pins.update(gate=gate, lane=lanes.split()[0])
        self.status.update(gate=gate, lane=self.pins['lane'])
        builds = v.read(self.root/self.pins['builds']['path'])
        template = next(iter(v.read(self.root/self.pins['inputs']['path']).values()))['typical_1080p']
        bases = ['43b2625', '62a29b1', 'accepted-S3b'] if gate.startswith('R65-') else ['S0' if gate in ['G15', 'G18', 'VM-D6'] else 'accepted-S3b', 'candidate']
        if gate == 'G7a':
            self.pins['arms'].pop('reference')
            builds.pop('s3b')
        if gate.startswith('R65-'):
            builds['historical'] = copy.deepcopy(builds['s4'])
            self.pins['arms'] = dict(zip(['reference', 'candidate'], {'R65-historical':['s3b','s4'], 'R65-intervening':['s4','historical'], 'R65-cumulative':['s3b','historical']}[gate]))
        def put(path, value):
            file = self.root/path
            file.write_text(json.dumps(value), encoding='utf-8')
            return {'path':path, 'sha256':v.digest(file)}
        for (arm, build), basis in zip(builds.items(), bases):
            build['basis'] = basis
            approval = v.read(self.root/build['approval']['path'])
            approval['basis'] = basis
            build['approval'] = put(arm+'-approval.json', approval)
        self.pins['builds'] = put('builds.json', builds)
        self.pins['inputs'] = put('inputs.json', {a:{w:template for w in sorted(workloads)} for a in builds})
        rules = v.COUNTERS if gate.startswith('R65-') else v.RULES.get(gate, {'fps':('each_ge','fps',0)})
        self.pins['metrics'] = {k:dict(decision=d, unit=u, limit=l, scope='paired_median' if d in ['ratio','R65'] else 'invariant' if d in ['equal','zero','invariant'] else 'each_run') for k,(d,u,l) in rules.items()}
        order = []
        for workload in sorted(workloads):
            for rotation, arms in enumerate(itertools.permutations(builds) if gate.startswith('R65-') else [tuple(builds)]):
                for seed_index, seed in enumerate([11,22,33] if gate != 'G7a' else [11]):
                    order.extend(dict(workload=workload, pair=rotation*3+seed_index, arm=arm, seed=seed) for arm in arms)
        self.pins['protocol'] = dict(name=protocol, poll_ms=5, sample_ms=100, runs=18 if gate.startswith('R65-') else 1 if gate=='G7a' else 3, order=order, **parameters)
        row = self.status['runs'][0]
        self.status['runs'] = [dict(copy.deepcopy(row), **entry, metrics={k:'a'*64 if u=='sha256' else 0 if d in ['zero','R50_abs'] else 100 for k,(d,u,l) in rules.items()}) for entry in order]
        if self.pins['lane'] != 'LH':
            self.pins['gpu']['policy'] = {'kind':'software'}
            for row in self.status['runs']: row['gpu']['policy'] = {'kind':'software'}

    def test_contract(self):
        for gate in ['I4','G18','G15','G3','G4','G2','VM-D6','G7a']:
            self.setUp(); self.configure(gate); self.run_record(0)
            self.pins['protocol']['name'] = 'D13'; self.run_record(2)
            self.pins['protocol']['name'] = v.CONTRACTS[gate][1]
            self.pins['lane'] = 'CI-W' if gate != 'G7a' else 'LL'; self.run_record(2)
        self.setUp(); self.configure('G15'); self.pins['protocol'].pop('children'); self.run_record(2)

    def test_freshness(self):
        for offset in [-86400,86400]:
            self.setUp()
            for sample in self.status['runs'][0]['preflight']: sample['timestamp_s'] += offset
            self.run_record(2)

    def test_export_equal(self):
        self.configure('G18'); self.status['runs'][1]['metrics']['output_sha256']='b'*64; self.run_record(1)
    def test_export_time(self):
        self.configure('G18')
        for row in self.status['runs']:
            if row['arm']=='s4': row['metrics']['wall_ms']=106
        self.run_record(1)
        self.status['runs'].pop(); self.run_record(2)
    def test_rss_delta(self):
        self.configure('G15'); self.status['runs'][-1]['metrics']['settled_bytes']+=4194305; self.run_record(1)
    def test_export_incomplete(self):
        self.configure('G18'); self.status['runs'].pop(); self.run_record(2)
    def test_strict_floor(self):
        for gate, floor in [('G3',60),('VM-D6',20)]:
            self.setUp(); self.configure(gate)
            for row in self.status['runs']: row['metrics']['fps']=floor-1
            self.run_record(1)
            for row in self.status['runs']:
                if row['arm']=='s3b': row['metrics']['fps']=floor
            self.run_record(1)
    def test_semantics(self):
        self.status['runs'][0]['preflight'][0]['processes'][0]['kind']='C'
        self.status['runs'][-1]['metrics']['extra']=42; self.run_record(2)
        self.status['runs'][0]['preflight'][0]['processes'][0]['kind']='C+G'
        self.status['runs'][0]['exit']=101; self.status['runs'][-1]['metrics']={'mean_ms':-1}; self.run_record(2)
    def test_counter_registry(self):
        for change in ['omit','downgrade']:
            self.setUp(); self.configure('R65-historical'); self.run_record(0)
            if change=='omit': self.pins['metrics'].pop('timeouts')
            else: self.pins['metrics']['timeouts'].update(decision='R65',scope='paired_median')
            self.run_record(2)
    def test_rotation_seeds(self):
        self.configure('R65-historical')
        for row in self.pins['protocol']['order']+self.status['runs']: row['seed']=11
        self.run_record(2)
    def test_r65_small(self):
        self.configure('R65-intervening')
        for candidate, reference, expected in [([11]*3,[10]*3,1),([10]*3,[0,0,10],1),([7,10,13],[4,10,16],0),([106]*3,[100]*3,1),([105]*3,[100]*3,0)]:
            for row in self.status['runs']: row['metrics']['admission_waits']=(candidate if row['arm']=='historical' else reference)[row['pair']%3]
            self.run_record(expected)
        self.status['runs'][0]['metrics']['timeouts']=1; self.run_record(1)
        self.setUp(); self.configure('R65-cumulative'); self.run_record(75)
    def test_r50(self):
        self.configure('G4')
        for reference,candidate,expected in [(30,30,0),(30,10,1),(10,10,75),(10,9,1)]:
            for row in self.status['runs']: row['metrics']['fps']=reference if row['arm']=='s3b' else candidate
            self.run_record(expected)

    def test_checkout(self):
        repo = Path(__file__).resolve().parents[2]
        paths = ['docs/PF1-PINS.json','docs/PF1-PINS.schema.json'] + [str(p.relative_to(repo)) for p in (repo/'docs/reports/pf1-s4/example').iterdir()]
        attrs = subprocess.check_output(['git','check-attr','eol','--']+paths, cwd=repo, text=True)
        self.assertTrue(all(line.endswith(': lf') for line in attrs.splitlines()))
        subprocess.run(['git','-c','core.autocrlf=true','checkout-index','--all','--prefix='+str(self.root/'checkout')+'/'], cwd=repo, check=True, capture_output=True)
        for path in paths: self.assertEqual(subprocess.check_output(['git','show',':'+path], cwd=repo), (self.root/'checkout'/path).read_bytes())
    def test_ci_trigger(self):
        workflow = (Path(__file__).resolve().parents[2]/'.github/workflows/ci.yml').read_text()
        ignores = re.findall(r"^      - '([^']+)'", workflow, re.M)
        for path in ['docs/PF1-PINS.schema.json','docs/PF1-PINS.json','docs/reports/pf1-s4/example/synthetic.txt']:
            self.assertFalse(any(fnmatch.fnmatchcase(path, pattern) for pattern in ignores))
    def test_workloads(self):
        for gate in ['G15','G3']:
            self.setUp(); self.configure(gate)
            path=self.root/self.pins['inputs']['path']; inputs=v.read(path)
            for arm in inputs: inputs[arm]={'typical_1080p':next(iter(inputs[arm].values()))}
            path.write_text(json.dumps(inputs)); self.pins['inputs']['sha256']=v.digest(path)
            for container in [self.pins['protocol']['order'], self.status['runs']]:
                first = container[0]['workload']
                container[:] = [r for r in container if r['workload']==first]
                for row in container: row['workload']='typical_1080p'
            self.run_record(2)

    @unittest.skipUnless(os.name=='posix', 'Linux process-group launcher')
    def test_owned_cleanup(self):
        for script in ["import signal,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); print('ready',flush=True); time.sleep(60)", "print('ready',flush=True)"]:
            with self.assertRaisesRegex(ValueError, 'invalid readback'):
                with launch.owned_child([sys.executable,'-c',script], stdout=subprocess.PIPE, text=True) as child:
                    self.assertEqual(child.stdout.readline().strip(), 'ready')
                    raise ValueError('invalid readback')
            self.assertIsNotNone(child.returncode)
            child.stdout.close()

if __name__ == '__main__': unittest.main()
