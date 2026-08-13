# Agent evals

OpenReel's Arc 2 editing competence suite runs only when `OPENREEL_EVAL=1` is explicitly set. It uses generated media, the real MCP server, and an installed subscription harness. CI covers the framework with a fake driver and spends nothing.

## Run

```powershell
$env:OPENREEL_EVAL = '1'
cargo run -p openreel-agent --bin openreel-eval
# Optional: -- --harness codex
```

Results are written as timestamped, environment-stamped JSONL under `target/evals/`. A full live suite is intentionally expensive and must not be placed in CI.

The versioned public contract and first machine-readable baseline live under [`benchmarks/auto-edit/v1`](../benchmarks/auto-edit/v1/README.md). The baseline preserves the real 6/7 task result and leaves human first-pass acceptance unset.

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
