# History capacity: stage 1

First delivery: compact persisted and transported history, explicit document resolution, compact lineage, and the 400-clip vertical task lifecycle. Final simultaneous-cap constants and the remaining stage-1 proofs follow this delivery. No raw storage limit is raised. Work is local on `te1-run-limit` from `e0131f02`; nothing is pushed.

## Representation and boundary inventory

`HistorySnapshot` remains the resolved in-process form, including every Undo/Redo boundary document. Its portable encoding contains baseline and current documents once each and a `HistoryJournal` of commands, compact identity effects, accepted order, floor offset and boundary/current hashes. The ordinary Core executor replays the journal and checks its resulting commands, effects, cursor and every retained document hash. After validation, a restored actor shares the supplied immutable boundary documents, avoiding a second permanent document set. Existing Undo/Redo and boundary-document readers remain intact.

`HistoryRecord` is the persisted form: explicit role, baseline/current document object references and journal. Both document references are graph dependencies. `ProjectStore::resolve_history` pins the closure, reserves typed replay memory and resolves through the same journal implementation used by transport. Pre-adoption recovery uses that resolver with the namespace retained until adoption. Compact lineage stores baseline/current maps, split ancestry and changes carrying the original occurrence identities; decoding rebuilds the existing per-boundary maps. No fresh occurrence or publication batch IDs are allocated by replay.

| Boundary | Form carried |
| --- | --- |
| Project History object; standalone `admit_history` | Referenced `HistoryRecord`; standalone admission stages both documents first. |
| Checkpoint open, recovery selection, branch reads | Explicit pinned record resolution to `HistorySnapshot`. |
| Host snapshot/bootstrap, reply history and session-status history | Self-contained portable history, two documents once each plus journal. |
| Leased GUI snapshot reader | Same pinned resolver, charged to the GUI ordinary read pool, including feedback accepted-cut reads. |
| Switch attachment/payload | Portable history, owned inputs and compact lineage; verifies the stored record reference separately from the portable digest. |
| Borrowed foreground snapshots and unsaved captures | Portable digest binding and resolved capture; live state charged separately from encoded transfer. |
| Save As and rescue | Resolved source capture; output stages/copies the referenced document closure. |
| Registered floor anchor | Compact transport; reconstructs cursor documents only when the floor-prefix proof needs them. |
| Core Undo/Redo, input retention, feedback, task Undo policy | Full resolved boundary documents and lineage maps, unchanged in process. |
| App recovery suffix | Existing header document plus command journal; distinct from checkpoint history. |
| Outstanding manual publication | Existing accepted batch, predecessor and before/after digests. |

Stored-object hashes and portable-history digests are separate identities. Checkpoint/receipt/floor graph references retain the former; manual admission, dirty-state, host activation and switch payload validation use the latter. Ordinary/manual history keeps its virtual document-bearing encoded and decoded admission costs and the shared 128 MiB pool. The decoded cost is counted one boundary at a time, so this check does not allocate a complete old-shaped history JSON. This avoids admitting an ordinary stream whose accumulated Undo commands cannot subsequently save its preserved Redo tail at the same cap. Native task/candidate histories leave that pool and remain subject to the raw metadata-plus-decoded ceiling.

## First witness and gates

The slow-tier vertical witness uses the scaled `save_cost_document` fixture with 400 solid clips: one accepted MCP marker edit under an explicit narrow scope, Finish with Freeze, referenced checkpoint resolution with exact full-history and batch identity equality, merge, Stop with independently observed unbound-attempt cleanup, and exact reopen of task and candidate. Candidate history grows by one boundary per merge. The existing `director_scope` convenience helper refuses scopes with more than 256 clips/tracks/other objects. A whole-document scope for 400 clips therefore does not fit that independent policy clamp. The vertical witness uses a valid marker-only scope over the complete 400-clip document; it does not qualify the app’s automatic whole-document scope at that size. The later capacity witness must state its edited subset and scope explicitly. This existing clamp is reported separately, not silently raised.

