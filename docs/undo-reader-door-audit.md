# H2b reader and installation audit

Audit base: `aa39a0e5` (the Windows cfg sweep follows the accepted Linux CI repairs). The checkpoint preserves the lead's accepted lifetime,
namespace, issuer and pool decisions. It does not delete the legacy proofs.

[The row inventory](undo-reader-inventory.json) includes all 207 table rows of
H1's inventory, including the lead's GUI transport additions. Original line
numbers identify historical rows. Replacement symbols identify the current
implementation. Rows share runtime witnesses where they consume the same
contract; this is not a claim of 207 independent tests.

## Reader contracts

| Family | Current reader and evidence | Disposition |
|---|---|---|
| History and command position | `operation_actor::ChangeId` carries actor, generation, epoch, accepted position and revision. `Core::capture_state` captures the token and owner atomically. | No stack-depth or floor-offset substitute. Empty Undo, Redo and Close advance accepted position. |
| Applied delta | `Core::operations(false)` returns `OwnerOperations`. `TimelineBranch`, MCP proposals, investigator proposals, durable tasks and candidate merges consume that leased slice. | Every delta role is NeverFold. Undone and discarded edits are absent. |
| Complete activity | Evaluation `PreparedFixture::new` now opts into `Core::spawn_recording`; `query_operations` uses `Core::operations(true)`. The baseline harness already uses `spawn_recording_at_revision` and leased operations. | Audit found and fixed a disabled recorder followed by an obsolete direct query. No app constructor opts in. |
| Opt-in TE1 durable replay | `Studio::finish` authenticates native checkpoints; `checks::histories` validates a leased `SnapshotTransport` into a private actor, walks Undo/Redo, and checks both stacks and command boundaries. | H1-207 had remained on the old API behind its feature. The feature build and portable replay/combined-identity regressions now pin this reader. |
| Scoring evidence | The evaluator keeps the leased operation reader through scoring while `EvalOutcome` owns its independent evidence Vec. | Both vectors fit the reader's Ordinary 104 MiB envelope: at most twice its 24 MiB decoded operations. No document or lineage is copied into this reader. |
| Approval audit | `KeptTaskAudit::removal_proof` reads the exact consent reference from the authenticated saved branch. `LiveEditCut.answers` contains bounded scalar ledger answers. | No final-document target-satisfaction check. Legacy `removal_proof` remains only as the differential oracle until deletion. |
| Host and UI approval readers | `task_consent` composes scalar answers, orchestration serializes the lists, `RunView::of` formats them, and feedback/recovery UI displays those lines. | Uncertain is distinct from kept and missing. The UI excludes it even if an inconsistent page also sets `applied=true`. |
| Retained-work disclosure | `editing_session::retained_work` and the two app recovery renderers consume the same missing/uncertain summary. | No alternate document-based answer. Damaged authentication remains uncertainty, not a removal claim. |
| Lineage | `OwnerBoundary`, `Origins`, baseline seeds and replay observations supply occurrence maps and split ancestry. Feedback mapping reads that boundary. | No boundary map retained per command. Clip-id reuse preserves original occurrence identity. |
| Resources and collection | `StoredOwnerRecord` resources and exact bindings feed checkpoint edges. Baseline and both stacks remain covered; certificates, decoder leases and immutable pins have independent lifetimes. | Existing real LUT Redo/collection/cold-open/Save As test remains the byte-ownership witness. |
| Unhashed media | `Resource::Media` without hash and length remains an external locator, with no owned binding, manifest, table or media edge. | Portable codec refusal and real owned-input tests remain. New rich-state doors retain an unbound resource too. |
| Boundary consent previews | `Core::boundary` and `preview_command` replay in a private admitted holder. Preview policy and creation bounds come from the source. | No agent can replace its live actor with a chosen preview. |
| Receipt, run and metadata IDs | Existing request ID, payload digest, original recovery ID, issuer sequence, accounting turn and terminal tokens remain separate from Core keys. | Fold/Undo never refunds spending or reinterprets those identities. |
| Kernel determinism and reservations | H1's operation/model hash-container audit and 67-kind differential remain. The kernel's validation, duration and identity passes are unchanged. | No incremental kernel validation or new timing claim in this checkpoint. |

## Installation authority and state checks

Person-selected state is authorized by the private editor path. Agent MCP and
Viewer/Producer attachments cannot invoke a snapshot-selection operation.
The portable authority matrix drives 13 commands through both attachment roles,
checks the exact `role_refused` result, checks the MCP command whitelist, and
compares the unchanged complete owner after every refusal. It does not rely on
a later missing file, lease, branch or input refusal to prove authority.

