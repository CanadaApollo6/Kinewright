# Maximal qualification

## Qualified after proof-capture release (latest)

**QUALIFIED.** The single post-classification maximal release run passed in
**1406.485695 seconds**, including exactly 256
approve/delete/Undo cycles, 720 clocks, eight tasks, 128 sends, 24 attempts,
Freeze, Stop, settled cleanup, collection, exact reopen/audit and all seven cold
opens. The original 3,600-second authority was not renewed. No pool cap, profile,
fixture constant or production behavior changed. Maximum graph depth is 9.

The approval loop took 469.058000 seconds. The seven whole-project cold opens
were 4.394952, 4.374865, 4.350219, 4.365414, 4.411642, 4.490107 and 4.537343
seconds (median 4.394952). These open this
complete maximal run, not the separate 2,000-step heavy-history benchmark.
Libtest reports 1 passed / 0 failed / 0 ignored. Shell time is 1458.441 seconds,
including shared-lock admission and cached Cargo preparation. The recorded
binary ran at nice 10 with four build jobs; source stayed fixed throughout.

[The capacity classification](undo-capacity-classification.md) accounts for all
fourteen leases in the prior refusal: five retained fixture proof captures
consumed the Task capture room alongside six real actors. Each proof is now
checked against its durable checkpoint, reduced to the exact same combined
identity that snapshot equality already compares, then released before the
next specialist. Actual actors retain their production lifetimes through
Freeze and Stop. No work, state assertion or operation was removed.

The permanent `C5E_BETWEEN_TASKS_ONLY_PRODUCT_ACTOR_LEASES` guard fails on the
original Small fixture order (three leases versus two actors, 9.20 seconds)
and passes after correction, including the full Small tail (one pass,
13.84 seconds). `C5E_DURABLE_PROOF_IS_EXACT_BEFORE_RELEASE` checks the saved
owner before releasing each proof. Both guards also pass across all eight
maximal tasks. [Red/green source and logs](measurements/undo-capacity-proof-release.json.gz)
and [full qualification evidence](measurements/undo-maximal-qualification-final.json.gz)
are preserved. The earlier failures below are historical evidence.