The vertical witness passes in the current full Agent suite. Its fixture retains two task actors and three durable registrations; the registrations alias task/candidate actors and add no independent document set. The gate includes Core, Project (`test-util`), Agent and App (`slow-tests`) full suites, all-target clippy for those four crates, fmt, slow-test lint, and workspace test compilation. Final gate counts are recorded below.

## Live memory: provisional constants and unresolved envelope

DHAT measures requested heap allocations, not RSS, in the Linux debug/test build. Inputs and original comparison state exist before profiling. The measured restored history and lineage equal the original in full. A subsequent actor restore measures serial replay overlap. These timings include DHAT overhead and are not release open/restore qualification.

| 200-edit fixture | History heap | Lineage heap | Pair heap | Typed pair charge | Peak while restoring actor |
| --- | ---: | ---: | ---: | ---: | ---: |
| 400 clips, flags | 19.366 MiB | 14.257 MiB | 33.624 MiB | 41.319 MiB | 53.402 MiB |
| 400 clips, includes 20 ripple-deletes | 18.940 MiB | 14.132 MiB | **33.072 MiB** | **40.430 MiB** | **52.449 MiB** |
| 1,200 clips, 200 flags, diagnostic above the role cap | 56.894 MiB | 42.919 MiB | 99.813 MiB | 118.106 MiB | 156.849 MiB |

The required row holds 34,678,360 bytes. Its provisional 41 MiB task live bound leaves **19.34% actual-heap headroom**; its conservative typed charge is 42,393,940 bytes. That charge includes command/log allowance and typed document capacities, not just compact bytes. The required row encodes to 182,867 portable-history bytes and 111,706 lineage bytes; the old document-bearing oracle encodes to 12,745,440 history bytes. Serial decode/replay overlap above the retained pair is 20,317,879 bytes (19.377 MiB); after restore the actor shares documents, with 639,703 bytes additional retained actor state. Including that actor state, the required resident task is 35,318,063 bytes (33.682 MiB), leaving **17.85%** actual-heap headroom under 41 MiB. The allocator witness also requires the history charge to cover this actor state. This measured overlap is specific to that fixture, not an all-operations maximum.

| Constant | Provisional value and source |
| --- | --- |
| Task persisted T | 1,111,040 bytes, the accepted design sketch; recomputation from final record/admission measurements remains due. |
| Candidate persisted C | 8T = 8,888,320 bytes, same ruling. |
| Task live | 41 MiB; required 400/200 row above. |
| Candidate live | **2 MiB**, reduced from 4 MiB before this report to preserve total headroom. |
| Document admission | 5,645,312 bytes, accepted design sketch; final fixture table remains due. |
| Standalone portable replay expansion guard | 128 MiB per decode; checked before host bootstrap as well as on decode. This is a pre-materialization guard, not another retained-history allowance or a shared global reservation. |
| Ordinary/manual pool | **128 MiB unchanged**; includes ordinary working branches and legacy agent-session histories. They add no second allowance to the sum. |
| Raw metadata plus non-decoder decoded total | **512 MiB unchanged**. |
| Other existing clamps | 65,536 batches/full-delta operations; 256 KiB command envelope; 8 MiB encoded and 32 MiB decoded run records; 32 MiB ordinary branch history. |

Candidate sizing probe uses one `DoBatch` per merge and real Core/lineage construction, but does not yet execute the full merge lifecycle at each threshold. At 2 MiB: 40 and 120 clips fit two merges of 200 flag operations; 400 fits one; 1,200 fits zero 200-operation merges, or **one 80-operation merge** (1,780,908 bytes typed). At 4 MiB the respective counts were five, five, four, two and three. The smaller candidate budget is provisional pending the first-commit ruling.

