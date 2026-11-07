#!/usr/bin/env python3
"""B1 person-owned billing policy and hardened role exposure, exact restores."""
from pathlib import Path
import importlib.util
import os
import sys
sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('te15_b1_base', ROOT/'scripts/te13-merge-mutations.py')
h = importlib.util.module_from_spec(spec)
spec.loader.exec_module(h)
h.OUT = Path(os.environ.get('KINEWRIGHT_TE15_B1_OUT', ROOT/'target/te1-5/b1/pairs'))
h.PAIRS = []
A = 'crates/kinewright-agent/src/'
C = A+'orchestration/adapters/certification.rs'
O = A+'writer_host/orchestration.rs'
H = 'writer_host::hooks_tests::tests::'
P = 'orchestration::adapters::certification::tests::'
S = 'orchestration::specialist_setting::tests::'
W = H+'te15_b1_assignment_snapshots_person_choice_and_replays_original_after_change'
N = P+'te15_b1_native_profiles_pin_model_options_role_and_reject_tampering'
def add(name, file, before, after, witness, marker):
    h.PAIRS.append((name, file, before, after, 'kinewright-agent', witness, marker))
add('person-choice', O, '        bind(selection)\n', '        bind(assignment.brief.provider.clone())\n', W, 'TE15_PERSON_CHOICE')
add('immutable-replay', O, '.filter(|task| task.parent.as_ref() == Some(&assignment.parent))', '.filter(|task| false && task.parent.as_ref() == Some(&assignment.parent))', W, 'TE15_ASSIGN_REPLAY')
add('reconcile-provider', 'crates/kinewright-project/src/orchestration_records.rs', '.remove("provider");', '.remove("unused-provider");', W, 'TE15_ASSIGN_RECONCILE')
add('role-manifest', C, 'crate::orchestration::task_manifest::names(*role)', 'self.session.tools', N, 'TE15_ROLE_PIN')
add('durable-role', A+'writer_host/studio_runtime.rs', '.for_task(selection.clone(), role);', '.for_task(selection.clone(), crate::orchestration::orchestration_records::Role::Director);', W, 'TE15_FACTORY_ROLE')
add('selected-model', C, '.map_or_else(|| self.selection(), |(selection, _)| selection.clone())', '.map_or_else(|| self.selection(), |(_, _)| self.selection())', N, 'TE15_SELECTION_PIN')
add('fixed-session', C, 'if fixed != self.config {', 'if false && fixed != self.config {', N, 'TE15_POLICY_TAMPER')
add('unavailable-before-probe', C, '&& (selection.connector != self.provider', '&& false && (selection.connector != self.provider', P+'te15_b1_unavailable_task_policy_refuses_before_probe', 'TE15_UNAVAILABLE_PROBED')
add('unavailable-setting', A+'orchestration/specialist_setting.rs', '            SpecialistStatus::NoLongerOffered(selection)\n', '            SpecialistStatus::Set(SpecialistChoice::SameAsDirector)\n', S+'te15_b1_unavailable_and_broken_choices_are_preserved_and_refuse', 'TE15_SPECIALIST_REFUSAL')
# Nearby retained launch/dispatch security guards; no installed CLI runs.
L = A+'orchestration/native_launcher.rs'
I = H+'s4b_c6_claude_argv_and_environment_are_isolated'
add('retained-claude-tool-search', L, '("ENABLE_TOOL_SEARCH", "false".to_owned())', '("ENABLE_TOOL_SEARCH", "true".to_owned())', I, 'TOOL_SEARCH_OFF')
add('retained-claude-hooks', L, '"disableAllHooks": true,', '"disableAllHooks": false,', I, 'NO_HOOKS')
add('retained-role-dispatch', A+'server.rs', 'Ok(role) => task_role_tools(role)?,', 'Ok(_) => task_role_tools(crate::orchestration::orchestration_records::Role::Director)?,', H+'s4b_c6_merged_claude_mcp_surface_is_the_durable_role', 'C6_DURABLE_ROLE_SURFACE')
add('retained-hidden-role-tool', A+'orchestration/task_manifest.rs', '    "assign_task",', '    "prepare_edit_plan",\n    "assign_task",', H+'te12_list_hidden', 'TE12_LIST_HIDDEN')
J = 'crates/kinewright-project/src/orchestration_records.rs'
R = A+'writer_host/receipt_reconciliation.rs'
V = H+'te15_b1_legacy_assignment_receipt_digest_is_exact_and_read_only'
add('legacy-receipt', J, 'if !matches!(command, OrchestrationCommand::AssignTask(_)) {', 'if true || !matches!(command, OrchestrationCommand::AssignTask(_)) {', V, 'TE15_LEGACY_RECEIPT')
add('legacy-command-binding', J, 'if task_command_digest(command)? == *expected {', 'if true || task_command_digest(command)? == *expected {', V, 'TE15_LEGACY_CHANGED_COMMAND')
add('retained-receipt-task-scope', R, 'if !state.attempts.iter().any(original_attempt)', 'if false && !state.attempts.iter().any(original_attempt)', H+'te12_receipt_reconciliation_is_read_only_and_scope_bound', 'TE12_CROSS_TASK_RECEIPT')
if __name__ == '__main__':
    h.main()
