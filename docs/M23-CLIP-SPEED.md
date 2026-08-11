# M23 Constant-Rate Clip Speed

M23 gives media clips an integer playback speed: `Clip.speed_percent`, range
10..=1000, default 100. A clip at 50 plays in slow motion over twice its
real-time project duration; 200 halves it. Titles and freeze frames have no
speed — their durations are already project-local.

## The effective-fps principle

Speed is implemented as exactly one idea: the clip consumes its source as if
the source ran at `asset_fps * speed_percent / 100`. That scaled rate is an
exact reduced rational (`speed_scaled_fps`), and `clip_effective_fps` is the
single helper every source-to-project mapping goes through — duration
(`Document::clip_duration`), splitting (`find_source_boundary`), trimming,
decode positioning (`media_source_for_clip`), interactive edge-trims, and the
agent's rendered durations. No call site scales frame rates itself, so the
integer-exact mapping guarantees (shared boundaries, no cumulative drift)
carry over to every speed unchanged. A split's right half inherits the speed,
and the contract tests prove split adjacency and total-duration conservation
across speeds from 10 to 1000 at NTSC rates.

## The operation

`SetClipSpeed { clip, speed_percent }` is pure and non-rippling: it fails
(atomically) if the new duration would overlap a later clip. The inspector's
speed slider builds the ergonomic form: when growth would collide on the
clip's track it first opens a `RippleInsertGap` at the clip's current end —
shifting sync-locked tracks and markers together — then applies the speed.
Shrinking leaves a gap by design (ripple-delete closes it when wanted).
Linked A/V members take the same speed in the same batch; anything else would
structurally desynchronize the pair.

## v1 boundaries (deliberate)

- **Audio is muted at any speed other than 100.** Varispeed shifts pitch and
  pitch-preserving stretch is real DSP work; silence is the honest middle
  ground, and slowed b-roll is usually muted anyway. The mixer and export
  skip speeded clips identically (`timeline_audio_segments` is their shared
  source), and the agent tool documentation says so.
- **Derived timeline mappings skip speeded clips.** Transcript words, silence
  spans, and scene changes no longer align project-linearly once a clip is
  resped; remapping them through the effective rate is mechanical but
  deferred to keep this change reviewable.
- **No speed ramping.** Keyframed speed interacts with VFR mapping and is a
  separate, deliberate milestone.
- No dedicated H.264 speed parity test: speed adds no render code — preview
  and export share `media_source_for_clip`, whose mapping is unit-tested, and
  the existing parity suite covers the shared path.

## UI

The inspector's Speed slider displays a multiplier (0.10x..10.00x) backed by
the integer percent, with a muted note while audio is disabled. Clips at any
non-real-time speed draw a small monospace multiplier badge at their top-right
on the timeline.
