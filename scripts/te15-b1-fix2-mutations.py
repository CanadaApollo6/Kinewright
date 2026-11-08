#!/usr/bin/env python3
"""B1 fix 2 live recovery, last-unpaid checks and safe saved-choice pairs."""
from pathlib import Path
import importlib.util
import os
import sys
sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent

def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'scripts' / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

h = load('fix2_base', 'te13-merge-mutations.py')
h.OUT = Path(os.environ.get('KINEWRIGHT_TE15_B1_FIX2_OUT', ROOT / 'target/te1-5/b1-fix2/pairs'))
h.PAIRS = []
A = 'crates/kinewright-agent/src/'
W = A + 'writer_host/'
R = W + 'provider_readiness.rs'
M = A + 'orchestration/machine.rs'
E = A + 'orchestration/effect_pump.rs'
O = W + 'orchestration.rs'
S = W + 'studio_runtime.rs'
F = A + 'orchestration/setting_file.rs'
N = A + 'orchestration/native_launcher.rs'
H = 'writer_host::hooks_tests::tests::'
T = H + 'te15_b1_fix2_'
C = A + 'orchestration/adapters/certification.rs'
U = 'crates/kinewright-app/src/settings_ui.rs'
CATALOG = 'orchestration::adapters::certification::tests::te15_b1_native_profiles_pin_model_options_role_and_reject_tampering'
UI = 'settings_ui::te15_tests::te15_b1_settings_show_default_and_persist_specific_without_director_change'
h.EQUIPPED_WITNESSES = {T + suffix for suffix in ['live_continue_sends_once', 'live_child_wakes_after_repair', 'live_steering_reuses_its_effect', 'setup_recheck_wait_and_unknown_continue_refusal', 'postcommit_error_keeps_provider_recovery']}
h.EQUIPPED_WITNESSES.add(H + 'te15_b1_fix_bad_setting_keeps_the_live_director')
h.EMPTY_CATALOG_WITNESSES = {CATALOG, UI}
h.TEST_TARGETS = {'s4b_unavailable_saved_choice_is_kept_and_never_substituted': ['--test', 'ja_s4_certification']}

def add(name, file, before, after, witness, marker, package='kinewright-agent'):
    h.PAIRS.append((name, file, before, after, package, witness, marker))

def section(file, start, end):
    text = (ROOT / file).read_text()
    a = text.index(start)
    return text[a:text.index(end, a)]

add('pump-alive', E,
    'if resumable {',
    'if false && resumable {',
    T+'setup_recheck_wait_and_unknown_continue_refusal', 'TE15_FIX2_SETUP_WAIT_PUMP_ALIVE')
add('live-child-authority', O,
    '!matches!(projection.wait, crate::orchestration::orchestration_records::RunWait::ProviderSettings(_))',
    'true', T+'live_child_wakes_after_repair', 'TE15_FIX2_CHILD_CAN_FINISH')
add('end-turn-words', R,
    'The run is waiting for the person to fix Settings. End your turn now. Do not retry this call.',
    'The run is waiting for the person to fix Settings.', T+'live_continue_sends_once', 'TE15_FIX2_END_TURN_WORDS')
add('fresh-continuation', M, 'if needs_setup_continuation(&next) {',
    'if false && needs_setup_continuation(&next) {',
    T+'live_continue_sends_once', 'TE15_FIX2_REAL_CONTINUATION')
for name, witness in [('reuse-wake', 'live_child_wakes_after_repair'), ('reuse-steering', 'live_steering_reuses_its_effect')]:
    body = section(M, '    matches!(state.wait, RunWait::ProviderSettings(_))', '\n}\n\n/// C5 fix 10: the fixed reason')
    add(name, M, body, '    matches!(state.wait, RunWait::ProviderSettings(_))', T+witness, 'TE15_FIX2_PENDING_REUSED')
body = section(M, '    super::requests::director(state).is_ok_and(|task| {', '\n}\n\npub(crate) fn needs_setup_continuation')
add('unknown-continue', M, body, '    let _ = state;\n    true',
    H+'te15_b1_fix3_claimed_and_unknown_have_distinct_words', 'FIX3_TRUE_CONTINUE_WORDS')
add('final-other-provider', R, 'plan.check_except(checks.as_ref(), Some(provider))',
    '{ let _ = (checks, provider); Ok(()) }', T+'bound_signout_does_not_charge', 'TE15_FIX2_FINAL_ZERO_SEND')
body = section(R, '        let current = owner.readiness_for_run(run);', '\n    }\n}\n')
add('final-settings', R, body, '        let _ = (owner, run); Ok(())',
    T+'bound_settings_does_not_charge', 'TE15_FIX2_FINAL_ZERO_SEND')
