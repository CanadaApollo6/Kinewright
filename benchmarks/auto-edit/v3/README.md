# Kinewright Editorial-Cut Benchmark v3

V3 exists because the M37 artifact passed every machine gate and still received
a human rejection at 2.25/5. It replaces arbitrary color bars and nonsense
dialogue with one coherent authored story, meaningful visual scenes, real take
selection, and output checks that do not trust the same source transcript used
to make the edit.

The fixture remains local and redistributable. FFmpeg creates five vertical
motion-graphic scenes and Windows SAPI speaks the checked-in script in
[`ground-truth.json`](ground-truth.json). Two takes contain factual or delivery
mistakes. Three form the intended story.

Machine scoring now distinguishes:

- source-guided editing from independent post-render transcription capped at
  15% ordered word error rate against the authored dialogue;
- caption existence and containment from exact authored caption text;
- hard silence removal from configurable natural pause retention;
- a technically valid MP4 from a human-acceptable first cut.

The agent reads all source transcripts through one batched capability and
finishes with one compact editorial-readiness proof. That proof owns silence,
QA, delivery, and storyboard verification so repeated context is not the price
of being thorough.

Run preparation and ordinary tests spend no model quota. A subscription-backed
run remains explicit:

```powershell
& .\scripts\setup-ffmpeg.ps1
$env:KINEWRIGHT_EVAL = '1'
cargo run -p kinewright-agent --bin kinewright-eval -- `
  --suite editorial-cut-v3 `
  --harness codex `
  --only f2 `
  --samples 3
```

The published Codex baseline on revision `f232e35` passed all three samples and
all 93 assertions. It used 16-17 tool calls and 236,336-251,345 reported tokens
per sample. Every trial produced the same 602-frame MP4 at 2.39% independently
measured word error rate. The exact machine record is in
[`baseline.json`](baseline.json). Human review accepted the artifact at a
4.08/5 mean with every dimension at least 3.5, so M38 passes its published exit
contract. Because all three trials had the same SHA-256, one viewing was
applied transparently to the three SHA-bound review rows.

Each output is SHA-bound to a separate human review. The milestone passes only
when all three samples pass the machine contract, at least two are accepted by
a person, the overall human mean is at least 3.5, no dimension is below 3, and
no material caption error or audible filler survives.

The visual fixture is intentionally honest about its reach. It tests coherent
story and take selection better than the old color bars, but generated motion
graphics cannot establish professional camera continuity, shot taste, or
photoreal finishing. Those require a future redistributable real-footage suite.
