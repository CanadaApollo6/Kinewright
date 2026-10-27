# S2-H re-review fixes (commit C)

Parent `5f683a614da13998a1ac0b3b4f15473bf166df22`, branch `ja1/s2`, worktree
`/home/riels/.t3/worktrees/Kinewright/t3-cf687318`. Exactly one child commit is added; no push.
The final commit binder under the evidence root ties the resulting SHA to the tested source.

Evidence root `E = /home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3`.
The command audit contains 37 rows and 13 distinct repaired protocol cuts, D01-D13. The
mutation-catalog defect is recorded separately. Shared repairs are counted once per distinct
failure cut, rather than once per command that uses the same response mechanism.

## Finding, fix, witness and mutation

U means `ja1_attachment::tests::` in the App binary. H means
`writer_host::s2h_tests::` in the agent library. The table uses exact test leaf names.

| Defect | Owning fix | Witness | Mutation |
|---|---|---|---|
| D01, Astra N1: initial recovery-plan reply lost | `writer_host/recovery.rs::begin_recovery` fingerprints the original private client, frozen cut, pending chain and discard request. A matching retry retrieves the recorded plan; changed requests refuse. | U `s2h_begin_recovery_reply_loss_retries_original_plan`; H `s2h_recovery_plan_retries_bind_original_client_cut_and_chain` | `lost-begin-refuses-matching-original`; `begin-retry-accepts-another-client-or-cut` |
| D02, Astra N1: floor-confirm reply lost | Host records the floor-to-close handoff. GUI `RecoveryPlan` preserves Publish versus Confirm progress and adopts the returned close plan. | U `s2h_floor_confirm_reply_loss_adopts_recorded_close_plan` | `lost-floor-confirm-discards-close-handoff`; `gui-republishes-floor-after-lost-confirm` |
| D03, Astra N2: completed switch still pending | `complete_switch_handoff` retires the envelope only after the confirmed view/association handoff. Retry selects from explicit pending/completed switch state. | U `s2h_completed_switch_then_lost_save_uses_c10_on_same_actor`, acknowledged edit followed by lost Save, same actor, exact C10 result and working producer gate. | `completed-switch-keeps-original-envelope` |
| D04, Astra N3: Status thaws uncertainty | `unknown_operation` survives every heartbeat, independently of permit presence. Only the original validated result or C10 clears it. Unknown text remains visible. | U `s2h_status_heartbeat_never_thaws_unknown_save_with_or_without_permit`; U `s2h_lost_save_floor_stays_frozen_until_exact_publisher_result`, including a real 12,000-command Save floor and independent reopen. | `status-thaws-unknown-operation`; `unknown-operation-ignores-bootstrap-without-permit` |
| D05, Muse N1: post-grant identity read escapes rollback | `confirm_switch` captures current identity inside the lease/input acquisition transaction. Its error arm records continuation before refreeze. | H `s2h_post_grant_identity_failure_revokes_and_retries_one_lease`, scoped existing IO-failure machinery at the identity read, revoked partial grant, Frozen shadow, successful retry and unchanged generation on duplicate confirmation. | `post-grant-identity-escapes-revoke-transaction` |
| D06, audit: final close-confirm reply lost | A matching final confirmation returns only its same still-current grant after exact history/switch-generation and unresolved-state checks. | U `s2h_close_confirm_reply_loss_reuses_delivered_authority` | `lost-close-confirm-discards-current-grant` |
| D07, audit: foreground result consumed without a surviving GUI permit or plan | `HostRequest.operation` and the one volatile host response slot bind client, operation, command and forwarded roots. Retry retrieves the exact original result rather than executing it again. GUI retains the original request. | U `s2h_lost_save_as_and_preview_replies_replay_original_results`; U `s2h_lost_destination_admission_and_manual_grant_replies_retry_in_context`; H `s2h_foreground_result_replay_is_exact_bounded_and_client_bound`; H `s2h_lost_rescue_result_returns_original_activation_without_second_copy` | `replayed-foreground-result-executes-again`; `save-as-lost-result-is-not-retained`; `first-save-admission-lost-result-is-not-retained`; `rescue-lost-result-executes-without-choice-again` |
| D08, audit: destination admission reply lost before publication | Retain the enclosing original foreground request before admission. The same path reuses its existing native pinned capability, including after the target becomes the active committed project. | U `s2h_lost_destination_admission_and_manual_grant_replies_retry_in_context`; U `s2h_lost_save_as_and_preview_replies_replay_original_results` | `first-save-admission-lost-result-is-not-retained`; `destination-admission-reinspects-an-already-committed-target` |
| D09, audit: initial grant after First Save lost | Retry retrieves that original grant before any current-attachment query. The existing owner endpoint validates and atomically resumes the frozen actor with that grant. Inputs are read before issuing an initial lease. | U `s2h_lost_destination_admission_and_manual_grant_replies_retry_in_context` | `lost-manual-grant-is-replaced-by-current-query` |
| D10, audit: initial Attach loss kills the live owner | A discovered live child and the actual frozen GUI owner survive an unconfirmed Attach response. No retention proof is invented; Retry repeats that read. Confirmed retention failure still enters L2. | U `s2h_lost_initial_attach_reply_keeps_owned_host_and_retries`, clean and dirty bootstrap, same child/actor/history and explicit Save for dirty bootstrap. | `initial-attach-reply-loss-kills-live-owner` |
| D11, audit: launch has no outer action to fence an initial lease error | The central manual-start path records its failed request as unknown, including during launch. | U `s2h_lost_initial_manual_grant_keeps_original_request_for_retry` | `initial-manual-acquisition-error-is-not-fenced` |
| D12, audit: read-only heartbeat failure creates an operation with no result to reconcile | Failed heartbeat reads are retried without creating a publication freeze. Owned-child death is still checked on the frame thread. | U `s2h_status_heartbeat_never_thaws_unknown_save_with_or_without_permit`, separate lost-Status and lost-Save phases. | `read-only-heartbeat-error-freezes-an-operation` |
| D13, audit: an identity refusal resolves an unknown operation | `HostClient::request_operation` rejects an unvalidated client/operation/payload identity response as an error. The original GUI request and freeze remain intact. | U `s2h_foreground_identity_refusal_cannot_thaw_unknown_operation`, real HTTP refusal after a changed forwarded scope, then successful exact original retry. | `identity-refusal-is-treated-as-an-original-result` |
| Astra N4 / Muse N2: orphaned anchors | Retarget the eight C10 guards to `actor/manual_history.rs`. Audit every JSON case catalog, repair moved C8/C9/C11/A sites and explicitly supersede obsolete C9/C11 controls with current named equivalents. | All-catalog anchor audit plus the eight focused C10 pairs. The moved C12 cold-open-view pair and two C8 destination controls also rerun. | Exact per-case results below. |

