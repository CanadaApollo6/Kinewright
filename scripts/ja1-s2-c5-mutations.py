#!/usr/bin/env python3
"""C5 exact runtime red/restored-green pairs. Build failures are never mutation evidence."""
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
E = P + 'editing_session/'
D = 'c5::w13_checkpoint_recovery_preserves_dirty_b_before_activation'
F = 'c5::w13_frozen_cut_rescue_at_both_unknown_publications_and_late_a_isolation'
T = 'c5::w13_one_level_retention_and_explicit_retirement'
X = 'c5::w13_prepared_existing_target_is_never_a_retirement_offer'
H = 'c5::w13_existing_target_retry_requires_current_membership_and_head'
Q = 'c5_pressure::w13_definite_rescue_sparse_cache_growing_saves_and_same_pin_quota_retry'
G = 'durable_branch::tests::w13_registered_session_blocks_branch_retirement'
O = 'c5::w13_omission_only_precedes_trim'
C = 'c5::w13_trim_then_separate_omission_recomputes_closure'
R = 'c5::w13_one_level_rescue_recopy_and_explicit_ack_retirement'


def section(file, start, end):
    source = (ROOT / file).read_text()
    first = source.index(start)
    return source[first:source.index(end, first) + len(end)]


def cases():
    cases = {}
    def add(name, edits, witness, target='ja_s2_rescue_record', package='kinewright-project', diagnostic=None):
        cases[name] = (edits, witness, target, package, diagnostic)
    def one(name, file, old, new, witness, target='ja_s2_rescue_record', package='kinewright-project'):
        add(name, [(file, old, new)], witness, target, package)
    guard = P + 'bundle_recovery/publication.rs'
    target_only = [(guard, '        let _ = id;', '        let active = active && *id == root.manual_branch_id;')]
    add('target-only-ordinary-publisher', target_only, 'linux::w13_every_publication_fits_fresh_open', 'ja_s2_save', diagnostic='live history bytes')
    add('target-only-c5-sparse-cache', target_only, Q, diagnostic='published candidate cannot exceed independent fresh-open capacity')
    one('select-before-definite-ordinary-save', E+'recovery.rs', 'let save = if self.dirty()? {', 'let save = if false && self.dirty()? {', D)
    # A general Save floor refusal rejects a real legal ordinary floor, before selection.
    one('refuse-legal-ordinary-floor', P+'bundle.rs', 'let bounded = crate::bundle_commit::select_history(&aligned, self.store.limits())?;', 'let bounded = crate::bundle_commit::select_history(&aligned, self.store.limits())?; if bounded.floor.is_some() { return Err(BundleError::Capacity("mutant refuses legal floor")); }', 'c5::w13_preliminary_save_uses_ordinary_floor_with_redo_or_gesture')
    one('bypass-ordinary-select-history', P+'bundle.rs', 'let bounded = crate::bundle_commit::select_history(&aligned, self.store.limits())?;', 'let bounded = crate::bundle_commit::BoundedHistory { history: aligned.clone(), floor: None };', 'c5::w13_preliminary_save_uses_ordinary_floor_with_redo_or_gesture')
    one('unfreeze-selection-unknown', P+'bundle_switch.rs', '            Err(_) if self.unsettled || self.publication_state == PublicationState::Unknown => {\n                ProjectSelectionResult::Unknown', '            Err(_) if self.unsettled || self.publication_state == PublicationState::Unknown => {\n                let owner = self.attach_core(core).unwrap(); let cut = owner.capture().unwrap(); owner.resume(&cut).unwrap(); ProjectSelectionResult::Unknown', F)
    one('drop-l-on-later-r-save', P+'bundle_commit.rs', '        branch_index.retain(|id, _| branches.contains_key(id));', '        if old_switch == 1 && selection.is_none() { branches.retain(|id, _| id == branch_id); } branch_index.retain(|id, _| branches.contains_key(id));', D)
    one('drop-r-on-later-l-save', P+'bundle_commit.rs', '        branch_index.retain(|id, _| branches.contains_key(id));', '        if old_switch >= 2 && selection.is_none() { branches.retain(|id, _| id == branch_id); } branch_index.retain(|id, _| branches.contains_key(id));', D)
    one('remint-shared-prefix-on-first-r-save', P+'bundle_commit.rs', '        let all_batch_ids = batch_ids.clone();', '        if old_switch == 1 && selection.is_none() { for id in &mut batch_ids { *id = BatchId::fresh()?; } } let all_batch_ids = batch_ids.clone();', D)
    one('omit-quota-retirement-action', E+'recovery.rs', '            .filter(|(_, _, blockers)| blockers.is_empty())', '            .filter(|(_, _, _)| false)', G, '--lib', 'kinewright-agent')
    add('release-target-pin-before-quota-retry', [
        (P+'bundle_switch.rs', '    pin: Option<StorePin>,', '    pub(crate) pin: Option<StorePin>,'),
        (E+'recovery.rs', '_ => self.prepared = Some(prepared),', '_ => { prepared.pin.take(); self.prepared = Some(prepared); },')], Q)
    old = section(E+'recovery.rs', '        if self\n            .prepared\n', '            blockers.push("prepared recovery selection target".into());\n        }')
    one('offer-prepared-target-for-retirement', E+'recovery.rs', old, '', X)
    one('remove-prepared-target-directly', E+'recovery.rs', '        let blockers = self.branch_blockers(branch);', '        let mut blockers = self.branch_blockers(branch); blockers.retain(|b| b != "prepared recovery selection target");', X)
    existing = section(P+'bundle_switch.rs', '            && prepared.existing_head.as_ref().is_none_or', '            });')
    one('existing-target-head-not-revalidated', P+'bundle_switch.rs', existing, '            && prepared.existing_head.as_ref().is_none_or(|_| self.branches.contains_key(&prepared.target_branch));', H)
    # Reinsert from the verified pin, rather than requiring current membership. The resulting graph is valid.
    add('resurrect-missing-existing-target', [
        (P+'bundle_switch.rs', existing, '            && true;'),
        (P+'bundle_switch.rs', '        let target_ref = match &prepared.target {\n            RecoverySelection::WorkingBranch(id) => self.checkpoint_reference(id)?,', '        let target_ref = match &prepared.target {\n            RecoverySelection::WorkingBranch(id) => { if !self.branches.contains_key(id) { self.branches.insert(id.clone(), prepared.restore.checkpoint.clone()); let sealed = self.store.admit_record(&prepared.restore.checkpoint)?; let mut index = (*self.store.read_record::<std::collections::BTreeMap<BranchId, crate::bundle_records::ObjectRef>>(&self.root.as_ref().unwrap().branch_index_ref)?).clone(); index.insert(id.clone(), sealed.reference.clone()); self.root.as_mut().unwrap().branch_index_ref = self.store.admit_record(&index)?.reference.clone(); } self.store.admit_record(&prepared.restore.checkpoint)?.reference.clone() },'),
    ], H)
    registration = '        self.durable_sessions.push(RegisteredSession {'
    for route in ['fork_durable_branch', 'reopen_durable_branch']:
        start = section(E+'durable.rs', '    pub fn '+route+'(', '\n        Ok(guard)\n    }')
        replacement = start.replace(registration, '        drop(RegisteredSession {')
        one('unregistered-'+route, E+'durable.rs', start, replacement, G, '--lib', 'kinewright-agent')
    one('ignore-live-session-blocker', E+'recovery.rs', '(&session.guard.branch == id || &session.guard.base.target_branch == id)', '(&session.guard.base.target_branch == id)', G, '--lib', 'kinewright-agent')
    one('ignore-required-specialist-base', E+'recovery.rs', '(&session.guard.branch == id || &session.guard.base.target_branch == id)', '(&session.guard.branch == id)', G, '--lib', 'kinewright-agent')
    add('retire-active-working-branch', [
        (E+'recovery.rs', '        let mut blockers = vec![];', '        let mut blockers = vec![]; if self.active_bundle().is_ok_and(|b| b.manual_branch_id() == id) { return blockers; }'),
        (E+'recovery.rs', '            .ok_or(BundleError::HistoryChanged)?;\n        let result = BundleCommit::publish_maintenance(bundle);', '            .ok_or(BundleError::HistoryChanged)?;\n        if &bundle.manual_branch_id == branch { bundle.manual_branch_id = bundle.branches.keys().next().unwrap().clone(); } let result = BundleCommit::publish_maintenance(bundle);')], T)
    one('retirement-without-explicit-consent', E+'recovery.rs', '        if !confirmed {', '        if false && !confirmed {', T)
    one('duplicate-retirement-acknowledged', E+'recovery.rs', '.ok_or(BundleError::HistoryChanged)?;\n        let result = BundleCommit::publish_maintenance(bundle);', '.unwrap_or_else(|| bundle.branches.values().next().unwrap().clone());\n        let result = BundleCommit::publish_maintenance(bundle);', T)
    one('resume-after-frozen-cancel', P+'editing_session.rs', '        if !capture.was_frozen {', '        if true {', F)
    one('resume-after-frozen-preview-refusal', E+'rescue.rs', 'if result.is_err() && !was_frozen {', 'if result.is_err() {', F)
    one('resume-after-frozen-publication-failure', E+'rescue.rs', '                if !was_frozen {', '                if true {', F)
    one('resume-after-frozen-unknown-b', E+'rescue.rs', '\n        if !was_frozen {', '\n        if true {', F)
    one('late-a-seals-b', E+'rescue.rs', '        let state = owner.bundle()?.publication_state();', '        let state = owner.bundle()?.publication_state(); if let Ok(owner) = ProjectAttachment::new(&self.core) { if let Ok(cut) = owner.freeze() { let _ = owner.seal(&cut); } }', F)
    one('rescue-shares-original-core', E+'rescue.rs', 'let b_core = Core::spawn((*selection.history.baseline).clone())\n            .map_err(|e| BundleError::Corrupt(e.to_string()))?;', 'let b_core = self.core.clone();', 'c5::w01_first_save_rescue_is_independent', 'ja_s2_rescue')
    one('rescue-record-not-published', P+'bundle_copy.rs', '        target.retain_rescue_record(&record)?;', '        let _ = &record;', R, 'ja_s2_rescue')
    one('rescue-drops-default-ack', P+'bundle_copy.rs', '                record.acknowledged_root = Some(projected);', '                let _ = projected; record.acknowledged_root = None; record.acknowledged_absence = Some("mutant dropped ack".into());', R, 'ja_s2_rescue')
    one('rescue-drops-selected-p2', P+'bundle_copy.rs', '                .map(|(_, r)| r.clone())\n                .collect();', '                .take(1).map(|(_, r)| r.clone())\n                .collect();', R, 'ja_s2_rescue')
    one('rotation-forgets-ack-record', P+'bundle_commit.rs', '        let mut evidence = bundle.evidence.clone();', '        let mut evidence = bundle.evidence.iter().filter(|r| crate::rescue_record::parse(&bundle.store.read_evidence(r).unwrap()).unwrap().is_none()).cloned().collect::<Vec<_>>();', R, 'ja_s2_rescue')
    one('ack-retirement-clears-independent-evidence', E+'recovery.rs', '            bundle.evidence.retain(|r| r != &old);', '            let _ = &old; bundle.evidence.clear();', R, 'ja_s2_rescue')
    one('silent-recovery-omission', P+'bundle_rescue.rs', '|| (!self.omissions.is_empty() && !consent.omission)', '|| false && (!self.omissions.is_empty() && !consent.omission)', O, 'ja_s2_rescue')
    one('one-consent-authorizes-both-losses', P+'bundle_rescue.rs', '        consent.trim = true;', '        consent.trim = true; consent.omission = true;', C, 'ja_s2_rescue')
    one('silent-history-trim', P+'bundle_rescue.rs', '(self.removed_undo > 0 && !consent.trim)', '(false && self.removed_undo > 0 && !consent.trim)', 'c5::w04_oldest_first_trim_preserves_redo_gesture_and_inputs', 'ja_s2_rescue')
    add('skip-omission-alone-before-trim', [
        (E+'rescue_selection.rs', '        } else if let Ok(roots) = self.rescue_graph(before, &unavailable, &limits) {', '        } else if let Ok(roots) = self.rescue_graph(before, &[], &limits) {'),
        (E+'rescue_selection.rs', '            let ordinary = crate::bundle_commit::select_history(before, &limits)?;', '            let mut ordinary = crate::bundle_commit::select_history(before, &limits)?; ordinary.history = kinewright_core::HistoryTransition::floor(before.clone(), 1).map_err(crate::bundle::history_error)?.after().clone();')], O, 'ja_s2_rescue')
    one('omit-available-roots-for-quota', E+'rescue_selection.rs', 'if !missing.entries.is_empty() || !missing.dependencies_complete {', 'if true {', 'c5::w13_available_recovery_roots_cannot_be_omitted_for_quota', 'ja_s2_rescue')
    one('omitted-inventory-reintroduced', P+'store.rs', '        self.copy_selected_graph(source, roots, true, false)', '        self.copy_selected_graph(source, roots, true, true)', O, 'ja_s2_rescue')
    required = section(E+'rescue_selection.rs', '        for reference in &bundle.evidence {', '            roots.push(reference.clone());\n        }')
    one('prune-independent-root-for-rescue', E+'rescue_selection.rs', required, '', C, 'ja_s2_rescue')
    newer = section(P+'bundle_copy.rs', '        if destination.unsettled\n', '                .mark_uncertain()?;\n        }')
    one('active-b-new-unknown-uses-old-copy-status', P+'bundle_copy.rs', newer, '', 'c5::w13_recovery_on_active_rescue_copy_settles_the_new_attempt', 'ja_s2_rescue')
    one('close-leaves-uncertain-source-reuse', P+'store.rs', '    pub(crate) fn close_originating_context(&mut self) {\n        self.uncertain = false;\n    }', '    pub(crate) fn close_originating_context(&mut self) {}', 'c5::w08_context_close_fences_handles_and_retires_temporary_sources_once', 'ja_s2_rescue')
    one('additive-rescue-observation-false', 'crates/kinewright-agent/src/ja_harness/s2_observer.rs', '"rescue_publication":outcome.activated && separate && sealed', '"rescue_publication":(false && outcome.activated && separate && sealed)', 's2_additive_publication_fixture_observes_real_rescue', 'ja_s1', 'kinewright-agent')
    one('copied-record-profile-not-admitted', E+'rescue_selection.rs', '        for reference in &closure {\n            source.admit_rescue_record_profile(reference, limits)?;\n        }', '', 'c5::w13_available_recovery_roots_cannot_be_omitted_for_quota', 'ja_s2_rescue')
    bounds = section(E+'rescue_selection.rs', '        if bytes > limits.owned_bytes', '            return Err(BundleError::Capacity("rescue selected graph/overlap"));\n        }')
    one('selected-graph-bounds-not-admitted', E+'rescue_selection.rs', bounds, '', 'c5::w06_rescue_bounds_refuse_before_destination_mutation', 'ja_s2_rescue')
    one('late-a-replaces-active-core', E+'rescue.rs', '        let state = owner.bundle()?.publication_state();', '        let state = owner.bundle()?.publication_state(); self.core = self.retained_destinations[index].1.clone();', F)
    restore = '            self.pending_save = None;\n            return Ok(());\n        };'
    one('no-ticket-reconcile-restores-live-history', P+'bundle.rs', restore, '            if let Some(live) = live { if live.attachment_state().map_err(history_error)? == kinewright_core::AttachmentState::Live { self.attach_core(live)?.restore_history((*self.branch_history(&self.manual_branch_id)?).clone()).map_err(history_error)?; } }\n'+restore, 'c5::w10_pending_command_exclusion_and_original_id_lookup', 'ja_s2_rescue')
    for suffix, witness in [('a','c5::w16_first_save_a_writer_and_later_floor_continue'), ('b','c5::w16_repeated_active_b_attempts_settle_only_their_tickets')]:
        settlement = section(P+'bundle.rs', '    pub(crate) fn settle_destination_ticket(', '\n    pub(crate) fn settle_pending_save(')
        one('exact-ticket-settlement-omitted-'+suffix, P+'bundle.rs', settlement, settlement.replace('        if let Some(live) = live {','        if let Some(live) = None::<&Core> {'), witness, 'ja_s2_rescue')
    # C4's graph safety cases now run against the actual C5 rescue publisher.
    one('published-ack-has-no-dedicated-edge', P+'rescue_record.rs', 'self.acknowledged_root.iter().cloned().collect()', 'vec![]', R, 'ja_s2_rescue')
    one('published-ack-counted-as-third-predecessor', P+'bundle_commit.rs', 'recovery.predecessors.truncate(2);', 'recovery.predecessors.truncate(2); if recovery.predecessors.len() == 2 && !bundle.rescue_records()?.is_empty() { return Err(BundleError::Capacity("third recovery root")); }', R, 'ja_s2_rescue')
    one('preliminary-save-unknown-resumes-o', E+'recovery.rs', '                    self.recovery_save_pending = true;', '                    self.recovery_save_pending = true; let cut = self.attachment.capture().map_err(crate::bundle::history_error)?; self.attachment.resume(&cut).map_err(crate::bundle::history_error)?;', 'c5::w13_preliminary_save_faults_keep_exact_cut_and_do_not_select')
    one('completed-reply-allocates-another-r', E+'recovery.rs', '            && previous == &target', '            && false && previous == &target', 'c5::w13_selection_refusal_and_absence_keep_p2_for_same_target_retry')
    one('overwrite-l-on-later-r-save', P+'bundle_commit.rs', '        branch_index.retain(|id, _| branches.contains_key(id));', '        branch_index.retain(|id, _| branches.contains_key(id)); if old_switch == 1 && selection.is_none() && !metadata_only { for (id, reference) in &mut branch_index { if id != branch_id { let mut wrong = branches[branch_id].clone(); wrong.branch_id = id.clone(); wrong.head_id = HeadId::fresh()?; let mut wrong_lineage = lineage.clone(); wrong_lineage.branch_origin = id.clone(); wrong.lineage_ref = bundle.store.admit_record(&wrong_lineage)?.reference.clone(); *reference = bundle.store.admit_record(&wrong)?.reference.clone(); branches.insert(id.clone(), wrong); } } }', D)
    one('remint-shared-prefix-on-later-l-save', P+'bundle_commit.rs', '        let all_batch_ids = batch_ids.clone();', '        if old_switch >= 2 && selection.is_none() { for id in &mut batch_ids { *id = BatchId::fresh()?; } } let all_batch_ids = batch_ids.clone();', D)
    one('rescue-owned-graph-copy-omitted', P+'bundle_copy.rs', '        target.store.copy_rescue_graph(source, roots)?;', '        let _ = roots;', 'c5::w01_first_save_rescue_is_independent', 'ja_s2_rescue')
    one('rescue-rewrites-input-profile', P+'bundle_copy.rs', '        let rebound = rebind_inputs(&mut target, &selected_inputs)?;', '        for entry in &mut selected_inputs.entries { entry.profile = Some("mutated input profile".into()); } let rebound = rebind_inputs(&mut target, &selected_inputs)?;', 'c5::w01_first_save_rescue_is_independent', 'ja_s2_rescue')
    one('rescue-closes-open-gesture', E+'rescue.rs', '        let b_cut = b_attachment', '        if selection.history.cursor.undo.last().is_some_and(|e| e.coalesce_key.is_some()) { b_attachment.install_history_transition(kinewright_core::HistoryTransition::recovery_close(selection.history.clone(), &selection.history).map_err(crate::bundle::history_error)?).map_err(crate::bundle::history_error)?; } let b_cut = b_attachment', 'c5::w04_oldest_first_trim_preserves_redo_gesture_and_inputs', 'ja_s2_rescue')
    one('rescue-discards-redo', E+'rescue.rs', '        let b_cut = b_attachment', '        if !selection.history.cursor.redo.is_empty() { let track = &selection.history.document.tracks[0]; b_core.request(kinewright_core::Command::Do(kinewright_core::Operation::SetTrackSyncLock { track: track.id, locked: track.sync_lock })).map_err(|e| BundleError::Corrupt(e.to_string()))?; } let b_cut = b_attachment', 'c5::w04_oldest_first_trim_preserves_redo_gesture_and_inputs', 'ja_s2_rescue')
    one('unknown-preliminary-save-acknowledged', E+'recovery.rs', '                    self.recovery_save_pending = true;', '                    self.recovery_save_pending = true; let saved = self.history_digest()?; self.association.as_mut().ok_or(BundleError::Unsettled)?.saved = Some(saved);', F)
    activation_guard = section(P+'bundle_switch.rs', '        if self.manual_branch_id != prepared.target_branch', '            return Err(BundleError::HistoryBarrierInvariant);\n        }')
    add('activate-without-selection-root', [
        (P+'bundle_switch.rs', '    fn finish_selection(', '    pub(crate) fn finish_selection('),
        (P+'bundle_switch.rs', activation_guard, ''),
        (E+'recovery.rs', '        let core = self.core.clone();\n        match self.active_bundle_mut() {\n            Ok(bundle) => bundle.publish_project_selection(prepared, &core),', '        let core = self.core.clone();\n        match self.active_bundle_mut() {\n            Ok(bundle) => match bundle.finish_selection(prepared, &core) { Ok(report) => ProjectSelectionResult::Activated(report), Err(error) => ProjectSelectionResult::Refused(error) },')], D)
    namespace_witness = 'durable_branch::tests::w13_registered_session_keeps_original_namespace_after_rescue'
    readonly = section(E+'durable.rs', '    pub(super) fn registered_bundle(', '    fn registered_bundle_mut(').removesuffix('    fn registered_bundle_mut(')
    writable = section(E+'durable.rs', '    fn registered_bundle_mut(', '    #[must_use]\n    pub fn durable_sessions(').removesuffix('    #[must_use]\n    pub fn durable_sessions(')
    add('registered-guard-routes-to-current-project', [
        (E+'durable.rs', readonly, '    pub(super) fn registered_bundle(&self, guard: &DurableBranchSessionGuard) -> Result<&crate::bundle::ProjectBundle, BundleError> { self.registered(guard)?; self.active_bundle() }\n'),
        (E+'durable.rs', writable, '    fn registered_bundle_mut(&mut self, guard: &DurableBranchSessionGuard) -> Result<&mut crate::bundle::ProjectBundle, BundleError> { self.registered(guard)?; self.active_bundle_mut() }\n')], namespace_witness, '--lib', 'kinewright-agent')
    one('original-guard-blocks-independent-b-copy', E+'recovery.rs', '.is_ok_and(|bundle| bundle.store.owner_epoch() == session.guard.epoch)', '.is_ok_and(|_| true)', namespace_witness, '--lib', 'kinewright-agent')
    live_original = section(E+'rescue.rs', '        let live = self.core_for_destination(', '        let owner = &mut self.retained_destinations[index].0;').removesuffix('        let owner = &mut self.retained_destinations[index].0;')
    one('registered-original-ticket-not-settled', E+'rescue.rs', live_original, '        let live: Option<Core> = None;\n', namespace_witness, '--lib', 'kinewright-agent')
    prefix = section(E+'durable.rs', '            let mut full_delta = specialist_floor_delta_from_baseline(', '            full_delta.extend(cut.history().applied_operations());')
    one('registered-proposal-loses-floored-prefix', E+'durable.rs', prefix, '            let full_delta = cut.history().applied_operations();', namespace_witness, '--lib', 'kinewright-agent')
    one('specialist-base-not-in-branch-publication', E+'durable.rs', '            let reference = bundle.stage_specialist_base(guard.base())?;', '            let reference = bundle.store.admit_record(guard.base())?.reference.clone();', namespace_witness, '--lib', 'kinewright-agent')
    warning = section(E+'recovery.rs', '        for registered in &self.durable_sessions {', '        Ok(disclosure)')
    one('registered-close-warning-omitted', E+'recovery.rs', warning, '        Ok(disclosure)', namespace_witness, '--lib', 'kinewright-agent')
    one('stale-guard-replaces-canonical-state', E+'durable.rs', '        let canonical = self.durable_sessions[index].guard.clone();', '        let canonical = guard.clone();', namespace_witness, '--lib', 'kinewright-agent')
    return cases


