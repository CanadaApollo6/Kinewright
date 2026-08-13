# M32 - Editorial credibility

M32 makes professional editorial intent a typed part of OpenReel's Rust domain
model. Agents no longer need to reconstruct common NLE edits from brittle
sequences of trims, moves, and speed changes.

## Boundary decisions

- The `Document` remains the branchable, serializable source of truth.
- Source-monitor marks are request state, not hidden mutable project state.
  `get_source_info` accepts an asset and optional exact source-frame In/Out.
- Three-point editing is one atomic `Operation`. Exactly three of source In,
  source Out, record In, and record Out are supplied; the fourth is derived
  with integer frame-rate mapping.
- Slip, roll, slide, replace, and fit-to-fill are one operation and one undo
  entry each. Core rejects missing handles, non-adjacent clips, mismatched
  durations, and unrepresentable boundaries without partial mutation.
- Fit-to-fill searches the supported integer speed domain (10%-1000%) and
  accepts only a speed that produces the exact existing project-frame slot.
- Bins, string-outs, and sync groups are project data. They participate in
  branches, comparison, merge, save/reopen, and undo/redo like timeline edits.
- Sync groups store named source angles and exact offsets relative to group
  zero. They are the stable foundation for later speaker-aware multicam, not a
  claim that automatic angle switching exists in M32.
- Transcript words now carry an optional diarization label. The current local
  Whisper path leaves it empty; speaker-aware analyzers can populate the same
  contract later without changing search or edit semantics.

## Agent surface

New edit operations:

- `three_point_edit`
- `slip_clip`
- `roll_edit`
- `slide_clip`
- `replace_clip`
- `fit_to_fill`

New media-graph operations:

- `upsert_bin`, `remove_bin`, `set_asset_bin`
- `upsert_string_out`, `remove_string_out`
- `upsert_sync_group`, `remove_sync_group`

New inspectors:

- `get_source_info` returns technical metadata, exact source marks, cached
  transcript words and speaker labels, scene changes, beats, and job status.
- `search_media` filters names, paths, transcripts, speakers, media kind,
  resolution, duration, scene density, beat density, and transcript readiness.
  Word hits include exact source ranges that can feed a three-point edit.

The system prompt explicitly tells every supported harness to use these typed
operations instead of approximating their behavior with low-level recipes.

## Human surface

The Media rail shows catalog counts and asset-bin membership. Selecting an
asset exposes exact source In/Out controls plus Insert-at-playhead and Overwrite
actions. These actions call the same `ThreePointEdit` operation available to
agents; there is no separate UI-only edit path.

## Acceptance contract

M32 is complete when:

1. Every new operation round-trips through JSON and MCP schema generation.
2. Slip preserves timeline placement and source span.
3. Roll and slide preserve the sequence's outer duration.
4. Replace preserves the slot and rejects duration mismatch.
5. Fit-to-fill produces an exact integer-speed duration or rejects atomically.
6. Three-point insert derives its missing boundary, splits target footage at a
   record point when needed, and honors sync-lock ripple behavior.
7. Three-point overwrite replaces only the record range.
8. Invalid handles and catalog references leave the document byte-for-byte
   unchanged.
9. Catalog changes survive save/reopen and restore with one undo.
10. Source inspection and media search return structured, stable IDs and exact
    source ranges, including optional speaker labels.
11. The whole workspace test suite and strict Clippy pass with the local FFmpeg
    build environment active.

## Deliberate limits

- The first human source monitor uses exact numeric source marks and media
  thumbnails. A dedicated dual-monitor playback dock can build on the same
  contract without changing core operations.
- Current source patching chooses the first compatible track. Explicit V/A
  source patch routing is future UI work; agents can already target any track.
- Sync groups do not yet create a live multicam sequence or choose angles.
- Speaker labels are supported and searchable, but M32 does not add a local
  diarization model.
