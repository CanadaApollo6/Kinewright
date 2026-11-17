# Native history switch: checkpoint 1

The product actor, compact owner checkpoint and ordered writer protocol use the
new history. The old executor and record code remain as test oracles until the
deletion checkpoint. There is no product reader for the old history record.
The portable session model remains the first gate: each process has its own
pool, and its one-project corpus asserts a peak of two Ordinary leases.

This checkpoint also ports the installation doors and their immediate readers.
They shared the old snapshot types, so leaving them on that shape would require
the compatibility projection the brief prohibits. The next checkpoint audits
the complete reader and door inventory; this document does not claim that audit
or the final deletion is complete.

| Inventory group | Current implementation | Permanent evidence |
| --- | --- | --- |
| Actor, applied delta, activity recorder | `operation_actor`; command keys and original digests survive installs; explicit recorder only | `explicit_activity_recorder_and_applied_delta_have_separate_leased_readers`; `audit_answers_read_command_fate_without_holding_a_task_history` |
| Stored owner and cold queries | `operation_checkpoint`, `StoredOwnerRecord`; combined identity, mandatory immutable sizing | `sealed_checkpoint_reopens_its_exact_owner_and_receipt_cut`; `prospective_candidate_record_opens_with_the_original_live_descriptor_before_commit` |
| GUI/writer stream | actor producer, `operation_protocol`, editing-session manual publisher | `manual_actor_records_only_normalised_successes_and_explicit_close`; portable session corpus; native manual-history tests |
| Save, first Save, rebound | capture once, publish the exact owner, install the selected namespace epoch | first-save suffix recovery regressions; `publisher_history_matches_exact_report_and_ignores_later_actor_edits` |
| Save As, branch and checkpoint selection | native snapshot installation and independently charged prepared actor | `g05a_atomic_retain_create_select_and_switch_generation`; `pinned_save_as_preserves_dirty_source_history_and_external_nested_copy` |
| Rescue and uncertain publication | full selected cut, exact cold archives, namespace owners and pins retained | `w13_frozen_cut_rescue_at_both_unknown_publications_and_late_a_isolation`; host rescue and 18-recopy tests |
| GUI handover and host recovery | validated body and combined identity before installation; revoked old producer | `a_peer_actor_adopts_the_selected_epoch_at_its_own_frozen_cut`; `s2h_recovery_plan_retries_bind_original_client_cut_and_chain` |
| Native task/candidate lifecycle | fixed creation descriptor; NeverFold; full source survives a merge refusal | `native_task_history_capacity_table`; candidate refusal/reopen tests; 20,000-clip native merge witness |
| Lineage and feedback | current occurrence map from owner; original command origin retained across forks | `delta_fork_preserves_the_current_occurrences_without_a_parent_capture`; all four native lineage integration tests |
| Resources and collection | union of baseline, Undo and Redo plus exact input certificates | `compact_history_redo_keeps_owned_lut_after_collection_reopen_and_save_as`; all 13 owned-input integration tests |
| Approval display and retained-work warnings | approved command or transition fate; uncertainty is separate | native audit and run-limit recovery tests; owner uncertainty tests; final reader audit remains |

Agent admission still uses the existing role and scope checks. Snapshot-choice
commands are person editor actions. Task resumption names its own authenticated
persisted source and candidate binding; consent previews affect a private actor.
The final door-by-door decision-2 audit and negative probes remain required at
the reader checkpoint. No broader agent restore authority was introduced.

Ordinary history needs three full leases in actual two-tab and prepared
selection flows; its pool is 312 MiB. The Task/Candidate/workspace pools remain
400/144/168 MiB, for 1,024 MiB of history admission accounting. The sizing
function, task and concurrent Revise counts, and the measured baseline tables
are in [undo-creation-sizing.md](undo-creation-sizing.md). No lease grows after
creation. A failed capture or constructor leaves the source intact.

Snapshot identity is hashed on capture/publication and validated install, not on
a warm accepted edit. An immutable reply can share its already charged encoded
capture. The producer outbox has its fixed arena; retained send buffers keep
that arena charged until their last drop. Repeated recovery confirmations keep
the exact selected reply and a request digest rather than an extra full source
history. Cold archives have actual store owners and closure pins.

H4 can group publication by staging one exact accepted prefix, publishing its
owner, final sequence, chain tip and every receipt atomically, then acknowledging
that prefix. A failed or uncertain publication must retain the original fence
and retry identity. It must preserve per-command boundaries inside a coalesced
step and each command's input extension. A group needs a measured working lease;
retaining a full preview per message would violate the current pool arithmetic.

## Checkpoint choices and boundaries

An authenticated later foreground nonce releases a cached full snapshot body
before preparing its own capture. The former client, sequence and digest stay
as a typed replay fence; a retry before that later nonce gets its exact original
reply. Fixed-size replies stay cached across refusals. This avoids a fourth
Ordinary lease in retained-task selection without discarding a source actor,
outbox or durable result. The decision row makes this protocol lifetime choice
explicit for checkpoint review.

A completed copy that still owns cold recovery archives cannot be released for
an editable open. Its archive owns the actual namespace and closure pins;
returning success while keeping that lock was misleading, and dropping the
owner would endanger recovery. The UI offers the existing read-only open and
Close. Independent archives already copied into the current namespace permit
normal explicit release. Archive relocation is a later option.

