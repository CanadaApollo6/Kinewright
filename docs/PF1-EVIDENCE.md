# PF1 evidence: pre-design measurements, provenance, and the session checklist

> Companion to `docs/PF1-PLAYBACK-PERFORMANCE.md` (revision 4). Not line-budgeted (rulings R9,
> R14). Holds the non-normative material moved out of the design: the diagnosis
> table (E0), the revision-2 and revision-3 changelogs (E6, E7) and the preview
> cache arithmetic (E8).
> These are worker measurements from 2026-09-27 on a scratch worktree of main
> `6cbc150`. The worktree is deleted, but the instrumentation patch and the
> commands can be replayed exactly with `scripts/pf1-probe-replay.sh` in this
> directory (§E2).
>
> **Status:** evidence for the design's diagnosis, **not** gate baselines.
> The S0 harness re-measures every number it gates, on the pinned FFmpeg,
> through the engine (design §2 and §11).

## E0 Diagnosis table (moved from design §1, revision 2)

Main 6cbc150, measured with worker instrumentation. Each evidence table has
its own measurement boundary (E1).

| # | Finding | Evidence |
|---|---|---|
| 1 | `WorkingFrame::from_rgba64_le` costs 36 ms per 1280×720 source frame (81 ms at 1080×1920): per-channel `powf` plus a per-pixel `collect::<Result<Vec<_>, _>>()` allocation | T1, T5 |
| 2 | Two sources thrash the 224 MiB cache: Sequential windows (`PREFETCH_FRAMES` + 1) convert eagerly and `evict_oldest_video_frame` round-robins `source_order`, evicting the other source's next frames. Per output frame: 12.2 conversions, 38 decodes, 0.88 seeks | T4 |
| 3 | `Worker::tick` → `present` renders synchronously (p50 36 ms, p95 1.7 s); `fill_audio` runs only between renders, so the ring underruns | T4 |
| 4 | Preview round-trips GPU → CPU → GPU: `readback_for`'s per-pixel encode costs 19–26 ms, then `ColorImage::from_rgba_unmultiplied` + `TextureHandle::set` | T4, T6 |
| 5 | `open_scaled_internal` gives every decoder min(parallelism, 16) frame threads: 139 MiB per 1080p decoder at 16 threads | T1, T8 |
| 6 | swscale 0.8 ms and plane copy 0.6 ms: small until item 1 is fixed | T1 |
| 7 | A paused seek is keyframe → target decode (12 / 41 ms at GOP 60 / 250, 16 frame threads, decoder-level) plus one 36 ms conversion; a +1 scrub step always reseeks | T3 |
| 8 | `render_output` advances `SharedClock` by every callback frame, underrun included: permanent drift | T7 |
| 9 | `PREVIEW_MAX_WIDTH` caps width only: a 9:16 monitor frame is 1080×1920 | T1 |

**Provisional post-S1 model:** ≈ 4 ms per 1280×720 source frame (0.8 swscale
+ 0.6 copy + 2.3 table conversion) plus decode service time. T1's 0.1 ms
"decode" is exposed waiting overlapped by conversion, not decode cost; readers
add contention. S1 closes by re-measuring this model with the S0 harness.

## E1 Provenance

**Machine and software.**
- i5-13600K (20 hardware threads), 31 GiB RAM.
- RTX 3090 on NVIDIA driver 615.71.09. lavapipe from Mesa, as in ME13
  (LLVM 22.1.8).
- Linux 7.2.5 (Omarchy).

**Engine build.**
- Commit: `6cbc150` plus the instrumentation patch.
- Release profile, `cargo test --release -p kinewright-media --lib`.
- The release test binary was `kinewright_media-47548499709b80d3`.

**Decode libraries.**
- In-engine probes (T1 and T3–T9) link the pinned build
  `third_party/ffmpeg`, n8.0-23-gd1f31a829d, through
  `scripts/setup-ffmpeg.sh`.
- Media generation and the T2 CLI timings used the **system**
  `/usr/bin/ffmpeg` (n9.0.2) with its libx264.
- The S0 fixtures regenerate media through `test_support`'s pinned tool.
  The encoded bytes will therefore differ from these files. The GOP,
  size, noise and colour arguments are the same.

