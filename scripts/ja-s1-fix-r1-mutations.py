"""Replay final-head review mutations, one named behavioral witness per mutation."""
from pathlib import Path
import gzip
import hashlib
import json
import os
import re
import signal
import subprocess
import sys
import time

out = Path(sys.argv[1]).resolve()
out.mkdir(parents=True, exist_ok=True)
project = "crates/kinewright-project/src/"
namespace = project + "namespace/linux.rs"
commit = project + "bundle_commit.rs"
receipts = project + "receipts.rs"
bundle = project + "bundle.rs"
store = project + "store.rs"
F = "final_head::"
cases = {
    "original-app-regressions": ("crates/kinewright-app/src/app.rs", "kinewright-app", "five-final-head-app-regressions"),
    "ambiguous-rename": (namespace, "ja_s1_storage", F+"ambiguous_rename_error_requires_reconciliation"),
    "active-lut-context": (commit, "ja_s1_storage", F+"reused_lut_id_retains_both_content_identities_through_first_save_and_recovery"),
    "lut-identity": (commit, "ja_s1_storage", F+"reused_lut_id_retains_both_content_identities_through_first_save_and_recovery"),
    "pending-directory-barriers": (project+"namespace/mod.rs", "ja_s1_storage", F+"directory_sync_failure_retains_barriers_and_retry_seals_existing_ancestors"),
    "existing-directory-barriers": (namespace, "ja_s1_storage", F+"directory_sync_failure_retains_barriers_and_retry_seals_existing_ancestors"),
    "unchanged-history-cache": (store, "ja_s1_receipts", "unchanged_comment_skips_history_decode_and_coalesces_directory_syncs"),
    "object-directory-coalescing": (namespace, "ja_s1_receipts", "unchanged_comment_skips_history_decode_and_coalesces_directory_syncs"),
    "issuer-lifetime-cap": (receipts, "ja_s1_receipts", "two_hundred_resolved_issuers_retire_without_a_lifetime_cap"),
    "history-floor": (commit, "ja_s1_storage", F+"realistic_history_saves_every_edit_and_advances_durable_floor"),
    "native-layout-decode-charge": (store, "ja_s1_storage", F+"realistic_history_saves_every_edit_and_advances_durable_floor"),
    "legacy-size-cap": (bundle, "ja_s1_storage", F+"legacy_large_document_keeps_its_previous_open_size_contract"),
    "post-publication-gc": (commit, "ja_s1_storage", F+"post_ack_gc_failure_never_changes_committed_outcome_and_retries"),
    "paired-io-production-fallback": (namespace, "ja_s1", "strict_io_properties_revised_use_actual_syncs_and_no_fallback"),
    "production-rename-fallback": (namespace, "ja_s1_storage", "strict_replace_refusal_preserves_exact_root"),
    "alias-after-acquisition": (namespace, "ja_s1_storage", "namespace_remains_bound_after_alias_swap"),
    "parent-directory-sync": (namespace, "ja_s1_storage", F+"save_as_new_destination_syncs_each_child_and_parent_before_root"),
    "destination-directory-syncs": (namespace, "ja_s1_storage", F+"save_as_new_destination_syncs_each_child_and_parent_before_root"),
    "baseline-continuation": (commit, "ja_s1_storage", F+"baseline_change_is_not_a_continuation_of_a_zero_batch_checkpoint"),
    "invalid-command-pending": (commit, "ja_s1_receipts", "invalid_comment_does_not_enter_pending_or_pause_the_next_command"),
    "retired-predecessor-label": (receipts, "ja_s1_receipts", "retired_unresolved_never_reapplies_or_acknowledges"),
    "orphan-quota-recovery": (store, "ja_s1_storage", F+"verified_reopen_retires_over_quota_orphan_before_admission"),
    "orphan-retirement": (store, "ja_s1_storage", F+"verified_reopen_retires_unreachable_generated_orphans_and_their_charges"),
    "activation-legacy-write": (bundle, "ja_s1_storage", F+"migration_retries_b3_lost_b5_and_corrupt_backup_without_duplicate_identity"),
    "migration-backup": (bundle, "ja_s1_storage", F+"migration_retries_b3_lost_b5_and_corrupt_backup_without_duplicate_identity"),
    "archived-history-residency": (project+"bundle_recovery.rs", "ja_s1_storage", F+"realistic_history_saves_every_edit_and_advances_durable_floor"),
}

def replace(source, before, after, count=1):
    assert source.count(before) == count, (before, source.count(before))
    return source.replace(before, after)

