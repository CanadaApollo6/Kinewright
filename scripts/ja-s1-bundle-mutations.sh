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
old_hash=hashlib.sha256(Path('crates/kinewright-agent/tests/fixtures/ja/s1-migration-input.kinewright').read_bytes()).hexdigest()
cases=[]
def add(name,file,anchor,replacement,witness,target='storage'):
 cases.append((name,Path(file),anchor,replacement,witness,target))
for name,condition in [('B1',f'operation == "object_file_sync" && observed.file_name().and_then(|n|n.to_str()) != Some("{old_hash}")'),('B3',f'operation == "object_file_sync" && observed.file_name().and_then(|n|n.to_str()) == Some("{old_hash}")'),('B4','operation == "root_file_sync"')]:
 add(name,project/'namespace/linux.rs','    file.sync_all()?;\n    note(operation, observed);',f'    if {condition} {{ return Ok(()); }}\n    file.sync_all()?;\n    note(operation, observed);','durable_bundle::bundle_crash_cuts_preserve_acknowledged_graph')
for name,condition in [('B2','observed.ends_with("studio/objects")'),('ancestor','observed.file_name().is_some_and(|n|n == "studio.kinewright-assets")')]:
 add(name,project/'namespace/linux.rs','    fs::fsync(directory)?;\n    note("directory_sync", observed);',f'    if {condition} {{return Ok(());}}\n    fs::fsync(directory)?;\n    note("directory_sync", observed);','durable_bundle::bundle_crash_cuts_preserve_acknowledged_graph')
# Omitting this actual root namespace barrier also removes its observation.
add('B5',project/'namespace/linux.rs','''        sync_directory(
            &namespace.parent,
            namespace.identity.resolved_root.parent().unwrap(),
        )
        .map_err(|_| BundleError::DurabilityUnknown)?;''','', 'durable_bundle::bundle_crash_cuts_preserve_acknowledged_graph')
add('backup',project/'bundle.rs','recovery.pre_migration = Some(reference.clone());','recovery.pre_migration = None;','durable_bundle::migration_keeps_exact_backup_and_is_idempotent')
add('support-api',project/'bundle_recovery.rs','if manifest.required_apis != supported_apis() {','if false && manifest.required_apis != supported_apis() {','durable_bundle::v3_support_inventory_refuses_hidden_and_future_kinds_without_writes')
add('corrupt-history',project/'store.rs','''        if measured != reference.byte_length
            || !digest.as_ref().is_ok_and(|h| h == &reference.sha256)
        {''','''        if false && (measured != reference.byte_length
            || !digest.as_ref().is_ok_and(|h| h == &reference.sha256))
        {''','durable_bundle::corrupt_history_quarantines_without_rewriting_usable_versions')
add('source-destination-admission',project/'bundle.rs','''    pub fn authorize_legacy_write(&self, destination: &Path) -> Result<(), ProjectSaveError> {''','''    pub fn authorize_legacy_write(&self, destination: &Path) -> Result<(), ProjectSaveError> {
        if !destination.as_os_str().is_empty() {return Ok(());}''','headless::tests::save_as_unsupported_source_or_destination_has_zero_mutations','project-lib')
add('app-source-destination-admission',project/'bundle.rs','''    pub fn authorize_legacy_write(&self, destination: &Path) -> Result<(), ProjectSaveError> {''','''    pub fn authorize_legacy_write(&self, destination: &Path) -> Result<(), ProjectSaveError> {
        if !destination.as_os_str().is_empty() {return Ok(());}''','app::in2b_tests::save_as_unsupported_source_or_destination_has_zero_mutations','app')
add('history-reopen',project/'bundle.rs','''        core.restore_history((*branch.1).clone())''','''        core.restore_history(kinewright_core::Core::spawn((*branch.1.document).clone()).unwrap().export_history().unwrap())''','history_loss_fixture_revised_preserves_one_entry','agent')
add('eval-v3','crates/kinewright-agent/src/bin/kinewright-eval.rs','    let read = project_format_version(bytes);','    let read = project_format_version(bytes); if read == 3 {return Ok(());}','tests::rerender_refuses_v3_before_projection_or_output','eval')
add('comment-baseline','crates/kinewright-agent/src/ja_harness/revised.rs','"binding_present":root.receipt_index_ref.is_some()','"binding_present":true','comment_observer_persists_exact_bytes_then_missing_receipt','agent')
for name,path,anchor,replacement,witness,target in cases:
 if (out/f'{name}-restored.log.gz').exists():continue
 original=path.read_bytes();source=original.decode();assert source.count(anchor)==1,(name,source.count(anchor))
 args={'storage':['-p','kinewright-project','--features','test-util','--test','ja_s1_storage'], 'project-lib':['-p','kinewright-project','--features','test-util','--lib'], 'app':['-p','kinewright-app','--bin','kinewright-app'], 'agent':['-p','kinewright-agent','--features','eval-harness','--test','ja_s1'], 'eval':['-p','kinewright-agent','--features','eval-harness','--bin','kinewright-eval']}[target]
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
   binary=re.search(r'Running .* \(([^)]+)\)',text);assert binary,(name,phase)
   with log.open('a') as f:f.write('\nbinary_sha256: '+hashlib.sha256(Path(binary.group(1)).read_bytes()).hexdigest()+'\n')
   with gzip.open(str(log)+'.gz','wb') as f:f.write(log.read_bytes())
   log.unlink();print(name,phase,summary[0],flush=True)
 finally:path.write_bytes(original)
PY
