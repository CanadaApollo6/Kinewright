from contextlib import contextmanager
import os
import signal
import subprocess
import re
import platform
import validate as v
from pathlib import Path

TESTS = {'G4': 'pf1_harness::s3b_native_play_lane', 'G15': 'pf1_harness::pf1_rss_baseline',
         'D13': 'pf1_harness::s4_d13_active', 'G1': 'pf1_harness::pf1_play_baseline',
         'G2': 'pf1_harness::s3b_native_play_lane', 'G14': 'pf1_harness::pf1_play_baseline'}

def test_check(spec, test):
    v.require(test == TESTS[spec['gate']], 'test does not implement pinned gate/protocol')

def arm_deadline(seconds):
    def expired(signum, frame):
        signal.setitimer(signal.ITIMER_REAL, 0)
        raise TimeoutError('complete invocation deadline')
    signal.signal(signal.SIGALRM, expired)
    signal.setitimer(signal.ITIMER_REAL, seconds)

def host_readback():
    v.require(platform.system() == 'Linux', 'unsupported host readback')
    models = {line.split(':', 1)[1].strip() for line in Path('/proc/cpuinfo').read_text().splitlines() if line.startswith('model name')}
    v.require(len(models) == 1, 'missing/ambiguous CPU model')
    return dict(os='linux', version=platform.release(), architecture=platform.machine(),
                cpu=models.pop(), ram_bytes=os.sysconf('SC_PHYS_PAGES') * os.sysconf('SC_PAGE_SIZE'))

def host_check(host, actual):
    v.require(all(host[key] == value for key, value in actual.items()), 'current host differs from pins')
    v.require(set(actual) == {'os', 'version', 'architecture', 'cpu', 'ram_bytes'}, 'incomplete host readback')

def checked(fn, *args):
    try:
        return fn(*args)
    except (ValueError, OSError, KeyError, TypeError, AttributeError) as error:
        print('invalid_evidence: ' + str(error))
        raise SystemExit(2)

def monitor(process, readback, sample, sleep):
    while process.poll() is None:
        readback()
        sample()
        sleep(5)

def live_record(path, token, started, now, pins, host, root, environment):
    if not path.exists():
        v.require(now-started < 10, 'missing live collector readback')
        return None
    record = v.read(path)
    expected = input_binding(pins, root, int(environment['PF1_ROW']))
    v.require(record.get('consumed') == expected, 'frozen inputs omitted/substituted/unconsumed')
    if pins['gate'] in ['G15', 'D13'] and record.get('valid') is True:
        native_check(record)
    v.require(record['token'] == token and started <= record['timestamp_s'] <= now, 'stale/future collector publication')
    v.schema(record['selected'], v.SCHEMA['$defs']['selected'])
    required = dict(name=pins['protocol']['name'], poll_ms=5, **v.CONTRACTS[pins['gate']][3])
    v.require(record['protocol'] == required, 'collector protocol mismatch')
    v.require(pins['lane'] == 'LL' and record['software'] is True and record['selected']['backend'] == 'Vulkan', 'unsupported collector route')
    driver = record['driver_info']
    mesa = re.search(r'\bMesa (\S+)', driver)
    llvm = re.search(r'\bLLVM ([\d.]+)', record['selected']['name'] + ' ' + driver)
    icd = environment.get('VK_DRIVER_FILES') or environment.get('VK_ICD_FILENAMES', '')
    v.require(mesa and llvm and icd and os.pathsep not in icd, 'missing LL driver/ICD readback')
    gpu = dict(selected=record['selected'], stack=dict(runtime=driver, mesa=mesa[1], llvm=llvm[1], icd_manifest_sha256=v.digest(Path(icd))), policy=dict(kind='software'), telemetry=dict(status='unavailable', reason='software adapter has no hardware clocks'))
    v.schema(gpu, v.SCHEMA['$defs']['gpu'])
    v.gpu_check(gpu, pins, host, root)
    return record, gpu

def native_check(record):
    cleanup = record['native_cleanup']
    v.require(record['native_children'] == 1 and cleanup['reaped'] is True and cleanup['exit'] == 0 and cleanup['pid'] == record['publisher_pid'], 'native child count/cleanup mismatch')