| Simultaneous envelope, after candidate reduction | MiB |
| --- | ---: |
| Eight task roles at 41 MiB | 328 |
| Eight candidate roles at 2 MiB | 16 |
| Full ordinary/manual pool | 128 |
| Subtotal before overlap or any other metadata | **472** |
| Measured required-row serial overlap | 19.377 |
| Measured-overlap subtotal | **491.377** |
| Margin against 512, before other metadata/residents | **20.623 (4.03%)** |
| One decoded run record at its existing 32 MiB limit | 32 |
| Subtotal including that record, before stored graph metadata/other residents | **523.377** |
| Margin at that subtotal | **−11.377 (−2.22%)** |
| Alternative serial window: existing typed-read preflight reservation | 64 |
| 472 subtotal + 64 reservation, before run record/other metadata | **536** |
| Margin for that reservation envelope | **−24 (−4.69%)** |
| Same reservation envelope + one maximum decoded run record | **568** |
| Margin before graph/other metadata | **−56 (−10.94%)** |
| Alternative unledgered portable replay cap: 472 + 128 + 32 | **632** |
| Margin at that upper-cap envelope, before other metadata | **−120 (−23.44%)** |

The store additionally reserves its existing 64 MiB decoded-record preflight allowance during typed reads (a ledger allowance, not a measured 64 MiB heap allocation). Using that allowance as the serial decode window gives 536 MiB with the full ordinary pool, or 568 MiB including one maximum decoded run record: margins of −24 MiB and −56 MiB before graph/other metadata. It is sequential with the measured replay in the resolver, so these alternatives are not added to one another.

This is not a qualified global envelope. Stored control/object/edge metadata and ordinary lineage maps are also charged to this same ceiling; the ordinary history pool does not by itself bound those separate lineage allocations. Using a conservative full 41 MiB role-sized replay overlap instead of the measured fixture gap gives 513 MiB even before other metadata. The raw ceiling continues to refuse excess reservations; a small serialized journal is not evidence that every cap can coexist.

The existing project root also embeds its authoritative current document; removing that root-level copy is outside this representation change. The history record adds no inline document copy. Its memory remains part of the other metadata class.

Source audit: `run_admission.rs` refuses a second active run, and the shipped Revise path constructs one native candidate for that run. The eight-candidate row above is the requested conservative envelope. With one active candidate the role-plus-ordinary subtotal is 458 MiB; retained candidates and task actors still need accounting. The host has up to 64 retained run slots; other raw version/object clamps can bind earlier, so 64 slots is a retention upper bound rather than proof that every run can reach every cap. 24 attempts replace authority on the same task actor; they do not create 24 independent histories. Finish and Freeze keep that actor, and attempt retirement does not release it. `HostRun.tasks` retains terminal actors across up to 64 retained runs. Owner durable registrations keep candidate actors. Up to 16 legacy host sessions use the ordinary/manual class, already included in its 128 MiB allowance. Up to four retained GUI rescue actors and frozen input documents add residents. The cache retains the selected manual branch and ordinary working/session histories. Native task/candidate histories are resolved serially rather than cached in full. Compact floor anchors retain only two documents but still require accounting. One queued/running media read per task can retain a current document; a finished media result retains its certified source and sample, not the whole history. Frozen run input contexts also hold a current document. During GUI attachment, the GUI and the host retain manual actors in separate processes; their document allocations do not share an Arc across IPC. The per-owner ordinary ledgers do not yet prove that these replicas together stay inside one global 128 MiB allowance. These document holders must be classified against the role/overlap estimates before the final envelope is qualified. The host command and MCP-host ingress share a one-permit semaphore; task previews use the project replay gate. These serialize their own work, but do not eliminate the retained holders above. The active eight-plus-eight count therefore does not cover every reachable holder.

