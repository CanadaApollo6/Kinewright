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