| Door | Current path | Who selects the state / proof |
|---|---|---|
| Fresh document | `Core::spawn`, `ProjectSession::create` | Person's new/open document. Valid baseline equals current with empty stacks. |
| Full manual cold open | `Ja1EditingSession::open_loaded`, `Checkpoint::open`, `Core::open` | Person's selected project. Validated token, combined identity and scope checked before live actor creation. |
| First Save and rebound | `first_save`, `from_borrowed_copy`, `ProjectAttachment::install_snapshot` | Frozen own actor cut. `first_save_save_and_cold_open_preserve_the_complete_owner`. First Save is editor-only. |
| Ordinary Save and reconciliation | `save_active`, `published_history`, receipt reconciliation | Exact actor cut and publisher report. Save does not select or fold another history. Same rich-state test; lost-ack/reconciliation suites remain. |
| Save As | `save_as_with_admission`, copied checkpoint, peer install | Person's selected destination, exact source cut. `save_as_checkpoint_and_working_branch_preserve_the_complete_owner`. |
| Host startup and foreground adoption | `HostSnapshot::verify`, `install_verified`, `install_validated_host_snapshot` | Private bootstrap/editor. Body validates before identity comparison; a differing attached manual stream refuses `manual_stream_required`. Rich host-shadow test plus role matrix. |
| GUI host activation | `complete_host_cut`, `activate_host_cut_prepared` | Publisher-selected result bound to the GUI's original frozen cut and combined identity. Existing first-save, C11 and C12 full-owner comparisons and stale-cut refusals remain. |
| Manual restart barrier | `begin_recovery`, durable install fact, `PublishBarrier`, `ManualRecoveryEndpoint::install`, `ConfirmBarrier` | Original accepted suffix and receipts, not a caller-chosen past state. Namespace and exact installed identity checked. Editor-only controls; original receipt window survives restart. |
| Working branch selection | `prepare_recovery`, `prepare_project_selection`, `prepare_selection`, `publish_project_selection` | Person selects a checked branch. Rich-state test restores it after Redo changed the current line. An unrelated target's head must remain unchanged through preliminary Save. |
| Root/checkpoint selection | `prepare_checkpoint_restore`, `prepare_selection` | Person selects the pinned checkpoint and its closure. Rich-state checkpoint test checks complete state and command fate. |
| Checkpoint-only document recovery | `CheckpointRecovery` | Person explicitly accepts unavailable Undo history. New baseline=current, empty stacks and new epoch; current-document resources are retained. Existing checkpoint recovery and refusal tests remain. |
| Host branch/snapshot switch | `prepare_switch`, `switch_preview`, `prepare_gui_switch`, `activate_prepared_gui_switch`, confirmation | Editor chooses target. Prepared payload and committed result use the combined identity, branch, revision and inputs. Original envelope/receipt does not choose a later state. |
| Rescue | `prepare_rescue`, `preview_rescue`, `rescue_with_activation` | Person selects destination and consents to disclosure. Rich-state rescue test starts with uncertain Save and missing parent; no new destructive approval is requested. |
| In-memory thread/investigator fork | `TimelineBranch::fork`, `Core::fork` | New fixed base from the current actor, not an older snapshot. Source pool and NeverFold policy retained. Applied-delta tests remain. |
| Registered task/session/candidate reopen | `prepare_reopened_branch`, `Core::open` | Current persisted checkpoint for the authenticated branch and fixed base. Rich task-reopen test checks both stacks, approval, resources, policy and baseline preview. |
| Frozen specialist/task recovery | `restore_frozen_task_inner`, `restore_task_candidate`, `EditorTask::reopen` | `saved_task_history(state, task)` authenticates project/run/task/base; candidate binding and frozen source must still equal that record. Agent supplies no snapshot/root selector. Existing frozen-task recovery tests remain. |
| Private consent and merge preview | `Core::preview_command`, `Core::boundary`, `prepare_merge` | Constructed from the current source actor or its authenticated frozen delta. Results are validated private holders, not a live restore API. |
| Prepared media/interpretation | `prepare_owned_media`, prepared atomic update | Editor supplies a command, admitted inputs and exact before cut. Refusal leaves source and pins unchanged. Agent role matrix covers both public preparation commands. |
| Reopened chat display | `chat_ui` validated `Core::open`, `TimelineBranch::from_registered` | Baseline-bound branch display and fixed-base delta. Does not select the person's live project. |
| Failed publication rollback | Prepared-update transaction cancellation and original expected cut | Internal restoration of the same attempted transition. No externally selectable root, no new approval and no accepted acknowledgement before root publication. |

The rich-state project fixture has nonempty Undo and Redo, an applied approval,
baseline occurrence identities, per-command origins and a resource reference.
It compares the complete serialized owner, then queries the approval answer.
Only the install epoch and its specified fate-provenance changes may differ.
A new peer observes absent-to-applied and records that install epoch; continuity
of an unchanged applied approval preserves its prior provenance.

Existing C10/C11/C12 tests remain necessary for the process/namespace/receipt
boundaries. The new project tests do not replace them or claim to simulate a
writer process crash. Real owned bytes are covered by `ja_s3_owned_inputs`;
the new fixture's external locator intentionally cannot stand for owned bytes.

## Evaluation ownership

TE1 callers pass one pool through fixture creation, the writer, cold open and
private replay. The disposable setup actor retires synchronously after its Save;
writer shutdown retires the manual actor before the cold-open check. A candidate
capture becomes a leased `SnapshotTransport`, retained through verification and
serialization. Private replay takes its own creation-sized reservation, drops
the rendering capture before Undo/Redo, and leaves the source transport intact.
Only nine generated source-hash fields changed in the measurement pin; all
non-hash fields, frozen media/oracles, tool surfaces, models, limits and grant
rules compare equal. The old pin and its grants are invalid for this source;
refreshing observations grants no native authority.

