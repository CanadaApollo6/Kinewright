# History capacity: stage 1

Stage 1 stores and transports a compact journal over referenced documents while retaining Core's existing boundary documents in memory. The first vertical delivery is `dfde093a`; the complete operation oracle is `3a987c5f`. The capacity follow-up adds native merge admission before staging and separate retained/replay charges. Final simultaneous-cap constants and truthful replay-failure recovery remain outstanding. No raw storage limit is raised. Work is local on `te1-run-limit` from `e0131f02`; nothing is pushed.

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
| 400 clips, includes 20 ripple-deletes | 18.940 MiB | 14.132 MiB | **33.072 MiB** | **40.442 MiB** | **52.449 MiB** |
| 1,200 clips, 200 flags, diagnostic above the role cap | 56.894 MiB | 42.919 MiB | 99.813 MiB | 118.106 MiB | 156.849 MiB |

The required row holds 34,678,360 bytes. Its provisional 41 MiB task live bound leaves **19.34% actual-heap headroom**; its conservative typed charge is 42,406,084 bytes. That charge includes command/log allowance and typed document capacities, not just compact bytes. The required row encodes to 182,867 portable-history bytes and 111,706 lineage bytes; the old document-bearing oracle encodes to 12,745,440 history bytes. Serial decode/replay overlap above the retained pair is 20,317,879 bytes (19.377 MiB); after restore the actor shares documents, with 639,703 bytes additional retained actor state. Including that actor state, the required resident task is 35,318,063 bytes (33.682 MiB), leaving **17.85%** actual-heap headroom under 41 MiB. The allocator witness also requires the history charge to cover this actor state. This measured overlap is specific to that fixture, not an all-operations maximum.

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

The initial candidate sizing probe used one `DoBatch` per merge and real Core/lineage construction. The later native lifecycle table supersedes this isolated estimate. At 2 MiB: 40 and 120 clips fit two merges of 200 flag operations; 400 fits one; 1,200 fits zero 200-operation merges, or **one 80-operation merge** (1,780,908 bytes typed). At 4 MiB the respective counts were five, five, four, two and three. The smaller candidate budget is provisional pending the first-commit ruling.

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

Recommendation for the constants ruling: keep the provisional 41/2 MiB role bounds and the ordinary pool's full 128 MiB value, but release checkpointed task actors and their durable registrations after attempt retirement, and release inactive candidate actors when a run settles. Rehydrate them from the exact saved journal when work resumes or a frozen result needs review/merge. `reconcile_native_effect` already persists runtime task work before publishing retirement, and `validate_projection` admits at most three unretired/reserved attempts. Three unretired actors plus one serially restored task require a four-slot resident-Core rule. Including that temporary task gives a role/pool/overlap subtotal of **4 × 41 + 8 × 2 + 128 + 64 + 32 = 404 MiB**. That leaves 108 MiB before other metadata, or at most 56.8 MiB of that class while retaining a 10% total margin. This is a proposed lifecycle/admission change, not a claim about today's retained actors. It needs explicit removal of both Core owners, slot checks on restoration and proof that dirty retired work reopens exactly. The raw 512 MiB ledger must also account for the resident role reservations and other metadata before the total can be qualified. This policy is not implemented pending the lead's ruling.

The current eight-role envelope above remains unqualified. The standalone portable decoder also needs its replay charged to shared overlap/ordinary admission rather than relying only on its 128 MiB amplification guard. Its 632 MiB upper-cap envelope cannot coexist with all other allowances. No active ordinary allowance, aggregate GUI/host policy or lifecycle eviction is silently introduced in this delivery; the vertical witness does not depend on final constants.

## Remaining stage-1 work and scope

The vertical lifecycle, all-operation replay oracle, LUT retention, isolated task capacities, restored-heap witnesses and before/after Save/restore measurements are complete. The capacity follow-up below records the full native lifecycle and non-latching candidate refusal. The final replay-failure fallback and the simultaneous 512 MiB envelope remain outstanding. The compatibility merge will use `e160e419`, including the incoming strict journal-command shape and CI-only mirror fix.

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

The prospective native task admission guard over the note's fixture admitted the following counts. These are isolated history capacities; approval, run-record and other tool-path clamps are separate. The next count returned the existing actionable history message in every row. The strengthened measurement uses the task's actual retained seed, applies edits through its normal command path, freezes the admitted task and resolves the stored history exactly. It passed all four rows in 32.27 seconds. This qualifies the isolated native history path, not the still-pending complete tool lifecycle.

| Seed clips | Accepted steps | Stored history encoded / decoded bytes | History + lineage live charge bytes | First binding quota |
| ---: | ---: | --- | ---: | --- |
| 40 | 316 | 87,685 / 1,108,483 | 11,252,768 | T = 1,111,040; next decoded charge 1,111,976 |
| 120 | 316 | 87,687 / 1,108,487 | 23,933,616 | T = 1,111,040; next decoded charge 1,111,980 |
| 400 | 202 | 58,154 / 740,669 | 42,813,477 | 41 MiB = 42,991,616; next pair 43,017,433 |
| 1,200 | 68 | 19,180 / 242,105 | 42,849,570 | 41 MiB; next pair 43,462,455 |

These streams are track sync-lock flag batches. The 400-clip row replaces step 101 with twenty tail ripple-deletes. The live charge includes the native seed/capture and the capacity retained by a fresh replay. The persisted history and lineage are freshly decoded and checked together against the role limit. Guard margins are 2,557 / 2,553 bytes of T and 178,139 / 142,046 bytes of the live bound respectively. Typed-charge margin is distinct from the measured heap headroom. These counts isolate the history guard. In the full tool path the existing 256-approval cap can bind before 316 edits at 40/120 clips; this table does not claim 316 model-approved edits. The later lifecycle and candidate tables qualify their stated paths. Detailed accepted/next-refused metrics, portable sizes, lineage sizes and replay bounds are in `target/history-capacity/native-task-capacity.json`.

The 1,200/68 restored-heap witness passed. It measured history **20,618,400 bytes**, lineage **15,595,424 bytes**, and actor-only overhead **270,623 bytes**: **36,484,447 bytes (34.794 MiB)** resident, **15.14%** below the provisional 41 MiB task allowance. The restored typed pair charge was **42,849,570 bytes (40.865 MiB)**. Its portable history is 402,737 bytes and lineage is 328,908 bytes. The measured transient peak was 56,637,871 bytes. The serial excess over the restored pair is **19.478 MiB**; using this larger measured excess in the earlier envelope gives **523.478 MiB**, or **11.478 MiB over 512 before other metadata**. It remains unqualified. The required 400/200 row remains 33.682 MiB including its actor, with 17.85% heap headroom. Debug DHAT restore timings are allocation-instrumented diagnostics, not release latency claims.

## Release Save measurement after the representation change

Same `save_cost_measure` harness, release profile, three warm saves and twelve changed saves per point. All saves succeeded and published changed histories; each median includes fourteen fsync calls. Evidence: `target/history-capacity/save-cost-after.log`, `save-cost-after-result.json`, and the prior campaign under `target/run-limit-followups/`.

| Initial edits | Before median ms | After median ms | After range ms | Before / after retained batches |
| ---: | ---: | ---: | --- | --- |
| 100 | 112.0 | 117.1 | 114.3–146.9 | 115 / 115 |
| 400 | 137.2 | 143.1 | 141.5–146.4 | 415 / 415 |
| 1,600 | 250.7 | 275.4 | 273.9–369.7 | 1,615 / 1,574 |
| 3,000 | 272.8 | 275.3 | 273.3–282.0 | 1,690 / 1,574 |

There is no same-history speed-up claim. At the high points the new admission/flooring schedule retains fewer batches, so those timings compare different retained work. At 100 and 400 the batch counts match and medians are 5.1/5.9 ms higher; fsync medians rose by 4.1/2.1 ms. The support-field walk at 3,000 fell from 46.9 to 9.3 ms, while whole-Save time stayed near 275 ms. These are two separate campaigns, not an interleaved performance experiment. The after campaign's load averages were 4.70/4.05/4.74 at start and 4.10/3.95/4.70 at end on twenty logical CPUs. Release open/restore comparisons and their breakdown follow below.

## Follow-up gate evidence

The final foreground batch exited 0. Full **Core: 830 passed, 0 failed, 2 ignored**; full **Project (`test-util`): 455 passed, 0 failed, 3 ignored**. Fmt, slow-test lint (127 / 150 / 27), all-target clippy `-D warnings` for both changed crates, and workspace test compilation **PASS**. Coverage records all 67 operation kinds with no missing or excluded kind. Evidence: `target/history-capacity/all-operation-core.log`, `all-operation-coverage.json`, `proof-full-project.log`, `oracle-clippy.log`, `proof-project-clippy.log`, `proof-final-*.log` and `proof-gates.json`.

`target/` is **35.91 GB (33.44 GiB; 35,906,908,160 bytes)**, including the 77 MB source archive prepared for the remaining restore comparison. Windows and paid/provider runs: **NOT RUN**. This follow-up changes tests and this report; product behavior and provisional constants remain those of the vertical delivery. The complete native tool lifecycle, candidate refusal, release restore/open and final replay-failure recovery remain active work.


## Release restore comparison and breakdown

The common harness reconstructs the same 400/200-ripple and 1,200/68 histories, retains one Redo boundary, then checks the restored history and future Redo. Five alternating release samples per revision compare `e0131f02` with the current representation. The resulting documents are byte-identical. Executables are copied immediately after each build, have different SHA-256 hashes, and the compact/old byte-size ratio is asserted. An initial attempt reused the same Cargo artifact across source archives; it was rejected, marked invalid and excluded. Rebuilding Core separately for each side supplied the valid comparison below. Other crates' existing debug gates were unaffected.

| Same history | Old portable bytes | New portable bytes | Old decode / actor restore median ms | New decode / actor restore median ms | Old / new combined actor-ready median ms |
| --- | ---: | ---: | --- | --- | --- |
| 400 clips / 200 edits / one Redo | 13,156,733 | 182,937 | 34.396 / 35.348 | 52.198 / 55.149 | 70.709 / 107.347 |
| 1,200 clips / 68 edits / one Redo | 13,871,672 | 402,805 | 30.486 / 31.525 | 49.859 / 54.049 | 61.311 / 102.557 |

Combined actor-ready time regressed **51.8% / 67.3%**. Its sample ranges were 67.882–75.821 versus 103.786–210.632 ms at 400 clips, and 60.849–103.378 versus 100.229–198.378 ms at 1,200. Portable deserialization now performs verified replay; the older parser materializes the supplied boundary documents. Actor restoration subsequently verifies again. These figures are Core transport/restore, **not Project open latency**.

The follow-on release breakdown measures the source of the regression before any optimization. At 400 clips, median journal capture/hash time is 36.811 ms, transport parsing 0.397 ms, replay-size estimation 0.141 ms, verified journal replay 47.277 ms and ordinary channel command replay 11.816 ms. At 1,200 they are 38.854, 0.781, 0.087, 49.114 and 11.992 ms respectively. Repeated full-document hashing and verified replay dominate; parsing the compact shape does not. No hash, boundary or replay check has been removed to improve these numbers. Evidence: `restore-comparison.json`, distinct binary/source hashes, `restore-before.log`, `restore-after.log`, `restore-breakdown.log` and `restore-breakdown.json` under `target/history-capacity/`.


## Native lifecycle and candidate admission

The complete 400-clip/200-edit row (including the twenty-ripple-delete batch and one retained Redo) passed the real prepare/commit tool path, Finish's durable Freeze, candidate merge, Stop with independent unbound-attempt cleanup, abrupt subprocess exit without destructors, exact fresh open and Redo. The full-suite attempt recorded 319.487 seconds for that row. Finish performs Freeze itself; its original applied Freeze receipt and the frozen checkpoint are checked. A fresh standalone Freeze followed by Finish is not a valid lifecycle sequence in the existing task state machine.

The first full-suite attempt passed the 400-clip row but failed at the 1,200-clip merge and in the candidate threshold test. It completed with 1,322 passed, two failed and 58 ignored lib tests. Inspection found two product errors. Native replay used the candidate's 2 MiB retained allowance as its transient ceiling. Also, `bind_merge_receipt` published a caller binding and `EditorTask::prepare_merge` retained an envelope before candidate admission. The change separates the replay reservation from the retained allowance and runs pure candidate admission before either state is staged, with another check before commit. No role allowance or raw limit is raised.

The native candidate test passes all four rows in the final full Agent run. A prior focused run passed in 386.26 seconds; the table below uses the final replay-capacity estimator. Each task is built with accepted Core commands and checked against its registered native admission before Finish. The separate full lifecycle test exercises every prepare/commit tool call. The first 400-clip task includes the twenty-ripple-delete batch. Subsequent tasks start from that candidate's smaller current document. Finished attempts are retired with independently observed cleanup before assigning more work, respecting the three-unretired-attempt cap.

| Seed clips | Operations per task | Accepted merges | Accepted history + lineage charge bytes | Next proposal charge bytes | Candidate limit bytes |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 40 | 200 | 2 | 1,744,256 | 2,589,511 | 2,097,152 |
| 120 | 200 | 2 | 1,895,248 | 2,780,407 | 2,097,152 |
| 400 | 219 first, then 200 | 1 | 1,457,789 | 2,472,196 | 2,097,152 |
| 1,200 | 68 | 1 | 1,970,047 | 2,856,380 | 2,097,152 |

Every refused proposal returns the existing candidate-history message. The witness checks unchanged candidate checkpoint, live Core history/revision, frozen result and run projection apart from existing refused-call usage accounting. There is no run latch, pending command, caller binding or staged merge envelope. Fresh dirty siblings can Finish and Freeze, and Stop settles with cleanup. Accepted replay peaks are 2,609,836 / 2,742,892 / 1,959,065 / 2,147,805 bytes respectively; they are charged separately from the retained role. Evidence: `target/history-capacity/capacity-full-agent.log` and `native-candidate-capacity.json`.

The corrected release lifecycle passes at both sizes, including the 1,200-clip merge and fresh reopen. The final full-suite gate batch passed; counts are recorded below.

## Release ordinary-project open comparison

A separate release harness creates the requested 400/200-ripple and 1,200/68 histories, retains Redo, performs the production ordinary Save and repeatedly opens a fresh owner. Both revisions floor at the same retained cut under the existing ordinary admission rule. Each open reproduces its saved history exactly; both sides have identical document hashes and retained counts. One warm open is excluded, followed by five samples. Core, Media and Project were cleaned and rebuilt for each side; copied executable hashes differ. This is **ordinary open at the floored cuts, not full native-task open**.

