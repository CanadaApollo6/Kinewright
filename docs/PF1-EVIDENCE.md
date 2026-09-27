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
  (LL, `--nocapture`), and in each commit's `cargo test -p kinewright-media`.

### E10.2 I4 (S0's 5% rule, `BASELINES` untouched)

`mo2_perf_fixtures.rs` and `perf_fixtures.rs` are unchanged since
`4082d5d`.

| Build | LL mean ms (3 runs) | LL delta | LH mean ms (3 runs) | LH delta | Result |
|---|---|---|---|---|---|
| S0 `4082d5d`, same session | 529.83 / 528.64 / 531.24 | +6.4% | 526.96 / 526.99 / 528.01 | +6.7% | FAILED on both lanes |
| S1a `47b7d68` (control alone) | 232.95 / 232.66 / 233.48 | −53.2% | 231.93 / 229.98 / 230.32 | −53.3% | ok |
| S1 end `217986a` | 77.03 / 76.23 / 75.95 | −84.7% | 72.87 / 73.00 / 73.69 | −85.2% | ok |

- **The unmodified S0 binary fails I4 in this session** (+6.4% / +6.7%,
  against +2.4% / +2.6% in E9.2). Nothing in the code changed, so this is
  the ambient load above. It shows the 5% rule's margin is within this
  desktop's noise; the S1 figures clear it by 80 points.
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

All pass in every commit's `cargo test -p kinewright-media` and on the
stage-end release binary (LL):
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
  `select_conversion`, 2,567,942 rejected, 90 keys, every code),
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

### E10.7 Deviations and notes

- **S1e (U-1) is not implemented.** Its condition is "S0 finds
  `upload_bytes`'s extra full copy material"; S0 never measured it (E3 says
  S0 would; E9 has no figure). A scratch release measurement (not
  committed) gives `upload_bytes` 1.829 ms against a plain copy's 0.250 ms
  for one 1280×720 layer (7,372,800 bytes), about 1.6 ms of extra copy per
  layer per frame, and the phases `upload_ms` above is 10–13 ms of a 14 ms
  LH frame. G3 passes without it. An exact byte cast needs
  `zerocopy::IntoBytes` for `f16` (`half` 2.7.1 already implements it and
  zerocopy is already in the lock file) as a direct dependency of
  `kinewright-media`, because the workspace forbids `unsafe`. Proposed: the
  lead decides whether that counts as material and whether to add the
  direct dependency (S1e follow-up) or leave it to S3b's staging ring.
- **Two S0 harness tests are adjusted for V-1** besides the named test
  (C-5 says only the V-1 test is edited):
  `stepped_callbacks_pop_through_render_output_and_count_underruns` (its
  comment already said "V-1 fixes that in S1") and
  `an_observed_clock_never_runs_ahead_of_its_underruns`, which relied on the
  clock advancing over an empty ring (it would never finish under V-1) and
  now half-fills the ring each callback. Proposed: C-5 reads "only the V-1
  tests (including S0's T7/R25 harness tests) are edited".
- **The live table counter is test-only** (`table_live_kib=` on the
  harness line). `CacheStats` is public wire data, so the production
  exposure is left to S2a's preview `stats`.
- **K-6:** titles are still evicted after every video frame, and a pinned
  frame per demand point can overshoot the window cap by at most one frame
  per active source until the next reservation (S1 claims no I12).
- **Budget:** 1,029 non-blank, non-comment `.rs` lines added and 93
  removed across S1a–S1g, against ~650; the exhaustive and worker tests
  are most of the excess (per-commit counts are in the S1 report).
