# Kinewright Generalization Gauntlet v5

V5 is the M40 benchmark. It stops optimizing only for Kinewright's synthetic
garden story and measures unfamiliar, licensed footage in three distinct edit
families: interview/documentary, event/multicam, and music montage.

All three families are executable. `g1` is a real public-domain interview with
filmmaker Helen Hill. `g2` is a CC BY 4.0 AMI meeting with four synchronized
participant cameras, a program headset mix, and pinned manual speaker labels.
`g3` is a cuts-first horizontal music montage built from two CC BY 3.0 Blender
Foundation productions. Its rejected historical preflight used Kevin MacLeod's
CC BY 4.0 instrumental "Cipher"; the v2 recovery used Scott Buckley's CC BY
4.0 "Uprising." The prepared v3 recovery keeps that cue, replaces the forced
Big Buck Bunny cameo with a compatible Tears of Steel source, and gives the
editor a real musical event and natural ending to cut around. Each task uses
real footage and independently probed MP4 delivery, not generated bars or
motion graphics.

## Immutable fixture acquisition

Downloaded footage is not committed. Its source page, license, byte count,
SHA-256, and exact URL live in `fixture-pack.json`,
`event-fixture-pack.json`, `music-fixture-pack.json`,
`music-fixture-pack-v2.json`, and `music-fixture-pack-v3.json`. Acquisition is
an explicit network action:

```powershell
& .\scripts\setup-ffmpeg.ps1
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --prepare-fixtures benchmarks/auto-edit/v5/fixture-pack.json
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --prepare-fixtures benchmarks/auto-edit/v5/event-fixture-pack.json
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --prepare-fixtures benchmarks/auto-edit/v5/music-fixture-pack-v3.json
```

Verify the local pack without network access:

```powershell
& .\scripts\setup-ffmpeg.ps1
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --verify-fixtures benchmarks/auto-edit/v5/fixture-pack.json
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --verify-fixtures benchmarks/auto-edit/v5/event-fixture-pack.json
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --verify-fixtures benchmarks/auto-edit/v5/music-fixture-pack-v3.json
```

`KINEWRIGHT_EVAL_FIXTURE_DIR` overrides the cache root. Existing files with a
wrong length or hash are rejected and never silently overwritten. Benchmark
execution itself never downloads inputs.

## Run the interview task

```powershell
& .\scripts\setup-ffmpeg.ps1
$env:KINEWRIGHT_EVAL = '1'
cargo run -p kinewright-agent --bin kinewright-eval -- `
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
$env:KINEWRIGHT_EVAL = '1'
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --suite generalization-v5 `
  --harness codex `
  --only g2 `
  --samples 1
```

## Music-montage preflight

`g3` measures the missing composition layer: whether the model can survey raw
source footage visually, choose a deliberate shot sequence, and assemble it
against detected musical onsets without hand-authoring frame arithmetic. The
prepared v3 recovery is a 28-second action-to-release arc across Sintel and
Tears of Steel, ending at a verified natural tail of the Uprising cue. It is a
recovery target, not a completed or human-accepted run.

The recovery contract uses three general agent-facing primitives. One
full-range `get_source_shot_board` call per source replaces the redundant broad
storyboard pass and returns up to 10 exact scene-derived candidate envelopes
with start, middle, and end evidence frames. Passing
`minimum_duration_frames: 55` and `minimum_confidence_basis_points: 1000`
filters unusably short candidates while retaining low-confidence boundaries as
source-cut vetoes. This spends fewer model tokens while making baked edits
harder to miss; candidate ids and indexes remain stable, and ranges crossing a
detected source boundary are inadmissible.
`get_music_structure` returns beat, bar, and phrase candidates for the selected
music range. Its roles and confidence are explicitly heuristic, so the model
still has to make the editorial decision. Finally, `plan_beat_montage` accepts
the model's ordered source envelopes and exact `cut_anchor_frames`, finds a
source-feasible gapless hard-cut assembly, and returns one opaque
revision-gated plan. These tools do not choose the footage, invent
transitions, retime clips, or claim to score taste.

The v2 machine gate checked both visual sources, but that was not enough to
prove a coherent arc: a required-source assertion could force an isolated Big
Buck Bunny cameo, and the source-scene confidence floor did not fully veto
baked scene cuts. An independent audit caught a baked dissolve in an earlier
machine-passing candidate; that candidate was discarded before the published
v2 rerun. V2 also anchored the music start without requiring a musical source
endpoint or a quiet encoded tail, which left the published artifact ending
inside a phrase.

The prepared v3 contract closes those gaps. It uses compatible Tears of Steel
footage, requires at least two clips and 120 project frames from each visual
asset, lowers the scene-boundary veto floor to 10% confidence, measures the
actual first and last shot holds, anchors the music source end exactly at the
cue's natural tail, and verifies the final encoded five-frame window is quiet.
It retains scene-clean source exclusions, nonuniform shot cadence, structural
music anchors, source-audio exclusion, loudness, and independent H.264/AAC MP4
probing. Captions are intentionally not part of this instrumental task and are
marked not applicable in human review rather than receiving an invented score.

Prepare the pack as above, then run:

```powershell
& .\scripts\setup-ffmpeg.ps1
$env:KINEWRIGHT_EVAL = '1'
cargo run -p kinewright-agent --bin kinewright-eval -- `
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
and is published separately in `music-montage-recovery-baseline.json`. Human
review rejected that exact SHA-bound artifact with story 2.5, pacing 3.5, visual
finish 4.0, audio finish 2.0, delivery readiness 2.0, and captions not
applicable. The story was "better, the bunny thing now feels very out of place".
The main issue here is that the whole just ends in the middle of a musical
phrase. It feels like we're ending in the middle of a longer video, not that the
video has one coherent arc. It used a 24-second section of "Uprising" with a
pronounced midpoint lift, plus bounded and inspectable anchor repair. Its edit
contract was 600 project frames with a held dramatic Sintel opening, a clear
pivot into Big Buck Bunny's playful energy, action-oriented development at the
musical lift, and a held finish; at least three duration bands; no more than
three near-equal shots in a row; exact scene-derived source envelopes; and
structural bar/phrase anchors carrying at least half of the cuts.

The v3 recovery is prepared but has not run. Its pinned pack and ground truth
are `music-fixture-pack-v3.json` and `music-ground-truth-v3.json`. It replaces
the forced Bunny cameo with Tears of Steel, requires meaningful use of both
assets, treats low-confidence scene changes as vetoes, checks the actual edge
shot holds, anchors the music source end to the natural cue tail, and verifies
the encoded tail. No v3 machine result or human review exists yet.

The published v2 recovery also passed a separate 16-frame overview, every-shot
midpoint sheet, and before/at/after inspection around all eight cuts. That audit
was necessary because the v2 machine contract had not fully caught baked source
cuts: an earlier machine-passing candidate was discarded after its final shot
began inside a baked dissolve. The v2 exclusion was then widened and fixture
feasibility stopped counting frames inside excluded intervals. V3 makes that
lesson explicit with the lower-confidence scene-boundary veto and per-shot
edge checks.

V5 exits only after three samples per family pass the machine contract and the
published human gate in `manifest.json` passes.
