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

The current Codex machine baseline on revision `283e704` passes all three
samples and all 102 assertions. Every sample independently produces the same
585-frame dialogue timing, four acoustically measured sentence gaps of 33, 15,
23, and 16 project frames, no cuttable 20-frame silence, and a 4.77%
independent rendered-dialogue word error rate.

Exact authored wording now goes directly into `add_styled_captions`. Script
punctuation is a hard grouping boundary, and the evaluator rejects any cue
that ends one sentence and begins another. This removes the caption inspection,
manual correction plan, and second commit. Every sample uses exactly 8 tool
calls and 107,900-108,597 reported tokens, averaging 108,296. That is 31.9%
below the prior 159,109-token baseline and 56.5% below the superseded
249,112-token mean.

All three agents now produce one byte-identical MP4, rather than two caption-
dependent variants. The full record, single hash, and exact cue grouping are in
[`baseline.json`](baseline.json). A side-by-side qualitative review of the
previous run preferred Sample 3 because its grouping respected sentence
structure; that finding is now a deterministic product rule and machine
assertion. The new SHA remains pending a formal human rubric.

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
is complete, but the current SHA does not yet have a formal v4 rubric. A
machine pass alone does not complete M39.

This is still a synthetic motion-graphic fixture. It can isolate dialogue
editing and narrative correctness, but it cannot establish professional shot
taste or photoreal finishing.
