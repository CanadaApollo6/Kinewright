"""Source-bound round-five mutation pairs; one affected witness per run."""
from pathlib import Path
import runpy
import sys
runner = runpy.run_path(str(Path(__file__).with_name('ja-s1-fix-r1-mutations.py')), run_name='r5_library')
cases = runner['cases']; cases.clear()
P = 'crates/kinewright-project/src/'
C = 'crates/kinewright-core/src/'
R = 'round_five::'
cases.update({'reopen-entry': (P + 'bundle.rs', 'ja_s1_storage', R + 'reopen_settles_the_orphan_before_restore'), 'frozen-save-as': (P + 'bundle.rs', 'ja_s1_storage', R + 'frozen_save_as_settles_its_source_core_before_copying'), 'hash-vectors': ('crates/kinewright-hash/src/lib.rs', 'media-lib', 'sha256::tests::matches_standard_sha256_vectors_and_streaming_boundaries'), 'pre-b5-object-retry': (P + 'store.rs', 'ja_s1_storage', R + 'pre_b5_object_uncertainty_releases_and_same_session_retries'), 'orphan-own': (P + 'bundle.rs', 'ja_s1_storage', R + 'orphan_equal_root_commits_with_new_owner'), 'orphan-older': (P + 'bundle.rs', 'ja_s1_storage', R + 'orphan_older_root_releases_with_new_owner'), 'orphan-branch': (P + 'bundle.rs', 'ja_s1_storage', R + 'orphan_branch_save_cannot_stack_a_barrier'), 'orphan-absent': (P + 'bundle.rs', 'ja_s1_storage', R + 'orphan_absent_root_releases_and_retries'), 'readable-save-as': (P + 'bundle.rs', 'ja_s1_storage', R + 'readable_orphan_a_reconciles_before_save_as_b'), 'unreadable-save-as': (P + 'bundle.rs', 'ja_s1_storage', R + 'unreadable_orphan_a_refuses_save_as_b_without_mutations_then_retries'), 'copy-source-recovery': (P + 'bundle_copy.rs', 'ja_s1_storage', R + 'dropped_first_copy_can_save_as_in_the_same_session'), 'capture-conflict': (P + 'bundle.rs', 'ja_s1_storage', R + 'command_during_selection_is_a_retryable_conflict'), 'ordinary-pre-b5': (P + 'bundle.rs', 'ja_s1_storage', R + 'ordinary_save_object_uncertainty_releases_then_new_owner_saves'), 'recreated-parent': (P + 'bundle.rs', 'ja_s1_storage', R + 'recreated_parent_a_is_a_definite_release_before_save_as_b'), 'vanished-root': (P + 'bundle.rs', 'ja_s1_storage', R + 'published_owner_does_not_accept_a_vanished_root_as_unpublished'), 'reconcile-object-retry': (P + 'bundle.rs', 'ja_s1_storage', R + 'ordinary_save_object_uncertainty_releases_then_new_owner_saves'), 'recorded-spawn-reap': ('crates/kinewright-agent/src/ja_harness/controller.rs', 'ja_baseline', 'setup_failure_after_a_recorded_spawn_still_reaps_it')})
replace = runner['replace']
def mutate(source, name):
    if name == 'hash-vectors':
        return replace(source, '0x6a09_e667,', '0x6a09_e668,')
    if name == 'reopen-entry':
        return replace(source, '        reconcile_core_save(core)?;\n        let branch', '        let branch')
    if name == 'pre-b5-object-retry':
        return replace(source, '        self.account_residuals(false)?;\n        self.inner', '        return Err(BundleError::Unsettled);\n        self.account_residuals(false)?;\n        self.inner')
    if name in ('orphan-own', 'orphan-branch', 'readable-save-as', 'frozen-save-as'):
        return replace(source, 'let commit = reconcile_save_tag(tag)?;', 'let commit = reconcile_save_tag(tag)? && false;')
    if name == 'orphan-older':
        return replace(source, 'let commit = reconcile_save_tag(tag)?;', 'let commit = reconcile_save_tag(tag)? || true;')
    if name == 'orphan-absent':
        return replace(source, '        let Some(bytes) = self.store.with_namespace(|n| n.read_root())? else {', '        let Some(bytes) = self.store.with_namespace(|n| n.read_root())? else {\n            return Err(BundleError::Corrupt("missing reconciliation root".into()));')
    if name == 'unreadable-save-as':
        return replace(source, 'read().map_err(|_| BundleError::DurabilityUnknownAt(tag.destination.clone()))', 'read().or(Ok(false))')
    if name == 'copy-source-recovery':
        return replace(source, '            .reconciled();\n    }\n    *pending = None;', '            .mark_uncertain()?;\n    }\n    *pending = None;')
    if name == 'capture-conflict':
        return replace(source, 'kinewright_core::HistoryError::CaptureChanged => BundleError::HistoryChanged,', 'kinewright_core::HistoryError::CaptureChanged => BundleError::Corrupt("selection raced".into()),')
    if name == 'ordinary-pre-b5':
        return replace(source, '                if self\n                    .pending_save\n                    .as_ref()\n                    .is_some_and(|p| p.root_sha256.is_some())\n                    && self.unsettled\n                {', '                if self.unsettled\n                    || matches!(error, BundleError::DurabilityUnknown | BundleError::Unsettled)\n                {')
    if name == 'recreated-parent':
        return replace(source, '        let own = tagged_parent\n', '        if !tagged_parent {\n            return Err(BundleError::Identity("reconciliation authority changed".into()));\n        }\n        let own = tagged_parent\n')
    if name == 'reconcile-object-retry':
        return replace(source, '        self.store\n            .with_namespace(ProjectNamespace::finish_object_batch)?;\n        if self.store.has_unsettled_objects()? {', '        if self.store.has_unsettled_objects()? {')
    if name == 'vanished-root':
        return replace(source, '        if self.root.is_some() {\n            // Only a never-published owner', '        if false && self.root.is_some() {\n            // Only a never-published owner')
    if name == 'recorded-spawn-reap':
        return replace(source, '            return Err(match controller.cleanup(report) {', '            return Err(match Ok::<(), String>(std::mem::take(&mut controller.workers).clear()) {')
    raise AssertionError(name)
selected = sys.argv[2:] or list(cases)
if selected == ['--check']:
    for name, (file, _, _) in cases.items():
        assert mutate(Path(file).read_text(), name) != Path(file).read_text()
        print(name, 'applies')
else:
    runner['run_cases'](selected, mutate)
