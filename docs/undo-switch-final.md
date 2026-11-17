# Native history switch: final checkpoint

## Capacity and maximal qualification

The maximal release qualification passes **256 approve/delete/Undo cycles and
the complete tail in 1406.485695 seconds**, below the original 3600-second
authority. It includes eight tasks, 720 clocks, 128 sends, 24 attempts, Freeze,
Stop, durable settlement, collection, exact reopen/audit and seven cold opens.
No authority renewal, profile increase, fixture constant change or production
growth fix was made.

The earlier refusal was **Task retained admission**, preserving one largest
live-Core capture's room. Six actual actors plus five fixture proof captures
charged 380,823,872 bytes; the next actor and required free capture room would
need 450,064,576 against the unchanged 419,430,400-byte ceiling. Every charge
equals its immutable envelope. None of the actors had reached durable settle;
Freeze is not supposed to shrink or release one. The [full fourteen-lease
table and code trace](undo-capacity-classification.md) distinguish the actual
product holders from the extra fixture proofs.

The fixture now verifies each completed task's durable combined identity, drops
its full expected-state capture before admitting the next task, and preserves
that same combined identity for all later cold-open checks. Snapshot equality
already used only that identity. Actual actors keep their original lifetimes.
`C5E_BETWEEN_TASKS_ONLY_PRODUCT_ACTOR_LEASES` fails with the old Small order
(three leases for two actors), and passes with the corrected order through the
entire Small tail. `C5E_DURABLE_PROOF_IS_EXACT_BEFORE_RELEASE` checks the cut
before releasing each proof. Both pass across all eight maximal tasks.

The [qualification checkpoint](undo-maximal-qualification.md) records the raw
red/green controls, exact release command, fixed source hash and full-tail proof.
Commit `a3d34c1a` contains that checkpoint; `1236997e` separately gates only the
two Linux fixture imports identified by Windows CI.

## Product-path measurements

