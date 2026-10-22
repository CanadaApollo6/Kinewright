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
core='crates/kinewright-core/src/'
project='crates/kinewright-project/src/'
agent='crates/kinewright-agent/src/durable_branch.rs'
add('fresh-revision',agent,'''        let revision = self.bind(bundle, live)?;
        let frozen = self''','''        let revision = TimelineRevision(4);
        let frozen = self''','reopened_exact_base_merges_once_with_fresh_runtime_revision')
add('head-identity',agent,'            || checkpoint.head_id != self.base.base_head\n','','equal_bytes_new_head_and_equal_counter_other_head_conflict')
add('full-applied-delta',agent,'        self.base.full_applied_delta = frozen.applied_operations();','''        let Event::QueryResult(QueryResult::OpLog(operations))=self.branch.core().request(Command::Query(Query::OpLog)).unwrap() else {panic!("op log")};
        self.base.full_applied_delta=(*operations).clone();''','reopened_exact_base_merges_once_with_fresh_runtime_revision')
add('lookup-before-bind',agent,'        let previous = bundle.receipt_lookup(envelope)?;','''        self.bind(bundle,live)?;
        let previous = bundle.receipt_lookup(envelope)?;''','merge_lost_reply_preserves_one_undo_entry')
add('undone-ancestry',project+'bundle_commit.rs','    split_ancestry = ancestry(&current, &split_ancestry);','','restored_lineage_tracks_siblings_reuse_and_undone_splits')
add('actor-exact-cut',core+'actor.rs','.is_some_and(|before| state.history() != *before)','.is_some_and(|before| false && state.history() != *before)','ordered_floor_install_preserves_open_gesture_and_redo','core')
add('restore-redo',core+'actor.rs','                    state = candidate;','                    candidate.redo.clear();\n                    state = candidate;','ordered_floor_install_preserves_open_gesture_and_redo','core')
add('suffix-equivalence',core+'history_transition.rs','        if before != *reconciled {','        if false && before != *reconciled {','recovery_close_follows_suffix_equivalence','core')
add('floor-cut',project+'bundle_branch.rs','                || checkpoint.command_cut.as_ref() != Some(command_cut)\n','','merge_lost_reply_preserves_one_undo_entry')
add('floor-generation',project+'bundle_branch.rs','                || checkpoint.floor_generation != *preceding_floor_generation\n','','merge_lost_reply_preserves_one_undo_entry')
add('shared-history',project+'bundle_recovery.rs','''            .get(&checkpoint.history_ref.sha256)
            .cloned()''','''            .remove(&checkpoint.history_ref.sha256)''','restored_lineage_tracks_siblings_reuse_and_undone_splits')
for name,path,anchor,replacement,witness,target in cases:
 if (out/f'{name}-restored.log.gz').exists():continue
 original=path.read_bytes();source=original.decode();assert source.count(anchor)==1,(name,source.count(anchor))
 args=['-p','kinewright-core','--test','ja_s1_history'] if target=='core' else ['-p','kinewright-agent','--features','eval-harness','--test','ja_s1_branches']
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
