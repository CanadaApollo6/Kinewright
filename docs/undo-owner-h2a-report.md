# H2a part 2 qualification

The new owner records remain beside the old history. No production history, audit, lineage, retention or writer reader has moved. No push, app launch or subagent was used. The Core operation kernel's validation, duration and identity passes remain unchanged.

## Local commits and checks

| Commit | Scope | Core + Project tests before commit | Clippy |
| --- | --- | --- | --- |
| `e9e5dfdc` | Event-driven owner, ledger, lineage, resources and reservation | 1,091 passed, 6 ignored; 109.26 summed test-seconds | Workspace passed, 21.24 s |
| `2a5c59a7` | Review guards, chronology and real-capacity diagnostics | 1,097 passed, 6 ignored; 115.43 s | Workspace passed, 20.36 s |
| `a3f36da9` | Candidate/shared workspace profiles; unused charge removal | 1,099 passed, 6 ignored; 112.75 s | Workspace passed, 28.56 s; feature example 0.15 s |
| `9405a998` | Owner capture, stored codec, open/install, transition evidence and differential | 1,130 passed, 6 ignored; 133.49 s | Workspace 22.08 s; benchmark feature 11.82 s |
| `41171a3b` | Requested-role ingress cap and counter-evidence guard; candidate benchmark assertion | 1,130 passed, 6 ignored; 119.38 s | Workspace incremental 0.19 s; benchmark 4.72 s |

Formatting passed before each. The complete Undo slow-feature binary also passed 37 tests in 32.14 s after the profile changes. Later gates and measurement source are appended below when complete; queue time is excluded from Cargo/test timings.

## Delivered records and semantics

- Core reports the accepted command's first step key and permits owner preparation inside its existing atomic transaction. A rejected metadata update leaves document, stacks, ledger, lineage, resources and counters unchanged.
- The ledger preserves original command key and payload digest across epochs. Events move Do entries through applied/undone/redone/folded/discarded. Recreation is a sticky bit from later accepted creation effects. It cannot resurrect a discarded command. Installation derives fate from exact installed stacks or matching bounded fold facts, never from a snapshot fate flag alone; selected S can restore the same command after another history line discarded it. Do entries record the install epoch that last changed their fate.
- Transition approvals contain consent id, direction and an 88-byte step-evidence value: first command key/digest, command count and ordered digest of every key/digest. No Core command/key is added for the approval. Their answers are derived from installed command locations. Undo holds while not applied; Redo holds while applied. An incompatible prefix is uncertain. Ordinary Redo closes the gesture; restoring an older open snapshot can extend a step while preserving its approved prefix.
- A fixed exclusive fold frontier and bounded facts resolve known cases. A folded Do fact carries a fixed prefix stamp ending at its approved command, so it can also settle a later transition when the complete target evidence matches. A different grouping cannot turn an individual folded command into proof of the approved prefix. No operation payloads or unbounded folded-command archive are retained.
- The exact pre-approval folded-snapshot sequence reports uncertain when the subject is absent, older than the installed frontier and lacks matching evidence. Unknown is distinct from held, kept, reversed and discarded in answers and counters. An analogous pre-approval Do snapshot cannot occur: Do acceptance and its approval entry are atomic.
- Lineage retains baseline seeds and per-command origins. The same replay builds each queried document and its occurrence/ancestry map. Fold advances seeds; no per-boundary lineage maps are retained. Capacity accounting uses actual borrowed payload capacities, including reserved opaque ids and paths, without a compacting document copy.
- Resources are the baseline references plus reference-counted payload references from both stacks. They shrink when payload commands are dropped from Redo, when folded payload references transfer to an exactly scanned baseline, and at open/install after reconstruction. At each point all reachable states are still covered; independent stored captures own separate graph roots/leases. Real LUT bytes and a WAV manifest/table/media closure survive collection through a record, and become collectible after its last root is released.
- The compact store body carries keys/digests once per command, origins at those boundaries, seeds, resources/bindings, ledger, frontier and bounded facts. Core canonical digest remains snapshot identity; the store's separate object hash also binds owner metadata. Raw body storage avoids the old per-node inflation. New role caps leave old record caps untouched.
- Public stored open reserves the requested role before reading. The advertised shell length is capped for that requested role before store allocation, even if the record claims another role. Owner validation runs alongside untrusted Core replay. Empty steps, wrong targets, impossible command counts, malformed digests, absent active commands, wrong origins/seeds/resources, role/policy mismatches and conflicting keys/digests are typed refusals.

## Differential and certain-answer differences

