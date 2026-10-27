# JA-1 S4 integration

Worktree `/home/riels/.t3/worktrees/Kinewright/ja1-s4-int`, branch `t3/ja1-s4-int`.
The base is `24d1411993681235f404b2c5f895727c51ef6992`. Nothing was pushed.
No review rounds were run.

## Replay and conflict resolution

Each replay passed `cargo build --workspace` before the next replay began.
The build logs are `integration/replay-N-build.log.gz`, with N from 1 to 5.

| Original | Replayed | Subject |
| --- | --- | --- |
| `e572724` | `887c1aa5` | `feat(ja1-s4): C2 contain execution before releasing provider code [slow-tier]` |
| `724422f` | `2e0a0513` | `fix(ja1-s4): C2 review fixes (cgroup namespace boundary, subtree kill, Windows claim namespace) [slow-tier]` |
| `bc94f93` | `ae08f853` | `fix(ja1-s4): C2 re-review fixes (PID namespace, readiness pipe, service-proof witnesses) [slow-tier]` |
| `73cde94` | `e2bfc2e2` | `feat(ja1-s4): C5 persist usage and bound production ingress [slow-tier]` |
| `bb0c1cd` | `d87575e3` | `fix(ja1-s4): C5 review fixes (usage coverage before send, fault stop, accounting depth) [slow-tier]` |

All five commit bodies and coauthor trailers match their originals byte for byte.
Only the two requested subjects changed.

The C5 replay had two content conflicts:

- `ci/ignored-tests.txt`. Retained A's two lifecycle exclusions and C5's H1 usage-reopen exclusion.
- `crates/kinewright-agent/tests/fixtures/ja_s4_native.rs`. Retained C2's namespace inspection, descriptor checks, controlled exit, and init orphan-reaping branches. Inserted C5's ingress branch before `publish`, preserving its original early return and both output streams.

The other commits replayed without content conflicts. The automatic merges retained A's orchestration records and validation rules while adding the accounting variant and dependency traversal.
`origin/main` at `0856b2efa8c506afaa83daa0bdd3fe53a1412419` merged with a merge commit. Both parser files merged cleanly.
No conflict required a design decision.

The all-target compile also found one integration fixture break that workspace builds do not compile.
`ja_s4_usage.rs` initialized A's `TaskRecord` without its new `progress` field.
The follow-up supplies `TaskProgress::default()` and retains the accounting fixture's existing task state and checks.

## Follow-up behavior and witnesses

- R2C-M1. The shim drops its retained leaf `Cgroup` immediately after `enter()`, before creating the readiness pipe or forking init. The proc-view fixture attempts `/proc/1/fd` from the payload. If that view is denied, the test uses the host view captured from the pinned init PID before COMMIT. It requires exactly three descriptors, all COMMIT pipe or `/dev/null` stdio, and rejects cgroup directory, events, and kill handles.
- R2C-A1 and R2C-M2. The additional PID mutation removes NEWPID and the PID-1 verification body. It also suppresses the independent fresh-proc startup check so payload execution reaches the proc-view test's own namespace visibility assertion. The existing startup-refusal mutation remains separate.
- R2C-M3. The nested alias fixture adds `unshare --mount`, attempts `umount` on `/proc` and the runtime mask, and records both results. It checks that both unmounts fail, both mount devices survive, the runtime stays empty, procfs exposes only namespace processes, and no host proc-root alias appears. The host PID now propagates into the nested fixture.
- R2C-M4. An empty readiness EOF returns `payload_namespace_unavailable: readiness closed without result`. Namespace entry requests each flag separately, preserving their order, and names the failing flag and numeric errno. A real NEWUSER refusal in a multithreaded process checks that diagnostic. A closed readiness stream checks the named EOF reason.
- R4B-M1. Each queued ACP frame retains its restore classification. The writer refuses non-restore frames after the ingress latch and continues to service restore frames. A barrier holds the first write in flight while an executable frame queues. After the latch trips, the executable frame receives an error and contributes no bytes, while a restore frame completes. The same writer function serves the real child pipe and the controlled test sink.

The Sequence Work into Verifiable Units principle determined the per-commit build order.
Model the Domain kept the existing restore classification with each queued frame instead of reconstructing it from serialized bytes in the writer.

## Final gates

All commands use the shared `flock -o` lock, four Cargo build jobs, the requested nice levels, and the pinned FFmpeg shell setup. Memory admission checks require at least 6 GiB available.
The app uses `target/int-localappdata/Kinewright/recovery` on the durable worktree filesystem.

