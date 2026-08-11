# M15 cross-track ripple sync lock

M15 resolves the cross-track ripple future-work item filed by M13. It adds an
explicit sync-lock flag to each track while retaining integer project frames,
pure operations, snapshot undo, atomic batch application, and journal replay.

## Track model and compatibility

`Track.sync_lock` is `true` by default. A track participates in ripple edits
started on another track unless the user explicitly unlocks it. The field is
omitted when it has the default value, so pre-M15 project files load as locked
and reserialize without project-format noise. An unlocked track serializes
`"sync_lock": false`. This is an additive project-format change and does not
require a format-version bump.

`SetTrackSyncLock { track, locked }` is a pure, non-destructive operation. A
missing track rejects the operation without changing the document. The normal
operation spine provides snapshot undo and durable journal replay.

## Ripple boundary

`RippleDeleteClip` defines the ripple point as the deleted clip's pre-edit end
frame. It removes that clip, then subtracts the deleted clip's exact duration in
project frames from clips on the edited track and every other sync-locked track
whose `timeline_start` is greater than or equal to the ripple point. The edited
track always participates, even when its own sync lock is off.

The start comparison is the complete boundary rule. A clip whose start is
before the ripple point is not shifted, shortened, split, or otherwise changed,
even if its range straddles the ripple point. If shifting a later clip left
would overlap that unchanged clip, final document validation rejects the whole
operation and the caller's document remains unchanged.

`RippleInsertGap` is symmetric. It adds the requested duration to clips whose
start is at or after `at` on the edited track and every other sync-locked track.
A clip that starts before `at` remains unchanged, including one that straddles
`at`. The edited track again always participates.

Linked A/V ripple deletion produces exactly one `RippleDeleteClip`: companion
link members use normal `DeleteClip` operations in the same atomic plan. This
prevents one user edit from applying the cross-track shift more than once.

## Marker ripple semantics

The **marker-ripple semantics** future-work item is resolved. Project markers
are timeline annotations, so leaving them behind during a ripple edit would
orphan them from the content they annotate. They therefore participate in every
ripple regardless of track sync locks. Partial marker shifting based on track
participation would desynchronize project-level annotations from content on the
other tracks they may describe.

`RippleDeleteClip` shifts markers at or after the deleted clip's pre-edit end
left by the removed duration. Markers strictly before that boundary stay fixed.
A marker that would land at a negative project position is clamped to frame
zero because marker positions are non-negative. `RippleInsertGap` shifts markers
at or after `at` right by the inserted duration; earlier markers stay fixed.

This is a semantics extension of the existing ripple operations. It adds no
operations or model, serialization, or project-format changes. Snapshot undo
restores marker positions, and journal replay reproduces their shifts through
the existing operation spine.