[The full differential report](undo-owner-differential.md) lists every observed class of old certain control answer which differs from the specified transition contract, with reproducible sequences. [Raw witnesses/counts](measurements/undo-owner-differential.json) retain them. There are 6,480 generated transitions across 67 operation kinds, two policies and two seeds, plus two folded-snapshot probes: 27 accepted approvals, 272 DoIfRevision and 284 DoBatchIfRevision acceptances recorded as plain commands, 48 narrowly recognized Do target-shortcut exemptions, one uncertain observation and 54 control-difference observations in four classes. No blanket mismatch exemption exists. The same-command snapshot restoration and its undone mirror also invoke the actual old audit and agree without exception.

The audit adapter calls the actual old target-presence test and retained-batch walk, supplied with the selected line's complete kept-cut coverage. It is a semantic differential; it does not exercise the full writer/GUI publication and consent authentication chain. The separate old lineage journal and old resource-retention function rebuild/check every reachable boundary. Product end-to-end ownership, transport and approval fencing remain H2b work.

## Deliberate wrong changes, all reverted

| Wrong change | Actual failing assertion |
| --- | --- |
| Dropped Do approval marked applied | `DISCARDED_APPROVAL_NEVER_KEPT` |
| Undo drops all payload resources, ignoring Redo | `REDO_PAYLOAD_RESOURCE_SUPERSET` |
| Fold leaves the old occurrence seeds | `FOLD_ADVANCES_OCCURRENCE_SEEDS` |
| History drop does not release its reservation | `HISTORY_DROP_RELEASES_RESERVATION` |
| Transition entry stores a holding flag and the getter reads it instead of stacks | `APPROVED_UNDO_REDO_REVERSED` |

The last mutation added an actual stored bool, set it true on approval, and replaced derivation with that field; the failure was Holds versus Reversed. The normal record-key assertion also forbids a fate flag. Logs: this worktree's `target/hx-part2/owner-mutation-*.log` and `owner-mutations-run.log`.

## Ten Part 1 review follow-ups

| Item | Commit / disposition | Wrong change and named assertion |
| --- | --- | --- |
| 1: bounds after all transitions, reserved paths | `2a5c59a7` | Cold replay uses anchor bound; Redo keeps old bound; replay takes `before`: each red at `EVERY_TRANSITION_ENCODED_BOUND`. Path length replaces capacity: `RESERVED_PATH_CAPACITY_REFUSED`. The differential also asserts `EVERY_TRANSITION_LIVE_BOUND` without copying. |
| 2: populated install resets cache | `2a5c59a7` | Delete cache reset: `INSTALL_REPLACED_CACHE_DOCUMENT`; the test also walks all boundaries and Undo/Redo. |
| 3: typed map payload floor | `2a5c59a7` | Delete `.max(operation.heap()?)`: `MAP_PAYLOAD_TYPED_CAPACITY_FLOOR` with 50 map entries. |
| 4: digest and duplicate id refusals | `2a5c59a7` | Skip digest: `PAYLOAD_DIGEST_MISMATCH_REFUSED`. Remove the redundant duplicate/position/order guards together: `DUPLICATE_COMMAND_ID_REFUSED`. Removing only one redundant guard still refuses the input. |
| 5: strictly increasing command epochs | `2a5c59a7` | Drop full-key chronology check: `COMMAND_EPOCH_CHRONOLOGY`. Walk is Undo then reversed Redo. |
| 6: kernel work is separate | `2a5c59a7`, docs | Counter definition and measurements explicitly preserve kernel validation/duration/identity passes; no incremental kernel validation is attempted. |
| 7: clip growth terms | `a3f36da9`, explicit permitted fallback | Conservative formulas remain. A touched clip omits enclosing vector doubling and implicit fan-out. The four-to-five clip witness proves the proposed local-only replacement unsound. A smaller sound formula needs maintained capacity observations; another document search on each command is disallowed. The 67-kind matrix passed; copy/ripple/split paired before/after numbers are in undo-measurements.md. All still perform one exact document measurement. |
| 8: Step size | `a3f36da9` and docs | Accepted old Step is 120 bytes; after removing `before`, the benchmark prints 104. StepCommand 72, Operation 160, Clip 240, identity effect 312, command effects 40. |
| 9: unused `StepCharge.before` | `a3f36da9` | Deleted. Its prior deliberate substitution had already failed `EVERY_TRANSITION_ENCODED_BOUND`. |
| 10: candidate workspace | `a3f36da9` plus owner profiles | W=16 MiB, R=2 MiB, peak=18 MiB. Own-role arithmetic subtotal 12,340,736 bytes, ten percent slack 12.946 MiB, rounded to 16. Requested-role ingress cap guards that input assumption. |

All nine reviewer mutations actually failed and were restored; logs are `target/hx-part2/review-mutation-*.log`. The small-profile arithmetic and clipping probe binaries/raw hashes are in [undo-measurements.md](undo-measurements.md).

## Readers, installation doors and H2b

