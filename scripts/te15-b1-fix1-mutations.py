#!/usr/bin/env python3
"""B1 readiness, safe settings and immutable refusal pairs with exact restores."""
from pathlib import Path
import importlib.util
import os
import sys
sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('te15_fix_base', ROOT/'scripts/te13-merge-mutations.py')
h = importlib.util.module_from_spec(spec)
spec.loader.exec_module(h)
h.OUT = Path(os.environ.get('KINEWRIGHT_TE15_B1_FIX1_OUT', ROOT/'target/te1-5/b1-fix1/pairs'))
h.PAIRS = []
A = 'crates/kinewright-agent/src/'
W = A+'writer_host/'
R = W+'provider_readiness.rs'
O = W+'orchestration.rs'
S = A+'orchestration/specialist_setting.rs'
H = 'writer_host::hooks_tests::tests::'
P = 'orchestration::specialist_setting::'
T = H+'te15_b1_fix_'
def add(name, file, before, after, witness, marker, package='kinewright-agent'):
    h.PAIRS.append((name, file, before, after, package, witness, marker))
add('codex-mixed-preflight', R,
    'self.check_except(checks, None)', 'Ok(())',
    T+'codex_waits_for_claude_before_send', 'TE15_FIX_PREFLIGHT_BEFORE_RESERVATION')
add('claude-mixed-preflight', R,
    'checks\n                .preflight(spec)',
    'checks\n                .preflight(spec).or_else(|error| if spec.provider() == "codex" { Ok(()) } else { Err(error) })',
    T+'claude_waits_for_codex_before_send', 'TE15_FIX_ZERO_SEND_provider')
add('malformed-preflight', S, 'SpecialistStatus::Unreadable(why) => Err(why),',
    'SpecialistStatus::Unreadable(_) => Ok(director.clone()),', T+'malformed_waits_before_send', 'TE15_FIX_ZERO_SEND_malformed')
add('retired-preflight', S, 'SpecialistStatus::NoLongerOffered(_) => Err(UNAVAILABLE.into()),',
    'SpecialistStatus::NoLongerOffered(_) => Ok(director.clone()),', T+'retired_waits_before_send', 'TE15_FIX_ZERO_SEND_retired')
add('replay-preflight', W+'task_mcp.rs', 'let _ = super::provider_readiness::check_run(&owner, &state.run);', 'let _ = &state.run;', T+'replay_keeps_outcome_but_waits_on_broken_choice', 'TE15_FIX_REPLAY_SETUP_WAIT')
midrun=(ROOT/(W+'task_mcp.rs')).read_text()
a=midrun.index('            super::provider_readiness::check_run(&owner, &state.run).map_err')
b=midrun.index('            host = owner',a)
add('midrun-preflight', W+'task_mcp.rs', midrun[a:b], '            let _ = &state.run;\n', T+'midrun_assignment_waits_without_another_send', 'TE15_FIX_MIDRUN_WAIT')
add('active-turn-preserved', R, '        if !same {',
    '        for native in self.hooks.native.values().filter(|n| n.identity.run == *run) { native.control.close(); }\n        if !same {',
    T+'bad_setting_keeps_the_live_director', 'TE15_FIX2_LIVE_TURN_PRESERVED')
h.EQUIPPED_WITNESSES={T+'bad_setting_keeps_the_live_director'}
add('continue-settings-wait', A+'orchestration/machine.rs',
    'matches!(state.wait, RunWait::None | RunWait::ProviderSettings(_))',
    'state.wait == RunWait::None', T+'continue_and_steering_recheck_choices', 'TE15_FIX_CONTINUE_REPAIRED')
add('reopen-preflight', R, 'RunAction::Continue | RunAction::SendMessage { .. }',
    'RunAction::SendMessage { .. }', T+'reopen_continue_checks_the_new_setting', 'TE15_FIX_REOPEN_PREFLIGHT')
add('pending-original-first', R, 'if self.hooks.pending.is_some() {', 'if false && self.hooks.pending.is_some() {', T+'pending_revise_reconciles_before_new_choices', 'TE15_FIX_PENDING_REVISE_RECONCILES')
add('http-entry-preflight', W+'mod.rs',
    'let reply = provider_readiness::execute(&state.owner, &bearer, request);',
    'let reply = state.owner.lock().unwrap().execute_bearer(Some(&bearer), request);',
    T+'revise_and_direct_start_wait_before_spending', 'TE15_FIX_REVISE_WAIT')
add('host-mcp-entry-preflight', W+'mod.rs',
    'Ok::<_, ()>(provider_readiness::execute(&state.owner, &bearer, request))',
    'Ok::<_, ()>(state.owner.lock().unwrap().execute_bearer(Some(&bearer), request))',
    T+'host_mcp_revise_waits_before_spending', 'TE15_FIX_HOST_MCP_WAIT')
replay='''if let Some(receipt) = self
            .owner()?
            .orchestration_receipt(run, &request.envelope.id)?'''
