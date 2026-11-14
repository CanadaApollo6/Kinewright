# Operation-history qualification (H2a)

Owner qualification is appended below: hand edits/open pass, while strict folding and one gesture-apply pair miss their ceilings. Both raw owner runs are retained; shipping readers still use the old history.

The performance revision meets every explicit timing ceiling on the same heavy fixture in five alternating release pairs. An ordinary edit now adds one document copy and no exact document measurement; the existing kernel passes remain unchanged. Strict depth flatness remains unproven: the 32-sample ranges overlap in all five lanes, but the ranges of lane medians are disjoint, with a 7.3% increase from 1,000 to 10,000 steps. Old history remains in use in every product path.

## Performance revision and reproduction

Measured source: `75dc1dad774263bc1236c7f19495e2d6fe968aef`.
Release executable SHA-256: `cac096cd9b573340f56409778d3bf0f72fbeb34b6f21d37ab9576c45d8b5f5aa`.
Raw JSON SHA-256: `390a64e68d8775b24f10702fddeec8cbf899b884a40279ea078c5f5973134d2f`; deterministic gzip SHA-256: `9200041a77a4b5880415cc87199433dce406738a88fdc21cce357bffe6a574bf`.
Complete samples, counters and machine observations: [compressed raw run](measurements/undo-75dc1dad.json.gz).
The [current comparison chart](measurements/undo-performance.html) accompanies the initial table retained below.

The existing `undo_bench` fixture, driver, command work and five strict old/new alternations are unchanged. Build the instrumented example after sourcing FFmpeg:

```sh
source scripts/setup-ffmpeg.sh
flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo build --release -p kinewright-core --features history-metrics --example undo_bench
# Copy and hash the binary after the source commit; finish all owned checks first.
nice -n 19 python3 -I scripts/measure_undo.py <copied-executable> <output-directory> 4
```

All ten lanes exited zero and reported zero unexpected errors. All work/document assertions and warm-path counter assertions passed. The final merge `e49b0995` integrates `origin/te1` at `354917cfd12b7015891ccb16ef0e62e5dd06b640`; its Core tree is byte-identical to the measured source. The only conflict was the slow-test manifest, resolved by retaining both additions. No benchmark was run during an owned build or test.

The fixture still has 400 clips, 400 effects and 3,200 keyframes; its old structural estimator reports 3,301,441 live bytes and its JSON is 245,233 bytes. Copy transfers 16 effects / 128 keys; ripple removes one clip and moves 399. Neither is substituted with a trivial flag operation.

Host: Intel Core i5-13600K, 20 logical CPUs, Linux 7.2.15-2-omarchy, rustc 1.99.1. Measurement interval: 2026-11-14 03:58:23–04:00:13 UTC; lane wall times total 108.511 seconds. One-minute load ranged 3.44–4.77. Every lane had external Python contamination at 99–104% CPU; five lanes also sampled external rustc at 99–195%. Start/end snapshots record other compiler activity as well. The RTX 3090 stayed in P2 at 1800–1875 MHz graphics / 9501 MHz memory. No GPU is used by this benchmark. No external process was stopped; no quiet-machine measurement or sampled hot-stack attribution is claimed.

## Current results beside the initial qualification

Times are milliseconds: median of five lane summaries [minimum, maximum]. A walk's worst row summarizes the five per-lane maxima, rather than a pooled percentile. The initial column is the accepted `031cbd57` run; it is historical context, not a contemporaneous pair. Relative targets are evaluated against each current paired Core lane, not the ratio of column medians.

| Measurement (ms) | Initial history median | Paired raw Core now: median [range] | History now: median [range] | Target result |
| --- | ---: | ---: | ---: | --- |
| Hand edit at 1,000 steps | 2.104 | 0.384 [0.383, 0.395] | 0.230 [0.229, 0.234] | ≤1.5× paired Core: 5/5 |
| Hand edit at 10,000 steps | 2.397 | 0.392 [0.383, 0.404] | 0.247 [0.238, 0.250] | ≤1.5× paired Core: 5/5 |
| 120-batch gesture: apply | 253.382 | 29.114 [27.523, 36.140] | 26.837 [26.630, 27.102] | ≤1.5× paired Core: 5/5 |
| 120-batch gesture: Redo | 315.680 | 0.005 [0.003, 0.016] | 15.997 [15.976, 16.042] | <50 ms: 5/5 |
| Edit at folding bound | 6.640 | 0.396 [0.378, 0.409] (ordinary control) | 1.382 [1.354, 1.446] | <2 ms: all 160 samples |
| Open / validate at 2,000 | 6555.475 | 70.329 [69.126, 70.492] (refusal) | 282.018 [273.624, 317.811] | <1,000 ms: 5/5 |
| Cold Undo after open | 428.941 | N/A (open refused) | 252.951 [251.717, 255.290] | Below prior run: 5/5 |
| Walk 2,000: p95 | 8.691 | 0.003 [0.002, 0.010] | 5.692 [5.679, 5.751] | <100 ms: 5/5 |
| Walk 2,000: worst | 410.592 | 0.027 [0.025, 0.054] | 252.904 [249.313, 265.228] | <1,000 ms: 5/5 |
| Walk 2,000: median | 0.03005 | 0.001 [0.001, 0.002] | 0.040 [0.038, 0.045] | No additional target |
| Single Undo | 0.02809 | 0.001 [0.001, 0.002] | 0.024 [0.024, 0.038] | No additional target |
| 120-batch gesture: Undo | 0.04730 | 0.003 [0.001, 0.035] | 0.030 [0.029, 0.032] | No additional target |
| Copy attributes at depth | 2.131 | 0.313 [0.263, 0.541] | 1.205 [1.177, 1.245] | No additional target |
| Ripple delete at depth | 2.570 | 0.264 [0.257, 0.268] | 1.108 [1.083, 1.142] | No additional target |