add('charge-lock-compare', E, 'plan.current(&mut host, &self.run)?;',
    'let _ = &plan;', T+'settings_compare_is_under_the_charge_lock', 'TE15_FIX2_CHARGE_COMPARE')
add('probe-budget', R, 'plan.check_except(checks.as_ref(), Some(provider))',
    '{ let _ = (checks, provider); Ok(()) }', T+'probe_counts_stay_bounded', 'TE15_FIX2_PROBE_BUDGET')
body = section(N, '            .recheck_runtime(self.checks.as_ref())', '\n        let immutable =')
add('typed-setup-launch', N, body,
    '            .recheck_runtime(self.checks.as_ref()).map_err(io::Error::other)?;',
    T+'setup_recheck_wait_and_unknown_continue_refusal', 'TE15_FIX2_SETUP_RECHECK_CLASSIFIED')
add('generic-launch', R, 'if resumable {', 'if true || resumable {', T+'postcommit_error_keeps_provider_recovery', 'TE15_FIX2_POSTCOMMIT_PROVIDER_WAIT')
add('generic-model-words', W+'task_mcp.rs',
    'if super::provider_readiness::setup_failure(&why) {',
    'if true || super::provider_readiness::setup_failure(&why) {',
    T+'nonsetup_model_refusal_keeps_provider_words', 'TE15_FIX2_NONSETUP_MODEL_WORDS')
body = section(S, '        let state = self.spending_scope(producer, &identity.run)?;\n        if matches!(', '\n        let replan =')
add('stream-wait', S, body, '        let state = self.spending_scope(producer, &identity.run)?;',
    T+'stream_refusal_preserves_setup_wait', 'TE15_FIX2_STREAM_WAIT_PRESERVED')
add('queued-retry-bound', S, 'if blocked_run == &run && continuation == &current {',
    'if false && blocked_run == &run && continuation == &current {',
    T+'queued_failed_worker_needs_a_new_continue', 'TE15_FIX2_BLOCKED_QUEUED')
add('acknowledged-continue', O, 'runtime.last_continue = Some(receipt.sequence);',
    'let _ = runtime;', T+'queued_failed_worker_needs_a_new_continue', 'TE15_FIX2_CONTINUE_UNBLOCKED')
add('director-link', F, 'std::fs::symlink_metadata(path)', 'std::fs::metadata(path)',
    'orchestration::director_setting::te15_b1_fix2_director_links_keep_an_unreadable_choice', 'TE15_FIX2_DIRECTOR_LINK')
body = section(F, '    let metadata = match', '    let mut bytes = Vec::new();')
add('director-fifo', F, body, '    let file = std::fs::File::open(path).map_err(|_| ())?;\n',
    'orchestration::director_setting::te15_b1_fix2_director_fifo_is_bounded', 'TE15_FIX2_DIRECTOR_FIFO_BOUNDED')
add('unavailable-director-words', A+'orchestration/director_setting.rs',
    'DirectorStatus::NoLongerOffered(_) => Err("Your saved Director choice is no longer available. Choose another Director in Settings; your notes are saved and the choice has not been substituted.".into()),',
    'DirectorStatus::NoLongerOffered(director) => Err(format!("The Director you chose ({} · {}) is no longer available on this computer. Your notes are saved. Choose another Director in Settings, then Revise cut again.", director.connector, director.model)),',
    's4b_unavailable_saved_choice_is_kept_and_never_substituted', 'TE15_FIX2_UNAVAILABLE_DIRECTOR_FIXED_WORDS')
for name, file, witness, marker, package in [
    ('agent-private-catalog', C, CATALOG, 'TE15_FIX2_PRIORITY_CATALOG', 'kinewright-agent'),
    ('app-private-catalog', U, UI, 'TE15_FIX2_APP_PRIORITY_CATALOG', 'kinewright-app')]:
    add(name, file, 'if !te15_catalog_child(', 'if false && !te15_catalog_child(', witness, marker, package)
near = load('fix2_near', 'te15-b1-fix1-mutations.py')
names = {'active-turn-preserved', 'dangling-choice', 'fifo-choice', 'replay-preflight', 'refused-replay', 'malformed-preflight', 'tier-pin', 'retained-immutable-replay', 'pending-original-first'}
rows = [row for row in near.h.PAIRS if row[0] in names]
assert len(rows) == len(names)
h.PAIRS.extend(('near-'+row[0], *row[1:]) for row in rows)
for name, file, before, *_ in h.PAIRS:
    assert (ROOT / file).read_text().count(before) == 1, name + ': mutation anchor drifted'
if __name__ == '__main__':
    h.main()
