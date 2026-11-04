# TE1.1 fix 2: Grok spending follow-up

This checks the blind review of 25e4fe7d against the implementation at 67c41a73.
The confirmed Major is fixed at the raw usage boundary. The two smaller accounting
findings were already fixed; new regressions pin their exact scenarios. The corpus
history witness now uses a production fork from the person's actual saved cut.
No provider executable or paid witness was run. Review handoff is the full
spending-tier panel, owned by the lead; no review agents were started here.

## Pinned upstream verification

The Codex claim is confirmed from [the emitter at rust-v0.166.0](https://github.com/openai/codex/blob/rust-v0.166.0/codex-rs/exec/src/event_processor_with_jsonl_output.rs#L118-L128).
Its Git blob SHA is `a92f762096ad08543eaef6d9d8cd101f23a9f182`. Lines 119–121 say:

```rust
let Some(usage) = self.last_total_token_usage.as_ref() else {
    return Usage::default();
};
```

[Lines 511–513](https://github.com/openai/codex/blob/rust-v0.166.0/codex-rs/exec/src/event_processor_with_jsonl_output.rs#L511-L513)
store the token notification without emitting a JSONL usage event:

```rust
self.last_total_token_usage = Some(notification.token_usage);
```

[Lines 535–536](https://github.com/openai/codex/blob/rust-v0.166.0/codex-rs/exec/src/event_processor_with_jsonl_output.rs#L535-L536)
put that result in the completed event:

```rust
usage: self.usage_from_last_total(),
```

[The same tag's Usage definition](https://github.com/openai/codex/blob/rust-v0.166.0/codex-rs/exec/src/exec_events.rs#L59-L73)
derives Default and Serialize over five i64 token fields, without a zero-skipping
serializer. Default therefore serializes a present five-zero usage object.
The type-definition blob SHA is `06565c097b28d3bf2ad592943cc30c776b45e236`.
Fetched files were treated as untrusted reference text and never executed.
The saved bytes reproduce both Git blob hashes. Source and provenance are under
`target/te1-fix2-grok/upstream/`.

Claude's [documented ModelUsage contract](https://code.claude.com/docs/en/agent-sdk/typescript#modelusage)
uses numeric counters without a nonzero constraint. I did not establish a pinned
closed-source CLI fallback equivalent to Codex's. As a conservative inference from
that accepted input shape, the same all-zero-final rule applies if such a Claude
result arrives. This is fixture coverage, not live Claude qualification. The
[pinned SDK changelog](https://github.com/anthropics/claude-agent-sdk-typescript/blob/v0.3.316/CHANGELOG.md#L3-L6)
identifies SDK 0.3.316 as matching Claude Code 2.1.316.

## Major: zero finals cannot establish known spend

`decode_usage` now classifies an all-zero final for either versioned provider as
unobserved token data. It marks the sample incomplete and clears token values to
None, including model snapshot values. Model identities and the raw payload hash
remain, so replay and changed-payload checks keep their authority. Price parsing
still runs independently after this classification.

An initial placeholder has an exact lower bound of 0, but input/output and the
reported total are unknown. A placeholder after known usage retains the 79-token
lower bound, marks coverage incomplete, and accepts a later 100-token recovery.
A genuinely positive one-token result remains complete. Individual zero
categories and unused zero-valued models alongside positive usage remain valid.

Native EOF sees the incomplete final and appends its ordinary gap. Under a
positive ceiling this records UsageUnavailable; consume_send refuses another send.
Before EOF, consume_send already refuses because the observed total is incomplete.
The generic driver also keeps an accepted send uncovered, with no complete price
claim and no fabricated parser failure.

The source-derived Codex JSON fixture has all five counters present at zero:
`tests/fixtures/te1-codex-zero-final.json`, SHA-256
`0ffb9d4b8c9e881428e03adc8197320dbf3be1a2fd9430b3b86e8c4ce37c90d3`.
It is byte-pinned by the repository's existing te1-*.json -text rule.

Fail-before evidence, each exactly one executed witness:

```text
te1_zero_final_after_accepted_send_is_unobserved_and_refuses_next_send
TE1_ZERO_FINAL_IS_UNOBSERVED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 14 filtered out; finished in 0.00s

te1_native_zero_final_keeps_eof_gap_open_under_a_ceiling
TE1_ZERO_CANNOT_SUPPRESS_EOF
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 987 filtered out; finished in 1.78s
```

The restored adapter and native witnesses pass, respectively:

```text
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 14 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 988 filtered out; finished in 2.01s
```

After the fix, 15 usage tests, three corpus tests and 26 focused library tests
pass. The final-source full gates and mutation proofs are below. Existing
nonzero prompt, schema, tool and complete-wake goldens remain unchanged. The
placeholder's accounting representation intentionally changes to unknown; this
is the spending correction requested by the review.

## Minors checked at head

- **Evaluation lower bound: already fixed.** SessionMetrics.total_tokens reads
  authoritative lower bounds, and evaluate_budgets calls it. The new exact fixture
  observes input 100/output 10, then missing input/output 18. The retained total is
  118, coverage is incomplete, and a token budget of 20 fails with `118 <= 20`.
- **Observer rollback: already fixed.** A failed normalization pops the sample
  before returning. A new two-provider witness observes 100, refuses 50, then
  successfully observes a complete 200 under a fresh identity. No observer runtime
  change is needed for this finding.
- **Manual redo: the old assertion was vacuous.** Its separate manual Core was
  untouched by the task test. That assertion is removed. The new Linux case-1
  history witness saves the person's MoveMarker/Undo history through
  Ja1EditingSession, forks the production task branch from that exact checkpoint,
  reads the checkpoint's full HistorySnapshot and compares it with the person's
  original cut, including redo. It applies the actual case-1 batch to that task,
  checks task undo/redo and durable publication, then checks the person's entire
  history remains exact and executes the person's Redo successfully.

Case 1 only edits markers. Its history witness uses those fixture/oracle markers
without the legacy media records, which lack current owned-media provenance. The
separate full corpus test still checks every original document and unchanged clip,
all operations, hashes, reopen behavior and production preview. Task and person
share an authenticated source history through the real project fork; they do not
apply competing edits to one Core, where ordinary DoBatch intentionally clears
redo. The history witness tests the production branch-isolation contract.

All 18 corpus hashes are unchanged. In particular:

| File | SHA-256 |
| --- | --- |
| case-1.json | 925f71204be909a40d6667fa9bc71d4ea9c7a88ec4bf896bc9fa62a448f1ce2c |
| case-1-oracle.kinewright | 15f458ca1a54bc2e073edaa81b6b740f0871999ea893f5dcf89fc600e67d96c1 |

## Mutation pairs

All 35 final-source mutation pairs pass. Each RED executes exactly one witness
and fails it; each byte-identically restored GREEN executes exactly that witness
and passes it. The runner checks restored bytes and SHA-256 before and after
GREEN. All subject files match their backups after the last pair. Full proofs,
byte backups and RED/GREEN logs are in `target/te1-fix2-grok/mutations/`, indexed
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
| `zero-final-codex` | 0 passed / 1 failed | 1 passed / 0 failed |
| `zero-final-claude` | 0 passed / 1 failed | 1 passed / 0 failed |
| `zero-final-eof` | 0 passed / 1 failed | 1 passed / 0 failed |
| `budget-lower-bound` | 0 passed / 1 failed | 1 passed / 0 failed |
| `observer-regression-rollback` | 0 passed / 1 failed | 1 passed / 0 failed |
| `corpus-manual-fork` | 0 passed / 1 failed | 1 passed / 0 failed |

The six additions are separate Codex/Claude zero-final regressions, native EOF
gap preservation, the exact lower-bound budget, observer rollback and production
fork isolation. The last mutates the real project fork to reuse the manual actor;
the new corpus history witness must fail. Every previous spending/wake pair stays.

## Full gates

| Full suite | Passed | Failed | Ignored | Targets |
| --- | ---: | ---: | ---: | ---: |
| core | 815 | 0 | 1 | 20 |
| project | 426 | 0 | 2 | 18 |
| agent | 1311 | 0 | 70 | 28 |
| app | 971 | 0 | 5 | 1 |

All 3,523 tests pass with zero failures and 78 ignored across 67 targets. Formatting
and Linux workspace all-targets clippy pass. The Windows script passes workspace
all-targets and Agent slow-feature all-targets compilation/lint for MSVC. Manifest
lint reports 118 tests, 121 well-formed allowlist entries and 27 equipped-session
skip sites. All 44 Python self-tests pass. Windows runtime was not exercised.

All 719 pinned crate/source/fixture, Cargo and gate-helper files match
after the gates. The manifest SHA-256 is
`9cfb4369e38c4b6228a2e65d61bb7ce5c8950dbf263e66835267f4ee66e0396a`.
Exact logs, start/end stamps, exit files and counts are under
`target/te1-fix2-grok/`. Earlier fix-2 evidence is historical.

Cargo sources FFmpeg in the same shell and uses the prescribed shared flock,
four build jobs and nice 10. Python uses nice 19 and isolated mode. Temporary and
LOCALAPPDATA paths stay under the owning worktree's target.

## Final maximal and A/B

The final-source pair passes the approximate 10% upper budget. No repeat was required.
No pair waited for a quiet machine. Both selected
library-test binaries use opt-level 0, line-tables-only debug information, debug
assertions and overflow checks, with default, eval-harness, slow-tests and test-util
features. All 308 shared artifact profiles/features match; both compilers are
identical (saved toolchain-A/B.txt). A uses its own physical target/w/target.

| Pair | A fixture seconds | B fixture seconds | Wall B/A | CPU B/A | A load 1m | B load 1m |
| --- | ---: | ---: | ---: | ---: | --- | --- |
| pair1 | 2540.32 | 2520.09 | 0.9920 | 0.9932 | 3.21 → 1.64 | 1.64 → 1.72 |

Each run exits zero and passes this exact work/recovery summary:

```text
C5_FIX1_MAXIMAL clocks=720 tasks=8 sends=128 attempts=24 max_depth=7 stop=true cleanup=true reopen_exact=true
```

The final-source B maximal passes in **2520.09 s**: 1 passed / 0 failed / 0 ignored.
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
| pair1-A start | 2026-11-04T05:03:33.873812+00:00 | `00:03:33 up 1 day,  5:16,  1 user,  load average: 3.21, 2.99, 2.98` | `3.21 2.99 2.98 1/2448 3546232` |
| pair1-A end | 2026-11-04T05:45:54.346352+00:00 | `00:45:54 up 1 day,  5:58,  1 user,  load average: 1.64, 1.75, 1.82` | `1.64 1.75 1.82 2/2353 449911` |
| pair1-B start | 2026-11-04T05:45:54.659319+00:00 | `00:45:54 up 1 day,  5:58,  1 user,  load average: 1.64, 1.75, 1.82` | `1.64 1.75 1.82 2/2352 450114` |
| pair1-B end | 2026-11-04T06:27:54.837637+00:00 | `01:27:54 up 1 day,  6:40,  1 user,  load average: 1.72, 1.70, 1.69` | `1.72 1.70 1.69 2/2342 1537327` |

Snapshots also retain CPU affinity/count and CPU/I/O pressure. The following are
last own-PID samples; dividing by 128 is an aggregate per-fixture-send measure,
not instrumentation of individual sends, tool calls or fsyncs.

| Run | Logical write bytes | Write calls | Logical bytes / send | Calls / send | Sample to end snapshot, seconds |
| --- | ---: | ---: | ---: | ---: | ---: |
| pair1-A | 565,988,589 | 19,243 | 4,421,785.85 | 150.34 | 0.733 |
| pair1-B | 567,015,450 | 19,240 | 4,429,808.20 | 150.31 | 0.422 |

Store verification counters, full process I/O samples, exact CPU/wall timers,
matched build JSON and all fixture logs are retained under
`target/te1-fix2-grok/ab/`, indexed by `results.json`. Verification calls/bytes are
not cumulative-store write or fsync counts. Fsync and observer records per send
were not separately instrumented. No new per-tool durable publication is added.

All 719 gated source inputs and both binary hashes match before and after timing.
The B library-test binary SHA-256 is `f4981c437919af6967265ccea7acb320fe11a22060c166c0c5c69c630768c8a2`; A is
`cefe1d33f4e4788fe005fdc4ecd5adefefbe529031cec57421757e6690753e89`.
The review copy is checked clean at b4e9abc8 before deletion, then removed with
its independent target. The shared lock file is retained. No product source
changes after timing. Final source, restore and cleanup proofs are under
`target/te1-fix2-grok/`.

Accounting changed, so maximal is run again on the final source and paired with
b4e9abc8. A has its own physical Cargo target. The pair runs back to back, without
waiting for a quiet machine. It repeats for wall B/A >=1.05 or CPU/wall disagreement
about the approximate 1.10 upper budget. Builds and lock waits are outside timers.
Uptime and raw /proc/loadavg are recorded at every start and end. Maximal runs use
managed background processes, directly awaited through exit; all other gates run
in the foreground. No other build or test from this worker runs during timing.

## Files and limits

This is a new local follow-up on published 67c41a73. Published history is
preserved. The worker makes no push. The commit changes these files:

- `crates/kinewright-agent/src/eval.rs`
- `crates/kinewright-agent/src/orchestration/usage.rs`
- `crates/kinewright-agent/src/usage_observer.rs`
- `crates/kinewright-agent/src/writer_host/te1_tests.rs`
- `crates/kinewright-agent/tests/fixtures/te1-codex-zero-final.json`
- `crates/kinewright-agent/tests/te1_corpus.rs`
- `crates/kinewright-agent/tests/te1_usage.rs`
- `docs/reports/te1-1-fix2-grok.md`
- `docs/reports/te1-1-fix2.md`
- `scripts/te1-mutations.py`

No installed Codex/Claude execution, paid witness, credentials, C6 qualification,
Windows runtime, release refresh, final-head hosted CI or independent panel review
was run here. Full workspace runners and ignored equipped-session tests remain CI
scope. The full changed-crate suites and prescribed Windows compile/lint script
are local gates. No subagents. No push. Muse's deferred native-size and future
native-resume Minors remain in te1-1-fix1.md as instructed.
