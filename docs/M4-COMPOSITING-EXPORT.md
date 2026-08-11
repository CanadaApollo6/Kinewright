# M4 compositing and export

Preview and export both call `FrameRenderer::render`. That renderer resolves every active video
clip at a project frame, decodes its source frame to RGBA with FFmpeg/swscale, and passes the
ordered layers to the wgpu compositor. Tracks are drawn in document order, so the last video
track is on top. FFmpeg is used only to decode sources and encode/mux the finished video and
audio streams.

The desktop app constructs the media engine with clones of eframe's wgpu `Device` and `Queue`.
This keeps preview compositing on the UI's existing GPU context. Headless users and tests create
the same `GpuContext` through wgpu; compositor tests explicitly request the fallback adapter
(WARP on Windows). Export uses the media engine's same device, compositor implementation, and
frame renderer as preview.

## Effect parameters

Operation payloads stay integer-only. All percentages below use whole percentage points in
`ParamValue::Integer`; there are no floating-point operation values. Missing parameters use the
neutral value.

| Effect | Parameter | Range | Neutral | Meaning |
| --- | --- | ---: | ---: | --- |
| `brightness` | `percent` | -100..100 | 0 | Adds the percentage to each RGB channel. |
| `contrast` | `percent` | -100..100 | 0 | Scales RGB distance from 50% gray by `1 + percent / 100`. |
| `saturation` | `percent` | -100..100 | 0 | Interpolates from luminance at -100 to unchanged at 0 and doubles saturation at 100. |
| `opacity` | `percent` | 0..100 | 100 | Multiplies layer alpha. |
| `transform` | `scale_percent` | 1..400 | 100 | Uniform scale around the frame center. |
| `transform` | `x_percent` | -100..100 | 0 | Horizontal offset as a percentage of project width; positive moves right. |
| `transform` | `y_percent` | -100..100 | 0 | Vertical offset as a percentage of project height; positive moves down. |

Multiple effects are folded into one shader parameter block. Brightness and offsets add;
contrast, saturation, opacity, and scale multiply. The result is clamped by the shader.

M4 introduced `crossfade`; the current registered transition set and exact shading/audio semantics
are defined by the core descriptor table and documented in [M20 transitions](M20-TRANSITIONS.md).
`transition_in.duration` remains a positive number of project frames no longer than the clip, and
a one-frame transition is fully visible on that frame.

## Export and cancellation

Export converts the project duration to the requested output frame rate, renders each output
frame through `FrameRenderer`, reads the RGBA target back from wgpu, converts it to YUV420P, and
encodes H.264. Independently, every audio-bearing clip on every track is decoded only across its
trimmed source range, resampled to 48 kHz stereo, and added at sample-accurate timeline offsets.
The clamped mix is encoded as AAC and muxed with the video into MP4.

`ExportSettings::cancellation` is a clonable `ExportCancellation` handle backed by an atomic
flag. Calling `cancel()` is observed between decoded audio chunks, video frames, and audio encoder
chunks.
The exporter writes to an adjacent part file and removes it on error or cancellation, so a
cancelled export does not leave a partial destination. `ProgressSink` reports 0 initially and
then one update per fully composited and submitted video frame.

## Manual verification fixture

With the documented FFmpeg build environment active, run:

```powershell
cargo run -p openreel-media --example m4_verify -- target/m4-manual
```

The example generates two 2-second A/V source clips, builds `two-track.openreel` through effect
and transition operations, samples the preview compositor, advances real playback, and exports
`two-track-export.mp4`. The source tones are 440 Hz and 660 Hz, the upper blue layer has 65%
opacity, and its crossfade-in lasts 15 frames. The generated directory is under `target` and is
safe to remove after verification.
