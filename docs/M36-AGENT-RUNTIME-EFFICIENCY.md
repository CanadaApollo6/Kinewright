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
| Internal capability registry (2026-09-09, after AU4 Part B) | 134 | 1,524,370 B | 1,389,434 B | 112,948 B |
| Served MCP runtime (2026-09-09, after AU4 Part B) | 7 | 5,660 B | 3,510 B | 998 B |
| Internal capability registry (2026-09-10, after AU5 Part A) | 135 | 1,531,264 B | 1,391,430 B | 117,683 B |
| Served MCP runtime (2026-09-10, after AU5 Part A) | 7 | 5,660 B | 3,510 B | 998 B |
| Internal capability registry (2026-09-10, after AU5 Part B) | 138 | 1,540,264 B | 1,397,156 B | 120,458 B |
| Served MCP runtime (2026-09-10, after AU5 Part B) | 7 | 5,660 B | 3,510 B | 998 B |

AU5 Part B's three capabilities, measured one row each (AU5 §5.9 rule 117):

| Capability | Kind | Serialized | Input schema | Description |
|---|---|---:|---:|---:|
| `plan_dialogue_repair` | Planner | 3,369 B | 2,207 B | 995 B |
| `capture_room_tone` | Action | 2,638 B | 1,664 B | 808 B |
| `plan_room_tone_fill` | Planner | 2,993 B | 1,855 B | 972 B |

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

AU4 Part B adds two planners and no operation at all: `plan_audio_ducking` and
`plan_clip_fades` join `INSPECTOR_TOOL_NAMES` beside `plan_audio_normalization`,
so the counts go to 54 generated operations + 80 inspectors = 134 and the
registry grows by 5,318 B to 1,524,370 B. Unlike Part A, this costs nothing
outside the two new rows: neither planner touches the `Operation` model, and a
planner's arguments embed no `Operation` at all, so no pre-existing tool moves
a byte and the split is simply the two tools whole:

- **3,146 B** of input schema, `AudioDuckingPlanArgs` (2,391 B) and
  `ClipFadesPlanArgs` (755 B), both `deny_unknown_fields` and both embedding
  only `TrackId`, `TimeCode` and the shared half-open project range;
- **1,845 B** of description, 1,000 B for `plan_audio_ducking` and 845 B for
  `plan_clip_fades`, each under the 1,024 B budget.

That is 3,146 + 1,845 = 4,991 B against 5,318 B serialized; the remaining 327 B
are the two rows' names, annotations and JSON envelopes (3,556 B + 1,762 B
measured whole). Both descriptions spend most of their budget on their *first*
sentence, and that is the point: `get_capability` and `search_capabilities`
publish `first_sentence(description)` and drop the rest, so the two facts an
agent needs before committing a duck — that the plan replaces the music track's
whole gain automation, and that the loudness measurement can come back null on
a short window while the curve commits anyway — have to live in it. The fade
planner's row is 14 B heavier than its first measurement for exactly that
reason: "emitting set_clip_audio only" was moved out of a third sentence no
compact surface publishes and into the first, which is 14 B of prose bought to
make one clause reachable. The ducking row carries 137 B a review round bought
in *field* doc comments rather than prose — 10 B on `depth_tenth_db` for the
sign it now refuses and 127 B on `range` for the straddling window that holds
the floor to the end of the project — and that is why the description column
does not move at all while the input-schema column does: a field doc is billed
to the schema its type generates, so the 24 B left in `plan_audio_ducking`'s
1,024 B description budget stayed unspent. The served quad is byte-identical
for the tenth consecutive measurement at 7 / 5,660 B / 3,510 B / 998 B: a
planner is registry-only, reached through `invoke_capability`, whose argument
schema is generic and embeds no planner argument type. The reduction holds at
99.63%, 5,660 B served against a 1,524,370 B registry.

AU5 Part A adds one tool, the `get_audio_repair` inspector, and three effect
descriptors: 54 generated operations + 81 inspectors = 135, and the registry
grows by 6,894 B to 1,531,264 B. The 1,996 B of new input schema is
`AudioRepairArgs`' own schema entire — AU5 adds no `Operation` variant, and a
new *descriptor* reaches no input schema at all, because the `Operation` schema
embeds `Effect.parameters` as an untyped map. Forty-five new parameter rows are
therefore free on the input-schema column and cost something only on the
description column, where the 4,735 B splits exactly three ways: 925 B of
`get_audio_repair`'s own prose, 3,790 B of `effect_documentation()` growth at
758 B on each of the five spliced effect tools, and 20 B on `plan_clip_fades`,
whose window sentence now describes the fade-length RMS window and the
one-pass-per-track measurement that replaced AU4's two 400 ms renders per clip,
and whose skip clause now carries the same predicate the emitted per-clip
reason does.

