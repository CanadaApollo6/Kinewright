# S2-H review fixes (commit A)

Parent `334da4f99329ea5d914dcb0fe5f331c2c5f647ab`, branch `ja1/s2`, worktree `/home/riels/.t3/worktrees/Kinewright/t3-cf687318`. No push. The final commit binder ties the tested Rust/Cargo files, fixtures, mutation sources and timed binaries to the resulting SHA. Commit B's history-cost work has not started in this commit.

## Finding, fix, witness and mutation

| Finding | Owning fix | Real-path witness | Mutation |
|---|---|---|---|
| Astra 1: legacy handoff | Enable Studio refuses a named legacy file before stopping its original ordinary writer. Existing editing and legacy Save stay available, including media that Studio cannot yet admit. Lower-level host handover also requires a published Studio root before release. This follows plan §3.1 separate legacy mode and §5.1 validated handover, plus JA-1 §5.1; no legacy-in-host format is added. | U `s2h_legacy_unsaved_activation_keeps_local_save_and_exact_reopen`: actual Enable Studio with unsaved edits and a legacy media pool, original writer retained, unchanged full live history, ordinary Save exact bytes and independent Document reopen. U `s2h_direct_legacy_handoff_keeps_exact_history_until_local_studio_save` separately verifies lower-level refusal and existing explicit local conversion for an admitted fixture. | `legacy-enable-replaces-local-writer`, `legacy-handover-releases-before-studio-save` |
| Muse 3: bootstrap failure | Reap failed bootstrap, reacquire original named writer, verify expected root under pinned authority, resume retained cut. Post-discovery failure retains L2. | U `s2h_unlaunchable_host_restores_local_writer_and_both_saves`: missing and corrupt executable; same-path and fresh-path saves, exact independent reopen. | `failed-bootstrap-never-reacquires-local-writer` |
| Astra 2: unknown reply | Freeze unknown outcomes without losing original pending envelopes or switch request. Existing alive host executes C10 reconciliation; committed commands are not reapplied and absence uses existing authorization. Confirmed owned-child death stays L2. | U `s2h_manual_and_save_reply_loss_reconcile_originals_and_kill_is_l2`, plus revised real-error C11 switch observer. A real TCP proxy drops the actual HTTP response after publication; App receives an error. A second dropped reply during barrier publication preserves the same live child and permits another Retry. | `unknown-reply-latches-retention-loss`, `confirmed-death-is-treated-as-unknown`, `recovery-reply-loss-drops-owned-host` |
| Astra 3: installed B | Retain B cut, committed attachment and staged view until confirmation. Retry confirms that same identity without reinstalling or resuming A. Host caches one exact grant bound to the original installation and current manual pointer. Duplicate confirmation returns that lease even if unrelated root metadata changes. | U `s2h_confirm_reply_loss_retries_installed_b_without_reinstallation`: before processing and after reply; same staged actor, first edit/Undo, return and stale permit refusal. | `installed-switch-state-is-discarded`, `duplicate-confirm-refuses-original-grant`, `duplicate-confirm-binds-unrelated-root-metadata` |
| Muse 2: partial confirmation | Acquire lease and inputs together; revoke partial grant and refreeze on failure. Retain bounded continuation identity across lease-issuer publication while requiring the same manual pointer. | H `s2h_confirm_inputs_failure_revokes_partial_grant_and_refreezes`, existing root-rename hook removes input object after lease issuance, restores bytes, retries and checks one live lease/duplicate generation. | `failed-confirm-keeps-undelivered-lease` |
| Muse 1: producers | Verified Source dispatch uses `send_manual_command`. Relink and LUT refuse before probes/workers or optimistic status/reservation where replacement input bytes cannot be admitted. Rescan also migrates still/video/A-V placement and stranded A/B restoration. | M `s2h_real_media_dispatch_uses_manual_gate_or_refuses_before_side_effects`, actual dispatches on one real host, admitted synthetic media bytes, real LUT store/import completion, exact accepted batch counts. This is input/control admission, not decoder certification. | 9 producer mutations: Source, two relink, two LUT, three placements and A/B restoration |
| Astra 4: disclosure cost | Memoize Save disclosure by actor/history-change/limits and registered-session hashes by actor/history-change. No cached history payload or second ledger. Root, session dirty/unsettled and selection state still determine current disclosure. | H `s2h_status_disclosures_cache_and_invalidate_actual_histories`: repeated Status does no history selection/hash; actual session edits update unsaved warnings, current edit invalidates selection. Paired timing observer below. | Four cache-use/invalidation mutations |
| Muse 6: anchors | Two old C8 anchors name current C12 replacements. The moved failed-launch source-close anchor names its new S2-H case. C8 exact bootstrap snapshot and C10 intermediate receipts are re-anchored. Existing C12 confirmation/cache anchors target their current owners. | All current C12 anchors unique; moved cases rerun against current owners. | `confirmation-trusts-supplied-pointer`, fork/reopen registration, registered Close warning, current App source-close, C8 exact bootstrap snapshot and C10 intermediate receipts; `s2h-prelaunch-failure-implicitly-closes-source` |

