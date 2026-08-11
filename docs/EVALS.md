# Agent evals

OpenReel's Arc 2 editing competence suite runs only when `OPENREEL_EVAL=1` is explicitly set. It uses generated media, the real MCP server, and an installed subscription harness. CI covers the framework with a fake driver and spends nothing.

## Run

```powershell
$env:OPENREEL_EVAL = '1'
cargo run -p openreel-agent --bin openreel-eval
# Optional: -- --harness codex
```

Results are written as timestamped, environment-stamped JSONL under `target/evals/`. A full live suite is intentionally expensive and must not be placed in CI.

## Seed suite

| Eval | Rationale | USD ceiling |
|---|---|---:|
| e1 split-and-delete | Measures the original M3 compound edit with exact source-range semantics. | $0.75 |
| e2 silence-gap removal | Measures analysis-led dead-air removal without coupling success to Whisper spelling. | $0.75 |
| e3 filler-word removal | Measures transcript-driven deletion while retaining every non-filler word heard pre-edit. | $0.75 |
| e4 scene-cut | Measures scene analysis and exact splitting without prescribing a plan shape. | $0.75 |
| e5 effect-and-transition | Measures non-destructive effect and transition orchestration on an ordinal target. | $0.75 |
| e6 ordinal-resolution stress | Catches the M3 trap where an early split renumbers later ordinal targets. | $0.75 |
| e7 flagship rough cut | Measures deterministic rough-cut assembly, cleanup, ordering, and reversibility from an empty timeline. | $1.50 |

## Baseline snapshot

This is the latest complete live run. Assertion failures remain part of the measured baseline; they are not rewritten as fixture success.

- Date: `2026-08-11`
- Harness: `claude-code`
- Harness version: `2.1.227 (Claude Code)`
- Model: `harness-default`
- Platform: `windows-x86_64`
- Result artifact: `target/evals\openreel-eval-20260811-020441-claude-code.jsonl`

| Eval | Pass | Assertions | Turns | Tools | Tokens | USD | Wall | Ops |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| e1 split-and-delete | PASS | 13/13 | 1 | 3 | 434 | $0.6946 | 12.7s | 2 |
| e2 silence-gap removal | PASS | 15/15 | 1 | 6 | 1855 | $0.2840 | 32.9s | 3 |
| e3 filler-word removal | PASS | 14/14 | 1 | 4 | 836 | $0.2254 | 23.1s | 4 |
| e4 scene-cut | PASS | 13/13 | 1 | 4 | 683 | $0.2088 | 16.8s | 2 |
| e5 effect-and-transition | PASS | 15/15 | 1 | 3 | 406 | $0.1601 | 23.4s | 2 |
| e6 ordinal-resolution stress | PASS | 14/14 | 1 | 3 | 614 | $0.1809 | 16.2s | 4 |
| e7 flagship rough cut | FAIL | 22/24 | 1 | 11 | 4894 | $0.5263 | 73.3s | 7 |
| **TOTAL** | **FAIL** | **106/108** | **7** | **34** | **9722** | **$2.2802** | **198.8s** | **24** |

### Failures

- `e7 flagship rough cut`: long silence absent (observed 3 silence spans at least 20 source frames); duration bounds (expected 162..=405 frames, observed 437)