def main():
    evidence = Path(sys.argv[1]).resolve()
    evidence.mkdir(parents=True, exist_ok=True)
    registry = cases()
    selected = sys.argv[2:] or list(registry)
    identity = ROOT / 'scripts/ja1-s2-c1-evidence.py'
    subprocess.run([sys.executable, '-I', str(identity), 'identity', str(evidence/'source-before.json')], cwd=ROOT, check=True)
    (evidence/'rustc.txt').write_bytes(subprocess.check_output(['rustc', '-Vv'], cwd=ROOT))
    (evidence/'environment.json').write_text(json.dumps({
        'cwd': str(ROOT), 'profile': 'test',
        'environment': {key: os.environ.get(key) for key in [
            'CARGO_TARGET_DIR', 'TMPDIR', 'CARGO_BUILD_JOBS', 'RUST_TEST_THREADS',
            'FFMPEG_DIR', 'RUSTFLAGS', 'LD_LIBRARY_PATH']},
        'status': subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT, text=True),
    }, indent=2)+'\n')
    for name in selected:
        edits, witness, target, package, diagnostic = registry[name]
        originals = {file: (ROOT/file).read_bytes() for file, _, _ in edits}
        mutated = {file: data.decode() for file, data in originals.items()}
        for file, old, new in edits:
            if mutated[file].count(old) != 1:
                raise SystemExit(f'{name}: {file} replacement count {mutated[file].count(old)}')
            mutated[file] = mutated[file].replace(old, new)
        directory = evidence/name
        directory.mkdir(exist_ok=True)
        (directory/'source.json').write_bytes((evidence/'source-before.json').read_bytes())
        (directory/'identity.json').write_text(json.dumps({'witness': witness, 'target': target, 'package': package,
            'edits': [{'file': file, 'old': old, 'new': new} for file, old, new in edits],
            'original_sha256': {file: hashlib.sha256(data).hexdigest() for file, data in originals.items()},
            'mutated_sha256': {file: hashlib.sha256(data.encode()).hexdigest() for file, data in mutated.items()}}, indent=2)+'\n')
        args = ['test','-p',package,'--features','eval-harness' if package=='kinewright-agent' else 'test-util']
        args += ['--lib'] if target=='--lib' else ['--test',target]
        args += [witness,'--','--exact','--nocapture','--test-threads=4']
        try:
            for phase in ['red','green']:
                for file, original in originals.items():
                    (ROOT/file).write_bytes(mutated[file].encode() if phase=='red' else original)
                deadline = time.monotonic()+600
                while int(next(line.split()[1] for line in Path('/proc/meminfo').read_text().splitlines() if line.startswith('MemAvailable:'))) < 6291456:
                    if time.monotonic() > deadline: raise SystemExit('6 GiB memory prerequisite unavailable')
                    time.sleep(5)
                command = ['nice','-n','10','cargo',*args]
                (directory/(phase+'.command.json')).write_text(json.dumps(command)+'\n')
                started_ns = time.time_ns()
                with (directory/(phase+'.log')).open('w') as log:
                    result = subprocess.run(command, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, check=False, timeout=600)
                output = (directory/(phase+'.log')).read_text()
                summaries = re.findall(r'^test result:.*$', output, re.M)
                pattern = r'^test result: FAILED\. 0 passed; 1 failed; 0 ignored;' if phase=='red' else r'^test result: ok\. 1 passed; 0 failed; 0 ignored;'
                if result.returncode != (101 if phase=='red' else 0) or not re.search(pattern,output,re.M):
                    print(output[-8000:]); raise SystemExit(f'{name}/{phase}: expected one runtime outcome, exit {result.returncode}')
                if phase=='red' and diagnostic and diagnostic not in output:
                    raise SystemExit(f'{name}: red did not reach required oracle {diagnostic!r}')
                binary_name = re.findall(r'^\s*Running .* \(([^()]+)\)$',output,re.M)[0]
                binary = (ROOT/binary_name).resolve()
                with binary.open('rb') as stream: digest = hashlib.file_digest(stream,'sha256').hexdigest()
                (directory/(phase+'.result.json')).write_text(json.dumps({'exit':result.returncode,'raw_summary':summaries,'binary':str(binary),'binary_sha256':digest,'started_ns':started_ns,'finished_ns':time.time_ns()},indent=2)+'\n')
                print(f'{name}/{phase}: {summaries[0]}',flush=True)
        finally:
            for file, data in originals.items(): (ROOT/file).write_bytes(data)
    subprocess.run([sys.executable,'-I',str(identity),'identity',str(evidence/'source-after.json')],cwd=ROOT,check=True)
    if (evidence/'source-before.json').read_bytes() != (evidence/'source-after.json').read_bytes():
        raise SystemExit('source identity changed across mutation run')

if __name__ == '__main__': main()
