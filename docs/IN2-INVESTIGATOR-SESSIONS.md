# IN2 — Investigator sessions (Part A)

Status: contract revision 2 — promoted 2026-09-16, after one brief critique, one contract critique and two probes
Programme: [Roadmap and workflows](ROADMAP-AND-WORKFLOWS.md) "Investigator and harness programme", row IN2 at `:805`
Discharges: [IN1](IN1-INCIDENTS-AND-THE-COLOUR-CASE.md) **§13 D5, D6, D9**; and the IN2 half of the roadmap row as amended by §9's table
Depends on: IN1 Part A (`29fad67`, `d4ed8eb`) and IN1b (`cdb4691`, `7e85972`)
Companion: **Part B**, `docs/IN2B-INCIDENT-PERSISTENCE.md`, drafted after Part A lands, carrying IN1 §13 D1/D2 and IN1b §13 D-B2/D-B3/D-B4/D-B6 (§13)
Scope: **a person looking at an `Explain` card whose code has no deterministic recovery gets a typed proposal from an investigator session, and on one approval that fix is applied on the live core with provenance, an undo entry and a card — inside the policy, inside named budgets the card shows, with nothing destructive at any point; and with no harness installed every code that has a deterministic recovery is one button.**

The words **must**, **must not** and **may** are normative. Rules are numbered inside their section so a critic can cite "§3 rule 12". Every code claim carries `file:line` at **`7e85972`** and was re-read for this revision. A figure that is arithmetic over measured components says so. Revision 1's **[probe-2]** markers are gone: every one is now the measured value from `target/review/in2/probe2-report.md`, except **§10 figure 2** (the three budget defaults, which nothing in the tree could run before §8 lands — marked **[impl]**) and the three registry **byte** figures, which are carried as probe-2's measurement **plus its decomposition**, because they are a function of description text this contract deliberately does not fix (N2.5).

IN2 Part A adds **no served MCP tool** and **no `Operation` variant**. The served quad stays **7 / 5 660 / 3 510 / 998** for the **eighteenth** consecutive measurement (probe-2 T4, measured with `propose_fix` registered), asserted unmoved in its **three** value sites and re-worded in its **five** counter-word sites (§9 clause 18). `served_tools()` filters `capability_tools()` by `COMPACT_TOOL_NAMES` (`crates/kinewright-agent/src/server.rs:727-732`, `crates/kinewright-agent/src/runtime.rs:17-25`) and Part A touches neither list. The **registry sextuple does move**, by exactly one registry-only capability (`propose_fix`) and three `destructiveHint` annotation flips, over **six** pin sites named in §14 row D.

---

## 0. Change log

### 0.1 Rulings carried from `target/review/in2/orchestrator-notes.md`, by id

Every ruling of **N0**, **N1**, **N2** and **N2.5** is binding and is not relitigated. Each is carried here by its id with the section that discharges it. Where a ruling accepts a critic finding or a probe measurement, that id is carried with it.

**N0 — framing.**

- **N0/Q0 — the roadmap row's exit gate is restated and the row amended at promotion.** §9's discharge table, §14's docs row.
- **N0/Q0.1 — two parts, Part A the session end to end.** Header, §1, §13.
- **N0/Q0.2 — the model boundary**: a session acts within `PolicyClass`; "zero destructive actions" is three checkable properties; the person sees `Investigating` with turn, token and time counters. §3.6, §5.5, §6.
- **N0/Q0.3 — token efficiency**: what the session is given, measured in bytes at session start, and the budget defaults. §3.4, §5, §10.

**N1 — the brief, the brief critic and probe-1.**

- **N1/Q1 — two contracts from the start.** `docs/IN2-INVESTIGATOR-SESSIONS.md` (this file) and `docs/IN2B-INCIDENT-PERSISTENCE.md`. Erratum ranges: core R1–R19, media R20–R39, app R40–R59, agent R60–R79. Header, §14.
- **N1/Q2 — the exit gate is restated and every clause asserts a positive count** so none passes vacuously (brief critic B2). §9.2.
- **N1/Q3 — A then B.** §13. **N1/Q14 — all six deferrals in Part B**, D-B2(a) the first cut. §13.
- **N1/Q13 — dedup key `(code, subject)`; one running session per project; a queue behind it.** §3.2.
- **N1/proposal, not confirmation** — the session *proposes*; a new pure fn `proposal_card` renders the proposal beside `incident_card`, whose call sites do not change; **the app** applies and records. §4. *(The proposal's own shape is N2.5/B1's, below.)*
- **N1/B7 — the investigator cannot record outcomes.** `resolve_incident` is not reachable from an investigator session; the app records every outcome. §4.5, §6 rule 4.
- **N1/critic Finding 2 — the refused `Operation` is not put on the wire**; the app keeps it beside the queued incident (the `Event::OpRejected` drain arm already holds `op`, `crates/kinewright-app/src/app.rs:1369-1385`) and composes it into the opening message. §3.4.
- **N1/Q5 — a branch core with `Core::spawn_at(document, revision)`** so the branch counter starts at live's. §3.3 rules 14–16.
- **N1/B8 — no re-base.** On approval the app applies the proposal's operations through `apply_to_live` at the current live revision, retries **once** on `Conflict`, and on a second conflict leaves the incident `Open` with the proposal stale and a **Re-investigate** action. §4.4.
- **N1/Q6 — re-scoped.** `ConfirmationRequest` gains `incident: Option<IncidentId>`; a broker request raised inside an investigator session is a policy violation, auto-rejected, attributed and recorded. `pending_requests()`'s draining semantics are unchanged. §4.6.
- **N1/Q7 — `IncidentOutcome::Rejected` only.** `ResolveIncidentOutcome` gains nothing, so the 809 B `resolve_incident` input schema does not move (`server.rs:26139`). §4.5 rule 21, §6.4.
- **N1/Q8 — `IncidentState::Investigating` counts as open**; `IncidentState::is_open()` replaces the silently-changed `==` sites and the one exhaustive match; wire value `"investigating"`. §3.6.
- **N1/B1 — the deterministic sixteen.** The 16 `Explain` rows probe-1 P8 names gain `RecoveryKind::Operation` through `policy_recovery` **where the operation is buildable from the evidence**; class unchanged; the card's button and the no-harness fallback come from the same arm. §7. Membership: N2's ratified narrowing to **3**, confirmed by probe-2 T2.
- **N1/S16 — a session starts only for codes on a named allowlist.** §3.1, with the measured list and its count of **54** printed in full (N2.5/B7).
- **N1/Q4 — no `AskFirst` rows in IN2.** The 11 codes probe-1 P8 names are IN3's `AskFirst` candidates and are **on** Part A's allowlist. §3.1 rule 6, §13 D-A1.
- **N1/B3 — "zero destructive actions" is three checks** (i) the session's tool surface and the server-side capability denylist, (ii) the proposal's operations checked against the seven-variant set **before** the proposal is recorded, (iii) the broker gate widened to all seven in both description functions. §6.
- **N1/Q9 — turns at the driver; wall time and tokens in a non-feature-gated session pump in the agent crate, with one owner.** §3.5, §5.1–§5.3.
- **N1/Q10 — a per-user config file in the platform config directory**, not eframe persistence and not egui memory; per-project preferences in the `Document` as one optional field. §2.
- **N1/Q12 — `ScriptedDriver: AgentDriver` is production code in `kinewright-agent`, not feature-gated**, holding its own MCP client. §8.
- **N1/auth `Unknown` is treated as available**; a first-session failure reports through the card and disables the harness for the app session, never permanently. §2.2 rules 7–8.
- **N1/off switch mid-session, person resolves by hand mid-session, incident resolved elsewhere** — the session is cancelled, its branch dropped, the outcome recorded by the app, never left `Investigating`. §3.7, as amended by N2.5/B3.
- **N1/two projects** — one running session per project, concurrent sessions capped app-wide at **2**. §3.2 rules 9–11.
- **N1/corrections carried** — D11's telemetry additions re-measured; `mirror_agent_cost` has **zero** production callers (`crates/kinewright-agent/src/server.rs:16352`, exported `lib.rs:48`, called only at `server.rs:26745`, `:26767`, `:26782`) and Part A adds the first; the docs calibration is **162**; `--workspace` does compile `eval` (`crates/kinewright-agent/Cargo.toml:18`). §5.4, §12.

**N2 — contract draft v1.** Decisions **a–j** ratified and carried into this revision, three of them amended by N2.5 and said so here:

- **a** — JSON config at `<config>/Kinewright/investigator.json` with no new dependency (§2.1). **Amended by N2.5:** the path rule is probe-2 T10's three-branch resolution and the `recovery.rs` "precedent" claim is deleted (§2.1 rule 1, §0.2 probe 11).
- **b** — mutes as `Vec<String>` of `code()` strings (§2.4). **c** — `IncidentProposal.operations` `#[serde(skip)]` with a bounded wire summary (§4.2). **d** — a 240 B explanation ceiling (§4.1 rule 5; **amended by critic S3** to bound the *serialised* string). **e** — the broker stamps `incident` (§4.6). **f** — a 5 s investigator broker timeout (§4.6 rule 25). **g** — a server-side `capability_denylist`, deliberately not D11's mechanism (§6 rule 2; **amended by N2.5/B9** from 6 names to **9**). **h** — an opening-operation ceiling of 4 096 B (§3.4 rule 23). **i** — order **A → D → C** (§14). **j** — the allowlist as a named `const` with its criterion beside it (§3.1).
- **N2** also ratified §0.3 D1 (three buildable colour rows, not ten) and §0.3 D2 (the registry sextuple moves, the served quad does not), and upheld the drafter's partial dispute of brief-critic B1. Both are now **measured** (probe-2 T2, T4) and are no longer deviations; they are carried as §7 rule 4 and §6.4.

**N2.5 — the contract critic and probe-2.** Every ruling below is discharged by the section named.

- **N2.5/B1 — the branch IS the proposal.** `propose_fix { incident, explanation }` takes **no operations** and has **no revision gate**: it snapshots the branch's applied-operation list, runs check (ii) over it, and records it. §4.1, §4.2.
- **N2.5/B2 — the pump's execution model.** One dedicated OS thread per session, owned by the app's investigator module; the pump owns the `AgentSession`, sends the opening message, loops on events, counts turn boundaries, enforces wall time and tokens between events, publishes counters through an `Arc` of atomics the UI reads each frame, and observes a cancellation flag. `pump_session` gains a prompt source, a five-variant `SessionStop`, and a confirmation policy. One owner at `crates/kinewright-agent/src/session.rs`. §3.5.
- **N2.5/B3 — two different ends.** A person's explicit **Reject** → `Resolved(Rejected)`, which suppresses. A budget stop, the off switch, a policy violation, a disconnect or a hand-resolve elsewhere → the incident **returns to `Open`** with the session's telemetry recorded and **no** suppression, and the card shows *"investigation stopped: &lt;reason&gt;"* with **Re-investigate**. IN1 §2.3 rule 19 is **unamended**. §3.7, §4.5.
- **N2.5/B4 — `ScriptedCost` has six fields; `mirror_agent_cost` accumulates; the pump is its first production caller.** §5.4, §8 rule 4.
- **N2.5/B5 — check (iii) is the branch server's broker**, with the 5 s timeout and `RejectAndStop`, at run time; check (ii) at `propose_fix` is the second net. Clause 3 asserts zero requests over the resolving scripts **and** a destructive-attempt script asserts exactly one auto-rejected request. §4.6 rule 27, §6 rule 5, §8 script S11.
- **N2.5/B6 — the one-owner property is asserted per crate by source read**, each crate reading its own files, CRLF-normalised. §9.1 items 35–36, §9.2 clause 16.
- **N2.5/B7 — the allowlist is 54.** The six rows probe-2 measured unbuildable join it; `delivery_verification_frame_count_out_of_range` joins its twin. The 11 `AskFirst` candidates are on it. §3.1.
- **N2.5/B8 — §3.1 rule 5 asserts three**, not nine. §3.1 rule 7.
- **N2.5/B9 — the denylist is derived from the 12-row side-effect inventory**, printed with the resulting list and its count, with a test in both directions. §6 rule 2.
- **N2.5/probe pins** — 819 unmoved; ceiling **4 096**; served quad eighteenth; registry decomposition; `spawn_at` 24/5; `is_open()` 21/8 with five changed comparisons; `Rejected`'s two compile sites; 52 literals; fixture 1 215 B / `c9da3186e131e4fd`; `IncidentProposal` derives `Eq`; the four opening-message counts (§3.4 rule 24); per-session start bytes; zero pinned description texts move; the bin-only sentence pinned as measured; **+22 threads** answered by a **2-worker** runtime; T10's config path; `rmcp` streamable-HTTP client at 70 lines; §10 figure 2 is **[impl]**; `IncidentTelemetry` drops `Copy` at zero cost. §§2, 3, 4, 6, 8, 9, 10.

### 0.2 What revision 2 changed, by id

**Critic blocking, all nine accepted** (N2.5).

| id | change | where |
| --- | --- | --- |
| **B1** | `propose_fix` loses `operations` and `expected_revision`; the branch's `Query::AppliedOperations` is the proposal; refusals go 8 → **6**; `branch_revision` becomes `base_revision` | §4.1, §4.2 |
| **B2** | the pump's execution model written in full: one OS thread, the nine-argument signature, five-variant `SessionStop`, `ConfirmationPolicy`, `Arc` counters, cancellation flag | §3.5, §3.2 rule 8 |
| **B3** | six of the seven `Rejected` rows become *returns to `Open`* with telemetry and no suppression; only the person's **Reject** resolves | §3.7, §4.5 rule 22 |
| **B4** | `ScriptedCost` grows to six fields; the pump **accumulates** and mirrors once, at resolution | §5.4 rule 11, §8 rule 4 |
| **B5** | §4.6 rule 27's stated reason replaced with the true one; script **S11** added; clause 3's zero made a measured zero | §4.6 rule 27, §6 rule 7, §8 |
| **B6** | the one-owner test reads **both** crates' own sources, matches a regex over three shapes, allowlists the known non-session sites by file and asserts a positive count | §9.1 items 35–36, clause 16 |
| **B7** | the allowlist is **54** — POLICY rows 14–67, contiguous — with the criterion split into its two reasons and both columns printed | §3.1 |
| **B8** | §3.1 rule 7's converse is asserted over the rows §7 rule 4 marks **yes**, which probe-2 measured as exactly three; row 3's marker dropped | §3.1 rule 7, §7 rule 4 |
| **B9** | the denylist is **9** names, derived from the printed 12-row inventory, with a test in both directions | §6 rule 2 |

**Critic substantive.** S1 `Eq` added and the deserialised-proposal rule written (§4.2 rules 8, 10; §13 D2). S2 the `recovery.rs` precedent deleted, the line range corrected to `:774-779`, and `default_recovery_directory`'s own Linux/macOS defect deferred as **D-A7** (§2.1 rule 1, §13). S3 the explanation ceiling bounds the **serialised** string (§4.1 rule 4). S4 the outcome table gains `Err(BranchError)` and drops `NoChanges`; rule 13's *"by value"* corrected (§4.4). S5 **18** call sites, 15 / 2 / 1, in all three places (§0.1, §4.3 rule 13, R-E). S6 `remove_bin`, `remove_string_out` and `remove_sync_group` gain `destructive(true)`; `destructiveHint` goes 12 → **15** (§6 rule 5, §14 row D). S7 the cut-list preamble names the clause each cut changes (§12). S8 the envelope is **331 B** whole-result, 195 B of it structured (§3.4 rule 20). S9 implementer **B is deleted**; `kinewright-media` is implementer A's (§14). S10 `state_label` gains **two** arms, four today, six after (§3.6 rule 33, §4.5 rule 21, §14 row C). **S11 is disputed — §0.3 D1.** S12 `spawn_at`'s real benefit restated with the queued-session limit (§3.3 rule 16, §10 limit 10). S13 the router **sends** in the observing tick (§6 rule 8). S14 figure 5's baseline is the app's existing per-thread servers (§10 figure 5, §3.2 rule 11). S15 the lock discipline written (§4.4 rule 18, §5.4 rule 11).

**Critic nits.** N1 `opened_at`'s skip is `incident.rs:1170-1171`. N2 `Core::spawn` is `actor.rs:171-179`, `CoreState::new` is `:248-256`. N3 `serde_json` is `crates/kinewright-app/Cargo.toml:20`. N4 the seven destructive variants span `runtime.rs:349-363`. N5 `Operation` has **57** variants counted directly over `crates/kinewright-core/src/operation.rs:23-387`, not inferred from 54 + 3. N6 the ×1.45 band states its rounding (§12). N7 `max_turns` is clamped to `1..=64` at the settings boundary (§2.1 rule 2). N8 §7 row 3's marker dropped.

**Probe-2 disagreements, all sixteen folded.** 1 `Eq` (§4.2). 2 the ceiling's cause is the proposal alone at 2 057 B and the constant is **4 096** (§4.2 rule 9). 3 the divisor holds by **135 B**, stated (§4.2 rule 9). 4 the loop is the pin, not a remembered 1 737 (§4.2 rule 9, §9.1 item 30). 5 **six** registry pin sites including `cc7_the_agent_surface_is_unchanged_by_this_slice` (§6.4, §14 row D). 6 the byte figures carry their decomposition (§6.4). 7 all six rows are **no**; clause 4 asserts **3**; "3 + N" resolves to **3 of 67** (§7, §14's roadmap row). 8 §7 rule 5 states predicate **plus** reachability. 9 the pump's four signature traps (§3.5 rule 29). 10 the bin-only sentence pinned as measured (§6 rule 6). 11 the config-path claim deleted and T10's resolution made the rule (§2.1). 12 **five** changed comparisons, and `Rejected`'s **two** compile sites priced (§3.6, §4.5, §12). 13 figure 2 is **[impl]** (§10). 14 **+22 threads**, answered by a 2-worker runtime (§3.3 rule 13, §10 figure 5). 15 §2.4 rule 12 says **neither** surface moves. 16 the `Copy` fallout is **0** (§10 limit 9).

### 0.3 Deviations revision 2 makes from a ruling or a finding, with evidence

**One.**

- **D1 — critic S11 is disputed: `policy_entry` does exist.** S11 says §9.1 item 1's `policy_entry(code)` "does not exist" and that the test must scan `POLICY` instead. It exists at `crates/kinewright-core/src/incident.rs:2223` — `const fn policy_entry(code: IncidentCode) -> PolicyEntry { POLICY[code.table_index()] }` — and returns the whole row, whose `class` **and** `predicate` are both `pub` fields (`incident.rs:1640-1649`). It is private to the module, and §9.1 item 1 is a unit test **in that module** (`crates/kinewright-core/src/incident.rs`, the house's own location for the IN1/IN1b policy tests), so it is reachable without a single line of new surface. S11's half that reproduces — that `policy_class(code, evidence)` cannot expose the predicate — is the reason item 1 calls `policy_entry` rather than `policy_class`, and the contract says so. **Nothing is added to §14's core row for this.** The rest of S11's remedy is folded: item 1 asserts over the row, not over a re-derivation.

### 0.4 Decisions revision 2 makes that no ruling covers

Each carries its reason. They are here rather than buried so a reviewer can refuse one without reading §3.

- **k — `SessionStop`'s five variants are `Completed | Budget(BudgetKind) | Cancelled(StopReason) | Disconnected | PolicyViolation`, and the harness failure lives inside `StopReason`.** N2.5/B2 pins the five; probe-2's prototype needed eight, and the two extra it needed beyond the five — a per-budget discriminant and a harness error — are exactly what `BudgetKind` and `StopReason::Harness(String)` carry. §2.2 rule 8's one-strike disable needs to tell a harness failure from an off switch, and a flat `Cancelled` cannot. §3.5 rule 28.
- **l — `IncidentProposal` keeps `operation_count` and `stale` beside N2.5/B1's four fields.** `stale` is asserted by clause 18 and is what critic S1's deserialised-proposal rule sets; `operation_count` is what the card's headline reads when `summary` has been truncated. Both are one word each on the wire. §4.2 rule 8.
- **m — implementer B is deleted and `kinewright-media` belongs to implementer A** (critic S9's first option). A owns the `Document` field, so A owns its 20 media-crate literal edits; a second implementer whose entire brief is "edit no file" is an implementer whose `cargo fmt` lane nobody runs. The R20–R39 erratum range stays reserved. §14.
- **n — the three registry byte figures are pinned at probe-2's measurement *and* carry their decomposition and the S6 delta.** Probe-2 measured `1 552 577 / 1 407 717 / 121 726` with its own 411 B description; critic S6's three `destructive(true)` flips move `serialized_bytes` again by an amount nobody has measured. The contract therefore pins the three **counts** (141 / 54 / 87) hard, and states the three **bytes** as probe-2's measurement plus `+160 B fixed + schema + description + the three annotation flips`, re-pinned from the implementer's own measurement in the same commit. §6.4, §9 clause 18.
- **o — `pump_session` carries `#[allow(clippy::too_many_arguments)]` with a written reason**, in the shape `start_configured` already has (`server.rs:507-511`). Nine arguments is what N2.5/B2's ruling costs; hiding them in a struct would hide which of them the two callers disagree about. §3.5 rule 28.
- **p — a session that returns its incident to `Open` writes a `stopped_reason` onto telemetry, not onto the incident's own fields.** `IncidentTelemetry` is already the one place a resolution's provenance lives, and `Incident` gains no second optional string to price against the ceiling. §3.7 rule 38, §5.4 rule 12.

### 0.5 Errata recorded when the app stage landed (2026-09-16)

Each names the rule it amends and the review finding or lead ruling behind it (`target/review/in2/impl-app.md` §8, `impl-app-fixes-brief.md` F1–F15, `impl-app-brief.md` §8). None changes a §9 clause.

- **E-C1 — a recorded proposal outlives the end of its session** (§4.3, §4.4 rules 18–19; review F1). A session that records a proposal and then ends on a budget leaves the incident `Open` *with* a fresh proposal, and that proposal stays actionable: **Approve** and **Reject** accept a fresh proposal in either `Open` or `Investigating`; **Re-investigate** from `Investigating` ends the running session first, then re-enqueues; `begin_investigation` stales the old proposal. The card's action set is derived from the same predicates the press paths use, so a shown button never returns `false`: **Re-investigate** is hidden while a session for this incident's `(code, subject)` pair is running or queued for another incident (two open incidents can share a pair, since `observe` dedups on `(code, subject, observed)`), and a proposal card is drawn only for an open incident.
- **E-C2 — the frame thread never joins the pump** (§3.7 rule 37; review F3). Cancellation, hand-resolution, project close and shutdown set the flag, `reject_all` the broker, signal the session's `McpServer` to shut down, **write the `Open` end on the frame thread**, and hand the thread and branch to a detached reaper that joins, drains and drops them. A late pump result arriving after the app-owned end is discarded. Rule 37's "joins the pump thread" is read as the reaper's join; its invariant — an incident is never left `Investigating` — is kept by writing the end first.
- **E-C3 — `durable == false` means neither read nor write for that run** (§2.1 rule 1; review F5). The fallback path is not written *and is not read*: the run holds in-memory defaults, Settings edits live for the run, and Settings shows one non-persistence notice. A pre-existing file at the fallback path is left byte-identical.
- **E-C4 — the `Explained` end writes telemetry only** (§3.7 rule 38 end table; lead ruling C-R1). No `QuestionKind::Recovery` counter is written; that kind stays eval-only and core gains no question API.
- **E-C5 — the two `Core::request` waits on the approve path are unbounded** (§4.4 rule 18; C-R2). Core exposes no timeout-shaped request (`actor.rs` offers `send`, `request` and `subscribe` only), and the approve path runs on the frame thread against the live actor the same frame already blocks on for every other command. Recorded, not fixed; a bounded request is an additive Core API for a later slice.
- **E-C6 — the investigator's server serves exactly `INVESTIGATOR_TOOL_NAMES` itself** (§3.3 rule 19, §3.4 rule 25's "one global filter" sentence; harness-slice H0, lead ruling §8.1b). `McpServer` gains an optional per-instance served-tool allowlist; `start_investigator_session` sets it to the six names, `list_tools` filters by it, and the chat server's list and the served quad are unchanged. Additive, one agent test.
- **E-C7 — `IncidentLog::mark_proposal_stale(&mut self, id) -> bool`** (§4.4 rule 19; lead ruling §8.2). Core gains the one setter the app needed to stale a proposal without a remove-and-re-record dance. Additive, one core test.
- **E-C8 — `kinewright_agent` re-exports `schema::operation_tool_name`** (§4.3 rule 12; lead ruling §8.1a) so the card renders an edit list from typed operations rather than by splitting a summary string.
- **E-C9 — the refused-operation stash is gated and bounded** (§3.4 rule 23; review F2). An `OpRejected` or branch-apply refusal is stashed only when the investigator is enabled, the code allowlisted and unmuted and the harness known, and the stash is an eight-entry oldest-first ring.
- **E-C10 — `SessionConfig.working_directory` is the saved project's parent directory, or the current directory for an unsaved project** (§3.3 rule 14; review F6).
- **E-C11 — line budget.** Part A landed at **12 345** insertions over its three stages (core 1 940, agent 4 798, app stage 5 607 across three crates, 2 676 of them the new `investigator.rs`), **+19 %** over §12's upper bound of 10 350; the harness slice that preceded stage C is not counted. The overrun is the app crate's: §12 estimated 1 560–2 400 for its seven rows and it landed at roughly 5 400, most of the difference being §9.1 item 40's sixteen tests written as full loopback sessions and three review rounds' regression tests.
- **E-C12 — a `start_session` that never returns strands its resources for the app's lifetime** (§3.3 rule 13, §3.7 rule 37; review-app-3 B3 secondary, C-R3). Cancellation signals the server, writes the end and hands the thread to the reaper, so the frame is never blocked; but if the harness's `start_session` never returns, the reaper's join never completes and that session's ephemeral listener port and branch `Core` thread live until the process exits. Bounded at two by the app-wide cap; recorded, not fixed.

