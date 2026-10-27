# JA-1 S4 worker A follow-up 2

Parent `d85b85ba00ea40d1cf7101a9a0542c3def7de865`, branch `t3/ja1-s4-c1`,
worktree `/home/riels/.t3/worktrees/Kinewright/t3-f4cb0356`.
The brief authorizes one commit and no push.

## Fixes and witnesses

| Item | Fix and witness |
| --- | --- |
| A3 | `ExpireSession` refuses notifications without a current, unexpired, nonterminal task belonging to that session. `fu2_late_expiry_preserves_replacement_and_finished_phase` expires A, supplies the C2-owned retirement fact, starts B, then submits a new late A command. It checks the unchanged projection, B identity, and pending effect eligibility. It also checks expiry after Stop leaves Finished unchanged. |
| R3-M1 | Candidate settlement installs the published history for both `Committed` and `Retired`. Initial and repeated commit calls retain the durable receipt if settlement fails; `reconcile_active` retries the retained hold. `fu2_candidate_settlement_recovers_retired_receipt_without_masking_commit` injects two settlement failures, acknowledges and retires the issuer through the existing receipt API, reconciles, and saves. It checks Frozen then Live, exact candidate history, unchanged manual history, hold release, and the original receipt from both calls. The fault seam is test-only at the settlement history-read boundary. |
| R3-M2 | The kernel refuses `SettleExpiredBackground`, `RecordContext`, `WaitTimeout`, and `CancelTask` when outcome is not Pending. `fu2_terminal_runs_refuse_owner_lifecycle_commands` exercises all four on Failed, Cancelled, and Ready projections and checks no acknowledged change. Expiry preserves Finished, covered by the A3 witness. |
| R3-M3 | Every Applied `prepare` cut refreshes cleanup and derives wait from pending questions, live child dependencies, and unobserved deliveries. Result publication also refreshes wait. Owner-supplied Files and Provider waits stay intact until their owner settles them, covered by `s4_stop_retains_cleanup_and_never_continues_failed_work_contract`. `fu2_wait_tracks_questions_children_and_deliveries` drives question admission and answer, child wait, result publication, and delivery observation through the kernel. It checks the wait transitions and cleanup validity without hand-setting wait. Native attempt admission and retirement still belong to C2 and H1. |
| R3-M4 | Only the `merge` doc comment changes. It requires H3 to authenticate the current attachment and project, and names `Pending`, `Retired`, `RetiredUnresolved`, and `IdPayloadConflict` for explicit caller handling. Authentication implementation remains with S3. |
| R3-M5 | `result_and_delivery` and `next_effect` document that a wake delivers every record whose `wake` equals the effect ID. `payload` contains only the first result. The existing `s4_observed_and_cancelled_siblings_settle_one_wake` covers shared wake identity and independent observation. The H3 executor witness remains outside this branch. |
| R3-M6 | The Studio pump skips registration of an identical live request. `poll_runtime_request` documents consuming or repeating a pending request with the same identity and contents. `fu2_repeated_pending_request_uses_acknowledged_owner` polls the same request twice, waits for the C1 owner's answer, and checks one response and normal completion. |
| R3-M7 | The complete local `kinewright-core` target set runs with the final source, alongside agent and project targets. Results appear below. |
| R3-M8 | `s4_drain_and_durable_base` gives the first merge a one-hour expiry and revokes authority before replay, without a wall-clock sleep. `fu1_unknown_branch_refuses_after_receipt_lookup` was ungated here. FU3 restores its Linux gate because G1-W still refuses durable bundle activation on Windows. Both run in the full affected-crate gate. |

M5's index assignments in `publish_branch_impl` and all excluded C2 and C5 files stay
unchanged. This worker starts no independent review round.

## Verification

All Cargo commands use `bash docs/reports/ja1-s4/c1-cargo.sh`. The wrapper checks
at least 6 GiB available, sources `scripts/setup-ffmpeg.sh` in the same shell,
and runs `flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo`.
Tests use `--test-threads=1`. Every Python run uses `nice -n 19 python3 -I`.
The pinned predecessor recipe completed before the project suite, with a separate
source and Cargo target directory.

