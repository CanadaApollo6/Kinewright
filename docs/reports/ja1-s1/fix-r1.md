# JA-1 S1 final-head fix round 1

Worktree `/home/riels/.t3/worktrees/Kinewright/t3-5e39b056`, branch `ja1/s1`.
This is one follow-up to `0f4f88e0a46544810be41db654f66472f7d4540c`; commits 1–8
are preserved. No push or independent review was performed.

## Finding disposition

Witness names below are Rust test names unless a script is named. The new storage
witnesses live in `tests/support/ja_s1_fix_r1.rs`, included as `final_head` by
`ja_s1_storage`. “Fixed” describes the implemented correction and its stated
evidence; Windows source corrections still require native Windows CI.

| Finding | Status and witness |
| --- | --- |
| A1 | Fixed: `reused_lut_id_retains_both_content_identities_through_first_save_and_recovery`; rejects wrong digest/length, retains A and B under one reused local ID, and refuses a current checkpoint borrowing its identity from historical manifests. |
| A2 | Fixed: `directory_sync_failure_retains_barriers_and_retry_seals_existing_ancestors`; child/parent failures retain uncertainty and a reopened retry seals EXIST paths before root publication. |
| A3 | Fixed: `post_ack_gc_failure_never_changes_committed_outcome_and_retries`; GC is explicit after acknowledgment, failures preserve the committed root, and retry succeeds. `ambiguous_rename_error_requires_reconciliation` also preserves unknown publication status. |
| A4 | Fixed: `two_hundred_resolved_issuers_retire_without_a_lifetime_cap`, plus `issuer_exhaustion_preserves_unresolved_evidence`; resolved retirement releases the live slot while unresolved evidence remains bounded and retained. |
| A5 | Fixed in source: OS-specific `portable_owned_paths_refuse_nonportable_spellings`, shared `Document` import and cfg stub corrections; installed-target compile results appear below. Native Windows execution/clippy remain NOT RUN. |
| A6 | Fixed: controlled target-directory fixture roots, full default-TMPDIR fast gate, and `tmpfs_refuses_named_and_session_storage_with_zero_mutations`. Runtime session ownership is described below. |
| A7 | Fixed implementation: `unchanged_comment_skips_history_decode_and_coalesces_directory_syncs` and fresh paired release measurements; one object-directory sync per acknowledged publication, cached unchanged history, GC outside the acknowledged publication. L6 remains open for the lead. |
| O1 | Fixed: original five IN1B/IN2B tests reproduced red against old app source and green after restoration; test-only items moved below modules and three fixtures now serialize writable `Document::default()`, preserving assertion intent. |
| O2 | Fixed: `realistic_history_saves_every_edit_and_advances_durable_floor`; 2,000-track document, 110 saved edits, lowered 8 MiB history cap, recorded floor and retained newest undo; complete recovery validates archived checkpoints one at a time. Encoded-format decode reservations are calibrated against separate actual allocator runs. |
| O3 | Fixed: all durable-success JA tests/harness roots use controlled target/output directories; full fast gate leaves TMPDIR unset. |
| O4 | Fixed: `strict_replace_refusal_preserves_exact_root` and `strict_io_properties_revised_use_actual_syncs_and_no_fallback`; refusal injected at the rename call, shared production error arm, exact old root and temporary fate asserted. Permission refusal is definite; other errno results conservatively require reconciliation. |
| O5 | Fixed: `save_as_new_destination_syncs_each_child_and_parent_before_root`; actual Save As ancestor creation, both barriers before root, injected failure and retry. |
| O6 | Fixed: same 200-issuer retirement/reopen and unresolved-cap witnesses as A4. |
| O7 | Fixed implementation: same cache/barrier witness and paired timing as A7; the 250 ms target is unchanged and L6 remains open. |
| O8 | Fixed: `legacy_large_document_keeps_its_previous_open_size_contract`; a legacy document above the Studio decode ceiling still opens, without weakening Studio limits. |
| O9 | Fixed: same failed-barrier/reopened-EXIST retry witness as A2. |
| O10 | Fixed: `invalid_comment_does_not_enter_pending_or_pause_the_next_command`; invalid UTF-8 refuses before pending admission and the next valid command commits. |
| O11 | Deferred: legacy check/write authority race predates the Studio writer; changing that separate legacy authorization path is outside this follow-up's durable-bundle correction. |
| O12 | Fixed: `migration_retries_b3_lost_b5_and_corrupt_backup_without_duplicate_identity`; an activation journal blocks legacy writes before mutation. |
| O13 | Fixed: same migration witness exercises B3 retry, lost B5 acknowledgment, backup sync failure, corrupt backup, stable identities and exact backup bytes. |
| O14 | Fixed: `namespace_remains_bound_after_alias_swap`; alias mutation now changes binding after acquisition, and fails at the original-root property assertion. |
| O15 | Fixed: `baseline_change_is_not_a_continuation_of_a_zero_batch_checkpoint`; changed zero-batch baseline and forged forward floor refuse. |
| O16 | Fixed: `retired_unresolved_never_reapplies_or_acknowledges`; exact retired evidence reports retired immediately, including predecessor receipts. |
| O17 | Fixed: `verified_reopen_retires_unreachable_generated_orphans_and_their_charges` and `verified_reopen_retires_over_quota_orphan_before_admission`; fully verified closure authorizes generated-orphan retirement and releases charges, including a sparse over-quota residual. |
| O18 | Fixed: `save_as_unsupported_source_or_destination_has_zero_mutations` snapshots the tree before read-only open and compares it afterward. |
| O19 | Fixed in source: `durable_bundle` imports `Document` on every OS; portable CrashFs remains ungated. Full project MSVC compile is prerequisite-blocked, as recorded below. |
| O20 | Fixed in source: OS-specific `portable_owned_paths_refuse_nonportable_spellings`; Windows `a\\b` equals `a/b`, multi-component serde roundtrip stays valid, embedded component separators refuse. Native Windows test NOT RUN. |
| O21 | Fixed in source: cfg-limited namespace stub lint allowances, Linux-only `adopt_usage`, and Linux activation helper; Linux workspace clippy passes. Native Windows clippy NOT RUN pending native dependencies. |
| O22 | Fixed: predecessor script checks actual native commands, without reading stale `$LASTEXITCODE` after a `.ps1`; bash syntax and actionlint checks. PowerShell execution NOT RUN. |
| O23 | Fixed: predecessor script selects `python` without `nice` on Windows and `nice -n 19 python3 -I` on Linux; bash syntax/actionlint plus Linux predecessor gate. Windows shell execution NOT RUN. |
| O24 | Fixed: predecessor EXIT trap prints a failing log's tail before compression; shell syntax and Linux predecessor gate. |
| O25 | Fixed: CI bypasses the desktop memory wait, desktop wait is bounded to 600 seconds, fast jobs have 45-minute timeouts; actionlint and Linux predecessor gate. Hosted CI NOT RUN. |
| O26 | Deferred: reconstructing a deleted registered predecessor checkout requires separate worktree lifecycle handling; this task explicitly retains the existing worktree and predecessor. |
| O27 | Fixed in source: `rerender_refuses_v3_before_projection_or_output` runs on every OS; admitted legacy rendering is a separate portable test. Linux witnesses pass; Windows execution NOT RUN. |
| O28 | Fixed in source: Windows headless refusal test occupies the actual `sidecar_path_for_project`, preserving exact decoy bytes. Native Windows execution NOT RUN. |
| L1 | Fixed: `native_cli_retains_literal_assertions_and_unchanged_fixture_identity` and full workspace fast gate with TMPDIR unset on this host's tmpfs. |
| L2 | Fixed: the five original app regression witnesses and the full app test target, followed by full workspace fast gate. |