Raw Core open is an expected capacity refusal, not a successful open comparison; new open validates both streams and installs successfully. Core's folding row is an ordinary-edit control. All 160 new folding observations are below 2 ms; global worst is 1.782656 ms. All five walk p95 values are below 100 ms, and the global walk maximum is 265.228426 ms. Cold Undo is 251.717–255.290 ms, below the prior median 428.941 ms (and its entire prior range).

### Depth flatness limitation

The stated within-lane sample-range comparison holds in all five lanes:

| Pair | 1,000-step sample range (ms) | 10,000-step sample range (ms) | Overlap |
| --- | ---: | ---: | --- |
| 1 | 0.201347–0.294515 | 0.213248–0.324729 | Yes |
| 2 | 0.199361–0.286586 | 0.210636–0.319122 | Yes |
| 3 | 0.200242–0.292744 | 0.214219–0.360724 | Yes |
| 4 | 0.198454–0.299922 | 0.208731–0.284575 | Yes |
| 5 | 0.198311–0.293023 | 0.231146–0.323589 | Yes |

That does not establish equal latency distributions. Five-run median bands are **0.228779–0.233651** versus **0.237982–0.249927 ms**; median-of-medians rises from 0.229770 to 0.246545 ms (**7.3008%**). Treat strict flatness as unproven, despite meeting the explicit within-lane range criterion and every paired 1.5× ceiling. History-side phase counts are identical at both depths. Counts alone do not attribute the remaining drift to the kernel or allocator. Incremental kernel validation is outside this turn and was not attempted.

The one-copy rule covers work added by history and owner accounting. Existing whole-document kernel validation, duration and identity passes remain unchanged and are counted separately; incremental kernel validation is a later job after the switch.

## Phase counters and the warm-path guard

Counts below are totals for each measured phase, identical in all five new lanes. Counter snapshots and correctness queries are outside the timed regions. `document_bytes_serialised` counts only history exact document measurements; total bytes also include operation charging, digests, records and shared payload serialization. A zero document-byte count does not imply zero payload encoding. Counters count calls, not CPU time, allocations or bytes read by kernel passes.

| New-history phase | Commands | Copies | Exact | Document / total serialized bytes | Reservations | Kernel validation / duration / identity |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| edit_1000 | 32 | 32 | 0 | 0 / 6016 | 32 | 64 / 32 / 64 |
| edit_10000 | 32 | 32 | 0 | 0 / 6016 | 32 | 64 / 32 / 64 |
| gesture_apply | 120 | 120 | 0 | 0 / 22576 | 120 | 240 / 120 / 240 |
| gesture_redo | 1 | 1 | 0 | 0 / 0 | 0 | 240 / 120 / 240 |
| fold | 32 | 64 | 32 | 7847744 / 7853760 | 32 | 128 / 64 / 128 |
| open | 1 | 1 | 4 | 736429 / 1019429 | 2000 | 4003 / 2000 / 4000 |
| cold_undo | 1 | 4 | 3 | 735717 / 735717 | 0 | 3998 / 1999 / 0 |
| walk | 2000 | 1997 | 1310 | 321266480 / 321266480 | 0 | 142928 / 71464 / 0 |
| copy | 1 | 1 | 1 | 253207 / 253569 | 1 | 2 / 1 / 2 |
| ripple | 1 | 1 | 1 | 237147 / 237304 | 1 | 2 / 1 / 2 |

Thus each warm edit performs **1 copy, 0 exact measurements, 0 document bytes and 188 total serialized bytes**, plus one reservation; existing kernel counts are **2 validation / 1 duration / 2 identity passes per command in both new history and old Core**. The 120-command gesture apply similarly has one copy per command and no exact measurements. One gesture Redo uses one candidate copy and zero reservations/measurements while replaying all 120 kernel operations. Fold's second copy belongs to the long-lived baseline; its exact measure tightens the new baseline. Open's four measurements are fixed boundary work, not 2,000 per-operation serializations. Cold/walk exact work measures materialized cache anchors, never each replayed operation.

Old history-side copy/exact counters are not instrumented; their emitted zeros are **N/A**, not evidence of no copies. Its feature-gated kernel counts are instrumented and valid. The feature is disabled in product builds; its global kernel counters are suitable for this single-worker benchmark, not per-thread attribution during concurrent tests. History counters are thread-local. The warm regression test uses named assertions `ONE_ATOMIC_DOCUMENT_COPY`, `NO_WARM_EXACT_MEASUREMENT` and `NO_WARM_DOCUMENT_SERIALIZATION` across 64 edits; these counts, rather than timing, guard the command path.

## Running bounds, capacity and trusted replay

`DocumentBound` travels with every current, baseline and cache document. Admission adds the existing reservation table's conservative growth, using the known document bound instead of scanning it. Bounds use saturating arithmetic. An encoded or live bound that crosses its cap triggers an exact measurement; acceptance then replaces both bounds with measured values, or refuses atomically. Construction, validated open, folded baselines and materialized anchors measure at their existing long-lived boundaries. Removal may leave loose bounds. Structural edits can conservatively cross the bound and measure once, as the copy/ripple counters show; no constant-latency claim is made for every operation kind.

Spare capacity is **charged explicitly**, with no document slack multiplier or normalization copy: typed recursive accounting counts Vec/String/PathBuf capacity, nested owners, inline sizes, a 16-byte per-allocation allowance and a conservative BTreeMap node allowance (eleven entries plus 128 bytes, at most one node per entry plus an empty root). Exhaustive field destructuring and inline `needs_drop` assertions expose new owned fields. The single atomic copy recursively preserves owned capacity. This matters for a coalesced removal/rebuild: ordinary `Clone` would compact the candidate while trusted in-place Redo retains old spare slots, invalidating its admitted peak. No copy is made solely to normalize capacity. Operation payload capacities are also retained and charged.