**Media.**
- All media is 30 fps and carries the pinned MO2 x264 arguments:
  `-c:v libx264 -preset veryfast -pix_fmt yuv420p`, bt709 primaries,
  transfer and matrix, tv range, and x264-params repeating them.
- Files:
  - `a1080_g60`: testsrc2 1080p, GOP 60, 600 frames.
  - `b1080_g60`: smptebars 1080p, GOP 60.
  - `a1080_g250`: testsrc2 1080p, GOP 250.
  - `p1080x1920_g60` and `f1080x1350_g60`: portrait and 4:5.
  - `a2160_g60`: 4K, 300 frames.
  - `n1080_g60`: `noise=alls=10:allf=t`.
  - `talk1080_g250_10min`: noisy 1080p, GOP 250, 600 s, with an AAC
    128k 220 Hz tone. About 8.4 Mb/s.

**Instrumentation.** Atomic phase counters were added around:
- demux, `send_packet`/`receive_frame`, and the filter graph;
- `read_plane` + flip + rotate;
- `WorkingFrame::from_rgba64_le`.

`phases::render_split` then splits `FrameRenderer` into four parts:
`decoded_layers`, upload, GPU plus readback, and CPU monitor encode.

The counters add roughly 10% to end-to-end means. Compare: MO2 R28
measured 494–498 ms, against 549–553 ms here.

**What each timing includes.** The tables do not share one boundary; the
critic's S2 point.

| Table | Boundary |
|---|---|
| T1 | Decoder-side **exposed wait**. The ~0.1 ms "decode" with frame threads is the time `receive_frame` blocked while the previous frame's 36 ms conversion overlapped the decode. It is not decode service time. |
| T2 | CLI throughput to `-f null`. For hwaccel runs the output stays in device memory (`-hwaccel_output_format cuda` / `vulkan`), so no download is included. |
| T3 | **Decoder-level**: `VideoDecoder::decode_window(t, t)` on one decoder at the 1280 proxy with a 32-frame `FrameCache`, no renderer, compositor or engine. 30 random seeks (LCG, seed 12345) per row after one warm decode, then 30 consecutive +1-frame steps from mid-duration. Each seek converts exactly one frame. |
| T4 | Instrumented `FrameRenderer` render loop. Not engine presentation. |
| T6 | Encode only, over an already-read-back buffer. It excludes the readback. |

## E2 Commands

**Setup.** The replay script has two phases, run one at a time from a shell
where `setup-ffmpeg.sh` has not been sourced:
- `bash scripts/pf1-probe-replay.sh prepare` (the default) creates
  `../Kinewright-pf-probe` at `6cbc150`, symlinks `third_party`, generates
  `/tmp/pf1` with system `ffmpeg` (probing each file separately), and applies
  the patch. Every scripted source edit asserts the exact number of
  occurrences of its anchor text (including both `sed` substitutions, which
  must each match once before and zero times after), so a drifted checkout fails
  loudly instead of silently skipping or over-applying an edit. It builds
  nothing.
- `bash scripts/pf1-probe-replay.sh build` sources
  `setup-ffmpeg.sh` in the worktree and builds the release test binary
  (`--no-run`), printing its path.

**Run.** Then, one heavy command at a time:

```bash
cd ../Kinewright-pf-probe && source ./scripts/setup-ffmpeg.sh
B=target/release/deps/kinewright_media-<hash>          # printed by the build phase
$B --ignored pf1_probe::pf1_decode_breakdown --nocapture --test-threads=1   # T1
$B --ignored pf1_probe::pf1_seek_latency     --nocapture --test-threads=1   # T3
$B --ignored pf1_probe::pf1_e2e_lavapipe     --nocapture --test-threads=1   # T4 lavapipe
$B --ignored pf1_probe::pf1_e2e_hardware     --nocapture --test-threads=1   # T4 3090
for t in pf1_transfer_lut pf1_monitor_encode_lut pf1_underrun_advances_clock \
         pf1_texture_write_cost; do
  $B --ignored pf1_probe::$t --nocapture --test-threads=1                   # T5 T6 T7 T9
done
for t in 16 8 4 1; do
  PF1_T=$t PF1_FILE=/tmp/pf1/n1080_g60.mp4 \
    $B --ignored pf1_probe::pf1_decoder_rss --nocapture --test-threads=1    # T8
done
```

