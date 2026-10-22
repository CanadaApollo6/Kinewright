"""Run the S1 per-commit gate, every touched crate, and the default-TMPDIR workspace fast tier."""
from pathlib import Path
import ast
import gzip
import hashlib
import json
import os
import re
import subprocess
import sys
import time

out = Path(sys.argv[1]).resolve()
out.mkdir(parents=True, exist_ok=True)
env = dict(os.environ, CARGO_BUILD_JOBS='4', RUST_TEST_THREADS='4', JA_S1_FINAL_HEAD='1')
env.pop('TMPDIR', None)
results = []

def run(label, command, optional_native=False):
    assert not (out/f'{label}.log.gz').exists(), label
    while (label in ('per-commit','workspace-fast') or 'cargo' in command) and int(next(line.split()[1] for line in Path('/proc/meminfo').read_text().splitlines() if line.startswith('MemAvailable:'))) < 6291456:
        print(f'{label}: waiting for 6 GiB available', flush=True)
        time.sleep(30)
    memory = subprocess.check_output(['free','-g'],text=True)
    header = {
        'head':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
        'diff_sha256':hashlib.sha256(subprocess.check_output(['git','diff','--binary'])).hexdigest(),
        'command':command, 'TMPDIR':'unset', 'memory':memory,
        'source_sha256': {str(path): hashlib.sha256(path.read_bytes()).hexdigest() for path in Path('crates').rglob('*.rs') if 'target' not in path.parts},
    }
    print(f'Running {label}',flush=True)
    started=time.monotonic()
    path=out/f'{label}.log'
    with path.open('w') as log:
        log.write(json.dumps(header)+'\n');log.flush()
        process=subprocess.run(command,stdout=log,stderr=subprocess.STDOUT,env=env)
    text=path.read_text()
    with gzip.open(str(path)+'.gz','wt') as compressed:compressed.write(text)
    path.unlink()
    summaries=re.findall(r'^test result: .+$',text,re.M)
    row={'label':label,'command':command,'exit':process.returncode,'seconds':time.monotonic()-started,'summaries':summaries,'tail':text.splitlines()[-16:],'head':header['head']}
    if optional_native and process.returncode != 0:
        assert 'error[E' not in text and re.search(r'failed to run custom build command for [`\'](?:aws-lc-sys|ffmpeg-sys|whisper-rs-sys)',text),text[-6000:]
        row['status']='NOT RUN: native MSVC dependency prerequisite'
    else:
        row['status']='passed' if process.returncode==0 else 'failed'
    results.append(row)
    (out/'results.json').write_text(json.dumps(results,indent=2)+'\n')
    for line in summaries:print(label,line,flush=True)
    if process.returncode and not optional_native:
        print('\n'.join(text.splitlines()[-80:]),flush=True)
        raise SystemExit(process.returncode)
    if row['status'].startswith('NOT RUN'):print(label,row['status'],flush=True)

run('per-commit',['bash','scripts/ja-s1-gate.sh','8',str(out/'per-commit')])
installed=subprocess.check_output(['rustup','target','list','--installed'],text=True)
(out/'installed-targets.txt').write_text(installed)
if 'x86_64-pc-windows-msvc' in installed:
    run('windows-core',['nice','-n','10','cargo','check','-p','kinewright-core','--features','test-util','--target','x86_64-pc-windows-msvc','--lib','--tests'])
    run('windows-project',['nice','-n','10','cargo','check','-p','kinewright-project','--features','test-util','--target','x86_64-pc-windows-msvc','--lib','--tests'],optional_native=True)
else:
    (out/'windows-not-run.txt').write_text('NOT RUN: MSVC Rust target is not installed. No toolchains installed.\n')
for package,extra in [
    ('kinewright-core',['--features','test-util']),
    ('kinewright-project',['--features','test-util']),
    ('kinewright-agent',['--features','eval-harness']),
    ('kinewright-app',[]),
    ('kinewright-media',['--features','test-util','--','--test-threads=1']),
]:
    run(package,['nice','-n','10','cargo','test','-p',package,*extra])
run('workspace-fast',['nice','-n','10','python3','-I','scripts/slow_tests.py','fast'])
run('classification',['nice','-n','19','python3','-I','scripts/slow_tests.py','lint'])
run('clippy-final',['nice','-n','10','cargo','clippy','--workspace','--all-targets','--','-D','warnings'])
run('actionlint',['actionlint','.github/workflows/ci.yml'])
run('shell-syntax',['bash','-n','scripts/ja-s1-gate.sh','scripts/ja-s1-predecessor-controls.sh'])
run('frozen-fixtures',['git','diff','--exit-code','0f4f88e','--','crates/kinewright-agent/tests/fixtures/ja','.gitattributes'])
for path in Path('scripts').glob('ja-s1-fix-r1-*.py'):ast.parse(path.read_text(),filename=str(path))
print('Linux stricter gate complete. Windows execution and full slow CI remain NOT RUN.',flush=True)
