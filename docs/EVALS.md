# Agent evals

Kinewright's Arc 2 editing competence suite runs only when `KINEWRIGHT_EVAL=1` is explicitly set. It uses generated media, the real MCP server, and an installed subscription harness. CI covers the framework with a fake driver and spends nothing.

## Run

```powershell
$env:KINEWRIGHT_EVAL = '1'
cargo run -p kinewright-agent --bin kinewright-eval
# Optional: -- --harness codex
```

Results are written as timestamped, environment-stamped JSONL under `target/evals/`. A full live suite is intentionally expensive and must not be placed in CI.

To reclaim space from failed or abandoned runs, use the guarded cleanup script:

```powershell
& .\scripts\clean-eval-runs.ps1 -WhatIf
& .\scripts\clean-eval-runs.ps1
```

It only considers direct `kinewright-eval-*` directories under `target/evals/`.
Machine-passing runs, completed human-reviewed runs (accepted or rejected),
unrecognized directories, unreadable review artifacts, and recent incomplete or
pending-review runs are preserved. The default 24-hour cutoff applies before an
old incomplete or pending run can be removed; use
`-IncompleteMinimumAgeHours` to change that cutoff. The script refuses
reparse-point roots or descendants and supports `-WhatIf` for a dry run.

The exact-operation contract and first machine-readable baseline live under
[`benchmarks/auto-edit/v1`](../benchmarks/auto-edit/v1/README.md). The baseline
preserves the real 6/7 task result and leaves human first-pass acceptance unset.

The finished-cut contract lives under
[`benchmarks/auto-edit/v2`](../benchmarks/auto-edit/v2/README.md). It runs one
full prompt-to-MP4 task and writes a machine report, immutable document, proof
sheet, probed and hashed MP4, and a separate pending human-review record. Run it
with:

```powershell
$env:KINEWRIGHT_EVAL = '1'
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --suite finished-cut-v2 `
  --harness codex
```

Human review is scored without running another agent:

```powershell
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --score-review target/evals/<run>/human-review.json
```

The current v2 Codex baseline passed 32/32 machine assertions in 108.126 seconds
with 18 tool calls and 234,924 reported tokens. It produced an independently
probed 421-frame 1080x1920 audio-video MP4. This is 67.9% fewer tokens than the
first passing v2 sample. The SHA-bound human review rejected it with a 2.25/5
mean after finding retained audible fillers, inaccurate captions, awkward cuts,
unclear story assembly, and no visual narrative in the synthetic fixture.

The editorial-truth contract lives under
[`benchmarks/auto-edit/v3`](../benchmarks/auto-edit/v3/README.md). It replaces
the rejected fixture with one coherent five-take story and scores take order,
recognized filler removal, exact authored captions, and a fresh transcription
of the finished MP4. The rendered dialogue must remain within a 15% ordered
word error rate. Run three independent samples with:

```powershell
& .\scripts\setup-ffmpeg.ps1
$env:KINEWRIGHT_EVAL = '1'
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --suite editorial-cut-v3 `
  --harness codex `
  --only f2 `
  --samples 3
```

The current v3 Codex machine baseline passed all three samples and all 93
assertions. Samples used 16-17 tool calls and 236,336-251,345 reported tokens,
and all three produced the same 602-frame 1080x1920 MP4 at 2.39% independently
measured word error rate. The exact record lives in
[`benchmarks/auto-edit/v3/baseline.json`](../benchmarks/auto-edit/v3/baseline.json).
Human review accepted the byte-identical output at a 4.08/5 mean with no
dimension below 3.5, no audible filler, and no material caption error. One
viewing was applied to all three SHA-bound rows because their artifact hashes
were identical. M38 passes its full machine-and-human exit contract.

The dialogue-pacing contract lives under
[`benchmarks/auto-edit/v4`](../benchmarks/auto-edit/v4/README.md). It preserves
the accepted v3 story and output assertions, then independently requires every
detected acoustic sentence pause to land between 10 and 40 project frames. The
agent can cap excessive pause across removed filler runs and inspect the final
rhythm without receiving the evaluator's result. Transcript timing is used
only as an explicit fallback while acoustic analysis is unavailable. Run it
with:

```powershell
& .\scripts\setup-ffmpeg.ps1
$env:KINEWRIGHT_EVAL = '1'
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --suite dialogue-pacing-v4 `
  --harness codex `
  --only f3 `
  --samples 3