No evaluator reads an obsolete record or holds a decoded JSON history outside
its reservation. Paid/native entry points are compiled only, never invoked by
this checkpoint's verification.

The replay reader checks the combined identity before making any changes. After
Undo/Redo its position must advance by exactly twice the original Undo depth;
a streaming digest covers baseline, current document, both stacks, command
boundaries, original IDs/digests, epoch and policy. This later-command behavior
check does not call two positions the same snapshot. Its portable regression
starts with a two-command coalesced step and a live Redo tail. Another constructs
equal Core records with different owner origins and requires reopen equality to
refuse that comparison.

## Snapshot identity cost and H4

`SnapshotIdentity` combines canonical Core and owner digests, including immutable
creation sizing. Capture, Save, semantic open and installation compute/check it.
Warm commands update counters and sparse owner state without hashing history.
The accepted checkpoint-1 counters and paired measurements remain unchanged.
No new latency measurement is made for this test/audit change.

H4 must publish one accepted prefix, its final sequence and chain tip, and every
receipt in one root transaction. Keep normalized command boundaries inside a
coalesced step, original command IDs/digests, input origins and issuer positions.
A later operation may retire an acknowledged heavy reply body, but must retain
its typed replay fence. Pool/capture admission, uncertain publication and the
original namespace proof must hold for the whole prefix.

## Deliberate wrong changes

Each wrong change compiled, failed at the named assertion, and was restored.
The final gates run on the restored source. [Logs and replay driver](measurements/undo-reader-door-breaks.json.gz)
retain the results.

| Wrong change | Named red assertion |
|---|---|
| Compare the evaluation snapshot's Core digest alone | `TE1_REOPEN_REQUIRES_THE_COMBINED_OWNER_IDENTITY` |
| Count an uncertain approval in removal claims | `UNCERTAIN_APPROVAL_NEVER_COUNTS_AS_REMOVAL` |
| Bypass attachment role admission for snapshot commands | `AGENT_SNAPSHOT_DOOR_REFUSES_BEFORE_INSTALL` (Viewer, First Save; otherwise a later root refusal would hide the authority gap) |
| Construct an evaluation fixture with recording disabled | `EVAL_FIXTURE_OPTS_IN_TO_COMPLETE_RECORDER` |

The first two pin switch-mutation requirements; the last two independently pin
the admission and opt-in recorder findings from this reader audit. The earlier
protocol combined-identity mutation remains in the checkpoint-1 evidence.

## Checkpoint verification

The restored final tree passes 5,545 workspace fast tests across 92 targets,
with 287 existing skips, and 4,381 touched-crate slow tests across 82 targets,
with 116 existing skips. All 16 long session partitions pass: 64 seeds and
65,536 attempts. The opt-in TE1 feature passes 41 library tests and 13 unpaid
measurement tests, with zero real provider launches in the scripted cases.
The five rich-state project tests and the portable authority matrix are included
in the corresponding full gates.

Workspace and explicit TE1-feature clippy pass with `--all-targets -- -D warnings`;
fmt, the workspace build and all three separate `--no-default-features` shipping
builds pass. Those builds retain the existing `authentication.rs::REFRESH`
dead-code warning. Slow-test lint passes: 160 tests, 162 well-formed allowlist
entries and 27 equipped-session skip sites.

The MSVC test check was attempted on the installed target. FFmpeg and Whisper
binding generation fail on missing `errno.h`/`stdio.h`, and AWS-LC's native build
also fails; no Windows SDK/C toolchain is available here. It stops before project
test compilation. The Windows cfg sweep and hidden-body compile probes are
recorded separately in [undo-windows-cfg-sweep.md](undo-windows-cfg-sweep.md).
This does not qualify native Windows compilation or execution.

An initial workspace run could not execute two planned launchers after an
incorrect mid-run cleanup removed them. The executing tests passed. Both
launchers were rebuilt and the complete workspace gate was rerun successfully,
including their four guardian tests. Cleanup now refuses to run while this
worktree has a Cargo process; subsequent pruning occurred only between gates.
Logs, command drivers and counts are under `target/hx-reader-audit/`.
All started runs have finished. No source files, legacy proofs or fixtures were
deleted at this checkpoint. No limits, timeouts, performance fixtures or kernel
passes changed. The dq1 seams in `recovery.rs`, `session.rs` and `sidecar.rs` were
not touched.

## Remaining checkpoint

The old executor, records, replay/floor proofs and C6 trim mutation entries stay
until legacy deletion. The old `HistoryRole` constants still name legacy store
escrow code; they are not the creation-sized native record limits. Remove those
module dependencies with the obsolete records, preserving the independent
store/run escrow contract. No legacy record reader is added or enabled here.
The 256-cycle maximal task qualification and the remaining switch mutations
remain required before the final H2b report. The uncertainty/removal-total
mutation is run at this reader checkpoint.
