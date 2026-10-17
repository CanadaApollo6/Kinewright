"""Linux wrapper integration with synthetic pins and a real publishing child."""
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import unittest

import validate as v
import launch

CHILD = '''#!/usr/bin/env python3
import hashlib, json, os, signal, sys, time
from pathlib import Path
case = os.environ['PF1_FIXTURE_CASE']
print('child_pid=' + str(os.getpid()), flush=True)
print('native_nice=' + str(os.getpriority(os.PRIO_PROCESS, 0)), flush=True)
print('seed=' + os.environ['PF1_SEED'], flush=True)
signal.signal(signal.SIGTERM, signal.SIG_IGN)
record = json.loads(Path(os.environ['PF1_FIXTURE']).read_text())
inputs = json.loads(Path(os.environ['PF1_FIXTURE_INPUTS']).read_text())
row = json.loads(Path(os.environ['PF1_PINS']).read_text())['protocol']['order'][int(os.environ['PF1_ROW'])]
value = inputs[row['arm']][row['workload']]
record['consumed'] = {key: value[key]['sha256'] for key in ['workload', 'media', 'document']}
record['consumed']['workload'] = hashlib.sha256(Path(os.environ['PF1_FROZEN_INPUT']).read_bytes()).hexdigest()
if case == 'unconsumed':
    record.pop('consumed')
if case == 'substituted':
    record['consumed']['document'] = 'f' * 64
record.update(token=os.environ['PF1_TOKEN'], timestamp_s=time.time())
path = Path(os.environ['PF1_RECORD'])
def publish():
    temporary = path.with_suffix('.tmp')
    temporary.write_text(json.dumps(record))
    temporary.replace(path)
if case == 'startup':
    marker = path.with_name(path.name.replace('-collector.json', '-owned-pids.json'))
    deadline = time.monotonic() + 30
    while not marker.exists() and time.monotonic() < deadline: time.sleep(.01)
    record.pop('consumed')
    publish()
    load = path.parent/'logs'/path.name.replace('-collector.json', '-load.jsonl')
    while len(load.read_text().splitlines()) < 3 and time.monotonic() < deadline: time.sleep(.01)
    sys.exit(101)
if case == 'missing-live':
    sys.exit(0)
if case == 'invalid':
    record['selected']['vendor_id'] += 1
publish()
if case == 'invalid':
    time.sleep(60)
    sys.exit(0)
if case == 'assertion':
    print('synthetic assertion failed', flush=True)
    sys.exit(101)
if case != 'no-final':
    fps = 10 if case == 'shared' or case == 'candidate' and os.environ['S3B_SIDE'] == 'candidate' else 30
    record.update(timestamp_s=time.time(), valid=case != 'invalid-final', metrics=dict(fps=fps))
    publish()
'''

MUTATIONS = {
    'late_alarm': ('lane.py', ' row=None\n', ''),
    'startup_epoch': ('lane.py', '  started=time.time()\n  if lane', '  pass\n  if lane'),
    'startup_readiness': ('launch.py', "'metrics' in record or 'valid' in record", 'True'),
    'priority': ('lane.py', 'os.setpriority(os.PRIO_PROCESS,0,19)', 'pass'),
    'consumption': ('launch.py', "record.get('consumed') == expected", 'True'),
    'publication': ('lane.py', "pins.require(collected is not None,'missing final collector record')", "collected = collected or (json.loads(Path(env['PF1_FIXTURE']).read_text()) | dict(valid=True,metrics=dict(fps=30)),spec['gpu'])"),
    'final': ('lane.py', "pins.require('metrics' in raw and type(raw.get('valid')) is bool,'missing final measurements')", "raw.update(valid=True,metrics=raw.get('metrics',dict(fps=30)))"),
    'receipt': ('lane.py', "receipt=receipt_ref)", "receipt=None)"),
    'assertion': ('lane.py', "code=101 if process is not None and process.returncode==101 and cleanup.get('reaped') else 75 if code==75 else 2", "code=2"),
    'readback': ('launch.py', "v.gpu_check(gpu, pins, host, root)", "pass"),
    'kill': ('launch.py', '[signal.SIGTERM, signal.SIGKILL]', '[signal.SIGTERM]'),
    'validity': ('lane.py', "pins.require(raw['valid'],'collector correctness/validity failed')", 'pass'),
    'failure_pins': ('validate.py', "receipt['pins'] == digest(pins_path)", 'True'),
    'failure_order': ('validate.py', "all(receipt[k] == x for k, x in pins['protocol']['order'][len(status['runs'])].items())", 'True'),
    'host': ('launch.py', "all(host[key] == value for key, value in actual.items())", 'True'),
    'dispatch': ('launch.py', "test == TESTS[spec['gate']]", 'True'),
    'seed': ('final-measure.py', "PF1_SEED=str(entry['seed'])", "PF1_SEED=str(entry['pair'])"),
    'unsealed': ('final-measure.py', "v.require(journal.with_name(journal.name.replace('attempts-','status-').replace('.jsonl','.json')).is_file(),", "v.require(True,"),
}
mutation = None
if '--mutation' in sys.argv:
    index = sys.argv.index('--mutation')
    mutation = sys.argv[index + 1]
    del sys.argv[index:index + 2]

