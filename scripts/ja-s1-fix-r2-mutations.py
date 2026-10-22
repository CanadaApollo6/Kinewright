"""Re-record the 17 round-1 storage mutations and every new round-2 witness."""
from pathlib import Path
import runpy
import sys

runner = runpy.run_path(str(Path(__file__).with_name('ja-s1-fix-r1-mutations.py')), run_name='r2_mutation_library')
cases = runner['cases']
round_one = [name for name, (_, target, witness) in cases.items() if target == 'ja_s1_storage' and witness.startswith('final_head::')]
assert len(round_one) == 17
project = 'crates/kinewright-project/src/'
commit = project + 'bundle_commit.rs'
bundle = project + 'bundle.rs'
store = project + 'store.rs'
R = 'round_two::'
cases.update({
    'r2-copy-rollback': (store, 'ja_s1_copy', 'save_as_contention_preserves_source_and_destination_graphs'),
    'r2-copy-trim': (project+'bundle_copy.rs', 'ja_s1_storage', R+'save_as_preserves_the_published_floor_and_refuses_silent_copy_trim'),
    'r2-live-floor': (bundle, 'ja_s1_storage', R+'ordinary_save_installs_floor_preserving_redo_gesture_and_reopen'),
    'r2-first-floor': (project+'bundle_copy.rs', 'ja_s1_storage', R+'first_save_installs_the_same_floor_before_handoff'),
    'r2-lut-floor': (commit, 'ja_s1_storage', R+'floor_retains_lut_context_across_multi_edit_save_reuse_and_retirement'),
    'r2-new-input': (store, 'ja_s1_storage', R+'newly_admitted_unpinned_input_survives_previous_ack_cleanup'),
    'r2-gc-retry': (commit, 'ja_s1_storage', R+'persistent_collection_failure_is_recorded_and_does_not_block_saves'),
    'r2-inventory': (store, 'ja_s1_storage', R+'over_quota_reopen_keeps_inventory_only_owned_graph_until_retirement'),
    'r2-comment-cut': (commit, 'ja_s1_storage', R+'no_edit_comment_repairs_the_original_core_cut_after_floor_reconciliation'),
    'r2-lut-length': (commit, 'ja_s1_storage', R+'wrong_lut_length_refuses_even_when_sha_and_entry_id_match'),
    'r2-specialist': ('crates/kinewright-agent/src/durable_branch.rs', 'ja_s1_branches', 'floored_specialist_reopens_original_full_delta_and_merges_once'),
    'r2-app-floor': (bundle, 'app-bin', 'app::in2b_tests::app_session_undo_stops_at_saved_floor_and_next_save_keeps_edits'),
    'r2-core-cut': ('crates/kinewright-core/src/actor.rs', 'core-history', 'checkpoint_install_refuses_a_newer_producer_suffix'),
    'r2-fixture-root': ('crates/kinewright-media/src/test_support.rs', 'media-lib', 'test_support::workspace_target_dir_is_absolute_without_parent_components'),
    'r2-floor-noop-redo': (commit, 'ja_s1_storage', R+'ordinary_save_installs_floor_preserving_redo_gesture_and_reopen'),
    'r2-floor-noop-undo': (commit, 'ja_s1_storage', R+'ordinary_save_installs_floor_preserving_redo_gesture_and_reopen'),
})
replace = runner['replace']

def mutate(source, name):
    if name not in cases or not name.startswith('r2-'):
        return runner['mutate'](source, name)
    if name == 'r2-copy-rollback':
        return replace(source, 'self.collect_copy_staging()?;', 'self.collect_unpublished()?;')
    if name == 'r2-copy-trim':
        return replace(source, 'crate::bundle_commit::BundleCommit::publish(&mut target, &history, &inputs)',
            'let floor = kinewright_core::HistoryTransition::floor((*history).clone(), 1).map_err(|e| BundleError::Corrupt(e.to_string()))?;\n            crate::bundle_commit::BundleCommit::publish(&mut target, floor.after(), &inputs)')
    if name in ('r2-live-floor', 'r2-app-floor'):
        return replace(source, 'self.install_published(core, before, &id)?;', 'let _ = (core, before, id);')
    if name == 'r2-first-floor':
        return replace(source, 'target.install_published(core, frozen.clone(), &id)?;', 'let _ = (core, &frozen, id);')
    if name == 'r2-lut-floor':
        return replace(source, 'contexts[0] = seal!(&floor_inputs);', 'let _ = floor_inputs;')
    if name == 'r2-new-input':
        return replace(source, '(ledger.committed.is_empty() || ledger.acknowledged_objects.contains(*h))', 'true')
    if name == 'r2-gc-retry':
        return replace(source, '&& let Err(error) = bundle.collect_after_ack()', '&& let Err(error) = { bundle.collect_after_ack()?; Ok::<u64, BundleError>(0) }')
    if name == 'r2-inventory':
        start = source.index('        crate::bundle_recovery::retain_inventory_graph(')
        end = source.index(')?;', start)+3
        return source[:start]+'        if false {\n'+source[start:end]+'\n        }'+source[end:]
    if name == 'r2-comment-cut':
        return replace(source, 'if **published != original', 'if **published != before')
    if name == 'r2-lut-length':
        return replace(source, '&& e.object.byte_length == asset.byte_len', '')
    if name == 'r2-specialist':
        before = 'let baseline_delta = specialist_floor_delta(bundle, &base, &history)?;'
        after = 'if history.baseline != Arc::new((*baseline).clone()) || history.applied_operations() != base.full_applied_delta { return Err(BundleError::Corrupt("frozen specialist delta differs from checkpoint".into())); }\n'+before
        return replace(source, before, after)
    if name == 'r2-core-cut':
        start = source.index('    pub fn install_checkpoint(')
        end = source.index('    /// Install one validated floor', start)
        part = replace(source[start:end], 'Some(Box::new(before)),', '{ let _ = before; None },')
        return source[:start]+part+source[end:]
    if name == 'r2-fixture-root':
        return replace(source, '.ancestors()\n        .nth(2)\n        .expect("workspace root")\n        .join("target")', '.join("../../target")')
    if name == 'r2-floor-noop-redo':
        return replace(source, '''if let Some(next) = redo.pop() {
                    undo.push(current);
                    current = next;
                }''', '''let next = redo.pop().ok_or_else(|| BundleError::Corrupt("lineage redo underflow".into()))?;
                undo.push(current);
                current = next;''')
    if name == 'r2-floor-noop-undo':
        return replace(source, '''if let Some(previous) = undo.pop() {
                    redo.push(current);
                    current = previous;
                }''', '''let previous = undo.pop().ok_or_else(|| BundleError::Corrupt("lineage undo underflow".into()))?;
                redo.push(current);
                current = previous;''')
    raise AssertionError(name)

selected = sys.argv[2:] or round_one+[name for name in cases if name.startswith('r2-')]
if selected == ['--check']:
    for name in round_one+[name for name in cases if name.startswith('r2-')]:
        source = Path(cases[name][0]).read_text()
        assert mutate(source, name) != source, name
        print(name, 'mutation applies')
else:
    runner['run_cases'](selected, mutate)
