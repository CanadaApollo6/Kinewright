# TE1.1 review fixes and paired performance check

Follow-up to `25e4fe7dcc3f230f284322dbef4e853173b3ad33` on local branch `te1`.
This report supersedes the corresponding review findings in the original TE1.1 report;
that report remains a record of the original commit's gates. No push or provider CLI run.

The subsequent [fix-2 report](te1-1-fix2.md) completes the full wake metadata
compatibility correction, implements Muse's mandatory encode/clone removals, and
records final-source gates and paired timing. The body-only wake golden and
no-performance-patch statements below describe this first fix commit only.

## Paired performance result

A is `b4e9abc8884c0c0260ca4d85b79710b34867a37c`; B is `25e4fe7d`.
The order was A, B, A, B without waiting for a quiet machine. Both versions used
independent physical Cargo targets, identical profiles/features for all 261 shared
build targets, the same FFmpeg/toolchain, and the shared heavy-Cargo lock. Build
and lock wait were outside fixture timers. Every fixture passed.

| Pair | A seconds | B seconds | B/A | A load 1m, start → end | B load 1m, start → end |
| --- | ---: | ---: | ---: | --- | --- |
| 1 | 2589.32 | 2821.76 | 1.0898 | 2.57 → 8.81 | 8.81 → 3.10 |
| 2 | 2891.28 | 2481.27 | 0.8582 | 2.99 → 2.23 | 2.23 → 1.12 |

There is no reproducible TE1.1 performance regression: both B runs meet the
requested approximate 10% upper budget, and the direction reverses in the second
pair. Current A itself takes 5.29–5.91 times the historical 489.30 seconds. These
pairs implicate run/environment variance in the historical comparison; they do
not prove CPU load alone explains that absolute gap. No performance fix was made.

B logical write bytes rose 0.18% in both pairs; write syscalls rose 0.27% and 0.06%.
Verified store calls were 90,604 for A and 90,610 for B; verified bytes rose about
0.31%. These are process/store counters, not exact fsync or cumulative-publication
counts. Fsyncs and observer rows per send were not separately instrumented. The
maximal fixture uses scripted sends rather than TaskMcp result calls.

Raw evidence, exact start/end `uptime` and `/proc/loadavg`, CPU/I/O samples,
binary/source pins, commands, and full method are retained under `target/te1-ab/`.
The disposable base worktree and its independent target were removed after all
four runs and identity checks. The shared flock file was retained.

## Review fixes

- Astra Major 1: provider-specific result identity (`uuid` for Claude, `id` for
  Codex). Exact duplicates return before capacity consumption; changed payloads
  under the same identity refuse. Decode/normalization failure leaves the observer
  unchanged. Real-shaped Claude results cover ignored-body changes, exact replay,
  and replay at the 1,024-record capacity.
- Astra Major 2: normalize advanced per-model watermarks, then derive category
  completeness from the current snapshot. A=100/B=100 followed by A missing/B=150
  retains 250 known tokens and crosses the 225-token ceiling. Serialized restart,
  resumed binding, empty gap, model regression, and complete recovery to 300 are
  covered. Evaluation, per-result, and scoreboard totals consume the retained lower bound too.
  The scoreboard footer sums those bounds rather than reconstructing tokens from
  incomplete category fields. The same fake-driver fixture checks the exact 250-token
  footer, and an additional mutation restores the old zero-total defect.
- Astra Major 3: each accepted send creates pending coverage. Only a new complete
  raw observation while that send is active covers it. Done, missing final, and
  exact final replay do not clear it. Two-send fake drivers exercise both providers,
  persistent/fresh streams, missing first/last final, replay, and positive controls.
- Astra Minor 4: case 6 now trims linked video/audio clips 3 and 6 to source
  [61,100), timeline [51,90), preserving the caption at [52,90), one frame behind
  the media start. Source bounds, links, exact document, unchanged objects, undo,
  redo, reopen, dependency requirements, and production preview remain asserted.
- Muse Major 1: production wake accounting expansion restores base event hashes
  from retained payload hashes before stripping observer-only fields. Stored
  identities and host-owned usage references are unchanged. A real stored
  accounting record exercises this expansion and its frozen byte hash. This golden
  covers expanded accounting contents; it does not assert identical outer result
  ObjectRef hashes/lengths, which refer to the actual stored records.
- Muse Major 2: attributed by the completed A/B above. No performance patch or
  speculative accounting/store optimization. The per-model correctness repair also
  removes the old discarded watermark prepass.