Recommendation for the constants ruling: keep task 41 MiB and candidate 2 MiB provisionally; use an active-run ordinary-pool allowance below 128 MiB without changing its raw value, and release terminal actor registrations before admitting another run, retaining their persisted histories for review. The minimum ordinary-pool reduction is more than 11.377 MiB for measured replay plus one run record, or more than 56 MiB for the existing preflight-reservation window plus that record, before graph metadata or margin. A 64 MiB active allowance frees 64 MiB but leaves only 8 MiB before graph/other metadata in the latter envelope; it is not a qualified final setting. The final active allowance and any tighter preflight reservation must be qualified together with measured graph metadata, serial-overlap bounds, legacy sessions and rescue holders before claiming 512 MiB/10% margin. The standalone portable decoder also needs its replay charged to the shared overlap/ordinary admission rather than relying only on its 128 MiB amplification guard; the 632 MiB upper-cap envelope above cannot coexist even with an ordinary allowance reduced by 120 MiB before other metadata or margin. No such allowance, aggregate GUI/host ledger or lifecycle change is silently implemented in this delivery; the vertical witness does not depend on the final constants.

## Remaining stage-1 work and scope

Not yet qualified by this first delivery: admitted 400/200 lifecycle, the 1,200-clip admitted edit count/lifecycle, end-to-end task table at 40/120/400/1,200, non-latching candidate refusal lifecycle, complete crash matrix, release open/restore measurements, and truthful replay-failure fallback. The follow-up sections below add the all-operation/pending-save/manual rollback oracle, LUT retention, isolated history capacity, the admitted-size heap and after-change Save measurements. These remain active work, including the final replay-failure commit specified by the brief. The already measured pre-change Save baseline is 272.8 ms median over twelve changed saves at `e0131f02`; no after-change number is claimed here.

Overlap with the other workers: Core model/history/actor registration and Project records/store/publication/manual-history paths. No linear-run-storage implementation or old-reader/migration support is added. `target/` size and final gate counts are recorded below.

Verification uses worktree-local `LOCALAPPDATA` on the durable filesystem. Bundle fixtures are regenerated in the current shape; the three consent-archive generator mutations restore both production successor guards before normal tests. Source restoration hashes and fixture catalog hashes were checked. No old-shape reader or migration is added.

Legacy-session reachability (read-only audit): Studio chat calls `HostChat::start` → `HostCommand::StartSession`, which currently refuses `provider_unqualified` before creating a branch. The same constructor is still reachable from `StartScripted` and `ReopenScripted`; these are ungated variants of the shipping host protocol and are admitted by the MCP `host_command` connector whitelist. No current person-facing UI starts a live session through StartSession; that implementation is disabled. The unpaid scripted session path remains reachable through the shipping authenticated host command endpoint. `DurableTimelineBranch::fork_registered` has only test callers in the current tree: that standalone wrapper is dead product code for removal, and receives no additional memory allowance. No path is removed in this job. Creation requires `DurableBranchRole::Session` or `RunCandidate`; session reopen checks its requested role against the persisted base and checkpoint. Native task seeds separately require their Task role.

Portable-boundary verification: the hosted lost-Save fixture now uses 4,000 unsaved accepted commands, still requiring a real ordinary Save floor, within the portable replay guard. Larger estimated expansion is refused by `HostSnapshot::from_history` before starting the child, using the existing foreground-history capacity error. The focused App witness and full App suite pass on that path.

## First-delivery gate evidence

| Full suite | Passed | Failed | Ignored | Evidence |
| --- | ---: | ---: | ---: | --- |
| Core | 826 | 0 | 2 | `target/history-capacity/verified-full-core.log` |
| Project (`test-util`) | 452 | 0 | 3 | `target/history-capacity/verified-full-project.log` |
| Agent (`slow-tests`) | 1,707 | 0 | 95 | `target/history-capacity/verified-full-agent.log` |
| App (`slow-tests`) | 1,019 | 0 | 5 | `target/history-capacity/verified-full-app.log` |