| Seed / requested edits | Both sides: batches / floor offset / Undo / Redo | Before median ms (range) | After median ms (range) |
| --- | --- | --- | --- |
| 400 / 200 including ripple batch | 27 / 174 / 25 / 1 | 214.529 (204.154–223.826) | 59.925 (59.305–73.676) |
| 1,200 / 68 | 8 / 61 / 6 / 1 | 229.867 (228.859–244.589) | 91.430 (90.707–107.742) |

The portable full-history Core restore benchmark above regresses while these smaller, persisted ordinary-project opens improve. Neither number substitutes for the full native lifecycle's open measurement. Evidence: `target/history-capacity/open-comparison.json`, `open-before.log`, `open-after.log`, `open-*-binary.json` and build logs. These measurements compare the archived `e0131f02` baseline with the compact representation before the compatibility merge, which remains pending.

Budget correction: the 40-clip native proposal refused at its second 200-operation merge retained **1,499,933 bytes** (history 1,457,625; lineage 42,308), but its conservative replay peak was **2,609,836 bytes**. Reusing the 2 MiB retained allowance as a transient replay ceiling caused that refusal. Native replay now uses the existing 64 MiB decoded-record window, independently of the unchanged 41/2 MiB retained roles; ordinary/manual replay limits stay unchanged. The raw 512 MiB ledger is still charged and the project replay gate serializes this work. Publication transport decoding, namespace/store resolution, rescue checks and native floor-anchor replay use that distinction. Post-replay retained state is checked separately, including lineage during graph admission. Prospective native admission also includes baseline document-array padding, batch/cursor capacities retained at their replay peaks, and coalesced-operation growth; maximum-admitted task histories are tested through staged persistence and fresh history/lineage decoding. The Agent lifecycle separately tests published checkpoints through fresh open.

With the full 128 MiB ordinary pool, the conservative table remains **328 + 16 + 128 + 64 + 32 = 568 MiB before other metadata**, not a qualified 512 MiB total. Other retained metadata includes frozen task deltas, specialist-base full-applied-delta vectors, batch/input references and pins, alongside the already listed actor/document holders. No final active-run ordinary allowance or aggregate GUI/host policy is introduced without the lead's constants ruling. The full candidate and crash/reopen capacity witnesses and final gates pass. The total envelope remains unqualified.


The first strengthened capacity gate caught a fixture mistake: a generic branch Save after task staging applies ordinary flooring. Native Finish instead pairs its staged task checkpoint with the H1 root transaction. The matrix now checks staged record decoding at the correct boundary; the Agent lifecycle tests actual H1 publication and fresh open. The fresh-decode check also exposed a real undercharge at 400 clips/203 edits: captured history plus lineage was 42,967,044 bytes, but decoded history plus lineage was 43,005,817, above the unchanged 42,991,616-byte role limit. The guard now covers replay's spare batch/cursor capacities, their earlier peaks after Undo, and growth of coalesced operation vectors. A Core regression checks the forecast against actual decoded typed charges after 203 commands, 190 Undo commands and a coalesced gesture growing from twenty operations to twenty-one. The resulting maximum is 202 at 400 clips; the required 200 still fits. No storage limit changed.


## Release native-task crash and reopen

The corrected full native lifecycle passes in release at 400/200 and 1,200/68. Every edit uses the prepare/commit tool path. The next 1,200-clip edit is refused before mutation with the existing task-history message. The task then retains one Redo entry, Finish supplies its durable Freeze, the candidate accepts the delta, and Stop settles with independent unbound-attempt cleanup. A child process exits with code 86 while its writer and Cores are still live. Fresh owners recover exactly the published task and candidate documents, batch IDs and complete histories; Redo reproduces the retained boundary document.

Each row takes six fresh opens. The first is excluded as warm-up; the table gives the remaining five samples. The admitted-history Undo command makes the final batch counts 201 and 69. The measured region is `Ja1EditingSession::open`; independent history and Redo comparisons follow it.

| Seed clips / edits | Full native open median ms | Range ms | Complete child lifecycle + six opens seconds |
| --- | ---: | --- | ---: |
| 400 / 200 | 336.609 | 322.353–354.850 | 60.336 |
| 1,200 / 68 | 521.376 | 514.581–529.723 | 36.086 |

These are after-change full native histories, with no before-change native-open speed claim: the original admission could not persist these histories. The separate same-history Core restore and same-floored-cut ordinary-open comparisons above supply before/after timings. The release run used four build jobs on twenty logical CPUs; host load before execution was about 6.9/8.6/8.1. Evidence: `target/history-capacity/capacity-release-lifecycle.log` and `native-open-release.json`. The test passed in 96.42 seconds.

At the two merge cuts the store ledger reported metadata 2,812,388 / 5,374,781 bytes and decoded 5,496,979 / 12,250,162 bytes, with decoded peaks 100,083,084 / 108,964,986. Each fixture retained two task actors and three durable registrations. These counters are ledger reservations, not whole-process heap measurements. In particular, native Core boundary documents remain resident outside the store's snapshot cache; their individually checked role allowance must still be included in the resident-envelope sum. Native lineage maps are resolved serially rather than retained in that cache, except when the branch is selected as manual history. The allocator figures and role-count audit, not these small ledger values alone, determine the unfinished 512 MiB qualification.


## Capacity follow-up gate evidence

The final foreground gate batch and final warm Agent clippy check both exited 0. Full **Core: 831 passed, 0 failed, 2 ignored**; **Project (`test-util`): 455 passed, 0 failed, 3 ignored**; **Agent (`slow-tests`): 1,710 passed, 0 failed, 95 ignored**. Fmt, all-target clippy `-D warnings` for all three changed crates, slow-test lint (129 tests / 150 allowlist entries / 27 equipped-session skip sites), and workspace test compilation **PASS**. The separate release crash/reopen witness also passes. Evidence: `target/history-capacity/capacity-*.log`, `capacity-gates.json`, `native-task-capacity.json`, `native-candidate-capacity.json` and `native-open-release.json`.

`target/` is **36.83 GB (34.30 GiB; 36,832,864,131 bytes)**. Windows and paid/provider runs: **NOT RUN**. This commit closes the admitted-size native lifecycle and non-latching candidate refusal proofs. The requested merge of `e160e419`, final truthful history-replay recovery, and the lead's final resident-memory ruling remain next. No background build or test is left running at this commit boundary.

## Current-format merge

Merged `e160e419d8a952a21d331ce85f197ecd6d8bd16c` in `06569425241e2e98a6304de2187f60be715d799a`. Compact history uses the current tagged journal command. All new persisted and transported records reject unknown fields; the always-written nullable `split_parent` also rejects omission in typed decoding and the compiled support schema. The document-bearing encoding remains test-only. Current writer output replaces the affected bundle and regression archives; all three temporary fixture-generation source changes were independently checked as restored byte for byte. No old-shape reader or migration was added.

These measurements supersede the earlier capacity and heap rows. T, C, D and the role limits have not changed. Admission is the bounded JSON record estimate, not measured heap. The task rows measure history capacity; a real run can meet other count quotas first.

| Clips | Task edits | H1 bytes | H1 admission | Accepted live pair | Next refused live pair | Candidate merges | Operations per merge |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 40 | 295 | 87,778 | 1,107,678 | 10,177,855 | 10,211,685 | 3 | 200 |
| 120 | 295 | 87,780 | 1,107,682 | 22,020,719 | 22,094,453 | 2 | 200 |
| 400, including 20 ripple-deletes | 203 | 62,490 | 794,100 | 42,797,667 | 43,001,176 | 1 | 200 |
| 1,200 | 68 | 20,540 | 258,833 | 42,762,995 | 43,375,427 | 1 | 68 |

The small task rows refuse on T: the next H1 admission charges are 1,111,419 and 1,111,423 bytes. The larger rows refuse on the 41 MiB live bound. At 400/200 with ripple-deletes the typed pair charge is 42,187,677 bytes. Candidate accepted live charges are 2,069,717 / 1,547,956 / 1,266,613 / 1,909,093 bytes; the next charges are 2,742,470 / 2,260,613 / 2,108,518 / 2,736,761 bytes. Every candidate row checks the actionable non-latching refusal, unchanged candidate/result, and continued Finish, Freeze and Stop.

| Required native row | History heap | Lineage heap | Additional actor heap | Resident MiB | Headroom under 41 MiB | Measured serial excess over pair |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 400/200, ripple-deletes | 19,484,776 | 14,818,624 | 257,135 | 32.959 | 19.61% | 18.676 MiB |
| 1,200/68 | 20,479,584 | 15,595,424 | 106,791 | 34.506 | 15.84% | 19.213 MiB |

The full native 400/200 and 1,200/68 lifecycles pass through Finish/Freeze, merge, Stop, process death, exact fresh reopen and Redo. Current debug opens were 1,858.997 ms and 4,830.439 ms in the initial merged suite; these are after-only debug observations, not new release latency qualification. The earlier release comparison remains scoped to its recorded pre-merge revision.

The aggregate ceiling remains **unqualified**: 328 + 16 + 128 + 64 + 32 = 568 MiB before other metadata and the extra resident holders already audited above. Ordinary/manual and reachable legacy-session histories remain in the unchanged 128 MiB pool. No raw limit or final aggregate policy changed. The proposed retired-actor eviction/rehydration policy remains a recommendation awaiting the lead's ruling.

The first merged Agent gate exposed two test-fixture assumptions: the 40-clip candidate count rose from two to three, and a recovery-close pressure fixture's fixed 512-byte allowance no longer fit the tagged Undo/Redo pair. The count assertion now records three. The pressure fixture derives a narrowed test budget from the current recovery-close encoding while still requiring a floor and a fitting close; product limits and assertions about the published shared floor are unchanged. The corrected pressure witness passes.

Merge gates pass: fmt, workspace build, workspace all-target clippy with Project `test-util` and all slow features, slow-test lint (129 markers / 150 allowlist entries / 27 equipped-session skip sites), and workspace test compilation. Full suites: Core 801 passed / 2 ignored across 22 targets; Media 1,270 / 34 across 6 targets, serial slow; Project 451 / 3 across 23 targets with `test-util`; Agent 1,708 / 95 across 30 targets with slow features; App 1,011 / 5 in its slow-enabled suite. All qualified targets have zero failures. The passing Agent library and binary targets were unchanged by the integration-only pressure-fixture correction; all 23 integration targets and doc tests were rerun after that correction. Failed earlier fixture runs remain in separately named evidence logs and are not counted as passes.

At this boundary `target/` is 29,647,493,489 bytes. The idle cleanup before fixture regeneration reduced it from 36,832,864,131 to 6,380,474,026 bytes; cited logs were preserved. No Cargo run was interrupted. Windows execution, ignored equipped-provider tests and new paid runs were not performed. Replay-failure recovery remains the final, subsequent stage-1 change; this commit does not claim it is implemented.

## Replay failure recovery

The app's Studio open path now offers a verified checkpoint when that branch's history is corrupt or fails replay. The checkpoint document opens with an empty history and the persisted current occurrence map. A typed `UndoHistoryUnavailable` disclosure names the failed history reference and cause. The view is read-only and offers **Save a recovered copy**; the resulting project opens through the ordinary strict loader and can activate Studio. This is a checkpoint cut, not a claim to recover a later uncheckpointed document.

`VerifiedCheckpointRecovery` owns the source namespace and independently verifies the checkpoint document, current lineage, current input manifest and selected input closure. Its lineage reader uses the exact current wire shape, retains current occurrences and ancestry, and does not rebuild unavailable Undo/Redo maps. Only resources reachable from that checkpoint document are copied; history-only resources stay in the original project. The borrowed SHA index and visited/found keys have a bounded temporary reservation. No failed history is returned from `resolve_history`, installed in a verified cache, or admitted to a native run.

Recovery publication accepts only a fresh destination under the existing pinned parent and alias guards. It streams verified blobs and admits typed chunk tables, rebinds the current input manifest to the new owner, and publishes an empty ordinary history with an explicit lineage seed. It rechecks the original root before publication. The original root and failed history remain untouched, including after destination saves, collection, close and reopen. An uncertain publication names its destination; it does not claim a definite failure before publication.

The app recovery journal keeps its distinct contract. A successful prefix can be restored, but its failed suffix is not consumed or deleted. The selected restore carries its damage facts into the app, which discloses the recovered prefix, unrecovered byte range and retained file path. The full source journal survives Save, close and restart; startup offers and discloses it again. Explicit Discard retains the existing directory, file identity and byte-digest checks.

| Behavior | Local evidence |
| --- | --- |
| Well-hashed history with a bad boundary hash or rejected command | Exact checkpoint/current occurrences recovered into a fresh project; new Undo/Redo empty; original bytes retained; strict source open still refuses |
| Damaged checkpoint document | Recovery refuses; no fallback from unverified document bytes |
| Unusable old lineage Undo maps | Current occurrences recovered without replay; unknown fields still refuse |
| Damaged resource unreachable from the checkpoint | New copy succeeds without reading it; original resource retained |
| Person-facing open, Studio refusal and rescue | Actual app handlers produce an editable fresh Studio project; owned WAV probes/opens after deleting its original locator; occurrences unchanged |
| Journal prefix followed by a rejected command and later work | Only the prefix restored; complete original bytes kept after Save/close; damage and prefix rediscovered on restart |

No persisted compatibility reader, migration, raw limit increase or in-memory replay-based Undo was added. Ordinary and native healthy histories still restore through the same Core executor and keep their existing boundary documents. Legacy-session budget ownership is unchanged. The aggregate 512 MiB envelope remains unqualified. The lead finalized the stage-1 role constants and authorized settled-run release as the next separate change; the raw-refusal limitation below is accepted for stage 1 under the later lead ruling.

The final recovery gates below supersede the earlier gate-ordering runs.

## Raw-ledger refusal audit and pre-commit pause

