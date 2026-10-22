# JA-1 S1 round-3 dispositions

Parent: `8d53bf4ca151b9358faf04bea096825ba9933185`. Worktree:
`/home/riels/.t3/worktrees/Kinewright/t3-5e39b056`, branch `ja1/s1`.
The round-3 brief controls this follow-up. No push or independent review was run.

The required local gate passed. The final Rust source maps match all 17 outer gate logs and all 10 restored Rust mutation runs. Native Windows execution, MSVC App/Project completion and hosted CI remain NOT RUN; details follow.

## Findings

| Finding | Disposition and witness |
| --- | --- |
| R3-1 | Fixed. First save bounds the exported history before encoding or sizing the copy reservation, while keeping the original Core cut for installation. `round_three::first_save_floors_history_above_default_encoded_limit` crosses 32 MiB with default limits; publication, exact Core/durable/reopened equality, prepublication failure preservation and the undo floor are checked. |
| R3-2 | Fixed. Synthesized LUT entry IDs include local ID, SHA-256 and byte length. Identical entries merge dependencies; conflicting other entry IDs refuse manifest validation before staging. `round_three::short_reused_lut_ids_floor_save_and_reopen_both_contents` starts with short `lut:1` IDs, floors while A and B are both needed, rotates predecessors and checks both exact byte strings after reopen. |
| F1 | Fixed. Same repair as R3-1. `round_three::first_save_floors_history_above_default_batch_limit` starts with 65,537 accepted batches at default limits and checks the same handoff, preserved failure state and undo-floor behavior. |
| F2 | Fixed. The owner retains Core, original cut, branch and prepared-root digest for a pending save. Reconciliation installs only that exact root's transition; another confirmed root leaves Core untouched. The four `round_three::reconcile_*` witnesses cover manual/specialist saves and landed/older roots, then undo, edit and save again. `first_save_unknown_floor_handoff_reconciles_once` covers an uncertain first save that actually advances the floor; the copy's older duplicate installation is removed. Its mutation restores the duplicate and fails at reconciliation. |
| F3 | Fixed. Collection skips an ordinary per-file refusal, retains its charge and continues known-object, temporary and orphan sweeps. A bounded report includes failing paths, attempt counts and total pending files/bytes. `round_three::persistent_file_refusal_collects_other_garbage_and_reports_pending_bytes` keeps one path refused throughout the scope and eight successive saves, checks other garbage retires, bounds object count and then verifies successful cleanup clears the report. This is native Linux IO with a persistent injected file refusal, not an ACL or physical device fault. |
| F4 | Fixed. Core searches with one advancing prefix replay. Per-entry encoded admission costs reject impossible suffixes before replay/serialization; the chosen cut still passes exact cursor, redo, gesture and full-history validation. `floor_search_matches_exhaustive_nonmonotone_cuts` compares the first fitting cut with exhaustive production floor checks, including valid-invalid-valid cuts and corrupt histories. Measurements below use default limits. |
| F5 | Fixed. `kinewright-project/test-util` now enables `kinewright-media/test-util`. Both a library-only check and an independent resolver-3 consumer without dev-dependencies exercise the feature contract. `cargo-hack` is unavailable; the separate consumer supplies the no-dev build. |
| C1 | Mitigated locally; hosted confirmation pending. Local setup profiling identifies three executable hashes as the delay. The Linux script strips the sealed artifact, keeps source `076a96c` and every fixture/deadline frozen, and records per-process executable-read intervals plus individual external-probe times. Expected control exits remain `0,1,1,1`; exit 2 still fails. The hosted 30-second failure itself is not reproduced locally, so the next hosted run must confirm closure. |
| C3 | Parent native lane green; new native execution NOT RUN. The supplied Windows run at the parent passed all relevant steps. No Windows adapter or capability policy changes here. Current Windows checks and their exact prerequisites are reported below; native Windows execution on this follow-up remains NOT RUN. |

## Setup cause and timing

The frozen unstripped executable is 164,282,392 bytes; the stripped executable is
63,643,176 bytes. Stripping changes the sealed artifact, not its source or behavior.
Its new SHA-256 is recorded with the frozen predecessor source and fixture identity.

