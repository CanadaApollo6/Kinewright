"""One source-bound red/restored pair per round-3 runtime witness."""
from pathlib import Path
import runpy
import sys

runner = runpy.run_path(str(Path(__file__).with_name('ja-s1-fix-r1-mutations.py')), run_name='r3_library')
cases = runner['cases']
project = 'crates/kinewright-project/src/'
R = 'round_three::'
cases.clear()
cases.update({
    'r3-first-bytes': (project+'bundle_copy.rs', 'ja_s1_storage', R+'first_save_floors_history_above_default_encoded_limit'),
    'r3-first-batches': (project+'bundle_copy.rs', 'ja_s1_storage', R+'first_save_floors_history_above_default_batch_limit'),
    'r3-first-copy-own': (project+'bundle_copy.rs', 'ja_s1_storage', R+'first_save_unknown_floor_handoff_reconciles_once'),
    'r3-lut-ids': (project+'bundle_commit.rs', 'ja_s1_storage', R+'short_reused_lut_ids_floor_save_and_reopen_both_contents'),
    'r3-manual-own': (project+'bundle.rs', 'ja_s1_storage', R+'reconcile_own_floor_installs_manual_core'),
    'r3-manual-older': (project+'bundle.rs', 'ja_s1_storage', R+'reconcile_older_root_preserves_manual_unsaved_edits'),
    'r3-specialist-own': (project+'bundle.rs', 'ja_s1_storage', R+'reconcile_own_floor_installs_specialist_core'),
    'r3-specialist-older': (project+'bundle.rs', 'ja_s1_storage', R+'reconcile_older_root_preserves_specialist_unsaved_edits'),
    'r3-persistent-gc': (project+'store.rs', 'ja_s1_storage', R+'persistent_file_refusal_collects_other_garbage_and_reports_pending_bytes'),
    'r3-floor-search': ('crates/kinewright-core/src/actor.rs', 'core-history', 'floor_search_matches_exhaustive_nonmonotone_cuts'),
})
replace = runner['replace']

def mutate(source, name):
    if name == 'r3-first-copy-own':
        source = replace(source, 'use kinewright_core::Core;', 'use kinewright_core::{Core, HistorySnapshot};')
        source = replace(source, '    input_rebindings: BTreeMap<ObjectRef, ObjectRef>,', '    input_rebindings: BTreeMap<ObjectRef, ObjectRef>,\n    handoff: Option<(Core, HistorySnapshot)>,')
        source = replace(source, '    publication: Result<BundleSaveReport, BundleError>,', '    publication: Result<BundleSaveReport, BundleError>,\n    handoff: Option<(Core, HistorySnapshot)>,')
        source = replace(source, '        input_rebindings: rebindings,', '        input_rebindings: rebindings,\n        handoff,')
        call = 'result(target, before, prepared, source, map, publication)'
        assert source.count(call) == 2
        source = source.replace(call, 'result(target, before, prepared, source, map, publication, None)', 1)
        source = replace(source, call, 'result(target, before, prepared, source, map, publication, Some((core.clone(), frozen)))')
        return replace(source, '        self.status = CopyStatus::Committed(report.clone());', '''        if let Some((core, before)) = &self.handoff {
            let id = destination.manual_branch_id.clone();
            destination.install_published(core, before.clone(), &id)?;
        }
        self.status = CopyStatus::Committed(report.clone());''')
    if name in ('r3-first-bytes', 'r3-first-batches'):
        return replace(source, 'let bounded = crate::bundle_commit::bound_history(&frozen, &limits)?;',
            'frozen.encoded_bytes().map_err(|e| BundleError::Corrupt(e.to_string()))?;\n        let bounded = crate::bundle_commit::bound_history(&frozen, &limits)?;')
    if name == 'r3-lut-ids':
        return replace(source, 'if entry.object.record_kind == RecordKind::OwnedLut', 'if false && entry.object.record_kind == RecordKind::OwnedLut')
    if name.endswith('-own'):
        start = source.index('        if let Some(pending) = self.pending_save.take()')
        end = source.index('        self.unsettled = false;', start)
        return source[:start] + '        self.pending_save = None;\n' + source[end:]
    if name.endswith('-older'):
        start = source.index('        if let Some(pending) = self.pending_save.take()')
        end = source.index('        self.unsettled = false;', start)
        part = source[start:end]
        guard = part.index('&& pending.root_sha256')
        stop = part.index('\n            && let Err(error)', guard)
        return source[:start]+part[:guard]+'&& pending.root_sha256.is_some()'+part[stop:]+source[end:]
    if name == 'r3-persistent-gc':
        return replace(source, '                Ok(false)', '                Err(error)')
    if name == 'r3-floor-search':
        return replace(source, '&& accepts(&after)', '&& false && accepts(&after)')
    raise AssertionError(name)

selected = sys.argv[2:] or list(cases)
if selected == ['--check']:
    for name, (file, _, _) in cases.items():
        assert mutate(Path(file).read_text(), name) != Path(file).read_text()
        print(name, 'mutation applies')
else:
    runner['run_cases'](selected, mutate)