---

## 1. Goal, and what Part A does not deliver

IN1 and IN1b made every failure in the desktop application *readable*: 67 codes, 8 subjects, one writer, one sentence each, one call for an agent. **Nothing acts on what it reads.** The router applies **3** codes deterministically (`crates/kinewright-app/src/app.rs:1193` `audit_new_incident`, over the three `AutoApply` rows at `crates/kinewright-core/src/incident.rs:1668-1685`); the other **64** open a card that explains and offers nothing to press — `card_actions` enables a button only for `RecoveryKind::Operation` (`crates/kinewright-app/src/incident_ui.rs:416-452`) and `policy_recovery`'s `Explain` arm returns `RecoveryKind::Explain` for every one of them (`incident.rs:2292-2295`).

Part A makes one sentence true that IN1 and IN1b do not:

> **A person looking at an `Explain` card whose code has no deterministic recovery gets a typed proposal from an investigator session, and on one approval the fix is applied on the live core with provenance, an undo entry and a card — and with the investigator off or no harness installed, every code that *has* a deterministic recovery is one button.**

**What Part A makes true.**

1. A default investigator harness, model and budgets live in a per-user file that survives a restart, with an off switch, and per-project per-code mutes live in the `Document` (§2).
2. A session starts headless against the **live** project's incident log, on its own branch core whose revision counter starts at live's, for exactly the **54** codes of the named allowlist and for no other (§3).
3. `IncidentState::Investigating` is a third state that **counts as open** everywhere the five `==` comparisons probe-2 T9 measured decide it by accident today (§3.6).
4. A session's only route to the live document is a typed `IncidentProposal` the person approves; the **app** applies it and the **app** records every outcome (§4).
5. Turns, wall time and tokens are enforced at named points in one non-feature-gated pump that the eval harness also calls, and the numbers reach the card and the incident's telemetry — which gives the six `AgentEvent::Cost` mirrors their **first production writer** (§5).
6. "Zero destructive actions" is three checkable properties rather than an adjective, and the broker gate covers all **seven** destructive `Operation` variants instead of three (§6).
7. Exactly **three** `Explain` codes gain a pressable recovery through `policy_recovery`, so the roadmap's *"Never below the button"* (`docs/ROADMAP-AND-WORKFLOWS.md:791-794`) becomes true of **3 of 67** rather than of zero of the 64 (§7).
8. A production `ScriptedDriver` drives whole sessions to `Applied`, `Rejected`, `Explained` and back to `Open` with no model and no network beyond loopback (§8).

**Part A does not deliver.** The inventory is normative; each item is something a reader might reasonably expect.

1. **No persistence of incidents and no cross-process timestamp** — IN1 §13 D1/D2, **Part B** (§13).
2. **No typing of the panel-local sinks, the four `Result<_, String>` seams, the `media backend error: ` prefix or a per-subject name** — IN1b §13 D-B2/D-B3/D-B4/D-B6, **Part B** (§13).
3. **No `AskFirst` row.** The class stays declared with no entry; the 11 codes probe-1 P8 names are IN3's candidates and are investigated meanwhile (§3.1 rule 6, §13 D-A1).
4. **No new `Operation` variant, no served-tool growth, no new capability *kind*.** `propose_fix` is a registry-only capability of the existing `Action` kind.
5. **No widening of `PolicyPredicate::Rec709Compatible`** and **no per-code colour body** — IN1 §13 D8, IN3. The seven `unsupported_source_*` rows keep their shared sentence, gain no operation, and are the only codes in Part A with **neither** a button nor a session (§7 rule 5, §10 limit 11).
6. **No transcode, no relink execution, no `RecoveryKind::Relink`/`Transcode`** — IN1 §13 D7, IN3.
7. **No task-scoped capability packs and no general per-session server-side tool surface** — IN1 §13 D11, IN3. §6 rule 2's denylist is one named list on one struct, not a mechanism.
8. **No per-harness cost-driver work.** Cursor constructs **zero** `AgentEvent::Cost` (`crates/kinewright-agent/src/cursor.rs` has no `AgentEvent::Cost` construction); the token budget reads "unknown" there and the session runs on turns and time (§5.3 rule 9). IN1 §13 D10, IN3.
9. **No multi-project incident attribution** — IN1b §13 D14. "One session per project" is a structural claim in Part A (§3.2 rule 10).
10. **No live `KinewrightApp` harness and no tested card placement** — IN1 §13 D12, IN1 §11.1 limit 2, unchanged (§10 limit 1).
11. **No migration of the agent crate's 277 refusal sites** — IN1b §13 D-B1, IN3.
12. **No `IncidentSeverity::Informs` row** — IN1b §13 D-B7, IN3.
13. **No fix for `default_recovery_directory`'s own per-platform defect** — §13 D-A7.

**Part A is complete when** a person who does not know what a source fingerprint is can hit a failure the table cannot fix by itself, watch a card say *Investigating* with the turns, tokens and seconds it is allowed to spend, read a sentence saying what Kinewright proposes to do and the exact edits it would make, press **Approve** once, see the edit land with an undo entry and a card that says it landed — and can turn the whole thing off in Settings and still press a button on every code that has one, with nothing silenced by having turned it off.

---

## 2. Settings, discovery and preferences

### 2.1 The per-user config file

1. **One file, owned by the app, written and read by the app alone.** Its path is `<config>/Kinewright/investigator.json`, resolved with `std::env::var_os` and **no new dependency** (§0.1 N2/a). **There is no precedent for this resolution in the tree and the contract does not claim one** (critic S2, probe-2 disagreement 11): `default_recovery_directory` (`crates/kinewright-app/src/recovery.rs:774-779`) reads **one** variable, `LOCALAPPDATA`, and falls to `std::env::temp_dir()` everywhere else, which on this machine resolves to `/tmp/Kinewright/recovery` (measured, `probe2/t10-config-path.txt`). What this contract borrows from it is the *shape* — one `var_os`, a `map_or_else` fallback, `.join("Kinewright")` — and nothing else. The resolution is new code and is normative here, in probe-2 T10's measured form:

| platform | `<config>` | durable? |
| --- | --- | :-: |
| Linux and other Unix | `$XDG_CONFIG_HOME`, else `$HOME/.config` | yes |
| Linux, both unset | `std::env::temp_dir()` | **no** |
| macOS | `$HOME/Library/Application Support` | yes |
| macOS, `HOME` unset | `std::env::temp_dir()` | **no** |
| Windows | `%APPDATA%` (**not** `%LOCALAPPDATA%`) | yes |
| Windows, `APPDATA` unset | `std::env::temp_dir()` | **no** |

   The three platform branches are `cfg!(windows)` and `cfg!(target_os = "macos")`, so the two non-native branches are reviewed by reading rather than by running. The resolver returns `(PathBuf, bool)` — the `bool` is *durable* — so "a fallback path **must** disable persistence for that run and say so once in the Settings section" is a value the Settings section reads rather than a comment nobody enforces. Measured on this machine: `("/home/riels/.config/Kinewright/investigator.json", true)`.

2. **The shape, normative.**

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct InvestigatorSettings {
    /// File-format version. `1` in Part A. A file whose version this build
    /// does not know is ignored and the defaults are used.
    pub(crate) version: u32,
    /// The off switch. Defaults **off** (rule 4).
    pub(crate) enabled: bool,
    /// The harness a session starts on, by `HarnessId`.
    pub(crate) harness: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<String>,
    pub(crate) service_tier: Option<String>,
    pub(crate) budgets: InvestigatorBudgets,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct InvestigatorBudgets {
    pub(crate) max_turns: u32,
    pub(crate) max_wall_time_seconds: u64,
    pub(crate) max_tokens: u64,
}
```

   `harness`, `model`, `effort` and `service_tier` are `Option<String>` because that is exactly what `SessionConfig` takes (`crates/kinewright-core/src/agent.rs:36-50`) and what the app already holds per harness (`crates/kinewright-app/src/app.rs:229-237`). Nothing is a typed enum, so a file written by a newer build that names a harness this build does not have degrades to "harness not detected" rather than to a parse failure. **`max_turns` is clamped to `1..=64` on read** (critic N7): `CursorAcpDriver` already floors it at `cap.max(1)` (`crates/kinewright-agent/src/cursor.rs:317-318`), the other two do not, and a `max_turns: 0` in a hand-edited file would otherwise mean "start a session and stop it before it speaks". `max_wall_time_seconds` is clamped to `5..=900` and `max_tokens` to `1 000..=2 000 000` for the same reason.

3. **Reading and writing, including the malformed file.** Four cases resolve to `InvestigatorSettings::default()`: a **missing** file, an **unreadable** file (permissions, a directory in its place), a file whose **`version` is unknown**, and a file that **does not parse**. The first is silent — a fresh install has no file and that is not a problem to report. The other three additionally open **one** incident — `IncidentCode::Label(LabelIncident::Agent)` (`agent_unclassified`) against `IncidentSubject::Agent`, through `note_label` (`crates/kinewright-app/src/error_ui.rs:138`) — so the person is told rather than silently defaulted, and **the malformed file is not overwritten until the person changes a setting**, because a file a person hand-edited badly is a file they may want to fix. **No new `IncidentCode` is minted and `POLICY` stays at 67 rows.**
4. **Every write is atomic.** The app serialises to `<path>.tmp` in the same directory, `sync_all`s it, and `std::fs::rename`s it over `<path>` — one rename on every platform the app ships on, so a crash mid-write leaves either the old file or the new one and never a truncated one. A failed write opens the same `agent_unclassified` incident **once per app session**, never once per keystroke.
5. **`enabled` defaults to `false`.** It is switched on by the person and only becomes switchable when at least one harness is detected (§2.2 rule 7). This is what makes *"no invisible autonomy"* true on first run: a fresh install never spends a token without being asked to.
6. **The file is read once at startup and written on every change.** There is no watcher and no reload; a second Kinewright process that writes it wins the last write, and Part A does not defend against that. §10 limit 6 records it.

### 2.2 Discovery and `AuthenticationStatus`

7. **Discovery re-runs the three `detect()` calls when the Settings window's Investigator section opens**, not only once at construction. Today they run exactly once, at `crates/kinewright-app/src/app.rs:435-437`, so a harness installed after launch is invisible until a restart. The three are `ClaudeCodeDriver`, `CodexDriver` (`crates/kinewright-agent/src/drivers.rs:71`, `:68`) and `CursorAcpDriver` (`crates/kinewright-agent/src/cursor.rs:39`). **`AuthenticationStatus::Unknown` is treated as available** (N1). The enum has **three** variants (`crates/kinewright-core/src/agent.rs:18-23`) and `Unknown` is the default return of two of the three detectors — `drivers.rs:849`, `:865`, `:877` and `cursor.rs:105`'s `map_or(AuthenticationStatus::Unknown, …)` — so a rule that required `Authenticated` would leave the investigator permanently off on the machines where it is most useful. The Settings row says which of the three the harness reported, through the existing `authentication_label` (`crates/kinewright-app/src/chat_ui.rs:2500`).
8. **One strike disables the harness for the app session and no longer.** When the first session on a harness fails to start (`AgentDriver::start_session` returns `Err`) or its first turn returns `AgentError::Harness` — which reaches the app as `SessionStop::Cancelled(StopReason::Harness(message))` (§3.5 rule 28) — the app sets a runtime flag, records the reason on the incident's telemetry, reports it on the card, and starts no further session on that harness until the next launch. The **file is not written**: a transient authentication failure must not silently turn the feature off for good.

### 2.3 The off switch

9. **One toggle, in the Settings window's PROVIDERS page**, using the same `toggle_switch` (`crates/kinewright-app/src/settings_ui.rs:79`) the three provider cards use, in a new `INVESTIGATOR` section beside them (`settings_ui.rs:52-72`). It writes `enabled` through §2.1 rules 3–4.
10. **Switching it off stops what is running, and silences nothing.** §3.7 rules 37–38 say exactly what happens to a session in flight; an off switch that only prevents the *next* session is not the one the roadmap row asks for, and an off switch that suppresses the incidents it was investigating is the regression critic B3 measured (§4.5 rule 22).

### 2.4 `Document.investigator`

11. **One optional top-level field on `Document`** (`crates/kinewright-core/src/model.rs:1088-1121`, ten fields today), declared between `lut_assets` and `fps` — the position probe-1 P7 measured and probe-2 T11 built:

```rust
/// Per-project investigator preferences (IN2 §2.4). Absent in every
/// pre-IN2 project, so those projects load byte-unchanged and re-save
/// without the field until a preference is set.
#[serde(default, skip_serializing_if = "Option::is_none")]
#[schemars(default)]
pub investigator: Option<InvestigatorPreferences>,

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InvestigatorPreferences {
    /// Stable `IncidentCode::code()` strings this project never starts a
    /// session for. An unrecognised string is inert (§0.1 N2/b).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(default)]
    pub muted_codes: Vec<String>,
}
```

12. **The measured cost is 52 mechanical edits and nothing else** (probe-1 P7, re-measured by probe-2 T11 by building it): **51** struct-literal sites plus `impl Default for Document` (`model.rs:1150`) — **media 20, agent 14, core 12, app 6**; by location, `crates/*/src/` **31**, `crates/*/tests/` **20**, `crates/*/examples/` **1** (`crates/kinewright-media/examples/m4_verify.rs:45`). The whole diff is **69 insertions / 1 deletion over 32 files**. The pre-M13 fixture round trip stays byte-identical at **1 215 B** with FNV-1a 64 `c9da3186e131e4fd` (`crates/kinewright-core/tests/contracts.rs:6478`, `:6487`), and **neither surface moves** — probe-2 T11 measured the served quad at `7 / 5 660 / 3 510 / 998` and the registry sextuple identical to T4's, i.e. `Document` is in no served **or** registry tool's input schema (probe-2 disagreement 15). A field **without** `skip_serializing_if` would break the 1 215 B pin, which is why both the field and the vector carry one.
13. **An older reader drops the field on save**, exactly as IN1 §13 D1 describes for provenance. Part A states it; the version gate that tells the person is **Part B**'s (§13, IN1 §13 D1).
14. **A muted code is not investigated and is still explained.** Muting changes the session predicate (§3.1 rule 4) and nothing else: the card, the badge, `get_incidents` and the `Explain` body are unchanged. The Incidents panel's card gains a **Never investigate this** action that appends the code, and the Settings section lists what a project has muted with a way to remove each.

---

## 3. The session

### 3.1 The start predicate: a named allowlist of 54 codes

1. **A session starts for a code on `INVESTIGATOR_ALLOWLIST` and for no other code** (N1/S16, N2.5/B7). The list is a `pub const INVESTIGATOR_ALLOWLIST: [IncidentCode; 54]` in `crates/kinewright-core/src/incident.rs`, declared in `POLICY`'s own order, **not** a predicate evaluated at run time. A predicate would silently enrol Part B's eleven newly reachable codes (IN1b §13 D-B3) and D-B2's three panel-sink groups the day they land, on a population Part A never measured; a `const` makes joining it an edit somebody reviews.

2. **The criterion, split into the two reasons it actually has** (critic B7). A code is off the allowlist for exactly one of two reasons, and both are printed:
   - **(1) it has a deterministic recovery** — §7 rule 4 marks it **yes**, so the card already has a button and a code with a button must never spend a model. Three rows: `unknown_source_range`, `unknown_source_bit_depth`, `unknown_source_white_point`.
   - **(2) no honest recovery exists in Part A and a session cannot invent one** — the seven `unsupported_source_*` rows, whose probe carries a *known* non-Rec.709 value that a recovery would have to overwrite (§7 rule 5, IN1 §13 D8). A session would spend a model to arrive at the sentence the card already shows.

   Plus the three `AutoApply` rows, which the router owns. **13 off, 54 on.** Everything else is on it, including every row §7 measured **unbuildable**: a row with no button and no honest operation is exactly the row a session is for.

3. **The list is `POLICY` rows 14–67, contiguous**, which is a checkable property and not a coincidence: `POLICY` declares the three `AutoApply` rows first, then the colour rows the recovery vocabulary can and cannot serve, then everything else. Rows 1–13 are the 3 `AutoApply` + the 3 buildable + the 7 `unsupported_source_*`; rows 14–67 are the allowlist. §9.1 item 1 asserts the contiguity as well as the membership, because a contiguous range is a rule a later editor can violate loudly.

4. **The full predicate at the call site**, evaluated in `route_incidents` after `audit_new_incident` (`crates/kinewright-app/src/app.rs:1144-1183`): enqueue the `Observed::Opened(id)` when **all** of — `INVESTIGATOR_ALLOWLIST.contains(&incident.code)`; `settings.enabled`; a harness is detected (§2.2); the code's `code()` string is not in the focused project's `Document.investigator.muted_codes`; the app-wide concurrent-session cap (§3.2 rule 12) is not exceeded, else queue. `Observed::Deduped` and `Observed::Suppressed` enqueue nothing, exactly as they audit nothing (IN1 §5.2 rule 17). **Work is never enqueued from the media drain**, for IN1 §5.2b rule 19's reason.

5. **The list, printed in full, in `POLICY` order, with the reason each row is a session and not a button.** Every entry was checked against `target/review/in2/probe1/p8-policy-dump.tsv`: all 54 are `PolicyClass::Explain` with `PolicyPredicate::Always`. **‡** marks the six rows probe-2 T2 measured unbuildable, which revision 1 wrongly left off. **†** marks the eleven IN3 `AskFirst` candidates (rule 6).

| # | POLICY row | code | family | why a session, not a button |
| ---: | ---: | --- | --- | --- |
| 1 | 14 | `unsupported_decoder_format` † | Media | the fix is a transcode or a relink; the *target* is a judgement |
| 2 | 15 | `media_backend_unclassified` † | Media | the rendered message is the only description Kinewright has; a model can read it |
| 3 | 16 | `unsupported_delivery_codec` ‡ | Delivery | the evidence is a code and a rendered sentence; no typed operation exists (§7 rule 6) |
| 4 | 17 | `unsupported_delivery_color` ‡ | Delivery | the wrong value lives in the export settings, not in `Document.color_context` (§7 rule 6) |
| 5 | 18 | `delivery_pixel_format_depth_mismatch` ‡ | Delivery | an export lane is not a document field |
| 6 | 19 | `delivery_encoder_pixel_format_unavailable` ‡ | Delivery | same |
| 7 | 20 | `delivery_verification_not_full_resolution` | DeliveryVerification | the measurement must be re-requested with different arguments |
| 8 | 21 | `delivery_verification_plane_out_of_container` | DeliveryVerification | same |
| 9 | 22 | `delivery_verification_frame_count_mismatch` | DeliveryVerification | same |
| 10 | 23 | `delivery_verification_frame_count_out_of_range` ‡ | DeliveryVerification | same — **the twin of row 9**, and revision 1 split them on identical evidence |
| 11 | 24 | `delivery_verification_budget_lane_mismatch` | DeliveryVerification | same |
| 12 | 25 | `color_qc_proxy_proof_refused` | ColorQc | the caller must re-request against different evidence |
| 13 | 26 | `color_qc_raster_length_mismatch` | ColorQc | same |
| 14 | 27 | `color_qc_region_empty` | ColorQc | same |
| 15 | 28 | `color_qc_node_budget_exceeded` ‡ | ColorQc | the out-of-range value is an argument to a capability call; no document field changes |
| 16 | 29 | `color_qc_matte_region_raster_mismatch` | ColorQc | same |
| 17 | 30 | `color_qc_node_removal_rejected` | ColorQc | same |
| 18 | 31 | `operation_bounds` | Operation | the refused `Operation` is in the opening message; the clamp is a re-send with one value changed |
| 19 | 32 | `operation_malformed` | Operation | same shape |
| 20 | 33 | `operation_duplicate` | Operation | same shape |
| 21 | 34 | `operation_placement` | Operation | same shape |
| 22 | 35 | `operation_missing` | Operation | same shape |
| 23 | 36 | `operation_structure` | Operation | same shape |
| 24 | 37 | `operation_relink` † | Operation | the file the person must point at is theirs to choose |
| 25 | 38 | `operation_unrepresentable` | Operation | a different frame or duration is a judgement over the cut |
| 26 | 39 | `operation_unknown_name` | Operation | the correct name is discoverable through `search_capabilities` |
| 27 | 40 | `operation_internal` | Operation | **the lowest-yield member**: its body says nothing the caller changes will fix it (§10 limit 7) |
| 28 | 41 | `operation_color_policy` | Operation | the admitted value set is in `allowed` |
| 29 | 42 | `lut_asset_policy` † | LutAssetPolicy | the store's own import is the recovery; the file is the person's |
| 30 | 43 | `edit_revision_conflict` | EditRevisionConflict | re-read at `actual` and re-send; the refused operation is in the opening message |
| 31 | 44 | `edit_plan_rejected` | Rejection | re-plan against the current revision |
| 32 | 45 | `delivery_variant_rejected` | Rejection | the variant must be rebuilt from the delivery profile |
| 33 | 46 | `agent_branch_rejected` | Rejection | the branch base no longer fits; the recovery is a new plan |
| 34 | 47 | `source_edit_rejected` † | Rejection | the in/out the person wants is theirs |
| 35 | 48 | `relink_rejected` † | Rejection | the replacement file is theirs |
| 36 | 49 | `project_save_failed` † | Rejection | the destination is theirs |
| 37 | 50 | `caption_plan_rejected` † | Rejection | the plan must be rebuilt |
| 38 | 51 | `operations_unclassified` | Label | the site's own sentence is all there is; a model can read it |
| 39 | 52 | `look_unclassified` † | Label | same |
| 40 | 53 | `look_incomplete` † | Label | the missing look's file is the person's |
| 41 | 54 | `export_unclassified` | Label | the site's own sentence is all there is |
| 42 | 55 | `source_monitor_unclassified` | Label | same |
| 43 | 56 | `relink_unclassified` | Label | same |
| 44 | 57 | `agent_branch_unclassified` | Label | same |
| 45 | 58 | `transcript_edit_unclassified` | Label | same |
| 46 | 59 | `media_unclassified` | Label | same |
| 47 | 60 | `media_incomplete` † | Label | the missing media's location is the person's |
| 48 | 61 | `agent_unclassified` | Label | same |
| 49 | 62 | `recording_unclassified` | Label | same |
| 50 | 63 | `project_unclassified` | Label | same |
| 51 | 64 | `captions_unclassified` | Label | same |
| 52 | 65 | `mixer_unclassified` | Label | same |
| 53 | 66 | `media_cache_unclassified` | Label | same |
| 54 | 67 | `timeline_unclassified` | Label | same |

6. **The eleven `AskFirst` candidates are on the allowlist and are named as IN3's future rows** (N1/Q4, N2.5/B7). They are the eleven **†** rows above: `unsupported_decoder_format`, `media_backend_unclassified`, `operation_relink`, `lut_asset_policy`, `source_edit_rejected`, `relink_rejected`, `project_save_failed`, `caption_plan_rejected`, `look_unclassified`, `look_incomplete`, `media_incomplete`. Each names an operation whose **argument** — a file, a location, a transcode target — only a person can supply, so IN3 will give them `AskFirst` with `RecoveryKind::Relink`/`Transcode` (§13 D-A1). Until it does, no button exists for them, and a session may still explain the failure or propose a fix that needs no such argument. **No code in the 67 has neither a button nor a session except the seven `unsupported_source_*` rows** (§10 limit 11), and that is the whole of the second-order consequence critic B7 asked for.

7. **A test proves the list is what it claims.** §9.1 item 1 asserts `INVESTIGATOR_ALLOWLIST.len() == 54`, that the 54 are pairwise distinct, that each member's `POLICY` row (through the module-private `policy_entry`, `incident.rs:2223`, §0.3 D1) reads `PolicyClass::Explain` with `PolicyPredicate::Always`, that no member is one of the three `AutoApply` codes, that the members are exactly `POLICY[13..67]`, and that **no member yields a `RecoveryKind::Operation`** from `policy_recovery` under any evidence §7 can build one from. The converse — every non-member `Explain` code yields one — is asserted over the rows **§7 rule 4 marks `yes`**, which probe-2 T2 measured as exactly three, with no number written into the test that the table does not supply (N2.5/B8).

### 3.2 One session per project, a queue, dedup and the cap

8. **`ProjectSession` gains `investigator: Option<InvestigatorSession>`** beside `threads` (`crates/kinewright-app/src/project.rs:287`; 37 fields today, `:252-328`). The type owns: the running incident's id and its refused `Operation`; a `TimelineBranch`; an `McpServer` built by §3.3; a `ConfirmationBroker` from that server; the session thread's `JoinHandle`; an `Arc<SharedCounters>` of atomics the pump writes and the UI reads each frame; an `Arc<AtomicBool>` cancellation flag; a `Receiver<(SessionStop, SessionCounters)>` the app drains in `route_incidents`' tick, which is where it already drains everything else; and a `VecDeque<QueuedIncident>`. **The `Box<dyn AgentSession>` is owned by the pump thread and by nothing else**, which is what makes `AgentSession::interrupt(&mut self)` (`crates/kinewright-core/src/agent.rs:113`) reachable at all (critic B2).

9. **`QueuedIncident { id: IncidentId, code: IncidentCode, subject: IncidentSubject, refused: Option<Operation> }`.** The refused operation is carried from the drain (§3.4 rule 21) and never re-derived.

10. **The queue is deduped by `(code, subject)`** (N1/Q13) — the exact pair `IncidentLog.suppressed` uses (`crates/kinewright-core/src/incident.rs:2404`, inserted at `:2538` and `:2554`) — so a queue entry and a suppression can never disagree. An enqueue whose pair is already queued or running is dropped. **One running session per project**; a project with a session in flight queues, and the queue is drained in order as each session ends.

11. **"One per project" is a structural claim in Part A and a behavioural one only after IN1b §13 D14.** `route_incidents` takes `let focused = self.focused_project;` and writes every observation into `self.projects[focused].incidents` (`app.rs:1155-1167`), with its own comment saying why, so a background project has nothing to be enqueued from. The type, the queue and the cap are per project; the exit gate quantifies over what can be provoked (critic S10 of the brief).

12. **Concurrent sessions are capped app-wide at 2** (N1), as a named `const INVESTIGATOR_MAX_CONCURRENT_SESSIONS: usize = 2`. **The cap bounds the investigator's share and not the application's total** (critic S14): every chat thread already owns an `McpServer` (`AgentThread::new`, `crates/kinewright-app/src/chat_ui.rs:245-252`, and another at `:299-306`), and that population is unbounded today. Probe-2 measured the marginal cost of one more server at **+22 OS threads, +1 ephemeral loopback listener and +0.4–1.1 MB RSS** on a 20-CPU machine — the 22 being `new_multi_thread`'s default one-worker-per-CPU (20) plus the runtime driver plus the branch `Core` actor thread (`server.rs:634-638`, `crates/kinewright-core/src/actor.rs:171-179`). **The answer is to bound the runtime, not the cap** (N2.5): §3.3 rule 13's constructor builds the investigator's server on a **2-worker** Tokio runtime, which cuts ~18 of the 22 and makes the cap of 2 a policy choice rather than a resource one. §10 figure 5 records the measurement and its baseline.

### 3.3 The headless start against the live project

13. **A new eighth `McpServer` constructor**, `start_investigator_session`, taking the six arguments `start_isolated_with_exporter_project_path_and_incidents` takes (`crates/kinewright-agent/src/server.rs:471-489` — the **only** one of the seven public constructors that takes an `IncidentLogHandle`) plus two more: a `capability_denylist: &'static [&'static str]` and a `worker_threads: usize`. It calls `start_configured` with `publish_to_playback: false`, the project's own `IncidentLogHandle`, the project's `ProjectPathHandle`, `worker_threads: 2` (rule 12), and a `ConfirmationBroker::for_incident(Duration::from_secs(5), incident)` (§4.6). **Every existing constructor keeps its signature**, defaults the denylist to `&[]` and the worker count to the Tokio default, which is the rule `server.rs:460-466` already states for the incident handle.

14. **Why a new constructor rather than two more arguments on the existing one.** The existing one is called from two app sites (`chat_ui.rs:245` and `:300`) that must not gain arguments they would always pass empty, and `start_configured` already carries an `#[allow(clippy::too_many_arguments)]` with a written reason (`server.rs:507-511`) that this rule extends by two rather than reopening.

15. **The session's core is a `TimelineBranch` over the live document, spawned at the live revision.** `TimelineBranch::new` calls `Core::spawn((*base_document).clone())` (`crates/kinewright-agent/src/branch.rs:140`) and `CoreState::new` starts every core at `TimelineRevision::default()` (`crates/kinewright-core/src/actor.rs:248-256`), so a branch's counter is independent of live's. Core therefore gains:

```rust
pub fn spawn_at(initial_document: Document, revision: TimelineRevision)
    -> Result<Self, OpError>;
```

   with `Core::spawn(document)` becoming `Self::spawn_at(document, TimelineRevision::default())` (`actor.rs:171-179`) and `CoreState::new` taking the revision. **Measured by probe-2 T3: 24 insertions / 5 deletions of production code**, `spawn_at(doc, TimelineRevision(41))` answering `Query::Snapshot` with 41 and the first accepted `Command::Do` taking it to 42, with `cargo test -p kinewright-core` at 233 passed and `-p kinewright-agent --lib` at 512 passed. No existing behaviour changes: the only other `CoreState::new` callers are two core unit tests, which pass `TimelineRevision::default()` explicitly.

16. **`spawn_at` is used by the investigator's branch and by nothing else** (critic S1 of the brief). `TimelineBranch` gains `new_at(name, base_revision, base_document)`, which spawns at `base_revision`; `TimelineBranch::new` is unchanged, so `BranchComparison.branch_revision` (`branch.rs:189`) keeps its present meaning for the chat agent and no chat-panel assertion moves.

17. **What `spawn_at` buys, stated with its limit** (critic S12). It is **not** that `get_incidents` stops conflicting — `GetIncidentsArgs.expected_revision` is an `Option` and the gate is skipped when it is `None` (`server.rs:1321-1327`), so a session that omits it never sees a conflict at all. What it buys is that **the branch and live agree at the moment the session starts**, so the operations a session proves on its branch are proved against the document approval will apply them to, and `propose_fix`'s `base_revision` (§4.2) is a live revision rather than a private counter. Its limit: the branch is spawned at the live revision at *session start*, which equals `incident.revision` only if live has not moved since the incident opened — false whenever the session was queued behind another (rule 10) or the person edited in between. §10 limit 10 records that case; §4.4's single retry is what handles it.

18. **`SessionConfig`, field by field**, built by the app from §2's settings:

| field | investigator value | why |
| --- | --- | --- |
| `working_directory` | the project file's parent, else `std::env::current_dir()` | the same rule `start_agent_turn` uses (`chat_ui.rs:421-427`) |
| `model` / `effort` / `service_tier` | `InvestigatorSettings`' three | §2.1 rule 2 |
| `max_turns` | `Some(budgets.max_turns)` | **not `None`** — the app passes `None` today (`chat_ui.rs:433`) and that is the whole of its budget surface |
| `mcp_url` | the investigator server's `endpoint()` | `server.rs:566-569` |
| `tool_names` | `Some(INVESTIGATOR_TOOL_NAMES.to_vec())` | rule 19 |

19. **`INVESTIGATOR_TOOL_NAMES` is a named `[&str; 6]` in `kinewright-agent`, a strict subset of `COMPACT_TOOL_NAMES`** (`crates/kinewright-agent/src/runtime.rs:17-25`):

| served tool | investigator gets it? | reason |
| --- | :-: | --- |
| `get_timeline_state` | **yes** | the session must see the cut it is proposing against |
| `search_capabilities` | **yes** | it must find the capability a fix needs |
| `get_capability` | **yes** | it must read that capability's schema |
| `invoke_capability` | **yes** — a **carrier**, `destructiveHint: true` at `server.rs:12663`/`:12671` | `get_incidents` and `propose_fix` are reachable only through it (`server.rs:775-786`) |
| `prepare_edit_plan` | **yes** | it validates operations before committing them to the branch |
| `commit_edit_plan` | **yes** — a **carrier**, `destructiveHint: true` at `server.rs:12687`/`:12695` | **the branch is the proposal** (§4.1): this is how a session writes one |
| `discard_edit_plan` | **no** | nothing in a session's path needs it, and a smaller allowlist is a smaller surface |

   The two carriers stay for the reason N1/B3 gives: a session that cannot dispatch a capability or commit a branch plan cannot build a proposal at all. Every **non**-carrier `destructiveHint` tool is unreachable by construction — `clear_media_cache` (`server.rs:12627`/`:12635`), `import_lut_asset` (`:12585`/`:12593`), `convert_legacy_look` (`:12597`/`:12605`), `capture_room_tone` (`:12837`/`:12845`), `queue_export` (`:12921`/`:12929`), `apply_edit_plan` (`:12987`/`:12995`) and the generated ones (`crates/kinewright-agent/src/schema.rs:361-366`) are none of them served — but *reachability through the carriers* is what §6 rule 2 closes, and `commit_edit_plan`'s own reachability of the broker is what §6 rule 5 and §4.6 rule 27 close.

### 3.4 The opening message

20. **The session is given three things and nothing else**: the incident's own wire body, the refused `Operation` the app already holds, and one instruction paragraph. There is no new capability for reading it back and no privileged access.

21. **The incident's wire body is inlined** (N1, brief critic Finding 2), so the session's first `get_incidents` is optional rather than mandatory. Measured by probe-1 P2 and confirmed by probe-2 T5: one incident alone is **819 B** for the pinned WebM colour fixture and **1 358 B** worst case (`unsupported_decoder_format` on `Chain(Bus(AudioBusId(u64::MAX)))`); the same through `get_incidents` costs **1 150 B** and **1 661 B** whole-result, of which **331 B is fixed envelope, 195 B of it structured** (critic S8 — revision 1 mixed the two columns and understated the saving by 136 B). Inlining saves the envelope on the first read and costs nothing on the wire, because the bytes are paid **once per session** rather than on every `get_incidents` for every incident forever.

22. **The refused `Operation` is composed in-app and never put on the wire** (brief critic Finding 2, S8). The app already holds it at every site that opens these incidents: `Event::OpRejected` carries `op` and the drain arm binds it (`crates/kinewright-core/src/actor.rs:117-131`, bound at `app.rs:1369`), and `BranchApplyOutcome::Rejected` carries `operations` (`branch.rs:41-44`, consumed at `chat_ui.rs:665-671` and `:770-776`). `QueuedIncident.refused` carries it (rule 9). **Core gains nothing**: `IncidentEvidence::OpError` and `::Revision` are unchanged, both size constants are unmoved by this rule, and `IN1_INCIDENT_SERIALIZED_CEILING_BYTES` survives as a property — which it could not if a `Vec<Operation>` reached the wire (§0.1 N2/c).

23. **The composed operation is elided past `INVESTIGATOR_OPENING_OPERATION_CEILING_BYTES = 4_096`** (§0.1 N2/h), with the literal marker `… (operation elided at 4096 bytes)`. An elided operation still names its variant, because the variant is what a clamp or a re-send is built from. **Probe-2 T5 measured where it bites: an `AddTrack` of 36 clips, at 4 140 B.** The scale, for review: `AddTrack` with 0 clips is **94 B**, 1 clip **202 B**, 8 clips **979 B**, 32 clips **3 688 B**, 35 clips **4 027 B**, 36 clips **4 140 B** (elides), 64 clips 7 304 B, 128 clips 14 608 B; `DeleteClip` is **44 B** and `SetColorContext` is **645 B**. A 36-clip refused `AddTrack` is not a hypothetical shape — it is one `AddTrack` carrying a populated track.

24. **The opening message's total size, measured** (probe-2 T5, `probe2/t5-opening-message.txt`). The instruction paragraph does not exist in the tree; the probe wrote one in the voice of `IN1_INCIDENTS_NEXT` and measured it at **743 B**, and the implementer's own paragraph moves every total below by its own difference in length.

| | no refused operation | with one (`AddTrack`, 8 clips, 979 B) |
| --- | ---: | ---: |
| 819 B fixture incident | **1 578 B** | **2 594 B** |
| 1 358 B worst incident | **2 117 B** | **3 133 B** |

   With the operation elided the message is **1 661 B** (fixture) / **2 200 B** (worst) — *smaller* than a 32-clip non-elided one, which is the point of the constant. **The message's own worst case is just below the ceiling, not at it**: 1 358 B of incident + 743 B of instruction + the two prefix labels + a 4 095 B operation ≈ **6 250 B**. Revision 1's arithmetic ("819–1 358 of incident plus a paragraph plus up to 4 096 of operation") under-read it by the labels and is replaced by this measurement.

25. **What Kinewright puts in front of the model before the first user word, per harness** (probe-1 P2, probe-2 T5). System prompt **1 840 B** (`crates/kinewright-agent/src/drivers.rs:28`); served tool list **5 668 B** as JSON (the pinned 5 660 is `ToolSurfaceMetrics::measure`'s per-tool sum; the 8 B is the array's punctuation).

| harness | what it sends | per-session start |
| --- | --- | --- |
| Claude Code | prompt + tool list + opening message, once | **9 086 – 10 641 B** |
| Codex | prompt + opening message, **re-sent every turn** | **3 418 – 4 973 B per turn** |
| Cursor | tool list + opening message, turn 0 only | 7 246 – 8 801 B |

   **The contract states both numbers for Codex** because they are different quantities: a six-turn Codex session pays **20.6 – 29.8 kB** of re-sent prompt before its growing recap (`drivers.rs:424-437`), which is ~5–7 k tokens of a 40 000-token budget. §5.3 rule 10's defaults are chosen knowing that, and §13 D-A5 owns the per-harness fix. A six-tool allowlist reduces the *CLI* allowlist argument and not the served list, because the server applies one global filter (`server.rs:727-732`); the contract says so rather than claiming a saving it did not measure.

### 3.5 The session pump: one owner, one thread, one loop

26. **A new non-feature-gated module `crates/kinewright-agent/src/session.rs`**, declared in `lib.rs` beside `pub mod branch;` with **no** `#[cfg(feature = "eval-harness")]`. Probe-2 T6 built it at **198 lines** and proved there is no feature-gating trap: `crossbeam-channel` is already an app dependency (`crates/kinewright-app/Cargo.toml:11`), `ConfirmationBroker` is exported outside every `cfg`, `cargo check -p kinewright-agent --no-default-features --lib` is clean, and a call from `crates/kinewright-app` compiles under `cargo check -p kinewright-app`.

27. **The execution model, in three sentences** (N2.5/B2). Each session runs on **one dedicated OS thread**, named `kinewright-investigator`, spawned and owned by the app's investigator module (`crates/kinewright-app/src/investigator.rs`). That thread owns the `Box<dyn AgentSession>` for the session's whole life, so it is the only thread that can call `interrupt(&mut self)`; the app never touches the session directly. The pump publishes progress through an `Arc<SharedCounters>` of atomics the UI reads each frame and observes an `Arc<AtomicBool>` cancellation flag between events, calling `interrupt` itself when it sees the flag set — which is how §3.7's cancellation works without a second owner of a `&mut`.

28. **The exported surface, normative** (N2.5/B2, probe-2 disagreement 9, §0.4 k):

```rust
pub struct SessionLimits { pub max_turns: u32, pub max_wall_time: Duration,
                           pub max_tokens: Option<u64> }

pub enum BudgetKind { Turns, WallTime, Tokens }

pub enum StopReason {
    /// The person switched the investigator off.
    OffSwitch,
    /// The person resolved the incident by hand, or it was resolved elsewhere.
    ResolvedElsewhere,
    /// The harness refused to start or refused a turn (§2.2 rule 8).
    Harness(String),
    /// The observer asked to stop, with its own reason.
    Observer(String),
}

/// Exactly five variants (N2.5/B2).
pub enum SessionStop {
    Completed,
    Budget(BudgetKind),
    Cancelled(StopReason),
    Disconnected,
    PolicyViolation,
}

pub enum ConfirmationPolicy { ApproveAll, RejectAndStop }

#[derive(Default)]
pub struct SessionCounters { pub turns: u32, pub input_tokens: u64,
                             pub output_tokens: u64, pub elapsed: Duration,
                             pub tool_calls: u32 }

/// The atomics the UI reads each frame; the pump is the only writer.
pub struct SharedCounters { /* five atomics, one per SessionCounters field */ }
impl SharedCounters { pub fn snapshot(&self) -> SessionCounters; }

