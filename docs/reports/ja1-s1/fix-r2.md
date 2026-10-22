# JA-1 S1 fix round 2

Worktree `/home/riels/.t3/worktrees/Kinewright/t3-5e39b056`, branch `ja1/s1`;
follow-up to `099fdd0b47492b45984f89be2916057ebadd3f13`. One follow-up commit,
with no push and no independent review performed by this implementer.

## Dispositions

Project witnesses below live in `tests/support/ja_s1_fix_r2.rs`, included as
`round_two` in `ja_s1_storage`, unless another crate/target is named.

| Finding | Disposition and witness |
| --- | --- |
| R2-1 | Fixed: `floor_retains_lut_context_across_multi_edit_save_reuse_and_retirement`. The post-floor baseline gets a sealed owned-input manifest for exactly the retained resource identities. The multi-edit save, A/B ID reuse, reopen, predecessor retirement and exact LUT bytes are checked. `wrong_lut_length_refuses_even_when_sha_and_entry_id_match` independently checks length with the correct digest and entry ID. |
| R2-2 | Fixed: `ordinary_save_installs_floor_preserving_redo_gesture_and_reopen`, `first_save_installs_the_same_floor_before_handoff`, `save_as_preserves_the_published_floor_and_refuses_silent_copy_trim`, and `no_edit_comment_repairs_the_original_core_cut_after_floor_reconciliation`. Save and branch-save now receive the live Core and install the exact published floor through Core's ordered, expected-before transition before returning success. First-save reconciliation retains that same frozen cut. Save As preserves acknowledged histories exactly and refuses a narrower destination allowance before copy mutation. Full history, redo order, the open gesture and reopen equality are asserted. |
| R2-3 | Fixed: agent `ja_s1_branches::floored_specialist_reopens_original_full_delta_and_merges_once`. Chosen representation: the existing specialist record retains its original base and full proposal independently of its bounded undo checkpoint. Reopen replays the prefix into the checkpoint baseline and the retained delta into the checkpoint document. Further edit/Undo/persist, reopen, full-delta preservation and one-entry/one-receipt merge retry are checked. |
| R2-4 | Fixed: `over_quota_reopen_keeps_inventory_only_owned_graph_until_retirement`. Writer reopen extends verified accounting with the durable inventory's content closure, using the recovery walk's existing exclusion of archived automatic inventories. A 16 MiB owned object absent from the selected graph survives reopen under an 8 MiB metadata ceiling; the inventory is unchanged and no unlink occurs. Ordinary maintenance then durably retires the inventory before unlink. The existing dropped-write retirement model checks its crash cuts separately. |
| N1 | Fixed in source and portable fixture-root witness; same as C2. Native Windows execution remains a separate lane. |
| N2 | Fixed; same as R2-2, including the real app/session witness `app::in2b_tests::app_session_undo_stops_at_saved_floor_and_next_save_keeps_edits`. Undo is driven through `app.undo()`, past the saved floor; saving and redoing every retained edit succeed without data loss. This exercises the existing app/session Core, not the deferred S2 Studio attachment. |
| N3 | Deferred hosted root-cause closure; bounded diagnostics fixed; same as C1. Expected exit 1 is unchanged; exit 2 remains a failed prerequisite. |
| N4 | Deferred: valid floor cuts depend on Undo/Redo and gesture boundaries and are not a monotonic range; this follow-up preserves the existing checked search rather than assuming an unproven binary-search predicate. No improved cold-save bound or performance closure is claimed. |
| N5 | Fixed: `newly_admitted_unpinned_input_survives_previous_ack_cleanup` and `ja_s1_copy::save_as_contention_preserves_source_and_destination_graphs`. Collection only considers known objects present at the previous acknowledgment; a new manifest and its bytes survive even after their admission guards are dropped. |
| N6 | Fixed: `persistent_collection_failure_is_recorded_and_does_not_block_saves`. Ordinary unlink failures are retained in `collection_error()` and retried; three consecutive injected failures still permit new saved edits. Uncertain namespace/publication state remains fenced. Successful maintenance clears the recorded failure. |
| C1 | Deferred hosted root-cause closure; bounded diagnostics implemented and locally exercised. The actual reason remains undetermined until a hosted rerun emits its summary. See the diagnosis below. |
| C2 | Fixed: every discovered durable scratch root built with `../../target` now uses the shared `workspace_target_dir()` helper. Media's portable `test_support::workspace_target_dir_is_absolute_without_parent_components` checks the real helper. The Windows app fixture still requires literal `durability_unsupported`, an empty mutation trace, exact sidecar/tree bytes and unchanged session history. No string assertion was loosened. |
| A1 (partly closed r1) | Fixed by R2-1 and the independent wrong-length witness. |
| O2 (partly closed r1) | Fixed correctness by R2-2/R2-3 and the app/session witness; N4's optimization is deferred. The new undo-at-floor case also exposed and fixed lineage replay's rejection of Core's accepted no-op Undo/Redo; cursor-count validation remains. |
| O17 (partly closed r1) | Fixed by R2-4; genuine unrelated over-quota orphans remain collectible and inventory-owned objects are now typed, verified and charged before residual deletion. |
| A7 (partly closed r1) | Fixed implementation: cache/coalescing and deferred maintenance behavior retained. L6's latency target remains deferred to the lead; historical timing is not relabelled as a fresh run. |
| O7 (partly closed r1) | Fixed implementation with the same cache/coalescing and maintenance evidence as A7; L6 remains deferred with historical timing only. |
| A5/O20/O21/O28 (source-only Windows r1 closure) | Fixed in source: corrections retained, normalized fixture roots added, and installed-target checks attempted separately. Native Windows tests/clippy require the lead's hosted lane. |

