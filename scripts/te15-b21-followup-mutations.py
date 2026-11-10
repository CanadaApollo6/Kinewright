#!/usr/bin/env python3
"""All B2-1 pairs, nearby retained guards, and seven follow-up witnesses."""
from pathlib import Path
import importlib.util
import os
import sys

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
os.environ.setdefault('KINEWRIGHT_TE15_B21_OUT', str(ROOT/'target/te1-5/b2-1-followups/pairs'))
spec = importlib.util.spec_from_file_location('b21', ROOT/'scripts/te15-b21-mutations.py')
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)
A = m.A
C = m.C
S = m.S
B = m.B
H = 'writer_host::hooks_tests::tests::tests::consent_followups::tests::te15_b21f_'
U = 'writer_host::task_mcp::consent::tests::te15_b21f_'
APP = 'crates/kinewright-app/src/studio_run_view.rs'
APP_TEST = 'studio_feedback::ja_s3_feedback::te15_b1_fix4::'
P = 'crates/kinewright-project/src/orchestration_commit.rs'

history = '''            let history = self.history_with_edit_consent(
                &core,
                step.expected_revision,
                name == "redo_task_edit",
            )?;'''
bypass = '''            let history = self.endpoint.history_step(
                &core, step.expected_revision, name == "redo_task_edit"
            ).map_err(|e| refused(&e))?;'''
for name, witness, marker in [
    ('history-missing-grant', 'legacy_history_refuses', 'FOL_LEGACY_HISTORY'),
    ('history-destructive-approval', 'history_uses_inverse_classes_and_records_application', 'FOL_DESTRUCTIVE_UNDO_RECORDED'),
]:
    m.add(name, [(A+'writer_host/task_mcp.rs', history, bypass)], H+witness, marker)
m.add('history-valid-direction', [(C, '.history_step(core, revision, *redo)', '.history_step(core, revision, !*redo)')], H+'history_uses_inverse_classes_and_records_application', 'FOL_HISTORY_ALLOWED')
m.add('inverse-add-clip', [(C, '''        "add_clip"
        | "add_title"''', '''        "add_title"''')], U+'add_clip_inverse_is_removal', 'FOL_ADD_CLIP_INVERSE')
m.add('add-track-class', [(C, '        "add_track"\n        | "add_clip"', '        "add_clip"')], H+'add_track_is_permitted_in_scope', 'FOL_PREPARE')
m.add('operation-decision-list', [(C, '"trim_clip" => Some("trim"),', '"trim_clip" => None,')], U+'every_operation_is_decided', 'FOL_OPERATION_DECISION')
m.add('legacy-refusal-words', [(C, '        "consent_missing",', '        "consent_required",')], H+'legacy_history_refuses', 'FOL_LEGACY_WORDS')
m.add('policy-refusal-words', [(S, 'else {\n                                "consent_policy"\n                            }', 'else {\n                                ""\n                            }')], H+'policy_and_turn_reasons', 'FOL_POLICY_WORDS')
m.add('turn-refusal-words', [(C, '                    "consent_turn",', '                    "consent_required",')], H+'policy_and_turn_reasons', 'FOL_TURN_WORDS')
m.add('scope-refusal-words', [(C, 'problem("consent_scope", format!("{reason}. Tell the Director."))', 'problem("consent_required", format!("{reason}. Tell the Director."))')], H+'scope_reason_tells_director', 'FOL_SCOPE_WORDS')
m.add('broker-refusal-words', [(B, 'Self::Capacity => "consent_broker_capacity",', 'Self::Capacity => "consent_required",')], H+'broker_capacity_has_its_reason', 'FOL_BROKER_WORDS')
m.add('short-lock-window', [(C, 'std::time::Duration::from_millis(350)', 'std::time::Duration::from_millis(0)')], H+'short_host_hold_succeeds', 'FOL_SHORT_BUSY')
m.add('long-lock-window', [(C, 'std::time::Duration::from_millis(350)', 'std::time::Duration::from_millis(1000)')], H+'long_host_hold_refuses_without_edit', 'FOL_LONG_BUSY')
m.add('applied-only-after-success', [(C, 'if outcome.applied() {', 'if true {')], H+'failed_core_is_not_applied', 'FOL_NOT_APPLIED')
m.add('bad-audit-retains-view', [(C, '        if integrity {', '        if false {')], H+'bad_audit_keeps_run_view', 'FOL_AUDIT_INTEGRITY')
m.add('plain-applied-counts', [(APP, '"Removed {} in the draft version. Your project is unchanged until you choose it."', '"Plan removes {} - approve?"')], APP_TEST+'te15_b21_run_details_lists_the_approved_private_batch', 'B21_PLAIN_APPROVAL_DETAILS', 'kinewright-app')
m.add('collapse-applied-list', [(APP, 'if applied.len() > 5 {', 'if false {')], APP_TEST+'te15_b21f_details_distinguish_unapplied_integrity_and_summary', 'FOL_COLLAPSED_DETAILS', 'kinewright-app')
m.add('plain-unapplied-words', [(APP, 'approved, but {} changes are not confirmed.', 'approved, and {} changes were made.')], APP_TEST+'te15_b21f_details_distinguish_unapplied_integrity_and_summary', 'FOL_UNAPPLIED_WORDS', 'kinewright-app')
m.add('grant-fixed-at-store', [(P, 'if after.edit_consent != before.edit_consent {', 'if false {')], H+'grant_is_fixed', 'FOL_FIXED_STORE')
m.add('approvals-append-at-store', [(P, '!after.edit_approvals.starts_with(&before.edit_approvals)', 'false')], H+'approvals_are_append_only', 'FOL_FIXED_STORE')

if __name__ == '__main__':
    m.main()