import test_validate as fixtures


@unittest.skipUnless(sys.platform == 'linux', 'Linux wrapper integration')
class Wrapper(unittest.TestCase):
    def test_final_alarm(self):
        lane = self.runners/'lane.py'
        lane.write_text(lane.read_text().replace('\nexcept (ValueError,', "\n if env['PF1_ROW']=='23': os.kill(os.getpid(),signal.SIGALRM)\nexcept (ValueError,"))
        self.pipeline('alarm', 2, 23)
        status = v.read(self.runners/'series-alarm/status-0.json')
        self.assertEqual(len(status['runs']), 23)
        self.assertEqual(v.read(self.root/status['failure']['receipt']['path'])['reason'], 'complete invocation deadline')

    def test_startup(self):
        lane = self.runners/'lane.py'
        lane.write_text(lane.read_text().replace('def sample():', 'def sample():\n global started\n if process is None: started-=11'))
        self.pipeline('startup', 101)

    def test_priority(self):
        self.pipeline('priority', 0)
        expected = min(19, os.getpriority(os.PRIO_PROCESS, 0) + 10)
        for path in (self.runners/'series-priority/logs').glob('*.log'):
            self.assertIn('native_nice=' + str(expected), path.read_text())
            samples = path.with_name(path.stem + '-load.jsonl')
            self.assertEqual(json.loads(samples.read_text().splitlines()[-1])['monitor_nice'], 19)

    def test_frozen_consumption(self):
        self.pipeline('unconsumed', 2)
        self.pipeline('substituted', 2)

    def setUp(self):
        self.fixture = fixtures.Pins()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.fixture.configure('G4')
        self.fixture.freeze_inputs()
        self.fixture.run_record(0)
        self.temp = tempfile.TemporaryDirectory(prefix='kinewright-s4c-wrapper-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        shutil.copytree(self.fixture.root, self.root, dirs_exist_ok=True)
        self.runners = self.root / 'runners'
        self.runners.mkdir()
        source = Path(os.environ['PF1_RUNNERS'])
        for name in ['lane.py', 'final-measure.py', 'final-figures.py']:
            shutil.copyfile(source / name, self.runners / name)
        repo = Path(__file__).resolve().parents[2]
        modules = self.root / 'repo/scripts/pf1'
        modules.mkdir(parents=True)
        for name in ['launch.py', 'validate.py', 'series.py']:
            shutil.copyfile(repo / 'scripts/pf1' / name, modules / name)
        (self.root / 'repo/docs').mkdir()
        shutil.copyfile(repo / 'docs/PF1-PINS.schema.json', self.root / 'repo/docs/PF1-PINS.schema.json')
        if mutation:
            name, old, new = MUTATIONS[mutation]
            path = (self.runners if name in ['lane.py', 'final-measure.py'] else modules) / name
            source = path.read_text()
            self.assertIn(old, source)
            path.write_text(source.replace(old, new))
        self.child = self.root / 'publisher'
        self.child.write_text(CHILD)
        self.child.chmod(0o755)
        self.pins = v.read(self.root / 'pins.json')
        host_path = self.root / self.pins['host']['path']
        host_path.write_text(json.dumps(v.read(host_path) | launch.host_readback()))
        self.pins['host']['sha256'] = v.digest(host_path)
        builds_path = self.root / self.pins['builds']['path']
        builds = v.read(builds_path)
        for build in builds.values():
            build['binary'] = dict(path='publisher', sha256=v.digest(self.child))
        builds_path.write_text(json.dumps(builds))
        self.pins['builds']['sha256'] = v.digest(builds_path)
        icd = self.root / 'icd.json'
        icd.write_text('synthetic ICD, no GPU used')
        raw = dict(selected=dict(backend='Vulkan', name='llvmpipe LLVM 20.1.5', vendor_id=65541, device_id=0, driver='llvmpipe'), driver_info='Mesa 25.1.0', software=True, protocol=dict(name='native-display', poll_ms=5, warmup_ms=2000, duration_s=60))
        self.pins['gpu'] = dict(selected=raw['selected'], stack=dict(runtime=raw['driver_info'], mesa='25.1.0', llvm='20.1.5', icd_manifest_sha256=v.digest(icd)), policy=dict(kind='software'), telemetry=dict(status='unavailable', reason='software adapter has no hardware clocks'))
        (self.root / 'pins.json').write_text(json.dumps(self.pins))
        (self.root / 'fixture.json').write_text(json.dumps(raw))
        tools = self.root / 'bin'
        tools.mkdir()
        nvidia = tools / 'nvidia-smi'
        nvidia.write_text('#!/bin/sh\nprintf "synthetic GPU inventory\\n"\n')
        nvidia.chmod(0o755)
        self.env = dict(os.environ, PF1_REPO=str(self.root / 'repo'), PF1_ROOT=str(self.root), PF1_FIXTURE=str(self.root / 'fixture.json'), PF1_FIXTURE_INPUTS=str(self.root / self.pins['inputs']['path']), VK_DRIVER_FILES=str(icd), PATH=str(tools) + os.pathsep + os.environ['PATH'])

    def pipeline(self, case, expected, failed_ordinal=0):
        self.env['PF1_FIXTURE_CASE'] = case
        plan = dict(id=case, gates=[dict(pins=dict(path='pins.json', sha256=v.digest(self.root / 'pins.json')), test=launch.TESTS['G4'], series={})])
        plan_path = self.root / (case + '-plan.json')
        plan_path.write_text(json.dumps(plan))
        work = self.runners / ('series-' + case)
        try:
            for name in ['final-measure.py', 'final-figures.py']:
                result = subprocess.run([sys.executable, str(self.runners / name), str(plan_path)], env=self.env, capture_output=True, text=True, timeout=180)
                (self.root / (case + '-' + name + '.log')).write_text(result.stdout + result.stderr)
                self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
            status = v.read(work / 'status-0.json')
            for ref in status['artifacts']:
                v.artifact(self.root, ref)
            journal = (work / 'attempts-0.jsonl').read_text().splitlines()
            self.assertEqual(len(journal), failed_ordinal+1 if expected in [2, 101] else 24)
            for index, line in enumerate(journal):
                row = json.loads(line)['row']
                failed = expected in [2, 101] and index == failed_ordinal
                if failed:
                    self.assertIn('receipt', row)
                    receipt_ref = row['receipt']
                    v.artifact(self.root, receipt_ref)
                else:
                    self.assertEqual(row['fingerprint'], v.fingerprint(row))
                    self.assertEqual(row['environment']['PF1_SEED'], str(row['seed']))
                    self.assertIn('--exact', row['command'])
                    receipt_ref = row['logs'][-1]
                receipt = v.read(self.root / receipt_ref['path'])
                self.assertEqual(receipt['pins'], plan['gates'][0]['pins']['sha256'])
                self.assertTrue(receipt['cleanup']['reaped'])
                self.assertEqual(receipt['exit'], expected if failed else 0)
                with self.assertRaises(ProcessLookupError):
                    os.kill(receipt['cleanup']['pid'], 0)
                for ref in receipt['logs']:
                    v.artifact(self.root, ref)
                if case == 'invalid':
                    self.assertIn('SIGKILL', receipt['cleanup']['signals'])
                    self.assertIn('adapter/stack mismatch', receipt['reason'])
            result = subprocess.run([sys.executable, str(self.runners / 'final-measure.py'), str(plan_path)], env=self.env, capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
            self.assertEqual((work / 'attempts-0.jsonl').read_text().splitlines(), journal)
            if expected == 101:
                ref = status['failure']['receipt']
                path = self.root / ref['path']
                shutil.copyfile(path, path.with_name(path.name + '.original'))
                shutil.copyfile(work / 'status-0.json', work / 'status-0.json.original')
                original = v.read(path)
                for key, value in [('pins', 'f' * 64), ('seed', -1)]:
                    receipt = dict(original, **{key: value})
                    path.write_text(json.dumps(receipt))
                    ref['sha256'] = v.digest(path)
                    for artifact in status['artifacts']:
                        if artifact['path'] == ref['path']:
                            artifact['sha256'] = ref['sha256']
                    (work / 'status-0.json').write_text(json.dumps(status))
                    result = subprocess.run([sys.executable, str(self.runners / 'final-figures.py'), str(plan_path)], env=self.env, capture_output=True, text=True, timeout=10)
                    self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
            if expected == 0:
                row = status['runs'][0]
                self.assertEqual(row['metrics'], dict(fps=30))
                self.assertEqual(len(status['runs']), 24)
                log = self.root / row['logs'][0]['path']
                shutil.copyfile(log, log.with_name(log.name + '.original'))
                log.write_text(log.read_text() + 'changed bytes')
                result = subprocess.run([sys.executable, str(self.runners / 'final-figures.py'), str(plan_path)], env=self.env, capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        finally:
            # A deliberately broken cleanup mutation must not leave its child alive.
            for log in work.glob('logs/*.log'):
                for line in log.read_text().splitlines():
                    if line.startswith('child_pid='):
                        try:
                            pid = int(line.split('=')[1])
                            with os.fdopen(os.pidfd_open(pid), 'rb') as descriptor:
                                args = (Path('/proc') / str(pid) / 'cmdline').read_bytes().split(b'\0')
                                if str(self.child).encode() in args:
                                    signal.pidfd_send_signal(descriptor.fileno(), signal.SIGKILL)
                        except (ProcessLookupError, FileNotFoundError):
                            pass
            if os.environ.get('PF1_WITNESS_ROOT'):
                destination = Path(os.environ['PF1_WITNESS_ROOT']) / ((mutation or 'restored') + '-' + case)
                shutil.copytree(self.root, destination)

    def test_publication(self): self.pipeline('missing-live', 2)
    def test_final(self): self.pipeline('no-final', 2)
    def test_assertion(self): self.pipeline('assertion', 101)
    def test_readback(self): self.pipeline('invalid', 2)
    def test_pass(self): self.pipeline('pass', 0)
    def test_shared(self): self.pipeline('shared', 75)
    def test_candidate(self): self.pipeline('candidate', 1)
    def test_validity(self): self.pipeline('invalid-final', 2)

    def test_runner_guards(self):
        self.env['PF1_FIXTURE_CASE'] = 'pass'
        for case in ['dispatch', 'unsealed']:
            with self.subTest(case=case):
                plan = dict(id=case, gates=[dict(pins=dict(path='pins.json', sha256=v.digest(self.root / 'pins.json')), test='wrong_test' if case == 'dispatch' else launch.TESTS['G4'], series={})])
                if case == 'unsealed':
                    predecessor = self.runners / 'series-interrupted'
                    predecessor.mkdir()
                    journal = predecessor / 'attempts-0.jsonl'
                    journal.write_bytes(b'{"partial":')
                plan_path = self.root / (case + '.json')
                plan_path.write_text(json.dumps(plan))
                result = subprocess.run([sys.executable, str(self.runners / 'final-measure.py'), str(plan_path)], env=self.env, capture_output=True, text=True, timeout=60)
                self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
                if case == 'dispatch':
                    receipt = v.read(self.runners / 'series-dispatch/0-0-receipt.json')
                    self.assertEqual(receipt['reason'], 'test does not implement pinned gate/protocol')
                    self.assertEqual(receipt['cleanup'], {})
                else:
                    self.assertIn('unsealed predecessor blocks collection', result.stdout)
                    self.assertEqual(journal.read_bytes(), b'{"partial":')
                    self.assertFalse((self.runners / 'series-unsealed').exists())
        if os.environ.get('PF1_WITNESS_ROOT'):
            shutil.copytree(self.root, Path(os.environ['PF1_WITNESS_ROOT']) / ((mutation or 'restored') + '-runner'))

    def test_host(self):
        host_path = self.root / self.pins['host']['path']
        original = v.read(host_path)
        for key in launch.host_readback():
            with self.subTest(key=key):
                changed = dict(original)
                changed[key] = original[key] + 4096 if key == 'ram_bytes' else 'other'
                if key == 'os': changed[key] = 'windows'
                host_path.write_text(json.dumps(changed))
                self.pins['host']['sha256'] = v.digest(host_path)
                (self.root / 'pins.json').write_text(json.dumps(self.pins))
                self.env['PF1_FIXTURE_CASE'] = 'pass'
                plan = dict(id='host-' + key, gates=[dict(pins=dict(path='pins.json', sha256=v.digest(self.root / 'pins.json')), test=launch.TESTS['G4'], series={})])
                plan_path = self.root / ('host-' + key + '.json')
                plan_path.write_text(json.dumps(plan))
                result = subprocess.run([sys.executable, str(self.runners / 'final-measure.py'), str(plan_path)], env=self.env, capture_output=True, text=True, timeout=60)
                self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
                work = self.runners / ('series-host-' + key)
                receipt = v.read(work / '0-0-receipt.json')
                self.assertEqual(receipt['host'], launch.host_readback())
                self.assertEqual(receipt['reason'], 'current host differs from pins')
                self.assertEqual(receipt['cleanup'], {})
                self.assertFalse((work / 'logs/0-0.log').exists())
        if os.environ.get('PF1_WITNESS_ROOT'):
            shutil.copytree(self.root, Path(os.environ['PF1_WITNESS_ROOT']) / ((mutation or 'restored') + '-host'))


if __name__ == '__main__':
    unittest.main()