The final stage-1 constants are task live **41 MiB**, candidate live **2 MiB**, ordinary/manual history **128 MiB**, native replay through the existing **64 MiB decoded-record window**, and the unchanged raw **512 MiB metadata-plus-decoded ledger**. Ordinary/manual includes the reachable legacy agent-session path, whose history grows per edit. The resident envelope is **NOT QUALIFIED: 328 + 16 + 128 + 64 + 32 = 568 MiB before other metadata**. The holder audit above still applies: up to 64 retained `HostRun`s keep terminal task actors, owner durable registrations and inactive candidate actors; checkpointing, Finish, Freeze and retirement do not release them. No bound was raised. Settled-run release is the next separately authorized change, followed by the four-slot in-run policy and resident raw-ledger charging; none is implemented in this recovery change.

**The pre-commit audit first paused recovery for a latching publication refusal. The later lead ruling below accepts it for stage 1.** The ledger itself returns `BundleError::Capacity("metadata and decoded bytes")` at `storage_budget.rs:353`. Native publication's `store.rs:906-930` routes it through `run_admission.rs:183-193`. That conversion excludes person and cleanup admissions, but does not exclude terminal admission. It sets the run latch and returns `BundleError::RunLimit`. The host exposes code **`run_limit`**, message **`Run limit reached. Stop this run to finish cleanup.`** It is typed, but it is not non-latching.

A focused unpaid native-run probe reaches the actual terminal record publication and injects retained-read ledger reservations immediately before its 64 MiB write preflight. The diagnostic records `terminal=true`, `cleanup=false`, `raw_limit=536870912`, and refusal of `decoded=67108864`. After every injected reservation is released, the run is still Pending and its task Core history is unchanged, but both `finish_task` and `freeze_task` return that same `run_limit` message with `is_error=true`. Stop can subsequently publish, reconcile both exact retirement proofs and settle cleanup; the persisted dirty task history equals the pre-refusal history in full. This is **a sticky Finish/Freeze limitation, not an unrecoverable Stop or observed work loss**. It is reported before the commit because the requested continued Finish/Freeze guarantee cannot be stated for this site. No fix or admission-policy change has been made.

The injection simulates a retained reader acquiring raw ledger capacity between publication admission and reservation, which share a concurrent ledger. It is not a measured 512 MiB heap workload and does not establish that this month's ordinary workload naturally reaches that pressure. In eight separate stationary-pressure cases, raw reads refused before publication instead; those refusals did not latch. For the five cases whose Stop was refused, releasing the retained reservations allowed Finish with its automatic Freeze, then Stop and exact dirty-history cleanup. The three cases with sufficient headroom applied Stop directly. All eight corrected cases and the publication-race observation completed; the initial control-preparation and duplicate-Stop probe failures are excluded and retained separately.

| Native site | Raw reservation and exposed refusal | Typed / non-latching | Finish, Freeze, Stop and retained work |
| --- | --- | --- | --- |
| Task/candidate record and document decode, including cold branch restore | `read_copy_record` at `store.rs:2863-2925` uses `reserve_copy_read` at `4035-4080`; an active native run receives `run_limit` and the exact message above | Yes / yes, at this adapter; it does not call `note_run_limit` | Failed cold restore does not register the new actor or replace the saved journal. Read-pressure native controls recovered after releasing pressure in the probe. The task/candidate entry-point classification is from the shared adapter and rollback code, not separate injections at every caller. |
| Native history replay, checkpoint capture, anchor and retained base capture | `history_record.rs:162-227`, `store.rs:404-412,2677-2718` use raw retained-read reservations; active native refusal is `run_limit` with the same message | Yes / yes, at these reservations | Replay/capture refusal keeps the existing Core and journal. Task freeze reinserts its runtime on error; candidate capture resumes the fenced cut before returning. Pure pre-edit and proposed-merge admission retains the separate actionable task/candidate-history words and does not stage a merge. |
| Ordinary native record publication | `store.rs:906-930,2770-2840` calls `run_error`; its earlier ordinary clamp is 256 MiB, rather than the raw 512 MiB limit | Yes / **no**: `run_limit`, same message | Ordinary continuation including Finish/Freeze can be refused until Stop. This is distinct from a raw read refusal. Stop's terminal admission and proof-bearing cleanup remain available; the prior C3 dirty-task latch witnesses cover preservation. |
| Terminal native publication, including Stop's H1 cut | The same `reserve` adapter uses the raw 512 MiB ledger under terminal admission | Yes / **no**, reproduced: `run_limit`, same message | After pressure is released, Finish/Freeze stay refused. Stop retry plus proof-bearing cleanup succeeds and keeps the dirty history exactly. This is the reported pre-commit gap. |
| Proof-bearing cleanup publication and its reads/replay | Raw 512 MiB ledger with `CleanupAdmission`; both `run_error` and the read adapter exclude this scope from the latch conversion | Yes / yes: host code **`storage_backpressure`**, message **`storage_backpressure: metadata and decoded bytes`** | A refusal retains the actor, dirty work, saved root and retirement proof for retry. Cleanup after the publication-race refusal completed with exact history in the focused probe. Refusal at each individual cleanup substep was code-audited, not separately injected. |
| Native-open namespace decode/replay before a run is active | `resolve_namespace_history` and namespace record reservations have no active native run conversion | Yes / yes: **`storage_backpressure`**, same capacity message | Capacity does not authorize the recovery fallback or dropping Undo. Source bytes stay on disk; open can retry after capacity becomes available. Only independently verified corruption authorizes the checkpoint rescue described above. |

The unchanged 64 MiB single-replay-window refusal is a separate `Capacity("history replay memory")`, exposed as `storage_backpressure: history replay memory`; it does not set the raw ledger latch. The task/candidate role checks return their existing actionable `run_limit` words before mutation. None of these errors is silently treated as successful replay.

Evidence is under `target/history-capacity/`: `raw-ledger-audit.log` has the six corrected pressure cases, `raw-ledger-publication-audit.log` has the two finer pressure cases, and `raw-publication-race-audit.log` records the actual raw publication refusal and subsequent controls. `raw-ledger-audit-instrumentation.patch` preserves the test-only probe and diagnostics. Both instrumented source files were restored byte-exact to their pre-probe SHA-256 values in `raw-audit-source-hashes.txt`; the probe is not part of the product diff.

Recovery gates so far: fmt, workspace all-target clippy with Project `test-util` and all slow features, slow-test lint, and all three `--no-default-features` shipping builds **PASS**. Full Project with `test-util`: **455 passed, 0 failed, 3 ignored**, across 23 targets. Both new app recovery witnesses pass. The full App rerun has **1,011 passed, 2 failed, 5 ignored** because the shipping host built without fixture providers replaced the test host: H4 reports `provider_wait: unknown connector provider`; Studio end-to-end does not obtain its synthetic linked run. Its earlier missing-host setup run is also retained separately. A fixture-enabled host rebuild and full App rerun are still required. The current release six-open lifecycle measurement and workspace test compilation were not started after the App gate stopped. They are **NOT RUN** for this recovery diff; the previously recorded merged-tree gates remain scoped to their commits.

At this pause `target/` is **15,711,028,816 bytes**, down from **29,649,346,827 bytes** before the idle package cleanup; immediately after cleanup it was **9,673,424,314 bytes**. Small evidence logs were kept. No Cargo process was interrupted. At that audit pause, recovery and the report were uncommitted. No push was made, and settled-run release had not started.

The lead's subsequent upstream notice queues a real merge of `e6893f44d924b369f857b42916e6d0c85b31fab0` at the next commit boundary. It has not been merged during this pre-commit stop. That merge must preserve `KeptEditCoverage.position = floor_offset + batches.len()` from the accepted captured cut, use the new fourth `persisted_history` return value, and pass the C4b witnesses on the merged tree. Its `scripts/ci-shipping-builds.sh` becomes the shipping gate and must run without an outer lock. Read-only inspection of the upstream diff shows no change to the terminal `run_error` conversion described above; no runtime claim is made for the unmerged tree.

## Lead ruling and recovery completion

The lead ruled that the sticky publication latch is **not a stage-1 defect**: Stop settles cleanup and the dirty history is retained exactly. **Finish and Freeze are NOT guaranteed after a raw-ledger refusal of an ordinary or terminal native publication; Stop and exact retention are.** The audit table above is unchanged. Stage 1 closes with the **568 MiB envelope NOT QUALIFIED**, the final role constants above, and the unchanged raw ceiling.

A non-latching raw-ledger refusal at ordinary and terminal native publication is a hard precondition of resident ledger charging in 2c. That later change must distinguish `Capacity("metadata and decoded bytes")` from monotonic run-storage bounds, return retryable `storage_backpressure` with waiting/Stop words, and retain the latch for monotonic bounds. It must first show that an ordinary publication cannot take room Stop then needs without a safe cleanup retry. The publication-race probe becomes a kept test, with Finish and Freeze succeeding after pressure is released. This recovery change does not implement that later policy.

The two App failures recorded above were a gate-ordering artifact: no-default shipping builds replaced the fixture-enabled host in the shared debug target. The shipping build matrix itself passed. The resumed gate rebuilds the fixture-enabled host before the full App suite. Subsequent shipping gates use an isolated `CARGO_TARGET_DIR=target/shipping`, removed on success, and invoke `scripts/ci-shipping-builds.sh` without an outer lock after the upstream merge.

Recovery completion gates now **PASS**: fmt; workspace all-target clippy with Project `test-util` and all slow features; slow-test lint (129 / 150 / 27); all three no-default shipping builds; fixture-enabled host rebuild; full Project **455 passed / 0 failed / 3 ignored** across 23 targets; full App **1,013 / 0 / 5**; the release crash/reopen lifecycle; and workspace test compilation. Both previously failing App tests pass after rebuilding the fixture host. Their earlier failures remain recorded as the shipping/test-host gate-ordering artifact. Windows, equipped-provider and paid runs were not performed.

| Release native row | Warm open excluded, ms | Five subsequent opens, median ms | Range ms | Complete lifecycle plus six opens, seconds |
| --- | ---: | ---: | ---: | ---: |
| 400 clips / 200 edits / 201 retained batches | 345.164 | 322.209 | 317.224–340.581 | 59.079 |
| 1,200 clips / 68 edits / 69 retained batches | 525.063 | 521.357 | 519.068–539.079 | 34.929 |

These opens restore the full native cut and verify exact history and Redo, after Finish/Freeze, merge, Stop and actual child process death. They are current release observations, not a new before/after speed-up claim. The previously reported restore regression and hashing/replay breakdown remain scoped to their recorded comparison. Evidence: `recovery-full-project.log`, `recovery-full-app.log`, `recovery-fixture-host.log`, `recovery-release-lifecycle.log`, `recovery-workspace-no-run.log`, `recovery-shipping-*.log` and `recovery-gates.json` under `target/history-capacity/`.

At the recovery commit boundary `target/` is **25,403,630,303 bytes**. The earlier idle cleanup reduced it from 29,649,346,827 to 9,673,424,314 bytes; logs were kept and no Cargo run was interrupted. The next boundary is the real merge of `e6893f44`, followed by the compact/C4b position check and full merged-tree gates. The recovery change is ready for its local commit; the aggregate memory envelope remains **NOT QUALIFIED**.

## Merge of the closed run-limit work

Recovery is committed as `a1325ba77e110bd5937cbf6ff9da45c85eedf11d`. The subsequent real merge brings in `e6893f44d924b369f857b42916e6d0c85b31fab0`: C5 linear retention, C4 whole-approval admission, C4b missing-edit disclosure, shipping cfg and pump fixes. Compact history resolution remains the explicit `HistoryRecord` → resolved history API. The merge forwards the prepared publication fence through the serial replay wrapper, preserves the new root-publication transaction, and keeps the resolved-history digest in save receipts. The floor-prefix verifier materializes the compact anchor under its replay reservation and supplies both checkpoint persistence and the pending-history measurement; no document-per-boundary reader is restored.

`persisted_history` now returns the fourth value, `CapturedHistory`, taken from the accepted cut under the same Core freeze as its checkpoint. `KeptEditCoverage.position` remains `floor_offset + batches.len()`. The new fast `c4b_compact_history_preserves_absolute_accepted_batch_position` witness records six accepted commands, including empty Undo and Redo, and tests both no floor and a two-batch floor. After compact serialization and Core restore, every history value and the absolute position agree with the accepted cut. C4b's native crash/reopen disclosure and App display witnesses also pass. The initial test's attempt to request a zero-length floor was corrected to retain the original history for the unfloored case; the product floor contract was unchanged.

All four semantic regression archives were regenerated through the merged writer with compact history/lineage, C5 retention and C4b coverage. They retain the specific positive-open, negative-error and no-rewrite assertions. All three temporary generator source files were restored byte-exact and independently hash-checked before normal gates. Evidence is in `fixture-generation-merged/`; the test-only pending-history profile now measures the actual persisted `HistoryRecord`, rather than its document-bearing portable transport.

Formatting, workspace all-target clippy `-D warnings`, slow-test lint (**132 / 154 / 27**), the isolated shipping matrix and workspace build pass. `CARGO_TARGET_DIR=target/shipping` kept the shipping host separate from test fixtures, and that directory was removed after success. Peak total `target/` during shipping was **13,012,212,311 bytes**. The complete suite and compilation results follow. The role constants and **568 MiB NOT QUALIFIED** envelope are unchanged; the accepted latch limitation and the hard 2c precondition above remain in force.

Final merged-tree gates **PASS**: fmt; workspace all-target clippy with Project `test-util` and all slow features; slow-test lint; isolated shipping matrix; workspace build; the full suites below; and workspace test compilation. C4b's native crash/reopen and UI witnesses, the compact absolute-position test, both required native capacity lifecycles, non-latching candidate refusal and the regenerated archive reader tests pass in the full run.

| Crate | Passed | Failed | Ignored | Completed targets including doc tests |
| --- | ---: | ---: | ---: | ---: |
| Core | 801 | 0 | 2 | 22 |
| Project | 467 | 0 | 3 | 23 |
| Agent | 1,722 | 0 | 99 | 30 |
| App | 1,014 | 0 | 5 | 1 |

The standard full suites enable the slow features. Explicitly ignored equipped/provider and on-demand tests, including the six-hour maximal fixture, remain ignored; no standalone ignored-test or paid run is claimed. Media source was unchanged by this merge; its previously reported full suite remains scoped to that earlier revision, while the merged workspace compiles its tests. The release six-open timings above are scoped to recovery commit `a1325ba7`; the merged tree's debug capacity lifecycles also pass and are not substituted for a new release comparison.

