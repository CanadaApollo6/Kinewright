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

## Hooks fix 1: grants follow the acknowledged attempt

Base: `4bd37517bfa2c7043f38592a1ef7bd200f3a7d18`. `RuntimeGrantBinding` carries the exact run, task, attempt and owner epoch. The existing endpoint sync is now the single `sync_runtime_grants` path at all three acknowledged publication/replay cuts. It revokes task attachments, retained MCP endpoints, observers and queued consent when their durable authority dies. Host teardown also revokes retained endpoints and attention grants. Grant deadlines remain checked at consumption.

Queued consent carries a private request revocation flag and deadline as well as its holder flag. The sole session pump checks these immediately before delivery, after matching the current provider callback. Publication also checks the durable turn, so a provider that still advertises the old callback cannot receive consent after an acknowledged turn change. Ordinary questions retain their existing route. Existing MCP admission and operation checks consume the synchronized endpoint revocation flag before prepared-plan lookup or replay.

Model the Domain shaped the shared typed binding and eligibility predicate. Boundary Discipline placed the final checks at callback delivery and MCP consumption.

The new barrier witnesses pass for approval after Stop, secret after Stop, expiry at delivery and a changed durable turn. The real authenticated MCP witness first applies an edit, prepares another, retires the failed attempt while the task stays Preparing and the run Running/Pending, then checks the existing server's prepared edit, its replay, the earlier committed-plan replay and a new prepare. All are refused with identical Core history and revision.

The [consumption mutations](hooks-fix1-evidence/mutations) contain **two accepted runtime red/restored-green pairs across five witnesses**. Bypassing consent delivery authority gives `0 passed; 4 failed`, then `4 passed; 0 failed` after restoration. Bypassing MCP revocation applies the prepared edit at revision 2 and fails `HF1_RETIRED_MCP_PREPARED`, then passes after restoration. Both red exits are 101; both restored-green exits are 0. Setup/compile failures from fixture development are excluded.

Astra Minor 4 is fixed by refusing generic `HostCommand::Retry` while a hook publication is pending. Its witness covers direct and HTTP ingress for the original Editor and a replacement attachment; the original-envelope reconciliation witness still passes. Astra Minor 3, private issuer reply loss, and all Muse minors remain assigned to C6. No Studio changes are needed for this fix; the previously queued C6 integration remains.

Workspace build, Clippy with warnings denied, formatting, manifest lint and all 42 lint self-tests passed on the [frozen source](hooks-fix1-evidence/source-before-gates.json). All requested default-feature lib/integration targets completed, with Project split into four bounded groups. Every Cargo execution used the shared lane and a 600000 ms timeout. No final gate required an execution-timeout retry.

| Crate | Targets | Libtest passed | Failed | Ignored |
| --- | ---: | ---: | ---: | ---: |
| Core | 19 | 810 | 0 | 1 |
| Project | 16 | 411 | 0 | 2 |
| Agent | 21 | 1,056 | 0 | 63 |
| App | 1 | 887 | 10 | 6 |

The three library doc-tests passed. Agent's reported passes include eight explicit equipped/provider skips, which are NOT RUN. [Commands, raw summaries and the complete ignored/skip inventory](hooks-fix1-evidence/gates.json) are retained. App's ten failures [exactly match](hooks-fix1-evidence/app-failures.json) both the approved C2 inventory and the archived parent `4bd37517` run. No other suite failed. Parent comparisons used verified archived logs; a fresh parent rerun was NOT RUN because no additional suite failure appeared.

The Windows Core cross-check passed. The Agent cross-check stopped in FFmpeg/Whisper bindings and AWS-LC native prerequisites, including missing `errno.h`/`stdio.h` and `pthread_rwlock_t` errors. The [same prerequisite errors appear in the archived parent run](hooks-fix1-evidence/windows-prerequisites.json); Windows Agent compilation remains unvalidated. The [final audit](hooks-fix1-evidence/verification-final.json) verifies both mutation pairs, restored producer/witness sources, all 522 frozen inputs, exact App exclusions and unchanged Studio files.

NOT RUN for this fix: native Windows runtime/durability and completed Windows Agent compilation; real provider callbacks/secret delivery; C6 integration, native same-attempt continuation and Studio controls/screenshots; hosted CI and slow tier; the full workspace fast tier; every ignored/equipped case listed in the inventory. Laptop offload was unavailable because Tailscale SSH required an additional authentication check. Worker review rounds were NOT RUN under the binding worker brief. Reviewer: NOT RUN because the role does not exist at this head. Nonempty ClaimIndex runtime: NOT RUN, with reason "no ClaimIndex producer; nonempty refused at bundle.rs:156". The inherited projection and oracle still carry and compare all four refs.
