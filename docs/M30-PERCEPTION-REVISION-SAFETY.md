# M30 - Perception and Revision Safety

M30 makes every supported agent harness a safer and more capable editor. It is
the foundation for M31 branches: an agent cannot branch, compare, or merge work
reliably until it can name the exact timeline state it observed.

## Requirements

### Functional

- Every authoritative timeline snapshot has a monotonic revision.
- Agent mutations carry the revision they were planned against and fail
  atomically when it is stale.
- Models can inspect a bounded storyboard instead of guessing individual frame
  positions one at a time.
- Storyboards are rendered from the same timeline compositor as preview/export
  and can serve as low-resolution visual proof after an edit.
- Audio assets have cached, deterministic beat/onset analysis mapped into both
  source and project frames.
- Analysis work has a common machine-readable lifecycle with progress when
  known, explicit errors, and cancellation.
- The public eval artifact has a versioned manifest and machine-readable
  baseline suitable for publishing.

### Non-functional

- No floating-point time enters the project model; frame positions stay exact.
- Derived media results remain content-addressed and outside the saved project.
- A stale plan never partially mutates the timeline and never enters undo.
- Storyboard requests are bounded in frame count, pixel size, and encoded bytes.
- Beat detection is local, reproducible, dependency-free beyond the existing
  FFmpeg decode path, and useful without claiming music-theory certainty.
- Existing project files remain backward compatible.

## Data flow

```text
agent harness
    |
    | get_timeline_state / get_timeline_storyboard / get_*_analysis
    v
Kinewright MCP  ---- exact snapshot revision ----> Core actor
    |                                            |
    |                                            | DoBatchIfRevision
    |                                            v
    |                                      atomic accept/conflict
    |
    +---- storyboard/proof ----> Analysis ----> shared compositor
    |
    +---- beat/status/cancel --> derived-analysis worker + content cache
```

MCP owns presentation and bounded image assembly. Core owns revisions and
conflict decisions. Media owns derived analysis and rendering. This preserves
the existing crate boundaries.

## Core contract

`TimelineRevision` is an opaque monotonic integer owned by one `Core` actor. The
initial validated document is revision 0. Each accepted state change increments
it once, including an atomic batch, undo, or redo. Rejected operations and
no-op undo/redo commands do not increment it.

`Query::Snapshot` returns `{ revision, document }` atomically. Existing document
queries remain available to avoid forcing unrelated consumers through a new
shape.

Agent mutations use `DoIfRevision` and `DoBatchIfRevision`. A mismatch produces
`RevisionConflict { expected, actual }`; it does not validate or apply the
operation, ask for destructive confirmation, touch history, or update playback.
The agent must inspect again and re-plan.

The revision is actor-local runtime state, not serialized into `.kinewright`.
Saving a project describes media and editorial state; reopening it intentionally
starts a new revision lineage at 0.

## Storyboard and proof contract

`get_timeline_storyboard` accepts an optional half-open timeline range and a
bounded frame count. Sampling is deterministic and includes both ends of the
visible range. The result contains:

- the source timeline revision;
- a compact frame manifest mapping cells to exact project frames;
- one PNG contact sheet assembled from compositor thumbnails.

The tool is both a survey and a proof primitive. Models use it before an edit to
understand a sequence and after an edit to support claims about the result.
M31 can persist the same manifest alongside branch provenance.

MCP does not emit a temporary video file in M30. Current model harnesses consume
images reliably but do not share one portable video-content contract. A contact
sheet is deterministic, cheap, inspectable, and maps every observation back to
exact frames. Revisit short proxy-video resources when ACP and MCP clients expose
a consistent video input surface.

## Beat analysis contract

Beat sense stores transient `AssetBeats` beside the existing transcript,
silence, and scene caches. It contains content hash, source time base, estimated
tempo, and onset markers with integer strength basis points.

The first detector is intentionally conservative: local energy novelty with a
refractory interval, followed by a robust median interval tempo estimate. It is
designed to find useful cut anchors in rhythmic music, not label bars, meter,
downbeats, harmony, or genre. Later detectors can replace it by bumping the
derived-cache version without changing the agent contract.

`get_beats` returns source markers. `get_timeline_beats` maps audible, real-time
media clips into project frames and states when speed-changed clips were omitted.

## Analysis job lifecycle

The shared shape is:

```text
not_requested -> queued -> hashing/downloading/analyzing -> ready
                                              \-> unavailable
                                              \-> failed(error)
                                              \-> cancelled
```

Each status names the analysis kind and asset, exposes progress from 0 through
100 when the backend can calculate it, and carries an error only for failure.
Cancellation is cooperative and idempotent. Decode loops and Whisper inference
receive the same cancellation token; cached results that completed before a
cancel remain valid.

## Published benchmark v1

The existing generated-media eval becomes a publishable artifact through:

- a versioned suite manifest describing tasks, fixture provenance, assertions,
  and budgets;
- JSONL run traces already produced by `kinewright-eval`;
- a checked-in baseline JSON containing harness/model, pass rate, latency, token
  usage, and failure categories;
- a documented reproduction command that never depends on private footage.

Human first-pass acceptance is added as an optional reported field rather than
fabricated from structural correctness.

## Acceptance gates

1. A stale single operation and stale batch are rejected without changing the
   document, revision, operation log, or undo history.
2. One accepted batch increments the revision once; one undo increments it once
   and restores the exact document.
3. A storyboard fixture returns the requested bounded cell count, exact frame
   manifest, valid PNG, and the revision it rendered.
4. A generated 120 BPM click track produces stable beat markers and an estimate
   within the documented tolerance across supported sample rates.
5. Status and cancellation are observable through the MCP contract and do not
   deadlock the media workers.
6. Existing agent and app suites remain green; a live subscription smoke test
   proves models use the revision precondition correctly.

## M31 dependency

M31 branches will store a base `TimelineRevision`, base document, validated
operation sequence, working document, and provenance. Merge is a revisioned
atomic batch. A conflict remains a branch for comparison or rebase instead of
silently overwriting the live timeline.

What to revisit as the system grows: persistent branch storage, proxy-video MCP
resources, learned beat/downbeat models, analysis worker parallelism, and
cross-project content-index sharing.