O11 and O26 retain their lead-authorized round-1 deferrals; neither is reclassified
as fixed here. No S0 fixture/oracle bytes or storage caps were raised.

## C1 diagnosis and next hosted run

The original Linux job's log confirms `format-corpus=0`, `history-loss=1`, and
`reply-retry=2`, but contains no `missing_requirements`. GitHub retained zero
artifacts for run 39699473933, so the missing control summary cannot be recovered
from that run. There is no evidence establishing a timeout, spawn failure or report quota as its actual cause.

The predecessor script now prints each available control summary's `disposition`
and `missing_requirements` after an unexpected exit. It reads at most 1 MiB per
summary, prints at most 2,048 characters of the reason per control, and retains
the original failure status. A synthetic `not_run` control with a long cleanup
reason verifies both the fields and truncation. The rerunnable
`scripts/ja-s1-control-diagnostics-check.py` also records a missing-reason
omission mutation failing that literal field assertion, followed by a restored
pass; neither run claims the hosted control itself was reproduced. The next hosted execution must
retain exit 1 for the expected behavioral red; exit 2 is never accepted as pass.

The fresh local controls returned `0, 1, 1, 1` for format, history, reply/retry
and legacy IO respectively, all with `disposition=observed` and no missing
requirements. The reply/retry output contained 49 files totaling 75,329 bytes
against its 16 MiB fixture output quota. The final gate evidence follows below. The frozen control has no declared requirements and no `server_start` action.
Its workers use piped stdio (`076a96c:ja_harness/controller.rs:89`), and the
baseline starts with `server=None` (`076a96c:ja_harness/baseline.rs:31`); no TCP
server is requested by this control. Passing locally rules out a deterministic
fixture/protocol/quota defect and a permanent local spawn restriction for this
run, not an intermittent hosted spawn/cleanup limit or slow-runner deadline. The lead must push and inspect
the bounded reason on a failing hosted rerun before claiming root-cause closure.

## Evidence and validation