**Filtering output.** Extract the result lines with `grep -oE "PF1 .*"`.

**T2 (system `ffmpeg`, same `/tmp/pf1`).**
```bash
ffmpeg -hide_banner -benchmark $MODE -i talk1080_g250_10min.mp4 -frames:v 3000 -an -f null -
```
`$MODE` is one of:
- `-threads 1`
- `-threads 16`
- `-hwaccel cuda -hwaccel_output_format cuda`
- `-init_hw_device vulkan=vk:0 -hwaccel vulkan -hwaccel_output_format vulkan`

The 1080p and 4K rows use the same modes on `a1080_g60.mp4` and
`a2160_g60.mp4`. A VAAPI mode was also attempted, but its output was not recorded.

**Cleanup.**
```bash
rm ../Kinewright-pf-probe/third_party
rm -rf ../Kinewright-pf-probe/target
git worktree remove --force ../Kinewright-pf-probe
rm -rf /tmp/pf1
```

## E3 Results

**T1 Sequential decode per frame.** Proxy width 1280, 240 frames, times in
ms per frame.

| Source | Threads | Decode (wait) | swscale | Plane | Transfer | Total | 16-frame window |
|---|---|---|---|---|---|---|---|
| 1080p GOP 60 | frame 1 | 2.65 | 0.82 | 0.65 | 36.06 | 40.5 | 649 |
| 1080p GOP 60 | frame 16 | 0.13 | 0.82 | 0.56 | 36.21 | 38.1 | 610 |
| 1080p GOP 60 | slice 16 | 2.62 | 0.81 | 0.55 | 36.23 | 40.6 | 649 |
| noisy 1080p GOP 60 | frame 1 / 4 / 16 | 3.82 / 0.08 / 0.07 | 0.81 | 0.59 | 36.4 | 41.7 / 38.3 / 38.3 | 667 / 613 / 612 |
| 1080×1920 | frame 16 | 0.07 | 1.18 | 1.69 | 80.89 | 84.3 | 1,348 |
| 1080×1350 | frame 16 | 0.06 | 0.88 | 1.06 | 57.01 | 59.3 | 949 |
| 4K → 1280 | frame 1 / 16 | 9.56 / 0.32 | 1.35 | 0.6 | 35.9 | 48.7 / 40.1 | 779 / 641 |
| talk | frame 1 / 16 | 3.57 / 0.09 | 0.81 | 0.6 | 36.2 | 41.4 / 38.2 | 663 / 612 |

**T2 Hardware decode (CLI wall time, system FFmpeg 9.0.2).**

| Source | SW 1 thread | SW 16 threads | cuda (NVDEC) | vulkan |
|---|---|---|---|---|
| talk, 3,000 frames | 10.3 s | 1.46 s | 4.63 s (~0.3 s CPU) | 4.73 s |
| 1080p, 600 frames | — | 0.24 s | 0.97 s | — |
| 4K, 300 frames | — | 0.55 s | 1.84 s | — |

- cuda initialisation took 0.054 s.
- The build offers these hwaccels: vdpau, cuda, vaapi, qsv, drm, opencl,
  vulkan and amf.
- This shows that NVDEC and Vulkan Video are slower **for these H.264
  8-bit files on this host**. It is not a general claim (critic S9).

**T3 Paused seek and scrub.** Decoder-level (E1): 30 random seeks per row
(LCG seed 12345), then 30 +1-frame steps; ms, mean / p95. Every seek converted
exactly one frame (≈ 35 ms). "Decode per seek" is the counted decoder phase
mean. The design's ≈ 95 ms decode at GOP 250 / 16 threads is an **inferred
residual** (p95 131 ms minus one ≈ 36 ms conversion), not a measured decode
p95; upload, composite, encode and consumer latency are not in T3.

| Source | Threads | Seek | Frames decoded per seek | Decode per seek | +1-frame step |
|---|---|---|---|---|---|
| GOP 60 | 1 / 4 / 16 | 108 / 180, 60 / 81, 52 / 61 | 26.2 | 71 / 23 / 12 | 85 / 119, 54 / 66, 49 / 56 |
| GOP 250 | 1 / 4 / 16 | 352 / 677, 133 / 217, 83 / 131 | 122.2 | 312 / 94 / 41 | 207 / 243, 94 / 106, 67 / 77 |
| talk | 1 / 4 / 16 | 305 / 677, 132 / 237, 83 / 135 | 72.9 | 267 / 92 / 42 | 335 / 473, 140 / 193, 88 / 109 |

