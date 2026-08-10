# Media correctness policy

OpenReel's project timeline is constant-frame-rate (CFR). Every project and
source edit boundary remains an integer `TimeCode` plus an exact rational frame
rate. Importing variable-frame-rate (VFR) media does not change that model.

## VFR mapping

When FFmpeg's `avg_frame_rate` and `r_frame_rate` diverge, probe examines video
packet durations and presentation timestamps. Variable packet timing selects
`avg_frame_rate` as the asset's effective CFR rate; constant timing selects
`r_frame_rate`. This avoids treating a CFR file's last-PTS span as its frame
rate while still recognizing phone-style VFR. If only one valid rate exists,
probe uses it. Duration comes from the stream timestamp duration, rounded up
onto that effective grid. Probe also measures the final video packet end and
uses the longer of the packet and declared video-stream durations. Container
duration is only a fallback because its coarse time base can otherwise create
a phantom final frame. This includes a final CFR/VFR picture whose stream
duration stops at its PTS without trusting `nb_frames`, which does not describe
elapsed time for VFR sources.

Effective source frame `n` represents source time `n / effective_fps`. Decode
selects the source picture with the greatest presentation timestamp (PTS) not
after that time. A picture is therefore held until the next source PTS. Before
the first PTS, the earliest decoded picture is used; after the last PTS, the
last picture is held through the probed duration. Equal-PTS pictures resolve in
stable decoder order.

Seeking converts the requested integer source frame to the same timestamp,
seeks to a keyframe at or before it, decodes forward, and applies the same PTS
selection rule. Repeated seeks therefore land on the same picture, including
with long GOPs.

This policy has no cumulative A/V drift: video selection and audio range
boundaries are both derived from the same effective rational time base. At any
instant, a VFR picture can be older than the CFR grid time by less than the
current source inter-frame interval. That visible hold can be long in a
pathological source, but it does not grow across the clip. Edit boundaries can
also differ from an intended wall-clock point by less than one effective source
grid frame because edits are still integer frames.

Audio-only assets use the existing 30/1 source grid solely to express integer
edit boundaries; audio decode and export resample from the stream's real sample
rate, channel layout, and PTS.

## Rotation metadata

Display-matrix side data is authoritative, with the legacy `rotate` stream tag
as a fallback. OpenReel applies 0, 90, 180, and 270 degree rotations during the
shared decode path. Probe reports post-rotation display dimensions, and width
limits are calculated in display orientation. Preview, cached thumbnails,
compositing, and export all consume those same rotated RGBA frames.

Reflected matrices and non-right-angle rotations are rejected with an explicit
error instead of being rendered incorrectly.

## Unsupported or damaged media

Probe verifies that the linked FFmpeg build has decoders for every selected
audio and video stream. Missing decoders identify the media kind, codec, and
file. Container-open, seek, packet-decode, pixel-conversion, and audio-resample
failures include the file and operation; damaged-container errors state that
the file may be truncated. These errors flow through `MediaError` and the
existing application error log. A failed decode never substitutes a silent
black frame.

## Torture-matrix generator and tests

The generator is the test-support module
`crates/openreel-media/src/media_matrix_tests.rs`. It invokes only the
provisioned `ffmpeg.exe`, writes into a unique temporary directory, and removes
the generated files after the test. No fixtures or codec binaries are checked
in.

The default fast matrix generates small VFR, rational CFR, long-GOP, HEVC,
portrait, rotation-metadata, odd-rate audio, audio-only, video-only, and very
short sources. It verifies probe data, first/middle/last decode, deterministic
seek, applicable audio resampling, damaged-input errors, and one mixed-timeline
export. Run it with:

```powershell
cargo test -p openreel-media --lib fast_media_matrix -- --nocapture
```

The gated matrix adds 3840x2160 H.264 with a 250-frame GOP and three-second
duration plus 1920x1080 HEVC. It performs the same checks and prints a per-file
result table:

```powershell
$env:OPENREEL_MEDIA_MATRIX = '1'
cargo test -p openreel-media --lib full_media_matrix -- --nocapture
```

The provisioned FFmpeg build contains `libx265`. The generator detects it; if a
different supported build lacks it, the test uses `libx264` as the documented
fallback and prints that substitution in the matrix output.
