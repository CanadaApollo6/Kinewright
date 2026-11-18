"""One source-bound red/restored pair per round-3 runtime witness."""
from pathlib import Path
import runpy
import sys

runner = runpy.run_path(str(Path(__file__).with_name('ja-s1-fix-r1-mutations.py')), run_name='r3_library')
cases = runner['cases']
project = 'crates/kinewright-project/src/'
R = 'round_three::'
cases.clear()
cases.update({'r3-lut-ids': (project + 'bundle_commit.rs', 'ja_s1_storage', R + 'short_reused_lut_ids_floor_save_and_reopen_both_contents'), 'r3-persistent-gc': (project + 'store.rs', 'ja_s1_storage', R + 'persistent_file_refusal_collects_other_garbage_and_reports_pending_bytes')})
replace = runner['replace']

def mutate(source, name):
    if name == 'r3-lut-ids':
        return replace(source, 'if entry.object.record_kind == RecordKind::OwnedLut', 'if false && entry.object.record_kind == RecordKind::OwnedLut')
    if name == 'r3-persistent-gc':
        return replace(source, '                Ok(false)', '                Err(error)')
    raise AssertionError(name)

selected = sys.argv[2:] or list(cases)
if selected == ['--check']:
    for name, (file, _, _) in cases.items():
        assert mutate(Path(file).read_text(), name) != Path(file).read_text()
        print(name, 'mutation applies')
else:
    runner['run_cases'](selected, mutate)
