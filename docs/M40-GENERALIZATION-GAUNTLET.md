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

M40 is **in progress**. The first family is executable; the other two remain
required before the milestone can pass.

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

## Commands

Prepare and verify the pack:

```powershell
& .\scripts\setup-ffmpeg.ps1
cargo run -p openreel-agent --bin openreel-eval -- `
  --prepare-fixtures benchmarks/auto-edit/v5/fixture-pack.json
cargo run -p openreel-agent --bin openreel-eval -- `
  --verify-fixtures benchmarks/auto-edit/v5/fixture-pack.json
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

## Exit gate

M40 passes only when all three families are checked in and executable, each
passes 3/3 model samples, at least two outputs per family are accepted by a
person, the mean human rating is at least 4.0/5, and no material caption error
survives. One successful interview does not satisfy the milestone.

The next implementation target is event/multicam. Its fixture must make sync,
speaker choice, audio continuity, and reframe stability independently
measurable before the first model run.