Fmt, slow-test lint (127 tests, 150 allowlist entries, 27 equipped-session skip sites), all-target clippy `-D warnings` for all four changed crates, and workspace test compilation **PASS**. The complete foreground batch exited 0. Evidence: `target/history-capacity/verified-*.log` and `first-delivery-gates.json`. `target/` is **35.57 GB (33.12 GiB; 35,565,739,976 bytes)**. Windows and paid/provider runs: **NOT RUN**.

## Document-bearing oracle proof

The old document-per-boundary serialization remains test-only in `crates/kinewright-core/tests/support/history_oracle.rs`. Existing valid-operation fixtures execute the same accepted operation directly and through Core, then compact-write/restore and compare the entire original snapshot, reconstructed old serialization, every cursor boundary, identity effects and `applied_operations`. Coverage is collected against the exhaustive 67-operation subject registry. An invalid pre-state or rejected replay fails the witness. No operation is excluded or silently repaired.

The generated witness uses seed `0x71e18a5ef32ab0ac`, 32 sequences and 40 prefixes each. At every prefix the source and resumed actors run the same next command and must export exactly equal snapshots. Cases include Do, DoBatch, coalescing, empty Undo/Redo and new edits after Undo. Explicit cases pin `gesture(k), Undo, Redo, gesture(k)`, the A/B/Undo/C live path, public clip-ID reuse with different content, and corruption of each baseline/current/Undo/Redo hash. The pending-save witness adds a clip and splits it while I/O is pending, then checks both commit and abort, restored identity effects and later Undo/Redo. Manual rollback checks compact roundtrips after tentative execution is refused by encoder or admission; the original independent state/counter/pending/byte assertions remain.

This is test-only evidence. Product code and the provisional budgets are unchanged by this commit. The final gate evidence and target size are recorded below.

## Owned LUT and occurrence retention

`compact_history_redo_keeps_owned_lut_after_collection_reopen_and_save_as` imports a real parsed Cube after initial Save, admits its exact bytes as an owned LUT, registers it and applies `creative_look`. It undoes both commands, then performs four Saves with an empty current-input manifest and collection, drops the import pins and removes the external input. After reopen and Save As, both projects retain a history-bound owned-input manifest whose bytes certify and parse. Redo in both yields the exact original applied document, cursor and live operation path. The focused witness passed in 1.36 seconds and the first full Project run also passed it.

`compact_lineage_keeps_original_occurrences_when_a_clip_id_is_reused` starts with one baseline clip, deletes it, recreates different content under the same public ID and undoes the recreation. The baseline map must retain the original occurrence and the Redo map the new occurrence, including its persisted creation batch. Full map equality survives compact serialization, fresh project open and Redo/Save. This exercises replacement of a map value under an unchanged public key. The full Project run passed it.

## Capacity and heap follow-up

The prospective native task admission guard over the note's fixture admitted the following counts. These are isolated history capacities; approval, run-record and other tool-path clamps are separate. The next count returned the existing actionable history message in every row. The strengthened measurement uses the task's actual retained seed, applies edits through its normal command path, freezes the admitted task and resolves the stored history exactly. It passed all four rows in 31.44 seconds. This qualifies the isolated native history path, not the still-pending complete tool lifecycle.

| Seed clips | Accepted steps | Stored history encoded / decoded bytes | History + lineage live charge bytes | First binding quota |
| ---: | ---: | --- | ---: | --- |
| 40 | 316 | 87,685 / 1,108,483 | 11,111,334 | T = 1,111,040; next decoded charge 1,111,976 |
| 120 | 316 | 87,687 / 1,108,487 | 23,792,182 | T = 1,111,040; next decoded charge 1,111,980 |
| 400 | 203 | 58,430 / 744,162 | 42,967,044 | 41 MiB = 42,991,616; next pair 43,171,729 |
| 1,200 | 68 | 19,180 / 242,105 | 42,806,274 | 41 MiB; next pair 43,419,880 |

