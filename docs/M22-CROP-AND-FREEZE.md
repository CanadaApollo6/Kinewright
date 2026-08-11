# M22: Crop and freeze-frame clips

M22 adds a source-texture crop effect and project-local freeze-frame clips. It does not add speed changes; constant-rate speed adjustment remains deferred to a later milestone.

## Crop effect

`crop` has four integer percentage parameters: `left_percent`, `right_percent`, `top_percent`, and `bottom_percent`. Each stored value is validated in `0..=45`, with `0` as the neutral value.

Multiple crop effects fold additively, like brightness and transform offsets. After all effects are folded, each compositor inset is independently clamped to `0.0..=0.45`. This lets duplicate effects accumulate without ever inverting or eliminating the source rectangle.

The compositor uniform is 16 `f32` values (64 bytes). Crop occupies slots 9 through 12 as left, right, top, and bottom, followed by three padding floats. Rust serialization and WGSL declaration order are identical.

Crop selects a sub-rectangle of the source texture by making pixels outside `[left, 1-right] x [top, 1-bottom]` transparent. UV y=0 is the top of the source. The crop alpha decision runs after transition fade alpha handling, so cropped pixels remain transparent during fades.

Transform scales and positions the quad while crop tests the source UV. The result is crop-then-transform, and the controls remain orthogonal by construction.

## Freeze-frame model

`ClipContent::Freeze(FreezeFrame)` stores one `source_frame` in asset source frames. Like a title, its `source_range` is a project-local duration span and bypasses source/project FPS mapping. Unlike a title, its `asset` is meaningful and must resolve to a `Video` or `AudioVideo` asset. The held source frame must satisfy `0 <= source_frame < asset.duration`.

Freeze clips are video-only and silent. They carry the normal effects and incoming transition, can be moved, trimmed, split, rippled, linked, deleted, and undone, and retain the same held source frame across trims and splits. Title parameter edits and clip-audio edits reject them.

At render time a freeze produces the normal video layer with `source_at = source_frame` and `source_end = source_frame + 1`. Preview and export therefore use the same render path. The renderer's frame cache makes later frames of the hold reuse the first decoded frame.

Freeze clips deliberately remain invisible to `source_on_track` and the public media-only `timeline_source_at` lookup. This preserves the split-at-playhead fallback as a moving-media operation and prevents creating a freeze from another freeze. They also do not participate in timeline audio, transcript words, silence spans, scene changes, or captions.

## Insert at playhead

The app's **Freeze** action resolves the media source under the playhead and creates a two-second hold using `nominal_fps * 2` project frames. It submits one atomic batch:

1. `SplitClip` at the playhead when the position is inside the source clip, but not at a clip boundary.
2. `RippleInsertGap` on that video track for the freeze duration.
3. `AddFreezeFrame` at the playhead using the resolved asset and source frame.

The ripple gap shifts the edited track, other sync-locked tracks, and markers according to the existing ripple contract. The whole batch is one undo step.

## App presentation

Timeline freeze clips use the media-style body, selection, hover, border, label, and transition layers. The filmstrip is replaced by repeated tiles of the one thumbnail cached at `source_frame`; no waveform is drawn. A small `HOLD` glyph identifies the clip. The inspector shows the asset, frozen source frame, project duration, and the same Effects and Transition sections used by media clips.
