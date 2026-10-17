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
    if 'metrics' in record or 'valid' in record:
        v.require(record.get('consumed') == expected, 'frozen inputs omitted/substituted/unconsumed')
    if pins['gate'] in ['G15', 'D13'] and record.get('valid') is True:
        native_check(record)
        if pins['gate'] == 'D13': v.d13_check(record, pins)
    v.require(record['token'] == token and started <= record['timestamp_s'] <= now, 'stale/future collector publication')
    v.schema(record['selected'], v.SCHEMA['$defs']['selected'])
    required = dict(name=pins['protocol']['name'], poll_ms=5, **v.CONTRACTS[pins['gate']][3])
    if pins['gate'] == 'D13': required['d_bytes'] = pins['protocol']['d_bytes']
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
    v.require(all(type(value) is int for value in [record['native_children'], record['publisher_pid'], cleanup['pid'], cleanup['exit']]) and cleanup['pid'] > 0, 'malformed native receipt numbers')
    v.require(record['native_children'] == 1 and cleanup['reaped'] is True and cleanup['exit'] == 0 and cleanup['pid'] == record['publisher_pid'], 'native child count/cleanup mismatch')

def lh_probe(read, environment):
    icd = environment.get('VK_DRIVER_FILES') or environment.get('VK_ICD_FILENAMES', '')
    v.require(icd and os.pathsep not in icd, 'one explicit LH ICD required')
    uuid = environment['PF1_GPU_UUID']
    return dict(inventory=read(['nvidia-smi', '--query-gpu=index,uuid,name,pci.device_id,driver_version', '--format=csv,noheader,nounits']),
                readback=read(['nvidia-smi', '-i', uuid, '--query-gpu=uuid,clocks.gr,clocks.mem,pstate,power.limit,power.draw', '--format=csv,noheader,nounits']),
                icd=Path(icd).read_text())

def lh_identity(record, probe):
    selected = record['selected']
    v.require(record['software'] is False and selected['backend'] == 'Vulkan' and selected['vendor_id'] == 0x10de and selected['driver'] == 'NVIDIA', 'LH requires actual proprietary NVIDIA session')
    inventory = probe['inventory']
    v.require(inventory['exit'] == 0, 'failed LH inventory')
    rows = [line.split(', ') for line in inventory['output'].strip().splitlines()]
    v.require(rows and all(len(row) == 5 for row in rows), 'malformed LH inventory')
    for index, uuid, name, pci, driver in rows:
        v.require(re.fullmatch(r'\d+', index) and re.fullmatch(r'GPU-[0-9a-f-]{36}', uuid) and re.fullmatch(r'0x[0-9A-Fa-f]{8}', pci) and re.fullmatch(r'\d+\.\d+\.\d+', driver), 'malformed LH identity fields')
    v.require(len({row[1] for row in rows}) == len(rows), 'duplicate inventory UUID')
    matches = [row for row in rows if int(row[3], 16) == selected['device_id'] << 16 | selected['vendor_id']]
    v.require(len(matches) == 1, 'ambiguous/missing actual-session PCI identity')
    _, uuid, name, _, driver = matches[0]
    v.require(name == selected['name'] == 'NVIDIA GeForce RTX 3090' and driver in record['driver_info'] and selected.get('uuid', uuid) == uuid, 'LH session driver/name/UUID mismatch')
    manifest = v.json.loads(probe['icd'], object_pairs_hook=v.pairs)
    v.require(Path(manifest['ICD']['library_path']).name == 'libGLX_nvidia.so.0', 'LH requires proprietary NVIDIA ICD')
    unavailable = dict(status='unavailable', reason='proprietary NVIDIA stack; Mesa/LLVM not used')
    stack = dict(runtime=record['driver_info'] + '; nvidia-smi driver=' + driver, mesa=unavailable, llvm=unavailable, icd_manifest_sha256=v.hashlib.sha256(probe['icd'].encode()).hexdigest())
    return dict(selected, uuid=uuid), stack

def lh_observation(probe, timestamp, uuid):
    query = probe['readback']
    v.require(query['exit'] == 0, 'failed LH readback')
    fields = query['output'].strip().split(', ')
    v.require(len(fields) == 6 and fields[0] == uuid, 'malformed/uncorrelated LH readback')
    result = dict(timestamp_s=timestamp, uuid=uuid)
    for key, value in zip(['graphics_mhz', 'memory_mhz', 'pstate', 'power_limit_w', 'power_draw_w'], fields[1:]):
        if value in ['[N/A]', '[Not Supported]', 'N/A']:
            result[key] = dict(status='unavailable', reason='nvidia-smi returned ' + value)
        else:
            result[key] = value if key == 'pstate' else float(value)
    v.schema(result, v.SCHEMA['$defs']['default_observation'])
    return result

def input_binding(pins, root, ordinal):
    entry = pins['protocol']['order'][ordinal]
    value = v.artifact(root, pins['inputs'], 'inputs')[entry['arm']][entry['workload']]
    return v.frozen_input(root, entry['workload'], value)

def input_environment(pins, root, ordinal):
    input_binding(pins, root, ordinal)
    entry = pins['protocol']['order'][ordinal]
    value = v.read(root/pins['inputs']['path'])[entry['arm']][entry['workload']]
    environment = dict(PF1_FROZEN_INPUT=str(root/value['workload']['path']), PF1_FROZEN_SHA256=value['workload']['sha256'])
    if pins['gate'] == 'D13':
        v.artifact(root, pins['protocol']['calibration'])
        environment.update(PF1_D13_CALIBRATION=(root/pins['protocol']['calibration']['path']).read_text(), PF1_D13_CALIBRATION_SHA256=pins['protocol']['calibration']['sha256'])
    return environment

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
