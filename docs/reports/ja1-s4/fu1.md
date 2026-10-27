# JA-1 S4 worker A follow-up 1

Base `061573e0809c3362bed2198fe8f818e3e1c29a7d`, branch `t3/ja1-s4-c1`,
worktree `/home/riels/.t3/worktrees/Kinewright/t3-f4cb0356`.
One commit is authorized. No push is authorized or made.

## Fixes and witnesses

| Item | Change and witness |
| --- | --- |
| A1 / M3 | `EditorTask::merge` looks up the complete original envelope through the existing owner before checking execution expiry, revocation, epoch or scope. A different command ID still refuses. `s4_drain_and_durable_base` merges once, drops the reply, waits for actual endpoint expiry, retries, revokes the endpoint and retries again. Both retries return the same receipt. The candidate still contains one merge undo entry. H3 retains current-caller project authorization. |
| M1 | `commit_candidate` refuses an unsettled bundle or pending command before freezing. Serialization or retained-charge failure resumes the cut. Definite pre-publication refusal clears the candidate hold and resumes the Core. Unknown publication retains it. `reconcile_active` selects the attempt owner with `core_for_destination` before settling the candidate. A pending candidate command supplies its registered Core; an exact foreign floor ticket still selects its own actor. `fu1_refused_candidate_commit_does_not_block_unknown_manual_save` exercises landed and unlanded unknown manual floor saves, with and without an earlier candidate refusal, then refuses candidate work, reconciles the manual attempt, saves and merges successfully. `fu1_definite_candidate_refusal_releases_the_frozen_cut` refuses an issuer sequence, checks Live state and an unchanged root, then commits the valid sequence. Both H2 witnesses remain required. |
| M2 | The C3 witness calls the real `call_exposed_blocking` on a second thread. Two barriers pause that call immediately after production admission, start owner freeze, and then release the call. The witness checks that freeze waits, the call succeeds, and its split appears in the frozen delta. It never acquires `admit()` itself. The per-handler hook exists only in test builds. The production-admission mutation drops the actual dispatcher guard before dispatch. |
| M4 | `command_target` requires the target checkpoint immediately after receipt lookup. An absent branch returns `Unsupported("unknown branch")`. `fu1_unknown_branch_refuses_after_receipt_lookup` supplies `expected_head: None` for an absent branch, then commits a valid command and removes its in-memory branch entry to verify that an existing receipt still wins. |
| A2 / M6 | C1 report, handoff and identities name final parent `4250d146eb767b64a058bcbafb5f9adbccbb5264`. They separately retain original measured base `7fa471fcd2ee0455142f7ee41893ec4ccaf646df`. Original hashes, logs, summaries and mutations remain historical. Every C1-local gate is marked NOT RUN at the new parent. H2, C3 and this report retain their own later-head evidence. `fu1-audit.py` checks the parent against Git and verifies every original identity field except the corrected base is unchanged. |
| M7 | `policy.rs` and `server_tests.rs` identify original Kinewright implementation and say no upstream source was translated. `fu1-audit.py` checks both headers. App attribution and the packaged notice link are NOT RUN because the S3 App and packaging integration is not present in this branch and belongs to that owner. |
| M8 | `EditorTask::close` freezes and persists terminal work, calls the existing owner close, seals the Core, removes the registration and revokes ingress. The C3 witness closes the expired frozen task after receipt replay, checks registration count falls by one and checks the retained Core is Sealed. |
| C4-R2-1 | Owner-only `SettleExpiredBackground` requires the exact expired turn, original epoch, no specialist endpoint or attempt authority, and its exact acknowledged Retired reservation. Only then does it clear background work. `fu1_expired_background_settles_only_after_native_cleanup` drives the real pump and C1 kernel. It rejects settlement before cleanup, old-session ingress, a mismatched turn and reuse of the retired attempt. After the fixture supplies the C2-owned retirement fact, owner settlement allows a fresh attempt and session. Actual native retirement proof remains NOT RUN pending C2. |
| C4-R2-2 | Final Ready validation accepts a Cancelled task only when it has a parent. It retains the completed-root, cleanup, effects, delivery and request checks. `fu1_cancelled_reviewer_allows_completed_root_ready` cancels a queued reviewer through the pump and C1 kernel, publishes the root result and validates the final Ready projection after fixture-supplied cleanup. The witness also refuses unsettled cleanup and a cancelled or incomplete root. Final durable publication remains NOT RUN pending H1. |
| Report wording | C4 now attributes T1 to the lead's request. `fu1-audit.py` checks the correction. |