U = `ja1_attachment::tests::`, H = `writer_host::s2h_tests::`, M = `media_workflow::tests::`. The old App switch reply-drop bypass is replaced by a real failed HTTP request path. A runtime proxy can discard a request before processing or a reply after processing; it does not supply product semantics.

The final full-gate development run exposed two integration mismatches. A received typed
`project_not_saved` refusal must keep its Save-once guidance, so only unknown responses enter
transport reconciliation. `known-refusal-becomes-unknown-reply` fails the real C9 bootstrap
witness and passes after restoration. The older C8 missing-binary test now requires Live
editing and an exact independent First Save reopen while preserving GUI source bytes/charges.
It no longer expects permanent L2 before host acquisition. The strengthened host confirmation
witness also forks an unrelated durable branch between confirms, proving that unchanged manual
authority survives a root-metadata publication without a second lease.

## Boundaries and retained deferrals

All original 110 C12 fixture/index files remain byte-identical; no historical fixture/oracle or identity definition changes. No P copy, retained-manual descriptor, read-only working role, editable pause, Continue, special headroom or retention bypass returns. The pending installed attachment is the explicitly required finite confirmation state. The original selected-target pin crash window remains accepted.

JA-1 now states: the host bearer carries editor authority while no manual lease is held; S4 binds an editor credential at attach. This is the requested disclosure, not an S4 credential implementation. The existing test-only OwnedMedia ID/kind admission does not certify decoder bytes or introduce a production media producer; its fingerprint binding remains owed before that producer.

All added durable witnesses are Linux-only. Added Windows runtime is estimated 0 s; portable production/test-helper compilation allowance is 30–90 s, within the approximately two-minute budget. This is a source-based estimate, not native timing. Both cfg branches were inspected, Windows-only imports/helpers remain gated appropriately, and no OS cfg was added in `app.rs`. Native Windows build/lint/runtime and hosted CI remain NOT RUN here, owned by the lead. Physical device/power-loss, native providers/G6, G1-W/G7-D/G20, interactive display/product acceptance and Kani remain NOT RUN.

## Development exclusions

An interrupted campaign preceded the portable App-helper cfg correction. A LUT-start mutant survived because the first fixture lacked a named LUT store; the final fixture supplies a real store and that mutant has a runtime red/restored green. The registration mutant initially omitted the new digest-cache initializer and failed compilation; its corrected runtime pair replaces that rejected result. These artifacts are preserved under development names and receive no red credit. The final 32 per-case records all have exactly one runtime failure then restored pass, with original source restoration and executed Rust/Cargo identities audited. Acceptance uses only the audited final per-case records.

## Raw producer re-scan

Command: `rg -n -U -C 1 '\bcore(?:\(\))?\s*\.\s*send\s*\(' crates/kinewright-app/src --glob '*.rs'`. The retained output is `producer-rescan.txt`.

| Remaining raw call | Disposition |
|---|---|
| `project.rs:709` | Local fallback only when `ProjectSession.ja1` is absent; otherwise dispatches `Ja1Attachment::send_manual`. |
| `ja1_attachment.rs:431` | Local/no-manual fallback. A bound active host gate uses `gate.send` immediately above; Frozen/Sealed bootstrap or lost-retention actors refuse raw mutations. |
| `app.rs:8171` | Test-only pre-existing incident/proposal setup. |
| `app.rs:8441` | Test-only pre-existing incident/proposal setup. |
| `app.rs:8826` | Test-only deliberate rejected operation. |
| `app.rs:9023` | Test-only pre-existing Undo control. |
| `app.rs:17310` | Test-only deliberate raw bootstrap refusal and actual UI-event-drain message control. |
| `ja1_attachment_tests.rs:1791` | Test-only deliberate frozen-actor bypass/refusal control. |

