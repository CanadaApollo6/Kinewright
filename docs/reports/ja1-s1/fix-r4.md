# JA-1 S1 round-4 implementation dispositions

Worktree: `/home/riels/.t3/worktrees/Kinewright/t3-5e39b056`; branch `ja1/s1`.
Parent: `9cd2e710db370006a853596182a60ba239a9de9a`.

The lead's post-stop ruling supersedes the original eager-floor decision. A pending
Core barrier retains the protected entries and blocks Undo below them before destination
IO. Verified success commits against current Core history; definite failure or a confirmed
older/foreign root releases it. Unknown publication retains it until reconciliation.
No save-time snapshot is installed by reconciliation. A stale export refuses before destination IO.
Blocked Undo is not appended as an accepted command, so releasing the barrier cannot turn
that blocked attempt into a later replayed Undo of the protected prefix.

## Dispositions

| Finding | Disposition and witness |
| --- | --- |
| N1 | Fixed with one Core-owned revocable barrier across ordinary save, specialist save and first-save copy. `moving_manual_core_reconciles_and_saves`, `moving_specialist_core_reconciles_and_saves`, `first_copy_moving_core_reconciles_and_saves` accept an edit and Undo before own-root reconcile, preserve the current suffix, then save exact Core/durable equality. `first_failure_releases_every_original_undo_and_redo_then_retries` visits every original Undo/Redo target after a definite prepublication failure and retries. `older_root_releases_every_entry` checks release against an older root. `command_lands_mid_save_above_pending_floor` executes commands at the real B0 hook, checks retained entries and blocked Undo, and saves the new suffix. The portable Core witnesses also check current-history commit, one visible floor revision, release, restore fencing, a stale capture and refusal to reuse a settled barrier handle. |
| N2 | Fixed: a suffix that crosses below a candidate cut omits unsafe batch cost terms from the lower bound and reaches exact replay/fit. The production predicate is exercised by the small valid-cut set `{1,2,6,7}`, the 100-edit/30-undo/30-redo/5-edit case retaining 20 Undo steps at cut 85, and the seeded differential test. The old stand-in Core budget test is replaced. `prefilter_zero_undo_counters_cross_digit_boundaries` checks the exact counter-cost shortcut at 10, 100, 1,000 and 10,000; these pure prefilter tests are enabled on both supported platforms. |
| R4-1 / m1 | Fixed: placeholder removal uses twice the production charge of JSON null (144 bytes). `exact_cap_saves_and_one_byte_over_floors_one_more_step` saves at the exact decoded admission cap and, with one byte less allowance, selects the next floor; both saves reopen with exact history. Limits are unchanged. |
| m2 | Deferred: unverified-source media-ID reuse fails closed; content-qualified media synthesis remains app-wiring work. |
| m3 | Deferred: duplicate bare LUT IDs refuse at first-save manifest admission; producers must use the existing content-qualified spelling. |
| m4 | Deferred: bounded per-path cleanup diagnostics and fail-closed Identity handling remain unchanged. |
| m5 | Corrected the archived mutation claims in README: first-copy-own is a double-install red; persistent-GC fails at the status counter. No stronger archived continuation claim is made. |
| m6 / C1 | Fixed traceability: fresh predecessor identity records the pre-strip hash, strip version/arguments and the within-run clean-source build provenance; run.json source_head is explicitly its working directory's HEAD. The lead reports hosted Linux C1 closed at the parent; local controls are rerun independently. |

Save As copies frozen acknowledged history and has no live Core parameter. It retains
its existing refusal when a lower destination cap would require flooring that history;
there is no pending floor to install or commit in that path. Copy root reconciliation
uses the same pending-save settlement when first save supplies a live Core.

Differential seed: `0x4a41315331523401`; histories: **10,000**, mixing Do, Undo,
Redo and coalesced gesture boundaries. Its final source-bound restored run passed. This compares the
actual production predicate's chosen result with exhaustive floor plus actual admission;
it is not a separate imitation of the prefilter.

## Evidence and verification