The five fixed alternating pairs use the unchanged heavy fixture and frozen
old-Core control. All ten executions return zero errors. Milliseconds below
are the median of each side's five lane summaries; [all samples, ranges and
phase counters](undo-measurements.md#h2b-final-product-path-measurements) remain
beside the earlier tables.

| Lane | Old Core | Product | Target result |
| --- | ---: | ---: | --- |
| Edit at 1,000 steps | 0.200734 | 0.258075 | All five pairs within 1.5× |
| Edit at 10,000 steps | 0.378070 | 0.273180 | All five pairs within 1.5×; depth sample ranges overlap |
| 120-batch gesture apply | 28.056302 | 30.290638 | All five pairs within 1.5× |
| Gesture Redo | 0.001417 | 15.843536 | All five below 50 ms |
| Edit at folding bound | 0.206076 | 1.439672 | **154/160 below 2 ms; six misses; worst 2.239076 ms** |
| Validated open, 2,000 steps | — | 347.974835 | All five below 1 second |
| Cold Undo | — | 249.199941 | Recorded measurement; no tuning target |
| Undo walk p95 | 0.002460 | 5.685876 | All five below 100 ms |
| Undo walk worst | 0.063745 | 247.498573 | All five below 1 second |

Every pair has the same counters as checkpoint 1. A warm command adds one
history-side document copy, zero exact measurements and zero document bytes
serialized; owner document/resource/seed/ledger scans are zero. The kernel
still performs its existing two validation, one duration and two identity
passes per operation. These are kernel costs, separately counted, not a claim
that all document-wide work has disappeared.

A fold has two history copies, one exact measurement and 245,242 document
bytes serialized; owner work touches one folded command and three origins,
with zero whole-document, resource-set, seed-map or ledger scans. On this
sync-lock fixture the folded payload/resource/seed counts are zero. The counts
pin work proportional to the folded step, but do not explain the timing tail.
No quiet wait, repeated timing or code tuning followed the six misses.

Validated open records two history copies, five exact measurements and
980,932 document bytes serialized, plus four owner document scans, two resource
scans, two seed verifications, one lineage encoding and one owner hash of
617,427 bytes. Cold Undo replays 1,999 operations with three history copies,
two exact measurements and 490,466 document bytes serialized. Those are cold
boundary/replay costs; incremental kernel validation remains separate work.

## Reader, door and proof closure

All **207 unique inventory rows** are accounted for: 126 moved, 28 retired,
18 preserved and 35 unchanged; none pending. The 28 obsolete-proof retirements
are complete. The old executor, records, floor/cursor proofs and compatibility
readers are gone. Muse F3's `manual_history.rs` site and the
`CoreState::restore` fallback were removed with that legacy path. Eight kernel
contract modules moved to the native Project actor tests; the H1 oracle and
67-kind fixtures remain. [The deletion report](undo-legacy-deletion.md) lists
their replacements. [The exact deleted-file list and line counts](undo-deleted-files.json)
separate the full switch from the post-audit checkpoints and distinguish Git
renames from deletions.

The [door-by-door table](undo-reader-door-audit.md#installation-authority-and-state-checks)
records every installation path, selector, authority proof and complete-state
witness. Person-selected checkpoint, branch, rescue and recovery doors install
the selected state without a new destructive approval. Agent-reachable doors
can restore only their authenticated actor's last persisted state; the portable
attachment-role matrix rejects arbitrary selection before other refusals.
All comparisons use the combined Core/owner identity. Unhashed media remains
an external locator and cannot stand for store-owned bytes.

For **H4 group publication**, preserve the accepted prefix atomically with every
receipt, final sequence/chain tip, issuer position and original namespace proof;
keep all reachable pins until exact publication is known. A coalesced step still
contains separate command boundaries. The foreground reply can release its
full capture only behind the typed replay fence; a reply owning a cold archive
cannot release it. A working-copy lease is creation-sized. Do not reintroduce
a full preview snapshot per message. Identity hashing is a Save/install/cold
boundary cost, not warm-edit work.

**Task edit durability.** An MCP reply that acknowledges a non-destructive task
edit does not make that edit durable. The edit becomes durable at the next task
checkpoint publication: preparation, finish, freeze, merge or attempt
retirement. This is unchanged from `2a914931`. If the host stops between the
reply and that publication, the acknowledged edit is lost. Ticket **HX-T1**
below owns the change.

The mutation checkpoints qualify **29 retargeted C6**, five additional C4 and
seven distinct C2 edits with runtime red/restored-green controls. The other
84 retained C6 anchors were checked for unique applicability, not rerun as a
complete historical corpus. All **seven switch mutations** are runtime red at
their named assertions and restored green. [Retargeting evidence](undo-mutation-retargeting.md)
and [switch evidence](undo-switch-mutations.md) preserve the original case sets
and unqualified attempts; 35 pre-existing zero-match catalogue hooks are
explicitly outside this qualification.

## Final gate

The final Linux gate passes **5,464 workspace fast tests** (283 declared
ignores), **5,588 workspace slow tests** (159 declared ignores), and **41 library
plus 13 unpaid opt-in TE1 measurement tests**. All 64 session seeds / 65,536
attempts and the explicit 20,000-clip product path pass. Workspace and TE1-feature
all-target clippy with `-D warnings`, fmt, the workspace build, all three separate
shipping builds, slow-test lint, three mutation-runner fault tests, 52 slow-harness
self-tests and 26 CI-planner self-tests pass. The measurement pin observer finds
zero changed leaves. The qualified fixture and all implementation source stay
unchanged through these gates.

An initial fast-gate process ended with signal 143 before a result; its available
log has no assertion failure. Its cause is unattributed and it earns no pass.
The subsequent complete fast run supplies the result above. Both attempts remain
in the [final gate evidence](measurements/undo-switch-final-gates.json.gz).

The installed MSVC-target workspace/test check finishes with exit 101 in native
cross-build prerequisites: FFmpeg cannot find `errno.h`, Whisper cannot find
`stdio.h`, and AWS-LC's Linux C compiler cannot resolve `pthread_rwlock_t`.
Project test compilation is not reached; the log contains no Rust diagnostic.
This is the machine limitation accepted by the lead, not a Windows pass.
**Windows CI remains the compilation/execution check.** No SDK, profile or
native dependency workaround was introduced. The full command and native
failure log are retained in the final gate evidence.

All heavy commands use the prescribed FFmpeg setup, normal shared lock, four
build jobs and nice 10; Python checks use nice 19 and isolated mode. Completed
release artifacts were removed only after their commands, hashes and raw proof
were sealed. During the slow test phase, only completed fast executables with
newly compiled slow replacements were removed, excluding active processes; all
upcoming slow binaries remained. The largest recorded footprint was 39.261 GB; slow compilation reached
about 38.5 GB before that targeted cleanup. Both are below 40 GB. No cleanup touched another worktree.

## Owed after HX

Local ticket **HX-G1**, under [Owed after HX](undo-switch-progress.md#owed-after-hx),
retains all sixteen cycle windows, the R² 0.987279 linear per-cycle growth,
manifest/snapshot counters, publication suspects and explicit extrapolations
to 2,000 accepted transitions or 2,000 Do/Undo cycles. It is deferred, not fixed
or claimed fast enough for that projected workload. Creation-sized capture
leases and leases that grow with actual use remain the previously recorded
follow-ups; admission accounting stays **312 + 400 + 144 + 168 = 1,024 MiB**.

Local ticket **HX-T1: publish task checkpoint before the MCP reply.** Publish
the task's checkpoint before the MCP reply acknowledges a non-destructive task
edit, so that an acknowledged edit survives a host stop.
