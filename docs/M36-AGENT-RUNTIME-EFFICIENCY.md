# M36 - Agent runtime efficiency

M36 gives Kinewright-owned agent sessions a small, task-shaped editing contract
while keeping the complete capability catalog internal. It targets the repeated
tool-schema cost and coordination risk created by advertising every editor
capability on every model turn.

## Outcome and requirements

An Kinewright session now starts with seven stable tools:

1. `get_timeline_state`
2. `search_capabilities`
3. `get_capability`
4. `invoke_capability`
5. `prepare_edit_plan`
6. `commit_edit_plan`
7. `discard_edit_plan`

The model searches a concise capability directory, opens only the exact schema
it needs, invokes non-edit capabilities through one dispatcher, and submits
timeline mutations as one compact plan. The requirements were:

- keep the generated capability registry internal and expose one runtime;
- keep every mutation inside the existing validated Rust `Operation` model;
- validate a complete plan before changing the live document;
- bind prepared plans to an exact `TimelineRevision`;
- keep destructive confirmation, undo, provenance, and replay semantics on the
  existing commit path;
- measure advertised schema bytes and provider-reported token categories
  honestly.

## Architecture

```text
Claude Code / Codex / Cursor
            |
            v
  seven-tool compact surface
      |                 |
      v                 v
capability directory  prepared-plan store
      |                 |
      +--------+--------+
               v
    existing capability handlers
               |
               v
 deterministic Rust Operation core
```

The transport remains MCP because the supported agent harnesses understand it.
There is no full-surface server mode. Every client sees the same seven tools and
an exact harness allowlist. A direct MCP call to a known internal capability
name is rejected; the capability must pass through the dispatcher or prepared-
plan path.

## Capability loading

`search_capabilities` returns names, kinds, and one-sentence summaries from the
authoritative internal registry. `get_capability` returns the selected capability's
exact input schema and whether to use `invoke_capability` or place the operation
inside `prepare_edit_plan`.

The directory distinguishes inspectors, planners, actions, and edit operations.
It does not duplicate capability implementations: `invoke_capability` dispatches
to the existing handler after checking its allowlist. Edit operations cannot be
invoked through that dispatcher because they must be prepared and committed as
one atomic unit.

## Prepared-plan lifecycle

`prepare_edit_plan` accepts compact operation objects and an expected timeline
revision. The server decodes every operation, applies the entire batch to a
document clone, and returns an opaque plan id plus a deterministic before/after
preview. The live timeline is unchanged.

`commit_edit_plan` requires that plan id and the same revision. It reuses the
existing atomic edit-plan path, including revision checking, one destructive
confirmation, one undo entry, and normal document-change events. A committed
plan cannot be committed again. `discard_edit_plan` explicitly releases an
unused plan.

Prepared plans are process-local and intentionally bounded to 64 entries. Old
entries may expire, and every entry disappears when the project server stops.
The safe recovery is to inspect the current revision and prepare again.

## Measured surface reduction

The catalog is measured from the exact serialized `rmcp::model::Tool` values
served by the runtime. The M36 regression test records:

