# OpenReel Finished-Cut Benchmark v2

V1 proves that an agent can make exact edits. V2 measures the whole first-cut
loop: generated footage enters an isolated OpenReel runtime, an installed model
edits it through the public MCP contract, and the benchmark renders the exact
result as a reviewable vertical MP4.

The two score layers are deliberately separate:

- **Machine score:** brief compliance, retained dialogue, removed filler and
  dead air, caption automation, tool and cost budgets, QA, delivery
  conformance, proof rendering, MP4 creation, SHA-256, and an independent media
  probe.
- **Human score:** first-pass acceptance plus six 1–5 ratings for story,
  pacing, visual finish, audio finish, captions, and delivery readiness.

Token and wall-time ceilings remain hard gates. USD is recorded when a harness
reports it, but v2 does not fail a subscription harness merely because it does
not expose an attributable per-turn price.

A machine pass is not called a good edit. Human acceptance stays `null` until a
person watches the artifact and records a decision.

## Run

From a PowerShell where `scripts/setup-ffmpeg.ps1` has activated the pinned
native build environment:

```powershell
$env:OPENREEL_EVAL = '1'
cargo run -p openreel-agent --bin openreel-eval -- `
  --suite finished-cut-v2 `
  --harness codex
```

The run package is written beneath `target/evals/openreel-eval-*`. It contains
the raw JSONL results, machine report, final document, proof sheet, MP4, and an
unscored `human-review.json` template.

After watching `finished.mp4`, fill every rating and set `accepted` for the
task. Then validate and score it without spending model quota:

```powershell
cargo run -p openreel-agent --bin openreel-eval -- `
  --score-review target/evals/<run>/human-review.json
```

That writes `human-score.json`. Partial or out-of-range reviews fail instead of
silently entering the public metrics.

The versioned contract is [manifest.json](manifest.json). Generated run media
stays outside Git. The first reconciled machine run is published in
[baseline.json](baseline.json); it records the artifact SHA-256 while leaving
human acceptance unset. Its proof-sheet observation about vertical caption
width remains visible instead of being turned into an unearned acceptance.
