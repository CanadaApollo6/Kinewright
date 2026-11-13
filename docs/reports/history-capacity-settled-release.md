# Settled-run release: recovery boundary

Inspected on `8d81d6f967bee3425ed8c39b6f0c8bc213ed3952`, after recovery commit `a1325ba77e110bd5937cbf6ff9da45c85eedf11d`. This records the original boundary investigation, before the lead authorized 2a. Stage 1 and its CI changes have since been pushed by the lead; fix 1 is local commit `910a3e8924602e77bc59f5022b0694e11a6fe90a`.

## Original ownership and boundary investigation

`HostRun.tasks` owns each `EditorTask`; the task's `DurableBranchSessionGuard` and the owner's `RegisteredSession` both retain its Core. `TaskEndpoint` retains authority, policy and actor identity, rather than the Core. Some admitted tool calls temporarily clone the Core while outside the host mutex. Release must therefore check endpoint quiescence and holders, rather than relying only on the run's cleanup state.

The existing `close_registered_task_checkpoint` compares the frozen live history with the saved branch history, seals the actor, and removes the durable registration (`editing_session/durable.rs:1014–1055`). It refuses dirty work instead of saving it implicitly. `reconcile_native_effect` checkpoints task work before acknowledging retirement. Healthy frozen-task rehydration already uses the retained H1 source and specialist base (`writer_host/task_history.rs:181–240`).

The intended small change is an owner-serialized release sweep after acknowledged settlement, with retries after outstanding holders disappear. It would remove a task only after its current cut is checkpointed and its endpoint is no longer held by live work. Candidate registrations would be released only when inactive and unused by other live tasks. A second possible shape, keeping a separate cache of dormant actors, preserves the memory growth and is unsuitable. Neither shape had been implemented at the time of this investigation.

## The failed-rehydration witness has no current recovery offer

A focused Project probe creates a native candidate and task, makes an edit, persists the task, closes both registrations, and rehydrates the task from its saved journal. The restored history equals the original in full. It then closes the registration again and constructs an authentic content-addressed task history whose boundary hash cannot replay. Only that task history is damaged; the manual history and the task's checkpoint document remain intact.

Observed results:

- Native rehydration and checkpoint review both return `Corrupt("history replay differs from the retained checkpoint")`. Neither registers an actor or publishes a replacement.
- `assess` identifies exactly the native task branch as quarantined and independently verifies its checkpoint document.
- `VerifiedCheckpointRecovery::open` returns `Ok(None)`. It selects only `root.manual_branch_id` (`bundle_recovery/checkpoint.rs:86–104`) and also compares against the root's manual inspection document at lines 122–127.
- The original root and failed history bytes remain exact.

This probe uses a real native-role branch and its ordinary durable registration APIs. It is not the requested full HostRun settlement, frozen-result merge, or N-run heap witness.

The required 2a failed-rehydration recovery is therefore unavailable today. The existing rescue capability also acquires the project namespace itself, while a live host already owns that namespace. Merely calling the manual-path opener from task rehydration cannot supply the missing behavior.

**Decision needed before widening 2a:** permit an owner-backed recovery capability for an explicitly selected native checkpoint. My recommendation is to reuse the same independent document, current-lineage and owned-input verification, then offer read-only recovery with Undo unavailable and Save Recovered Copy. A failed replay must never become a resumed task or a mergeable frozen result. The task/run/source checkpoint identities and failed journal would remain retained. This adds a native recovery entry point, beyond releasing actors and replaying healthy journals. I stopped here under the lead's instruction to report a new 2a design decision instead of widening the change.

## Original probe evidence and limits

`target/history-capacity/settled-recovery-probe.rs` and `settled-recovery-probe.log` retain the probe and its completed **1 passed, 0 failed** result. The temporary include was removed; `checkpoint_tests.rs` was restored byte-exact to SHA-256 `3fba03669488aa8c8c6bd69b7c47ac0836883bcf86480bd2e6225d68a37cfe1a`. The restored baseline recovery module then passed **4 tests, 0 failed** (`settled-recovery-baseline.log`). No build or test remains running.

The full merged-tree gates remain as reported in `history-capacity-stage1.md`: Core 801, Project 467, Agent 1,722 and App 1,014 passed, with zero failures. Those gates precede this restored test-only probe. Those original probe gates did not exercise 2a release, heap residency, the full-run failed-rehydration UI, Windows or paid providers. After the baseline rerun drained, `target/` is **29,483,697,248 bytes**, with more than 169 GB free on the filesystem. The 568 MiB envelope remains **NOT QUALIFIED**. The raw-publication latch ruling and the non-latching fix required before 2c are unchanged.


## Ruled 2a implementation