Raw logs and mutation ledgers are stored under
`/home/riels/Projects/Personal/Kinewright/target/review/ja/s1-evidence/r2/` and
compressed. The rerunnable mutation driver is
`scripts/ja-s1-fix-r2-mutations.py`; it reuses the round-1 driver, verifies the 17
storage-witness mutations, and adds the round-2 cases. Each log binds the actual
source, baseline source, witness sources, executable, command and memory check.
Only behavioral test failures earn mutation-red credit.

A preliminary matrix completed 32 Rust red/restored pairs and the C1 diagnostic
omission pair. Those Rust runs precede the copy-rollback correction and are
retained as preliminary evidence only. The final matrix is recorded separately
in `mutations-final/`, including the added exact-copy rollback regression.

The final matrix is re-recorded after the full copy target exposed a rollback
interaction with N5. Rollback now collects only this attempt's new objects after
restoring the prior root/inventory, preserves every pre-copy known/unclassified
object and temporary, and prevents owner Drop from running ordinary GC on that
restored inventory. The existing exact-destination copy-failure witness now
rotates the destination first, so its conservative inventory is covered too.
A separate rollback mutation is added to the final matrix.

The final matrix passed **33 Rust red/restored pairs** (66 executions): 17
round-1 pairs and 16 round-2/rollback pairs. All 13 new Rust witnesses and the
strengthened existing copy-failure witness are included. Separate Undo and Redo
omission mutations target Core's accepted no-ops at the history floor.

`mutation-source-audit.json.gz` verifies that every restored run matches the
final 263-file Rust snapshot and every red changes only its named implementation
file. Every round-1 run records the committed-witness SHA-256
`27da8d73f97fd04b2113faa50b83280200c9bc51bf93274895a6b15920cdcd17`
for `ja_s1_fix_r1.rs`. The post-landing source/commit binding is kept beside the audit.
The C1 diagnostic omission pair has its separate bounded-field assertion logs.

The final `ja_s1` target freshly retains the earlier S0-to-revised flips; it
does not introduce a new baseline seal:

| Baseline property | Revised witness and positive control |
| --- | --- |
| R1 history loss | `history_loss_fixture_revised_preserves_one_entry`: all unchanged-fixture assertions pass, including `history-survives-reopen`; actual bundle writer bytes match. |
| R2/R3 strict IO | `paired_io_baseline_has_successful_refusal_controls_and_literal_reds` keeps four successful controls and the three literal baseline reds. `strict_io_protocol_separates_legacy_control_from_bundle_result` and `strict_io_properties_revised_use_actual_syncs_and_no_fallback` pass the paired Linux strict-writer properties, with the injected refusal reached, exact root preserved and no in-place fallback. The original conflicting IO oracle remains unchanged. |
| R4 reply retry | `reply_retry_fixture_revised_has_one_durable_entry`: original/retried receipt equality, durable receipt backend, and exactly one undo entry after retry. |
| Comment receipt | `comment_receipt_fixture_revised_reconciles_exact_original_result`: exact original record/receipt, zero mutation on reconciliation and retired-unresolved refusal, plus unchanged commit-4 seal/input/oracle identities. Its baseline is historical, not freshly resealed. |

Final local validation passed on the 263-file Rust snapshot recorded in
[`r2-validation.json`](r2-validation.json). All 16 outer gate headers match that
snapshot; all 37 modified/new code files match the initial per-commit gate log.
Documentation was finalized afterward. `final-source-audit.json.gz` records the
audit; `landed-source-binding.json.gz` binds it to the landed commit.

The ordered per-commit gate completed 46 commands and
33 raw test summaries: 295 selected passes and five
classified ignores, including overlaps between the required selections.
Workspace/eval/no-default builds, checks, heap-example check, touched-file
formatting, both normal/eval architecture scans, all native app bridges,
harness/format/retention/copy/receipt/branch/media selections and classification
passed. Full crate totals below sum distinct target summaries within each crate;
workspace totals are a separate execution, not added to those crate totals.

