# M13 editing ergonomics

M13 adds per-track ripple edits, A/V clip links, project markers, and expanded
timeline snapping without changing the core architecture: integer project
frames, pure operations, snapshot undo, atomic `DoBatch`, durable journal
replay, and no I/O in core.

## Ripple design

`RippleDeleteClip { clip }` removes one clip and shifts every later clip on the
same track left by the removed clip's exact project-frame duration.
`RippleInsertGap { track, at, duration }` shifts clips whose start is at or
after `at` right by a positive project-frame duration.

Ripple is deliberately **per-track in M13**. Cross-track ripple preserves sync
in many NLE workflows, but it also makes an edit on one lane mutate unrelated
lanes unless sync intent is explicit. Per-track ripple is predictable. A
future sync-lock model can opt tracks into cross-track ripple without changing
the M13 primitives.

M15 implements that filed follow-up. Its exact cross-track boundary and marker
decisions are recorded in `M15-CROSS-TRACK-RIPPLE-SYNC-LOCK.md`.

Both operations use normal core validation and snapshot undo. Ripple delete is
classified as destructive by the MCP schema and confirmation broker;
insert-gap is reversible and non-destructive.

## A/V link model and enforcement

Each clip has an optional `link` id. `LinkClips { clips }` validates a unique
selection of at least two existing clips and assigns a fresh link id.
`UnlinkClips { clips }` validates one or more unique existing clips and clears
only those clips' link metadata. Same-asset and overlap checks are intentionally
not link requirements.

Core apply remains per-operation and does **not** make `MoveClip`, `TrimClip`,
or delete operations cascade. Link-follow enforcement belongs to the callers:

- Timeline move, trim, normal delete, and ripple delete expand the selected
  link group into one atomic `DoBatch`.
- The agent system prompt requires the same expansion in one edit plan.
- A link id with one remaining member is valid metadata; core does not add
  hidden cascading behavior when another member is edited directly.

Adding an `AudioVideo` asset to the timeline now creates matching video and
audio clips and links them. The first existing video and audio tracks are used.
If no audio track exists, `AddTrack` creates one in the same batch before both
clips are placed.

## Markers

Markers live on `Document` and contain an id, project-frame position, label,
and stable design-token color index. `AddMarker`, `RemoveMarker`, and
`MoveMarker` are pure, validated, non-destructive operations. The confirmation
broker does not prompt for them because they are an editorial suggestion
surface, not media removal.

The timeline ruler paints compact flags. `M` adds a marker at the playhead;
click-drag moves it; right-click or Delete removes the selected marker. Agent
guidance prefers markers over edits when the user asks for footage review.

## Snapping and renderings

Move, trim, marker, and playhead drags snap within an 8-screen-point tolerance.
Targets are visible ruler ticks, the playhead, markers, and clip edges from all
tracks. Alt bypasses snapping during a drag. The existing accent guide and
diamond remain the only snap indicator.

`get_timeline_state` reports marker and link-group counts, compact link member
lists, and marker position, token index, and label lines.

## Project-format compatibility

This is an additive project-format change and does not require a version bump.
`Clip.link` defaults to `None` when absent and is omitted when empty;
`Document.markers` defaults to an empty list and is omitted when empty. The
pre-M13 fixture in `crates/openreel-core/tests/fixtures` proves that an old
project JSON document loads, validates, and reserializes without either field.
