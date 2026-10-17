import argparse
import hashlib
import itertools
import json
import math
from pathlib import Path
import re
import statistics

SCHEMA = json.loads((Path(__file__).resolve().parents[2] / 'docs/PF1-PINS.schema.json').read_text())
PLAY = set('typical_1080p blend_heavy_1080p explainer_16x9 reel_9x16 feed_4x5 talk_recut'.split())
CONTRACTS = {
    'D13': ('LL', 'D13', {'seek_gop60'}, {'children': 3, 'cycles': 30, 'B': 16, 'reader_cap': 2}),
    'I4': ('LL LH', 'P-comp', {'typical_1080p'}, {'warm_frames': 30, 'measured_frames': 300}),
    'G18': ('LH', 'export', {'typical_1080p'}, {'export_clips': 24, 'frames': 120}),
    'G15': ('LL', 'P-rss', PLAY | {'title_only'}, {'children': 3}),
    'G3': ('LH', 'P-comp', {'blend_heavy_1080p'}, {'warm_frames': 30, 'measured_frames': 300}),
    'G4': ('LL', 'native-display', PLAY - {'feed_4x5', 'talk_recut'}, {'warmup_ms': 2000, 'duration_s': 60}),
    'G2': ('LH', 'native-display', PLAY - {'typical_1080p', 'blend_heavy_1080p'}, {'warmup_ms': 2000, 'duration_s': {w: 240 if w == 'talk_recut' else 60 for w in PLAY - {'typical_1080p', 'blend_heavy_1080p'}}}),
    'G1': ('LH', 'P-play', {'typical_1080p'}, {'warmup_ms': 2000, 'duration_s': 60}),
    'G6': ('LL LH', 'P-play', {'typical_1080p'}, {'warmup_ms': 2000, 'duration_s': 60}),
    'VM-D6': ('VM-W', 'VM', {'typical_1080p'}, {}),
    'G7a': ('CI-L CI-W', 'correctness', {'app_readback'}, {}),
    **{gate: ('LL', 'counters', {'seek_gop60', 'explainer_16x9'}, {}) for gate in ['R65-historical', 'R65-intervening', 'R65-cumulative']},
}
COUNTERS = {name: ('R65', 'count', 0) for name in 'admission_waits windows_filled count_run_decoded count_run_converted count_refill_decoded count_refill_converted count_back_hits count_back_refills count_back_hits_preconverted count_refills_started count_kept_handoff_redecode conv_t_n conv_w_n'.split()}
COUNTERS.update({name: ('zero', 'count', 0) for name in ['timeouts', 'count_idle_discard', 'guard_trips']})
RULES = {
    'D13': {**{k: ('invariant', 'boolean', 0) for k in ['oracle_ok', 'cleanup_ok']}, **{k: ('report', 'byte', 0) for k in ['idle_bytes', 'peak_bytes', 'peak_minus_idle_bytes', 'u_bytes']}},
    'I4': {'mean_ms': ('ratio', 'ms', 1.05)},
    'G18': {'wall_ms': ('ratio', 'ms', 1.05), 'output_sha256': ('equal', 'sha256', 0)},
    'G15': {'settled_bytes': ('delta', 'byte', 4194304)},
    'G3': {'fps': ('each_ge', 'fps', 60)},
    **{gate: {'miss_pct': ('R50_abs', 'percent', 1), 'present_p95_ms': ('R50_abs', 'ms', 50)} for gate in ['G1', 'G2']},
    'G6': {'present_ratio': ('R50_abs', 'ratio', 3)},
    'G7a': {'readbacks': ('zero', 'count', 0)}, 'VM-D6': {'fps': ('each_ge', 'fps', 20)},
}

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
    default = gpu['policy']['kind'] == 'recorded-default'
    if host['os'] == 'linux' and gpu['selected']['backend'] == 'Vulkan' and not default:
        require(all(isinstance(gpu['stack'][k], str) for k in ['mesa', 'llvm', 'icd_manifest_sha256']), 'Linux Vulkan stack unavailable')
    if pins['lane'] == 'LH':
        require(bool(gpu['selected'].get('uuid')) and gpu['policy']['kind'] in ['hardware', 'recorded-default'], 'LH selected UUID/hardware required')
    policy = gpu['policy']
    require({k: v for k, v in policy.items() if k != 'observed'} == {k: v for k, v in pins['gpu']['policy'].items() if k != 'observed'}, 'policy mismatch')
    if default:
        require(pins['lane'] == 'LH' and pins['gate'] in ['G1', 'G2', 'G14', 'Q3-G1', 'Q3-G14'], 'gate not covered by Q1(b)')
        artifact(root, policy['ruling'])
        ruling = (root/policy['ruling']['path']).read_text()
        require('## R89-2:' in ruling and '- Change no GPU setting.' in ruling, 'missing recorded-default ruling')
        require(gpu['selected']['vendor_id'] == 0x10de and gpu['selected']['driver'] == 'NVIDIA' and isinstance(gpu['stack']['icd_manifest_sha256'], str), 'missing proprietary LH stack')
        require(all(isinstance(gpu['stack'][k], dict) and gpu['stack'][k]['status'] == 'unavailable' for k in ['mesa', 'llvm']), 'Mesa/LLVM must be marked inapplicable')
        require(isinstance(gpu['telemetry'], list), 'recorded-default observations required')
        for observation in gpu['telemetry']:
            schema(observation, SCHEMA['$defs']['default_observation'])
            require(observation['uuid'] == gpu['selected']['uuid'], 'uncorrelated default-policy observation')
    if policy['kind'] == 'hardware':
        approval = artifact(root, policy['approval'], 'policy_approval')
        require(approval['requested'] == policy['requested'] and isinstance(gpu['telemetry'], list), 'approval/telemetry missing')
        artifact(root, approval['record'])
        observations = [policy['observed']] + gpu['telemetry']
        require(all(x['timestamp_s'] >= approval['issued_s'] for x in observations), 'approval postdates observations')
        for observation in observations:
            require(observation['pstate'] in policy['requested']['pstates'], 'pstate outside policy')
            require(all(bounds[0] <= observation[key] <= bounds[1] for key, bounds in policy['requested'].items() if key != 'pstates'), 'clock/power outside policy')

