#!/usr/bin/env python3
"""C4 targeted runtime red/restored-green pairs; compilation errors never earn a red."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
ROOT = Path(__file__).resolve().parent.parent
P = 'crates/kinewright-project/src/'
S = P+'bundle_switch.rs'
C = P+'bundle_commit.rs'
R = P+'rescue_record.rs'
G = P+'bundle_recovery.rs'
V = P+'bundle_recovery/restore.rs'
L = 'durable::w13_local_selection_mapping_limits_and_retry'
I = 'durable_branch::tests::w13_shared_prefix_occurrence_identity_survives_branch_saves'
A = 'durable::w13_ack_and_two_predecessors_reopen_rotate_and_recover'
W = 'durable::w14_raw_role_schema_api_and_checkpoint_edges_are_required'
Q = 'durable::w13_projection_preserves_every_non_linkage_payload'

def cases():
    return {
        'fresh-open-guard-omitted': (P+'store.rs', 'crate::bundle_recovery::validate_publication_reopen_fit(\n                n,\n                bytes,\n                self.limits(),\n                &mut *self\n                    .inner\n                    .verified_memo\n                    .lock()\n                    .map_err(|_| BundleError::Capacity("memo poisoned"))?,\n            )', 'Ok(())', 'durable::w13_selection_fresh_open_exact_and_one_byte_over'),
        'committed-selection-phase-erased': (S, 'ProjectSelectionResult::CommittedWithError(error)', 'ProjectSelectionResult::Refused(error)', L),
        'frozen-owner-install-allowed': ('crates/kinewright-core/src/actor.rs', 'else if attachment == AttachmentState::Frozen {\n                    Err(HistoryError::MutationRefused(CoreMutationRefusal::Frozen))\n                } else if state.pending_floor.is_some()', 'else if state.pending_floor.is_some()', L),
        'frozen-owner-install-core-witness': ('crates/kinewright-core/src/actor.rs', 'else if attachment == AttachmentState::Frozen {\n                    Err(HistoryError::MutationRefused(CoreMutationRefusal::Frozen))\n                } else if state.pending_floor.is_some()', 'else if state.pending_floor.is_some()', 'attachment::tests::w05_seal_checks_cut_and_all_mutation_paths'),
        'portable-nonzero-switch-refused': (P+'bundle.rs', 'self.manual_switch_generation >= self.generation', '(self.manual_switch_generation >= self.generation || self.manual_switch_generation != 0)', 'w14_raw_switch_structure_and_ordinary_v3_backward_control'),
        'compaction-attachment-drops-independent-root': (R, 'self.evidence.push(reference.clone());', 'self.evidence.clear(); self.evidence.push(reference.clone());', 'durable::w11_independent_roots_survive_provenance_compaction'),
        'ack-exact-receipt-lookup-omitted': (P+'receipts.rs', 'recovery.extend(records.iter().filter_map(|r| r.acknowledged_root.clone()));', 'let _ = &records;', 'durable::w10_pending_command_exclusion_and_original_id_lookup'),
        'existing-target-checkpoint-replaced': (C, 'let existing_selection =', 'let existing_selection = false &&', 's2_additive_recovery_fixture_observes_local_selection'),
        's0-recovery-pointer-fixture': (C, 'bundle.manual_branch_id = manual_branch_id.clone();', 'let _ = &manual_branch_id;', 's2_additive_recovery_fixture_observes_local_selection'),
        'selected-pin-omitted': (S, 'let pin = self.store.pin(std::slice::from_ref(&reference))?;', 'let pin = self.store.pin(&[])?;', L),
        'selected-pin-released-before-activation': (S, 'prepared.source_head = self.manual_head().cloned().ok_or(BundleError::Unsettled)?;', 'prepared.source_head = self.manual_head().cloned().ok_or(BundleError::Unsettled)?; prepared.pin.take();', L),
        'resolve-by-automatic-list-position': (S, 'let restore = self.prepare_checkpoint_restore(&target)?;', 'let wrong = match &target { RecoverySelection::Checkpoint { branch, .. } => RecoverySelection::Checkpoint { root: self.recovery.predecessors[0].clone(), branch: branch.clone() }, other => other.clone() }; let restore = self.prepare_checkpoint_restore(&wrong)?;', L),
        'require-continued-automatic-membership': (S, '// Verify the original identity even after its automatic slot has rotated away.', 'if !self.recovery.references().contains(&target_ref) { owner.resume(&cut).map_err(crate::bundle::history_error)?; return Err(BundleError::Unsettled); }\n        // Verify the original identity even after its automatic slot has rotated away.', L),
        'restored-history-truncated': (V, '.restore_history((*history).clone())', '.restore_history(kinewright_core::HistoryTransition::floor((*history).clone(), 1).map_err(crate::bundle::history_error)?.after().clone())', L),
        'shared-prefix-batches-reminted': (C, 'let all_batch_ids = batch_ids.clone();', 'for id in &mut batch_ids { *id = BatchId::fresh()?; } let all_batch_ids = batch_ids.clone();', I),
        'occurrences-restamped': (C, 'let origin = origins.get(id).unwrap_or(branch);', 'let _ = &origins; let origin = branch;', I),
        'floor-forgets-shared-origins': (C, 'old_lineage.as_deref().or(seed),', 'None,', I),
        'definite-pointer-not-refreshed': (C, 'bundle.manual_branch_id = manual_branch_id.clone();', 'let _ = &manual_branch_id;', L),
        'reconciled-pointer-not-refreshed': (P+'bundle.rs', 'self.manual_branch_id = root.manual_branch_id.clone();', 'let _ = &root.manual_branch_id;', L),
        'later-save-resets-switch-generation': (C, 'manual_switch_generation: switch_generation,', 'manual_switch_generation: if selection.is_none() { 0 } else { switch_generation },', L),
        'retry-allocates-new-target': (S, 'if let Some(hash) = &prepared.published_root {', 'if let Some(hash) = &prepared.published_root { prepared.target_branch = BranchId::fresh()?;', L),
        'frozen-direct-save-allowed': (P+'bundle.rs', 'kinewright_core::AttachmentState::Frozen => {\n            return Err(history_error(\n                kinewright_core::HistoryError::MutationRefused(\n                    kinewright_core::CoreMutationRefusal::Frozen,\n                ),\n            ));\n        }', 'kinewright_core::AttachmentState::Frozen => {}', L),
        'pin-admission-charge-omitted': (S, '(closure.len() as u64)\n                .checked_mul(128)', '0_u64\n                .checked_mul(128)', 'durable::w13_selection_pin_capacity_refuses_before_publication'),
        'raw-switch-requirement-omitted': (R, 'apis.insert("manual-switch".into(), 1);', 'let _ = &mut apis;', L),
        'supported-switch-unconditionally-refused': (P+'bundle.rs', 'self.manual_switch_generation >= self.generation', '(self.manual_switch_generation >= self.generation || self.manual_switch_generation != 0)', L),
        'record-ack-edge-omitted': (R, 'self.acknowledged_root.iter().cloned().collect()', 'vec![]', A),
        'record-digest-only-ack': (R, 'self.acknowledged_root.iter().cloned().collect()', 'vec![]', W),
        'two-predecessor-validator-weakened': (P+'store.rs', 'self.predecessors.len() > 2', 'self.predecessors.len() > 3', W),
        'record-role-admission-omitted': (R, 'if role.map(String::as_str) != record.map(|r| r.role.as_str()) {', 'if false && role.map(String::as_str) != record.map(|r| r.role.as_str()) {', W),
        'record-edge-admission-omitted': (R, 'if expected != actual {', 'if false && expected != actual {', W),
        'record-schema-accepted': (R, 'self.role != RESCUE_ROLE || self.schema != 1', 'self.role != RESCUE_ROLE', W),
        'record-omitted-before-publication': (R, 'self.evidence.push(reference.clone());', 'let _ = &reference;', W),
        'provenance-count-unbounded': (R, 'self.attempts.len() <= 16', 'self.attempts.len() <= 100', 'w11_warning_provenance_compaction_and_recopy'),
        'provenance-overflow-lost': (R, 'self.summary.count_overflow |= overflow;', 'let _ = overflow;', 'w11_warning_provenance_compaction_and_recopy'),
        'provenance-treated-as-absent': (R, 'ReceiptLookup::UnresolvedOrigin', 'ReceiptLookup::Absent', 'durable::w10_pending_command_exclusion_and_original_id_lookup'),
        'origin-digest-conflict-ignored': (R, 'if expected == digest {', 'if true || expected == digest {', 'durable::w10_pending_command_exclusion_and_original_id_lookup'),
        'l1-document-payload-change': (V, 'if before.inspection_document() != after.inspection_document()', 'if false && (before.inspection_document() != after.inspection_document()', Q),
        'l1-batch-payload-change': (V, 'if before.inspection_document() != after.inspection_document()', 'if false && (before.inspection_document() != after.inspection_document()', Q),
        'l1-input-payload-change': (V, 'if before.inspection_document() != after.inspection_document()', 'if false && (before.inspection_document() != after.inspection_document()', Q),
        'independent-projection-root-dropped': (V, 'let mut pins = manifest.durable_pins.clone();', 'let mut pins = manifest.durable_pins.iter().filter(|r| manifest.evidence_roles.contains_key(&r.sha256)).cloned().collect::<Vec<_>>();', Q),
        'current-provenance-summarized': (R, 'self.validate(&StorageLimits::default())?;\n        for attempt in inherited {', 'self.attempts.clear(); self.validate(&StorageLimits::default())?;\n        for attempt in inherited {', 'w11_warning_provenance_compaction_and_recopy'),
        'current-omission-dropped': (R, 'self.validate(&StorageLimits::default())?;\n        for attempt in inherited {', 'self.omissions.clear(); self.validate(&StorageLimits::default())?;\n        for attempt in inherited {', 'w11_warning_provenance_compaction_and_recopy'),
        'oversized-raw-role-hidden': (G, 'if serde_json::from_slice::<RoleProbe>(&bytes).is_ok_and(|probe| probe.role) {', 'if false && serde_json::from_slice::<RoleProbe>(&bytes).is_ok_and(|probe| probe.role) {', 'durable::w14_oversized_unclaimed_raw_role_refuses'),
        'archived-raw-switch-requirement-skipped': (G, 'crate::rescue_record::required_apis(root.manual_switch_generation, false)', 'crate::rescue_record::required_apis(0, false)', L),
        'missing-raw-requirement-accepted': (R, '|| required.iter().any(|(k, v)| declared.get(k) != Some(v))', '|| false && required.iter().any(|(k, v)| declared.get(k) != Some(v))', L),
        'rack-counted-as-third-predecessor': (C, 'recovery.predecessors.truncate(2);', 'recovery.predecessors.truncate(2); if recovery.predecessors.len() == 2 && !bundle.rescue_records()?.is_empty() { return Err(BundleError::Capacity("third recovery root")); }', A),
        'selected-predecessor-dropped': (P+'store.rs', 'let mut all = roots.to_vec();', 'let mut recovery = recovery; recovery.predecessors.truncate(1); let mut all = roots.to_vec();', A),
        'b-activation-rewrites-origin-inputs': (C, 'let historical_input_owner = switch_generation != 0', 'let historical_input_owner = false && switch_generation != 0', 'durable::w13_checkpoint_restore_uses_b_identity_without_rewriting_inputs'),
    }


def main():
    evidence = Path(sys.argv[1]).resolve(); evidence.mkdir(parents=True, exist_ok=True)
    selected = sys.argv[2:] or list(cases())
    identity = ROOT/'scripts/ja1-s2-c1-evidence.py'
    subprocess.run([sys.executable, '-I', str(identity), 'identity', str(evidence/'source-before.json')], cwd=ROOT, check=True)
    for name in selected:
        file, old, new, witness = cases()[name]
        path = ROOT/file; original = path.read_bytes(); source = original.decode()
        if source.count(old) != 1: raise SystemExit(f'{name}: expected one replacement, got {source.count(old)}')
        # Disable both exact payload predicates together for the preservation-oracle mutation.
        mutated = source.replace(old, new)
        if name.startswith('l1-'):
            mutated = mutated.replace('|| left.issuer_state_ref != right.issuer_state_ref\n    {', '|| left.issuer_state_ref != right.issuer_state_ref)\n    {')
        directory = evidence/name; directory.mkdir()
        (directory/'identity.json').write_text(json.dumps({'source_file':file,'original_sha256':hashlib.sha256(original).hexdigest(),'mutated_sha256':hashlib.sha256(mutated.encode()).hexdigest(),'witness':witness},indent=2)+'\n')
        agent = witness == I
        fixture = witness.startswith('s2_additive_')
        core = witness.startswith('attachment::')
        package = 'kinewright-core' if core else ('kinewright-agent' if agent or fixture else 'kinewright-project')
        args = ['test','-p',package]
        if not core: args += ['--features','eval-harness' if agent or fixture else 'test-util']
        args += ['--lib'] if agent or core else ['--test','ja_s1' if fixture else 'ja_s2_rescue_record']
        args += [witness,'--','--exact','--nocapture','--test-threads=4']
        try:
            for phase in ['red','green']:
                path.write_bytes(mutated.encode() if phase == 'red' else original)
                waited = time.monotonic()
                while int(next(line.split()[1] for line in Path('/proc/meminfo').read_text().splitlines() if line.startswith('MemAvailable:'))) < 6291456:
                    if time.monotonic()-waited > 600: raise SystemExit('6 GiB memory prerequisite unavailable')
                    time.sleep(5)
                command = ['nice','-n','10','cargo',*args]
                (directory/(phase+'.command.json')).write_text(json.dumps(command)+'\n')
                with (directory/(phase+'.log')).open('w') as log:
                    environment = os.environ.copy()
                    if name.startswith('l1-'): environment['JA_C4_L1_CASE'] = name.split('-')[1]
                    result = subprocess.run(command,cwd=ROOT,env=environment,stdout=log,stderr=subprocess.STDOUT,check=False,timeout=600)
                output = (directory/(phase+'.log')).read_text()
                summaries = re.findall(r'^test result:.*$',output,re.M)
                expected = r'^test result: FAILED\. 0 passed; 1 failed; 0 ignored;' if phase=='red' else r'^test result: ok\. 1 passed; 0 failed; 0 ignored;'
                if result.returncode != (101 if phase=='red' else 0) or not re.search(expected,output,re.M):
                    print(output[-8000:]); raise SystemExit(f'{name}/{phase}: no single runtime outcome (exit {result.returncode})')
                binary_name = re.findall(r'^\s*Running .* \(([^()]+)\)$',output,re.M)[0]
                binary = (ROOT/binary_name).resolve()
                with binary.open('rb') as stream: digest = hashlib.file_digest(stream,'sha256').hexdigest()
                (directory/(phase+'.result.json')).write_text(json.dumps({'exit':result.returncode,'raw_summary':summaries,'binary':str(binary),'binary_sha256':digest},indent=2)+'\n')
                print(f'{name}/{phase}: {summaries[0]}',flush=True)
        finally: path.write_bytes(original)
    subprocess.run([sys.executable,'-I',str(identity),'identity',str(evidence/'source-after.json')],cwd=ROOT,check=True)
    if (evidence/'source-before.json').read_bytes() != (evidence/'source-after.json').read_bytes(): raise SystemExit('source changed across mutation run')
if __name__ == '__main__': main()
