# Operation-history qualification (H2a)

The new history reduces the conservative retained charge by 99.764% on the heavy fixture and meets both Undo-walk latency targets in all five pairs. Edits, Redo and validation cost more than raw Core. The flat edit-latency goal remains unproven: median edit latency rises 13.9% between 1,000 and 10,000 steps (12.0–18.1% within individual pairs). The implementation has no edit-time history scan; the source alone does not explain away this measured drift. Before integration I would instrument admission and allocation phases and test whether compact-document allocation layout explains it. No targets or existing product limits were raised.

## Revision, reproduction and scope

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

## Fixture and host

The fixture has 400 clips, 400 effects and 3,200 keyframes. Clip 1 has 16 effects (the existing maximum), clips 2–385 have one effect, and clips 386–400 have none. Each effect has eight keyframes. Copy attributes transfers 16 effects / 128 keyframes to clip 400. Ripple delete removes clip 1 and moves the other 399 clips. Live document charge is 3,301,441 bytes; JSON is 245,233 bytes. `Step`, `StepCommand` and `Operation` are respectively 80, 72 and 160 bytes on this build.

Host: Intel Core i5-13600K, 20 logical CPUs; Linux 7.2.15-2-omarchy; rustc 1.99.1. Measurement interval: 2026-11-14 01:27:15–01:32:20 UTC. The sum of lane wall times is 302.623 seconds. One-minute load ranged 7.35–7.75. Every lane had external CPU contamination: Whisper used 348–395% CPU, Python 99–104%, and another worktree's Agent test 81–97%; PipeWire, T3 and Brave also crossed the driver's 20% flag occasionally. Start/end snapshots include both cumulative `ps` CPU and sampled instantaneous top processes. The RTX 3090 stayed in P2, graphics clocks 1815–1875 MHz, memory 9501 MHz. This benchmark uses no GPU. No other process was stopped, and no quiet-machine run is claimed.

## Results

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

## Retained memory at 2,000 steps

All byte figures below are decimal bytes. The structural charges are conservative estimates from the existing estimators, not sampled heap allocations. RSS includes the entire fresh benchmark process, allocator retention, the original expected fixture and executor. It is not interchangeable with a history charge.

| Measure | Raw Core median [range] | Operation history median [range] |
| --- | ---: | ---: |
| Conservative retained charge | 7,275,586,438 [same] | 17,135,349 [same] |
| Total process RSS | 1,547,108,352 [1,547,087,872, 1,547,198,464] | 10,973,184 [10,887,168, 10,989,568] |
| RSS increase from startup | 1,540,571,136 [1,540,505,600, 1,540,636,672] | 4,362,240 [4,296,704, 4,423,680] |
| Startup RSS | 6,561,792 [6,451,200, 6,627,328] | 6,606,848 [6,524,928, 6,615,040] |

The same estimator conservatively charges the old captured boundary documents many times. The new retained-bound query counts actual distinct document pointers, payload storage, current stack capacity and recorder storage. Only this explicit query walks history to estimate cost; transitions keep charges incrementally.

## Cache and replay

Four document slots hold one pre-top pointer, one 32-step anchor and two nearby replay boundaries. Pre-top makes immediate Undo after an edit a pointer swap. During a longer walk, a replay warms only the final surviving boundary for each nearby slot, selected using depth trailing zeros, and reuses the periodic anchor. Cache documents are absent from records. Warming is staged until success and retries without warming if workspace is tight. Dropping the cache changes no semantic state.

Two exploratory new lanes at `765842f3` missed the targets: p95 1,046–1,122 ms, worst 6,092–7,739 ms. Those partial pilot lanes are excluded from the five-pair results. Source inspection identified repeated structural workspace scans during immutable prefix replay. The final implementation reserves each step's private validated peak, then runs the existing operation kernel and document validation. The peak is computed at admission or open, skipped on the wire, and inaccessible to live callers. Do, Redo, folding and untrusted record validation still use the existing per-operation reservation table. This source-supported explanation motivated the change; no tracer or sampled hot-stack attribution was used.

A fixed small cache cannot promise constant-time arbitrary boundary queries or a universal worst-case Undo time. Cold Undo still replays a prefix. Larger fixtures/depths may require additional budgeted checkpoints or a different replay representation; no such guarantee follows from this fixture.

## Proposed role profiles and arithmetic

These profiles are additions only; no existing role or limit is changed. Let D be maximum live document charge, K cached documents, O the decoded operation/step/recorder allowance, A reserved stack headroom and owner metadata, W the serial replay/decode window. Retained envelope R = (2 + K)D + O + A; peak = R + W. Shared pointers can reduce actual retention. Independent captures/endpoints require their own charge. Raw/decoded input and temporary normalization overlap are charged inside W on raw decode. These are conservative structural estimates, not an OS RSS guarantee.

| Role / policy | Operation bytes / steps | K | D / JSON document cap | O / proposed A / W | R / peak |
| --- | --- | ---: | --- | --- | --- |
| Ordinary / Fold | 512 KiB / 16,384 | 4 | 3.5 MiB / 512 KiB | 5 / 6 / 32 MiB | 6×3.5+5+6 = 32 / 64 MiB |
| Task / NeverFold | 24 KiB / 1,024 | 4 | 4 MiB / 256 KiB | .5 / 1 / 64 MiB | 6×4+.5+1 = 25.5 / 89.5 MiB |
| Candidate / NeverFold | 64 KiB / 512 | 0 | .5 MiB / 64 KiB | .5 / .5 / 64 MiB | 2×.5+.5+.5 = 2 / 66 MiB |

The task retained proposal is below today's 41 MiB native cap. Candidate retains today's 2 MiB cap, so the heavy fixture cannot fit it: no candidate-heavy success is claimed. Current persisted task/candidate caps are 1,111,040 / 8,888,320 bytes. Part 2 must qualify complete graph references, audit/lineage/resource records and owner reservations against those existing caps. A is an explicit proposal for that owner budget; this module does not implement the owner ledger or cumulative cross-history admission. Optional recorder bytes share O and can exhaust it; recording is off by default and never silently truncates.

## Verification and commits

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

## Design assessment and part 2

The model, event effects, immutable original command IDs, atomic folds, typed snapshot token and canonical record identity are implemented. The fixed-cache design trades bounded retention for occasional prefix replay; the measured costs and edit-depth drift make an unconditional constant-latency claim too strong. Role envelopes remain proposals until the owner layer accounts for all endpoints, captures and ledgers.

Part 2 needs to implement the audit ledger, lineage seeds and resource ownership from events; qualify persisted graph/reference sizes and cumulative role reservations; migrate GUI-to-writer snapshot transport/install fencing; define recorder ownership; and move existing readers before deleting old history. The module supplies accepted/moved/discarded/folded command keys, identity effects, boundary queries, a validated record install, canonical digest and optional recorder. It deliberately does not yet supply those owner-layer systems or durable receipt/publication fencing.