def preflight(samples, uuid, started_s):
    schema(samples, {'type': 'array', 'minItems': 3, 'maxItems': 3, 'items': {'$ref': '#/$defs/preflight'}})
    require(all(s['uuid'] == uuid for s in samples) and all(5 <= b['timestamp_s'] - a['timestamp_s'] <= 6 for a, b in zip(samples, samples[1:])), 'malformed preflight identity/spacing')
    require(0 <= started_s - samples[-1]['timestamp_s'] <= 10, 'future/stale preflight; launch within ten seconds of final sample')
    return all(s['utilization_pct'] >= 80 for s in samples) or any(not p['owned'] and (p['kind'] == 'C' or p['memory_bytes'] > 2147483648) for s in samples for p in s['processes'])

def frozen_input(root, key, value):
    for kind in ['workload', 'media', 'document']:
        artifact(root, value[kind])
    require(read(root/value['workload']['path']) == dict(key=key, document=value['document'], media=value['media']), 'substituted input manifest')
    for ref in read(root/value['media']['path'])['files']:
        path = root/ref['path']
        require(not path.is_symlink() and path.is_file() and path.suffix.lower() in ['.mp4', '.wav'], 'frozen media must be a regular allowlisted file')
        with path.open('rb') as stream:
            header = stream.read(12)
        require(header[4:8] == b'ftyp' if path.suffix.lower() == '.mp4' else header[:4] == b'RIFF' and header[8:12] == b'WAVE', 'indirect/unsupported frozen media format')
        artifact(root, ref)
    return {kind: value[kind]['sha256'] for kind in ['workload', 'media', 'document']}