Evidence root: `/home/riels/Projects/Personal/Kinewright/target/review/ja/s1-evidence/r4/`.
The mutation driver records the tested implementation/witness source hashes, test executable
hash, exact command and raw summary for each targeted red/restored pair. Compiler/setup
failures earn no runtime-red credit. The final runtime ledger is `mutations-final/results.json.gz`: **15 red/restored pairs**.
Every red reaches a runtime assertion and differs from the restored tree in exactly one
Rust implementation file. All restored source maps match the 265 current Rust files;
`mutation-audit.json.gz` records the assertion sites. The extra provenance pair keeps
all four predecessor controls successful while an omitted strip-version field makes its
traceability check fail; restoration passes. This is a traceability-contract pair, not a
persistence runtime red. Its controls exit 0 in both phases.

The earlier `gate/` run passed before the final performance refinement and is diagnostic
evidence only. `gate-final/` is the final-source gate. Its source maps, the restored runtime
mutation maps and the final F4 map all match the same 265 Rust files.

No push, self-review, new implementation worktree, GPU setting change or other-project
process action is authorized or performed. Native Windows execution, full slow CI, Kani,
physical storage faults, decoder-open certification and L6 release acceptance remain NOT RUN.
L6 comment latency was not remeasured; historical round-1 p50/p95 was 131.511/164.736 ms.

## F4 measurement

`measure-final-v3/` is the final-source run: five pairs, one new test process per workload,
with source/executable SHA-256 identities, compiler, whole-test maximum RSS, start/end
CPU/GPU annotations and five-second load samples. Each lane has a 600-second timeout.
The runner used `setsid nohup`; no own build or other test overlapped a timing window.
“Cold selection” is the first bounding call in that process before the injected copy
verification failure; it does not claim a cold filesystem cache. First-save retry timings
include the subsequent successful save. No measurement-only Rust source overlay is used.

| Workload | Cold selection median, ms (range) | First-save retry median, ms (range) | Whole-test max RSS median, KiB |
| --- | --- | --- | --- |
| 500 edits, 2,000 tracks, original encoding above 32 MiB; retains 20 batches | 783.868 (779.417–818.541) | 2076.469 (2043.219–2089.189) | 110580 |
| 65,537 no-op Undo batches; retains 26,673 batches | 186.090 (185.464–191.457) | 2074.574 (2053.478–2104.182) | 346296 |
| 100 edits / 30 Undo / 30 Redo / 5 edits; retains 20 Undo steps at cut 85 | 30.235 (29.855–32.289) | not a first-save workload | 23504 |

The round-3 archived cold medians were 784.211/254.188 ms and its retry medians were
3405.234/2419.945 ms. These are historical comparisons, not fresh reruns of the old binary.
The new cold medians remain within those numbers and below the one-second selection gate;
the full measured ranges are retained above rather than treating a median as a maximum.
Whole-test RSS is not a whole-process or live-history budget acceptance claim.

The first implementation exposed a timing regression. The final path carries the already
validated floor transition into the actor barrier, and commits by replaying only the current
retained suffix and checking its exact cursor/document. Barrier checks apply at actor command
ingress, not during private replay. The prefilter skips irrelevant batch terms once per cut;
for zero Undo counters it sums order-digit growth exactly by decimal thresholds. This keeps
the same exhaustive selection policy and unchanged caps. Earlier `measure*` runs are diagnostic,
not the final-source performance claim.

## Final gate

`gate-final/` exited 0: **18 outer commands, 16 passed and 2 explicitly NOT RUN** for native
MSVC prerequisites. The nested per-commit gate passed **46 commands**, with **33 raw test
summaries and 317 selected passes** (overlapping selections). Full Core targets passed
**792 tests**; full Project targets passed **215 tests**, with two existing ignores.
The default-environment workspace fast tier passed **3,770 tests** and its exact skip audit.
The three app-bridge ignores ran explicitly in the per-commit gate.

