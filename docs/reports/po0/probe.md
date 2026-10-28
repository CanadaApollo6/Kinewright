# PO0 motion probe

**NVIDIA 60 Hz: FAIL. Lavapipe 60 Hz: FAIL.** These are measurements of the opt-in probe on Riel's shared workstation, with incomplete runtime coverage identified below. They do not establish a reason to replace egui. No rollout, push, display-mode change or GPU tuning was performed.

The source is **1920×1080 at 30 fps; the actual native presenter texture is Proxy 1280×720**. The UI viewport is 2560×1440. Per Riel, there is no full-1080p native-output variant. The 1080p memory figures below are arithmetic projections.

## Provenance and protocol

Measured native sources are frozen separately. The final companion commit adds documentation and a runner-only classification correction; it does not change the native binary:

| Segment | Source commit | Accepted moments |
| --- | --- | --- |
| C2, original NVIDIA matrix | `c2cf9ab546b0ad42b05a0d2cfba08fe78b72fff4` | 1, 2, 4–10 |
| C3, corrected NVIDIA M3 | `60e8befc835fd42aa7599994be36a418303643f8` | 3 |
| C2, completed software cases | `c2cf9ab546b0ad42b05a0d2cfba08fe78b72fff4` | 1, 2, 4 |
| C3, remaining software cases | `60e8befc835fd42aa7599994be36a418303643f8` | 3, 5–10 |
| C3, NVIDIA deadline variant and final captures | `60e8befc835fd42aa7599994be36a418303643f8` | M1 snap; captures separate |

The original C2 M3 incorrectly advanced `FrameStamp.epoch` without ending/beginning PF1 due-frame accounting. The native presenter sent first-paint ACKs, but the new epoch's ACKs were unmatched by accounting. That original M3 G1 result is **INVALID PROBE DATA**, not a toolkit failure. C3 aligns accounting to the applied runtime position before any new render, preserves outstanding old-epoch ACK records and leaves the live audio clock running. A regression produced a literal red result before the fix and green afterwards. C3 changes only this guarded M3 handler and report-only ACK fields; the other C2 rendering/physics cases remain applicable.

The original software sequence was deliberately stopped after M5 paused p0 to fix M3; M5's partial 40-trial case is retained as an archive and rerun in full on C3. Every accepted case uses an explicit source segment, without filtering by load, errors or success. The archived C2 M3 and partial M5 retain their logs, metrics and load but receive no accepted repetition credit.

Original frozen release binary:

```text
target/po0-probe/bin/kinewright-app-c2cf9ab5-11f61a430274d381
SHA-256 11f61a430274d3817895e99004dc54a07bdb14b0f654dcc21ae5ee8eefa081d5
```

Corrected frozen release binary:

```text
target/po0-probe/bin/kinewright-app-60e8befc-23f796a7970c66cc
SHA-256 23f796a7970c66ccdf44d67f73082a90bd32c56eae442abb56ac92b9e769945b
```

`target/po0-probe/accepted/{nvidia-60,lavapipe-60,nvidia-60-snap520}/results.json` identifies each chunk's commit, binary hash and raw source file. The original directories are retained, including superseded and deliberately interrupted runs. The binaries were built from clean source trees and copied before measuring. After collection, the runner was corrected to score the deadline-snap variant against the 520 ms deadline instead of reusing the primary physics conflict label. Accepted snap manifests record this interpretation and the runner hash; original raw results and all measured values are unchanged. Primary physics keeps the DESIGN-DOC CONFLICT classification. The actual variant repaint windows exceed 520 ms because of rendered-frame/scheduling delay; they are deadline misses, not an equation/window conflict.

No readable talking-head clip was found in the checkout or `~/Videos`. The input is a generated 660-second `testsrc2` H.264 source, libx264 ultrafast/CRF 30, `yuv420p`, limited-range BT.709, with silent 48 kHz AAC. The SPS colour tags were set with `h264_metadata` and verified with FFprobe. Managed source admission remains enabled. The source is not committed:

```text
target/po0-probe/source-vui-1080p30-11min.mp4
SHA-256 88e6974efff8b2de6d53fb38d35df8801a420af80286b465873d7681741af7e7
```

Each scene/case requests five fresh processes × 40 repetitions: 200 paused and 200 playing. M6 alternates all-effects OFF then ON for each pair, adding 400 control repetitions per lane. Thus each primary lane requests 4,400 repetitions. M2 has 12 buttons per sweep. M9 alternates bottom-follow and scrolled-up reading, 100 trials of each per condition. M10 calls the existing `track_reframe_subject` once per chunk, seeds a fixed 12%×20% box at (50%,45%), and replays/interpolates its 25-point path (frames 0–120, step 5). The prepared edit plan is discarded. There is no person detector.

Each timed GUI process has a 580-second report deadline and a 590-second parent timeout. Smoke, setup and separate capture runs receive no acceptance repetition credit. Audio-error runs are retained and do not establish uninterrupted playing coverage. An advancing clock alone does not establish visible playback.

Frame quantiles are not pooled: the tables show the worst per-run statistic. `frame.info().cpu_usage` is eframe 0.35's previous-frame UI plus CPU rendering time, excluding vsync waiting. Process CPU includes all process threads; added CPU subtracts the same run's three-second static baseline from the whole scripted period, including gaps. The playing baseline keeps playback running. Negative deltas are retained. Neither quantity isolates the animator's computation cost. Update intervals are not physical scanout measurements.

GPU timestamp distributions include setup/warmup and active frames. Query preparation allocates resolve/readback buffers per frame. OFF measurements are reported beside ON; software bucket attribution requires the qualification below. Worker times are elapsed times for **applied** light samples, not all produced/discarded samples or isolated CPU time.