def d13_check(record, pins):
    protocol, metrics = pins['protocol'], record['metrics']
    require(record['calibration_sha256'] == protocol['calibration']['sha256'] and metrics['u_bytes'] == 30*protocol['d_bytes'], 'D13 calibration/U mismatch')
    require(metrics['peak_bytes'] >= metrics['idle_bytes'] and metrics['peak_minus_idle_bytes'] == metrics['peak_bytes']-metrics['idle_bytes'], 'D13 RSS report mismatch')
    rows = record['d13_intervals']
    require([(r['cycle'], r['target']) for r in rows] == [(cycle, target) for cycle in range(30) for target in [180, 1200]], 'D13 missing/duplicate cancellation')
    require(len({r['version'] for r in rows}) == 60 and all(type(r['version']) is int and r['version'] > 0 for r in rows), 'D13 repeated/invalid applied plan')
    require(all(b['ack_us'] > a['ack_us'] and b['version'] > a['version'] for a, b in zip(rows, rows[1:])), 'D13 ack/version order')
    require(all(set(a['identities']) == set(b['identities']) for a, b in zip(rows[::2], rows[1::2])), 'D13 changed cycle identities')
    for row in rows:
        times, ids, ack = row['sample_us'], row['identities'], row['ack_us']
        require(len(times) == 2 and all(type(t) is int and t > 0 for t in times) and type(ack) is int and ack >= 0, 'D13 timestamp shape')
        require(times[0] >= ack+100000 and times[1] >= ack+200000 and times[1]-times[0] >= 100000, 'D13 missing two-sample live interval')
        require(ids and len(ids) <= protocol['reader_cap']*(protocol['B']-1) and len(ids) == len(set(ids)) and all(type(i) is int and i > 0 for i in ids), 'D13 retained identities missing/over cap')