Trusted Redo, prefix replay and folding reserve the private step peak recorded during admission or validation, then call the existing operation kernel without repeating per-operation structural reservations. Raw records cannot obtain an installable `ValidatedSnapshot` without full validation. Validation clears all private runtime proofs, even for an in-memory captured record, before reconstructing them through semantic replay. `replay_step` asserts `VALIDATED_REDO_ONLY`. The kernel's validation, duration and identity passes remain unchanged; source changes there are feature-gated counter hooks only. Existing actor behavior and all shipping callers remain unchanged.

## Task role and native envelope

Profiles remain proposals for the later owner migration. Let D include explicitly charged capacity, K be retained cache slots, O decoded operations/step metadata/optional recorder, A separately reserved vacant stack storage and owner metadata, and W the serial replay/decode workspace. **R = (2 + K)D + O + A; peak = R + W.** There is no additional document slack factor because capacity is counted. These are conservative structural budgets, not RSS guarantees or implemented cross-owner ledgers.

| Role / policy | Encoded operations / steps | K | D / JSON cap | O / proposed A / W | R / peak |
| --- | --- | ---: | --- | --- | --- |
| Ordinary / Fold | 512 KiB / 16,384 | 4 | 3.5 MiB / 512 KiB | 6 / 5 / 32 MiB | 6×3.5+6+5 = 32 / 64 MiB |
| Task / NeverFold | **256 KiB / 4,096** | 4 | 4 MiB / 2 MiB | **8 / 4 / 64 MiB** | **6×4+8+4 = 36 / 100 MiB** |
| Candidate / NeverFold | 64 KiB / 512 | 0 | .5 MiB / 64 KiB | .5 / .5 / 64 MiB | 2×.5+.5+.5 = 2 / 66 MiB |

The task's 36 MiB retained envelope is 5 MiB below the existing 41 MiB native retained cap; 100 MiB includes its separate serial workspace, not a claim that the peak fits inside 41 MiB. Both 2,000 ordinary flag commands and **2,000 real successive SplitClip cuts** are accepted in tests. The latter checks 2,001 resulting clips, encoded charge ≤256 KiB, retained charge <41 MiB, Undo and Redo. Optional recording shares O and may consume admission space. A remains a proposal that the owner layer must validate against all ledgers and vacant capacities.

The old task persistence cap of 1,111,040 decoded bytes is **not** a constraint on this new record; H2b replaces it, per the design ruling. This supersedes the historical paragraph below. Candidate graph/resource overhead and cumulative owner admission still require H2b qualification; the heavy fixture still cannot fit Candidate's D cap.

## Current retained memory

| Measure (decimal bytes) | Paired raw Core now: median [range] | History now: median [range] |
| --- | ---: | ---: |
| Conservative retained charge | 7275586438 [7275586438, 7275586438] | 7580330 [7580330, 7580330] |
| Total process RSS | 1547231232 [1547169792, 1547292672] | 11014144 [10960896, 11079680] |
| RSS increase from startup | 1540702208 [1540636672, 1540702208] | 4493312 [4427776, 4493312] |
| Startup RSS | 6574080 [6467584, 6590464] | 6574080 [6467584, 6586368] |

Physical RSS remains approximately **1,547 MB versus 11 MB**, decimal. New retained charge uses capacity accounting rather than the initial estimator: its 7,580,330 bytes must not be compared with the initial 17,135,349 bytes as measured heap savings. RSS includes the entire fresh process and allocator retention, rather than history alone. Only the explicit retained-bound query walks the complete history; command transitions maintain incremental charge.

## Current verification and deliberate breaks

- `9ad274fa`: running bounds, capacity-preserving atomic copies, metrics, expanded task profile and guard tests. Core: 855 passed / 5 ignored / 33.89 test-seconds; undo including slow: 31 passed / 20.79 s; Core clippy, fmt and release benchmark build passed.
- `75dc1dad`: trusted folding and cache final-bound reuse. Core: 856 passed / 5 ignored / 33.88 test-seconds; undo including slow: 32 passed / 0 ignored / 20.90 s. Feature-enabled Core clippy, fmt and release benchmark build passed.
- `e49b0995`: requested merge of `origin/te1` (`354917cf`) with `[slow-tier]`. Post-merge Core: 856 passed / 5 ignored / 34.04 test-seconds; workspace clippy (all targets, warnings denied) 27.77 s; formatting passed; workspace build 34.59 s; slow-test lint passed (142 tests, 159 allowlist entries, 27 equipped-session skip sites). Shared-lock queue time is excluded. These gates also cover the unchanged Rust tree in the final report-only commit.

Every one of the 67 operation kinds has a limit/bound case at four marker-padding prefixes; all 53 growing kinds have a growing fixture. The other 14 still check bound invariants. Exact encoded/live cap refusal must leave the source state unchanged. These tests do not reuse the model's cost oracle.

Both required breaks were executed independently and reverted before the green source commits:

| Deliberate break | Actual named failure | Log |
| --- | --- | --- |
| Omit AddMarker growth from the running bound | `EVERY_GROWING_KIND_LIMIT AddMarker prefix=0` | `target/hx-perf/mutation-growth.log` |
| Route an untrusted record through trusted replay before validation | `VALIDATED_REDO_ONLY` | `target/hx-perf/mutation-untrusted-redo.log` |

An additional fail-before check with a compacting candidate clone failed `ATOMIC_COPY_PRESERVES_CAPACITY` (`capacity-before-2.log`); the capacity-preserving implementation passes. H1 fate/install/literal-byte tests and the prior six mutations are retained in the historical evidence below. Current production fates still come from effects; malformed records refuse; literal accounting boundaries remain tested.