That 758 B is what the profile hatch buys. `audio_denoise` carries 31
`profile_band{nn}_tenth_db` rows, and an enumerated row measures 48 B, so the
31 of them with their separators are 1,550 B against the 175 B pattern sentence
that replaces them: the growth would have been 2,133 B per spliced tool and
10,665 B over the five, and the hatch saves 1,375 B per tool and 6,875 B in
all — an eighth kind of pattern documentation on the same argument that gave
`color_curves` its compact form. Unlike the earlier hatches this one is
mandatory rather than economical: the 31 rows are written all-or-none by a
measurement, so an agent has no use for their individual bounds. Serialized,
3,084 B is the new tool whole, 3,790 B is the effect-tool description growth
and 20 B is the fade planner's, summing to the 6,894 B measured. The served
quad is byte-identical for the eleventh consecutive measurement at
7 / 5,660 B / 3,510 B / 998 B, and the reduction holds at 99.63%, 5,660 B
served against a 1,531,264 B registry.

AU5 Part B adds three capabilities and no operation at all: the two planners
`plan_dialogue_repair` and `plan_room_tone_fill`, and `capture_room_tone`,
which is a `CapabilityKind::Action` by inference — it carries no `get_` or
`plan_` prefix and no `CAPABILITY_KIND_OVERRIDES` entry — so the counts go to
54 generated operations + 84 hand-written capabilities = 138 and the registry
grows by 9,000 B to 1,540,264 B. As in AU4 Part B, this costs nothing outside
the three new rows: none of the three touches the `Operation` model, and
`capture_room_tone` writes its asset through the ordinary `AddAsset` variant
that already existed, so no pre-existing tool moves a byte. Part B also adds no
effect descriptor, so `effect_documentation()` is byte-unchanged and Part A's
pattern sentence — which has named `plan_dialogue_repair` on five spliced tool
descriptions since three commits before the planner existed — is deliberately
left exactly as it was rather than rewritten for a second measurement of the
same ledger.

The split is the three rows whole, and it sums exactly:

- **5,726 B** of input schema, `DialogueRepairPlanArgs` (2,207 B),
  `RoomToneFillPlanArgs` (1,855 B) and `CaptureRoomToneArgs` (1,664 B), all
  three `deny_unknown_fields`;
- **2,775 B** of description, 995 B, 972 B and 808 B, each inside the 1,024 B
  budget.

That is 5,726 + 2,775 = 8,501 B against 9,000 B serialized; the remaining
499 B are the three rows' names, annotations and JSON envelopes. A row's
envelope is **147 B plus the length of its name**, which reproduces every
earlier measurement in this document — `get_audio_qc` 147 + 12 = 159,
`get_audio_repair` 147 + 16 = 163, `plan_dialogue_repair` 147 + 20 = 167,
`plan_room_tone_fill` 147 + 19 = 166 — and `capture_room_tone` measures 166
rather than its 147 + 17 = 164 because its description quotes the default asset
name and the description column counts those two `"` unescaped while the
serialized column counts them escaped. The two annotation sets cost the same
number of bytes: a read-only planner spells `true`/`false` where the
byte-writing Action spells `false`/`true`.

Each description spends its budget the same way AU4's two planners do, and for
the same reason: `get_capability` and `search_capabilities` publish
`first_sentence(description)` and drop the rest, so the one clause an agent
must not miss has to live in sentence one. For `plan_dialogue_repair` that is
that the planner **refuses** when the measured signal-to-noise gain misses
`minimum_snr_gain_db_hundredths`, together with the direction the percentile
floor biases that measurement — a caller that does not know the floor is a
10th-percentile window rather than a detected silence reads an honest refusal
on continuous speech as a bug. For `plan_room_tone_fill` it is that only
leading and interior gaps are filled, on the same track and butt-joined, and
that a gap it cannot fill to the exact frame is skipped with a reason instead
of failing the whole plan. For `capture_room_tone` it is that the tool
**writes bytes** under the project directory and therefore asks first. The
served quad is byte-identical for the twelfth consecutive measurement at
7 / 5,660 B / 3,510 B / 998 B — all three capabilities are registry-only,
reached through `invoke_capability`, whose argument schema is generic — so the
reduction holds at 99.63%, 5,660 B served against a 1,540,264 B registry.

The registry grew with the colour tools; the served surface stays at seven
tools. The `color_curves` descriptor (133
parameters) is summarized as a compact pattern in tool documentation, keeping
roughly 18.8 KB out of the registry, AU2 gives `audio_parametric_eq`'s
twelve peaking-band rows the same treatment, and AU5 gives `audio_denoise`'s
31 learned noise-floor bands theirs, keeping a further 6.9 KB out.
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
