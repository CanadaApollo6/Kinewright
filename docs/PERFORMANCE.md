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