Slice 16 equals frame 1 in every row.

**T4 End to end.** `FrameRenderer` playback, `DecodeStrategy::Sequential`,
1280×720 proxy, 30 + 150 frames, one run. Phase columns are ms per output
frame.

| Lane | Workload | Cache | Mean / p50 / p95 | fps | decoded_layers (transfer) | Upload | GPU + readback | Encode | Converts / decoded / seeks per frame |
|---|---|---|---|---|---|---|---|---|---|
| lavapipe | typical | 224 MiB | 553 / 38 / 1,707 | 1.8 | 518 (482) | 8.8 | 6.3 | 19.4 | 12.23 / 38.4 / 0.88 |
| lavapipe | typical | 1 GiB | 157 / 34 / 1,221 | 6.4 | 123 (115) | 8.9 | 6.2 | 19.4 | 2.87 / 8.6 / 0.19 |
| lavapipe | blend_heavy | 224 MiB | 1,973 / 2,084 / 2,195 | 0.5 | 1,927 (1,800) | 8.8 | 10.7 | 26.3 | 44.0 / 137.5 / 2.97 |
| lavapipe | blend_heavy | 1 GiB | 165 / 46 / 1,184 | 6.1 | 118 (112) | 9.0 | 10.9 | 26.4 | 3.13 / 3.1 / 0.00 |
| 3090 | typical | 224 MiB | 549 / 36 / 1,700 | 1.8 | 517 (481) | 10.5 | 2.1 | 19.3 | 12.23 / 38.4 / 0.88 |
| 3090 | typical | 1 GiB | 154 / 31 / 1,221 | 6.5 | 122 (114) | 10.7 | 2.3 | 19.0 | 2.87 / 8.6 / 0.19 |
| 3090 | blend_heavy | 224 MiB | 1,973 / 2,089 / 2,185 | 0.5 | 1,934 (1,807) | 10.7 | 2.7 | 26.0 | 44.0 / 134.8 / 2.97 |
| 3090 | blend_heavy | 1 GiB | 157 / 38 / 1,183 | 6.4 | 118 (113) | 10.4 | 2.5 | 25.7 | 3.13 / 3.1 / 0.00 |

The 1 GiB rows still convert on the Sequential window (p95 ≈ 1.2 s), so a
larger cache alone does not reach real time.

**T5 Exact transfer table.** Bt709 / Limited / 8-bit / Bt709 matrix, at
1280×720:
- per-pixel path 39.17 ms;
- 65,536-entry table 2.25 ms;
- bit-identical over all 65,536 codes **for that one descriptor** (B1:
  it does not establish other descriptors).

**T6 Exact monitor encode table.** At 1280×720:
- per-pixel `encode_monitor_rgba8` 26.03 ms;
- f16-bit-indexed table 1.22 ms;
- bit-identical over all 65,536 f16 patterns, including NaN, ±Inf and
  denormals.

**T7 Underrun.** `render_output` with an empty ring and a 1,024-frame
callback writes all silence and advances the position by 1,024.

**T8 Decoder RSS.** VmRSS delta, noisy 1080p at the 1280 proxy, 32 frames
decoded per decoder, one process, decoders opened cumulatively. Values are
MiB for 1 / 2 / 3 / 4 decoders.

| Frame threads | 1 / 2 / 3 / 4 decoders |
|---|---|
| 16 | 139 / 290 / 437 / 593 |
| 8 | 85 / 192 / 275 / 379 |
| 4 | 59 / 129 / 193 / 258 |
| 1 | 35 / 79 / 134 / 159 |

**T9 Upload.** `queue.write_texture` of one 1280×720 layer, mean:

| Lane | rgba8 | rgba16f |
|---|---|---|
| lavapipe | 0.31 ms | 0.80 ms |
| 3090 | 0.57 ms | 1.09 ms |

The ME15 per-layer mapped-at-creation staging path measures about 2.6 ms
per layer.

**Not measured.**
- WARP (the VM needs Riel).
- egui present cost.
- `WorkingFrame::upload_bytes`'s extra CPU copy (critic S1). S0 measures
  it.
