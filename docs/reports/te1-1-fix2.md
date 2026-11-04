# TE1.1 fix 2

The latest spending qualification is the [Grok follow-up](te1-1-fix2-grok.md).
The sections below record the earlier wake and GLM rounds.

Fix-2 work is published as afda1f0d and 0845690a. The latter has the exact tree
of the worker's earlier local replacement 30296bce; it preserves the published
history. The GLM follow-up is published as 67c41a73 above 0845690a.
The unpushed performance commit 2e8acc88 was consolidated into that fix-2 work.
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
and size observations and their corresponding publications. Initial start, first
final, a size-only publication, another start and a recovered final produce pending
and recovered
wakes. An unpinned usage-bearing event with a kind longer than 64 characters
then exercises the exact base truncation and kind label. No identities or metadata
are removed or normalized in the comparison.

| Full base prompt | Bytes | SHA-256 |
| --- | ---: | --- |
| Pending child | 5494 | 343395316528fb70c60003430447de1fff922b8b01c28d8bd79af7b8278e7cd1 |
| Recovered child | 5892 | 46faed1cf29baa5523b6f106dc4916a7b7b3dd08f5ee0293dd840cd6bd36426d |
| Unpinned kind | 6345 | 7c5e495f5c103258f2977dce3f8d3aa8bd132ce09c5171ddb4021f71aa674f2b |

The earlier pre-fix full-prompt witness failed, 0 passed / 1 failed (historical
logs under `target/te1-fix2-wake/`). The final-source golden matches all three
base captures and all four full-wake mutants fail before restoration. The earlier
accounting-body golden remains a separate boundary and is retained. The base capture, harness patch, driver, exact-source
fragment hash and logs are under `target/te1-fix2-muse/`.

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

Muse approves the original spending fixes. New Minors 1–3 are included here:

- Native usage identities retain the actual provider kind, truncated to the base
  limit of 64 characters, alongside the ordinal. The complete production wake
  restores its base hash identity. The full-prompt golden includes an unpinned
  usage-bearing kind and a mutation that substitutes `provider_usage`.
- The provider view normalizes totals after stripping starts. The pending golden
  follows a completed result with another start: authoritative coverage is
  incomplete, while displayed samples and totals match the complete base wake.
- Validation borrows stream prefixes instead of cloning the entire task.