The proxy really drops HTTP responses after the host processes them. It also supports dropping
before processing for the retained confirmation witness. Initial Attach/Acquire faults install
that same scoped proxy before the real launch request. No transport mock supplies product
semantics. The new rescue-result witness executes the real serialized command owner and
existing Unknown-publication IO hook, then independently reopens B. It is an owner result-loss
witness, not a new TCP/App rescue-activation claim; the real HTTP original-result route is
separately exercised by Save As and First Save.

## Host command audit

The table covers each command sent by the production GUI attachment, its HostClient destination
helper and its chat adapter. `PrepareSwitch`, `SwitchReceipt`, scripted/probe commands and
`ReadHistory` have no production GUI send site; host-internal dispatches and test calls are not
invented GUI rows. `E/gui-command-sites.txt` and `E/host-command-audit.json` retain the inventory.

F is the single outstanding foreground operation on the same live host. It retains one bounded
reply and exact request identity. A later GUI operation replaces the slot; heartbeats, manual
messages, recovery and chat cannot evict it. A replay proves current source retention and binds
the same private client, operation, payload and forwarded roots. It is volatile protocol state,
not a second store, actor, publisher or durable ledger. Confirmed host loss retains L2.

C10 reconciles the original manual chain, installs the exact durable floor/close result on both
Cores and grants fresh producer authority. S confirms the same installed switch and completes
the GUI handoff. R retains the recovery publication/confirmation phase and exact recorded plan.
An identity-conflict refusal is not a validated F result and cannot clear the unknown operation. Only F, C10, S or R can complete the corresponding unknown operation. Read/status messages do
not authorize a thaw. A live host that is truly wedged still has explicit context Close as its
pre-existing C10 exit; this change adds no retry timeout or editable paused state.

