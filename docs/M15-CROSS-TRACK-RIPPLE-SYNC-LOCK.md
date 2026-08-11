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

## Marker exclusion and future work

Project markers do not shift during either ripple operation in M15. Markers are
project-level editorial notes rather than track members, and M15 does not infer
whether a marker follows content, absolute program time, or a particular lane.

Future work is filed as **marker-ripple semantics**: define explicit marker
affinity or a project-level ripple policy before any ripple operation moves a
marker. Until that model exists, markers remain fixed by design.
