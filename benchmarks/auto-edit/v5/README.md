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

V5 is intentionally marked `in_progress`. M40 does not pass on one interview.
It exits only after event/multicam and music-montage tasks are also executable,
three samples per family pass the machine contract, and the published human
gate in `manifest.json` passes.