```

Set `KINEWRIGHT_EVAL_TRACE=1` for a bounded stderr trace of agent text, tool
arguments, and tool results while diagnosing a model loop. The v4 harness
fails fast after 24 tool calls or 350,000 reported tokens; the current
machine path uses 8 calls and averages 108,296 tokens.

M39 requires a 3/3 machine pass before human review. Its human gate is at
least two accepted SHA-bound artifacts, a 4.0/5 overall mean, 4.5/5 pacing,
3.5/5 in every other dimension, and zero audible fillers or material caption
errors. The corrected Codex machine baseline passes 3/3 samples and 102/102
assertions. All samples produce 585-frame cuts with acoustic sentence gaps of
33, 15, 23, and 16 frames, no cuttable silence, exact authored captions, and a
4.77% rendered word error rate. Each uses exactly 8 tool calls; reported
tokens range from 107,900 to 108,597 and average 108,296. Mean usage is 56.5%
below the superseded 249,112-token baseline after deterministic planner handles,
a shared pacing/readiness invariant, and deterministic sentence-coherent
caption grouping removed the model repair loop. All three samples produced one
byte-identical artifact recorded in
[`benchmarks/auto-edit/v4/baseline.json`](../benchmarks/auto-edit/v4/baseline.json).
It still requires a formal SHA-bound human rubric, so M39 is not complete.

The in-progress real-footage generalization contract lives under
[`benchmarks/auto-edit/v5`](../benchmarks/auto-edit/v5/README.md). Its fixture
pack records exact download URLs, source pages, licenses, lengths, and SHA-256
identities. Downloads are explicit; verification and benchmark runs are
offline. Prepare and verify the first public-domain interview pack with:

```powershell
& .\scripts\setup-ffmpeg.ps1
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --prepare-fixtures benchmarks/auto-edit/v5/fixture-pack.json
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --verify-fixtures benchmarks/auto-edit/v5/fixture-pack.json
```

Task `g1` asks the agent to isolate and finish one coherent Hurricane Katrina
film-recovery story from a naturally recorded two-minute interview. It is the
first non-synthetic fixture in the published benchmark. M40 remains incomplete
until interview/documentary, event/multicam, and music-montage families each
pass three model samples and their separate human gate.

The recorded `g3` recovery baseline below is the separately pinned v2 music
pack. The active recovery is v4: a 22-second, single-source Tears of Steel
trailer cut to Vanguard's authored final tag. Prepare or verify its inputs
explicitly before the offline run:

```powershell
& .\scripts\setup-ffmpeg.ps1
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --prepare-fixtures benchmarks/auto-edit/v5/music-fixture-pack-v4.json
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --verify-fixtures benchmarks/auto-edit/v5/music-fixture-pack-v4.json
```

The rejected v1 Cipher artifact remains published as historical evidence. V2
uses Scott Buckley's CC BY 4.0 "Uprising," exact 24-second track coverage,
bounded inspectable beat-anchor repair, source-phase arc gates, and negative
checks for transitions, effects, fades, retiming, and periodic A/B cadence.
The first v2 recovery is published in
`benchmarks/auto-edit/v5/music-montage-recovery-baseline.json`: 34/34 machine
assertions, 15 tool calls, 318,225 total tokens, and an independently audited
24-second MP4. Human review rejected that exact SHA-bound artifact: story 2.5,
pacing 3.5, visual finish 4.0, audio finish 2.0, delivery readiness 2.0, and
captions were not applicable. The story was "better, the bunny thing now feels
very out of place". The main issue here is that the whole just ends in the
middle of a musical phrase. It feels like we're ending in the middle of a longer
video, not that the video has one coherent arc.

The v2 machine contract also had three important blind spots. Its required
asset/phase assertions could force an isolated Big Buck Bunny cameo without
testing whether the cameo belonged in the story. Its source-scene confidence
floor did not fully veto baked source cuts; an earlier machine-passing candidate
was discarded only after independent cut-boundary review found a baked dissolve.
And music fit checked the start and duration, not the musical source endpoint or
the delivered encoded tail. The published v2 artifact therefore remained a
machine pass and a human rejection.

The hardened v3 recovery addressed those gaps with a compatible Tears of Steel
source, a minimum of three clips and 210 project frames per visual asset,
distinct early and late appearances for both sources, a 10% scene-boundary veto
floor, actual first/last shot-hold assertions, exact end-anchored music fit, and
encoded quiet-tail verification. One local diagnostic passed its then-current
38/38 machine assertions with 15 tool calls, 11 operations, and 333,560 total
tokens, but independent review withheld it: Sintel appeared in only two
disconnected clips totaling 140 frames against seven Tears of Steel clips and
560 frames. It is not a published baseline and does not count toward the family
gate. No v3 human acceptance existed at that point.

Two fresh samples on the hardened contract now pass 40/40. The first used 18
calls, six montage-planner attempts, and 432,226 tokens. Kinewright now returns
the nearest globally feasible source- and cadence-valid anchor schedule as an
exact retry patch when a bounded montage request fails. The next sample used 15
calls, three planner attempts, and 342,058 tokens, reducing tokens by 20.9%.
Their SHA-256 values are
`203461a7331ad0b7ed45654954b244b63c71dc6e3ca1fd11f0f3a562ef22dac4` and
`4c802c58d2a056f87bc305e742db5bdb9fbfc0dfafa2a200604835ee3857daf6`.
Neither is an accepted baseline. Owner review rejected the parallel-world
premise: one music track made the disconnected footage read as one incoherent
story, neither source felt essential, and the ending still sounded mid-phrase.
No numeric scores were supplied for that review. V4 removes the forced second
source and uses a trailer cue with a distinct authored ending. Its fixture test
proves the 22-second eight-shot contract is source-, beat-, cadence-, and
endpoint-feasible before a model run.

The first v4 model sample now passes 37/37 with 12 calls, 10 operations, and
221,521 tokens. It uses eight scene-clean Tears of Steel shots, aligns five of
seven cuts to structural candidates, measures -15.99 LUFS, and ends within one
source frame of the reviewed Vanguard endpoint. Its SHA-256 is
`9b813c6f6888e36e90ba3b2f5ad0938f8d3827374a2465161c3992aa40a8d99a`.
Independent cut-neighborhood inspection passed; human review remains pending.

The first corrected interview preflight is published at
`benchmarks/auto-edit/v5/baseline.json`: 1/1 sample, 25/25 assertions, 7 tool
calls, 108,701 total tokens, and 9.20% independent rendered-speech WER. Human
review rejected it: story 5.0, pacing 5.0, visual finish 3.0, audio finish 5.0,
captions 2.5, and delivery readiness 3.0. The caption failure now has hard
regressions for text intent, semantic grouping, subject-safe presentation, and
caption agreement with independently transcribed rendered speech. This single
run does not satisfy the family gate.

## Seed suite

| Eval | Rationale | USD ceiling |
|---|---|---:|
| e1 split-and-delete | Measures the original M3 compound edit with exact source-range semantics. | $2.00 |
| e2 silence-gap removal | Measures analysis-led dead-air removal without coupling success to Whisper spelling. | $2.00 |
| e3 filler-word removal | Measures transcript-driven deletion while retaining every non-filler word heard pre-edit. | $2.00 |
| e4 scene-cut | Measures scene analysis and exact splitting without prescribing a plan shape. | $2.00 |
| e5 effect-and-transition | Measures non-destructive effect and transition orchestration on an ordinal target. | $2.00 |
| e6 ordinal-resolution stress | Catches the M3 trap where an early split renumbers later ordinal targets. | $2.00 |
| e7 flagship rough cut | Measures deterministic rough-cut assembly, cleanup, ordering, and reversibility from an empty timeline. | $2.50 |

## Baseline snapshot

This is the latest complete live run. Assertion failures remain part of the measured baseline; they are not rewritten as fixture success.

- Date: `2026-08-11`
- Harness: `claude-code`
- Harness version: `2.1.227 (Claude Code)`
- Model: `harness-default`
- Platform: `windows-x86_64`
- Result artifact: `target/evals\openreel-eval-20260811-145547-claude-code.jsonl`

| Eval | Pass | Assertions | Turns | Tools | Tokens | USD | Wall | Ops |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| e1 split-and-delete | PASS | 13/13 | 1 | 3 | 416 | $0.3260 | 13.9s | 2 |
| e2 silence-gap removal | PASS | 15/15 | 1 | 5 | 1105 | $0.4443 | 25.9s | 3 |
| e3 filler-word removal | PASS | 14/14 | 1 | 4 | 874 | $0.3721 | 24.2s | 3 |
| e4 scene-cut | PASS | 13/13 | 1 | 4 | 660 | $0.3479 | 16.1s | 2 |
| e5 effect-and-transition | PASS | 15/15 | 1 | 2 | 337 | $0.2460 | 12.2s | 2 |
| e6 ordinal-resolution stress | PASS | 14/14 | 1 | 3 | 608 | $0.3509 | 17.7s | 3 |
| e7 flagship rough cut | FAIL | 22/24 | 1 | 9 | 2810 | $0.5943 | 45.8s | 7 |
| **TOTAL** | **FAIL** | **106/108** | **7** | **30** | **6810** | **$2.6815** | **156.0s** | **22** |

### Failures

- `e7 flagship rough cut`: long silence absent (observed 3 cuttable silence spans from raw spans at least 20 source frames); duration bounds (expected 178..=445 frames, observed 459)
