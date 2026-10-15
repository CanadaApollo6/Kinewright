"""Portable PF1 artifact validation. Launchers and device collectors stay outside this module."""
import argparse
import hashlib
import itertools
import json
import math
from pathlib import Path
import re
import statistics

SCHEMA = json.loads((Path(__file__).resolve().parents[2] / 'docs/PF1-PINS.schema.json').read_text())
RULES = {
    'I4': {'mean_ms': ('ratio', 'ms', 1.05)},
    'G18': {'wall_ms': ('ratio', 'ms', 1.05), 'output_sha256': ('equal', 'sha256', 0)},
    'G15': {'settled_bytes': ('delta', 'byte', 4194304)},
    'G3': {'fps': ('each_ge', 'fps', 60)}, 'G11': {'underruns': ('zero', 'count', 0)},
    'G16': {'stall_ms': ('R50_abs', 'ms', 100)}, 'G14': {'held_ms': ('R50_abs', 'ms', 100)},
    **{gate: {'miss_pct': ('R50_abs', 'percent', 1), 'present_p95_ms': ('R50_abs', 'ms', 50)} for gate in ['G1', 'G2']},
    'G6': {'present_ratio': ('R50_abs', 'ratio', 3)}, 'G17': {'fallback_pct': ('each_le', 'percent', .1)},
    'G7b': {'encode_p99_ms': ('each_le', 'ms', 2)}, 'G7a': {'readbacks': ('zero', 'count', 0)},
    'G5': {'warm_seeks': ('zero', 'count', 0)}, 'G10': {'offset_ms': ('each_le', 'ms', 33)},
    'G13': {'threads': ('each_le', 'count', 20)}, 'L-4b': {'step_mean_ms': ('ratio', 'ms', 1.25)},
    'G8': {**{k: ('R50_abs', 'ms', v) for k, v in [('l1_ms', 40), ('l2_ms', 110), ('l3_ms', 20), ('l4a_ms', 20)]}, 'gop60_fps': ('each_ge', 'fps', 10), 'gop250_fps': ('each_ge', 'fps', 7), 'release_delivered': ('invariant', 'boolean', 1)},
    'D13': {**{k: ('invariant', 'boolean', 1) for k in ['retention', 'coverage', 'cleanup']}, 'rss_peak_minus_idle': ('report', 'byte', 0)},
    'PERFORMANCE': {'mean_ms': ('report', 'ms', 0), 'p95_ms': ('report', 'ms', 0)}, 'VM-D6': {'fps': ('each_ge', 'fps', 20)},
}
for gate in 'I1 C-5 C-3 I3 I9 I8 I18 I13 I15 I11 I12 I10 C-4 I1b I5 I6 I16 I17 G9 G12'.split():
    RULES[gate] = {'assertions_passed': ('invariant', 'boolean', 1)}

def require(condition, message):
    if not condition:
        raise ValueError(message)

def pairs(items):
    result = dict(items)
    require(len(result) == len(items), 'duplicate JSON key')
    return result

def read(path):
    return json.loads(Path(path).read_text(encoding='utf-8'), object_pairs_hook=pairs, parse_constant=lambda x: require(False, 'nonfinite ' + x))

def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()

def fingerprint(row):
    return hashlib.sha256(json.dumps({k: v for k, v in row.items() if k != 'fingerprint'}, sort_keys=True, separators=(',', ':'), allow_nan=False).encode()).hexdigest()

def schema(value, rule):
    if '$ref' in rule:
        return schema(value, SCHEMA['$defs'][rule['$ref'].split('/')[-1]])
    if 'anyOf' in rule:
        for choice in rule['anyOf']:
            try:
                schema(value, choice)
                return
            except ValueError:
                pass
        raise ValueError('no schema alternative matches')
    require(type(value) not in [int, float] or math.isfinite(value), 'nonfinite number')
    types = {'object': dict, 'array': list, 'string': str, 'integer': int, 'number': (int, float), 'boolean': bool}
    require('type' not in rule or isinstance(value, types[rule['type']]) and (rule['type'] not in ['number', 'integer'] or type(value) is not bool), 'wrong type')
    require('const' not in rule or type(value) is type(rule['const']) and value == rule['const'], 'wrong constant')
    require('enum' not in rule or value in rule['enum'], 'wrong enum')
    for key, test in [('minimum', lambda a, b: a >= b), ('maximum', lambda a, b: a <= b), ('exclusiveMinimum', lambda a, b: a > b)]:
        require(key not in rule or test(value, rule[key]), key)
    if isinstance(value, str):
        require(len(value) >= rule.get('minLength', 0) and ('pattern' not in rule or re.fullmatch(rule['pattern'], value)), 'invalid string')
    if isinstance(value, list):
        require(rule.get('minItems', 0) <= len(value) <= rule.get('maxItems', len(value)), 'array length')
        require(not rule.get('uniqueItems') or len({json.dumps(x, sort_keys=True) for x in value}) == len(value), 'duplicate array item')
        for item in value:
            schema(item, rule.get('items', {}))
    if isinstance(value, dict):
        require(set(rule.get('required', [])) <= value.keys() and len(value) >= rule.get('minProperties', 0), 'missing field')
        for key, item in value.items():
            child = rule.get('properties', {}).get(key, rule.get('additionalProperties', {}))
            require(child is not False, 'unknown field ' + key)
            schema(item, child if isinstance(child, dict) else {})

