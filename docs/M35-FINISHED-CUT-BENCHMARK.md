# M35 - Finished-cut benchmark

M35 turns the public agent benchmark into a real delivery loop:

```text
generated footage
    -> isolated OpenReel runtime
    -> installed agent edits through MCP
    -> structural and editorial assertions
    -> delivery conformance
    -> real compositor proof sheet
    -> immutable MP4 export
    -> independent media probe and SHA-256
    -> optional human acceptance review
```

The benchmark contract lives in
[`benchmarks/auto-edit/v2`](../benchmarks/auto-edit/v2/README.md). V1 remains
the exact-operation regression suite. V2 measures whether the same model-facing
contract can produce a finished, reviewable first cut.

## Acceptance boundary

The machine and human score layers are intentionally separate.

A machine pass proves that the agent followed the brief, stayed within its
budget, produced a valid timeline, passed technical QA and delivery
conformance, rendered a proof sheet, exported a non-empty MP4, and produced a
file whose independently probed raster and duration match the verified
document. It does not prove that the edit is good.

Human acceptance remains `null` until a person watches the exact hashed output
and records:

- first-pass accepted or rejected;
- story;
- pacing;
- visual finish;
- audio finish;
- captions;
- delivery readiness.

Every rating is required when an acceptance decision is set. Partial reviews,
duplicate task IDs, invalid hashes, and scores outside `1..=5` are rejected.

## Artifact contract

Every finished-cut run writes one self-contained package under
`target/evals/<run-id>/`:

- `results.jsonl` - environment, task results, assertions, budgets, and totals;
- `machine-report.json` - the machine outcome and artifact references;
- `artifacts/<task>-sample-<n>/final-document.json` - exact verified timeline;
- `artifacts/<task>-sample-<n>/proof.png` - uniformly sampled compositor output;
- `artifacts/<task>-sample-<n>/finished.mp4` - immutable rendered deliverable;
- `human-review.json` - pending human-review template;
- `human-score.json` - written only after a complete review is validated.

The exporter refuses to overwrite an existing benchmark deliverable. The
document snapshot is captured before the undo-integrity check restores the
fixture. Multiple samples receive stable, unique human-review IDs.

Token and wall-time ceilings are always enforced. A USD ceiling is optional:
subscription harnesses such as Codex can report cumulative token usage without
an attributable per-turn dollar price. Missing price telemetry is recorded but
is not misreported as a bad edit.

## Finished-cut task

The first v2 task asks one installed model to turn five generated takes into a
vertical social edit. It must:

- use the requested takes in the requested order and reject the others;
- remove cuttable dead air and recognized filler words without dropping the
  retained dialogue;
- keep primary media gapless;
- add social captions with pop motion;
- inspect a 9:16 delivery storyboard;
- run technical QA and inspect centered `vertical_short` conformance;
- remain within explicit turn, tool, operation, token, cost, wall-time, and
  undo budgets.

OpenReel, not the agent, exports the exact verified timeline after the turn.
That prevents an agent from satisfying the benchmark with an unrelated file or
an unverified shell command.

## Deliberate limits

- The fixture is generated and redistributable, but it is still synthetic.
- One task is evidence of a vertical slice, not broad editorial competence.
- Technical assertions cannot score taste, emotional timing, or whether a
  human would publish the cut.
- Proof sheets catch obvious rendering failures but do not replace watching the
  MP4 with audio.
- Human results are meaningful only when tied to the artifact SHA-256.

The next benchmark work is breadth: real-world licensed footage, multiple edit
genres, multiple models, repeated samples, blind human review, and accepted
minutes per dollar and minute of wall time.

## First live baseline

The first passing Codex run completed on 2026-08-13:

- 30/30 machine assertions;
- one turn, 24 tool calls, and 48 applied operations;
- 731,311 cumulatively reported tokens and 122.871 seconds;
- 421 frames at 1080x1920;
- 6,617,816-byte MP4 with SHA-256
  `5a352c38a49ad1626936ba98efe38e9bf546682bc89cc58c95080125bcc3491d`;
- exact probed raster and duration;
- human acceptance still pending.

The compositor proof also suggests that some caption lines exceed the vertical
safe area. The machine pass proves the pipeline and brief contract, not visual
taste or publish readiness. Delivery-aware caption layout is therefore a
concrete next gate, not a hidden caveat.
