#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/setup-ffmpeg.sh
export CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 TMPDIR="$PWD/target/ja-s1-tmp"
nice -n 19 python3 -I - "${1:?absolute evidence directory}" <<'PY'
from pathlib import Path
import gzip,hashlib,os,re,subprocess,sys,time
out=Path(sys.argv[1]);assert out.is_absolute();out.mkdir(parents=True,exist_ok=True)
cases=[]
def add(name,path,anchor,replacement,witness,target='branches'):
 cases.append((name,Path(path),anchor,replacement,witness,target))
project='crates/kinewright-project/src/'
copy=project+'bundle_copy.rs'
all_assets='save_as_copies_every_retained_asset_including_luts_before_root'
alias='save_as_alias_traversal_case_collision_and_foreign_store_refuse'
unknown='copy_b5_unknown_reconciles_before_handoff_or_retry'
add('lut-closure',project+'store.rs', '                let mut file = source.open_verified(&reference)?;', '                if reference.record_kind == RecordKind::OwnedLut {copied.insert(hash);continue;}\n                let mut file = source.open_verified(&reference)?;',all_assets)
add('evidence-closure',project+'store.rs', '                let mut file = source.open_verified(&reference)?;', '                if reference.record_kind == RecordKind::OwnedEvidence {copied.insert(hash);continue;}\n                let mut file = source.open_verified(&reference)?;',all_assets)
add('source-pin',project+'store.rs','        let pin = self.pin(roots)?;\n        let mut ledger = self\n            .inner\n            .ledger\n            .lock()\n            .map_err(|_| BundleError::Capacity("ledger poisoned"))?;\n        let previous_collection', '        let pin = self.pin(&[])?;\n        let mut ledger = self\n            .inner\n            .ledger\n            .lock()\n            .map_err(|_| BundleError::Capacity("ledger poisoned"))?;\n        let previous_collection',unknown)
add('unknown-handoff',copy,'        if matches!(self.status, CopyStatus::Committed(_))','        if true',unknown)
add('foreign-store',project+'namespace/mod.rs','            linux::has_asset_store(self)','            Ok(false)',alias)
add('alias-parent',project+'namespace/linux.rs','    let expected = fs::fstat(&prepared.parent)?;', '    if !prepared.path.as_os_str().is_empty() {return Ok(()); }\n    let expected = fs::fstat(&prepared.parent)?;',alias)
add('copy-rehash',copy, '            for reference in target.store.closure(&roots)? {\n                target.store.verify(&reference)?;\n            }\n            let history = self.branch_history', '            let history = self.branch_history',alias)
add('copy-before-claim',copy,'    namespace.probe_copy_contention()?;', '    std::fs::write(&prepared.path,b"copy before claim")?;\n    namespace.probe_copy_contention()?;','save_as_contention_preserves_source_and_destination_graphs')
add('rollback-inventory',project+'store.rs','''        self.with_namespace(|n| {
            if let Some(bytes) = inventory {
                n.replace_relative(&path, bytes)
            } else {
                n.unlink_owned(&path)
            }
        })?;''', '', 'save_as_contention_preserves_source_and_destination_graphs')
add('archived-inventory',project+'store.rs','        self.closure_with_inventory(std::slice::from_ref(reference), false)?;','        self.closure_with_inventory(std::slice::from_ref(reference), true)?;',all_assets)
for name,path,anchor,replacement,witness,target in cases:
 if (out/f'{name}-restored.log.gz').exists():continue
 original=path.read_bytes();source=original.decode();assert source.count(anchor)==1,(name,source.count(anchor))
 args=['-p','kinewright-project','--features','test-util','--test','ja_s1_copy']
 try:
  for phase in ['red','restored']:
   path.write_bytes(source.replace(anchor,replacement,1).encode() if phase=='red' else original)
   while int(subprocess.check_output(['free','-g'],text=True).splitlines()[1].split()[-1])<6:
    print('Waiting for at least 6 GB available',flush=True);time.sleep(300)
   command=['nice','-n','10','cargo','test',*args,witness,'--','--exact','--nocapture']
   log=out/f'{name}-{phase}.log'
   with log.open('wb') as f:
    f.write(subprocess.check_output(['git','rev-parse','HEAD']))
    f.write(('diff_sha256: '+hashlib.sha256(subprocess.check_output(['git','diff','--binary'])).hexdigest()+'\nsource_sha256: '+hashlib.sha256(path.read_bytes()).hexdigest()+'\ncommand: '+repr(command)+'\n').encode())
    manifest={}
    for item in subprocess.check_output(['git','ls-files','-m','-o','--exclude-standard','-z']).split(b'\0'):
     if item:
      file=Path(os.fsdecode(item))
      if file.is_file():manifest[str(file)]=hashlib.sha256(file.read_bytes()).hexdigest()
    f.write(('source_manifest: '+repr(manifest)+'\n').encode());f.flush()
    status=subprocess.run(command,stdout=f,stderr=subprocess.STDOUT).returncode
   text=log.read_text();summary=re.findall(r'^test result:.*$',text,re.M)
   assert summary,(name,phase,'missing runtime summary',log)
   summary=[summary[-1]]
   assert not re.search(r'(setup failed|setup deadline|completion deadline|participants deadline|controller release deadline)',text),(name,phase,'setup or timeout is not a red',log)
   assert ('FAILED. 0 passed; 1 failed;' in summary[0] and status!=0) if phase=='red' else ('ok. 1 passed; 0 failed;' in summary[0] and status==0),(name,phase,summary,log)
   binary=re.search(r'Running .* \(([^)]+)\)',text);assert binary,(name,phase)
   with log.open('a') as f:f.write('\nbinary_sha256: '+hashlib.sha256(Path(binary.group(1)).read_bytes()).hexdigest()+'\n')
   with gzip.open(str(log)+'.gz','wb') as f:f.write(log.read_bytes())
   log.unlink();print(name,phase,summary[0],flush=True)
 finally:path.write_bytes(original)
PY
