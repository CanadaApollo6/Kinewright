# M20 transitions

M20 completes the transition-in path with three registered transition types:

| Name | Video semantics |
| --- | --- |
| `crossfade` | Ramps the entering layer alpha from 0 to 1, revealing already-composited lower layers or black. |
| `fade_from_black` | Starts the entering layer as solid opaque black and mixes to its content. |
| `fade_from_white` | Starts the entering layer as solid opaque white and mixes to its content. |

The color transitions are deliberately named `fade_from_*`, not dips. A transition-in covers only
the entering clip's first `duration` project frames; it does not span an outgoing and incoming
clip as a dip would.

## Frame and occlusion rules

Transition duration is a positive integer number of project frames and cannot exceed the clip
duration. Durations of one frame are fully visible no-ops. For longer transitions, frame progress
is `offset / (duration - 1)`, so the first frame is at the transition start and the last frame in
the window is fully visible.

Crossfade uses that progress as layer alpha. Color fades keep layer alpha at 1, mix processed RGB
toward black or white by `1 - progress`, and force fragment alpha to 1 while the color mix is
non-zero. The opaque fade frame therefore occludes lower layers. Preview and export share this
same timeline-to-compositor path.

## Audio policy

Every audio-bearing media clip with a transition-in gets the same linear gain ramp, regardless of
transition type. The ramp boundaries are derived with integer frame-to-sample conversion from the
clip start and `clip start + transition duration`. Gain is the only floating-point step. The first
sample is silent, the last sample in a multi-frame ramp is full gain, and later samples stay at
full gain. One-frame transitions remain no-ops.

Playback sources retain their absolute project-sample cursor and channel position between feeder
chunks. Export evaluates the identical shared gain function from its absolute destination sample
index, which keeps preview/playback and export phase-continuous and sample-identical.

## Descriptor table and consumers

`openreel-core` owns `TRANSITION_DESCRIPTORS`. Each entry contains the stable operation/serde name,
a one-line description, and compositor shading metadata. The table is the source of truth for:

- core operation and document validation;
- media timeline shading selection;
- the inspector add/type menus;
- generated agent operation documentation.

The serialized model remains `Transition { name, duration }`. Inspector changes replace the
transition on the selected clip and every linked member in one atomic operation batch, so linked
audio receives the same ramp. Timeline clips show a restrained left-edge wedge whose width follows
the transition duration at the current zoom and stays inside the trim handles.

