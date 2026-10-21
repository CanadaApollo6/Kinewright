# S0 final-head review fixes

Parent: `1c055b87bf9a616156069fee27c32921384c1e53` (S0-5).
One follow-up commit; none of S0-1 through S0-5 is rewritten.
All twelve lead decisions are accepted; no finding is rejected.

1. **Unexpected controller exit:** Unix observes the leader with
   `waitid(WNOWAIT)` and retains it through bounded group termination. Linux
   reaps adopted fixed-role descendants before the leader; the supervisor records
   cleanup and synthesizes NOT RUN even when both the report and exit code are
   absent. Witness: `unexpected_controller_exit_reaps_a_recorded_stopped_worker_before_return`.
   The worker is recorded and SIGSTOPped, so EOF cannot satisfy cleanup.
2. **App bridge:** validate the completed inner report, its fixture/executable
   identities, classification and evidence hashes independently of libtest exit
   101. Import assertions/observations/pending rows once, with rebased references.
   Native witnesses: `native_app_bridge_retains_exercised_control_and_property_failures`
   (exit 1 for both failed control and baseline property), and
   `native_app_bridge_keeps_inner_setup_failure_as_not_run_exit_two` (missing
   gesture setup, exit 2). The failed assertion is `actual-headless-callback-ran`;
   its actual value is true and the independently supplied oracle is false.
   The successful `coalesced-gesture-one-undo` control precedes the property red.
3. **Deadline/exit race:** accept ESRCH, retain ownership without prematurely
   reaping the leader, and route termination errors through the receipt/summary
   path. Witness: `ja_harness::supervision::tests::deadline_exit_race_accepts_esrch_and_preserves_permission_errors`.
   ESRCH is injected at the kernel-result boundary deterministically. A timed
   scheduler interleaving would be unreliable; holding the waitable leader also
   removes the original process-group disappearance window. This is not a claim
   of a deterministically scheduled native race.
4. **Cleanup budget:** derive the phase deadline from all fixed-role media and
   reap allowances plus report-sealing headroom. Share the actual media allowance
   with baseline, decode and App teardown and admit the resulting maximum phase
   budget. Witness: `ja_harness::tests::cleanup_deadline_outlives_all_fixed_role_teardown_allowances`.
5. **Control disposition:** use top-level `control_failed` when a selected
   control fails, preserving the frozen exit codes. Witness:
   `ja_harness::tests::failed_positive_control_has_control_failed_summary_and_exit_one`.
6. **R2 vocabulary:** the fixture's `directory-sync-trace` oracle kind, the
   observer and README name the coupling. S1 must emit `directory_sync` at the
   exact parent-directory sync call site. The scalar oracle and red are unchanged;
   existing witness: `ja_harness::tests::current_io_control_precedes_missing_directory_sync_assertion`.
7. **Shipping configuration:** README explicitly names only
   `cargo build --package kinewright-app --release` as shipping. The separate
   package build in the gate checks the dev feature graph. Workspace feature
   unification does not identify a shipping release artifact.
8. **Source manifest:** resolve the repository root once, check every git exit,
   run git there and record HEAD plus a binary-diff hash. Each fixture no longer
   rehashes the tree. Witness:
   `native_source_identity_is_repository_wide_from_root_and_crate_cwd`.
9. **Participant errors:** call `finish(false, report)` before returning refused
   kill/inventory/exit-request errors. Witness:
   `refused_participant_kill_records_the_graceful_exit_and_reap`.
10. **Path admission:** validate all PathArgs and DecodeArgs before execution,
    including locks, headless save, media export/decode; keep output-path rechecks.
    Witness: `ja_harness::tests::every_path_argument_is_refused_at_admission_before_any_action`.
11. **Gate catalog:** cache the embedded parsed catalog with `OnceLock`; its
    existing pending rows and allocation behavior are retained.
12. **Teardown panic:** Drop waits without asserting; each wrapped pin explicitly
    calls `finish(self)` for every engine. Witness:
    `teardown_observation_failure_panics_only_in_explicit_finish` plus the three
    retained generated-media regression pins.

The optional eval-only rustix direct dependency reuses the already locked 1.1.4
package, avoiding unsafe code in this workspace. Fixed-role supervision supplies
no production guardian, host-death/resume semantics or Windows certification.

The evidence archive distinguishes the initial preflight failures (a missing
oracle in the new test and missing crash-summary synthesis) from accepted
mutation reds. Neither preflight failure receives mutation or baseline credit.
Historical S0 archive bytes and source identities are retained unchanged.

All 48 Studio gate rows remain pending with the owners in `gate-observers.json`.
Windows durability/guardian containment, provider/modalities, private practice,
independent editorial labels, physical-display/human review, PF1 H-T/G14,
decoder-bound inputs, and fast/slow CI remain with their existing stage/lead
owners. No release-profile binary or full workspace test suite is claimed here.
No push or independent review round is part of this follow-up.

Exact commands, source/binary/log hashes, raw summaries and final-HEAD reruns are
sealed in the separate successor archive named in README. Its external manifest
binds the archive after final-HEAD runs; no historical report is relabelled.

Targeted mutation verification completed: **10 runtime reds and 10 restored
passes**. Each selected mutation exited 101 with exactly one failed test; each
restored run exited 0 with exactly one passed test. Every original source hash
was rechecked after all pairs completed. Cases: unexpected-controller-exit,
app-bridge-classification, app-bridge-setup, deadline-exit-race, cleanup-budget,
control-summary, source-root, participant-error-exit, path-admission and
teardown-drop. Exact raw summaries and failed assertions are in each sealed
case's mutation/restored test logs and receipts. These pairs identify the parent
HEAD plus their exact source mutation; they are not final-HEAD reruns.
