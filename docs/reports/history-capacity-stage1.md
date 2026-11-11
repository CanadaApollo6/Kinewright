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