`target/` at this merge boundary is **29,364,414,759 bytes**. The idle cleanup after recovery reduced it from 25,403,631,117 to 8,809,975,449 bytes; the pre-gate cleanup after fixture generation reduced it from 21,147,232,544 to 9,129,845,420 bytes. Shipping's separate directory was deleted after success. No Cargo process was interrupted. Evidence: `upstream-full-suites.log`, `upstream-full-suites-summary.json`, `upstream-workspace-no-run.log`, `upstream-clippy.log`, `upstream-shipping.log`, `upstream-c4b*.log` and the cleanup size files under `target/history-capacity/`. Windows execution was not performed. Both commits are local and unpushed; settled-run release is next.

## Stage 1 CI fixes

CI run `43139119478` on `8d81d6f9` found a Windows-only unused helper, a 600-second lifecycle timeout, and one post-pass allocator abort. The 2a patch and decision note are parked separately, hash-checked, under `target/history-capacity/parked-2a-before-ci-fixes/`; none belongs to this fix commit. Its two queued Cargo commands had not started. Their cwd, argv and absence of child processes were checked before terminating only those waiting `flock` processes, then draining both sessions. No running Cargo build or test was interrupted.

`pending_restore_for_test` now has its callers' `cfg(all(test, target_os = "linux"))`. The neighbouring `capture_pending_for_test` remains `cfg(test)`: unlike the new helper, it has callers in the cross-platform `in2b_tests` module. The other test-only additions between `e6893f44` and stage 1 were audited; the snapshot and absolute-position tests compile on both platforms, while checkpoint recovery has its enclosing Linux gate. The audit is in `ci-fixes/test-cfg-audit.json`.

The earlier local Windows evidence was not a final-tree cross-clippy pass. The old `windows-cfg-clippy.log` checks only Project on Linux, and the stage-1 report records Windows as not run. I omitted a Windows cross-clippy invocation on the final recovery/merge tree. The existing Windows script already lints workspace default targets and Agent slow targets; that workspace invocation would have caught this helper on the final tree. It now also explicitly lints App's standalone default and slow feature graphs and prints the checked HEAD. No final-tree Windows pass is claimed until the rerun below completes.

The original unmodified combined lifecycle completed locally: **470.159 seconds wall, 455.386 user CPU + 3.223 system CPU**, one run under the recorded concurrent machine load. The 400/200 row took **307.872 seconds**, including a 1,830.171 ms open; 1,200/68 took **161.703 seconds**, including a 3,224.438 ms open. All assertions passed. Samples of the parent and child processes show continued CPU work in the MCP worker threads, rather than a deadlocked child. The parent was normally waiting for that working child. The first sampler missed children created by a libtest worker; the second enumerated every thread's children, and the final `RUSAGE_CHILDREN` CPU totals cover the reaped child processes independently of that sampling correction.

The repeated work is the persistence/admission path executed by those MCP workers: prospective cuts scan retained history and hash its boundary documents through the JSON codecs. This is a code-path attribution supported by the codec/profile trials below, not a sampled native-stack profile. The increasing cost per 25-edit block remains visible in the instrumented child; the final selected run spends 46.701 of its 55.574 seconds accepting the 200 edits.

The 400/200 test keeps its full native tool edits, the 20-ripple-delete batch, Redo, Finish/Freeze, merge, Stop, actual process death and exact reopen. The unchanged 1,200/68 lifecycle is a separate, explicitly ignored local measurement, listed in `ci/ignored-tests.txt`; it was not reduced. The child now reports edit progress and has its own 500-second deadline below the unchanged 600-second shard limit. On timeout the harness verifies its owned child's cwd and executable/argument identity, terminates it and waits for it, then reports the child log. It no longer waits indefinitely or relies on the outer shard process to dispose of its child.

The newly separated, explicitly selected local-only 1,200/68 entry point also passes on two CPUs: **31.224 seconds wall, 26.029 user + 0.922 system CPU**. It still refuses the next edit before mutation and checks the original Finish/Freeze, merge, Stop, process-death and exact-reopen behavior. Evidence is in `ci-fixes/final-1200/`.

The serializer-only two-core trial took 268.125 seconds (262.822 user + 1.710 system CPU). Optimizing Core and Project codecs as well took 70.014 seconds (64.425 user + 1.517 system), with every assertion passing. Agent at level 1 added no meaningful improvement (68.508 seconds), so that setting was removed. Optimizing the scalar JSON formatters instead took 61.240 seconds (55.804 user + 1.334 system). The selected profile also optimizes Agent at level 3: the complete two-core 400/200 witness passes in **55.574 seconds wall, 49.054 user + 1.460 system CPU**, including exact crash/reopen. The 200 real edits take 46.701 seconds. At the requested 3–4× slowdown this projects to **167–222 seconds**; the sub-200-second aim is not qualified on an actual CI runner, while the unchanged 600-second limit has substantial margin. These trials pin the process and its crash child to CPUs 0 and 1; queue/build time is excluded. Debug assertions and overflow checks remain enabled, and development/shipping profiles are unchanged. These are test-build settings, not a claim of a production speed-up or a change in paid-run behavior.

The first complete Agent run exposed a readiness race with the faster test profile: **1,717 passed, 5 failed, 100 ignored** across all targets. All five media-wait failures received the existing retry reply when they expected the finished sample. The decode worker released its endpoint permit before the watcher joined it and stored `Ready`; an immediate retry could reach the watcher's 10 ms polling gap. The watcher now holds that same permit through memo publication. Thus an idle endpoint implies its result is already installed. No words, deadline, retry cache key or admission limit changed. The existing five failures are the before evidence. The test-only outcome hook now observes the watcher's final result, after cancellation, rather than a raw decode that may finish just before cancellation is observed. All five targeted tests then pass (**39.93 seconds**), retaining the real 30-second wait, exact memo reuse and no-publication-after-Stop assertions. The final full suites pass: Agent **1,722 / 0 / 100** across 30 targets with two test threads; App **1,014 / 0 / 5** with four test threads. Counts are passed / failed / ignored. The Agent run includes all five media cases and all four provider child fixtures. Evidence: `ci-fixes/full-agent-before-handoff.log`.

The first rerun after that fix completes with **1,718 passed, 4 failed, 100 ignored**: all five media cases pass, but four B2b provider-preparation child fixtures reach their existing 45-second deadline after printing only `running 1 test`. Those same four pass together, unchanged and with four test threads, in **12.88 seconds** (13.057 seconds including Cargo; 3.965 user + 1.657 system CPU). The passing full Agent rerun uses two test threads to reduce native-fixture contention; that cause remains an inference, and no deadline, case or product bound was changed. Evidence: `full-agent-provider-timeout.log`, its summary, `provider-retries.log`, `provider-retries-result.json` and per-process CPU/wait samples.

The teardown probe alternated paired processes under load using CI fast arguments (`--color never --exact <test> --test-threads 4`). The **unpatched binaries pass 60/60 at each head, zero aborts**: median 4.670 seconds at `8d81` (range 4.420–5.171), 4.620 at `e689` (4.370–5.171). They were built from exact Git archives with the original test profiles and separate output directories and fixture hosts. Workspace packages compiled afresh; only registry/native dependency outputs were reused. Sources match their committed bytes before and after all 120 processes. The manifest, commands, profiles, binary hashes and per-process logs are in `ci-fixes/unpatched/`.

Separately, the instrumented binaries pass another **60/60 at each head, zero aborts**, capturing `/proc/self/task` after all local owners dropped. Their medians are 4.822 seconds at `8d81` (4.620–7.282) and 4.771 at `e689` (4.420–6.931). Both temporary patches were restored byte-exact and independently hash-checked. Those runs supply the thread census; the unpatched repetitions avoid the census I/O affecting the teardown race.

Both return snapshots enumerate **16–18 TIDs, median 16**. Names remain readable for **10–17 threads** at both heads (median 13.5 at stage 1, 14 at base); the difference reflects threads exiting during the per-TID reads, not an atomic process-wide snapshot. Exact distributions are retained in `ci-fixes/returning-thread-totals.json`. The stable native/host families and count ranges agree: each Vulkan analysis/update family remains at one; Core 0–1, host 1–2, MCP 4–5, and each media/derived/transcription/visual family 0–1 at both heads. Directory snapshots also raced threads already exiting: 181 and 184 entries respectively no longer had a readable name. One stage-1 snapshot saw one transient `tokio-rt-worker` with `wchan=0`; no base snapshot happened to catch it. The unchanged `McpServer::Drop` only signals shutdown at both heads, rather than joining there; neither the MCP drop code nor its joining `shutdown` API changed in stage 1. No new retained replay, resolver, host or guardian thread was found. This does not prove a cause for the one CI abort. It is recorded as an unreproduced one-off for the requested shard rerun, with the exact text **`double free or corruption (fasttop)`**, after an otherwise passing test and exit `-6`. No speculative product teardown change was made.

The Windows script completes successfully, including workspace MSVC clippy, Agent slow clippy, App default and slow clippy, three MSVC shipping checks, the shard self-tests and three isolated Linux shipping builds. Windows execution remains unrun. Linux changed-crate all-target clippy passes in both default and slow feature modes. The separate shipping target was removed after success.

The first fixture workspace build encountered a local artifact collision from the paired base-revision probe: both roots emitted `libkinewright_core-524cbdf418fa82d2` into the shared target directory, and Cargo reused the base artifact without the new journal API. Both paired-build manifests independently show fresh Core compilation from their respective source roots; each executable was copied before building the other head, so the paired process evidence is unaffected. This was my probe-output isolation error. Package-scoped Core/Project cleanup and a fixture-enabled rebuild precede the resumed full suites; the failed build log is retained rather than hidden. `shared-target-artifact-audit.json` records the identical artifact paths and different source package identities.

Evidence: `target/history-capacity/ci-fixes/paired-results.json`, `paired-summary.json`, `paired-build-manifest.json`, per-process thread snapshots/logs, `lifecycle-baseline-result.json`, `lifecycle-baseline.log`, and `lifecycle-all-thread-cpu.jsonl`. Final gates **PASS**: fmt; Agent/App all-target clippy `-D warnings` in default and slow modes; the Windows cross-clippy script, including both App graphs; all three isolated Linux shipping builds and three MSVC shipping checks; slow-test lint (**132 / 155 / 27**); shard self-tests (**22**); full Agent (**1,722 / 0 / 100**, 30 targets, two test threads); full App (**1,014 / 0 / 5**, four test threads); workspace test compilation (**80 executables**). No Windows execution or paid/equipped-provider run is claimed. Both final Windows Agent test artifacts are newer than the final test-hook change.

At the initial clean boundary, package-scoped cleanup reduced `target/` from **38,783,005,869 to 23,598,558,393 bytes**, preserving parked 2a sources and evidence. After the isolated Linux shipping matrix passed, removing its completed output reduced the then-current size from **37,864,578,981 to 34,021,263,763 bytes**; the active workspace output used its own directory. Final `target/` is **34,021,267,530 bytes**. `scripts/__pycache__/` and the separate shipping output are removed. This CI fix is one local commit on `8d81d6f9`; no push is performed. The separate review-fix brief follows before returning to parked 2a. The 568 MiB envelope remains NOT QUALIFIED, and the stage-1 raw-publication latch limitation is unchanged.

Post-commit gate correction: a fresh PITFALLS read caught that the two-thread Agent run did not follow the standing four-thread rule. The exact committed `e4f73529` tree was then rerun at `nice -n 19`, `-j 4`, `RUST_TEST_THREADS=4` and `--test-threads=4`: **1,722 passed, 0 failed, 100 ignored across 30 targets**. All five media cases and all four formerly timed-out provider child fixtures pass. No test, deadline or product code changed for that rerun. Evidence: `ci-fixes/full-agent-four-final.log` and its summary. The commit is not rewritten; this correction travels with the next report update.


## Stage 1 fix 1

The healthy-history recovery defect is fixed without raising a cap. Final gate counts are recorded below.

The command-payload estimate is no longer an admission gate. Core executes replay inside a bounded window and checks temporary room before each operation, including operations within one batch. Store resolution reserves the role's replay window first, then releases unused overlap while retaining the resolved history's charge without an uncharged gap. The same bounded execution and retained-role check are used by pre-edit admission, candidate preview, ordinary selection/publication and cold resolution. Memoized publication proofs are installed only after those checks and are bound to the record identity and exact limit profile. Replacing a memo entry preserves its existing resolved-history proof; decoding support fields alone does not manufacture one. No role limit or storage ceiling changes.

The replay budget charges the current candidate, typed array growth, supplied command payloads, actual observed identity payloads and the distinct documents still resident in the replay. Discarded coalesced states and cleared Redo branches release their charge; the peak remains charged against the window before each operation. An ephemeral address-to-cost map owns no documents, is pruned after every command and always remeasures a new current document, including when its allocation reuses a cleared Redo address. It does not rely on unverified journal cursor counts or identity declarations to forecast allocations. Before copying prior state it additionally charges:

| Operations | Prior state reserved before execution |
| --- | --- |
| `CopyClipAttributes` | Three source-clip payloads and two destination-clip payloads, covering source effects, replacement effects, retained target keyframes and vector growth. |
| `SplitClip` | Four source-clip payloads, covering the new right clip and observed parent/results. Actual identity observations remain charged if a later operation removes the pieces. |
| `ThreePointEdit`, `PatchedThreePointEdit` | Four current documents: implicit splits can duplicate clips across participating tracks before insertion or overwrite removes them. |
| `TrimClip`, `SlipClip`, `ReplaceClip`, `FitToFill`, `SetClipSpeed` | Four addressed-clip payloads for the local original and automation rewrites. |
| `RollEdit`, `SlideClip` | Four current documents, covering up to three local clip originals and automation rewrites. |
| `RippleInsertGap`, `RippleDeleteClip` | Four current documents for retained clip/track-automation originals, rewritten curves and maps. |
| `ConvertLegacyLook`, `SetTitleParam` | Four addressed-clip payloads for the retained enable curve or title/preset source. |
| `SetEffectParam`, `SetEffectKeyframes`, `UpsertEffectKeyframe`, `RemoveEffectKeyframe` | Four addressed-clip payloads for prospective effects, keyframe maps and curves. |
| `SetAssetColorDescription` | Four current documents, including the previous interpreted description retained as provenance. |
| All other operation kinds | Their supplied payload and typed array growth; they transfer/remove existing payload or clone values supplied in the command. `AddFreezeFrame` creates a fresh clip, while LUT registration carries metadata rather than samples. |