Final check logs are `target/hx-perf/merged-{core,clippy,fmt,build,lint}.log`. No workspace test suite, Windows build, hosted CI, GUI or GPU qualification was run. No push or subagent was used. The remaining concern is the measured depth drift, not an inferred kernel bottleneck; H2b owner/persistence work remains outside this addition.

---

## Initial qualification (historical, before the performance revision)

The following report is preserved as the original measurement and implementation record. Its normalization strategy, task limits and proposed next performance work were superseded above; its tables remain the accepted earlier results.


The new history reduces the conservative retained charge by 99.764% on the heavy fixture and meets both Undo-walk latency targets in all five pairs. Edits, Redo and validation cost more than raw Core. The flat edit-latency goal remains unproven: median edit latency rises 13.9% between 1,000 and 10,000 steps (12.0–18.1% within individual pairs). The implementation has no edit-time history scan; the source alone does not explain away this measured drift. Before integration I would instrument admission and allocation phases and test whether compact-document allocation layout explains it. No targets or existing product limits were raised.

### Revision, reproduction and scope

Measured source: `031cbd578e855be3c1b82d34546a43422326d51c`.
Release executable SHA-256: `0b3c1eccb365b3ba7fedb9f830d3a0bf05df600e87d3169f70aa8bf28ef33b4a`.
Raw JSON SHA-256: `08bdd74beb9ae848c764a25a457e5528bbea64bcb17ae3021613b2a2a5c5144b`.
The complete samples, start/end machine state and contamination observations are in [the compressed raw run](measurements/undo-031cbd57.json.gz). The edit trend is in [the standalone chart](measurements/undo-edit-depth.html).

The one benchmark is `crates/kinewright-core/examples/undo_bench.rs`; the driver is `scripts/measure_undo.py`. It compares synchronous raw Core requests with direct operation-history execution on the identical checked command stream. The actor remains unchanged and in use everywhere. No GUI, writer, publication, project store, media path or end-to-end application speedup is measured. All ten lanes exited successfully; every lane reported zero unexpected errors. The expected old open refusal is counted separately.

Build after `source scripts/setup-ffmpeg.sh` with:

```sh
flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo build --release -p kinewright-core --example undo_bench
```

After all owned builds/tests finish, copy and hash that executable, then run:

```sh
nice -n 19 python3 -I scripts/measure_undo.py <copied-executable> <output-directory> 4
```

The driver runs five strict old/new pairs, with a 600-second timeout per lane and no wait for a quiet machine. The fixed executable was copied after the source commit; the preceding build used the identical source tree. The driver and benchmark contain assertions for work and resulting documents, rather than treating command completion alone as success.

### Fixture and host

The fixture has 400 clips, 400 effects and 3,200 keyframes. Clip 1 has 16 effects (the existing maximum), clips 2–385 have one effect, and clips 386–400 have none. Each effect has eight keyframes. Copy attributes transfers 16 effects / 128 keyframes to clip 400. Ripple delete removes clip 1 and moves the other 399 clips. Live document charge is 3,301,441 bytes; JSON is 245,233 bytes. At the accepted performance revision `75dc1dad`, the benchmark prints `Step`, `StepCommand` and `Operation` sizes of 120, 72 and 160 bytes; 80 bytes is `StepCharge`.

Host: Intel Core i5-13600K, 20 logical CPUs; Linux 7.2.15-2-omarchy; rustc 1.99.1. Measurement interval: 2026-11-14 01:27:15–01:32:20 UTC. The sum of lane wall times is 302.623 seconds. One-minute load ranged 7.35–7.75. Every lane had external CPU contamination: Whisper used 348–395% CPU, Python 99–104%, and another worktree's Agent test 81–97%; PipeWire, T3 and Brave also crossed the driver's 20% flag occasionally. Start/end snapshots include both cumulative `ps` CPU and sampled instantaneous top processes. The RTX 3090 stayed in P2, graphics clocks 1815–1875 MHz, memory 9501 MHz. This benchmark uses no GPU. No other process was stopped, and no quiet-machine run is claimed.

### Results

Times are milliseconds, median of the five lane summaries, followed by their minimum and maximum. Hand edits and single Undo use 32 observations per lane; each walk has 2,000 observations. The walk's worst row is the median of the five maxima with the full range, not a pooled percentile.

| Measurement (ms) | Raw Core: median [range] | Operation history: median [range] |
| --- | ---: | ---: |
| Hand edit at 1,000 steps | 0.404 [0.377, 0.698] | 2.104 [2.084, 2.124] |
| Hand edit at 10,000 steps | 0.388 [0.382, 0.701] | 2.397 [2.380, 2.507] |
| Single Undo | 0.00142 [0.00139, 0.00146] | 0.02809 [0.02716, 0.05003] |
| Walk 2,000: median | 0.00142 [0.00042, 0.00148] | 0.03005 [0.02974, 0.03060] |
| Walk 2,000: p95 | 0.00252 [0.00128, 0.00275] | 8.691 [8.609, 8.857] |
| Walk 2,000: worst | 0.03073 [0.01300, 0.160] | 410.592 [388.821, 644.782] |
| Cold Undo after open | N/A (open refused) | 428.941 [421.197, 437.602] |
| 120-batch gesture: apply | 30.910 [27.747, 44.526] | 253.382 [250.552, 268.533] |
| 120-batch gesture: Undo | 0.00261 [0.00145, 0.00281] | 0.04730 [0.04671, 0.05242] |
| 120-batch gesture: Redo | 0.00468 [0.00310, 0.00495] | 315.680 [315.482, 324.274] |
| Copy attributes at depth | 0.272 [0.267, 0.490] | 2.131 [2.122, 2.169] |
| Ripple delete at depth | 0.270 [0.256, 0.404] | 2.570 [2.537, 2.614] |
| Edit at folding bound | 0.682 [0.385, 0.702] | 6.640 [6.621, 6.738] |
| Open / validate at 2,000 steps | 69.303 [68.713, 70.109] | 6555.475 [6475.475, 6711.261] |

