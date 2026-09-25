# AW0 — Agent Workbench programme design

Status: **promoted 2026-09-24** (revision 2) — incorporates the Opus critic
verdict (`revise`), the lead's N1 rulings, and Riel's answers (all binding).
Decides the boundary, slice order, headless/code-clip/whole-edit models,
sharing, security, determinism, budgets, and slices AW1–AW4.

Conventions: prose cites symbols (`Type`/`fn`/module names), never
`file:line`. Numbers are measured on main unless marked as estimates.

## Changes in revision 2

- B1 → §10/§12: undo gate restated (change present + one CLI commit in
  provenance); cross-process undo claimed only on the B2 proxy path.
- B2 → §4/§10: the central change — GUI-proxy stdio design; lockfile
  holds endpoint + 0600 Bearer [REDACTED]; auth added to the server.
- B3 → §4/§10: project IO moves to a shared crate; lock covers project
  + sidecar + journal; decision log via temp+rename, not append-only.
- B4 → §4/§7: registry lifecycle ops, CLI `new`/`import`/`save`,
  headless save-after-commit policy.
- B5 → §4/§8: destructive-consent reconciliation (per-call consent +
  snapshot + explicit flag + roots); stderr prompt deleted.
- S6 → §4/§9: headless renderer confirmed feasible; pinned adapter for
  byte-identity; transport/handler split; stdout protocol-only.
- S7 → §3/§5: SVG-import reconciliation, declared descriptors, typed
  bindings, hash-blob source; MO2 dependency softened.
- S8 → §1/§5/§12/§14: bespoke DSL replaced by Lottie (Riel ruled).
- S9 → §6/§7: aggregate `apply_batch`, lenient plan deserialize,
  revision-indexed snapshots, two-tier `explain_frame`, capped payloads.
- S10 → §12/§13: AW4 via Claude Code `-p` both arms, full-strength
  baseline, N runs, blinded; "code scores zero" dropped.
- S11 → §12: serialised landing at registry pin sites and app files.
- S12 → §1/§4/§6/§8/§15: per-frame hash, batch branch proofs, hashed
  fonts, MCPB package, file-reference images; audio-gen/URL-fetch out.
- Nit → §4: verb count fixed. Manim/Remotion → §5/§15: native-only for
  now; render-import adapter specified, built only if AW4 shows need.
- N1.1 → §1/§5/§7/§12/§13/§15: (a) no-bundled-runtime principle,
  (b) AW2 parity goal + job → feature → gap table, (c) memory/CPU
  budgets as named gates in every slice and AW4, (d) adapter
  specified-only and external if ever built.

## 0. How this programme runs

Recipe v2 mechanics, fixed for AW1–AW4 (the MO0 precedent):

- One design doc per slice (≤ ~600 lines), citing symbols not `file:line`;
  an Opus critic reviews each design; probes only for questions reading
  cannot settle.
- Stages ship with two Opus reviews each and a green workspace gate per
  commit; a hands-on session with Riel ends each slice, and the slice is
  not complete until its findings are discharged or recorded as deferrals.
- Exit gates are named behaviours that fail today, never "no regressions"
  alone. Every render gate names its lane (GPU / lavapipe / CPU reference).

This doc validates the lead's three-slice proposal, keeps all three, and
adds one: AW4 (workflow evaluation) so the "video from code in Claude
Desktop" comparison is a measured slice, not an aspiration.

## 1. Product boundary

**Thesis (binding).** Frontier models can make playable demos from code,
but handed a real engine they accomplish vastly more in far less time.
Kinewright must be that engine for video: an agent driving Kinewright
does everything "make a video from code" in Claude Desktop can, and
vastly more, faster — measured on the agent-as-power-user axes: speed,
ease, token efficiency.

In scope for this programme:

- A headless Kinewright: a `kinewright` CLI plus a stdio MCP mode that
  proxies to the GUI's live server when the project is open there and
  runs headless otherwise — so any MCP client drives a real project.
- Code as a first-class clip source: vector/code-generated clips
  (SVG first, then Lottie; Typst, WGSL later) with keyframable
  parameters, deterministic cached renders, sandboxing, and provenance.
  Manim/Remotion stay native-only-for-now: execution closed, a
  render-import adapter specified in AW2 but built only if AW4 shows
  agents need it — and outside Kinewright if ever built (Riel ruled).
- Programme principle (N1.1): native Rust only. Kinewright never ships
  a bundled Node, Python, or Chromium runtime to render code clips.