def mutate(source, name):
    if name == "original-app-regressions":
        return subprocess.check_output(['git','show','0f4f88e:crates/kinewright-app/src/app.rs'],text=True)
    if name == "ambiguous-rename":
        return replace(source, "if error != rustix::io::Errno::ACCESS {", "if false && error != rustix::io::Errno::ACCESS {")
    if name == "active-lut-context":
        return replace(source, '        validate_document_inputs(&checked.document, &manifest)?;', '')
    if name == "lut-identity":
        source = replace(source, "&& e.object.sha256 == asset.sha256", "")
        return replace(source, "&& e.object.byte_length == asset.byte_len", "")
    if name == "pending-directory-barriers":
        return replace(source, 'self.uncertain || {', 'self.uncertain || false && {')
    if name == "existing-directory-barriers":
        return replace(source, '        if create {\n            // Keep both barriers', '        if created {\n            // Keep both barriers')
    if name == "unchanged-history-cache":
        return replace(source, 'if let Some(cached) = ledger.support_fields.get(&reference.sha256)', 'if let Some(cached) = None::<&StorageReservation<BTreeSet<String>>>')
    if name == "object-directory-coalescing":
        return replace(source, 'if !namespace.batch_objects {', 'if true {')
    if name == "issuer-lifetime-cap":
        source = replace(source, "self.issuers.remove(id);", "let _ = id;")
        return replace(source, '        if first_sequence == 0', '        if self.issuers.len() as u64 >= self.store.limits().issuers { return Err(BundleError::Capacity("issuer entries")); }\n        if first_sequence == 0')
    if name == "history-floor":
        return replace(source, 'for cut in 1..=history.batches.len()', 'for cut in 1..=0')
    if name == "native-layout-decode-charge":
        return replace(source, 'self.charge(64)?;', 'self.charge(1200)?;')
    if name == "legacy-size-cap":
        return replace(source, 'let holds = if legacy {', 'let holds = if false && legacy {')
    if name == "post-publication-gc":
        return replace(source, '        drop(seals);\n        Ok(BundleSaveReport', '        drop(seals);\n        bundle.store.allow_collection()?;\n        bundle.store.collect_unpublished()?;\n        Ok(BundleSaveReport')
    if name in ("production-rename-fallback", "paired-io-production-fallback"):
        return replace(source, '            // Permission refusal is definite; other errors can leave publication ambiguous.', '''            let mut target = File::from(fs::openat(&namespace.parent, &namespace.root_name, OFlags::WRONLY | OFlags::TRUNC | OFlags::NOFOLLOW, Mode::empty())?);
            target.write_all(bytes)?;
            note("in_place_fallback", &namespace.identity.resolved_root);''')
    if name == "alias-after-acquisition":
        return replace(source, '    claim(&mut namespace)?;\n    Ok(namespace)', '    claim(&mut namespace)?;\n    namespace.root_name = path.file_name().unwrap().to_os_string();\n    Ok(namespace)')
    if name == "parent-directory-sync":
        return replace(source, '        sync_directory(handle, &path).map_err(|_| BundleError::DurabilityUnknown)?;', '        if !*parent { sync_directory(handle, &path).map_err(|_| BundleError::DurabilityUnknown)?; }')
    if name == "destination-directory-syncs":
        a = source.index('pub(super) fn prepare_destination(')
        b = source.index('pub(super) fn matches_destination(', a)
        part = source[a:b]
        part = replace(part, '        sync_directory(&next, &actual).map_err(|_| BundleError::DurabilityUnknown)?;', '')
        part, n = re.subn(r'        sync_directory\(\n            &parent,\n            actual.*?\n        \)\?;', '', part, count=1, flags=re.S)
        assert n == 1
        source = source[:a]+part+source[b:]
        a = source.index('pub(super) fn seal_destination_ancestors(')
        b = source.index('pub(super) fn matches_destination(', a)
        return source[:a] + 'pub(super) fn seal_destination_ancestors(_namespace: &ProjectNamespace) -> Result<(), BundleError> { Ok(()) }\n' + source[b:]
    if name == "baseline-continuation":
        source, n = re.subn(r'aligned.baseline != old.baseline\s*\|\| ', '', source, count=1)
        assert n == 1
        return source
    if name == "invalid-command-pending":
        a = source.index('match crate::receipts::stage_comment(')
        b = source.index('bundle.pending_command = Some', a)
        part = source[a:b]
        part = replace(part, 'return Err(error);', 'bundle.unsettled = true;\n bundle.pending_command = Some(reservation.hold(envelope.clone()));\n return Err(error);')
        return source[:a]+part+source[b:]
    if name == "retired-predecessor-label":
        return replace(source, 'ReceiptLookup::Committed(receipt)\n                            if self', 'ReceiptLookup::Committed(receipt)\n                            if false && self')
    if name == "orphan-quota-recovery":
        return replace(source, 'if verified_graph && generated_orphan {', 'if false && verified_graph && generated_orphan {')
    if name == "orphan-retirement":
        return replace(source, 'for (hash, length) in orphans {', 'for (hash, length) in orphans.into_iter().take(0) {')
    if name == "activation-legacy-write":
        a=source.index('fn refuse_activation_journal(')
        part=source[a:]
        part=part.replace('    if path', '    if false && path', 1)
        return source[:a]+part
    if name == "migration-backup":
        return replace(source, 'if let Some(original) = original {', 'if let Some(original) = original.filter(|_| false) {')
    if name == "archived-history-residency":
        return replace(source, '        if active {\n            histories.insert', '        if true {\n            histories.insert')
    raise AssertionError(name)

