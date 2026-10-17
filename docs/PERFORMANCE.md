# Performance work

Kinewright performance is measured in three lanes, with shared desktop startup
and idle measurements. Use optimized release binaries, fixed workloads, and the
same machine for comparisons. Correct output, atomic edits, and undo/redo remain
acceptance requirements.

| Lane | Initial repeatable workload | Measures |
| --- | --- | --- |
| Typical editing | Two generated 1080p30 H.264/AAC sources, 3 tracks, 24 clips | Exact preview seek request to frame receipt; resident memory; preview cache residency |
| Heavy editing | Two generated 4K30 H.264/AAC sources, 4 tracks, 200 clips | The same seek and memory measurements at higher source resolution |
| Agent editing | 1,200 clips across 4 tracks, repeated 20-operation plans, 4,320 mapped transcript words | Core batch/undo/redo round trips, cached transcript mapping, resident memory |
| Shared desktop | Fresh empty project with installed harness discovery enabled | First UI callback, first preview upload, UI callback durations, RSS, CPU and OS threads |

The first pass and its evidence are in
[the September 16 report](reports/performance-2026-09-16/README.md).

## Reproduce

From the repository root on Linux with the native build dependencies installed:

```bash
source scripts/setup-ffmpeg.sh
cargo build --release -p kinewright-app
python3 scripts/measure-desktop.py --runs 5 --seconds 8 --output /tmp/desktop.json
cargo build --release -p kinewright-project --example performance_workloads
# Use separate processes so one lane does not inherit another lane's memory.
target/release/examples/performance_workloads --lane typical --seeks 30 --plans 100 --out /tmp/typical.json
target/release/examples/performance_workloads --lane heavy --seeks 30 --plans 100 --out /tmp/heavy.json
target/release/examples/performance_workloads --lane agent --seeks 30 --plans 100 --out /tmp/agent.json
```

Repeat each lane three or more times. Compare medians of run medians and report
p95 alongside the median. Keep the baseline binary before rebuilding; alternate
baseline and candidate runs when a difference is close to ordinary run variance.
Do not run builds or tests concurrently with timed workloads.

The workload example generates synthetic media with pinned FFmpeg arguments; it
requires no footage download, model, provider session or API key. Setup/generation
and probing are excluded from seek/edit timings. Generated `.kinewright` projects
remain alongside the media under `--workdir` (default `/tmp/kinewright-perf/workloads`).
The optional `--lane all` uses a shared engine: its memory figures are cumulative;
use separate processes for independent memory comparisons.

The desktop sampler opens real windows and closes them after the requested
interval. Do not edit these measurement instances. It isolates XDG data and cache
paths, leaves HOME and installed harness discovery intact, and cleans up its own
process group. It does not modify existing projects or app data. On other
platforms, `KINEWRIGHT_PERF_REPORT=/path/report.json` and
`KINEWRIGHT_PERF_SECONDS=8` enable the in-app timings; the Python process sampler
currently requires Linux `/proc`.

Run focused release microbenchmarks to isolate a hot path:

```bash
cargo test --release -p kinewright-core performance_batch_workloads -- --ignored --nocapture
cargo test --release -p kinewright-app is_dirty_memoized_poll_release_perf -- --ignored --nocapture
cargo test --release -p kinewright-agent served_tools_direct_vs_legacy_filtered_microbench -- --ignored --nocapture
```

These ignored tests report timings and compare behavior against the prior
algorithm. Correctness tests run normally. Wall-clock performance thresholds are
not part of ordinary CI: noisy shared runners would make them unreliable.

## Measurement boundaries

- First UI is elapsed time from `app::run` to completion of the first egui UI
  callback. It excludes dynamic-loader time and does not measure compositor
  presentation. First preview means the first texture has reached the UI.
- UI durations measure callback wall time, excluding GPU presentation, after a
  two-second warmup. They are not input-to-photon latency. No forced frame loop
  is added; the sampler schedules only its exit deadline.
- RSS includes process CPU memory and mapped native/driver allocations, but is
  not VRAM accounting. `/proc` high-water RSS spans the entire process, including
  initialization. Desktop CPU is percent of one CPU core and excludes child CLIs.
  Installed harness probes may still be running after the warmup.
- Seek tests use 1280x720 preview output from real 1080p/4K sources. They exercise
  decoding, compositing and channel delivery, not audio-clock playback or export
  throughput. The current fixtures are SDR H.264; HEVC, VFR, alpha, HDR and many
  distinct source files need separate performance lanes before generalizing.
- The agent lane measures local editing and cached transcript mapping. Model
  inference, network transport, MCP protocol round trips and Whisper execution
  are excluded. It does not time a complete autonomous edit session.

## Next measurements

Continue each lane with representative real projects. Add continuous playback
and dropped-frame counts, mouse/keyboard-to-preview latency, repeated open/close
cycles, long undo histories, many distinct source files, and export throughput.
Measure Windows independently. Investigate source verification concurrency and
thumbnail LRU updates only with a workload that exposes their actual cost.


## PF1 S4c qualified evidence (R91/R92/R93)