| Surface | Tools | Serialized metadata | Input schemas | Descriptions |
|---|---:|---:|---:|---:|
| Internal capability registry (M36 baseline) | 85 | 585,247 B | 543,414 B | 27,949 B |
| Served MCP runtime (M36 baseline) | 7 | 5,305 B | 3,171 B | 982 B |
| Internal capability registry (2026-08-24, after CC1-CC3) | 113 | 1,007,001 B | not re-split | not re-split |
| Served MCP runtime (2026-08-24) | 7 | 5,660 B | not re-split | not re-split |
| Internal capability registry (2026-08-25, after CC4) | 120 | 1,222,241 B | not re-split | not re-split |
| Served MCP runtime (2026-08-25) | 7 | 5,660 B | not re-split | not re-split |
| Internal capability registry (2026-08-25, after CC5) | 123 | 1,269,402 B | 1,154,933 B | 94,274 B |
| Served MCP runtime (2026-08-25, after CC5) | 7 | 5,660 B | 3,510 B | 998 B |
| Internal capability registry (2026-08-25, after CC6) | 124 | 1,280,060 B | 1,163,879 B | 95,827 B |
| Served MCP runtime (2026-08-25, after CC6) | 7 | 5,660 B | 3,510 B | 998 B |
| Internal capability registry (2026-08-27, after CC7) | 124 | 1,280,060 B | 1,163,879 B | 95,827 B |
| Served MCP runtime (2026-08-27, after CC7) | 7 | 5,660 B | 3,510 B | 998 B |
| Internal capability registry (2026-09-07, after AU1) | 126 | 1,303,967 B | 1,186,449 B | 96,840 B |
| Served MCP runtime (2026-09-07, after AU1) | 7 | 5,660 B | 3,510 B | 998 B |
| Internal capability registry (2026-09-08, after AU2 Part A) | 126 | 1,312,132 B | 1,186,449 B | 105,005 B |
| Served MCP runtime (2026-09-08, after AU2 Part A) | 7 | 5,660 B | 3,510 B | 998 B |
| Internal capability registry (2026-09-08, after AU2 Part B) | 129 | 1,421,520 B | 1,293,084 B | 107,271 B |
| Served MCP runtime (2026-09-08, after AU2 Part B) | 7 | 5,660 B | 3,510 B | 998 B |
| Internal capability registry (2026-09-09, after AU3 Part A) | 130 | 1,424,875 B | 1,295,138 B | 108,413 B |
| Served MCP runtime (2026-09-09, after AU3 Part A) | 7 | 5,660 B | 3,510 B | 998 B |
| Internal capability registry (2026-09-09, after AU3 Part B) | 130 | 1,425,658 B | 1,295,459 B | 108,875 B |
| Served MCP runtime (2026-09-09, after AU3 Part B) | 7 | 5,660 B | 3,510 B | 998 B |
| Internal capability registry (2026-09-09, after AU4 Part A) | 132 | 1,519,052 B | 1,386,288 B | 111,103 B |
| Served MCP runtime (2026-09-09, after AU4 Part A) | 7 | 5,660 B | 3,510 B | 998 B |

That is a 99.1% reduction in initially advertised serialized tool metadata at
the M36 baseline, 99.4% at the 2026-08-24 measurement, 99.56% after CC6
(5,660 B served against a 1,280,060 B registry), and 99.57% after AU1
(5,660 B served against a 1,303,967 B registry); CC7 adds no tool, so its two
rows are byte-identical to CC6's. AU1 adds the generated `set_track_mix`
mutator and the `get_audio_levels` inspector, which grow the registry by
23,907 B without moving the served surface by a byte: neither tool is served,
and the seven served tools do not embed the `Operation` schema. AU2 Part A adds
no tool and no input-schema byte: the `Operation` schema embeds
`Effect.parameters` as an untyped map, so nothing it adds can reach an input
schema and only description bytes move, from 96,840 B to 105,005 B. That
8,165 B splits into 6,495 B of descriptor rows (1,299 B on each of the five
effect tools) and 1,670 B of prose (the rewritten `upsert_audio_bus` /
`remove_audio_bus` arm, 335 B to 1,170 B, on two tools). `input_schema_bytes`
stays byte-identical at 1,186,449 B and the served triple at
7 / 5,660 B / 3,510 B / 998 B.

AU2 Part B adds three tools — the generated `set_audio_master` and
`set_pan_law` mutators and the `get_audio_spectrum` inspector — so the registry
goes to 129 tools and 1,421,520 B. Unlike Part A it does move input schemas, by
106,635 B, because it changes the `Operation` model rather than the descriptor
table. The measured split, which sums exactly:

- **43,559 B**, the two new mutators' own schemas (21,785 B + 21,774 B), each
  carrying its own copy of the shared `Operation` `$defs`;
- **1,340 B**, `get_audio_spectrum`'s own schema, which embeds no `Operation`
  and is the cheapest tool in the registry;
- **61,736 B** spread over the 51 pre-existing tools that do embed `Operation`:
  1,195 B of shared `$defs` growth on each of the fifty generated tools
  (`AudioMaster` and `PanLaw` are new definitions and `AudioBus` gains
  `gain_tenth_db`; nothing references `AudioMix`, so its two new fields reach
  no input schema at all), plus 62 B on `set_track_mix` for the reworded
  `pan_percent` doc comment, plus 1,924 B on `apply_edit_plan` — the one
  non-generated tool that embeds `Operation`, whose inline definition gains the
  two new `oneOf` variants and the same 62 B doc comment.

That is 49 x 1,195 + (1,195 + 62) + 1,924 = 61,736, and
43,559 + 1,340 + 61,736 = 106,635.

