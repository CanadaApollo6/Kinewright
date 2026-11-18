# Maximal fixture capacity classification

**Designed refusal caused by retained fixture proof captures; no sizing change.**
The one authorized diagnostic release run reproduced `history_backpressure` at
specialist **task index 5, round 0**, after all 256 cycles, Freeze and task
markers 0–4. Libtest finished in **816.46 seconds**. The approval loop took
466.173640 seconds. The refused allocation belongs to this project's supplied
`HistoryPool`, **Task retained admission / live-Core capture headroom**.

## Every live lease at the refusal

The table describes the state after the rejected temporary reservation rolled
back, before diagnostic inspection. These are pool accounting charges, not RSS.
Every row's pool charge equals its immutable creation-sized retained envelope.
The diagnostic's temporary inspection captures were released, and independent
sums of all rows equal the pool's counters in every role. No holder is omitted.

| Holder | Creation kind | State | Envelope bytes | Pool charge bytes |
| --- | --- | --- | ---: | ---: |
| manual proof capture | Ordinary | live | 109,051,904 | 109,051,904 |
| manual Core | Ordinary | live | 109,051,904 | 109,051,904 |
| Task 0 expected-state capture | Task | fixture-owned immutable capture | 34,620,352 | 34,620,352 |
| Task 1 expected-state capture | Task | fixture-owned immutable capture | 34,620,352 | 34,620,352 |
| Task 2 expected-state capture | Task | fixture-owned immutable capture | 34,620,352 | 34,620,352 |
| Task 3 expected-state capture | Task | fixture-owned immutable capture | 34,620,352 | 34,620,352 |
| Task 4 expected-state capture | Task | fixture-owned immutable capture | 34,620,352 | 34,620,352 |
| RunCandidate actor | LiveCandidate | Live | 32,521,664 | 32,521,664 |
| Task 7 (Director) actor | Task | Live / preparing | 34,620,352 | 34,620,352 |
| Task 0 actor | Task | Frozen / freezing | 34,620,352 | 34,620,352 |
| Task 1 actor | Task | Live / queued | 34,620,352 | 34,620,352 |
| Task 2 actor | Task | Live / queued | 34,620,352 | 34,620,352 |
| Task 3 actor | Task | Live / queued | 34,620,352 | 34,620,352 |
| Task 4 actor | Task | Live / queued | 34,620,352 | 34,620,352 |

| Pool category | Retained bytes | Ceiling bytes | Free bytes | Live leases | Workspace bytes |
| --- | ---: | ---: | ---: | ---: | ---: |
| Ordinary | 218,103,808 | 327,155,712 | 109,051,904 | 2 | 0 |
| Task | 380,823,872 | 419,430,400 | 38,606,528 | 11 | 0 |
| Candidate | 32,521,664 | 150,994,944 | 118,473,280 | 1 | 0 |

The six actual Task actors are the Director and specialists 0–4. Task 0 is
frozen; tasks 1–4 are queued after retired attempts; the Director is preparing.
The run outcome is Pending, phase Running, and cleanup Pending. **No run or
actor has reached the durable-settle release condition.** The five other Task
leases are expected-state snapshots owned solely by the test's `histories`
array. There are two Ordinary leases (the manual actor and its proof capture)
and one live RunCandidate lease.

## The reservation that refused

`prepare_task_endpoint` prepares the first endpoint for specialist 5. It calls
`EditorTask::prepare_retained`, `fork_registered_from_candidate_retained`,
`Core::fork`, then `OwnerHistory::fork_current` / `assemble`. The new Task has
`baseline_live = 1,536`, `baseline_owners = 12,736`, document room
`1,536 + 2 MiB = 2,098,688`, owners `12,736 + 3 MiB = 3,158,464`,
24 MiB decoded history and one anchor. Its retained envelope is
`3*2,098,688 + 25,165,824 + 3,158,464 = 34,620,352` bytes.