M5 and the C2/C5-owned files named in the brief are unchanged. No review round is
started by this worker. The lead owns the requested review tier.

## Runtime reproduction

Before the fix, an issuer-sequence refusal leaves the candidate Frozen instead of
Live. After that refusal, an unknown manual floor save and another refused
candidate commit leave `reconcile_active` failing with `HistoryBarrierInvariant`.
The retained `fu1-manual-before.log.gz` records that failure. A fresh refusal during
an unknown save can already fail before freezing through existing store admission;
the regression witness covers both that route and the retained-hold route.

The original C3 merge replay fails after endpoint expiry with
`Unsupported("task_authority_expired")`. The cancelled-reviewer final projection
fails validation with `InvalidState`. Their before-fix logs are retained separately
from the final mutation campaign. Early fixture-construction failures are diagnostic
only and receive no mutation credit.

## Verification

`fu1-gate.sh` records the exact commands. The full raw per-target summaries are
retained in `fu1-test-summaries.json` and the compressed logs.

| Command or lane | Result |
| --- | --- |
| `c1-cargo.sh build --workspace` | Exit 0. ``Finished `dev` profile [unoptimized + debuginfo] target(s) in 15.51s``. |
| `c1-cargo.sh clippy --workspace --all-targets -- -D warnings` | Exit 0. ``Finished `dev` profile [unoptimized + debuginfo] target(s) in 13.69s``. |
| `c1-cargo.sh test -p kinewright-project --features test-util --no-fail-fast -- --test-threads=1` | Initial exit 101. 226 integration passes and 138 unit passes; one unit test lacked the sealed predecessor. Two declared ignores. |
| `c1-cargo.sh test -p kinewright-project --features test-util --lib -- --test-threads=1` | Exit 0 after prerequisite setup. `test result: ok. 139 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 19.74s`. Effective complete project target set is 365 passes and two ignores. |
| `c1-cargo.sh test -p kinewright-agent --no-fail-fast -- --test-threads=1` | Exit 0. 960 passes, zero failures and 35 declared ignores across 18 targets including empty binary and doc targets. |
| App architecture target in `fu1-gate.sh` | Exit 0. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 0.02s`. |
| Touched-file `rustfmt --check` | Exit 0 with no output. |
| `nice -n 19 python3 -I scripts/slow_tests.py lint` | `slow-test manifest and markers agree: 89 tests, features kinewright-agent/slow-tests,kinewright-app/slow-tests,kinewright-media/slow-tests; 59 allowlist entries well-formed`. |
| `nice -n 19 python3 -I docs/reports/ja1-s4/fu1-audit.py` | `FU1 provenance, retained C1 identities, C3 headers and T1 attribution: PASS`. |
| `git diff --check` | Exit 0 with no output. |

The C1, H2, C3 and C4 witnesses ran in those full affected-crate targets. Their
required target summaries include the following raw lines.

```text
tests/ja_s4_candidate.rs: test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.00s
tests/ja_s4_store.rs: test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
tests/ja_s4_kernel.rs: test result: ok. 23 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.12s
tests/ja_s4_lifecycle.rs: test result: ok. 36 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 2.18s
```

`nice -n 19 python3 -I docs/reports/ja1-s4/fu1-mutations.py` exited 0.
It recorded seven protected runtime red/restored-green pairs and three
audit red/restored-green pairs. Compiler errors and zero-test runs receive
no credit. Every original file is restored before its positive rerun.
The 12 Rust source hashes match the full-crate gate snapshot. The mutation
manifest records each exact command, restored source hash, raw summary and
compressed log hash.

| Item | Reverted result | Restored result |
| --- | --- | --- |
| M1-candidate-refusal | `test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 2 filtered out; finished in 4.85s` | `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out; finished in 6.38s` |
| M2-production-admission | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 724 filtered out; finished in 1.97s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 724 filtered out; finished in 4.53s` |
| A1-receipt-before-authority | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 724 filtered out; finished in 4.12s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 724 filtered out; finished in 4.56s` |
| M4-unknown-branch | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 140 filtered out; finished in 0.39s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 140 filtered out; finished in 0.48s` |
| M8-task-close | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 724 filtered out; finished in 4.11s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 724 filtered out; finished in 4.51s` |
| C4-R2-1-background-settlement | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 37 filtered out; finished in 0.27s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 37 filtered out; finished in 0.25s` |
| C4-R2-2-cancelled-ready | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 37 filtered out; finished in 0.27s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 37 filtered out; finished in 0.29s` |
| A2-C1-parent | `AssertionError` | `FU1 provenance, retained C1 identities, C3 headers and T1 attribution: PASS` |
| M7-provenance | `AssertionError` | `FU1 provenance, retained C1 identities, C3 headers and T1 attribution: PASS` |
| T1-request-owner | `AssertionError` | `FU1 provenance, retained C1 identities, C3 headers and T1 attribution: PASS` |

The first full project invocation exited 101 because the sealed max-2 predecessor
was absent. Its other test targets passed. `fu1-predecessor.sh` runs the existing
pinned recipe in `target/ja-s1-predecessor/source` at
`076a96c922b2f05984df4bb839ad7cf0900e6183`, with the required Cargo wrapper.
The source and both executable identities are retained in
`fu1-predecessor-identity.json`; its controls are archived in
`fu1-predecessor-evidence.tar.gz`. The initial wrapper's FFmpeg setup output
entered Cargo's JSON stream. Redirecting setup diagnostics to stderr fixed that
setup failure without changing the pinned source or observer.

`bash docs/reports/ja1-s4/fu1-gate.sh remaining` reran the full project unit target,
which passed 139 tests with two documented ignores, then ran the agent suite.
The first App command incorrectly used `--lib` for a binary-only crate. The corrected App command and the final static checks
ran through `bash docs/reports/ja1-s4/fu1-gate.sh finalize`. The original
full-project command retains its exit 101 result.
The complete affected-crate target set uses that unit rerun plus the successful
original integration targets. Initial lint failures and preliminary witness runs
are diagnostic archives. Final acceptance uses the full-crate gate logs, the
project unit rerun and the restored mutation greens.

Main-worktree Cargo invocations use `c1-cargo.sh`. It checks at least 6 GiB available,
sources `scripts/setup-ffmpeg.sh` in the same shell, sets `RUST_TEST_THREADS=4`, and
uses `flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo`.
The pinned predecessor helper applies the same memory check, same-shell FFmpeg
setup and exact Cargo wrapper. Cargo invocations that run tests use
`--test-threads=1`. Every Python invocation uses
`nice -n 19 python3 -I`. Formatting checks only touched files with
`rustfmt --edition 2024 --check --config skip_children=true`, as PITFALLS requires.
`fu1-identities.json` binds the final changed sources, harnesses, test binaries
and evidence archives. Build artifacts are removed after hashing.
The worktree shares the Cargo lock with other workers; lock waits are not test
failures or evidence of performance.

## NOT RUN

- App attribution list and packaged-notice link. S3 owns the App and packaging integration, which is absent here.
- Native C2 launch, containment and cleanup proof, and C5 accounting integration. The new owner settlement witness consumes a fixture-supplied acknowledged retirement fact and does not claim native acceptance.
- H1 durable orchestration root publication, graph retention, independent lifecycle reopen and crash recovery. The C1 kernel and final projection validator run without the absent H1 publisher.
- H3 current-person attachment and merge authorization, approval and secret holders, real connector certification, G12 and end-to-end G13. The pre-H3 receiving ports do not provide those integrations.
- Native Windows and G1-W. This workstation is Linux.
- Hosted fast and slow CI, full workspace suites and slow-feature suites. Workers do not push, and SPEED-RULES leaves those suites to CI. The local fast targets of both affected crates are included in the gate.
- Historical C1-local gates at exact parent `4250d14`. The original pre-rebase evidence is retained and explicitly labelled. Current-head reruns do not retroactively earn that base.
- The 37 existing declared ignores in the two affected crates. `fu1-ignored.json` retains each test and its exact reason, including manual predecessor and performance checks, unavailable App bridge or GUI participants, prepared speech or M40 media packs, live subscriptions and the slow tier. None is counted as a pass.
- Worker-run independent reviews. The brief reserves review rounds to the lead.
- A separate comparative timing campaign. No speed target is a gate for this follow-up.