The Python runner and GUI children inherited niceness 19. The app ran on the existing DP-2 monitor at 60.001 Hz. The other monitor was at 144 Hz; 120 Hz was offered but not active. No modes were changed. The runner selected and checked the actual adapter, moved only its own process's window, isolated XDG/config/cache/temp state under `target/po0-probe/`, and sampled load every five seconds. No own builds, tests or captures ran beside the timed matrix. [The load appendix](load.md) records every chunk; raw start/end snapshots, instantaneous top five and five-second flags remain in each lane's `results.json`. Physical RTX 3090 load is recorded even for the CPU-rendered lane. Load did not exclude a run.

Final cleanup found **51 leftover normal-startup CLI version/auth-status probes** with this worktree's cwd and isolated XDG paths, including mise/Omarchy wrapper loops. They had outlived their GUI processes. These earlier own probes form part of the recorded ambient load; the “other CPU” samples must not all be attributed to other projects. The exact contribution to each timing window was not isolated. After all timing/captures, commands, identity/start ticks, cwd and environment were checked and only these owned checks were terminated using pidfds. `target/po0-probe/owned-cli-cleanup.json` records the action. A clean rerun after that cleanup is **NOT RUN**. The measured FAILs remain fresh-process/instrumented/shared-load observations, not an intrinsic egui limit.

Owned debug/release/MSVC-check artifacts and transient per-run state were removed after measurement. Frozen binaries, the source clip, JSON/Markdown results and images remain. Large `windows-check.log` and `locked-media-tests.log` are retained as `.log.gz`; `target/po0-probe/cleanup.json` records cleanup. No shared `/tmp` files or other projects' processes were touched.


## NVIDIA RTX 3090, Vulkan, 60 Hz

The accepted primary matrix contains **4,360/4,400** recorded repetitions in 110 chunks. All 110 sampled monitor checks matched 60.001 Hz. The adapter was the RTX 3090, NVIDIA driver 615.78.08; the host reported 20 logical CPUs. These measurements fail the 60 Hz budgets; incomplete and error-bearing chunks remain visible below.

| §6 bar | Measurement | Verdict |
| --- | --- | --- |
| CPU frame p99 ≤10 ms | Worst reported chunk p99 **14.428 ms**, in corrected M3; one chunk wrote no report | FAIL |
| Dropped animated updates ≤0.5% | Worst **7.940%**; 93/109 reported chunks exceed the bar | FAIL |
| Added process CPU ≤10% of one core | Worst **175.987%**; includes filmstrip/decode work and instrumentation | FAIL |
| Zero motion-layer repaints after settle | **0** in M1; other scenes do not have this counter's coverage | PASS M1; NOT RUN across all scenes |
| Transition ends within ±1 frame | Recorded non-spring curve error up to **76.097 ms**, against 16.667 ms; manual transition timings are incomplete | FAIL |
| Spring token windows / ≤520 ms repaint window | Panel median completion ≈**633 ms**, repaint-window maximum **654.882 ms** | DESIGN-DOC CONFLICT; no drop |
| Dissolve begins ≤1 frame after incoming presents | **0 UI frames** in all 10 corrected M3 chunks | PASS |
| PF1 G1 during playing dissolve ≤1% bad | Corrected windows **48.750–52.454%** bad; worst per-chunk presented-interval p95 **83.963 ms** | FAIL |
| Light + glass + grain GPU ≤0.3 ms at 1440p | M6 paired ON maximum **0.558 ms**; all-scene maximum 0.706 ms | FAIL |
| Downsample worker ≤0.5 ms/update | M6 applied-update maximum **1.180 ms**; all-scene maximum 17.729 ms | FAIL |
| Cold strip ≤1 s paused / ≤3 s playing | Maxima **14,316.344 / 7,982.817 ms** | FAIL |
| PF1 G1 during strip fill | Playing windows **92.049–94.802%** bad; worst per-chunk presented-interval p95 **2,101.786 ms** | FAIL |
| Atlas ≤16 MB | One colour+neutral atlas pair **1,474,560 B** | PASS, texel payload |
| No glow banding on near-black, to Riel's eye | Human check pending | NOT RUN |
| Wayland offers transparent alpha | Actual surface offers **Opaque, PreMultiplied** | PASS capability; compositing NOT RUN |

| Case | Completed / requested | CPU p50 / p99 ms | Update p50 / p99 ms | Dropped % | Added CPU, one-core % | Scene-error / bad-exit runs |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| m01-paused-on | 200 / 200 | 0.622 / 4.810 | 16.701 / 31.176 | 1.791 | 66.302 | 0 / 0 |
| m01-play-on | 200 / 200 | 0.923 / 7.277 | 16.741 / 33.227 | 2.751 | 7.016 | 0 / 0 |
| m02-paused-on | 200 / 200 | 0.670 / 4.209 | 16.676 / 23.092 | 0.754 | 56.588 | 0 / 0 |
| m02-play-on | 200 / 200 | 0.986 / 6.735 | 16.720 / 32.456 | 2.059 | 6.163 | 1 / 0 |
| m03-paused-on | 200 / 200 | 0.923 / 4.721 | 16.714 / 23.435 | 0.818 | 99.101 | 0 / 0 |
| m03-play-on | 200 / 200 | 0.956 / 14.428 | 16.824 / 53.121 | 7.940 | 54.475 | 0 / 0 |
| m04-paused-on | 200 / 200 | 0.758 / 5.136 | 16.712 / 30.400 | 1.184 | 40.321 | 0 / 0 |
| m04-play-on | 200 / 200 | 1.009 / 6.203 | 16.729 / 33.127 | 2.208 | 2.246 | 0 / 0 |
| m05-paused-on | 200 / 200 | 0.668 / 4.092 | 16.698 / 23.003 | 0.628 | 72.208 | 0 / 0 |
| m05-play-on | 200 / 200 | 0.987 / 6.678 | 16.716 / 31.086 | 1.742 | 8.402 | 1 / 0 |
| m06-paused-off | 200 / 200 | 0.563 / 5.053 | 16.707 / 22.854 | 0.402 | 75.292 | 0 / 0 |
| m06-paused-on | 200 / 200 | 0.696 / 5.631 | 16.726 / 28.041 | 2.008 | 73.498 | 0 / 0 |
| m06-play-off | 200 / 200 | 0.668 / 8.035 | 16.790 / 34.148 | 3.520 | 1.810 | 0 / 0 |
| m06-play-on | 200 / 200 | 0.987 / 7.638 | 16.749 / 33.358 | 2.869 | 5.444 | 0 / 0 |
| m07-paused-on | 200 / 200 | 0.691 / 8.768 | 16.731 / 36.492 | 3.446 | 175.987 | 0 / 1 |
| m07-play-on | 160 / 200 | 0.775 / 4.917 | 16.699 / 27.041 | 1.251 | 94.565 | 2 / 1 |
| m08-paused-on | 200 / 200 | 0.644 / 3.996 | 16.692 / 25.709 | 1.093 | 77.238 | 0 / 0 |
| m08-play-on | 200 / 200 | 0.945 / 8.791 | 16.708 / 33.852 | 2.724 | 8.221 | 0 / 1 |
| m09-paused-on | 200 / 200 | 0.740 / 5.036 | 16.702 / 25.182 | 1.142 | 21.919 | 0 / 0 |
| m09-play-on | 200 / 200 | 0.978 / 8.056 | 16.723 / 33.398 | 2.969 | 5.042 | 0 / 0 |
| m10-paused-on | 200 / 200 | 0.765 / 4.935 | 16.707 / 25.843 | 1.123 | 81.920 | 0 / 2 |
| m10-play-on | 200 / 200 | 0.910 / 6.463 | 16.713 / 32.565 | 2.278 | 49.787 | 1 / 0 |