The 2,266 B of new descriptions splits exactly four ways: 1,302 B of the two
new mutators' prose, 636 B of `get_audio_spectrum`'s, 194 B for the bus fader
sentence on the two bus tools (97 B x 2), and 134 B for the law-neutral
`set_track_mix` rewrite. The served quad is byte-identical again at
7 / 5,660 B / 3,510 B / 998 B: none of the three new tools is served, and the
seven served tools do not embed the `Operation` schema, so even a model change
cannot reach them. That is the whole point of the split surface — a 99.6%
reduction that holds at 5,660 B served against a 1,421,520 B registry.

AU3 Part A adds one tool, the `get_audio_qc` inspector, so the registry goes to
130 tools and 1,424,875 B, up 3,355 B. The 2,054 B of new input schema is
`get_audio_qc`'s own schema entire: it embeds no `Operation`, and none of the
AU3 core types (`AudioLoudness`'s four new fields, `LoudnessTarget`,
`AudioQcReport`) appears in any tool's arguments, so no other schema moved. The
1,142 B of new descriptions splits exactly three ways: 890 B of `get_audio_qc`'s
own prose, 205 B for `get_audio_levels`' four-field gloss and its sub-block
refusal sentence, and 47 B for `get_delivery_profiles`' loudness-target clause.
The served quad is byte-identical again at 7 / 5,660 B / 3,510 B / 998 B: a
99.60% reduction that holds at 5,660 B served against a 1,424,875 B registry.

AU3 Part B adds no tool at all: the counts stay 52 generated operations + 78
inspectors = 130, and the registry grows by 783 B to 1,425,658 B. The 321 B of
new input schema is `QueueExportArgs`' `normalize_loudness` boolean and nothing
else — Part B's other new state (`ExportSettings.loudness_normalization`, the
three `ExportJobRecord` audio fields, `ExportAudioReport`,
`DeliveryAudioVerification`) is all *output*, and no tool takes an
`ExportJobRecord` or an `ExportSettings` as an argument, so none of it reaches
a schema the registry measures. The 462 B of new descriptions splits exactly
three ways: 124 B for `plan_audio_normalization`'s true-peak-limiter rename and
its re-cue clause, 267 B for `queue_export`'s `normalize_loudness` clause,
and 71 B for `get_export_jobs`' audio-verification and normalization-report
clauses. The planner's clause costs 4 B more than the standalone sentence it
replaced, and buys the only thing that matters: `get_capability` and
`search_capabilities` publish `first_sentence(description)` and drop the rest,
so a re-cue warning in a second sentence is 120 B no agent ever reads. The
served quad is byte-identical for the eighth consecutive measurement at
7 / 5,660 B / 3,510 B / 998 B — `queue_export`, `get_export_jobs` and
`plan_audio_normalization` are all registry-only tools — so the reduction
holds at 99.60%, 5,660 B served against a 1,425,658 B registry.

AU4 Part A adds two generated mutators, `set_clip_gain_envelope` and
`set_track_automation`, so the counts go to 54 generated operations + 78
inspectors = 132 and the registry grows by 93,394 B to 1,519,052 B. The
90,829 B of new input schema splits exactly three ways:

- **45,878 B**, the two new mutators' own schemas (22,842 B + 23,036 B), each
  carrying its own copy of the shared curve `$defs`;
- **42,640 B**, 820 B of shared `$defs` growth on each of the 52 pre-existing
  generated tools;
- **2,311 B** on `apply_edit_plan`, the one non-generated tool that embeds
  `Operation`: the same 820 B of `$defs` growth plus 1,491 B for the two new
  `oneOf` variants its inlined definition gains (844 B + 645 B + the two
  separating commas).

That is 45,878 + 52 x 820 + 2,311 = 90,829. The 820 B figure is the whole
point of AU4's reuse rule: it is *field* growth on the three
`Operation`-reachable types that gained a curve — `Clip.audio_gain_curve`
(347 B), `AudioBus.gain_curve` (234 B) and `AudioMaster.gain_curve` (236 B),
plus three separating commas; `TrackMix` is not `Operation`-reachable, so its
two curves cost the registry nothing — and not a new `$defs`
type, because `AutomationCurve`, `Keyframe` and `KeyframeInterpolation` are
already reachable through `SetEffectKeyframes`. AU2 Part B paid 1,195 B per
tool for two genuinely new definitions; AU4 pays 820 B for five new fields on
types that were already there. A second pair of variants modelled as
`Set…`/`Clear…` instead of one `Option<AutomationCurve>` would have cost two
more whole tool schemas, roughly 44 kB, and taught an agent nothing extra.

