"""Synthetic artifact witnesses; --mutation NAME changes only the named decision."""
import copy
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
}
if '--mutation' in sys.argv:
    index = sys.argv.index('--mutation'); name = sys.argv[index+1]; del sys.argv[index:index+2]
    old, new = MUTATIONS[name]; source = Path(v.__file__).read_text(); assert old in source
    exec(compile(source.replace(old, new), v.__file__, 'exec'), v.__dict__)

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
    def test_exit(self): self.status['runs'][0]['exit'] = 1; self.run_record(1)
    def test_environment(self): self.status['runs'][0]['validity'] = 'environment_limited_at_load'; self.run_record(75)
    def test_green_and_cli(self):
        self.run_record(0)
        command = [sys.executable, str(Path(v.__file__)), '--pins', str(self.p), '--run', str(self.root/'status.json'), '--root', str(self.root)]
        self.assertEqual(subprocess.run(command, capture_output=True).returncode, 0)

if __name__ == '__main__': unittest.main()