The seven production producer sites found in the review/rescan now use the gate or refuse before unavailable input admission: `media_workflow` verified Source, relink, LUT; `media_bin` still, video and A/V placements; `inspector_ui` stranded A/B restoration. Their real dispatch witness and nine independent mutation pairs execute these paths. Existing import/agent/recovery producers already use the central gate; no production OwnedMedia producer is introduced.

The first gate frame stopped at an empty media selector because the selection helper requires a full test name rather than an arbitrary prefix. Only the committed wrapper selector changed; product/witness Rust and all mutation binaries stayed unchanged. That partial frame is excluded, and the corrected complete gate is the source-bound frame reported below.

The earlier local-migration-only development route did not cover a legacy media pool without
Studio input admission. The final App refuses named legacy activation before its ordinary
writer changes, and the strengthened real App witness verifies ordinary Save byte-for-byte.
Earlier source frames are excluded; the final mutation/gate frame includes this correction.

The next gate frame was deliberately paused after a source inspection identified cleanup of a live recovery host on a second lost reply. The strengthened real App witness reproduced loss of the owned-child handle at runtime. The worker now returns its same client/child with a paused completion, so another Retry keeps original evidence and finishes without killing retained authority. Its independent mutation restores the erroneous child-drop behavior. Every final pair and gate is rerun after this source change.

Control-process deviation: some early, short Python JSON/inventory commands ran at normal priority. Cargo uses jobs 4/nice 10 throughout; all timed children use native nice 10, and timing annotation/sampling uses nice 19. No build or unrelated test overlaps a timed lane.

The first timing frame rejected the Core-only 65,534-command cap during host setup, before any capped command was timed. Core serialized-byte admission differs from the format-defined decoded-record charge used by Studio. That partial frame is retained as development evidence and excluded. The separate host status cap is the maximum admitted original coalesced prefix: 5,737 commands, 2,377,129 encoded bytes, charge 33,550,959 of 33,554,432. Its next prefix charges 33,556,800 and the actual owner setup refuses it. The fixture utility verifies new, first Save, two real forks and three Saves without pruning or raising limits. The standalone drag cap remains the original 65,534 seed leading to 65,536 accepted commands. All final timing lanes rerun from scratch after this fixture correction; product/witness source and the completed gate are unchanged.

The initial large-document fixture failed Core replay before timing because its copied batch identity effects still retained only clip 1. It is excluded. The replacement is generated with 16 actual coalesced commands over the same 500-clip baseline; each batch retains the real 500 IDs, and independent Core replay succeeds. The unchanged 45 accepted lanes keep their exact binaries/fixtures; five replacement large lanes plus the separate component diagnostic run under `E/timings-A-large/`. `E/timing-lane-provenance-A.json` identifies every accepted lane and config. The invalid fixture and failed setup log remain retained. No product or witness source changes.

The first five-pair Save block averaged 240.610 / 267.361 ms for 2aa6633/A. A run means ranged 220.477-311.414 ms. The inflated first three A runs spent 150-186 ms/save in nested sync versus 102-122 ms in the last two; publication fit stayed about 41-47 ms, total process CPU about 17.5 s, and the same intervals had heavy external-process CPU flags. The ordinary Save implementation/target are unchanged from C12. One fixed additional five-pair block uses the same sealed binaries immediately, without changing system settings, waiting for quiet, or dropping initial observations. Both blocks and their combined result are reported.

## Final source and reproducible commands

Evidence root `E = /home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1`. Frozen gate source-manifest SHA-256 `6f6db3491b76043b0c822bed921988b20ddc3b797c172ba951c2bf06d97fd70a`. Before/after manifests match exactly. `E/commit-A-binding.json` binds tested blobs to the final one-child commit; this report/README and one audited trailing-space correction to the gate wrapper are completed after the gate. `E/fixtures-A.json` records all 110 unchanged original fixture/index hashes. `E/mutation-anchors-final-A.json` records unique current C8/C10/C12/S2-H anchors or explicit replacements.

Run from the named worktree. The tracked gate retains the C12 selections/configurations and adds the new real witnesses. Every expanded command is retained in `E/gate-A/<label>.command`; selected names, raw stdout/stderr, binary hashes and counts are beside it.

