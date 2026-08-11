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
- Result artifact: `target/evals\openreel-eval-20260811-010523-claude-code.jsonl`

| Eval | Pass | Assertions | Turns | Tools | Tokens | USD | Wall | Ops |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| e1 split-and-delete | PASS | 13/13 | 1 | 3 | 404 | $0.1650 | 11.9s | 2 |
| e2 silence-gap removal | FAIL | 14/15 | 1 | 6 | 1158 | $0.2390 | 23.1s | 3 |
| e3 filler-word removal | PASS | 14/14 | 1 | 4 | 733 | $0.2182 | 22.6s | 4 |
| e4 scene-cut | PASS | 13/13 | 1 | 5 | 641 | $0.2077 | 18.9s | 2 |
| e5 effect-and-transition | PASS | 15/15 | 1 | 3 | 407 | $0.1598 | 13.3s | 2 |
| e6 ordinal-resolution stress | PASS | 14/14 | 1 | 3 | 599 | $0.1799 | 17.9s | 4 |
| e7 flagship rough cut | FAIL | 22/24 | 1 | 11 | 2865 | $0.3941 | 44.3s | 6 |
| **TOTAL** | **FAIL** | **105/108** | **7** | **35** | **6807** | **$1.5636** | **152.5s** | **23** |

### Failures

- `e2 silence-gap removal`: duration bounds (expected 15..=38 frames, observed 39)
- `e7 flagship rough cut`: words retained (pre-edit set="take-A-content" expected={"aurora", "copper", "crew", "guide", "lantern", "morning", "opens", "story", "the"}, present after edit={"aurora", "copper", "crew", "guide", "lantern", "morning", "opens", "the"}); words retained (pre-edit set="take-D-content" expected={"beacons", "closes", "crew", "delta", "home", "journey", "silver", "the", "welcome"}, present after edit={"beacons", "closes", "crew", "delta", "home", "silver", "the", "welcome"})
