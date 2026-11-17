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

## Seven switch mutations

The [switch mutation report](undo-switch-mutations.md) records all seven named
runtime failures and their sealed original passes. The resource test now checks
fresh bytes before teardown and names the cold-open reachability assertion;
two earlier unnamed refusals remain explicitly unqualified evidence. No fixture,
collection count, role limit or deadline changed. The final gates pass 5,464
workspace fast tests, 697 Project slow tests plus the explicit 20,000-clip case,
54 unpaid TE1 tests, all 64 session seeds, clippy/fmt, workspace and three separate
shipping builds, and lint/self-tests. The maximal qualification remains pending.

## Maximal qualification stop

`8ac25708` is the last green checkpoint. The exact 256-cycle run completed
all 256 approve/delete/Undo cycles but failed at `C4_TASK_STATUS` with
`task_authority_expired` after 3629.08 seconds, before completing the tail.
Per the brief's stop rule, no authority, limit or fixture was changed and no
run repeated. New paired measurements and final qualification gates are not
run. The test-only qualifier changes and [failure report](undo-maximal-qualification.md)
remain uncommitted for the lead's decision. No build/test remains running.

## Optimized maximal qualification stop

The lead authorized one optimized rerun under the original 3,600-second authority.
All 256 approve/delete/Undo cycles completed in 466.215756 seconds, with mean
window cost growing 0.510396 to 3.565586 seconds/cycle (R² 0.987279). The full
test failed at 817.34 seconds with typed `history_backpressure` while obtaining
an attempt history, after Freeze and task markers 0 through 4. The remaining
tail and seven cold opens did not complete. No fixture, authority or limit
changed and no growth fix or rerun followed. Qualification changes remain
uncommitted; paired timings and final gates are not run under the stop rule.
The independent Windows import fix is committed as `1236997e` and checked
separately. See [the qualification report](undo-maximal-qualification.md).

The two import guards are the sole contents of `1236997e`. Project all-target
clippy passes. The requested Project MSVC test check was attempted and blocked
before Project tests by FFmpeg's missing target `errno.h` and AWS-LC's native C
build; it is not a Windows compile pass. Both checks and the release test have
finished. Full logs are linked from the qualification report.

## Capacity ruling: fixture proof retention

The one authorized diagnostic release run reproduces the refusal at task index
5, round 0, after 816.46 seconds. Every lease reconciles: six actual Task actors
and five fixture-owned expected-state captures, each charged exactly its
34,620,352-byte creation envelope. The Task pool holds 380,823,872 bytes; the
next actor plus the required largest-capture headroom would need 450,064,576,
above its 419,430,400 ceiling. All pools have zero workspace use at inspection.
The frozen task remains within its original envelope, and no run has durably
settled. This is the designed-refusal / fixture-order case, not an over-charge.
[The complete lease table and code trace](undo-capacity-classification.md)
record all fourteen leases across the three roles. The corrected fixture releases each full proof capture only after checking its
saved combined identity; the same expected identity remains for every later
cold-open comparison. Actual actor lifetimes and all fixture constants stay
unchanged. The between-task lease guard is red on the original Small order
(3 leases versus 2 actors) and green after correction. The maximal release
qualification then passes all 256 cycles, 720 clocks, eight tasks, 128 sends,
24 attempts, the whole tail and seven cold opens in **1406.485695 seconds**,
without renewing the original 3600-second authority. Its whole-project cold
opens range from 4.350219 to 4.537343 seconds. The per-commit gate passes 3,130 core/project/agent tests, workspace clippy,
fmt and manifest lint. Paired measurements and the final gates follow this
qualification checkpoint.

## Owed after HX

### HX-G1 — Remove quadratic durable-publication growth

**Open; deferred by the lead. No growth fix in the qualification pass.** The
single release measurement completed the 256-cycle approval loop in
466.215756 seconds. Every window contains sixteen approve/delete/Undo cycles,
including the existing diagnostic work. Mean seconds per cycle grow roughly
linearly: 0.510396 to 3.565586; the fit against cycle index has R² 0.987279.
Consequently the aggregate cost has a quadratic term. Preserve the original
3,600-second authority, all capacity bounds and the full durability proofs.

| Window ending cycle | Elapsed seconds | Mean seconds per cycle |
| --- | --- | --- |
| 16 | 8.166343 | 0.510396 |
| 32 | 17.862616 | 0.606017 |
| 48 | 29.439592 | 0.723561 |
| 64 | 43.778985 | 0.896212 |
| 80 | 61.061127 | 1.080134 |
| 96 | 80.845150 | 1.236501 |
| 112 | 103.511871 | 1.416670 |
| 128 | 129.330346 | 1.613655 |
| 144 | 158.377350 | 1.815438 |
| 160 | 190.749030 | 2.023230 |
| 176 | 226.289105 | 2.221255 |
| 192 | 266.173289 | 2.492761 |
| 208 | 309.549265 | 2.710999 |
| 224 | 357.452552 | 2.993955 |
| 240 | 409.166384 | 3.232115 |
| 256 | 466.215756 | 3.565586 |

At cycles 32 → 256, the existing counters report:

| Counter | Cycle 32 | Cycle 256 |
| --- | ---: | ---: |
| `manifest_decoded` bytes | 1,253,884 | 8,007,062 |
| Manifest objects | 266 | 1,610 |
| Snapshot count | 73 | 521 |
| `snapshot_decoded_total` bytes | 9,260,771 | 184,025,071 |
| Graph depth | 9 | 9 |
| `max_history_encoded` bytes | 1,798 | 1,798 |

Investigate `crates/kinewright-project/src/bundle_recovery/publication.rs:376`
(manifest read), `:389–395` (graph accounting and membership map construction)
and `:448` (manifest object walk). These are the leading publication suspects.
The immutable memo avoids decoding every old object again: the total snapshot
counter is retained footprint, not a per-approval work counter. Also separate
the fixture's own every-32-cycle profile scan in
`orchestration_retention.rs:298–353` from product publication work before
claiming a CPU attribution. Add per-phase work counters before choosing a fix.

For scale only, the least-squares window fit corresponds to per-cycle cost
`t(n) ≈ 0.190007 + 0.012693765 n` seconds.
Its sum is `T(N) ≈ a*N + b*N*(N+1)/2`, reproducing
466.216 seconds at N=256. The task's 2,000-command room needs a
clear work-count convention: each measured cycle contains one accepted Do and
one accepted Undo. If 2,000 accepted edit transitions had this same publication
mix, N=1,000 projects to **6543.2 seconds (1.82 hours)**.
If the intended workload is 2,000 Do commands each followed by Undo, N=2,000
projects to **25780.2 seconds (7.16 hours)**.
These are extrapolations, not measurements or a qualification: the fixture's
approval cap remains 256, and ordinary unapproved commands may have a different
publication mix. Do not raise the cap to test this projection.

Acceptance for the later job: counters demonstrate publication work proportional
to the new cut, while preserving exact receipt replay, durable acknowledgements,
namespace/dependency validation, all reachable states, and bounded memory.
Measure the actual 2,000-command mix with its original authority when that job
is scheduled. [All readings and source evidence](measurements/undo-maximal-qualification-release-failure.json.gz).
