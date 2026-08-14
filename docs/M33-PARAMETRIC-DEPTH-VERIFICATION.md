# M33 - Parametric depth

M33 gives agents durable automation, compositing, color, and audio-mix
primitives instead of asking them to synthesize those capabilities through
FFmpeg commands. The Rust document remains the source of truth, and the same
evaluated graph drives preview, proof frames, scopes, playback audio, and
export.

## Boundary decisions

- Automation curves use exact integer frames and fixed-point integer values.
  Clip-effect curves are clip-local; audio-bus curves use project frames.
- `hold`, `linear`, `ease_in`, `ease_out`, and `ease_in_out` interpolation are
  evaluated with integer arithmetic. Preview and export cannot disagree due to
  floating-point curve drift.
- Visual effects remain an ordered, serializable effect stack. The shared GPU
  compositor evaluates color, built-in looks, external LUTs, masks, and chroma
  keys. There is no renderer-only hidden state.
- Mask tracking is deterministic sequential template matching on isolated
  compositor frames. It returns confidence observations and revision-gated
  keyframe operations; inspection never silently edits the timeline.
- External LUTs are 3D `.cube` files with red-fastest sample ordering,
  `DOMAIN_MIN`/`DOMAIN_MAX`, and explicit trilinear sampling. Parsed files are
  cached by canonical path, modified time, and length.
- Audio buses route each track to at most one bus. Unrouted tracks feed the
  master directly. Ducking reads declared pre-bus sidechains.
- One stateful `AudioMixProcessor` implements bus gain, fixed three-band EQ,
  compression, automation, and ducking for both playback and export. Seeking
  prerolls the graph from frame zero so stateful envelopes remain exact.

## Agent surface

New operations:

- `set_effect_keyframes` and `clear_effect_keyframes`
- `upsert_audio_bus` and `remove_audio_bus`

New visual effects:

- `color_grade`: exposure, temperature, and tint
- `look_lut`: four stable built-in looks plus intensity
- `cube_lut`: a text `path` to a 3D `.cube` file plus automatable intensity
- `mask`: rectangle or ellipse, center, size, feather, and inversion
- `chroma_key`: RGB key color, threshold, softness, and spill suppression

New bus effects:

- `audio_gain`
- `audio_eq`
- `audio_compressor`
- `audio_ducking`

New inspectors:

- `get_video_scopes` measures post-compositor RGB/luma histograms, means,
  clipping, and a 64-column luma waveform.
- `track_mask_region` follows an existing bounded mask, validates the editable
  center-X and center-Y curves, and returns an opaque `prepared_edit_plan`
  handle plus preview for direct commit.

`get_timeline_state` and `get_clip_info` render static effect values and their
automation curves. Audio-bus routes, processors, sidechains, and automation are
also present in the compact agent state.

## Human surface

The existing effect inspector exposes the new static visual controls while
excluding audio-only effects and file-backed LUTs that need a dedicated file
picker. Automation and audio-bus edits are currently agent-first typed
operations; their document state remains visible in the timeline inspector and
survives save, branch, merge, undo, and redo.

## Acceptance contract

M33 is complete when:

1. Curves reject empty, unordered, duplicate, negative, out-of-clip, and
   out-of-range keyframes without partial mutation.
2. Every interpolation mode resolves to the same integer value at an exact
   frame after save/reopen and undo/redo.
3. Timeline rendering evaluates visual curves in clip-local time before the
   shared compositor.
4. Color, built-in looks, masks, and chroma keys produce asserted GPU pixels.
5. A real 3D `.cube` file produces asserted GPU pixels in standard red-fastest
   ordering, while malformed and unsupported LUTs fail clearly.
6. Scopes measure real post-effect pixels and ignore fully transparent pixels.
7. Tracking follows a translated fixture exactly and a real MCP call returns
   revision-gated keyframe operations without changing the document.
8. Audio routing rejects duplicate track ownership, missing tracks, visual
   effects, invalid sidechains, and invalid project-frame automation.
9. EQ, dynamics, automation, and sidechain ducking change actual samples.
10. Streaming playback and export remain sample-for-sample equal through the
    same non-trivial bus chain, including a seek.
11. The full workspace test suite and strict Clippy pass with the pinned local
    FFmpeg environment active.

## Deliberate limits

- The compositor stack is not yet an arbitrary node DAG with mattes shared
  across clips. Multiple ordered effects are the stable M33 graph boundary.
- Tracking is local template matching, not learned subject segmentation,
  optical flow, occlusion recovery, or camera solving. Confidence is surfaced
  so agents can decline weak results.
- Scopes do not yet include a vectorscope or RGB parade image.
- Only 3D `.cube` LUTs sized 2 through 64 are accepted. 1D shaper LUTs and
  tetrahedral interpolation are future work.
- Audio buses are flat. There are no nested sends, aux returns, plug-in hosting,
  or loudness normalization in M33.
- The EQ uses deterministic 200 Hz and 4 kHz one-pole crossovers. It is useful
  shaping, not a replacement for a full parametric studio EQ.
- Exact stateful seek preroll can be expensive late in a long project. A future
  processor-checkpoint cache can preserve the contract while reducing seek
  latency.

