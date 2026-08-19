# M40 - Generalization Gauntlet

## Outcome

M40 moves Kinewright's quality claim from one tuned synthetic story to unfamiliar,
licensed footage across three edit families:

1. interview/documentary;
2. event/multicam;
3. music montage.

The milestone is benchmark-led. A new primitive ships only when a task exposes
the need and can score the result. Machine checks own exact facts, timing,
conformance, and artifact identity. A person still owns taste and acceptance.

M40 is **in progress**. Interview/documentary, event/multicam, and music
montage are executable. None has yet satisfied the full three-sample machine
and two-accept human exit gate.

## Phase 1 - licensed fixture packs

Synthetic media could be generated inside a fixture function. Real footage
needs a reproducible boundary of its own. `kinewright-agent::fixture_pack`
provides it:

- one checked-in JSON manifest per pack;
- HTTPS acquisition only;
- source page, license name, license URL, and attribution on every asset;
- exact byte length and lowercase SHA-256;
- safe, unique cache file names;
- explicit `--prepare-fixtures` network acquisition;
- offline `--verify-fixtures` and benchmark-time verification;
- atomic temporary downloads;
- no silent overwrite of an existing changed file;
- `KINEWRIGHT_EVAL_FIXTURE_DIR` for a shared or custom cache.

Downloaded media remains outside Git. A benchmark cannot start if a file is
missing, truncated, or has drifted.

## Phase 1 - real interview task