## Runtime session root

S1 does not attach `SessionStore` to the desktop GUI. The existing unsaved app
session remains in memory and has no recovery-sidecar path until named; S2 owns
the live Studio attachment and producer fencing. There is no silent tmpfs Studio
write in that runtime path.

The production `SessionStore::create(parent, limits)` requires an explicit,
existing parent. Its root is
`<parent>/.kinewright-session-<session-id>/session.kinewright`. Linux admission
checks that parent's filesystem before creating the directory and returns
`durability_unsupported` on tmpfs with zero mutations. Integration tests choose
`CARGO_TARGET_TMPDIR`, unit tests choose the worktree target, and the CLI workload
chooses its explicit output directory on the worktree/main checkout filesystem.

## Evidence and scope

Raw evidence is compressed outside Git under
`/home/riels/Projects/Personal/Kinewright/target/review/ja/s1-evidence/r1/`.
Twenty-six distinct runtime mutation red/restored-green pairs passed, including
the five-test old-app regression pair. Repeated pairs do not add coverage credit.
The mutation driver records the actual parent test summary, executable hash,
modified source hash and witness source hashes. Setup failures and preliminary
driver errors are preserved separately and receive no mutation credit. The
Save As ancestor mutation removes both creation and retry-seal barriers; either
barrier alone retains valid redundant protection. The additional AU6 existing
source-guard regression has its own original-red and restored-green evidence.

