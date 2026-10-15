from contextlib import contextmanager
import os
import signal
import subprocess
import re
import validate as v
from pathlib import Path

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
def owned_child(command, **options):
    process = subprocess.Popen(command, start_new_session=True, **options)
    try:
        yield process
    finally:
        for sig in [signal.SIGTERM, signal.SIGKILL]:
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