- App-process idle and playback RSS.

## E4 Raw result lines (T4 and T8)

```
PF1 e2e adapter=lavapipe workload=typical_1080p cache=default_224MiB dims=(1280, 720) mean_ms=552.9 p50=38.0 p95=1706.5 fps=1.8 | decoded_layers=518.4 (demux 0.8 decode 11.4 swscale 10.3 plane 8.6 transfer 481.9) upload=8.8 gpu+readback=6.3 monitor_encode=19.4 | per frame: converts=12.23 decoded=38.38 seeks=0.88
PF1 e2e adapter=lavapipe workload=typical_1080p cache=resident_1GiB dims=(1280, 720) mean_ms=157.2 p50=34.0 p95=1221.3 fps=6.4 | decoded_layers=122.6 (demux 0.2 decode 2.5 swscale 2.4 plane 1.7 transfer 114.6) upload=8.9 gpu+readback=6.2 monitor_encode=19.4 | per frame: converts=2.87 decoded=8.55 seeks=0.19
PF1 e2e adapter=lavapipe workload=blend_heavy_1080p cache=default_224MiB dims=(1280, 720) mean_ms=1973.0 p50=2084.3 p95=2194.6 fps=0.5 | decoded_layers=1927.1 (demux 2.5 decode 38.6 swscale 36.3 plane 30.7 transfer 1800.0) upload=8.8 gpu+readback=10.7 monitor_encode=26.3 | per frame: converts=44.00 decoded=137.52 seeks=2.97
PF1 e2e adapter=lavapipe workload=blend_heavy_1080p cache=resident_1GiB dims=(1280, 720) mean_ms=164.5 p50=45.9 p95=1183.5 fps=6.1 | decoded_layers=118.1 (demux 0.1 decode 0.1 swscale 2.5 plane 2.1 transfer 112.1) upload=9.0 gpu+readback=10.9 monitor_encode=26.4 | per frame: converts=3.13 decoded=3.13 seeks=0.00
PF1 e2e adapter=hardware workload=typical_1080p cache=default_224MiB dims=(1280, 720) mean_ms=548.6 p50=36.4 p95=1699.5 fps=1.8 | decoded_layers=516.8 (demux 0.8 decode 11.3 swscale 10.2 plane 8.3 transfer 480.9) upload=10.5 gpu+readback=2.1 monitor_encode=19.3 | per frame: converts=12.23 decoded=38.38 seeks=0.88
PF1 e2e adapter=hardware workload=typical_1080p cache=resident_1GiB dims=(1280, 720) mean_ms=154.0 p50=31.2 p95=1221.1 fps=6.5 | decoded_layers=122.0 (demux 0.2 decode 2.4 swscale 2.4 plane 2.0 transfer 113.8) upload=10.7 gpu+readback=2.3 monitor_encode=19.0 | per frame: converts=2.87 decoded=8.55 seeks=0.19
PF1 e2e adapter=hardware workload=blend_heavy_1080p cache=default_224MiB dims=(1280, 720) mean_ms=1972.9 p50=2089.1 p95=2185.1 fps=0.5 | decoded_layers=1933.6 (demux 2.5 decode 38.4 swscale 36.6 plane 29.8 transfer 1807.2) upload=10.7 gpu+readback=2.7 monitor_encode=26.0 | per frame: converts=44.00 decoded=134.83 seeks=2.97
PF1 e2e adapter=hardware workload=blend_heavy_1080p cache=resident_1GiB dims=(1280, 720) mean_ms=156.7 p50=38.0 p95=1183.3 fps=6.4 | decoded_layers=118.1 (demux 0.1 decode 0.1 swscale 2.5 plane 1.8 transfer 112.5) upload=10.4 gpu+readback=2.5 monitor_encode=25.7 | per frame: converts=3.13 decoded=3.13 seeks=0.00
PF1 rss file=/tmp/pf1/n1080_g60.mp4 threads=16 decoders=1..4 rss_delta_mib=138.7 289.5 437.2 593.2
PF1 rss file=/tmp/pf1/n1080_g60.mp4 threads=8  decoders=1..4 rss_delta_mib=85.4 192.2 274.8 378.9
PF1 rss file=/tmp/pf1/n1080_g60.mp4 threads=4  decoders=1..4 rss_delta_mib=58.8 128.6 193.4 257.5
PF1 rss file=/tmp/pf1/n1080_g60.mp4 threads=1  decoders=1..4 rss_delta_mib=35.4 79.2 134.2 158.7
```

