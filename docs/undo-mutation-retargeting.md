# Native history mutation retargeting

The new history has no Save-side floor, mutable history cursor, suffix replay
proof or executor save ticket. The catalogues retain every original case name
and its explicit status; obsolete mechanisms have a reason and native replacement
witnesses. Independent namespace, physical storage, graph, receipt and retained-work
requirements are preserved.

| Catalogue | Original cases | Retained edits | Retargeted edits | Retired mechanisms |
|---|---:|---:|---:|---:|
| C4 | 48 | 34 | 8 | 6 |
| C5 | 68 | 40 | 17 | 11 |
| C6 | 126 | 84 | 29 | 13 |
| C2 | 31 | 8 | 12 | 11 |

The central [catalogue](../scripts/undo-retargeted-mutations.json) pins each
original name set and count. C12 aliases use the same native edits; the mixed
older runners remove only the mechanisms listed in the
[retirement ledger](undo-obsolete-mutation-entries.json). The old S1 history
runner and round-four floor-only runner are deleted.

All 29 retargeted C6 cases ran as compiled mutations, each with one failing
runtime witness and one passing sealed original control. The five additional
C4 edits also ran. Shared C4/C5 edits are qualified by the corresponding C6
witness, rather than being counted as additional executions of every alias.
The unchanged 84 retained C6 edits were checked for unique applicable anchors;
their complete historical corpus was not rerun in this checkpoint.

The broad repository JSON audit finds 490 active anchors, 56 explicitly
superseded cases and 35 zero-match hooks across 18 catalogues, with no ambiguous
multi-match anchor. Those 35 stale hooks predate this switch; their names and
before/after classifications are retained in the proof. This is not a claim
that the broad audit is green. All 291 active or replacement witness references
in C2/C4/C5/C6 occur in the final compiled fast inventory; presence alone is not
counted as a mutation execution.

The execution checks distinguish build prerequisites, runtime failures and
survivors. Compilation has no test deadline, while execution takes the existing
600-second deadline after acquiring the shared lock. Three Python fault tests
assert that failed builds and surviving mutations cannot earn a runtime red,
that both restore the source, and that an original case cannot disappear through
retirement. No timing threshold, role allowance or fixture was raised.

Actual execution corrected four vacuous hooks. Ordinary Save has no floor ticket,
so exact settlement now targets the retained prepared candidate update. A frozen
rescue draft refuses a raw gesture close; its mutation must resume that independent
draft and actually close it. A specialist base is immutable and rooted at the
initial fork, so omission must happen at first admission. Finally, the old
fresh-open boundary witness now tests pool admission: a new proposed-root test
refuses an unavailable graph dependency before staging and proves the saved file
and complete Core unchanged.

Seven distinct C2 edits also ran: six fence/authority edits and the absent
candidate outcome. The latter now targets real prepared-update settlement,
rather than an unused old ticket-query return. Its permanent regression proves
the absent root does not commit, the source work survives, and an exact retry
applies once. C2 aliases and its committed-settlement edit are not counted as
extra executions beyond those and the C6 native settlement witnesses.

The portable actor fence test replaces the removed old actor's sealing proof:
frozen and sealed actors refuse raw Do/batch/Undo/Redo, explicit close and raw
installation; a sealed attachment refuses another cut, resume or installation.
The complete owner record and accepted change id remain unchanged. Durable
proposed-root and candidate publication checks stay Linux-only.

## C6 runtime witnesses

