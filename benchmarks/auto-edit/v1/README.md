# OpenReel Auto-Edit Benchmark v1

This benchmark measures whether an installed agent harness can inspect generated footage, apply exact OpenReel operations, satisfy structural and semantic edit assertions, and restore the original timeline through undo.

The checked-in [manifest](manifest.json) is the versioned suite contract. The checked-in [baseline](baseline.json) records the first full run without converting real failures into passes. `human_first_pass_acceptance` remains `null` until a person actually scores it.

Run the complete suite from the repository root:

```powershell
$env:OPENREEL_EVAL = '1'
cargo run -p openreel-agent --bin openreel-eval -- --harness claude-code
```

Use `codex` or `cursor` for the other supported harnesses. A run writes an environment-stamped JSONL trace under `target/evals/`; that raw generated artifact stays out of Git. A complete one-sample run also refreshes `docs/EVALS.md`.

The fixtures use pinned local FFmpeg generation plus Windows SAPI speech created from checked-in text. They require no private footage and no evaluation corpus download. Subscription-backed runs spend real quota and are never part of default CI.