def input_binding(pins, root, ordinal):
    entry = pins['protocol']['order'][ordinal]
    value = v.artifact(root, pins['inputs'], 'inputs')[entry['arm']][entry['workload']]
    for key in ['workload', 'media', 'document']:
        v.artifact(root, value[key])
    manifest = v.read(root/value['workload']['path'])
    v.require(manifest == dict(key=entry['workload'], document=value['document'], media=value['media']), 'substituted input manifest')
    for ref in v.read(root/value['media']['path'])['files']:
        v.artifact(root, ref)
    return {key: value[key]['sha256'] for key in ['workload', 'media', 'document']}

def input_environment(pins, root, ordinal):
    input_binding(pins, root, ordinal)
    entry = pins['protocol']['order'][ordinal]
    value = v.read(root/pins['inputs']['path'])[entry['arm']][entry['workload']]
    return dict(PF1_FROZEN_INPUT=str(root/value['workload']['path']), PF1_FROZEN_SHA256=value['workload']['sha256'])

def gpu_sample(table, query, timestamp, owned):
    v.require(table['exit'] == query['exit'] == 0, 'failed GPU query')
    match = re.fullmatch(r'(\d+), ([^,\n]+), (\d+(?:\.\d+)?)', query['output'].strip())
    v.require(match is not None, 'malformed selected GPU query')
    index, uuid, utilization = match.groups()
    lines = table['output'].strip().splitlines()
    headers = [i for i, line in enumerate(lines) if re.fullmatch(r'\| Processes:\s*\|', line)]
    v.require(len(headers) == 1, 'missing/ambiguous process table')
    i = headers[0]
    body = lines[i+4:]
    v.require(i > 0 and re.fullmatch(r'\+-+\+', lines[i-1]) and len(body) >= 2, 'truncated process table')
    v.require(re.fullmatch(r'\|\s*GPU\s+GI\s+CI\s+PID\s+Type\s+Process name\s+GPU Memory\s*\|', lines[i+1]) and re.fullmatch(r'\|\s*ID\s+ID\s+Usage\s*\|', lines[i+2]) and re.fullmatch(r'\|=+\|', lines[i+3]), 'unrecognized process headers')
    v.require(re.fullmatch(r'\+-+\+', body[-1]), 'missing process table terminator')
    processes = []
    if len(body) == 2 and re.fullmatch(r'\|\s*No running processes found\s*\|', body[0]):
        return dict(timestamp_s=timestamp, uuid=uuid, utilization_pct=float(utilization), processes=[], complete=True)
    for line in body[:-1]:
        match = re.fullmatch(r'\|\s*(\d+)\s+N/A\s+N/A\s+(\d+)\s+(C\+G|C|G)\s+(.+?)\s+(\d+)MiB\s*\|', line)
        v.require(match is not None, 'unrecognized GPU process row')
        device, pid, kind, name, memory = match.groups()
        v.require(device == index, 'process row belongs to another GPU')
        processes.append(dict(pid=int(pid), kind=kind, memory_bytes=int(memory)*1048576, owned=int(pid) in owned))
    return dict(timestamp_s=timestamp, uuid=uuid, utilization_pct=float(utilization), processes=processes, complete=True)

@contextmanager
def owned_child(command, cleanup=None, **options):
    cleanup = {} if cleanup is None else cleanup
    process = subprocess.Popen(command, start_new_session=True, **options)
    cleanup.update(pid=process.pid, signals=[], reaped=False)
    try:
        yield process
    finally:
        for sig in [signal.SIGTERM, signal.SIGKILL]:
            cleanup['signals'].append(sig.name)
            try:
                os.killpg(process.pid, sig)
            except ProcessLookupError:
                pass
            try:
                process.wait(timeout=1)
                os.killpg(process.pid, 0)
            except ProcessLookupError:
                break
            except subprocess.TimeoutExpired:
                pass
        process.wait(timeout=1)
        cleanup.update(reaped=True, returncode=process.returncode)
