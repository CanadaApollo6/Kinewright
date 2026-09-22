# IN1 — Incidents, policy, and the colour case

Status: contract revision 2 — for promotion
Programme: [Roadmap and workflows](ROADMAP-AND-WORKFLOWS.md) "Investigator and harness programme", stage IN1
Depends on: [CC1 managed SDR primary](CC1-MANAGED-SDR-PRIMARY.md), [CC6 QC and managed delivery](CC6-QC-AND-MANAGED-DELIVERY.md), [CC7 workflow evaluation](CC7-WORKFLOW-EVALUATION.md), [AU6 workflow evaluation](AU6-WORKFLOW-EVALUATION.md), [M36 agent runtime efficiency](M36-AGENT-RUNTIME-EFFICIENCY.md), [M41 offline relink cache visibility](M41-OFFLINE-RELINK-CACHE-VISIBILITY.md)
Scope: **turning one technical dead end into a typed incident with a declared policy class, applying its safe recovery as a visible, attributed, reversible operation, and proving the whole path — person, scripted agent, and the data a model would read — with an ordinary `cargo test` on both CI operating systems, with no model, no network and no audio device.**

IN1 adds **no served MCP tool** and **no `Operation` variant**; the served quad stays **7 / 5 660 B / 3 510 B / 998 B** for the sixteenth consecutive measurement (§6.6), measured with all of Part A applied. It adds **two internal capabilities** reached through `invoke_capability`, so the registry sextuple moves to a measured **140 / 54 / 86 / 1 551 301 / 1 407 012 / 121 315** and is re-pinned (§6.6).

The words **must**, **must not**, and **may** in this document are normative. Rules are numbered inside their section so a critic can cite "§2 rule 7". Every code claim carries `file:line` at `c3a5814` (unchanged at `3edf6fe`, which touched only `docs/`). A number nobody has measured yet is written **[probe-2b]** and is listed in §11; there are exactly **three**, all of them serialised sizes the implementer measures against the implementation.

**Two parts, one contract.** Part A — *Incidents, policy, and the colour case end to end* — migrates **exactly one** error source, `Media` playback at `crates/kinewright-app/src/app.rs:1252-1255`, and is specified to the line here. Part B — *The migration* — is sized and planned in §10 and is **not** specified to the line by this contract.

---

## 0. Change log

### 0.1 Rulings carried from `orchestrator-notes.md` N0–N1.7