`generalization-v5` task `g1` uses the public-domain [Helen Hill interview on
Wikimedia Commons](https://commons.wikimedia.org/wiki/File:HelenHillInterview.theora.ogv).
The pinned input is Wikimedia's 480p VP9 transcode: 9,294,247 bytes with SHA-256
`cc860fe89cdd7e1d653a55fa7458636e4b9d980915d50dedb7ba86d1d86c8656`.

The two-minute source contains an interviewer, event logistics, definitions,
the target story, and a later organizer discussion. The model must isolate
Helen Hill's Hurricane Katrina film-recovery answer, clean it, preserve source
dialogue, generate exact transcript-backed captions, check the vertical frame,
and produce a real MP4.

The evaluator independently checks:

- required recovery facts and off-topic exclusions;
- one gapless source assembly within duration bounds;
- exact caption word order and sentence grouping;
- caption safe area, audio presence, and technical QA;
- required model evidence and resource budgets;
- output probe, hash, and independent rendered-speech word error rate.

The human rubric remains acceptance, story, pacing, visual finish, audio
finish, captions, and delivery readiness. The MP4 SHA binds those ratings to an
exact artifact.

## First preflight result

The corrected `g1` preflight passed 25/25 machine assertions in one turn. It
used 7 tool calls, 44 edit operations, and 108,701 total tokens. The delivered
vertical MP4 is 1,038 frames, and independent post-render transcription measured
9.20% word error rate against the pinned story transcript. Its SHA-256 is
`0aa88e6fc3761867734d282403acdf505061cab38e997b4fde2610ef5aed9ccc`.

The checked-in `benchmarks/auto-edit/v5/baseline.json` binds those results to
the fixture, implementation revision, trace hash, proof hash, and output hash.
Human review rejected that exact artifact despite its machine pass: story 5.0,
pacing 5.0, visual finish 3.0, audio finish 5.0, captions 2.5, and delivery
readiness 3.0. The single-shot story and audio were publishable. Centered blue
captions obscured the subject, omitted words, lacked punctuation, and grouped
phrases illogically. This is one preflight for one family, not the three-sample
interview gate and not completion of M40.

The first attempt found an evaluator defect rather than an editing regression:
the expected-caption builder discarded numeric transcript tokens while the
rendered edit correctly retained `8` and `12`. The scorer now builds exact
caption expectations from the authored whitespace-token sequence. The invalid
failure is preserved in local run artifacts but excluded from the baseline.

The rejection also closed the machine/human gap. Caption generation now has an
explicit verbatim versus edited-readable contract, semantic phrase grouping,
corrected-script support, subject-aware top/lower-third placement, and a neutral
high-contrast social preset. V5 now separately verifies caption presentation,
semantic phrase boundaries, the exact answer endpoint, and the delivered
caption words against independently transcribed rendered speech with zero word
errors tolerated, rather than accepting a caption sequence that only matches
stale ASR.

The stricter recovery run is recorded in
`benchmarks/auto-edit/v5/caption-recovery-baseline.json`. It passed 28/28
assertions with the exact `[1682, 2547)` source range, 0% rendered-dialogue WER,
0% caption/audio WER, 7 tool calls, 44 operations, and 111,225 total tokens.
Human acceptance is pending. The original rejected artifact remains preserved
in `baseline.json` rather than being overwritten by the recovery run.

## Product capability found by the benchmark

The existing `plan_dialogue_assembly` could clean whole ordered assets but not
one answer inside a long recording. M40 adds optional `source_ranges`, one per
ordered asset. The planner now:

- validates every source envelope against its asset;
- removes silence and conservative fillers only inside that envelope;
- never leaks earlier or later interview material into the plan;
- maps every retained source range to exact project frames;
- returns the same revision-bound, previewable, atomic edit plan as before.

This is a general model-facing primitive, not a benchmark exception. It makes
"assemble and clean these answers" cheap and safe for interviews, podcasts,
oral histories, and event sound bites.

## Phase 2 - real event/multicam task

`generalization-v5` task `g2` uses the University of Edinburgh's
[AMI Meeting Corpus](https://groups.inf.ed.ac.uk/ami/corpus/), meeting
`ES2002a`, under CC BY 4.0. The immutable pack pins four synchronized close-up
cameras, the program headset mix, and the official annotation archive. The
download is about 234 MiB and remains outside Git.

The benchmark bounds a 31.76-second introduction at 25 fps. A speaker-labelled
sidecar derived from the pinned manual annotations names Laura, David, Andrew,
and Craig. The model must build exactly five contiguous speaker-driven shots,
preserve one continuous program-audio clip, normalize it to a measured delivery
target, and add editable face-safe tracked 9:16 reframe curves to every shot.
It must not add captions, music, titles, transitions, or dialogue edits.

The evaluator independently checks:

- the exact camera, timeline, and source range of all five video shots;
- the exact source range and continuous state of the program mix;
- gapless 794-frame coverage and delivery duration;
- one reframe effect per shot, keyframe count, face-safe bounds, linear motion,
  and a 2% maximum per-sample camera move;
- rendered integrated loudness from -18 to -14 LUFS and a -1 dBFS sample-peak ceiling;
- technical QA, vertical delivery conformance, undo integrity, and budgets;
- a real 1080x1920 MP4, output hash, frame count, duration, audio, and
  independently transcribed rendered dialogue.

The first model run produced the correct artifact but failed the harness. It
used 37 calls against a provisional 36-call cap, and the evaluator required the
retired direct `apply_edit_plan` tool even though the compact runtime does not
expose it. The trace revealed the deeper product issue: multicam and tracking
planners still returned copied operation arrays instead of the opaque
`prepared_edit_plan` handles promised by the compact contract.

The repaired planner boundary validates operations server-side and returns
handles that commit directly. The generic authored-plan decoder also accepts a
JSON-stringified operation object, a representation emitted by the harness,
without weakening operation validation. This removed 17 calls from the same
edit. The published final preflight passed 23/23 assertions in one turn with 20
tool calls, 20 edit operations, and 393,543 total tokens, of which 334,080 were
cached input. The 39,380,998-byte MP4 is 794 frames at 1080x1920 with SHA-256
`1c168637e2bcb5ba7447d6dafaf19846019cd701beaba01e8824a311f250dc07`.

Human review rejected that SHA-bound artifact. Speaker selection and cut rhythm
were sound, and most crops were good, but Laura drifted out of the safe frame.
More importantly, both the source segment and rendered programme measured about
-39.9 LUFS. The evaluator had mistaken "an audio stream exists" for audible
delivery. M40 now treats loudness and safe subject framing as independent
machine contracts. The editor measures BS.1770-style integrated loudness,
previews a compressor/gain/limiter bus, rerenders it in memory, and only returns
the revision-gated plan when the requested loudness and peak bounds are met.
The original rejected result remains immutable in
`event-multicam-baseline.json`; a recovery artifact must get its own baseline.

The first loudness recovery exposed AAC peak overshoot: its in-memory mix met
the requested ceiling, but the decoded MP4 reached +0.24 dBFS. Normalization
now reserves 2 dB of encoder headroom and verifies the encoded artifact rather
than trusting the pre-encode mix. The next model edit passed 24/24 then-current
assertions in one turn with 21 tool calls, 21 operations, and 417,385 total
tokens. Its encoded programme measured -16.98 LUFS with a -1.72 dBFS sample
peak.

Visual review of that run uncovered a second evaluator defect. The master edit
contained five animated reframes, but delivery materialization discarded every
curve and substituted a static centered crop. This made tracking changes
produce byte-identical proof sheets and explained Laura's framing failure.
Delivery now preserves same-aspect animated reframes, the artifact scorer
explicitly fails if those curves do not survive, and saved documents can be
rerendered without spending another agent turn. Rerendering the exact saved
model edit produced a distinct proof and a 39,718,336-byte MP4 with SHA-256
`468ffa70090b17ee85f8a149330b7fb641584b7bed8cd49f1fbfe7e984e7e5b8` while
retaining -16.98 LUFS and -1.72 dBFS. Human review confirmed the loudness fix
but rejected the framing: the camera visibly stuttered as it chased tracking
samples, and the final Laura shot still did not travel far enough to contain
her. It is not promoted to a checked-in baseline.

The failure came from treating raw tracker centers as camera positions every 12
frames and easing each short segment independently. That produced repeated
stop-start motion and allowed tracker noise to become visible camera motion.
Tracking now feeds a virtual camera: a three-sample median filter rejects
one-sample noise, a 6% subject dead zone prevents needless corrections, camera
travel is limited to 2% per sample, and every segment is linear so sustained
movement has continuous velocity. Focus automation uses basis points rather
than whole percentages; on this 352x288-to-1080x1920 crop, that reduces the
smallest representable horizontal movement from about 23.5 output pixels to
about 0.24 pixels. The evaluator rejects eased or faster curves and separately
checks that tracked subject bounds remain inside the real aspect-aware crop.
The next official sample also uses the crop's actual geometric travel
range (25-75% horizontally) instead of the overly conservative 45-55% clamp.
That first strict sample passed 24/25 checks but exposed a controller defect:
the reactive camera reached two left-edge constraints late, missing Andrew by
127 basis points and Laura by 17. The score remained strict. Tracking now
inverts the evaluator's exact aspect-aware crop geometry into an allowed focus
interval at every observation, then solves the complete path with a forward
reachable-interval pass and backward selection. The camera begins moving before
a future edge while retaining the 2% maximum step; it returns an explicit error
when no fully containing path exists.

The official recovery run at revision `181b35c` passed 25/25 assertions in one
turn with 21 tool calls, 26 operations, and 427,778 total tokens, including
388,864 cached input tokens. All five precise animated reframes and all five
tracked-subject sidecars survived delivery. The 40,126,007-byte, 794-frame
1080x1920 MP4 has SHA-256
`262491f9f849ed26fe921917f7769ebe8d5a7fcdd22a968b7aa98c4787b0396a`;
its programme measures -16.98 LUFS with a -1.72 dBFS sample peak. The project
owner reviewed that exact artifact and reported "Nailed it," providing the
event family's first human-accepted output. No numeric ratings were supplied,
so none are invented. The immutable recovery record is
`benchmarks/auto-edit/v5/event-multicam-recovery-baseline.json`.

Readiness also gained an explicit `check_silence` policy. Event work that must
preserve continuous program audio can skip irrelevant dead-air analysis while
still running technical QA, delivery conformance, and the real storyboard.
The original trace ended with editorial readiness `true`, proving that
readiness also lacked the audible-delivery signal. One accepted recovery sample
is not the three-sample family gate. The rejected original remains recorded in
`benchmarks/auto-edit/v5/event-multicam-baseline.json`; the accepted recovery is
recorded separately.

## Phase 3 - single-source trailer editing

`generalization-v5` task `g3` uses Blender Foundation footage. V1 through v3
tested multi-source montage contracts. Human review showed that the premise was
the problem: one music bed made disconnected worlds read as one story, neither
source felt essential, and a quiet tail still did not sound like a resolved
phrase. V4 is therefore a simpler 22-second trailer edit from one Tears of
Steel battle source, cut to Scott Buckley's trailer cue "Vanguard" through its
authored final tag and decay. All fixture versions remain pinned and openly
licensed. The task is deliberately horizontal: g1 and g2 already exercise 9:16
delivery, while g3 isolates source inspection, shot selection, beat sense,
story construction, and music finishing.

The benchmark exposed three agent-facing gaps. A model could inspect frames only
after putting footage on the timeline, the old beat planner could split one
existing clip but could not assemble model-chosen source selects, and there was
no compact representation of musical hierarchy for deliberate cut placement.
M40 adds:

- `get_source_storyboard`, a bounded source contact sheet whose manifest maps
  every cell to an exact asset frame;
- `get_source_shot_board`, a ranged scene-derived shot board with exact
  candidate envelopes plus start, middle, and end evidence frames;
- `get_music_structure`, a read-only heuristic beat/bar/phrase hierarchy whose
  confidence is disclosed to the model rather than presented as musicological
  truth;
- `plan_beat_montage`, a deterministic planner that accepts ordered source
  envelopes and explicit cut anchors, validates scene-clean source ranges,
  selects source-feasible boundaries under explicit shot-length bounds, and
  returns one atomic prepared plan. Explicit anchors remain strict by default;
  an opt-in repair mode searches the nearest globally source- and cadence-valid
  detected-beat schedule under a hard movement bound, preserving shot order and
  reporting every requested-to-resolved delta.

The model still owns the creative decision: which images to use, where each
source envelope begins, and the order that tells the contrast story. Kinewright
owns mixed-frame-rate mapping, beat snapping, collision policy, validation, and
revision safety. The first slice is honest hard cuts. It does not silently add
crossfades, speed ramps, looping, time stretch, or semantic shot selection.

The historical v2 recovery contract required 8-10 gapless visual shots over
exactly 24 seconds (600 project frames), both visual sources, separated and
scene-clean source ranges, 50-120-frame shots, at least three duration bands
without a long near-equal or repeating A/B run, and at least half of the
internal cuts on structural bar or phrase candidates. It required a held Sintel
opening, a Big Buck Bunny pivot, a held Sintel finish, and zero transitions,
effects, fades, or retiming. Those checks still allowed a forced cameo, did not
fully veto baked source cuts, and did not require a musical endpoint or quiet
encoded tail.

The active v4 contract retains the useful cadence, source-clean, endpoint, and
encoded-delivery checks while removing the source quota entirely. Tears of
Steel is the only visual source and must fill the entire 550-frame timeline in
eight or nine disjoint scene-clean shots. The story target is explicit:
establish the human team and weapon, reveal the mechanical threat, escalate
destruction, peak on confrontation, and resolve on a held survivor, team, or
aftermath image. The cue ends within 15 source frames of its reviewed endpoint,
which includes Vanguard's final tag plus decay. The fixture test proves an
eight-shot schedule is source-feasible, beat-valid, cadence-valid, and safe
across the 24 fps source, 25 fps project, and 30 fps music before model tokens
are spent. Human review still owns story, rhythm, visual finish, audio finish,
and delivery readiness. Captions are explicitly not applicable.

The first machine-passing `g3` preflight is recorded in
[`benchmarks/auto-edit/v5/music-montage-baseline.json`](../benchmarks/auto-edit/v5/music-montage-baseline.json).
It passed 22/22 assertions in one turn with 11 tool calls, 12 operations, and
154,358 total tokens. The delivered 800-frame 1920x1080 MP4 measures -16.02
LUFS and -2.55 dBFS peak, with SHA-256
`0cb3f6bdebe4a593887cb19d2817ccb761731acc4ac92c68d5171e6e88b0cab1`.
Human review of that exact artifact rejected it: story 1.0, pacing 1.5, visual
finish 2.0, and audio finish 4.5; captions were not applicable, and no numeric
delivery-readiness score was supplied. The reviewer found no discernible arc,
near-random and incoherent sequencing, metronomic and sometimes rushed pacing,
unmotivated cuts, occasional fades despite the stated hard-cut intent, and no
meaningful contrast between the visual styles. Audio was consistent and
audible, but the instrumental did not fit the footage. M40 remains **in progress**
and this is one preflight sample, not the three-sample family or milestone gate.

The original rejection is preserved as the immutable baseline and is not
converted into a score that the reviewer did not provide: captions are N/A and
delivery readiness has no numeric rating. The v2 recovery passed 34/34 machine
assertions in one turn with 15 tool calls, 11 operations, and 318,225 total
tokens. Its 600-frame 1920x1080 MP4 measures -16.04 LUFS and -4.20 dBFS peak,
with SHA-256
`236200c27d57bedfd82ccb3a7aae1afde49b79a85dfbf60e0b58504c01c10d69`.
The recovery is recorded separately in
[`benchmarks/auto-edit/v5/music-montage-recovery-baseline.json`](../benchmarks/auto-edit/v5/music-montage-recovery-baseline.json),
and human review rejected that exact SHA-bound artifact: story 2.5, pacing 3.5,
visual finish 4.0, audio finish 2.0, delivery readiness 2.0, and captions N/A.
The story was "better, the bunny thing now feels very out of place". The main
issue here is that the whole just ends in the middle of a musical phrase. It
feels like we're ending in the middle of a longer video, not that the video has
one coherent arc. The v2 machine score did not catch that composition failure:
required asset/phase assertions could force a Bunny cameo, and music fit had no
source-end or encoded quiet-tail gate. Its source-scene confidence floor also
did not fully veto baked source cuts; an earlier machine-passing candidate was
discarded after independent review found a baked dissolve. Its acceptance target
was a readable contrast arc (held dramatic Sintel opening, contiguous Bunny
pivot, Sintel action return at the cue's major lift, and held finish), exact
scene-derived shot envelopes, nonuniform shot cadence, and structural musical
anchors that explain the major transitions. The executable manifest requires
acceptance for a scored artifact and a 4.0 minimum mean for every applicable
human-rating dimension; N/A dimensions are excluded from those means.

The first local v3 diagnostic passed its then-current 38/38 machine assertions
with 15 tool calls, 11 operations, and 333,560 total tokens. Its rendered SHA-256
was `23a416bddc2753833c16f6e61cf555b7b7ab33a2a9ec84076a48df71c311e472`.
Independent review withheld it from human scoring and publication because it
used only two disconnected Sintel clips totaling 140 frames against seven Tears
of Steel clips totaling 560 frames. It proved the endpoint, tail, scene, and
delivery fixes, but also proved that the old multi-source minimum still allowed
a decorative cameo. It is not a baseline and does not count toward the family
gate. The executable contract now requires three clips and 210 frames from each
source plus distinct early and late appearances.

Two fresh samples on the hardened contract now pass 40/40 machine assertions.
The first used four Sintel shots over 293 frames and five Tears of Steel shots
over 407 frames. It required 18 calls, including six montage-planner attempts,
and 432,226 tokens; its rendered SHA-256 is
`203461a7331ad0b7ed45654954b244b63c71dc6e3ca1fd11f0f3a562ef22dac4`.
That retry loop was a real token regression. `plan_beat_montage` now returns the
nearest globally feasible source- and cadence-valid schedule plus an exact retry
patch when the requested movement bound is infeasible. The following sample
needed three planner calls, 15 total calls, and 342,058 tokens, a 20.9% token
reduction while preserving the 40/40 result. It used four Sintel shots over 248
frames and five Tears of Steel shots over 452 frames; its rendered SHA-256 is
`4c802c58d2a056f87bc305e742db5bdb9fbfc0dfafa2a200604835ee3857daf6`.
Neither fresh artifact is an accepted baseline. Owner review rejected the
parallel-world premise itself and supplied no numeric ratings, so none are
invented. V4 replaces that premise rather than tightening another usage quota.

Independent frame review rejected one earlier machine-passing recovery because
its final shot began inside a baked source dissolve. That artifact is not the
published v2 recovery. The manually reviewed exclusion was widened and
clean-frame feasibility accounting was corrected to stop counting excluded
intervals before the v2 rerun. Uniform frames, every shot midpoint, and frames
on both sides of every cut in the published v2 recovery show no black, title,
logo, slate, or baked transition tail. V3 turns that audit lesson into a lower
confidence scene-boundary veto rather than relying on the audit alone.

## Commands

Prepare and verify the interview pack:

```powershell
& .\scripts\setup-ffmpeg.ps1
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --prepare-fixtures benchmarks/auto-edit/v5/fixture-pack.json
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --verify-fixtures benchmarks/auto-edit/v5/fixture-pack.json
```

Prepare and verify the event pack:

```powershell
& .\scripts\setup-ffmpeg.ps1
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --prepare-fixtures benchmarks/auto-edit/v5/event-fixture-pack.json
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --verify-fixtures benchmarks/auto-edit/v5/event-fixture-pack.json
```

Prepare and verify the active single-source trailer pack:

```powershell
& .\scripts\setup-ffmpeg.ps1
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --prepare-fixtures benchmarks/auto-edit/v5/music-fixture-pack-v4.json
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --verify-fixtures benchmarks/auto-edit/v5/music-fixture-pack-v4.json
```

Run the first task:

```powershell
& .\scripts\setup-ffmpeg.ps1
$env:KINEWRIGHT_EVAL = '1'
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --suite generalization-v5 `
  --harness codex `
  --only g1 `
  --samples 1
```

Run the event/multicam task:

```powershell
& .\scripts\setup-ffmpeg.ps1
$env:KINEWRIGHT_EVAL = '1'
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --suite generalization-v5 `
  --harness codex `
  --only g2 `
  --samples 1
```

Run the music-montage task:

```powershell
& .\scripts\setup-ffmpeg.ps1
$env:KINEWRIGHT_EVAL = '1'
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --suite generalization-v5 `
  --harness codex `
  --only g3 `
  --samples 1
```

Rerender an exact saved edit after a renderer or delivery fix, without another
model session:

```powershell
& .\scripts\setup-ffmpeg.ps1
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --rerender-document target/evals/RUN/artifacts/g2-sample-1/final-document.json `
  --artifact-directory target/evals/RUN-rerender/artifacts/g2-sample-1 `
  --delivery-profile vertical_short `
  --loudness-contract '-1800,-1400,-100'
```

## Exit gate

M40 passes only when all three families are checked in and executable, each
passes 3/3 model samples, at least two outputs per family are accepted by a
person, the mean human rating is at least 4.0/5, and no material caption error
survives. One successful interview does not satisfy the milestone.

Music montage has one rejected historical preflight and one 34/34 machine-passing
v2 recovery rejected in human review. One v3 machine diagnostic was withheld
after independent review exposed source-cameo behavior, so it does not count.
Two fresh hardened-v3 samples passed 40/40, but owner review rejected the
multi-source premise. The first v4 single-source trailer sample now passes
37/37 with 12 calls, 221,521 tokens, five of seven cuts on structural anchors,
-15.99 LUFS audio, and a one-frame music-end offset. Independent frame and cut
inspection passed. Human review is pending, so the family still has no accepted
single-source trailer sample.
Event/multicam still needs two more machine samples, one more human-accepted
output, and numeric ratings sufficient to evaluate the 4.0 mean-rating gate.
