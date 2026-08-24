# M42 source/program patching and track targeting

Status: completed 2026-08-24.

M42 is the long-form editorial slice after CC1. It solves one concrete editor
job: inspect a source independently of the timeline, mark an exact range, see
where its video and audio will go, and commit one revision-safe insert or
overwrite without relying on a hidden "first compatible track" choice.

This is a technical workflow. Acceptance is objective; it does not require a
taste review.

## Product contract

### Source and program viewers

The Program viewer remains the live project output at the timeline playhead.
The Source viewer has its own selected asset, source-frame cursor, In, and Out.
Moving or marking Source does not seek Program, and moving Program does not
change Source.

M42's Source viewer is an independently addressable, frame-accurate still/scrub
viewer backed by the existing verified-source thumbnail path. It is not a claim
of simultaneous real-time source playback or source audio monitoring. Those
need a second playback/decode channel and remain deferred.

Source cursor, marks, and route selections are workspace/session state, not
hidden serialized `Document` state. Selecting another asset establishes a
valid cursor and full-source marks. Source imagery and new source/program edits
require a current **online verified** observation. Checking, legacy-unverified,
offline, changed, or unreadable states fail closed and cannot leave a stale
Source frame visible. Program retains M41's existing playback policy; this
stricter gate is scoped to the new Source workflow.

A verified Source display observation expires after five minutes. Expiry hides
the frame and schedules one asynchronous full-source identity check; it does not
turn an open viewer into a continuous large-file hashing loop. Insert and
Overwrite are stricter: every click forces, or joins, a current full verification
and dispatches only if the exact response is online verified and the captured
session, source identity, selection, cursor, marks, Program position, routes, and
timeline revision are all still current.

### Explicit patching and targeting

Every source/program edit visibly names its destinations:

- a video route is either off or one explicit video track;
- an audio route is either off or one explicit audio track;
- at least one route is required;
- a stale, missing, duplicate, or kind-incompatible destination fails closed;
- the initial compatible route may be chosen deterministically, but is shown to
  the editor and never applied as a hidden fallback.

The existing single-track `ThreePointEdit` wire contract and behavior remain
unchanged. M42 adds one additive compound operation for patched edits. It uses
the same exactly-three-of-four source In, source Out, record In, and record Out
semantics, derives the fourth boundary once, and validates the complete request
before mutation.

For a dual video/audio Insert, Core opens time once and places both components
in the same derived range. It must not compose two independent inserts, which
would ripple sync-locked material twice. A dual route creates one linked V/A
pair. Overwrite clears only the selected destination tracks. Single video or
audio routes affect only that component. The compound edit is one operation,
one revision, one journal entry, and one undo step.

## Human workflow

1. Selecting a media asset cues Source without changing Program.
2. The editor scrubs or steps Source, then marks In and Out explicitly.
3. Visible V and A patch selectors show the exact destination track ids and
   allow either component to be disabled when the source supports it.
4. Insert or Overwrite captures the selected source range, record anchor, mode,
   destinations, and exact observed timeline revision, then waits for the
   mandatory source verification before dispatch.
5. If another human or agent changes the project first, the edit conflicts
   safely. The app refreshes the document and asks the editor to act on current
   state rather than silently retargeting.

## Agent workflow

- A typed source/program planner accepts `expected_revision`, asset id, exactly
  three marks, mode, and explicit optional video/audio destinations.
- Preparation requires live online-verified source availability and validates
  the revision, compatibility, route kinds, ranges, and compound edit without
  mutating the document.
- The preview returns the resolved source and timeline ranges, selected routes,
  mode, and an opaque prepared plan id. Commit uses the ordinary prepared-plan
  revision guard.
- Source evidence inspectors return the timeline revision from the same
  snapshot as their evidence so a model can detect stale marks or routing
  context.
- Existing single-track `three_point_edit` remains available and compatible.

## Exit gates

- Existing `ThreePointEdit` JSON fixtures and semantics remain unchanged; the
  new compound operation round-trips through project journals and MCP schema.
- All four valid three-point mark combinations derive the same ranges as the
  existing primitive.
- Empty, missing, duplicate, stale, or wrong-kind routes reject atomically with
  a byte-for-byte unchanged document.
- Dual A/V Insert ripples the participating timeline exactly once, produces
  aligned linked clips, and increments the revision once.
- Dual A/V Overwrite changes only the selected destination tracks. Video-only
  and audio-only routes change only the enabled component.
- One undo restores the exact prior document; redo and recovery replay produce
  the same routed result.
- Source and Program positions are independent. Asset changes and document
  revisions clamp or reset invalid source/routing state deterministically.
- Checking, online-unverified, offline, changed, and unreadable sources never
  display a stale Source frame and cannot be edited through the human or agent
  workflow. Prepared plans revalidate availability at commit without persisting
  runtime status into the project.
- A stale human or agent request conflicts before mutation. Agent source
  evidence and prepared plans expose the revision they were derived from.
- Workspace format, tests, and strict Clippy pass on Linux. Windows and Linux
  CI pass, and the Linux desktop smoke uses the supported native FFmpeg/WGPU
  path.

## Deferred work

Independent real-time Source playback and audio monitoring, gang/split viewer
playback, persisted editor workspaces, multi-channel audio patch matrices,
record-side In/Out UI beyond the current playhead anchor, source clip bins,
keyboard mapping polish, compound/nested timelines, and dedicated long-sequence
navigation are separate slices.
