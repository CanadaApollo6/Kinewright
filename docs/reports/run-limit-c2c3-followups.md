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
