# Run-limit C2/C3 follow-ups

The capacity note was delivered before this work. Windows test guards are in `08796e467569b8a57b8ab9d136293f15465f6530`. This follow-up leaves the capacity design, candidate-merge latch, coverage gaps and snapshot reservation follow-up unchanged.

Cleanup authority now lasts only while the project-owned `CleanupPublication` lease exists. Store views retain a weak lease reference; dropping the handle makes their admission ordinary, including on unwinding. Expired cleanup constraints also stop participating in transition validation. The agent retains its guardian proof borrow but no longer performs manual restoration, and the public `end_cleanup_publication` API is removed. The `Arc`/`Weak` pair represents shared authority lifetime and revocation across store views.

`begin_cleanup_publication` remains public because the agent is a separate crate. The proof check remains in its private host wrapper: guardian proof types belong to the agent, which already depends on project, so importing them would create a dependency cycle. An unsealed generic witness would not enforce that proof. No new proof abstraction or dependency was added.

Both shared MCP apply funnels now check prospective task history when an endpoint is present; the LUT funnel also validates task policy. No capability was added. The existing task boundary witness now also refuses a changed floor offset without changing live history. Adding a private-funnel fixture would require a separate seam, so no such test was added, as the brief permits.

Cleanup publishes exact selected task cuts and reuses committed manual history. The metadata path refuses a pending manual command. It therefore cannot seal an automatic floor or a manual RecoveryClose transition; the kind allowlist now documents why both remain excluded.

`TaskHistoryAdmission` captures the prefix's floor offset, rejects a mismatch, and builds the hypothetical specialist record from its identity fields and prefix. It no longer copies the old full delta only to replace it.

## Focused verification before merging

- Project lease-drop witness: 1 passed, 0 failed, 180 filtered. It uses the same internal installation as the validated session port under a synthetic latched unit budget, allows a cleanup write, then drops the handle and gets `RunLimit` from an ordinary write. It does not forge an H1 projection or guardian proof.
- Agent cleanup/history boundary module: 3 passed, 0 failed, 1,374 filtered. Includes all ten latch/overlap cleanup cases, stale-floor refusal and bounded Undo/Redo. Final run: 117.65 s.
- Both crates' all-target clippy with project/test-util and agent/slow-tests: passed. Fmt check: passed. Slow-test lint: 125 tests, 150 allowlist entries, 27 equipped-session skip sites.

Local evidence is under `target/run-limit-followups/`: `cleanup-drop-final.log`, `cleanup-boundary-final.log`, `clippy-focused.log`, `fmt-focused.log` and `slow-lint-focused.log`. All commands source FFmpeg, use the shared heavy lock, four Cargo jobs and nice 10; Python uses nice 19 and isolated mode. The full merged verification and save measurement follow below after the merge.


## Merged verification

The follow-up is `4ba98679`; the fetched `te1` head is `d79b9375fd7a5082b23196dd9f5430c2dc514d22`. The merge applied cleanly. Inspection confirms the checked-history byte/charge/support-field reuse remains alongside C2/C3's person/cleanup admission, ordinary run clamps and specialist-base bound. The merge was held uncommitted for the gates.

All eight deleted paths from both sides were checked by full path and basename. There are no current application, CI or maintained-driver dependencies. Ten older report gate recipes had obsolete predecessor preparation removed; two predecessor-only recipes now explain that they are retired and exit. All twelve pass `bash -n`. Literal grep still finds 52 historical mentions in dated reports, immutable source-hash maps/logs or two frozen script captures. Those provenance records were preserved. Details are in `deleted-reference-audit.json` and `retired-recipe-changes.json`.

| Full merged suite | Passed | Failed | Ignored | Filtered | Suites |
| --- | ---: | ---: | ---: | ---: | ---: |
| kinewright-project, test-util | 447 | 0 | 3 | 0 | 21 |
| kinewright-agent, slow-tests | 1,705 | 0 | 95 | 0 | 30 |

Both full commands used `--no-fail-fast`; no retired-test name was skipped. The agent library passed 1,319 tests, with 58 ignored, in 1,197.12 s. The merged cleanup matrix, history bound and Undo/Redo cases all passed. Workspace `cargo test --workspace --no-run`, fmt and both crates' all-target clippy passed. Slow-test lint passed: 125 tests, 150 allowlist entries, 27 equipped-session skip sites. All 1,007 recorded crate/script/Cargo/CI source hashes matched after the gates.

One release `save_cost_measure` run passed. It used the default four points, with three warm saves and twelve measured changed-history saves per point:

| Requested edits | Retained batches | Median ms | Min–max ms | Fsync calls | Fsync ms | Standalone walk median ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 100 | 115 | 112.0 | 110.6–113.2 | 14 | 99.0 | 3.1 |
| 400 | 415 | 137.2 | 135.6–140.8 | 14 | 99.2 | 11.3 |
| 1,600 | 1,615 | 250.7 | 243.9–258.5 | 14 | 111.3 | 44.1 |
| 3,000, history cap | 1,690 | **272.8** | 240.4–280.8 | 14 | 107.4 | 46.9 |

The cap result remains consistent with `te1` merge `58eb76e8`'s recorded roughly 280 ms, after its roughly 660 ms baseline. This is one current run, not a new paired before/after campaign. Its start/end load averages and all samples' summary statistics are retained in `save-cost.log` and `save-cost-result.json`.

`target/` allocated **33,113,411,584 bytes (30.84 GiB)** after the gates; free disk was 291.13 GiB. The local command scripts, logs, per-suite count files and frozen source hashes are in `target/run-limit-followups/`. No provider command, paid run, credential access, push or local Windows runtime qualification occurred. Every own build and test finished before the merge commit.