Every finding of `target/review/in1/critic-brief.md` (B1–B8, S1–S13, nits 1–12, Q1–Q10) was ruled on by `orchestrator-notes.md` **N1**/**N1.5**, and the drafter's two deviations by **N1.7**. Both are binding and are not relitigated here. Each ruling is carried by its id and names the section that discharges it. Where revision 2 changes how a ruling is discharged, the change is recorded in §0.2, never here.

- **N1 split — ACCEPTED.** Two parts on one contract; Part A migrates only `Media` playback. §1, §10.
- **B1 — ACCEPTED.** The `set_asset_color_description` provenance guard (`crates/kinewright-core/src/operation.rs:1653-1658`) is widened to admit `UserOverride | AgentAssumption`, plus the byte-equal revert case of N1.5/B4; every other provenance, including `Other(_)`, is still refused by name. CC1 §1's "not silently" is now carried by four things together: the distinct provenance on the asset, the incident card, the revert action, and the guard, which still refuses every provenance that is not an explicit actor. §4.3, §4.4.
- **B2 — ACCEPTED, Part B.** Part A leaves `ErrorLog::push` (`crates/kinewright-app/src/error_ui.rs:30-36`) `pub(crate)` and leaves its three bypasses at `app.rs:693`, `app.rs:708`, `app.rs:1005` untouched, and says so in §1. §10 rule 6.
- **B3 — ACCEPTED, Part A, core + media + app.** `MediaError` gains `SourceColor(#[from] ColorSourceError)` and `SourceColorForAsset { … }`; `recovery_code()` gains both arms; `decode.rs:1081-1086` and `render.rs:704-707` construct them. Rendered text stays byte-identical. §4.1, §4.2.
- **B4 — ACCEPTED, option (b), finalised by N1.5/B4.** The card control is **Revert to probed description**: a second `SetAssetColorDescription` carrying the description the incident captured at open time. `MediaAsset` gains `assumed_from: Option<ColorDescription>`; the guard admits a third case, byte-equality with `assumed_from`. No new `Operation` variant. Global `Undo` is unchanged and is not the card's control. §4.3, §4.4, §5.4.
- **B5 — ACCEPTED.** `IncidentCode` enumerates **only** source-colour codes in Part A. No catch-all, no `Unclassified`. `OpError::incident_family()` is Part B. §2.2, §10 rule 4.
- **B6 — ACCEPTED.** Exit-gate clause 3 is re-cut: zero incidents of class `AskFirst`, zero `HumanQuestion`s of kind `Recovery`, exactly one incident whose class is `AutoApply` and whose card headline is `assert_eq!`-pinned. §9 clause 3.
- **B7 — ACCEPTED: capabilities, not tools.** `get_incidents` is an Inspector capability, `resolve_incident` an Action capability, both reached through `invoke_capability`. Served quad unchanged in **both** pin sites. The tool-shaped alternative is recorded as the road not taken with its measured cost. §6.
- **B8 — ACCEPTED.** `incident_card` in `crates/kinewright-app/src/incident_ui.rs` takes no `egui::Ui` argument; the paint function consumes the view and is untested. §5.3.
- **S1 — ACCEPTED**, boundary sentence verbatim: *core owns the code and the policy class; core does not own the code's trigger.* §2.1 rule 2.
- **S2 — ACCEPTED.** The first-failure classifier behaviour is recorded, the recovery builder fixes all six fields in one operation, and single-field recoveries are forbidden in IN1. §2.4 rules 29, 36–37.
- **S3 / Q2 — ACCEPTED, then REVERSED for Part A by N2/B9.** Confidence stays `COLOR_CONFIDENCE_MAX_BASIS_POINTS` and `AgentAssumption` is the whole signal (kept); **neither allowlist is touched** (reversed). §4.3, §13 D13.
- **S4 / Q8 — ACCEPTED.** Session state in IN1; the older-reader downgrade to `ColorProvenance::Other("agent_assumption")` is deferred explicitly. §13 D1.
- **S5 — ACCEPTED.** The router applies with `Command::DoIfRevision { expected: incident.revision, .. }` and treats `Event::RevisionConflict` as re-observe, never re-apply (mechanism: rule 10 erratum, `refresh_revision`). §5.2 rules 6–8.
- **S6 / Q3 — ACCEPTED.** `KinewrightApp::route_incidents`, called once per `update` after **both** event drains; auto-apply happens at the incident, not at import. §5.2 rules 1–3.
- **S7 — ACCEPTED.** `HumanQuestion.kind: QuestionKind { Recovery, Judgement }` defaulting to `Judgement`; the incident record carries its own `Duration` and, when an agent resolved it, the `Cost` categories reported. §8.
- **S8 — ACCEPTED** as an exit-gate clause asserted by equality over the whole `POLICY` table. §9 clause 6.
- **S9 — ACCEPTED.** `opened_at: Duration` since a session-start `Instant` named `IncidentLog::started`, mirroring `ErrorLog::started` (`error_ui.rs:17`). §2.3 rules 15–16.
- **S10 — Part B.** The two dynamic-source sites (`app.rs:403`, `inspector_ui.rs:793`) are named in Part A's non-delivery list. §1, §10.
- **S11 — RULED differently from the critic for Part A.** A migrated incident **also** writes one audit line to `ErrorLog` with source `"Incident"`, so the audit view stays complete and the toolbar badge does not regress while **120** literal-labelled sites are unmigrated. The two counts are different quantities and the contract says so. §5.2 rules 11–13.
- **S12 — dissolved by B7.**
- **S13 — ACCEPTED**, with the reason written beside the rule. §2.2 rule 9.
- **Nit 1–5, 10–11 — ACCEPTED**, absorbed into this contract's citations.
- **Nit 6 — ACCEPTED.** Fixture geometry reuses CC7/AU6: 25 fps, 320×180, `yuv420p`, 50 frames. §3.
- **Nit 7 — ACCEPTED.** The actor word in Part A is **Kinewright**. §5.3 rule 22.
- **Nit 8 — ACCEPTED.** Test names follow the house shape `in1_…`. §7 rule 1.
- **Nit 9 — ACCEPTED.** Deferrals say "deferred to IN2/IN3", not "no". §13.
- **Nit 12 — ACCEPTED.** §1 carries a negative inventory.
- **Q1 — core**, with S1's boundary sentence. **Q4 — `AutoApply`**, with the concrete probed `range` from P2 asserted. **Q5 — `Resolved(Reverted)`** with re-apply suppressed for that asset for the session. **Q6 — capabilities.** **Q7 — `Media` only in Part A.** **Q9 — Media panel beside the asset**, untested placement. **Q10 — recorded in the programme deferrals.**
- **N1.5 fills — carried.** The migration population (§10 rule 1), the four fixture tuples (§3 rule 5), the Part A code set and its `unknown_source_white_point` exclusion (§2.2 rule 6), the `assumed_from` revert (§4.4), the road not taken (§6.7).
- **N1.7/C1 — carried.** `INSPECTOR_TOOL_NAMES` is `[&str; 86]`, not `[&str; 85]`: `is_invocable_capability` (`crates/kinewright-agent/src/runtime.rs:194-206`) requires membership for anything reached through `invoke_capability` and the dispatcher refuses non-members by name (`server.rs:698-704`). §6.1, §6.6.
- **N1.7/C2 — carried, with its reason corrected by the contract critic.** `unsupported_source_colour_refuses_the_export_with_its_code_and_field` (`crates/kinewright-app/src/export_ui.rs:2017-2068`) *does* read a `MediaError` — `export_ui.rs:2064-2067` matches `Err(MediaError::Backend(message))`. It nevertheless stays green and unmodified, because that `Backend` is built on the delivery-preflight path from the `QaIssue` code at `crates/kinewright-core/src/delivery.rs:536`, never by `decode.rs:1081` or `render.rs:704`. §9 regression item R2.

### 0.2 What revision 2 changed, by id

`target/review/in1/critic-contract.md` (667 lines) and `target/review/in1/probe2-report.md` (866 lines) were ruled on by `orchestrator-notes.md` **N2** and **N2.5**. Every id, with one line saying what changed and where.

**Blocking findings (N2, all ten accepted).**

- **B1** — `SourceColorForAsset`'s `#[error(...)]` now carries the *whole* historical sentence, including the nested `media backend error: managed source profile rejected for {path} (assumption={assumption:?}): ` fragment, with `{status}` derived from the carried `ColorSourceError` accessors as trailing format arguments; the phrase "hand-written `Display`" is deleted. §4.1 rules 1–3.
- **B2** — §9 clause 11 pins a `format!` **template**, not a rendered string, because the message interpolates a per-run temp path **twice**. §9 clause 11, §4.1 rule 4.
- **B3** — `IncidentCode` serialises as the stable `code()` string; `field` is a first-class `Incident` field filled from `code.field()`; the round-trip test covers both. §2.2 rules 7–8, §2.3 rule 10, §6.2 rule 10.
- **B4** — `RecoveryKind` is **externally tagged** (the smaller of the two forms, see §2.5 rule 38); `Incident::state` is `#[serde(flatten)]`; §6.2's body is normative *as generated* and a test `assert_eq!`s the fixture serialisation against the literal. §2.3 rule 10, §2.5 rule 38, §6.2 rules 9–11.
- **B5** — `Incident.class: PolicyClass`, resolved at `observe` time and serialised. §2.3 rule 10, §2.3b rule 24.
- **B6** — the `IncidentLog` lives on `ProjectSession` (`crates/kinewright-app/src/project.rs:210`); the media drain attributes a playback incident to the **focused** project, normatively, with the reason; multi-project attribution is a named Part B deferral. §5.1, §5.2 rules 3–5, §13 D14.
- **B7** — `rec709_compatible` is defined by a provable property and implemented as an explicit chain, with an equivalence test; the rationale paragraph is rewritten. §2.4 rules 37–38. **The definition N2 dictated could not be used verbatim; see §0.3 D1.**
- **B8** — `IN1_FIXTURE_DEDUP_COUNT` is deleted; §9 clause 4 asserts exactly one open incident and `count >= 1`; `count == 2` moves to a deterministic core test. §2.3 rule 13, §3 rule 12, §9 clause 4.
- **B9** — **N1/S3 reversed for Part A**: neither allowlist is touched, the `allowed` phrase does not change, `export.rs:2677` and `delivery.rs:2211` stay green unmodified; §4.3's two tests become one, that provenance does not gate the decode. §4.3 rules 11–13, §9 regression item R3, §13 D13.
- **B10** — (i) the log inserts `(code, subject)` into `suppressed` on every router auto-apply; (ii) `incident_card(incident, assumed_from_present)` disables the revert action when `assumed_from` is `None`; (iii) a new exit-gate clause covers auto-apply followed by global `Undo`. §2.3 rules 17–18, §5.2b rule 16, §5.3 rule 20, §9 clause 17.

**Substantive findings (N2, all fourteen accepted).**

- **S1** — §6.7 is two tables: served-surface cost of the route decision, and registry cost of the two capabilities either way (identical on both routes, because `served_tools()` filters `capability_tools()` at `server.rs:649-654`). "Pays zero of this" is qualified to *served* bytes. §6.7.
- **S2** — old clauses 12, 14 and 19 move beneath the numbered gate into "regressions guarded and process gates"; the `opened_at` clause is re-cut positively. §9.
- **S3** — the budget constant is renamed `IN1_INCIDENT_SERIALIZED_BYTES` and asserted with `assert_eq!` against the named largest fixture incident. §6.2 rule 9, §9 clause 14.
- **S4** — `message` leaves the wire shape and the type; every `Option` telemetry field gains `skip_serializing_if`; `opened_at` is `#[serde(skip)]`. The measured saving is **[probe-2b]**. §2.3 rule 10, §8 rule 4, §11.2.
- **S5** — `observe` returns `Observed::{Suppressed, Deduped(IncidentId), Opened(IncidentId)}`. §2.3 rule 18.
- **S6** — `IncidentLog::resolve` and `IncidentLog::note_auto_applied` own the `suppressed` insertion; the old rule 17 is re-worded. §2.3 rule 17.
- **S7** — the `Srgb` sentence is written; `assume_srgb_full` is a §13 deferral owned by IN3. §2.4 rule 35, §13 D8.
- **S8** — the single-string `recovery_action()` fact is stated; per-code `Explain` bodies arrive with IN3. §2.5 rule 41, §13 D8.
- **S9** — a fourth roadmap-amendment row amends the *deliverable* column, not only the gate. §9 roadmap table.
- **S10** — clause 16 quantifies over the tests that survive §12's cut order; the WebM fallback question is moot (probe-2 T4). §9 clause 16, §3 rule 10.
- **S11** — the byte-identity clause takes the `contracts.rs:854-858` shape on an in-test document, naming `assumed_from`. §9 clause 10.
- **S12** — the canonical evidence per `POLICY` row is `in1_untagged.mp4`'s pinned tuple, both directions asserted. §9 clauses 2 and 6.
- **S13** — "the same six categories, each `Option`-wrapped because IN1 records no session". §8 rule 5.
- **S14** — one test, both fixtures, one log, one process; §7 now has **seven** `in1_` tests, not eight. §7 rule 3.

**Nits (N2: accepted unless one conflicts with a ruling).**

- **Nit 1** — "(twelve incident codes from thirteen classifier variants)" written once. §2.2 rule 6.
- **Nit 2** — the `setparams=` recipe takes `cc7_sources.rs:540`'s spelling, `range=limited`. §3 rule 4.
- **Nit 3** — one sentence records that `deny_unknown_fields` emits `additionalProperties: false` and therefore *adds* registry bytes, which probe-2's figure includes. §6.2 rule 6.
- **Nit 4** — the `destructive(false)` conclusion stands on `runtime.rs:351-358` alone; the `runtime.rs:150-152` citation is dropped. §6.3 rule 12.
- **Nit 5** — the amended media test's *input* changes to `MediaError::SourceColor(ColorSourceError::UnsupportedPrimaries(Bt2020))`, or the new assertions cannot hold. §4.2 rule 7.
- **Nit 6** — the app adapter that delegates to the core builder is named, so "cannot drift" is checkable. §4.5 rules 32–35.
- **Nit 7** — **conflicts with B9 and is overridden there**; see §0.3 D3. The other two halves are applied: `project.rs` is listed as gaining a *field* as well as a type alias, and `lib.rs`'s `pub use incident::{…}` names are spelled out. §14.
- **Nit 8** — `incident.rs` is re-estimated at 1 200–1 500 lines. §12.
- **Nit 9** — the `invoke_capability` helper's signature is named: `async fn invoke_capability(client: &RunningService<RoleClient, ()>, name: &str, arguments: serde_json::Value) -> CallToolResult` (`tests/mcp_server.rs:1934-1938`). §7 rule 1.
- **Nit 10** — §1 item 3's sentence is corrected: `CoreState::do_operation`'s `undo`/`op_log` (`actor.rs:238-247`) is **session** state; the only durable residue of a resolved incident is `MediaAsset.assumed_from` and the written `color_description`. §1 item 3.
- **Citation corrections** from the critic's table, all applied: `render.rs:704-707` (not `:702-707`), `render.rs:679-708` for `contextual_managed_decode_error` with the wrap at `:704-707` (not `:688-708`), `error_ui.rs:17` for `started` (not `:11`), `error_ui.rs:30-36` for `push`, `color.rs:946-962` with the `matches!` at `:952-955`, `color.rs:277-301` for `observed()`, `operation.rs:4275-4281` for `validate_document`'s colour loop, `actor.rs:393-397` for `Command::DoIfRevision`, `server.rs:665-670` for the direct-call refusal, `captions.rs:42` for the discarded `MediaError`, `docs/CC1-MANAGED-SDR-PRIMARY.md:66` for the `rec709_video` profile row, `media.rs:1760` for `recovery_code()`, `server.rs:1951-1959` is a `lut_import_error(...)` wrapper rather than `error_structured` directly. **One citation the critic marked as drift is not drift; see §0.3 D2.**

**New orchestrator rules (N2.5).**

- **N2.5/registry** — `AddAsset` **must** reject an asset whose `assumed_from` is `Some(_)`, closing the laundering path the new field opens in the `add_asset` input schema. §4.4 rules 26–27, §9 clause 18. **The ruling names `RelinkAsset` too and could not be honoured there verbatim; see §0.3 D4.**
- **N2.5/109** — §12 gains a row for the **109** `assumed_from: None` struct-literal edits across `--workspace --all-targets`. §12.
- **N2.5/deadline** — `IN1_DECODE_EVENT_DEADLINE` stays **10 s** against probe-2's recommended 1 s, with the measured 0.0754 s recorded in §11.1. §7 rule 13.
- **N2.5/webm** — §3 rule 10's fallback branch is **deleted**: the Windows pinned FFmpeg compiles `libvpx-vp9`, so the WebM pair gates both operating systems. §3 rule 10.
- **N2.5/muxer** — §3 rule 6's doc comment is corrected: it is the **Matroska/WebM muxer** that always writes a `Colour/Range` element, not the VP9 bitstream header. §3 rule 6.
- **N2.5/commit_edit_plan** — §6.7 rule 29's "byte-for-byte the size of `commit_edit_plan`" is **deleted**; the drafted schemas measure 1 145 / 493 / 492 and 1 339 / 809 / 365. §6.7 rule 29.
- **N2.5/zero-confidence** — both directions are stated as rules: a revert to a zero-confidence probed description returns `Ok(())`; a zero-confidence `AgentAssumption` returns `OpError::ZeroConfidenceColorOverride`. §4.4 rules 17 and 27.
- **N2.5/on-load** — §9 clause 10 says "rejected **on load**", at `deserialize_confidence_basis_points` (`color.rs:556-557`), with `validate_asset`'s new line keeping the programmatic path and its own unit test. §4.4 rule 23, §9 clause 10.
- **N2.5/errorlog** — §9's audit-line clause asserts the number of `"Incident"`-source entries is 1, **not** `ErrorLog::len() == 1`, because an ALSA `snd_pcm_pause` failure from `engine.rs:1920` is nondeterministic on a box with no audio device; the app-side test does not call `play`. §9 clause 12, §7 rule 3.
- **N2.5/two-emissions** — the "two emit sites" statement is corrected: both colour emissions leave through `engine.rs:2050` (`Worker::fail`) from two different `present` callers (`engine.rs:1857` and `:1876`); `engine.rs:1920` is the audio-pause site and carries no colour refusal. §3 rule 12, §11.1 limit 1.
- **N2.5/one-figure** — one migration figure everywhere: **121** literal sites over **15** labels, **+2** dynamic, **+3** bypasses; Part A migrates **1**; Part B owns **120 + 2 + 3 = 125**. §0.2 above, §1 item 6, §5.2 rule 11, §10 rule 1, §12, §13 D4.
- **N2.5/measured values** — the served quad, the registry sextuple and its attribution, the pin sites, the four fixture tuples, the encode cost, the display template, the dedup counts, the core-suite result and the `pre_m13_project.json` figures are folded in verbatim wherever a **[probe-2]** marker stood. Every **[probe-2]** marker is gone; three **[probe-2b]** markers replace the serialised sizes N2/S3 and N2/S4 moved. §6.2 rule 9, §6.6, §6.7, §11.

### 0.3 Deviations revision 2 makes from the rulings, with evidence

Four. Each keeps the ruling's intent, states the line that makes the literal form impossible, and is flagged to the orchestrator.

- **D1 — N2/B7's definition of `rec709_compatible` is near-vacuous as written, so revision 2 adds one clause and one field to it.** N2/B7 defines the predicate as `classify_source_with_assumption(&recovery_description(probed), None) == Ok(Rec709Video)`. But the recovery builder rewrites primaries, transfer and matrix **unconditionally** to `Bt709` (`crates/kinewright-app/src/color_ui.rs:129-131`) and sets `white_point: D65` unconditionally (`color_ui.rs:133`). A `Bt2020 / Smpte2084 / Bt2020Ncl` probe therefore *does* classify `Ok(Rec709Video)` after the recovery, so the definition alone returns `true` for almost every input and cannot be equivalent to N2's own implementation chain. Revision 2 keeps both halves of the intent — provable, not argued — by conjoining the clause the chain actually encodes: *the recovery overwrites no **known** field*. It also adds `white_point` to the chain, which N2's list omits and without which the equivalence fails for a known `D50` probe. §2.4 rules 37–38 carry the full form and its equivalence test.
- **D2 — the guard is at `operation.rs:1653-1658`, not `:1652-1657`.** The contract critic's citation table lists the draft's `1653-1658` as drift. It is not: at `c3a5814`, line 1652 is the `}` closing the zero-confidence check and line 1653 is `if color_description.provenance != ColorProvenance::UserOverride {`, ending at 1658. The draft's citation stands; every other correction in that table was verified and applied.
- **D3 — nit 7's request to add `crates/kinewright-media/src/export.rs` to §14 conflicts with B9 and B9 wins.** The critic asked for `export.rs` in §14 only "if N2 keeps the widening". N2/B9 reverses the widening, so `export.rs:2677` and `delivery.rs:2211` are **not edited**; they appear in §9's "regressions guarded" list instead, which is the stronger statement — the literals must still read `"application_default or user_override"` after IN1.
- **D4 — N2.5's `AddAsset`/`RelinkAsset` rule lands on `AddAsset` only, because `RelinkAsset` carries no `MediaAsset`.** `Operation::RelinkAsset { asset: AssetId, candidate: RelinkCandidate, allow_unverified_source: bool }` (`operation.rs:31-38`) carries a `RelinkCandidate`, whose six fields are `path`, `fingerprint`, `kind`, `fps`, `duration`, `resolution` (`crates/kinewright-core/src/model.rs:150-157`) — no `MediaAsset`, no `ColorDescription`, no `assumed_from`. `relink_asset` writes exactly two fields, `path` and `source_fingerprint` (`operation.rs:1608-1612`), and cannot reach `assumed_from`. The laundering path the ruling names exists only through `AddAsset { asset: MediaAsset }` (`operation.rs:24-26`), which is where the refusal goes. The ruling's intent — `assumed_from` is written only by the recovery and cleared only by the revert — is fully honoured, and `RelinkAsset` gets the test that proves its half rather than a rule it cannot break. §4.4 rules 26–27.

---

## 1. In scope, and what IN1 does not deliver

IN1 Part A delivers:

- **`kinewright_core::incident`** (§2) — `Incident`, `IncidentCode`, `SourceColorIncident`, `IncidentSeverity`, `IncidentSubject`, `IncidentEvidence`, `IncidentOutcome`, `IncidentState`, `IncidentTelemetry`, `IncidentObservation`, `Observed`, `RecoveryAction`, `RecoveryKind`, `PolicyClass`, `PolicyPredicate`, `PolicyEntry`, the `POLICY` table, `IncidentLog`, `recovery_description`, `assume_rec709_operation`, `rec709_compatible`, `policy_class`, `policy_recovery`. Data and policy only: no I/O, no clock beyond `IncidentLog::started`, no rendering.
- **Typed source-colour errors end to end** (§4.1, §4.2) — `MediaError::SourceColor` and `MediaError::SourceColorForAsset`, constructed where the type used to die, with `recovery_code()` arms and a byte-identical rendered sentence.
- **`ColorProvenance::AgentAssumption` and the widened operation guard** (§4.3, §4.4) — plus `MediaAsset.assumed_from`, the revert, and the `AddAsset` refusal.
- **The router and the card** (§5) — `KinewrightApp::route_incidents` at a named tick, and `incident_card` as a pure function with an `assert_eq!`-pinned headline.
- **Two internal capabilities** (§6) — `get_incidents` (Inspector) and `resolve_incident` (Action), reached through `invoke_capability`, with the served quad unchanged and the registry re-pinned.
- **Four generated fixtures** (§3) — `in1_untagged.mp4`, `in1_untagged_vp9.webm` and their BT.709-tagged twins, with pinned probed tuples.
- **Scripted agent tests** (§7) and **telemetry fields** (§8).

**IN1 does not deliver.** The inventory is normative; each item is a thing a reader might reasonably expect and must not.

1. **No new `Operation` variant.** The amended provenance guard is not a variant, and `operation_tools().len()` stays **54** (measured after all of Part A: probe-2 T1.6).
2. **No served-tool growth.** `COMPACT_TOOL_NAMES` stays `[&str; 7]` (`crates/kinewright-agent/src/runtime.rs:17-25`) and the served quad stays 7 / 5 660 / 3 510 / 998.
3. **No persistence of incidents.** `IncidentLog` is session state. The **only durable residue** of a resolved incident is the asset's written `color_description` and `MediaAsset.assumed_from` (§4.4). `CoreState::do_operation`'s `undo`/`op_log` (`crates/kinewright-core/src/actor.rs:238-247`) is in-memory session state, not a durable journal, and nothing in this contract rests on it.
4. **No model, no harness, no investigator session.** IN1's router is the investigator's deterministic stand-in. Default-model selection, discovery, the off switch and headless starts against the live project are IN2.
5. **No broker change.** `ConfirmationRequest` keeps its three fields (`crates/kinewright-agent/src/server.rs:137-142`); no IN1 path calls `ConfirmationBroker::confirm`. Part A declares no `AskFirst` code, so no approval card is needed. IN2 owns the typed payload.
6. **No migration beyond `Media` playback.** The other **120** literal-label sites, the **2** dynamic-source sites (`app.rs:403`, `inspector_ui.rs:793`) and the **3** `ErrorLog::push` bypasses (`app.rs:693`, `app.rs:708`, `app.rs:1005`) are untouched and stay `record_error`/`push` lines — **125** error paths for Part B. `ErrorLog::push` stays `pub(crate)`. **Erratum (IN1b-R11, 2026-09-15):** superseded twice over. The population is **129** error paths, not 125 (see the IN1b-R6 erratum on §10 rule 1), and `ErrorLog::push` does **not** stay `pub(crate)`: [IN1b](IN1B-ERROR-MIGRATION.md) §5.3 rule 17 drops it to module-private with `KinewrightApp::note_incident` as its only caller.
7. **No relink or transcode execution**, and no `RecoveryKind` variant for either. IN3 owns them.
8. **No single-field recoveries.** §2.4 rule 37.
9. **No per-OS constant.** Every pinned number is the same on both operating systems; a per-OS figure, if one is ever found, is a doc-comment note beside the constant, never a second constant (AU6 §0.2 item 12's discipline).
10. **No badge change.** The toolbar badge still counts `self.error_log.len()` (`crates/kinewright-app/src/app.rs:1487-1491`). Part B settles its semantics. **Erratum (IN1b-R8, 2026-09-15):** Part B settled them: [IN1b](IN1B-ERROR-MIGRATION.md) §5.4 rule 25 makes the badge count **open incidents** (`IncidentLog::open_count()`) while the audit window keeps `entries.len()`. The citation is also stale — at `d4ed8eb` the badge is `app.rs:1875` (`if self.error_log.len() > 0`) and `app.rs:1879` (`format!("{}", self.error_log.len())`).
11. **No `IncidentCode` beyond source colour**, no catch-all, no `Unclassified`, no `#[non_exhaustive]`.
12. **No conversion of an unsupported profile.** Every unsupported code is `Explain` in IN1.
13. **No live `KinewrightApp` harness.** Unchanged from CC7 §13 and AU6 §1: `KinewrightApp::new` is private (`app.rs:206`), there is no `crates/kinewright-app/tests/`, and every app test is a `#[cfg(test)] mod tests` inside a `src/*.rs` file.
14. **No widening of either managed-colour allowlist.** `color_description_matches_managed` (`crates/kinewright-core/src/color.rs:946-962`) and `delivery_color_mismatches` (`crates/kinewright-core/src/delivery.rs:622-631`) are unchanged, and so is the `"application_default or user_override"` phrase (N2/B9, §13 D13).
15. **No human-readable asset name on the wire.** `Incident.subject` is `{"asset": n}`; the asset's `name` is a named deferral (§13 D15). **Erratum (IN1b-R13, 2026-09-15):** "`Incident.subject` is `{"asset": n}`" is falsified by [IN1b](IN1B-ERROR-MIGRATION.md) §3.11 rule 42's three-shape union — a one-key object over a `u64`, a one-key object over a nested union (`{"chain":{"bus":3}}`), or a bare string (`"project"`). The `name` half is unchanged and is widened to seven subject kinds as IN1b §13 D-B6.

---

## 2. The incident model and the policy table — `kinewright_core::incident`

### 2.1 Module boundary

1. The module is `crates/kinewright-core/src/incident.rs`, declared `mod incident;` in `crates/kinewright-core/src/lib.rs` beside `mod color;` (`lib.rs:22`) and re-exported from the crate root in the shape of the existing `pub use color::{…}` block (`lib.rs:92`), with the names spelled out (§14). It **must not** be feature-gated: the app, the agent and the media crate all read it on the production path.
2. **Core owns the code and the policy class; core does not own the code's trigger.** An app-only or media-only failure is a core enum variant whose only constructor is outside core, exactly as `LiveAudioChange` (`crates/kinewright-core/src/media.rs:1780-1793`) lives in core because it crosses a crate boundary while the app computes it.
3. The module **must not** perform I/O, spawn a thread, read the filesystem, or call the classifier on a description it did not receive as an argument. It **may** hold one `std::time::Instant` (§2.3 rule 16) and it **must** hold no other clock.
4. `kinewright-core` has **no** `std::time::Instant` today (`grep -rn "Instant" crates/kinewright-core/src/` is empty at `c3a5814`). Rule 3's single exception is deliberate and is justified in §2.3 rule 15.

### 2.2 `IncidentCode`

5. `IncidentCode` is an exhaustive core enum. In Part A it has exactly one variant:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IncidentCode {
    SourceColor(SourceColorIncident),
}
```

   **It does not derive `Serialize`.** Per N2/B3 it carries a hand-written `Serialize` that emits `self.code()` and nothing else, so the wire value is the stable identifier every other agent surface already publishes (`delivery.rs:538`, `render.rs:670`) and never a nested map:

```rust
impl Serialize for IncidentCode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.code())
    }
}
```

6. `SourceColorIncident` mirrors `ColorSourceError::code()` (`crates/kinewright-core/src/color.rs:244-259`) **one-to-one except `unknown_source_white_point`**, which is not an incident — **twelve incident codes from thirteen classifier variants**. The reason is measured, not stylistic: `color_description_from_decoder` never sets a white point (`crates/kinewright-media/src/decode.rs:154`), so every correctly BT.709-tagged source in existence fails the bare classifier with that code (probe-1 §P2.4 result 2, reconfirmed by probe-2 T3.2) and passes only through `d65_assumption` (`crates/kinewright-media/src/render.rs:760-767`). **Erratum (IN1b-R4, 2026-09-15):** "**twelve incident codes from thirteen classifier variants**" becomes thirteen from thirteen ([IN1b](IN1B-ERROR-MIGRATION.md) §3.2 rule 16): `unknown_source_white_point` is a Part B code. This rule's measured *reason* — that the bare classifier reports it for every correctly tagged source — is unchanged and untouched; IN1b §3.2 rule 14 names the post-assumption sRGB-primaries path that makes the code reachable.

| # | `IncidentCode::SourceColor(_)` | `code()` | `field()` | declared class | predicate |
| ---: | --- | --- | --- | --- | --- |
| 1 | `UnknownPrimaries` | `unknown_source_primaries` | `primaries` | `AutoApply` | `Rec709Compatible` |
| 2 | `UnknownTransfer` | `unknown_source_transfer` | `transfer` | `AutoApply` | `Rec709Compatible` |
| 3 | `UnknownMatrix` | `unknown_source_matrix` | `matrix` | `AutoApply` | `Rec709Compatible` |
| 4 | `UnknownRange` | `unknown_source_range` | `range` | `Explain` | `Always` |
| 5 | `UnknownBitDepth` | `unknown_source_bit_depth` | `bit_depth` | `Explain` | `Always` |
| 6 | `UnsupportedPrimaries` | `unsupported_source_primaries` | `primaries` | `Explain` | `Always` |
| 7 | `UnsupportedTransfer` | `unsupported_source_transfer` | `transfer` | `Explain` | `Always` |
| 8 | `UnsupportedMatrix` | `unsupported_source_matrix` | `matrix` | `Explain` | `Always` |
| 9 | `UnsupportedRange` | `unsupported_source_range` | `range` | `Explain` | `Always` |
| 10 | `UnsupportedWhitePoint` | `unsupported_source_white_point` | `white_point` | `Explain` | `Always` |
| 11 | `UnsupportedBitDepth` | `unsupported_source_bit_depth` | `bit_depth` | `Explain` | `Always` |
| 12 | `UnsupportedCombination` | `unsupported_source_combination` | `profile` | `Explain` | `Always` |

7. `SourceColorIncident::code(self) -> &'static str` **must** return the `ColorSourceError::code()` string for the same failure, and `SourceColorIncident::field(self) -> &'static str` **must** return `ColorSourceError::field()`'s (`color.rs:264-274`). `IncidentCode` delegates both. The strings are **not** restated as literals anywhere in core.
8. One core test **must** assert the round trip over all **thirteen** `ColorSourceError` variants, comparing accessor against accessor: every variant except `UnknownWhitePoint` maps to exactly one `SourceColorIncident` whose `code()` and `field()` are the identical `&'static str`s, and `UnknownWhitePoint` maps to `None`. **Erratum (IN1b-R4, 2026-09-15):** "every variant except `UnknownWhitePoint` … and `UnknownWhitePoint` maps to `None`" becomes thirteen-for-thirteen with **no `None` case**, per [IN1b](IN1B-ERROR-MIGRATION.md) §3.2 rules 12 and 16.
9. There **must not** be a catch-all code, an `Unclassified` variant, or an `Other(String)`. A failure with no code is not an incident in IN1 and stays a `record_error` line (§1 item 6). **Erratum (IN1b-R12, 2026-09-15):** "A failure with no code is not an incident in IN1 and stays a `record_error` line" is superseded — after [IN1b](IN1B-ERROR-MIGRATION.md) §5.3 rule 23 every measured failure has a code and there is no `record_error` line to stay as. The rest of this rule — no catch-all, no `Unclassified`, no `Other(String)` — is unchanged, and IN1b §3.2 rule 10 keeps it. This is the twin of the §2.3c rule 28 erratum IN1b-R2.
10. `IncidentCode` and `SourceColorIncident` **must not** carry `#[non_exhaustive]`. The reason, written here because it is load-bearing and not a style preference: `#[non_exhaustive]` on a crate-public enum forces a wildcard arm in every downstream `match`, which would make §2.4 rule 32's exhaustiveness test — the one thing that proves the policy table covers every code — pass vacuously.

### 2.3 `Incident`, `IncidentLog`, and the dedup key

11. The record. Every serde attribute below is normative; §6.2 rule 10's body is its exact output.

```rust
pub struct IncidentId(pub u64);                           // serialises transparently

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentSeverity { Blocks, Degrades, Informs }

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentSubject { Asset(AssetId) }

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentEvidence {
    SourceColor {
        /// The description the incident captured at open time. The revert
        /// restores exactly these bytes (N1.5/B4).
        probed: ColorDescription,
        assumption: Option<ColorSourceProfileAssumption>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentOutcome { Applied, Reverted, Explained }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "outcome")]
pub enum IncidentState { Open, Resolved(IncidentOutcome) }

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Incident {
    pub id: IncidentId,
    /// Serialises as the stable `code()` string (§2.2 rule 5).
    pub code: IncidentCode,
    /// Resolved through `policy_class(code, &evidence.probed)` at `observe`
    /// time and stored, so an investigator decides from one read (N2/B5).
    pub class: PolicyClass,
    pub severity: IncidentSeverity,
    pub subject: IncidentSubject,
    /// Always `code.field()`; assigned at exactly one site (N2/B3).
    pub field: &'static str,
    pub observed: String,
    pub allowed: Option<String>,
    pub evidence: IncidentEvidence,
    pub recoveries: Vec<RecoveryAction>,
    pub revision: TimelineRevision,
    /// Session-relative and never serialised: a wire value nothing may assert
    /// is a wire value nothing should be charged for (N2/S4).
    #[serde(skip)]
    pub opened_at: Duration,
    pub count: u32,
    #[serde(flatten)]
    pub state: IncidentState,
    pub telemetry: IncidentTelemetry,   // §8
}
```

12. **`message` is not a field.** It was `ColorSourceError::actionable_message()` (`color.rs:336-345`), which is literally `display + " (field=…, observed=…, allowed=…). " + recovery_action()` — so `observed` and `allowed` travelled twice and `recovery_action` three times. It is reconstructible from `code`, `field`, `observed` and `allowed`, all of which are on the record. It is removed from the **type**, not hidden with `#[serde(skip)]`: a field nobody reads is a field that drifts (N2/S4).
13. `IncidentState::Investigating` is **not** declared in Part A: nothing constructs it while there is no session, and an unconstructed variant is dead code the exhaustive tests would have to carry. IN2 adds it (§13 D5).
14. `IncidentSubject` declares **only** `Asset(AssetId)` in Part A, for the same reason. Part B adds `Clip`, `Track`, `ExportJob`, `Project` and `Agent` as its sources are migrated (§10 rule 5).
15. **The dedup key is `(code, subject, observed)`**, in that order, and nothing else. `IncidentLog::observe` **must** search the **open** entries for an equal key; on a hit it **must** increment `count`, refresh `revision` to the observing revision, leave `opened_at` unchanged, and return `Observed::Deduped(id)`; on a miss it **must** append a new entry with `count: 1` and return `Observed::Opened(id)`. It **must not** compare `ColorDescription`s: probe-1 §P2.4 and probe-2 T3.2 both measured that an untagged MP4 and an untagged WebM differ in `range`, `confidence_basis_points` and `provenance` while sharing code, subject shape and observed, so a description-keyed dedup would split one problem into two incidents.
16. A `Resolved` entry **must not** absorb an observation. Dedup applies to `Open` entries only. The re-open that this permits is closed by rule 18's suppression, not by weakening this rule.
17. `opened_at: Duration` is measured from a session-start `Instant` named `IncidentLog::started`, mirroring `ErrorLog::started` (`crates/kinewright-app/src/error_ui.rs:17`, `:24`, `:33`). `Instant` is not serializable and `SystemTime` is not monotonic; `Duration` since a named origin is the only honest session-scoped stamp, and it is the shape the audit view already uses.
18. `IncidentLog` owns the origin and the suppression set:

```rust
pub struct IncidentLog {
    started: Instant,
    next_id: u64,
    entries: Vec<Incident>,
    suppressed: BTreeSet<(IncidentCode, IncidentSubject)>,
}
```

   `Default` captures `Instant::now()`. `IncidentLog::with_start(started: Instant)` **must** exist so a test can pin the origin; every test that reads `opened_at` **must** use it.
19. **The type owns its suppression invariant** (N2/S6, N2/B10(i)). Nothing outside `IncidentLog` inserts into `suppressed`. Exactly two methods insert:
    - `IncidentLog::note_auto_applied(&mut self, id) -> bool` — called by the router at the moment it **sends** an auto-apply command, before any acceptance arrives. A session auto-applies at most once per `(code, subject)`; any later return of the same problem is reported, never re-fixed. This is what makes a global `Undo` stick (N2/B10) and what closes the frame-boundary re-apply race (N2/B8).
    - `IncidentLog::resolve(&mut self, id, outcome) -> bool` — inserts for **every** outcome, `Applied`, `Reverted` and `Explained` alike. A session reports a given `(code, subject)` once; resolving it by any route ends that session's reporting of it.

   `observe` **must** return `Observed::Suppressed` for a suppressed key and **must not** create or touch an entry. Suppression is cleared only by dropping the log, which happens when the project session closes.
20. Public surface, exactly:

```rust
pub enum Observed { Suppressed, Deduped(IncidentId), Opened(IncidentId) }

impl IncidentLog {
    pub fn observe(&mut self, observation: IncidentObservation) -> Observed;
    pub fn open(&self) -> impl Iterator<Item = &Incident>;
    pub fn all(&self) -> impl Iterator<Item = &Incident>;
    pub fn get(&self, id: IncidentId) -> Option<&Incident>;
    pub fn resolve(&mut self, id: IncidentId, outcome: IncidentOutcome) -> bool;
    pub fn note_auto_applied(&mut self, id: IncidentId) -> bool;
    pub fn open_count(&self) -> usize;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn with_start(started: Instant) -> Self;
}
```

   Nothing else is `pub`. **Erratum (implementation, 2026-09-15):** one more accessor is `pub`, `telemetry_mut(&mut self, id: IncidentId) -> Option<&mut IncidentTelemetry>`, the single write path for `resolved_after` and the six cost mirrors; without it §5.2 rule 12 and §6.3 rule 16 could not be implemented. **Erratum (implementation, 2026-09-15):** a third narrowly typed writer is also `pub`, `refresh_revision(&mut self, id: IncidentId, revision: TimelineRevision) -> bool`, which writes `revision` and nothing else, only on an `Open` entry, and returns `false` unchanged for an unknown id or a `Resolved` entry; §5.2 rule 10 and §9 clause 9 require the refresh and `observe` cannot perform it, because rule 19's suppression returns first (see §5.2 rule 10's erratum). The three-state `Observed` exists because the router must do different things for a newly opened incident (write one `ErrorLog` audit line, consider auto-apply), a dedup hit (do neither) and a suppressed observation (do nothing at all); `Option<IncidentId>` could not tell the first two apart (N2/S5).

### 2.3b `IncidentObservation` — the only way an incident is created

21. Nothing outside `incident.rs` constructs an `Incident`. The one entry point is an observation, which carries everything the log needs and nothing it does not:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncidentObservation {
    pub code: IncidentCode,
    pub subject: IncidentSubject,
    pub observed: String,
    pub allowed: Option<String>,
    pub evidence: IncidentEvidence,
    pub revision: TimelineRevision,
}

impl IncidentObservation {
    /// Build an observation from a typed media failure, or `None` when the
    /// failure has no incident code in this slice.
    #[must_use]
    pub fn from_media_error(error: &MediaError, revision: TimelineRevision) -> Option<Self>;
}
```

22. `from_media_error` **must** match on every `MediaError` variant with **no** wildcard arm, so a new variant breaks the build and forces a decision rather than silently becoming a non-incident. `MediaError` has **9** variants at `c3a5814` (`media.rs:1690-1755`) and **11** after §4.1; the function returns `Some` for `SourceColorForAsset` and `None` for all **ten** others. **Erratum (implementation, 2026-09-15):** `Some` for `SourceColorForAsset` except when the carried refusal is `UnknownWhitePoint`, which §2.2 rule 6 makes a non-incident and therefore also returns `None`; tested. **Erratum (IN1b-R9, 2026-09-15):** that implementation erratum is **reversed** by [IN1b](IN1B-ERROR-MIGRATION.md) §0.2/d — `from_media_error` returns `Some` for all thirteen refusals, `UnknownWhitePoint` included (IN1b §3.2 rule 16). The rule's "returns `Some` for `SourceColorForAsset` and `None` for all **ten** others" also becomes false: after IN1b §3.9 every `MediaError` variant has a code and the function is **total**, returning `IncidentObservation` rather than `Option` (IN1b §5.1 rule 12).
23. `MediaError::SourceColor` maps to `None` in Part A, with the reason as a comment: an incident needs a subject, and the layer that supplies one is `contextual_managed_decode_error` (`crates/kinewright-media/src/render.rs:679-708`), which turns every `SourceColor` into a `SourceColorForAsset` before it leaves the crate (§4.2 rule 6). The arm exists so the exhaustive match is honest, not because the path is reachable.
24. Every string field comes from a core accessor and **must not** be built by the app: `observed` from `ColorSourceError::observed()` (`color.rs:277-301`), `allowed` from `allowed_values()` (`color.rs:304-325`). The app formats nothing, which is what makes "built from the typed value, never by parsing the string" checkable rather than aspirational.
25. **`field` and `class` are assigned at exactly one site each.** `IncidentLog::observe` computes `field: observation.code.field()` and `class: policy_class(observation.code, probed)` when it opens an entry, and a dedup hit recomputes neither. Drift is therefore impossible by construction; a core test asserts `incident.field == incident.code.field()` over all twelve codes anyway, so the day the assignment moves, the test moves with it. **Erratum (IN1b-R14, 2026-09-15):** "a core test asserts `incident.field == incident.code.field()` over all **twelve** codes" becomes over all **67** ([IN1b](IN1B-ERROR-MIGRATION.md) §3.2 rule 11, §7 item 6). The rule's own property — `field` and `class` assigned at exactly one site each — is unchanged and IN1b §3.5 rule 27 keeps it.
26. **Severity is data, not judgement.** `POLICY`'s `severity` column is the single source, and every Part A row is `IncidentSeverity::Blocks`, because a source-colour refusal stops the managed decode and no frame appears. `Degrades` and `Informs` are declared for Part B and IN3 and have no Part A row; a test **must** assert that every Part A row is `Blocks`, so the day one is not, the change is visible in a diff. **Erratum (IN1b-R1a, 2026-09-15):** the Part A test "every row is `Blocks`" becomes **"every colour row blocks and every row's severity is the one it declares"** over the sixty-seven-row table ([IN1b](IN1B-ERROR-MIGRATION.md) §3.6 rule 29), and `Degrades` does gain Part B rows. This rule's own reason — a source-colour refusal stops the managed decode — is unchanged and is why the colour half survives.
27. `IncidentLog::observe` **must** compute `opened_at` as `self.started.elapsed()` at the moment of a **miss** and **must not** recompute it on a dedup hit: `opened_at` is when the problem was first seen, not when it was last seen.

### 2.3c What `incident.rs` is not

28. It is **not** a log of everything that went wrong. An error with no `IncidentCode` is not an incident; it stays a `record_error` line (§1 item 6). **Erratum (IN1b-R2, 2026-09-15):** "An error with no `IncidentCode` is not an incident; it stays a `record_error` line" is superseded — after [IN1b](IN1B-ERROR-MIGRATION.md) §5.3 rule 23 there is no `record_error` line to stay as, because all 129 measured paths and the three log-bypassing sinks carry a code.
29. It is **not** a UI model. `IncidentCardView` lives in the app (§5.3) and core never learns what a card looks like.
30. It is **not** persisted (§1 item 3, §13 D2). The durable residue of a resolved incident is the written `color_description` and `MediaAsset.assumed_from`. **Erratum (E-B4, 2026-09-22):** "not persisted" becomes: persisted only as a sidecar record, never as a `Document` field — Ctrl+Z still cannot un-resolve an incident and branches still cannot fork the log ([IN2B](IN2B-INCIDENT-PERSISTENCE.md) §0.5 E-B4, §2).
31. It **must not** hold a `Document`, an `Arc<Document>`, or any view derived from one. An incident names its subject and carries its own evidence; a consumer that needs the document asks the actor for it. This is what lets `get_incidents` answer without a snapshot and what keeps `Incident` cheap to serialize (§6.2 rule 9). **Erratum (E-B4, 2026-09-22):** holds by construction under Part B — the observed name is captured by the app router and passed as an evidence string, in the same class as `observed`/`allowed` ([IN2B](IN2B-INCIDENT-PERSISTENCE.md) §0.5 E-B4, §9).

### 2.4 The policy table

32. Incidents **must** be raised from the decoder's *post-assumption* error — the `ColorSourceError` that travelled out of `classify_source_with_assumption` on the managed decode path — and **must not** be raised by re-running the bare classifier on an asset. Re-running the bare classifier would report `unknown_source_white_point` for every correctly tagged source (probe-1 §P2.4 result 2; probe-2 T3.2 reproduced it on both tagged twins).
33. `PolicyClass` is `AutoApply | AskFirst | Explain`, `#[serde(rename_all = "snake_case")]`. `AskFirst` has **no** entry in Part A's table; it is declared because §2.5's `RecoveryAction` contract and §9 clause 6's S8 assertion are written over both classes and Part B/IN3 will use it.
34. `POLICY` is a core `const` array, one row per `IncidentCode`, in the order of §2.2's table:

```rust
pub struct PolicyEntry {
    pub code: IncidentCode,
    pub class: PolicyClass,
    pub predicate: PolicyPredicate,
    pub severity: IncidentSeverity,
}
pub enum PolicyPredicate { Always, Rec709Compatible }
pub const POLICY: [PolicyEntry; 12] = [ /* §2.2's twelve rows */ ];
```

35. A core test **must** prove exhaustiveness by an explicit `match` on `IncidentCode` (through `SourceColorIncident`) with **no** wildcard arm, asserting that each variant appears in `POLICY` exactly once and that `POLICY.len() == 12`. A `HashSet`-of-codes test is not sufficient: the compile-time match is what fails when a code is added and the table is not. **Erratum (IN1b-R15, 2026-09-15):** "**`POLICY.len() == 12`**" becomes `POLICY.len() == 67` ([IN1b](IN1B-ERROR-MIGRATION.md) §3.2 rule 12). This is the second copy of §9 clause 1's assertion and takes its own erratum so neither is missed.
36. `pub fn policy_class(code: IncidentCode, probed: &ColorDescription) -> PolicyClass` resolves the declared class through the predicate: `Always` returns the declared class; `Rec709Compatible` returns the declared class when `rec709_compatible(probed)` is true and `PolicyClass::Explain` otherwise.
37. **`rec709_compatible`, normative** (N2/B7, as amended by §0.3 D1). The predicate is **defined** by two properties of the recovery, both provable:

> `rec709_compatible(probed)` is true exactly when
> **(a)** `recovery_description(probed)` leaves every **known** field of `probed` — primaries, transfer, matrix, range, white point, bit depth — unchanged, filling only the `Unknown` ones, **and**
> **(b)** `classify_source_with_assumption(&recovery_description(probed), None) == Ok(ColorSourceProfile::Rec709Video)`.

   It is **implemented** as the explicit chain below, and one core test **must** assert definition ≡ implementation over a constructed cross product of every named variant of all six tag enums plus `Other("x")` in each and `ColorBitDepth::Integer(n)` for `n ∈ {1, 7, 8, 16, 17}`:

```rust
#[must_use]
pub fn rec709_compatible(probed: &ColorDescription) -> bool {
    matches!(probed.primaries, ColorPrimaries::Unknown | ColorPrimaries::Bt709)
        && matches!(probed.transfer, ColorTransfer::Unknown | ColorTransfer::Bt709)
        && matches!(probed.matrix, ColorMatrix::Unknown | ColorMatrix::Bt709)
        && matches!(
            probed.range,
            ColorRange::Unknown | ColorRange::Limited | ColorRange::Full
        )
        && matches!(
            probed.white_point,
            ColorWhitePoint::Unknown | ColorWhitePoint::D65
        )
        && match &probed.bit_depth {
            ColorBitDepth::Unknown => true,
            value => value.integer_bits().is_some_and(|bits| (8..=16).contains(&bits)),
        }
}
```

38. **The rationale, rewritten** (N2/B7). Two different facts put a known value on the `Explain` side, and the contract says which is which rather than asserting one reason for all six fields.
    - **Primaries, transfer, matrix and white point fall to `Explain` when known and not Rec.709 because the recovery would have to *overwrite* them.** The shipped builder writes `Bt709` into primaries, transfer and matrix and `D65` into white point **unconditionally** (`crates/kinewright-app/src/color_ui.rs:129-133`). Rewriting a known `Bt1886` transfer or a known `Rgb` matrix to `Bt709` would change what the pixels declare themselves to mean — the exact silent behaviour CC1 §1 forbids — even though the result would classify. This is why the chain admits only `Unknown | Bt709` for the three tags, and `Unknown | D65` for the white point, rather than the wider sets the profile itself allows.
    - **Range and bit depth fall to `Explain` only when the recovery cannot make them classify.** The builder *preserves* a known range and a known depth (`color_ui.rs:118-125`), and `docs/CC1-MANAGED-SDR-PRIMARY.md:66` declares `rec709_video` as range `limited` **or** `full` with integer depth **8..=16**, confirmed in code at `color.rs:697` and `:759-765`. A known `Full` range and a known `Ten` depth therefore survive the recovery untouched and classify — so both are admitted. Only `ColorRange::Other(_)`, a non-integer depth and an integer depth outside `8..=16` are refused, and they are refused because the classifier refuses them.
    - **`ColorPrimaries::Srgb` is `Explain` in IN1, deliberately** (N2/S7). CC1 allows it: `allowed_values()` for a primaries failure is `"bt709 or srgb in a supported CC1 profile"` (`color.rs:307-309`), `validate_source_fields` accepts `Srgb | Bt709` (`color.rs:724-728`), and `srgb_full` is the second managed profile (`color.rs:702-710`, `docs/CC1-MANAGED-SDR-PRIMARY.md:67`). A source whose known tags point at `srgb_full` is `Explain` **because the only recovery IN1 ships targets `rec709_video`**; an `assume_srgb_full` recovery is deferred to IN3 with the conversions (§13 D8).
    - `Other(_)` in any tag is matched by no arm and therefore yields `false`.
39. **Nothing auto-applies except a row whose resolved class is `AutoApply`.** The router **must** call `policy_class` and **must not** consult any other predicate.
40. **The first-failure cascade, stated out loud (S2).** `validate_source_fields` (`color.rs:720-766`) returns the **first** failing field in a fixed order — primaries, transfer, matrix, range, white point, bit depth. A fully untagged source therefore reports `UnknownPrimaries` and nothing else; a partial fix would flip the code to `unknown_source_transfer`, a **different** dedup key and therefore a new incident.
41. The cascade does not occur on the `AutoApply` path because the recovery builder (§4.5) fixes **all six** fields in one operation. **IN1 must not offer a single-field recovery**, and `RecoveryKind::Operation` **must** carry a whole `ColorDescription`, never a field patch.

### 2.5 `RecoveryAction`

42. The two types, in full:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecoveryAction {
    /// Button text. `&'static str` so a recovery label cannot embed a path, a
    /// file name or a measured number and therefore cannot drift per fixture.
    pub label: &'static str,
    pub kind: RecoveryKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryKind {
    /// An exact operation the actor may send through `Command::Do*`.
    Operation(Operation),
    /// Plain language and nothing to press. The text is a core `&'static str`,
    /// never a sentence the app composed.
    Explain(&'static str),
}
```

   **`RecoveryKind` is externally tagged — serde's default — and that is the ruling N2/B4 asked the reviser to make.** The draft's `#[serde(tag = "kind")]` was an internally-tagged enum with a `&'static str` newtype variant, which compiles and then fails at run time with `"cannot serialize tagged newtype variant RecoveryKind::Explain containing a string"` (probe-2 T2.4, verified in a standalone crate) — and **nine of the twelve** Part A codes are `Explain` rows. Of the two forms N2 offered, external tagging is the smaller: `{"operation":{…}}` costs **14 B** of wrapper against adjacent tagging's `{"kind":"operation","value":{…}}` at **29 B**, so external tagging saves **15 B per recovery**. The field is still named `kind`, so a recovery renders as `{"label":"…","kind":{"operation":{…}}}` — one unambiguous key, no collision with the parent field name.
43. `IncidentState` keeps adjacent tagging (`tag = "state", content = "outcome"`) and is held in `Incident::state` with `#[serde(flatten)]`, so it renders as the two top-level keys `"state":"resolved","outcome":"applied"` — and as the single key `"state":"open"` while the incident is open. Adjacent tagging serialises as a map, which is what `flatten` requires.
44. `RecoveryKind` declares **no** `Capability`, `Relink` or `Transcode` variant in IN1: nothing would construct them and §1 item 7 forbids executing them. IN3 adds them (§13 D7).
45. `pub fn policy_recovery(code: IncidentCode, subject: IncidentSubject, evidence: &IncidentEvidence) -> Vec<RecoveryAction>` is the **single** producer of recoveries. The router, the card and `get_incidents` **must** all read it; none of them may build a `RecoveryAction` itself. This is what makes §9 clause 6's equality assertion meaningful.
46. For a resolved `AutoApply` row, `policy_recovery` returns exactly one action with `label: "Assume Rec.709 for this source"` and `kind: Operation(assume_rec709_operation(asset, probed))` (§4.5). For an `Explain` row it returns exactly one action with `label: "How to fix this"` and `kind: Explain(ColorSourceError::recovery_action())` — the core string, never restated (`color.rs:329-331`).
47. **`recovery_action()` returns the same literal for all thirteen variants** (`color.rs:327-331`: a `const fn` with no `match`), so all nine `Explain` rows carry the identical body, *"Apply an explicit supported source-colour override or relink to compatible media."* — advice that names relink, which §1 item 7 says IN1 cannot do. This is stated as fact so nobody later discovers it as a bug (N2/S8). The discrimination between codes is carried by §5.3 rule 23's twelve headlines, which are distinct by construction; per-code `Explain` bodies arrive with IN3's conversions (§13 D8).

---

## 3. Fixtures

All four fixtures are generated at test time by `crates/kinewright-media/src/in1_sources.rs`, a `pub` module beside `cc7_sources` under the existing `#[cfg(any(test, feature = "test-util"))]` gate, calling `crate::test_support::GeneratedMedia::ffmpeg` (`crates/kinewright-media/src/test_support.rs:54-57`). No fixture bytes are committed.

1. **Geometry, normative**, reusing CC7/AU6 so the fixture costs nothing new (nit 6):

```text
IN1_SOURCE_FPS    = 25      IN1_SOURCE_WIDTH  = 320
IN1_SOURCE_FRAMES = 50      IN1_SOURCE_HEIGHT = 180
IN1_SOURCE_PIXEL_FORMAT = "yuv420p"          (2.0 s at 25 fps)
```

2. The picture is `testsrc`; no IN1 claim reads a pixel value, so the raster content is not pinned.
3. The four fixtures:

| function | file | codec | tags | role |
| --- | --- | --- | --- | --- |
| `in1_untagged_mp4()` | `in1_untagged.mp4` | `libx264` | stripped | **the both-OS CI gate** |
| `in1_tagged_mp4()` | `in1_tagged.mp4` | `libx264` | `setparams` BT.709 | proves the negative |
| `in1_untagged_webm()` | `in1_untagged_vp9.webm` | `libvpx-vp9` | stripped | the Helen Hill row |
| `in1_tagged_webm()` | `in1_tagged_vp9.webm` | `libvpx-vp9` | `setparams` BT.709 | proves the negative |

4. **The tagging recipe is `setparams=`, and the reason is written beside it.** On the pinned FFmpeg 8 (`third_party/ffmpeg/bin/ffmpeg`, `n8.0-23-gd1f31a829d-20251022`, `libavcodec 62.11.100`) the *output-option* form `-color_primaries bt709 -color_trc bt709` is **silently ignored** and ffprobe reports `unknown` (probe-1 §P2.2, number 18). The tagged twins **must** use the filter in `cc7_sources`' spelling (`crates/kinewright-media/src/cc7_sources.rs:540`), which is `range=limited`, not `range=tv` (nit 2):

```text
-vf setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709
```

   The untagged pair **must** pass `-color_primaries unspecified -color_trc unspecified -colorspace unspecified -color_range unspecified` and **must not** pass a `setparams` filter.
5. **The pinned probed tuples**, measured by probe-1 §P2.4 and reproduced **byte for byte, all eight fields of all four rows, no mismatch** by probe-2 §T3.2 through `crate::decode::probe_path`:

| fixture | primaries | transfer | matrix | range | white point | depth | confidence bp | provenance |
| --- | --- | --- | --- | --- | --- | --- | ---: | --- |
| `in1_untagged.mp4` | Unknown | Unknown | Unknown | **Unknown** | Unknown | Eight | **2 000** | **Inferred** |
| `in1_tagged.mp4` | Bt709 | Bt709 | Bt709 | Limited | **Unknown** | Eight | 10 000 | StreamMetadata |
| `in1_untagged_vp9.webm` | Unknown | Unknown | Unknown | **Limited** | Unknown | Eight | **4 000** | **StreamMetadata** |
| `in1_tagged_vp9.webm` | Bt709 | Bt709 | Bt709 | Limited | **Unknown** | Eight | 10 000 | StreamMetadata |

6. The two constants that differ are named, not inlined: `IN1_UNTAGGED_MP4_CONFIDENCE_BASIS_POINTS = 2_000` and `IN1_UNTAGGED_WEBM_CONFIDENCE_BASIS_POINTS = 4_000`. **The untagged WebM reports `range=Limited` even though `unspecified` was requested, because the Matroska/WebM *muxer* always writes a `Colour/Range` element: a Matroska-family container is never fully untagged, whatever the codec.** That sentence **must** appear as a doc comment on the WebM constant. It replaces the draft's "the VP9 bitstream header always carries a range bit", which probe-2 §T4.4 disproved: `libx264` muxed into `.mkv`, with all four colour output options `unspecified` and no libvpx anywhere, produces a `Colour` element (`0x55B0` at byte 379) with a `Range` child (`0x55B9` at byte 382) and probes the Helen Hill tuple exactly — `Unknown/Unknown/Unknown/Limited/Unknown/Eight/4 000/StreamMetadata`. MP4 does not: even an explicit `setparams=range=tv` does not survive into an MP4 `colr` box on this build.
7. Both untagged fixtures **must** classify `Err` with code `unknown_source_primaries`, field `primaries`, observed `unknown`, allowed `"bt709 or srgb in a supported CC1 profile"`, and both tagged twins **must** classify `Ok(ColorSourceProfile::Rec709Video)` **only through the D65 assumption path** the decoder already uses (`render.rs:760-767`); a bare `classify_source` on a tagged twin returns `Err(UnknownWhitePoint)` and a fixture **must** assert that too, because it is the fact behind §2.2 rule 6. Probe-2 §R9 discharged this through the real `VideoDecoder::open_scaled_managed` call `Renderer::decode_video_frame` makes (`render.rs:504-512`), not through a throwaway classifier call: `Ok` for both tagged twins, `Err(… unknown_source_primaries …)` for both untagged.
8. After the router's auto-apply, **both** untagged assets **must** read `Bt709 / Bt709 / Bt709 / Limited / D65 / Eight / 10 000 / AgentAssumption`, identical in every field. The card headline is identical for both containers by construction (§5.3 rule 21).
9. **The MP4 fixture is the CI gate on both operating systems.** Every §9 clause that names a fixture without qualification names `in1_untagged.mp4`.
10. **The WebM fixture is a second gate on both operating systems.** The conditional the draft carried is resolved and its fallback branch is **deleted** (N2.5). Probe-2 §T4.2 determined definitively that the pinned Windows package — System233 `ffmpeg-8.0.1-r3_x64-windows-shared-gpl`, fetched by `scripts/setup-ffmpeg.ps1:12` from `.github/workflows/ci.yml:42` — compiles `libvpx-vp9` as an **encoder**: `FFMPEG_CONFIGURATION` in `bin/ffmpeg.exe` contains `--enable-libvpx`, and `bin/avcodec-62.dll` contains the strings `libvpx-vp9 encoder`, `libvpxenc.c` and the encoder's option names. The WebM tests are therefore **ordinary tests on both operating systems**, with no per-OS constant, no runtime skip and no "fail by name with the missing encoder" branch. Probe-2 §T4.4 also found a libvpx-free authoring route (`libx264 → .mkv`) that reproduces the same tuple, recorded here as the standby if the Windows package is ever re-pinned.
11. Fixtures are interned per process in the shape `cc7_sources` uses (`cc7_sources.rs:503-515`): one `OnceLock<Mutex<HashMap<…>>>` of encoded bytes, re-written to a fresh `GeneratedMedia` per call so the `Drop` guard still removes every temp file (`test_support.rs:80-84`).
12. **There is no `IN1_FIXTURE_DEDUP_COUNT`.** The draft's constant is deleted (N2/B8) because its mechanism was wrong and its value is not a property of the fixture. Probe-2 §R5 measured the truth and the contract states it instead:
    - Every colour refusal leaves the engine through **one** site, `self.emit(MediaEvent::Error(error))` at `crates/kinewright-media/src/engine.rs:2050` inside `Worker::fail`, reached from `Worker::present`'s `Err` arm (`engine.rs:1992`). What differs is the caller: `Worker::set_document`'s `present` (`engine.rs:1857`) and `handle_coalesced_requests`' `present` (`engine.rs:1876`), with `start_playback`'s (`engine.rs:1910`) added when `play` is called.
    - `engine.rs:1920` is the **audio-pause** site inside `Worker::pause` and carries no colour refusal. The draft's and N1.5/B3's "two emit sites" statement is corrected here.
    - The count is therefore **`1 + n`** for `n` frame requests: measured **2** for `set_document` plus one `request_frame`, **3** with `play`, **4** for three requests. The failed `open_scaled_managed` is not cached (`render.rs:501-514` inserts only on success), so a live app re-fails on every repaint.
    - What the contract pins is the **deterministic** consequence: **exactly one incident is open for the fixture however many errors arrived**, with `count >= 1` (§9 clause 4), and a core test feeding two identical `IncidentObservation`s into `IncidentLog::observe` asserts the second returns the same `IncidentId` with `count == 2` and an **unchanged** `opened_at` (§2.3 rules 15 and 27).
13. **Required media fixtures**, in `crates/kinewright-media/src/in1_fixtures.rs`. Each is an ordinary `#[test]`; none needs a window system, a network or an audio device.

| # | name | asserts |
| ---: | --- | --- |
| 1 | `in1_untagged_mp4_probes_the_pinned_tuple` | §3 rule 5 row 1, field by field, including `IN1_UNTAGGED_MP4_CONFIDENCE_BASIS_POINTS` and `ColorProvenance::Inferred` |
| 2 | `in1_untagged_webm_probes_the_pinned_tuple` | §3 rule 5 row 3, including `range == Limited` and `ColorProvenance::StreamMetadata` |
| 3 | `in1_tagged_twins_probe_bt709_with_an_unknown_white_point` | §3 rule 5 rows 2 and 4, and that `white_point` is `Unknown` on both |
| 4 | `in1_both_untagged_sources_refuse_with_the_same_code` | both yield `unknown_source_primaries` / `primaries` / `unknown` while differing in three of eight description fields |
| 5 | `in1_tagged_twins_classify_only_through_the_d65_assumption` | bare `classify_source` gives `Err(UnknownWhitePoint)`; `classify_source_with_assumption(_, Some(D65))` gives `Ok(Rec709Video)` |
| 6 | `in1_the_untagged_managed_decode_carries_the_typed_refusal` | the managed decode of `in1_untagged.mp4` fails with `MediaError::SourceColorForAsset`, `recovery_code() == Some("unknown_source_primaries")` |
| 7 | `in1_the_refusal_message_is_byte_identical_to_c3a5814` | §9 clause 11's template |
| 8 | `in1_the_tagged_mp4_decodes_managed` | the negative; no error |
| 9 | `in1_the_assumed_description_decodes_managed` | after `assume_rec709_operation`, the same decode succeeds — the proof that the recovery actually recovers |

14. Fixture 9 is the one that makes the whole slice honest: without it, the contract would prove that an incident was opened, classified and marked resolved, and would never prove that the frame appears. It **must not** be cut.
15. `in1_sources.rs` **must** be `pub` rather than `#[cfg(test)]`, for the reason `cc7_sources` records in its module doc (`cc7_sources.rs:1-9`): the agent's `tests/mcp_server.rs` needs it across a crate boundary, and a `cfg(test)` module is invisible there.
16. **Cost, measured** (probe-2 §T3.3, Linux default lane): `in1_untagged.mp4` 0.036 s / 9 585 B, `in1_tagged.mp4` 0.025 s / 9 770 B, `in1_untagged_vp9.webm` 0.249 s / 26 707 B, `in1_tagged_vp9.webm` 0.262 s / 26 587 B — **0.572 s and 72 649 B for the set**, of which the WebM pair is 0.511 s (89 %) and 53 294 B (73 %). Windows was not measurable from the probe machine (§11.1 limit 4).

---

## 4. Core, media and app changes, each with its test

### 4.1 `MediaError` gains two typed variants

1. On `MediaError` (`crates/kinewright-core/src/media.rs:1690-1755`), in the shape of the existing `ColorQc` and `DeliveryColor` arms:

```rust
/// CC1's typed source rejection, carried intact out of the decoder.
///
/// Its `Display` is never observed on the production path: every
/// `SourceColor` becomes a `SourceColorForAsset` before it leaves
/// `kinewright-media` (§4.2 rule 6), so this text may be short.
#[error("managed source profile rejected: {0}")]
SourceColor(#[from] crate::ColorSourceError),

/// The same refusal with the asset, the path, the probed description and the
/// assumption `contextual_managed_decode_error` exists to add.
SourceColorForAsset {
    asset: AssetId,
    path: PathBuf,
    error: crate::ColorSourceError,
    /// The exact probed description at the moment of refusal. The incident's
    /// `evidence.probed` is this value; the revert restores these bytes.
    description: ColorDescription,
    assumption: Option<ColorSourceProfileAssumption>,
},
```

   **Erratum (implementation, 2026-09-15):** the variant's five fields as declared make `MediaError` 312 bytes, over clippy's 128-byte `result_large_err` threshold at some 300 `Result<_, MediaError>` signatures. The fields are unchanged but live in `pub struct SourceColorRefusal { asset, path, error, description, assumption }`, and the variant is `SourceColorForAsset(Box<SourceColorRefusal>)` with `#[error("{0}")]`; the struct carries the template above verbatim, so the rendered text, the codes and every test are unchanged. `matches!(…, MediaError::SourceColorForAsset { .. })` reads `SourceColorForAsset(_)` wherever the contract writes it.

2. **`SourceColorForAsset`'s `#[error(...)]` carries the entire historical sentence** (N2/B1). Today that sentence is built by two nested formats: `decode.rs:1081-1086` produces `MediaError::Backend("managed source profile rejected for {path} (assumption={assumption:?}): {error}")`, and `render.rs:704-707`'s `error =>` catch-all renders *that* into `"managed decode for asset {asset} ({path}) failed: {error} [{status}, assumption=…, description=…]. Recovery: …"`, where `{error}` is a `MediaError` whose `Backend` `Display` is `#[error("media backend error: {0}")]` (`media.rs:1753`). The shipped string therefore contains **two** `media backend error: ` fragments and the path **twice**. The attribute must reproduce all of it:

```rust
#[error(
    "media backend error: managed decode for asset {asset} ({}) failed: \
media backend error: managed source profile rejected for {} (assumption={assumption:?}): \
{error} [source_color={}, field={}, observed={}, allowed={}, recovery={}, \
assumption={assumption:?}, description={description:?}]. Recovery: apply an explicit \
supported source-colour override, transcode to a supported integer format, or relink to \
compatible media.",
    path.display(),
    path.display(),
    error.code(),
    error.field(),
    error.observed(),
    error.allowed_values(),
    error.recovery_action()
)]
```

   `{status}` is **derived from the carried `ColorSourceError` accessors as trailing format arguments**, not by re-running the classifier: `managed_source_color_status` (`render.rs:662-677`) formats exactly `source_color={code}, field={field}, observed={observed}, allowed={allowed}, recovery={recovery}` from the same five accessors. thiserror permits named inline captures and trailing positional arguments in the same attribute, and `error.code()`, `field()`, `allowed_values()` and `recovery_action()` are all `const fn` (`color.rs:244`, `:264`, `:305`, `:329`) while `observed()` returns a `String` (`color.rs:278`).
3. **The leading `media backend error: ` is vestigial and is kept on purpose.** It is the old `Backend` wrapper's own prefix. Keeping it is the whole point of N1/B3: a log reader, a screenshot and a prose assertion must see no change on the day the type changes underneath them. Dropping it is a visible-text change and belongs with Part B's log demotion (§13 D16).
4. **The phrase "a hand-written `Display`" is deleted** from this contract. `MediaError` is `#[derive(Error)]` (`media.rs:1690`) and thiserror generates the `Display` impl for the **whole enum**; a hand-written `Display` for one variant is not a thing that exists. The entire sentence lives in the attribute.
5. `MediaError::recovery_code()` (`media.rs:1760-1772`) gains two arms delegating to `ColorSourceError::code()`: `Self::SourceColor(error) => Some(error.code())` and `Self::SourceColorForAsset { error, .. } => Some(error.code())`. `recovery_code()` is `const fn` and `ColorSourceError::code()` is also `const fn` (`color.rs:243-244`), in the same shape as the existing `Self::DeliveryColor(error) => Some(error.code())` arm (`media.rs:1763`). A core test **must** assert both arms for all thirteen `ColorSourceError` variants.
6. `#[from]` on `SourceColor` plus the sibling struct variant compiles: `#[from]` generates one `From<ColorSourceError> for MediaError`, there is only one such variant, and the sibling's `error: ColorSourceError` field is unannotated.
7. `MediaError` keeps `Backend(String)`. IN1 removes no variant. `MediaError` goes from **9** variants to **11**.

### 4.2 The two construction sites

8. `crates/kinewright-media/src/decode.rs:1081-1086` (`VideoDecoder::open_scaled_managed`) **must** stop mapping the classifier `Err` into `MediaError::Backend(format!(…))` and **must** construct `MediaError::SourceColor(error)` instead — one line, `.map_err(MediaError::SourceColor)?`. The contextual sentence it used to build is rebuilt by rule 2's attribute, so no caller sees a different string.
9. `crates/kinewright-media/src/render.rs:679-708` (`contextual_managed_decode_error`) gains a match arm, placed **before** the `error =>` catch-all at `:704-707`:

```rust
MediaError::SourceColor(error) => MediaError::SourceColorForAsset {
    asset,
    path: path.to_path_buf(),
    error,
    description: description.clone(),
    assumption,
},
```

   The existing `MediaError::UnsupportedDecoderFormat { .. }` arm (`render.rs:688-703`) and the catch-all are unchanged. Carrying `description` and `assumption` on the variant is what lets the app build the incident without re-probing and without parsing (§5.2 rule 5), and what lets rule 2's attribute render `[{status}, assumption=…, description=…]` from the variant alone.
10. **The one string-shaped test that reads a `MediaError` is amended, not deleted.** `managed_decode_error_names_asset_field_observed_allowed_and_recovery` (`crates/kinewright-media/src/render.rs:1328-1351`) keeps every existing `message.contains(...)` assertion and **gains** `assert_eq!(error.recovery_code(), Some("unsupported_source_primaries"))` and a `matches!(error, MediaError::SourceColorForAsset { .. })`. **Its input must change** (nit 5): today it passes `MediaError::Backend("managed source profile rejected")` (`render.rs:1334`), which after rule 9 still lands in the `error =>` catch-all and still produces a `Backend`, so the two new assertions would fail. The input becomes `MediaError::SourceColor(ColorSourceError::UnsupportedPrimaries(ColorPrimaries::Bt2020))`. Its two siblings at `render.rs:1354-1377` and `:1379-1394` stay unchanged.
11. `MediaError::SourceColor` **must not** be constructed anywhere except `decode.rs`; `SourceColorForAsset` **must not** be constructed anywhere except `render.rs`. A media test greps the crate for both constructor names and asserts the site counts (1 and 1) in the shape AU6's inventory arrays use.

### 4.3 `ColorProvenance::AgentAssumption`

12. `color_tag!`'s `ColorProvenance` block (`crates/kinewright-core/src/color.rs:142-153`) gains one line: `AgentAssumption => "agent_assumption"`, after `ApplicationDefault`. The macro (`color.rs:18-65`) derives the wire string, the `Other(String)` fallback, `Serialize` and `Deserialize`; no other change is needed for the file format.
13. **It costs zero registry bytes, measured.** `color_tag!`'s `JsonSchema` impl emits a plain `string` with no enumerated variant list, so probe-2 §T1.4 measured the new provenance at **0 / 0 / 0** across the registry sextuple. The contract states that as a measured fact rather than a hope.
14. **Confidence stays `COLOR_CONFIDENCE_MAX_BASIS_POINTS`** (`color.rs:16`, 10 000). The tuple is asserted with full confidence; what is uncertain is *who* asserted it, and the provenance is the whole signal. IN1 **must not** introduce a second confidence value, an `assumed: bool` field, or any arithmetic threshold on `confidence_basis_points`: nothing in the tree reads it as a threshold today, and a new `ColorDescription` field would change the serialized shape of every project file for no consumer.
15. **Neither managed-colour allowlist is touched** (N2/B9, reversing N1/S3 for Part A). `color_description_matches_managed` (`color.rs:946-962`, the `matches!` at `:952-955`) is called from exactly three places — `color.rs:915`, `:922`, `:929` — and all three pass a `ColorContext` description (`working`, `monitoring`, `delivery`), never a `MediaAsset::color_description`. `delivery_color_mismatches` (`delivery.rs:622-631`) is likewise called on the project's **delivery** description. Neither is on the source-asset path, so widening them would be dead code in Part A, would silently permit a future caller to stamp a project's delivery description `AgentAssumption`, and would break two green tests that assert the literal `"application_default or user_override"` (`crates/kinewright-core/src/delivery.rs:2211` and `crates/kinewright-media/src/export.rs:2677`). The `allowed` phrase **does not change**.
16. **The test that does bear on IN1** replaces the draft's two. One core test **must** assert that `classify_source_with_assumption` returns `Ok(ColorSourceProfile::Rec709Video)` for the post-recovery tuple under provenance `AgentAssumption`, `UserOverride` and `StreamMetadata` alike. That is the real statement: `classify_source_with_assumption` (`color.rs:665-718`) **never reads `provenance`** — it checks the six tag fields and the profile match and nothing else — so **provenance does not gate the decode**, and an assumed Rec.709 decodes exactly as a user override does without any gate being widened.
17. The CC6 QC report surfaces the provenance on the source line as evidence only; IN1 adds no QC gate on it.

### 4.4 `MediaAsset.assumed_from`, the widened guard, and the `AddAsset` refusal

18. `MediaAsset` (`crates/kinewright-core/src/model.rs:121-145`) gains one field after `color_description` (`model.rs:144`):

```rust
/// The description an agent assumption replaced, when one did.
///
/// Set by the recovery, never supplied: written by a
/// `SetAssetColorDescription` whose provenance is `AgentAssumption`, cleared
/// by the revert that restores it, and refused outright on `AddAsset`
/// (rule 24). Absent in every project written before IN1 and omitted when
/// `None`, so no existing project file changes byte.
#[serde(default, skip_serializing_if = "Option::is_none")]
#[schemars(default)]
pub assumed_from: Option<ColorDescription>,
```

   The attribute pair matches `source_fingerprint`'s shape (`model.rs:134-136`). Probe-2 §T7 measured the omission: `crates/kinewright-core/tests/fixtures/pre_m13_project.json` round-trips to **1 215 B** with FNV-1a 64 **`ff6c17d72643a88b`**, identical at `c3a5814` and with the field, and `"assumed_from"` absent. **Erratum (implementation, 2026-09-15):** the length reproduces exactly (1 215 B compact, 1 891 B pretty) but the digest does not; standard FNV-1a 64 (offset basis `0xcbf2_9ce4_8422_2325`, prime `0x100_0000_01b3`) over the compact `serde_json::to_vec` bytes is **`c9da3186e131e4fd`**, which the test pins. Probe-2's digest came from an unrecorded spelling of the hash. When the field *is* `Some` it serialises as an ordinary inline `ColorDescription` of **176 B**.
19. **The mechanical cost is 109 lines and is budgeted.** `MediaAsset` has no `Default`, so every struct literal must gain `assumed_from: None`. Probe-2 §T6.1 measured **109** construction sites across `cargo build --workspace --all-targets`, converging in three compile passes (32, then 46, then 31). §12 carries the row.
20. `set_asset_color_description` (`crates/kinewright-core/src/operation.rs:1639-1661`) becomes, verbatim in its guard — replacing the single-provenance check at `operation.rs:1653-1658`:

```rust
let current = &doc.media_pool[index];
let is_actor = matches!(
    color_description.provenance,
    ColorProvenance::UserOverride | ColorProvenance::AgentAssumption
);
let is_revert = current.assumed_from.as_ref() == Some(&color_description);
if !is_actor && !is_revert {
    return Err(OpError::InvalidColorOverrideProvenance {
        asset: asset_id,
        actual: color_description.provenance,
    });
}
if is_actor && color_description.confidence_basis_points == 0 {
    return Err(OpError::ZeroConfidenceColorOverride { asset: asset_id });
}
```

21. Ordering is normative: `validate_color_description` first (unchanged, `operation.rs:1649`), then the three-way admission, then the zero-confidence check. **The reorder is safe, and the contract says why with the citation** so a reviewer does not stop on it: the only existing test that reads `ZeroConfidenceColorOverride` (`crates/kinewright-core/tests/contracts.rs:1443-1454`) builds it from a `user_color_override()`, i.e. provenance `UserOverride`, so moving the check behind the admission test changes no existing expectation. Probe-2 §T6.1 confirmed it empirically: the whole core suite — **16 binaries, 0 failures** — stays green with the guard, the provenance, the field and rule 23's `validate_asset` line all applied.
22. **Zero confidence, both directions, stated as rules** (N2.5, measured in probe-2 §T6.2):
    - A revert to a zero-confidence probed description returns **`Ok(())`**, because `is_revert` is byte-equality and is ordered **before** the zero-confidence check. A probe may legitimately have produced confidence 0 (`ColorDescription::unknown()`, `color.rs:569-582`); refusing to restore the truth because the truth is uncertain would strand the asset.
    - A zero-confidence `AgentAssumption` or `UserOverride` returns **`OpError::ZeroConfidenceColorOverride`**.
23. Byte-equality in `is_revert` is over the **whole** `ColorDescription`, provenance included. A caller cannot smuggle a forged provenance through it: the only way to make `is_revert` true is to send back exactly the bytes core itself recorded. Probe-2 §T6.2 confirmed that a *different* `StreamMetadata` description after an assume is still refused.
24. The bookkeeping, immediately after the guard:

```rust
if is_revert {
    doc.media_pool[index].assumed_from = None;
} else if color_description.provenance == ColorProvenance::AgentAssumption
    && doc.media_pool[index].assumed_from.is_none()
{
    doc.media_pool[index].assumed_from = Some(doc.media_pool[index].color_description.clone());
}
doc.media_pool[index].color_description = color_description;
```

   **Erratum (implementation, 2026-09-15):** the block above is the corrected order; the rule originally tested the `AgentAssumption` arm first, which core review pass-2 B1 showed is wrong, because a write can be **both** an actor write and a revert when the recorded bytes themselves carry `AgentAssumption` provenance. In that case the assumption arm won and `assumed_from` was never cleared, leaving an absorbing state: clearing it afterwards would need a write whose bytes equal the recorded description *and* whose provenance is not `AgentAssumption`, which byte-equality forbids, so §6.3 rule 16's `reverted` verification could never succeed and its `applied` verification succeeded unconditionally. Testing `is_revert` first fixes it and preserves every other case in rule 31's list. One absorbing state of the same class survives and is accepted: `assumed_from = Some(d)` where `d` is an `AgentAssumption` description at `confidence_basis_points: 0` can never be cleared, because the only escape is a revert into `d`'s bytes and rule 21/22's normatively ordered zero-confidence guard refuses that write with `ZeroConfidenceColorOverride`. It is accepted because no operation can *write* it — `assumed_from` only ever takes the previous `color_description`, and the same guard forbids `color_description` from becoming a zero-confidence `AgentAssumption` in the first place — so it is reachable only by hand-editing a saved project file into a shape the product never emits.
25. `assumed_from` is recorded **only when it is `None`**, so a second assumption can never overwrite the first probed truth. A user override applied over an assumption leaves `assumed_from` intact, so the revert is still reachable — deliberate, and measured true in probe-2 §T6.2.
26. **`AddAsset` must reject an asset whose `assumed_from` is `Some(_)`** (N2.5's new rule). The reason is a laundering path the new field opens: `assumed_from` is now visible in the `add_asset` input schema (probe-2 §T1.4 attributes **+8 525 B** of registry growth to exactly that inlining), so an agent that supplied `assumed_from: Some(d)` on `AddAsset` and then "reverted" into `d` would set an arbitrary colour description through the guard's byte-equality case, bypassing the provenance guard entirely. `add_asset` (`operation.rs:1537-1562`) gains one check before `validate_asset`:

```rust
if asset.assumed_from.is_some() {
    return Err(OpError::AssumedFromNotSuppliable { asset: asset.id });
}
if asset.color_description.provenance == ColorProvenance::AgentAssumption {
    return Err(OpError::AssumedFromNotSuppliable { asset: asset.id });
}
```

```rust
#[error("asset {asset} assumed_from is written by the colour recovery, never supplied on add")]
AssumedFromNotSuppliable { asset: AssetId },
```

   The check lives in `add_asset`, **not** in `validate_asset`: `validate_asset` runs on load through `validate_document` (`operation.rs:4268-4289`), and a saved project with a live assumption legitimately carries the field. `OpError` therefore goes from 153 variants to **154**; `Operation` gains none. **Erratum (implementation, 2026-09-15):** `add_asset` refuses one more thing beside `assumed_from`, per core review pass-2 S1 — an asset whose `color_description.provenance` is `ColorProvenance::AgentAssumption`, returning the **same** `OpError::AssumedFromNotSuppliable`, so `OpError` stays at **154**, the count this rule itself pins two sentences above (§4.6 rule 36 pins `Operation`'s variants and `operation_tools().len()`, neither of which this choice could move). The block above shows both checks, the way rule 24's block was replaced. `color_description` is in the same input schema and §4.3 rule 12 has just added that provenance to it, so without the check an added asset could claim Kinewright assumed its colour when it did not — an attribution lie the QC report and the Media-panel line read as evidence, and the entry point to B1's absorbing state.
27. **`RelinkAsset` needs no rule, and the contract says why rather than asserting one it cannot break** (§0.3 D4). `Operation::RelinkAsset` (`operation.rs:31-38`) carries `asset: AssetId`, `candidate: RelinkCandidate` and `allow_unverified_source: bool`. `RelinkCandidate`'s six fields are `path`, `fingerprint`, `kind`, `fps`, `duration`, `resolution` (`model.rs:150-157`) — no `MediaAsset`, no `ColorDescription`, no `assumed_from` — and `relink_asset` writes exactly two of the asset's fields, `path` and `source_fingerprint` (`operation.rs:1608-1612`). A core test **must** assert the consequence directly: a relink over an asset that carries `assumed_from: Some(d)` leaves `assumed_from` byte-identical and leaves `color_description` byte-identical.
28. **The `add_asset` and `relink_asset` tool descriptions are not edited**, so served bytes and registry description bytes do not move for this rule; the field's doc comment (rule 18) carries "set by the recovery, never supplied". **Erratum (implementation, 2026-09-15):** that sentence and the rest of rule 18's explanation live in an ordinary `//` comment above the field; the schema-visible `///` line is the single sentence probe-2 measured, `The description an agent assumption replaced, when one did.` `MediaAsset` is inlined into 55 registry schemas, so the full comment would have cost 55 × 329 B = 18 095 B of registry an agent pays on every `get_capability("add_asset")`, and would have put §6.6 rule 27's pin out of reach of any faithful implementation.
29. `OpError::InvalidColorOverrideProvenance`'s message (`operation.rs:741`) becomes `"asset {asset} color override requires user_override or agent_assumption provenance, got {actual:?}"`. **The change is free, measured:** probe-2 §R10 greps all targets including doc comments and finds exactly three sites — the declaration (`operation.rs:741-745`), the one construction site (`operation.rs:1654`) and one variant-matching test (`crates/kinewright-core/tests/contracts.rs:1455-1471`) — and **no** test reads the `Display` text.
30. `validate_asset` (`operation.rs:3746-3762`) gains one line: when `assumed_from` is `Some`, it **must** run `validate_color_description` on it too, guarding the **programmatic** path — the same reason the existing `validate_color_description(&asset.color_description)` line exists. It is not the line that catches a hand-edited project file; see §9 clause 10 and probe-2 §T6.3. It carries its own unit test that builds a `Document` in memory with an out-of-range `assumed_from` and asserts `OpError::ColorConfidenceOutOfRange`.
31. Tests, each named and each required:
    - every provenance other than `UserOverride` and `AgentAssumption` — `Unknown`, `ContainerMetadata`, `StreamMetadata`, `SidecarMetadata`, `Inferred`, `ApplicationDefault`, `Other("agent_assumption")`, `Other("anything")` — is refused **by name** with `OpError::InvalidColorOverrideProvenance`, listed one arm per variant so adding a provenance breaks the test;
    - an `AgentAssumption` set records the exact previous description in `assumed_from`;
    - a second `AgentAssumption` set does not overwrite it;
    - sending `assumed_from`'s bytes back clears the field and restores the description byte-identically, with the original provenance, at confidence 2 000 and at 4 000;
    - sending a *different* `StreamMetadata` description is still refused;
    - a user override applied over an assumption leaves `assumed_from` intact;
    - a zero-confidence probed description round-trips through the revert (`Ok(())`);
    - a zero-confidence `AgentAssumption` is still refused with `ZeroConfidenceColorOverride`;
    - `AddAsset` with `assumed_from: Some(_)` is refused with `AssumedFromNotSuppliable`, and with `None` is accepted;
    - `in1_a_revert_into_an_agent_assumption_still_clears_assumed_from` — rule 24's erratum: B1's three steps reproduced on a document built in memory (rule 26's erratum makes step 1 unreachable through `AddAsset`), asserting `assumed_from` is `None` afterwards and `color_description` is the sent bytes;
    - `in1_add_asset_refuses_an_agent_assumption_description` — rule 26's erratum: `AddAsset` carrying `recovery_description(&probed)` as its colour description is refused with `AssumedFromNotSuppliable` and changes nothing;
    - a relink leaves `assumed_from` and `color_description` byte-identical (rule 27);
    - `validate_asset` refuses a programmatically built out-of-range `assumed_from` (rule 30).

### 4.5 The recovery builder

32. Core gains the description half and the operation half, beside the policy table:

```rust
/// The description the Rec.709 recovery writes. Fills only `Unknown` fields
/// except for the three tags and the white point, which the shipped builder
/// has always written unconditionally (`color_ui.rs:129-133`).
#[must_use]
pub fn recovery_description(probed: &ColorDescription) -> ColorDescription;

#[must_use]
pub fn assume_rec709_operation(asset: AssetId, probed: &ColorDescription) -> Operation;
```

   `recovery_description` reproduces the shipped app builder's exact semantics (`crates/kinewright-app/src/color_ui.rs:117-139`): `Bt709` primaries, transfer and matrix; `range` kept if known else `Limited`; `bit_depth` kept if known else `Eight`; `white_point: D65`; `confidence_basis_points: COLOR_CONFIDENCE_MAX_BASIS_POINTS`; **provenance `AgentAssumption`**. `assume_rec709_operation` wraps it in `Operation::SetAssetColorDescription { asset, color_description }`. §2.4 rule 37's definition is written against `recovery_description`, so the predicate and the builder cannot diverge.
33. `assume_sdr_rec709_operation` (`color_ui.rs:117-139`) **must** stay, **must** keep `ColorProvenance::UserOverride`, and **must** be rewritten to delegate, so the person's button and the router's recovery cannot drift. The adapter is named so "cannot drift" is checkable (nit 6): the app function keeps its shipped signature `assume_sdr_rec709_operation(asset: &MediaAsset) -> Operation`, and its whole body becomes

```rust
let Operation::SetAssetColorDescription { asset: id, mut color_description } =
    kinewright_core::assume_rec709_operation(asset.id, &asset.color_description)
else { unreachable!("assume_rec709_operation returns SetAssetColorDescription") };
color_description.provenance = ColorProvenance::UserOverride;
Operation::SetAssetColorDescription { asset: id, color_description }
```

34. The Media-panel button (`crates/kinewright-app/src/media_bin.rs:384-390`) and `ASSUME_SDR_REC709_TOOLTIP` (`color_ui.rs:8`) are unchanged; no panel or button is removed (N0.3).
35. An app test **must** assert that the two builders differ in exactly one field — `provenance` — for the probed description of each of the four fixtures.

### 4.6 What Part A does not change in core, media or the app

36. `Operation` gains **no** variant and loses none; `operation_tools().len()` stays **54** (measured, probe-2 §T1.6).
37. `MediaError` loses **no** variant; `Backend(String)` stays and is still what every untyped media failure travels as.
38. `ColorDescription`'s eight fields (`color.rs:535-562`) are unchanged in name, order, type and serde attribute. Only `MediaAsset` grows, and only by an `Option` that is omitted when `None`.
39. `ColorContext`'s three descriptions, `validate_document`'s colour loop (`operation.rs:4275-4281`) and `ColorPipelineState` are untouched. `set_color_context` (`operation.rs:1662-1673`) gains no provenance guard; it has none today and IN1 declares no operation that writes a context.
40. Both managed-colour allowlists and the `"application_default or user_override"` phrase are untouched (§4.3 rule 15).
41. `ErrorLog`, `ErrorEntry`, `record_error` and `ErrorLog::push` are untouched (`crates/kinewright-app/src/error_ui.rs:10-54`). The router **adds** one line through the existing `record_error` and changes nothing about how the log works.
42. `Command::Undo`, `Command::Redo`, `HistoryEntry` and the undo stack are untouched (`crates/kinewright-core/src/actor.rs:200-213`, `:402-416`). §5.4 rule 32 says why, and §5.2b rule 16 says what the router does about it.
43. The confirmation broker and its six `confirm(...)` call sites are untouched (§1 item 5).
44. `COMPACT_TOOL_NAMES`, `served_tools()`, `call_exposed_blocking` and `is_invocable_capability` keep their current bodies; only `INSPECTOR_TOOL_NAMES`' contents and length change.

---

## 5. The router, and the card as a pure function

### 5.1 Where the log lives

1. **The `IncidentLog` lives on `ProjectSession`, not on `KinewrightApp`** (N2/B6). `KinewrightApp` is multi-project — `projects: Vec<ProjectSession>` (`crates/kinewright-app/src/app.rs:61`), each session owning its own `core`, `document` and `revision` (`crates/kinewright-app/src/project.rs:210-216`) — and asset ids are per-document, so an app-level log would make `subject: Asset(AssetId)` ambiguous across projects and would let project B's agent read project A's incidents.
2. `ProjectSession` gains one field beside `agent_project_path` (`project.rs:233`):

```rust
/// The incident log every MCP server in this session shares, in the shape
/// `agent_project_path` already uses (CC4 §2.2).
pub(crate) incidents: IncidentLogHandle,
```

   with `pub(crate) type IncidentLogHandle = std::sync::Arc<std::sync::RwLock<IncidentLog>>;` declared beside `ProjectPathHandle` (`project.rs:207`).
3. The handle is shared with every per-thread `McpServer` exactly as `ProjectPathHandle` already is (`crates/kinewright-app/src/chat_ui.rs:244-250`, `:295-301`; `crates/kinewright-agent/src/server.rs:525`) — **each project's servers share that project's handle, and no other's**. `McpServer` gains `incident_log_handle()` and one `…_with_incidents` constructor; every existing constructor **must** keep its signature and default the handle to a fresh empty log, so no existing call site changes.

### 5.2 `KinewrightApp::route_incidents`

4. The router is one method on `KinewrightApp`, called **once per `update`**, **after both** event drains: after the core-event loop (`app.rs:1045-1230`) and after the media-event drain (`app.rs:1242-1257`). It **must not** be a thread, a timer, or an immediate-mode call from a panel.
5. **Part A attributes a playback incident to the focused project, normatively, and the reason is written beside the rule.** `media_events` is a **single** app-level receiver from the one engine (`app.rs:78`, built at `app.rs:232` from `media.events()`) and the media drain at `app.rs:1242-1257` has no `project_index`. There is one engine and it plays the focused document, so the focused project is the correct owner of a playback refusal. The drain reads `let revision = self.focused().revision;` (`app.rs:440-443`) and pushes onto the focused session's pending observations. **Multi-project attribution — a per-project engine, or a project tag on `MediaEvent` — is a named Part B deferral (§13 D14).**
6. The `Media` playback arm (`app.rs:1252-1255`) is rewritten. It **must not** call `record_error` and **must not** format the error:

```rust
MediaEvent::Error(error) => {
    self.playing = false;
    let revision = self.focused().revision;
    if let Some(observation) = IncidentObservation::from_media_error(&error, revision) {
        self.pending_observations.push(observation);
    } else {
        self.record_error("Media", format!("Playback error: {error}"));
    }
}
```

   The `else` arm is the honest statement of §1 item 6: an untyped `MediaError` from the playback path — including the ALSA `snd_pcm_pause` failure `engine.rs:1920` emits on a box with no audio device — is still a `record_error` line in Part A.
7. `IncidentObservation::from_media_error` is core (§2.1 rule 2: core owns the mapping from a typed core error to a code; media owns the trigger). It returns `Some` only for `MediaError::SourceColorForAsset`, maps `ColorSourceError` → `SourceColorIncident` (`None` for `UnknownWhitePoint`), and fills `observed` from `ColorSourceError::observed()`, `allowed` from `allowed_values()` and `evidence` from the variant's `description`/`assumption`.
8. **Auto-apply happens at the incident, not at import.** Applying at import would rewrite the description of a source the managed path would have accepted, which is the silent behaviour CC1 §1 forbids, and would make the incident path dead code IN2 then has to resurrect.
9. For each **newly opened** incident whose stored `class` is `AutoApply`, the router **must** send `Command::DoIfRevision { expected: incident.revision, operation }` (`crates/kinewright-core/src/actor.rs:393-397`) — **not** `Command::Do`, which is what `send_operation` uses (`app.rs:882-888`) and which has no revision gate — and **must** call `IncidentLog::note_auto_applied(id)` at the same moment (§2.3 rule 19).
10. On `Event::RevisionConflict { expected, actual }` (`actor.rs:113-116`, produced at `actor.rs:425`) for a router-issued command, the router **must** re-observe — refresh the incident's `revision` and leave it `Open` — and **must not** re-apply. `revision` is load-bearing on both paths because of this rule. **Erratum (implementation, 2026-09-15):** the refresh is not a re-observe, because it cannot be one: the router calls `note_auto_applied` at send time (rule 9), so rule 19's suppression check at the top of `observe` returns `Observed::Suppressed` before the dedup arm that would refresh `revision`. The mechanism is instead: `RouterConflict` carries `actual` alongside `expected`, and `reconcile_router_sends`, on matching a conflict to an outstanding `RouterApply`, calls `IncidentLog::refresh_revision(apply.incident, conflict.actual)` (§2.3 rule 20's erratum) and then drops the send. `actual` is used rather than the live project revision because it is the revision core itself reported when it refused — the only evidence the router holds — and it is deterministic in tests. `Event::RevisionConflict` carries no send identity and several router applies in one project can share `expected` (two incidents opened in one frame are sent against the same revision), so the conflict is matched to an outstanding apply the live document does **not** already show as landed, falling back to the first candidate when every one of them does — which is this clause's own case, where the person's identical edit landed first.
11. The router **must not** apply anything for a class of `Explain`. `AskFirst` has **no** Part A entry; if one is ever reached the router **must** leave the incident `Open` and rely on the card's buttons.
12. On acceptance (`Event::DocumentChanged` carrying the router's operation) the router **must** call `IncidentLog::resolve(id, IncidentOutcome::Applied)` and stamp `IncidentTelemetry.resolved_after`. **Erratum (implementation, 2026-09-15):** `IncidentLog::resolve` stamps `resolved_after` itself from the session clock it owns (`started` is private and nothing outside the log can measure against it); the router and `resolve_incident` write only the cost mirrors and `tool_calls` through `telemetry_mut`.
13. **A migrated incident also writes exactly one audit line to `ErrorLog`** with source `"Incident"` and the card headline as its message, at the moment the incident is first **opened** — not on a dedup hit and not on a suppressed observation. This keeps the audit view complete and keeps the toolbar badge from regressing while **120** literal-labelled sites are unmigrated.
14. **The badge is unchanged in Part A**: it still counts `self.error_log.len()` (`app.rs:1487-1491`). Part B settles its semantics on open incidents when the log is demoted.
15. **The two counts are different quantities and the contract says so**: `IncidentLog::open_count()` counts *problems currently unresolved*; `ErrorLog::len()` counts *lines ever written this session*. One auto-applied, immediately resolved incident contributes 0 to the first and 1 to the second. A test **must** assert exactly that pair on the fixture — and it asserts the **number of `"Incident"`-source entries**, not `ErrorLog::len()`, because the log may legitimately also hold an environment-dependent ALSA line (§9 clause 12).

### 5.2b The tick, written out

16. The body of `KinewrightApp::update` gains exactly one call, in exactly this position. The pseudocode is normative as to **order**; the Rust is the implementer's:

```text
KinewrightApp::update(ctx):
    ...
    for (project_index, event) in core_events:        # app.rs:1056-1230, unchanged
        ...
        Event::RevisionConflict { expected, actual } =>
            route_incidents_note_conflict(expected, actual)   # NEW, before the
            record_error("Operations", ...)                   # existing line
    ...
    while let Ok(event) = self.media_events.try_recv():        # app.rs:1242-1257
        MediaEvent::Error(error) => ...                        # rule 6
    ...
    self.route_incidents()                                     # NEW, once, here
    ...
```

17. `route_incidents` does, in order: drain `pending_observations` into the focused session's `IncidentLog`; branch on `Observed`; for each `Opened(id)` write one `ErrorLog` audit line with source `"Incident"` (rule 13); for each `Opened(id)` whose `class` is `AutoApply`, call `note_auto_applied(id)` and send `Command::DoIfRevision`; reconcile any conflict noted in this frame's core drain by refreshing that incident's `revision` and leaving it `Open`; mark accepted applies `Resolved(Applied)`. `Deduped` and `Suppressed` do nothing at all.
18. **`note_auto_applied` is what makes a global `Undo` stick** (N2/B10). `ASSUME_SDR_REC709_TOOLTIP` tells the person *"Ctrl+Z restores the prior probed description."* (`color_ui.rs:8`), and `Command::Undo` restores a whole document snapshot (`actor.rs:200-203`, `:402-410`), so `assumed_from` reverts to `None` and the description reverts to the probed tuple — correctly. The restored document then fails the managed decode again and the engine emits `MediaEvent::Error` again. Without rule 19's suppression the first incident is `Resolved(Applied)`, a `Resolved` entry must not absorb an observation (§2.3 rule 16), a **second** incident would open with class `AutoApply`, and the router would put the assumption straight back — undoing the person's undo, in the one interaction the shipped tooltip advertises. Because the log suppresses `(code, subject)` from the moment the router *sends* the auto-apply, `observe` returns `Observed::Suppressed` and nothing is sent. §9 clause 17 is the gate.
19. It **must** be called after both drains and **must not** be called from inside either. Calling it inside the media drain would apply an operation whose acceptance arrives in the *next* frame's core drain, which is where a second apply comes from.
20. It **must** be idempotent across frames with no new observations: a frame in which nothing was observed and nothing conflicted **must** send no command and write no log line. A test asserts this by running the observation chain twice against an unchanged log and asserting the `"Incident"`-source entry count did not move.
21. The `Event::RevisionConflict` arm keeps its existing `record_error("Operations", …)` line (`app.rs:1219-1228`). Part A adds a note, not a replacement: that arm is one of the 32 `Operations` sites Part B owns.
22. `route_incidents` **must** be `pub(crate)`. The observation → policy → operation chain is core (§2), so the app test asserts the chain through core and asserts only the tick placement by inspection (§11.1 limit 2).

### 5.3 The card

23. The seam is a pure function in `crates/kinewright-app/src/incident_ui.rs`:

```rust
/// `assumed_from_present` is the one document fact the card needs and the
/// incident cannot carry: whether the subject asset still has something to
/// revert to. Passing it keeps the function pure and egui-free (N2/B10(ii)).
#[must_use]
pub(crate) fn incident_card(incident: &Incident, assumed_from_present: bool) -> IncidentCardView;

pub(crate) struct IncidentCardView {
    pub severity: IncidentSeverity,
    pub class: PolicyClass,
    pub subject_label: String,
    pub headline: &'static str,
    pub state_label: &'static str,
    pub actions: Vec<CardAction>,
    pub details: Vec<(&'static str, String)>,
}

pub(crate) struct CardAction {
    pub label: &'static str,
    pub enabled: bool,
    pub recovery: RecoveryAction,
}
```

   `incident_card` takes **no `egui::Ui` argument**, touches no `egui` type, and is the only tested half. The paint function `show_incident_card(ui, &view)` consumes the view and is untested, exactly as AU6 §1 records for every app surface.
24. `subject_label` comes from `IncidentSubject::label() -> String`, a core method returning `"Asset 1"` for `Asset(AssetId(1))`. The asset's human **name** is not on the incident and is a named deferral with its measured cost (§13 D15).
25. **`headline` is `&'static str`.** It comes from `pub(crate) fn incident_headline(code: IncidentCode, class: PolicyClass) -> &'static str`, an exhaustive match with no wildcard arm. Making it `&'static str` is what makes "the card headline is identical for both containers" (N1.5/Q4) a type-level fact rather than a hope: a headline cannot embed a path, a file name or a measured number. The asset identity lives in `subject_label` and the probed tuple in `details`.
26. **The actor word in Part A is "Kinewright"**, in every headline. IN2 changes it to the investigator's name where a session resolved it; the provenance stays `AgentAssumption` because the router is the investigator's deterministic stand-in (D7).
27. The pinned headline table, `assert_eq!`-asserted row by row. It has **fifteen rows for twelve codes** because the three `Rec709Compatible` codes are reachable under **both** classes; the other nine are reachable only as `Explain`.

| code | class | headline |
| --- | --- | --- |
| `unknown_source_primaries` | `AutoApply` | `Kinewright assumed Rec.709 for this source because its colour primaries were unknown.` |
| `unknown_source_transfer` | `AutoApply` | `Kinewright assumed Rec.709 for this source because its colour transfer was unknown.` |
| `unknown_source_matrix` | `AutoApply` | `Kinewright assumed Rec.709 for this source because its colour matrix was unknown.` |
| `unknown_source_primaries` | `Explain` | `This source does not say what colour primaries it uses, and the rest of its colour metadata rules out Rec.709.` |
| `unknown_source_transfer` | `Explain` | `This source does not say what colour transfer it uses, and the rest of its colour metadata rules out Rec.709.` |
| `unknown_source_matrix` | `Explain` | `This source does not say what colour matrix it uses, and the rest of its colour metadata rules out Rec.709.` |
| `unknown_source_range` | `Explain` | `This source does not say whether its levels are full or limited.` |
| `unknown_source_bit_depth` | `Explain` | `This source does not say how many bits per sample it uses.` |
| `unsupported_source_primaries` | `Explain` | `This source's colour primaries are not ones Kinewright can manage yet.` |
| `unsupported_source_transfer` | `Explain` | `This source's colour transfer is not one Kinewright can manage yet.` |
| `unsupported_source_matrix` | `Explain` | `This source's colour matrix is not one Kinewright can manage yet.` |
| `unsupported_source_range` | `Explain` | `This source's level range is not one Kinewright can manage yet.` |
| `unsupported_source_white_point` | `Explain` | `This source's white point is not one Kinewright can manage yet.` |
| `unsupported_source_bit_depth` | `Explain` | `This source's bit depth is not one Kinewright can manage yet.` |
| `unsupported_source_combination` | `Explain` | `This source's colour metadata does not add up to a profile Kinewright can manage yet.` |

28. `state_label` is `"Open"`, `"Applied"`, `"Reverted"` or `"Explained"`, from `IncidentState`.
29. **Actions.** For an incident whose state is `Resolved(Applied)` from an `AutoApply` recovery, `actions` **must** be exactly one entry with `label: "Revert to probed description"`, `recovery: RecoveryAction { label: "Revert to probed description", kind: Operation(SetAssetColorDescription { asset, color_description: evidence.probed.clone() }) }`, and **`enabled: assumed_from_present`**. The `enabled: false` case is not cosmetic: after a global `Undo` has cleared `assumed_from`, that operation carries provenance `StreamMetadata`, `is_actor` and `is_revert` are both false, and the guard returns `OpError::InvalidColorOverrideProvenance` — a hard rejection from a button the card would otherwise advertise as live (N2/B10(ii)).
30. For any incident whose class is `AskFirst` or `Explain`, `actions` **must** be exactly the `Vec<RecoveryAction>` that `policy_recovery` returned, one `CardAction` per entry, in order, with `enabled` true for `RecoveryKind::Operation` and false for `RecoveryKind::Explain`.
31. **"Never below the button", stated as a rule the exit gate can fail.** For every `POLICY` entry whose declared class is `AutoApply` or `AskFirst`, `incident_card` on the canonical incident for that entry **must** return a `CardAction` whose `recovery` is **equal** to the `RecoveryAction` `policy_recovery` returns for the same code and evidence. §9 clause 6 asserts this by equality over the whole table, with the canonical evidence named. For an `Explain` entry the principle is discharged by the shipped Media-panel control, which IN1 does not remove.
32. `details` carries the typed fields verbatim, in this fixed order: `("code", …)`, `("field", …)`, `("observed", …)`, `("allowed", …)`, `("probed", format!("{probed:?}"))`, `("assumed", …)` when `assumed_from_present`, `("revision", …)`, `("seen", count)`. Nothing in `details` is asserted by `assert_eq!` except the key order and the `code`/`field`/`observed`/`allowed` values, which come from core accessors.
33. **Placement.** The card is drawn in the Media panel beside the asset, where the "Assume SDR Rec.709 metadata" button already is (`media_bin.rs:382-391`). Placement is **untested**; the pure view is the tested thing. A toolbar-badge-anchored list for non-asset subjects is Part B.

### 5.4 The revert

34. Pressing **Revert to probed description** sends the card action's `Operation` through the router with `Command::DoIfRevision { expected: <current revision>, .. }`, marks the incident `Resolved(Reverted)` through `IncidentLog::resolve`, which inserts `(code, subject)` into `suppressed` (§2.3 rule 19) so the router does not re-apply for that asset for the session.
35. Global `Undo` is unchanged and **is not** the card's control. `Command::Undo` returns `last_op: None` (`actor.rs:402-410`) and `HistoryEntry` has no label (`actor.rs:200-213`), so the app is told *that* an undo happened and never *what* was undone; a card "Undo" would undo whatever is on top of the single global stack, which after one further edit is the person's edit, not the assumption. The Undo path is nonetheless honoured end to end by §5.2b rule 18 and §9 clause 17.
36. The revert **must** survive save and reopen, because `assumed_from` persists (§4.4 rule 18). A revert after a reopen restores the byte-identical probed description with its original provenance. Probe-2 §T6.2 measured the round trip at confidence 2 000 and 4 000 and found it byte-identical, provenance included.

---

## 6. The two capabilities

### 6.1 Route

1. `get_incidents` is an **Inspector** capability and `resolve_incident` is an **Action** capability. Both are registered in `inspector_tools()` (`crates/kinewright-agent/src/server.rs:12219+`) and in `INSPECTOR_TOOL_NAMES` (`crates/kinewright-agent/src/schema.rs:16`), and both are reached **only** through `invoke_capability` (`server.rs:697-710`). Neither is added to `COMPACT_TOOL_NAMES` (`runtime.rs:17-25`).
2. No `CAPABILITY_KIND_OVERRIDES` entry is needed, **measured**: probe-2 §T1.6 ran `capability_kind` (`runtime.rs:161-178`) and got `get_incidents = Inspector` on the `get_` prefix and `resolve_incident = Action` by fallthrough. A test **must** assert both kinds, so a future rename cannot silently reclassify them.
3. `is_invocable_capability` (`runtime.rs:194-206`) returns `true` for both, **measured** (probe-2 §T1.6), and **nothing it accepted before is now refused**: the only `INSPECTOR_TOOL_NAMES` entry that is neither invocable nor compact is still `"apply_edit_plan"`, and **zero** of the 54 generated operation tools became invocable. `call_exposed_blocking` (`server.rs:665-670`) still refuses a direct call to either by name with the existing `"{name} is an internal capability, not an MCP tool; use search_capabilities, get_capability, and invoke_capability or prepare_edit_plan"`; a test **must** assert that refusal for both, because it is the M36 design.

### 6.2 `get_incidents`

4. Annotations: `read_only(true).destructive(false).idempotent(true).open_world(false)` — the existing `read_only()` helper at `server.rs:12220-12227`.
5. Arguments, declared beside `DiscardEditPlanArgs` (`server.rs:10969`). **The doc comments are normative bytes**: probe-2 measured the 493 B input schema with exactly these words, and the two-line `expected_revision` comment contributes a literal `\n` that schemars embeds in the schema.

```rust
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetIncidentsArgs {
    /// Only incidents observed at this exact revision are returned; omit to
    /// read every open incident at the current revision.
    #[serde(default)]
    expected_revision: Option<u64>,
    /// Include incidents already resolved this session.
    #[serde(default)]
    include_resolved: bool,
}
```

6. `deny_unknown_fields` is required, matching the AU6 §13 `QueueExportArgs` ruling (`tests/mcp_server.rs:2411-2413`, "+29 B input schema"): a registry-only tool that silently accepts a misspelled argument teaches the model the wrong shape. One sentence for the ledger (nit 3): `deny_unknown_fields` emits `"additionalProperties": false` and therefore **adds** registry bytes, and probe-2's 493 B and 809 B figures already include it.
7. Response, through `success_structured` (`server.rs:13281`): `{ "timeline_revision": u64, "incidents": [Incident…], "open_count": usize, "next": "…" }`. `Incident` serialises through its own derive (§2.3 rule 11); the agent crate adds no second rendering.
8. When `expected_revision` is `Some` and does not match, the response **must** be `revision_conflict_text(expected, actual)` (`server.rs:16047-16051`), the same refusal every other revision-gated capability gives.
9. **Token budget, two constants** (N2/S3, N2.5). Both live in `crates/kinewright-agent/src/server.rs`:
    - **`IN1_INCIDENT_SERIALIZED_BYTES` = [probe-2b]** — asserted with `assert_eq!(serde_json::to_vec(&incident).len(), IN1_INCIDENT_SERIALIZED_BYTES)` for the **named largest fixture incident**, the untagged-WebM one. A budget with slack is not a budget; this is the shape every other M36 pin in this repo uses and the only shape that fails when a field is added. The doc comment names the fixture and the reason it is largest: probe-2 §T2.1 measured the two untagged incidents at 1 269 B (MP4) and 1 276 B (WebM) on the **draft's** shape, and the whole 7 B difference is `evidence.probed.provenance` — `"stream_metadata"` (15 B) against `"inferred"` (8 B); `observed`, `allowed` and `message` were *identical*. The draft's claim that the WebM's `observed` and `allowed` are longest is corrected here: they are equal.
    - **`IN1_INCIDENT_SERIALIZED_CEILING_BYTES` = [probe-2b]** — asserted `<=` over **all twelve codes** on one synthetic subject, with the value set to the smallest power of two above the measured worst. The ceiling gates the population Part B will grow, not the fixture alone; probe-2 §T2.2 measured the worst of the twelve on the draft's shape as `unsupported_source_combination` at 1 444 B, 168 B above the fixture worst, because its `observed()` is the only formatted tuple in the set (`color.rs:292-299`).
    - Both are **[probe-2b]** because revision 2 changed the measured object: dropping `message`, skipping `None` telemetry, keeping `opened_at` off the wire and fixing `RecoveryKind` move every figure. **The implementer measures against the implementation, not another prototype**, fills both constants, and the promotion note records them (§11.2).
10. **The pinned wire body, normative as generated** (N2/B4). A test **must** serialise the fixture incident and `assert_eq!` the result against this literal, so the two cannot drift. The fixture is built deterministically — `IncidentLog::with_start`, the `in1_untagged_vp9.webm` observation fed **twice** so `count` is 2 without an engine, and the incident read while still `Open` so no `Duration` is on the wire:

```json
{"id":1,"code":"unknown_source_primaries","class":"auto_apply","severity":"blocks","subject":{"asset":1},"field":"primaries","observed":"unknown","allowed":"bt709 or srgb in a supported CC1 profile","evidence":{"source_color":{"probed":{"primaries":"unknown","transfer":"unknown","matrix":"unknown","range":"limited","white_point":"unknown","bit_depth":8,"confidence_basis_points":4000,"provenance":"stream_metadata"},"assumption":null}},"recoveries":[{"label":"Assume Rec.709 for this source","kind":{"operation":{"SetAssetColorDescription":{"asset":1,"color_description":{"primaries":"bt709","transfer":"bt709","matrix":"bt709","range":"limited","white_point":"d65","bit_depth":8,"confidence_basis_points":10000,"provenance":"agent_assumption"}}}}}],"revision":1,"count":2,"state":"open","telemetry":{"tool_calls":0}}
```

   Every byte follows from §2.3 rule 11, §2.5 rule 42 and the types core already owns: `IncidentId`, `AssetId` and `TimelineRevision` are `#[serde(transparent)]` newtypes over `u64` (`crates/kinewright-core/src/model.rs:15-32`, `actor.rs:29-30`); `ColorBitDepth::Eight` serialises as the number `8` (`color.rs:485-502`); `Operation` is externally tagged with no `rename_all`, hence `"SetAssetColorDescription"`; `state` is flattened and the incident is open, hence the single `"state":"open"` and no `"outcome"`; `opened_at` is `#[serde(skip)]`; every `Option` telemetry field is skipped, leaving only `tool_calls`. The same object after the auto-apply resolves reads `"state":"resolved","outcome":"applied"` in place of `"state":"open"` and gains `"resolved_after"` inside `telemetry`; **no test asserts either of those two forms byte for byte**, because `resolved_after` is a wall-clock `Duration` (§9 regression item R4).

   Hand-serialising the literal above gives **819 bytes** against the draft shape's measured 1 276 — a saving of roughly 36 %. That figure is arithmetic on this page, **not** a measurement, and it is not what the constants carry; §11.2 says what the implementer measures.
11. The `next` string is one sentence, in the voice of the existing `search_capabilities` and `get_capability` responses (`server.rs:683-687`), and is what stops a model inventing a second round trip: *"Apply a recovery through prepare_edit_plan and commit_edit_plan, then record the outcome with resolve_incident at the same revision."*
12. **What the payload no longer carries, and why it is IN1's one true efficiency statement beyond the surface pins** (N2/S4). `message` is gone (§2.3 rule 12). The six `AgentEvent::Cost` mirrors and `resolved_after` skip when `None`, so an unresolved incident serialises `"telemetry":{"tool_calls":0}` instead of eight keys of `null`. `opened_at` is off the wire entirely — a value §9 forbids asserting is a value nobody should be charged for. `evidence.probed` and `recoveries[0]…color_description` remain the same eight fields twice, differing in five of them, and that redundancy is **kept on purpose**: the before and the after are the two things a person or a model has to compare to judge the assumption.

### 6.3 `resolve_incident`

13. Annotations: `read_only(false).destructive(false).idempotent(false).open_world(false)`. **`destructive(false)` is deliberate**: the destructive list that drives the confirmation broker is `DeleteClip | RippleDeleteClip | RemoveTrack | RemoveBin | RemoveStringOut | RemoveSyncGroup | RemoveAudioBus` (`runtime.rs:351-358`) and `SetAssetColorDescription` is not in it, so a `destructive(true)` annotation would advertise a broker gate that does not exist. The conclusion stands on `runtime.rs:351-358` alone; the draft's `runtime.rs:150-152` citation is dropped, because that comment governs `CAPABILITY_KIND_OVERRIDES` (nit 4).
14. Arguments. **The four doc comments are normative bytes** — they are 204 B of the measured 809 B input schema (probe-2 §T1.1), so changing a sentence moves the registry pin:

```rust
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ResolveIncidentArgs {
    /// Exact incident id returned by `get_incidents`.
    incident_id: u64,
    /// The same exact revision the incident was observed at.
    expected_revision: u64,
    /// Outcome the caller already produced through the ordinary edit path.
    outcome: ResolveIncidentOutcome,
    /// Optional note recorded beside the outcome.
    #[serde(default)]
    note: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ResolveIncidentOutcome { Applied, Reverted, Explained }
```

15. `expected_revision` is **not** optional. The write side is revision-gated unconditionally; a stale call **must** return `revision_conflict_text` and **must not** touch the log.
16. Semantics, normative:
    - `applied` — the caller applied the incident's `AutoApply` recovery through `prepare_edit_plan`/`commit_edit_plan` or through the router. The server verifies the asset now carries `AgentAssumption` and `assumed_from` is `Some`; if not, it **must** refuse with code `incident_not_applied` and change nothing.
    - `reverted` — the caller restored `assumed_from`. The server verifies `assumed_from` is now `None` and the description equals the incident's `evidence.probed`; if not, it **must** refuse with code `incident_not_reverted`.
    - `explained` — the log records the outcome and the optional `note`. No document check. **Erratum (implementation, 2026-09-15):** the record has no note field in Part A; `resolve_incident` echoes `note` in its structured response and stores nothing. Recording it beside the outcome arrives with IN2's session, which is what would read it back.
17. `Approved` and `Rejected` are **not** IN1 outcomes. They arrive with IN2, which owns the typed broker (§13 D6).
18. `resolve_incident` **must not** apply an operation itself. It records an outcome against work the caller already did. This keeps the single mutation path — `Command::Do*` through the core actor — and keeps the capability free of the edit-plan machinery `prepare_edit_plan` owns.
19. Refusals use `error_structured` (`server.rs:13376`) with `{ "code", "field", "observed", "allowed", "recovery_action" }`, the shape `import_lut_asset` reaches through the `lut_import_error(...)` wrapper (`server.rs:1951-1959`). The codes IN1 adds to the agent-facing set are exactly three: `incident_not_found`, `incident_not_applied`, `incident_not_reverted`.

### 6.4 Descriptions

20. Both descriptions are one dense paragraph in the voice of the existing seven, first sentence self-contained because `first_sentence` (`runtime.rs:185-190`) is what `search_capabilities` and `get_capability` publish. **These exact words were measured** (probe-2 §T1.5: 492 B and 365 B of description, first sentences 236 and 149 characters):

- `get_incidents` — *"Return the open incidents for this project at one exact timeline revision, each with its stable code, severity, subject, observed and allowed values, the probed source description, and the typed recovery actions its policy class allows. An incident of class auto_apply has already been applied by the application; revert it by sending its probed description back through set_asset_color_description. Pass expected_revision to gate the read, include_resolved to see this session's resolutions."*
- `resolve_incident` — *"Record the outcome of one incident at the exact revision it was observed at, after applying or reverting its recovery through the ordinary edit path. Outcomes are applied, reverted, or explained; the server verifies the document matches the claimed outcome and refuses with a typed code when it does not. This records an outcome and never edits the document itself."*

21. `get_incidents`' description claims the payload states the class, and after N2/B5 it does: `"class"` is a field (§6.2 rule 10), not a predicate the agent must re-derive.

### 6.5 Where the server gets the incidents

22. `KinewrightMcp` gains one field, `incidents: IncidentLogHandle` (§5.1 rule 3), threaded through `configured` (`server.rs:600-638`) exactly as `project_path` is.
23. In the app the handle is the live one for **that project**, so the chat agent sees the same incidents the person sees and no others. In a scripted test the handle is the test's own, and the test observes the **real** typed error produced by a **real** managed decode of the fixture (§7 rule 4), never a hand-built `Incident`.
24. `get_incidents` **must not** derive incidents by classifying the document's assets. That is the bare-classifier path §2.4 rule 32 forbids, and it would report a white-point incident for every correctly tagged source.

### 6.6 The surface pins

25. **The served quad does not move: `7 / 5 660 / 3 510 / 998`**, measured by probe-2 §T1.3 with both capabilities registered **and** the core changes of §4.3 and §4.4 applied. Asserted unchanged in **both** pin sites: `crates/kinewright-agent/src/server.rs:25499-25508` and `crates/kinewright-agent/tests/mcp_server.rs:2550-2554`. This is the **sixteenth** consecutive measurement (`tests/mcp_server.rs:2416` says fifteenth). **Erratum (IN1b-R3, 2026-09-15):** "Asserted unchanged in **both** pin sites … the **sixteenth** consecutive measurement" becomes all **three** pin sites and the **seventeenth** consecutive measurement after [IN1b](IN1B-ERROR-MIGRATION.md) §6.4 rules 7–8; the quad's own four figures do not move.
26. Each pin site's doc comment gains one sentence naming IN1 and saying why the quad did not move: the two capabilities are registry-only and `served_tools()` filters by `COMPACT_TOOL_NAMES` (`server.rs:649-654`), which IN1 does not touch.
27. **The registry sextuple is re-pinned to probe-2's measurement on the drafted schemas: `140 / 54 / 86 / 1 551 301 / 1 407 012 / 121 315`** — `tool_count` / `operation_tools().len()` / `INSPECTOR_TOOL_NAMES.len()` / serialized / input-schema / description bytes. Baseline at `c3a5814` was `138 / 54 / 84 / 1 540 292 / 1 397 185 / 120 458`. M36 reduction ratio **99.635 %** (`5 660 / 1 551 301`).
28. **The pin sites, with the owners probe-2 §T1.7 named:**

| site at `c3a5814` | assertion | before | after |
| --- | --- | ---: | ---: |
| `crates/kinewright-agent/src/server.rs:21794` | `INSPECTOR_TOOL_NAMES.len()` | 84 | **86** |
| `crates/kinewright-agent/src/server.rs:25476-25479` | `registry.len() == operation_tools().len() + INSPECTOR_TOOL_NAMES.len()` | 138 = 54 + 84 | **140 = 54 + 86** |
| `crates/kinewright-agent/src/server.rs:25484-25489` | `(registry.serialized, served.serialized)` | `(1_540_292, 5_660)` | **`(1_551_301, 5_660)`** |
| `crates/kinewright-agent/src/server.rs:25491-25494` | `registry.input_schema_bytes` | 1 397 185 | **1 407 012** |
| `crates/kinewright-agent/src/server.rs:25495-25498` | `registry.description_bytes` | 120 458 | **121 315** |
| `crates/kinewright-agent/src/server.rs:25499-25508` | the served quad | `(7, 5_660, 3_510, 998)` | **unchanged** |
| `crates/kinewright-agent/tests/mcp_server.rs:2444-2456` | `registry.len()` | 138 | **140** |
| `crates/kinewright-agent/tests/mcp_server.rs:2458-2465` | `operations.len()` | 54 | **unchanged** |
| `crates/kinewright-agent/tests/mcp_server.rs:2486-2492` | `registry.len() - operations.len()` | 84 | **86** |
| `crates/kinewright-agent/tests/mcp_server.rs:2550-2554` | the served quad | `7 / 5 660 / 3 510 / 998` | **unchanged** |

   **`server.rs:21794` has a third owner the draft did not name**: `server::tests::cc5_matte_tools_are_registered_read_only_inspectors` (`server.rs:21829`) was the only agent test other than the two pin tests that broke in probe-2's worktree. §14's implementer-D row names it. With those five numbers updated and nothing else changed, probe-2 measured `cargo test -p kinewright-agent --lib` → **501 passed, 0 failed** and `cc7_the_agent_surface_is_unchanged_by_this_slice` → ok.
29. `docs/M36-AGENT-RUNTIME-EFFICIENCY.md` gains two rows after the AU6 rows (`:124-132`): "Internal capability registry (after IN1 Part A)" with the sextuple of rule 27, and "Served MCP runtime (after IN1 Part A)" at 7 / 5 660 B / 3 510 B / 998 B, with one sentence stating that IN1 added two capabilities and no tool.
30. IN1 **must not** change `operation_tools().len()`; it adds no `Operation` variant. The `54` pin at `tests/mcp_server.rs:2458-2465` stays.

### 6.7 What the route cost, in two tables

31. **Table 1 — what the served surface costs, which is the whole of the route decision.** The tool-route column was measured by probe-1 §P3 on *sketch* schemas (a one-field `GetIncidentsArgs`, four outcomes, `destructive(true)`), so it is a **lower bound** on what serving the drafted schemas would cost; probe-2 §T1.5 confirmed the drafted schemas are larger than the sketches in every column.

| served quantity | capability route (IN1) | tool route (rejected, sketch schemas) | delta |
| --- | ---: | ---: | ---: |
| tool_count | **7** | 9 | +2 (+28.6 %) |
| serialized_bytes | **5 660** | 7 672 | **+2 012 (+35.5 %)** |
| input_schema_bytes | **3 510** | 4 880 | +1 370 (+39.0 %) |
| description_bytes | **998** | 1 316 | +318 (+31.9 %) |
| M36 reduction ratio | **99.635 %** | 99.50 % | −0.13 pt |

   **The capability route pays zero of this — of these *served* bytes.** That qualification is the correction N2/S1 required.

32. **Table 2 — what IN1 costs the registry, which both routes pay identically.** `served_tools()` is a filtered view of `capability_tools()` (`server.rs:649-654`), so the two schemas land in the registry either way and the registry delta does not depend on the route. Measured by probe-2 §T1.4, each cause in its own worktree configuration:

| cause | Δ serialized | Δ input_schema | Δ description |
| --- | ---: | ---: | ---: |
| the two capabilities (§6.2–§6.4) | +2 484 | +1 302 | **+857** |
| `ColorProvenance::AgentAssumption` (§4.3 rule 12) | **0** | **0** | 0 |
| **`MediaAsset.assumed_from` (§4.4 rule 18)** | **+8 525** | **+8 525** | 0 |
| total, `c3a5814` → after Part A | **+11 009** | **+9 827** | +857 |

   The sextuple after the capabilities **alone** is `140 / 54 / 86 / 1 542 776 / 1 398 487 / 121 315`. **`assumed_from` is the larger cost by a factor of three and a half**, and the reason is worth writing down: `MediaAsset`'s `JsonSchema` is inlined into the generated `add_asset` and `relink_asset` operation tools, and `ColorDescription`'s eight-property sub-schema is inlined with it. The +857 description bytes are exactly `492 + 365`, so the whole description delta is the capabilities'. `assumed_from`'s visibility in `add_asset`'s schema is also what forces §4.4 rule 26.

33. **Per capability on the drafted schemas** (probe-2 §T1.5), by `ToolSurfaceMetrics::measure(&vec![tool.clone()])`: `get_incidents` **1 145 / 493 / 492**, `resolve_incident` **1 339 / 809 / 365**; `1 145 + 1 339 = 2 484`, exactly the capabilities-only delta. These supersede probe-1's 960/624/176 and 1 052/746/142, which were measured on sketch schemas. **The draft's claim that `resolve_incident` is "byte-for-byte the size of `commit_edit_plan`" is deleted** (N2.5): at 1 339 / 809 / 365 it is the largest capability in the compact set, ahead of `commit_edit_plan` (1 056 / 746 / 146) and `search_capabilities` (1 180 / 885 / 129). These figures are **not** `[probe-2b]`: revision 2 changes neither argument struct nor either description, and §6.2 rule 5, §6.3 rule 14 and §6.4 rule 20 now carry the exact doc comments and sentences probe-2 measured.
34. One measurement note for the implementer: schemars joins a two-line doc comment with a literal `\n`, and that newline is one of `get_incidents`' 493 bytes. Keeping §6.2 rule 5's comment on two source lines is what reproduces the pin.

---

## 7. The scripted agent tests

1. All tests live in `crates/kinewright-agent/tests/mcp_server.rs`, are named `in1_…`, and imitate the `au6_` shape (`mcp_server.rs:8995`, `au6_a1_the_interview_ducks_the_bed_and_matches_the_voices`): build the document, `Core::spawn`, `McpServer::start…`, `().serve(StreamableHttpClientTransport::from_uri(server.endpoint()))`, drive through the existing helper — `async fn invoke_capability(client: &RunningService<RoleClient, ()>, name: &str, arguments: serde_json::Value) -> CallToolResult` (`mcp_server.rs:1934-1938`) — assert, `client.cancel()`, `server.shutdown()`. The signature is named here so seven tests do not each rediscover it (nit 9).
2. The document is built from `in1_sources::in1_untagged_mp4()` through the ordinary probe path, so the asset carries the §3 rule 5 tuple and nothing is hand-written. The asset id is **1**, because §9 clause 11's template pins `asset 1`.
3. **`in1_the_untagged_source_opens_one_incident_and_the_tagged_twin_opens_none`.** One test, **both** fixtures, **one** log, **one** process (N2/S14), so an always-empty `get_incidents` fails on the first half rather than passing the negative. Attempt a managed decode of `in1_untagged.mp4` through the live `Playback`, drain `events()` until a `MediaEvent::Error` arrives or `IN1_DECODE_EVENT_DEADLINE` expires, feed it to the shared `IncidentLog` through `IncidentObservation::from_media_error`, then `invoke_capability("get_incidents", {})`. Then do the same with `in1_tagged.mp4` against the same log. Asserts: after the first, **exactly one** incident, `code == "unknown_source_primaries"`, `class == "auto_apply"`, `field == "primaries"`, `observed == "unknown"`, `severity == "blocks"`, `count >= 1`, one recovery whose `kind` is `{"operation": …}` carrying `SetAssetColorDescription`; after the second, still exactly one incident and no new id. **The test does not call `play`**: probe-2 §R5.3 measured an ALSA `snd_pcm_pause` failure arriving nondeterministically from `engine.rs:1920` on a box with no audio device, and `set_document` plus one `request_frame` is enough to produce the refusal.
4. The test **must** obtain its typed error from the real decoder on the real fixture. Hand-constructing a `MediaError::SourceColorForAsset` would make the test pass with the plumbing of §4.2 absent.
5. **`in1_the_agent_applies_the_recovery_and_records_the_outcome`.** Take the recovery operation from `get_incidents`, send it through `prepare_edit_plan` + `commit_edit_plan`, then `invoke_capability("resolve_incident", {incident_id, expected_revision, outcome: "applied"})`. Asserts: the revision advanced; the asset reads the §3 rule 8 tuple; `assumed_from` is `Some` and equals the probed tuple; the second `get_incidents` with `include_resolved: true` shows `"state":"resolved","outcome":"applied"`.
6. **`in1_a_stale_resolve_is_refused_and_changes_nothing`.** `resolve_incident` with `expected_revision + 3` returns the `revision_conflict_text` refusal, the log is unchanged, and the document is unchanged, in the shape of `cc7_assert_stale_revision`.
7. **`in1_the_revert_restores_the_probed_description_and_clears_the_record`.** Send `assumed_from`'s bytes back through `prepare_edit_plan` + `commit_edit_plan`, then `resolve_incident` with `outcome: "reverted"`. Asserts: the description is byte-identical to the probed tuple including provenance; `assumed_from` is `None`; the incident is `Resolved(Reverted)`.
8. **`in1_a_wrong_outcome_claim_is_refused_by_code`.** `resolve_incident` with `outcome: "reverted"` when the document still carries the assumption returns `incident_not_reverted` and changes nothing.
9. **`in1_neither_capability_is_callable_as_a_tool`.** A direct `call_tool("get_incidents", …)` and a direct `call_tool("resolve_incident", …)` both return the "internal capability, not an MCP tool" refusal (`server.rs:665-670`).
10. **`in1_the_agent_surface_grows_by_two_capabilities_and_no_tool`.** The served quad, the registry sextuple, the two capability kinds, and `INSPECTOR_TOOL_NAMES.len()`, in the shape of `cc7_the_agent_surface_is_unchanged_by_this_slice` (`mcp_server.rs:2419`, body `:2418-2558`).
11. That is **seven** `in1_` tests, not the draft's eight: N2/S14 merged the tagged-twin negative into rule 3.
12. Every `in1_` test runs with **no model, no network beyond loopback, and no audio device**. The fixtures are video-only.
13. **`IN1_DECODE_EVENT_DEADLINE = Duration::from_secs(10)`**, one constant in `crates/kinewright-agent/tests/mcp_server.rs` beside the AU5 silence helpers. Every `in1_` test that waits for a `MediaEvent` **must** use it, **must** fail with the fixture name and the events it did see, and **must not** loop forever. **The 10 s stands against probe-2's recommended 1 s, and the ruling's reason is recorded beside the constant** (N2.5): a deadline here is a hang detector that only fires on failure, a cold Windows CI runner loading the FFmpeg DLLs for a first VP9 decode is not bounded by a warm Linux figure, and a tighter bound buys nothing when the test passes. The measured figure — `Playback::set_document` to the first `MediaEvent::Error`, five fresh engines on the lavapipe software lane, **max 0.0754 s**, min 0.0602 s, WebM 0.0603 s — is recorded in §11.1 as the thing the margin is measured against, not as the bound.
14. The wait helper is `in1_await_media_error(playback, deadline) -> Option<MediaError>`, one function, shared by every test that needs it, in the shape of `au5_invoke_when_silence_is_ready`. No test spells the loop itself.
15. **Two harness quirks are pre-existing and are not IN1 regressions**, recorded so a reviewer does not chase them: `cargo test -p kinewright-media --lib -- <filter> --test-threads=2` segfaults reproducibly, and `cargo test -p kinewright-agent --test mcp_server <filter>` exits `SIGSEGV` *after* every test reports ok. Both predate IN1 (AU6 §14).
16. **Expected lane cost, estimated not measured** (probe-2 §R8.2): the two bracketing proxies in the same binary are `cc7_the_agent_surface_is_unchanged_by_this_slice` at 0.27 s and `au6_a1_…` at 23.63 s; an `in1_` test is the first shape plus one 0.036 s fixture encode, one `probe_path` and one `FfmpegMediaEngine` start, and probe-2's engine-driving measurements completed in 0.62–1.01 s each. **Estimate 0.3–1.5 s per test, 2–11 s for all seven.** Marked as an estimate wherever it is quoted.

---

## 8. Telemetry

1. `HumanQuestion` (`crates/kinewright-agent/src/eval.rs:1291-1299`) gains one field:

```rust
#[serde(default)]
pub kind: QuestionKind,

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind { Recovery, #[default] Judgement }
```

2. `#[serde(default)]` resolving to `Judgement` is required so every historic `human-review.json` still loads: every question asked before IN1 was a creative judgement question, and none was a recovery question. A test **must** assert that a v6-era question JSON without `kind` deserializes to `Judgement`.
3. `QuestionKind::Recovery` is what IN4's "zero recovery questions asked" gate counts. IN1 adds the field and asks no recovery question; it pins no budget.
4. `IncidentTelemetry` on `Incident`. **Every `Option` field skips when `None`** (N2/S4), so an unresolved incident costs one key:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct IncidentTelemetry {
    /// Wall time from observation to resolution, measured at the router.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_after: Option<Duration>,
    /// Tool calls the resolver made, when an agent resolved it.
    pub tool_calls: u32,
    /// Provider token categories reported by the resolving session, when one
    /// reported them.
    #[serde(skip_serializing_if = "Option::is_none")] pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")] pub cached_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")] pub cache_creation_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")] pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")] pub reasoning_output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")] pub cost_usd_millionths: Option<i64>,
}
```

5. **The six token fields are the same six categories as `AgentEvent::Cost` (`crates/kinewright-core/src/agent.rs:64-76`), each `Option`-wrapped because IN1 records no session** (N2/S13). They are *not* a one-for-one mirror of the event's types: `input_tokens` and `output_tokens` are plain `u64` on the event (`agent.rs:67`, `:72`) and `Option<u64>` here. `cost_usd` is `Option<f64>` on the event and is stored here as `Option<i64>` **millionths of a dollar**, because core carries no `f64` in a stored record (AU6 §2.1's rule) and because a dollar figure is a currency amount, not a measurement.
6. In IN1 the router resolves every incident, so `resolved_after` is set and every token field is `None`. A test **must** assert `resolved_after.is_some()` — never a value — and that all six token fields are `None`, so the fields are proven to serialize and proven to be honestly empty rather than silently zero.
7. IN1 **records**; it pins no budget. IN2 pins budgets once measured.
8. The programme fact recorded by probe-1 §Q10 and carried into §13: Claude Code reports all five categories plus `cost_usd` (`crates/kinewright-agent/src/protocol.rs:90-129`); Codex reports the categories but **no `cost_usd`** (`protocol.rs:150-185`, `docs/agent-harnesses.md:79-82`); Cursor ACP constructs **no** `AgentEvent::Cost` at all (`crates/kinewright-agent/src/cursor.rs`).

---

## 9. Exit gate

Every numbered clause is falsifiable, is discharged by an ordinary `cargo test` on **both** CI operating systems, and needs no model, no network beyond loopback and no audio device. **A clause that can pass at `c3a5814` before IN1 exists is not a clause** — the three that could have are moved below the list, where they belong (N2/S2).

**How the roadmap's IN1 row is discharged.** `docs/ROADMAP-AND-WORKFLOWS.md:804` states the row's gate in three clauses and its deliverable in one. This contract discharges all four as amended, and the row is rewritten at promotion (§14):

| roadmap clause | discharged by | amended how |
| --- | --- | --- |
| "Every existing `record_error` source emits a typed incident" | **Part B**, §10 rule 9 | N1's split: Part A migrates **one** source, `Media` playback, and the row gains "Part A: one source, end to end; Part B: the remaining 125 error paths." |
| "the policy table is exhaustive over declared codes and tested without a model" | **clause 1** | unchanged; "declared codes" is Part A's twelve (§2.2) and Part B's full set |
| "importing an untagged BT.709-shaped source and playing it asks the person nothing on both CI operating systems" | **clauses 3, 5 and 6** | B6: "asks nothing" was unfalsifiable as "zero confirmation requests", because `SetAssetColorDescription` is not in the broker's destructive list (`runtime.rs:351-358`) and the app's own apply path never reaches the broker (`app.rs:882-888`). It becomes zero `AskFirst` incidents, zero `Recovery` questions, one pinned headline, plus S8's whole-table equality and B7's unmoved served quad. |
| **deliverable:** "`get_incidents` and `resolve_incident` **on the compact runtime**" | **§6** | **S9: the deliverable column is amended, not only the gate.** The two are reachable through the compact runtime's capability dispatcher, with the **served surface unchanged**; putting them *on* the served surface would have cost **+2 012 B (+35.5 %)** of served bytes (§6.7 table 1). Riel reads the roadmap, not §6.7, so the reason goes in the row. |

1. **`POLICY` is exhaustive over `IncidentCode`** by an explicit `match` with no wildcard arm, `POLICY.len() == 12`, every row `Blocks`, and `SourceColorIncident::code()`/`field()` equal to `ColorSourceError::code()`/`field()` for all twelve, with `UnknownWhitePoint` mapping to `None`. (§2.2 rules 7–8, §2.4 rule 35) **Erratum (IN1b-R1a, 2026-09-15):** "every row `Blocks`" becomes "every **colour** row blocks and every row's severity is the one it declares", because `POLICY` gains a severity column over the full code set ([IN1b](IN1B-ERROR-MIGRATION.md) §3.6 rule 29). **Erratum (IN1b-R1b, 2026-09-15):** "`POLICY.len() == 12` … for all twelve, with `UnknownWhitePoint` mapping to `None`" becomes `POLICY.len() == 67` and thirteen-for-thirteen with **no `None` case** (IN1b §3.2 rules 12 and 16).
2. **Every `AutoApply` entry's action is an `Operation` that `Document::apply` accepts on a fixture**, and the canonical evidence is named (N2/S12): for each `AutoApply` row, `policy_recovery`'s operation under `in1_untagged.mp4`'s pinned §3 rule 5 tuple applies cleanly to a document built from that fixture, with no capability call and no file write; and under a `Bt2020` probe the same row returns the `Explain` recovery instead. Both directions in one test. (§2.5 rules 45–46, §4.5)
3. **The person path, as amended by B6.** Importing `in1_untagged.mp4` and playing it reaches a **playable managed decode** with **zero** incidents of `class == AskFirst`, **zero** `HumanQuestion`s of kind `Recovery`, and **exactly one** incident of `class == AutoApply` — read from the incident's own `class` field, not re-derived — whose `incident_card(…).headline` is `assert_eq!`-equal to `"Kinewright assumed Rec.709 for this source because its colour primaries were unknown."` Both operating systems. (§2.3 rule 11, §3, §5.3)
4. **Dedup, split into the deterministic half and the engine half** (N2/B8). (a) On the fixture, **exactly one** incident is open however many errors arrived, with `count >= 1`. (b) A core test feeds two identical `IncidentObservation`s into `IncidentLog::observe` and asserts the second returns `Observed::Deduped` with the **same** `IncidentId`, `count == 2` and an **unchanged** `opened_at`. (c) A third observation differing only in `observed` opens a **second** incident. (§2.3 rules 15 and 27, §3 rule 12)
5. **The negative.** `in1_tagged.mp4` opens **zero** incidents and decodes managed, in the same test, the same process and the same log as the positive. (§7 rule 3)
6. **Never below the button (S8).** For every `POLICY` entry whose declared class is `AutoApply` or `AskFirst`, `incident_card` returns a `CardAction` whose `recovery` is equal to `policy_recovery`'s, asserted by equality **over the whole table** in one test, with `in1_untagged.mp4`'s pinned tuple as the canonical evidence for every row. (§5.3 rule 31)
7. **The guard.** Every `ColorProvenance` other than `UserOverride` and `AgentAssumption` — each named in its own assertion, including `Other("agent_assumption")` and `Other("anything")` — is refused with `OpError::InvalidColorOverrideProvenance`. (§4.4 rule 31)
8. **The revert round trip, and zero confidence both ways.** After an auto-apply, sending `assumed_from`'s bytes back restores the **byte-identical** probed `ColorDescription`, provenance included, and sets `assumed_from` to `None`; a zero-confidence probed description reverts with `Ok(())`; a zero-confidence `AgentAssumption` is refused with `OpError::ZeroConfidenceColorOverride`. (§4.4 rules 22, 24, 31)
9. **The router applies with a revision gate.** A router auto-apply issued against a stale revision produces `Event::RevisionConflict` and the incident stays `Open` with its `revision` refreshed and **no** second apply. (§5.2 rules 9–10)
10. **Serialization, and the hand edit is caught on load.** (a) In the `contracts.rs:854-858` shape, a `Document` built in-test with no assumption serialises without the substring `"assumed_from"`, and `crates/kinewright-core/tests/fixtures/pre_m13_project.json` round-trips to **1 215 B** with FNV-1a 64 **`ff6c17d72643a88b`**, byte-identical to `c3a5814` (**Erratum (implementation, 2026-09-15):** the pinned digest is **`c9da3186e131e4fd`**, standard FNV-1a 64 over the compact bytes; see §4.4 rule 18). (b) A hand-edited `assumed_from` carrying `confidence_basis_points: 10_001` is rejected **on load**, at deserialisation, with `"colour confidence must be in 0..=10000 basis points"` — the test asserts the **deserialise** error, not an `OpError`, because `ColorDescription`'s `#[serde(deserialize_with = "deserialize_confidence_basis_points")]` (`color.rs:556-557`, the function at `:606`) fires before `validate_asset` is ever called. (§4.4 rules 18, 30)
11. **The message did not move — pinned as a template, not a rendered string** (N2/B2). The rendered `Display` of the managed-decode failure on `in1_untagged.mp4` is asserted as

```rust
assert_eq!(
    message.replace(&path.display().to_string(), "{path}"),
    IN1_MANAGED_DECODE_REFUSAL
);
```

   against the **750 B** template captured from `c3a5814` at `target/review/in1/probe2/display-literal-template.txt`, in which `{path}` occurs **twice**. A rendered literal is undischargeable: every IN1 fixture is a `GeneratedMedia` under `std::env::temp_dir().join(unique_stem(label))` (`test_support.rs:54-57`), so the path differs per run and per operating system, and §1 item 9 forbids a per-OS constant. The `replace` form pins every byte that is not the path and survives Windows' `\` separators. Probe-2 §T5 measured the rendered instance at **920 B** with its 91 B path twice, the path-independent part at 738 B, and the app-level `format!("Playback error: {error}")` at **936 B** — 16 B more. (§4.1 rules 2–4) **Erratum (E-B5, 2026-09-22):** the **750 B** template becomes **708 B** — 750 − 2×21 for the dropped `"media backend error: "` prefix — with both pinned literals and both length pins moving ([IN2B](IN2B-INCIDENT-PERSISTENCE.md) §0.5 E-B5, §8).
12. **The two counts.** On the fixture, `IncidentLog::open_count() == 0` after the auto-apply resolves, and the number of `ErrorLog` entries whose source is `"Incident"` is **exactly 1**. The clause counts `"Incident"`-source entries rather than `ErrorLog::len()`, because the log may also legitimately hold an environment-dependent ALSA line (probe-2 §R5.3). (§5.2 rule 15)
13. **The surface.** The served quad is `7 / 5 660 / 3 510 / 998` in **both** pin sites; `INSPECTOR_TOOL_NAMES.len() == 86`; `operation_tools().len() == 54`; the registry sextuple equals `140 / 54 / 86 / 1 551 301 / 1 407 012 / 121 315`; `get_incidents` is `Inspector` and `resolve_incident` is `Action`; both are refused as direct tool calls; `apply_edit_plan` is still the only non-invocable inspector name and **zero** operation tools became invocable. (§6.6) **Erratum (IN1b-R3, 2026-09-15):** "in **both** pin sites" becomes all **three** after [IN1b](IN1B-ERROR-MIGRATION.md) §6.4 rules 7–8, at the seventeenth consecutive measurement; the quad and the sextuple are unchanged.
14. **The token budget, two assertions that can fail.** `assert_eq!(serde_json::to_vec(&incident).len(), IN1_INCIDENT_SERIALIZED_BYTES)` for the named largest fixture incident, and `<= IN1_INCIDENT_SERIALIZED_CEILING_BYTES` for all twelve codes on a synthetic subject. Both constants are filled from the implementer's own measurement (§11.2). (§6.2 rule 9) **Erratum (IN1b-R10, 2026-09-15):** "`<= IN1_INCIDENT_SERIALIZED_CEILING_BYTES` for all **twelve** codes on a **synthetic subject**" becomes **67** codes across all **seven** subject shapes — 469 pairs ([IN1b](IN1B-ERROR-MIGRATION.md) §9 clause 16). The ceiling constant moves from 1 024 to **2 048** and `IN1_INCIDENT_SERIALIZED_BYTES` stays **819**.
15. **The wire body cannot drift from the types.** A test serialises the deterministic fixture incident and `assert_eq!`s the result against §6.2 rule 10's literal, byte for byte. (§6.2 rule 10)
16. **The agent scripts.** Every `in1_` test of §7 **that survives §12's cut order** passes on both operating systems (N2/S10).
17. **The undo sticks.** After a router auto-apply followed by a global `Command::Undo`, the same fixture error observed again yields `Observed::Suppressed`, **no further operation is sent for that asset for the session**, and the first incident reads as reverted by the person rather than being re-applied. (§2.3 rule 19, §5.2b rule 18)
18. **`assumed_from` cannot be supplied.** `AddAsset` with `assumed_from: Some(_)` is refused with `OpError::AssumedFromNotSuppliable` and changes nothing; with `None` it is accepted; and a `RelinkAsset` over an asset carrying `assumed_from: Some(d)` leaves both `assumed_from` and `color_description` byte-identical. (§4.4 rules 26–27) **Erratum (implementation, 2026-09-15):** acceptance with `None` is additionally conditional on `color_description.provenance` not being `ColorProvenance::AgentAssumption`, which `add_asset` refuses with the same variant (§4.4 rule 26's erratum).
19. **`rec709_compatible` is one function, not two.** Its explicit chain and its two-clause definition agree over the constructed cross product of §2.4 rule 37. (§2.4 rules 37–38)

**Regressions guarded and process gates.** None of these is a clause: each either passes at `c3a5814` by construction, or is not a `cargo test`. They are listed because a reviewer must still see them discharged (N2/S2).

- **R1 — `unsupported_source_colour_refuses_the_export_with_its_code_and_field`** (`crates/kinewright-app/src/export_ui.rs:2017-2068`) passes **unmodified**. It *does* read a `MediaError` (`export_ui.rs:2064-2067`), but the `Backend` it matches is built on the delivery-preflight path from the `QaIssue` code at `crates/kinewright-core/src/delivery.rs:536`, never by `decode.rs:1081` or `render.rs:704`. Its staying green is the evidence that §4.2 did not leak into the delivery preflight. (§0.1 N1.7/C2)
- **R2 — the two `"application_default or user_override"` literals** at `crates/kinewright-core/src/delivery.rs:2211` and `crates/kinewright-media/src/export.rs:2677` are **unmodified and green**, which is the observable consequence of N2/B9's reversal. Neither file appears in §14. (§4.3 rule 15)
- **R3 — the six core round-trip tests probe-2 §T6.1 names** stay green unmodified: `project_json_round_trip_preserves_exact_document_equality`, `source_fingerprint_defaults_round_trips_and_is_exposed_in_schemas`, `pre_cc4_projects_round_trip_without_a_lut_assets_key`, `document_and_every_operation_variant_round_trip_through_json`, `source_colour_description_round_trips_known_and_future_values`, `clip_speed_serde_defaults_skips_and_round_trips` — and the whole `kinewright-core` suite stays at **16 binaries, 0 failures**, across the **109** mechanical `assumed_from: None` edits.
- **R4 — `opened_at` and `resolved_after` discipline, positively cut.** One test builds two incidents through `IncidentLog::with_start` and asserts `a.opened_at <= b.opened_at`; §8 rule 6's test asserts `resolved_after.is_some()`. **No other assertion in the workspace reads either field's value**, and neither is on the wire.
- **P1 — the build gates.** `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and **`cargo check -p kinewright-app`** — the last because of `ba0dd0a`, "gate the agent eval module with the fixture data it consumes", where a feature-gating regression compiled under `--workspace` and failed for the app alone. Every cargo command runs after `source ./scripts/setup-ffmpeg.sh`.
- **P2 — two review passes per crate**, per the slice recipe (N0.6), before each commit.
- **P3 — the promotion note records the two `[probe-2b]` constants** and the measured saving of §11.2, so the contract in `docs/` carries numbers and not markers.

---

## 10. Part B — the migration, sized

**This section is a sizing taken at `c3a5814`, not a contract, and it is discharged by [IN1b — the error migration](IN1B-ERROR-MIGRATION.md).** That companion restates the population, re-measured at `d4ed8eb`, as **129** measured error paths plus **3** log-bypassing sinks — **132** rows in its Appendix B — and it supersedes rules **1**, **4**, **6** and **8** below wherever the errata on them say so (IN1b-R6, IN1b-R7, IN1b-R5 and IN1b-R16).

Part B lands as `feat: complete IN1b error migration` after Part A's `feat: complete IN1a incidents and the colour case`.

1. **The population, written once, in this form, and used with these figures everywhere in this contract.** The tree at `c3a5814` has **121** `record_error(` call sites with a literal source label over **15** labels, **plus 2** dynamic-source sites, **plus 3** direct `ErrorLog::push` bypasses — 123 `record_error` call sites and **126** error paths in all. **Part A migrates exactly 1** of the 121. **Part B owns the remaining 120 + 2 + 3 = 125.** **Erratum (IN1b-R6, 2026-09-15):** re-measured at `d4ed8eb` by [IN1b](IN1B-ERROR-MIGRATION.md) §2: **124** literal-labelled sites over **16** literal spellings (**17** reachable labels), **2** dynamic-source sites and **3** bypasses — **129** error paths, three of them added by Part A. The comparable pair is 126 → 129, not 125 → 129.

| source label | sites | dominant payload | typed today | Part B note |
| --- | ---: | --- | :-: | --- |
| Operations | 32 | `OpError` (154 variants after §4.4 rule 26), `BatchError` | 2 | needs rule 4's family map |
| Look | 16 | `String`, 2× `MediaError`, 2× `io::Error` | 2 | mostly `Explain` |
| Export | 12 | `DeliveryVariantError`, reports, `MediaError` | 5 | highest typed density; `DeliveryColorError`/`DeliveryVerificationError` already carry `code()` |
| Source monitor | 11 | `SourceEditRejection`, literals | 1 | typed once `MediaError::SourceColor` exists |
| Relink | 8 | `RelinkRevisionConflict`, `RelinkRejection`, `MediaError` | 6 | best-typed label proportionally |
| Agent branch | 7 | `BatchError`, `BranchError` | 5 | |
| Transcript edit | 7 | `MediaError`, `String` | 2 | |
| **Media** | **6** | `MediaError` | 2 | **1 migrated in Part A** (`app.rs:1252-1255`); 5 remain |
| Agent | 5 | `AgentError`, event payload | 2 | |
| Recording | 5 | `String` | 0 | `Explain` |
| Project | 4 | `ProjectSaveError`, `String` | 1 | `load_document` (`app.rs:2020`) collapses three failures into one `String` — the highest-value typing opportunity outside colour |
| Captions | 3 | `String` (`captions.rs:42` and `:79` discard a `MediaError` with `.map_err(\|error\| error.to_string())?`) | 0 | cheapest single-line typing win in the crate |
| Mixer | 3 | `String` | 0 | `Explain` |
| Branch preview | 1 | `MediaError` | 1 | |
| Media cache | 1 | `MediaError` (`cache_clear_failed`) | 1 | |
| *(dynamic)* `app.rs:403` | 1 | `&'static str` from `load_error` | — | rule 5 |
| *(dynamic)* `inspector_ui.rs:793` | 1 | `&'static str` from `edits.error_category()` | — | rule 5 |
| *(bypass)* `app.rs:693` / `:708` / `:1005` | 3 | `String` | — | rule 6 |

   Measured 2026-09-15 at `c3a5814` (probe-1 §P1.1). Overall: **30 of 121 (24.8 %)** format a domain error enum, **82 (67.8 %)** are string-only, 4 are `std::io::Error`, 3 are typed-report summaries, 2 are typed events with `String` payloads.
2. **`MediaError` is the largest per-type family at 10 sites**, which is why Part A's `Media` migration reaches it first. Next: `RelinkRevisionConflict` + `RelinkRejection` 5, `BatchError` + `OpError` 4, `BranchError` 3, `DeliveryVariantError` 3, `AgentError` 2.
3. Part B's order **must** be: the typed families first (`MediaError`'s remaining 9, then Export's 5 typed, then Relink's 6, then Agent branch's 5), because each brings a `code()` with it; then `OpError`; then the string-only labels as `Explain` placeholders.
4. **`OpError::incident_family()`.** `OpError` has **154** variants after §4.4 rule 26 (`crates/kinewright-core/src/operation.rs:494-1056` plus the new one) and **no** `code()`. Part B adds a `const fn OpError::incident_family(&self) -> IncidentFamily` with approximately **8** families — `Validation`, `Missing`, `RangeOrBounds`, `IdExhausted`, `Duplicate`, `Ordering`, `ColorPolicy`, `TimeOverflow` — with an exhaustive core match test, and decides per-variant codes from Part A's experience. 154 codes would blow the slice; one catch-all would blow "typed at the boundary" for the single largest source. **Erratum (IN1b-R7, 2026-09-15):** "approximately **8** families — `Validation`, `Missing`, `RangeOrBounds`, `IdExhausted`, `Duplicate`, `Ordering`, `ColorPolicy`, `TimeOverflow`" becomes the **11** families [IN1b](IN1B-ERROR-MIGRATION.md) §3.1 rule 1 names over all 154 variants, and per-variant codes are **two**, not none (IN1b §3.1 rule 6).
5. **The two dynamic sites** become typed at their producers — `load_error` returns `Option<IncidentObservation>` and `edits.error_category()` becomes `edits.incident_code()` — or they are named as the two `Explain` placeholders Part B closes. Either is acceptable; silence is not.
6. **The sink gate.** `ErrorLog::push` becomes private to `error_ui.rs`, the only writer is the incident router, and the grep gate covers `record_error(` **and** `error_log.push(` with an expected count, not `record_error(` alone — which passes today with three live untyped paths, two of them in the `Media` family IN1 exists to fix. **Erratum (IN1b-R5, 2026-09-15):** "the only writer is the incident router" is superseded by [IN1b](IN1B-ERROR-MIGRATION.md) §5.3: the single writer is `KinewrightApp::note_incident`, a method on the app rather than on the router, because `status` and `error_log_open` are app fields (N0/Q5, N1/B5). The gate is four counted greps reading 0 / 0 / 1 / 1, not one (IN1b §5.3 rule 20).
7. **The badge** moves to open incidents; the log window keeps counting entries; the contract states the two numbers are different quantities (§5.2 rule 15 already says why).
8. `POLICY` becomes exhaustive over the full code set. `IncidentSubject` grows to `Clip`, `Track`, `ExportJob`, `Project`, `Agent`. **Erratum (IN1b-R16, 2026-09-15):** "`IncidentSubject` grows to `Clip`, `Track`, `ExportJob`, `Project`, `Agent`" becomes **six** added variants, not five — `Chain(AudioChain)` joins them (seven in all with Part A's `Asset`) — and `ExportJob`, `Project` and `Agent` are **unit** variants carrying no id ([IN1b](IN1B-ERROR-MIGRATION.md) §3.3 rule 18).
9. Part B's exit gate is Part A's clause 1 rewritten as a sink gate with an expected count, plus one per-source test.
10. Part B also owns **multi-project attribution** (§13 D14) and the **badge semantics** (§13 D4).
11. The sizing table above is **provisional for `Operations`** until `OpError::incident_family` exists; every other row is measured.

---

## 11. Measurement limits and what the implementer still measures

### 11.1 Limits IN1 records rather than papers over

1. **The engine emits the same refusal `1 + n` times, and no count is pinned on the app path.** Probe-2 §R5 measured 2 emissions for `set_document` plus one `request_frame`, 3 with `play`, 4 for three requests, all through `engine.rs:2050` (`Worker::fail`) from two or three different `present` callers. `handle_coalesced_requests` presents once per `frame_sequence` bump (`engine.rs:1872-1877`) and `Playback::request_frame` bumps it on every call (`engine.rs:676-682`), and the failed decode is not cached (`render.rs:501-514`), so a live app re-fails on every repaint. The event channel is `bounded(16)` with drop-oldest (`engine.rs:406`, `send_latest` at `:2058-2066`), so an error can also be dropped under pressure. What the contract asserts is the property dedup actually buys: **one incident, not n** (§9 clause 4).
2. **The person path is asserted at the builder and data level, not in a running app.** There is no `crates/kinewright-app/tests/`, `KinewrightApp::new` is private (`app.rs:206`), and AU6 §1 already records that a live app harness is not delivered (§13 D12). The card is proven as a pure function; its placement is not proven at all.
3. **`opened_at` is session-scoped and not comparable across processes.** `Instant` is not serializable; a persisted incident needs a different stamp, which is IN2's problem (§13 D2). This is why it is off the wire.
4. **Windows costs are unmeasured in both directions.** Probe-2 ran on Linux/lavapipe: the fixture encode cost (0.572 s, 72 649 B) and the decode-event latency (max 0.0754 s over five runs) are Linux figures, and probe-2 could not measure either on Windows. This is the whole reason `IN1_DECODE_EVENT_DEADLINE` stays at 10 s (§7 rule 13) — a cold Windows runner loading the FFmpeg DLLs for a first VP9 decode is not bounded by a warm Linux figure.
5. **No IN1 number is per-OS.** Every pinned number in §9 is the same on both operating systems. If one is ever found that differs, it becomes a doc-comment note beside the single constant, never a second constant.
6. **The `in1_` lane wall time is an estimate, not a measurement** (§7 rule 16), because the tests do not exist yet.
7. **What a fully untagged MP4 cannot prove.** MP4 does not carry a `Colour/Range` element on this build, so `in1_untagged.mp4` alone can never exercise the Helen Hill row's `StreamMetadata`/4 000 evidence. That is the whole job of the WebM pair, and it is why §12 cuts it second rather than first.

### 11.2 What the implementer measures — the three `[probe-2b]` figures

Probe-2 measured the **draft's** incident shape. Revision 2 drops `message`, skips `None` telemetry, keeps `opened_at` off the wire and changes `RecoveryKind`'s tagging, so **every serialised figure moves**. The probe-2b run happens **against the implementation, not another prototype**: the implementer measures, fills the constants, and the promotion note records them (§9 process gate P3).

| # | figure | draft-shape measurement (probe-2) | what the implementer measures |
| ---: | --- | ---: | --- |
| 1 | `IN1_INCIDENT_SERIALIZED_BYTES` **[probe-2b]** | 1 276 B (`in1_untagged_vp9.webm`, largest of four; MP4 1 269 B) | `serde_json::to_vec(&incident).len()` for the same named incident on the revision-2 shape, asserted with `assert_eq!` |
| 2 | `IN1_INCIDENT_SERIALIZED_CEILING_BYTES` **[probe-2b]** | worst of twelve codes 1 444 B (`unsupported_source_combination`) | the worst of the twelve on the revision-2 shape, with the constant set to the smallest power of two above it, asserted `<=` |
| 3 | the measured saving **[probe-2b]** | — | figure 1 against the draft shape's 1 276 B, stated as a measured percentage in the promotion note and the M36 rows. This is IN1's one true token-efficiency statement beyond the served-quad pin, and **it must be a measured number, not an adjective** (N2/S4). |

§6.2 rule 10's hand-serialised 819 B is arithmetic on this page, offered as a sanity anchor for figure 1 and **not** as a value to paste into a constant.

There are **no `[probe-2]` markers left in this contract**. Every number probe-1 or probe-2 measured is written out where its marker stood.

---

## 12. Line budget, and what is cut first

**Size, re-estimated against revision 2's rules.** IN1 Part A is one new core module, two `MediaError` variants with their plumbing, one `MediaAsset` field with its guard and its `AddAsset` refusal, one app router, one pure card function, two capabilities and seven agent tests — plus one mechanical 109-line diff nothing in the draft accounted for. Estimate **4 000–5 300 insertions** across five crates, against AU6's measured 11 000–15 000 for five scenarios.

| § | deliverable | crate | estimate |
| --- | --- | --- | ---: |
| 2 | `incident.rs` — twelve codes, `POLICY`, `IncidentLog` with suppression, `Observed`, the observation, `recovery_description`/`assume_rec709_operation`, `rec709_compatible`, the manual `Serialize`, and their tests | core | 1 200–1 500 |
| 4.1–4.2 | `MediaError` variants, the long `#[error]` attribute, `recovery_code()`, the two construction sites, the amended test | core + media | 300–400 |
| 4.3 | `AgentAssumption` and the one provenance-does-not-gate test (N2/B9 removed the two allowlist tests) | core | 60–90 |
| 4.4 | `assumed_from`, the guard, the bookkeeping, the `AddAsset` refusal and its `OpError` variant, eleven tests | core | 350–450 |
| 4.4 | **the 109 `MediaAsset` struct literals that need `assumed_from: None`** across `--workspace --all-targets` (probe-2 §T6.1, three compile passes: 32, 46, 31) | workspace | **109** |
| 3 | `in1_sources.rs` and the nine fixture assertions | media | 350–450 |
| 5 | router, `incident_ui.rs`, the fifteen-row headline table, card tests | app | 500–700 |
| 6 | two capabilities, arg structs, descriptions, handle threading on `ProjectSession`, the five pins | agent | 500–700 |
| 7 | seven `in1_` tests | agent | 400–550 |
| 8 | `QuestionKind`, `IncidentTelemetry`, their tests | core + agent | 120–180 |
| docs | this file promoted, `ROADMAP-AND-WORKFLOWS.md` status and row, `M36` two rows, `CHANGELOG.md` | docs | 150 |
| | **total** | | **4 039–5 279** |

The `incident.rs` row is the one that moved most (nit 8): it carries twelve codes, `POLICY`, the log with two suppression entry points, four public types, the observation, the recovery builder, the predicate with its equivalence test and a manual `Serialize` — against §4.4's eleven guard tests alone at 350–450 in a crate where one house-style test is 25–45 lines. The 109-line row is mechanical and cannot be cut.

**Cut order.** Exactly three things are cuttable, each because its failing direction costs a fixture the canonical measurement does not:

1. **`in1_a_wrong_outcome_claim_is_refused_by_code`** (§7 rule 8). The two refusal codes are still exercised by their construction sites; the test proves the server verifies the claim. Cut **first**.
2. **The WebM pair** (`in1_untagged_vp9.webm`, `in1_tagged_vp9.webm`). The MP4 pair carries every §9 clause; the WebM pair proves that the Helen Hill row's *evidence* differs while its *code* does not, on a real container rather than on constructed descriptions (§11.1 limit 7). Cutting it saves **0.511 s of the 0.572 s** encode cost and 53 294 of the 72 649 bytes and buys nothing else. If it is cut, **it is cut from the contract before promotion and §11.1 records the lost coverage** — never left as a red Windows lane, never as a runtime skip (N2/S10). Probe-2 §T4.2 removed the reason it might have been cut. Cut **second**.
3. **`details`' key-order assertion** (§5.3 rule 32). The values are asserted through core accessors either way. Cut **third**.

**Never cut**, because they are the exit gate: the MP4 pair and its pinned tuple; media fixture 9 (§3 rule 14); `POLICY`'s exhaustive match; `rec709_compatible`'s equivalence test; the guard's by-name refusal test; the revert round trip; the `AddAsset` refusal; the undo-sticks test; §9 clause 6's S8 equality over the whole table; §9 clause 15's wire-body literal; the served-quad pins in both sites; `cargo check -p kinewright-app`.

---

## 13. Explicit deferrals

Each names why it is a slice and not a flag, and carries a cost and an owner.

- **D1 — the older-reader downgrade to `ColorProvenance::Other("agent_assumption")` (S4).** `color_tag!`'s `Deserialize` maps an unrecognised string to `Other(String)` and round-trips it verbatim (`crates/kinewright-core/src/color.rs:52-63`), so an older Kinewright reading an IN1 project loads a valid `rec709_video` tuple with the attribution gone — CC1 §1's failure mode, one version removed. **Not free:** the mitigation is persisting the incident, or a project-format version gate, both of which change the file contract. **Owner: IN2**, together with Q8's persistence question. IN1 mitigates it partially: `assumed_from` *does* persist and an older reader round-trips it untouched, so the evidence survives even when the label does not. **Owner line added at IN2 Part A's promotion (2026-09-16):** **Owner: IN2 Part B** (`IN2B-INCIDENT-PERSISTENCE.md`) — the project-format version gate rides on D2's file, and Part A adds a second field an older reader silently drops, `Document.investigator` ([IN2](IN2-INVESTIGATOR-SESSIONS.md) §13, §2.4 rule 13). **Erratum (E-B7, 2026-09-22):** restated as its three facts — (1) no Part B field can warn an already-shipped older build; (2) Part B ships a marker future pairs honour, a newer `format_version` opening with an incident and disabled overwrite-save; (3) the gate covers provenance AND `Document.investigator` ([IN2B](IN2B-INCIDENT-PERSISTENCE.md) §0.5 E-B7, §4 rule 8). **Owner line added at IN2 Part B's promotion (2026-09-22):** **Discharged by IN2 Part B** ([IN2B-INCIDENT-PERSISTENCE.md](IN2B-INCIDENT-PERSISTENCE.md) §4) — the version rides in an app-side `ProjectFile` envelope with v1 bytes identical, never a `Document` field.
- **D2 — persisting incidents, and a cross-process timestamp.** IN1's log is session state (Q8). A persisted incident needs a stamp that is neither `Instant` (unserializable) nor `SystemTime` (non-monotonic). **Not free:** half a day plus a project-format field. **Owner: IN2.** **Owner line added at IN2 Part A's promotion (2026-09-16):** **Owner: IN2 Part B** (`IN2B-INCIDENT-PERSISTENCE.md`); Part A chooses the shape Part B must deserialise — three `IncidentState` variants, and an `IncidentProposal` whose `operations` are `#[serde(skip)]`, so every loaded proposal is `stale` and offers **Re-investigate** rather than **Approve** ([IN2](IN2-INVESTIGATOR-SESSIONS.md) §13, §4.2 rule 10). **Owner line added at IN2 Part B's promotion (2026-09-22):** **Discharged by IN2 Part B** ([IN2B-INCIDENT-PERSISTENCE.md](IN2B-INCIDENT-PERSISTENCE.md) §§2–3) — a `<stem>.kinewright-incidents` sidecar beside the project with an injected wall origin, loaded proposals always stale, suppression never persisted.
- **D3 — Part B's 125 error paths.** The sizing table is §10 rule 1, and it is the cost. `OpError::incident_family()` with approximately **8** families and an exhaustive core match test over **154** variants is its largest single item; the **2** dynamic-source sites and the **3** `ErrorLog::push` bypasses are its smallest and are already named. **Owner: IN1 Part B.** **Owner line added at IN1b's promotion (2026-09-15): IN1b, [`IN1B-ERROR-MIGRATION.md`](IN1B-ERROR-MIGRATION.md)**, which re-measures this deferral's population as 129 error paths plus 3 log-bypassing sinks and sizes every item of it.
- **D4 — badge semantics (S11).** The badge counts open incidents once the log is demoted to audit; Part A leaves it counting log entries so it does not regress while **120** literal-labelled sites are unmigrated. **Cost:** one predicate and the Part B sink gate. **Owner: IN1 Part B.** **Owner line added at IN1b's promotion (2026-09-15): IN1b §5**, where the badge reads `open_count()` (§5.4 rule 25) and the sink gate is four counted greps (§5.3 rule 20); discharged, see the erratum IN1b-R8 on §1 item 10.
- **D5 — `IncidentState::Investigating`.** Nothing constructs it while there is no session. **Cost:** one variant and the exhaustive matches that follow it. **Owner: IN2.** **Owner line added at IN2 Part A's promotion (2026-09-16):** **Discharged by IN2 Part A** ([IN2-INVESTIGATOR-SESSIONS.md](IN2-INVESTIGATOR-SESSIONS.md) §3.6), which declares the third top-level variant between `Open` and `Resolved`, adds `is_open()`, and decides the five silently-changing comparison sites one by one.
- **D6 — a typed broker payload and `Approved`/`Rejected` outcomes.** `ConfirmationRequest` is `{ id: u64, tool_name: String, description: String }` (`crates/kinewright-agent/src/server.rs:137-142`) with a **private** `ConfirmationDecision::Rejected(String)`, a 60 s one-shot deadline (`server.rs:114`) and a draining `pending_requests()` that uses `try_iter()` (`server.rs:180-184`), so a second consumer would steal requests. It cannot carry an incident, and a downstream crate cannot even name the decision type. **Not free:** either every one of the **6** `confirm(&str, String)` call sites changes, or the broker becomes generic, or a second parallel broker exists. **Owner: IN2.** **Owner line added at IN2 Part A's promotion (2026-09-16):** **Discharged by IN2 Part A** ([IN2-INVESTIGATOR-SESSIONS.md](IN2-INVESTIGATOR-SESSIONS.md) §4.5 and §4.6): `IncidentOutcome` gains `Rejected` (§4.5 rule 20), and `ConfirmationBroker::for_incident` stamps `incident` on the request at construction — route (a) executed at one site instead of the six `confirm(` call sites (§4.6 rule 25).
- **D7 — `RecoveryKind::Relink` and `RecoveryKind::Transcode`, and the `AskFirst` class in practice.** IN1 declares neither variant because nothing would construct them, and declares `AskFirst` with no table entry. **Owner: IN3**, with the conversions for unsupported profiles.
- **D8 — conversions for unsupported profiles, per-code `Explain` bodies, and `assume_srgb_full`.** Every `unsupported_*` code is `Explain` in IN1: telling a person their BT.2020 source is not manageable is honest; silently transcoding it is not, and there is no transcode execution path. Two named sub-items ride here: (a) `ColorSourceError::recovery_action()` returns one literal for all thirteen variants (`color.rs:327-331`), so all nine `Explain` bodies are identical and mention relink, which IN1 cannot do (§2.5 rule 47); (b) a source whose known tags point at `srgb_full` — the second managed profile, `color.rs:702-710`, `docs/CC1-MANAGED-SDR-PRIMARY.md:67` — is `Explain` in IN1 because the only recovery IN1 ships targets `rec709_video` (§2.4 rule 38). **Not free:** a transcode proposal needs a target-profile vocabulary, a proof, and a place to put the output; per-code bodies need twelve written sentences with a reviewer who knows colour. **Owner: IN3.** **Owner line re-affirmed at IN2 Part A's promotion (2026-09-16):** **Owner: IN3**, carried as [IN2](IN2-INVESTIGATOR-SESSIONS.md) §13 D-A2 — Part A leaves the seven `unsupported_source_*` rows `Explain` with no operation, the only codes of the 67 with neither a button nor a session (IN2 §7 rule 5, §10 limit 11).
- **D9 — a headless session against the live project.** The plumbing to start a session with no chat panel **already exists**: `crates/kinewright-agent/src/eval.rs:1601-1634` builds an `McpServer` from a fixture `Core`/`Playback`/`Analysis`/`Export`, constructs a `SessionConfig` at `eval.rs:1620` and runs headless from `src/bin/kinewright-eval.rs` with no egui — and it **auto-approves every confirmation** (`eval.rs:3864-3868`), which is the closest existing analogue to IN1's router. What does not exist is a start against the app's **live** project: the app's only session start is `send_agent_message` (`crates/kinewright-app/src/chat_ui.rs:405-443`), which needs a project index, a thread index, a rendered `AgentThread`, a per-thread isolated `McpServer`/`TimelineBranch` and a user-typed message. **Owner: IN2.** **Owner line added at IN2 Part A's promotion (2026-09-16):** **Discharged by IN2 Part A** ([IN2-INVESTIGATOR-SESSIONS.md](IN2-INVESTIGATOR-SESSIONS.md) §3.3), an eighth `McpServer` constructor, `start_investigator_session`, which starts headless against the live project's own `IncidentLogHandle` and `ProjectPathHandle` on a branch core, pumped by §3.5.
- **D10 — per-harness cost telemetry (Q10).** Cursor ACP reports **no** cost categories; Codex reports categories but **no `cost_usd`**. IN4's "efficiency budgets green on every harness" is unsatisfiable on Cursor without driver work. Recorded in the **programme** deferrals of `docs/ROADMAP-AND-WORKFLOWS.md`, not only here. **Owner: IN3.** **Owner line re-affirmed at IN2 Part A's promotion (2026-09-16):** **Owner: IN3**, carried as [IN2](IN2-INVESTIGATOR-SESSIONS.md) §13 D-A5 — per-harness budgets and the Cursor driver work stay IN3's and the efficiency half is IN4's gate; Part A ships one token number per install with a per-harness “unknown” (IN2 §5.3 rules 9–10).
- **D11 — a per-session server-side tool surface.** `SessionConfig.tool_names` (`crates/kinewright-core/src/agent.rs:47-49`) is an allowlist passed to the harness CLI, while the server applies **one global filter for everybody** (`server.rs:649-654`, `:665-670`). "Investigator-only capabilities" does not exist as a mechanism. B7's capability route dissolves the question for IN1; a slice that needs it must build it. **Owner: IN3.** **Owner line re-affirmed at IN2 Part A's promotion (2026-09-16):** **Owner: IN3**, carried as [IN2](IN2-INVESTIGATOR-SESSIONS.md) §13 D-A3 — Part A's `capability_denylist` is one `&'static [&'static str]` field on one struct consulted at one call site, deliberately not this mechanism (IN2 §6 rule 2, §1 non-delivery 7).
- **D12 — a live `KinewrightApp` harness.** Unchanged from CC7 §13 and AU6 §13: a harness needs a constructible app, an injectable media engine, a deterministic frame pump, and a way to assert on painted output. **This is a programme-level item with no owning slice**, recorded in the programme deferrals of `docs/ROADMAP-AND-WORKFLOWS.md` alongside D10, exactly as CC7 §13 and AU6 §13 recorded it. **Owner: the programme roadmap.** **Owner line re-affirmed at IN2 Part A's promotion (2026-09-16):** unchanged — **Owner: the programme roadmap.** [IN2](IN2-INVESTIGATOR-SESSIONS.md) §13 lists it under “Unowned, and re-deferred rather than cut” rather than giving it to IN3 (IN2 §1 non-delivery 10, §10 limit 1).
- **D13 — the two managed-colour allowlists, if a later slice ever assumes a *project context* description** (N2/B9). Today `color_description_matches_managed` (`color.rs:946-962`) and `delivery_color_mismatches` (`delivery.rs:622-631`) are called only on `ColorContext` descriptions, never on a `MediaAsset`'s, and `set_color_context` (`operation.rs:1662-1673`) has no provenance guard at all, so widening them in IN1 would be dead code that silently blessed an unruled behaviour. **When a slice does assume a context description, the two `matches!` arms, the `"application_default or user_override"` phrase, `delivery.rs:2211` and `export.rs:2677` move together, in one commit.** **Owner: whichever slice first writes an assumed `ColorContext`.**
- **D14 — multi-project incident attribution** (N2/B6). `media_events` is a single app-level receiver from one engine (`app.rs:78`, `:232`), so Part A attributes a playback incident to the focused project (§5.2 rule 5). A background project whose document is not playing cannot produce one today; the day the engine is per-project, or `MediaEvent` carries a project tag, the rule moves. **Cost:** a project field on `MediaEvent` or a per-project receiver, plus the drain rewrite. **Owner: IN1 Part B.** **Owner line added at IN1b's promotion (2026-09-15): re-deferred to the slice that makes the engine per-project, or that tags `MediaEvent`**, per [IN1b](IN1B-ERROR-MIGRATION.md) §13 D14 — no Part B clause can fail on it today and nothing in Part B pre-builds for it.
- **D15 — the asset's human name on the incident.** `Incident.subject` serialises as `{"asset": 1}`, so an agent that wants to tell the person *which file* calls `get_timeline_state` — one extra round trip on the most common follow-up. A `subject_label: String` carrying the asset's `name` would remove it for roughly 25 wire bytes per incident, but it needs the router to read the document at observe time and it is not a ruling of N2. The card does not suffer: it is drawn in the Media panel beside the asset (§5.3 rule 33). **Owner: IN2**, with the persistence work that already touches the observation shape. **Owner line added at IN2 Part A's promotion (2026-09-16):** **Owner: IN2 Part B** (`IN2B-INCIDENT-PERSISTENCE.md`), carried there as IN1b §13 D-B6 and widened by the seven further subjects ([IN2](IN2-INVESTIGATOR-SESSIONS.md) §13).
- **D16 — the vestigial `media backend error: ` prefix.** §4.1 rule 3 keeps it so the sentence stays byte-identical. Dropping it is a visible-text change that belongs with Part B's demotion of `ErrorLog` to an audit view, when every colour refusal reaches the person as a card rather than as a log line. **Owner: IN1 Part B.** **Owner line added at IN1b's promotion (2026-09-15): IN1b keeps the prefix** — it removes only the three `strip_prefix` parsers that read it back (IN1b §6.2) — and **dropping the prefix is deferred to IN2 as IN1b §13 D-B4**, whose only remaining reason is IN1 §9 clause 11's 750 B template and the two pinned literals.

**The hand-run checklist.** Nothing below is a `cargo test`.

1. **The smoke session that opened the programme, repeated.** Import an untagged WebM into the running app, press play, and confirm that the card appears beside the asset in the Media panel, that nothing was asked, and that **Revert to probed description** restores the probed line. This is the session that produced the roadmap's "a person without colour knowledge had no way to find it".
2. **The Explain path, seen.** Import a genuinely unsupported source (a BT.2020/PQ file), confirm the card explains rather than applies, and confirm the Media-panel button is still reachable and still does what it did.
3. **The undo, seen.** Repeat session 1, then press Ctrl+Z, and confirm the probed description returns **and stays returned** — that the card's revert action greys out and no new card appears. §9 clause 17 is the automated half; this is the half a person can see.

---

## 14. Files

| Implementer | Crate | Files, exhaustively |
| --- | --- | --- |
| **A** | core | `crates/kinewright-core/src/incident.rs` (new); `src/lib.rs` (one `mod incident;` beside `mod color;` at `:22`, and one `pub use incident::{Incident, IncidentCode, IncidentEvidence, IncidentId, IncidentLog, IncidentObservation, IncidentOutcome, IncidentSeverity, IncidentState, IncidentSubject, IncidentTelemetry, Observed, POLICY, PolicyClass, PolicyEntry, PolicyPredicate, RecoveryAction, RecoveryKind, SourceColorIncident, assume_rec709_operation, policy_class, policy_recovery, rec709_compatible, recovery_description};` block in the shape of `lib.rs:92`); `src/color.rs` (**one** `color_tag!` line — no allowlist edit, see §4.3 rule 15); `src/media.rs` (two `MediaError` variants, two `recovery_code()` arms); `src/model.rs` (one `MediaAsset` field); `src/operation.rs` (the guard, the bookkeeping, one `validate_asset` line, one `add_asset` check, one new `OpError` variant, one amended `#[error]` string); `src/agent.rs` — **read, not edited**; `tests/contracts.rs` (the provenance, revert, zero-confidence, `AddAsset` and relink tests); **plus the mechanical `assumed_from: None` edits wherever they fall** (§12). `src/delivery.rs` and `crates/kinewright-media/src/export.rs` are **read, not edited** — §9 regression item R2. |
| **B** | media | `src/in1_sources.rs` (new); `src/in1_fixtures.rs` (new); `src/lib.rs` (two module lines); `src/decode.rs` (`open_scaled_managed`'s map, one line at `:1081-1086`); `src/render.rs` (one match arm in `contextual_managed_decode_error` before the catch-all at `:704-707`, one amended test at `:1328-1351` including its input). `engine.rs` is **read, not edited**. |
| **C** | app | `src/incident_ui.rs` (new); `src/app.rs` (the `Media` arm at `:1252-1255`, `route_incidents`, the conflict note in the core drain); `src/project.rs` (the `IncidentLogHandle` type alias beside `:207` **and** the `incidents` field on `ProjectSession` at `:210-216`); `src/chat_ui.rs` (two constructor calls at `:244-250` and `:295-301`); `src/color_ui.rs` (`assume_sdr_rec709_operation` delegates to the core builder); `src/media_bin.rs` (draw the card beside the asset at `:382-391`). `src/error_ui.rs` is **read, not edited** in Part A. |
| **D** | agent | `src/schema.rs` (`INSPECTOR_TOOL_NAMES` 84 → 86 at `:16`, two names); `src/server.rs` (two `Tool::new` entries at `:12219`, two arg structs and one outcome enum at `:10969`, two handlers, one `KinewrightMcp` field threaded through `configured` at `:600-638`, the two serialised-size constants, and the four pins at `:21794`, `:25476-25479`, `:25484-25498`, `:25499-25508` — **`:21794`'s third owner is `cc5_matte_tools_are_registered_read_only_inspectors` at `:21829`**); `src/runtime.rs` — **read, not edited**; `src/eval.rs` (`QuestionKind` at `:1291-1299`); `tests/mcp_server.rs` (seven `in1_` tests, the pins at `:2444-2456`, `:2486-2492`, `:2550-2554`). |
| orchestrator | docs | `docs/IN1-INCIDENTS-AND-THE-COLOUR-CASE.md` (this file, promoted, with the two `[probe-2b]` constants filled); `docs/ROADMAP-AND-WORKFLOWS.md` (the IN1 status paragraph, the IN1 row's exit gate **and its deliverable column** as amended by §9's table, and the two programme deferrals D10 and D12); `docs/M36-AGENT-RUNTIME-EFFICIENCY.md` (two rows); `CHANGELOG.md` (one entry per part). |

**Erratum ranges follow the crate, not the implementer**, as AU6 §12.4 sets: A writes R1–R19, B R20–R39, C R40–R59, D R60–R79, and §0 gains a `0.4 Changes from implementation and review` subsection at promotion carrying each by id.

**Order.** A → B → (C ‖ D). C and D both depend on A and B and may run in parallel; neither may assert a pinned byte count before the figures of §6.6 and §11.2 are in place. The workspace gate and `cargo check -p kinewright-app` run before each commit, and two review passes per crate precede each (N0.6). Every cargo command runs after `source ./scripts/setup-ffmpeg.sh`.

IN1 is complete only when a person who does not know what colour primaries are can import an untagged source, press play, watch it play, read one sentence saying what Kinewright assumed and why, and press one button to undo that assumption — and when the agent in the chat panel can read the same incident, with its code, its field, its class and its recovery, in one call, at the same revision, for zero additional served bytes.
