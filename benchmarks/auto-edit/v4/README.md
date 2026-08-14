# OpenReel Dialogue-Pacing Benchmark v4

V4 targets the one material weakness left in the accepted M38 cut: sentence
rhythm. The M38 output scored 4/5 for pacing and contained project-frame gaps
of 12, 7, 12, and 12 at detected sentence boundaries. The 7-frame transition
was valid by the old cleanup contract, but it made the finished narration feel
less consistent.

The fixture and editorial truth are unchanged from v3. Keeping that successful
artifact contract stable isolates the pacing change. V4 adds:

- exact pause retention across consecutive removed filler words;
- a compact `get_dialogue_pacing` inspector that reports sentence boundaries,
  acoustic pauses, transcript fallbacks, reasons, and short/target/long status;
- an evaluator-owned assertion requiring every detected acoustic sentence
  pause to land between 10 and 40 project frames;
- a higher human gate: pacing at least 4.5/5, overall mean at least 4.0/5, and
  every other dimension at least 3.5/5.

The model is asked for a 12-source-frame bridge where fillers are removed. The
planner removes the whole filler run atomically and divides the retained pause
across its two clean speech boundaries without preserving filler audio. The
independent evaluator measures mapped acoustic silence, not planner claims.

Run three subscription-backed samples with:

```powershell
& .\scripts\setup-ffmpeg.ps1
$env:OPENREEL_EVAL = '1'
cargo run -p openreel-agent --bin openreel-eval -- `
  --suite dialogue-pacing-v4 `
  --harness codex `
  --only f3 `
  --samples 3
```

The published Codex machine baseline on revision `6e22f3a` passed all three
samples and all 99 assertions under the original transcript-only metric. Every
sample produced the same 607-frame MP4 and a 4.77% independent rendered-dialogue
word error rate. Samples used 16-17 tool calls and 242,265-258,873 reported
tokens, averaging 249,112. Human timestamp review later showed that the four
reported 12-frame gaps were not acoustic measurements: the opening rendered at
roughly 47 frames followed by roughly 9, while the later pauses felt natural.
That baseline remains in [`baseline.json`](baseline.json) as historical and
token-regression evidence, but it no longer satisfies the corrected pacing
contract. A fresh benchmark run is required.

The checked-in [regression evidence](token-regression.json) and
[portable technical report](../../../docs/reports/m39-token-regression/report.html)
preserve the pre-fix run, post-fix run, root cause, and limitations.

Each output remains SHA-bound to separate human review. The milestone passes
only when all three samples pass the machine contract, at least two are
accepted by a person, mean human rating is at least 4.0, pacing is at least
4.5, no other dimension falls below 3.5, and no material caption error or
audible filler survives.

Qualitative review called the pacing a major improvement and identified the two
opening defects above, but no formal v4 rubric has been submitted. A fresh
machine run and SHA-bound human review are required. A machine pass alone does
not complete M39.

This is still a synthetic motion-graphic fixture. It can isolate dialogue
editing and narrative correctness, but it cannot establish professional shot
taste or photoreal finishing.