Each of the four published `curve` occurrences — one in each new tool's own
variant schema, one in each of `apply_edit_plan`'s two new `oneOf` variants —
is an `anyOf` of a `$ref` to `AutomationCurve` and `null`, not an inlined
curve object. Required and nullable are both load-bearing: `null` is the only
clear, so a schema that admits only the object would forbid the one documented
way to remove a ride. The `anyOf` is also 96 B cheaper per occurrence.

The 2,228 B of new descriptions is the two new tools' prose entire (1,003 B +
1,225 B); no existing description was touched, and the sum being exact is what
proves it. Serialized, the two tools cost 48,444 B whole plus the same
44,951 B of schema growth on the 53 pre-existing `Operation`-embedding tools,
**minus one byte**: `set_effect_keyframes` joins the `.idempotent(...)` list in
the same edit, and `"idempotentHint":true` is one byte shorter than the
`"idempotentHint":false` it replaces. The served quad is byte-identical for the
ninth consecutive measurement at 7 / 5,660 B / 3,510 B / 998 B — neither new
tool is served, and the seven served tools embed no `Operation` schema, so a
model change cannot reach them at all — so the reduction holds at 99.63%,
5,660 B served against a 1,519,052 B registry.

The registry grew with the colour tools; the served surface stays at seven
tools. The `color_curves` descriptor (133
parameters) is summarized as a compact pattern in tool documentation, keeping
roughly 18.8 KB out of the registry, and AU2 gives `audio_parametric_eq`'s
twelve peaking-band rows the same treatment.
It is not yet a claim of 99.1% fewer provider tokens. Providers transform,
cache, and meter tool definitions differently. A controlled benchmark between
the pre-M36 revision and the current runtime is the acceptance gate for model-
token and latency savings; Kinewright does not retain an obsolete production mode
only to run that comparison.

## Token telemetry

`AgentEvent::Cost`, the chat UI, JSONL eval records, suite totals, and the
scoreboard now carry these categories when a harness reports them:

- total input tokens;
- cached input tokens;
- cache-creation input tokens;
- output tokens;
- reasoning output tokens;
- dollar cost.

Claude cache reads and cache creation are included in normalized total input.
Codex accepts both its direct fields and OpenAI-style nested token-detail
fields. Missing provider categories remain `n/a`; Kinewright does not silently
turn missing telemetry into zero. Each eval result also records the exact tool
surface byte counts used for that session.

## Reliability and safety

- Revision conflicts fail closed before a live mutation.
- Invalid batches fail during clone validation and leave no prepared plan.
- The bounded plan store prevents unbounded session memory growth.
- Edit operations cannot bypass atomic preparation through the generic
  dispatcher.
- The authoritative Core actor still owns validation, confirmation, undo,
  journal, provenance, and document broadcasts.
- Direct MCP calls to internal capability names fail closed.

## Deliberate limits and next proof gates

M36 does not replace MCP, add a hosted agent service, or claim a live benchmark
win that has not been measured. Schemas returned as capability results still
consume context when opened, but only for the current task. The generic
dispatcher is conservatively annotated because it can reach both read-only and
action capabilities; a later runtime can split those paths if a harness uses
annotations for permission prompts.

The next useful runtime steps are:

1. run the M35 suite against the pre-M36 revision and current runtime and
   publish token, latency, tool-call, correction, and acceptance deltas;
2. add task-scoped capability packs and revision-delta observation;
3. add content-addressed plan identities for durable replay across processes;
4. benchmark native, Pi, Prime Agent, and other harness adapters against the
   same work-quality and token-efficiency gates before adopting one;
5. keep MCP as the public interoperability layer while allowing a tighter
   in-process runtime for harnesses Kinewright directly controls.

## Verification

M36 is gated by unit tests for compact operation decoding, capability search,
stale and invalid plan rejection, atomic prepare/commit behavior, duplicate
commit rejection, underlying capability attribution, custom harness allowlists,
cache-token normalization, and eval aggregation. The full workspace build,
test, formatting, and strict Clippy gates remain the release boundary.
