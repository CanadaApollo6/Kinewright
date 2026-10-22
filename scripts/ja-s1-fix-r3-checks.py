"""Red/restored pairs for the standalone feature and setup-diagnostic contracts."""
from pathlib import Path
import gzip
import hashlib
import json
import os
import subprocess
import sys

assert os.getpriority(os.PRIO_PROCESS, 0) == 19
out=Path(sys.argv[1]).resolve()
out.mkdir(parents=True,exist_ok=True)
cases={
 'feature':('crates/kinewright-project/Cargo.toml','test-util = ["kinewright-media/test-util"]','test-util = []',['bash','scripts/ja-s1-test-util-check.sh'],'could not find `test_support`','compile contract'),
 'setup-profile':('scripts/ja-s1-setup-profile.py','if fd.resolve() == binary:','if False and fd.resolve() == binary:',['nice','-n','19',sys.executable,'-I','scripts/ja-s1-setup-profile-check.py'],'AssertionError','runtime diagnostic'),
 'setup-status':('scripts/ja-s1-profile-control.sh','[[ "$sampler_result" -eq 0 ]] || exit 2','[[ "$sampler_result" -eq 0 ]] || exit "$sampler_result"',['nice','-n','19',sys.executable,'-I','scripts/ja-s1-setup-status-check.py'],'AssertionError','runtime diagnostic status'),
}
rows=[]
for name in sys.argv[2:] or cases:
    file,before,after,command,red_marker,kind=cases[name]
    path=Path(file);original=path.read_text()
    assert original.count(before)==1
    try:
        for phase,content in [('red',original.replace(before,after)),('restored',original)]:
            path.write_text(content)
            identity={'name':name,'phase':phase,'kind':kind,'command':command,'source_sha256':hashlib.sha256(path.read_bytes()).hexdigest(),'head':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip()}
            identity['diff_sha256']=hashlib.sha256(subprocess.check_output(['git','diff','--binary'])).hexdigest()
            identity['witness_sources_sha256']={str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in [*Path('crates').rglob('*.rs'),*Path('scripts').glob('ja-s1-*.py'),*Path('scripts').glob('ja-s1-*.sh')] if 'target' not in p.parts}
            result=subprocess.run(command,capture_output=True,text=True)
            text=result.stdout+result.stderr
            with gzip.open(out/f'{name}-{phase}.log.gz','wt') as stream:stream.write(json.dumps(identity)+'\n'+text)
            assert (result.returncode!=0 and red_marker in text) if phase=='red' else result.returncode==0,(name,phase,text[-4000:])
            rows.append({**identity,'exit':result.returncode})
            print(name,phase,kind,'expected result',result.returncode,flush=True)
    finally:
        path.write_text(original)
with gzip.open(out/'results.json.gz','wt') as stream:json.dump(rows,stream,indent=2)