Old open is an expected `HistoryError::Capacity` refusal in the existing bounded transport resolver. Its 69.303 ms is refusal time; it is not a successful open comparison. Consequently old cold Undo is unavailable. New open includes decode, structural preflight, both replay streams and installation. All five new opens succeeded.

The old folding row is an ordinary-edit control: raw Core has no ordinary auto-fold admission. The new row uses a literal 500-byte operation budget and checks that folding has started, depth stays 10, the first retained position advances from 22 to 54, encoded charge remains 465 bytes, and the baseline changes on every edit. It is not a matched old folding implementation.

The complete walk checks every boundary document and reaches depth zero. The gesture checks all 120 operations occupy one step and verifies documents after Undo and Redo. Copy/ripple check their actual expanded results. Cold Undo checks depth 1,999 and its expected document. Assertions and document queries are outside timed regions; every timed command executes synchronously.

All five new walk p95 values are below 100 ms: 8.609, 8.691, 8.727, 8.625, 8.857. Their maxima are below 1,000 ms: 410.592, 437.189, 644.782, 388.821, 392.566. Global worst is 644.782 ms. These qualify this fixture and host, not every valid document or future product path.

The edit increase from 2.104 to 2.397 ms misses a strict flat-latency interpretation. A tenfold depth increase did not produce tenfold work, but this run does not establish depth-independent measured latency. Admission's document validation/encoding and allocation are source-supported suspects, not a measured attribution. Instrumenting those phases and testing a steadier allocation layout is the next performance task. Gesture Redo and record open retain full per-operation workspace accounting; their observed cost must also be considered before product integration.

### Retained memory at 2,000 steps

All byte figures below are decimal bytes. The structural charges are conservative estimates from the existing estimators, not sampled heap allocations. RSS includes the entire fresh benchmark process, allocator retention, the original expected fixture and executor. It is not interchangeable with a history charge.

| Measure | Raw Core median [range] | Operation history median [range] |
| --- | ---: | ---: |
| Conservative retained charge | 7,275,586,438 [same] | 17,135,349 [same] |
| Total process RSS | 1,547,108,352 [1,547,087,872, 1,547,198,464] | 10,973,184 [10,887,168, 10,989,568] |
| RSS increase from startup | 1,540,571,136 [1,540,505,600, 1,540,636,672] | 4,362,240 [4,296,704, 4,423,680] |
| Startup RSS | 6,561,792 [6,451,200, 6,627,328] | 6,606,848 [6,524,928, 6,615,040] |

The same estimator conservatively charges the old captured boundary documents many times. The new retained-bound query counts actual distinct document pointers, payload storage, current stack capacity and recorder storage. Only this explicit query walks history to estimate cost; transitions keep charges incrementally.

### Cache and replay

Four document slots hold one pre-top pointer, one 32-step anchor and two nearby replay boundaries. Pre-top makes immediate Undo after an edit a pointer swap. During a longer walk, a replay warms only the final surviving boundary for each nearby slot, selected using depth trailing zeros, and reuses the periodic anchor. Cache documents are absent from records. Warming is staged until success and retries without warming if workspace is tight. Dropping the cache changes no semantic state.

Two exploratory new lanes at `765842f3` missed the targets: p95 1,046–1,122 ms, worst 6,092–7,739 ms. Those partial pilot lanes are excluded from the five-pair results. Source inspection identified repeated structural workspace scans during immutable prefix replay. The final implementation reserves each step's private validated peak, then runs the existing operation kernel and document validation. The peak is computed at admission or open, skipped on the wire, and inaccessible to live callers. Do, Redo, folding and untrusted record validation still use the existing per-operation reservation table. This source-supported explanation motivated the change; no tracer or sampled hot-stack attribution was used.

A fixed small cache cannot promise constant-time arbitrary boundary queries or a universal worst-case Undo time. Cold Undo still replays a prefix. Larger fixtures/depths may require additional budgeted checkpoints or a different replay representation; no such guarantee follows from this fixture.

### Proposed role profiles and arithmetic

These profiles are additions only; no existing role or limit is changed. Let D be maximum live document charge, K cached documents, O the decoded operation/step/recorder allowance, A reserved stack headroom and owner metadata, W the serial replay/decode window. Retained envelope R = (2 + K)D + O + A; peak = R + W. Shared pointers can reduce actual retention. Independent captures/endpoints require their own charge. Raw/decoded input and temporary normalization overlap are charged inside W on raw decode. These are conservative structural estimates, not an OS RSS guarantee.

| Role / policy | Operation bytes / steps | K | D / JSON document cap | O / proposed A / W | R / peak |
| --- | --- | ---: | --- | --- | --- |
| Ordinary / Fold | 512 KiB / 16,384 | 4 | 3.5 MiB / 512 KiB | 5 / 6 / 32 MiB | 6×3.5+5+6 = 32 / 64 MiB |
| Task / NeverFold | 24 KiB / 1,024 | 4 | 4 MiB / 256 KiB | .5 / 1 / 64 MiB | 6×4+.5+1 = 25.5 / 89.5 MiB |
| Candidate / NeverFold | 64 KiB / 512 | 0 | .5 MiB / 64 KiB | .5 / .5 / 64 MiB | 2×.5+.5+.5 = 2 / 66 MiB |