```bash
bash scripts/ja1-s2h-mutations.sh /home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/mutations-A
bash docs/reports/ja1-s2/s2h-gate.sh /home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-A 334da4f
```

The gate sources `scripts/setup-ffmpeg.sh` in the Cargo shell, uses `CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4`, native nice 10, private target/TMPDIR roots, one heavy Cargo at a time and at least 6 GiB available memory. Media uses one test thread. Slow selectors execute directly with their feature. The whole `mcp_server` binary runs unfiltered.

The exact top-level commands and selection calls are also visible in [s2h-gate.sh](s2h-gate.sh). All command rows below exit zero.

| Command row | Result |
|---|---|
| `rustfmt --edition 2024 --config skip_children=true --check crates/kinewright-agent/src/writer_host/mod.rs crates/kinewright-agent/src/writer_host/s2h_tests.rs crates/kinewright-agent/src/writer_host/switch.rs crates/kinewright-app/src/app.rs crates/kinewright-app/src/inspector_ui.rs crates/kinewright-app/src/ja1_attachment.rs crates/kinewright-app/src/ja1_attachment_tests.rs crates/kinewright-app/src/ja1_host.rs crates/kinewright-app/src/ja1_host_recovery.rs crates/kinewright-app/src/ja1_host_transport_tests.rs crates/kinewright-app/src/media_bin.rs crates/kinewright-app/src/media_workflow.rs crates/kinewright-project/src/editing_session.rs crates/kinewright-project/src/editing_session/durable.rs crates/kinewright-project/src/editing_session/recovery.rs crates/kinewright-project/src/editing_session/switch.rs` | 0 |
| `nice -n 10 cargo build --workspace` | 0 |
| `nice -n 10 cargo build -p kinewright-agent --no-default-features --lib` | 0 |
| `nice -n 10 cargo build -p kinewright-app --no-default-features --bin kinewright-app` | 0 |
| `nice -n 10 cargo build -p kinewright-agent --no-default-features --bin kinewright` | 0 |
| `nice -n 10 cargo build -p kinewright-agent --features eval-harness --bins` | 0 |
| `nice -n 10 cargo clippy --workspace --all-targets -- -D warnings` | 0 |
| `nice -n 10 cargo test -p kinewright-core --all-targets --no-fail-fast -- --test-threads=1` | 0 |
| `nice -n 10 cargo test -p kinewright-project --features test-util --all-targets --no-fail-fast -- --test-threads=1` | 0 |
| `nice -n 10 cargo test -p kinewright-agent --features eval-harness --all-targets --no-fail-fast -- --test-threads=1` | 0 |
| `nice -n 10 cargo test -p kinewright-app --all-targets --no-fail-fast -- --test-threads=1` | 0 |
| `actionlint .github/workflows/ci.yml` | 0 |
| `nice -n 19 python3 -I scripts/slow_tests.py lint` | 0 |
| `nice -n 19 python3 -I scripts/slow_tests.py features` | 0 |
| `env -u JA_SWITCH_FIXTURE_OUTPUT -u JA_HARNESS_APP_TEST_BINARY -u JA_S1_PREDECESSOR_BINARY -u JA_SAVE_BENCH_OUTPUT nice -n 10 cargo test --workspace --no-fail-fast -j 4 -- --test-threads=1` | 0 |
| `nice -n 19 python3 -I scripts/ja1-s2-c10-fast-log.py /home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-A/workspace-fast.log /home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-A/workspace-fast.stderr /home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-A/workspace-fast-structured.log` | 0 |
| `nice -n 19 python3 -I scripts/slow_tests.py verify-fast /home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-A/workspace-fast-structured.log` | 0 |

All **99 command rows** exit zero. The affected set has intentional overlaps: **865 expected / 865 selected / 841 executed passes / 24 ignored NOT RUN**, zero failures.