- Agent-lethality multipliers on the existing runtime: one-call
  whole-edit plans with complete errors, compact timeline state diffs,
  and `explain_frame` provenance for any pixel/region.
- The sharing contract: one project, person plus agent, with explicit
  locking and a project decision log (intent memory beside the project).
- A shipped Kinewright Claude skill with craft defaults.

Out of scope (owned elsewhere, not re-decided here): the IN3 row
(timeline view, post-render self-check, cut defaults, capability packs,
verbatim transcripts) — AW consumes it; MO2 compositing and MO6
titles/templates — AW2 coordinates with them; hosted services, account
systems, generated raster/video models (deferred per the
generative-assets decision); audio generation and URL fetching (the
host downloads; Kinewright ingests local bytes).

**Rejected alternative.** "Kinewright as a library" (Python/JS SDK).
Rejected: it strands the validated core, undo, revision gating, proofs,
and the GUI that opens the same file — every property that beats code.

**Cost.** One new binary surface (CLI), one new transport feature on the
pinned `rmcp`, one shared project-IO crate (moved, not new logic), one
new `ClipContent` variant family, lifecycle + lethality capabilities
(all registry-only), no new model dependencies.

## 2. Current foundation, measured

What exists on main (read, not assumed):

- **One in-process MCP server.** `McpServer`'s eight public constructors
  all serve Streamable HTTP on `127.0.0.1` with an OS-assigned port.
  No stdio transport exists: the workspace pins `rmcp` `=3.1.2` with
  only streamable-http features, so AW1 must add an `rmcp` IO feature
  (measured gap, not assumed).
- **The seven-tool runtime.** `COMPACT_TOOL_NAMES` serves exactly the
  M36 seven from a cached compact authority; a per-instance allowlist
  (`served_tools_for_instance`) narrows it per session, and
  `call_exposed_blocking` rejects direct internal-capability calls. The
  served quad is pinned at 7 / 5,660 B / 3,510 B / 998 B for the
  nineteenth consecutive measurement.
- **Denylists and brokers exist.** The investigator constructor takes a
  `capability_denylist` plus bounded worker count, and
  `ConfirmationBroker::for_incident` auto-rejects destructive requests —
  the two mechanisms AW1's headless policy reuses.
- **The eval bin is the headless precedent.** `kinewright-eval.rs`
  already drives `Core`, media engines, and real harnesses with no GUI;
  AW1's CLI generalizes this shape to arbitrary projects.
- **App session startup is the wiring to copy.** `AgentThread::new`
  starts a branch server sharing the project-path handle and incident
  log; `start_agent_turn` builds the `SessionConfig`. Headless replays
  this without `eframe`.
- **Core is headless-ready.** `Document` serializes as `.kinewright`
  JSON with `skip_serializing_if` migration discipline; `Operation`
  validates through the `Core` actor with revision gating, undo, and
  journal; `TimelineBranch` already isolates one thread's work for
  merge/cherry-pick.
- **The render path already runs headless.** `FrameRenderer::render` is
  the one preview/export path; headless tests fall back to Mesa
  lavapipe; `Export`/`Analysis`/`Playback` are traits, so a CLI wires
  the real media engine without GUI-only objects.
- **The gaps are exactly the programme.** No `kinewright` CLI binary
  exists; no stdio server mode; no project lockfile; `ClipContent` has
  three variants (`Media`, `Title`, `Freeze`) with no code source; no
  whole-edit error aggregation beyond first-failure batch rejection; no
  state diff or frame explanation; no decision log (nearest precedent:
  the IN2B incidents sidecar); no shipped skill.

## 3. Slice order and dependencies

**Recommendation: AW1 first, then AW3, then AW2, then AW4** — one
reordering against the lead's implied AW1→AW2→AW3:

- **AW1 first (validated).** It unlocks everything else: every later AW
  slice is proven through an external client (stock Claude Desktop, not
  just in-process harnesses), and the thesis comparison cannot be
  measured without it. It is also the most parallel-safe slice (§12):
  a new bin plus an appended constructor, no `Operation` variant, no
  served byte moved.
- **AW3 second (reordered ahead of AW2).** Whole-edit plans, diffs, and
  `explain_frame` are pure `kinewright-agent` runtime work with no core
  model change and no renderer work. They make the external agent
  dramatically more capable per call, which is exactly what AW4 measures
  — and AW2's code clips are far easier to drive (and to debug, via
  `explain_frame`) once AW3 exists. AW3 can also start while AW1 is in
  review, since the files barely overlap.
