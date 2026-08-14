# OpenReel Dialogue-Pacing Benchmark v4

V4 targets the one material weakness left in the accepted M38 cut: sentence
rhythm. The M38 output scored 4/5 for pacing and contained project-frame gaps
of 12, 7, 12, and 12 at detected sentence boundaries. The 7-frame transition
was valid by the old cleanup contract, but it made the finished narration feel
less consistent.

The fixture and editorial truth are unchanged from v3. Keeping that successful
artifact contract stable isolates the pacing change. V4 adds:

- acoustic upper bounds across consecutive removed filler words without
  shortening already-natural pauses;
- a compact `get_dialogue_pacing` inspector that reports sentence boundaries,
  acoustic pauses, transcript fallbacks, reasons, and short/target/long status;
- an evaluator-owned assertion requiring every detected acoustic sentence
  pause to land between 10 and 40 project frames;
- a higher human gate: pacing at least 4.5/5, overall mean at least 4.0/5, and
  every other dimension at least 3.5/5.

The model is asked to cap filler bridges at 31 acoustic source frames and retain
9 source frames around ordinary silence cuts. The planner removes a filler run
atomically, trims only silence beyond the cap, and does not preserve filler
audio. Source words whose ASR timestamps sit wholly inside detected silence are
excluded from timestamp-proxy retention assertions; the independent rendered
transcript remains authoritative for audible words. The pacing evaluator
measures mapped acoustic silence, not planner claims.

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

The corrected Codex machine baseline on revision `5f6cfea` passes all three
samples and all 99 assertions. Every sample independently produces the same
585-frame dialogue timing, four acoustically measured sentence gaps of 33, 15,
23, and 16 project frames, no cuttable 20-frame silence, and a 4.77%
independent rendered-dialogue word error rate. Each sample uses exactly 11 tool
calls and 158,076-160,022 reported tokens, averaging 159,109. That is 36.1%
below the superseded 249,112-token mean while satisfying the stronger acoustic
contract.

The run produced two SHA-distinct MP4s because the third agent chose a
different valid caption-correction grouping. Both variants pass exact caption,
safe-area, render, and delivery assertions. The full record and both hashes are
in [`baseline.json`](baseline.json); both remain pending human review.

The checked-in [regression evidence](token-regression.json) and
[portable technical report](../../../docs/reports/m39-token-regression/report.html)
preserve the pre-fix run, post-fix run, root cause, and limitations.

Each output remains SHA-bound to separate human review. The milestone passes
only when all three samples pass the machine contract, at least two are
accepted by a person, mean human rating is at least 4.0, pacing is at least
4.5, no other dimension falls below 3.5, and no material caption error or
audible filler survives.

Qualitative review of the superseded artifact called the pacing a major
improvement and identified its two opening defects. The corrected machine run
is complete, but neither new SHA has a formal v4 rubric yet. A machine pass
alone does not complete M39.

This is still a synthetic motion-graphic fixture. It can isolate dialogue
editing and narrative correctness, but it cannot establish professional shot
taste or photoreal finishing.