| Selection | Expected / selected / run | Ignored NOT RUN | Raw result |
|---|---:|---:|---|
| `agent-acp-session` | 9 / 9 / 9 | 0 | `test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 690 filtered out; finished in 5.03s` |
| `agent-drivers` | 11 / 11 / 11 | 0 | `test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 688 filtered out; finished in 0.00s` |
| `agent-rescue-stress` | 1 / 1 / 1 | 0 | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 8.17s` |
| `agent-scripted` | 5 / 5 / 5 | 0 | `test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 694 filtered out; finished in 2.27s` |
| `app` | 191 / 191 / 187 | 4 | `test result: ok. 187 passed; 0 failed; 4 ignored; 0 measured; 682 filtered out; finished in 193.99s` |
| `app-rescue-stress` | 1 / 1 / 1 | 0 | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 872 filtered out; finished in 26.58s` |
| `arch-agent-eval` | 1 / 1 / 1 | 0 | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 698 filtered out; finished in 0.02s` |
| `arch-agent-production` | 1 / 1 / 1 | 0 | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 594 filtered out; finished in 0.02s` |
| `arch-app-default` | 1 / 1 / 1 | 0 | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 872 filtered out; finished in 0.02s` |
| `arch-app-production` | 1 / 1 / 1 | 0 | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 872 filtered out; finished in 0.02s` |
| `core-actor` | 24 / 24 / 23 | 1 | `test result: ok. 23 passed; 0 failed; 1 ignored; 0 measured; 297 filtered out; finished in 0.35s` |
| `core-attachment` | 8 / 8 / 8 | 0 | `test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 313 filtered out; finished in 0.15s` |
| `core-history` | 13 / 13 / 13 | 0 | `test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s` |
| `harness-lib` | 21 / 21 / 21 | 0 | `test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 678 filtered out; finished in 7.33s` |
| `host-integration` | 20 / 20 / 16 | 4 | `test result: ok. 16 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 16.07s` |
| `host-lib` | 45 / 45 / 45 | 0 | `test result: ok. 45 passed; 0 failed; 0 ignored; 0 measured; 550 filtered out; finished in 38.29s` |
| `host-recovery-absent-matrix` | 1 / 1 / 1 | 0 | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 19 filtered out; finished in 15.39s` |
| `host-recovery-floor-matrix` | 1 / 1 / 1 | 0 | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 19 filtered out; finished in 14.01s` |
| `host-recovery-pressure` | 1 / 1 / 1 | 0 | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 20 filtered out; finished in 3.40s` |
| `host-switch-matrix` | 1 / 1 / 1 | 0 | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 19 filtered out; finished in 7.61s` |
| `ja_baseline` | 13 / 13 / 13 | 0 | `test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 8.72s` |
| `ja_s1` | 20 / 20 / 20 | 0 | `test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 11.56s` |
| `ja_s1_branches` | 5 / 5 / 5 | 0 | `test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.96s` |
| `ja_s1_copy` | 6 / 6 / 6 | 0 | `test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.75s` |
| `ja_s1_receipts` | 9 / 9 / 9 | 0 | `test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 54.22s` |
| `ja_s1_storage` | 86 / 86 / 86 | 0 | `test result: ok. 86 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 108.53s` |
| `ja_s2_rescue` | 35 / 35 / 35 | 0 | `test result: ok. 35 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 23.53s` |
| `ja_s2_rescue_record` | 33 / 33 / 33 | 0 | `test result: ok. 33 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 175.46s` |
| `ja_s2_save` | 13 / 13 / 13 | 0 | `test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 65.19s` |
| `ja_s2_save_cost` | 1 / 1 / 1 | 0 | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 20.77s` |
| `ja_s2_source_boundary` | 16 / 16 / 16 | 0 | `test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.76s` |
| `ja_s2_switch` | 7 / 7 / 7 | 0 | `test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 8.05s` |
| `ja_s2_writer` | 7 / 7 / 7 | 0 | `test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.89s` |
| `local-agent-admission` | 1 / 1 / 1 | 0 | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.91s` |
| `mcp-server` | 86 / 86 / 76 | 10 | `test result: ok. 76 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 71.56s` |
| `media-generated` | 17 / 17 / 14 | 3 | `test result: ok. 14 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out; finished in 2.63s` |
| `media-pins` | 10 / 10 / 10 | 0 | `test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 1189 filtered out; finished in 3.29s` |
| `project-lib` | 140 / 140 / 138 | 2 | `test result: ok. 138 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 91.02s` |
| `specialist-identity` | 1 / 1 / 1 | 0 | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 698 filtered out; finished in 1.00s` |
| `specialist-namespace` | 1 / 1 / 1 | 0 | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 698 filtered out; finished in 3.16s` |
| `specialist-registration` | 1 / 1 / 1 | 0 | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 698 filtered out; finished in 1.44s` |

Complete targets run before the final workspace fast tier.

| Target group | Passes | Failures | Ignored NOT RUN |
|---|---:|---:|---:|
| `full-core` | 805 | 0 | 1 |
| `full-project` | 351 | 0 | 2 |
| `full-agent` | 876 | 0 | 32 |
| `full-app` | 867 | 0 | 6 |
| `workspace-fast` | 4066 | 0 | 111 |

Workspace fast failure list: `[]`. All raw full-target `test result:` lines are retained below.

```text
full-core
test result: ok. 320 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 1.47s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 59 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.82s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 39 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 65 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 113 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.08s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
full-project
test result: ok. 138 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 131.70s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 8.30s
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 61.85s
test result: ok. 86 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 224.86s
test result: ok. 35 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 59.88s
test result: ok. 33 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 206.61s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 67.14s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 20.82s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.09s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.72s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.97s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
full-agent
test result: ok. 698 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 150.93s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 42 passed; 0 failed; 5 ignored; 0 measured; 0 filtered out; finished in 2.57s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 10 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out; finished in 20.58s
test result: ok. 19 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 33.32s
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.91s
test result: ok. 17 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 37.39s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.83s
test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 76 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 71.68s
test result: ok. 0 passed; 0 failed; 7 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
full-app
test result: ok. 867 passed; 0 failed; 6 ignored; 0 measured; 0 filtered out; finished in 380.14s
workspace-fast
test result: ok. 698 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 151.92s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 42 passed; 0 failed; 5 ignored; 0 measured; 0 filtered out; finished in 2.46s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 10 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out; finished in 20.29s
test result: ok. 19 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 32.91s
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.93s
test result: ok. 17 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 37.53s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.85s
test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 76 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 70.45s
test result: ok. 0 passed; 0 failed; 7 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 867 passed; 0 failed; 6 ignored; 0 measured; 0 filtered out; finished in 369.63s
test result: ok. 320 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 1.39s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 59 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.89s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 39 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 65 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 113 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.09s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1134 passed; 0 failed; 65 ignored; 0 measured; 0 filtered out; finished in 1022.57s
test result: ok. 4 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 22.37s
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.81s
test result: ok. 14 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out; finished in 2.69s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 138 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 136.38s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.96s
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 61.21s
test result: ok. 86 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 231.25s
test result: ok. 35 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 61.80s
test result: ok. 33 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 201.19s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 67.82s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 20.89s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.79s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 13.07s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.07s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