| Gate | Exit | Passed / failed / ignored |
| --- | ---: | --- |
| predecessor | 0 | passed |
| per-commit | 0 | 295 / 0 / 5 |
| windows-core | 0 | passed |
| windows-app | 101 | NOT RUN: native MSVC dependency prerequisite |
| windows-project | 101 | NOT RUN: native MSVC dependency prerequisite |
| kinewright-core | 0 | 790 / 0 / 1 |
| kinewright-project | 0 | 195 / 0 / 2 |
| kinewright-agent | 0 | 801 / 0 / 26 |
| kinewright-app | 0 | 798 / 0 / 5 |
| kinewright-media | 0 | 1164 / 0 / 70 |
| workspace-fast | 0 | 3748 / 0 / 104 |
| classification | 0 | passed |
| clippy-final | 0 | passed |
| actionlint | 0 | passed |
| shell-syntax | 0 | passed |
| frozen-fixtures | 0 | passed |

`cargo test -p kinewright-media --features test-util -- --test-threads=1` used
one media test thread. Other Cargo tests used four; every Cargo command used
four build jobs, a fresh 6 GiB start prerequisite, the sourced FFmpeg environment
and `nice -n 10` under the nice-19 Python driver. No heavy jobs overlapped.

The full fast command was
`CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 nice -n 10 python3 -I scripts/slow_tests.py fast`
with `TMPDIR` unset and inherited Python niceness 19. Exit 0 verifies all 50
manifest slow-test skips and 54 allowlisted ignores; no additional skip is
accepted. Full slow tier, native Windows execution/clippy, S2 GUI/host attachment,
L4 decoder-open certification and physical device/power-fault certification are
NOT RUN. The portable dropped-write model and Linux process-death witnesses
passed in their existing targets; those are separate evidence classes.

Final predecessor control log lines (the outer script also exited 0):

```text
running 1 test
test pinned_max2_format_controls ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

{"exit_code":0,"output":"/home/riels/Projects/Personal/Kinewright/target/review/ja/s1-evidence/r2/final-gate-3/predecessor-controls/controls-6VqHjwgl/format-corpus","protocol":"kinewright.ja-harness","protocol_version":1}
{"exit_code":1,"output":"/home/riels/Projects/Personal/Kinewright/target/review/ja/s1-evidence/r2/final-gate-3/predecessor-controls/controls-6VqHjwgl/history-loss","protocol":"kinewright.ja-harness","protocol_version":1}
{"exit_code":1,"output":"/home/riels/Projects/Personal/Kinewright/target/review/ja/s1-evidence/r2/final-gate-3/predecessor-controls/controls-6VqHjwgl/reply-retry","protocol":"kinewright.ja-harness","protocol_version":1}
{"exit_code":1,"output":"/home/riels/Projects/Personal/Kinewright/target/review/ja/s1-evidence/r2/final-gate-3/predecessor-controls/controls-6VqHjwgl/legacy-io","protocol":"kinewright.ja-harness","protocol_version":1}
Sealed predecessor: /home/riels/.t3/worktrees/Kinewright/t3-5e39b056/target/ja-s1-predecessor/sealed/kinewright-ja-baseline
```

Final workspace fast lines (exit 0):

```text

   Doc-tests kinewright_media

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests kinewright_project

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

fast tier on linux skipped exactly the 50 manifest tests and 54 allowlisted ignores (ci/ignored-tests.txt), each observed, nothing else
```

Raw per-commit `test result:` lines, in execution order:

```text
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 649 filtered out; finished in 0.02s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 802 filtered out; finished in 0.02s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 546 filtered out; finished in 0.01s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 802 filtered out; finished in 0.02s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 39.20s
test result: ok. 44 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 101.00s
test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 629 filtered out; finished in 154.67s
test result: ok. 8 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out; finished in 28.42s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 10 filtered out; finished in 34.97s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 10 filtered out; finished in 16.99s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 10 filtered out; finished in 35.26s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 308 filtered out; finished in 0.00s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 647 filtered out; finished in 0.00s
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 127 filtered out; finished in 0.62s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 311 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 132 filtered out; finished in 9.09s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 46 filtered out; finished in 0.00s
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 798 filtered out; finished in 0.39s
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 1188 filtered out; finished in 2.94s
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 798 filtered out; finished in 0.12s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 14 filtered out; finished in 1.13s
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 127 filtered out; finished in 0.01s
test result: ok. 53 passed; 0 failed; 1 ignored; 0 measured; 84 filtered out; finished in 4.94s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 787 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 802 filtered out; finished in 1.62s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 46 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1197 filtered out; finished in 0.86s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 46 filtered out; finished in 0.23s
test result: ok. 42 passed; 0 failed; 1 ignored; 0 measured; 760 filtered out; finished in 0.03s
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 55.61s
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.12s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 8.05s
```