Recorded repetitions are not a clean acceptance count. Five accepted chunks reported `audio output stream failed`: M2 play p3; M5 play p2; M7 play p0/p2; M10 play p4. M7 play p4 exited on SIGKILL after 132 seconds, below the runner timeout, without a report: **40 repetitions are unmeasured**. No matching kernel/systemd-oomd kill record was found; its cause is unclassified.

Three accepted chunks aborted after writing 40 repetitions (M7 paused p2, M8 play p2, M10 paused p2); M10 paused p1 similarly wrote its report before SIGSEGV. Thus 105 processes exited 0, three aborted, one segfaulted and one was killed. These are ten failure-bearing rows, including the five audio-error chunks. No failed row was filtered out.

Corrected M3 completed all 200 paused and 200 playing repetitions, with clean exits and no scene errors. Every playing chunk reported **zero unmatched paint ACKs**; incoming/outgoing paint ACKs were attributed to their epochs, with the late counts retained. Paused unmatched frames are not due-frame failures. The corrected PF1 G1 values above supersede the original invalid accounting result; they still fail the bar. Ready-to-dissolve delay remains zero.

M6 play ON p0 had **no first preview image**, zero applied light samples and 100% whole-run PF1 bad frames despite an advancing clock. It allocated grain only (65,536 B), so it does not certify live-light visuals. This does not establish a light identity-matching bug. The first archived abort's core shows the export-queue worker destroying the NVIDIA Vulkan device while the main thread is in NVIDIA GLX process cleanup. That is consistent with concurrent shutdown cleanup; it is not a proven explanation of every failed exit. The opt-in path drains the preview worker, but a joined export-worker/device shutdown contract remains a follow-up.

## Linux lavapipe, Vulkan, 60 Hz

The accepted matrix recorded **4,280/4,400** repetitions in 110 chunks. The adapter was **llvmpipe (LLVM 23.1.3, 256 bits), CPU**, Mesa 26.2.5-arch1.1 Vulkan, with no hardware substitution. The viewport was 2560×1440 on the same 60.001 Hz monitor. This correctness lane **FAILS** its interval/end-time bars and has incomplete playing M3 coverage. Process exit counts: {'0': 108, 'TIMEOUT': 2}.

| §6 bar | Measurement | Verdict |
| --- | --- | --- |
| Update interval p99 ≤33 ms | Worst per-chunk p99 **408.152 ms** | FAIL |
| Non-spring transitions end within ±1 frame | Recorded curve error up to **769.267 ms**; manual transition coverage incomplete | FAIL |
| Press visible on next frame | No press-event-to-visible-paint instrumentation | NOT RUN |
| Spring repaint window ≤520 ms | Recorded M1 window maximum **770.863 ms**; equations/thresholds preserved | DESIGN-DOC CONFLICT; no drop |
| Zero motion-layer repaints after settle | M1 measured **0**; other scenes not covered | PASS M1; all scenes NOT RUN |
| Light + glass + grain ≤2 ms/frame | Raw M6 ON combined maximum **101.547 ms**, all-scene raw maximum **495.563 ms**; empty OFF grain bucket also large | FAIL as instrumented; isolated material cost NOT RUN |
| Dissolve begins ≤1 frame after incoming presents | Reported maximum **0.000 UI frames**; playing M3 incomplete | PASS observed; missing trials NOT RUN |
| PF1 G1 during playing dissolve | Corrected measured windows **95.522–98.083%** bad; only 80/200 repetitions completed | FAIL; 120 repetitions NOT RUN |
| Cold strip ≤1 s paused / ≤3 s playing | Maxima **8,220.940 / 10,328.323 ms** | FAIL |
| PF1 G1 during fill | Playing windows **99.812–99.910%** bad; worst presented-interval p95 **8,586.342 ms** | FAIL |
| Atlas ≤16 MB | M7 pair **1,474,560 B**; M3 two-pair peak **2,949,120 B** | PASS, texel payload |
| Transparent alpha capability | Surface offers **Opaque, PreMultiplied** | PASS capability; compositing NOT RUN |