| Mutation | Exact witness | Outcome |
|---|---|---|
| repeat-existing-target-head-not-revalidated | `ja1_attachment::tests::recovery_cases::w13_existing_target_retry_requires_current_membership_and_head` | red / original green |
| repeat-resurrect-missing-existing-target | `ja1_attachment::tests::recovery_cases::w13_existing_target_retry_requires_current_membership_and_head` | red / original green |
| repeat-unregistered-fork_durable_branch | `durable_branch::tests::w13_registered_session_blocks_branch_retirement` | red / original green |
| repeat-unregistered-reopen_durable_branch | `durable_branch::tests::w13_registered_session_blocks_branch_retirement` | red / original green |
| repeat-resume-after-frozen-publication-failure | `ja1_attachment::tests::recovery_cases::w13_frozen_cut_rescue_at_both_unknown_publications_and_late_a_isolation` | red / original green |
| repeat-rescue-shares-original-core | `ja1_attachment::tests::rescue_cases::w01_first_save_rescue_is_independent` | red / original green |
| repeat-rotation-forgets-ack-record | `ja1_attachment::tests::rescue_cases::w13_one_level_rescue_recopy_and_explicit_ack_retirement` | red / original green |
| repeat-silent-recovery-omission | `ja1_attachment::tests::rescue_cases::w13_omission_only_precedes_trim` | red / original green |
| repeat-prune-independent-root-for-rescue | `ja1_attachment::tests::rescue_cases::w13_trim_then_separate_omission_recomputes_closure` | red / original green |
| repeat-active-b-new-unknown-uses-old-copy-status | `ja1_attachment::tests::rescue_cases::w13_recovery_on_active_rescue_copy_settles_the_new_attempt` | red / original green |
| repeat-exact-ticket-settlement-omitted-a | `h2_candidate_unknown_publication_reconciles_only_its_registered_core` | red / original green |
| repeat-exact-ticket-settlement-omitted-b | `editing_session::durable::fu2_tests::fu2_candidate_settlement_recovers_retired_receipt_without_masking_commit` | red / original green |
| repeat-published-ack-has-no-dedicated-edge | `ja1_attachment::tests::rescue_cases::w13_one_level_rescue_recopy_and_explicit_ack_retirement` | red / original green |
| repeat-preliminary-save-unknown-resumes-o | `ja1_attachment::tests::recovery_cases::w13_preliminary_save_faults_keep_exact_cut_and_do_not_select` | red / original green |
| repeat-rescue-closes-open-gesture | `ja1_attachment::tests::rescue_cases::w04_oldest_first_trim_preserves_redo_gesture_and_inputs` | red / original green |
| repeat-specialist-base-not-in-branch-publication | `durable_branch::tests::w13_registered_session_keeps_original_namespace_after_rescue` | red / original green |
| repeat-registered-close-warning-omitted | `durable_branch::tests::w13_registered_session_keeps_original_namespace_after_rescue` | red / original green |
| graph-record-ack-edge-omitted | `durable::w13_ack_and_two_predecessors_reopen_rotate_and_recover` | red / original green |
| graph-record-digest-only-ack | `durable::w14_raw_role_schema_api_and_checkpoint_edges_are_required` | red / original green |
| graph-b-activation-rewrites-origin-inputs | `durable::w13_checkpoint_restore_uses_b_identity_without_rewriting_inputs` | red / original green |
| destination-save-as-availability-omitted | `ja1_attachment::tests::w13_actual_destination_controls_and_background_freeze` | red / original green |
| destination-open-project-availability-omitted | `ja1_attachment::tests::w13_actual_destination_controls_and_background_freeze` | red / original green |
| destination-open-dialog-availability-omitted | `ja1_attachment::tests::w13_actual_destination_controls_and_background_freeze` | red / original green |
| destination-new-project-availability-omitted | `ja1_attachment::tests::w13_actual_destination_controls_and_background_freeze` | red / original green |
| destination-direct-write-availability-omitted | `ja1_attachment::tests::w13_actual_destination_controls_and_background_freeze` | red / original green |
| producer-ui-freeze-omitted | `ja1_attachment::tests::w13_actual_destination_controls_and_background_freeze` | red / original green |
| app-admission-after-store-mutation | `ja1_attachment::tests::w14_unsupported_rescue_refuses_before_session_mutation` | red / original green |
| slow-agent-owner-dropped | `rescue_reopen_recopy_retention_stress` | red / original green |
| slow-app-owner-dropped | `ja1_attachment::tests::rescue_complete_app_lifecycle_stress` | red / original green |

## Restored-source verification

The checkpoint passes 5,463 workspace fast tests (283 declared ignores),
5,587 Linux workspace slow tests (159 declared ignores), and 41 library plus
13 unpaid opt-in TE1 measurement tests. All 16 long session partitions pass:
64 seeds and 65,536 attempts. Workspace and feature clippy, fmt, the workspace
build, three separate shipping builds, slow-test lint, three mutation-runner
fault tests, 26 CI-planner self-tests and 52 slow-harness self-tests pass.
The final supervised gates record no signals. The initial Clippy declaration-order
failure is retained; moving a local test type before the statements changes no
mutation edit or assertion. No timing or new performance result is claimed.

The two new store-backed regressions are Linux-only. The actor fencing table
is portable. The native MSVC prerequisites remain unavailable as recorded in
[the deletion checkpoint](undo-legacy-deletion.md); this checkpoint's source
cfg review is not Windows execution qualification.

[Mutation, catalogue and gate evidence](measurements/undo-mutation-retargeting-proof.json.gz)
includes the original compiler/profile and binary hashes, exact red/green
outcomes, prior failed hooks, restored-source gate commands, complete logs,
compiled-witness audit and final diff. No build or test remains running at
this commit. The seven switch mutations and maximal full-tail qualification
follow this checkpoint.