These streams are track sync-lock flag batches. The 400-clip row replaces step 101 with twenty tail ripple-deletes. The live charge in this table is for the native seed/capture; the separate restored-heap row below charges the wire reconstruction's capacities. Guard margins are 2,557 / 2,553 bytes of T and 24,572 / 185,342 bytes of the live bound respectively. Typed-charge margin is distinct from the measured heap headroom. Candidate and complete tool-path capacities remain unqualified by these rows. Detailed accepted/next-refused metrics, portable sizes, lineage sizes and replay bounds are in `target/history-capacity/native-task-capacity.json`.

The 1,200/68 restored-heap witness passed. It measured history **20,618,400 bytes**, lineage **15,595,424 bytes**, and actor-only overhead **270,623 bytes**: **36,484,447 bytes (34.794 MiB)** resident, **15.14%** below the provisional 41 MiB task allowance. The restored typed pair charge was **42,849,570 bytes (40.865 MiB)**. Its portable history is 402,737 bytes and lineage is 328,908 bytes. The measured transient peak was 56,637,871 bytes. The serial excess over the restored pair is **19.478 MiB**; using this larger measured excess in the earlier envelope gives **523.478 MiB**, or **11.478 MiB over 512 before other metadata**. It remains unqualified. The required 400/200 row remains 33.682 MiB including its actor, with 17.85% heap headroom. Debug DHAT restore timings are allocation-instrumented diagnostics, not release latency claims.

## Release Save measurement after the representation change

Same `save_cost_measure` harness, release profile, three warm saves and twelve changed saves per point. All saves succeeded and published changed histories; each median includes fourteen fsync calls. Evidence: `target/history-capacity/save-cost-after.log`, `save-cost-after-result.json`, and the prior campaign under `target/run-limit-followups/`.

| Initial edits | Before median ms | After median ms | After range ms | Before / after retained batches |
| ---: | ---: | ---: | --- | --- |
| 100 | 112.0 | 117.1 | 114.3–146.9 | 115 / 115 |
| 400 | 137.2 | 143.1 | 141.5–146.4 | 415 / 415 |
| 1,600 | 250.7 | 275.4 | 273.9–369.7 | 1,615 / 1,574 |
| 3,000 | 272.8 | 275.3 | 273.3–282.0 | 1,690 / 1,574 |

There is no same-history speed-up claim. At the high points the new admission/flooring schedule retains fewer batches, so those timings compare different retained work. At 100 and 400 the batch counts match and medians are 5.1/5.9 ms higher; fsync medians rose by 4.1/2.1 ms. The support-field walk at 3,000 fell from 46.9 to 9.3 ms, while whole-Save time stayed near 275 ms. These are two separate campaigns, not an interleaved performance experiment. The after campaign's load averages were 4.70/4.05/4.74 at start and 4.10/3.95/4.70 at end on twenty logical CPUs. Release open/restore and their breakdown remain pending.

## Follow-up gate evidence

The final foreground batch exited 0. Full **Core: 830 passed, 0 failed, 2 ignored**; full **Project (`test-util`): 455 passed, 0 failed, 3 ignored**. Fmt, slow-test lint (127 / 150 / 27), all-target clippy `-D warnings` for both changed crates, and workspace test compilation **PASS**. Coverage records all 67 operation kinds with no missing or excluded kind. Evidence: `target/history-capacity/all-operation-core.log`, `all-operation-coverage.json`, `proof-full-project.log`, `oracle-clippy.log`, `proof-project-clippy.log`, `proof-final-*.log` and `proof-gates.json`.

`target/` is **35.91 GB (33.44 GiB; 35,906,908,160 bytes)**, including the 77 MB source archive prepared for the remaining restore comparison. Windows and paid/provider runs: **NOT RUN**. This follow-up changes tests and this report; product behavior and provisional constants remain those of the vertical delivery. The complete native tool lifecycle, candidate refusal, release restore/open and final replay-failure recovery remain active work.