## E5 Production-session checklist (moved from design §15)

Before the production session that makes the three videos:

- [ ] Every PF1 exit gate (design §11) is green on its lane. The WARP VM
  numbers are recorded.
- [ ] On the 3090, in the app, with the real footage:
  - the presenter + cutaways explainer plays at 30 fps with no stutter at
    cuts;
  - the 9:16 Reel and the 4:5 feed cut play in real time;
  - the recut talk plays across every cut without a freeze.
- [ ] The status line shows ≤ 1% dropped (and zero held-frame streaks
  over 100 ms) across a full pass of each video.
- [ ] Scrubbing a 10-minute source feels immediate, and the frame after
  a drag is the drag's final target.
- [ ] Presenter lip sync is right under deliberate CPU load (a parallel
  build), with no lasting offset.
- [ ] Agent `preview_frame` during playback returns promptly. Its
  recorded interruption is noted.
- [ ] The real footage's codec, GOP, resolution, bit depth and HDR flags
  are recorded. HEVC, 10-bit or HDR footage triggers the D1/D2 revisit
  and the CC8 check.
- [ ] Playback RSS is recorded against G15 and the P-rss rows. VRAM is recorded against
  the ledger.

## E6 Revision-2 changelog (moved from the design, revision 3)

The revision-2 findings (B = blocking, S = should-fix) and where revision 2
resolved them. Revision 3 supersedes several of these sections; the design's
revision-3 changelog maps RB/RS findings.

| Finding | Where revision 2 resolved it |
|---|---|
| B1 | §4 X-1/X-2: the key is the full validated descriptor; parity is enumerated through `classify_source_with_assumption` (R1) |
| B2 | §4 X-4, §10 G-2, §12: tables and GPU encode are SDR only; HDR stays fused f32 on the CPU path, owned by CC8 (R2) |
| B3 | §6 H-2…H-6: `Sched` state, wait predicates, notifications, lock order, shutdown sequence |
| B4 | §7 R-1…R-3: generation stamps checked at the final consumer on both paths; request kinds; frames held until due |
| B5 | §10 G-3/G-4/G-7: shared queue, slot state machine, completion-owned resources, charges that outlive app views |
| B6 | §10 G-5: a completion-owned flag readback keeps the typed `NonFiniteRender` refusal |
| B7 | §5: hard ceiling, admission classes, arithmetic, separate full-resolution/export policy (R3) |
| B8 | §6 H-5: thread permits; L-1/L-2 restated with arithmetic (R4) |
| B9 | §8: flush-at-keyframe continuation, bounded backward window, hit vs refill latency, witnesses |
| B10 | §9 V-1/V-2: whole programme frames counted regardless of content; named test amended; drained EOS (R6) |
| B11 | §2 Q-2/Q-3, §9 V-4/V-5: due-frame outcomes, consumer acks, held age, freeze control, device-free driver, 60 s runs (R7) |
| B12 | §2 Q-4: ME13 `BASELINES` untouched; separate `PF1_PINS` with provenance (R5) |
| S1 | §13 S1a/S1d/S1e: `collect` removal, source-aware window cap, `upload_bytes` copy |
| S2 | §1 (provisional model, re-measured after S1); evidence E1 boundary table; deleted instrumentation replayable by script |
| S3 | §3 W-1/W-2: `talk_recut` pinned at 240 s with deterministic spans; AW2 limitation; PERFORMANCE lanes kept |
| S4 | §2 Q-1, §11: lanes and classes; honest "today" values; pending-baseline gates marked |
| S5 | §7 R-4: bounded FIFO agent lane, fairness, document-local LUT; drops measured, not promised |
| S6 | §6 H-7: startup, first-render and settled-idle budgets; AW1 B5; per-OS RSS collection |
| S7 | §12 (PF1 → CC8 S2 → CC8 S3, R1), §13 (R8 order, harness counted, budget re-estimated) |
| S8 | §4 X-5 inventory; §10 G-6 zero-tolerance self-check with CPU fallback |
| S9 | §14 E-1/E-2 typed errors kept; §15 D1/D2 narrowed |
| Nits | §3 (named workloads, `perf_fixtures` extraction); §1 (no drafting residue); §10 G-3 (no egui_wgpu in media), P-1 (4:5 = +42%, D8); §11 C-5 (G-1 is monitor-only) |