New Minors 4 and 5 are explicitly deferred in
[the first fix report](te1-1-fix1.md#deferred-minors-from-muses-recheck-of-9a2b93b2).
The unpushed size-recording changes previously included in fix 2 were reverted to
9a2b93b2 as directed. No native size portability/race fix or resume route is added.

## Mutation pairs

All 29 final-source mutation pairs pass. Each RED executes exactly one witness
and fails it; each byte-identically restored GREEN executes exactly that witness
and passes it. The runner checks restored bytes and SHA-256 before and after
GREEN. All subject files match their backups after the last pair. Full proofs,
byte backups and RED/GREEN logs are in `target/te1-fix2-glm/mutations/`, indexed
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
| `full-wake-kind` | 0 passed / 1 failed | 1 passed / 0 failed |
| `raw-overflow-claude` | 0 passed / 1 failed | 1 passed / 0 failed |
| `raw-overflow-codex` | 0 passed / 1 failed | 1 passed / 0 failed |
| `cross-platform-instructions` | 0 passed / 1 failed | 1 passed / 0 failed |

The first five additions to the original 21 are complete-wake reference projection,
complete-wake revision, stripped wake totals, durable observer-counter regression
and the unpinned native kind. Original size-loss witnesses remain; deferred
size-teardown and off-Linux corrections have no fix-2 mutation claims. The three
GLM pairs verify nonfatal overflow for each provider and instruction-byte
protection, bringing the current total to 29.

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

All 718 pinned crate/source/fixture, Cargo and gate-helper files match
after the gates. The manifest SHA-256 is
`371b7509138b54b77d5272b7daf6a810950ce76f6dada34951dcb569e544e617`.
Exact logs, start/end stamps, exit files and counts are under
`target/te1-fix2-glm/`. Earlier fix-2 evidence is historical.

The commands remain the original TE1.1 full gate commands. Every Cargo command
sources FFmpeg in the same shell and uses the shared flock, four build jobs and
nice 10; test threads are four. Python uses nice 19 and isolated mode. TMPDIR and
LOCALAPPDATA are under the owning physical target.

## Paired accounting qualification at 30296bce

The pair at `30296bce` passes the approximate 10% upper budget. No repeat was required.
No pair waited for a quiet machine. Both selected
library-test binaries use opt-level 0, line-tables-only debug information, debug
assertions and overflow checks, with default, eval-harness, slow-tests and test-util
features. All 308 shared artifact profiles/features match; both compilers are
identical (saved toolchain-A/B.txt). A uses its own physical target/w/target.

| Pair | A fixture seconds | B fixture seconds | Wall B/A | CPU B/A | A load 1m | B load 1m |
| --- | ---: | ---: | ---: | ---: | --- | --- |
| pair1 | 2551.17 | 2512.70 | 0.9849 | 0.9857 | 4.43 → 1.98 | 1.98 → 1.41 |

Each run exits zero and passes this exact work/recovery summary:

```text
C5_FIX1_MAXIMAL clocks=720 tasks=8 sends=128 attempts=24 max_depth=7 stop=true cleanup=true reopen_exact=true
```

The `30296bce` B maximal passes in **2512.70 s**: 1 passed / 0 failed / 0 ignored.
The managed processes were awaited directly through exit, with no other build or test from this worker during timing. The
ignored library test is invoked directly with its full exact name and
`--exact --ignored --nocapture`; build and shared-lock wait are outside the timers.

The historical fivefold TE1.1 regression is not reproduced. Current base runs
already exceed the historical 489.30 s. These data support run/environment variance
in that old comparison; they do not isolate CPU load as its sole cause or establish
a statistically significant speedup from any individual removal.

### Recorded load and counters

| Run / boundary | UTC | `uptime` output | Raw `/proc/loadavg` |
| --- | --- | --- | --- |
| pair1-A start | 2026-11-04T00:44:43.977657+00:00 | `19:44:43 up 1 day, 57 min,  1 user,  load average: 4.43, 3.24, 3.16` | `4.43 3.24 3.16 2/2444 3366881` |
| pair1-A end | 2026-11-04T01:27:15.238396+00:00 | `20:27:15 up 1 day,  1:39,  1 user,  load average: 1.98, 1.70, 1.77` | `1.98 1.70 1.77 2/2335 276269` |
| pair1-B start | 2026-11-04T01:27:15.762463+00:00 | `20:27:15 up 1 day,  1:39,  1 user,  load average: 1.98, 1.70, 1.77` | `1.98 1.70 1.77 1/2334 276535` |
| pair1-B end | 2026-11-04T02:09:08.605140+00:00 | `21:09:08 up 1 day,  2:21,  1 user,  load average: 1.41, 1.57, 1.67` | `1.41 1.57 1.67 1/2362 1359893` |

Snapshots also retain CPU affinity/count and CPU/I/O pressure. The following are
last own-PID samples; dividing by 128 is an aggregate per-fixture-send measure,
not instrumentation of individual sends, tool calls or fsyncs.

| Run | Logical write bytes | Write calls | Logical bytes / send | Calls / send | Sample to end snapshot, seconds |
| --- | ---: | ---: | ---: | ---: | ---: |
| pair1-A | 565,989,856 | 19,240 | 4,421,795.75 | 150.31 | 0.509 |
| pair1-B | 567,187,597 | 19,271 | 4,431,153.10 | 150.55 | 0.116 |

Store verification counters, full process I/O samples, exact CPU/wall timers,
matched build JSON and all fixture logs are retained under
`target/te1-fix2-muse/ab/`, indexed by `results.json`. Verification calls/bytes are
not cumulative-store write or fsync counts. Fsync and observer records per send
were not separately instrumented. No new per-tool durable publication is added.

All 717 gated inputs and both binary hashes matched before and after the
30296bce timing. Later source changes are recorded in the GLM follow-up below.
The B library-test binary SHA-256 is `b2967556471546807d16de19c2909d990db2c2123bf5260f970a80aa0be09cc0`; A is
`b32b2cda7a2603f8eeba2ff1dcf1141559f27ce47fcedc40b24cd43486c42df8`.
The review copy is checked clean at b4e9abc8 before deletion, then removed with
its independent target. The shared lock file is retained. Final source, restore and cleanup proofs are under
`target/te1-fix2-muse/`.

Only maximal fixtures use managed background execution and are awaited directly,
as explicitly approved. Build and lock wait are outside the paired timers. Each
run records start/end uptime and raw loadavg plus own-PID CPU/I/O samples and
requires clocks 720, tasks 8, sends 128, attempts 24, depth 7, successful stop,
cleanup and exact reopen. A repeat is required for wall B/A at least 1.05 or a
CPU/wall disagreement about the approximate 1.10 upper budget.

## GLM follow-up and final-source maximal

GLM 2 is already covered by Muse's Minor 7 fix in 9a2b93b2: observation refusal
is an Error event, not an AgentError returned from parsing. Both parsers preserve
Done and suppress the refused Cost projection. Evaluation retains known usage
and marks it incomplete. No observer/parser runtime change is made here.

The new two-provider regression fills 1,024 observations, refuses the next one,
and completes a third send. A separate branch refuses a 33-model Claude result.
All retain the 168-token lower bound, incomplete coverage, no price claim and no
session interruption. This test passed against unchanged production code before
the instruction-constant move. Two new mutants restore fatal parser errors and
must fail this witness.

GLM 3 is fixed: the unchanged 1,753-byte instruction literal lives in a shared
module, compiled for Linux production and all unit-test platforms. The surface
hash assertion has no Linux guard. The hash remains
`28b1932b2b866b88ecef978943e57ed3546685f29f1a35ab016b19aaab117d0f`.
A third new mutant changes the literal and must fail the surface golden. Windows
compilation/lint, not Windows runtime, is the available local cross-platform gate.

GLM 1 remains the native-resume follow-up already logged in the first fix report.
GLM 4 needs no change. Muse's Minors 4/5 remain deferred as directed.

Accounting code and the native wake implementation are unchanged since 30296bce;
the paired measurements above qualify that implementation. The following maximal
run gates this final source after the test and constant-location changes.

The final-source maximal passes: **2502.26 seconds**, 1 passed / 0 failed /
0 ignored. The completed work is:

```text
C5_FIX1_MAXIMAL clocks=720 tasks=8 sends=128 attempts=24 max_depth=7 stop=true cleanup=true reopen_exact=true
```

It ran as a managed background process, directly awaited through exit. No other
build or test from this worker ran during timing. All 718 frozen inputs and the
binary hash match before and after this run; no product source changes followed.
The binary SHA-256 is `771794a6a3ebe5bae62533cfa7e792fd8606c4c5a82775f5d92d7bce43942b6d`.
Full gate, mutation, process/timer and source proofs are under
`target/te1-fix2-glm/`; the result is indexed by `final-proof.json`. The earlier
paired measurement remains explicitly attributed to 30296bce above.

| Maximal boundary | UTC | `uptime` | Raw `/proc/loadavg` |
| --- | --- | --- | --- |
| Start | 2026-11-04T03:03:43.014545+00:00 | `22:03:43 up 1 day,  3:16,  1 user,  load average: 1.34, 2.25, 2.81` | `1.34 2.25 2.81 6/2494 3697710` |
| End | 2026-11-04T03:45:25.355377+00:00 | `22:45:25 up 1 day,  3:58,  1 user,  load average: 2.32, 2.01, 1.90` | `2.32 2.01 1.90 1/2336 584836` |

## GLM follow-up file list

- `crates/kinewright-agent/src/eval.rs`
- `crates/kinewright-agent/src/server.rs`
- `crates/kinewright-agent/src/writer_host/codex_instructions.rs`
- `crates/kinewright-agent/src/writer_host/codex_prompt.rs`
- `crates/kinewright-agent/src/writer_host/mod.rs`
- `docs/reports/te1-1-fix2.md`
- `scripts/te1-mutations.py`

## Combined fix-2 file list

- `crates/kinewright-agent/src/eval.rs`
- `crates/kinewright-agent/src/server.rs`
- `crates/kinewright-agent/src/writer_host/codex_instructions.rs`
- `crates/kinewright-agent/src/writer_host/codex_prompt.rs`
- `crates/kinewright-agent/src/writer_host/hooks_tests.rs`
- `crates/kinewright-agent/src/writer_host/mod.rs`
- `crates/kinewright-agent/src/writer_host/native_invocation.rs`
- `crates/kinewright-agent/src/writer_host/native_spending.rs`
- `crates/kinewright-agent/src/writer_host/native_wake.rs`
- `crates/kinewright-agent/src/writer_host/te1_prompt_tests.rs`
- `crates/kinewright-agent/src/writer_host/te1_tests.rs`
- `crates/kinewright-agent/src/writer_host/te1_wake_tests.rs`
- `crates/kinewright-agent/tests/fixtures/te1-director-wake-complete.json`
- `crates/kinewright-agent/tests/fixtures/te1-director-wake-pending.json`
- `crates/kinewright-agent/tests/fixtures/te1-director-wake-unpinned.json`
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