def artifact(root, ref, kind=None):
    schema(ref, SCHEMA['$defs']['ref'])
    path = (root / ref['path']).resolve()
    require(not Path(ref['path']).is_absolute() and path.is_relative_to(root.resolve()), 'artifact outside evidence root')
    require(digest(path) == ref['sha256'], 'changed artifact ' + ref['path'])
    if kind:
        data = read(path)
        schema(data, SCHEMA['$defs'][kind])
        return data

def gpu_check(gpu, pins, host, root):
    require(gpu['selected'] == pins['gpu']['selected'] and gpu['stack'] == pins['gpu']['stack'], 'selected adapter/stack mismatch')
    if host['os'] == 'linux' and gpu['selected']['backend'] == 'Vulkan':
        require(all(isinstance(gpu['stack'][k], str) for k in ['mesa', 'llvm', 'icd_manifest_sha256']), 'Linux Vulkan stack unavailable')
    if pins['lane'] == 'LH':
        require(bool(gpu['selected'].get('uuid')) and gpu['policy']['kind'] == 'hardware', 'LH selected UUID/hardware required')
    policy = gpu['policy']
    require({k: v for k, v in policy.items() if k != 'observed'} == {k: v for k, v in pins['gpu']['policy'].items() if k != 'observed'}, 'policy mismatch')
    if policy['kind'] == 'hardware':
        approval = artifact(root, policy['approval'], 'policy_approval')
        require(approval['requested'] == policy['requested'] and isinstance(gpu['telemetry'], list), 'approval/telemetry missing')
        artifact(root, approval['record'])
        observations = [policy['observed']] + gpu['telemetry']
        require(all(x['timestamp_s'] >= approval['issued_s'] for x in observations), 'approval postdates observations')
        for observation in observations:
            require(observation['pstate'] in policy['requested']['pstates'], 'pstate outside policy')
            require(all(bounds[0] <= observation[key] <= bounds[1] for key, bounds in policy['requested'].items() if key != 'pstates'), 'clock/power outside policy')

def preflight(samples, uuid):
    schema(samples, {'type': 'array', 'minItems': 3, 'maxItems': 3, 'items': {'$ref': '#/$defs/preflight'}})
    require(all(s['uuid'] == uuid for s in samples) and all(5 <= b['timestamp_s'] - a['timestamp_s'] <= 6 for a, b in zip(samples, samples[1:])), 'malformed preflight identity/spacing')
    return all(s['utilization_pct'] >= 80 for s in samples) or any(not p['owned'] and (p['kind'] == 'C' or p['memory_bytes'] > 2147483648) for s in samples for p in s['processes'])

