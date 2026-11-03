# TE1.1 fix 2

This consolidates the unpushed performance commit `2e8acc88` with Astra's full-wake
recheck correction and Muse's applicable size-observer corrections. Parent is
`9a2b93b284ac3f8677e9f97c849ee27feb7d2015`. One local fix-2 commit, no push.
The first fix's findings, counters, corpus pins and historical A/B remain in
[the first fix report](te1-1-fix1.md) and [the counter report](te1-1.md).

## Complete Director wake compatibility

The first fix's golden compared only expanded accounting contents. It missed the
revision and the enclosing result's `usage_ref` hash/length. That did not satisfy
the complete unchanged-input contract. The new golden compares every byte of the
final Codex Director prompt, including that metadata, against real base captures.

The common driver starts with fixed identities and a 1000 ms clock. It uses
production decode, ingestion, successor validation, real store admission, the
complete wake assembler and the ordinary Codex wrapper. The base harness compiles
these against `b4e9abc8`, using the exact wake assembly copied from that source.
The base uses its legacy event naming and skips starts; the head adds snapshot
and size observations and their corresponding publications. Initial start, first final, a size-only
publication, another start and a recovered final produce pending and recovered
wakes. No identities or metadata are removed or normalized in the comparison.

| Full base prompt | Bytes | SHA-256 |
| --- | ---: | --- |
| Pending child | 5494 | 343395316528fb70c60003430447de1fff922b8b01c28d8bd79af7b8278e7cd1 |
| Recovered child | 5892 | 46faed1cf29baa5523b6f106dc4916a7b7b3dd08f5ee0293dd840cd6bd36426d |

The pre-fix full-prompt witness failed, 0 passed / 1 failed. The repaired witness
matches both base captures. The earlier accounting-body golden remains a separate
boundary and is retained. The base capture, harness patch, driver, exact-source
fragment hash and logs are under `target/te1-fix2-wake/`.

The actual stored `TaskResult` and its real host-owned `usage_ref` are unchanged.
The wake first reads and verifies those real records. A Serialize-only wire
descriptor then describes the legacy accounting bytes included in the payload.
It is not minted as a store ObjectRef and has no deserializer or conversion to
mutation authority. Provider JSON is assembled separately; stored results and
replay checks keep their original references. The golden reads the stored result
again and checks it is unchanged after rendering.

Accounting retains a bounded, monotonic `observer_publications` count. It defaults
to zero for older records and is omitted when zero. It cannot exceed the actual
revision or regress in a successor. Observer-only changes increment it only when
the legacy body is unchanged. An actual no-op remains a no-op. The provider view
subtracts this offset from the displayed revision, removes the counter, restores
base event labels, removes observer fields/starts and recomputes totals over the
shown samples. Exact replays and EOFs the base suppressed are suppressed in that
view. Authoritative usage, lower bounds, ceilings and actual revisions are never
replaced by this legacy display projection.

The real native-owner turn witness checks the durable counter, JSON persistence,
start-gap coverage, exact final replay and EOF. Its authoritative lower bound stays
79 and incomplete while the legacy view remains equal to the earlier base final.
Legacy samples without TE1 snapshots do not incur the extra projection comparison.

## Performance removals retained

`AccountingRecord::validate_successor_size` returns the serialized length already
measured by validation. `change_spending` reuses it for the 4 KiB reserve instead
of encoding again. Successor validation checks the previous revision directly,
avoiding a clone just to obtain it. `validate_streams` validates borrowed prefixes
through the same binding rules used by `bind_stream`, without cloning the task.
The discarded snapshot-watermark prepass was removed by the first correctness fix.

An initial start or repeated start while coverage is already incomplete skips
publication. A start after a complete result still durably marks a gap. Starts
carrying usage/cost and provider failure/rate-limit handling keep their checks.
Reserve, persisted binding, no-op start and required-gap mutation pairs are retained.

The earlier source's paired result was 2565.64 s at base and 2552.27 s at
`2e8acc88`, with wall B/A 0.9948 and CPU B/A 0.9883. That is historical evidence
for the superseded source. Its report is archived under
`target/te1-fix2-wake/report-before-wake.md`; its raw evidence is still under
`target/te1-fix2/ab/`. Final-source qualification is recorded below.

## Muse recheck

