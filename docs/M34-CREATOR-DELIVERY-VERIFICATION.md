# M34 - Creator leverage and delivery

M34 turns M30-M33 perception and automation primitives into creator-facing
plans and a safe path from an agent branch to a delivery file. The Rust
document remains the source of truth. Planning tools return ordinary,
revision-gated edit operations, and queued exports render immutable document
snapshots through the same compositor and audio graph used by preview.

## Boundary decisions

- Caption motion is ordinary `opacity` or `transform` automation on title
  clips. `none`, `fade`, `pop`, and `slide_up` therefore remain editable,
  serializable, undoable, and preview/export-identical.
- Beat pacing is a plan, not an automatic mutation. It accepts fully analyzed
  timeline beats from any track, filters and spaces them deterministically,
  exposes the selected points in ascending order, and emits safe newest-first
  split operations.
- Music fitting is one exact-duration three-point edit anchored to an eligible
  analyzed source beat. The result explicitly reports straight real-time
  playback, no repeat, no time stretch, and no claimed source-end alignment.
- Speaker multicam requires actual diarization labels, an explicit
  speaker-to-angle map, and an existing sync group. Missing or ambiguous data
  is an error instead of a guessed cut.
- Subject reframe uses deterministic sequential template tracking from a
  caller-supplied subject box. Confidence observations remain visible, and the
  resulting focus curves are ordinary editable effect keyframes. It is not
  presented as face detection, person detection, or learned segmentation.
- Delivery variants are non-destructive materialized documents. Repeated
  materialization replaces the prior `reframe` effect instead of stacking
  crops.

## Agent surface

New planning and inspection tools:

- `plan_beat_pacing`
- `plan_music_fit`
- `plan_speaker_multicam`
- `track_reframe_subject`
- `get_delivery_profiles`
- `get_delivery_conformance`

New mutation and delivery tools:

- `add_styled_captions` now accepts `none`, `fade`, `pop`, or `slide_up`
  motion.
- `queue_export` captures one immutable, revision-gated branch snapshot.
- `get_export_jobs` returns retained machine-readable status and progress.
- `cancel_export` cooperatively cancels queued or running work.

Every deterministic creator plan is validated server-side and returns an
opaque `prepared_edit_plan` handle plus a compact preview. Agents inspect the
evidence and preview, then commit the handle at the same revision without
copying a large operation array through another tool call.

## Delivery contracts

The stable profiles are `source_master`, `youtube_1080p`, `vertical_short`,
and `square_social`. Each resolves to an MP4 container with H.264 video and AAC
audio plus an exact raster and bitrate. The YouTube profile follows the
official 1080p SDR guidance: 1920x1080, 8 Mbps through 30 fps or 12 Mbps above
30 fps, and 384 kbps stereo audio. The vertical profile uses a full-screen 9:16
1080x1920 composition; `vertical_short` deliberately remains platform-neutral
rather than claiming one service's full upload-policy contract.

References:

- [YouTube recommended upload encoding settings](https://support.google.com/youtube/answer/1722171?hl=en)
- [YouTube resolution and aspect-ratio guidance](https://support.google.com/youtube/answer/6375112)
- [TikTok full-screen 9:16 creative guidance](https://ads.tiktok.com/business/library/Top_Tips_One_Pager_SMB.pdf)

Conformance materializes the exact delivery document, checks structural QA,
and exposes the container, raster, codecs, bitrates, and every issue. The queue
also rejects a filename whose extension does not match the profile container.

## Export safety

- One worker serializes exports; the pending queue is bounded at 64 jobs by
  default.
- Each job owns an immutable document snapshot, profile, destination, focal
  point, overwrite intent, cancellation token, progress, conformance report,
  state, and terminal error.
- New destinations do not require confirmation. Every `overwrite=true`
  request still enters the confirmation broker before enqueue.
- Output paths are normalized through their real parent directory. Active jobs
  reserve destinations case-insensitively on Windows.
- The server refuses any destination resolving to a project source asset.
- Directories and symbolic-link outputs are rejected. A destination created
  after enqueue is checked again immediately before the exporter runs.
- Backend errors and panics fail the job without killing the worker. Queued and
  running cancellation are both covered.

## Acceptance contract

M34 is complete when:

1. Every caption preset accepts every motion and short captions remain visible.
2. Caption animation survives as editable automation curves.
3. Beat pacing rejects pending or incomplete analysis and produces a
   deterministic, fully valid split plan.
4. Music fitting produces the exact requested duration without overstating
   loop, stretch, or end-beat behavior.
5. Multicam refuses missing diarization, unmapped speakers, invalid angles, and
   invalid sync coverage.
6. Reframe tracking returns confidence-gated focus curves without mutating the
   document.
7. Every delivery profile materializes the same raster and settings its
   conformance report describes.
8. The queue preserves snapshot immutability, serial execution, bounded
   capacity, destination ownership, cancellation, failure recovery, and the
   enqueue-to-worker overwrite race check.
9. The app gives each isolated agent branch the real export backend while
   retaining the human confirmation boundary.
10. The MCP registry and implemented handler set are exactly equal, the full
    workspace suite passes, formatting is clean, and strict Clippy is clean.

## Deliberate limits

- Beat pacing currently creates cuts; it does not score narrative meaning or
  synthesize retiming ramps.
- Music fitting does not loop, time-stretch, remix stems, or align both ends to
  beats.
- Speaker multicam depends on upstream diarization quality and explicit sync
  metadata. It does not infer camera identity from pixels.
- Template reframe can lose a subject through occlusion, large scale changes,
  or hard cuts. Low confidence is surfaced for review instead of hidden.
- Delivery profiles do not yet include loudness normalization, HDR metadata,
  caption sidecars, platform upload APIs, or hardware-encoder selection.
- Export jobs are process-local and retained in memory. A durable queue that
  survives application restart is future work.

## Verification record

The release gate is:

```text
cargo fmt --all -- --check
cargo test --workspace --no-fail-fast
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

Focused tests additionally cover all creator planners, caption motions,
delivery profiles, MCP tool-registry equality, source-path alias protection,
and the export queue's concurrency and filesystem races.