| Case | Completed / requested | CPU p50 / p99 ms | Update p50 / p99 ms | Dropped % | Added CPU, one-core % | Scene-error / bad-exit runs |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| m01-paused-on | 200 / 200 | 0.590 / 11.102 | 95.664 / 198.391 | 100.000 | 215.699 | 0 / 0 |
| m01-play-on | 200 / 200 | 1.449 / 58.157 | 128.779 / 269.208 | 100.000 | -5.349 | 0 / 0 |
| m02-paused-on | 200 / 200 | 0.658 / 10.102 | 116.600 / 350.515 | 100.000 | 222.563 | 0 / 0 |
| m02-play-on | 200 / 200 | 1.058 / 56.242 | 121.420 / 305.962 | 100.000 | 2.518 | 2 / 0 |
| m03-paused-on | 200 / 200 | 2.019 / 30.607 | 217.272 / 408.152 | 100.000 | 107.651 | 0 / 0 |
| m03-play-on | 80 / 200 | 0.817 / 47.260 | 131.118 / 352.516 | 100.000 | 68.660 | 2 / 2 |
| m04-paused-on | 200 / 200 | 0.722 / 7.498 | 100.115 / 199.510 | 100.000 | 161.525 | 0 / 0 |
| m04-play-on | 200 / 200 | 1.688 / 55.522 | 116.652 / 220.438 | 100.000 | 32.958 | 0 / 0 |
| m05-paused-on | 200 / 200 | 0.595 / 5.887 | 96.357 / 214.211 | 100.000 | 232.729 | 0 / 0 |
| m05-play-on | 200 / 200 | 2.074 / 58.024 | 121.340 / 248.991 | 100.000 | 52.237 | 0 / 0 |
| m06-paused-off | 200 / 200 | 0.520 / 4.456 | 61.881 / 132.519 | 100.000 | 208.756 | 0 / 0 |
| m06-paused-on | 200 / 200 | 0.619 / 14.088 | 97.711 / 219.429 | 100.000 | 263.287 | 0 / 0 |
| m06-play-off | 200 / 200 | 1.493 / 51.619 | 82.674 / 168.937 | 100.000 | 47.520 | 0 / 0 |
| m06-play-on | 200 / 200 | 2.356 / 83.727 | 114.457 / 213.298 | 100.000 | 32.629 | 0 / 0 |
| m07-paused-on | 200 / 200 | 0.610 / 10.769 | 83.755 / 180.339 | 100.000 | 326.835 | 0 / 0 |
| m07-play-on | 200 / 200 | 0.648 / 40.505 | 86.028 / 195.028 | 100.000 | 76.438 | 1 / 0 |
| m08-paused-on | 200 / 200 | 0.646 / 41.153 | 83.437 / 179.122 | 100.000 | 392.901 | 0 / 0 |
| m08-play-on | 200 / 200 | 33.122 / 199.380 | 106.962 / 216.874 | 100.000 | 12.684 | 1 / 0 |
| m09-paused-on | 200 / 200 | 0.741 / 6.607 | 81.543 / 205.384 | 100.000 | 117.236 | 0 / 0 |
| m09-play-on | 200 / 200 | 2.593 / 62.703 | 106.873 / 298.051 | 100.000 | 42.957 | 0 / 0 |
| m10-paused-on | 200 / 200 | 0.710 / 32.063 | 68.008 / 166.702 | 100.000 | 403.686 | 0 / 0 |
| m10-play-on | 200 / 200 | 0.919 / 46.029 | 102.936 / 248.152 | 100.000 | 57.849 | 1 / 0 |

| Run | Exit | Recorded reps / 40 | Scene errors | Duration s |
| --- | --- | ---: | --- | ---: |
| m02-play-on-p0 | 0 | 40 | audio output stream failed | 154.432 |
| m02-play-on-p1 | 0 | 40 | audio output stream failed | 155.259 |
| m03-play-on-p0 | 0 | 0 | audio output stream failed | 580.560 |
| m03-play-on-p1 | 0 | 40 | audio output stream failed | 234.207 |
| m03-play-on-p2 | TIMEOUT | NOT RUN | no report | 591.745 |
| m03-play-on-p3 | TIMEOUT | NOT RUN | no report | 591.501 |
| m07-play-on-p3 | 0 | 40 | audio output stream failed | 292.123 |
| m08-play-on-p0 | 0 | 40 | audio output stream failed | 39.812 |
| m10-play-on-p1 | 0 | 40 | audio output stream failed | 52.063 |

Corrected M3 completed all 200 paused repetitions but only 80/200 playing: p0 hit the report deadline with no first image and no completed trial; p1 completed 40 with an audio error; p2/p3 hit the parent timeout without reports; p4 completed 40 cleanly. The clean p4 window still failed G1 and reported zero unmatched paint ACKs. The 120 missing/uncompleted playing repetitions receive no credit. All M5 and M6 chunks exited 0 with no scene errors. M6 play ON p4 applied only one light update, below the runner's two-update coverage check. M7 completed all 400 repetitions, with an audio error in play p3; its four other playing chunks independently fail fill/G1.

## Light and filmstrip measurements

These M6 cells are the worst of the per-chunk p50 or maximum, not a pooled frame distribution. “Rest” includes base and remaining scene drawing. OFF contains none of the three effects. Paused PF1 cells are inapplicable rather than zero-cost/zero-failure observations.

**NVIDIA: M6 GPU split.**

| M6 case | Light p50 / max ms | Grain bucket p50 / max ms | Rest p50 / max ms | Combined max ms | Worker max ms | Applied updates, per-run range |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| m06-paused-off | 0.000 / 0.001 | 0.000 / 0.001 | 0.142 / 0.376 | 0.001 | NOT RUN | 0.000–0.000 |
| m06-paused-on | 0.083 / 0.137 | 0.135 / 0.271 | 0.153 / 0.421 | 0.402 | 0.100 | 1.000–1.000 |
| m06-play-off | 0.000 / 0.001 | 0.000 / 0.001 | 0.140 / 0.446 | 0.001 | NOT RUN | 0.000–0.000 |
| m06-play-on | 0.080 / 0.267 | 0.132 / 0.455 | 0.148 / 0.403 | 0.558 | 1.180 | 0.000–53.000 |

