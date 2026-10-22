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
store=project+'store.rs'
retention='retention::recovery_inventory_retains_two_predecessors_and_one_staging_slot'
crash='retention::inventory_retirement_crash_preserves_required_predecessors'
pins='retention::old_floor_collection_keeps_independent_branch_version_and_receipt_pins'
verify='retention::metadata_commit_verifies_new_bytes_but_recovery_verifies_full_graph'
caps='retention::every_storage_cap_backpressures_without_data_loss'
add('recursive-inventory',store,'include_inventory && reference.record_kind != RecordKind::RootCheckpoint;','include_inventory;',retention)
add('retained-floors',project+'bundle_commit.rs',"""            evidence.retain(|r| {
                !matches!(
                    r.record_kind,
                    RecordKind::HistoryFloor | RecordKind::RecoveryClose
                )
            });""",'',pins)
add('live-pin',store,'ledger.pins.get(*h).copied().unwrap_or(0) == 0 && !durable.contains_key(*h)','!durable.contains_key(*h)',caps)
add('full-recovery-hash',store,'if measured != reference.byte_length\n            || !digest.as_ref().is_ok_and(|h| h == &reference.sha256)','if measured != reference.byte_length',verify)
add('copy-overlap',store,'        self.reserve(bytes, 0, 0, true)','        self.reserve(0, 0, 0, true)',caps)
add('double-retirement',store,'let Some(physical) = usage.physical_bytes.checked_sub(reference.byte_length) else {','let Some(physical) = usage.physical_bytes.checked_sub(reference.byte_length * 2) else {',caps)
add('physical-recovery',store,'        store.account_residuals()?;','',crash)
add('temporary-retirement',store,'    for file in temporaries {','    for file in temporaries.into_iter().take(0) {',crash)
add('prospective-depth',store,'            reference.record_kind != RecordKind::RootCheckpoint,\n            1,','            reference.record_kind != RecordKind::RootCheckpoint,\n            0,',caps)
add('control-cap',store,'.limits = limits;','.limits = StorageLimits {control_bytes:StorageLimits::default().control_bytes,..limits};',caps)
add('inventory-barrier',project+'namespace/linux.rs','        sync_directory(&dir, path.as_path().parent().unwrap())\n            .map_err(|_| BundleError::DurabilityUnknown)?;','',crash)
add('combined-metadata',store,'                    .and_then(|n| n.checked_add(usage.decoded_bytes))\n','',caps)
for name,path,anchor,replacement,witness,target in cases:
 if (out/f'{name}-restored.log.gz').exists():continue
 original=path.read_bytes();source=original.decode();assert source.count(anchor)==1,(name,source.count(anchor))
 args=['-p','kinewright-project','--features','test-util','--test','ja_s1_storage']
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