add('refused-replay', O, replay, 'if let Some(receipt) = None::<OrchestrationReceipt>',
    T+'refused_assignment_replays_after_settings_change', 'TE15_FIX_REFUSED_REPLAY')
F=A+'orchestration/setting_file.rs'
add('dangling-choice', F, 'std::fs::symlink_metadata(path)', 'std::fs::metadata(path)',
    P+'entry_tests::te15_b1_fix_links_never_inherit_or_follow', 'TE15_FIX_DANGLING_INHERITED')
# Remove the actual shape/no-follow/nonblocking open guard. The parent witness
# kills and reaps its FIFO reader if the old blocking open is restored.
source=(ROOT/F).read_text()
a=source.index('    let metadata = match')
b=source.index('    let mut bytes = Vec::new();',a)
add('fifo-choice', F, source[a:b],
    '    let file = std::fs::File::open(path).map_err(|_| ())?;\n',
    P+'te15_b1_fix_fifo_refuses_without_blocking', 'TE15_FIX_FIFO_BOUNDED')
add('test-settings-isolation', A+'orchestration/director_setting.rs',
    'panic!("TE15_FIX_TEST_DEFAULT_SETTINGS");', 'return Ok(PathBuf::new());',
    'orchestration::director_setting::te15_b1_fix_test_hosts_cannot_read_default_folder', 'TE15_FIX_NO_PERSONAL_FOLDER')
add('executable-repair', A+'orchestration/native_runtime.rs',
    'crate::orchestration::adapters::CertificationRefusal::ExecutableResolution(error.to_string())',
    'crate::orchestration::adapters::CertificationRefusal::ExecutableIdentity',
    'native_runtime::tests::te15_b1_fix_two_installs_keep_the_repair_words', 'TE15_FIX_EXECUTABLE_REPAIR')
add('provider-omission', W+'task_mcp.rs',
    'brief.insert(\n                "provider".into(),\n                serde_json::to_value(&state.tasks[0].brief.provider)?,\n            );',
    'let _ = brief;', T+'assignment_omits_provider_on_the_wire', 'TE15_FIX_NO_PROVIDER_REQUIRED')
add('unbound-assignment', O,
    'let Some(run) = &request.run else {\n            return Err(refused("assignment run is unavailable"));\n        };',
    'let Some(run) = &request.run else { return Ok(request.clone()); };',
    T+'assignment_without_a_run_refuses_at_binding', 'TE15_FIX_UNBOUND_ASSIGNMENT')
add('unknown-run-assignment', O,
    'let Some(state) = self.owner()?.orchestration_projection(run)? else {\n            return Err(refused("assignment run is unavailable"));\n        };',
    'let Some(state) = self.owner()?.orchestration_projection(run)? else { return Ok(request.clone()); };',
    T+'assignment_without_a_run_refuses_at_binding', 'TE15_FIX_UNKNOWN_RUN_ASSIGNMENT')
add('tier-pin', A+'orchestration/adapters/certification.rs',
    'service_tier: option("service_tier")', 'service_tier: option("unbound_service_tier")',
    'orchestration::adapters::certification::tests::te15_b1_native_profiles_pin_model_options_role_and_reject_tampering', 'TE15_FIX_TIER_PIN')
app='crates/kinewright-app/src/settings_ui.rs'
ui='settings_ui::te15_tests::te15_b1_settings_show_default_and_persist_specific_without_director_change'
add('run-wording', app, 'ui.colored_label(color::TEXT_MUTED, "Specialists follow the Director of their run. A run that\'s already open keeps its own.");',
    'ui.colored_label(color::TEXT_MUTED, "New specialists follow the current Director.");', ui, 'TE15_FIX_RUN_WORDS', 'kinewright-app')
add('app-settings-isolation', app, 'Some(crate::ja1_attachment::test_scheduler_folder())', 'None', ui, 'TE15_FIX_APP_SETTINGS_ISOLATED', 'kinewright-app')
add('fast-wording', app, 'label.push_str(" · Fast (increased plan usage)");',
    'label.push_str(" · Fast");', ui, 'TE15_FIX_FAST_WORDS', 'kinewright-app')
# Nearby person-choice, immutable outcomes, role/profile and receipt scope guards.
spec = importlib.util.spec_from_file_location('te15_b1_near', ROOT/'scripts/te15-b1-mutations.py')
near = importlib.util.module_from_spec(spec)
spec.loader.exec_module(near)
names={'person-choice','immutable-replay','reconcile-provider','selected-model','durable-role','role-manifest','fixed-session','unavailable-before-probe','retained-receipt-task-scope'}
selected=[row for row in near.h.PAIRS if row[0] in names]
assert len(selected)==len(names)
h.PAIRS.extend(('retained-'+row[0].removeprefix('retained-'), *row[1:]) for row in selected)
if __name__ == '__main__':
    h.main()