Muse's recheck approves the original spending fixes and identifies minor residuals.
The full-wake projection recomputes displayed totals after stripping starts.
The redundant encode/task clone removals above were already in fix 2.

Size recording now retains the host lock through the measurement write in the
existing host-to-measurement lock order. Final publication or teardown cannot
interleave between attachment validation and recording. A two-barrier fixture
holds the recorder at that boundary and verifies teardown remains excluded; it
releases and joins its worker before asserting, including under the mutant.

Native size recording returns an optional observation. Off Linux it is unobserved,
which does not poison independently observed wire request/result completeness.
A known native failure still marks wire metrics incomplete. The real-MCP byte
witness covers that independent wire path. No per-tool publication or fsync is added.
Windows runtime remains unrun; the required Windows compile/lint gate is below.

Future unpinned event kinds and native resume assumptions are explicit follow-ups
in [the counter report](te1-1.md). No arbitrary raw kind text is newly retained,
and no native resume route is enabled.

## Mutation pairs

All 27 final-source mutation pairs pass. Each RED executes exactly one witness
and fails it; each byte-identically restored GREEN executes exactly that witness
and passes it. The runner checks restored bytes and SHA-256 before and after
GREEN. All subject files match their backups after the last pair. Full proofs,
byte backups and RED/GREEN logs are in `target/te1-fix2-wake/mutations/`, indexed
by `pairs.json`.

| Mutation | RED | Byte-identical restore / GREEN |
| --- | --- | --- |
| `replay` | 0 passed / 1 failed | 1 passed / 0 failed |
| `reset` | 0 passed / 1 failed | 1 passed / 0 failed |
| `claude-uuid` | 0 passed / 1 failed | 1 passed / 0 failed |
| `model-watermarks` | 0 passed / 1 failed | 1 passed / 0 failed |
| `scoreboard-lower-bound` | 0 passed / 1 failed | 1 passed / 0 failed |
| `pending-sends` | 0 passed / 1 failed | 1 passed / 0 failed |
| `payload-collision` | 0 passed / 1 failed | 1 passed / 0 failed |
| `observer-payload` | 0 passed / 1 failed | 1 passed / 0 failed |
| `model-regression` | 0 passed / 1 failed | 1 passed / 0 failed |
| `partial-coverage` | 0 passed / 1 failed | 1 passed / 0 failed |
| `wake-events` | 0 passed / 1 failed | 1 passed / 0 failed |
| `wake-snapshots` | 0 passed / 1 failed | 1 passed / 0 failed |
| `cost-retention` | 0 passed / 1 failed | 1 passed / 0 failed |
| `cost-budget` | 0 passed / 1 failed | 1 passed / 0 failed |
| `direct-input-coverage` | 0 passed / 1 failed | 1 passed / 0 failed |
| `native-observer-loss` | 0 passed / 1 failed | 1 passed / 0 failed |
| `native-size-loss` | 0 passed / 1 failed | 1 passed / 0 failed |
| `serialized-reserve` | 0 passed / 1 failed | 1 passed / 0 failed |
| `persisted-bindings` | 0 passed / 1 failed | 1 passed / 0 failed |
| `turn-start-noop` | 0 passed / 1 failed | 1 passed / 0 failed |
| `turn-start-gap` | 0 passed / 1 failed | 1 passed / 0 failed |
| `full-wake-reference` | 0 passed / 1 failed | 1 passed / 0 failed |
| `full-wake-revision` | 0 passed / 1 failed | 1 passed / 0 failed |
| `full-wake-totals` | 0 passed / 1 failed | 1 passed / 0 failed |
| `observer-revision` | 0 passed / 1 failed | 1 passed / 0 failed |
| `size-teardown` | 0 passed / 1 failed | 1 passed / 0 failed |
| `wire-unobserved` | 0 passed / 1 failed | 1 passed / 0 failed |

The six additions are complete-wake reference projection, complete-wake revision,
stripped wake totals, durable observer-counter regression, size-teardown exclusion,
and unobserved-native versus complete wire metrics. All original 21 pairs remain,
with the moved wake sites and optional native-size API reflected in their anchors.

## Full gates

| Full suite | Passed | Failed | Ignored | Targets |
| --- | ---: | ---: | ---: | ---: |
| core | 815 | 0 | 1 | 20 |
| project | 426 | 0 | 2 | 18 |
| agent | 1304 | 0 | 70 | 28 |
| app | 971 | 0 | 5 | 1 |