## E7 Revision-3 changelog (moved from the design, revision 4)

The rev-2 critic's RB/RS findings and where revision 3 resolved them. Revision
4 supersedes K-3 streaming/banding (R15 replaces R10), H-5 revocation (R16),
the R-5 ack boundary and G-4 teardown (R17) and the S-2 domain (R18); section
numbers refer to revision 3.

| Finding | Where revision 3 resolved it |
|---|---|
| RB1 | §6 H-2 state table (runnable/idle/waiting per participant), H-3 versioned results and failures, timed reader retirement, H-4 guards dropped outside `Sched`, H-6 replies sent after unlock, H-8 model interleavings |
| RB2 | §6 H-5: `Permits` monitor covering sync decoders retained by `FrameRenderer::video_sources`; no hold-and-wait; cooperative revoke/reopen; P = 1/2/3; arithmetic follows the algorithm |
| RB3 | §5 K-2 atomic required reservation, speculative eviction/cancellation first, waits only on live ownership; K-3 streaming and banding replace the ceiling exception (R10) |
| RB4 | §7 R-1 stamps taken at issue (incl. `request_frame`) and bound to target/document/LUT; R-2 re-validation at `finalize_preview`; stamped errors |
| RB5 | §10 G-4 root full-frame-epoch fence (R11); G-7 completion owner with a submission-index token through teardown |
| RB6 | §8 S-2 continuation only inside a stated equivalence domain, else today's seek (R12); S-3 refill cost includes the GOP |
| RB7 | §9 V-2 `Worker::stop_at_end`, not `pause()`; drained predicate avoids the pop/update window |
| RB8 | §7 R-5 acks at the paint-acceptance boundary; §2 Q-2 fixed denominator and wall-clock guard; §11 G14 held age ≤ 100 ms |
| RS1 | §4 X-2 enumerates primaries and white point through the production dispatch; X-1 empty-frame error behaviour corrected |
| RS2 | §3 W-2 and §12: AW0 A1 reconciled (cached external renders are ordinary media; native SVG gates absent here; no live producer) |
| RS3 | §5 K-4 exact shares; §6 H-5 L-1/L-2 provisional; §8 S-3 clamps |
| RS4 | §11 every gate has an evidence class (direct vs pending-baseline); timing only on LL/LH/VM; H-7 RSS is a pinned local gate |
| RS5 | evidence E1/E3 T3 corrected (30 decoder-level seeks); replay split into `prepare`/`build`, ffprobe loop, checked replacements |
| RS6 | §13 stages S0, S1, S2a, S2b, S2c, S3a, S3b, S4 (R13) with their own invariants; I12 moved to S2b; premultiply to S3a; budgets re-estimated |
| RS7 | §7 R-4 fair selection with a service bound, `CancelOnDrop`, exactly-once replies, agent errors only via the reply |
| RS8 | §4 X-5 table bound via live decoders ≤ P; §10 G-7 display textures charged at actual size; §11 G15 title-only idle RSS |
| Nits | `App::ui`/`App::logic`/root epoch terminology throughout; §11 L-3/L-5 boundaries; evidence E5 references fixed |

## E8 Preview cache arithmetic (moved from design K-4)

K-2's lookahead shares: H = n_active · f and share = (C − G − H) / n_active,
with f = 7.031 MiB at 1280×720 or 720×1280 and 10 MiB at 1024×1280.

| Case | Share per source | Lookahead |
|---|---|---|
| typical: 2 sources, 3 titles | (224 − 21.09 − 14.06) / 2 = 94.42 MiB | 13 |
| `reel_9x16`: 3 sources, 4 titles | (224 − 28.13 − 21.09) / 3 = 58.26 MiB | 8 |
| `feed_4x5`: 3 sources, 4 titles | (224 − 40 − 30) / 3 = 51.33 MiB | 5 (starved, recorded) |
| 8 sources at 720p, 3 titles (P = 20, R = 8) | (224 − 21.09 − 56.25) / 8 = 18.33 MiB | 2 (starved) |
