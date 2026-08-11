# M21: Per-clip audio and master peak meters

M21 adds integer document controls for each media clip's audio contribution and a compact stereo master-output meter. It does not change video contribution, clip timing, or the audio stored in source assets.

## Document model and operation

Every `Clip` has three backward-compatible fields. They default to zero and are omitted when serialized at their defaults:

- `audio_gain_tenth_db: i32` is gain in tenths of a decibel. Its valid range is `-600..=120`, or -60.0 dB through +12.0 dB.
- `audio_fade_in_frames: TimeCode` is a non-negative number of project frames.
- `audio_fade_out_frames: TimeCode` is a non-negative number of project frames.

The sum of the two fades cannot exceed the clip's project duration. These fields apply only to the clip's audio contribution. Title clips have no audio and cannot receive audio settings.

`SetClipAudio` is an idempotent full-set operation. It replaces all three values together. Operation validation and full-document validation enforce the same gain and fade rules, so invalid hand-edited project files are rejected during load. Snapshot undo restores the exact previous document.

## Per-sample composition

`ClipAudioShaping` is constructed once for each playback source or export segment. It owns the constant gain and optional `AudioGainRamp` values for fade-in, fade-out, and the M20 transition ramp. Both real-time mixing and export call the same `gain_at(project_sample)` evaluator with an absolute project-sample index.

The evaluator uses this exact `f32` order:

```text
constant_gain = 10^(audio_gain_tenth_db / 200)
effective_gain = ((constant_gain * fade_in_ramp) * fade_out_ramp) * transition_ramp
```

Missing ramps contribute `1.0`. Fade and transition frame windows use `frame_to_samples`. As with the M20 transition ramp, windows of zero or one frame are no-ops. Longer ramps use the existing duration-minus-one sample denominator, so their endpoints and curve convention match. Fade-in starts at the clip's project start. Fade-out is anchored to the clip's project end, `timeline_start + clip_duration`, regardless of source range placement.

Real-time feeder accumulation uses its absolute-sample counter. Export uses its absolute output-sample index. Keeping construction and arithmetic in the shared helper makes those paths bit-identical before their common limiting step.

## Master output meters

Playback owns an `Arc<MeterState>` with one `AtomicU32` per output channel. Each atomic stores the bit representation of the most recent `f32` peak. The mixer measures the absolute per-channel peak after clamping, at the point where a completed mixed chunk is handed toward the audio stream. This is the post-limiter signal the device receives. Recording performs no allocation and takes no lock.

The `Playback` facet exposes one read-only `output_peaks() -> [f32; 2]` method. This keeps the surface narrow: the app receives only current stereo output telemetry, with no mixer, stream, or mutable meter internals crossing the facet boundary.

The transport maps amplitude to a -60 dBFS floor and renders two thin horizontal bars. Fill is segmented by level: success through -12 dBFS, warning above -12 dBFS, and danger above -3 dBFS. The UI rises immediately to a new peak and applies a fixed-rate frame-time-based fall-off. It continues requesting repaint while residual meter state drains. Stopping or pausing playback makes the displayed meters decay to silence, while the engine clears its live meter state immediately.

## Inspector and agent surface

The media inspector shows Audio controls when the selected clip's asset carries audio or a linked member carries it. If the selected clip carries audio, edits target it. Otherwise the linked audio-carrying member is the target. Gain is edited in 0.1 dB integer steps; fade lengths are integer frames and are mutually bounded to the target clip duration. Each edit emits one full `SetClipAudio` operation. Reset sets gain and both fades to zero.

Agent operation schema describes the fixed-point gain unit, bounds, clip-end fade-out anchoring, and transition composition. Timeline rendering appends an `audio=gain:...,fade_in:...f,fade_out:...f` suffix only when at least one value is non-default.

## Deferred work

The following are explicitly outside M21 and remain future work:

- Keyframed gain envelopes.
- Per-track meters.
- Loudness measurement, including LUFS.