| GUI HostCommand | Host state change | Repeated request after reply loss | Message allowed to thaw its freeze | Witness or reason |
|---|---|---|---|---|
| `Attach` | Read current attachment state; initial launch has already transferred write authority. | Idempotent read. Initial loss retains the owned child and original Attach for Retry. | F: validated original foreground result; Status cannot complete it. | U s2h_lost_initial_attach_reply_keeps_owned_host_and_retries D10. |
| `Status` | Read current owner state; no manual publication. | Another heartbeat reads state again. A failed heartbeat creates no unknown publication. An existing unknown operation remains frozen. | None. Status never resolves an unknown operation. | U s2h_status_heartbeat_never_thaws_unknown_save_with_or_without_permit; U s2h_lost_save_floor_stays_frozen_until_exact_publisher_result D04, D12. |
| `CurrentAttachment` | Read the current committed switch attachment. | F retains the original read; its result cannot evict an undelivered AcquireManual. | S: exact switch confirmation, then completed GUI handoff. | U s2h_lost_initial_manual_grant_keeps_original_request_for_retry; existing g05a_first_edit_undo_and_exact_switch_back |
| `AdmitDestination` | Retain the native pin for the explicit person-picked destination. | The same path reuses its existing pinned capability before capacity or source/destination reinspection. The original enclosing First Save/Save As/rescue request remains retained. | F: validated original foreground result; Status cannot complete it. | U s2h_lost_destination_admission_and_manual_grant_replies_retry_in_context; U s2h_lost_save_as_and_preview_replies_replay_original_results D08. |
| `AcquireManual` | Issue one private manual lease and input context. Inputs are read before issuing the lease. | F returns the same original grant. Retry does not replace the request with CurrentAttachment. Frozen acquisition finishes through the existing owner endpoint. | F: validated original foreground result; Status cannot complete it. | U s2h_lost_destination_admission_and_manual_grant_replies_retry_in_context; U s2h_lost_initial_manual_grant_keeps_original_request_for_retry D09, D11. |
| `CommitHistory` | Validate and publish original accepted manual envelopes and receipts. | Existing receipt reconciliation distinguishes committed and absent originals; no acknowledged command is reapplied. | C10: completed exact recovery and fresh producer authority. | Existing s2h_manual_and_save_reply_loss_reconcile_originals_and_kill_is_l2; U s2h_begin_recovery_reply_loss_retries_original_plan |
| `BeginRecovery` | Fence the old lease, reconcile the frozen original chain and record one barrier. | Same client, cut, chain and discard request returns the existing plan; a changed request refuses. | R: recorded barrier result, equivalence and final confirmation. | U s2h_begin_recovery_reply_loss_retries_original_plan; H s2h_recovery_plan_retries_bind_original_client_cut_and_chain D01. |
| `PublishBarrier` | Publish or recover the original floor/recovery-close receipt and install its exact shadow history. | Existing receipt lookup avoids another publication. GUI retains Publish versus Confirm progress. | R: recorded barrier result, equivalence and final confirmation. | Existing s2h_manual_and_save_reply_loss_reconcile_originals_and_kill_is_l2; U s2h_floor_confirm_reply_loss_adopts_recorded_close_plan |
| `ConfirmBarrier` | Hand a confirmed floor to its recovery-close plan, or grant authority after exact close installation. | A repeated floor confirmation returns its recorded continuation. A repeated final confirmation returns only the same still-current grant and exact installed history. | R: recorded barrier result, equivalence and final confirmation. | U s2h_floor_confirm_reply_loss_adopts_recorded_close_plan; U s2h_close_confirm_reply_loss_reuses_delivered_authority D02, D06. |
| `ConfirmSwitch` | Resume the confirmed shadow and acquire its lease, inputs and current identity atomically. | Existing same-client/current-pointer grant lookup returns one lease. Acquisition failure revokes the undelivered grant and records continuation before refreeze. | S: exact switch confirmation, then completed GUI handoff. | H s2h_post_grant_identity_failure_revokes_and_retries_one_lease; existing s2h_confirm_reply_loss_retries_installed_b_without_reinstallation D05. |
| `Switch` | Publish the prepared original switch receipt and select its committed attachment. | Original receipt lookup plus S. Explicit pending/completed state chooses Retry; successful GUI handoff retires the envelope. | S: exact switch confirmation, then completed GUI handoff. | U s2h_completed_switch_then_lost_save_uses_c10_on_same_actor; existing g05a_first_edit_undo_and_exact_switch_back D03. |
| `Save` | Publish the frozen history, possibly with an ordinary floor. | With a manual permit, Retry runs C10. Bootstrap without a permit retrieves F and installs the publisher history and digest. | C10: completed exact recovery and fresh producer authority. Bootstrap uses F. | U s2h_status_heartbeat_never_thaws_unknown_save_with_or_without_permit; U s2h_lost_save_floor_stays_frozen_until_exact_publisher_result D04. |
| `SaveAs` | Commit the copy and change the host association; GUI relinquishes its old permit. | F returns the original saved history/path without another copy. | F: validated original foreground result; Status cannot complete it. | U s2h_lost_save_as_and_preview_replies_replay_original_results D07. |
| `FirstSave` | Commit the first named copy from the actual retained GUI source. | F returns the original committed copy instead of refusing because a named owner now exists. | F: validated original foreground result; Status cannot complete it. | U s2h_lost_destination_admission_and_manual_grant_replies_retry_in_context D07. |
| `Retry` | Reconcile an existing publication, or open an existing prepared host selection. | Pending GUI operation, pending switch and C10 states choose their original protocol. The plain host Retry itself uses F. | F: validated original foreground result; Status cannot complete it. Pending manual/switch states use C10/S. | U s2h_completed_switch_then_lost_save_uses_c10_on_same_actor; initial/foreground/recovery proxy witnesses |
| `PreviewRescue` | Freeze/select a bounded rescue capture and explicit consents. | F retrieves that original choice, without taking another capture or reservation. | F: validated original foreground result; Status cannot complete it. A live choice keeps editing read-only. | Shared F mechanism exercised by U s2h_lost_save_as_and_preview_replies_replay_original_results and H s2h_foreground_result_replay_is_exact_bounded_and_client_bound; existing full rescue targets cover choice semantics. |
| `ConsentTrim` | Mark explicit trim consent on the current rescue choice. | F returns the consent result; it never implies omission consent. | F: validated original foreground result; Status cannot complete it. A live choice stays read-only. | Shared F witness; existing w03_choices_never_supply_implicit_consent. No independent publication or new recovery protocol. |
| `ConsentOmission` | Mark explicit omission consent on the current rescue choice. | F returns the consent result; it never implies trim consent. | F: validated original foreground result; Status cannot complete it. A live choice stays read-only. | Shared F witness; existing w03_choices_never_supply_implicit_consent. No independent publication or new recovery protocol. |
| `RescueTo` | Consume the selected capture, commit B and possibly activate its fresh actor. | F returns the original activation/capture result even though the choice was consumed. It cannot create a second B. | F: validated original foreground result; Status cannot complete it. Non-activated or unresolved results remain frozen as before. | H s2h_lost_rescue_result_returns_original_activation_without_second_copy; actual HTTP F path is covered by the Save As/First Save witnesses; existing rescue/App lifecycle targets cover activation. This is not a new TCP rescue probe. D07. |
| `Cancel` | Cancel the current choice; unresolved publication remains frozen. | F returns the original cancellation instead of depending on a still-existing choice. | F: validated original foreground result; Status cannot complete it. Only a resolved original result can resume its cut. | Shared F witness and existing full App rescue/choice target; no new publication identity. U s2h_foreground_identity_refusal_cannot_thaw_unknown_operation. An identity refusal is not an original result. D13. |
| `PreviewRecovery` | Install/check the cut, prepare the selected target and original switch envelope. | F retrieves the original envelope and preview after a lost preparation reply. | F: validated original foreground result; Status cannot complete it. Prepared selection stays frozen until S or explicit Abandon. | U s2h_lost_save_as_and_preview_replies_replay_original_results D07. |
| `OpenRecovery` | Publish the existing prepared switch, when no GUI-held envelope supplies Switch directly. | F retains the result; normal Switch receipt/confirmation rules still apply. | S: exact switch confirmation, then completed GUI handoff. | Existing g05a_first_edit_undo_and_exact_switch_back and confirm-loss witness; same switch owner, no separate executor. |
| `Abandon` | Release the prepared selection and return its current attachment for confirmation when necessary. | F retrieves the original attachment even after the prepared selection is gone. | S: exact switch confirmation, then completed GUI handoff. Non-switch cancellation uses F. | Shared F and existing W13 abandoned-selection/availability witnesses; no new mutation-specific publisher. |
| `PreviewRetirement` | Record branch and expected root for explicit removal consent. | F retrieves that same expected-root choice. | F: validated original foreground result; Status cannot complete it. Choice remains read-only. | Shared F and existing registered-session/base-retirement witnesses; only preview state changes. |
| `RemoveBranch` | Consume explicit removal choice and retire the selected ordinary branch. | F returns the original root publication; a repeated request cannot remove another branch. | F: validated original foreground result; Status cannot complete it. | H s2h_foreground_result_replay_is_exact_bounded_and_client_bound covers exact one-result replay; existing full retirement targets cover root/registration refusal. |
| `PreviewForget` | Record the exact acknowledged recovery point and expected root. | F retrieves that same point/consent choice. | F: validated original foreground result; Status cannot complete it. Choice remains read-only. | Shared F plus existing rescue recovery-point tests; only preview state changes. |
| `Forget` | Consume consent and retire the exact selected recovery edge. | F returns the original root publication, without requiring the consumed choice. | F: validated original foreground result; Status cannot complete it. | Shared F exact-result witness plus existing recovery-point retirement tests. |
| `Reconcile` | Reconcile one retained destination; a late committed copy never activates editing. | F returns the original settlement; repeating the underlying reconciliation also converges. | F: validated original foreground result; Status cannot complete it. No activation or manual reauthorization from late destination proof. | Existing late-A/capture-only rescue witnesses and shared F result witness. |
| `PreviewRelease` | Record an explicit completed-capture release choice. | F retrieves that same destination choice. | F: validated original foreground result; Status cannot complete it. Choice remains read-only. | Existing explicit-release App witness plus shared F mechanism; no publication. |
| `Release` | Release a completed destination owner; originating source retention is unchanged. | F returns the original result after the choice/owner is consumed. | F: validated original foreground result; Status cannot complete it. | Existing explicit-release/actual-source ownership witnesses plus shared F mechanism. |
| `SaveSession` | Persist the registered isolated branch session. | F returns the original result without another session publication. | F: validated original foreground result; Status cannot complete it. This does not authorize a manual history change. | Existing registered live-session/save-or-close witnesses and shared F mechanism. |
| `CloseSession` | Stop matching isolated sessions, persist their branch and close its registration. | F returns the original result after registration/choice consumption. | F: validated original foreground result; Status cannot complete it. This does not authorize a manual history change. | Existing registered-session retirement/close witnesses and shared F mechanism. |
| `Shutdown` | Explicit originating Close ends the owned host. | Close still ends the context if the response is lost; child wait/kill and actual GUI source cleanup remain owned and idempotent. | None. The editing context is closing. | Existing L2/explicit-Close/host-Drop ownership witnesses; no recoverable editor freeze is created. |
| `StartSession` | Create an isolated registered chat/agent branch session. | No automatic resend. Its registered branch remains visible in Studio and can be saved/closed there. A new chat request is a separate person action. | None. Chat never resolves a manual unknown operation. | Existing host session and registration targets; isolated branch session control creates no manual freeze. |
| `SendMessage` | Enqueue input to the identified isolated agent session. | No automatic resend of an uncertain message. Session status and explicit stop/save/close remain available. | None. No manual authority is granted. | Existing host/chat/session tests. No editor freeze or publication result installation uses this reply. |
| `InterruptSession` | Request stop for the identified isolated session. | Repeated stop converges; status/save/close remain available. | None. No manual authority is granted. | Existing host/chat stop tests; no manual freeze is created. |
| `SessionStatus` | Read isolated session progress. | Repeat the read; missing chat status does not change manual authority. | None. It never resolves a manual unknown operation. | Existing chat polling/session tests; pure session read, no manual result installation. |