The operation-copy match is exhaustive. The ordinary session anchor is distinct from the native role window: it reserves only the room actually remaining in the unchanged 128 MiB ordinary pool and raw ledger before its prefix replay. Capacity refusal leaves the anchor and work untouched.

Corruption audit of stage-1 additions and the paths changed here:

| Sites | Classification |
| --- | --- |
| Core journal resolver: baseline/current validation, hashes, accepted commands, per-operation identities, retained boundary hashes and final document | `Corrupt` denotes disagreement with the saved bytes. Budget refusals are `Capacity`; missing internal budget state is a commit invariant. |
| Core canonical hashing and size-payload generation | Encoding failure is typed and becomes I/O invalid-data; the infallible string formatter has an internal invariant refusal. Neither is corruption. Generated JSON scanning overflow/depth is capacity. |
| `bundle::history_error` | Exhaustive mapping: capacity stays capacity, disconnected actor becomes a broken-pipe I/O failure, history/barrier/commit refusals retain their types. Only `HistoryError::Corrupt` becomes damaged-bundle authority. |
| Project history record capture/verification/validation | Record/document/hash identity disagreements are corruption. Command-count and role/encoded-size limits are capacity. Encoding failures are I/O invalid-data errors. |
| Project history replay and replay gate | A replay-size disagreement cannot grant recovery. Poisoned gate/accounting or allocation refusal is capacity. |
| `bundle_commit` and `store` history staging/checking | Different staged document identities or a decoded transport not matching the selected cut remain corruption. Typed decode failures remain corruption; encoding failures are I/O invalid-data errors. |
| `bundle_recovery` history graph inspection | Missing journal/manual history references or disagreement among saved checkpoint, replay and lineage boundaries remains corruption. Undo/Redo lineage clip sets now also match every replayed boundary. |
| Compact lineage typed decoding | Malformed lineage bytes or invalid compact references remain corruption. Quota refusal stays capacity. |
| Checkpoint recovery verification | Missing checkpoint/input edges, wrong checkpoint identity/document/lineage kind/current occurrences, unexpected dependency kinds and changed owned inputs remain corruption. Only an authoritative corrupt history resolution can offer recovery; capacity and other refusals propagate without an offer. |
| Candidate preview and run seed | Invalid baseline documents remain semantic corruption. A stopped request channel is I/O; capacity is never relabelled as a damaged history or as a head/floor mismatch. |
| Admission-only batch IDs | Failure of the fixed-width internal generator is a commit invariant, not damaged project bytes. |


The real-store split/delete witness fails before this change: 60 splits of a clip with ten primary-correction keyframes produce a valid history that Save rejects as `Corrupt("history replay exceeded its live reservation")`. It now saves, closes, cold-opens with the complete history equal, and exercises Undo and Redo to both ends. The copy/remove witness uses ten clips and a 1,000-keyframe source; it reaches the actual injected publication failure, refuses twice while raw capacity is held, then saves on retry after capacity returns and cold-opens exactly. Neither witness seeds the memo. A separate low-bound reopen returns typed capacity, offers no empty-Undo checkpoint recovery, preserves the source bytes, and restores complete Undo when the bound is restored.

The native copy/remove checkpoint takes the same 1,000-keyframe shape through Finish, Freeze, merge, Stop, child-process exit and exact cold reopen. The final selected lifecycle passes in **8.91 seconds**, with two accepted batches and no lost Undo. This fixture uses a Colour task with explicit effect grants; its first two setup attempts correctly refused missing effect scope and an Editor role. The final fixture validates those batches through the real Colour policy before applying them. Those setup refusals did not weaken product checks or count as the requested witness.

Core keeps 67 real operation-oracle cases in-tree, with an exhaustive kind mapping and a schema walk that fails if any operation kind has no case. Seeded sequences now include copy/remove and split/delete compositions. Large implicit splits through both three-point edit forms, cold nested-vector capacities, and discarded coalesced states are checked independently. In the single-batch intermediate-copy test the final document equals its baseline, but an independently sized replay window cannot hold the temporary copies. Removing only the per-operation check makes that assertion fail by returning `Ok`; restoring the check passes. The negative-control log is compressed under `fix1-operation-check-mutation-final.log.gz`. An earlier version derived its window from the mutated measurement and was vacuous; it is not claimed as evidence. No temporary mutation or oracle-export helper remains in the source.


The full Project gate also caught a warm/cold capacity-model disagreement: a memo hit retained the replay proof but modeled only retained history, while a cold reader first reserves the whole replay window alongside the input documents. Publication's independent empty-reader model now charges that same overlap on memo hits; it does not create another replay or add a fictitious reservation to the writer's physical ledger. The existing exact-fit/one-byte-over witness is retained. Two rescue fixtures were corrected to distinguish resident history from temporary replay peaks and to compare matching limit profiles. Their lineage check compares the floor's complete saved document and occurrence map, including a parent at a pre-split floor; it retains exact current occurrences, split ancestry and shared batch identities, and verifies that saving the other branch leaves the inactive lineage wholly unchanged.

Before this fix, the lead's CI changes were cherry-picked unchanged: `9b8d99fc` as `46e9279e`, `73207d48` as `0d5df7ae`, and `e70ad650` as `260fc9f6`. Cargo.toml has no Core, Project or Agent opt-level override. Full Linux slow test gates use `.cargo/ci-test-linux-slow.toml`; the codec overrides remain global. The oracle marker uses `println!`, and its gate uses `--show-output`. No local build-profile trial or CI timeout increase is included. The imported generated Python cache files are removed in this fix's cleanup. The parked 2a patch is unchanged and remains outside this commit.


The Windows cross-clippy wrapper also invokes the Linux shipping helper. That invocation now explicitly sets `CARGO_TARGET_DIR=target/shipping`, so it cannot overwrite the fixture-enabled host in the main target directory. MSVC checks retain their explicit `target/win-xwin` directory. Both wrappers continue to own their locks; no outer lock is added.


The first full Agent gate exposed two further assumptions. The snapshot test forged a large Undo counter and expected the former estimate to reject it at capture. It now proves that the receiving bounded replay rejects that wrong counter as corruption before adoption, without allocating from the claimed count or changing the original Core. The host-switch fixture had used a replay peak as retained quota headroom. Its corrected retained-byte allowance then exposed a real redundant replay: confirming the original source after a refused switch reserved another full window while holding the selected target. Unchanged saves and retirement disclosure now reuse the already materialized, verified cut. That cached proof carries its object identity and effective history policy; changing a replay, retained, encoded or record bound invalidates it. The shared pool total is checked by the real ledger and the fresh-reader publication model on every operation. Only its effective per-role limit belongs to the immutable history proof. Cold reads now enforce the same ordinary encoded and document-bearing admission checks as selection. The pinned-target confirmation, retirement, original-envelope retry and exact reopen witness passes without changing a bound or dropping the target pin.

The first final Project pass reported **466 passed, 4 failed, 3 ignored**. All four exposed the newly exercised cached-cut recheck decoding the journal again: unchanged saves lost their no-decode behavior, and that early read repaired an intentionally misbound accounting memo before publication could reject its identity. The recheck now hashes the saved bytes, checks the full cached checkpoint identity and retained policy, and leaves accounting memo misses to the independent publication check. It does not redo bounded replay or weaken its identity/policy-bound proof. The existing four regression assertions remain intact.

The unchanged full Project rerun now passes **470 / 0 / 3** with `test-util` and four test threads. All four cache regressions above pass. The full Core gate passes **806 / 0 / 2**, with captured oracle markers; its seven changed source/fixture files are byte-identical to that completed gate.

The final full Agent gate passes **1,723 / 0 / 100** across all targets with `slow-tests` and four test threads. It includes the unchanged 400/200 crash lifecycle, the new native copy/remove lifecycle, candidate non-latching refusal, dirty cleanup, the corrected forged-cursor check and all pinned-target switch lanes.

The final full App gate passes **1,014 / 0 / 5** with `slow-tests` and four test threads. No fixture host was overwritten by shipping builds. Counts throughout this section are passed / failed / ignored.

Final remaining gates **PASS**: fmt; all-target clippy `-D warnings` for Core, Project, Agent and App in default and slow modes; workspace test compilation (**80 executables**); slow-test lint (**133 / 155 / 27**); shard self-tests (**22**); slow-test self-tests (**48**); the Windows cross-clippy script with workspace, Agent slow and both App graphs; all **three isolated Linux shipping builds** and **three MSVC shipping checks**. Feature-disabled shipping checks emit `REFRESH`/`TASK_TOOL_NAMES` warnings in unchanged adapter code; the required clippy graphs are clean. Windows execution and paid/equipped-provider runs remain unrun. Evidence: `target/history-capacity/fix1-final-gates/`, plus the unchanged Core gate in `fix1-gates/core-full.log`.

Package cleanup before the gates removed **994 files / 16.3 GiB**. Workspace test compilation plus isolated shipping output temporarily reached **45,233,594,368 allocated bytes**, above the target budget. After the three Linux shipping builds passed, their **3,856,478,208-byte** output was removed; at the next idle Cargo boundary, unused example output (**1,912,840,192 bytes**) and the imported Python caches were removed. Final allocated `target/` usage is **39,716,573,184 bytes (39.72 GB)**; apparent size is **39,692,266,650 bytes**. No active build output was removed. All 18 parked 2a files were verified byte-exact, and small evidence logs remain. Cleanup evidence: `fix1-package-clean.log`, `fix1-shipping-cleanup.json` and `fix1-idle-cleanup.json`.

The role constants and raw ceiling are unchanged. The **568 MiB envelope remains NOT QUALIFIED**, with the holder audit and accepted publication-latch limitation above: **Finish and Freeze are NOT guaranteed after a raw-ledger refusal of an ordinary or terminal native publication; Stop and exact retention are.** The non-latching raw-publication change remains a hard precondition of 2c. Settled-run release is outside this commit. This is one local fix commit after the three supplied CI commits; no push is performed.

## Stage 1 fix 2

Manual ingress now runs the writer's effective ordinary history predicate inside the Core transaction, before `DocumentChanged` or `AcceptedHistory`. The strict lease carries the writer's actual history limits. A capacity refusal restores the document, revision, Undo/Redo, journal, pending messages, sequence, predecessor and every JSON-cache byte/index. The actor-owned encoder stages its proof through an exclusive mutable reference and commits it only after encoding and queue admission also succeed. A lower writer limit cannot be replaced by the GUI's defaults.

Astra's twenty-clip batch is refused before acknowledgement, and a smaller retry publishes and cold-reopens exactly. The ten-clip control still publishes, cold-reopens with equal history, and traverses Undo/Redo. The refusal is typed `ManualHistoryCapacity`: “This edit exceeds the project's history capacity. Try a smaller edit or start a new project.” No limit is raised.

The twenty-clip case has 977,382 encoded bytes and **17,674,226 retained bytes** under corrected typed accounting. Its **typed replay peak is 39,037,324 bytes**, exceeding the ordinary 33,554,432-byte window by **5,482,892 bytes**. Retained room is not the refusal. Each operation temporarily reserves three source and two prior-destination clip payloads (2,280,354 bytes here); these allowances are **peaked per operation, never accumulated across the batch**. The hypothetical nineteen-operation sum is 43,326,726 bytes and is not charged. Only actual retained identity observations accumulate. On this exact 1,000-keyframe fixture with twenty clips, an ordinary project accepts copying to **15 destinations (16 clips including the source)**: peak 31,929,300, retained 14,306,457, encoded 792,678 bytes. Sixteen destinations exceed the window (33,704,229). These are conservative typed charges, not measured physical heap. The on-demand census records each fanout and its individual versus hypothetical cumulative copy allowance.

Replay metadata charges canonical portable-vector capacities at capture and cold decode, including command and identity arrays. The prefix witness now also serializes the transport and requires its cold proof to pass at exactly the warm window at every cut. The obsolete three-JSON-payload replay gate was removed from floor prefiltering; encoded/ordinary lower bounds remain, and the shared predicate alone decides replay room. An independent exhaustive floor search agrees across 10,000 seeded histories, undo excursions and decimal counter boundaries. Its quota-setting helper includes the actual replay peak, instead of calibrating only the older retained/ordinary costs.

Acknowledgement-door audit:

| Door | Predicate before acknowledgement | Kept witness |
| --- | --- | --- |
| GUI manual lease, single/batch/coalesced edit | `ManualIngress::execute` calls the mandatory encoder guard before cache/identity encoding, then commits the proof only with the queue transaction. `OrdinaryHistoryAdmission` uses the shared Project predicate. | Twenty-clip refusal, ten-clip exact cold control, lower-limit lease; Core cache rollback mode 5. |
| Native task batch, including specialist direct apply | `TaskEndpoint` previews the proposed cut and `TaskHistoryAdmission` applies the shared role predicate before the consent/apply funnel returns an accepted edit. Its role proof includes the specialist delta and lineage bound. | Existing cleanup history pre-mutation and full native lifecycle witnesses. |
| Ordinary agent MCP single edit, plans and LUT batches | The three application funnels all call `check_batch_history`; their endpoint's ordinary proof previews before the revision-gated live request. | New single/plan/LUT refusal witness; complete live history and revision unchanged. |
| Legacy session merge/cherry-pick and approved proposals | All use `branch::apply_to_live`; ordinary admission precedes the optimistic live transaction. | New shared-funnel capacity refusal with live history unchanged. |
| Native candidate application and accepted merge | Candidate preview applies the same role predicate before staging; publication checks the identical record and live bounds. Capacity remains actionable and non-latching at preview. | Existing candidate merge reservation/refusal and full lifecycle tests. |
| Owned import and colour interpretation | Saved commands go through ordinary publication's shadow check; unsaved candidate installation now explicitly applies ordinary admission before installing its document/input manifest. GUI input commands additionally use the manual guard. | New unsaved-import refusal preserves document and inputs; existing saved import/interpretation suites. |
| GUI Undo/Redo | The manual transaction guard also covers history steps, before their accepted event and message. | Cache rollback/cursor cases and ten-clip cold Undo/Redo control. |
| Native Undo/Redo | `preview_history_step` validates the proposed cursor and runs the role guard before applying the real history step. | Existing bounded cleanup Undo/Redo witness. |
| Cold restoration and checkpoint/recovery adoption | Store resolution uses bounded replay and the same retained/ordinary checks before returning a cut. Recovered document copies are independently checked before first Save; frozen installation remains an exact-cut transition. | Existing cold capacity/no-recovery and exact recovery suites; new stale-expectation witness. |
| Bootstrap handoff | Decoding produces an opaque Core replay proof; host adoption consumes that proof. Locally constructed snapshots still require bounded replay and full value agreement. | Forged snapshot/cursor tests, cold-versus-adopted history and operation-log comparison, startup lifecycle measurement. |