- Muse Minors 1–2: preserve stream-wide cumulative integer cost separately from
  per-model token snapshots; reject regressions, retain known lower bounds through
  gaps, and keep missing cost unknown. Eval sums distinct stream estimates once and
  enforces max_cost_usd against the known lower bound. Legacy Cost is not added
  again. Missing final coverage removes the complete price claim. Claude result
  total_cost_usd is cumulative according to the [official cost documentation](https://code.claude.com/docs/en/agent-sdk/cost-tracking);
  these are provider estimates, not billing qualification.
- Muse Minor 3: incomplete stream reports leave direct_input unknown, consistently
  with inclusive input, uncached input, and output.
- Muse Minor 4: native result-size observation is owned by a bounded per-task Arc
  retained through host/run/task teardown. Detachment or poisoned state marks that
  observer incomplete; wire-level request/result counts also remain observable.
  The next legal publication merges the retained measurements without introducing
  a per-tool publication or affecting unrelated tasks. Real HTTP detach fixtures
  exercise missing task and missing run, including durable incomplete coverage.
  Additional retained-observer fixtures cover an unavailable weak host and a poisoned
  host mutex, preserving counts and the durable incomplete flag.
- Muse Minor 5: renamed test-local decision checks and assertions to explicitly
  describe scripted fixture requirements. They establish no live Director decision
  quality or future TE1.4 evidence-gate enforcement.
- Muse Minors 6–7: broader mutation coverage below. Generic usage refusal preserves
  result Done, marks usage incomplete, and suppresses the refused line's legacy
  Cost projection. Capacity remains bounded and identity collisions remain refused;
  no heuristic accepts hypothetical future provider identity changes.

## Frozen fixture hashes

| Artifact | SHA-256 |
| --- | --- |
| case-6.json | e5e897f2fc700ee4389dc890f398258b34dcc2968b274309dccdd2efe4dd0186 |
| case-6-oracle.kinewright | 9fe495da95517e498e4574b99b69925d317493275e5d562c9411941aa5851423 |
| 18-entry hashes.json manifest | 81d2057ea57670892ed9ddb77958edf9b828f698528bc66f2ce42c67c6b47f69 |
| expanded base accounting wake | 676cdb2a29742982f383277d7cc6ab41e3fe93ebf19ea2387dde016037a1203c |

The fixed instruction, full wrapper prompt, and tool/schema golden pins are unchanged.
The new wake pin is independently constructed from the base event algorithm and
legacy accounting shape, then compared with production store-backed expansion.

## Mutation pairs and gates

All full changed-crate suites pass, including binary, integration, and doc targets.

| Full suite | Passed | Failed | Ignored | Targets |
| --- | ---: | ---: | ---: | ---: |
| core | 815 | 0 | 1 | 20 |
| project | 426 | 0 | 2 | 18 |
| agent | 1299 | 0 | 70 | 28 |
| app | 971 | 0 | 5 | 1 |

Commands match the original brief:

```bash
cargo test -p kinewright-core -p kinewright-project --features kinewright-project/test-util --no-fail-fast
cargo test -p kinewright-agent --features eval-harness,slow-tests --no-fail-fast
cargo test -p kinewright-app --features slow-tests --no-fail-fast
cargo fmt -- --check
cargo clippy --workspace --all-targets -- -D warnings
bash docs/reports/ja1-s4b/c5-windows-clippy.sh
nice -n 19 python3 -I scripts/slow_tests.py lint
nice -n 19 python3 -I scripts/slow_tests_selftest.py
```

Every Cargo gate sources FFmpeg in the same shell and takes the prescribed
shared flock with four jobs and nice 10. Tests use four threads. TMPDIR and
LOCALAPPDATA stay under this worktree's target. Only the maximal fixture uses a
managed background process; it is awaited directly until exit as the user approved.

Formatting and workspace clippy pass. Windows compilation/lint passes both the
workspace all-targets and Agent all-targets slow-feature checks for MSVC; no Windows
runtime claim. Manifest lint reports 118 tests, 121 well-formed allowlist entries,
and 27 skip sites. All 44 Python self-tests pass. The full Agent suite includes the
fix-2/10/13/14 replay, publication, settlement, archive, and terminal authority
regressions, both architecture scans, the three surface goldens, raw adapters,
coverage, price ceilings, corpus rendering, and real MCP result-size witnesses.

The 656-file source/Cargo/fixture manifest SHA-256 is `3d8457aff93120735c7b4723924cbbd2919c35f55d9cc50e245251623a35d3ed`.
It is checked after the full gates, after all final mutation restores, and before
and after the maximal fixture. Runtime evidence is under `target/te1-fix1/`.

All 17 final-source mutation pairs pass. Every RED invocation executes exactly one
witness and fails it (0 passed / 1 failed); every restored GREEN invocation executes
exactly one witness and passes it (1 passed / 0 failed). The runner compares restored
bytes directly with the backup and verifies SHA-256 before and after GREEN. All
current subject files match those backups, and all 656 gated source files match
the final manifest after the last restore.

| Mutation | RED | Byte-identical restore / GREEN |
| --- | --- | --- |
| Add a cumulative replay again | 0 passed / 1 failed | 1 passed / 0 failed |
| Accept an unproven stream reset | 0 passed / 1 failed | 1 passed / 0 failed |
| Read Claude id instead of uuid | 0 passed / 1 failed | 1 passed / 0 failed |
| Discard retained per-model values | 0 passed / 1 failed | 1 passed / 0 failed |
| Reconstruct scoreboard tokens from incomplete categories | 0 passed / 1 failed | 1 passed / 0 failed |
| Drop accepted-send pending coverage | 0 passed / 1 failed | 1 passed / 0 failed |
| Ignore the ledger payload hash | 0 passed / 1 failed | 1 passed / 0 failed |
| Ignore observer payload-only changes | 0 passed / 1 failed | 1 passed / 0 failed |
| Accept a per-model regression | 0 passed / 1 failed | 1 passed / 0 failed |
| Treat a missing category as complete | 0 passed / 1 failed | 1 passed / 0 failed |
| Expose ordinal event IDs in wakes | 0 passed / 1 failed | 1 passed / 0 failed |
| Expose observer snapshots in wakes | 0 passed / 1 failed | 1 passed / 0 failed |
| Discard global cost | 0 passed / 1 failed | 1 passed / 0 failed |
| Disable the raw cost ceiling | 0 passed / 1 failed | 1 passed / 0 failed |
| Expose incomplete direct input as known | 0 passed / 1 failed | 1 passed / 0 failed |
| Drop the retained native incomplete flag | 0 passed / 1 failed | 1 passed / 0 failed |
| Drop the wire-level incomplete flag | 0 passed / 1 failed | 1 passed / 0 failed |

The three Astra Majors have their own targeted pairs (`claude-uuid`,
`model-watermarks`, `pending-sends`). The case-6 no-trim witness failed before the
fixture repair (0 passed / 1 failed) and the repaired complete corpus passes both
operation/history and production-preview tests. Additional before-fix logs cover
UUID capacity, recovery, cost, wake contents, direct-input coverage, native size
loss, usage capacity liveness, and refused-raw projection.

Final pair evidence: `target/te1-fix1/mutations/pairs.json`, individual JSON proofs,
byte backups, and RED/GREEN logs. Superseded development runs are archived under
`mutations-before-lint` and `mutations-before-scoreboard`; final claims use only the
17-pair directory above.

The final maximal fixture passes: **2450.87 seconds (40m 50.87s)**, exit 0,
1 passed / 0 failed / 0 ignored. Wrapper start/end: `2026-11-03T14:15:19Z` → `2026-11-03T14:56:10Z`;
`wrapper_real_seconds=2451.097 user_seconds=2260.007 sys_seconds=10.254` includes Cargo startup and any shared-lock wait.

```text
C5_FIX1_MAXIMAL clocks=720 tasks=8 sends=128 attempts=24 max_depth=7 stop=true cleanup=true reopen_exact=true
```

Command (through the same FFmpeg/flock/four-job/nice wrapper):

```bash
cargo test -p kinewright-agent --features eval-harness,slow-tests --lib writer_host::hooks_tests::tests::s4b_c5e_maximal_run_keeps_depth_stop_and_recovery_bounded -- --exact --ignored --nocapture
```

The managed background process was awaited directly through exit 0, as the user
approved. No build/test remained running. No source or fixture changed after the
maximal launch; all 656 files match the gated source manifest before and after it.
Final library-test binary SHA-256: `b8e3ac9971e5bf2e19ed49eb112c8e2826e6d83648d54e4f20babeab2b99e888`.
The four A/B runs remain comparisons of the original pinned A/B sources; this
additional run qualifies the review fixes at the final source and is unpaired.


## File list

- `.gitattributes`
- `crates/kinewright-agent/src/eval.rs`
- `crates/kinewright-agent/src/orchestration/usage.rs`
- `crates/kinewright-agent/src/protocol.rs`
- `crates/kinewright-agent/src/server.rs`
- `crates/kinewright-agent/src/usage_observer.rs`
- `crates/kinewright-agent/src/writer_host/c4_tests.rs`
- `crates/kinewright-agent/src/writer_host/hooks_grant_tests.rs`
- `crates/kinewright-agent/src/writer_host/native_invocation.rs`
- `crates/kinewright-agent/src/writer_host/native_spending.rs`
- `crates/kinewright-agent/src/writer_host/orchestration.rs`
- `crates/kinewright-agent/src/writer_host/task_mcp.rs`
- `crates/kinewright-agent/src/writer_host/te1_prompt_tests.rs`
- `crates/kinewright-agent/src/writer_host/te1_tests.rs`
- `crates/kinewright-agent/tests/fixtures/te1-wake.json`
- `crates/kinewright-agent/tests/fixtures/te1/case-6-oracle.kinewright`
- `crates/kinewright-agent/tests/fixtures/te1/case-6.json`
- `crates/kinewright-agent/tests/fixtures/te1/hashes.json`
- `crates/kinewright-agent/tests/te1_corpus.rs`
- `crates/kinewright-agent/tests/te1_usage.rs`
- `crates/kinewright-core/src/agent.rs`
- `crates/kinewright-project/src/orchestration_accounting.rs`
- `crates/kinewright-project/src/usage_stream.rs`
- `docs/reports/te1-1-fix1.md`
- `scripts/te1-mutations.py`

## Limits

No paid witnesses, installed Codex/Claude execution, credentials, C6 qualification,
release evidence refresh, Windows runtime execution, or final-head hosted CI.
Full workspace fast/slow runners and ignored equipped-session witnesses remain CI
scope as in the original brief; full changed-crate suites and the required Windows
compile/lint script are local gates. No subagents were used. No push.