A final 20 ms process sample, run from the frozen predecessor checkout, observed
these executable-read spans: header 3985.65 ms, worker A 4007.87 ms, worker B 4015.80 ms. These are sampled intervals, not exact syscall
durations. The ordinary external probes took git 2.16 ms, git 2.03 ms, git 2.97 ms, rustc 10.40 ms, ffmpeg 7.92 ms, nvidia-smi 24.03 ms, uptime 2.05 ms.
The sample is in `c1-original-final/`; its run header binds source `076a96c` and the
actual executable hash. Worker setup is dominated by the same pure-Rust debug hash
work. No worker server or port startup occurs in this fixture's setup.

Five alternating original/stripped runs use `CI=true` and the predecessor script's
durable TMPDIR. The unwrapped header medians were
4.061 s
(range 3.994–4.075)
and 1.600 s
(range 1.590–1.611).
Worker B was ready at median 12.153 s versus
4.709 s. Every reply-retry run exited 1 with
`disposition: observed` and no missing requirements. Probe wrappers and file-read
sampling are separate diagnostic runs, excluded from those paired timing distributions.

The final complete predecessor preparation passed with exits `0,1,1,1` for
format/history/retry/legacy-IO, respectively. A new boundary witness also verifies
that diagnostic failure always becomes not-run exit 2; it cannot turn a fixture's
exit 2 into the expected baseline-red exit 1. Its mutation restores that status
masking and fails at `(diagnostic_fails=True, fixture_exit=2, actual_exit=1)`.
No timeout is increased. The hosted slowdown beyond 30 s remains an inference;
the next hosted run prints labeled header/worker read spans and individual probe times.

## Floor-search measurement

The pre-optimization search, with the early first-save capacity refusal removed,
did not complete its first bounded-history selection within 600.069 s. That run
exited 124 and is a censored timing, not a passing save or a speedup denominator.
Its test executable hash and measured peak RSS are retained in `f4-before-valid.log.gz`.

Five unprofiled debug/test runs per default-limit workload measure the successful
first-save call after the witness's deliberate prepublication failure. A separate sealed
measurement-only overlay prints the already captured bound duration after each call.
This records the first, cold selection before that injected failure can occur. The
production source was restored byte-for-byte before running the overlay binary; its
one-file instrumentation map and executable hash are retained in `cold-bound/`.
No timer includes the added print. No production or release-latency guarantee is inferred.

| Default-limit workload | Cold selection median (range), ms | Successful first-save retry median (range), ms | Retry bound calls median, ms | Whole-test peak RSS median, KiB |
| --- | --- | --- | --- | --- |
| 500 closed track-lock edits; 2,000-track document; original encoding >32 MiB; retains 20 batches | 784.211 (779.596–796.482) | 3405.234 (3392.081–3422.621) | 881.313 | 181592 |
| 65,537 accepted no-op Undo batches; retains 26,673 batches | 254.188 (249.283–256.994) | 2419.945 (2410.460–2438.922) | 401.540 | 432920 |

Every run requires actual publication, exact live/durable/reopened history equality and
undo stopping at the floor. Peak RSS covers the entire test, including setup, the
failed-copy control and reopen; it is not decoded-history allocation or an allowance.
The encoded-limit case supplies the nonempty undo-floor control; the batch-limit case
isolates the accepted-command ceiling. Load, five-second CPU deltas and GPU state/clocks
are recorded around each run. No builds or other tests overlap the timing windows.


## Evidence and boundaries

Raw evidence is gzipped under
`/home/riels/Projects/Personal/Kinewright/target/review/ja/s1-evidence/r3/`.
The runtime mutation driver records one failing property witness and its restored pass
per new Rust witness, with source maps and executable hashes. Feature compilation and
the setup sampler have separate red/restored checks. A compile-contract red is not
reported as a runtime behavioral red.

One measurement attempt was stopped after a stale executable was copied into staging.
It is excluded. The final measurement uses the executable selected by the actual Cargo artifact manifest,
binds every Rust source and executable hash, and retains load/GPU annotations.
The final measurement runner now reads the gate's Cargo artifact manifest and seals
the selected test executable itself, so the staging step cannot choose a cached filename.