## Final mutation results

Every row has exactly one executed runtime failure, then one restored pass. Compile failures, skips, setup failures, survivors and earlier source frames receive no red credit. Every restored-green binary equals a binary used by the final gate. Per-case original/executed source identities and failure reasons are in `E/mutations-A/` and `E/mutation-failure-reasons-A.json`.

| Mutation | Runtime red | Restored green |
|---|---|---|
| `legacy-enable-replaces-local-writer` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 872 filtered out; finished in 0.44s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 872 filtered out; finished in 0.47s` |
| `legacy-handover-releases-before-studio-save` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 872 filtered out; finished in 0.24s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 872 filtered out; finished in 0.96s` |
| `failed-bootstrap-never-reacquires-local-writer` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 872 filtered out; finished in 0.29s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 872 filtered out; finished in 1.84s` |
| `unknown-reply-latches-retention-loss` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 872 filtered out; finished in 1.11s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 872 filtered out; finished in 2.98s` |
| `confirmed-death-is-treated-as-unknown` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 872 filtered out; finished in 1.35s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 872 filtered out; finished in 2.98s` |
| `installed-switch-state-is-discarded` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 872 filtered out; finished in 1.36s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 872 filtered out; finished in 5.12s` |
| `duplicate-confirm-refuses-original-grant` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 872 filtered out; finished in 1.38s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 872 filtered out; finished in 5.06s` |
| `failed-confirm-keeps-undelivered-lease` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 594 filtered out; finished in 1.11s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 594 filtered out; finished in 1.67s` |
| `source-dispatch-bypasses-permit` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 872 filtered out; finished in 0.99s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 872 filtered out; finished in 1.02s` |
| `relink-dispatch-bypasses-input-refusal` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 872 filtered out; finished in 0.86s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 872 filtered out; finished in 1.07s` |
| `lut-dispatch-bypasses-input-refusal` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 872 filtered out; finished in 1.21s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 872 filtered out; finished in 0.97s` |
| `relink-starts-probe-before-admission` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 872 filtered out; finished in 1.16s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 872 filtered out; finished in 1.32s` |
| `lut-starts-worker-before-admission` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 872 filtered out; finished in 0.93s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 872 filtered out; finished in 1.27s` |
| `still-placement-bypasses-permit` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 872 filtered out; finished in 1.13s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 872 filtered out; finished in 0.97s` |
| `av-placement-bypasses-permit` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 872 filtered out; finished in 0.83s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 872 filtered out; finished in 2.85s` |
| `video-placement-bypasses-permit` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 872 filtered out; finished in 0.85s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 872 filtered out; finished in 1.29s` |
| `stranded-ab-restore-bypasses-permit` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 872 filtered out; finished in 0.82s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 872 filtered out; finished in 0.97s` |
| `save-disclosure-cache-is-never-used` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 594 filtered out; finished in 0.65s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 594 filtered out; finished in 0.83s` |
| `registered-digest-cache-is-never-used` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 594 filtered out; finished in 0.91s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 594 filtered out; finished in 0.52s` |
| `registered-digest-never-invalidates` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 594 filtered out; finished in 0.61s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 594 filtered out; finished in 0.39s` |
| `save-disclosure-never-invalidates` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 594 filtered out; finished in 0.48s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 594 filtered out; finished in 0.49s` |
| `known-refusal-becomes-unknown-reply` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 872 filtered out; finished in 1.78s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 872 filtered out; finished in 3.04s` |
| `duplicate-confirm-binds-unrelated-root-metadata` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 594 filtered out; finished in 0.99s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 594 filtered out; finished in 0.96s` |
| `s2h-prelaunch-failure-implicitly-closes-source` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 872 filtered out; finished in 0.10s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 872 filtered out; finished in 0.32s` |
| `recovery-reply-loss-drops-owned-host` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 872 filtered out; finished in 1.05s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 872 filtered out; finished in 2.76s` |
| `confirmation-trusts-supplied-pointer` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 19 filtered out; finished in 1.43s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 19 filtered out; finished in 4.95s` |
| `host-fork-without-registration` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 594 filtered out; finished in 0.83s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 594 filtered out; finished in 1.53s` |
| `host-reopen-without-registration` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 594 filtered out; finished in 1.18s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 594 filtered out; finished in 1.55s` |
| `repeat-c5-registered-close-warning-omitted` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 698 filtered out; finished in 0.88s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 698 filtered out; finished in 3.29s` |
| `s2h-app-host-disconnect-implicitly-closes-source` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 872 filtered out; finished in 0.94s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 872 filtered out; finished in 0.92s` |
| `app-bootstrap-replaces-frozen-gui-history-with-empty-core` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 872 filtered out; finished in 0.16s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 872 filtered out; finished in 0.93s` |
| `intermediate-recovery-receipts-omitted` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 20 filtered out; finished in 3.37s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 20 filtered out; finished in 3.53s` |

