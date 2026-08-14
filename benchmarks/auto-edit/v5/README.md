# OpenReel Generalization Gauntlet v5

V5 is the M40 benchmark. It stops optimizing only for OpenReel's synthetic
garden story and measures unfamiliar, licensed footage in three distinct edit
families: interview/documentary, event/multicam, and music montage.

The first executable family is `g1`, a real public-domain interview with
filmmaker Helen Hill. The agent must isolate her Hurricane Katrina film-
recovery answer from a two-minute source, clean natural dead air, preserve the
story, generate source-faithful captions, and deliver a vertical MP4. This is
real low-resolution talking-head footage with two speakers and imperfect ASR,
not generated bars or motion graphics.

## Immutable fixture acquisition

Downloaded footage is not committed. Its source page, license, byte count,
SHA-256, and exact transcode URL live in `fixture-pack.json`. Acquisition is an
explicit network action:

```powershell
& .\scripts\setup-ffmpeg.ps1
cargo run -p openreel-agent --bin openreel-eval -- `
  --prepare-fixtures benchmarks/auto-edit/v5/fixture-pack.json
```

Verify the local pack without network access:

```powershell
& .\scripts\setup-ffmpeg.ps1
cargo run -p openreel-agent --bin openreel-eval -- `
  --verify-fixtures benchmarks/auto-edit/v5/fixture-pack.json
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

Human review is pending. The source is a low-resolution 3:2 close-up, so the
vertical crop is necessarily tight; machine success does not assert that its
framing, pacing, or finish are publishable.

V5 is intentionally marked `in_progress`. M40 does not pass on one interview.
It exits only after event/multicam and music-montage tasks are also executable,
three samples per family pass the machine contract, and the published human
gate in `manifest.json` passes.