The S0 fixtures and oracles remain byte-identical. The existing original legacy-IO
contradiction, model/process-crash distinction, decoder certification boundary and S2
producer fencing remain as documented. No comment-latency rerun or change to the L6
250 ms target is claimed. Full slow CI, Kani and native Windows durability remain NOT RUN.

## Inherited baseline controls and lanes

The fresh per-commit harness target reran the revised baseline witnesses:

| Baseline | Revised witness and retained positive control |
| --- | --- |
| R1, history loss | `history_loss_fixture_revised_preserves_one_entry`: exact writer bytes remain the positive control; `history-survives-reopen` passes with one entry. The sealed predecessor still exhibits the literal history red. |
| R2, parent-directory sync | `strict_io_properties_revised_use_actual_syncs_and_no_fallback`: the additive paired fixture observes actual production syncs and passes the strict property; its baseline successful-refusal controls and literal reds remain. |
| R3, in-place fallback | The same paired witness reaches an injected refusal with independent attempted-byte equality, preserves the root and observes no in-place fallback. The contradictory original `legacy-io.json` is unchanged and remains an expected baseline red. No whole-original-IO-fixture pass is claimed. |
| R4, reply retry | `reply_retry_fixture_revised_has_one_durable_entry`: the original and retried receipts match and undo count is one; the frozen predecessor's accepted-edit control still produces the two-entry red. |
| Comment result binding | `comment_receipt_fixture_revised_reconciles_exact_original_result`: original record/receipt identities survive retry, and retired requests cause zero mutations. `comment_observer_persists_exact_bytes_then_missing_receipt` retains successful exact-byte controls and the literal missing-binding baseline red. The clean commit-4 baseline seal is archived evidence, not a fresh old-source rerun. |

Portable Core history/model checks, format admission and routing ran locally. Linux
bundle, copy, receipt, persistent-refusal and native harness witnesses ran on the owned
durable filesystem. Windows Core is compile-only; native Windows execution and the
App/Project cross-checks are qualified below.

L6 numbers are unchanged. The archived round-1 workload measured comment p50/p95
131.511/164.736 ms after the earlier repair, 640,687 retained Rust allocator bytes
under a 3,676,919-byte encoded reservation, and a 16,259,646-byte whole-workload Rust
allocator peak. These are prior measurements from [fix-r1.md](fix-r1.md), not fresh
round-3 measurements or whole-process RSS acceptance. L6 remains open for the lead.

## Verification

The final gate is `gate-final-v2/`. It ran 17 outer commands: 15 succeeded, including
MSVC Core compilation; MSVC App and Project stopped at native dependency prerequisites.
The nested per-commit gate ran 46 commands with 33 raw test summaries and 305 selected
passes. Selections overlap. Full Core targets passed 791 tests; full Project targets
passed 204 tests with two existing ignores. Workspace fast passed 3,758 tests and
verified its exact skip inventory. Explicit app-bridge ignores ran separately in the
per-commit gate. These are local executions, not new hosted CI.

Library-only `cargo check -p kinewright-project --features test-util --lib`, project
no-default builds/checks and the independent no-dev consumer passed. Workspace build,
explicit eval bins, both normal architecture scans, all harness modules/integration,
the three explicit app bridges, format regressions, receipts/copy/branch tests,
touched-file rustfmt, workspace/all-target clippy, actionlint, shell syntax and
classification lint passed. Full slow CI and Kani remain NOT RUN.

The installed MSVC target compiled Core plus tests. App stopped at missing MSVC
`stdio.h` for Whisper and `aws-lc-sys`/NASM prerequisites; Project stopped at missing
MSVC `errno.h` for FFmpeg bindings and `aws-lc-sys`/NASM prerequisites. No Rust cfg
success is claimed for those two crates and no toolchain was installed.

The final runtime ledger is `mutations-v2/results.json.gz`: 10 red/restored Rust pairs.
Each red changes exactly one implementation file and reaches an assertion, with
`0 passed; 1 failed`; each restored run has `1 passed; 0 failed`. All 264 Rust source
hashes match the final production snapshot. `contracts-final/` adds the standalone
feature compile-contract pair and two runtime diagnostic pairs (file reads and status
routing). A failed compile is counted only as a compile-contract red.