The raw reservation fits: `380,823,872 + 34,620,352 = 415,444,224`
bytes, below the `419,430,400`-byte Task ceiling. Promotion to a live Core
must also leave the largest live Task's full capture room free:
`380,823,872 + 34,620,352 + 34,620,352 = 450,064,576` bytes,
which exceeds the ceiling. `HistoryReservation::promote_replacing` correctly
returns `PoolFull(Task)`; the temporary reservation is dropped and the eleven
original leases remain. This is not a workspace refusal and no provider starts.

Relevant implementation: `operation_history/reservation.rs:119–136` bills the
fixed envelope; `:217–257` preserves capture headroom; `:181–194` returns the
charge on last retention drop. `operation_history/owner.rs:554–608` forks under
the parent's existing workspace and `:488` promotes the new Core before start.

## Contract and concurrency

The checkpoint-1 contract in [creation sizing](undo-creation-sizing.md#ruling-7-implementation)
leaves **one largest Task capture** free. `run_admission::TASKS` is **8** task
records per run (`run_admission.rs:5`); `ATTEMPT_LIMIT` is **3** concurrent
dependency slots (`orchestration_records.rs:33`, `tasks.rs:1070`). Revise's
`RUN_SLOTS = 64` bounds its registry, not simultaneous memory admission.
The published global pool table permits 10/8/4 Task actors and 3/2/1 live
candidates with capture room at 1,000/5,000/20,000 clips. This four-clip fixture
has one candidate and six actors while preparing the seventh, below its eight
record cap. Eight actual actors plus one capture need only
`9*34,620,352 = 311,583,168` bytes, below 400 MiB. The excess is **five independent
proof captures**, not extra product tasks.

The 256-cycle Task's bill stays **34,620,352 bytes** throughout its lifetime;
`CreationSizing` is immutable. Its current retained operation charge is only
3,910 decoded / 162 encoded bytes, below its 25,165,824 / 1,048,576 limits.
Its saved owner-record decode forecast is 1,716,756 bytes, within the
3,158,464-byte owner allowance. Captures must keep the same envelope under
rulings 6/7; charging their unused future room is deliberate, not over-charge.

Freeze seals and persists the actor; it neither releases nor shrinks its fixed
lease (`orchestration/tasks.rs:678–707`). The release sweep requires the run's
durable outcome and cleanup to be settled (`writer_host/settled_release.rs:69–78`),
and verifies exact cold replay before release. Releasing the frozen task early
would change that contract. `FrozenTaskResult` itself carries fixed references
and `CapturedHistory`, not a retained full snapshot
(`editing_session/durable.rs:35–40`).

## Verified fixture correction

Settle each completed task's expected-state capture against its durable
checkpoint, keep its **combined `SnapshotIdentity`**, then release that capture
before admitting the next specialist. Keep the current task's full capture
through all its retry checks, and keep all actual actors through the original
Freeze/Stop/cleanup sequence. This follows the one-capture contract without
changing any limit, profile, authority, fixture constant, command or outcome.

`OwnerSnapshot::PartialEq` already compares only those combined identities
(`operation_history/snapshot.rs:26–31`); the expected value therefore retains
exactly the same protection for document, both stacks, ledger, lineage,
resources and sizing. Every later check still opens the real saved owner.
The permanent between-task counter requires the Task lease count to equal
its product actor count, detecting a reintroduced retained proof capture.

[Full diagnostic, all leases, source, command and binary observation](measurements/undo-capacity-diagnostic.json.gz)
records the classification. The original-order Small witness fails at
`C5E_BETWEEN_TASKS_ONLY_PRODUCT_ACTOR_LEASES` (3 versus 2); the corrected Small
control passes its entire tail. The single post-correction maximal release
qualification also passes, including all eight tasks and seven cold opens,
in **1406.485695 seconds** under the original 3600-second authority.
[Red/green guard evidence](measurements/undo-capacity-proof-release.json.gz) and
[full qualification](measurements/undo-maximal-qualification-final.json.gz) preserve the results.
