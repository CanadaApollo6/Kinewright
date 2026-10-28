# S4 hooks H1/H3/H4

Base: `ed23d49aa6e7d2b1dadc3aff6676ce48d37ef74e`. One combined hooks commit; no push and no review rounds.

H1 registers Project's shared `orchestration@1` records through the sealed RunIndex registry. Run cuts, submitted-batch consumption and owner receipts publish atomically through `MetadataUpdate`, without caller-supplied root pointers. Reopen checks cut/predecessor/receipt bindings; uncertain publication permits only the original command and live attachment to reconcile before current authorization and original-result replay.

H3 resolves actors on HTTP, MCP and direct-owner routes before publication and replay. Metadata issuers bind to subjects and have issuance/refresh commands (C1-M4/M5); recovery freezes metadata writes (C1-M2). Viewer start/control/consent and Producer scope escape/expiry/minting are refused. Editor approval/secret responses require the person-facing holder and exact live callback. The existing session pump handles typed responses and revocation; delegates can forward ordinary questions only. Credentials/callbacks stay runtime-only.

H4 retains certified host-owned inputs and an independent host/epoch Producer; task grants keep finite deadlines. Detach checks retained descriptors with the verified cursor before source release, revokes only the departing attachment, and preserves run/attempt, task endpoint, heads, pins, charges and allowances. App's existing poll releases its source after acknowledgment. Shutdown refuses continuable runs. The private issuer reissues the original Editor under the one-live-Editor rule and retained manual-client identity.

Only these two lines were added to Project `lib.rs`:

```rust
pub mod orchestration_commit;
pub mod orchestration_records;
```

Studio UI files are unchanged. C6 must consume issuer/run/attention replies in the existing App poll, wire Pause/Stop/Continue/SendMessage and holder attention controls, retain the private continuation issuer outside the closing project view, and use it to rejoin. C6 supplies the real native same-attempt continuation witness.

## Verification

Workspace build, Clippy with warnings denied, formatting, slow-test manifest lint and all 42 lint self-tests passed. Full fast-tier crate targets and library doc-tests completed; [commands, raw summaries and skip inventory](hooks-evidence/gates.json) are bound to the frozen source manifest.

| Crate | Targets | Libtest passed | Failed | Ignored |
| --- | ---: | ---: | ---: | ---: |
| Core | 19 | 810 | 0 | 1 |
| Project | 16 | 411 | 0 | 2 |
| Agent | 21 | 1,050 | 0 | 63 |
| App | 1 | 887 | 10 | 6 |

The three library doc-tests passed. Agent's reported passes include eight equipped-session skips; those are NOT RUN. Both architecture scans, all 28 new host witnesses, the shipping-host owned-audio Close/detach/reattach witness, and the corrected legacy Close fixture passed.

All ten App failures reproduce at `ed23d49a`: `test result: FAILED. 42 passed; 10 failed; 0 ignored; 0 measured; 850 filtered out; finished in 37.16s`. [Parent identities and exact failures](hooks-evidence/prerequisites/parent-app.json) are retained. Studio files remain unchanged. Test-only corrections adapt the certificate fixture to C2 full verification/early corruption refusal, settle the App import lease before handover, and keep its legacy proxy alive through acknowledged Close.

The parent check left one stale Cargo feature artifact. [Cache reconciliation](hooks-evidence/prerequisites/cache-reconciliation.json) verifies all 35 completed Core/Project target fingerprints used current Core; the stale variant was rebuilt before Agent/App/doc-tests. Parent checks now use an isolated target directory.

The Windows Core cross-check passed. The desktop cross-check was attempted and failed in FFmpeg/Whisper bindings and native dependencies (`errno.h`/`stdio.h` unavailable), before desktop compilation could be validated.

Mutation campaign: **36/36 accepted runtime red/restored-green pairs**. The [final audit](hooks-evidence/verification-final.json) verifies log hashes, expected failure sites and source restoration. Only [assertion metadata](hooks-evidence/verification-inputs.json) changed after gates; all Cargo producer and witness sources remain identical to the gated manifest. The initial registry expectation mismatch is archived and excluded; no setup failure receives mutation credit.

NOT RUN: native Windows runtime/durability, real provider callbacks/secret delivery, C6 native same-attempt continuation, Studio UI controls/screenshots, hosted CI and slow tier. Reviewer: NOT RUN because the attachment role does not exist at this head. Nonempty ClaimIndex runtime: NOT RUN — no ClaimIndex producer; nonempty refused at bundle.rs:156. All four index refs remain in the inherited root-producer projection and oracle.

The inherited `s4_usage_durable_reopen_h1` and `s4_editor_result_to_candidate` slots remain ignored. Every actual ignored/equipped case is listed in the gate inventory. Full workspace fast-tier execution and laptop offload are NOT RUN; SSH requires additional Tailscale authentication.