**Lavapipe: M6 GPU split.**

| M6 case | Light p50 / max ms | Grain bucket p50 / max ms | Rest p50 / max ms | Combined max ms | Worker max ms | Applied updates, per-run range |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| m06-paused-off | 0.511 / 16.731 | 18.039 / 73.620 | 21.739 / 102.242 | 73.832 | NOT RUN | 0.000–0.000 |
| m06-paused-on | 20.269 / 56.362 | 17.102 / 58.069 | 35.989 / 145.145 | 101.547 | 0.098 | 1.000–1.000 |
| m06-play-off | 0.459 / 14.084 | 16.336 / 64.021 | 23.397 / 109.462 | 64.205 | NOT RUN | 0.000–0.000 |
| m06-play-on | 19.690 / 67.516 | 15.188 / 67.532 | 33.462 / 109.596 | 100.130 | 1.188 | 1.000–11.000 |

The software timestamp split is **not a validated isolated material-pass measurement**: even with all effects OFF, its empty grain bucket has per-run p50 values of 10.505–18.039 ms; ON p50 values are 10.396–17.102 ms and are often smaller than their OFF partner. Query/callback boundaries and base/driver work contaminate attribution. Raw ON combined maxima exceed 2 ms and are reported as instrumented; an intrinsic grain cost or a confirmed grain-specific §6 drop cannot be inferred. Warmup/query cost is included, and no isolated software re-profile was run. The software lane independently fails interval/correctness bars.

The worker distribution contains applied samples only. A missing distribution is not a zero-cost update. The all-scene 17.729 ms NVIDIA elapsed maximum can include scheduling delay; there was no isolated computation profile. The measured worker overrun still misses the stated elapsed per-update bar.

“Cold” filmstrip means the app atlas and synchronous thumbnail decoder cache were cleared. Playback reader rings, shader caches and the OS page cache were retained; no kernel cache was flushed. Each tile is 144×80. Requests go one at a time through the existing PF1 R-4 bounded agent lane, which alternates agent work with runnable transport attempts; the probe adds no second player or bypass. A thumbnail seek/render is synchronous and cannot be preempted mid-request, so yielding between requests does not certify G1 under that workload. Tile latency includes queueing, rendering/readback and resize, not just codec decode. Fill completion is the first paint with all 16 tiles, and PF1 windows include final upload plus two further UI turns. M3's two atlas fills are setup/prewarm measurements, not M7 repetitions.

**NVIDIA: filmstrip.**

| Scene / case | Tile p50 / p95 ms | Strip fill max ms | Window G1 bad % | Window presented p95 ms |
| --- | ---: | ---: | ---: | ---: |
| m03-paused-on | 750.848 / 1240.306 | 12377.490 | NOT RUN | 19.866 |
| m03-play-on | 842.701 / 1662.103 | 13350.268 | 52.454 | 83.963 |
| m07-paused-on | 419.568 / 1091.627 | 14316.344 | NOT RUN | NOT RUN |
| m07-play-on | 403.607 / 652.360 | 7982.817 | 94.802 | 2101.786 |

**Lavapipe: filmstrip.**

| Scene / case | Tile p50 / p95 ms | Strip fill max ms | Window G1 bad % | Window presented p95 ms |
| --- | ---: | ---: | ---: | ---: |
| m03-paused-on | 1849.186 / 3618.972 | 35776.467 | NOT RUN | NOT RUN |
| m03-play-on | 1727.632 / 2822.877 | 32113.088 | 98.083 | 392.239 |
| m07-paused-on | 420.131 / 694.551 | 8220.940 | NOT RUN | NOT RUN |
| m07-play-on | 540.200 / 824.876 | 10328.323 | 99.910 | 8586.342 |

NVIDIA M7 play p1 and p3 had no scene error and still failed fill/G1, so that result is not confined to audio-error chunks. M6 effects-OFF whole-run PF1 bad-frame rates were 3.023–62.326%, median 3.963%; those counters include startup, unlike the M3/M7 active-window counters. They do not establish an unchanged steady baseline or isolate the fill's causal effect. The absolute fill/G1 requirements remain unmet.

An incoming native texture is acknowledged on its first paint, including opacity ≥1/255, through the existing PF1 marker. Held stills copy the existing bound display texture; interrupted dissolves flatten the current blend on the same device. There is one player/presenter. A zero ready-to-dissolve delay does **not** mean instant request-to-image response: NVIDIA playing M3 trial repaint windows reached 73.198 seconds. Media readiness and post-ready dissolve delay are distinct.

## Memory at the actual presenter size

These are texel payloads, not allocator/driver reservation, RSS or total VRAM. MB is decimal. The normal preview path remains Proxy 1280×720; nothing selects a new full-resolution presenter path. The opt-in maximum-width request is bounded by the existing Proxy clamp, and the unset path retains its normal cap.

| Allocation | Actual 1280×720 presenter | 1920×1080 arithmetic projection only |
| --- | ---: | ---: |
| One held RGBA8 still | **3,686,400 B / 3.6864 MB** | **8,294,400 B / 8.2944 MB** |
| Interrupted dissolve, two held stills | **7,372,800 B / 7.3728 MB** | **16,588,800 B / 16.5888 MB** |
| Light + glass, 2×32×18×4 | **4,608 B** when allocated | Same fixed dimensions: 4,608 B |
| Grain, 128×128×4 | **65,536 B** | Same fixed dimensions: 65,536 B |
| Full light/glass/grain texture payload | **70,144 B** | Same fixed dimensions: 70,144 B |
| One 16-tile colour + neutral atlas pair | **1,474,560 B** | Tile dimensions unchanged; not projected |
| M3's two prewarmed atlas pairs | **2,949,120 B** | Tile dimensions unchanged; not projected |