The task retained proposal is below today's 41 MiB native cap. Candidate retains today's 2 MiB cap, so the heavy fixture cannot fit it: no candidate-heavy success is claimed. Current persisted task/candidate caps are 1,111,040 / 8,888,320 bytes. Part 2 must qualify complete graph references, audit/lineage/resource records and owner reservations against those existing caps. A is an explicit proposal for that owner budget; this module does not implement the owner ledger or cumulative cross-history admission. Optional recorder bytes share O and can exhaust it; recording is off by default and never silently truncates.

### Verification and commits

- `7723a5a6`: separate test-only H1 oracle repairs from both reviews. H1 fast: 14 passed, 1 ignored, 7.80 s; H1 long: 1 passed, 115.22 s. Full Core: 826 passed, 3 ignored, 30.78 test-seconds. Clippy and formatting passed.
- `84a39870`: production state/executor, record/open and differential tests. Core: 842 passed, 4 ignored. New fast: 16 passed, 1 ignored, 3.06 s; new long 17.31 s. Clippy and formatting passed.
- `765842f3`: bounded cache, role proposals, allocation normalization and measurement harness. Core: 849 passed, 4 ignored, 35.35 test-seconds. New fast 23 passed / 1 ignored / 5.38 s; long 29.94 s. Clippy, formatting, workspace build and release example passed.
- `031cbd57`: trusted replay reservation and typed payload normalization. Final Core: 850 passed, 4 ignored, 39.54 test-seconds; compilation 22.05 s. H1 fast 14 passed / 1 ignored / 9.10 s; new fast 24 passed / 1 ignored / 5.28 s; combined fast binaries 14.38 s. New long 1 passed / 29.31 s (8.80 s compilation). Clippy 11.40 s, workspace build 40.32 s, release example 17.82 s; formatting passed. Shared-lock queue time is excluded. These gates cover the unchanged Rust source in the report-only commit.

The slow-test manifest lint passes: 140 tests, 159 allowlist entries, 27 equipped-session skip sites. The long differential spans every one of 67 operation kinds, both policies, step caps 1/3 and cache sizes 0/1/4, random cache drops and round-trip installs. Generated command fates come from production effects and are compared for every command, including coalesced members. Separate cases cover typed malformed installs, literal 46/47 and 92/93-byte admission, empty-batch precedence, document limits, no-op closes, original epochs, bounded recording and near-workspace replay.

Each required mutation was executed separately, failed and was reverted. These are actual failures, not read-only predictions:

| Mutation | Named failing assertion |
| --- | --- |
| H1 Redo marks the wrong fate | `EVERY_COMMAND_FATE` |
| H1 install omits Redo replay | `INSTALL_REFUSES_INVALID_REDO` |
| H1 doubles its byte cost | `LITERAL_PLAIN_EXACT_FIT` |
| Production fold fails to release charge | `FOLD_DROPS_CHARGE` |
| Production Undo selects a stale depth | `CHECKPOINT_DEPTH_DOCUMENT` |
| Production refusal clears Redo | `PRODUCTION_REDO_REFUSAL_ATOMIC` |

Logs are retained under this worktree's `target/hx-h2a/` as `h1-mutation-*.log`, `production-mutation-*.log`, `final-{core,long,clippy,fmt,workspace,release}.log`, and `paired/`. The three H1 mutations were run for the oracle-repair commit; the production three were re-run after the cache addition. The final unmutated tree passed all gates above. A canceled queued allocation probe never started and is not claimed as fail-before evidence.

No full workspace test suite, Windows build, hosted CI, GUI or GPU qualification was run. Core tests and a Linux workspace build are the executed scope. Nothing was pushed; no subagent was used.

### Design assessment and part 2

The model, event effects, immutable original command IDs, atomic folds, typed snapshot token and canonical record identity are implemented. The fixed-cache design trades bounded retention for occasional prefix replay; the measured costs and edit-depth drift make an unconditional constant-latency claim too strong. Role envelopes remain proposals until the owner layer accounts for all endpoints, captures and ledgers.

Part 2 needs to implement the audit ledger, lineage seeds and resource ownership from events; qualify persisted graph/reference sizes and cumulative role reservations; migrate GUI-to-writer snapshot transport/install fencing; define recorder ownership; and move existing readers before deleting old history. The module supplies accepted/moved/discarded/folded command keys, identity effects, boundary queries, a validated record install, canonical digest and optional recorder. It deliberately does not yet supply those owner-layer systems or durable receipt/publication fencing.

## Part 1 review follow-ups during H2a part 2

Guard commit `2a5c59a7` adds encoded and actual-capacity assertions after every differential transition and boundary query, plus populated installation, reserved-path, typed map-payload, digest, duplicate-key and backwards-epoch cases. Nine reverted wrong changes each failed a named assertion; their names and logs are recorded in the part 2 report. Command keys now increase strictly while walking Undo then reversed Redo.

Profile commit `a3f36da9` removes the unused `StepCharge.before`. The fixed probe prints `Step = 104`, `StepCommand = 72`, `Operation = 160`, `Clip = 240`, identity effect = 312 and command effects = 40 bytes on this build. The historical 120-byte Step quoted above belongs to the accepted performance build before that deletion.

The candidate workspace is now 16 MiB. At O = 512 KiB and 160 bytes per inline operation, at most 3,276 operations fit before other charges. Four identity-observation slots per operation, each including 312 + 40 + 4 × 16 bytes, reserve 5,451,264 bytes. Add kernel workspace 8D = 4 MiB, stored-shell input O+2A+8 KiB = 1.5 MiB + 8 KiB, owner scratch 2A = 1 MiB and 64 KiB headers: 12,340,736 bytes (11.769 MiB). Ten percent slack gives 12.946 MiB; round up to 16 MiB. Payload-heavy/coalesced cases must also pass their actual recorded-peak admission. R = 2D + O + A = 2 MiB; peak = 18 MiB. The shared pool sums retained caps 128 + 328 + 16 = 472 MiB and allows at most 64 MiB of concurrent workspace across roles, for 536 MiB total structural envelope.