The pre-Studio document-only GUI still writes a document without persistent Undo; it is not a durable-history acknowledgement. Its document becomes a new baseline when Studio is activated, where ordinary admission applies before activation/Save succeeds. The native guard's shared-ledger reservation and publication's fresh-reader model remain separate from immutable per-cut proofs: a temporary ledger shortage is not proof of damaged bytes. The existing raw-publication run latch is unchanged in this fix.

The stopped manual shadow request now returns broken-pipe I/O instead of `Corrupt`. The exhaustive `history_error` match maps `StaleExpectation` and `CaptureChanged` to `HistoryChanged`; a changed restore expectation cannot authorize an empty-Undo rescue. The existing `CoreState::restore` raw-snapshot path is left unchanged: production inputs reach it after bounded admission. The new resolved-proof adoption path avoids that second execution for the bootstrap and native preview.

Debug startup A/B uses identical compact input and real subprocess launch with the unchanged five-second production deadline, followed by a complete history comparison and graceful shutdown for every successful launch. Commands force Core, Project and Agent to opt-level 0; media uses the CI test config and the unchanged codec overrides. Ten samples per head alternate in rotating order, pinned to four CPUs on an i5-13600K; no quiet-machine wait. The reported samples span load averages **4.46–13.85**. This is a local four-CPU proxy, not hosted CI execution.

| Head | Batches | Samples | Median / worst attempt | Failed launches |
| --- | --- | --- | --- | --- |
| `e4f73529` | 4,000 | 10 | 479.646 / 706.026 ms | 0 |
| `910a3e89` | 4,000 | 10 | 4,059.361 / 5,114.721 ms | 2, startup deadline |
| `e4f73529` | 12,000 | 10 | **No ready:** 370.614 / 374.301 ms to refusal | 10 |
| `910a3e89` | 12,000 | 10 | **No ready:** 367.329 / 425.259 ms to refusal | 10 |
| Final fix 2 startup implementation | 12,000 | 10 | **1,490.409 / 1,520.028 ms to ready** | **0** |

Both old-head 12,000-edit stderr diagnostics say “history exceeds its bounded admission”; their EOF launch errors are capacity exits, not fast successful startups. The 4,000-edit comparison reproduces the reported deadline regression under load. The final 12,000-edit pairs are the ten samples per head in `fix2-current-measure/samples.json`, built after canonical vector accounting and the floor-prefilter correction. The 4,000-edit controls are the unchanged pinned heads in `fix2-startup/final/samples.json`. Earlier fix-2 prototype samples are retained separately and are not pooled into the final row. Build commands, binary digests and source digests accompany both sets.
This confirms a fix-1 startup regression. Its accounting rebuilt a boundary map after every journal batch, visiting a growing retained prefix quadratically. Replay now updates document reference counts as Undo/Redo/coalescing move or discard entries; each new document is measured once and abandoned Redo entries are visited once when released. An independent complete resident walk agrees after each command. Bootstrap decoding additionally retains an immutable replay proof, so adoption no longer executes that journal a second time. The proof cannot be set by a wire flag or by a caller-supplied digest. The runtime proof uses an opaque Core-created metadata digest and weak references to the exact immutable documents. It retains no document payload and cannot arrive in input bytes. All decoded cuts can use it; the strict compact transport is unchanged.

The original App fixture used 12,000 edits at `e6893f44`; stage-1 commit `dfde093a` reduced it to 4,000 to fit the then-overcharged portable window. **This fix restores the original 12,000 edits**, without grouping, lowering the fixture, raising a bound or changing the production deadline. Inline `ColorContext` was already inside `size_of::<Document>()` but was charged again as a heap payload; a compile-time no-owned-storage assertion now protects that correction. Replay counts the typed journal, its buffers and future actor metadata directly instead of treating it as three retained JSON trees; owned operation payloads still have conservative charges, and prior-state copies still pass the per-operation guard. Restored observation-vector spare capacity and the ten-percent margin are canonical across cold and incremental proof. The final A/B above includes ten samples of the restored 12,000-edit cut at all three heads. Old-head refusals are reported as failures, never successful fast launches. One invalid archive build reused the current host; its distinct-binary check caught that before any timings, and it is excluded from the A/B evidence.

The measured 1,000-batch hand edit exceeded the lead's 100 ms trigger, so this fix implements incremental proof rather than leaving it as a follow-up. Core retains the proven prefix's exact dynamic peak separately from the charge common to the complete input. Extension updates that common input term, rechecks the old maximum, then executes only the tail through the same bounded executor. Changed baselines, floors or prefixes take full replay. Cold and incremental peaks, peak-minus-one capacity refusals, full cursor/identity agreement, coalescing and copy/split/removal sequences are compared at every cut. Cached proofs share immutable live documents; an equal actual cut rebinds preview documents so previews cannot retain a second growing document history. Canonical replay vector capacities are retained across prefix adoption.

Observed debug cost samples are pinned to the same four CPUs. The earlier full-replay and incremental development runs are descriptive controls, not an alternating speedup claim. Final measured rows use ten attempts per row:

| Accepted edits before selection | Retained cut | Hand edit median / worst | Save median / worst |
| --- | --- | --- | --- |
| 1,000, earlier full-replay guard | 1,000–1,009 batches | 225.862 / 311.758 ms | 897.734 / 1,459.549 ms |
| 1,000, final incremental guard | 1,000–1,009 batches; 10 accepted | **121.168 / 140.966 ms** | Changed publication **676.212 / 706.833 ms** |
| 12,000, final selection | Initial Save floors 9,741; retains 2,259 | One next edit accepted: **267.275 ms** | Initial flooring Save **5,334.271 ms**; next changed publication **1,293.618 ms** |
| Same 12,000-edit selection, next nine attempts | 2,260 retained batches | **Capacity refusal**, 253.009 / 442.741 ms; no edit accepted | Unchanged Save 535.559 / 591.217 ms |

A hand edit and a task batch now execute only their new operations for replay admission after the initial proof. Their metadata capture, comparisons, size scans and (for native tasks) lineage/delta construction still scale with retained history length: O(H × document/payload size), plus the new operations. Final retained accounting has a single O(H log H) address-set pass. The remaining **121 ms debug hand-edit cost at 1,000 edits** is a metadata latency limitation, recorded for follow-up; it is not hidden full-prefix command execution. A changed Save performs complete record/publication validation and replay, O(H × document/payload size), plus that retained set pass; selecting a floor may try multiple suffixes. Cold open executes each admitted journal once for resolution with linear boundary bookkeeping. Its opaque runtime proof avoids a second actor execution; raw unproved snapshots retain the original validation/replay fallback. No successful hand-edit timing is claimed at a 12,000-batch ordinary cut: the unchanged ordinary bounds require the stated floor first. No limit was raised, no refusal is counted as an accepted fast edit, and no claim of constant-time Save or zero-copy metadata is made.

The shared-floor host recovery witness had sized its injected quota for recovery-close alone, then expected another Undo after replacement-lease admission. The new guard correctly refuses that extra growth at the old synthetic cap. The fixture now measures both recovery-close and its explicitly requested next Undo, checks that proposed cut with the shared ordinary predicate, and still requires pressure to force a floor. Its full integration target passes **20 / 0 / 1**, retaining the old-permit refusal, new-permit edit, original batch-ID and receipt assertions. No product bound changed.

The lock-file hammer's child and parent code, lock implementation, Project module registration and Cargo.lock are unchanged between the supplied CI-profile parent and fix 1. Children re-exec only the existing `race_child` path, with no history open or new runtime initializer. The parent starts its threads without a child-ready fairness barrier. The lead's rerun passed; this remains a first-occurrence load/fairness flake, with the assertion and ownership checks unchanged.

All 67 operation kinds were walked through the executor helpers and indirect clone calls. The complete table below supersedes the earlier “all other kinds” row. Every operation also reserves the current candidate, typed array growth, its supplied payload and actual retained identity observations. The match remains exhaustive; no estimate-versus-retained comparison becomes corruption.

| Kind | Growth and additional prior-state allowance |
| --- | --- |
| `AddAsset` | New or replacement owned values come from the supplied payload; existing history keeps prior states. |
| `RelinkAsset` | Clones affected clips sequentially; reserve the largest affected clip once, not their sum. |
| `SetAssetColorDescription` | Copies prior interpreted provenance; reserve four current documents. |
| `SetColorContext` | New or replacement owned values come from the supplied payload; existing history keeps prior states. |
| `UpsertBin` | New or replacement owned values come from the supplied payload; existing history keeps prior states. |
| `RemoveBin` | Removes/moves existing values; no extra prior payload clone. Retained Undo documents and observations remain charged. |
| `SetAssetBin` | In-place scalar/identity update; current candidate and typed collection growth cover the edit. |
| `UpsertStringOut` | New or replacement owned values come from the supplied payload; existing history keeps prior states. |
| `RemoveStringOut` | Removes/moves existing values; no extra prior payload clone. Retained Undo documents and observations remain charged. |
| `UpsertSyncGroup` | New or replacement owned values come from the supplied payload; existing history keeps prior states. |
| `RemoveSyncGroup` | Removes/moves existing values; no extra prior payload clone. Retained Undo documents and observations remain charged. |
| `UpsertAudioBus` | New or replacement owned values come from the supplied payload; existing history keeps prior states. |
| `RemoveAudioBus` | Removes/moves existing values; no extra prior payload clone. Retained Undo documents and observations remain charged. |
| `SetAudioMaster` | New or replacement owned values come from the supplied payload; existing history keeps prior states. |
| `SetPanLaw` | In-place scalar/identity update; current candidate and typed collection growth cover the edit. |
| `AddTrack` | New or replacement owned values come from the supplied payload; existing history keeps prior states. |
| `RemoveTrack` | Removes/moves existing values; no extra prior payload clone. Retained Undo documents and observations remain charged. |
| `SetTrackSyncLock` | In-place scalar/identity update; current candidate and typed collection growth cover the edit. |
| `SetTrackMix` | Clones the stored mix entry and its existing curves; reserve that entry. |
| `SetTrackAutomation` | Clones existing mix curves and the supplied replacement; reserve the stored entry plus supplied payload. |
| `AddClip` | Creates a fresh clip from supplied metadata/scalars; typed clip/vector growth and payload are reserved. |
| `AddTitle` | New or replacement owned values come from the supplied payload; existing history keeps prior states. |
| `SplitClip` | Duplicates the complete source clip and observations; four source clip payloads, with actual identities retained. |
| `TrimClip` | Local original and automation rewrite can copy prior clip payload; reserve four addressed clips. |
| `MoveClip` | In-place scalar/identity update; current candidate and typed collection growth cover the edit. |
| `ThreePointEdit` | Implicit splitting or multi-clip/automation rewrite; reserve four current documents plus retained observations. |
| `PatchedThreePointEdit` | Implicit splitting or multi-clip/automation rewrite; reserve four current documents plus retained observations. |
| `SlipClip` | Local original and automation rewrite can copy prior clip payload; reserve four addressed clips. |
| `RollEdit` | Implicit splitting or multi-clip/automation rewrite; reserve four current documents plus retained observations. |
| `SlideClip` | Implicit splitting or multi-clip/automation rewrite; reserve four current documents plus retained observations. |
| `ReplaceClip` | Local original and automation rewrite can copy prior clip payload; reserve four addressed clips. |
| `FitToFill` | Local original and automation rewrite can copy prior clip payload; reserve four addressed clips. |
| `DeleteClip` | Removes/moves existing values; no extra prior payload clone. Retained Undo documents and observations remain charged. |
| `RippleDeleteClip` | Implicit splitting or multi-clip/automation rewrite; reserve four current documents plus retained observations. |
| `RippleInsertGap` | Implicit splitting or multi-clip/automation rewrite; reserve four current documents plus retained observations. |
| `LinkClips` | In-place scalar/identity update; current candidate and typed collection growth cover the edit. |
| `UnlinkClips` | In-place scalar/identity update; current candidate and typed collection growth cover the edit. |
| `AddMarker` | New or replacement owned values come from the supplied payload; existing history keeps prior states. |
| `RemoveMarker` | Removes/moves existing values; no extra prior payload clone. Retained Undo documents and observations remain charged. |
| `MoveMarker` | In-place scalar/identity update; current candidate and typed collection growth cover the edit. |
| `AddEffect` | New or replacement owned values come from the supplied payload; existing history keeps prior states. |
| `InsertEffect` | New or replacement owned values come from the supplied payload; existing history keeps prior states. |
| `RemoveEffect` | Removes/moves existing values; no extra prior payload clone. Retained Undo documents and observations remain charged. |
| `SetEffectParam` | Preset/title/effect/curve edits clone prior payload; reserve four addressed clips plus supplied payload. |
| `SetEffectKeyframes` | Preset/title/effect/curve edits clone prior payload; reserve four addressed clips plus supplied payload. |
| `ClearEffectKeyframes` | Removes/moves existing values; no extra prior payload clone. Retained Undo documents and observations remain charged. |
| `UpsertEffectKeyframe` | Preset/title/effect/curve edits clone prior payload; reserve four addressed clips plus supplied payload. |
| `RemoveEffectKeyframe` | Preset/title/effect/curve edits clone prior payload; reserve four addressed clips plus supplied payload. |
| `SetEffectEnabled` | In-place scalar/identity update; current candidate and typed collection growth cover the edit. |
| `SetClipEnabled` | In-place scalar/identity update; current candidate and typed collection growth cover the edit. |
| `SetClipEnabledCurve` | New or replacement owned values come from the supplied payload; existing history keeps prior states. |
| `CopyClipAttributes` | Copies effects/curves out of existing clips; three source plus two destination clip payloads, per operation. |
| `ConvertLegacyLook` | Preset/title/effect/curve edits clone prior payload; reserve four addressed clips plus supplied payload. |
| `AddLutAsset` | New or replacement owned values come from the supplied payload; existing history keeps prior states. |
| `RemoveLutAsset` | Removes/moves existing values; no extra prior payload clone. Retained Undo documents and observations remain charged. |
| `SetTitleParam` | Preset/title/effect/curve edits clone prior payload; reserve four addressed clips plus supplied payload. |
| `SetClipAudio` | In-place scalar/identity update; current candidate and typed collection growth cover the edit. |
| `SetClipGainEnvelope` | New or replacement owned values come from the supplied payload; existing history keeps prior states. |
| `AddTransition` | New or replacement owned values come from the supplied payload; existing history keeps prior states. |
| `RemoveTransition` | Removes/moves existing values; no extra prior payload clone. Retained Undo documents and observations remain charged. |
| `SetMarkerParam` | Replaces a scalar/string from the supplied value without cloning the prior marker payload. |
| `AddFreezeFrame` | Creates a fresh clip from supplied metadata/scalars; typed clip/vector growth and payload are reserved. |
| `SetClipSpeed` | Local original and automation rewrite can copy prior clip payload; reserve four addressed clips. |
| `SetClipBlendMode` | In-place scalar/identity update; current candidate and typed collection growth cover the edit. |
| `AddAdjustmentClip` | Creates a fresh clip from supplied metadata/scalars; typed clip/vector growth and payload are reserved. |
| `AddSolidClip` | Creates a fresh clip from supplied metadata/scalars; typed clip/vector growth and payload are reserved. |
| `SetSolidColor` | In-place scalar/identity update; current candidate and typed collection growth cover the edit. |

