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
`filler_bridge_pause_source_frames` value. When a run of filler words sits
between clean dialogue, the planner:

1. protects the full bridge from ordinary silence and filler cuts;
2. removes the entire filler run with one central cut;
3. retains the requested total clean pause, split across the adjacent speech;
4. reports the source range, actual retained pause, and whether the request had
   to be constrained by available clean audio.

The option is additive. Existing callers that omit it retain the v3 behavior.

`get_dialogue_pacing` is a compact read-only inspector over final timeline
words. It recognizes boundaries from terminal punctuation, asset changes,
speaker changes, and pause-backed capitalization. It reports every boundary's
word pair, project-frame gap, reason, and short/target/long classification.

## Independent scoring

The v4 evaluator calculates the same boundary facts from its final timeline
word snapshot rather than trusting the planner or inspector response. Its new
`DialoguePauseBounds` assertion fails if any detected sentence gap falls
outside 9 through 15 project frames.

The published contract is
[`benchmarks/auto-edit/v4`](../benchmarks/auto-edit/v4/README.md). It preserves
the v3 fixture, story, caption, render, and delivery assertions while asking
the agent to normalize removed filler bridges to 12 source frames. The human
target rises to 4.5/5 for pacing, 4.0/5 overall, and 3.5/5 for every other
dimension.

## Exit contract

- Three of three Codex samples pass every machine assertion.
- At least two of three SHA-bound artifacts receive human acceptance.
- Mean human rating is at least 4.0/5.
- Pacing is at least 4.5/5 and every other dimension is at least 3.5/5.
- No audible filler or material caption error remains.

Machine success does not complete M39. Human review remains authoritative for
whether the normalized pauses actually feel natural.