`scripts/ja-s1-fix-r1-gate.py` runs the mandatory per-commit gate, installed MSVC
checks, all five touched crates' full test targets, the full workspace fast tier
with TMPDIR unset, classification lint, final workspace clippy, actionlint,
format/shell checks and frozen-fixture identity checks. No slow-tier feature is
enabled on this desktop. The first stricter-gate attempt exposed AU6's existing
whole-source OS-gate prohibition in the eval test; the redundant Linux-only
attribute was removed, keeping both rerender tests portable. The failed media
attempt was stopped after diagnosis and remains preliminary evidence. The
complete stricter gate was then restarted in `r1/final-gate/`. The media full-crate target uses one test thread.

Linux syscall traces, dropped-write CrashFs and native process-death tests remain
different evidence classes. No physical power/device fault, native Windows
durability, hosted fast/slow CI, L4 decoder-open certification, full G15/G20,
GUI Studio attachment, or new Kani proof is claimed.

## Validation results

The complete Linux stricter gate passed. All gates below used TMPDIR unset;
`/tmp` remained tmpfs and the controlled target roots were on btrfs. Execution
HEAD was `0f4f88e` with the fix applied; every gate header's Rust source manifest
matches the committed fix source. The source manifest SHA-256 is
`2bc062b1f0e488b163139ab0731b9060ad5586e869cf3780e6ef79c52afc20b6`. Final commit/source binding is recorded outside Git after landing.

The mandatory per-commit gate passed 46 commands and 33 raw test summaries,
including both normal/eval architecture scans, all harness modules, three
explicit native App bridges, refusal/format/media selections, all new storage,
receipt, branch and copy targets, and the AU6 declaration regression witness.
The complete per-phase command/status/timing and raw summaries are also in
`r1-validation.json`.

Installed-target checks: MSVC Core **lib + tests with test-util passed**.
Project's MSVC check was attempted with test-util, lib + tests; native FFmpeg,
Whisper and AWS-LC build prerequisites failed (missing MSVC headers/tooling).
That project cfg lane and native Windows execution/clippy are **NOT RUN**;
no toolchain was installed. The error is a prerequisite failure, not a Rust
compile success. Native Windows CI remains the lead's next lane.

The default-TMPDIR full workspace fast command exited **0**:

```text
CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 nice -n 10 python3 -I scripts/slow_tests.py fast

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests kinewright_media

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests kinewright_project

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

fast tier on linux skipped exactly the 50 manifest tests and 54 allowlisted ignores (ci/ignored-tests.txt), each observed, nothing else
```