def run_cases(selected, mutator=mutate):
    def interrupted(signum, _frame):
        raise SystemExit(f"interrupted by signal {signum}")
    signal.signal(signal.SIGTERM, interrupted)
    rows = json.loads((out/'results.json').read_text()) if (out/'results.json').exists() else []
    for name in selected:
        file, target, witness = cases[name]
        path=Path(file)
        original=path.read_text()
        modified=mutator(original, name)
        assert modified != original, name
        try:
            for phase, content in [("red", modified), ("restored", original)]:
                path.write_text(content)
                wait_started = time.monotonic()
                while int(next(l.split()[1] for l in Path('/proc/meminfo').read_text().splitlines() if l.startswith('MemAvailable:'))) < 6291456:
                    if time.monotonic() - wait_started > 7200: raise SystemExit('memory blocked for over two hours')
                    print(f'{name}: waiting for 6 GiB', flush=True)
                    time.sleep(30)
                memory = subprocess.check_output(['free', '-g'], text=True)
                assert int(next(l.split()[6] for l in memory.splitlines() if l.startswith('Mem:'))) >= 6
                package, feature = ('kinewright-agent','eval-harness') if name == 'paired-io-production-fallback' else ('kinewright-project','test-util')
                if target in ('ja_s1_branches', 'ja_baseline'): package, feature = 'kinewright-agent', 'eval-harness'
                if name == 'original-app-regressions':
                    filters=['app::in1_tests::in1b_the_production_half_reader_is_line_ending_agnostic','app::in1_tests::in1b_the_unavailable_look_list_is_one_incident_with_a_joined_observed','app::in2b_tests::in2b_first_dedup_into_loaded_joins_opened_and_second_does_not','app::in2b_tests::in2b_load_starts_no_session_and_presses_queue','app::in2b_tests::in2b_investigate_refuses_a_missing_subject']
                    args=['nice','-n','10','cargo','test','-p','kinewright-app','--bin','kinewright-app','--','--exact',*filters]
                elif target in ('app-bin', 'media-lib', 'core-history'):
                    package = {'app-bin':'kinewright-app','media-lib':'kinewright-media','core-history':'kinewright-core'}[target]
                    selector = ['--bin','kinewright-app'] if target == 'app-bin' else ['--lib'] if target == 'media-lib' else ['--test','ja_s1_history']
                    args = ['nice','-n','10','cargo','test','-p',package,*selector,witness,'--','--exact']
                else:
                    args=['nice','-n','10','cargo','test','-p',package,'--features',feature,'--test',target,witness,'--','--exact']
                identity={'head':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(), 'diff_sha256':hashlib.sha256(subprocess.check_output(['git','diff','--binary'])).hexdigest(), 'file_sha256':hashlib.sha256(path.read_bytes()).hexdigest(), 'command':args, 'memory':memory, 'baseline_file_sha256':hashlib.sha256(original.encode()).hexdigest()}
                result=subprocess.run(args,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
                output=result.stdout
                binaries=re.findall(r'Running [^\n]+ \(([^)]+)\)',output)
                assert binaries, (name, phase, output[-6000:])
                identity['witness_sources_sha256']={str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in Path('crates').rglob('*.rs') if 'target' not in p.parts}
                identity['binary_sha256']=hashlib.sha256(Path(binaries[-1]).read_bytes()).hexdigest()
                with gzip.open(out/f'{name}-{phase}.log.gz','wt') as log:
                    log.write(json.dumps(identity)+'\n'+output)
                summaries=re.findall(r'^test result: .+$',output,re.M)
                count=5 if name=='original-app-regressions' else 1
                expected=f'FAILED. 0 passed; {count} failed;' if phase=='red' else f'ok. {count} passed; 0 failed;'
                assert result.returncode == (101 if phase=='red' else 0) and expected in summaries[-1], (name,phase,summaries,output[-6000:])
                if name == 'alias-after-acquisition' and phase == 'red':
                    assert 'assertion `left == right` failed' in output, 'alias mutation failed before the byte property'
                rows.append({'mutation':name,'phase':phase,'witness':witness,'summary':summaries[-1],**identity})
                print(name,phase,summaries[-1],flush=True)
        finally:
            path.write_text(original)
        (out/'results.json').write_text(json.dumps(rows,indent=2)+'\n')


if __name__ == "__main__":
    run_cases(sys.argv[2:] or list(cases))
