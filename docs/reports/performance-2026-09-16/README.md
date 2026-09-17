# Performance pass 1 — September 16, 2026

This pass establishes separate typical-editing, heavy-editing and local-agent
workloads, plus native desktop startup/process measurements. It retains four
changes: fewer temporary document copies during atomic edit plans, memoized
saved-state comparisons, cheaper, bounded MCP servers, and media-driven UI wakeups. The decoder-thread
experiments were rejected; production decode, preview quality and export behavior
remain unchanged.

## Retained changes

- `apply_batch` uses one unpublished candidate for the whole plan. Initial state
  is validated once; every intermediate edit is still recomputed and validated.
  Rejections preserve the original document and the same failing operation index.
  Snapshot undo/redo and the operation journal retain their existing semantics.
- `ProjectSession::is_dirty` caches the comparison against both document
  identities, avoiding a deep equality walk on every frame. It still recognizes
  undo back to saved content, independently allocated equal snapshots, no-op
  edits, and replacement of the saved document. Arc ownership prevents stale
  verdicts from pointer reuse or mutation through a unique Arc. Fast paths clear
  obsolete cached snapshots after a save or removal of the saved snapshot; a
  Weak-reference regression test verifies both allocations are released.
- The seven exposed MCP tool descriptors are built directly and cached. The
  full registry remains available for discovery, and serialized schemas,
  descriptions, annotations and ordering are unchanged. Each ordinary MCP
  server now uses two async worker threads; explicit investigator overrides
  still apply. Blocking tool calls and export work retain their separate workers.
- The media worker wakes the UI after publishing a preview or event. Faster
  startup exposed a missing wakeup: without unrelated window activity, the
  first preview sometimes waited until the sampler's eight-second exit timer.
  Initial frames, paused seeks and errors now request repaint when ready,
  without adding a continuous idle repaint loop. A regression test receives
  these results only through the wake callback, verifying publication order.

## Method

Baseline: `db3ec2d1f92240e14c8b82a458f3bd6823709c20`, plus the same opt-in
measurement hooks. Linux, Intel i5-13600K (20 logical CPUs), NVIDIA RTX 3080,
Rust 1.98.0, pinned FFmpeg 8.0. Measurements use release builds. The GPU listed is
hardware inventory; the headless workload uses the application's normal wgpu
adapter selection. This is not a Windows measurement.

Each initial workload baseline has three process runs with 30 measured seeks or
100 edit plans per run. The agent project contains 1,200 clips, and each plan has
20 operations followed by undo and redo. Preview generation/probing are setup,
not seek time. Media are synthetic H.264/AAC; preview output is 1280x720. The
native empty-project sampler uses five eight-second launches, with fresh XDG
data/cache directories and the installed harness probes enabled.

The machine was shared with other work, so small timing differences are not
claimed as improvements. Timed final runs wait for this task's builds/tests to
finish. The full correctness suite was run in a separate checkout containing
only this patch, excluding concurrent agent-harness edits. The verified source
files are copied back to the working checkout; those unrelated edits are intact.

See [reproduction commands and metric boundaries](../../PERFORMANCE.md) and
[raw reports](results.json). First UI is completion of the first egui callback,
not first screen presentation. RSS is CPU process residency, not VRAM. The agent
lane excludes model/network latency and transcription generation. UI equality
and tool-descriptor microbenchmarks isolate one component; their speedups are
not whole-application speedups.

## Results retained

Final desktop results are medians of five paired runs, alternating baseline and
candidate order. Agent results are medians of three paired process runs, each
containing 100 plans. These final pairs supersede the earlier exploratory runs
also preserved in `results.json`.

| Metric | Baseline | Final | Interpretation |
| --- | ---: | ---: | --- |
| Agent 20-operation batch median | 3.024 ms | 1.477 ms | 51.2% less elapsed time |
| Agent batch p95 | 3.176 ms | 1.640 ms | 48.4% less elapsed time |
| Empty desktop OS threads | 42 | 24 | 18 fewer threads; 42.9% reduction |
| First preview reaches UI | 7,984.44 ms | 505.50 ms | Missing wakeup fixed; baseline usually waited for the exit timer |
| First UI callback complete | 333.19 ms | 341.14 ms | 2.4% higher in this run set; no startup-speed claim |
| Empty desktop idle CPU, one-core percent | 0.669% | 0.335% | Lower in this short run set; exploratory |
| Empty desktop process RSS | 289.43 MiB | 282.73 MiB | Not equivalent readiness; see below |

