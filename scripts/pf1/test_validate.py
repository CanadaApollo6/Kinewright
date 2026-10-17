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
from unittest.mock import Mock, patch
import validate as v
import series as runner

MUTATIONS = {
    'lh_cached': ("lh_gpu(raw, lh_samples(row['environment']), pins['gpu']['policy']) == row['gpu']", "True"),
    'control_source': ("other['builds'] == pins['builds']", "True"),
    'control_required': ("control_code != 0", "False"),
    'control_sensitivity': ("return 0 if all(any(row['metrics'][key] > rule['limit'] for key,rule in pins['metrics'].items()) for row in rows) else 1", "return 0"),
    'lh_identity': ("len(matches) == 1", "len(matches) >= 1"),
    'lh_readback': ("fields[0] == uuid", "True"),
    'icd_bytes': ("icd=Path(icd).read_bytes().decode('utf-8')", "icd=Path(icd).read_text()"),
    'default_scope': ("pins['gate'] in ['G1', 'G2', 'G14', 'Q3-G1', 'Q3-G14']", "True"),
    'default_uuid': ("observation['uuid'] == gpu['selected']['uuid']", "True"),
    'default_coverage': ("any(row['started_s'] < x['timestamp_s'] < row['ended_s'] for x in telemetry)", "True"),
    'media_guard': ("not path.is_symlink() and path.is_file() and path.suffix.lower() in ['.mp4', '.wav']", 'path.is_file()'),
    'media_format': ("header[4:8] == b'ftyp' if path.suffix.lower() == '.mp4' else header[:4] == b'RIFF' and header[8:12] == b'WAVE'", 'True'),
    'receipt_exit': ("receipt['exit'] == row['exit'] == 0", 'True'),
    'receipt_order': ("all(receipt[k] == value for k, value in expected.items())", 'True'),
    'd13_cap': ("len(ids) <= protocol['reader_cap']*(protocol['B']-1)", 'True'),
    'd13_sets': ("all(set(a['identities']) == set(b['identities']) for a, b in zip(rows[::2], rows[1::2]))", 'True'),
    'd13_order': ("all(b['ack_us'] > a['ack_us'] and b['version'] > a['version'] for a, b in zip(rows, rows[1:]))", 'True'),
    'd13_period': ("protocol['sample_ms'] == 100", 'True'),
    'collector_final': ("raw['valid'] is True and raw['metrics'] == row['metrics'] and raw['selected'] == row['gpu']['selected']", 'True'),
    'd13_coverage': ("[(r['cycle'], r['target']) for r in rows] == [(cycle, target) for cycle in range(30) for target in [180, 1200]]", 'True'),
    'd13_spacing': ("times[0] >= ack+100000 and times[1] >= ack+200000 and times[1]-times[0] >= 100000", 'True'),
    'd13_calibration': ("record['calibration_sha256'] == protocol['calibration']['sha256'] and metrics['u_bytes'] == 30*protocol['d_bytes']", 'True'),
    'native_count': ("record['native_children'] == 1", 'True'),
    'native_types': ("all(type(value) is int for value in [record['native_children'], record['publisher_pid'], cleanup['pid'], cleanup['exit']]) and cleanup['pid'] > 0", 'True'),
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
    'unsupported': ("require(gate in CONTRACTS, 'unsupported/incomplete gate contract')", "gate = 'I4'"),
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
    'gpu_table': ("v.require(match is not None, 'unrecognized GPU process row')", "if match is None: continue"),
    'live': ("v.gpu_check(gpu, pins, host, root)", "pass"),
    'replacement': ("disposition['previous_sha256'] == series['previous']['sha256']", "True"),
    'resume': ("if not destination.exists():", "if True:"),
    'reference': ("single or reference == expected", "True"),
    'protocol': ("all(protocol.get(key) == value for key, value in parameters.items())", "True"),
    'denominator': ("require(all(x > 0 for x in a), 'zero reference denominator')", "if not all(x > 0 for x in a): return 0"),
    'role': ("role = 'reference' if entry['arm'] == pins['arms'].get('reference') else 'candidate'", "role = entry['arm']"),
    'runner_exit': ("return code if code in [2, 75, 101] else 2", "return 0"),
    'launch_error': ("code, row = 2, dict(validity='invalid_evidence', exit=2, reason=str(error))", "raise"),
    'r50': ("else 75 for x, y in zip(a, b)", "else 0 for x, y in zip(a, b)"),
    'host_shape': ("set(actual) == {'os', 'version', 'architecture', 'cpu', 'ram_bytes'}", 'True'),
    'deadline': ('signal.setitimer(signal.ITIMER_REAL, seconds)', 'pass'),
    'host_ram': ("os.sysconf('SC_PHYS_PAGES')", "os.sysconf('SC_AVPHYS_PAGES')"),
}
if '--mutation' in sys.argv:
    index = sys.argv.index('--mutation'); name = sys.argv[index+1]; del sys.argv[index:index+2]
    old, new = MUTATIONS[name]
    target = runner if name in ['resume', 'role', 'runner_exit', 'launch_error'] else launch if name in ['icd_bytes', 'lh_identity', 'lh_readback', 'owned_cleanup', 'gpu_table', 'live', 'host_shape', 'host_ram', 'deadline', 'native_count', 'native_types'] else v
    source = Path(target.__file__).read_text(); assert old in source
    exec(compile(source.replace(old, new), target.__file__, 'exec'), target.__dict__)