Accepted native stream packets bind the original parent identity and namespace
name by a fixed digest. The writer validates that evidence before receipt lookup
or replay. Issuer positions advance with the same root publication as their
receipts. After a restart, a lost recovery barrier is reconstructed only from
the current durable install, the validated original source and exact recorded
receipts. The selected full owner identity is checked before confirmation.

The dq1 path seams in `recovery.rs`, `session.rs` and `sidecar.rs` are unchanged
from the accepted CI-splitting base. The native fixture replaces its old history
objects; no old proof or executor has been deleted at this checkpoint.

The old GUI-trim case was retargeted to require the complete native recovery
history. `scripts/ja1-s2-c6-mutations.py` still names its old trim proof; that
entry is not evidence for this checkpoint and must be removed or retargeted
with the obsolete proof catalogue at checkpoint 3. The new combined-identity
mutation and seven creation-sizing constant mutations are the deliberate
changes recorded so far; the remaining six switch mutations belong to the
reader/door and deletion checkpoints.

[Deliberate-change logs and replay drivers](measurements/undo-switch-checkpoint1-breaks.json.gz)
record the eight red results and restoration. The constant assertions are
`TASK_TO_CANDIDATE_ENCODED_FIT`, `TASK_TO_CANDIDATE_DECODED_FIT`,
`TASK_TO_CANDIDATE_LIVE_DOCUMENT_FIT`, `CANDIDATE_PRODUCT_DOCUMENT_ENCODED_FIT`,
`TASK_TO_CANDIDATE_WORKSPACE_FIT`, `CANDIDATE_TO_ORDINARY_ENCODED_FIT` and
`CANDIDATE_TO_ORDINARY_DECODED_FIT`. The protocol mutation fails at
`INSTALL_EQUALITY_USES_COMBINED_SNAPSHOT_IDENTITY`.

## Checkpoint 1 verification

The final Linux tree passes 1,522 core/project tests (28 expected ignored) and
5,535 workspace fast tests including doc tests (287 expected ignored). Workspace
clippy with `-D warnings`, fmt, the workspace build and all three separate
`--no-default-features` shipping builds pass. The standalone builds retain the
unchanged `authentication.rs::REFRESH` dead-code warning. Slow-test lint reports
160 tests, 162 well-formed allowlist entries and 27 equipped-session skip sites.
The required 20,000-clip native-path qualification also passes on the final
tree in release (one test, 23.45 seconds). Windows and the complete slow tier
on this tree remain CI gates after the push.
[Exact commands and log hashes](measurements/undo-switch-checkpoint1-gates.json)
record the scope. No build or test remains running at the checkpoint.

[Product timing and phase counters](undo-measurements.md#h2b-checkpoint-1-shipping-actor-and-owner)
meet every timing target in five fixed pairs. The source manifest matches the
final gated Rust/Cargo tree. The historical tables remain intact.

The foundation commits since the CI-splitting base are `a0348a0f`, `21955fc1`,
`905311c2`, `1f868280`, `42a10160`, `c6cbf350`, `2d4e1de3`, `81861e46`,
`c09a66eb`, `459a1f97`, `0495cbed` and `e281962c`. The requested media fix is
`1e9dcccd`, with the original author and message preserved. `04dce7fe` is the
previously accepted sizing-scope note. None of these local commits was pushed
by this worker.

## Reader and installation-door checkpoint

The [reader/door audit](undo-reader-door-audit.md) accounts for all 207 H1 rows
and records the door-by-door authority and complete-owner witnesses. It ports
the remaining opt-in TE1 native replay reader, enables the explicit evaluation
recorder, and pins uncertainty exclusion from removal claims. Four reverted
wrong changes fail their named assertions. The nine refreshed measurement-pin
fields are source hashes; every non-hash observation is unchanged.

The final tree passes 5,545 workspace fast tests, 4,381 touched-crate slow tests
(including all 64 session-model seeds), 54 opt-in TE1 tests, workspace and feature
clippy, fmt, the workspace build, three separate shipping builds and slow lint.
The MSVC check stops in native dependencies without a Windows SDK, before
project-test compilation; the platform sweep and portable-body probes do not
replace Windows CI. This checkpoint deletes no old proof or record code.
Legacy deletion, C6 retargeting, the remaining stream/resource mutations and the
256-cycle maximal-task qualification remain for the next checkpoint.

## Legacy and mutation checkpoints

`3be098c8` removes the old executor, records, Save-side floor and cursor proofs;
[the deletion report](undo-legacy-deletion.md) lists the removed paths and native
replacements. All 207 inventory rows remain accounted for, including the 28
obsolete-proof retirements. Muse F3's manual-history site and the CoreState
restore fallback are removed with the legacy path.

The [mutation checkpoint](undo-mutation-retargeting.md) accounts for the original
C2/C4/C5/C6 case sets and qualifies all 29 retargeted C6 edits, five additional
C4 edits and seven distinct C2 edits with runtime red/restored-green controls.
Its final gates pass 5,463 fast, 5,587 Linux slow and 54 unpaid TE1 tests, all
64 session seeds, workspace and feature clippy, fmt, workspace and three separate
shipping builds, and lint/self-tests. The seven switch mutations and exact
256-cycle maximal qualification remain; no new timing result is claimed here.