def spec(pins_path, root):
    pins = read(pins_path)
    schema(pins, SCHEMA['$defs']['pins'])
    inputs, builds, host = [artifact(root, pins[k], k) for k in ['inputs', 'builds', 'host']]
    require(set(inputs) == set(builds) and set(pins['arms'].values()) <= builds.keys() and len(set(pins['arms'].values())) == 2, 'arm manifest mismatch')
    for arm, build in builds.items():
        source = artifact(root, build['production'], 'source')
        approval = artifact(root, build['approval'], 'approval')
        require(approval['basis'] == build['basis'] and approval['source_sha256'] == build['production']['sha256'], 'reference approval mismatch')
        for ref in source['files'] + artifact(root, build['harness'], 'manifest')['files'] + build['test_overlays'] + [build['binary'], build['lockfile'], approval['record']]:
            artifact(root, ref)
        require(inputs[arm] == inputs[pins['arms']['candidate']], 'paired inputs differ')
        for workload in inputs[arm].values():
            for key in ['workload', 'media', 'document']:
                artifact(root, workload[key])
    gate, protocol = pins['gate'], pins['protocol']
    reference = builds[pins['arms']['reference']]['basis']
    expected = 'S0' if gate in ['G18', 'G15', 'VM-D6'] else 'oracle' if gate == 'D13' else 'accepted-S3b'
    if gate.startswith('R65-'):
        expected, candidate = {'R65-historical': ('43b2625', '62a29b1'), 'R65-intervening': ('62a29b1', 'accepted-S3b'), 'R65-cumulative': ('43b2625', 'accepted-S3b')}[gate]
        require(builds[pins['arms']['candidate']]['basis'] == candidate and {'admission_waits', 'windows_filled'} <= pins['metrics'].keys(), 'counter arms/rows')
        rules = {k: ('zero' if k.startswith('zero_') else 'R65', 'count', 0) for k in pins['metrics']}
    else:
        require(gate in RULES or gate == 'G4', 'unregistered gate')
        rules = RULES.get(gate, {'fps': ('each_ge', 'fps', 0)})
    require(reference == expected, 'wrong reference basis')
    require(all(build['harness'] == builds[pins['arms']['candidate']]['harness'] for build in builds.values()), 'paired harness mismatch')
    required_workloads = {'G4': {'typical_1080p', 'explainer_16x9', 'blend_heavy_1080p', 'reel_9x16'}, 'G2': {'explainer_16x9', 'reel_9x16', 'feed_4x5', 'talk_recut'}}
    require(gate not in required_workloads or set(inputs[pins['arms']['candidate']]) == required_workloads[gate], 'missing required workload')
    for key, rule in pins['metrics'].items():
        require(rule['scope'] == ('report' if rule['decision'] == 'report' else 'paired_median' if rule['decision'] in ['ratio', 'R65'] else 'invariant' if rule['decision'] in ['invariant', 'zero', 'equal'] else 'each_run'), 'metric scope mismatch')
        require(key in rules and (rule['decision'], rule['unit'], rule['limit']) == rules[key], 'metric decision/unit/threshold mismatch')
    require(pins['metrics'].keys() == rules.keys(), 'missing required metric')
    require(gate not in ['G18', 'G3', 'G1', 'G2', 'G7b', 'G14'] or pins['lane'] == 'LH', 'required LH lane')
    require(protocol['runs'] == (18 if gate.startswith('R65-') else 3), 'required run count')
    expected_rows = {(w, pair, arm) for w in inputs[pins['arms']['candidate']] for pair in range(protocol['runs']) for arm in builds}
    require({(r['workload'], r['pair'], r['arm']) for r in protocol['order']} == expected_rows and len(protocol['order']) == len(expected_rows), 'incomplete/duplicate order')
    require(all(len({r['seed'] for r in protocol['order'] if r['workload'] == w and r['pair'] == pair}) == 1 for w, pair, _ in expected_rows), 'paired seeds differ')
    for row in protocol['order']:
        require(row['seed'] in inputs[row['arm']][row['workload']]['seeds'], 'seed not pinned')
    if gate.startswith('R65-'):
        require({b['basis'] for b in builds.values()} == {'43b2625', '62a29b1', 'accepted-S3b'} and len(builds) == 3, 'three counter arms required')
        for workload in inputs[pins['arms']['candidate']]:
            orders = [r for r in protocol['order'] if r['workload'] == workload]
            require(all({tuple(r['arm'] for r in orders[i:i+3]) for i in range(seed*3, len(orders), 9)} == set(itertools.permutations(builds)) for seed in range(3)), 'unbalanced counter rotations')
    if gate == 'D13':
        require(all(k in protocol for k in ['B', 'reader_cap', 'd_bytes', 'children', 'cycles', 'calibration', 'control']), 'D13 calibration missing')
        for key in ['calibration', 'control']:
            for ref in artifact(root, protocol[key], 'manifest')['files']:
                artifact(root, ref)
    require(gate not in ['G3'] or pins['protocol']['name'] == 'P-comp', 'unpaced protocol required')
    gpu_check(pins['gpu'], pins, host, root)
    return pins, builds, host