pub trait SessionObserver {
    fn on_event(&mut self, event: &AgentEvent) -> SessionFlow;
    fn on_tick(&mut self, counters: &SessionCounters) -> SessionFlow;
    fn on_confirmation_error(&mut self, _message: String) {}
}

pub enum SessionFlow { Continue, Stop(String) }

#[allow(clippy::too_many_arguments)] // §0.4 o: nine is what the two callers
                                     // disagree about; a struct would hide it.
pub fn pump_session(
    session: &mut dyn AgentSession,
    events: &Receiver<AgentEvent>,
    prompts: &mut dyn Iterator<Item = String>,
    confirmations: Option<&ConfirmationBroker>,
    policy: ConfirmationPolicy,
    limits: &SessionLimits,
    counters: &Arc<SharedCounters>,
    cancel: &Arc<AtomicBool>,
    observer: &mut dyn SessionObserver,
) -> (SessionStop, SessionCounters);
```

29. **Each of the four traps probe-2 measured is closed by a named argument.** (a) **No prompt source**: `prompts` supplies them — the investigator yields the opening message once and then `None`, the eval yields `collect_session`'s `prompts: &[&str]` (`crates/kinewright-agent/src/eval.rs:3843-3850`). (b) **No `Disconnected` stop**: `collect_session`'s existing *"agent event stream disconnected"* error had nowhere to come from, and now does. (c) **`Cancelled` carried no reason**: `StopReason` carries it, and `Harness(String)` is what §2.2 rule 8 reads. (d) **The two callers' confirmation policies are opposite** — the eval approves every request (`eval.rs:3885-3893`), the investigator must reject and end the session (§4.6) — so `ConfirmationPolicy` is a parameter and not a convention. A fifth, minor: `on_confirmation_error` carries the eval's *"confirmation N disappeared before approval"*, and the eval's `operation_count: F` returning `Result<_, EvalError>` is stashed in the eval's own observer and re-raised by `collect_session`, because `SessionFlow::Stop(String)` cannot carry it.

30. **One owner, enforced in writing and in a test.** `collect_session`'s event loop (`eval.rs:3843-4010`) **must** be rewritten to call `pump_session` with an eval-specific `SessionObserver` that keeps the four budgets the pump does not own — `max_operations` (`eval.rs:3905`), `max_tool_calls` (`eval.rs:3922`), `max_cost_usd` (`eval.rs:3958`) and the auto-approval — returning `SessionFlow::Stop` for each. **There must be no second agent-event loop in `kinewright-agent` or in `kinewright-app`**, asserted by §9.1 items 35–36 as a per-crate source-shape test (N2.5/B6). Probe-2 T6 performed the rewrite: **411 insertions / 134 deletions over 4 files** (`session.rs` 198, `eval.rs` +189/−134, `lib.rs` +1, plus a 24-line app-side reachability proof), with `cargo test -p kinewright-agent` green at 514 lib + 38 + 6 + 59 + 1 + 1 and both check gates clean.

31. **The eval harness's behaviour does not change.** `EvalBudgets`' seven fields (`eval.rs:80-91`) keep their meanings, five of them still enforced mid-flight and `max_tokens`/`max_undos` still scored after the fact (`eval.rs:5580-5583`); `SessionMetrics` is unchanged. §9 regression R-B asserts the eval suite stays green, as it did under probe-2's own rewrite.

### 3.6 `IncidentState::Investigating`, and the sites it forces

32. **A third top-level variant** on `IncidentState` (`crates/kinewright-core/src/incident.rs:1089-1095`), declared between `Open` and `Resolved`, serialising as the single key `"state":"investigating"` with no `"outcome"` — the shape `Open` already has under `#[serde(rename_all = "snake_case", tag = "state", content = "outcome")]`. It costs **+9 B** on the wire over `"open"` (probe-2 T1).

