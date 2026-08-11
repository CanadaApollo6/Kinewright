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

## Preview proxies and decode strategy

Interactive preview uses an in-memory decode proxy capped at 1280 display
pixels wide. Height is calculated from the source display aspect ratio, and
sources narrower than 1280 pixels are never enlarged. A 3840x2160 source is
therefore decoded and composited as 1280x720. This is the same bilinear
swscale conversion used by thumbnails, but `thumbnail_at` continues to honor
the caller's independent `max_width`. Export continues to open full-resolution
decoders and renders at the requested export resolution.

## Titles and the shared compositor path

Titles are declarative video-track clips. The media crate rasterizes their text
with the embedded Inter variable font into a full-frame transparent RGBA layer,
then submits that layer to the same wgpu compositor used for decoded media.
Preview and export both call `FrameRenderer::render`; there is no export-only
text path. Font-size and color indices resolve through the core title descriptor
tables, positions are fixed presets, and fade lengths remain integer project
frames. Title fade alpha multiplies the existing transition/opacity alpha before
the compositor blends layers bottom to top.

Rasterization is deterministic for a title, token set, and output resolution.
Preview proxies rasterize at preview resolution; full-resolution export
rasterizes again at export resolution from the same embedded font bytes and
declarative title data.

Decoder/cache identity includes the asset ID and proxy-width limit. A future
preview resize or zoom that selects another width therefore opens a matching
scaler and cannot reuse pixels from a differently sized proxy. `FrameTexture`
dimensions are authoritative throughout the compositor and UI; the preview
panel scales each delivered texture to fit its available rectangle.

Paused scrubbing uses deterministic keyframe seeks and the existing coalesced
latest-position request. Playback and export use sequential windows. The first
window seeks to the requested source time; directly adjacent windows retain
the demux/decoder cursor and M8 PTS-hold lookahead instead of flushing and
seeking again. A discontinuity still falls back to the seek path. This keeps
the audio master clock and the greatest-PTS-not-after selection rule unchanged.

Sequential rendering prefetches up to 15 frames and retains at most 32 entries per
source/scale. It also enforces a conservative 224 MiB aggregate RGBA cache
budget across sources, counting every grid entry even when held VFR frames
share one allocation. Prefetch shrinks for large full-resolution frames so a
single decode window fits that budget. Scrubbing decodes only its requested
frame because the next coalesced request may supersede it. The remaining
headroom below 256 MiB is for the current decoder lookahead, compositor output,
and channel-owned frame.

## Playback audio mixdown

Playback enumerates audio-bearing clips on every audio and video track for the
remaining project range. The media worker mixes them in document order at
unity gain, then applies the export mixdown's single hard clamp to `-1.0..=1.0`.
Project-frame boundaries are converted directly to device sample boundaries;
each source is trimmed to its source-sample range and padded with silence when
that range cannot fill its mapped project duration. Stream PTS is normalized by
the stream-start offset before trimming. A playback seek rebuilds the source
set at one shared project-sample position, while paused scrubbing remains
video-only.

The worker mixes fixed 1,024-sample-frame chunks and lazily opens a decoder only
when the feeder reaches its clip. Since the output ring holds two seconds, this
normally opens an upcoming boundary nearly two seconds before it is heard.
Completed sources are closed immediately after their final mixed sample. The
audio callback is unchanged: it only pops interleaved samples (or zero on
underflow) and atomically advances the sample-count master clock. A project with
no audio therefore still advances on callback-generated silence.

The two-second output ring is shared by the entire mix rather than duplicated
per source. OpenReel-owned f32 sample storage is
`2 * sample_rate * channels * 4` bytes for that ring, plus
`1,024 * channels * 4` bytes for the feeder chunk, plus one unread resampled
decoder chunk per simultaneously active source. At 48 kHz stereo the fixed
portion is 750 KiB + 8 KiB. For a decoded frame of `F` sample frames, source
staging is normally `F * channels * 4` bytes. Initial PTS-gap padding is capped
at one second, making the explicit per-source ceiling
`(sample_rate + F) * channels * 4` bytes, plus FFmpeg's codec and resampler
state. At 48 kHz stereo with 1,024-frame AAC this is normally 8 KiB and at most
383 KiB when the full gap allowance is used. Thus long timelines do not retain
a two-second buffer or an open decoder per clip; only actual overlap increases
live decoder memory.

## Derived audio and scene analysis

Silence and scene data are reproducible derived assets. They never enter the
project `Document` or operation journal. Import only queues background work;
hashing, cache access, audio decode, and proxy video decode stay off the UI
thread. Cache files are keyed by the source SHA-256 plus the relevant analysis
configuration, written atomically, and treated as misses when corrupt or when
their source metadata or algorithm version does not match.

Silence analysis decodes 48 kHz mono audio and measures non-overlapping 10 ms
RMS windows. The default threshold is -40.00 dBFS. Window boundaries map back
to the asset's rational frame rate using floor starts and ceil ends, and callers
apply an integer source-frame minimum duration (six frames by default).

Scene analysis decodes sequentially at a maximum 320-pixel proxy width. It
combines normalized luma SAD with a 64-bin luma histogram distance, then uses a
temporal-difference score to suppress persistent motion in the same manner as
`scdet`. Confidence is stored as integer basis points. The UI and timeline
tools default to a 10.00% threshold; lower-level cached candidates remain
available for callers that request another threshold.

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

## Preview performance gate

The gated preview performance test generates only the 4K H.264 long-GOP and
1080p HEVC cases. It reports 20 deterministic random cold-cache seeks, a
sequential decode in 15-frame windows, and 20 forward scrub steps, all at the
1280-pixel proxy limit:

```powershell
$env:OPENREEL_PERF_TEST = '1'
cargo test -p openreel-media --lib proxy_preview_performance -- --nocapture
```

The local machine-class budgets are cold-seek p95 below 250 ms, scrub-step p95
below 250 ms, sequential decode at or above 60 fps, and exactly one FFmpeg
seek for the sequential run. The gate is opt-in and is not run in CI.

M9 baseline measured on Windows 10.0.26200, an Intel Core i5-13600K (20
logical processors), and software FFmpeg decode capped at 16 frame threads.
The installed RTX 3080 was not used for video decode:

| file | proxy | cold seek p50/p95 | sequential | scrub step p50/p95 |
|---|---:|---:|---:|---:|
| 4k-h264-g250 | 1280x720 | 89.4/140.7 ms | 125.9 fps | 94.7/120.9 ms |
| hevc-1080p | 1280x720 | 27.0/35.6 ms | 251.0 fps | 24.7/31.0 ms |