## Scope, identities and platform limits

The Core/manual-history acceptance and encoding path is untouched. Neither
`crates/kinewright-core/src/actor/manual_history.rs` nor
`crates/kinewright-project/src/manual_history.rs` changes in C. The 4,096-command and small-history
paired drag lanes are therefore NOT RUN under the brief's explicit conditional. Temporary C10
mutation edits are restored and do not become a product change. Commit B's v1 identity values,
cache/byte limits and budgets remain unchanged; no v2 identity is added.

Original S0 fixture/index bytes stay unchanged. These are additive witnesses and runtime
mutation controls, with no new S0 flip or native/product acceptance claim. The actual GUI
source retains its ownership, pins and sole accounting ledger. No P copy, retained-manual
descriptor, read-only working role, editable pause, Continue, special headroom or exact-retention
bypass returns.

All added durable witnesses are Linux-only. The modified transport helper module and its launch
hook have matching Linux test cfgs; every production field/helper is used on both platforms.
The portable project identity-read fault point uses the same test/test-util cfg as its observer module. No OS cfg is added in `app.rs`. Estimated
Windows addition is 0 s of test runtime and 30-90 s of portable compilation, within the roughly
two-minute allowance. This is a source-based estimate, not a native measurement. CI timeout,
slow-test manifest and limits are unchanged.

Native Windows build/lint/runtime, hosted CI/slow, non-x86 runtime, physical device/power-loss,
native providers/G6, G1-W/G7-D/G20, interactive display acceptance and Kani remain NOT RUN.
The S4 bearer/editor credential, v2 structural identity, OwnedMedia fingerprint binding and
default eval-harness feature remain the lead's explicit deferrals. No review round is started;
the inherited C12 instruction assigns independent final-head review to the lead.

## Reproduction and source binding

The gate uses the inherited C12 selections unchanged, plus every new U/H witness through their
existing selected modules and one required all-catalog audit row. It runs touched-file rustfmt,
five shipping/eval builds, workspace Clippy with `-D warnings`, the complete affected set,
complete touched-crate test targets, all affected exact slow selectors and the complete
workspace fast tier with `--no-fail-fast`, followed by its strict verifier.

Run from the named worktree. Exact expanded commands, selected test names, raw streams, binary
hashes and counts are retained beside each command row under `E/gate-C`. The runner sources
FFmpeg in the Cargo shell, sets `CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4`, uses native nice 10,
checks Cargo idleness and 6 GiB available memory, and serializes heavy commands. Media runs
with one test thread. Python control/verification runs at nice 19 with `-I`. No build/test
runs alongside paired timing. Logs and binaries are not committed.

```bash
bash docs/reports/ja1-s2/s2h-fix3-gate.sh /home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C 5f683a6
```

## Development exclusions

