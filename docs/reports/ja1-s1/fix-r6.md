# JA-1 S1 round 6c

Worktree `/home/riels/.t3/worktrees/Kinewright/t3-5e39b056`, branch `ja1/s1`, starting HEAD
`169197b819e0059e6b8f2519667c5cb3f7ab912b`. One commit:
`fix(ja1-s1): close round-6 review findings`. No push or self-review.

Round 6c continues the retained round-6b implementation after the unrelated deletion of
the main checkout's `target/` and Cargo registry. All round-6 evidence below was freshly
regenerated; none of the lost logs is credited. The lead's CI timeout commit is unchanged.
The measurement example's wildcard-import and manual-divisibility clippy lints are fixed.

The lead's **Lead ruling after the round-6 stop** withdraws decision 2. There is no
abandon action. S0 `:403-405` and plan `:574-575` retain priority: a matching root with
an unreadable graph, or an unreadable tagged destination, remains `durability_unknown`
and refuses further attempts until reconciliation succeeds. The known limitation is
recorded in the README. This replaces the round-6a stop report.

## Dispositions

Storage witnesses below are `round_six::` in `kinewright-project`, `--features test-util`,
`--test ja_s1_storage`. Core witnesses are in `kinewright-core --test ja_s1_history`.
Evidence is under the main checkout's `target/review/ja/s1-evidence/r6/`, gzipped.
These are local Linux filesystem/fault witnesses and portable Core logic; they are not
physical storage-crash or Windows certification.

| Finding | Disposition and witness |
| --- | --- |
| R6-1, root-staging cleanup uncertainty retains Core | Fixed. Namespace replacement returns an explicit `NeverAttempted`, `DefinitelyRefused`, or `Unknown` boundary independently of general store uncertainty. Only `Unknown` retains publication identities, Core's barrier and copy-source reconciliation. `root_staging_and_cleanup_failure_releases_first_save_to_another_destination` injects root staging and unlink-cleanup failures, observes both faults and zero rename attempts, checks A's exact original root, the pending temporary's charge, every original Undo/Redo target, and a same-session save to B while A is unavailable. A's owner subsequently retries its retained cleanup. Mutation pairs: `root-boundary`, `root-cleanup-retry`. The one `publish_root` caller covers ordinary, specialist and copy publication; first-copy result classification uses its cleared/retained publication tag. |
| Opus m1, successful copy reconciliation leaves a stale source tag | Fixed. The shared source hold records settlement; the old pending source tag is cleared without reading an already-settled destination. `reconciled_first_copy_does_not_gate_the_session_on_its_old_destination` and `reconciled_save_as_does_not_gate_the_source_owners_next_save` reconcile, make that destination unavailable, and save through the same source owner. `pending_copy_source_reconciles_before_the_next_first_save` covers the still-pending readable destination and retains its necessary settlement. Pairs: `first-copy-source`, `named-copy-source`, `active-copy-source`. |
| Opus m2, failed commit leaves save in progress | Fixed. An own-ticket internal commit failure clears the actor's ticket without replacing history and returns typed `CommitInvariant`, mapped to `HistoryCommitInvariant` by project. `failed_commit_releases_the_ticket_with_a_typed_internal_error` reaches real revision overflow after retaining a tagged barrier, checks every entry remains identical, and obtains a new barrier. Pair: `failed-commit-release`. |
| Opus m3 / withdrawn decision 2 | Known limitation, recorded as directed in README. No abandon action or automatic release was added. Existing strict unreadable-destination and matching-bytes/unreadable-graph rules remain. No new exit witness is claimed. |
| Opus m4, restore during a pending ticket reports corruption | Fixed. Restore returns typed `SaveInProgress` while either an in-flight or retained ticket exists and preserves history. `restore_during_a_save_returns_typed_contention_and_keeps_entries`; pair `typed-restore`. |
| Opus m5, public proof-free ticket APIs | Fixed. Raw Core begin/settle APIs are crate-private. A persistence capability binds each Core handle lineage to its owner's private proof type; project constructs its proof only inside the private `history_save` module. Commit, tag, retention and settlement require that proof. Core clones and restore cannot rebind it, and an untagged ticket cannot become ready. `another_proof_type_cannot_take_over_a_history_save_owner` plus two compile-fail API examples. Pairs: `proof-binding`, `untagged-retention`, `raw-api-visibility`. Owners of their own fresh Core may bind their own private proof; they cannot operate a project-bound Core with a different proof. Core does no filesystem IO and gains no project dependency. |
| Opus m6, durable-history reread per save | Measured and fixed; see below. Cache entries now bind the exact history ObjectRef to the decoded history. Floor alignment reuses only the entry matching the current branch checkpoint, otherwise it reads durable state. `durable_floor_uses_only_the_current_checkpoint_history_cache` checks zero early decodes across edits, repaired cache entries and reopen, plus one fallback decode after invalidating the binding. Pairs: `floor-cache-read`, `floor-cache-binding`. |
| Astra report Minor, quota red spawn count | Corrected `fix-r5.md`: the constrained red recorded one spawn. The source/CI causal statement is qualified separately; the setup-timeout conclusion is unchanged. |
| Astra report Minor, actor blocking bound | Corrected `fix-r5.md`: the reported interval is sampled suffix replay plus document/history comparison. It excludes replacement/drop and notification/reply, so it is not a complete actor-blocking bound. |

## m6 measurement

The exact default serialized-history cap, **33,554,432 bytes**, was attempted and refused by
the existing decode admission: parser scratch alone is 2x encoded size, exhausting the
64 MiB decoded-record cap before any decoded values. A full admitted reread at that exact
serialized size therefore cannot run under today's limits. No cap was changed.

