"""The setup sampler must observe a real executable read, without tracing unrelated PIDs."""
from pathlib import Path
import gzip
import json
import os
import subprocess
import sys
import tempfile

assert os.getpriority(os.PRIO_PROCESS, 0) == 19
root = Path(__file__).resolve().parent.parent
with tempfile.TemporaryDirectory(prefix='setup-profile-check-', dir=root/'target') as temporary:
    temporary = Path(temporary)
    binary = temporary/'binary'
    binary.write_bytes(b'x' * (1 << 20))
    code = 'import sys,time\nf=open(sys.argv[1],"rb")\nwhile f.read(65536): time.sleep(.03)\ntime.sleep(.1)'
    process = subprocess.Popen(['nice','-n','19',sys.executable,'-I','-c',code,str(binary)])
    try:
        subprocess.run(['nice','-n','19',sys.executable,'-I',str(root/'scripts/ja-s1-setup-profile.py'),str(process.pid),str(binary),str(temporary/'profile.json.gz'),str(temporary/'missing-report')],check=True,stdout=subprocess.DEVNULL)
    finally:
        process.wait(timeout=10)
    with gzip.open(temporary/'profile.json.gz','rt') as stream:
        profile = json.load(stream)
    assert set(profile['processes']) == {str(process.pid)}, profile
    row = profile['processes'][str(process.pid)]
    assert row['hash_read_start_s'] is not None, row
    assert row['max_hash_position'] == binary.stat().st_size, row
    assert row['hash_read_end_s'] - row['hash_read_start_s'] >= .3, row
print('setup profiler observed the owned full-file read and no unrelated process')