Every full-crate and workspace raw `test result:` line, every outer gate command,
and the complete per-commit command inventory are preserved in the small
validation JSON beside this report. Complete execution logs remain gzipped
outside Git under `s1-evidence/r2/final-gate-3/`.

After the gate, cleanup removed 36 owned AU6 temporary items from the two final
media test processes and the four build directories created by this run
(`target/debug`, `target/tmp`, `target/x86_64-pc-windows-msvc`, and
`target/ja-s1-predecessor/build`). No `/tmp/kinewright-*` remains. The sealed
predecessor, its checkout and all review evidence remain available; raw logs
are gzipped. `cleanup.json.gz` records the exact removed paths and temporary
file identities. No other projects' processes or temporary files were touched.

Three earlier gate attempts failed and are retained separately: `gate/` exposed
the inherited cleanup assertion that conflicted with N6; `final-gate/` exposed
the AU6 whole-source OS-guard inventory; `final-gate-2/` exposed the exact-copy
rollback interaction with N5. The integration adjustments below identify each
correction. These failed attempts are not credited as successful gates; the
final complete run is `final-gate-3/`.

The installed `x86_64-pc-windows-msvc` target passed Core's `cargo check --lib
--tests --features test-util`. The app and project test checks were both
attempted and exited 101 in native build prerequisites: app at FFmpeg/aws-lc,
project at FFmpeg/Whisper/aws-lc. These are **NOT RUN** Rust cfg/test lanes,
not passing compile checks. Native Windows execution and Windows clippy remain
NOT RUN; no toolchains or native cross-build dependencies were installed.

## L6

No fresh latency or allocator measurement was run in round 2. The round-1 paired
release results remain historical: median-run comment p50 131.511 ms, median-run
p95 164.736 ms and 16 median sync calls per comment after the round-1 changes.
They are not current-head measurements. The 250 ms target is unchanged, and L6
remains open for the lead. No new latency improvement or regression is claimed.

## Integration adjustments

The inherited retirement witness now checks the actual injected failure trace.
Definite cleanup failures must allow a save under N6; uncertain inventory
replacement still must refuse. Both saved/restored history and physical accounting
remain checked (`tests/support/ja_s1_retention.rs`).
The adjusted assertion is at
`crates/kinewright-project/tests/support/ja_s1_retention.rs:263`; the recorded
cleanup result is owned by `crates/kinewright-project/src/bundle.rs:1047`.

The Linux app/session witness is included from `src/ja_s1_app_tests.rs` inside
`app::in2b_tests`. The existing AU6 source inventory forbids an OS guard anywhere
in `app.rs`, including unrelated new storage tests. Moving this storage-only
witness preserves its native Linux lane without changing the AU6 rule or any of
its tests. The AU6 inventory and the new app witness both passed after the move;
the final gate also reruns both architecture scans.
The inclusion point is `crates/kinewright-app/src/app.rs:10882`.

N5's acknowledgment cutoff also applies to ordinary collection during owner
Drop. Failed copies instead need to remove only their newly staged objects,
without changing the restored conservative inventory. The existing rollback
path now calls the same collector with staging-only eligibility
(`crates/kinewright-project/src/store.rs:2419`, `:2425`). This preserves the
single collection owner and is covered by exact destination bytes after four
prior saves, plus the separate rollback mutation.