- **AW2 third.** It needs MO2's compositing reality and MO1's
  `ClipContent`/`MediaKind` churn settled; after AW3 its keyframed
  parameters also get the cheapest commit path. Starting it before MO1
  lands would collide on every content-resolution match arm.
- **AW4 last (added).** The CC7/AU6/MO7 pattern: no feature, no tool —
  scenario authority, scripted agent/person paths, the Claude-Desktop
  comparison harness, and blinded review. Depends on AW1–AW3 and IN3's
  self-check.

Dependency edges: AW1 → AW3 (external provability) → AW2 (drivability)
→ AW4 (measurement). AW2 needs MO1's content-model churn settled; the
MO2 dependency is softened (S7) — code clips rasterise CPU-side like
`TimelineVisualLayer::Title` and alpha-over, so they need composite
*order*, not the sub-composite pass. AW3 consumes IN3's capability
packs when they land but must not block on them (byte budgets instead,
per the MO0 risk rule). AW4 consumes IN3's self-check and the MO7
critic-pass shape.

**Ruling 2026-09-25 (Riel): AW2 now precedes AW3** — both production
videos need code clips (animated text, shapes, diagrams); AW3 is
agent-runtime-only and may run alongside. The paragraph below records the
original reasoning.

**Rejected alternative.** AW2 before AW3 (clips you can barely drive)
and AW3 before AW1 (lethality with no external client to prove it).
The reorder costs nothing; code-clip gratification arrives one slice
later on a settled content model.

## 4. AW1 — Headless Kinewright

**Recommendation.** A `kinewright` CLI (twelve verbs: `new`, `open`,
`import`, `save`, `inspect`, `schema`, `apply plan`, `proof frame`,
`strip`, `check`, `export`, `branch`) plus a `kinewright mcp` stdio
mode serving the same seven-tool runtime — **proxying to the GUI's live
server when the project is open there, headless otherwise** (B2, the
Unity/Unreal MCP-bridge pattern). Every verb maps to an existing seam;
proofs and strips return images as file references, not inline bytes.

