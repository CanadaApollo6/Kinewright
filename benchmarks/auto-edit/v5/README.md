# OpenReel Generalization Gauntlet v5

V5 is the M40 benchmark. It stops optimizing only for OpenReel's synthetic
garden story and measures unfamiliar, licensed footage in three distinct edit
families: interview/documentary, event/multicam, and music montage.

All three families are executable. `g1` is a real public-domain interview with
filmmaker Helen Hill. `g2` is a CC BY 4.0 AMI meeting with four synchronized
participant cameras, a program headset mix, and pinned manual speaker labels.
`g3` is a cuts-first horizontal music montage built from two CC BY 3.0 Blender
Foundation trailers. Its rejected historical preflight used Kevin MacLeod's
CC BY 4.0 instrumental "Cipher"; the recovery fixture uses Scott Buckley's
CC BY 4.0 "Uprising," whose slow-burn-to-heroic transition gives the editor a
real musical event to cut around. Each task uses real footage and
independently probed MP4 delivery, not generated bars or motion graphics.

## Immutable fixture acquisition

Downloaded footage is not committed. Its source page, license, byte count,
SHA-256, and exact URL live in `fixture-pack.json`,
`event-fixture-pack.json`, `music-fixture-pack.json`, and
`music-fixture-pack-v2.json`. Acquisition is an explicit network action:

```powershell
& .\scripts\setup-ffmpeg.ps1
cargo run -p openreel-agent --bin openreel-eval -- `
  --prepare-fixtures benchmarks/auto-edit/v5/fixture-pack.json
cargo run -p openreel-agent --bin openreel-eval -- `
  --prepare-fixtures benchmarks/auto-edit/v5/event-fixture-pack.json
cargo run -p openreel-agent --bin openreel-eval -- `
  --prepare-fixtures benchmarks/auto-edit/v5/music-fixture-pack-v2.json
```

Verify the local pack without network access:

```powershell
& .\scripts\setup-ffmpeg.ps1
cargo run -p openreel-agent --bin openreel-eval -- `
  --verify-fixtures benchmarks/auto-edit/v5/fixture-pack.json
cargo run -p openreel-agent --bin openreel-eval -- `
  --verify-fixtures benchmarks/auto-edit/v5/event-fixture-pack.json
cargo run -p openreel-agent --bin openreel-eval -- `
  --verify-fixtures benchmarks/auto-edit/v5/music-fixture-pack-v2.json
```

`OPENREEL_EVAL_FIXTURE_DIR` overrides the cache root. Existing files with a
wrong length or hash are rejected and never silently overwritten. Benchmark
execution itself never downloads inputs.

## Run the interview task

```powershell
& .\scripts\setup-ffmpeg.ps1
$env:OPENREEL_EVAL = '1'
cargo run -p openreel-agent --bin openreel-eval -- `
  --suite generalization-v5 `
  --harness codex `
  --only g1 `
  --samples 1
```

The machine gate checks story facts and exclusions, exact caption words,
sentence grouping, duration, gapless media, audio, QA, tool evidence, budgets,
the rendered file, and independent rendered-speech transcription. A person
still owns the acceptance, story, pacing, visual, audio, caption, and delivery
ratings for the exact SHA-bound artifact.

## First preflight baseline

The first corrected `g1` preflight is published in `baseline.json`:

- 1/1 sample and 25/25 machine assertions passed;
- 7 model tool calls and 44 committed edit operations;
- 108,701 total tokens, including 70,144 cached input tokens;
- 1,038 frames at 1080x1920;
- 9.20% independent rendered-speech word error rate;
- output SHA-256
  `0aa88e6fc3761867734d282403acdf505061cab38e997b4fde2610ef5aed9ccc`.

An earlier preflight exposed a scorer bug: numeric transcript tokens such as
`8` and `12` were removed from the expected caption sequence even though the
edit retained them. The expected-word builder now preserves those tokens, and
that invalid failure is not included in the baseline.

Human review rejected the artifact with story 5.0, pacing 5.0, visual finish
3.0, audio finish 5.0, captions 2.5, and delivery readiness 3.0. The edit and
audio were publishable, but centered blue captions obscured the subject,
omitted words, lacked punctuation, and grouped phrases illogically.

That rejection now has executable regression coverage. The task supplies a
corrected verbatim script, requires subject-safe neutral caption presentation,
scores semantic phrase boundaries and the exact answer endpoint, and requires
burned-in caption words to match independently transcribed rendered speech with
zero word errors. The source is a low-resolution 3:2
close-up, so the vertical crop remains necessarily tight.

The resulting machine recovery is published separately in
`caption-recovery-baseline.json`: 28/28 assertions, exact source range
`[1682, 2547)`, 0% rendered-dialogue WER, 0% caption/audio WER, 7 tool calls,
44 operations, and 111,225 total tokens. Its human review is pending; the
rejected first artifact remains unchanged in `baseline.json`.

## Event/multicam preflight

`g2` edits the 31.76-second AMI `ES2002a` introduction into exactly five
speaker-selected shots. It must preserve the program audio as one untouched
clip and produce stable editable 9:16 reframe curves for every shot. The
machine contract scores exact source/timeline ranges, camera order, audio
identity, reframe stability, QA, undo, budgets, the rendered MP4, and
independent rendered-dialogue transcription.

The original preflight passed its then-current 23/23 machine assertions but was
human-rejected for inaudible programme audio and unsafe late Laura framing. It
remains immutable in `event-multicam-baseline.json`.

