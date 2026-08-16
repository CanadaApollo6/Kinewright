# OpenReel Generalization Gauntlet v5

V5 is the M40 benchmark. It stops optimizing only for OpenReel's synthetic
garden story and measures unfamiliar, licensed footage in three distinct edit
families: interview/documentary, event/multicam, and music montage.

Two families are executable. `g1` is a real public-domain interview with
filmmaker Helen Hill. `g2` is a CC BY 4.0 AMI meeting with four synchronized
participant cameras, a program headset mix, and pinned manual speaker labels.
Both use real low-resolution footage and independently probed MP4 delivery,
not generated bars or motion graphics.

## Immutable fixture acquisition

Downloaded footage is not committed. Its source page, license, byte count,
SHA-256, and exact URL live in `fixture-pack.json` and
`event-fixture-pack.json`. Acquisition is an explicit network action:

```powershell
& .\scripts\setup-ffmpeg.ps1
cargo run -p openreel-agent --bin openreel-eval -- `
  --prepare-fixtures benchmarks/auto-edit/v5/fixture-pack.json
cargo run -p openreel-agent --bin openreel-eval -- `
  --prepare-fixtures benchmarks/auto-edit/v5/event-fixture-pack.json
```

Verify the local pack without network access:

```powershell
& .\scripts\setup-ffmpeg.ps1
cargo run -p openreel-agent --bin openreel-eval -- `
  --verify-fixtures benchmarks/auto-edit/v5/fixture-pack.json
cargo run -p openreel-agent --bin openreel-eval -- `
  --verify-fixtures benchmarks/auto-edit/v5/event-fixture-pack.json
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

V5 is intentionally marked `in_progress`. M40 does not pass on one interview
and one event sample. It exits only after music montage is executable, three
samples per family pass the machine contract, and the published human gate in
`manifest.json` passes.
