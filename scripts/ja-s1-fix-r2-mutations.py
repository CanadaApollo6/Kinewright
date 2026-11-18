"""Re-record the 17 round-1 storage mutations and every new round-2 witness."""
from pathlib import Path
import runpy
import sys

runner = runpy.run_path(str(Path(__file__).with_name('ja-s1-fix-r1-mutations.py')), run_name='r2_mutation_library')
cases = runner['cases']
round_one = [name for name, (_, target, witness) in cases.items() if target == 'ja_s1_storage' and witness.startswith('final_head::')]
assert len(round_one) == 13
project = 'crates/kinewright-project/src/'
commit = project + 'bundle_commit.rs'
bundle = project + 'bundle.rs'
store = project + 'store.rs'
R = 'round_two::'
cases.update({'r2-copy-rollback': (store, 'ja_s1_copy', 'save_as_contention_preserves_source_and_destination_graphs'), 'r2-new-input': (store, 'ja_s1_storage', R + 'newly_admitted_unpinned_input_survives_previous_ack_cleanup'), 'r2-gc-retry': (commit, 'ja_s1_storage', R + 'persistent_collection_failure_is_recorded_and_does_not_block_saves'), 'r2-inventory': (store, 'ja_s1_storage', R + 'over_quota_reopen_keeps_inventory_only_owned_graph_until_retirement'), 'r2-lut-length': (commit, 'ja_s1_storage', R + 'wrong_lut_length_refuses_even_when_sha_and_entry_id_match'), 'r2-fixture-root': ('crates/kinewright-media/src/test_support.rs', 'media-lib', 'test_support::workspace_target_dir_is_absolute_without_parent_components')})
replace = runner['replace']

def mutate(source, name):
    if name not in cases or not name.startswith('r2-'):
        return runner['mutate'](source, name)
    if name == 'r2-copy-rollback':
        return replace(source, 'self.collect_copy_staging()?;', 'self.collect_unpublished()?;')
    if name == 'r2-new-input':
        return replace(source, '(ledger.committed.is_empty() || ledger.acknowledged_objects.contains(*h))', 'true')
    if name == 'r2-gc-retry':
        return replace(source, '&& let Err(error) = bundle.collect_after_ack()', '&& let Err(error) = { bundle.collect_after_ack()?; Ok::<u64, BundleError>(0) }')
    if name == 'r2-inventory':
        start = source.index('        crate::bundle_recovery::retain_inventory_graph(')
        end = source.index(')?;', start) + 3
        return source[:start] + '        if false {\n' + source[start:end] + '\n        }' + source[end:]
    if name == 'r2-lut-length':
        return replace(source, '&& e.object.byte_length == asset.byte_len', '')
    if name == 'r2-fixture-root':
        return replace(source, '.ancestors()\n        .nth(2)\n        .expect("workspace root")\n        .join("target")', '.join("../../target")')
    raise AssertionError(name)

selected = sys.argv[2:] or round_one+[name for name in cases if name.startswith('r2-')]
if selected == ['--check']:
    for name in round_one+[name for name in cases if name.startswith('r2-')]:
        source = Path(cases[name][0]).read_text()
        assert mutate(source, name) != source, name
        print(name, 'mutation applies')
else:
    runner['run_cases'](selected, mutate)