The projected single 1080p still explains §5's approximately 8.3 MB. It is not a measured 1080p allocation. The one missing-light NVIDIA chunk is identified above rather than counted as a full 70,144-byte allocation.

## Spring equations and deadline variant

Riel's clarification is binding: the primary run keeps mass 1, period 350 ms, the specified damping ratios, semi-implicit Euler substeps ≤0.25 ms on `stable_dt`, and stopping thresholds `<0.25 pt` and `<1 pt/s`. Conflicts between spring equations/stopping thresholds and a declared token/window are **DESIGN-DOC CONFLICT**, never a toolkit FAIL and never a §6 drop. Scheduling/end-time misses remain separate measured budget failures. Retargeting preserves velocity. Reduce motion remains a setting; OS detection is out of scope.

For the NVIDIA 336 pt critically damped panel, the 200 paused completion samples had median **633.048 ms** (625.054–649.852 ms); the 200 playing samples had median **633.073 ms** (621.972–653.543 ms). These are transition-duration samples, not pooled CPU quantiles. The maximum recorded primary repaint window was 654.882 ms; motion-layer repaint count after actual settle was zero.

Fixed-step numerical runs from rest show why one window cannot describe every travel distance:

| Token / nominal window | 32 pt at 60 Hz | 64 pt | 160 pt | 336 pt |
| --- | ---: | ---: | ---: | ---: |
| Spring, ζ=0.85 / 488 ms | 333.33 ms | 516.67 ms | 566.67 ms | 600.00 ms |
| Panel, ζ=1 / 514 ms | 483.33 ms | 533.33 ms | 583.33 ms | 633.33 ms |

All equation/window mismatches receive the same conflict classification, including early finishes. Actual NVIDIA `Spring` observations include 3.85 pt landing radii at 278.183–334.653 ms, 13.5 pt pin growth at 307.643–339.713 ms, 36 pt version thumbs at 321.829–351.650 ms, and 64 pt rows at 502.344–542.922 ms. Retargeted animations can carry velocity; these observations are not all rest-to-rest experiments. The numerical critical-panel result is 625 ms at 120/144 Hz timesteps. Those are **numerical samples, not native 120/144 Hz GUI lanes**. Raw numerical data: `target/po0-probe/springs-final.json`.

Analytically, the 336 pt critical response at 514 ms still has 0.337857 pt distance and 5.472147 pt/s velocity; at 520 ms, 0.306552 pt and 4.970721 pt/s. The separately labelled variant clips integration at **520 ms**, records the residual and snaps. Its paint occurs on the next rendered frame; an integrated deadline is not a claim of physical scanout at exactly 520 ms.

**Deadline-snap-520ms, NVIDIA 60 Hz, separate variant:** 400/400 repetitions recorded (200 paused + 200 playing requested), 400 snap residuals. Absolute residual distance **0.317780–0.318481 pt** and absolute residual velocity **5.104563–5.111405 pt/s**. Both signs occur when opening and closing. Recorded completion wall times **520.095–564.445 ms**, median **532.996 ms**; integrated times **520.000–520.000 ms**. This tests a clipped integration deadline on rendered frames, not exact physical scanout. Variant process exits: {'0': 9, '-6': 1}. Scene-error chunks: ['m01-play-on-p4-snap520']. Playing chunks without a first preview: none. The variant does not replace primary physics or trigger a spring drop.

| Deadline variant bar | Measured | Verdict |
| --- | --- | --- |
| CPU p99 ≤10 ms | Worst per-chunk p99 **6.369 ms** | PASS |
| Dropped animated updates ≤0.5% | Worst **1.537%** | FAIL |
| Added CPU ≤10% of one core | Worst **61.007%** | FAIL |
| Repaint window ≤520 ms | Per-chunk maxima **535.311–573.586 ms** | FAIL, rendered-frame/deadline delay |
| Motion-layer repaints after actual snap | **0** | PASS M1 |
| Runtime reliability | 9/10 clean exits; play p0 aborted after its report; play p4 reported an audio error | FAIL |

The variant's deadline misses are reported separately from the primary equation/window conflict. They do not override Riel's instruction to preserve primary equations or trigger the Editor-120-Hz spring drop. The runner-only correction in the companion commit changes this classification; the measured native binary and every duration/residual remain unchanged. Original raw variant tables retain their old shared M1 conflict label, superseded by the explicit corrected interpretation in `accepted/`.

Final C3 captures are under `target/po0-probe/captures-accepted/`. Five of the six separate capture processes exited 0; the active M3 capture saved its PNG and then aborted (SIGABRT, exit −6). That failed shutdown is retained in the load appendix and receives no quantitative credit. Screenshot-only runs close before emitting their final performance report, so they supply images and load snapshots, not acceptance timing/repetition credit. The original M3 prewarm shot is retained but the active shot below is the review artifact.

| Moment / evidence | Artifact | Observation |
| --- | --- | --- |
| 3, active version switching | [scene](../../../target/po0-probe/captures-accepted/m03-active/scene.png) | 2/40 switch repetitions; native picture, lit room and prewarmed strip visible |
| 6, materials ON | [scene](../../../target/po0-probe/captures-accepted/m06/scene.png) | Picture-derived glow, rail/composer glass and grain |
| 10, markup + existing tracked pin | [scene](../../../target/po0-probe/captures-accepted/m10/scene.png) | Circle and blue pin visible at 8/40; screenshot alone does not certify uninterrupted playback |
| Primary panel, every rendered frame in the capture window | [13 original frames](../../../target/po0-probe/captures-accepted/panel-primary/frames) | Picture-region changes stop after frame 7 in this capture |
| Deadline-snap panel | [18 original frames](../../../target/po0-probe/captures-accepted/panel-snap520/frames) | Snap visible frame-by-frame as a small picture-edge raster step at frames 5→6; subsequent frames identical in the measured region |