A valid **15,470,000-byte** escaped-marker history, close to this shape's effective live-history
admission boundary, was admitted through the production store. Five production `read_record`
reads/decodes took **247.908–253.612 ms** (median **248.489 ms**), with separately sampled
result drops at 0.007–0.008 ms. One sample crossed 250 ms; the current-checkpoint cache path
is retained. This is one controlled shape and a read/decode interval, not a complete save,
actor-blocking, or cold-device bound.

`measure-near-cap/` binds the current direct-read measurement binary and source hashes,
compiler and profile, and before/after uptime/top-five CPU/GPU annotations. The five-second
load sampler was enabled; the 3.148-second lane produced zero interval samples. There were
no concurrent builds/tests of ours. `ja_s1_history_read` reproduces the exact-cap refusal
and the admitted read workload; `ja-s1-fix-r6-measure.py` runs the already-built binary
under a 600-second bound. The exact-cap refusal is not credited as a successful read.

## Mutation ledger

`mutations-final/` contains twelve source-bound red/restored pairs from one fixed Rust source
tree: eleven runtime property pairs, each `0 passed; 1 failed` then `1 passed; 0 failed`, and
one visibility pair, `0 passed; 2 failed` then `2 passed; 0 failed`. The visibility red means
the formerly negative examples unexpectedly compile; it is an exercised rustdoc assertion,
not a prerequisite/compiler failure. Every new behavioral witness has a targeted pair.
`scripts/ja-s1-fix-r6-mutations.py` records source and runtime-binary hashes and restores the
mutated file even when a run fails. All twelve restored rows share one Rust source identity,
which also matches the measurement and gate source hashes.

## Gate

`scripts/ja-s1-fix-r6-gate.py`, TMPDIR unset, at parent HEAD `169197b` with
tracked diff SHA-256 `56c85f71b2b2975662a4612631d245855a64be5c7aaaad008dc713194763ec2f`.
Every recorded gate header matches all 270 current Rust source files, the restored mutation
rows and the measurement. The final report is documentation-only relative to those checks.

| Step | Result |
| --- | --- |
| Frozen 076a96c predecessor controls | passed; format control and the expected history/retry/legacy-IO baseline reds were exercised |
| Per-commit gate (`ja-s1-gate.sh 8`) | 344 passed, 0 failed across 33 invocations; builds, normal/eval architecture scans, clippy, rustfmt, ceiling/format controls, native App bridge tests and affected tests passed |
| Core, full targets with test-util | 798 passed, 0 failed; 1 declared ignore |
| Project, full targets with test-util | 237 passed, 0 failed; 2 declared ignores |
| Explicit selected `ja_baseline` | 9 passed, 0 failed, 3 declared ignores; each ignored native App bridge witness also passed explicitly |
| Measurement example build and explicit test-util clippy | passed |
| Standalone test-util consumer, test-util library, project no-default library | passed |
| Predecessor provenance and setup diagnostic | passed |
| Windows MSVC Core lib/tests compile check | passed |
| Windows MSVC app/project compile checks | NOT RUN: native `aws-lc-sys` cross-build prerequisite failed on this Linux host, including unknown `pthread_rwlock_t`; project/app Rust checks were not reached |
| Workspace fast tier, all tests serialized (`--test-threads=1`) | 3,799 passed, 0 failed, 104 expected ignores; exit 0 |
| Classification, final workspace clippy, actionlint, shell syntax, frozen fixtures since 45aa892 | passed |

The standalone feature consumer's first offline attempt stopped because the deleted Cargo
cache lacked its separately resolved `aws-lc-rs v1.18.1`. Its failed log is retained as
`gate/feature-consumer-attempt-1.log.gz`. A locked online fetch repopulated that consumer's
cache without changing repository sources or lockfiles. The resumed gate rechecked HEAD,
tracked diff, every Rust source hash, command and TMPDIR before retaining each completed
step. The consumer then passed, and every remaining step ran. Both driver logs are kept;
there is no credited pass for the failed cache prerequisite.

The brief permits one selected `ja_baseline` invocation because no harness code changed;
the workspace fast tier also includes that target normally. The three on-demand native
App bridge tests passed through the per-commit gate. All raw `test result:` lines are
preserved in the gzipped logs, with touched-crate lines copied below.

Final workspace fast-tier line:

```text
fast tier on linux skipped exactly the 50 manifest tests and 54 allowlisted ignores (ci/ignored-tests.txt), each observed, nothing else
```

Final driver lines:

```text
Linux round-6 gate complete. Windows execution and full slow CI remain NOT RUN.
gate exit=0
```

Native Windows execution, app/project MSVC Rust compilation, hosted CI, the full slow
CI tier, Kani, GUI interaction and physical storage faults were NOT RUN. Core's MSVC compile
check is compile-only. No native Windows durable-success or product acceptance is claimed.

Only this run's 18 `/tmp/kinewright-*` AU6 fixtures, identified by the recorded media-test
PID 107802, were removed after the gate. No `/tmp/kinewright-*` remains. The main checkout's
recovered review files and pre-existing target contents were left intact.

## Touched-crate raw summaries

`kinewright-core`, full targets:

```text
test result: ok. 311 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.54s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 59 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.96s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 39 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 65 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 113 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
```

`kinewright-project`, full targets:

```text
test result: ok. 136 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 5.42s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.28s
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 50.36s
test result: ok. 86 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 91.76s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```