33. **`IncidentState::is_open()` is added, and the sites are decided one by one.** Probe-2 T9 built the variant and measured the blast radius: **exactly one** compile error workspace-wide (`incident_ui.rs:402`, `state_label`'s exhaustive match) and **five** — not six — `==`/`matches!` comparisons that silently change meaning. The sixth site revision 1 listed, `incident_ui.rs:421`'s revert guard, is `== Resolved(Applied)` and correctly does **not** change; the table's own last row already said so. The whole change is **21 insertions / 8 deletions over 3 files** (`incident.rs` 17/3, `incident_ui.rs` 3/1, `media_bin.rs` 9/4).

```rust
impl IncidentState {
    /// Whether the incident is still outstanding. An incident under
    /// investigation is unresolved: the work is happening, not finished.
    #[must_use]
    pub const fn is_open(self) -> bool {
        matches!(self, Self::Open | Self::Investigating)
    }
}
```

| # | site at `7e85972` | shape today | decision |
| ---: | --- | --- | --- |
| 1 | `incident_ui.rs:400-408` (`state_label`) | the one exhaustive `match`, **four** arms | gains **two** arms — `"Investigating"` and `"Rejected"` (§4.5 rule 21) — for **six**, with its doc comment at `:399` rewritten (critic S10) |
| 2 | `incident.rs:2451` (`observe`'s dedup arm) | `== IncidentState::Open` | **`is_open()`** — a repeat observation while a session runs must dedup, not open a second incident about the same problem |
| 3 | `incident.rs:2490` (`IncidentLog::open`) | `== IncidentState::Open` | **`is_open()`** — an investigated incident must stay in `get_incidents`' default listing (`server.rs:1328-1333`) |
| 4 | `incident.rs:2593` (`open_count`) | `self.open().count()` | inherits site 3; the badge does not fall while the work happens |
| 5 | `incident.rs:2580` (`refresh_revision`) | `!= IncidentState::Open` → `false` | **`is_open()`** — IN1 §5.2 rule 10's conflict refresh must keep working for an investigated incident |
| 6 | `incident_ui.rs:539` (`incident_panel_rows`) | `== IncidentState::Open` | **`is_open()`** — the `Investigating` card must render in the Incidents panel, which is the only surface non-asset subjects have |
| 7 | `media_bin.rs:459-464` | `matches!(state, Open \| Resolved(Applied))` | **`is_open() \|\| matches!(state, Resolved(Applied))`** — the per-asset card list keeps its deliberate shape (IN1b §5.5 rule 31) and gains the third state |
| 8 | `incident_ui.rs:421` (`card_actions`'s revert guard) | `== Resolved(Applied)` | **unchanged, and not one of the five** — it is correct as written and `is_open()` would break it |

34. **Nothing constructs `Resolved` from `Investigating` except through `IncidentLog::resolve`**, which is unchanged: it already writes `Resolved(outcome)` over whatever state the entry had (`incident.rs:2531-2540`) and already inserts the `(code, subject)` pair into `suppressed` for every outcome. §4.5 rule 22 is what decides which ends go through it.

35. **`IncidentLog` gains two narrowly typed writers**, in the shape `note_auto_applied` and `refresh_revision` already have (`incident.rs:2549`, `:2576`):
   - **`begin_investigation(id) -> bool`** — writes `state = Investigating` and **only** on an `Open` entry, returning `false` and changing nothing otherwise. It does **not** touch `suppressed`, `count`, `opened_at`, `revision` or telemetry. A session that fails to begin because the incident is no longer `Open` never starts (§3.7 rules 41–42).
   - **`end_investigation(id, stopped_reason) -> bool`** — writes `state = Open` and a `stopped_reason` onto telemetry (§0.4 p), and **only** on an `Investigating` entry. It does **not** touch `suppressed`. This is the writer N2.5/B3 requires and the one revision 1 did not have.

36. **The wire value is a third string in a field two consumers already read.** `get_incidents` renders it (`server.rs:1334-1345`) and `state_label` renders it on the card. `IncidentState` derives `Serialize` only (`incident.rs:1087-1088`); Part B's persistence needs `Deserialize` and **Part A chooses the shape it will have to deserialize** (critic S7 of the brief).

### 3.7 The lifecycle: the ends that are not an approved proposal

37. **Cancellation is one path with one shape.** `InvestigatorSession::cancel(reason)` sets the `Arc<AtomicBool>`, calls `ConfirmationBroker::reject_all(reason)` (`server.rs:204-218`), joins the pump thread — which has called `interrupt` itself on seeing the flag (rule 27) — and then drops the branch and its `McpServer`. **An incident is never left `Investigating`**: the app writes the end before it drops anything.

38. **Two different ends, and only one of them resolves** (N2.5/B3, critic B3). This is the rule revision 1 did not have, and without it turning the investigator **off** would silence the problems it was investigating for the rest of the app session.

| end | what the app writes | suppresses? |
| --- | --- | :-: |
| the person pressed **Reject** on the proposal | `IncidentLog::resolve(id, Rejected)` | **yes** — they said no |
| `SessionStop::Budget(_)` | `end_investigation(id, "budget: turns \| wall time \| tokens")` → back to `Open` | no |
| the off switch, mid-session | `end_investigation(id, "the investigator was switched off")` → `Open` | no |
| `SessionStop::PolicyViolation` | `end_investigation(id, "the session asked for a confirmation")` → `Open` | no |
| `SessionStop::Disconnected` | `end_investigation(id, "the agent event stream disconnected")` → `Open` | no |
| `SessionStop::Cancelled(StopReason::Harness(m))` | `end_investigation(id, m)` → `Open`, plus §2.2 rule 8's one-strike disable | no |
| the person resolved it by hand, or it was resolved elsewhere | **nothing** — their outcome is already written and the session's is never written over it | (theirs) |

   **IN1 §2.3 rule 19 is unamended and stands as written** (`docs/IN1-INCIDENTS-AND-THE-COLOUR-CASE.md:293-297`): *"`IncidentLog::resolve` … inserts for **every** outcome"*. Six of the seven ends above are not outcomes, so they never reach `resolve`; the one that is an outcome suppresses, exactly as `Applied`, `Reverted` and `Explained` do. Nothing in `IncidentLog`'s suppression invariant moves.

39. **A returned-to-`Open` incident shows a stopped card.** `incident_card`'s `details` gains a `"stopped"` row reading *"investigation stopped: &lt;reason&gt;"* and `card_actions` offers **Re-investigate** (§4.3 rule 12's action, reused). The badge does not fall, the card still explains, and the next observation of the same `(code, subject)` still dedups into the same incident because `observe`'s dedup arm reads `is_open()` (§3.6 site 2).

40. **The off switch clears the queue and re-queues nothing.** A person who turned it off did not ask for a backlog. Every running session is cancelled through rule 37; every queued entry is dropped; the incidents behind them are untouched and still `Open`.

41. **The person resolves the incident by hand mid-session.** The card's buttons stay live while a session runs — `send_incident_recovery` (`crates/kinewright-app/src/app.rs:1538-1568`) has no guard — and `Investigating` is not one either: `IncidentLog::resolve` writes the person's own outcome, the app sees on its next tick that the state is no longer `Investigating`, cancels the session, and writes **nothing**. The last write wins and it is the person's.

42. **The incident is resolved elsewhere** — by the router, by `resolve_incident` from the chat agent, or by a suppression — with the same result as rule 41.

---

## 4. The proposal

### 4.1 `propose_fix`, and why the branch *is* the proposal

1. **The seam revision 1 got wrong, and the ruling that fixes it** (critic B1, N2.5/B1). Revision 1 had `propose_fix` take a `Vec<Operation>` and gate it on `expected_revision == self.snapshot()?.0`. The session's own happy path calls `commit_edit_plan` first, which dispatches `apply_edit_plan` (`server.rs:820-859`, `:852`) and advances the branch core's revision — so the gate refused every proposal the workflow produced, and the "re-apply on a clone" refusal beside it refused every non-idempotent one a second time. **The session edits its branch through the ordinary edit path, and the branch's applied-operation list is the proposal.** There is nothing to pass and nothing to gate.

2. **One new capability, registry-only, of the existing `Action` kind.** It joins `INSPECTOR_TOOL_NAMES` (`crates/kinewright-agent/src/schema.rs`) directly after `resolve_incident`, exactly as IN1 §6.1 placed `get_incidents` after `get_timeline_state`, and is reached only through `invoke_capability` (`server.rs:775-786`) — a direct `call_tool("propose_fix", …)` returns the existing *"internal capability, not an MCP tool"* refusal (`server.rs:743-748`). The **served** quad does not move; the registry sextuple does (§6.4).

3. **The input schema, normative — two fields.**

```rust
#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct ProposeFixArgs {
    /// The incident this proposal is for, from get_incidents.
    incident_id: u64,
    /// One sentence a person can act on. Truncated to 240 bytes of JSON.
    explanation: String,
}
```

4. **What the handler does, in order.** (a) Reads the branch's own applied-operation list — `Command::Query(Query::AppliedOperations)` on the server's core, which for an investigator server **is** the branch (`crates/kinewright-core/src/actor.rs:89`, `:361-366`); it returns the operations still represented by the undo stack, so an edit the session undid is not proposed, and because the branch was spawned by `spawn_at` with an empty `op_log` (`actor.rs:255`) "since `spawn_at`" is the whole list. This is the same list `TimelineBranch::compare` already reads (`branch.rs:182-186`) and the same one `apply_to_live` replays. (b) Runs destructive check (ii) over it (§6 rule 3). (c) Renders `summary` and truncates both strings (rule 5). (d) Writes the `IncidentProposal` onto the incident through the shared `IncidentLogHandle` (`server.rs:1308-1312`), increments `telemetry.tool_calls`, and returns `success_structured` with the incident id, the operation count and `base_revision`. **It does not resolve the incident, it does not change its state, and it does not block on a person.** No MCP handler blocks on a person anywhere in this contract.

5. **Both strings are capped on their *serialised* length, not their raw length** (critic S3). JSON escaping is not length-preserving: 240 bytes of control characters serialise to about 1 440 B, and `IN1_INCIDENT_SERIALIZED_CEILING_BYTES` is an `assert` in production code (`server.rs:16437`, asserted at `:26292`) over a string the **model** supplies. So `INVESTIGATOR_EXPLANATION_CEILING_BYTES = 240` bounds `serde_json::to_string(&explanation).len() - 2`, applied by truncating at a `char` boundary, re-serialising and re-truncating until it fits — at most three passes, because each pass strictly shrinks. `summary` is capped the same way. This is what makes the ceiling a property again rather than a bet on which 240 bytes arrive.

6. **The handler's refusals, by code, in order.** Six, not eight: revision 1's revision-conflict and "does not apply cleanly to a clone" refusals are gone with the parameter that needed them. Each is an `incident_error` in the shape `get_incidents`/`resolve_incident` already use (`server.rs:1364-1372`), carrying `code`/`field`/`observed`/`allowed`/`recovery_action`.

| # | code | when |
| ---: | --- | --- |
| 1 | `incident_not_found` | no such id in the shared log |
| 2 | `incident_not_investigating` | the incident's state is not `Investigating` — the person or the router got there first (§3.7) |
| 3 | `proposal_empty` | the branch has applied no operation, or every one was undone |
| 4 | `proposal_too_large` | more than `INVESTIGATOR_MAX_PROPOSAL_OPERATIONS` (**8**) operations on the branch |
| 5 | `proposal_destructive` | any operation is in the seven-variant destructive set — §6 rule 3, checked **before** anything is recorded |
| 6 | `proposal_already_recorded` | the incident already carries a non-stale proposal |

7. **`IncidentLog` gains one more narrowly typed writer, `record_proposal(id, proposal) -> bool`**, in the shape of §3.6 rule 35: it writes `proposal` and only on an `Investigating` entry.

### 4.2 `IncidentProposal`

8. **The type, and what of it reaches the wire.**

```rust
/// A typed fix an investigator session proposes for one incident (IN2 §4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IncidentProposal {
    /// The branch's applied operations, in order. **Never serialised**:
    /// `Operation` has 57 variants (`operation.rs:23-387`, counted directly)
    /// and several carry unbounded payloads, so a `Vec<Operation>` on the
    /// wire would end `IN1_INCIDENT_SERIALIZED_CEILING_BYTES` as a property
    /// rather than move it (IN2 §0.1 N2/c). The app reads them through the
    /// shared log, in process, which is the only consumer there is.
    #[serde(skip)]
    pub operations: Vec<Operation>,
    /// How many there are (§0.4 l).
    pub operation_count: usize,
    /// One line per operation, at most 240 bytes of JSON (§4.1 rule 5).
    pub summary: String,
    /// One sentence, at most 240 bytes of JSON (§4.1 rule 5).
    pub explanation: String,
    /// The **live** revision the branch was seeded at by `spawn_at`.
    pub base_revision: TimelineRevision,
    /// Set by the app when a merge has conflicted twice (§4.4 rule 18), and
    /// by Part B on every proposal it loads (rule 10).
    pub stale: bool,
}
```

   **`Eq` is not optional** (critic S1, probe-2 disagreement 1): `Incident` derives it (`crates/kinewright-core/src/incident.rs:1140`) and `Option<IncidentProposal>` on `Incident` breaks that derive without it. `Operation` is `Eq` (`crates/kinewright-core/src/operation.rs:22`), so it costs one word. Probe-2 built the type and the derive and both compile.

   and on `Incident` (`incident.rs:1141-1180`), beside `telemetry`:

```rust
/// The session's proposal, when one has been recorded.
#[serde(skip_serializing_if = "Option::is_none")]
pub proposal: Option<IncidentProposal>,
```

9. **`base_revision`, not `branch_revision`** (N2.5/B1). The field a reader needs is *what live looked like when this was proved*, which is `spawn_at`'s seed (§3.3 rule 17) — a live revision, comparable with `ProjectSession.revision` at approval time. The branch's own counter is private to a core nobody outside the session can query and is not put on the wire.

10. **The decision N1 asked for, with its reason, and what Part B inherits.** The proposal **does not** live only on the app side, and the operations **do not** go on the wire. Putting the whole proposal app-side would put it out of reach of `get_incidents`, which is the one place an agent — or a second Kinewright surface — reads an incident; putting the operations on the wire would put an unbounded `Vec<Operation>` inside a record whose ceiling is an `assert`. The record carries the facts a reader can act on (how many edits, what they are, what for, at which live revision) and the app carries the edits, which is exactly IN1's split for `opened_at` (`incident.rs:1170-1171`). **The consequence for Part B is a rule, not a surprise** (critic S1): `#[serde(skip)]` deserialises `operations` to `vec![]` while `operation_count` stays 3, so **a deserialised proposal is always `stale`** — Part B sets `stale = true` on every proposal it loads, the card offers **Re-investigate** and not **Approve**, and no reload can produce an `Applied` outcome that applied nothing. §13's D2 row carries it.

11. **Two constants, both measured** (probe-2 T1, over `67 codes × 8 subject shapes × {no proposal, a proposal at the cap} × {open, investigating, resolved-with-all-telemetry, resolved-without-the-two-new-fields}` = **4 288 measurements**).

   - **`IN1_INCIDENT_SERIALIZED_BYTES = 819` does not move**, and stays an `assert_eq!` (`server.rs:16406`). The pinned fixture incident is `Open`, carries no proposal and no filled telemetry, and every field this contract adds — `proposal`, `turns`, `resolver` — carries `skip_serializing_if`, so its serialisation is byte-identical. Probe-2 ran both ceiling suites with the assertion untouched and both passed.
   - **`IN1_INCIDENT_SERIALIZED_CEILING_BYTES` moves from 2 048 to `4_096`.** The measured worst is **2 183 B**, on `unsupported_decoder_format × Chain(Bus(AudioBusId(u64::MAX)))` — the same pair IN1b measured, so the ceiling's *identity* did not move, only its value. **The cause is the proposal alone**: the same shape without either new telemetry field is **2 057 B**, already over 2 048. Revision 1 attributed the move to "three keys"; two of them are optional and the third is sufficient on its own. The measured components are `"investigating"` over `"open"` **+9 B**, `turns` + `resolver` on a fully resolved incident **+126 B**, a proposal at the cap **+342 B**.
   - **`server.rs:26324`'s `worst > CEILING / 2` divisor holds, by 135 B** (2 183 > 2 048). The contract records that it was checked and by how little rather than saying "reconsider": one more optional string on `Incident` and the *divisor* assertion fails while the ceiling assertion still passes, which is the failure mode worth naming in advance. `summary`, which this revision adds beside `explanation`, is inside that margin only because both are capped at 240 B of JSON; the implementer re-runs the loop and re-pins in the same commit.
   - **The pin is the loop, not a remembered number** (probe-2 disagreement 4). Revision 1's 1 737 B "resolved worst" is not reproducible — probe-2 measures 1 715 B for the same shape, and the difference is which saturated `Duration` and `cost_usd_millionths` the builder picks. §9.1 item 32 asserts the loop's own worst against the constant and asserts nothing about any intermediate figure. For the record, `"explained"` (9 B) is the widest of the four outcome strings, 1 B wider than `"rejected"`, and the loop uses it.

### 4.3 `proposal_card`

12. **A new pure function beside `incident_card`**, in `crates/kinewright-app/src/incident_ui.rs`:

```rust
#[must_use]
pub(crate) fn proposal_card(incident: &Incident, proposal: &IncidentProposal)
    -> ProposalCardView;

pub(crate) struct ProposalCardView {
    pub(crate) subject_label: String,
    pub(crate) headline: &'static str,
    pub(crate) explanation: String,
    pub(crate) operation_summary: Vec<String>,
    pub(crate) stale: bool,
    pub(crate) actions: Vec<ProposalAction>,
}

pub(crate) enum ProposalAction { Approve, Reject, Reinvestigate }
```

13. **`incident_card`'s signature does not change and neither do its 18 call sites.** The figure is **18**, not revision 1's 21 (critic S5, re-measured for this revision): **15** in `incident_ui.rs`, **2** in `app.rs` (`:5930`, `:6924`) and **1** in `media_bin.rs` (`:474`), against the declaration at `incident_ui.rs:83`. The 21 was a substring count that also matched the three `show_incident_card(` calls, and revision 1's own breakdown summed to 20. `proposal_card` is drawn **below** `incident_card`'s view wherever a card with a proposal is drawn, which is the Incidents panel (`incident_ui.rs:600`) and the Media panel's per-asset list (`media_bin.rs:459-476`). `headline` stays `&'static str` (IN1 §5.3 rule 25), so no measured number can reach it; the numbers are in `explanation` and `operation_summary`, both `String`.

14. **`operation_summary` is one line per operation, from `crate::schema::operation_tool_name`** (`crates/kinewright-agent/src/runtime.rs:359` uses it for exactly this) plus the operation's own subject through `Operation::incident_subject()` (IN1b §3.3 rule 20, unchanged). It is capped at the eight operations `propose_fix` accepts, so the card cannot grow without bound, and it is the same rendering `IncidentProposal.summary` carries on the wire.

15. **A stale proposal renders with `stale: true`, `Approve` absent and `Reinvestigate` present.** A button that cannot succeed is the one thing IN1 §5.3 rule 29 forbids.

### 4.4 Apply on approve

16. **`apply_to_live` becomes `pub`** (`crates/kinewright-agent/src/branch.rs:247-271`) and is re-exported from `lib.rs` beside `TimelineBranch`. It is already the single application path for both merge and cherry-pick, and it is already revision-gated through `Command::DoBatchIfRevision`. **It returns `Result<BranchApplyOutcome, BranchError>`, not `BranchApplyOutcome` by value** (critic S4): `live.request(…)` can fail, and `BranchError::{CoreDisconnected, UnexpectedResponse}` (`branch.rs:46-57`, produced at `:268`) is a real arm the approve path must have. The call is still **synchronous**, so the caller that holds the `IncidentId` holds the answer.

17. **`Command::DoBatchIfRevision` does not gain a `CommandToken`, and `Event::BatchRejected` does not gain an echo.** IN1b §4 rule 4 already rules this in writing and its reason is the transport, not the absence: the token exists for the fire-and-forget `Core::send` path, which only `DoIfRevision` uses (`crates/kinewright-core/src/actor.rs:65-72`). **No erratum is opened against IN1b §4 rule 4** and none is needed (brief critic B4; probe-1 P5's item 13 measures the change that is therefore not made).

18. **The approval path, normative, with its lock discipline** (critic S15). On **Approve**, the app, on the UI thread, in this order: takes a read guard on the shared `Arc<RwLock<IncidentLog>>` with the house's `unwrap_or_else(PoisonError::into_inner)` (`app.rs:1159-1161`, `:1176-1178`); **clones** `proposal.operations`; **drops the guard**; reads the live revision `r` from `ProjectSession.revision`; calls `apply_to_live(&project.core, r, operations)`; and branches on the outcome. **No core request is ever made while the incident log is locked**, and the same discipline governs the pump's `telemetry_mut` writes (§5.4 rule 11), because the `propose_fix` handler writes that lock from an MCP worker thread while the UI thread reads it every frame for `incident_panel_rows`.

| outcome | what the app does |
| --- | --- |
| `Ok(Applied { revision, document, operation_count })` | records `IncidentOutcome::Applied` through `IncidentLog::resolve`, writes the audit line through `note_incident` (`crates/kinewright-app/src/error_ui.rs:89`), and lets the ordinary core drain publish the new document. The undo entry is core's own, from `DoBatchIfRevision`'s single history entry |
| `Ok(Conflict { .. })` **first time** | **retries once**, at the revision the app has re-read after draining the core's events — N1/B8's single retry. Operations are id-addressed and `Document::apply` refuses what no longer fits, so the retry is safe or it is rejected |
| `Ok(Conflict { .. })` **second time** | leaves the incident `Open`, sets `proposal.stale = true`, and the card offers **Re-investigate** |
| `Ok(Rejected { operations, error })` | leaves the incident `Open`, sets `stale`, and opens the ordinary `BatchError` incident through `IncidentObservation::from_batch_error` — the same arm `chat_ui.rs:665-671` already uses |
| `Err(BranchError)` | leaves the incident `Open`, sets `stale`, puts the error's message on telemetry and offers **Re-investigate**. The live core has stopped or answered something impossible; nothing is recorded as an outcome (critic S4) |

   **There is no `NoChanges` row.** `apply_to_live`'s own `match` maps `DocumentChanged`/`RevisionConflict`/`BatchRejected` and everything else to `Err`, so it cannot return one; revision 1's reasoning for that row was right and its premise was wrong. `propose_fix` also refuses an empty proposal (§4.1 rule 6 code 3).

19. **Reject** records `IncidentOutcome::Rejected` through `IncidentLog::resolve` and drops the proposal — the one end in this contract that suppresses (§3.7 rule 38). **Re-investigate** clears the proposal, re-enqueues the `(code, subject)` pair (§3.2 rule 10) and starts a fresh session with the same budgets. Re-investigation is not free and is not automatic: it happens only because a person pressed it.

### 4.5 Outcomes, recorded by the app and only by the app

20. **`IncidentOutcome` gains exactly one variant** (`incident.rs:1069-1076`, whose doc comment already names it as IN2's). **Probe-2 T9 measured its blast radius at exactly two compile errors**, which revision 1 did not price: `crates/kinewright-app/src/app.rs:183` (the outcome match in the auto-apply audit; the new arm is `Explained | Rejected => false`) and `incident_ui.rs:402` (`state_label` gains `"Rejected"`).

```rust
pub enum IncidentOutcome {
    Applied,
    Reverted,
    Explained,
    /// The person refused the session's proposal. An approved proposal that
    /// lands is `Applied`, which the document shows (IN2 §0.1 N1/Q7). A
    /// session that ended without a proposal the person refused does **not**
    /// produce this outcome — it returns the incident to `Open` (§3.7).
    Rejected,
}
```

21. **`ResolveIncidentOutcome` gains nothing** (`server.rs:11204-11210`, three variants, private), so the **809 B `resolve_incident` input schema does not move** (`server.rs:26139`) and no wire value is added to that tool. `state_label` gains a `"Rejected"` arm (§3.6 rule 33 site 1) and `incident_headline`'s table grows by nothing, because headlines are keyed on `(code, class)` and not on the outcome (`incident_ui.rs:143`).

22. **The complete end table. Every row is written by the app; no row is written by a model.** This is §3.7 rule 38 stated once more from the outcome's side, because it is the rule critic B3 found missing and it is easier to get wrong than to state.

| what happened | what is written | state afterwards |
| --- | --- | --- |
| a proposal was approved and `apply_to_live` returned `Applied` | `Applied` | `Resolved(Applied)`, suppressed |
| the person pressed **Reject** | `Rejected` | `Resolved(Rejected)`, suppressed |
| the session ended `Completed` with no proposal | `Explained`, plus one `QuestionKind::Recovery` | `Resolved(Explained)`, suppressed |
| a budget stopped the session | `end_investigation` + the stop and the counters on telemetry | **`Open`**, not suppressed |
| a broker request was raised inside the session | `end_investigation` + the violation on telemetry, attributed to the incident | **`Open`**, not suppressed |
| a proposal contained a destructive operation | `end_investigation` + the offending variant on telemetry | **`Open`**, not suppressed |
| the off switch, or a disconnect | `end_investigation` + the reason | **`Open`**, not suppressed |
| the harness failed to start or refused a turn | `end_investigation` + the message, plus the one-strike disable | **`Open`**, not suppressed |
| a hand resolution, or an outcome recorded elsewhere | **nothing** | the person's own |

23. **`resolve_incident` is not reachable from an investigator session** (N1/B7). §6 rule 4 is the mechanism and §9 clause 12 is the assertion. This closes, for the investigator, the hole the brief critic measured: `verify_claimed_outcome` verifies an `Applied` colour claim against **the document it was handed** (`server.rs:16512-16532`, `:16543-16590`), which for a branch-backed server is the branch's — so a session could otherwise write `Resolved(Applied)` into the person's live log with the live document untouched. Part A removes the capability from the session rather than re-plumbing the verifier; the chat agent's identical exposure is unchanged and is recorded as §10 limit 3.

### 4.6 `ConfirmationRequest.incident`, and the auto-reject rule

24. **One field on the request** (`server.rs:139-144`, three fields since IN1):

```rust
pub struct ConfirmationRequest {
    pub id: u64,
    pub tool_name: String,
    pub description: String,
    /// The incident an investigator session is working on, when this broker
    /// belongs to one. `None` for every chat-panel and eval broker.
    pub incident: Option<IncidentId>,
}
```

25. **The broker stamps it; the six `confirm(` sites do not change** (§0.1 N2/e). `ConfirmationBroker` gains one `incident: Option<IncidentId>` set at construction, and `confirm` (`server.rs:230-262`) copies it into the request it sends. `ConfirmationBroker::with_timeout` keeps its signature and a new `for_incident(timeout, incident)` sits beside it, so the **97** test constructions probe-1 P4 counted do not move. The six sites stay exactly as they are at `server.rs:1231`, `:1655`, `:2145`, `:2363`, `:2718`, `:3957`. This is route (a) of IN1 §13 D6 executed at one site instead of eight.

26. **`pending_requests()`'s draining `try_iter()` is unchanged** (`server.rs:180-186`) and the drain defect is **not** fixed here. The rule that makes that safe is written instead: **each broker has exactly one consumer.** The chat panel's brokers are drained by `poll_agent` (`chat_ui.rs:863-876`) and rendered at `chat_ui.rs:1568`; the investigator's broker is drained by its own session pump on its own thread and rendered nowhere. The two populations never meet, because `poll_agent` iterates `project.threads` and an `InvestigatorSession` is not a thread.

27. **A confirmation request raised inside an investigator session is a policy violation, and the contract states the true reason it can be raised at all** (N1/Q6, critic B5, N2.5/B5). Revision 1 claimed *"the allowlist and the denylist together leave the session no brokered tool it can reach"*, and that is false: `commit_edit_plan` dispatches `apply_edit_plan`, which calls `plan_confirmation_description` and then `self.confirmations.confirm("apply_edit_plan", …)` at `server.rs:1654-1656` — one of the six sites — so a session that builds a plan containing a destructive operation reaches the broker on a normal path, and §6 rule 5's widening makes four more operations reach it. **The true statement:** the session's only brokered path is `commit_edit_plan` → `apply_edit_plan`, and it fires only for an operation §6 rule 3 already refuses, so a request means the session tried to commit a destructive plan on its branch — the same violation, caught one step earlier. The pump, with `ConfirmationPolicy::RejectAndStop`, rejects it within one 100 ms tick with the literal reason `"an investigator session may not ask for a confirmation"`, returns `SessionStop::PolicyViolation`, and the app returns the incident to `Open` with the violation on telemetry (§3.7 rule 38). **This is check (iii) at run time** (N2.5/B5), and §6 rule 3's `proposal_destructive` is the second net behind it.

28. **The investigator's broker is constructed with a 5 s timeout** (§0.1 N2/f), not the 60 s `DEFAULT_CONFIRMATION_TIMEOUT` (`server.rs:116`), so a pump that has stopped cannot hold an MCP handler thread (`server.rs:251-264`) for a minute. Nothing a person sees is behind this broker, so no person-facing deadline is created and the session's **wall-time budget is the only clock** on a session.

---

## 5. Budgets, enforcement and telemetry

### 5.1 Turns

1. **`SessionConfig.max_turns` carries the turn budget and the driver enforces it.** All three already do, and IN2 simply stops passing `None` (`crates/kinewright-app/src/chat_ui.rs:433`).

2. **"Turn" is defined once, here, and the divergence is named rather than smoothed over.** A **turn** is one `AgentSession::send_user_message` accepted by the harness. Codex (`crates/kinewright-agent/src/drivers.rs:341-348`) and Cursor (`crates/kinewright-agent/src/cursor.rs:317-323`) enforce exactly that, refusing the send with `AgentError::Harness("Turn cap reached ({cap}); …")`; Cursor additionally floors the cap at `cap.max(1)` (`cursor.rs:317-318`), which §2.1 rule 2's clamp makes uniform across the three. Claude counts something else — assistant messages inside one send, `is_claude_assistant_message` at `drivers.rs:772-784` — and **kills the child process**, emitting `AgentEvent::Error`. Both stop the session; they stop it at different boundaries and in different ways.

3. **The pump therefore counts turns itself and is the assertion's subject.** `SessionCounters.turns` increments on each accepted `send_user_message`, and `pump_session` returns `SessionStop::Budget(BudgetKind::Turns)` when the count reaches `limits.max_turns` — independently of whichever driver also refuses. §9 clause 5's budget assertions are written against `ScriptedDriver`, whose turn semantics are the pump's by construction (§8 rule 6), and §10 limit 4 records that the three production drivers' boundaries are not asserted.

### 5.2 Wall time and tokens

4. **Wall time is enforced in `pump_session` on every 100 ms tick**, the arm `collect_session` already has at `eval.rs:3895-3903`, returning `SessionStop::Budget(BudgetKind::WallTime)` after `session.interrupt()`.

5. **The token arm is new code, not a copy.** `EvalBudgets::max_tokens` is the one budget `collect_session` never enforces mid-flight — it is scored after the fact at `eval.rs:5580-5583` — so the arm the pump needs is modelled on `max_cost_usd`'s (`eval.rs:3955-3970`), which accumulates from `AgentEvent::Cost` and breaks mid-loop. The contract says so rather than calling it a move.

6. **A token budget is a post-turn stop and the contract states the real ceiling.** `AgentEvent::Cost` is emitted once per turn completion — on Claude's `result` line (`crates/kinewright-agent/src/protocol.rs:115-127`) and on Codex's `turn.completed` (`protocol.rs:158-190`) — so the earliest a token bound can fire is **after** a turn that already exceeded it. The enforced ceiling is therefore `max_tokens + one turn`, and that is what `SessionStop::Budget(BudgetKind::Tokens)` means. A budget that does not say this is not the number it claims.

7. **The accumulated quantity is `input_tokens + output_tokens`**, both of which every reporting harness fills (`crates/kinewright-core/src/agent.rs:64-75`). `cached_input_tokens` and `cache_creation_input_tokens` are already folded into Claude's `input_tokens` by the adapter (`protocol.rs:101-112`) and are not double-counted here.

### 5.3 Per harness, and the defaults

8. **What each harness reports** (probe-1 P3, re-read for this revision):

| | Claude Code | Codex | Cursor |
| --- | --- | --- | --- |
| `AgentEvent::Cost` | yes, `protocol.rs:115-127` | yes, `protocol.rs:158-190` | **never constructed** |
| token budget enforceable | yes | yes | **no** |
| system prompt per turn | once (`drivers.rs:189-193`) | **every turn**, with a growing recap (`drivers.rs:424-437`) | turn 0 only (`cursor.rs:344-347`) |
| turn cap semantics | assistant messages, kills the child | `send_user_message`, refuses | `send_user_message`, refuses, floored at 1 |

9. **On a harness that reports no tokens the token budget reads "unknown" and the session runs on turns and wall time**, and the card says so in those words (N1). It is not silently treated as zero and it is not a reason to refuse to start: Cursor sessions are turn-and-time bounded, which are two of the three budgets the roadmap row asks for.

10. **The three budget defaults are the one figure this contract still carries a marker for — `[impl]`, not `[probe-2]`** (N2.5, probe-2 disagreement 13). Probe-2 could not measure them and says so plainly: figure 2 asks for a six-turn `ScriptedDriver` session, `ScriptedDriver` is §8's own deliverable, probe-1 P6 established that no existing fake makes tool calls, and no harness is authenticated in that environment — so **nothing in the tree at `7e85972` can run a session at all**. The provisional values stand as **6 turns / 90 s / 40 000 tokens**, chosen knowing §3.4 rule 25's measured per-turn arithmetic (Codex re-sends 3 418–4 973 B every turn, ~5–7 k tokens over six turns). The implementer re-runs the figure after §8 lands and writes the three constants into §2.1's defaults in the same commit. **No clause in §9 asserts any of the three values**; clause 5 asserts that each bound *fires*, at whatever the constant is.

### 5.4 Telemetry

11. **The six `AgentEvent::Cost` mirrors get their first production writer, and it accumulates** (critic B4, N2.5/B4). `mirror_agent_cost` (`server.rs:16352-16370`, exported `lib.rs:48`) writes all six `IncidentTelemetry` fields today and has **zero** production callers — its only three are test assertions at `server.rs:26745`, `:26767`, `:26782`. It **overwrites**: all six are straight assignments. Since the pump sees one `Cost` event per turn, calling it per event would leave the telemetry holding turn six's tokens while `SessionCounters` holds the sum, and §11 item 1's person would read two different numbers on one card. **The rule:** the pump accumulates into a local `IncidentTelemetry` across the session and mirrors **once**, at the end, through `IncidentLog::telemetry_mut` (`incident.rs:2516`) under §4.4 rule 18's lock discipline, so the incident's token totals equal `SessionCounters`' by construction. Clause 7 asserts that equality. This is real work in §12's row, not a free ride.

12. **Two fields are added, both skipping when empty**, so `IN1_INCIDENT_SERIALIZED_BYTES = 819` cannot move (`tool_calls` carries no `skip_serializing_if`, so a bare `turns: u32` beside it would add `,"turns":0` to **every** incident and break an `assert_eq!`):

```rust
/// Turns the resolving session spent, when a session resolved it.
#[serde(skip_serializing_if = "Option::is_none")]
pub turns: Option<u32>,
/// Who resolved it, when it was not the router; or why a session stopped
/// without resolving it (IN2 §0.4 p).
#[serde(skip_serializing_if = "Option::is_none")]
pub resolver: Option<IncidentResolver>,

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentResolver {
    Router,
    Person,
    Session { harness: String, model: Option<String>, stop: String },
}
```

   A router-resolved incident costs **zero** extra keys, which is IN1 §8 rule 4's shape. `stop` is a `String` rather than a `&'static str` because `StopReason::Harness` and `Observer` carry a message. **`IncidentTelemetry` stops being `Copy`, at zero cost**: probe-2 measured the fallout as **0** — `cargo check --workspace --all-targets` is clean with `#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]`, over **24** lines workspace-wide that name `.telemetry` or `IncidentTelemetry` (core 16, agent 6, app 1, `lib.rs` 1), none of which copies it by value. The two fields together add **+126 B** to a fully resolved incident (probe-2 T1).

13. **`QuestionKind::Recovery` gets its first non-zero count.** IN1 §8 rules 1–3 declared it and IN1 asked none; a session that ends `Explained` records one, which makes IN4's *"zero recovery questions asked"* gate falsifiable rather than vacuous.

### 5.5 What the card shows while a session runs

14. **`state_label` reads `"Investigating"`** and three rows join `details`, already `Vec<(&'static str, String)>` (`incident_ui.rs:47`): `turns` as `"2 of 6"`, `tokens` as `"12 400 of 40 000"` or the literal `"unknown"` on a harness that reports none, and `elapsed` as `"18s of 90s"`. `headline` stays `&'static str`, so no number reaches it (IN1 §5.3 rule 25). A fourth row, `stopped`, appears after §3.7 rule 39's stopped end.
15. **The counters are read from the pump's `Arc<SharedCounters>`, not from telemetry**, because telemetry is written once at the end (rule 11) and the card is drawn every frame. `SharedCounters::snapshot()` is five relaxed atomic loads and is called once per frame per running session, at most twice.

---

## 6. Safety: "zero destructive actions" as three checks, and the policy boundary

### 6.1 The three populations, named

**Three populations exist in the tree and they do not agree** (probe-1 P4, brief critic B3). `destructiveHint: true` marks **12** registry tools — 4 generated (`crates/kinewright-agent/src/schema.rs:361-366`: `delete_clip`, `ripple_delete_clip`, `remove_track`, `remove_audio_bus`) and 8 hand-written (`server.rs:12585`/`:12593` `import_lut_asset`, `:12597`/`:12605` `convert_legacy_look`, `:12627`/`:12635` `clear_media_cache`, `:12663`/`:12671` `invoke_capability`, `:12687`/`:12695` `commit_edit_plan`, `:12837`/`:12845` `capture_room_tone`, `:12921`/`:12929` `queue_export`, `:12987`/`:12995` `apply_edit_plan`). The **broker's** six `confirm(` sites gate **8** of those 12. `EditPlanPreview.destructive_operations` names **7** `Operation` variants (`crates/kinewright-agent/src/runtime.rs:349-363`) and is a **reporting** field, not a gate. Part A names all three and says what each check quantifies over, rather than asserting one number over a population it does not fit. **Part A moves the first population to 15** (rule 5, critic S6).

### 6.2 The three checks

1. **Check (i) — the session's tool surface, at the CLI.** `SessionConfig.tool_names` is `INVESTIGATOR_TOOL_NAMES`, the six of §3.3 rule 19. `discard_edit_plan` is dropped; the two `destructiveHint` **carriers**, `invoke_capability` and `commit_edit_plan`, stay, because a session that cannot dispatch a capability or commit a branch plan cannot build a proposal at all (N1/B3, N2.5/B1).

2. **Check (i) continued — the capability denylist, server-side, at the one chokepoint that exists** (§0.1 N2/g, N2.5/B9). `tool_names` is an allowlist passed to the harness CLI while the server applies one global filter for everybody (`server.rs:727-732`, IN1 §13 D11), so a CLI allowlist alone leaves every registry capability reachable *through* `invoke_capability`. `KinewrightMcp` therefore gains `capability_denylist: &'static [&'static str]`, consulted at `server.rs:777-782` beside the existing `is_invocable_capability` check (`runtime.rs:194-206`, a free function with no session context, which is why the denylist is a field and not a predicate).

   **The denylist is derived from a complete side-effect inventory, printed here so it can be checked rather than trusted.** The question the inventory answers, for every capability a session can reach: *does its effect land outside the branch document?*

| capability | effect outside the branch document | denied? |
| --- | --- | :-: |
| `clear_media_cache` | deletes the shared media cache | **yes** |
| `import_lut_asset` | copies bytes under the project directory | **yes** (also brokered) |
| `convert_legacy_look` | copies bytes under the project directory | **yes** (also brokered) |
| `capture_room_tone` | writes 48 kHz samples under the project directory | **yes** (also brokered) |
| `queue_export` | writes or overwrites an output file | **yes** (also brokered on `overwrite`) |
| `resolve_incident` | writes the person's live incident log | **yes** (N1/B7) |
| `cancel_export` | cancels the person's **running** export job (`server.rs:1174-1177`) | **yes** |
| `cancel_analysis` | cancels the person's **running** analysis (`server.rs:1192-1195`) | **yes** |
| `request_analysis` | starts analysis work on the shared engine (`server.rs:1188-1191`) | **yes** |
| `import_media` / `relink_media` | probes an arbitrary filesystem path; the document effect is branch-local | **no** — read-only, named deliberately |
| `render_color_proof`, `get_frame_at`, `get_video_scopes*`, `track_*` | decode work, shared-cache writes, CPU | **no** — read-only and bounded by the session's wall-time budget, named deliberately |
| `apply_edit_plan` and the four generated destructive operation tools | — | **no entry needed**: `is_invocable_capability` already refuses all five (`runtime.rs:196-206` excludes `apply_edit_plan`; a generated operation tool is not in `INSPECTOR_TOOL_NAMES`) |

   Revision 1's list of six missed the last three of the denied nine (critic B9): they are registry capabilities, they pass `is_invocable_capability`, they are not `Operation`s so check (ii) never sees them, they are not brokered so check (iii) never sees them, and §3.3 rule 13 hands the investigator server *the project's own* `Arc<dyn Export>` and `Arc<dyn Analysis>` (`server.rs:700-715`). **An investigator session could cancel the person's running export.** The resulting list is **nine**:

```rust
const INVESTIGATOR_CAPABILITY_DENYLIST: [&str; 9] = [
    "clear_media_cache",   // destructiveHint, not a carrier
    "import_lut_asset",    // destructiveHint, not a carrier
    "convert_legacy_look", // destructiveHint, not a carrier
    "capture_room_tone",   // destructiveHint, not a carrier
    "queue_export",        // destructiveHint, not a carrier
    "resolve_incident",    // N1/B7: only the app records an outcome
    "cancel_export",       // acts on the live application's queue, not the branch
    "cancel_analysis",     // acts on the live application's engine, not the branch
    "request_analysis",    // starts work on the live application's engine
];
```

   A denied name returns an `error_text` naming the capability and the reason. **Every other server keeps an empty denylist and is byte-for-byte unchanged.** This is not IN1 §13 D11's mechanism and does not pre-build for it (§1 non-delivery 7).

   **The test runs in both directions** (N2.5/B9), as §9.1 item 27: every name in `INVESTIGATOR_CAPABILITY_DENYLIST` exists in `INSPECTOR_TOOL_NAMES` (so a rename cannot silently empty the list), and every capability the inventory above marks **yes** is in the denylist (so a capability added to the inventory cannot be forgotten). The inventory's twelve rows live beside the const as a doc comment, which is what the second half reads.

3. **Check (ii) — the branch's operations are checked before the proposal is recorded.** `propose_fix` matches every operation in the branch's applied list against the **seven**-variant set `runtime.rs:349-363` names. A hit refuses with `proposal_destructive` (§4.1 rule 6 code 5), records nothing on the incident, returns the incident to `Open` with the offending variant's `operation_tool_name` on telemetry (§3.7 rule 38), and ends the session. The set is lifted into one named predicate — `pub fn is_destructive_operation(op: &Operation) -> bool` in `runtime.rs` — so `EditPlanPreview`'s filter, `propose_fix`'s check and rule 5's broker gate all read **one** list and cannot drift.

4. **`resolve_incident` is out of reach twice over**, because one lock is not a rule: it is not on `INVESTIGATOR_TOOL_NAMES` (it is not served at all), and it is on `INVESTIGATOR_CAPABILITY_DENYLIST`. §9 clause 10 asserts the denial positively, by making the call and reading the refusal.

5. **Check (iii) — the broker gate is widened from three variants to seven, and three generated tools gain the annotation that follows.** Today `confirmation_description` (`server.rs:1246-1268`) matches `DeleteClip | RippleDeleteClip` and `RemoveTrack` **only when the track has clips**, `_ => None` otherwise; `plan_confirmation_description` (`server.rs:16749-16787`) counts the same three. `RemoveBin`, `RemoveStringOut`, `RemoveSyncGroup` and `RemoveAudioBus` pass ungated and destroy a bin, a string-out, a sync group or a bus without a confirmation. Both functions **must** cover all seven, with the four new sentences exactly as probe-2 T8 measured them:

| variant | field | `confirmation_description`'s sentence |
| --- | --- | --- |
| `DeleteClip { clip }` (`crates/kinewright-core/src/operation.rs:211`) | `ClipId` | **unchanged** |
| `RippleDeleteClip { clip }` (`:224`) | `ClipId` | **unchanged** |
| `RemoveTrack { track }` (`:95`) | `TrackId` | **unchanged**, including the empty-track `None` |
| `RemoveBin { bin }` (`:56`) | `BinId` | *"The agent wants to remove bin {bin} and unfile its assets. This edit can be undone."* |
| `RemoveStringOut { string_out }` (`:67`) | `StringOutId` | *"The agent wants to remove string-out {string_out}. This edit can be undone."* |
| `RemoveSyncGroup { sync_group }` (`:73`) | `SyncGroupId` | *"The agent wants to remove sync group {sync_group}. This edit can be undone."* |
| `RemoveAudioBus { bus }` (`:79`) | `AudioBusId` | *"The agent wants to remove audio bus {bus} and its routing. This edit can be undone."* |

   **`remove_bin`, `remove_string_out` and `remove_sync_group` gain `destructive(true)`** in `schema.rs:361-366`'s `matches!` (critic S6). All three are generated operation tools — `UNGENERATED_OPERATION_VARIANTS` is only `["RelinkAsset", "AddLutAsset", "ConvertLegacyLook"]` (`schema.rs:123-125`) — and widening the gate without the annotation would create a **fourth** disagreeing population: a tool that raises a confirmation while advertising `destructiveHint: false`. Three words, and `destructiveHint` goes **12 → 15**. The `serialized_bytes` this moves is folded into §6.4's decomposition.

6. **The pinned description texts, measured: zero move, and the bin-only sentence is pinned as written.** Probe-2 T8 widened both functions and ran `cargo test -p kinewright-agent`: **513 lib + 38 + 6 + 59 + 1 + 1, zero failures, with no test file edited.** The only pinned description string in `crates/` is `"Plan removes 1 clip and 1 track - approve?"` at **`crates/kinewright-agent/tests/mcp_server.rs:2007`** and **`crates/kinewright-agent/src/server.rs:28522`**, and the widened `plan_confirmation_description` emits it unchanged. `confirmation_description`'s three existing sentences are asserted nowhere at `7e85972`, which the probe confirmed, so the four new ones move nothing.

   **The rule that keeps the pin green has a consequence the contract states rather than leaving to the implementer** (probe-2 disagreement 10, N2.5). The clip-and-track clause is **always** emitted, so a plan that removes only a bin reads, verbatim and measured:

   > `"Plan removes 0 clips and 0 tracks, and 1 bin - approve?"`

   This is **accepted as written and pinned**. A second sentence shape for the no-clip-no-track case would make the pinned sentence one of two branches, which is a worse property than one slightly wooden sentence that is always the same shape; and the sentence a person reads is still true. §9.1 item 29 asserts both this string and the unchanged one verbatim.

7. **What the widening does not do.** It does not gate the four `confirm(` sites that guard **non**-`Operation` destruction — `import_lut_asset` (`server.rs:2145`), `capture_room_tone` (`:2363`), `convert_legacy_look` (`:2718`) and `queue_export` with `overwrite` (`:3957`). Those are already brokered and are additionally denied to the investigator by rule 2. **"Zero `ConfirmationRequest` in an investigator session" is therefore a stronger property than the operation set alone would give, and it is a measured zero rather than an assumed one** (critic B5): §8's script **S11** makes the session commit a plan containing `RemoveBin`, which raises exactly one request that is rejected within one tick, and clause 3 asserts zero over the resolving scripts *beside* S11's one, over a population that can be non-zero.

### 6.3 The policy boundary

8. **The policy boundary, normative.** A session acts within `PolicyClass` and nothing outside it. Concretely: **(a)** `AutoApply` is the router's — it **sends** the recovery in the same `route_incidents` tick that observes the incident (`app.rs:1144-1183`) and calls `note_auto_applied`, which suppresses the pair (`incident.rs:2549-2555`), while the `Resolved(Applied)` write happens later in `reconcile_router_sends` when the landing arrives (`app.rs:1169`, comment at `:1189-1192`). The exclusion of those three codes does **not** depend on when the landing arrives, because they are off `INVESTIGATOR_ALLOWLIST` by construction (§3.1 rule 2); revision 1's "it resolves in the same tick" was the wrong mechanism for the right conclusion (critic S13). §9 clause 2 asserts zero sessions for the three and asserts the router's three `Applied` landings beside them, so the clause cannot pass by nothing happening. **(b)** An `Explain` session **proposes** — it may read, it may build and prove operations on its **branch**, and its only route to the live document is a person's approval. **(c)** `AskFirst` gets no entry in IN2 (N1/Q4), so no arm of this contract can reach it.

9. **`class` is stored on the incident at observe time** (`incident.rs:2463`, stored at `:2469`) and the session predicate reads that stored value, never a re-derived one — the same discipline `audit_new_incident` already keeps (`app.rs:1205-1214`).

10. **The app records outcomes; the model does not.** §4.5 rule 22 is the whole table, and rule 4 above is the mechanism that makes it enforceable rather than hoped for.

### 6.4 The registry, measured

11. **The registry sextuple moves by exactly one capability and three annotation flips; the served quad does not move at all.** Probe-2 T4 registered `propose_fix` after `resolve_incident` with `read_only(false) destructive(false) idempotent(false) open_world(false)` and a stub handler, and measured:

```
registry = ToolSurfaceMetrics { tool_count: 141, serialized_bytes: 1552577,
                                input_schema_bytes: 1407717, description_bytes: 121726 }
served   = ToolSurfaceMetrics { tool_count: 7, serialized_bytes: 5660,
                                input_schema_bytes: 3510, description_bytes: 998 }
operation_tools().len() = 54
```

   **The three counts are pinned hard: `141 / 54 / 87`.** `INSPECTOR_TOOL_NAMES` goes 86 → 87, the registry 140 → 141, `operation_tools()` stays 54. **The three byte figures are pinned as probe-2's measurement carrying its decomposition** (probe-2 disagreement 6, §0.4 n): `1 552 577 / 1 407 717 / 121 726`, which is `+1 276 / +705 / +411` over `1 551 301 / 1 407 012 / 121 315`, decomposing as **+160 B fixed** (the name, the annotations, the JSON envelope) **+705 B** of generated `ProposeFixArgs` schema **+411 B** of description text. This contract deliberately does not fix the description text, so the implementer's three numbers differ from probe-2's by exactly the difference in description length, **plus** whatever the three `destructive(true)` flips of rule 5 add to `serialized_bytes`. The implementer measures and re-pins in the same commit; clause 18 asserts the counts and the decomposition, not a remembered byte.

   **Measured at stage D (fbaf52e) and unmoved by stage C:** the registry sextuple is **`141 / 54 / 87 / 1 552 431 / 1 407 446 / 121 854`**, which is **+1 130 / +434 / +539** over IN1b's `1 551 301 / 1 407 012 / 121 315`, decomposing as **+434 B** of the two-field `ProposeFixArgs` schema (probe-2's four-field prototype was +705 B), **+539 B** of `propose_fix` description text (probe-2's was 411 B) and **+157 B** fixed — the +160 B envelope less 3 B for rule 5's three `destructive(true)` flips, each one byte shorter than `false`. 157 + 434 + 539 = 1 130 exactly. The decomposition is written beside the pin in `server.rs`; stage C's per-instance served allowlist (§0.5 E-C6) touches no registry byte.

12. **The served quad is `7 / 5 660 / 3 510 / 998`, unmoved, for the eighteenth consecutive measurement** — measured by probe-2 with `propose_fix` registered **and** by probe-2 T11 with `Document.investigator` present. `served_tools()` filters by `COMPACT_TOOL_NAMES` (`server.rs:727-732`) and `propose_fix` is not on it, the same argument IN1 §6.6 and IN1b §6.4 made for `get_incidents` and `resolve_incident`.

13. **Six pin sites must be edited, not the three revision 1 named** (probe-2 disagreement 5). `cc7_the_agent_surface_is_unchanged_by_this_slice` fails at two sites revision 1 named nowhere. The six are listed in §14 row D and asserted by §9 clause 19; the three quad **value** sites (`tests/mcp_server.rs:2567-2569`, `:10420`, `server.rs:26856`) and the five counter-**word** sites (`tests/mcp_server.rs:2427`, `:10346`, `:10366`, `:10421`, `server.rs:26806`) are named there too.

---

## 7. The deterministic sixteen, of which three are buildable

1. **`policy_recovery` gains one arm and keeps its shape.** It is still the single producer of recoveries (`crates/kinewright-core/src/incident.rs:2268-2296`), which is what makes IN1 §9 clause 6's equality assertion meaningful. The `Explain` arm becomes:

```rust
PolicyClass::Explain => {
    let mut actions = Vec::new();
    if let Some(action) = deterministic_recovery(code, subject, evidence) {
        actions.push(action);
    }
    actions.push(RecoveryAction {
        label: EXPLAIN_LABEL,
        kind: RecoveryKind::Explain(explain_body(code)),
    });
    actions
}
```

   so a row with a buildable operation offers **a button and the sentence**, in that order, and every other row is byte-for-byte what it is today. `card_actions` needs no change at all: it already enables an action iff its kind is `RecoveryKind::Operation` (`incident_ui.rs:437-445`).

2. **`deterministic_recovery` is an exhaustive `match` over `IncidentCode` with no wildcard**, so a code added later cannot silently default to "no button". Its arms are the sixteen below; every other code returns `None` through an explicit `|`-joined arm.

3. **The class column of `POLICY` does not change** (N1/B1). All sixteen stay `Explain` with `PolicyPredicate::Always`, `POLICY.len()` stays **67**, and the three `AutoApply` rows are still the only rows the router applies without being asked. Changing a row's declared class is IN3's, with D8's bodies.

4. **The sixteen, each with its builder and whether it is buildable at `7e85972` — measured, with no markers left.** Probe-2 T2 built every one of the sixteen through its real producer and reported `probed()`, `observed` and `allowed` for each (`probe2/t2-deterministic-sixteen.txt`).

| # | code | candidate builder | buildable? |
| ---: | --- | --- | :-: |
| 1 | `unknown_source_range` | `assume_rec709_operation(asset, probed)` (`incident.rs:2336-2341`), under `rec709_compatible(probed)` | **yes** |
| 2 | `unknown_source_bit_depth` | the same | **yes** |
| 3 | `unknown_source_white_point` | the same | **yes** |
| 4 | `unsupported_source_primaries` | — | **no** (rule 5) |
| 5 | `unsupported_source_transfer` | — | **no** |
| 6 | `unsupported_source_matrix` | — | **no** |
| 7 | `unsupported_source_range` | — | **no** |
| 8 | `unsupported_source_white_point` | — | **no** |
| 9 | `unsupported_source_bit_depth` | — | **no** |
| 10 | `unsupported_source_combination` | — | **no** |
| 11 | `unsupported_delivery_codec` | — | **no** (rule 6) |
| 12 | `unsupported_delivery_color` | — | **no** (rule 6, and the reason nobody expected) |
| 13 | `delivery_pixel_format_depth_mismatch` | — | **no** (rule 6) |
| 14 | `delivery_encoder_pixel_format_unavailable` | — | **no** (rule 6) |
| 15 | `color_qc_node_budget_exceeded` | — | **no** (rule 6) |
| 16 | `delivery_verification_frame_count_out_of_range` | — | **no** (rule 6) |

   **Row 3 carries no marker.** Revision 1 wrote *"yes, [probe-2] on reachability"*; all three of rows 1–3 are constructed at `crates/kinewright-core/src/media.rs:2697`, `:2699`, `:2701`, asserted by code string at `crates/kinewright-media/src/in1_fixtures.rs:244` and `crates/kinewright-media/src/cc1_fixtures.rs:3084`, `:3091`, and read at `crates/kinewright-agent/src/color_status.rs:1262`. The reachability was settled in the tree all along (critic B8, N8).

5. **Rows 1–3 are buildable and rows 4–10 are not, and the reason is a predicate *plus* a reachability argument — both halves, because the predicate alone does not do it** (probe-2 disagreement 8). Their producer is the same: `IncidentObservation::from_media_error`'s `MediaError::SourceColorForAsset` arm (`incident.rs:1385-1397`), which emits `IncidentEvidence::SourceColor { probed, assumption }` and sets the subject to `IncidentSubject::Asset(refusal.asset)` itself, so every input `assume_rec709_operation` needs is on the evidence and the asset id is on the subject.
   - **The predicate.** `rec709_compatible` (`incident.rs:2370-2391`) is true only when every one of primaries, transfer, matrix and white point is `Unknown` or its Rec.709 value, range is `Unknown | Limited | Full`, and bit depth is `Unknown` or integer `8..=16`. `recovery_description` (`incident.rs:2313-2333`) *fills* `Unknown` fields and preserves known ones.
   - **The reachability.** `rec709_compatible` is a predicate over the **probe**, not over the code: probe-2 measured it returning `true` for an all-`Unknown` probe on an `unsupported_*` code. What separates the two groups is that an `unsupported_*` row **is only reached** when the named field carries a known non-Rec.709 value, and `rec709_compatible` of such a probe is `false` (measured on a BT.2020 probe). So the builder would have to **overwrite** a known value — *"the silent behaviour CC1 §1 forbids"*, in `rec709_compatible`'s own doc comment (`incident.rs:2358-2366`).

   Revision 1 stated the first half and read as though the predicate did the whole job. The seven are IN1 §13 D8's and are named in §13 D-A2 with a cost and an owner.

6. **Rows 11–16 are all `no`, for two independent reasons either of which is sufficient** (probe-2 T2, disagreement 7). Revision 1 marked all six `[probe-2]` and expected row 12 to be the most likely **yes**. Every one of the six is produced through `from_media_error`'s `DeliveryColor` / `DeliveryVerification` / `ColorQc` arms (`incident.rs:1432-1467`), whose evidence is `media_evidence(error, code)` = `IncidentEvidence::MediaError { code: &'static str, message: String }` (`incident.rs:1509-1514`).
   - **(1) The signature.** `deterministic_recovery(code, subject, evidence)` never sees `observed` or `allowed`: those are fields of `Incident`, not of `IncidentEvidence`, and `policy_recovery` is called at observe time with the evidence alone (`incident.rs:2268`). The evidence these six carry is two strings — the code, and the rendered sentence. `evidence.probed()` is `None` for all six.
   - **(2) The type.** Even given `observed`/`allowed`, both are prose: `DeliveryColorMismatch.observed`/`.allowed` are `format!("{:?}", …)` of a colour enum (`crates/kinewright-core/src/delivery.rs:1084-1116`); `ColorQcError::NodeBudgetExceeded` carries `"17"` and `"1..=16"`. Building a typed `Operation` means parsing `Debug` output, which is judgement and is forbidden by §1 non-delivery 4.

   **Row 12 is `no` on two further counts**, and they are worth recording because the expectation was reasonable: the error is raised once **per field**, so the incident carries one mismatch and not a `ColorContext` that `SetColorContext` could take; and the target that is wrong is `settings.delivery_color` in the **export settings** (`delivery.rs:413-425`), not `Document.color_context` (`crates/kinewright-core/src/model.rs:1106-1109`). Rows 15 and 16 are `no` for the reason revision 1 expected — the out-of-range value is an argument to a capability call, so no document field changes. All six join **§13 D-A2**, and all six join the **allowlist** (§3.1 rule 2), because a row with no button and no honest operation is exactly the row a session is for.

7. **The no-harness fallback and the card's button are the same value, by construction** — IN1 §5.3 rule 31 generalised. `card_actions` maps `incident.recoveries` (stored at observe time, `incident.rs:2463-2465`) one-to-one, and `policy_recovery` is the single producer, so with the investigator off, or with no harness installed, the button on the card is exactly the operation an investigator would have proposed. §9 clause 4 asserts it over the three rows that have one.

8. **What this does to the programme principle, stated honestly.** `docs/ROADMAP-AND-WORKFLOWS.md:791-794` says *"Never below the button. With the investigator off or no harness installed, **every** incident still renders its typed recoveries as buttons on the card."* At `7e85972` that is true of **3** of 67 codes — the three `AutoApply` rows. After Part A it is true of **6 of 67**: those three plus §7 rows 1–3. Revision 1 wrote "3 + N" with N unmeasured; **N = 0** and the number is final (probe-2 disagreement 7). The remaining 61 are owned by name: the seven `unsupported_source_*` rows by IN3 with D8 (§13 D-A2), the six delivery and clamp rows by IN3 with D-A2, the eleven `AskFirst` candidates by IN3 (§13 D-A1), and the other thirty-seven by the investigator, which is what this contract is for. The roadmap row is amended at promotion to say **3 of 67 gain a pressable recovery** (§14).

---

## 8. The scripted driver

1. **`ScriptedDriver` is production code in `kinewright-agent`, outside `#[cfg(feature = "eval-harness")]`** (N1/Q12), in `crates/kinewright-agent/src/scripted.rs`, declared in `lib.rs` with no `cfg`. It is the fourth `AgentDriver` implementation beside `ClaudeCodeDriver`, `CodexDriver` and `CursorAcpDriver`.

2. **Why not `eval`'s `FakeDriver`.** It is `#[cfg(test)]`-private inside `eval.rs`'s test module (`eval.rs:9221`, `:9243`, `:9272`), it ignores the whole `SessionConfig` including `mcp_url`, it makes **no tool calls**, and it is one-shot through `Option::take`, so it cannot drive a multi-turn budget test (probe-1 P6). It is also behind `eval-harness`, which `cargo check -p kinewright-app` does not compile (`crates/kinewright-app/Cargo.toml:15`) — the argument that matters, since it is the app's own tests that must reach it. The stronger claim *"the app cannot compile `eval` at all"* is true only of that one build: the crate's own `default = ["eval-harness"]` (`crates/kinewright-agent/Cargo.toml:18`) unifies the feature on under `--workspace`, and this contract does not rely on the stronger claim.

3. **`AgentSession` has no tool-calling method** — the trait is `send_user_message` / `events` / `interrupt` and nothing else (`crates/kinewright-core/src/agent.rs:105-114`) — so a driver that *makes* tool calls must hold its own MCP client (probe-1 P6). **The client is `rmcp`'s streamable-HTTP client, it is 70 non-comment lines, and it costs zero new dependencies** (probe-2 T7). `rmcp = "=3.1.2"` is a non-optional, non-dev workspace dependency with `features = ["client", "server", "transport-streamable-http-client-reqwest", "transport-streamable-http-server"]` already on (`Cargo.toml:33`), and `reqwest 0.13.4` and `hyper 1.11.0` are already in `Cargo.lock`. No hand-rolled JSON-RPC client exists in production code in either crate — `StreamableHttpClientTransport` appears only in test code (`eval.rs:12603`, `:12647`, `:13726` and 58 sites in `tests/mcp_server.rs`), and `cursor.rs:448`'s `"initialize"` is the ACP handshake with a different protocol. `ScriptedSession` therefore builds `().serve(StreamableHttpClientTransport::from_uri(config.mcp_url))` on an owned `tokio::runtime::Builder::new_current_thread()`, exposes `call_tool`, `list_tools` and `cancel` behind a blocking API, and is driven from an ordinary `std::thread` worker so `send_user_message` stays non-blocking. Probe-2 proved it against a live `McpServer`, reading `served_tools = 7` and `is_error = Some(false)` from `get_timeline_state`, under `cargo check -p kinewright-agent --no-default-features --lib` and `cargo check -p kinewright-app`.

4. **The script format, normative. `ScriptedCost` carries the event's six fields, not two** (critic B4, N2.5/B4): `mirror_agent_cost` passes the event's four `Option` fields straight through (`server.rs:16352-16370`), so a two-field cost would leave four of the six mirrors `None` for every scripted turn and make clause 7 unsatisfiable by any test this contract specifies.

```rust
pub struct ScriptedTurn {
    /// Tool calls made in order, each through the session's own MCP client.
    pub calls: Vec<ScriptedCall>,
    /// The assistant's final message for this turn.
    pub message: String,
    /// The cost event this turn reports, or `None` for a harness that
    /// reports none (the Cursor shape).
    pub cost: Option<ScriptedCost>,
}

pub struct ScriptedCall { pub tool: String, pub arguments: serde_json::Value }

/// The six fields of `AgentEvent::Cost` (`agent.rs:49-75`).
pub struct ScriptedCost {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    pub reasoning_output_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
}

pub struct ScriptedDriver {
    turns: Arc<Mutex<VecDeque<ScriptedTurn>>>,
    authentication: AuthenticationStatus,
}
```

5. **One turn per `send_user_message`.** The session pops the front turn; for each `ScriptedCall` it emits `AgentEvent::ToolCall`, makes the real call, emits `AgentEvent::ToolResult` with the real result; then `AgentEvent::Text(message)`, then `AgentEvent::Cost { … }` when the turn declares one, then `AgentEvent::Done`. A `send_user_message` with an empty script returns `AgentError::Harness("scripted driver has no turn left")`, which is a real stop the pump can observe rather than a hang.

6. **`ScriptedDriver::detect()` is configurable**, returning a `HarnessInfo` whose `authentication` is whichever of the three variants the test asked for, so §2.2 rule 7's `Unknown`-is-available rule and rule 8's one-strike disable are both testable.

7. **What the tests drive with it. Eleven scripts, every one an ordinary `cargo test` with no model and no network beyond loopback.** S11 is new in revision 2 and is what makes clause 3's zero a measured zero (critic B5).

| script | drives | ends |
| --- | --- | --- |
| **S1 the happy proposal** | `get_timeline_state`, `invoke_capability{get_incidents}`, `prepare_edit_plan`, `commit_edit_plan`, `invoke_capability{propose_fix}` | a recorded `IncidentProposal` whose `operations` are the branch's; the app approves; `Resolved(Applied)` |
| **S2 the rejected proposal** | S1's calls | the app rejects; `Resolved(Rejected)`, the live document unchanged, the pair suppressed |
| **S3 the explanation** | `get_timeline_state` and a final message, no `propose_fix` | `Resolved(Explained)`, with one `QuestionKind::Recovery` recorded |
| **S4 turn exhaustion** | `max_turns + 1` turns of one call each | `SessionStop::Budget(Turns)`; the incident **back at `Open`**, not suppressed; counters on telemetry |
| **S5 wall-time exhaustion** | one turn whose script sleeps past the bound | `SessionStop::Budget(WallTime)`; back at `Open` |
| **S6 token exhaustion** | two turns, the second declaring a `ScriptedCost` past the bound | `SessionStop::Budget(Tokens)`; back at `Open`; the ceiling reads `max_tokens + one turn` (§5.2 rule 6) |
| **S7 the destructive proposal** | a branch carrying `Operation::RemoveBin`, then `propose_fix` | refused `proposal_destructive`, **nothing recorded**, back at `Open` |
| **S8 the denied capability** | `invoke_capability` for each of the nine denylist names | nine refusals by name; the live log, the export queue and the analysis engine all unchanged |
| **S9 the auto-apply negative** | a session is attempted for each of the three `AutoApply` codes | **zero** sessions start, and the router's three `Applied` landings still arrive |
| **S10 the conflicting approval** | S1, then a live edit, then Approve | one retry, then `Applied`; and with a second edit between, `stale` and **Re-investigate** |
| **S11 the destructive commit** | `prepare_edit_plan` + `commit_edit_plan` with `RemoveBin`, before any `propose_fix` | **exactly one** `ConfirmationRequest` on the investigator's broker, carrying `incident == Some(id)`, rejected within one tick; `SessionStop::PolicyViolation`; no branch change; back at `Open` |

8. **Every assertion is a positive count.** A script that made no call would pass a "zero destructive operations" assertion vacuously, so each of S1–S11 asserts what it *did* — tool calls made, events emitted, proposals recorded, ends written — and then asserts the zero beside it (N1/Q2).

---

## 9. Tests and the exit gate

### 9.1 Tests, by name

Every test is an ordinary `cargo test` on **both** CI operating systems, needs no model, no network beyond loopback and no audio device, and follows the house `in2_…` naming. **Forty numbered items, fifty-one test functions** — item 40 names twelve at once because they are the app's behavioural suite and each is one short function over the same fixture.

**core — `crates/kinewright-core/src/incident.rs`**

1. `in2_the_allowlist_is_fifty_four_explain_codes_that_are_policy_rows_fourteen_to_sixty_seven` — `INVESTIGATOR_ALLOWLIST.len() == 54`, the 54 pairwise distinct, each member's `policy_entry(code)` row reading `PolicyClass::Explain` with `PolicyPredicate::Always`, no member one of the three `AutoApply` codes, the members equal to `POLICY[13..67]` in order, and `policy_recovery` yielding no `RecoveryKind::Operation` for any member. The converse is asserted over the rows §7 rule 4 marks **yes** and reads that column rather than a literal (§3.1 rules 3, 7).
2. `in2_investigating_counts_as_open_at_every_site` — `is_open()` over all three states, plus the six behaviours §3.6 rule 33 decides, each a **positive** count: an `Investigating` incident is in `open()`, in `open_count()`, in `incident_panel_rows`, in the Media panel's filter, is deduped by a second observation, and is refreshed by `refresh_revision`; and `card_actions`' revert guard still reads `Resolved(Applied)` only.
3. `in2_begin_investigation_writes_one_field_and_only_on_an_open_entry` — `true` on `Open`, `false` on `Investigating` and on every `Resolved`, with `count`/`revision`/`opened_at`/`suppressed`/telemetry unchanged in all cases (§3.6 rule 35).
4. `in2_end_investigation_returns_the_incident_to_open_and_suppresses_nothing` — from `Investigating`, the entry reads `Open`, `suppressed` is **empty**, `telemetry.resolver` carries the stop, and a second observation of the same `(code, subject)` **dedups into the same incident** rather than being suppressed. This is N2.5/B3's rule and it fails at `7e85972` because neither the state nor the writer exists (§3.7 rule 38).
5. `in2_an_explicit_reject_resolves_and_suppresses` — the one end that does: `resolve(id, Rejected)` writes `Resolved(Rejected)`, inserts the pair, and the next observation is `Observed::Suppressed`. IN1 §2.3 rule 19 unamended (§4.5 rule 22).
6. `in2_the_three_deterministic_rows_offer_a_button_and_the_sentence` — for each of `unknown_source_range`, `unknown_source_bit_depth`, `unknown_source_white_point`, `policy_recovery` returns two actions in order, the first `Operation` and the second `Explain(explain_body(code))`, and `card_actions`' first action is `enabled` (§7 rules 1, 7).
7. `in2_an_unsupported_colour_row_still_offers_only_its_sentence` — each of the seven `unsupported_source_*` rows returns exactly one `Explain` action, with `rec709_compatible(probed)` asserted `false` for the evidence that opens it **and** asserted `true` for an all-`Unknown` probe, so the test carries rule 5's both halves (§7 rule 5).
8. `in2_the_six_delivery_and_clamp_rows_carry_no_typed_evidence` — for each of §7 rows 11–16, `evidence.probed().is_none()` and the evidence is `IncidentEvidence::MediaError`, which is why no builder exists (§7 rule 6).
9. `in2_a_recorded_proposal_never_serialises_its_operations` — an incident carrying a three-operation proposal serialises with `"operation_count":3` and **without** the substring `"operations"` (§4.2 rule 8).
10. `in2_both_proposal_strings_are_capped_on_their_serialised_length` — an explanation of 240 control characters and a summary of 240 multi-byte characters both serialise to at most 242 B each including quotes, and truncation lands on a `char` boundary (§4.1 rule 5).
11. `in2_the_proposal_and_the_new_telemetry_fields_skip_when_absent` — an `Open` incident with no proposal, no `turns` and no `resolver` serialises without any of the three keys, and its bytes equal the `Open` shape at `7e85972` (§5.4 rule 12).
12. `in2_a_muted_code_round_trips_through_the_document_byte_identically` — `pre_m13_project.json` still round-trips to **1 215 B** / FNV-1a 64 `c9da3186e131e4fd` with the field absent (`crates/kinewright-core/tests/contracts.rs:6478`, `:6487`), and a document with one muted code round-trips that code and ignores an unrecognised one (§2.4).

**core — `crates/kinewright-core/src/actor.rs`**

13. `in2_a_core_spawned_at_a_revision_reports_that_revision` — `Core::spawn_at(document, TimelineRevision(41))` answers `Query::Snapshot` with **41**, the first accepted `Command::Do` takes it to **42**, and `Core::spawn` still answers `TimelineRevision::default()` (§3.3 rule 15; probe-2 T3 wrote and ran this test).
14. `in2_a_branch_spawned_at_a_revision_reports_its_applied_operations_from_empty` — a core from `spawn_at` answers `Query::AppliedOperations` with an empty list before any edit, with the two edits after two, and with one after one is undone (§4.1 rule 4).

**agent — `crates/kinewright-agent/src/session.rs`**

15. `in2_the_pump_stops_at_each_of_its_three_bounds` — three `ScriptedDriver` sessions return `SessionStop::Budget(Turns)`, `Budget(WallTime)` and `Budget(Tokens)`, each with non-zero `SessionCounters` (scripts S4–S6).
16. `in2_the_token_stop_is_a_post_turn_stop` — the accumulated total at the stop is **greater** than `max_tokens`, which is what §5.2 rule 6 says it must be.
17. `in2_a_confirmation_inside_a_session_ends_it_with_a_policy_violation` — a request raised on the session's broker under `ConfirmationPolicy::RejectAndStop` is rejected within one tick, `SessionStop::PolicyViolation` is returned, and the request carried the incident id (§4.6 rule 27).
18. `in2_the_pump_under_approve_all_approves_and_continues` — the same request under `ConfirmationPolicy::ApproveAll` is approved and the session runs on, which is the eval harness's behaviour and the half a single `Option<&ConfirmationBroker>` could not express (§3.5 rule 29d).
19. `in2_a_cancelled_pump_interrupts_from_its_own_thread` — setting the `Arc<AtomicBool>` from another thread returns `SessionStop::Cancelled(_)` within two ticks and `interrupt` was called exactly once (§3.5 rule 27).
20. `in2_a_disconnected_event_stream_stops_the_pump` — dropping the sender returns `SessionStop::Disconnected` (§3.5 rule 29b).

**agent — `crates/kinewright-agent/src/scripted.rs`**

21. `in2_the_scripted_driver_makes_real_tool_calls_through_its_own_client` — against a real `McpServer`, a two-turn script produces ≥ 4 `AgentEvent::ToolCall`/`ToolResult` pairs whose results came from the server, not from the script (§8 rules 3, 5).
22. `in2_a_scripted_driver_reports_whichever_authentication_status_it_was_given` — all three `AuthenticationStatus` variants (§8 rule 6).
23. `in2_a_scripted_cost_fills_all_six_mirror_fields` — a `ScriptedCost` with all six set produces an `AgentEvent::Cost` whose six fields are all `Some`, which is what clause 7 needs (§8 rule 4).

**agent — `crates/kinewright-agent/src/server.rs` and `tests/mcp_server.rs`**

24. `in2_propose_fix_records_the_branchs_operations_and_returns_without_blocking` — after two `commit_edit_plan`s on the branch, `propose_fix` returns inside the test's deadline with `operation_count == 2`, the shared log carries a proposal whose `operations` equal the branch's `Query::AppliedOperations`, `base_revision` equals the live revision the branch was spawned at, the incident's state is still `Investigating`, and `telemetry.tool_calls` advanced by one (§4.1 rule 4, §4.2 rule 8).
25. `in2_propose_fix_refuses_each_of_its_six_codes` — six refusals, each asserted by `code` (§4.1 rule 6).
26. `in2_a_destructive_branch_is_refused_before_anything_is_recorded` — a branch carrying `RemoveBin` makes `propose_fix` refuse `proposal_destructive` with `log.get(id).proposal.is_none()` (§6 rule 3, script S7).
27. `in2_the_denylist_refuses_nine_capabilities_through_invoke_capability` — nine positive refusals on an investigator server, the same nine accepted on a server with an empty denylist, **every** denied name asserted present in `INSPECTOR_TOOL_NAMES`, and every capability the §6 rule 2 inventory marks **yes** asserted present in the denylist (N2.5/B9).
28. `in2_the_broker_gate_covers_all_seven_destructive_variants` — `confirmation_description` returns `Some` for all seven with the four new sentences verbatim, `None` for an empty `RemoveTrack`, and `None` for at least three non-destructive variants (§6 rule 5).
29. `in2_the_plan_confirmation_sentence_is_unchanged_and_the_bin_only_sentence_is_pinned` — `"Plan removes 1 clip and 1 track - approve?"` verbatim for a clip-and-track plan, and `"Plan removes 0 clips and 0 tracks, and 1 bin - approve?"` verbatim for a bin-only one (§6 rule 6).
30. `in2_the_seven_destructive_variants_all_advertise_the_hint` — each of the seven variants' generated or hand-written tool carries `destructiveHint: true`, and `destructiveHint` marks **15** registry tools (§6 rule 5, critic S6).
31. `in2_a_request_from_an_investigator_broker_carries_its_incident` — `incident == Some(id)` on an investigator broker and `None` on a chat-panel one (§4.6 rule 25).
32. `in2_every_code_fits_the_re_measured_ceiling` — the rewrite of `in1b_every_code_fits_the_measured_ceiling` (`server.rs:~26256-26330`), quantified over §4.2 rule 11's 4 288-shape product, asserting the loop's own worst against `IN1_INCIDENT_SERIALIZED_CEILING_BYTES` and the `worst > CEILING / 2` divisor, with `IN1_INCIDENT_SERIALIZED_BYTES` still `assert_eq!` **819** and **no intermediate byte figure asserted** (probe-2 disagreement 4).
33. `in2_the_served_quad_does_not_move_for_the_eighteenth_measurement` — the rewrite of `in1b_the_served_quad_does_not_move_for_the_seventeenth_measurement` (`tests/mcp_server.rs:10366`), asserting `7 / 5 660 / 3 510 / 998` in its three value sites and the counter word in its five word sites (§6.4 rule 13).
34. `in2_the_registry_grows_by_one_capability` — `capability_tool_names().len() == 141`, `INSPECTOR_TOOL_NAMES.len() == 87`, `operation_tools().len() == 54`, the three byte figures at the implementer's own measurement, and `propose_fix` registered directly after `resolve_incident` (§6.4 rule 11).

**agent — one owner, per crate** (N2.5/B6)

35. `in2_the_agent_crate_has_one_agent_event_loop` — a source-shape test over `crates/kinewright-agent/src` **only**, reached through `concat!(env!("CARGO_MANIFEST_DIR"), "/src")`, matching the regex `\.recv_timeout\(|\.recv_deadline\(|crossbeam_channel::select` rather than a bound name, allowlisting the known non-session sites by file (`export_queue.rs`, `acp.rs`, `server.rs`'s broker), asserting **a positive count** of allowlisted sites so it cannot pass by reading nothing, asserting it read at least 10 files, and normalising line endings before matching (IN1b N6's Windows lesson). The only agent-event loop is `session.rs`'s.

**app**

36. `in2_the_app_crate_has_no_agent_event_loop_and_one_pump_call` — the same shape over `crates/kinewright-app/src` **only**, each crate reading its own files: **zero** `send_user_message(` calls outside `chat_ui.rs`'s existing one, exactly **one** `pump_session(` call, and `recovery.rs`'s known `recv_timeout` allowlisted by name with a positive count. Revision 1's single workspace-scoped, variable-name-keyed test could not see the app at all (critic B6).
37. `in2_the_settings_file_round_trips_and_defaults_off` — write, re-read, equal; a missing file yields `enabled == false` **and opens no incident**; an unknown `version` yields the defaults; the budget clamps hold at both ends (§2.1).
38. `in2_an_unreadable_or_unparseable_settings_file_opens_one_agent_incident_and_is_not_overwritten` — exactly one incident, `agent_unclassified`, subject `Agent`, and the file's bytes unchanged afterwards (§2.1 rule 3).
39. `in2_the_config_path_resolves_per_platform_and_reports_durability` — with `XDG_CONFIG_HOME` set, unset-with-`HOME`, and both unset, the resolver returns the three paths T10 measured and `durable` is `false` only in the third (§2.1 rule 1).
40. `in2_a_session_starts_only_for_an_allowlisted_unmuted_code`, `in2_no_session_starts_for_the_three_auto_apply_codes`, `in2_the_queue_dedups_by_code_and_subject_and_caps_at_two`, `in2_the_investigating_card_shows_its_three_counters`, `in2_the_stopped_card_offers_reinvestigate`, `in2_the_proposal_card_offers_approve_reject_and_reinvestigate`, `in2_an_approved_proposal_applies_and_records_applied`, `in2_a_conflicting_approval_retries_once_then_goes_stale`, `in2_a_branch_error_on_approve_leaves_the_incident_open`, `in2_the_off_switch_cancels_a_running_session_and_suppresses_nothing`, `in2_a_hand_resolution_mid_session_keeps_the_persons_outcome`, `in2_the_scripted_suite_drives_eleven_scripts_to_their_ends` — the app's twelve behavioural tests, each asserting its own positive counts, against §§2.3, 3.1, 3.2, 3.7, 4.3, 4.4, 5.5 and §8 rule 7.

### 9.2 Exit gate

Every numbered clause is falsifiable, is discharged by an ordinary `cargo test` on **both** CI operating systems, and needs no model, no network beyond loopback and no audio device. **A clause that can pass at `7e85972` before IN2 exists is not a clause**, and **every clause asserts a positive count** so that none passes over an empty population (N1/Q2). Each names the rule it discharges, the test that fails today, and the **measured** reason it fails.

| # | clause | discharges | test | fails at `7e85972` because, measured |
| ---: | --- | --- | --- | --- |
| 1 | `INVESTIGATOR_ALLOWLIST` holds exactly **54** codes, equal to `POLICY[13..67]`, all `Explain`/`Always`, none of them one of the three `AutoApply` codes, and none of them yielding an `Operation` recovery | §3.1 | item 1 | the const does not exist: `grep -c INVESTIGATOR_ALLOWLIST crates/` = 0 |
| 2 | Over a `ScriptedDriver` run, sessions start for **≥ 6** distinct allowlisted `Explain` codes and for **exactly 0** of `unknown_source_primaries`, `unknown_source_transfer`, `unknown_source_matrix`, whose router `Applied` still lands **3** times | §3.1, §6 rule 8 | items 40, S9 | no session exists; `InvestigatorSession` is not a type and `ProjectSession` has 37 fields, none of them an investigator (`crates/kinewright-app/src/project.rs:252-328`) |
| 3 | **≥ 3** proposals are approved and applied on the live core, each advancing the live revision and each leaving a `Resolved(Applied)` incident, with **0** operations from the seven-variant destructive set and **0** `ConfirmationRequest` raised on any investigator broker across those runs — **beside** script S11's **exactly 1** auto-rejected request, so the zero is measured over a population that can be non-zero | §4.4, §6 rules 1–5 | items 24, 40, S1, S11 | `propose_fix` does not exist, `Incident` has no `proposal` field (`crates/kinewright-core/src/incident.rs:1141-1180`), and `IncidentProposal` is not a type |
| 4 | **exactly 3** `Explain` codes yield a `RecoveryKind::Operation` from `policy_recovery`, and each one's `card_actions` first action is `enabled`, with the `Explain` sentence still second | §7 rules 1, 4, 7 | items 6–8 | `policy_recovery`'s `Explain` arm returns exactly one `RecoveryKind::Explain` for all 64 codes (`incident.rs:2292-2295`), so the count is **0** |
| 5 | Three scripted sessions stop at three **distinct** `BudgetKind` values — `Turns`, `WallTime`, `Tokens` — each with non-zero turns, non-zero elapsed and a recorded end; and the token stop's total exceeds `max_tokens` | §5 | items 15–16, 40 | outside `eval-harness` only `max_turns` is enforced, by each driver (`drivers.rs:341-348`, `:772-784`, `cursor.rs:317-323`); the app passes `max_turns: None` (`chat_ui.rs:433`) and runs no loop |
| 6 | An `Investigating` incident is returned by **all six** of `open()`, `open_count()`, `incident_panel_rows`, the Media panel's filter, `observe`'s dedup and `refresh_revision` | §3.6 | items 2–3 | the variant does not exist; probe-2 T9 built it and measured **1** compile error (`incident_ui.rs:402`) and **5** silently changed comparisons (`incident.rs:2451`, `:2490`, `:2580`, `incident_ui.rs:539`, `media_bin.rs:463`) |
| 7 | The six `AgentEvent::Cost` mirrors, `turns` and `resolver` are all `Some` on **≥ 1** session-resolved incident; their token totals **equal** the session's `SessionCounters`; and all eight are absent from a router-resolved one | §5.4 | items 11, 23, 40 | `mirror_agent_cost` has **zero** production callers (`server.rs:16352`; only `:26745`, `:26767`, `:26782`) and **overwrites** rather than accumulates, `turns` and `resolver` do not exist, and IN1 §8 rule 6 asserts every mirror is `None` |
| 8 | **≥ 4** budget, off-switch or policy-violation stops each return their incident to `Open` with **0** entries added to `suppressed`, and a later observation of the same `(code, subject)` dedups into the same incident rather than being suppressed | §3.7 rule 38, §4.5 rule 22 | items 4–5, 40 | `IncidentLog::resolve` inserts into `suppressed` for **every** outcome (`incident.rs:2531-2539`, IN1 §2.3 rule 19) and there is no `end_investigation`; today every `Rejected` would silence its pair |
| 9 | `propose_fix` refuses **6** named codes, records the branch's own `Query::AppliedOperations` as the proposal, and refuses a destructive branch with **nothing** recorded on the incident | §4.1, §6 rule 3 | items 24–26 | the capability does not exist; `INSPECTOR_TOOL_NAMES.len() == 86` and the registry is 140 (`tests/mcp_server.rs:10395-10397`) |
| 10 | An investigator server refuses **9** named capabilities through `invoke_capability`, including `resolve_incident`, `cancel_export`, `cancel_analysis` and `request_analysis`; a server with an empty denylist accepts the same nine; and every inventory row marked **yes** is on the list | §6 rules 2, 4 | item 27 | `KinewrightMcp` has no denylist and `is_invocable_capability` is a free function with no session context (`runtime.rs:194-206`), so all nine succeed today |
| 11 | The settings file round-trips, defaults `enabled` to `false`, resolves to **3** distinct per-platform paths with `durable` false in exactly the fallback case, and an unreadable file opens **exactly 1** `agent_unclassified` incident without overwriting it | §2.1 | items 37–39 | nothing persists: eframe's `persistence` feature is off (`ron` and `home` are absent from `Cargo.lock`), `settings_ui.rs:28-36` writes only in-process egui memory, and `default_recovery_directory` reads **one** variable (`recovery.rs:774-779`) |
| 12 | `confirmation_description` returns `Some` for **all seven** destructive variants and `None` for an empty `RemoveTrack`; `plan_confirmation_description` emits **both** pinned sentences verbatim; and all seven variants' tools advertise `destructiveHint: true`, for **15** in all | §6 rules 5–6 | items 28–30 | both functions match **3** variants and nothing else (`server.rs:1246-1268`, `:16749-16787`), so four of the seven pass ungated, and `schema.rs:361-366` names **4** generated tools |
| 13 | `Document.investigator` round-trips **≥ 1** muted code, ignores **≥ 1** unrecognised one, and `pre_m13_project.json` still round-trips to **1 215 B** / `c9da3186e131e4fd`, with **neither** the served quad nor the registry sextuple moved by the field | §2.4 | item 12 | `Document` has ten top-level fields and none of them is `investigator` (`crates/kinewright-core/src/model.rs:1088-1121`) |
| 14 | A confirmation raised inside a session ends it with `SessionStop::PolicyViolation` within one tick, with `request.incident == Some(id)`; the same request under `ApproveAll` is approved and the session continues | §4.6 | items 17–18, 31 | `ConfirmationRequest` is three fields (`server.rs:139-144`) and carries no incident, and no pump exists to answer it either way |
| 15 | `Core::spawn_at` reports the revision it was given, `Core::spawn` still reports `TimelineRevision::default()`, and a branch spawned by it reports an **empty** applied-operation list before its first edit | §3.3 rules 15–16, §4.1 rule 4 | items 13–14 | `CoreState::new` hard-codes `TimelineRevision::default()` for every core (`actor.rs:248-256`) |
| 16 | `crates/kinewright-agent/src` contains exactly **one** agent-event loop and it is in `session.rs`; `crates/kinewright-app/src` contains **zero**, exactly **one** `pump_session(` call and **zero** new `send_user_message(` calls; each asserted by that crate reading its own sources, CRLF-normalised, over a positive count of allowlisted non-session sites | §3.5 rule 30 | items 35–36 | the only agent-event loop is `collect_session`'s, behind `eval-harness` (`eval.rs:3843-4010`), which `cargo check -p kinewright-app` does not compile; workspace-wide there are 56 `recv_timeout` sites across 12 files, of which the app already holds several |
| 17 | `ScriptedDriver` makes **≥ 4** real tool calls through its own MCP client from a two-turn script, visible from both `crates/kinewright-agent/tests/mcp_server.rs` and the app's tests under `default-features = false`, with **0** new dependencies in `Cargo.lock` | §8 | items 21–23 | the only fake is `#[cfg(test)]`-private inside `eval.rs`'s test module, makes no tool calls and is one-shot (`eval.rs:9243-9299`) |
| 18 | A conflicting approval retries **exactly once**, a twice-conflicting one leaves the incident `Open` with `stale == true` and a **Re-investigate** action, and an `Err(BranchError)` does the same with the message on telemetry | §4.4 rules 16–19 | items 40 | `apply_to_live` is private and retries nothing on `Conflict` (`branch.rs:247-271`); no approval path exists |
| 19 | The served quad is `7 / 5 660 / 3 510 / 998` in **all three** value sites with the counter reading **eighteenth** in **all five** word sites; the registry counts are `141 / 54 / 87` across **all six** pin sites; `IN1_INCIDENT_SERIALIZED_BYTES` is still `assert_eq!` **819**; and the re-measured ceiling holds over **4 288** shapes with the `> CEILING / 2` divisor asserted | §6.4, §4.2 rule 11 | items 32–34 | **partly by a word, and the clause says so**: the quad's values already hold, and what fails is the five sites reading "seventeenth" (`tests/mcp_server.rs:2427`, `:10346`, `:10366`, `:10421`, `server.rs:26806`), the registry assertions reading `140`/`86` (`tests/mcp_server.rs:2461`, `:2501`, `:10395`, `:10397`, `server.rs:22381`, `:26185`) and the ceiling at 2 048, which probe-2 measured a real shape at 2 183 against |

**Regressions guarded and process gates.** None is a clause: each passes at `7e85972` by construction, or is not a `cargo test`.

- **R-A — IN1's and IN1b's full suites stay green**, with these named exceptions, each rewritten and each forced by a rule this contract mandates: `in1b_the_served_quad_does_not_move_for_the_seventeenth_measurement`, `cc7_the_agent_surface_is_unchanged_by_this_slice` and `served_surface_is_small_and_keeps_the_internal_registry_discoverable` (the counter word and the registry counts, §6.4); `in1b_every_code_fits_the_measured_ceiling` and its core twin (§4.2 rule 11); `in1b_the_incidents_panel_lists_every_open_subject` (`crates/kinewright-app/src/app.rs:5795`, which gains the `Investigating` row); and IN1 §8 rule 6's telemetry test, which asserts the six mirrors are `None` and must now say *"on a router-resolved incident"*. Every other `in1_` and `in1b_` test passes **unmodified**, including `in1_the_fixture_incident_serialises_to_the_pinned_wire_body` (`incident.rs:1391`) and the whole 17-test per-label suite. Probe-2 ran each of these suites against its own prototypes and reports zero failures beyond the listed rewrites.
- **R-B — the eval harness is unchanged in behaviour.** `EvalBudgets`' seven fields keep their meanings and `SessionMetrics` is unmodified; `cargo test -p kinewright-agent --features eval-harness` stays green across §3.5's refactor, as it did under probe-2 T6.
- **R-C — the 750 B template and both managed-colour allowlists do not move** (IN1 §9 regressions R2, R-B, R-E). Part A touches no colour literal.
- **R-D — the chat panel's confirmation path is unchanged.** Its broker, its drain (`chat_ui.rs:863-876`), its render (`chat_ui.rs:1567-1602`), its 60 s deadline and its request shape all behave exactly as they do today; the one added field is `None` for every chat request, and the 97 test constructions of `ConfirmationBroker` do not move.
- **R-E — `incident_card`'s signature and its 18 call sites (§4.3 rule 13) are unmodified**, asserted by their continued compilation and by IN1b §7 items 25–27 staying green.
- **P1 — the build gates.** `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and **`cargo check -p kinewright-app`** before **each** commit. Every cargo command runs after `source ./scripts/setup-ffmpeg.sh`. While two implementers are live, `cargo fmt` is run **per crate** (`cargo fmt -p <crate> -- --check`), with the workspace `--check` once before the commit (IN1b erratum D-R63).
- **P2 — two review passes per crate** before each commit, per the slice recipe. One of the two app passes **must** be briefed to read §4.3's card and §5.5's counters as a person with no NLE expertise would.
- **P3 — the Windows CI lane is part of the gate.** Every source-reading test normalises line endings before matching, and no test derives a temporary directory name from the clock (the two IN1b lessons, `33f5b44` and `7e85972`).
- **P4 — the promotion note records §10 figure 2 and the three registry byte figures** once the implementer has them, so the contract in `docs/` carries numbers and not markers.

---

## 10. Measurement limits, and the one figure still open

**Limits Part A records rather than papers over.**

1. **The card's placement is still not proven at all.** There is no `crates/kinewright-app/tests/`, `KinewrightApp::new` is private, and both `show_incident_card` (`incident_ui.rs:552`) and `show_incidents_panel` (`:600`) are untested by design. `proposal_card` is proven as a pure function and its placement is not proven either. IN1 §11.1 limit 2 and IN1b §10 limit 1 are unchanged and now cover a third surface.
2. **No session in this contract's tests runs a model.** Every assertion is against `ScriptedDriver`. What the three real harnesses do with the opening message, with a six-tool allowlist and with a turn cap is measured by the implementer for figure 2 on the machines that have them and is **not** asserted by any clause. §11's hand-run checklist is the only place a real harness appears.
3. **The chat agent's `resolve_incident` exposure is unchanged, and is recorded rather than fixed.** `verify_claimed_outcome` verifies an `Applied` colour claim against the document it was handed (`server.rs:16512-16532`, `:16543-16590`), which for the chat panel's branch-backed server is the **branch's**, while the log it writes is the project's live one (`chat_ui.rs:245-251`). Part A removes the capability from the investigator (§6 rule 4) and leaves the chat agent's route exactly as it is at `7e85972`. Making the verifier read the live document is §13 D-A6.
4. **"Turn" means two different things across the three production drivers and Part A asserts only the pump's.** §5.1 rule 2 names the divergence; no clause quantifies over Claude's mid-send assistant-message counter or over the child kill it performs.
5. **The per-session server cost is measured, and its baseline is the app's existing population.** Probe-2: on a 20-CPU machine, baseline RSS 162 872 kB / 11 threads; one server +26 064 kB / +2 threads (the process already held workers); each further server **+22 OS threads**, +1 ephemeral listener and +0.4–1.1 MB RSS. The 22 is `new_multi_thread`'s one-worker-per-CPU (20) plus the driver thread plus the branch `Core` actor. **The scaling term is threads and it is proportional to the CPU count**, which is why §3.3 rule 13 bounds the investigator's runtime at **2 workers** instead of raising the cap. The cap of 2 bounds the investigator's share only: every chat thread already starts a server of its own (`chat_ui.rs:245-252`, `:299-306`) and that population is unbounded today (critic S14).
6. **The settings file is read once and has no watcher.** Two Kinewright processes writing it is last-write-wins, and Part A does not defend against it (§2.1 rule 6).
7. **`operation_internal` is on the allowlist and its own `Explain` body says nothing the caller changes will fix it.** It is the lowest-yield member of the 54 and it is named here rather than quietly excluded; it is the first row IN3 removes if production telemetry's `resolver`/`stop` distribution shows it spending a model for nothing.
8. **No Part A number is per-OS.** If one is ever found that differs it becomes a doc-comment note beside the single constant, never a second constant.
9. **`IncidentTelemetry` stops being `Copy` at zero cost.** Probe-2 measured the fallout as **0** compile errors over the **24** workspace lines that name `.telemetry` or `IncidentTelemetry`; §5.4 rule 12's instruction to re-read every site is a real instruction with an empty result.
10. **`spawn_at` aligns the branch with live at session *start*, not with the incident's own revision.** A session queued behind another (§3.2 rule 10), or started after the person edited, begins on a branch whose base is newer than `incident.revision`. Nothing breaks — `get_incidents`' gate is optional and the merge is not gated on the branch counter — but the proposal is proved against a document the incident was not observed against, and §4.4's single retry is the only thing that notices (critic S12).
11. **Seven codes get neither a button nor a session in Part A**, and they are the only ones: the seven `unsupported_source_*` rows (§3.1 rule 2 reason 2, §7 rule 5). D-A2 owns them. Every other code either has a button (6) or starts a session (54). 6 + 54 + 7 = 67.

**The one open figure.**

| # | figure | who measures it | why it cannot be argued |
| ---: | --- | --- | --- |
| 2 | the three budget defaults, **discharged at stage C** — `6 / 90 s / 40 000`, written as `InvestigatorBudgets`' serde defaults in `crates/kinewright-app/src/investigator.rs` and exercised by §9.1 item 40's scripted sessions under those defaults (S4 stops at the turn bound, S5 at the wall bound, S6 at `max_tokens` + one turn; §9.2 clause 5). What a scripted session cannot measure is a real harness's token spend against the 40 000, so that half of the figure moves to §11's hand-run checklist, where a Codex session is expected at ~5–7 k tokens of re-sent prompt over six turns (§3.4 rule 25); the constants change only if that run says so | the implementer, after §8 lands | probe-2 could not: figure 2 asks for a six-turn `ScriptedDriver` session, `ScriptedDriver` is §8's own deliverable, probe-1 P6 established that no existing fake makes tool calls, and no harness was authenticated in that environment — **nothing in the tree at `7e85972` can run a session at all**. Provisional **6 / 90 s / 40 000**, chosen knowing Codex's measured **3 418–4 973 B per turn** (§3.4 rule 25). No clause asserts any of the three (§5.3 rule 10) |

   The other six figures revision 1 marked `[probe-2]` are discharged and live in the sections that own them: figure 1 the opening message (§3.4 rule 24), figure 3 `IN1_INCIDENT_SERIALIZED_BYTES` (§4.2 rule 11), figure 4 the ceiling (§4.2 rule 11), figure 5 the server cost (limit 5 above), figure 6 the registry (§6.4), figure 7 the buildable rows and the `Copy` fallout (§7 rule 6, limit 9 above). The three registry **byte** figures carry their decomposition rather than a bare number, for the reason §0.4 n gives.

---

## 11. The hand-run checklist

Nothing below is a `cargo test`. §10 limit 1 means no automated test sees a card on screen and §10 limit 2 means no automated test runs a model, so this is the only evidence for both.

1. **The one sentence, end to end.** With a harness installed and authenticated, open Settings, switch the investigator on, pick the harness and model, and leave the budgets at their defaults. Provoke an `Explain` incident with no deterministic recovery — the cheapest is a refused trim past a clip's source end, which opens `operation_bounds`. Confirm: the badge increments; the Incidents panel shows the card reading **Investigating** with three counters that move; within the wall-time budget a proposal card appears with a sentence and a list of the edits; **Approve** applies them; the timeline shows the edit; the card reads **Applied**; and **Ctrl+Z** takes it back as one undo step.
2. **The off switch, seen, and nothing silenced.** Repeat item 1 to the point where the card reads *Investigating*, then switch the investigator off in Settings. Confirm the card leaves *Investigating* within a frame, reads **Open** with *"investigation stopped: the investigator was switched off"*, the badge does **not** fall, and provoking the same failure again still opens and dedups into the same incident rather than being silently swallowed. This is the regression §3.7 rule 38 exists to prevent and it is the one item on this list a reviewer should refuse to sign without seeing.
3. **The button, with no harness at all.** Rename every harness executable off `PATH`, restart, and confirm: the Settings section says none is detected and the toggle cannot be switched on; an `unknown_source_range` incident on a Rec.709-shaped source with an unknown range still shows **one enabled button** on its card; and pressing it applies the same operation the investigator would have proposed (§7 rule 7).
4. **The mute, seen.** Press **Never investigate this** on an `operations_unclassified` card, save the project, reopen it, provoke the same failure, and confirm no session starts and the card still explains.
5. **The race, seen.** Provoke an incident, wait for the proposal card, make a hand edit on the timeline, then press **Approve**: confirm it still lands (the single retry). Repeat with **two** hand edits and confirm the card goes stale with **Re-investigate** offered instead of **Approve**.
6. **Nothing destructive, seen.** With a session running, watch the chat panel: confirm no `AGENT CONFIRMATION REQUIRED` frame appears there at any point (`chat_ui.rs:1567-1602`), that the timeline still holds every clip, bin, string-out, sync group and bus it held before, and that a running export and a running analysis both survive the session untouched (§6 rule 2's three new denials).
7. **Two projects.** Open two projects, provoke an incident in each, and confirm the second's session starts (the app-wide cap is 2), each card counts its own budget, and neither project's proposal appears on the other's card.
8. **The settings file, on this platform.** Confirm the file lands at the path §2.1 rule 1's table names for this OS, that hand-corrupting it opens exactly one incident and leaves the file alone, and that fixing it by hand and restarting restores the settings.

---

## 12. Line budget, and what is cut first

**Calibration, re-derived, because IN1's +28 % is known to be too small.** IN1 Part A estimated 4 039–5 279 and landed **6 758** insertions across 70 files (`git diff --shortstat e3ad623..29fad67`) — **+28 %** over its upper bound. IN1b estimated 4 392–6 992, carried that +28 % forward, and landed **10 100 insertions / 930 deletions across 41 files** (`git diff --shortstat 369998b..7e85972`) — **+44.5 %** over its raw upper bound and still **+13 %** over its calibrated one. Per crate, re-derived: core **4 296**, app **4 158**, agent **1 127**, media **338**, `docs/` **162**, `CHANGELOG.md` **16**, `.gitattributes` **3** — the docs figure is `docs/` alone and is labelled here because 178 is `docs/` plus `CHANGELOG.md`. **This budget carries +45 % and says so here rather than discovering it.** Rows marked **‡** are new in revision 2; rows whose estimate moved carry probe-2's measured production diff beside them.

| deliverable | crate | est. |
| --- | --- | ---: |
| `Core::spawn_at`, `CoreState::new(revision)`, `TimelineBranch::new_at` (§3.3) — probe-2 measured **24 (+) / 5 (−)** for `spawn_at` alone | core + agent | 40–70 |
| `IncidentState::Investigating`, `is_open()`, the five decided sites, `begin_investigation`, `end_investigation`, `state_label`'s two arms (§3.6) — probe-2 measured **21 (+) / 8 (−)** for the variant and the predicate | core + app | 120–200 |
| `IncidentOutcome::Rejected` and the **two** compile sites it costs (§4.5) — probe-2 measured both | core + app | 30–60 |
| `IncidentProposal` with `Eq`, `Incident.proposal`, `record_proposal`, the `#[serde(skip)]`, the two serialised-length caps (§4.2) | core | 150–230 |
| `INVESTIGATOR_ALLOWLIST`'s **54** rows and the derivation test (§3.1) | core | 100–150 |
| `deterministic_recovery`'s sixteen arms and the widened `policy_recovery` arm (§7) | core | 180–280 |
| `Document.investigator` / `InvestigatorPreferences` **plus the 52 mechanical struct-literal edits** (§2.4) — probe-2 measured the whole diff at **69 (+) / 1 (−)** over 32 files | core + workspace | 90–140 |
| `IncidentTelemetry.turns`, `resolver`, `IncidentResolver`, and the `Copy` → `Clone` fallout, measured at **0** sites (§5.4) | core + app + agent | 70–120 |
| the session pump `session.rs` and `collect_session` rewritten onto it (§3.5) — probe-2 measured **411 (+) / 134 (−)** over 4 files, `session.rs` at **198** lines | agent | 380–480 |
| ‡ the session thread, `SharedCounters`' five atomics, the cancellation flag and the app-side join (§3.5 rule 27, §3.2 rule 8) | agent + app | 60–110 |
| ‡ `SessionStop`'s five variants, `BudgetKind`, `StopReason`, `ConfirmationPolicy` and the match arms that follow them (§3.5 rule 28) | agent + app | 40–80 |
| `ScriptedDriver` / `ScriptedSession` with its own MCP client and runtime (§8) — probe-2 measured the client at **70** non-comment lines, **0** new dependencies | agent | 250–380 |
| `propose_fix`: two args, the applied-operations snapshot, six refusals, registration, description (§4.1) | agent | 200–300 |
| the **9**-name capability denylist, the eighth constructor with its 2-worker runtime, `ConfirmationRequest.incident`, `for_incident` (§3.3, §4.6, §6 rule 2) | agent | 130–200 |
| ‡ the twelve-row side-effect inventory as a doc comment and its two-directional test (§6 rule 2) | agent | 60–100 |
| the broker gate widened to seven in both description functions, `is_destructive_operation`, and the three `destructive(true)` flips (§6 rules 3, 5) — probe-2 measured **~54 (+) / 4 (−)** production | agent | 90–150 |
| `apply_to_live` made `pub` and re-exported, with its `Result` handled (§4.4) | agent | 10–20 |
| ‡ the pins re-measured and re-worded across the **six** registry sites, the three quad value sites and the five counter-word sites, and the ceiling loop regrown over 4 288 shapes (§6.4, §4.2 rule 11) | agent | 140–220 |
| `InvestigatorSettings`, the per-user file, T10's path resolution with its durability flag, the atomic write, the Settings section, the discovery refresh, the off switch (§2) | app | 320–480 |
| `InvestigatorSession`, the queue, the dedup, the cap, start / stop / cancel (§3.2, §3.7) | app | 440–680 |
| the opening-message composer and `QueuedIncident.refused` carried from both drains (§3.4) | app | 120–200 |
| `proposal_card`, `ProposalCardView`, its three actions and its rendering beside `incident_card` (§4.3) | app | 300–450 |
| apply-on-approve with the single retry, the end table, the `Err(BranchError)` arm, `stale`, **Re-investigate** (§4.4) | app | 190–290 |
| the three `Investigating` counters and the `stopped` row in `details`, the `state_label` wiring, the **Never investigate this** action (§2.4, §3.7, §5.5) | app | 100–160 |
| **the policy boundary enforced and the telemetry written** — the guard on the session's core handle, the first accumulating `mirror_agent_cost` call (§5.4, §6 rules 8–10) | app | 90–140 |
| the **51** test functions of §9.1's forty items | all | 980–1 450 |
| | **total** | **4 680 – 7 140** |

**With +45 %, plan Part A at ≈ 6 790 – 10 350 insertions.** The arithmetic, stated so the rounding is not a mystery: 4 680 × 1.45 = 6 786, rounded **up** to 6 790; 7 140 × 1.45 = 10 353, rounded **down** to 10 350. Plus one promotion commit for the contract itself (IN1b's docs row measured **162**, and this contract is longer).

**Cut order, Part A.** Each names the clause or hand-run item it changes, and **none is in the never-cut set**. Revision 1's preamble claimed no clause rested on any of them, and its own second item then amended clause 17.

1. **The `Never investigate this` card action and the Settings mute list** (§2.4 rule 14, ~90 of the 100–160 row). Cut **first**. Clause 13 still passes with a mute written by a test rather than by a button; **§11 item 4 loses its only evidence** and is struck with it; a person can still turn the whole feature off in Settings.
2. **Re-investigate** (§3.7 rule 39, §4.4 rule 19, ~70 of the 190–290 row). Cut **second**. A stale proposal then renders with no `Approve` and the person's route is to provoke the failure again. **Clause 18's wording drops the action** and reads *"leaves the incident `Open` with `stale == true`"*; item 40's `in2_the_proposal_card_offers_…` asserts two actions on a fresh proposal and one on a stale one.
3. **The three counters on the `Investigating` card** (§5.5 rule 14, ~60). Cut **third**. `"Investigating"` alone tells the person a session is running, and the numbers are on the incident's telemetry either way. **Clause 6 stands unchanged; §11 item 1 loses one observation.**
4. **`ScriptedDriver::detect()`'s configurable `AuthenticationStatus`** (§8 rule 6, ~30). Cut **fourth**. §2.2 rule 7's `Unknown` rule is then proved by a unit test over the settings predicate rather than end to end. **Item 22 goes with it, and clause 17 drops nothing, because it names items 21–23 and item 22 is not the one that makes the tool calls.** This is the cheapest thing on the list and the last one worth cutting.

**Never cut**, because they are the exit gate: the 54-code allowlist; the headless start against the live project with `spawn_at`; `Investigating` with `is_open()` and its five decided sites; **`end_investigation` and the non-suppressing ends**; `propose_fix` reading the branch's own operations, with its six refusals and its destructive check; the app-only end table; the nine-name capability denylist including `resolve_incident`, `cancel_export`, `cancel_analysis` and `request_analysis`; the broker gate over all seven variants with both pinned sentences intact; the non-feature-gated pump with one owner, its thread and its cancellation flag; `ScriptedDriver` itself and S1–S11; the three deterministic rows; the served-quad pin in all three value sites and the six registry pin sites; `cargo check -p kinewright-app`; both CI lanes.

---

## 13. Explicit deferrals

Each names why it is a slice and not a flag, and carries a cost and an owner. Ids in the `D-A…` namespace belong to Part A's own deferrals; the six rows Part B inherits keep the ids their parent contracts gave them (N1/Q14).

**Part B — `docs/IN2B-INCIDENT-PERSISTENCE.md`. All six, with D-B2(a) the first cut** (N1/Q3, N1/Q14).

- **IN1 §13 D2 — persisting incidents, and a cross-process timestamp.** The log is session state; `opened_at` is a `Duration` from a private `Instant` and is `#[serde(skip)]` (`crates/kinewright-core/src/incident.rs:1170-1171`); the project file is the serialised `Document` and nothing else (`crates/kinewright-app/src/project.rs:190`), so this is a sidecar plus a stamp that is neither `Instant` nor `SystemTime`. **Part A chooses the shape Part B must deserialize**: `IncidentState` derives `Serialize` only (`incident.rs:1087-1088`) and now has three variants, one of which is `Investigating`. **And Part A hands Part B one rule it must not get wrong** (critic S1): `IncidentProposal.operations` is `#[serde(skip)]`, so a loaded proposal deserialises to an empty vector while `operation_count` says three — **Part B sets `stale = true` on every proposal it loads**, the card offers **Re-investigate** and not **Approve**, and no reload can record an `Applied` that applied nothing (§4.2 rule 10). **Cost:** 400–650. **Owner: IN2 Part B.**
- **IN1 §13 D1 — the older-reader provenance downgrade, with persistence.** A project-format version gate, riding on D2's file. Part A adds a second thing an older reader silently drops, `Document.investigator` (§2.4 rule 13), so the gate now covers two fields. **Cost:** 80–150. **Owner: IN2 Part B.**
- **IN1b §13 D-B2 — the app's panel-local untyped sinks, three groups.** Re-located by probe-1 P9 at `7e85972`: (a) nine `Unavailable(String)`-shaped labels — `color_scopes_ui.rs:810`, `color_qc_ui.rs:1199-1202`, `preview_ui.rs:1139`, `:921`, `transcript_ui.rs:162-166`, `chat_ui.rs:1241`, `export_ui.rs:1331-1341`, `:569`, `:953`; (b) two `recovery.rs` modals at `:699`/`:703` and `:639`/`:666`/`:675`; (c) the LUT-store tooltips, now **8** consumers rather than 4. **Cost:** (a) 300–500, (b) 80–150, (c) 40–80. **Owner: IN2 Part B**, with **(a) the first cut**.
- **IN1b §13 D-B3 — the `Result<_, String>` seams.** D-B3 counts four; probe-1 P9 counts **six** method signatures. They flatten **21** typed error values and keep **11** of the 67 declared codes unreachable. **Cost:** 250–450. **Owner: IN2 Part B.** Part B's exit gate must assert the session count over the newly reachable population, because those eleven codes join `INVESTIGATOR_ALLOWLIST` only by an explicit edit (§3.1 rule 1).
- **IN1b §13 D-B4 — the vestigial `media backend error: ` prefix.** D-B4 counts five asserting sites; probe-1 P9 counts **12**, including **one production consumer** that still parses it (`export_queue.rs:451`'s `strip_prefix`). **Cost:** 90–160, revised up from D-B4's 40–80. **Owner: IN2 Part B.**
- **IN1b §13 D-B6 — a per-subject asset, clip or look name on the incident.** IN1 §13 D15 widened by the seven further subjects. It needs the router to read the document at observe time, which is the coupling IN1 §2.3c rule 31 exists to avoid. **Cost:** 80–150. **Owner: IN2 Part B.** It matters more after Part A than before it, because a proposal card that says *"Clip 4"* rather than the clip's name is the card a person has to approve.

**IN3.**

- **D-A1 — the eleven `AskFirst` candidates** (N1/Q4). Probe-1 P8's group (B), listed in full at §3.1 rule 6. Each names an operation whose **argument** — a file, a location, a transcode target — only a person can supply. **Not free:** declaring an `AskFirst` row without shipping its recovery produces a class whose card has nothing to press, which IN1 §9 clause 6 forbids, so the rows and `RecoveryKind::Relink`/`Transcode` (IN1 §13 D7) land together. **Cost:** 400–700 with the recoveries. **Owner: IN3.** All eleven are on Part A's allowlist meanwhile, so they are investigated rather than ignored.
- **D-A2 — the thirteen rows §7 measures unbuildable.** The seven `unsupported_source_*` rows are IN1 §13 D8's item — a target-profile vocabulary, a proof and somewhere to put the output — and cannot be served by `assume_rec709_operation` for the reason §7 rule 5 measures. The six delivery and clamp rows need either export-dialog state to become document state (rows 11–14) or a capability-argument recovery kind (rows 15–16), and in every case they need typed evidence the incident does not carry: all six arrive as `IncidentEvidence::MediaError { code, message }` and `deterministic_recovery` never sees `observed`/`allowed` at all (§7 rule 6). **The seven colour rows are the only codes in the 67 with neither a button nor a session** (§10 limit 11); the six delivery and clamp rows are on the allowlist. **Cost:** 500–900. **Owner: IN3**, with D8's per-code bodies.
- **D-A3 — a general per-session server-side tool surface**, IN1 §13 D11. §6 rule 2's denylist is one `&'static [&'static str]` field on one struct, consulted at one call site; task-scoped capability packs are a mechanism. **Cost:** unchanged from IN1 §13 D11. **Owner: IN3.**
- **D-A4 — the agent crate's 277 refusal sites**, IN1b §13 D-B1, and **D-B5**, the three `reason: String` `OpError` variants (`crates/kinewright-core/src/operation.rs:924`, `:979`, `:982`) with the seven `Result<_, String>` app helpers. Both are unchanged and both are **IN3's**; Part A touches neither and does not pre-build for either.
- **D-A5 — per-harness prompt and cost accounting.** Codex re-prefixes the 1 840 B system prompt into **every** user turn with a growing recap (`drivers.rs:424-437`), while Claude sends it once (`drivers.rs:189-193`) and Cursor only on turn 0 (`cursor.rs:344-347`); and Cursor constructs **no** `AgentEvent::Cost` at all. Probe-2 measured the consequence: a Codex session pays **3 418–4 973 B per turn**, i.e. **20.6–29.8 kB over six turns**, against Claude Code's one-off **9 086–10 641 B**. Part A's answer is one number per install with a per-harness "unknown" (§5.3 rules 9–10); **per-harness budgets and the driver work that would make Cursor report tokens are IN1 §13 D10's, owned by IN3, and the efficiency half is IN4's gate.** **Cost:** 200–400 for per-harness budgets, plus unmeasured driver work for Cursor.
- **D-A6 — `verify_claimed_outcome` against the live document.** §10 limit 3. **Cost:** 80–150 plus a decision about which document a branch-backed server is allowed to verify against. **Owner: IN3**, with the investigator surface.
- **D-A7 — `default_recovery_directory`'s own per-platform defect.** It reads one Windows-only variable and falls to `std::env::temp_dir()` everywhere else (`crates/kinewright-app/src/recovery.rs:774-779`), so on Linux a person's crash-recovery journals live in `/tmp` and do not survive a reboot — measured at `/tmp/Kinewright/recovery` (probe-2 T10). Part A writes the correct three-branch resolution for its own file (§2.1 rule 1) and **deliberately does not change `recovery.rs`**, because the recovery journal's directory is load-bearing for a feature with its own tests and its own scan-order rules (`recovery.rs:781`'s `scan_directory`), and changing where a shipped build looks for journals is a migration, not a one-line fix. Part A names it rather than leaving the asymmetry unexplained. **Cost:** 60–120 plus a read-both-locations migration window. **Owner: IN3**, or the first slice that touches crash recovery.

**Unowned, and re-deferred rather than cut.**

- **IN1b §13 D14 — multi-project incident attribution.** `media_events` is a single app-level receiver from one engine, so `route_incidents` attributes every playback refusal to the focused project (`app.rs:1155-1167`). Part A's "one session per project" is structural until this lands (§3.2 rule 11) and **nothing in Part A pre-builds for it**. **Owner: the slice that makes the engine per-project, or that tags `MediaEvent`.**
- **IN1 §13 D12 — a live `KinewrightApp` harness**, and **IN1b §13 D-B7 — `IncidentSeverity::Informs`**, both unchanged. **Owners:** the programme roadmap, and IN3.

---

## 14. Files

**Three implementers, not four** (critic S9, §0.4 m). Revision 1's implementer B was given no file to edit and then given 20 of implementer A's edits in its crate; `kinewright-media` belongs to **A**, who owns the `Document` field those edits follow from. The R20–R39 erratum range stays reserved for that crate.

| Implementer | Crate | Files, exhaustively |
| --- | --- | --- |
| **A** | core **and media** | `src/actor.rs` (`Core::spawn_at`, `CoreState::new(revision)`, two tests); `src/incident.rs` (`IncidentState::Investigating` and `is_open()`, `IncidentOutcome::Rejected`, `IncidentProposal` with `Eq` and its `#[serde(skip)]`, `Incident.proposal`, `IncidentLog::begin_investigation`, `end_investigation` and `record_proposal`, `IncidentTelemetry.turns`/`resolver` and `IncidentResolver`, `INVESTIGATOR_ALLOWLIST`'s 54 rows, `deterministic_recovery` and `policy_recovery`'s widened `Explain` arm, `InvestigatorPreferences`, the twelve core tests); `src/model.rs` (**one** `Document` field and one `Default` arm); `src/lib.rs` (the `pub use incident::{…}` names). `src/color.rs`, `src/delivery.rs`, `src/color_qc.rs` and `src/operation.rs` are **read, not edited** — `rec709_compatible` and every `code()`/`recovery_action()` accessor are unchanged (§9 regression R-C). In `kinewright-media`, **no file is edited except by the mechanical pass**; `src/in1_fixtures.rs` is **read** and its assertions stay green. **Plus the 52 mechanical `investigator: None` struct-literal edits wherever they fall** (media 20, agent 14, core 12, app 6; 32 files, 69 (+) / 1 (−) as probe-2 T11 measured them), which land **last** (see Order). |
| **C** | app | `src/settings_ui.rs` (the INVESTIGATOR section, the off switch, the discovery refresh, the mute list, the non-durable-path notice); **`src/investigator.rs` (new)** (`InvestigatorSettings`, the per-user file and T10's path resolution with its durability flag and atomic write, `InvestigatorSession`, `QueuedIncident`, the queue, the cap, the session **thread** with its `Arc<SharedCounters>` and `Arc<AtomicBool>`, start/stop/cancel, the opening-message composer, apply-on-approve with its single retry and its `Err(BranchError)` arm); `src/app.rs` (`route_incidents`' enqueue and its end-drain, the `Event::OpRejected` arm at `:1369` carrying `op` into the queue, the outcome match at `:183`, the `Investigating` badge path, `open_incident_count`, the tests at `:5795` and `:5930`); `src/incident_ui.rs` (`state_label`'s two new arms and its doc comment at `:399`, `incident_panel_rows`' `is_open()`, `proposal_card` and `ProposalCardView`, the three counters and the `stopped` row in `card_details`, the **Never investigate this** and **Re-investigate** actions, the panel's proposal rendering); `src/media_bin.rs` (the `:459-464` filter's third state); `src/project.rs` (`investigator: Option<InvestigatorSession>` on `ProjectSession`); `src/chat_ui.rs` (the two `BranchApplyOutcome::Rejected` arms at `:665-671` and `:770-776` carrying `operations` into the queue). `src/error_ui.rs` and `src/recovery.rs` are **read, not edited**: `note_incident` (`:89`) and `note_label` (`:138`) are called unchanged, and `default_recovery_directory` is §13 D-A7's. |
| **D** | agent | **`src/session.rs` (new)** (`SessionLimits`, `BudgetKind`, `StopReason`, `SessionStop`, `ConfirmationPolicy`, `SessionCounters`, `SharedCounters`, `SessionObserver`, `SessionFlow`, `pump_session`); **`src/scripted.rs` (new)** (`ScriptedDriver`, `ScriptedSession`, `ScriptedTurn`, `ScriptedCost`'s six fields, its `rmcp` streamable-HTTP client and current-thread runtime); `src/server.rs` (`propose_fix`'s two args, its applied-operations snapshot, its handler and six refusals, `ConfirmationRequest.incident` and `ConfirmationBroker::for_incident` with its 5 s timeout, `KinewrightMcp.capability_denylist` with the twelve-row inventory doc comment and the check at `:777-782`, `start_investigator_session` with its 2-worker runtime, the seven-variant `confirmation_description` at `:1246-1268` and `plan_confirmation_description` at `:16749-16787`, the two size constants at `:16406`/`:16437`, the ceiling loop at `:26256-26330`, the quad pin at `:26806-26860`); `src/runtime.rs` (`INVESTIGATOR_TOOL_NAMES`, `is_destructive_operation` and `EditPlanPreview`'s filter reading it); `src/schema.rs` (`INSPECTOR_TOOL_NAMES` 86 → 87 with one name, and three names added to `:361-366`'s `destructive(…)`); `src/branch.rs` (`apply_to_live` made `pub`, `TimelineBranch::new_at`); `src/eval.rs` (`collect_session` rewritten onto `pump_session` with an eval `SessionObserver`; **nothing else**); `src/lib.rs` (the two new modules and the `pub use` names); `tests/mcp_server.rs` (the pins below, and the new `in2_` tests). `src/drivers.rs`, `src/cursor.rs` and `src/protocol.rs` are **read, not edited** — the three production drivers are unchanged (§1 non-delivery 8). |
| orchestrator | docs | the six edits at promotion, below. |

**The pin sites implementer D must edit, named** (probe-2 T4; revision 1 named three of them and missed the test that fails first).

| # | site | pin |
| ---: | --- | --- |
| 1 | `crates/kinewright-agent/tests/mcp_server.rs:2461` | `registry.len()` 140 → **141** — inside `cc7_the_agent_surface_is_unchanged_by_this_slice`, which revision 1 named nowhere |
| 2 | `crates/kinewright-agent/tests/mcp_server.rs:2501` | `INSPECTOR_TOOL_NAMES` 86 → **87**, same test |
| 3 | `crates/kinewright-agent/tests/mcp_server.rs:10395` | `registry.len()` 140 → **141** |
| 4 | `crates/kinewright-agent/tests/mcp_server.rs:10397` | `registry.len() - operations.len()` 86 → **87** |
| 5 | `crates/kinewright-agent/src/server.rs:26838`, `:26842`, `:26846` | the three registry byte figures, re-measured with their decomposition (§6.4 rule 11) |
| 6 | `crates/kinewright-agent/src/server.rs:22381`, `:26185` | `INSPECTOR_TOOL_NAMES.len()` 86 → **87** |

   Beside them, and **not** part of the six: the served quad's three **value** sites (`tests/mcp_server.rs:2567-2569`, `:10420`, `server.rs:26856`), which assert `7 / 5 660 / 3 510 / 998` **unchanged**, and its five counter-**word** sites (`tests/mcp_server.rs:2427`, `:10346`, `:10366`, `:10421`, `server.rs:26806`), which go from "seventeenth" to **"eighteenth"**. Probe-2 confirmed all three named tests green after the six edits.

**The docs edits at promotion, exhaustively.** `docs/IN2-INVESTIGATOR-SESSIONS.md` (this file, promoted, with §10 figure 2 and the three registry byte figures filled from the implementer's measurement); `docs/ROADMAP-AND-WORKFLOWS.md` (the IN2 row at `:805`, amended to the wording below, and the IN1 status paragraph gaining IN2's two-contract note); **`docs/M36-AGENT-RUNTIME-EFFICIENCY.md`** (one row: the served quad unchanged after IN2 Part A, **eighteenth** consecutive measurement, with the registry sextuple's new figures beside it and the `destructiveHint` count moving 12 → 15); `docs/IN1-INCIDENTS-AND-THE-COLOUR-CASE.md` (§13 D5, D6 and D9 gain owner lines reading "discharged by IN2 Part A"; D1, D2 and D15 gain "IN2 Part B"; D8, D10, D11 and D12 gain "IN3"); `docs/IN1B-ERROR-MIGRATION.md` (§13 D-B2, D-B3, D-B4 and D-B6 gain "IN2 Part B" owner lines; D-B4's cost is corrected from 40–80 to 90–160 per probe-1 P9; D-B1, D-B5 and D-B7 gain "IN3"); **`CHANGELOG.md`** (one entry, naming the investigator session, the 54-code allowlist, the three new buttons and the widened destructive gate).

**The amended roadmap row, in full, written here so it is reviewed rather than composed at promotion** (N1/Q2, N1's proposal ruling, N2.5/B7, §7 rule 8):

> **IN2 — Investigator sessions (contracts: `IN2-INVESTIGATOR-SESSIONS.md`, `IN2B-INCIDENT-PERSISTENCE.md`).** *Deliverable:* a default investigator harness and model in a per-user settings file with discovery, an off switch and per-project per-code mutes in the project document; headless sessions started per incident against the live project on a branch core, with turn, token and wall-time budgets, one per project, deduped by `(code, subject)`, capped at two app-wide, over a named **54**-code allowlist; typed proposals recorded by a new registry-only `propose_fix` capability from the session's own branch, rendered as **approval cards from typed proposals**, applied by the application on one approval through the live core with an undo entry; **the confirmation broker guards destructive tools** and is widened to all seven destructive operation variants; a production scripted driver; `IncidentState::Investigating`, with every end that is not a person's decision returning the incident to `Open` rather than silencing it; the first deterministic recoveries outside the three auto-apply rows. Part B: persistence, a cross-process timestamp, the older-reader gate, the panel-local sinks, the `Result<_, String>` seams, the subject label and the `media backend error: ` prefix. *Exit gate:* sessions start for at least six `Explain` codes and for none of the three auto-apply codes; at least three proposals are approved and applied with zero destructive operations and a measured zero confirmation requests beside one deliberately provoked and auto-rejected; each of the three budgets stops a scripted session at its own bound, reports its numbers on the incident and returns that incident to `Open` unsuppressed; **three** `Explain` codes gain a pressable recovery, so the no-harness fallback resolves **3 of 67** by button; the served quad is unchanged in all three value sites for the eighteenth consecutive measurement.

**Order. A → D → C** (§0.1 N2/i). A is first because every other crate compiles against `IncidentState::Investigating`, `IncidentOutcome::Rejected`, `IncidentProposal` and `INVESTIGATOR_ALLOWLIST`. **D is second and C third, not parallel**, because C's `InvestigatorSession` calls four things D writes — `pump_session`, `SessionStop`, `start_investigator_session` and `ScriptedDriver` — and a C that guesses their signatures is a C that is rewritten. Inside D the order is `session.rs`, then the server changes, then `scripted.rs`, then the pins; inside A the `Document` field and its 52 mechanical edits land **last**, so a compile error from them cannot hide one from the types. Neither C nor D may assert a pinned byte count before §6.4's registry figures and §4.2 rule 11's ceiling are re-measured and in place. The workspace gate and `cargo check -p kinewright-app` run before **each** commit, two review passes per crate precede each, and every cargo command runs after `source ./scripts/setup-ffmpeg.sh`.

IN2 Part A is complete only when a person who does not know why their edit was refused can watch Kinewright work on it inside a budget they can see, read one sentence and one list of edits, press **Approve** once, and get the fix with an undo entry — when the same person, with the investigator switched off and no harness on the machine, still has a button to press on every code that has one — and when turning it off gives them back exactly the incidents they had before, and not silence.