All 3,516 tests pass with zero failures and 78 ignored across 67 targets. Formatting
and Linux workspace all-targets clippy pass. The Windows script passes workspace
all-targets and Agent slow-feature all-targets compilation/lint for MSVC. Manifest
lint reports 118 tests, 121 well-formed allowlist entries and 27 equipped-session
skip sites. All 44 Python self-tests pass. Windows runtime was not exercised.

All 716 pinned crate/source/fixture, Cargo and gate-helper files match
after the gates. The manifest SHA-256 is
`c2db921bca0558888e7ab322753b5cbd74694e4404dffe82ba7042d92c2cae23`.
Exact logs, start/end stamps, exit files and counts are under
`target/te1-fix2-wake/`. The earlier 2e8acc88 gate evidence is historical.

The commands remain the original TE1.1 full gate commands. Every Cargo command
sources FFmpeg in the same shell and uses the shared flock, four build jobs and
nice 10; test threads are four. Python uses nice 19 and isolated mode. TMPDIR and
LOCALAPPDATA are under the owning physical target.

## Final maximal and paired qualification

Both final-source pairs pass the approximate 10% upper budget. Pair 1 was near
the boundary and triggered the predeclared repeat; pair 2 used the same frozen
source and binaries. Neither pair waited for a quiet machine. Both selected
library-test binaries use opt-level 0, line-tables-only debug information, debug
assertions and overflow checks, with default, eval-harness, slow-tests and test-util
features. All 308 shared artifact profiles/features match; both compilers are
identical (saved toolchain-A/B.txt). A uses its own physical target/w/target.

| Pair | A fixture seconds | B fixture seconds | Wall B/A | CPU B/A | A load 1m | B load 1m |
| --- | ---: | ---: | ---: | ---: | --- | --- |
| pair1 | 2970.71 | 3265.89 | 1.0993 | 1.0707 | 2.31 → 23.97 | 23.97 → 3.69 |
| pair2 | 2530.72 | 2519.59 | 0.9956 | 0.9971 | 3.28 → 3.11 | 3.11 → 1.75 |

Each run exits zero and passes this exact work/recovery summary:

```text
C5_FIX1_MAXIMAL clocks=720 tasks=8 sends=128 attempts=24 max_depth=7 stop=true cleanup=true reopen_exact=true
```

Both B runs gate the final-source maximal fixture. Their durations are **3265.89 s**, **2519.59 s**,
each 1 passed / 0 failed / 0 ignored. The managed processes are awaited directly
through exit, with no other build or test from this worker during timing. The
ignored library test is invoked directly with its full exact name and
`--exact --ignored --nocapture`; build and shared-lock wait are outside the timers.

The historical fivefold TE1.1 regression is not reproduced. Current base runs
already exceed the historical 489.30 s. These data support run/environment variance
in that old comparison; they do not isolate CPU load as its sole cause or establish
a statistically significant speedup from any individual removal.

### Recorded load and counters

| Run / boundary | UTC | `uptime` output | Raw `/proc/loadavg` |
| --- | --- | --- | --- |
| pair1-A start | 2026-11-03T20:41:01.757644+00:00 | `15:41:01 up 20:53,  1 user,  load average: 2.31, 3.95, 4.67` | `2.31 3.95 4.67 3/2419 617641` |
| pair1-A end | 2026-11-03T21:30:32.875378+00:00 | `16:30:32 up 21:43,  1 user,  load average: 23.97, 12.83, 11.83` | `23.97 12.83 11.83 22/3001 1264900` |
| pair1-B start | 2026-11-03T21:30:33.246201+00:00 | `16:30:33 up 21:43,  1 user,  load average: 23.97, 12.83, 11.83` | `23.97 12.83 11.83 27/2972 1265127` |
| pair1-B end | 2026-11-03T22:24:59.275619+00:00 | `17:24:59 up 22:37,  1 user,  load average: 3.69, 11.54, 14.88` | `3.69 11.54 14.88 2/2910 2967092` |
| pair2-A start | 2026-11-03T22:25:16.333282+00:00 | `17:25:16 up 22:37,  1 user,  load average: 3.28, 10.94, 14.61` | `3.28 10.94 14.61 2/2883 2973783` |
| pair2-A end | 2026-11-03T23:07:27.151143+00:00 | `18:07:27 up 23:20,  1 user,  load average: 3.11, 2.99, 3.63` | `3.11 2.99 3.63 2/2473 4079015` |
| pair2-B start | 2026-11-03T23:07:27.215094+00:00 | `18:07:27 up 23:20,  1 user,  load average: 3.11, 2.99, 3.63` | `3.11 2.99 3.63 3/2472 4079054` |
| pair2-B end | 2026-11-03T23:49:26.889093+00:00 | `18:49:26 up 1 day, 2 min,  1 user,  load average: 1.75, 1.76, 2.05` | `1.75 1.76 2.05 1/2415 975782` |