The three newly explicit clone allowances have a large-prior-payload witness using real relink and mix/automation operations: a final-retained-only window refuses as Capacity, a full bounded replay agrees exactly, and Undo/Redo retain the original and resulting documents.

The 41 MiB task, 2 MiB candidate, 128 MiB ordinary/manual pool, 64 MiB native replay window and 512 MiB raw ceiling are unchanged. The **568 MiB resident envelope remains NOT QUALIFIED**, before other metadata including runtime proof metadata. Endpoint proofs share live immutable documents after rebinding; the transported evidence uses weak references and retains no document payload. **Finish and Freeze are NOT guaranteed after a raw-ledger refusal of an ordinary or terminal native publication; Stop and exact retention are.** The non-latching raw-publication fix remains a hard precondition of 2c.

Rust stays at 1.99.1. The lead's CI pin `f270876d` is cherry-picked as `0bed2b9d463ffa2042cb82021e7e3d79f2b03486`, underneath this fix and the later rebased 2a. No toolchain update or 1.100 adaptation was made.

Final full suites **PASS** (passed / failed / ignored): Core **811 / 0 / 2**, Project **476 / 0 / 4**, Agent **1,725 / 0 / 102**, App **1,014 / 0 / 5**. Project includes `test-util`; Agent and App include `slow-tests`; the Linux slow configuration optimizes Core and Project only. The exact restored 12,000-edit App witness also passes separately with unoptimized Core/Project, keeping its five-second production startup limit.

Remaining gates **PASS**: fmt; all-target Clippy `-D warnings` for all four changed crates in default and slow/test-util modes; slow-test lint (**133 / 158 / 27**); shard self-tests (**22**) and slow-test self-tests (**48**); workspace test compilation (**82 executables**); the Windows cross-Clippy wrapper (workspace, Agent slow, App default and slow), all **three isolated Linux shipping builds** and **three MSVC shipping checks**. Windows runtime, hosted CI and paid-provider behavior were not run. Shared-lock waiting is included in gate wall times and excluded from startup/edit measurements.

Comparison cleanup, after verifying no own Cargo process or comparison executable was active, removed **1,955,946,496 allocated bytes**: target usage fell from **27,734,536,192** to **25,778,589,696 bytes**. Small logs, source/binary digests and commands were kept. Completed slow-suite executables and isolated shipping output were also removed at their gate boundaries. At the idle commit boundary, scoped Cargo cleanup removed 2,171 package-output files (Cargo reported 19.5 GiB), preserving the evidence logs. Final allocated `target/` size is **15,346,065,408 bytes (15.35 GB)**, below the approximate 40 GB cap.

Evidence: `target/history-capacity/fix2-final-gates/`, `fix2-current-measure/`, `fix2-final-canonical-core.log`, `fix2-final-prefilter.log`, `fix2-canonical-manual.log` and `fix2-canonical-census.log`, and the pinned 4,000-edit controls under `fix2-startup/final/`. Native rescue remains parked byte-exact outside this change. The unpushed 2a delta is rebased only after this fix commit and checked independently against `3ca9ae3e`. No push is performed.

## Stage 1 fix 3

Fix 3 is built on public `769206fa82f8ebaf58aa5511dd8fb7776b424961`. The final gates below pass; this report accompanies its local commit. The original allocation-disagreement witness is now active and passing. Its pre-fix reproduction accepted `SetTrackSyncLock` at floor 2 on a producer with a spare 4,001-slot baseline, while the tighter writer selected floor 0 and returned identical `HistoryChanged` refusals on both attempts; cold reopen retained the pre-edit cut. Evidence remains in `allocation-floor-parity{,-diagnostic}.log`.

Ordinary selection now executes on tight owned baseline/current/journal values. Deep rematerialization compacts their nested owned arrays and strings. A canonical immutable prefix can extend through the new batch; ordinary proofs never replace their replay documents with a holder's different allocations. Equal-cut memo reuse compares values because the cached proof remains canonical; the receiving actor adopts its checked layout. Native role replay and task doors are unchanged.

The writer derives the producer's absolute chosen floor from the accepted batch's relative order and preceding absolute position. It constructs that exact legal transition, independently performs full canonical admission, and checks the complete history/cursor/batch identities. It does not select a second floor. Direct Save, Save As, copy and adoption remain canonical selectors. The regression now selects floor 3 on both holders and publishes/cold-reopens exactly. Twelve real-lease baseline/cursor/current/vector cases (producer only, writer only, both) pass; 30.8–35.2 MB of injected actual geometry becomes 14,633 typed retained bytes on both actors under an unchanged 87,406-byte test allowance.

Physical spare memory is not discounted: at lease binding and admitted mutations the actor adopts the checked replay allocations, preserving history values and its runtime authority/tickets. An actual floor's retained unpublished source is tightly rematerialized once and checked against its existing pool remainder. This keeps floor choice independent of physical layout while bounding the holder's resulting allocations. Allocator and natural-geometry measurements, mutation pairs and final gates follow below when complete.


Ordinary edits now move the Undo floor inside the Core admission transaction when the complete history is full. Save and every ordinary edit door call the same `select_admitted_history` policy. The selected cut preserves the document, accepted absolute batch position and retained cursor. A too-large individual edit refuses before acknowledgement with `ManualHistoryCapacity`: **This edit is too large for one step. Try it in smaller parts.** No bound is raised.

### Floor, headroom and transaction

The selector first finds the fewest oldest steps whose removal produces a fully admitted cut. It then tries dropping another `ceil(retained Undo depth / 8)` steps from that candidate, keeping at least the newest step. Every candidate is measured after moving its actual baseline. If that extra drop costs more than the hard window permits, the first fitting cut remains available. The ordinary door selects this canonical cut. The manual writer applies and fully checks the carried floor, closing ruling point 3 without a second selection. Direct Save and copy remain selectors.

For a new Undo step, the search includes the cut retaining only that newest step at its actual pre-step document and absolute position. If that one-step cut fits, it is an available fallback even when a heavier earlier baseline makes other floors fail. A refusal means that real one-step candidate cannot fit; the check never substitutes an empty or cheaper document for its baseline. An existing coalesced gesture remains one Undo step. The generated setup failures below have no admitted starting cut under their deliberately smaller limits; they are not a saved project becoming stuck after accepting an edit.

A fitting history stays unchanged. Between floors the complete effective ordinary cut remains bounded by the existing ordinary admission, live, replay, command and portable encoded limits, normally a 32 MiB effective per-cut window. Proof extension executes only new commands; full prefix replay is paid when a floor actually moves or a proof cannot be reused. Flooring changes no accepted command on disk. It discards the oldest Undo prefix under the same policy Save uses.

Manual ingress stages the floor, proof, JSON cache and encoder state in the edit transaction. Late encoding, queue, predicate or cache failures restore document, revision, Undo/Redo, journal, sequence, predecessor and every cache byte/index. Ordinary direct/MCP endpoints retain at most one exact unpublished source, without a source chain. Its typed allocation, including its wrapper, is bounded by the ordinary pool remainder, normally 96 MiB beside the selected 32 MiB cut. Save publishes from that source, consumes it only on acknowledgement, and preserves it on failure. A second unpublished floor returns temporary backpressure until that acknowledgement. GUI accepted-envelope streams retain no such extra source. This is a bounded per-endpoint handoff, not qualification of the aggregate raw ledger.

Canonical prefix reuse preserves the proof's canonical baseline allocation. Equal-cut value reuse cannot supply or substitute holder allocations; the actor adopts the checked canonical state when needed. Astra's spare-capacity/rebound-cursor case and equal-cut parity regression are retained without discounting any allocation charge.

### Canonical floor choice and exact writer verification

Allocation geometry is outside the selection domain. The ordinary resolver deep-rematerializes the owned baseline/current documents and journal; all nested owned arrays and strings are tight. Its private construction provenance permits prefix extension only from another canonical replay. A generic/holder proof, or a proof subsequently rebound to holder allocations, cannot enter that reuse path. The ordinary endpoint retains canonical replay documents rather than replacing them with value-equal holder documents. Native replay retains its existing policy.

A lease binds only an already admissible value cut, then compacts the actor to the checked allocations without changing document, revision, history, authority, pending floor ticket or accepted messages. Each admitted ordinary mutation similarly installs the checked allocations inside its rollback transaction. Layout-only changes keep the existing JSON-cache update path; only an actual floor rebuilds it. Publication keeps the canonical decoded allocations whose reservation it checked; it no longer swaps an uncharged holder allocation back into that reservation. A floor's one unpublished source is tightly rematerialized and charged against its existing 96 MiB remainder.

The accepted last batch order carries the producer's selected floor: `previous absolute position - accepted batch.order`. The new batch itself is retained. The writer applies a legal step-boundary transition at that offset, runs the complete ordinary predicate, and checks full history/cursor/batch digests. Its own sealed prefix may skip already proved execution, but no wire field supplies admission evidence. A dedicated producer intentionally chooses a different, still-admissible floor (4 where the writer's selector would choose 3); publication and cold reopen match that exact choice. This is why replacing verification with re-selection is a regression even when the two current selectors usually agree.

For an honest producer, the writer begins at the same acknowledged predecessor values, executes the same typed command, uses the same fixed lease/store limits and canonical construction, and verifies the carried legal floor. Thus its exact predicate sees the same values and charges the producer admitted. A capacity disagreement from physical allocations is unreachable in that sequence; no retry policy hides one. Authority, malformed-command and identity refusals retain their existing typed paths. No native task door, role constant or raw ceiling was changed.

The real-lease geometry generator covers baseline, cursor and current document allocations plus batch/cursor vector allocations, each on the producer, writer and both. All twelve admitted cuts publish and cold-reopen exactly with the same floor. The maximal injected cases start at 30,824,845–35,224,845 typed bytes and finish at 14,633 bytes per actor under the same 87,406-byte test cap.

The isolated allocator witness starts with 280,016,255 heap bytes (308,024,845 typed), deliberately above the ordinary role bound. Its cure peaks at 280,039,796 bytes, only 23,541 bytes above that already inflated entry, then retains **24,687 actual heap bytes / 31,117 typed bytes** under the unchanged 32 MiB bound. Two instrumented executions take **4.86–5.36 ms**. Compaction is paid at lease binding or the admitted transaction; baseline/current/journal normalization and rare unpublished-source rematerialization are the allocations involved. This single-actor measurement does not qualify the aggregate 512 MiB ceiling.

The natural row uses real additions and removals: eight surviving tracks and 69 solid clips, with a seven-slot/six-clip vector left by editing, without injected capacity. Both initial executors report 292,171 bytes; both initial Saves choose floor 3. Across 120 subsequent edits there are **60 floors, zero refusals, zero old/new floor disagreements**, and an exact cold reopen. Thus this natural row does not establish the old shipping trigger; the stronger injected allocation cases close the independently blocking disagreement as ruled.

### Undo pays for every Redo on the way back

The chosen change is a return reservation; navigation still appends its existing journal batches. An Undo passes only if its resulting cut and **every intermediate cut** obtained by draining Redo fit all ordinary bounds. An Undo that cannot make that reservation refuses transactionally with `ManualUndoCapacity`: **Can't undo further. This project's undo history is full.** A new edit clears Redo before selecting a floor, as before.

The forecast does not replay one future history per Redo. Redo moves existing document and operation payloads between current/Undo/Redo. Its only journal growth is a payload-free Redo batch with known numeric counters. Decimal-range sums give the exact additional encoded batch bytes, including commas and counter digit changes. The reservation uses the final batch-vector capacity, both cursor-vector maxima, the largest encoded and allocated current document across the entire return path, and the proven prefix's dynamic replay peak. It includes live and canonical-decoded allocation charges; sharing equal values cannot erase the latter. These quantities stay constant or decrease while Redo drains. The ordinary structural/record checks reserve the same added batches and maximum current-reference length. All arithmetic is checked.

Cold resolution and publication apply this same return predicate before admitting a cut with Redo. No wire flag can supply proof, and a capacity refusal never becomes corruption. A lazy scalar memo retains canonical maximum charges, not boundary documents. One Undo needs linear capture/size scans and a single prefix extension, not a quadratic sequence of full future replays. Existing metadata scans still scale with history and document size; no constant-time claim is made.

The real-lease 180-edit witness passes unchanged in shape. It accepts all 180 edits through 25 floors, with maximum Undo depth 33. After close/cold reopen, the excursion takes 27 Undos and all 27 Redos with zero refusal, including another cold reopen at the Redo cut. The first Undo may legally move the floor; the final document and complete kept cursor match the pre-excursion cut after that exact floor, and the accepted navigation batches append after its exact kept journal. The six seeded pair witnesses cover 12 tiny/heavy cases, 432 requested walk steps and 60 cold reopens: 284 Undos accepted, six Undos refused byte-identically, and 233 walk Redos accepted, plus all final drains. No Redo refuses.

| Document / loop | First floor | Accepted / refused edits | Floors | Undo depth after first floor |
| --- | ---: | ---: | ---: | --- |
| 400 clips, 400 effects, 3,200 keyframes | Edit 6 | 18 / 0 | 7 | 4–5; final 4; cold Undo/Redo 4/4 |
| Plain 300 clips, no effects | Edit 36 | 108 / 0 | 13 | 30–35; final 30; cold Undo/Redo 30/30 |

