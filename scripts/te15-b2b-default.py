#!/usr/bin/env python3
"""Compile the default Agent and inspect its actual artifacts, without running them."""
import hashlib
import json
import os
import subprocess
from pathlib import Path
ROOT=Path(__file__).resolve().parent.parent
OUT=Path(os.environ.get('KINEWRIGHT_B2B_DEFAULT_OUT',ROOT/'target/te1-5/b2b/default'))
OUT.mkdir(parents=True,exist_ok=True)
command=['bash','-c','source scripts/setup-ffmpeg.sh >&2 && exec flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 nice -n19 cargo build -j4 -p kinewright-agent --lib --bins --message-format=json']
with (OUT/'build.jsonl').open('w') as out,(OUT/'build.log').open('w') as log:
    code=subprocess.run(command,cwd=ROOT,stdout=out,stderr=log).returncode
assert code==0,'B2B_DEFAULT_BUILD'
artifacts=[]
for line in (OUT/'build.jsonl').read_text().splitlines():
    value=json.loads(line)
    if value.get('reason')=='compiler-artifact' and value['target']['name'] in ['kinewright_agent','kinewright-agent','kinewright-job-guardian','kinewright-eval','te1-measurement-pin']:
        assert 'te1-eval' not in value['features'],'B2B_DEFAULT_EXCLUDES_FEATURE'
        assert value['target']['name']!='te1-measurement-pin','B2B_DEFAULT_EXCLUDES_PIN_BIN'
        for name in value['filenames']:
            path=Path(name)
            if path.suffix not in ['.rlib',''] or not path.is_file():continue
            data=path.read_bytes()
            assert b'KINEWRIGHT_TE1_RUN' not in data,'B2B_DEFAULT_EXCLUDES_GRANT_CODE'
            artifacts.append({'target':value['target']['name'],'path':str(path),'features':value['features'],'sha256':hashlib.sha256(data).hexdigest()})
assert any(a['target']=='kinewright_agent' for a in artifacts),'B2B_DEFAULT_LIBRARY_OBSERVED'
(OUT/'artifacts.json').write_text(json.dumps(artifacts,indent=2)+'\n')
print(f'{len(artifacts)} default artifacts; no te1-eval feature or grant entry code')