`slow_tests.py lint`, final `cargo clippy --workspace --all-targets -- -D warnings`,
actionlint, shell syntax, Python AST, touched-file rustfmt and frozen-fixture
identity checks passed. Existing ignored/slow-feature lanes remain NOT RUN,
with names in `ci/ignored-tests.txt` and `ci/slow-tests.txt`; the three mandated
App bridge ignores were each run explicitly and passed.

The modified predecessor script ran successfully against the existing pinned
`076a96c` checkout: its max-2 observer passed, the format fixture exited 0 and
the three unchanged history/retry/IO baseline reds each exited their expected 1.
The script verified actual reports, source and sealed-artifact identities.

```text
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

Raw full-target per-crate summaries (including doc targets and existing ignores):

kinewright-core:

```text
test result: ok. 311 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.57s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 59 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.97s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 39 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 65 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 113 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

kinewright-project:

```text
test result: ok. 136 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 12.56s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.16s
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 52.09s
test result: ok. 35 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 96.67s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

kinewright-agent:

```text
test result: ok. 649 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 51.62s
test result: ok. 42 passed; 0 failed; 5 ignored; 0 measured; 0 filtered out; finished in 0.76s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 8 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out; finished in 27.94s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 36.88s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.27s
test result: ok. 76 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 21.89s
test result: ok. 0 passed; 0 failed; 7 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

kinewright-app:

```text
test result: ok. 797 passed; 0 failed; 5 ignored; 0 measured; 0 filtered out; finished in 17.09s
```

kinewright-media:

```text
test result: ok. 1132 passed; 0 failed; 65 ignored; 0 measured; 0 filtered out; finished in 1109.73s
test result: ok. 3 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 23.25s
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.02s
test result: ok. 13 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out; finished in 2.88s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

## L6 paired measurements

Five alternating, unprofiled before/after pairs completed with eight comments
per process, followed by five separate DHAT runs. All 15 workloads had zero
unexpected errors and exactly three intended copy refusals each (45 total).
The original production source is `0f4f88e` plus counter instrumentation only;
the measured after-source manifest and binary hashes are in the compressed raw
identity. The sole subsequent Rust-source change removes the redundant Linux-only
attribute from an agent eval test; the benchmark dependency sources are unchanged. Rebuilding both ordinary and
profiled examples from the final source reproduced their measured sealed
binaries byte-for-byte.
The same 32-track/120-command workload, retained 8 MiB evidence, eight saves,
eight comments, six ordered floors, copies, full recovery and first-save
handoff ran on both sides. No build or test overlapped the timing window.

| Comment metric | Before | After |
| --- | ---: | ---: |
| Median run p50 | 238.948 ms | 131.511 ms |
| Median run p95 | 262.516 ms | 164.736 ms |
| Range of run p95 | 254.199–312.907 ms | 161.305–165.546 ms |
| Median sync calls/comment | 26 | 16 |
| Range of sync calls/comment | 24–28 | 16–20 |

The exact call sequences in every pair were before `[28,24,25,26,26,26,26,26]`
and after `[20,16,16,16,16,16,16,16]`. These count actual file and directory
sync calls around the complete comment API, including maintenance at the next
save. The acknowledged publication itself coalesces object-directory syncs to
one and does not run GC. This bounded workload now falls below the unchanged
250 ms target; **L6 remains open for the lead**. Shared desktop CPU/GPU load and
five-second contamination samples are retained for every lane, without gating
on a quiet machine. These numbers do not establish a general latency guarantee.

The separate five DHAT decodes each retained 640,687 actual Rust allocator bytes
under a 3,676,919-byte encoded-format reservation for 216,346 encoded bytes.
Each whole-workload Rust allocator peak was 16,259,646 bytes. Allocator timings
are excluded. Reservation bounds, Rust heap and kernel RSS are distinct;
this calibration covers the measured workload, not every possible document.
Full per-run measurements and binary identities are in
`r1-storage-measurements.json`; raw samples are in `r1/timing/` outside Git.