Fresh S4c collection is limited to desktop LL G4/G15/D13 on lavapipe, with frozen
documents/media, source-bound release binaries and native child receipts. It is
source-bound at `4783d5f` (1,190 counted lines): R92's single fresh G4 series
passes all 24 valid rows with zero missed callbacks and both arms above every
floor; G15 and D13 native contracts pass. The original invalid G4 series and
finding are explicitly superseded, with their bytes unchanged. Full workspace
fast plus strict Linux log verification passed on this source in round 3. E16 in
[PF1-EVIDENCE.md](PF1-EVIDENCE.md#e16--s4c-r91-c-plus-frozen-inputs) and the main-checkout
`target/review/pf/s4c-report.md` carry the exact outcomes, raw evidence and source
identity. The standing report-only PERFORMANCE lanes above are deferred; these
LL checks do not replace those measurements. G4 uses the accepted S3b shipping
state with a common test overlay and makes no speedup claim. S0 cleanup uses
successful process exit and parent reap; the candidate also checks its engine's
bounded teardown. Whole-process active RSS remains report-only pending a separate
allowance; G15's 4 MiB limit is only for settled idle.

The six-row G4 fps ranges are blend 29.526–29.959 (floor 15), explainer
29.859–29.959 (24), reel 29.826–29.926 (15), and typical 29.776–29.959 (24).
Aggregate validation and cached figures exit 0. R92-2 diagnostics for both
attempts are under `target/review/pf/s4/s4c/round-4/`: native nice 10 is observed;
new media/pacer-group threads are observed at nice 10, Linux priority 30,
SCHED_OTHER. Original per-thread priority is source-derived, not observed.
All 360 new five-second load samples contain contamination annotations;
one-minute load ranges from 6.22 to 20.59 and does not gate. Other projects'
processes were untouched; no worker build or test overlapped timing.

R93 changes only Python checkers/runners/tests: **1,260/1,260** retained counted lines, including the 31-line S0 overlay; no Rust, native, shipping, unsafe or lint-policy change. Fixed cached validation returns 0 for G4 round 4 (24 rows), G15 round 3 final (42 rows), and D13 round 3 final (3 rows), including successful wrapper-receipt agreement, D13 identity bounds/order and the lean frozen-media guard. No lane was re-measured. Collection used the pre-fix runner archive `8dab58af55029abcfea54e2c5188c3fbcd9f1037cbf737ad2bd2b34289115916`; this is not evidence collected by the fixed runner. The measured runners remain at the pinned `target/review/pf/s4/work/` paths. The separate fixed runner is archived at `target/review/pf/s4/archives/d3f9851547ee46439f670791a3a71f709d71950c6f8f6caf7284e83457da2c51.tar` (SHA-256 `d3f9851547ee46439f670791a3a71f709d71950c6f8f6caf7284e83457da2c51`) and extracted under `s4/s4c/fix-r93/fixed-runners/` for PF1-LH; `target/review/pf/s4c-fix-report.md` records both archives and the initial fingerprint mismatch.

The consumed-input record proves the frozen bytes at import and at the later checks. It does not prove the bytes the decoder opened. Substitution and restoration during a run, and resources referenced from inside a container, are not detected. The full decoder-bound input snapshot is carried to PF1-LH.

Proposed qualified closure (pending the narrow re-check and both-OS/both-tier CI): **PF1 closes qualified on this desktop's recorded lavapipe stack: LL G4 passes all 24 fresh valid rows, G15 passes all 42 fresh-child settled-idle checks, and D13 passes its three-child/30-cycle retained-plane oracle and cleanup checks; whole-process active RSS remains unresolved/report-only, no RTX 3090 or supported-hardware playback claim is made, and every named deferral in E16 remains deferred, not passed; the consumed-input record proves frozen bytes at import and later checks, but does not prove decoder-opened bytes or detect substitution and restoration during a run or resources referenced from inside a container.**

D13 cleanup failure can emit one late `D13 sample` line within the same already-failed test binary (RR-2, R88).

- AVBuffer padding beyond the plane spans is not observed;
- FFmpeg-internal buffer-pool lifetimes are not observed;
- identity means plane address plus a lifetime generation, and a generation is recorded at each registration so that a reused pooled address is a new identity.

The historical 8,250 ms hold remains unexplained.

The R91 qualified scope carries these named deferrals, each **deferred, not passed**:

- I4, the LH part of I5, I8's lane-based L6, G3, G6, G7b, G8/L1–L6 and L4b, G11, G16, G17, both G18 decisions, R65 historical/intervening/cumulative, the standing PERFORMANCE reports, D6/the manual-VM WARP floor, and the RTX 4050 rerun;
- automatic journal-only sealing (B3). Until it exists, an interrupted unsealed series blocks: preserve every byte, stop, and return for a ruling (critic §2);
- the numerical whole-process RSS allowance, which stays unresolved;
- the D13 witness-body refactor (R86 B-7).
- the common LH collector and Q1(b) default-policy variant — deferred to PF1-LH, not passed;
- LH G1, G2 (including the 240 s talk) and G14 — deferred to the scheduled PF1-LH follow-up slice, not passed.

G3 and LH L4a remain unresolved under driver defaults. A failed in-scope LL gate remains a finding for lead disposition.


PF1-LH entry items under R93 (these do not defer PF1 gates): the full decoder-bound input snapshot (A-1); G4 floors in the pins schema (B-3); renaming G15's legacy protocol field (B-4); and a Linux CI job restoring the runner archive and running `test_lane.py` with `PF1_RUNNERS` (B-5).