| Gate | Result |
| --- | --- |
| `cargo build --workspace` | PASS |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| `cargo fmt -- --check` | PASS |
| MSVC native agent and job-sys route | PASS |
| MSVC project route with documented media adapter | PASS |
| job-sys full lib and integration targets | 1 passed, 0 failed |
| agent full lib and integration targets, slow feature enabled | 1,039 passed, 22 inherited ignores |
| project full lib and integration targets | 367 passed, 2 inherited ignores |
| app `ja1_attachment`, slow feature enabled | 72 passed, 0 failed |
| Slow-test manifest lint | PASS, 117 tests, 60 allowlist entries, 27 equipped-session skip sites |
| Slow-test parser self-tests | PASS, 42 tests |

The initial full agent and project commands each failed one library test because the fresh worktree lacked the sealed JA-S1 predecessor. All integration targets passed in those commands.
The existing `c5-fix1-predecessor.sh` route built and checked the literal predecessor at `076a96c922b2f05984df4bb839ad7cf0900e6183` in an isolated checkout. The complete agent and project libraries then passed, with raw summaries:

```text
test result: ok. 737 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 52.28s
test result: ok. 141 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 15.04s
test result: ok. 72 passed; 0 failed; 0 ignored; 0 measured; 812 filtered out; finished in 339.15s
```

The initial failures remain in `integration/agent-initial.log.gz` and `integration/project-initial.log.gz`. The restored libraries are in `integration/libraries-after-prerequisite.log.gz`. Counts above replace the initial failed library outcomes with the restored full-library outcomes; they do not count duplicate executions.
The gate script was edited while its initial invocation was reading it, which stopped that invocation after the project targets. The app and Python gates were completed explicitly. The corrected script includes predecessor setup before testing.

All 32 guardian tests passed with the slow feature enabled. No `C2_NATIVE_NOT_RUN` marker occurred. C5 usage/admission and library witnesses also ran with the slow feature enabled. Ignored H1/H3 and fixture-pack tests remain exclusions.

## Mutation results

All 19 pairs earned a runtime red and a restored green. Compilation failures, skipped native prerequisites, and zero selected tests receive no credit.

| Set | Pairs | Guards |
| --- | --- | --- |
| New follow-up witnesses | 6 | init cgroup handles; payload PID visibility; nested fresh procfs; empty readiness; unshare flag/errno; C5 queued-frame latch |
| Retained C2 fix-2 witnesses | 4 | PID startup refusal; fresh proc-view; runtime aliases and nested mask survival; descriptor readiness under the production runtime path |
| Retained C5 fix-1 witnesses | 9 | first/later/crash coverage; matching-turn settlement; acknowledged settlement; cancellable reply writes; reap before terminal; fault publication after cleanup; prior write-fault publication |

The PID visibility red reached `s4_c2_proc_shows_only_the_namespace`'s own `procfs must contain only namespace init and provider` assertion. The nested fresh-proc red reached `nested procfs must remain namespace-only`. The queued-frame red reached `queued executable frame must never be written after ingress latch`.
The init mutation exposed the retained cgroup descriptors to the host fallback and failed `init retained cgroup descriptor`.

`integration/mutations/results.json` records the 10 new/retained C2 pairs. `integration/c5-mutations/results.json` records the nine C5 pairs. `integration/source-bindings.json` verifies both source digests against the fully restored final source. C5 restored greens also record and verify the exact test binary hashes.

Run the gate with `bash docs/reports/ja1-s4/integration-gate.sh`.
Run the new and retained C2 pairs with `nice -n 19 python3 -I docs/reports/ja1-s4/integration-mutations.py`.
Run the nine C5 fix-1 pairs with `nice -n 19 python3 -I docs/reports/ja1-s4/integration-c5-mutations.py`.

## Scope exclusions

Native Windows execution and hosted CI are NOT RUN on this Linux desktop. The specified MSVC routes check the native agent modules and job-sys crate, and the project crate with the documented media adapter. They do not replace a complete Windows agent/app build or runtime run.
The inherited H1/H3, hardware, and explicit fixture-pack exclusions remain named in the test output and manifests. No ignored test is credited as a runtime witness.
Whole-workspace runtime suites remain for CI under SPEED-RULES. This run covers the brief's named desktop targets.