The lead authorized settled-run release, with fresh verification before dropping either owner. Native checkpoint rescue is a separate next commit. The original missing-rescue finding above remains true until that commit; no release or rehydration message promises recovery in this commit.

The existing `close_registered_task_checkpoint` freezes the live Core and compares its complete `HistorySnapshot` with `ProjectBundle::branch_history`. That method calls the store resolver directly: it freshly reads the persisted record and document objects, executes the bounded journal, and verifies every hash. It does not use `CachedHistory`. The existing comparison covers document, Undo/Redo, commands, identities and floor offset. It does not establish endpoint/client-handle quiescence or compare the persisted occurrence maps, so it alone is insufficient for 2a.

The release path checks settlement and retired attempts under the host owner mutex. `EditorTask` additionally requires a revoked/sealed endpoint with no external holder, no media work and an available ingress gate. A Core client-handle counter distinguishes the two task owners (host task plus durable registration) from any admitted tool clone. Candidate release is a separate entry point requiring only the durable owner, and it refuses candidates referenced by a registered task or another active run. The counter's shared allocation represents real client ownership; it is not a compiler workaround.

A matching logical history digest detects a clean/current cut, then fresh checkpoint decode, bounded store replay and full history equality prove the journal. The absolute batch position is preserved by equality of `floor_offset` and the entire accepted batch stream, including empty Undo/Redo. The shared cold-open lineage check verifies baseline/current and every cursor boundary's clip keys and role budget. Rebuilding occurrence maps from the live accepted commands, persisted batch IDs and original occurrence seeds must equal the freshly decoded lineage, including split ancestry. Only then is the actor sealed and the durable registration removed. The host task entry is removed in the same owner-serialized sweep. Failed or capacity-refused verification leaves the actor in place; corruption is included in the shared person/Director run status. There is no dormant-actor cache.

Task and candidate rehydration prepare both registrations before exposing either. A failed journal cannot register a partial pair or become a resumed/mergeable task. Existing `Corrupt` refusals name the task, its saved result and the unaffected other project work, retain the bytes, and avoid promising the later rescue capability.

The Project witnesses cover dirty work, extra holders and capacity deferral; complete Redo rehydration; freshly detected saved-byte corruption with a usable resident actor; frozen review and merge after unpublished-object collection; and split-occurrence equality. The initial capacity injector incorrectly requested more than one decoded-record window. Its correction fills the raw ledger through bounded chunks, without changing a limit or weakening a failure assertion.

The host witnesses run a dirty task through Finish and settled Stop, drop both actor owners, reopen its exact saved history, release it again, and damage its journal. The next reopen returns typed `Corrupt` (`damaged_bundle`), registers no actor or partial pair, publishes nothing and preserves the root and damaged journal bytes. Another witness injects damage before release: the task stays resident with its complete history, and both the person and the Director receive the task-specific notice. The candidate identity is recorded at run admission, so a run stopped before its first task actor also releases its inactive candidate.

If fresh verification discovers bad bytes, the store invalidates its dependency memo. A later run projection can therefore refuse before a normal `RunViewPage` exists. In that case the same typed refusal includes the retained task notice; it does not bypass dependency verification to manufacture a healthy view. Producer scope and expiry are checked before attaching that notice. Restoring bytes by hand does not reset this session's invalidated dependency ledger. The live actor remains usable and the original work stays on disk. This commit adds neither a repair operation nor a recovery promise.

## Resident heap measurement and remaining holders

The isolated six-run witness keeps the complete **400-clip, 200-edit** row, with an additional Undo (201 accepted batches). Each row includes Finish and settled Stop. After dropping the observation snapshot, every host task map is empty and the durable registration count returns to its initial value of one. That one registration is the fixture's original seed candidate; no candidate from these six runs remains.