def spec(pins_path, root):
    pins = read(pins_path)
    schema(pins, SCHEMA['$defs']['pins'])
    inputs, builds, host = [artifact(root, pins[k], k) for k in ['inputs', 'builds', 'host']]
    single = pins['gate'] in ['G7a', 'D13']
    require(set(inputs) == set(builds) and set(pins['arms'].values()) <= builds.keys() and len(set(pins['arms'].values())) == (1 if single else 2), 'arm manifest mismatch')
    require(('reference' not in pins['arms']) == single and len(builds) == (1 if single else 3 if pins['gate'].startswith('R65-') else 2), 'unexpected comparator')
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
        if pins['gate'] in ['G4', 'G15', 'D13']:
            for key, workload in inputs[arm].items(): frozen_input(root, key, workload)
    gate, protocol = pins['gate'], pins['protocol']
    require(gate in CONTRACTS, 'unsupported/incomplete gate contract')
    lanes, name, workloads, parameters = CONTRACTS[gate]
    require(pins['lane'] in lanes.split() and protocol['name'] == name and set(inputs[pins['arms']['candidate']]) == workloads, 'gate lane/protocol/workloads mismatch')
    require(all(protocol.get(key) == value for key, value in parameters.items()), 'essential protocol parameter missing')
    reference = builds[pins['arms'].get('reference', pins['arms']['candidate'])]['basis']
    expected = 'S0' if gate in ['G18', 'G15', 'VM-D6'] else 'oracle' if gate == 'D13' else 'accepted-S3b'
    if gate.startswith('R65-'):
        expected, candidate = {'R65-historical': ('43b2625', '62a29b1'), 'R65-intervening': ('62a29b1', 'accepted-S3b'), 'R65-cumulative': ('43b2625', 'accepted-S3b')}[gate]
        require(builds[pins['arms']['candidate']]['basis'] == candidate, 'counter arms')
        rules = COUNTERS
    else:
        rules = RULES.get(gate, {'fps': ('each_ge', 'fps', 0)})
    require(single or reference == expected, 'wrong reference basis')
    require(all(build['harness'] == builds[pins['arms']['candidate']]['harness'] for build in builds.values()), 'paired harness mismatch')
    for key, rule in pins['metrics'].items():
        require(rule['scope'] == ('report' if rule['decision'] == 'report' else 'paired_median' if rule['decision'] in ['ratio', 'R65'] else 'invariant' if rule['decision'] in ['invariant', 'zero', 'equal'] else 'each_run'), 'metric scope mismatch')
        require(key in rules and (rule['decision'], rule['unit'], rule['limit']) == rules[key], 'metric decision/unit/threshold mismatch')
    require(pins['metrics'].keys() == rules.keys(), 'missing required metric')
    require(protocol['runs'] == (1 if gate == 'G7a' else 18 if gate.startswith('R65-') else 3), 'required run count')
    expected_rows = {(w, pair, arm) for w in inputs[pins['arms']['candidate']] for pair in range(protocol['runs']) for arm in builds}
    require({(r['workload'], r['pair'], r['arm']) for r in protocol['order']} == expected_rows and len(protocol['order']) == len(expected_rows), 'incomplete/duplicate order')
    require(all(len({r['seed'] for r in protocol['order'] if r['workload'] == w and r['pair'] == pair}) == 1 for w, pair, _ in expected_rows), 'paired seeds differ')
    for row in protocol['order']:
        require(row['seed'] in inputs[row['arm']][row['workload']]['seeds'], 'seed not pinned')
    if gate.startswith('R65-'):
        require({b['basis'] for b in builds.values()} == {'43b2625', '62a29b1', 'accepted-S3b'} and len(builds) == 3, 'three counter arms required')
        for workload in inputs[pins['arms']['candidate']]:
            orders = [r for r in protocol['order'] if r['workload'] == workload]
            seeds = inputs[pins['arms']['candidate']][workload]['seeds']
            require(len(seeds) == len(set(seeds)) == 3 and all(r['pair'] == i // 3 and r['seed'] == seeds[(i // 3) % 3] for i, r in enumerate(orders)), 'six three-seed rotation schedule required')
            require(all({tuple(r['arm'] for r in orders[i:i+3]) for i in range(seed*3, len(orders), 9)} == set(itertools.permutations(builds)) for seed in range(3)), 'unbalanced counter rotations')
    if gate == 'D13':
        require(protocol['sample_ms'] == 100, 'D13 requires 100 ms RSS samples')
        artifact(root, protocol['calibration'])
        calibration = read(root/protocol['calibration']['path'])
        media = read(root/inputs[pins['arms']['candidate']]['seek_gop60']['media']['path'])['files']
        require(len(media) == 1 and calibration['media_sha256'] == media[0]['sha256'], 'D13 calibration source mismatch')
        require(calibration['method'] == 'safe measured plane span union' and calibration['B'] == 16 and calibration['reader_cap'] == 2 and type(calibration['d_bytes']) is int and calibration['d_bytes'] == protocol['d_bytes'], 'D13 independent calibration mismatch')
        require(calibration['frames'] == 1800 and calibration['shape']['width'] == 1920 and calibration['shape']['height'] == 1080, 'D13 calibration shape mismatch')
    gpu_check(pins['gpu'], pins, host, root)
    return pins, builds, host

def series_check(status, pins_path, root, seen):
    series = status['series']
    require(series['id'] not in seen and len(seen) < 16 and series['pins'] == digest(pins_path), 'repeated/cross-pins series')
    if 'previous' not in series:
        require('rejection' not in series, 'rejection without predecessor')
        return
    artifact(root, series['previous'])
    previous = read(root / series['previous']['path'])
    schema(previous, SCHEMA['$defs']['status'])
    require(previous['runs'] or previous.get('artifacts'), 'predecessor has no attempt artifacts')
    for ref in previous.get('artifacts', []):
        artifact(root, ref)
    for row in previous['runs']:
        require(row['fingerprint'] == fingerprint(row), 'changed predecessor row')
        for ref in row['logs']:
            artifact(root, ref)
    # Validate ancestry even when an incomplete predecessor fails before its rows.
    series_check(previous, pins_path, root, seen | {series['id']})
    try:
        code = evaluate(pins_path, root / series['previous']['path'], root, seen | {series['id']}, ancestry_checked=True)
    except (ValueError, OSError, KeyError, TypeError):
        code = 2
    rejection = series.get('rejection', {})
    require(code != 0 and rejection.get('exit') == code and bool(rejection.get('reason')), 'missing/incorrect rejection reason')
    if code == 1 or code in [2, 101] and previous['runs']:
        disposition = artifact(root, rejection['disposition'], 'disposition')
        artifact(root, disposition['record'])
        require(disposition['previous_sha256'] == series['previous']['sha256'], 'disposition targets another series')

def evaluate(pins_path, run_path, root, seen=frozenset(), ancestry_checked=False):
    pins, builds, host = spec(pins_path, root)
    status = read(run_path)
    schema(status, SCHEMA['$defs']['status'])
    for ref in status.get('artifacts', []):
        artifact(root, ref)
    if not ancestry_checked: series_check(status, pins_path, root, seen)
    require((status['lane'], status['gate']) == (pins['lane'], pins['gate']), 'status lane/gate mismatch')
    if 'failure' in status:
        failure = status['failure']
        artifact(root, failure['receipt'])
        receipt = read(root / failure['receipt']['path'])
        require(receipt['exit'] == failure['exit'] and bool(receipt['reason']), 'failure receipt mismatch')
        require(receipt['pins'] == digest(pins_path), 'cross-pins failure receipt')
        require(len(status['runs']) < len(pins['protocol']['order']) and all(receipt[k] == x for k, x in pins['protocol']['order'][len(status['runs'])].items()), 'failure entry/order mismatch')
        for ref in receipt['logs']:
            artifact(root, ref)
        require(failure['exit'] != 101 or receipt['cleanup']['reaped'] and receipt['cleanup']['returncode'] == 101, 'unsubstantiated assertion exit')
        return failure['exit']
    rows, order = status['runs'], pins['protocol']['order']
    require(len(rows) == len(order), 'incomplete attempts')
    busy = False
    native_pids = set()
    for row, expected in zip(rows, order):
        require(all(row[k] == v for k, v in expected.items()) and row['attempt'] == 0 and row['supersedes'] == '', 'attempt/order mismatch; preserve failures in a new ruled series')
        build = builds[row['arm']]
        identity = {k: build[k]['sha256'] for k in ['binary', 'production', 'harness']}
        require(row['identity'] == dict(identity, pins=digest(pins_path), host=pins['host']['sha256']), 'stale identity')
        require(row['fingerprint'] == fingerprint(row), 'stale status fingerprint')
        for ref in row['logs']:
            artifact(root, ref)
        if pins['gate'] in ['G4', 'G15', 'D13']:
            from launch import TESTS, host_check
            receipt = read(root/row['logs'][-1]['path'])
            host_check(host, receipt['host'])
            command = ['timeout', '--kill-after=1', '570', 'nice', '-n', '10', str(root/build['binary']['path']), '--exact', TESTS[pins['gate']], '--ignored', '--nocapture', '--test-threads=1']
            require(receipt['exit'] == row['exit'] == 0 and receipt['cleanup']['reaped'] is True and receipt['cleanup']['returncode'] == 0 and receipt['command'] == row['command'] == command and receipt['pins'] == digest(pins_path) and receipt['token'] == row['environment']['PF1_TOKEN'], 'pass receipt/command mismatch')
            require(all(receipt[k] == value for k, value in expected.items()) and row['environment']['PF1_SEED'] == str(expected['seed']), 'pass receipt seed/order mismatch')
            path = Path(row['environment']['PF1_RECORD'])
            require(path.resolve().is_relative_to(root.resolve()), 'collector outside evidence root')
            matches = [ref for ref in row['logs'] if root/ref['path'] == path]
            require(len(matches) == 1, 'missing/duplicate collector artifact')
            ref = matches[0]
            artifact(root, ref)
            raw = read(path)
            protocol = dict(name=pins['protocol']['name'],poll_ms=5,**CONTRACTS[pins['gate']][3])
            if pins['gate'] == 'D13': protocol['d_bytes'] = pins['protocol']['d_bytes']
            require(raw['protocol'] == protocol, 'collector protocol mismatch')
            require(raw['valid'] is True and raw['metrics'] == row['metrics'] and raw['selected'] == row['gpu']['selected'], 'collector final mismatch')
            require(raw['token'] == row['environment']['PF1_TOKEN'] and row['started_s'] <= raw['timestamp_s'] <= row['ended_s'], 'collector publication mismatch')
            require(raw['consumed'] == frozen_input(root, row['workload'], read(root/pins['inputs']['path'])[row['arm']][row['workload']]), 'collector input mismatch')
            if pins['gate'] != 'G4':
                from launch import native_check
                native_check(raw)
                require(raw['publisher_pid'] not in native_pids, 'native child reused across rows')
                native_pids.add(raw['publisher_pid'])
            if pins['gate'] == 'D13': d13_check(raw, pins)
        gpu_check(row['gpu'], pins, host, root)
        telemetry = row['gpu']['telemetry']
        require(row['started_s'] < row['ended_s'], 'invalid run interval')
        if row['gpu']['policy']['kind'] == 'hardware':
            require(isinstance(telemetry, list) and telemetry[0]['timestamp_s'] <= row['started_s'] and telemetry[-1]['timestamp_s'] >= row['ended_s'] and all(0 < b['timestamp_s']-a['timestamp_s'] <= pins['protocol']['sample_ms']/1000+.002 for a,b in zip(telemetry,telemetry[1:])), 'policy sampling coverage missing')
        if row['gpu']['policy']['kind'] == 'recorded-default':
            require(pins['protocol']['sample_ms'] == 5000 and len(telemetry) >= 3 and telemetry[0]['timestamp_s'] <= row['started_s'] and telemetry[-1]['timestamp_s'] >= row['ended_s'] and any(row['started_s'] < x['timestamp_s'] < row['ended_s'] for x in telemetry), 'default-policy before/during/after coverage missing')
            require(all(0 < b['timestamp_s']-a['timestamp_s'] <= 6 for a,b in zip(telemetry,telemetry[1:])), 'default-policy sampling gap')
        require(host['os'] == 'windows' or isinstance(row['cpu']['load_average'], (int, float)), 'load average unavailable outside Windows')
        busy |= preflight(row['preflight'], pins['gpu']['selected']['uuid'], row['started_s']) if pins['lane'] == 'LH' else False
        require(row['validity'] != 'invalid_evidence', 'invalid attempt')
        require(set(row['metrics']) == set(pins['metrics']), 'missing/extra metric values')
        for metric, rule in pins['metrics'].items():
            value = row['metrics'][metric]
            require(type(value) is bool if rule['unit'] == 'boolean' else isinstance(value, str) and re.fullmatch(r'[0-9a-f]{64}', value) if rule['unit'] == 'sha256' else type(value) in [int, float] and math.isfinite(value) and value >= 0 and (rule['unit'] != 'percent' or value <= 100), 'invalid metric value/unit')
    verdicts = []
    for workload in {r['workload'] for r in rows}:
        arm_rows = {side: [r for r in rows if r['workload'] == workload and r['arm'] == arm] for side, arm in pins['arms'].items()}
        for metric, rule in pins['metrics'].items():
            values = {side: [r['metrics'][metric] for r in sorted(data, key=lambda r: r['pair'])] for side, data in arm_rows.items()}
            b = values['candidate']
            a = values.get('reference', b)
            decision, limit = rule['decision'], rule['limit']
            if decision in ['ratio', 'R65'] and (decision == 'ratio' or statistics.median(a) >= 20):
                require(all(x > 0 for x in a), 'zero reference denominator')
                ratio = statistics.median(y / x for x, y in zip(a, b))
                verdicts.append(0 if (.95 <= ratio <= 1.05 if decision == 'R65' else ratio <= limit) else 1)
            elif decision == 'R65':
                verdicts.append(0 if abs(statistics.mean(b) - statistics.mean(a)) <= max(1, .25 * statistics.mean(a)) and max(min(a), min(b)) <= min(max(a), max(b)) else 1)
            elif decision in ['R50_abs', 'each_ge']:
                limit = (24 if workload in ['typical_1080p', 'explainer_16x9'] else 15) if pins['gate'] == 'G4' else limit
                fails = (lambda x: x < limit) if decision == 'each_ge' else (lambda x: x > limit)
                verdicts.extend(0 if not fails(y) else 1 if pins['gate'] in ['G3', 'VM-D6'] or not fails(x) or (x-y if decision == 'each_ge' else y-x) > max(a)-min(a) else 75 for x, y in zip(a, b))
            elif decision != 'report':
                passed = {'equal': lambda: a == b, 'zero': lambda: all(r['metrics'][metric] == 0 for r in rows), 'invariant': lambda: all(x is True for x in a+b), 'each_le': lambda: max(b) <= limit, 'delta': lambda: all(y <= x+limit for x, y in zip(a, b))}[decision]()
                verdicts.append(0 if passed else 1)
    if busy:
        return 75
    require(all(r['validity'] != 'pending_gpu_busy' for r in rows), 'unsubstantiated busy status')
    if any(r['exit'] != 0 for r in rows):
        return 101
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
    print({0: 'launch_spec_valid' if args.spec_only else 'gate_pass', 1: 'gate_failed', 2: 'invalid_evidence', 75: 'pending_or_report_only', 101: 'runtime_assertion_failed'}[code])
    raise SystemExit(code)