The first shipping gate found a missing observer cfg; its partial frame and prior mutation records are excluded. All pairs and the complete gate rerun after the portable hook receives the observer module's exact cfg. Development compilation caught a moved client, cfg-only mutability, method-closure and
formatting/lint issues; they were fixed before the final gate. The first rescue App setup
created a definite refusal, not the required Unknown publication; that setup is excluded and
replaced by the actual owner/IO-hook rescue witness described above. Initial-acquisition
inspection stopped a development mutation frame, restored its source and added the missing
launch witness. No interrupted frame receives final runtime credit. One final-frame mutant's
expected diagnostic named a later assertion, while the real failure occurred at Retry; that
rejected record is excluded and the corrected pair is rerun. Per-case source, diagnostics and
all restored-green outcomes are audited before acceptance. A real HTTP identity-conflict
witness then exposed premature resolution on a non-matching refusal; its pre-fix red is
retained, the client boundary rejects that response, and every final pair/gate is rerun.

The final report and README row are completed after the gate. No tested product or witness
source is changed afterward. `E/commit-C-binding.json` binds the exact one-child commit, source,
fixtures, mutation originals and timed binaries. The tracked contract correction is the
S2-H review-corrections section in `docs/JA1-STUDIO-ORCHESTRATION.md`.

## Recorded gate, mutation and timing results

All **100 gate command rows exit zero**. The affected set has **880 expected / 880 selected / 856 passing executions / 24 existing ignored outcomes**, zero failures. Selections overlap deliberately. Complete touched-crate targets pass, followed by workspace fast **4091 passed / 0 failed / 111 existing ignores**. Failure list is `[]`. Ignored outcomes are NOT RUN, not passes. Exact affected slow selectors execute with their features. Gate before/after source manifests are byte-identical.