## Paired timing observations

All binaries were built before timing and copied to hash-named sealed files. `E/timing-source-audit-A.json` verifies exact baseline archives; `334da4f` adds only the identical status observer and module declaration. Current binaries bind to the frozen A source. The `2aa6633` Save target is rebuilt inside this authorized evidence/worktree area; the old retained C12 binary is never executed because its embedded temporary root points outside this task.

`E/timings-A/config.json`, supplemental configs and `E/timing-lane-provenance-A.json` give exact order, paths, hashes, arguments and environments. The main and supplemental timing directories retain results, native wait4 resource/RSS records, uptime, instantaneous top-five CPU, GPU state/clocks and sampled external CPU flags. Timed children run at native nice 10, annotation/sampling at nice 19. There are five alternating pairs per status size and ten alternating official Save pairs in two fixed five-pair blocks. Status numbers below are the median of five per-run medians, with the range of those medians. The owner is the real serialized `HostCommandOwner`; this is a debug common-status command measurement, not HTTP, frame or Windows acceptance.

| Status history seed | 334da4f median ms [range] | A median ms [range] |
|---|---:|---:|
| 16 commands | 2.376 [2.343, 2.501] | 1.536 [1.484, 1.722] |
| 5,737 commands | 177.896 [176.673, 180.179] | 1.507 [1.494, 1.569] |

