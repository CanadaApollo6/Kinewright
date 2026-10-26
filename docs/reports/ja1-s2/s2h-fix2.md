# S2-H constant-factor history cost reduction (commit B)

Parent `8fc930ec18e9d928aebe88967d5654afabe86c9b` is accepted and pushed by the lead. This work leaves A unchanged and creates one unpushed B on `ja1/s2` in `/home/riels/.t3/worktrees/Kinewright/t3-cf687318`. The resulting SHA is bound after commit in `commit-B-binding.json` under the evidence directory below.

Evidence directory `E = /home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1`. `round3-final-audit.json` binds the complete source, gate binaries, mutations and fixtures. `round3-timing-provenance.json` binds all fifty official timing lanes. Logs and binaries are evidence only and are not committed.

## Result and scope

The five-pair release median at 4,096 accepted commands is **1.643 ms**, passing the 2 ms gate. Small history and the 500-clip lane both improve. Ordinary six-branch Save remains around 235 ms. The cap median is **24.783 ms**, missing the report-only 16.7 ms target. This commit proceeds under the Round 3 ruling and reports the remaining phases below.

Every v1 identity keeps the definition `SHA256(serde_json::to_vec(HistorySnapshot))`. `HistorySnapshot` and durable envelope/receipt formats are unchanged. The fast stream reproduces every sampled canonical JSON byte, not a reordered or structural hash. The previous accepted after digest supplies the next before digest. No v2 identity, contract amendment, new external dependency, P copy, retained-manual descriptor, read-only working role, editable pause, Continue, headroom or retention bypass is introduced.

## Finding, fix, witness and mutation

| Requirement | Owning implementation | Witness | Runtime mutation |
|---|---|---|---|
| Carry identity and serialize once | `actor/manual_history.rs:45` carries the accepted digest; `HistoryJson` at `:350` caches baseline, closed batches and unchanged cursor segments. Only document and changed tails are serialized; after and cursor identities stream the exact v1 bytes. Project `manual_history.rs:84` retains the existing envelope construction and input admission. | `actor::manual_history::tests::s2h_cached_history_matches_full_after_every_command`, randomized/coalesced/empty Undo/Redo/branch commands. After every command it compares fresh full digest, cursor digest, every serialized byte and encoded length. All four real drag fixtures additionally verify the production encoder envelopes against fresh full identities. | `stale-cached-history-segment` |
| Avoid full candidate history copies | `CandidateDelta` at `actor/manual_history.rs:154` retains affected stack tails, document identity and lengths, then rolls back every execution/encoder/control refusal. The existing floor-only `CoreState::clone` remains outside manual acceptance. | `s2h_cached_history_rolls_back_encoder_and_admission_refusals`, including input/codec/sequence/byte/backpressure refusal, redo preservation and successful acceptance afterward. | `refused-candidate-keeps-open-operations` |
| Invalidate after external history changes | Private `history_epoch` at `actor.rs:747` changes on floor settlement and history replacement. Surviving ingress rebuilds once before the next command. Install/restore still obey existing actor authority. | `s2h_cached_history_rebinds_after_floor_restore_and_cursor_changes`, including a surviving ingress, a refused next command, subsequent acceptance and independent wire restore. | `floor-keeps-stale-history-cache` |
| Exact incremental byte admission | `encoded_bytes` at `actor/manual_history.rs:489` sums measured cached segment lengths and document bytes. Command/message limits remain unchanged. `BoundedJson` at `:260` refuses oversized chunks before extending the new serialized allocation; prefix, document and tail use it. | `s2h_cached_history_rejects_exact_byte_limit_then_accepts`, total-history refusal before encoder entry, exact limit and limit+1, a bounded writer overflow and a following accepted command. | `history-byte-accounting-skips-total-cap`, `serialized-chunk-allocates-before-cap-refusal` |
| Runtime SHA-NI with fallback | `kinewright-hash/src/lib.rs:159` batches blocks. `x86.rs:13` checks CPU SHA before the sole unsafe kernel call, with local `target_feature(enable = "sha")`. The kernel uses SHA and baseline SSE2, safe references and no raw pointers. Public hash signatures stay unchanged; the software compression body is byte-identical to A. | Six hash tests cover NIST empty/abc/56-byte/112-byte/million-a answers, forced software, real hardware dispatch, every random length 0-10 KiB, 2/3/5 MiB inputs, chunking/padding and arbitrary chaining states. | `sha-ni-round-word-order` |