def evaluate(pins_path, run_path, root):
    pins, builds, host = spec(pins_path, root)
    status = read(run_path)
    schema(status, SCHEMA['$defs']['status'])
    require((status['lane'], status['gate']) == (pins['lane'], pins['gate']), 'status lane/gate mismatch')
    rows, order = status['runs'], pins['protocol']['order']
    require(len(rows) == len(order), 'incomplete attempts')
    busy = False
    for row, expected in zip(rows, order):
        require(all(row[k] == v for k, v in expected.items()) and row['attempt'] == 0 and row['supersedes'] == '', 'attempt/order mismatch; preserve failures in a new ruled series')
        build = builds[row['arm']]
        identity = {k: build[k]['sha256'] for k in ['binary', 'production', 'harness']}
        require(row['identity'] == dict(identity, pins=digest(pins_path), host=pins['host']['sha256']), 'stale identity')
        require(row['fingerprint'] == fingerprint(row), 'stale status fingerprint')
        for ref in row['logs']:
            artifact(root, ref)
        gpu_check(row['gpu'], pins, host, root)
        telemetry = row['gpu']['telemetry']
        require(row['started_s'] < row['ended_s'], 'invalid run interval')
        if row['gpu']['policy']['kind'] == 'hardware':
            require(isinstance(telemetry, list) and telemetry[0]['timestamp_s'] <= row['started_s'] and telemetry[-1]['timestamp_s'] >= row['ended_s'] and all(0 < b['timestamp_s']-a['timestamp_s'] <= pins['protocol']['sample_ms']/1000+.002 for a,b in zip(telemetry,telemetry[1:])), 'policy sampling coverage missing')
        require(host['os'] == 'windows' or isinstance(row['cpu']['load_average'], (int, float)), 'load average unavailable outside Windows')
        busy |= preflight(row['preflight'], pins['gpu']['selected']['uuid']) if pins['lane'] == 'LH' else False
        require(row['validity'] != 'invalid_evidence', 'invalid attempt')
    if busy:
        return 75
    require(all(r['validity'] != 'pending_gpu_busy' for r in rows), 'unsubstantiated busy status')
    if any(r['exit'] != 0 for r in rows):
        return 1
    verdicts = []
    for workload in {r['workload'] for r in rows}:
        arm_rows = {side: [r for r in rows if r['workload'] == workload and r['arm'] == arm] for side, arm in pins['arms'].items()}
        for metric, rule in pins['metrics'].items():
            require(all(set(r['metrics']) == set(pins['metrics']) for r in rows), 'missing/extra metric values')
            values = {side: [r['metrics'][metric] for r in sorted(data, key=lambda r: r['pair'])] for side, data in arm_rows.items()}
            a, b = values['reference'], values['candidate']
            decision, limit = rule['decision'], rule['limit']
            require(all((type(x) is bool if rule['unit'] == 'boolean' else isinstance(x, str) if rule['unit'] == 'sha256' else type(x) in [int, float] and x >= 0 and (rule['unit'] != 'percent' or x <= 100)) for x in a + b), 'invalid metric value/unit')
            if decision in ['ratio', 'R65'] and (decision == 'ratio' or statistics.median(a) >= 20):
                require(all(x > 0 for x in a), 'zero reference denominator')
                ratio = statistics.median(y / x for x, y in zip(a, b))
                verdicts.append(0 if (.95 <= ratio <= 1.05 if decision == 'R65' else ratio <= limit) else 1)
            elif decision == 'R65':
                verdicts.append(0 if abs(statistics.mean(b) - statistics.mean(a)) <= max(1, .25 * statistics.mean(a)) and max(min(a), min(b)) <= min(max(a), max(b)) else 1)
            elif decision in ['R50_abs', 'each_ge']:
                limit = (24 if workload in ['typical_1080p', 'explainer_16x9'] else 15) if pins['gate'] == 'G4' else limit
                fails = (lambda x: x < limit) if decision == 'each_ge' else (lambda x: x > limit)
                verdicts.extend(0 if not fails(y) else 1 if not fails(x) or (x-y if decision == 'each_ge' else y-x) > max(a)-min(a) else 75 for x, y in zip(a, b))
            elif decision != 'report':
                passed = {'equal': lambda: a == b, 'zero': lambda: all(x == 0 for x in a+b), 'invariant': lambda: all(x is True for x in a+b), 'each_le': lambda: max(b) <= limit, 'delta': lambda: all(y <= x+limit for x, y in zip(a, b))}[decision]()
                verdicts.append(0 if passed else 1)
    return 1 if 1 in verdicts else 75 if 75 in verdicts or pins['gate'] in ['PERFORMANCE', 'R65-cumulative'] or any(r['validity'] == 'environment_limited_at_load' for r in rows) else 0

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--pins', type=Path, required=True)
    parser.add_argument('--run', type=Path)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--spec-only', action='store_true', help='validate launch inputs, not PF1 acceptance')
    args = parser.parse_args()
    try:
        code = 0 if args.spec_only and spec(args.pins, args.root) else evaluate(args.pins, args.run, args.root)
    except (ValueError, OSError, KeyError, TypeError) as error:
        print('invalid_evidence: ' + str(error))
        code = 2
    print({0: 'launch_spec_valid' if args.spec_only else 'gate_pass', 1: 'gate_failed', 2: 'invalid_evidence', 75: 'pending_or_report_only'}[code])
    raise SystemExit(code)
