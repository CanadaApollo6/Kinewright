"""Check the fresh control run's source and transformation provenance."""
from pathlib import Path
import json
import platform
import sys
if len(sys.argv) > 1:
    import gzip, hashlib, subprocess
    out=Path(sys.argv[1]); out.mkdir(parents=True,exist_ok=True)
    path=Path('scripts/ja-s1-predecessor-controls.sh')
    original=path.read_text()
    before="manifest['strip_version']=(sealed/'strip-version.txt').read_text().splitlines()[0]"
    assert original.count(before)==1
    rows=[]
    try:
        for phase, content in [('red',original.replace(before,"manifest['strip_version']=None")),('restored',original)]:
            path.write_text(content)
            header={'phase':phase,'script_sha256':hashlib.sha256(path.read_bytes()).hexdigest(),
                'head':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip()}
            control=subprocess.run(['bash',str(path),str(out/phase)],capture_output=True,text=True)
            assert control.returncode==0,control.stdout[-4000:]+control.stderr[-4000:]
            checked=subprocess.run(['nice','-n','19',sys.executable,'-I',__file__],capture_output=True,text=True)
            identity_path=Path('target/ja-s1-predecessor/sealed/identity.json')
            header['identity_sha256']=hashlib.sha256(identity_path.read_bytes()).hexdigest()
            with gzip.open(out/(phase+'.log.gz'),'wt') as stream:
                stream.write(json.dumps(header)+'\n'+control.stdout+control.stderr+checked.stdout+checked.stderr)
            assert (checked.returncode != 0 and 'AssertionError' in checked.stderr) if phase=='red' else checked.returncode==0
            rows.append({**header,'control_exit':control.returncode,'check_exit':checked.returncode})
            print('provenance',phase,'controls',control.returncode,'check',checked.returncode,flush=True)
    finally:path.write_text(original)
    with gzip.open(out/'results.json.gz','wt') as stream:json.dump(rows,stream,indent=2)
    raise SystemExit(0)
identity=json.loads(Path('target/ja-s1-predecessor/sealed/identity.json').read_text())
assert identity['source_clean']
assert len(identity['pre_strip_binary_sha256']) == 64
assert 'working directory' in identity['build_provenance']
if platform.system() == 'Linux':
    assert identity['strip_version'] and identity['strip_arguments'] == ['--strip-all']
    assert identity['pre_strip_binary_sha256'] != identity['binary_sha256']
print('predecessor pre-strip hash, strip version, source build provenance and run working-directory semantics checked')
