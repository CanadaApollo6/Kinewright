#!/usr/bin/env python3
"""C3 single-witness runtime mutations with source restoration and exact counts.

Source FFmpeg and use a private CARGO_TARGET_DIR. Invoke at nice 19 with python3 -I.
Compile errors, empty selections and timeouts never earn a behavioral red.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
ROOT = Path(__file__).resolve().parent.parent
E = 'crates/kinewright-project/src/editing_session.rs'
R = 'crates/kinewright-project/src/bundle_rescue.rs'
S = 'crates/kinewright-project/src/store.rs'
N = 'crates/kinewright-project/src/namespace/linux.rs'
B = 'crates/kinewright-project/src/bundle_copy.rs'
H = 'crates/kinewright-project/src/history_save.rs'
P = 'c3::'

def cases():
    return {
        'retained-membership': (S, 'if !self.objects.contains(reference) {', 'if false && !self.objects.contains(reference) {', 'project', P+'w02_retained_handle_permission_boundaries'),
        'retained-hash': (S, 'if file.metadata()?.len() != reference.byte_length\n            || !digest.as_ref().is_ok_and(|h| h == &reference.sha256)', 'if file.metadata()?.len() != reference.byte_length\n            || false && !digest.as_ref().is_ok_and(|h| h == &reference.sha256)', 'project', P+'w02_retained_handle_permission_boundaries'),
        'permission-bypass': (S, 'store.open_copy_source(reference, true)', 'store.open_copy_source(reference, true).or_else(|_| File::open("/dev/null").map_err(BundleError::from))', 'project', P+'w02_retained_handle_permission_boundaries'),
        'local-source-priority': (E, 'if unsaved\n', 'if false && unsaved\n', 'project', P+'w02_first_save_capture_prefers_local_owned_bytes'),
        'missing-diagnostic': (S, 'let total = missing.len();', 'missing.pop_first();\n        let total = missing.len();', 'project', P+'w02_required_missing_bytes_and_incomplete_inventory'),
        'incomplete-inventory': (S, 'complete = false;\n                    uses.entry(root.clone())', 'complete = true;\n                    uses.entry(root.clone())', 'project', P+'w02_required_missing_bytes_and_incomplete_inventory'),
        'eligibility-phase': (R, 'state == PublicationState::Unknown', 'true', 'project-lib', 'bundle_rescue::tests::w03_cause_table_preserves_native_errors_and_exclusions'),
        'eligibility-interruption': (R, 'K::Interrupted => C::Interrupted,', 'K::Interrupted => C::Unavailable,', 'project-lib', 'bundle_rescue::tests::w03_cause_table_preserves_native_errors_and_exclusions'),
        'eligibility-cleanup': (R, 'return C::CleanupOnly;', 'return C::Persistence;', 'project-lib', 'bundle_rescue::tests::w03_cause_table_preserves_native_errors_and_exclusions'),
        'stale-consent': (R, 'if self.tag != consent.tag || self.digest != consent.digest {', 'if false && (self.tag != consent.tag || self.digest != consent.digest) {', 'project', P+'w03_changed_capture_invalidates_consent'),
        'capture-digest': (E, 'let digest = kinewright_media::sha256_bytes(&binding);', 'let _ = binding; let digest = kinewright_media::sha256_bytes(&[]);', 'project', P+'w03_changed_capture_invalidates_consent'),
        'early-adoption': (E, 'let was_frozen = self', 'let _early = self.rescue.slots.begin(second.tag.clone())?;\n        let was_frozen = self', 'project', P+'w03_probe_consent_and_premutation_cancel'),
        'lost-parent-subtree': (N, 'let lost_parent = verify_parent(source).is_err();', 'let lost_parent = false;', 'project', P+'w06_native_path_identity_and_missing_mount_separation'),
        'alias-follow': (N, '.union(OFlags::NOFOLLOW)', '.union(OFlags::empty())', 'project', P+'w06_native_path_identity_and_missing_mount_separation'),
        'inspection-reopen': (N, 'if current.ancestor != inspection.ancestor\n        || held.st_dev != actual.st_dev\n        || held.st_ino != actual.st_ino', 'if false && (current.ancestor != inspection.ancestor\n        || held.st_dev != actual.st_dev\n        || held.st_ino != actual.st_ino)', 'project', P+'w06_native_path_identity_and_missing_mount_separation'),
        'slot-destination-dedup': (E, 'if !self.consumed.contains(&source) {', 'if self.consumed.iter().all(|entry| entry.destination != source.destination) {', 'project', P+'w07_repeated_source_attempts_do_not_deduplicate_by_destination'),
        'slot-reservation': (E, 'let needed = usize::from(!self.consumed.contains(source)) + 1;', 'let needed = usize::from(!self.consumed.contains(source));', 'project', P+'w07_repeated_source_attempts_do_not_deduplicate_by_destination'),
        'slot-refund': (E, 'self.reserved = false;\n        drop(reservation);', 'self.reserved = false; self.consumed.clear();\n        drop(reservation);', 'project', P+'w07_mutation_boundary_and_four_attempt_slots'),
        'slot-success-counted': (E, 'if let Some(tag) = unknown {', 'if let Some(tag) = unknown.or_else(|| Some(reservation.source.clone())) {', 'project', P+'w07_mutation_boundary_and_four_attempt_slots'),
        'ordinary-save-slot-gate': (E, 'pub fn save_active(&mut self) -> Result<BundleSaveReport, BundleError> {', 'pub fn save_active(&mut self) -> Result<BundleSaveReport, BundleError> {\n        if self.rescue_slots() >= 4 { return Err(BundleError::Capacity("rescue slots")); }', 'project', P+'w07_mutation_boundary_and_four_attempt_slots'),
        'retained-charge': (S, 'let hold = self.begin_copy_source(&roots, bytes, true)?;', 'let mut hold = self.begin_copy_source(&roots, bytes, true)?; drop(hold.overlap.take());', 'project', P+'w03_changed_capture_invalidates_consent'),
        'retained-pin': (S, 'let overlap = self.reserve_copy_read(bytes, 0, 0, true, cleanup_only)?;\n        let pin = self.pin(roots)?;', 'let overlap = self.reserve_copy_read(bytes, 0, 0, true, cleanup_only)?;\n        let pin = self.pin(&[])?;', 'project', P+'w03_changed_capture_invalidates_consent'),
        'early-owner-retirement': (E, 'if !self.rescue.attempts.is_empty()\n', 'if false && !self.rescue.attempts.is_empty()\n', 'project', P+'w01_retained_source_and_a_writer_control'),
        'ordinary-retirement-omitted': (E, 'self.source = None;', 'let _ = &self.source;', 'project', P+'w01_ordinary_handoff_retires_only_unadopted_source'),
        'writer-reacquire': (B, 'let result = destination.save(core, inputs);', 'let path = destination.store.resolved_root()?; let _rival = ProjectBundle::activate(&path, BundleWriteAdmission::read(&path)?)?;\n        let result = destination.save(core, inputs);', 'project', P+'w16_first_save_a_writer_and_later_floor_continue'),
        'older-root-writer-reacquire': (B, 'let result = destination.save(core, inputs);', 'let path = destination.store.resolved_root()?; let _rival = ProjectBundle::activate(&path, BundleWriteAdmission::read(&path)?)?;\n        let result = destination.save(core, inputs);', 'project', P+'w16_first_save_older_root_writer_reuses_retained_namespace'),
        'continuity-refusal-ignored': (E, 'Self::FirstSave(copy) => copy.reconcile_retained(core),', 'Self::FirstSave(copy) => copy.reconcile_retained(core).or(Ok(None)),', 'project', P+'w09_lifecycle_continuity_refuses_changed_parent_and_foreign_root'),
        'stuck-live-ticket': (H, 'settle(core, &attempt.ticket, commit).map_err(crate::bundle::history_error)?;', 'let _ = commit;', 'project', P+'w16_first_save_a_writer_and_later_floor_continue'),
        'no-ticket-history-restore': ('crates/kinewright-project/src/receipts.rs', '        self.reconcile_destination(Some(core))?;\n', '        self.reconcile_destination(Some(core))?;\n        let checkpoint = self.branches.get(&self.manual_branch_id).unwrap();\n        let history = self.store.read_record::<HistorySnapshot>(&checkpoint.history_ref)?;\n        self.attach_core(core)?.restore_history((*history).clone()).map_err(crate::bundle::history_error)?;\n', 'project', P+'w10_lifecycle_no_ticket_destination_settlement_keeps_live_history'),
        'no-ticket-source-owner': (E, 'let ticket = current.pending_attempt();', 'let ticket = Some(current.pending_attempt().ok_or(BundleError::HistoryBarrierInvariant)?);', 'project', P+'w07_no_ticket_attempt_keeps_actual_namespace_owner'),
        'owner-after-settlement': (E, 'if !self.rescue.attempts.is_empty()\n', 'if false && !self.rescue.attempts.is_empty()\n', 'project', P+'w08_actual_owners_survive_drop_settlement_and_reader_retirement'),
        'preexisting-freeze-cancel': (E, 'if !capture.was_frozen {', 'if true {', 'project', P+'w03_probe_consent_and_premutation_cancel'),
        'stale-offer-tag': (E, '|| second.tag != offer.tag', '|| false && second.tag != offer.tag', 'project', P+'w03_changed_capture_invalidates_consent'),
        'second-probe-omitted': (E, 'let Some(second) = self.probe_active()? else {\n            return Ok(None);\n        };', 'let second = RescueProbe { tag: offer.tag.clone(), ticket: offer.ticket.clone(), phase: offer.phase, cause: offer.cause, eligibility: offer.eligibility, operation: offer.operation, error: BundleError::Unsettled };', 'project', P+'w03_probe_consent_and_premutation_cancel'),
        'duplicate-attempt-owner': (E, 'let reservation = self.rescue.slots.begin(tag.clone())?;\n        if !retained {', 'let reservation = self.rescue.slots.begin(tag.clone())?;\n        if true {', 'project', P+'w07_no_ticket_attempt_keeps_actual_namespace_owner'),
        'attempt-status-not-settled': (E, 'attempt.state = state;', 'let _ = state;', 'project', P+'w07_repeated_source_attempts_do_not_deduplicate_by_destination'),
        'native-cleanup-phase': (N, 'crate::bundle_rescue::ReconcilePhase::Cleanup,', 'crate::bundle_rescue::ReconcilePhase::Barrier,', 'project', P+'w03_native_probe_causes_keep_phase_operation_and_errno'),
        'native-authority-phase': (E, 'phase = ReconcilePhase::Authority;', 'phase = ReconcilePhase::Namespace;', 'project', P+'w03_native_probe_causes_keep_phase_operation_and_errno'),
        'native-error-erased': ('crates/kinewright-project/src/namespace/mod.rs', 'original.unwrap_or(error)', 'error', 'project', P+'w03_native_probe_causes_keep_phase_operation_and_errno'),
        'native-ticket-proof-erased': ('crates/kinewright-project/src/bundle.rs', 'if continuous && matches!(error, BundleError::Io(_)) {', 'if false && continuous && matches!(error, BundleError::Io(_)) {', 'project', P+'w03_native_probe_causes_keep_phase_operation_and_errno'),
        'native-retained-route': (B, 'if !retained && let Some(tag) = &self.tag {', 'if let Some(tag) = &self.tag {', 'project', P+'w03_native_probe_causes_keep_phase_operation_and_errno'),
        'native-stale-error': (E, 'n.clear_probe_failure();', 'let _ = &n;', 'project', P+'w03_native_probe_causes_keep_phase_operation_and_errno'),
        'native-recorded-phase-ignored': (E, 'observed.map_or(phase, |(phase, _)| phase),', 'phase,', 'project', P+'w03_native_probe_causes_keep_phase_operation_and_errno'),
        'native-error-source-chain': ('crates/kinewright-project/src/bundle.rs', 'Self::Io(error) => Some(error),', 'Self::Io(_) => None,', 'project', P+'w03_native_probe_causes_keep_phase_operation_and_errno'),
        'reader-lease-owner-omitted': (S, 'Ok(RetainedReadLease {\n            file,\n            _hold: Arc::clone(&self.hold),\n            _charge: charge,\n        })', 'Ok(file)', 'project', P+'w08_actual_owners_survive_drop_settlement_and_reader_retirement'),
        'source-fixture-owner': (E, 'if !self.rescue.attempts.is_empty()\n', 'if false && !self.rescue.attempts.is_empty()\n', 'agent', 's2_additive_source_fixture_observes_actual_lifecycle_owners'),
        'slots-fixture-ordinary-save': (E, 'pub fn save_active(&mut self) -> Result<BundleSaveReport, BundleError> {', 'pub fn save_active(&mut self) -> Result<BundleSaveReport, BundleError> {\n        if self.rescue_slots() >= 4 { return Err(BundleError::Capacity("rescue slots")); }', 'agent', 's2_additive_slots_fixture_observes_ordinary_save_at_four_slots'),
    }

def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def main():
    evidence = Path(sys.argv[1]).resolve()
    evidence.mkdir(parents=True, exist_ok=True)
    selected = sys.argv[2:] or list(cases())
    identity = ROOT / 'scripts/ja1-s2-c1-evidence.py'
    subprocess.run([sys.executable, '-I', str(identity), 'identity', str(evidence/'source-before.json')], cwd=ROOT, check=True)
    for name in selected:
        file, old, new, target, witness = cases()[name]
        path = ROOT / file
        original = path.read_bytes()
        text = original.decode()
        if text.count(old) != 1:
            raise SystemExit(f'{name}: expected one replacement, got {text.count(old)}')
        directory = evidence / name
        directory.mkdir()
        (directory / 'original.rs').write_bytes(original)
        mutated = text.replace(old, new).encode()
        (directory / 'identity.json').write_text(json.dumps({'source_file': file, 'original_sha256': sha(original), 'mutated_sha256': sha(mutated), 'witness': witness}, indent=2)+'\n')
        args = ['test', '-p', 'kinewright-'+target]
        if target in ('core', 'project-lib'):
            args[2] = 'kinewright-project' if target == 'project-lib' else 'kinewright-core'
            args += ['--features', 'test-util', '--lib'] if target == 'project-lib' else ['--lib']
        else:
            args += ['--features', 'eval-harness' if target == 'agent' else 'test-util', '--test', 'ja_s1' if target == 'agent' else 'ja_s2_rescue']
        args += [witness, '--', '--exact', '--nocapture', '--test-threads=4']
        try:
            for phase in ['red', 'green']:
                path.write_bytes(mutated if phase == 'red' else original)
                started = time.monotonic()
                while int(next(line.split()[1] for line in Path('/proc/meminfo').read_text().splitlines() if line.startswith('MemAvailable:'))) < 6291456:
                    if time.monotonic()-started > 600:
                        raise SystemExit('6 GiB memory prerequisite unavailable')
                    time.sleep(5)
                command = ['nice', '-n', '10', 'cargo', *args]
                (directory / (phase+'.command.json')).write_text(json.dumps(command)+'\n')
                with (directory / (phase+'.log')).open('w') as log:
                    result = subprocess.run(command, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, check=False)
                output = (directory / (phase+'.log')).read_text()
                summary = re.findall(r'^test result:.*$', output, re.M)
                expect = r'^test result: FAILED\. 0 passed; 1 failed; 0 ignored;' if phase == 'red' else r'^test result: ok\. 1 passed; 0 failed; 0 ignored;'
                if result.returncode != (101 if phase == 'red' else 0) or not re.search(expect, output, re.M):
                    print(output[-9000:])
                    raise SystemExit(f'{name}/{phase}: missing required single runtime outcome (exit {result.returncode})')
                executable = re.findall(r'^\s*Running .* \(([^()]+)\)$', output, re.M)
                binary = (ROOT/executable[0]).resolve()
                with binary.open('rb') as stream:
                    digest = hashlib.file_digest(stream, 'sha256').hexdigest()
                (directory/(phase+'.result.json')).write_text(json.dumps({'exit':result.returncode, 'raw_summary':summary, 'binary':str(binary), 'binary_sha256':digest},indent=2)+'\n')
                print(f'{name}/{phase}: {summary[0]}', flush=True)
        finally:
            path.write_bytes(original)
        if path.read_bytes() != original:
            raise SystemExit('source restoration failed')

    subprocess.run([sys.executable, '-I', str(identity), 'identity', str(evidence/'source-after.json')], cwd=ROOT, check=True)
    if (evidence/'source-before.json').read_bytes() != (evidence/'source-after.json').read_bytes():
        raise SystemExit('complete source identity changed across mutations')

if __name__ == '__main__':
    main()
