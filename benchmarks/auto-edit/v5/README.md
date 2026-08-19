# Kinewright Generalization Gauntlet v5

V5 is the M40 benchmark. It stops optimizing only for Kinewright's synthetic
garden story and measures unfamiliar, licensed footage in three distinct edit
families: interview/documentary, event/multicam, and music-led trailer editing.

All three families are executable. `g1` is a real public-domain interview with
filmmaker Helen Hill. `g2` is a CC BY 4.0 AMI meeting with four synchronized
participant cameras, a program headset mix, and pinned manual speaker labels.
`g3` is now a cuts-first single-source trailer edit built from the CC BY 3.0
Tears of Steel battle clip. It uses Scott Buckley's CC BY 4.0 trailer cue
"Vanguard" through a reviewed set of musical events and a short decay. Earlier
multi-source montage contracts remain historical evidence, but owner review
rejected the premise: one music bed made unrelated worlds read as one story,
neither source felt essential, and a quiet tail did not prove musical closure.
Each task uses real footage and independently probed MP4 delivery, not
generated bars or motion graphics.

## Immutable fixture acquisition

Downloaded footage is not committed. Its source page, license, byte count,
SHA-256, and exact URL live in `fixture-pack.json`,
`event-fixture-pack.json`, `music-fixture-pack.json`,
`music-fixture-pack-v2.json`, `music-fixture-pack-v3.json`, and the active
`music-fixture-pack-v4.json`. Acquisition is an explicit network action:

```powershell
& .\scripts\setup-ffmpeg.ps1
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --prepare-fixtures benchmarks/auto-edit/v5/fixture-pack.json
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --prepare-fixtures benchmarks/auto-edit/v5/event-fixture-pack.json
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --prepare-fixtures benchmarks/auto-edit/v5/music-fixture-pack-v4.json
```

Verify the local pack without network access:

```powershell
& .\scripts\setup-ffmpeg.ps1
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --verify-fixtures benchmarks/auto-edit/v5/fixture-pack.json
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --verify-fixtures benchmarks/auto-edit/v5/event-fixture-pack.json
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --verify-fixtures benchmarks/auto-edit/v5/music-fixture-pack-v4.json
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

## Single-source trailer preflight

`g3` measures the missing composition layer: whether the model can survey one
narrative source visually, choose a deliberate shot sequence, and recut it into
a trailer against detected musical onsets without hand-authoring frame
arithmetic. The active v9 contract is an 18-second action arc from Tears of
Steel. It moves forward through one source chronology: tower-scale mechanical
threat, device preparation, operator reveal, battle, and a later industrial
aftermath held through a short audible decay. It is a recovery target, not a
completed or human-accepted sample.

The recovery contract uses four general agent-facing primitives. One
coverage-mode `get_source_shot_board` call replaces the redundant broad
storyboard pass and returns up to 12 scene-derived candidate envelopes sampled
across the full source range, with start, middle, and end evidence frames.
Passing `minimum_duration_frames: 30` and
`minimum_confidence_basis_points: 1000`
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

`get_cut_neighborhoods` closes the source-board sampling gap after the edit. It
renders exact outgoing and incoming frames at every hard cut, measures large
secondary changes inside each incoming handle, and returns a blocking verdict.
The model must repair a dirty edge and re-run the proof without exchanging the
reviewed climax and resolution roles.

The v2 machine gate checked both visual sources, but that was not enough to
prove a coherent arc: a required-source assertion could force an isolated Big
Buck Bunny cameo, and the source-scene confidence floor did not fully veto
baked scene cuts. An independent audit caught a baked dissolve in an earlier
machine-passing candidate; that candidate was discarded before the published
v2 rerun. V2 also anchored the music start without requiring a musical source
endpoint or a quiet encoded tail, which left the published artifact ending
inside a phrase.

The active v9 contract keeps v4's single-source correction and fixes the music contract.
Tears of Steel must be the sole visual source and fill the complete video track
through exactly five disjoint, scene-clean shots. Four human-reviewed musical
events have explicit editorial roles: first lift, commitment, climax drive, and
release. The machine gate requires cuts at all four events, nonuniform cadence,
a held resolution, five ordered semantic source windows, strictly forward
source chronology, exact cut-edge cleanliness, source-audio exclusion,
loudness, no more than one second of perceptually inactive audio at the end,
and independent H.264/AAC MP4 probing. The fixture test proves the five-shot
schedule is beat-valid, cadence-valid, mixed-frame-rate safe, chronological,
and source-feasible before a model can spend tokens on it. Captions are
intentionally not part of this instrumental task and are marked not applicable
instead of receiving an invented score.

The first v4 model sample passed the old 37/37 machine assertions in one turn with 12
tool calls, 10 operations, and 221,521 total tokens. It uses eight Tears of
Steel shots over exactly 550 project frames, places five of seven cuts on
structural candidates, measures -15.99 LUFS, and ends at Vanguard source frame
6995 with an encoded five-frame tail below -46 dBFS peak. Its rendered SHA-256
is `9b813c6f6888e36e90ba3b2f5ad0938f8d3827374a2465161c3992aa40a8d99a`.
Human review rejected that exact artifact: the cuts and visual action did not
feel motivated by the music, and the cue became perceptually inactive around
15 seconds while picture continued for almost seven seconds. No numeric ratings
were supplied. That failure invalidated the old structural-share and terminal-
quiet gates; the 18-second reviewed-event and maximum-inactive-tail contract now
replaces them.

The first reviewed-event replacement passed 37/37 machine assertions in one turn
with 13 tool calls, 7 operations, and 238,496 total tokens. It uses the exact
five-shot cadence `[48, 78, 123, 126, 75]`, resolves music source frames
`6335..6875` with zero endpoint drift, measures -16.03 LUFS, and keeps the final
one-second encoded window above the -30 LUFS activity floor before ending below
-18.5 dBFS peak. Its rendered SHA-256 is
`816ece8a11a69b1048a949420a597a1839726fd8a9bcd58d3fbf7f3d482f824c`.
Human review rejected that exact artifact. The edit spent the five-second build
on a man looking at his arm, then placed the most climactic action after the
15-second musical hit under a fading note and silence. No numeric ratings were
supplied. V6 reverses those last two editorial roles: frames 249..375 must carry
the strongest sustained action, and frame 375 must cut away to a held low-motion
resolution with no fighting, firing, collision, destruction, or major robot
movement under the fade.

The first v6 replacement passes 37/37 machine assertions with 12 tool calls, 7
operations, and 218,894 total tokens. Its rendered SHA-256 is
`8c7f34c6633819a3b3f48bbe90a79fbee20fe96df15a6a500be8e7ebcae99d46`.
Human review rejected it without numeric ratings: the cut near 0:10 contained a
two-project-frame flash from one dirty source frame, and the final near-match
wide-to-wide cut read as a stutter.

V7 adds exact cut-neighborhood inspection with a blocking secondary-change
verdict, excludes the reviewed dirty source frame, and scores reviewed source
windows for the climax and resolution roles. The first combined run passes
39/39 with 12 tool calls, 7 operations, and 234,951 total tokens. Its rendered
SHA-256 is
`eeeb83c18af6e5b7af19334b871939aa9bd0853a4eeabc194ae2e05a6cf136f4`.
Encoded cut inspection confirms a clean frame-249 action in-point and a distinct
stable close-up at frame 375. Human review found the opening armored-balcony
shot disconnected from the workshop team, machine, and story carried by the
remaining four shots. No numeric ratings were supplied.

V8 adds a reviewed connected-opening role at source frames 716..789. The first
shot must establish the same workshop team, room, and machine continued by the
following shots. The first run passes 40/40 with 16 tool calls, 12 operations,
and 347,488 total tokens. Its rendered SHA-256 is
`e5aacda303f81cffb3479455faaf554b4ae46f3ca23a9bbf3296448e93574660`.
Encoded inspection confirms a workshop-wide opening, a motivated move inward
to the device at frame 48, and the previously repaired clean climax and release
cuts. Human review rejected this exact artifact: its source sequence
`716 -> 221 -> 482 -> 990 -> 309` put the story out of order, reused the former
ending image as its opening, and ended by returning to the earlier man-looking-
at-his-arm material. No numeric ratings were supplied.

V9 makes that failure machine-visible. `source_ranges_chronological` rejects a
single narrative asset when later timeline clips move backward or reuse earlier
source time, even if every range is otherwise disjoint. All five timeline roles
are also pinned to reviewed, ordered source windows. The first run passes 43/43
with 12 tool calls, 7 operations, and 236,944 total tokens. Its exact source
sequence is `165..211 -> 221..296 -> 482..613 -> 987..1107 -> 1285..1345`,
the output SHA-256 is
`c5e7fe4d3c8184c7cf2f33ae49f6f4f2c42704c1190c44b1db42616d6380cda6`,
and independent story and cut-neighborhood audits pass. A preceding 40/41
attempt was withheld because its battle select crossed a detected source cut;
it was never presented for human review. Human review of the v9 artifact is
pending.

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

The first local v3 diagnostic passed its then-current 38/38 machine assertions
with 15 tool calls, 11 operations, and 333,560 total tokens. Its rendered SHA-256
was `23a416bddc2753833c16f6e61cf555b7b7ab33a2a9ec84076a48df71c311e472`.
Independent review withheld it from human scoring and publication: Sintel
appeared in only two disconnected clips totaling 140 frames, while Tears of
Steel occupied seven clips and 560 frames. The old minimum-use rule had allowed
a source cameo while claiming multi-source composition. This diagnostic is not
a baseline and does not count toward the family gate. The contract now requires
three clips and 210 frames from each source plus distinct early and late
appearances. Its pinned pack and ground truth remain `music-fixture-pack-v3.json`
and `music-ground-truth-v3.json`.

Two fresh samples on that hardened contract now pass 40/40 machine assertions.
The first used 18 calls and 432,226 tokens; it used four Sintel shots over 293
frames and five Tears of Steel shots over 407 frames. Its rendered SHA-256 is
`203461a7331ad0b7ed45654954b244b63c71dc6e3ca1fd11f0f3a562ef22dac4`.
The planning trace exposed a token regression: six montage-planner calls
repeated the accumulated visual context. `plan_beat_montage` now returns the
nearest globally feasible source- and cadence-valid schedule as an exact retry
patch when a bounded schedule fails. The next sample needed three planner calls,
15 total calls, and 342,058 tokens: 20.9% fewer tokens while preserving the
stricter 40/40 result. It used four Sintel shots over 248 frames and five Tears
of Steel shots over 452 frames; its rendered SHA-256 is
`4c802c58d2a056f87bc305e742db5bdb9fbfc0dfafa2a200604835ee3857daf6`.
Neither artifact is a published or accepted baseline. Owner review rejected the
parallel-world premise itself: the shared music made the footage read as one
incoherent story, neither source felt essential, and the supposed resolved tail
still sounded mid-phrase. No numeric ratings were supplied for that review, so
none are invented. V4 replaces the premise rather than adding another quota.

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