For the snap, frames 5→6 have mean absolute RGB difference **0.349986/255** and maximum **255/255** in the picture region `1950x1020+300+100`; the preceding pair has **0.070211/255**, maximum **9/255**. [An 8× nearest-neighbour edge crop](../../../target/po0-probe/captures-accepted/panel-snap520/edge-f5-left-f6-right.png), frame 5 left / frame 6 right, exposes a one-pixel raster-edge shift. Original frames are unchanged. Full ROI differences are in `captures-accepted/panel-frame-differences.json`. Thus **yes, the snap is detectable in a frame-by-frame capture**, with a small full-window displacement. Riel's normal-viewing visibility/feel judgement is NOT RUN. File indices do not encode a fixed timestep; the primary and snap captures have different numbers of rendered frames. These are egui framebuffer captures, not physical scanout.

## Drops and toolkit decision

In §6 order:

1. **True backdrop blur:** already absent; glass re-paints the tiny ambient texture/mesh.
2. **Software grain:** not confirmed. The instrumented combined software material bucket exceeds 2 ms, but the nominally empty OFF grain bucket is also large. Valid isolated grain attribution is NOT RUN; the raw failure alone cannot prove that grain's pass is responsible. This is a measurement limitation, not a PASS for grain. WARP is NOT RUN.
3. **Live light during playback:** **triggered** by the NVIDIA worker elapsed-time overruns. Freeze light during playback, updating on version switch and while paused; retain the glow. The GPU combined budget also fails, so this fallback is not claimed to resolve every material cost.
4. **Editor springs:** **not triggered**. Native Editor 120 Hz is NOT RUN. Spring document conflicts trigger no drop.
5. **Filmstrip thumbnails:** **triggered**. Restrict filling to paused; since paused fill also exceeds 1 s, the ordered fallback reaches plain beat blocks with one thumbnail at each beat's start. Absolute fill/G1 failures are measured; causal attribution relative to an already imperfect whole-run baseline is limited.
6. **Note flight:** no confirmed trigger. Dropped updates occur during flight, but there is no flight OFF/ON control to establish that flight cost them. That causal check is NOT RUN; the landing remains if flight is later removed.

Generic Studio 60 Hz CPU/drop/curve failures have no additional named drop in §6. The probe does not apply a rollout or certify the composition/performance after these fallbacks. Neutral surround, Reduce motion and version dissolve remain.

Of the three toolkit conditions: proximity to the two-day NVIDIA 120 Hz p99 >8 ms condition is unassessed because native 120 Hz and that profiling period are NOT RUN; the measured 60 Hz p99 failure is not a substitute. The outgoing frame was held without a second presenter pipeline on both measured adapters, so that condition was not approached. Proximity to the text-in-motion condition is unassessed because Riel's judgement and a snapping remedy are NOT RUN. No toolkit-replacement condition is established by these data.

## Lanes not run

| Lane | §6 bar | Verdict |
| --- | --- | --- |
| NVIDIA 120 Hz | CPU p50 ≤3 ms, p99 ≤6 ms; interval p99 ≤10.417 ms | NOT RUN |
| NVIDIA 120 Hz | Dropped animated updates ≤0.5%, never two consecutive | NOT RUN |
| NVIDIA 120 Hz | Added CPU, settle/end timing, dissolve/G1, light and filmstrip bars | NOT RUN |
| NVIDIA 144 Hz, additional lane | No separate §6 144 Hz acceptance bar; cannot substitute for 120 Hz | NOT RUN |
| Windows VM/WARP 60 Hz | Interval p99 ≤33 ms, dropped updates ≤5% | NOT RUN |
| Windows VM/WARP | Materials ≤2 ms/frame; filmstrip fill/G1/atlas bars | NOT RUN |
| Windows VM/WARP | Mica composites with the wgpu swapchain | NOT RUN; Mica not implemented |

No display mode was changed. The 120 Hz modes were offered but not active; the additional native 144 Hz lane was not exercised. Replay commands are below.

## Verification and remaining scope

C3 passed `cargo build --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt -- --check`, the two focused media regressions (version-epoch/audio continuity and unset normal preview cap), ten existing PF1 accounting tests, and the unset app-state test. Every Cargo command used the same-shell FFmpeg setup, available-memory check, four jobs, niceness and shared lock. Logs: `target/po0-probe/r5-{build,clippy,fmt,epoch-green,stats-tests,app-unset}.log`. The epoch regression is red without the fix (`r5-epoch-red.log`: expected two late ACKs, got one) and green with it. The release binary was rebuilt and frozen before timing. The final runner-only classification change passed a Python syntax check and direct physical-versus-deadline checks; Rust sources/binaries were unchanged afterwards.

C2's focused app/motion tests (24) and light tests (2) passed. They include velocity-preserving interruption, equation-driven stopping, one-frame-after-settle repaint cessation, Reduce motion, list bounds/queued inserts, bezier endpoints, zero-mean grain and normal startup without motion requests or PO0 textures. The app and agent one-loop/one-pump architecture checks passed. Slow-test manifest lint passed; no slow markers changed.

Earlier full touched-crate fast tests were **not all green**: app 886 passed / 1 failed / 6 ignored (`c9_fix1_unsaved_studio_activation_requires_first_save_before_manual_edit`, an `Option::unwrap` in existing attachment setup; its isolated-XDG rerun passed); agent 736 passed / 1 failed / 2 ignored (missing sealed max-2 predecessor prerequisite); media library 1,134 passed / 65 ignored, with its integration and doc tests passing. Those broad runs precede the final small probe fixes; they are archived evidence, not final-head full-suite acceptance. Full workspace fast/slow suites and CI were NOT RUN. A Linux-to-MSVC check stopped in the native `aws-lc-sys` prerequisite (`pthread_rwlock_t`); it is a setup failure, **not Windows compilation evidence**. Native Windows build/test acceptance is NOT RUN.

