"""One focused runtime or compile-fail property per round-six mechanism mutation."""
from pathlib import Path
import gzip
import hashlib
import json
import os
import re
import subprocess
import sys

assert os.getpriority(os.PRIO_PROCESS,0) == 19
out = Path(sys.argv[1]).resolve()
out.mkdir(parents=True,exist_ok=True)
P='crates/kinewright-project/src/'
C='crates/kinewright-core/src/'
R='round_six::'
cases = {
 'root-boundary': (P+'bundle_commit.rs','project',R+'root_staging_and_cleanup_failure_releases_first_save_to_another_destination'),
 'root-cleanup-retry': (P+'namespace/linux.rs','project',R+'root_staging_and_cleanup_failure_releases_first_save_to_another_destination'),
 'first-copy-source': (P+'store.rs','project',R+'reconciled_first_copy_does_not_gate_the_session_on_its_old_destination'),
 'named-copy-source': (P+'store.rs','project',R+'reconciled_save_as_does_not_gate_the_source_owners_next_save'),
 'active-copy-source': (P+'bundle_copy.rs','project',R+'pending_copy_source_reconciles_before_the_next_first_save'),
 'failed-commit-release': (C+'actor.rs','core','failed_commit_releases_the_ticket_with_a_typed_internal_error'),
 'typed-restore': (C+'actor.rs','core','restore_during_a_save_returns_typed_contention_and_keeps_entries'),
 'proof-binding': (C+'actor.rs','core','another_proof_type_cannot_take_over_a_history_save_owner'),
 'untagged-retention': (C+'actor.rs','core','another_proof_type_cannot_take_over_a_history_save_owner'),
 'raw-api-visibility': (C+'actor.rs','doc','history_transition::HistorySaveOwner'),
 'floor-cache-read': (P+'bundle.rs','project',R+'durable_floor_uses_only_the_current_checkpoint_history_cache'),
 'floor-cache-binding': (P+'bundle.rs','project',R+'durable_floor_uses_only_the_current_checkpoint_history_cache'),
}
def replace(s,a,b):
    assert s.count(a)==1,(a,s.count(a))
    return s.replace(a,b)
def mutate(s,name):
    if name=='root-boundary':
        return replace(s,'if failure.boundary == crate::namespace::RootReplaceBoundary::Unknown {','if bundle.store.with_namespace(|n| Ok(n.is_uncertain()))? {')
    if name=='root-cleanup-retry':
        return replace(s,'if let Some(temporary) = &namespace.pending_root_cleanup {','if let Some(temporary) = namespace.pending_root_cleanup.as_ref().filter(|_| false) {')
    if name in ('first-copy-source','named-copy-source'):
        return replace(s,'pub(crate) const fn requires_reconciliation(&self) -> bool {\n        self.uncertain', 'pub(crate) const fn requires_reconciliation(&self) -> bool {\n        true')
    if name=='active-copy-source':
        a=s.index('pub(crate) fn settle_copy_source('); b=s.index('impl BundleCopy',a)
        return s[:a]+replace(s[a:b],'.reconciled();', '.mark_uncertain()?;')+s[b:]
    if name=='failed-commit-release':
        return replace(s,'        if result.is_err() {\n            self.pending_floor = None;\n            return Err(HistoryError::CommitInvariant);', '        if result.is_err() {\n            return Err(HistoryError::CommitInvariant);')
    if name=='typed-restore':
        return replace(s,'let admitted = if state.pending_floor.is_some() {\n                    Err(HistoryError::SaveInProgress)', 'let admitted = if state.pending_floor.is_some() {\n                    Err(HistoryError::Corrupt)')
    if name=='proof-binding':
        return replace(s,'if owner.is_some_and(|existing| existing != requested) {', 'if false && owner.is_some_and(|existing| existing != requested) {')
    if name=='untagged-retention':
        return replace(s,'if ready && pending.save.tag.is_none() {', 'if false && ready && pending.save.tag.is_none() {')
    if name=='raw-api-visibility':
        s=replace(s,'pub(crate) fn begin_history_floor(', 'pub fn begin_history_floor(')
        return replace(s,'pub(crate) fn settle_history_save(', 'pub fn settle_history_save(')
    if name=='floor-cache-read':
        return replace(s,'if let Some(cached) = self.cached_history(branch) {', 'if let Some(cached) = self.cached_history(branch).filter(|_| false) {')
    if name=='floor-cache-binding':
        return replace(s,'.filter(|cached| &cached.reference == current)', '.filter(|cached| &cached.reference == current || true)')
    raise AssertionError(name)
selected=sys.argv[2:] or list(cases)
if selected==['--check']:
    for name,(file,_,_) in cases.items():
        assert mutate(Path(file).read_text(),name)!=Path(file).read_text()
        print(name,'applies')
    raise SystemExit(0)
rows=[]
for name in selected:
    file,target,witness=cases[name]
    path=Path(file)
    original=path.read_text()
    modified=mutate(original,name)
    try:
        for phase,source in [('red',modified),('restored',original)]:
            path.write_text(source)
            memory=subprocess.check_output(['free','-g'],text=True)
            assert int(next(line.split()[6] for line in memory.splitlines() if line.startswith('Mem:'))) >=6,memory
            command=['nice','-n','10','cargo','test','-p','kinewright-project','--features','test-util','--test','ja_s1_storage',witness,'--','--exact','--test-threads=1']
            if target=='core':
                command=['nice','-n','10','cargo','test','-p','kinewright-core','--test','ja_s1_history',witness,'--','--exact','--test-threads=4']
            if target=='doc':
                command=['nice','-n','10','cargo','test','-p','kinewright-core','--doc',witness]
            identity={'head':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
                      'diff_sha256':hashlib.sha256(subprocess.check_output(['git','diff','--binary'])).hexdigest(),
                      'sources':{str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in Path('crates').rglob('*.rs') if 'target' not in p.parts},
                      'memory':memory,'command':command}
            result=subprocess.run(['timeout','180','bash','-c','source scripts/setup-ffmpeg.sh && exec "$@"','r6-mutation',*command],env=dict(os.environ,CARGO_BUILD_JOBS='4',RUST_TEST_THREADS='4'),capture_output=True,text=True)
            output=result.stdout+result.stderr
            binaries=re.findall(r'Running [^\n]+ \(([^)]+)\)',output)
            if target!='doc':
                assert binaries,(name,phase,output[-5000:])
                identity['binary_sha256']=hashlib.file_digest(Path(binaries[-1]).open('rb'),'sha256').hexdigest()
            with gzip.open(out/f'{name}-{phase}.log.gz','wt') as log:
                log.write(json.dumps(identity)+'\n'+output)
            summaries=re.findall(r'^test result: .+$',output,re.M)
            count=2 if target=='doc' else 1
            expected=f'FAILED. 0 passed; {count} failed;' if phase=='red' else f'ok. {count} passed; 0 failed;'
            assert summaries and expected in summaries[-1] and result.returncode==(101 if phase=='red' else 0),(name,phase,summaries,output[-5000:])
            rows.append({'mutation':name,'phase':phase,'witness':witness,'summary':summaries[-1],**identity})
            (out/'results.json').write_text(json.dumps(rows,indent=2)+'\n')
            print(name,phase,summaries[-1],flush=True)
    finally:
        path.write_text(original)
