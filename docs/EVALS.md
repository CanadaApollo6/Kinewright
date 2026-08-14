# Agent evals

OpenReel's Arc 2 editing competence suite runs only when `OPENREEL_EVAL=1` is explicitly set. It uses generated media, the real MCP server, and an installed subscription harness. CI covers the framework with a fake driver and spends nothing.

## Run

```powershell
$env:OPENREEL_EVAL = '1'
cargo run -p openreel-agent --bin openreel-eval
# Optional: -- --harness codex
```

Results are written as timestamped, environment-stamped JSONL under `target/evals/`. A full live suite is intentionally expensive and must not be placed in CI.

The exact-operation contract and first machine-readable baseline live under
[`benchmarks/auto-edit/v1`](../benchmarks/auto-edit/v1/README.md). The baseline
preserves the real 6/7 task result and leaves human first-pass acceptance unset.

The finished-cut contract lives under
[`benchmarks/auto-edit/v2`](../benchmarks/auto-edit/v2/README.md). It runs one
full prompt-to-MP4 task and writes a machine report, immutable document, proof
sheet, probed and hashed MP4, and a separate pending human-review record. Run it
with:

```powershell
$env:OPENREEL_EVAL = '1'
cargo run -p openreel-agent --bin openreel-eval -- `
  --suite finished-cut-v2 `
  --harness codex
```

Human review is scored without running another agent:

```powershell
cargo run -p openreel-agent --bin openreel-eval -- `
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
$env:OPENREEL_EVAL = '1'
cargo run -p openreel-agent --bin openreel-eval -- `
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
$env:OPENREEL_EVAL = '1'
cargo run -p openreel-agent --bin openreel-eval -- `
  --suite dialogue-pacing-v4 `
  --harness codex `
  --only f3 `
  --samples 3
```

Set `OPENREEL_EVAL_TRACE=1` for a bounded stderr trace of agent text, tool
arguments, and tool results while diagnosing a model loop. The v4 harness
fails fast after 24 tool calls or 350,000 reported tokens; the accepted
historical path needed 16-17 calls and averaged 249,112 tokens.

M39 requires a 3/3 machine pass before human review. Its human gate is at
least two accepted SHA-bound artifacts, a 4.0/5 overall mean, 4.5/5 pacing,
3.5/5 in every other dimension, and zero audible fillers or material caption
errors. The historical Codex machine baseline passed 3/3 samples and 99/99
assertions under the superseded transcript-only pacing metric. All three
artifacts are byte-identical and 607 frames long. A post-review acoustic audit
found that the reported four 12-frame gaps were actually about 47, 9, 31, and
13 frames; the reviewer called out only the first two as pacing defects.
Samples used
16-17 tool calls and 242,265-258,873 reported tokens, averaging 249,112. A
protocol fix removed redundant capability discovery and cut mean usage 11.9%
from the first v4 run without changing the artifact. The historical record
lives in
[`benchmarks/auto-edit/v4/baseline.json`](../benchmarks/auto-edit/v4/baseline.json).
Qualitative review called the pacing a major improvement and identified the two
opening defects, but no formal v4 rubric has been submitted. A fresh corrected
run and SHA-bound review are required, so M39 is not complete.

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