The growth follow-up remains open under [Owed after HX](undo-switch-progress.md#owed-after-hx).
It contains all sixteen first-release windows, counters, publication suspects
and explicit 2,000-command extrapolation assumptions. No growth fix was made.
The per-commit gate passes **3,130 tests** across core, project and agent
(189 declared ignores), workspace all-target clippy with `-D warnings`, fmt
and slow-test manifest lint. [Exact commands, logs and source hash](measurements/undo-maximal-qualification-gates.json.gz)
record the scope. The paired measurements and final gates follow this
qualification checkpoint in the lead's prescribed order.

## Earlier optimized stop: typed history backpressure

**NOT QUALIFIED.** The single authorized `--release` run completed all **256
approve/delete/Undo cycles**, then failed at **817.34 seconds** with
`C4_MCP_ATTEMPT_HISTORY: history_backpressure` (`writer_host/c4_tests.rs:458`).
It completed Freeze and the task-completion markers for task indices 0 through
4. Stop, settled cleanup, collection, exact reopen/audit and all seven cold
opens did not complete. This is a capacity refusal during the remaining tail,
not an authority expiry; it does not prove the full tail fits 3,600 seconds.

The command used the original fixture and 3,600-second authority, four jobs,
nice 10 and normal shared-lock admission. MS1 held the lock first and was not
signalled. The release build took 3m 38s; shell time was 1040.339 seconds,
including build/lock admission, versus 817.34 seconds reported by libtest.
The compiled binary and the running test's actual nice level are recorded.
The separate two-import Windows fix, `1236997e`, was committed while this
already compiled test ran; it changes neither the fixture nor that binary.

All sixteen windows are preserved below; each includes the existing fixture
and diagnostic work in those sixteen cycles. The approval loop took
**466.215756 seconds**. Mean seconds per cycle grow roughly linearly with
cycle index: **0.510396 → 3.565586**, least-squares slope **0.012694 seconds
per cycle per additional cycle index**, **R² = 0.987279**. This describes one
release run; it is not a debug/release speedup comparison or a causal profile.

| Window ending cycle | Loop elapsed seconds | Mean seconds per cycle |
| --- | --- | --- |
| 16 | 8.166343 | 0.510396 |
| 32 | 17.862616 | 0.606017 |
| 48 | 29.439592 | 0.723561 |
| 64 | 43.778985 | 0.896212 |
| 80 | 61.061127 | 1.080134 |
| 96 | 80.845150 | 1.236501 |
| 112 | 103.511871 | 1.416670 |
| 128 | 129.330346 | 1.613655 |
| 144 | 158.377350 | 1.815438 |
| 160 | 190.749030 | 2.023230 |
| 176 | 226.289105 | 2.221255 |
| 192 | 266.173289 | 2.492761 |
| 208 | 309.549265 | 2.710999 |
| 224 | 357.452552 | 2.993955 |
| 240 | 409.166384 | 3.232115 |
| 256 | 466.215756 | 3.565586 |

The leading suspect is the growing publication manifest/closure processing.
Between cycle 32 and 256, `manifest_decoded` grows from **1,253,884 to
8,007,062 bytes**, object count **266 → 1,610**, snapshot count **73 → 521**
and `snapshot_decoded_total` **9,260,771 → 184,025,071 bytes**. Graph depth
stays **9** and `max_history_encoded` stays **1,798 bytes**. Publication reads
the manifest, accounts its graph and builds/iterates membership maps
(`bundle_recovery/publication.rs:376,389–395,448`). Its immutable memo avoids
re-decoding every old payload on every command: the retained snapshot total
is a footprint counter, **not** a counter of CPU work repeated per approval.
The existing profile probe also reads the retained objects every 32 cycles
(`orchestration_retention.rs:298–353`); that diagnostic work is included in
the corresponding windows. These counters identify a leading suspect, not
how many milliseconds it costs. No growth fix was attempted.

At cycle 256 the recorded physical peak is **33,721,409 bytes**, metadata
**35,235,124 bytes** and peak decoded bytes **81,496,208**. Those store
counters do not identify the history pool whose allocation was refused; no
pool category or leak is inferred from them. The test did not log a pool
usage breakdown at the refusal. No limit, lease profile, authority or fixture
constant was changed, and no second release qualification was run.

At that stop, the qualification/test/report changes remained **uncommitted**;
paired timings and the final qualification gates were **NOT RUN**. The separate
Windows import-fix checks followed. The later capacity ruling and qualified
correction above supersede that pending status.
[Raw release failure, source, command, binary hash, lock evidence and counters](measurements/undo-maximal-qualification-release-failure.json.gz)
and [the sixteen-window plot](measurements/undo-maximal-release-windows.html)
preserve the result. The earlier default-profile failure below remains historical
evidence; its stop status predates the authorized release rerun.

## Separate Windows import fix

Commit **`1236997e9c6eb5bfab9937d57a90fcfd0c8954da`** adds only the two
Linux cfg guards in `tests/support/ja_s1_fix_r4.rs`. No qualification changes
were staged. `cargo clippy -p kinewright-project --all-targets -- -D warnings`
passes. The requested `cargo check -p kinewright-project --tests --target
x86_64-pc-windows-msvc` was attempted with the installed target, original
FFmpeg setup and normal shared lock; it exits 101 in native dependencies
before Project test compilation: FFmpeg cannot find the target C header
`errno.h`, and AWS-LC's Linux `cc` reports unknown `pthread_rwlock_t`.
Windows compilation is **unverified locally**, not reported green. Source
inspection confirms both imports now have the same Linux cfg as the fixture
items using them. [Commands, full logs, source and commit](measurements/undo-windows-import-1236997e.json.gz)
retain the exact boundary. Both check processes and the failed qualification
finished and were reaped before the subsequent capacity investigation.

## Earlier unoptimized stop

**NOT QUALIFIED.** On `8ac25708` plus the attached test-only changes, the
maximal test failed after **3629.08 seconds** at `C4_TASK_STATUS`
(`writer_host/c4_tests.rs:94`). The response was the typed refusal
`task_authority_expired: task authority expired`.

All **256 approve/delete/Undo cycles** completed. The log also confirms the
following Redo acknowledgement and checkpoint-phase history metrics. The complete
Freeze/Stop/cleanup/collection/reopen/audit tail and seven cold opens did not
complete. This is not a full-tail pass. No authority was renewed, no limit
raised, no fixture constant changed, and no timing run was repeated.

At 256 cycles the graph-depth bound still held at 9; the run had 1610 objects,
33711562 peak physical bytes, 35225789 metadata bytes and 81494352 peak decoded
bytes. No history-capacity refusal or storage-bound assertion failed. These
are measured counters, not an attribution of the runtime cost. The test used
the default unoptimized test profile and the prescribed four build jobs,
shared lock and nice level 10. Its shell time was 3629.348 seconds, including
0.12 seconds of cached Cargo preparation; libtest reported 3629.08 seconds.

The [brief](/home/riels/Projects/Personal/Kinewright/.review/arch/task-hx-h2b.md) requires stopping if the maximal
fixture cannot fit the original 3600-second authority. Work stops here. The
new paired benchmark, final qualification gate and final MSVC check are
**NOT RUN** after this failure. Prior checkpoint gates remain valid only for
their committed trees. No build or test remains running; the failed process
was allowed to finish and was reaped.

The two qualification files remain uncommitted: `c5e_tests.rs` removes the old
history-limit escape, requires exact cycle counts, releases the audit owner
before cold-open timing and asserts the whole-tail time; `ci/ignored-tests.txt`
only clarifies the existing on-demand entry. They are not committed as a green
checkpoint. [Failure log, source, counters, command and diff](measurements/undo-maximal-qualification-failure.json.gz)
preserve the evidence.

Completed local checkpoints, without a push:

- `c00db31a`: CI cfg fixes, synchronous actor lease teardown and retained-work disclosure.
- `3be098c8`: legacy deletion, Muse F3 site and CoreState restore fallback removal.
- `a8de5ca3`: C6 mutation retargeting and runtime qualification.
- `8ac25708`: all seven switch mutations and the green checkpoint gate.

The last checkpoint passes 5464 workspace fast tests, 697 Project slow tests
plus the explicitly enabled 20000-clip integration case, all 64 session seeds /
65536 attempts, 54 unpaid TE1 tests, clippy, fmt, workspace and three shipping
builds, lint and runner self-tests. Its detailed inventory and authority table
remain in [the reader/door audit](undo-reader-door-audit.md). All 207 inventory
rows are accounted for; 28 obsolete-proof rows are retired. No new timing table
is substituted for the historical paired measurements.

Committed range `69057aea..8ac25708` deletes 21 files, with 10787 inserted and 18745 deleted text lines (net -7958). The [complete deleted-path list](undo-deleted-files.json) excludes the pending qualification/report changes and binary-evidence line counts.