The test uses pinned, test-only `stats_alloc` 0.1.10. Its [System allocator counters](https://docs.rs/stats_alloc/0.1.10/src/stats_alloc/lib.rs.html) include reallocation deltas. Samples are allocated minus deallocated Rust bytes, taking the minimum over a bounded 500 ms interval because Core drops its state on its receiver thread. They are not RSS, native decoder heap or a 512 MiB qualification. The self-exec child writes to a captured file, has a checked 500 s deadline and is reaped. Markers use `println!` with libtest `--show-output`.

The final-source isolated witness passes **1 / 0**, completing all six runs in **53.07 s**. The standard-library process timer records 53.079 s wall, 29.153 s user CPU and 1.303 s system CPU, excluding the Cargo-lock queue. It runs the same compiled slow-tier Agent unit binary as the full passing gate, with `--exact --show-output --test-threads=1`. Evidence: `target/history-capacity/settled-release-heap-final.log` and its `-result.json` companion.

| Settled runs | Live Rust bytes | Resident task actors | Durable registrations | One task history's typed live charge |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 2,423,821 | 0 | 1 | 23,862,362 |
| 2 | 2,848,336 | 0 | 1 | 23,862,362 |
| 3 | 3,271,585 | 0 | 1 | 23,862,362 |
| 4 | 3,689,210 | 0 | 1 | 23,862,362 |
| 5 | 4,088,043 | 0 | 1 | 23,862,362 |
| 6 | 4,542,067 | 0 | 1 | 23,862,362 |

**Retained actor-history residency is flat; total process heap is not literally flat.** The measured span is 2,118,246 bytes, about 0.42 MB per additional run, rather than another 23.86 MB history per run. `HostRun.inputs` still holds each run's `FrozenContext.document`, `PlaybackInputs.document`, input leases and snapshot pin. Run Producer scopes, measurement maps and bounded store/control metadata also remain. There is no dormant actor cache. Releasing frozen input closures would need its own continuation/rehydration change and is not hidden inside actor release.

The earlier DHAT diagnostic also completed six rows, but took 346.31 s because it collected allocation backtraces. It is not retained as the CI instrumentation. One parent attempt counted five markers because `--nocapture` joined the first marker to the libtest header; `--show-output` fixed that accounting error. The six-run shape was not reduced.

The stage-1 envelope remains **568 MiB before other metadata, NOT QUALIFIED** against the unchanged raw 512 MiB ceiling. Task 41 MiB, candidate 2 MiB and ordinary/manual 128 MiB bounds are unchanged. This commit proves removal and exact rehydration of settled actors; it does not claim the enforced envelope or four resident in-run slots. Those remain 2b and 2c. Finish and Freeze are **NOT guaranteed** after a raw-ledger refusal of an ordinary or terminal native publication. Stop and exact retention **are**. The non-latching raw-ledger publication change remains a hard precondition of 2c.

## Per-commit gates

The first full Project gate finished **473 passed, 1 failed, 3 ignored**. The newly added collection witness called the raw store collector before collection was admitted, producing `Unsettled`. The fixture now calls the existing bundle `collect_after_ack()` path, retaining its GC assertion. All started tests drained before the correction. Evidence for that attempt is in `target/history-capacity/settled-release-gates-first/`. All final gates pass. Evidence is under `target/history-capacity/settled-release-gates/`; its `results.json` records commands, results and SHA-256s of every changed source file. Those hashes were checked again after the last gate and still match.

| Gate | Result |
| --- | --- |
| Core full suite | 807 passed, 0 failed, 2 ignored |
| Project full suite with `test-util` | 474 passed, 0 failed, 3 ignored |
| Agent full suite with `slow-tests` | 1,728 passed, 0 failed, 100 ignored |
| App full suite with `slow-tests` | 1,014 passed, 0 failed, 5 ignored |
| Core, Project, Agent and App all-target clippy, default and slow/test-util modes | Both passed with `-D warnings` |
| Workspace test compilation | Passed, 80 executables |
| `cargo fmt -- --check` | Passed |
| Slow-test lint | 134 markers, 155 allowlist entries, 27 equipped-session skip sites |
| CI shard and slow-test self-tests | 22 and 48 passed |
| Windows wrapper | 3 Linux shipping builds, 4 MSVC clippy commands, 3 MSVC shipping checks passed |

Full suites used the approved `.cargo/ci-test-linux-slow.toml`; no Core, Project or Agent optimization block was added to root `Cargo.toml`. The fixture-enabled host was built before App. Shipping builds used `target/shipping`, which was removed after success. The existing `REFRESH` and `TASK_TOOL_NAMES` warnings occur in shipping feature graphs; the clippy graphs passed with warnings denied. Windows execution, equipped-provider cases and paid-provider runs were not run. Declared ignores remain explicit in the table.

At the idle Cargo boundary after Agent, pruning 41 superseded test binaries reduced `target/` from **33,954,471,903** to **29,496,350,952 bytes**. Every binary named by the completed gates was retained. The gate controller alone was paused while Agent continued unchanged, then resumed after Cargo exited and pruning finished. No build or test received a signal. The deletion manifest is `target/history-capacity/settled-pruned-binaries.json`. The preliminary 6.46 GB candidate list shrank to 4.46 GB as Agent's completed integration binaries joined the keep set.

The final heap witness above also passes. After removing the isolated shipping output, `target/` is **37,595,695,250 apparent bytes** and **37,621,977,088 allocated bytes** (35.04 GiB allocated). The untracked `scripts/__pycache__/` created by Python self-tests was removed. All started gate and measurement processes have exited.