| Command after the wrapper | Result |
| --- | --- |
| `build --workspace` | Exit 0. ``Finished `dev` profile [unoptimized + debuginfo] target(s) in 15.35s``. |
| `clippy --workspace --all-targets -- -D warnings` | Exit 0. ``Finished `dev` profile [unoptimized + debuginfo] target(s) in 12.08s``. |
| `test -p kinewright-core --no-fail-fast -- --test-threads=1` | Exit 0. 812 passed, zero failed, 1 declared ignore across 20 targets, including doc targets. |
| `test -p kinewright-agent --no-fail-fast -- --test-threads=1` | Exit 0. 964 passed, zero failed, 35 declared ignores across 18 targets, including doc targets. |
| `test -p kinewright-project --features test-util --no-fail-fast -- --test-threads=1` | Exit 0. 366 passed, zero failed, 2 declared ignores across 14 targets, including doc targets. |
| Touched-file formatting | Exit 0. `rustfmt --edition 2024 --check --config skip_children=true` on the 11 touched Rust files. PITFALLS requires checking the touched files directly. |
| `nice -n 19 python3 -I scripts/slow_tests.py lint` | `slow-test manifest and markers agree: 89 tests, features kinewright-agent/slow-tests,kinewright-app/slow-tests,kinewright-media/slow-tests; 59 allowlist entries well-formed`. |
| `git diff --check` | Exit 0. |

[fu2-gates.log.gz](fu2-gates.log.gz) retains every raw target summary and each
ignore's stated reason. The counts above are sums of those summaries. The initial
agent invocation had one failure in
`s4_stop_retains_cleanup_and_never_continues_failed_work_contract`. Refreshing
wait cleared an owner-supplied Provider fact. The fix preserves Files and Provider
facts until their owner settles them. The corrected focused targets passed 23 and
40 tests, and the complete gate then passed with that source.

The four new agent regressions failed on the original behavior and passed with
the fixes. The R3-M1 witness separately reverts retired-receipt handling, initial
receipt preservation, and retry receipt preservation. Each reversion compiles and
fails its runtime assertion; each restored source passes. No compiler or setup
failure receives mutation credit. The manual document has one clip while the
candidate has none, so history equality cannot pass by substituting the candidate
for the person's work.

| R3-M1 reversion | Reverted runtime | Restored runtime |
| --- | --- | --- |
| `Retired` settlement arm | Exit 101, zero passed, one failed. | Exit 0, one passed, zero failed. |
| Initial committed receipt | Exit 101, zero passed, one failed. | Exit 0, one passed, zero failed. |
| Retried committed receipt | Exit 101, zero passed, one failed. | Exit 0, one passed, zero failed. |

[fu2-mutation.log.gz](fu2-mutation.log.gz) is the single mutation log. It contains
the exact replacements, commands, source hashes, runtime failures, and restored
raw summaries. The restored `durable.rs` SHA-256 is
`6c0492a73390d6f98b65db644ceb1b6ca3891f62e9c4afb73f7a047fed20ab56`, unchanged through the final gate.

## NOT RUN

- 38 existing declared test ignores, comprising 1 core, 35 agent, and 2 project ignores. Reasons cover manual performance and old-reader checks, a historical uncooperative journal fixture, the known media-engine teardown SIGSEGV, missing App or GUI participants and H1 or H3 integrations, unprepared speech and media packs, live subscriptions, and the slow tier. Each exact test and reason is retained in `fu2-gates.log.gz`; none is counted as a pass. The predecessor prerequisite recipe itself did run successfully.
- Native Windows, Windows clippy, and G1-W. This workstation runs Linux. `bundle_commit::fu1_tests::fu1_unknown_branch_refuses_after_receipt_lookup` is NOT RUN on Windows. It requires durable bundle activation and receipt publication, which G1-W does not support. FU3 restores the Linux-only gate. The receipt-before-unknown-branch ordering cannot be exercised through a supported Windows bundle owner until G1-W exists.
- Hosted CI, the full workspace test suites, and slow-feature suites. The brief forbids pushing, and SPEED-RULES leaves full suites to CI. Required local core, agent, and project targets run here.
- Native C2 admission, containment, and retirement through a kernel command, plus C5 accounting. Those implementations and files are outside this worker's scope. Lifecycle fixtures supply acknowledged C2 retirement facts. The cleanup refresh call is wired for their later integration.
- H1 durable orchestration publication, independent lifecycle reopen, and crash recovery. The publisher is not merged in this branch.
- H3 current-attachment authentication, approval and secret holders, real connector certification, and a coalesced-wake executor witness. S3 and H3 owns those integrations. R3-M4 changes only documentation.
- G12 and end-to-end G13. They require the absent H1 and H3 and native integrations.
- Worker-run review rounds. The brief reserves reviews to the lead.