These loops each exceed three times their observed retained depth. The low-water mark preserves nearly all heavy-document steps while leaving hundreds of steps for the long small-edit case. Exact depth depends on document and payload size; the fraction is not a promise of a fixed number of Undo steps.

### Ordinary acknowledgement doors

| Door | Before acknowledgement | Witness |
| --- | --- | --- |
| GUI manual single, batch and coalesced commands | Mandatory encoder guard inside `ManualIngress::execute`; staged floor commits only with the accepted message | Continuous edits, heavy copy refusal/control, lower writer limit, late encoder/cache rollback, coalesced gesture |
| Ordinary MCP single, plans and LUT batches | Actual revision-gated command goes through persistent ordinary admission and the same Core transaction | Existing single/plan/LUT refusal tests; ordinary source/Save/retry and full Agent suite |
| Legacy merge, cherry-pick and approved proposals | `branch::apply_to_live` passes its owned operation vector to the admitted revision-gated batch | Shared-funnel refusal tests and full Agent suite |
| Owned import and colour interpretation | Saved path stages through ordinary publication; unsaved path selects before installing document or inputs | Unsaved import rollback and complete saved import/interpretation suites |
| GUI Undo and Redo | Same transactional guard, with the complete return reserve whenever Redo remains | Guarded cold cursor round trip and seeded excursions |
| Save, Save As, copy and cold adoption | Same selected cut and full admission; runtime source is consumed only after exact publication | Exhaustive/seeded floor agreement, source retry, copy/recovery suites |
| Native task edit/Undo/Redo and native candidate merge | Existing role predicate and pre-mutation guard, unchanged in this commit | Existing native lifecycle/cleanup and candidate reservation tests |

The independent exhaustive oracle agrees on 10,000 seeded inline histories and on 60 additional payload histories at 180 cap neighbors. Payload shapes include copy fanout, split-then-delete and multi-operation batches. Real leases admit, publish and cold-reopen 164 cuts exactly; four attempts refuse before acknowledgement, and 59 selected cuts move the floor. Every door-admitted cut in this set is accepted by Save and cold open. The twenty-clip heavy batch still refuses with the new text and unchanged bytes/stacks/revision/queue; the ten-clip control publishes and reopens exactly.

The final 1,000-keyframe census still accepts copying to **15 destinations, 16 clips including the source**, on the twenty-clip document. Its typed replay peak is 31,929,300 bytes, retained charge 14,306,493 and encoded history 792,678. Sixteen destinations require a 33,704,229-byte peak and refuse the unchanged 33,554,432-byte window. Nineteen destinations need 39,037,324 bytes while retaining 17,674,261; retained room is not the refusal. Copy's 2,280,354-byte temporary allowance is peaked per operation, not held cumulatively (the hypothetical nineteen-operation sum would be 43,326,726 bytes).

The copy-floor witness measures the actual candidate, not a subtraction from the old peak:

| Effect-bearing clips after copy | Full journal peak, bytes | Floored candidate peak, bytes |
| --- | ---: | ---: |
| 2 | 7,030,474 | 7,127,151 |
| 4 | 12,254,084 | 14,221,702 |
| 8 | 22,708,660 | 28,416,085 |
| 16 | 43,617,880 | 56,804,902 |

The 16-clip row is an above-limit Core measurement, not an admitted ordinary publication. These increases are why an extra floor is checked again and why the first fitting floor is retained as a fallback.

The twelve setup failures all return typed `Capacity("history memory")` before any lease. Exhaustive selection finds no fitting prefix at those deliberately lowered caps. They are four prefix shapes, each tested at three adjacent values:

| Generated shape | Offered cap range, bytes | Minimum full-prefix allowance | Shortfall |
| --- | ---: | ---: | ---: |
| Case 25, split/delete | 229,711–229,713 | 230,186 | 473–475 |
| Case 29, multi-operation | 848,726–848,728 | 848,730 | 2–4 |
| Case 44, multi-operation | 566,735–566,737 | 566,775 | 38–40 |
| Case 45, copy fanout | 1,070,275–1,070,277 | 1,070,278 | 1–3 |

Those caps were derived from a different later candidate, not from the starting prefix. Adding the prospective step permits a different legal floor and baseline. The setup prefixes themselves cannot replay in the offered window; their exact minimums are measured by full-predicate search. These twelve refusals precede any accepted guarded edit; they are separate from the pre-fix allocation disagreement above. Older fixed-floor/disclosure witnesses now independently enumerate full-admission cuts and apply the same ruled headroom. The fresh-reader pressure fixture calibrates its artificially lowered branch cap against full admission with the newest step retained, while preserving its exact shared-pool boundary and quota-retry assertions.

### ManualBackpressure contract

`ManualBackpressure` now means that an acknowledgement can make room for this same command. The accepted-message window stays 262,144 bytes and 64 messages. Own encoded message size is checked against that fixed bound before occupied-byte/count checks; it is not inferred from current occupancy.

| Site | Result |
| --- | --- |
| `actor/manual_history.rs`, accepted-message admission | Own message above 262,144 bytes: final `ManualHistoryCapacity`. A fitting message blocked by pending count/bytes: `ManualBackpressure`; acknowledgement admits the identical retry. |
| `actor/manual_history.rs`, pending floor ticket | `ManualBackpressure` only while the existing floor awaits its Save settlement/acknowledgement. |
| `actor/manual_history.rs`, one unpublished ordinary source | `ManualBackpressure` until Save acknowledges that exact source; a source that itself exceeds the static source allowance is final `ManualHistoryUnavailable`. |
| `actor/manual_history.rs`, cache rebuild/update, identity encoding, malformed sequence, arithmetic/invariant or untyped `check_history` failure | Final `ManualHistoryUnavailable`; explicit typed guard refusals remain typed. |
| `attachment.rs` | No production backpressure return site; boundary contract tests now distinguish an oversized empty-window message from an occupied fitting window. |
| Project `manual_history.rs`, predicate fallback and encoder fallback | Capacity stays final history/Undo capacity. Every other preparation failure is final `ManualHistoryUnavailable`; no generic backpressure fallback remains. A poisoned proof cache is preparation I/O failure, not a capacity refusal. Intrinsic source/epoch failures are checked before pending source/ticket backpressure, so a pending acknowledgement cannot mask a final failure. |

The new final variant's exact text is **This edit could not be prepared. Your timeline is unchanged.** It adds no retry policy. The failure matrix checks cache rebuild before/after flooring, cache update, predicate errors, encoder errors, malformed sequence and overflow, unchanged state on repeat, and permanent failure despite acknowledgement. The exact-byte test covers 262,145 bytes in an empty window, 262,144 bytes in a full window and success after acknowledgement; count-window retry and an oversized message offered to an already full count window are also pinned. Project's actual poisoned-cache request and encoder-size rollback exercise its two adapters.

### Coalesced gesture and publication cost

The active 400-clip/effect, 3,200-keyframe gesture witness sends 120 distinct slider sub-batches while the real publisher runs. Every accepted sub-batch is acknowledged, retained in order under one Undo step, and cold-reopens exactly. Every refusal must be `ManualBackpressure`, never history capacity. Counts and timings are measurements, not acceptance assertions. The strict all-120/final-119 version remains separately ignored with the GUI resend follow-up reason in `ci/ignored-tests.txt`. The lead assigned GUI retry to the App worker; no queue limit or Project retry is changed here.

The final full-Project run measures 43 accepted and 77 backpressured sub-batches, with all 43 accepted values in order and cold-reopened exactly. The first measured queue refusal is sub-batch 38 with 35 messages and 261,430 queued bytes, leaving 714 bytes. Peak queue usage is 261,514/262,144 bytes. Request median/worst is 38.697/173.144 ms; publication median/worst is 621.417/711.387 ms under this run's load. The saved exposure is 111 versus the requested final 119, the known GUI resend gap. Undo depth moves from five to four through a floor. No sub-batch receives history-capacity refusal, and no accepted value disappears.

An earlier instrumented WIP run, before canonical allocation selection, measured **523.210 ms mean per publication** over 44 publications. Exclusive means were shadow admission 38.560 ms, transport decode/replay 74.567 ms, record read/replay 92.665 ms, JSON preflight 25.133 ms, record encoding 3.089 ms, capture encoding/hash 9.628 ms, seal encoding/hash/remaining I/O 4.175 ms, disk sync 139.065 ms, other publication work 96.365 ms and other manual-commit work 39.964 ms. This is a rough timed split, not CPU attribution inside the I/O-bearing scopes. There were 705 sync calls, about 16 per publication. The host loops and publishes **one new sub-batch per commit**, carrying its cumulative kept journal, rather than combining all pending messages into one publication. No comparable stage-1-base timing was available, and no old head was rebuilt. All six instrumented files were restored byte-exact.

### Cost and gates

The final sampler uses unoptimized Core, Project and Agent, four pinned CPUs (0–3), niceness 19 and the same real-store/manual-lease method as fix 2. It does not wait for a quiet machine: one-minute load is 2.55 before and 2.47 after. All ten subsequent edits in each row are accepted, published and cold-reopened exactly.

| Setup edits | Retained batches before each edit | Hand edit median / worst, ms | Save median / worst, ms | Accepted |
| ---: | ---: | ---: | ---: | ---: |
| 1,000 | 1,000–1,009 | 130.875 / 131.654 | 832.453 / 857.104 | 10 / 10 |
| 12,000, after first Save | 1,976–1,985 | 251.497 / 257.144 | 1,500.266 / 1,529.542 | 10 / 10 |

The 12,000-edit floor stays at 10,024 throughout all ten samples; the 1,000-edit floor stays zero. Neither row pays another floor or full-prefix replay per edit. Both still capture, encode, hash and validate metadata over retained history, so they remain O(H) per hand edit; the measured ratio is about 1.92 for about 1.97 times the retained batches. The canonical proof executes only the new batch on a matching prefix. A changed Save verifies its exact chosen cut and performs publication/graph/I/O work; cold open performs one complete linear replay and complete admission. These are debug timings, not shipping-build latency qualification. The separate unchanged 12,000-edit production-startup witness passes with unoptimized Core/Project on four CPUs and its five-second startup limit intact.

All seven negative mutations fail in executed tests at their intended assertions: no door floor; no Redo return reserve; an end-only return check; maximum-fitting rather than step headroom; allocation-sensitive selection; writer re-selection; and missing local compaction. Sources restore byte-exact, and both the original return-reservation controls and all three canonical controls pass after restoration. In the heavy-middle/final-light Core shape the intermediate peak is 5,645,790 bytes versus 5,270,752 at the final cut; the complete return reservation is 5,647,937 bytes.

Current complete gates: workspace build and fmt; all-target Clippy `-D warnings` for Core, Project, Agent and App in default and slow/test-util modes; Core **818 / 0 / 2** across 22 targets; Project **500 / 0 / 5** across 24; Agent **1,730 / 0 / 102** across 31. Counts are passed / failed / ignored. App passes **1,014 / 0 / 5**. Workspace all-feature test compilation passes **82 executables**. The prescribed Windows script passes workspace/all-target Clippy, Agent slow Clippy, App default/slow Clippy, all three isolated Linux shipping builds and their three MSVC shipping checks. Windows execution is not claimed. The Python-driven Cargo runner inherited niceness 19; that actual priority is recorded, rather than describing those processes as niceness 10. Remaining Cargo gates use a shell runner at niceness 10. The performance sampler retains the same four-CPU/niceness-19 method as its baseline. Workspace compilation and the separate startup check ran from the corrected shell runner at actual niceness 10.

Both previously red fixture expectations are corrected. `w13_every_publication_fits_fresh_open` now derives its lowered branch cap by binary search over full canonical selection with the newest step retained; its retained-memory lower bound still independently calibrates the shared-pool boundary. All 13 tests in that target pass. The legacy Agent merge/cherry-pick witness now expects a 4,000-small-edit source plus one fitting step to floor and continue; a separate oversized single marker, derived from the existing `HISTORY_BYTES` constant, still refuses transactionally. The full Agent suite passes that changed expectation.

Slow-test lint currently reports **146 tests / 159 allowlist entries / 27 equipped-session skip sites**; its self-tests pass **48**, and shard self-tests **22**. Evidence is in `target/history-capacity/fix3/final-gates/`, `canonical-mutations/`, `mutations/`, and the original allocation failure logs. A source-patch digest verifies that production code stayed unchanged during this gate run.

At the native task role bound, the unchanged pre-edit guard returns `RunAdmission` with **Task history limit reached. Finish or freeze this task before making more edits.** It applies no refused edit. The previously admitted cut remains eligible for Finish, Freeze and Stop under that monotonic history bound. There is no newly introduced state that can neither continue nor end cleanly. The accepted raw-ledger publication exception is unchanged: **Finish and Freeze are NOT guaranteed after a raw-ledger refusal of an ordinary or terminal native publication; Stop and exact retention are.** Native doors, rescue and release fixes are outside fix 3.

An earlier draft's all-feature compilation reached **48,523,587,584 allocated bytes**, above the requested cap; that breach remains recorded. For the final gate, pruning completed executables **before** workspace compilation reduced 31,727,394,816 bytes to **22,020,059,136**. Compilation reached **31,410,229,248**; deleting its 82 completed compile-only outputs reduced it to **22,021,914,624**. Root fixture binaries, libraries, small logs and results were retained. After Windows/shipping, the isolated shipping directory and Python caches were deleted. Final `target/` is **23,365,373,952 allocated bytes (23.37 GB)**. All 15 parked rescue files still match their original hashes; `/tmp/kinewright-*` contains only the shared lock. Every started build and test has completed.

The source-patch SHA-256 tested by the final gate is `f08ef337e62f365e866f5d8f8e4648fa1f523648f42562b4c56efe0f90e0be51` (Rust, manifests, CI lists and scripts against the public parent; this report is excluded). It still matches immediately before staging. No 2a fix-1 or rescue changes are mixed into this commit.

The role constants remain task 41 MiB, candidate 2 MiB, ordinary/manual pool 128 MiB and native replay window 64 MiB, with raw ceiling 512 MiB. The **568 MiB envelope remains NOT QUALIFIED**, with the existing holder audit and publication-latch limitation. Windows runtime, paid/equipped providers and hosted CI are unrun locally. No migration, old reader, toolchain update, push or signal was performed. The native rescue stays parked; 2a fix 1 is next.
