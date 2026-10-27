#!/usr/bin/env python3
"""C6 exact runtime mutations, including App repetitions of C5 and C4 graph controls."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time
sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent

def prior(number):
    spec = importlib.util.spec_from_file_location('c'+number, ROOT/('scripts/ja1-s2-c'+number+'-mutations.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def cases():
    c5 = prior('5')
    registry = {}
    for name, (edits, witness, target, package, diagnostic) in c5.cases().items():
        if name != 'duplicate-retirement-acknowledged' and package == 'kinewright-project' and witness.startswith('c5::'):
            module = 'rescue_cases' if target == 'ja_s2_rescue' else 'recovery_cases'
            witness = 'ja1_attachment::tests::'+module+'::'+witness.split('::',1)[1]
            target, package = '--bin', 'kinewright-app'
        elif witness.startswith('c5_pressure::'):
            witness = 'ja1_attachment::tests::pressure_cases::'+witness.split('::',1)[1]
            target, package = '--bin', 'kinewright-app'
        registry['repeat-'+name] = (edits,witness,target,package,diagnostic)
    c4 = prior('4')
    graph = {'compaction-attachment-drops-independent-root','ack-exact-receipt-lookup-omitted',
        'record-ack-edge-omitted','record-digest-only-ack','two-predecessor-validator-weakened',
        'record-role-admission-omitted','record-edge-admission-omitted','record-schema-accepted',
        'record-omitted-before-publication','provenance-count-unbounded','provenance-overflow-lost',
        'provenance-treated-as-absent','origin-digest-conflict-ignored',
        'l1-document-payload-change','l1-batch-payload-change','l1-input-payload-change',
        'independent-projection-root-dropped','current-provenance-summarized','current-omission-dropped',
        'oversized-raw-role-hidden','archived-raw-switch-requirement-skipped',
        'missing-raw-requirement-accepted','rack-counted-as-third-predecessor',
        'selected-predecessor-dropped','b-activation-rewrites-origin-inputs'}
    for name,(file,old,new,witness) in c4.cases().items():
        if name not in graph: continue
        edits=[(file,old,new)]
        if name.startswith('l1-'):
            edits.append((file,'|| left.issuer_state_ref != right.issuer_state_ref\n    {','|| left.issuer_state_ref != right.issuer_state_ref)\n    {'))
        registry['graph-'+name]=(edits,witness,'ja_s2_rescue_record','kinewright-project',None)
    u='crates/kinewright-app/src/ja1_attachment.rs'
    app='crates/kinewright-app/src/app.rs'
    project='crates/kinewright-project/src/'
    def add(name,file,old,new,witness,package='kinewright-app',target='--bin'):
        registry[name]=([(file,old,new)],witness if package!='kinewright-app' else 'ja1_attachment::tests::'+witness,target,package,None)
    source=(ROOT/u).read_text()
    begin=source.index('            Action::PreviewRecovery(target) => {')
    end=source.index('            Action::OpenRecovery => {',begin)
    block=source[begin:end]
    add('preview-availability-omitted',u,block,block.replace('self.owner.selection_availability()?', 'String::new()'),'w15_late_rescue_is_capture_only_and_release_is_explicit')
    add('handoff-availability-omitted',u,'.push_str(&self.owner.rescue_handoff_disclosure(&selection)?);','.push_str("");','w15_late_rescue_is_capture_only_and_release_is_explicit')
    add('close-availability-omitted',u,'        text\n    }\n    pub fn read_only', '        text = text.replace("This recovery point is no longer in the project\'s automatic list. It is kept only for this selection in this session. Abandoning the selection lets it go.", "");\n        text\n    }\n    pub fn read_only','w13_actual_destination_controls_and_background_freeze')
    for surface in ['save-as','open-project','open-dialog','new-project','direct-write']:
        add('destination-'+surface+'-availability-omitted',u,'    pub fn destination_change(&self) -> Result<(), BundleError> {\n        self.owner.check_destination_change()\n    }','    pub fn destination_change(&self) -> Result<(), BundleError> {\n        Err(BundleError::Identity("Abandon this selection explicitly".into()))\n    }','w13_actual_destination_controls_and_background_freeze')
    add('rescue-action-omitted',u,'ui.button("Save rescue copy")','ui.button("Retry A only")','w03_choices_never_supply_implicit_consent')
    add('frozen-rescue-warning-omitted',u,'"B\'s save outcome is unknown. Editing remains read-only while A\'s save or selection is unresolved."','"B\'s save outcome is unknown."','recovery_cases::w13_frozen_cut_rescue_at_both_unknown_publications_and_late_a_isolation')
    add('storage-close-labelled-safe',project+'editing_session/recovery.rs','Closing can discard the only accessible copy in this session.','Closing is safe.','w13_preview_close_and_every_destination_disclose_session_pin_loss')
    add('modal-removed',app,'egui::Modal::new(egui::Id::new("ja1-busy"))','egui::Window::new("ja1-busy")','w13_actual_destination_controls_and_background_freeze')
    add('producer-ui-freeze-omitted','crates/kinewright-app/src/project.rs','self.ja1_job.is_some()\n            || self','false\n            || self','w13_actual_destination_controls_and_background_freeze')
    add('preliminary-save-labelled-saved',u,'Your current save outcome is unknown. Editing is read-only while it is checked; the recovery point has not been opened.','Saved the current edit.','recovery_cases::w13_preliminary_save_faults_keep_exact_cut_and_do_not_select')
    add('floor-disclosure-omitted',u,block,block.replace('self.save_disclosure = self.owner.recovery_save_disclosure()?;', 'self.save_disclosure.clear();'),'w15_late_rescue_is_capture_only_and_release_is_explicit')
    add('quota-retirement-ui-omitted',u,'self.candidates = self.owner.retirement_candidates().unwrap_or_default();','self.candidates.clear();','w13_owner_reported_blockers_render_save_and_close_routes')
    add('blocker-save-route-omitted',u,'format!("Save this session: {name}")','format!("Session {name}")','w13_owner_reported_blockers_render_save_and_close_routes')
    add('blocker-close-route-omitted',u,'format!("Close this session: {name}")','format!("Session {name}")','w13_owner_reported_blockers_render_save_and_close_routes')
    add('blocker-name-omitted',u,'ui.label(format!("This branch is in use by {name} / specialist base. Save or close that session first."));','ui.label("This branch is in use.");','w13_owner_reported_blockers_render_save_and_close_routes')
    add('trim-consent-supplied-implicitly',u,'trim: false,','trim: true,','w03_trim_choice_requires_its_own_consent_and_keeps_a_exact')
    add('prepared-view-subscription-omitted',project+'editing_session/recovery.rs','        if let Err(error) = prepare(&prepared.restore.core) {','        if let Err(error) = Ok::<(), BundleError>(()) {','w05_capture_races_and_infallible_handoff')
    add('release-unproved-capture',project+'editing_session.rs','if bundle.publication_state() != PublicationState::Committed\n            || bundle.unsettled','if false','w15_completed_capture_release_keeps_sources_and_active_c_exact')
    # Each late-admission mutation performs a real store creation before the refusal.
    add('local-headless-admission-after-store-mutation',project+'headless.rs','    crate::editing_session::Ja1EditingSession::activate(path, admission, core, inputs)?','    let _premature = crate::bundle_copy::SessionStore::create(path.parent().unwrap(), crate::storage_budget::StorageLimits::default())?;\n    crate::editing_session::Ja1EditingSession::activate(path, admission, core, inputs)?','w14_local_headless_admission_precedes_mutation','kinewright-project','ja_s2_rescue_record')
    add('local-agent-admission-after-store-mutation','crates/kinewright-agent/src/local_editing.rs','        Ok(Self {','        let _premature = kinewright_project::bundle_copy::SessionStore::create(path.parent().unwrap(), kinewright_project::storage_budget::StorageLimits::default())?;\n        Ok(Self {','w14_local_agent_admission_precedes_mutation','kinewright-agent','ja_s2_rescue_admission')
    add('app-admission-after-store-mutation',u,'        Self::new(Ja1EditingSession::open(path)?)','        let _premature = kinewright_project::bundle_copy::SessionStore::create(path.parent().unwrap(), StorageLimits::default())?;\n        Self::new(Ja1EditingSession::open(path)?)','w14_unsupported_rescue_refuses_before_session_mutation')
    for name,package,target,witness in [('slow-agent-owner-dropped','kinewright-agent','ja_s2_rescue_e2e','rescue_reopen_recopy_retention_stress'),('slow-app-owner-dropped','kinewright-app','--bin','rescue_complete_app_lifecycle_stress')]:
        add(name,project+'editing_session/rescue.rs','                self.retained_destinations.push((original, old_core));','                drop((original, old_core));',witness,package,target)
    add('trim-control-does-not-grant-consent',u,'                selection.consent_trim(consent)?;', '                let _ = &consent;', 'rescue_cases::w13_trim_then_separate_omission_recomputes_closure')
    add('omission-control-does-not-grant-consent',u,'                selection.consent_omission(consent)?;', '                let _ = &consent;', 'rescue_cases::w13_trim_then_separate_omission_recomputes_closure')
    add('source-labelled-as-capture-copy',u,'if state == PublicationState::Committed && self.owner.is_capture_destination(&path)', 'if state == PublicationState::Committed', 'rescue_cases::w10_pending_command_exclusion_and_original_id_lookup')
    add('legacy-inspection-flag-blocks-studio-save',app,'if session.recovery_suspended && session.ja1.is_none() && session.ja1_job.is_none()', 'if session.recovery_suspended', 'w13_actual_destination_controls_and_background_freeze')
    add('focus-drops-originating-owner',app,'        self.focus_project_with_rebind(index, false);', '        drop(self.focused_mut().ja1.take()); self.focus_project_with_rebind(index, false);', 'w08_focus_and_status_close_preserve_originating_context')
    add('additive-app-handoff-disclosure-omitted',u,'.push_str(&self.owner.rescue_handoff_disclosure(&selection)?);', '.push_str("");', 's2_additive_app_fixture_observes_real_controls','kinewright-agent','ja_s1')
    edits, witness, target, package, diagnostic = registry['preview-availability-omitted']
    registry['preview-availability-omitted'] = (edits+[(u, '        if self.owner.has_prepared_selection() {\n            ui.label', '        if self.owner.has_prepared_selection() && !matches!(self.choice, Some(Choice::Recovery)) {\n            ui.label')], witness, target, package, diagnostic)
    edits, witness, target, package, diagnostic = registry['blocker-name-omitted']
    registry['blocker-name-omitted'] = (edits+[(u, 'blockers.join(" ")', 'String::new()')], witness, target, package, diagnostic)
    return registry

def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()

def wait_memory():
    deadline = time.monotonic() + 600
    while int(next(s.split()[1] for s in Path('/proc/meminfo').read_text().splitlines() if s.startswith('MemAvailable:'))) < 6291456:
        if time.monotonic() > deadline:
            raise SystemExit('6 GiB memory prerequisite unavailable')
        time.sleep(5)

def cargo_args(name, package, target):
    args = ['test', '-p', package]
    if package == 'kinewright-agent':
        args += ['--features', 'eval-harness,slow-tests' if name.startswith('slow-') else 'eval-harness']
    elif package == 'kinewright-project':
        args += ['--features', 'test-util']
    elif name.startswith('slow-'):
        args += ['--features', 'slow-tests']
    return args + (['--lib'] if target == '--lib' else ['--bin', 'kinewright-app'] if target == '--bin' else ['--test', target])

def build_test_binary(args, directory, label):
    wait_memory()
    command = ['nice', '-n', '10', 'cargo', *args, '--no-run', '--message-format=json']
    (directory/(label+'.command.json')).write_text(json.dumps(command)+'\n')
    build = subprocess.run(command, cwd=ROOT, capture_output=True, text=True, check=False, timeout=600)
    (directory/(label+'.stdout.log')).write_text(build.stdout)
    (directory/(label+'.stderr.log')).write_text(build.stderr)
    if build.returncode:
        raise SystemExit(f'{label}: build prerequisite failed; NOT a runtime red')
    package = args[args.index('-p')+1]
    artifacts = [json.loads(line) for line in build.stdout.splitlines() if line.startswith('{')]
    candidates = [a for a in artifacts if a.get('reason') == 'compiler-artifact'
        and a.get('executable') and a.get('profile', {}).get('test')
        and a.get('manifest_path') == str(ROOT/'crates'/package/'Cargo.toml')]
    if len(candidates) != 1:
        raise SystemExit(f'{label}: missing/ambiguous selected test binary')
    return candidates[0]

def main():
    evidence = Path(sys.argv[1]).resolve()
    evidence.mkdir(parents=True, exist_ok=True)
    registry = cases()
    selected = sys.argv[2:] or list(registry)
    identity = ROOT/'scripts/ja1-s2-c1-evidence.py'
    subprocess.run([sys.executable, '-I', str(identity), 'identity', str(evidence/'source-before.json')], cwd=ROOT, check=True)
    source_bytes = (evidence/'source-before.json').read_bytes()
    source_digest = hashlib.sha256(source_bytes).hexdigest()
    rustc = subprocess.check_output(['rustc', '-Vv'], cwd=ROOT)
    (evidence/'rustc.txt').write_bytes(rustc)
    (evidence/'environment.json').write_text(json.dumps({
        'cwd': str(ROOT), 'profile': 'test', 'green_execution': 'exact sealed original-source test binary',
        'environment': {key: os.environ.get(key) for key in ['CARGO_TARGET_DIR', 'TMPDIR', 'CARGO_BUILD_JOBS', 'RUST_TEST_THREADS', 'FFMPEG_DIR', 'RUSTFLAGS', 'LD_LIBRARY_PATH']},
        'status': subprocess.check_output(['git','status','--porcelain'],cwd=ROOT,text=True),
    }, indent=2)+'\n')
    baselines = {}
    configs = [cargo_args(name, registry[name][3], registry[name][2]) for name in selected]
    app_args = cargo_args('ordinary', 'kinewright-app', '--bin')
    if 'additive-app-handoff-disclosure-omitted' in selected:
        configs.append(app_args)
    for args in configs:
        key = hashlib.sha256(json.dumps(args).encode()).hexdigest()[:16]
        if key in baselines:
            continue
        directory = evidence/'baselines'/key
        directory.mkdir(parents=True, exist_ok=True)
        index_path = directory/'identity.json'
        saved = json.loads(index_path.read_text()) if index_path.exists() else None
        if not (saved and saved['source_sha256'] == source_digest and saved['rustc'] == rustc.decode()
                and saved['args'] == args and Path(saved['binary']).is_file()
                and digest(saved['binary']) == saved['binary_sha256']):
            artifact = build_test_binary(args, directory, 'build')
            original = Path(artifact['executable'])
            binary_sha = digest(original)
            binary = directory/(original.name+'-'+binary_sha)
            shutil.copy2(original, binary)
            saved = {'source_sha256':source_digest, 'rustc':rustc.decode(), 'args':args,
                'binary':str(binary), 'binary_sha256':binary_sha, 'compiler_profile':artifact['profile']}
            index_path.write_text(json.dumps(saved,indent=2)+'\n')
            (directory/'source.json').write_bytes(source_bytes)
        listing = subprocess.run(['nice','-n','10',saved['binary'],'--list'],cwd=ROOT,capture_output=True,text=True,check=False,timeout=600)
        (directory/'list.log').write_text(listing.stdout+'\n'+listing.stderr)
        if listing.returncode:
            raise SystemExit('sealed original binary cannot enumerate; NOT RUN')
        saved['compiled'] = re.findall(r'^(.+): test$', listing.stdout, re.M)
        saved['index_path'] = str(index_path)
        baselines[key] = saved
        print(f'sealed original {args}: {len(saved["compiled"])} compiled tests',flush=True)
    subprocess.run([sys.executable,'-I',str(identity),'identity',str(evidence/'source-after-baseline-builds.json')],cwd=ROOT,check=True)
    if (evidence/'source-after-baseline-builds.json').read_bytes() != source_bytes:
        raise SystemExit('source changed during baseline builds')
    app_key = hashlib.sha256(json.dumps(app_args).encode()).hexdigest()[:16]
    for name in selected:
        edits, witness, target, package, diagnostic = registry[name]
        args = cargo_args(name, package, target)
        key = hashlib.sha256(json.dumps(args).encode()).hexdigest()[:16]
        baseline = baselines[key]
        if witness not in baseline['compiled']:
            raise SystemExit(f'{name}: exact witness absent from sealed original')
        originals = {file:(ROOT/file).read_bytes() for file,_,_ in edits}
        mutated = {file:data.decode() for file,data in originals.items()}
        for file, old, new in edits:
            if mutated[file].count(old) != 1:
                raise SystemExit(f'{name}: replacement count {mutated[file].count(old)}')
            mutated[file] = mutated[file].replace(old,new)
        directory = evidence/name
        directory.mkdir(exist_ok=True)
        (directory/'source.json').write_bytes(source_bytes)
        (directory/'identity.json').write_text(json.dumps({'witness':witness,'target':target,'package':package,
            'edits':[{'file':file,'old':old,'new':new} for file,old,new in edits],
            'original_sha256':{file:hashlib.sha256(data).hexdigest() for file,data in originals.items()},
            'mutated_sha256':{file:hashlib.sha256(data.encode()).hexdigest() for file,data in mutated.items()}},indent=2)+'\n')
        try:
            for phase in ['red','green']:
                for file, original in originals.items():
                    (ROOT/file).write_bytes(mutated[file].encode() if phase == 'red' else original)
                wait_memory()
                environment = os.environ.copy()
                if name.startswith('graph-l1-'):
                    environment['JA_C4_L1_CASE'] = name.split('-')[2]
                if name.startswith('destination-'):
                    surface = name.removeprefix('destination-').removesuffix('-availability-omitted')
                    environment['JA_C6_DESTINATION_CASE'] = str(['save-as','open-project','open-dialog','new-project','direct-write'].index(surface))
                if name == 'additive-app-handoff-disclosure-omitted':
                    if phase == 'red':
                        app = build_test_binary(app_args, directory, 'red.app-build')
                        app_binary = app['executable']
                    else:
                        app_binary = baselines[app_key]['binary']
                    environment['JA_HARNESS_APP_TEST_BINARY'] = app_binary
                    (directory/(phase+'.app-binary.json')).write_text(json.dumps({'path':app_binary,'sha256':digest(app_binary)})+'\n')
                test_args = [witness,'--exact','--nocapture','--test-threads=4']
                if witness == 's2_additive_app_fixture_observes_real_controls':
                    test_args.append('--include-ignored')
                if phase == 'red':
                    command = ['nice','-n','10','cargo',*args,witness,'--',*test_args[1:]]
                else:
                    if digest(baseline['binary']) != baseline['binary_sha256']:
                        raise SystemExit('sealed original changed')
                    command = ['nice','-n','10',baseline['binary'],*test_args]
                (directory/(phase+'.command.json')).write_text(json.dumps(command)+'\n')
                (directory/(phase+'.environment.json')).write_text(json.dumps({k:environment.get(k) for k in ['LD_LIBRARY_PATH','JA_C4_L1_CASE','JA_C6_DESTINATION_CASE','JA_HARNESS_APP_TEST_BINARY']},indent=2)+'\n')
                started_ns = time.time_ns()
                with (directory/(phase+'.log')).open('w') as log:
                    result = subprocess.run(command,cwd=ROOT,env=environment,stdout=log,stderr=subprocess.STDOUT,check=False,timeout=600)
                output = (directory/(phase+'.log')).read_text()
                summaries = re.findall(r'^test result:.*$',output,re.M)
                pattern = r'^test result: FAILED\. 0 passed; 1 failed; 0 ignored;' if phase == 'red' else r'^test result: ok\. 1 passed; 0 failed; 0 ignored;'
                if result.returncode != (101 if phase == 'red' else 0) or len(summaries) != 1 or not re.search(pattern,output,re.M):
                    print(output[-8000:]); raise SystemExit(f'{name}/{phase}: expected one runtime outcome, exit {result.returncode}')
                if phase == 'red' and diagnostic and diagnostic not in output:
                    raise SystemExit(f'{name}: did not reach oracle {diagnostic!r}')
                binary = (ROOT/re.findall(r'^\s*Running .* \(([^()]+)\)$',output,re.M)[0]).resolve() if phase == 'red' else Path(baseline['binary'])
                (directory/(phase+'.result.json')).write_text(json.dumps({'exit':result.returncode,'raw_summary':summaries,
                    'binary':str(binary),'binary_sha256':digest(binary),'started_ns':started_ns,'finished_ns':time.time_ns(),
                    'original_baseline_identity':baseline['index_path'] if phase == 'green' else None},indent=2)+'\n')
                print(f'{name}/{phase}: {summaries[0]}',flush=True)
        finally:
            for file,data in originals.items():
                (ROOT/file).write_bytes(data)
    subprocess.run([sys.executable,'-I',str(identity),'identity',str(evidence/'source-after.json')],cwd=ROOT,check=True)
    if (evidence/'source-after.json').read_bytes() != source_bytes:
        raise SystemExit('source identity changed across mutation run')

if __name__ == '__main__': main()
