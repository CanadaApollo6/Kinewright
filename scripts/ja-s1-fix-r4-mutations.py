"""Source-bound red/restored pairs for round 4, one named witness at a time."""
from pathlib import Path
import runpy
import sys
runner = runpy.run_path(str(Path(__file__).with_name('ja-s1-fix-r1-mutations.py')), run_name='r4_library')
cases = runner['cases']
cases.clear()
project = 'crates/kinewright-project/src/'
actor = 'crates/kinewright-core/src/actor.rs'
R = 'round_four::'
for name in ['moving_manual_core_reconciles_and_saves', 'moving_specialist_core_reconciles_and_saves', 'first_copy_moving_core_reconciles_and_saves']:
    cases[name] = (actor, 'ja_s1_storage', R+name)
for name in ['prefilter_small_excursion_keeps_first_valid_cut', 'prefilter_large_excursion_keeps_twenty_undo_steps', 'production_prefilter_matches_ten_thousand_seeded_histories']:
    cases[name] = (project+'bundle_commit.rs', 'ja_s1_storage', R+name)
cases.update({
    'first_failure_releases_every_original_undo_and_redo_then_retries': (actor, 'ja_s1_storage', R+'first_failure_releases_every_original_undo_and_redo_then_retries'),
    'older_root_releases_every_entry': (project+'bundle.rs', 'ja_s1_storage', R+'older_root_releases_every_entry'),
    'command_lands_mid_save_above_pending_floor': (actor, 'ja_s1_storage', R+'command_lands_mid_save_above_pending_floor'),
    'zero-counter-digits': (project+'bundle_commit.rs', 'ja_s1_storage', R+'prefilter_zero_undo_counters_cross_digit_boundaries'),
    'exact_cap_saves_and_one_byte_over_floors_one_more_step': (project+'bundle_commit.rs', 'ja_s1_storage', R+'exact_cap_saves_and_one_byte_over_floors_one_more_step'),
    'core-current': (actor, 'core-history', 'pending_floor_blocks_undo_and_commits_current_suffix'),
    'core-floor-revision': (actor, 'core-history', 'pending_floor_blocks_undo_and_commits_current_suffix'),
    'core-settled-guard': ('crates/kinewright-core/src/history_transition.rs', 'core-history', 'pending_floor_release_restores_entries_and_rejects_a_stale_capture'),
    'core-release-race': (actor, 'core-history', 'pending_floor_release_restores_entries_and_rejects_a_stale_capture'),
})
replace = runner['replace']
def mutate(source, name):
    if name.startswith('moving_') or name.startswith('first_copy_') or name == 'core-current':
        return replace(source, '        if commit {', '        if false && commit {')
    if name.startswith('prefilter_') or name.startswith('production_prefilter_'):
        return replace(source, '            let excursion = dropped > 0 && minimum_undo[cut] < dropped;', '            if dropped > 0 && minimum_undo[cut] < dropped { return false; }\n            let excursion = false;')
    if name.startswith('first_failure_'):
        return replace(source, '        } else {\n            self.pending_floor = None;', '        } else {\n            let _ = &self.pending_floor;')
    if name.startswith('older_root_'):
        return replace(source, 'self.settle_pending_save(own_root)?;', 'self.settle_pending_save(own_root || self.pending_save.is_some())?;')
    if name.startswith('command_lands_'):
        return replace(source, 'if matches!(command, Command::Undo)', 'if false && matches!(command, Command::Undo)')
    if name == 'zero-counter-digits':
        return replace(source, '2 * (count - threshold) as u64', '2 * (count - threshold + 1) as u64')
    if name.startswith('exact_cap_'):
        return replace(source, '.checked_sub(2 * record_cost(&serde_json::Value::Null, limits)?)', '.checked_sub(128)')
    if name == 'core-floor-revision':
        return replace(source, '                    .checked_add(1)\n                    .ok_or(HistoryError::Capacity)?', '                    .checked_add(2)\n                    .ok_or(HistoryError::Capacity)?')
    if name == 'core-settled-guard':
        return replace(source, '        if self.settled {\n            return Err(HistoryError::Corrupt);\n        }', '', count=2)
    if name == 'core-release-race':
        return replace(source, '|| self.history() != transition.before', '|| false && self.history() != transition.before')
    raise AssertionError(name)
selected = sys.argv[2:] or list(cases)
if selected == ['--check']:
    for name, (file, _, _) in cases.items():
        assert mutate(Path(file).read_text(), name) != Path(file).read_text()
        print(name, 'mutation applies')
else:
    runner['run_cases'](selected, mutate)