- **Proxy-first stdio.** The stdio process reads the lockfile (§10):
  GUI open → forward MCP calls to the GUI's live in-process server over
  loopback with the lockfile's Bearer [REDACTED] Edits land live in the GUI,
  undoable, revision-continuous. GUI absent → serve headless
  in-process against a local `Core`. Auth (Bearer [REDACTED] is added to the
  server, which has none today.
- **Transport/handler split.** The bind currently lives inside
  `start_configured_for_investigator`; AW1 splits the `KinewrightMcp`
  handler from its transport so stdio serves it via `transport-io`
  (present in `rmcp` 3.1.2 — enable the feature, no version change).
  Stdout stays protocol-only; all logging goes to stderr or a file.
- **Headless renderer (S6: feasible as specified).** `GpuContext::headless`
  with the lavapipe fallback, `FfmpegMediaEngine::new`, lazy audio;
  proofs via `Analysis::thumbnail_for_document`. Byte-identity needs a
  *pinned adapter* (the GUI's eframe adapter vs the CLI's LowPower pick
  must not diverge silently) — the AW1 brief pins it. FFmpeg: the same
  pinned FFmpeg 8 `third_party` libs or fail closed, never system FFmpeg.
- **Lifecycle (B4).** Registry-only `open_project`, `save_project`,
  `undo`, `revert_to_revision` plus CLI `new`/`import`/`save`. Save
  policy: atomic save after each commit in headless mode (temp+rename
  through the shared crate, B3). No new served tool.
- **Destructive consent (B5 reconciles the fail-closed default with the
  loop).** A destructive commit proceeds when the host grants per-call
  consent (MCP `destructiveHint` on the commit plus `elicitation` where
  the host supports it) or the invocation passes the explicit flag;
  otherwise it is refused with a typed error naming the flag — never
  silently. A snapshot is taken before every destructive commit. File
  scope is MCP roots / `--allow-root`: absolute paths inside roots are
  fine (so `import_media` works). The stderr prompt is deleted — it
  cannot work over stdio.
- **Skill v1 + MCPB ship here.** The Claude skill plus an MCPB (Desktop
  Extension) package so stock Claude Desktop installs the stdio command
  in one step; content grows in AW2/AW3.

**Rejected alternative.** HTTP-only headless and exclusive-lock
single-writer. Rejected: Desktop's MCP story is stdio commands, and an
exclusive lock forbids the core loop (person cutting while the agent
edits) since the GUI always opens for write. Proxying keeps one writer
(the live `Core`) with two clients.

**Cost.** One bin, the transport split, one `rmcp` feature, auth on the
server, the lockfile + proxy client (§10), the shared project-IO crate
(B3, moved logic), lifecycle ops (registry-only), consent plumbing,
skill v1 + MCPB. No served byte, no renderer change.

## 5. AW2 — Code-generated clips

**Recommendation.** A new `ClipContent::Code` whose source is code plus
parameters: `{ language, source, params }`, rendered deterministically,
cached by content hash, sandboxed, with provenance. Ship order: **SVG
first, then Lottie** (S8 — a known keyframed JSON motion format with a
Rust renderer, not a bespoke language with no model prior); Typst and
WGSL-shader sources are specified interfaces with deferred
implementations. Source is a hash-addressed blob (the LUT-store
precedent); only typed params animate, via `AutomationCurve` owners.

- **Reconciliation with MO0/MO6 (S7).** "SVG import is AE scope" stands
  — and code clips are not an SVG *import workflow* (no pen tools,
  paths, or artboard editing). They are *generated assets*: authored
  source rendered to pixels, like the CC4 built-in looks. Stated here
  so the boundary holds on both sides.
- **Declared descriptors + typed bindings.** Code-clip params get their
  own declared descriptor table (not `EFFECT_DESCRIPTORS` rows) with
  typed bindings to SVG ids/attributes and Lottie properties — no
  string templating into source, ever. Unbound ids fail closed.
- **Parameters, not source, animate.** Source blob immutable per
  `SetCodeSource` revision; only declared numeric params take curves,
  riding the keep-outside video rules from MO1.
- **Manim/Remotion: native-only for now (Riel ruled).** Execution stays
  closed; AW2 *specifies* (not builds) a render-import adapter — the
  host renders externally, `import_render` ingests an alpha image
  sequence / ProRes 4444 / FFV1, hash-addressed, with source/props/tool
  versions as provenance. Specified-only; external if ever built.
- **Parity goal (N1.1): build something equally capable ourselves**
  — Remotion/Manim parity for the common video-from-code jobs, natively:

| Job | Native feature | Gap, if any |
| --- | --- | --- |
| Animated text / typography | SVG text + Lottie text animators, hashed fonts | None targeted |
| Shapes and paths | SVG geometry + typed attribute bindings | None targeted |
| Easing and springs | `AutomationCurve` easings; spring solver if needed | Verified in AW2, else named gap |
| Data / chart animation | Param curves driving SVG chart bindings | None targeted |
| Math / equation display | Typst source (later slice) | Deferred, interface specified in AW2 |
| Sequencing / staggering | Clip-local frame evaluation + `Hold` keys | None targeted |
| Templates with props | Descriptors + bindings (§5) as the props surface | None targeted |

- **MO coordination.** Code clips rasterise CPU-side and alpha-over
  like titles (S7), so AW2 needs composite order, not MO2's
  sub-composite pass; MO6 may embed code clips as template fields later.

**Rejected alternative.** Shell-out rendering, the bespoke DSL, and
bundling Node/Python/Chromium (N1.1: never by default). All surrender
what beats code: typed, cached, reproducible, lean.

**Cost.** One `ClipContent` variant + renderer dispatch, one SVG
dependency and one Lottie dependency (pure Rust), the descriptor table
+ bindings, hash-blob store, param-curve owners, generated mutators
(registry-only). No served byte.

## 6. AW3 — Aggregate diagnostics, state diffs, explain-frame

**Recommendation.** Lethality capabilities plus the decision log.
Whole-edit plans already exist (`prepare_edit_plan` takes N ops);
S9 narrows the new work to error aggregation plus two inspectors:

- **Aggregate diagnostics.** A core aggregate `apply_batch` variant
  walks the clone collecting **all** per-operation errors with fix
  suggestions (independent failures all reported; dependent ones marked
  possibly-cascading), plus lenient `PlanOperation` deserialization so
  one malformed op does not void the other forty-nine. "Free" response
  growth is capped: diagnostics answer inside a pinned byte budget with
  a truncation marker, like every other payload.
- **State diffs.** Registry-only `get_timeline_diff` (brief confirms
  the name): compact change since revision N, backed by a
  revision-indexed snapshot query — the token-efficiency core of the
  thesis (no whole-timeline re-read per turn).
- **`explain_frame`, two tiers.** Tier 1: the layer stack at frame t
  via `visual_layers_at` (clips, effects, keys, mattes, code renders,
  grade nodes + evaluated values). Tier 2: per-region attribution,
  needing an isolated-alpha GPU pass — specified in the AW3 brief,
  deferred if the pass does not fit the slice.
- **Per-frame hash (S12).** A registry capability hashing rendered
  frames over a range — the cheap "did anything change" behind diffs,
  sheets, and AW4 re-run comparisons.
- **Project decision log.** Intent memory beside the project
  (`<stem>.kinewright-decisions.jsonl`, the IN2B sidecar precedent):
  timestamped goal/decision/rejected-alternative/author entries, CLI +
  registry readable first, GUI panel later (Riel ruled). Written
  temp+rename through the shared crate under the lock (B3) — IN2B is
  single-writer temp+rename, not append-only JSONL, and the log follows.
- **Comparison sheet lives here** (§11): branch diffs + QA + proofs as
  registry capabilities plus CLI `check` output, with batch proofs
  across branches in one call.

**Rejected alternative.** Whole-edit plans as a new served tool.
Rejected: it would move the served quad for the first time in twenty
measurements to buy nothing the existing plan argument cannot carry.

**Cost.** Core aggregate variant + lenient deserialize, revision
snapshot query, three registry inspectors with byte budgets, the
sidecar writer, comparison rendering. New `Operation`s: none —
aggregation reuses the existing variants.

## 7. Served surface and token budgets

**Recommendation: the served quad stays frozen through the whole
programme.** Every slice re-measures and asserts byte-identity
(7 / 5,660 B / 3,510 B / 998 B) in the existing pin sites, and records
registry growth in the M36 ledger exactly as AU4/AU5 did. Per-slice
impact:

- **AW1: registry-only.** Stdio serves the identical seven tools
  (transport is not surface); the CLI is not MCP at all. The four
  lifecycle capabilities (`open_project`, `save_project`, `undo`,
  `revert_to_revision`) arrive through `invoke_capability` and are
  measured in the ledger like any AU4/AU5 addition.
- **AW2: registry-only.** New generated mutators (`SetCodeSource`,
  param-curve setters) and any code-clip inspector arrive through
  `invoke_capability`. The `Operation` `$defs` growth rule applies
  (AU4's 820 B-per-tool precedent): params reuse `AutomationCurve`,
  source is a blob reference. Descriptions keep the 1,024 B budget
  with the load-bearing clause in sentence one.
- **AW3: registry-only plus capped response growth.** `get_timeline_diff`,
  `explain_frame`, and the per-frame hash are registry inspectors;
  whole-edit diagnostics ride the existing plan response (zero served
  bytes — responses are not in any `Tool` definition) inside a pinned
  byte budget (S9). The hard rule: `prepare_edit_plan`'s *input*
  schema is frozen — no new argument, however tempting.
- **AW4: zero.** No feature, no tool (the CC7/AU6/MO7 rule).

Token budgets (the thesis in numbers): each slice brief pins per-job
budgets — input/output tokens by category, tool calls, wall time,
corrections — and AW4 gates the programme on beating the code baseline
(§13). Memory/CPU budgets are named gates in every slice (N1.1): AW1 pins
headless peak RSS; AW2 pins per-frame render cost and cache bounds;
AW3 pins diff/explain compute bounds; AW4 measures all three against
the baseline's toolchain footprint. M36 discipline holds: load-bearing
first sentences, decimation within tolerance, byte budgets with fail-
closed summaries over budget.

**Rejected alternative.** A second, richer served surface for "power"
clients (full-surface server mode). Rejected: M36 deliberately killed
that mode, IN1 measured +35.5% served bytes for just two tools, and two
surfaces means two allowlists, two telemetry paths, and harness drift.
One surface, one quad, every client.

**Cost.** Ledger rows and pin-site assertions per slice; byte budgets in
three inspector/planner briefs. No code beyond what the slices already
carry.

## 8. Security: headless and code clips

**Recommendation.** The threat model flips in headless mode, and the
design says so plainly:

- **Today the app sandboxes the agent** (scratch working directories,
  harness allowlists, the broker). **Headless inverts it: the agent host
  is outside our sandbox** (stock Claude Desktop running the user's own
  tools), and the CLI is the thing that must not be tricked. The CLI
  therefore treats the project file as untrusted (typed load validation
  plus explicit rejection of unknown `ClipContent` languages/versions),
  every op goes through Core validation, and destructive commits need
  per-call consent or the explicit flag with a pre-commit snapshot (B5).
  File scope is MCP roots / `--allow-root`: absolute paths inside roots
  are allowed, so `import_media` works; anything outside fails closed.
  The loopback proxy is authenticated (0600 Bearer [REDACTED], B2) — no
  unauthenticated local port.
- **Code clips are sandboxed by construction, not by container.**
  Renderers are pure functions over `(source_bytes, params, frame) →
  pixels`: no path access, no network, no clock, no process spawn in the
  render call. SVG needs no execution at all; Lottie evaluates
  keyframed JSON with no IO. Resource bounds are explicit: output
  resolution cap, per-frame CPU timeout, render memory cap derived from
  dimensions; over budget fails closed with a typed incident, never a
  partial frame served as final.
- **Provisioning stays pinned.** The CLI locates the same pinned FFmpeg
  8 `third_party` build the app uses and fails closed without it.
  Fonts are hashed assets (S12, the LUT-store precedent), not a system
  fallback — "same source, different machine" cannot diverge.

**Rejected alternative.** OS-level sandboxing (seccomp, containers).
Rejected as disproportionate: the renderers take no IO, so the sandbox
is the function signature plus budgets. A future executing language
re-opens this section.

**Cost.** Path-resolution guard in CLI + plan apply, per-frame render
budgets, bundled-font decision in the AW2 brief, typed incidents for
each refusal. No new dependencies, no platform-specific sandbox code.

## 9. Determinism and caching

**Recommendation.** Same bytes in, same pixels out — across runs,
machines, and GUI/headless:

- **Code-clip cache key** is `hash(source_bytes, evaluated_params,
  output_size, renderer_name, renderer_version)`. Renderer version in
  the key is load-bearing: a renderer upgrade invalidates silently
  otherwise. Cache entries live under the M41 managed-cache discipline
  (scoped inspection, clearing, byte accounting) — visible in the same
  UI, clearable by the same operation.
- **No nondeterminism sources.** No wall clock, no RNG without a
  seed carried in params, no system-font fallback (§8), no thread-count-
  dependent float summation in the renderers (fixed-point or pinned
  order, the AU1 summation precedent). SVG renderers that internally
  multithread must still produce byte-identical output — verified, not
  assumed, by the AW2 cross-run gate.
- **Headless/GUI agreement.** Both drive `FrameRenderer::render` and the
  same `Core` validation; on the proxy path (§4) they are literally the
  same renderer. Headless CLI-proof/GUI-proof identity holds on the
  pinned adapter (S6) with the lane named per gate (the MO0 lane rule):
  byte for byte at the same revision, or the gate fails.
- **Plan determinism.** Whole-edit plans commit atomically with the same
  revision gate as today; replaying a recorded plan against the same
  base revision yields the same document (the M31 merge property,
  extended to CLI-applied plans).

**Rejected alternative.** Cache by clip ID + revision. Rejected: it
breaks when two clips share source and cannot deduplicate across
projects or undo cycles. Content hashing is the feature.

**Cost.** Hashing at render dispatch, version constants per renderer,
one cross-run/cross-lane differential suite in AW2, CLI/GUI proof
identity gates in AW1. The shared project-IO crate is moved logic.

## 10. Person and agent sharing one project

**Recommendation: proxy to the live writer; the lockfile is discovery,
not exclusion.** The `Core` actor is single-process and undo/branches
are in-memory — so co-editing goes *through* the live process, never
around it (B1, B2).

- **Lockfile as discovery.** `<stem>.kinewright.lock` beside the project
  holds the GUI's loopback endpoint + a 0600 Bearer [REDACTED] when the GUI
  has the project open. `kinewright mcp` reads it: endpoint live →
  proxy every call to the GUI's server (edits land live, undoable,
  revision-continuous — this is the only path that claims GUI undo).
  GUI absent → the CLI serves headless in-process and owns the lock.
- **Headless writes go through shared project IO (B3).**
  `write_project_document`, digest pairing, `can_overwrite_save`, the
  sidecar writer, and the recovery-journal interplay move from
  `pub(crate)` in the app to a shared crate both processes use, so
  headless saves keep IN2B's digest pairing, sidecar-before-project
  order, and journal. The lock covers project + sidecar + journal.
- **Write contention fails closed with a handoff.** Two headless
  writers, or headless vs a GUI that appeared mid-command: the loser
  refuses with a typed error and writes the validated plan to a
  `.plan.json` the GUI imports as a branch — never a surprise live
  mutation, no last-writer-wins. Stale locks (dead pid) reclaim with a
  typed warning in the decision log, never silently.
- **Decision log under the lock.** Written temp+rename through the
  shared crate (§6), covered by the same lock — no lock-free appends.

**Rejected alternative.** Exclusive-lock single-writer (forbids the
person-cutting-while-agent-edits loop, since the GUI always opens for
write) and live multi-writer (re-implements `Core`'s revision
discipline across processes). Proxying keeps one writer with two
clients; branches still solve "two editors, one project" for the
handoff case.

**Cost.** Lockfile + proxy client, server auth, the shared project-IO
crate (moved logic), plan import/export. No Core change.

## 11. Branches, sub-agents, comparison sheet

**Recommendation (deciding the lead's open placement):**

- **Branches stay M31 and gain a CLI surface in AW1.** `TimelineBranch`
  already isolates one thread's work with merge/cherry-pick/discard and
  a provenance ledger. AW1 exposes it via the `branch` verb group — on
  the proxy path (§10) branches are the GUI's own live branches; only
  headless mode mints file-local ones. No new branch semantics.
- **Sub-agent fan-out stays a harness concern, not a Kinewright
  feature.** Parallel animation agents (the video-use study's lesson 6)
  are spawned by the external harness (Codex/Muse sub-agents); what
  Kinewright owes them is cheap branch creation and the comparison
  surface to judge their outputs. Kinewright never orchestrates model
  calls outside its own investigator sessions (IN2) — building a second
  orchestrator would duplicate every harness's job badly.
- **The comparison sheet lives in AW3.** One rendered view over N
  branches: per-branch diff against base (AW3's state diff), QA report,
  proof frames/strips, token/time cost, and the merge/cherry-pick
  actions. Delivered as registry capabilities (reached through the
  unchanged seven tools) plus CLI `check --compare` output and a GUI
  panel. It is AW3's because it is composed almost entirely of AW3
  primitives (diffs, proofs, whole-plan apply).

**Rejected alternative.** A Kinewright-native sub-agent orchestrator.
Rejected: orchestration is the harness's product surface; Kinewright's
advantage is making each thread cheap, isolated, and comparable.

**Cost.** CLI branch verbs, comparison capabilities from AW3 parts, one
GUI panel. No new model calls, no new Core semantics.

## 12. Slices AW1–AW4

Gates are named behaviours that fail today. Every gate names its lane;
"served quad unchanged" appears on every slice by the §7 rule.

**AW1 — Headless Kinewright.** CLI verbs + proxy-first stdio + server
auth + lockfile discovery + shared project-IO crate + lifecycle ops +
consent plumbing + branch verbs + skill v1 + MCPB. *Exit gate:* with
no GUI running, a stdio client opens a project, splits a clip, commits,
and pulls a proof; the GUI then opens the same file with the change
present and provenance showing one CLI commit (B1 restatement — no
cross-process undo claim). With the GUI open, the same session proxies:
the edit lands live and one GUI undo removes it. Destructive without
consent refuses with the flag named; a `RippleDeleteClip` with consent
commits. CLI proof equals GUI proof on the pinned adapter; headless
peak RSS inside its pinned budget; served quad unchanged. *Depends:*
nothing. *Size:* extra large, three parts.

**AW3 — Aggregate diagnostics, state diffs, explain-frame.** Aggregate
`apply_batch`, `get_timeline_diff`, two-tier `explain_frame`,
per-frame hash, decision-log sidecar, comparison sheet with batch
proofs. *Exit gate:* a 50-op plan with three seeded errors (one
malformed) returns all three plus fixes in one call inside its budget;
a diff since revision N answers a cut-point question inside budget;
tier-1 `explain_frame` names every stack contributor; two branches
render one sheet with diffs, QA, batch proofs, and costs; diff/explain
compute inside budget; served quad unchanged. *Depends:* AW1. *Size:*
medium-large, two parts.

**AW2 — Code-generated clips.** `ClipContent::Code` with SVG + Lottie
sources, declared descriptors, typed bindings, hash-blob store,
param curves, sandbox bounds, provenance; Typst/WGSL specified but
deferred; render-import adapter specified, not built. *Exit gate:* an
SVG lower third byte-identical across runs and both CI operating
systems; a Lottie animation's params survive trim-in-then-out; an
over-budget render fails closed with no partial frame; unbound ids
fail closed; per-frame render cost and cache bounds inside budget;
parity table green except named gaps; served quad unchanged. *Depends:*
AW1, AW3; MO1 content model settled. *Size:* large, two parts.

**AW4 — Workflow evaluation.** Scenario authority, scripted agent/person
paths, the §13 comparison harness (Claude Code `-p` both arms), blinded
review; no feature, no tool. *Exit gate:* all gates green on both CI
operating systems with lanes named; N runs per task per arm with setup
time counted; engine wins tokens/time/corrections and peak RSS
against the full-strength baseline; human reviewer left only creative
questions.
*Depends:* AW1–AW3, IN3 self-check. *Size:* medium, one part plus the
real-harness runs and blind review.

**Parallelism with MO1/MO2 (file-conflict analysis).** AW1 runs parallel
with MO1 except the shared-crate move touches app-owned IO call sites —
that cutover serialises. AW3 and MO2 both edit `INSPECTOR_TOOL_NAMES`
and the registry pin sites (S11): landing there serialises, one slice
re-pins after the other. AW2 starts once MO1's `ClipContent` match arms
are stable; its MO2 coupling is order-only (§3).

## 13. Evaluation matrix vs "video from code"

Both arms run Claude Code `-p` stream-json (S10 — Desktop cannot run
in CI; Desktop + MCPB is a manual smoke). The baseline is full
strength: ffmpeg scripts video-use style **plus Remotion/Manim with
their official skills**; both arms keep bash. Same tasks, same
fixtures, same model; N runs per task per arm with setup time counted;
quality blinded. Dimensions: wall time, tokens by category, tool calls,
corrections, peak RSS and per-frame render cost, and output quality
through structural → technical → creative in order (passing the first
two never earns "excellent").

| Task | Baseline may use | Engine must win on |
| --- | --- | --- |
| Silence-cut + captions on a podcast | ASR, gap ranking, filtergraph concat, srt burn-in | Tokens/corrections: diffs + aggregate plans vs re-emitted filtergraphs; cut-default correctness |
| Animated lower third over an interview | Remotion/Manim + official skills | Iteration speed: SVG/Lottie clip + param curves + instant proof vs render-loop scripting |
| Reframe 16:9 to 9:16 with caption safety | Crop arithmetic, caption re-layout | Correctness: deterministic cover-crop + safe-area QA |
| Match two cameras + deliver tagged | FFmpeg colour work, tag flags | Technical gate: managed pipeline + scopes + decoded-output verification |
| Revise after review ("tighter, swap take 2") | Re-run, re-verify | Revision cost: one branch + plan + undo; decision log carries intent |

Engine properties gated alongside (measured, not assumed): changes
undoable, plans revision-gated, renders explained, projects
GUI-openable — each scored per task on both arms, so the comparison is
honest about what code actually recovers by re-running.

## 14. Risks and decided questions

Risks:

- **AW1 is now the biggest slice.** Proxy + auth + shared-crate cutover
  + lifecycle + consent is a three-part slice; if it runs hot, split
  the cutover from the proxy rather than thinning either.
- **AW2's renderer dependencies are the programme's new third-party
  trust.** SVG + Lottie crates and hashed fonts must be license-clean,
  deterministic, and cross-platform. The AW2 brief picks the crates and
  records the license verdicts.
- **Registry pin-site contention (S11).** AW3/MO2 landings at
  `INSPECTOR_TOOL_NAMES` serialise; whoever lands second re-pins.
- **AW4 needs IN3's self-check.** If IN3 slips, AW4 gates on QA + lane
  renders alone and records the comparison as owed.

Decided since revision 1 (Riel's answers + N1, recorded here so the
questions are closed, not dropped): CLI bundled with the desktop app;
destructive fail-closed unless per-call consent or the explicit flag,
reconciled with the loop via B5; SVG first, then Lottie as the motion
format; Manim/Remotion native-only-for-now with the render-import
adapter specified, built only if AW4 shows need; decision log CLI +
registry first, GUI panel later.

Open questions: none. All five revision-1 questions are answered above.

## 15. Future work

Deferred with owners, not rejected:

- **Typst and WGSL code sources.** Owner: a future AW code-depth slice;
  interfaces specified in AW2, implementations unbudgeted.
- **Manim/Remotion render-import adapter.** Specified-only in AW2;
  external if ever built, only if AW4 shows need (Riel ruled).
- **Multi-machine project sharing.** Owner: a future collaboration
  slice; the lockfile assumes one machine. Network drives get
  fail-closed behaviour, not corruption.
- **Richer decision-log consumers.** Owner: creator-packages scope;
  AW3 ships the log, not the consumers.
- **Live multi-writer.** Owner: none — explicitly not on the roadmap
  (§10); proxying plus branches plus handoff is the sharing model.
