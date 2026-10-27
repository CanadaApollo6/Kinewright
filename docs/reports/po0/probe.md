# PO0 motion probe

This report belongs to the implementation on `po0/probe`. No rollout or push is authorized by this probe. All scene, texture, full-resolution preview, adapter-selection, and shutdown changes require `KINEWRIGHT_MOTION_PROBE=1`. Normal launches retain the existing UI and preview cap.

## Source and measurement

No readable video was found in the checkout or `~/Videos`. The source is a generated 660-second, 1920×1080, 30 fps H.264 test pattern with a silent AAC track. Its path is `target/po0-probe/source-vui-1080p30-11min.mp4`. It is not a talking-head recording and is not committed. The seeded pin is a fixed region, not a person detector.

The probe uses the existing player, native presenter textures, and PF1 paint callback. An outgoing still is copied into a separate texture. An interrupted dissolve flattens the current blend into a still on the same GPU device. It does not create another player. Incoming images retain their first-paint acknowledgement, including low-opacity paints.

GPU timestamp boundaries surround the actual base, light and glass, grain, and remaining scene draws. Unsupported timestamp features produce missing measurements. The report does not infer GPU time from CPU timing. Update intervals are callback intervals, not physical display presentation times.

The filmstrip has 16 144×80 tiles in a colour atlas plus a neutral atlas. Each cold fill clears synchronous thumbnail decoders and the atlas, while retaining playback reader rings. Thumbnail jobs use the existing transport-prioritized analysis lane. PF1 windows include the final upload and two further UI turns; strip completion uses a dedicated paint callback after all tiles arrive. The tracker calls `track_reframe_subject` through the existing MCP and session pump, with a fixed box; its prepared edit plan is discarded.

The runner writes JSON and Markdown, records the binary SHA-256 and source commit, and runs both paused and playback cases. Every moment uses five runs of 40 repetitions per paused/playback case. Moment 6 alternates effects-off/effects-on, with 200 repetitions per side in total. Every run records load, other processes over 20% CPU at five-second intervals, display state, and GPU power state and clocks. Load is an annotation and does not gate a run.

## Spring specification conflict

Riel's clarification is binding. The primary run keeps the specified equations and the `<0.25 pt` and `<1 pt/s` stopping thresholds. A mismatch between those equations and a declared window is a **DESIGN-DOC CONFLICT**, not a toolkit failure, and triggers no §6 drop.

For a 336 pt critically damped panel, the analytic response at 514 ms has 0.337857 pt residual distance and 5.472147 pt/s velocity. At 520 ms it has 0.306552 pt residual distance and 4.970721 pt/s velocity. Both velocities exceed the stopping threshold. The separately labelled deadline variant snaps at 520 ms and reports its numerical residual before snapping. It does not replace the primary run.

The Rust spring report covers both damping ratios, four travel distances, and 60, 120, and 144 Hz time steps. Those numerical 120 Hz samples are not a 120 Hz GUI lane. Frame captures of the primary and deadline variant are separate from performance runs.

## Lane results

Runtime acceptance is pending the final source build and lane runs. Smoke runs are setup evidence only.

| Lane | Verdict | Scope |
| --- | --- | --- |
| Linux NVIDIA, 60 Hz | NOT RUN | Final-source acceptance pending |
| Linux NVIDIA, 120 Hz | NOT RUN | Neither display currently runs at 120 Hz; display modes are not changed |
| Linux NVIDIA, 144 Hz | NOT RUN | Additional current-rate lane, not a substitute for 120 Hz |
| Linux lavapipe, 60 Hz | NOT RUN | Final-source acceptance pending |
| Windows VM, WARP | NOT RUN | Explicitly excluded from this implementation run |
| NVIDIA Wayland alpha capability | PASS | The actual GUI surface offers `Opaque` and `PreMultiplied` |

## Drops and toolkit conditions

The true backdrop blur is absent by design. No spring specification conflict triggers a drop. Other drop decisions await the corresponding completed lane measurements.

The two-day NVIDIA 120 Hz profiling condition is NOT RUN. A second presenter pipeline is not needed for the held-frame path. Riel's evaluation of text in motion, near-black banding, snap visibility, and the five-minute feel check is NOT RUN.

## Replay commands

Build with at least 6 GiB available, from this checkout, using the shared lock. Use the same FFmpeg environment for the binary and the runner.

```bash
source scripts/setup-ffmpeg.sh
flock -o /tmp/kinewright-cargo-heavy.lock env CARGO_BUILD_JOBS=4 nice -n 10 cargo build --release -p kinewright-app
setsid nohup nice -n 19 python3 -I scripts/po0-probe.py --lane nvidia --hz 60 --media target/po0-probe/source-vui-1080p30-11min.mp4 --output target/po0-probe/nvidia-60 > target/po0-probe/nvidia-60.log 2>&1
setsid nohup nice -n 19 python3 -I scripts/po0-probe.py --lane lavapipe --hz 60 --media target/po0-probe/source-vui-1080p30-11min.mp4 --output target/po0-probe/lavapipe-60 > target/po0-probe/lavapipe-60.log 2>&1
```

For 120 Hz, run the NVIDIA command with `--hz 120` only when a display already runs at that rate. Do not change the display mode for this probe. The runner records the app's actual monitor refresh and refuses to credit a mismatched lane.

Run the separately labelled deadline variant with `--moments 1 --snap-variant`. Set `KINEWRIGHT_PROBE_CAPTURE_DIR` to a directory under `target/po0-probe/` for a separate frame capture. Set `KINEWRIGHT_PROBE_SPRING_REPORT` to a JSON path, together with the probe flag, to run just the numerical spring report without opening a window.

The exact Windows VM command for the lead follows. Build with the repository's Windows FFmpeg setup first, and put the generated source at the named path. This command has not been run.

```powershell
$env:WGPU_BACKEND = 'dx12'
$env:WGPU_ADAPTER_NAME = 'Microsoft Basic Render Driver'
py -3 -I scripts/po0-probe.py --lane warp --hz 60 --binary target/release/kinewright-app.exe --media target/po0-probe/source-vui-1080p30-11min.mp4 --output target/po0-probe/warp-60
```

WARP adapter selection must appear in the resulting report; another adapter is a failed lane. Mica compositing remains NOT RUN.

## Verification

The final-source build, clippy, formatting, full fast tests of touched crates, and Windows compile check are pending. Earlier spring tests passed after preserving the physical equations and separating their window conflict.

An initial NVIDIA smoke run crashed during preview-worker GPU destruction after writing its report. The opt-in shutdown path now drains that worker before eframe tears down the shared device. Subsequent effects smoke runs exited cleanly. Those smoke measurements are not acceptance evidence.
