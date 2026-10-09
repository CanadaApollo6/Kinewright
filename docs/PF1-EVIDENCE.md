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
> Those S0 baselines are in **§E9**; the WARP VM run there is owed.
> The S1 stage-end results (I4, G3, G5, G9, G12, I9, P-play) are in **§E10**.
> The S2a results (I4, G3, G10, G11, G16, L-6, I8, I13, I18, P-play, P-seek) are in **§E11**.
> The S2b results (I4, G3, G1, G6, G11, G14, G15 provisional, G16, G17, G18, L-6, P-play, P-seek, and the
> must-pass mutations) are in **§E12**. G1 and G14 fail on LH (E12.10 D1).
> The R37 fixes, their closure table and the (partial, contaminated) timing rerun are in **§E12.13**; every LH
> figure before it was taken with the desktop screensaver running (E12.13.4).
> The R38 fixes and the merge of main are in **§E12.14**. The R41 fixes, including the Windows file-lock catch, are
> in **§E12.15**. The R43 fixes (bounded, interruptible and panic-safe retirement; the in-order walk) are in
> **§E12.16**.

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


## E9 S0 baselines

These are the S0 harness's first measurements of today's engine, through
`pf1_harness` on the pinned FFmpeg. They are the "S0" figures the design's
gates cite as **PB**. The WARP VM run is **owed**; the lead runs it by hand.

### E9.1 Provenance

**Machine and software.**
- i5-13600K (20 hardware threads), 31 GiB RAM.
- RTX 3090 on NVIDIA driver 615.71.09. lavapipe is llvmpipe (LLVM 22.1.8).
- Linux 7.2.5-4-omarchy, with PipeWire as the audio server.

**Background load.** Two `foot` terminal screensaver processes used about
1.5 cores for the whole session. The load average was 7.1 at the start and
4.8 at the end. Nothing else ran during timing: builds, tests and the timed
lanes ran strictly one at a time.

**Build.**
- `pf1/impl` at `d19bdf9` for I4, P-play and P-seek (2026-09-27, 04:03–06:11
  EDT). P-rss ran at `3df1a22` (06:17–06:25 EDT). That commit only fixes how
  the P-rss parent finds its child's result line.
- The review-fix reruns (P-seek, and the P-play spot checks) ran at
  `82fe858`; E9.9 has their provenance.
- rustc 1.98.0, release profile. The binary is
  `cargo test --release -p kinewright-media --lib`
  (`kinewright_media-cf306d076b2fd4bf`).

**FFmpeg.** The pinned `third_party/ffmpeg` (n8.0-23-gd1f31a829d-20251022),
selected through `scripts/setup-ffmpeg.sh`. It serves both as the linked
libraries and as the fixture tool (`test_support::ffmpeg_tool` reads
`FFMPEG_DIR`). System FFmpeg was not used.

**Commands.** Each was run with `--ignored --nocapture --test-threads=1`:
- `mo2_perf_fixtures::r28_end_to_end_tracked` on LL, then with
  `R28_HARDWARE=1` for LH.
- `pf1_harness::pf1_play_baseline`, `pf1_seek_baseline` and
  `pf1_rss_baseline`. LL is the default; `PF1_HARDWARE=1` selects LH.
- `PF1_HARDWARE=1 PF1_DEVICE=1 pf1_play_baseline`: the V-5 real-device
  cross-check, one run per workload.

**Workloads.**
- W-1 as built by `perf_fixtures`.
- MO2's builders, unchanged, parameterised to 60 s for P-play so that the
  denominator is 1,800:
  - `typical_1080p` = `cuts((1920,1080), 600, 3, 360, 15)`;
  - `blend_heavy_1080p` = `blend_heavy(1800)`.
- `seek_gop60`: a noisy 1080p GOP 60 single source, 60 s. It is added for
  L-1, because W-1 has no single-source GOP 60 timeline.
- `title_only`: for G15.

### E9.2 I4

ME13's `BASELINES` are untouched, and `r28_end_to_end_tracked`'s 5% rule is
green on both lanes:
- LL: 509.8 ms against a 497.86 ms baseline, +2.4%.
- LH: +2.6% against 494.39 ms.

The raw lines are in E9.8.

### E9.3 P-play

Setup (Q-2):
- Three runs per workload on the paced V-5 driver, plus one real-device run
  per workload on LH.
- Every column gives the range over the runs (a single value where the runs
  agree). Every run is 100% late or dropped, so no run is "worst" on G1.
- Every run was **valid** under the rule of the time: the clock reached the
  duration within 1.02 × duration + 0.5 s. The review fixes (E9.9) add a
  lower bound, 0.98 × duration − 0.5 s, and invalidate a run whose paced
  driver missed a callback deadline; the E9.9 spot checks meet both.
- **`underrun_frames` in this table and in E9.4 is the pre-R22 counter, and
  it overstates.** It estimated each callback's shortfall from the ring's
  occupancy *before* the callback popped, and it was read after a 250 ms
  drain past the endpoint. R22 replaced it with a count of the pops that
  actually failed, snapshot at the endpoint (E9.9). The G1 and G14
  conclusions below do not depend on it.
- These runs predate two metric fixes (review A F1 and its should-fix): a
  receipt now counts only once the clock has reached its frame (early frames
  are counted apart and fail G1), and present intervals and held age now use
  the same population, eligible receipts of newer frames up to the endpoint.
  They were not rerun (R24); the E9.9 spot checks use the new rules.

| Workload | Lane | Output | Valid | On time / late / dropped (range over runs) | Present p50 / p95 / max ms | Held max ms | A/V offset max ms | Clock stall max ms | underrun_frames (pre-R22 counter, overstated) | Peak RSS MiB | Ledger peak MiB |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `typical_1080p` | LL | simulated | all | 0 / 56–77 / 1723–1744 of 1800 | 679–1103 / 1317–1578 / 1553–1708 | 1550–1708 | 1567–1700 | 45.5–46.0 | 260096–525312 | 863–1237 | 56.3 |
| `blend_heavy_1080p` | LL | simulated | all | 0 / 30 / 1770 of 1800 | 1997–2008 / 2045–2054 / 2050–2080 | 2048–2079 | 2067 | 45.4–46.1 | 1398784–1405952 | 1454–2050 | 63.3 |
| `explainer_16x9` | LL | simulated | all | 0 / 67 / 1733 of 1800 | 698–726 / 1355–1373 / 1467–1502 | 1466–1499 | 1467–1500 | 45.3–46.4 | 317440–330752 | 1729–2297 | 56.3 |
| `reel_9x16` | LL | simulated | all | 0 / 37–41 / 1759–1763 of 1800 | 1317–1336 / 2033–3254 / 2570–3302 | 2565–3300 | 2533–3267 | 45.4–48.4 | 933888–1110016 | 1984–2456 | 156.9 |
| `feed_4x5` | LL | simulated | all | 0 / 60–61 / 1739–1740 of 1800 | 1020–1027 / 1997–2043 / 2079–2188 | 2078–2185 | 2100–2167 | 45.8–47.8 | 534528–558080 | 2289–2525 | 114.5 |
| `talk_recut` | LL | simulated | all | 0 / 360–361 / 6839–6840 of 7200 | 675–676 / 720–725 / 774–844 | 773–844 | 767–833 | 45.7–46.2 | 7168–8192 | 2058–2240 | 42.2 |
| `typical_1080p` | LH | simulated | all | 0 / 61–65 / 1735–1739 of 1800 | 986–1010 / 1453–1568 / 1618–1726 | 1614–1723 | 1633–1700 | 45.4–45.5 | 367616–492544 | 1037–1418 | 56.3 |
| `blend_heavy_1080p` | LH | simulated | all | 0 / 31 / 1769 of 1800 | 1948–1968 / 1989–2027 / 1990–2036 | 1989–2031 | 2000–2033 | 45.4–45.7 | 1339392–1342464 | 1786–2167 | 63.3 |
| `explainer_16x9` | LH | simulated | all | 0 / 66–67 / 1733–1734 of 1800 | 707–736 / 1381–1390 / 1497–1539 | 1493–1536 | 1500–1533 | 45.4–45.9 | 324608–337920 | 1953–2436 | 56.3 |
| `reel_9x16` | LH | simulated | all | 0 / 36–39 / 1761–1764 of 1800 | 1315–1332 / 2531–3254 / 3219–3268 | 3217–3265 | 3200–3267 | 45.4–45.5 | 1048576–1121280 | 2202–2334 | 156.9 |
| `feed_4x5` | LH | simulated | all | 0 / 60–62 / 1738–1740 of 1800 | 1010–1018 / 1979–2023 / 2070–2245 | 2066–2243 | 2067–2233 | 45.4–45.5 | 535552–537600 | 1983–2262 | 114.5 |
| `talk_recut` | LH | simulated | all | 0 / 361 / 6839 of 7200 | 673–675 / 719–722 / 772–854 | 768–854 | 767–833 | 45.8–46.7 | 7168–8192 | 1914–2017 | 42.2 |
| `typical_1080p` | LH | device (1 run) | all | 0 / 69 / 1731 of 1800 | 980 / 1340 / 1648 | 1645 | 1633 | 45.4 | 282624 | 928 | 56.3 |
| `blend_heavy_1080p` | LH | device (1 run) | all | 0 / 31 / 1769 of 1800 | 1965 / 1992 / 1994 | 1993 | 2000 | 45.4 | 1335808 | 1150 | 63.3 |
| `explainer_16x9` | LH | device (1 run) | all | 0 / 66 / 1734 of 1800 | 734 / 1389 / 1521 | 1520 | 1533 | 45.4 | 324608 | 1285 | 56.3 |
| `reel_9x16` | LH | device (1 run) | all | 0 / 40 / 1760 of 1800 | 1309 / 2547 / 3194 | 3190 | 3167 | 45.3 | 1012224 | 1293 | 156.9 |
| `feed_4x5` | LH | device (1 run) | all | 0 / 59 / 1741 of 1800 | 1020 / 1992 / 2483 | 2482 | 2467 | 45.3 | 557056 | 1174 | 114.5 |
| `talk_recut` | LH | device (1 run) | all | 0 / 360 / 6840 of 7200 | 674 / 725 / 784 | 783 | 767 | 45.5 | 6656 | 1011 | 42.2 |

How to read the table:
- **Every workload fails G1 and G14 today, on both lanes.** No due frame was
  presented on time. Frames arrive about every 0.7–2 s, which matches E3's
  T4 (1.8 / 0.5 fps).
- **LL ≈ LH.** Playback is bound by decode and transfer, not by the GPU
  (E0).
- **The real device reproduces the simulated driver.** The present
  distribution, held age and stall match. `device_latency_ms` is 20.9–21.3
  (one PipeWire quantum), and the stall floor is the same.
- **The clock-stall floor is about 45 ms, and it is structural.** A
  1,024-frame callback is 21.3 ms. The position is a 30 fps frame index, so it
  can stay unchanged for up to two callbacks (42.7 ms), plus the 5 ms sampling
  step. G16's 100 ms bound sits above this floor.
- **`underrun_frames` (pre-R22) is large because the worker renders
  synchronously in `tick`.** A render slower than the 1 s live fill target drains the ring.
  Today's clock advances through underruns (T7), so these runs stay valid.
  `talk_recut`'s renders take about 0.7 s, under the fill target, so it
  underruns least.
- **Peak RSS is in-process.** VmHWM is reset per run through
  `/proc/self/clear_refs`, but memory kept from earlier runs in the same test
  process stays in the reading. P-rss (E9.5) is the authoritative memory
  figure.

### E9.4 Q-3 controls

Each control fires at 20 s on `typical_1080p` (slowdown: from the start).
Every control **fails its metric** on both lanes, and `pf1_play_baseline`
asserts this.

| Control | Lane | Metric | Fails | Late + dropped | Present p95 ms | Held max ms | Clock stall max ms | underrun_frames (pre-R22 counter, overstated) |
|---|---|---|---|---|---|---|---|---|
| slowdown | LL | G1 | true | 1800 | 1699.8 | 1725.6 | 47.0 | 418816 |
| freeze | LL | G14 | true | 1800 | 1578.6 | 1870.7 | 45.4 | 385024 |
| clock freeze | LL | G16 | true | 1800 | 1772.0 | 1801.7 | 1050.0 | 476160 |
| stall | LL | underrun_frames | true | 1800 | 1814.5 | 3734.2 | 46.2 | 762880 |
| slowdown | LH | G1 | true | 1800 | 1638.8 | 1702.8 | 45.6 | 530432 |
| freeze | LH | G14 | true | 1800 | 1575.0 | 2636.4 | 45.5 | 345088 |
| clock freeze | LH | G16 | true | 1800 | 1446.5 | 1666.0 | 1050.0 | 350208 |
| stall | LH | underrun_frames | true | 1800 | 1655.5 | 3196.0 | 45.8 | 525312 |

**Only the clock freeze discriminates today.** The uncontrolled runs already
fail G1 and G14 and already underrun. The controls still show the expected
direction:
- slowdown: 1,800 of 1,800 frames late or dropped;
- freeze: held age 1.87 s / 2.64 s, against 1.55–1.73 s uncontrolled;
- clock freeze: stall 1,050 ms, against about 46 ms;
- stall: held age 3.7 s / 3.2 s, and more underrun frames (pre-R22 counter).

The stall control's "fails" was decided on the pre-R22 counter, so it was
true for every run. It is not evidence of discrimination.

CI covers discrimination with deterministic witnesses instead:
- `pf1_metrics_pass_a_clean_trace_and_every_control_fails_its_metric` shows
  that the slowdown, freeze and clock-freeze metrics each flip a *passing*
  synthetic trace (G1, G14, G16).
- `a_two_second_fill_stall_underruns_where_the_clean_schedule_does_not`
  covers the fourth control. It runs matched producer schedules through the
  simulated consumer and `render_output`: the clean schedule records 0
  underrun frames and the 2 s fill stall a positive count (40,000–50,000
  expected).

This replaces the earlier claim that the first test alone proved every
control; it did not cover the fill stall (review A F6). S2 should rerun the
controls once the uncontrolled run passes G1 and G14.

### E9.5 P-rss (fresh process per workload)

Resident MiB / thread count at each point:
- **Before:** before the engine is created.
- **Constructed:** 500 ms after construction, with no document.
- **First render:** the first paused frame.
- **Settled idle:** 6 s later. This is G15's baseline.
- **Playing:** after 10 s of simulated-driver playback, with the peak since a
  VmHWM reset.

LL is the pinned lane; LH is recorded as well.

| Workload | Lane | Before | Constructed | First render | Settled idle (G15) | Playing 10 s | Playing peak MiB |
|---|---|---|---|---|---|---|---|
| `typical_1080p` | LL | 33.4/2 | 169.9/48 | 584.7/140 | 584.8/140 | 833.4/141 | 946.3 |
| `blend_heavy_1080p` | LL | 33.5/2 | 170.0/48 | 677.2/186 | 677.2/186 | 995.1/187 | 995.1 |
| `explainer_16x9` | LL | 33.5/2 | 169.9/48 | 546.4/140 | 546.4/140 | 964.2/187 | 973.5 |
| `reel_9x16` | LL | 33.5/2 | 170.0/48 | 492.4/94 | 492.5/94 | 1127.0/187 | 1138.4 |
| `feed_4x5` | LL | 33.9/2 | 170.0/48 | 402.7/94 | 402.7/94 | 898.7/187 | 964.8 |
| `talk_recut` | LL | 33.6/2 | 170.1/48 | 381.8/94 | 381.8/94 | 649.0/95 | 651.1 |
| `title_only` | LL | 33.5/2 | 170.2/48 | 236.0/48 | 236.0/48 | 254.0/49 | 254.0 |
| `typical_1080p` | LH | 33.4/2 | 163.5/11 | 624.9/103 | 624.9/103 | 862.5/104 | 862.5 |
| `blend_heavy_1080p` | LH | 33.5/2 | 164.0/11 | 711.0/149 | 711.0/149 | 1026.7/150 | 1026.7 |
| `explainer_16x9` | LH | 33.6/2 | 163.7/11 | 587.7/103 | 587.7/103 | 958.6/150 | 1004.2 |
| `reel_9x16` | LH | 33.6/2 | 163.9/11 | 479.0/57 | 479.0/57 | 1031.2/150 | 1086.4 |
| `feed_4x5` | LH | 33.5/2 | 164.1/11 | 450.8/57 | 450.8/57 | 937.2/150 | 980.8 |
| `talk_recut` | LH | 33.5/2 | 163.7/11 | 450.8/57 | 450.8/57 | 716.2/58 | 720.1 |
| `title_only` | LH | 33.5/2 | 164.0/11 | 305.3/11 | 305.3/11 | 326.7/12 | 326.7 |

Settled idle equals first render on every workload. Nothing is released
after 6 s idle: decoders, caches and threads stay resident.

### E9.6 P-seek

Rerun after the review fixes (R24), at `82fe858`. The original E9 P-seek
figures are superseded: that run's first "forward" steps were backward
seeks, and its release was measured only after the drag had settled
(review A F4 and should-fix).

Paused, three seeded runs per workload (`0x5EED0000 + run`). Each run does:
- 200 random seeks;
- 200 forward steps (+1…+12). They start with a seek to a seeded random
  start frame, so that every step really moves forward from where the
  transport is;
- 200 backward steps (−1…−12);
- a 5 s `request_frame` drag at 30 Hz, forward and monotone, in 1–4 frame
  steps;
- then, at the 5 s boundary and **without waiting for the drag to settle**,
  the release `seek` to the frame after the last drag target. The drag never
  requests that frame, so its arrival is attributable to the release.

Each operation mirrors the app's `seek_to` (`seek` + `request_frame`). The
latency runs from the call to receipt of the matching frame. A drag call is
answered by the first drag frame at or past its target, up to the last drag
target.

How to read the columns:
- Latency columns are the worst run's value (the largest over the three
  runs). Distinct fps is the smallest. Counts and release latency are given
  as ranges.
- `+1` is the subset of forward steps that were +1; there are only 8–17
  per run.
- **Backward is combined.** `FrameRenderer::decode_video_frame` already
  checks the source frame cache before decoding, so backward steps mix cache
  hits and refills. **L-4a and L-4b are not measured separately** (review A
  F5).
- Drag p95 counts an unanswered call as infinite. Answered p95 is taken over
  answered calls only. Unanswered means no drag frame at or past the call's
  target arrived before the release frame. Since R25 (D4) the harness takes
  answers only from arrivals up to the release receipt; the later 500 ms
  feed only the overwrite count. The rerun's tables are unaffected: no
  frame arrived after a release frame.
- Pending is the number of drag calls still unanswered when the release was
  issued. It counts calls, not renders: the engine may coalesce them.
- Stale / over:
  - *stale* counts drag frames delivered after the release call;
  - *over* counts frames delivered within 500 ms after the release frame,
    which would replace it on screen.

| Workload | Lane | Random p95 / max | Forward p95 | +1 p95 (L-3) | Backward p95, combined | Drag p95 / answered p95 | Drag unanswered | Drag distinct fps (L-5) | Pending at release | Release shown / ms (L-6) | Stale / over release | Timeouts |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `seek_gop60` | LL | 117.5 / 128.8 | 118.7 | 118.1 | 116.8 | 196.8 / 194.3 | 0–2 | 10.2 | 2–4 | all / 86.1–170.6 | 0–1 / 0 | 0 |
| `talk_recut` | LL | 162.0 / 223.4 | 162.4 | 181.0 | 147.5 | 236.7 / 232.4 | 0–2 | 8.8 | 3–6 | all / 129.1–173.7 | 1 / 0 | 0 |
| `explainer_16x9` | LL | 211.6 / 273.1 | 206.9 | 224.5 | 206.2 | 359.8 / 359.8 | 0–1 | 6.8 | 3–5 | all / 178.9–296.6 | 1 / 0 | 0 |
| `seek_gop60` | LH | 119.8 / 127.0 | 117.7 | 125.1 | 117.1 | 197.9 / 197.7 | 1–2 | 10.0 | 4 | all / 94.1–183.1 | 1 / 0 | 0 |
| `talk_recut` | LH | 162.8 / 225.7 | 163.0 | 189.8 | 152.9 | 231.5 / 231.5 | 0 | 9.0 | 2–4 | all / 189.6–238.7 | 1 / 0 | 0 |
| `explainer_16x9` | LH | 208.0 / 270.1 | 204.0 | 222.3 | 203.5 | 387.9 / 366.2 | 2 | 6.6 | 4–6 | all / 109.4–243.6 | 1 / 0 | 0 |

Mapping to G8 (all ms):

| Metric | Measured (LL / LH) | Gate |
|---|---|---|
| L-1 | 117.5 / 119.8 | ≤ 40 |
| L-2 (talk) | 162.0 / 162.8 | ≤ 110 |
| L-3 | 118.1 / 125.1 at GOP 60 (8–17 samples per run) | ≤ 20 |
| L-4a / L-4b | not measured separately; combined backward p95 is 116.8 / 117.1 at GOP 60 | — |
| L-5 | 10.2 / 10.0 at GOP 60; 8.8 / 9.0 at GOP 250 | ≥ 10 / 7 |
| L-6 | shown in every run; 86–297 ms after an unsettled release | — |

What changed from the first run:
- **Release latency rose.** It was about 30 ms against an already-rendered
  target. Now it is 86–297 ms, because the release is issued while 2–6
  drag calls are still unanswered and queues behind their renders (however
  many the engine coalesces them into).
- **A pending drag render still lands after the release call** (stale = 1)
  in 17 of 18 runs. None landed after the release frame (over = 0), so
  today's release is never overwritten within 500 ms. Today's API has no
  release stamp, so attribution relies on the release frame being unique.
- **`talk_recut`'s step latencies fell.** Forward p95 went from 196.4 /
  191.4 to 162.4 / 163.0, and backward from 192.1 / 192.9 to 147.5 / 152.9.
  The cause is not isolated. The forward-init seek draws one more value from
  the seeded generator, so every later position in the run moved.
- **The talk L-5 LL worst run is now 8.8 fps** (it was 9.0), still above
  GOP 250's 7.
- **Random seeks (L-1, L-2) and the other workloads' forward and backward
  steps are within about 5% of the first run.** Two exceptions:
  - `explainer_16x9`'s +1 p95 is about 9% higher (224.5 / 222.3 against
    205.5 / 203.6), but it rests on only 8–17 samples per run;
  - its LH drag p95 is 7.5% higher (387.9 against 360.8), because
    unanswered calls now count as infinite.

### E9.7 Harness notes (proposed amendments are in the S0 report)

- **Windows `process_memory()`.** It runs `Get-Process` (WorkingSet64,
  PeakWorkingSet64, Threads.Count) through `powershell.exe` rather than
  calling `GetProcessMemoryInfo` and Toolhelp: the workspace forbids
  `unsafe_code`. The counters are the same ones. The peak cannot be reset,
  so Windows peaks are process-lifetime values, marked `(lifetime)`.
  Compiled for `x86_64-pc-windows-msvc`; it has not yet run on Windows.
  CI-W runs `pf1_process_memory_reads_this_process`. Since the review fixes
  the probe has a 30 s deadline. It checks the exit status and requires
  exactly three integers. On a timeout, or if waiting fails, the child is
  killed and reaped within a bounded 5 s, and the error reports what the
  kill and reap did (R25/D3). A failed probe is
  reported as `unavailable(...)`, never as 0 (review B F2 and S2).
- **A/V offset.** Today has no ack, so the offset is measured at receipt:
  |`position()` − frame stamp|.
- **Underruns (R22).** `render_output` counts the pops that failed, at the
  existing `unwrap_or(0.0)` sites. The samples and the clock are unchanged.
  - Since R25 (D2) it records them *before* the clock's release advance, so
    an observer that sees the advanced clock sees that callback's underruns.
  - The harness snapshots the counter right after the clock sample that
    reached the duration. The snapshot holds every callback up to the
    endpoint. It may also hold callbacks after it, because the sampling
    thread can be suspended between reading the clock and the counter.
    That error only ever adds underruns to the measured window, never
    removes them.
  - What follows (the drain to the engine's own pause) is reported apart,
    as `drain_underrun_frames`.
  - Witnesses: `failed_pops_are_exact_under_a_concurrent_refill` (the count
    under a concurrent refill) and
    `an_observed_clock_never_runs_ahead_of_its_underruns` (the ordering; it
    fails with the pre-R25 order).
- **Diagnostics are per engine (R23).** `AudioDiagnostics` (underrun frames
  and device latency) is owned by each engine's `AudioRuntime` and shared
  with the harness by `Arc`. The process-wide statics are gone, and the
  reads are `cfg(test)`. The counters still record in production builds;
  that is two relaxed atomic operations per callback.
- **Missed callback deadlines (review A F2, R25/D1).** The paced driver
  runs one callback per deadline and never bursts to catch up.
  - Since R25 it stamps each callback where it executes, under the stream
    lock: immediately before the consume (recorded before the clock
    advances) and immediately after it.
  - Since R27 both stamps are measured against the deadline the callback
    serves, not against each other. Delays split across the wake, the start
    and the consume therefore add up.
  - A callback that completes a whole period or more past its deadline
    misses that many deadlines. After any miss the schedule re-anchors on
    the completion, so nothing is caught up.
  - A P-play run with any missed deadline is invalid, wherever it falls
    (drain included). `missed_callbacks` reports the run's total.
  - Witnesses:
    - `a_suspension_with_a_refill_is_a_missed_deadline_that_invalidates_the_run`:
      a 32 ms suspension, with a refill during it that hides the starvation
      from the underrun count, still counts a miss and invalidates the run.
    - `a_split_suspension_with_a_refill_is_a_missed_deadline_that_invalidates_the_run`
      (R27): 16 ms before the start plus 16 ms before the finish, again with
      a refill, counts one miss, re-anchors with no immediate catch-up
      callback, and invalidates the run.
- **P-rss guards.** Parent and child require a release build. The parent
  checks the child's exit status. The child requires at least 5 s of played
  timeline and no engine error before it labels the snapshot "playing".

### E9.8 Raw result lines

These are the first run's lines. In the `PF1 play` and `PF1 control` lines,
`underrun_frames` is the pre-R22 counter, which overstates. The `PF1 seek`
lines are superseded by E9.9's rerun.

```
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=false workload=typical_1080p run=0 dims=(1280, 720) mean_ms=509.86 fps=2.0 p95_ms=1619.55 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=false workload=typical_1080p run=1 dims=(1280, 720) mean_ms=510.75 fps=2.0 p95_ms=1616.60 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=false workload=typical_1080p run=2 dims=(1280, 720) mean_ms=508.83 fps=2.0 p95_ms=1615.36 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) workload=typical_1080p baseline_ms=497.86 delta=+2.4%
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=typical_1080p run=0 valid=true elapsed_s=60.02 due=1800 on_time=0 late=56 dropped=1744 present_p50_ms=1103.2 present_p95_ms=1574.8 present_max_ms=1708.2 held_max_ms=1707.7 av_offset_max_ms=1700.0 clock_stall_max_ms=45.5 underrun_frames=525312 peak_rss_mib=863.1 ledger_peak_mib=56.3
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=typical_1080p run=1 valid=true elapsed_s=60.02 due=1800 on_time=0 late=77 dropped=1723 present_p50_ms=679.1 present_p95_ms=1317.2 present_max_ms=1552.6 held_max_ms=1550.0 av_offset_max_ms=1566.7 clock_stall_max_ms=45.8 underrun_frames=260096 peak_rss_mib=1236.6 ledger_peak_mib=56.3
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=typical_1080p run=2 valid=true elapsed_s=60.02 due=1800 on_time=0 late=61 dropped=1739 present_p50_ms=1003.6 present_p95_ms=1578.5 present_max_ms=1685.8 held_max_ms=1683.1 av_offset_max_ms=1700.0 clock_stall_max_ms=46.0 underrun_frames=402432 peak_rss_mib=1056.8 ledger_peak_mib=56.3
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=blend_heavy_1080p run=0 valid=true elapsed_s=60.02 due=1800 on_time=0 late=30 dropped=1770 present_p50_ms=2005.1 present_p95_ms=2044.9 present_max_ms=2050.1 held_max_ms=2047.6 av_offset_max_ms=2066.7 clock_stall_max_ms=45.4 underrun_frames=1399808 peak_rss_mib=1454.4 ledger_peak_mib=63.3
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=blend_heavy_1080p run=1 valid=true elapsed_s=60.02 due=1800 on_time=0 late=30 dropped=1770 present_p50_ms=2007.9 present_p95_ms=2046.0 present_max_ms=2061.6 held_max_ms=2058.3 av_offset_max_ms=2066.7 clock_stall_max_ms=46.1 underrun_frames=1405952 peak_rss_mib=1787.8 ledger_peak_mib=63.3
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=blend_heavy_1080p run=2 valid=true elapsed_s=60.02 due=1800 on_time=0 late=30 dropped=1770 present_p50_ms=1997.1 present_p95_ms=2053.8 present_max_ms=2079.5 held_max_ms=2078.9 av_offset_max_ms=2066.7 clock_stall_max_ms=45.4 underrun_frames=1398784 peak_rss_mib=2050.3 ledger_peak_mib=63.3
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=explainer_16x9 run=0 valid=true elapsed_s=60.02 due=1800 on_time=0 late=67 dropped=1733 present_p50_ms=725.7 present_p95_ms=1372.7 present_max_ms=1497.1 held_max_ms=1492.6 av_offset_max_ms=1500.0 clock_stall_max_ms=46.4 underrun_frames=330752 peak_rss_mib=2297.2 ledger_peak_mib=56.3
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=explainer_16x9 run=1 valid=true elapsed_s=60.02 due=1800 on_time=0 late=67 dropped=1733 present_p50_ms=698.4 present_p95_ms=1355.5 present_max_ms=1502.1 held_max_ms=1499.0 av_offset_max_ms=1466.7 clock_stall_max_ms=46.0 underrun_frames=318464 peak_rss_mib=1729.0 ledger_peak_mib=56.3
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=explainer_16x9 run=2 valid=true elapsed_s=60.02 due=1800 on_time=0 late=67 dropped=1733 present_p50_ms=711.0 present_p95_ms=1355.2 present_max_ms=1467.3 held_max_ms=1466.0 av_offset_max_ms=1466.7 clock_stall_max_ms=45.3 underrun_frames=317440 peak_rss_mib=1812.8 ledger_peak_mib=56.3
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=reel_9x16 run=0 valid=true elapsed_s=60.02 due=1800 on_time=0 late=37 dropped=1763 present_p50_ms=1336.4 present_p95_ms=3253.8 present_max_ms=3302.5 held_max_ms=3299.8 av_offset_max_ms=3266.7 clock_stall_max_ms=45.4 underrun_frames=1110016 peak_rss_mib=1983.5 ledger_peak_mib=156.9
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=reel_9x16 run=1 valid=true elapsed_s=60.02 due=1800 on_time=0 late=38 dropped=1762 present_p50_ms=1328.0 present_p95_ms=2593.4 present_max_ms=3271.7 held_max_ms=3267.3 av_offset_max_ms=3266.7 clock_stall_max_ms=47.2 underrun_frames=1038336 peak_rss_mib=2371.6 ledger_peak_mib=156.9
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=reel_9x16 run=2 valid=true elapsed_s=60.02 due=1800 on_time=0 late=41 dropped=1759 present_p50_ms=1317.1 present_p95_ms=2033.2 present_max_ms=2569.5 held_max_ms=2565.4 av_offset_max_ms=2533.3 clock_stall_max_ms=48.4 underrun_frames=933888 peak_rss_mib=2456.4 ledger_peak_mib=156.9
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=feed_4x5 run=0 valid=true elapsed_s=60.02 due=1800 on_time=0 late=61 dropped=1739 present_p50_ms=1026.8 present_p95_ms=1996.7 present_max_ms=2187.8 held_max_ms=2185.1 av_offset_max_ms=2166.7 clock_stall_max_ms=46.3 underrun_frames=558080 peak_rss_mib=2524.9 ledger_peak_mib=114.5
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=feed_4x5 run=1 valid=true elapsed_s=60.02 due=1800 on_time=0 late=60 dropped=1740 present_p50_ms=1019.5 present_p95_ms=2013.5 present_max_ms=2100.2 held_max_ms=2098.4 av_offset_max_ms=2100.0 clock_stall_max_ms=45.8 underrun_frames=551936 peak_rss_mib=2343.6 ledger_peak_mib=114.5
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=feed_4x5 run=2 valid=true elapsed_s=60.02 due=1800 on_time=0 late=60 dropped=1740 present_p50_ms=1020.7 present_p95_ms=2042.8 present_max_ms=2079.3 held_max_ms=2078.3 av_offset_max_ms=2100.0 clock_stall_max_ms=47.8 underrun_frames=534528 peak_rss_mib=2288.7 ledger_peak_mib=114.5
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=talk_recut run=0 valid=true elapsed_s=240.00 due=7200 on_time=0 late=360 dropped=6840 present_p50_ms=675.4 present_p95_ms=719.9 present_max_ms=795.9 held_max_ms=795.2 av_offset_max_ms=800.0 clock_stall_max_ms=46.0 underrun_frames=7168 peak_rss_mib=2239.9 ledger_peak_mib=42.2
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=talk_recut run=1 valid=true elapsed_s=240.01 due=7200 on_time=0 late=360 dropped=6840 present_p50_ms=675.9 present_p95_ms=722.6 present_max_ms=844.4 held_max_ms=844.0 av_offset_max_ms=833.3 clock_stall_max_ms=45.7 underrun_frames=7168 peak_rss_mib=2058.0 ledger_peak_mib=42.2
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=talk_recut run=2 valid=true elapsed_s=240.01 due=7200 on_time=0 late=361 dropped=6839 present_p50_ms=676.0 present_p95_ms=724.9 present_max_ms=773.9 held_max_ms=772.7 av_offset_max_ms=766.7 clock_stall_max_ms=46.2 underrun_frames=8192 peak_rss_mib=2167.3 ledger_peak_mib=42.2
PF1 control=Slowdown lane=LL workload=typical_1080p metric=G1 fails=true valid=true elapsed_s=60.02 due=1800 on_time=0 late=59 dropped=1741 present_p50_ms=987.3 present_p95_ms=1699.8 present_max_ms=1726.1 held_max_ms=1725.6 av_offset_max_ms=1733.3 clock_stall_max_ms=47.0 underrun_frames=418816 peak_rss_mib=2246.0 ledger_peak_mib=56.3
PF1 control=Freeze lane=LL workload=typical_1080p metric=G14 fails=true valid=true elapsed_s=60.02 due=1800 on_time=0 late=67 dropped=1733 present_p50_ms=899.6 present_p95_ms=1578.6 present_max_ms=1874.7 held_max_ms=1870.7 av_offset_max_ms=1666.7 clock_stall_max_ms=45.4 underrun_frames=385024 peak_rss_mib=2238.1 ledger_peak_mib=56.3
PF1 control=ClockFreeze lane=LL workload=typical_1080p metric=G16 fails=true valid=true elapsed_s=61.02 due=1800 on_time=0 late=62 dropped=1738 present_p50_ms=994.8 present_p95_ms=1772.0 present_max_ms=1805.1 held_max_ms=1801.7 av_offset_max_ms=1800.0 clock_stall_max_ms=1050.0 underrun_frames=476160 peak_rss_mib=2360.5 ledger_peak_mib=56.3
PF1 control=Stall lane=LL workload=typical_1080p metric=underrun_frames fails=true valid=true elapsed_s=60.02 due=1800 on_time=0 late=54 dropped=1746 present_p50_ms=1128.8 present_p95_ms=1814.5 present_max_ms=3737.0 held_max_ms=3734.2 av_offset_max_ms=1800.0 clock_stall_max_ms=46.2 underrun_frames=762880 peak_rss_mib=2305.0 ledger_peak_mib=56.3
PF1 seek lane=LL workload=seek_gop60 run=0 random_p95_ms=114.8 random_max_ms=129.0 forward_p95_ms=112.6 plus1_p95_ms=117.9 backward_p95_ms=114.7 drag_p95_ms=185.6 drag_distinct_fps=10.4 release_shown=true release_ms=28.8 timeouts=0
PF1 seek lane=LL workload=seek_gop60 run=1 random_p95_ms=119.5 random_max_ms=145.4 forward_p95_ms=117.4 plus1_p95_ms=120.2 backward_p95_ms=115.7 drag_p95_ms=188.8 drag_distinct_fps=10.2 release_shown=true release_ms=30.3 timeouts=0
PF1 seek lane=LL workload=seek_gop60 run=2 random_p95_ms=116.0 random_max_ms=120.0 forward_p95_ms=116.1 plus1_p95_ms=106.8 backward_p95_ms=114.5 drag_p95_ms=188.8 drag_distinct_fps=10.4 release_shown=true release_ms=30.0 timeouts=0
PF1 seek lane=LL workload=talk_recut run=0 random_p95_ms=144.1 random_max_ms=202.0 forward_p95_ms=189.9 plus1_p95_ms=188.0 backward_p95_ms=191.0 drag_p95_ms=224.3 drag_distinct_fps=9.2 release_shown=true release_ms=29.2 timeouts=0
PF1 seek lane=LL workload=talk_recut run=1 random_p95_ms=147.5 random_max_ms=207.4 forward_p95_ms=193.5 plus1_p95_ms=208.4 backward_p95_ms=192.1 drag_p95_ms=219.1 drag_distinct_fps=9.2 release_shown=true release_ms=29.7 timeouts=0
PF1 seek lane=LL workload=talk_recut run=2 random_p95_ms=163.8 random_max_ms=221.7 forward_p95_ms=196.4 plus1_p95_ms=179.2 backward_p95_ms=188.8 drag_p95_ms=227.0 drag_distinct_fps=9.0 release_shown=true release_ms=29.7 timeouts=0
PF1 seek lane=LL workload=explainer_16x9 run=0 random_p95_ms=200.0 random_max_ms=266.5 forward_p95_ms=205.0 plus1_p95_ms=205.5 backward_p95_ms=204.4 drag_p95_ms=365.4 drag_distinct_fps=6.6 release_shown=true release_ms=35.2 timeouts=0
PF1 seek lane=LL workload=explainer_16x9 run=1 random_p95_ms=205.7 random_max_ms=226.2 forward_p95_ms=206.0 plus1_p95_ms=201.2 backward_p95_ms=206.0 drag_p95_ms=366.5 drag_distinct_fps=7.0 release_shown=true release_ms=29.8 timeouts=0
PF1 seek lane=LL workload=explainer_16x9 run=2 random_p95_ms=205.4 random_max_ms=271.8 forward_p95_ms=200.1 plus1_p95_ms=200.2 backward_p95_ms=206.0 drag_p95_ms=356.2 drag_distinct_fps=6.6 release_shown=true release_ms=36.5 timeouts=0
R28 adapter=NVIDIA GeForce RTX 3090 resident=false workload=typical_1080p run=0 dims=(1280, 720) mean_ms=507.26 fps=2.0 p95_ms=1619.27 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 resident=false workload=typical_1080p run=1 dims=(1280, 720) mean_ms=508.49 fps=2.0 p95_ms=1610.05 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 resident=false workload=typical_1080p run=2 dims=(1280, 720) mean_ms=506.56 fps=2.0 p95_ms=1616.88 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 workload=typical_1080p baseline_ms=494.39 delta=+2.6%
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=typical_1080p run=0 valid=true elapsed_s=60.02 due=1800 on_time=0 late=61 dropped=1739 present_p50_ms=1010.4 present_p95_ms=1568.2 present_max_ms=1716.0 held_max_ms=1711.5 av_offset_max_ms=1700.0 clock_stall_max_ms=45.4 underrun_frames=492544 peak_rss_mib=1036.9 ledger_peak_mib=56.3
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=typical_1080p run=1 valid=true elapsed_s=60.02 due=1800 on_time=0 late=65 dropped=1735 present_p50_ms=986.2 present_p95_ms=1453.3 present_max_ms=1617.5 held_max_ms=1614.4 av_offset_max_ms=1633.3 clock_stall_max_ms=45.5 underrun_frames=367616 peak_rss_mib=1357.2 ledger_peak_mib=56.3
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=typical_1080p run=2 valid=true elapsed_s=60.02 due=1800 on_time=0 late=61 dropped=1739 present_p50_ms=1006.8 present_p95_ms=1548.0 present_max_ms=1726.1 held_max_ms=1723.0 av_offset_max_ms=1700.0 clock_stall_max_ms=45.5 underrun_frames=425984 peak_rss_mib=1418.5 ledger_peak_mib=56.3
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=blend_heavy_1080p run=0 valid=true elapsed_s=60.02 due=1800 on_time=0 late=31 dropped=1769 present_p50_ms=1967.7 present_p95_ms=2000.8 present_max_ms=2001.4 held_max_ms=1999.6 av_offset_max_ms=2000.0 clock_stall_max_ms=45.7 underrun_frames=1342464 peak_rss_mib=1786.2 ledger_peak_mib=63.3
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=blend_heavy_1080p run=1 valid=true elapsed_s=60.02 due=1800 on_time=0 late=31 dropped=1769 present_p50_ms=1948.4 present_p95_ms=2027.0 present_max_ms=2035.6 held_max_ms=2031.1 av_offset_max_ms=2033.3 clock_stall_max_ms=45.4 underrun_frames=1339392 peak_rss_mib=2127.2 ledger_peak_mib=63.3
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=blend_heavy_1080p run=2 valid=true elapsed_s=60.02 due=1800 on_time=0 late=31 dropped=1769 present_p50_ms=1960.4 present_p95_ms=1989.3 present_max_ms=1990.3 held_max_ms=1988.7 av_offset_max_ms=2000.0 clock_stall_max_ms=45.5 underrun_frames=1342464 peak_rss_mib=2166.7 ledger_peak_mib=63.3
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=explainer_16x9 run=0 valid=true elapsed_s=60.02 due=1800 on_time=0 late=67 dropped=1733 present_p50_ms=713.4 present_p95_ms=1381.6 present_max_ms=1539.4 held_max_ms=1536.4 av_offset_max_ms=1533.3 clock_stall_max_ms=45.5 underrun_frames=324608 peak_rss_mib=2435.9 ledger_peak_mib=56.3
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=explainer_16x9 run=1 valid=true elapsed_s=60.02 due=1800 on_time=0 late=66 dropped=1734 present_p50_ms=736.4 present_p95_ms=1389.9 present_max_ms=1497.0 held_max_ms=1493.2 av_offset_max_ms=1500.0 clock_stall_max_ms=45.9 underrun_frames=337920 peak_rss_mib=1952.8 ledger_peak_mib=56.3
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=explainer_16x9 run=2 valid=true elapsed_s=60.02 due=1800 on_time=0 late=67 dropped=1733 present_p50_ms=706.8 present_p95_ms=1381.4 present_max_ms=1524.6 held_max_ms=1522.5 av_offset_max_ms=1533.3 clock_stall_max_ms=45.4 underrun_frames=332800 peak_rss_mib=2174.3 ledger_peak_mib=56.3
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=reel_9x16 run=0 valid=true elapsed_s=60.02 due=1800 on_time=0 late=39 dropped=1761 present_p50_ms=1325.3 present_p95_ms=2540.0 present_max_ms=3245.0 held_max_ms=3240.4 av_offset_max_ms=3266.7 clock_stall_max_ms=45.4 underrun_frames=1048576 peak_rss_mib=2309.5 ledger_peak_mib=156.9
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=reel_9x16 run=1 valid=true elapsed_s=60.02 due=1800 on_time=0 late=36 dropped=1764 present_p50_ms=1332.3 present_p95_ms=3254.1 present_max_ms=3268.4 held_max_ms=3265.1 av_offset_max_ms=3266.7 clock_stall_max_ms=45.4 underrun_frames=1121280 peak_rss_mib=2201.5 ledger_peak_mib=156.9
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=reel_9x16 run=2 valid=true elapsed_s=60.02 due=1800 on_time=0 late=37 dropped=1763 present_p50_ms=1315.0 present_p95_ms=2530.6 present_max_ms=3219.4 held_max_ms=3217.1 av_offset_max_ms=3200.0 clock_stall_max_ms=45.5 underrun_frames=1064960 peak_rss_mib=2334.5 ledger_peak_mib=156.9
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=feed_4x5 run=0 valid=true elapsed_s=60.02 due=1800 on_time=0 late=62 dropped=1738 present_p50_ms=1013.1 present_p95_ms=1979.2 present_max_ms=2245.3 held_max_ms=2242.8 av_offset_max_ms=2233.3 clock_stall_max_ms=45.5 underrun_frames=535552 peak_rss_mib=2261.8 ledger_peak_mib=114.5
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=feed_4x5 run=1 valid=true elapsed_s=60.02 due=1800 on_time=0 late=61 dropped=1739 present_p50_ms=1017.8 present_p95_ms=1980.7 present_max_ms=2070.2 held_max_ms=2065.6 av_offset_max_ms=2066.7 clock_stall_max_ms=45.4 underrun_frames=537600 peak_rss_mib=1983.1 ledger_peak_mib=114.5
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=feed_4x5 run=2 valid=true elapsed_s=60.02 due=1800 on_time=0 late=60 dropped=1740 present_p50_ms=1009.8 present_p95_ms=2022.7 present_max_ms=2086.4 held_max_ms=2082.1 av_offset_max_ms=2100.0 clock_stall_max_ms=45.4 underrun_frames=536576 peak_rss_mib=2187.8 ledger_peak_mib=114.5
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=talk_recut run=0 valid=true elapsed_s=240.01 due=7200 on_time=0 late=361 dropped=6839 present_p50_ms=673.7 present_p95_ms=722.3 present_max_ms=771.6 held_max_ms=770.7 av_offset_max_ms=766.7 clock_stall_max_ms=46.1 underrun_frames=8192 peak_rss_mib=1914.4 ledger_peak_mib=42.2
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=talk_recut run=1 valid=true elapsed_s=240.01 due=7200 on_time=0 late=361 dropped=6839 present_p50_ms=673.0 present_p95_ms=718.9 present_max_ms=771.9 held_max_ms=768.3 av_offset_max_ms=766.7 clock_stall_max_ms=45.8 underrun_frames=7168 peak_rss_mib=1914.4 ledger_peak_mib=42.2
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=talk_recut run=2 valid=true elapsed_s=240.01 due=7200 on_time=0 late=361 dropped=6839 present_p50_ms=674.8 present_p95_ms=719.0 present_max_ms=854.3 held_max_ms=854.2 av_offset_max_ms=833.3 clock_stall_max_ms=46.7 underrun_frames=8192 peak_rss_mib=2016.8 ledger_peak_mib=42.2
PF1 control=Slowdown lane=LH workload=typical_1080p metric=G1 fails=true valid=true elapsed_s=60.02 due=1800 on_time=0 late=59 dropped=1741 present_p50_ms=1066.2 present_p95_ms=1638.8 present_max_ms=1704.5 held_max_ms=1702.8 av_offset_max_ms=1700.0 clock_stall_max_ms=45.6 underrun_frames=530432 peak_rss_mib=2000.2 ledger_peak_mib=56.3
PF1 control=Freeze lane=LH workload=typical_1080p metric=G14 fails=true valid=true elapsed_s=60.02 due=1800 on_time=0 late=64 dropped=1736 present_p50_ms=989.3 present_p95_ms=1575.0 present_max_ms=2637.4 held_max_ms=2636.4 av_offset_max_ms=1633.3 clock_stall_max_ms=45.5 underrun_frames=345088 peak_rss_mib=2082.1 ledger_peak_mib=56.3
PF1 control=ClockFreeze lane=LH workload=typical_1080p metric=G16 fails=true valid=true elapsed_s=61.02 due=1800 on_time=0 late=71 dropped=1729 present_p50_ms=995.6 present_p95_ms=1446.5 present_max_ms=1670.7 held_max_ms=1666.0 av_offset_max_ms=1666.7 clock_stall_max_ms=1050.0 underrun_frames=350208 peak_rss_mib=2072.9 ledger_peak_mib=56.3
PF1 control=Stall lane=LH workload=typical_1080p metric=underrun_frames fails=true valid=true elapsed_s=60.02 due=1800 on_time=0 late=58 dropped=1742 present_p50_ms=1090.7 present_p95_ms=1655.5 present_max_ms=3199.0 held_max_ms=3196.0 av_offset_max_ms=1700.0 clock_stall_max_ms=45.8 underrun_frames=525312 peak_rss_mib=1836.9 ledger_peak_mib=56.3
PF1 seek lane=LH workload=seek_gop60 run=0 random_p95_ms=112.4 random_max_ms=138.8 forward_p95_ms=113.7 plus1_p95_ms=117.7 backward_p95_ms=116.5 drag_p95_ms=185.0 drag_distinct_fps=10.4 release_shown=true release_ms=28.3 timeouts=0
PF1 seek lane=LH workload=seek_gop60 run=1 random_p95_ms=115.6 random_max_ms=125.1 forward_p95_ms=114.7 plus1_p95_ms=120.0 backward_p95_ms=114.6 drag_p95_ms=197.0 drag_distinct_fps=10.2 release_shown=true release_ms=27.5 timeouts=0
PF1 seek lane=LH workload=seek_gop60 run=2 random_p95_ms=114.1 random_max_ms=124.2 forward_p95_ms=113.8 plus1_p95_ms=103.8 backward_p95_ms=113.4 drag_p95_ms=191.1 drag_distinct_fps=10.6 release_shown=true release_ms=28.1 timeouts=0
PF1 seek lane=LH workload=talk_recut run=0 random_p95_ms=144.9 random_max_ms=205.7 forward_p95_ms=191.4 plus1_p95_ms=188.3 backward_p95_ms=192.9 drag_p95_ms=214.4 drag_distinct_fps=9.4 release_shown=true release_ms=28.1 timeouts=0
PF1 seek lane=LH workload=talk_recut run=1 random_p95_ms=145.9 random_max_ms=207.3 forward_p95_ms=190.8 plus1_p95_ms=202.3 backward_p95_ms=189.3 drag_p95_ms=216.7 drag_distinct_fps=9.2 release_shown=true release_ms=28.4 timeouts=0
PF1 seek lane=LH workload=talk_recut run=2 random_p95_ms=162.8 random_max_ms=213.1 forward_p95_ms=189.0 plus1_p95_ms=177.1 backward_p95_ms=191.6 drag_p95_ms=228.7 drag_distinct_fps=9.0 release_shown=true release_ms=27.6 timeouts=0
PF1 seek lane=LH workload=explainer_16x9 run=0 random_p95_ms=205.6 random_max_ms=270.8 forward_p95_ms=202.5 plus1_p95_ms=194.0 backward_p95_ms=203.4 drag_p95_ms=356.6 drag_distinct_fps=6.8 release_shown=true release_ms=32.1 timeouts=0
PF1 seek lane=LH workload=explainer_16x9 run=1 random_p95_ms=205.4 random_max_ms=225.3 forward_p95_ms=206.2 plus1_p95_ms=203.6 backward_p95_ms=207.8 drag_p95_ms=349.9 drag_distinct_fps=6.8 release_shown=true release_ms=36.3 timeouts=0
PF1 seek lane=LH workload=explainer_16x9 run=2 random_p95_ms=205.0 random_max_ms=262.1 forward_p95_ms=203.8 plus1_p95_ms=196.3 backward_p95_ms=205.1 drag_p95_ms=360.8 drag_distinct_fps=6.8 release_shown=true release_ms=33.1 timeouts=0
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=device workload=typical_1080p run=0 valid=true elapsed_s=59.99 due=1800 on_time=0 late=69 dropped=1731 present_p50_ms=980.3 present_p95_ms=1339.5 present_max_ms=1647.5 held_max_ms=1644.6 av_offset_max_ms=1633.3 clock_stall_max_ms=45.4 underrun_frames=282624 peak_rss_mib=928.5 ledger_peak_mib=56.3 device_latency_ms=21.3
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=device workload=blend_heavy_1080p run=0 valid=true elapsed_s=60.00 due=1800 on_time=0 late=31 dropped=1769 present_p50_ms=1964.7 present_p95_ms=1991.6 present_max_ms=1994.0 held_max_ms=1993.0 av_offset_max_ms=2000.0 clock_stall_max_ms=45.4 underrun_frames=1335808 peak_rss_mib=1149.8 ledger_peak_mib=63.3 device_latency_ms=21.3
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=device workload=explainer_16x9 run=0 valid=true elapsed_s=60.00 due=1800 on_time=0 late=66 dropped=1734 present_p50_ms=734.0 present_p95_ms=1388.6 present_max_ms=1521.3 held_max_ms=1519.9 av_offset_max_ms=1533.3 clock_stall_max_ms=45.4 underrun_frames=324608 peak_rss_mib=1284.7 ledger_peak_mib=56.3 device_latency_ms=21.3
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=device workload=reel_9x16 run=0 valid=true elapsed_s=60.00 due=1800 on_time=0 late=40 dropped=1760 present_p50_ms=1308.6 present_p95_ms=2546.6 present_max_ms=3193.6 held_max_ms=3190.0 av_offset_max_ms=3166.7 clock_stall_max_ms=45.3 underrun_frames=1012224 peak_rss_mib=1293.2 ledger_peak_mib=156.9 device_latency_ms=21.3
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=device workload=feed_4x5 run=0 valid=true elapsed_s=59.99 due=1800 on_time=0 late=59 dropped=1741 present_p50_ms=1020.1 present_p95_ms=1992.2 present_max_ms=2482.8 held_max_ms=2482.2 av_offset_max_ms=2466.7 clock_stall_max_ms=45.3 underrun_frames=557056 peak_rss_mib=1174.0 ledger_peak_mib=114.5 device_latency_ms=20.9
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=device workload=talk_recut run=0 valid=true elapsed_s=240.01 due=7200 on_time=0 late=360 dropped=6840 present_p50_ms=673.5 present_p95_ms=725.0 present_max_ms=784.0 held_max_ms=783.0 av_offset_max_ms=766.7 clock_stall_max_ms=45.5 underrun_frames=6656 peak_rss_mib=1010.8 ledger_peak_mib=42.2 device_latency_ms=21.3
PF1 rss before=33.4/2 constructed=169.9/48 first_render=584.7/140 settled_idle=584.8/140 playing=833.4/141 playing_peak_mib=946.3 lane=LL workload=typical_1080p
PF1 rss before=33.5/2 constructed=170.0/48 first_render=677.2/186 settled_idle=677.2/186 playing=995.1/187 playing_peak_mib=995.1 lane=LL workload=blend_heavy_1080p
PF1 rss before=33.5/2 constructed=169.9/48 first_render=546.4/140 settled_idle=546.4/140 playing=964.2/187 playing_peak_mib=973.5 lane=LL workload=explainer_16x9
PF1 rss before=33.5/2 constructed=170.0/48 first_render=492.4/94 settled_idle=492.5/94 playing=1127.0/187 playing_peak_mib=1138.4 lane=LL workload=reel_9x16
PF1 rss before=33.9/2 constructed=170.0/48 first_render=402.7/94 settled_idle=402.7/94 playing=898.7/187 playing_peak_mib=964.8 lane=LL workload=feed_4x5
PF1 rss before=33.6/2 constructed=170.1/48 first_render=381.8/94 settled_idle=381.8/94 playing=649.0/95 playing_peak_mib=651.1 lane=LL workload=talk_recut
PF1 rss before=33.5/2 constructed=170.2/48 first_render=236.0/48 settled_idle=236.0/48 playing=254.0/49 playing_peak_mib=254.0 lane=LL workload=title_only
PF1 rss before=33.4/2 constructed=163.5/11 first_render=624.9/103 settled_idle=624.9/103 playing=862.5/104 playing_peak_mib=862.5 lane=LH workload=typical_1080p
PF1 rss before=33.5/2 constructed=164.0/11 first_render=711.0/149 settled_idle=711.0/149 playing=1026.7/150 playing_peak_mib=1026.7 lane=LH workload=blend_heavy_1080p
PF1 rss before=33.6/2 constructed=163.7/11 first_render=587.7/103 settled_idle=587.7/103 playing=958.6/150 playing_peak_mib=1004.2 lane=LH workload=explainer_16x9
PF1 rss before=33.6/2 constructed=163.9/11 first_render=479.0/57 settled_idle=479.0/57 playing=1031.2/150 playing_peak_mib=1086.4 lane=LH workload=reel_9x16
PF1 rss before=33.5/2 constructed=164.1/11 first_render=450.8/57 settled_idle=450.8/57 playing=937.2/150 playing_peak_mib=980.8 lane=LH workload=feed_4x5
PF1 rss before=33.5/2 constructed=163.7/11 first_render=450.8/57 settled_idle=450.8/57 playing=716.2/58 playing_peak_mib=720.1 lane=LH workload=talk_recut
PF1 rss before=33.5/2 constructed=164.0/11 first_render=305.3/11 settled_idle=305.3/11 playing=326.7/12 playing_peak_mib=326.7 lane=LH workload=title_only
```

### E9.9 Review-fix reruns (R24)

**Provenance.**
- `pf1/impl` at `82fe858` (the review fixes), rustc 1.98.0, release
  (`kinewright_media-cf306d076b2fd4bf`). Same machine, drivers and pinned
  FFmpeg as E9.1.
- 2026-09-27, 07:04–07:32 EDT. The runs were sequential, with no build in
  parallel.
- **The S0 reruns ran under about 1.5 cores of screensaver load** (R26). The
  two `foot` screensaver processes belong to Riel's desktop session, which
  S0 does not change; the load average was 6.5 at the start. No S0
  conclusion depends on this: today's figures miss their gates by 10–50×.
  The S4 `PF1_PINS` must be taken with the screensaver off.
- **The spot checks and P-seek figures predate R25** (`f8035c4`) and R27:
  - the pacer now stamps callbacks at execution and accounts them against
    their scheduled deadlines;
  - underruns are recorded before the clock advances;
  - drag answers are capped at the release receipt.

  No rerun was required (P14). The P-seek tables are unaffected, because no
  frame arrived after a release frame. For D1/D2, the pre-R25 counters
  reported zero misses; they could not have detected the cases R25 and R27
  close.
- Commands, each with `--exact --ignored --nocapture --test-threads=1`:
  - `PF1_ONLY=typical_1080p PF1_RUNS=1 pf1_play_baseline` on LL, then with
    `PF1_HARDWARE=1` on LH. `PF1_ONLY` excludes `controls`, so no control
    ran.
  - `pf1_seek_baseline` on LL and on LH; E9.6 has the table.

**P-play spot checks** (`typical_1080p`, one run per lane, simulated driver).
They exercise the R22 counter, the early-frame rule, the new validity
bounds and the missed-callback check:

| Lane | Valid | Missed callbacks | On time / late / early / dropped | Present p50 / p95 / max ms | Held max ms | Clock stall max ms | underrun_frames (R22, to endpoint) | drain_underrun_frames | Passes |
|---|---|---|---|---|---|---|---|---|---|
| LL | true (60.02 s) | 0 | 0 / 64 / 0 / 1736 | 1028.7 / 1374.5 / 1659.1 | 1659.1 | 47.5 | 355328 | 0 | false |
| LH | true (60.02 s) | 0 | 0 / 63 / 0 / 1737 | 1009.1 / 1564.4 / 1642.0 | 1637.9 | 46.4 | 452608 | 4096 | false |

- **The R22 counter still reports hundreds of thousands of underrun
  frames.** Those are pops that actually failed: the synchronous renders
  drain the ring, as E9.3 explains. The pre-R22 figures for `typical_1080p`
  were in the same range (260,096–525,312 across six simulated runs). The
  ranges overlap, and the runs differ, so they do not measure how much the
  old counter overstated: only that today's real shortfall is of the same
  order.
- **No early frames.** Today's engine renders at or behind the clock. The
  early-frame rule matters once S2 renders ahead.
- **The pre-R25 counters reported zero misses**, and both runs meet the new
  lower bound (≥ 0.98 × 60 s − 0.5 s).
- **G1 and G14 still fail**, as in E9.3.

**Observed once.** After the LH play test reported `ok`, during process
exit, the engine's worker thread panicked in wgpu's
`Buffer::get_mapped_range` (`wgpu_core.rs:2253`). `FfmpegMediaEngine` has no
`Drop` that joins its worker, so a readback in flight when the process
exits can meet a device already torn down. This happened after the
measurement, and the result was complete. It was not seen in the first
run, and nothing in `82fe858` touches that path.

**Raw lines.**

```
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=typical_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=0 late=64 early=0 dropped=1736 present_p50_ms=1028.7 present_p95_ms=1374.5 present_max_ms=1659.1 held_max_ms=1659.1 av_offset_max_ms=1666.7 clock_stall_max_ms=47.5 underrun_frames=355328 drain_underrun_frames=0 peak_rss_mib=984.2 ledger_peak_mib=56.3 passes=false
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=typical_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=0 late=63 early=0 dropped=1737 present_p50_ms=1009.1 present_p95_ms=1564.4 present_max_ms=1642.0 held_max_ms=1637.9 av_offset_max_ms=1633.3 clock_stall_max_ms=46.4 underrun_frames=452608 drain_underrun_frames=4096 peak_rss_mib=894.8 ledger_peak_mib=56.3 passes=false
PF1 seek lane=LL workload=seek_gop60 run=0 random_p95_ms=112.8 random_max_ms=121.8 forward_p95_ms=118.3 plus1_p95_ms=116.5 plus1_n=17 backward_combined_p95_ms=116.8 drag_p95_ms=194.3 drag_answered_p95_ms=194.3 drag_unanswered=0 drag_distinct_fps=10.2 release_pending_drag_calls=3 release_shown=true release_ms=170.6 stale_frames_after_release=1 frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=seek_gop60 run=1 random_p95_ms=117.5 random_max_ms=128.8 forward_p95_ms=118.7 plus1_p95_ms=118.1 plus1_n=11 backward_combined_p95_ms=115.8 drag_p95_ms=196.8 drag_answered_p95_ms=191.1 drag_unanswered=2 drag_distinct_fps=10.2 release_pending_drag_calls=2 release_shown=true release_ms=86.1 stale_frames_after_release=0 frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=seek_gop60 run=2 random_p95_ms=116.3 random_max_ms=123.9 forward_p95_ms=117.5 plus1_p95_ms=116.8 plus1_n=8 backward_combined_p95_ms=115.9 drag_p95_ms=194.8 drag_answered_p95_ms=191.4 drag_unanswered=2 drag_distinct_fps=10.2 release_pending_drag_calls=4 release_shown=true release_ms=102.8 stale_frames_after_release=1 frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=talk_recut run=0 random_p95_ms=147.6 random_max_ms=203.9 forward_p95_ms=162.4 plus1_p95_ms=181.0 plus1_n=17 backward_combined_p95_ms=147.5 drag_p95_ms=220.0 drag_answered_p95_ms=220.0 drag_unanswered=0 drag_distinct_fps=9.2 release_pending_drag_calls=3 release_shown=true release_ms=173.7 stale_frames_after_release=1 frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=talk_recut run=1 random_p95_ms=150.3 random_max_ms=207.6 forward_p95_ms=141.1 plus1_p95_ms=145.8 plus1_n=12 backward_combined_p95_ms=145.7 drag_p95_ms=228.6 drag_answered_p95_ms=225.2 drag_unanswered=2 drag_distinct_fps=9.0 release_pending_drag_calls=6 release_shown=true release_ms=144.4 stale_frames_after_release=1 frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=talk_recut run=2 random_p95_ms=162.0 random_max_ms=223.4 forward_p95_ms=145.3 plus1_p95_ms=145.3 plus1_n=8 backward_combined_p95_ms=144.7 drag_p95_ms=236.7 drag_answered_p95_ms=232.4 drag_unanswered=2 drag_distinct_fps=8.8 release_pending_drag_calls=6 release_shown=true release_ms=129.1 stale_frames_after_release=1 frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=explainer_16x9 run=0 random_p95_ms=204.7 random_max_ms=264.5 forward_p95_ms=204.7 plus1_p95_ms=221.8 plus1_n=17 backward_combined_p95_ms=205.0 drag_p95_ms=359.8 drag_answered_p95_ms=359.8 drag_unanswered=0 drag_distinct_fps=6.8 release_pending_drag_calls=3 release_shown=true release_ms=296.6 stale_frames_after_release=1 frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=explainer_16x9 run=1 random_p95_ms=208.7 random_max_ms=226.9 forward_p95_ms=206.9 plus1_p95_ms=224.5 plus1_n=11 backward_combined_p95_ms=205.8 drag_p95_ms=325.4 drag_answered_p95_ms=324.5 drag_unanswered=1 drag_distinct_fps=7.0 release_pending_drag_calls=5 release_shown=true release_ms=224.7 stale_frames_after_release=1 frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=explainer_16x9 run=2 random_p95_ms=211.6 random_max_ms=273.1 forward_p95_ms=203.5 plus1_p95_ms=204.3 plus1_n=8 backward_combined_p95_ms=206.2 drag_p95_ms=352.6 drag_answered_p95_ms=352.6 drag_unanswered=0 drag_distinct_fps=6.8 release_pending_drag_calls=3 release_shown=true release_ms=178.9 stale_frames_after_release=1 frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=seek_gop60 run=0 random_p95_ms=118.1 random_max_ms=127.0 forward_p95_ms=115.8 plus1_p95_ms=115.8 plus1_n=17 backward_combined_p95_ms=117.1 drag_p95_ms=194.3 drag_answered_p95_ms=194.2 drag_unanswered=1 drag_distinct_fps=10.4 release_pending_drag_calls=4 release_shown=true release_ms=183.1 stale_frames_after_release=1 frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=seek_gop60 run=1 random_p95_ms=119.8 random_max_ms=125.3 forward_p95_ms=117.7 plus1_p95_ms=125.1 plus1_n=11 backward_combined_p95_ms=116.7 drag_p95_ms=197.9 drag_answered_p95_ms=197.7 drag_unanswered=1 drag_distinct_fps=10.0 release_pending_drag_calls=4 release_shown=true release_ms=128.0 stale_frames_after_release=1 frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=seek_gop60 run=2 random_p95_ms=115.5 random_max_ms=126.9 forward_p95_ms=115.2 plus1_p95_ms=104.6 plus1_n=8 backward_combined_p95_ms=115.3 drag_p95_ms=197.4 drag_answered_p95_ms=194.5 drag_unanswered=2 drag_distinct_fps=10.2 release_pending_drag_calls=4 release_shown=true release_ms=94.1 stale_frames_after_release=1 frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=talk_recut run=0 random_p95_ms=150.6 random_max_ms=204.3 forward_p95_ms=163.0 plus1_p95_ms=189.8 plus1_n=17 backward_combined_p95_ms=147.0 drag_p95_ms=212.7 drag_answered_p95_ms=212.7 drag_unanswered=0 drag_distinct_fps=9.6 release_pending_drag_calls=2 release_shown=true release_ms=189.6 stale_frames_after_release=1 frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=talk_recut run=1 random_p95_ms=142.0 random_max_ms=204.4 forward_p95_ms=137.2 plus1_p95_ms=141.5 plus1_n=12 backward_combined_p95_ms=147.3 drag_p95_ms=223.2 drag_answered_p95_ms=223.2 drag_unanswered=0 drag_distinct_fps=9.2 release_pending_drag_calls=4 release_shown=true release_ms=238.7 stale_frames_after_release=1 frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=talk_recut run=2 random_p95_ms=162.8 random_max_ms=225.7 forward_p95_ms=137.1 plus1_p95_ms=144.6 plus1_n=8 backward_combined_p95_ms=152.9 drag_p95_ms=231.5 drag_answered_p95_ms=231.5 drag_unanswered=0 drag_distinct_fps=9.0 release_pending_drag_calls=4 release_shown=true release_ms=190.5 stale_frames_after_release=1 frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=explainer_16x9 run=0 random_p95_ms=200.5 random_max_ms=270.1 forward_p95_ms=204.0 plus1_p95_ms=222.3 plus1_n=17 backward_combined_p95_ms=201.2 drag_p95_ms=376.9 drag_answered_p95_ms=366.2 drag_unanswered=2 drag_distinct_fps=6.6 release_pending_drag_calls=4 release_shown=true release_ms=243.6 stale_frames_after_release=1 frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=explainer_16x9 run=1 random_p95_ms=208.0 random_max_ms=228.1 forward_p95_ms=203.7 plus1_p95_ms=219.9 plus1_n=11 backward_combined_p95_ms=203.5 drag_p95_ms=308.7 drag_answered_p95_ms=302.6 drag_unanswered=2 drag_distinct_fps=7.4 release_pending_drag_calls=6 release_shown=true release_ms=195.5 stale_frames_after_release=1 frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=explainer_16x9 run=2 random_p95_ms=204.9 random_max_ms=266.0 forward_p95_ms=195.7 plus1_p95_ms=208.3 plus1_n=8 backward_combined_p95_ms=202.3 drag_p95_ms=387.9 drag_answered_p95_ms=360.3 drag_unanswered=2 drag_distinct_fps=6.6 release_pending_drag_calls=5 release_shown=true release_ms=109.4 stale_frames_after_release=1 frames_over_release=0 timeouts=0
```

## E10 S1 results

### E10.1 Provenance

- `pf1/impl`: S0 at `4082d5d`, S1a at `47b7d68`, the stage end at `217986a`
  (S1g). rustc 1.98.0, release profile, binary
  `kinewright_media-cf306d076b2fd4bf` (built once per commit and copied
  aside, so every lane of one commit ran the same binary). Same machine as
  E9.1: i5-13600K, RTX 3090 on NVIDIA 615.71.09, llvmpipe (LLVM 22.1.8),
  Linux 7.2.5-4-omarchy, PipeWire; pinned FFmpeg n8.0-23-gd1f31a829d.
- 2026-09-27. S0 and S1a timing 09:53–10:29 EDT; the stage-end runs
  11:21–11:45 EDT. Every timed run ran alone: no build, test or other lane
  alongside.
- **Ambient load (R26), unchanged.** The two `foot` screensaver processes
  used about 1.5 cores throughout (≈107% + 54% CPU at 11:21). A transient
  `omarchy-agent-u` process at ~99% CPU was seen during the S0 runs. Load
  averages were 3.3–8.4 across the runs; each log records `uptime` before
  and after.
- Commands, each `--exact --ignored --nocapture --test-threads=1` from
  `crates/kinewright-media`: `mo2_perf_fixtures::r28_end_to_end_tracked`
  (LL; `R28_HARDWARE=1` for LH); `mo2_perf_fixtures::blend_heavy_holds_floors_on_hardware`
  (G3, LH) and `…_on_the_fallback_adapter` (LL, recorded);
  `PF1_ONLY=typical_1080p PF1_RUNS=1 pf1_harness::pf1_play_baseline` (LL;
  `PF1_HARDWARE=1` for LH). The CI gates ran on the same release binary
  (LL, `--nocapture`), and in `cargo test -p kinewright-media` at each
  commit from the one that introduced them (E10.4).
- **Review fixes (R29).** `507e22c`, `9ee47fa`, `d8c6364` and `bb9b68d`
  fix both S1 reviews' findings after the stage end; E10.8 records them.
  The timing lanes were not rerun (E10.8 says why); the correctness gates'
  raw lines at `bb9b68d` are in E10.6.

### E10.2 I4 (S0's 5% rule, `BASELINES` untouched)

`mo2_perf_fixtures.rs` and `perf_fixtures.rs` are unchanged since
`4082d5d`.

| Build | LL mean ms (3 runs) | LL delta | LH mean ms (3 runs) | LH delta | Result |
|---|---|---|---|---|---|
| S0 `4082d5d`, same session | 529.83 / 528.64 / 531.24 | +6.4% | 526.96 / 526.99 / 528.01 | +6.7% | FAILED on both lanes |
| S1a `47b7d68` (control alone) | 232.95 / 232.66 / 233.48 | −53.2% | 231.93 / 229.98 / 230.32 | −53.3% | ok |
| S1 end `217986a` | 77.03 / 76.23 / 75.95 | −84.7% | 72.87 / 73.00 / 73.69 | −85.2% | ok |

- **The unmodified S0 binary fails I4 in this session** (+6.4% / +6.7%,
  against +2.4% / +2.6% in E9.2). The code is the same, so this is
  session-to-session variability; the ambient load above is the likely
  cause but is not shown to be the only one. It shows the 5% rule's margin
  is within this desktop's noise; the S1 figures clear it by 80 points.
- **S1a's own effect** (the control, measured before S1b): −56% of the
  end-to-end mean on both lanes (529.8 → 233.0 ms LL, 527.0 → 230.7 ms
  LH), from removing one per-pixel `Vec` collect.
- S1b–S1g take it a further −67% (233 → 76 ms LL, 231 → 73 ms LH).

### E10.3 G3 (`blend_heavy_holds_floors_on_hardware`, 60 fps floor, LH)

| Build | Lane | `blend_heavy_1080p` fps (3 runs) | mean ms | p95 ms | Result |
|---|---|---|---|---|---|
| S0 `4082d5d`, same session | LH | 25.6 / 25.8 / 25.8 | 38.7–39.0 | 46.9–48.4 | FAILED (design value 27.4) |
| S1 end `217986a` | LH | 72.1 / 71.4 / 71.7 | 13.86–14.00 | 15.47–15.67 | **passes** |
| S1 end `217986a` | LL (recorded) | 47.0 / 47.3 / 47.2 | 21.13–21.29 | 23.60–23.83 | the fallback test's floor holds |

- The other R28 resident workloads on LH at S1: `typical_1080p` 72.1–73.1
  fps, `heavy_4k` 57.8–58.8 fps. The slowdown control still fails its
  verdict on both lanes, as it must.
- **The `R28 phases` lines are a diagnostic, not production.**
  `render::phases::monitor` (ME14) still calls the f32
  `encode_monitor_rgba8_for_description` per pixel, so its
  `monitor_encode_ms` (≈19–27 ms) is the pre-G-1 cost. Its `upload_ms` is
  production's `Compositor::composite` (layer staging, upload and pass
  recording), ≈10–13 ms on LH, now most of a 14 ms resident frame; see
  E10.7 (U-1).

### E10.4 G9, G12, G5 and I9 (CI)

Each passes in `cargo test -p kinewright-media` at every commit from the
one that introduced it (G5 `8e1a285`; G9 and I9 `207fd6c`; G12 `217986a`;
the I1 tests `47b7d68`–`1d54158`; before those commits the tests did not
exist), and on the stage-end release binary (LL). The raw lines, rerun
after the review fixes at `bb9b68d`, are in E10.6:
- **G9** `audio::tests::the_clock_counts_whole_popped_frames_only`: a partial
  frame pops one frame (position 10 → 11); an empty ring leaves the position
  unchanged while `underrun_frames` counts every frame.
  `callback_consumes_ring_then_writes_silence_and_accounts_frames` is amended
  as §9 says (position 11, `[0.25, -0.5, 0, 0]`, 1 underrun frame, return 2).
  `stepped_callbacks_pop_through_render_output_and_count_underruns` now reads
  (1,125, 1,023): the clock stops where the ring ran dry (T7 had 2,148).
- **G12** `engine::tests::the_monitor_caps_the_long_edge_at_1280`: the engine
  presents a 1080×1920 document (the `reel_9x16` size) at 720×1280; 4:5 →
  1024×1280, 16:9 and small documents unchanged.
- **G5** `render::tests::preview_window_plays_two_continuous_sources_without_seeking`:
  two continuous 64×36 sources under a 12-frame budget, 60 frames: the
  preview renderer seeks 0 times after warm-up; the control
  (`FrameRenderer::new`, today's round-robin) keeps seeking (109 seeks
  after warm-up during development); the test asserts it is non-zero.
- **I9** `engine::tests::a_drained_one_frame_timeline_stops_at_its_duration`
  (30000/1001, 48 kHz, ends at sample 1,601 where the clock reads frame 0)
  and `…a_long_programme_stops_at_its_duration_and_a_stall_does_not_complete`
  (5 s; 94 callbacks with no fill leave the clock short of the end, still
  playing, not drained): both end with `position()` = duration,
  `paused_at` = duration, the stream dropped, `sample_rate` 0 and
  `Paused` then `Position(duration)`. With the drained predicate disabled,
  the one-frame test fails (it never stops). AU2/AU3/AU4 suites unedited
  and green.
- **I1** (exhaustive): `decode::tests::input_tables_match_every_accepted_descriptor`
  (442 accepted tuples through `classify_source_with_assumption` and
  `select_conversion`, 2,567,942 rejected, every code; since `507e22c`
  each accepted tuple is checked against its own per-pixel reference
  decode, not one reference per production `ConversionKey` (review A F2);
  the 442 tuples share 90 keys),
  `decode::tests::fused_table_fill_matches_the_unfused_path_for_every_orientation`,
  `frame::tests::rgba64_decode_matches_the_collect_reference_bit_for_bit`
  and `conversion::tests::monitor_table_equals_the_f32_encode_for_every_f16_pattern`.

### E10.5 P-play (`typical_1080p`, one run per lane, simulated driver)

| Build | Lane | On time / late / dropped of 1800 | Present p50 / p95 / max ms | Held max ms | A/V offset max ms | underrun_frames (R22) | table_live_kib |
|---|---|---|---|---|---|---|---|
| S0 | LL | 0 / 66 / 1734 | 937.1 / 1559.9 / 1806.8 | 1804.9 | 1800.0 | 386048 | — |
| S0 | LH | 0 / 61 / 1739 | 924.9 / 1751.5 / 1765.2 | 1763.6 | 1766.7 | 502784 | — |
| S1a | LL | 0 / 119 / 1681 | 502.7 / 576.9 / 674.0 | 672.0 | 666.7 | 512 | — |
| S1a | LH | 0 / 120 / 1680 | 497.5 / 564.9 / 674.3 | 671.2 | 666.7 | 512 | — |
| S1 end | LL | 450 / 640 / 710 | 33.6 / 146.0 / 173.6 | 170.9 | 200.0 | 512 | 128 |
| S1 end | LH | 523 / 590 / 687 | 38.3 / 145.6 / 188.7 | 185.8 | 200.0 | 512 | 128 |

G1/G14 are S2 gates and still fail (`passes=false`): the worker still
renders synchronously in `tick`. All runs are valid under the E9.9 rules
(60.02 s, 0 missed callbacks). One live 128 KiB table (the BT.709 8-bit
key) serves the whole workload. After the S1 LH run reported `ok`, process
exit panicked in `khronos-egl` (`lib.rs:841`), as in the S1a LH run; it is
teardown after the measurement (compare E9.9's exit-time wgpu panic).

### E10.6 Raw result lines

I4 (`r28_end_to_end_tracked`):

```
# S0 (4082d5d), same session
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=false workload=typical_1080p run=0 dims=(1280, 720) mean_ms=529.83 fps=1.9 p95_ms=1681.10 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=false workload=typical_1080p run=1 dims=(1280, 720) mean_ms=528.64 fps=1.9 p95_ms=1685.60 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=false workload=typical_1080p run=2 dims=(1280, 720) mean_ms=531.24 fps=1.9 p95_ms=1687.03 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) workload=typical_1080p baseline_ms=497.86 delta=+6.4%
R28 adapter=NVIDIA GeForce RTX 3090 resident=false workload=typical_1080p run=0 dims=(1280, 720) mean_ms=526.96 fps=1.9 p95_ms=1680.47 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 resident=false workload=typical_1080p run=1 dims=(1280, 720) mean_ms=526.99 fps=1.9 p95_ms=1684.47 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 resident=false workload=typical_1080p run=2 dims=(1280, 720) mean_ms=528.01 fps=1.9 p95_ms=1678.17 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 workload=typical_1080p baseline_ms=494.39 delta=+6.7%
# S1a (47b7d68)
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=false workload=typical_1080p run=0 dims=(1280, 720) mean_ms=232.95 fps=4.3 p95_ms=680.46 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=false workload=typical_1080p run=1 dims=(1280, 720) mean_ms=232.66 fps=4.3 p95_ms=685.46 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=false workload=typical_1080p run=2 dims=(1280, 720) mean_ms=233.48 fps=4.3 p95_ms=683.85 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) workload=typical_1080p baseline_ms=497.86 delta=-53.2%
R28 adapter=NVIDIA GeForce RTX 3090 resident=false workload=typical_1080p run=0 dims=(1280, 720) mean_ms=231.93 fps=4.3 p95_ms=682.44 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 resident=false workload=typical_1080p run=1 dims=(1280, 720) mean_ms=229.98 fps=4.3 p95_ms=678.14 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 resident=false workload=typical_1080p run=2 dims=(1280, 720) mean_ms=230.32 fps=4.3 p95_ms=681.21 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 workload=typical_1080p baseline_ms=494.39 delta=-53.3%
# S1 end (217986a)
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=false workload=typical_1080p run=0 dims=(1280, 720) mean_ms=77.03 fps=13.0 p95_ms=210.06 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=false workload=typical_1080p run=1 dims=(1280, 720) mean_ms=76.23 fps=13.1 p95_ms=208.88 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=false workload=typical_1080p run=2 dims=(1280, 720) mean_ms=75.95 fps=13.2 p95_ms=206.03 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) workload=typical_1080p baseline_ms=497.86 delta=-84.7%
R28 adapter=NVIDIA GeForce RTX 3090 resident=false workload=typical_1080p run=0 dims=(1280, 720) mean_ms=72.87 fps=13.7 p95_ms=204.63 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 resident=false workload=typical_1080p run=1 dims=(1280, 720) mean_ms=73.00 fps=13.7 p95_ms=202.29 ledger_peak_mib=56.3 validate_us=2.2
R28 adapter=NVIDIA GeForce RTX 3090 resident=false workload=typical_1080p run=2 dims=(1280, 720) mean_ms=73.69 fps=13.6 p95_ms=207.84 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 workload=typical_1080p baseline_ms=494.39 delta=-85.2%
```

G3:

```
# S1 end (217986a), LH: blend_heavy_holds_floors_on_hardware — ok
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=typical_1080p run=0 dims=(1280, 720) mean_ms=13.68 fps=73.1 p95_ms=15.34 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=typical_1080p run=1 dims=(1280, 720) mean_ms=13.88 fps=72.1 p95_ms=16.88 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=typical_1080p run=2 dims=(1280, 720) mean_ms=13.72 fps=72.9 p95_ms=15.63 ledger_peak_mib=56.3 validate_us=1.3
R28 phases workload=typical_1080p upload_ms=10.05 gpu_passes_readback_ms=1.31 monitor_encode_ms=19.36
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=blend_heavy_1080p run=0 dims=(1280, 720) mean_ms=13.86 fps=72.1 p95_ms=15.47 ledger_peak_mib=63.3 validate_us=1.4
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=blend_heavy_1080p run=1 dims=(1280, 720) mean_ms=14.00 fps=71.4 p95_ms=15.67 ledger_peak_mib=63.3 validate_us=1.2
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=blend_heavy_1080p run=2 dims=(1280, 720) mean_ms=13.95 fps=71.7 p95_ms=15.62 ledger_peak_mib=63.3 validate_us=1.2
R28 phases workload=blend_heavy_1080p upload_ms=10.94 gpu_passes_readback_ms=1.46 monitor_encode_ms=26.55
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=heavy_4k run=0 dims=(1280, 720) mean_ms=17.31 fps=57.8 p95_ms=20.34 ledger_peak_mib=70.3 validate_us=9.2
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=heavy_4k run=1 dims=(1280, 720) mean_ms=17.01 fps=58.8 p95_ms=19.11 ledger_peak_mib=70.3 validate_us=14.7
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=heavy_4k run=2 dims=(1280, 720) mean_ms=17.01 fps=58.8 p95_ms=18.64 ledger_peak_mib=70.3 validate_us=9.1
R28 phases workload=heavy_4k upload_ms=13.37 gpu_passes_readback_ms=1.72 monitor_encode_ms=20.10
R28 control=slowdown delay_ms=41.7 mean_ms=56.28 p95_ms=57.89 verdict=Err("17.8 fps (floor 60), p95 57.9 ms vs mean 56.3 ms")
# S0 (4082d5d), LH, same session — FAILED (floor 60)
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=typical_1080p run=0 dims=(1280, 720) mean_ms=30.97 fps=32.3 p95_ms=34.54 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=typical_1080p run=1 dims=(1280, 720) mean_ms=31.22 fps=32.0 p95_ms=35.44 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=typical_1080p run=2 dims=(1280, 720) mean_ms=32.70 fps=30.6 p95_ms=44.66 ledger_peak_mib=56.3 validate_us=1.3
R28 phases workload=typical_1080p upload_ms=10.03 gpu_passes_readback_ms=1.77 monitor_encode_ms=19.61
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=blend_heavy_1080p run=0 dims=(1280, 720) mean_ms=39.01 fps=25.6 p95_ms=48.42 ledger_peak_mib=63.3 validate_us=2.0
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=blend_heavy_1080p run=1 dims=(1280, 720) mean_ms=38.74 fps=25.8 p95_ms=47.04 ledger_peak_mib=63.3 validate_us=1.9
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=blend_heavy_1080p run=2 dims=(1280, 720) mean_ms=38.82 fps=25.8 p95_ms=46.89 ledger_peak_mib=63.3 validate_us=1.2
R28 phases workload=blend_heavy_1080p upload_ms=10.03 gpu_passes_readback_ms=1.84 monitor_encode_ms=25.86
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=heavy_4k run=0 dims=(1280, 720) mean_ms=35.05 fps=28.5 p95_ms=42.08 ledger_peak_mib=70.3 validate_us=9.5
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=heavy_4k run=1 dims=(1280, 720) mean_ms=34.48 fps=29.0 p95_ms=37.67 ledger_peak_mib=70.3 validate_us=9.4
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=heavy_4k run=2 dims=(1280, 720) mean_ms=34.56 fps=28.9 p95_ms=37.62 ledger_peak_mib=70.3 validate_us=9.5
R28 phases workload=heavy_4k upload_ms=13.23 gpu_passes_readback_ms=1.86 monitor_encode_ms=18.81
R28 control=slowdown delay_ms=41.7 mean_ms=80.16 p95_ms=90.67 verdict=Err("12.5 fps (floor 60), p95 90.7 ms vs mean 80.2 ms")
R28 gate 10 failed: [
# S1 end (217986a), LL: blend_heavy_holds_floors_on_the_fallback_adapter — ok
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=true workload=typical_1080p run=0 dims=(1280, 720) mean_ms=16.86 fps=59.3 p95_ms=19.46 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=true workload=typical_1080p run=1 dims=(1280, 720) mean_ms=17.39 fps=57.5 p95_ms=20.57 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=true workload=typical_1080p run=2 dims=(1280, 720) mean_ms=17.13 fps=58.4 p95_ms=19.71 ledger_peak_mib=56.3 validate_us=1.3
R28 phases workload=typical_1080p upload_ms=8.56 gpu_passes_readback_ms=6.06 monitor_encode_ms=19.17
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=true workload=blend_heavy_1080p run=0 dims=(1280, 720) mean_ms=21.29 fps=47.0 p95_ms=23.60 ledger_peak_mib=63.3 validate_us=2.1
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=true workload=blend_heavy_1080p run=1 dims=(1280, 720) mean_ms=21.13 fps=47.3 p95_ms=23.74 ledger_peak_mib=63.3 validate_us=1.2
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=true workload=blend_heavy_1080p run=2 dims=(1280, 720) mean_ms=21.18 fps=47.2 p95_ms=23.83 ledger_peak_mib=63.3 validate_us=1.2
R28 phases workload=blend_heavy_1080p upload_ms=8.85 gpu_passes_readback_ms=10.56 monitor_encode_ms=26.34
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=true workload=heavy_4k run=0 dims=(1280, 720) mean_ms=23.43 fps=42.7 p95_ms=26.93 ledger_peak_mib=70.3 validate_us=9.3
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=true workload=heavy_4k run=1 dims=(1280, 720) mean_ms=23.34 fps=42.8 p95_ms=26.55 ledger_peak_mib=70.3 validate_us=10.0
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=true workload=heavy_4k run=2 dims=(1280, 720) mean_ms=23.59 fps=42.4 p95_ms=26.22 ledger_peak_mib=70.3 validate_us=9.2
R28 phases workload=heavy_4k upload_ms=12.02 gpu_passes_readback_ms=9.55 monitor_encode_ms=20.21
R28 control=slowdown delay_ms=312.5 mean_ms=334.09 p95_ms=336.64 verdict=Err("3.0 fps (floor 8), p95 336.6 ms vs mean 334.1 ms")
```

P-play:

```
# S0 (4082d5d)
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=typical_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=0 late=66 early=0 dropped=1734 present_p50_ms=937.1 present_p95_ms=1559.9 present_max_ms=1806.8 held_max_ms=1804.9 av_offset_max_ms=1800.0 clock_stall_max_ms=45.5 underrun_frames=386048 drain_underrun_frames=0 peak_rss_mib=1013.4 ledger_peak_mib=56.3 passes=false
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=typical_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=0 late=61 early=0 dropped=1739 present_p50_ms=924.9 present_p95_ms=1751.5 present_max_ms=1765.2 held_max_ms=1763.6 av_offset_max_ms=1766.7 clock_stall_max_ms=45.8 underrun_frames=502784 drain_underrun_frames=5120 peak_rss_mib=891.8 ledger_peak_mib=56.3 passes=false
# S1a (47b7d68)
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=typical_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=0 late=119 early=0 dropped=1681 present_p50_ms=502.7 present_p95_ms=576.9 present_max_ms=674.0 held_max_ms=672.0 av_offset_max_ms=666.7 clock_stall_max_ms=47.5 underrun_frames=512 drain_underrun_frames=9216 peak_rss_mib=851.5 ledger_peak_mib=56.3 passes=false
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=typical_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=0 late=120 early=0 dropped=1680 present_p50_ms=497.5 present_p95_ms=564.9 present_max_ms=674.3 held_max_ms=671.2 av_offset_max_ms=666.7 clock_stall_max_ms=45.8 underrun_frames=512 drain_underrun_frames=8192 peak_rss_mib=959.8 ledger_peak_mib=56.3 passes=false
# S1 end (217986a)
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=typical_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=450 late=640 early=0 dropped=710 present_p50_ms=33.6 present_p95_ms=146.0 present_max_ms=173.6 held_max_ms=170.9 av_offset_max_ms=200.0 clock_stall_max_ms=47.9 underrun_frames=512 drain_underrun_frames=0 peak_rss_mib=860.3 ledger_peak_mib=56.3 table_live_kib=128 passes=false
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=typical_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=523 late=590 early=0 dropped=687 present_p50_ms=38.3 present_p95_ms=145.6 present_max_ms=188.7 held_max_ms=185.8 av_offset_max_ms=200.0 clock_stall_max_ms=45.6 underrun_frames=512 drain_underrun_frames=1024 peak_rss_mib=898.6 ledger_peak_mib=56.3 table_live_kib=128 passes=false
```

Correctness gates (review B S3), `bb9b68d`, debug profile, default lane
(LL, llvmpipe), `cargo test -p kinewright-media --lib -- --exact
--test-threads=1 <names>`. FFmpeg's stderr log lines that interleave with
four of the `ok` lines are removed; the summary line is as printed:

```
running 23 tests
test audio::tests::callback_consumes_ring_then_writes_silence_and_accounts_frames ... ok
test audio::tests::the_clock_counts_whole_popped_frames_only ... ok
test cache::tests::backward_travel_drops_the_frames_above_the_demand_first ... ok
test cache::tests::capacity_eviction_keeps_the_pinned_frames ... ok
test cache::tests::distance_eviction_spares_the_shown_frame_and_drops_behind_travel_first ... ok
test cache::tests::travel_follows_reversals_and_seeks ... ok
test conversion::tests::a_key_being_built_is_built_once_under_eviction ... ok
test conversion::tests::built_tables_are_counted_while_held_and_alpha_is_exact ... ok
test conversion::tests::monitor_table_equals_the_f32_encode_for_every_f16_pattern ... ok
test decode::tests::input_tables_match_every_accepted_descriptor ... ok
test engine::tests::a_drained_one_frame_timeline_stops_at_its_duration ... ok
test engine::tests::a_long_programme_stops_at_its_duration_and_a_stall_does_not_complete ... ok
test engine::tests::a_pause_after_the_programme_drained_stops_at_the_duration ... ok
test engine::tests::a_playing_seek_that_races_the_terminal_stop_keeps_playing ... ok
test engine::tests::a_seek_after_the_stop_or_before_a_pause_stays_paused ... ok
test engine::tests::only_the_live_monitor_encodes_through_the_table ... ok
test engine::tests::the_monitor_caps_the_long_edge_at_1280 ... ok
test mo2_perf_fixtures::r28_ledger_control_rejects_the_1080p_budget ... ok
test mo2_perf_fixtures::r28_ledger_holds_the_ceilings_and_releases_every_charge ... ok
test render::tests::a_thumbnail_leaves_the_preview_demand_alone ... ok
test render::tests::capacity_eviction_keeps_every_shown_frame_of_one_source ... ok
test render::tests::overshoot_is_bounded_by_the_demand_points ... ok
test render::tests::preview_window_plays_two_continuous_sources_without_seeking ... ok
test result: ok. 23 passed; 0 failed; 0 ignored; 0 measured; 877 filtered out; finished in 52.60s
```

G5 = `preview_window_plays…`; G9 = `the_clock_counts…` and the amended
`callback_consumes…`; G12 = `the_monitor_caps…`; I9 = the two
`…stops_at_its_duration…` tests; I3 = the two `r28_ledger_*` tests (MO2's,
unchanged by S1: S1's tables are CPU allocations and charge no GPU ledger).
The full `cargo test --workspace` at `bb9b68d` is in E10.8.

### E10.7 Deviations and notes

- **S1e (U-1) is not implemented — resolved by R28: U-1 moves to S3b.**
  Its condition is "S0 finds
  `upload_bytes`'s extra full copy material"; S0 never measured it (E3 says
  S0 would; E9 has no figure). A scratch release measurement (not
  committed) gives `upload_bytes` 1.829 ms against a plain copy's 0.250 ms
  for one 1280×720 layer (7,372,800 bytes), about 1.6 ms of extra copy per
  layer per frame, and the phases `upload_ms` above is 10–13 ms of a 14 ms
  LH frame. G3 passes without it. An exact byte cast needs
  `zerocopy::IntoBytes` for `f16` (`half` 2.7.1 already implements it and
  zerocopy is already in the lock file) as a direct dependency of
  `kinewright-media`, because the workspace forbids `unsafe`. (Proposed at
  the stage end; R28 moved it to S3b's staging ring.)
- **Two S0 harness tests are adjusted for V-1** besides the named test
  (C-5 says only the V-1 test is edited):
  `stepped_callbacks_pop_through_render_output_and_count_underruns` (its
  comment already said "V-1 fixes that in S1") and
  `an_observed_clock_never_runs_ahead_of_its_underruns`, which relied on the
  clock advancing over an empty ring (it would never finish under V-1) and
  now half-fills the ring each callback. **Resolved by R28:** C-5 now
  permits these V-1 edits to S0's harness tests.
- **The live table counter is test-only** (`table_live_kib=` on the
  harness line). `CacheStats` is public wire data, so the production
  exposure is left to S2a's preview `stats`.
- **K-6 overshoot (corrected twice: review B S2, re-review D4).** Titles
  are still evicted after every video frame. The frame shown at each
  *demand point* is pinned, one source can have several points, and a
  source at or under its share is never asked to give up a frame, so the
  S1 preview renderer can hold more than C. What is true of its reservation
  loop (`reserve_for`): it stops only when the total fits C or nothing is
  evictable, i.e. no inactive source holds a frame, no title is cached, and
  every active source s holds at most its share (C − G)/n or only its
  pinned frames. So, with p_s the bytes of s's pinned frames:
  - after each video layer's final reservation, live ≤ max(C, Σ_s
    max((C − G)/n, p_s));
  - while a layer's window is being decoded, add at most that window: w_r
    frames of the requesting source r, w_r ≤ `PREFETCH_FRAMES` + 1 = 16
    (at least one frame, so a frame larger than its share still lands),
    each of r's proxy frame bytes f_r.
  The earlier "max(C, P·f)" is false across sources:
  `mixed_sources_hold_their_shares_and_pins_over_the_budget` (C = 4f; A
  demands 0/100/200, B demands 0; each share 2f) ends with A holding its
  three pins and B its 2f window, 5f, over max(C, P·f) = 4f and equal to
  the bound above (3f + 2f). `overshoot_is_bounded_by_the_demand_points`
  is the one-source case (a one-frame budget holds 3f). Per R30 the S1
  eviction policy is unchanged: **S2b-3's I12 (live ≤ C at all times)
  must close this**, with K-2's atomic admission and K-5's eviction.
- **Budget:** 1,029 non-blank, non-comment `.rs` lines added and 93
  removed across S1a–S1g, against ~650; the exhaustive and worker tests
  are most of the excess (per-commit counts are in the S1 report).

### E10.8 Review fixes (R29)

Both S1 reviews (`target/review/pf/review-s1-A.md`, `review-s1-B.md`)
accepted with fixes. Every finding was confirmed against the code; none is
disputed.

"Witness" tests fail with their fix reverted (the re-review confirmed
these distinguish the old behaviour); "coverage" tests pass on the old
code too and document or widen what is checked.

| Finding | Commit | Fix | Witness / coverage |
|---|---|---|---|
| A F1: G-1 reached proofs and agent images | `9ee47fa` | `MonitorPurpose`; only `Worker::present` (`render_live`) encodes through the table; proofs, `Control::Thumbnail` and the fixtures keep the f32 encode, except the R28/G3 bench (`mo2_bench`), which requests `LiveMonitor` to measure the live path | witness `only_the_live_monitor_encodes_through_the_table` |
| A F2: oracle memoised by `ConversionKey` | `507e22c` | a per-pixel reference decode per accepted tuple | coverage: the exhaustive test itself |
| A F3: two builds of one key under eviction | `507e22c`, `cfcd756` | a key being built is never evicted (since `cfcd756`: kept in `building`, outside the LRU) | witness `a_key_being_built_is_built_once_under_eviction` |
| A should: counter lifecycle | `507e22c` | isolated counter and registry: eviction retains, clones, final drop, failed builds add 0 | coverage `built_tables_are_counted_while_held_and_alpha_is_exact` |
| B F1: EOS turned a racing seek into a paused one | `bb9b68d` | tick defers the stop while a seek is pending; a seek stamped before the stop's `eos_generation` resumes playing | witness `a_playing_seek_that_races_the_terminal_stop_keeps_playing`; coverage (control) `a_seek_after_the_stop_or_before_a_pause_stays_paused` |
| B F2: a pause after drain stopped short | `bb9b68d`, `1fad53f` | `Control::Pause` takes `stop_at_end` once the ring has drained (since `1fad53f`: the ring only, and no seek pending) | witness `a_pause_after_the_programme_drained_stops_at_the_duration` |
| B F3: capacity eviction ignored pins | `d8c6364` | `FrameCache::insert` evicts the oldest *unpinned* entry | witness `capacity_eviction_keeps_every_shown_frame_of_one_source` (old code: frame 0 evicted); coverage `capacity_eviction_keeps_the_pinned_frames` |
| B F4: "behind" ignored direction | `d8c6364`, `a5362ff` | per-cache travel: the demand point that moved least sets it (forward on ties, per point too since `a5362ff`); a discontinuous seek is a step in the jump's direction; unmoved or cleared demand keeps it; the first is Forward | witness `backward_travel_drops_the_frames_above_the_demand_first`; coverage `travel_follows_reversals_and_seeks` |
| B F5: thumbnails used the preview policy | `9ee47fa`, `a5362ff` | `render_thumbnail` sets the preview demand aside (since `a5362ff` through a drop guard), so no demand rebuild, pins, window or distance eviction | witness `a_thumbnail_leaves_the_preview_demand_alone` |
| B S1: drained ≠ audible | docs | §9 V-2 and D9 state the boundary | — |
| B S2: overshoot bound | `d8c6364`, docs | E10.7 (corrected again by E10.9, D4) | coverage `overshoot_is_bounded_by_the_demand_points` |
| B S3: raw gate lines | docs | E10.6 | — |

- **B S1 — drained means the ring is drained, not that the device has
  played it.** The clock advances inside the output callback, before cpal
  hands the buffer to ALSA (`writei`) or WASAPI (`ReleaseBuffer`); the
  stop can therefore cut up to the device's queued output before it
  sounds. The simulated fixtures cannot show last-audible-sample
  completion, and S1 does not claim device drain. An output-drain
  acknowledgment and its test belong to the AU follow-up with D9.
- **Continuous seeking defers the end (accepted boundary).** While a seek
  is pending, `tick` does not take the terminal stop. A finite burst of
  seeks is consumed on the next worker pass (`handle_coalesced_requests`
  runs every iteration), so the stop follows at once; only a caller that
  publishes a new seek before every pass could defer it indefinitely, and
  then the transport is where that caller keeps putting it. The
  `eos_generation` bump, before `Paused` is emitted, is the stop boundary.
- **Timing lanes not rerun.** No fix moves G3 or G5's hot path: G3's
  bench (`mo2_bench`) renders through `FrameRenderer::new` with
  `MonitorPurpose::LiveMonitor`, the same encode as `217986a`; a legacy
  renderer has no demand, so capacity eviction takes the same entry as
  before (an empty pin list, no allocation) and `pin_demand` is a no-op
  pass over its sources. G5 is a seek-count gate and passes (E10.6).
- **Workspace gate at `bb9b68d`** (debug, default lane, default threads):
  `cargo test --workspace` exit 0, 36 test binaries, 3,364 passed,
  0 failed, 41 ignored; `cargo fmt -- --check` clean. Each review-fix
  commit also passed the workspace build, `clippy --workspace
  --all-targets -D warnings`, rustfmt on its files and the full
  `cargo test -p kinewright-media` (880 lib tests passed, 20 ignored, at
  `bb9b68d`).

### E10.9 Re-review fixes (R30)

The S1 fix re-review (`target/review/pf/rereview-s1.md`) accepted with
fixes: four defects and nits, each confirmed against the code, none
disputed. Per R30 the S1 eviction policy is unchanged (D4 is documented).

| Item | Commit | Fix | Witness (fails on `1fc7d01`) / coverage |
|---|---|---|---|
| D1: `seek(duration)` + `Pause` manufactured completion | `1fad53f` | `pause_or_stop_at_end` decides from the current runtime's drained ring (samples the callback consumed), never `position()`; a pending seek keeps the ordinary pause | witness `a_pause_behind_a_seek_to_the_end_is_not_terminal`: exactly `[Paused, Position(duration)]`, undrained and drained; the old code emits `[Paused, Position(50), Position(50)]` |
| D2: registry above eight keys; abandoned cells | `cfcd756` | builds in flight live in `building` (cell + waiting requests), outside the ready LRU; the last request to leave a finished build moves it to the LRU, trimmed to eight; a drop guard forgets a build whose builder unwound | witnesses `concurrent_builds_finish_within_the_key_bound` (old: 9 held) and `a_panicking_build_is_forgotten_and_a_retry_builds` (old: the empty cell stays); `a_key_being_built_is_built_once_under_eviction` still green |
| D3: nearest-previous-point tie ignored forward | `a5362ff` | `(magnitude, backward)` ordering per point as well as across points | witness `travel_ties_resolve_forward_in_any_order` (old: `[10, 0] → [5]` read Backward) |
| D4: the overshoot bound was false across sources | docs, `a5362ff` | E10.7 states the true bound, Σ_s max(share, pinned_s) plus one window in flight; I12 (S2b-3) must close it | coverage (documents today's behaviour) `mixed_sources_hold_their_shares_and_pins_over_the_budget`: 5f with C = 4f |
| Nit: thumbnail restore not scoped | `a5362ff` | `PreviewAside` drop guard | coverage: the thumbnail test's Err path and an unwind |
| Nit: E10.8 wording | docs | the bench's `LiveMonitor`; witness vs coverage labels | — |
| Nit: continuous seeks defer EOS | docs | E10.8 records the accepted boundary | — |

Raw lines, `1fad53f`, debug profile, default lane (LL, llvmpipe),
`cargo test -p kinewright-media --lib -- --exact --test-threads=1 <names>`
(stderr discarded):

```
running 7 tests
test cache::tests::travel_ties_resolve_forward_in_any_order ... ok
test conversion::tests::a_key_being_built_is_built_once_under_eviction ... ok
test conversion::tests::a_panicking_build_is_forgotten_and_a_retry_builds ... ok
test conversion::tests::concurrent_builds_finish_within_the_key_bound ... ok
test engine::tests::a_pause_behind_a_seek_to_the_end_is_not_terminal ... ok
test render::tests::a_thumbnail_leaves_the_preview_demand_alone ... ok
test render::tests::mixed_sources_hold_their_shares_and_pins_over_the_budget ... ok
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 898 filtered out; finished in 1.61s
```

- **"Fails on `1fc7d01`"** was shown by running each witness against the
  previous code (for D2, the previous registry with a `held()` count
  added; for D1 and D3, the previous predicate or ordering restored);
  each failed as the table says, and passes at `1fad53f`.
- **Workspace gate at `1fad53f`** (debug, default lane, default threads):
  `cargo test --workspace` exit 0, 36 test binaries, 3,369 passed,
  0 failed, 41 ignored; `cargo fmt -- --check` clean. Each R30 commit
  passed the workspace build, `clippy --workspace --all-targets -D
  warnings`, rustfmt on its files and the full `cargo test -p
  kinewright-media` (885 lib tests passed, 20 ignored, at `1fad53f`).
- **Timing lanes not rerun:** the R30 fixes touch table-registry
  bookkeeping on a cache miss, the pause path, a tie-break in
  `set_demand` and the thumbnail path; none is on G3's resident render
  loop, and G5 passes.

## E11 S2a results

### E11.1 Provenance

- `pf1/impl`: S2a is `6b423a4`…`9101e18`, plus `58e2f69` (the R-5 end-frame
  fix, found by these runs; E11.4). The timing lanes ran the release test
  binary of `9101e18` (`kinewright_media-cf306d076b2fd4bf`, sha256
  `d21d2bd93166b6b1…`), built once and copied aside, so every lane ran the
  same binary. rustc 1.98.0. Same machine as E9.1/E10.1: i5-13600K, RTX 3090
  on NVIDIA 615.71.09, llvmpipe (LLVM 22.1.8), Linux 7.2.5-4-omarchy,
  PipeWire; pinned FFmpeg n8.0-23-gd1f31a829d.
- 2026-09-27, 15:20–16:16 EDT (main lanes) and 16:16–16:29 EDT (follow-up).
  Every timed run ran alone: no build, test or other lane alongside.
- **Ambient load (R26), unchanged.** The two `foot` screensaver processes
  used about 1.45 cores throughout (≈92% + 52% CPU at every BEGIN/END),
  with Hyprland at ≈24%. Load averages were 2.9–11.6. The seek lanes raise
  load themselves: they run decode threads.
- Commands, each `--exact --ignored --nocapture --test-threads=1` from
  `crates/kinewright-media`:
  - `mo2_perf_fixtures::r28_end_to_end_tracked` (LL; `R28_HARDWARE=1` for LH);
  - `mo2_perf_fixtures::blend_heavy_holds_floors_on_hardware` (G3, LH);
  - `pf1_harness::pf1_play_baseline` with `PF1_ONLY`/`PF1_RUNS` as in
    E11.9 (`PF1_HARDWARE=1` for LH);
  - `pf1_harness::pf1_seek_baseline`.
- **Follow-up run.** The main runner kept only lines starting `R28`/`PF1`.
  libtest prints `test … ...` without a newline, so each process's first
  `PF1` line was dropped: typical_1080p `run=0`, explainer_16x9 and
  seek_gop60 `run=0`. No run was lost to a failure; each process
  reported `test result: ok`. A follow-up with unfiltered output reran
  them in fresh processes on the same binary, and added the four Q-3
  controls on LH. seek_gop60 therefore has five runs per lane.

### E11.2 I4 (S0's 5% rule, `BASELINES` untouched)

*Superseded (R32): these lines are from `9101e18`, before the R32 fixes.
The reruns on the fixed binary are in E11.10.4, and they replace these
lines for every gate.*

| Build | LL mean ms | LL delta | LH mean ms | LH delta | Result |
|---|---|---|---|---|---|
| S1 end `217986a` (E10.2) | 77.03 / 76.23 / 75.95 | −84.7% | 72.87 / 73.00 / 73.69 | −85.2% | ok |
| S2a `9101e18` | 75.94 / 75.36 | −84.8% | 73.40 / 75.03 | −85.1% | ok |

One invocation per lane, which printed two runs. R28 drives
`FrameRenderer` directly, not the engine, so S2a's preview thread is not
on this path. Both lanes are within run-to-run noise of S1.

### E11.3 G3 (60 fps floor, LH)

*Superseded (R32): these lines are from `9101e18`, before the R32 fixes.
The reruns on the fixed binary are in E11.10.4, and they replace these
lines for every gate.*

| Build | `blend_heavy_1080p` fps (3 runs) | mean ms | p95 ms | Result |
|---|---|---|---|---|
| S1 end `217986a` (E10.3) | 72.1 / 71.4 / 71.7 | 13.86–14.00 | 15.47–15.67 | passes |
| S2a `9101e18` | 70.2 / 70.7 / 70.3 | 14.15–14.25 | 15.82–16.26 | **passes** |

- Other resident workloads: `typical_1080p` 72.0–72.8 fps (S1 72.1–73.1),
  `heavy_4k` 56.4–58.3 fps (S1 57.8–58.8). The slowdown control still fails
  its verdict (17.6 fps).
- **R28: the phases diagnostic now times the G-1 encode.**
  `phases::monitor` uses `monitor_rgba8` through the BT.709 table, as
  production does. Its `monitor_encode_ms` is 1.61–1.85 ms, where E10.3
  had ≈19–27 ms from the f32 per-pixel encode. `upload_ms` (9.9–13.8 ms)
  is still most of a resident frame (U-1, S3b).

### E11.4 G11, G16 and P-play (simulated driver)

*Superseded (R32): these lines are from `9101e18`, before the R32 fixes.
The reruns on the fixed binary are in E11.10.4, and they replace these
lines for every gate.*

G1 and G14 are S2b gates and still fail (`passes=false`); S2a does not
claim them. Every run is valid: 60.02 s (240.01–240.03 s for
`talk_recut`), 0 missed callbacks. Harness columns use the harness's own
rules (E9.9). Engine columns are R-5's `stats()` from the same run, read
before the pause.

| Workload | Lane | Runs | Harness on time / late / dropped | Engine on time / late / dropped | Present p50 / p95 / max ms | Held max ms (engine) | A/V offset at ack ms (engine) | Clock stall max ms, harness / engine | `underrun_frames` (events) | Rejected at consumer |
|---|---|---|---|---|---|---|---|---|---|---|
| `typical_1080p` | LL | 3 | 569–571 / 5–9 / 1220–1226 | 574–580 / 0 / 1221–1227 | 64.2–64.3 / 261.7–272.8 / 340.5–384.0 | 340.5–384.0 | 0.0 | 47.2–47.7 / 42.7–43.2 | 512 (1) | 0 |
| `blend_heavy_1080p` | LL | 3 | 653–656 / 3–7 / 1137–1144 | 656–663 / 0 / 1138–1145 | 64.4 / 192.5–193.0 / 256.1–277.5 | 256.1–277.5 | 0.0 | 46.8–47.1 / 42.6–42.9 | 512 (1) | 1 |
| `explainer_16x9` | LL | 1 | 1461 / 5 / 334 | 1466 / 0 / 335 | 42.3 / 106.4 / 212.8 | 212.8 | 0.0 | 46.5 / 42.6 | 512 (1) | 1 |
| `reel_9x16` | LL | 1 | 1328 / 4 / 468 | 1332 / 0 / 469 | 42.4 / 106.9 / 298.8 | 298.8 | 0.0 | 47.7 / 42.8 | 512 (1) | 1 |
| `feed_4x5` | LL | 1 | 985 / 4 / 811 | 989 / 0 / 812 | 42.8 / 170.7 / 1088.0 | 1088.0 | 0.0 | 48.9 / 42.9 | 512 (1) | 1 |
| `talk_recut` | LL | 1 | 6460 / 22 / 718 of 7200 | 6482 / 0 / 719 | 42.0 / 66.2 / 235.2 | 235.2 | 0.0 | 60.0 / 42.8 | **0 (0)** | 0 |
| `typical_1080p` | LH | 3 | 575–596 / 10–11 / 1194–1214 | 586–606 / 0 / 1195–1215 | 64.2 / 251.5–259.0 / 361.3–384.6 | 361.3–384.6 | 0.0 | 45.8–46.1 / 42.3–42.8 | 512 (1) | 0 |
| `blend_heavy_1080p` | LH | 3 | 776–785 / 6–9 / 1006–1018 | 782–794 / 0 / 1007–1019 | 63.9–64.0 / 170.3–171.1 / 234.8–256.3 | 234.8–256.3 | 0.0 | 45.7–47.3 / 42.4–42.6 | 512 (1) | 0–1 |
| `explainer_16x9` | LH | 1 | 1476 / 9 / 315 | 1485 / 0 / 316 | 42.1 / 106.2 / 234.5 | 234.5 | 0.0 | 45.5 / 42.9 | 512 (1) | 1 |
| `reel_9x16` | LH | 1 | 1404 / 7 / 389 | 1411 / 0 / 390 | 42.2 / 106.7 / 298.3 | 298.3 | 0.0 | 45.8 / 42.6 | 512 (1) | 1 |
| `feed_4x5` | LH | 1 | 1221 / 2 / 577 | 1223 / 0 / 578 | 42.4 / 128.2 / 725.4 | 725.4 | 0.0 | 45.5 / 42.4 | 512 (1) | 1 |
| `talk_recut` | LH | 1 | 6446 / 13 / 741 of 7200 | 6459 / 0 / 742 | 42.0 / 71.1 / 256.4 | 256.4 | 0.0 | 45.7 / 42.9 | **0 (0)** | 1 |

Harness due is 1,800 (7,200 for `talk_recut`). The engine's due was
1,801 (7,201) at `9101e18`; see the end-frame note below.

**G16 passes on both lanes.** The harness's max clock stall is 45.5–60.0 ms
in every run, against the 100 ms gate. The clock-freeze control fails it
on both lanes, as it must (1050.0 ms; the engine sees 1043.9 / 1044.7 ms).

**G11: 0 underruns inside the programme; the counter reads 512.** Every
60 s run records exactly one underrun event of 512 frames, on both lanes,
in every workload and control. S1 had the same 512 (E10.5). It is the
last callback straddling the endpoint:
- 60 s at 48 kHz is 2,880,000 frames, and 2,880,000 mod 1,024 = 512. The
  final 1,024-frame callback pops the ring's last 512 programme frames
  and fails the other 512, which lie past the duration. The harness
  snapshot includes "at most the one [callback] after" the endpoint
  (E9.7). The failed half is in that callback, and its failed pops are
  recorded before the clock reaches the duration.
- The control is `talk_recut`: 240 s is 11,520,000 frames, exactly 11,250
  callbacks. It shows `underrun_frames=0` and 0 events on both lanes, with
  no other change.

So `blend_heavy_1080p` on LL (and every other workload) has no underrun
inside its 60 s. Read literally, though, the gate's counter is 512, not 0.
**Proposed amendment:** count only failed pops before the duration. The
alternative is to state G11 as "0 underruns before the endpoint" and have
the harness report the straddle apart. The stall control still fails
(48,640 frames in 48 events on both lanes).

Notes:
- **S2a against S1 (E10.5, `typical_1080p`).**
  - A/V offset at ack falls from 200.0 ms to 0.0: R-2 binds only the
    clock's own frame while playing.
  - That rule shows fewer frames. On LL, S1 presented 1,090 of 1,800
    (450 on time, 640 late, up to 200 ms off); S2a presents 569–580,
    all on time by R-5.
  - Held max rises from 170.9 to 340–384 ms, and present p50 from 33.6 to
    64.2 ms.
  - Why: a `typical_1080p` preview render takes about 64 ms here (decode
    included). The lead is capped at 2 frames (66.7 ms, R-3), so a render
    that overruns it lands past its frame and is dropped by R-3's rule.
  - This is the design's intended trade. S2b's decode pipeline is what
    G1/G14 depend on.
  - The lighter workloads (`explainer_16x9`, `reel_9x16`, `talk_recut`)
    present 74–90% of due frames on time, with p50 ≈ 42 ms.
- **The engine's late is 0; the harness's late is 2–22.** R-5 allows due +
  1 frame + 16.7 ms (one root epoch). The harness allows due + 1 frame from
  its own 5 ms sample. Engine on time = harness on time + late in every
  run.
- **The harness's `av_offset_max_ms` of 1933–1967 ms is not the at-ack
  offset.** The harness computes it over every arrival, rejected ones
  included (unchanged from S0, when there were no acks). It appears
  exactly in the runs with `consumer_rejected=1`. The offset is 58–59
  frames, which matches the warm-up's 2 s: the likely source is the
  warm-up pause's resting render, published after the measured `play(0)`
  and rejected by R-2 (older epoch). The at-ack figure is the engine's,
  0.0 in every run. The harness field is left as a receipt-side diagnostic.
- **R-5 end-frame fix (`58e2f69`).** At `9101e18` the engine counted the
  clock's arrival at the duration as a due frame (1,801 of 1,800), so each
  run had one spurious drop. `Counters::begin` now takes the programme's end
  frame. `due_frames_are_counted_once_by_their_ack` samples at the end and
  fails without the bound. The timing lines predate it.
- **The engine's clock stall is blind while the worker is blocked.** In the
  stall control the harness sees 1045.0 ms, the engine 42.6 / 42.9 ms.
  - R-5 samples at each worker tick. The stall fault sleeps inside `tick`,
    so no sample falls inside the frozen span.
  - When the worker resumes, the clock has moved (the ring held 1 s), so
    the span is lost.
  - The clock-freeze control, which stops the clock with the worker
    running, is seen (1043.9 ms).
  - A real fill starvation would block the same thread, so the engine
    would under-report that stall.
  - G16 is judged on the harness's independent sampler, so it is not
    affected. **Proposed amendment:** record clock progress where it
    changes, in `render_output`'s release advance, e.g. the longest gap
    between advancing callbacks in `AudioDiagnostics`, rather than by
    worker sampling.
- **Peak RSS** in a fresh process: `typical_1080p` 852.4 MiB LL and
  903.2 MiB LH (S1 860.3 / 898.6). Later runs in the same process read
  higher (up to 1,803 MiB). P-rss is measured one process per workload
  at S4.
- **Exit-time panic.** After `test result: ok`, the `play_LL_main` and
  `seek_LH` processes printed `thread 'kinewright-preview' panicked at
  khronos-egl-6.0.0/src/lib.rs:841`. It is the teardown seen in E10.5 (S1,
  on the worker thread), now on the thread that owns the renderer.
  - The engine does not join its worker on drop (unchanged from the base),
    so the worker's H-6 `preview.join()` can run into process exit.
  - It is after the measurement, and `dropping_the_engine_stops_the_preview_thread`
    covers the join itself.
  - Joining the worker in the engine's `Drop` is left as a proposal: the
    base never joined it, and that would block the app's shutdown on a
    render.

### E11.5 L-6 and P-seek

*Superseded (R32): these lines are from `9101e18`, before the R32 fixes.
The reruns on the fixed binary are in E11.10.4, and they replace these
lines for every gate.*

Paused, as E9.6. Latency columns show the worst run, distinct fps the
smallest, and counts and release ms as ranges.

| Workload | Lane | Runs | Random p95 / max | Forward p95 | +1 p95 | Backward p95 | Drag p95 / answered | Unanswered | Distinct fps | Pending | Release shown / ms (L-6) | Stale / over / valid over release | Timeouts |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `seek_gop60` | LL | 5 | 64.9 / 77.4 | 63.8 | 69.7 | 64.5 | 106.8 / 106.8 | 0 | 23.2 | 1–2 | all / 46.2–87.2 | 1 / 0 / 0 | 0 |
| `talk_recut` | LL | 3 | 109.7 / 164.4 | 109.7 | 138.6 | 94.2 | 146.2 / 144.4 | 0–1 | 18.2 | 1–3 | all / 73.7–87.6 | 1 / 0 / 0 | 0 |
| `explainer_16x9` | LL | 3 | 140.5 / 182.1 | 130.3 | 137.9 | 134.6 | 253.1 / 253.1 | 0–1 | 13.6 | 1–3 | all / 73.0–135.4 | 1 / 0 / 0 | 0 |
| `seek_gop60` | LH | 5 | 62.5 / 71.5 | 65.7 | 70.6 | 60.1 | 101.6 / 101.6 | 0 | 24.0 | 1–2 | all / 41.4–88.4 | 1 / 0 / 0 | 0 |
| `talk_recut` | LH | 3 | 105.5 / 161.4 | 103.9 | 130.4 | 92.0 | 131.9 / 131.9 | 0–1 | 19.0 | 1–2 | all / 74.7–131.2 | 1 / 0 / 0 | 0 |
| `explainer_16x9` | LH | 3 | 138.2 / 179.9 | 128.7 | 137.4 | 132.7 | 234.2 / 234.2 | 0 | 14.2 | 1–2 | all / 90.9–175.1 | 1 / 0 / 0 | 0 |

- **L-6 passes:**
  - the release target is shown in every run on both lanes, and
    `valid_frames_over_release=0`;
  - the one stale frame in each run is an older-epoch drag frame
    delivered after the release call, which R-2 rejects.
- Against S0 (E9.6), S1's conversion work shows here:
  - random p95 at GOP 60 falls from 117.5 to 64.9 ms;
  - release from 86.1–170.6 to 46.2–87.2 ms;
  - drag distinct fps rises from 10.2 to 23.2 (LL).
- G8's latency gates (L-1 ≤ 40, L-3 ≤ 20, …) are S2c's and still fail.

### E11.6 CI gates (G10, I8, I18, I13, V-3; S1's I1, I3, I9, C-5, C-3)

All pass in `cargo test -p kinewright-media` and `-p kinewright-app` at
every S2a commit from the one that introduced them. The raw lines at
`58e2f69` are in E11.9. The full `cargo test --workspace` at `9101e18`
passed: 36 test binaries, 3,395 passed, 0 failed, 41 ignored.
`cargo fmt -- --check` was clean.
- **G10** `engine::tests::g10_video_tracks_the_clock_within_five_seconds_of_a_fill_stall`
  (stepped): 47 callbacks, then a 2 s fill stall (94 callbacks, no worker
  tick), then recovery. It asserts that underruns register, the clock
  freezes, a frame is published before recovery, and every published
  frame in the tail sits at the clock's own frame (offset 0 ≤ 33 ms)
  within 5 s.
- **V-3** `pf1_harness::pf1_the_fill_runs_while_the_preview_renders`: a
  5 s preview render delay, 150 paced callbacks, 0 underruns and the clock
  ≥ 95 frames. The worker no longer renders.
- **I8** `engine::tests::i8_media_0`…`_7` (1,000 seeded interleavings of
  seeks, frame requests, play/pause, stamped failures and a scripted play
  segment):
  - no old-epoch or expired frame is published;
  - no superseded error stops playback;
  - the release target is shown.

  *Correction (R32, review B):* these shards stepped the worker's and the
  preview's methods on one thread; they did not run the real worker and
  preview threads. I8 was rewritten in R32 (E11.10.2); the real-thread
  witnesses are listed there.

  Other I8 witnesses:
  - `only_a_current_stamped_failure_stops_playback`;
  - `a_request_waits_for_the_controls_issued_before_it`;
  - `preview::tests::the_paused_slot_keeps_the_newest_target_and_renders_the_last`;
  - the app model `presenter::tests::seeded_interleavings_bind_and_ack_only_current_frames`
    (1,000 seeds) and `playback_binds_only_the_clocks_frame`.
- **I18** covers A-layout, B-bind, C-deferred, seek-before-paint and a
  stale cell:
  - `presenter::tests::the_marker_marks_what_is_bound_when_it_paints`
    covers those paths.
  - `a_marker_that_never_paints_is_never_acked` covers the rest.
    Abandoned, zero-clip and discarded-pass paints all reach the marker
    as a `paint` that never runs.
  - One ack per `frame_id` is checked in the marker witness and the
    seeded model.
- **I13:**
  - `preview::tests::the_agent_lane_is_fifo_bounded_and_replies_exactly_once`;
  - `engine::tests::a_full_agent_queue_is_refused_without_waiting`;
  - `preview::tests::a_cancelled_agent_job_is_discarded_unanswered`;
  - `preview::tests::agent_jobs_take_turns_with_transport_attempts`
    (playback holds; *correction (R32):* this is not a paused-wait
    suspension witness. S2a has no paused `FrameWait`: a paused job renders
    at once, so there is no paused wait to suspend. The paused
    `FrameWait` arrives with S2b's decode pipeline);
  - `preview::tests::shutdown_answers_every_agent_job_exactly_once`;
  - `preview::tests::the_preview_thread_leaves_a_playback_hold_at_shutdown`;
  - `engine::tests::dropping_the_engine_stops_the_preview_thread` (H-6).
- **R-5 counters:** `stats::tests::due_frames_are_counted_once_by_their_ack`.
- **S1's items, unchanged and green:**
  - I1 (`input_tables_match_every_accepted_descriptor`,
    `fused_table_fill_…`, `monitor_table_equals_…`, `rgba64_decode_…`);
  - I3 (`r28_ledger_holds_the_ceilings_and_releases_every_charge`, its
    control);
  - I9 (`a_drained_one_frame_timeline_…`, `a_long_programme_…`);
  - G9, G12, G5;
  - C-5: `render.rs`, `conversion.rs`, `frame.rs` and `decode.rs` are
    untouched by S2a. `compositor.rs` changes only in `phases::monitor`,
    a diagnostic. The byte-exact suites are unedited and green;
  - C-3: the paint marker only records, and draws nothing. The program
    viewer's pixels change only when the bound image is stale (the
    "STALE" caption, E11.8);
  - `preview::tests::only_the_live_monitor_encodes_through_the_table`.

### E11.7 Broken variants (race and kill tests)

Each variant was applied to `9101e18`, then the affected suites were run
and the variant reverted. The tree was verified clean after each.

| Variant | Break | Witnesses that fail |
|---|---|---|
| M1 | R-2: every stamped failure stops playback | `only_a_current_stamped_failure_stops_playback`, all 8 `i8_media` shards |
| M2 | R-1: a request does not wait for earlier controls | `a_request_waits_for_the_controls_issued_before_it` (the I8 shards stay green) |
| M3 | R-3: playback publishes early (position + 1 ≥ target) | all 8 I8 shards, G10, `agent_jobs_take_turns_with_transport_attempts` |
| M4 | H-6: a playback hold ignores shutdown and supersession | `the_preview_thread_leaves_a_playback_hold_at_shutdown` |
| M5 | S-1: the slot keeps the oldest pending job | `a_request_waits…`, `i8_media_0`…`_6`, `the_paused_slot_keeps_the_newest_target_and_renders_the_last`, `agent_jobs_take_turns…`; the run then hung to its 900 s timeout |
| M6 | H-6/R-4: shutdown leaves queued agent jobs unanswered | `a_full_agent_queue_is_refused_without_waiting`, `shutdown_answers_every_agent_job_exactly_once` |
| M7 | R-4: a cancelled job is not discarded | `a_cancelled_agent_job_is_discarded_unanswered` |
| M8 | V-3/H-4: a render holds the counters leaf (the worker waits on rendering) | `pf1_the_fill_runs_while_the_preview_renders` |
| A1 | R-5: the marker copies the cell at layout | `the_marker_marks_what_is_bound_when_it_paints` |
| A2 | R-5: no seek-before-paint check | the marker witness, the seeded app model |
| A3 | R-5: acked on every paint, not once per `frame_id` | the marker witness, the seeded app model |
| A4 | R-5: no stale check at paint | none — redundant: `stale` is set only when the cell's epoch is older than latest, which the paint's epoch check already rejects |
| A5 | R-2: `finalize` binds any epoch | the seeded app model, `playback_binds_only_the_clocks_frame` |
| A6 | R-2: playback binds expired frames | the seeded app model |
| E1 (`58e2f69`) | R-5: due frames counted to the duration inclusive | `due_frames_are_counted_once_by_their_ack` |

### E11.8 Deviations and notes

These are interpretations of the design. Each is also listed in the S2a
report.
- **Frame requests while playing** re-post the `Playback` job with the
  newest stamp.
- **Resting renders:** pause and a failed play post a clamped resting
  `Paused` render. Paused frames are not clamped otherwise.
- **Job stamps (withdrawn in R32).** At `9101e18` a job carried the newest
  applied stamp. That relabelled an old request with a newer document's
  stamp, which R-1 forbids (reviews A/B F1). Since R32 a job carries its
  own request's stamp. Controls apply in issuance order, and a request
  issued before an applied control is superseded, never relabelled. One
  case folds: a seek issued before a `Pause` sets where the pause rests,
  and that resting image is posted under the pause's own stamp.
- **Lead:** at least 1 frame.
- **LUT rebinds** update the pending job in place.
- **Cache commands:** the `Control` cache commands are merged into one
  with a `Result` reply.
- **Agent deadline** during a playback hold: (lead + 1) frames.
- **Test-only faults:** `fake_render` and `step_hold`.
- **Stats** restart at each explicit `play`; a seek while playing
  re-anchors them.
- **R-5 windows (withdrawn in R32; amendment).**
  - At `9101e18`, an ack more than 2 s late (`DUE_WINDOW`) left the frame
    dropped. The "+ 1 epoch" term was a fixed 16.7 ms. Both are removed.
  - Each ack now carries its paint instant: the app's marker records it,
    and the harness uses the receipt instant. The frame is on time iff it
    was painted by due + 1 frame, and late otherwise.
  - This is R-5's "acked within due + 1 frame + 1 epoch", with each ack's
    actual epoch. An ack arrives at paint + its own gap e, so "acked ≤ due
    + 1 frame + e" is "painted ≤ due + 1 frame". The fixed 16.7 ms
    guessed at e.
  - **Amendment:** pending due records are bounded by count, not by age.
    The bound is `DUE_RECORDS` = 65,536: over 18 minutes of unacknowledged
    frames at 60 fps, at most about 3 MB.
  - Only past that bound is the oldest record evicted. An evicted frame
    acknowledged later still counts late, never dropped.
  - Since R33 (re-review A D4 / B D5), eviction keeps identity. Each
    epoch keeps its evicted frame range and the ranges already settled.
    An evicted frame counts late on its first ack only. A duplicate ack,
    a key that was never due, or a key from another epoch counts nothing.
  - Records outlive a stop, so an ack after EOS counts. Only an explicit
    `play` clears them.
  - **Amendment (R34, re-review 2 D3):** the eviction metadata is capped
    too. At most 8 epochs (`EVICTED_EPOCHS`) keep evicted identity; an
    older epoch is forgotten. At most 64 settled ranges
    (`SETTLED_RANGES`) are kept per epoch; past that the two oldest
    merge.
    - Approximation: a forgotten epoch's evicted frames, and the frames
      inside a merged gap, stay dropped; a later ack of one is
      unmatched. An evicted frame charged to an agent job stays charged.
    - Worst case: 65,536 records, 8 × 64 ranges, 256 held acks and 256
      in the channel, a few MB.
- **Single-writer accounting (R34 amendment, re-review 2 D1–D4;
  supersedes R33's ack-time snapshot, `cb36a53`, now deleted).** The
  amendment text is in the design's §7 R-5.
  - The worker is the only writer of due-frame outcomes. It registers due
    frames from its own applied runtime: the frame of the samples its
    audio callback consumed (`Worker::runtime_position`). A caller's
    `seek`/`play`/`set_document` writes the shared clock but never those
    samples, and neither does a worker transition, so no clock write can
    label playback that did not happen (D1).
  - Each transition ends the outgoing epoch through that runtime
    position first (D2). *R35 (re-review 3 D1, D2):* the transition stops
    the outgoing stream before it reads the position (`Worker::quiesce`:
    dropping the runtime joins its callback thread), so a callback that
    runs meanwhile is counted, and reads it at the stream's own rate and
    the outgoing document's fps. This covers the terminal stop, a pause, a
    new document, a playing seek and a `play`. The terminal stop closes at
    that consumed frame clamped to the duration, never where the terminal
    check's caller-visible clock was; its user-visible behaviour is S1's,
    unchanged. An explicit `play` then clears the counters, as before.
  - *R35 (re-review 3 D3):* `ack_presented` hands (epoch, frame, paint,
    expired) to the worker on the lane's bounded channel (256) with
    `try_send`, taking no lock (the R34 push took the counters' lock,
    which the worker holds through registration and settlement). A full
    channel drops the newest ack (`acks_overflowed`). The worker moves the
    channel into its counters when it takes them (`Lane::settle`), and
    holds at most 256 acks ahead of its registrations, again dropping the
    newest.
  - The worker drains them each tick (it now ticks while stopped, too)
    and at each transition. An ack ahead of the registrations in the
    open epoch is held until a registration reaches its frame. It is
    unmatched (`acks_unmatched`) if its epoch closes first, like a
    duplicate or a frame never due.
  - Agent attribution (D4): a due frame registered in an agent job's
    epoch while the job runs, past the newest frame the preview had
    published, is charged to `dropped_agent`. A later paint of it
    removes the charge, so `dropped_agent` counts job-window frames never
    painted.
  - The A/V offset is now taken at paint: the whole frames the paint
    trailed its frame's stored due instant, at least one if `expired`.
    The ack no longer reads the clock.
  - Due instants are the registering tick's. The tick's 5 ms is a
    receive timeout, not a bound (R35, re-review 3 D4): worker fills,
    control processing and scheduling can delay a tick while the
    callbacks advance. A paint before its frame's registration waits and
    settles against the later instant, so it reads on time and zero
    elapsed; one already two frames expired then reads one frame (from
    `expired`). The offset is therefore a proxy from stored timestamps,
    which can under-report, not a guaranteed consumed-clock offset.
- **Final check and binding: the residual window (R33 amendment,
  re-review A D1).**
  - `Presenter::finalize` re-checks the stamp and the clock after it
    prepares the image. It then writes the cell and calls
    `texture.set`.
  - The check and the binding cannot be one atomic step:
    - the audio callback advances the clock on its own thread;
    - `texture.set` takes egui's texture-manager lock.
  - So a window remains between the last check and the binding. A frame
    that was current at the check can be bound after the clock has
    passed it.
  - **Amendment:** eligibility is judged again at paint.
    - The paint marker reads the clock in the paint callback: the stamp,
      and the position while playing.
    - A frame the clock has passed is acked `expired`
      (`Playback::ack_presented`'s new flag), and R-5 counts it late,
      never on time.
    - The image is still drawn for that paint; the next pass's finalize
      replaces it.
  - R-2's "an expired frame is never bound" therefore holds up to this
    window. Within it, "an expired binding is never counted on time" is
    what R-5 enforces.
  - The window between the paint callback and the actual scan-out is
    inherent to any paint-time judgment, and is not claimed.
  - Witness: `presenter::tests::a_frame_expiring_after_its_last_check_is_acked_expired`.
    The clock passes frame 10 after the last check, in two ways: before
    the binding is written, and before the paint.
- **Held age (R32 reconciliation with R-5; qualified in R33).**
  - R-5 samples it every 5 ms (harness) or at each `App::logic` (app).
  - The engine samples it on its worker tick (nominally every 5 ms) for
    any consumer, and at every settled ack (R34: the worker settles acks
    on its tick).
    - While the worker keeps its tick, this is denser than
      `App::logic`. It is not while the worker itself stalls.
    - Each settled ack still records the whole gap from the previous
      paint to its own. A stalled worker therefore delays the record of
      an ongoing age, but does not lose a gap that ends in a paint.
    - The app has no sampler of its own.
  - The age runs from the paint instant of the newest painted frame,
    carried in the ack, not from the ack's arrival. The ack's one-epoch
    delay therefore does not add to the held age.
  - Between a paint and its settlement, samples still count from the
    previous paint. The maximum can therefore over-report by at most that
    gap (one epoch plus one tick), and never under-reports.
- **App candidates:** the app keeps at most 4, in publication order.
- **The "STALE" caption** (`STATUS_WARNING`; since R32 the viewer
  reserves a slot and `finalize_preview` fills it, so the caption
  describes the finalized binding, review A nit) marks a stale image in the
  program viewer.
- **Budget:** 2,272 non-blank, non-comment `.rs` lines added and 393
  removed across the six commits, against ~1,150. By crate: core +82,
  app +403, media +1,766, agent +11, project +10. About 800 of the added
  lines are tests:
  - `preview.rs` 261;
  - `engine.rs` 311;
  - `presenter.rs` 173;
  - `stats.rs` 30;
  - the harness.

### E11.9 Raw result lines

*Superseded (R32): these raw lines are from `9101e18`. The R32 raw
lines are in E11.10.6.*

I4 (`r28_end_to_end_tracked`, 9101e18):

```
# LL
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=false workload=typical_1080p run=1 dims=(1280, 720) mean_ms=75.94 fps=13.2 p95_ms=204.09 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=false workload=typical_1080p run=2 dims=(1280, 720) mean_ms=75.36 fps=13.3 p95_ms=205.30 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) workload=typical_1080p baseline_ms=497.86 delta=-84.8%
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 926 filtered out; finished in 77.95s
# LH (R28_HARDWARE=1)
R28 adapter=NVIDIA GeForce RTX 3090 resident=false workload=typical_1080p run=1 dims=(1280, 720) mean_ms=73.40 fps=13.6 p95_ms=210.73 ledger_peak_mib=56.3 validate_us=1.7
R28 adapter=NVIDIA GeForce RTX 3090 resident=false workload=typical_1080p run=2 dims=(1280, 720) mean_ms=75.03 fps=13.3 p95_ms=209.48 ledger_peak_mib=56.3 validate_us=2.3
R28 adapter=NVIDIA GeForce RTX 3090 workload=typical_1080p baseline_ms=494.39 delta=-85.1%
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 926 filtered out; finished in 76.32s
```

G3 (`blend_heavy_holds_floors_on_hardware`, LH):

```
# LH
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=typical_1080p run=1 dims=(1280, 720) mean_ms=13.73 fps=72.8 p95_ms=15.39 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=typical_1080p run=2 dims=(1280, 720) mean_ms=13.89 fps=72.0 p95_ms=15.63 ledger_peak_mib=56.3 validate_us=1.3
R28 phases workload=typical_1080p upload_ms=9.87 gpu_passes_readback_ms=1.25 monitor_encode_ms=1.61
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=blend_heavy_1080p run=0 dims=(1280, 720) mean_ms=14.25 fps=70.2 p95_ms=16.26 ledger_peak_mib=63.3 validate_us=1.1
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=blend_heavy_1080p run=1 dims=(1280, 720) mean_ms=14.15 fps=70.7 p95_ms=15.82 ledger_peak_mib=63.3 validate_us=1.6
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=blend_heavy_1080p run=2 dims=(1280, 720) mean_ms=14.22 fps=70.3 p95_ms=15.85 ledger_peak_mib=63.3 validate_us=1.2
R28 phases workload=blend_heavy_1080p upload_ms=10.48 gpu_passes_readback_ms=1.51 monitor_encode_ms=1.85
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=heavy_4k run=0 dims=(1280, 720) mean_ms=17.73 fps=56.4 p95_ms=19.47 ledger_peak_mib=70.3 validate_us=9.0
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=heavy_4k run=1 dims=(1280, 720) mean_ms=17.42 fps=57.4 p95_ms=19.48 ledger_peak_mib=70.3 validate_us=11.4
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=heavy_4k run=2 dims=(1280, 720) mean_ms=17.16 fps=58.3 p95_ms=18.55 ledger_peak_mib=70.3 validate_us=9.3
R28 phases workload=heavy_4k upload_ms=13.79 gpu_passes_readback_ms=1.46 monitor_encode_ms=1.67
R28 control=slowdown delay_ms=41.7 mean_ms=56.82 p95_ms=59.90 verdict=Err("17.6 fps (floor 60), p95 59.9 ms vs mean 56.8 ms")
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 926 filtered out; finished in 159.09s
```

P-play (`pf1_play_baseline`). The main lanes, then the follow-up (fresh processes) that recovers each process's first line and adds the LH controls:

```
# LL, PF1_ONLY=typical_1080p,blend_heavy_1080p,controls PF1_RUNS=3 (typical run=0 lost to the filter)
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=typical_1080p run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=569 late=5 early=0 dropped=1226 present_p50_ms=64.3 present_p95_ms=267.0 present_max_ms=362.1 held_max_ms=359.6 av_offset_max_ms=0.0 clock_stall_max_ms=47.2 underrun_frames=512 drain_underrun_frames=0 peak_rss_mib=972.1 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1801 engine_on_time=574 engine_late=0 engine_dropped=1227 engine_dropped_agent=0 engine_held_max_ms=362.1 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.7 engine_underrun_events=1 engine_underrun_frames=512 engine_stale_errors=0 consumer_rejected=0
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=typical_1080p run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=569 late=7 early=0 dropped=1224 present_p50_ms=64.2 present_p95_ms=261.7 present_max_ms=384.0 held_max_ms=382.3 av_offset_max_ms=0.0 clock_stall_max_ms=47.7 underrun_frames=512 drain_underrun_frames=0 peak_rss_mib=1053.8 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1801 engine_on_time=576 engine_late=0 engine_dropped=1225 engine_dropped_agent=0 engine_held_max_ms=384.0 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.7 engine_underrun_events=1 engine_underrun_frames=512 engine_stale_errors=0 consumer_rejected=0
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=blend_heavy_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=654 late=7 early=0 dropped=1139 present_p50_ms=64.4 present_p95_ms=193.0 present_max_ms=277.5 held_max_ms=274.3 av_offset_max_ms=1966.7 clock_stall_max_ms=47.1 underrun_frames=512 drain_underrun_frames=0 peak_rss_mib=1198.3 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1801 engine_on_time=661 engine_late=0 engine_dropped=1140 engine_dropped_agent=0 engine_held_max_ms=277.5 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.8 engine_underrun_events=1 engine_underrun_frames=512 engine_stale_errors=0 consumer_rejected=1
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=blend_heavy_1080p run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=656 late=7 early=0 dropped=1137 present_p50_ms=64.4 present_p95_ms=192.7 present_max_ms=256.1 held_max_ms=255.8 av_offset_max_ms=1966.7 clock_stall_max_ms=46.9 underrun_frames=512 drain_underrun_frames=0 peak_rss_mib=1337.4 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1801 engine_on_time=663 engine_late=0 engine_dropped=1138 engine_dropped_agent=0 engine_held_max_ms=256.1 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.6 engine_underrun_events=1 engine_underrun_frames=512 engine_stale_errors=0 consumer_rejected=1
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=blend_heavy_1080p run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=653 late=3 early=0 dropped=1144 present_p50_ms=64.4 present_p95_ms=192.5 present_max_ms=277.5 held_max_ms=275.0 av_offset_max_ms=1966.7 clock_stall_max_ms=46.8 underrun_frames=512 drain_underrun_frames=0 peak_rss_mib=1431.9 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1801 engine_on_time=656 engine_late=0 engine_dropped=1145 engine_dropped_agent=0 engine_held_max_ms=277.5 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.9 engine_underrun_events=1 engine_underrun_frames=512 engine_stale_errors=0 consumer_rejected=1
PF1 control=Slowdown lane=LL workload=typical_1080p metric=G1 fails=true valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=130 late=25 early=0 dropped=1645 present_p50_ms=405.8 present_p95_ms=1021.2 present_max_ms=1502.7 held_max_ms=1499.7 av_offset_max_ms=0.0 clock_stall_max_ms=47.6 underrun_frames=512 drain_underrun_frames=0 peak_rss_mib=1148.0 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1801 engine_on_time=155 engine_late=0 engine_dropped=1646 engine_dropped_agent=0 engine_held_max_ms=1502.7 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.2 engine_underrun_events=1 engine_underrun_frames=512 engine_stale_errors=0 consumer_rejected=0
PF1 control=Freeze lane=LL workload=typical_1080p metric=G14 fails=true valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=569 late=9 early=0 dropped=1222 present_p50_ms=64.2 present_p95_ms=261.5 present_max_ms=1280.6 held_max_ms=1276.5 av_offset_max_ms=0.0 clock_stall_max_ms=47.3 underrun_frames=512 drain_underrun_frames=0 peak_rss_mib=1166.4 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1801 engine_on_time=578 engine_late=0 engine_dropped=1223 engine_dropped_agent=0 engine_held_max_ms=1280.6 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.9 engine_underrun_events=1 engine_underrun_frames=512 engine_stale_errors=0 consumer_rejected=0
PF1 control=ClockFreeze lane=LL workload=typical_1080p metric=G16 fails=true valid=true elapsed_s=61.02 missed_callbacks=0 due=1800 on_time=584 late=3 early=0 dropped=1213 present_p50_ms=64.2 present_p95_ms=257.4 present_max_ms=1090.8 held_max_ms=1086.5 av_offset_max_ms=0.0 clock_stall_max_ms=1050.0 underrun_frames=512 drain_underrun_frames=0 peak_rss_mib=1092.7 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1801 engine_on_time=587 engine_late=0 engine_dropped=1214 engine_dropped_agent=0 engine_held_max_ms=1090.8 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=1043.9 engine_underrun_events=1 engine_underrun_frames=512 engine_stale_errors=0 consumer_rejected=0
PF1 control=Stall lane=LL workload=typical_1080p metric=underrun_frames fails=true valid=true elapsed_s=61.02 missed_callbacks=0 due=1800 on_time=582 late=5 early=0 dropped=1213 present_p50_ms=64.2 present_p95_ms=254.5 present_max_ms=1066.2 held_max_ms=1064.7 av_offset_max_ms=0.0 clock_stall_max_ms=1045.0 underrun_frames=48640 drain_underrun_frames=0 peak_rss_mib=1182.5 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1801 engine_on_time=587 engine_late=0 engine_dropped=1214 engine_dropped_agent=0 engine_held_max_ms=1066.2 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.9 engine_underrun_events=48 engine_underrun_frames=48640 engine_stale_errors=0 consumer_rejected=0
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 926 filtered out; finished in 646.22s
thread 'kinewright-preview' (587520) panicked at /home/riels/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/khronos-egl-6.0.0/src/lib.rs:841:27:
# LH, PF1_ONLY=typical_1080p,blend_heavy_1080p PF1_RUNS=3 (typical run=0 lost to the filter)
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=typical_1080p run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=589 late=10 early=0 dropped=1201 present_p50_ms=64.2 present_p95_ms=255.4 present_max_ms=361.3 held_max_ms=357.6 av_offset_max_ms=0.0 clock_stall_max_ms=45.8 underrun_frames=512 drain_underrun_frames=0 peak_rss_mib=1011.0 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1801 engine_on_time=599 engine_late=0 engine_dropped=1202 engine_dropped_agent=0 engine_held_max_ms=361.3 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.8 engine_underrun_events=1 engine_underrun_frames=512 engine_stale_errors=0 consumer_rejected=0
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=typical_1080p run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=575 late=11 early=0 dropped=1214 present_p50_ms=64.2 present_p95_ms=259.0 present_max_ms=384.6 held_max_ms=380.3 av_offset_max_ms=0.0 clock_stall_max_ms=46.1 underrun_frames=512 drain_underrun_frames=0 peak_rss_mib=1137.5 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1801 engine_on_time=586 engine_late=0 engine_dropped=1215 engine_dropped_agent=0 engine_held_max_ms=384.6 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.3 engine_underrun_events=1 engine_underrun_frames=512 engine_stale_errors=0 consumer_rejected=0
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=blend_heavy_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=776 late=6 early=0 dropped=1018 present_p50_ms=64.0 present_p95_ms=170.3 present_max_ms=234.8 held_max_ms=233.6 av_offset_max_ms=0.0 clock_stall_max_ms=45.7 underrun_frames=512 drain_underrun_frames=0 peak_rss_mib=1311.9 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1801 engine_on_time=782 engine_late=0 engine_dropped=1019 engine_dropped_agent=0 engine_held_max_ms=234.8 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.4 engine_underrun_events=1 engine_underrun_frames=512 engine_stale_errors=0 consumer_rejected=0
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=blend_heavy_1080p run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=782 late=9 early=0 dropped=1009 present_p50_ms=63.9 present_p95_ms=171.1 present_max_ms=235.0 held_max_ms=234.5 av_offset_max_ms=1966.7 clock_stall_max_ms=47.3 underrun_frames=512 drain_underrun_frames=0 peak_rss_mib=1503.8 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1801 engine_on_time=791 engine_late=0 engine_dropped=1010 engine_dropped_agent=0 engine_held_max_ms=235.0 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.4 engine_underrun_events=1 engine_underrun_frames=512 engine_stale_errors=0 consumer_rejected=1
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=blend_heavy_1080p run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=785 late=9 early=0 dropped=1006 present_p50_ms=63.9 present_p95_ms=170.9 present_max_ms=256.3 held_max_ms=253.9 av_offset_max_ms=0.0 clock_stall_max_ms=45.8 underrun_frames=512 drain_underrun_frames=0 peak_rss_mib=1602.6 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1801 engine_on_time=794 engine_late=0 engine_dropped=1007 engine_dropped_agent=0 engine_held_max_ms=256.3 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.6 engine_underrun_events=1 engine_underrun_frames=512 engine_stale_errors=0 consumer_rejected=0
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 926 filtered out; finished in 390.60s
# LL, PF1_ONLY=explainer_16x9,reel_9x16,feed_4x5,talk_recut PF1_RUNS=1 (explainer lost to the filter)
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=reel_9x16 run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1328 late=4 early=0 dropped=468 present_p50_ms=42.4 present_p95_ms=106.9 present_max_ms=298.8 held_max_ms=298.2 av_offset_max_ms=1966.7 clock_stall_max_ms=47.7 underrun_frames=512 drain_underrun_frames=0 peak_rss_mib=1526.7 ledger_peak_mib=68.4 table_live_kib=128 passes=false engine_due=1801 engine_on_time=1332 engine_late=0 engine_dropped=469 engine_dropped_agent=0 engine_held_max_ms=298.8 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.8 engine_underrun_events=1 engine_underrun_frames=512 engine_stale_errors=0 consumer_rejected=1
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=feed_4x5 run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=985 late=4 early=0 dropped=811 present_p50_ms=42.8 present_p95_ms=170.7 present_max_ms=1088.0 held_max_ms=1086.7 av_offset_max_ms=1966.7 clock_stall_max_ms=48.9 underrun_frames=512 drain_underrun_frames=0 peak_rss_mib=1720.1 ledger_peak_mib=99.0 table_live_kib=128 passes=false engine_due=1801 engine_on_time=989 engine_late=0 engine_dropped=812 engine_dropped_agent=0 engine_held_max_ms=1088.0 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.9 engine_underrun_events=1 engine_underrun_frames=512 engine_stale_errors=0 consumer_rejected=1
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=talk_recut run=0 valid=true elapsed_s=240.03 missed_callbacks=0 due=7200 on_time=6460 late=22 early=0 dropped=718 present_p50_ms=42.0 present_p95_ms=66.2 present_max_ms=235.2 held_max_ms=235.0 av_offset_max_ms=0.0 clock_stall_max_ms=60.0 underrun_frames=0 drain_underrun_frames=0 peak_rss_mib=1458.3 ledger_peak_mib=42.2 table_live_kib=128 passes=false engine_due=7201 engine_on_time=6482 engine_late=0 engine_dropped=719 engine_dropped_agent=0 engine_held_max_ms=235.2 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.8 engine_underrun_events=0 engine_underrun_frames=0 engine_stale_errors=0 consumer_rejected=0
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 926 filtered out; finished in 537.57s
# LH, same (explainer lost to the filter)
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=reel_9x16 run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1404 late=7 early=0 dropped=389 present_p50_ms=42.2 present_p95_ms=106.7 present_max_ms=298.3 held_max_ms=297.8 av_offset_max_ms=1966.7 clock_stall_max_ms=45.8 underrun_frames=512 drain_underrun_frames=0 peak_rss_mib=1591.3 ledger_peak_mib=68.4 table_live_kib=128 passes=false engine_due=1801 engine_on_time=1411 engine_late=0 engine_dropped=390 engine_dropped_agent=0 engine_held_max_ms=298.3 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.6 engine_underrun_events=1 engine_underrun_frames=512 engine_stale_errors=0 consumer_rejected=1
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=feed_4x5 run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1221 late=2 early=0 dropped=577 present_p50_ms=42.4 present_p95_ms=128.2 present_max_ms=725.4 held_max_ms=723.4 av_offset_max_ms=1966.7 clock_stall_max_ms=45.5 underrun_frames=512 drain_underrun_frames=0 peak_rss_mib=1803.0 ledger_peak_mib=99.0 table_live_kib=128 passes=false engine_due=1801 engine_on_time=1223 engine_late=0 engine_dropped=578 engine_dropped_agent=0 engine_held_max_ms=725.4 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.4 engine_underrun_events=1 engine_underrun_frames=512 engine_stale_errors=0 consumer_rejected=1
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=talk_recut run=0 valid=true elapsed_s=240.01 missed_callbacks=0 due=7200 on_time=6446 late=13 early=0 dropped=741 present_p50_ms=42.0 present_p95_ms=71.1 present_max_ms=256.4 held_max_ms=256.0 av_offset_max_ms=1933.3 clock_stall_max_ms=45.7 underrun_frames=0 drain_underrun_frames=0 peak_rss_mib=1765.8 ledger_peak_mib=42.2 table_live_kib=128 passes=false engine_due=7201 engine_on_time=6459 engine_late=0 engine_dropped=742 engine_dropped_agent=0 engine_held_max_ms=256.4 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.9 engine_underrun_events=0 engine_underrun_frames=0 engine_stale_errors=0 consumer_rejected=1
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 926 filtered out; finished in 541.49s
# follow-up LL, PF1_ONLY=typical_1080p,explainer_16x9 PF1_RUNS=1
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=typical_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=571 late=9 early=0 dropped=1220 present_p50_ms=64.2 present_p95_ms=272.8 present_max_ms=340.5 held_max_ms=340.1 av_offset_max_ms=0.0 clock_stall_max_ms=47.5 underrun_frames=512 drain_underrun_frames=0 peak_rss_mib=852.4 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1801 engine_on_time=580 engine_late=0 engine_dropped=1221 engine_dropped_agent=0 engine_held_max_ms=340.5 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=43.2 engine_underrun_events=1 engine_underrun_frames=512 engine_stale_errors=0 consumer_rejected=0
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=explainer_16x9 run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1461 late=5 early=0 dropped=334 present_p50_ms=42.3 present_p95_ms=106.4 present_max_ms=212.8 held_max_ms=212.2 av_offset_max_ms=1966.7 clock_stall_max_ms=46.5 underrun_frames=512 drain_underrun_frames=0 peak_rss_mib=1268.9 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1801 engine_on_time=1466 engine_late=0 engine_dropped=335 engine_dropped_agent=0 engine_held_max_ms=212.8 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.6 engine_underrun_events=1 engine_underrun_frames=512 engine_stale_errors=0 consumer_rejected=1
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 926 filtered out; finished in 136.04s
# follow-up LH, PF1_ONLY=typical_1080p,explainer_16x9,controls PF1_RUNS=1
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=typical_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=596 late=10 early=0 dropped=1194 present_p50_ms=64.2 present_p95_ms=251.5 present_max_ms=362.8 held_max_ms=362.1 av_offset_max_ms=0.0 clock_stall_max_ms=45.8 underrun_frames=512 drain_underrun_frames=0 peak_rss_mib=903.2 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1801 engine_on_time=606 engine_late=0 engine_dropped=1195 engine_dropped_agent=0 engine_held_max_ms=362.8 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.4 engine_underrun_events=1 engine_underrun_frames=512 engine_stale_errors=0 consumer_rejected=0
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=explainer_16x9 run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1476 late=9 early=0 dropped=315 present_p50_ms=42.1 present_p95_ms=106.2 present_max_ms=234.5 held_max_ms=233.9 av_offset_max_ms=1966.7 clock_stall_max_ms=45.5 underrun_frames=512 drain_underrun_frames=0 peak_rss_mib=1325.9 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1801 engine_on_time=1485 engine_late=0 engine_dropped=316 engine_dropped_agent=0 engine_held_max_ms=234.5 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.9 engine_underrun_events=1 engine_underrun_frames=512 engine_stale_errors=0 consumer_rejected=1
PF1 control=Slowdown lane=LH workload=typical_1080p metric=G1 fails=true valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=154 late=27 early=0 dropped=1619 present_p50_ms=386.4 present_p95_ms=985.4 present_max_ms=1518.2 held_max_ms=1513.7 av_offset_max_ms=0.0 clock_stall_max_ms=45.6 underrun_frames=512 drain_underrun_frames=0 peak_rss_mib=1525.2 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1801 engine_on_time=181 engine_late=0 engine_dropped=1620 engine_dropped_agent=0 engine_held_max_ms=1518.2 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=41.5 engine_underrun_events=1 engine_underrun_frames=512 engine_stale_errors=0 consumer_rejected=0
PF1 control=Freeze lane=LH workload=typical_1080p metric=G14 fails=true valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=573 late=11 early=0 dropped=1216 present_p50_ms=64.2 present_p95_ms=256.2 present_max_ms=1080.8 held_max_ms=1076.4 av_offset_max_ms=0.0 clock_stall_max_ms=45.7 underrun_frames=512 drain_underrun_frames=0 peak_rss_mib=1665.9 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1801 engine_on_time=584 engine_late=0 engine_dropped=1217 engine_dropped_agent=0 engine_held_max_ms=1080.8 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.5 engine_underrun_events=1 engine_underrun_frames=512 engine_stale_errors=0 consumer_rejected=0
PF1 control=ClockFreeze lane=LH workload=typical_1080p metric=G16 fails=true valid=true elapsed_s=61.02 missed_callbacks=0 due=1800 on_time=588 late=6 early=0 dropped=1206 present_p50_ms=64.2 present_p95_ms=254.7 present_max_ms=1091.3 held_max_ms=1086.3 av_offset_max_ms=0.0 clock_stall_max_ms=1050.0 underrun_frames=512 drain_underrun_frames=0 peak_rss_mib=1548.3 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1801 engine_on_time=594 engine_late=0 engine_dropped=1207 engine_dropped_agent=0 engine_held_max_ms=1091.3 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=1044.7 engine_underrun_events=1 engine_underrun_frames=512 engine_stale_errors=0 consumer_rejected=0
PF1 control=Stall lane=LH workload=typical_1080p metric=underrun_frames fails=true valid=true elapsed_s=61.02 missed_callbacks=0 due=1800 on_time=586 late=13 early=0 dropped=1201 present_p50_ms=64.2 present_p95_ms=248.8 present_max_ms=1066.3 held_max_ms=1065.2 av_offset_max_ms=0.0 clock_stall_max_ms=1045.0 underrun_frames=48640 drain_underrun_frames=0 peak_rss_mib=1626.9 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1801 engine_on_time=599 engine_late=0 engine_dropped=1202 engine_dropped_agent=0 engine_held_max_ms=1066.3 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.6 engine_underrun_events=48 engine_underrun_frames=48640 engine_stale_errors=0 consumer_rejected=0
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 926 filtered out; finished in 391.24s
```

P-seek (`pf1_seek_baseline`):

```
# LL (seek_gop60 run=0 lost to the filter)
PF1 seek lane=LL workload=seek_gop60 run=1 random_p95_ms=64.9 random_max_ms=77.4 forward_p95_ms=61.4 plus1_p95_ms=63.6 plus1_n=11 backward_combined_p95_ms=62.5 drag_p95_ms=106.8 drag_answered_p95_ms=106.8 drag_unanswered=0 drag_distinct_fps=23.6 release_pending_drag_calls=1 release_shown=true release_ms=46.7 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=seek_gop60 run=2 random_p95_ms=62.2 random_max_ms=72.9 forward_p95_ms=61.2 plus1_p95_ms=51.6 plus1_n=8 backward_combined_p95_ms=59.9 drag_p95_ms=101.9 drag_answered_p95_ms=101.9 drag_unanswered=0 drag_distinct_fps=24.4 release_pending_drag_calls=1 release_shown=true release_ms=47.0 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=talk_recut run=0 random_p95_ms=89.7 random_max_ms=153.8 forward_p95_ms=109.7 plus1_p95_ms=138.6 plus1_n=17 backward_combined_p95_ms=94.2 drag_p95_ms=129.6 drag_answered_p95_ms=129.6 drag_unanswered=0 drag_distinct_fps=20.4 release_pending_drag_calls=1 release_shown=true release_ms=73.7 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=talk_recut run=1 random_p95_ms=93.7 random_max_ms=154.3 forward_p95_ms=86.2 plus1_p95_ms=107.5 plus1_n=12 backward_combined_p95_ms=86.0 drag_p95_ms=132.9 drag_answered_p95_ms=130.1 drag_unanswered=1 drag_distinct_fps=19.0 release_pending_drag_calls=3 release_shown=true release_ms=87.6 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=talk_recut run=2 random_p95_ms=109.7 random_max_ms=164.4 forward_p95_ms=82.9 plus1_p95_ms=88.9 plus1_n=8 backward_combined_p95_ms=87.1 drag_p95_ms=146.2 drag_answered_p95_ms=144.4 drag_unanswered=1 drag_distinct_fps=18.2 release_pending_drag_calls=3 release_shown=true release_ms=78.4 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=explainer_16x9 run=0 random_p95_ms=127.5 random_max_ms=182.1 forward_p95_ms=130.3 plus1_p95_ms=137.9 plus1_n=17 backward_combined_p95_ms=134.6 drag_p95_ms=253.1 drag_answered_p95_ms=253.1 drag_unanswered=0 drag_distinct_fps=13.6 release_pending_drag_calls=1 release_shown=true release_ms=122.1 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=explainer_16x9 run=1 random_p95_ms=140.5 random_max_ms=165.0 forward_p95_ms=125.9 plus1_p95_ms=125.2 plus1_n=11 backward_combined_p95_ms=132.9 drag_p95_ms=173.2 drag_answered_p95_ms=168.8 drag_unanswered=1 drag_distinct_fps=15.4 release_pending_drag_calls=3 release_shown=true release_ms=135.4 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=explainer_16x9 run=2 random_p95_ms=140.1 random_max_ms=177.9 forward_p95_ms=126.6 plus1_p95_ms=123.2 plus1_n=8 backward_combined_p95_ms=127.9 drag_p95_ms=233.2 drag_answered_p95_ms=233.2 drag_unanswered=0 drag_distinct_fps=14.8 release_pending_drag_calls=1 release_shown=true release_ms=73.0 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 926 filtered out; finished in 456.32s
# LH (seek_gop60 run=0 lost to the filter)
PF1 seek lane=LH workload=seek_gop60 run=1 random_p95_ms=61.8 random_max_ms=68.7 forward_p95_ms=59.0 plus1_p95_ms=70.6 plus1_n=11 backward_combined_p95_ms=59.2 drag_p95_ms=100.4 drag_answered_p95_ms=100.4 drag_unanswered=0 drag_distinct_fps=24.0 release_pending_drag_calls=1 release_shown=true release_ms=53.3 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=seek_gop60 run=2 random_p95_ms=62.2 random_max_ms=70.7 forward_p95_ms=60.7 plus1_p95_ms=50.1 plus1_n=8 backward_combined_p95_ms=59.3 drag_p95_ms=101.6 drag_answered_p95_ms=101.6 drag_unanswered=0 drag_distinct_fps=25.2 release_pending_drag_calls=1 release_shown=true release_ms=45.8 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=talk_recut run=0 random_p95_ms=90.0 random_max_ms=152.7 forward_p95_ms=103.9 plus1_p95_ms=130.4 plus1_n=17 backward_combined_p95_ms=92.0 drag_p95_ms=127.6 drag_answered_p95_ms=127.6 drag_unanswered=0 drag_distinct_fps=21.0 release_pending_drag_calls=1 release_shown=true release_ms=79.3 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=talk_recut run=1 random_p95_ms=88.2 random_max_ms=152.4 forward_p95_ms=83.3 plus1_p95_ms=94.8 plus1_n=12 backward_combined_p95_ms=89.4 drag_p95_ms=131.9 drag_answered_p95_ms=131.9 drag_unanswered=0 drag_distinct_fps=19.6 release_pending_drag_calls=2 release_shown=true release_ms=131.2 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=talk_recut run=2 random_p95_ms=105.5 random_max_ms=161.4 forward_p95_ms=82.1 plus1_p95_ms=87.9 plus1_n=8 backward_combined_p95_ms=85.4 drag_p95_ms=130.2 drag_answered_p95_ms=129.5 drag_unanswered=1 drag_distinct_fps=19.0 release_pending_drag_calls=2 release_shown=true release_ms=74.7 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=explainer_16x9 run=0 random_p95_ms=128.1 random_max_ms=175.2 forward_p95_ms=128.7 plus1_p95_ms=137.4 plus1_n=17 backward_combined_p95_ms=132.4 drag_p95_ms=231.4 drag_answered_p95_ms=231.4 drag_unanswered=0 drag_distinct_fps=14.2 release_pending_drag_calls=2 release_shown=true release_ms=112.3 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=explainer_16x9 run=1 random_p95_ms=138.2 random_max_ms=157.6 forward_p95_ms=121.9 plus1_p95_ms=127.8 plus1_n=11 backward_combined_p95_ms=132.7 drag_p95_ms=167.5 drag_answered_p95_ms=167.5 drag_unanswered=0 drag_distinct_fps=15.8 release_pending_drag_calls=2 release_shown=true release_ms=175.1 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=explainer_16x9 run=2 random_p95_ms=128.7 random_max_ms=179.9 forward_p95_ms=124.8 plus1_p95_ms=121.2 plus1_n=8 backward_combined_p95_ms=124.8 drag_p95_ms=234.2 drag_answered_p95_ms=234.2 drag_unanswered=0 drag_distinct_fps=15.2 release_pending_drag_calls=1 release_shown=true release_ms=90.9 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 926 filtered out; finished in 449.36s
thread 'kinewright-preview' (708437) panicked at /home/riels/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/khronos-egl-6.0.0/src/lib.rs:841:27:
# follow-up LL, PF1_ONLY=seek_gop60
PF1 seek lane=LL workload=seek_gop60 run=0 random_p95_ms=59.1 random_max_ms=69.5 forward_p95_ms=63.8 plus1_p95_ms=69.7 plus1_n=17 backward_combined_p95_ms=64.5 drag_p95_ms=103.5 drag_answered_p95_ms=103.5 drag_unanswered=0 drag_distinct_fps=23.2 release_pending_drag_calls=2 release_shown=true release_ms=87.2 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=seek_gop60 run=1 random_p95_ms=63.8 random_max_ms=69.4 forward_p95_ms=60.1 plus1_p95_ms=66.7 plus1_n=11 backward_combined_p95_ms=61.2 drag_p95_ms=101.1 drag_answered_p95_ms=101.1 drag_unanswered=0 drag_distinct_fps=23.8 release_pending_drag_calls=1 release_shown=true release_ms=59.6 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=seek_gop60 run=2 random_p95_ms=61.1 random_max_ms=69.0 forward_p95_ms=60.4 plus1_p95_ms=50.1 plus1_n=8 backward_combined_p95_ms=62.8 drag_p95_ms=103.4 drag_answered_p95_ms=103.4 drag_unanswered=0 drag_distinct_fps=24.6 release_pending_drag_calls=1 release_shown=true release_ms=46.2 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 926 filtered out; finished in 105.74s
# follow-up LH, PF1_ONLY=seek_gop60
PF1 seek lane=LH workload=seek_gop60 run=0 random_p95_ms=58.3 random_max_ms=68.6 forward_p95_ms=65.7 plus1_p95_ms=66.7 plus1_n=17 backward_combined_p95_ms=59.6 drag_p95_ms=99.4 drag_answered_p95_ms=99.4 drag_unanswered=0 drag_distinct_fps=25.0 release_pending_drag_calls=2 release_shown=true release_ms=88.4 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=seek_gop60 run=1 random_p95_ms=62.5 random_max_ms=68.9 forward_p95_ms=62.6 plus1_p95_ms=60.2 plus1_n=11 backward_combined_p95_ms=60.1 drag_p95_ms=98.7 drag_answered_p95_ms=98.7 drag_unanswered=0 drag_distinct_fps=24.0 release_pending_drag_calls=1 release_shown=true release_ms=41.4 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=seek_gop60 run=2 random_p95_ms=60.9 random_max_ms=71.5 forward_p95_ms=58.1 plus1_p95_ms=57.7 plus1_n=8 backward_combined_p95_ms=59.9 drag_p95_ms=98.6 drag_answered_p95_ms=98.6 drag_unanswered=0 drag_distinct_fps=24.8 release_pending_drag_calls=1 release_shown=true release_ms=44.1 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 926 filtered out; finished in 103.57s
```

CI gates at `58e2f69` (debug, default lane):

```
test audio::tests::the_clock_counts_whole_popped_frames_only ... ok
test engine::tests::a_full_agent_queue_is_refused_without_waiting ... ok
test engine::tests::a_request_waits_for_the_controls_issued_before_it ... ok
test engine::tests::a_drained_one_frame_timeline_stops_at_its_duration ... ok
test decode::tests::fused_table_fill_matches_the_unfused_path_for_every_orientation ... ok
test conversion::tests::monitor_table_equals_the_f32_encode_for_every_f16_pattern ... ok
test engine::tests::only_a_current_stamped_failure_stops_playback ... ok
test engine::tests::a_long_programme_stops_at_its_duration_and_a_stall_does_not_complete ... ok
test frame::tests::rgba64_decode_matches_the_collect_reference_bit_for_bit ... ok
test preview::tests::a_cancelled_agent_job_is_discarded_unanswered ... ok
test engine::tests::dropping_the_engine_stops_the_preview_thread ... ok
test mo2_perf_fixtures::r28_ledger_control_rejects_the_1080p_budget ... ok
test preview::tests::agent_jobs_take_turns_with_transport_attempts ... ok
test stats::tests::due_frames_are_counted_once_by_their_ack ... ok
test preview::tests::shutdown_answers_every_agent_job_exactly_once ... ok
test preview::tests::the_agent_lane_is_fifo_bounded_and_replies_exactly_once ... ok
test preview::tests::the_paused_slot_keeps_the_newest_target_and_renders_the_last ... ok
test preview::tests::only_the_live_monitor_encodes_through_the_table ... ok
test preview::tests::the_preview_thread_leaves_a_playback_hold_at_shutdown ... ok
test engine::tests::the_monitor_caps_the_long_edge_at_1280 ... ok
test engine::tests::g10_video_tracks_the_clock_within_five_seconds_of_a_fill_stall ... ok
test render::tests::preview_window_plays_two_continuous_sources_without_seeking ... ok
test pf1_harness::pf1_the_fill_runs_while_the_preview_renders ... ok
test decode::tests::input_tables_match_every_accepted_descriptor ... ok
test engine::tests::i8_media_7 ... ok
test engine::tests::i8_media_5 ... ok
test mo2_perf_fixtures::r28_ledger_holds_the_ceilings_and_releases_every_charge ... ok
test engine::tests::i8_media_6 ... ok
test engine::tests::i8_media_1 ... ok
test engine::tests::i8_media_0 ... ok
test engine::tests::i8_media_3 ... ok
test engine::tests::i8_media_2 ... ok
test engine::tests::i8_media_4 ... ok
test result: ok. 33 passed; 0 failed; 0 ignored; 0 measured; 894 filtered out; finished in 43.97s
test presenter::tests::a_marker_that_never_paints_is_never_acked ... ok
test presenter::tests::the_marker_marks_what_is_bound_when_it_paints ... ok
test presenter::tests::playback_binds_only_the_clocks_frame ... ok
test presenter::tests::seeded_interleavings_bind_and_ack_only_current_frames ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 783 filtered out; finished in 0.01s
```

### E11.10 R32 review fixes and reruns

Reviews A (reject) and B (accept with fixes) of S2a, with R31's rulings,
were addressed in three commits on `pf1/impl`:
- `5a7fbeb`: media and core;
- `d764262`: the agent;
- `af5abf9`: the app.

Each commit passed its gates:
- build, `clippy --workspace --all-targets -D warnings`, and rustfmt on the
  touched files;
- the affected crates' tests (media at the default thread count);
- `cargo build -p kinewright-app`.

Code delta over `9b51006..af5abf9` (non-blank, non-comment `.rs` lines):
- +1,998 / −439 in total: core +51/−16, media +1,374/−326, agent +82/−11,
  app +491/−86;
- of the added lines, about 1,100 are tests and about 900 are production
  code and harness.

#### E11.10.1 Findings

"Fails on the old code" names the witness that fails when the fix is
reverted by a mutation (E11.10.3).

**R33 qualification.** The re-reviews of `87f25d5` found several of these
rows only partly closed:
- A-F2, A-F4, A-F6 = B-F6, B-F3, the UI item, `dropped_agent`, and R31
  (a) and (b);
- the I8 consumer (A-S1) and the egui discard case (A-S2).

The rows describe the code at `af5abf9`. Where a row claims more than that
code does, E11.11 records the gap and its fix; "closed" is claimed only
there.

| Finding | Fix | Witness (fails on the old code) |
|---|---|---|
| A-F1 = B-F1 relabelling | Every transport call is issued in `Coalesced` under its lock: stamp, control index and caller-side clock effect together. It is sent after unlock (H-4). The worker applies controls in index order and stashes early arrivals. Requests issued before an applied control are superseded (not rendered). A seek issued before a `Pause` sets where the pause rests; the resting image goes under the pause's stamp. A job carries its own request's stamp. The E11.8 "newest applied stamp" reading is withdrawn. | `a_request_issued_before_a_new_document_is_superseded_not_relabelled`; `controls_apply_in_issuance_order_whatever_order_they_arrive`; `two_concurrent_senders_reversed_on_the_wire_apply_in_issuance_order` (real engine; a hook reverses two senders' sends); the I8 shards. `concurrent_callers_never_relabel_a_request` (4 threads, issuance-log oracle) passes under the relabel mutation, so the two above are the F1 witnesses. The R29/R30 stop witnesses stay green. |
| A-F2 bind after expiry | `Presenter::finalize` chooses a frame, prepares its CPU image, then re-checks the latest stamp and clock. Only a still-eligible frame writes the cell and binds the texture. (R33: the check and the binding are not one step; the residual window is closed by judging expiry at paint, E11.8.) | `presenter::tests::a_clock_advance_during_preparation_binds_nothing` (barrier: the clock moves 10 → 11 during preparation) |
| A-F3 clipped image acked | The marker is added with the image rect as its clip, intersected with the viewer's clip (`painter_at(image_rect)`). | `the_real_paint_callback_acks_only_painted_images`, clipped-image case: a positive 64×8 viewer clip that misses the image |
| A-F4 scrub resume skips the target | Release seeks, then resumes only once the target's own image is bound under a stamp no older than the seek. Any other transport call, an error, a preview clear or a project switch cancels the resume. | `a_released_scrub_resumes_only_once_its_target_is_shown` (target = duration − 1, auto-resume). R33: binding is not a paint. The resume now waits for the target's paint ack (E11.11, A-D2). |
| A-F5 boundary counts | The first frame is due at `play`. The terminal stop samples through the end. Records outlive the stop. | `the_first_frame_is_due_at_play_even_when_drained_before_a_tick`; `an_ack_after_the_terminal_stop_counts`; `stats::an_ack_after_the_stop_counts` (pause after drain, ack after EOS); `due_frames_are_counted_once_by_their_ack` |
| A-F6 = B-F6 windows | Each ack is judged by its paint instant and its own epoch. Records are bounded by count (65,536, E11.8), and a late ack counts late, never dropped. App held-age sampling is reconciled with R-5 (E11.8). | `a_late_acknowledgment_is_judged_by_its_paint_and_never_expires` (100 ms ack delay; 2.1 s late ack); `an_evicted_record_acknowledged_later_is_late` |
| B-F2 MCP cancellation | Core `AgentCancel`. `call_tool` runs the handler in its scope and cancels it when rmcp cancels the request context, or when the future drops. A waiting agent request then returns "preview-thread: agent request cancelled" (E-2) and its job is discarded. | `mcp_server::a_cancelled_tool_call_cancels_its_waiting_frame_proof` (real `McpServer`, cancellable client request, `get_frame_at` behind `hold_agent_lane`); `engine::tests::a_cancelled_caller_stops_waiting_for_its_agent_job` |
| B-F3 racing seek | `tick` and `pause` sample the clock only when every issued control has been applied and no seek is pending. | `a_seek_racing_the_tick_manufactures_no_due_frames` (seek(900) at frame 10, sequential stepping of the worker, not a thread barrier). R33: the ack path still sampled the caller clock (E11.11, A-D3 = B-D1). |
| B-F4 telemetry | `sync_decoders` comes from a `DecoderGauge`. It is scoped on the preview, the proof renderers and the exports, and held by each open `VideoSource`. `PlaybackStats.live_table_bytes` is new. | `sync_decoders_counts_open_decoders_until_teardown` (open, close, engine drop) |
| B-F5 P-play repetitions | 3 runs per workload per lane for all six workloads (E11.10.4) | — |
| R31 (a) straddle | The callback classifies failed pops after the programme is fully pushed as post-end, reported apart (`post_end_underrun_*`). G11 is amended to "0 underruns before the programme end". | `underruns_before_the_programme_end_are_reported_apart` |
| R31 (b) clock progress | `render_output` records each advancing callback. `max_clock_stall_ms` is the longest gap, including an ongoing one. | `the_callback_records_clock_stalls_the_worker_cannot_see`; the stall control (E11.10.4) |
| R31 (c) harness offset | `av_offset_max_ms` is renamed `receipt_offset_max_ms`: a receipt-side diagnostic over every arrival, rejected ones included. The at-ack offset is the engine's `engine_av_offset_max_ms`. | — |
| A-S1 / B I8-M5 | I8 is rewritten (E11.10.2). | coverage floors; the mutations |
| A-S2 real paint | I18 runs egui-wgpu's `Renderer` on a headless device (lavapipe). | `the_real_paint_callback_acks_only_painted_images`: positive, discarded-pass, invisible-root and clipped-image cases. A no-op `paint` fails it. |
| B UI blocking | `cache_inventory` and the branch-review `thumbnail_for_document` run off the UI thread (`KinewrightApp::off_ui`) and apply at the next pass (about 80 lines). | `the_cache_dialog_opens_while_the_agent_lane_is_busy` (took the 2 s hold before) |
| B `dropped_agent` | An agent job counts the due frames that pass while it runs, once against a watermark. | `preview::tests::an_agent_job_counts_the_due_frames_it_displaces` |
| B P-seek stamps | `wait_frame` and `drag_and_release` match the issued stamp (`is_current`), not only `at`. | — |
| B RSS | Teardown telemetry between runs (E11.10.5) | — |
| Nits | STALE caption from the finalized binding; E11.6 corrections (real threads; no paused `FrameWait`); E11.8 16.7 ms and held age | — |

The khronos-egl exit panic is recorded only (R31). Separately, some app
tests that start a real engine (`in1_harness`) end with a SIGSEGV at
process exit when run alone, after `test result: ok`. The pre-existing
`in1b_media_produces_its_declared_code_class_and_severity` does the same.
The full app suite exits 0.

C-5 and C-3:
- `render.rs` changes only to hold a decoder gauge per `VideoSource`.
  `conversion.rs` only makes `live_table_bytes` non-test.
- The byte-exact suites are unedited and green.
- The monitor's pixels are unchanged. The STALE caption is the same shape,
  now placed after finalization.
- No tolerance was added.

#### E11.10.2 I8 (rewritten)

- **Shards:** `engine::tests::i8_media_0`…`_3`, 4 × 250 seeded sequences
  with fake audio, plus `i8_media_real_audio`, 24 seeds on the real
  simulated audio path.
- **Each sequence:** 32 random steps over 15 step kinds, with issue, send,
  worker pass, take, execute and consume split into separate steps and
  sends delivered in random order. Then:
  - a scripted play;
  - every third seed, a scripted current failure;
  - a release pause + seek, with the L-6 assertion.
- **Stepping:** single-thread and stepped, not real threads. The real
  threads are the F1 witnesses above.
- **Step budget:** `QUIESCENCE_STEPS` = 64 per drain. A mutation that
  keeps a job forever fails; it does not hang.
- **Oracle:** independent. Each published frame's stamp maps to the call
  that issued it, and the frame's document (its duration encoded in the
  fake render) and target must equal that call's.
- **Coverage floors per shard:** 10 in the fake-audio shards, 1 in the
  real-audio shard. Typical fake-audio shard counts:
  - reversed sends 700–800;
  - superseded requests about 360;
  - stale results rejected about 120;
  - playback published about 300;
  - paused published about 370;
  - interleaved takes about 220;
  - current failures about 85;
  - stale failures about 75.
- **CI cost:** about 1.8 s user + 0.4 s system CPU, 1.7 s wall (debug),
  for all five shards, against about 330 CPU-s before.

#### E11.10.3 Broken variants (R32)

Each was applied to the R32 tree, the affected suites were run, and the
variant was reverted.

| Variant | Break | Witnesses that fail |
|---|---|---|
| M1 | relabel old requests with the newest stamp; no supersession | `a_request_issued_before…`, all I8 shards, real-audio I8 |
| M2 | no stash: controls apply in arrival order | `controls_apply_in_issuance_order…`, `two_concurrent_senders…`, all I8 shards |
| M3 | `tick` samples the caller-side clock | `a_seek_racing_the_tick…` |
| M4 | no first-frame registration | `the_first_frame_is_due_at_play…`, `due_frames_are_counted_once…` |
| M5 | acks ignored after the stop | `an_ack_after_the_terminal_stop_counts`, `stats::an_ack_after_the_stop_counts` |
| M6 | judged by the ack's arrival, not its paint | `a_late_acknowledgment…`, `due_frames_are_counted_once…` |
| M7 | agent execution counts no displaced frames | `an_agent_job_counts_the_due_frames_it_displaces` |
| M8 | nothing classified post-end | `underruns_before_the_programme_end_are_reported_apart` |
| M9 | the callback records no progress | `the_callback_records_clock_stalls…` |
| M10 | the decoder gauge never increments | `sync_decoders_counts_open_decoders_until_teardown` |
| B1 | `call_tool` without the cancel scope | `a_cancelled_tool_call_cancels_its_waiting_frame_proof` |
| A1 | no-op `CallbackTrait::paint` | `the_real_paint_callback_acks_only_painted_images` |
| A2 | marker clipped to the viewer, not the image | `the_real_paint_callback_acks_only_painted_images` |
| A3 | no re-check after preparation | `a_clock_advance_during_preparation_binds_nothing` |
| A4 | release plays at once | `a_released_scrub_resumes_only_once_its_target_is_shown` |

#### E11.10.4 Timing reruns (R32)

These lines replace E11.2–E11.5 for every gate.

**Provenance.**
- Binary: the release test binary of `af5abf9`
  (`kinewright_media-cf306d076b2fd4bf`, sha256
  `fae49c133e7f088850a4a4d84d48261061d481a84f7532e69e7c10835b616cf4`). It was
  built once and copied aside, so every lane ran the same binary. rustc
  1.98.0. The machine, drivers and FFmpeg are as in E11.1.
- When: 2026-09-27, 18:00:59–19:29:33 EDT. One runner ran the seven lanes in
  sequence, each alone, with no build, test or other lane alongside:

  | Lane | Window (EDT) |
  |---|---|
  | I4 LL | 18:00:59–18:02:18 |
  | I4 LH | 18:02:18–18:03:34 |
  | G3 LH | 18:03:34–18:06:13 |
  | P-play LL | 18:06:13–18:40:20 |
  | P-play LH | 18:40:20–19:14:24 |
  | P-seek LL | 19:14:24–19:22:01 |
  | P-seek LH | 19:22:01–19:29:33 |

- Output was logged unfiltered, so no first line was lost (compare E11.1).
  - The P-play lanes are `PF1_RUNS=3` with no `PF1_ONLY`: every workload
    ran 3 times, then the four Q-3 controls ran once, all in one process
    per lane.
  - P-seek runs 3 per workload per lane.
- Every lane printed `test result: ok. 1 passed`.
- **Ambient load (R26).**
  - The two `foot` screensaver processes and Hyprland were as in E11.1:
    ≈92% + 52% and ≈24% at every BEGIN/END. These `ps` figures are
    lifetime averages.
  - During the two P-seek lanes, two external `gh` processes (not started
    by this work) also showed 70–125% CPU at their BEGIN/END marks. They
    were gone by the end of P-seek LH.
  - Load averages were 2.7–14.0. The highest, 11.7–14.0, came during
    P-seek, which runs decode threads and overlapped the `gh` processes.
- **Exit-time crashes, after the measurement.**
  - P-seek LL printed the khronos-egl `kinewright-preview` panic
    (`lib.rs:841`, E11.4) after `test result: ok`. Its exit status was 0.
  - P-seek LH exited with status 139 (SIGSEGV) after `test result: ok … 448.79s`.
    The core dump's stack is `vkDestroyDevice` (libvulkan) →
    `libnvidia-glcore`: NVIDIA driver teardown at process exit, after the
    lane's last measurement.
  - Both are recorded only (R31). No measurement is affected.

**Measured passes and complete gate evidence (B-F5).**

| Gate | Evidence here | Status |
|---|---|---|
| I4 | 3 runs per lane, LL and LH | measured pass; complete |
| G3 | 3 runs, LH | measured pass; complete |
| G11 (amended, R31 a) | `blend_heavy_1080p`, LL, 3 runs. The five other workloads are 3 runs each, on both lanes. | measured pass; complete for the gate as defined (P-play, LL, simulated driver) |
| G16 | 3 runs × 6 workloads, on LL and LH; the clock-freeze control on both | measured pass; complete for the gate as defined (simulated driver) |
| L-6 | 3 runs × 3 workloads, on LL and LH | measured pass; complete |
| P-play, P-seek | 3 runs per workload per lane | baselines, not gates |
| G1, G14 (S2b); G8 latency (S2c) | measured, `passes=false` | not claimed by S2a |

Two things are not rerun on this binary, and no gate depends on them:
- V-5's one-run-per-workload cross-check on the real audio device (E9);
- the Windows lane.

**I4** (S0's 5% rule, `BASELINES` untouched):

| Build | LL mean ms | LL delta | LH mean ms | LH delta | Result |
|---|---|---|---|---|---|
| S1 end `217986a` (E10.2) | 77.03 / 76.23 / 75.95 | −84.7% | 72.87 / 73.00 / 73.69 | −85.2% | ok |
| R32 `af5abf9` | 76.71 / 76.97 / 76.63 | −84.6% | 73.16 / 74.16 / 72.95 | −85.1% | **ok** |

**G3** (60 fps floor, LH):

| Build | `blend_heavy_1080p` fps (3 runs) | mean ms | p95 ms | Result |
|---|---|---|---|---|
| S1 end `217986a` (E10.3) | 72.1 / 71.4 / 71.7 | 13.86–14.00 | 15.47–15.67 | passes |
| R32 `af5abf9` | 71.7 / 71.2 / 71.8 | 13.92–14.04 | 15.37–15.76 | **passes** |

- Other resident workloads:
  - `typical_1080p`: 73.1–74.1 fps;
  - `heavy_4k`: 59.0–59.4 fps.
- The slowdown control fails its verdict (17.7 fps), as it must.
- Phases:
  - `monitor_encode_ms`: 1.66–1.75;
  - `gpu_passes_readback_ms`: 1.31–1.57;
  - `upload_ms`: 10.08–13.45.

**P-play, G11 and G16 (simulated driver).**

Every run is valid: 60.02 s (240.01–240.03 s for `talk_recut`) and 0 missed
callbacks. Harness columns use the harness's own rules (E9.9). Engine
columns are R-5's `stats()` from the same run.

Column notes:
- Underrun frames are shown before the programme end, harness / engine.
- Post-end is the straddle, reported apart (R31 a).
- Receipt offset is the renamed harness diagnostic (R31 c).
- `engine_av_offset_max_ms` is 0.0 in every run.
- `engine_dropped_agent` is 0 in every run: P-play issues no agent jobs.

| Workload | Lane | Runs | Harness on time / late / dropped | Engine due: on time / late / dropped | Present p50 / p95 / max ms | Held max ms (engine) | Clock stall max ms, harness / engine | Underrun frames, harness / engine | Post-end frames | Receipt offset max ms | Rejected at consumer | Sync decoders |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `typical_1080p` | LL | 3 | 573–575 / 6–10 / 1217–1219 | 1800: 573–574 / 8–9 / 1217–1219 | 64.2 / 256.6–260.8 / 341.8–363.3 | 347.3–363.3 | 47.5–47.6 / 22.9–24.8 | **0 / 0** | 512 | 0 | 0 | 2 |
| `blend_heavy_1080p` | LL | 3 | 643–644 / 3–9 / 1147–1153 | 1800: 641–644 / 4–9 / 1147–1154 | 64.5 / 192.2–195.5 / 256.3–299.2 | 256.3–299.2 | 46.5–47.8 / 22.9–23.2 | **0 / 0** | 512 | 1966.7 | 1 | 3 |
| `explainer_16x9` | LL | 3 | 1447–1451 / 4–6 / 345–348 | 1800: 1447–1451 / 3–6 / 345–348 | 42.2–42.3 / 106.3–106.4 / 235.0–235.7 | 235.0–235.7 | 45.9–47.3 / 22.7–22.8 | 0 / 0 | 512 | 1966.7 | 1 | 4 |
| `reel_9x16` | LL | 3 | 1323–1331 / 5–8 / 464–469 | 1800: 1322–1331 / 5–9 / 464–469 | 42.3 / 106.8–107.1 / 299.1–341.4 | 299.1–341.4 | 46.8–47.5 / 22.8–22.9 | 0 / 0 | 512 | 1966.7 | 1 | 3 |
| `feed_4x5` | LL | 3 | 969–979 / 2–5 / 819–826 | 1800: 968–979 / 2–6 / 819–826 | 42.8–42.9 / 170.5–170.9 / 597.7–1386.7 | 597.7–1386.7 | 47.5–48.9 / 22.9–23.5 | 0 / 0 | 512 | 1966.7 | 1 | 3 |
| `talk_recut` | LL | 3 | 6449–6476 / 17–25 / 699–734 of 7200 | 7200: 6445–6475 / 19–26 / 699–736 | 42.0–42.1 / 65.3–68.9 / 255.9–256.5 | 255.9–256.5 | 59.6–60.0 / 42.9 | 0 / 0 | 0 | 0–33.3 | 0–2 | 1 |
| `typical_1080p` | LH | 3 | 583–594 / 8–12 / 1198–1208 | 1800: 585–594 / 7–11 / 1198–1208 | 64.2 / 247.8–259.1 / 341.6–362.8 | 341.6–362.8 | 45.6–45.8 / 22.1–22.5 | 0 / 0 | 512 | 0 | 0 | 2 |
| `blend_heavy_1080p` | LH | 3 | 671–877 / 0–7 / 918–1129 | 1800: 671–875 / 0–7 / 918–1129 | 63.4–64.3 / 150.3–191.9 / 233.8–298.7 | 233.8–298.7 | 45.4–45.7 / 22.0–22.1 | 0 / 0 | 512 | 0–1966.7 | 0–1 | 3 |
| `explainer_16x9` | LH | 3 | 1480–1488 / 7–16 / 296–313 | 1800: 1480–1488 / 7–16 / 296–313 | 42.1–42.2 / 105.4–106.2 / 214.2–234.8 | 214.2–234.8 | 45.4–45.6 / 21.8–21.9 | 0 / 0 | 512 | 1966.7 | 1 | 4 |
| `reel_9x16` | LH | 3 | 1395–1416 / 9–13 / 371–396 | 1800: 1394–1420 / 9–12 / 371–396 | 42.2 / 106.5–107.0 / 277.2–319.9 | 277.2–319.9 | 45.5–45.6 / 21.8–22.0 | 0 / 0 | 512 | 1966.7 | 1 | 3 |
| `feed_4x5` | LH | 3 | 1194–1215 / 3–8 / 578–603 | 1800: 1193–1215 / 4–8 / 578–603 | 42.3–42.4 / 149.2–169.9 / 425.9–960.6 | 425.9–960.6 | 45.4–45.7 / 22.0–22.9 | 0 / 0 | 512 | 1966.7 | 1 | 3 |
| `talk_recut` | LH | 3 | 6502–6510 / 27–39 / 659–671 of 7200 | 7200: 6497–6508 / 32–40 / 659–671 | 42.0 / 62.6–63.3 / 235.5–256.4 | 235.5–256.4 | 45.7–60.0 / 22.0–43.2 | 0 / 0 | 0 | 0 | 0 | 1 |

Q-3 controls (`typical_1080p`, one run each):

| Control | Lane | Metric | Fails, as it must | Clock stall max ms, harness / engine | Held max ms | Underrun frames (engine events) / post-end | On time, harness / engine |
|---|---|---|---|---|---|---|---|
| Slowdown | LL | G1 | yes | 47.7 / 23.7 | 1475.0 | 0 (0) / 512 | 134 / 135 |
| Freeze | LL | G14 | yes | 48.6 / 23.6 | 1206.9 | 0 (0) / 512 | 574 / 573 |
| ClockFreeze | LL | G16 | yes | **1050.0 / 1026.9** | 1086.5 | 0 (0) / 512 | 574 / 574 |
| Stall | LL | underruns | yes | **1045.0 / 1024.0** | 1063.8 | **48128 (47)** / 512 | 574 / 571 |
| Slowdown | LH | G1 | yes | 45.7 / 22.0 | 1939.4 | 0 (0) / 512 | 173 / 175 |
| Freeze | LH | G14 | yes | 45.7 / 22.1 | 1106.4 | 0 (0) / 512 | 580 / 582 |
| ClockFreeze | LH | G16 | yes | **1049.8 / 1027.5** | 1067.7 | 0 (0) / 512 | 591 / 592 |
| Stall | LH | underruns | yes | **1045.0 / 1024.0** | 1065.1 | **48128 (47)** / 512 | 585 / 585 |

- **G11 passes (amended: 0 underruns before the programme end).**
  - `blend_heavy_1080p` on LL reads `underrun_frames=0` in all 3 runs, and
    so does every other workload on both lanes.
  - The 512-frame straddle (E11.4) is now `post_end_underrun_frames`, in
    both the harness and the engine. It reads 512 in every 60 s run and 0
    in `talk_recut`, whose 240 s is a whole number of callbacks.
  - The stall control still fails: 48,128 frames in 47 events before the
    end, plus the 512 post-end. E11.4's 48,640 in 48 events is exactly
    this sum, so the split reclassifies only the straddle.
- **G16 passes on both lanes.**
  - The harness's max clock stall is 45.4–60.0 ms in every run, against
    the 100 ms gate.
  - The clock-freeze control fails on both lanes (1050.0 / 1049.8 ms).
- **R31 (b) witness: the engine now sees the stall.**
  - In the stall control, `engine_clock_stall_max_ms` is 1024.0 ms on
    both lanes. At `9101e18` it was 42.6 / 42.9 ms (E11.4).
  - In the clock-freeze control, the engine reads 1026.9 / 1027.5 ms.
  - The steady-state engine figure is now one callback period: 21.8–24.8
    ms against 1,024 frames at 48 kHz = 21.3 ms. Before, it was the worker
    tick's view (42–43 ms).
  - `talk_recut` reads 42.9 ms in all LL runs and in one LH run: one gap
    of two callback periods, with no underrun. The harness also reads
    about 60 ms on that workload (E11.4 had the same). It is below the
    gate, and it was not diagnosed further.
- **Engine and harness counts now agree (A-F6).**
  - Engine late is 0–40 in every run, where it was 0 at `9101e18`.
    Harness and engine on time and late agree within ±5 in every run.
  - R-5 now judges each due frame by its paint instant, within due + 1
    frame. The harness judges by its own 5 ms samples, within due + 1
    frame. The 16.7 ms root-epoch allowance is gone (E11.8).
  - Engine due is exactly 1,800 (7,200), from the end-frame fix
    (`58e2f69`).
- **`receipt_offset_max_ms`** (R31 c, renamed from `av_offset_max_ms`):
  - it is 1966.7 ms exactly in the runs with `consumer_rejected=1`, and 0
    otherwise;
  - `talk_recut` LL run 2 reads 33.3 ms with 2 rejected;
  - it is a receipt-side diagnostic (E11.4). The at-ack offset,
    `engine_av_offset_max_ms`, is 0.0 everywhere.
- **`engine_sync_decoders` (B-F4)** is 1–4 per workload during playback: the
  preview's open `VideoSource`s under the `DecoderGauge`. It reads 0 at
  teardown in every run.
- **Spread, not a regression.**
  - `blend_heavy_1080p` on LH ranges from 671 to 877 on time. The
    run-to-run spread is wider than the 776–785 at `9101e18`.
  - The 671 run has no rejected arrival and no late frame. Its present p95
    (191.9 ms) matches the LL runs.
  - The frames this preview misses are the R-3 lead cap's drops at
    ≈64 ms per render (E11.4). They are S2b's G1/G14, not an S2a gate.
- The other P-play figures are within run-to-run noise of `9101e18`
  (E11.4): held max, present percentiles, and the harness's on-time
  counts.

**L-6 and P-seek** (paused, as E9.6):

The table shows the worst run's latency, the smallest distinct fps, and
counts and release ms as ranges.

| Workload | Lane | Runs | Random p95 / max | Forward p95 | +1 p95 | Backward p95 | Drag p95 / answered | Unanswered | Distinct fps | Pending | Release shown / ms (L-6) | Stale / over / valid over release | Timeouts |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `seek_gop60` | LL | 3 | 62.7 / 73.7 | 63.8 | 68.1 | 63.7 | 102.4 / 102.4 | 0 | 23.6 | 1–2 | all / 44.1–71.9 | 1 / 0 / 0 | 0 |
| `talk_recut` | LL | 3 | 107.4 / 178.1 | 106.5 | 124.9 | 93.1 | 137.1 / 137.1 | 0 | 18.8 | 1–2 | all / 70.4–125.4 | 1 / 0 / 0 | 0 |
| `explainer_16x9` | LL | 3 | 140.3 / 180.6 | 129.4 | 135.7 | 136.5 | 234.4 / 234.4 | 0 | 14.6 | 1–2 | all / 85.3–174.0 | 1 / 0 / 0 | 0 |
| `seek_gop60` | LH | 3 | 62.1 / 69.0 | 63.5 | 67.3 | 61.6 | 99.7 / 99.7 | 0 | 24.2 | 1 | all / 36.9–99.3 | 1 / 0 / 0 | 0 |
| `talk_recut` | LH | 3 | 107.4 / 155.7 | 107.7 | 134.7 | 98.9 | 130.7 / 130.2 | 0–1 | 19.0 | 1–3 | all / 53.9–128.5 | 1 / 0 / 0 | 0 |
| `explainer_16x9` | LH | 3 | 140.3 / 174.0 | 127.1 | 137.1 | 131.9 | 237.8 / 237.0 | 1–2 | 14.6 | 2–4 | all / 55.0–102.7 | 1 / 0 / 0 | 0 |

- **L-6 passes on both lanes.**
  - The release target is shown in every run, and
    `valid_frames_over_release=0`.
  - The harness now matches the issued stamp (`is_current`, B P-seek),
    not only `at`.
  - The one stale frame per run is still an older-epoch drag frame
    delivered after the release call, which R-2 rejects.
- The latencies are within run-to-run noise of `9101e18` (E11.5).
- G8's latency gates are S2c's and still fail.

#### E11.10.5 RSS teardown telemetry (B RSS)

Each P-play run now ends with a teardown record. The harness drops the
engine and waits until the frame channel disconnects. The preview
thread's exit causes that; the worker's own teardown is not awaited (R33).
Only then does it read the record. `teardown_complete=true`
in all 44 runs, controls included.

| Lane | Ledger live | Sync decoders | Conversion tables live | Threads | Teardown RSS MiB across the process's runs (in order) |
|---|---|---|---|---|---|
| LL | 0 KiB in every run | 0 in every run | 128 KiB in every run | 86–88 | 798 → 1017 → 1082 (typical) → 1344–1538 (blend) → 1674–2160 (explainer) → 2184–2273 (reel) → 2531–2857 (feed) → 2748–2796 (talk) → 2584–2947 (controls) |
| LH | 0 KiB in every run | 0 in every run | 128 KiB in every run | 12–13 | 851 → 1144 → 961 (typical) → 1077–1320 (blend) → 1338–1500 (explainer) → 1353–1539 (reel) → 1472–1490 (feed) → 1478 (talk) → 1500–1530 (controls) |

**Attribution (qualified in R33): consistent with retention below the
engine, not proof.**
- R33 caveat (re-review A nit, B D6): at `af5abf9` this teardown sampled
  RSS while the harness still held the `GpuContext` (device and queue).
  It observed only that the frame channel disconnected, not that the
  whole worker had finished. E11.11 reruns the teardown to that
  boundary.
- Everything the engine accounts for returns to its baseline at every
  teardown: the texture/buffer ledger is 0, and no decoder is open.
- The conversion-table registry is a bounded, process-wide LRU. It holds
  128 KiB, constant from the first run.
- Thread counts are flat across runs, so no thread accumulates.
- The growth plateaus instead of rising linearly:
  - LH levels off at about 1.48–1.53 GiB from the fourth workload on;
  - LL levels off at about 2.7–2.9 GiB;
  - LH even falls between runs (1144 → 961 MiB).
- RSS that plateaus while every counter the engine keeps returns to its
  baseline is consistent with retention below the engine. It does not
  prove that every engine-owned allocation was released: an allocation
  no counter tracks would look the same. Two holders fit:
  - the allocator keeps freed arenas and pages (glibc malloc across about
    86 threads on LL);
  - the graphics driver keeps its own allocations. LL's llvmpipe
    rasterizer threads and its larger growth point to llvmpipe
    specifically.
- This run separates neither those two holders nor retention from an
  untracked allocation. It does not show that no engine change could be
  needed.
- The first-run figures match E11.4:
  - peak RSS 891.5 MiB (LL) and 921.5 MiB (LH) in a fresh process;
  - S1 had 860.3 / 898.6.
- P-rss is still measured one process per workload, at S4 (E11.4).

#### E11.10.6 Raw result lines (R32, `af5abf9`)

`#` lines are the runner's lane markers and exit statuses; everything else
is verbatim from `lanes.log`.

```
# I4_LL (began 2026-09-27 18:00:59)
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=false workload=typical_1080p run=0 dims=(1280, 720) mean_ms=76.71 fps=13.0 p95_ms=208.16 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=false workload=typical_1080p run=1 dims=(1280, 720) mean_ms=76.97 fps=13.0 p95_ms=209.14 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=false workload=typical_1080p run=2 dims=(1280, 720) mean_ms=76.63 fps=13.1 p95_ms=206.28 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) workload=typical_1080p baseline_ms=497.86 delta=-84.6%
# exit=0 (I4_LL)
# I4_LH (began 2026-09-27 18:02:18)
R28 adapter=NVIDIA GeForce RTX 3090 resident=false workload=typical_1080p run=0 dims=(1280, 720) mean_ms=73.16 fps=13.7 p95_ms=202.08 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 resident=false workload=typical_1080p run=1 dims=(1280, 720) mean_ms=74.16 fps=13.5 p95_ms=207.13 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 resident=false workload=typical_1080p run=2 dims=(1280, 720) mean_ms=72.95 fps=13.7 p95_ms=203.02 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 workload=typical_1080p baseline_ms=494.39 delta=-85.1%
# exit=0 (I4_LH)
# G3_LH (began 2026-09-27 18:03:34)
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=typical_1080p run=0 dims=(1280, 720) mean_ms=13.56 fps=73.7 p95_ms=15.38 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=typical_1080p run=1 dims=(1280, 720) mean_ms=13.49 fps=74.1 p95_ms=14.96 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=typical_1080p run=2 dims=(1280, 720) mean_ms=13.69 fps=73.1 p95_ms=15.44 ledger_peak_mib=56.3 validate_us=1.4
R28 phases workload=typical_1080p upload_ms=10.08 gpu_passes_readback_ms=1.31 monitor_encode_ms=1.67
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=blend_heavy_1080p run=0 dims=(1280, 720) mean_ms=13.94 fps=71.7 p95_ms=15.37 ledger_peak_mib=63.3 validate_us=1.2
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=blend_heavy_1080p run=1 dims=(1280, 720) mean_ms=14.04 fps=71.2 p95_ms=15.76 ledger_peak_mib=63.3 validate_us=1.2
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=blend_heavy_1080p run=2 dims=(1280, 720) mean_ms=13.92 fps=71.8 p95_ms=15.53 ledger_peak_mib=63.3 validate_us=1.2
R28 phases workload=blend_heavy_1080p upload_ms=10.18 gpu_passes_readback_ms=1.55 monitor_encode_ms=1.75
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=heavy_4k run=0 dims=(1280, 720) mean_ms=16.86 fps=59.3 p95_ms=18.23 ledger_peak_mib=70.3 validate_us=9.7
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=heavy_4k run=1 dims=(1280, 720) mean_ms=16.95 fps=59.0 p95_ms=18.65 ledger_peak_mib=70.3 validate_us=10.9
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=heavy_4k run=2 dims=(1280, 720) mean_ms=16.83 fps=59.4 p95_ms=18.26 ledger_peak_mib=70.3 validate_us=9.0
R28 phases workload=heavy_4k upload_ms=13.45 gpu_passes_readback_ms=1.57 monitor_encode_ms=1.66
R28 control=slowdown delay_ms=41.7 mean_ms=56.36 p95_ms=59.09 verdict=Err("17.7 fps (floor 60), p95 59.1 ms vs mean 56.4 ms")
# exit=0 (G3_LH)
# play_LL (began 2026-09-27 18:06:13)
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=typical_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=573 late=10 early=0 dropped=1217 present_p50_ms=64.2 present_p95_ms=259.2 present_max_ms=362.8 held_max_ms=360.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=47.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=891.5 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=574 engine_late=9 engine_dropped=1217 engine_dropped_agent=0 engine_held_max_ms=362.8 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=24.8 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=798.3 threads=86
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=typical_1080p run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=575 late=6 early=0 dropped=1219 present_p50_ms=64.2 present_p95_ms=260.8 present_max_ms=341.8 held_max_ms=350.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=47.6 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1030.4 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=573 engine_late=8 engine_dropped=1219 engine_dropped_agent=0 engine_held_max_ms=347.3 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=23.4 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1017.2 threads=87
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=typical_1080p run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=574 late=8 early=0 dropped=1218 present_p50_ms=64.2 present_p95_ms=256.6 present_max_ms=363.3 held_max_ms=360.5 receipt_offset_max_ms=0.0 clock_stall_max_ms=47.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1179.2 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=574 engine_late=8 engine_dropped=1218 engine_dropped_agent=0 engine_held_max_ms=363.3 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1082.4 threads=87
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=blend_heavy_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=643 late=4 early=1 dropped=1152 present_p50_ms=64.5 present_p95_ms=194.9 present_max_ms=256.3 held_max_ms=254.0 receipt_offset_max_ms=1966.7 clock_stall_max_ms=46.6 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1410.4 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=641 engine_late=6 engine_dropped=1153 engine_dropped_agent=0 engine_held_max_ms=256.3 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1343.7 threads=86
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=blend_heavy_1080p run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=644 late=9 early=0 dropped=1147 present_p50_ms=64.5 present_p95_ms=195.5 present_max_ms=256.7 held_max_ms=256.5 receipt_offset_max_ms=1966.7 clock_stall_max_ms=46.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1529.5 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=644 engine_late=9 engine_dropped=1147 engine_dropped_agent=0 engine_held_max_ms=256.7 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=23.2 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1460.9 threads=87
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=blend_heavy_1080p run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=643 late=3 early=1 dropped=1153 present_p50_ms=64.5 present_p95_ms=192.2 present_max_ms=299.2 held_max_ms=294.8 receipt_offset_max_ms=1966.7 clock_stall_max_ms=47.8 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1562.3 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=642 engine_late=4 engine_dropped=1154 engine_dropped_agent=0 engine_held_max_ms=299.2 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=23.0 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1538.4 threads=87
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=explainer_16x9 run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1448 late=4 early=0 dropped=348 present_p50_ms=42.2 present_p95_ms=106.4 present_max_ms=235.7 held_max_ms=232.3 receipt_offset_max_ms=1966.7 clock_stall_max_ms=45.9 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1724.1 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1449 engine_late=3 engine_dropped=348 engine_dropped_agent=0 engine_held_max_ms=235.7 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.7 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=4 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1674.3 threads=86
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=explainer_16x9 run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1447 late=6 early=0 dropped=347 present_p50_ms=42.2 present_p95_ms=106.3 present_max_ms=235.0 held_max_ms=233.3 receipt_offset_max_ms=1966.7 clock_stall_max_ms=47.3 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2049.0 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1447 engine_late=6 engine_dropped=347 engine_dropped_agent=0 engine_held_max_ms=235.0 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.7 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=4 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1945.5 threads=87
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=explainer_16x9 run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1451 late=4 early=0 dropped=345 present_p50_ms=42.3 present_p95_ms=106.4 present_max_ms=235.3 held_max_ms=233.7 receipt_offset_max_ms=1966.7 clock_stall_max_ms=47.2 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2175.6 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1451 engine_late=4 engine_dropped=345 engine_dropped_agent=0 engine_held_max_ms=235.3 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.8 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=4 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2159.7 threads=87
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=reel_9x16 run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1331 late=5 early=0 dropped=464 present_p50_ms=42.3 present_p95_ms=107.1 present_max_ms=320.2 held_max_ms=317.5 receipt_offset_max_ms=1966.7 clock_stall_max_ms=47.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2275.0 ledger_peak_mib=68.4 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1331 engine_late=5 engine_dropped=464 engine_dropped_agent=0 engine_held_max_ms=320.2 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2183.9 threads=87
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=reel_9x16 run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1327 late=6 early=0 dropped=467 present_p50_ms=42.3 present_p95_ms=106.8 present_max_ms=341.4 held_max_ms=340.9 receipt_offset_max_ms=1966.7 clock_stall_max_ms=46.8 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2330.6 ledger_peak_mib=68.4 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1327 engine_late=6 engine_dropped=467 engine_dropped_agent=0 engine_held_max_ms=341.4 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2232.1 threads=87
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=reel_9x16 run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1323 late=8 early=0 dropped=469 present_p50_ms=42.3 present_p95_ms=106.9 present_max_ms=299.1 held_max_ms=296.4 receipt_offset_max_ms=1966.7 clock_stall_max_ms=47.1 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2421.0 ledger_peak_mib=68.4 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1322 engine_late=9 engine_dropped=469 engine_dropped_agent=0 engine_held_max_ms=299.1 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.8 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2272.5 threads=87
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=feed_4x5 run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=969 late=5 early=0 dropped=826 present_p50_ms=42.9 present_p95_ms=170.5 present_max_ms=1386.7 held_max_ms=1383.4 receipt_offset_max_ms=1966.7 clock_stall_max_ms=48.8 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2754.6 ledger_peak_mib=99.0 table_live_kib=128 passes=false engine_due=1800 engine_on_time=968 engine_late=6 engine_dropped=826 engine_dropped_agent=0 engine_held_max_ms=1386.7 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2531.0 threads=87
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=feed_4x5 run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=979 late=2 early=0 dropped=819 present_p50_ms=42.8 present_p95_ms=170.5 present_max_ms=865.3 held_max_ms=861.4 receipt_offset_max_ms=1966.7 clock_stall_max_ms=48.9 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2979.8 ledger_peak_mib=99.0 table_live_kib=128 passes=false engine_due=1800 engine_on_time=979 engine_late=2 engine_dropped=819 engine_dropped_agent=0 engine_held_max_ms=865.3 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2825.1 threads=88
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=feed_4x5 run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=974 late=5 early=0 dropped=821 present_p50_ms=42.9 present_p95_ms=170.9 present_max_ms=597.7 held_max_ms=594.5 receipt_offset_max_ms=1966.7 clock_stall_max_ms=47.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=3007.2 ledger_peak_mib=99.0 table_live_kib=128 passes=false engine_due=1800 engine_on_time=975 engine_late=4 engine_dropped=821 engine_dropped_agent=0 engine_held_max_ms=597.7 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=23.5 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2857.3 threads=87
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=talk_recut run=0 valid=true elapsed_s=240.03 missed_callbacks=0 due=7200 on_time=6476 late=25 early=0 dropped=699 present_p50_ms=42.1 present_p95_ms=65.3 present_max_ms=256.5 held_max_ms=254.4 receipt_offset_max_ms=0.0 clock_stall_max_ms=60.0 underrun_frames=0 post_end_underrun_frames=0 peak_rss_mib=2821.6 ledger_peak_mib=42.2 table_live_kib=128 passes=false engine_due=7200 engine_on_time=6475 engine_late=26 engine_dropped=699 engine_dropped_agent=0 engine_held_max_ms=256.5 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=0 engine_sync_decoders=1 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2774.9 threads=87
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=talk_recut run=1 valid=true elapsed_s=240.03 missed_callbacks=0 due=7200 on_time=6451 late=19 early=0 dropped=730 present_p50_ms=42.0 present_p95_ms=67.9 present_max_ms=256.0 held_max_ms=254.4 receipt_offset_max_ms=0.0 clock_stall_max_ms=60.0 underrun_frames=0 post_end_underrun_frames=0 peak_rss_mib=2865.4 ledger_peak_mib=42.2 table_live_kib=128 passes=false engine_due=7200 engine_on_time=6449 engine_late=21 engine_dropped=730 engine_dropped_agent=0 engine_held_max_ms=256.0 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=0 engine_sync_decoders=1 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2795.5 threads=86
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=talk_recut run=2 valid=true elapsed_s=240.03 missed_callbacks=0 due=7200 on_time=6449 late=17 early=0 dropped=734 present_p50_ms=42.1 present_p95_ms=68.9 present_max_ms=255.9 held_max_ms=254.6 receipt_offset_max_ms=33.3 clock_stall_max_ms=59.6 underrun_frames=0 post_end_underrun_frames=0 peak_rss_mib=2749.5 ledger_peak_mib=42.2 table_live_kib=128 passes=false engine_due=7200 engine_on_time=6445 engine_late=19 engine_dropped=736 engine_dropped_agent=0 engine_held_max_ms=255.9 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=0 engine_sync_decoders=1 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=2 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2747.6 threads=87
PF1 control=Slowdown lane=LL workload=typical_1080p metric=G1 fails=true valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=134 late=26 early=0 dropped=1640 present_p50_ms=410.0 present_p95_ms=975.2 present_max_ms=1477.8 held_max_ms=1475.0 receipt_offset_max_ms=0.0 clock_stall_max_ms=47.7 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2773.5 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=135 engine_late=25 engine_dropped=1640 engine_dropped_agent=0 engine_held_max_ms=1477.8 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=23.7 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2584.0 threads=87
PF1 control=Freeze lane=LL workload=typical_1080p metric=G14 fails=true valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=574 late=6 early=0 dropped=1220 present_p50_ms=64.2 present_p95_ms=255.9 present_max_ms=1208.4 held_max_ms=1206.9 receipt_offset_max_ms=0.0 clock_stall_max_ms=48.6 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2911.1 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=573 engine_late=7 engine_dropped=1220 engine_dropped_agent=0 engine_held_max_ms=1208.4 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=23.6 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2874.1 threads=88
PF1 control=ClockFreeze lane=LL workload=typical_1080p metric=G16 fails=true valid=true elapsed_s=61.02 missed_callbacks=0 due=1800 on_time=574 late=5 early=0 dropped=1221 present_p50_ms=64.2 present_p95_ms=264.1 present_max_ms=1091.2 held_max_ms=1086.5 receipt_offset_max_ms=0.0 clock_stall_max_ms=1050.0 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2868.6 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=574 engine_late=5 engine_dropped=1221 engine_dropped_agent=0 engine_held_max_ms=1091.2 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=1026.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2670.7 threads=86
PF1 control=Stall lane=LL workload=typical_1080p metric=underrun_frames fails=true valid=true elapsed_s=61.02 missed_callbacks=0 due=1800 on_time=574 late=5 early=0 dropped=1221 present_p50_ms=64.2 present_p95_ms=267.5 present_max_ms=1067.1 held_max_ms=1063.8 receipt_offset_max_ms=0.0 clock_stall_max_ms=1045.0 underrun_frames=48128 post_end_underrun_frames=512 peak_rss_mib=2968.2 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=571 engine_late=8 engine_dropped=1221 engine_dropped_agent=0 engine_held_max_ms=1067.1 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=1024.0 engine_underrun_events=47 engine_underrun_frames=48128 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2947.0 threads=87
# exit=0 (play_LL)
# play_LH (began 2026-09-27 18:40:20)
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=typical_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=594 late=8 early=0 dropped=1198 present_p50_ms=64.2 present_p95_ms=250.2 present_max_ms=341.6 held_max_ms=339.9 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.8 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=921.5 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=594 engine_late=8 engine_dropped=1198 engine_dropped_agent=0 engine_held_max_ms=341.6 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.3 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=851.0 threads=13
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=typical_1080p run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=583 late=9 early=0 dropped=1208 present_p50_ms=64.2 present_p95_ms=259.1 present_max_ms=362.8 held_max_ms=362.4 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.7 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1213.5 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=585 engine_late=7 engine_dropped=1208 engine_dropped_agent=0 engine_held_max_ms=362.8 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.5 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1144.3 threads=13
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=typical_1080p run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=590 late=12 early=0 dropped=1198 present_p50_ms=64.2 present_p95_ms=247.8 present_max_ms=362.4 held_max_ms=359.6 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.6 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1104.1 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=591 engine_late=11 engine_dropped=1198 engine_dropped_agent=0 engine_held_max_ms=362.4 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.1 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=960.5 threads=13
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=blend_heavy_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=847 late=7 early=0 dropped=946 present_p50_ms=63.7 present_p95_ms=170.2 present_max_ms=233.8 held_max_ms=233.3 receipt_offset_max_ms=1966.7 clock_stall_max_ms=45.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1228.0 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=847 engine_late=7 engine_dropped=946 engine_dropped_agent=0 engine_held_max_ms=233.8 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.0 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1220.4 threads=13
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=blend_heavy_1080p run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=671 late=0 early=0 dropped=1129 present_p50_ms=64.3 present_p95_ms=191.9 present_max_ms=298.7 held_max_ms=294.5 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.4 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1281.4 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=671 engine_late=0 engine_dropped=1129 engine_dropped_agent=0 engine_held_max_ms=298.7 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.1 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1077.0 threads=13
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=blend_heavy_1080p run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=877 late=5 early=0 dropped=918 present_p50_ms=63.4 present_p95_ms=150.3 present_max_ms=256.1 held_max_ms=253.5 receipt_offset_max_ms=1966.7 clock_stall_max_ms=45.7 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1322.6 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=875 engine_late=7 engine_dropped=918 engine_dropped_agent=0 engine_held_max_ms=256.1 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.1 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1319.8 threads=13
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=explainer_16x9 run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1480 late=7 early=0 dropped=313 present_p50_ms=42.2 present_p95_ms=106.2 present_max_ms=234.8 held_max_ms=233.6 receipt_offset_max_ms=1966.7 clock_stall_max_ms=45.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1555.8 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1480 engine_late=7 engine_dropped=313 engine_dropped_agent=0 engine_held_max_ms=234.8 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=21.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=4 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1499.9 threads=13
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=explainer_16x9 run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1488 late=16 early=0 dropped=296 present_p50_ms=42.1 present_p95_ms=105.4 present_max_ms=234.8 held_max_ms=233.5 receipt_offset_max_ms=1966.7 clock_stall_max_ms=45.4 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1378.8 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1488 engine_late=16 engine_dropped=296 engine_dropped_agent=0 engine_held_max_ms=234.8 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=21.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=4 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1337.7 threads=12
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=explainer_16x9 run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1482 late=7 early=0 dropped=311 present_p50_ms=42.2 present_p95_ms=106.1 present_max_ms=214.2 held_max_ms=213.2 receipt_offset_max_ms=1966.7 clock_stall_max_ms=45.6 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1400.4 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1481 engine_late=8 engine_dropped=311 engine_dropped_agent=0 engine_held_max_ms=214.2 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=21.8 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=4 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1351.0 threads=13
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=reel_9x16 run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1406 late=12 early=0 dropped=382 present_p50_ms=42.2 present_p95_ms=106.6 present_max_ms=319.9 held_max_ms=319.0 receipt_offset_max_ms=1966.7 clock_stall_max_ms=45.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1404.2 ledger_peak_mib=68.4 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1406 engine_late=12 engine_dropped=382 engine_dropped_agent=0 engine_held_max_ms=319.9 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=21.8 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1353.2 threads=13
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=reel_9x16 run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1395 late=9 early=0 dropped=396 present_p50_ms=42.2 present_p95_ms=107.0 present_max_ms=277.2 held_max_ms=273.4 receipt_offset_max_ms=1966.7 clock_stall_max_ms=45.6 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1605.8 ledger_peak_mib=68.4 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1394 engine_late=10 engine_dropped=396 engine_dropped_agent=0 engine_held_max_ms=277.2 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=21.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1514.8 threads=13
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=reel_9x16 run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1416 late=13 early=0 dropped=371 present_p50_ms=42.2 present_p95_ms=106.5 present_max_ms=277.3 held_max_ms=276.2 receipt_offset_max_ms=1966.7 clock_stall_max_ms=45.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1657.0 ledger_peak_mib=68.4 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1420 engine_late=9 engine_dropped=371 engine_dropped_agent=0 engine_held_max_ms=277.3 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.0 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1538.8 threads=13
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=feed_4x5 run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1194 late=3 early=0 dropped=603 present_p50_ms=42.4 present_p95_ms=150.2 present_max_ms=427.2 held_max_ms=426.0 receipt_offset_max_ms=1966.7 clock_stall_max_ms=45.7 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1553.1 ledger_peak_mib=99.0 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1193 engine_late=4 engine_dropped=603 engine_dropped_agent=0 engine_held_max_ms=427.2 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.1 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1490.0 threads=13
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=feed_4x5 run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1204 late=8 early=0 dropped=588 present_p50_ms=42.4 present_p95_ms=149.2 present_max_ms=960.6 held_max_ms=959.5 receipt_offset_max_ms=1966.7 clock_stall_max_ms=45.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1603.9 ledger_peak_mib=99.0 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1204 engine_late=8 engine_dropped=588 engine_dropped_agent=0 engine_held_max_ms=960.6 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.0 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1472.0 threads=13
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=feed_4x5 run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1215 late=7 early=0 dropped=578 present_p50_ms=42.3 present_p95_ms=169.9 present_max_ms=425.9 held_max_ms=425.4 receipt_offset_max_ms=1966.7 clock_stall_max_ms=45.4 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1580.1 ledger_peak_mib=99.0 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1215 engine_late=7 engine_dropped=578 engine_dropped_agent=0 engine_held_max_ms=425.9 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1475.5 threads=13
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=talk_recut run=0 valid=true elapsed_s=240.01 missed_callbacks=0 due=7200 on_time=6510 late=30 early=0 dropped=660 present_p50_ms=42.0 present_p95_ms=62.6 present_max_ms=256.4 held_max_ms=255.6 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.8 underrun_frames=0 post_end_underrun_frames=0 peak_rss_mib=1479.8 ledger_peak_mib=42.2 table_live_kib=128 passes=false engine_due=7200 engine_on_time=6508 engine_late=32 engine_dropped=660 engine_dropped_agent=0 engine_held_max_ms=256.4 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.0 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=0 engine_sync_decoders=1 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1478.1 threads=13
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=talk_recut run=1 valid=true elapsed_s=240.03 missed_callbacks=0 due=7200 on_time=6502 late=39 early=0 dropped=659 present_p50_ms=42.0 present_p95_ms=63.2 present_max_ms=256.2 held_max_ms=253.8 receipt_offset_max_ms=0.0 clock_stall_max_ms=60.0 underrun_frames=0 post_end_underrun_frames=0 peak_rss_mib=1479.1 ledger_peak_mib=42.2 table_live_kib=128 passes=false engine_due=7200 engine_on_time=6501 engine_late=40 engine_dropped=659 engine_dropped_agent=0 engine_held_max_ms=256.2 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=43.2 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=0 engine_sync_decoders=1 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1478.1 threads=13
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=talk_recut run=2 valid=true elapsed_s=240.01 missed_callbacks=0 due=7200 on_time=6502 late=27 early=0 dropped=671 present_p50_ms=42.0 present_p95_ms=63.3 present_max_ms=235.5 held_max_ms=231.2 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.7 underrun_frames=0 post_end_underrun_frames=0 peak_rss_mib=1479.3 ledger_peak_mib=42.2 table_live_kib=128 passes=false engine_due=7200 engine_on_time=6497 engine_late=32 engine_dropped=671 engine_dropped_agent=0 engine_held_max_ms=235.5 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.2 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=0 engine_sync_decoders=1 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1478.2 threads=12
PF1 control=Slowdown lane=LH workload=typical_1080p metric=G1 fails=true valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=173 late=37 early=0 dropped=1590 present_p50_ms=158.1 present_p95_ms=571.7 present_max_ms=1940.4 held_max_ms=1939.4 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.7 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1502.3 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=175 engine_late=35 engine_dropped=1590 engine_dropped_agent=0 engine_held_max_ms=1940.4 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.0 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1500.4 threads=13
PF1 control=Freeze lane=LH workload=typical_1080p metric=G14 fails=true valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=580 late=11 early=0 dropped=1209 present_p50_ms=64.2 present_p95_ms=256.1 present_max_ms=1106.7 held_max_ms=1106.4 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.7 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1532.0 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=582 engine_late=9 engine_dropped=1209 engine_dropped_agent=0 engine_held_max_ms=1106.7 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.1 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1530.1 threads=12
PF1 control=ClockFreeze lane=LH workload=typical_1080p metric=G16 fails=true valid=true elapsed_s=61.02 missed_callbacks=0 due=1800 on_time=591 late=16 early=0 dropped=1193 present_p50_ms=64.2 present_p95_ms=247.4 present_max_ms=1069.6 held_max_ms=1067.7 receipt_offset_max_ms=0.0 clock_stall_max_ms=1049.8 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1533.4 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=592 engine_late=15 engine_dropped=1193 engine_dropped_agent=0 engine_held_max_ms=1069.6 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=1027.5 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1530.1 threads=13
PF1 control=Stall lane=LH workload=typical_1080p metric=underrun_frames fails=true valid=true elapsed_s=61.02 missed_callbacks=0 due=1800 on_time=585 late=9 early=0 dropped=1206 present_p50_ms=64.2 present_p95_ms=258.9 present_max_ms=1067.1 held_max_ms=1065.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=1045.0 underrun_frames=48128 post_end_underrun_frames=512 peak_rss_mib=1533.1 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=585 engine_late=9 engine_dropped=1206 engine_dropped_agent=0 engine_held_max_ms=1067.1 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=1024.0 engine_underrun_events=47 engine_underrun_frames=48128 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1530.1 threads=13
# exit=0 (play_LH)
# seek_LL (began 2026-09-27 19:14:24)
PF1 seek lane=LL workload=seek_gop60 run=0 random_p95_ms=62.2 random_max_ms=70.4 forward_p95_ms=61.7 plus1_p95_ms=61.7 plus1_n=17 backward_combined_p95_ms=63.7 drag_p95_ms=95.5 drag_answered_p95_ms=95.5 drag_unanswered=0 drag_distinct_fps=24.0 release_pending_drag_calls=2 release_shown=true release_ms=71.9 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=seek_gop60 run=1 random_p95_ms=62.7 random_max_ms=73.7 forward_p95_ms=60.0 plus1_p95_ms=68.1 plus1_n=11 backward_combined_p95_ms=61.5 drag_p95_ms=100.4 drag_answered_p95_ms=100.4 drag_unanswered=0 drag_distinct_fps=23.6 release_pending_drag_calls=1 release_shown=true release_ms=44.1 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=seek_gop60 run=2 random_p95_ms=61.3 random_max_ms=69.9 forward_p95_ms=63.8 plus1_p95_ms=56.5 plus1_n=8 backward_combined_p95_ms=60.1 drag_p95_ms=102.4 drag_answered_p95_ms=102.4 drag_unanswered=0 drag_distinct_fps=24.6 release_pending_drag_calls=1 release_shown=true release_ms=45.1 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=talk_recut run=0 random_p95_ms=92.1 random_max_ms=147.7 forward_p95_ms=106.5 plus1_p95_ms=124.9 plus1_n=17 backward_combined_p95_ms=93.1 drag_p95_ms=122.6 drag_answered_p95_ms=122.6 drag_unanswered=0 drag_distinct_fps=20.8 release_pending_drag_calls=1 release_shown=true release_ms=70.4 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=talk_recut run=1 random_p95_ms=90.6 random_max_ms=178.1 forward_p95_ms=85.9 plus1_p95_ms=92.9 plus1_n=12 backward_combined_p95_ms=88.4 drag_p95_ms=128.5 drag_answered_p95_ms=128.5 drag_unanswered=0 drag_distinct_fps=19.4 release_pending_drag_calls=2 release_shown=true release_ms=125.4 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=talk_recut run=2 random_p95_ms=107.4 random_max_ms=172.8 forward_p95_ms=86.5 plus1_p95_ms=89.5 plus1_n=8 backward_combined_p95_ms=86.5 drag_p95_ms=137.1 drag_answered_p95_ms=137.1 drag_unanswered=0 drag_distinct_fps=18.8 release_pending_drag_calls=2 release_shown=true release_ms=85.2 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=explainer_16x9 run=0 random_p95_ms=129.9 random_max_ms=180.6 forward_p95_ms=128.9 plus1_p95_ms=135.7 plus1_n=17 backward_combined_p95_ms=132.3 drag_p95_ms=234.4 drag_answered_p95_ms=234.4 drag_unanswered=0 drag_distinct_fps=14.6 release_pending_drag_calls=2 release_shown=true release_ms=99.2 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=explainer_16x9 run=1 random_p95_ms=140.3 random_max_ms=161.9 forward_p95_ms=129.4 plus1_p95_ms=125.5 plus1_n=11 backward_combined_p95_ms=136.5 drag_p95_ms=175.6 drag_answered_p95_ms=175.6 drag_unanswered=0 drag_distinct_fps=15.6 release_pending_drag_calls=2 release_shown=true release_ms=174.0 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=explainer_16x9 run=2 random_p95_ms=131.1 random_max_ms=169.3 forward_p95_ms=125.5 plus1_p95_ms=120.1 plus1_n=8 backward_combined_p95_ms=127.4 drag_p95_ms=224.0 drag_answered_p95_ms=224.0 drag_unanswered=0 drag_distinct_fps=15.4 release_pending_drag_calls=1 release_shown=true release_ms=85.3 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
# exit=0 (seek_LL)
# seek_LH (began 2026-09-27 19:22:01)
PF1 seek lane=LH workload=seek_gop60 run=0 random_p95_ms=57.8 random_max_ms=66.1 forward_p95_ms=62.6 plus1_p95_ms=61.9 plus1_n=17 backward_combined_p95_ms=61.6 drag_p95_ms=98.9 drag_answered_p95_ms=98.9 drag_unanswered=0 drag_distinct_fps=25.0 release_pending_drag_calls=1 release_shown=true release_ms=99.3 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=seek_gop60 run=1 random_p95_ms=62.1 random_max_ms=69.0 forward_p95_ms=63.5 plus1_p95_ms=67.3 plus1_n=11 backward_combined_p95_ms=59.5 drag_p95_ms=99.7 drag_answered_p95_ms=99.7 drag_unanswered=0 drag_distinct_fps=24.2 release_pending_drag_calls=1 release_shown=true release_ms=36.9 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=seek_gop60 run=2 random_p95_ms=60.8 random_max_ms=67.8 forward_p95_ms=59.9 plus1_p95_ms=54.8 plus1_n=8 backward_combined_p95_ms=58.4 drag_p95_ms=96.7 drag_answered_p95_ms=96.7 drag_unanswered=0 drag_distinct_fps=25.2 release_pending_drag_calls=1 release_shown=true release_ms=39.1 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=talk_recut run=0 random_p95_ms=89.0 random_max_ms=146.9 forward_p95_ms=107.7 plus1_p95_ms=134.7 plus1_n=17 backward_combined_p95_ms=98.9 drag_p95_ms=120.3 drag_answered_p95_ms=120.3 drag_unanswered=0 drag_distinct_fps=20.6 release_pending_drag_calls=1 release_shown=true release_ms=53.9 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=talk_recut run=1 random_p95_ms=89.8 random_max_ms=153.7 forward_p95_ms=82.7 plus1_p95_ms=82.4 plus1_n=12 backward_combined_p95_ms=92.5 drag_p95_ms=127.8 drag_answered_p95_ms=127.8 drag_unanswered=0 drag_distinct_fps=19.6 release_pending_drag_calls=2 release_shown=true release_ms=128.5 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=talk_recut run=2 random_p95_ms=107.4 random_max_ms=155.7 forward_p95_ms=86.2 plus1_p95_ms=87.2 plus1_n=8 backward_combined_p95_ms=87.1 drag_p95_ms=130.7 drag_answered_p95_ms=130.2 drag_unanswered=1 drag_distinct_fps=19.0 release_pending_drag_calls=3 release_shown=true release_ms=70.4 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=explainer_16x9 run=0 random_p95_ms=124.9 random_max_ms=174.0 forward_p95_ms=127.1 plus1_p95_ms=137.1 plus1_n=17 backward_combined_p95_ms=131.8 drag_p95_ms=228.1 drag_answered_p95_ms=227.3 drag_unanswered=1 drag_distinct_fps=14.6 release_pending_drag_calls=2 release_shown=true release_ms=72.9 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=explainer_16x9 run=1 random_p95_ms=140.3 random_max_ms=164.4 forward_p95_ms=121.6 plus1_p95_ms=118.8 plus1_n=11 backward_combined_p95_ms=131.9 drag_p95_ms=188.7 drag_answered_p95_ms=172.0 drag_unanswered=2 drag_distinct_fps=15.8 release_pending_drag_calls=4 release_shown=true release_ms=102.7 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=explainer_16x9 run=2 random_p95_ms=130.3 random_max_ms=162.1 forward_p95_ms=126.0 plus1_p95_ms=120.4 plus1_n=8 backward_combined_p95_ms=125.3 drag_p95_ms=237.8 drag_answered_p95_ms=237.0 drag_unanswered=1 drag_distinct_fps=15.0 release_pending_drag_calls=2 release_shown=true release_ms=55.0 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
# exit=139 (seek_LH)
```

### E11.11 R33 re-review fixes and reruns

The re-reviews of `87f25d5` were A (reject) and B (accept with fixes). They
were addressed in six code commits on `pf1/impl`:

| Commit | What it does |
|---|---|
| `845ac31` | Ack accounting, expiry judged at paint, and resume from the paint ack (A-D1, A-D2, A-D3 = B-D1, A-D4 = B-D5, A-N1, B-D3) |
| `9cb27a0` | Underruns and stalls classified against the consumed programme end (A-D5 = B-D4) |
| `b466842` | The I8 consumer checks expiry against its own transport view (A-D6) |
| `9858d8b` | Off-UI replies bind only into the context they were asked for (B-D2) |
| `d3a4454` | A complete harness teardown before RSS (B-D6), and review B's witness nits |
| `cb36a53` | An ack samples a coherent transport snapshot. This corrects `845ac31`'s A-D3 fix, after the first timing rerun exposed it (E11.11.4). *Superseded by R34 (`9a3c583`, E11.12): the snapshot is deleted and the worker is the only writer.* |

Each commit passed its gates:
- build, `clippy --workspace --all-targets -D warnings`, and rustfmt on the
  touched files (`skip_children`);
- the affected crates' tests, with media at the default thread count;
- `cargo build -p kinewright-app`.

None of the findings is disputed. C-5 and C-3 still hold:
- no render, conversion or compositor path changed; `compositor.rs` gains
  only a test-only ledger handle;
- the byte-exact suites are unedited and green;
- the monitor's pixels are unchanged, and no tolerance was added.

Agent-protocol replies and MCP shapes are unchanged:
- `call_tool`'s cancellation is refactored into `cancellable_blocking`
  with the same behaviour;
- no new error was added.

#### E11.11.1 Findings

"Fails on the old code" names the witness that fails when a mutation
reverts the fix (E11.11.2).

| Finding | Fix | Witness (fails on the old code) |
|---|---|---|
| A-D3 = B-D1 ack samples the caller clock | The ack reads the newest issued epoch and the clock together under the coalesced lock. It samples that position only if the epoch is the applied, playing one; the A/V offset is taken against that snapshot (E11.8, `cb36a53`). `845ac31` first made the ack settle-only. That regressed the engine's on-time count, because an image painted before the worker's tick reached its frame found no record. The first P-play LL rerun showed it (E11.11.4). | `engine::tests::an_ack_racing_an_unapplied_seek_manufactures_no_due_frames`. Frame 10 is painted, `seek(900)` is issued, and the paint is acked through the production path before the worker applies the seek (M1, M26). Regression witnesses: `engine::tests::an_ack_ahead_of_the_workers_tick_counts_on_time` (production path) and `stats::tests::an_ack_ahead_of_the_tick_samples_its_applied_snapshot` (M25) |
| A-D4 = B-D5 eviction identity | Evicted records keep per-epoch identity (`Evicted { frames, settled }`). The first ack of an evicted frame is late. A duplicate, a never-due key or another epoch's key counts nothing (E11.8). | `stats::tests::an_evicted_record_acknowledged_later_is_late`. Cases: duplicate; never due, past the clock; never due, before the start; an epoch that registered nothing; acked before its eviction; each evicted frame once (M2, M3) |
| A-D5 = B-D4 consumed programme end | `AudioRuntime::open` sets the programme end, `frame_to_samples(duration)`. The callback classifies its failed pops by the sample it consumed through (position before the pop + popped). The ongoing clock stall is measured until the callbacks consume the programme or playback stops. The `programme_pushed` flag is removed. The callback adds only atomic loads and stores: no lock and no allocation. | `audio::tests::a_tail_pushed_during_a_callback_underruns_after_the_end`: the producer pushes the final tail between two callbacks (M9). *Qualified in R34 (re-review 2 nit):* the push lands between two complete callback calls, not inside a preempted one, so the witness checks the endpoint arithmetic, not a concurrent interleaving. A barrier inside a preempted callback would need the callback restructured around a test hook (it is one uninterrupted call into `render_output`); the lock- and allocation-free classification is argued from the code, not witnessed. `engine::tests::a_freeze_with_the_tail_queued_is_a_stall`: the whole programme is pushed and the callbacks freeze with the tail queued (M10, M11) |
| A-D1 residual window after the final check | E11.8 amendment. The paint marker reads the clock at paint, and a frame the clock has passed is acked `expired`. R-5 counts it late. | `presenter::tests::a_frame_expiring_after_its_last_check_is_acked_expired`: the clock passes frame 10 before the binding is written and before the paint (M5). `stats::tests::a_frame_expired_at_its_paint_is_late` (M4) |
| A-D2 resume before the target is painted | `release_scrub`'s resume waits for the matching paint ack at a later root epoch (`App::acknowledge_paint`). A binding is not enough. | `app::tests::a_released_scrub_resumes_only_once_its_target_is_painted`. It goes through real paint, a discarded pass, egui's `request_discard`, an invisible root and an older epoch's image, then to EOS (M6: "bound, never painted"). *Qualified in R34 (re-review 2 nit):* the paint is real, but the terminal events are injected through `RecordingPlayback`; the witness does not run an engine to EOS, so it does not cover the engine's own EOS behaviour (which the engine's V-2 tests cover separately). |
| A-D6 I8 consumer expiry | `Model::observe` tracks the transport from the `PlaybackStateChanged` events, and it is asserted equal to the worker's after every step. `Model::consume` rejects a playing frame whose `at` the clock has passed (`expired_rejected`). The release phase advances the clock before consumption at random. | The five I8 shards, with the `expired_rejected` floor (M12) and the transport-view assertion (M13) |
| A-N1 egui discard | `Pass::EguiDiscard`: a real `request_discard` multipass through egui's `Context::run` | `the_real_paint_callback_acks_only_painted_images`: 2 layouts, nothing acked. This is a coverage addition: the old code already acked nothing here. |
| B-D2 off-UI reply identity | Agent threads get a process-unique id. `BranchReview { generation, thread_id, stamp, document }` binds only when all of these hold: its thread is found by id in the focused project; the branch is still at the rendered document (`Arc::ptr_eq`); there was no transport call since (the playback stamp); and no newer review exists. Otherwise the reply is discarded, with no presenter clear and no provenance. A cache-inventory reply applies only if no newer refresh was issued. | `chat_ui::tests::a_delayed_branch_frame_binds_only_into_its_own_viewer`. Cases: an earlier thread closes; the thread itself is removed; a newer review; a seek; a branch edit; a project switch (M14–M17). `app::tests::a_superseded_cache_inventory_reply_is_discarded` (M18) |
| B-D3 `dropped_agent` | `AgentWindow { epoch, generation, registered, newest }`. The job is charged the due frames the worker registered in its own epoch while it ran. A seek, pause or replay charges only the frames before it. A `clear` bumps the generation, so the old job charges nothing. | `stats::tests::an_agent_job_is_charged_only_its_epochs_registered_frames`; `preview::tests::an_agent_job_counts_the_due_frames_it_displaces`, with advance, seek, pause and replay cases (M7, M8) |
| B-D6 teardown boundary | Sequence: the harness keeps only telemetry handles (the GPU ledger and the decoder gauge) and drops the rest of the session. It waits on the worker thread's test-only `finished` signal (sent after the worker joins the preview and drops itself), drops the `GpuContext`, and only then samples RSS. E11.10.5 is qualified. | `engine::tests::dropping_the_engine_stops_the_preview_thread` now waits on `finished`, then requires the frame channel disconnected (M24). Figures are in E11.11.4. |
| B nit: cancellation witnesses | `cancellable_blocking` | `server::cancel_tests::an_abandoned_call_cancels_its_handler` (M20); `a_client_cancellation_cancels_its_handler_and_awaits_it`; core `agent_cancel_scopes_nest_and_restore_on_unwind` (M21) |
| B nit: `live_table_bytes` | Production field asserted | `sync_decoders_counts_open_decoders_until_teardown`: at least 128 KiB after an SDR render, a whole number of tables, no underflow after teardown (M22) |
| B nit: P-seek stale answer | `wait_answer(frames, target, issued, …)` | `pf1_harness::pf1_a_repeated_seek_target_is_answered_only_by_its_own_call` (M23) |
| B nit and A nit: wording | Changes in E11.8 and E11.10: the tick witness is "sequential stepping", not a barrier; held-age density is qualified; E11.10's closure claims are qualified; the RSS attribution is "consistent with allocator or driver retention", not proof; "neither needs an engine change" is withdrawn. | — |

#### E11.11.2 Broken variants (R33)

Each mutation was applied to the fixed tree and its witness run, then the
mutation was reverted. The app crate has no library target, so its
mutations run `--bins`. Every mutation fails its witness, except M19.

M1–M24 ran on `d3a4454`. M25 and M26 ran on `cb36a53`, whose change is
confined to the ack. On `cb36a53`, M26 is M1's counterpart: the caller
clock sampled whatever its epoch.

| # | Mutation (the old behaviour) | Fails |
|---|---|---|
| M1 | The ack samples the caller-side clock | `an_ack_racing_an_unapplied_seek_…`: 901 due against 11; offset 29,666.7 ms |
| M2 | Evicted keys: no dedup | `an_evicted_record_…`: "a duplicate" (0, 2) against (0, 1) |
| M3 | Evicted keys: only a frontier check | the same test: "never due: before the start" |
| M4 | The counters ignore `expired` | `a_frame_expired_at_its_paint_is_late`: (2, 0) against (1, 1) |
| M5 | The marker is never expired | `a_frame_expiring_after_its_last_check_is_acked_expired` |
| M6 | Resume on binding (the old `Presenter::shows`) | `a_released_scrub_resumes_only_once_its_target_is_painted`: "bound, never painted" issues `play(29)` |
| M7 | The agent window takes any epoch | `an_agent_job_counts_…`: "seek" 8 against 2 |
| M8 | The agent is charged across a `clear` | the same test: "replay" 2 against 0 |
| M9 | Underruns classified by the pre-pop position (the old flag read before the pop) | `a_tail_pushed_during_a_callback_…`: [2, 4, 0, 0] against [1, 2, 1, 2] |
| M10 | Stall measurement ends at the producer's last push | `a_freeze_with_the_tail_queued_is_a_stall`: "frozen with the tail queued: 0 ms" |
| M11 | The ongoing stall never ends | the same test: 300.05 ms of idle after the end counted |
| M12 | I8 consumes without the expiry check (the old consume) | all 5 I8 shards: `expired_rejected` 0, below its floor |
| M13 | No `Paused` event at EOS | all 5 I8 shards: "the state events" (the consumer's transport view diverges) |
| M14 | The old reply: project id + captured index | `a_delayed_branch_frame_…`: provenance (0, 1) against (1, 0), misattributed to the next thread |
| M15 | No review generation | the same test: "superseded" |
| M16 | No transport-stamp check | the same test: "a transport call since" |
| M17 | No branch-document identity | the same test: "the branch moved on" |
| M18 | No inventory generation | `a_superseded_cache_inventory_reply_is_discarded`: "superseded" |
| M19 | No project-id check | **survived**. It is redundant with the process-unique thread id, which is looked up in the focused project, so the check was removed. |
| M20 | No cancel on abandonment | `an_abandoned_call_cancels_its_handler`: "dropping the future cancelled it" |
| M21 | The scope restores only on return | `agent_cancel_scopes_nest_and_restore_on_unwind`: "restored by the unwind" |
| M22 | `stats().live_table_bytes` set to 0 | `sync_decoders_counts_open_decoders_until_teardown`: 0 |
| M23 | `wait_answer` ignores the stamp | `pf1_a_repeated_seek_target_is_answered_only_by_its_own_call`: answered by the stale frame, 3 left against 1 |
| M24 | `finished` sent before the teardown | `dropping_the_engine_stops_the_preview_thread`: frames `Empty` against `Disconnected` |
| M25 | A settle-only ack (`845ac31`: the snapshot is never sampled) | `an_ack_ahead_of_the_workers_tick_counts_on_time`, `an_ack_ahead_of_the_tick_samples_its_applied_snapshot` and `due_frames_are_counted_once_by_their_ack` |
| M26 | The snapshot is sampled whatever its epoch | `an_ack_racing_an_unapplied_seek_manufactures_no_due_frames` and `an_ack_ahead_of_the_tick_samples_its_applied_snapshot` ("a seek not applied") |

#### E11.11.3 I8 coverage (R33)

`expired_rejected` (new) per fake shard is 47 / 44 / 41 / 53 (floor 10),
and 7 on real audio (floor 1). The other floors are as in E11.10.2.

#### E11.11.4 Timing reruns (R33)

**Provenance.**
- Binary: the release test binary of `cb36a53`
  (`kinewright_media-cf306d076b2fd4bf`, sha256
  `8aa5a945d84b443ac7198a425a072ec131bcce393cf0532f2a4aaac5426453de`). It
  was built once and copied aside, so every lane ran the same binary.
  rustc 1.98.0; the machine, drivers and FFmpeg are as in E11.1.
- When: 2026-09-27, 21:29:45–22:40:34 EDT. One runner ran the three lanes
  in sequence, each alone, with no build, test or other lane alongside:

  | Lane | Window (EDT) |
  |---|---|
  | G3 LH | 21:29:45–21:32:23 |
  | P-play LL | 21:32:23–22:06:28 |
  | P-play LH | 22:06:28–22:40:34 |

- Output was logged unfiltered.
  - The P-play lanes are `PF1_RUNS=3` with no `PF1_ONLY`: every workload
    ran 3 times, then the four Q-3 controls ran once, in one process per
    lane.
- Every lane printed `test result: ok. 1 passed` and exited 0. No exit-time
  panic or SIGSEGV this time.
- **Ambient load (R26).**
  - The two `foot` screensaver processes (≈91% + 51%) and Hyprland (≈24%)
    were present at every BEGIN/END. These `ps` figures are lifetime
    averages.
  - No `gh` process was present at any mark.
  - 1-minute load averages were 4.6–9.3.
- **A first rerun, superseded (E11.11.6).**
  - It ran on the `d3a4454` binary (sha256
    `2304016d56a81291ebb2c242a6b0d84abd0d08a74dddab7a2254904878337ed9`),
    20:43:01–21:22:57 EDT: G3 LH passed, and P-play LL completed.
  - Its engine columns exposed a regression in `845ac31`'s settle-only
    ack. Engine on time was 119–165 of 1,800 in every 60 s workload
    (614–639 of 7,200 for `talk_recut`), against the harness's 568–1,463.
    `engine_av_offset_max_ms` was 33.3 in every run.
    - Cause: an image painted before the worker's 5 ms tick reached its
      frame found no record, and the tick then registered the frame
      unacknowledged.
  - Its P-play LH lane was stopped 3 minutes in, and `cb36a53` fixed the
    ack (E11.8, E11.11.1).
  - The harness columns, G3, G11 and G16 of that rerun agree with the
    lanes below.

**G3** (60 fps floor, LH):

| Build | `blend_heavy_1080p` fps (3 runs) | mean ms | p95 ms | Result |
|---|---|---|---|---|
| R32 `af5abf9` (E11.10.4) | 71.7 / 71.2 / 71.8 | 13.92–14.04 | 15.37–15.76 | passes |
| R33 `cb36a53` | 71.6 / 72.2 / 71.2 | 13.85–14.05 | 15.18–15.63 | **passes** |

- `typical_1080p` reads 73.3 fps, and `heavy_4k` 58.7–59.1 fps.
- The slowdown control fails its verdict (17.8 fps), as it must.
- Phases for `blend_heavy_1080p`:
  - `upload_ms` 10.56;
  - `gpu_passes_readback_ms` 1.43;
  - `monitor_encode_ms` 1.72.
- The superseded `d3a4454` rerun read 72.3 / 70.7 / 72.1.

**P-play, G11 and G16 (simulated driver).** The columns are as in
E11.10.4.
- Every run is valid: 60.02 s (240.0 s for `talk_recut`), with 0 missed
  callbacks.
- `engine_av_offset_max_ms` is 0.0 in every run.
- `engine_dropped_agent` is 0, and `engine_stale_errors` is 0.

| Workload | Lane | Runs | Harness on time / late / dropped | Engine due: on time / late / dropped | Present p50 / p95 / max ms | Held max ms (engine) | Clock stall max ms, harness / engine | Underrun frames, harness / engine | Post-end frames | Receipt offset max ms | Rejected at consumer | Sync decoders |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `typical_1080p` | LL | 3 | 577–581 / 4–5 / 1215–1219 | 1800: 576–580 / 5–7 / 1215–1219 | 64.2 / 253.9–259.7 / 319.8–362.0 | 319.8–378.0 | 47.3–47.8 / 22.9–24.4 | 0 / 0 | 512 | 0.0 | 0 | 2 |
| `blend_heavy_1080p` | LL | 3 | 645–655 / 3–8 / 1137–1149 | 1800: 642–655 / 3–9 / 1137–1149 | 64.4–64.5 / 192.6–195.2 / 299.4–340.9 | 299.4–340.9 | 46.7–47.3 / 22.9–23.4 | 0 / 0 | 512 | 0.0–1966.7 | 0–1 | 3 |
| `explainer_16x9` | LL | 3 | 1441–1465 / 1–7 / 330–358 | 1800: 1442–1464 / 0–7 / 330–358 | 42.2–42.3 / 106.1–106.3 / 234.0–234.6 | 234.0–234.6 | 45.9–46.7 / 22.8–23.1 | 0 / 0 | 512 | 1966.7 | 1 | 4 |
| `reel_9x16` | LL | 3 | 1313–1340 / 3–8 / 456–480 | 1800: 1312–1339 / 4–8 / 456–480 | 42.3 / 107.0–107.4 / 319.0–341.3 | 319.0–341.3 | 47.6–48.2 / 22.8–23.3 | 0 / 0 | 512 | 1966.7 | 1 | 3 |
| `feed_4x5` | LL | 3 | 965–987 / 3–6 / 807–832 | 1800: 965–987 / 3–6 / 807–832 | 42.8–42.9 / 170.2–171.1 / 729.5–1367.1 | 729.5–1367.1 | 49.2–49.7 / 22.9–23.4 | 0 / 0 | 512 | 1966.7 | 1 | 3 |
| `talk_recut` | LL | 3 | 6438–6474 / 14–23 / 703–748 of 7200 | 7200: 6432–6472 / 20–28 / 703–748 | 42.1 / 65.4–69.7 / 235.1–256.0 | 235.1–256.0 | 59.5–60.0 / 42.9–43.1 | 0 / 0 | 0 | 0.0 | 0 | 1 |
| `typical_1080p` | LH | 3 | 574–575 / 4–8 / 1218–1221 | 1800: 572–575 / 5–10 / 1218–1221 | 64.2 / 258.9–271.6 / 320.8–384.6 | 335.8–384.6 | 45.5–46.3 / 22.0–22.3 | 0 / 0 | 512 | 0.0 | 0 | 2 |
| `blend_heavy_1080p` | LH | 3 | 690–821 / 5–11 / 968–1103 | 1800: 689–824 / 5–8 / 968–1103 | 63.8–64.3 / 171.0–190.9 / 255.9–277.4 | 255.9–277.4 | 45.5–46.3 / 22.0–22.4 | 0 / 0 | 512 | 0.0–1966.7 | 0–1 | 3 |
| `explainer_16x9` | LH | 3 | 1486–1499 / 7–16 / 285–307 | 1800: 1486–1497 / 7–18 / 285–307 | 42.1–42.2 / 98.2–106.1 / 234.5–235.3 | 234.5–235.3 | 45.5–46.2 / 21.8–21.9 | 0 / 0 | 512 | 1966.7 | 1 | 4 |
| `reel_9x16` | LH | 3 | 1403–1416 / 8–10 / 376–387 | 1800: 1404–1416 / 8–9 / 376–387 | 42.1 / 106.6–106.8 / 298.7–320.3 | 298.7–320.3 | 45.5–45.6 / 21.9–22.4 | 0 / 0 | 512 | 1966.7 | 1 | 3 |
| `feed_4x5` | LH | 3 | 1180–1208 / 4–6 / 588–614 | 1800: 1180–1206 / 5–6 / 588–614 | 42.3–42.4 / 149.3–169.9 / 426.5–1194.6 | 426.5–1194.6 | 45.5–45.6 / 22.0–22.4 | 0 / 0 | 512 | 1966.7 | 1 | 3 |
| `talk_recut` | LH | 3 | 6514–6522 / 28–43 / 638–658 of 7200 | 7200: 6516–6520 / 25–45 / 638–659 | 42.0 / 62.8–63.1 / 235.2–256.3 | 235.2–256.3 | 45.7–60.1 / 22.0–42.9 | 0 / 0 | 0 | 0.0–33.3 | 0–1 | 1 |

Q-3 controls (`typical_1080p`, one run each):

| Control | Lane | Metric | Fails, as it must | Clock stall max ms, harness / engine | Held max ms | Underrun frames (engine events) / post-end | On time, harness / engine |
|---|---|---|---|---|---|---|---|
| Slowdown | LL | G1 | yes | 47.9 / 22.9 | 1514.7 | 0 (0) / 512 | 121 / 122 |
| Freeze | LL | G14 | yes | 47.8 / 22.9 | 1091.2 | 0 (0) / 512 | 550 / 552 |
| ClockFreeze | LL | G16 | yes | 1050.0 / 1027.2 | 1091.4 | 0 (0) / 512 | 571 / 570 |
| Stall | LL | underruns | yes | 1045.0 / 1024.0 | 1065.3 | 48128 (47) / 512 | 574 / 574 |
| Slowdown | LH | G1 | yes | 45.4 / 22.8 | 1055.8 | 0 (0) / 512 | 178 / 183 |
| Freeze | LH | G14 | yes | 45.7 / 23.1 | 1122.7 | 0 (0) / 512 | 571 / 569 |
| ClockFreeze | LH | G16 | yes | 1050.0 / 1026.8 | 1086.5 | 0 (0) / 512 | 593 / 592 |
| Stall | LH | underruns | yes | 1045.0 / 1024.0 | 1063.8 | 48128 (47) / 512 | 586 / 587 |

- **G11 passes (amended: 0 underruns before the programme end).** The
  consumed-end classification (A-D5 = B-D4) changes nothing in steady
  state:
  - every run reads 0 underrun frames before the end, in the harness and
    in the engine;
  - the post-end straddle is 512 in every 60 s run and 0 in
    `talk_recut`, as in R32;
  - the stall control still reads 48,128 frames in 47 events before the
    end, plus 512 post-end.
- **G16 passes on both lanes.**
  - The harness's max clock stall is 45.5–60.1 ms in every run, against
    the 100 ms gate.
  - The clock-freeze control fails on both lanes (1050.0 ms each).
  - The engine's stall figure is still one callback period in steady
    state (21.8–24.4 ms). `talk_recut` reads 42.9–43.1 ms, as in R32. It is 1024.0 / 1026.8–1027.2 ms in the stall
    and clock-freeze controls. The ongoing-stall measurement until
    consumption therefore adds nothing at the end of a clean play-out.
- **Engine and harness agree.**
  - On time and late agree within ±6 in every run. The ±6 is on
    `talk_recut`'s 7,200 frames; elsewhere it is within ±5, as in R32.
  - Engine late is 0–45.
  - Engine due is exactly 1,800 (7,200).
- The other figures are within run-to-run noise of R32 (E11.10.4): held
  max, present percentiles, receipt offset, rejected arrivals and sync
  decoders.
  - `blend_heavy_1080p` on LH again spreads widely (690–821 on time), as
    in R32 (671–877).

**Teardown and RSS (B-D6).** Since `d3a4454` each run's teardown record
is read at a new boundary. The worker has finished its whole teardown,
the harness holds only telemetry handles, and the `GpuContext` is
dropped. Only then is RSS sampled.

| Lane | Complete | Ledger live | Sync decoders | Conversion tables live | Threads | Teardown RSS MiB across the process's runs (in order) |
|---|---|---|---|---|---|---|
| LL | true in all 22 | 0 KiB | 0 | 128 KiB | 44 in every run (R32: 86–88) | 708 → 846 → 989 (typical) → 1109–1351 (blend) → 1440–1899 (explainer) → 1831–2216 (reel) → 2279–2569 (feed) → 2315–2330 (talk) → 2116–2262 (controls) |
| LH | true in all 22 | 0 KiB | 0 | 128 KiB | 7 in every run (R32: 12–13) | 573 → 726 → 728 (typical) → 879–1058 (blend) → 1101–1193 (explainer) → 1312–1347 (reel) → 1312–1339 (feed) → 1337 (talk) → 1389–1493 (controls) |

- Dropping the device releases its threads. The llvmpipe rasterizer
  threads go on LL (44 against 86–88), and the NVIDIA driver's threads on
  LH (7 against 12–13).
- Teardown RSS is lower than at R32's boundary, where RSS was sampled with
  the device held:
  - LL levels off at about 2.2–2.3 GiB, against 2.7–2.9 GiB;
  - LH levels off at about 1.31–1.34 GiB before the controls, against
    about 1.48–1.53 GiB.
- It still grows run over run, then plateaus, and it falls between some
  runs:
  - LL 2569 → 2365 (feed);
  - LH 1058 → 879 (blend).
- With no device, no decoder, no engine thread and a constant 128 KiB of
  tables left, this is consistent with allocator or driver retention
  (freed arenas, driver caches that outlive the device). It is not proof:
  an allocation that no counter tracks would look the same.
- P-rss is still measured one process per workload, at S4 (E11.4).
- The first run's peak RSS is 856.3 MiB (LL) and 914.4 MiB (LH). E11.4's
  fresh-process figures were 891.5 and 921.5.

#### E11.11.5 Line delta

Non-blank, non-comment `.rs` lines over `87f25d5..cb36a53`:

| Crate | Added | Removed | Of the added: tests | Of the added: harness | Of the added: production |
|---|---|---|---|---|---|
| core | 27 | 1 | 19 | — | 8 |
| media | 613 | 157 | 263 | 63 | 287 |
| agent | 70 | 20 | 44 | — | 26 |
| app | 434 | 176 | 219 | — | 215 |
| **Total** | **1,144** | **354** | **545** | **63** | **536** |

- Tests are the `#[cfg(test)]` test modules. Test-only items outside
  them, such as `finished` and `shared_ledger`, count as production.
- `git diff --stat 87f25d5..cb36a53 -- crates`: 14 files changed, 1,488
  insertions and 434 deletions.
- The docs are extra.

#### E11.11.6 Raw result lines (R33)

`#` lines are the runner's lane markers (with `ps` and load averages) and
exit statuses. Everything else is verbatim from `timing.log`, `cb36a53`:

```
# BEGIN G3 LH 2026-09-27 21:29:45 EDT load: 4.57 6.07 6.31
#      1423 24.1  2-03:47:23 Hyprland
#    180308  0.0  2-03:16:59 foot
#   3754322 91.1    08:40:04 foot
#   3754361 51.1    08:40:04 foot

running 1 test
test mo2_perf_fixtures::blend_heavy_holds_floors_on_hardware ... R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=typical_1080p run=0 dims=(1280, 720) mean_ms=13.65 fps=73.3 p95_ms=15.36 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=typical_1080p run=1 dims=(1280, 720) mean_ms=13.70 fps=73.0 p95_ms=15.41 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=typical_1080p run=2 dims=(1280, 720) mean_ms=13.61 fps=73.5 p95_ms=15.29 ledger_peak_mib=56.3 validate_us=1.3
R28 phases workload=typical_1080p upload_ms=10.06 gpu_passes_readback_ms=1.32 monitor_encode_ms=1.68
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=blend_heavy_1080p run=0 dims=(1280, 720) mean_ms=13.96 fps=71.6 p95_ms=15.19 ledger_peak_mib=63.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=blend_heavy_1080p run=1 dims=(1280, 720) mean_ms=13.85 fps=72.2 p95_ms=15.18 ledger_peak_mib=63.3 validate_us=1.6
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=blend_heavy_1080p run=2 dims=(1280, 720) mean_ms=14.05 fps=71.2 p95_ms=15.63 ledger_peak_mib=63.3 validate_us=1.5
R28 phases workload=blend_heavy_1080p upload_ms=10.56 gpu_passes_readback_ms=1.43 monitor_encode_ms=1.72
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=heavy_4k run=0 dims=(1280, 720) mean_ms=16.97 fps=58.9 p95_ms=18.67 ledger_peak_mib=70.3 validate_us=10.0
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=heavy_4k run=1 dims=(1280, 720) mean_ms=16.93 fps=59.1 p95_ms=18.13 ledger_peak_mib=70.3 validate_us=10.0
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=heavy_4k run=2 dims=(1280, 720) mean_ms=17.03 fps=58.7 p95_ms=18.73 ledger_peak_mib=70.3 validate_us=16.8
R28 phases workload=heavy_4k upload_ms=13.44 gpu_passes_readback_ms=1.66 monitor_encode_ms=1.78
R28 control=slowdown delay_ms=41.7 mean_ms=56.31 p95_ms=58.14 verdict=Err("17.8 fps (floor 60), p95 58.1 ms vs mean 56.3 ms")
ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 946 filtered out; finished in 157.57s

# exit=0
# END G3 LH 2026-09-27 21:32:23 EDT load: 8.54 7.50 6.84
#      1423 24.1  2-03:50:00 Hyprland
#    180308  0.0  2-03:19:37 foot
#   3754322 91.1    08:42:42 foot
#   3754361 51.1    08:42:42 foot
# BEGIN P-play LL 2026-09-27 21:32:23 EDT load: 8.54 7.50 6.84
#      1423 24.1  2-03:50:00 Hyprland
#    180308  0.0  2-03:19:37 foot
#   3754322 91.1    08:42:42 foot
#   3754361 51.1    08:42:42 foot

running 1 test
test pf1_harness::pf1_play_baseline ... PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=typical_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=579 late=5 early=0 dropped=1216 present_p50_ms=64.2 present_p95_ms=257.4 present_max_ms=362.0 held_max_ms=361.7 receipt_offset_max_ms=0.0 clock_stall_max_ms=47.8 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=856.3 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=577 engine_late=7 engine_dropped=1216 engine_dropped_agent=0 engine_held_max_ms=362.0 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=23.2 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=707.7 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=typical_1080p run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=577 late=4 early=0 dropped=1219 present_p50_ms=64.2 present_p95_ms=259.7 present_max_ms=349.6 held_max_ms=380.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=47.3 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1159.9 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=576 engine_late=5 engine_dropped=1219 engine_dropped_agent=0 engine_held_max_ms=378.0 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=24.4 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=845.6 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=typical_1080p run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=581 late=4 early=0 dropped=1215 present_p50_ms=64.2 present_p95_ms=253.9 present_max_ms=319.8 held_max_ms=317.8 receipt_offset_max_ms=0.0 clock_stall_max_ms=47.4 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1257.3 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=580 engine_late=5 engine_dropped=1215 engine_dropped_agent=0 engine_held_max_ms=319.8 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=989.2 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=blend_heavy_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=651 late=3 early=0 dropped=1146 present_p50_ms=64.4 present_p95_ms=195.2 present_max_ms=340.9 held_max_ms=339.8 receipt_offset_max_ms=1966.7 clock_stall_max_ms=46.8 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1387.0 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=651 engine_late=3 engine_dropped=1146 engine_dropped_agent=0 engine_held_max_ms=340.9 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=23.4 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1109.2 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=blend_heavy_1080p run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=645 late=6 early=0 dropped=1149 present_p50_ms=64.4 present_p95_ms=195.1 present_max_ms=299.4 held_max_ms=296.0 receipt_offset_max_ms=1966.7 clock_stall_max_ms=46.7 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1469.7 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=642 engine_late=9 engine_dropped=1149 engine_dropped_agent=0 engine_held_max_ms=299.4 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1210.8 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=blend_heavy_1080p run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=655 late=8 early=0 dropped=1137 present_p50_ms=64.5 present_p95_ms=192.6 present_max_ms=340.8 held_max_ms=338.2 receipt_offset_max_ms=0.0 clock_stall_max_ms=47.3 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1656.6 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=655 engine_late=8 engine_dropped=1137 engine_dropped_agent=0 engine_held_max_ms=340.8 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=23.0 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1350.5 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=explainer_16x9 run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1465 late=5 early=0 dropped=330 present_p50_ms=42.3 present_p95_ms=106.3 present_max_ms=234.0 held_max_ms=229.8 receipt_offset_max_ms=1966.7 clock_stall_max_ms=46.0 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1857.8 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1464 engine_late=6 engine_dropped=330 engine_dropped_agent=0 engine_held_max_ms=234.0 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=23.1 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=4 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1440.4 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=explainer_16x9 run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1460 late=7 early=0 dropped=333 present_p50_ms=42.2 present_p95_ms=106.1 present_max_ms=234.4 held_max_ms=230.9 receipt_offset_max_ms=1966.7 clock_stall_max_ms=46.7 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1917.2 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1460 engine_late=7 engine_dropped=333 engine_dropped_agent=0 engine_held_max_ms=234.4 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=4 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1693.5 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=explainer_16x9 run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1441 late=1 early=0 dropped=358 present_p50_ms=42.2 present_p95_ms=106.3 present_max_ms=234.6 held_max_ms=235.1 receipt_offset_max_ms=1966.7 clock_stall_max_ms=45.9 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2013.5 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1442 engine_late=0 engine_dropped=358 engine_dropped_agent=0 engine_held_max_ms=234.6 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.8 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=4 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1898.7 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=reel_9x16 run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1340 late=3 early=0 dropped=457 present_p50_ms=42.3 present_p95_ms=107.1 present_max_ms=319.0 held_max_ms=317.3 receipt_offset_max_ms=1966.7 clock_stall_max_ms=47.7 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2291.0 ledger_peak_mib=68.4 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1339 engine_late=4 engine_dropped=457 engine_dropped_agent=0 engine_held_max_ms=319.0 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1830.5 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=reel_9x16 run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1313 late=7 early=0 dropped=480 present_p50_ms=42.3 present_p95_ms=107.4 present_max_ms=320.1 held_max_ms=318.0 receipt_offset_max_ms=1966.7 clock_stall_max_ms=48.2 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2316.0 ledger_peak_mib=68.4 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1312 engine_late=8 engine_dropped=480 engine_dropped_agent=0 engine_held_max_ms=320.1 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=23.3 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2082.3 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=reel_9x16 run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1336 late=8 early=0 dropped=456 present_p50_ms=42.3 present_p95_ms=107.0 present_max_ms=341.3 held_max_ms=337.7 receipt_offset_max_ms=1966.7 clock_stall_max_ms=47.6 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2592.1 ledger_peak_mib=68.4 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1336 engine_late=8 engine_dropped=456 engine_dropped_agent=0 engine_held_max_ms=341.3 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.8 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2215.6 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=feed_4x5 run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=971 late=5 early=0 dropped=824 present_p50_ms=42.8 present_p95_ms=171.1 present_max_ms=729.5 held_max_ms=726.1 receipt_offset_max_ms=1966.7 clock_stall_max_ms=49.7 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2695.4 ledger_peak_mib=99.0 table_live_kib=128 passes=false engine_due=1800 engine_on_time=971 engine_late=5 engine_dropped=824 engine_dropped_agent=0 engine_held_max_ms=729.5 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=23.4 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2278.6 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=feed_4x5 run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=965 late=3 early=0 dropped=832 present_p50_ms=42.9 present_p95_ms=170.6 present_max_ms=1026.9 held_max_ms=1023.4 receipt_offset_max_ms=1966.7 clock_stall_max_ms=49.2 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2864.6 ledger_peak_mib=99.0 table_live_kib=128 passes=false engine_due=1800 engine_on_time=965 engine_late=3 engine_dropped=832 engine_dropped_agent=0 engine_held_max_ms=1026.9 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=23.1 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2569.0 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=feed_4x5 run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=987 late=6 early=0 dropped=807 present_p50_ms=42.8 present_p95_ms=170.2 present_max_ms=1367.1 held_max_ms=1363.9 receipt_offset_max_ms=1966.7 clock_stall_max_ms=49.2 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2588.7 ledger_peak_mib=99.0 table_live_kib=128 passes=false engine_due=1800 engine_on_time=987 engine_late=6 engine_dropped=807 engine_dropped_agent=0 engine_held_max_ms=1367.1 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2365.3 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=talk_recut run=0 valid=true elapsed_s=240.03 missed_callbacks=0 due=7200 on_time=6464 late=22 early=0 dropped=714 present_p50_ms=42.1 present_p95_ms=67.5 present_max_ms=255.8 held_max_ms=254.6 receipt_offset_max_ms=0.0 clock_stall_max_ms=60.0 underrun_frames=0 post_end_underrun_frames=0 peak_rss_mib=2526.7 ledger_peak_mib=42.2 table_live_kib=128 passes=false engine_due=7200 engine_on_time=6458 engine_late=28 engine_dropped=714 engine_dropped_agent=0 engine_held_max_ms=255.8 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=0 engine_sync_decoders=1 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2314.5 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=talk_recut run=1 valid=true elapsed_s=240.03 missed_callbacks=0 due=7200 on_time=6438 late=14 early=0 dropped=748 present_p50_ms=42.1 present_p95_ms=69.7 present_max_ms=256.0 held_max_ms=254.4 receipt_offset_max_ms=0.0 clock_stall_max_ms=60.0 underrun_frames=0 post_end_underrun_frames=0 peak_rss_mib=2638.3 ledger_peak_mib=42.2 table_live_kib=128 passes=false engine_due=7200 engine_on_time=6432 engine_late=20 engine_dropped=748 engine_dropped_agent=0 engine_held_max_ms=256.0 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=0 engine_sync_decoders=1 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2329.5 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=talk_recut run=2 valid=true elapsed_s=240.03 missed_callbacks=0 due=7200 on_time=6474 late=23 early=0 dropped=703 present_p50_ms=42.1 present_p95_ms=65.4 present_max_ms=235.1 held_max_ms=234.8 receipt_offset_max_ms=0.0 clock_stall_max_ms=59.5 underrun_frames=0 post_end_underrun_frames=0 peak_rss_mib=2378.2 ledger_peak_mib=42.2 table_live_kib=128 passes=false engine_due=7200 engine_on_time=6472 engine_late=25 engine_dropped=703 engine_dropped_agent=0 engine_held_max_ms=235.1 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=43.1 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=0 engine_sync_decoders=1 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2324.2 threads=44
PF1 control=Slowdown lane=LL workload=typical_1080p metric=G1 fails=true valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=121 late=27 early=0 dropped=1652 present_p50_ms=419.6 present_p95_ms=1017.9 present_max_ms=1519.6 held_max_ms=1514.7 receipt_offset_max_ms=33.3 clock_stall_max_ms=47.9 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2570.8 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=122 engine_late=25 engine_dropped=1653 engine_dropped_agent=0 engine_held_max_ms=1519.6 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2262.3 threads=44
PF1 control=Freeze lane=LL workload=typical_1080p metric=G14 fails=true valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=550 late=11 early=0 dropped=1239 present_p50_ms=64.3 present_p95_ms=275.5 present_max_ms=1094.1 held_max_ms=1091.2 receipt_offset_max_ms=33.3 clock_stall_max_ms=47.8 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2625.5 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=552 engine_late=8 engine_dropped=1240 engine_dropped_agent=0 engine_held_max_ms=1094.1 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2233.4 threads=44
PF1 control=ClockFreeze lane=LL workload=typical_1080p metric=G16 fails=true valid=true elapsed_s=61.02 missed_callbacks=0 due=1800 on_time=571 late=6 early=0 dropped=1223 present_p50_ms=64.2 present_p95_ms=269.2 present_max_ms=1091.7 held_max_ms=1091.4 receipt_offset_max_ms=0.0 clock_stall_max_ms=1050.0 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2505.8 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=570 engine_late=7 engine_dropped=1223 engine_dropped_agent=0 engine_held_max_ms=1091.7 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=1027.2 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2233.5 threads=44
PF1 control=Stall lane=LL workload=typical_1080p metric=underrun_frames fails=true valid=true elapsed_s=61.02 missed_callbacks=0 due=1800 on_time=574 late=9 early=0 dropped=1217 present_p50_ms=64.2 present_p95_ms=260.4 present_max_ms=1066.5 held_max_ms=1065.3 receipt_offset_max_ms=33.3 clock_stall_max_ms=1045.0 underrun_frames=48128 post_end_underrun_frames=512 peak_rss_mib=2448.6 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=574 engine_late=8 engine_dropped=1218 engine_dropped_agent=0 engine_held_max_ms=1066.5 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=1024.0 engine_underrun_events=47 engine_underrun_frames=48128 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2115.5 threads=44
ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 946 filtered out; finished in 2045.22s

# exit=0
# END P-play LL 2026-09-27 22:06:28 EDT load: 4.79 4.36 5.04
#      1423 24.1  2-04:24:06 Hyprland
#    180308  0.0  2-03:53:42 foot
#   3754322 91.0    09:16:47 foot
#   3754361 51.1    09:16:47 foot
# BEGIN P-play LH 2026-09-27 22:06:28 EDT load: 4.79 4.36 5.04
#      1423 24.1  2-04:24:06 Hyprland
#    180308  0.0  2-03:53:42 foot
#   3754322 91.0    09:16:47 foot
#   3754361 51.1    09:16:47 foot

running 1 test
test pf1_harness::pf1_play_baseline ... PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=typical_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=574 late=6 early=0 dropped=1220 present_p50_ms=64.2 present_p95_ms=271.6 present_max_ms=362.3 held_max_ms=362.2 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=914.4 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=575 engine_late=5 engine_dropped=1220 engine_dropped_agent=0 engine_held_max_ms=362.3 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.3 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=572.8 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=typical_1080p run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=574 late=8 early=0 dropped=1218 present_p50_ms=64.2 present_p95_ms=258.9 present_max_ms=384.6 held_max_ms=380.3 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1084.9 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=572 engine_late=10 engine_dropped=1218 engine_dropped_agent=0 engine_held_max_ms=384.6 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.2 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=725.5 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=typical_1080p run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=575 late=4 early=0 dropped=1221 present_p50_ms=64.2 present_p95_ms=266.3 present_max_ms=320.8 held_max_ms=340.3 receipt_offset_max_ms=0.0 clock_stall_max_ms=46.3 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1103.8 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=574 engine_late=5 engine_dropped=1221 engine_dropped_agent=0 engine_held_max_ms=335.8 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.0 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=727.7 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=blend_heavy_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=749 late=5 early=0 dropped=1046 present_p50_ms=64.0 present_p95_ms=171.7 present_max_ms=255.9 held_max_ms=252.0 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1202.6 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=749 engine_late=5 engine_dropped=1046 engine_dropped_agent=0 engine_held_max_ms=255.9 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.4 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1054.8 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=blend_heavy_1080p run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=690 late=7 early=0 dropped=1103 present_p50_ms=64.3 present_p95_ms=190.9 present_max_ms=277.4 held_max_ms=274.7 receipt_offset_max_ms=1966.7 clock_stall_max_ms=46.3 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1416.5 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=689 engine_late=8 engine_dropped=1103 engine_dropped_agent=0 engine_held_max_ms=277.4 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.1 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=878.8 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=blend_heavy_1080p run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=821 late=11 early=0 dropped=968 present_p50_ms=63.8 present_p95_ms=171.0 present_max_ms=276.8 held_max_ms=274.9 receipt_offset_max_ms=1966.7 clock_stall_max_ms=45.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1312.1 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=824 engine_late=8 engine_dropped=968 engine_dropped_agent=0 engine_held_max_ms=276.8 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.0 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1058.3 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=explainer_16x9 run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1499 late=16 early=0 dropped=285 present_p50_ms=42.1 present_p95_ms=98.2 present_max_ms=235.3 held_max_ms=232.9 receipt_offset_max_ms=1966.7 clock_stall_max_ms=46.2 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1532.4 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1497 engine_late=18 engine_dropped=285 engine_dropped_agent=0 engine_held_max_ms=235.3 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=21.8 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=4 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1101.3 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=explainer_16x9 run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1486 late=7 early=0 dropped=307 present_p50_ms=42.2 present_p95_ms=106.1 present_max_ms=234.5 held_max_ms=230.4 receipt_offset_max_ms=1966.7 clock_stall_max_ms=45.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1478.0 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1486 engine_late=7 engine_dropped=307 engine_dropped_agent=0 engine_held_max_ms=234.5 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=21.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=4 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1163.5 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=explainer_16x9 run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1488 late=9 early=0 dropped=303 present_p50_ms=42.2 present_p95_ms=106.0 present_max_ms=234.9 held_max_ms=231.1 receipt_offset_max_ms=1966.7 clock_stall_max_ms=45.9 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1550.2 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1487 engine_late=10 engine_dropped=303 engine_dropped_agent=0 engine_held_max_ms=234.9 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=21.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=4 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1192.5 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=reel_9x16 run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1416 late=8 early=0 dropped=376 present_p50_ms=42.1 present_p95_ms=106.6 present_max_ms=320.3 held_max_ms=318.5 receipt_offset_max_ms=1966.7 clock_stall_max_ms=45.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1552.2 ledger_peak_mib=68.4 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1416 engine_late=8 engine_dropped=376 engine_dropped_agent=0 engine_held_max_ms=320.3 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.4 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1346.5 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=reel_9x16 run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1403 late=10 early=0 dropped=387 present_p50_ms=42.1 present_p95_ms=106.8 present_max_ms=298.7 held_max_ms=296.4 receipt_offset_max_ms=1966.7 clock_stall_max_ms=45.6 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1582.2 ledger_peak_mib=68.4 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1404 engine_late=9 engine_dropped=387 engine_dropped_agent=0 engine_held_max_ms=298.7 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=21.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1328.3 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=reel_9x16 run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1410 late=9 early=0 dropped=381 present_p50_ms=42.1 present_p95_ms=106.6 present_max_ms=298.7 held_max_ms=297.1 receipt_offset_max_ms=1966.7 clock_stall_max_ms=45.6 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1615.8 ledger_peak_mib=68.4 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1410 engine_late=9 engine_dropped=381 engine_dropped_agent=0 engine_held_max_ms=298.7 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=21.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1311.7 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=feed_4x5 run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1188 late=6 early=0 dropped=606 present_p50_ms=42.3 present_p95_ms=150.0 present_max_ms=1194.6 held_max_ms=1192.2 receipt_offset_max_ms=1966.7 clock_stall_max_ms=45.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1564.9 ledger_peak_mib=99.0 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1189 engine_late=5 engine_dropped=606 engine_dropped_agent=0 engine_held_max_ms=1194.6 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.1 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1311.7 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=feed_4x5 run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1208 late=4 early=0 dropped=588 present_p50_ms=42.4 present_p95_ms=149.3 present_max_ms=426.5 held_max_ms=424.6 receipt_offset_max_ms=1966.7 clock_stall_max_ms=45.6 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1552.3 ledger_peak_mib=99.0 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1206 engine_late=6 engine_dropped=588 engine_dropped_agent=0 engine_held_max_ms=426.5 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.0 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1338.8 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=feed_4x5 run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1180 late=6 early=0 dropped=614 present_p50_ms=42.4 present_p95_ms=169.9 present_max_ms=426.9 held_max_ms=425.4 receipt_offset_max_ms=1966.7 clock_stall_max_ms=45.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1567.5 ledger_peak_mib=99.0 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1180 engine_late=6 engine_dropped=614 engine_dropped_agent=0 engine_held_max_ms=426.9 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.4 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1329.8 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=talk_recut run=0 valid=true elapsed_s=240.03 missed_callbacks=0 due=7200 on_time=6514 late=28 early=0 dropped=658 present_p50_ms=42.0 present_p95_ms=63.1 present_max_ms=256.3 held_max_ms=254.5 receipt_offset_max_ms=33.3 clock_stall_max_ms=60.1 underrun_frames=0 post_end_underrun_frames=0 peak_rss_mib=1478.7 ledger_peak_mib=42.2 table_live_kib=128 passes=false engine_due=7200 engine_on_time=6516 engine_late=25 engine_dropped=659 engine_dropped_agent=0 engine_held_max_ms=256.3 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=0 engine_sync_decoders=1 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1337.4 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=talk_recut run=1 valid=true elapsed_s=240.01 missed_callbacks=0 due=7200 on_time=6519 late=43 early=0 dropped=638 present_p50_ms=42.0 present_p95_ms=62.8 present_max_ms=256.0 held_max_ms=251.0 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.7 underrun_frames=0 post_end_underrun_frames=0 peak_rss_mib=1478.6 ledger_peak_mib=42.2 table_live_kib=128 passes=false engine_due=7200 engine_on_time=6517 engine_late=45 engine_dropped=638 engine_dropped_agent=0 engine_held_max_ms=256.0 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.0 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=0 engine_sync_decoders=1 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1337.4 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=talk_recut run=2 valid=true elapsed_s=240.03 missed_callbacks=0 due=7200 on_time=6522 late=36 early=0 dropped=642 present_p50_ms=42.0 present_p95_ms=63.0 present_max_ms=235.2 held_max_ms=234.7 receipt_offset_max_ms=0.0 clock_stall_max_ms=60.0 underrun_frames=0 post_end_underrun_frames=0 peak_rss_mib=1477.9 ledger_peak_mib=42.2 table_live_kib=128 passes=false engine_due=7200 engine_on_time=6520 engine_late=38 engine_dropped=642 engine_dropped_agent=0 engine_held_max_ms=235.2 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=0 engine_sync_decoders=1 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1337.4 threads=7
PF1 control=Slowdown lane=LH workload=typical_1080p metric=G1 fails=true valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=178 late=31 early=0 dropped=1591 present_p50_ms=378.0 present_p95_ms=585.2 present_max_ms=1058.2 held_max_ms=1055.8 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.4 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1531.9 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=183 engine_late=26 engine_dropped=1591 engine_dropped_agent=0 engine_held_max_ms=1058.2 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.8 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1388.9 threads=7
PF1 control=Freeze lane=LH workload=typical_1080p metric=G14 fails=true valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=571 late=8 early=0 dropped=1221 present_p50_ms=64.2 present_p95_ms=254.9 present_max_ms=1125.3 held_max_ms=1122.7 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.7 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1896.3 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=569 engine_late=10 engine_dropped=1221 engine_dropped_agent=0 engine_held_max_ms=1125.3 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=23.1 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1492.8 threads=7
PF1 control=ClockFreeze lane=LH workload=typical_1080p metric=G16 fails=true valid=true elapsed_s=61.02 missed_callbacks=0 due=1800 on_time=593 late=4 early=0 dropped=1203 present_p50_ms=64.2 present_p95_ms=256.2 present_max_ms=1090.5 held_max_ms=1086.5 receipt_offset_max_ms=0.0 clock_stall_max_ms=1050.0 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1636.6 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=592 engine_late=5 engine_dropped=1203 engine_dropped_agent=0 engine_held_max_ms=1090.5 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=1026.8 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1492.8 threads=7
PF1 control=Stall lane=LH workload=typical_1080p metric=underrun_frames fails=true valid=true elapsed_s=61.02 missed_callbacks=0 due=1800 on_time=586 late=11 early=0 dropped=1203 present_p50_ms=64.2 present_p95_ms=259.8 present_max_ms=1067.1 held_max_ms=1063.8 receipt_offset_max_ms=0.0 clock_stall_max_ms=1045.0 underrun_frames=48128 post_end_underrun_frames=512 peak_rss_mib=1911.6 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=587 engine_late=10 engine_dropped=1203 engine_dropped_agent=0 engine_held_max_ms=1067.1 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=1024.0 engine_underrun_events=47 engine_underrun_frames=48128 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1492.7 threads=7
ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 946 filtered out; finished in 2046.57s

# exit=0
# END P-play LH 2026-09-27 22:40:34 EDT load: 9.29 5.63 4.84
#      1423 24.2  2-04:58:12 Hyprland
#    180308  0.0  2-04:27:49 foot
#   3754322 91.0    09:50:54 foot
#   3754361 51.2    09:50:54 foot
# ALL DONE 2026-09-27 22:40:34 EDT
```

The superseded first rerun (`d3a4454`, the settle-only ack). These are
its P-play LL lines and its stopped LH lane's only line:

```
# BEGIN P-play LL 2026-09-27 20:45:38 EDT load: 8.02 7.11 5.69
test pf1_harness::pf1_play_baseline ... PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=typical_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=574 late=5 early=0 dropped=1221 present_p50_ms=64.2 present_p95_ms=259.5 present_max_ms=362.6 held_max_ms=360.4 receipt_offset_max_ms=0.0 clock_stall_max_ms=47.3 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=870.2 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=124 engine_late=6 engine_dropped=1670 engine_dropped_agent=0 engine_held_max_ms=362.6 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=22.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=588.9 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=typical_1080p run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=568 late=8 early=0 dropped=1224 present_p50_ms=64.2 present_p95_ms=275.2 present_max_ms=362.5 held_max_ms=358.9 receipt_offset_max_ms=0.0 clock_stall_max_ms=47.3 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1005.9 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=119 engine_late=7 engine_dropped=1674 engine_dropped_agent=0 engine_held_max_ms=362.5 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=23.1 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=726.4 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=typical_1080p run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=571 late=10 early=0 dropped=1219 present_p50_ms=64.3 present_p95_ms=256.7 present_max_ms=361.9 held_max_ms=380.0 receipt_offset_max_ms=0.0 clock_stall_max_ms=47.4 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1138.8 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=129 engine_late=7 engine_dropped=1664 engine_dropped_agent=0 engine_held_max_ms=379.2 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=22.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=856.4 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=blend_heavy_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=653 late=7 early=0 dropped=1140 present_p50_ms=64.5 present_p95_ms=192.5 present_max_ms=278.1 held_max_ms=275.0 receipt_offset_max_ms=1966.7 clock_stall_max_ms=47.1 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1395.6 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=130 engine_late=10 engine_dropped=1660 engine_dropped_agent=0 engine_held_max_ms=278.1 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=23.3 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1104.8 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=blend_heavy_1080p run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=652 late=8 early=0 dropped=1140 present_p50_ms=64.4 present_p95_ms=192.6 present_max_ms=276.8 held_max_ms=274.2 receipt_offset_max_ms=1966.7 clock_stall_max_ms=46.9 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1468.3 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=135 engine_late=7 engine_dropped=1658 engine_dropped_agent=0 engine_held_max_ms=276.8 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=23.3 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1189.0 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=blend_heavy_1080p run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=651 late=9 early=0 dropped=1140 present_p50_ms=64.4 present_p95_ms=193.5 present_max_ms=283.9 held_max_ms=282.9 receipt_offset_max_ms=0.0 clock_stall_max_ms=46.3 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1642.2 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=138 engine_late=8 engine_dropped=1654 engine_dropped_agent=0 engine_held_max_ms=283.9 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=22.8 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1559.8 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=explainer_16x9 run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1455 late=7 early=0 dropped=338 present_p50_ms=42.2 present_p95_ms=106.1 present_max_ms=235.4 held_max_ms=235.1 receipt_offset_max_ms=1966.7 clock_stall_max_ms=46.3 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2064.2 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=165 engine_late=6 engine_dropped=1629 engine_dropped_agent=0 engine_held_max_ms=235.4 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=22.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=4 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1757.6 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=explainer_16x9 run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1440 late=3 early=0 dropped=357 present_p50_ms=42.3 present_p95_ms=106.5 present_max_ms=234.6 held_max_ms=233.2 receipt_offset_max_ms=1966.7 clock_stall_max_ms=46.7 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2257.5 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=146 engine_late=3 engine_dropped=1651 engine_dropped_agent=0 engine_held_max_ms=234.6 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=22.6 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=4 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1861.6 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=explainer_16x9 run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1463 late=4 early=0 dropped=333 present_p50_ms=42.2 present_p95_ms=106.3 present_max_ms=213.3 held_max_ms=211.3 receipt_offset_max_ms=1966.7 clock_stall_max_ms=46.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2400.3 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=143 engine_late=4 engine_dropped=1653 engine_dropped_agent=0 engine_held_max_ms=213.3 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=22.8 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=4 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1786.6 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=reel_9x16 run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1311 late=7 early=0 dropped=482 present_p50_ms=42.3 present_p95_ms=107.0 present_max_ms=341.8 held_max_ms=340.0 receipt_offset_max_ms=1966.7 clock_stall_max_ms=47.3 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2118.9 ledger_peak_mib=68.4 table_live_kib=128 passes=false engine_due=1800 engine_on_time=137 engine_late=9 engine_dropped=1654 engine_dropped_agent=0 engine_held_max_ms=341.8 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=23.3 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1909.0 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=reel_9x16 run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1325 late=5 early=0 dropped=470 present_p50_ms=42.3 present_p95_ms=106.9 present_max_ms=341.6 held_max_ms=337.2 receipt_offset_max_ms=1966.7 clock_stall_max_ms=46.9 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2296.2 ledger_peak_mib=68.4 table_live_kib=128 passes=false engine_due=1800 engine_on_time=150 engine_late=6 engine_dropped=1644 engine_dropped_agent=0 engine_held_max_ms=341.6 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=22.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=1950.5 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=reel_9x16 run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1333 late=7 early=0 dropped=460 present_p50_ms=42.4 present_p95_ms=107.0 present_max_ms=321.0 held_max_ms=318.2 receipt_offset_max_ms=1966.7 clock_stall_max_ms=47.7 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2365.8 ledger_peak_mib=68.4 table_live_kib=128 passes=false engine_due=1800 engine_on_time=163 engine_late=7 engine_dropped=1630 engine_dropped_agent=0 engine_held_max_ms=321.0 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=24.6 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2144.5 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=feed_4x5 run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=972 late=3 early=0 dropped=825 present_p50_ms=42.9 present_p95_ms=170.9 present_max_ms=1088.0 held_max_ms=1085.8 receipt_offset_max_ms=1966.7 clock_stall_max_ms=48.7 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2697.1 ledger_peak_mib=99.0 table_live_kib=128 passes=false engine_due=1800 engine_on_time=130 engine_late=3 engine_dropped=1667 engine_dropped_agent=0 engine_held_max_ms=1088.0 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=23.2 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2156.9 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=feed_4x5 run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=961 late=8 early=0 dropped=831 present_p50_ms=42.8 present_p95_ms=170.8 present_max_ms=1280.1 held_max_ms=1276.4 receipt_offset_max_ms=1966.7 clock_stall_max_ms=48.3 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2599.2 ledger_peak_mib=99.0 table_live_kib=128 passes=false engine_due=1800 engine_on_time=142 engine_late=10 engine_dropped=1648 engine_dropped_agent=0 engine_held_max_ms=1280.1 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=23.0 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2352.1 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=feed_4x5 run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=995 late=7 early=0 dropped=798 present_p50_ms=42.8 present_p95_ms=170.3 present_max_ms=1494.0 held_max_ms=1493.8 receipt_offset_max_ms=1966.7 clock_stall_max_ms=47.2 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2877.9 ledger_peak_mib=99.0 table_live_kib=128 passes=false engine_due=1800 engine_on_time=124 engine_late=5 engine_dropped=1671 engine_dropped_agent=0 engine_held_max_ms=1494.0 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=23.3 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2676.2 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=talk_recut run=0 valid=true elapsed_s=240.01 missed_callbacks=0 due=7200 on_time=6454 late=18 early=0 dropped=728 present_p50_ms=42.1 present_p95_ms=71.0 present_max_ms=255.9 held_max_ms=255.4 receipt_offset_max_ms=1933.3 clock_stall_max_ms=46.4 underrun_frames=0 post_end_underrun_frames=0 peak_rss_mib=2832.0 ledger_peak_mib=42.2 table_live_kib=128 passes=false engine_due=7200 engine_on_time=616 engine_late=19 engine_dropped=6565 engine_dropped_agent=0 engine_held_max_ms=255.9 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=22.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=0 engine_sync_decoders=1 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2378.3 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=talk_recut run=1 valid=true elapsed_s=240.01 missed_callbacks=0 due=7200 on_time=6469 late=19 early=0 dropped=712 present_p50_ms=42.1 present_p95_ms=66.2 present_max_ms=256.2 held_max_ms=255.5 receipt_offset_max_ms=33.3 clock_stall_max_ms=46.4 underrun_frames=0 post_end_underrun_frames=0 peak_rss_mib=2424.2 ledger_peak_mib=42.2 table_live_kib=128 passes=false engine_due=7200 engine_on_time=639 engine_late=21 engine_dropped=6540 engine_dropped_agent=0 engine_held_max_ms=256.2 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=22.4 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=0 engine_sync_decoders=1 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2378.3 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=talk_recut run=2 valid=true elapsed_s=240.03 missed_callbacks=0 due=7200 on_time=6454 late=12 early=0 dropped=734 present_p50_ms=42.1 present_p95_ms=67.3 present_max_ms=235.5 held_max_ms=235.4 receipt_offset_max_ms=0.0 clock_stall_max_ms=60.0 underrun_frames=0 post_end_underrun_frames=0 peak_rss_mib=2692.4 ledger_peak_mib=42.2 table_live_kib=128 passes=false engine_due=7200 engine_on_time=614 engine_late=15 engine_dropped=6571 engine_dropped_agent=0 engine_held_max_ms=235.5 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=42.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=0 engine_sync_decoders=1 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2410.1 threads=44
PF1 control=Slowdown lane=LL workload=typical_1080p metric=G1 fails=true valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=138 late=30 early=0 dropped=1632 present_p50_ms=393.4 present_p95_ms=983.1 present_max_ms=1534.3 held_max_ms=1530.0 receipt_offset_max_ms=0.0 clock_stall_max_ms=47.6 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2779.7 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=130 engine_late=26 engine_dropped=1644 engine_dropped_agent=0 engine_held_max_ms=1534.3 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=23.0 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2501.5 threads=44
PF1 control=Freeze lane=LL workload=typical_1080p metric=G14 fails=true valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=569 late=11 early=0 dropped=1220 present_p50_ms=64.2 present_p95_ms=253.6 present_max_ms=1202.3 held_max_ms=1201.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=47.7 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2699.3 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=138 engine_late=10 engine_dropped=1652 engine_dropped_agent=0 engine_held_max_ms=1202.3 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=23.2 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2398.8 threads=44
PF1 control=ClockFreeze lane=LL workload=typical_1080p metric=G16 fails=true valid=true elapsed_s=61.02 missed_callbacks=0 due=1800 on_time=575 late=7 early=0 dropped=1218 present_p50_ms=64.3 present_p95_ms=258.3 present_max_ms=1092.2 held_max_ms=1090.9 receipt_offset_max_ms=0.0 clock_stall_max_ms=1050.0 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=2691.9 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=124 engine_late=8 engine_dropped=1668 engine_dropped_agent=0 engine_held_max_ms=1092.1 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=1027.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2354.9 threads=44
PF1 control=Stall lane=LL workload=typical_1080p metric=underrun_frames fails=true valid=true elapsed_s=61.02 missed_callbacks=0 due=1800 on_time=571 late=14 early=0 dropped=1215 present_p50_ms=64.2 present_p95_ms=258.6 present_max_ms=1067.4 held_max_ms=1065.4 receipt_offset_max_ms=0.0 clock_stall_max_ms=1045.0 underrun_frames=48128 post_end_underrun_frames=512 peak_rss_mib=2571.6 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=142 engine_late=13 engine_dropped=1645 engine_dropped_agent=0 engine_held_max_ms=1067.4 engine_av_offset_max_ms=966.7 engine_clock_stall_max_ms=1024.0 engine_underrun_events=47 engine_underrun_frames=48128 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=2358.9 threads=44
# BEGIN P-play LH 2026-09-27 21:19:37 EDT load: 6.26 5.10 5.33
test pf1_harness::pf1_play_baseline ... PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=typical_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=589 late=8 early=0 dropped=1203 present_p50_ms=64.2 present_p95_ms=251.9 present_max_ms=341.8 held_max_ms=341.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=912.7 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=136 engine_late=10 engine_dropped=1654 engine_dropped_agent=0 engine_held_max_ms=341.8 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=23.0 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=582.8 threads=7
```

### E11.12 R34 single-writer accounting (re-review 2)

Re-review 2 of `b5c2916` rejected on four R-5 defects (D1–D4); every
other finding is closed. Ruling R34 amended the design (§7 R-5,
"Amendment R34") instead of patching the ack-time snapshot again, and
E11.8 is updated to match. One code commit on `pf1/impl`:

| Commit | What it does |
|---|---|
| `9a3c583` | The worker is the only writer of due-frame outcomes; `ack_presented` only queues; bounded settlement, caps and per-frame agent attribution. Deletes `cb36a53`'s `Transport` snapshot and `Worker`-side `transport()`. |

It passed its gates:
- build, `clippy --workspace --all-targets -D warnings`, and rustfmt on the
  touched files (`skip_children`);
- media (933 lib tests pass, 20 ignored, at the default thread count, plus its integration tests) and core (782) tests;
- `cargo build -p kinewright-app`.

C-5 and C-3 still hold: no render, conversion or compositor path changed,
and no tolerance was added. Agent-protocol replies and MCP shapes are
unchanged: `PlaybackStats` (in-process only, never serialized to an agent)
gains `acks_overflowed` and `acks_unmatched`, and the harness prints them.
No error was added.

#### E11.12.1 Findings

| Finding | Fix | Witness | Fails on |
|---|---|---|---|
| D1: an issued epoch labels an older worker clock (the R29 terminal race) | The worker registers from its own runtime's consumed samples (`Worker::runtime_position`), which no caller call or worker transition writes. The ack reads no clock and registers nothing. | `engine::tests::an_old_paints_ack_delayed_across_a_raced_stop_registers_nothing`: `seek(10)` is issued before a 50-frame programme's terminal stop (clock 50); an old paint of frame 48 is acked after the stop and the resumed seek. Due frames stay 51 (0..=49, then 10). | The R33 code (`b5c2916`, the same ordering through its `transport` + ack): 90 due, 11..=49 of the seek's epoch manufactured. M35 (applied position from the shared clock). |
| D2: a pre-tick paint is lost when a pause intervenes | Acks wait in the queue. Every transition first ends the outgoing epoch through its runtime position (`start_playback`, `pause_through`, `set_document` via `pause`, `stop_at_end`), which registers the frame; the held ack then settles it. | `engine::tests::an_ack_ahead_of_the_tick_counts_across_a_pause_seek_or_document`: the frame is painted ahead of the tick, a pause, seek or new document is issued, the ack arrives, then the worker applies the call: on time 1, unmatched 0, in each case. `stats::tests::an_ack_ahead_of_the_tick_waits_for_its_registration` (the same at the counters, plus an epoch that closes short of the frame: unmatched). | The R33 code: on time 0 in all three cases (12 due, 12 dropped). M27, M28, M35, M36. |
| D3: eviction metadata is unbounded | Hard caps (below); the oldest evicted epoch is forgotten, the two oldest settled ranges merge, a full ack queue drops its oldest. | `stats::tests::eviction_keeps_a_bounded_number_of_epochs` (40 epochs of 4,000 frames: at most 8 epochs at every step, 8 at the end; a forgotten epoch's ack is unmatched, a remembered one's late). `stats::tests::sparse_acks_keep_a_bounded_number_of_settled_ranges` (151 alternating acks of evicted frames: at most 64 ranges; a frame inside the merged range is unmatched, one past it late). `stats::tests::a_full_ack_queue_drops_its_oldest` (300 acks: 44 overflowed, 256 held and then settled). | M29, M30, M31 respectively. |
| D4: an agent job charges a frame that was painted | Each due record carries its own charge: set when the worker registers it inside the job's window (its epoch, past the newest frame the preview published before the job), removed when a paint settles it. The preview's `Dropped { agent: true }` increment and `agent_dropped_through` are gone. | `stats::tests::an_agent_job_is_charged_only_its_epochs_registered_frames` (new cases: frame 11 published and acked before the worker's tick during the job: 12 and 13 charged, not 11; a frame registered in the window and painted after it is uncharged). `preview::tests::an_agent_job_is_not_charged_a_frame_published_before_it` (the preview passes its published frame to the window). | M32, M33, M34. |
| Nit: the tail-push witness's concurrency claim | E11.11.1 qualified: the push lands between two callbacks, and why a preempted-callback barrier would need the callback restructured. | — | — |
| Nit: the scrub witness's EOS | E11.11.1 qualified: the terminal events are injected through `RecordingPlayback`; no engine-driven EOS test is added. | — | — |

The D1 and D2 witnesses were also run against the R33 tree: a scratch
worktree at `b5c2916` with the same orderings written against its API
(`transport` captured after the issuing call, then its six-argument
`acknowledge`). Both failed as shown; the worktree was then removed.

**Caps.** Due records `DUE_RECORDS` = 65,536 (unchanged); evicted epochs
`EVICTED_EPOCHS` = 8; settled ranges per evicted epoch `SETTLED_RANGES` =
64; the ack queue `ACK_QUEUE` = 256 (about four seconds of 60 fps paints
if the worker stopped draining). The approximation is in E11.8 and the
design amendment.

#### E11.12.2 Updated witnesses

Every R-5 witness stays green. These were updated, only where the
single-writer model moves when an outcome happens, or for the new API:

| Witness | Update | Why |
|---|---|---|
| `stats::due_frames_are_counted_once_by_their_ack` | Acks go through the queue and a drain. The never-due ack and the duplicate now also count `acks_unmatched` (2). Frame 1 is registered at 33 ms (its frame start) instead of 5 ms. | The A/V offset is now measured at paint from the frame's due instant, so the due instant has to be the frame's start for the expected one-frame offset (33 ms, unchanged). |
| `stats::a_late_acknowledgment_…`, `stats::a_frame_expired_at_its_paint_is_late`, `stats::an_ack_after_the_stop_counts` | API only (queued ack + drain; `sample`/`end` take the worker's `Option` position) | — |
| `stats::an_evicted_record_acknowledged_later_is_late` | API; each no-count case now also asserts `acks_unmatched` | Unmatched acks are counted. |
| `stats::an_agent_job_is_charged_only_its_epochs_registered_frames` | `agent_started(published)` / `agent_finished()` no longer return the window; two D4 cases added | The charge is per record, set at registration and removed at settlement. |
| `stats::an_ack_ahead_of_the_tick_samples_its_applied_snapshot` | Replaced by `an_ack_ahead_of_the_tick_waits_for_its_registration` | The snapshot is deleted; the ack is held instead of sampling. |
| `engine::an_ack_racing_an_unapplied_seek_manufactures_no_due_frames`, `engine::an_ack_ahead_of_the_workers_tick_counts_on_time`, `engine::an_ack_after_the_terminal_stop_counts` | The ack is followed by `worker.tick()` before the assertions | Outcomes settle at the worker's next drain, not at the ack. |
| `preview::an_agent_job_counts_the_due_frames_it_displaces` | API only (`Some(position)`); the expected charges (6 / 2 / 2 / 0, "counted once" 6) are unchanged | — |

#### E11.12.3 Broken variants (R34)

Each mutation was applied to `9a3c583`, the R-5 witnesses were run
(`stats::`, the engine ack and transport tests, `preview::an_agent…`), and
the mutation was reverted. Every mutation fails at least one witness.

| # | Mutation | Fails |
|---|---|---|
| M27 | No held-ack requeue: an ack ahead of the registrations is unmatched at once | `an_ack_ahead_of_the_tick_waits_for_its_registration`, `a_full_ack_queue_drops_its_oldest`, `an_agent_job_is_charged_only_…` |
| M28 | `start_playback` ends the outgoing epoch through nothing | `an_ack_ahead_of_the_tick_counts_across_a_pause_seek_or_document` ("seek") |
| M29 | No evicted-epoch cap | `eviction_keeps_a_bounded_number_of_epochs` |
| M30 | No settled-range cap | `sparse_acks_keep_a_bounded_number_of_settled_ranges` |
| M31 | No ack-queue cap | `a_full_ack_queue_drops_its_oldest` |
| M32 | The agent charge ignores the published frame | `an_agent_job_is_charged_only_…`, `an_agent_job_is_not_charged_a_frame_published_before_it` |
| M33 | A settled paint keeps its agent charge | `an_agent_job_is_charged_only_…` |
| M34 | The preview opens the window with no published frame | `an_agent_job_is_not_charged_a_frame_published_before_it` |
| M35 | The worker's applied position is the shared clock (which callers and transitions write) | `an_ack_racing_an_unapplied_seek_…`, `a_seek_racing_the_tick_manufactures_no_due_frames`, `an_ack_ahead_of_the_tick_counts_across_a_pause_seek_or_document` |
| M36 | `end` registers no outgoing interval | `an_ack_after_the_stop_counts`, `an_ack_ahead_of_the_tick_waits_…`, `an_agent_job_is_charged_only_…`, `an_ack_ahead_of_the_tick_counts_across_…`, `an_agent_job_counts_the_due_frames_it_displaces` |

#### E11.12.4 Timing reruns (R34)

**Provenance.**
- Binary: the release test binary of `9a3c583`
  (`kinewright_media-cf306d076b2fd4bf`, sha256
  `09e07f7c772cd682987d330aae6aa198795aac8cbcb9fdcd6cf053077823d314`),
  built once and copied aside, so every lane ran the same binary. rustc
  1.98.0; the machine, drivers and FFmpeg are as in E11.1.
- When: 2026-09-27, 23:25:18–23:32:40 EDT. One runner ran the three lanes
  in sequence, each alone, with no build, test or other lane alongside:

  | Lane | Window (EDT) |
  |---|---|
  | G3 LH | 23:25:18–23:27:58 |
  | P-play LL | 23:27:58–23:30:18 |
  | P-play LH | 23:30:18–23:32:40 |

- Per R34 the P-play lanes are `PF1_RUNS=1
  PF1_ONLY=typical_1080p,blend_heavy_1080p`: the two workloads once per
  lane, and no controls. Output was logged unfiltered.
- Every lane printed `test result: ok. 1 passed` and exited 0, with no
  exit-time panic or SIGSEGV.
- **Ambient load (R26).** The two `foot` screensaver processes (≈91% +
  51%) and Hyprland (≈24%) were present at every BEGIN/END (lifetime
  averages from `ps`). No `gh` process was present at any mark. 1-minute
  load averages were 4.8–7.9.

**G3** (60 fps floor, LH):

| Build | `blend_heavy_1080p` fps (3 runs) | mean ms | p95 ms | Result |
|---|---|---|---|---|
| R33 `cb36a53` (E11.11.4) | 71.6 / 72.2 / 71.2 | 13.85–14.05 | 15.18–15.63 | passes |
| R34 `9a3c583` | 71.0 / 72.5 / 70.9 | 13.79–14.10 | 15.23–15.90 | **passes** |

- `typical_1080p` reads 72.5–73.5 fps, and `heavy_4k` stays under the
  floor as before (58.8 fps on its first run; it is not gated).
- The slowdown control fails its verdict (17.7 fps), as it must.
- `blend_heavy_1080p` phases: `upload_ms` 10.73, `gpu_passes_readback_ms`
  1.61, `monitor_encode_ms` 1.64.

**P-play (simulated driver), one run each.** Both runs are valid
(60.02 s, 0 missed callbacks).

| Workload | Lane | Harness on time / late / dropped | Engine due: on time / late / dropped | Δ on time / late | Engine A/V offset max ms | Acks overflowed / unmatched | Held max ms, harness / engine | Clock stall max ms, harness / engine | Underrun frames, harness / engine; post-end |
|---|---|---|---|---|---|---|---|---|---|
| `typical_1080p` | LL | 563 / 7 / 1230 | 1800: 562 / 8 / 1230 | −1 / +1 | 33.3 | 0 / 0 | 424.6 / 426.4 | 47.7 / 23.0 | 0 / 0; 512 |
| `blend_heavy_1080p` | LL | 657 / 6 / 1137 | 1800: 658 / 5 / 1137 | +1 / −1 | 33.3 | 0 / 0 | 273.6 / 277.4 | 47.1 / 23.6 | 0 / 0; 512 |
| `typical_1080p` | LH | 577 / 8 / 1215 | 1800: 573 / 12 / 1215 | −4 / +4 | 33.3 | 0 / 0 | 340.1 / 341.1 | 45.5 / 22.6 | 0 / 0; 512 |
| `blend_heavy_1080p` | LH | 767 / 6 / 1027 | 1800: 767 / 5 / 1028 | 0 / −1 | 33.3 | 0 / 0 | 272.7 / 276.9 | 46.1 / 22.0 | 0 / 0; 512 |

- **Engine and harness agree within ±6** (at most 4 on time and 4 late,
  on `typical_1080p` LH). Engine due is exactly 1,800 in every run.
- The harness's on-time counts are near R33's (E11.11.4): LL 563 and
  657 against 577–581 and 645–655; LH 577 and 767 against 574–575 and
  690–821. The engine's counts move with the harness's, so the spread is
  playback run-to-run variation, not the accounting.
- `engine_dropped_agent` is 0, and `engine_stale_errors` is 0.
  `engine_acks_overflowed` and `engine_acks_unmatched` are 0: no ack fell
  past a cap, and none matched nothing.
- **The A/V offset changed meaning (E11.8).** `engine_av_offset_max_ms` is
  33.3 in every run, against 0.0 in R33, which measured it against the
  ack-time clock snapshot. R34 measures, at paint, the whole frames the paint
  trailed its frame's due instant, at least one if expired at paint.
  Every run has late frames (5–12), and a late frame is painted more than
  one frame after it was due or expired at paint, so it reads at least one
  frame. It is not a regression. *Qualified in R35 (re-review 3 D4):*
  33.3 ms does not show that no paint trailed its frame by two frames.
  The figure is a proxy from stored timestamps: a paint that precedes a
  delayed registration reads zero elapsed, and `expired` supplies only
  one frame (E11.8). The harness's `receipt_offset_max_ms` is a different
  measure and is unchanged in kind (0.0, and 1966.7 in one run, as in R33).
- G11 and G16 still pass: 0 underrun frames before the end in the harness
  and the engine, the post-end straddle 512, and the harness's clock stall
  45.5–47.7 ms against the 100 ms gate.
- Teardown: complete in every run, with 44 threads on LL and 7 on LH, as
  in R33. Teardown RSS (457.7–617.5 MiB on LL, 415.9–528.3 MiB on LH) is
  lower than R33's because each lane's process ran only two workloads.

#### E11.12.5 Line delta

Non-blank, non-comment `.rs` lines over `b5c2916..9a3c583`, split at each
file's `#[cfg(test)]` test module:

| File | Production | Tests |
|---|---|---|
| `media/src/stats.rs` | 219 → 247 (+28) | 165 → 271 (+106) |
| `media/src/engine.rs` | 2,229 → 2,233 (+4) | 2,374 → 2,441 (+67) |
| `media/src/preview.rs` | 591 → 587 (−4) | 340 → 372 (+32) |
| `media/src/pf1_harness.rs` | +2 | — |
| `core/src/media.rs` | +2 | — |
| **Total** | **+32** | **+205** |

- `stats.rs` production is +28: the queue, the drain, the range merge and
  the epoch cap (+ one test-only accessor), less the deleted
  `Transport` snapshot, `AgentWindow`'s registration counters and the
  ack-time sampling. The engine loses `transport()` and gains
  `runtime_position()`; the preview loses its agent-drop increment.
- `git diff --stat b5c2916..9a3c583 -- crates`: 5 files changed, 618
  insertions and 304 deletions. The docs are extra.

#### E11.12.6 Raw result lines (R34)

`#` lines are the runner's lane markers (with `ps` and load averages) and
exit statuses. Everything else is verbatim from `timing.log`, `9a3c583`:

```
# binary sha256 09e07f7c772cd682987d330aae6aa198795aac8cbcb9fdcd6cf053077823d314 commit 9a3c583
# BEGIN G3 LH 2026-09-27 23:25:18 EDT load: 7.05 6.87 5.45
#    1423 24.2  2-05:42:56 Hyprland
#  180308  0.0  2-05:12:33 foot
# 3754322 90.9    10:35:38 foot
# 3754361 51.2    10:35:38 foot

running 1 test
test mo2_perf_fixtures::blend_heavy_holds_floors_on_hardware ... R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=typical_1080p run=0 dims=(1280, 720) mean_ms=13.60 fps=73.5 p95_ms=15.10 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=typical_1080p run=1 dims=(1280, 720) mean_ms=13.71 fps=72.9 p95_ms=15.27 ledger_peak_mib=56.3 validate_us=1.5
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=typical_1080p run=2 dims=(1280, 720) mean_ms=13.80 fps=72.5 p95_ms=15.66 ledger_peak_mib=56.3 validate_us=1.3
R28 phases workload=typical_1080p upload_ms=10.02 gpu_passes_readback_ms=1.32 monitor_encode_ms=1.59
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=blend_heavy_1080p run=0 dims=(1280, 720) mean_ms=14.09 fps=71.0 p95_ms=15.74 ledger_peak_mib=63.3 validate_us=1.2
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=blend_heavy_1080p run=1 dims=(1280, 720) mean_ms=13.79 fps=72.5 p95_ms=15.23 ledger_peak_mib=63.3 validate_us=1.2
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=blend_heavy_1080p run=2 dims=(1280, 720) mean_ms=14.10 fps=70.9 p95_ms=15.90 ledger_peak_mib=63.3 validate_us=1.2
R28 phases workload=blend_heavy_1080p upload_ms=10.73 gpu_passes_readback_ms=1.61 monitor_encode_ms=1.64
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=heavy_4k run=0 dims=(1280, 720) mean_ms=17.01 fps=58.8 p95_ms=18.59 ledger_peak_mib=70.3 validate_us=9.3
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=heavy_4k run=1 dims=(1280, 720) mean_ms=16.96 fps=59.0 p95_ms=18.22 ledger_peak_mib=70.3 validate_us=10.1
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=heavy_4k run=2 dims=(1280, 720) mean_ms=17.11 fps=58.5 p95_ms=19.03 ledger_peak_mib=70.3 validate_us=9.2
R28 phases workload=heavy_4k upload_ms=13.44 gpu_passes_readback_ms=1.62 monitor_encode_ms=1.71
R28 control=slowdown delay_ms=41.7 mean_ms=56.34 p95_ms=58.28 verdict=Err("17.7 fps (floor 60), p95 58.3 ms vs mean 56.3 ms")
ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 952 filtered out; finished in 159.18s

# exit=0
# END G3 LH 2026-09-27 23:27:58 EDT load: 4.80 6.19 5.42
#    1423 24.2  2-05:45:36 Hyprland
#  180308  0.0  2-05:15:12 foot
# 3754322 90.9    10:38:17 foot
# 3754361 51.2    10:38:17 foot
# BEGIN P-play LL 2026-09-27 23:27:58 EDT load: 4.80 6.19 5.42
#    1423 24.2  2-05:45:36 Hyprland
#  180308  0.0  2-05:15:12 foot
# 3754322 90.9    10:38:17 foot
# 3754361 51.2    10:38:17 foot

running 1 test
test pf1_harness::pf1_play_baseline ... PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=typical_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=563 late=7 early=0 dropped=1230 present_p50_ms=64.3 present_p95_ms=266.8 present_max_ms=426.4 held_max_ms=424.6 receipt_offset_max_ms=0.0 clock_stall_max_ms=47.7 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=858.9 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=562 engine_late=8 engine_dropped=1230 engine_dropped_agent=0 engine_held_max_ms=426.4 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=23.0 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=457.7 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=blend_heavy_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=657 late=6 early=0 dropped=1137 present_p50_ms=64.4 present_p95_ms=192.6 present_max_ms=277.4 held_max_ms=273.6 receipt_offset_max_ms=0.0 clock_stall_max_ms=47.1 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1121.3 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=658 engine_late=5 engine_dropped=1137 engine_dropped_agent=0 engine_held_max_ms=277.4 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=23.6 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=617.5 threads=44
ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 952 filtered out; finished in 140.67s

# exit=0
# END P-play LL 2026-09-27 23:30:18 EDT load: 7.94 6.83 5.76
#    1423 24.2  2-05:47:56 Hyprland
#  180308  0.0  2-05:17:33 foot
# 3754322 90.9    10:40:38 foot
# 3754361 51.2    10:40:38 foot
# BEGIN P-play LH 2026-09-27 23:30:18 EDT load: 7.94 6.83 5.76
#    1423 24.2  2-05:47:56 Hyprland
#  180308  0.0  2-05:17:33 foot
# 3754322 90.9    10:40:38 foot
# 3754361 51.2    10:40:38 foot

running 1 test
test pf1_harness::pf1_play_baseline ... PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=typical_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=577 late=8 early=0 dropped=1215 present_p50_ms=64.2 present_p95_ms=253.9 present_max_ms=341.1 held_max_ms=340.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=924.6 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=573 engine_late=12 engine_dropped=1215 engine_dropped_agent=0 engine_held_max_ms=341.1 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=22.6 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=415.9 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=blend_heavy_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=767 late=6 early=0 dropped=1027 present_p50_ms=64.0 present_p95_ms=171.0 present_max_ms=276.9 held_max_ms=272.7 receipt_offset_max_ms=1966.7 clock_stall_max_ms=46.1 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1129.9 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=767 engine_late=5 engine_dropped=1028 engine_dropped_agent=0 engine_held_max_ms=276.9 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=22.0 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=3 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 consumer_rejected=2 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=528.3 threads=7
ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 952 filtered out; finished in 141.35s

# exit=0
# END P-play LH 2026-09-27 23:32:40 EDT load: 7.43 7.46 6.19
#    1423 24.2  2-05:50:18 Hyprland
#  180308  0.0  2-05:19:54 foot
# 3754322 90.9    10:42:59 foot
# 3754361 51.2    10:42:59 foot
# ALL DONE
```

### E11.13 R35 runtime-only registration and a lock-free ack hand-off (re-review 3)

Re-review 3 of `b90128f` closed D3 (caps) and D4 (agent charges). It
rejected on four implementation gaps against R34's own rule ("registration
only from the worker's runtime position"). Each was verified against the
code before fixing; none is disputed:
- D1 holds: `tick` reads `seek_pending`, then `programme_ended()` reads
  `clock.position()`, so a `seek(1000)` between them made `stop_at_end`
  register through the duration.
- D2 holds: `start_playback` read `runtime_position()` before dropping the
  runtime, and `pause` read it before `audio.pause()`.
- D3 holds: `acknowledge` took the counters mutex, which the worker holds
  through registration and settlement.
- D4 holds: the 5 ms is `WORKER_TICK`, a `recv_timeout`, and
  `Counters::settle` saturates the elapsed time to zero for a paint that
  precedes its registration.

One code commit on `pf1/impl`:

| Commit | What it does |
|---|---|
| `8e1b1b1` | `Worker::quiesce` stops the outgoing stream, then reads its final consumed frame; every transition, the terminal stop included, closes R-5 there. `Lane::ack` hands acks over on a bounded crossbeam channel with `try_send` (an existing dependency). Both the channel and the held acks drop the newest when full. It also carries the witness nits. |

It passed its gates:
- build, `clippy --workspace --all-targets -D warnings`, and rustfmt on the
  touched files (`skip_children`);
- the media tests (936 lib tests pass, 20 ignored, at the default thread
  count, plus its integration tests);
- `cargo build -p kinewright-app`.

Core is untouched. C-5 and C-3 hold: no render path changed and no
tolerance was added. The user-visible terminal behaviour is S1's: the R29
and R30 witnesses (`a_playing_seek_that_races_the_terminal_stop_…`,
`a_seek_after_the_stop_or_before_a_pause_…`,
`a_pause_behind_a_seek_to_the_end_…`, `a_drained_one_frame_timeline_…`)
are unedited and green. A pause now reads its resting position after the
stream has stopped, not after `pause()` was requested; for cpal, the
stream stops for certain only when it is dropped. The events and their
order are unchanged. Agent-protocol replies and MCP shapes are unchanged.

#### E11.13.1 Findings

| Finding | Fix | Witness | Fails when reverted |
|---|---|---|---|
| D1: shared-clock terminal detection registers unplayed frames | `stop_at_end` ends R-5 at `quiesce`'s consumed frame, clamped to the duration. The I8 model has no runtime, so it uses its settled clock instead. The stop itself is unchanged. | `engine::tests::a_seek_racing_the_terminal_check_registers_no_unplayed_frames`. A test hook (`on_terminal_check`) issues `seek(1000)` between the tick's pending-seek check and its terminal check, at runtime frame 10 of 1,000. The tick stops at the end and nothing more becomes due; the raced seek then resumes and registers only its own first frame. | M37 (the stop ends at the duration: frames 11..=999 become due) |
| D2: outgoing position captured before the callbacks stop | `quiesce` takes the runtime, pauses it where the transition always did, and drops it. Dropping joins cpal's callback thread; for the simulated driver, pausing stops it under its stream lock. Only then does `quiesce` read `position_samples`, with the stream's own rate and the outgoing document's fps. `start_playback`, `pause_through` (and so `set_document`) and `stop_at_end` use it. | `engine::tests::a_callback_racing_a_transition_still_registers_its_frame`. A test hook (`on_quiesce`) runs stepped callbacks into the next frame after the transition began and before the stream stops, for a pause, a playing seek and a new document. That frame is due in each case. | M38 (position read before the hook, i.e. before the stream stops) |
| D3: acknowledgement can block the UI | `Lane::ack` calls `try_send` on a bounded crossbeam channel (256), with no lock. A full channel drops the newest ack and counts it (lane atomic, merged into `stats().acks_overflowed`, reset at `play`). `Lane::settle` moves the channel into the counters for the worker; `Lane::clear_counters` discards it at `play`. | `engine::tests::an_ack_never_waits_for_the_counters`. While the test thread holds the counters lock, another thread sends 257 acks and finishes. One overflows (the newest), and after a tick the worker holds 100..=355. | M39 (the ack takes the counters lock): the sender blocks, 10 s timeout |
| D4: temporal claims exceed the implementation | Docs only. The 5 ms is qualified as a receive timeout (design §7 R-5 amendment; E11.8). The A/V offset is qualified as a stored-timestamp proxy, and E11.12.4's "no paint trailed its frame by two frames" is withdrawn. | — | — |
| Nit: the overflow witness checks counts only | The held acks drop the newest (R35's rule). `stats::tests::a_full_ack_hold_drops_the_newest` (renamed from `a_full_ack_queue_drops_its_oldest`) asserts the kept endpoints 1 and 256, their order, and that 257 and 300 stay dropped. | the same | M40 (drop the oldest) |
| Nit: the agent witness never settles a paint after the job | `stats::tests::an_agent_job_is_charged_only_…` acks frame 22 after `agent_finished()`, and its charge is removed (3 → 2). | the same | (coverage) |
| Nit: duplicated `#[cfg(test)]` in `stats.rs` | One `#[cfg(test)] impl Counters` block | — | — |
| Nit: duplicated "Final check and binding" text in E11.8 | Removed | — | — |

Mutations M37–M40 were each applied to `8e1b1b1`, followed by a run of the
R-5 witnesses (`stats::`, the engine's `a_…` and `an_…` tests,
`preview::tests`), then reverted. Each fails only the witness named
above. M38 stops at the first transition (the pause).

**Updated witnesses.** `stats::a_full_ack_queue_drops_its_oldest` became
`a_full_ack_hold_drops_the_newest`. The model changed which ack a full
hold drops, so the test's `dropped` expectation is unchanged (45) but it
now names the newest. `stats::an_agent_job_is_charged_only_…` gained one
settlement after the job. No other witness changed.

#### E11.13.2 Timing reruns (R35)

- Binary: the release test binary of `8e1b1b1`
  (`kinewright_media-cf306d076b2fd4bf`, sha256
  `e138f70903d6247705943e49e6d62e93541e1adc34ac813f36c383aa03d57deb`),
  built once and copied aside. rustc 1.98.0.
- When: P-play LL 2026-09-27 23:57:51–23:58:56 EDT, then P-play LH
  23:58:56–2026-09-28 00:00:02 EDT. Each lane ran alone, with no build or
  test alongside. Both lanes are `PF1_RUNS=1 PF1_ONLY=typical_1080p`, and
  both exited 0.
- Ambient load (R26): the two `foot` screensaver processes (≈91% + 51%)
  and Hyprland (≈24%) were present at every mark. Two `gh` processes
  appeared at the final END mark only (elapsed 00:00, after the LH run
  had finished). 1-minute load averages were 6.0–7.5.
- No G3 rerun: R35 changes only the ack hand-off and the transitions. No
  hot-path file outside `stats.rs` and the transitions changed.

| Workload | Lane | Harness on time / late / dropped | Engine due: on time / late / dropped | Δ on time / late | Acks overflowed / unmatched | Held max ms, harness / engine | Clock stall max ms, harness / engine | Underrun frames, harness / engine; post-end |
|---|---|---|---|---|---|---|---|---|
| `typical_1080p` | LL | 575 / 4 / 1221 | 1800: 572 / 7 / 1221 | −3 / +3 | 0 / 0 | 360.3 / 363.2 | 46.5 / 23.9 | 0 / 0; 512 |
| `typical_1080p` | LH | 576 / 9 / 1215 | 1800: 577 / 8 / 1215 | +1 / −1 | 0 / 0 | 340.1 / 333.8 | 45.5 / 22.0 | 0 / 0; 512 |

- **The engine and harness agree within ±6**: 3 at most. Engine due is
  1,800.
- Both runs are valid: 60.02 s, 0 missed callbacks.
- Other fields:
  - `engine_dropped_agent` 0, `engine_stale_errors` 0;
  - `engine_av_offset_max_ms` 33.3 (a proxy, E11.8);
  - receipt offset 0.0, rejected at the consumer 0, sync decoders 2;
  - present p50 / p95 / max: 64.3 / 268.8 / 363.2 ms (LL), 64.2 / 256.6 /
    320.5 ms (LH);
  - teardown complete, threads 44 (LL) and 7 (LH);
  - teardown RSS 456.1 and 445.3 MiB;
  - peak RSS 854.1 and 891.6 MiB.
- These are within run-to-run noise of E11.12.4 and E11.11.4.

#### E11.13.3 Line delta

Non-blank, non-comment `.rs` lines over `b90128f..8e1b1b1`, split at each
file's test module:

| File | Production | Tests |
|---|---|---|
| `media/src/stats.rs` | 247 → 251 (+4) | 271 → 288 (+17) |
| `media/src/engine.rs` | 2,233 → 2,252 (+19) | 2,441 → 2,534 (+93) |
| `media/src/preview.rs` | 587 → 620 (+33) | unchanged |
| **Total** | **+56** | **+110** |

- The production figure includes the two test-only fault hooks and the
  test-only `held` accessor (about 12 lines).
- `git diff --stat b90128f..8e1b1b1 -- crates`: 3 files changed, 287
  insertions and 69 deletions.

#### E11.13.4 Raw result lines (R35)

`#` lines are the runner's lane markers and exit statuses; everything else
is verbatim from `timing.log`, `8e1b1b1`:

```
# binary sha256 e138f70903d6247705943e49e6d62e93541e1adc34ac813f36c383aa03d57deb commit 8e1b1b1
# BEGIN P-play LL 2026-09-27 23:57:51 EDT load: 6.82 6.11 5.27
#    1423 24.2  2-06:15:29 Hyprland
#  180308  0.0  2-05:45:05 foot
# 3754322 90.9    11:08:10 foot
# 3754361 51.2    11:08:10 foot

running 1 test
test pf1_harness::pf1_play_baseline ... PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=typical_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=575 late=4 early=0 dropped=1221 present_p50_ms=64.3 present_p95_ms=268.8 present_max_ms=363.2 held_max_ms=360.3 receipt_offset_max_ms=0.0 clock_stall_max_ms=46.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=854.1 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=572 engine_late=7 engine_dropped=1221 engine_dropped_agent=0 engine_held_max_ms=363.2 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=23.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=456.1 threads=44
ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 955 filtered out; finished in 65.46s

# exit=0
# END P-play LL 2026-09-27 23:58:56 EDT load: 5.95 6.04 5.31
#    1423 24.2  2-06:16:34 Hyprland
#  180308  0.0  2-05:46:11 foot
# 3754322 90.9    11:09:16 foot
# 3754361 51.2    11:09:16 foot
# BEGIN P-play LH 2026-09-27 23:58:56 EDT load: 5.95 6.04 5.31
#    1423 24.2  2-06:16:34 Hyprland
#  180308  0.0  2-05:46:11 foot
# 3754322 90.9    11:09:16 foot
# 3754361 51.2    11:09:16 foot

running 1 test
test pf1_harness::pf1_play_baseline ... PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=typical_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=576 late=9 early=0 dropped=1215 present_p50_ms=64.2 present_p95_ms=256.6 present_max_ms=320.5 held_max_ms=340.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=891.6 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=577 engine_late=8 engine_dropped=1215 engine_dropped_agent=0 engine_held_max_ms=333.8 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=22.0 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=2 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=445.3 threads=7
ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 955 filtered out; finished in 65.32s

# exit=0
# END P-play LH 2026-09-28 00:00:02 EDT load: 7.51 6.65 5.58
#    1423 24.2  2-06:17:39 Hyprland
#  180308  0.0  2-05:47:16 foot
# 2920961 87.5       00:00 gh
# 2920998  125       00:00 gh
# 3754322 90.9    11:10:21 foot
# 3754361 51.2    11:10:21 foot
# ALL DONE
```

## E12 S2b results

### E12.1 Provenance

- **Commits** on `pf1/impl`, after S2a `3b221bd`:
  - `72246d3` S2b-1;
  - `2a632fa` S2b-2;
  - `e0a15ac` S2b-3;
  - `cce86e1` S2b-4.
- **Binary.** The stage-end lanes all ran one release test binary, built once at `cce86e1` and copied aside
  (`kinewright_media-cf306d076b2fd4bf`, sha256 `4287890a9fb73714984af705e8fd7ad7e1c6bd69ed1b04a0e74b4df84c4ac640`).
  rustc 1.98.0.
- **Machine:** the same as E9.1, E10.1 and E11.1:
  - i5-13600K;
  - RTX 3090 on NVIDIA 615.71.09;
  - llvmpipe (LLVM 22.1.8);
  - Linux 7.2.5-4-omarchy;
  - pinned FFmpeg n8.0-23-gd1f31a829d.
- **G18's S0 side** is `d19bdf9` plus the self-contained, test-only `pf1_export_lane.rs` added unchanged. Its binary's
  sha256 is `210260a2a90360f21ce688276b70f3b4625dbcf23ecc3564670fe754ac398295`. Both sides run with
  `PF1_EXPORT_CLIPS=24`.
- **When.** 2026-09-28, 08:08:41–09:55:57 EDT, plus a P-rss LH rerun from 09:56:19 to 09:58:56. One runner ran the
  lanes in sequence. Each lane ran alone, with no build, test or other lane alongside.

  | Lane | Window (EDT) | Exit |
  |---|---|---|
  | I4 LL | 08:08:41–08:10:00 | 0 |
  | I4 LH | 08:10:00–08:11:15 | 0 |
  | G3 LH | 08:11:15–08:13:53 | 0 |
  | P-play LL (`PF1_RUNS=3`, controls included) | 08:13:53–08:47:58 | 0 |
  | P-play LH (`PF1_RUNS=3`, controls included) | 08:47:58–09:22:06 | 0 |
  | P-seek LL (`PF1_RUNS=3`) | 09:22:06–09:29:47 | 0 |
  | P-seek LH (`PF1_RUNS=3`) | 09:29:47–09:37:21 | 0 |
  | P-rss LL (G15, unpinned, provisional) | 09:37:21–09:41:26 | 0 |
  | P-rss LH | 09:41:26–09:44:07 | 101 (E12.8) |
  | G18 LH, S0 | 09:44:07–09:54:06 | 0 |
  | G18 LH, S2b | 09:54:06–09:55:57 | 0 |
  | P-rss LH, rerun | 09:56:19–09:58:56 | 101 (E12.8) |

- **Ambient load (R26), unchanged:**
  - the two `foot` screensaver processes (≈91% + 52%, lifetime averages from `ps`) and Hyprland (≈24%) were present
    at every BEGIN/END;
  - two `gh` processes appeared at the G18 S2b END mark only (elapsed 00:00, after that lane had finished);
  - 1-minute load averages were 4.3–13.7, and at least 20 GiB was free at every mark.

  The seek and play lanes raise load themselves: they run reader threads.
- **Commands.** Each is `--exact --ignored --nocapture --test-threads=1` from `crates/kinewright-media`:
  - `mo2_perf_fixtures::r28_end_to_end_tracked` (LL, then `R28_HARDWARE=1` for LH);
  - `mo2_perf_fixtures::blend_heavy_holds_floors_on_hardware`;
  - `pf1_harness::pf1_play_baseline` (`PF1_RUNS=3`; `PF1_HARDWARE=1` for LH);
  - `pf1_harness::pf1_seek_baseline` (the same variables);
  - `pf1_harness::pf1_rss_baseline`;
  - `pf1_export_lane::pf1_export_lane` (`PF1_HARDWARE=1 PF1_RUNS=3 PF1_EXPORT_CLIPS=24`).

  Output was logged unfiltered. The full log and the mutation logs are gzipped in `target/review/pf/s2b-logs/` in the
  main checkout.
- **Full `cargo test --workspace`** at `cce86e1`, run before the lanes: every test target passed (36 `test result: ok` lines),
  with 3,459 tests passed, 0 failed and 42 ignored. Media alone: 960 passed and 21 ignored, in 206 s at the default
  thread count.

### E12.2 Line counts

Non-blank, non-comment `.rs` lines, net (added − removed), split at each file's `#[cfg(test)] mod tests`:

| Commit | Budget | Production | Tests | Total | Against budget |
|---|---|---|---|---|---|
| S2b-1 `72246d3` | ~450 | 945 | 635 | 1,580 | 3.5× |
| S2b-2 `2a632fa` | ~200 | 202 | 312 | 514 | 2.6× |
| S2b-3 `e0a15ac` | ~250 | 560 | 506 | 1,066 | 4.3× |
| S2b-4 `cce86e1` | ~150 | 68 | 143 | 211 | 1.4× |
| **Stage** | **~1,050** | **1,775** | **1,596** | **3,371** | **3.2×** |

Per file:

- **S2b-1:**
  - `sched.rs` 450 production + 425 tests;
  - `preview.rs` 284 + 192;
  - `render.rs` 178 + 18;
  - `decode.rs` 32 (the packet-boundary stop).
- **S2b-2:**
  - `sched.rs` 128 + 178;
  - `preview.rs` 73 + 113;
  - `perf_fixtures.rs` 21 (test fixtures).
- **S2b-3:**
  - `sched.rs` 252 + 301;
  - `preview.rs` 230 + 191;
  - `pf1_export_lane.rs` 55 (a test-only lane, counted as production because it is its own file);
  - `render.rs` 18;
  - `perf_fixtures.rs` 14;
  - `pf1_harness.rs` 3.
- **S2b-4:**
  - `engine.rs` 47 + 41;
  - `preview.rs` 14 + 102;
  - `render.rs` 7.

**Why the overrun.** S2b-1, S2b-2 and S2b-3 are each more than 50% over budget. No test was trimmed.

- **S2b-1:**
  - The budget covered `Sched` and the readers. The commit also carries:
    - the preview's side of the move, which posts versioned plans, waits in FrameWait (the H-2 table) and composites
      reader frames with `render_scheduled`;
    - the renderer split that keeps C-5 byte-identical (`render.rs`, 178);
    - the reader thread's packet-boundary stop (`decode.rs`).
  - Tests: the H-8 model (a bounded exploration, E12.13), the owed-reader and FrameWait tables, and real-thread C-5, quiescence, E-2,
    agent-suspension and shutdown-in-every-phase witnesses.
- **S2b-2:**
  - Production is on budget (202).
  - The tests add the permit book to the model (1.07 M states), plus the P arithmetic and the real-thread G13/I11
    witnesses.
- **S2b-3:**
  - K-1 counts five kinds of owner against C (rings, reservations, decodes in flight, render pins and title G). Each is
    a drop guard (`Hold`) released after unlock, with the H-4 debug flag.
  - Draining needs a stop at a packet boundary and a way back in after any release.
  - Eviction and title trimming are the rest.
  - The G18 lane is 55 more lines.
  - Tests: the model's admission, drain and cancel events with independent K-2 oracles, plus eight real-reader
    witnesses.

### E12.3 Must-pass items and mutations

Every mutation was applied alone to the committed code, and the named tests were run. The logs are
`s2b{1,2,3,4}-mutations.log`.

| Item | Evidence (tests, all green at `cce86e1`) | Mutations killed (witness) | Survived, with disposition |
|---|---|---|---|
| **I15 model** (S2b-1+) | `sched::tests::the_reader_model_holds_for_every_short_sequence` (bounded exploration, not exhaustive: every sequence of five events from each P's start, four from the two targeted starts, P ∈ {1, 2, 3, 20}, 1.07–1.1 M states, liveness from every leaf, step-budgeted; condvar wake-ups and cache clears are not modelled, and have real-thread witnesses; R37 qualification, review A S2); `…_for_seeded_sequences`; `an_owed_reader_outlives_the_quiescence_deadline`; `frame_wait_steps_follow_the_table`; `a_reader_retired_mid_decode_stays_retiring` | M41 stale failure recorded, M42, M43 rings not pruned, M45 no per-source cap, M46 double decode (model); M44 (owed-reader test); M47 (agent push); M48 relabelled frame (C-5 witness); M83 `deliver` overwrites Retiring (the new test) | — |
| **I11 / G13** (S2b-2) | the model with the permit book; `permits_split_the_pool_by_the_plan`; `a_widening_plan_rebalances_the_permits`; `shutdown_wakes_the_readers_waiting_for_permits`; four sources at P = 20 hold 5 × 4 = 20 while a thumbnail completes | M49 FIFO bypass, M50 lost cancel, M51 no shrink, M52 no growth, M53 release keeps permits, M54, M55, M56 shutdown does not end a wait, M57 grant over free; M59, M60 (real threads) | **M58** (a grant does not wake the next head): survives. It slows the dense stress 2.9×, because every release, close and cancel also notifies `permits_cv`. Recorded as masked, as at S2b-2. |
| **I12** (S2b-3) | the model's I12 bound and K-2 oracles; `admission_drains_lookahead_to_fit_the_required_set`; `a_required_set_over_c_falls_back_to_the_synchronous_renderer`; `lookahead_stops_at_its_share_of_c_less_g`; `eviction_takes_over_share_sources_then_the_farthest`; `a_drain_stops_lookahead_and_holds_the_render_for_its_titles`; `title_rasters_are_trimmed_to_the_job`; `a_release_under_sched_panics` (H-4); the stress | M61, M64 beyond C, M65 H ignored, M66 share ignored, M67 lookahead kept while draining, M68 share ignores G, M69, M70 eviction order, M71, M72 reservations kept; M62 and M76 (K-3 check, count); M73 and M79 (drop guard, H-4 flag); M74 (H-4); M75 and M77 (drain stop, render waits); M78 (title trim); M82 (halt under Sched) | **M80** (a drain that unlocked waits without re-admitting): equivalent in the reachable states. The first unlock is followed by the post's own `continue`. After it, draining admits no lookahead and drops delivered lookahead, so a later wait has nothing to evict. **M63 and M81** are void: M63's target branch was dead code and was removed; M81's target check was reverted. |
| **I10** (S2b-4) | `engine::tests::an_idle_engine_holds_no_preview_reader_or_decoder` (sequence below); `a_preview_spawn_failure_is_prefixed` | M84 eager preview spawn ("+0 threads at construction", 0.14 s); M85 parked preview keeps decoders; M86 `release_sources` keeps decoders; M87 idle reader never quiesces; M88 spawn failure not reported | — |
| **I15 stress** (S2b-4) | `preview::tests::the_scheduler_survives_a_seeded_stress_on_real_threads` (details below; ≈23 s) | M64 admission beyond C (3.76 s); M73 drop guard does not release (60.77 s: a checkpoint got no frame) | **M89** (a decode stop is never cleared): survives the stress, which checkpoints every eighth sequence. The drain test kills it (watchdog, 120 s): it now renders a later frame from the stopped reader. |

- **The I10 test runs this sequence:**
  1. A barrier: `set_event_wakeup` is set on the worker.
  2. At construction: 0 previews and 0 readers.
  3. After the first frame: 1 preview and 2 readers.
  4. A thumbnail runs. After its park, `decoders.open() == 0`.
  5. The injected skew reaches QUIESCENCE, and the slots empty.
  6. At the end: 1 preview and 0 readers.
- **The stress test:**
  - It runs 10,000 sequences over eight (P, C) lanes: P ∈ {1, 2, 3, 20}, with C from 2.5 frames to 224 MiB.
  - Each sequence has 1–4 events: paused, playback and empty posts, agent pushes, and quiescence ticks.
  - Every eighth sequence ends in a paused post, which must publish within 60 s.
  - After each sequence it checks: peak ≤ C, readers ≤ R, and permits ≤ P.
  - At shutdown it checks: no slot, 0 live bytes and 0 permits.
  - It runs under a 600 s watchdog.
- **The stress's density was tuned (a deviation in method, recorded).** Two denser variants came first:
  - 40 × 250 steps, checkpointed every 50 steps: M58, M80, M83 and M89 all survived it.
  - A dense variant, checkpointed after every sequence: it killed M89, but took 182 s, which is too slow for the 2–4
    core CI runners.
  - The committed variant checkpoints every eighth sequence and takes ≈23 s. M89's witness moved to the drain test.
- **The stage found one S2b-1 defect.** `deliver` reset a reader that retired mid-decode to Idle, so a preview dropped
  without a lane shutdown could join it forever. The drain test's hang exposed it.
  - It is fixed in `e0a15ac`: a Retiring reader stays Retiring.
  - M83 re-applies the old code, and `a_reader_retired_mid_decode_stays_retiring` kills it.

### E12.4 I4 and G3

**I4** (S0's 5% rule; `BASELINES` untouched). S0's baseline is 497.86 ms (LL) and 494.39 ms (LH).

| Build | LL mean ms | LL delta | LH mean ms | LH delta | Result |
|---|---|---|---|---|---|
| S1 end `217986a` (E10.2) | 77.03 / 76.23 / 75.95 | −84.7% | 72.87 / 73.00 / 73.69 | −85.2% | ok |
| S2a R32 `af5abf9` (E11.10.4) | 76.71 / 76.97 / 76.63 | −84.6% | 73.16 / 74.16 / 72.95 | −85.1% | ok |
| S2b `cce86e1` | 76.98 / 75.54 / 77.16 | −84.6% | 73.58 / 73.46 / 73.33 | −85.1% | **ok** |

**G3** (60 fps floor, LH). S0 ("Today") was 27.4 fps.

| Build | `blend_heavy_1080p` fps (3 runs) | mean ms | p95 ms | Result |
|---|---|---|---|---|
| S1 end `217986a` (E10.3) | 72.1 / 71.4 / 71.7 | 13.86–14.00 | 15.47–15.67 | passes |
| S2a R34 `9a3c583` (E11.12.4) | 71.0 / 72.5 / 70.9 | 13.79–14.10 | 15.23–15.90 | passes |
| S2b `cce86e1` | 70.7 / 71.2 / 71.7 | 13.95–14.15 | 15.46–16.31 | **passes** |

- `typical_1080p` reads 72.2–74.8 fps. `heavy_4k` reads 58.2–59.0 fps; it is not gated, and it was below the floor
  before too.
- The slowdown control fails its verdict (17.7 fps), as it must.
- Phases for `blend_heavy_1080p`: `upload_ms` 10.08, `gpu_passes_readback_ms` 1.46, `monitor_encode_ms` 1.66.

### E12.5 P-play: G1, G6, G11, G14, G16, G17 (simulated driver)

Three runs per workload per lane. Every run is valid: 60.02 s (240.01–240.03 s for `talk_recut`), with 0 missed
callbacks.

| Workload | Lane | Harness on time / late / dropped | Engine on time / late / dropped | Present p50 / p95 / max ms | p95/p50 (G6) | Held max ms (harness) | Clock stall max ms, harness / engine | Underrun frames / post-end | `sync_fallback_frames` (G17) | `lookahead_starved` |
|---|---|---|---|---|---|---|---|---|---|---|
| `typical_1080p` | LL | 1021–1078 / 5–14 / 716–774 | 1020–1076 / 6–14 / 716–774 | 63.4–63.7 / 85.5–85.9 / 281.7–356.5 | 1.35 | 279.8–356.1 | 48.6–54.8 / 24.5–26.0 | 0 / 512 | 0 | 705–735 |
| `blend_heavy_1080p` | LL | 1782–1793 / 0–1 / 7–17 | 1782–1793 / 0–1 / 7–17 | 37.6–37.8 / 42.9 / 64.4–112.0 | 1.1 | 145.1–150.1 | 47.2–47.7 / 23.4–24.0 | **0** / 512 | 0 | 1954–2069 |
| `explainer_16x9` | LL | 1796 / 0 / 4 | 1796 / 0 / 4 | 42.0 / 43.2 / 43.7–44.2 | 1.0 | 150.1 | 45.9–48.8 / 21.9–25.8 | 0 / 512 | 0 | 908–926 |
| `reel_9x16` | LL | 1778–1788 / 0 / 12–22 | 1778–1788 / 0 / 12–22 | 40.2–40.5 / 43.3 / 88.5–120.1 | 1.1 | 86.6–119.8 | 45.7–47.1 / 24.2–24.4 | 0 / 512 | 0 | 251–253 |
| `feed_4x5` | LL | 1641–1648 / 0 / 152–159 | 1641–1648 / 0 / 152–159 | 38.4–39.2 / 64.0 / 86.0–130.6 | 1.6–1.7 | 85.4–128.1 | 47.5–50.6 / 23.9–25.9 | 0 / 512 | 0 | 300–330 |
| `talk_recut` | LL | 7199 / 0 / 1 of 7200 | 7199 / 0 / 1 | 42.0 / 43.2–43.3 / 44.0–44.1 | 1.0 | 60.1–60.2 | 46.9–60.0 / 22.0–42.9 | 0 / 0 | 0 | 0 |
| `typical_1080p` | LH | **1621–1665 / 3–6 / 132–174** | 1622–1665 / 3–7 / 132–174 | 37.9–39.3 / **45.6–63.5** / 218.6–384.5 | 1.2–1.6 | **214.2–383.0** | 47.3–49.0 / 23.6–24.2 | 0 / 512 | 0 | 1017–1093 |
| `blend_heavy_1080p` | LH | 1797 / 0 / 3 | 1797 / 0 / 3 | 42.0 / 43.2–43.3 / 43.7–44.2 | 1.0 | **115.1–125.1** | 46.4–47.2 / 21.8–23.0 | 0 / 512 | 0 | 2142–3674 |
| `explainer_16x9` | LH | 1797 / 0 / 3 | 1797 / 0 / 3 | 42.0 / 43.3 / 43.8–44.1 | 1.0 | **110.1–120.1** | 45.5–47.0 / 21.8–28.2 | 0 / 512 | 0 | 908–931 |
| `reel_9x16` | LH | 1799 / 0 / 1 | 1799 / 0 / 1 | 42.0 / 43.2–43.3 / 43.7–45.5 | 1.0 | 55.1–65.1 | 45.7–47.4 / 21.8–22.3 | 0 / 512 | 0 | 248–253 |
| `feed_4x5` | LH | 1799 / 0 / 1 | 1799 / 0 / 1 | 41.8 / 43.2–43.3 / 43.8–45.8 | 1.0 | 45.3–45.9 | 45.6–46.5 / 22.6–23.6 | 0 / 512 | 0 | 464–466 |
| `talk_recut` | LH | 7199 / 0 / 1 of 7200 | 7199 / 0 / 1 | 42.0–42.1 / 43.2–43.3 / 43.9–44.2 | 1.0 | 60.1–60.4 | 45.6–60.0 / 22.0–42.9 | 0 / 0 | 0 | 0 |

**`typical_1080p` on LH, run by run (G1).** No run satisfies both conditions: ≤ 1% late/held/dropped, and p95 ≤
50 ms. Run 0's p95 of 45.6 ms is within 50 ms, but its 7.5% is not within 1%.

| Build | On time / late / dropped of 1800 | Late + dropped | Present p50 / p95 ms | Held max ms |
|---|---|---|---|---|
| S0 (E9.3) | 0 / 61–65 / 1735–1739 | 100% | 986–1010 / 1453–1568 | 1614–1723 |
| S1 end (E10.5) | 523 / 590 / 687 | 71% | 38.3 / 145.6 | 185.8 |
| S2a R33 (E11.11.4) | 574–575 / 4–8 / 1218–1221 | 68% | 64.2 / 258.9–271.6 | 335.8–384.6 |
| S2a R35 (E11.13.2) | 576 / 9 / 1215 | 68% | 64.2 / 256.6 | 340.1 |
| S2b run 0 | 1665 / 3 / 132 | 7.5% | 37.9 / 45.6 | 214.2 |
| S2b run 1 | 1621 / 5 / 174 | 9.9% | 39.3 / 63.5 | 253.0 |
| S2b run 2 | 1645 / 6 / 149 | 8.6% | 39.2 / 61.9 | 383.0 |

Gate verdicts:

- **G1 (LH) fails.** S2b takes `typical_1080p` from 32% on time (S2a) to 90–92%, and p95 from ≈257 ms to 46–64 ms.
  That is still short of ≤ 1% and ≤ 50 ms. See E12.10 D1.
- **G6 passes** on LL and LH: `typical_1080p` p95/p50 is 1.35 (LL) and 1.2–1.6 (LH), against ≤ 3. The largest
  ratio in any workload is 1.7 (`feed_4x5` LL).
- **G14 (LH) fails** on three workloads:
  - `typical_1080p`: 214–383 ms;
  - `blend_heavy_1080p`: 115–125 ms;
  - `explainer_16x9`: 110–120 ms.

  It passes on `reel_9x16` (55–65), `feed_4x5` (45–46) and `talk_recut` (60). The freeze control fails it, as it
  must (E12.6).
  - `blend_heavy_1080p` and `explainer_16x9` drop exactly 3 frames per run. Their held maximum is consistent with one
    3-frame gap. This run did not localise the gap.
- **G11 passes:** 0 underrun frames before the end, in every run on both lanes. The post-end straddle is 512, as
  before.
- **G16 passes:** the harness's maximum clock stall is 60.0 ms (`talk_recut`), against 100 ms. The clock-freeze
  control fails it.
- **G17 passes:** `sync_fallback_frames` is 0 in every run of every workload on both lanes.

Other fields, identical in every run:

- The engine and harness agree within ±6.
- `engine_stale_errors`, `engine_dropped_agent`, `engine_acks_overflowed`, `engine_acks_unmatched` and
  `consumer_rejected` are 0.
- The receipt offset is 0.0.
- `engine_sync_decoders` is 0 (S2a: 1–4): transport renders no longer open synchronous decoders.
- Teardown is complete in every run, with 0 decoders and 44 (LL) or 7 (LH) threads, as in S2a.

RSS:

- Teardown RSS is 665–947 MiB (LL) and 505–797 MiB (LH). In S2a's E11.11.4 process it grew to 2,569 MiB.
- Peak RSS while playing is higher than S2a's: 1,044–1,528 MiB (LL) and 1,008–1,573 MiB (LH). E11.13.2 read 854
  and 892 MiB for `typical_1080p`. The scheduler now holds up to C = 224 MiB of lookahead, and there are no RSS
  gates on playback (G15 is settled idle).

**LL `typical_1080p`** has 1,021–1,078 on time, with p50 ≈63.5 ms. On llvmpipe the render itself is the likely bound (not
probed). G1 is gated on LH only. `blend_heavy_1080p` LL reaches 99.0–99.6% on time.

### E12.6 Q-3 controls

Each control must fail its gate.

| Control | Lane | Gate | Fails, as it must | Clock stall max ms, harness / engine | Held max ms | Underrun frames / post-end | On time, harness / engine |
|---|---|---|---|---|---|---|---|
| Slowdown | LL | G1 | yes | 50.0 / 24.6 | 754.6 | 0 / 512 | 271 / 269 |
| Freeze | LL | G14 | yes | 48.9 / 25.4 | 1065.1 | 0 / 512 | 1063 / 1065 |
| ClockFreeze | LL | G16 | yes | 1050.0 / 1026.7 | 1091.6 | 0 / 512 | 1084 / 1082 |
| Stall | LL | underruns | yes | 1045.0 / 1024.0 | 1043.8 | 48128 / 512 | 1052 / 1053 |
| Slowdown | LH | G1 | yes | 46.1 / 22.6 | 635.3 | 0 / 512 | 547 / 549 |
| Freeze | LH | G14 | yes | 47.5 / 22.6 | 1298.9 | 0 / 512 | 1612 / 1612 |
| ClockFreeze | LH | G16 | yes | 1050.0 / 1027.5 | 1037.2 | 0 / 512 | 1669 / 1669 |
| Stall | LH | underruns | yes | 1045.0 / 1024.0 | 1044.1 | 48128 / 512 | 1689 / 1689 |

### E12.7 L-6 and P-seek

Paused, as E9.6. The table shows the worst run's latency, the smallest distinct fps, and counts and release ms as
ranges. S2a rows are from E11.10.4 and S0 rows from E9.6.

| Workload | Lane | Build | Random p95 / max | +1 p95 | Drag p95 / answered | Unanswered | Distinct fps (L-5) | Release shown / ms (L-6) | Stale / over / valid over release |
|---|---|---|---|---|---|---|---|---|---|
| `seek_gop60` | LL | S0 | 117.5 / 128.8 | 118.1 | 196.8 / 194.3 | 0–2 | 10.2 | all / 86.1–170.6 | 0–1 / 0 / — |
| | | S2a | 62.7 / 73.7 | 68.1 | 102.4 / 102.4 | 0 | 23.6 | all / 44.1–71.9 | 1 / 0 / 0 |
| | | **S2b** | 66.5 / 134.2 | 18.4 | 400.4 / 346.8 | 0–6 | 13.4 | all / 14.9–37.9 | 0–1 / 0 / 0 |
| `talk_recut` | LL | S0 | 162.0 / 223.4 | 181.0 | 236.7 / 232.4 | 0–2 | 8.8 | all / 129.1–173.7 | 1 / 0 / — |
| | | S2a | 107.4 / 178.1 | 124.9 | 137.1 / 137.1 | 0 | 18.8 | all / 70.4–125.4 | 1 / 0 / 0 |
| | | **S2b** | 110.4 / 181.8 | 24.5 | 1708.3 / 1675.0 | 1–7 | 6.4 | all / 20.9–101.3 | 0 / 0 / 0 |
| `explainer_16x9` | LL | S0 | 211.6 / 273.1 | 224.5 | 359.8 / 359.8 | 0–1 | 6.8 | all / 178.9–296.6 | 1 / 0 / — |
| | | S2a | 140.3 / 180.6 | 135.7 | 234.4 / 234.4 | 0 | 14.6 | all / 85.3–174.0 | 1 / 0 / 0 |
| | | **S2b** | 159.7 / 193.0 | 26.4 | inf / 2667.6 | 3–35 | 3.0 | all / 47.8–168.6 | 0 / 0 / 0 |
| `seek_gop60` | LH | S0 | 119.8 / 127.0 | 125.1 | 197.9 / 197.7 | 1–2 | 10.0 | all / 94.1–183.1 | 1 / 0 / — |
| | | S2a | 62.1 / 69.0 | 67.3 | 99.7 / 99.7 | 0 | 24.2 | all / 36.9–99.3 | 1 / 0 / 0 |
| | | **S2b** | 61.4 / 71.1 | 16.6 | 470.6 / 470.6 | 0–6 | 13.8 | all / 17.7–50.1 | 0–1 / 0 / 0 |
| `talk_recut` | LH | S0 | 162.8 / 225.7 | 189.8 | 231.5 / 231.5 | 0 | 9.0 | all / 189.6–238.7 | 1 / 0 / — |
| | | S2a | 107.4 / 155.7 | 134.7 | 130.7 / 130.2 | 0–1 | 19.0 | all / 53.9–128.5 | 1 / 0 / 0 |
| | | **S2b** | 104.7 / 172.7 | 22.2 | inf / 1506.0 | 1–37 | 6.2 | all / 19.7–87.0 | 0 / 0 / 0 |
| `explainer_16x9` | LH | S0 | 208.0 / 270.1 | 222.3 | 387.9 / 366.2 | 2 | 6.6 | all / 109.4–243.6 | 1 / 0 / — |
| | | S2a | 140.3 / 174.0 | 137.1 | 237.8 / 237.0 | 1–2 | 14.6 | all / 55.0–102.7 | 1 / 0 / 0 |
| | | **S2b** | 159.1 / 183.9 | 22.5 | inf / 2595.9 | 3–43 | 3.4 | all / 54.1–112.0 | 0 / 0 / 0 |

- **L-6 passes on both lanes.** The release target is shown in every run, `valid_frames_over_release` is 0, and 0
  timeouts. Release latency is within or below S2a's range, except `explainer_16x9` LH, whose worst run (112.0 ms) is
  above S2a's worst (102.7).
- **+1 steps improved sharply.** Their p95 is 14.7–26.4 ms, against 67–138 ms at S2a, because the ring's lookahead
  now serves them. `plus1_n` is only 8–17 per run.
- **Random seeks are unchanged**, within noise of S2a.
- **The 30 Hz drag regressed**, against S2a on every workload, and against S0 on `talk_recut` and
  `explainer_16x9`. Up to 35–43 drag calls are unanswered at release. G8, including L-5 (≥ 10 / 7 fps at GOP
  60 / 250), is S2c's gate, not S2b's. Today `seek_gop60` (13.4–13.8) would clear 10, and `talk_recut` (6.2–6.4)
  would miss 7. See E12.10 D2.

### E12.8 G15 (P-rss, unpinned, provisional)

G15 is measured unpinned on LL, with the screensaver running (R26). It is labelled **provisional**; the pinned verdict
is taken at S4 with Riel. Fresh child process per workload, as E9.5. Each cell is resident MiB / threads.

| Workload | S0 settled idle (E9.5) | S2b constructed | S2b first render | S2b settled idle | Δ vs S0 | ≤ S0 + 4 MiB |
|---|---|---|---|---|---|---|
| `typical_1080p` | 584.8/140 | 158.4/48 | 503.1/131 | 473.1/49 | −111.7 | yes |
| `blend_heavy_1080p` | 677.2/186 | 158.7/48 | 494.9/160 | 457.2/49 | −220.0 | yes |
| `explainer_16x9` | 546.4/140 | 158.7/48 | 472.3/131 | 443.1/49 | −103.3 | yes |
| `reel_9x16` | 492.5/94 | 158.6/48 | 401.7/96 | 393.8/49 | −98.7 | yes |
| `feed_4x5` | 402.7/94 | 158.5/48 | 406.4/96 | 395.8/49 | −6.9 | yes |
| `talk_recut` | 381.8/94 | 158.5/48 | 380.7/96 | 372.9/49 | −8.9 | yes |
| `title_only` | 236.0/48 | 158.6/48 | 237.4/49 | 237.4/49 | **+1.4** | yes |

- **G15 passes provisionally** on every workload, including the title-only document (+1.4 MiB, within +4).
- **H-7 thread counts, from the same lines:**
  - at construction: 48 threads, the same as S0's 48 (+0; the preview is not started);
  - settled: 49, the parked preview alone, where S0 kept 94–186;
  - playing: 74–161.
- **Constructed RSS** is 158.4–158.7 MiB, against 169.9–170.2 at S0.
- **LH is recorded, not gated, and is partial:**
  - `typical_1080p`: settled 443.6/12 (first run) and 486.7/12 (rerun), against S0's 624.9/103.
  - `blend_heavy_1080p`: measured 436.6/12 and 434.1/12, against S0's 711.0/149.
  - Each time, the child then exited with SIGSEGV after printing its line and `test result: ok`, so the harness
    panicked (`pf1_harness.rs:920`) and the lane stopped after two workloads. It reproduced on the rerun.
- **The LH crash backtrace** (`coredumpctl debug`) shows a teardown race at process exit:
  - The preview thread is in `drop_glue::<Preview>` → `FrameRenderer` → `Compositor` → `wgpu` `Device` →
    `vkDestroyDevice`, and it faults in `libnvidia-glcore`.
  - The worker thread is in `Worker::shut_down`, joining the preview.
  - The main thread is already in the NVIDIA libraries' exit-time teardown.
  - `FfmpegMediaEngine` has no `Drop` that joins its worker, so the worker's shutdown can outlive the engine. This was
    also true before S2b, and E9.9 and E10.5 record earlier exit-time GPU teardown failures.
  - E12.10 D3 proposes the amendment.

### E12.9 G18 (export lane, LH)

`typical_1080p` first 4 s: 3 tracks, 24 clips × 15, 120 frames at 1920 × 1080, through the production
`export_document`.

| Build | When (EDT, 2026-09-28) | Wall ms (3 runs) | Median ms | Bytes | vs S0 |
|---|---|---|---|---|---|
| S0 `d19bdf9` (+ lane) | 02:47–02:57 | 198471.0 / 198555.6 / 197997.1 | 198471.0 | 3375562 | — |
| S2b-2 `2a632fa` (+ lane; binary sha256 `718fddb0…`) | 02:57–02:59 | 36313.6 / 35775.6 / 35844.4 | 35844.4 | 3375562 | −81.9% |
| S0 `d19bdf9` (+ lane), stage end | 09:44–09:54 | 198576.2 / 198262.2 / 198715.2 | 198576.2 | 3375562 | — |
| S2b `cce86e1`, stage end | 09:54–09:56 | 35916.1 / 36299.4 / 35993.9 | 35993.9 | 3375562 | **−81.9%** |

- **G18 passes** (≤ S0 + 5%), both at S2b-2 and at the stage end.
- **Identity is not established by these runs** (R38, review B S1). They recorded only each file's length, and every
  run's length is 3,375,562 bytes. Equal lengths are consistent with identical exports but do not show them; a
  consistently changed export of the same length would pass. The lane now prints each run's SHA-256 and asserts one
  hash per build (R37-13), and the runner's summary step (`g18_verdict.py`, the last line of the gate plan) compares
  the S0 exports' hash with the candidate's across the lane logs. Until that rerun, the identity claim stands
  unproven. The gain is S1's X-1 conversion. The synchronous decoders keep min(P, 16) threads outside the permit pool
  (R19), so S2b does not slow export.

### E12.10 Deviations and proposed amendments

**D1: G1 and G14 fail on LH (S2b-4 gates).**

- **What the evidence shows.** The lookahead is capped by K-2's share. `lookahead_starved` is 1,017–1,093 per
  `typical_1080p` LH run.
  - **Qualification (R37, review B S4).** `lookahead_starved` (the scheduler's `starved_keys`) counts K-2 refusals,
    at most once per source per plan. It does not count stalled presentations: a refusal need not delay any frame.
    It shows the share binding often, not that a refusal caused a drop.
- **E8's arithmetic** gives `typical_1080p` 13 frames of lookahead per source: (224 − 21.09 − 14.06) / 2 MiB at
  7.03 MiB per 1280×720 working frame.
  - Three tracks share two sources, so one source serves two regions. That leaves ≈6–7 frames (≈0.2 s) ahead per
    region.
  - The clips are 0.5 s (15 frames), and every cut jumps to a new source position, which needs a seek and a decode
    to the target.
  - A region's next clip cannot be decoded far enough ahead, so a cut sometimes lands before its frame is ready. That
    produces the dropped frames and the held ages of 214–383 ms.
- **This is a hypothesis, not probed.** It fits the counters and the arithmetic. No probe varied C or the share to
  confirm it, because the brief forbids silent deviation from K-2's numbers.
- **Proposed amendment options** (the orchestrator's choice):
  - (a) **Compact lookahead:** store lookahead as the decoder's 4:2:0 planes, ≈1.4 MB at 720p, and convert on pin.
    That fits ≈5× the frames in C. X-1 conversion would move from the reader to pin time; C-5 is unaffected if
    the same X-1 tables run there. The cost is ≈2–3 ms per pinned frame (the E0 model).
  - (b) **Share per region,** not per source (n = regions), so a two-position source gets two shares.
  - (c) **A lookahead budget sized from the plan:** at least one clip length per region, within the ledger ceilings
    (I3), in place of a fixed C.
  - (d) **Re-stage the gates:** move G1 and G14 to the stage that lands (a)–(c), and record S2b's numbers as interim.
- **Recommendation:** a short probe of (a) against (b) on `typical_1080p` LH before choosing.
- **G14 on `blend_heavy_1080p` and `explainer_16x9`** (110–125 ms, 3 drops per run) may be a start-up or single-cut
  gap. The probe should localise it.
  - **Qualification (R37, review B S4).** Their `present_max_ms` is below 45 ms, while their held maxima exceed
    110 ms. The long held interval therefore lies at a measurement boundary, not between two recorded presentations:
    `Trace::metrics` measures held age from the trace's start until the first presentation, and from the last
    presentation until the window's end. The three drops are not attributed to a cut until that interval is
    localised.

**D2: drag regression (not an S2b gate; G8/L-5 is S2c).**

- Drag distinct fps fell:
  - `seek_gop60`: 13.4–13.8, against 23.6–24.2 at S2a;
  - `talk_recut`: 6.2–6.4, against 18.8–19.0;
  - `explainer_16x9`: 3.0–3.4, against 14.6.
- For `talk_recut` and `explainer_16x9` this is below S0.
- **Hypothesis, not probed:** reader churn on each drag post.
  - Each 30 Hz `request_frame` posts a new plan.
  - Regions move, and readers are reassigned, and shrink or close-before-growth reopens a decoder at a new permit
    width.
  - During the drag, lookahead is dropped as obsolete.
  - S2a's synchronous renderer kept one warm decoder per source and never reopened.
- **Proposed:** S2c starts with this.
  - Keep a reader's decoder across a plan change when the new region is on the same source and ahead of it (C-4's
    continuation domain).
  - Defer shrink or growth while the transport is dragging.
  - Add a drag counter of reader reopens to P-seek.
- Until S2c, scrubbing on multi-source documents is slower than S2a. Riel should know this before any hands-on use of
  `pf1/impl`.

**D3: P-rss LH exits with SIGSEGV (E12.8).**

- The measurement lines print, but the child's process exit races the worker's GPU teardown.
- **Proposed:**
  - (a) **Harness:** P-rss's child waits for the engine's teardown record before returning, as P-play's teardown
    telemetry does. Until then, a crash after `test result: ok` is recorded, not treated as a lane failure.
  - (b) **Engine,** a separate small change: join the worker on `FfmpegMediaEngine` drop, or give it an explicit
    shutdown.
- (b) also closes the app's exit-time variant. It is outside S2b's scope, so it is not done here.
- The LL lane, which is G15's lane, completed.

**D4: the stress's density (method).** The committed stress checkpoints every eighth sequence, so that it fits CI
(E12.3). M89's witness moved to the drain test. M58 survives it, as it did the S2b-2 witnesses (masked by the other
permit notifications).

**D5: line budget** (E12.2). The stage is 3,371 lines against ≈1,050. No test was trimmed.

**D6: G15 is measured unpinned and provisional,** per the brief. The pinned verdict is at S4.

### E12.11 Raw result lines

`R28` and `PF1` lines from the stage-end log, in run order:

- I4 LL, then I4 LH, then G3;
- P-play LL, then LH, each with its controls;
- P-seek LL, then LH;
- P-rss LL (7 lines);
- the first P-rss LH attempt (2 lines);
- G18 S0, then G18 S2b;
- the P-rss LH rerun (2 lines).

In each P-rss LH attempt, the second line is the child's for `blend_heavy_1080p`. It carries no lane or workload
fields, because the parent panicked before it could label the line.

The G18 lines from S2b-2 come from a separate runner log. They are listed first.

```
PF1 export lane=LH workload=typical_1080p frames=120 run=0 wall_ms=198471.0 bytes=3375562   (S0, at S2b-2)
PF1 export lane=LH workload=typical_1080p frames=120 run=1 wall_ms=198555.6 bytes=3375562   (S0, at S2b-2)
PF1 export lane=LH workload=typical_1080p frames=120 run=2 wall_ms=197997.1 bytes=3375562   (S0, at S2b-2)
PF1 export lane=LH workload=typical_1080p frames=120 runs=3 median_ms=198471.0 min_ms=197997.1 max_ms=198555.6   (S0, at S2b-2)
PF1 export lane=LH workload=typical_1080p frames=120 run=0 wall_ms=36313.6 bytes=3375562   (S2b-2)
PF1 export lane=LH workload=typical_1080p frames=120 run=1 wall_ms=35775.6 bytes=3375562   (S2b-2)
PF1 export lane=LH workload=typical_1080p frames=120 run=2 wall_ms=35844.4 bytes=3375562   (S2b-2)
PF1 export lane=LH workload=typical_1080p frames=120 runs=3 median_ms=35844.4 min_ms=35775.6 max_ms=36313.6   (S2b-2)
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=false workload=typical_1080p run=0 dims=(1280, 720) mean_ms=76.98 fps=13.0 p95_ms=209.58 ledger_peak_mib=56.3 validate_us=1.5
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=false workload=typical_1080p run=1 dims=(1280, 720) mean_ms=75.54 fps=13.2 p95_ms=203.78 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) resident=false workload=typical_1080p run=2 dims=(1280, 720) mean_ms=77.16 fps=13.0 p95_ms=210.40 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=llvmpipe (LLVM 22.1.8, 256 bits) workload=typical_1080p baseline_ms=497.86 delta=-84.6%
R28 adapter=NVIDIA GeForce RTX 3090 resident=false workload=typical_1080p run=0 dims=(1280, 720) mean_ms=73.58 fps=13.6 p95_ms=206.34 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 resident=false workload=typical_1080p run=1 dims=(1280, 720) mean_ms=73.46 fps=13.6 p95_ms=203.01 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 resident=false workload=typical_1080p run=2 dims=(1280, 720) mean_ms=73.33 fps=13.6 p95_ms=201.49 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 workload=typical_1080p baseline_ms=494.39 delta=-85.1%
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=typical_1080p run=0 dims=(1280, 720) mean_ms=13.64 fps=73.3 p95_ms=15.30 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=typical_1080p run=1 dims=(1280, 720) mean_ms=13.37 fps=74.8 p95_ms=14.83 ledger_peak_mib=56.3 validate_us=1.3
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=typical_1080p run=2 dims=(1280, 720) mean_ms=13.85 fps=72.2 p95_ms=15.94 ledger_peak_mib=56.3 validate_us=1.3
R28 phases workload=typical_1080p upload_ms=10.18 gpu_passes_readback_ms=1.34 monitor_encode_ms=1.75
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=blend_heavy_1080p run=0 dims=(1280, 720) mean_ms=14.15 fps=70.7 p95_ms=16.31 ledger_peak_mib=63.3 validate_us=1.2
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=blend_heavy_1080p run=1 dims=(1280, 720) mean_ms=14.05 fps=71.2 p95_ms=15.94 ledger_peak_mib=63.3 validate_us=1.2
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=blend_heavy_1080p run=2 dims=(1280, 720) mean_ms=13.95 fps=71.7 p95_ms=15.46 ledger_peak_mib=63.3 validate_us=1.2
R28 phases workload=blend_heavy_1080p upload_ms=10.08 gpu_passes_readback_ms=1.46 monitor_encode_ms=1.66
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=heavy_4k run=0 dims=(1280, 720) mean_ms=16.96 fps=59.0 p95_ms=18.53 ledger_peak_mib=70.3 validate_us=9.2
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=heavy_4k run=1 dims=(1280, 720) mean_ms=16.97 fps=58.9 p95_ms=18.57 ledger_peak_mib=70.3 validate_us=9.3
R28 adapter=NVIDIA GeForce RTX 3090 resident=true workload=heavy_4k run=2 dims=(1280, 720) mean_ms=17.18 fps=58.2 p95_ms=18.86 ledger_peak_mib=70.3 validate_us=9.7
R28 phases workload=heavy_4k upload_ms=13.27 gpu_passes_readback_ms=1.56 monitor_encode_ms=1.64
R28 control=slowdown delay_ms=41.7 mean_ms=56.45 p95_ms=58.73 verdict=Err("17.7 fps (floor 60), p95 58.7 ms vs mean 56.5 ms")
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=typical_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1078 late=6 early=0 dropped=716 present_p50_ms=63.4 present_p95_ms=85.5 present_max_ms=356.5 held_max_ms=356.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=48.6 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1230.6 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1076 engine_late=8 engine_dropped=716 engine_dropped_agent=0 engine_held_max_ms=356.5 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=24.5 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=735 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=716.4 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=typical_1080p run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1021 late=5 early=0 dropped=774 present_p50_ms=63.7 present_p95_ms=85.9 present_max_ms=281.7 held_max_ms=279.8 receipt_offset_max_ms=0.0 clock_stall_max_ms=50.1 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1281.8 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1020 engine_late=6 engine_dropped=774 engine_dropped_agent=0 engine_held_max_ms=281.7 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=26.0 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=705 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=665.1 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=typical_1080p run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1030 late=14 early=0 dropped=756 present_p50_ms=63.6 present_p95_ms=85.8 present_max_ms=329.4 held_max_ms=325.7 receipt_offset_max_ms=0.0 clock_stall_max_ms=54.8 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1217.7 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1030 engine_late=14 engine_dropped=756 engine_dropped_agent=0 engine_held_max_ms=329.4 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=25.2 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=711 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=748.6 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=blend_heavy_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1783 late=1 early=0 dropped=16 present_p50_ms=37.6 present_p95_ms=42.9 present_max_ms=85.3 held_max_ms=150.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=47.2 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1046.2 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1783 engine_late=1 engine_dropped=16 engine_dropped_agent=0 engine_held_max_ms=142.8 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=24.0 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=1954 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=772.4 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=blend_heavy_1080p run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1793 late=0 early=0 dropped=7 present_p50_ms=37.7 present_p95_ms=42.9 present_max_ms=64.4 held_max_ms=145.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=47.3 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1046.0 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1793 engine_late=0 engine_dropped=7 engine_dropped_agent=0 engine_held_max_ms=139.2 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=23.7 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=2060 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=789.4 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=blend_heavy_1080p run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1782 late=1 early=0 dropped=17 present_p50_ms=37.8 present_p95_ms=42.9 present_max_ms=112.0 held_max_ms=150.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=47.7 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1043.8 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1782 engine_late=1 engine_dropped=17 engine_dropped_agent=0 engine_held_max_ms=142.6 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=23.4 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=2069 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=737.1 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=explainer_16x9 run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1796 late=0 early=0 dropped=4 present_p50_ms=42.0 present_p95_ms=43.2 present_max_ms=43.7 held_max_ms=150.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=48.8 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1242.7 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1796 engine_late=0 engine_dropped=4 engine_dropped_agent=0 engine_held_max_ms=144.3 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=23.6 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=926 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=796.3 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=explainer_16x9 run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1796 late=0 early=0 dropped=4 present_p50_ms=42.0 present_p95_ms=43.2 present_max_ms=44.1 held_max_ms=150.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.9 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1263.4 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1796 engine_late=0 engine_dropped=4 engine_dropped_agent=0 engine_held_max_ms=138.4 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=21.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=915 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=757.0 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=explainer_16x9 run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1796 late=0 early=0 dropped=4 present_p50_ms=42.0 present_p95_ms=43.2 present_max_ms=44.2 held_max_ms=150.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.9 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1318.7 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1796 engine_late=0 engine_dropped=4 engine_dropped_agent=0 engine_held_max_ms=145.1 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=25.8 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=908 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=683.3 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=reel_9x16 run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1782 late=0 early=0 dropped=18 present_p50_ms=40.5 present_p95_ms=43.3 present_max_ms=120.1 held_max_ms=119.8 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.7 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1318.4 ledger_peak_mib=68.4 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1782 engine_late=0 engine_dropped=18 engine_dropped_agent=0 engine_held_max_ms=120.1 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=24.3 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=253 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=736.2 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=reel_9x16 run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1778 late=0 early=0 dropped=22 present_p50_ms=40.5 present_p95_ms=43.3 present_max_ms=108.4 held_max_ms=108.4 receipt_offset_max_ms=0.0 clock_stall_max_ms=47.1 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1321.5 ledger_peak_mib=68.4 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1778 engine_late=0 engine_dropped=22 engine_dropped_agent=0 engine_held_max_ms=108.4 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=24.2 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=252 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=694.2 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=reel_9x16 run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1788 late=0 early=0 dropped=12 present_p50_ms=40.2 present_p95_ms=43.3 present_max_ms=88.5 held_max_ms=86.6 receipt_offset_max_ms=0.0 clock_stall_max_ms=46.1 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1528.2 ledger_peak_mib=68.4 table_live_kib=128 passes=true engine_due=1800 engine_on_time=1788 engine_late=0 engine_dropped=12 engine_dropped_agent=0 engine_held_max_ms=88.5 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=24.4 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=251 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=672.5 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=feed_4x5 run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1648 late=0 early=0 dropped=152 present_p50_ms=38.8 present_p95_ms=64.0 present_max_ms=86.0 held_max_ms=85.4 receipt_offset_max_ms=0.0 clock_stall_max_ms=50.6 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1473.8 ledger_peak_mib=99.0 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1648 engine_late=0 engine_dropped=152 engine_dropped_agent=0 engine_held_max_ms=86.0 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=25.1 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=330 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=772.4 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=feed_4x5 run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1641 late=0 early=0 dropped=159 present_p50_ms=38.4 present_p95_ms=64.0 present_max_ms=130.6 held_max_ms=128.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=47.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1517.8 ledger_peak_mib=99.0 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1641 engine_late=0 engine_dropped=159 engine_dropped_agent=0 engine_held_max_ms=130.6 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=25.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=316 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=759.8 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=feed_4x5 run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1641 late=0 early=0 dropped=159 present_p50_ms=39.2 present_p95_ms=64.0 present_max_ms=124.6 held_max_ms=121.7 receipt_offset_max_ms=0.0 clock_stall_max_ms=48.6 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1481.2 ledger_peak_mib=99.0 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1641 engine_late=0 engine_dropped=159 engine_dropped_agent=0 engine_held_max_ms=124.6 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=23.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=300 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=856.2 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=talk_recut run=0 valid=true elapsed_s=240.03 missed_callbacks=0 due=7200 on_time=7199 late=0 early=0 dropped=1 present_p50_ms=42.0 present_p95_ms=43.3 present_max_ms=44.1 held_max_ms=60.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=60.0 underrun_frames=0 post_end_underrun_frames=0 peak_rss_mib=1094.6 ledger_peak_mib=42.2 table_live_kib=128 passes=true engine_due=7200 engine_on_time=7199 engine_late=0 engine_dropped=1 engine_dropped_agent=0 engine_held_max_ms=44.1 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=0 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=867.3 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=talk_recut run=1 valid=true elapsed_s=240.01 missed_callbacks=0 due=7200 on_time=7199 late=0 early=0 dropped=1 present_p50_ms=42.0 present_p95_ms=43.2 present_max_ms=44.0 held_max_ms=60.2 receipt_offset_max_ms=0.0 clock_stall_max_ms=46.9 underrun_frames=0 post_end_underrun_frames=0 peak_rss_mib=1155.4 ledger_peak_mib=42.2 table_live_kib=128 passes=true engine_due=7200 engine_on_time=7199 engine_late=0 engine_dropped=1 engine_dropped_agent=0 engine_held_max_ms=44.0 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.5 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=0 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=947.4 threads=44
PF1 play lane=LL adapter=llvmpipe (LLVM 22.1.8, 256 bits) output=simulated workload=talk_recut run=2 valid=true elapsed_s=240.01 missed_callbacks=0 due=7200 on_time=7199 late=0 early=0 dropped=1 present_p50_ms=42.0 present_p95_ms=43.3 present_max_ms=44.0 held_max_ms=60.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=46.9 underrun_frames=0 post_end_underrun_frames=0 peak_rss_mib=1059.7 ledger_peak_mib=42.2 table_live_kib=128 passes=true engine_due=7200 engine_on_time=7199 engine_late=0 engine_dropped=1 engine_dropped_agent=0 engine_held_max_ms=44.0 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.0 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=0 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=844.7 threads=44
PF1 control=Slowdown lane=LL workload=typical_1080p metric=G1 fails=true valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=271 late=68 early=0 dropped=1461 present_p50_ms=160.4 present_p95_ms=393.6 present_max_ms=759.4 held_max_ms=754.6 receipt_offset_max_ms=33.3 clock_stall_max_ms=50.0 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1452.8 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=269 engine_late=69 engine_dropped=1462 engine_dropped_agent=0 engine_held_max_ms=759.4 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=24.6 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=730 consumer_rejected=1 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=864.9 threads=44
PF1 control=Freeze lane=LL workload=typical_1080p metric=G14 fails=true valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1063 late=13 early=0 dropped=724 present_p50_ms=63.3 present_p95_ms=85.4 present_max_ms=1066.1 held_max_ms=1065.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=48.9 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1439.5 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1065 engine_late=11 engine_dropped=724 engine_dropped_agent=0 engine_held_max_ms=1066.1 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=25.4 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=800 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=911.8 threads=44
PF1 control=ClockFreeze lane=LL workload=typical_1080p metric=G16 fails=true valid=true elapsed_s=61.02 missed_callbacks=0 due=1800 on_time=1084 late=2 early=0 dropped=714 present_p50_ms=63.4 present_p95_ms=85.4 present_max_ms=1092.0 held_max_ms=1091.6 receipt_offset_max_ms=0.0 clock_stall_max_ms=1050.0 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1461.1 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1082 engine_late=4 engine_dropped=714 engine_dropped_agent=0 engine_held_max_ms=1092.0 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=1026.7 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=809 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=924.6 threads=44
PF1 control=Stall lane=LL workload=typical_1080p metric=underrun_frames fails=true valid=true elapsed_s=61.02 missed_callbacks=0 due=1800 on_time=1052 late=4 early=0 dropped=744 present_p50_ms=63.5 present_p95_ms=85.4 present_max_ms=1045.7 held_max_ms=1043.8 receipt_offset_max_ms=0.0 clock_stall_max_ms=1045.0 underrun_frames=48128 post_end_underrun_frames=512 peak_rss_mib=1367.8 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1053 engine_late=3 engine_dropped=744 engine_dropped_agent=0 engine_held_max_ms=1045.7 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=1024.0 engine_underrun_events=47 engine_underrun_frames=48128 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=715 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=888.8 threads=44
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=typical_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1665 late=3 early=0 dropped=132 present_p50_ms=37.9 present_p95_ms=45.6 present_max_ms=218.6 held_max_ms=214.2 receipt_offset_max_ms=0.0 clock_stall_max_ms=48.2 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1188.9 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1665 engine_late=3 engine_dropped=132 engine_dropped_agent=0 engine_held_max_ms=218.6 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=24.2 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=1093 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=504.7 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=typical_1080p run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1621 late=5 early=0 dropped=174 present_p50_ms=39.3 present_p95_ms=63.5 present_max_ms=256.2 held_max_ms=253.0 receipt_offset_max_ms=0.0 clock_stall_max_ms=47.3 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1290.3 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1622 engine_late=4 engine_dropped=174 engine_dropped_agent=0 engine_held_max_ms=256.2 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=23.6 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=1017 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=651.5 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=typical_1080p run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1645 late=6 early=0 dropped=149 present_p50_ms=39.2 present_p95_ms=61.9 present_max_ms=384.5 held_max_ms=383.0 receipt_offset_max_ms=0.0 clock_stall_max_ms=49.0 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1272.5 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1644 engine_late=7 engine_dropped=149 engine_dropped_agent=0 engine_held_max_ms=384.5 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=24.0 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=1044 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=630.9 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=blend_heavy_1080p run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1797 late=0 early=0 dropped=3 present_p50_ms=42.0 present_p95_ms=43.3 present_max_ms=44.2 held_max_ms=125.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=46.4 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1028.8 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1797 engine_late=0 engine_dropped=3 engine_dropped_agent=0 engine_held_max_ms=114.1 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=2142 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=641.8 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=blend_heavy_1080p run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1797 late=0 early=0 dropped=3 present_p50_ms=42.0 present_p95_ms=43.2 present_max_ms=43.7 held_max_ms=120.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=46.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1133.2 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1797 engine_late=0 engine_dropped=3 engine_dropped_agent=0 engine_held_max_ms=114.4 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=23.0 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=2230 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=658.8 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=blend_heavy_1080p run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1797 late=0 early=0 dropped=3 present_p50_ms=42.0 present_p95_ms=43.3 present_max_ms=43.9 held_max_ms=115.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=47.2 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1007.6 ledger_peak_mib=63.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1797 engine_late=0 engine_dropped=3 engine_dropped_agent=0 engine_held_max_ms=113.4 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=21.8 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=3674 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=677.2 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=explainer_16x9 run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1797 late=0 early=0 dropped=3 present_p50_ms=42.0 present_p95_ms=43.3 present_max_ms=43.9 held_max_ms=120.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.6 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1445.3 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1797 engine_late=0 engine_dropped=3 engine_dropped_agent=0 engine_held_max_ms=117.2 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=24.4 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=912 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=719.4 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=explainer_16x9 run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1797 late=0 early=0 dropped=3 present_p50_ms=42.0 present_p95_ms=43.3 present_max_ms=44.1 held_max_ms=110.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1484.2 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1797 engine_late=0 engine_dropped=3 engine_dropped_agent=0 engine_held_max_ms=103.2 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=28.2 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=931 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=610.5 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=explainer_16x9 run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1797 late=0 early=0 dropped=3 present_p50_ms=42.0 present_p95_ms=43.3 present_max_ms=43.8 held_max_ms=115.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=47.0 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1524.3 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1797 engine_late=0 engine_dropped=3 engine_dropped_agent=0 engine_held_max_ms=106.3 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=21.8 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=908 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=712.9 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=reel_9x16 run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1799 late=0 early=0 dropped=1 present_p50_ms=42.0 present_p95_ms=43.3 present_max_ms=45.5 held_max_ms=65.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.9 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1408.9 ledger_peak_mib=68.4 table_live_kib=128 passes=true engine_due=1800 engine_on_time=1799 engine_late=0 engine_dropped=1 engine_dropped_agent=0 engine_held_max_ms=63.6 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.0 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=250 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=758.2 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=reel_9x16 run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1799 late=0 early=0 dropped=1 present_p50_ms=42.0 present_p95_ms=43.3 present_max_ms=44.0 held_max_ms=60.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.7 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1573.3 ledger_peak_mib=68.4 table_live_kib=128 passes=true engine_due=1800 engine_on_time=1799 engine_late=0 engine_dropped=1 engine_dropped_agent=0 engine_held_max_ms=62.7 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=21.8 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=248 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=747.6 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=reel_9x16 run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1799 late=0 early=0 dropped=1 present_p50_ms=42.0 present_p95_ms=43.2 present_max_ms=43.7 held_max_ms=55.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=47.4 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1449.8 ledger_peak_mib=68.4 table_live_kib=128 passes=true engine_due=1800 engine_on_time=1799 engine_late=0 engine_dropped=1 engine_dropped_agent=0 engine_held_max_ms=63.7 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.3 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=253 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=688.0 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=feed_4x5 run=0 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1799 late=0 early=0 dropped=1 present_p50_ms=41.8 present_p95_ms=43.3 present_max_ms=45.8 held_max_ms=45.3 receipt_offset_max_ms=0.0 clock_stall_max_ms=46.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1484.3 ledger_peak_mib=99.0 table_live_kib=128 passes=true engine_due=1800 engine_on_time=1799 engine_late=0 engine_dropped=1 engine_dropped_agent=0 engine_held_max_ms=63.2 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=23.6 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=466 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=745.8 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=feed_4x5 run=1 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1799 late=0 early=0 dropped=1 present_p50_ms=41.8 present_p95_ms=43.2 present_max_ms=44.0 held_max_ms=45.3 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.6 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1569.0 ledger_peak_mib=99.0 table_live_kib=128 passes=true engine_due=1800 engine_on_time=1799 engine_late=0 engine_dropped=1 engine_dropped_agent=0 engine_held_max_ms=63.4 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.6 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=464 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=689.9 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=feed_4x5 run=2 valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1799 late=0 early=0 dropped=1 present_p50_ms=41.8 present_p95_ms=43.2 present_max_ms=43.8 held_max_ms=45.9 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.9 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1448.4 ledger_peak_mib=99.0 table_live_kib=128 passes=true engine_due=1800 engine_on_time=1799 engine_late=0 engine_dropped=1 engine_dropped_agent=0 engine_held_max_ms=63.0 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=465 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=736.2 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=talk_recut run=0 valid=true elapsed_s=240.01 missed_callbacks=0 due=7200 on_time=7199 late=0 early=0 dropped=1 present_p50_ms=42.0 present_p95_ms=43.3 present_max_ms=44.1 held_max_ms=60.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.9 underrun_frames=0 post_end_underrun_frames=0 peak_rss_mib=1166.7 ledger_peak_mib=42.2 table_live_kib=128 passes=true engine_due=7200 engine_on_time=7199 engine_late=0 engine_dropped=1 engine_dropped_agent=0 engine_held_max_ms=44.1 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=23.6 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=0 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=794.4 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=talk_recut run=1 valid=true elapsed_s=240.03 missed_callbacks=0 due=7200 on_time=7199 late=0 early=0 dropped=1 present_p50_ms=42.0 present_p95_ms=43.2 present_max_ms=43.9 held_max_ms=60.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=60.0 underrun_frames=0 post_end_underrun_frames=0 peak_rss_mib=1185.8 ledger_peak_mib=42.2 table_live_kib=128 passes=true engine_due=7200 engine_on_time=7199 engine_late=0 engine_dropped=1 engine_dropped_agent=0 engine_held_max_ms=43.9 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=42.9 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=0 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=759.8 threads=7
PF1 play lane=LH adapter=NVIDIA GeForce RTX 3090 output=simulated workload=talk_recut run=2 valid=true elapsed_s=240.01 missed_callbacks=0 due=7200 on_time=7199 late=0 early=0 dropped=1 present_p50_ms=42.1 present_p95_ms=43.2 present_max_ms=44.2 held_max_ms=60.4 receipt_offset_max_ms=0.0 clock_stall_max_ms=45.6 underrun_frames=0 post_end_underrun_frames=0 peak_rss_mib=1178.8 ledger_peak_mib=42.2 table_live_kib=128 passes=true engine_due=7200 engine_on_time=7199 engine_late=0 engine_dropped=1 engine_dropped_agent=0 engine_held_max_ms=44.2 engine_av_offset_max_ms=0.0 engine_clock_stall_max_ms=22.0 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=0 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=0 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=796.5 threads=7
PF1 control=Slowdown lane=LH workload=typical_1080p metric=G1 fails=true valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=547 late=50 early=0 dropped=1203 present_p50_ms=71.3 present_p95_ms=177.5 present_max_ms=637.5 held_max_ms=635.3 receipt_offset_max_ms=0.0 clock_stall_max_ms=46.1 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1341.8 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=549 engine_late=48 engine_dropped=1203 engine_dropped_agent=0 engine_held_max_ms=637.5 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=22.6 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=776 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=807.9 threads=7
PF1 control=Freeze lane=LH workload=typical_1080p metric=G14 fails=true valid=true elapsed_s=60.02 missed_callbacks=0 due=1800 on_time=1612 late=4 early=0 dropped=184 present_p50_ms=38.2 present_p95_ms=56.6 present_max_ms=1301.2 held_max_ms=1298.9 receipt_offset_max_ms=0.0 clock_stall_max_ms=47.5 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1467.2 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1612 engine_late=4 engine_dropped=184 engine_dropped_agent=0 engine_held_max_ms=1301.2 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=22.6 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=1003 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=834.1 threads=7
PF1 control=ClockFreeze lane=LH workload=typical_1080p metric=G16 fails=true valid=true elapsed_s=61.02 missed_callbacks=0 due=1800 on_time=1669 late=9 early=0 dropped=122 present_p50_ms=38.3 present_p95_ms=44.8 present_max_ms=1039.2 held_max_ms=1037.2 receipt_offset_max_ms=0.0 clock_stall_max_ms=1050.0 underrun_frames=0 post_end_underrun_frames=512 peak_rss_mib=1537.2 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1669 engine_late=9 engine_dropped=122 engine_dropped_agent=0 engine_held_max_ms=1039.2 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=1027.5 engine_underrun_events=0 engine_underrun_frames=0 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=1086 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=812.4 threads=7
PF1 control=Stall lane=LH workload=typical_1080p metric=underrun_frames fails=true valid=true elapsed_s=61.02 missed_callbacks=0 due=1800 on_time=1689 late=3 early=0 dropped=108 present_p50_ms=38.4 present_p95_ms=43.6 present_max_ms=1045.6 held_max_ms=1044.1 receipt_offset_max_ms=0.0 clock_stall_max_ms=1045.0 underrun_frames=48128 post_end_underrun_frames=512 peak_rss_mib=1331.5 ledger_peak_mib=56.3 table_live_kib=128 passes=false engine_due=1800 engine_on_time=1689 engine_late=3 engine_dropped=108 engine_dropped_agent=0 engine_held_max_ms=1045.6 engine_av_offset_max_ms=33.3 engine_clock_stall_max_ms=1024.0 engine_underrun_events=47 engine_underrun_frames=48128 engine_post_end_underrun_frames=512 engine_sync_decoders=0 engine_table_live_kib=128 engine_stale_errors=0 engine_acks_overflowed=0 engine_acks_unmatched=0 engine_sync_fallback_frames=0 engine_lookahead_starved=1089 consumer_rejected=0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=850.6 threads=7
PF1 seek lane=LL workload=seek_gop60 run=0 random_p95_ms=59.6 random_max_ms=70.5 forward_p95_ms=62.8 plus1_p95_ms=18.4 plus1_n=17 backward_combined_p95_ms=62.2 drag_p95_ms=400.4 drag_answered_p95_ms=300.4 drag_unanswered=6 drag_distinct_fps=13.6 release_pending_drag_calls=6 release_shown=true release_ms=37.9 stale_frames_after_release=0 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=seek_gop60 run=1 random_p95_ms=66.5 random_max_ms=134.2 forward_p95_ms=62.1 plus1_p95_ms=15.5 plus1_n=11 backward_combined_p95_ms=61.5 drag_p95_ms=337.1 drag_answered_p95_ms=337.1 drag_unanswered=0 drag_distinct_fps=15.6 release_pending_drag_calls=1 release_shown=true release_ms=14.9 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=seek_gop60 run=2 random_p95_ms=61.8 random_max_ms=69.4 forward_p95_ms=61.0 plus1_p95_ms=17.1 plus1_n=8 backward_combined_p95_ms=61.1 drag_p95_ms=346.8 drag_answered_p95_ms=346.8 drag_unanswered=0 drag_distinct_fps=13.4 release_pending_drag_calls=1 release_shown=true release_ms=17.1 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=talk_recut run=0 random_p95_ms=93.0 random_max_ms=161.7 forward_p95_ms=103.3 plus1_p95_ms=17.8 plus1_n=17 backward_combined_p95_ms=92.5 drag_p95_ms=865.8 drag_answered_p95_ms=836.0 drag_unanswered=1 drag_distinct_fps=7.2 release_pending_drag_calls=1 release_shown=true release_ms=20.9 stale_frames_after_release=0 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=talk_recut run=1 random_p95_ms=91.9 random_max_ms=155.6 forward_p95_ms=87.3 plus1_p95_ms=24.5 plus1_n=12 backward_combined_p95_ms=88.5 drag_p95_ms=1375.7 drag_answered_p95_ms=1142.4 drag_unanswered=7 drag_distinct_fps=6.4 release_pending_drag_calls=7 release_shown=true release_ms=101.3 stale_frames_after_release=0 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=talk_recut run=2 random_p95_ms=110.4 random_max_ms=181.8 forward_p95_ms=83.8 plus1_p95_ms=16.0 plus1_n=8 backward_combined_p95_ms=86.0 drag_p95_ms=1708.3 drag_answered_p95_ms=1675.0 drag_unanswered=1 drag_distinct_fps=6.4 release_pending_drag_calls=1 release_shown=true release_ms=48.2 stale_frames_after_release=0 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=explainer_16x9 run=0 random_p95_ms=148.6 random_max_ms=193.0 forward_p95_ms=139.4 plus1_p95_ms=26.4 plus1_n=17 backward_combined_p95_ms=154.8 drag_p95_ms=2867.6 drag_answered_p95_ms=2667.6 drag_unanswered=6 drag_distinct_fps=3.6 release_pending_drag_calls=6 release_shown=true release_ms=47.8 stale_frames_after_release=0 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=explainer_16x9 run=1 random_p95_ms=159.7 random_max_ms=183.8 forward_p95_ms=138.1 plus1_p95_ms=25.7 plus1_n=11 backward_combined_p95_ms=155.3 drag_p95_ms=inf drag_answered_p95_ms=1544.0 drag_unanswered=35 drag_distinct_fps=3.0 release_pending_drag_calls=35 release_shown=true release_ms=168.6 stale_frames_after_release=0 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LL workload=explainer_16x9 run=2 random_p95_ms=147.9 random_max_ms=181.8 forward_p95_ms=136.0 plus1_p95_ms=22.1 plus1_n=8 backward_combined_p95_ms=138.2 drag_p95_ms=1314.5 drag_answered_p95_ms=1272.3 drag_unanswered=3 drag_distinct_fps=4.0 release_pending_drag_calls=3 release_shown=true release_ms=53.0 stale_frames_after_release=0 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=seek_gop60 run=0 random_p95_ms=57.5 random_max_ms=69.5 forward_p95_ms=61.8 plus1_p95_ms=14.7 plus1_n=17 backward_combined_p95_ms=61.2 drag_p95_ms=407.1 drag_answered_p95_ms=307.0 drag_unanswered=6 drag_distinct_fps=14.4 release_pending_drag_calls=6 release_shown=true release_ms=50.1 stale_frames_after_release=0 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=seek_gop60 run=1 random_p95_ms=61.4 random_max_ms=71.1 forward_p95_ms=58.5 plus1_p95_ms=16.1 plus1_n=11 backward_combined_p95_ms=60.6 drag_p95_ms=470.6 drag_answered_p95_ms=470.6 drag_unanswered=0 drag_distinct_fps=13.8 release_pending_drag_calls=1 release_shown=true release_ms=17.7 stale_frames_after_release=1 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=seek_gop60 run=2 random_p95_ms=59.4 random_max_ms=67.0 forward_p95_ms=59.4 plus1_p95_ms=16.6 plus1_n=8 backward_combined_p95_ms=58.9 drag_p95_ms=329.4 drag_answered_p95_ms=311.1 drag_unanswered=1 drag_distinct_fps=14.2 release_pending_drag_calls=1 release_shown=true release_ms=19.7 stale_frames_after_release=0 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=talk_recut run=0 random_p95_ms=90.4 random_max_ms=154.9 forward_p95_ms=105.4 plus1_p95_ms=22.2 plus1_n=17 backward_combined_p95_ms=93.9 drag_p95_ms=736.9 drag_answered_p95_ms=732.6 drag_unanswered=1 drag_distinct_fps=8.4 release_pending_drag_calls=1 release_shown=true release_ms=19.7 stale_frames_after_release=0 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=talk_recut run=1 random_p95_ms=90.0 random_max_ms=150.0 forward_p95_ms=83.1 plus1_p95_ms=17.2 plus1_n=12 backward_combined_p95_ms=89.9 drag_p95_ms=1366.0 drag_answered_p95_ms=1132.6 drag_unanswered=7 drag_distinct_fps=6.2 release_pending_drag_calls=7 release_shown=true release_ms=87.0 stale_frames_after_release=0 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=talk_recut run=2 random_p95_ms=104.7 random_max_ms=172.7 forward_p95_ms=81.0 plus1_p95_ms=15.1 plus1_n=8 backward_combined_p95_ms=88.8 drag_p95_ms=inf drag_answered_p95_ms=1506.0 drag_unanswered=37 drag_distinct_fps=6.4 release_pending_drag_calls=37 release_shown=true release_ms=47.5 stale_frames_after_release=0 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=explainer_16x9 run=0 random_p95_ms=145.7 random_max_ms=183.9 forward_p95_ms=136.1 plus1_p95_ms=22.5 plus1_n=17 backward_combined_p95_ms=151.0 drag_p95_ms=2762.5 drag_answered_p95_ms=2595.9 drag_unanswered=5 drag_distinct_fps=3.4 release_pending_drag_calls=5 release_shown=true release_ms=54.7 stale_frames_after_release=0 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=explainer_16x9 run=1 random_p95_ms=159.1 random_max_ms=182.2 forward_p95_ms=133.8 plus1_p95_ms=21.8 plus1_n=11 backward_combined_p95_ms=153.3 drag_p95_ms=inf drag_answered_p95_ms=1551.5 drag_unanswered=43 drag_distinct_fps=3.4 release_pending_drag_calls=43 release_shown=true release_ms=112.0 stale_frames_after_release=0 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 seek lane=LH workload=explainer_16x9 run=2 random_p95_ms=145.3 random_max_ms=178.3 forward_p95_ms=141.7 plus1_p95_ms=17.6 plus1_n=8 backward_combined_p95_ms=136.4 drag_p95_ms=1306.9 drag_answered_p95_ms=1240.4 drag_unanswered=3 drag_distinct_fps=3.6 release_pending_drag_calls=3 release_shown=true release_ms=54.1 stale_frames_after_release=0 frames_over_release=0 valid_frames_over_release=0 timeouts=0
PF1 rss before=33.9/2 constructed=158.4/48 first_render=503.1/131 settled_idle=473.1/49 playing=750.1/132 playing_peak_mib=757.9 played_s=10.0 lane=LL workload=typical_1080p
PF1 rss before=33.8/2 constructed=158.7/48 first_render=494.9/160 settled_idle=457.2/49 playing=767.2/161 playing_peak_mib=767.7 played_s=10.0 lane=LL workload=blend_heavy_1080p
PF1 rss before=34.0/2 constructed=158.7/48 first_render=472.3/131 settled_idle=443.1/49 playing=788.9/132 playing_peak_mib=792.8 played_s=10.0 lane=LL workload=explainer_16x9
PF1 rss before=33.9/2 constructed=158.6/48 first_render=401.7/96 settled_idle=393.8/49 playing=839.0/74 playing_peak_mib=886.2 played_s=10.0 lane=LL workload=reel_9x16
PF1 rss before=34.0/2 constructed=158.5/48 first_render=406.4/96 settled_idle=395.8/49 playing=970.3/74 playing_peak_mib=990.4 played_s=10.0 lane=LL workload=feed_4x5
PF1 rss before=34.0/2 constructed=158.5/48 first_render=380.7/96 settled_idle=372.9/49 playing=748.5/132 playing_peak_mib=748.5 played_s=10.0 lane=LL workload=talk_recut
PF1 rss before=34.0/2 constructed=158.6/48 first_render=237.4/49 settled_idle=237.4/49 playing=256.0/50 playing_peak_mib=256.0 played_s=10.0 lane=LL workload=title_only
PF1 rss before=33.7/2 constructed=162.6/11 first_render=551.5/94 settled_idle=443.6/12 playing=815.2/95 playing_peak_mib=821.9 played_s=10.0 lane=LH workload=typical_1080p
PF1 rss before=33.8/2 constructed=162.6/11 first_render=535.7/123 settled_idle=436.6/12 playing=810.7/124 playing_peak_mib=811.9 played_s=10.0
PF1 export lane=LH workload=typical_1080p frames=120 run=0 wall_ms=198576.2 bytes=3375562
PF1 export lane=LH workload=typical_1080p frames=120 run=1 wall_ms=198262.2 bytes=3375562
PF1 export lane=LH workload=typical_1080p frames=120 run=2 wall_ms=198715.2 bytes=3375562
PF1 export lane=LH workload=typical_1080p frames=120 runs=3 median_ms=198576.2 min_ms=198262.2 max_ms=198715.2
PF1 export lane=LH workload=typical_1080p frames=120 run=0 wall_ms=35916.1 bytes=3375562
PF1 export lane=LH workload=typical_1080p frames=120 run=1 wall_ms=36299.4 bytes=3375562
PF1 export lane=LH workload=typical_1080p frames=120 run=2 wall_ms=35993.9 bytes=3375562
PF1 export lane=LH workload=typical_1080p frames=120 runs=3 median_ms=35993.9 min_ms=35916.1 max_ms=36299.4
PF1 rss before=34.2/2 constructed=162.7/11 first_render=551.2/94 settled_idle=486.7/12 playing=807.6/95 playing_peak_mib=807.8 played_s=10.0 lane=LH workload=typical_1080p
PF1 rss before=33.9/2 constructed=162.5/11 first_render=535.4/123 settled_idle=434.1/12 playing=814.9/124 playing_peak_mib=816.9 played_s=10.0
```

### E12.12 Addendum: D3(a) landed, P-rss LH completes

`e6a6e27` lands E12.10 D3(a). The P-rss child ends with the same `teardown` as P-play:
- it drops the session;
- it waits (≤ 30 s) for the engine's worker to finish its whole teardown;
- it drops the GPU context;
- it appends the teardown record to its `PF1 rss` line.

D3(b), the engine joining its worker on drop, is deferred to S3b.

The P-rss LH lane was rerun once on `e6a6e27`: release test binary sha256 `3f485a11…e9d1f7`, 10:52:31–10:56:38 EDT,
alone, with the screensaver's two `foot` processes at 91% and 52% (R26). **All seven workloads complete, with exit 0.**
The rerun hit no SIGSEGV, and every child reports `teardown_complete=true`. LH is recorded, not gated (G15 is LL's).

| Workload (LH) | Constructed | First render | Settled idle | Playing | Playing peak MiB | Teardown RSS MiB / threads |
|---|---|---|---|---|---|---|
| `typical_1080p` | 162.3/11 | 551.6/94 | 451.8/12 | 834.8/95 | 839.8 | 297.6/2 |
| `blend_heavy_1080p` | 162.9/11 | 536.0/123 | 458.6/12 | 824.6/124 | 825.2 | 279.5/2 |
| `explainer_16x9` | 163.0/11 | 521.1/94 | 447.5/12 | 806.6/95 | 868.2 | 267.7/2 |
| `reel_9x16` | 162.6/11 | 456.0/59 | 433.9/12 | 1016.1/37 | 1031.0 | 302.4/2 |
| `feed_4x5` | 162.8/11 | 443.8/59 | 433.1/12 | 1063.5/37 | 1099.5 | 340.3/2 |
| `talk_recut` | 162.6/11 | 449.8/59 | 428.3/12 | 782.1/95 | 782.1 | 292.8/2 |
| `title_only` | 162.4/11 | 306.7/12 | 306.7/12 | 329.0/13 | 329.0 | 133.6/2 |

- **Every child's teardown record:**
  - 0 ledger KiB live and 0 decoders;
  - 128 KiB of conversion tables (0 for `title_only`);
  - 2 threads.
- **Settled idle** for `typical_1080p` (451.8/12) and `blend_heavy_1080p` (458.6/12) is in line with E12.8's partial
  LH readings (443.6–486.7 and 434.1–436.6). Against S0's LH readings:
  - `typical_1080p`: 451.8/12, against 624.9/103;
  - `blend_heavy_1080p`: 458.6/12, against 711.0/149.

Raw lines:

```
PF1 rss before=33.8/2 constructed=162.3/11 first_render=551.6/94 settled_idle=451.8/12 playing=834.8/95 playing_peak_mib=839.8 played_s=10.0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=297.6 threads=2 lane=LH workload=typical_1080p
PF1 rss before=34.1/2 constructed=162.9/11 first_render=536.0/123 settled_idle=458.6/12 playing=824.6/124 playing_peak_mib=825.2 played_s=10.0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=279.5 threads=2 lane=LH workload=blend_heavy_1080p
PF1 rss before=34.2/2 constructed=163.0/11 first_render=521.1/94 settled_idle=447.5/12 playing=806.6/95 playing_peak_mib=868.2 played_s=10.0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=267.7 threads=2 lane=LH workload=explainer_16x9
PF1 rss before=34.0/2 constructed=162.6/11 first_render=456.0/59 settled_idle=433.9/12 playing=1016.1/37 playing_peak_mib=1031.0 played_s=10.0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=302.4 threads=2 lane=LH workload=reel_9x16
PF1 rss before=34.2/2 constructed=162.8/11 first_render=443.8/59 settled_idle=433.1/12 playing=1063.5/37 playing_peak_mib=1099.5 played_s=10.0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=340.3 threads=2 lane=LH workload=feed_4x5
PF1 rss before=33.9/2 constructed=162.6/11 first_render=449.8/59 settled_idle=428.3/12 playing=782.1/95 playing_peak_mib=782.1 played_s=10.0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=128 teardown_rss_mib=292.8 threads=2 lane=LH workload=talk_recut
PF1 rss before=33.9/2 constructed=162.4/11 first_render=306.7/12 settled_idle=306.7/12 playing=329.0/13 playing_peak_mib=329.0 played_s=10.0 teardown_complete=true teardown_ledger_live_kib=0 teardown_decoders=0 teardown_table_live_kib=0 teardown_rss_mib=133.6 threads=2 lane=LH workload=title_only
```

### E12.13 R37 fixes and reruns

Ruling P34 (R37): the G1/G14 amendment (iv), the start-up fix, and every finding of reviews A and B, one commit per
logical fix on `pf1/impl` from `10a2d18`. **The timing half of this section is incomplete.** The rerun's lanes were
contaminated, and their runner died partway through (E12.13.5). The stage-gate verdicts wait for a quiet-window rerun.

#### E12.13.1 Closure table

Each mutation was applied alone to the committed code, and its named witnesses were run
(`s2b-logs/r37-mutations.log.gz`; the script is `mutate_r37.py.gz`). **All 20 were killed.**

| Finding | Commit | Witness | Mutation caught |
|---|---|---|---|
| G1/G14 amendment (iv), review B F3: regions merge only across decoder continuation (`REGION_GAP` 16 → 1; a required time after lookahead starts a region) | `2e5c7f8` R37-1 | `sched::tests::two_playheads_fourteen_apart_read_forward_on_two_readers` (two readers, forward only, one seek each; region shapes) | R37-1a `REGION_GAP` back to 16; R37-1b a required time after lookahead continues the region |
| Start-up (amendment): a play supersedes earlier paused jobs; playback's first target is its start frame | `545a8f5` R37-2 (text: `bbc13a1`) | `preview::tests::a_play_supersedes_the_paused_job_issued_before_it`, `…::a_play_withdraws_a_waiting_paused_demand`, `…::playback_first_targets_its_start_frame`, `engine::tests::play_supersedes_earlier_paused_jobs_as_it_is_issued` | R37-2a superseded job kept; R37-2b the waiting demand ignores a play; R37-2c no withdrawal (60 s); R37-2d first target clock + lead; R37-2e `play` not issued on the lane |
| A F2: a needed shrink waits behind lookahead | `7bfa588` R37-3 | `sched::tests::a_needed_shrink_comes_before_more_lookahead`; the H-8 model's decode-step assertion (R37-7) | R37-3 (killed by both) |
| A F3: no witness for M58 | `58176aa` R37-4 | `preview::tests::a_grant_wakes_the_next_ticket` (a test-only wake hook forces the interleaving on the real condvar) | M58, masked at S2b-2, now killed (10 s timeout) |
| A S1: a retired reader's late delivery kept | `f71e275` R37-5 | `sched::tests::a_reader_retired_mid_decode_stays_retiring` (extended: the frame comes back, the rings stay empty) | R37-5 |
| A F1 and A S2 (clears): a cache clear confuses active and completed demand | `1c9ed58` R37-6 | `preview::tests::a_cleared_idle_reader_still_retires`, `…::a_clear_during_a_paused_wait_keeps_its_frames`; the seeded stress now clears | R37-6a an active clear drops the wait's frames; R37-6b an idle clear keeps the completed plan |
| A S2 (model scope) and A's nit (depths) | `6839644` R37-7; `712d108` (E12.3 text) | the model asserts A F2 | the R37-3 mutation fails the model |
| B F4: eviction ignores K-5's travel direction | `631ef61` R37-8 | `sched::tests::eviction_follows_the_travel_direction` | R37-8 |
| B F2: an adjustment allocates a frame per render | `4e1679e` R37-9 | `render::k1_allocation::a_render_allocates_exactly_its_generated_reservation` | R37-9 |
| B S2: resident rasters reserved again | `2dc3341` R37-10 | `preview::tests::a_resident_title_is_not_reserved_again` | R37-10 |
| B F1: f charged from metadata | `6ee4672` R37-11 | `decode::tests::frame_size_is_what_the_decoder_converts`, `preview::tests::reservations_measure_frames_not_metadata`, `…::a_frame_larger_than_its_reservation_fails_and_cleans_up` | R37-11a f from metadata; R37-11b a frame of another size accepted; R37-11c orientation ignored |
| B S3: C-5's witness shows too little | `61cb973` R37-12 | `preview::tests::scheduled_multilayer_frames_match_the_synchronous_renderer` (seven visible layers; visibility proven by removal) | R37-12a scene LUT library ignored; R37-12b every source opened as limited range. The old C-5 witness passes both. |
| B S1: the export lane compares lengths | `2aa4e1b` R37-13 | the G18 lane asserts one SHA-256 per build and prints it | none (a lane change; it is exercised by the G18 rerun) |
| B S4 and B's nit | `712d108` | E12.10 D1 qualifications; E12.5 "no run satisfies both conditions" | — |

#### E12.13.2 Tests and line counts

**Full `cargo test --workspace` at `712d108`:** every target passed (36 `test result: ok` lines), with 3,475 tests passed,
0 failed and 42 ignored, in 389 s. Media alone: 976 passed and 21 ignored, in 224 s. The log is
`s2b-logs/r37-workspace.log.gz`. Every R37 commit also passed the per-commit gate: build, media clippy with
`-D warnings`, rustfmt on the touched files, and `cargo test -p kinewright-media`.

Non-blank, non-comment `.rs` lines, net. They are split at every `#[cfg(test)] mod … {`, and `cfg(test)` hooks outside
test modules count as production.

| Commit | Production | Tests |
|---|---|---|
| R37-1 `2e5c7f8` | 3 | 64 |
| R37-2 `545a8f5` | 51 | 123 |
| R37-3 `7bfa588` | 3 | 66 |
| R37-4 `58176aa` | 22 | 38 |
| R37-5 `f71e275` | 1 | 8 |
| R37-6 `1c9ed58` | 35 | 64 |
| R37-7 `6839644` | 0 | 1 |
| R37-8 `631ef61` | 24 | 30 |
| R37-9 `4e1679e` | 1 | 58 |
| R37-10 `2dc3341` | 44 | 30 |
| R37-11 `6ee4672` | 23 | 112 |
| R37-12 `61cb973` | 0 | 301 |
| R37-13 `2aa4e1b` | 12 | 0 |
| **R37** | **219** | **895** |

- **Per file** (production + tests):
  - `sched.rs` 62 + 169;
  - `preview.rs` 96 + 620;
  - `render.rs` 40 + 60;
  - `decode.rs` 3 + 28;
  - `engine.rs` 3 + 18;
  - `cache.rs` 3;
  - `pf1_export_lane.rs` 12.
- **Docs:** +32 lines (`bbc13a1` and `712d108`).

#### E12.13.3 Deviations within R37 (recorded, none changes a design number)

- **Start-up.** A FrameWait a play abandons withdraws its demand and also stops the readers' decodes at the next packet
  boundary, so no reader keeps decoding the stale frame. Amendment R37's text (`bbc13a1`) states this.
- **A S1** is closed by returning a retired reader's delivery to the reader, which releases it, instead of keeping it in
  the ring.
- **B S2.** The preview binds a new generation before it schedules, so titles cleared by the rebind count as missing.
- **A F1.** The seeded stress makes every other agent job a cache clear. Its density is unchanged.
- **A F2.** The H-8 model gained an assertion: only a required decode defers a needed shrink.
- **B F1** measures f with a one-thread open of the reader's own decoder, once per source per preview.
  - Why: IN1 allows a single wrap site, `contextual_managed_decode_error`, so the measurement reuses `SourceSpec::open`.
  - The cost: the first render of each source pays one extra open.
  - What stays: the synchronous decode path (`decode_video_frame`) still sizes its cache from asset metadata, as it did
    before S2b.
- **The timing lanes run the release test binary** (`cargo test --release … --no-run`), not a production build, so
  the `cfg(test)` hooks are compiled into what is timed. Their blocking hooks stay unset in every timing lane, but the
  checks around them still execute (R38, review B's overhead nit):
  - per decode, a `gate` and a `hold_at` mutex check (`hold_at` is R37-6's per-time gate);
  - per permit wake, a `woke` mutex check (R37-4);
  - per permit poll, a `PermitBook::polls` map update;
  - per permit wait, grant and reopen, an extra `ready` notification;
  - per cancelled decode, a counter increment and a notification (R37-2).

  All are uncontended. No timing impact is inferred from them either way: none was measured. Every earlier PF1 lane
  carried hooks of the same kind. Production builds, release or debug, contain none of them.

#### E12.13.4 Provenance note on E9–E12: the screensaver

Every PF1 timing lane before 2026-09-28 13:00 ran with Riel's two `foot` terminal-screensaver processes busy. They
used ≈1.4–1.6 cores (≈91% + 52% at every E11–E12 mark) and kept Hyprland drawing continuously. This covers:

- the S0 baselines (E9);
- S1 (E10);
- S2a (E11);
- S2b (E12.1–E12.12);
- probe R36.

Between E12.12 (10:52–10:56, present) and the R37 rerun (13:12, absent) the screensaver ended. Its processes were gone
at every R37 mark.

The earlier records called this CPU load (R26). The R37 rerun suggests a second effect: with a continuously
redrawing desktop, the RTX 3090 probably stayed at high clocks between frames (E12.13.5). **The LH figures in E9–E12
are therefore "screensaver running" figures. They are not idle-desktop figures, and they are not pinned figures.**
- LL (llvmpipe) figures carried the CPU load instead.
- No earlier verdict changes, but no LH comparison across that boundary is like for like.
- S4's pinned run must fix and record the GPU's state (pstate and clocks), not only the CPU's.

#### E12.13.5 Incident: the R37 timing rerun (partial, contaminated)

**Binary:** the release test binary at `712d108` (sha256 `c4de5a15…c1945`). G18's S0 side was rebuilt from `d19bdf9`
plus the hashing lane (sha256 `a4b501aa…82e03`). Machine and software as in E12.1.

**What happened.**
- One runner started the lanes at 13:12:54 EDT.
- **Completed:**
  - I4 LL and I4 LH (exit 0);
  - G3 LH (exit 101, R28 gate 10);
  - P-play LL with controls (exit 0, 13:18:28–13:52:20).
- **Cut short:** P-play LH ran `typical_1080p`, `blend_heavy_1080p`, `explainer_16x9`, `reel_9x16` and `feed_4x5`
  (3 runs each) and `talk_recut` run 0.
- **Why it stopped:** at about 14:13 the runner died. It was killed with the agent shell that launched it. It wrote no
  exit marker, and there is no coredump.
- **Never run:** P-play LH's controls, P-seek, P-rss and G18.
- **The runner has since been restructured:**
  - it is detached with `setsid nohup` and polled;
  - each lane has a timeout of at most 10 min;
  - each BEGIN/END records `uptime`, the top-5 CPU processes and the GPU's pstate and clocks;
  - a 5 s sampler flags any process other than the test and T3 Code above 20% CPU.

**Contamination.**
- The screensaver was gone (E12.13.4).
- Riel's other interactive work ran during the lanes: other agent sessions, brakeman, Brave and Spotify.
- Short-lived `gh` processes appeared at marks, at 11–83% CPU.
- 1-minute load averages were 2.97–6.53.
- The runner recorded only lifetime-average `ps` figures, so the lanes' true ambient load is unknown.

**These numbers are not gate evidence.**

**I4** (S0's 5% rule). It passes, but LH moved against S2b:

| Build | LL mean ms | LL delta | LH mean ms | LH delta |
|---|---|---|---|---|
| S2b `cce86e1` (E12.4) | 76.98 / 75.54 / 77.16 | −84.6% | 73.58 / 73.46 / 73.33 | −85.1% |
| R37 `712d108`, contaminated | 69.09 / 68.27 / 68.67 | −86.2% | 80.48 / 80.81 / 80.31 | −83.7% |

**G3** (60 fps floor, LH). It **failed** in this rerun:

| Build | `typical_1080p` fps | `blend_heavy_1080p` fps (p95 ms) | `heavy_4k` fps | Slowdown control |
|---|---|---|---|---|
| S2b `cce86e1` (E12.4) | 72.2–74.8 | 70.7 / 71.2 / 71.7 (15.46–16.31) | 58.2–59.0 | 17.7 fps (mean 56.45 ms) |
| R37 `712d108`, 13:15–13:18 | 40.9 / 47.6 / 48.4 | 50.1 / 48.4 / 46.4 (31.1–31.8) | 29.4 / 29.1 / 29.4 | 14.1 fps (mean 71.03 ms) |
| R37 `712d108`, the lead's rerun, 15:29 (idle start; other load rose during it) | — | 57–68 | — | — |

The ME14 phases, printed by the same lane from the same binary, did **not** slow. Each cell is upload / GPU passes and
readback / monitor encode, in ms:

| Workload | S2b | R37 |
|---|---|---|
| `typical_1080p` | 10.18 / 1.34 / 1.75 | 10.08 / 1.11 / 1.57 |
| `blend_heavy_1080p` | 10.08 / 1.46 / 1.66 | 10.31 / 1.27 / 1.63 |
| `heavy_4k` | 13.27 / 1.56 / 1.64 | 13.54 / 1.35 / 1.63 |

**P-play LL** (complete, 3 runs, every run valid). R37-1's effect shows:

| Workload | On time / late / dropped | Present p50 / p95 / max ms | Held max ms | `lookahead_starved` |
|---|---|---|---|---|
| `typical_1080p` | 1796–1798 / 0 / 2–4 (S2b: 1021–1078) | 42.1–42.2 / 43.3–43.4 / 47.4–90.1 | 85.1–86.5 (S2b: 279.8–356.1) | 2342–2369 |
| `blend_heavy_1080p` | 1798 / 0 / 2 | 41.8 / 43.2–43.3 / 43.6–43.7 | 85.1–85.3 | 3632–3679 |
| `explainer_16x9` | 1798–1799 / 0–1 / 0–2 | 42.1 / 43.2–43.3 / 43.7 | 43.5–85.1 | 887–961 |
| `reel_9x16` | 1796–1798 / 0 / 2–4 | 42.0 / 43.3 / 43.7–85.5 | 85.1 | 196–203 |
| `feed_4x5` | 1767–1785 / 0–1 / 15–32 | 36.4–38.4 / 43.2–43.3 / 85.7–119.9 | 83.3–119.5 | 319–371 |
| `talk_recut` | 7178–7199 / 0–1 / 0–22 of 7200 | 42.1 / 43.2 / 43.8–575.9 | 43.3–573.0 | 0 |

- In every LL run: 0 underrun frames before the end, `sync_fallback_frames` 0 and `engine_sync_decoders` 0.
- The four LL controls fail their gates, as they must:
  - Slowdown: 729 on time;
  - Freeze: held 1962.5 ms;
  - ClockFreeze: stall 1050.1 ms;
  - Stall: 48128 underrun frames.
- `talk_recut` run 2 had a single 573 ms hold, with 22 drops. It is not localised.

**P-play LH** (partial, 3 runs except `talk_recut`):

| Workload | On time / dropped | Present p50 / p95 / max ms | Held max ms | Valid |
|---|---|---|---|---|
| `typical_1080p` | 1784 / 13; 1702 / 96; 1152 / 646 | 33.8–62.3 / 43.1–84.8 / 77.2–313.7 | 73.8 / 307.5 / 275.5 | run 2 invalid (1 missed callback) |
| `blend_heavy_1080p` | 1608 / 192; 1380 / 420; 1699 / 101 (S2b: 1797 / 3) | 32.5–32.8 / 63.3–64.7 / 86.2 | 84.8–85.4 | yes |
| `explainer_16x9` | 1800 / 0; 1799 / 0; 1796 / 4 | 39.8–42.0 / 43.3 / 43.7–64.2 | 45.5–61.7 (S2b: 110.1–120.1) | yes |
| `reel_9x16` | 1661 / 139; 1673 / 126; 1721 / 78 (S2b: 1799 / 1) | 36.7–37.7 / 43.5–63.9 / 86.0 | 84.9–85.1 | yes |
| `feed_4x5` | 1550 / 250; 1442 / 357; 1481 / 318 (S2b: 1799 / 1) | 36.9–40.2 / 64.3–64.7 / 86.0–103.7 | 84.9–102.4 | yes |
| `talk_recut` (run 0 only) | 7193 / 6 of 7200 | 42.1 / 43.3 / 123.9 | 120.3 | yes |

**Reading (hypothesis, not yet tested):** an idle-clocking GPU, not R37's code. The evidence:

- **Only one R37 change runs on G3's timed path.** That is R37-9's shared adjustment frame, and only `blend_heavy`
  has an adjustment; the compositor never uploads an adjustment's frame. `typical_1080p` and `heavy_4k` run no changed
  code, yet slowed as much.
  - `Cargo.lock`, rustc (1.98.0) and the NVIDIA driver (615.71.09) are unchanged.
- **The per-frame work did not slow:** the ME14 phases above match S2b. What slowed is the lane's loop, where each
  frame's decode leaves the GPU idle between composites.
- **The slowdown control's composite doubled** with identical code: 71.03 − 41.7 ≈ 29.3 ms, against 56.45 − 41.7 ≈
  14.7 ms at S2b. That control sleeps 41.7 ms between frames.
- **I4 moved in opposite directions on the two lanes:** LL was 10% faster (the screensaver's CPU load was gone) and LH
  10% slower. A change in shared code moves both lanes the same way.
- **At idle the GPU sat at P8/P5,** 210–480 MHz against a 2,100 MHz maximum.
- **The same binary read 57–68 fps** on the lead's rerun.
- **In P-play LH, the workloads that regressed are the three with an adjustment layer:** `blend_heavy_1080p`,
  `reel_9x16` and `feed_4x5`. They are the heaviest on the GPU (a snapshot copy and an extra pass). Their p95 present
  (≈64 ms) is one frame period over. Their held maxima (≈85 ms) are two periods, while LL was unaffected.
  - By inspection, no R37 change is LH-specific. The scheduler (R37-1, -3, -8) is shared with LL, which improved.
  - R37-2's lead-less target applies once per play.
- **R37-1 remains the leading code suspect.** Probe R36 measured it only on `typical_1080p` LH.

**Owed, in Riel's quiet window,** with every lane's GPU state and top-5 CPU recorded:

1. **Step 0: an interleaved A/B**, A B A B. A is `10a2d18`, which runs the same code as S2b; B is `712d108`. Two lanes:
   - G3 narrowed to `typical_1080p,blend_heavy_1080p`;
   - P-play LH `blend_heavy_1080p` with 2 runs.
2. **If B is worse than A beyond A's own spread:** a sweep over the ten runtime-changing R37 commits, all prebuilt.
3. **The full R37 stage gates,** on one binary with each lane alone:
   - I4, G3;
   - P-play LL and LH with controls;
   - P-seek and L-6;
   - G18 with hashes;
   - P-rss LL.

### E12.14 R38 fixes and the merge of main

Ruling R38: the Astra re-review of `6ea6437..712d108` (`rereview-s2b.md`) rejected R37 on four new defects (D1–D4),
left review B's F3, S1 and S2 partial, and added two nits. One commit per item on `pf1/impl`, after `a485b57`, then
a merge of `main`. **The timing half is pending: quiet window.** No lane was run for R38. The binary
and the plan are ready (E12.14.3).

#### E12.14.1 Closure table

Each mutation was applied alone to the committed code, and only its named witness was run
(`s2b-logs/r38-mutations.log.gz`). **Every one was killed:** 9 failing witness runs over 8 distinct code mutations
(R38-1, 2, 3a, 3b, 4, 5a, 5b and 7; 5b ran against two witnesses), plus G18's synthetic checks (R38-6), which mutate
no code. (Corrected in R41; this line first said "all 10".) Each row's raw lines are the failing test's summary and
its panic message, verbatim.

| Finding | Commit | Witness | Mutation | Raw result under the mutation |
|---|---|---|---|---|
| D1: a lookahead source whose frame size cannot be measured fails the current frame | `f9cad24` R38-1 | `preview::tests::an_unreadable_lookahead_source_does_not_fail_the_current_frame` | R38-1: `Err(_) if frame != at.0` → `Err(_) if false` (every measurement failure fails the job) | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 997 filtered out; finished in 0.36s`; the panic is `render_ahead`'s "frame 0 did not render" (`preview.rs:2867`) |
| D2: a suspended wait composites with the LUT library of the job that ran during it | `cbec07e` R38-2 | `preview::tests::a_suspended_wait_composites_with_its_own_lut` | R38-2: the rebind after `schedule` removed | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 998 filtered out; finished in 0.73s`; "A composited with A's look" |
| D3: charges follow raster identities; rasters the synchronous path caches are adopted or dropped | `1d093ac` R38-3 | `preview::tests::thumbnail_rasters_are_charged_or_dropped` | R38-3a: no `settle_titles` after a thumbnail; R38-3b: adoption without a charge (`adopt` bypassed) | 3a: `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 999 filtered out; finished in 0.41s`; 3b: `… finished in 0.31s`; both "thumbnail: live is what the preview holds" |
| D4: the shared adjustment frame still owns an uncharged eight-byte buffer (C + 8 during composition) | `c2cb734` R38-4 | `render::k1_allocation::a_render_allocates_exactly_its_generated_reservation` (now unfiltered; asserts the adjustments' capacity is 0) | R38-4: the 1 × 1 frame restored | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 999 filtered out; finished in 0.23s`; "no pixel buffer" |
| B F3: at the global reader limit, `plan_regions` merges independent playheads and a merged reader decodes backwards (`0→14→1`) | `a25fc80` R38-5 (text in Amendment R37: *Reader limit*) | `sched::tests::over_the_reader_limit_a_merged_region_still_reads_forward`; `preview::tests::a_reader_limit_merge_is_counted` | R38-5a: a merged region keeps the earlier region's lookahead; R38-5b: `merged += 0` (run against both witnesses) | 5a: `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1001 filtered out; finished in 0.00s`, "A's regions merged forward only"; 5b (sched): `… finished in 0.00s`, "the last resort is counted"; 5b (preview): `… finished in 4.13s`, "P = 2: merges counted" |
| B S1: G18's identity rests on equal lengths | `572f768` R38-6 (E12.9 qualified) | `g18_verdict.py`, the gate plan's last step (outside the repository, like the runner) | R38-6: synthetic logs with S0 hashing `aaaa…` and the candidate varied | same hash: "identity: **IDENTICAL**"; another hash: "**DIFFERENT**"; candidate lane exit 124: "**INCOMPLETE** (`G18 LH R38` exit 124)"; no candidate: "**INCOMPLETE** (no candidate G18 lane)"; the runner's `@` step printed the verdict into the lane log |
| B S2 and its nit: the resident-title witness compares buffer addresses, which freed buffers can reuse | `cae3385` R38-7 | `preview::tests::a_resident_title_is_not_reserved_again` (counts rasterizations, `[2, 0]`) | R38-7: every generated raster reserved again (`Some(demand.generated)`). This reverts review B S2's earlier reservation fix (`generated_uncharged`), so the witness also detects that revert | `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1001 filtered out; finished in 0.32s`; "two rasters made once, then reused" |
| B nit: the timing binaries' `cfg(test)` overhead is unstated | `32022ee` R38-8 | E12.13.3 lists every check the timing binaries execute | — | — |

- **B F3's stat.** `PlaybackStats::regions_merged` counts each last-resort merge. The preview drains it at Ready with
  `lookahead_starved`, and the P-play lane prints it as `engine_regions_merged`.
- **Still open (documented, not a regression):** `regions()` keeps a per-source cap residual. A source's third
  required region past the cap is folded before `plan_regions` sees it, so it is not counted as a merge. (Closed in
  E12.15: R41-2.)

#### E12.14.2 Merge of main, tests and line counts

- **The merge.** `b980b4e` merges `origin/main` `64e5391` (CI test tiers) without conflicts. `ci/ignored-tests.txt`
  gained the five manual release lanes `pf1/impl` adds, each `any on-demand`:
  - `pf1_export_lane::pf1_export_lane`;
  - `pf1_harness::pf1_play_baseline`;
  - `pf1_harness::pf1_rss_baseline`;
  - `pf1_harness::pf1_rss_child`;
  - `pf1_harness::pf1_seek_baseline`.
- **The local stage gate at `b980b4e`** (every command at `nice -n 19`, `-j 4`, `RUST_TEST_THREADS=4`):
  - `cargo build --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`, each with and without
    the three `slow-tests` features: exit 0;
  - `cargo fmt -- --check`: exit 0;
  - `cargo build -p kinewright-app`: exit 0;
  - `python3 scripts/slow_tests.py lint`: "slow-test manifest and markers agree: 47 tests, features
    kinewright-agent/slow-tests,kinewright-app/slow-tests,kinewright-media/slow-tests; 44 allowlist entries
    well-formed";
  - `cargo test --workspace` (the fast tier): 36 `test result: ok` lines, 3,436 passed, 0 failed, 91 ignored.
    Media: `test result: ok. 951 passed; 0 failed; 56 ignored; 0 measured; 0 filtered out; finished in 166.18s`.
    The log is `s2b-logs/r38-workspace-fast.log.gz`;
  - `slow_tests.py verify-fast` on that log with `--os linux`: "fast tier on linux skipped exactly the 47 manifest
    tests and 44 allowlisted ignores (ci/ignored-tests.txt), each observed, nothing else".
  - The slow tier was not run locally. It runs in CI on the push.
- **Test durations** (debug, 4 threads, the media binary under `--report-time`; `s2b-logs/r38-media-times.log.gz`).
  One PF1 test runs over 60 s: `sched::tests::the_reader_model_holds_for_every_short_sequence` (H-8's model),
  94.5 s. It is a slow-tier candidate and is **not** marked. The next PF1 tests are the seeded stress (24.1 s) and
  `decode::tests::input_tables_match_every_accepted_descriptor` (10.3 s), which needs no attention.
- **Before the merge,** each R38 commit passed the per-commit gate: build, media clippy with `-D warnings`, rustfmt on
  the touched files and `cargo test -p kinewright-media`. The last per-commit media run read
  `test result: ok. 981 passed; 0 failed; 21 ignored; 0 measured; 0 filtered out; finished in 350.07s`.
  - One per-commit media run, at a load average near 85 from unrelated processes, failed the known load flake
    `preview::tests::a_widening_plan_rebalances_the_permits` at its 60 s wait. It passed 3 of 3 alone and in the
    next full run. It passed in the fast-tier run above.

Non-blank, non-comment `.rs` lines, net, counted as in E12.13.2:

| Commit | Production | Tests |
|---|---|---|
| R38-1 `f9cad24` | 11 | 31 |
| R38-2 `cbec07e` | 1 | 94 |
| R38-3 `1d093ac` | 54 | 103 |
| R38-4 `c2cb734` | 0 | 2 |
| R38-5 `a25fc80` | 28 | 51 |
| R38-7 `cae3385` | 0 | 1 |
| **R38** | **94** (+155 −61) | **282** (+293 −11) |

- **Per file** (production + tests):
  - `preview.rs` 49 + 247;
  - `render.rs` 22 + 2;
  - `sched.rs` 21 + 33;
  - `media.rs` 1;
  - `pf1_harness.rs` 1.
- **Docs before this section:** 2 files, +27 −10 (`a25fc80`, `572f768`, `32022ee`).

#### E12.14.3 Timing: pending, quiet window

Nothing was timed for R38. What is ready:

- **The binary:** the release test binary at `b980b4e`, `/tmp/kinewright-r37/bins/bin-b980b4e`, sha256
  `b3266763cc6d329d6f7161d6f150c9671881ee3a09b73d5de334e7dd66b9a895` (in that directory's `SHA256SUMS`).
- **The plan:** `plan-gates.txt` now runs every stage-gate lane on that binary. G18's candidate lane is `G18 LH R38`,
  and the plan ends with `@G18 verdict`, which runs `g18_verdict.py` on the lane log.
- **Unchanged:** the S0 side of G18 (`media-s0-g18`, sha256 `a4b501aa…82e03`) and the step-0 A/B plan (`plan-ab.txt`,
  `10a2d18` against R37's `712d108`).
- Every stage-gate verdict of E12.13.5's owed list therefore still waits for the quiet window, now on this binary.

### E12.15 R41 fixes and the Windows file-lock catch

Ruling R41 (`rereview-r38.md`; orchestrator note P43) covered five items:

- BF3-2: a merged reader rewinds across jobs;
- the H-1 cap residual;
- U-1: unbounded per-source metadata;
- the E12.14 count, corrected in place;
- R41-5, added after Windows CI failed on `573a803`.

There is one commit per fix on `pf1/impl`, after `573a803`, and this section is in the docs commit after them. **The
timing half is pending: quiet window.** No lane was run for R41. The binary and the plan are ready (E12.15.4).

- **Windows catch (E12.15 line).** CI run 36528931046 on `573a803` failed on Windows only, in
  `generated_media::relinked_moved_source_round_trip_renders_identical_frame` (`generated_media.rs:1026`). The
  `std::fs::rename` of a source that had just left the document returned os error 32.
  - Cause: an idle S2b reader keeps its decoder, and so its file handle, open until its 5 s quiescence. Linux allows
    the rename, so no Linux lane could see it.
  - This is the first Windows-only S2b catch. It is logged in `target/review/ci/ledger.md` under `pf1/impl`, class
    "Windows platform (file lock)".
  - Fixed by R41-5 below. The witness is state-based and runs on every OS. Whether the relink test itself passes on
    Windows is for the CI run on the push.

#### E12.15.1 Closure table

Each mutation was applied alone to the committed code, and only its named witness was run
(`s2b-logs/r41-mutations.log.gz`, which also keeps every earlier run). The table has 16 witness runs over 15 distinct
code mutations (corrected in R43, E12.16; it said 14), and **every one was killed**:

- R41-1a–d;
- R41-2a–d;
- R41-3a–d;
- R41-5a–c, with 5a run against both witnesses.

Three mutations survived first (corrected in R43; it said two), and each witness was strengthened before its commit
was amended. See the notes under the table.

| Finding | Commit | Witness | Mutation | Raw result under the mutation |
|---|---|---|---|---|
| BF3-2: a merged reader rewinds across jobs | `74781d9` R41-1 (Amendment R41) | `sched::tests::a_merged_reader_rewinds_at_most_once_per_job` (24 jobs, two advancing playheads on one source, at P = 2 and P = 3); `preview::tests::merged_rewinds_reach_the_engine_stats` | 1a: `merged_rewinds += …*0`; 1b: the merged region keeps the earlier region's lookahead; 1c: `merged = true` removed; 1d: the preview drain `*0` | 1a: `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1008 filtered out; finished in 0.00s`, "every rewind is counted"; 1b: `… finished in 0.00s`, "within a job every reader decodes forward: Rewinds { within: [1, 0, …"; 1c: `… finished in 0.00s`, "only merged readers rewind: Rewinds { …, unmerged: [0, 1, 1, …"; 1d: `… finished in 0.39s`, "P = 2: rewinds counted (8 merges)" |
| The H-1 cap residual: a fold past two readers per source is uncounted and reads 14 → 28 → 15 | `4c3a222` R41-2 | `sched::tests::the_h1_fold_reads_forward_within_a_job` (the fold's shape, and 24 jobs of three playheads at P = 8); `preview::tests::merged_rewinds_reach_the_engine_stats` | 2a: the fold keeps every lookahead; 2b: folds not counted; 2c: the fold not flagged merged; 2d: the preview drain of `regions_folded` `*0` | 2a: `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1009 filtered out; finished in 0.00s`, "the fold keeps only the lookahead past 28"; 2b: `… 0.00s`, "the fold is counted"; 2c: `… 0.00s`, "the fold keeps only the lookahead past 28"; 2d: `… 0.38s`, "P = 2: 8 merged, 0 folded, 7 rewinds: the pre-roll folds are counted" |
| U-1: `Preview::sizes` and `Readers::travel` grow without bound | `c9076a7` R41-3 | `preview::tests::source_memory_is_bounded_and_forgets_removed_sources` (24 sources through a cap of 3: (1) kept, (2) removed one at a time, (3) cleared) | 3a: no cap (`make_room` never evicts); 3b: a new generation keeps every size; 3c: `forget` keeps a removed source's travel; 3d: a cache clear keeps every size | 3a: `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1011 filtered out; finished in 0.32s`, "(1) source 4: {1, 2, 3, 4} {1, 2, 3, 4}"; 3b: `… 0.52s` and 3c: `… 0.51s`, both "(2) source 1: a removed source is remembered"; 3d: `… 0.72s`, "(3) a clear forgets" |
| R41-5: a removed source's reader keeps its file open (Windows run 36528931046) | `c9076a7` R41-5 | `engine::tests::a_removed_source_has_no_open_decoder_after_the_next_frame` (the relink flow; source 1's reader is held at its next decode when the source leaves); U-1's witness, phases 2 and 3 | 5a: a removed source's readers are not retired; 5b: retired readers are not waited for; 5c: no stop flag for a retired reader | 5a (engine): `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1011 filtered out; finished in 0.41s`, "a frame came while a removed source's decoder was open"; 5a (U-1): `… 0.68s`, "(3) a clear closes the readers' decoders"; 5b: `… 0.41s`, the same message as 5a (engine); 5c: `… 1.42s`, "the retired reader decoded on" |

- **Three first survivals.**
  - R41-1b survived at `c96c9aa`. That witness counted only rewinds per job. Removing the lookahead retain moves the
    one rewind per job to within the job; it does not add one. The witness now separates within-job rewinds from
    across-job rewinds (`74781d9`).
  - R41-5b survived at `404b85e`. The retired idle reader exited before the empty frame arrived: a race the witness
    could not lose. The engine witness now holds source 1's reader at its next decode, with its decoder open, when the
    source leaves. It asserts that no frame arrives within 1 s, that the decoder is still open, and, after release,
    that the frame follows with nothing open (`447f758`).
  - R41-5c then survived at `447f758`. A one-frame decode completes and the reader retires at its next check without
    the flag; the flag only shortens its exit. The witness now also asserts that the retired reader's decode ended
    `Cancelled` (`c9076a7`).
  - Each survivor was rerun against the amended witness and killed, as were its siblings: 1a–1d at `74781d9`, 5a–5c
    at `c9076a7`. 3a–3d ran at `404b85e`; its U-1 code and witness are unchanged in `c9076a7`.
- **U-1's phase 2 does not by itself witness R41-5.** Under 5a it passed. Each later document spawns a reader, and
  H-5's rule retires obsolete idle readers while a ticket waits. The engine witness spawns no reader for the empty
  document, so it is the deterministic one.
- **The design text.**
  - **Amendment R41 (proposed), "Fallback regions rewind across jobs"** (design §6). A merged region reads forward
    within a job. It rewinds at most once per job per merged group. That is the degraded mode, the documented cost of
    exceeding R or H-1. Rewinds are counted in the new `PlaybackStats::merged_rewinds`. R37's continuation rule
    governs every non-fallback region.
  - **Amendment R41 (proposed), "A removed source closes its decoders"** (design §6, after H-7).
  - `PlaybackStats` gains `regions_folded` and `merged_rewinds`. The P-play lanes print them as
    `engine_regions_folded` and `engine_merged_rewinds`, beside `engine_regions_merged`, whose R38 meaning is kept
    (merges at the reader limit only).
- **Trade-off (R41-1).** In the degraded mode, the merged region's lookahead is only what lies past its last required
  time. So the earlier playhead's next frame is never prefetched, and it decodes on demand in every job, after one
  seek back.
  - The alternative keeps the earlier lookahead. It has the same count in steady state, one rewind per job, but it
    rewinds within the job.
  - Only the retain guarantees at most one rewind per job when the playheads seek or jump.
- **Other handles on a removed source (by reading; not run on Windows):**
  - The preview's synchronous renderer serves thumbnails and K-3. It closes its decoders in `bind` on a new
    generation, before the frame renders, and at every park (H-7).
  - `SourceSpec::measure` opens a decoder and drops it at once.
  - The worker's `set_document` pauses first. `quiesce` drops the audio runtime, and with it the mix sources'
    `AudioDecoder`s.
  - There are no other demux contexts in `engine.rs`. The U-1 maps hold keys and numbers, never handles.
- **The wait.** Retiring readers are waited for on the preview thread, never the UI's. `set_document` only sends. A
  reader leaves at its next check or packet boundary, but an open in progress runs to its end. The cost is one such
  delay before the first frame of a document that removed a source with a live reader. (Re-review R41 RS-1 found this
  wait unbounded. R43 bounds it at 200 ms and makes the reader's IO interruptible: E12.16.)
- **Narrowed in R43 (re-review R41 note 2).** Readers and the preview close a removed source's files. Visual-asset jobs
  (`analysis.rs`) and proof renderers (`monitor_proof_for_document`) hold a removed file until their own job ends.

#### E12.15.2 Tests, the stage gate and line counts

- **New tests:** 5, all fast tier (none marked slow, none ignored):
  - `sched::tests::a_merged_reader_rewinds_at_most_once_per_job`;
  - `sched::tests::the_h1_fold_reads_forward_within_a_job`;
  - `preview::tests::merged_rewinds_reach_the_engine_stats`;
  - `preview::tests::source_memory_is_bounded_and_forgets_removed_sources`;
  - `engine::tests::a_removed_source_has_no_open_decoder_after_the_next_frame`.
- **The two long PF1 tests** (`the_reader_model_holds_for_every_short_sequence` and
  `the_scheduler_survives_a_seeded_stress_on_real_threads`) stay in the fast tier, as ruled.
- **Per-commit gate.** Each R41 commit passed build, media clippy with `-D warnings`, rustfmt on the touched files
  and `cargo test -p kinewright-media --lib`. The last such media run, at R41-3/5 before its witness was
  strengthened, read `test result: ok. 956 passed; 0 failed; 56 ignored; 0 measured; 0 filtered out; finished in
  165.95s`.
- **The local stage gate at `c9076a7`** (every command at `nice -n 19`, `-j 4`, `RUST_TEST_THREADS=4`):
  - `cargo build --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`, each with and without the
    three `slow-tests` features: exit 0;
  - `cargo fmt -- --check`: exit 0;
  - `cargo build -p kinewright-app`: exit 0;
  - `python3 scripts/slow_tests.py lint`: "slow-test manifest and markers agree: 47 tests, features
    kinewright-agent/slow-tests,kinewright-app/slow-tests,kinewright-media/slow-tests; 44 allowlist entries
    well-formed";
  - `cargo test --workspace` (the fast tier): 36 `test result: ok` lines, 3,441 passed, 0 failed, 91 ignored.
    Media: `test result: ok. 956 passed; 0 failed; 56 ignored; 0 measured; 0 filtered out; finished in 165.98s`.
    The log is `s2b-logs/r41-workspace-fast.log.gz`;
  - `slow_tests.py verify-fast` on that log with `--os linux`: "fast tier on linux skipped exactly the 47 manifest
    tests and 44 allowlisted ignores (ci/ignored-tests.txt), each observed, nothing else".
  - The slow tier and Windows were not run locally. Both run in CI on the push.

Non-blank, non-comment `.rs` lines, net, counted as in E12.13.2. Fixture files count as tests.

| Commit | Production | Tests |
|---|---|---|
| R41-1 `74781d9` | 16 | 115 |
| R41-2 `4c3a222` | 33 | 52 |
| R41-3/5 `c9076a7` | 211 | 184 |
| **R41** | **260** (+289 −29) | **351** (+367 −16) |

- **Per file** (production + tests):
  - `sched.rs` 178 + 140;
  - `preview.rs` 60 + 138;
  - `render.rs` 17;
  - `engine.rs` 47 (tests);
  - `perf_fixtures.rs` 26 (tests);
  - `pf1_harness.rs` 3;
  - `media.rs` 2.
- `sched.rs`'s production share is mostly `SourceMemory` (the bounded map) and `Readers::forget`.
- **Docs before this section:** `PF1-PLAYBACK-PERFORMANCE.md` +48 −8. This commit adds E12.15 and corrects E12.14's
  count.

#### E12.15.3 E12.14 correction

E12.14.1 said "All 10 were killed". It was 9 failing witness runs over 8 distinct code mutations, plus G18's synthetic
checks. R38-7's mutation (`Some(demand.generated)`) reverts review B S2's earlier reservation fix, so its witness also
detects that revert. Both are corrected in place.

#### E12.15.4 Timing: pending, quiet window

Nothing was timed for R41. What is ready:

- **The binary:** the release test binary at `c9076a7`, `/tmp/kinewright-r37/bins/bin-c9076a7`, sha256
  `f1e835d8e2124c656f19acac3cf43874f3b9caff436ec1d93b9b8b535152de2b` (in that directory's `SHA256SUMS`, which
  `sha256sum -c` passes). It is built from the same code as this docs commit.
- **The plan:** `plan-gates.txt` now runs every stage-gate lane on that binary (26 lines repointed from
  `bin-b980b4e`). G18's candidate lane is `G18 LH R41`, and the plan still ends with `@G18 verdict`.
- **Unchanged:** the S0 side of G18 (`media-s0-g18`) and the step-0 A/B plan (`plan-ab.txt`).
- The lanes print the new counters. A fallback region's rewinds on the W workloads are measured with the rest.

### E12.16 R43 fixes: bounded, interruptible and panic-safe retirement; the in-order walk

Ruling R43 (`rereview-r41.md`, which rejected closing S2b's code at `f68e46d`; orchestrator note P46) covered four
blockers and the notes:

- RS-1: the preview's wait for retired readers was unbounded;
- RS-2: a reader's panic skipped its cleanup;
- RS-3: BF3-2 still admitted two rewinds in a job;
- RS-4: a late cancel left a tombstone;
- item 5: the file-release claim was too broad, and E12.15's counts were wrong (corrected in place: 16 witness runs
  over 15 distinct mutations, three first survivals).

One commit per fix on `pf1/impl`, after `f68e46d`: RS-3 `95c45b4`, RS-4 `684b7cf`, the merge of `origin/main`
(`e86c9ed`, the CI parser fix) as `963e486`, RS-2 `89f41d4` and RS-1 `3ef47d6`. This section and the design text are
in the docs commit after them. **The timing half is pending: quiet window.** No lane was run for R43 (E12.16.4).

#### E12.16.1 Closure table

Each mutation was applied alone to the committed code, and only its named witnesses were run
(`s2b-logs/r43-mutations.log.gz`; the script is `r43-mutate.sh.gz`). There are **19 witness runs over 16 distinct
code mutations**. Three survived first (R43-3b, 3c and 4c). Each witness was strengthened, its commit was amended,
and the survivor was rerun and killed. **Every mutation is killed at the final commits.**

| Finding | Commit | Witness | Mutation | Raw result under the mutation |
|---|---|---|---|---|
| RS-3: a replan posted mid-job, with an older in-flight decode that fails stale, made a second rewind in one job | `95c45b4` | `sched::tests::a_mid_job_replan_with_a_failing_decode_rewinds_once` (Astra's counterexample, scripted); `mid_job_replans_rewind_at_most_once_per_job` (300 seeds at P = 2 and 3); `evicted_lookahead_behind_a_reader_is_not_read_again_within_the_job`; `a_stopped_lookahead_decode_is_decoded_again_without_a_rewind`; the exhaustive and seeded models, whose every decode checks the bound | 3a: required times skipped as before R43; 3b: lookahead ignores the reader's floor; 3c: a stopped decode keeps its position | 3a: `test result: FAILED. 12 passed; 4 failed; 0 ignored; 0 measured; 998 filtered out; finished in 0.12s`, "it rewinds to 0, then waits at 14" and "R43: reader 0 rewound to 0 from Some(1) within v3"; 3b (rerun): `test result: FAILED. 17 passed; 1 failed; 0 ignored; 0 measured; 998 filtered out; finished in 95.55s`, "2 is behind it: not read again"; 3c (rerun): `test result: FAILED. 17 passed; 1 failed; 0 ignored; 0 measured; 998 filtered out; finished in 95.49s`, "15 is decoded again" |
| RS-4: a cancel after the reader's `forget` kept a tombstone | `684b7cf` | `sched::tests::a_cancel_after_its_reader_exited_leaves_nothing` (grant, retire, forget, then cancel; and the cap); `preview::tests::an_obsolete_ticket_is_cancelled_while_the_pool_is_held` (P = 2, real threads); the model checks `cancelled ⊆ live` | 4a: a departed reader's cancel is kept; 4b: no cap; 4c: the preview never joins its readers to the book | 4a: `test result: FAILED. 18 passed; 1 failed; 0 ignored; 0 measured; 998 filtered out; finished in 99.27s`, "a tombstone of a departed reader"; 4b: `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1016 filtered out; finished in 0.00s`, "capped"; 4c (rerun): `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1017 filtered out; finished in 60.29s`, "the lane never reached the condition" |
| RS-2: a reader's panic skipped permit release, slot removal and notify | `89f41d4` | `preview::tests::a_panicking_reader_releases_its_permits_and_its_slot` (a cfg(test) seam panics the reader at its first decode) | 2a: the guard skips an unwinding reader (the old tail); 2b: permits not forgotten on unwind; 2c: an unwinding reader leaves as if it had returned | 2a: `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1018 filtered out; finished in 60.28s`, "the lane never reached the condition"; 2b: `… finished in 0.29s`, "its permits return once"; 2c: `… finished in 60.27s`, "the job waited on a panicked reader" |
| RS-1: a reader blocked in FFmpeg IO stalled every later preview job; note 1: an open after retirement | `3ef47d6` | `decode::tests::an_interrupt_ends_a_readers_packet_read`; `preview::tests::an_interrupt_ends_a_retired_readers_open`; `a_retirement_stuck_in_io_renders_at_the_deadline`; `a_reader_retired_before_its_open_never_opens_the_file`; `engine::tests::a_removed_source_has_no_open_decoder_after_the_next_frame` (updated) | 1a: the callback ignores the flag; 1b: the wait is unbounded again; 1c: no overrun counted; 1d: no flag check before the open; 1e: `packets()` again; 1f: a detached reader is waited for again; 1g: a detached reader keeps its permits | 1a: `test result: FAILED. 2 passed; 3 failed; 0 ignored; 0 measured; 1018 filtered out; finished in 0.76s`, "the read failed with AVERROR_EXIT", "the open failed with AVERROR_EXIT"; 1b: `test result: FAILED. 2 passed; 3 failed; … finished in 30.69s`, "the frame, at the deadline: Timeout" (three witnesses); 1c: `test result: FAILED. 2 passed; 3 failed; … finished in 0.75s`, "the overrun counts"; 1d: `test result: FAILED. 4 passed; 1 failed; … finished in 0.75s`, "a retired reader's open read its file"; 1e: `test result: FAILED. 4 passed; 1 failed; … finished in 60.05s`, "the interrupted read ended: Timeout"; 1f: `test result: FAILED. 4 passed; 1 failed; … finished in 0.83s`, "waited once"; 1g: `test result: FAILED. 3 passed; 2 failed; … finished in 30.64s`, "the frame, at the deadline: Timeout" |

- **Three first survivals.**
  - R43-3b and 3c survived at `cfb9966` (`test result: ok. 16 passed; …`). No witness evicted lookahead behind a
    reader or stopped a lookahead decode. The two sched witnesses were added and RS-3 was amended to `95c45b4`. 3a's
    code is unchanged by the amend.
  - R43-4c survived at `a6b8c2d`. No preview test cancelled a ticket. The P = 2 preview witness was added, and RS-4 was
    amended to `684b7cf`. 4a and 4b ran at `a6b8c2d`; their code is unchanged by the amend.
- **What is not witnessed (qualified).**
  - The close's shutdown and supersession escapes. Without them the wait still ends at the deadline, so only a
    timing assertion could tell them apart, and it would be flaky.
  - RS-3's wake of a reader waiting on another's in-flight time. Without it, the waiting reader resumes at its
    quiescence timer (≤ 5 s): a latency, not a hang. The model has no condition variables.
  - `an_interrupt_ends_a_retired_readers_open` prints `retire_overruns` rather than asserting 0. Its reader exits
    within milliseconds of the flag, but an assertion against a 200 ms clock under CI load would be a flake risk.
    `a_retirement_stuck_in_io_renders_at_the_deadline` is the deterministic one: its reader cannot exit before release.

#### E12.16.2 Design choices and deviations

- **RS-3, the in-order walk (Amendment R43, design §6).** A reader decodes the first required time that has neither a
  frame nor a failure, and waits instead of passing it. Lookahead comes only after every required time is resolved,
  and only past its floor in the current plan version. A stopped decode rolls the reader back to just before that
  time. So only the first decode in a plan version can rewind: at most one rewind per reader per plan version, and a
  job posts one. The trade-off: a reader no longer decodes past a time another reader has in flight. It waits for
  that result, and the delivery wakes it.
- **RS-4.** `PermitBook` keeps a `live` set: `join` at spawn, before the thread runs, and `forget` at exit. A `cancel`
  of an id that is not live is a no-op. `cancelled` is capped at 64 as a backstop, dropping the lowest id; only live
  readers' cancels are kept, and there are at most R of them.
- **RS-2.** `read()`'s first binding is the `Exit` guard, so it drops last: after `Sched` is released and the decoder
  has closed. It forgets the reader's permits, removes the slot and wakes the preview, on return and on unwind. On
  unwind it also fails the reader's required times ("decode-reader: the reader panicked"), so the job answers.
  `fail_start` now records only the times the job still requires.
- **RS-1 (a), the interrupt.**
  - `ffmpeg-next` 8.0's `format::input_with_interrupt` installs the `AVIOInterruptCB`. It is the crate's safe API; the
    workspace forbids `unsafe`.
  - That API leaks the callback's box. The callback is a zero-sized `fn`, so the box allocates nothing. It reads the
    stop flag of the reader running on the calling thread (a thread-local set at the reader's open). A reader's
    decoder opens, seeks and reads only on its own thread, and FFmpeg calls the callback on the thread doing the IO.
  - `SourceSpec::measure`, on the preview thread, and every synchronous renderer keep the uninterruptible open.
  - The packet loop reads one packet per turn and checks the flag first. An EOF seen while stopped is not treated as
    an end. A failed or stopped window leaves no cursor, so the next window seeks.
- **RS-1 (b), the deadline: `RETIRE_DEADLINE` = 200 ms.** Neither the flag nor the interrupt can end one libavcodec
  call and one frame conversion that are already running. By E0's figures that is tens of milliseconds: a whole
  keyframe-to-target seek took 12–41 ms, and the pre-S1 1080×1920 conversion 81 ms. 200 ms covers that with room and
  stays under R43's 250 ms ceiling on the stall after an edit that removes a source.
- **Deviation from P46's wording, withdrawn by R47 (E12.17): detached readers' permits returned at the deadline.**
  P46 had the reader's own exit guard release its permits. At P = 2, though, a stuck reader holds the permits its
  replacement needs, so the "frame at the deadline" never arrived: mutation 1g showed it (two witnesses time out).
  - The justification given here was wrong. It said a detached reader is blocked inside one IO or libavcodec call,
    so its frame threads are idle and H-5's P is exceeded only by idle threads. A reader past 200 ms may still be
    decoding or converting (re-review R43 blocker 2), so the return broke H-5's bound. It also left P = 1 (the
    detached reader holds the only slot) and retained bytes stalling a frame (blocker 1).
  - R47 reverts it: a detached reader keeps its permits, slot and bytes until its exit guard runs, and a frame they
    detain renders by K-3 (E12.17).
- **The degraded case (Windows).** A detached reader closes its decoder when its blocked call returns. Until then its
  file stays open, and on Windows it stays locked. `retire_overruns` counts each wait that ended this way.
  `PlaybackStats` gains `retire_overruns`, and the P-play lanes print it as `engine_retire_overruns`.
- **Unchanged (H-6), until R47.** `Preview::drop` still joins every reader. A reader blocked in the OS forever would
  hold shutdown, but not playback. (R47 bounds the join by the deadline: E12.17.)
- **Item 5.** The design's R41 file-release amendment now covers only readers and the preview's synchronous renderer,
  unless the retirement overran. Visual-asset jobs (`analysis.rs`, such as a waveform's decode) and proof renderers
  (`monitor_proof_for_document`) hold a removed file until their own job ends. E12.15's "other handles" note says the
  same.

#### E12.16.3 Tests, the stage gate and line counts

- **New tests: 11**, all fast tier (none marked slow, none ignored):
  - sched: `a_mid_job_replan_with_a_failing_decode_rewinds_once`, `evicted_lookahead_behind_a_reader_is_not_read_again_within_the_job`,
    `a_stopped_lookahead_decode_is_decoded_again_without_a_rewind`, `mid_job_replans_rewind_at_most_once_per_job` and
    `a_cancel_after_its_reader_exited_leaves_nothing`;
  - preview: `an_obsolete_ticket_is_cancelled_while_the_pool_is_held`, `a_panicking_reader_releases_its_permits_and_its_slot`,
    `an_interrupt_ends_a_retired_readers_open`, `a_retirement_stuck_in_io_renders_at_the_deadline` and
    `a_reader_retired_before_its_open_never_opens_the_file`;
  - decode: `an_interrupt_ends_a_readers_packet_read`.
- **Changed:** `engine::tests::a_removed_source_has_no_open_decoder_after_the_next_frame` now expects the frame at the
  deadline, with the held reader's decoder still open and one overrun counted. After release, the reader stops and
  closes. The exhaustive model gains a BudgetWait start state, so its coverage cells hold under the walk.
- **The two long PF1 tests** stay in the fast tier, as ruled.
- **Per-commit gate.** Each R43 commit passed media clippy with `-D warnings`, rustfmt on the touched files and
  `cargo test -p kinewright-media --lib`. The runs read:
  - RS-3 at `cfb9966`, before its two added witnesses: `test result: ok. 958 passed; 0 failed; 56 ignored; 0 measured;
    0 filtered out; finished in 168.57s`;
  - RS-4 at `a6b8c2d`, before its added witness: `test result: ok. 961 passed; 0 failed; 56 ignored; 0 measured; 0
    filtered out; finished in 171.20s`;
  - RS-2: `test result: ok. 963 passed; 0 failed; 56 ignored; 0 measured; 0 filtered out; finished in 170.43s`;
  - RS-1: `test result: ok. 967 passed; 0 failed; 56 ignored; 0 measured; 0 filtered out; finished in 171.19s`.
  - The amended RS-3 and RS-4 had no full media run of their own. Their witnesses ran in the mutation reruns, and the
    full runs at RS-2 and RS-1 include every one of them.
- **The merge of main (`963e486`).** `python3 scripts/slow_tests.py lint` read "slow-test manifest and markers agree: 47
  tests, features kinewright-agent/slow-tests,kinewright-app/slow-tests,kinewright-media/slow-tests; 44 allowlist
  entries well-formed".
- **The local stage gate at `3ef47d6`.** Every command ran at `nice -n 19`, `-j 4` and `RUST_TEST_THREADS=4`.
  - `cargo build --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`, each with and without the
    three `slow-tests` features: exit 0.
  - `cargo fmt -- --check`: exit 0.
  - `cargo build -p kinewright-app`: exit 0.
  - `slow_tests.py lint`: the same line as above.
  - `cargo test --workspace` (the fast tier): 36 `test result: ok` lines; 3,452 passed, 0 failed, 91 ignored. The
    media line was `test result: ok. 967 passed; 0 failed; 56 ignored; 0 measured; 0 filtered out; finished in
    168.86s`. The log is `s2b-logs/r43-workspace-fast.log.gz`.
  - `slow_tests.py verify-fast` on that log with `--os linux`: "fast tier on linux skipped exactly the 47 manifest tests
    and 44 allowlisted ignores (ci/ignored-tests.txt), each observed, nothing else".
- **A parser gap: the first fast-tier run.** It was equally green (3,452 passed, 0 failed, 91 ignored; media 967
  passed), but `verify-fast` refused it:

  ```
  kinewright_media: saw 966 passed, 0 failed, 56 ignored, 0 measured, but its summary says 967, 0, 56, 0
  ```

  - A whole swscaler log line landed between `test title::tests::vertical_social_caption_pixels_stay_inside_shared_safe_bounds ...`
    and its `ok`, which came on the next line. Main's parser fix (`e86c9ed`) handles fragments on the result's line,
    not an outcome pushed to the next line, so CI can hit this too.
  - The test run and `verify-fast` were rerun once, giving the log above. Both logs are in `r43-gate-logs.log.gz`.
- The slow tier and Windows were not run locally. Both run in CI on the push.

Non-blank, non-comment `.rs` lines, net, counted as in E12.13.2 (only a `mod tests` block counts as tests):

| Commit | Production | Tests |
|---|---|---|
| RS-3 `95c45b4` | 32 | 205 |
| RS-4 `684b7cf` | 23 | 93 |
| merge `963e486` | 0 | 0 |
| RS-2 `89f41d4` | 30 | 51 |
| RS-1 `3ef47d6` | 292 | 196 |
| **R43** | **377** (+462 −85) | **545** (+554 −9) |

- **Per file** (production + tests):
  - `decode.rs` 200 + 57;
  - `preview.rs` 88 + 238;
  - `sched.rs` 48 + 251;
  - `render.rs` 39;
  - `engine.rs` −1 (tests);
  - `pf1_harness.rs` 1;
  - `media.rs` 1.
- Of `decode.rs`'s 200 production lines, 131 are the `#[cfg(test)]` `interrupt_seam` module. It is a test seam compiled
  only into tests, but it sits outside `mod tests`.
- **Docs:** `PF1-PLAYBACK-PERFORMANCE.md` gains the two R43 amendments (the walk; bounded, interruptible and panic-safe
  retirement with the narrowed file-release claim). E12.15's counts and its "other handles" note are corrected in
  place.

#### E12.16.4 Timing: pending, quiet window

Nothing was timed for R43. What is ready:

- **The binary:** the release test binary at `3ef47d6`, `/tmp/kinewright-r37/bins/bin-3ef47d6`, sha256
  `95658f42f9fccecf6fe6ab3cb9a1cf8dc0173dfda26a61697c014ce6c53c8068`. It is in that directory's `SHA256SUMS`, and
  `sha256sum -c` passes. It is built from the same code as this docs commit.
- **The plan:** `plan-gates.txt` now runs every stage-gate lane on that binary: its 25 lane lines and its header are
  repointed from `bin-c9076a7`. G18's candidate lane is `G18 LH R43`, and the plan still ends with `@G18 verdict`.
  A copy is `s2b-logs/r43-plan-gates.txt.gz`.
- **Unchanged:** the S0 side of G18 (`media-s0-g18`) and the step-0 A/B plan (`plan-ab.txt`).
- The lanes print `engine_retire_overruns`. The W workloads remove no source mid-run, so it should read 0.

### E12.17 R47 fixes: detached readers never stall a frame; H-5 exact; CI run 36547988690

Ruling R47 (`rereview-r43.md`, which rejected closing S2b's non-timing part at `d041829`; orchestrator note P50)
kept RS-2, RS-4, note 5 and RS-3's stale-failure fix closed, and raised three blockers:

- (a) a stuck detached reader could stall a frame indefinitely: at P = 1 it holds the only slot, and its retained
  `Hold` bytes can block admission with slots spare;
- (b) R43's return of a detached reader's permits at the deadline broke H-5's P bound (E12.16.2's justification,
  "its frame threads are idle", was unsupported);
- (c) two witnesses deleted a file while a reader held it open, which Windows refuses.

CI run 36547988690 at `d041829` failed on both OSes, and both failures are in this round:

- Windows, fast tier: `a_retirement_stuck_in_io_renders_at_the_deadline` (preview.rs:3871:37) and
  `an_interrupt_ends_a_retired_readers_open` (preview.rs:3832:37) both panicked with `the source is removed: Os {
  code: 32, kind: Uncategorized, message: "The process cannot access the file because it is being used by another
  process." }` (blocker c); the media line was `test result: FAILED. 961 passed; 2 failed; 56 ignored; 0 measured; 0
  filtered out; finished in 663.09s`.
- Linux, fast tier: `preview::tests::a_widening_plan_rebalances_the_permits` panicked at preview.rs:2482:29, "the lane
  never reached the condition" (E12.17.3).

Commits on `pf1/impl` after `d041829`: the fix `13fd3fd`, the widening witness `ae0249e`, the merge of `origin/main`
(`1593758`, the second CI parser fix) as `9b09a30`, and the docs commit with this section. **The timing half is still
pending: quiet window** (E12.17.5).

#### E12.17.1 What changed

- **Item 1, the revert.** `Preview::close` no longer forgets a detached reader's permits. A detached reader keeps its
  slot, its permits and its bytes in flight until its own `Exit` guard runs, so accounted reader permits never exceed
  P. The "idle threads" comment and E12.16.2's deviation are corrected.
- **Item 2, K-3 for a detained set.** `Readers::detained(detached, planned, set)` is true when the job's required
  regions need more slots than R leaves beside the detached readers, or more readers of a source than H-1 allows beside
  them; when they hold the whole pool; or when the set does not fit in C beside their bytes in flight. It considers
  slots and permits only if the plan has a required region, so an empty document is never detained. The preview
  decides once, before the post (`Preview::plan`): detached readers can only exit while the job waits, which frees.
  A detained frame takes K-3's synchronous fallback with reason 2 (`LaneState::fallbacks` is now `[u64; 3]`), counted
  in `sync_fallback_frames` and the new `PlaybackStats::detained_fallback_frames`; the timing lanes print
  `engine_detained_fallback_frames`. Each job decides again, so readers serve again once the detached readers exit.
- **Item 3, Windows-safe witnesses.** Every witness that holds a reader in a file's IO now drops the source from the
  document first and deletes the file only after the reader has exited, asserting the delete succeeds ("the reader
  released its file"). No test deletes a held file (`grep remove_file` in `preview.rs`, `engine.rs`, `decode.rs` and
  `render.rs`: only these post-exit deletes). This is verified on Windows only by CI on the push.
- **Item 4, bounded shutdown.** `Preview::drop` waits for its readers at most `RETIRE_DEADLINE`, joins those whose
  slot is gone (their `Exit` ran), and leaves a reader still alive detached: its thread owns an `Arc<Lane>` and exits
  on its own. `PlaybackStats::shutdown_detached_readers` counts it; the lanes print
  `engine_shutdown_detached_readers`. The preview's reader handles are now kept by id.
- **Item 5, deterministic witnesses.** The supersession and shutdown escapes are witnessed on the test thread with a
  seam-held reader, asserting `Halt::Superseded`, no overrun and nothing detached, never a time. The
  interruptible-open witness asserts through the seam that the callback saw the flag and the open failed with
  `AVERROR_EXIT`, and no longer prints an overrun count. RS-3's reader wake stays documented as partial (E12.16.1).
- **Item 6, docs.** Design §6 gains Amendment R47 (K-3 for a detained set, H-5 written exactly as
  min(R × min(P, 16), P), since detached readers keep their permits; the draft's `P + D` double-counted them
  (rereview-r47.md), bounded shutdown, Windows-safe witnesses), qualifies the interrupt to local-file IO on the
  reader's thread (FFmpeg's `async` protocol reads on a helper thread), and restates RS-3's bound as "at most one
  scheduled-frame rewind per reader per plan version", which does not bound FFmpeg's seeks. R43's retirement
  amendment, H-6 (5) and K-3's reasons are updated in place.

#### E12.17.2 Closure table

Each mutation was applied alone, with only its named witnesses run (`s2b-logs/r47-mutations.log.gz`; the script is
`r47-mutate.sh.gz`). R47-1 and R47-7 ran first on the working tree, before `plan` was extracted from `schedule` for
clippy's line limit and rustfmt rewrapped the join, and were rerun on `13fd3fd` (1c, 7c). The other mutated lines
are textually identical at `13fd3fd`. **11 runs over 9 distinct mutations; every one is killed, none survived.**

| Item | Witness | Mutation | Raw result under the mutation |
|---|---|---|---|
| 2 | `preview::tests::at_p1_a_stuck_detached_reader_leaves_its_frames_to_k3`, `at_p2_…` and `a_detached_readers_bytes_leave_its_frames_to_k3` | 1: no detained fallback | 1: `test result: FAILED. 0 passed; 3 failed; 0 ignored; 0 measured; 1026 filtered out; finished in 60.68s`, "the frame, by K-3: Timeout" (three); 1c at `13fd3fd`: `… 0 passed; 3 failed; … finished in 60.63s` |
| 1 | the same three | 2: R43's permit return at the deadline restored | `test result: FAILED. 0 passed; 3 failed; 0 ignored; 0 measured; 1026 filtered out; finished in 0.83s`, "it keeps its permits" (left: 0) |
| 2 | `sched::tests::detached_readers_detain_a_plan_by_what_they_keep` (+ the three) | 3: no slot clause | `test result: FAILED. 3 passed; 1 failed; … finished in 0.87s`, "no slot is free" |
| 2 | as above | 4: no H-1 clause | `test result: FAILED. 3 passed; 1 failed; … finished in 0.89s`, "a third reader of 0" |
| 2 | as above | 5: no permit clause | `test result: FAILED. 2 passed; 2 failed; … finished in 60.38s`, "no permit is free"; P = 2's "the frame, by K-3: Timeout" |
| 2 | as above | 6: no bytes clause | `test result: FAILED. 2 passed; 2 failed; … finished in 60.68s`, "not beside f"; the bytes witness's "the frame, by K-3: Timeout" |
| 4 | `preview::tests::shutdown_detaches_a_reader_stuck_past_the_deadline` | 7: the join unbounded again (every handle joined) | 7: `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1028 filtered out; finished in 60.28s`, "the preview did not exit cleanly" (left: `Err(Timeout)`); 7c at `13fd3fd`: `… finished in 60.27s` |
| 5 | `preview::tests::shutdown_ends_a_retirements_wait` | 8a: no shutdown escape | `test result: FAILED. 1 passed; 1 failed; … finished in 0.74s`, "no deadline" (left: 1) |
| 5 | `preview::tests::a_newer_post_ends_a_retirements_wait` | 8b: no supersession escape | `test result: FAILED. 1 passed; 1 failed; … finished in 0.73s`, "no deadline" (left: 1) |

- **Isolation.** At P = 1 the detached reader holds both the only slot and the pool, so mutation 3 alone leaves the
  P = 1 witness green (the permit clause still detains). The pure `sched` witness isolates each clause: eight
  one-permit detached readers at P = 9 (R = 8) for slots, a detached reader of source 0 and a plan of two regions of
  0 for H-1, P = 2 for permits and C = 3 f − 1 for bytes, each short of that one resource only, with negatives
  within all four, with no detached reader, with an empty plan, and after the reader exits.
- **Without the fixes.** Each witness fails with its fix removed: mutations 1 to 7 each take out one part of
  items 1, 2 and 4, and 8a/8b take out the escapes item 5 witnesses. The logs show every failure.
- **Not witnessed (qualified).** RS-3's reader wake stays partial, as in E12.16.1. Windows' file release is asserted by
  the post-exit deletes, but only CI runs them on Windows.

#### E12.17.3 CI run 36547988690, Linux: `a_widening_plan_rebalances_the_permits`

- **Not caused by R43.** R43 did not change the permit path (`acquire`, `Next::Open`, `Next::Close`, `next()`'s
  shrink and grow). The test is S2b-2's and passed on every earlier pf1 run, and on Windows in this run.
- **The cause: the test assumed one thread interleaving.** At P = 20, reader A reads one source on 16 permits; the plan
  widens to two sources (w = 10). The wait after it required two slots with no permits, which holds only if:
  - the newcomer B polls for permits while A still holds 16, so it is granted 4 (short);
  - and B goes inactive after A has released, so it closes to grow at once.
  - If A closes first (it shrinks as soon as B's ticket is queued), B is granted all 10 and never closes. If B goes
    inactive before A's release, it re-evaluates only at its 5 s quiescence timer, where it closes and may retire at
    once. Either way the condition is never met, and the wait times out.
  - The product behaviour is H-5's in both interleavings: FIFO grants of min(w, free), no reader waiting with permits.
    The second leaves a short reader short until its next wake (the next job, during playback), a latency only.
- **Evidence** (`s2b-logs/r47-widening.log.gz`). A probe, not committed, delayed reader 1's (B's) permit poll or
  reader 0's (A's) close by 300 ms under an environment variable:

  ```
  old test, no probe:      test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1028 filtered out; finished in 0.37s
  old test, B's poll late: test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1028 filtered out; finished in 60.58s
  old test, A's close late: test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1028 filtered out; finished in 60.32s
  new test, no probe:      test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1028 filtered out; finished in 0.38s
  new test, B's poll late: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1028 filtered out; finished in 0.90s
  new test, A's close late: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1028 filtered out; finished in 5.32s
  ```

  Both failures read "the lane never reached the condition", CI's message. Under load alone it did not reproduce
  here: 300 runs of the old test beside 24 CPU-bound loops (all at `nice -n 19`) passed ("load loop bin-old-probe: 300
  passed, 0 failed of up to 300"), as did 300 of the new ("load loop bin-new-probe: 300 passed, 0 failed of up to
  300"). CI's media run took 476 s on its runner, so its scheduling is the likelier trigger; the probe shows the
  interleavings are real, not that CI took a particular one.
- **The fix (`ae0249e`, test only).** A is held decoding the widened job's frame (`hold_at`); a reader shrinks only
  while inactive, so A keeps 16 until released and B's grant is 4 whatever the timing (asserted: `[4, 16]`). The wait
  then asks only that every reader has closed, which, once reached, holds (a reader that retires at its quiescence
  timer has also closed). The final 10 + 10 is unchanged.

#### E12.17.4 Tests, the gate and line counts

- **New tests: 7**, all fast tier (none marked slow, none ignored):
  - sched: `detached_readers_detain_a_plan_by_what_they_keep`;
  - preview: `at_p1_a_stuck_detached_reader_leaves_its_frames_to_k3`, `at_p2_a_stuck_detached_reader_leaves_its_frames_to_k3`,
    `a_detached_readers_bytes_leave_its_frames_to_k3`, `a_newer_post_ends_a_retirements_wait`,
    `shutdown_ends_a_retirements_wait` and `shutdown_detaches_a_reader_stuck_past_the_deadline`.
- **Replaced:** `a_retirement_stuck_in_io_renders_at_the_deadline` became the shared `stuck_retirement(P)`, run by
  the two `at_p…` witnesses. The media lib goes from 967 to 973 tests.
- **Changed:** `an_interrupt_ends_a_retired_readers_open` (Windows-safe, asserts through the seam, no `eprintln`) and
  `a_widening_plan_rebalances_the_permits` (E12.17.3). The `fallbacks` assertions now compare three reasons.
- **The two long PF1 tests** stay in the fast tier, as ruled.
- **Per-commit gate.**
  - `13fd3fd`: media clippy with `-D warnings`, rustfmt on the touched files, and `cargo test -p kinewright-media
    --lib`: `test result: ok. 973 passed; 0 failed; 56 ignored; 0 measured; 0 filtered out; finished in 170.97s`.
  - Its witnesses alone: `test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 1019 filtered out; finished
    in 1.02s` and `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 1027 filtered out; finished in 0.59s`.
  - `ae0249e` (one test changed) ran with the other permit witnesses: `test result: ok. 4 passed; 0 failed; 0 ignored;
    0 measured; 1025 filtered out; finished in 0.44s`. The stage gate below covers it in full.
- **The merge of main (`9b09a30`).** `python3 scripts/slow_tests.py lint` read "slow-test manifest and markers agree: 47
  tests, features kinewright-agent/slow-tests,kinewright-app/slow-tests,kinewright-media/slow-tests; 44 allowlist
  entries well-formed".
- **The local stage gate at `9b09a30`.** Every command ran at `nice -n 19`, `-j 4` and `RUST_TEST_THREADS=4`.
  - `cargo build --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`, each with and without the
    three `slow-tests` features: exit 0.
  - `cargo fmt -- --check`: exit 0.
  - `cargo build -p kinewright-app`: exit 0.
  - `slow_tests.py lint`: the same line as above.
  - `cargo test --workspace` (the fast tier), run twice: each gave 36 `test result: ok` lines; 3,458 passed, 0 failed,
    91 ignored. The media lines were `test result: ok. 973 passed; 0 failed; 56 ignored; 0 measured; 0 filtered out;
    finished in 169.97s` and `… finished in 169.75s`.
  - `slow_tests.py verify-fast` on the second log with `--os linux`: "fast tier on linux skipped exactly the 47
    manifest tests and 44 allowlisted ignores (ci/ignored-tests.txt), each observed, nothing else". Both logs are in
    `s2b-logs/r47-gate-logs.log.gz`.
- **A parser gap remains after `1593758`.** `verify-fast` refused the first, equally green run:

  ```
  could not parse cargo test output: line 57612: kinewright_media: saw 972 passed, 0 failed, 56 ignored, 0 measured, but its summary says 973, 0, 56, 0.
  ```

  - This is a third shape. An FFmpeg prefix came before `test` on the result's line, the message followed the `...`,
    and the outcome `ok` came on the next line:

    ```
    [swscaler @ 0x73c2e0b48d80] test room_tone_store::tests::capture_room_tone_refuses_a_bad_or_over_long_range_before_it_decodes_anything ... No accelerated colorspace conversion found from yuv420p to rgba64le.
    ok
    ```

  - Main's two fixes handle a fragment on the result's line (`e86c9ed`) and a whole message between the result and its
    outcome (`1593758`), not this combination. CI can hit it too.
  - The test run and `verify-fast` were rerun once, giving the accepted log above.
- The slow tier and Windows were not run locally. Both run in CI on the push; Windows is where item 3 is really tested.

Non-blank, non-comment `.rs` lines, net, counted as in E12.13.2 (only a `mod tests` block counts as tests):

| Commit | Production | Tests |
|---|---|---|
| fix `13fd3fd` | 63 (+85 −22) | 226 (+246 −20) |
| widening witness `ae0249e` | 0 | 9 (+12 −3) |
| merge `9b09a30` | 0 | 0 |
| **R47** | **63** | **235** |

- **Per file** (production + tests): `preview.rs` 32 + 179; `sched.rs` 26 + 56; `pf1_harness.rs` 3; `media.rs` 2.
- **Docs:** `PF1-PLAYBACK-PERFORMANCE.md` gains Amendment R47, and its RS-3 bound, interrupt scope, R43 retirement
  text, H-6 (5) and K-3 reasons are corrected in place. E12.16.2's deviation and H-6 bullets are marked withdrawn or
  updated by R47.

#### E12.17.5 Timing: pending, quiet window

*Run on 2026-10-05 with rebuilt binaries: see E12.18.*

Nothing was timed for R47. What is ready:

- **The binary:** the release test binary at `9b09a30`, `/tmp/kinewright-r37/bins/bin-9b09a30`, sha256
  `4dd3d04707c0f5beae15f55c9bd1a6b4185634006e6176341f7d50db37b01796`. It is in that directory's `SHA256SUMS`, and
  `sha256sum -c` passes. It is built from the same code as this docs commit.
- **The plan:** `plan-gates.txt` now runs every stage-gate lane on that binary: its 25 lane lines and its header are
  repointed from `bin-3ef47d6`. G18's candidate lane is `G18 LH R47`, and the plan still ends with `@G18 verdict`.
  A copy is `s2b-logs/r47-plan-gates.txt.gz`.
- **Unchanged:** the S0 side of G18 (`media-s0-g18`) and the step-0 A/B plan (`plan-ab.txt`).
- The lanes also print `engine_detained_fallback_frames` and `engine_shutdown_detached_readers`. The W workloads remove
  no source mid-run and detach no reader, so both should read 0, as should `engine_retire_overruns`.

### E12.18 S2b timing: the step-0 A/B and the slim R47 gates (2026-10-05)

Riel gave a quiet window. By agreement it ran a **slim plan**: step 0, then the lanes that answer "is S2b's playback
fixed?" and "did anything regress?". The other lanes move to one combined run after S2c (E12.18.5). All logs, plans
and scripts are in `s2b-logs/r47-timing-2026-10-05.tar.gz`.

#### E12.18.1 Binaries, machine and conditions

- **Rebuilt after a reboot** cleared `/tmp`, from the archived plans and scripts, each in its own target directory
  (a target directory shared across worktrees had mixed `kinewright-core` between commits). rustc 1.98.0.

  | Binary | Source | sha256 |
  |---|---|---|
  | `bin-d1fa8fd` (R47) | `pf1/impl` `d1fa8fd`; `crates/` identical to `9b09a30` | `0f7a7c0328f26b0ecede197bd1a40d7457c0fa4e30e852e68b8efdfe4b378a49` |
  | `bin-10a2d18` (A) | `10a2d18`, the S2b code | `4d9e206cd41d27cbf9b48f7cbd760747919c0b8a19d2193d2cf4e350670b1d84` |
  | `bin-712d108` (B) | `712d108`, R37 | `2adefd0b347b8bb3ef9d43f82b8b712b211a75deac3f84b2f24b0b08f3b4e10d` |
  | `media-s0-g18` | `d19bdf9` + `pf1_export_lane.rs` at `2aa4e1b` | `c8ab80032da4c59149522d0c1dec19aa751d8be32bf24cca4a199de9cabd93e9` |

- **Machine:** i5-13600K; RTX 3090 on NVIDIA 615.71.09; llvmpipe (**LLVM 23.1.1**, was 22.1.8 in E12.1); Linux
  7.2.5-4-omarchy; FFmpeg n8.0-23-gd1f31a829d.
- **When:** the launcher waited for 3 min of load < 2 with no process above 20% CPU, then ran step 0 from 12:16:13
  to 12:32:21 EDT and the gates from 12:32:21 to 13:01:36 EDT, each lane alone.
- **Contamination:** the sampler (E12.13.5) recorded every 5 s. Riel ruled the Omarchy bar (`quickshell`) and its
  widget pollers baseline load (Amendment R48); from the gates on, the runner exempts their descendants and the test
  binary's own children. Every flag raised was one of:
  - the fixtures' `ffmpeg`, a child of the test, before the timed part;
  - the bar's agent-usage poller (one core for ~15 s every 15 min) and its calendar poll (`gcalcli`, ~22%, seconds);
  - one `opencode serve` start-up spawned by T3 Code (65% for ~3 s) in `G18 LH S0 run1`, which was the faster S0 run.
- **GPU state:** in every LH lane the RTX sat mostly at **P8**. G3 lanes: 52–62% of samples at P8, mean graphics
  clock 620–850 MHz; P-play lanes: 87–93% at P8, 310–390 MHz. This confirms E12.13.4's idle-clock hypothesis.

#### E12.18.2 Step 0: the interleaved A/B (A = `10a2d18`, B = `712d108`)

G3 narrowed with `R28_ONLY=typical_1080p,blend_heavy_1080p` (fps, three runs per lane):

| Lane | `typical_1080p` | `blend_heavy_1080p` |
|---|---|---|
| A1 | 64.8, 49.1, 47.1 | 45.6, 47.7, 47.0 |
| B1 | 46.1, 46.0, 45.1 | 46.4, 49.9, 48.3 |
| A2 | 45.6, 43.9, 45.9 | 42.5, 48.2, 48.3 |
| B2 | 45.0, 48.4, 47.7 | 47.4, 45.2, 44.8 |

P-play LH `blend_heavy_1080p`, `PF1_RUNS=2` (of 1,800 due; no run late):

| Lane | on time | dropped | held max (ms) |
|---|---|---|---|
| A1 | 1789, 1771 | 11, 29 | 105.1, 105.5 |
| B1 | 1777, 1793 | 23, 7 | 85.1, 62.4 |
| A2 | 1793, 1790 | 7, 10 | 105.1, 105.1 |
| B2 | 1791, 1798 | 9, 1 | 85.1, 61.1 |

- **Verdict: B is not worse than A beyond A's own spread, so there is no bisect** (E12.13.5, "Owed", item 2).
- **G3's drop is the environment.** Identical code read ~71 fps under the screensaver (E12) and 42–65 fps here.
- **R37 improves held frames on an idle card.** A breaks G14 (held > 100 ms) in all four runs; B never does. S2b's
  earlier P-play LH passes were screensaver figures too.

#### E12.18.3 R47 gates (`bin-d1fa8fd`)

P-play, `PF1_RUNS=3` (of 1,800 due; `valid=true`, `passes=true`, `underrun_frames=0`,
`engine_sync_fallback_frames=0`, `engine_retire_overruns=0` and `engine_detained_fallback_frames=0` in every run):

| Lane, workload | on time | late | dropped | p50 / p95 present (ms) | p95/p50 | held max (ms) | clock stall max (ms) |
|---|---|---|---|---|---|---|---|
| LL `typical_1080p` | 1798, 1798, 1798 | 0, 0, 0 | 2, 2, 2 | 42.2 / 43.3 | 1.03 | 90.1, 85.1, 85.1 | 46.1, 45.9, 46.0 |
| LH `typical_1080p` | 1797, 1799, 1798 | 1, 1, 0 | 2, 0, 2 | 33.7 / 42.9 | 1.27 | 61.7, 46.5, 85.1 | 47.2, 45.9, 47.1 |
| LH `blend_heavy_1080p` | 1798, 1799, 1783 | 0, 0, 0 | 2, 1, 17 | 32.0 / 42.9 | 1.34 | 85.1, 54.4, 85.1 | 46.2, 47.1, 45.9 |

- **LH controls** (`typical_1080p`): Slowdown fails G1 (364 on time), Freeze fails G14, ClockFreeze fails G16, Stall
  fails `underrun_frames`. All four are caught.
- **G18** (`@G18 verdict`): every export hashes `fe87e67c33845ccdf6e62d1e4d576a2d5fc86e54cb48cc1f5f7f03dd2d979b88`
  (3,375,562 bytes), **IDENTICAL**. S0 wall 213.9 / 192.2 / 192.1 s, median 192.2 s; R47 36.9 / 36.6 / 36.8 s, median
  36.8 s, **−80.9%, PASS**.
- **G3 LH:** `typical_1080p` 44.0, 45.3, 47.8 fps; `blend_heavy_1080p` 49.7, 46.8, 45.7 (floor 60, exit 101);
  `heavy_4k` 28.8–28.9. The same as both A/B binaries.

#### E12.18.4 Verdicts

| Gate | Verdict | Evidence |
|---|---|---|
| G1 (`typical_1080p`, LH) | **Pass**: worst run 3 of 1,800 (≤ 18), p95 42.9 ms (≤ 50) | E12.18.3 |
| G6 (typical p95/p50 ≤ 3) | **Pass**: 1.03 LL, 1.27 LH | E12.18.3 |
| G14 (held ≤ 100 ms, LH) | **Pass**: max 85.1 ms over typical and `blend_heavy`; the freeze control fails it | E12.18.3 |
| G16 (clock stall ≤ 100 ms) | **Pass** on the measured lanes: max 47.2 ms; the clock-freeze control fails it | E12.18.3 |
| G17 (`sync_fallback_frames`) | **Pass on the measured lanes** (0); the other W workloads are owed | E12.18.5 |
| G18 (export ≤ S0 + 5%) | **Pass**, hashes identical | E12.18.3 |
| G3 (LH, 60 fps) | **Blocked (environment)**, Amendment R48; no binary passes on an idle unpinned card | E12.18.2 |

- **Margin to watch:** `blend_heavy_1080p` LH run 2 dropped 17 frames, one under G1's budget (`blend_heavy` is G2's
  criterion, due at S3b; recorded here because it is the workload that regressed in E12.13.5).

#### E12.18.5 Owed to the combined run after S2c

- P-play LL and LH for `blend_heavy_1080p` (LL), `explainer_16x9`, `reel_9x16`, `feed_4x5` and `talk_recut`, which
  completes G17 and G11 and gives G2 a first reading;
- P-play LL controls; P-seek LL and LH (G8, L-6); P-rss LL (G15, provisional; the pinned verdict stays S4's);
- I4 LL and LH.

## E13 S2c results

### E13.1 S2c-1: the drag probe and Amendment R49

The 30 Hz drag regressed at S2b (E12.10 D2). The brief's hypothesis was reader churn. A counting probe (not timing:
it needs no quiet machine) instrumented the P-seek drag on LL, drag only, three runs per workload, at load 13–17,
so its fps are indicative only. Its scratch patch (never committed), logs and runner are in
`s2c-logs/probe/` (`s2c1-probe.patch`, `probe-{base,finish,continue,both}.log.gz`, `run-probe.sh`). Variants:
`base` (ad8f896), `finish` (a paused wait is not abandoned for a newer paused job of its epoch), `continue` (a
naive forward continuation when c < t ≤ c + 12, without S-2's anchor and key-packet rules) and `both`. Ranges are
over the three runs:

| Variant | Workload | posts | jobs abandoned in wait / taken | published | decodes (req) | stale at delivery | seeks (fwd) | frames decoded / converted | opens / closes | retired / spawned | shrink + grow | reassigned | lookahead discarded | distinct fps | unanswered |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| base | seek_gop60 | 43–136 | 33–93 / 43–136 | 10–43 | 25–116 | 15–73 | 21–94 (21–94) | 811–2882 / 25–116 (≈23.0, 32.4, 24.8 per frame) | 0 / 0 | 0 / 0 | 0–0 | 0 | 0 | 1.6–8.6 | 0–4 |
| base | talk_recut | 150 | 109–121 / 150 | 28–40 | 108–118 | 76–84 | 88–99 (88–99) | 5546–5749 / 107–117 (≈47.8, 49.1, 52.9 per frame) | 0 / 0 | 0 / 0 | 0–0 | 0 | 0 | 5.6–8.0 | 1–37 |
| base | explainer_16x9 | 149–150 | 126–135 / 149–150 | 14–23 | 152–174 | 78–85 | 127–134 (125–132) | 6772–7377 / 151–173 (≈48.9, 39.1, 45.0 per frame) | 2 / 2 | 2 / 2 | 0–0 | 0–1 | 0 | 2.8–4.6 | 3–28 |
| finish | seek_gop60 | 129–132 | 0–1 / 129–133 | 128–131 | 129–132 | 0 | 100–106 (100–106) | 3094–3116 / 128–131 (≈23.6, 24.3, 24.0 per frame) | 0 / 0 | 0 / 0 | 0–0 | 0 | 0 | 25.6–26.2 | 0–2 |
| finish | talk_recut | 105–111 | 0 / 105–111 | 104–110 | 105–111 | 0 | 82–92 (82–92) | 4883–5341 / 104–110 (≈44.4, 49.5, 50.6 per frame) | 0 / 0 | 0 / 0 | 0–0 | 0 | 0 | 20.8–22.0 | 1–2 |
| finish | explainer_16x9 | 72–79 | 0 / 72–79 | 71–78 | 101–116 | 0 | 95–109 (93–107) | 5658–6128 / 100–116 (≈60.5, 48.8, 55.2 per frame) | 2 / 2 | 2 / 2 | 0–0 | 0–1 | 0 | 14.2–15.6 | 1–2 |
| continue | seek_gop60 | 148–150 | 0 / 148–150 | 148–150 | 148–150 | 0 | 0 (0) | 360–375 / 148–150 (≈2.5, 2.5, 2.4 per frame) | 0 / 0 | 0 / 0 | 0–0 | 0 | 0 | 29.6–30.0 | 0 |
| continue | talk_recut | 149–150 | 3–6 / 149–150 | 144–146 | 149–150 | 3–5 | 6 (6) | 634–776 / 149 (≈4.6, 4.3, 5.2 per frame) | 0 / 0 | 0 / 0 | 0–0 | 0 | 0 | 28.8–29.2 | 0 |
| continue | explainer_16x9 | 150 | 0–2 / 150 | 148–150 | 220–224 | 0–2 | 2–3 (0–1) | 575–599 / 220–224 (≈2.7, 2.7, 2.6 per frame) | 2 / 2 | 2 / 2 | 0–0 | 0–1 | 0 | 29.6–30.0 | 0 |
| both | seek_gop60 | 150 | 0 / 150 | 150 | 150 | 0 | 0 (0) | 360–375 / 150 (≈2.5, 2.5, 2.4 per frame) | 0 / 0 | 0 / 0 | 0–0 | 0 | 0 | 30.0 | 0 |
| both | talk_recut | 147–150 | 0 / 147–150 | 147–150 | 147–150 | 0 | 6 (6) | 634–776 / 147–150 (≈4.6, 4.2, 5.3 per frame) | 0 / 0 | 0 / 0 | 0–0 | 0 | 0 | 29.4–30.0 | 0–1 |
| both | explainer_16x9 | 149–150 | 0 / 149–150 | 149–150 | 218–224 | 0 | 2–3 (0–1) | 576–599 / 218–224 (≈2.7, 2.7, 2.6 per frame) | 2 / 2 | 2 / 2 | 0–0 | 0–1 | 0 | 29.8–30.0 | 0 |

- **Reader churn is refuted.** No decoder reopens, permit resizes, reassignments or discarded lookahead on
  `seek_gop60` and `talk_recut`; `explainer_16x9`'s two opens and retirements are its cutaway entering and leaving.
  S2a did not keep a warm decoder either: it sought on every paused render (`DecodeStrategy::Seek`, 3b221bd).
- **Cause 1, H-2 preemption:** 62–90% of taken paused jobs were abandoned in FrameWait by the next
  `request_frame`, and 40–68% of decodes were stale at delivery. `finish` restores S2a's rates (25.6–26.2,
  20.8–22.0 and 14.2–15.6 distinct fps; S2a 23.6, 18.8 and 14.6).
- **Cause 2, seek cost:** every non-+1 decode seeks and walks its GOP, ≈ 23–55 frames decoded per frame converted.
  `continue` reaches ~30 fps with 0–6 seeks per drag. S-2 is that fix, with its anchor and key-packet rules.
- **Prediction for the timing run:** S-2 abandons continuation at each key packet read before t, so against
  `continue`'s counts expect about 6 extra seeks per drag at GOP 60 (a drag walks ~375 frames) and 1–2 at GOP 250.
  P-seek's `drag_seeks` now reports it.
- **Ruling:** Amendment R49 (design §8), accepted by the lead on 2026-10-05 with the `RETIRE_DEADLINE` bound.
- **Witnesses.** Each mutation was applied alone and the four witnesses rerun by `s2c-logs/s2c1/mutate.sh`, in a
  throwaway worktree with its own `target/`. Two runs were made: at exactly caa6ef4 (`s2c-logs/s2c1/mut-*.log`)
  and at the follow-up commit (`s2c-logs/s2c1/followup/mut-*.log`; its `crates/` tree is
  da0c0858, since the commit was amended for this text only). Each log's first line records the commit and the
  only modified file. The mutations:

  - *R49 removed*: `view.newer` alone supersedes.
  - *deadline removed*: `!view.drag`.
  - *deadline wake removed*: the paused wait's timed wake, `else if false`.
  - *kept wait ignores the newest job*: `!view.young`.
  - *epoch unchecked*: the drag test's `job.stamp.epoch == stamp.epoch` (preview.rs:737 at caa6ef4, 763 after the follow-up) made `true`.

  | # | Witness | R49 removed | deadline removed | deadline wake removed | kept wait ignores the newest job | epoch unchecked |
  |---|---|---|---|---|---|---|
  | 1 | `preview::tests::a_drag_publishes_every_taken_paused_job` (29 posts at 30 Hz over two sources, each job's decodes held until the next post) | fails: `abandoned` (both runs) | passes | passes | passes | passes |
  | 2 | `preview::tests::a_stuck_drag_job_is_superseded_at_the_deadline` (R43's seam, `Stuck`) | fails: `superseded at 6.12 ms` / `6.05 ms` | fails: `superseded at the deadline` (never, 10 s) | fails: same | passes | passes |
  | 3 | `preview::tests::controls_still_supersede_a_paused_wait_at_once` (seek, pause, document, play, shutdown) | passes | passes | passes | fails: caa6ef4 `superseded late: seek 200.5 ms, pause 200.5 ms, document 200.7 ms`; follow-up `seek None, pause None, document None` (not within 5 s) | fails: caa6ef4 `superseded late: seek 201.0 ms`; follow-up `seek None, pause None` |
  | 4 | `sched::tests::a_drag_never_abandons_a_young_paused_wait` (every 6-event sequence, 8 events, drained) | fails: `a drag abandoned a young wait (Frame)` | fails: `kept past the deadline` | passes (the model has no wake) | fails: `Seek did not abandon the wait at once` | passes (the model is given `drag`) |

  Witness 3 guards L-6 against an over-broad R49, so removing R49 leaves it green. Its `play` and shutdown cases
  stay green under every mutation: R37's `played` and the shutdown flag supersede independently of R49's test.
  Witness 3's `seek` case is the deterministic catcher of the epoch mutation: a seek is one post of a newer
  epoch's paused job. `pause` and `document` post `None` and then the resting job, so whether they catch it
  depends on whether the waiter wakes between the two posts (it did not at caa6ef4, and did at the follow-up for
  `pause`). No new witness was needed.
- **Robustness to render speed (follow-up).** At caa6ef4, witnesses 1 and 3 measured R49 against the real 200 ms
  `RETIRE_DEADLINE`. Witness 1 holds each job's decodes until the next post (33 ms apart). A loaded CI or
  llvmpipe render whose decode runs past 200 ms after its gate opened would be abandoned legitimately, so
  witness 1 could flake. Witness 3 started its clock before the job was taken and allowed 200 ms, so a slow take
  plus a slow wake could flake it too. The follow-up adds a per-lane test drag deadline (`Lane::drag_deadline`,
  `cfg(test)`, read by `FrameWait::superseded` and the paused wait's timed wake). Production still uses
  `RETIRE_DEADLINE`. Witnesses 1 and 3 set it to 30 s:
  - In witness 1, "kept" holds while any render finishes within 30 s.
  - Witness 3 now times from the control, and "at once" means within 5 s, against a 30 s deadline. A control
    that wrongly waits out the deadline is still separated from one that supersedes at once by 25 s.

  Witness 2 keeps the real 200 ms because its assertion is a lower bound (`after >= RETIRE_DEADLINE`, a 10 s
  ceiling). Its render is held in its open by R43's `Stuck` seam, so render speed cannot shorten it, and load
  can only lengthen it within the 10 s ceiling. Witness 4 is a pure model and does not depend on time.
- **Counter scope (follow-up).** `PlaybackStats::reader_seeks` (P-seek's `drag_seeks`) counts the preview's
  reader-decoder seeks only. A K-3 synchronous fallback render's seeks are not counted, and its doc says so.
  `sync_fallback_frames` already reports when the fallback ran.
- **Budget (S2c-1, follow-up).** caa6ef4 adds 280 non-blank, non-comment source lines against the brief's ~150:
  about 54 production and about 226 test. The overrun is the witnesses: R49 is a one-predicate change in
  `paused_superseded` plus the deadline wake. Showing that it keeps drags, still yields to every control, and
  is bounded by the deadline took four witnesses, including witness 4's exhaustive sched model. The lead
  accepted the overrun on 2026-10-05. The follow-up commit adds 24 such lines, all of them test seam or test.

### E13.2 S2c-2: S-2 forward continuation

**Implementation.** Design §8 S-2's *Implementation [S2c-2]* paragraph describes it.
- A reader decodes a paused plan's frame with `VideoDecoder::decode_paused(cursor, t)`. A playback plan keeps
  `decode_window_sequential`.
- The decoder tracks, since its last seek, the first video packet, any later key packet and the timestamps.
  After each paused seek it compares the shadow's A(t0) with the real first packet and latches on a mismatch.
  Before continuing it reads A(t) from the shadow.
- Rule 1's stream test is read at open (`selects_stream`); rule 4's pair is `mov` with H.264.
- `receive_frames` checks the stop flag after each frame while `decode_paused` runs.
- The C-4 merge (9133530, from 1602761) needed four adapter edits, about 9 code lines:
  - the probe joins R43's stop field;
  - a test-only `set_stop`;
  - `Fixture::open` passes the new seventh argument;
  - a stop check before each packet read in `decode_cursor`.

  R43 had moved that check to the top of the loop. Without it, "cancel with frames waiting" read one more
  packet ("cancelled 5 packets in, frame 2 arrived at packet 4").
- `ContinuationPath` (`pf1_s2c_continuation.rs`) adapts the production call. Both of its witnesses are no
  longer ignored. The R46 guard test holds, as does the new open-time test
  `decode::tests::continuation_needs_the_witnessed_pair_and_the_seek_stream`. That test covers real files:
  plain, with audio, with a PNG cover (`attached_pic`, checked with ffprobe), two video streams, MPEG-4 Part 2
  in MP4, and H.264 in Matroska.

**Must-pass (C-4) mutations.** `s2c-logs/s2c2/mutate.py`, each mutation alone against 1a4787b's production code (this commit before its evidence was amended in; the `crates/` tree, 57ca297e, is the same).
Each run reruns the two `ContinuationPath` witnesses and the open-time test. Logs are in `s2c-logs/s2c2/mut-*.log`;
`pre-commit/` holds an earlier run on the uncommitted tree.

| Mutation | Fails | First failure line |
|---|---|---|
| per-frame stop check removed (R39 item 3) | scripted | `cancel with frames waiting x1: step 2: cancel left Observation { frame: Some(..), ... }` |
| key packets unchecked (rule 2) | scripted, oracle | `edit x1: step 1: route Continue, expected Seek`; `Default x1 Target { c: 32, t: 44, t2: Some(46), kind: "c+12" } then 46: route Continue, S-2 says Seek` |
| timestamps unchecked (rule 3) | scripted | `missing timestamp x1: step 3: route Continue, expected Seek` |
| A(t) not compared (rule 2) | scripted | `anchor drifts x1: step 4: route Continue, expected Seek` |
| real first packet not compared, no latch | scripted | `mismatch once, key change x1: step 6: route Continue, expected Seek` |
| latch removed | scripted | same |
| window c + 12 unbounded (rule 3) | oracle | `Default x1 Target { c: 31, t: 44, t2: Some(50), kind: "c+13 (outside)" }: route Continue, S-2 says Seek` |
| pair unchecked (rule 4) | oracle, open-time | `AviDtsGuess x1 Target { c: 87, t: 88, t2: Some(89), kind: "eof" }: route Continue, S-2 says Seek`; `s2-mpeg4: c + 1` |
| no reset on error | scripted | `error x1: step 3: route Continue, expected Seek` |
| stream test skipped (rule 1) | open-time | `s2-two-videos` left `(true, true)` |
| attached pictures counted as video (rule 1) | open-time | `s2-cover` left `(true, false)` |

Rule 1 is witnessed on real files only by the open-time test. The C-4 corpus injects `StreamMismatch`, which
bypasses `selects_stream`. That makes the stream rule **partial** at the C-4 level, and complete only with the
open-time test.

**Route band (R39 item 1).** The witnesses check the route against `Model::expected`, which applies rule 2 to the
key packets the decoder actually read. Every boundary case took the side below, and `s2c-logs/s2c2/route-band.log`
has every target. Unless noted, the cases hold for Default, Pyramid, EditList and Vfr at 1, 4 and 16 threads.

| Case | Side taken |
|---|---|
| c + 1 | Continue at 1 thread. Seek where a key lies within the decoder's read-ahead: at 4 threads 43 → 44 (key 48); at 16 threads also 12 → 13 (key 24) |
| c + 12 (32 → 44) | Continue at 1 thread; Seek at 4 and 16 (key 48 read before 44 is produced) |
| c + 13 | Seek |
| anchor boundary (key − 2, key − 1, key) | Seek, Seek, Continue (the first step past an anchor change continues on the new run) |
| boundary window, second hop across a key | Seek. Exception: Vfr at 1 thread continues each hop's first step |
| open GOP: before the key (22 → 23), at the key (23 → 24) | Continue at every thread count |
| end of file | Continue |
| AviDtsGuess (every case) | Seek (rule 4) |
| seeded random targets | Continue 9 / 6 / 2 of 20 on Default at 1 / 4 / 16 threads, and 0–10 of 20 across the corpus |

**Finding: frame threads make rule 2 bite before each key.** A frame-threaded H.264 decoder reads about `threads`
packets ahead of the frame it outputs. When a key packet lies within that distance after t, rule 2 abandons the
continuation even though the anchor for t is unchanged. Readers get up to 16 frame threads (H-5). A +1 drag
therefore seeks on about the last `threads` steps before each key, and each of those seeks decodes from the
previous key. The probe's prediction was about 6 extra seeks per drag at GOP 60 and 1–2 at GOP 250; it assumed
one exit per key. At 16 threads the expectation is about 16 seeks per GOP crossed. The timing run's `drag_seeks`
measures it. This is rule 2 as written, implemented exactly per the lead's ruling. A narrower rule, for example a
key packet whose frame presents at or before t, is a design question for the lead and was not adopted.

**R39 items 5 and 6.** NoPts covers rule 4 and DTS-guessed timestamps only (`AviDtsGuess`): the product refuses
raw streams without a probe. The ±1 tick qualification became the test author's raw-timestamp probes (R40):
`the_anchor_boundaries_hold_at_the_tick_level` and `a_shadow_boundary_one_tick_off_fails_the_tick_witness` pass.
The frame-grid boundary never shifts a frame.

**Memory.** Each reader that has rendered a paused frame keeps a second demuxer context (the shadow) with its own
copy of the index. P-rss's paired run records the cost.

**Fast tier.** The `pf1_s2c` module takes 338 s wall at 4 threads locally (`s2c-logs/s2c2/pf1_s2c-times.log`). The
longest tests are the test author's oracle and meta tests: 155, 130, 87, 77 and 67 s. The two witnesses this
commit enabled take 114 s (`continuation_reproduces_seek_on_every_fixture`) and 38 s. Both stay in the fast tier.
The first is the only witness that catches the window and pair mutations, so it falls under ci/slow-tests.txt's
C1-R1 rule (coverage over fast time). Moving the module's longest tests to the slow tier is left to the lead.

**Gates (1a4787b).** All of these passed:
- build, `clippy --workspace --all-targets -D warnings` (1.99) and `fmt --check`;
- `cargo build -p kinewright-app`;
- `cargo test -p kinewright-media`: lib 1010 passed, 56 ignored (415 s); au3 3 passed / 2 ignored; au5 9; generated_media 12 passed / 3 ignored; transcript_e2e 2;
- slow-tests lint.

At 816d0c1 (the merge plus the S2c-1 follow-up), the `decode::`, `pf1_s2c`, `preview::` and `sched::` tests
passed 125, with 2 ignored and a 6.2 GB peak RSS. The first attempt was SIGKILLed during a concurrent foreign
build, with systemd-oomd active, and the rerun passed.

**Lines (non-blank, non-comment, added).** About 428 in total, against S2c-2 and S2c-3's shared ~560:
- decode.rs 274, including the `cfg(test)` shadow-fault seams;
- render.rs 6 and preview.rs 5;
- pf1_s2c_witness.rs 43 (16 removed);
- pf1_s2c_continuation.rs 100.

### E13.3 S2c-3: S-3 bounded backward window

**Implementation (95faf28).** Design §8 S-3's *Implementation [S2c-3]* paragraph describes it.
- `Readers::backward` (sched.rs) takes a paused job's single-time sources. A source refills when t is below its
  last playhead and t's frame is not in the ring. B = clamp(⌊(C − G − H) / n / f⌋, 1, 16) and
  start = max(in-point, t − B + 1). [start, t) joins t's required times, so K-2 reserves the window.
- `ReaderDemand.in_points` (render.rs) carries each source's in-point: the media clip's `source_range.start`, or t
  for a freeze clip.
- `Readers::hold` keeps each window across posts while its source stays planned (`post` forgets the others). A step
  inside a held window, in either direction, is a hit and converts nothing. K-5 evicts the frames behind the travel
  first.
- `preview::backward_windows` runs only for a paused job (`wait.playback.is_none()`). The job's `paused_plan` then
  sends the reader through `decode_paused`, so the window decodes with one seek to the key ≤ start and continues
  under S-2.

**Witnesses.**
- `preview::tests::a_backward_drag_matches_fresh_seeks_and_refills_once_per_window` drives a real paused drag on a
  generated 160×90 file (GOP 30), along [40, 39, 38, 35, 31, 30, 29, 28, 20, 14, 13, 12, 5, 6]. It checks two things.
  - Every presented frame is byte-identical to a fresh-seek decode (C-5).
  - The reader seeks exactly at [40, 39, 29, 28, 12]: the first paused frame, then one refill per window crossed.
- `sched::tests::a_backward_step_refills_a_bounded_window_and_then_hits` covers:
  - B from the share, the clamp at 1 and at 16, and the in-point floor;
  - forward steps (no refill), hits that decode nothing, and K-2's set never exceeding C.
- `sched::tests::a_held_window_is_evicted_behind_the_travel_after_a_reversal` covers K-5 after a reversal.
- `preview::tests::only_a_paused_job_refills_a_backward_window`: a playback job holds no window.

**Mutations.** Each mutation alone against 95faf28 (`s2c-logs/s2c3/mutate.py`, logs `mut-*.log`, summary
`mutate-95faf28.out`). All 7 fail. The first run (`first-run/`, on the uncommitted tree) let `paused-only-removed`
survive. The fourth witness was added for it before the commit.

| Mutation | Failing tests | First failure line |
|---|---|---|
| hold ignored (a held window never hits) | drag, refill, reversal | `hit 37`; `[40, 39, 36]: the set does not fit beside the window` |
| share ignored (B always 16) | refill, reversal | `K-3: a set over C was posted` |
| in-point floor ignored | refill | `C 1000 G 0 floor 35: refill` |
| forward steps refill | refill | `floor 0: forward` |
| a hit drops the window | drag, refill, reversal | `left: None right: Some((31, 39))` |
| travel ignored in eviction | reversal, `eviction_follows_the_travel_direction` | `[40, 39, 36] left [31, 32] right [39, 38]`; the existing `[101] → 100` |
| paused-only check removed | `only_a_paused_job_refills_a_backward_window` | `paused false` |

**Gates.** L-4a (a hit) is gated by the timing run's P-seek and L-4b (a refill) is recorded there (E13.4); GOPs
over 250 are recorded, not gated (D4). Possible L-1 risk: a backward click-seek that misses the ring refills
up to 16 frames before t. The P-seek lanes show whether that costs L-1.

**R39 item 4: open GOP. Stopped, needs a ruling.** A fresh seek to an open-GOP leading frame yields no frame, and
`SourceSpec::decode` then returns `no_frame`. This is a product bug that predates PF1. A diagnostic (never
committed; `s2c-logs/s2c3/opengop-diag.txt`) decoded every frame of each C-4 fixture with a fresh one-frame
window at 1 thread:
- Default, Pyramid, EditList and Vfr: every frame is present.
- OpenGop: frames 21, 22, 23, 47 and 71 are missing (the leading frames before keys 24, 48 and 72 that a seek lands on).
- AviDtsGuess: frames 0, 1, 24, 25, 48, 49, 72 and 73 are missing.

The likely fix, prototyped (`s2c-logs/s2c3/opengop-retry-prototype.diff`), retries once. When the window misses
`start` and the first packet is a key past the stream start, it seeks to that key's dts − 1 and decodes again. That
recovers every OpenGop frame and AviDtsGuess 24/25/48/49/72/73; AviDtsGuess 0 and 1 stay missing, since no earlier
key exists. With the prototype, `pf1_s2c` failed 14 of 32 tests, all at `Truth::new`
(`pf1_s2c_witness.rs:512`, `assert_eq!(decoder.seek_count(), 1, "{:?}: the linear run seeked again")`), for OpenGop
(left 2) and AviDtsGuess (`s2c-logs/s2c3/opengop-proto-pf1_s2c.log`).

The brief allows regenerating the oracle from a fixed seek path, but that is not enough here:
- The C-4 oracle assumes a run's anchor is A(t), the shadow's seek result. A retry anchors the run one key earlier.
- `Truth`'s one-seek assertion would have to change, and so would S-2's anchor rule for a retried run: the prototype
  refuses continuation after a retry.
- That is an edit to the oracle's assertions and to rule 2, and the brief reserves both for the lead.

The behaviour stays pinned (continuation equals today's seek path) and the prototype was reverted. The proposed
amendment: a retried seek anchors at the earlier key, A(t) becomes that key for leading frames, and the test author
updates `Truth` and the model. The AviDtsGuess misses at 24/25 suggest the AVI case has a different cause
(DTS-guessed timestamps) and need their own look.

**Lines (non-blank, non-comment, added; 95faf28).** 252 in total: sched.rs 147 (47 production, 100 tests),
preview.rs 100 (37 production, 63 tests; 10 removed), render.rs 5. With S2c-2's ~428, S-2 and S-3 together come to
about 680, against ~560: 21% over, inside the 50% rule. Tests make up most of the excess: 163 of S2c-3's lines.

**Gates (95faf28 before the fourth witness was amended in).**
- `cargo test -p kinewright-media`: lib 1013 passed, 56 ignored (414 s); au3 3 passed / 2 ignored; au5 9;
  generated_media 12 passed / 3 ignored; transcript_e2e 2.
- clippy (1.99, `-D warnings`) clean; fmt clean; `cargo build -p kinewright-app` ok.

The final tree is covered by S2c-4's workspace gate (E13.4).

### E13.4 S2c-4: the combined timing run under Amendment R50 (2026-10-05/06)

S2c-4 ran the combined plan that R50 asks for: G8's P-seek lanes plus the lanes E12.18.5 still owed. Every lane is
paired against the last closed stage's binary (`ad8f896`), and G18 against S0, interleaved A B A B on the machine
as it was. A supplement then added a third binary, the candidate without its test-only conversion tap (E13.4.7). Kit,
plans, logs and binaries are in `target/review/pf/s2c-timing/` (logs gzipped). Load is recorded, not gated.

#### E13.4.1 Binaries, plans and conditions

Each binary is `cargo test --release -p kinewright-media --lib --no-run`, rustc 1.99.0, built in its own worktree
and target directory under `kr-s2c/` (deleted afterwards) before any timing started.

| Binary | Source | sha256 |
|---|---|---|
| `cand-abcd7cb` | `pf1/impl` `abcd7cb` (`crates/` clean) | `2f8375085318c73129dd9a7b637c249ef5250caf09ad1c410b4080f4f29d60a5` |
| `ref-ad8f896` | `ad8f896`, S2b's close | `6d52c2f9401d30930b41a3806ff4761e03b118a8a2a8b927655565ce95824a71` |
| `s0-d19bdf9-lane2aa4e1b` | `d19bdf9` + `pf1_export_lane.rs` at `2aa4e1b` + its `lib.rs` line | `5731d7ae0ad49926def6417e075bccc57ba8caaf57838bc95c0ff717c609ee75` |
| `count-ref-ad8f896` | `ad8f896` + the counting patch | `0025af649a57b721726289f6779854770f44471a9b454f71519da5f06af9b898` |
| `count-cand-abcd7cb` | `abcd7cb` + the counting patch | `0c3708204d5bd9fe386d05d8331c7eb83018898ab5a48601507b82e890e70b45` |
| `cand-notap-abcd7cb` | `abcd7cb` with C-4's `tap_conversion` call disabled (`if false && …`) | `9b77ba6c75aeb934b92fec072357930b631c0a89ee242ff989445f26102635e5` |

- **Counting patch** (`count/pf1_count.rs`, `count/apply-count.py`; `cfg(test)`, never committed): process-wide
  atomics for frames decoded (each successful `receive_frame`), frames converted (each `convert`) and seeks (each
  `seek_count` increment). P-seek prints them per phase (`count_{random,forward,backward,drag}_*`). A backward step
  that decoded nothing is a hit, so the backward phase splits into hits and refills, with each one's p95.
- **Plans:** `plan.txt` (131 lanes, 18:23:05 to 00:18:12 EDT) and `plan-notap.txt` (the supplement, from 00:21).
  The runner is `lanes3.sh`, R47's `lanes2.sh` adapted to R50: no quiet-wait, and contamination lines are annotations. Each lane runs alone, under the 5 s
  sampler (load, top five processes, GPU state).
- **Machine:** as E12.18.1. rustc 1.99.0, NVIDIA 615.71.09, llvmpipe LLVM 23.1.1.
- **Load record (annotation only, R50 item 4):** the 1-minute load at lane start ran from 3.4 to 36.9. Foreign
  processes in the top-five samples: other projects' `rustc` and nextest runs, `python`, `npm`, `chrome-headless`,
  `campfire`, `spotify` and `pipewire`. On LH lanes the RTX sat at P8 in 87% of samples (P0 8%).
- **One self-inflicted overlap:** a poll loop of mine (a `read` from `/dev/zero`, about one core) ran from 00:24:19
  to 00:33:49 during the supplement's `seek_gop60` `cand1` and `notap1` lanes. Its triple was run again at the end
  of the supplement (triple 3), and triple 1 is left out of E13.4.7's `seek_gop60` rows.

#### E13.4.2 G8: P-seek, paired (main run)

Six runs per side per cell: two R C R C pairs × three seeded runs each. Each cell is the median, with the range in
brackets. All ms, except the drag's distinct stamped frames per second (L-5).

| Lane, workload | Side | random p95 | forward p95 | +1 p95 | backward p95 (combined) | drag distinct fps | drag p95 | drag seeks | release (L-6) |
|---|---|---|---|---|---|---|---|---|---|
| LL `seek_gop60` | ref | 63.3 [57.6–87.1] | 63.2 | 19.0 [16.9–29.5] | 64.3 | 14.1 [10.6–14.6] | 354.9 | — | 6/6, 19.4 |
| LL `seek_gop60` | cand | 768.8 [701.9–876.6] | 69.3 | 68.7 [55.6–79.9] | 704.7 | 26.0 [24.6–26.8] | 105.8 | 38 [33–40] | 6/6, 19.1 |
| LL `talk_recut` | ref | 101.2 [87.5–177.5] | 90.6 | 18.5 | 90.5 | 7.5 [5.0–8.2] | ∞ (unanswered) | — | 6/6, 59.2 |
| LL `talk_recut` | cand | 652.0 [580.0–1011.6] | 86.2 | 94.3 | 595.6 | 27.1 [26.0–27.8] | 119.3 | 16.5 [12–23] | 6/6, 36.7 |
| LL `explainer_16x9` | ref | 153.7 | 140.1 | 25.1 | 149.9 | 2.8 [2.0–4.0] | 2877.1 | — | 6/6, 53.8 |
| LL `explainer_16x9` | cand | 491.5 | 105.7 | 120.0 | 558.2 | 27.1 [21.2–28.2] | 151.4 | 23 [18–25] | 6/6, 25.6 |
| LH `seek_gop60` | ref | 62.7 [61.1–68.5] | 65.4 | 23.2 [22.6–23.5] | 65.8 | 13.2 [11.4–14.6] | 332.8 | — | 6/6, 25.9 |
| LH `seek_gop60` | cand | 805.8 [707.3–837.8] | 71.2 | 70.2 [59.8–74.0] | 701.3 | 25.1 [24.8–25.6] | 114.4 | 37 [32–38] | 6/6, 30.4 |
| LH `talk_recut` | ref | 96.0 [84.0–107.5] | 94.0 | 23.2 | 89.8 | 6.7 [4.2–7.8] | ∞ (unanswered) | — | 6/6, 51.2 |
| LH `talk_recut` | cand | 676.2 [542.4–945.8] | 89.5 | 94.3 | 572.4 | 26.7 [25.8–27.8] | 131.4 | 16 [13–22] | 6/6, 43.1 |
| LH `explainer_16x9` | ref | 145.2 | 138.5 | 32.2 | 151.4 | 2.9 [2.0–4.0] | ∞ (unanswered) | — | 6/6, 79.5 |
| LH `explainer_16x9` | cand | 505.9 | 111.2 | 122.8 | 523.5 | 25.7 [25.2–27.4] | 156.7 | 21.5 [19–24] | 6/6, 48.6 |

- `drag_paused_abandoned` is 0 in 35 of the 36 candidate P-seek runs (1 in one LL `explainer_16x9` run), and at
  most 4 in the COUNT lanes. The reference harness predates `drag_seeks` and `drag_paused_abandoned`.
- `+1 p95` rests on 8–17 samples per run (`plus1_n`, median 11), so it is close to each run's maximum.
- L-4a and L-4b come from the COUNT lanes (E13.4.3): backward hits p95 15.2 [11.3–42.5] at GOP 60, 14.2 [11.6–26.8]
  on `talk_recut` and 21.1 [19.7–22.8] on `explainer_16x9`. Refills p95 904 [803–2101], 1048 [890–1245] and 1016
  [724–1246]. The reference has no hits: every backward step seeks, at a p95 of 80, 100 and 154.

| G8 item | Gate | Reference (LL / LH) | Candidate (LL / LH) | Verdict (R50 item 2) |
|---|---|---|---|---|
| L-1 random p95, GOP 60 | ≤ 40 | 63.3 / 62.7 | 768.8 / 805.8 | **Fail, against the candidate:** both miss, and the candidate is ×12 the reference, far beyond the pair spread |
| L-2 random p95, `talk_recut` | ≤ 110 | 101.2 / 96.0 (pass) | 652.0 / 676.2 | **Fail, against the candidate:** the reference passes in the same run |
| L-3 +1 p95, GOP 60 | ≤ 20 | 19.0 / 23.2 | 68.7 / 70.2 | **Fail, against the candidate:** the reference passes on LL |
| L-4a backward hit p95, GOP 60 | ≤ 20 | no hits | 15.2 (LL only) | **Pass on LL**; LH is not split (partial) |
| L-4b refill p95 | recorded | 79.6 (every step) | 904 (LL) | Recorded |
| L-5 drag distinct fps, GOP 60 / 250 | ≥ 10 / 7 | 14.1, 7.5 / 13.2, 6.7 | 26.0, 27.1 / 25.1, 26.7 | **Pass**, about ×2 and ×4 the reference |
| L-6 release shown | every run | 36/36 | 36/36 | **Pass** |

**G8 does not close at `abcd7cb`.** S2c fixes the drag (L-5, about 25–27 fps against 3–14) and holds L-6, but it
regresses random seeks ×5–12 and +1 steps ×3–5. The counters (E13.4.3) show why. The tap is not the cause (E13.4.7).

#### E13.4.3 Load-independent counters (COUNT lanes, LL, R50 item 3)

Six runs per side (R C R C, three seeded runs each), the same seeds as E13.4.2. Each phase is 200 operations,
except the drag (150 calls). Cells are medians [range]; "per conv." is frames decoded per frame converted.

| Workload | Phase | Ref: seeks | Ref: decoded / converted (per conv.) | Cand: seeks | Cand: decoded / converted (per conv.) |
|---|---|---|---|---|---|
| `seek_gop60` | random | 200 | 6015 / 200 (30.1) | 753 [692–828] | 31200 / 1699 (18.4) |
| `seek_gop60` | forward (+1…+12) | 191 [187–195] | 5761 / 203 (28.4) | 115 [107–119] | 4855 / 233 (20.8) |
| `seek_gop60` | backward (−1…−12) | 200 | 6004 / 200 (30.0) | 428 [416–439] | 18746 / 1088 (17.2) |
| `seek_gop60` | drag | 106 [99–110] | 3191 / 140 (22.7) | 40 [31–42] | 1760 / 130 (13.5) |
| `talk_recut` | random | 200 | 14040 / 200 (70.2) | 334 [324–414] | 31206 / 1469 (21.2) |
| `talk_recut` | forward | 190 [184–193] | 12256 / 201 (61.0) | 61 [53–62] | 6048 / 201 (30.1) |
| `talk_recut` | backward | 200 | 13192 / 200 (66.0) | 225 [188–226] | 24005 / 1096 (21.9) |
| `talk_recut` | drag | 90 [84–102] | 5656 / 109 (51.9) | 18 [12–24] | 1886 / 134 (14.1) |
| `explainer_16x9` | random | 307 [301–309] | 19777 / 307 (64.4) | 477 [450–532] | 37962 / 2272 (16.7) |
| `explainer_16x9` | forward | 281 [277–287] | 19331 / 298 (64.9) | 72 [71–75] | 6340 / 352 (18.0) |
| `explainer_16x9` | backward | 301 [301–303] | 19784 / 301 (65.7) | 304 [290–308] | 25712 / 1607 (16.0) |
| `explainer_16x9` | drag | 132 [119–135] | 6864 / 160 (42.9) | 24 [21–26] | 1904 / 196 (9.7) |

Backward split (candidate): 132 [130–133] hits and 68 [67–70] refills at GOP 60; 125 [122–127] and 75 [73–78] on
`talk_recut`; 113 [111–118] and 87 [82–89] on `explainer_16x9`. The reference refills all 200.

What the counters show:
- **The drag is fixed by mechanism.** Seeks per drag fall from 90–132 to 18–40, and frames decoded per frame
  converted from 23–52 to 10–14. This is S-2 continuing forward, with R49 keeping the render in flight.
- **Forward steps cost less in total**: about 40–75% fewer seeks and 15–67% fewer frames decoded. The forward p95 is
  equal or lower (E13.4.2).
- **Random seeks are the regression.** The candidate converts 7–8.5 frames per random seek, against 1. About half
  of all random targets fall below the last playhead and outside the ring. S-3 treats each of these as a backward
  step and refills a window of up to B = 16 frames before t. Each refill then decodes its window frame by frame
  through `decode_paused(cursor, at)`. That is S-2 continuation with a one-frame cache (`SourceSpec::decode`), and it
  re-seeks wherever rule 2 trips. Rule 2 abandons at any key packet read, and with 16 frame threads the decoder reads
  up to 16 packets ahead. So the window's frames near the next key seek again: about 6 seeks and about 270 frames
  decoded per refill at GOP 60. E13.3 named the L-1 risk ("a backward click-seek that misses the ring refills up to
  16 frames"); the counters confirm it.
- **The +1 regression is rule 2 under frame-thread read-ahead.** At `ad8f896`, a +1 step after a rendered frame
  reused the decoder through the old `continuation_at == start` path and never sought. At `abcd7cb`, a +1 step whose
  read-ahead reaches the next key (about `threads` positions before each key, 16 of 60 at GOP 60) abandons and
  seeks. With about 11 +1 samples per run, one such seek sets the p95. **Partial:** this is inferred from the
  forward phase's seek count and the p95's size (a seek plus a GOP walk), not counted per +1 step.

#### E13.4.4 P-play, paired (main run)

`PF1_RUNS=1` per lane, R C R C R C, so three runs per side, in run order. Each row lists on time, dropped,
present p95 (ms), held max (ms), clock stall max (ms), peak RSS (MiB) and `passes`. Every one of the 72 runs had
`valid=true`, `underrun_frames=0`, `engine_sync_fallback_frames=0`, `engine_retire_overruns=0`,
`engine_detained_fallback_frames=0` and `engine_stale_errors=0`.

| Lane, workload | Side | on time | dropped | p95 | held max | clock stall max | peak RSS | passes |
|---|---|---|---|---|---|---|---|---|
| LL `typical_1080p` | ref | 1798, 1798, 1798 | 2, 2, 2 | 43.3, 43.2, 43.2 | 85.0, 85.1, 85.1 | 46.3, 47.5, 48.4 | 982.2, 992.9, 939.0 | 3/3 |
| LL `typical_1080p` | cand | 1747, 1793, 1796 | 52, 7, 4 | 43.4, 43.2, 43.2 | 100.1, 90.6, 150.1 | 51.7, 49.6, 47.9 | 1070.0, 1039.0, 1054.5 | 1/3 |
| LL `blend_heavy_1080p` | ref | 1782, 1779, 1774 | 18, 21, 25 | 43.1, 43.0, 43.1 | 95.6, 85.3, 95.2 | 49.3, 51.6, 47.8 | 786.7, 774.9, 774.0 | 1/3 |
| LL `blend_heavy_1080p` | cand | 1798, 1793, 1731 | 2, 7, 69 | 43.0, 43.1, 43.3 | 85.0, 145.5, 105.1 | 49.4, 49.8, 49.8 | 869.6, 856.8, 848.8 | 1/3 |
| LL `explainer_16x9` | ref | 1798, 1798, 1798 | 2, 2, 2 | 43.3, 43.3, 43.3 | 85.0, 90.1, 85.1 | 47.2, 46.3, 46.1 | 1386.5, 1157.0, 1185.4 | 3/3 |
| LL `explainer_16x9` | cand | 1794, 1798, 1798 | 6, 2, 2 | 43.2, 43.2, 43.3 | 116.5, 95.1, 85.1 | 53.1, 46.5, 46.7 | 1304.1, 1402.0, 1540.7 | 2/3 |
| LL `reel_9x16` | ref | 1768, 1798, 1332 | 31, 2, 461 | 43.3, 43.2, 74.5 | 144.0, 85.1, 514.8 | 51.1, 46.8, 62.2 | 1238.1, 1395.0, 971.4 | 1/3 |
| LL `reel_9x16` | cand | 1798, 1444, 1750 | 2, 355, 49 | 43.2, 64.8, 43.4 | 85.1, 902.1, 98.2 | 55.3, 78.4, 49.1 | 1413.5, 1224.3, 1238.7 | 1/3 |
| LL `feed_4x5` | ref | 1678, 1769, 1542 | 120, 30, 258 | 63.8, 43.2, 64.4 | 127.6, 124.7, 154.3 | 47.9, 46.9, 58.9 | 1186.0, 1475.6, 1242.0 | 0/3 |
| LL `feed_4x5` | cand | 1654, 1640, 1690 | 146, 159, 110 | 63.9, 64.0, 63.6 | 168.2, 90.1, 85.2 | 54.4, 49.4, 46.8 | 1494.2, 1698.7, 1551.9 | 0/3 |
| LL `talk_recut` | ref | 7173, 7111, 7166 | 27, 87, 33 | 43.3, 43.2, 43.3 | 701.1, 1257.9, 941.2 | 51.8, 49.7, 51.7 | 782.9, 776.8, 768.0 | 0/3 |
| LL `talk_recut` | cand | 7198, 7150, 7050 | 2, 49, 150 | 43.3, 43.3, 43.3 | 85.5, 1016.3, 1695.8 | 47.3, 60.0, 46.8 | 832.2, 853.9, 850.9 | 1/3 |
| LH `typical_1080p` | ref | 1798, 1773, 1675 | 2, 27, 125 | 42.7, 43.0, 63.7 | 85.1, 85.1, 85.1 | 46.5, 48.9, 50.5 | 1023.8, 1002.8, 1042.9 | 1/3 |
| LH `typical_1080p` | cand | 1789, 1720, 1518 | 11, 80, 282 | 42.8, 43.5, 64.4 | 85.1, 85.1, 142.5 | 47.6, 51.5, 49.9 | 1130.5, 1112.0, 1124.3 | 1/3 |
| LH `blend_heavy_1080p` | ref | 1362, 1450, 1717 | 438, 350, 82 | 64.7, 64.6, 43.2 | 84.9, 85.1, 635.0 | 48.6, 56.5, 68.3 | 829.6, 837.4, 825.2 | 0/3 |
| LH `blend_heavy_1080p` | cand | 1629, 1792, 1779 | 171, 8, 21 | 64.1, 43.1, 43.0 | 85.1, 109.7, 110.9 | 48.5, 52.1, 63.9 | 895.8, 896.7, 874.4 | 0/3 |
| LH `explainer_16x9` | ref | 1615, 1792, 1753 | 185, 8, 47 | 64.1, 43.2, 43.3 | 130.0, 85.0, 85.1 | 68.8, 51.4, 50.6 | 977.2, 1262.0, 1367.6 | 1/3 |
| LH `explainer_16x9` | cand | 1756, 1779, 1758 | 44, 21, 42 | 43.3, 43.3, 43.3 | 185.1, 135.7, 85.1 | 52.2, 55.8, 49.0 | 1024.6, 1135.3, 1096.4 | 0/3 |
| LH `reel_9x16` | ref | 1651, 1643, 1643 | 149, 157, 157 | 64.0, 64.0, 64.1 | 85.1, 118.4, 85.5 | 50.4, 48.8, 50.3 | 1339.7, 1258.8, 1417.9 | 0/3 |
| LH `reel_9x16` | cand | 1638, 1645, 1645 | 162, 155, 155 | 64.0, 64.0, 64.0 | 90.1, 85.1, 85.1 | 51.7, 49.8, 53.4 | 1374.2, 1360.4, 1357.6 | 0/3 |
| LH `feed_4x5` | ref | 1204, 1240, 1258 | 596, 558, 542 | 84.9, 84.8, 84.7 | 416.9, 99.4, 88.9 | 48.9, 47.7, 51.2 | 1426.6, 1300.4, 1342.7 | 0/3 |
| LH `feed_4x5` | cand | 1228, 1229, 1201 | 572, 569, 599 | 84.7, 85.0, 84.8 | 115.8, 113.0, 93.9 | 47.9, 49.0, 48.1 | 1452.2, 1680.8, 1539.0 | 0/3 |
| LH `talk_recut` | ref | 7178, 7181, 7065 | 21, 19, 134 | 43.4, 43.4, 43.4 | 416.7, 464.0, 1415.0 | 51.7, 48.0, 74.1 | 838.2, 851.9, 845.1 | 0/3 |
| LH `talk_recut` | cand | 7163, 7150, 6956 | 37, 50, 244 | 43.4, 43.4, 43.4 | 1001.2, 1402.6, 1959.3 | 49.4, 60.0, 58.5 | 913.1, 912.8, 894.2 | 0/3 |

- **G17 and G11 are complete:** `sync_fallback_frames` and underruns are 0 on all six workloads, LL and LH,
  both binaries.
- **G6** (p95/p50 ≤ 3): the worst run is 2.01 (LH `feed_4x5`, 42.1 / 84.7 ms). Pass.
- **G16** (clock stall ≤ 100 ms): the worst run is 78.4 ms. Pass.
- **G1 and G14 on LH `typical_1080p`:** the reference passes G1 in 1 of 3 runs (dropped 2, 27, 125) and the
  candidate in 1 of 3 (11, 80, 282). That is a shared miss, *environment-limited at load 5.0–9.9* (the 1-minute
  load at these six lanes' starts; the supplement's LH `typical_1080p` lanes started at 10.6–40.5). G14: the reference
  holds 85.1 ms in all three, and the candidate holds 85.1, 85.1 and **142.5**. By R50 item 2 that one miss counts
  against the candidate binary as built. E13.4.7 attributes it to the test-only tap, which a product build does not
  contain.
- **G2's first reading** (`blend_heavy_1080p`, LH): dropped 438, 350, 82 (reference) and 171, 8, 21 (candidate).
- **Peak RSS** is 3–25% higher in the candidate on every workload. E13.4.7 attributes this to the tap too.
- `talk_recut` held max is 417–1959 ms on both binaries, LL and LH: its GOP 250 cuts. It is not a G14 workload.

#### E13.4.5 Controls (candidate only, `typical_1080p`)

Every Q-3 control fails its metric on both lanes:

| Control | LL | LH | Metric failed |
|---|---|---|---|
| Slowdown | 374 on time, 1353 dropped | 285 on time, 1432 dropped | G1 |
| Freeze | held max 1406.5 ms | held max 1171.1 ms | G14 |
| ClockFreeze | clock stall max 1050.0 ms | clock stall max 1050.0 ms | G16 |
| Stall | 48,128 underrun frames | 48,128 underrun frames | `underrun_frames` (G11) |

#### E13.4.6 P-rss (G15), I4, G3 and G18 (main run)

**P-rss LL** (R C R C, one child process per workload; settled idle, MiB / threads):

| Workload | S0 (E9.5) + 4 | ref1 | cand1 | ref2 | cand2 | Candidate ≤ S0 + 4 |
|---|---|---|---|---|---|---|
| `typical_1080p` | 588.8 | 440.2/49 | 480.7/49 | 467.1/49 | 482.4/49 | yes |
| `blend_heavy_1080p` | 681.2 | 470.9/49 | 491.4/49 | 453.8/49 | 465.7/49 | yes |
| `explainer_16x9` | 550.4 | 437.4/49 | 469.3/49 | 440.7/49 | 467.1/49 | yes |
| `reel_9x16` | 496.5 | 383.2/49 | 399.5/49 | 391.7/49 | 397.5/49 | yes |
| `feed_4x5` | 406.7 | 384.3/49 | 397.3/49 | 393.1/49 | 394.8/49 | yes |
| `talk_recut` | 385.8 | 359.0/49 | 373.6/49 | 373.9/49 | 375.8/49 | yes |
| `title_only` | 240.0 | 228.1/49 | 234.0/49 | 234.3/49 | 226.9/49 | yes |

- **G15 passes provisionally** (unpinned; the pinned verdict stays S4's). The tightest margins are `talk_recut`
  (382.3 MiB at most over all candidate runs, the supplement included, against 385.8) and `title_only` (234.0
  against 240.0).
- Settled-idle thread count is 49 everywhere, as at S2b.
- The candidate settles −7 to +41 MiB from the reference in the same pair (median +14). The supplement puts this
  on the tap (E13.4.7).

**I4** (`r28_end_to_end_tracked`, mean ms over three runs per lane; ME13's 5% rule is against `BASELINES`):

| Lane | ref1 | cand1 | ref2 | cand2 | Delta vs baseline (candidate) | Paired median ratio (cand / ref), mean / p95 |
|---|---|---|---|---|---|---|
| LL | 74.7–75.4 | 123.8–130.4 | 86.8–92.1 | 142.5–165.0 | −74.6%, −68.5% | **1.77** / 1.86 |
| LH | 96.5–112.0 | 153.0–179.4 | 104.0–116.8 | 159.6–186.6 | −66.6%, −65.1% | **1.55** / 1.70 |

The test's own 5% rule passes (every run is far below its baseline). R50 item 1 instead judges I4 on the paired
ratio, and the candidate binary fails that by 55–77%. E13.4.7 shows the tap causes it: without it the ratio is 1.02.

**G3 LH** (reported, not gated, R48; every binary exits 101 against its 60 fps floor). Median fps over each lane's
three runs, ref1 / cand1 / ref2 / cand2: `typical_1080p` 41.3 / 36.2 / 40.2 / 33.8; `blend_heavy_1080p` 43.7 / 32.8
/ 38.5 / 29.4; `heavy_4k` 27.5 / 25.4 / 25.5 / 26.1. The paired median ratio is 0.76 on `blend_heavy_1080p`
and 0.86 on `typical_1080p`. The tap explains most of it (E13.4.7).

**G18 LH** (R38's 120-frame export; S0 against the candidate, S C S C): every export hashes
`fe87e67c33845ccdf6e62d1e4d576a2d5fc86e54cb48cc1f5f7f03dd2d979b88` (3,375,562 bytes), **identical** (C-5). S0
255.0 / 262.1 s, candidate 79.4 / 78.8 s: **paired median ratio 0.31, pass** (≤ 1.05). R47 read 0.19 on a quiet
machine (E12.18.3). Both S0 and the candidate are slower under this load. The candidate's extra cost is the tap
(E13.4.7). `g18_verdict.py` printed INCOMPLETE because it expects lane names with a separate `S0` word. The verdict
above is computed by hand from the same lines.

#### E13.4.7 The supplement: the candidate without its test-only conversion tap

**Why.** Every timing binary is a `cargo test --release --lib` binary, so `cfg(test)` code is compiled in. C-4's
`DecoderProbe::tap_conversion` (`decode.rs`) copies every RGBA64 conversion into `last_conversion` with a
`flat_map`/`collect`. At 1080p that is a 16.6 MB allocation and copy per converted frame, and the retained copy
stays resident. `ad8f896` has no tap. A product build has none either. `cand-notap-abcd7cb` is `abcd7cb` with
only the tap call disabled. `plan-notap.txt` interleaves ref / cand / notap (00:21:15 to 02:45:23 EDT, load at lane
start 6.3–40.5).

P-seek LL (two triples per workload, three seeded runs each; `seek_gop60` uses triples 2 and 3, E13.4.1):

| Workload | Side | random p95 | forward p95 | +1 p95 | backward p95 | drag distinct fps | drag seeks |
|---|---|---|---|---|---|---|---|
| `seek_gop60` | ref | 66.4 [61.7–69.7] | 67.8 | 21.8 | 67.7 | 9.9 [7.2–12.6] | — |
| `seek_gop60` | cand | 780.8 [696.6–2783.1] | 66.2 | 70.1 | 702.0 | 26.1 [25.6–26.8] | 38.5 |
| `seek_gop60` | notap | 734.5 [698.3–830.0] | 64.3 | 65.6 | 692.9 | 26.7 [25.0–26.8] | 39.5 |
| `talk_recut` | ref | 101.8 [88.5–123.3] | 95.0 | 20.8 | 97.8 | 5.2 [4.2–8.6] | — |
| `talk_recut` | cand | 688.8 [562.0–1055.5] | 96.1 | 99.4 | 622.7 | 26.4 [25.2–27.6] | 15.5 |
| `talk_recut` | notap | 792.2 [591.2–1202.8] | 89.5 | 87.5 | 539.0 | 26.7 [24.6–28.0] | 16.0 |

P-play (`PF1_RUNS=1`; LL two triples per workload, LH `typical_1080p` two and `talk_recut` one; columns as E13.4.4):

| Lane, workload | Side | on time | dropped | p95 | held max | clock stall max | peak RSS | passes |
|---|---|---|---|---|---|---|---|---|
| LL `typical_1080p` | ref | 1798, 1798 | 2, 2 | 43.2, 43.2 | 85.1, 105.1 | 47.9, 48.0 | 1039.6, 940.5 | 1/2 |
| LL `typical_1080p` | cand | 1780, 1798 | 19, 2 | 43.2, 43.1 | 170.1, 85.1 | 53.9, 49.7 | 1059.6, 1041.3 | 1/2 |
| LL `typical_1080p` | notap | 1792, 1755 | 8, 45 | 43.2, 43.2 | 90.1, 115.1 | 56.7, 52.3 | 966.6, 928.2 | 1/2 |
| LL `feed_4x5` | ref | 1709, 1561 | 89, 238 | 48.7, 64.2 | 92.4, 786.2 | 48.2, 50.1 | 1280.2, 1286.4 | 0/2 |
| LL `feed_4x5` | cand | 1594, 1419 | 206, 380 | 64.2, 64.7 | 100.5, 141.6 | 47.3, 49.8 | 1525.2, 1581.1 | 0/2 |
| LL `feed_4x5` | notap | 1612, 1511 | 186, 288 | 64.1, 64.5 | 97.1, 142.6 | 49.9, 48.8 | 1337.9, 1359.1 | 0/2 |
| LH `typical_1080p` | ref | 1539, 1606 | 261, 194 | 64.4, 64.1 | 85.2, 113.1 | 56.1, 53.2 | 1041.5, 1010.7 | 0/2 |
| LH `typical_1080p` | cand | 960, 1547 | 840, 253 | 85.3, 64.3 | 171.2, 113.0 | 77.4, 55.9 | 1082.3, 1129.3 | 0/2 |
| LH `typical_1080p` | notap | 1551, 1692 | 249, 108 | 64.4, 63.6 | 85.2, 85.3 | 55.0, 57.3 | 1020.3, 998.2 | 0/2 |
| LH `talk_recut` | ref | 7112 | 87 | 43.4 | 1272.0 | 56.6 | 752.7 | 0/1 |
| LH `talk_recut` | cand | 7077 | 123 | 43.4 | 2896.8 | 55.3 | 815.4 | 0/1 |
| LH `talk_recut` | notap | 7157 | 43 | 43.4 | 884.7 | 57.5 | 844.6 | 0/1 |

Other lanes (one triple each, except I4 with two):

| Lane | ref | cand | notap | notap / ref |
|---|---|---|---|---|
| P-rss LL settled idle, `typical_1080p` / `explainer_16x9` / `talk_recut` (MiB) | 457.7 / 451.9 / 371.2 | 476.6 / 468.3 / 382.3 | 456.5 / 459.7 / 376.5 | −1.2 / +7.8 / +5.3 MiB |
| P-rss LL playing peak, `typical_1080p` / `feed_4x5` (MiB) | 990.2 / 1040.6 | 1033.2 / 1188.1 | 981.3 / 1013.4 | 0.99 / 0.97 |
| I4 LL mean (median of three runs), triple 1 / 2 (ms) | 80.3 / 74.4 | 144.6 / 123.6 | 81.3 / 76.8 | **1.01 / 1.03, median 1.02** |
| I4 LL p95, triple 1 / 2 (ms) | 231.6 / 202.9 | 445.0 / 360.1 | 229.5 / 210.9 | 0.99 / 1.04 |
| G3 LH fps, `typical` / `blend_heavy` / `heavy_4k` (median of three) | 45.9 / 46.1 / 29.3 | 40.2 / 37.7 / 27.4 | 43.2 / 43.1 / 28.3 | 0.94 / 0.93 / 0.97 |
| G18 LH wall, against S0's 267.4 s (s) | — | 79.6 | 53.0 | S0 ratio: cand 0.30, notap **0.20** |

Every G18 export in the supplement hashes `fe87e67c…d2d979b88` too. P-rss across all seven workloads, notap minus ref
is −2.8 to +7.8 MiB, and cand minus ref +1.9 to +18.9.

**What the supplement shows:**
- **The P-seek regressions are S2c's, not the tap's.** notap matches cand within the spread on every P-seek metric:
  random p95 ×11 / ×7.8 the reference, +1 p95 ×3 / ×4. The drag gain is S2c's too.
- **The I4, G3, G18, peak-RSS and P-play held/dropped differences are the tap's.** Without it, I4's paired ratio is
  1.02 (≤ 1.05, **pass**). G18 is 0.20 of S0, as R47 found on a quiet machine. Settled idle is within 8 MiB of the
  reference, playing peak at or below it, and G3 is within 3–7% on one pair. On LH `typical_1080p`, notap holds 85.2 and
  85.3 ms where cand held 171.2 and 113.0 (G14), and it drops the fewest frames of the three binaries.
- **Consequence for the timing kit:** a timing binary must not carry the tap. The proposed fix is to gate the copy
  behind a switch only C-4's witnesses turn on (for example an `AtomicBool` the probe sets), so a release test
  binary pays nothing. This changes only test code. It is left for the lead, because the tap belongs to C-4's
  witness harness.

#### E13.4.8 The three exit-139 lanes: the known exit-time teardown race

Three P-seek lanes in the main run exited with SIGSEGV: LL `talk_recut` cand2 (19:40:31), LH `seek_gop60` cand1
(20:02:32) and LH `explainer_16x9` **ref1** (20:28:20). Each had already printed all three runs and `test result:
ok`, so its data is complete and is used above. The cores' `coredumpctl info` output is kept in
`s2c-logs/s2c4/sigsegv-{221921,678585,1183491}-*.txt.gz`. All three cores show the same picture as E12.8's P-rss
crash:
- The main thread is in libc `exit()`, running the NVIDIA libraries' exit handlers (`libGLX_nvidia`,
  `libnvidia-glcore`, `libnvidia-glvkspirv`).
- A preview thread is still dropping its `FrameRenderer`'s wgpu device (`libnvidia-glcore`, `vkDestroyDevice`).
- The engine's worker thread is in `Thread::join`, joining that preview.

**Cause:** P-seek's `seek_run` (`pf1_harness.rs`) lets its `Session` drop at the end of the function. P-play
(line 562) and P-rss (line 984) instead call `teardown(session)`, which waits up to 30 s for
`FfmpegMediaEngine::finished` before dropping the GPU context. So libtest can reach `exit()` while the engine's
threads are still destroying their Vulkan device. The reference crashed as well, and every crash came after the
test passed, so this is the harness and the driver's exit ordering. It is not S2c. **No rerun was needed.**
Proposed harness fix (not made, as `seek_run` is shared timing code and the stage is closed for timing): end
`seek_run` with `teardown(session)` as P-play does. The product-side gap E12.8 named, `FfmpegMediaEngine` having
no `Drop` that joins its worker, is unchanged.

#### E13.4.9 Verdicts and what S2c needs

| Gate | Verdict | Evidence |
|---|---|---|
| G8 L-1 (random p95 ≤ 40, GOP 60) | **Fail, against S2c** (×11–12 the reference; the reference also misses, at 63) | E13.4.2, E13.4.7 |
| G8 L-2 (random p95 ≤ 110, `talk_recut`) | **Fail, against S2c** (the reference passes at 96–102) | E13.4.2 |
| G8 L-3 (+1 p95 ≤ 20, GOP 60) | **Fail, against S2c** (the reference passes on LL at 19.0) | E13.4.2 |
| G8 L-4a (backward hit p95 ≤ 20) | **Pass on LL** by the median at GOP 60 (15.2; one of six runs at 42.5) and `talk_recut` (14.2; range to 26.8); LH not split (**partial**) | E13.4.3 |
| G8 L-4b (refill) | Recorded: p95 0.9–1.0 s | E13.4.3 |
| G8 L-5 (drag ≥ 10 / 7 fps) | **Pass**: 25–27 fps (reference 3–14) | E13.4.2 |
| G8 L-6 (release shown) | **Pass**, 36/36 | E13.4.2 |
| G1 (LH `typical_1080p`) | Shared miss, *environment-limited*: load at lane start 5.0–9.9 (main run), 10.6–40.5 (supplement); notap drops the fewest | E13.4.4, E13.4.7 |
| G6 (p95/p50 ≤ 3) | **Pass**, worst 2.01 | E13.4.4 |
| G14 (held ≤ 100 ms, LH typical) | **Pass without the tap** (85.1–85.3). The cand binary misses once per run set (142.5, 171.2), and the reference once in the supplement (113.1) | E13.4.4, E13.4.7 |
| G16 (clock stall ≤ 100 ms) | **Pass**, worst 78.4 | E13.4.4 |
| G17 / G11 | **Pass** on all six workloads, LL and LH; the E12.18.5 debt is cleared | E13.4.4 |
| G15 (settled idle ≤ S0 + 4, provisional) | **Pass** on all seven workloads | E13.4.6 |
| I4 (paired 5% rule) | **Pass without the tap** (1.02); the cand binary 1.55–1.77 | E13.4.6, E13.4.7 |
| G18 (≤ S0 + 5%, bytes identical) | **Pass**: 0.31 (cand), 0.20 (notap); every hash identical | E13.4.6, E13.4.7 |
| G3 (LH, 60 fps) | Reported, blocked by R48; notap within 3–7% of the reference | E13.4.6, E13.4.7 |
| Q-3 controls | Each fails its metric on LL and LH | E13.4.5 |

**S2c does not close at `abcd7cb`: G8's L-1, L-2 and L-3 fail, and the cause is S2c's.** The drag is fixed
(L-5), and nothing else regressed once the tap is excluded, on the supplement's evidence. That rests on n = 2 notap
runs per cell, and LL `typical_1080p` notap dropped 8 and 45 frames against the reference's 2 and 2, so the claim is
provisional until a 3-pair re-time (E13.5 settles it). The fixes below are design changes, so they go back to
the lead and are not made here:
1. **L-1/L-2: refill only on steps.** S-3 refills when t is below the last playhead and not in the ring. A random
   click-seek backward meets that test, so it pays a window of up to 16 frames. Proposal: refill only when t is
   within 12 frames below the last *paused target* (the L-4 step domain), and otherwise seek as `ad8f896` did.
2. **The window costs about 6 seeks per refill.** Each window frame is a separate `decode_paused(cursor, at)` with
   a one-frame cache, and rule 2 re-seeks near the next key under 16-thread read-ahead. Options: decode the window
   in one pass (`decode_window(start, t)`, one seek, converting each frame as it presents); or narrow rule 2 to key
   packets that present at or before the target; or decode paused jobs with one frame thread.
3. **L-3: +1 steps.** The same read-ahead makes a +1 step about `threads` frames before a key abandon and seek,
   where `ad8f896` reused the decoder. Narrowing rule 2 as in item 2 would fix both. Whether a one-thread paused
   decoder costs L-5 is to be measured.
4. **Timing kit:** gate the C-4 tap behind a witness-only switch (E13.4.7), and give `seek_run` a `teardown`
   (E13.4.8).

#### E13.4.10 Open GOP (R39 item 4): options for the lead

E13.3 records the stop and its diagnostic. The options are:
- **(a) Keep it pinned** (today). Continuation equals today's seek path, including its missing open-GOP leading
  frames (`no_frame`). This is a product bug that predates PF1, and it stays open.
- **(b) Adopt the earlier-key retry** (`s2c-logs/s2c3/opengop-retry-prototype.diff`). A window that misses `start`
  after landing on a key past the stream start seeks again to that key's dts − 1. A retried run anchors at the
  earlier key, and A(t) becomes that key for leading frames. This needs an amendment to S-2's anchor rule, and the
  C-4 author has to update `Truth`'s one-seek assertion and the model, then regenerate the oracle. It recovers
  every OpenGop miss and AviDtsGuess 24/25/48/49/72/73.
- **(c) Investigate AviDtsGuess on its own.** Its misses at 0/1 cannot be recovered (no earlier key), and its
  misses at 24/25 point to DTS-guessed timestamps rather than open GOP.

**Recommendation:** (b) for OpenGop, done by the C-4 test author together with the S-2 anchor amendment, after the
G8 fixes in E13.4.9 (they touch the same paused path). Do (c) separately. Until then, (a) holds.

#### E13.4.11 Tests, gates and lines

- **Workspace** (`cargo test --workspace`, fast tier, at `abcd7cb`'s `crates/` tree; S2c-4 changes only docs).
  36 test binaries, all ok, including:
  - media lib 1014 passed, 56 ignored (417 s);
  - app 790 passed, 4 ignored;
  - core 310 passed, 1 ignored;
  - agent 628 passed, 1 ignored; `mcp_server` 76 passed, 10 ignored.

  Log: `s2c-logs/s2c4/workspace-test.log.gz`.
- **Gates** (`s2c-logs/s2c4/gates.log`):
  - rustc 1.99.0;
  - `cargo clippy --workspace --all-targets -- -D warnings` clean;
  - `cargo fmt -- --check` OK;
  - `python3 scripts/slow_tests.py lint`: "slow-test manifest and markers agree: 47 tests …; 44 allowlist entries
    well-formed".
- **Logs:** S2c-2's mutation logs are now gzipped (`s2c-logs/s2c2/mut-*.log.gz`, named `mut-*.log` in E13.2).
- **Lines:** S2c-4 is documentation only: design §8 gains Amendment R50 (as given by Riel and the lead), and this
  section is added.

### E13.5 The S2c fix round: R51, R52, the tap gate, teardown and the R50 re-time (2026-10-06)

The lead's rulings after E13.4 (Amendments R51 and R52, the test-only tap gate, the P-seek teardown, open GOP (b)
as S2c-5, and the evidence wording fixes) were taken in the ruled order. The paired re-time ran on `d116e92`
against `ad8f896`. Two diagnostic runs followed: the playback counters (r3) and the backward-step means (r4). Kit,
plans, logs and binaries are in `target/review/pf/s2c-timing/r2`, `r3` and `r4`. Load is recorded, not gated.

#### E13.5.1 What changed

| Commit | Change | Lines (+/−) |
|---|---|---|
| `f967bae` | R51 oracle, alone and first: C-4's `Model::expected` uses the anchor test only; the key-packet rules go | +29/−94 (test) |
| `700b729` | R51: rule 2 is the anchor test alone. `key_read` and the key branch of `on_packet` are removed | +9/−15 |
| `de7a00f` | R52: a backward paused target refills only when `last − B < t < last` (`Readers::backward`); two new witnesses | +120/−9 |
| `3d1d96d` | S-2 rule 4: the `mov` pair excludes fragmented MP4 (`unfragmented_mov`: `moov` read, no `mvex`) | +81/−3 |
| `8a5c7bf` | The C-4 tap copies only while a witness holds `Tap` (`CONVERSION_TAP`); the output oracle fails without it | +57/−8 |
| `9f50a23` | `seek_run` ends with `teardown(session)`, as P-play and P-rss do | +6/−2 (test) |
| `8db4303` | Design: S-2 rule 2 and rule 4, S-3's refill and exclusions, Amendments R51 and R52 | +37/−12 (docs) |
| `905b231` | E13.4 wording: the n = 2 claim, the load range stated once per run, L-4a's 42.5 ms run | +7/−4 (docs) |
| `d116e92` | Witness: a conversion without the tap clears the last record | +21 (test) |

- About 56 production lines are added (`decode.rs` about 49, most of them `unfragmented_mov`; `sched.rs` 7). The
  rest is tests and the harness.
- **Agent renders (R52).** An agent thumbnail goes through `run_agent`, a synchronous Seek. It never posts to the
  readers, so it neither refills nor moves the travel memory. The new preview witness
  `a_backward_jump_seeks_alone_and_an_agent_render_moves_no_travel` shows this.
  - With paths [59, 40, 39, 38], the reader seeks at 59, 40 and 39: the jump to 40 seeks alone, and 39 is a step
    and refills.
  - With [40, agent thumbnail at 35, 39, 38], the reader seeks at 40 and 39 only.
  - The bytes match the reference renderer (C-5), and the ring is unchanged by the agent render.

#### E13.5.2 Mutations

**R51** (`s2c-logs/s2c7/r51-mutate.txt`, at `700b729`). The witnesses are
`continuation_reproduces_seek_on_every_fixture` and `continuation_holds_the_scripted_cases`.

| Mutant | Result |
|---|---|
| `key-abandons`: reverts `700b729`, so any key packet read abandons | Killed (2 of 2 fail) |
| `shadow-unchecked`: continues without `shadow_anchor(t) == run` | Killed (2 of 2) |
| `first-unchecked`: drops `s2.first == s2.run` | Survives. **Equivalent:** `paused` sets the `disabled` latch whenever the seek's real first packet differs from the shadow's, so `!disabled && run.is_some()` already implies it |

**R52** (`r52-mutate.txt`, at `de7a00f`, the `sched` and `preview` witnesses). This includes the lead's two
`backward_windows` mutations.

| Mutant | Killed by |
|---|---|
| `bound-removed`: any `t < last` refills | `only_a_step_within_b_of_the_last_time_refills`, `a_backward_jump_seeks_alone_…` |
| `bound-off-by-one`: `t <= last − B` becomes `t < last − B` | `only_a_step_within_b_of_the_last_time_refills` |
| `extend-removed`: `required.extend(times)` dropped | `only_a_paused_job_refills_a_backward_window`, `a_backward_jump_seeks_alone_…`, `a_backward_drag_matches_fresh_seeks_and_refills_once_per_window` |
| `set-removed`: the `*set` accounting dropped | `only_a_paused_job_refills_a_backward_window` (now asserts `set == before + f·|window|`) |

**Fragmented MP4** (`frag-mut-*.log`). Two mutants are both killed by
`continuation_needs_the_witnessed_pair_and_the_seek_stream`:
- `gate-removed`: the pair no longer checks `unfragmented_mov`;
- `mvex-ignored`: the walker no longer looks for `mvex`.

These mutants, and the R52 ones above, first ran on uncommitted working trees whose diff matched the later commit.
After review, both fragmented-MP4 mutants were rerun on the committed tree at `118895a` (which includes `3d1d96d`), and
both are killed again (`frag-mut-{mvex-ignored,gate-removed}-118895a.log`; the unmutated run passes,
`frag-clean-118895a.log`).

The test now covers three files:
- `frag_keyframe+empty_moov` gives (pair, stream) = (false, true);
- `frag_keyframe` gives (false, true);
- `+faststart` gives (true, true).

**C-4 rerun** (`mutate.py`; `mutate-9f50a23.out`, `mutate-tap-rerun.out`). The run covers the 15 decode-side
mutants: the 12 earlier ones (R51 swaps `keys-unchecked` for `key-abandons`) and three for the tap gate. Each
reruns the ContinuationPath witnesses, the open-time pair test and the output-oracle tests.
- At `9f50a23`, 14 of 15 were killed. `tap-stale-kept` survived (a conversion without the tap kept the previous
  record), because no witness used a decoder across tap states.
- `d116e92` adds `a_conversion_without_the_tap_clears_the_last_record`. Rerun at `d116e92`, `tap-stale-kept` and
  `tap-never-copies` exit 101. **All 15 are killed.**
- `the_output_oracle_fails_without_the_conversion_tap` shows the oracle **fails** ("a returned frame without its
  conversion") rather than skipping.

#### E13.5.3 S2c-5, open GOP (b): stopped before any commit

The ruling allowed four pieces, each in its own commit before the implementation:
- an anchor-rule note in S-2;
- `Truth`'s one-seek assertion;
- the model;
- the regenerated oracle.

A prototype (`s2c-logs/s2c7/s2c5-prototype.diff`, never committed) shows (b) needs more than that, so I stopped
as the ruling says. The prototype works like this:
- `decode_window` seeks again, to just before the landed key's DTS, when the first decoded frame presents after
  `start` and the first packet is a key past the stream start.
- The retried run has no anchor.
- `decode_paused` skips rule 2 after a retry.

What the prototype shows:
- OpenGop's CLI check matches 89 of 89 frames, with 0 unreachable.
- The AVI timestamp-guess mismatches against the CLI fall from 7 to 1 (`s2c5-avi-cli-{base,proto}.log`).
- The pins change for OpenGop and AviDtsGuess.

What (b) needs beyond the ruling:
1. **The positive control.** `ReferenceContinuation` (C-4's reference implementation) must also skip the rule-2
   latch after a retried seek. Without that, 13 witnesses fail at the pin check (`s2c5-proto.log`).
2. **An oracle assertion.** `the_oracle_covers_the_special_targets` asserts "Rule 1's premise: the real seek's
   first packet is A(t)". That is false for a retried target by construction. It would need an exception, and that
   means editing an oracle assertion.
3. **A second oracle assertion.** `the_seek_path_matches_the_cli_reference` asserts `unreachable > 0` exactly on
   OpenGop. It would become `unreachable == 0` everywhere. That is a strengthening, but it is still an assertion
   edit.
4. **AviDtsGuess retries too** (at 24, 25, 48, 49, 72 and 73). The retry lives in the shared `decode_window`, so
   (c), which the ruling keeps out of S2c, comes into scope. There are two ways forward:
   - `Truth`'s exact seek count needs a retry predictor for AVI. The prototype's model predicate is "A(t) is a key
     that is not the first key, and its first frame presents after t". It holds for OpenGop, but AVI's
     DTS-guessed stamps have not been shown to follow it.
   - Or the retry is limited to the S-2 pair (`mov`/H.264), which leaves the AVI misses as they are.
5. **Product scope.** `decode_window` serves every Seek render: preview, agent thumbnails and playback region
   starts. So (b) changes returned frames outside S-2 (a `no_frame` error becomes a frame). These are new frames,
   not changed bytes, but the design text should say so.

Questions for the lead:
- (i) May the C-4 positive control and the two assertions above be edited, with the reviewer checking them
  against the S-2 note?
- (ii) Should the retry cover every file, with AVI in scope, or the S-2 pair only?

(a) holds until then.

#### E13.5.4 Binaries, plans and conditions

Each binary was built as `cargo test --release -p kinewright-media --lib --no-run` with rustc 1.99.0, in its own
worktree and target directory under `kr-s2c/`, before any timing started.

| Binary | Source | sha256 |
|---|---|---|
| `cand-d116e92` | `pf1/impl` `d116e92` (tap off unless a witness holds it) | `bb7c6630f3ef0393af9eff21870197177d2f9b0f7b7dbfd960d79b4c374bdc84` |
| `ref-ad8f896` | reused from E13.4 | `6d52c2f9401d30930b41a3806ff4761e03b118a8a2a8b927655565ce95824a71` |
| `s0-d19bdf9-lane2aa4e1b` | reused (G18's S0) | `5731d7ae0ad49926def6417e075bccc57ba8caaf57838bc95c0ff717c609ee75` |
| `count-cand-d116e92` | `d116e92` + the counting patch (E13.4.1) | `aa804966552cc09b6f0e834de75e371d153970ff71e97afb2aed1e4fb60767ea` |
| `count-ref-ad8f896` | reused | `0025af649a57b721726289f6779854770f44471a9b454f71519da5f06af9b898` |
| `count2-cand-d116e92` | `d116e92` + counting v2 (r4: `count/apply-count2.py`) | `7104781a63a28975ece23b28c155f377a2bd181a346f1cc38afa8b25ecf08ba4` |
| `count2-ref-ad8f896` | `ad8f896` + counting v2 | `9ba7ac32e9cae885b911d74478b935618d05402dfafc2050b3e571e6aa20451c` |

Counting v2 adds, per P-seek run:
- the means of backward hits, refills and the whole backward phase;
- the random and forward means;
- the frames decoded and converted inside refills only.

The plans:
- **r2, the re-time** (`r2/plan-r2.txt`, 118 lanes, 04:38 to 08:49 EDT). Every lane is paired and interleaved,
  reference first:
  - COUNT P-seek and P-seek, LL and LH, three workloads, three pairs each;
  - P-play `typical_1080p` and `feed_4x5`, LL and LH, three pairs;
  - I4, LL and LH, three pairs;
  - G3 LH, two pairs (reported only);
  - G18 LH, S0 against the candidate, three pairs.
- **r3, the playback diagnosis** (18 lanes, 09:10 to 09:33). LH `typical_1080p` ran as three COUNT pairs, then
  three plain pairs **in reverse order** (candidate first). LL `feed_4x5` ran as three COUNT pairs.
- **r4, the L-4 means** (36 lanes, 09:41 to 11:25). COUNT2 P-seek, LL and LH, three workloads, three pairs.

Conditions:
- **Load record (annotation only):** the 1-minute load at lane start was 1.9–22.5 in r2 (median 8.2),
  2.1–16.4 in r3 and 6.5–38.6 in r4.
- Foreign processes in the samples: other projects' `rustc`, `clippy-driver` and `campfire` nextest runs, `cc1`,
  `git` and `python`. r2 wrote 4,226 contamination annotations.
- On r2's LH lanes the RTX sat at P8 in 86% of samples (P0 9%).
- None of my cargo commands ran during any timing lane. The r4 binaries were built between r3 and r4.

#### E13.5.5 G8: P-seek, paired (r2)

Each cell is the median over a side's runs (n = 8–9: three pairs × three seeded runs), with the range in brackets.
Values are ms, except the drag's distinct stamped frames per second (L-5). The last column is the median per-pair
ratio, cand/ref, for random p95 · +1 p95 · backward combined p95.

| Lane, workload | Side | random p95 | forward p95 | +1 p95 | backward p95 (combined) | drag distinct fps | drag seeks | release (L-6) | pair ratio |
|---|---|---|---|---|---|---|---|---|---|
| LL `seek_gop60` | ref | 63.2 [55.7–70.8] | 64.7 | 19.1 [14.7–22.0] | 63.7 | 12.7 [7.6–17.0] | — | 8/8 | |
| LL `seek_gop60` | cand | 61.4 [57.8–66.7] | 33.7 | 16.6 [16.0–21.2] | 157.6 | 30.0 [30.0–30.0] | 6 | 8/8 | 0.92 · 0.79 · 2.43 |
| LL `talk_recut` | ref | 86.2 [80.6–102.7] | 81.4 | 16.1 | 84.3 | 8.0 [6.8–9.2] | — | 9/9 | |
| LL `talk_recut` | cand | 93.6 [83.5–115.7] | 57.8 | 17.2 | 160.6 | 29.8 [29.6–30.0] | 7 | 9/9 | 1.11 · 1.05 · 1.86 |
| LL `explainer_16x9` | ref | 149.1 | 134.2 | 23.3 | 145.4 | 3.9 [2.2–5.6] | — | 8/8 | |
| LL `explainer_16x9` | cand | 150.9 | 36.7 | 26.7 | 199.5 | 30.0 [29.8–30.0] | 8 | 8/8 | 1.00 · 1.09 · 1.34 |
| LH `seek_gop60` | ref | 71.2 [67.1–78.6] | 72.1 | 25.7 [23.6–27.0] | 73.2 | 8.0 [5.2–13.2] | — | 9/9 | |
| LH `seek_gop60` | cand | 74.3 [63.3–99.8] | 46.1 | 26.6 [22.9–34.7] | 201.0 | 30.0 [29.8–30.0] | 6 | 9/9 | 1.03 · 0.97 · 2.67 |
| LH `talk_recut` | ref | 103.6 [85.2–121.7] | 93.7 | 23.4 | 92.4 | 7.0 [4.8–9.0] | — | 9/9 | |
| LH `talk_recut` | cand | 90.4 [84.5–115.7] | 63.2 | 23.4 | 151.6 | 29.8 [29.6–30.0] | 7 | 9/9 | 1.02 · 1.00 · 1.64 |
| LH `explainer_16x9` | ref | 145.6 | 139.3 | 32.6 | 152.1 | 3.8 [2.2–4.6] | — | 9/9 | |
| LH `explainer_16x9` | cand | 157.2 | 47.0 | 36.0 | 213.2 | 29.8 [29.4–30.0] | 8 | 8/8 | 1.05 · 1.11 · 1.39 |

- `drag_paused_abandoned` is 0 in every candidate run.
- The COUNT lanes agree with the table above. LL `seek_gop60`: random p95 60.4 against 58.5, +1 p95 17.4 against
  17.4. LH: +1 p95 20.4 against 23.0.

| G8 item | Gate | Reference (LL / LH) | Candidate (LL / LH) | Verdict (R50 item 2) |
|---|---|---|---|---|
| L-1 random p95, GOP 60 | ≤ 40 | 63.2 / 71.2 | 61.4 / 74.3 | **Shared miss, environment-limited at load 1.9–22.5.** The pair ratios are 0.92 and 1.03, inside the pair spread (0.89–1.08) |
| L-2 random p95, `talk_recut` | ≤ 110 | 86.2 / 103.6 | 93.6 / 90.4 | **Pass** (pair ratios 1.11 and 1.02) |
| L-3 +1 p95, GOP 60 | ≤ 20 | 19.1 / 25.7 | 16.6 / 26.6 | **Pass on LL. LH is a shared miss** (pair ratio 0.97) |
| L-4a backward hit p95, GOP 60 | ≤ 20 | no hits | 11.4 / 17.4 (COUNT lanes, r2: a plain lane cannot tell a hit from a refill) | **Pass on LL and LH** |
| L-4b refill p95 | recorded | 58.9 / 63.1 (every step seeks) | 161.1 / 159.7 | Recorded; E13.5.7 |
| L-5 drag distinct fps, GOP 60 / 250 | ≥ 10 / 7 | 12.7, 8.0 / 8.0, 7.0 | 30.0, 29.8 / 30.0, 29.8 | **Pass**, ×2.5–7.9 the reference |
| L-6 release shown | every run | 52/52 | 51/51 | **Pass** |

**G8 passes at `d116e92`.** R51 and R52 removed the regressions in L-1, L-2 and L-3 (E13.4: ×5–12 and ×3–5).
Random and +1 steps are now within the pair spread of the reference. The drag holds at 30 fps. L-4a passes on
both lanes.

#### E13.5.6 Load-independent counters (COUNT lanes, r2)

These are medians over 7–9 runs per side, the same seeds as E13.5.5. The counters are identical on LL and LH for
the same binary and workload.

| Workload | Phase | Ref: seeks | Ref: decoded / converted | Cand: seeks | Cand: decoded / converted |
|---|---|---|---|---|---|
| `seek_gop60` | random | 200 | 6015 / 200 | 200 [198–200] | 6015 / 215 |
| `seek_gop60` | forward | 191 | 5761 / 203 | 28 [26–31] | 1359 / 203 |
| `seek_gop60` | backward | 200 | 6004 / 200 | 86 [86–91] | 3200 / 1088 |
| `seek_gop60` | drag | 108 | 3222 / 142 | 6 [6–7] | 373 / 151 |
| `talk_recut` | random | 200 | 14040 / 200 | 200 [199–200] | 14040 / 200 |
| `talk_recut` | forward | 190 | 12256 / 201 | 35 [28–36] | 2909 / 201 |
| `talk_recut` | backward | 200 | 13192 / 200 | 100 [94–100] | 7145 / 984 |
| `talk_recut` | drag | 94 | 5954 / 118 | 7 [7–11] | 690 / 150 |
| `explainer_16x9` | random | 307 | 19777 / 307 | 304 [300–306] | 19576 / 366 |
| `explainer_16x9` | forward | 281 | 19331 / 298 | 28 [26–32] | 2499 / 298 |
| `explainer_16x9` | backward | 301 | 19784 / 301 | 131 [127–137] | 9009 / 1607 |
| `explainer_16x9` | drag | 133 | 7159 / 161 | 8 [8–10] | 595 / 224 |

- **Random seeks are back to one seek each.** The candidate converts 1.0–1.2 frames per target, against 7–8.5 at
  `abcd7cb`. The few extra conversions are R52 steps that fall within B of the last time.
- **Forward steps** need 82–90% fewer seeks and 76–87% fewer decoded frames. That is R51: a key packet in the
  read-ahead no longer ends a run.
- **Drag:** 6–8 seeks per 150 calls, against 94–133.
- **Backward:** 132 hits and 68 refills at GOP 60 (111/89 on `talk_recut`, 113/87 on `explainer_16x9`). The phase
  needs about half the reference's seeks and decoded frames, but 4.9–5.4× its conversions (E13.5.7).

#### E13.5.7 L-4: backward steps, hits against refills (r4)

Per side, nine runs (three pairs × three seeded runs) of the P-seek backward phase: 200 steps of −1…−12 from a
random start. A step that decoded nothing is a hit. Cells are medians, in ms. The pair ratio is cand/ref per
pair, median over the three pairs. The reference refills every step (one seek), so its step is its "refill".

| Lane, workload | Ref step: mean / p95 | Cand backward: mean / p95 | Pair ratio: mean / p95 | Cand hits: mean / p95 (n) | Cand refills: mean / p95 (n) | Per refill: decoded / converted (ref per step) |
|---|---|---|---|---|---|---|
| LL `seek_gop60` | 44.1 / 62.4 | 55.6 / 166.0 | **1.26** / 2.73 | 10.4 / 13.8 (132) | 139.8 / 173.0 (68) | 46.0 / 16.0 (30.0 / 1.0) |
| LL `talk_recut` | 57.7 / 91.0 | 57.1 / 155.8 | **1.02** / 1.83 | 9.9 / 12.3 (111) | 113.3 / 182.7 (89) | 80.3 / 11.0 (66.0 / 1.0) |
| LL `explainer_16x9` | 89.0 / 159.7 | 90.7 / 254.0 | **1.00** / 1.46 | 15.9 / 26.7 (113) | 186.3 / 312.6 (87) | 96.9 / 18.5 (98.5 / 1.5) |
| LH `seek_gop60` | 48.7 / 65.6 | 56.9 / 170.0 | **1.20** / 2.61 | 13.0 / 18.3 (132) | 144.0 / 183.0 (68) | 46.0 / 16.0 (30.0 / 1.0) |
| LH `talk_recut` | 69.1 / 111.8 | 67.5 / 185.3 | **0.88** / 1.64 | 12.6 / 16.8 (111) | 132.6 / 214.0 (89) | 80.3 / 11.0 (66.0 / 1.0) |
| LH `explainer_16x9` | 84.4 / 151.8 | 74.0 / 198.7 | **0.89** / 1.29 | 15.1 / 24.6 (113) | 149.2 / 217.7 (87) | 97.3 / 18.5 (98.4 / 1.5) |

- r4's load reached 38.6. Two pairs carry one-sided spikes: LL `explainer_16x9` pair 1 (ratio 2.38) and LH
  `talk_recut` pair 1 (0.29). The medians set them aside.
- r4 repeats L-4a: hit p95 at GOP 60 is 13.8 (LL) and 18.3 (LH), a pass. `explainer_16x9` hits reach a p95 of
  24.6–26.7 ms (two sources per frame). It is not an L-4a workload.

**What the backward cost is.**
- **The mean is about even except at GOP 60.** The backward phase's mean is 0.88–1.02× the reference's on
  `talk_recut` and `explainer_16x9`, and 1.20–1.26× at GOP 60.
- **The p95 is 1.3–2.7×**, because the p95 is the refill.
- A refill costs 1.8–3.2× a reference step: 113–186 ms against 44–89.
- **Where a refill's time goes, at GOP 60:**
  - Against a reference step, it decodes 16 more frames: 46 against 30, because a quarter of the windows
    start before the key and walk the earlier GOP.
  - It converts 15 more frames: 16 against 1.
  - The reference's step is 44 ms for 30 decoded frames and one conversion, so decoding costs at most about
    1.2–1.5 ms a frame (an upper bound: that 44 ms also includes the seek and the one conversion). The 16 extra
    decodes are therefore at most about 20–24 ms of the 96 ms difference. **The conversions are the remaining
    ~70–75 ms, about 5 ms each.**
  - `talk_recut` gives the same rate: 55 ms for 14 more decodes and 10 more conversions, about 4 ms per
    conversion.
  - **Partial:** this split is inferred from the counters and the means. Conversion time was not measured per
    frame.
- **Under −1 stepping** (arrow keys, the case S-3 is for), one refill buys 15 hits. Modelled from these means, a
  GOP 60 step costs (139.8 + 15 × 10.4) / 16 ≈ 18 ms on average, against the reference's 44 ms. The price is one
  stall of about 170 ms (p95) every 16 steps. **Partial:** this is modelled, not measured; P-seek's mix is
  −1…−12.

**Proposal (no change made).** Keep B, and stop converting the window before t is published:
1. **Preferred: publish t first.** A refill is held up by its 15 eager conversions, about 75 ms. Two ways to move
   them off the critical path:
   - the reader converts t first and publishes it, then converts [start, t) while the user looks at t. This
     needs a short-lived run of decoded (YUV) frames: about 3 MB a frame at 1080p 4:2:0, against 16.6 MB for
     the RGBA64 working frame;
   - or [start, t) is posted as the paused job's lookahead rather than its required times. This is simpler, but
     the window costs a second seek and a GOP walk in the background.

   Either way, a refill's latency should fall to about a reference seek plus the extra decodes: roughly
   44 + 20 ≈ 65 ms at GOP 60, against 140 today. The hits are unchanged.
2. **Fallback: shrink B to 8.** That halves the conversions, for a refill of roughly 44 + 10 + 35 ≈ 90 ms. Hits
   halve too: under −1 stepping the mean rises to about (90 + 7 × 10.4) / 8 ≈ 20 ms, and a stall comes every 8
   steps instead of every 16. B = 8 also no longer meets R52's "B = the window's actual size ≤ 16" rationale for
   large frames.

Both change S-3's text ("[start, t) joins t's required times", B's formula), so they are the lead's call.

#### E13.5.8 P-play, and the playback diagnosis

**r2** (`PF1_RUNS=1`, R C × 3, in run order). Every run had:
- `valid=true` and `underrun_frames=0`;
- `engine_sync_fallback_frames=0`, `engine_retire_overruns=0` and `engine_stale_errors=0`.

| Lane, workload | Side | dropped | present p95 | held max | lookahead starved | regions folded | peak RSS | load at start |
|---|---|---|---|---|---|---|---|---|
| LL `typical_1080p` | ref | 2, 2, 2 | 43.3, 43.2, 43.3 | 85.1, 85.7, 85.1 | 2403, 2370, 2420 | 66, 66, 66 | 981.7, 998.3, 924.5 | 5.8, 7.9, 5.6 |
| LL `typical_1080p` | cand | 2, 2, 2 | 43.3, 43.2, 43.3 | 85.0, 85.1, 85.1 | 2340, 2318, 2382 | 66, 66, 66 | 931.9, 968.6, 953.7 | 4.7, 6.8, 3.6 |
| LL `feed_4x5` | ref | 58, 105, 179 | 43.3, 63.6, 64.1 | 85.3, 84.6, 116.4 | 296, 264, 239 | 0 | 1247.5, 1362.8, 1436.4 | 3.4, 7.8, 9.6 |
| LL `feed_4x5` | cand | 136, 87, 164 | 64.0, 46.8, 64.1 | 111.5, 107.6, 122.3 | 278, 272, 238 | 0 | 1402.1, 1253.6, 1347.4 | 7.9, 9.6, 9.0 |
| LH `typical_1080p` | ref | 30, 19, 111 | 43.0, 42.9, 63.5 | 85.1, 85.1, 112.0 | 2389, 2468, 2386 | 66, 64, 63 | 1010.4, 1013.6, 1044.1 | 8.4, 7.6, 9.4 |
| LH `typical_1080p` | cand | 126, 163, 336 | 63.9, 64.1, 64.6 | 85.1, 113.3, 85.1 | 2332, 2296, 2048 | 59, 59, 57 | 1032.8, 994.4, 1018.1 | 4.7, 8.0, 22.3 |
| LH `feed_4x5` | ref | 591, 646, 521 | 84.6, 85.0, 66.9 | 98.4, 96.5, 89.1 | 209, 193, 199 | 0 | 1458.7, 1185.1, 1168.3 | 16.0, 13.4, 12.4 |
| LH `feed_4x5` | cand | 633, 562, 647 | 85.0, 84.9, 84.9 | 94.0, 112.3, 85.2 | 190, 217, 206 | 0 | 1282.6, 1239.1, 1177.9 | 16.7, 12.6, 13.0 |

- **LL `typical_1080p` is settled.** It dropped 2, 2 and 2 frames on both binaries, held 85 ms, and passed 3 of 3
  on both. The n = 2 notap reading in E13.4.7 (8 and 45 dropped) does not reproduce.
- **r2 also showed two possible regressions:**
  - LH `typical_1080p` dropped more frames in all three pairs (126, 163 and 336 against 30, 19 and 111), with
    present p95 at 64 against 43;
  - LL `feed_4x5` held max exceeded G14's 100 ms in 3 of 3 pairs, against 1 of 3 for the reference.

**What S2c changes on the playback path (by reading `ad8f896..d116e92`).** S-2 keeps `decode_window_sequential`
for playback: `SourceSpec::decode(…, paused)` takes `paused = state.paused_plan`, which a playback post sets false.
`backward_windows` returns nothing for playback. `FrameWait::superseded` and `wait_ready` are unchanged for a
playback wait (R49 applies to paused waits only). What remains on the playback path:
- per packet: `Continuation::on_packet` (a counter and, on packet 1, one tuple);
- per frame: `Continuation::on_frame` (a comparison), and one more `stopped()` atomic load before each read;
- in test binaries only, `DecoderProbe` bookkeeping and the gated tap (a counter and one thread-local read per
  conversion);
- per required source per frame: `reader_demand` looks up the clip's in-point;
- per post: `windows.retain` on an empty map;
- per reader decode that seeked: one `reader_seeks` counter update;
- per decoder open: `unfragmented_mov`, a few header reads.

None of these changes how many frames playback decodes, converts or seeks.

**The counters agree (r3).** The counting binaries print the frames decoded, frames converted and decoder seeks
for the whole 60 s play (`count_play_*`):

| Lane, workload | Pair | Side (order) | dropped | held max | decoded | converted | seeks | lookahead starved | folded |
|---|---|---|---|---|---|---|---|---|---|
| LH `typical_1080p` | 1 | ref, cand | 6, 4 | 138.2, 85.5 | 9803, 9805 | 5168, 5168 | 175, 174 | 2490, 2457 | 66, 66 |
| LH `typical_1080p` | 2 | ref, cand | 11, 10 | 85.1, 85.1 | 9811, 9902 | 5168, 5168 | 175, 179 | 2475, 2528 | 66, 66 |
| LH `typical_1080p` | 3 | ref, cand | 147, 69 | 85.1, 85.1 | 9827, 9779 | 5168, 5168 | 175, 173 | 2262, 2428 | 49, 66 |
| LL `feed_4x5` | 1 | ref, cand | 277, 220 | 105.7, 109.5 | 6671, 6623 | 2717, 2720 | 141, 139 | 236, 216 | 0, 0 |
| LL `feed_4x5` | 2 | ref, cand | 180, 286 | 103.5, 1407.9 | 6862, 6991 | 2722, 2717 | 145, 150 | 255, 251 | 0, 0 |
| LL `feed_4x5` | 3 | ref, cand | 234, 266 | 161.3, 996.1 | 6699, 6662 | 2720, 2719 | 139, 141 | 218, 236 | 0, 0 |

The plain binaries ran in reverse order (candidate first) on LH `typical_1080p`:

| Pair | dropped (cand, ref) | present p95 | held max | load at start |
|---|---|---|---|---|
| 1 | 91, **310** | 50.9, 64.4 | 85.3, 85.1 | 16.4, 12.3 |
| 2 | 62, 55 | 43.2, 42.7 | 117.1, **1305.1** | 8.9, 11.7 |
| 3 | 164, 173 | 63.9, 64.1 | 317.5, 85.2 | 9.5, 8.6 |

**Diagnosis: no playback regression is attributable to S2c.**
- Playback does the same work on both binaries. Decoded frames differ by at most 1.9%, conversions by at most 5,
  and seeks by at most 5, in both directions.
- Sync fallbacks, retire overruns and stale errors are 0 everywhere.
- With the candidate run first (r3), the reference drops more in 2 of 3 pairs. In r3's COUNT pairs (reference
  first), the candidate drops fewer in 3 of 3. Over the nine LH `typical_1080p` pairs in r2 and r3, the candidate
  drops more in 4 and fewer in 5.
- Dropped frames follow lanes, not binaries: lookahead starved and regions folded fall in whichever run drops
  more, on either binary. With `underrun_frames=0`, the drops are on the render and present side.
- G14's 100 ms is missed on both binaries. Under r3's load (12–16, `campfire` nextest on most cores), both
  binaries also held a frame for about a second: the reference 1305.1 ms on LH, the candidate 1407.9 and 996.1 ms
  on LL `feed_4x5`.
- **Partial:** I have not found what holds a frame for a second. The counters show it is not decode, conversion
  or seek work. LL `feed_4x5` held max is a shared miss, *environment-limited at load 12–16* in r3; in r2 the
  reference also missed once (116.4).

No bisect was needed: decode, conversion and seek counts match per run.

#### E13.5.9 I4, G3, G18 and the teardown (r2)

**I4** (`r28_end_to_end_tracked`, `typical_1080p`, mean ms, median of three runs per lane):

| Lane | ref1 / cand1 | ref2 / cand2 | ref3 / cand3 | Pair ratios, mean | Median ratio: mean / p95 |
|---|---|---|---|---|---|
| LL | 98.44 / 89.66 | 71.36 / 73.34 | 71.88 / 74.35 | 0.91, 1.03, 1.03 | **1.03** / 1.04 |
| LH | 83.36 / 86.29 | 88.38 / 97.56 | 85.25 / 88.46 | 1.04, 1.10, 1.04 | **1.04** / 1.04 |

**I4 passes** on the tap-off binary (median per-pair ratio ≤ 1.05; the test's own baseline rule passes too, at
−80 to −86%). LH pair 2 reads 1.10, and the other two 1.04. The verdict uses the median.

**G3 LH** is reported only (R48), and every binary exits 101 against its 60 fps floor. These are median fps,
ref1 / cand1 / ref2 / cand2:

| Workload | ref1 | cand1 | ref2 | cand2 |
|---|---|---|---|---|
| `typical_1080p` | 45.6 | 58.9 | 62.0 | 45.9 |
| `blend_heavy_1080p` | 46.8 | 65.8 | 60.4 | 43.9 |
| `heavy_4k` | 28.8 | 41.4 | 28.9 | 28.7 |

The pair ratios (fps) are 1.02, 1.07 and 1.22. The swings follow the run, not the binary.

**G18 LH** (R38's 120-frame export, S0 against the candidate, S C × 3):
- S0 took 222.6, 223.2 and 223.7 s; the candidate 43.0, 43.2 and 42.6 s.
- The pair ratios are 0.193, 0.193 and 0.191: **median 0.193, pass** (≤ 1.05). R47 read 0.19 on a quiet machine.
- Every export hashes `fe87e67c33845ccdf6e62d1e4d576a2d5fc86e54cb48cc1f5f7f03dd2d979b88` (3,375,562 bytes):
  **identical** (C-5).
- `g18_verdict.py` printed INCOMPLETE again, because of the lane-name format (E13.4.6). The verdict is computed by
  `r2/analyze.py` from the same lines.

**Teardown.** All 36 candidate P-seek lanes exited 0, COUNT lanes included. The reference, whose `seek_run` still
drops its session at return, crashed three times, all after `test result: ok`:
- COUNT LH `talk_recut` ref1 exited 134 ("double free or corruption");
- COUNT LH `explainer_16x9` ref1 exited 134 (a `khronos-egl` unwrap on a preview thread);
- P-seek LH `talk_recut` ref3 exited 139.

In r4, all 36 COUNT2 lanes exited 0, the 18 reference lanes included. That makes 0 crashes in 54 candidate
P-seek lanes, against 3 in 54 reference lanes. That is consistent with the teardown fix (repeated P-seek lanes ending
without a 139), not a proof: the reference's crashes were 3 in 54.

#### E13.5.10 Verdicts

| Gate | Verdict | Evidence |
|---|---|---|
| G8 L-1 (random p95 ≤ 40, GOP 60) | Shared miss, *environment-limited at load 1.9–22.5*; pair ratios 0.92 and 1.03 | E13.5.5 |
| G8 L-2 (random p95 ≤ 110, `talk_recut`) | **Pass**, 93.6 and 90.4 | E13.5.5 |
| G8 L-3 (+1 p95 ≤ 20, GOP 60) | **Pass on LL** (16.6). LH is a shared miss (26.6 against 25.7) | E13.5.5 |
| G8 L-4a (backward hit p95 ≤ 20, GOP 60) | **Pass**, 11.4 (LL) and 17.4 (LH) | E13.5.5, E13.5.7 |
| G8 L-4b (refill) | Recorded: refill mean 113–186 ms and p95 173–313 ms (r4), against the reference's 44–89 ms per step. The backward mean is 0.88–1.26× | E13.5.7 |
| G8 L-5 (drag ≥ 10 / 7 fps) | **Pass**, 29.8–30.0 fps (reference 3.8–12.7) | E13.5.5 |
| G8 L-6 (release shown) | **Pass**, 51/51 | E13.5.5 |
| P-play, LL `typical_1080p` (the n = 2 question) | **Settled: no regression**, 2/2/2 dropped on both binaries | E13.5.8 |
| P-play, LH `typical_1080p` and LL `feed_4x5` | **Not S2c's:** identical decode, conversion and seek work; the difference reverses with run order. G1 and G14 misses are shared, *environment-limited* | E13.5.8 |
| G17 / G11 | **Pass**: 0 sync fallbacks and 0 underruns in every run | E13.5.8 |
| I4 (paired ≤ 1.05) | **Pass**, 1.03 (LL) and 1.04 (LH) | E13.5.9 |
| G18 (≤ S0 + 5%, bytes identical) | **Pass**, 0.193, identical hash | E13.5.9 |
| G3 (LH, 60 fps) | Reported (R48), pair ratios 1.02–1.22 | E13.5.9 |
| Teardown | **Consistent with the fix**: 0 crashes in 54 candidate P-seek lanes, against 3 in 54 reference lanes | E13.5.9 |

**S2c closes on G8 at `d116e92`**, with L-1 and LH's L-3 as shared misses. No other regression is attributed to S2c on
the evidence above. Two costs stay in view: the refill p95 (L-4b, E13.5.7) and the order-flipped LH `typical_1080p`
drops (E13.5.8). Still open:
- S2c-5 waits on the lead's answers (E13.5.3);
- L-4b's refill cost is recorded with a proposal (E13.5.7).

#### E13.5.11 Tests, gates and lines

- **Workspace** (`cargo test --workspace`, fast tier, at `d116e92`): 36 test binaries, all ok, 3,502 passed and
  91 ignored. That includes:
  - media lib 1017 passed, 56 ignored (378 s);
  - app 790 passed, 4 ignored;
  - core 310 passed, 1 ignored;
  - agent 628 passed, 1 ignored; `mcp_server` 76 passed, 10 ignored.

  Log: `s2c-logs/s2c7/workspace-test.log.gz`.
- **Gates at `d116e92`** (`s2c-logs/s2c7/gate-*.log`):
  - rustc 1.99.0;
  - `cargo clippy --workspace --all-targets -- -D warnings` clean;
  - `cargo fmt -- --check` OK;
  - `cargo build -p kinewright-app` OK;
  - `python3 scripts/slow_tests.py lint`: "slow-test manifest and markers agree: 47 tests …; 44 allowlist entries
    well-formed".
- **New tests**, none ignored and none slow, so `ci/ignored-tests.txt` and `ci/slow-tests.txt` are unchanged:
  - `sched::tests::only_a_step_within_b_of_the_last_time_refills`;
  - `preview::tests::a_backward_jump_seeks_alone_and_an_agent_render_moves_no_travel`;
  - `pf1_s2c_witness::tests::the_output_oracle_fails_without_the_conversion_tap`;
  - `pf1_s2c_witness::tests::a_conversion_without_the_tap_clears_the_last_record`.

  `only_a_paused_job_refills_a_backward_window` and `continuation_needs_the_witnessed_pair_and_the_seek_stream`
  are strengthened.
- **Lines:** see E13.5.1. This section is documentation only.

### E13.6 S2c-5, Amendments R53–R64 and the closing re-time (2026-10-06/07)

The lead's order (r53-s2c5-rulings.md): S2c-5, then R53, gates, the workspace test, the closing re-time, this
section. Steps 1–4 were done first. The closing re-time then stopped after a paired check of the R53 gates:
both L-4 gates missed at GOP 60 (E13.6.4), and the section went to the lead with the conflict.

Amendment R54 (E13.6.6) fixed the refill: L-4b passed, L-4a still missed. Amendment R55 asked for a
measurement first and a stop if a hit's wait is not mostly conversion. It is not (E13.6.7), so R55 stopped
there: no conversion change was made. Amendment R56 asked for a trace first and a stop if the conversions run
back to back and still fall behind. They do (E13.6.8), so R56 stopped there too. Amendment R57 added a
row-wise fill and converts the window only below the newest t (E13.6.9). L-4b passed on both lanes and L-4a on
LL, but L-4a missed on LH, whose hit is now mostly its ~13 ms render.

Amendment R58 (Riel) records L-4a on LH as *environment-limited (GPU idle clocks), re-checked at S4's pinned
run*: not a pass, and on S4's checklist beside G3. The closing plan at `5fa62b4` (r58) then found P-seek steps
timing out on the candidate only. Amendment R59 found the cause (a reader idling while holding discard charges)
and fixed it, and the closing re-time ran at `c90d062` (E13.6.10). What stays open is listed at the end of
E13.6.10. Both Astra stage-close reviews then said do not close; Amendment R61 fixed their findings (E13.6.11).
R62–R64 replaced its re-time gate with work-counter equivalence and added a seeded model of the scheduler's
accounting (E13.6.12). R66 fixed three more paths of its one charge rule, made the model's oracle its own and
re-ran the gate; R67 waived the one counter that moved, windows filled, explicitly rather than attributing it (E13.6.13).

#### E13.6.1 S2c-5, open GOP (b)

| Commit | What | Lines |
|---|---|---|
| `354b30e` | S-2 note: the earlier-key retry on the witnessed pair | docs |
| `93c6b8c` | Oracle: Truth expects the retry's second seek (`retries`, AviDtsGuess excluded) | test |
| `04596d6` | Oracle: the model's retried seek has no anchor | test |
| `ebac92a` | Control: `ReferenceContinuation` skips the rule-2 latch after a retry (ruling item 1) | test |
| `69ad48c` | Oracle: a retried target's first packet is the retried anchor (ruling item 1) | test |
| `6c9e81f` | Oracle: `unreachable == 0` on the retried pair (ruling item 1; AviDtsGuess keeps its assertion) | test |
| `0c7c659` | The OpenGop pin regenerated: digest `14694f55cd0d355b` → `ce0029f0ab06abf5`, output `dfb48baf4f89ec08` → `854e56e5b3109248` | test |
| `5f6c2d2` | Witness: an OpenGop playback region start (starts 21, 22, 23, 47, 71) returns its leading frames, matching a linear decode, with two seeks | test |
| `118895a` | The retry: on the pair only, in every Seek render of `decode_window` | media |

Mutations (logs `s2c-logs/s2c7/s2c5-*.log`):
- retry off: **killed** (the region-start witness: no frame at 21; the CLI check: 5 unreachable);
- the latch skip off: **killed** (`continuation_reproduces_seek_on_every_fixture`);
- the pair gate off: **killed** (11 tests);
- seek to `dts` instead of `dts − 1`: **survives**. It is equivalent **on the recorded fixtures only**: on their
  mov/H.264 pair both seeks land on the same key (E13.6.6 gives the argument and the probed timestamps). This is
  not an equivalence for mov/H.264 in general (Amendment R61, item 8): the argument depends on where the fixture's
  earlier keys present, and a stream whose earlier key presents exactly at dts(K), or a GOP no longer than the
  reorder delay, could tell the two apart. The one-tick margin is therefore not witnessed by any fixture.

#### E13.6.2 Amendment R53

| Commit | What |
|---|---|
| `ffa94e7` | Design text: S-3's "t first" bullet and the Amendment R53 paragraph |
| `7f4bd3e`, `d37587e` | The three ruled witnesses (red before the implementation: `r53-witness-red.log`); `d37587e` keeps the workload's media alive |
| `f54ce3f` | A fourth witness: a step to the frame whose conversion is in flight waits for it (red: 2 extra seeks, `r53-inflight-red.log`) |
| `a7420fb` | The implementation (decoder: `decode_refill`, `convert_retained`, kept raw frames; scheduler: `Plan.window`, t first then descending, `Slot.retained`/`converting`, `Next::Decode.from`) |
| `eb7a4c1` | Design text for the in-flight case |

**What "t first" means, and what the witness shows (Amendment R61, item 7).** R53's guarantee is that t is
converted and **delivered to the preview ring** before any window conversion starts; "published" in R53 means
exactly that. It is not an ordering against the preview's render: under R56 the window's conversions deliberately
run while the preview renders and shows t. The witness `a_refill_publishes_t_before_converting_its_window`
(`preview.rs`) holds the reader before the first window conversion (t − 1) and checks that t is already in the
ring and that the refill converted one frame. It also sees t shown, but only because the hold keeps the conversion
from starting, so the shown check says nothing about production ordering. The witness and its mutation kills above
show conversion order against delivery, not against render. No code changed
for this item.

The fourth witness came from the media suite under load. `a_backward_drag_matches_fresh_seeks_and_refills_once_per_window`
seeked at 14 and 5 instead of 12: a step was posted while its window frame's conversion was in flight, so no reader
kept it any more and the step refilled. `backward` now counts that conversion as kept, which is what R53's
"waits for that conversion; it is not re-decoded" says.

Mutations (`s2c-logs/s2c7/r53-mut/`), every one killed:

| Mutant | Killed by |
|---|---|
| plan ascending (no t first) | the three ruled witnesses |
| no retaining (convert the window eagerly) | the three witnesses and two S-3 tests |
| a window plan survives a newer post | the step witness (and the drag test hung; stopped) |
| `backward` ignores kept frames | the drag test and the step witness |
| `decode` ignores kept frames | all five R53/S-3 preview tests |
| `backward` ignores the in-flight conversion | the fourth witness |

**Memory, outside K-1:** the reader keeps up to B − 1 = 15 decoded frames at the source's decoded size (about
3 MB each for 1080p 4:2:0) until it converts them, decodes anything else or closes. They are copies:
ffmpeg-next's `Clone` deep-copies, and the workspace forbids `unsafe`, so a reference-counted `av_frame_ref` is not
available.

#### E13.6.3 Review follow-ups (Sonnet, f967bae..cc0e8f3)

1. E13.5 wording: done in `230fbee`. The reviewer placed L-4a's hit p95 in the plain P-seek lanes. It comes from
   the COUNT lanes: a plain lane cannot tell a hit from a refill. E13.5 says so.
2. **`fall_back` posts do not update the travel memory: recorded as harmless, not changed.** R52 defines `last`
   as the last *posted required* time. A fall-back post's step is compared with an older `last`. The effect is
   only whether that step refills:
   - frames still match fresh seeks either way (C-5);
   - B·f ≤ share holds either way (K-2).

   Changing it would change R52's text.
3. The `mvex-ignored` and gate-removed fragmented-MP4 mutants were rerun on the committed tree `118895a`: both
   killed, and the clean tree passes (`frag-mut-*-118895a.log`, `frag-clean-118895a.log`). The R52 and
   fragmented-MP4 mutations first ran on trees whose diff matched the commit.
4. Not applicable (item 2 is unchanged).

#### E13.6.4 The R53 gates: a paired check (r5a)

Before the full closing plan (`s2c-timing/r5/plan-r5.txt`, 116 lanes, about four hours), COUNT P-seek
`seek_gop60` ran as three pairs on LL and on LH:
- reference `count2-ref-ad8f896`;
- candidate `count3-cand-eb7a4c1`, which is `eb7a4c1` + `count/apply-count3.py`. That is counting v2 plus R53's
  time to the full window.

Conditions:
- 12 lanes, 13:22–13:44 EDT, all exit 0;
- 1-minute load 8.0–19.0 (annotation only, R50);
- nothing of mine ran during the lanes.

Medians over nine runs per side; pair ratios cand/ref, median of three:

| Lane | Ref step mean / p95 | Cand backward mean (ratio) | Cand hit mean / **p95** (n) | Cand refill = time to t, mean (**ratio to ref step mean**) | Per refill: decoded / converted | Windows fully converted |
|---|---|---|---|---|---|---|
| LL | 51.4 / 73.5 | 40.4 (0.78) | 18.9 / **26.7** (132) | 82.3 (**1.59**: 1.68, 1.57, 1.59) | 46 / 3.1 | 0 of 68 |
| LH | 47.6 / 65.2 | 35.9 (0.77) | 16.9 / **23.2** (132) | 72.6 (**1.52**: 1.52, 1.45, 1.63) | 46 / 3.2 | 0 of 68 |

Against r4 (`d116e92`, eager window conversion), R53 at GOP 60:
- **time to t on a refill fell** from 139.8 / 144.0 ms to 82.3 / 72.6;
- **the backward mean fell** from 1.26 / 1.20× the reference to 0.78 / 0.77×;
- **hits got slower:** from 10.4 / 13.0 ms to 18.9 / 16.9, and the hit p95 rose from 13.8 / 18.3 to 26.7 / 23.2.

The verdicts:
- **L-4b fails:** 1.59 / 1.52 against ≤ 1.25.
- **L-4a fails:** a hit p95 of 26.7 / 23.2 against ≤ 20.

**Why both gates miss (the conflict):**
- **L-4a.** P-seek posts each backward step as soon as the previous one is shown. Each post cancels the window's
  remaining conversions, which R53 requires and a witness checks. So no window is ever fully converted (0 of 68).
  A step into the window is served by converting its kept frame on demand: no decode and no seek, so it counts as
  a hit, but it costs one conversion and a reader hand-off. Before R53 the refill had paid those conversions and
  a hit was a ring lookup.
- **L-4b.** Time to t is bounded below by the window's decoding. A refill decodes 46 frames against the
  reference's 30, because a quarter of the windows start before t's key and walk the earlier GOP. It also copies
  15 decoded frames. At about 1.2–1.5 ms per decoded frame (E13.5.7), the 16 extra decodes alone are 20–24 ms on
  a ~50 ms step: about 1.4–1.5× before any copy. The copy cost was never measured, and R54 removed it: kept
  frames are moved decoder references, not copies (E13.6.6, `count_keep_copied` 0).

At R53 the full closing re-time was not run, because its L-4 gates would fail for the reasons above. Only the
workspace test was done. P-play, I4 and G18 ran in the closing plan after R58, at `5fa62b4`, and again after R59,
at `c90d062` (E13.6.10).

#### E13.6.5 Tests and gates at `eb7a4c1`

- Workspace (`cargo test --workspace`, fast tier, at `a7420fb`; `eb7a4c1` is docs only): 36 binaries, all ok,
  3,507 passed and 91 ignored. Log: `workspace-a7420fb.log.gz`. Media lib: 1022 passed and 56 ignored
  (`r53-media-tests2.log.gz`).
- rustc 1.99.0: `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt -- --check` OK;
  `python3 scripts/slow_tests.py lint` OK.
- No new test is ignored or slow, so `ci/ignored-tests.txt` and `ci/slow-tests.txt` are unchanged.

#### E13.6.6 Amendment R54: the paired check (r54a)

Commits: `feae206` (design), `51e90d8` (three witnesses, red), `b366669` (implementation). Mutations, each red:
the refill seeking from the window start (`decode_window(start, t)`) fails the fresh-seek witness; reserving f for
a kept window frame fails the K-1 witness (red before the implementation: live 368,640 bytes, 16 f, against
1,296,000 kept + f); dropping the in-window continuation fails the conversion witness. Media suite at `b366669`: 1025 passed, 56 ignored; clippy 1.99 and fmt
clean.

Implementation notes the ruling did not spell out:
- d is an upper bound computed when f is measured: the decoder's pixel format at its default buffer geometry
  (stride a multiple of 64 bytes for every plane, so the width is padded to 64 chroma samples; the height padded
  to 32 after H.264's two extra rows). A first version without the chroma pad undercounted (a 320-wide H.264
  frame has a 384-byte luma stride) and the K-1 witness caught it.
- A frame cancelled by a post outside the window has its reservation returned at that post, as R54 says; its
  decoded frame stays in the reader's decoder until that reader's next decode or close. So **one window's kept
  frames are uncharged for that interval.** Charging them until the drop could deadlock: admission would wait
  for bytes that only that reader's next (admitted) decode frees. Amendment R56 accepts the gap as bounded: one
  window per reader, at most B − 1 = 15 decoded frames, until that reader decodes again or closes. It is deferred
  to S4's G15 RSS check (design §15, D13).
- The keeping cost (r54a, GOP 60, 1080p): 40.8 MB of decoded frames kept per refill (about 13 frames), with no
  copies: every kept frame was a moved decoder reference (`count_keep_copied` 0).
- S2c-5's surviving mutant (seeking to `dts` rather than `dts − 1`) is equivalent on the recorded fixtures'
  mov/H.264 pair, not on mov/H.264 in general (R61 item 8). On that pair both
  seeks land on the latest key whose pts ≤ the target. An open-GOP key K with leading frames has pts(K) > dts(K),
  so both exclude K and land on the key before it unless that earlier key presents exactly at dts(K). In the
  fixture the earlier keys present at 0, 12288 and 24576 against dts 9728, 23040 and 35328 (time base 1/15360;
  `s2c5-dts-probe.log`). A counterexample needs a GOP no longer than the reorder delay; no fixture was made.

The paired check: r5a kit, `seek_gop60`, three pairs on LL and on LH, 14:41–15:05 EDT, all exit 0. Reference
`count2-ref-ad8f896`, candidate `count4-cand-b366669` (counting v4). Log `r5/timing-r54a.log.gz`, table
`r5/l5-r54a.txt`.

| Lane | Ref step mean | Cand backward mean (ratio) | Cand hit mean / **p95** (n) | **L-4b** (pairs) | Pre-converted hits | Windows filled | Decoded per refill |
|---|---|---|---|---|---|---|---|
| LL | 46.0 | 30.2 (0.66) | 17.1 / **23.3** (125) | **1.14** (1.19, 1.14, 1.08) | 18 of 125 | 11 of 75 | 32.9 (ref 30.0) |
| LH | 47.3 | 33.5 (0.68) | 19.7 / **26.4** (125) | **1.15** (1.11, 1.15, 1.15) | 21 of 125 | 11 of 75 | 32.9 (ref 30.0) |

L-4b passes; **L-4a misses**. The lead's Amendment R55 followed.

#### E13.6.7 Amendment R55, step 1: the measurement (stopped)

Commits: `2d39659` (design), `585e97b` (test-build counters, `pf1_clock`; no behaviour change). One pair per lane,
`seek_gop60`, reference `count2-ref-ad8f896`, candidate `cand-585e97b` (`c1e2c086…`), 15:18–15:24 EDT, all exit 0.
Log `r5/timing-r55m.log.gz`. Medians of the candidate's three runs per lane:

| Lane | Hit mean / p95 | **Wait for frames** mean / p95 | **Render** mean | Rest (dispatch, demand) | Pre-converted | Windows filled |
|---|---|---|---|---|---|---|
| LL | 19.8 / 26.8 | 7.5 / 14.1 | 9.5 | about 2.8 | 6, 13, 19 of ~124 | 5–6 of ~76 |
| LH | 17.5 / 22.7 | 5.0 / 9.3 | 9.8 | about 2.7 | 23, 25, 28 of ~124 | 10–16 of ~76 |

Conversion, per frame (ms), split into the filter graph (YUV to RGBA64, swscale) and the working-frame step:

| Lane | t: total / graph / working | Window frame: total / graph / working | Graph's calling-thread CPU ÷ wall (t, window) | Process CPU per refill |
|---|---|---|---|---|
| LL | 5.69 / 1.60 / 3.98 | 6.44 / 1.18 / 5.20 | 0.63, 0.90 | 463 ms |
| LH | 4.86 / 1.28 / 3.56 | 4.40 / 0.80 / 3.60 | 0.81, 1.31 | 415 ms |

Findings:
- **A hit's time is not mostly conversion.** Its wait for its frame is 7.5 / 5.0 ms (about a third of the hit),
  and the render after it is 9.5 / 9.8 ms. Under R55's own rule that stops the slice here.
- Before R53 a whole hit averaged 10.4 / 13.0 ms (E13.5), render included. The render is now about 9.5 ms on both
  lanes. **Partial:** the render may be slower now because the window conversions run beside it (CPU contention).
  That is inferred, not measured.
- The converter's graph is effectively single-threaded. The scale filter's `threads` option defaults to 1, and the
  calling thread's CPU is about the graph's wall time on window frames. But the graph is only about 20% of a
  conversion (0.8–1.6 ms); the working-frame step (our RGBA64 to working-frame code) is the other 80%.
- So threading swscale (R55 option a) could save at most about 1 ms a conversion. A CLI probe shows the option
  works in safe code: 1080p to 1280×720 rgba64le, 120 frames, scale `threads` 1 / 2 / 4 = 0.66 / 0.34 / 0.31 s.
- If the wait for conversion went to zero, a hit would be about render + rest: roughly 12.5 ms mean. **Partial:**
  that is an estimate; its p95 was not measured.
- The process CPU per refill counts everything in the backward phase (the hits' renders and lavapipe's CPU
  rendering on LL), not only the refill.

#### E13.6.8 Amendment R56, step 1: the trace (stopped)

Commits: `794caf3` (test-build trace in `pf1_clock`; no behaviour change), `8257911` (R56 design text, D12, D13).
One pair per lane, `seek_gop60`, reference `count2-ref-ad8f896`, candidate `cand-794caf3` (`3d0e7d0c…`) with
`PF1_TRACE`, 15:38–15:45 EDT. All exit 0, except the LH reference: it exited 139 after `test result: ok` (a
teardown crash). Files: `r56/timing-r56t.log.gz`, the traces `r56/trace-r56t-{LL,LH}.txt.gz`, and the analysis
scripts `r56/trace.py`, `extra.py` and `est.py`.

The candidate's own counters match r55m: hit p95 20.1 / 28.6 / 23.9 (LL) and 24.6 / 28.3 / 30.9 (LH), with 11–22
of ~124 hits pre-converted.

What the reader was doing when each in-window hit was posted (all three runs, 372 hits per lane; times in ms):

| Lane | Converting a frame above the new t | Converting t itself | t already converted | Wait (mean / p95): above / own / pre-converted |
|---|---|---|---|---|
| LL | 286 (77%): 2.1 to finish it, then 4.7 for t | 29 (8%) | 57 (15%) | 6.8 / 10.2, 2.6 / 5.0, 0.8 / 6.9 |
| LH | 289 (78%): 3.3 to finish it, then 6.5 for t | 25 (7%) | 58 (16%) | 9.9 / 16.3, 4.0 / 9.3, 1.2 / 9.7 |

Findings:
- **The conversions run back to back.** On the same reader, the median gap between one window conversion and the
  next is 0.02 ms. The reader went idle 39 (LL) and 25 (LH) times, and never while it still kept an unconverted
  frame. No hit, render, demand, hand-off or job-thread stall holds them up: they run on the reader thread, not
  the preview's. About 3.0–3.2 conversions finish between one post and the next.
- **They still fall behind, because the stepper skips frames.** P-seek's backward steps are 1–12 frames (mean
  6.8 here), and a window lives 1.6 hits (228 refills, 372 hits). In the ~20 ms between posts the reader
  converts t − 1, t − 2 and t − 3. The next step usually lands below them, so the frame in flight is one the
  stepper skipped.
- **Most window conversions are for frames never shown.** Of 1,891 (LL) and 1,786 (LH), only 388 and 390 are of
  frames a later step showed. 632 and 611 started above the newest posted t: the descending pass resumes from the
  top of what is left, so after a step it converts the skipped frames first.
- The free time between a hit's render and the next post is 2.7 ms (the harness's own turn). The rest of a cycle
  is the hit's wait and render, during which the conversions run.

Under R56 item 1 ("if the trace shows conversions really do run back-to-back and still fall behind, stop and
report"), this round stops here; no scheduling change was made. For the lead, **partial (estimates, not
measured):**
- Re-prioritizing (after a step, convert below the new t first; never above it) removes the 632 / 611
  conversions that start above t, and their CPU. It does not change how many frames fit between posts (about 3),
  so a step longer than about 3 frames still lands on an unconverted frame. With steps uniform on 1–12, that is
  about three hits in four.
- A second lane converting t at once, instead of after the frame in flight, removes the "finish it" part. Taking
  that part out of each traced hit leaves hit p95 at 20.9 (LL) and 25.3 (LH) ms. Taking the whole wait out leaves
  16.0 and 16.7. So for L-4a this stepper needs nearly every hit pre-converted. That means converting the whole
  skipped range (about 6 frames, 30–40 ms) within one ~20 ms cycle: two to three lanes beyond the reader, over
  R56's one-extra-thread cap.

#### E13.6.9 Amendment R57: the row-wise fill and the re-prioritized window (stopped: L-4a misses on LH)

Commits:

| Commit | What |
|---|---|
| `c2bbc6d` | R57 (revised) design text |
| `4eb29ad` | fill witness, red against a stub; the fused loop moved unchanged to `fill_fused` |
| `959d9e3` | `fill_rows`: the fast path for unrotated, unflipped frames |
| `23c815b` | re-prioritization witness, red |
| `96ea61d` | the window converts only below the newest t |

**Item 1, the fast path.** The witness compares `fill_rows` and `fill_managed_plane` with the fused loop. It covers
five separable descriptions (the fixtures' BT.709 limited and full, RGB 10-bit, BT.1886 12-bit and identity
16-bit), sizes 1×1, 5×3, 7×5, 33×7 and 64×4, and padded strides of 0, 3, 8 and 24 pixels. Through
`fill_managed_plane` it also checks every rotation and flip. All are bit-identical. A row-stride off-by-one
mutation fails three tests. The X-2 parity tests and every pin are unchanged.

**Item 2, re-prioritization.** After a post at t′ inside a held window, `continued()` keeps only the kept frames
below t′, nearest first. Every reader's kept frames above t′ leave its retained set. Their reservations move to
the reader, and its next decode's hold carries them, with `discard = t′`. The decoder drops its kept frames above
t′ before that decode. The hold releases their bytes with the excess once the frame is converted, or on a stop
or failure. `closed`, `exited` and `fail_start` release them if the decoder goes first. The conversion in flight
finishes and stays held. No new uncharged interval was added.

R54's in-window witness changes, as R57 states: after 19 → 14 with 17 in flight, the order was 14, 16, 15, 13…4
and is now 14, 13…4. This edits an R54 oracle (the order, and 15 and 16 no longer held). The edit follows R57
item 2, and the lead confirmed it. Its C-5, no-seek and conversion-only checks stay, and it gains three checks:
- K-1 live is unchanged across the post;
- the decoder's kept bytes show 15 and 16 dropped (10/13 of the bytes it kept after 17);
- 15 and 16 are not in the ring.

Three mutations fail it: frames above t′ kept in the pass, their bytes released at the post, and the decoder not
dropping them. Media: lib 1026 passed, 56 ignored; the integration tests are green.

**The paired L-4 check (r57),** 16:27–16:51 EDT. `seek_gop60`, 3 pairs per lane: reference
`count2-ref-ad8f896` against candidate `cand-96ea61d` (`90490da8…`). Pair 1's candidate was traced. One extra
run per lane used the R56 binary `cand-794caf3` ("before"). All exits were 0. The load was heavy and is recorded,
not gating (R50): another project's rustc, ffmpeg and test builds averaged 1.5–9 cores beside the test (LL cand1:
about 9). Files are in `s2c-timing/r57/`: `timing-r57.log.gz`, `samples-r57.log.gz`, `l57.py` and `l57-r57.txt`. Medians
of each run's three passes:

| Lane | L-4a hit p95 per pair → median | Hit mean | L-4b (≤ 1.25) | Back mean ÷ ref |
|---|---|---|---|---|
| LL | 23.9 (cand1, ~9 cores of other load), 17.2, 17.6 → **17.6 PASS** | 17.9, 12.6, 12.7 | 1.07 PASS | 0.59 |
| LH | 22.4, 23.5, 21.1 → **22.4 MISS** | 18.2, 18.5, 17.1 | 1.06 PASS | 0.65 |

A supplementary paired run (r57w) put before (`cand-794caf3`) against the candidate, 3 pairs per lane, untraced
(`timing-r57w.log.gz`, `samples-r57w.log.gz`, `l57-r57w.txt`):
- the candidate's L-4a: LL 16.8, 17.3, 17.0; LH 23.9, 23.8, 32.1 (that run had about 10 cores of other load);
- a third traced pair (r57t2) gave LL 16.6 and LH 21.3.

Counters (the r57 candidate's untraced pairs and r57w; "before" is R56's binary under the same harness):

| Lane | Wait / render (mean) | Pre-converted of 125 | Windows filled of ~75 | Process CPU per refill |
|---|---|---|---|---|
| LL now | 2.2 / 7.6–8.0 | 44–50 | 52–54 | 442–454 ms |
| LL before | 5.0–6.6 / 7.3–8.2 | 13–19 | 7–9 | 437–455 ms |
| LH now | 1.6–3.0 / 13.0–13.3 | 51–70 | 57–68 | 422–430 ms |
| LH before | 4.3–5.6 / 13.0–13.3 | 27–35 | 17–23 | 426–446 ms |

The working-frame step per conversion (ms; r57w, 3 pairs, before → now; median ratio):

| Lane | t | Window frame |
|---|---|---|
| LL | 3.39, 3.32, 3.69 → 1.87, 1.90, 1.92 (×0.55) | 4.47, 3.63, 4.43 → 2.12, 2.11, 2.18 (×0.49) |
| LH | 3.37, 3.86, 3.59 → 1.94, 1.87, 2.61 (×0.58) | 3.74, 4.60, 4.60 → 2.48, 3.31, 3.30 (×0.72) |

Whole conversions (graph and working frame) went from 4.4–5.3 to 2.9–3.2 ms on LL, and from 4.5–5.4 to 3.2–4.2
ms on LH (×0.56–0.77). The before binary differs from the candidate in both items, but the working-frame step
does not depend on the scheduling.

Traces: `trace-r57-{LL,LH}.txt.gz` (r57's pair 1) and `trace2-r57-{LL,LH}.txt.gz` (r57t2: `timing-r57t2.log.gz`,
`samples-r57t2.log.gz`, `l57-r57t2.txt`), analysed after gunzip by
`../r56/trace.py`. From r57t2 (372 hits per lane):
- **Pre-converted hits:** 40% (LL) and 51% (LH), up from 15–16% under R56. The others wait behind the conversion
  in flight (1.3 / 1.4 ms) and then their own (2.6 / 2.8 ms).
- **Conversions started above the newest t:** 1 and 0, down from 632 and 611.
- **Conversions of frames later shown:** 390 of 2,213 (LL) and 397 of 2,476 (LH), 16–18%. The stepper still skips
  frames, and the pass converts them on the way down.
- **A hit's time:** wait 2.2 / 1.8 ms and render 7.8 / 13.2 ms. The rest is about 2.5 ms of dispatch.

Findings:
- **LH misses L-4a because of its render, not conversion.** The LH render is 13.0–13.3 ms in every run, the R56
  binary included. Under R55 it was 9.8 (E13.6.7), so the change comes from the environment, not R57. In 117 of the
  136 GPU samples taken during the LH runs, the GPU was at P8 (210 MHz), its idle clocks. With the whole wait removed, the r57t2 LH trace
  still gives hit p95 18.1 ms.
- **LL passes L-4a** in every pair not run under about 9 cores of other load (16.6–17.6 ms).
- **L-4b passes on both lanes** (1.06–1.07). The backward mean is 0.59 / 0.65 of the reference.

Under R57 item 4 ("if L-4a misses: stop and report the counters"), this round stops here. The workspace test,
the closing plan (R50, I4, G18) and E13.6's close were not run.

The estimates for the lead and Riel are **partial: estimates, not measured.** `r57/est57.py` replays each traced
window and its posts with n lanes per source. Each lane converts the kept frames below the newest t, nearest
first, using the run's own conversion durations. A hit's latency is then its measured step-to-seen, minus its
measured wait, plus the simulated wait and the render slowdown. The render slowdown is the render's regression
on the conversions overlapping it, times the extra overlap the lanes add. With one lane the replay matches the
measurement (LL p95 17.6 against 16.8 measured; LH 23.4 against 22.0). From r57t2 (`est57-t2.txt`):

| Lanes per source | LL hit mean / p95 | LL render slowdown | LH hit mean / p95 | Conversions per refill | Conversion CPU per refill |
|---|---|---|---|---|---|
| 1 (now) | 12.9 / 17.6 | — | 17.9 / 23.4 | 10.1 / 11.1 | 31 / 34 ms |
| 2 (+1) | 11.5 / 15.3 | +0.44 ms (+6%) | 16.0 / 19.5 | 12.8 / 13.0 | 40 / 40 ms |
| 3 (+2) | 10.7 / 13.1 | +0.18 ms (+2%) | 15.6 / 18.1 | 13.1 / 13.1 | 41 / 41 ms |

- With the extra lanes' conversions 25% slower under contention, the LH p95 is 22.3 with +1 and 18.4 with +2.
  LL's render slowdown is then +7–8%.
- **Lavapipe's slope:** each overlapping conversion adds about 1.2 ms to an LL render (7.8 ms mean), against 0.4
  ms on LH. More lanes add little render overlap, because they finish the window sooner.
- **The extra CPU per refill** is about +9 ms (+1 lane) or +10 ms (+2), against about 440 ms of process CPU per
  refill. The window is only ~13 frames, so the lanes convert little more than one does.
- **LH would pass L-4a only with two extra lanes**, or with one if conversions do not slow, and then by about
  0.5–2 ms. Its ~13 ms render is the floor either way. One heavily loaded LH run reached 32.1 ms.

#### E13.6.10 The closing plan (r58), Amendment R59 and the closing re-time (r59)

**The closing plan at `5fa62b4` (r58).** 114 lanes (`s2c-timing/r58/plan-r58.txt`), 17:40–22:25 EDT on 2026-10-06.
The candidate was `cand-5fa62b4` (`b1cf2d9c…`); the references were `ref-ad8f896` and, on COUNT lanes,
`count2-ref-ad8f896`; G18 ran against `s0-d19bdf9-lane2aa4e1b`. The load at lane start was 4.1–34.2 (median 12.6),
with `campfire` at up to 1361% CPU (annotation, R50). It found three things:
- **P-seek `explainer_16x9` backward steps timed out at 10 s, on the candidate only:** 117 timeouts in 15 of its
  36 runs (LL 11 of 18, LH 4 of 18), and none in the reference's 36. Every lane still exited 0, because the
  harness printed `timeouts=` without failing on it. That is how this hid until the closing plan.
- **P-play LH `feed_4x5` was worse on the candidate in all three pairs:** held max 728 / 547 / 8,250 ms against
  135 / 171 / 174, and dropped 516 / 762 / 1,126 against 439 / 646 / 597. These lanes carried no counters.
- **LH `talk_recut` random p95:** pair ratio median 1.25 (0.83, 1.45, 1.25).

I4 (0.87 / 0.80) and G18 (identical hash, 0.174) passed. The R58 L-4a result on LH is unchanged.

**The cause (R59 item 1).** The instrumented build `diag-counters` (`5fa62b4` + the counters, `b8d2fba6…`) repeated
LL `explainer_16x9` run 0 (`s2c-timing/r59/diag/`): 6 timeouts; 50 admission waits totalling 60,063 ms, the longest
60,011 ms; and 25 counts of a reader idle while holding discard charges. The trace shows it:
- the post at 235 is a refill below the held window [237, 250], and its admission waits;
- reader 0 finishes converting 241, then idles holding the charges of the frames the post discarded;
- it wakes every 5 s (the quiescence timeout), has no decode to start (its own required frame is unreserved), and
  idles again; it cannot retire, because that frame is still owed;
- nothing decodes until the wait ends 60 s later, six 10 s steps on.

This is the deadlock E13.6.6 warned of. The design's Amendment R59 states the fix.

**The fix.**

| Commit | What |
|---|---|
| `8a1d833` | test: the R59 counters (admission waits: count, total, longest; a reader idle while holding discard charges); a P-seek step timeout, or `count_idle_discard` > 0, fails the lane |
| `7281b9a` | test: the two-source liveness witness, red |
| `c90d062` | media: the discard-only job (`Next::Discard`), and the design text R59 |

- **The discard-only job.** In `Readers::next`, a reader with no decode to start but with discarded frames pending
  is handed `Next::Discard { bound, bytes }`. Outside `Sched` its decoder drops the kept frames above `bound`, and
  then their charges are released, which wakes admission. A post wakes every reader, so the job runs at the post.
- **The witness** `a_reader_holding_discards_drops_them_so_the_other_source_is_admitted` (sched):
  - budget 20 frames; source 0's reader keeps a window, so 36, 37 and 38 are retained;
  - a step to 35 beside source 1's frame at 0 does not fit beside their charges: `Wait`;
  - after one step of every reader, admission must be `Ready` while source 0's reader still exists, and both
    frames resolve.

  It fails before the fix (`s2c-logs/s2c7/r59-witness-red.log`) and passes after. With the discard-only job
  removed (the mutation) it fails again (`r59-mutant.log`). `preview::` and `sched::`: 88 passed
  (`r59-fix-tests.log`). The media library at `c90d062`: 1027 passed, 56 ignored (`r59-media-lib.log`).
- **The asserts (the coordinator's addition to R59).** `pf1_seek_baseline` now fails on any P-seek step timeout,
  with "R59: N P-seek step(s) timed out (workload, run N)". Both harnesses fail if `count_idle_discard` is not 0.
  Both asserts went in with the counters (`8a1d833`), so timeouts and idle discard charges fail loudly. The
  pre-fix reproduction above now exits 101.

**P-play LH `feed_4x5` (R59 item 3; r59a).** Nine counted lanes, 23:05–23:19 EDT, all exit 0, rotated: pair 1 ref,
pre, cand; pair 2 pre, cand, ref; pair 3 cand, ref, pre. The binaries were ref `count5-ref-ad8f896` (`bd5f14b5…`);
pre `diag-counters` (the r58 candidate's code); and cand `cand-c90d062` (`55acda05…`). Logs:
`s2c-timing/r59/timing-r59a.log.gz`, `samples-r59a.log.gz`.

| Lane | Load | Held max (ms) | Dropped | On time | Admission waits (total ms) | Idle with discards | Decoded | Converted | Lookahead starved |
|---|---|---|---|---|---|---|---|---|---|
| ref1 | 10.9 | 170.6 | 661 | 1139 | 0 | — | 6614 | 2705 | 203 |
| pre1 | 14.7 | 183.8 | 645 | 1155 | 0 | 0 | 6880 | 2705 | 200 |
| cand1 | 15.9 | 306.4 | 761 | 1039 | 0 | 0 | 6849 | 2697 | 202 |
| pre2 | 17.2 | **1658.2** | 931 | 867 | 0 | 0 | 6915 | 2624 | 130 |
| cand2 | 24.8 | 1092.8 | 739 | 1060 | 1 (4.8) | 0 | 7402 | 2656 | 175 |
| ref2 | 29.4 | 507.8 | 589 | 1211 | 0 | — | 7511 | 2704 | 198 |
| cand3 | 17.6 | 242.9 | 612 | 1187 | 1 (2.4) | 0 | 6630 | 2705 | 199 |
| ref3 | 14.7 | 875.6 | 946 | 843 | 0 | — | 6650 | 2598 | 162 |
| pre3 | 26.9 | 85.1 | 691 | 1109 | 0 | 0 | 7120 | 2704 | 194 |

- **This is not the admission stall.** The pre-fix build had no admission wait in any run, yet held a frame for
  1,658 ms in pre2. The fixed candidate waited at most once, for 4.8 ms. The reference spikes too (ref3: 875.6 ms,
  946 dropped).
- **The counters match across the three binaries:** decoded 6.6–7.5k, converted 2.6–2.7k, ledger peak 99.0 MiB,
  and no retire overrun or sync fallback. The large holds come with fewer lookahead-starved counts and higher clock
  stalls (pre2: 130 and 100.9 ms; ref3: 162 and 89.8 ms), on the reference as well.
- **The lead's ruling:** the LH `feed_4x5` spikes are a property of the workload and the machine, shared with the
  reference, and are not attributed to S2c. They join the "~1 s holds go to S4" finding (E13.5.8). **G14 on
  `feed_4x5` is not a pass.**
- **r58's 8,250 ms run stays unexplained:** its lanes had no counters, so it can be neither confirmed nor excluded
  as the stall.

**The closing re-time at `c90d062` (r59b).** I stopped the first start of `plan-r59.txt` (114 lanes) about two
minutes in, for the stop above. It was restarted at 23:23:54 EDT, with the lead's stop guards checked every 30 s
(`r59/guard.py`). The run would stop if a candidate P-play or P-seek lane showed any of:
- `count_idle_discard` > 0;
- an admission wait of 1 s or more;
- a P-seek timeout;
- a held max over 2 s;
- a nonzero exit.

The run ended at 04:28:26 EDT on 2026-10-07. All 48 candidate P-play and P-seek lanes passed the guard. The pairs
were:
- P-seek and P-play: `ref-ad8f896` (`6d52c2f9…`) against `cand-c90d062`;
- COUNT P-seek: `count5-ref-ad8f896` against the same candidate;
- G18: `s0-d19bdf9-lane2aa4e1b` (`5731d7ae…`) against the candidate.

Three reference lanes exited 139 after `test result: ok`, the reference binary's known teardown crash (COUNT LH
`seek_gop60` ref2, COUNT LH `explainer_16x9` ref3, and LH `talk_recut` ref2). Every other lane exited 0. The load
at lane start was 4.7–46.1 (median 14.8); `chrome-headless`, `node`, `rustc` and `campfire` were the main foreign CPU
(annotation, R50). Logs: `timing-r59b.log.gz`, `samples-r59b.log.gz`, `analyze-r59b.txt` (`r2/analyze.py`) and `l5-r59b.txt`
(`r5/l5.py`). `l5.py` now starts a new lane at any `# BEGIN`. Before this fix, a plain lane's lines that followed a
COUNT lane were counted into it. *Correction (Amendment R61, item 9):* earlier plans did mix the two kinds. r58
(`plan-r58.txt`, 36 COUNT and 78 other lanes) did, and so did the unrun full plan `r5/plan-r5.txt`; E13.6.11 gives
the re-run and what changes.

The candidate's R59 counters, per run (36 lanes of 3 P-seek runs, 12 P-play runs):

| Workload | Admission waits per run | Their total per run (ms) | Longest (ms) | Idle with discards | Timeouts |
|---|---|---|---|---|---|
| `seek_gop60` (LL, LH) | 3–17 | 0.9–75.8 | 35.3 | 0 | 0 |
| `talk_recut` | 1–10 | 0.0–20.5 | 17.7 | 0 | 0 |
| `explainer_16x9` | 49–63 | 55.9–250.5 | 21.3 | 0 | 0 |
| P-play (both workloads) | 0–1 | 0.0–2.2 | 2.2 | 0 | — |

Admission still waits in `explainer_16x9`, about 55 times a run, but the longest wait was 21.3 ms. Before the fix,
50 waits took 60 s.

**P-seek (R50, 3 pairs of 3 runs; per-pair ratio cand/ref, median and the three pairs):**

| Lane, workload | Random p95 | Forward p95 | +1 p95 | Backward p95 | Drag distinct fps | Timeouts (cand) |
|---|---|---|---|---|---|---|
| LL `seek_gop60` | 0.96 (0.93, 0.96, 1.73) | 0.50 | 0.85 | 0.96 | ×3.00 | 0 |
| LL `talk_recut` | 0.96 (0.96, 1.04, 0.86) | 0.73 | 0.95 | 0.93 | ×3.92 | 0 |
| LL `explainer_16x9` | 1.00 (1.05, 0.94, 1.00) | 0.27 | 0.92 | 0.88 | ×12.25 | **0** |
| LH `seek_gop60` | 0.59 (0.59, 0.48, 1.02) | 0.58 | 1.03 | 1.08 | ×3.75 | 0 |
| LH `talk_recut` | 0.82 (0.61, 0.82, 2.97) | 0.78 | 1.62 (1.62, 1.08, 2.52) | 2.45 (2.45, 1.18, 2.66) | ×4.15 | 0 |
| LH `explainer_16x9` | 0.94 (0.78, 0.94, 1.12) | 0.27 | 1.07 | 0.80 | ×37.00 | **0** |

- **`explainer_16x9` timed out 0 times** in all 36 candidate runs (P-seek and COUNT, LL and LH), against 117 at
  `5fa62b4`.
- **LH `talk_recut` random p95, as the R50 pair median with the reference spread:** 0.82 (0.61, 0.82, 2.97). The
  candidate's median was 306.6 ms (range 101.9–517.4) against the reference's 160.0 (110.2–300.2). In the COUNT
  lanes it was 0.77 (0.77, 0.43, 1.59), with the reference at 261.6 (116.0–628.1).
- **LH `talk_recut` pair 3.** All three of cand3's runs were slow: random p95 474.6 / 465.8 / 517.4 ms, backward
  378.1 / 407.7 / 388.0. That pair alone carries the 2.97, 2.52 and 2.66.
  - Its counters match the fast runs: decoded 23.8–25.3k (other candidate runs 23.5–25.3k), converted 1.05–1.13k
    (1.08–1.32k), process CPU in the backward phase 55.6–56.7 s (53.4–56.5 s), and the longest admission wait 17.7
    ms.
  - The same work took longer on the wall clock. The foreign CPU logged during that lane was about three times any
    other `talk_recut` lane's (95,051 %·samples against 29,687–34,130).
  - In the COUNT lanes, with counters on both sides, the same workload's backward p95 ratio is 0.50 and its L-4b
    is 0.67.

**L-4b (COUNT lanes; time to t on a refill divided by the reference's step mean; ≤ 1.25) passes on every
workload:**

| Workload | LL | LH |
|---|---|---|
| `seek_gop60` | 0.82 | 1.02 |
| `talk_recut` | 1.11 | 0.67 |
| `explainer_16x9` | 0.71 | 1.01 |

The backward mean is 0.40–0.63 of the reference's.

**L-4a (hit p95 ≤ 20 ms, `seek_gop60`).**
- **In r59b, under this run's load, the candidate's hit p95 was LL 41.6 ms (19.3–106.0) and LH 29.7 (26.8–63.7).**
  - The hit's render was 8.9–35.8 ms on LL (r58: 7.5–8.1 in eight of nine runs, 12.7 in the ninth) and 14.1–25.0 on LH (r58: 12.9–13.5).
  - Its wait was 2.7–12.8 / 4.2–9.5 ms (r58: 2.0–4.4 / 1.5–3.1).
- **A paired check against the pre-fix code** (r59c, 04:30–04:49 EDT, load 7.6–16.1): `cand-5fa62b4` (R57's code,
  the build that passed L-4a on LL in r57 and r58) against `cand-c90d062`, 3 rotated pairs per lane. Logs:
  `timing-r59c.log.gz`, `samples-r59c.log.gz`.

  | Lane | Pre-fix hit p95 per pair | Candidate | Ratio per pair → median | Wait / render mean (pre; cand) | Pre-converted; windows full (pre; cand) |
  |---|---|---|---|---|---|
  | LL | 25.8, 23.7, 23.6 | 29.3, 26.3, 26.3 | 1.14, 1.11, 1.11 → **1.11** | 4.5 / 10.4; 5.2 / 10.9 | 37; 46 · 34; 45 |
  | LH | 27.7, 26.9, 23.9 | 28.3, 27.1, 24.7 | 1.02, 1.01, 1.03 → **1.02** | 3.6 / 14.1; 3.9 / 14.0 | 48; 52 · 47; 50 |

  CPU per refill is equal on both binaries (LL 445–482 ms; LH 417–451 ms).
- **The R59 job never ran in `seek_gop60`.** A traced candidate run (LL, all three seeds,
  `r59/diag/trace-gop60-LL.txt.gz`) has 0 `Discard` events in 600 steps, against 10 admission waits. On this workload
  the candidate executes the pre-fix code path, plus the test-build counters.
- **Findings (the lead's reading, R60):**
  - **This session's absolute L-4a miss on LL is environmental.** The pre-fix build, R57's exact code, misses it
    too (23.7 ms, against 17.3 in r58). The counters say why: the hit's lavapipe render rose from 7.5–8.1 ms to 10–11
    ms on both binaries, while decode, conversion and CPU per refill are unchanged. Under R50 the miss is shared by
    identical code and attributed by the counters. **L-4a on LL stays passed on the r57/r58 evidence (17.6 / 17.3
    ms).** L-4a on LH stays as R58 records it.
  - **The candidate's LL hit p95 was 1.11× the pre-fix build's in all three r59c pairs** (about +2.6 ms; wait +0.7
    ms and render +0.5 ms on the means). The candidate lanes ran at the higher load in all three pairs (11.0, 11.4
    and 11.2 against 8.0, 10.7 and 9.9), which is an annotation, not counter evidence, and the trace rules out the
    R59 job. Amendment R60 isolated it (below).

**P-play (3 pairs; held max ms and dropped, cand against ref):**

| Lane, workload | Held max | Dropped | Present p95 |
|---|---|---|---|
| LL `typical_1080p` | 85.1 / 131.4 / 107.5 vs 175.1 / 190.0 / 95.2 | 2 / 18 / 17 vs 8 / 43 / 4 | 43.2–43.3 vs 43.1–43.3 |
| LL `feed_4x5` | 85.6 / 85.1 / 120.4 vs 85.2 / 564.8 / 123.0 | 89 / 71 / 120 vs 149 / 316 / 143 | 43.6–63.7 vs 63.9–65.2 |
| LH `typical_1080p` | 532.6 / 85.1 / 85.3 vs 85.1 / 121.8 / 339.7 | 373 / 65 / 26 vs 5 / 157 / 357 | 42.9–68.0 vs 42.8–67.6 |
| LH `feed_4x5` | 115.5 / **1247.9** / 169.1 vs **1156.6** / 116.0 / 251.8 | 385 / 507 / 614 vs 501 / 360 / 592 | 64.6–85.0 vs 64.6–84.9 |

- Every run was `valid=true`, with 0 underrun frames, 0 sync fallback frames, 0 stale errors and 0 retire
  overruns.
- The candidate's runs had at most one admission wait (≤ 2.2 ms) and 0 idle-with-discards.
- **G14 (held ≤ 100 ms) is missed on both binaries**, as in E13.5.8: in 7 of 12 candidate runs and 9 of
  12 reference runs. LH `feed_4x5` again holds a frame
  for over a second, once on each binary. It is not a pass, and it goes to S4 with the "~1 s holds" finding.

**I4 and G18, rerun.** R59 item 4 let them stand only if the fix's diff touched neither the playback nor the export
path. It touches `Readers::next` and the reader loop in `preview.rs`, which playback runs on every decode. The new
branch is reached only when a held window discards, but the diff does touch that path, so both were rerun.

**I4** (`typical_1080p`, mean ms per run; pair ratio cand/ref, median):

| Lane | ref1 / cand1 | ref2 / cand2 | ref3 / cand3 | Pair ratios, mean | Median ratio: mean / p95 |
|---|---|---|---|---|---|
| LL | 88.26 / 68.50 | 96.21 / 73.46 | 89.35 / 71.74 | 0.78, 0.76, 0.80 | **0.78** / 0.79 |
| LH | 98.36 / 85.63 | 100.81 / 73.34 | 110.52 / 198.76 | 0.87, 0.73, 1.80 | **0.87** / 0.88 |

**I4 passes** on both lanes (median ratio ≤ 1.05). The test's own baseline rule also passes on every run (−59.9%
to −86.2%). LH cand3 (198.76 ms) is the outlier, and the median uses its pair.

**G18** (export): S0 `fe87e67c…`, 3,375,562 bytes, in all three runs; the candidate's output is byte-identical in
all three. Wall time cand/S0 is 0.157, 0.178 and 0.156, median **0.157** (r58: 0.174). **G18 passes.**

**Where E13.6 closes:**
- **Resolved:**
  - the `explainer_16x9` stall: 0 timeouts in 36 candidate runs, 0 idle-with-discards in every run, and both now
    fail the lane;
  - the E13.6.1 dts mutant: recorded as equivalent on the recorded fixtures only (R61 item 8 corrects the
    scope);
  - the E13.6.4 copy cost: R54 removed it;
  - L-4b on every workload and both lanes;
  - I4 and G18, rerun at `c90d062`.
- **Recorded, not passed:**
  - L-4a on LH: *environment-limited (GPU idle clocks), re-checked at S4's pinned run* (R58), beside G3;
  - G14 on LH `feed_4x5`: shared with the reference, with the "~1 s holds" finding, for S4;
  - r58's 8,250 ms LH `feed_4x5` hold: unexplained (no counters).
- **L-4a on LL:** passed on the r57/r58 evidence (17.6 / 17.3 ms), with no R59 regression. This session's
  absolute miss is shared by the identical pre-fix code and attributed by the counters to the LL render; R60's
  three-way check reads the r59c 1.11 as noise (lead, R60).
- The E13.6.7 and E13.6.8 estimates stay estimates.

**Amendment R60: the three-way paired check (r60).** LL only, COUNT P-seek `seek_gop60`, 05:01–05:20 EDT on
2026-10-07, all 12 lanes exit 0. Three binaries, four Latin-square rotations (pre, diag, cand; diag, cand, pre;
cand, pre, diag; pre, cand, diag):
- pre: `cand-5fa62b4` (`b1cf2d9c…`, R57's code);
- diag: `diag-counters` (`b8d2fba6…`, the same code plus the R59 test-build counters);
- cand: `cand-c90d062` (`55acda05…`, the fix plus the counters).

Every lane ran with the R56 trace on (`PF1_TRACE`), the same on all three binaries, to count `Discard` events per
lane. *Correction (R61, item 5):* the r59c lanes did not write a trace, but they were not untraced. Until `2ccd995`
the harness switched the in-process trace on for every P-seek backward phase, with or without `PF1_TRACE`, on all
three of these binaries (E13.6.11). Load at lane start was 6.1–16.4 (annotation, R50). Logs:
`s2c-timing/r60/timing-r60.log.gz`, `samples-r60.log.gz`, `analyze-r60.txt` and `trace-*.txt.gz`. Per lane, the
median of its three runs:

| Lane | Load | Hit p95 (ms) | Hit mean | Wait / render mean | CPU per refill (ms) | `Discard` events | Admission waits (traced) |
|---|---|---|---|---|---|---|---|
| pre1 | 6.1 | 22.5 | 15.9 | 3.6 / 9.7 | 453 | — | 0 |
| diag1 | 8.6 | 28.0 | 19.8 | 5.3 / 11.8 | 474 | — | 17 |
| cand1 | 12.0 | 18.9 | 13.8 | 2.5 / 8.9 | 456 | 0 | 20 |
| diag2 | 9.8 | 21.7 | 15.3 | 3.7 / 9.0 | 456 | — | 16 |
| cand2 | 10.5 | 21.3 | 15.4 | 3.5 / 9.2 | 456 | 0 | 17 |
| pre2 | 11.6 | 30.2 | 19.6 | 5.7 / 11.0 | 460 | — | 0 |
| cand3 | 13.0 | 23.1 | 15.8 | 4.0 / 9.2 | 456 | 0 | 15 |
| pre3 | 15.0 | 22.9 | 16.2 | 4.4 / 9.4 | 459 | — | 0 |
| diag3 | 11.9 | 27.8 | 18.8 | 5.1 / 10.8 | 469 | — | 9 |
| pre4 | 14.5 | 24.0 | 17.5 | 4.7 / 10.0 | 460 | — | 0 |
| cand4 | 16.4 | 24.5 | 17.9 | 4.9 / 10.1 | 466 | 0 | 15 |
| diag4 | 14.6 | 23.7 | 17.1 | 4.1 / 10.5 | 462 | — | 21 |

`Discard` exists only in the candidate; pre and diag have no discard-only job. Idle-with-discards was 0 in every
lane.

Hit p95 ratios per rotation, and their median:
- **diag / pre:** 1.24, 0.72, 1.21, 0.99 → **1.10**;
- **cand / pre:** 0.84, 0.71, 1.01, 1.02 → **0.92**;
- **cand / diag:** 0.68, 0.98, 0.83, 1.03 → **0.91**.

The hit means read the same way: 1.07, 0.92 and 0.92.

**Reading (lead, R60, as corrected by R61 item 10): within noise. No regression was detected; the check cannot
resolve a 5% difference on this machine.**
- **The fix is the fastest of the three here.** cand/pre is 0.92 in r60. Over all seven pairs (r59c's three and
  these four) it is 0.71–1.14, median 1.02.
- **The work is identical:** CPU per refill is the same on all three binaries (453–474 ms).
- **The R59 job never ran:** 0 `Discard` events in 2,400 traced candidate steps (4 lanes × 3 runs × 200 backward steps).
- **The single-rotation ratios swing by ±30%,** more than R60's 1.05 threshold, so that threshold could not resolve
  anything on this machine. That was a fault in how R60 was set, not a finding.
- **diag's 1.10 against pre is not a product question.** diag is a test-only build that never ships, and the fix
  carries the same counters yet beat diag at 0.91. The diag lanes logged the most foreign CPU in every rotation
  (9,799–20,847 %·samples, against 4,336–10,903 for pre and 4,393–10,667 for cand); that is on record as an
  annotation, not used as the explanation.

**L-4a on LL stays passed on r57/r58 (17.6 / 17.3 ms), with no R59 regression.**


#### E13.6.11 Amendment R61: the stage-close fixes (2026-10-07)

Both Astra stage-close reviews (`review-s2c-a.md` on the code `cc0e8f3..8792841`, `review-s2c-b.md` on the stage and
its evidence) said do not close. The lead's ruling (`r61-rulings.md`) is Amendment R61 in the design. Logs:
`s2c-logs/s2c7/r61/`; notes: `s2c-logs/s2c7/notes-fix-round.md` (R61 section).

**The code fixes.** Each has a witness, red first, a mutation and its own commit. Per-commit gates: build, clippy
(Rust 1.99, `-D warnings`), rustfmt on the touched files, and the affected tests.

| Item | Commit | Fix | Witness | Red / mutation (killed) | Affected tests |
|---|---|---|---|---|---|
| 1 (A1, B1) | `d57087d` | `Readers::refilled` takes the refill's plan version; it clips the newer plan only if that version is current, and otherwise only the reader's own kept set | `sched::a_superseded_refill_leaves_the_newer_plan_whole` (targets 3 and 2 below the window, 8 inside it) | red without the version check: "3: still required", and "8: still required" (`item1-red.log`) | `sched::` 26 passed, `preview::` 63 passed |
| 2 (A2a) | `60b657b` | `fit_continued`: continued window times join the required set only while H + G ≤ C, nearest t first; the cut times' kept frames are discarded through a kept range [low, bound] (`discard_kept_outside`) | `sched::a_continued_window_is_shortened_to_fit_k3` (A's numbers: C = 20f, window [4, 19] kept 4–18, another source's f, G = 6f; unshortened 22f) | 2a no cut: fails at "nearest t first" (the pre-fix path, which reaches admit's K-3 debug assert); 2b cut without discarding: fails at "the cut kept frames go" (`item2-mutations.log`) | `sched::` + `preview::` 90 passed |
| 3 (A2b) | `64cea5a` | `detained` counts a detached reader's `discarding` beside its `flight` | `sched::a_detached_reader_detains_a_plan_by_its_discards` (8f of discards; a 13f plan is detained, 12f is not) | discarding left out (the pre-fix sum): "not beside the 8f it holds" (`item3-mutation.log`) | the detain/detach/fallback filter, 12 passed, 3 ignored (pre-existing) |
| 4 (A3) | `63564ae` | `KeptFrame.times`: a converted time leaves the set; a frame still owed for another time converts a copy, and its last time converts the frame itself | `preview::a_vfr_window_keeps_nothing_once_converted` (new fixture `one_vfr_source`: one decoded frame shows at two grid times) | pre-fix: window conversions fail; 4a times kept after conversion: "nothing kept once converted" (207,360 bytes, 12 times); 4b no copy: the same failures as pre-fix (`item4-mutations.log`) | `preview::` `decode::` `pf1_s2c` `render::` 146 passed |
| 5 (A5) | `2ccd995` | the P-seek backward phase traces only when `PF1_TRACE` is set | `pf1_harness::the_backward_phase_traces_only_on_request` | always trace (the pre-fix harness): "traced without PF1_TRACE" (`item5.log`) | `pf1_harness` 8 passed, 4 ignored |
| 6 (B2) | `4577ac7` | every setup and reposition seek counts toward P-seek's `timeouts`; P-play's `valid=false` fails the lane | `pf1_harness::every_p_seek_timeout_counts`, `pf1_harness::an_invalid_p_play_run_fails_the_lane` | 6a the setup phase not counted: "phase 3"; 6b any `valid=` accepted: "valid=false passed" (`item6.log`) | `pf1_harness` 10 passed, 4 ignored |

- **Item 1, the audit of the other completions.** `deliver`, `stopped`, `closed`, `exited` and `fail_start` act
  only on the reader's own decoder state, or record for the current version and a time the current job still
  requires. The discard completion releases exactly the bytes moved to the reader at the post. The window cancel is
  synchronous within the newest post. No other stale-completion clip was found. One consequence of the ruling:
  after a superseded refill, a continued window time below the decoder's `kept_from` stays required in the newer
  plan and is decoded normally, with a seek. That keeps liveness, at the cost of extra decodes in that race only.
- **Item 4, a finding.** Writing the witness exposed a pre-existing defect (since R53/R54). The managed converter's
  `source.add` moves the frame's buffers into the filter graph. A kept frame covering two window times was
  converted for its first time and kept for the second, which then failed with "managed source frame submission
  failed …: Cannot allocate memory". On the VFR fixture every second-converted time (10, 8, 6, 4, 2, 0) failed so.
  In practice A3's "uncharged decoded pixels" was an emptied frame struct. The visible defect was failed window
  conversions on VFR sources. The fix converts a copy while another time is still owed. The copy is a deep copy of
  one decoded frame, transient during that conversion, inside that time's max(f, d) hold.
- **Item 4 and the export path.** `retained` is filled only by `decode_refill` (preview refills). Export never
  keeps a frame, so `convert_retained` and `discard_kept_outside` are not reached there. The `render.rs` change only
  widens the preview's discard argument. Under the ruling, neither G18 nor I4 was rerun.
- **Item 5, which closing runs traced.** From `794caf3` (R56) to `63564ae`, `seek_run` switched the in-process
  trace on for every backward phase, with or without `PF1_TRACE`. It wrote a file only when `PF1_TRACE` was set.
  So every candidate P-seek run from R56 on traced its 200 backward steps in process: `cand-5fa62b4`,
  `diag-counters` and `cand-c90d062`, in r57, r58, r59b, r59c and r60. That puts a trace lock per event on the
  reader paths. The random, forward and drag phases were not traced. The reference binaries (`ref-ad8f896`,
  `count5-ref-ad8f896`, `s0-d19bdf9-lane2aa4e1b`) have no trace code. So on the candidate-against-reference
  comparisons the trace could only have handicapped the candidate's backward (L-4) numbers. r59c and r60 compared
  traced builds with traced builds.
- **Item 6, partial.** The call site's `setup.push(op(..))` lines are not witnessed: removing one would not fail a
  test. The guard's hardening (`r61/guard.py`) is the lead's.

**The docs (items 7–11, `43b2625`).**
- **7:** the design's S-3 and R53 text and E13.6.2 now say "published" means delivered to the preview ring, and
  that the witness shows conversion order against delivery, not against render.
- **8:** E13.6.1, E13.6.6 and E13.6.10 now say the `dts − 1` mutant is equivalent on the recorded fixtures only.
- **9:** the fixed `l5.py` was re-run over every recorded run whose plan mixed COUNT and plain lanes. Inputs were
  split-normalized only; outputs are in `s2c-timing/r61/l5/` (old = the pre-fix `l5.py`, kept as `l5-old.py`).
  - **r58** (`plan-r58.txt`: 36 COUNT and 78 other lanes) changes in one row only. LH COUNT `explainer_16x9`'s
    candidate had pooled n = 117 lines (the bleed) and now has 9. L-4b goes from 0.88 (0.88, 0.97, 0.85) to
    **0.97** (0.88, 0.97, 1.18), PASS both ways. Its pair ratios: backward mean 0.55 → 0.59, backward p95
    0.72 → 0.85, random mean 0.95 → 1.11, forward mean 0.35 → 0.40. None of these r58 `l5` numbers was cited in
    the docs.
  - **r59b:** the recorded `l5-r59b.txt` was made with the fixed `l5.py` and equals the re-run. The pre-fix
    parser would have shown LH `explainer_16x9` L-4b 1.01 with the third pair at 0.71 instead of 0.85.
  - The root `timing.log` has no COUNT lanes, so it does not change. The r5, r54a, r55m, r56, r59a, r59c and r60
    plans are COUNT-only, and the r57 plans have no COUNT lanes. The full `r5/plan-r5.txt` was mixed but never
    ran (only its COUNT subset r5a did). `r2/analyze.py` already started a lane at every `# BEGIN`.
  - **No gate result changes.** E13.6.10's "no earlier plan mixed the two kinds" is corrected in place.
- **10:** E13.6.10's R60 reading now says "no regression detected; the check cannot resolve a 5% difference on
  this machine", and that 2,400 candidate steps were traced (4 lanes × 3 runs × 200), not 1,800. LL L-4a stays
  passed under the ruling. E13.6.10 also corrects "the r59c lanes were untraced" (item 5 above).
- **11:** D12 now gives the post-R57 working-frame cost (E13.6.9). D13 and S4's row in §13 add an active
  scenario: RSS sampled during window cancellation, not only at settled idle. S-3's hold text now says that,
  after R57, reversing above the newest t to a frame never converted may need a re-decode.

**Residuals (outside the ruling; not fixed, not witnessed).**
- `unkept` shrinks the (source, time) reservations of any time a reader retained. If two readers of one source
  ever retain the same time (a refill by reader B over times that reader A still keeps unconverted), A's later
  `unkept` would shrink B's reservation to f while B still keeps its decoded frame.
- The scheduler charges an own required time inside a held window at max(f, d), but the preview's K-3 decision
  charges it f. With d > f and C < d + G, t alone could still exceed C in scheduler terms and reach admit's K-3
  debug assert.
- Item 1's consequence above (extra decodes after a superseded refill).

**The gate.**
- **The workspace fast tier** at `43b2625` (`cargo test --workspace`, nice 19, `-j 4`, `RUST_TEST_THREADS=4`):
  exit 0, 36 test binaries, **3,519 passed, 0 failed, 91 ignored** (the media library: 1034 passed, 56 ignored)
  (`workspace-fast.log`).
- **The slow-test lint:** "slow-test manifest and markers agree: 47 tests, features
  kinewright-agent/slow-tests,kinewright-app/slow-tests,kinewright-media/slow-tests; 44 allowlist entries
  well-formed" (`slow-lint.log`).
- **The proportionate re-time (r61),** 06:34–07:28 EDT, `s2c-timing/r61/plan-r61.txt`. It ran 24 COUNT P-seek
  lanes: `seek_gop60` and `explainer_16x9`, LL and LH, 3 pairs each, reference then candidate. The binaries were
  `count5-ref-ad8f896` (`bd5f14b5…`) and `cand-43b2625` (`0225653b…`). Every lane exited 0; three LH reference
  lanes printed the reference's known EGL teardown panic after their results. The R61 guard ran every 30 s:
  `guard --final`: 12 candidate lanes checked, **0 trips**. Load at lane start was 8.0–18.5 (median 11.8,
  annotation). Logs: `timing-r61.log.gz`, `samples-r61.log.gz`, `l5-r61.txt`, `table-r61.txt`, `analyze-r61.txt`.

  | Lane, workload | **L-4b** (pairs) | Backward mean ratio | Cand hit p95 per pair (ms) | Hit wait / render mean (ms) | Admission waits per run; longest | Timeouts; idle with discards |
  |---|---|---|---|---|---|---|
  | LL `seek_gop60` | **1.10** (1.10, 0.98, 1.11) | 0.61 | 23.3, 21.8, 20.8 | 3.7–4.4 / 9.0–9.8 | 4–8; 6.3 | 0; 0 |
  | LL `explainer_16x9` | **0.91** (0.91, 0.89, 0.95) | 0.55 | 21.8, 26.9, 32.4 | 2.2–4.2 / 11.0–14.9 | 51–61; 13.0 | 0; 0 |
  | LH `seek_gop60` | **1.06** (0.96, 1.06, 1.07) | 0.63 | 24.5, 25.9, 24.5 | 2.6–3.6 / 13.3–13.6 | 7–21; 4.5 | 0; 0 |
  | LH `explainer_16x9` | **0.98** (0.98, 0.88, 0.99) | 0.60 | 34.5, 30.9, 32.5 | 1.3–2.5 / 17.3–18.1 | 53–61; 9.8 | 0; 0 |

  L-4b passes on every lane (≤ 1.25). The candidate never timed out and never idled holding discards (36 runs).
- **The paired L-4a non-regression check (r61b; the lead's R61 item 12),** 07:30–07:43 EDT, LL COUNT P-seek
  `seek_gop60`. It compared `cand-c90d062-notrace` (`605e02b2…`: `c90d062` plus `2ccd995`'s test-only trace gate)
  with `cand-43b2625` over 4 rotations, order alternated, with no `PF1_TRACE` on either. All 8 lanes exited 0, and
  the guard reported 0 trips. Logs: `timing-r61b.log.gz`, `samples-r61b.log.gz`, `table-r61b.txt`.

  | Lane | Load | Hit p95 per run (median) | Hit mean | Wait / render mean | CPU per refill (ms) | Foreign CPU (%·samples) |
  |---|---|---|---|---|---|---|
  | pre1 | 10.0 | 18.7, 25.4, 23.3 (23.3) | 16.4 | 4.1 / 9.6 | 448.8 | 3,672 |
  | cand1 | 7.4 | 24.1, 18.7, 40.8 (24.1) | 17.4 | 3.7 / 9.6 | 464.1 | 4,048 |
  | cand2 | 12.6 | 53.6, 38.1, 22.8 (38.1) | 23.1 | 5.7 / 14.7 | 472.3 | 6,050 |
  | pre2 | 16.2 | 20.3, 20.5, 32.6 (20.5) | 14.7 | 3.4 / 8.9 | 458.0 | 4,157 |
  | pre3 | 17.0 | 26.2, 27.6, 24.7 (26.2) | 19.4 | 5.4 / 11.1 | 461.2 | 6,381 |
  | cand3 | 14.2 | 30.4, 23.4, 21.6 (23.4) | 17.0 | 4.7 / 9.9 | 455.6 | 5,403 |
  | cand4 | 14.3 | 23.3, 27.6, 27.0 (27.0) | 18.8 | 4.9 / 11.4 | 463.4 | 5,019 |
  | pre4 | 14.9 | 23.4, 21.2, 25.9 (23.4) | 15.7 | 3.9 / 9.3 | 465.1 | 3,879 |

  Cand / pre per rotation:
  - hit p95: 1.03, 1.86, 0.89, 1.15, median **1.09**; the candidate was faster in 1 of 4;
  - hit mean: median 1.13;
  - wait: 1.08;
  - render: 1.11;
  - CPU per refill: 1.03, 1.03, 0.99, 1.00, median 1.01.

  How many continued windows `fit_continued` shortened is **not measured**: there is no counter, and none was
  added (the lead's instruction). **The lead's no-regression condition is not met** (median ≤ 1.05 or faster in
  most rotations, equal CPU, 0 windows shortened), so the run stopped here for the lead.

#### E13.6.12 Amendments R62–R64: one charge rule and the accounting model (2026-10-07)

The lead stopped R61's re-time on item 12 and ruled R62 (`r62-rulings.md`): timing non-regression is reported, not
gating; work-counter equivalence is the gate. R62 fixed four residuals and asked for a seeded state-machine test of
the scheduler's accounting (item 9). That model found seven new cases (F1–F7, R63) and then F8 (R64). The model is
now the gate for scheduler accounting. Logs: `s2c-logs/s2c7/r62/`; notes: `notes-fix-round.md` (R62, R63, R64).

**R62 items 1–8.** Each has a red-first witness, a mutation and its own commit (`r62/item*-*.log`).

| Item | Commit | Fix | Witness |
|---|---|---|---|
| 1 | `29dc0b1` | `unkept` shrinks a time only when no reader still keeps it | `sched::a_time_two_readers_keep_shrinks_only_when_the_last_drops_it` (re-set outside a held window at F5) |
| 2 | `a87b783` | one charge rule: `sched::charge` (max(f, d) inside a held window below its t, f otherwise); K-3 fits a paused job by `job_bytes` | `sched::a_hit_in_a_held_window_is_charged_by_k3_as_admission_charges_it` |
| 4, 8 | `94478d8` | the harness call sites (`setup.push`, the trace request, the P-play check) are witnessed | `pf1_harness::every_p_seek_op_timeout_fails_the_lane`, `the_backward_phase_traces_only_on_request`, `a_p_play_run_is_checked_where_the_lane_runs_it` (10 mutations) |
| 5 | `c52403d` | a continued window fits in the room detached readers leave (`Readers::room`, one function for the fit and detention) | `sched::a_continued_window_fits_beside_a_detached_reader` |
| 6 | `ff701bd` | a dispatched discard stays charged to its reader (`flight`) | `sched::a_dispatched_discard_stays_charged_to_its_reader` |
| 7 | `1482372` | a failed result first drops the decoder's kept frames | `preview::a_failed_window_conversion_leaves_nothing_kept` (re-set to the charge rule at F5, R64 item 2) |

- **Item 3 (D14, not fixed).** After a refill superseded mid-decode, the newer plan's continued times below the
  decoder's `kept_from` stay required though no reader keeps them: the reader decodes them again with one seek, at
  most B − 1 = 15 times plus the GOP pre-roll, once per superseded refill. Added to §13 as D14 (S3 backlog).
- **Item 4/8, partial.** `BackwardPhase::new`'s `traced: trace_requested()` is checked against `PF1_TRACE` only in
  the environment the tests run in (unset): a constant `true` is caught, `false` is not.
- **Items 3, 4 and 8 are outside a scheduler model**; their own witnesses cover 4 and 8.

**The model (R62 item 9, R63 item 4, R64): `sched::accounting`** (`crates/kinewright-media/src/sched_accounting.rs`).
Plain Rust, no dependency, no thread or clock. Each sequence is one seed's random walk of 64 operations over what
the preview and its readers do: the post (paused steps in and out of a held window, playback, a second playhead),
K-3's plan and fallbacks, admission, cache clears and source removal, R47's detach of a reader stalled past its
retirement, and every reader's `Next` acted on outside the lock, with the decoder's kept frames in the decoder's own
bookkeeping (`KeptFrames`, shared with `VideoDecoder` since the R63 refactor `4101e96`). Source 0 is VFR (one
decoded frame shows at two grid times, d > f), source 1 CFR. After every operation it checks:
- **I1** (K-1): live is exactly what the owners hold (ring, reservations, discards, decodes and discards in flight,
  the admitted G), live ≤ C, and (R64, exact) each reservation is its charge by the one charge rule, above it only
  while a reader keeps the time decoded (at most max(charge, d));
- **I2** (K-1): a kept time the plan requires is charged (reserved at max(f, d) while its slot keeps it, or riding
  its discard or decode); a kept time no plan requires is D13's gap, bounded below B per reader and counted;
- **I3** (K-3): an admitted job's H + G fits C less the detached readers' charges;
- **I4** (R59): no reader idles holding discard charges or a flight;
- **I5** (progress): every required frame is resolved, in flight, planned or waiting; every 8 operations and at the
  end a clone runs to rest with the detached readers stalled, the newest job resolves, and once they return nothing
  stays charged but the rings and G;
- **I6** (R64): no time is decoded while another live reader keeps it, but on B's counted path; and after each post
  a live keeper of times the plan wants has some of them in its plan unless each went to another keeper.

Seven weighted profiles (Uniform ×2, Holders, Discards, Detained, Stops, Kept) bias the generator (R63 item 4).
`PF1_MODEL_SEQUENCES`, `PF1_MODEL_SEED`, `PF1_MODEL_TRACE`, `PF1_MODEL_SURVEY` and `PF1_MODEL_EMIT` (a seed written
out as a fixed case's steps) drive exploration. Fixed cases (`fixed`) replay explicit operation sequences, each
step checked as a seed's are, so a later generator change cannot move them.

**What the model found, and the fixes (R63, R64).** Each has a red-first witness, a mutation and its own commit;
per-commit gates as in R61. The F numbers are the order found.

| Fix | Commit | Seed found | The case | Fix | Witness |
|---|---|---|---|---|---|
| F2 | `947fb3c` | 480 | a refill kept window times the ring already held: never converted, never charged, until the reader's next decode | at dispatch a refill keeps only its window less the ring's times (`Readers::retaining`, `decode_refill(.., retain, ..)`); the decode is unchanged (R54); `KeptFrames::cover` keeps R54's `kept_from` | `sched::a_refill_keeps_none_of_the_rings_times`; the model's F2 exemption is gone |
| F7 | `c43987f` | 2008 | a playback post required a time a reader keeps (reserved max(f, d)); K-3 counted it at f, admission could never fit the set (I5) | option A: `job_bytes`/`Readers::job_set` count a kept frame at max(charge, d), paused and playback; t + G > C is K-3's existing synchronous fallback (§5 K-3 [S2b-3], R15; I12) | the model's fixed case `a_kept_required_frame_counts_at_its_held_charge_in_k3` (seed 2008's scenario, reduced) |
| F8 | `26a560c` | 16102 | a playback region with no kept time took the only free reader, and a new reader decoded a time the first still kept (charge consumed, I2) | R64 keeper affinity (`Readers::keepers`), B as fallback (`Readers::hand_off`, counted `kept_handoffs`) | `sched::a_region_goes_to_the_reader_that_keeps_its_times`, `sched::a_kept_time_its_keeper_cannot_take_is_discarded_and_counted`, the model's fixed case `a_region_goes_to_the_reader_that_keeps_its_times` |
| F1 | `50b3080` | 225 | a reservation carried from a playback plan at f stayed below its charge once a paused post held a window over it; the refill kept d against f | `post_within` returns a reservation below its charge; admission reserves it again at the charge | the model's fixed case `a_carried_reservation_below_its_charge_is_made_again_at_it` (seed 11, reduced to 2 steps) |
| F3 | `0310165` | 835 | `discard_outside` moved a retiring reader's kept frames' reservations onto it after detention decided (I3) | it skips a retiring reader (the guards in `continued` and `backward`, never caught alone, were dropped) | the model's fixed case `a_retiring_reader_is_given_no_discard_after_detention` (seed 674, reduced to 15 steps) |
| F4 | `9697cf3` | 2686 | a refill's t, reserved (carried), dispatched while admission still waited; its window times were unreserved, so it kept them uncharged | `work_for` holds a refill's t until every window time is reserved or resolved | `sched::a_refill_waits_until_its_window_is_reserved` (red first at this commit); the model's fixed case `a_refill_never_keeps_its_window_uncharged` (seed 3123 of 30k, reduced to 8 steps; lands with F11, which it also needs) |
| F5 | `12db818` | 2961 | `unkept` shrank a held window's reservations to f, below the charge rule; a reopened reader's refill kept d against f | `unkept` shrinks to `frame_bytes` | `sched::a_dropped_time_inside_the_held_window_keeps_its_charge`; item 1's witness re-set outside a held window; item 7's re-set to the charge rule |
| F6 | `c80bbcc` | 1273 | the read loop's stopped path did not settle the decoder: it kept raw frames the scheduler had let go | the stopped path drops the kept frames first (item 7's rule), mirrored in the model | `preview::a_stopped_window_conversion_leaves_nothing_kept` |
| F9 | `645c41d` | 14917 | `exited` dropped a retiring slot's kept times with their reservations still max(f, d) | exit releases them through `unkept` (R64 item 5) | `sched::an_exiting_reader_returns_its_kept_frames_charges` |
| F10 | `ef82c29` | 167 | a superseded refill clipped its kept times below `kept_from` without shrinking their reservations | the clipped times go through `unkept` | `sched::a_superseded_refill_returns_what_it_does_not_keep` |
| F11 | `a00d1e4` | 5 | a post that no longer held a window left reservations made inside it at max(f, d), kept by nobody; admission waited on them | `post_within` lowers a reservation above its charge (max(charge, d) while a reader keeps the time) | `sched::a_post_returns_a_reservation_above_its_charge` |
| F12 | `8d3dc4e` | 106741 (the 300k slow tier) | a busy reader's pending discard dropped a kept time (an earlier post's); a later post gave that time to another reader, which decoded it while the first decoder still kept it, uncounted (I6) | B's path covers it: a reader tracks the times its pending discard drops (`dropping`, cleared when it serves the discard, closes, fails or stops), and `hand_off` hands off any another region wants, counted when decoded again. Only counting changes: no reader's work or reservation does | the model's fixed case `a_time_a_busy_readers_discard_drops_is_counted_when_decoded_again` (seed 106741, reduced to 9 steps) |

**The slow tier (`a973773`).** `sched::accounting::the_readers_accounting_holds_for_300k_seeded_sequences` runs the
same model over 300,000 seeds (four threads), under `slow-tests`, registered in `ci/slow-tests.txt` with its marker
(`slow_tests.py lint`: agree, 48 tests). Its first run failed at seed 106741 (I6), which became F12; after F12 it
is green in 85 s locally (`r64-slow-300k.log`, `r64-slow-300k-2.log`).

**The fast tier and the lint (R62 gate).** `cargo test --workspace` at `a973773` (nice 19, `-j 4`,
`RUST_TEST_THREADS=4`, `TMPDIR` on disk): exit 0, 36 test binaries, **3,542 passed, 0 failed, 92 ignored** (the
media library: 1,057 passed, 57 ignored) (`r64-workspace-fast.log.gz`). Two earlier attempts were cut off by the
machine (a SIGKILL of the media binary, then a T3 restart) and are kept as `-1-sigkill` and `-2-cutoff`. `slow_tests.py lint`: agree, 48 tests
(`r64-lint-final.log`).

**The gate (R62, R63 item 5): stopped on R62's 5% band, then PASSED under Amendment R65 (below).** Counter equivalence, LL COUNT P-seek,
`cand-43b2625` (`0225653b…`, pre) against `cand-a973773` (`ea911ffd…`), tracing off, 2 rotations per workload,
alternated. Rotation 2 first failed on the machine, not the code: `/tmp` (tmpfs) returned "Disk quota exceeded" while
the fixtures were generated, and three lanes exited 101/137 before any result. It was re-run as r64b with `TMPDIR`
on disk. Logs: `s2c-timing/r64/` (`timing-r64-gate.log` = r64's rotation 1 + r64b; `table-r64.txt`). Every lane of
the gate exited 0; `guard --final`: 4 candidate lanes, 0 trips. Load at lane start 19–25 (others' builds; annotation).

| Workload | Decodes (refill + run) | CPU per refill | Hits pre-converted | Windows filled | Admission waits | Timeouts; idle with discards (cand) | Hit p95 (info) |
|---|---|---|---|---|---|---|---|
| `seek_gop60` | 1.000, 1.000 → **1.000** | 1.039, 1.019 → **1.029** | 0.968, 0.972 → **0.970** | 1.047, 1.073 → **1.060** ✗ | 1.000, 1.333 → **1.167** ✗ | 0; 0 | 1.21, 1.04 |
| `explainer_16x9` | 0.999, 0.995 → **0.997** | 1.020, 0.966 → **0.993** | 1.022, 0.960 → **0.991** | 0.959, 1.044 → **1.001** | 0.963, 1.000 → **0.982** | 0; 0 | 2.13, 0.89 |

(cand / pre per rotation, each the median of the lane's 3 runs → the median over rotations.)

- Decodes on `seek_gop60` are identical run for run in the gate's two rotations (12,793 / 12,712 / 12,653 on both
  binaries, and the same refill and hit counts). *Corrected under Amendment R66 item 5:* they are not identical in
  the diagnostic rotations (r64c): pre3's second run decoded 12,711 and pre4's 12,916, against cand's 12,712 in
  both. The workload is close to deterministic, not exactly so. `kept_handoff_redecode` is 0 in every candidate
  run.
- Windows filled and admission waits vary run to run within each binary (windows 39–46, waits 2–7 on `pre`), as
  conversion races the steps. With 3–5 waits a run, one more wait is a 20–33% change.
- **Diagnostic only, beyond the gate (r64c):** two more `seek_gop60` rotations (`plan-r64c.txt`, guard 0 trips, load
  31–38). Over the four rotations: decodes 1.000, CPU per refill 1.003, pre-converted 1.003, windows filled
  **1.012**, admission waits **1.167** (1.00, 1.33, 0.67, 1.67; pre's runs 2–7, mean 3.75; cand's 3–5, mean 4.08).
  Consistent with timing noise at these counts, but not shown to be; F4 (a refill's t waits for its window's
  reservations) could add admission waits. `table-r64-diag.txt`.
- **Amendment R65 (the lead, `r65-rulings.md`): equivalence for small counters; the gate is PASSED.** A ratio
  band cannot work on a counter of 2–7 a run (one wait moves a rotation by 25–33%). From now on (R62's gate and
  the S3/S4 counter gates): a counter whose pre median is ≥ 20 a run keeps the band (median cand/pre within 5%); one
  under 20 passes when the pooled per-run means differ by at most max(1, 25% of pre's mean) and cand's per-run
  range overlaps pre's; a counter that must be 0 stays 0. `seek_gop60` admission waits, pooled over the four
  rotations (the last two diagnostic): cand 49 waits in 12 runs (**4.08** a run, range 3–5), pre 45 in 12
  (**3.75**, range 2–7): a difference of 0.33, ranges overlapping, so it passes. *Stated plainly under Amendment
  R66 item 5:* `seek_gop60` admission waits went from 45 to 49 over 12 runs, and that rise is not attributed (to
  F4 or to timing); R65's rule is a tolerance, not proof that the work is unchanged. S4's pinned run re-checks
  admission waits with more runs. Windows filled stays on the band
  and passes at 1.012 over the four rotations. F4's possible waits do not show where waits are many:
  `explainer_16x9`'s, about 54 a run, are at 0.98. Decodes match run for run, and timeouts, idle with discards and
  `kept_handoff_redecode` are 0.

**The revert table (R63 item 4, R64 item 7), on the final code (`a973773`).** Each mutation reverts one fix or
item in the final tree (`mut.py`, archived in `s2c7/r63-tooling.tar.gz`). "3k" is the fast tier's `sched::accounting` filter (the
seeded 3,000 and the fixed cases): what fails. "30k" is the survey of 30,000 seeds (`PF1_MODEL_SURVEY`): failing
seeds (the lowest failing seed). Log: `r62/r64-revert-final.txt` and `r64-revert-final-*.log`.

| Reverted | 3k (fast tier) | 30k: seeds failing (lowest) | Targeted witness |
|---|---|---|---|
| A1 (R61) | seeded + fixed F12 case | 775 (9) | `sched::a_superseded_refill_leaves_the_newer_plan_whole` |
| A2a (R61) | seeded + fixed F4/F11 case | 877 (8) | `sched::a_continued_window_is_shortened_to_fit_k3` |
| A2b (R61) | seeded | 18 (347) | `sched::a_detached_reader_detains_a_plan_by_its_discards` |
| A3 (R61) | seeded + 3 fixed cases | 8,754 (2) | `preview::a_vfr_window_keeps_nothing_once_converted` |
| item 1 | **not caught** | 0 | `sched::a_time_two_readers_keep_shrinks_only_when_the_last_drops_it` (R64 item 3, below) |
| item 2 | seeded | 576 (173) | `sched::a_hit_in_a_held_window_is_charged_by_k3_as_admission_charges_it` |
| item 5 | seeded | 14 (895) | `sched::a_continued_window_fits_beside_a_detached_reader` |
| item 6 | seeded | 19 (486) | `sched::a_dispatched_discard_stays_charged_to_its_reader` |
| item 7 | seeded | 333 (107) | `preview::a_failed_window_conversion_leaves_nothing_kept` |
| F1 | seeded + fixed | 114 (11) | fixed case (seed 11) |
| F2 | seeded | 66 (187) | `sched::a_refill_keeps_none_of_the_rings_times` |
| F3 | seeded + fixed | 12 (674) | fixed case (seed 674) |
| F4 | fixed case only | 5 (3123) | `sched::a_refill_waits_until_its_window_is_reserved`; fixed case (seed 3123) |
| F5 | seeded + fixed | 2,071 (5) | `sched::a_dropped_time_inside_the_held_window_keeps_its_charge` |
| F6 (model side) | seeded | 37 (270) | — |
| F6 (read loop) | not caught (outside the model) | 0 | `preview::a_stopped_window_conversion_leaves_nothing_kept` |
| F7 | fixed case only | 0 | fixed case (seed 2008) |
| F8 (A) | seeded + fixed | 128 (11) | `sched::a_region_goes_to_the_reader_that_keeps_its_times`; fixed case (seed 16102) |
| F8 (B, `hand_off` removed) | fixed F12 case | 0 | `sched::a_kept_time_its_keeper_cannot_take_is_discarded_and_counted` |
| F9 | **not caught at 3k** | 2 (14917) | `sched::an_exiting_reader_returns_its_kept_frames_charges` |
| F10 | seeded + fixed F12 case | 81 (167) | `sched::a_superseded_refill_returns_what_it_does_not_keep` |
| F11 | seeded + fixed | 9,253 (5) | `sched::a_post_returns_a_reservation_above_its_charge` |
| F12 | fixed case only | 0 | fixed case (seed 106741) |
| none | green (3.3 s here) | 0 | — |

- The witnesses of F1–F12 and of items 1 and 7 were shown red under these mutations on the final code
  (`r64-witness-summary.txt`, `r64-F12-witness-F12.log`). Those of A1–A3 and items 2, 5 and 6 are the R61/R62
  witnesses, red at their own commits; they were not re-run under these mutations.
- **The R63 bar** (A1, A2a, A2b, A3, items 1, 2, 5, 6, 7, F1, F3, F4, F5, F6, F7, each caught by the fast tier)
  is met but for item 1, which R64 item 3 accepts: the generator cannot reach two readers keeping one time while
  one of them drops it (a refill keeps only what the ring lacks, F2, and a region goes to its keeper, F8), so
  `unkept`'s "no other reader still keeps it" test is never decisive. Its deterministic witness stays.
- **F4 and F7 (R64 item 4).** A profile weight was tried first: Holders ×3 at 3k caught neither (0 and 0,
  `r64-weight-holders3-*.log`), so both took the fixed-case route. F7: seed 2008's scenario. F4: no seed in 30k
  fails on F4 alone at F4's commit (all 16 also need F11), and 20,000 fuzzed variants of seed 3123 found none; so
  F4's red-first witness is the unit test, and the seed-3123 case (reduced to 8 steps) lands with F11.
- F9, F12 and F8's B path are caught at 3k only by fixed cases or not at all: their witnesses are deterministic.
- 3k model time on the final code: 3.3–5.6 s under load 20–40 (target ≈ 3 s).

**Rulings and open items (R65).**
- **Accepted (R65):** F10 and F11 (the one charge rule) and F12 (counting only, on R64's B path; the replay shows
  the same scheduling) as same-rule fixes; the model's fast-tier time, 3.3–5.6 s under load (2.63 s on a quiet
  run) against ≈ 3 s.
- **Deferred (R65): D15.** `sched::tests::the_reader_model_holds_for_every_short_sequence` (about 120 s in the
  fast tier, from S2b `72246d3`) goes to the S3 backlog: to the slow tier, or shrunk.
- **F8's reading:** a keeper mid-decode stays eligible for affinity; a retiring or detached keeper's kept times
  stay with it (F3, D13), so B's discard never reaches them.
- **Open, not blockers (R65): F11's gate.** `preview::` twice hit the known Vulkan-loader SIGSEGV (in
  `vkCreateInstance`), and one run of `a_backward_drag_matches_fresh_seeks_and_refills_once_per_window` failed
  with its details lost; 3 solo and 6 full reruns passed. Not reproduced.

#### E13.6.13 Amendments R66–R67: the one charge rule on three more paths, and the re-gate (2026-10-07)

Astra's re-review of R65 (`target/review/pf/rereview-r65.md`) found three more paths that missed R63's one charge
rule. The R66 ruling took them as same-rule fixes and made the model's oracle independent of the scheduler's
helpers. Every fix has a red-first witness, a fast-tier (3k) mutation or an explicit fixed case, and its own
commit. The per-commit gates are build, `clippy -D warnings --all-targets -p kinewright-media`, rustfmt on the
touched files, and the affected tests. Because of the desktop crash, all CPU work ran on the laptop (`-j4`, 4 test
threads, nice). The counter gate ran on the desktop, alone. Logs are in `s2c-logs/s2c7/r66/` (the laptop's copies
in `laptop/`) and `s2c-timing/r66/`.

**The commits (on `ae0cd3b`).**

| Commit | What | Red-first witness (red, then green) |
|---|---|---|
| `29ed1d7` F13 | `admit` and recharge reserve a missing required time a reader still keeps at max(charge, d) (`kept_charge`), as K-3 counts it, not at f. Model: D13's exemption ends once a plan requires the time again and admission reserves it | fixed case `a_kept_time_required_again_is_admitted_at_its_kept_charge` (Astra's scenario: A keeps 37 at d = 3f, playback at 25 releases 37, playback requires it again before A advances): red I2 on `ae0cd3b` and with the fix alone removed from the final tree |
| `3754ad4` F14 | a discard-only job's dropped times stay tracked (`dispatched`) until the reader is back from serving it, and go on close, failure and stop; a post made meanwhile hands them off, and a redecode is counted. Model: an exact handoff-redecode count per time against its own handoff record, not "the counter moved" | fixed case `a_time_a_dispatched_discard_drops_is_counted_when_decoded_again` (A's discard of 37 dispatched, A suspended before it is served, regions 25 and 37 posted): red I6 exact, counted 0 against 1 |
| `6336c4d` (found by F14's exact count, seeds 3325, 868 and 3517) | `refilled` clips a retiring reader's refill, and the times a pending discard drops, to what its decoder kept (`kept_from`); a retiring reader's failed result keeps nothing | `a_retired_readers_refill_keeps_only_what_its_decoder_kept`: red I6 exact. `a_refill_clips_the_times_its_pending_discard_drops`: red I6 exact on 26. Its first version was vacuous (green with the clip removed, since no discard was pending) and was re-set with a paused in-window step beside G = 55f |
| `34955fa` (F13 on R61's fit; seed 29956 at 30k, I5) | `fit_continued` counts the base set and each level at `kept_charge`, so a continuation never fits that admission cannot admit | `a_continued_window_fits_beside_a_kept_time_at_its_kept_charge` (the continuation is cut to 8–18): red I5 |
| `83bf318` F15 | `fail_start`, which a panic exit calls (R43), releases the slot's kept times through `unkept`, as `exited` does (F9). Model: a panic outcome for a decoding reader that goes through `fail_start` (`Outcome::Panic`, `panicked`); the run asserts it is reached | fixed case `a_panicking_readers_kept_frames_return_to_their_charge` (A panics serving 36 with 36 and 37 kept at 3f): red I1, 37 reserved 30 against its charge of 10 |
| `a45d09b` item 4 | the model's oracle: I1's charges come from the model's own held windows (set at a planned post, cleared at a fallback and a clear, clipped on a current refill; asserted equal to the scheduler's), f, each kept frame's d and which reader keeps which time. I3's H is the sum of the model's charges over the required set, then cross-checked against the scheduler's figure. No check calls `frame_bytes` or `required_bytes` for its expected value | 3k and 30k clean |
| `25cff0a` | unit witness of `6336c4d`'s failed-result path, a race the model cannot reach (the read loop checks the stop flag outside the lock, so a retiring reader can deliver an Err) | `sched::tests::a_retiring_readers_failed_refill_keeps_nothing`: red "nothing kept" |
| `62a29b1` | three fixed cases reduced by ddmin, for the rows the oracle change cost the 3k column (below) | `a_detached_readers_discard_charges_count_in_the_room` (seed 5580, 10 steps, A2b); `a_continued_window_fits_beside_a_detached_readers_charges` (seed 4156, 9 steps, item 5); `a_detached_readers_dispatched_discard_counts_in_the_room` (seed 3413, 22 steps, item 6) |

Each red was shown with the fix's lines alone removed from the final tree (`r66-*-red*.log`), and each witness is
green at its commit and on `62a29b1` (`r66-cases-green-final.log`; gate `r66-gate-R1.log`: 57 passed, fmt and
clippy clean). `6336c4d`, `34955fa` and `25cff0a` go beyond the ruling's three blockers. R67 accepts them as
same-rule fixes: R64's superseded-refill clip applied to a retiring reader, and F13 applied to R61's fit.

**The revert table on the new oracle.** The 3k column was re-run for every row: on `a45d09b` for every row, then on
`62a29b1` for A2b, items 5 and 6, and the failed-result path. `25cff0a` and `62a29b1` add tests only, so they can only
add catches to the `a45d09b` rows. The 30k column was re-run for each row whose 3k result changed and for each new
row. Other rows keep R64's 30k figures, which were measured on R64's walk. The panic outcome and the held-window
records shift every seed's walk, so seeded counts are not comparable across R64 and R66. Logs:
`laptop/mut/r66-3k-*.log`, `r66-3k-b-*.log` and `r66-30k-*.log`, with their summaries.

| Reverted | 3k (fast tier) | 30k: seeds failing (lowest) | Against R64 |
|---|---|---|---|
| A1 (R61) | seeded + 4 fixed | 775 (9) (R64) | — |
| A2a (R61) | seeded + 4 fixed | 877 (8) (R64) | — |
| A2b (R61) | fixed case only (on `a45d09b`: **not caught**) | 10 (5580) | was seeded |
| A3 (R61) | seeded + 4 fixed | 8,754 (2) (R64) | — |
| item 1 | **not caught** | 0 (R64) | as before (R64 item 3) |
| item 2 | seeded | 576 (173) (R64) | — |
| item 5 | fixed case only (on `a45d09b`: **not caught**) | 11 (4156) | was seeded |
| item 6 | fixed case only (on `a45d09b`: **not caught**) | 8 (3413) | was seeded |
| item 7 | seeded | 333 (107) (R64) | — |
| F1 | seeded + fixed | 114 (11) (R64) | — |
| F2 | seeded | 66 (187) (R64) | — |
| F3 | seeded + fixed | 12 (674) (R64) | — |
| F4 | fixed case only | 5 (3123) (R64) | — |
| F5 | seeded + fixed | 2,071 (5) (R64) | — |
| F6 (model side) | seeded | 37 (270) (R64) | — |
| F6 (read loop) | not caught (outside the model) | 0 (R64) | as before |
| F7 | seeded (25, lowest 66) + fixed | 158 (66) | was fixed case only |
| F8 (A) | seeded + 2 fixed | 128 (11) (R64) | — |
| F8 (B, `hand_off` removed) | seeded + 3 fixed | 515 (55) | was the F12 fixed case only |
| F9 | seeded (24, lowest 55) | 212 (55) | was not caught at 3k |
| F10 | seeded + fixed | 81 (167) (R64) | — |
| F11 | seeded + fixed | 9,253 (5) (R64) | — |
| F12 | seeded (4, lowest 1417) + 2 fixed | 39 (1417) | was fixed case only |
| F13 | seeded + 2 fixed | 1,716 (5) | new |
| F13 on the fit (`34955fa`) | fixed case only | 1 (7952) | new |
| F14 | seeded (1, seed 2319) + fixed | 1 (2319) | new |
| F15 | seeded (5, lowest 184) + fixed | 43 (184) | new |
| retiring refill clip (`6336c4d`) | seeded + fixed | 18 (868) | new |
| pending-discard clip (`6336c4d`) | fixed case only | 3 (17816) | new |
| retiring failed result (`6336c4d`) | not caught (outside the model) | 0 | new; witnessed by `25cff0a` |

- **The bar.** Every R63 row and every R66 row is caught by the fast tier, except:
  - item 1, which R64 item 3 accepted, as before;
  - F6's read-loop side and the retiring failed result, which are outside the model. Each has a deterministic
    witness.
- **The cost of the oracle change.** On `a45d09b` the seeded 3k walk no longer caught A2b, item 5 or item 6. Each
  is now caught at 3k by a fixed case that ddmin reduced from its lowest 30k seed (`62a29b1`).
- **Coverage.** In random seeds `reach.handoffs` (a live keeper of a time another reader decodes) is 0 at 30k; only
  the fixed cases reach it. Counted redecodes are reached 437 times at 30k.

**The slow tier.** `the_readers_accounting_holds_for_300k_seeded_sequences` at `62a29b1`, on the laptop (4 threads,
nice, others' load about 45): green, 199.28 s test time, 228 s wall (`laptop/r66-slow-300k.log`).

**The fast tier, partial.** Only the media library was re-run (`cargo test -p kinewright-media --lib`, 4 threads, at
`62a29b1`, laptop): 1,067 passed, 0 failed, 57 ignored, 608 s (`laptop/r66-media-lib.log.gz`). The workspace fast
tier was not re-run here; it belongs to the push gate.

**The gate (R66 item 6.4): PASSED under R65 and R67, with a behaviour change recorded.** The R62 counter gate was
run again: LL COUNT P-seek, `cand-43b2625` (`0225653b…`, pre) against `cand-62a29b1` (`d20efde4…`, built at
`62a29b1` with only docs uncommitted), tracing off, 2 rotations per workload, alternated (`plan-r66.txt`). It ran
on the desktop from 20:10:56 to 20:23:54 with no other desktop cargo work. All 8 lanes exited 0, and
`guard --final` checked 4 candidate lanes with 0 trips. Load at lane start was 0.17–10.1, with others' processes
annotated (R50). Logs: `s2c-timing/r66/timing-r66.log`, `table-r66.txt`, `samples-r66.log.gz`.

(cand / pre per rotation, each the median of the lane's 3 runs → the median over rotations. R65 applies the 5% band
where pre's median is ≥ 20 a run, and the small-counter rule below that.)

| Workload | Decodes (refill + run) | CPU per refill | Hits pre-converted | Windows filled | Admission waits | Timeouts; idle with discards (cand) | Hit p95 (info) |
|---|---|---|---|---|---|---|---|
| `seek_gop60` | 1.000, 1.000 → **1.000** | 0.993, 0.976 → **0.984** | 0.981, 1.114 → **1.047** | 1.057, 1.125 → **1.091** (R67) | pre 9.5 a run < 20: pooled cand 11.17 (67 in 6), pre 9.50 (57 in 6), difference 1.67 ≤ 2.38, ranges 9–15 and 4–16 overlap → **pass** | 0; 0 | 0.99, 0.82 |
| `explainer_16x9` | 0.997, 1.002 → **1.000** | 0.986, 1.071 → **1.029** | 1.096, 0.949 → **1.023** | 1.059, 0.990 → **1.025** | 0.966, 0.949 → **0.957** | 0; 0 | 0.95, 1.20 |

- **`seek_gop60` windows filled: outside R65's band, accepted under R67.**
  - Per run, cand had 52, 56, 60, 54, 52 and 60 (pooled mean 55.67) and pre had 53, 52, 59, 55, 48 and 44 (51.83):
    +7.4%.
  - **Accepted as an explicit waiver, not a causal attribution.** The counter counts a refill that keeps nothing
    and a window whose conversion queue empties; it is not a budget-full stop (Astra's R67 re-check corrected the
    lead's first reading). F13 changes what admission reserves, so it can plausibly move the counter, but no run
    isolates F13, and the counts here neither establish that cause nor exclude reduced window coverage. No
    starvation or lost-window scenario was found in review.
  - The noise is the same size as the shift: pre's own two rotations differ by 10.4% (medians 53 and 48).
  - Pre's level moves with load. At R64's gate, at load 19–25, pre filled 41–44 windows a run, and that gate was
    also outside the band on this counter (1.060).
- **The work is unchanged within the band.** Decodes, CPU per refill and hits pre-converted are within 5% on both
  workloads, and admission waits pass on both. Timeouts and idle with discards are 0. `kept_handoff_redecode` is 0
  in all 12 candidate runs.
- **Rechecked at S4.** S4's pinned run re-checks windows filled, along with R65's admission waits (E13.6.12), with
  more runs.

**Evidence corrections (R66 item 5).** These are made in place in E13.6.12:
- "decodes identical run for run" now holds for the gate's two rotations only. In the diagnostic rotations pre3's
  second run decoded 12,711 and pre4's 12,916, against cand's 12,712.
- E13.6.12 now says plainly that `seek_gop60` admission waits rose from 45 to 49 over 12 runs and that the rise is
  not attributed.

**Rulings (R67).** The windows-filled rise is waived explicitly (not attributed to F13), and no extra rotations are run. `6336c4d`,
`34955fa` and `25cff0a` are accepted with the R66 commits. Nothing is pushed. Next come Astra's quick re-check, the
push gate on the laptop, and then the push.

## E14 PF1 S3a Phase 2d — GPU display, partial until the remaining gates

Phase 2d follows R68/R68a/R69/R70. The S3a source is split into the opt-in encoder, GPU timestamp instrumentation, native app activation, and this evidence. D15 remains `6edf77f`. Nothing was pushed. CI-W and the lead's review rounds remain pending; this section does not close S3a.

**Implementation.** The media engine's default transport stays CPU; the native app explicitly enables its display session on the shared egui-wgpu device. The three-slot display pool conserves ownership across free, pending, reserved, bound and retiring holders. A strictly later root epoch, one rebind per epoch, bounded acquisition and repaint wakeups govern reuse. Terminal disarms transfers, frees the app's native texture registration, then retires resources through observed GPU completion. Resizes retain old allocations through a post-release completion token. Device loss and receiver disconnection return charges; the latter preserves the CPU preview route.

The 192 KiB exact SDR/premultiply tables drive a storage-texture encode. Self-checks sweep all f16 input patterns with asymmetric opaque channels, independent alpha, and reachable premultiply pairs. A broken table falls back to CPU without a user event. GPU non-finite flags keep their issued stamp. Full-frame readback accounting excludes diagnostic self-check copies.

GPU timing measures the encode compute pass with a device-requested `TIMESTAMP_QUERY`, never CPU wall time. Query resolve/readback allocations are ledgered. Zero, reversed or unavailable timestamps are not accepted as GPU duration samples. Encoded samples are collected even when their frame is abandoned before publication; submission-to-flags-map latency is candidate-only information (R70). The on-demand release runner warms the actual paced P-play path and drains shutdown samples.

**Witnesses.** `display::tests` checks I5 on the all-pattern texture and ten exact frames from every production W builder, I6's sticky flags and stamped transport refusal, G-6 fallback, and I16 ownership/epoch/completion/resize/loss/disconnect/starvation rules. The real headless app fixture reuses the existing IN1 app harness and V5 paced callback: I1b checks all 65,536 actual Color32 premultiply entries; G7a binds eight new production playback frames with zero full-frame readbacks; app Drop frees its native texture id and returns ledger charges to zero. The forced CPU G7a control records one full-frame readback and fails the GPU-route assertion.

Raw runtime red/green summaries, device identities, gate logs, timing/load records, source counts and cleanup are in the main checkout's `target/review/pf/s3a-phase2-report.md` and `target/review/pf/s3a-logs/s3a-phase2d-*`. Compilation failures, fixture setup failures and a SIGKILL-interrupted earlier media run are excluded from pass evidence. The interruption's cause is unproven.

**Local commits.** `abd743e` opt-in display encoder/ownership/self-check; `fb7dfc7` encode-pass timestamp instrumentation; `019481d` native app activation. Each source commit has recorded laptop workspace build/clippy, touched-file formatting and affected-crate test logs. The `fb7dfc7` gate has no gate-bound source manifest; later synchronization is not proof of that earlier tested snapshot. The app commit additionally passed the full workspace stage gate and an explicit app build.

**Local fast-suite stage gate (CI-W and the slow tier are not established by this run):** 3,567 passed, 0 failed, 94 ignored across 36 test programs, one test thread. Media library: 1,079 passed, 59 ignored (1,422.37 s); app: 793 passed, 4 ignored (69.69 s). Slow-test lint: 49 markers and 45 on-demand allowlist entries agree. Final RTX4050 correctness: all 13 display tests passed (278.74 s), with every W builder's ten frames exact; all three app witnesses passed (0.83 s). The report records 22 mutation controls. R71 review found that I5 parity-route red failed at the constructor `unwrap`, before the route assertion; that red is excluded until the R71 rerun.

**LL, information only** (`llvmpipe`, Vulkan, software fallback, GPU claim false):

| Run | Encode samples | p50 ms | p95 ms | p99 ms | max ms |
|---|---:|---:|---:|---:|---:|
| 0 | 1,799 | 1.032243 | 1.393081 | 1.677657 | 2.250668 |
| 1 | 1,799 | 1.053924 | 1.478637 | 2.028550 | 4.860773 |
| 2 | 1,799 | 1.050442 | 1.417126 | 1.761418 | 2.214646 |

All three candidate runs and their three interleaved unchanged `cand-62a29b1` references exited 0 with Q-2 valid and zero missed callbacks. Begin/end one-minute load ranged 1.54–9.88; top-five CPU and GPU state samples were recorded. Candidate/reference present-p95 ratios were 1.053269, 1.056120 and 1.055239 (median 1.055239); the higher candidate p95 is reported without causal attribution or an improvement claim. Present intervals and candidate-only map-latency quartiles are in the report. The reference hash is unchanged (`d20efde4…`); candidate `019481d` is `c63a2ea4…`.

**LH lead run at `540818d` (2026-10-08 03:07–03:16 EDT).** R68a's three idle samples permitted this run after the worker's earlier busy check. Exact self-check, six-workload/ten-frame parity, and broken-table fallback all passed on the RTX 3090. G7b **passed** separately in all three warm runs: n = 1800/1799/1799; p50 = 0.130944/0.130272/0.130784 ms; p95 = 0.149536/0.147776/0.148256 ms; p99 = 0.153408/0.152512/0.153184 ms; max = 0.182240/0.177952/0.186816 ms. Q-2 was valid with zero missed callbacks in each run. Present-p95 candidate/reference ratios were 1.0638/1.0635/1.0632, median 1.0635 (information). Candidate submission-to-map p50 was about 4.03 ms, p95 4.17 ms, p99 4.28 ms; max 5.26–5.50 ms. CPU load was recorded (0.66–15.18); GPU P8 at the idle check. Raw evidence: `target/review/pf/s3a-lh-results.md` and `s3a-logs/s3a-phase2d-LH-*` in the main checkout. These pre-fix results must be superseded by post-R71-fix reruns; they do not establish the post-fix candidate.

**Timing log interpretation (R71 F11).** `ALL DONE` establishes runner completion only. Success additionally requires every lane exit to be zero and its validity/gate assertions to pass. The corrected analyser recognizes `PF1 play` after the test prefix on the same line and retains pair 2. Post-fix I4 on both LL/LH and G7b/LH correctness are reported below. R73 relocates cargo gates to this desktop; CI-W remains lead-owned.

**R70 source budget against `193c1b3`:** 1,127 added nonblank production lines, including shader, app integration and timing; 1,157 test/witness/runner lines. This is below the approximately 1,250 production allowance and 1,375 stop. Test utilities are conservatively counted as production where their cfg includes `test-util`. The runner and app fixture reuse production W builders, the existing IN1 app harness and the existing V5 paced callback. No additional design amendment is proposed.


**R71 fix round under R72/R73 (2026-10-09).** The laptop is unavailable;
PID 3004347's uncollected run is lost and the rendered F1 check was rerun
locally. Every cargo command used `CARGO_BUILD_JOBS=4 nice -n 10 cargo`,
with FFmpeg sourced, a separate worktree target, serialized heavy commands,
and one test thread for media (two or fewer elsewhere). After detecting FFmpeg's automatic thread count, cargo verification commands and
fixture children were additionally restricted to four CPU cores. Accepted native
timing lanes used the normal CPU allocation, nice 10, and one test thread.
No laptop SSH, push, Tensorfold action, or main-checkout build occurred.

R72 accepts retained registrations: at most one CPU and one native texture,
each with its own live cell. Same-route late binding remains; a handoff keeps
the inactive registration until its next route rebind, clear, or teardown.
Worst additional CPU retention is 8,294,400 bytes (about 8 MiB) at 1080p RGBA.
The actual-render GPU→CPU→GPU→CPU witness recorded
`route_counts=[1, 2, 2, 2, 2, 2, 2]`, with zero after clear and teardown.
Rebinding reuses the native registration without an extra temporary id.

The initial F1–F9 controls and F9's forced fallback are archived with
before/after source manifests in `target/review/pf/s3a-fix-r71/r73/`.
Re-review excluded F4's initial early-slot-uncharge control: it cleared slots
before the completion wait and did not distinguish encode from consumer
completion. The revised F4 control and witness are recorded below. F9 fails at the GPU route
assertion, not the constructor. The repeated F6 destruction probe exposed
delayed loss-callback delivery; the panic path now drains queued work before
confirming loss and otherwise propagates ordinary panics. The final 12-run
probe recorded zero hangs, zero propagated panics and exact zero charges.
F12 uses test-only publication/receipt traces and the unchanged default 5 ms
consumer poll, with a 500 µs diagnostic knob; no F12 production fix is made.

The RTX4050 correctness rerun, laptop log retrieval and laptop build/owned
fixture cleanup remain **owed when the laptop returns**, under R73. The
RTX4050 rerun is not an S3a close blocker. CI-W remains owed to the lead.
Post-fix gates, committed source identities, lane results and the F12 verdict
are recorded in the updated R71 fix report; historical timing above never
establishes the post-fix binary.


**Local fix gates and witnesses.** Media commit `307b465` passed workspace
build, all-target clippy with `-D warnings`, touched-file formatting, slow-test
lint and the full affected media suite: 1,152 passed, zero failed, 27 ignored.
App commit `2726b14` passed the same gates, the explicit app build and the full
app suite: 797 passed, zero failed, two ignored. Committed bytes were checked
against each gate's source manifest. An earlier app gate failed its structural
source scanner because it counted a test-only receiver wait; the shared witness
helper now uses the existing bounded `try_recv` pattern. That failed gate and the
interrupted earlier media gate are retained and excluded from green evidence.

Final isolated F1–F9 greens and the R72 bound ran on the committed Rust source.
The F6 construction witness additionally proves the Writing guard covers both
allocation and candidate-view construction: each was runtime-red on exact
540818d production with only cfg(test) fault hooks, then finished with zero
charges after the fix. The final destruction probe recorded
`iterations=12 hangs=0 panics=0`; each iteration asserts exact zero charges.
F9 reported ten exact GPU-route frames on all six workloads. Total added
nonblank Rust/shader lines versus `193c1b3` after the F4 follow-up:
1,315 production, 2,367 tests. F4 adds zero production lines; production
behavior is unchanged. Production remains below 1,375; this fix round's production changes address
F1–F8. Source manifests and raw per-finding red/green summaries accompany the
R71 report. The final workspace fast tier and release/lane results follow there.


**F4 completion-boundary follow-up (independent re-review r2).** The witness
records and completes the actual encode fence before submitting its texture
consumer. A cfg(test)-only pool probe runs the unchanged production completion
wait; a callback gate withholds its completion observation after a real
successful poll of the post-consumer release fence. The first successful poll
returns to production and the witness observes its wait loop re-entering before
permitting the callback. While completion observation is withheld,
the witness compares the pool's actual texture with the original wgpu texture
handle and checks all 197,392 charged bytes. It then permits the callback,
checks the retained identity again, samples the consumer output, and verifies
release back to the original zero-byte baseline.

The red control polls the captured encode fence and releases slots on that
successful encode completion, before waiting for the post-consumer fence.
It is not the prior unconditional clear. It fails at the retained-allocation
assertion while completion observation is withheld: `completion_wait_reentered=true identity_retained=false
charges=196880 expected=197392`. Restored green records
`identity_retained=true charges=197392 expected=197392`, followed by
`consumer_completed=true allocation_released=true charges=0 baseline=0`.
Raw red: `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured;
1147 filtered out; finished in 0.27s`. Raw green: `test result: ok. 1 passed;
0 failed; 0 ignored; 0 measured; 1147 filtered out; finished in 0.27s`.
Both are runtime llvmpipe/Vulkan witnesses, not hardware timing evidence.
Mutation patch, red/restored source snapshots, per-command source manifests,
gate logs and owned-output cleanup inventory are retained outside the main
build target at `/home/riels/kw-logs/pf1-f4-r2/`; the updated R71 report records
the gate exits. Two compile-only control setup failures are excluded from red
evidence. This scoped follow-up changes no production behavior and makes no
new LH, RTX4050, CI-W or timing claim.


**Final local fast tier and release (R73).** On `a4eb605`, the source-bound
workspace fast tier passed: 3,579 passed, zero failed, 94 ignored across
36 test programs. Workspace build, all-target clippy `-D warnings`, touched-file
formatting and slow-test lint passed; 49 markers and 45 allowlist entries agree.
This is local fast-tier evidence, not CI-W or an aggregate slow-tier result.
Each command's before/after source manifest is unchanged. The final Rust hashes
also match all 13 isolated witness logs and the release build. The later evidence
commit changes only this document; it does not change that tested Rust source.

The post-fix release harness is preserved at
`target/review/pf/s3a-fix-r71/bins/cand-r71-a4eb605-101c40f07bed` in the main
checkout: SHA256 `101c40f07bed3d508dd5a55af284724f66db15c460b6f17c3441afbf8f0ad03f`.
The reference remains `s2c-timing/bins/cand-62a29b1` (`d20efde4…`); complete
binary identities, manifests and commands are in `r73/release-identity.json`.

**Post-fix LH provenance and per-lane evidence (R74a).**

**Status: post-fix LH measurement complete (R74a, 2026-10-09).** LH correctness trio PASS; G7b PASS; paired I4 PASS. This supersedes bb592a3's literal-R74 preflight stop. CI-W and lead stage acceptance remain separate.

Measurement worktree: `pf1/impl` at `bb592a320bd9fffebfa24debd4b71710e7cbcc0c`. The preserved a4eb605 candidate SHA256 is `101c40f07bed3d508dd5a55af284724f66db15c460b6f17c3441afbf8f0ad03f`; the same LL reference `s2c-timing/bins/cand-62a29b1` has SHA256 `d20efde471b2c4d6f256011cc548b1087e4c5f81715f5ea13fd4589fc3f6fd70`. The previous source identity was rechecked: 217 manifest files, cfg(test)-only F4 changes and the equivalent callback binding; production and tracked WGSL match the preserved candidate. No cargo, source edits, rebuild, push or laptop SSH.

Artifacts: `/home/riels/kw-logs/pf1-lh-r74a/`. Plans, runner/analyser, `source-identity.json`, both timing and 5 s sampler logs, each lane's `*-preflight.log` / `.json` (full nvidia-smi tables with Types), actual runner PID records, lane timeout PIDs and descendant inventories are retained. Launchers are recorded separately. Runs were serialized at nice 10, one test thread, normal CPU allocation and a 600 s timeout.

**Correctness.** All three lanes identify NVIDIA GeForce RTX 3090 / Vulkan, `software_fallback=false`, `gpu_claim=true`. W prints `frames=10 route=GPU exact=true` for typical_1080p, blend_heavy_1080p, explainer_16x9, reel_9x16, feed_4x5 and talk_recut. G-6 is the intentional broken-table CPU fallback / scratch-release witness, not parity evidence.

**G7b: warm typical_1080p P-play, encode-pass GPU timestamps.** Canonical runs are LH-candidate-0/1/2; the extra candidate-1 rerun is present-pair evidence and is not substituted into this trio. All canonical runs have Q-2 valid, zero missed callbacks.

| Run | n | p50 ms | p95 ms | p99 ms | max ms |
|---|---:|---:|---:|---:|---:|
| 0 | 1764 | 0.129952 | 0.150752 | 0.156864 | 0.176416 |
| 1 | 1698 | 0.130496 | 0.151072 | 0.155552 | 0.175584 |
| 2 | 1799 | 0.129728 | 0.150752 | 0.156704 | 0.184224 |

Every p99 ≤2 ms: **PASS**. Maxima are recorded, not gated. Submission-to-map candidate-only p50/p95/p99/max ms: [4.086725, 4.293983000000001, 5.02086, 9.273396]; [4.070569, 4.284205999999999, 4.731781, 7.096726]; [4.04791, 4.231242, 4.405082, 6.909389].

**Present interval (information).** Candidate/reference alternation follows LL. Initial pair 1's reference failed Q-2 (`valid=false`, one missed callback, exit 101). The full candidate/reference pair was rerun once; both reruns passed with zero missed callbacks. The failed attempt remains excluded, with its raw failure preserved. All three accepted reference rows have `passes=false` for broader P-play counters despite valid Q-2 and exit 0: this is present information, not a P-play stage gate or causal improvement claim.

| Pair | Candidate p50 / p95 / max ms | Reference p50 / p95 / max ms | p95 ratio |
|---|---|---|---:|
| 0 | 33.728911 / 45.599575 / 110.321779 | 64.000000 / 85.500000 / 170.000000 | 0.533328 |
| 1 (rerun) | 35.481969 / 45.569101 / 86.664628 | 63.600000 / 85.100000 / 86.300000 | 0.535477 |
| 2 | 35.480487 / 45.575122 / 50.779156 | 33.200000 / 43.000000 / 84.900000 | 1.059887 |

Median present p95 ratio: **0.535477**, information only. Reference spread and recorded load are retained.

**Paired I4: r28_end_to_end_tracked, typical_1080p, E13 protocol.** Each external test contributes the mean of its three internal R28 run means, matching LL. All six tests pass on RTX 3090.

| Pair | Reference mean ms | Candidate mean ms | Candidate/reference |
|---|---:|---:|---:|
| 0 | 73.803333 | 69.663333 | 0.943905 |
| 1 | 67.496667 | 67.746667 | 1.003704 |
| 2 | 66.720000 | 67.490000 | 1.011541 |

Median ratio **1.003704 ≤1.05: PASS**. This is the prescribed paired gate under recorded load.

**F12 hardware confirmation: 5 ms versus 1 ms measurement-consumer poll.** Both controls pass Q-2 with zero missed callbacks and zero slot starvation.

| Poll µs | Publication p95 ms | Receipt-lag p95 ms | Present p95 ms | Trace frames |
|---:|---:|---:|---:|---:|
| 5000 | 42.731215 | 4.827753 | 45.516414 | 1800 |
| 1000 | 42.943288 | 1.708613 | 43.099395 | 1798 |

Present p95 fell 2.417019 ms (5.31%); publication p95 changed +0.212073 ms (+0.50%). Receipt-lag p95 changed from 4.827753 to 1.708613 ms. Per-frame tail median receipt-lag delta: 2.831404 versus 0.511933 ms. **LH verdict: consistent with LL's measurement-consumer receipt quantization.** Producer cadence, receipt lag and traces are reported separately; no application-speed improvement or additional production defect is established. The canonical 5 ms protocol and production source are unchanged.

**Per-lane exits and raw test result lines (including excluded attempt).**

- `LH-correctness-bits`: exit=0; `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1146 filtered out; finished in 0.46s`.
- `LH-correctness-W`: exit=0; `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1146 filtered out; finished in 161.92s`.
- `LH-correctness-G6`: exit=0; `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1146 filtered out; finished in 0.32s`.
- `LH-candidate-0`: exit=0; `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1146 filtered out; finished in 66.12s`.
- `LH-reference-0`: exit=0; `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1122 filtered out; finished in 66.63s`.
- `LH-candidate-1`: exit=0; `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1146 filtered out; finished in 66.92s`.
- `LH-reference-1`: exit=101; `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1122 filtered out; finished in 66.96s`.
- `LH-candidate-1-rerun`: exit=0; `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1146 filtered out; finished in 65.83s`.
- `LH-reference-1-rerun`: exit=0; `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1122 filtered out; finished in 66.29s`.
- `LH-candidate-2`: exit=0; `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1146 filtered out; finished in 65.60s`.
- `LH-reference-2`: exit=0; `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1122 filtered out; finished in 65.53s`.
- `LH-I4-reference-0`: exit=0; `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1122 filtered out; finished in 75.23s`.
- `LH-I4-candidate-0`: exit=0; `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1146 filtered out; finished in 70.49s`.
- `LH-I4-reference-1`: exit=0; `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1122 filtered out; finished in 68.52s`.
- `LH-I4-candidate-1`: exit=0; `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1146 filtered out; finished in 68.65s`.
- `LH-I4-reference-2`: exit=0; `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1122 filtered out; finished in 67.99s`.
- `LH-I4-candidate-2`: exit=0; `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1146 filtered out; finished in 68.45s`.
- `LH-F12-poll-5000`: exit=0; `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1146 filtered out; finished in 64.56s`.
- `LH-F12-poll-1000`: exit=0; `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1146 filtered out; finished in 64.51s`.

The initial aggregate ended `success=False` because of the retained reference failure. It is not called green. Accepted results exclude only that invalid reference and use the complete replacement pair.

**GPU preflights (three samples, 5 s apart before every lane).** Triples below are chronological utilisation % / memory MiB / pstate. Every full table shows only G/C+G desktop contexts; maximum per-process allocation across all 57 samples is 234 MiB, below 2 GiB. No pure-C co-tenant or all-three ≥80% group occurred. R74a exempts the desktop C+G contexts; utilisation/load are recorded.

| Lane | Sample times EDT | Util % ×3 | Memory MiB ×3 | Pstate ×3 |
|---|---|---|---|---|
| LH-correctness-bits | 17:39:21.482 / 17:39:26.482 / 17:39:31.479 | 31 / 36 / 31 | 1223 / 1256 / 1223 | P8 / P8 / P8 |
| LH-correctness-W | 17:39:35.377 / 17:39:40.386 / 17:39:45.384 | 12 / 32 / 33 | 1223 / 1223 / 1223 | P5 / P8 / P8 |
| LH-correctness-G6 | 17:42:30.032 / 17:42:35.037 / 17:42:40.034 | 0 / 0 / 0 | 1223 / 1223 / 1223 | P8 / P8 / P8 |
| LH-candidate-0 | 17:42:43.882 / 17:42:48.883 / 17:42:53.883 | 1 / 0 / 0 | 1223 / 1223 / 1223 | P8 / P8 / P8 |
| LH-reference-0 | 17:44:02.751 / 17:44:07.754 / 17:44:12.754 | 0 / 0 / 0 | 1223 / 1223 / 1223 | P8 / P8 / P8 |
| LH-candidate-1 | 17:45:22.068 / 17:45:27.069 / 17:45:32.069 | 0 / 0 / 0 | 1223 / 1223 / 1223 | P8 / P8 / P8 |
| LH-reference-1 | 17:46:41.727 / 17:46:46.727 / 17:46:51.730 | 0 / 0 / 0 | 1223 / 1223 / 1223 | P8 / P8 / P8 |
| LH-candidate-1-rerun | 17:49:04.484 / 17:49:09.482 / 17:49:14.484 | 0 / 0 / 0 | 1223 / 1223 / 1223 | P8 / P8 / P8 |
| LH-reference-1-rerun | 17:50:23.376 / 17:50:28.376 / 17:50:33.380 | 0 / 0 / 0 | 1223 / 1223 / 1223 | P8 / P8 / P8 |
| LH-candidate-2 | 17:51:42.322 / 17:51:47.320 / 17:51:52.320 | 0 / 0 / 0 | 1223 / 1223 / 1223 | P5 / P8 / P8 |
| LH-reference-2 | 17:53:01.170 / 17:53:06.171 / 17:53:11.168 | 0 / 0 / 0 | 1223 / 1223 / 1223 | P8 / P8 / P8 |
| LH-I4-reference-0 | 17:54:20.010 / 17:54:25.010 / 17:54:30.009 | 0 / 0 / 0 | 1223 / 1223 / 1223 | P8 / P8 / P8 |
| LH-I4-candidate-0 | 17:55:48.855 / 17:55:53.852 / 17:55:58.852 | 0 / 0 / 0 | 1223 / 1223 / 1223 | P8 / P8 / P8 |
| LH-I4-reference-1 | 17:57:12.677 / 17:57:17.680 / 17:57:22.680 | 0 / 0 / 0 | 1223 / 1223 / 1223 | P8 / P8 / P8 |
| LH-I4-candidate-1 | 17:58:33.813 / 17:58:38.814 / 17:58:43.812 | 0 / 0 / 0 | 1223 / 1223 / 1223 | P8 / P8 / P8 |
| LH-I4-reference-2 | 17:59:55.063 / 18:00:00.065 / 18:00:05.065 | 0 / 0 / 0 | 1223 / 1223 / 1223 | P8 / P8 / P8 |
| LH-I4-candidate-2 | 18:01:15.687 / 18:01:20.689 / 18:01:25.690 | 0 / 0 / 0 | 1223 / 1223 / 1223 | P8 / P8 / P8 |
| LH-F12-poll-5000 | 18:02:36.736 / 18:02:41.737 / 18:02:46.734 | 0 / 0 / 0 | 1223 / 1223 / 1223 | P8 / P8 / P8 |
| LH-F12-poll-1000 | 18:03:53.909 / 18:03:58.910 / 18:04:03.909 | 0 / 0 / 0 | 1223 / 1222 / 1222 | P8 / P8 / P8 |

Begin/end one-minute CPU load: 1.74–22.77; 1107 >20% CPU contamination annotations (repeated samples included). Full top-five, pstate/clocks/power and 5 s sampler records are alongside the logs. No external process was changed or signalled.

Cleanup: removed 0 recorded-PID-owned `/tmp/kinewright-*` fixtures (0 apparent bytes); verified absent. Other fixtures are preserved. The worktree target remains absent. Inventory: `cleanup-inventory.json`. Candidate/reference hashes and production identity were rechecked after measurement. Only E14 is committed; the main-checkout report is review evidence. No push.

**Post-fix LL I4 (F10, E13 paired protocol).** Each external R28 test contributes
the mean of its three internal run means. All six tests passed.

| Pair | Reference mean ms | Candidate mean ms | Candidate/reference |
|---|---:|---:|---:|
| 0 | 77.916667 | 68.373333 | 0.877519 |
| 1 | 75.293333 | 71.833333 | 0.954046 |
| 2 | 71.586667 | 66.993333 | 0.935835 |

Median 0.935835 ≤ 1.05: **LL I4 PASS**. This establishes the prescribed
no-regression gate under recorded load; it is not a causal improvement claim.
LH I4 is reported above. LL raw means and lane exits are in `r73/LL-I4-plan-timing.log`.

**Post-fix LL information.** The three canonical candidate/reference pairs
passed their test and Q-2 checks, with zero missed callbacks. Encode sample
counts: 1704/1779/1781; encode p99: 4.344111/2.729578/3.297489 ms. These are
software-lane information, never G7b gate proof. Present p95 was
51.018454/45.751131/45.720494 ms versus 43.1/43.2/43.1 ms; median ratio
1.060800. Two reference P-play rows reported `passes=false`; their test/Q-2
checks passed, and this comparison is present information rather than a
P-play stage gate. Full quartiles, maxima, counters and flags are retained.
R50 load was recorded: begin/end one-minute load across canonical/diagnostic
attempts ranged 8.23–31.69, with 5 s contamination sampling and top-five/GPU
state records. No quiet-window or load-attributed performance claim is made.

**F12 verdict on LL: measurement-consumer polling quantization.** A valid 1 ms
control reduced present p95 from 45.813703 to 43.798822 ms, while publication
p95 stayed 43.281396 versus 43.324317 ms (0.10% difference). Receipt-lag p95
fell from 4.805471 to 1.770448 ms. Both runs had Q-2 true, zero missed callbacks
and zero slot starvation. The per-frame tail trace's median receipt-lag change
was 3.112296 ms at 5 ms polling versus 0.629647 ms at 1 ms polling. The extra
steady p95 appears after publication, in the timing harness's consumer poll;
the producer cadence is essentially unchanged. No native app defect is
established and no F12 production fix is made. The canonical 5 ms protocol stays
unchanged; any measurement-consumer update belongs to a separately agreed
measurement/S3b change. LH hardware confirmation is recorded above.

The first 500 µs control and one rerun failed Q-2 (2 and 1 missed callbacks,
exits 101); they are excluded from paced evidence. The valid comparison uses
the later 1 ms control on the same binary. The initial four-core timing trials
are also excluded: one Q-2 failure and one completed capacity-limited trial.
Failed/interrupted logs remain archived, with owned PIDs recorded. No failed
aggregate is represented as green, and `ALL DONE` is completion only. The cause
of those missed callbacks is not proven. The report retains the exact accepted
and excluded runs, trace facts, cleanup inventory and any owed work.

Post-fix LH is complete; CI-W and lead acceptance follow below. The RTX4050 rerun and the laptop's
log, build and owned-fixture cleanup remain owed under R73 for when the laptop
returns.

**Independent re-review (2026-10-09).** A fresh Astra xhigh reviewed
`540818d..2d6faa4` (`target/review/pf/s3a-review/r2/`). F1–F3, F5–F9, R72,
F11, F12 and the 1,315-line budget passed. Its one blocker was F4: the witness
did not separate encode completion from completion of the later consumer.
`b10aaec` reworked the witness, with no production-behaviour change, and a
second fresh Astra check (`s3a-review/r3/`) closed F4. No review finding
remains open.

**S3a closed (2026-10-09).** CI run 37992860857 on `9787be9` (`[slow-tier]`) is
green on Linux and Windows, fast and slow tiers. Its first attempt failed one
Windows fast-tier test, `preview::tests::a_newer_job_cancels_the_rest_of_the_window`
(`preview.rs:3344`: frame 16 of the old window was converted after the newer
post). That run used the WARP software adapter. Under R69's flaky-test rule the
job got one rerun, and it passed. The test predates S3a (R53, S2c) and
passed on Linux in both tiers. It is recorded as a Windows timing flake; a
second failure would be a finding. The lead accepts S3a. The RTX 4050 rerun
and the laptop cleanup remain owed under R73.
