#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/setup-ffmpeg.sh
export CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 TMPDIR="$PWD/target/ja-s1-tmp"
nice -n 19 python3 -I - "${1:?absolute evidence directory}" <<'PY'
from pathlib import Path
import gzip,hashlib,os,re,subprocess,sys,time
out=Path(sys.argv[1]);assert out.is_absolute();out.mkdir(parents=True,exist_ok=True)
project=Path('crates/kinewright-project/src')
cases=[]
def add(name,file,anchor,replacement,witness):
 cases.append((name,project/file,anchor,replacement,witness,'receipts'))
add('stale-before-lookup','bundle_commit.rs','        let lookup = bundle.receipt_lookup(envelope)?;','''        if envelope.owner_epoch != bundle.store.owner_epoch() {return Err(BundleError::Unsupported("stale epoch".into()));}
        let lookup = bundle.receipt_lookup(envelope)?;''','edit_and_comment_lost_replies_reconcile_before_stale_admission')
add('ignore-digest','receipts.rs','return if receipt.digest != *digest {','return if false && receipt.digest != *digest {','same_id_changed_digest_refuses_without_mutation')
add('replay-retired','bundle_commit.rs','''        if lookup != ReceiptLookup::Absent {
            return Ok(lookup);
        }''','''        if lookup != ReceiptLookup::Absent {
            if lookup == ReceiptLookup::RetiredUnresolved {
                if let Some(command) = &envelope.command { let _ = core.request(Command::from(command.clone())); }
            }
            return Ok(lookup);
        }''','retired_unresolved_never_reapplies_or_acknowledges')
add('drop-retired-exact','receipts.rs','[(&self.detailed, false), (&self.retired, true)]','[(&self.detailed, false), (&self.detailed, true)]','retired_exact_evidence_resolves_original_edit_and_comment_results')
add('evict-issuer','receipts.rs',"        if first_sequence == 0 || self.issuers.len() as u64 >= self.store.limits().issuers {",'''        if self.issuers.len() as u64 >= self.store.limits().issuers {self.issuers.clear();}
        if first_sequence == 0 {''','issuer_exhaustion_preserves_unresolved_evidence')
add('ack-unknown','bundle_commit.rs',"        if let Err(error) = Self::publish(bundle, &after, inputs) {",'''        if let Err(error) = Self::publish(bundle, &after, inputs) {
            if matches!(error, BundleError::DurabilityUnknown) {bundle.pending_command=None;core.restore_history(after).unwrap();return bundle.receipt_lookup(envelope);}''','ambiguous_commit_keeps_pending_until_receipt_reconciliation')
cases.append(('comment-result-binding',project/'receipts.rs','''    Ok(ReceiptResult::Comment {
        record_id,''','''    Ok(ReceiptResult::Comment {
        record_id: "comment-00000000000000000000000000000000".into(),''','comment_receipt_fixture_revised_reconciles_exact_original_result','agent'))
for name,path,anchor,replacement,witness,target in cases:
 if (out/f'{name}-restored.log.gz').exists():continue
 original=path.read_bytes();source=original.decode();assert source.count(anchor)==1,(name,source.count(anchor))
 args=['-p','kinewright-project','--features','test-util','--test','ja_s1_receipts'] if target=='receipts' else ['-p','kinewright-agent','--features','eval-harness','--test','ja_s1']
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
   assert len(summary)==1,(name,phase,'missing runtime summary',log)
   assert ('FAILED. 0 passed; 1 failed;' in summary[0] and status!=0) if phase=='red' else ('ok. 1 passed; 0 failed;' in summary[0] and status==0),(name,phase,summary,log)
   if phase=='red' and target=='agent':assert 'left: Some(1)' in text,(name,'must be exercised native property red, not CLI setup exit 2',log)
   binary=re.search(r'Running .* \(([^)]+)\)',text);assert binary,(name,phase)
   with log.open('a') as f:f.write('\nbinary_sha256: '+hashlib.sha256(Path(binary.group(1)).read_bytes()).hexdigest()+'\n')
   with gzip.open(str(log)+'.gz','wb') as f:f.write(log.read_bytes())
   log.unlink();print(name,phase,summary[0],flush=True)
 finally:path.write_bytes(original)
PY
