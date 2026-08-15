# M40 - Generalization Gauntlet

## Outcome

M40 moves OpenReel's quality claim from one tuned synthetic story to unfamiliar,
licensed footage across three edit families:

1. interview/documentary;
2. event/multicam;
3. music montage.

The milestone is benchmark-led. A new primitive ships only when a task exposes
the need and can score the result. Machine checks own exact facts, timing,
conformance, and artifact identity. A person still owns taste and acceptance.

M40 is **in progress**. Interview/documentary and event/multicam are executable.
Music montage remains required before the milestone can pass.

## Phase 1 - licensed fixture packs

Synthetic media could be generated inside a fixture function. Real footage
needs a reproducible boundary of its own. `openreel-agent::fixture_pack`
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
- `OPENREEL_EVAL_FIXTURE_DIR` for a shared or custom cache.

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
- one reframe effect per shot, keyframe count, face-safe bounds, and maximum jumps;
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
retaining -16.98 LUFS and -1.72 dBFS. Human acceptance of this recovery
artifact is pending, so it is not promoted to a checked-in baseline.

The next official sample also uses the crop's actual geometric travel range
(25-75% horizontally) instead of the overly conservative 45-55% clamp. That
lets the tracker follow Laura while keeping the supplied 25%-wide subject box
inside the 9:16 crop.

Readiness also gained an explicit `check_silence` policy. Event work that must
preserve continuous program audio can skip irrelevant dead-air analysis while
still running technical QA, delivery conformance, and the real storyboard.
The final trace ended with editorial readiness `true`, proving that readiness
also lacked the audible-delivery signal. One passing machine sample is not the
three-sample family gate, and this one is human-rejected. The machine result,
review outcome, and immutable hashes are recorded in
`benchmarks/auto-edit/v5/event-multicam-baseline.json`.

## Commands

Prepare and verify the interview pack:

```powershell
& .\scripts\setup-ffmpeg.ps1
cargo run -p openreel-agent --bin openreel-eval -- `
  --prepare-fixtures benchmarks/auto-edit/v5/fixture-pack.json
cargo run -p openreel-agent --bin openreel-eval -- `
  --verify-fixtures benchmarks/auto-edit/v5/fixture-pack.json
```

Prepare and verify the event pack:

```powershell
& .\scripts\setup-ffmpeg.ps1
cargo run -p openreel-agent --bin openreel-eval -- `
  --prepare-fixtures benchmarks/auto-edit/v5/event-fixture-pack.json
cargo run -p openreel-agent --bin openreel-eval -- `
  --verify-fixtures benchmarks/auto-edit/v5/event-fixture-pack.json
```

Run the first task:

```powershell
& .\scripts\setup-ffmpeg.ps1
$env:OPENREEL_EVAL = '1'
cargo run -p openreel-agent --bin openreel-eval -- `
  --suite generalization-v5 `
  --harness codex `
  --only g1 `
  --samples 1
```

Run the event/multicam task:

```powershell
& .\scripts\setup-ffmpeg.ps1
$env:OPENREEL_EVAL = '1'
cargo run -p openreel-agent --bin openreel-eval -- `
  --suite generalization-v5 `
  --harness codex `
  --only g2 `
  --samples 1
```

Rerender an exact saved edit after a renderer or delivery fix, without another
model session:

```powershell
& .\scripts\setup-ffmpeg.ps1
cargo run -p openreel-agent --bin openreel-eval -- `
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

The next family implementation target is music montage. Event/multicam still
needs two more machine samples and at least two human-accepted outputs before
its family gate can pass.