The hash crate mirrors every current workspace lint except `unsafe_code = "deny"` in place of inherited `forbid`. Cargo cannot locally allow an inherited forbid. The two allowances cover only the runtime-gated wrapper and target-feature kernel. `round3-final-audit.json` verifies lint parity and unchanged software compression. There is no workspace-wide unsafe relaxation.

## Gate

Exact entry point, with FFmpeg setup, four Cargo jobs, nice 10, memory/Cargo-idle guards and media tests at one thread embedded in the script:

```bash
bash docs/reports/ja1-s2/s2h-fix2-gate.sh /home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B 8fc930e
JA_S2H_MUTATION_CASES=scripts/ja1-s2h-fix2-mutation-cases.json bash scripts/ja1-s2h-mutations.sh /home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/mutations-B
```

All **107 command rows pass**. Affected set has **875 expected/selected, 851 passed, 24 existing ignores, zero failures**, across 43 selections. No applicable selector is empty. Source before/after manifests match exactly. All six restored-green binary hashes equal their final gate binary hashes. Every tested Rust/Cargo file matches the final source.

Each row below links its exact executed command. Selection lists and per-test outcomes are retained beside it; compile/list commands are recorded separately in `gate.tsv` and `*.command`. The tracked gate script reproduces all arguments and configurations.