The first-preview baseline is **not eight seconds of decode work**. In the
unmodified app, a frame can be queued while the event loop sleeps; the sampler's
deadline eventually supplies the missing wake. Earlier runs with more incidental
window activity showed a much shorter baseline, confirming why a timing-only
optimization could conceal this bug. All five final candidate runs received a
preview well before the deadline.

The candidate has already uploaded its preview during the idle window; the
baseline generally has not. Therefore the RSS row cannot establish a memory
regression or improvement at equivalent app readiness. No whole-app RAM saving
is claimed. Idle UI timing also has only one to four samples per launch; it is
preserved as diagnostic evidence, not a responsiveness percentile claim.

Isolated component checks help explain the batch improvement and removed work:

| Component | Prior algorithm | Retained algorithm |
| --- | ---: | ---: |
| 1,200-clip, 20-operation batch median | 2.761 ms | 1.349 ms |
| 20,000 unchanged saved-state polls, 1,200 clips | 74.941 ms | 0.024 ms |
| 100 compact MCP descriptor reads | 629.457 ms | 0.030 ms, warmed cache |

These compare both algorithms in the same release test process. They do not
include rendering, network calls, or model inference. The compact-schema test
measures warmed reads; it is not a cold-server startup measurement.

Typical and heavy media lanes remain performance controls: their initial
three-run median seek times were 194.45 ms and 458.15 ms respectively. After
restoring the original decoder settings, final smoke runs measured 197.07 ms
and 464.47 ms. Heavy p95 varied materially (743.66 ms in that smoke run), so no
media throughput or tail-latency improvement is claimed. The useful result for
these lanes is a repeatable baseline and a rejected memory/latency tradeoff.

## Validation

The complete workspace suite passed before the final wakeup fix: 2,574 tests,
zero failures, 30 ignored across 31 suite summaries. After that fix, the new
wakeup regression test, all 19 media-engine tests and all 567 app tests passed.
Workspace Clippy with warnings denied, formatting, and the release workspace
and workload-example builds passed on the final source. One four-thread engine
test invocation was terminated with SIGKILL; no assertion failed,
and the complete engine group passed with one test thread on retry. The cause
of the termination was not established.

Independent GPT-5.6 Sol xhigh review found the stale document retention on save;
it was fixed and re-reviewed with no remaining technical blockers. The final
binaries and paired measurements were refreshed after that fix, and the raw
report records source and binary hashes.

## Decoder experiments rejected

The existing decoder allows up to 16 frame threads. FFmpeg documents that frame
threading adds decode delay, so smaller proxy-decode pools were tested without
changing source resolution, output resolution, cache budgets, or the full-resolution
export pool ([FFmpeg documentation](https://www.ffmpeg.org/ffmpeg-codecs.html)).

Two threads cut memory considerably but increased median seeks in both lanes.
Eight threads gave a more attractive memory result, but the paired reruns still
showed worse typical-workload tail latency:

| Paired median across three runs | Original 16 threads | Trial 8 threads | Change |
| --- | ---: | ---: | ---: |
| 1080p seek median | 189.37 ms | 193.77 ms | 2.3% slower |
| 1080p seek p95 | 208.69 ms | 229.79 ms | 10.1% slower |
| 1080p RSS after seeks | 772.40 MiB | 649.29 MiB | 15.9% lower |
| 4K seek median | 475.58 ms | 468.70 ms | 1.4% faster; within observed variance |
| 4K seek p95 | 582.00 ms | 594.67 ms | 2.2% slower |
| 4K RSS after seeks | 1,472.02 MiB | 1,098.27 MiB | 25.4% lower |

The pairing alternated baseline/candidate ordering between runs. Memory savings
were consistent, but this pass does not trade slower preview tails for lower
RSS. No decoder-thread change or decoder experiment hook is retained.

## Continue from here

The heavy-workload baseline is the next useful target: about 1.4 GiB CPU RSS,
with an approximately 218 MiB explicit preview-frame cache. Decoder/native
allocations deserve separate accounting before reducing cache quality or
parallelism. Add many distinct source files, sustained playback, long projects,
and real footage. Also measure the per-asset verification thread fan-out and
thumbnail recency maintenance, then choose changes against those measurements.
For desktop memory, add a separate measurement that settles both versions to
the same displayed preview before sampling; keep the unforced startup probe to
catch missing wakeups. Extend idle sampling beyond the short harness-probe window.