Workspace build, eval bins, no-default builds/checks, workspace/all-target Clippy, touched-file
rustfmt, both architecture scans, harness modules/integration, format/caller/lock/media pins,
receipt/branch/copy targets, standalone feature consumer, library-only test-util check,
classification lint, actionlint, shell syntax, Python AST parsing and frozen fixtures passed.
All raw summaries are retained in `raw-summaries.md.gz`, in addition to the individual logs.

Literal final lines:

```text
Sealed predecessor: /home/riels/.t3/worktrees/Kinewright/t3-5e39b056/target/ja-s1-predecessor/sealed/kinewright-ja-baseline
fast tier on linux skipped exactly the 50 manifest tests and 54 allowlisted ignores (ci/ignored-tests.txt), each observed, nothing else
slow-test manifest and markers agree: 50 tests, features kinewright-agent/slow-tests,kinewright-app/slow-tests,kinewright-media/slow-tests; 54 allowlist entries well-formed
Linux stricter gate complete. Windows execution and full slow CI remain NOT RUN.
```

Representative literal witness summaries from the final gate:

```text
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 64 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 101.00s
```

The installed MSVC target compiled Core and its tests. App/Project cross-checks stopped
in native dependencies (MSVC headers / FFmpeg / Whisper / aws-lc prerequisites), not in a
completed Rust cfg check; no success is claimed for those two lanes. No toolchain was
installed. Native Windows execution, full slow CI and Kani remain NOT RUN.

The lead ruling is the only change to the approved floor mechanism: barrier, commit and
release replace irrevocable eager trim. The first-save selection carries the validated Core
floor result rather than recomputing it; no second store, writer, collector or serializer
was introduced. Accepted minor deferrals are recorded above and in README.

## Inherited controls and lane limits

| Baseline | Revised property and positive control retained in this gate |
| --- | --- |
| R1 history loss | `history_loss_fixture_revised_preserves_one_entry`: changed writer/reopen bytes remain the control, and exact retained history has one entry. The fresh sealed predecessor retains its literal history red. |
| R2 parent sync | `strict_io_properties_revised_use_actual_syncs_and_no_fallback`: the paired fixture reaches real sync calls after a successful changed-byte/refusal control. |
| R3 fallback | The same paired witness reaches the armed production rename refusal with independent attempted bytes, preserves the prior root and observes no fallback. The original contradictory legacy-IO fixture is unchanged; no whole-original-fixture green is claimed. |
| R4 retry | `reply_retry_fixture_revised_has_one_durable_entry`: matching original/retry receipt identities and one undo entry follow an accepted-edit control. The predecessor still exhibits the literal duplicate-entry red. |
| Comment binding | `comment_receipt_fixture_revised_reconciles_exact_original_result` retains original record/receipt identities through retry and retirement with exact byte controls. The separate missing-binding baseline keeps its persisted-byte control; the original clean commit-4 seal is archived evidence, not a fresh old-source run. |

Portable Core/model and production-prefilter witnesses run locally; the pure prefilter
witnesses are also enabled for native Windows CI. Real bundle/copy/receipt/namespace,
process-death and durable-filesystem witnesses run in the Linux lane. Modeled storage loss
and process death are distinct from physical storage-fault certification. Core's installed
MSVC target is compile-only. Native Windows App/Project execution, their unavailable native
cross-build prerequisites, G1-W/S6 durability, full slow CI, Kani and real decoder-open
certification remain NOT RUN. No S2 host/producer fencing or broader PF1 acceptance is claimed.

## Landing hygiene

The exact F4 executable is preserved as a gzip seal named with its tested parent and
SHA-256 (`60347b2aa0c0039e4fb01bf862be251104984c2c34f1f029454dcbacd8fa8814`),
alongside its source map. Final logs/metadata are compressed. Removed 36 fixture-owned
AU6 temporary items from the two recorded full-fast runs after checking their process IDs
and creation times against those runs. Removed this run's regenerated `target/debug`;
pre-existing temporary roots and predecessor seals remain. Other projects' processes and
other `/tmp` items were untouched. No push or self-review was run.