| Executed command record | Result |
|---|---|
| [rustfmt](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/rustfmt.command) | Exit 0. |
| [workspace-build](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/workspace-build.command) | Exit 0. |
| [agent-shipping-build](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/agent-shipping-build.command) | Exit 0. |
| [app-shipping-build](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/app-shipping-build.command) | Exit 0. |
| [host-shipping-build](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/host-shipping-build.command) | Exit 0. |
| [eval-build](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/eval-build.command) | Exit 0. |
| [clippy](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/clippy.command) | Exit 0. |
| [arch-agent-eval-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/arch-agent-eval-list.command) | Exit 0. |
| [arch-agent-eval](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/arch-agent-eval.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 702 filtered out; finished in 0.01s` |
| [arch-agent-production-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/arch-agent-production-list.command) | Exit 0. |
| [arch-agent-production](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/arch-agent-production.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 598 filtered out; finished in 0.02s` |
| [arch-app-production-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/arch-app-production-list.command) | Exit 0. |
| [arch-app-production](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/arch-app-production.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 0.02s` |
| [arch-app-default-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/arch-app-default-list.command) | Exit 0. |
| [arch-app-default](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/arch-app-default.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 0.02s` |
| [core-actor-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/core-actor-list.command) | Exit 0. |
| [core-actor](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/core-actor.command) | 24 expected/selected; 23 passed; 1 ignored. `test result: ok. 23 passed; 0 failed; 1 ignored; 0 measured; 301 filtered out; finished in 0.35s` |
| [core-attachment-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/core-attachment-list.command) | Exit 0. |
| [core-attachment](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/core-attachment.command) | 8 expected/selected; 8 passed; 0 ignored. `test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 317 filtered out; finished in 0.03s` |
| [core-history-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/core-history-list.command) | Exit 0. |
| [core-history](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/core-history.command) | 13 expected/selected; 13 passed; 0 ignored. `test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| [project-lib-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/project-lib-list.command) | Exit 0. |
| [project-lib](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/project-lib.command) | 140 expected/selected; 138 passed; 2 ignored. `test result: ok. 138 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 93.30s` |
| [ja_s1_storage-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/ja_s1_storage-list.command) | Exit 0. |
| [ja_s1_storage](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/ja_s1_storage.command) | 86 expected/selected; 86 passed; 0 ignored. `test result: ok. 86 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 105.62s` |
| [ja_s1_receipts-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/ja_s1_receipts-list.command) | Exit 0. |
| [ja_s1_receipts](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/ja_s1_receipts.command) | 9 expected/selected; 9 passed; 0 ignored. `test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 53.36s` |
| [ja_s1_copy-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/ja_s1_copy-list.command) | Exit 0. |
| [ja_s1_copy](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/ja_s1_copy.command) | 6 expected/selected; 6 passed; 0 ignored. `test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.59s` |
| [ja_s2_save-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/ja_s2_save-list.command) | Exit 0. |
| [ja_s2_save](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/ja_s2_save.command) | 13 expected/selected; 13 passed; 0 ignored. `test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 61.09s` |
| [ja_s2_rescue-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/ja_s2_rescue-list.command) | Exit 0. |
| [ja_s2_rescue](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/ja_s2_rescue.command) | 35 expected/selected; 35 passed; 0 ignored. `test result: ok. 35 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 23.01s` |
| [ja_s2_rescue_record-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/ja_s2_rescue_record-list.command) | Exit 0. |
| [ja_s2_rescue_record](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/ja_s2_rescue_record.command) | 33 expected/selected; 33 passed; 0 ignored. `test result: ok. 33 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 170.56s` |
| [ja_s2_writer-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/ja_s2_writer-list.command) | Exit 0. |
| [ja_s2_writer](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/ja_s2_writer.command) | 7 expected/selected; 7 passed; 0 ignored. `test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.82s` |
| [ja_s2_switch-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/ja_s2_switch-list.command) | Exit 0. |
| [ja_s2_switch](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/ja_s2_switch.command) | 7 expected/selected; 7 passed; 0 ignored. `test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.28s` |
| [ja_s2_source_boundary-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/ja_s2_source_boundary-list.command) | Exit 0. |
| [ja_s2_source_boundary](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/ja_s2_source_boundary.command) | 16 expected/selected; 16 passed; 0 ignored. `test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.86s` |
| [ja_s2_save_cost-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/ja_s2_save_cost-list.command) | Exit 0. |
| [ja_s2_save_cost](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/ja_s2_save_cost.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 20.27s` |
| [specialist-identity-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/specialist-identity-list.command) | Exit 0. |
| [specialist-identity](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/specialist-identity.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 702 filtered out; finished in 1.01s` |
| [specialist-registration-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/specialist-registration-list.command) | Exit 0. |
| [specialist-registration](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/specialist-registration.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 702 filtered out; finished in 1.44s` |
| [specialist-namespace-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/specialist-namespace-list.command) | Exit 0. |
| [specialist-namespace](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/specialist-namespace.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 702 filtered out; finished in 3.23s` |
| [host-lib-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/host-lib-list.command) | Exit 0. |
| [host-lib](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/host-lib.command) | 49 expected/selected; 49 passed; 0 ignored. `test result: ok. 49 passed; 0 failed; 0 ignored; 0 measured; 550 filtered out; finished in 38.67s` |
| [host-integration-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/host-integration-list.command) | Exit 0. |
| [host-integration](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/host-integration.command) | 20 expected/selected; 16 passed; 4 ignored. `test result: ok. 16 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 16.53s` |
| [host-recovery-pressure-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/host-recovery-pressure-list.command) | Exit 0. |
| [host-recovery-pressure](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/host-recovery-pressure.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 20 filtered out; finished in 3.34s` |
| [host-recovery-absent-matrix-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/host-recovery-absent-matrix-list.command) | Exit 0. |
| [host-recovery-absent-matrix](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/host-recovery-absent-matrix.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 19 filtered out; finished in 15.33s` |
| [host-recovery-floor-matrix-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/host-recovery-floor-matrix-list.command) | Exit 0. |
| [host-recovery-floor-matrix](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/host-recovery-floor-matrix.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 19 filtered out; finished in 14.23s` |
| [host-switch-matrix-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/host-switch-matrix-list.command) | Exit 0. |
| [host-switch-matrix](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/host-switch-matrix.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 19 filtered out; finished in 7.29s` |
| [agent-scripted-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/agent-scripted-list.command) | Exit 0. |
| [agent-scripted](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/agent-scripted.command) | 5 expected/selected; 5 passed; 0 ignored. `test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 698 filtered out; finished in 2.34s` |
| [agent-drivers-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/agent-drivers-list.command) | Exit 0. |
| [agent-drivers](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/agent-drivers.command) | 11 expected/selected; 11 passed; 0 ignored. `test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 692 filtered out; finished in 0.00s` |
| [agent-acp-session-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/agent-acp-session-list.command) | Exit 0. |
| [agent-acp-session](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/agent-acp-session.command) | 9 expected/selected; 9 passed; 0 ignored. `test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 694 filtered out; finished in 5.03s` |
| [harness-lib-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/harness-lib-list.command) | Exit 0. |
| [harness-lib](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/harness-lib.command) | 21 expected/selected; 21 passed; 0 ignored. `test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 682 filtered out; finished in 3.25s` |
| [ja_baseline-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/ja_baseline-list.command) | Exit 0. |
| [ja_baseline](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/ja_baseline.command) | 13 expected/selected; 13 passed; 0 ignored. `test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.19s` |
| [ja_s1-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/ja_s1-list.command) | Exit 0. |
| [ja_s1](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/ja_s1.command) | 20 expected/selected; 20 passed; 0 ignored. `test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.65s` |
| [ja_s1_branches-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/ja_s1_branches-list.command) | Exit 0. |
| [ja_s1_branches](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/ja_s1_branches.command) | 5 expected/selected; 5 passed; 0 ignored. `test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.98s` |
| [app-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/app-list.command) | Exit 0. |
| [app](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/app.command) | 202 expected/selected; 198 passed; 4 ignored. `test result: ok. 198 passed; 0 failed; 4 ignored; 0 measured; 682 filtered out; finished in 174.70s` |
| [local-agent-admission-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/local-agent-admission-list.command) | Exit 0. |
| [local-agent-admission](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/local-agent-admission.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.77s` |
| [agent-rescue-stress-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/agent-rescue-stress-list.command) | Exit 0. |
| [agent-rescue-stress](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/agent-rescue-stress.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.95s` |
| [app-rescue-stress-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/app-rescue-stress-list.command) | Exit 0. |
| [app-rescue-stress](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/app-rescue-stress.command) | 1 expected/selected; 1 passed; 0 ignored. `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 28.09s` |
| [media-pins-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/media-pins-list.command) | Exit 0. |
| [media-pins](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/media-pins.command) | 10 expected/selected; 10 passed; 0 ignored. `test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 1189 filtered out; finished in 3.41s` |
| [media-generated-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/media-generated-list.command) | Exit 0. |
| [media-generated](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/media-generated.command) | 17 expected/selected; 14 passed; 3 ignored. `test result: ok. 14 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out; finished in 2.82s` |
| [mcp-server-list](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/mcp-server-list.command) | Exit 0. |
| [mcp-server](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/mcp-server.command) | 86 expected/selected; 76 passed; 10 ignored. `test result: ok. 76 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 71.69s` |
| [full-core](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/full-core.command) | 809 passed; 0 failed; 1 ignored. Raw summaries below. |
| [full-project](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/full-project.command) | 351 passed; 0 failed; 2 ignored. Raw summaries below. |
| [full-agent](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/full-agent.command) | 880 passed; 0 failed; 32 ignored. Raw summaries below. |
| [full-app](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/full-app.command) | 878 passed; 0 failed; 6 ignored. Raw summaries below. |
| [actionlint](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/actionlint.command) | Exit 0. |
| [mutation-anchor-audit](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/mutation-anchor-audit.command) | Exit 0. |
| [slow-lint](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/slow-lint.command) | Exit 0. |
| [slow-features](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/slow-features.command) | Exit 0. |
| [workspace-fast](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/workspace-fast.command) | 4091 passed; 0 failed; 111 ignored. Raw summaries below. |
| [workspace-fast-log](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/workspace-fast-log.command) | Exit 0. |
| [workspace-fast-verifier](/home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/gate-C/workspace-fast-verifier.command) | Exit 0. |

### Complete-target raw summaries

```text
full-core
test result: ok. 324 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 2.93s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 59 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.05s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 39 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 65 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 113 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.09s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
full-project
test result: ok. 138 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 133.95s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.43s
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 60.61s
test result: ok. 86 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 220.09s
test result: ok. 35 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 58.78s
test result: ok. 33 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 211.68s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 66.55s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 20.10s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.78s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.77s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.95s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
full-agent
test result: ok. 702 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 138.65s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 42 passed; 0 failed; 5 ignored; 0 measured; 0 filtered out; finished in 2.58s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 10 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out; finished in 10.14s
test result: ok. 19 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 15.72s
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.97s
test result: ok. 17 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 38.04s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.83s
test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 76 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 71.93s
test result: ok. 0 passed; 0 failed; 7 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
full-app
test result: ok. 878 passed; 0 failed; 6 ignored; 0 measured; 0 filtered out; finished in 404.98s
workspace-fast
test result: ok. 702 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 140.08s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 42 passed; 0 failed; 5 ignored; 0 measured; 0 filtered out; finished in 2.46s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 10 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out; finished in 10.10s
test result: ok. 19 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 15.99s
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.09s
test result: ok. 17 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 35.98s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.85s
test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 76 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 71.45s
test result: ok. 0 passed; 0 failed; 7 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 878 passed; 0 failed; 6 ignored; 0 measured; 0 filtered out; finished in 394.87s
test result: ok. 324 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 2.81s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 59 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.92s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 39 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 65 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 113 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.32s
test result: ok. 1134 passed; 0 failed; 65 ignored; 0 measured; 0 filtered out; finished in 1038.86s
test result: ok. 4 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 22.88s
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.79s
test result: ok. 14 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out; finished in 2.80s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 138 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 132.83s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.76s
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 61.18s
test result: ok. 86 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 220.50s
test result: ok. 35 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 59.07s
test result: ok. 33 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 220.42s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 66.62s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 20.31s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.35s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 10.45s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.98s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.07s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

### Runtime mutation results

All 31 accepted pairs have one named runtime failure, exit 101, then one restored pass, exit 0. Setup/compile failures, skips, survivors, interrupted frames and rejected diagnostics receive no credit. All mutable originals and all executed Rust/Cargo source manifests match the final product/witness source. `E/accepted-mutations.json` identifies every accepted directory. The final audit checks each restored binary hash against its matching gate configuration.

| Mutation | Red | Restored green |
|---|---|---|
| `lost-begin-refuses-matching-original` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 883 filtered out; finished in 1.25s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 1.69s` |
| `begin-retry-accepts-another-client-or-cut` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 598 filtered out; finished in 0.36s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 598 filtered out; finished in 0.36s` |
| `lost-floor-confirm-discards-close-handoff` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 883 filtered out; finished in 1.54s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 2.05s` |
| `gui-republishes-floor-after-lost-confirm` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 883 filtered out; finished in 1.55s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 2.11s` |
| `lost-close-confirm-discards-current-grant` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 883 filtered out; finished in 1.60s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 1.66s` |
| `completed-switch-keeps-original-envelope` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 883 filtered out; finished in 0.86s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 6.65s` |
| `status-thaws-unknown-operation` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 883 filtered out; finished in 2.85s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 7.22s` |
| `unknown-operation-ignores-bootstrap-without-permit` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 883 filtered out; finished in 12.70s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 15.94s` |
| `replayed-foreground-result-executes-again` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 598 filtered out; finished in 0.44s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 598 filtered out; finished in 0.38s` |
| `save-as-lost-result-is-not-retained` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 883 filtered out; finished in 1.27s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 1.81s` |
| `first-save-admission-lost-result-is-not-retained` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 883 filtered out; finished in 0.31s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 2.74s` |
| `rescue-lost-result-executes-without-choice-again` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 598 filtered out; finished in 0.64s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 598 filtered out; finished in 0.65s` |
| `lost-manual-grant-is-replaced-by-current-query` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 883 filtered out; finished in 1.78s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 2.70s` |
| `read-only-heartbeat-error-freezes-an-operation` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 883 filtered out; finished in 1.39s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 7.25s` |
| `post-grant-identity-escapes-revoke-transaction` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 598 filtered out; finished in 0.60s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 598 filtered out; finished in 0.73s` |
| `initial-attach-reply-loss-kills-live-owner` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 883 filtered out; finished in 0.37s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 1.36s` |
| `initial-manual-acquisition-error-is-not-fenced` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 883 filtered out; finished in 0.51s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 0.60s` |
| `destination-admission-reinspects-an-already-committed-target` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 883 filtered out; finished in 0.91s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 1.66s` |
| `identity-refusal-is-treated-as-an-original-result` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 883 filtered out; finished in 0.59s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 0.70s` |
| `unfrozen-discrete` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 883 filtered out; finished in 0.51s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 3.50s` |
| `unfrozen-batch-import` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 883 filtered out; finished in 0.50s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 3.36s` |
| `unfrozen-live-coalesced` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 883 filtered out; finished in 0.53s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 3.31s` |
| `unfrozen-keyboard-undo` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 883 filtered out; finished in 0.50s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 3.51s` |
| `unfrozen-keyboard-redo` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 883 filtered out; finished in 0.54s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 3.47s` |
| `unfrozen-router-incident` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 883 filtered out; finished in 0.53s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 3.42s` |
| `unfrozen-queued-revision-batch` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 883 filtered out; finished in 0.51s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 3.39s` |
| `additive-fixtures-require-frozen-producers` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 883 filtered out; finished in 0.95s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 4.14s` |
| `app_person_picked_destination_skips_explicit_host_grant` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 883 filtered out; finished in 0.26s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 2.58s` |
| `app_person_picked_destination_discards_granted_forwarded_scope` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 883 filtered out; finished in 0.43s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 1.84s` |
| `failed-confirm-keeps-undelivered-lease` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 598 filtered out; finished in 0.72s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 598 filtered out; finished in 0.92s` |
| `cold-open-drops-restored-view` | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 883 filtered out; finished in 2.31s` | `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 883 filtered out; finished in 4.90s` |

Exact mutation entry points, all run from this worktree with same-shell FFmpeg setup in the wrapper.

```bash
JA_S2H_MUTATION_CASES=scripts/ja1-s2h-fix3-mutation-cases.json bash scripts/ja1-s2h-mutations.sh /home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/mutations-final
JA_S2H_MUTATION_CASES=scripts/ja1-s2-c10-mutation-cases.json bash scripts/ja1-s2h-mutations.sh /home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/mutations-c10 unfrozen-discrete unfrozen-batch-import unfrozen-live-coalesced unfrozen-keyboard-undo unfrozen-keyboard-redo unfrozen-router-incident unfrozen-queued-revision-batch additive-fixtures-require-frozen-producers
JA_S2H_MUTATION_CASES=scripts/ja1-s2-c8-mutation-cases.json bash scripts/ja1-s2h-mutations.sh /home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/mutations-c8 app_person_picked_destination_skips_explicit_host_grant app_person_picked_destination_discards_granted_forwarded_scope
bash scripts/ja1-s2h-mutations.sh /home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/mutations-fix1 failed-confirm-keeps-undelivered-lease
JA_S2H_MUTATION_CASES=scripts/ja1-s2-c12-mutation-cases.json bash scripts/ja1-s2h-mutations.sh /home/riels/Projects/Personal/Kinewright/.review/ja/s2-evidence/s2h-fix1/fix3/mutations-c12 cold-open-drops-restored-view
```

### All-catalog anchor audit

`nice -n 19 python3 -I scripts/ja1-s2-mutation-anchor-audit.py` finds **8 JSON catalogs, 399 active anchors, 0 zero-match anchors and 0 multi-match anchors**, with 7 explicit `superseded_by` entries. Discovery uses the case schema and validates every named catalog, not a hand-picked C8/C10/C12 list. This is an anchor audit; it gives no new runtime credit to untouched historical campaigns.

| Catalog | Cases | Active anchors | Explicit supersessions |
|---|---:|---:|---:|
| `scripts/ja1-s2-c10-mutation-cases.json` | 26 | 26 | 0 |
| `scripts/ja1-s2-c11-mutation-cases.json` | 47 | 49 | 3 |
| `scripts/ja1-s2-c12-mutation-cases.json` | 172 | 199 | 0 |
| `scripts/ja1-s2-c8-mutation-cases.json` | 36 | 35 | 3 |
| `scripts/ja1-s2-c9-mutation-cases.json` | 34 | 33 | 1 |
| `scripts/ja1-s2h-fix1-mutation-cases.json` | 32 | 32 | 0 |
| `scripts/ja1-s2h-fix2-mutation-cases.json` | 6 | 6 | 0 |
| `scripts/ja1-s2h-fix3-mutation-cases.json` | 19 | 19 | 0 |

The eight required C10 pairs pass after their path-only retarget. The C9 move-only controls and admitted-document anchor are repaired; its obsolete whole-candidate-copy control explicitly names B's rollback replacement. Three obsolete C11 cases name their current C12 equivalents. C11/C12 cold-open-view anchors follow the central manual-start result handling and their single current C12 pair reruns. Two C8 admission controls and A's moved revocation control also rerun. All 110 original fixture/index hashes match both the parent and final source.

### Paired ordinary Save observations

The required unchanged `ja_s2_save_cost` debug publisher target runs five alternating 2aa6633/C pairs, twelve actual comment-changing Saves per run, sixty per side, with independent reopen and zero reported history decodes. These are descriptive observations, not a speedup claim or HTTP/frame/Windows timing. The 822 archived baseline files are verified against exact `2aa6633`; the original Cargo artifact and sealed binary hash are checked again. C's sealed binary matches the gate. Every binary is built before timing. No own build/test overlaps it. The runner uses `setsid nohup`, per-lane `timeout 600`, native nice 10, wait4 resources, start/end uptime/top-five CPU/GPU annotations and five-second external-load sampling. No lane or outlier is discarded.

| Save side | Mean ms/save | Median run mean ms | Full run-mean range ms | Saves | Sync share |
|---|---:|---:|---:|---:|---:|
| 2aa6633 | 249.496 | 243.695 | 234.975-282.220 | 60 | 51.2% |
| C | 239.321 | 237.275 | 236.120-244.056 | 60 | 50.8% |

`E/save-config.json`, `E/save-timings/`, `E/save-baseline-binding.json`, `E/save-current-binding.json` and `E/save-summary.json` retain every command, sample and source/binary identity. The workload includes actual sync/publication work; nested phase timings are not added as independent durations. Load annotations and measured sync share accompany the results. The release drag lanes are NOT RUN because the manual-history path is untouched, as stated above.

Per-Save timers exclude fixture setup; the retained wait4 resources cover the whole test process. Sync accounts for 51.2% of baseline Save time and 50.8% of C. All 50 five-second samples across the ten lanes flag external CPU above 20%, with individual flags ranging from 25.7% to 125.4% (a persistent Python process and occasional T3 processes). All start/end GPU annotations report P2, 1935 MHz graphics and 9501 MHz memory. The 282.220 ms baseline run mean remains included. These observations do not identify a cause for that outlier or establish a C speedup. `E/save-load-summary.json` retains the load and process-resource summary. Desktop, power and GPU settings were not changed.

Four idle host children from excluded mutation failures were terminated only after their command, executable, working directory and deleted test-root handles were verified against this worktree. This cleanup precedes the timing window. The retained sealed Save binaries, source manifests and raw records remain under E; worktree build/temp artifacts are removed at completion and large logs are gzip-compressed with raw-content hashes.