The recovery contract independently measures encoded loudness, verifies that
precise animated reframes and compact subject-provenance sidecars survive
delivery, and requires every tracked subject box to remain inside the real
aspect-aware crop. The official recovery at revision `181b35c` passed 25/25
assertions in one turn with 21 tool calls, 26 operations, and 427,778 total
tokens, of which 388,864 were cached input. Its 794-frame 1080x1920 output is
40,126,007 bytes with SHA-256
`262491f9f849ed26fe921917f7769ebe8d5a7fcdd22a968b7aa98c4787b0396a`;
programme audio measures -16.98 LUFS and -1.72 dBFS peak. Exact run metadata is
published in `event-multicam-recovery-baseline.json`. The project owner
accepted that exact artifact with the feedback "Nailed it." No numeric ratings
were supplied, so none are invented.

The benchmark also forced a runtime efficiency fix. Deterministic multicam,
beat, music-fit, mask-tracking, and reframe-tracking results now return opaque
`prepared_edit_plan` handles instead of making the model copy operation arrays
through another planning call. The event edit fell from 37 calls to 20, and the
gate is now capped at 24.

Run it with:

```powershell
& .\scripts\setup-ffmpeg.ps1
$env:OPENREEL_EVAL = '1'
cargo run -p openreel-agent --bin openreel-eval -- `
  --suite generalization-v5 `
  --harness codex `
  --only g2 `
  --samples 1
```

## Music-montage preflight

`g3` measures the missing composition layer: whether the model can survey raw
source footage visually, choose a deliberate shot sequence, and assemble it
against detected musical onsets without hand-authoring frame arithmetic. The
24-second recovery brief asks for a contrast arc from Sintel's dramatic imagery into
Big Buck Bunny's playful energy, then a clean musical finish.

The recovery contract exposes three general agent-facing primitives. The broad
`get_source_storyboard` contact sheet lets the model survey each full source
before footage is on the timeline. One full-range `get_source_shot_board` call
per source then returns up to 12 exact scene-derived candidate envelopes with
start, middle, and end evidence frames. Passing
`minimum_duration_frames: 50` and `minimum_confidence_basis_points: 5000`
filter short candidates and weak motion-derived boundaries before pagination,
so the model spends tokens on shots that can actually serve the 50-frame minimum;
the manifest reports the requested threshold plus filtered and total counts,
while candidate ids and indexes remain stable. Candidates crossing a detected
source boundary are not admissible.
`get_music_structure` returns beat, bar, and phrase candidates for the selected
music range. Its roles and confidence are explicitly heuristic, so the model
still has to make the editorial decision. Finally, `plan_beat_montage` accepts
the model's ordered source envelopes and exact `cut_anchor_frames`, finds a
source-feasible gapless hard-cut assembly, and returns one opaque
revision-gated plan. These tools do not choose the footage, invent
transitions, retime clips, or claim to score taste.

The machine gate checks both visual sources are used without overlapping source
ranges, every selected envelope is scene-clean, shot durations have at least
three substantially different bands rather than a long near-equal run, and at
least half of the internal cuts land on structural bar or phrase candidates.
It also checks every internal cut against the detected music beat set, the
music bed is one exact real-time clip, source audio is absent, encoded
loudness is within contract, and the horizontal H.264/AAC MP4 is independently
probed. Captions are intentionally not part of this instrumental task and are
marked not applicable in human review rather than receiving an invented score.

Prepare the pack as above, then run:

```powershell
& .\scripts\setup-ffmpeg.ps1
$env:OPENREEL_EVAL = '1'
cargo run -p openreel-agent --bin openreel-eval -- `
  --suite generalization-v5 `
  --harness codex `
  --only g3 `
  --samples 1
```

## First music-montage preflight

The first machine-passing `g3` preflight is published in
[`music-montage-baseline.json`](music-montage-baseline.json):

- 1/1 sample and 22/22 machine assertions passed;
- 11 tool calls, 12 edit operations, and 154,358 total tokens;
- 800 frames at 1920x1080 with 10 visual shots and one music clip;
- rendered audio measured -16.02 LUFS and -2.55 dBFS peak;
- output SHA-256
  `0cb3f6bdebe4a593887cb19d2817ccb761731acc4ac92c68d5171e6e88b0cab1`.

Human review of that exact artifact rejected it: story 1.0, pacing 1.5, visual
finish 2.0, and audio finish 4.5; captions were not applicable, and no numeric
delivery-readiness score was supplied. The reviewer found no discernible arc,
near-random and incoherent sequencing, metronomic and sometimes rushed pacing,
unmotivated cuts, occasional fades despite the stated hard-cut intent, and no
meaningful contrast between the visual styles. Audio was consistent and
audible, but the instrumental did not fit the footage. V5 remains `in_progress`;
this is one music sample, not the three-sample family gate or the M40 exit gate.

This rejected artifact and its v1 Cipher fixture remain immutable in meaning
and are not silently re-scored. The v2 recovery passed 34/34 machine assertions
and is published separately in `music-montage-recovery-baseline.json`; human
review is pending. It uses a 24-second section of "Uprising" with a pronounced midpoint
lift, plus bounded and inspectable anchor repair. Its edit contract is 600
project frames with a held dramatic Sintel opening, a clear pivot into Big Buck
Bunny's playful energy, action-oriented development at the musical lift, and a
held finish; at least three duration bands; no more than three near-equal shots
in a row; exact scene-derived source envelopes; and structural bar/phrase
anchors carrying at least half of the cuts. The original review supplied no
numeric delivery-readiness score, so the v1 baseline records that dimension as
`null`.

The published recovery also passed a separate 16-frame overview, every-shot
midpoint sheet, and before/at/after inspection around all eight cuts. An earlier
machine pass was discarded when that independent audit found a baked dissolve
at the start of the last source range. The v2 exclusion now covers the full
dissolve tail, and fixture feasibility no longer counts frames inside excluded
intervals.

V5 exits only after three samples per family pass the machine contract and the
published human gate in `manifest.json` passes.
