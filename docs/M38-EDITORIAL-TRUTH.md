# M38 - Editorial truth

M37 passed 32 machine assertions and failed human review at 2.25/5. M38 turns
that disagreement into a stronger benchmark and a shorter correction loop. It
does not claim that the editor is now human-acceptable. It makes the next result
much harder to misread.

## What changed

The `editorial-cut-v3` suite replaces the arbitrary M37 names and color bars
with one authored neighborhood-garden story. Five local takes are generated:
three form a coherent beginning, middle, and result; two are factual or delivery
mistakes. Each take has a semantically distinct 9:16 motion-graphic scene.

The checked-in ground truth owns:

- accepted take order;
- exact intended dialogue and captions;
- rejected facts and filler words;
- the SAPI performance and visual role for every source.

The fixture is still generated and redistributable. It is more legible than
color bars, but it is not a substitute for a future licensed real-footage suite.
In particular, it cannot prove professional shot selection, camera continuity,
or photoreal compositing taste.

## Independent score layers

Source Whisper timestamps guide the edit and prove that every recognized
selected word remains while every recognized `um` range is gone. The fixture
refuses to start unless the pinned recognizer actually heard its authored
fillers, so a missing source token cannot become a false pass.

The finished MP4 is then probed and transcribed as a new asset. Its ordered word
error rate against authored dialogue must be at most 15%. This is intentionally
not exact string equality: punctuation, compound-word segmentation, and small
recognizer variations are not editorial errors. Generated caption text remains
an exact normalized ordered-word comparison, which catches the M37
`Map Steady the Exped` defect deterministically.

Human review stays separate and SHA-bound. The published exit target is three
machine-passing samples, at least two human accepts, a mean rating of at least
3.5, no dimension below 3.0, no material caption error, and no audible filler.

## Agent correction surfaces

`get_captions` returns bounded pages of cue ids, text, presets, and exact ranges.
It keeps ordinary timeline state compact while making every generated word
reviewable. `plan_caption_corrections` validates up to 100 caption-only text
replacements against an exact revision and returns one atomic prepared plan.
It never mutates the timeline by itself.

`plan_dialogue_assembly` now accepts two explicit pacing controls:

- `retained_pause_source_frames` preserves a chosen amount of natural silence
  across each detector cut;
- `filler_padding_source_frames` removes a small boundary around a recognized
  filler so the audible onset or tail is not stranded.

Both values are included in the returned evidence. The model still chooses the
story and takes; OpenReel owns the frame arithmetic.

## Transcription reliability found during preparation

The full fixture smoke test exposed a bug in `whisper-rs` 0.16's safe abort
callback: the wrapper stores an erased closure but invokes it through a
different concrete pointer type. On Windows this intermittently aborts healthy
encoder and decoder graphs with errors `-6` and `-9`.

OpenReel no longer installs that callback. Cancellation is still observed
before decoding, before inference, immediately after inference, and before
cache publication. A cancellation requested during the synchronous Whisper
call becomes terminal at the next boundary rather than corrupting inference.
The eval path also supplies the known English language hint, avoiding an
unnecessary language-detection pass while normal application requests remain
multilingual by default.

## Local verification

The generated fixture gate spends no model-session quota:

```powershell
& .\scripts\setup-ffmpeg.ps1
cargo test -p openreel-agent --bin openreel-eval `
  v3_fixture_builds_with_authored_and_recognized_truth -- --ignored
```

It renders all five sources, transcribes fresh authored speech, runs silence
analysis, verifies the portrait project contract, and reconciles the recognized
and authored truth sets. Ordinary workspace formatting, tests, build, and
strict Clippy remain required before a live sample.

## Live run

The subscription-backed model run is explicit:

```powershell
& .\scripts\setup-ffmpeg.ps1
$env:OPENREEL_EVAL = '1'
cargo run -p openreel-agent --bin openreel-eval -- `
  --suite editorial-cut-v3 `
  --harness codex `
  --only f2 `
  --samples 3
```

Machine green still does not mean accepted. The exact MP4s must be watched with
audio and scored through their generated human-review file.