| Gate command | Result |
|---|---|
| [rustfmt](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/rustfmt.command) | Exit 0. |
| [workspace-build](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/workspace-build.command) | Exit 0. |
| [agent-shipping-build](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/agent-shipping-build.command) | Exit 0. |
| [app-shipping-build](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/app-shipping-build.command) | Exit 0. |
| [host-shipping-build](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/host-shipping-build.command) | Exit 0. |
| [eval-build](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/eval-build.command) | Exit 0. |
| [clippy](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/clippy.command) | Exit 0. |
| [hash-windows-check](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/hash-windows-check.command) | Exit 0. |
| [core-windows-check](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/core-windows-check.command) | Exit 0. |
| [hash-windows-clippy](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/hash-windows-clippy.command) | Exit 0. |
| [arch-agent-eval](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/arch-agent-eval.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 698 filtered out; finished in 0.01s` |
| [arch-agent-production](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/arch-agent-production.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 594 filtered out; finished in 0.02s` |
| [arch-app-production](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/arch-app-production.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 872 filtered out; finished in 0.02s` |
| [arch-app-default](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/arch-app-default.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 872 filtered out; finished in 0.02s` |
| [core-actor](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/core-actor.command) | 24 expected/selected; 23 passed; 1 ignored. `test result: ok. 23 passed; 0 failed; 1 ignored; 0 measured; 301 filtered out; finished in 0.33s` |
| [core-attachment](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/core-attachment.command) | 8 expected/selected; 8 passed; 0 ignored. `test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 317 filtered out; finished in 0.03s` |
| [core-history](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/core-history.command) | 13 expected/selected; 13 passed; 0 ignored. `test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| [core-lib](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/core-lib.command) | 4 expected/selected; 4 passed; 0 ignored. `test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 321 filtered out; finished in 0.85s` |
| [hash-lib](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/hash-lib.command) | 6 expected/selected; 6 passed; 0 ignored. `test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.22s` |
| [project-lib](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/project-lib.command) | 140 expected/selected; 138 passed; 2 ignored. `test result: ok. 138 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 88.12s` |
| [ja_s1_storage](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/ja_s1_storage.command) | 86 expected/selected; 86 passed; 0 ignored. `test result: ok. 86 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 103.88s` |
| [ja_s1_receipts](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/ja_s1_receipts.command) | 9 expected/selected; 9 passed; 0 ignored. `test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 52.73s` |
| [ja_s1_copy](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/ja_s1_copy.command) | 6 expected/selected; 6 passed; 0 ignored. `test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.47s` |
| [ja_s2_save](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/ja_s2_save.command) | 13 expected/selected; 13 passed; 0 ignored. `test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 60.37s` |
| [ja_s2_rescue](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/ja_s2_rescue.command) | 35 expected/selected; 35 passed; 0 ignored. `test result: ok. 35 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 23.38s` |
| [ja_s2_rescue_record](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/ja_s2_rescue_record.command) | 33 expected/selected; 33 passed; 0 ignored. `test result: ok. 33 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 156.83s` |
| [ja_s2_writer](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/ja_s2_writer.command) | 7 expected/selected; 7 passed; 0 ignored. `test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.87s` |
| [ja_s2_switch](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/ja_s2_switch.command) | 7 expected/selected; 7 passed; 0 ignored. `test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.37s` |
| [ja_s2_source_boundary](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/ja_s2_source_boundary.command) | 16 expected/selected; 16 passed; 0 ignored. `test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.58s` |
| [ja_s2_save_cost](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/ja_s2_save_cost.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 20.27s` |
| [specialist-identity](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/specialist-identity.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 698 filtered out; finished in 0.95s` |
| [specialist-registration](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/specialist-registration.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 698 filtered out; finished in 1.39s` |
| [specialist-namespace](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/specialist-namespace.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 698 filtered out; finished in 2.97s` |
| [host-lib](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/host-lib.command) | 45 expected/selected; 45 passed; 0 ignored. `test result: ok. 45 passed; 0 failed; 0 ignored; 0 measured; 550 filtered out; finished in 37.83s` |
| [host-integration](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/host-integration.command) | 20 expected/selected; 16 passed; 4 ignored. `test result: ok. 16 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 16.02s` |
| [host-recovery-pressure](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/host-recovery-pressure.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 20 filtered out; finished in 3.11s` |
| [host-recovery-absent-matrix](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/host-recovery-absent-matrix.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 19 filtered out; finished in 15.38s` |
| [host-recovery-floor-matrix](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/host-recovery-floor-matrix.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 19 filtered out; finished in 14.09s` |
| [host-switch-matrix](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/host-switch-matrix.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 19 filtered out; finished in 7.57s` |
| [agent-scripted](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/agent-scripted.command) | 5 expected/selected; 5 passed; 0 ignored. `test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 694 filtered out; finished in 2.25s` |
| [agent-drivers](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/agent-drivers.command) | 11 expected/selected; 11 passed; 0 ignored. `test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 688 filtered out; finished in 0.00s` |
| [agent-acp-session](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/agent-acp-session.command) | 9 expected/selected; 9 passed; 0 ignored. `test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 690 filtered out; finished in 5.02s` |
| [harness-lib](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/harness-lib.command) | 21 expected/selected; 21 passed; 0 ignored. `test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 678 filtered out; finished in 3.14s` |
| [ja_baseline](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/ja_baseline.command) | 13 expected/selected; 13 passed; 0 ignored. `test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.25s` |
| [ja_s1](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/ja_s1.command) | 20 expected/selected; 20 passed; 0 ignored. `test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.71s` |
| [ja_s1_branches](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/ja_s1_branches.command) | 5 expected/selected; 5 passed; 0 ignored. `test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.83s` |
| [app](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/app.command) | 191 expected/selected; 187 passed; 4 ignored. `test result: ok. 187 passed; 0 failed; 4 ignored; 0 measured; 682 filtered out; finished in 179.63s` |
| [local-agent-admission](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/local-agent-admission.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.86s` |
| [agent-rescue-stress](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/agent-rescue-stress.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.92s` |
| [app-rescue-stress](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/app-rescue-stress.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 872 filtered out; finished in 26.23s` |
| [media-pins](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/media-pins.command) | 10 expected/selected; 10 passed; 0 ignored. `test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 1189 filtered out; finished in 3.27s` |
| [media-generated](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/media-generated.command) | 17 expected/selected; 14 passed; 3 ignored. `test result: ok. 14 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out; finished in 2.75s` |
| [mcp-server](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/mcp-server.command) | 86 expected/selected; 76 passed; 10 ignored. `test result: ok. 76 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 70.52s` |
| [full-core](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/full-core.command) | 809 passed; 0 failed; 1 existing ignores. |
| [full-hash](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/full-hash.command) | 6 passed; 0 failed; 0 existing ignores. |
| [full-project](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/full-project.command) | 351 passed; 0 failed; 2 existing ignores. |
| [full-agent](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/full-agent.command) | 876 passed; 0 failed; 32 existing ignores. |
| [full-app](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/full-app.command) | 867 passed; 0 failed; 6 existing ignores. |
| [actionlint](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/actionlint.command) | Exit 0. |
| [slow-lint](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/slow-lint.command) | Exit 0. |
| [slow-features](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/slow-features.command) | Exit 0. |
| [workspace-fast](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/workspace-fast.command) | 4076 passed; 0 failed; 111 existing ignores. |
| [workspace-fast-log](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/workspace-fast-log.command) | Exit 0. |
| [workspace-fast-verifier](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/gate-B/workspace-fast-verifier.command) | Exit 0. |

Full-target raw summaries, including all workspace binaries. Workspace failure list is `[]`; the strict fast-tier verifier passes.

```text
full-agent
test result: ok. 698 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 136.14s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 42 passed; 0 failed; 5 ignored; 0 measured; 0 filtered out; finished in 2.46s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 10 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out; finished in 10.15s
test result: ok. 19 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 15.71s
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.60s
test result: ok. 17 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 35.62s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.86s
test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 76 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 70.82s
test result: ok. 0 passed; 0 failed; 7 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
full-app
test result: ok. 867 passed; 0 failed; 6 ignored; 0 measured; 0 filtered out; finished in 353.86s
full-core
test result: ok. 324 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 2.90s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 59 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.07s
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
full-hash
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.32s
full-project
test result: ok. 138 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 130.71s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.62s
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 60.53s
test result: ok. 86 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 218.67s
test result: ok. 35 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 59.60s
test result: ok. 33 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 205.90s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 66.15s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 20.13s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.90s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.85s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.98s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
workspace-fast
test result: ok. 698 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 141.56s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 42 passed; 0 failed; 5 ignored; 0 measured; 0 filtered out; finished in 2.51s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 10 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out; finished in 10.18s
test result: ok. 19 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 16.36s
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.87s
test result: ok. 17 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 36.86s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.99s
test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 76 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 76.61s
test result: ok. 0 passed; 0 failed; 7 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 867 passed; 0 failed; 6 ignored; 0 measured; 0 filtered out; finished in 354.05s
test result: ok. 324 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 2.80s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 59 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.90s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 39 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 65 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 113 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.09s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.32s
test result: ok. 1134 passed; 0 failed; 65 ignored; 0 measured; 0 filtered out; finished in 1025.80s
test result: ok. 4 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 22.86s
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.82s
test result: ok. 14 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out; finished in 2.85s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 138 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 223.11s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 8.49s
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 64.17s
test result: ok. 86 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 221.69s
test result: ok. 35 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 58.77s
test result: ok. 33 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 204.64s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 65.99s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 20.10s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.72s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.56s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.94s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

## Runtime mutation pairs

All six final pairs have one actual runtime failure, exit 101, then a restored pass, exit 0. Compile/setup failures, skips, survivors and prior source frames receive no red credit. Exact replacements are tracked in `scripts/ja1-s2h-fix2-mutation-cases.json`. Original/executed sources, binary hashes and failure reasons are retained under `E/mutations-B/`.

| Mutation | Runtime red | Restored green |
|---|---|---|
| `stale-cached-history-segment` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 324 filtered out; finished in 0.00s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 324 filtered out; finished in 0.86s` |
| `sha-ni-round-word-order` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 5 filtered out; finished in 0.00s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 5 filtered out; finished in 0.00s` |
| `refused-candidate-keeps-open-operations` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 324 filtered out; finished in 0.00s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 324 filtered out; finished in 0.21s` |
| `floor-keeps-stale-history-cache` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 324 filtered out; finished in 0.01s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 324 filtered out; finished in 0.01s` |
| `history-byte-accounting-skips-total-cap` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 324 filtered out; finished in 0.50s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 324 filtered out; finished in 0.49s` |
| `serialized-chunk-allocates-before-cap-refusal` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 324 filtered out; finished in 0.00s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 324 filtered out; finished in 0.49s` |

All 172 current C12 and 32 A mutation anchors remain unique. None moved, so no prior case required a new mutation run. These are anchor checks, not fresh runtime credit for those historical pairs. All 110 parent fixture/index files remain byte-identical; original S0 fixtures/oracles are unchanged. Existing controls and prior flips are not new B baseline-red claims.

## Paired budgets

The unchanged release drag method is rebuilt against B with only its encoder API wrapper adapted. A uses the retained release binary whose hash and all 332 contributing Rust/Cargo source hashes were verified against accepted `8fc930e`. Both sides use the identical existing fixture bytes. Each run restores the fixture, accepts one warmup and one measured command through the real `ManualPermit::request` actor/production encoder path, then verifies exact envelope before/after identities and document/cursor/batch results outside timing. There are five alternating A/B pairs per lane, with ten accepted and verified commands per side/lane and zero failures. Medians below are the medians of five measured command times; brackets give their full range. This measures local actor acceptance, not GUI paint, HTTP or G20.

| After accepted commands | Clips | A median ms [range] | B median ms [range] | Ruling |
|---|---:|---:|---:|---|
| 18 | 1 | 0.258 [0.160, 0.266] | 0.085 [0.078, 0.156] | PASS, no regression |
| 4,096 | 1 | 21.136 [20.692, 23.001] | 1.643 [1.585, 1.718] | PASS, <=2 ms |
| 65,536 | 1 | 351.200 [348.850, 381.834] | 24.783 [24.579, 25.644] | 16.7 ms target MISSED; report-only |
| 18 | 500 | 2.984 [2.944, 4.702] | 0.969 [0.617, 0.998] | PASS, no regression |

Ordinary Save uses the unchanged debug six-populated-branch `ja_s2_save_cost` target. It performs twelve real comment-changing Saves per run and independently reopens the result. Five alternating pairs give sixty Saves per side. The small difference is within observed run variation; no Save speedup is claimed.

| Save side | Mean ms/save | Median run mean ms [range] | History decodes | Sync share |
|---|---:|---:|---:|---:|
| A | 238.907 | 238.813 [233.934, 247.977] | 0 | 50.4% |
| B | 235.358 | 235.616 [228.448, 239.756] | 0 | 50.5% |

All binaries were built before the official timing window and sealed by SHA-256. No own build or test ran alongside it. All fifty lanes retain exit/work checks, command/binary/fixture/source identities, wait4 resources, start/end uptime/top-five CPU/GPU clocks, and the five-second contamination sampler. External load annotations are retained; no lane or outlier is discarded. `round3-timing-summary.json`, `round3-timing-provenance.json` and `timings-B/config.json` contain every sample and pair delta.

### Remaining cap cost

The excluded five-run instrumented copy changes only two copied Core files, verified in `round3-breakdown-instrumentation.json`. It accepts and verifies the same two commands per run. These phase observations do not replace the uninstrumented budget numbers. Each row is a median and range; medians need not sum exactly.

| Actor phase | Median ms [range] |
|---|---:|
| Serialize document and changed open/cursor tails | 10.317 [10.138, 10.791] |
| Stream v1 history and cursor SHA-256 | 14.895 [14.804, 15.488] |
| Document clone and changed cache-tail memory copies | 0.793 [0.775, 0.874] |
| Admission, ordinary execution, control and timer remainder | 0.063 [0.059, 0.088] |
| Instrumented actor total | 26.081 [25.863, 26.814] |

Hashing and changed-tail serialization dominate the observed remaining time. V1 places the changing document before the historical arrays, so every new after identity still streams the cached historical bytes. The changed open gesture is still reserialized. The separate v2 structural-identity amendment remains deferred by the lead. No constant-time or capped-history frame-rate claim is made.

## Producer re-scan

App source is unchanged. The fresh raw scan `rg -n -U -C 1 '\bcore(?:\(\))?\s*\.\s*send\s*\(' crates/kinewright-app/src --glob '*.rs'` matches A after sorting raw output blocks; see `round3-producer-rescan.txt`.

| Remaining raw call | Disposition |
|---|---|
| `project.rs:709` | Local fallback only without a JA-1 attachment; otherwise dispatches through `Ja1Attachment::send_manual`. |
| `ja1_attachment.rs:431` | Local/no-manual fallback; an active host permit uses `gate.send`. Frozen/Sealed/lost-retention actors refuse mutations. |
| `app.rs:8171`, `:8441` | Test-only incident/proposal setup. |
| `app.rs:8826` | Test-only deliberate rejected operation. |
| `app.rs:9023` | Test-only Undo control. |
| `app.rs:17310` | Test-only raw bootstrap refusal and actual typed guidance. |
| `ja1_attachment_tests.rs:1791` | Test-only deliberate frozen-producer refusal. |

## Windows and unrun lanes

Core and hash all-target MSVC cross-checks pass, and hash MSVC Clippy passes with `-D warnings`. Both sides of added cfgs were inspected: x86_64 dispatch/kernel, unconditional software fallback, architecture-specific test/imports and test-only force/count fields. Forced software actually runs on this SHA-capable Linux CPU; the dispatch witness requires real hardware blocks. Non-x86 compilation was not run. No OS cfg is added in `app.rs`.

Added portable test runtime measured locally is about 1.6 s for the four Core witnesses and 0.3 s for six hash witnesses. Estimated native Windows fast-tier addition is 3-10 s runtime plus 30-90 s compilation, within the approximately two-minute allowance. This is an estimate, not Windows timing. CI timeout and slow-test manifest are unchanged.

Native Windows full workspace/runtime, hosted CI, physical power-loss/device, native provider/G6, G1-W/G7-D/G20, interactive display acceptance and Kani remain NOT RUN. Independent final-head reviews belong to the lead and were not started, following the inherited C12 instruction. ARMv8 acceleration is optional and not implemented. V2 is explicitly deferred. The cap target remains missed; the required breakdown is provided above.

## Development exclusions and source binding

Development compilation caught a private constant import and a floor-helper Clone derive; Clippy caught let-else and hash documentation formatting. These were corrected before the final gate. The first broad core selector ran tests successfully but its inherited verifier rejected a libtest should-panic display suffix. The new affected selector now names the four added history witnesses; the full-core target still runs every test. A supplemental floor mutant initially failed at a stale digest earlier than the expected range diagnostic; its validator was corrected and the pair rerun.

Source audit then found unbounded temporary JSON serialization before final size refusal. The private bounded writer and its mutation were added. The active agent target was allowed to finish normally before that source change; the prior partial gate and five pairs are excluded under development names. All six pairs and the entire final gate were rerun at the corrected source. No product source changed afterward.

The preflight single samples and phase observer are excluded diagnostics. The official fifty uninstrumented lanes alone supply the budget table. After the frozen gate, only this report and the README row are completed. The final audit rejects any other file change, checks fixture/source/binary identity and binds the resulting one child of A. No push is performed.
