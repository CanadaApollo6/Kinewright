"""A broken diagnostic must never turn a fixture's not-run exit 2 into expected exit 1."""
from pathlib import Path
import os
import subprocess
import tempfile

assert os.getpriority(os.PRIO_PROCESS, 0) == 19
repo=Path(__file__).resolve().parent.parent
with tempfile.TemporaryDirectory(prefix='setup-status-check-',dir=repo/'target') as directory:
    root=Path(directory)
    binary=root/'kinewright-ja-baseline'
    binary.write_text('#!/usr/bin/env bash\nexit "$JA_S1_FAKE_EXIT"\n')
    binary.chmod(0o755)
    fixture=root/'fixture.json';fixture.write_text('{}')
    shims=root/'shims';shims.mkdir()
    python=shims/'python3';python.write_text('#!/usr/bin/env bash\nexit 1\n');python.chmod(0o755)
    for diagnostic_fails in (False,True):
        for code in ((2,1,0) if diagnostic_fails else (0,1,2)):
            env=dict(os.environ,JA_S1_FAKE_EXIT=str(code))
            if diagnostic_fails:env['PATH']=str(shims)+os.pathsep+env['PATH']
            profile=root/f'profile-{diagnostic_fails}-{code}'
            result=subprocess.run(['bash',str(repo/'scripts/ja-s1-profile-control.sh'),str(binary),str(fixture),str(root/'report'),str(profile)],env=env,capture_output=True,text=True)
            expected=2 if diagnostic_fails else code
            assert result.returncode==expected,(diagnostic_fails,code,result.returncode,result.stdout,result.stderr)
print('fixture exits 0/1/2 preserved; diagnostic failures stay not-run exit 2')