Repeated unchanged Status executes zero full-history disclosure selections/hashes in the trace witness. Actual manual/session history changes invalidate it, and the first subsequent disclosure recomputes. The paired timings cover unchanged-history commands. This identifies the removed work; remaining root/status/actor queries remain. The observer also records three unchanged-history Host Save commands per run as diagnostics; they are not the ordinary Save budget measurement.

The unchanged-history Host Save diagnostics are in `E/timing-summary-A.json`, fifteen commands per side per history size. Snapshot capture precedes timing. They are distinct from the ordinary Save budget.

The ordinary Save lane is the unchanged six-branch `ja_s2_save_cost` target in the debug test profile. It makes twelve actual comment-changing Saves per run and independently reopens the result. Ten runs give 120 Saves per side, retaining both five-pair blocks.

| Official Save side | Mean ms/save | Median run mean ms [range] | Full history decodes | Sync share |
|---|---:|---:|---:|---:|
| 2aa6633 | 243.152 | 243.912 [233.441, 252.562] | 0 | 50.2% |
| A | 253.467 | 240.886 [220.477, 311.414] | 0 | 51.8% |

| Save block | 2aa6633 mean ms | A mean ms |
|---|---:|---:|
| initial | 240.610 | 267.361 |
| recheck | 245.694 | 239.573 |

These are observed paired results, with all ranges/load annotations retained. No comparison to an old archived timing earns a speedup claim. The table reports the measured sync share. Detailed fit/sync/charge/verified-byte counters are retained; nested durations are not additive.

### Commit A manual-command baseline for B

The existing release `drag-probe` is rebuilt without source changes against A. Small/cap fixtures are the exact old decompressed bytes. The 4,096 lane uses its generated prefix and the large-document lane has 500 clips. Every run restores the fixture through Core and checks both accepted envelopes against fresh full identities; verification and fixture loading are outside timing. Each of five runs times one warmup plus one measured accepted command. The table uses only the five measured commands. This is `ManualPermit::request` through the actual actor/encoder, without a GUI render or host transport.

| After accepted commands | Clips | A median ms [range] | B budget |
|---|---:|---:|---:|
| 18 | 1 | 0.265 [0.218, 0.278] | no worse than A beyond noise |
| 4,096 | 1 | 22.175 [21.228, 23.657] | <= 1 ms |
| 65,536 | 1 | 356.922 [355.150, 360.311] | <= 4 ms |
| 18 | 500 | 2.935 [2.903, 4.862] | reported, not gated |

Each lane has 10 accepted commands, 10 exact-envelope verifications and zero failures. A separate unreported comparison sample attributes 196.589 ms of 354.733 ms at the cap to the existing encoder; it is excluded from the five budget observations. Core/manual identity code is unchanged by A. The history-proportional manual path remains for commit B.

The sampler records 117 external-process observations above 20% CPU; see `E/timing-contamination-A.json`. Short lanes may finish before a five-second sample, so instantaneous start/end CPU annotations are retained too. None of the lanes is silently discarded.

### Unchanged-history Host Save commands

Median and range across fifteen commands per side/history size, from five alternating runs. Snapshot capture precedes timing. The ordinary six-branch Save workload above remains the budget measurement.

| History seed | 334da4f ms [range] | A ms [range] |
|---|---:|---:|
| 16 | 102.576 [94.861, 110.701] | 102.999 [95.236, 117.131] |
| 5,737 | 943.489 [898.682, 969.249] | 945.318 [899.545, 974.960] |

All 61 accepted timing lanes are listed in the provenance manifest. The two failed fixture setups are retained and receive no timing credit. The lead owns the independent review round; none was run by this implementation task.

Tracked contract correction: `docs/JA1-STUDIO-ORCHESTRATION.md:1490` records legacy refusal/rollback, unknown outcomes, installed confirmation and the deferred S4 bearer rule. `E/windows-source-audit-A.md` records cfg applicability and the per-test Windows estimate.

The staged whitespace check caught a trailing space outside the quoted App selector at `s2h-gate.sh:102`. The first unpublished A commit was amended to remove that one byte. `E/gate-wrapper-whitespace-A.json` retains the exact executed/final wrapper bytes and hashes; shell arguments and all 99 executed commands are unchanged. `bash -n` and the staged whitespace check pass after correction. All tested Rust/Cargo, fixtures and mutation sources remain byte-identical. No product or witness source changes after the gate.
