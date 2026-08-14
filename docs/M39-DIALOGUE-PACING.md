# M39: Dialogue pacing

## Why this milestone exists

M38 moved the benchmark from a rejected 2.25/5 artifact to an accepted 4.08/5
cut. The remaining direct review note was pacing: some sentence transitions
had strange gaps, while others had almost none. Inspection of the accepted
timeline found four detected sentence gaps at 12, 7, 12, and 12 project frames.

M39 turns that feedback into a narrow, testable capability. It does not change
the accepted v3 benchmark or claim to solve general editorial rhythm.

## Agent-facing changes

`plan_dialogue_assembly` now accepts an optional
`maximum_filler_bridge_pause_source_frames` value. When a run of filler words sits
between clean dialogue, the planner:

1. protects the full bridge from ordinary silence and filler cuts;
2. removes the entire filler run with one central cut;
3. caps excessive acoustic silence without shortening a natural pause already
   below the cap;
4. reports the available pause, configured maximum, retained pause, acoustic
   source range, and measurement mode.

The option is additive. Existing callers that omit it retain the v3 behavior.

`get_dialogue_pacing` is a compact read-only inspector over final timeline
words and mapped acoustic silence. It recognizes boundaries from terminal
punctuation, asset changes, speaker changes, and pause-backed capitalization.
It reports every boundary's word pair, acoustic pause, transcript-only gap,
measurement source, reason, and short/target/long classification. Transcript
timing is an explicit fallback while acoustic analysis is unavailable.

Human review of the first v4 artifact exposed a measurement defect. Its four
transcript gaps were all reported as 12 frames, but the rendered opening
contained about 47 frames of silence at the first transition and only about 9
at the second. The two later transitions felt natural. Filler-bridge planning
now retains pause from detected acoustic edges, and the default speech-silence
threshold is -35 dBFS so the measured endpoints better match what is audible.

## Independent scoring

The v4 evaluator calculates the same boundary facts from mapped final-timeline
silence rather than trusting the planner or inspector response. Its
`DialoguePauseBounds` assertion fails if any detected acoustic pause falls
outside 10 through 40 project frames. That calibrated range rejects the two
reviewed opening defects without rejecting the later natural pauses; it is a
quality bound, not a request to make every sentence break identical.

The published contract is
[`benchmarks/auto-edit/v4`](../benchmarks/auto-edit/v4/README.md). It preserves
the v3 fixture, story, caption, render, and delivery assertions while asking
the agent to cap removed filler bridges at 31 acoustic source frames and retain
9 frames around ordinary silence cuts. Source-ASR words located wholly inside
detected silence are excluded from timestamp-proxy assertions because the
independent rendered transcript is authoritative for audible content. The human
target rises to 4.5/5 for pacing, 4.0/5 overall, and 3.5/5 for every other
dimension.

## Exit contract

- Three of three Codex samples pass every machine assertion.
- At least two of three SHA-bound artifacts receive human acceptance.
- Mean human rating is at least 4.0/5.
- Pacing is at least 4.5/5 and every other dimension is at least 3.5/5.
- No audible filler or material caption error remains.

The original 3/3 machine baseline is retained as historical evidence, but its
transcript-only pacing assertion is no longer considered valid. A fresh run is
required after this correction. Machine success does not complete M39; human
review remains authoritative for whether the bounded pauses feel natural.
