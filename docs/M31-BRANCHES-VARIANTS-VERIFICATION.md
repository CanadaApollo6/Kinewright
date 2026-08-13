# M31 branches, variants, and verification

## Outcome

Every agent thread edits an isolated timeline branch. The live project changes
only when a person merges or cherry-picks validated branch operations. The
branch retains enough evidence to answer what the model saw, what it proposed,
what rendered proof it requested, what QA found, and what the person approved.

M31 also makes the two visible Pillar B outputs declarative: caption presets are
ordinary title data rendered by the shared preview/export path, and delivery
variants describe a target aspect plus a reviewable reframe plan.

## Requirements and assumptions

- A branch starts from an exact live `TimelineRevision` and document snapshot.
- One thread owns one branch Core actor, MCP endpoint, confirmation broker, and
  agent process. No branch shares mutable timeline history with another.
- A branch merge is the branch's currently applied operation sequence submitted
  as one `DoBatchIfRevision` to the live Core. Undo therefore restores the live
  project in one step.
- Undo and redo inside a branch change the applied sequence. An append-only log
  is not sufficient for merging; Core must expose the operations represented by
  its current undo stack.
- A stale base never overwrites live work. It stays reviewable and may be
  discarded or cherry-picked against a newly inspected live revision.
- Cherry-pick uses stable one-based operation indices. Dependencies are not
  guessed: the selected subset is atomically validated and rejected if it is
  incomplete.
- Branch proof frames render an explicit branch document through the compositor.
  They do not replace or pause the live playback document.
- Caption presets use stable tokens and existing title fields. Preview and
  export already share `FrameRenderer`, so parity is a construction property
  backed by a pixel test.
- M31 auto-reframe is deterministic and reviewable, not a claim of learned
  subject tracking. It creates centered cover-crop delivery variants with a
  normalized focal point that agents or people can adjust. Learned tracking is
  an M34 backend behind the same contract.

## Branch data flow

```text
live Core (revision N, document D)
        |
        | snapshot
        v
thread branch Core (base N, initial D) <-- branch-local MCP <-- agent harness
        |                                      |
        | applied operations                   | prompts, inspections, proofs
        v                                      v
branch comparison + QA + provenance record
        |
        +-- discard: replace branch with a new live snapshot
        +-- cherry-pick: selected ops -> live DoBatchIfRevision(current N)
        +-- merge: all applied ops -> live DoBatchIfRevision(base N)
```

## Core branch contract

`Query::AppliedOperations` returns the ordered concatenation of operations in
the current undo stack. It differs deliberately from `OpLog`:

- accepted edit: appears in both;
- branch undo: remains in `OpLog`, disappears from `AppliedOperations`;
- branch redo: remains in `OpLog`, returns to `AppliedOperations`;
- rejected work: appears in neither.

`TimelineBranch` owns the base revision/document and branch Core. `comparison`
reports its base, head document, branch revision, and applied operation sequence;
the app derives the affected range and QA from those snapshots.
`merge` is all-or-nothing and returns `Merged`, `NoChanges`, `Conflict`, or
`Rejected`. `cherry_pick` returns the same live-Core outcomes plus invalid-index
errors.

## Provenance

Each branch records an ordered, serializable event ledger:

- prompt text;
- MCP inspection/tool call and arguments;
- MCP result summary;
- proof request (`get_frame_at` or `get_timeline_storyboard`);
- destructive approval or rejection;
- final applied operations and branch revision;
- QA report at review time;
- merge, cherry-pick, or discard decision.

The operation sequence remains authoritative. Provenance explains the decision;
it is not another mutation source.

## QA contract

QA is deterministic, fast, and side-effect free. It reports severity, stable
code, human message, and optional frame range. M31 checks:

- empty timelines;
- timeline gaps and abrupt untransitioned cuts;
- missing source files;
- non-real-time clips whose audio is muted by policy;
- captions longer than the preset line/character limits;
- captions too brief to read comfortably;
- timelines with no populated audio track.

These are review signals, not automatic blockers, except invalid document or
delivery settings which already fail their authoritative validators.

## Styled captions

`CaptionPreset` is a stable enum with three initial presets:

- `clean`: small primary lower-third text with a scrim;
- `social`: display-size accent text centered with a scrim;
- `minimal`: small primary lower-third text without a scrim.

The preset resolves completely into existing declarative `Title` fields. The
caption operation builder accepts a preset and remains one atomic batch. The
MCP tool exposes preset discovery and plan generation so models do not hand-code
dozens of title operations.

## Delivery variants and reframe

`DeliveryVariant` contains a target aspect and an explicit focal point in
integer percentages. The centered default is `(50, 50)`. A registered `reframe`
effect remaps source UVs with a deterministic cover crop in the shared GPU
compositor, preserving source aspect without stretching. The effect is applied
to media/freeze clips on a cloned document, leaving titles and the master
timeline unchanged.

The initial built-ins are 16:9 at 1920x1080, 9:16 at 1080x1920, and 1:1 at
1080x1080. Export receives the derived variant document plus target resolution.

## Acceptance gates

1. Two branches from one base can diverge without changing the live document.
2. A merge applies all currently applied branch operations as one live undo
   entry and advances the live revision once.
3. A stale merge changes neither live document nor live history.
4. Branch undo removes work from the merge sequence; redo restores it.
5. A valid cherry-pick applies only selected indices; a dependent invalid
   subset rejects atomically.
6. Branch proof rendering includes compositor effects without changing live
   playback state.
7. Provenance links prompt, evidence calls, proof, operations, QA, and decision.
8. Caption preset titles match preview and export pixels through the shared
   renderer.
9. 16:9, 9:16, and 1:1 plans produce even dimensions and deterministic cover
   transforms without stretching source aspect.
10. Workspace tests, strict clippy, and a real installed-agent branch smoke test
    pass before merge.

## Tradeoffs and future work

Branch state is process-local in M31. Project persistence and branch rebase are
deferred until the semantics have real use. Reframe focal points are explicit
and deterministic; face/person tracking, motion smoothing, shot-specific focal
curves, and caption-aware subject avoidance remain M34 execution backends. QA
starts structural and expands toward loudness, flash-frame, gamut, and delivery
conformance without changing its report shape.

## Verification record

Verified on Windows on 2026-08-12:

- `cargo test --workspace` passed, including branch divergence, atomic merge and
  undo, isolated proof rendering, QA, caption presets, GPU focal-point reframe,
  and existing preview/export title parity.
- `cargo clippy --workspace --all-targets -- -D warnings` passed.
- A real installed Codex CLI session inspected an isolated branch, submitted a
  compact two-operation edit plan, verified the result, and ran branch QA. The
  live document remained exactly equal to its base until merge; merge advanced
  live from revision 0 to 1; one live undo restored the exact base document.
- The smoke exposed and then closed an agent-usability gap: `apply_edit_plan`
  now accepts compact `{"op":"split_clip", ...}` operations as well as the
  generated Rust enum envelope. The repeated live run applied the compact form
  on its first attempt.
