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

That is a 99.1% reduction in initially advertised serialized tool metadata at
the M36 baseline and 99.4% at the 2026-08-24 measurement. The registry grew
with the colour tools; the served surface stays at seven tools. The
`color_curves` descriptor (133 parameters) is summarized as a compact pattern in
tool documentation, keeping roughly 18.8 KB out of the registry.
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
