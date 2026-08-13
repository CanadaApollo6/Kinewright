# M37 - Human-acceptable first cut

M37 prepares a controlled rerun of the exact M35 finished-cut task. It closes
the visible caption overflow found in the first artifact, adds a machine gate
for the defect, and removes repeated runtime text that would otherwise inflate
the model context. The approved subscription-backed run is complete. Its first
human watch-through rejected the artifact and exposed gaps that the machine
score did not detect.

## Acceptance target

The next run must satisfy all of these gates:

1. pass the original M35 editorial, transcript, proof, QA, delivery, and undo
   assertions;
2. pass the new `vertical_short` animated-caption safe-area assertion;
3. retain at least one real-time audio-bearing media clip and independently
   prove that the exported artifact contains both video and audio streams;
4. export and independently probe the exact 1080x1920 timeline snapshot;
5. retain the MP4 SHA-256, proof sheet, final document, JSONL telemetry, and
   human-review template as one immutable artifact set;
6. improve materially on the 731,311-token, 24-call, 122.871-second M35 Codex
   baseline without weakening the brief; and
7. receive a human watch-through and explicit acceptance score. Machine green
   is necessary, not sufficient.

## Shared layout contract

```text
Title project data + delivery raster
                |
                v
 exact Inter measurement and wrapping
                |
       +--------+---------+
       |                  |
       v                  v
preview/export raster   deterministic QA
       |                  |
       +--------+---------+
                v
      delivery conformance + eval assertion
```

`openreel-core::title_layout` is now the single composition contract. It:

- scales font tokens from the output short edge, so 1080x1920 no longer turns a
  96 px social caption into a 171 px caption;
- measures the embedded Inter font exactly and wraps at word boundaries, with
  deterministic hard wrapping for a word wider than the safe line;
- uses an 8% delivery-safe inset and adapts the font downward only when the
  wrapped block still cannot fit;
- reserves the full 110% pop scale and 15% slide-up translation used by the
  built-in caption motions; and
- returns the same lines, font size, line height, and pixel bounds consumed by
  the renderer and QA.

The media renderer no longer owns separate positioning math. Preview and export
already share that renderer. Delivery conformance materializes the target
raster before running QA, so the assertion checks the pixels the target profile
will actually render.

## Failure behavior

QA emits a blocking `caption_outside_safe_area` issue when the complete
transform envelope leaves the safe bounds. A title that cannot fit even at the
minimum size emits blocking `title_layout_unavailable`. Delivery conformance
therefore refuses the artifact before export, and the benchmark independently
records a failed `delivery caption safe area` assertion.

The check evaluates transform parameters and every automation keyframe. It is
not a screenshot heuristic and does not depend on a selected proof frame.

The benchmark also fails before artifact acceptance when the timeline has no
real-time media clip backed by an audio or audio-video asset. Video-track A/V
clips already feed OpenReel's mixer; requiring a duplicate audio-track copy
would double the mix. After export, the independent probe must classify the MP4
as audio-video. This catches both editorial audio omission and mux failures;
human review remains responsible for mix quality.

## Runtime context reductions

Two additional reductions are included because the M35 task exercises both:

- `get_timeline_state` collapses a generated caption track into one line with
  cue count, clip ids, range, preset, and motion. Cue text and full keyframe
  curves remain available through the existing per-clip inspector rather than
  repeating on every state read. A 12-cue regression fixture is capped below
  600 rendered bytes.
- successful plans larger than eight operations return one atomic count and an
  operation-type breakdown. Failures retain per-operation status so the exact
  rejected operation and rollback boundary remain visible. The M35 48-operation
  success shape becomes one result line.

These deterministic response-byte reductions are now reflected in the live
provider telemetry reported below.