[Every shortcut reader, its file:line and use](undo-audit-readers.md) is inventoried. The draft removal claims/counts and retained-work recovery guidance need an H2b installed-document identity-effects check only if their chosen contract requires a certain final-document answer. The host composer and wire page can forward uncertainty; rendering sites already have an uncertainty disclosure. No second approval-satisfaction answer is built.

That inventory also lists snapshot doors, including task/specialist reopen, host foreground save/adoption/recovery, selected branch/checkpoint/rescue and GUI activation. Task/specialist reopen at `kinewright-agent/src/durable_branch.rs:173` is a direct task-history restoration door. Private consent previews and the display-only host shadow are distinguished. H2b must decide which exposed re-application door requires fresh approval; none is added here. The exact design paragraph is also updated in the requested main-worktree `.review/arch/history-rework.md`, mirrored in undo-owner-design.md.

H2b still has to wire one shared pool into actual history/capture lifetimes, replace persisted task/candidate cap call sites with the new-kind profiles, build the writer door and snapshot transport/fencing/publication integration, choose recorder ownership, move readers, decide final-document checks and re-application consent per reader/door, run the maximal-fixture task exit test, then delete the old history/floor systems. Existing shared-store input/metadata/media budgets stay separate from the history holder/scratch pool; the pool is not a whole-process RSS ceiling.

No Windows, hosted CI, full workspace test execution, GUI, GPU or live provider qualification is claimed. Linux workspace build, workspace test compilation and the required Core/Project tests are the local gate scope. Performance misses and the retained conservative clip formulas remain explicit follow-ups; no data-loss or bound failure is claimed repaired by a timing result.

## Measurement result on final implementation

Source `41171a3ba37fca24a8e52d0f07cc7ab737140b9a`, release SHA-256 `9b75633724c4325dbe5b210588ed27ccd2fd12409e6353b4e87199226d10d03f`. All ten lanes completed without unexpected errors. Hand edits meet 1.5× paired Core in 5/5 pairs at both depths; validated open stays below one second in 5/5; gesture Redo and the walk p95/worst ceilings pass 5/5. Strict folding misses: 123/160 samples are below 2 ms, worst 2.847044 ms. Gesture apply passes 4/5: pair 4 is 45.881833 versus 28.634651 ms, ratio 1.602319 rather than ≤1.5. Cold Undo is 269.587079 ms median versus historical accepted 252.951; this is unpaired and not a demonstrated causal regression. Edit-depth sample ranges overlap in 4/5 lanes, so strict flatness is unproven. The initial 9405 run's 35 folding misses are retained as well.

The complete table, phase counters, raw hashes, machine contamination, encoded/decoded command costs, actual role memory and cap arithmetic are in [undo-measurements.md](undo-measurements.md#owner-qualification-h2a-part-2). No timing ceiling, fixture or kernel pass was changed to improve the outcome. The expected-role/counter guards are validation fixes. Their code is covered by `OPEN_REQUESTED_ROLE_CAP_PRECEDES_RECORD_ALLOCATION`, `OVERSIZED_OPEN_HAS_NO_PARTIAL_LEASE`, and `TRANSITION_EVIDENCE_CANNOT_EXCEED_ACCEPTED_POSITION`.

## Final local gates and artifacts

On the unchanged implementation `41171a3b`, the report-commit gates passed: Core + Project 1,130 passed / 6 ignored in 119.63 summed test-seconds (warm compilation 0.14 s); workspace clippy 0.20 s; formatting clean; `cargo build --workspace` 26.91 s; `cargo test --workspace --no-run` 53.36 s, 84 executables; slow-test lint 142 tests / 159 allowlist entries / 27 equipped-session skip sites. Earlier gate runs are retained rather than replacing the final summaries. All Cargo commands used the requested FFmpeg setup, shared flock, four jobs and nice level; no nested lock was used.

The first owner run is `docs/measurements/undo-owner-9405a998.json.gz` (gzip SHA-256 `7cfc5c73362e9925f5cffab2ddcada7fc647cba69b17bde5b5b906123e674591`). Final source raw pairs are `docs/measurements/undo-owner-41171a3b.json.gz` (gzip SHA-256 `01f19d0778f21968ddd458662cfafca882e912788a22041c47b07ab7cc21bc91`). Per-role values include their binary source/hash in `undo-owner-roles-41171a3b.json`; the chart is `undo-owner-performance.html`. Full command logs, reverted mutation logs and standalone binaries are retained under this worktree's `target/hx-part2/`.

Target storage peaked below 40 GB (37 GB observed). Removing only obsolete executable artifacts from this worktree's `target/debug/deps`, while retaining current-gate executables, brought it to 26 GB; final observation was 27 GB with more than 400 GB disk free. No shared lock file or another worktree's files were removed. Every owned build, test, role probe and timing run completed before handback.