Snapshots also retain CPU affinity/count and CPU/I/O pressure. The following are
last own-PID samples; dividing by 128 is an aggregate per-fixture-send measure,
not instrumentation of individual sends, tool calls or fsyncs.

| Run | Logical write bytes | Write calls | Logical bytes / send | Calls / send | Sample to end snapshot, seconds |
| --- | ---: | ---: | ---: | ---: | ---: |
| pair1-A | 566,186,429 | 19,264 | 4,423,331.48 | 150.50 | 0.829 |
| pair1-B | 567,210,215 | 19,307 | 4,431,329.80 | 150.84 | 0.150 |
| pair2-A | 566,160,159 | 19,229 | 4,423,126.24 | 150.23 | 1.021 |
| pair2-B | 567,182,425 | 19,222 | 4,431,112.70 | 150.17 | 0.931 |

Store verification counters, full process I/O samples, exact CPU/wall timers,
matched build JSON and all fixture logs are retained under
`target/te1-fix2-wake/ab/`, indexed by `results.json`. Verification calls/bytes are
not cumulative-store write or fsync counts. Fsync and observer records per send
were not separately instrumented. No new per-tool durable publication is added.

All 716 gated source inputs and both binary hashes match before and after timing.
The B library-test binary SHA-256 is `7d96a2f233c8690c40cec00032baeb8dc8e3fcf2516218eb28a9b76d4d087b7e`; A is
`bf51976cc5c5881a983d45cb6618ca23c09ebab9a6065a136c82499c50431645`.
The review copy is checked clean at b4e9abc8 before deletion, then removed with
its independent target. The shared lock file is retained. No product source
changes after timing. Final source, restore and cleanup proofs are under
`target/te1-fix2-wake/`.

Only maximal fixtures use managed background execution and are awaited directly,
as explicitly approved. Build and lock wait are outside the paired timers. Each
run records start/end uptime and raw loadavg plus own-PID CPU/I/O samples and
requires clocks 720, tasks 8, sends 128, attempts 24, depth 7, successful stop,
cleanup and exact reopen. A repeat is required for wall B/A at least 1.05 or a
CPU/wall disagreement about the approximate 1.10 upper budget.

## File list

- `crates/kinewright-agent/src/server.rs`
- `crates/kinewright-agent/src/writer_host/hooks_tests.rs`
- `crates/kinewright-agent/src/writer_host/native_invocation.rs`
- `crates/kinewright-agent/src/writer_host/native_spending.rs`
- `crates/kinewright-agent/src/writer_host/native_wake.rs`
- `crates/kinewright-agent/src/writer_host/task_mcp.rs`
- `crates/kinewright-agent/src/writer_host/te1_prompt_tests.rs`
- `crates/kinewright-agent/src/writer_host/te1_tests.rs`
- `crates/kinewright-agent/src/writer_host/te1_wake_tests.rs`
- `crates/kinewright-agent/tests/fixtures/te1-director-wake-complete.json`
- `crates/kinewright-agent/tests/fixtures/te1-director-wake-pending.json`
- `crates/kinewright-agent/tests/te1_usage.rs`
- `crates/kinewright-project/src/orchestration_accounting.rs`
- `docs/reports/te1-1-fix1.md`
- `docs/reports/te1-1-fix2.md`
- `docs/reports/te1-1.md`
- `scripts/te1-mutations.py`

## Limits

No installed Codex/Claude execution, paid witness, credentials, C6 qualification,
Windows runtime, release refresh or final-head hosted CI. Full workspace runners
and ignored equipped-session tests remain CI scope; the changed-crate full suites
and prescribed Windows script are local gates. No subagents or independent review
agents, under the explicit implementation-worker brief. No push.