The first live attempt also exposed discovery churn and unreliable manual
source-boundary arithmetic. M37 now lets agents batch independent catalog
queries and open up to 16 exact capability schemas in one call. The new
`plan_dialogue_assembly` capability converts ordered assets, ready transcripts,
and raw silence analysis into one validated gapless `AddClip` plan. It applies
the same speech-safe silence rules as the benchmark and can remove conservative
filler words, leaving the model to make editorial choices instead of copying
frame math between tools.

## Verification before the paid run

Preparation is complete only when these local gates pass in one FFmpeg-enabled
PowerShell process:

```powershell
& .\scripts\setup-ffmpeg.ps1
cargo fmt --all -- --check
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Coverage includes every caption preset and built-in motion on vertical output,
an intentionally unsafe transform, exact rendered-pixel containment, compact
caption state, compact bulk success reporting, delivery conformance, and the
new benchmark assertion.

## Approval-gated benchmark command

Do not run this as part of ordinary tests. After explicit user approval, run:

```powershell
& .\scripts\setup-ffmpeg.ps1
$env:OPENREEL_EVAL = '1'
cargo run -p openreel-agent --bin openreel-eval -- --suite finished-cut-v2 --harness codex --only f1 --samples 1
```

No model override is supplied, matching the harness-default shape of the first
Codex baseline. The run writes its packaged artifact set under `target/evals`.
The result is not accepted until the generated MP4 is watched with audio and
the SHA-bound human review is completed.

## Live Codex result

The approved sample on commit `83ab8cd` passed all 32 machine assertions in one
turn. It made 18 tool calls, applied 28 operations, and completed in 108.126
seconds. Provider telemetry reported 232,447 input tokens, of which 165,120
were cached, plus 2,477 output tokens and 459 reasoning tokens. The benchmark's
portable total is 234,924 tokens.

Against the M35 passing baseline, this is:

- 496,387 fewer total tokens, a 67.9% reduction;
- 6 fewer tool calls, a 25.0% reduction;
- 20 fewer applied operations, a 41.7% reduction; and
- 14.745 seconds faster, a 12.0% reduction.

The independent artifact probe found 421 frames at 1080x1920 with both video
and audio streams. The MP4 is 6,486,073 bytes with SHA-256
`2b03c1487fe069bb29a0e33d51c14f73221874623c1331c3640d6b5c3757eaa5`.
The SHA-bound human review rejected the first pass. Ratings were story 2.0,
pacing 2.5, visual finish 2.5, audio finish 3.0, captions 2.0, and delivery
readiness 1.5, for a 2.25 mean. The reviewer heard two retained `um`s, found the
assembled story difficult to interpret, described awkward picture and audio
cuts, found no narrative intent in the synthetic color-bar visuals, and caught
a material caption error: `River map steadies the expedition` rendered as
`Map Steady the Exped`.

This is the intended outcome of keeping the score layers separate: M37 remains
a 32/32 machine pass but is a failed human first-pass gate. In particular, the
recognized-word assertion proved only that Whisper no longer reported the
fixture's expected filler token; it did not prove that no audible filler
survived. Caption presence, animation, and containment likewise did not prove
transcription accuracy.

## Tradeoffs and revisit points

- The 8% inset is a general delivery-safe contract, not a per-platform UI map.
  Add named platform overlays only with published targets and regression media.
- Adaptive shrink guarantees containment but cannot guarantee attractive
  typography for arbitrarily long titles. Caption generation should keep its
  readability limits; QA continues to warn on overly long or brief cues.
- The compact timeline summary intentionally omits generated cue text. If
  correction workflows need bulk caption text, add a paginated caption
  inspector instead of expanding the always-on project state again.
- M37 fixes the known visual escape. It does not pre-score pacing, taste, audio
  quality, or publishability. The human acceptance gate remains the authority.
- The generated color fields make the run reproducible but cannot measure
  meaningful shot selection or visual storytelling. Future finished-cut tasks
  need semantically legible footage and blind review.
- Filler and caption gates need an independent post-render listening or
  transcription pass. Reusing the same recognition evidence for editing and
  scoring allowed both audible fillers and inaccurate captions through.