Clip-growth tightening is deferred under the review's explicit fallback. A touched clip's payload does not include a full enclosing clip array doubling, or implicit clip/automation fan-out. The new witness `touched_clip_payload_misses_full_track_array_growth` exercises a four-slot full track which splits to five clips. A sound smaller scalar bound needs maintained capacity maxima or kernel observations; another history-side clip search would add a whole-document traversal. The conservative formulas and their exact-measurement cost remain. The 67-kind bound matrix passes after this change.

Five strict alternating before/after release pairs use the same 400-clip fixture, depth 2,000 and identical copy, ripple and split work; no owned build/test overlapped the run. The before binary is production tree `2a5c59a7` plus the benchmark-only probe, SHA-256 `f3041a0e7cfadc7e144576d9958b52c0d9692f4731249f2c23d0d9d3eec02776`; after is production tree `a3f36da9`, SHA-256 `4bc42ab6d6e67bd08fb78379386d08c383b015dd72975db50ac25e151341bfff`. The latter build differs from the committed probe only by a lint annotation. All ten lanes exited zero. [Raw clip pairs](measurements/undo-review-clip-a3f36da9.json.gz) contain start/end machine observations, counters and document assertions.

| Same-fixture lane (ms) | Before median [range] | After median [range] | History phases after |
| --- | ---: | ---: | --- |
| Copy attributes | 1.204 [1.194, 1.212] | 1.212 [1.209, 1.289] | 1 copy, 1 exact, 253,207 document bytes |
| Ripple delete | 1.095 [1.088, 1.099] | 1.098 [1.089, 1.114] | 1 copy, 1 exact, 237,147 document bytes |
| Split | 1.256 [1.245, 1.288] | 1.255 [1.252, 1.277] | 1 copy, 1 exact, 253,367 document bytes |

There is no measured speedup. Each command retains two kernel validation passes, one duration pass and two identity passes, counted separately from history work. Kernel passes remain unchanged; incremental kernel validation is a later job after the switch.

<!-- hx-owner-qualification:start -->
## Owner qualification (H2a part 2)

Measured production source `41171a3ba37fca24a8e52d0f07cc7ab737140b9a`. Binary SHA-256 `9b75633724c4325dbe5b210588ed27ccd2fd12409e6353b4e87199226d10d03f`. [Complete raw pairs](measurements/undo-owner-41171a3b.json.gz) (gzip SHA-256 `01f19d0778f21968ddd458662cfafca882e912788a22041c47b07ab7cc21bc91`); [role measurements](measurements/undo-owner-roles-41171a3b.json); [interactive comparison](measurements/undo-owner-performance.html).

All ten lanes exited zero with no unexpected errors. Both sides use the unchanged heavy fixture (400 clips, 400 effects, 3,200 keyframes; JSON 245,233 bytes), identical commands and the same five alternating release-pair method. An identical DeleteClip400/Undo prelude leaves a nonempty approval ledger on the new side; seeds and a media reference are present. The fold lane uses that same fixture and prelude, with the original 500-byte operation budget. Old history remains in every shipping caller. This measures standalone execution and store open, not writer/GUI latency.

UTC 2026-11-14T10:25:40Z–2026-11-14T10:27:51Z; total lane wall time 129.430 s. One-minute load 7.29–9.06; 10/10 lanes sampled external work above 20% CPU. Peaks by process name: brave 23.6%, cc1plus 99.4%, pipewire 49.9%, python 104.0%, rustc 215.0%, t3code 54.0%. Raw metadata includes top-five instantaneous CPU samples and RTX 3090 clocks/states. No owned build or test overlapped timing; no process was stopped or quiet lane selected. Kernel validation, duration and identity passes remain unchanged and are counted separately from history/owner work.

Milliseconds: median of five lane summaries [minimum, maximum]. The bare-history column is accepted part 1, historical and unpaired. Old open is an expected transport capacity refusal, not a successful open comparison. New open includes reading the stored shell and both document objects, ownership checks, identity verification and full semantic validation. Encoding/staging is outside the open clock, as encoding was outside part 1's open clock.

| Measurement (ms) | Accepted bare history | Paired Core | Owner attached | Target |
| --- | ---: | ---: | ---: | --- |
| Hand edit, 1,000 | 0.230 | 0.394 [0.387, 0.404] | 0.245 [0.240, 0.271] | ≤1.5× paired Core: 5/5 |
| Hand edit, 10,000 | 0.247 | 0.401 [0.395, 0.746] | 0.273 [0.257, 0.464] | ≤1.5× paired Core: 5/5 |
| 120-batch gesture apply | 26.837 | 28.530 [28.143, 29.327] | 31.968 [28.228, 45.882] | ≤1.5× paired Core: 4/5 |
| Gesture Redo | 15.997 | 0.002 [0.001, 0.002] | 17.685 [16.871, 21.443] | <50: 5/5 |
| Edit at fold bound | 1.382 | 0.418 [0.405, 0.738] | 1.733 [1.630, 2.549] | <2: 123/160 samples; worst 2.847 |
| Validated open, 2,000 | 282.018 | 72.304 [71.495, 72.676] | 307.028 [289.176, 570.982] | <1,000: 5/5 |
| Cold Undo after open | 252.951 | N/A | 269.587 [259.096, 482.095] | Historical comparison, not paired |
| Walk p95 | 5.692 | 0.003 [0.002, 0.005] | 6.902 [5.946, 9.146] | <100: 5/5 |
| Walk worst | 252.904 | 0.069 [0.016, 0.165] | 329.129 [266.612, 472.708] | <1,000: 5/5 |