The flag is checked before constructing the probe or its GPU queries/textures and before sending probe-only worker operations. With it unset, normal document/startup state, normal performance schema and repaint behavior are retained. The unset preview-cap test additionally confirms that both the normal width request and the probe's ineffective 1920-width request are clamped by `RenderScale::Proxy` to **1280×720**. No normal preview-cap change is reachable with the flag unset.

The following remain **NOT RUN**, rather than inferred PASS:

- Native NVIDIA 120 Hz and 144 Hz, the Editor-specific 120 Hz spring fallback, consecutive-drop acceptance at 120 Hz, and the two-day Studio 120 Hz profiling condition. Neither an offered mode nor fixed-step spring arithmetic is a native lane.
- Windows VM/WARP, native Windows build/tests, Mica implementation/compositing, and transparent-window desktop compositing. Only Wayland surface alpha **capability** was queried.
- Riel's five-minute feel check, near-black banding, text-in-motion judgement and whether a snapping remedy satisfies Riel. The frame-by-frame snap observation above is an implementation observation, not Riel's approval.
- Instrumented press-event-to-visible-paint latency, complete manual-transition end-time coverage, motion-layer repaint cessation across every scene (M1 is measured), and physical display scanout/capture timing.
- Valid isolated software grain/material GPU attribution and a timestamp-instrumentation-OFF interval control; the raw OFF/ON controls are recorded. Isolated light computation CPU profiling and all-produced/discarded light-update timing are also NOT RUN; applied-sample elapsed timing is measured.
- A note-flight OFF/ON dropped-frame control, unchanged steady-state PF1 G1 relative to a matched fill-free baseline, composition/performance after ordered fallbacks, and uninterrupted playing coverage in error/time-out/no-first-image chunks. Missing repetitions are listed per case; they receive no credit.
- A clean primary matrix after terminating the accumulated startup CLI checks; those checks were part of recorded load, not filtered from it.
- Full final-head workspace fast/slow tests and CI, including tests skipped by slow-feature/ignore gates. No push or CI run was requested. The Windows cross-check's prerequisite failure receives no compile credit.

OS Reduce motion detection is explicitly out of scope; the setting is implemented. A full-1920×1080 presenter variant is explicitly excluded by Riel; only its held-frame arithmetic is projected. The material-fallback composition and the product rollout were not applied.

## Replay

Accepted manifests are under `target/po0-probe/accepted/`; each points to original `nvidia-60`, `nvidia-60-r5`, `lavapipe-60`, `lavapipe-60-r5` or `nvidia-60-snap520` JSON. Raw commands, source/binary hashes, FFprobe metadata, per-run load and errors are in those original `results.json` files; automated per-run checks are in their `results.md`. A raw automated PASS for “clock advanced” is initial advancement only, not uninterrupted visible playback. Smoke/capture folders are separate and are not substitutes for this matrix.

Build only with ≥6 GiB available and the same-shell FFmpeg environment, shared lock and limits. For example:

```bash
source scripts/setup-ffmpeg.sh
flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 nice -n 10 cargo build --release -p kinewright-app
```

To replay a complete primary matrix with the corrected native binary and a fresh output:

```bash
source scripts/setup-ffmpeg.sh
nice -n 19 python3 -I scripts/po0-probe.py --lane nvidia --hz 60 --binary target/po0-probe/bin/kinewright-app-60e8befc-23f796a7970c66cc --media target/po0-probe/source-vui-1080p30-11min.mp4 --output target/po0-probe/nvidia-60-replay
```

```bash
source scripts/setup-ffmpeg.sh
nice -n 19 python3 -I scripts/po0-probe.py --lane lavapipe --hz 60 --binary target/po0-probe/bin/kinewright-app-60e8befc-23f796a7970c66cc --media target/po0-probe/source-vui-1080p30-11min.mp4 --output target/po0-probe/lavapipe-60-replay
```

The unrun refresh-rate commands below are for a monitor **already** at the requested rate. They do not authorize changing a mode. Run one lane at a time and poll its log.

```bash
source scripts/setup-ffmpeg.sh
setsid nohup nice -n 19 python3 -I scripts/po0-probe.py --lane nvidia --hz 120 --binary target/po0-probe/bin/kinewright-app-60e8befc-23f796a7970c66cc --media target/po0-probe/source-vui-1080p30-11min.mp4 --output target/po0-probe/nvidia-120 > target/po0-probe/nvidia-120.log 2>&1 &
```

```bash
source scripts/setup-ffmpeg.sh
setsid nohup nice -n 19 python3 -I scripts/po0-probe.py --lane nvidia --hz 144 --binary target/po0-probe/bin/kinewright-app-60e8befc-23f796a7970c66cc --media target/po0-probe/source-vui-1080p30-11min.mp4 --output target/po0-probe/nvidia-144 > target/po0-probe/nvidia-144.log 2>&1 &
```

To replay the incomplete NVIDIA cases after addressing their runtime failures, use the same 60 Hz command with `--moments 2,3,5,6,7,8,10` and a fresh output directory. This retains the original failed dataset. For software coverage, replay C3 with `--lane lavapipe --hz 60 --moments 3 --output target/po0-probe/lavapipe-60-m3-replay` after addressing its first-image/audio/time-out failures. A fresh output preserves all original attempts.

For the lead's Windows VM, provision Windows/MSVC and Windows FFmpeg first using `docs/BUILDING.md`, build the release app, and put the generated source at the named path. **WARP was explicitly not run.** This is the exact runner command:

```powershell
$env:WGPU_BACKEND = 'dx12'
$env:WGPU_ADAPTER_NAME = 'Microsoft Basic Render Driver'
py -3 -I scripts/po0-probe.py --lane warp --hz 60 --binary target/release/kinewright-app.exe --media target/po0-probe/source-vui-1080p30-11min.mp4 --output target/po0-probe/warp-60
```

The resulting adapter report must identify WARP; another adapter is not a WARP result. Mica application/compositing is not implemented or tested by this probe command.