The final status-guard change was followed by those targeted pairs, shell syntax and
a fresh complete predecessor preparation. The production Rust source stayed unchanged.
The original 52 S0 fixture/oracle files and all 17 original index records match `076a96c`.

Raw final lines:

```text
Sealed predecessor: /home/riels/.t3/worktrees/Kinewright/t3-5e39b056/target/ja-s1-predecessor/sealed/kinewright-ja-baseline
Linux commit 8 gate complete; inspect ignored/NOT RUN lanes separately.
running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

fast tier on linux skipped exactly the 50 manifest tests and 54 allowlisted ignores (ci/ignored-tests.txt), each observed, nothing else
slow-test manifest and markers agree: 50 tests, features kinewright-agent/slow-tests,kinewright-app/slow-tests,kinewright-media/slow-tests; 54 allowlist entries well-formed
Linux stricter gate complete. Windows execution and full slow CI remain NOT RUN.
```

Raw test summaries (counts include overlapping selections):

```text
[per-commit]
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 649 filtered out; finished in 0.01s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 802 filtered out; finished in 0.02s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 546 filtered out; finished in 0.01s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 802 filtered out; finished in 0.02s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 36.85s
test result: ok. 53 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 100.43s
test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 629 filtered out; finished in 142.06s
test result: ok. 8 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out; finished in 27.84s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 10 filtered out; finished in 35.23s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 10 filtered out; finished in 17.18s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 10 filtered out; finished in 35.52s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 308 filtered out; finished in 0.00s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 647 filtered out; finished in 0.00s
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 127 filtered out; finished in 0.58s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 311 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 132 filtered out; finished in 4.10s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 46 filtered out; finished in 0.00s
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 798 filtered out; finished in 0.37s
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 1188 filtered out; finished in 2.81s
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 798 filtered out; finished in 0.12s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 14 filtered out; finished in 1.07s
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 127 filtered out; finished in 0.01s
test result: ok. 53 passed; 0 failed; 1 ignored; 0 measured; 84 filtered out; finished in 4.64s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 787 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 802 filtered out; finished in 1.56s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 46 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1197 filtered out; finished in 0.85s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 46 filtered out; finished in 0.24s
test result: ok. 42 passed; 0 failed; 1 ignored; 0 measured; 760 filtered out; finished in 0.03s
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 52.83s
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.98s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.18s
[kinewright-core]
test result: ok. 311 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.58s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 59 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.88s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 39 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 65 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 113 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
[kinewright-project]
test result: ok. 136 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 7.63s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.40s
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 51.44s
test result: ok. 53 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 100.53s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
[workspace-fast]
test result: ok. 649 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 50.73s
test result: ok. 42 passed; 0 failed; 5 ignored; 0 measured; 0 filtered out; finished in 0.73s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 8 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out; finished in 27.60s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 37.23s
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.88s
test result: ok. 76 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 21.24s
test result: ok. 0 passed; 0 failed; 7 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 798 passed; 0 failed; 5 ignored; 0 measured; 0 filtered out; finished in 18.97s
test result: ok. 311 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.56s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 59 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.93s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 39 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 65 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 113 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1133 passed; 0 failed; 65 ignored; 0 measured; 0 filtered out; finished in 348.25s
test result: ok. 4 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 20.73s
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.69s
test result: ok. 14 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out; finished in 0.92s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 136 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 7.63s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.16s
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 51.19s
test result: ok. 53 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 101.66s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

## Adjustments and unrun lanes

The copy's old handoff field was removed because the destination owner now retains the
same pending Core cut; installing through both owners would reject a reconciled floor.
The added uncertain-first-save witness and its duplicate-install mutation cover this
integration adjustment. No new store, collector, schema counter, host or S2 fence was added.
An interrupted early gate was discarded and the complete gate rerun against the final
Rust snapshot. Earlier diagnostic/measurement attempts are retained as superseded evidence.

The actual decoder-through-use adapter (L4), comment-latency/release-budget decision (L6),
O11/O26 deferrals, S2 producer coordination, full slow CI, provider/decoder certification,
physical device/power-fault tests and native Windows durability remain outside this round.
Storage witnesses use owned durable roots under the worktree target; `SessionStore::create`
continues to receive its parent explicitly.
No claim of current comment p95, new Windows CI success or whole-process RSS acceptance is made.