The folding ceiling is missed: 123/160 observations are below 2 ms, global worst 2.847044 ms. Cold Undo's median is 1.066× the historical accepted 252.951 ms; this is not a contemporaneous causal comparison. Edit-depth sample ranges overlap in 4/5 lanes; strict depth flatness is not claimed. No kernel validation work was replaced. Timing does not establish which kernel pass dominates.

The first owner qualification at `9405a998` is also retained: [raw run](measurements/undo-owner-9405a998.json.gz), timed binary `a0b9ac80548cc4f4dd9a87e48aa8d8955e6b06a85afc25e85ffe9b4b09e5b8ad`, 09:44:19–09:46:33 UTC. It had 125/160 folds below 2 ms (worst 2.755154), fold median 1.579 [1.569, 2.249], open 290.421 [284.741, 777.980], and cold Undo 260.936 [256.292, 485.880]. All ten lanes sampled external activity; load 7.31–16.81 and ffmpeg peaked at 1,254.45% CPU. The current requalification follows an expected-role ingress cap/counter-evidence guard and a benchmark-only candidate error-variant correction. The fixture, timing targets and paired method were not tuned; the earlier misses are not discarded.

Warm edits at both depths perform one candidate document copy, zero exact document measurements, zero document bytes serialized by the document-size meter, zero owner document scans and zero owner aggregate measurements. In the measured lane each edit serializes 188 Core payload bytes plus 529 owner payload bytes; these encode only the command's own operations/origin and reserve its future decode. Existing kernel passes are 2 validation, 1 duration and 2 identity per command. Tests assert the one-copy/zero-measurement property with a nonempty ledger, lineage and resources. Fold deliberately walks the new baseline; its per-command counters are 2 document copies / 1 Core exact measurement / 2 owner document scans / 2 owner aggregate measurements. These are history/owner counters, not a claim that the kernel avoids whole-document work.

The 2,000-command shell is 510,893 bytes (255.4465/command); the store charges 515,501 (257.7505/command), 0.902% above encoding. Baseline/current document objects are separate graph edges. The old approximate 2,400 decoded bytes for a 154-byte command is the brief's reference, not a contemporaneous identical-record measurement.

### Role and persisted bounds

Let D be capacity-counted live document bytes, K cached documents, O decoded Core operations/step storage and A owner storage. R=(2+K)D+O+A; peak=R+W. Shared-store caches, input-manifest decode and media I/O keep their existing store budgets; this history pool is not an overall application RSS cap. Resource trees have separate store content budgets. The history-JSON graph caps below mean the shell plus its two document objects, excluding external media/LUT/font trees.

| Role / policy | D; K; O; A; W (MiB) | Retained / peak | Encoded body / shell / history JSON graph cap |
| --- | --- | --- | --- |
| Ordinary / Fold | 3.5; 4; 6; 5; 32 | 6×3.5+6+5=32 / 64 MiB | 11 MiB / 16 MiB+8 KiB / 17 MiB+8 KiB |
| Task / NeverFold | 4; 4; 8; 4; 64 | 6×4+8+4=36 / 100 MiB | 12 MiB / 16 MiB+8 KiB / 20 MiB+8 KiB |
| Candidate / NeverFold | .5; 0; .5; .5; 16 | 2×.5+.5+.5=2 / 18 MiB | 1 MiB / 1.5 MiB+8 KiB / 1.625 MiB+8 KiB |

Body cap=O+A; shell cap=O+2A+8 KiB; history-JSON graph cap=shell+2J, where document JSON cap J is .5 / 2 / .0625 MiB. The requested role's shell cap is checked before store ingress allocation, even when a record claims a different role. Actual replay/decode admission still applies. These are new-kind caps; old persisted limits and callers are untouched.

Task retains 256 distinct approve/delete/Undo consents in one test: owner charge 226,576 bytes, total history charge 229,000, shell 135,184 and future decode reserve 4,467,720, all inside A=4 MiB, R=36 MiB (<41 MiB native retained cap), shell 16 MiB+8 KiB and W=64 MiB. Each Do precharges 768 bytes plus target/id capacity, including its eventual fixed folded-prefix fact. Each transition precharges 1,024 plus id capacity (normally 1,088): 256×1,088=278,528 bytes; decode 256×16,384=4 MiB. Do and transition entries share the same 256 cap. The full maximal-fixture run remains an H2b exit test.

Candidate W arithmetic is in the review-follow-up section above: 12,340,736 bytes subtotal, 12.946 MiB with ten percent slack, rounded to 16 MiB. Its retained envelope remains 2 MiB. The shared pool admits retained totals 128+328+16=472 MiB and one 64 MiB aggregate workspace window, 536 MiB for history holders/scratch. Captures and boundary results retain independent leases. Admission, one byte over, release on drop, shared-window contention and pre-allocation refusal are tested.

| Role | Measured commands / fixture | Retained charge (bytes) | R / peak |
| --- | --- | ---: | --- |
| Ordinary | 2000; heavy | 8,856,298 | 32 / 64 MiB |
| Task | 2000; heavy | 8,856,298 | 36 / 100 MiB |
| Candidate | 400; small (heavy refused) | 341,824 | 2 / 18 MiB |

Candidate's heavy fixture fails with typed `DocumentCapacity`; no candidate-heavy success is claimed. The ordinary timing lane including its nonempty ledger charges 8,857,562 bytes. Process RSS medians are 1,550,184,448 old versus 15,577,088 owner; startup-relative increases 1,540,767,744 versus 6,144,000. Structural charges and RSS are distinct measurements.
<!-- hx-owner-qualification:end -->
