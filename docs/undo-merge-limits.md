# Merge-path size qualification

Ruling 5 correctly reads `document_bytes` as the encoded whole-document limit;
`document_live_bytes` bounds its live allocation, including spare capacity.
These measurements qualify replacement limits. They do not change the shipped
profiles or switch a product caller.

Run the size probe from the repository root after sourcing
`scripts/setup-ffmpeg.sh`:

```sh
flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo run --release -p kinewright-project --example undo_merge_limits
```

The document fixture has eight video tracks, unique solid clips of 90 frames,
no effects or owned media, and ordinary opaque lineage IDs. Document live bytes
come from the capacity estimator on the allocation itself. No compacting copy
precedes measurement. The lineage charge column evaluates the current
NeverFold baseline accounting, including its conservative BTree node allowance;
it is an accounting bound, not RSS.

| Clips | Encoded document bytes | Live document bytes | Encoded baseline lineage | Baseline owner charge |
| ---: | ---: | ---: | ---: | ---: |
| 1,000 | 156,882 | 240,848 | 134,820 | 937,024 |
| 5,000 | 788,882 | 1,200,848 | 682,820 | 4,649,024 |
| 20,000 | 3,179,988 | 4,800,848 | 2,757,821 | 18,569,024 |

One four-track cut on the 20,000-clip project leaves 20,004 clips, encoded as
3,185,640 bytes, with **7,200,848 live bytes**. The four initially full track
arrays double their capacity. Spare capacity is retained and charged; a copy
made to normalize it would violate the command-path ruling.

The cut builder follows `transcript_edit::append_project_cut`: two splits on
each participating track, ordinary deletion of the secondary gaps, and one
ripple deletion on the primary. Four locked tracks therefore require twelve
operations per removal. The cut is six frames long, every sixty original
frames, at 30 fps. The source is long enough for all 2,000 removals (about
67 minutes; 2,000 two-second intervals exceed an hour). The 32-removal rows
are single batches, including one command/step charge, rather than 32 times
the isolated charge. Encoded charge excludes the array's commas and brackets;
wire bytes include them. Decoded charge comes from actual History admission.

| Tracks | Removals per batch | Wire bytes | Encoded charge | Decoded charge |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 1 | 99 | 95 | 2,800 |
| 1 | 32 | 3,310 | 3,213 | 79,034 |
| 4 | 1 | 377 | 364 | 10,094 |
| 4 | 32 | 12,938 | 12,553 | 313,906 |

The complete four-track stream of 2,000 separate removal commands charges
**850,911 encoded / 20,433,822 decoded bytes**. Forwarding its applied delta
unchanged to a fresh History as one batch charges **850,911 / 19,730,174**.
The final small source document is 1,324,935 encoded / 1,966,704 live bytes.
These executions use explicit sizing-probe bounds, not the unchanged product
Task/Candidate profiles. The old Task allowances of 256 KiB encoded and 8 MiB
decoded cannot hold this stream. The sizing option below raises those
allowances fourfold and threefold, respectively. The probe completed with
exit zero in `target/hx-h2b/merge-limits-measure-release.log`.

## Reservation boundary

The current owner constructor reserves `(2 + K)D + O + A` before calling its
document factory (`operation_history/owner.rs`, `with_bounds`). The measurement
constructor only lowers Core bounds; its pool lease is still the full role
envelope. Captures and cold opens also acquire independent full role leases.
Consequently, charging Candidate by its frozen input's measured need is not
enough to change the Task reservation.

Even before choosing rounded profile constants, the current four cache slots
require more than the authorized total. Take only the measured minima:
`D = 7,200,848`, `A = 18,569,024`, Task `O = 20,433,822`, and Ordinary one-step
`O = 19,730,174`. The Ordinary owner actually needs more than A because it
also retains the derived fold indexes; ignoring those only lowers this bound.
Origin/approval allowances, all Candidate leases and all captures are omitted.

| Minimum reservation with the current cache counts | Exact bytes | MiB |
| --- | ---: | ---: |
| Eight Task leases: `8 × (6D + O_task + A)` | 657,663,472 | 627.197 |
| One Ordinary lease: `6D + O_merge + A` | 81,504,286 | 77.729 |
| Shared workspace: `(2A + 7D)` plus ten percent | 96,298,383 | 91.837 |
| **Lower total, before any Candidate or capture** | **835,466,141** | **796.763** |

The workspace floor follows the existing arithmetic, not an arbitrary rounded
window: the Task owner reserves `2A` as scratch, and the ripple reservation
checks `3D + 4D` before growth. `ReplayBudget` adds ten percent. The decoded
and owner minima follow from admitting the required stream and forking the
20,000-clip project. Since fixed admission reserves the maxima for every Task,
this is a reservation floor even when some live tasks are still small.

The following fixture-sized option retains the existing checkpoint counts and task
step allowance. It is not installed and its full replay-workspace qualification
has not been completed. The table deliberately excludes all workspace to show
the retained-reservation problem alone.

| Role | Encoded operations | Decoded operations O | Common document D | K | Owner allowance A | Retained envelope |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Ordinary | 1 MiB | 24 MiB | 8 MiB | 4 | 32 MiB | 104 MiB |
| Task | 1 MiB | 24 MiB | 8 MiB | 4 | 24 MiB | 96 MiB |
| Candidate | 1 MiB | 24 MiB | 8 MiB | 0 | 24 MiB | 64 MiB |

The encoded whole-document allowance would be common too; 8 MiB leaves room
for clips added by cuts after the 20,000-clip starting project. Owner allowances
must cover baseline lineage as well as command origins, resources and approvals.

| Holders, restricted to just one run | Count | Retained MiB |
| --- | ---: | ---: |
| Ordinary Core | 1 | 104 |
| Task Core | 8 | 768 |
| Candidate Core | 1 | 64 |
| Subtotal, without captures or shared workspace | | **936** |

Eight task records are allowed per run (`run_admission::TASKS`, also used as
`orchestration_records::TASK_LIMIT`). `HostRun::tasks` owns their live actors;
waiting tasks do not have to be terminal. Settled actors are released only by
an explicit successful checkpoint/reopen check. The host also has 64 run slots
(`writer_host/revise.rs::RUN_SLOTS`) and creates a registered Candidate when
starting each Revise run. `continuation.rs::retain_run` independently enforces
the same 64 retained-run limit for the runtime map. Thus the host's record
slots allow 64 run candidates and up to 512 task records, before private
previews and captures. These are code-level upper counts, not a claim that the
existing store or pool can admit every slot at maximum size, nor that 512
providers execute concurrently. Even the smaller one-run table is above the
authorized total of 768 MiB.

Measured Candidate charging cannot remove the Task subtotal. Measured charging
for Task and Ordinary would additionally have to cover atomic growth, cold
open, independently retained captures, and cache allocations during Undo/Redo
without introducing a history-capacity refusal there. That is a change to the
reservation lifecycle, rather than a smaller input to `OwnerHistory::measured`.

The native partly-full destination does retain its source: `admit_task_merge`
calls admission before binding a pending merge, and `EditorTask::admit_merge`
leaves its frozen result unchanged on refusal. The persisted task checkpoint
and result reference also remain. Other publication doors have not yet been
switched or requalified.