class Pins(unittest.TestCase):
    def lh_fixture(self):
        uuid = 'GPU-b57f8cd6-b638-69a4-e9af-54ad6b8fc7f5'
        record = dict(selected=dict(backend='Vulkan', name='NVIDIA GeForce RTX 3090', vendor_id=0x10de, device_id=0x2204, driver='NVIDIA'), software=False, driver_info='615.78.08')
        probe = dict(inventory=dict(exit=0, output=f'0, {uuid}, NVIDIA GeForce RTX 3090, 0x220410DE, 615.78.08'), readback=dict(exit=0, output=f'{uuid}, 210, 405, P8, 350.00, 27.00'), icd=json.dumps(dict(ICD=dict(library_path='libGLX_nvidia.so.0'))))
        return record, probe, uuid

    def test_lh_identity(self):
        record, probe, uuid = self.lh_fixture()
        selected, stack = launch.lh_identity(record, probe)
        self.assertEqual(selected, dict(record['selected'], uuid=uuid))
        self.assertEqual(stack['mesa']['status'], 'unavailable')
        self.assertIn('615.78.08', stack['runtime'])
        changes = [
            ('ambiguous', lambda r,p: p['inventory'].update(output=p['inventory']['output']+'\n'+p['inventory']['output'].replace('GPU-b57f8cd6', 'GPU-a57f8cd6'))),
            ('name-only', lambda r,p: r['selected'].update(device_id=1)),
            ('vendor', lambda r,p: r['selected'].update(vendor_id=1)),
            ('software', lambda r,p: r.update(software=True)),
            ('driver', lambda r,p: r.update(driver_info='other version')),
            ('inventory', lambda r,p: p['inventory'].update(exit=1)),
            ('truncated', lambda r,p: p['inventory'].update(output='0, '+uuid)),
            ('UUID', lambda r,p: r['selected'].update(uuid='GPU-wrong')),
            ('ICD', lambda r,p: p.update(icd=json.dumps(dict(ICD=dict(library_path='libvulkan_lvp.so'))))),
        ]
        for name, change in changes:
            with self.subTest(name=name):
                changed, other = copy.deepcopy(record), copy.deepcopy(probe)
                change(changed, other)
                with self.assertRaises(ValueError): launch.lh_identity(changed, other)

    def test_lh_readback(self):
        _, probe, uuid = self.lh_fixture()
        observed = launch.lh_observation(probe, 100, uuid)
        self.assertEqual((observed['graphics_mhz'], observed['power_limit_w'], observed['power_draw_w']), (210,350,27))
        for text in [probe['readback']['output'].replace(uuid, 'GPU-other'), f'{uuid}, 210, 405, P8, 350', f'{uuid}, nan, 405, P8, 350, 27', f'{uuid}, 210, 405, BAD, 350, 27']:
            changed = copy.deepcopy(probe); changed['readback']['output'] = text
            with self.subTest(text=text), self.assertRaises(ValueError): launch.lh_observation(changed, 100, uuid)
        probe['readback']['output'] = f'{uuid}, 0, 10000, P0, [Not Supported], [N/A]'
        observed = launch.lh_observation(probe, 100, uuid)
        self.assertEqual(observed['graphics_mhz'], 0)
        self.assertIn('reason', observed['power_limit_w'])
        self.assertIn('reason', observed['power_draw_w'])
        probe['readback']['exit'] = 1
        with self.assertRaises(ValueError): launch.lh_observation(probe, 100, uuid)

    def test_icd_bytes(self):
        record, probe, uuid = self.lh_fixture()
        payload = (probe['icd']+'\r\n').encode()
        path = self.root/'nvidia.json'; path.write_bytes(payload)
        observed = launch.lh_probe(Mock(side_effect=[probe['inventory'],probe['readback']]),dict(VK_DRIVER_FILES=str(path),PF1_GPU_UUID=uuid))
        self.assertEqual(launch.lh_identity(record, observed)[1]['icd_manifest_sha256'],v.hashlib.sha256(payload).hexdigest())

    def default_policy(self):
        self.configure('G1')

    def use_default_policy(self):
        record, probe, uuid = self.lh_fixture()
        ruling = self.root/'r89.txt'; ruling.write_text('## R89-2:\n- Change no GPU setting.\n')
        policy = dict(kind='recorded-default', ruling=dict(path=ruling.name, sha256=v.digest(ruling)))
        for row in self.status['runs']:
            times = [row['started_s']-.01, (row['started_s']+row['ended_s'])/2, row['ended_s']+.01]
            row['gpu'] = launch.lh_gpu(record, [dict(utc=t, lh=probe) for t in times], policy)
            for sample in row['preflight']: sample['uuid'] = uuid
        self.pins['gpu'] = copy.deepcopy(self.status['runs'][0]['gpu'])
        self.pins['protocol']['sample_ms'] = 5000

    def test_recorded_default_policy(self):
        self.default_policy()
        for row in self.status['runs']:
            for observation in row['gpu']['telemetry']:
                observation.update(graphics_mhz=0, memory_mhz=100000, power_limit_w=1, power_draw_w=10000, pstate='P8')
        self.run_record(0)
        policy = self.pins['gpu']['policy']
        with self.assertRaises(ValueError): v.schema(dict(policy, requested={}), v.SCHEMA['$defs']['policy'])
        self.pins['gate'] = 'G3'
        with self.assertRaises(ValueError): v.gpu_check(self.pins['gpu'], self.pins, dict(os='linux'), self.root)
        self.pins['gate'] = 'G1'
        self.pins['gpu']['telemetry'][0]['uuid'] = 'GPU-other'
        with self.assertRaises(ValueError): v.gpu_check(self.pins['gpu'], self.pins, dict(os='linux'), self.root)

    def test_default_policy_coverage(self):
        self.default_policy()
        row = self.status['runs'][0]; original = copy.deepcopy(row['gpu']['telemetry'])
        for times in [[row['started_s']+.01, row['ended_s']-.01, row['ended_s']+.01], [row['started_s']-.02, row['started_s']-.01, row['ended_s']+.01], [row['started_s']-.01, row['ended_s']-.02, row['ended_s']-.01], [row['started_s']-.01, row['started_s']+.01, row['ended_s']+10]]:
            row['gpu']['telemetry'] = [dict(x,timestamp_s=t) for x,t in zip(original,times)]
            with self.subTest(times=times): self.run_record(2)

    def test_lh_identity_cannot_change_between_samples(self):
        record, probe, _ = self.lh_fixture()
        other = copy.deepcopy(probe); other['icd'] += '\n'
        with self.assertRaisesRegex(ValueError, 'changed during invocation'):
            launch.lh_gpu(record, [dict(utc=10,lh=probe),dict(utc=15,lh=other)], dict(kind='recorded-default'))

    def test_g14_contract(self):
        self.configure('G14'); self.run_record(0)
        self.assertEqual(len(self.status['runs']),36)
        self.assertEqual(self.pins['metrics']['held_max_ms'],dict(decision='R50_abs',unit='ms',scope='each_run',limit=100))
        original = copy.deepcopy(self.pins)
        for change in [lambda p:p['metrics'].pop('held_max_ms'),lambda p:p['protocol'].pop('control'),lambda p:p['protocol']['duration_s'].update(talk_recut=60),lambda p:p['metrics']['held_max_ms'].update(limit=101)]:
            altered = copy.deepcopy(original); change(altered); self.p.write_text(json.dumps(altered))
            with self.assertRaises((ValueError,KeyError)): v.spec(self.p,self.root)
        inputs = v.read(self.root/original['inputs']['path'])
        for values in inputs.values(): values.pop('feed_4x5')
        path=self.root/'missing-feed.json';path.write_text(json.dumps(inputs))
        original['inputs']=dict(path=path.name,sha256=v.digest(path))
        original['protocol']['order']=[r for r in original['protocol']['order'] if r['workload']!='feed_4x5']
        self.p.write_text(json.dumps(original))
        with self.assertRaises(ValueError): v.spec(self.p,self.root)

    def test_q3_sensitivity(self):
        for gate in ['Q3-G1','Q3-G14']:
            self.setUp();self.configure(gate);self.run_record(0)
            self.status['runs'][0]['metrics']={key:0 for key in self.pins['metrics']}
            with self.subTest(gate=gate):self.run_record(1)

    def test_lh_cached_samples(self):
        self.configure('G14');self.run_record(0)
        row=self.status['runs'][0];row['gpu']['telemetry'][0]['graphics_mhz']+=1
        row['fingerprint']=v.fingerprint(row)
        status=self.root/'status.json';status.write_text(json.dumps(self.status))
        with self.assertRaisesRegex(ValueError,'cached LH readback mismatch'):v.evaluate(self.p,status,self.root)

    def test_g14_control_requirement(self):
        for gate in ['G1','G14']:
            self.setUp();self.configure(gate);self.run_record(0)
            binding=v.read(self.root/self.pins['protocol']['control']['path']);path=self.root/binding['status'];status=v.read(path)
            saved=self.pins,self.status,self.p;self.p=self.root/binding['pins']['path'];self.pins=v.read(self.p);self.status=status
            for row in status['runs']:row['metrics']={key:0 for key in row['metrics']}
            self.run_record(1);self.pins,self.status,self.p=saved;self.run_record(1)
            for code in [75,101]:
                with self.subTest(gate=gate,control=code):
                    child=self.root/binding['pins']['path'];receipt=self.root/'control-failure.json'
                    receipt.write_text(json.dumps(dict(v.read(child)['protocol']['order'][0],exit=code,reason='synthetic control failure',pins=v.digest(child),logs=[],cleanup=dict(reaped=True,returncode=code))))
                    failed=dict(status,runs=[],failure=dict(exit=code,receipt=dict(path=receipt.name,sha256=v.digest(receipt))))
                    path.write_text(json.dumps(failed));self.run_record(code)
            for content in ['{',None]:
                if content is None:path.unlink()
                else:path.write_text(content)
                self.run_record(2)

    def test_control_source(self):
        self.configure('G14')
        binding_path=self.root/self.pins['protocol']['control']['path'];binding=v.read(binding_path)
        child_path=self.root/binding['pins']['path'];child=v.read(child_path)
        builds=v.read(self.root/child['builds']['path']);binary=self.root/'other-binary';binary.write_bytes(b'other source-bound binary')
        for build in builds.values():build['binary']=dict(path=binary.name,sha256=v.digest(binary))
        path=self.root/'other-builds.json';path.write_text(json.dumps(builds));child['builds']=dict(path=path.name,sha256=v.digest(path))
        child_path.write_text(json.dumps(child));binding['pins']['sha256']=v.digest(child_path)
        binding_path.write_text(json.dumps(binding));self.pins['protocol']['control']['sha256']=v.digest(binding_path)
        with self.assertRaisesRegex(ValueError,'control source/host mismatch'):v.control_binding(self.pins,self.root,v.read(self.root/self.pins['inputs']['path']))

    @unittest.skipUnless(sys.platform == 'linux', 'Linux invocation watchdog')
    def test_deadline(self):
        cleanup = {}
        previous = launch.signal.getsignal(launch.signal.SIGALRM)
        try:
            with self.assertRaisesRegex(TimeoutError, 'complete invocation deadline'):
                with launch.owned_child([sys.executable, '-c', 'import time; time.sleep(30)'], cleanup=cleanup) as process:
                    launch.arm_deadline(.05)
                    process.wait(timeout=4)
            self.assertTrue(cleanup['reaped'])
            self.assertIsNotNone(process.returncode)
        finally:
            launch.signal.setitimer(launch.signal.ITIMER_REAL, 0)
            launch.signal.signal(launch.signal.SIGALRM, previous)

    def test_host_readback(self):
        with patch.object(launch.platform, 'system', return_value='Linux'), patch.object(launch.platform, 'release', return_value='kernel'), patch.object(launch.platform, 'machine', return_value='x86_64'), patch.object(Path, 'read_text', return_value='model name : CPU\nmodel name : CPU\n'), patch.object(os, 'sysconf', create=True, side_effect=lambda key: {'SC_PHYS_PAGES': 1000, 'SC_AVPHYS_PAGES': 100, 'SC_PAGE_SIZE': 4096}[key]):
            self.assertEqual(launch.host_readback(), dict(os='linux', version='kernel', architecture='x86_64', cpu='CPU', ram_bytes=4096000))
        with patch.object(launch.platform, 'system', return_value='Windows'), self.assertRaises(ValueError):
            launch.host_readback()
        for text in ['', 'model name : CPU A\nmodel name : CPU B']:
            with patch.object(launch.platform, 'system', return_value='Linux'), patch.object(Path, 'read_text', return_value=text), self.assertRaises(ValueError):
                launch.host_readback()

    def test_host_shape(self):
        actual = dict(os='linux', version='kernel', architecture='x86_64', cpu='CPU', ram_bytes=1024)
        launch.host_check(actual, actual)
        for key in actual:
            incomplete = dict(actual)
            incomplete.pop(key)
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, 'incomplete host readback'):
                launch.host_check(actual, incomplete)

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='kinewright-pf1-pins-'); self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name); self.p = self.root/'pins.json'
        repo = Path(__file__).resolve().parents[2]
        shutil.copytree(repo/'docs/reports/pf1-s4/example', self.root/'docs/reports/pf1-s4/example')
        self.pins = v.read(repo/'docs/PF1-PINS.json'); self.status = v.read(self.root/'docs/reports/pf1-s4/example/status.json')
    def run_record(self, expected, resign=True):
        prefix = '' if self.p.name == 'pins.json' else self.p.stem+'-'
        self.p.write_text(json.dumps(self.pins), encoding='utf-8')
        self.status['series']['pins'] = v.digest(self.p)
        if self.pins['gate'] in v.COLLECTED:
            log = self.root/'native-fixture.log'; log.write_text('synthetic native log\n')
            for index, row in enumerate(self.status['runs']):
                row['logs'] = [dict(path=log.name,sha256=v.digest(log))]
                path = self.root/f'{prefix}collector-{index}.json'
                row['environment'].update(PF1_RECORD=str(path), PF1_TOKEN='portable-synthetic')
                value = v.read(self.root/self.pins['inputs']['path'])[row['arm']][row['workload']]
                raw = dict(valid=True, metrics=row['metrics'], selected=row['gpu']['selected'], token='portable-synthetic', timestamp_s=row['started_s'], consumed={k:value[k]['sha256'] for k in ['workload','media','document']}, native_children=1, publisher_pid=index+42, native_cleanup=dict(pid=index+42,reaped=True,exit=0))
                raw['protocol'] = dict(name=self.pins['protocol']['name'],poll_ms=5,**v.CONTRACTS[self.pins['gate']][3])
                if self.pins['lane'] == 'LH':
                    _, probe, _ = self.lh_fixture(); snapshots = []
                    keys = ['graphics_mhz','memory_mhz','pstate','power_limit_w','power_draw_w']
                    for observation in row['gpu']['telemetry']:
                        observed = copy.deepcopy(probe)
                        values = [observation['uuid']] + [str(observation[k]) if not isinstance(observation[k],dict) else observation[k]['reason'].removeprefix('nvidia-smi returned ') for k in keys]
                        observed['readback']['output'] = ', '.join(values)
                        snapshots.append(dict(utc=observation['timestamp_s'],lh=observed))
                    samples = self.root/f'{prefix}lh-{index}.jsonl'
                    samples.write_text(''.join(json.dumps(x)+'\n' for x in snapshots))
                    row['logs'].append(dict(path=samples.name,sha256=v.digest(samples)))
                    row['environment'].update(PF1_GATE=self.pins['gate'],PF1_LH_SAMPLES=str(samples))
                    raw.update(driver_info=row['gpu']['stack']['runtime'].split(';')[0],software=False)
                if self.pins['gate'] == 'D13': raw['protocol']['d_bytes'] = self.pins['protocol']['d_bytes']
                if self.pins['gate'] == 'D13' and 'calibration' in self.pins['protocol']: raw.update(self.d13_record())
                path.write_text(json.dumps(raw))
                row['logs'] = [ref for ref in row['logs'] if ref['path'] != path.name] + [dict(path=path.name,sha256=v.digest(path))]
                build = v.read(self.root/self.pins['builds']['path'])[row['arm']]
                row['identity'].update({k:build[k]['sha256'] for k in ['binary','production','harness']},host=self.pins['host']['sha256'])
                row['command'] = ['timeout', '--kill-after=1', '570', 'nice', '-n', '10', str(self.root/v.read(self.root/self.pins['builds']['path'])[row['arm']]['binary']['path']), '--exact', launch.TESTS[self.pins['gate']], '--ignored', '--nocapture', '--test-threads=1']
                row['environment']['PF1_SEED'] = str(row['seed'])
                receipt = self.root/f'{prefix}receipt-{index}.json'
                receipt.write_text(json.dumps(dict({k:row[k] for k in ['arm','workload','pair','seed','exit','command']}, host={k:v.read(self.root/self.pins['host']['path'])[k] for k in ['os','version','architecture','cpu','ram_bytes']}, pins=v.digest(self.p), token='portable-synthetic', cleanup=dict(reaped=True,returncode=0))))
                row['logs'] = [ref for ref in row['logs'] if ref['path'] != receipt.name] + [dict(path=receipt.name,sha256=v.digest(receipt))]
        for row in self.status['runs']:
            row['identity']['pins'] = v.digest(self.p)
            if resign: row['fingerprint'] = v.fingerprint(row)
        status = self.root/(prefix+'status.json'); status.write_text(json.dumps(self.status), encoding='utf-8')
        try: code = v.evaluate(self.p, status, self.root)
        except (ValueError, OSError, KeyError, TypeError): code = 2
        self.assertEqual(code, expected)

    def control_fixture(self):
        saved = self.pins, self.status, self.p
        parent, status, _ = saved
        gate = 'Q3-'+parent['gate']
        entries = [entry for entry in parent['protocol']['order'] if entry['workload']=='typical_1080p' and entry['pair']==0]
        inputs = v.read(self.root/parent['inputs']['path'])
        path = self.root/(gate+'-inputs.json')
        path.write_text(json.dumps({arm:{'typical_1080p':value['typical_1080p']} for arm,value in inputs.items()}))
        self.pins = copy.deepcopy(parent)
        self.pins.update(gate=gate,inputs=dict(path=path.name,sha256=v.digest(path)))
        self.pins['protocol'] = dict(name='P-play',poll_ms=5,sample_ms=5000,runs=1,order=entries,**v.CONTRACTS[gate][3])
        self.status = copy.deepcopy(status)
        self.status['gate'] = gate
        self.status['series'] = dict(id=gate,pins='a'*64)
        self.status['runs'] = [row for row in self.status['runs'] if row['workload']=='typical_1080p' and row['pair']==0]
        for row in self.status['runs']:
            row['metrics'] = {key:rule['limit']+1 for key,rule in self.pins['metrics'].items()}
        self.p = self.root/(gate+'-pins.json')
        self.run_record(0)
        control = dict(pins=dict(path=self.p.name,sha256=v.digest(self.p)),status=self.p.stem+'-status.json')
        self.pins, self.status, self.p = saved
        path = self.root/(gate+'-binding.json'); path.write_text(json.dumps(control))
        self.pins['protocol']['control'] = dict(path=path.name,sha256=v.digest(path))
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
        if gate in ['G7a', 'D13']:
            self.pins['arms'].pop('reference')
            builds.pop('s3b')
            bases = ['candidate']
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
        self.pins['metrics'] = {k:dict(decision=d, unit=u, limit=l, scope='report' if d == 'report' else 'paired_median' if d in ['ratio','R65'] else 'invariant' if d in ['equal','zero','invariant'] else 'each_run') for k,(d,u,l) in rules.items()}
        order = []
        for workload in sorted(workloads):
            for rotation, arms in enumerate(itertools.permutations(builds) if gate.startswith('R65-') else [tuple(builds)]):
                for seed_index, seed in enumerate([11] if gate == 'G7a' or gate.startswith('Q3-') else [11,22,33]):
                    order.extend(dict(workload=workload, pair=rotation*3+seed_index, arm=arm, seed=seed) for arm in arms)
        self.pins['protocol'] = dict(name=protocol, poll_ms=5, sample_ms=100, runs=18 if gate.startswith('R65-') else 1 if gate=='G7a' or gate.startswith('Q3-') else 3, order=order, **parameters)
        row = self.status['runs'][0]
        self.status['runs'] = [dict(copy.deepcopy(row), **entry, metrics={k:True if u=='boolean' else 'a'*64 if u=='sha256' else 0 if d in ['zero','R50_abs'] else 100 for k,(d,u,l) in rules.items()}) for entry in order]
        if self.pins['lane'] != 'LH':
            self.pins['gpu']['policy'] = {'kind':'software'}
            for row in self.status['runs']: row['gpu']['policy'] = {'kind':'software'}

        if gate in v.COLLECTED: self.freeze_inputs()
        if gate in v.LH_PLAY: self.use_default_policy()
        if gate.startswith('Q3-'):
            for row in self.status['runs']:
                row['metrics'] = {key:rule['limit']+1 for key,rule in self.pins['metrics'].items()}
        if gate in ['G1','G14']: self.control_fixture()
        if gate == 'D13':
            inputs = v.read(self.root/self.pins['inputs']['path'])
            media = self.root/next(iter(inputs.values()))['seek_gop60']['media']['path']
            payload = self.root/'seek.mp4'; payload.write_bytes(b'\0\0\0\x18ftypisom')
            media.write_text(json.dumps(dict(files=[dict(path=payload.name,sha256=v.digest(payload))])))
            for workloads in inputs.values(): workloads['seek_gop60']['media']['sha256'] = v.digest(media)
            for workloads in inputs.values():
                value = workloads['seek_gop60']; path = self.root/value['workload']['path']
                path.write_text(json.dumps(dict(key='seek_gop60',document=value['document'],media=value['media'])))
                value['workload']['sha256'] = v.digest(path)
            self.pins['inputs'] = put('inputs.json', inputs)
            calibration = dict(media_sha256=v.digest(payload), method='safe measured plane span union', B=16,reader_cap=2,d_bytes=100,frames=1800,shape=dict(width=1920,height=1080))
            self.pins['protocol'].update(d_bytes=100,calibration=put('calibration.json',calibration))
            for row in self.status['runs']: row['metrics'].update(idle_bytes=100,peak_bytes=150,peak_minus_idle_bytes=50,u_bytes=3000)

    def d13_record(self):
        return dict(calibration_sha256=self.pins['protocol']['calibration']['sha256'],d13_intervals=[dict(cycle=c,target=t,version=2*c+i+1,ack_us=100000+1000000*(2*c+i),identities=[1],sample_us=[200000+1000000*(2*c+i),300000+1000000*(2*c+i)]) for c in range(30) for i,t in enumerate([180,1200])])

    def test_pass_receipts(self):
        self.configure('G4'); self.run_record(0)
        row = self.status['runs'][0]; path = self.root/row['logs'][-1]['path']; original = v.read(path)
        for key, value in [('exit',2), ('host',dict(original['host'],cpu='wrong')), ('command',['wrong']), ('seed',-1), ('pair',999)]:
            path.write_text(json.dumps(dict(original, **{key:value})))
            row['logs'][-1]['sha256'] = v.digest(path); row['fingerprint'] = v.fingerprint(row)
            status = self.root/'status.json'; status.write_text(json.dumps(self.status))
            with self.subTest(key=key), self.assertRaises(ValueError): v.evaluate(self.p,status,self.root)

    def test_native_cached_final(self):
        self.configure('G15'); self.run_record(0)
        row = self.status['runs'][0]
        path = Path(row['environment']['PF1_RECORD']); raw = v.read(path)
        for field, value in [('metrics', dict(settled_bytes=999)), ('consumed', {}), ('native_children', 3)]:
            path.write_text(json.dumps(dict(raw, **{field:value})))
            for ref in row['logs']:
                if ref['path'] == path.name: ref['sha256'] = v.digest(path)
            row['fingerprint'] = v.fingerprint(row)
            status = self.root/'status.json'; status.write_text(json.dumps(self.status))
            with self.subTest(field=field), self.assertRaises(ValueError): v.evaluate(self.p,status,self.root)

    def test_frozen_nested_media(self):
        self.configure('G4')
        inputs_path = self.root/self.pins['inputs']['path']
        inputs = v.read(inputs_path)
        entry = self.pins['protocol']['order'][0]
        key = entry['workload']; value = inputs[entry['arm']][key]
        path = self.root/'nested-media.mp4'
        path.write_bytes(b'\0\0\0\x18ftypisom')
        media = self.root/value['media']['path']
        manifest = self.root/value['workload']['path']
        def bind(ref):
            media.write_text(json.dumps(dict(files=[ref])))
            value['media']['sha256']=v.digest(media)
            manifest.write_text(json.dumps(dict(key=key,document=value['document'],media=value['media'])))
            value['workload']['sha256']=v.digest(manifest)
            inputs_path.write_text(json.dumps(inputs))
            self.pins['inputs']['sha256']=v.digest(inputs_path)
        bind(dict(path=path.name,sha256=v.digest(path)))
        v.frozen_input(self.root,key,value)
        for name, content in [('link.mp4',None), ('directory.mp4',None), ('media.bin',b'RIFF\0\0\0\0WAVE'), ('concat.mp4',b"ffconcat version 1.0\nfile 'payload.mp4'\n"), ('playlist.wav',b'#EXTM3U\npayload.mp4\n')]:
            bad = self.root/name
            if name == 'link.mp4':
                try: bad.symlink_to(path)
                except OSError:
                    if os.name == 'nt': continue
                    raise
            elif name == 'directory.mp4': bad.mkdir()
            else: bad.write_bytes(content)
            bind(dict(path=name,sha256=v.digest(path) if content is None else v.digest(bad)))
            for check in [lambda: launch.input_environment(self.pins,self.root,0), lambda: v.frozen_input(self.root,key,value)]:
                with self.subTest(name=name), self.assertRaises(ValueError): check()
        bind(dict(path=path.name,sha256=v.digest(path)))
        path.write_bytes(b'changed media')
        with self.assertRaises(ValueError): v.frozen_input(self.root,key,value)

    def test_d13(self):
        self.configure('D13'); self.run_record(0)
        self.pins['protocol']['sample_ms']=99; self.run_record(2)
        self.pins['protocol']['sample_ms']=100
        raw = dict(self.d13_record(), metrics=self.status['runs'][0]['metrics'])
        v.d13_check(raw,self.pins)
        for change in ['interval','incomplete','spacing','calibration','cap','sets','ack','version']:
            altered = copy.deepcopy(raw)
            if change == 'interval': altered['d13_intervals'][0]['target']=999
            elif change == 'incomplete': altered['d13_intervals'].pop()
            elif change == 'spacing': altered['d13_intervals'][0]['sample_us']=[200000,299999]
            elif change == 'calibration': altered['calibration_sha256']='f'*64
            elif change == 'cap':
                for row in altered['d13_intervals']: row['identities']=list(range(1,32))
            elif change == 'sets': altered['d13_intervals'][1]['identities']=[2]
            elif change == 'ack': altered['d13_intervals'][1]['ack_us']=99999
            else: altered['d13_intervals'][1]['version']=61
            with self.subTest(change=change), self.assertRaises(ValueError): v.d13_check(altered,self.pins)
        self.pins['protocol'].pop('calibration'); self.run_record(2)

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

    def test_gpu_table(self):
        text = (Path(__file__).resolve().parents[2]/'docs/reports/pf1-s4/example/nvidia-processes.txt').read_text()
        start = text.index('|    0   N/A')
        end = text.index('+---', start)
        empty = text[:start] + '|  No running processes found |\n' + text[end:]
        bad = text[:start] + '| 0 N/A N/A 123 X /worker N/A |\n' + text[end:]
        variants = [(text,0,0),(empty,0,0),('Processes:\n',0,2),(text[:end],0,2),(bad,0,2),(text.replace('|    0   N/A','|    1   N/A'),0,2),(text,1,2),(text.replace('C+G','C'),0,75),(text.replace('498MiB','2049MiB'),0,75)]
        for table, query_exit, expected in variants:
            with self.subTest(table=table, query_exit=query_exit):
                try:
                    samples = [launch.gpu_sample(dict(exit=0,output=table),dict(exit=query_exit,output='0, synthetic-uuid, 0'),1700000000+i*5,set()) for i in range(3)]
                    for row in self.status['runs']: row['preflight'] = samples
                    self.run_record(expected)
                except ValueError:
                    self.assertEqual(expected,2)
        for row in self.status['runs']:
            row['preflight'] = [launch.gpu_sample(dict(exit=0,output=empty),dict(exit=0,output='0, synthetic-uuid, 80'),1700000000+i*5,set()) for i in range(3)]
        self.run_record(75)
        with self.assertRaises(SystemExit) as raised:
            launch.checked(launch.gpu_sample,dict(exit=0,output='Processes:\n'),dict(exit=0,output='0, synthetic-uuid, 0'),1700000000,set())
        self.assertEqual(raised.exception.code,2)

    def test_native_receipt(self):
        raw = dict(native_children=1, publisher_pid=42, native_cleanup=dict(pid=42, reaped=True, exit=0))
        launch.native_check(raw)
        for key, value in [('native_children', 3), ('native_children', True), ('reaped', False), ('exit', 101), ('exit', False), ('pid', 43)]:
            altered = copy.deepcopy(raw)
            (altered if key == 'native_children' else altered['native_cleanup'])[key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                launch.native_check(altered)

    def freeze_inputs(self):
        inputs = v.read(self.root/self.pins['inputs']['path'])
        for workloads in inputs.values():
            for key, value in workloads.items():
                media = self.root/(key+'-media.json')
                media.write_text(json.dumps(dict(files=[])))
                value['media'] = dict(path=media.name, sha256=v.digest(media))
                manifest = self.root/(key+'-workload.json')
                manifest.write_text(json.dumps(dict(key=key, document=value['document'], media=value['media'])))
                value['workload'] = dict(path=manifest.name, sha256=v.digest(manifest))
        path = self.root/self.pins['inputs']['path']
        path.write_text(json.dumps(inputs))
        self.pins['inputs']['sha256'] = v.digest(path)

    def test_live(self):
        self.configure('G4')
        path = self.root/'live.json'
        raw = dict(token='fresh', timestamp_s=12, selected=dict(backend='Vulkan',name='llvmpipe (LLVM 20.1.5, 256 bits)',vendor_id=65541,device_id=0,driver='llvmpipe'), driver_info='Mesa 25.1.0',software=True)
        raw['protocol'] = dict(name='native-display',poll_ms=5,warmup_ms=2000,duration_s=60)
        raw['consumed'] = launch.input_binding(self.pins, self.root, 0)
        icd = self.root/'lvp.json'; icd.write_text('fake ICD manifest')
        gpu = dict(selected=raw['selected'],stack=dict(runtime=raw['driver_info'],mesa='25.1.0',llvm='20.1.5',icd_manifest_sha256=v.digest(icd)),policy=dict(kind='software'),telemetry=dict(status='unavailable',reason='software adapter has no hardware clocks'))
        self.pins['gpu'] = copy.deepcopy(gpu)
        clock = Mock(return_value=15)
        check = lambda: launch.live_record(path,'fresh',10,clock(),self.pins,dict(os='linux'),self.root,dict(VK_ICD_FILENAMES=str(icd),PF1_ROW='0'))
        self.assertIsNone(check())
        clock.return_value=21
        with self.assertRaises(ValueError): check()
        clock.return_value=15; path.write_text(json.dumps(raw))
        self.assertEqual(check()[1],gpu)
        for changed in [dict(timestamp_s=9),dict(timestamp_s=16),dict(token='stale'),dict(protocol={})]:
            path.write_text(json.dumps(dict(raw,**changed)))
            with self.assertRaises(ValueError): check()
        path.write_text(json.dumps(raw))
        process = Mock(); process.poll.side_effect=[None,None,0]
        def change_device():
            raw['selected']['vendor_id']=123; path.write_text(json.dumps(raw))
        with self.assertRaisesRegex(ValueError,'adapter/stack mismatch'):
            launch.monitor(process,check,change_device,lambda seconds: None)

    def test_replacement(self):
        for row in self.status['runs']:
            if row['arm']=='s4': row['metrics']['mean_ms']=200
        self.run_record(1)
        old = self.root/'old.json'; shutil.copyfile(self.root/'status.json',old)
        previous = dict(path='old.json',sha256=v.digest(old))
        self.status['series']=dict(id='replacement',pins=v.digest(self.p),previous=previous,rejection=dict(exit=1,reason='all wall ratios rejected'))
        for row in self.status['runs']: row['metrics']['mean_ms']=100
        self.run_record(2)
        disposition=self.root/'disposition.json'
        ref=self.status['runs'][0]['logs'][0]
        disposition.write_text(json.dumps(dict(authority='synthetic ruling',previous_sha256=previous['sha256'],reason='dispose of all rejected ratios',record=ref)))
        self.status['series']['rejection']['disposition']=dict(path='disposition.json',sha256=v.digest(disposition))
        self.run_record(0)
        data=v.read(disposition); data['previous_sha256']='f'*64; disposition.write_text(json.dumps(data))
        self.status['series']['rejection']['disposition']['sha256']=v.digest(disposition); self.run_record(2)
        self.status['series']['pins']='f'*64
        with self.assertRaisesRegex(ValueError,'cross-pins'): v.series_check(self.status,self.p,self.root,frozenset())
        self.status['series']['pins']=v.digest(self.p)
        self.status['series']['id']='synthetic-original'
        with self.assertRaisesRegex(ValueError,'repeated'): v.series_check(self.status,self.p,self.root,frozenset())
        prior=v.read(old); prior['runs']=[]; prior['artifacts']=[ref]
        old.write_text(json.dumps(prior))
        self.status['series']=dict(id='complete-replacement',pins=v.digest(self.p),previous=dict(path='old.json',sha256=v.digest(old)),rejection=dict(exit=2,reason='interrupted series'))
        self.run_record(0)

    def test_resume(self):
        self.configure('G4'); self.run_record(0)
        plan=dict(id='fresh-series',gates=[dict(pins=dict(path='pins.json',sha256=v.digest(self.p)),series={})])
        def fake(index, ordinal, entry, role, *args):
            self.assertEqual(role,'reference' if entry['arm']=='s3b' else 'candidate')
            return 0,self.status['runs'][ordinal]
        work=self.root/'series'
        self.assertEqual(runner.collect(plan,self.root,work,fake),0)
        with self.subTest(case='valid resume'):
            self.assertEqual(runner.collect(plan,self.root,work,lambda *a: self.fail('relaunch on resume')),0)
        destination=work/'status-0.json'; cached=v.read(destination)
        prior=self.root/'prior.json'; prior.write_text(json.dumps(dict(cached,runs=[],artifacts=[self.status['runs'][0]['logs'][0]],series=dict(id='prior',pins=v.digest(self.p)))))
        for change in [dict(id='foreign'),dict(previous=dict(path='prior.json',sha256=v.digest(prior)),rejection=dict(exit=2,reason='interrupted predecessor'))]:
            destination.write_text(json.dumps(dict(cached,series=dict(cached['series'],**change))))
            with self.subTest(change=change), self.assertRaisesRegex(ValueError,'cached series'):
                runner.collect(plan,self.root,work,lambda *a: self.fail('relaunch on resume'))
        destination.write_text(json.dumps(cached))
        leaf=self.root/self.status['runs'][0]['logs'][0]['path']
        for change in ['changed','missing']:
            if change=='changed': leaf.write_text('changed artifact')
            else: leaf.unlink()
            with self.assertRaises((ValueError,OSError)): runner.collect(plan,self.root,work,fake)

    def test_runner_outcomes(self):
        self.configure('G4')
        for reference,candidate,exit_code,expected in [(10,10,0,75),(30,10,0,1),(30,30,101,101)]:
            for row in self.status['runs']: row['metrics']['fps']=reference if row['arm']=='s3b' else candidate
            self.run_record(75 if expected==75 else 1 if expected==1 else 0)
            plan=dict(id=str(expected),gates=[dict(pins=dict(path='pins.json',sha256=v.digest(self.p)),series={})])
            fake=Mock(side_effect=lambda index,ordinal,*args:(exit_code,self.status['runs'][ordinal]))
            self.assertEqual(runner.collect(plan,self.root,self.root/str(expected),fake),expected)
            self.assertEqual(fake.call_count,1 if exit_code else len(self.status['runs']))
        self.assertEqual(runner.collect(plan,self.root,self.root/'launch-error',Mock(side_effect=OSError('disposable launcher error'))),2)
        self.assertTrue((self.root/'launch-error/status-0.json').exists())

    def test_reference(self):
        self.configure('G18')
        self.pins['arms']=dict(reference='s4',candidate='s3b'); self.run_record(2)
    def test_protocol(self):
        self.configure('I4'); self.pins['protocol'].pop('warm_frames'); self.run_record(2)
    def test_g3_parameters(self):
        for missing in [('warm_frames',),('measured_frames',),('warm_frames','measured_frames')]:
            self.setUp(); self.configure('G3'); self.pins['protocol'].update(warm_frames=30,measured_frames=300); self.run_record(0)
            for key in missing: self.pins['protocol'].pop(key)
            with self.subTest(missing=missing): self.run_record(2)
    def test_g2_durations(self):
        self.configure('G2'); required={w:240 if w=='talk_recut' else 60 for w in v.CONTRACTS['G2'][2]}
        for duration in [60,240,{},dict(required,talk_recut=60),dict(required,feed_4x5=240),{k:d for k,d in required.items() if k!='talk_recut'}]:
            self.pins['protocol']['duration_s']=duration
            with self.subTest(duration=duration): self.run_record(2)
        self.pins['protocol']['duration_s']=required; self.run_record(0)
        self.pins['protocol'].pop('duration_s'); self.run_record(2)
    def test_denominator_precedence(self):
        for gate,condition in [('I4','busy'),('I4','assertion'),('R65-historical','assertion')]:
            self.setUp()
            if gate!='I4': self.configure(gate)
            self.status['runs'][0]['metrics']['mean_ms' if gate=='I4' else 'admission_waits']=0
            if condition=='busy': self.status['runs'][0]['preflight'][0]['processes'][0]['kind']='C'
            else: self.status['runs'][-1]['exit']=101
            with self.subTest(gate=gate,condition=condition): self.run_record(2)
        self.setUp(); self.configure('R65-historical')
        for row in self.status['runs']: row['metrics']['admission_waits']=0
        self.run_record(0)
    def test_ancestry_once(self):
        self.run_record(0); ref=self.status['runs'][0]['logs'][0]
        for depth in range(1,6):
            old=self.root/f'ancestor-{depth}.json'; old.write_text(json.dumps(dict(self.status,runs=[],artifacts=[ref])))
            self.status['series']=dict(id=f'replacement-{depth}',pins=v.digest(self.p),previous=dict(path=old.name,sha256=v.digest(old)),rejection=dict(exit=2,reason='interrupted predecessor'))
            with patch.object(v,'spec',wraps=v.spec) as checked:
                self.run_record(0)
                with self.subTest(depth=depth): self.assertEqual(checked.call_count,depth+1)
        prior=v.read(old); prior['series']['pins']='f'*64; old.write_text(json.dumps(prior))
        self.status['series']['previous']['sha256']=v.digest(old); self.run_record(2)
    def test_decision_edges(self):
        self.configure('G4')
        for candidate,expected in [([9,12,14],75),([5,12,14],1)]:
            for row in self.status['runs']: row['metrics']['fps']=([10,12,14] if row['arm']=='s3b' else candidate)[row['pair']]
            self.run_record(expected)
        self.setUp(); self.configure('R65-historical')
        for row in self.status['runs']: row['metrics']['admission_waits']=[0,100,100][row['pair']%3]
        self.run_record(2)

    def test_unsupported(self):
        self.pins['gate']='G16'; self.status['gate']='G16'; self.run_record(2)
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
            if change=='omit':
                self.pins['metrics'].pop('timeouts')
                for row in self.status['runs']:
                    row['metrics'].pop('timeouts')
            else:
                self.pins['metrics']['timeouts'].update(decision='R65',scope='paired_median')
            with self.subTest(change=change): self.run_record(2)
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
        paths = ['docs/PF1-PINS.json','docs/PF1-PINS.schema.json'] + [p.relative_to(repo).as_posix() for p in (repo/'docs/reports/pf1-s4/example').iterdir()]
        attrs = subprocess.check_output(['git','check-attr','eol','--']+paths, cwd=repo, text=True)
        subprocess.run(['git','-c','core.autocrlf=true','checkout-index','--all','--prefix='+str(self.root/'checkout')+'/'], cwd=repo, check=True, capture_output=True)
        for path in paths: self.assertEqual(subprocess.check_output(['git','show',':'+path], cwd=repo), (self.root/'checkout'/path).read_bytes())
        self.assertTrue(all(line.endswith(': lf') for line in attrs.splitlines()))
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
